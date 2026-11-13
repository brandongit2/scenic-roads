//! The build page's data on the lead (docs/pool.md §10, task #409; the page is the lead's alone,
//! docs/roles.md §3.3): `/api/swarm` and `/api/tocome` from its coordinator's view. The history
//! isn't here: `/api/history` and the swarm's `seq`, `epoch` and `rates` are the coordinator's, from
//! its memory (crate::coord::history, task #1160).
//!
//! While a page has asked in the last `ASKED_FOR_S`, a thread of the agent (`spawn`) asks the
//! view of the lead's coordinator, in process, every `LEAD_EVERY` (`tick_lead`, `Lead`): its
//! status, its coordinator's view, the pause, the Macs' addresses (each member's broker: where a
//! page reads its `/api/live`), and the forecast's input and the page's regions when their time
//! changed. A view that can't be had leaves the last one, said with its age. It keeps one
//! snapshot: the view and the forecast run here (`forecast::Record::forecast`) with its time
//! brought to now, this Mac's own lanes as its slots are now (`Inputs::own`, worked out as the
//! lead works out its own: forecast::SlotJob). The forecast runs on a thread of its own (`Runs`,
//! task #544), again when the input changed or `forecast::EVERY` passed: the snapshot carries the
//! last one done, never waiting on it. Its tiles to come name what they wait on in the
//! coordinator's history's numbering (`Lead::waits_basis`).

use super::forecast::{Forecast, Record, SlotJob, ToCome};
use crate::coord::history;
use crate::coord::ViewAsk;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// A page that asked this long ago (s) keeps the asks going; after it, nothing is asked.
pub const ASKED_FOR_S: u64 = 60;
/// The lead's view is asked again this often at most (it answers from memory: docs/coordination.md
/// step 4).
pub const LEAD_EVERY: Duration = Duration::from_secs(2);

/// Where the lead's view is asked (crate::coord::ViewAsk): its coordinator, in process (the page is
/// the lead's alone, docs/roles.md §3.3); a test's fake.
pub trait Lead: Send + Sync {
    /// The view's answer to `ask`.
    fn view(&self, ask: &ViewAsk) -> anyhow::Result<Value>;

    /// What the forecast's waits are worked out from (history::WaitsBasis: `old`, made when the
    /// history's number was `set_at`, and the targets' last finishes), the history's number now and
    /// its numbering's name (`/api/history`'s `epoch`). None: no history here (a test's lead).
    fn waits_basis(&self, _old: Arc<history::Waits>, _set_at: u64) -> Option<(history::WaitsBasis, u64, String)> {
        None
    }

    /// The history's last number by time `t` (unix s; 0: none).
    fn seq_at(&self, _t: u64) -> u64 {
        0
    }
}

impl Lead for crate::coord::Coordinator {
    fn view(&self, ask: &ViewAsk) -> anyhow::Result<Value> {
        Ok(self.pool_view(ask))
    }

    fn waits_basis(&self, old: Arc<history::Waits>, set_at: u64) -> Option<(history::WaitsBasis, u64, String)> {
        Some(self.history_waits_basis(old, set_at))
    }

    fn seq_at(&self, t: u64) -> u64 {
        self.history_seq_at(t)
    }
}

/// Where the lead's view is asked (`Inputs::lead`); None, nowhere (no snapshot: the page's data
/// answered by the coordinator from its memory). Told apart by which `Lead` it is.
#[derive(Clone, Default)]
pub struct LeadAt(pub Option<Arc<dyn Lead>>);

impl PartialEq for LeadAt {
    fn eq(&self, o: &LeadAt) -> bool {
        match (&self.0, &o.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        }
    }
}

impl std::fmt::Debug for LeadAt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "LeadAt(..)" } else { "LeadAt(None)" })
    }
}

/// A member's status older than this (s) isn't shown (as the lead's own `helpers` takes them).
const HELPER_FRESH_S: u64 = 600;

/// What the agent tells it, each loop, from what it holds: this Mac's app, its own lanes now (only
/// while a page asks: `PageView::wanted`), and where the lead's view is asked (none: not in the
/// pool).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub app: String,
    pub own: Vec<OwnLane>,
    /// Where this Mac keeps the last run's waits across a restart (a local file: `KEEP_WAITS_S`).
    pub keep: Option<std::path::PathBuf>,
    /// Where the lead's view is asked: the lead's coordinator here.
    pub lead: LeadAt,
}

/// The last run's waits (its items' dependencies: `history::Waits`) are kept on this Mac's disk this
/// often at most, and read back for the first run after a restart (task #863): without them, that
/// run's tiles to come waited on nothing finished (no `after`), the schedule's arrows from the jobs
/// that ran missing until the jobs they waited on finished again.
pub const KEEP_WAITS_S: u64 = 300;

/// One of this Mac's lanes (a slot: the agent's `worker_of`) and its job under way, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OwnLane {
    pub name: String,
    pub job: Option<SlotJob>,
}

/// The lead's view as last asked.
#[derive(Clone, Debug, Default)]
struct Snap {
    status: Value,
    /// The lead's status's change token as answered (`ViewAsk::status_at`): sent again only once it changed.
    status_at: Option<u64>,
    view: Value,
    pause: Value,
    helpers: Vec<Value>,
    /// Each member's addresses, from its heartbeat's broker: where a page reads its `/api/live`.
    macs: Vec<Value>,
    /// When the lead made it (unix s).
    at: Option<u64>,
}

/// The forecast run here off the view (`PageView::forecast_run`): the last one done and the one
/// before it (a page that read the swarm's just before a run ended still gets its tiles to come:
/// `PageView::tocome`); the one running; the input's time and when the last was started; the waits
/// the last one left (its items' dependencies, as the lead's agent keeps them for its coordinator's
/// history: `history::Waits`) and the history's number when they were made.
#[derive(Default)]
struct Runs {
    done: Option<Forecast>,
    tocome: Option<Done>,
    prev: Option<Done>,
    running: Option<std::thread::JoinHandle<Ran>>,
    of: Option<(SystemTime, Instant)>,
    waits: (Arc<history::Waits>, u64),
    /// The kept waits read back (once a process), and when they were last kept.
    loaded: bool,
    kept: Option<Instant>,
}

/// A forecast run here's tiles to come (`/api/tocome`, task #836: made of the same run, so its
/// `at` is the swarm's forecast's), its runs' `after` in the coordinator's history's numbering
/// (`epoch`; none without its history). The one before the last kept without its forecast (#836
/// review: only its tiles are asked for).
struct Done {
    tocome: ToCome,
    epoch: Option<String>,
}

/// What a run's thread gives back: it, its tiles to come, and the waits it made (with the
/// history's number then).
struct Ran {
    forecast: Forecast,
    done: Done,
    waits: Option<(history::Waits, u64)>,
}

impl Runs {
    /// The one running, taken in once it's done (`wait`: waited for).
    fn take(&mut self, wait: bool) {
        if self.running.as_ref().is_some_and(|h| wait || h.is_finished()) {
            match self.running.take().map(std::thread::JoinHandle::join) {
                Some(Ok(r)) => {
                    self.done = Some(r.forecast);
                    self.prev = self.tocome.replace(r.done);
                    if let Some((w, at)) = r.waits {
                        self.waits = (Arc::new(w), at);
                    }
                }
                Some(Err(_)) => eprintln!("pageview: the forecast failed"),
                None => {}
            }
        }
    }
}

#[derive(Default)]
struct State {
    snap: Option<Snap>,
    /// The forecast's input as last read, by its file's time.
    record: Option<(SystemTime, Arc<Record<'static>>)>,
    /// The lead's regions for the page (crate::agent::regionsview) as last read, by its file's time.
    regions: Option<(SystemTime, Arc<Value>)>,
    /// When the lead was last asked.
    lead_last: Option<Instant>,
    /// Since when (unix s) and why the lead can't be asked: the last view kept meanwhile.
    lead_failed: Option<(u64, String)>,
}

/// The page's data on this Mac.
pub struct PageView {
    the_plan: &'static dyn crate::agent::seam::Plan,
    asked: AtomicU64,
    inputs: Mutex<Inputs>,
    state: Mutex<State>,
    forecast: Mutex<Runs>,
}

fn now_s() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl std::fmt::Debug for PageView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PageView")
    }
}

impl PageView {
    pub fn new(the_plan: &'static dyn crate::agent::seam::Plan) -> Arc<PageView> {
        Arc::new(PageView::of(the_plan))
    }

    fn of(the_plan: &'static dyn crate::agent::seam::Plan) -> PageView {
        PageView {
            the_plan,
            asked: Default::default(),
            inputs: Default::default(),
            state: Default::default(),
            forecast: Default::default(),
        }
    }

    /// A page asked: the asks go on (or start) for `ASKED_FOR_S`.
    pub fn touch(&self) {
        self.touch_at(now_s());
    }

    /// A page asked at `at` (unix s): a test's clock, the same as its ticks', so an ask can't land
    /// a second past the time the test reads it at.
    fn touch_at(&self, at: u64) {
        self.asked.store(at, Ordering::Relaxed);
    }

    /// Whether a page asked lately.
    pub fn wanted(&self, now: u64) -> bool {
        now.saturating_sub(self.asked.load(Ordering::Relaxed)) < ASKED_FOR_S
    }

    /// Whether it serves the page (the agent told it where the lead's view is: in the pool). Not:
    /// the coordinator answers from its memory, as out of the pool.
    pub fn active(&self) -> bool {
        self.inputs.lock().unwrap().lead.0.is_some()
    }

    /// What the agent tells it (`Inputs`).
    pub fn set_inputs(&self, i: Inputs) {
        let mut cur = self.inputs.lock().unwrap();
        if *cur != i {
            *cur = i;
        }
    }

    /// The lead's view asked of it (`Lead::view`), when a page asked lately and `LEAD_EVERY` has
    /// passed (the first at once); true when it asked.
    pub fn tick_lead(&self, now: u64) -> bool {
        if !self.wanted(now) {
            return false;
        }
        let Some(lead) = self.inputs.lock().unwrap().lead.0.clone() else { return false };
        if self.state.lock().unwrap().lead_last.is_some_and(|t| t.elapsed() < LEAD_EVERY) {
            return false;
        }
        self.ask_lead(&*lead, now);
        true
    }

    /// Asks the lead for its view (`Lead::view`) with what it has, and takes it in: its status,
    /// view, pause, Macs, and the forecast's input and regions when they changed. One that can't be
    /// asked: the last view kept, said with its age (`lead_failed`).
    fn ask_lead(&self, lead: &dyn Lead, now: u64) {
        let ask = {
            let st = self.state.lock().unwrap();
            ViewAsk {
                status_at: st.snap.as_ref().and_then(|s| s.status_at),
                forecast_at: st.record.as_ref().map(|r| ms_of(r.0)),
                regions_at: st.regions.as_ref().map(|r| ms_of(r.0)),
            }
        };
        let got = lead.view(&ask);
        let mut st = self.state.lock().unwrap();
        st.lead_last = Some(Instant::now());
        let v = match got {
            Ok(v) => v,
            Err(e) => {
                if st.lead_failed.is_none() {
                    eprintln!("pageview: the lead's view: {e:#}");
                }
                st.lead_failed.get_or_insert((now, format!("{e:#}")));
                return;
            }
        };
        st.lead_failed = None;
        let at = |k: &str| v[k].as_u64().map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms));
        match (at("forecast_at"), v.get("forecast_input")) {
            (None, _) => st.record = None,
            (Some(t), Some(f)) => match serde_json::from_value::<Record<'static>>(f.clone()) {
                Ok(r) => st.record = Some((t, Arc::new(r))),
                Err(e) => eprintln!("pageview: the forecast's input: {e:#}"),
            },
            (Some(_), None) => {}
        }
        match (at("regions_at"), v.get("regions")) {
            (None, _) => st.regions = None,
            (Some(t), Some(r)) => st.regions = Some((t, Arc::new(r.clone()))),
            (Some(_), None) => {}
        }
        // (Its status only when it changed since the last answer: the one kept otherwise.)
        let old = st.snap.take();
        let status = match v.get("status") {
            Some(s) => s.clone(),
            None => old.map_or(Value::Null, |o| o.status),
        };
        let helpers = helpers_of(&status, now);
        let macs = v["macs"].as_array().cloned().unwrap_or_default();
        st.snap = Some(Snap {
            status,
            status_at: v["status_at"].as_u64(),
            view: v["view"].clone(),
            pause: v["pause"].clone(),
            helpers,
            macs,
            at: v["at"].as_u64(),
        });
        let record = st.record.clone();
        drop(st);
        let own = self.inputs.lock().unwrap().own.clone();
        self.forecast_run(record.as_ref(), now, &own);
    }

    /// `/api/swarm`'s answer (the page's shape: crate::coord's handler, which adds the history's
    /// `seq`, `epoch` and `rates` from its memory), from the snapshot; None before the lead's first
    /// view. A page asked.
    pub fn swarm(&self) -> Option<Value> {
        self.touch();
        let app = self.inputs.lock().unwrap().app.clone();
        let forecast = {
            let mut r = self.forecast.lock().unwrap();
            r.take(false);
            r.done.clone()
        };
        let st = self.state.lock().unwrap();
        let s = st.snap.as_ref()?;
        let mut a = answer(s, forecast.as_ref(), &app, now_s());
        if let Some((since, why)) = &st.lead_failed {
            a["view_unreached"] = serde_json::json!({ "since": since, "why": why });
        }
        Some(a)
    }

    /// `/api/tocome`'s answer (task #836) from the forecast run here: the one at `at` (the swarm's
    /// forecast's `at` the page read: the last or the one before), else the last; with the `epoch`
    /// its `after` is numbered in (the coordinator's history's; null without it: no `after`). None
    /// before the first run. A page asked.
    pub fn tocome(&self, at: Option<u64>) -> Option<Value> {
        self.touch();
        let mut r = self.forecast.lock().unwrap();
        r.take(false);
        let d = [&r.tocome, &r.prev].into_iter().flatten().find(|d| Some(d.tocome.at) == at).or(r.tocome.as_ref())?;
        let mut a = d.tocome.answer();
        a["epoch"] = d.epoch.clone().into();
        Some(a)
    }

    /// `/api/regions`' answer (crate::agent::regionsview): the lead's as last read; None before
    /// they're read (or while the lead hasn't written them). A page asked.
    pub fn regions(&self) -> Option<Arc<Value>> {
        self.touch();
        self.state.lock().unwrap().regions.as_ref().map(|r| r.1.clone())
    }
}

/// The members' statuses fresh (`HELPER_FRESH_S`) from the lead's status's `helpers`, trimmed of
/// what the page doesn't show.
fn helpers_of(status: &Value, now: u64) -> Vec<Value> {
    status["helpers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| now.saturating_sub(h["beat"].as_u64().unwrap_or(0)) < HELPER_FRESH_S)
        .map(|mut h| {
            if let Some(o) = h.as_object_mut() {
                for k in ["regions", "checklist", "built", "recent"] {
                    o.remove(k);
                }
            }
            h
        })
        .collect()
}

/// A time as unix ms.
fn ms_of(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The swarm's answer at `now` from snapshot `s` and the last forecast run here.
fn answer(s: &Snap, forecast: Option<&Forecast>, app: &str, now: u64) -> Value {
    let mut agent = s.status.clone();
    if agent.is_object() {
        if let Some(f) = forecast {
            agent["forecast"] = serde_json::to_value(f).unwrap_or(Value::Null);
        }
        agent["helpers"] = Value::Array(s.helpers.clone());
    }
    // (A lead on an app before view.json: no leases, workers nor tasks; its status's own forecast
    // when it writes no input either.)
    let (leases, workers) = crate::coord::ages(&s.view, now);
    // (The pause as the lead mirrors it to the NAS, absent when the build goes on: never the
    // view's, which a Resume leaves saying "paused" until the lead writes it again.)
    let pause = s.pause.clone();
    serde_json::json!({
        "now": now,
        "pause": pause,
        "agent": agent,
        "leases": leases,
        "workers": workers,
        "tasks": s.view["tasks"],
        "task_mb": s.view["task_mb"],
        "app": app,
        "macs": s.macs,
        // (How old the lead's view is: it's kept while the lead can't be asked.)
        "view_age_s": s.at.map(|at| now.saturating_sub(at)),
    })
}

impl PageView {
    /// The lead's forecast (`record`, by its file's time) run here at `now` on a thread of its own,
    /// when none is running and its input changed or `forecast::EVERY` passed; none without a
    /// record. The snapshot's answer takes the last one done (`Runs`).
    fn forecast_run(&self, record: Option<&(SystemTime, Arc<Record<'static>>)>, now: u64, own: &[OwnLane]) {
        let mut r = self.forecast.lock().unwrap();
        r.take(false);
        // (No record: the last one dropped; one running kept until it's done, never two at once.)
        let Some((t, rec)) = record else {
            (r.done, r.tocome, r.prev, r.of) = (None, None, None, None);
            return;
        };
        let due = r.of.is_none_or(|(at, started)| at != *t || started.elapsed() >= super::forecast::EVERY);
        if r.running.is_some() || !due {
            return;
        }
        let (rec, own) = (rec.clone(), own.to_vec());
        let (keep, lead) = {
            let i = self.inputs.lock().unwrap();
            (i.keep.clone(), i.lead.0.clone())
        };
        // (The first run after a start: the waits the last process kept, in the history's numbering,
        // the coordinator's, the same across its restarts.)
        if !r.loaded && r.waits.0.is_empty() {
            r.loaded = true;
            let kept = keep
                .as_ref()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| serde_json::from_slice::<history::KeptWaits>(&b).ok())
                // (Kept over a day ago: the build moved on, nothing read back.)
                .filter(|k| now.saturating_sub(k.made_t) < 86_400);
            if let (Some(k), Some(l)) = (kept, lead.as_ref()) {
                // (Its `from`: the history's last event by then; a number past it isn't of then.)
                let from = l.seq_at(k.made_t);
                let w = history::Waits::from_kept(k, from, |_, q| (q <= from).then_some(q));
                r.waits = (Arc::new(w), from);
            }
        }
        // (Kept again once `KEEP_WAITS_S` passed.)
        let save = keep.filter(|_| r.kept.is_none_or(|t| t.elapsed().as_secs() >= KEEP_WAITS_S));
        // (What its items wait on, worked out from the lead's history as it is now, off its lock.)
        let basis = lead.and_then(|l| l.waits_basis(r.waits.0.clone(), r.waits.1));
        if save.is_some() {
            r.kept = Some(Instant::now());
        }
        let the_plan = self.the_plan;
        match std::thread::Builder::new().name("pageview-forecast".into()).spawn(move || {
            super::forecast::low_priority();
            let mut forecast = local_forecast(the_plan, &rec, now, &own);
            let (mut tocome, item_job, jobs) = ToCome::take(&mut forecast, &rec.input.machines);
            let graph = std::mem::take(&mut forecast.graph);
            let (waits, epoch) = match basis {
                Some((b, at, epoch)) => {
                    let w = b.next(graph);
                    tocome.runs.after = w.by_job(&item_job, jobs);
                    if let Some(p) = &save {
                        keep_waits(p, &w.keep(now, |n| Some((epoch.clone(), n))));
                    }
                    (Some((w, at)), Some(epoch))
                }
                None => (None, None),
            };
            Ran { forecast, done: Done { tocome, epoch }, waits }
        }) {
            Ok(h) => (r.running, r.of) = (Some(h), Some((*t, Instant::now()))),
            Err(e) => eprintln!("pageview: the forecast's thread: {e}"),
        }
    }
}

/// Writes kept waits to `p` whole (store::whole: a temporary name unique to the call, renamed over
/// it); a failure said, not kept.
fn keep_waits(p: &std::path::Path, k: &history::KeptWaits) {
    let done = p
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .map_err(anyhow::Error::from)
        .and_then(|_| store::whole::write(p, &serde_json::to_vec(k)?));
    if let Err(e) = done {
        eprintln!("pageview: the forecast's waits not kept at {}: {e}", p.display());
    }
}

/// The lead's forecast run here at `now`: each lane's time left and each cadence's time since
/// moved on by the time since it was made; this Mac's own lanes (`own`) as their slots are now,
/// their time left worked out as the lead works out its own (forecast::SlotJob, each target's cost
/// from the input's table, else its step's time a target: `each_s`).
pub fn local_forecast(
    the_plan: &'static dyn crate::agent::seam::Plan,
    r: &Record,
    now: u64,
    own: &[OwnLane],
) -> Forecast {
    let mut r = r.clone();
    let dt = now.saturating_sub(r.input.now) as f64;
    let (costs, each) = (r.input.costs.clone(), r.each_s.clone());
    let cost = move |step: &str, t: &str| {
        costs.get(step).and_then(|m| m.get(t)).map(|c| c.secs).or_else(|| each.get(step).copied()).unwrap_or(0.0)
    };
    let age = |m: &mut super::forecast::Machine| {
        m.from_s = (m.from_s - dt).max(0.0);
        match own.iter().find(|o| o.name == m.name) {
            Some(o) => {
                m.busy_s = o.job.as_ref().map_or(0.0, |j| j.busy_s(&cost, m.speed));
                m.running = o.job.as_ref().map(SlotJob::running);
            }
            None => m.busy_s = (m.busy_s - dt).max(0.0),
        }
    };
    r.input.machines.iter_mut().for_each(age);
    let c = &mut r.input.cadence;
    let dt_s = dt as u64;
    for x in [&mut c.since_low, &mut c.since_marks, &mut c.since_overlays].into_iter().flatten() {
        *x += dt_s;
    }
    r.input.now = now.max(r.input.now);
    r.forecast(the_plan)
}

/// The thread that asks the lead's view while a page asks, each second (`PageView::tick_lead`).
pub fn spawn(pv: Arc<PageView>) {
    std::thread::Builder::new()
        .name("pageview-lead".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            pv.tick_lead(now_s());
        })
        .ok();
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::forecast::{self, Cadence, Cost, Input, Machine};
    use super::*;
    use crate::coord::history::Event;
    use std::borrow::Cow;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// The lead's view as a test's files hold it (term 7's, as `fixture` writes them), in its
    /// coordinator's shape (crate::coord::Coordinator::pool_view); no history (`Lead::waits_basis`).
    pub(crate) struct Files(pub PathBuf);

    impl Lead for Files {
        fn view(&self, _: &ViewAsk) -> anyhow::Result<Value> {
            let raw = |p: &str| std::fs::read(self.0.join(p)).ok();
            let json = |p: &str| raw(p).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let status = json("lead-status.json").unwrap_or(Value::Null);
            let macs: Vec<Value> = status["beats"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    let br = b.get("broker")?;
                    Some(serde_json::json!({ "host": b["host"], "urls": br["urls"], "https": br["https"] }))
                })
                .collect();
            let mut v = serde_json::json!({
                "status": status,
                "view": json("lead-view.json").map_or(Value::Null, |v| v["view"].clone()),
                "pause": json("state/build/pause.json").unwrap_or(Value::Null),
                "macs": macs,
                "at": now_s(),
            });
            let at = |p: &str| std::fs::metadata(self.0.join(p)).and_then(|m| m.modified()).ok().map(ms_of);
            let fpath = "state/build/term/7/forecast-input.json";
            if let (Some(t), Some(Ok(r))) = (at(fpath), raw(fpath).map(|b| Record::from_bytes(&b))) {
                v["forecast_at"] = t.into();
                v["forecast_input"] = serde_json::to_value(r)?;
            }
            let rpath = "state/build/term/7/regions.json";
            if let (Some(t), Some(r)) = (at(rpath), json(rpath)) {
                v["regions_at"] = t.into();
                v["regions"] = r;
            }
            Ok(v)
        }
    }

    /// An ask as the agent's thread makes it while a page asks: the lead's view (from `root`'s
    /// files, `Files`); whether it asked.
    pub(crate) fn read(pv: &PageView, root: &std::path::Path, now: u64) -> bool {
        let asked = pv.wanted(now);
        if asked {
            pv.ask_lead(&Files(root.into()), now);
        }
        asked
    }

    fn put(root: &store::nasfs::NasRoot, p: &str, b: &[u8]) {
        let p = root.join(p);
        p.parent().unwrap().create_dir_all().unwrap();
        p.write(b).unwrap();
    }

    fn mac(name: &str, busy_s: f64) -> Machine {
        Machine { name: name.into(), speed: 1.0, measured: true, mem_mb: 8000, busy_s, ..Default::default() }
    }

    pub(crate) fn record(now: u64) -> Record<'static> {
        let c = Cost { secs: 600.0, known: true, peak_mb: 1000 };
        Record {
            input: Input {
                now,
                before: Vec::new(),
                drops: Vec::new(),
                regions: Cow::Owned(Vec::new()),
                costs: Default::default(),
                cadence: Cadence::default(),
                blind: None,
                chains: (0..4).map(|i| ("depth".to_string(), format!("6/1/{i}"), c)).collect(),
                after: Vec::new(),
                idle: Vec::new(),
                machines: vec![mac("m4", 300.0), mac("m1", 900.0)],
                running: BTreeMap::new(),
                macs: BTreeMap::new(),
                sched: None,
                pace: None,
                step_pace: Default::default(),
                unknown: Vec::new(),
                paused: false,
                along: Default::default(),
                code: crate::agent::forecast::VERSION,
            },
            each_s: BTreeMap::new(),
            waiting: BTreeMap::new(),
        }
    }

    /// The lead's files for term 7: its status, its view, its forecast's input, a member's status.
    /// (Also coord's tests': `/api/tocome` from a pageview.)
    pub(crate) fn fixture(root: &store::nasfs::NasRoot, now: u64) {
        let view = serde_json::json!({ "at": now, "view": {
            "pause": null,
            "leases": [{ "id": 1, "worker": "m1", "what": "Roads", "step": "unit", "n": 2, "began": now - 100, "progress": null, "beat": now - 10, "lapses": now + 290 }],
            "workers": [{ "name": "m1", "label": "M1", "kind": "native", "seen": now - 5, "fit": [] }],
            "tasks": { "done": 3 },
            "task_mb": { "tail": 400 } } });
        put(root, "lead-view.json", view.to_string().as_bytes());
        put(root, "state/build/term/7/forecast-input.json", &record(now).bytes().unwrap());
        // (m-a heard by its beat: its heartbeat and status in the lead's; m-b, on an older app, on
        // the NAS.)
        let status = serde_json::json!({
            "host": "m4", "beat": 1, "forecast": null,
            "helpers": [{ "host": "m1", "beat": now, "regions": [1] }],
            "beats": [{ "member": "m-a", "host": "m1", "broker": { "urls": ["http://100.1.1.1:8090"], "https": "https://m1.t.ts.net/" } }],
        });
        put(root, "lead-status.json", status.to_string().as_bytes());
        put(root, "state/pool/members/m-b.json", br#"{"member":"m-b","host":"gone"}"#);
    }

    /// The forecast run off the view (`Runs`), waited for.
    pub(crate) fn settle(pv: &PageView) {
        pv.forecast.lock().unwrap().take(true);
    }

    fn inputs(app: &str) -> Inputs {
        Inputs { app: app.into(), own: Vec::new(), keep: None, lead: LeadAt::default() }
    }

    /// (Task #836) `/api/tocome` is the forecast's the swarm's answer carries (the same `at`): the
    /// lead's agent's own had lagged a pageview's by a forecast, so every page dropped its own
    /// Mac's answer. The page asks with the `at` it read; a run done since leaves that one
    /// answered, the newest without one.
    #[test]
    fn the_tiles_to_come_are_the_swarms_forecasts() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        assert!(pv.tocome(None).is_none());
        pv.touch_at(now);
        read(&pv, d.path(), now);
        settle(&pv);
        let a = pv.swarm().unwrap();
        let at = a["agent"]["forecast"]["at"].as_u64().unwrap();
        let tc = pv.tocome(Some(at)).unwrap();
        assert_eq!(tc["at"].as_u64(), Some(at));
        assert!(tc["epoch"].is_null(), "a lead without a history: no numbering");
        assert!(tc["macs"].as_object().is_some_and(|m| !m.is_empty()), "{tc}");
        // (Another run 30 s on: the swarm's is the new one; the one read before still answered.)
        pv.forecast.lock().unwrap().of = None;
        read(&pv, d.path(), now + 30);
        settle(&pv);
        let at2 = pv.swarm().unwrap()["agent"]["forecast"]["at"].as_u64().unwrap();
        assert_eq!(at2, now + 30);
        assert_eq!(pv.tocome(Some(at)).unwrap()["at"].as_u64(), Some(at));
        assert_eq!(pv.tocome(Some(at2)).unwrap()["at"].as_u64(), Some(at2));
        assert_eq!(pv.tocome(None).unwrap()["at"].as_u64(), Some(at2));
    }

    #[test]
    fn the_snapshot_is_made_from_the_leads_view_in_the_pages_shape() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs("1.2.3"));
        assert!(pv.swarm().is_none(), "nothing before the lead's first view");
        pv.touch_at(now);
        assert!(read(&pv, d.path(), now));
        settle(&pv);
        let a = pv.swarm().unwrap();
        assert_eq!(a["app"], "1.2.3");
        assert_eq!(a["tasks"]["done"], 3);
        assert_eq!(a["task_mb"]["tail"], 400);
        let l = &a["leases"][0];
        assert!(l["for_s"].as_u64().unwrap() >= 100 && l.get("began").is_none());
        assert!(l["beat_s"].as_u64().unwrap() >= 10 && l["lapses_in_s"].as_u64().unwrap() <= 290);
        assert!(a["workers"][0]["seen_s"].as_u64().unwrap() >= 5);
        // (The lead's status, its forecast run here, the members' statuses fresh, trimmed.)
        assert_eq!(a["agent"]["host"], "m4");
        assert!(a["agent"]["forecast"]["at"].as_u64().unwrap() >= now);
        assert_eq!(a["agent"]["helpers"].as_array().unwrap().len(), 1);
        assert!(a["agent"]["helpers"][0].get("regions").is_none());
        // (The history's parts are the coordinator's, from its memory: none here.)
        assert!(a.get("seq").is_none() && a.get("rates").is_none() && a.get("epoch").is_none());
        // (Each member with a broker, where its `/api/live` is read; one without, missing.)
        assert_eq!(
            a["macs"],
            serde_json::json!([{ "host": "m1", "urls": ["http://100.1.1.1:8090"], "https": "https://m1.t.ts.net/" }])
        );
    }

    #[test]
    fn the_forecast_has_this_macs_own_lanes_as_they_are_now() {
        let now = now_s();
        // This Mac's own lane as its slot is now (as the lead works its own out): its machine's
        // lane's work moves (and what the others take with it).
        let rec = record(now);
        let theirs = local_forecast(crate::agent::seam::for_tests(), &rec, now, &[]);
        let job = SlotJob {
            step: "depth".into(),
            targets: vec![("6/9/9".into(), false), ("6/9/8".into(), true)],
            alone_s: None,
            elapsed_s: 100.0,
            eta: None,
        };
        assert_eq!(job.busy_s(&|_, _| 3000.0, 1.0), 3000.0, "one left, its time spent within the one done");
        let own = [OwnLane { name: "m1".into(), job: Some(job) }];
        let mine = local_forecast(crate::agent::seam::for_tests(), &rec, now, &own);
        assert_ne!(theirs.lanes.get("m1"), mine.lanes.get("m1"));
        assert_eq!(
            theirs,
            forecast::forecast(crate::agent::seam::for_tests(), &rec.input),
            "nothing moved at the time it was made"
        );
    }

    /// A lead that counts its asks.
    struct Counted(Files, std::sync::atomic::AtomicUsize);

    impl Lead for Counted {
        fn view(&self, a: &ViewAsk) -> anyhow::Result<Value> {
            self.1.fetch_add(1, Ordering::Relaxed);
            self.0.view(a)
        }
    }

    #[test]
    fn nothing_is_asked_while_no_page_asks() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let lead = Arc::new(Counted(Files(d.path().into()), Default::default()));
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(Inputs { lead: LeadAt(Some(lead.clone())), ..inputs("") });
        let asked = || lead.1.load(Ordering::Relaxed);
        assert!(!pv.tick_lead(now));
        assert_eq!(asked(), 0);
        // Asked: once, not again within `LEAD_EVERY`; a minute after the last ask, not at all.
        pv.touch_at(now);
        assert!(pv.tick_lead(now) && !pv.tick_lead(now));
        assert_eq!(asked(), 1);
        pv.state.lock().unwrap().lead_last = None;
        assert!(!pv.tick_lead(now + ASKED_FOR_S));
        assert_eq!(asked(), 1);
        pv.touch_at(now);
        assert!(pv.tick_lead(now));
        assert_eq!(asked(), 2);
    }

    /// A lead's coordinator (`crate::coord::start_for_test` in `d/coord`), its agent's status beside
    /// it, its page's files handed to it; its pageview asking it in process.
    fn lead_and_view(d: &std::path::Path, now: u64) -> (Arc<crate::coord::Coordinator>, PageView) {
        let (c, _) = crate::coord::start_for_test(&d.join("coord"), "m4", "1.2.3");
        let status = serde_json::json!({ "host": "m4", "beat": now, "helpers": [{ "host": "m1", "beat": now }] });
        std::fs::write(d.join("status.json"), status.to_string()).unwrap();
        c.set_page_forecast(&record(now).bytes().unwrap());
        c.set_page_regions(Arc::new(serde_json::json!({ "catalog": 3 })));
        let c = Arc::new(c);
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(Inputs { lead: LeadAt(Some(c.clone())), ..inputs("1.2.3") });
        (c, pv)
    }

    #[test]
    fn a_freshly_started_pageviews_first_answer_has_targets_and_after() {
        // (Task #863) a pageview's first run after a restart: the waits the last process kept read
        // back, so a run waiting on a job that finished since names it (`after`, in the lead's
        // history's numbering: task #1160), not none; and its runs name their targets.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        let (c, first) = lead_and_view(d.path(), now);
        let mut rec = record(now);
        let cost = Cost { secs: 600.0, known: true, peak_mb: 1000 };
        rec.input.chains.push(("depth-lo".into(), "3/0/0".into(), cost));
        c.set_page_forecast(&rec.bytes().unwrap());
        let keep = d.path().join("home/pageview-waits.json");
        let with_keep = |keep: Option<PathBuf>| Inputs { keep, lead: LeadAt(Some(c.clone())), ..inputs("") };
        first.set_inputs(with_keep(Some(keep.clone())));
        first.touch_at(now);
        assert!(first.tick_lead(now));
        settle(&first);
        assert!(keep.exists(), "the run's waits kept");
        // (A depth job finishes; the agent restarts.)
        let done = c.note(Event {
            t: now + 5,
            step: Some("depth".into()),
            targets: vec!["6/1/0".into()],
            ..Event::new("done")
        });
        let after_of = |keep: Option<PathBuf>| {
            let pv = PageView::of(crate::agent::seam::for_tests());
            pv.set_inputs(with_keep(keep));
            pv.touch_at(now + 10);
            pv.ask_lead(&*c, now + 10);
            settle(&pv);
            let a = pv.tocome(None).unwrap();
            let runs = a["runs"].as_array().unwrap();
            assert!(runs.iter().any(|r| !r.as_array().unwrap().is_empty()), "its runs name their targets: {a}");
            assert_eq!(a["epoch"], c.history_answer(0, 0, false)["epoch"], "the lead's history's numbering");
            let named: Vec<Value> =
                a["after"].as_array().unwrap().iter().filter(|v| !v.as_array().unwrap().is_empty()).cloned().collect();
            named
        };
        assert_eq!(
            after_of(Some(keep)),
            [serde_json::json!([done])],
            "depth-lo's run waits on the depth job finished since, by its number in the lead's history"
        );
        assert!(after_of(None).is_empty(), "(without the kept waits: none, as before)");
    }

    /// docs/coordination.md step 4: the page has the lead's view (its status, view, pause, forecast
    /// and regions), asked again and again.
    #[test]
    fn the_page_shows_the_lead_s_view() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        let (c, pv) = lead_and_view(d.path(), now);
        pv.touch_at(now);
        for k in 0..3 {
            c.set_pause(Some(jobrt::control::Pause::new(jobrt::control::Mode::Drain, &format!("m{k}"))), now + k);
            pv.state.lock().unwrap().lead_last = None;
            assert!(pv.tick_lead(now));
            settle(&pv);
            let a = pv.swarm().expect("the lead's view");
            assert_eq!(a["agent"]["host"], "m4");
            assert_eq!(a["pause"]["by"], format!("m{k}"), "each ask the lead's pause as it is");
            assert_eq!(a["agent"]["helpers"].as_array().map(Vec::len), Some(1));
            assert!(a["agent"]["forecast"]["at"].as_u64().is_some(), "its forecast run here: {a}");
            assert!(a["view_age_s"].as_u64().is_some_and(|s| s < 60));
            assert_eq!(pv.regions().unwrap()["catalog"], 3);
        }
        // (The lead's status, the forecast's input and the regions sent only when they changed; the
        // page's status the one kept meanwhile.)
        let (st_at, f, r) = {
            let st = pv.state.lock().unwrap();
            let s = st.snap.as_ref().unwrap();
            assert!(s.status_at.is_some() && s.status["host"] == "m4");
            (s.status_at, st.record.as_ref().map(|r| ms_of(r.0)), st.regions.as_ref().map(|r| ms_of(r.0)))
        };
        let again = c.pool_view(&ViewAsk { status_at: st_at, forecast_at: f, regions_at: r });
        assert!(
            again.get("status").is_none() && again.get("forecast_input").is_none() && again.get("regions").is_none(),
            "{again}"
        );
        assert_eq!(again["status_at"].as_u64(), st_at);
        pv.ask_lead(&Fake(Ok(again)), now);
        assert_eq!(pv.swarm().unwrap()["agent"]["host"], "m4", "the status kept");
        let all = c.pool_view(&ViewAsk::default());
        assert!(all.get("forecast_input").is_some() && all["status"]["host"] == "m4");
    }

    /// A lead as a test sets it: its answer, or none to be had (`Err`).
    struct Fake(Result<Value, &'static str>);

    impl Lead for Fake {
        fn view(&self, _: &ViewAsk) -> anyhow::Result<Value> {
            self.0.clone().map_err(|e| anyhow::anyhow!("{e}"))
        }
    }

    /// `pv` told to ask `lead`.
    fn ask_of(pv: &PageView, lead: Fake) {
        pv.set_inputs(Inputs { lead: LeadAt(Some(Arc::new(lead))), ..inputs("1.2.3") });
    }

    /// docs/coordination.md step 4: a page asking with the lead down gets the last view, with its
    /// age and since when the lead isn't reached; the lead back, its view again.
    #[test]
    fn a_page_asking_with_the_lead_down_gets_the_last_view_with_its_age() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        let (c, _) = lead_and_view(d.path(), now);
        let view = c.pool_view(&ViewAsk::default());
        let pv = PageView::of(crate::agent::seam::for_tests());
        ask_of(&pv, Fake(Ok(view)));
        pv.touch_at(now);
        assert!(pv.tick_lead(now));
        assert!(pv.swarm().unwrap()["view_unreached"].is_null());
        ask_of(&pv, Fake(Err("connection refused")));
        pv.state.lock().unwrap().lead_last = None;
        pv.tick_lead(now + 20);
        let a = pv.swarm().expect("the last view kept");
        assert_eq!(a["agent"]["host"], "m4");
        assert_eq!(a["view_unreached"]["since"], now + 20);
        assert!(a["view_unreached"]["why"].as_str().unwrap().contains("refused"));
        assert!(a["view_age_s"].as_u64().is_some());
        ask_of(&pv, Fake(Ok(c.pool_view(&ViewAsk::default()))));
        pv.state.lock().unwrap().lead_last = None;
        pv.tick_lead(now + 30);
        assert!(pv.swarm().unwrap()["view_unreached"].is_null());
    }
}
