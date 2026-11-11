//! How many reads a process keeps in flight on one source, found as TCP finds its window (task
//! #1096, the owner's design of 10 Nov): additive increase, multiplicative decrease, each process
//! on its own. No allowance is handed out: every reader of the NAS (each job, on any Mac) and of a
//! host does this, so their shares settle between them, and a reader that goes quiet holds nothing.
//!
//! - **The limit** starts at `start` (2 on the NAS) and is never below 1 nor above the source's
//!   `ceiling`. A read waits (`Limiter::take`) while the reads in flight reach it.
//! - **Up by one a window** while reads finish near their best: a read whose latency is within
//!   `NEAR` of the floor adds 1 / limit (so a whole window of them adds one, as TCP's congestion
//!   avoidance does).
//! - **Halved** when a read takes past `SLOW` times the floor, or fails as the source does when
//!   it's overloaded or stuck (a timeout, a NAS error of the share's; an HTTP 429 or 5xx: the
//!   caller says so, `Slot::congested`). Once a window: a read begun before the last halving
//!   doesn't halve again (the burst that caused one is one signal).
//! - **The floor** is the least latency seen lately (`FLOOR_WINDOW`: the minimum over this window
//!   and the one before), kept by size class of the read (`class`: a 4 KB read and a 32 MB span
//!   aren't compared), so it follows a source that has become slower for good instead of halving
//!   for ever against an old best. A read of no bytes (nothing was read: a cache answered) or whose
//!   latency isn't its source's (a whole download's) is no sample (`Slot::unmeasured`).
//! - **No hang on a stuck source**: a read waits for its turn at most `TAKE_WAIT`, then goes
//!   anyway (a call stuck in the kernel holds its place until the kernel lets it go; the reads
//!   behind it aren't held longer than that).

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// A read within this many times its class's floor counts as near the best.
pub const NEAR: f64 = 1.25;
/// A read past this many times its class's floor halves the limit.
pub const SLOW: f64 = 2.0;
/// The floor's window (its minimum over this window and the one before).
pub const FLOOR_WINDOW: Duration = Duration::from_secs(30);
/// The longest a read waits for its turn before it goes anyway.
pub const TAKE_WAIT: Duration = Duration::from_secs(120);
/// Size classes: below 64 KB, 1 MB, 8 MB, and the rest.
const CLASSES: usize = 4;

fn class(bytes: u64) -> usize {
    match bytes {
        0..65_536 => 0,
        65_536..1_048_576 => 1,
        1_048_576..8_388_608 => 2,
        _ => 3,
    }
}

/// One source's limit: the reads in flight, the limit, the floors.
pub struct Limiter {
    st: Mutex<State>,
    cv: Condvar,
    ceiling: f64,
}

struct State {
    held: usize,
    limit: f64,
    /// Halvings so far: a read begun before the last one doesn't halve again.
    epoch: u64,
    /// By class: the least latency in this window and in the one before, and when this began.
    floors: [(Option<Duration>, Option<Duration>, Option<Instant>); CLASSES],
}

/// What a read's end tells its limiter (`Limiter::record`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// It finished, reading `bytes`, in `latency`.
    Done { bytes: u64, latency: Duration },
    /// It failed as the source does when overloaded or stuck.
    Congested,
    /// Nothing to learn from it (no bytes read, a whole download, an error of its own).
    Unmeasured,
}

impl Limiter {
    /// A limiter starting at `start` reads in flight, at most `ceiling`.
    pub const fn new(start: usize, ceiling: usize) -> Limiter {
        Limiter {
            st: Mutex::new(State { held: 0, limit: start as f64, epoch: 0, floors: [(None, None, None); CLASSES] }),
            cv: Condvar::new(),
            ceiling: ceiling as f64,
        }
    }

    /// A turn to read, waited for while the reads in flight reach the limit (at most `TAKE_WAIT`).
    pub fn take(&self) -> Slot<'_> {
        self.take_within(TAKE_WAIT)
    }

    fn take_within(&self, most: Duration) -> Slot<'_> {
        let t0 = Instant::now();
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        while g.held as f64 >= g.limit.floor().max(1.0) {
            let left = most.saturating_sub(t0.elapsed());
            if left.is_zero() {
                break;
            }
            g = self.cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        g.held += 1;
        Slot { of: self, epoch: g.epoch, t0: Instant::now(), bytes: 0, outcome: None }
    }

    /// A read's end: given back, and what it tells (begun at `epoch`) taken in at time `now`.
    pub fn record(&self, epoch: u64, outcome: Outcome, now: Instant) {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        g.held = g.held.saturating_sub(1);
        let halve = |g: &mut State| {
            if epoch == g.epoch {
                g.limit = (g.limit / 2.0).max(1.0);
                g.epoch += 1;
            }
        };
        match outcome {
            Outcome::Unmeasured => {}
            Outcome::Congested => halve(&mut g),
            Outcome::Done { bytes, latency } => {
                let f = &mut g.floors[class(bytes)];
                // (A new window: this one's minimum becomes the one before; two or more windows
                // on, both are forgotten.)
                if let Some(age) = f.2.map(|t| now.duration_since(t)) {
                    if age >= FLOOR_WINDOW * 2 {
                        *f = (None, None, Some(now));
                    } else if age >= FLOOR_WINDOW {
                        *f = (None, f.0, Some(now));
                    }
                } else {
                    f.2 = Some(now);
                }
                f.0 = Some(f.0.map_or(latency, |m| m.min(latency)));
                let floor = [f.0, f.1].into_iter().flatten().min().unwrap_or(latency);
                let ratio = latency.as_secs_f64() / floor.as_secs_f64().max(1e-6);
                if ratio > SLOW {
                    halve(&mut g);
                } else if ratio <= NEAR {
                    g.limit = (g.limit + 1.0 / g.limit).min(self.ceiling);
                }
            }
        }
        self.cv.notify_all();
    }

    /// The limit now (reads in flight it lets be), and the reads in flight.
    pub fn now(&self) -> (f64, usize) {
        let g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        (g.limit, g.held)
    }
}

/// A read's turn (`Limiter::take`): given back when dropped, its latency taken from the take to
/// then, as a read of `bytes` (`Slot::read`), unless it says otherwise.
pub struct Slot<'a> {
    of: &'a Limiter,
    epoch: u64,
    t0: Instant,
    bytes: u64,
    outcome: Option<Outcome>,
}

impl Slot<'_> {
    /// The read got `n` bytes (its latency is a sample of its size's class).
    pub fn read(&mut self, n: u64) {
        self.bytes += n;
    }

    /// The read failed as an overloaded or stuck source does: the limit halves.
    pub fn congested(&mut self) {
        self.outcome = Some(Outcome::Congested);
    }

    /// Nothing to learn from this read's time (a whole download: its length isn't the source's).
    pub fn unmeasured(&mut self) {
        if self.outcome.is_none() {
            self.outcome = Some(Outcome::Unmeasured);
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let now = Instant::now();
        let outcome = match self.outcome {
            Some(o) => o,
            None if self.bytes == 0 => Outcome::Unmeasured,
            None => Outcome::Done { bytes: self.bytes, latency: now.duration_since(self.t0) },
        };
        self.of.record(self.epoch, outcome, now);
    }
}

/// The NAS's limiter, this process's: every read of a stored file, a pack, a mid, a whole copy
/// (store::range's NAS reads, crate::nasfs's reads and copies here) takes its turn from it.
pub static NAS: Limiter = Limiter::new(NAS_START, NAS_CEILING);
/// The NAS's start and ceiling (task #534 measured 8 streams at 57 MB/s against one at 25; past 16
/// SMB gains nothing and the share's other users lose).
pub const NAS_START: usize = 2;
pub const NAS_CEILING: usize = 16;

/// Whether an I/O error is the share's being overloaded or stuck (a timeout, a network failure of
/// the mount), not the read's own (a file not there, a permission).
pub fn nas_congested(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(e.kind(), TimedOut | WouldBlock | ConnectionReset | ConnectionAborted | NotConnected | BrokenPipe)
        || matches!(
            e.raw_os_error(),
            Some(libc::ETIMEDOUT) | Some(libc::EIO) | Some(libc::ENETDOWN) | Some(libc::EHOSTDOWN)
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A source answering in `base` ms while `cap` reads or fewer are in flight, each one more
    /// adding `base` (a queue past its capacity): the limit an AIMD reader settles on.
    fn settle(l: &Limiter, cap: usize, base: u64, rounds: usize) -> Vec<f64> {
        let mut seen = Vec::new();
        let mut now = Instant::now();
        for _ in 0..rounds {
            let n = l.now().0.floor() as usize;
            let latency = ms(base * (1 + n.saturating_sub(cap) as u64));
            for _ in 0..n {
                let s = l.take_within(ms(0));
                let epoch = s.epoch;
                std::mem::forget(s);
                now += ms(1);
                l.record(epoch, Outcome::Done { bytes: 100_000, latency }, now);
            }
            seen.push(l.now().0);
        }
        seen
    }

    #[test]
    fn the_limit_climbs_to_the_sources_capacity_and_stays_near_it() {
        let l = Limiter::new(2, 16);
        let seen = settle(&l, 6, 20, 200);
        let last = &seen[100..];
        assert!(last.iter().all(|&v| (3.0..=8.0).contains(&v)), "around 6: {:?}", &seen[190..]);
        assert!(last.iter().any(|&v| v >= 6.0), "reaches the capacity");
        // (A source with room for more than the ceiling: the ceiling.)
        let l = Limiter::new(2, 16);
        let seen = settle(&l, 100, 20, 400);
        assert_eq!(*seen.last().unwrap(), 16.0);
    }

    #[test]
    fn a_slow_read_or_a_congested_one_halves_once_a_window_and_it_recovers() {
        let l = Limiter::new(8, 16);
        let now = Instant::now();
        let fast = Outcome::Done { bytes: 1000, latency: ms(10) };
        l.record(0, fast, now);
        assert!(l.now().0 > 8.0, "near the floor: up");
        let before = l.now().0;
        // (Three slow reads begun in the same window: one halving.)
        for _ in 0..3 {
            l.record(0, Outcome::Done { bytes: 1000, latency: ms(50) }, now);
        }
        assert_eq!(l.now().0, before / 2.0);
        l.record(1, Outcome::Congested, now);
        assert_eq!(l.now().0, before / 4.0);
        for _ in 0..10 {
            l.record(0, Outcome::Congested, now);
        }
        assert_eq!(l.now().0, before / 4.0, "begun before the last halving: no more");
        for e in 2..20 {
            l.record(e, Outcome::Congested, now);
        }
        assert_eq!(l.now().0, 1.0, "never below one");
        // (Fast again: back up, a window at a time.)
        for _ in 0..200 {
            l.record(20, fast, now);
        }
        assert_eq!(l.now().0, 16.0);
    }

    #[test]
    fn sizes_are_compared_within_their_class_and_the_floor_follows_a_slower_source() {
        let l = Limiter::new(4, 16);
        let t = Instant::now();
        l.record(0, Outcome::Done { bytes: 4096, latency: ms(5) }, t);
        let up = l.now().0;
        // (A 32 MB span taking 400 ms isn't slow against 4 KB reads of 5 ms: its own class.)
        l.record(0, Outcome::Done { bytes: 32 << 20, latency: ms(400) }, t);
        assert!(l.now().0 >= up, "no halving across classes");
        // (Two windows on, the old best is forgotten: 20 ms reads are the floor now, not slow.)
        let later = t + FLOOR_WINDOW * 2 + ms(1);
        l.record(0, Outcome::Done { bytes: 4096, latency: ms(20) }, later);
        let v = l.now().0;
        l.record(0, Outcome::Done { bytes: 4096, latency: ms(20) }, later + ms(1));
        assert!(l.now().0 > v, "near the new floor: up");
        // (Unmeasured: nothing changes.)
        let v = l.now().0;
        l.record(0, Outcome::Unmeasured, later);
        assert_eq!(l.now().0, v);
    }

    #[test]
    fn readers_wait_for_their_turn_and_a_quiet_one_frees_it_at_once() {
        let l = Limiter::new(2, 16);
        let a = l.take();
        let b = l.take();
        assert_eq!(l.now().1, 2);
        // (A third waits; it goes once one is given back, a read with nothing to learn from.)
        let went = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                let _c = l.take();
                went.store(true, Ordering::SeqCst);
            });
            std::thread::sleep(ms(50)); // (absence)
            assert!(!went.load(Ordering::SeqCst), "a read past the limit waits");
            drop(a);
        });
        assert!(went.load(Ordering::SeqCst));
        drop(b);
        assert_eq!(l.now().1, 0, "nothing held once the readers are done");
        // (A stuck read doesn't hold the rest past TAKE_WAIT: here 30 ms, then it goes anyway.)
        let _x = l.take();
        let _y = l.take();
        let t0 = Instant::now();
        let _z = l.take_within(ms(30));
        assert!(t0.elapsed() >= ms(30));
        assert_eq!(l.now().1, 3);
    }

    #[test]
    fn two_processes_reading_one_source_share_it() {
        // (Each its own limiter, the source's latency set by their reads together: their limits
        // settle near half its capacity each.)
        let (a, b) = (Limiter::new(2, 16), Limiter::new(2, 16));
        let mut now = Instant::now();
        let (cap, base) = (8usize, 10u64);
        for _ in 0..400 {
            let total = a.now().0.floor() as usize + b.now().0.floor() as usize;
            let latency = ms(base * (1 + total.saturating_sub(cap) as u64));
            for l in [&a, &b] {
                for _ in 0..l.now().0.floor() as usize {
                    let s = l.take_within(ms(0));
                    let e = s.epoch;
                    std::mem::forget(s);
                    now += ms(1);
                    l.record(e, Outcome::Done { bytes: 100_000, latency }, now);
                }
            }
        }
        let (x, y) = (a.now().0, b.now().0);
        assert!(x + y <= 2.0 * cap as f64 && x >= 1.0 && y >= 1.0, "{x} {y}");
        assert!((x - y).abs() <= 4.0, "a fair share each: {x} {y}");
    }

    /// A fake store of 4 reads' capacity (each past it adds a read's time to every read in flight),
    /// read by 16 threads at once through one limiter: the reads in flight never pass the limit,
    /// and it settles near the store's capacity, well under the threads.
    #[test]
    fn readers_of_a_fake_slow_store_settle_near_its_capacity() {
        let l = Limiter::new(2, 16);
        let inflight = std::sync::atomic::AtomicUsize::new(0);
        let most = std::sync::atomic::AtomicUsize::new(0);
        let limits = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..16 {
                s.spawn(|| {
                    for _ in 0..40 {
                        let mut turn = l.take();
                        let n = inflight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        most.fetch_max(n, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(ms(2 * (1 + n.saturating_sub(4) as u64)));
                        inflight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        turn.read(100_000);
                        drop(turn);
                        limits.lock().unwrap().push(l.now().0);
                    }
                });
            }
        });
        let limits = limits.into_inner().unwrap();
        let tail = &limits[limits.len() / 2..];
        let mean = tail.iter().sum::<f64>() / tail.len() as f64;
        assert!(mean < 10.0, "settled near the capacity (4), not the threads (16): {mean}");
        assert!(most.into_inner() <= 16);
        assert_eq!(l.now().1, 0);
    }

    #[test]
    fn the_nas_congests_on_timeouts_not_on_a_missing_file() {
        assert!(nas_congested(&std::io::Error::from(std::io::ErrorKind::TimedOut)));
        assert!(nas_congested(&std::io::Error::from_raw_os_error(libc::ETIMEDOUT)));
        assert!(!nas_congested(&std::io::Error::from(std::io::ErrorKind::NotFound)));
        assert!(!nas_congested(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
    }
}
