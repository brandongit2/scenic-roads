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
    pace: &'a Pace,
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

    /// Waits until fewer tasks run than are allowed, then counts this one. Only a thread holding
    /// none asks (`par_map`).
    pub fn permit(&self) -> Permit<'_> {
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
        Permit { pace: self }
    }

    /// The tasks running under permits now.
    pub fn active(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).active
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        HELD.with(|h| h.set(0));
        self.pace.state.lock().unwrap_or_else(|e| e.into_inner()).active -= 1;
        self.pace.freed.notify_one();
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

/// What the memory says of a check's threads (`allowed_now`).
#[derive(Clone, Copy, Debug)]
pub struct Memory {
    /// Its measured peak (MB), and the threads the run that measured it had, when that says.
    pub peak_mb: u64,
    pub width: Option<usize>,
    /// What it holds now (MB): its peak less this is the growth still to come.
    pub held_mb: u64,
    /// The memory free now (MB).
    pub free_mb: u64,
}

/// The threads a check may use now (the agent's rule, `Agent::tend_threads`): its `grant` at least
/// (at most `MOST`), and past it, one more for each core `cores` less `kept` for the owner and
/// `beside` given to other jobs leaves, but only while memory covers each extra thread. An extra
/// thread is counted at the measured peak over the threads that peak was measured at (a peak
/// measured on a narrower run is not spread over more threads than it had), the whole peak when
/// that width isn't known (one extra thread of a regions check can need about a region's worth);
/// and the check's growth still to come (its peak less what it holds now) is taken out of the free
/// memory first. With no peak measured yet, or no memory said, it holds at its grant.
pub fn allowed_now(cores: usize, kept: usize, beside: usize, grant: usize, memory: Option<Memory>) -> usize {
    let grant = grant.clamp(1, MOST);
    let extra = cores.saturating_sub(kept + beside).clamp(grant, MOST) - grant;
    let Some(m) = memory else { return grant };
    let per_thread = (m.peak_mb / m.width.unwrap_or(1).max(1) as u64).max(1);
    let spare = m.free_mb.saturating_sub(m.peak_mb.saturating_sub(m.held_mb));
    grant + extra.min((spare / per_thread) as usize)
}

/// `f` on each of `items`, in order, in parallel: each task under a permit, so no more run at once
/// than are allowed. Inside a task already under a permit it runs the items one after another on
/// that thread, within the permit's budget: a permit's holder never waits on a task that needs one,
/// and a nested operation no other thread can take beyond what is allowed.
pub fn par_map<I: Send, R: Send>(items: Vec<I>, f: impl Fn(I) -> R + Sync + Send) -> Vec<R> {
    use rayon::prelude::*;
    if HELD.with(|h| h.get()) > 0 {
        return items.into_iter().map(f).collect();
    }
    items
        .into_par_iter()
        .map(|x| {
            let _turn = permit();
            f(x)
        })
        .collect()
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

/// `permit`, unless this thread holds one already (then None: its work is within that one's).
pub fn permit_unless_held() -> Option<Permit<'static>> {
    (HELD.with(|h| h.get()) == 0).then(permit)
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
        let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|s| {
            let pace = &pace;
            let hold = s.spawn(move || {
                let _p = pace.permit();
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            held_rx.recv().unwrap();
            std::fs::write(&f, "3").unwrap();
            let seen = s.spawn(move || {
                let _p = pace.permit();
                pace.active()
            });
            assert_eq!(seen.join().unwrap(), 2);
            release_tx.send(()).unwrap();
            hold.join().unwrap();
        });
    }

    #[test]
    fn threads_widen_on_free_cores_while_memory_covers_each_extra() {
        // 14 cores, none kept; a peak of 4,000 MB measured at 1 thread, all of it held now.
        let mem = |width, held, free| Some(Memory { peak_mb: 4000, width, held_mb: held, free_mb: free });
        let now = |kept, beside, grant, memory| allowed_now(14, kept, beside, grant, memory);
        // Cores free and memory plenty: up to MOST.
        assert_eq!(now(0, 0, 1, mem(Some(1), 4000, 100_000)), MOST);
        // Memory for two more threads (8,000 MB): three.
        assert_eq!(now(0, 0, 1, mem(Some(1), 4000, 8500)), 3);
        // Memory short, or no peak measured: the grant.
        assert_eq!(now(0, 0, 1, mem(Some(1), 4000, 3000)), 1);
        assert_eq!(now(0, 0, 2, None), 2);
        // Cores taken by the owner and other jobs: only the rest widen it (14 - 2 - 9 = 3 left).
        assert_eq!(now(2, 9, 1, mem(Some(1), 4000, 100_000)), 3);
        // Never below the grant, whatever the cores.
        assert_eq!(now(2, 12, 4, mem(Some(1), 4000, 100_000)), 4);
    }

    #[test]
    fn the_peak_is_spread_over_the_width_it_was_measured_at_or_not_at_all() {
        let now = |width, held, free| {
            allowed_now(14, 0, 0, 2, Some(Memory { peak_mb: 8000, width, held_mb: held, free_mb: free }))
        };
        // Measured at 2 threads: 4,000 MB a thread, so 4,500 free covers one more.
        assert_eq!(now(Some(2), 8000, 4500), 3);
        // Measured at 8 threads: 1,000 a thread, so 4,500 covers four more.
        assert_eq!(now(Some(8), 8000, 4500), 6);
        // Width unknown: the whole peak for each extra, so 4,500 covers none and 17,000 two.
        assert_eq!(now(None, 8000, 4500), 2);
        assert_eq!(now(None, 8000, 17_000), 4);
    }

    #[test]
    fn growth_still_to_come_is_taken_out_of_the_free_memory_first() {
        let now = |held, free| {
            allowed_now(14, 0, 0, 1, Some(Memory { peak_mb: 8000, width: Some(1), held_mb: held, free_mb: free }))
        };
        // Holding all its peak: 17,000 free covers two extra of 8,000.
        assert_eq!(now(8000, 17_000), 3);
        // Holding 2,000 of 8,000: 6,000 more to come first, leaving 11,000: one extra.
        assert_eq!(now(2000, 17_000), 2);
        // Holding nothing yet: all 8,000 to come, leaving 9,000: one extra.
        assert_eq!(now(0, 17_000), 2);
        // Growth alone using up the free memory: the grant.
        assert_eq!(now(0, 7000), 1);
    }

    #[test]
    fn a_parallel_operation_inside_a_task_runs_in_order_on_its_thread() {
        // Two pool threads, eight outer tasks each with four inner: the inner run on the outer's
        // thread, one after another (nothing waits on a permit while holding one: this ends).
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let seen = pool.install(|| {
            par_map((0..8).collect(), |o: usize| {
                let me = std::thread::current().id();
                let inner = par_map((0..4).collect(), |i: usize| (i, std::thread::current().id()));
                assert!(inner.iter().enumerate().all(|(k, (i, t))| k == *i && *t == me));
                o
            })
        });
        assert_eq!(seen, (0..8).collect::<Vec<_>>());
        assert_eq!(global().active(), 0);
    }
}
