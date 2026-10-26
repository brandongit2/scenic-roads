//! The agent's NAS calls under a deadline (docs/pool.md §7.8, Task #380). An open on the SMB share
//! can stay stuck in the kernel for many minutes while the same file reads at once from a shell
//! (18 Oct: 17 minutes in `resume::on`, no heartbeat meanwhile); the loop must keep beating.
//!
//! Each call runs on a thread of a small pool while the caller waits a deadline at most. Three
//! pools, so one kind of call stuck can't starve another: the switches' reads (`read_switch`), the
//! records' reads (`read`, `read_to_string`, and `Deadlined`'s reads, stats and listings), and the
//! writes (`write`, and `Deadlined`'s writes, creates and removes). A call past its deadline
//! returns `ErrorKind::TimedOut` to the caller, which takes it as it takes any call that fails now
//! (its own "can't read" path), and is logged once per path per streak (a call on that path that
//! answers ends the streak). The thread stuck in it is left to it and counts as busy until it
//! returns; while every thread of a pool is busy, a call to it fails at once (also `TimedOut`), so
//! stuck threads never pile up past a pool's size. A healthy share answers well inside the
//! deadline: the result is the call's own, as before.
//!
//! And the loop's watchdog (`watch`): a round not finished in `STALL_AFTER` is logged with the
//! phase of the round it's in (`phase`), kept in the agent's folder for `scenic status`, and the
//! agent sampled once into that folder.

use crate::pool::nas::{Created, Nas};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use store::nasfs::NasPath;

/// How long a small read or write may take before its caller goes on without it.
pub const DEADLINE: Duration = Duration::from_secs(20);
/// How long a listing may take (3 to 33 s a folder under load, pool::nas::Nas::list).
pub const LIST_DEADLINE: Duration = Duration::from_secs(60);
/// How long a create in the terms' folder may take: a term made but its bytes not landed comes back
/// as `Created::Unwritten` for the driver to finish, which a timeout would lose.
pub const TERM_CREATE_DEADLINE: Duration = Duration::from_secs(300);
/// The switches' readers. (A unit test's process runs many agents at once, each a Mac's, sharing
/// the pools: more threads there.)
pub const SWITCH_READERS: usize = if cfg!(test) { 64 } else { 2 };
/// The records' readers.
pub const READERS: usize = if cfg!(test) { 64 } else { 4 };
/// The writers.
pub const WRITERS: usize = if cfg!(test) { 64 } else { 4 };

type Job = Box<dyn FnOnce() + Send>;

/// A small pool of threads, `n` at most busy at once.
pub struct Calls {
    n: usize,
    busy: Arc<AtomicUsize>,
    /// Its calls that passed their deadline or were refused, every thread busy (`missed`).
    missed: AtomicU64,
    tx: Mutex<Sender<Job>>,
}

impl Calls {
    pub fn new(name: &str, n: usize) -> Calls {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..n {
            let rx: Arc<Mutex<Receiver<Job>>> = rx.clone();
            std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || loop {
                    let job = match rx.lock() {
                        Ok(r) => r.recv(),
                        Err(_) => return,
                    };
                    match job {
                        Ok(job) => job(),
                        Err(_) => return,
                    }
                })
                .expect("spawn a NAS call thread");
        }
        Calls { n, busy: Arc::new(AtomicUsize::new(0)), missed: AtomicU64::new(0), tx: Mutex::new(tx) }
    }

    /// `f` run on a thread of the pool, its result if it returns within `deadline`; else
    /// `TimedOut` (also at once while every thread is busy). `what` names the call in the error.
    pub fn call<T: Send + 'static>(
        &self,
        what: &Path,
        deadline: Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> io::Result<T> {
        // A thread taken, or none free: fail fast.
        if self.busy.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |b| (b < self.n).then_some(b + 1)).is_err() {
            self.missed.fetch_add(1, Ordering::Relaxed);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{}: every NAS thread for it is stuck in an earlier call", what.display()),
            ));
        }
        let (rtx, rrx) = mpsc::sync_channel(1);
        let busy = self.busy.clone();
        let job: Job = Box::new(move || {
            let r = f();
            busy.fetch_sub(1, Ordering::SeqCst);
            rtx.send(r).ok();
        });
        if self.tx.lock().map_err(|_| io::Error::other("NAS threads poisoned"))?.send(job).is_err() {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            return Err(io::Error::other("NAS threads gone"));
        }
        rrx.recv_timeout(deadline).map_err(|_| {
            self.missed.fetch_add(1, Ordering::Relaxed);
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{}: no answer from the NAS in {} s", what.display(), deadline.as_secs()),
            )
        })
    }

    /// The threads busy now (stuck ones included).
    pub fn busy(&self) -> usize {
        self.busy.load(Ordering::SeqCst)
    }

    /// Its calls missed so far: past their deadline, or refused with every thread busy.
    pub fn missed(&self) -> u64 {
        self.missed.load(Ordering::Relaxed)
    }
}

static SWITCH_CALLS: OnceLock<Calls> = OnceLock::new();
static RECORD_CALLS: OnceLock<Calls> = OnceLock::new();
static WRITE_CALLS: OnceLock<Calls> = OnceLock::new();

fn switches() -> &'static Calls {
    SWITCH_CALLS.get_or_init(|| Calls::new("nas-switch", SWITCH_READERS))
}

fn records() -> &'static Calls {
    RECORD_CALLS.get_or_init(|| Calls::new("nas-read", READERS))
}

fn writers() -> &'static Calls {
    WRITE_CALLS.get_or_init(|| Calls::new("nas-write", WRITERS))
}

/// The process's NAS calls through the three pools (the live store's, crate::agent::live): those in
/// flight now (stuck ones included) and those missed so far (`Calls::missed`).
pub fn calls() -> (usize, u64) {
    [&SWITCH_CALLS, &RECORD_CALLS, &WRITE_CALLS]
        .iter()
        .filter_map(|c| c.get())
        .fold((0, 0), |(b, m), c| (b + c.busy(), m + c.missed()))
}

/// The paths whose calls are timing out now (logged once per streak).
fn streaks() -> &'static Mutex<BTreeSet<PathBuf>> {
    static S: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Notes a call's outcome: a timeout logged as its path's streak begins, an answer ending it.
fn note(p: &Path, timed_out: bool, e: Option<&io::Error>) {
    let Ok(mut s) = streaks().lock() else { return };
    if timed_out {
        if s.insert(p.to_path_buf()) {
            if let Some(e) = e {
                eprintln!("agent: {e}; taken as failed for now, the loop goes on");
            }
        }
    } else if s.remove(p) {
        eprintln!("agent: {} answers again", p.display());
    }
}

/// A call's result, flattened and noted.
fn noted<T>(p: &Path, r: io::Result<io::Result<T>>) -> io::Result<T> {
    match r {
        Ok(r) => {
            note(p, false, None);
            r
        }
        Err(e) => {
            note(p, true, Some(&e));
            Err(e)
        }
    }
}

/// A NAS path as its calls' label (their errors, the streaks' and in-flight keys): never opened.
fn label(p: &NasPath) -> PathBuf {
    PathBuf::from(p.display().to_string())
}

/// A switch file at `p` as text, read within `DEADLINE` on the switches' own readers.
pub fn read_switch(p: &NasPath) -> io::Result<String> {
    let (owned, p) = (p.clone(), &label(p));
    noted(p, switches().call(p, DEADLINE, move || owned.read_to_string()))
}

/// The file at `p`, read within `DEADLINE` (`TimedOut` past it).
pub fn read(p: &NasPath) -> io::Result<Vec<u8>> {
    let (owned, p) = (p.clone(), &label(p));
    noted(p, records().call(p, DEADLINE, move || owned.read()))
}

/// The file at `p` as text, read within `DEADLINE` (`TimedOut` past it).
pub fn read_to_string(p: &NasPath) -> io::Result<String> {
    let (owned, p) = (p.clone(), &label(p));
    noted(p, records().call(p, DEADLINE, move || owned.read_to_string()))
}

/// `f`, a write of `p`, run on the writers within `DEADLINE` (an error past it).
pub fn write(p: &NasPath, f: impl FnOnce() -> anyhow::Result<()> + Send + 'static) -> anyhow::Result<()> {
    let p = &label(p);
    let held = InFlight::take(p)?;
    match writers().call(p, DEADLINE, move || {
        let _held = held;
        f()
    }) {
        Ok(r) => {
            note(p, false, None);
            r
        }
        Err(e) => {
            note(p, true, Some(&e));
            Err(e.into())
        }
    }
}

/// A write's path while the write runs: a second write to it is refused (`TimedOut`) until the
/// first ends on its thread, not when its caller stopped waiting (two writes to one path would share
/// its temporary name). Released as dropped: at the call's end, or with a call never run.
struct InFlight(PathBuf);

fn in_flight() -> &'static Mutex<BTreeSet<PathBuf>> {
    static S: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

impl InFlight {
    fn take(p: &Path) -> io::Result<InFlight> {
        let mut s = in_flight().lock().map_err(|_| io::Error::other("in-flight writes poisoned"))?;
        if !s.insert(p.to_path_buf()) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{}: an earlier write to it still runs", p.display()),
            ));
        }
        Ok(InFlight(p.to_path_buf()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Ok(mut s) = in_flight().lock() {
            s.remove(&self.0);
        }
    }
}

/// Whether `e` is a call past its deadline (or refused, every thread for it stuck).
pub fn timed_out(e: &anyhow::Error) -> bool {
    e.downcast_ref::<io::Error>().is_some_and(|e| e.kind() == io::ErrorKind::TimedOut)
}

/// A NAS whose every call runs under a deadline: its reads, stats and listings on the records'
/// readers, its writes, creates and removes on the writers. The pool's driver and its member's
/// opening go through it (crate::agent::pool).
pub struct Deadlined {
    inner: Arc<dyn Nas + Send + Sync>,
    reads: &'static Calls,
    writes: &'static Calls,
    deadline: Duration,
    list_deadline: Duration,
    term_create_deadline: Duration,
}

impl Deadlined {
    pub fn new(inner: Arc<dyn Nas + Send + Sync>) -> Deadlined {
        Deadlined {
            inner,
            reads: records(),
            writes: writers(),
            deadline: DEADLINE,
            list_deadline: LIST_DEADLINE,
            term_create_deadline: TERM_CREATE_DEADLINE,
        }
    }

    /// Its terms' creates' deadline set (a test's).
    pub fn term_create_deadline(mut self, d: Duration) -> Deadlined {
        self.term_create_deadline = d;
        self
    }

    /// With pools and deadlines of its own (a test's).
    pub fn with(
        inner: Arc<dyn Nas + Send + Sync>,
        reads: &'static Calls,
        writes: &'static Calls,
        deadline: Duration,
        list_deadline: Duration,
    ) -> Deadlined {
        Deadlined { inner, reads, writes, deadline, list_deadline, term_create_deadline: TERM_CREATE_DEADLINE }
    }

    fn run<T: Send + 'static>(
        &self,
        calls: &Calls,
        path: &str,
        deadline: Duration,
        f: impl FnOnce(&(dyn Nas + Send + Sync)) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let inner = self.inner.clone();
        let p = Path::new(path);
        // (A write: refused while an earlier one to its path still runs, `InFlight`.)
        // (Its path on this NAS: a test's Macs in one process write the same paths on NASes of
        // their own.)
        let key = PathBuf::from(format!("{:p}", Arc::as_ptr(&self.inner) as *const ())).join(path);
        let held = if std::ptr::eq(calls, self.reads) { None } else { Some(InFlight::take(&key)?) };
        match calls.call(p, deadline, move || {
            let _held = held;
            f(&*inner)
        }) {
            Ok(r) => {
                note(p, false, None);
                r
            }
            Err(e) => {
                note(p, true, Some(&e));
                Err(e.into())
            }
        }
    }
}

impl Nas for Deadlined {
    fn create_new(&self, path: &str, bytes: &[u8]) -> anyhow::Result<Created> {
        let (p, b) = (path.to_string(), bytes.to_vec());
        let deadline = if path.starts_with(crate::pool::term::DIR) { self.term_create_deadline } else { self.deadline };
        self.run(self.writes, path, deadline, move |n| n.create_new(&p, &b))
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let (p, b) = (path.to_string(), bytes.to_vec());
        self.run(self.writes, path, self.deadline, move |n| n.write_whole(&p, &b))
    }

    fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let p = path.to_string();
        self.run(self.reads, path, self.deadline, move |n| n.read(&p))
    }

    fn read_fresh(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let p = path.to_string();
        self.run(self.reads, path, self.deadline, move |n| n.read_fresh(&p))
    }

    fn exists(&self, path: &str) -> anyhow::Result<bool> {
        let p = path.to_string();
        self.run(self.reads, path, self.deadline, move |n| n.exists(&p))
    }

    fn list(&self, dir: &str) -> anyhow::Result<Vec<String>> {
        let p = dir.to_string();
        self.run(self.reads, dir, self.list_deadline, move |n| n.list(&p))
    }

    fn remove(&self, path: &str) -> anyhow::Result<()> {
        let p = path.to_string();
        self.run(self.writes, path, self.deadline, move |n| n.remove(&p))
    }
}

/// `f`, a read of what's under `p` (a few small files, a folder listed), run within `DEADLINE`
/// (`TimedOut` past it), noted as a read of `p` is.
pub fn with<T: Send + 'static>(p: &NasPath, f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    let p = &label(p);
    noted(p, records().call(p, DEADLINE, f))
}

/// How long the NAS takes to list folder `p` (the latency series, crate::agent::live: a listing,
/// which macOS's SMB client asks the server for, where a stat may come from its cache), on
/// the records' readers within `DEADLINE`: None when it didn't answer in time, or when the last
/// probe is still stuck in its call (never two at once, so a hung share holds one reader at most).
pub fn probe(p: &NasPath) -> Option<Duration> {
    static PROBING: AtomicBool = AtomicBool::new(false);
    /// Clears `PROBING` as the call's closure is dropped: once it ran, or refused unrun.
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            PROBING.store(false, Ordering::SeqCst);
        }
    }
    if PROBING.swap(true, Ordering::SeqCst) {
        return None;
    }
    let (owned, done) = (p.clone(), Done);
    let r = records().call(&label(p), DEADLINE, move || {
        let _done = done;
        let t = std::time::Instant::now();
        owned.read_dir().is_ok().then(|| t.elapsed())
    });
    r.ok().flatten()
}

/// The watchdog's file in the agent's folder while its loop is stalled (`Stall`).
pub const STALL_FILE: &str = "loop-stall.json";
/// The agent sampled (`sample <pid> 10`) as a stall begins, in the agent's folder (the last stall's).
pub const STALL_SAMPLE: &str = "loop-stall-sample.txt";
/// A loop that hasn't finished a round in this long is stalled (its start counts as a round).
pub const STALL_AFTER: Duration = Duration::from_secs(300);
/// The agent's start given this much more before it's a stall (its first round opens the pool).
pub const STALL_GRACE: Duration = Duration::from_secs(300);
/// A phase of a round that takes this long or longer is logged by name as it ends (`phase`).
pub const SLOW_PHASE: Duration = Duration::from_secs(10);

/// When the agent's loop last finished a round, or the agent started (s since the epoch).
pub static ROUND_AT: AtomicU64 = AtomicU64::new(0);
/// Set as the agent's loop ends: the watchdog stops.
pub static WATCH_DONE: AtomicBool = AtomicBool::new(false);
/// The loop's phase in progress (`phase`): its name, and when it began (s since the epoch, and
/// as an instant).
static PHASE: Mutex<Option<(&'static str, u64, std::time::Instant)>> = Mutex::new(None);

/// The loop enters phase `name` (the one before it ends): the watchdog names it in a stall, and
/// the one that ends is logged if it took `SLOW_PHASE` or more.
pub fn phase(name: &'static str) {
    let was =
        PHASE.lock().unwrap_or_else(|e| e.into_inner()).replace((name, super::now_s(), std::time::Instant::now()));
    if let Some((was, _, t)) = was.filter(|w| w.2.elapsed() >= SLOW_PHASE) {
        eprintln!("agent: {} the loop's phase \"{was}\" took {} s", clock(super::now_s()), t.elapsed().as_secs());
    }
}

/// The loop's phase in progress, and when it began (s since the epoch).
fn phase_now() -> Option<(&'static str, u64)> {
    PHASE.lock().unwrap_or_else(|e| e.into_inner()).map(|(n, at, _)| (n, at))
}

/// `s` (s since the epoch) as the time of day in UTC, for the log: 14:03:59Z.
pub fn clock(s: u64) -> String {
    let d = s % 86400;
    format!("{:02}:{:02}:{:02}Z", d / 3600, d / 60 % 60, d % 60)
}

/// The loop stalled, as the watchdog saw it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stall {
    /// When the loop last finished a round (s since the epoch).
    pub round_at: u64,
    /// When the watchdog last looked (s since the epoch).
    pub seen_at: u64,
    /// The round's phase in progress (`phase`), and since when (s since the epoch).
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub phase_at: u64,
}

/// The agent's watchdog, started before the agent opens anything on the NAS (`round_at` set to
/// now): a thread that looks every `every` at when the loop last finished a round (`round_at`,
/// stored by the loop); past `after`, it logs it once with its phase (`phase`), has `sample` (the
/// sampler's path: `/usr/bin/sample`) sample this process for 10 s into `STALL_SAMPLE`, and keeps
/// `STALL_FILE` in `home` (this Mac's disk, never the NAS) up to date, removed when the loop goes
/// on (its real length logged then). It ends once `done` is set.
pub fn watch(
    round_at: &'static AtomicU64,
    done: &'static AtomicBool,
    home: PathBuf,
    after: Duration,
    grace: Duration,
    every: Duration,
    sample: Option<PathBuf>,
) -> std::thread::JoinHandle<()> {
    let start = super::now_s();
    round_at.store(start, Ordering::SeqCst);
    done.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("loop-watchdog".into())
        .spawn(move || {
            let file = home.join(STALL_FILE);
            // (The round the stall is in: when the one before it finished.)
            let mut stalled: Option<u64> = None;
            std::fs::remove_file(&file).ok();
            while !done.load(Ordering::SeqCst) {
                let now = super::now_s();
                let at = round_at.load(Ordering::SeqCst);
                // (The start given `grace` more: a slow start isn't a stall.)
                let limit = if at == start { after + grace } else { after };
                let (phase, phase_at) = phase_now().map_or((String::new(), 0), |(n, t)| (n.to_string(), t));
                if now.saturating_sub(at) >= limit.as_secs() {
                    if stalled.is_none() {
                        eprintln!(
                            "agent: {} the loop hasn't finished a round in {} s; stalled in \"{phase}\" ({} s so far)",
                            clock(now),
                            now.saturating_sub(at),
                            now.saturating_sub(phase_at)
                        );
                        if let Some(s) = &sample {
                            sample_self(s, &home.join(STALL_SAMPLE));
                        }
                    }
                    stalled = Some(at);
                    if let Ok(b) = serde_json::to_vec(&Stall { round_at: at, seen_at: now, phase, phase_at }) {
                        std::fs::write(&file, b).ok();
                    }
                } else if let Some(from) = stalled.take() {
                    eprintln!(
                        "agent: {} the loop goes on again: that round took {} s",
                        clock(now),
                        at.saturating_sub(from)
                    );
                    std::fs::remove_file(&file).ok();
                }
                std::thread::sleep(every);
            }
            std::fs::remove_file(&file).ok();
        })
        .expect("spawn the loop's watchdog")
}

/// This process sampled by `sampler` for 10 s into `out`, off the watchdog's thread (waited for
/// there, so it leaves no zombie). Its failure is logged, nothing more.
fn sample_self(sampler: &Path, out: &Path) {
    let child = store::spawn::spawn(
        store::spawn::command(sampler)
            .args([std::process::id().to_string(), "10".into(), "-mayDie".into(), "-file".into()])
            .arg(out)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    );
    match child {
        Ok(mut c) => {
            let out = out.to_path_buf();
            std::thread::spawn(move || match c.wait() {
                Ok(st) if st.success() => eprintln!("agent: the stalled loop sampled into {}", out.display()),
                Ok(st) => eprintln!("agent: sampling the stalled loop: {st}"),
                Err(e) => eprintln!("agent: sampling the stalled loop: {e}"),
            });
        }
        Err(e) => eprintln!("agent: sampling the stalled loop ({}): {e}", sampler.display()),
    }
}

/// The loop's stall as its watchdog last wrote it in `home`, if it's current (seen within a minute).
pub fn stall(home: &Path) -> Option<Stall> {
    let s: Stall = serde_json::from_slice(&std::fs::read(home.join(STALL_FILE)).ok()?).ok()?;
    (super::now_s().saturating_sub(s.seen_at) < 60).then_some(s)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_probe_times_a_listing_and_one_that_fails_is_none() {
        let d = tempfile::tempdir().unwrap();
        assert!(probe(&store::nasfs::NasPath::local_for_test(d.path())).is_some_and(|t| t < DEADLINE));
        assert_eq!(probe(&store::nasfs::NasPath::local_for_test(&d.path().join("missing"))), None);
        // (Its flag cleared each time: a third answers too.)
        assert!(probe(&store::nasfs::NasPath::local_for_test(d.path())).is_some());
    }

    /// A NAS whose calls on paths starting with `stuck` never return (until `release` drops),
    /// the rest a real share's.
    pub(crate) struct Stuck {
        pub inner: store::nasfs::NasRoot,
        pub stuck: String,
        pub gate: Arc<Mutex<Receiver<()>>>,
    }

    impl Stuck {
        fn hang(&self, p: &str) {
            if p.starts_with(&self.stuck) {
                self.gate.lock().unwrap().recv().ok();
            }
        }
    }

    impl Nas for Stuck {
        fn create_new(&self, path: &str, bytes: &[u8]) -> anyhow::Result<Created> {
            self.hang(path);
            self.inner.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> anyhow::Result<()> {
            self.hang(path);
            self.inner.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.hang(path);
            self.inner.read(path)
        }
        fn exists(&self, path: &str) -> anyhow::Result<bool> {
            self.hang(path);
            self.inner.exists(path)
        }
        fn list(&self, dir: &str) -> anyhow::Result<Vec<String>> {
            self.hang(dir);
            self.inner.list(dir)
        }
        fn remove(&self, path: &str) -> anyhow::Result<()> {
            self.hang(path);
            self.inner.remove(path)
        }
    }

    pub(crate) fn leak(c: Calls) -> &'static Calls {
        Box::leak(Box::new(c))
    }

    #[test]
    fn a_call_that_never_returns_times_out_within_the_deadline() {
        let r = Calls::new("t", 2);
        let (hold, gate) = mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let t = Instant::now();
        let g = gate.clone();
        let e = r
            .call(Path::new("/nas/state/pool/resume"), Duration::from_millis(200), move || {
                g.lock().unwrap().recv().ok();
            })
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        store::testkit::ended_before(
            t,
            Duration::from_secs(60),
            "the stuck call ends at its deadline, not when the share answers",
        );
        // The stuck thread stays busy; another still answers.
        assert_eq!(r.busy(), 1);
        assert_eq!(r.call(Path::new("x"), Duration::from_secs(5), || 7).unwrap(), 7);
        // Every thread stuck: fail fast, nothing more spawned.
        let g2 = gate.clone();
        let _ = r.call(Path::new("y"), Duration::from_millis(100), move || {
            g2.lock().unwrap().recv().ok();
        });
        assert_eq!(r.busy(), 2);
        let t = Instant::now();
        let e = r.call(Path::new("z"), Duration::from_secs(30), || ()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        store::testkit::ended_before(t, Duration::from_secs(30), "refused at once, not at its deadline");
        // Task #517: two past their deadline, one refused.
        assert_eq!(r.missed(), 3);
        // The share answers again: the threads free.
        drop(hold);
        store::testkit::eventually("the threads free", || r.busy() == 0);
        assert_eq!(r.call(Path::new("x"), Duration::from_secs(5), || 1).unwrap(), 1);
    }

    #[test]
    fn a_healthy_read_is_the_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s");
        assert_eq!(
            read_to_string(&store::nasfs::NasPath::local_for_test(&p)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            read_switch(&store::nasfs::NasPath::local_for_test(&p)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        std::fs::write(&p, "on").unwrap();
        assert_eq!(read_to_string(&store::nasfs::NasPath::local_for_test(&p)).unwrap(), "on");
        assert_eq!(read_switch(&store::nasfs::NasPath::local_for_test(&p)).unwrap(), "on");
        assert_eq!(read(&store::nasfs::NasPath::local_for_test(&p)).unwrap(), b"on");
        let q = d.path().join("w");
        let q2 = q.clone();
        write(&store::nasfs::NasPath::local_for_test(&q), move || Ok(std::fs::write(q2, "x")?)).unwrap();
        assert_eq!(std::fs::read(&q).unwrap(), b"x");
    }

    #[test]
    fn stuck_writes_time_out_and_leave_reads_and_switches_answering() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("state/pool/terms")).unwrap();
        std::fs::write(d.path().join("state/pool/terms/1.json"), "{}").unwrap();
        let (hold, gate) = mpsc::channel::<()>();
        let stuck = Stuck {
            inner: store::nasfs::NasRoot::local_for_test(d.path()),
            stuck: "state/pool/members".into(),
            gate: Arc::new(Mutex::new(gate)),
        };
        let (reads, writes) = (leak(Calls::new("tr", 2)), leak(Calls::new("tw", 2)));
        let n = Deadlined::with(Arc::new(stuck), reads, writes, Duration::from_secs(4), Duration::from_millis(200));
        // The heartbeat's write stuck: an error within its deadline; a second write to that path
        // refused at once while the first still runs (never two at once on one temporary name).
        let t = Instant::now();
        assert!(n.write_whole("state/pool/members/m.json", b"{}").is_err());
        store::testkit::ended_before(t, Duration::from_secs(60), "the stuck write ends at its deadline");
        assert_eq!(writes.busy(), 1);
        let t = Instant::now();
        let e = n.write_whole("state/pool/members/m.json", b"{}").unwrap_err();
        assert!(timed_out(&e) && format!("{e}").contains("still runs"), "{e:#}");
        store::testkit::ended_before(t, Duration::from_secs(4), "refused at once, not at the write deadline");
        assert_eq!(writes.busy(), 1, "no second thread on it");
        // Another path's write takes the other writer; then every writer is stuck: at once.
        assert!(n.write_whole("state/pool/members/o.json", b"{}").is_err());
        assert_eq!(writes.busy(), 2);
        let t = Instant::now();
        assert!(n.write_whole("state/pool/members/p.json", b"{}").is_err());
        store::testkit::ended_before(t, Duration::from_secs(4), "refused at once, not at the write deadline");
        // The reads go on: the terms listed, read.
        assert_eq!(n.list("state/pool/terms").unwrap(), vec!["1.json".to_string()]);
        assert_eq!(n.read("state/pool/terms/1.json").unwrap().unwrap(), b"{}");
        // And a switch, on its own readers.
        std::fs::write(d.path().join("sw"), "on").unwrap();
        assert_eq!(read_switch(&store::nasfs::NasPath::local_for_test(&d.path().join("sw"))).unwrap(), "on");
        // The share answers: the path's write ends on its thread, and the path is free again.
        drop(hold);
        store::testkit::eventually("the path's write ends", || writes.busy() == 0);
        n.write_whole("state/pool/members/m.json", b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(d.path().join("state/pool/members/m.json")).unwrap(), b"{\"a\":1}");
    }

    #[test]
    fn a_slow_term_create_keeps_its_answer() {
        let d = tempfile::tempdir().unwrap();
        let (hold, gate) = mpsc::channel::<()>();
        let stuck = Stuck {
            inner: store::nasfs::NasRoot::local_for_test(d.path()),
            stuck: crate::pool::term::DIR.into(),
            gate: Arc::new(Mutex::new(gate)),
        };
        let (reads, writes) = (leak(Calls::new("cr", 1)), leak(Calls::new("cw", 2)));
        let n = Deadlined::with(Arc::new(stuck), reads, writes, Duration::from_millis(100), Duration::from_millis(100))
            .term_create_deadline(Duration::from_secs(10));
        // The share answers after five ordinary deadlines: the term's create waits for its answer.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            drop(hold);
        });
        let term = format!("{}/7.json", crate::pool::term::DIR);
        assert!(matches!(n.create_new(&term, b"{}").unwrap(), Created::Made));
        assert!(matches!(n.create_new(&term, b"{}").unwrap(), Created::There));
    }

    #[test]
    fn opening_a_member_lists_the_terms_patiently_and_never_hangs() {
        let d = tempfile::tempdir().unwrap();
        let (hold, gate) = mpsc::channel::<()>();
        let stuck = Stuck {
            inner: store::nasfs::NasRoot::local_for_test(d.path()),
            stuck: crate::pool::term::DIR.into(),
            gate: Arc::new(Mutex::new(gate)),
        };
        let (reads, writes) = (leak(Calls::new("or", 4)), leak(Calls::new("ow", 1)));
        let n = Deadlined::with(Arc::new(stuck), reads, writes, Duration::from_millis(100), Duration::from_millis(100));
        let t = Instant::now();
        let e = crate::agent::pool::list_patiently(
            &n,
            crate::pool::term::DIR,
            &[Duration::from_millis(10), Duration::from_millis(20)],
        )
        .unwrap_err();
        assert!(timed_out(&e), "{e:#}");
        store::testkit::ended_before(t, Duration::from_secs(60), "tried three times, never hung");
        assert_eq!(reads.busy(), 3, "one stuck reader a try");
        drop(hold);
        // A listing that answers on a later try.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(crate::pool::term::DIR)).unwrap();
        let n = store::nasfs::NasRoot::local_for_test(d.path());
        assert_eq!(crate::agent::pool::list_patiently(&n, crate::pool::term::DIR, &[]).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn an_unreadable_extract_plans_no_job() {
        use crate::agent::present_or_missing;
        assert_eq!(present_or_missing(Ok(b"x".to_vec())), Some(Some(b"x".to_vec())));
        assert_eq!(present_or_missing(Err(io::ErrorKind::NotFound.into())), Some(None));
        assert_eq!(present_or_missing(Err(io::ErrorKind::TimedOut.into())), None);
        assert_eq!(present_or_missing(Err(io::ErrorKind::PermissionDenied.into())), None);
    }

    #[test]
    fn the_watchdog_flags_a_stalled_loop_and_clears_it() {
        static AT: AtomicU64 = AtomicU64::new(0);
        static DONE: AtomicBool = AtomicBool::new(false);
        let d = tempfile::tempdir().unwrap();
        // (A sampler that writes its arguments: sampled once, as the stall begins.)
        let sampler = d.path().join("sampler");
        std::fs::write(&sampler, "#!/bin/sh\necho \"$@\" > \"$5\"\n").unwrap();
        std::fs::set_permissions(&sampler, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        phase("a test's phase");
        let t = watch(
            &AT,
            &DONE,
            d.path().into(),
            Duration::from_secs(1),
            Duration::ZERO,
            Duration::from_millis(50),
            Some(sampler),
        );
        store::testkit::eventually("the stall flagged", || stall(d.path()).is_some());
        // (Another test's loop may have entered a phase of its own since.)
        assert!(!stall(d.path()).unwrap().phase.is_empty());
        store::testkit::eventually("the agent sampled", || {
            std::fs::read_to_string(d.path().join(STALL_SAMPLE))
                .is_ok_and(|s| s.starts_with(&format!("{} 10 -mayDie -file", std::process::id())))
        });
        // (A round just now, a little ahead so a second ticking over meanwhile doesn't count.)
        AT.store(crate::agent::now_s() + 60, Ordering::SeqCst);
        store::testkit::eventually("the stall cleared", || stall(d.path()).is_none());
        DONE.store(true, Ordering::SeqCst);
        t.join().unwrap();
    }
}
