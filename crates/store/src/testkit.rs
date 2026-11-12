//! What a test on the real clock waits with (task #662; tools/dev/testing.md, "A test that can't
//! flake"). Test builds only: `#[cfg(test)]` here, the `testkit` feature for the other crates'
//! tests (their dev-dependency on store turns it on).
//!
//! A test never waits a time of its own choosing for something to happen: a loaded test Mac (the
//! suite's three runs at nice 20, builds beside them) takes seconds for what takes milliseconds
//! idle. It waits for the event, under one watchdog ([`WATCHDOG`]) that only turns a hang into a
//! failure:
//! - [`eventually`] / [`wait_for`]: poll a condition (or a look-up) until it holds;
//! - [`Signal`]: one thread (or a stand-in's callback) says "done", another waits for it;
//! - [`sh`]: a stand-in child that says when it's set up (`$READY`) and is killed whole, with its
//!   process group, when the test drops it, panicking or not;
//! - [`ended_before`]: that code did NOT wait, judged against what waiting would take;
//! - [`thread_cpu_ns`]: what a call costs, by this thread's CPU time, never the wall clock;
//! - [`EnvVar`]: an environment variable set for one test, restored when it ends;
//! - [`ran_dir`]: a folder for a program the test runs, kept 10 minutes after (never a tempdir).
//!
//! Code whose decisions turn on time takes a [`crate::clock::Clock`]; its tests give it a
//! [`crate::clock::Virtual`] one and never sleep at all.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a test waits for something to happen before it fails: a watchdog, not a measure. Never
/// reached by a test that passes, so it can be long; tools/app/test_timing.py refuses a test's own
/// short one (a deadline under a minute on a wait). Under nextest's bound on a whole test (3 min,
/// .config/nextest.toml), so a waiting test fails itself first, saying what for.
pub const WATCHDOG: Duration = Duration::from_secs(120);

/// How often [`eventually`] and [`wait_for`] look again.
const POLL: Duration = Duration::from_millis(10);

/// Polls `done` until it holds, failing past [`WATCHDOG`] (saying `what` it waited for).
#[track_caller]
pub fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    wait_for(what, || done().then_some(()))
}

/// Polls `f` until it gives something, and returns it; fails past [`WATCHDOG`] (saying `what`).
#[track_caller]
pub fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + WATCHDOG;
    loop {
        if let Some(x) = f() {
            return x;
        }
        assert!(Instant::now() < end, "never: {what} (in {WATCHDOG:?})");
        std::thread::sleep(POLL);
    }
}

/// This thread's CPU time in ns: what "it costs little" is measured by (task #921). A loaded Mac
/// stretches the wall clock a loop takes, not the work it does; the process's clock would count
/// the other tests' threads.
pub fn thread_cpu_ns() -> u64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid timespec to write to.
    assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) }, 0);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

/// For a test that checks code didn't wait: asserts what began at `t0` ended well before `never`,
/// the time the code would have taken had it waited (its deadline, the stuck call's sleep, its
/// retries). Half of that, never a fixed "fast" bound such as 100 ms: those measured the machine.
#[track_caller]
pub fn ended_before(t0: Instant, never: Duration, what: &str) {
    let took = t0.elapsed();
    assert!(took < never / 2, "{what}: took {took:?}, as if it waited ({never:?})");
}

/// A one-way "it happened": cloned to whoever says it ([`Signal::set`]), waited on by the test
/// ([`Signal::wait`], under [`WATCHDOG`]). No polling, no time of its own.
#[derive(Clone, Default)]
pub struct Signal(Arc<(Mutex<bool>, Condvar)>);

impl Signal {
    pub fn new() -> Signal {
        Signal::default()
    }

    pub fn set(&self) {
        *self.0 .0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.0 .1.notify_all();
    }

    pub fn is_set(&self) -> bool {
        *self.0 .0.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[track_caller]
    pub fn wait(&self, what: &str) {
        let g = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = self.0 .1.wait_timeout_while(g, WATCHDOG, |set| !*set).unwrap_or_else(|e| e.into_inner());
        assert!(*g, "never: {what} (in {WATCHDOG:?})");
    }
}

/// A test's stand-in child: `/bin/sh -c script args…` in a process group of its own. The script
/// makes the file `$READY` names once it's set up (`: >"$READY"`) and [`TestChild::ready`] waits
/// for that, never a time. Dropped (the test ending, a panic included), its whole group is killed
/// and the child reaped: no `sleep` it started outlives the test (#654's 16 h stand-in).
pub fn sh(script: &str, args: &[&OsStr]) -> io::Result<TestChild> {
    use std::os::unix::process::CommandExt;
    static N: AtomicU64 = AtomicU64::new(0);
    let ready = std::env::temp_dir().join(format!(
        "testkit-ready-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let mut cmd = crate::spawn::command("/bin/sh");
    cmd.arg("-c").arg(script).args(args).env("READY", &ready).process_group(0);
    let child = crate::spawn::spawn(&mut cmd)?;
    Ok(TestChild { child, ready })
}

pub struct TestChild {
    child: Child,
    ready: PathBuf,
}

impl TestChild {
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Waits until the script said it's set up (made `$READY`); fails if it ends first.
    #[track_caller]
    pub fn ready(&mut self) {
        let what = format!("child {} ready", self.child.id());
        let ready = self.ready.clone();
        let child = &mut self.child;
        wait_for(&what, || {
            if ready.exists() {
                return Some(());
            }
            if let Ok(Some(s)) = child.try_wait() {
                panic!("child ended before it was ready: {s}");
            }
            None
        });
    }
}

impl Drop for TestChild {
    fn drop(&mut self) {
        // SAFETY: signals the group this child leads (made by process_group(0); its id is the
        // child's, which isn't reaped yet, so it can't be reused).
        unsafe { libc::killpg(self.child.id() as i32, libc::SIGKILL) };
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.ready);
    }
}

/// How long a folder a program ran from is kept once its test process was first seen gone
/// ([`ran_dir`]; the same as tools/dev/ran.zsh's `RAN_KEEP_S`, which the cargo shim's clones go by:
/// tools/dev/test/ran.zsh checks).
pub const RAN_KEEP: Duration = Duration::from_secs(600);

/// A new folder for a program the test copies or builds and then runs (#1131; tools/dev/testing.md,
/// "A program that ran is kept 10 minutes"): `$TMPDIR/scenic-ran/<pid>.<name>.<n>`, never removed by
/// the test. Gatekeeper scans a program's file after it starts, and syspolicyd crashed whenever the
/// file was gone by then (a tempdir dropped as the test ended: appupdate's prune test, 15 of 17
/// crashes on 10-11 Nov); while it restarts no process starts on the Mac. Each call first cleans the
/// folder as tools/dev/ran.zsh's `ran_clean` does: a folder whose process is gone is marked
/// (`.scenic-gone`) when first seen so, and removed once marked for [`RAN_KEEP`] (the folder's own
/// time says nothing of when a program last ran from it).
pub fn ran_dir(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join("scenic-ran");
    std::fs::create_dir_all(&root).unwrap();
    for e in std::fs::read_dir(&root).unwrap().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.split('.').next()?.parse::<i32>().ok()) else {
            continue;
        };
        // SAFETY: signal 0 only asks whether the process exists (EPERM: it does, another user's).
        let gone = unsafe { libc::kill(pid, 0) } != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if !gone || !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let mark = e.path().join(".scenic-gone");
        match std::fs::metadata(&mark).and_then(|m| m.modified()) {
            Ok(t) if t.elapsed().is_ok_and(|a| a >= RAN_KEEP) => {
                let _ = std::fs::remove_dir_all(e.path());
            }
            Ok(_) => {}
            Err(_) => {
                let _ = std::fs::File::create(&mark);
            }
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    let d = root.join(format!("{}.{name}.{}-{nanos}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&d).unwrap();
    d
}

/// An environment variable set for one test, put back as it was when dropped (a panic included).
/// A test's variables hold a process-wide turn while any is set (a test may set several), so two
/// tests setting variables take turns. Only for a variable no other test in the binary reads:
/// better, pass the value to the code under test.
pub struct EnvVar {
    key: OsString,
    was: Option<OsString>,
    /// Not Send: dropped on the thread that set it, where the turn is kept.
    _here: std::marker::PhantomData<*const ()>,
}

static ENV: Mutex<()> = Mutex::new(());

thread_local! {
    /// This thread's live EnvVars and, while any is, the turn: taken by the first, given back with
    /// the last (in whatever order they're dropped).
    static HERE: std::cell::RefCell<(usize, Option<std::sync::MutexGuard<'static, ()>>)> =
        const { std::cell::RefCell::new((0, None)) };
}

impl EnvVar {
    pub fn set(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> EnvVar {
        HERE.with_borrow_mut(|(n, turn)| {
            if *n == 0 {
                *turn = Some(ENV.lock().unwrap_or_else(|e| e.into_inner()));
            }
            *n += 1;
        });
        let key = key.as_ref().to_owned();
        let was = std::env::var_os(&key);
        std::env::set_var(&key, value);
        EnvVar { key, was, _here: std::marker::PhantomData }
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match &self.was {
            Some(v) => std::env::set_var(&self.key, v),
            None => std::env::remove_var(&self.key),
        }
        HERE.with_borrow_mut(|(n, turn)| {
            *n -= 1;
            if *n == 0 {
                *turn = None;
            }
        });
    }
}

/// The recorded chain of real catalogues (`src/map/testdata`: 798 to 809 of 30 Oct 2026, trimmed),
/// lowest first.
pub fn recorded_chain() -> Vec<crate::catalogue::Catalogue> {
    // (From the test's folder, its package's, as cargo and nextest run it: env!("CARGO_MANIFEST_DIR")
    // compiled one checkout's path into every test build of the shared build folder, #1123.)
    let dir = std::path::absolute("../store/src/map/testdata").expect("the test's folder");
    let mut ns: Vec<u64> = std::fs::read_dir(&dir)
        .expect("the recorded chain")
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json.zst")?.parse().ok())
        .collect();
    ns.sort_unstable();
    ns.into_iter()
        .map(|n| {
            let b = std::fs::read(dir.join(format!("{n}.json.zst"))).expect("a recorded catalogue");
            crate::catalogue::Catalogue::decode(&b).expect("a recorded catalogue decodes")
        })
        .collect()
}

/// Writes `cat` as the map of a test root (`root`, laid out as the NAS's project folder): its
/// `map/<seq>.json.zst` (crate::map::write_root_map; `seq` its `n`), which a reader on that root
/// takes with no lead (coordwire::map_now, a `--root` server).
pub fn map_root(root: &std::path::Path, cat: &crate::catalogue::Catalogue) {
    let m = crate::map::Map::of(cat).expect("a test catalogue is a map");
    crate::map::write_root_map(root, &m).expect("the test root's map");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_wakes_its_waiter_and_a_childs_group_dies_with_it() {
        let s = Signal::new();
        let s2 = s.clone();
        let t = std::thread::spawn(move || s2.set());
        s.wait("the thread's signal");
        t.join().unwrap();
        assert!(s.is_set());
        // The child's own child (the sleep) is in its group: both gone once it's dropped.
        let mut c = sh("sleep 600 & : >\"$READY\"; wait", &[]).unwrap();
        c.ready();
        let pid = c.id() as i32;
        drop(c);
        // SAFETY: probing with signal 0 sends nothing.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "the child reaped");
        // (The orphaned sleep is reaped by launchd, in its own time.)
        eventually("nothing left in its group", || unsafe { libc::killpg(pid, 0) } == -1);
        assert_eq!(wait_for("a value", || Some(7)), 7);
    }

    #[test]
    fn an_env_var_is_put_back() {
        let k = "SCENIC_TESTKIT_PROBE";
        {
            let _v = EnvVar::set(k, "1");
            let _w = EnvVar::set("SCENIC_TESTKIT_PROBE_2", "2");
            assert_eq!(std::env::var(k).as_deref(), Ok("1"));
            // The first dropped first: the second still holds the turn.
            drop(_v);
            assert!(std::env::var_os(k).is_none());
            let other = std::thread::spawn(|| {
                let _x = EnvVar::set("SCENIC_TESTKIT_PROBE_3", "3");
                std::env::var_os("SCENIC_TESTKIT_PROBE_2")
            });
            drop(_w);
            assert_eq!(other.join().unwrap(), None, "another thread waited for the turn");
        }
        assert!(std::env::var_os(k).is_none() && std::env::var_os("SCENIC_TESTKIT_PROBE_2").is_none());
    }

    /// A ran folder goes only once its process has been seen gone for RAN_KEEP: marked when first
    /// seen gone (however old the folder), removed once the mark is that old; a live process's never.
    #[test]
    fn a_ran_folder_goes_once_its_process_has_been_seen_gone_long_enough() {
        let mut c = crate::spawn::command("/usr/bin/true");
        let mut child = crate::spawn::spawn(&mut c).unwrap();
        let gone = child.id();
        child.wait().unwrap();
        let root = std::env::temp_dir().join("scenic-ran");
        std::fs::create_dir_all(&root).unwrap();
        let long_ago = std::time::SystemTime::now() - RAN_KEEP - Duration::from_secs(60);
        let folder = |name: String, marked: Option<std::time::SystemTime>| {
            let p = root.join(name);
            std::fs::create_dir_all(&p).unwrap();
            if let Some(t) = marked {
                std::fs::File::create(p.join(".scenic-gone")).unwrap().set_modified(t).unwrap();
            }
            std::fs::File::open(&p).unwrap().set_modified(long_ago).unwrap();
            p
        };
        let seen_long_ago = folder(format!("{gone}.testkit-old.0"), Some(long_ago));
        let unseen = folder(format!("{gone}.testkit-unseen.0"), None);
        let seen_now = folder(format!("{gone}.testkit-now.0"), Some(std::time::SystemTime::now()));
        let live = folder(format!("{}.testkit-live.0", std::process::id()), Some(long_ago));
        let d = ran_dir("testkit");
        assert!(d.is_dir() && d.starts_with(&root));
        assert!(d.file_name().unwrap().to_str().unwrap().starts_with(&format!("{}.testkit.", std::process::id())));
        assert!(!seen_long_ago.exists(), "a gone process's, seen gone long ago: removed");
        assert!(unseen.join(".scenic-gone").exists(), "a gone process's, an old folder first seen gone: marked");
        assert!(seen_now.exists(), "a gone process's, seen gone just now: kept");
        assert!(live.exists(), "a live process's, marked long ago: kept");
        for p in [d, unseen, seen_now, live] {
            std::fs::remove_dir_all(p).unwrap();
        }
    }
}
