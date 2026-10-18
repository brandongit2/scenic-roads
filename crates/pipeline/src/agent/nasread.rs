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
//! And the loop's watchdog (`watch`): a round not finished in `STALL_AFTER` is logged and kept in
//! the agent's folder for `scenic status`.

use crate::pool::nas::{Created, Nas};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How long a small read or write may take before its caller goes on without it.
pub const DEADLINE: Duration = Duration::from_secs(20);
/// How long a listing may take (3 to 33 s a folder under load, pool::nas::Nas::list).
pub const LIST_DEADLINE: Duration = Duration::from_secs(90);
/// The switches' readers.
pub const SWITCH_READERS: usize = 2;
/// The records' readers.
pub const READERS: usize = 4;
/// The writers.
pub const WRITERS: usize = 4;

type Job = Box<dyn FnOnce() + Send>;

/// A small pool of threads, `n` at most busy at once.
pub struct Calls {
    n: usize,
    busy: Arc<AtomicUsize>,
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
        Calls { n, busy: Arc::new(AtomicUsize::new(0)), tx: Mutex::new(tx) }
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
}

fn switches() -> &'static Calls {
    static R: OnceLock<Calls> = OnceLock::new();
    R.get_or_init(|| Calls::new("nas-switch", SWITCH_READERS))
}

fn records() -> &'static Calls {
    static R: OnceLock<Calls> = OnceLock::new();
    R.get_or_init(|| Calls::new("nas-read", READERS))
}

fn writers() -> &'static Calls {
    static R: OnceLock<Calls> = OnceLock::new();
    R.get_or_init(|| Calls::new("nas-write", WRITERS))
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

/// A switch file at `p` as text, read within `DEADLINE` on the switches' own readers.
pub fn read_switch(p: &Path) -> io::Result<String> {
    let owned = p.to_path_buf();
    noted(p, switches().call(p, DEADLINE, move || std::fs::read_to_string(owned)))
}

/// The file at `p`, read within `DEADLINE` (`TimedOut` past it).
pub fn read(p: &Path) -> io::Result<Vec<u8>> {
    let owned = p.to_path_buf();
    noted(p, records().call(p, DEADLINE, move || std::fs::read(owned)))
}

/// The file at `p` as text, read within `DEADLINE` (`TimedOut` past it).
pub fn read_to_string(p: &Path) -> io::Result<String> {
    let owned = p.to_path_buf();
    noted(p, records().call(p, DEADLINE, move || std::fs::read_to_string(owned)))
}

/// `f`, a write of `p`, run on the writers within `DEADLINE` (an error past it).
pub fn write(p: &Path, f: impl FnOnce() -> anyhow::Result<()> + Send + 'static) -> anyhow::Result<()> {
    match writers().call(p, DEADLINE, f) {
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
}

impl Deadlined {
    pub fn new(inner: Arc<dyn Nas + Send + Sync>) -> Deadlined {
        Deadlined { inner, reads: records(), writes: writers(), deadline: DEADLINE, list_deadline: LIST_DEADLINE }
    }

    /// With pools and deadlines of its own (a test's).
    pub fn with(
        inner: Arc<dyn Nas + Send + Sync>,
        reads: &'static Calls,
        writes: &'static Calls,
        deadline: Duration,
        list_deadline: Duration,
    ) -> Deadlined {
        Deadlined { inner, reads, writes, deadline, list_deadline }
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
        match calls.call(p, deadline, move || f(&*inner)) {
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
        self.run(self.writes, path, self.deadline, move |n| n.create_new(&p, &b))
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let (p, b) = (path.to_string(), bytes.to_vec());
        self.run(self.writes, path, self.deadline, move |n| n.write_whole(&p, &b))
    }

    fn write_aside(&self, path: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let (p, b) = (path.to_string(), bytes.to_vec());
        self.run(self.writes, path, self.deadline, move |n| n.write_aside(&p, &b))
    }

    fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let p = path.to_string();
        self.run(self.reads, path, self.deadline, move |n| n.read(&p))
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

/// The watchdog's file in the agent's folder while its loop is stalled (`Stall`).
pub const STALL_FILE: &str = "loop-stall.json";
/// A loop that hasn't finished a round in this long is stalled (its start counts as a round).
pub const STALL_AFTER: Duration = Duration::from_secs(300);

/// When the agent's loop last finished a round, or the agent started (s since the epoch).
pub static ROUND_AT: AtomicU64 = AtomicU64::new(0);
/// Set as the agent's loop ends: the watchdog stops.
pub static WATCH_DONE: AtomicBool = AtomicBool::new(false);

/// The loop stalled, as the watchdog saw it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stall {
    /// When the loop last finished a round (s since the epoch).
    pub round_at: u64,
    /// When the watchdog last looked (s since the epoch).
    pub seen_at: u64,
}

/// The agent's watchdog, started before the agent opens anything on the NAS (`round_at` set to
/// now): a thread that looks every `every` at when the loop last finished a round (`round_at`,
/// stored by the loop); past `after`, it logs it once and keeps `STALL_FILE` in `home` (this Mac's
/// disk, never the NAS) up to date, removed when the loop goes on. It ends once `done` is set.
pub fn watch(
    round_at: &'static AtomicU64,
    done: &'static AtomicBool,
    home: PathBuf,
    after: Duration,
    every: Duration,
) -> std::thread::JoinHandle<()> {
    round_at.store(super::now_s(), Ordering::SeqCst);
    done.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("loop-watchdog".into())
        .spawn(move || {
            let file = home.join(STALL_FILE);
            let mut stalled = false;
            std::fs::remove_file(&file).ok();
            while !done.load(Ordering::SeqCst) {
                let now = super::now_s();
                let at = round_at.load(Ordering::SeqCst);
                if now.saturating_sub(at) >= after.as_secs() {
                    if !stalled {
                        eprintln!("agent: the loop hasn't finished a round in {} s; stalled", now.saturating_sub(at));
                    }
                    stalled = true;
                    if let Ok(b) = serde_json::to_vec(&Stall { round_at: at, seen_at: now }) {
                        std::fs::write(&file, b).ok();
                    }
                } else if stalled {
                    eprintln!("agent: the loop goes on again");
                    stalled = false;
                    std::fs::remove_file(&file).ok();
                }
                std::thread::sleep(every);
            }
            std::fs::remove_file(&file).ok();
        })
        .expect("spawn the loop's watchdog")
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

    /// A NAS whose calls on paths starting with `stuck` never return (until `release` drops),
    /// the rest a real share's.
    pub(crate) struct Stuck {
        pub inner: crate::pool::nas::Share,
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
        assert!(t.elapsed() < Duration::from_secs(2), "within the deadline: {:?}", t.elapsed());
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
        assert!(t.elapsed() < Duration::from_millis(100), "at once");
        // The share answers again: the threads free.
        drop(hold);
        let t = Instant::now();
        while r.busy() > 0 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(r.busy(), 0);
        assert_eq!(r.call(Path::new("x"), Duration::from_secs(5), || 1).unwrap(), 1);
    }

    #[test]
    fn a_healthy_read_is_the_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s");
        assert_eq!(read_to_string(&p).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(read_switch(&p).unwrap_err().kind(), io::ErrorKind::NotFound);
        std::fs::write(&p, "on").unwrap();
        assert_eq!(read_to_string(&p).unwrap(), "on");
        assert_eq!(read_switch(&p).unwrap(), "on");
        assert_eq!(read(&p).unwrap(), b"on");
        let q = d.path().join("w");
        let q2 = q.clone();
        write(&q, move || Ok(std::fs::write(q2, "x")?)).unwrap();
        assert_eq!(std::fs::read(&q).unwrap(), b"x");
    }

    #[test]
    fn stuck_writes_time_out_and_leave_reads_and_switches_answering() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("state/pool/terms")).unwrap();
        std::fs::write(d.path().join("state/pool/terms/1.json"), "{}").unwrap();
        let (hold, gate) = mpsc::channel::<()>();
        let stuck = Stuck {
            inner: crate::pool::nas::Share::new(d.path()),
            stuck: "state/pool/members".into(),
            gate: Arc::new(Mutex::new(gate)),
        };
        let (reads, writes) = (leak(Calls::new("tr", 2)), leak(Calls::new("tw", 2)));
        let n = Deadlined::with(Arc::new(stuck), reads, writes, Duration::from_millis(200), Duration::from_millis(200));
        // The heartbeat's write stuck: an error within its deadline, then at once.
        for _ in 0..3 {
            let t = Instant::now();
            assert!(n.write_aside("state/pool/members/m.json", b"{}").is_err());
            assert!(t.elapsed() < Duration::from_secs(2));
        }
        assert_eq!(writes.busy(), 2);
        // The reads go on: the terms listed, read.
        assert_eq!(n.list("state/pool/terms").unwrap(), vec!["1.json".to_string()]);
        assert_eq!(n.read("state/pool/terms/1.json").unwrap().unwrap(), b"{}");
        // And a switch, on its own readers.
        std::fs::write(d.path().join("sw"), "on").unwrap();
        assert_eq!(read_switch(&d.path().join("sw")).unwrap(), "on");
        drop(hold);
    }

    #[test]
    fn opening_a_member_lists_the_terms_patiently_and_never_hangs() {
        let d = tempfile::tempdir().unwrap();
        let (hold, gate) = mpsc::channel::<()>();
        let stuck = Stuck {
            inner: crate::pool::nas::Share::new(d.path()),
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
        assert!(t.elapsed() < Duration::from_secs(2), "tried three times, never hung: {:?}", t.elapsed());
        assert_eq!(reads.busy(), 3, "one stuck reader a try");
        drop(hold);
        // A listing that answers on a later try.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(crate::pool::term::DIR)).unwrap();
        let n = crate::pool::nas::Share::new(d.path());
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
        let t = watch(&AT, &DONE, d.path().into(), Duration::from_secs(1), Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(2300));
        assert!(stall(d.path()).is_some(), "stalled");
        AT.store(crate::agent::now_s(), Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        assert!(stall(d.path()).is_none(), "going on");
        DONE.store(true, Ordering::SeqCst);
        t.join().unwrap();
    }
}
