//! One deadline for every NAS call a process makes (docs/nas-access.md §6, task #1141). An SMB call
//! can stay in the kernel for hours, answering nothing and failing nothing (11 Nov: two unit jobs
//! in `pread` for 40 minutes and more, task #1139; `scenic gc`'s queue removal for 2 h on the
//! mini). A call made here runs on a helper thread its calling thread keeps, and its caller waits
//! for it a deadline at most, counting only the time this process ran (`LOOK`); past it the caller
//! gets `TimedOut` (`past_deadline`), logged with what the call was, and the helper is left to the
//! kernel: its result, when it comes, goes nowhere, and the caller's next call gets a new helper.
//!
//! - **A cap** (`cap`): the calls past their deadline still in the kernel (`held`) are at most
//!   this many a process; while they're that many, a new call fails at once (`refused`), so stuck
//!   threads never pile up. Each Mac's own setting, on the build page (crates/agentcore's
//!   agent::settings, `CAP_FILE`); its agent gives it to its jobs (`CAP_ENV`), the CLI reads it.
//!   Not set: `CAP_DEFAULT`.
//! - **Progress** (`watched`): a call that says it moved (`Progress::add`, a copy's bytes) has its
//!   deadline counted from its last move, so a long copy that keeps moving never ends for its
//!   length alone.
//! - **Writes** (`writing`): a call that changes the share names the paths it changes; while one
//!   left past its deadline is still in the kernel, another such call on any of its paths fails at
//!   once. Abandoning a call never changes what it does on the share (it lands, or not, as it would
//!   have with its caller waiting); what it changes is what this process does next, and only a
//!   later call of this process on the same path could race it. (Why each class of call is safe to
//!   leave: docs/nas-access.md §6.)
//! - **Nested**: a call made on a helper (inside another call) runs there, inline: the outer
//!   call's deadline holds it.
//! - **A job's exit**: a process that left calls in the kernel may never end its exit (the kernel
//!   ends a process once each of its threads leaves its call); jobrt's `control::exit` writes the
//!   job's status first, which its agent takes as its end.
//!
//! Every NAS call of a job, the CLI, the agent and the server goes through here: store::nasfs's
//! `NasPath` (its root's deadline: `LONG` but the agent's loop's and the server's, `SHORT`), the
//! range readers (`range::PlainFile`), the server's I/O pool (store::iopool, its breaker on top),
//! the copies to this Mac's disk (store::blobs), and the agent's own (crates/agentcore's
//! agent::nasread).

use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The deadline of a job's or the CLI's NAS call, and of the agent's own threads': the NAS answers
/// a call in milliseconds, a span of tens of MB in seconds; a read waits as long for its turn
/// (crate::aimd::TAKE_WAIT).
pub const LONG: Duration = Duration::from_secs(120);
/// The deadline of a call whose caller must answer soon: the agent's loop and its parts', the
/// server's HTTP workers.
pub const SHORT: Duration = Duration::from_secs(20);

/// A read of `len` bytes' deadline: `base` and a second more a MB (a 32 MB span on a share giving
/// it 1 MB/s still ends).
pub fn for_bytes(base: Duration, len: usize) -> Duration {
    base + Duration::from_secs((len >> 20) as u64)
}

/// The calls past their deadline a process holds in the kernel at most, when the Mac's setting
/// isn't set: a job's every worker stuck once and more (a job ends once its work fails past a
/// deadline: jobrt's `control::exit`), the agent's parts' calls on a share hung for minutes.
pub const CAP_DEFAULT: usize = 32;
/// The cap as a job is given it by its agent (`cap`).
pub const CAP_ENV: &str = "SCENIC_NAS_HELD";
/// The cap as set on the build page: `{"n": …, "by": …, "at": …}` in the agent's folder.
pub const CAP_FILE: &str = "nas-held.json";

/// How often a caller looks at its call. A look that comes back over `LATE` times later than asked
/// counts as one look: the process was stopped (the agent's memory and disk guards freeze a job,
/// SIGSTOP), and that time isn't the call's. (The Mac asleep isn't counted either: `Instant` is the
/// uptime clock on macOS.) The rule only ever counts less than the time passed: a late look never
/// ends a call sooner.
const LOOK: Duration = Duration::from_secs(1);
const LATE: u32 = 4;

/// The cap set in this process (0: not yet read, `cap`).
static CAP: AtomicUsize = AtomicUsize::new(0);
/// Calls past their deadline, still in the kernel.
static HELD: AtomicUsize = AtomicUsize::new(0);
/// Calls running now, those held included.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
/// Calls past their deadline, and calls refused (`refused`), so far.
static MISSED: AtomicU64 = AtomicU64::new(0);
/// The paths of the writes held (`writing`), and how many there are (read without the lock).
static STUCK: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static STUCK_N: AtomicUsize = AtomicUsize::new(0);

/// The calls past their deadline this process holds in the kernel at most (the module's doc): as
/// set (`set_cap`), else as its agent gave it (`CAP_ENV`), else `CAP_DEFAULT`.
pub fn cap() -> usize {
    match CAP.load(Ordering::Relaxed) {
        0 => {
            let n = std::env::var(CAP_ENV).ok().and_then(|v| v.trim().parse().ok()).filter(|&n| n > 0);
            let n = n.unwrap_or(CAP_DEFAULT);
            CAP.store(n, Ordering::Relaxed);
            n
        }
        n => n,
    }
}

/// Sets the cap (None: `CAP_DEFAULT`), taking effect at the next call.
pub fn set_cap(n: Option<usize>) {
    CAP.store(n.filter(|&n| n > 0).unwrap_or(CAP_DEFAULT), Ordering::Relaxed);
}

/// The cap set in the agent's folder `home` (`CAP_FILE`), if it is.
pub fn cap_in(home: &std::path::Path) -> Option<usize> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join(CAP_FILE)).ok()?).ok()?;
    v["n"].as_u64().map(|n| n as usize).filter(|&n| n > 0)
}

/// The calls past their deadline still in the kernel.
pub fn held() -> usize {
    HELD.load(Ordering::Relaxed)
}

/// The calls running now, those held included.
pub fn in_flight() -> usize {
    IN_FLIGHT.load(Ordering::Relaxed)
}

/// The calls past their deadline, and those refused, so far.
pub fn missed() -> u64 {
    MISSED.load(Ordering::Relaxed)
}

/// What a call that says it moved adds to (`watched`).
#[derive(Clone)]
pub struct Progress(Arc<Shared>);

impl Progress {
    /// The call moved `n` (bytes): its deadline counts from now.
    pub fn add(&self, n: u64) {
        self.0.moved.fetch_add(n.max(1), Ordering::Relaxed);
    }
}

// A call's life: RUNNING → DONE, or RUNNING → LEFT (its caller gave up on it at its deadline).
const RUNNING: u8 = 0;
const DONE: u8 = 1;
const LEFT: u8 = 2;

struct Shared {
    state: AtomicU8,
    moved: AtomicU64,
    keys: Vec<PathBuf>,
}

/// A call's end on its helper: DONE, or, its caller gone, its hold let go (`HELD`, its keys).
struct Ends(Arc<Shared>);

impl Drop for Ends {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
        if self.0.state.compare_exchange(RUNNING, DONE, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return;
        }
        // (Left: its caller counted it held, its keys stuck, under STUCK's lock.)
        let mut s = lock(&STUCK);
        for k in &self.0.keys {
            if let Some(i) = s.iter().position(|p| p == k) {
                s.swap_remove(i);
            }
        }
        STUCK_N.store(s.len(), Ordering::Relaxed);
        HELD.fetch_sub(1, Ordering::Relaxed);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A call past its deadline (`past_deadline`), or refused (`refused`): the error's payload.
#[derive(Debug)]
enum Missed {
    Past(String),
    Refused(String),
}

impl Display for Missed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Missed::Past(s) | Missed::Refused(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Missed {}

/// Whether `e` is a call past its deadline here (not an OS error's `TimedOut`, nor a refusal).
pub fn past_deadline(e: &io::Error) -> bool {
    e.get_ref().and_then(|x| x.downcast_ref::<Missed>()).is_some_and(|m| matches!(m, Missed::Past(_)))
}

/// Whether `e` is a call refused at once: the cap reached, or a write on a path a held call has.
pub fn refused(e: &io::Error) -> bool {
    e.get_ref().and_then(|x| x.downcast_ref::<Missed>()).is_some_and(|m| matches!(m, Missed::Refused(_)))
}

fn refuse(said: String) -> io::Error {
    MISSED.fetch_add(1, Ordering::Relaxed);
    io::Error::new(io::ErrorKind::TimedOut, Missed::Refused(said))
}

/// A call `bounded` runs on its caller's helper.
type Job = Box<dyn FnOnce() + Send>;

thread_local! {
    /// This thread's helper (its calls run there, one at a time): made at its first call, kept
    /// while its calls come back in time, given up with one that doesn't.
    static HELPER: RefCell<Option<mpsc::Sender<Job>>> = const { RefCell::new(None) };
    /// Whether this thread is a helper (a call made here runs inline).
    static ON_HELPER: Cell<bool> = const { Cell::new(false) };
}

/// A new helper thread: it runs the calls sent it, and ends once its sender is dropped and the
/// call it's in (if any) has come back.
fn helper() -> io::Result<mpsc::Sender<Job>> {
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::Builder::new().name("nas call".into()).spawn(move || {
        ON_HELPER.with(|h| h.set(true));
        for c in rx {
            c();
        }
    })?;
    Ok(tx)
}

/// `f` (a NAS call on `on`, doing `op`) within `deadline` (the module's doc).
pub fn call<T: Send + 'static>(
    on: &dyn Display,
    op: &dyn Display,
    deadline: Duration,
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    run(on, op, deadline, Vec::new(), move |_| f())
}

/// `call`, for a call that changes the share at `keys` (its paths: the module's doc): refused at
/// once while a held call has any of them.
pub fn writing<T: Send + 'static>(
    on: &dyn Display,
    op: &dyn Display,
    deadline: Duration,
    keys: Vec<PathBuf>,
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    run(on, op, deadline, keys, move |_| f())
}

/// `call` (a write when `keys` has paths, as `writing`), given its `Progress` to say it moved:
/// its deadline counted from its last move.
pub fn watched<T: Send + 'static>(
    on: &dyn Display,
    op: &dyn Display,
    deadline: Duration,
    keys: Vec<PathBuf>,
    f: impl FnOnce(&Progress) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    run(on, op, deadline, keys, f)
}

/// What a look that took `took`, asked to take `look`, counts toward a deadline (`LATE`).
fn counted(took: Duration, look: Duration) -> Duration {
    if took > look * LATE {
        look
    } else {
        took
    }
}

fn run<T: Send + 'static>(
    on: &dyn Display,
    op: &dyn Display,
    deadline: Duration,
    keys: Vec<PathBuf>,
    f: impl FnOnce(&Progress) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    if !keys.is_empty() && STUCK_N.load(Ordering::Relaxed) > 0 {
        let s = lock(&STUCK);
        if let Some(k) = keys.iter().find(|k| s.contains(k)) {
            return Err(refuse(format!(
                "{on}: {op}: an earlier call on {} is still in the kernel past its deadline; not made",
                k.display()
            )));
        }
    }
    let shared = Arc::new(Shared { state: AtomicU8::new(RUNNING), moved: AtomicU64::new(0), keys });
    if ON_HELPER.with(|h| h.get()) {
        // (Inside another call: its deadline holds this one.)
        return f(&Progress(shared));
    }
    let (held, cap) = (HELD.load(Ordering::Relaxed), cap());
    if held >= cap {
        return Err(refuse(format!(
            "{on}: {op}: {held} NAS calls of this process are in the kernel past their deadline (its cap, \
             the build page's \"NAS calls held\"); not made"
        )));
    }
    #[cfg(any(test, feature = "testkit"))]
    let (f, deadline) = {
        let (gate, short) = fake::gate(on).unzip();
        let deadline = short.flatten().map_or(deadline, |d| d.min(deadline));
        let f = move |p: &Progress| {
            if let Some(g) = gate {
                while !g.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            f(p)
        };
        (f, deadline)
    };
    let nested = crate::retry::in_attempt();
    let (tx, rx) = mpsc::sync_channel(1);
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    let ends = Ends(shared.clone());
    let progress = Progress(shared.clone());
    let mut job: Job = Box::new(move || {
        // (The caller's retry's try is this call's too: a retry in it tries once, crate::retry.)
        let _a = nested.then(crate::retry::attempt);
        let r = f(&progress);
        drop(ends);
        let _ = tx.send(r);
    });
    let h = loop {
        let h = match HELPER.with(|h| h.borrow_mut().take()) {
            Some(h) => h,
            None => helper()?,
        };
        match h.send(job) {
            Ok(()) => break h,
            // (Its helper ended, a call of its panicking: a new one.)
            Err(mpsc::SendError(j)) => job = j,
        }
    };
    let (mut waited, mut moved) = (Duration::ZERO, 0);
    let r = loop {
        let t = Instant::now();
        match rx.recv_timeout(LOOK.min(deadline.saturating_sub(waited))) {
            Ok(r) => break r,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(io::Error::other(format!("{on}: {op}: its call panicked"))),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = shared.moved.load(Ordering::Relaxed);
        if now != moved {
            (moved, waited) = (now, Duration::ZERO);
            continue;
        }
        waited += counted(t.elapsed(), LOOK);
        if waited < deadline {
            continue;
        }
        // (Answered as the deadline came: taken.)
        if let Ok(r) = rx.try_recv() {
            break r;
        }
        let mut s = lock(&STUCK);
        if shared.state.compare_exchange(RUNNING, LEFT, Ordering::AcqRel, Ordering::Acquire).is_err() {
            // (It ended as the deadline came: its answer is on its way.)
            drop(s);
            match rx.recv() {
                Ok(r) => break r,
                Err(_) => return Err(io::Error::other(format!("{on}: {op}: its call panicked"))),
            }
        }
        s.extend(shared.keys.iter().cloned());
        STUCK_N.store(s.len(), Ordering::Relaxed);
        drop(s);
        let held = HELD.fetch_add(1, Ordering::Relaxed) + 1;
        MISSED.fetch_add(1, Ordering::Relaxed);
        let said = format!(
            "{on}: {op}: no answer in {} s; left to the kernel ({held} of this process's calls there now)",
            deadline.as_secs_f64().round()
        );
        crate::retry::log(format_args!("{said}"));
        return Err(io::Error::new(io::ErrorKind::TimedOut, Missed::Past(said)));
    };
    HELPER.with(|c| *c.borrow_mut() = Some(h));
    r
}

/// Test builds: the cap set to `n` until the guard is dropped (one test at a time holds it).
#[cfg(any(test, feature = "testkit"))]
pub fn cap_for_test(n: usize) -> CapForTest {
    static TURN: Mutex<()> = Mutex::new(());
    let turn = lock(&TURN);
    let was = CAP.swap(n, Ordering::Relaxed);
    CapForTest { _turn: turn, was }
}

#[cfg(any(test, feature = "testkit"))]
pub struct CapForTest {
    _turn: std::sync::MutexGuard<'static, ()>,
    was: usize,
}

#[cfg(any(test, feature = "testkit"))]
impl Drop for CapForTest {
    fn drop(&mut self) {
        CAP.store(self.was, Ordering::Relaxed);
    }
}

/// The slow-NAS fake (docs/agent-parts.md §7): every call on a path under a test's folder held
/// until the test lets the share answer again (`Hung` dropped), as a share stuck in the kernel
/// holds them. Test builds only.
#[cfg(any(test, feature = "testkit"))]
pub mod fake {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    type Gate = (String, Arc<AtomicBool>, Arc<AtomicUsize>, Option<Duration>);
    static HUNG: Mutex<Vec<Gate>> = Mutex::new(Vec::new());
    static ANY: AtomicUsize = AtomicUsize::new(0);

    thread_local! {
        /// The calls this thread made that a hung share held (`held_here`).
        static HELD_HERE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// The share hung under a folder while this lives; it counts the calls it held (`held`).
    pub struct Hung(String, Arc<AtomicBool>, Arc<AtomicUsize>);

    impl Hung {
        /// The calls under its folder that came while it hung.
        pub fn held(&self) -> usize {
            self.2.load(Ordering::SeqCst)
        }

        /// Its calls given up after `d` at most, not their own deadline: a test whose loop goes
        /// round on the hung share needn't wait out real deadlines. Only where the claim isn't how
        /// long a call waits.
        pub fn deadline(self, d: Duration) -> Hung {
            let mut h = super::lock(&HUNG);
            for g in h.iter_mut().filter(|g| g.0 == self.0 && Arc::ptr_eq(&g.1, &self.1)) {
                g.3 = Some(d);
            }
            drop(h);
            self
        }
    }

    impl Drop for Hung {
        fn drop(&mut self) {
            self.1.store(true, Ordering::SeqCst);
            let mut h = super::lock(&HUNG);
            h.retain(|(p, g, _, _)| !(p == &self.0 && Arc::ptr_eq(g, &self.1)));
            ANY.store(h.len(), Ordering::SeqCst);
        }
    }

    /// The calls this thread made that a hung share held, so far: a test's "it made no NAS call".
    pub fn held_here() -> usize {
        HELD_HERE.with(|c| c.get())
    }

    /// Every NAS call on a path under `folder` hangs until the answer is dropped.
    pub fn hang(folder: &Path) -> Hung {
        let (g, n) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let p = PathBuf::from(folder).display().to_string();
        let mut h = super::lock(&HUNG);
        h.push((p.clone(), g.clone(), n.clone(), None));
        ANY.store(h.len(), Ordering::SeqCst);
        Hung(p, g, n)
    }

    /// The gate a call on `on` waits on, if it's under a hung folder, and the fake's deadline.
    pub(super) fn gate(on: &dyn std::fmt::Display) -> Option<(Arc<AtomicBool>, Option<Duration>)> {
        if ANY.load(Ordering::SeqCst) == 0 {
            return None;
        }
        let on = on.to_string();
        let h = super::lock(&HUNG);
        let (_, g, n, d) = h.iter().find(|(p, _, _, _)| Path::new(&on).starts_with(p))?;
        n.fetch_add(1, Ordering::SeqCst);
        HELD_HERE.with(|c| c.set(c.get() + 1));
        Some((g.clone(), *d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A call: its answer when it comes in time; past its deadline `TimedOut`, held until it ends;
    /// and a look that came back far later than asked (the process stopped) counts as one.
    #[test]
    fn a_call_counts_the_time_the_process_ran() {
        let d = tempfile::tempdir().unwrap();
        let on = d.path().display();
        assert_eq!(call(&on, &"read", ms(500), || Ok(7)).unwrap(), 7);
        let hung = fake::hang(d.path()).deadline(ms(50));
        let before = held();
        let e = call(&on, &"read", LONG, || Ok(1)).unwrap_err();
        assert!(e.kind() == io::ErrorKind::TimedOut && past_deadline(&e) && !refused(&e), "{e}");
        assert!(e.to_string().contains("no answer"), "{e}");
        assert!(!past_deadline(&io::Error::from_raw_os_error(libc::ETIMEDOUT)));
        assert!(held() > before, "held while it's in the kernel");
        drop(hung);
        crate::testkit::eventually("the held call ends", || held() == before);
        // (A look as asked, or a little late, counts as it took; one back after the process was
        // stopped (the guards' freeze, 30 minutes) counts as one look.)
        assert_eq!(counted(LOOK, LOOK), LOOK);
        assert_eq!(counted(ms(1300), LOOK), ms(1300));
        assert_eq!(counted(Duration::from_secs(1800), LOOK), LOOK);
    }

    /// A thread's calls run on one helper, kept; the call past its deadline took it, and the next
    /// is a new one's. A call that panics loses its helper too, and the next goes on. A call made
    /// inside another runs on the same helper, inline.
    #[test]
    fn a_thread_keeps_its_helper() {
        let d = tempfile::tempdir().unwrap();
        let on = d.path().display();
        let here = || call(&on, &"t", ms(500), || Ok(std::thread::current().id())).unwrap();
        let (a, b) = (here(), here());
        assert!(a == b && a != std::thread::current().id());
        let nested = call(&on, &"outer", LONG, || call(&"inner", &"t", LONG, || Ok(std::thread::current().id())));
        assert_eq!(nested.unwrap(), a);
        let hung = fake::hang(d.path()).deadline(ms(20));
        call(&on, &"t", LONG, || Ok(())).unwrap_err();
        drop(hung);
        let c = here();
        assert!(c != a && here() == c);
        assert!(call::<()>(&on, &"t", ms(500), || panic!("(a test's panic)")).is_err());
        assert!(here() != c);
    }

    /// The cap: while as many calls as it are held, a call is refused at once (never made), counted
    /// missed; once one ends, calls go again.
    #[test]
    fn held_calls_are_capped() {
        let d = tempfile::tempdir().unwrap();
        let on = d.path().display();
        let _cap = cap_for_test(held() + 2);
        let hung = fake::hang(d.path()).deadline(ms(20));
        for _ in 0..2 {
            assert!(past_deadline(&call(&on, &"t", LONG, || Ok(())).unwrap_err()));
        }
        let missed0 = missed();
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let r = ran.clone();
        let t = Instant::now();
        let e = call(&"elsewhere", &"t", LONG, move || Ok(r.store(true, Ordering::SeqCst))).unwrap_err();
        assert!(refused(&e) && !past_deadline(&e) && e.kind() == io::ErrorKind::TimedOut, "{e}");
        assert!(e.to_string().contains("NAS calls held"), "{e}");
        crate::testkit::ended_before(t, LONG, "refused at once");
        assert!(missed() > missed0 && !ran.load(Ordering::SeqCst));
        drop(hung);
        crate::testkit::eventually("the held calls end", || call(&"elsewhere", &"t", LONG, || Ok(())).is_ok());
    }

    /// A write left past its deadline holds its paths: another write on one of them is refused at
    /// once while it's in the kernel, a read of it and a write elsewhere go on; once it ends, the
    /// path is free again.
    #[test]
    fn a_held_write_holds_its_path() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        let hung = fake::hang(&p).deadline(ms(20));
        let e = writing(&p.display(), &"write", LONG, vec![p.clone()], || Ok(())).unwrap_err();
        assert!(past_deadline(&e), "{e}");
        let e = writing(&"another", &"rename", LONG, vec![d.path().join("g"), p.clone()], || Ok(())).unwrap_err();
        assert!(refused(&e) && e.to_string().contains("still in the kernel"), "{e}");
        assert_eq!(call(&"another", &"read", LONG, || Ok(3)).unwrap(), 3);
        writing(&"another", &"write", LONG, vec![d.path().join("g")], || Ok(())).unwrap();
        drop(hung);
        crate::testkit::eventually("the path free again", || {
            writing(&"another", &"write", LONG, vec![p.clone()], || Ok(())).is_ok()
        });
    }

    /// A call that says it moved has its deadline counted from its last move: a copy moving for
    /// longer than its deadline ends; one that stops moving is given up at its deadline from then.
    #[test]
    fn a_moving_call_isnt_ended_for_its_length() {
        let n = watched(&"copy", &"t", ms(1500), Vec::new(), |p| {
            for _ in 0..8 {
                std::thread::sleep(ms(400));
                p.add(1);
            }
            Ok(8)
        });
        assert_eq!(n.unwrap(), 8);
        let (tx, rx) = mpsc::channel::<()>();
        let e = watched(&"copy", &"t", ms(300), Vec::new(), move |p| {
            p.add(1);
            rx.recv().map_err(io::Error::other)
        })
        .unwrap_err();
        assert!(past_deadline(&e));
        drop(tx);
    }

    /// The cap as an agent gives it to its jobs, and as set in its folder.
    #[test]
    fn the_cap_reads_its_setting() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(cap_in(d.path()), None);
        std::fs::write(d.path().join(CAP_FILE), r#"{"n": 7, "by": "t", "at": 1}"#).unwrap();
        assert_eq!(cap_in(d.path()), Some(7));
        assert_eq!(cap_in(Path::new("/nonexistent")), None);
    }
}
