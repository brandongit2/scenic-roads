//! How many threads a check runs (docs/inputs.md §4.3, Threads): its job's threads now, which its
//! agent grows while they're all kept busy and cuts while they're left idle (docs/pool.md §7.1,
//! Threads controlled live).
//!
//! A check lasts hours, one target: its job's threads are taken as they change, not as a target
//! begins (jobrt::control::on_grant). The agent keeps a file (jobrt::control::THREADS_ENV, replaced
//! whole by jobrt::control::grant, so a read never meets it emptied and falls back to every core)
//! with the threads the job may use at the moment, and the check's pools are as wide as `width`
//! (the Mac's cores), their tasks each taking a `permit` for as long as they run, and the items of a
//! `par_each` theirs, lent to the parallel maps inside them while those run: no more tasks run at
//! once than the file says, whichever threads the pool has. A change is seen within `POLL`.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How often the file is read again, and how long a task waits for a permit before looking again.
pub const POLL: Duration = Duration::from_secs(1);

thread_local! {
    /// The permit this thread holds: none (`FREE`), a task's (`TASK`: a parallel operation inside
    /// it runs in order on it, so a permit's holder never waits on a permit), or an item's of
    /// `par_each` (`ITEM`: lent to a parallel operation inside it while that runs).
    static HELD: Cell<u8> = const { Cell::new(FREE) };
}

const FREE: u8 = 0;
const TASK: u8 = 1;
const ITEM: u8 = 2;

struct State {
    active: usize,
    allowed: usize,
    read_at: Option<Instant>,
    /// The items waiting to take back the permit they lent (`Pace::lend`): served before any task.
    back: usize,
}

/// The threads allowed, and the tasks running under them.
pub struct Pace {
    file: Option<PathBuf>,
    /// The threads allowed when there is no file (or it can't be read): a hand run's, all the cores.
    fallback: usize,
    state: Mutex<State>,
    freed: Condvar,
}

/// A task's right to run (`Pace::permit`), given back as it drops: what the thread held before
/// (`HELD`) held again.
pub struct Permit<'a> {
    pace: &'a Pace,
    before: u8,
}

impl Pace {
    pub fn new(file: Option<PathBuf>, fallback: usize) -> Pace {
        let fallback = fallback.max(1);
        Pace {
            file,
            fallback,
            state: Mutex::new(State { active: 0, allowed: fallback, read_at: None, back: 0 }),
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
            .max(1);
        // (Its cost line says the fewest its target ran on: buildstore::costin's `threads_least`.)
        if self.file.is_some() {
            jobrt::control::allowed(s.allowed);
        }
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
        self.take(TASK)
    }

    fn take(&self, held: u8) -> Permit<'_> {
        self.wait_and_count(false);
        Permit { pace: self, before: HELD.with(|h| h.replace(held)) }
    }

    /// Waits until fewer run than are allowed, then counts this one; `back`, an item taking back
    /// its permit, before every other waiting (its region's next step, between its cells' maps,
    /// would otherwise wait behind every cell of the others).
    fn wait_and_count(&self, back: bool) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.back += usize::from(back);
        loop {
            self.refresh(&mut s);
            if s.active < s.allowed && (back || s.back == 0) {
                break;
            }
            s = self.freed.wait_timeout(s, POLL).unwrap_or_else(|e| e.into_inner()).0;
        }
        s.back -= usize::from(back);
        s.active += 1;
        drop(s);
        // (The others see `back` fall.)
        if back {
            self.freed.notify_all();
        }
    }

    fn uncount(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).active -= 1;
        self.freed.notify_all();
    }

    /// `f`, a parallel operation inside a `par_each` item, with the item's permit lent to it (its
    /// tasks take theirs), and taken back after (however `f` ends: a panic too).
    fn lend<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Back<'a>(&'a Pace);
        impl Drop for Back<'_> {
            fn drop(&mut self) {
                self.0.wait_and_count(true);
                HELD.with(|h| h.set(ITEM));
            }
        }
        self.uncount();
        HELD.with(|h| h.set(FREE));
        let _back = Back(self);
        f()
    }

    /// The tasks running under permits now.
    pub fn active(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).active
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        HELD.with(|h| h.set(self.before));
        self.pace.uncount();
    }
}

fn global() -> &'static Pace {
    static PACE: OnceLock<Pace> = OnceLock::new();
    PACE.get_or_init(|| Pace::new(std::env::var_os(jobrt::control::THREADS_ENV).map(PathBuf::from), cores()))
}

/// The Mac's cores.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// How wide a check's pool is: the Mac's cores (a hand run's `SCENIC_REGIONS_THREADS` says
/// otherwise), so its agent may grow it to any of them; `permit` holds the tasks running to the
/// threads allowed.
pub fn width() -> usize {
    std::env::var("SCENIC_REGIONS_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(cores()).max(1)
}

/// `f` on each of `items`, in order, in parallel: each task under a permit, so no more run at once
/// than are allowed. Inside a task already under a permit it runs the items one after another on
/// that thread, within the permit's budget: a permit's holder never waits on a task that needs one,
/// and a nested operation no other thread can take beyond what is allowed.
pub fn par_map<I: Send, R: Send>(items: Vec<I>, f: impl Fn(I) -> R + Sync + Send) -> Vec<R> {
    global().par_map(items, f)
}

/// `f` on each of `items`, in order, in parallel, for items whose work is itself parallel maps (a
/// region's cells): each item under a permit, which it lends to each `par_map` inside it while that
/// runs (its tasks take theirs), so a large item's tasks spread over every thread allowed beside
/// the small items', and nothing runs beyond what is allowed. Inside a task or an item, one after
/// another (`par_map`'s way).
pub fn par_each<I: Send, R: Send>(items: Vec<I>, f: impl Fn(I) -> R + Sync + Send) -> Vec<R> {
    global().par_each(items, f)
}

impl Pace {
    /// `par_map` under this pace's permits: each task takes one as it starts, so an operation under
    /// way follows the file as it changes (more tasks at once after a raise, fewer after a cut, as
    /// the tasks running end). Inside an item of `par_each`, with the item's permit lent to it.
    pub fn par_map<I: Send, R: Send>(&self, items: Vec<I>, f: impl Fn(I) -> R + Sync + Send) -> Vec<R> {
        use rayon::prelude::*;
        let held = HELD.with(|h| h.get());
        if held == TASK {
            return items.into_iter().map(f).collect();
        }
        let each = || {
            items
                .into_par_iter()
                .map(|x| {
                    let _turn = self.permit();
                    f(x)
                })
                .collect()
        };
        if held == ITEM {
            self.lend(each)
        } else {
            each()
        }
    }

    /// `par_each` under this pace's permits.
    pub fn par_each<I: Send, R: Send>(&self, items: Vec<I>, f: impl Fn(I) -> R + Sync + Send) -> Vec<R> {
        use rayon::prelude::*;
        if HELD.with(|h| h.get()) != FREE {
            return items.into_iter().map(f).collect();
        }
        items
            .into_par_iter()
            .map(|x| {
                let _item = self.take(ITEM);
                f(x)
            })
            .collect()
    }
}

/// A pool `width` wide for a check's parallel work (the process's own, `RAYON_NUM_THREADS`, is the
/// threads it was granted at its start; the file says them now), its threads run the translations' checker as the calling
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
        // (At least one; as many as it says past that.)
        let (_d, f) = file("0");
        assert_eq!(Pace::new(Some(f), 6).allowed(), 1);
        let (_d, f) = file("12");
        assert_eq!(Pace::new(Some(f), 6).allowed(), 12);
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

    /// Task #1420: one long operation under way (a regions check's one target) follows its file as
    /// its agent changes it: on 1 at first, as many as 4 at once once raised, 2 at most once cut.
    /// (Its windows from when each change was written, plus the `POLL` it's seen within and a
    /// margin: on a loaded Mac a sleep runs long, 18 Nov at a load of 47.)
    #[test]
    fn an_operation_under_way_follows_its_file_up_and_down() {
        let (_d, f) = file("1");
        let pace = Pace::new(Some(f.clone()), 1);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        let t0 = Instant::now();
        let now = AtomicUsize::new(0);
        // (When the file said 4, and 2, ms from t0; and when the tasks stop.)
        let (raised, cut, done) = (AtomicUsize::new(usize::MAX), AtomicUsize::new(usize::MAX), AtomicUsize::new(0));
        let ms = || t0.elapsed().as_millis() as usize;
        let settle = (POLL + Duration::from_millis(500)).as_millis() as usize;
        // (Each task: when it began (ms) and how many ran with it.)
        let seen: Vec<(usize, usize)> = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(1200));
                // (Its time taken first: no task before it saw 4.)
                raised.store(ms(), Ordering::SeqCst);
                std::fs::write(&f, "4").unwrap();
                std::thread::sleep(Duration::from_millis(2500));
                std::fs::write(&f, "2").unwrap();
                cut.store(ms(), Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(settle as u64 + 1000));
                done.store(1, Ordering::SeqCst);
            });
            pool.install(|| {
                pace.par_map((0..100_000).collect(), |_: usize| {
                    if done.load(Ordering::SeqCst) == 1 {
                        return (0, 0);
                    }
                    let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                    let at = ms();
                    std::thread::sleep(Duration::from_millis(20));
                    now.fetch_sub(1, Ordering::SeqCst);
                    (at, n)
                })
            })
        });
        let (raised, cut) = (raised.into_inner(), cut.into_inner());
        let most =
            |from: usize, to: usize| seen.iter().filter(|(at, n)| *n > 0 && (from..to).contains(at)).map(|s| s.1).max();
        assert_eq!(most(0, raised), Some(1), "on 1 at first");
        assert_eq!(most(raised + settle, cut), Some(4), "raised: 4 at once, the pool's every thread");
        assert!(
            most(cut + settle, usize::MAX).is_some_and(|n| n <= 2),
            "cut: 2 at most, {:?}",
            most(cut + settle, usize::MAX)
        );
        assert_eq!(pace.active(), 0);
    }

    /// `par_each`'s items, their own work and their cells' together, never run on more threads than
    /// are allowed, on a pool three times as wide; and an item alone lends its permit to its cells,
    /// which then take every thread allowed.
    #[test]
    fn items_at_once_and_their_cells_stay_within_the_threads_allowed() {
        let (_d, f) = file("2");
        let pace = Pace::new(Some(f), 2);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(6).build().unwrap();
        let (now, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        // (Signalled as the most at once changes.)
        let (seen, rose) = (Mutex::new(()), Condvar::new());
        // (A piece of work: counted while it runs; `meet`, it waits, up to 2 s, for two to have run
        // at once.)
        let work = |meet: bool| {
            let n = now.fetch_add(1, Ordering::SeqCst) + 1;
            if most.fetch_max(n, Ordering::SeqCst) < n {
                let _g = seen.lock().unwrap();
                rose.notify_all();
            }
            std::thread::sleep(Duration::from_millis(5)); // (sleep ok: a piece of work's length)
            if meet {
                let g = seen.lock().unwrap();
                drop(rose.wait_timeout_while(g, Duration::from_secs(2), |_| most.load(Ordering::SeqCst) < 2).unwrap());
            }
            now.fetch_sub(1, Ordering::SeqCst);
        };
        let item = |cells: usize, meet: bool| {
            work(false);
            pace.par_map((0..cells).collect(), |_: usize| work(meet));
            work(false);
        };
        pool.install(|| pace.par_each((0..8).collect(), |_: usize| item(6, false)));
        assert!(most.load(Ordering::SeqCst) <= 2, "two at most: {}", most.load(Ordering::SeqCst));
        assert_eq!(pace.active(), 0);
        most.store(0, Ordering::SeqCst);
        pool.install(|| pace.par_each(vec![0], |_: usize| item(8, true)));
        assert_eq!(most.load(Ordering::SeqCst), 2, "the item's cells on both threads allowed");
        assert_eq!(pace.active(), 0);
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
