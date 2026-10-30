//! The wall time this process spends blocked in I/O, measured where it blocks (task #870): every
//! NAS call (crate::nasfs's paths and files, crate::range's NAS reads, crate::iopool's operations,
//! crate::naming's and crate::blobs' copies), every network request (pipeline::fetch's `Net`, the
//! coordinator's client and the other HTTP callers) and the local disk calls the store makes
//! (crate::cachefile, crate::range's local files, the NAS's copies to this Mac's disk count as the
//! NAS's). A caller holds a guard (`nas()`, `net()`, `disk()`) across the call; pipeline::timings
//! takes `totals()` as a phase opens and as it closes, and splits the phase's wall time by the
//! difference.
//!
//! How parallel waits count: by the job's wall clock, not summed over threads. At each instant the
//! process is in one state: waiting on the NAS when at least one of its threads is in a NAS call;
//! else on the network when at least one is in a request; else on the local disk when one is in a
//! disk call; else computing. So the states' times add up to the wall time exactly, however many
//! threads wait at once (eight threads reading the NAS for the same second are one second of
//! waiting on the NAS), and a second in which one thread reads the NAS while others compute counts
//! as waiting on the NAS: the job could not have been quicker than its NAS waits allow there.
//!
//! Cost: nothing until `enable` (a relaxed load per call); then a lock and a clock read as a guard
//! is taken and as it's dropped (tens of ns: `the_guards_cost_little` measures it), against NAS and
//! network calls of hundreds of µs and more.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// What a guard waits on, in the order a wall-clock instant is given to (the first active wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum On {
    Nas = 0,
    Net = 1,
    Disk = 2,
}

/// The kinds of wait, by `On`'s order.
pub const KINDS: usize = 3;

/// Wall time (ns) the process spent waiting, by `On`, since `enable`.
pub type Totals = [u64; KINDS];

struct State {
    /// Guards held now, by kind.
    active: [u32; KINDS],
    /// When the state last changed.
    since: Instant,
    acc: Totals,
}

impl State {
    /// The time since the last change given to the wait that held it (none: computing).
    fn settle(&mut self, now: Instant) {
        if let Some(k) = self.active.iter().position(|&n| n > 0) {
            self.acc[k] += now.saturating_duration_since(self.since).as_nanos() as u64;
        }
        self.since = now;
    }
}

static ON: AtomicBool = AtomicBool::new(false);

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { active: [0; KINDS], since: Instant::now(), acc: [0; KINDS] }))
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

/// The wall time spent waiting so far, by kind (ns; all 0 before `enable`).
pub fn totals() -> Totals {
    if !enabled() {
        return [0; KINDS];
    }
    let mut s = lock();
    s.settle(Instant::now());
    s.acc
}

/// A wait in progress: it ends when dropped.
#[must_use]
pub struct Guard(Option<On>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(k) = self.0 {
            let mut s = lock();
            s.settle(Instant::now());
            s.active[k as usize] -= 1;
        }
    }
}

/// A wait on `on` from now until the guard is dropped.
pub fn wait(on: On) -> Guard {
    if !enabled() {
        return Guard(None);
    }
    let mut s = lock();
    s.settle(Instant::now());
    s.active[on as usize] += 1;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // (One test: the state is the process's.)
    #[test]
    fn waits_count_by_wall_clock_and_the_guards_cost_little() {
        // Off: nothing counted.
        drop(nas());
        assert_eq!(totals(), [0; KINDS]);
        enable();
        let t0 = totals();
        // Four threads waiting on the NAS for the same ~50 ms: ~50 ms, not 200.
        let hs: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    let _g = nas();
                    std::thread::sleep(Duration::from_millis(50));
                })
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        let t1 = totals();
        let nas_ms = (t1[0] - t0[0]) / 1_000_000;
        assert!((50..120).contains(&nas_ms), "{nas_ms} ms");
        // The NAS before the network: a request beside a NAS call counts as the NAS's.
        let g = nas();
        let n = net();
        std::thread::sleep(Duration::from_millis(20));
        drop(g);
        std::thread::sleep(Duration::from_millis(20));
        drop(n);
        let t2 = totals();
        let (nas2, net2) = ((t2[0] - t1[0]) / 1_000_000, (t2[1] - t1[1]) / 1_000_000);
        assert!((20..60).contains(&nas2) && (20..60).contains(&net2), "{nas2} {net2}");
        // Nested on one thread: counted once.
        {
            let _a = disk();
            let _b = disk();
            std::thread::sleep(Duration::from_millis(10));
        }
        let t3 = totals();
        assert!((10..40).contains(&((t3[2] - t2[2]) / 1_000_000)));
        // The cost: a guard taken and dropped.
        let n = 200_000;
        let t = Instant::now();
        for _ in 0..n {
            drop(disk());
        }
        let each = t.elapsed().as_nanos() as f64 / n as f64;
        eprintln!("a guard costs {each:.0} ns");
        assert!(each < 2_000.0, "{each} ns a guard");
    }
}
