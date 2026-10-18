//! The build page's data as this Mac reads it from the NAS (docs/pool.md §10, task #409): every
//! Mac's port 8090 answers `/work/swarm` and `/work/history` the same way, from a few whole files
//! the lead writes, read by known name (never a folder listing) while a page asks.
//!
//! A thread of the agent (`spawn`) reads them only while a page has asked in the last
//! `ASKED_FOR_S`, every `EVERY` at most: the lead's status (`state/status.json`), its
//! coordinator's view (`state/coord/term/<E>/view.json`), its forecast's input
//! (`state/build/term/<E>/forecast-input.json`, read again only when its time changes), the
//! pause (`state/build/pause.json`), each member's own status (`state/helpers/<host>.json`, the
//! members as the pool's driver knows them), and what the members' history files of the two
//! newest days (`state/coord/history/<day>/<member>.jsonl`) gained since the last read (from the
//! offset read up to). It keeps one snapshot: those, the history merged, and the forecast run
//! here (`forecast::Record::forecast`) with its time brought to now, this Mac's own lanes as its
//! slots are now (`Inputs::own`).

use super::forecast::{Forecast, Record};
use crate::coord::history::{Event, History};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// A page that asked this long ago (s) keeps the reads going; after it, nothing is read.
pub const ASKED_FOR_S: u64 = 60;
/// The files are read again this often at most.
pub const EVERY: Duration = Duration::from_secs(12);
/// A member's status older than this (s) isn't shown (as the lead's own `helpers` takes them).
const HELPER_FRESH_S: u64 = 600;

/// Where the files are read: the NAS (`NasFiles`), or a test's.
pub trait Source {
    /// The file at `path` (relative to the project's root) whole; None when it can't be read.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Its time last changed.
    fn mtime(&self, path: &str) -> Option<SystemTime>;
    /// What it holds from byte `offset` on.
    fn read_from(&self, path: &str, offset: u64) -> Option<Vec<u8>>;
}

/// The NAS's files, each read on the records' readers under their deadline (crate::agent::nasread).
pub struct NasFiles(pub PathBuf);

impl Source for NasFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        super::nasread::read(&self.0.join(path)).ok()
    }
    fn mtime(&self, path: &str) -> Option<SystemTime> {
        let p = self.0.join(path);
        super::nasread::with(&p.clone(), move || std::fs::metadata(&p)?.modified()).ok()
    }
    fn read_from(&self, path: &str, offset: u64) -> Option<Vec<u8>> {
        let p = self.0.join(path);
        super::nasread::with(&p.clone(), move || {
            use std::io::{Read, Seek};
            let mut f = std::fs::File::open(&p)?;
            f.seek(std::io::SeekFrom::Start(offset))?;
            let mut b = Vec::new();
            f.read_to_end(&mut b)?;
            Ok(b)
        })
        .ok()
    }
}

/// What the agent tells it, each loop: the project's root, the current term, the members (by id,
/// as the pool's driver knows them, and their hosts), this Mac's app and its own lanes now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub root: Option<PathBuf>,
    pub term: u64,
    pub members: Vec<String>,
    pub hosts: Vec<String>,
    pub app: String,
    pub own: Vec<OwnLane>,
}

/// One of this Mac's lanes (a slot: the agent's `worker_of`) as it is now: its job's time left (s;
/// 0, free).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OwnLane {
    pub name: String,
    pub busy_s: f64,
}

/// The files as last read.
#[derive(Clone, Debug, Default)]
struct Snap {
    status: Value,
    view: Value,
    pause: Value,
    helpers: Vec<Value>,
    forecast: Option<Forecast>,
}

#[derive(Default)]
struct State {
    snap: Option<Snap>,
    /// The forecast's input as last read, by its file's time.
    record: Option<(SystemTime, Record<'static>)>,
    /// Each history file read up to (bytes: the end of its last whole line).
    offsets: BTreeMap<String, u64>,
    history: Option<History>,
    last: Option<Instant>,
}

/// The page's data on this Mac.
#[derive(Default)]
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
        Arc::new(PageView::default())
    }

    /// A page asked: the reads go on (or start) for `ASKED_FOR_S`.
    pub fn touch(&self) {
        self.asked.store(now_s(), Ordering::Relaxed);
    }

    /// Whether a page asked lately.
    pub fn wanted(&self, now: u64) -> bool {
        now.saturating_sub(self.asked.load(Ordering::Relaxed)) < ASKED_FOR_S
    }

    /// What the agent tells it (`Inputs`).
    pub fn set_inputs(&self, i: Inputs) {
        let mut cur = self.inputs.lock().unwrap();
        if *cur != i {
            *cur = i;
        }
    }

    /// The reads, when a page asked lately and `EVERY` has passed (the first at once): true when
    /// it read.
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

    /// Reads the files and makes the snapshot.
    fn refresh(&self, src: &dyn Source, now: u64) {
        let inp = self.inputs.lock().unwrap().clone();
        let json = |p: &str| src.read(p).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let status = json("state/status.json").unwrap_or(Value::Null);
        let view = json(&super::pool::view_path(inp.term)).map_or(Value::Null, |v| v["view"].clone());
        let pause = json("state/build/pause.json").unwrap_or(Value::Null);
        let helpers: Vec<Value> = inp
            .hosts
            .iter()
            .filter_map(|h| json(&format!("state/helpers/{h}.json")))
            .filter(|h| now.saturating_sub(h["beat"].as_u64().unwrap_or(0)) < HELPER_FRESH_S)
            .map(|mut h| {
                if let Some(o) = h.as_object_mut() {
                    for k in ["regions", "checklist", "built", "recent"] {
                        o.remove(k);
                    }
                }
                h
            })
            .collect();
        let mut st = self.state.lock().unwrap();
        // The forecast's input, again only when its file changed.
        let fpath = super::pool::forecast_input_path(inp.term);
        match src.mtime(&fpath) {
            Some(t) if st.record.as_ref().is_none_or(|r| r.0 != t) => {
                match src.read(&fpath).map(|b| Record::from_bytes(&b)) {
                    Some(Ok(r)) => st.record = Some((t, r)),
                    Some(Err(e)) => eprintln!("pageview: the forecast's input: {e:#}"),
                    None => {}
                }
            }
            Some(_) => {}
            None => st.record = None,
        }
        let forecast = st.record.as_ref().map(|(_, r)| local_forecast(r, now, &inp.own));
        // The history: what each member's files of the two newest days gained.
        let mut new: Vec<(u64, String, u64, Event)> = Vec::new();
        let days: Vec<String> = [now.saturating_sub(86_400), now]
            .iter()
            .filter_map(|&t| crate::pool::journal::day(t))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        for day in &days {
            for m in &inp.members {
                let p = format!("state/coord/history/{day}/{m}.jsonl");
                let from = st.offsets.get(&p).copied().unwrap_or(0);
                let Some(b) = src.read_from(&p, from) else { continue };
                // (Whole lines alone: one being written is read next time.)
                let Some(end) = b.iter().rposition(|&c| c == b'\n') else { continue };
                for l in b[..end].split(|&c| c == b'\n') {
                    if let Ok(e) = serde_json::from_slice::<Event>(l) {
                        new.push((e.t, m.clone(), e.seq, e));
                    }
                }
                st.offsets.insert(p, from + end as u64 + 1);
            }
        }
        // (Days gone from the two: their offsets forgotten.)
        st.offsets.retain(|p, _| days.iter().any(|d| p.starts_with(&format!("state/coord/history/{d}/"))));
        new.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
        let h = st.history.get_or_insert_with(|| History::load(None));
        for (_, _, _, e) in new {
            // (Numbered here: the page's cursor, rising as events arrive.)
            h.add(e);
        }
        st.snap = Some(Snap { status, view, pause, helpers, forecast });
        st.last = Some(Instant::now());
    }

    /// `/work/swarm`'s answer (the page's shape: crate::coord's handler), from the snapshot; None
    /// before the first read. A page asked.
    pub fn swarm(&self) -> Option<Value> {
        self.touch();
        let app = self.inputs.lock().unwrap().app.clone();
        let st = self.state.lock().unwrap();
        let s = st.snap.as_ref()?;
        Some(answer(s, st.history.as_ref(), &app, now_s()))
    }

    /// `/work/history`'s answer: the events after number `since` (this Mac's numbering), at most
    /// `max`. None before the first read. A page asked.
    pub fn history(&self, since: u64, max: usize) -> Option<Value> {
        self.touch();
        let st = self.state.lock().unwrap();
        st.snap.as_ref()?;
        let h = st.history.as_ref()?;
        Some(serde_json::json!({ "seq": h.seq(), "events": h.since(since, max) }))
    }
}

/// The swarm's answer at `now` from snapshot `s`.
fn answer(s: &Snap, h: Option<&History>, app: &str, now: u64) -> Value {
    let mut agent = s.status.clone();
    if agent.is_object() {
        if let Some(f) = &s.forecast {
            agent["forecast"] = serde_json::to_value(f).unwrap_or(Value::Null);
        }
        agent["helpers"] = Value::Array(s.helpers.clone());
    }
    let (leases, workers) = crate::coord::ages(&s.view, now);
    // (The pause as the lead mirrors it, at once; else as its view said.)
    let pause = if s.pause.is_null() { s.view["pause"].clone() } else { s.pause.clone() };
    serde_json::json!({
        "now": now,
        "pause": pause,
        "agent": agent,
        "leases": leases,
        "workers": workers,
        "tasks": s.view["tasks"],
        "task_mb": s.view["task_mb"],
        "seq": h.map_or(0, History::seq),
        "rates": h.map(|h| h.rates(now, 24)),
        "app": app,
    })
}

/// The lead's forecast run here at `now`: each lane's time left and each cadence's time since
/// moved on by the time since it was made, this Mac's own lanes as `own` says.
pub fn local_forecast(r: &Record, now: u64, own: &[OwnLane]) -> Forecast {
    let mut r = r.clone();
    let dt = now.saturating_sub(r.input.now) as f64;
    let age = |m: &mut super::forecast::Machine| match own.iter().find(|o| o.name == m.name) {
        Some(o) => {
            m.busy_s = o.busy_s;
            m.from_s = (m.from_s - dt).max(0.0);
        }
        None => {
            m.busy_s = (m.busy_s - dt).max(0.0);
            m.from_s = (m.from_s - dt).max(0.0);
        }
    };
    r.input.machines.iter_mut().for_each(age);
    if let Some(e) = &mut r.equal {
        e.lanes.iter_mut().for_each(age);
    }
    let c = &mut r.input.cadence;
    let dt_s = dt as u64;
    for x in [&mut c.since_low, &mut c.since_marks, &mut c.since_overlays].into_iter().flatten() {
        *x += dt_s;
    }
    r.input.now = now.max(r.input.now);
    r.forecast()
}

/// The thread that reads while a page asks (`PageView::tick`, each second), from `root` as the
/// agent last told it.
pub fn spawn(pv: Arc<PageView>) {
    std::thread::Builder::new()
        .name("pageview".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let root = pv.inputs.lock().unwrap().root.clone();
            if let Some(r) = root {
                pv.tick(&NasFiles(r), now_s());
            }
        })
        .ok();
}

/// A path's file read here, for the tests' sources.
#[cfg(test)]
fn local(root: &std::path::Path, p: &str) -> Option<Vec<u8>> {
    std::fs::read(root.join(p)).ok()
}

#[cfg(test)]
mod tests {
    use super::super::forecast::{self, Cadence, Cost, Input, Machine};
    use super::*;
    use std::borrow::Cow;
    use std::sync::atomic::AtomicUsize;

    /// A folder's files, counted as read.
    struct Dir(PathBuf, AtomicUsize);

    impl Source for Dir {
        fn read(&self, p: &str) -> Option<Vec<u8>> {
            self.1.fetch_add(1, Ordering::Relaxed);
            local(&self.0, p)
        }
        fn mtime(&self, p: &str) -> Option<SystemTime> {
            self.1.fetch_add(1, Ordering::Relaxed);
            std::fs::metadata(self.0.join(p)).and_then(|m| m.modified()).ok()
        }
        fn read_from(&self, p: &str, offset: u64) -> Option<Vec<u8>> {
            self.1.fetch_add(1, Ordering::Relaxed);
            local(&self.0, p).map(|b| b[(offset as usize).min(b.len())..].to_vec())
        }
    }

    fn put(root: &std::path::Path, p: &str, b: &[u8]) {
        let p = root.join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }

    fn mac(name: &str, busy_s: f64) -> Machine {
        Machine { name: name.into(), speed: 1.0, measured: true, mem_mb: 8000, busy_s, ..Default::default() }
    }

    fn record(now: u64) -> Record<'static> {
        let c = Cost { secs: 600.0, known: true, peak_mb: 1000, cores: None };
        Record {
            input: Input {
                now,
                before: Vec::new(),
                regions: Cow::Owned(Vec::new()),
                costs: Default::default(),
                cadence: Cadence::default(),
                blind: None,
                chains: (0..4).map(|i| ("depth".to_string(), format!("6/1/{i}"), c)).collect(),
                after: Vec::new(),
                idle: Vec::new(),
                machines: vec![mac("m4", 300.0), Machine { equal: true, ..mac("m1", 900.0) }],
                running: BTreeMap::new(),
                macs: BTreeMap::new(),
                old_app: false,
            },
            each_s: BTreeMap::new(),
            waiting: BTreeMap::new(),
            equal: None,
        }
    }

    /// The lead's files for term 7: its status, its view, its forecast's input, two members' history
    /// of a day, a member's status.
    fn fixture(root: &std::path::Path, now: u64) {
        put(root, "state/status.json", br#"{"host":"m4","beat":1,"forecast":null,"helpers":[]}"#);
        let view = serde_json::json!({ "at": now, "view": {
            "pause": null,
            "leases": [{ "id": 1, "worker": "m1", "what": "Roads", "step": "unit", "n": 2, "granted": now - 100, "progress": null, "frac": 0.5, "beat": now - 10, "lapses": now + 290 }],
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
            &format!("state/coord/history/{day}/m-a.jsonl"),
            (ev(1, now - 50, "start") + &ev(2, now - 20, "done")).as_bytes(),
        );
        put(root, &format!("state/coord/history/{day}/m-b.jsonl"), ev(9, now - 30, "lease").as_bytes());
        put(root, "state/helpers/m1.json", format!(r#"{{"host":"m1","beat":{now},"regions":[1]}}"#).as_bytes());
    }

    fn inputs(app: &str) -> Inputs {
        Inputs {
            root: None,
            term: 7,
            members: vec!["m-a".into(), "m-b".into()],
            hosts: vec!["m1".into(), "gone".into()],
            app: app.into(),
            own: Vec::new(),
        }
    }

    #[test]
    fn the_snapshot_is_made_from_the_leads_files_in_the_pages_shape() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let pv = PageView::default();
        pv.set_inputs(inputs("1.2.3"));
        pv.touch();
        assert!(pv.tick(&Dir(d.path().into(), AtomicUsize::new(0)), now));
        let a = pv.swarm().unwrap();
        assert_eq!(a["app"], "1.2.3");
        assert_eq!(a["tasks"]["done"], 3);
        assert_eq!(a["task_mb"]["tail"], 400);
        let l = &a["leases"][0];
        assert!(l["for_s"].as_u64().unwrap() >= 100 && l.get("granted").is_none());
        assert!(l["beat_s"].as_u64().unwrap() >= 10 && l["lapses_in_s"].as_u64().unwrap() <= 290);
        assert!(a["workers"][0]["seen_s"].as_u64().unwrap() >= 5);
        // (The lead's status, its forecast run here, the members' statuses fresh, trimmed.)
        assert_eq!(a["agent"]["host"], "m4");
        assert!(a["agent"]["forecast"]["at"].as_u64().unwrap() >= now);
        assert_eq!(a["agent"]["helpers"].as_array().unwrap().len(), 1);
        assert!(a["agent"]["helpers"][0].get("regions").is_none());
        assert_eq!(a["seq"], 3);
        assert_eq!(a["rates"]["rows"].as_array().unwrap().len(), 24);
    }

    #[test]
    fn the_history_is_merged_by_time_writer_and_number_its_cursor_rising() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        pv.touch();
        pv.tick(&src, now);
        let h = pv.history(0, 100).unwrap();
        let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["start", "lease", "done"], "by time, across the writers");
        let seqs: Vec<u64> = h["events"].as_array().unwrap().iter().map(|e| e["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, [1, 2, 3]);
        // A line being written is left for next time; what's appended after is read from where it
        // stopped, numbered after what the page has (even when it happened earlier).
        let day = crate::pool::journal::day(now).unwrap();
        let p = d.path().join(format!("state/coord/history/{day}/m-b.jsonl"));
        let mut text = std::fs::read_to_string(&p).unwrap();
        text += &(serde_json::to_string(&Event { seq: 10, t: now - 40, ..Event::new("fail") }).unwrap() + "\n");
        text += r#"{"seq":11,"t":"#;
        std::fs::write(&p, &text).unwrap();
        pv.state.lock().unwrap().last = None;
        pv.tick(&src, now);
        let h = pv.history(3, 100).unwrap();
        let evs = h["events"].as_array().unwrap();
        assert_eq!((evs.len(), evs[0]["kind"].as_str(), evs[0]["seq"].as_u64()), (1, Some("fail"), Some(4)));
        assert!(pv.history(4, 100).unwrap()["events"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_members_answer_is_the_leads_but_for_its_own_lanes() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let (lead, member) = (PageView::default(), PageView::default());
        lead.set_inputs(inputs("1.2.3"));
        member.set_inputs(inputs("1.2.3"));
        for pv in [&lead, &member] {
            pv.touch();
            pv.tick(&Dir(d.path().into(), AtomicUsize::new(0)), now);
        }
        let strip = |mut a: Value| {
            a["now"] = Value::Null;
            a["agent"]["forecast"]["at"] = Value::Null;
            a
        };
        assert_eq!(strip(lead.swarm().unwrap()), strip(member.swarm().unwrap()));
        // This Mac's own lane as its slot is now: its machine's work moves, the others' don't.
        let rec = record(now);
        let theirs = local_forecast(&rec, now, &[]);
        let mine = local_forecast(&rec, now, &[OwnLane { name: "m1".into(), busy_s: 5000.0 }]);
        assert_ne!(theirs.lanes.get("m1"), mine.lanes.get("m1"));
        assert_eq!(theirs, forecast::forecast(&rec.input), "nothing moved at the time it was made");
    }

    #[test]
    fn nothing_is_read_while_no_page_asks() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        assert!(!pv.tick(&src, now));
        assert_eq!(src.1.load(Ordering::Relaxed), 0);
        // Asked: read once, not again within `EVERY`; a minute after the last ask, not at all.
        pv.touch();
        assert!(pv.tick(&src, now) && !pv.tick(&src, now));
        let n = src.1.load(Ordering::Relaxed);
        pv.state.lock().unwrap().last = None;
        assert!(!pv.tick(&src, now + ASKED_FOR_S));
        assert_eq!(src.1.load(Ordering::Relaxed), n);
        // (The forecast's input read again only when its file changed.)
        pv.touch();
        assert!(pv.tick(&src, now));
        assert_eq!(src.1.load(Ordering::Relaxed) - n, n - 1, "all but the forecast's input");
    }
}
