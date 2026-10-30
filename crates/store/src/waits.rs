//! The wall time this process spends blocked in I/O, measured where it blocks (task #870): every
//! NAS call (crate::nasfs's paths and files, crate::range's NAS reads, crate::iopool's operations,
//! crate::naming's and crate::blobs' copies), every network request (pipeline::fetch's `Net`, the
//! coordinator's client and the other HTTP callers) and the local disk calls the store makes
//! (crate::cachefile, crate::range's local files, the NAS's copies to this Mac's disk count as the
//! NAS's), and the waits on other work a job's code says it blocks in (`other()`: another worker's
//! subtrees, a thread copying ahead: a wait on a condition or a join, which no I/O call shows). A
//! caller holds a guard (`nas()`, `net()`, `disk()`) across the call; pipeline::timings
//! takes `totals()` as a phase opens and as it closes, and splits the phase's wall time by the
//! difference.
//!
//! How parallel waits count: by the job's wall clock, not summed over threads. At each instant the
//! process is in one state: waiting on the NAS when at least one of its threads is in a NAS call;
//! else on the network when at least one is in a request; else on the local disk when one is in a
//! disk call; else on other work when one waits for it; else computing. So the states' times add up to the wall time exactly, however many
//! threads wait at once (eight threads reading the NAS for the same second are one second of
//! waiting on the NAS), and a second in which one thread reads the NAS while others compute counts
//! as waiting on the NAS: the job could not have been quicker than its NAS waits allow there.
//!
//! What ran alongside (task #891): beside each kind's time, the seconds two things ran at once on
//! parallel threads, by `PAIRS`: two kinds of wait both in progress (exact: a guard of each held),
//! and a kind of wait with computing (while that kind held the instant, the process's CPU time
//! meanwhile, at most that time: a thread blocked in a call spends next to none, so it is the
//! other threads' computing). A pair's seconds lie inside the first kind's time (the instant was
//! given to it), except two waits' while a third, earlier kind held the instant. They are extra:
//! the kinds' times alone add up to the wall time.
//!
//! Cost: nothing until `enable` (a relaxed load per call); then a lock and a clock read as a guard
//! is taken and as it's dropped, and a read of the process's CPU time when the kind holding the
//! instant changes (hundreds of ns a guard: `waits_count_by_wall_clock_and_the_guards_cost_little`
//! measures it), against NAS and network calls of hundreds of µs and more.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// What a guard waits on, in the order a wall-clock instant is given to (the first active wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum On {
    Nas = 0,
    Net = 1,
    Disk = 2,
    /// Another worker, another thread's work, a lock.
    Other = 3,
}

/// The kinds of wait, by `On`'s order.
pub const KINDS: usize = 4;

/// Computing, as a pair's second member in `PAIRS` (no guard's kind).
pub const CPU: usize = KINDS;

/// The pairs whose time at once `Totals` counts after the kinds' own, by `On`'s order and `CPU`:
/// the waits two by two, then each wait with computing.
pub const PAIRS: [(usize, usize); 10] =
    [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3), (0, CPU), (1, CPU), (2, CPU), (3, CPU)];

/// The waits' pairs at the head of `PAIRS` (the rest: with computing).
const WAIT_PAIRS: usize = 6;

/// Wall time (ns) since `enable`: the process's waiting, by `On` (`[..KINDS]`), then each of
/// `PAIRS`' time at once (`[KINDS + p]`).
pub type Totals = [u64; KINDS + PAIRS.len()];

struct State {
    /// Guards held now, by kind.
    active: [u32; KINDS],
    /// When the state last changed.
    since: Instant,
    /// The kind holding the instant (`active`'s first held), since when, the process's CPU time then.
    top: Option<usize>,
    top_since: Instant,
    top_cpu: u64,
    acc: Totals,
}

impl State {
    /// The time since the last change given to the wait that held it (none: computing), and to the
    /// pairs of waits both in progress.
    fn settle(&mut self, now: Instant) {
        let d = now.saturating_duration_since(self.since).as_nanos() as u64;
        if let Some(k) = self.top {
            self.acc[k] += d;
            for (p, &(i, j)) in PAIRS[..WAIT_PAIRS].iter().enumerate() {
                if self.active[i] > 0 && self.active[j] > 0 {
                    self.acc[KINDS + p] += d;
                }
            }
        }
        self.since = now;
    }

    /// After `active` changed (settled at `now`): when another kind holds the instant, the last
    /// one's time with computing taken (`close`).
    fn retop(&mut self, now: Instant) {
        let top = self.active.iter().position(|&n| n > 0);
        if top != self.top {
            self.close(now);
            self.top = top;
        }
    }

    /// The kind holding the instant's time with computing since `top_since`: the process's CPU time
    /// meanwhile, at most that time. A new span from `now`.
    fn close(&mut self, now: Instant) {
        let cpu = cpu_ns();
        if let Some(k) = self.top {
            let d = now.saturating_duration_since(self.top_since).as_nanos() as u64;
            self.acc[KINDS + WAIT_PAIRS + k] += cpu.saturating_sub(self.top_cpu).min(d);
        }
        self.top_since = now;
        self.top_cpu = cpu;
    }
}

/// The process's CPU time (ns: user and system, all its threads); 0 in the wasm build (no
/// process clock there: no time with computing counted).
#[cfg(target_family = "wasm")]
fn cpu_ns() -> u64 {
    0
}

/// The process's CPU time (ns: user and system, all its threads).
#[cfg(not(target_family = "wasm"))]
fn cpu_ns() -> u64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a valid timespec to write to.
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut t) } != 0 {
        return 0;
    }
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

static ON: AtomicBool = AtomicBool::new(false);

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        let now = Instant::now();
        Mutex::new(State {
            active: [0; KINDS],
            since: now,
            top: None,
            top_since: now,
            top_cpu: 0,
            acc: [0; KINDS + PAIRS.len()],
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts measuring (pipeline::timings::start: a timed run). Before it every guard is a no-op.
pub fn enable() {
    let _ = state();
    ON.store(true, Ordering::Release);
}

/// Whether waits are measured.
pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

/// The wall time spent waiting so far, by kind, and the pairs' at once (ns; all 0 before `enable`).
pub fn totals() -> Totals {
    if !enabled() {
        return [0; KINDS + PAIRS.len()];
    }
    let mut s = lock();
    let now = Instant::now();
    s.settle(now);
    if s.top.is_some() {
        s.close(now);
    }
    s.acc
}

/// A wait in progress: it ends when dropped.
#[must_use]
pub struct Guard(Option<On>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(k) = self.0 {
            let mut s = lock();
            let now = Instant::now();
            s.settle(now);
            s.active[k as usize] -= 1;
            s.retop(now);
        }
    }
}

/// A wait on `on` from now until the guard is dropped.
pub fn wait(on: On) -> Guard {
    if !enabled() {
        return Guard(None);
    }
    let mut s = lock();
    let now = Instant::now();
    s.settle(now);
    s.active[on as usize] += 1;
    s.retop(now);
    Guard(Some(on))
}

/// A NAS call in progress.
pub fn nas() -> Guard {
    wait(On::Nas)
}

/// A network request in progress.
pub fn net() -> Guard {
    wait(On::Net)
}

/// A call on this Mac's own disk in progress.
pub fn disk() -> Guard {
    wait(On::Disk)
}

/// A wait on other work (another worker, a thread, a lock) in progress.
pub fn other() -> Guard {
    wait(On::Other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // (One test: the state is the process's.)
    #[test]
    fn waits_count_by_wall_clock_and_the_guards_cost_little() {
        // Run again in a process of its own, this test alone in it: in the runner's, the other
        // tests' NAS calls (their threads) would count as this one's waits meanwhile.
        const ALONE: &str = "SCENIC_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let out = crate::spawn::output(
                crate::spawn::command(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "waits::tests::waits_count_by_wall_clock_and_the_guards_cost_little",
                        "--nocapture",
                    ])
                    .env(ALONE, "1"),
            )
            .unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && said.contains(" 1 passed"),
                "{said}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        // Off: nothing counted.
        drop(nas());
        assert_eq!(totals(), [0; KINDS + PAIRS.len()]);
        enable();
        let t0 = totals();
        let span = Instant::now();
        // Four threads waiting on the NAS for the same ~50 ms: ~50 ms, not 200.
        let hs: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    let _g = nas();
                    std::thread::sleep(Duration::from_millis(50)); // (sleep ok: a NAS call's stand-in)
                })
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        let t1 = totals();
        let nas_ms = (t1[0] - t0[0]) / 1_000_000;
        // (At least the sleep, at most the span the four shared: not summed.)
        assert!(nas_ms >= 50 && nas_ms as u128 <= span.elapsed().as_millis(), "{nas_ms} ms");
        // The NAS before the network: a request beside a NAS call counts as the NAS's.
        let g = nas();
        let n = net();
        std::thread::sleep(Duration::from_millis(20)); // (sleep ok: a NAS call beside a request)
        drop(g);
        std::thread::sleep(Duration::from_millis(20)); // (sleep ok: the request alone)
        drop(n);
        let t2 = totals();
        let (nas2, net2) = ((t2[0] - t1[0]) / 1_000_000, (t2[1] - t1[1]) / 1_000_000);
        assert!(nas2 >= 20 && net2 >= 20, "{nas2} {net2}");
        // ...and the 20 ms they ran at once counted as the NAS's and the network's pair (extra).
        let both = (t2[KINDS] - t1[KINDS]) / 1_000_000;
        assert!((20..40).contains(&both), "{both} ms at once");
        // A thread computing beside a NAS call: the NAS's time with computing.
        let g = nas();
        let spin = Instant::now();
        let mut x = 0u64;
        while spin.elapsed() < Duration::from_millis(30) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        drop(g);
        let tc = totals();
        let with_cpu = (tc[KINDS + 6] - t2[KINDS + 6]) / 1_000_000;
        assert!((15..=40).contains(&with_cpu), "{with_cpu} ms of computing beside the NAS");
        // A NAS call alone, nothing computing: next to none.
        let g = nas();
        std::thread::sleep(Duration::from_millis(30)); // (sleep ok: a NAS call's stand-in)
        drop(g);
        let t2 = totals();
        assert!((t2[KINDS + 6] - tc[KINDS + 6]) / 1_000_000 < 10, "{:?}", t2);
        // Nested on one thread: counted once.
        {
            let _a = disk();
            let _b = disk();
            std::thread::sleep(Duration::from_millis(10)); // (sleep ok: a disk call's stand-in)
        }
        let t3 = totals();
        assert!((t3[2] - t2[2]) / 1_000_000 >= 10);
        // The cost: a guard taken and dropped.
        let n = 200_000;
        let t = Instant::now();
        for _ in 0..n {
            drop(disk());
        }
        let each = t.elapsed().as_nanos() as f64 / n as f64;
        eprintln!("a guard costs {each:.0} ns");
        assert!(each < 20_000.0, "{each} ns a guard");
    }
}
