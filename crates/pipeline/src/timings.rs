//! A job's timings (docs/plan.md §8, Timings; the record: docs/formats.md, timings.jsonl): its run
//! broken into named phases, each with its wall time, its CPU time (user + system, the process's
//! and its finished children's), its class (what it waits on: the NAS, the network, the local disk,
//! the CPU, a lock or another worker) and, for I/O, the bytes and files it moved.
//!
//! - `phase(name, class)` opens a phase (a guard: it ends when dropped); `sub` opens one inside the
//!   phase open on this thread, at most one level down. A phase opened again under the same name
//!   adds to it (`n` counts its spans): a loop over areas or tiles gives each of its stages one
//!   phase, with totals, never one per area.
//! - `count(bytes, files)` adds to the innermost phase open on this thread (`Phase::count` to a
//!   given one, from any thread): an addition to two atomics, cheap enough for a loop's body.
//! - A child program run within a phase (`child(&mut cmd)`) writes its own phases (this module's
//!   twin in Python, dem/timings.py, or a scenic-build run as a child) to a file this one names; its
//!   phases come in as the phase's sub-phases as it ends.
//! - CPU is the process's, from `getrusage`: a phase while another thread's phase (or work in no
//!   phase) runs counts that thread's CPU too. Such a phase is marked `overlapped`, its CPU an
//!   approximation; a phase on a thread but the job's main one is marked `background`, and its wall
//!   isn't counted toward the run's timed part (it ran beside the main thread's phases).
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

/// What a phase waits on, declared where it's opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Reading the NAS (its share, mounted).
    NasRead,
    /// Writing the NAS.
    NasWrite,
    /// The Mac's own disk (its caches, the scratch folder).
    Disk,
    /// The internet, or another Mac's coordinator.
    Net,
    /// The CPU.
    #[default]
    Compute,
    /// A lock, a slot, another worker, another thread.
    Wait,
    /// A phase of several classes, each in a sub-phase (a child program's run).
    Mixed,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::NasRead => "nas-read",
            Class::NasWrite => "nas-write",
            Class::Disk => "disk",
            Class::Net => "net",
            Class::Compute => "compute",
            Class::Wait => "wait",
            Class::Mixed => "mixed",
        }
    }
}

/// A phase as recorded: its totals over its spans.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseRec {
    pub name: String,
    pub class: Class,
    pub wall_s: f64,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub: Vec<PhaseRec>,
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
}

impl RunRec {
    /// The main thread's top-level phases' wall time.
    pub fn timed_s(&self) -> f64 {
        self.phases
            .iter()
            .filter(|p| !p.background)
            .map(|p| p.wall_s)
            .sum()
    }
}

/// A phase's running totals.
#[derive(Clone)]
struct Acc {
    name: String,
    class: Class,
    parent: Option<usize>,
    wall_ns: u64,
    cpu_us: Option<u64>,
    n: u64,
    bytes: u64,
    files: u64,
    overlapped: bool,
    background: bool,
    /// Its child programs' phases, as they came in.
    kids: Vec<PhaseRec>,
}

/// An open span's counters, which other threads (and `count`) add to.
struct Open {
    idx: usize,
    bytes: AtomicU64,
    files: AtomicU64,
    overlapped: AtomicBool,
    thread: std::thread::ThreadId,
    children: Mutex<Vec<PathBuf>>,
    /// When it opened, and the CPU then: a span still open as the run ends counts to then.
    t0: Instant,
    cpu0: Option<u64>,
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Starts timing this process's run as a job of `kind` (its step), from now. Again: no change.
pub fn start(kind: &str) {
    let mut g = run().lock().unwrap();
    if g.is_none() {
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
        });
    }
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
        Phase {
            open: None,
            t: Instant::now(),
            cpu: None,
        }
    }
}

/// Opens a phase: a top-level one when none is open on this thread, else a sub-phase of the one
/// that is; inside a sub-phase, nothing (it's folded into that one: one level only). So a library's
/// own phases (the records read and saved, `crate::out::Out`) nest under the step's.
pub fn phase(name: &str, class: Class) -> Phase {
    open(name, class)
}

/// `phase`, where the call site means a sub-phase (read as one).
pub fn sub(name: &str, class: Class) -> Phase {
    open(name, class)
}

fn open(name: &str, class: Class) -> Phase {
    let t_in = Instant::now();
    let mut g = run().lock().unwrap();
    let Some(r) = g.as_mut().filter(|r| !r.finished) else {
        return Phase::none();
    };
    let me = std::thread::current().id();
    // (Its parent: the phase open on this thread; in a sub-phase, it's folded into that one.)
    let inner = STACK.with(|s| s.borrow().last().map(|o| o.idx));
    if inner.is_some_and(|i| r.accs[i].parent.is_some()) {
        return Phase::none();
    }
    let parent = inner;
    let idx = match r
        .accs
        .iter()
        .position(|a| a.name == name && a.parent == parent)
    {
        Some(i) => i,
        None => {
            r.accs.push(Acc {
                name: name.to_string(),
                class,
                parent,
                wall_ns: 0,
                cpu_us: None,
                n: 0,
                bytes: 0,
                files: 0,
                overlapped: false,
                background: me != r.main,
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
        thread: me,
        children: Mutex::new(Vec::new()),
        t0: Instant::now(),
        cpu0: cpu_us(),
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
    Phase {
        open: Some(o),
        t: Instant::now(),
        cpu,
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        let Some(o) = self.open.take() else { return };
        let wall = self.t.elapsed();
        let t_in = Instant::now();
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
        let kids: Vec<PhaseRec> = o
            .children
            .lock()
            .map(|c| c.iter().flat_map(|p| take_child(p)).collect())
            .unwrap_or_default();
        let mut g = run().lock().unwrap();
        let Some(r) = g.as_mut() else { return };
        if let Some(i) = r.open.iter().position(|x| Arc::ptr_eq(x, &o)) {
            r.open.remove(i);
        }
        if r.finished {
            return;
        }
        let parent = r.accs[o.idx].parent;
        let a = &mut r.accs[o.idx];
        a.wall_ns += wall.as_nanos() as u64;
        a.cpu_us = match (a.cpu_us, cpu) {
            (Some(x), Some(y)) => Some(x + y),
            (None, y) if a.n == 0 => y,
            (x, _) => x,
        };
        a.n += 1;
        a.bytes += o.bytes.load(Ordering::Relaxed);
        a.files += o.files.load(Ordering::Relaxed);
        a.overlapped |= o.overlapped.load(Ordering::Relaxed);
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
            p.cpu_s = match (p.cpu_s, k.cpu_s) {
                (Some(a), Some(b)) => Some(a + b),
                (a, b) => a.or(b),
            };
            p.n += k.n;
            p.bytes += k.bytes;
            p.files += k.files;
            p.overlapped |= k.overlapped;
        }
        None => list.push(PhaseRec {
            sub: Vec::new(),
            background: false,
            ..k
        }),
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

/// Records a span timed by the caller (a lap: `crate::unit::Laps`) as phase `name`, as `phase`
/// would have, with the CPU taken since `cpu0` (`cpu_us()` at its start).
pub fn record(
    name: &str,
    class: Class,
    wall: std::time::Duration,
    cpu0: Option<u64>,
    bytes: u64,
    files: u64,
) {
    let mut g = run().lock().unwrap();
    let Some(r) = g.as_mut().filter(|r| !r.finished) else {
        return;
    };
    let me = std::thread::current().id();
    let idx = match r
        .accs
        .iter()
        .position(|a| a.name == name && a.parent.is_none())
    {
        Some(i) => i,
        None => {
            r.accs.push(Acc {
                name: name.to_string(),
                class,
                parent: None,
                wall_ns: 0,
                cpu_us: None,
                n: 0,
                bytes: 0,
                files: 0,
                overlapped: false,
                background: me != r.main,
                kids: Vec::new(),
            });
            r.accs.len() - 1
        }
    };
    let over = r.open.iter().any(|x| x.thread != me);
    let cpu = match (cpu0, cpu_us()) {
        (Some(a), Some(b)) => Some(b.saturating_sub(a)),
        _ => None,
    };
    let a = &mut r.accs[idx];
    a.wall_ns += wall.as_nanos() as u64;
    a.cpu_us = match (a.cpu_us, cpu) {
        (Some(x), Some(y)) => Some(x + y),
        (None, y) if a.n == 0 => y,
        (x, _) => x,
    };
    a.n += 1;
    a.bytes += bytes;
    a.files += files;
    a.overlapped |= over;
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
    r.map(|r| {
        r.phases
            .into_iter()
            .map(|p| PhaseRec {
                sub: Vec::new(),
                ..p
            })
            .collect()
    })
    .unwrap_or_default()
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
        if let (Some(c0), Some(c1)) = (o.cpu0, now_cpu) {
            a.cpu_us = Some(a.cpu_us.unwrap_or(0) + c1.saturating_sub(c0));
        }
        a.n += 1;
        a.bytes += o.bytes.load(Ordering::Relaxed);
        a.files += o.files.load(Ordering::Relaxed);
        a.overlapped |= o.overlapped.load(Ordering::Relaxed);
    }
    let rec_of = |i: usize| {
        let a = &accs[i];
        let mut sub: Vec<PhaseRec> = r
            .accs
            .iter()
            .filter(|s| s.parent == Some(i) && s.n > 0)
            .map(|s| PhaseRec {
                name: s.name.clone(),
                class: s.class,
                wall_s: s.wall_ns as f64 / 1e9,
                cpu_s: s.cpu_us.map(|c| c as f64 / 1e6),
                n: s.n,
                bytes: s.bytes,
                files: s.files,
                overlapped: s.overlapped,
                background: s.background,
                sub: Vec::new(),
            })
            .collect();
        for k in &a.kids {
            merge_into(&mut sub, k.clone());
        }
        PhaseRec {
            name: a.name.clone(),
            class: a.class,
            wall_s: a.wall_ns as f64 / 1e9,
            cpu_s: a.cpu_us.map(|c| c as f64 / 1e6),
            n: a.n,
            bytes: a.bytes,
            files: a.files,
            overlapped: a.overlapped,
            background: a.background,
            sub,
        }
    };
    let phases: Vec<PhaseRec> = (0..accs.len())
        .filter(|&i| accs[i].parent.is_none() && accs[i].n > 0)
        .map(rec_of)
        .collect();
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
        threads: std::env::var("RAYON_NUM_THREADS")
            .ok()
            .and_then(|t| t.parse().ok()),
        ok,
        untimed_s: 0.0,
        overhead_s: r.overhead_ns as f64 / 1e9,
        phases,
    };
    rec.untimed_s = (wall_s - rec.timed_s()).max(0.0);
    rec
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
        format!(
            "{} h {} min",
            (s / 3600.0) as u64,
            (s % 3600.0 / 60.0) as u64
        )
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
        let share = if r.wall_s > 0.0 {
            p.wall_s / r.wall_s * 100.0
        } else {
            0.0
        };
        let mut extra = Vec::new();
        if let Some(c) = p.cpu_s {
            extra.push(format!(
                "CPU {}{}",
                secs(c),
                if p.overlapped { " ~" } else { "" }
            ));
        }
        if p.n > 1 {
            extra.push(format!("×{}", p.n));
        }
        if p.bytes > 0 {
            extra.push(bytes(p.bytes));
        }
        if p.files > 0 {
            extra.push(format!("{} files", p.files));
        }
        if p.background {
            extra.push("beside".into());
        }
        s.push_str(&format!(
            "timings: {indent}{:<40} {:>10} {:>4.0}%  {:<9} {}\n",
            p.name,
            secs(p.wall_s),
            share,
            p.class.name(),
            extra.join(", ")
        ));
    };
    for p in &r.phases {
        line(&mut s, "", p);
        for q in &p.sub {
            line(&mut s, "  ", q);
        }
    }
    let share = if r.wall_s > 0.0 {
        r.untimed_s / r.wall_s * 100.0
    } else {
        0.0
    };
    s.push_str(&format!(
        "timings: {:<40} {:>10} {:>4.0}%\n",
        "(untimed)",
        secs(r.untimed_s),
        share
    ));
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
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)?
        .write_all(&line)?;
    if std::fs::metadata(p)?.len() > LOG_MAX {
        let b = std::fs::read(p)?;
        let cut = b.len() / 2;
        let from = b[cut..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(b.len(), |i| cut + i + 1);
        write_whole(p, &b[from..])?;
    }
    Ok(())
}

/// The runs in the log at `p`, oldest first (lines that don't read skipped).
pub fn read_log(p: &Path) -> Vec<RunRec> {
    let Ok(s) = std::fs::read_to_string(p) else {
        return Vec::new();
    };
    s.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
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
    pub class: Class,
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
}

fn add_phase(list: &mut Vec<PhaseSum>, p: &PhaseRec, total: f64, top: bool) {
    let i = match list.iter().position(|q| q.name == p.name) {
        Some(i) => i,
        None => {
            list.push(PhaseSum {
                name: p.name.clone(),
                class: p.class,
                ..Default::default()
            });
            list.len() - 1
        }
    };
    let q = &mut list[i];
    q.runs += 1;
    q.wall_s += p.wall_s;
    q.cpu_s = match (q.cpu_s, p.cpu_s) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
    q.n += p.n;
    q.bytes += p.bytes;
    q.files += p.files;
    q.overlapped |= p.overlapped;
    q.background |= p.background;
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
pub fn summary(
    runs: &[RunRec],
    kind: Option<&str>,
    last: usize,
    host: Option<&str>,
) -> Vec<KindSum> {
    let mut kinds: Vec<&str> = runs
        .iter()
        .map(|r| r.kind.as_str())
        .filter(|k| kind.is_none_or(|w| w == *k))
        .collect();
    kinds.sort();
    kinds.dedup();
    kinds
        .into_iter()
        .map(|k| {
            let mut mine: Vec<&RunRec> = runs
                .iter()
                .filter(|r| r.kind == k && host.is_none_or(|h| h == r.host))
                .collect();
            let skip = mine.len().saturating_sub(last);
            mine.drain(..skip);
            let mut s = KindSum {
                kind: k.to_string(),
                runs: mine.len(),
                ..Default::default()
            };
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
            }
            let total = s.wall_s;
            shares(&mut s.phases, total);
            s
        })
        .filter(|s| s.runs > 0)
        .collect()
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
    runs.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(r, _)| r.clone())
        .collect()
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
            "  {:<44} {:>6} {:>11} {:>11} {:>7}  {:<9} {}\n",
            "phase", "share", "total", "a run", "CPU/wall", "class", "moved"
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
                "  {:<44} {:>5.1}% {:>11} {:>11} {:>7}  {:<9} {}\n",
                name,
                p.share * 100.0,
                secs(p.wall_s),
                secs(p.wall_s / n),
                ratio,
                p.class.name(),
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
            "  {:<44} {:>5.1}% {:>11} {:>11}\n\n",
            "(untimed)",
            if k.wall_s > 0.0 {
                k.untimed_s / k.wall_s * 100.0
            } else {
                0.0
            },
            secs(k.untimed_s),
            secs(k.untimed_s / n)
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

    // (One test: the run is the process's.)
    #[test]
    fn phases_add_up_nest_and_take_in_children() {
        reset();
        start("test");
        for _ in 0..3 {
            let p = phase("loop stage", Class::Compute);
            p.count(10, 1);
            count(5, 0);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        {
            let _p = phase("parent", Class::Mixed);
            {
                let _s = sub("child a", Class::NasRead);
                count(100, 2);
                let _ss = sub("folded", Class::Disk);
            }
            // A child program's phases, as one writes them.
            let mut c = std::process::Command::new("true");
            child(&mut c);
            let to = c
                .get_envs()
                .find(|(k, _)| *k == PHASES_TO_ENV)
                .and_then(|(_, v)| v)
                .map(PathBuf::from)
                .unwrap();
            let kid = RunRec {
                v: 1,
                kind: "x".into(),
                phases: vec![PhaseRec {
                    name: "py read".into(),
                    class: Class::NasRead,
                    wall_s: 1.5,
                    n: 1,
                    bytes: 7,
                    sub: vec![PhaseRec {
                        name: "deep".into(),
                        n: 1,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            std::fs::write(&to, serde_json::to_vec(&kid).unwrap()).unwrap();
        }
        // A thread's phase beside the main one's.
        {
            let _m = phase("main work", Class::Compute);
            std::thread::spawn(|| {
                let _b = phase("beside", Class::NasRead);
            })
            .join()
            .unwrap();
        }
        // A span still open as the run ends: counted to then.
        let held = phase("held open", Class::Disk);
        held.count(3, 1);
        let r = snapshot(true).unwrap();
        let names: Vec<&str> = r.phases.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["loop stage", "parent", "main work", "beside", "held open"]);
        assert_eq!((r.phases[4].n, r.phases[4].bytes), (1, 3));
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
        assert!(table(&r).contains("(untimed)"));
        finish(true);
        drop(held);
        assert!(!on());
        // Nothing timed after.
        let _p = phase("after", Class::Compute);
        reset();
    }

    #[test]
    fn logs_round_trip_and_sum() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("timings.jsonl");
        let ph = |name: &str, w: f64, b: u64| PhaseRec {
            name: name.into(),
            class: Class::NasRead,
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
            phases: vec![
                PhaseRec {
                    sub: vec![ph("inner", 1.0, 5)],
                    ..ph("read", 6.0, 10)
                },
                ph("write", 3.0, 0),
            ],
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
        assert_eq!(
            s.iter().map(|k| k.kind.as_str()).collect::<Vec<_>>(),
            ["peaks", "unit"]
        );
        let u = &s[1];
        assert_eq!((u.runs, u.failed, u.wall_s, u.untimed_s), (2, 1, 20.0, 2.0));
        assert_eq!(u.hosts, ["m1", "m4"]);
        assert_eq!(
            (u.phases[0].wall_s, u.phases[0].bytes, u.phases[0].share),
            (12.0, 20, 0.6)
        );
        assert_eq!(
            (u.phases[0].sub[0].name.as_str(), u.phases[0].sub[0].wall_s),
            ("inner", 2.0)
        );
        let m4 = summary(&back, Some("unit"), 10, Some("m4"));
        assert_eq!((m4.len(), m4[0].runs), (1, 2));
        let t = summary_text(&s);
        assert!(
            t.contains("unit — 2 runs (1 not ok) on m1, m4")
                && t.contains("(untimed)")
                && t.contains("  inner"),
            "{t}"
        );
        // Cut to its newer half past the limit.
        let big = RunRec {
            id: "x".repeat(1 << 20),
            ..run("unit", "m4", true, 1.0)
        };
        for _ in 0..17 {
            append(&log, &big).unwrap();
        }
        let n = std::fs::metadata(&log).unwrap().len();
        assert!(n <= LOG_MAX && n > LOG_MAX / 4, "{n}");
        assert!(read_log(&log).iter().all(|r| r.id.len() == 1 << 20));
    }
}
