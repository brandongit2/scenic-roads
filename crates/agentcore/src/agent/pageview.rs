//! The build page's data on the lead (docs/pool.md §10, task #409; the page is the lead's alone,
//! docs/roles.md §3.3): `/api/swarm` from its coordinator's view. The history isn't here:
//! `/api/history` and the swarm's `seq`, `epoch` and `rates` are the coordinator's, from its memory
//! (crate::coord::history, task #1160); so are the forecast and the tiles to come (`/api/tocome`):
//! the lead's agent's own, as its status has them, handed to its coordinator as each is taken in
//! (crate::coord::PageFiles, task #1487), never run again for the page.
//!
//! While a page has asked in the last `ASKED_FOR_S`, a thread of the agent (`spawn`) asks the
//! view of the lead's coordinator, in process, every `LEAD_EVERY` (`tick_lead`, `Lead`): its
//! status, its coordinator's view, the pause, the Macs' addresses (each member's broker: where a
//! page reads its `/api/live`); and the page's regions as its coordinator holds them, shared
//! (`Lead::regions`). A view that can't be had leaves the last one, said with its age. It keeps one
//! snapshot, which the swarm's answer is made from with the forecast the coordinator holds.

use super::forecast::Forecast;
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

    /// The page's regions its agent handed it (crate::coord::PageFiles), shared, with when (unix
    /// ms); none (a test's lead).
    fn regions(&self) -> Option<(u64, Arc<Value>)> {
        None
    }
}

impl Lead for crate::coord::Coordinator {
    fn view(&self, ask: &ViewAsk) -> anyhow::Result<Value> {
        Ok(self.pool_view(ask))
    }

    fn regions(&self) -> Option<(u64, Arc<Value>)> {
        self.page().regions
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

/// A member's status older than this (s) isn't shown (as the lead's own `members` takes them).
const MEMBER_FRESH_S: u64 = 600;

/// What the agent tells it, each loop, from what it holds: this Mac's app and where the lead's view
/// is asked (none: not in the pool).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub app: String,
    /// Where the lead's view is asked: the lead's coordinator here.
    pub lead: LeadAt,
}

/// The lead's view as last asked.
#[derive(Clone, Debug, Default)]
struct Snap {
    status: Value,
    /// The lead's status's change token as answered (`ViewAsk::status_at`): sent again only once it changed.
    status_at: Option<u64>,
    view: Value,
    pause: Value,
    members: Vec<Value>,
    /// Each member's addresses, from its heartbeat's broker: where a page reads its `/api/live`.
    macs: Vec<Value>,
    /// When the lead made it (unix s).
    at: Option<u64>,
}

#[derive(Default)]
struct State {
    snap: Option<Snap>,
    /// The lead's regions for the page (crate::agent::regionsview) as last read, by when its agent
    /// handed them.
    regions: Option<(SystemTime, Arc<Value>)>,
    /// When the lead was last asked.
    lead_last: Option<Instant>,
    /// Since when (unix s) and why the lead can't be asked: the last view kept meanwhile.
    lead_failed: Option<(u64, String)>,
}

/// The page's data on this Mac.
pub struct PageView {
    asked: AtomicU64,
    inputs: Mutex<Inputs>,
    state: Mutex<State>,
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
    pub fn new() -> Arc<PageView> {
        Arc::new(PageView::of())
    }

    fn of() -> PageView {
        PageView { asked: Default::default(), inputs: Default::default(), state: Default::default() }
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
    /// view, pause, Macs; and the regions as the lead holds them, shared (`Lead::regions`). One that
    /// can't be asked: the last view kept, said with its age (`lead_failed`).
    fn ask_lead(&self, lead: &dyn Lead, now: u64) {
        let ask = {
            let st = self.state.lock().unwrap();
            ViewAsk { status_at: st.snap.as_ref().and_then(|s| s.status_at) }
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
        // (The regions as the lead holds them: shared, never copied.)
        let at = |ms: u64| SystemTime::UNIX_EPOCH + Duration::from_millis(ms);
        st.regions = lead.regions().map(|(ms, r)| (at(ms), r));
        // (Its status only when it changed since the last answer: the one kept otherwise.)
        let old = st.snap.take();
        let status = match v.get("status") {
            Some(s) => s.clone(),
            None => old.map_or(Value::Null, |o| o.status),
        };
        let members = members_of(&status, now);
        let macs = v["macs"].as_array().cloned().unwrap_or_default();
        st.snap = Some(Snap {
            status,
            status_at: v["status_at"].as_u64(),
            view: v["view"].clone(),
            pause: v["pause"].clone(),
            members,
            macs,
            at: v["at"].as_u64(),
        });
    }

    /// `/api/swarm`'s answer (the page's shape: crate::coord's handler, which adds the history's
    /// `seq`, `epoch` and `rates` from its memory), from the snapshot with the lead's agent's
    /// forecast as its coordinator holds it (`forecast`: crate::coord::PageFiles); None before the
    /// lead's first view. A page asked.
    pub fn swarm(&self, forecast: Option<&Forecast>) -> Option<Value> {
        self.touch();
        let app = self.inputs.lock().unwrap().app.clone();
        let st = self.state.lock().unwrap();
        let s = st.snap.as_ref()?;
        let mut a = answer(s, forecast, &app, now_s());
        if let Some((since, why)) = &st.lead_failed {
            a["view_unreached"] = serde_json::json!({ "since": since, "why": why });
        }
        Some(a)
    }

    /// `/api/regions`' answer (crate::agent::regionsview): the lead's as last read; None before
    /// they're read (or while the lead hasn't written them). A page asked.
    pub fn regions(&self) -> Option<Arc<Value>> {
        self.touch();
        self.state.lock().unwrap().regions.as_ref().map(|r| r.1.clone())
    }
}

/// The members' statuses fresh (`MEMBER_FRESH_S`) from the lead's status's `members` (sent as
/// "helpers", read as "members" too: crate::agent::Status::members), trimmed of what the page
/// doesn't show.
fn members_of(status: &Value, now: u64) -> Vec<Value> {
    status
        .get("members")
        .unwrap_or(&status["helpers"])
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| now.saturating_sub(h["beat"].as_u64().unwrap_or(0)) < MEMBER_FRESH_S)
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

/// The swarm's answer at `now` from snapshot `s` and the lead's agent's forecast (its status's
/// own: none once its plan's shape changed, a stop's with its reason and no finish).
fn answer(s: &Snap, forecast: Option<&Forecast>, app: &str, now: u64) -> Value {
    let mut agent = s.status.clone();
    if agent.is_object() {
        agent["forecast"] = forecast.map_or(Value::Null, |f| serde_json::to_value(f).unwrap_or(Value::Null));
        agent["helpers"] = Value::Array(s.members.clone());
    }
    // (A lead on an app before view.json: no leases, workers nor tasks.)
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
    /// A time as unix ms.
    fn ms_of(t: SystemTime) -> u64 {
        t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
    }

    use super::*;
    use std::path::PathBuf;

    /// The lead's view as a test's files hold it (term 7's, as `fixture` writes them), in its
    /// coordinator's shape (crate::coord::Coordinator::pool_view).
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
            Ok(serde_json::json!({
                "status": status,
                "view": json("lead-view.json").map_or(Value::Null, |v| v["view"].clone()),
                "pause": json("state/build/pause.json").unwrap_or(Value::Null),
                "macs": macs,
                "at": now_s(),
            }))
        }

        fn regions(&self) -> Option<(u64, Arc<Value>)> {
            let p = self.0.join("state/build/term/7/regions.json");
            let at = std::fs::metadata(&p).and_then(|m| m.modified()).ok().map(ms_of);
            let v = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
            at.zip(v).map(|(t, r)| (t, Arc::new(r)))
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

    /// The lead's files for term 7: its status, its view, a member's status.
    fn fixture(root: &store::nasfs::NasRoot, now: u64) {
        let view = serde_json::json!({ "at": now, "view": {
            "pause": null,
            "leases": [{ "id": 1, "worker": "m1", "what": "Roads", "step": "unit", "n": 2, "began": now - 100, "progress": null, "beat": now - 10, "lapses": now + 290 }],
            "workers": [{ "name": "m1", "label": "M1", "kind": "native", "seen": now - 5, "fit": [] }],
            "tasks": { "done": 3 },
            "task_mb": { "tail": 400 } } });
        put(root, "lead-view.json", view.to_string().as_bytes());
        // (m-a heard by its beat: its heartbeat and status in the lead's; m-b, on an older app, on
        // the NAS.)
        let status = serde_json::json!({
            "host": "m4", "beat": 1, "forecast": { "at": 1 },
            "helpers": [{ "host": "m1", "beat": now, "regions": [1] }],
            "beats": [{ "member": "m-a", "host": "m1", "broker": { "urls": ["http://100.1.1.1:8090"], "https": "https://m1.t.ts.net/" } }],
        });
        put(root, "lead-status.json", status.to_string().as_bytes());
        put(root, "state/pool/members/m-b.json", br#"{"member":"m-b","host":"gone"}"#);
    }

    fn inputs(app: &str) -> Inputs {
        Inputs { app: app.into(), lead: LeadAt::default() }
    }

    #[test]
    fn the_snapshot_is_made_from_the_leads_view_in_the_pages_shape() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(&store::nasfs::NasRoot::local_for_test(d.path()), now);
        let pv = PageView::of();
        pv.set_inputs(inputs("1.2.3"));
        assert!(pv.swarm(None).is_none(), "nothing before the lead's first view");
        pv.touch_at(now);
        assert!(read(&pv, d.path(), now));
        let f = Forecast { at: now - 30, why: Some("a stop's reason".into()), ..Default::default() };
        let a = pv.swarm(Some(&f)).unwrap();
        assert_eq!(a["app"], "1.2.3");
        assert_eq!(a["tasks"]["done"], 3);
        assert_eq!(a["task_mb"]["tail"], 400);
        let l = &a["leases"][0];
        assert!(l["for_s"].as_u64().unwrap() >= 100 && l.get("began").is_none());
        assert!(l["beat_s"].as_u64().unwrap() >= 10 && l["lapses_in_s"].as_u64().unwrap() <= 290);
        assert!(a["workers"][0]["seen_s"].as_u64().unwrap() >= 5);
        // (The lead's status with its agent's forecast as its coordinator holds it, as it was
        // made: a stop's with its reason and no finish; the members' statuses fresh, trimmed.)
        assert_eq!(a["agent"]["host"], "m4");
        assert_eq!(a["agent"]["forecast"]["at"], now - 30);
        assert_eq!(a["agent"]["forecast"]["why"], "a stop's reason");
        assert!(a["agent"]["forecast"]["done_at"].is_null());
        assert_eq!(a["agent"]["helpers"].as_array().unwrap().len(), 1);
        assert!(a["agent"]["helpers"][0].get("regions").is_none());
        // (None held, none shown: never the status file's, which may be a beat behind.)
        assert!(pv.swarm(None).unwrap()["agent"]["forecast"].is_null());
        // (The history's parts are the coordinator's, from its memory: none here.)
        assert!(a.get("seq").is_none() && a.get("rates").is_none() && a.get("epoch").is_none());
        // (Each member with a broker, where its `/api/live` is read; one without, missing.)
        assert_eq!(
            a["macs"],
            serde_json::json!([{ "host": "m1", "urls": ["http://100.1.1.1:8090"], "https": "https://m1.t.ts.net/" }])
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
        let pv = PageView::of();
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

    /// The lead's members read from its status under either name (task #858: sent as "helpers"
    /// this release), the stale left out; the agent's own Status alike.
    #[test]
    fn the_members_are_read_as_helpers_or_members() {
        let now = 10_000;
        let m = |host: &str, beat| crate::agent::Status { host: host.into(), beat, ..Default::default() };
        let lead = crate::agent::Status {
            host: "m4".into(),
            beat: now,
            members: vec![m("m1", now), m("m6", 1)],
            ..Default::default()
        };
        let mut sent = serde_json::to_value(&lead).unwrap();
        let members = sent.as_object_mut().unwrap().remove("helpers").expect("sent as before");
        for key in ["helpers", "members"] {
            let mut status = sent.clone();
            status[key] = members.clone();
            let hosts: Vec<Value> = members_of(&status, now).iter().map(|m| m["host"].clone()).collect();
            assert_eq!(hosts, [serde_json::json!("m1")], "{key}");
            let st: crate::agent::Status = serde_json::from_value(status).unwrap();
            assert_eq!(st.members.len(), 2, "{key}");
        }
    }

    /// A lead's coordinator (`crate::coord::start_for_test` in `d/coord`), its agent's status beside
    /// it, its page's regions handed to it; its pageview asking it in process.
    fn lead_and_view(d: &std::path::Path, now: u64) -> (Arc<crate::coord::Coordinator>, PageView) {
        let (c, _) = crate::coord::start_for_test(&d.join("coord"), "m4", "1.2.3");
        let status = serde_json::json!({ "host": "m4", "beat": now, "helpers": [{ "host": "m1", "beat": now }] });
        std::fs::write(d.join("status.json"), status.to_string()).unwrap();
        c.set_page_regions(Arc::new(serde_json::json!({ "catalog": 3 })));
        let c = Arc::new(c);
        let pv = PageView::of();
        pv.set_inputs(Inputs { lead: LeadAt(Some(c.clone())), ..inputs("1.2.3") });
        (c, pv)
    }

    /// docs/coordination.md step 4: the page has the lead's view (its status, view, pause and
    /// regions), asked again and again.
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
            let a = pv.swarm(None).expect("the lead's view");
            assert_eq!(a["agent"]["host"], "m4");
            assert_eq!(a["pause"]["by"], format!("m{k}"), "each ask the lead's pause as it is");
            assert_eq!(a["agent"]["helpers"].as_array().map(Vec::len), Some(1));
            assert!(a["view_age_s"].as_u64().is_some_and(|s| s < 60));
            assert_eq!(pv.regions().unwrap()["catalog"], 3);
        }
        // (The lead's status sent only when it changed, the page's the one kept meanwhile; the
        // regions never in the view, the lead's own shared: task #1335.)
        let st_at = {
            let st = pv.state.lock().unwrap();
            let s = st.snap.as_ref().unwrap();
            assert!(s.status_at.is_some() && s.status["host"] == "m4");
            assert!(Arc::ptr_eq(&st.regions.as_ref().unwrap().1, &c.page().regions.unwrap().1), "its regions, shared");
            s.status_at
        };
        let again = c.pool_view(&ViewAsk { status_at: st_at });
        assert!(again.get("status").is_none(), "{again}");
        assert_eq!(again["status_at"].as_u64(), st_at);
        pv.ask_lead(&Fake(Ok(again)), now);
        assert_eq!(pv.swarm(None).unwrap()["agent"]["host"], "m4", "the status kept");
        let all = c.pool_view(&ViewAsk::default());
        assert!(all["status"]["host"] == "m4" && all.get("forecast").is_none() && all.get("regions").is_none());
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
        let pv = PageView::of();
        ask_of(&pv, Fake(Ok(view)));
        pv.touch_at(now);
        assert!(pv.tick_lead(now));
        assert!(pv.swarm(None).unwrap()["view_unreached"].is_null());
        ask_of(&pv, Fake(Err("connection refused")));
        pv.state.lock().unwrap().lead_last = None;
        pv.tick_lead(now + 20);
        let a = pv.swarm(None).expect("the last view kept");
        assert_eq!(a["agent"]["host"], "m4");
        assert_eq!(a["view_unreached"]["since"], now + 20);
        assert!(a["view_unreached"]["why"].as_str().unwrap().contains("refused"));
        assert!(a["view_age_s"].as_u64().is_some());
        ask_of(&pv, Fake(Ok(c.pool_view(&ViewAsk::default()))));
        pv.state.lock().unwrap().lead_last = None;
        pv.tick_lead(now + 30);
        assert!(pv.swarm(None).unwrap()["view_unreached"].is_null());
    }
}
