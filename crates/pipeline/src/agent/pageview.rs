//! The build page's data as this Mac reads it from the NAS (docs/pool.md §10, task #409): in the
//! pool, every Mac's port 8090 answers `/work/swarm` and `/work/history` the same way, from a few
//! whole files the lead writes, read by known name (never a folder listing) while a page asks.
//!
//! A thread of the agent (`spawn`) reads them only while a page has asked in the last
//! `ASKED_FOR_S`, every `EVERY` at most: the lead's status (`state/status.json`), its
//! coordinator's view (`state/coord/term/<E>/view.json`), its forecast's input
//! (`state/build/term/<E>/forecast-input.json`, read again only when its time changes), its
//! regions for the page (`state/build/term/<E>/regions.json`, the same), the
//! pause (`state/build/pause.json`), each member's heartbeat (`state/pool/members/<id>.json`, the
//! members as the pool's driver knows them: its host) and own status
//! (`state/helpers/<host>.json`), and what the members' history files of the last `DAYS` days
//! (`state/coord/history/<day>/<member>.jsonl`) gained since the last read (from the offset read
//! up to; from the start again when a file is now shorter; a day older than yesterday once read,
//! not again), `CHUNK_MAX` of a file a read at most, the week's events kept (`data` dropped: the
//! page doesn't show it), `history::KEEP_MAX` at most. Every read is done before its state's
//! lock is taken, so a slow NAS never holds up an answer. It keeps one snapshot: those, the history
//! merged and numbered here (`epoch` names the numbering), and the forecast run here
//! (`forecast::Record::forecast`) with its time brought to now, this Mac's own lanes as its slots
//! are now (`Inputs::own`, worked out as the lead works out its own: forecast::SlotJob). The
//! forecast runs on a thread of its own (`Runs`, task #544), again when the input changed or
//! `FORECAST_EVERY` passed: the snapshot carries the last one done, never waiting on it.

use super::forecast::{Forecast, Record, SlotJob};
use crate::coord::history::{self, Event, History, DAYS};
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
/// The forecast is run here again this often at most while its input is unchanged.
const FORECAST_EVERY: Duration = Duration::from_secs(60);
/// A member's status older than this (s) isn't shown (as the lead's own `helpers` takes them).
const HELPER_FRESH_S: u64 = 600;
/// The most of a history file taken in a read (bytes); the rest is read the next time.
pub const CHUNK_MAX: u64 = 4 << 20;

/// Where the files are read: the NAS (`NasFiles`), or a test's.
pub trait Source {
    /// The file at `path` (relative to the project's root) whole; None when it can't be read.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Its time last changed.
    fn mtime(&self, path: &str) -> Option<SystemTime>;
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

/// A file's inode (0 where there's none to tell: WebAssembly).
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
    fn read_from(&self, path: &str, offset: u64) -> Option<Chunk> {
        let p = self.0.join(path);
        super::nasread::with(&p.clone(), move || {
            use std::io::{Read, Seek};
            let mut f = std::fs::File::open(&p)?;
            let m = f.metadata()?;
            let from = if m.len() < offset { 0 } else { offset };
            let mut head = vec![0u8; 512];
            let n = f.read(&mut head)?;
            head.truncate(n);
            f.seek(std::io::SeekFrom::Start(from))?;
            let mut b = Vec::new();
            f.take(CHUNK_MAX).read_to_end(&mut b)?;
            Ok(Chunk { from, bytes: b, id: (ino(&m), head_of(&head)) })
        })
        .ok()
    }
}

/// What the agent tells it, each loop, from what it holds (no NAS read): the project's root, the
/// current term, the members' ids as the pool's driver knows them, this Mac's app, and its own
/// lanes now (only while a page asks: `PageView::wanted`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub root: Option<PathBuf>,
    pub term: u64,
    pub members: Vec<String>,
    pub app: String,
    pub own: Vec<OwnLane>,
}

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
    view: Value,
    pause: Value,
    helpers: Vec<Value>,
    /// Each member's addresses, from its heartbeat's broker: where a page reads its `/work/live`.
    macs: Vec<Value>,
}

/// The forecast run here off the reads (`PageView::forecast_run`): the last one done; the one
/// running; the input's time and when the last was started.
#[derive(Default)]
struct Runs {
    done: Option<Forecast>,
    running: Option<std::thread::JoinHandle<Forecast>>,
    of: Option<(SystemTime, Instant)>,
}

impl Runs {
    /// The one running, taken in once it's done (`wait`: waited for).
    fn take(&mut self, wait: bool) {
        if self.running.as_ref().is_some_and(|h| wait || h.is_finished()) {
            match self.running.take().map(std::thread::JoinHandle::join) {
                Some(Ok(f)) => self.done = Some(f),
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
    history: Option<History>,
    /// The history's numbering: made with the first snapshot (a new one each process).
    epoch: String,
    last: Option<Instant>,
    /// History chunks of CHUNK_MAX passed over for holding no line's end.
    skipped: u64,
}

/// The page's data on this Mac.
#[derive(Default)]
pub struct PageView {
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
    pub fn new() -> Arc<PageView> {
        Arc::new(PageView::default())
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

    /// Whether it serves the page (the agent told it a root: in the pool). Not: the coordinator
    /// answers from its memory, as out of the pool.
    pub fn active(&self) -> bool {
        self.inputs.lock().unwrap().root.is_some()
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

    /// Reads the files (without the state's lock: what it needs of it taken first) and makes the
    /// snapshot (under it, briefly).
    fn refresh(&self, src: &dyn Source, now: u64) {
        let inp = self.inputs.lock().unwrap().clone();
        let (offsets, had, had_regions) = {
            let st = self.state.lock().unwrap();
            (st.offsets.clone(), st.record.clone(), st.regions.clone())
        };
        let json = |p: &str| src.read(p).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        let status = json("state/status.json").unwrap_or(Value::Null);
        let view = json(&super::pool::view_path(inp.term)).map_or(Value::Null, |v| v["view"].clone());
        let pause = json("state/build/pause.json").unwrap_or(Value::Null);
        // (Each member's host and addresses, from its heartbeat; then its own status.)
        let beats: Vec<Value> =
            inp.members.iter().filter_map(|m| json(&format!("state/pool/members/{m}.json"))).collect();
        let macs: Vec<Value> = beats.iter().filter_map(mac_of).collect();
        let helpers: Vec<Value> = beats
            .iter()
            .filter_map(|b| b["host"].as_str().map(str::to_string))
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
        // The forecast's input, again only when its file changed.
        let fpath = super::pool::forecast_input_path(inp.term);
        let record = match src.mtime(&fpath) {
            Some(t) if had.as_ref().is_none_or(|r| r.0 != t) => {
                match src.read(&fpath).map(|b| Record::from_bytes(&b)) {
                    Some(Ok(r)) => Some((t, Arc::new(r))),
                    Some(Err(e)) => {
                        eprintln!("pageview: the forecast's input: {e:#}");
                        had
                    }
                    None => had,
                }
            }
            Some(_) => had,
            None => None,
        };
        self.forecast_run(record.as_ref(), now, &inp.own);
        // The regions, again only when their file changed.
        let rpath = super::pool::regions_path(inp.term);
        let regions = match src.mtime(&rpath) {
            Some(t) if had_regions.as_ref().is_none_or(|r| r.0 != t) => {
                json(&rpath).map(|v| (t, Arc::new(v))).or(had_regions)
            }
            Some(_) => had_regions,
            None => None,
        };
        // The history: what each member's files of the last DAYS days gained.
        let mut new: Vec<(u64, String, u64, Event)> = Vec::new();
        let mut read_to: BTreeMap<String, (u64, FileId, bool)> = BTreeMap::new();
        let days: Vec<(u64, String)> = (0..DAYS)
            .filter_map(|k| crate::pool::journal::day(now.saturating_sub(k * 86_400)).map(|d| (k, d)))
            .collect();
        let week = now.saturating_sub(history::KEEP_S);
        let mut skipped = 0;
        for (k, day) in &days {
            for m in &inp.members {
                let p = format!("state/coord/history/{day}/{m}.jsonl");
                let kept = offsets.get(&p);
                // (A day before yesterday, read already to its end: done with, kept as read.)
                if *k >= 2 {
                    if let Some(kk) = kept.filter(|kk| kk.2) {
                        read_to.insert(p, kk.clone());
                        continue;
                    }
                }
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
        }
        new.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
        // (The newest KEEP_MAX alone: the most the history keeps.)
        new.drain(..new.len().saturating_sub(history::KEEP_MAX));
        // Swapped in.
        let mut st = self.state.lock().unwrap();
        // (Days gone from the last DAYS: their offsets forgotten.)
        st.offsets = read_to;
        if skipped > 0 {
            st.skipped += skipped;
            eprintln!("pageview: {} history chunks passed over without a line's end", st.skipped);
        }
        st.record = record;
        st.regions = regions;
        if st.history.is_none() {
            st.epoch = format!("view {} {}", std::process::id(), now);
        }
        st.taken.retain(|k| k.1 >= week);
        let fresh: Vec<Event> = new
            .into_iter()
            .filter(|(t, m, seq, e)| st.taken.insert((m.clone(), *t, *seq, e.kind.clone())))
            .map(|n| n.3)
            .collect();
        let h = st.history.get_or_insert_with(|| History::load(None));
        for e in fresh {
            // (Numbered here: the page's cursor, rising as events arrive.)
            h.add(e);
        }
        // (What's taken in, as many as the history keeps: those it let go of forgotten.)
        if st.taken.len() > history::KEEP_MAX {
            let oldest = st.history.as_ref().and_then(History::oldest).unwrap_or(week);
            st.taken.retain(|k| k.1 >= oldest);
        }
        st.snap = Some(Snap { status, view, pause, helpers, macs });
        st.last = Some(Instant::now());
    }

    /// `/work/swarm`'s answer (the page's shape: crate::coord's handler), from the snapshot; None
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
        Some(answer(s, forecast.as_ref(), st.history.as_ref(), &st.epoch, &app, now_s()))
    }

    /// `/work/regions`' answer (crate::agent::regionsview): the lead's as last read; None before
    /// they're read (or while the lead hasn't written them). A page asked.
    pub fn regions(&self) -> Option<Arc<Value>> {
        self.touch();
        self.state.lock().unwrap().regions.as_ref().map(|r| r.1.clone())
    }

    /// `/work/history`'s answer: the events after number `since` (this Mac's numbering, `epoch`),
    /// at most `max`. None before the first read. A page asked.
    pub fn history(&self, since: u64, max: usize) -> Option<Value> {
        self.touch();
        let st = self.state.lock().unwrap();
        st.snap.as_ref()?;
        let h = st.history.as_ref()?;
        Some(serde_json::json!({ "seq": h.seq(), "epoch": st.epoch, "events": h.since(since, max) }))
    }
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
        "epoch": epoch,
        "rates": h.map(|h| h.rates(now, 24)),
        "app": app,
        "macs": s.macs,
    })
}

/// A member's host and addresses (`{host, urls, https}`: its broker's, HTTP then HTTPS) from its
/// heartbeat; None without a host or a broker (an app before the brokers, or their switch off).
fn mac_of(beat: &Value) -> Option<Value> {
    let host = beat["host"].as_str()?;
    let b: crate::coord::brokers::Broker = serde_json::from_value(beat.get("broker")?.clone()).ok()?;
    Some(serde_json::json!({ "host": host, "urls": b.urls, "https": b.https }))
}

impl PageView {
    /// The lead's forecast (`record`, by its file's time) run here at `now` on a thread of its own,
    /// when none is running and its input changed or `FORECAST_EVERY` passed; none without a
    /// record. The snapshot's answer takes the last one done (`Runs`).
    fn forecast_run(&self, record: Option<&(SystemTime, Arc<Record<'static>>)>, now: u64, own: &[OwnLane]) {
        let mut r = self.forecast.lock().unwrap();
        r.take(false);
        // (No record: the last one dropped; one running kept until it's done, never two at once.)
        let Some((t, rec)) = record else {
            (r.done, r.of) = (None, None);
            return;
        };
        let due = r.of.is_none_or(|(at, started)| at != *t || started.elapsed() >= FORECAST_EVERY);
        if r.running.is_some() || !due {
            return;
        }
        let (rec, own) = (rec.clone(), own.to_vec());
        match std::thread::Builder::new().name("pageview-forecast".into()).spawn(move || {
            super::forecast::low_priority();
            local_forecast(&rec, now, &own)
        }) {
            Ok(h) => (r.running, r.of) = (Some(h), Some((*t, Instant::now()))),
            Err(e) => eprintln!("pageview: the forecast's thread: {e}"),
        }
    }
}

/// The lead's forecast run here at `now`: each lane's time left and each cadence's time since
/// moved on by the time since it was made; this Mac's own lanes (`own`) as their slots are now,
/// their time left worked out as the lead works out its own (forecast::SlotJob, each target's cost
/// from the input's table, else its step's time a target: `each_s`).
pub fn local_forecast(r: &Record, now: u64, own: &[OwnLane]) -> Forecast {
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
        fn read_from(&self, p: &str, offset: u64) -> Option<Chunk> {
            self.1.fetch_add(1, Ordering::Relaxed);
            let b = local(&self.0, p)?;
            let ino = ino(&std::fs::metadata(self.0.join(p)).ok()?);
            let from = if (b.len() as u64) < offset { 0 } else { offset };
            let to = b.len().min(from as usize + CHUNK_MAX as usize);
            Some(Chunk { from, bytes: b[from as usize..to].to_vec(), id: (ino, head_of(&b)) })
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
        let c = Cost { secs: 600.0, known: true, peak_mb: 1000 };
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
                sched: None,
                release: Vec::new(),
                held: Default::default(),
                pace: None,
            },
            each_s: BTreeMap::new(),
            waiting: BTreeMap::new(),
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
        put(
            root,
            "state/pool/members/m-a.json",
            br#"{"member":"m-a","host":"m1","broker":{"urls":["http://100.1.1.1:8090"],"https":"https://m1.t.ts.net/"}}"#,
        );
        put(root, "state/pool/members/m-b.json", br#"{"member":"m-b","host":"gone"}"#);
        put(root, "state/helpers/m1.json", format!(r#"{{"host":"m1","beat":{now},"regions":[1]}}"#).as_bytes());
    }

    /// The forecast run off the reads (`Runs`), waited for.
    fn settle(pv: &PageView) {
        pv.forecast.lock().unwrap().take(true);
    }

    fn inputs(app: &str) -> Inputs {
        Inputs { root: None, term: 7, members: vec!["m-a".into(), "m-b".into()], app: app.into(), own: Vec::new() }
    }

    #[test]
    fn the_snapshot_is_made_from_the_leads_files_in_the_pages_shape() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let pv = PageView::default();
        pv.set_inputs(inputs("1.2.3"));
        pv.touch_at(now);
        assert!(pv.tick(&Dir(d.path().into(), AtomicUsize::new(0)), now));
        settle(&pv);
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
        // (Each member with a broker, where its `/work/live` is read; one without, missing.)
        assert_eq!(
            a["macs"],
            serde_json::json!([{ "host": "m1", "urls": ["http://100.1.1.1:8090"], "https": "https://m1.t.ts.net/" }])
        );
    }

    #[test]
    fn the_regions_are_the_leads_read_again_only_when_their_file_changes() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        pv.tick(&src, now);
        assert!(pv.regions().is_none(), "none written yet");
        let path = d.path().join(super::super::pool::regions_path(7));
        put(d.path(), &super::super::pool::regions_path(7), br#"{"catalog":3,"regions":[{"id":"a"}]}"#);
        pv.state.lock().unwrap().last = None;
        pv.tick(&src, now);
        assert_eq!(pv.regions().unwrap()["catalog"], 3);
        // (The same time, other bytes: not read again; a new time: read.)
        let t = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, br#"{"catalog":4,"regions":[]}"#).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(t).unwrap();
        pv.state.lock().unwrap().last = None;
        pv.tick(&src, now);
        assert_eq!(pv.regions().unwrap()["catalog"], 3);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(t + Duration::from_secs(5)).unwrap();
        pv.state.lock().unwrap().last = None;
        pv.tick(&src, now);
        assert_eq!(pv.regions().unwrap()["catalog"], 4);
    }

    #[test]
    fn the_history_is_merged_by_time_writer_and_number_its_cursor_rising() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
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
            pv.touch_at(now);
            pv.tick(&Dir(d.path().into(), AtomicUsize::new(0)), now);
            settle(pv);
        }
        let strip = |mut a: Value| {
            a["now"] = Value::Null;
            a["epoch"] = Value::Null;
            a["agent"]["forecast"]["at"] = Value::Null;
            a
        };
        assert_eq!(strip(lead.swarm().unwrap()), strip(member.swarm().unwrap()));
        // This Mac's own lane as its slot is now (as the lead works its own out): its machine's
        // lane's work moves (and what the others take with it).
        let rec = record(now);
        let theirs = local_forecast(&rec, now, &[]);
        let job = SlotJob {
            step: "depth".into(),
            targets: vec![("6/9/9".into(), false), ("6/9/8".into(), true)],
            alone_s: None,
            elapsed_s: 100.0,
            eta: None,
        };
        assert_eq!(job.busy_s(&|_, _| 3000.0, 1.0), 3000.0, "one left, its time spent within the one done");
        let own = [OwnLane { name: "m1".into(), job: Some(job) }];
        let mine = local_forecast(&rec, now, &own);
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
        pv.touch_at(now);
        assert!(pv.tick(&src, now) && !pv.tick(&src, now));
        let n = src.1.load(Ordering::Relaxed);
        pv.state.lock().unwrap().last = None;
        assert!(!pv.tick(&src, now + ASKED_FOR_S));
        assert_eq!(src.1.load(Ordering::Relaxed), n);
        // (The forecast's input read again only when its file changed.)
        pv.touch_at(now);
        assert!(pv.tick(&src, now));
        assert_eq!(src.1.load(Ordering::Relaxed) - n, n - 1, "all but the forecast's input");
    }

    #[test]
    fn a_new_numbering_has_a_new_epoch() {
        // (Task #409 review) the page starts its cursor again when the epoch changes: a new
        // process's view (its agent restarted) numbers from 1 again, under another epoch.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let first = PageView::default();
        first.set_inputs(inputs(""));
        assert!(first.swarm().is_none() && first.history(0, 10).is_none(), "nothing before the first read");
        first.tick(&Dir(d.path().into(), AtomicUsize::new(0)), now);
        let a = first.swarm().unwrap();
        let again = PageView::default();
        again.set_inputs(inputs(""));
        again.touch_at(now);
        again.tick(&Dir(d.path().into(), AtomicUsize::new(0)), now + 1);
        let b = again.swarm().unwrap();
        assert!(a["epoch"].as_str().is_some_and(|e| !e.is_empty()));
        assert_ne!(a["epoch"], b["epoch"]);
        assert_eq!(first.history(0, 10).unwrap()["epoch"], a["epoch"]);
        // (Its own numbering stays its own across reads.)
        first.state.lock().unwrap().last = None;
        first.tick(&Dir(d.path().into(), AtomicUsize::new(0)), now);
        assert_eq!(first.swarm().unwrap()["epoch"], a["epoch"]);
    }

    /// A source whose reads wait until they're let go.
    struct Slow(Dir, Mutex<std::sync::mpsc::Receiver<()>>);

    impl Source for Slow {
        fn read(&self, p: &str) -> Option<Vec<u8>> {
            self.1.lock().unwrap().recv().ok();
            self.0.read(p)
        }
        fn mtime(&self, p: &str) -> Option<SystemTime> {
            self.0.mtime(p)
        }
        fn read_from(&self, p: &str, offset: u64) -> Option<Chunk> {
            self.0.read_from(p, offset)
        }
    }

    #[test]
    fn a_slow_read_never_holds_up_an_answer() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let pv = Arc::new(PageView::default());
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        pv.tick(&Dir(d.path().into(), AtomicUsize::new(0)), now);
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

    #[test]
    fn a_history_file_made_again_shorter_is_read_from_its_start() {
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        pv.tick(&src, now);
        let day = crate::pool::journal::day(now).unwrap();
        let line = serde_json::to_string(&Event { seq: 1, t: now - 5, ..Event::new("catalog") }).unwrap() + "\n";
        std::fs::write(d.path().join(format!("state/coord/history/{day}/m-a.jsonl")), line).unwrap();
        pv.state.lock().unwrap().last = None;
        pv.tick(&src, now);
        let h = pv.history(3, 10).unwrap();
        assert_eq!(h["events"][0]["kind"], "catalog");
    }

    #[test]
    fn a_history_file_written_again_whole_is_read_again_without_twice_the_events() {
        // (Task #409 review) made again in its place (renamed over it), longer than before: read
        // from its start, the events taken already passed over.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        pv.tick(&src, now);
        let day = crate::pool::journal::day(now).unwrap();
        let p = d.path().join(format!("state/coord/history/{day}/m-a.jsonl"));
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
        pv.tick(&src, now);
        let h = pv.history(3, 10).unwrap();
        let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["catalog", "pause", "resume"], "the old second not again");
    }

    #[test]
    fn the_history_is_the_last_seven_days_an_old_day_read_once() {
        // (Task #509) today's and the six days before; the eighth not; `data` dropped.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        for k in 1..=DAYS {
            let t = now - k * 86_400 + 60;
            let day = crate::pool::journal::day(t).unwrap();
            let e = Event {
                seq: 100 + k,
                t,
                note: format!("day {k}"),
                data: Some(serde_json::json!({ "big": 1 })),
                ..Event::new("task")
            };
            let line = serde_json::to_string(&e).unwrap() + "\n";
            put(d.path(), &format!("state/coord/history/{day}/m-a.jsonl"), line.as_bytes());
        }
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        pv.tick(&src, now);
        let h = pv.history(0, 1000).unwrap();
        let evs = h["events"].as_array().unwrap();
        let notes: Vec<&str> = evs.iter().filter_map(|e| e["note"].as_str()).collect();
        assert_eq!(notes, ["day 6", "day 5", "day 4", "day 3", "day 2", "day 1"], "the 7th day back is past the week");
        assert!(evs.iter().all(|e| e.get("data").is_none()));
        // (A second read: m-a's five days before yesterday not read again; today's and
        // yesterday's are, and m-b's missing files are tried again.)
        let first = src.1.load(Ordering::Relaxed);
        pv.state.lock().unwrap().last = None;
        pv.tick(&src, now);
        let history = 2 * 2 + 5; // (today's and yesterday's of both; m-b's five missing old days)
        let others = 9; // (the lead's files, the beats and the members' statuses; not the forecast's input)
        assert_eq!(src.1.load(Ordering::Relaxed) - first, history + others);
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
            put(d.path(), &format!("state/coord/history/{day}/m-b.jsonl"), text.as_bytes());
        }
        put(d.path(), "state/status.json", b"{}");
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        pv.tick(&src, now);
        let h = pv.history(0, usize::MAX).unwrap();
        let evs = h["events"].as_array().unwrap();
        assert_eq!(evs.len(), history::KEEP_MAX);
        assert_eq!(evs.last().unwrap()["t"].as_u64(), Some(now - 3600 - 1));
        assert!(pv.state.lock().unwrap().taken.len() <= history::KEEP_MAX);
        // (Read again with nothing new: none added.)
        pv.state.lock().unwrap().last = None;
        pv.tick(&src, now);
        assert_eq!(pv.history(0, 1).unwrap()["seq"].as_u64(), Some(history::KEEP_MAX as u64));
    }

    #[test]
    fn a_line_longer_than_a_chunk_is_passed_over() {
        // (Task #509 review) a chunk with no line's end: skipped, counted, the lines after it read.
        let d = tempfile::tempdir().unwrap();
        let now = now_s();
        fixture(d.path(), now);
        let day = crate::pool::journal::day(now).unwrap();
        let p = d.path().join(format!("state/coord/history/{day}/m-b.jsonl"));
        let mut text = "x".repeat(CHUNK_MAX as usize + 10) + "\n";
        text += &(serde_json::to_string(&Event { seq: 1, t: now - 5, ..Event::new("pause") }).unwrap() + "\n");
        std::fs::write(&p, text).unwrap();
        let src = Dir(d.path().into(), AtomicUsize::new(0));
        let pv = PageView::default();
        pv.set_inputs(inputs(""));
        pv.touch_at(now);
        for _ in 0..3 {
            pv.state.lock().unwrap().last = None;
            pv.tick(&src, now);
        }
        assert_eq!(pv.state.lock().unwrap().skipped, 1);
        let h = pv.history(0, 10).unwrap();
        let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["start", "done", "pause"]);
    }
}
