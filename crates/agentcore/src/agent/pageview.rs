//! The build page's data on the lead (docs/pool.md §10, task #409; the page is the lead's alone,
//! docs/roles.md §3.3): `/api/swarm` and `/api/history` from its coordinator's view and the
//! history's file on its own disk.
//!
//! While a page has asked in the last `ASKED_FOR_S`, a thread of the agent (`spawn`) asks the
//! view of the lead's coordinator, in process, every `LEAD_EVERY` (`tick_lead`, `Lead`): its
//! status, its coordinator's view, the pause, the Macs' addresses (each member's broker: where a
//! page reads its `/api/live`), and the forecast's input and the page's regions when their time
//! changed. A view that can't be had leaves the last one, said with its age. Another thread reads the
//! history's files (`tick`, `Inputs::history`: the lead's coordinator's `coord/history.jsonl`, on
//! its own disk; no NAS read, task #1119), every `EVERY` at most: what each gained since the last
//! read (from the offset read up to; from the start again when it's another file now, or shorter),
//! `CHUNK_MAX` of a file a read at most, the week's events kept (`data` dropped: the page doesn't
//! show it), `history::KEEP_MAX` at most. Every read is done before its state's lock is taken, so a
//! slow disk never holds up an answer. It keeps one
//! snapshot: the view, the history merged and numbered here
//! (`epoch` names the numbering), and the forecast run here (`forecast::Record::forecast`) with its
//! time brought to now, this Mac's own lanes as its slots are now (`Inputs::own`, worked out as the
//! lead works out its own: forecast::SlotJob). The forecast runs on a thread of its own (`Runs`,
//! task #544), again when the input changed or `forecast::EVERY` passed: the snapshot carries the
//! last one done, never waiting on it.

use super::forecast::{Forecast, Record, SlotJob, ToCome};
use crate::coord::history::{self, Event, History};
use crate::coord::ViewAsk;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// A page that asked this long ago (s) keeps the reads going; after it, nothing is read.
pub const ASKED_FOR_S: u64 = 60;
/// The history's files are read again this often at most.
pub const EVERY: Duration = Duration::from_secs(12);
/// The lead's view is asked again this often at most (it answers from memory: docs/coordination.md
/// step 4).
pub const LEAD_EVERY: Duration = Duration::from_secs(2);

/// Where the lead's view is asked (crate::coord::ViewAsk): its coordinator, in process (the page is
/// the lead's alone, docs/roles.md §3.3); a test's fake.
pub trait Lead: Send + Sync {
    /// The view's answer to `ask`.
    fn view(&self, ask: &ViewAsk) -> anyhow::Result<Value>;
}

impl Lead for crate::coord::Coordinator {
    fn view(&self, ask: &ViewAsk) -> anyhow::Result<Value> {
        Ok(self.pool_view(ask))
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
/// The most of a history file taken in a read (bytes); the rest is read the next time.
pub const CHUNK_MAX: u64 = 4 << 20;

/// Where the history's files are read: this Mac's disk (`LocalFiles`), or a test's.
pub trait Source {
    /// What it holds from byte `offset` on, and where that is: from the start (0) when it's now
    /// shorter than `offset` (made again); and which file it is (`FileId`).
    fn read_from(&self, path: &str, offset: u64) -> Option<Chunk>;
}

/// A file as told apart from one made again in its place: its inode and its first line (512 bytes
/// at most).
pub type FileId = (u64, Vec<u8>);

/// What `Source::read_from` read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chunk {
    pub from: u64,
    pub bytes: Vec<u8>,
    pub id: FileId,
}

/// A file's inode (0 where there's none to tell).
fn ino(m: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    return std::os::unix::fs::MetadataExt::ino(m);
    #[cfg(not(unix))]
    {
        let _ = m;
        0
    }
}

/// A file's first line, 512 bytes at most.
fn head_of(b: &[u8]) -> Vec<u8> {
    let b = &b[..b.len().min(512)];
    b[..b.iter().position(|&c| c == b'\n').unwrap_or(b.len())].to_vec()
}

/// This Mac's files, by path.
pub struct LocalFiles;

impl Source for LocalFiles {
    fn read_from(&self, path: &str, offset: u64) -> Option<Chunk> {
        use std::io::{Read, Seek};
        let r = (|| -> std::io::Result<Chunk> {
            let mut f = std::fs::File::open(path)?;
            let m = f.metadata()?;
            let from = if m.len() < offset { 0 } else { offset };
            let mut head = vec![0u8; 512];
            let n = f.read(&mut head)?;
            head.truncate(n);
            f.seek(std::io::SeekFrom::Start(from))?;
            let mut b = Vec::new();
            f.take(CHUNK_MAX).read_to_end(&mut b)?;
            Ok(Chunk { from, bytes: b, id: (ino(&m), head_of(&head)) })
        })();
        r.ok()
    }
}

/// What the agent tells it, each loop, from what it holds: the history's files (none: not in the
/// pool), this Mac's app, and its own lanes now (only while a page asks: `PageView::wanted`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    /// The history's files, each with its writer: the lead's coordinator's (`coord/history.jsonl`
    /// in its agent's folder).
    pub history: Vec<(String, String)>,
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

/// The files as last read.
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

/// The forecast run here off the reads (`PageView::forecast_run`): the last one done and the one
/// before it (a page that read the swarm's just before a run ended still gets its tiles to come:
/// `PageView::tocome`); the one running; the input's time and when the last was started; the waits
/// the last one left (its items' dependencies, as the lead's agent keeps them for its coordinator's
/// history: `history::Waits`) and this history's number when they were made.
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
/// `at` is the swarm's forecast's), its runs' `after` in this history's numbering (`epoch`; none
/// before the history's first read). The one before the last kept without its forecast (#836
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
    /// Each history file read up to (bytes: the end of its last whole line), which file it was,
    /// and whether that read took in all it held (not cut at CHUNK_MAX).
    offsets: BTreeMap<String, (u64, FileId, bool)>,
    /// The events taken in (writer, time, number, kind): one read again (its file made again) isn't
    /// taken twice. A week's kept, as the history keeps, and no older than its oldest once over
    /// history::KEEP_MAX.
    taken: std::collections::BTreeSet<(String, u64, u64, String)>,
    /// Each finished job's number here, by its writer's file and its number there: what a lease's
    /// or start's `after` and `/api/tocome`'s are written in (task #797 review).
    numbered: BTreeMap<(String, u64), u64>,
    history: Option<History>,
    /// The history's numbering: made with the first snapshot (a new one each process).
    epoch: String,
    last: Option<Instant>,
    /// History chunks of CHUNK_MAX passed over for holding no line's end.
    skipped: u64,
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

    /// A page asked: the reads go on (or start) for `ASKED_FOR_S`.
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

    /// Whether it serves the page (the agent told it the history's files: in the pool). Not: the
    /// coordinator answers from its memory, as out of the pool.
    pub fn active(&self) -> bool {
        !self.inputs.lock().unwrap().history.is_empty()
    }

    /// What the agent tells it (`Inputs`).
    pub fn set_inputs(&self, i: Inputs) {
        let mut cur = self.inputs.lock().unwrap();
        if *cur != i {
            *cur = i;
        }
    }

    /// The NAS's reads, when a page asked lately and `EVERY` has passed (the first at once): the
    /// history's files; true when it read.
    pub fn tick(&self, src: &dyn Source, now: u64) -> bool {
        if !self.wanted(now) {
            return false;
        }
        if self.state.lock().unwrap().last.is_some_and(|t| t.elapsed() < EVERY) {
            return false;
        }
        self.refresh(src, now);
        true
    }

    /// The lead's view asked of it (`Lead::view`), when a page asked lately and `LEAD_EVERY` has
    /// passed (the first at once), off the NAS's reads (its own thread: a hung NAS never holds it
    /// up); true when it asked.
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

    /// Reads the history's files (without the state's lock: what it needs of it taken first) and
    /// takes them in (under it, briefly).
    fn refresh(&self, src: &dyn Source, now: u64) {
        let inp = self.inputs.lock().unwrap().clone();
        let offsets = self.state.lock().unwrap().offsets.clone();
        // The history: what each of its files gained.
        let mut new: Vec<(u64, String, u64, Event)> = Vec::new();
        let mut read_to: BTreeMap<String, (u64, FileId, bool)> = BTreeMap::new();
        let week = now.saturating_sub(history::KEEP_S);
        let mut skipped = 0;
        for (m, p) in &inp.history {
            let p = p.clone();
            let kept = offsets.get(&p);
            let Some(mut c) = src.read_from(&p, kept.map_or(0, |k| k.0)) else {
                if let Some(k) = kept {
                    read_to.insert(p, k.clone());
                }
                continue;
            };
            // (Another file in its place, written again whole: read from its start, what was
            // taken in already passed over.)
            if c.from > 0 && kept.is_some_and(|k| k.1 != c.id) {
                match src.read_from(&p, 0) {
                    Some(again) => c = again,
                    None => continue,
                }
            }
            let (from, b) = (c.from, &c.bytes);
            let all = (b.len() as u64) < CHUNK_MAX;
            // (Whole lines alone: one being written is read next time.)
            let Some(end) = b.iter().rposition(|&c| c == b'\n') else {
                // (A whole chunk without a line's end: a line longer than CHUNK_MAX, never
                // whole; passed over, counted, so the file isn't stuck on it.)
                let to = if all { from } else { from + b.len() as u64 };
                skipped += u64::from(!all);
                read_to.insert(p, (to, c.id, all));
                continue;
            };
            for l in b[..end].split(|&c| c == b'\n') {
                if let Ok(mut e) = serde_json::from_slice::<Event>(l) {
                    if e.t >= week {
                        e.data = None;
                        new.push((e.t, m.clone(), e.seq, e));
                    }
                }
            }
            read_to.insert(p, (from + end as u64 + 1, c.id, all));
        }
        new.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
        // (The newest KEEP_MAX alone: the most the history keeps.)
        new.drain(..new.len().saturating_sub(history::KEEP_MAX));
        // Swapped in.
        let ran = {
            let mut st = self.state.lock().unwrap();
            // (Files no longer read: their offsets forgotten.)
            st.offsets = read_to;
            if skipped > 0 {
                st.skipped += skipped;
                eprintln!("pageview: {} history chunks passed over without a line's end", st.skipped);
            }
            if st.history.is_none() {
                st.epoch = format!("view {} {}", std::process::id(), now);
            }
            st.taken.retain(|k| k.1 >= week);
            let fresh: Vec<(String, u64, Event)> = new
                .into_iter()
                .filter(|(t, m, seq, e)| st.taken.insert((m.clone(), *t, *seq, e.kind.clone())))
                .map(|n| (n.1, n.2, n.3))
                .collect();
            let st = &mut *st;
            let h = st.history.get_or_insert_with(|| History::load(self.the_plan, None));
            for (m, seq, mut e) in fresh {
                // (What it waited on in this numbering: a job not taken in here, dropped.)
                e.after = e.after.iter().filter_map(|q| st.numbered.get(&(m.clone(), *q)).copied()).collect();
                let done = history::finished(&e);
                // (Numbered here: the page's cursor, rising as events arrive.)
                let n = h.add_at(e, now);
                if done {
                    st.numbered.insert((m, seq), n);
                }
            }
            if st.numbered.len() > history::KEEP_MAX {
                let from = h.seq().saturating_sub(history::KEEP_MAX as u64);
                st.numbered.retain(|_, n| *n > from);
            }
            // (What's taken in, as many as the history keeps: those it let go of forgotten.)
            if st.taken.len() > history::KEEP_MAX {
                let oldest = st.history.as_ref().and_then(History::oldest).unwrap_or(week);
                st.taken.retain(|k| k.1 >= oldest);
            }
            st.last = Some(Instant::now());
            st.record.clone()
        };
        // (The forecast run once the history is taken in: its runs' `after` in its numbering.)
        self.forecast_run(ran.as_ref(), now, &inp.own);
    }

    /// `/api/swarm`'s answer (the page's shape: crate::coord's handler), from the snapshot; None
    /// before the first read. A page asked.
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
        let mut a = answer(s, forecast.as_ref(), st.history.as_ref(), &st.epoch, &app, now_s());
        if let Some((since, why)) = &st.lead_failed {
            a["view_unreached"] = serde_json::json!({ "since": since, "why": why });
        }
        Some(a)
    }

    /// `/api/tocome`'s answer (task #836) from the forecast run here: the one at `at` (the swarm's
    /// forecast's `at` the page read: the last or the one before), else the last; with the `epoch`
    /// its `after` is numbered in (null before the history's first read: no `after`). None before
    /// the first run. A page asked.
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

    /// `seqs` (lists of the history's numbers as `member` wrote them: `/api/tocome`'s `after`) in
    /// this Mac's numbering, those not taken in here dropped; with the numbering's `epoch`. None
    /// before the first read.
    pub fn renumber(&self, member: &str, seqs: &[Vec<u64>]) -> Option<(String, Vec<Vec<u64>>)> {
        let st = self.state.lock().unwrap();
        st.history.as_ref()?;
        let of = |q: &u64| st.numbered.get(&(member.to_string(), *q)).copied();
        Some((st.epoch.clone(), seqs.iter().map(|v| v.iter().filter_map(of).collect()).collect()))
    }

    /// `/api/history`'s answer: the events after number `since` (this Mac's numbering, `epoch`),
    /// at most `max`. None before the first read. A page asked.
    pub fn history(&self, since: u64, max: usize) -> Option<Value> {
        self.touch();
        let st = self.state.lock().unwrap();
        st.snap.as_ref()?;
        let h = st.history.as_ref()?;
        Some(serde_json::json!({ "seq": h.seq(), "epoch": st.epoch, "events": h.since(since, max) }))
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
fn answer(s: &Snap, forecast: Option<&Forecast>, h: Option<&History>, epoch: &str, app: &str, now: u64) -> Value {
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
        "seq": h.map_or(0, History::seq),
        "epoch": epoch,
        "rates": h.map(|h| h.rates(now, 24)),
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
        let keep = self.inputs.lock().unwrap().keep.clone();
        // (The first run after a start: the waits the last process kept, in this numbering.)
        if !r.loaded && r.waits.0.is_empty() {
            r.loaded = true;
            let kept = keep
                .as_ref()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| serde_json::from_slice::<history::KeptWaits>(&b).ok())
                // (Kept over a day ago: the build moved on, nothing read back.)
                .filter(|k| now.saturating_sub(k.made_t) < 86_400);
            let st = self.state.lock().unwrap();
            if let (Some(k), Some(h)) = (kept, st.history.as_ref()) {
                // (Its `from`: this numbering's last event by then.)
                let from = h.since(0, usize::MAX).iter().filter(|e| e.t <= k.made_t).map(|e| e.seq).max().unwrap_or(0);
                let w = history::Waits::from_kept(k, from, |m, q| st.numbered.get(&(m.to_string(), q)).copied());
                r.waits = (Arc::new(w), from);
            }
        }
        // (Kept again once `KEEP_WAITS_S` passed: each number's writer and its number there.)
        let save = keep.filter(|_| r.kept.is_none_or(|t| t.elapsed().as_secs() >= KEEP_WAITS_S));
        // (What its items wait on, worked out from the history as read so far, off its lock.)
        let (basis, names) = {
            let st = self.state.lock().unwrap();
            let names: Option<std::collections::HashMap<u64, (String, u64)>> =
                save.as_ref().map(|_| st.numbered.iter().map(|(k, &n)| (n, k.clone())).collect());
            (st.history.as_ref().map(|h| (h.waits_basis_from(r.waits.0.clone(), r.waits.1), st.epoch.clone())), names)
        };
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
                Some(((b, at), epoch)) => {
                    let w = b.next(graph);
                    tocome.runs.after = w.by_job(&item_job, jobs);
                    if let (Some(p), Some(names)) = (&save, &names) {
                        keep_waits(p, &w.keep(now, |n| names.get(&n).cloned()));
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

/// The threads that read while a page asks, each second: the history's files (`PageView::tick`, as
/// the agent last told them) and the lead's view (`PageView::tick_lead`), apart so a slow read never
/// holds the view up.
pub fn spawn(pv: Arc<PageView>) {
    let lead = pv.clone();
    std::thread::Builder::new()
        .name("pageview".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            if pv.active() {
                pv.tick(&LocalFiles, now_s());
            }
        })
        .ok();
    std::thread::Builder::new()
        .name("pageview-lead".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            lead.tick_lead(now_s());
        })
        .ok();
}

/// A path's file read here, for the tests' sources.
#[cfg(test)]
fn local(root: &store::nasfs::NasRoot, p: &str) -> Option<Vec<u8>> {
    root.join(p).read().ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::forecast::{self, Cadence, Cost, Input, Machine};
    use super::*;
    use std::borrow::Cow;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    /// A folder's files, counted as read.
    pub(crate) struct Dir(pub PathBuf, pub AtomicUsize);

    impl Dir {
        pub(crate) fn at(p: &std::path::Path) -> Dir {
            Dir(p.into(), AtomicUsize::new(0))
        }
    }

    /// A test's history files, a file a day a writer for the last `DAYS` (by now): as the agent
    /// tells the history's files (`Inputs::history`), each with its writer.
    pub(crate) fn files(writers: &[&str]) -> Vec<(String, String)> {
        (0..crate::coord::history::DAYS)
            .filter_map(|k| crate::pool::journal::day(now_s().saturating_sub(k * 86_400)))
            .flat_map(|d| writers.iter().map(move |m| (m.to_string(), format!("history/{d}/{m}.jsonl"))))
            .collect()
    }

    impl Source for Dir {
        fn read_from(&self, p: &str, offset: u64) -> Option<Chunk> {
            self.1.fetch_add(1, Ordering::Relaxed);
            let b = local(&store::nasfs::NasRoot::local_for_test(&self.0), p)?;
            let ino = ino(&std::fs::metadata(self.0.join(p)).ok()?);
            let from = if (b.len() as u64) < offset { 0 } else { offset };
            let to = b.len().min(from as usize + CHUNK_MAX as usize);
            Some(Chunk { from, bytes: b[from as usize..to].to_vec(), id: (ino, head_of(&b)) })
        }
    }

    /// The lead's view as a test's files hold it (term 7's, as `fixture` writes them), in its
    /// coordinator's shape (crate::coord::Coordinator::pool_view).
    pub(crate) struct Files(pub PathBuf);

    impl Lead for Files {
        fn view(&self, _: &ViewAsk) -> anyhow::Result<Value> {
            let raw = |p: &str| std::fs::read(self.0.join(p)).ok();
            let json = |p: &str| raw(p).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            let status = json("state/status.json").unwrap_or(Value::Null);
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
                "view": json("state/coord/term/7/view.json").map_or(Value::Null, |v| v["view"].clone()),
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

    /// A read as the agent's two threads make it: the history's files (`tick`), then the lead's
    /// view (from `root`'s files, `Files`) while a page asks; whether the history was read.
    pub(crate) fn read(pv: &PageView, root: &std::path::Path, src: &dyn Source, now: u64) -> bool {
        let read = pv.tick(src, now);
        if pv.wanted(now) {
            pv.ask_lead(&Files(root.into()), now);
        }
        read
    }

    fn rd(pv: &PageView, src: &Dir, now: u64) -> bool {
        read(pv, &src.0, src, now)
    }

    fn put(root: &store::nasfs::NasRoot, p: &str, b: &[u8]) {
        let p = root.join(p);
        p.parent().unwrap().create_dir_all().unwrap();
        p.write(b).unwrap();
    }

    fn mac(name: &str, busy_s: f64) -> Machine {
        Machine { name: name.into(), speed: 1.0, measured: true, mem_mb: 8000, busy_s, ..Default::default() }
    }

    fn record(now: u64) -> Record<'static> {
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

    /// The lead's files for term 7: its status, its view, its forecast's input, two members' history
    /// of a day, a member's status.
    /// (Also coord's tests': `/api/tocome` from a pageview.)
    pub(crate) fn fixture(root: &store::nasfs::NasRoot, now: u64) {
        let view = serde_json::json!({ "at": now, "view": {
            "pause": null,
            "leases": [{ "id": 1, "worker": "m1", "what": "Roads", "step": "unit", "n": 2, "began": now - 100, "progress": null, "beat": now - 10, "lapses": now + 290 }],
            "workers": [{ "name": "m1", "label": "M1", "kind": "native", "seen": now - 5, "fit": [] }],
            "tasks": { "done": 3 },
            "task_mb": { "tail": 400 } } });
        put(root, "state/coord/term/7/view.json", view.to_string().as_bytes());
        put(root, "state/build/term/7/forecast-input.json", &record(now).bytes().unwrap());
        let day = crate::pool::journal::day(now).unwrap();
        let ev =
            |seq: u64, t: u64, kind: &str| serde_json::to_string(&Event { seq, t, ..Event::new(kind) }).unwrap() + "\n";
        put(
            root,
            &format!("history/{day}/m-a.jsonl"),
            (ev(1, now - 50, "start") + &ev(2, now - 20, "done")).as_bytes(),
        );
        put(root, &format!("history/{day}/m-b.jsonl"), ev(9, now - 30, "lease").as_bytes());
        // (m-a heard by its beat: its heartbeat and status in the lead's; m-b, on an older app, on
        // the NAS.)
        let status = serde_json::json!({
            "host": "m4", "beat": 1, "forecast": null,
            "helpers": [{ "host": "m1", "beat": now, "regions": [1] }],
            "beats": [{ "member": "m-a", "host": "m1", "broker": { "urls": ["http://100.1.1.1:8090"], "https": "https://m1.t.ts.net/" } }],
        });
        put(root, "state/status.json", status.to_string().as_bytes());
        put(root, "state/pool/members/m-b.json", br#"{"member":"m-b","host":"gone"}"#);
    }

    /// The forecast run off the reads (`Runs`), waited for.
    pub(crate) fn settle(pv: &PageView) {
        pv.forecast.lock().unwrap().take(true);
    }

    fn inputs(app: &str) -> Inputs {
        Inputs {
            history: files(&["m-a", "m-b"]),
            app: app.into(),
            own: Vec::new(),
            keep: None,
            lead: LeadAt::default(),
        }
    }

    /// (Task #836) `/api/tocome` is the forecast's the swarm's answer carries (the same `at`, its
    /// `after` in this Mac's numbering): the lead's agent's own had lagged a pageview's by a
    /// forecast, so every page dropped its own Mac's answer. The page asks with the `at` it read;
    /// a run done since leaves that one answered, the newest without one.
    #[test]
    fn the_tiles_to_come_are_the_swarms_forecasts() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        assert!(pv.tocome(None).is_none());
        pv.touch_at(now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        rd(&pv, &src, now);
        settle(&pv);
        let a = pv.swarm().unwrap();
        let at = a["agent"]["forecast"]["at"].as_u64().unwrap();
        let tc = pv.tocome(Some(at)).unwrap();
        assert_eq!(tc["at"].as_u64(), Some(at));
        assert_eq!(tc["epoch"], a["epoch"]);
        assert!(tc["macs"].as_object().is_some_and(|m| !m.is_empty()), "{tc}");
        // (Another run 30 s on: the swarm's is the new one; the one read before still answered.)
        pv.forecast.lock().unwrap().of = None;
        pv.state.lock().unwrap().last = None;
        rd(&pv, &src, now + 30);
        settle(&pv);
        let at2 = pv.swarm().unwrap()["agent"]["forecast"]["at"].as_u64().unwrap();
        assert_eq!(at2, now + 30);
        assert_eq!(pv.tocome(Some(at)).unwrap()["at"].as_u64(), Some(at));
        assert_eq!(pv.tocome(Some(at2)).unwrap()["at"].as_u64(), Some(at2));
        assert_eq!(pv.tocome(None).unwrap()["at"].as_u64(), Some(at2));
    }

    #[test]
    fn the_snapshot_is_made_from_the_leads_files_in_the_pages_shape() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs("1.2.3"));
        pv.touch_at(now);
        assert!(rd(&pv, &Dir(d.path().into(), AtomicUsize::new(0)), now));
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
        assert_eq!(a["seq"], 3);
        assert_eq!(a["rates"]["rows"].as_array().unwrap().len(), 24);
        // (Each member with a broker, where its `/api/live` is read; one without, missing.)
        assert_eq!(
            a["macs"],
            serde_json::json!([{ "host": "m1", "urls": ["http://100.1.1.1:8090"], "https": "https://m1.t.ts.net/" }])
        );
    }

    #[test]
    fn the_history_is_merged_by_time_writer_and_number_its_cursor_rising() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        rd(&pv, &src, now);
        let h = pv.history(0, 100).unwrap();
        let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["start", "lease", "done"], "by time, across the writers");
        let seqs: Vec<u64> = h["events"].as_array().unwrap().iter().map(|e| e["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, [1, 2, 3]);
        // A line being written is left for next time; what's appended after is read from where it
        // stopped, numbered after what the page has (even when it happened earlier).
        let day = crate::pool::journal::day(now).unwrap();
        let p = d.path().join(format!("history/{day}/m-b.jsonl"));
        let mut text = std::fs::read_to_string(&p).unwrap();
        text += &(serde_json::to_string(&Event { seq: 10, t: now - 40, ..Event::new("fail") }).unwrap() + "\n");
        text += r#"{"seq":11,"t":"#;
        std::fs::write(&p, &text).unwrap();
        pv.state.lock().unwrap().last = None;
        rd(&pv, &src, now);
        let h = pv.history(3, 100).unwrap();
        let evs = h["events"].as_array().unwrap();
        assert_eq!((evs.len(), evs[0]["kind"].as_str(), evs[0]["seq"].as_u64()), (1, Some("fail"), Some(4)));
        assert!(pv.history(4, 100).unwrap()["events"].as_array().unwrap().is_empty());
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

    #[test]
    fn nothing_is_read_while_no_page_asks() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        assert!(!rd(&pv, &src, now));
        assert_eq!(src.1.load(Ordering::Relaxed), 0);
        // Asked: read once, not again within `EVERY`; a minute after the last ask, not at all.
        pv.touch_at(now);
        assert!(rd(&pv, &src, now) && !rd(&pv, &src, now));
        let n = src.1.load(Ordering::Relaxed);
        pv.state.lock().unwrap().last = None;
        assert!(!rd(&pv, &src, now + ASKED_FOR_S));
        assert_eq!(src.1.load(Ordering::Relaxed), n);
        // (Asked again: the history's files read again.)
        pv.touch_at(now);
        assert!(rd(&pv, &src, now));
        assert_eq!(src.1.load(Ordering::Relaxed) - n, n);
    }

    #[test]
    fn a_new_numbering_has_a_new_epoch() {
        // (Task #409 review) the page starts its cursor again when the epoch changes: a new
        // process's view (its agent restarted) numbers from 1 again, under another epoch.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let first = PageView::of(crate::agent::seam::for_tests());
        first.set_inputs(inputs(""));
        assert!(first.swarm().is_none() && first.history(0, 10).is_none(), "nothing before the first read");
        rd(&first, &Dir(d.path().into(), AtomicUsize::new(0)), now);
        let a = first.swarm().unwrap();
        let again = PageView::of(crate::agent::seam::for_tests());
        again.set_inputs(inputs(""));
        again.touch_at(now);
        rd(&again, &Dir(d.path().into(), AtomicUsize::new(0)), now + 1);
        let b = again.swarm().unwrap();
        assert!(a["epoch"].as_str().is_some_and(|e| !e.is_empty()));
        assert_ne!(a["epoch"], b["epoch"]);
        assert_eq!(first.history(0, 10).unwrap()["epoch"], a["epoch"]);
        // (Its own numbering stays its own across reads.)
        first.state.lock().unwrap().last = None;
        rd(&first, &Dir(d.path().into(), AtomicUsize::new(0)), now);
        assert_eq!(first.swarm().unwrap()["epoch"], a["epoch"]);
    }

    /// A source whose reads wait until they're let go.
    struct Slow(Dir, Mutex<std::sync::mpsc::Receiver<()>>);

    impl Source for Slow {
        fn read_from(&self, p: &str, offset: u64) -> Option<Chunk> {
            self.1.lock().unwrap().recv().ok();
            self.0.read_from(p, offset)
        }
    }

    #[test]
    fn a_slow_read_never_holds_up_an_answer() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let pv = Arc::new(PageView::of(crate::agent::seam::for_tests()));
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        rd(&pv, &Dir(d.path().into(), AtomicUsize::new(0)), now);
        // A second read, stuck on its first file: the answers go on from the snapshot meanwhile.
        let (go, wait) = std::sync::mpsc::channel::<()>();
        pv.state.lock().unwrap().last = None;
        let reader = {
            let (pv, root) = (pv.clone(), d.path().to_path_buf());
            std::thread::spawn(move || pv.tick(&Slow(Dir(root, AtomicUsize::new(0)), Mutex::new(wait)), now))
        };
        // (sleep ok: a head start for the reader; the answers hold either way)
        std::thread::sleep(Duration::from_millis(100));
        assert!(pv.swarm().is_some() && pv.history(0, 10).is_some());
        drop(go);
        assert!(reader.join().unwrap());
    }

    /// (Task #797 review) What a lease waited on and `/api/tocome`'s `after`, written in the
    /// lead's numbering, are given in this Mac's: the page matches them against the ends it holds.
    #[test]
    fn what_was_waited_on_is_renumbered_here() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        let root = store::nasfs::NasRoot::local_for_test(d.path());
        fixture(&root, now);
        let day = crate::pool::journal::day(now).unwrap();
        let done = Event {
            seq: 40,
            t: now - 15,
            step: Some("unit".into()),
            targets: vec!["6/1/1".into()],
            ..Event::new("done")
        };
        let lease = Event { seq: 41, t: now - 10, after: vec![40, 7], ..Event::new("lease") };
        let lines: String = [&done, &lease].iter().map(|e| serde_json::to_string(e).unwrap() + "\n").collect();
        put(&root, &format!("history/{day}/m-b.jsonl"), lines.as_bytes());
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        rd(&pv, &Dir(d.path().into(), AtomicUsize::new(0)), now);
        let h = pv.history(0, 10).unwrap();
        let of = |k: &str| h["events"].as_array().unwrap().iter().find(|e| e["kind"] == k).unwrap().clone();
        let n = h["events"].as_array().unwrap().iter().find(|e| e["step"] == "unit").unwrap()["seq"].as_u64().unwrap();
        assert_ne!(n, 40);
        assert_eq!(of("lease")["after"], serde_json::json!([n]));
        let (epoch, after) = pv.renumber("m-b", &[vec![40], vec![7], vec![]]).unwrap();
        assert_eq!((epoch, after), (h["epoch"].as_str().unwrap().to_string(), vec![vec![n], vec![], vec![]]));
        // (Another writer's numbers are its own.)
        assert_eq!(pv.renumber("m-a", &[vec![40]]).unwrap().1, vec![Vec::<u64>::new()]);
    }

    #[test]
    fn a_history_file_made_again_shorter_is_read_from_its_start() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        rd(&pv, &src, now);
        let day = crate::pool::journal::day(now).unwrap();
        let line = serde_json::to_string(&Event { seq: 1, t: now - 5, ..Event::new("catalog") }).unwrap() + "\n";
        std::fs::write(d.path().join(format!("history/{day}/m-a.jsonl")), line).unwrap();
        pv.state.lock().unwrap().last = None;
        rd(&pv, &src, now);
        let h = pv.history(3, 10).unwrap();
        assert_eq!(h["events"][0]["kind"], "catalog");
    }

    #[test]
    fn a_history_file_written_again_whole_is_read_again_without_twice_the_events() {
        // (Task #409 review) made again in its place (renamed over it), longer than before: read
        // from its start, the events taken already passed over.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        rd(&pv, &src, now);
        let day = crate::pool::journal::day(now).unwrap();
        let p = d.path().join(format!("history/{day}/m-a.jsonl"));
        let ev =
            |seq: u64, t: u64, kind: &str| serde_json::to_string(&Event { seq, t, ..Event::new(kind) }).unwrap() + "\n";
        // (Its first event dropped as it's rewritten, one new before the old second, one after.)
        let text = ev(5, now - 25, "catalog")
            + &ev(2, now - 20, "done")
            + &ev(6, now - 10, "pause")
            + &ev(7, now - 9, "resume");
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, text).unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        pv.state.lock().unwrap().last = None;
        rd(&pv, &src, now);
        let h = pv.history(3, 10).unwrap();
        let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["catalog", "pause", "resume"], "the old second not again");
    }

    #[test]
    fn the_history_is_the_last_weeks() {
        // (Task #509) the events of the last week (`history::KEEP_S`), the older not; `data`
        // dropped. (Task #901) On a clock of its own, never the wall's, at times of day round a
        // UTC midnight. (Task #1119: one file, the lead's coordinator's, read again from where it
        // was read to.)
        let midnight = 1_790_000_000 / 86_400 * 86_400;
        for now in [midnight - 1, midnight, midnight + 1, midnight + 43_200, midnight + 86_399] {
            let d = tempfile::tempdir().unwrap();
            let root = store::nasfs::NasRoot::local_for_test(d.path());
            fixture(&root, now);
            let today = now / 86_400 * 86_400;
            let mut text = String::new();
            let mut want = Vec::new();
            for k in (1..=crate::coord::history::DAYS).rev() {
                let start = today - k * 86_400;
                for (i, t) in [start + 60, start + 86_340].into_iter().enumerate() {
                    let e = Event {
                        seq: 100 + 2 * k + i as u64,
                        t,
                        note: format!("day {k}"),
                        data: Some(serde_json::json!({ "big": 1 })),
                        ..Event::new("task")
                    };
                    text += &(serde_json::to_string(&e).unwrap() + "\n");
                    if t >= now - history::KEEP_S {
                        want.push(format!("day {k}"));
                    }
                }
            }
            put(&root, "history/m-a.jsonl", text.as_bytes());
            let src = Dir(d.path().into(), AtomicUsize::new(0));
            let pv = PageView::of(crate::agent::seam::for_tests());
            pv.set_inputs(Inputs { history: vec![("m-a".into(), "history/m-a.jsonl".into())], ..inputs("") });
            pv.touch_at(now);
            rd(&pv, &src, now);
            let h = pv.history(0, 1000).unwrap();
            let evs = h["events"].as_array().unwrap();
            let notes: Vec<&str> = evs.iter().filter_map(|e| e["note"].as_str()).collect();
            assert_eq!(notes, want, "at {now}: the week's alone");
            assert!(evs.iter().all(|e| e.get("data").is_none()));
            // (A second read: the file once, from where it was read to.)
            let first = src.1.load(Ordering::Relaxed);
            pv.state.lock().unwrap().last = None;
            rd(&pv, &src, now);
            assert_eq!(src.1.load(Ordering::Relaxed) - first, 1, "at {now}: the history's file alone");
        }
    }

    #[test]
    fn the_history_read_is_bounded_by_the_history_keep() {
        // (Task #509) more events across days than history::KEEP_MAX: the newest kept alone.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        let per = (history::KEEP_MAX / 2 + 1000) as u64;
        for k in 0..3u64 {
            let base = now - k * 86_400 - 3600;
            let day = crate::pool::journal::day(base).unwrap();
            let text: String = (0..per)
                .map(|i| {
                    serde_json::to_string(&Event { seq: i + 1, t: base - per + i, ..Event::new("lease") }).unwrap()
                        + "\n"
                })
                .collect();
            put(
                &store::nasfs::NasRoot::local_for_test(d.path()),
                &format!("history/{day}/m-b.jsonl"),
                text.as_bytes(),
            );
        }
        put(&store::nasfs::NasRoot::local_for_test(d.path()), "state/status.json", b"{}");
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        rd(&pv, &src, now);
        let h = pv.history(0, usize::MAX).unwrap();
        let evs = h["events"].as_array().unwrap();
        assert_eq!(evs.len(), history::KEEP_MAX);
        assert_eq!(evs.last().unwrap()["t"].as_u64(), Some(now - 3600 - 1));
        assert!(pv.state.lock().unwrap().taken.len() <= history::KEEP_MAX);
        // (Read again with nothing new: none added.)
        pv.state.lock().unwrap().last = None;
        rd(&pv, &src, now);
        assert_eq!(pv.history(0, 1).unwrap()["seq"].as_u64(), Some(history::KEEP_MAX as u64));
    }

    #[test]
    fn a_line_longer_than_a_chunk_is_passed_over() {
        // (Task #509 review) a chunk with no line's end: skipped, counted, the lines after it read.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let day = crate::pool::journal::day(now).unwrap();
        let p = d.path().join(format!("history/{day}/m-b.jsonl"));
        let mut text = "x".repeat(CHUNK_MAX as usize + 10) + "\n";
        text += &(serde_json::to_string(&Event { seq: 1, t: now - 5, ..Event::new("pause") }).unwrap() + "\n");
        std::fs::write(&p, text).unwrap();
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::of(crate::agent::seam::for_tests());
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        for _ in 0..3 {
            pv.state.lock().unwrap().last = None;
            rd(&pv, &src, now);
        }
        assert_eq!(pv.state.lock().unwrap().skipped, 1);
        let h = pv.history(0, 10).unwrap();
        let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["start", "done", "pause"]);
    }

    #[test]
    fn a_freshly_started_pageviews_first_answer_has_targets_and_after() {
        // (Task #863) a pageview's first run after a restart: the waits the last process kept read
        // back, so a run waiting on a job that finished since names it (`after`), not none; and
        // its runs name their targets.
        let d = tempfile::tempdir().unwrap();
        let root = store::nasfs::NasRoot::local_for_test(d.path());
        let now = now_s();
        fixture(&root, now);
        let mut rec = record(now);
        let c = Cost { secs: 600.0, known: true, peak_mb: 1000 };
        rec.input.chains.push(("depth-lo".into(), "3/0/0".into(), c));
        put(&root, "state/build/term/7/forecast-input.json", &rec.bytes().unwrap());
        let keep = d.path().join("home/pageview-waits.json");
        let with_keep = |keep: Option<PathBuf>| Inputs { keep, ..inputs("") };
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let first = PageView::of(crate::agent::seam::for_tests());
        first.set_inputs(with_keep(Some(keep.clone())));
        first.touch_at(now);
        rd(&first, &src, now);
        settle(&first);
        assert!(keep.exists(), "the run's waits kept");
        // (A depth job finishes; the agent restarts.)
        let day = crate::pool::journal::day(now).unwrap();
        let ev = |seq: u64, t: u64, kind: &str, step: Option<&str>| {
            let targets = step.map_or(Vec::new(), |_| vec!["6/1/0".to_string()]);
            serde_json::to_string(&Event { seq, t, step: step.map(str::to_string), targets, ..Event::new(kind) })
                .unwrap()
                + "\n"
        };
        let lines =
            ev(1, now - 50, "start", None) + &ev(2, now - 20, "done", None) + &ev(3, now + 5, "done", Some("depth"));
        put(&root, &format!("history/{day}/m-a.jsonl"), lines.as_bytes());
        let after_of = |keep: Option<PathBuf>| {
            let pv = PageView::of(crate::agent::seam::for_tests());
            pv.set_inputs(with_keep(keep));
            pv.touch_at(now + 10);
            rd(&pv, &src, now + 10);
            settle(&pv);
            let a = pv.tocome(None).unwrap();
            let runs = a["runs"].as_array().unwrap();
            assert!(runs.iter().any(|r| !r.as_array().unwrap().is_empty()), "its runs name their targets: {a}");
            assert!(a["epoch"].is_string());
            a["after"].as_array().unwrap().iter().filter(|v| !v.as_array().unwrap().is_empty()).count()
        };
        assert!(after_of(Some(keep)) > 0, "depth-lo's run waits on the depth job finished since");
        assert_eq!(after_of(None), 0, "(without the kept waits: none, as before)");
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
        pv.set_inputs(Inputs {
            lead: LeadAt(Some(c.clone())),
            ..inputs("1.2.3")
        });
        (c, pv)
    }

    /// docs/coordination.md step 4: the page has the lead's view (its status, view, pause, forecast
    /// and regions) while the history's read waits: it waits on its own thread, the view asked
    /// again and again meanwhile.
    #[test]
    fn the_page_shows_the_lead_s_view_while_the_history_waits() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        let (c, pv) = lead_and_view(d.path(), now);
        let pv = Arc::new(pv);
        pv.touch_at(now);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let nas = {
            let (pv, root) = (pv.clone(), d.path().to_path_buf());
            std::thread::spawn(move || pv.tick(&Slow(Dir::at(&root), Mutex::new(wait)), now))
        };
        for k in 0..3 {
            c.set_pause(Some(jobrt::control::Pause::new(jobrt::control::Mode::Drain, &format!("m{k}"))), now + k);
            pv.state.lock().unwrap().lead_last = None;
            assert!(pv.tick_lead(now));
            settle(&pv);
            let a = pv.swarm().expect("the lead's view, the NAS hung");
            assert_eq!(a["agent"]["host"], "m4");
            assert_eq!(a["pause"]["by"], format!("m{k}"), "each ask the lead's pause as it is");
            assert_eq!(a["agent"]["helpers"].as_array().map(Vec::len), Some(1));
            assert!(a["agent"]["forecast"]["at"].as_u64().is_some(), "its forecast run here: {a}");
            assert!(a["view_age_s"].as_u64().is_some_and(|s| s < 60));
            assert_eq!(pv.regions().unwrap()["catalog"], 3);
        }
        assert!(!nas.is_finished(), "the history's read still waits");
        drop(go);
        nas.join().unwrap();
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
