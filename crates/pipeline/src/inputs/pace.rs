//! How many threads a check runs (docs/inputs.md §4.3, Threads): the count its agent says now, which
//! rises as cores free up, and falls when they are wanted.
//!
//! A job's threads are set as it starts (`RAYON_NUM_THREADS`), and a check lasts hours: a Mac that
//! was busy at the start leaves it on one core long after the rest are idle. So the agent keeps a
//! file (`THREADS_ENV`) with the threads the check may use at the moment (the Mac's cores, less those
//! kept for its owner and those its other jobs were given), and the check's pools are as wide as
//! `width`, their tasks each taking a `permit` for as long as they run: no more tasks run at once
//! than the file says, whichever threads the pool has. A raise is seen within `POLL`.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The environment variable naming the file the agent keeps the threads allowed now in.
pub const THREADS_ENV: &str = "SCENIC_THREADS_FILE";

/// The most threads a check is ever allowed: the regions' work held four at once before (each
/// operation's memory grows with its region), and the lead placed the check with memory for its grant.
pub const MOST: usize = 8;

/// How often the file is read again, and how long a task waits for a permit before looking again.
pub const POLL: Duration = Duration::from_secs(1);

thread_local! {
    /// The permits this thread holds: a task it runs inside another (a pool thread waiting on a join
    /// runs others' tasks) takes none, so a permit's holder never waits on a permit.
    static HELD: Cell<usize> = const { Cell::new(0) };
}

struct State {
    active: usize,
    allowed: usize,
    read_at: Option<Instant>,
}

/// The threads allowed, and the tasks running under them.
pub struct Pace {
    file: Option<PathBuf>,
    /// The threads allowed when there is no file (or it can't be read): a hand run's, all the cores.
    fallback: usize,
    state: Mutex<State>,
    freed: Condvar,
}

/// A task's right to run (`Pace::permit`), given back as it drops.
pub struct Permit<'a> {
    pace: Option<&'a Pace>,
}

impl Pace {
    pub fn new(file: Option<PathBuf>, fallback: usize) -> Pace {
        let fallback = fallback.max(1);
        Pace {
            file,
            fallback,
            state: Mutex::new(State { active: 0, allowed: fallback, read_at: None }),
            freed: Condvar::new(),
        }
    }

    fn refresh(&self, s: &mut State) {
        if s.read_at.is_some_and(|t| t.elapsed() < POLL) {
            return;
        }
        s.read_at = Some(Instant::now());
        s.allowed = self
            .file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| t.trim().parse::<usize>().ok())
            .unwrap_or(self.fallback)
            .clamp(1, MOST);
    }

    /// The threads allowed now.
    pub fn allowed(&self) -> usize {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.refresh(&mut s);
        s.allowed
    }

    /// Waits until fewer tasks run than are allowed, then counts this one (a thread already running
    /// one takes none more).
    pub fn permit(&self) -> Permit<'_> {
        if HELD.with(|h| h.get()) > 0 {
            HELD.with(|h| h.set(h.get() + 1));
            return Permit { pace: None };
        }
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            self.refresh(&mut s);
            if s.active < s.allowed {
                break;
            }
            s = self.freed.wait_timeout(s, POLL).unwrap_or_else(|e| e.into_inner()).0;
        }
        s.active += 1;
        HELD.with(|h| h.set(1));
        Permit { pace: Some(self) }
    }

    /// The tasks running under permits now.
    pub fn active(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).active
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        HELD.with(|h| h.set(h.get().saturating_sub(1)));
        if let Some(p) = self.pace {
            p.state.lock().unwrap_or_else(|e| e.into_inner()).active -= 1;
            p.freed.notify_one();
        }
    }
}

fn global() -> &'static Pace {
    static PACE: OnceLock<Pace> = OnceLock::new();
    PACE.get_or_init(|| Pace::new(std::env::var_os(THREADS_ENV).map(PathBuf::from), cores()))
}

/// The Mac's cores.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// How wide a check's pool is: the cores up to `MOST` (a hand run's `SCENIC_REGIONS_THREADS` says
/// otherwise); `permit` holds the tasks running to the threads allowed.
pub fn width() -> usize {
    std::env::var("SCENIC_REGIONS_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(cores().min(MOST)).max(1)
}

/// A pool `width` wide for a check's parallel work (the process's own, `RAYON_NUM_THREADS`, is the
/// threads it was granted at its start), its threads run the translations' checker as the calling
/// thread's do (`lint::here`).
pub fn pool() -> anyhow::Result<rayon::ThreadPool> {
    let lint = super::lint::here();
    Ok(rayon::ThreadPoolBuilder::new().num_threads(width()).start_handler(move |_| lint.install()).build()?)
}

/// The right to run a task, under the threads allowed now (the file the agent keeps).
pub fn permit() -> Permit<'static> {
    global().permit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn file(n: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("threads");
        std::fs::write(&f, n).unwrap();
        (d, f)
    }

    #[test]
    fn the_file_says_the_threads_and_a_missing_or_odd_one_the_fallback() {
        let (_d, f) = file("3\n");
        assert_eq!(Pace::new(Some(f), 6).allowed(), 3);
        let (_d, f) = file("many");
        assert_eq!(Pace::new(Some(f), 6).allowed(), 6);
        assert_eq!(Pace::new(Some("/nonexistent/threads".into()), 6).allowed(), 6);
        assert_eq!(Pace::new(None, 6).allowed(), 6);
        // (At least one, at most MOST.)
        let (_d, f) = file("0");
        assert_eq!(Pace::new(Some(f), 6).allowed(), 1);
        let (_d, f) = file("99");
        assert_eq!(Pace::new(Some(f), 6).allowed(), MOST);
    }

    #[test]
    fn no_more_tasks_run_at_once_than_are_allowed_and_a_raise_is_seen() {
        let (_d, f) = file("1");
        let pace = Pace::new(Some(f.clone()), 1);
        let (now, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..3 {
                        let _p = pace.permit();
                        let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(n, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        now.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });
        assert_eq!(most.load(Ordering::SeqCst), 1);
        assert_eq!(pace.active(), 0);
        // Raised while a task waits: it goes on at the new count, beside the one holding.
        std::thread::scope(|s| {
            let hold = s.spawn(|| {
                let _p = pace.permit();
                std::thread::sleep(Duration::from_millis(3500));
            });
            std::thread::sleep(Duration::from_millis(200));
            std::fs::write(&f, "3").unwrap();
            let seen = s.spawn(|| {
                let _p = pace.permit();
                pace.active()
            });
            assert_eq!(seen.join().unwrap(), 2);
            hold.join().unwrap();
        });
    }

    #[test]
    fn a_task_inside_another_on_a_thread_takes_no_permit() {
        let pace = Pace::new(None, 1);
        let outer = pace.permit();
        let inner = pace.permit();
        assert_eq!(pace.active(), 1);
        drop(inner);
        assert_eq!(pace.active(), 1);
        drop(outer);
        assert_eq!(pace.active(), 0);
    }
}
