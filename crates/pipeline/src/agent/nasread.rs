//! The agent loop's small NAS reads, each with a deadline (docs/pool.md §7.8, Task #380): a switch
//! file, a member's progress file, a small record read each loop. An open on the SMB share can stay
//! stuck in the kernel for many minutes while the same file reads at once from a shell (18 Oct:
//! 17 minutes in `resume::on`, no heartbeat meanwhile); the loop must keep beating.
//!
//! Each read runs on one of a few reader threads (`READERS`) while the caller waits `DEADLINE` at
//! most. A read past it returns `ErrorKind::TimedOut` to the caller, which takes it as it takes any
//! file it can't read now (its own "can't read" path), and is logged once per path per streak (a
//! read of that path that answers ends the streak). The thread stuck in the read is left to it and
//! counts as busy until its read returns; while every reader is busy, a read fails at once (also
//! `TimedOut`), so stuck threads never pile up past `READERS`. A healthy share answers well inside
//! the deadline: the result is the read's own, as before.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How long a small read may take before its caller goes on without it.
pub const DEADLINE: Duration = Duration::from_secs(20);
/// The reader threads (the most that can be stuck at once).
pub const READERS: usize = 4;

type Job = Box<dyn FnOnce() + Send>;

/// A small pool of reader threads, `n` at most busy at once.
pub struct Readers {
    n: usize,
    busy: Arc<AtomicUsize>,
    tx: Mutex<Sender<Job>>,
}

impl Readers {
    pub fn new(n: usize) -> Readers {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..n {
            let rx: Arc<Mutex<Receiver<Job>>> = rx.clone();
            std::thread::Builder::new()
                .name(format!("nas-read-{i}"))
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
                .expect("spawn a NAS reader");
        }
        Readers { n, busy: Arc::new(AtomicUsize::new(0)), tx: Mutex::new(tx) }
    }

    /// `f` run on a reader, its result if it returns within `deadline`; else `TimedOut` (also at
    /// once while every reader is busy). `what` names the call in the error.
    pub fn call<T: Send + 'static>(
        &self,
        what: &Path,
        deadline: Duration,
        f: impl FnOnce() -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        // A reader taken, or none free: fail fast.
        if self.busy.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |b| (b < self.n).then_some(b + 1)).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{}: every NAS reader is stuck in an earlier read", what.display()),
            ));
        }
        let (rtx, rrx) = mpsc::sync_channel(1);
        let busy = self.busy.clone();
        let job: Job = Box::new(move || {
            let r = f();
            busy.fetch_sub(1, Ordering::SeqCst);
            rtx.send(r).ok();
        });
        if self.tx.lock().map_err(|_| io::Error::other("NAS readers poisoned"))?.send(job).is_err() {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            return Err(io::Error::other("NAS readers gone"));
        }
        match rrx.recv_timeout(deadline) {
            Ok(r) => r,
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{}: no answer from the NAS in {} s", what.display(), deadline.as_secs()),
            )),
        }
    }

    /// The readers busy now (stuck ones included).
    pub fn busy(&self) -> usize {
        self.busy.load(Ordering::SeqCst)
    }
}

fn readers() -> &'static Readers {
    static R: OnceLock<Readers> = OnceLock::new();
    R.get_or_init(|| Readers::new(READERS))
}

/// The paths whose reads are timing out now (logged once per streak).
fn streaks() -> &'static Mutex<BTreeSet<PathBuf>> {
    static S: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Notes a read's outcome: a timeout logged as its path's streak begins, any answer ending it.
fn note<T>(p: &Path, r: &io::Result<T>) {
    let Ok(mut s) = streaks().lock() else { return };
    match r {
        Err(e) if e.kind() == io::ErrorKind::TimedOut => {
            if s.insert(p.to_path_buf()) {
                eprintln!("agent: {e}; taken as unreadable for now, the loop goes on");
            }
        }
        _ => {
            if s.remove(p) {
                eprintln!("agent: {} reads again", p.display());
            }
        }
    }
}

/// The file at `p`, read within `DEADLINE` (`TimedOut` past it).
pub fn read(p: &Path) -> io::Result<Vec<u8>> {
    let owned = p.to_path_buf();
    let r = readers().call(p, DEADLINE, move || std::fs::read(owned));
    note(p, &r);
    r
}

/// The file at `p` as text, read within `DEADLINE` (`TimedOut` past it).
pub fn read_to_string(p: &Path) -> io::Result<String> {
    let owned = p.to_path_buf();
    let r = readers().call(p, DEADLINE, move || std::fs::read_to_string(owned));
    note(p, &r);
    r
}

/// The watchdog's file in the agent's folder while its loop is stalled (`Stall`).
pub const STALL_FILE: &str = "loop-stall.json";
/// A loop that hasn't finished a round in this long is stalled.
pub const STALL_AFTER: Duration = Duration::from_secs(300);

/// The loop stalled, as the watchdog saw it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stall {
    /// When the loop last finished a round (s since the epoch).
    pub round_at: u64,
    /// When the watchdog last looked (s since the epoch).
    pub seen_at: u64,
}

/// The loop's watchdog: a thread that looks every few seconds at when the loop last finished a
/// round (`round_at`, s since the epoch, stored by the loop); past `after`, it logs it once and
/// keeps `STALL_FILE` in `home` (this Mac's disk, never the NAS) up to date, removed when the loop
/// goes on. It ends once `done` is set.
pub fn watch(
    round_at: Arc<std::sync::atomic::AtomicU64>,
    done: Arc<std::sync::atomic::AtomicBool>,
    home: PathBuf,
    after: Duration,
    every: Duration,
) -> std::thread::JoinHandle<()> {
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
mod tests {
    #[test]
    fn the_watchdog_flags_a_stalled_loop_and_clears_it() {
        use std::sync::atomic::{AtomicBool, AtomicU64};
        let d = tempfile::tempdir().unwrap();
        let round = Arc::new(AtomicU64::new(crate::agent::now_s()));
        let done = Arc::new(AtomicBool::new(false));
        let t = watch(round.clone(), done.clone(), d.path().into(), Duration::from_secs(1), Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(2300));
        assert!(stall(d.path()).is_some(), "stalled");
        round.store(crate::agent::now_s(), Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        assert!(stall(d.path()).is_none(), "going on");
        done.store(true, Ordering::SeqCst);
        t.join().unwrap();
    }

    use super::*;
    use std::time::Instant;

    #[test]
    fn a_read_that_never_returns_times_out_within_the_deadline() {
        let r = Readers::new(2);
        let (hold, gate) = mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let t = Instant::now();
        let g = gate.clone();
        let e = r
            .call(Path::new("/nas/state/pool/resume"), Duration::from_millis(200), move || {
                g.lock().unwrap().recv().ok();
                Ok(())
            })
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(2), "within the deadline: {:?}", t.elapsed());
        // The stuck reader stays busy; another reader still answers.
        assert_eq!(r.busy(), 1);
        assert_eq!(r.call(Path::new("x"), Duration::from_secs(5), || Ok(7)).unwrap(), 7);
        // Every reader stuck: fail fast, nothing more spawned.
        let g2 = gate.clone();
        let _ = r.call(Path::new("y"), Duration::from_millis(100), move || {
            g2.lock().unwrap().recv().ok();
            Ok(())
        });
        assert_eq!(r.busy(), 2);
        let t = Instant::now();
        let e = r.call(Path::new("z"), Duration::from_secs(30), || Ok(())).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(t.elapsed() < Duration::from_millis(100), "at once");
        // The share answers again: the readers free.
        drop(hold);
        let t = Instant::now();
        while r.busy() > 0 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(r.busy(), 0);
        assert_eq!(r.call(Path::new("x"), Duration::from_secs(5), || Ok(1)).unwrap(), 1);
    }

    #[test]
    fn a_healthy_read_is_the_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s");
        assert_eq!(read_to_string(&p).unwrap_err().kind(), io::ErrorKind::NotFound);
        std::fs::write(&p, "on").unwrap();
        assert_eq!(read_to_string(&p).unwrap(), "on");
        assert_eq!(read(&p).unwrap(), b"on");
    }
}
