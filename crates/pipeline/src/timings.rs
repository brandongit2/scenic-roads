//! A job's timings (docs/building.md §2, Timings; the record: docs/formats.md, timings.jsonl): its run
//! broken into named phases, each with its wall time, its CPU time (user + system, the process's
//! and its finished children's), its wall time split by what the job waited on while it was open
//! (`Waits`: measured, store::waits) and, for I/O, the bytes and files it moved.
//!
//! - `phase(name)` opens a phase (a guard: it ends when dropped); `sub` opens one inside the
//!   phase open on this thread, at most one level down. A phase opened again under the same name
//!   adds to it (`n` counts its spans): a loop over z3 tiles or tiles gives each of its stages one
//!   phase, with totals, never one per z3 tile.
//! - `count(bytes, files)` adds to the innermost phase open on this thread (`Phase::count` to a
//!   given one, from any thread): an addition to two atomics, cheap enough for a loop's body.
//! - A child program run within a phase (`child(&mut cmd)`) writes its own phases (this module's
//!   twin in Python, dem/timings.py, or a scenic-build run as a child) to a file this one names; its
//!   phases come in as the phase's sub-phases as it ends.
//! - CPU is the process's, from `getrusage`: a phase while another thread's phase (or work in no
//!   phase) runs counts that thread's CPU too. Such a phase is marked `overlapped`, its CPU an
//!   approximation; a phase on a thread but the job's main one is marked `background`, and its wall
//!   isn't counted toward the run's timed part (it ran beside the main thread's phases).
//! - Memory: while a run is timed, a thread samples the job's memory four times a second (its
//!   process group's physical footprints summed, `crate::sys::group_footprint`, as the agent's
//!   memory guard measures a job), and each phase keeps the most it saw while open (`peak_mb`): a
//!   phase that runs a program (a unit's steps, the heritage chain's scripts, osmium) is that
//!   program's peak, with the job's own memory beside it. A child's own phases carry its samples.
//! - Waits (task #870): as a span opens and closes it takes store::waits' totals, the job's wall
//!   time blocked in NAS calls, network requests, local disk calls and waits on other work so far,
//!   and the NAS's and the network's bytes (store::nasio, crate::fetch); the differences are its
//!   `Waits`, the rest of its wall time computing. They count by the job's wall clock (an instant
//!   is the NAS's when any of its threads is in a NAS call, else the network's, else the disk's,
//!   else other work's: store::waits), so a phase's parts add up to its wall time. A phase that
//!   runs a child program takes in the waits of the child's main-thread phases (its own measured:
//!   the Python twin, dem/timings.py, measures them the same way). No phase is classed by hand.
//! - Live: the top-level phase open on the main thread, and since when, is said in the
//!   job's counts file (store::nasio::set_phase, a few stores in mapped memory) as it opens, and
//!   cleared as it ends; the agent shows it under the job (crate::agent::jobs::Live).
//! - `finish(ok)` ends the run: the readable table in the log (stderr), and the record, where
//!   `SCENIC_TIMINGS` says (the agent collects it), or as a child's phases where `SCENIC_PHASES_TO`
//!   says. A run never `start`ed (a program run by hand, a test) keeps nothing and prints nothing.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Where a job's run record goes (one JSON object; the agent sets it, and collects the file).
pub const TIMINGS_ENV: &str = "SCENIC_TIMINGS";
/// The job's id as the agent knows it ("unit 6/32/24").
pub const JOB_ID_ENV: &str = "SCENIC_JOB_ID";
/// Where a child program writes its phases, for the phase that ran it.
pub const PHASES_TO_ENV: &str = "SCENIC_PHASES_TO";
/// The record's version.
pub const VERSION: u32 = 1;

/// A phase's wall time by what the job waited on while it was open, measured (store::waits): by
/// the job's wall clock, so the waits never sum past the phase's wall time; the rest is computing.
/// The NAS's and the network's bytes moved meanwhile beside them (the job's, all its threads').
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Waits {
    #[serde(default)]
    pub nas_s: f64,
    #[serde(default)]
    pub net_s: f64,
    #[serde(default)]
    pub disk_s: f64,
    /// Waiting on other work: another worker, another thread, a lock.
    #[serde(default)]
    pub other_s: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub nas_b: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub net_b: u64,
}

impl Waits {
    /// Its waits' seconds together.
    pub fn waited(&self) -> f64 {
        self.nas_s + self.net_s + self.disk_s + self.other_s
    }

    fn add(&mut self, o: &Waits) {
        self.nas_s += o.nas_s;
        self.net_s += o.net_s;
        self.disk_s += o.disk_s;
        self.other_s += o.other_s;
        self.nas_b += o.nas_b;
        self.net_b += o.net_b;
    }

    /// Held to `wall`: scaled down when they sum past it (a child's waits taken in beside its
    /// parent's own).
    fn within(mut self, wall: f64) -> Waits {
        let w = self.waited();
        if w > wall && w > 0.0 {
            let k = wall / w;
            self.nas_s *= k;
            self.net_s *= k;
            self.disk_s *= k;
            self.other_s *= k;
        }
        self
    }

    /// A phase's waits: its measured ones, else (a record from before task #870, or a page's) its
    /// declared class's, the whole of its wall time; None when neither says.
    pub fn of(p: &PhaseRec) -> Option<Waits> {
        if p.waits.is_some() {
            return p.waits;
        }
        let w = p.wall_s;
        let (nas_b, net_b) = match p.class.as_str() {
            "nas-read" | "nas-write" => (p.bytes, 0),
            "net" => (0, p.bytes),
            _ => (0, 0),
        };
        Some(match p.class.as_str() {
            "nas-read" | "nas-write" => Waits { nas_s: w, nas_b, ..Default::default() },
            "net" => Waits { net_s: w, net_b, ..Default::default() },
            "disk" => Waits { disk_s: w, ..Default::default() },
            "wait" => Waits { other_s: w, ..Default::default() },
            "compute" => Waits::default(),
            _ => return None,
        })
    }

    /// The waits between two of store::waits' totals and the bytes then (`at` the earlier).
    fn between(at: &(store::waits::Totals, u64, u64), now: &(store::waits::Totals, u64, u64)) -> Waits {
        let d = |i: usize| now.0[i].saturating_sub(at.0[i]) as f64 / 1e9;
        Waits {
            nas_s: d(0),
            net_s: d(1),
            disk_s: d(2),
            other_s: d(3),
            nas_b: now.1.saturating_sub(at.1),
            net_b: now.2.saturating_sub(at.2),
        }
    }

    /// Its parts as shares of `wall`, readably ("NAS 41%, network 3%"; none under 1%).
    pub fn text(&self, wall: f64) -> String {
        if wall <= 0.0 {
            return String::new();
        }
        let mut v = Vec::new();
        for (name, s) in
            [("NAS", self.nas_s), ("network", self.net_s), ("disk", self.disk_s), ("other work", self.other_s)]
        {
            let f = s / wall * 100.0;
            if f >= 1.0 {
                v.push(format!("{name} {f:.0}%"));
            }
        }
        v.push(format!("CPU-side {:.0}%", ((wall - self.waited()) / wall * 100.0).max(0.0)));
        v.join(" ")
    }
}

/// store::waits' totals and the NAS's and network's bytes now (a span's ends).
fn waits_now() -> (store::waits::Totals, u64, u64) {
    use store::nasio::{get, Count};
    (store::waits::totals(), get(Count::ReadBytes) + get(Count::WroteBytes), crate::fetch::fetched_bytes())
}

fn add_waits(a: &mut Option<Waits>, b: Option<Waits>) {
    if let Some(b) = b {
        a.get_or_insert_with(Waits::default).add(&b);
    }
}

/// A phase as recorded: its totals over its spans.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseRec {
    /// Its name (one from before task #844 read as its new one: crate::phase_names).
    #[serde(deserialize_with = "current_name")]
    pub name: String,
    /// What it was declared to wait on, in a record from before task #870 (measured since: `waits`),
    /// or a page's: "nas-read", "net", "compute" …. (The Python twin, dem/timings.py, measures.)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub class: String,
    pub wall_s: f64,
    /// Its wall time by what the job waited on (task #870): None in a record from before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waits: Option<Waits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_s: Option<f64>,
    /// Its spans (a loop's stage: one a pass).
    pub n: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub files: u64,
    /// Another thread's work ran during it: its CPU counts that too (approximate).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overlapped: bool,
    /// Run on a thread beside the main one: not counted toward the run's timed part.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    /// The most memory the job's processes held together while it was open (MB; the most over
    /// its spans): None where the job's memory can't be read, or from a record before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub: Vec<PhaseRec>,
}

/// A phase's name as recorded, renamed when it's one from before task #844 (crate::phase_names).
fn current_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    let n = crate::phase_names::current(&s);
    Ok(if n == s.as_str() { s } else { n.into_owned() })
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A run's record (a line of timings.jsonl).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunRec {
    pub v: u32,
    /// The job's kind (its step: "unit", "peaks", "gc").
    pub kind: String,
    /// The job's id, as the agent knows it.
    #[serde(default)]
    pub id: String,
    /// The Mac (or page) that ran it: the agent or coordinator collecting it says.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// When it started (seconds since the epoch).
    pub start: u64,
    pub wall_s: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_s: Option<f64>,
    /// Its worker threads (RAYON_NUM_THREADS), where set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    pub ok: bool,
    /// The run's time in no phase of its main thread.
    pub untimed_s: f64,
    /// What the timing itself cost (seconds, measured: the phases' opening and closing).
    #[serde(default)]
    pub overhead_s: f64,
    pub phases: Vec<PhaseRec>,
    /// Where its untimed time lies: the main thread's spans between two top-level phases, by the
    /// phases either side ("the start", "the end" at the run's ends), largest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<Gap>,
    /// The safe points it reached (crate::control::safe_gaps; task #885), and the longest it went
    /// without one, in seconds (from its start, and to its end): how soon a pause could stop it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub safe_points: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_gap_s: Option<f64>,
}

/// Untimed time between two of a run's top-level phases on its main thread, summed over the times
/// it fell there.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    #[serde(deserialize_with = "current_name")]
    pub after: String,
    #[serde(deserialize_with = "current_name")]
    pub before: String,
    pub wall_s: f64,
    pub n: u64,
}

/// A run's untimed share past which the report flags it (docs/building.md §2, Timings: under about 5 %).
pub const UNACCOUNTED_MAX: f64 = 0.05;
/// A gap shorter than this isn't kept (a phase's close and the next's open).
const GAP_MIN_NS: u64 = 1_000_000;
/// How many gaps a run keeps, largest first.
const GAPS_KEPT: usize = 8;
/// Where nothing is either side of a gap.
const START: &str = "the start";
const END: &str = "the end";

impl RunRec {
    /// The main thread's top-level phases' wall time.
    pub fn timed_s(&self) -> f64 {
        self.phases.iter().filter(|p| !p.background).map(|p| p.wall_s).sum()
    }
}

/// A phase's running totals.
#[derive(Clone)]
struct Acc {
    name: String,
    parent: Option<usize>,
    /// Its spans' waits (`Waits::between` each span's ends).
    waits: Waits,
    wall_ns: u64,
    cpu_us: Option<u64>,
    n: u64,
    bytes: u64,
    files: u64,
    overlapped: bool,
    background: bool,
    /// The most memory seen while a span of it was open (bytes; `Open::peak`).
    peak: Option<u64>,
    /// Its child programs' phases, as they came in.
    kids: Vec<PhaseRec>,
}

/// An open span's counters, which other threads (and `count`) add to.
struct Open {
    idx: usize,
    bytes: AtomicU64,
    files: AtomicU64,
    overlapped: AtomicBool,
    /// The most memory sampled while it's open (bytes; 0: none yet).
    peak: AtomicU64,
    thread: std::thread::ThreadId,
    children: Mutex<Vec<PathBuf>>,
    /// When it opened, and the CPU then: a span still open as the run ends counts to then.
    t0: Instant,
    cpu0: Option<u64>,
    /// store::waits' totals and the bytes as it opened.
    w0: (store::waits::Totals, u64, u64),
}

struct Run {
    kind: String,
    start_wall: u64,
    t0: Instant,
    cpu0: Option<u64>,
    main: std::thread::ThreadId,
    accs: Vec<Acc>,
    open: Vec<Arc<Open>>,
    overhead_ns: u64,
    finished: bool,
    /// The main thread's last top-level phase to end, and when (the start at first).
    mark: (Instant, String),
    /// Its gaps so far: (after, before) → (ns, spans).
    gaps: Vec<(String, String, u64, u64)>,
}

/// Adds a gap of `ns` between `after` and `before` (none under `GAP_MIN_NS`).
fn add_gap(gaps: &mut Vec<(String, String, u64, u64)>, after: &str, before: &str, ns: u64) {
    if ns < GAP_MIN_NS {
        return;
    }
    match gaps.iter_mut().find(|g| g.0 == after && g.1 == before) {
        Some(g) => {
            g.2 += ns;
            g.3 += 1;
        }
        None => gaps.push((after.to_string(), before.to_string(), ns, 1)),
    }
}

static RUN: OnceLock<Mutex<Option<Run>>> = OnceLock::new();

fn run() -> &'static Mutex<Option<Run>> {
    RUN.get_or_init(|| Mutex::new(None))
}

thread_local! {
    static STACK: RefCell<Vec<Arc<Open>>> = const { RefCell::new(Vec::new()) };
}

/// The process's CPU time so far (user + system, its own threads' and its finished children's),
/// microseconds. None where there's no `getrusage` (WebAssembly).
pub fn cpu_us() -> Option<u64> {
    #[cfg(all(unix, not(target_os = "wasi")))]
    {
        let tv = |t: libc::timeval| t.tv_sec as u64 * 1_000_000 + t.tv_usec as u64;
        let mut total = 0;
        for who in [libc::RUSAGE_SELF, libc::RUSAGE_CHILDREN] {
            // SAFETY: getrusage fills the struct it's given.
            let mut r: libc::rusage = unsafe { std::mem::zeroed() };
            if unsafe { libc::getrusage(who, &mut r) } != 0 {
                return None;
            }
            total += tv(r.ru_utime) + tv(r.ru_stime);
        }
        Some(total)
    }
    #[cfg(not(all(unix, not(target_os = "wasi"))))]
    {
        None
    }
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Starts timing this process's run as a job of `kind` (its step), from now. Again: no change.
pub fn start(kind: &str) {
    let mut g = run().lock().unwrap();
    if g.is_none() {
        crate::control::gaps_begin();
        store::waits::enable();
        *g = Some(Run {
            kind: kind.to_string(),
            start_wall: now_s(),
            t0: Instant::now(),
            cpu0: cpu_us(),
            main: std::thread::current().id(),
            accs: Vec::new(),
            open: Vec::new(),
            overhead_ns: 0,
            finished: false,
            mark: (Instant::now(), START.to_string()),
            gaps: Vec::new(),
        });
        drop(g);
        sample_memory();
    }
}

/// The job's memory as last sampled (bytes; 0: not sampled).
static LAST_MEM: AtomicU64 = AtomicU64::new(0);
/// How often the job's memory is sampled.
const MEM_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

/// Starts the thread that samples the job's memory (`crate::sys::group_footprint`) into every open
/// span while the run is timed; none where it can't be read (WebAssembly, elsewhere than macOS).
fn sample_memory() {
    static ON: std::sync::Once = std::sync::Once::new();
    let Some(now) = crate::sys::group_footprint() else { return };
    LAST_MEM.store(now, Ordering::Relaxed);
    ON.call_once(|| {
        std::thread::Builder::new()
            .name("timings-memory".into())
            .spawn(|| loop {
                std::thread::sleep(MEM_EVERY);
                // (Sampled while a run is timed, spans open or not: a span opening takes the last
                // sample, at most `MEM_EVERY` old, as its first.)
                if !on() {
                    continue;
                }
                let Some(v) = crate::sys::group_footprint() else { continue };
                LAST_MEM.store(v, Ordering::Relaxed);
                let Ok(g) = run().lock() else { return };
                for o in g.iter().flat_map(|r| &r.open) {
                    o.peak.fetch_max(v, Ordering::Relaxed);
                }
            })
            .ok();
    });
}

/// Folds a span's peak (bytes; 0: none) into a phase's.
fn fold_peak(a: Option<u64>, b: u64) -> Option<u64> {
    match (a, b) {
        (a, 0) => a,
        (Some(a), b) => Some(a.max(b)),
        (None, b) => Some(b),
    }
}

fn mb(b: Option<u64>) -> Option<u64> {
    b.map(|b| b >> 20)
}

/// Whether a run is being timed.
pub fn on() -> bool {
    run().lock().unwrap().as_ref().is_some_and(|r| !r.finished)
}

/// An open phase: it ends when dropped.
pub struct Phase {
    open: Option<Arc<Open>>,
    t: Instant,
    cpu: Option<u64>,
}

impl Phase {
    /// Adds bytes and files moved to it.
    pub fn count(&self, bytes: u64, files: u64) {
        if let Some(o) = &self.open {
            o.bytes.fetch_add(bytes, Ordering::Relaxed);
            o.files.fetch_add(files, Ordering::Relaxed);
        }
    }

    /// Ends it now.
    pub fn end(self) {}

    /// A phase that times nothing (no run being timed).
    pub fn none() -> Phase {
        Phase { open: None, t: Instant::now(), cpu: None }
    }
}

/// Opens a phase: a top-level one when none is open on this thread, else a sub-phase of the one
/// that is; inside a sub-phase, nothing (it's folded into that one: one level only). So a library's
/// own phases (the records read and saved, `crate::out::Out`) nest under the step's.
pub fn phase(name: &str) -> Phase {
    open(name)
}

/// `phase`, where the call site means a sub-phase (read as one).
pub fn sub(name: &str) -> Phase {
    open(name)
}

fn open(name: &str) -> Phase {
    let t_in = Instant::now();
    let mut g = run().lock().unwrap();
    let Some(r) = g.as_mut().filter(|r| !r.finished) else {
        return Phase::none();
    };
    let me = std::thread::current().id();
    // (Its parent: the phase open on this thread; in a sub-phase, it's folded into that one.)
    let inner = STACK.with(|s| s.borrow().last().map(|o| o.idx));
    // (Nor under one of its own name: a library's phase inside the step's that already says it,
    // the records saved within "saving records".)
    if inner.is_some_and(|i| r.accs[i].parent.is_some() || r.accs[i].name == name) {
        return Phase::none();
    }
    let parent = inner;
    // (The main thread's time since its last top-level phase ended: a gap, named by its ends.)
    if parent.is_none() && me == r.main {
        // (Said in the job's counts file for the agent to show live: store::nasio::set_phase.)
        store::nasio::set_phase(name);
        let ns = r.mark.0.elapsed().as_nanos() as u64;
        let after = std::mem::take(&mut r.mark.1);
        add_gap(&mut r.gaps, &after, name, ns);
        r.mark.1 = after;
    }
    let idx = match r.accs.iter().position(|a| a.name == name && a.parent == parent) {
        Some(i) => i,
        None => {
            r.accs.push(Acc {
                name: name.to_string(),
                parent,
                waits: Waits::default(),
                wall_ns: 0,
                cpu_us: None,
                n: 0,
                bytes: 0,
                files: 0,
                overlapped: false,
                background: me != r.main,
                peak: None,
                kids: Vec::new(),
            });
            r.accs.len() - 1
        }
    };
    let o = Arc::new(Open {
        idx,
        bytes: AtomicU64::new(0),
        files: AtomicU64::new(0),
        overlapped: AtomicBool::new(false),
        peak: AtomicU64::new(LAST_MEM.load(Ordering::Relaxed)),
        thread: me,
        children: Mutex::new(Vec::new()),
        t0: Instant::now(),
        cpu0: cpu_us(),
        w0: waits_now(),
    });
    // (Another thread's phase open now, or opening while this one is: both overlap.)
    let mut over = false;
    for other in r.open.iter().filter(|x| x.thread != me) {
        other.overlapped.store(true, Ordering::Relaxed);
        over = true;
    }
    if over {
        o.overlapped.store(true, Ordering::Relaxed);
    }
    r.open.push(o.clone());
    STACK.with(|s| s.borrow_mut().push(o.clone()));
    let cpu = cpu_us();
    r.overhead_ns += t_in.elapsed().as_nanos() as u64;
    Phase { open: Some(o), t: Instant::now(), cpu }
}

impl Drop for Phase {
    fn drop(&mut self) {
        let Some(o) = self.open.take() else { return };
        let wall = self.t.elapsed();
        let t_in = Instant::now();
        let waits = Waits::between(&o.w0, &waits_now());
        let cpu = match (self.cpu, cpu_us()) {
            (Some(a), Some(b)) => Some(b.saturating_sub(a)),
            _ => None,
        };
        STACK.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(i) = s.iter().rposition(|x| Arc::ptr_eq(x, &o)) {
                s.remove(i);
            }
        });
        // Its child programs' phases (read before the lock: files).
        let kids: Vec<PhaseRec> =
            o.children.lock().map(|c| c.iter().flat_map(|p| take_child(p)).collect()).unwrap_or_default();
        let mut g = run().lock().unwrap();
        let Some(r) = g.as_mut() else { return };
        if let Some(i) = r.open.iter().position(|x| Arc::ptr_eq(x, &o)) {
            r.open.remove(i);
        }
        if r.finished {
            return;
        }
        let parent = r.accs[o.idx].parent;
        if parent.is_none() && o.thread == r.main {
            store::nasio::set_phase("");
            r.mark = (Instant::now(), r.accs[o.idx].name.clone());
        }
        let a = &mut r.accs[o.idx];
        a.wall_ns += wall.as_nanos() as u64;
        a.waits.add(&waits);
        a.cpu_us = match (a.cpu_us, cpu) {
            (Some(x), Some(y)) => Some(x + y),
            (None, y) if a.n == 0 => y,
            (x, _) => x,
        };
        a.n += 1;
        a.bytes += o.bytes.load(Ordering::Relaxed);
        a.files += o.files.load(Ordering::Relaxed);
        a.overlapped |= o.overlapped.load(Ordering::Relaxed);
        o.peak.fetch_max(LAST_MEM.load(Ordering::Relaxed), Ordering::Relaxed);
        a.peak = fold_peak(a.peak, o.peak.load(Ordering::Relaxed));
        // (A sub-phase's child's phases go to its parent's: one level.)
        if !kids.is_empty() {
            let to = parent.unwrap_or(o.idx);
            for k in kids {
                merge_into(&mut r.accs[to].kids, k);
            }
        }
        r.overhead_ns += t_in.elapsed().as_nanos() as u64;
    }
}

/// Adds `k` to `list`, into the phase of its name there.
fn merge_into(list: &mut Vec<PhaseRec>, k: PhaseRec) {
    match list.iter_mut().find(|p| p.name == k.name) {
        Some(p) => {
            p.wall_s += k.wall_s;
            add_waits(&mut p.waits, k.waits);
            p.cpu_s = match (p.cpu_s, k.cpu_s) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            };
            p.n += k.n;
            p.bytes += k.bytes;
            p.files += k.files;
            p.overlapped |= k.overlapped;
            p.peak_mb = p.peak_mb.max(k.peak_mb);
        }
        None => list.push(PhaseRec { sub: Vec::new(), background: false, ..k }),
    }
}

/// Adds `bytes` and `files` to the innermost phase open on this thread (none: nothing).
pub fn count(bytes: u64, files: u64) {
    STACK.with(|s| {
        if let Some(o) = s.borrow().last() {
            o.bytes.fetch_add(bytes, Ordering::Relaxed);
            o.files.fetch_add(files, Ordering::Relaxed);
        }
    });
}

/// Has `c`, a child program about to run within this thread's innermost phase, write its phases
/// for that phase (`SCENIC_PHASES_TO`): they come in as the phase's sub-phases when it ends. No
/// phase open (or no run timed): nothing.
pub fn child(c: &mut std::process::Command) {
    static N: AtomicU64 = AtomicU64::new(0);
    let Some(o) = STACK.with(|s| s.borrow().last().cloned()) else {
        return;
    };
    let p = std::env::temp_dir().join(format!(
        "scenic-phases-{}-{}.json",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::remove_file(&p).ok();
    // (Not the job's record: the child's phases are the phase's.)
    c.env(PHASES_TO_ENV, &p).env_remove(TIMINGS_ENV);
    if let Ok(mut v) = o.children.lock() {
        v.push(p);
    };
}

/// A child's phases from its file (gone after), top-level only (its sub-phases are in them).
fn take_child(p: &Path) -> Vec<PhaseRec> {
    let Ok(b) = std::fs::read(p) else {
        return Vec::new();
    };
    std::fs::remove_file(p).ok();
    let r: Option<RunRec> = serde_json::from_slice(&b).ok();
    r.map(|r| r.phases.into_iter().map(|p| PhaseRec { sub: Vec::new(), ..p }).collect()).unwrap_or_default()
}

/// The run so far as a record (`ok` as given).
pub fn snapshot(ok: bool) -> Option<RunRec> {
    let g = run().lock().unwrap();
    let r = g.as_ref()?;
    Some(record_of(r, ok))
}

fn record_of(r: &Run, ok: bool) -> RunRec {
    // (Spans still open, a phase held past the run's end or a thread's still going: counted to now.)
    let mut accs = r.accs.clone();
    let now_cpu = cpu_us();
    for o in &r.open {
        let a = &mut accs[o.idx];
        a.wall_ns += o.t0.elapsed().as_nanos() as u64;
        a.waits.add(&Waits::between(&o.w0, &waits_now()));
        if let (Some(c0), Some(c1)) = (o.cpu0, now_cpu) {
            a.cpu_us = Some(a.cpu_us.unwrap_or(0) + c1.saturating_sub(c0));
        }
        a.n += 1;
        a.bytes += o.bytes.load(Ordering::Relaxed);
        a.files += o.files.load(Ordering::Relaxed);
        a.overlapped |= o.overlapped.load(Ordering::Relaxed);
        a.peak = fold_peak(a.peak, o.peak.load(Ordering::Relaxed));
    }
    let rec_of = |i: usize| {
        let a = &accs[i];
        let mut sub: Vec<PhaseRec> = r
            .accs
            .iter()
            .filter(|s| s.parent == Some(i) && s.n > 0)
            .map(|s| PhaseRec {
                name: s.name.clone(),
                class: String::new(),
                wall_s: s.wall_ns as f64 / 1e9,
                waits: Some(s.waits),
                cpu_s: s.cpu_us.map(|c| c as f64 / 1e6),
                n: s.n,
                bytes: s.bytes,
                files: s.files,
                overlapped: s.overlapped,
                background: s.background,
                peak_mb: mb(s.peak),
                sub: Vec::new(),
            })
            .collect();
        // (Its child programs' waits taken in beside its own: it waited on the child meanwhile. Their
        // main thread's phases' alone: one beside them spans the same wall time. Measured only.)
        let wall_s = a.wall_ns as f64 / 1e9;
        let mut waits = a.waits;
        for k in &a.kids {
            if let Some(w) = k.waits.filter(|_| !k.background) {
                waits.add(&w);
            }
            merge_into(&mut sub, k.clone());
        }
        PhaseRec {
            name: a.name.clone(),
            class: String::new(),
            wall_s,
            waits: Some(waits.within(wall_s)),
            cpu_s: a.cpu_us.map(|c| c as f64 / 1e6),
            n: a.n,
            bytes: a.bytes,
            files: a.files,
            overlapped: a.overlapped,
            background: a.background,
            peak_mb: mb(a.peak),
            sub,
        }
    };
    let phases: Vec<PhaseRec> =
        (0..accs.len()).filter(|&i| accs[i].parent.is_none() && accs[i].n > 0).map(rec_of).collect();
    let wall_s = r.t0.elapsed().as_secs_f64();
    let cpu_s = match (r.cpu0, cpu_us()) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a) as f64 / 1e6),
        _ => None,
    };
    let mut rec = RunRec {
        v: VERSION,
        kind: r.kind.clone(),
        id: std::env::var(JOB_ID_ENV).unwrap_or_default(),
        host: String::new(),
        start: r.start_wall,
        wall_s,
        cpu_s,
        threads: std::env::var("RAYON_NUM_THREADS").ok().and_then(|t| t.parse().ok()),
        ok,
        untimed_s: 0.0,
        overhead_s: r.overhead_ns as f64 / 1e9,
        phases,
        gaps: Vec::new(),
        safe_points: 0,
        safe_gap_s: None,
    };
    let sg = crate::control::safe_gaps();
    rec.safe_points = sg.points;
    rec.safe_gap_s = Some((sg.longest * 10.0).round() / 10.0);
    rec.untimed_s = (wall_s - rec.timed_s()).max(0.0);
    // (Its gaps, the one since the main thread's last phase too where none is open on it now.)
    let mut gaps = r.gaps.clone();
    if !r.open.iter().any(|o| o.thread == r.main && r.accs[o.idx].parent.is_none()) {
        add_gap(&mut gaps, &r.mark.1, END, r.mark.0.elapsed().as_nanos() as u64);
    }
    gaps.sort_by(|a, b| b.2.cmp(&a.2));
    gaps.truncate(GAPS_KEPT);
    rec.gaps =
        gaps.into_iter().map(|(after, before, ns, n)| Gap { after, before, wall_s: ns as f64 / 1e9, n }).collect();
    rec
}

/// "between A and B" (or "before B", "after A" at the run's ends).
fn gap_name(g: &Gap) -> String {
    match (g.after.as_str(), g.before.as_str()) {
        (START, END) => "with no phase at all".into(),
        (START, b) => format!("before {b}"),
        (a, END) => format!("after {a}"),
        (a, b) => format!("between {a} and {b}"),
    }
}

/// Ends the run: its table in the log, its record where `SCENIC_TIMINGS` (a job's) or
/// `SCENIC_PHASES_TO` (a child's) says. Once; a run never started: nothing.
pub fn finish(ok: bool) {
    let rec = {
        let mut g = run().lock().unwrap();
        let Some(r) = g.as_mut().filter(|r| !r.finished) else {
            return;
        };
        let rec = record_of(r, ok);
        r.finished = true;
        rec
    };
    let child = std::env::var_os(PHASES_TO_ENV);
    // (A child's table: its parent's log has it, under the phase that ran it.)
    if child.is_none() {
        eprint!("{}", table(&rec));
        if let Some(g) = rec.safe_gap_s {
            eprintln!("timings: {} safe points, the longest without one {}", rec.safe_points, secs(g));
        }
    }
    let line = serde_json::to_vec(&rec).unwrap_or_default();
    for p in [std::env::var_os(TIMINGS_ENV), child].into_iter().flatten() {
        if let Err(e) = write_whole(Path::new(&p), &line) {
            eprintln!("timings: not kept ({}: {e})", Path::new(&p).display());
        }
    }
}

fn write_whole(p: &Path, b: &[u8]) -> std::io::Result<()> {
    let tmp = p.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, b)?;
    std::fs::rename(&tmp, p)
}

/// Runs `f` as the job of `kind`, timed: its record kept however it ends (an error's too).
pub fn job<T>(kind: &str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    start(kind);
    let r = f();
    finish(r.is_ok());
    r
}

/// A record as another worker sent it (a page's, a helper's): None where it doesn't read, so a
/// done is never refused for its timings.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<RunRec>, D::Error> {
    let v: Option<serde_json::Value> = Option::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

/// Seconds, readably: "412 ms", "9.1 s", "12 min 3 s", "2 h 4 min".
pub fn secs(s: f64) -> String {
    if s < 1.0 {
        format!("{:.0} ms", s * 1000.0)
    } else if s < 60.0 {
        format!("{s:.1} s")
    } else if s < 3600.0 {
        format!("{} min {} s", (s / 60.0) as u64, (s % 60.0) as u64)
    } else {
        format!("{} h {} min", (s / 3600.0) as u64, (s % 3600.0 / 60.0) as u64)
    }
}

/// Bytes, readably.
pub fn bytes(b: u64) -> String {
    match b {
        0..1_000_000 => format!("{:.0} kB", b as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        _ => format!("{:.2} GB", b as f64 / 1e9),
    }
}

/// The run's table, as the log ends with it.
pub fn table(r: &RunRec) -> String {
    let mut s = format!(
        "timings: {} {} in {} (CPU {}), {}\n",
        r.kind,
        if r.ok { "done" } else { "ended" },
        secs(r.wall_s),
        r.cpu_s.map(secs).unwrap_or_else(|| "?".into()),
        if r.ok { "ok" } else { "failed" }
    );
    let line = |s: &mut String, indent: &str, p: &PhaseRec| {
        let share = if r.wall_s > 0.0 { p.wall_s / r.wall_s * 100.0 } else { 0.0 };
        let mut extra = Vec::new();
        if let Some(c) = p.cpu_s {
            extra.push(format!("CPU {}{}", secs(c), if p.overlapped { " ~" } else { "" }));
        }
        if p.n > 1 {
            extra.push(format!("×{}", p.n));
        }
        if p.bytes > 0 {
            extra.push(bytes(p.bytes));
            // (A transfer's rate over its time waiting on the network.)
            if let Some(w) = Waits::of(p).filter(|w| w.net_s > 0.0 && w.net_s >= w.nas_s) {
                extra.push(format!("{}/s", bytes((p.bytes as f64 / w.net_s) as u64)));
            }
        }
        if p.files > 0 {
            extra.push(format!("{} files", p.files));
        }
        if let Some(m) = p.peak_mb {
            extra.push(format!("peak {}", bytes(m << 20)));
        }
        if p.background {
            extra.push("beside".into());
        }
        s.push_str(&format!(
            "timings: {indent}{:<40} {:>10} {:>4.0}%  {:<34} {}\n",
            p.name,
            secs(p.wall_s),
            share,
            p.waits.map(|w| w.text(p.wall_s)).unwrap_or_else(|| p.class.clone()),
            extra.join(", ")
        ));
    };
    for p in &r.phases {
        line(&mut s, "", p);
        for q in &p.sub {
            line(&mut s, "  ", q);
        }
    }
    let share = if r.wall_s > 0.0 { r.untimed_s / r.wall_s * 100.0 } else { 0.0 };
    s.push_str(&format!("timings: {:<40} {:>10} {:>4.0}%\n", "(unaccounted)", secs(r.untimed_s), share));
    for g in r.gaps.iter().take(3) {
        s.push_str(&format!("timings:   {:<38} {:>10}\n", gap_name(g), secs(g.wall_s)));
    }
    if share > UNACCOUNTED_MAX * 100.0 {
        s.push_str(&format!(
            "timings: ! {share:.0}% of the run in no phase (over {:.0}%): the gaps above want phases\n",
            UNACCOUNTED_MAX * 100.0
        ));
    }
    s
}

/// A timings log (an agent's `timings.jsonl`, a coordinator's `coord/timings.jsonl`) past this is
/// cut to its newer half.
pub const LOG_MAX: u64 = 16 << 20;

/// Appends `rec` to the log at `p` (a line), cutting the log to its newer half past `LOG_MAX`.
pub fn append(p: &Path, rec: &RunRec) -> std::io::Result<()> {
    use std::io::Write;
    // (One writer at a time in this process: the agent's loop, the coordinator's answers.)
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut line = serde_json::to_vec(rec).map_err(std::io::Error::other)?;
    line.push(b'\n');
    std::fs::OpenOptions::new().create(true).append(true).open(p)?.write_all(&line)?;
    if std::fs::metadata(p)?.len() > LOG_MAX {
        let b = std::fs::read(p)?;
        let cut = b.len() / 2;
        let from = b[cut..].iter().position(|&c| c == b'\n').map_or(b.len(), |i| cut + i + 1);
        write_whole(p, &b[from..])?;
    }
    Ok(())
}

/// The runs in the log at `p`, oldest first (lines that don't read skipped).
pub fn read_log(p: &Path) -> Vec<RunRec> {
    let Ok(s) = std::fs::read_to_string(p) else {
        return Vec::new();
    };
    s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Takes the record a job left at `p` (`SCENIC_TIMINGS`), the file gone after; `host` stamped on it.
pub fn take_record(p: &Path, host: &str) -> Option<RunRec> {
    let b = std::fs::read(p).ok()?;
    std::fs::remove_file(p).ok();
    let mut r: RunRec = serde_json::from_slice(&b).ok()?;
    if r.host.is_empty() {
        r.host = host.to_string();
    }
    Some(r)
}

/// A phase's totals over runs (`summary`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct PhaseSum {
    pub name: String,
    /// Its waits summed over the runs that measured them (`Waits::of`: an older run's by its class),
    /// and the wall time of those runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waits: Option<Waits>,
    pub waits_wall_s: f64,
    /// The runs it was in.
    pub runs: usize,
    pub wall_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_s: Option<f64>,
    pub n: u64,
    pub bytes: u64,
    pub files: u64,
    /// In some run it overlapped another thread's work: its CPU is approximate.
    pub overlapped: bool,
    pub background: bool,
    /// Its share of the runs' wall time.
    pub share: f64,
    /// The most memory the job held while it was open, over the runs (MB; `PhaseRec::peak_mb`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_mb: Option<u64>,
    pub sub: Vec<PhaseSum>,
}

/// A kind's runs summed (`scenic timings`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct KindSum {
    pub kind: String,
    pub runs: usize,
    pub failed: usize,
    pub hosts: Vec<String>,
    pub wall_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_s: Option<f64>,
    pub untimed_s: f64,
    pub overhead_s: f64,
    pub phases: Vec<PhaseSum>,
    /// The runs' gaps summed by their ends, largest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<Gap>,
}

impl KindSum {
    /// The share of its runs' time in no phase.
    pub fn unaccounted(&self) -> f64 {
        if self.wall_s > 0.0 {
            self.untimed_s / self.wall_s
        } else {
            0.0
        }
    }
}

/// The kinds whose runs leave more than `UNACCOUNTED_MAX` of their time in no phase.
pub fn blind_spots(sums: &[KindSum]) -> Vec<&KindSum> {
    sums.iter().filter(|k| k.unaccounted() > UNACCOUNTED_MAX).collect()
}

/// Where each kind's memory peaks (`scenic pool needs --fit`): over its last `last` runs, its
/// top-level phase that held the most (`PhaseRec::peak_mb`), with its sub-phase (a child program's
/// own phase) that did where one says; by kind. Kinds whose runs carry no peaks are left out.
pub fn peak_holders(runs: &[RunRec], last: usize) -> Vec<PeakHolder> {
    summary(runs, None, last, None)
        .into_iter()
        .filter_map(|k| {
            let top = k.phases.iter().filter(|p| p.peak_mb.is_some()).max_by_key(|p| p.peak_mb)?;
            let sub = top.sub.iter().filter(|p| p.peak_mb.is_some()).max_by_key(|p| p.peak_mb);
            Some(PeakHolder {
                kind: k.kind.clone(),
                runs: k.runs,
                phase: top.name.clone(),
                peak_mb: top.peak_mb.unwrap_or(0),
                sub: sub.map(|s| (s.name.clone(), s.peak_mb.unwrap_or(0))),
            })
        })
        .collect()
}

/// A kind's heaviest phase (`peak_holders`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PeakHolder {
    pub kind: String,
    pub runs: usize,
    pub phase: String,
    pub peak_mb: u64,
    pub sub: Option<(String, u64)>,
}

/// `peak_holders`, readably: a line a kind, heaviest first.
pub fn peak_text(holders: &[PeakHolder]) -> String {
    let mut h: Vec<&PeakHolder> = holders.iter().collect();
    h.sort_by(|a, b| b.peak_mb.cmp(&a.peak_mb).then(a.kind.cmp(&b.kind)));
    let mut out = String::new();
    for p in h {
        out.push_str(&format!(
            "  {:<16} {:>9}  {}{} (of {} run{})\n",
            p.kind,
            bytes(p.peak_mb << 20),
            p.phase,
            p.sub.as_ref().map(|(n, m)| format!("; within it {n} {}", bytes(m << 20))).unwrap_or_default(),
            p.runs,
            if p.runs == 1 { "" } else { "s" }
        ));
    }
    out
}

fn add_phase(list: &mut Vec<PhaseSum>, p: &PhaseRec, total: f64, top: bool) {
    let i = match list.iter().position(|q| q.name == p.name) {
        Some(i) => i,
        None => {
            list.push(PhaseSum { name: p.name.clone(), ..Default::default() });
            list.len() - 1
        }
    };
    let q = &mut list[i];
    q.runs += 1;
    q.wall_s += p.wall_s;
    if let Some(w) = Waits::of(p) {
        add_waits(&mut q.waits, Some(w));
        q.waits_wall_s += p.wall_s;
    }
    q.cpu_s = match (q.cpu_s, p.cpu_s) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
    q.n += p.n;
    q.bytes += p.bytes;
    q.files += p.files;
    q.overlapped |= p.overlapped;
    q.background |= p.background;
    q.peak_mb = q.peak_mb.max(p.peak_mb);
    if top {
        for s in &p.sub {
            add_phase(&mut q.sub, s, total, false);
        }
    }
}

fn shares(list: &mut [PhaseSum], total: f64) {
    for q in list {
        q.share = if total > 0.0 { q.wall_s / total } else { 0.0 };
        shares(&mut q.sub, total);
    }
}

/// The runs of each kind (or of `kind`), their last `last` each (oldest first in `runs`), and of
/// `host` alone when given, summed: each phase's totals and share.
pub fn summary(runs: &[RunRec], kind: Option<&str>, last: usize, host: Option<&str>) -> Vec<KindSum> {
    let mut kinds: Vec<&str> = runs.iter().map(|r| r.kind.as_str()).filter(|k| kind.is_none_or(|w| w == *k)).collect();
    kinds.sort();
    kinds.dedup();
    kinds
        .into_iter()
        .map(|k| {
            let mut mine: Vec<&RunRec> =
                runs.iter().filter(|r| r.kind == k && host.is_none_or(|h| h == r.host)).collect();
            let skip = mine.len().saturating_sub(last);
            mine.drain(..skip);
            let mut s = KindSum { kind: k.to_string(), runs: mine.len(), ..Default::default() };
            for r in &mine {
                s.failed += !r.ok as usize;
                if !s.hosts.contains(&r.host) {
                    s.hosts.push(r.host.clone());
                }
                s.wall_s += r.wall_s;
                s.cpu_s = match (s.cpu_s, r.cpu_s) {
                    (Some(a), Some(b)) => Some(a + b),
                    (a, b) => a.or(b),
                };
                s.untimed_s += r.untimed_s;
                s.overhead_s += r.overhead_s;
                for p in &r.phases {
                    add_phase(&mut s.phases, p, r.wall_s, true);
                }
                for g in &r.gaps {
                    match s.gaps.iter_mut().find(|h| h.after == g.after && h.before == g.before) {
                        Some(h) => {
                            h.wall_s += g.wall_s;
                            h.n += g.n;
                        }
                        None => s.gaps.push(g.clone()),
                    }
                }
            }
            s.gaps.sort_by(|a, b| b.wall_s.total_cmp(&a.wall_s));
            let total = s.wall_s;
            shares(&mut s.phases, total);
            s
        })
        .filter(|s| s.runs > 0)
        .collect()
}

/// How many of a step's last runs its phase shares are the mean of (`PhaseShares`).
pub const SHARES_RUNS: usize = 10;

/// A step's phases as shares of its runs, the mean over its last `SHARES_RUNS` runs that ended
/// well (docs/pool.md §10, the steps): the lead's, published in its status, so every page shows the
/// same measured split.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseShares {
    /// The runs it's the mean of.
    pub runs: usize,
    /// Each top-level phase of its main thread, by its mean share of wall time, largest first.
    pub phases: Vec<PhaseShare>,
}

/// A phase's mean shares of its step's runs (`PhaseShares`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseShare {
    pub name: String,
    /// Its mean share of a run's wall time (a run without it: 0; the rest of a run is in no phase).
    pub wall: f64,
    /// Its mean share of the bytes a run's phases moved (over the runs that moved any; 0 if none).
    pub bytes: f64,
}

/// The step a run of `kind` is a job of (crate::agent::steps' id): a leased task's runs are timed
/// as "task <its step>" (`scenic task`), every other job under its step's id.
pub fn step_of(kind: &str) -> &str {
    kind.strip_prefix("task ").unwrap_or(kind)
}

/// The phase shares of each step's runs in `runs` (oldest first; by `step_of`): its last
/// `SHARES_RUNS` that ended well.
pub fn phase_shares<'a>(runs: impl IntoIterator<Item = &'a RunRec>) -> std::collections::BTreeMap<String, PhaseShares> {
    let mut by: std::collections::BTreeMap<&str, Vec<&RunRec>> = Default::default();
    for r in runs.into_iter().filter(|r| r.ok && r.wall_s > 0.0) {
        by.entry(step_of(&r.kind)).or_default().push(r);
    }
    by.into_iter()
        .map(|(k, mut rs)| {
            rs.drain(..rs.len().saturating_sub(SHARES_RUNS));
            let mut out: Vec<PhaseShare> = Vec::new();
            let mut moved = 0usize;
            for r in &rs {
                let top: Vec<&PhaseRec> = r.phases.iter().filter(|p| !p.background).collect();
                let bytes: u64 = top.iter().map(|p| p.bytes).sum();
                moved += (bytes > 0) as usize;
                for p in top {
                    let i = match out.iter().position(|q| q.name == p.name) {
                        Some(i) => i,
                        None => {
                            out.push(PhaseShare { name: p.name.clone(), ..Default::default() });
                            out.len() - 1
                        }
                    };
                    out[i].wall += p.wall_s / r.wall_s;
                    if bytes > 0 {
                        out[i].bytes += p.bytes as f64 / bytes as f64;
                    }
                }
            }
            for q in &mut out {
                q.wall /= rs.len() as f64;
                q.bytes = if moved > 0 { q.bytes / moved as f64 } else { 0.0 };
            }
            out.sort_by(|a, b| b.wall.total_cmp(&a.wall));
            (k.to_string(), PhaseShares { runs: rs.len(), phases: out })
        })
        .collect()
}

/// A timings log read as it grows (`PhaseShares` on the lead): only what was appended since the
/// last read, each step's last `SHARES_RUNS` runs that ended well kept; read whole again when the
/// log is cut (`append` writes its newer half as a new file) or replaced: another file (inode) at
/// its path, or a shorter one.
#[cfg(unix)]
#[derive(Default)]
pub struct LogTail {
    at: u64,
    file: u64,
    runs: std::collections::BTreeMap<String, std::collections::VecDeque<RunRec>>,
    /// Every run that ended within `TIMES_EDGES`' last edge of the newest one read, oldest first:
    /// failed runs too (`PhaseTimes`).
    recent: std::collections::VecDeque<RunSum>,
}

#[cfg(unix)]
impl LogTail {
    /// Reads what the log at `p` gained since the last read (its size taken first, read-only; a
    /// partly written last line left for the next read). True when anything new was read.
    pub fn read(&mut self, p: &Path) -> bool {
        use std::io::{Read, Seek};
        use std::os::unix::fs::MetadataExt;
        // (Its size taken before reading: what's appended meanwhile waits for the next read.)
        let Ok((len, file)) = std::fs::metadata(p).map(|m| (m.len(), m.ino())) else {
            return false;
        };
        if len < self.at || file != self.file {
            *self = Self { file, ..Self::default() };
        }
        if len == self.at {
            return false;
        }
        let Ok(mut f) = std::fs::File::open(p) else {
            return false;
        };
        let mut b = Vec::new();
        if f.seek(std::io::SeekFrom::Start(self.at)).is_err() || f.take(len - self.at).read_to_end(&mut b).is_err() {
            return false;
        }
        let Some(end) = b.iter().rposition(|&c| c == b'\n') else {
            return false;
        };
        self.at += end as u64 + 1;
        for l in b[..end].split(|&c| c == b'\n') {
            let Ok(r) = serde_json::from_slice::<RunRec>(l) else { continue };
            if r.wall_s <= 0.0 {
                continue;
            }
            self.recent.push_back(RunSum::of(&r));
            if !r.ok {
                continue;
            }
            let q = self.runs.entry(step_of(&r.kind).to_string()).or_default();
            q.push_back(r);
            if q.len() > SHARES_RUNS {
                q.pop_front();
            }
        }
        true
    }

    /// Each step's phase shares over the runs kept.
    pub fn shares(&self) -> std::collections::BTreeMap<String, PhaseShares> {
        phase_shares(self.runs.values().flatten())
    }

    /// Each step's time by phase over the runs that ended within each of `TIMES_EDGES` of `now`
    /// (runs older than the last edge let go).
    pub fn times(&mut self, now: u64) -> PhaseTimes {
        let last = TIMES_EDGES[TIMES_EDGES.len() - 1];
        while self.recent.front().is_some_and(|r| r.end + last < now) {
            self.recent.pop_front();
        }
        phase_times(self.recent.iter(), now)
    }
}

/// The page's chart ranges (the build page's footer, "Charts show the last": web/work/index.html's
/// `RANGES`), in seconds: `PhaseTimes` sums a step's runs by the first of these its end is within.
pub const TIMES_EDGES: [u64; 10] = [60, 300, 600, 1800, 3600, 7200, 21600, 86400, 259200, 604800];

/// The most phases of a step `PhaseTimes` names; the rest are summed as `OTHER_PHASES`.
pub const TIMES_PHASES: usize = 8;

/// The phases past a step's `TIMES_PHASES` largest, summed.
pub const OTHER_PHASES: &str = "other phases";

/// A run as `PhaseTimes` sums it: its step, end, wall and CPU, and its main thread's top-level
/// phases.
#[derive(Clone, Debug)]
pub struct RunSum {
    step: String,
    end: u64,
    ok: bool,
    wall: f64,
    cpu: f64,
    phases: Vec<PhaseSumOf>,
}

/// A top-level phase of a run in `RunSum`: its name, its declared class (an unmeasured record's;
/// empty since task #870), wall and CPU seconds, NAS and internet bytes, and its measured waits.
#[derive(Clone, Debug)]
struct PhaseSumOf {
    name: String,
    class: String,
    wall: f64,
    cpu: f64,
    nas: u64,
    net: u64,
    waits: Option<Waits>,
}

impl RunSum {
    pub fn of(r: &RunRec) -> RunSum {
        // (A measured phase's bytes are the job's NAS and network bytes while it was open. An older
        // record's go by class: a phase's own counted bytes by its class, a sub-phase's by its own,
        // a compute or mixed one's by the nearest phase above it that reads the NAS or the
        // internet; bytes of the local disk, or of no such phase, aren't said.)
        fn moved(p: &PhaseRec, above: &str, nas: &mut u64, net: &mut u64) {
            let c = match p.class.as_str() {
                "nas-read" | "nas-write" | "net" | "disk" => p.class.as_str(),
                _ => above,
            };
            match c {
                "nas-read" | "nas-write" => *nas += p.bytes,
                "net" => *net += p.bytes,
                _ => {}
            }
            for k in &p.sub {
                moved(k, c, nas, net);
            }
        }
        let phases = r
            .phases
            .iter()
            .filter(|p| !p.background)
            .map(|p| {
                let (mut nas, mut net) = (0, 0);
                match p.waits {
                    Some(w) => (nas, net) = (w.nas_b, w.net_b),
                    None => moved(p, "compute", &mut nas, &mut net),
                }
                PhaseSumOf {
                    name: p.name.clone(),
                    class: p.class.clone(),
                    wall: p.wall_s,
                    cpu: p.cpu_s.unwrap_or(0.0),
                    nas,
                    net,
                    waits: p.waits,
                }
            })
            .collect();
        RunSum {
            step: step_of(&r.kind).to_string(),
            end: r.start + r.wall_s.round() as u64,
            ok: r.ok,
            wall: r.wall_s,
            cpu: r.cpu_s.unwrap_or(0.0),
            phases,
        }
    }
}

/// Each step's time by phase over the runs that ended within each of the page's chart ranges
/// (`TIMES_EDGES`): the lead's, in its status as `phase_times` (docs/pool.md §10, Time by phase),
/// so the build page breaks a step's time down over the range its footer picks. Compact: a step's
/// runs are summed by the first range their end is within (`b`), the page adding up those within
/// its own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseTimes {
    /// When (seconds since the epoch) the ranges end.
    pub at: u64,
    pub edges: Vec<u64>,
    pub steps: std::collections::BTreeMap<String, StepTimes>,
}

/// A step's runs in `PhaseTimes`: its phases named once (`[name, class]`, the largest over all
/// its runs first; at most `TIMES_PHASES`, then `OTHER_PHASES`; the class only an unmeasured
/// record's, else empty), and a sum per range its runs ended within: `[edge, runs, failed, wall s,
/// CPU s, [[phase, wall s, CPU s, NAS MB, internet MB], …]]`, `edge` an index into `edges`, `phase`
/// into `phases`; whole seconds, MB to a tenth. A run's time in no phase is its wall less its
/// phases'. Beside them, its phases' measured waits (task #870) per range: `w`, `[edge, phase,
/// measured wall s, NAS s, network s, disk s, other work s]` over the runs that measured them (to
/// a tenth of a second); the rest of a phase's measured wall computing, its unmeasured wall (older
/// runs') shown by its class.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StepTimes {
    pub phases: Vec<(String, String)>,
    pub b: Vec<(usize, u32, u32, u64, u64, Vec<(usize, u64, u64, f64, f64)>)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub w: Vec<(usize, usize, f64, f64, f64, f64, f64)>,
}

/// `PhaseTimes` of `runs` at `now`; a run ended past the last edge isn't counted.
pub fn phase_times<'a>(runs: impl IntoIterator<Item = &'a RunSum>, now: u64) -> PhaseTimes {
    use std::collections::BTreeMap;
    // (Step → its edge's sums: runs, failed, wall, CPU, by phase: wall, CPU, NAS, internet.)
    // (And by phase: its measured wall and its waits.)
    type Sums = (u32, u32, f64, f64, BTreeMap<String, (f64, f64, u64, u64)>, BTreeMap<String, (f64, Waits)>);
    let mut by: BTreeMap<&str, (BTreeMap<String, (String, f64)>, BTreeMap<usize, Sums>)> = BTreeMap::new();
    for r in runs {
        let Some(e) = TIMES_EDGES.iter().position(|&e| r.end + e >= now) else { continue };
        let (all, edges) = by.entry(&r.step).or_default();
        let s = edges.entry(e).or_default();
        s.0 += 1;
        s.1 += !r.ok as u32;
        s.2 += r.wall;
        s.3 += r.cpu;
        for ph in &r.phases {
            let e = all.entry(ph.name.clone()).or_insert((ph.class.clone(), 0.0));
            if ph.waits.is_some() {
                // (Measured in some run: no class said.)
                e.0.clear();
            }
            e.1 += ph.wall;
            let p = s.4.entry(ph.name.clone()).or_default();
            p.0 += ph.wall;
            p.1 += ph.cpu;
            p.2 += ph.nas;
            p.3 += ph.net;
            if let Some(w) = ph.waits {
                let m = s.5.entry(ph.name.clone()).or_default();
                m.0 += ph.wall;
                m.1.add(&w);
            }
        }
    }
    let mb = |b: u64| (b as f64 / 1e5).round() / 10.0;
    let steps = by
        .into_iter()
        .map(|(step, (all, edges))| {
            let mut order: Vec<(String, String, f64)> = all.into_iter().map(|(n, (c, w))| (n, c, w)).collect();
            order.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
            let others = order.len() > TIMES_PHASES;
            let mut phases: Vec<(String, String)> = order
                .iter()
                .take(if others { TIMES_PHASES - 1 } else { TIMES_PHASES })
                .map(|(n, c, _)| (n.clone(), c.clone()))
                .collect();
            if others {
                phases.push((OTHER_PHASES.to_string(), String::new()));
            }
            let at = |name: &str| phases.iter().position(|(n, _)| n == name).unwrap_or(phases.len() - 1);
            let tenth = |v: f64| (v * 10.0).round() / 10.0;
            let mut w: Vec<(usize, usize, f64, f64, f64, f64, f64)> = Vec::new();
            for (e, s) in &edges {
                let mut sums: BTreeMap<usize, (f64, Waits)> = BTreeMap::new();
                for (name, (mw, ws)) in &s.5 {
                    let x = sums.entry(at(name)).or_default();
                    x.0 += mw;
                    x.1.add(ws);
                }
                for (i, (mw, ws)) in sums {
                    w.push((*e, i, tenth(mw), tenth(ws.nas_s), tenth(ws.net_s), tenth(ws.disk_s), tenth(ws.other_s)));
                }
            }
            let b = edges
                .into_iter()
                .map(|(e, (runs, failed, wall, cpu, ps, _))| {
                    let mut sums: Vec<(usize, f64, f64, u64, u64)> = Vec::new();
                    for (name, (w, c, nas, net)) in ps {
                        let i = at(&name);
                        match sums.iter_mut().find(|x| x.0 == i) {
                            Some(x) => {
                                x.1 += w;
                                x.2 += c;
                                x.3 += nas;
                                x.4 += net;
                            }
                            None => sums.push((i, w, c, nas, net)),
                        }
                    }
                    sums.sort_by_key(|x| x.0);
                    let ps = sums
                        .into_iter()
                        .map(|(i, w, c, nas, net)| (i, w.round() as u64, c.round() as u64, mb(nas), mb(net)))
                        .collect();
                    (e, runs, failed, wall.round() as u64, cpu.round() as u64, ps)
                })
                .collect();
            (step.to_string(), StepTimes { phases, b, w })
        })
        .collect();
    PhaseTimes { at: now, edges: TIMES_EDGES.to_vec(), steps }
}

/// The last `last` runs of each kind (or of `kind`), oldest first.
pub fn last_runs(runs: &[RunRec], kind: Option<&str>, last: usize) -> Vec<RunRec> {
    let mut seen: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut keep = vec![false; runs.len()];
    for (i, r) in runs.iter().enumerate().rev() {
        let n = seen.entry(r.kind.as_str()).or_default();
        if kind.is_none_or(|k| k == r.kind) && *n < last {
            *n += 1;
            keep[i] = true;
        }
    }
    runs.iter().zip(keep).filter(|(_, k)| *k).map(|(r, _)| r.clone()).collect()
}

/// `summary`, readably: a table per kind. CPU marked `~` is approximate (the phase overlapped
/// another thread's work).
pub fn summary_text(sums: &[KindSum]) -> String {
    let mut out = String::new();
    for k in sums {
        let n = k.runs.max(1) as f64;
        out.push_str(&format!(
            "{} — {} run{}{} on {}: {} in all, {} a run, CPU {}; untimed {:.1}%, timing's cost {:.3}%\n",
            k.kind,
            k.runs,
            if k.runs == 1 { "" } else { "s" },
            if k.failed > 0 { format!(" ({} not ok)", k.failed) } else { String::new() },
            k.hosts.join(", "),
            secs(k.wall_s),
            secs(k.wall_s / n),
            k.cpu_s.map(secs).unwrap_or_else(|| "?".into()),
            if k.wall_s > 0.0 { k.untimed_s / k.wall_s * 100.0 } else { 0.0 },
            if k.wall_s > 0.0 { k.overhead_s / k.wall_s * 100.0 } else { 0.0 },
        ));
        out.push_str(&format!(
            "  {:<44} {:>6} {:>11} {:>11} {:>7}  {:<34} {:>9}  {}\n",
            "phase", "share", "total", "a run", "CPU/wall", "waits", "peak", "moved"
        ));
        let row = |out: &mut String, indent: &str, p: &PhaseSum| {
            let ratio = match p.cpu_s {
                Some(c) if p.wall_s > 0.0 => {
                    format!("{}{:.1}", if p.overlapped { "~" } else { "" }, c / p.wall_s)
                }
                _ => "".into(),
            };
            let mut moved = Vec::new();
            if p.bytes > 0 {
                moved.push(bytes(p.bytes));
            }
            if p.files > 0 {
                moved.push(format!("{} files", p.files));
            }
            if p.background {
                moved.push("(beside)".into());
            }
            let name = format!("{indent}{}", p.name);
            out.push_str(&format!(
                "  {:<44} {:>5.1}% {:>11} {:>11} {:>7}  {:<34} {:>9}  {}\n",
                name,
                p.share * 100.0,
                secs(p.wall_s),
                secs(p.wall_s / n),
                ratio,
                p.waits.map(|w| w.text(p.waits_wall_s)).unwrap_or_default(),
                p.peak_mb.map(|m| bytes(m << 20)).unwrap_or_default(),
                moved.join(", ")
            ));
        };
        for p in &k.phases {
            row(&mut out, "", p);
            for q in &p.sub {
                row(&mut out, "  ", q);
            }
        }
        out.push_str(&format!(
            "  {:<44} {:>5.1}% {:>11} {:>11}\n",
            "(unaccounted)",
            k.unaccounted() * 100.0,
            secs(k.untimed_s),
            secs(k.untimed_s / n)
        ));
        for g in k.gaps.iter().take(3) {
            out.push_str(&format!("    {:<42} {:>6} {:>11}\n", gap_name(g), "", secs(g.wall_s)));
        }
        if k.unaccounted() > UNACCOUNTED_MAX {
            out.push_str(&format!(
                "  ! {:.0}% in no phase (over {:.0}%): its gaps want phases\n",
                k.unaccounted() * 100.0,
                UNACCOUNTED_MAX * 100.0
            ));
        }
        out.push('\n');
    }
    let blind = blind_spots(sums);
    if !blind.is_empty() {
        out.push_str(&format!(
            "Over {:.0}% of their time in no phase: {}\n",
            UNACCOUNTED_MAX * 100.0,
            blind.iter().map(|k| format!("{} {:.0}%", k.kind, k.unaccounted() * 100.0)).collect::<Vec<_>>().join(", ")
        ));
    }
    out
}

/// The phases a child program wrote (`SCENIC_PHASES_TO`'s file), for tests.
pub fn read_child(p: &Path) -> Vec<PhaseRec> {
    take_child(p)
}

/// Resets the run (tests: one process, many runs).
#[cfg(test)]
pub(crate) fn reset() {
    *run().lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share_run(kind: &str, ok: bool, wall: f64, phases: &[(&str, f64, u64)]) -> RunRec {
        RunRec {
            v: 1,
            kind: kind.into(),
            ok,
            wall_s: wall,
            phases: phases
                .iter()
                .map(|&(n, w, b)| PhaseRec { name: n.into(), wall_s: w, bytes: b, ..Default::default() })
                .collect(),
            ..Default::default()
        }
    }

    /// Task #844: a step's runs summed by the first chart range their end is within; failed runs
    /// counted; bytes by their phase's class (a compute phase's by the NAS phase above it); past a
    /// step's `TIMES_PHASES` largest phases, the rest summed as one; a leased task's runs its step's.
    #[test]
    fn phase_times_by_range() {
        let now = 1_000_000;
        let mut a = share_run("depth", true, 100.0, &[("reading", 60.0, 2_000_000), ("composing", 30.0, 0)]);
        a.phases[0].class = "nas-read".into();
        a.phases[0].cpu_s = Some(6.0);
        a.phases[0].sub =
            vec![PhaseRec { name: "decoding".into(), class: "compute".into(), bytes: 1_000_000, ..Default::default() }];
        a.start = now - 100 - 30; // (ended 30 s ago: the first range, a minute)
        let mut b = share_run("task depth", false, 50.0, &[("composing", 50.0, 0)]);
        b.start = now - 50 - 3000; // (50 min ago: within the hour)
        let mut old = share_run("depth", true, 10.0, &[("reading", 10.0, 0)]);
        old.start = now - 10 - 700_000; // (past a week: not counted)
        let many: Vec<(String, f64, u64)> =
            (0..TIMES_PHASES + 2).map(|i| (format!("p{i}"), 10.0 - i as f64, 0)).collect();
        let many: Vec<(&str, f64, u64)> = many.iter().map(|(n, w, b)| (n.as_str(), *w, *b)).collect();
        let mut c = share_run("slope", true, 60.0, &many);
        c.start = now - 60;
        // (Task #870: a measured run: its phases' waits beside, its NAS bytes measured, no class.)
        let mut e = share_run("depth", true, 100.0, &[("reading", 80.0, 0)]);
        e.phases[0].waits =
            Some(Waits { nas_s: 50.0, net_s: 4.04, disk_s: 1.0, nas_b: 5_000_000, ..Default::default() });
        e.start = now - 100 - 3000;
        let sums: Vec<RunSum> = [a, b, old, c, e].iter().map(RunSum::of).collect();
        let t = phase_times(&sums, now);
        assert_eq!(t.edges, TIMES_EDGES);
        let d = &t.steps["depth"];
        assert_eq!(d.phases, [("reading".to_string(), String::new()), ("composing".to_string(), String::new())]);
        assert_eq!(
            d.b,
            [
                (0, 1, 0, 100, 0, vec![(0, 60, 6, 3.0, 0.0), (1, 30, 0, 0.0, 0.0)]),
                (4, 2, 1, 150, 0, vec![(0, 80, 0, 5.0, 0.0), (1, 50, 0, 0.0, 0.0)])
            ]
        );
        assert_eq!(d.w, [(4, 0, 80.0, 50.0, 4.0, 1.0, 0.0)]);
        // (Only unmeasured runs: the class said.)
        let only_old = phase_times(
            &[RunSum::of(&{
                let mut r = share_run("x", true, 5.0, &[("reading", 5.0, 0)]);
                r.phases[0].class = "nas-read".into();
                r.start = now - 5;
                r
            })],
            now,
        );
        assert_eq!(only_old.steps["x"].phases, [("reading".to_string(), "nas-read".to_string())]);
        assert!(only_old.steps["x"].w.is_empty());
        let s = &t.steps["slope"];
        assert_eq!(s.phases.len(), TIMES_PHASES);
        assert_eq!(s.phases[TIMES_PHASES - 1].0, OTHER_PHASES);
        let other = s.b[0].5.iter().find(|p| p.0 == TIMES_PHASES - 1).unwrap();
        assert_eq!(other.1, (TIMES_PHASES - 1..TIMES_PHASES + 2).map(|i| 10 - i as u64).sum::<u64>());
        // (A log read: its runs' times, the old ones let go as the ranges move on.)
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("timings.jsonl");
        let mut r = share_run("pack", false, 5.0, &[("x", 5.0, 0)]);
        r.start = now - 5;
        append(&log, &r).unwrap();
        let mut tail = LogTail::default();
        assert!(tail.read(&log));
        assert_eq!(tail.times(now).steps["pack"].b[0].2, 1, "a failed run counts");
        assert!(tail.times(now + 700_000).steps.is_empty());
    }

    /// Task #844: a record from before the phases' renaming reads with their new names (gaps too).
    #[test]
    fn old_phase_names_read_as_new() {
        let l = r#"{"v":1,"kind":"depth","start":1,"wall_s":2,"ok":true,"untimed_s":0,"phases":[{"name":"depth sources' windows read","class":"nas-read","wall_s":1,"n":1}],"gaps":[{"after":"the start","before":"gaps filled","wall_s":1,"n":1}]}"#;
        let r: RunRec = serde_json::from_str(l).unwrap();
        assert_eq!(r.phases[0].name, "reading the depth sources' windows");
        assert_eq!(r.gaps[0].before, "filling gaps");
        assert_eq!(r.gaps[0].after, "the start");
    }

    #[test]
    fn phase_shares_mean_of_last_runs() {
        let mut runs = vec![share_run("t", true, 10.0, &[("read", 10.0, 100)])];
        // (Older than the last SHARES_RUNS: left out; a failed run too.)
        for _ in 0..SHARES_RUNS - 1 {
            runs.push(share_run("t", true, 10.0, &[("read", 2.0, 300), ("write", 6.0, 100)]));
        }
        runs.push(share_run("t", false, 10.0, &[("read", 10.0, 1)]));
        runs.push(share_run("t", true, 10.0, &[("write", 5.0, 0)]));
        let s = &phase_shares(&runs)["t"];
        assert_eq!(s.runs, SHARES_RUNS);
        assert_eq!(s.phases[0].name, "write");
        assert!((s.phases[0].wall - (0.6 * 9.0 + 0.5) / 10.0).abs() < 1e-9);
        assert!((s.phases[1].wall - 0.2 * 0.9).abs() < 1e-9);
        // (Bytes over the 9 runs that moved any: 3/4 read, 1/4 write.)
        assert!((s.phases[1].bytes - 0.75).abs() < 1e-9);
        assert!((s.phases[0].bytes - 0.25).abs() < 1e-9);
    }

    #[test]
    fn log_tail_reads_what_was_added() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("timings.jsonl");
        let mut t = LogTail::default();
        assert!(!t.read(&log), "no log");
        for i in 0..12 {
            append(&log, &share_run("a", true, 4.0, &[("x", i as f64 * 0.1, 0)])).unwrap();
        }
        assert!(t.read(&log));
        assert!(!t.read(&log), "nothing new");
        assert_eq!(t.shares()["a"].runs, SHARES_RUNS);
        // (A partly written line waits for its end.)
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        let line = serde_json::to_string(&share_run("b", true, 2.0, &[("y", 1.0, 5)])).unwrap();
        f.write_all(line[..10].as_bytes()).unwrap();
        t.read(&log);
        assert!(!t.shares().contains_key("b"));
        f.write_all(format!("{}\n", &line[10..]).as_bytes()).unwrap();
        assert!(t.read(&log));
        assert_eq!(t.shares()["b"].phases[0].wall, 0.5);
        // (Cut shorter: read whole again.)
        std::fs::write(&log, format!("{line}\n")).unwrap();
        assert!(t.read(&log));
        assert!(!t.shares().contains_key("a"));
        // (Replaced by a longer file, as a cut is written: read whole again, not from the old end.)
        let mut longer = String::new();
        for _ in 0..3 {
            longer += &format!("{}\n", serde_json::to_string(&share_run("c", true, 2.0, &[("z", 1.0, 0)])).unwrap());
        }
        write_whole(&log, longer.as_bytes()).unwrap();
        assert!(t.read(&log));
        assert!(!t.shares().contains_key("b"));
        assert_eq!(t.shares()["c"].runs, 3);
    }

    #[test]
    fn a_task_is_its_step() {
        // (`scenic task` times a leased task as "task <step>": its shares are its step's.)
        let s = phase_shares(&[
            share_run("task treeblock", true, 2.0, &[("y", 1.0, 0)]),
            share_run("trees", true, 2.0, &[]),
        ]);
        assert_eq!(s.keys().collect::<Vec<_>>(), ["treeblock", "trees"]);
    }

    // (One test: the run is the process's.)
    #[test]
    fn phases_add_up_nest_and_take_in_children() {
        // Run again in a process of its own, this test alone in it: in the runner's, the other
        // tests' phases (their code's, on their threads) would come into this run meanwhile.
        const ALONE: &str = "SCENIC_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let out = store::spawn::output(
                store::spawn::command(std::env::current_exe().unwrap())
                    .args(["--exact", "timings::tests::phases_add_up_nest_and_take_in_children"])
                    .env(ALONE, "1"),
            )
            .unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && said.contains(" 1 passed"),
                "{said}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        reset();
        start("test");
        for _ in 0..3 {
            let p = phase("loop stage");
            p.count(10, 1);
            count(5, 0);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        {
            let _p = phase("parent");
            {
                let _s = sub("child a");
                count(100, 2);
                let _ss = sub("folding");
            }
            // A child program's phases, as one writes them.
            let mut c = store::spawn::command("true");
            child(&mut c);
            let to = c.get_envs().find(|(k, _)| *k == PHASES_TO_ENV).and_then(|(_, v)| v).map(PathBuf::from).unwrap();
            let kid = RunRec {
                v: 1,
                kind: "x".into(),
                phases: vec![PhaseRec {
                    name: "py read".into(),
                    class: "nas-read".into(),
                    wall_s: 1.5,
                    n: 1,
                    bytes: 7,
                    sub: vec![PhaseRec { name: "deep".into(), n: 1, ..Default::default() }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            std::fs::write(&to, serde_json::to_vec(&kid).unwrap()).unwrap();
        }
        // A thread's phase beside the main one's.
        {
            let _m = phase("main work");
            std::thread::spawn(|| {
                let _b = phase("beside");
            })
            .join()
            .unwrap();
        }
        // Task #870: a phase's wall time split by what the job waited on, by wall clock: a NAS
        // call's 40 ms, two threads' network requests at once (30 ms each, together): 30 ms, not
        // 60; the rest computing.
        let mut both = std::time::Duration::ZERO;
        {
            let _p = phase("measured");
            {
                let _w = store::waits::nas();
                std::thread::sleep(std::time::Duration::from_millis(40)); // (sleep ok: a NAS call's stand-in)
            }
            let t0 = Instant::now();
            let t: Vec<_> = (0..2)
                .map(|_| {
                    std::thread::spawn(|| {
                        let _w = store::waits::net();
                        std::thread::sleep(std::time::Duration::from_millis(30));
                        // (sleep ok: a request's stand-in)
                    })
                })
                .collect();
            t.into_iter().for_each(|t| t.join().unwrap());
            both = t0.elapsed();
            std::thread::sleep(std::time::Duration::from_millis(20)); // (sleep ok: computing's stand-in)
        }
        // Time in no phase: a gap, named by the phases either side.
        std::thread::sleep(std::time::Duration::from_millis(30)); // (sleep ok: a gap of at least 30 ms to find)
                                                                  // A span still open as the run ends: counted to then.
        let held = phase("held open");
        held.count(3, 1);
        let r = snapshot(true).unwrap();
        let names: Vec<&str> = r.phases.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["loop stage", "parent", "main work", "beside", "measured", "held open"]);
        assert_eq!((r.phases[5].n, r.phases[5].bytes), (1, 3));
        let m = &r.phases[4];
        let w = m.waits.unwrap();
        // (Lower bounds by the sleeps; the two requests' together no more than the span they shared,
        // however loaded the Mac: not summed.)
        assert!(w.nas_s >= 0.04 && w.net_s >= 0.03 && w.net_s <= both.as_secs_f64() + 1e-3, "{w:?} {both:?}");
        assert!(w.waited() < m.wall_s && m.wall_s - w.waited() >= 0.02, "{w:?} {}", m.wall_s);
        assert!(table(&r).contains("NAS ") && table(&r).contains("network "), "{}", table(&r));
        // (A phase that ran a child takes in its phases' measured waits only: "py read", an
        // unmeasured record's phase declared a NAS read, adds none, no class guessed from.)
        let pw = r.phases[1].waits.unwrap();
        assert!(pw.nas_s < r.phases[1].wall_s && pw.nas_b == 0, "{pw:?}");
        let l = &r.phases[0];
        assert_eq!((l.n, l.bytes, l.files), (3, 45, 3));
        assert!(l.wall_s >= 0.015);
        let p = &r.phases[1];
        let subs: Vec<&str> = p.sub.iter().map(|s| s.name.as_str()).collect();
        // (The sub-phase's own sub-phase: folded into it, one level only.)
        assert_eq!(subs, ["child a", "py read"]);
        assert_eq!((p.sub[0].bytes, p.sub[0].files), (100, 2));
        assert_eq!(p.sub[1].wall_s, 1.5);
        assert!(p.sub[1].sub.is_empty());
        assert!(r.phases[2].overlapped && r.phases[3].overlapped && r.phases[3].background);
        assert!(!r.phases[0].overlapped);
        // The record's round trip.
        let back: RunRec = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        assert!((r.untimed_s - (r.wall_s - r.timed_s()).max(0.0)).abs() < 1e-9);
        assert!(table(&r).contains("(unaccounted)"));
        let g = &r.gaps[0];
        assert_eq!((g.after.as_str(), g.before.as_str(), g.n), ("measured", "held open", 1), "{:?}", r.gaps);
        assert!(g.wall_s >= 0.03 && g.wall_s <= r.untimed_s + 1e-9, "{:?} {}", r.gaps, r.untimed_s);
        // (A phase open on the main thread: no gap after the last.)
        assert!(r.gaps.iter().all(|g| g.before != END), "{:?}", r.gaps);
        assert!(table(&r).contains("between measured and held open"), "{}", table(&r));
        // A transfer's rate beside its bytes.
        let net = RunRec {
            wall_s: 4.0,
            phases: vec![PhaseRec {
                name: "fetching its files from the coordinator".into(),
                class: "net".into(),
                wall_s: 4.0,
                n: 1,
                bytes: 8_000_000,
                files: 2,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(table(&net).contains("8.0 MB, 2.0 MB/s, 2 files"), "{}", table(&net));
        finish(true);
        drop(held);
        assert!(!on());
        // Nothing timed after.
        let _p = phase("after");
        reset();
    }

    /// Each phase keeps the most memory the job held while it was open (its process group's, as the
    /// memory guard measures a job): a phase holding 400 MB more than the one before says so, a
    /// child's phases bring their own, and the report names the heaviest phase of each kind.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_phase_keeps_the_memory_the_job_held() {
        // (Alone, in a process group of its own: the job's memory is its group's, and the runner's
        // other tests aren't this run's.)
        const ALONE: &str = "SCENIC_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let mut c = store::spawn::command(std::env::current_exe().unwrap());
            c.args(["--exact", "timings::tests::a_phase_keeps_the_memory_the_job_held"]).env(ALONE, "1");
            crate::sys::own_group(&mut c);
            let out = store::spawn::output(&mut c).unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && said.contains(" 1 passed"),
                "{said}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        reset();
        start("test");
        let hold = |mb: usize| {
            let v = vec![1u8; mb << 20];
            // (Long enough for the samples, four a second, to see it.)
            std::thread::sleep(MEM_EVERY * 3); // (sleep ok: the sampler's to see the memory held)
            std::hint::black_box(&v);
        };
        {
            let _p = phase("light");
            hold(0);
        }
        {
            let _p = phase("heavy");
            hold(400);
            let mut c = store::spawn::command("true");
            child(&mut c);
            let to = c.get_envs().find(|(k, _)| *k == PHASES_TO_ENV).and_then(|(_, v)| v).map(PathBuf::from).unwrap();
            let kid = RunRec {
                phases: vec![PhaseRec { name: "its program".into(), n: 1, peak_mb: Some(123), ..Default::default() }],
                ..Default::default()
            };
            std::fs::write(&to, serde_json::to_vec(&kid).unwrap()).unwrap();
        }
        let r = snapshot(true).unwrap();
        finish(true);
        reset();
        let (light, heavy) = (r.phases[0].peak_mb.unwrap(), r.phases[1].peak_mb.unwrap());
        assert!(heavy >= light + 350, "light {light} MB, heavy {heavy} MB");
        assert_eq!(r.phases[1].sub[0].peak_mb, Some(123));
        assert!(table(&r).contains(&format!("peak {}", bytes(heavy << 20))), "{}", table(&r));
        // The record's round trip, and a record from before peaks (none) reads.
        let back: RunRec = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        let old: PhaseRec = serde_json::from_str(r#"{"name":"x","class":"disk","wall_s":1.0,"n":1}"#).unwrap();
        assert_eq!(old.peak_mb, None);
        // Over runs: each phase's most, and the kind's heaviest phase with its program.
        let mut r2 = r.clone();
        r2.phases[0].peak_mb = Some(heavy + 10);
        let h = peak_holders(&[r.clone(), r2.clone()], 20);
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].phase.as_str(), h[0].peak_mb, h[0].runs), ("light", heavy + 10, 2));
        let h = peak_holders(&[r], 20);
        assert_eq!((h[0].phase.as_str(), h[0].sub.clone()), ("heavy", Some(("its program".to_string(), 123))));
        assert!(peak_text(&h).contains("within it its program"), "{}", peak_text(&h));
        assert!(summary_text(&summary(&[r2], None, 20, None)).contains(&bytes((heavy + 10) << 20)));
    }

    /// The report says where a kind's untimed time lies and flags a kind past `UNACCOUNTED_MAX`.
    #[test]
    fn blind_spots_are_flagged_with_their_gaps() {
        let run = |kind: &str, wall: f64, untimed: f64, gap: (&str, &str)| RunRec {
            kind: kind.into(),
            wall_s: wall,
            untimed_s: untimed,
            phases: vec![PhaseRec { name: "work".into(), wall_s: wall - untimed, n: 1, ..Default::default() }],
            gaps: vec![Gap { after: gap.0.into(), before: gap.1.into(), wall_s: untimed, n: 1 }],
            ..Default::default()
        };
        let runs = [
            run("depth-lo", 100.0, 30.0, ("making z3–7 from their children", END)),
            run("depth-lo", 100.0, 10.0, ("making z3–7 from their children", END)),
            run("depth-lo", 100.0, 5.0, (START, "work")),
            run("terrain", 100.0, 1.0, ("work", END)),
        ];
        let sums = summary(&runs, None, 50, None);
        let blind: Vec<&str> = blind_spots(&sums).iter().map(|k| k.kind.as_str()).collect();
        assert_eq!(blind, ["depth-lo"]);
        let d = &sums[0];
        assert_eq!((d.gaps[0].wall_s, d.gaps[0].n), (40.0, 2));
        assert_eq!(d.gaps[1].before, "work");
        let t = summary_text(&sums);
        assert!(t.contains("after making z3–7 from their children"), "{t}");
        assert!(t.contains("before work"), "{t}");
        assert!(t.contains("! 15% in no phase"), "{t}");
        assert!(t.contains("Over 5% of their time in no phase: depth-lo 15%"), "{t}");
        assert!(!t.contains("! 1%"), "{t}");
    }

    #[test]
    fn logs_round_trip_and_sum() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("timings.jsonl");
        let ph = |name: &str, w: f64, b: u64| PhaseRec {
            name: name.into(),
            class: "nas-read".into(),
            wall_s: w,
            cpu_s: Some(w / 2.0),
            n: 1,
            bytes: b,
            files: 1,
            ..Default::default()
        };
        let run = |kind: &str, host: &str, ok: bool, w: f64| RunRec {
            v: 1,
            kind: kind.into(),
            host: host.into(),
            wall_s: w,
            ok,
            untimed_s: 1.0,
            phases: vec![PhaseRec { sub: vec![ph("inner", 1.0, 5)], ..ph("read", 6.0, 10) }, ph("write", 3.0, 0)],
            ..Default::default()
        };
        let runs = [
            run("unit", "m4", true, 10.0),
            run("unit", "m1", false, 10.0),
            run("peaks", "m4", true, 20.0),
            run("unit", "m4", true, 10.0),
        ];
        for r in &runs {
            append(&log, r).unwrap();
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .and_then(|mut f| std::io::Write::write_all(&mut f, b"not json\n"))
            .unwrap();
        let back = read_log(&log);
        assert_eq!(back, runs);
        // A job's record taken, stamped with the host.
        let rec = d.path().join("run.json");
        std::fs::write(&rec, serde_json::to_vec(&run("gc", "", true, 1.0)).unwrap()).unwrap();
        assert_eq!(take_record(&rec, "m1").unwrap().host, "m1");
        assert!(!rec.exists());
        // Summed: kinds in order, the last two runs of unit.
        let s = summary(&back, None, 2, None);
        assert_eq!(s.iter().map(|k| k.kind.as_str()).collect::<Vec<_>>(), ["peaks", "unit"]);
        let u = &s[1];
        assert_eq!((u.runs, u.failed, u.wall_s, u.untimed_s), (2, 1, 20.0, 2.0));
        assert_eq!(u.hosts, ["m1", "m4"]);
        assert_eq!((u.phases[0].wall_s, u.phases[0].bytes, u.phases[0].share), (12.0, 20, 0.6));
        assert_eq!((u.phases[0].sub[0].name.as_str(), u.phases[0].sub[0].wall_s), ("inner", 2.0));
        let m4 = summary(&back, Some("unit"), 10, Some("m4"));
        assert_eq!((m4.len(), m4[0].runs), (1, 2));
        let t = summary_text(&s);
        assert!(
            t.contains("unit — 2 runs (1 not ok) on m1, m4") && t.contains("(unaccounted)") && t.contains("  inner"),
            "{t}"
        );
        // Cut to its newer half past the limit.
        let big = RunRec { id: "x".repeat(1 << 20), ..run("unit", "m4", true, 1.0) };
        for _ in 0..17 {
            append(&log, &big).unwrap();
        }
        let n = std::fs::metadata(&log).unwrap().len();
        assert!(n <= LOG_MAX && n > LOG_MAX / 4, "{n}");
        assert!(read_log(&log).iter().all(|r| r.id.len() == 1 << 20));
    }
}
