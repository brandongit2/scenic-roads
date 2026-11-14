//! The agent's NAS calls under a deadline (docs/pool.md §7.8, Task #380). An open on the SMB share
//! can stay stuck in the kernel for many minutes while the same file reads at once from a shell
//! (18 Oct: 17 minutes in `resume::on`, no heartbeat meanwhile); the loop must keep beating.
//!
//! Each call is a store::deadline call (the one mechanism every NAS call of the process goes
//! through), waited for `DEADLINE` at most. A call past its deadline returns
//! `ErrorKind::TimedOut` to the caller, which takes it as it takes any call that fails now (its own
//! "can't read" path), and is logged once per path per streak (a call on that path that answers
//! ends the streak). The call is left to the kernel; the process's calls held there are capped
//! (store::deadline's cap, the build page's "NAS calls held"), a call at the cap failing at once
//! (also `TimedOut`), so stuck threads never pile up. A write left there holds its path: another
//! write to it is refused at once until it ends (a late write would land over the newer). A
//! healthy share answers well inside the deadline: the result is the call's own, as before.
//!
//! A plan running past `beat::LOOP_STUCK` (the one stall threshold the agent's parts are held to)
//! is logged with its phase and the process sampled (crate::agent::plan); the round's phases are
//! named (`phase`), one taking `SLOW_PHASE` or more logged as it ends. The heartbeat is not the
//! round's: it beats on a timer of its own (crate::agent::beat).

use crate::pool::nas::{Created, Nas};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use store::deadline;
use store::nasfs::NasPath;

/// How long a small read or write may take before its caller goes on without it.
pub const DEADLINE: Duration = deadline::SHORT;

/// The process's NAS calls (crate::agent::live): those in flight now (held ones included) and
/// those missed so far (past their deadline, or refused at the cap).
pub fn calls() -> (usize, u64) {
    (deadline::in_flight(), deadline::missed())
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

/// Whether a call's error is its deadline's (past it, or refused at the cap or on a held path).
fn missed(e: &io::Error) -> bool {
    deadline::past_deadline(e) || deadline::refused(e)
}

/// A call's result, noted: its deadline's error a timeout of `p`'s, anything else an answer.
fn noted<T>(p: &Path, r: io::Result<T>) -> io::Result<T> {
    match &r {
        Err(e) if missed(e) => note(p, true, Some(e)),
        _ => note(p, false, None),
    }
    r
}

/// A NAS path as its calls' label (their errors, the streaks' and the held writes' keys): never
/// opened.
fn label(p: &NasPath) -> PathBuf {
    PathBuf::from(p.display().to_string())
}

/// `f`, a read of what's under `p`, within `deadline` (`TimedOut` past it), noted.
fn read_under<T: Send + 'static>(
    p: &NasPath,
    deadline: Duration,
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let p = &label(p);
    noted(p, deadline::call(&p.display(), &"read", deadline, f))
}

/// The file at `p`, read within `DEADLINE` (`TimedOut` past it).
pub fn read(p: &NasPath) -> io::Result<Vec<u8>> {
    let owned = p.clone();
    read_under(p, DEADLINE, move || owned.read())
}

/// The file at `p` as text, read within `DEADLINE` (`TimedOut` past it).
pub fn read_to_string(p: &NasPath) -> io::Result<String> {
    let owned = p.clone();
    read_under(p, DEADLINE, move || owned.read_to_string())
}

/// `f`, a write of `p`, within `DEADLINE` (an error past it); refused at once while an earlier
/// write to `p` is held in the kernel.
pub fn write(p: &NasPath, f: impl FnOnce() -> anyhow::Result<()> + Send + 'static) -> anyhow::Result<()> {
    let p = &label(p);
    noted(p, deadline::writing(&p.display(), &"write", DEADLINE, vec![p.clone()], move || Ok(f())))?
}

/// A NAS whose every call is a deadline call (`DEADLINE`); its writes and creates hold their path
/// while held. The pool's driver and its member's opening go through it (crate::agent::membership).
pub struct Deadlined {
    inner: Arc<dyn Nas + Send + Sync>,
    deadline: Duration,
}

impl Deadlined {
    pub fn new(inner: Arc<dyn Nas + Send + Sync>) -> Deadlined {
        Deadlined { inner, deadline: DEADLINE }
    }

    /// With a deadline of its own (a test's).
    pub fn with(inner: Arc<dyn Nas + Send + Sync>, deadline: Duration) -> Deadlined {
        Deadlined { inner, deadline }
    }

    fn run<T: Send + 'static>(
        &self,
        writes: bool,
        path: &str,
        f: impl FnOnce(&(dyn Nas + Send + Sync)) -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let inner = self.inner.clone();
        let p = Path::new(path);
        // (A write's key: its path on this NAS: a test's Macs in one process write the same paths
        // on NASes of their own.)
        let keys = if writes {
            vec![PathBuf::from(format!("{:p}", Arc::as_ptr(&self.inner) as *const ())).join(path)]
        } else {
            Vec::new()
        };
        noted(p, deadline::writing(&p.display(), &"call", self.deadline, keys, move || Ok(f(&*inner))))?
    }
}

impl Nas for Deadlined {
    fn create_new(&self, path: &str, bytes: &[u8]) -> anyhow::Result<Created> {
        let (p, b) = (path.to_string(), bytes.to_vec());
        self.run(true, path, move |n| n.create_new(&p, &b))
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let (p, b) = (path.to_string(), bytes.to_vec());
        self.run(true, path, move |n| n.write_whole(&p, &b))
    }

    fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let p = path.to_string();
        self.run(false, path, move |n| n.read(&p))
    }

    fn read_fresh(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let p = path.to_string();
        self.run(false, path, move |n| n.read_fresh(&p))
    }

    fn exists(&self, path: &str) -> anyhow::Result<bool> {
        let p = path.to_string();
        self.run(false, path, move |n| n.exists(&p))
    }
}

/// `f`, a read of what's under `p` (a few small files, a folder listed), run within `DEADLINE`
/// (`TimedOut` past it), noted as a read of `p` is.
pub fn with<T: Send + 'static>(p: &NasPath, f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    read_under(p, DEADLINE, f)
}

/// How long a big read may take (the build's keys: the manifest and its records): 5 min, 20 s in a
/// unit test (`buildstore::out::testing`).
fn big_read_deadline() -> Duration {
    if buildstore::out::testing() {
        Duration::from_secs(20)
    } else {
        Duration::from_secs(300)
    }
}

/// `with`, for a big read of what's under `p` (the build's keys), within `big_read_deadline`.
pub fn with_big<T: Send + 'static>(p: &NasPath, f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    read_under(p, big_read_deadline(), f)
}

/// How long the NAS takes to list folder `p` (the latency series, crate::agent::live: a listing,
/// which macOS's SMB client asks the server for, where a stat may come from its cache), within
/// `DEADLINE`: None when it didn't answer in time, or when the last probe is still stuck in its
/// call (never two at once, so a hung share holds one call at most).
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
    let r = deadline::call(&label(p).display(), &"list", DEADLINE, move || {
        let _done = done;
        let t = std::time::Instant::now();
        Ok(owned.read_dir().is_ok().then(|| t.elapsed()))
    });
    r.ok().flatten()
}

/// A phase of a round that takes this long or longer is logged by name as it ends (`phase`).
pub const SLOW_PHASE: Duration = Duration::from_secs(10);

/// One part's phases (the loop's, the plan's: each on its own thread): the one in progress, its
/// name and when it began. Each part its own, so one part's phase never ends or stretches
/// another's (task #923: the plan thread's phases, in one slot with the loop's, had the loop's
/// logged as lasting until the plan's next).
pub struct Phases {
    who: &'static str,
    now: Mutex<Option<(&'static str, std::time::Instant)>>,
}

impl Phases {
    pub const fn new(who: &'static str) -> Phases {
        Phases { who, now: Mutex::new(None) }
    }

    /// The part enters phase `name` ("": none, its work over): the one that ends is logged if it
    /// took `SLOW_PHASE` or more (but `WAIT`); it, and how long it took. A mutex held only to swap
    /// the phase.
    pub fn enter(&self, name: &'static str) -> Option<(&'static str, Duration)> {
        let next = (!name.is_empty()).then(|| (name, std::time::Instant::now()));
        let was = std::mem::replace(&mut *self.now.lock().unwrap_or_else(|e| e.into_inner()), next)?;
        let took = was.1.elapsed();
        if was.0 != WAIT && took >= SLOW_PHASE {
            eprintln!("agent: {} {} phase \"{}\" took {} s", clock(super::now_s()), self.who, was.0, took.as_secs());
        }
        Some((was.0, took))
    }

    /// The phase in progress.
    pub fn now(&self) -> Option<&'static str> {
        self.now.lock().unwrap_or_else(|e| e.into_inner()).map(|(n, _)| n)
    }
}

/// The loop's phases (`phase`).
pub static LOOP: Phases = Phases::new("the loop's");

/// The plan's phases, on its thread (crate::agent::plan).
pub static PLAN: Phases = Phases::new("the plan's");

/// The loop's wait between rounds (up to 20 s, as planned): a phase, never logged as slow.
pub const WAIT: &str = "the wait between rounds";

/// The loop's round enters phase `name` (`Phases::enter`).
pub fn phase(name: &'static str) {
    LOOP.enter(name);
}

/// The loop's phase in progress (`phase`), for the heartbeat's word on a stuck round.
pub fn phase_now() -> Option<&'static str> {
    LOOP.now()
}

/// `s` (s since the epoch) as the time of day in UTC, for the log: 14:03:59Z.
pub fn clock(s: u64) -> String {
    let d = s % 86400;
    format!("{:02}:{:02}:{:02}Z", d / 3600, d / 60 % 60, d % 60)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Instant;

    /// Task #923: the plan's phases, on their thread, neither end nor replace the loop's: the
    /// loop's phase ends at its own next one, whatever the plan entered meanwhile.
    #[test]
    fn the_plans_phases_never_end_the_loops() {
        let (lp, plan) = (Phases::new("a loop's"), Phases::new("a plan's"));
        lp.enter("the grants");
        std::thread::scope(|s| {
            s.spawn(|| {
                plan.enter("build::plan");
                plan.enter("its jobs");
            });
        });
        assert_eq!(lp.now(), Some("the grants"));
        assert_eq!(lp.enter("the caches").map(|w| w.0), Some("the grants"));
        assert_eq!(plan.enter("").map(|w| w.0), Some("its jobs"));
        assert_eq!(plan.now(), None, "its work over: no phase");
        assert_eq!(plan.enter(""), None);
    }

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
    }

    #[test]
    fn a_healthy_read_is_the_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s");
        assert_eq!(
            read_to_string(&store::nasfs::NasPath::local_for_test(&p)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        std::fs::write(&p, "on").unwrap();
        assert_eq!(read_to_string(&store::nasfs::NasPath::local_for_test(&p)).unwrap(), "on");
        assert_eq!(read(&store::nasfs::NasPath::local_for_test(&p)).unwrap(), b"on");
        let q = d.path().join("w");
        let q2 = q.clone();
        write(&store::nasfs::NasPath::local_for_test(&q), move || Ok(std::fs::write(q2, "x")?)).unwrap();
        assert_eq!(std::fs::read(&q).unwrap(), b"x");
    }

    #[test]
    fn stuck_writes_time_out_and_leave_reads_answering() {
        let d = tempfile::tempdir().unwrap();
        let _cap = store::deadline::cap_for_test(store::deadline::CAP_DEFAULT);
        std::fs::create_dir_all(d.path().join("state/pool/terms")).unwrap();
        std::fs::write(d.path().join("state/pool/terms/1.json"), "{}").unwrap();
        let (hold, gate) = mpsc::channel::<()>();
        let stuck = Stuck {
            inner: store::nasfs::NasRoot::local_for_test(d.path()),
            stuck: "state/pool/members".into(),
            gate: Arc::new(Mutex::new(gate)),
        };
        let n = Deadlined::with(Arc::new(stuck), Duration::from_secs(4));
        // The heartbeat's write stuck: an error within its deadline; a second write to that path
        // refused at once while the first is held (a late write would land over the newer).
        let t = Instant::now();
        assert!(n.write_whole("state/pool/members/m.json", b"{}").is_err());
        store::testkit::ended_before(t, Duration::from_secs(60), "the stuck write ends at its deadline");
        let t = Instant::now();
        let e = n.write_whole("state/pool/members/m.json", b"{}").unwrap_err();
        assert!(buildstore::records::nas::timed_out(&e) && format!("{e}").contains("still in the kernel"), "{e:#}");
        store::testkit::ended_before(t, Duration::from_secs(4), "refused at once, not at the write deadline");
        // The reads go on.
        assert!(n.exists("state/pool/terms/1.json").unwrap());
        assert_eq!(n.read("state/pool/terms/1.json").unwrap().unwrap(), b"{}");
        // The share answers: the path's write ends, and the path is free again.
        drop(hold);
        store::testkit::eventually("the path's write ends", || {
            n.write_whole("state/pool/members/m.json", b"{\"a\":1}").is_ok()
        });
        assert_eq!(std::fs::read(d.path().join("state/pool/members/m.json")).unwrap(), b"{\"a\":1}");
    }

    #[test]
    fn an_unreadable_extract_plans_no_job() {
        use crate::agent::present_or_missing;
        assert_eq!(present_or_missing(Ok(b"x".to_vec())), Some(Some(b"x".to_vec())));
        assert_eq!(present_or_missing(Err(io::ErrorKind::NotFound.into())), Some(None));
        assert_eq!(present_or_missing(Err(io::ErrorKind::TimedOut.into())), None);
        assert_eq!(present_or_missing(Err(io::ErrorKind::PermissionDenied.into())), None);
    }
}
