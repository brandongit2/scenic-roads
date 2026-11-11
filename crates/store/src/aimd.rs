//! How many reads a process keeps in flight on one source, found as TCP finds its window (task
//! #1096, the owner's design of 10 Nov): additive increase, multiplicative decrease, each process
//! on its own. No allowance is handed out: every reader of the NAS (each job, on any Mac) and of a
//! host does this, so their shares settle between them, and a reader that goes quiet holds nothing.
//!
//! - **The limit** starts at `start` (2 on the NAS) and is never below 1 nor above the source's
//!   `ceiling`. A read waits (`Limiter::take`) while the reads in flight reach it.
//! - **Decided once a window**, as TCP decides once a round trip: a window is the reads begun at
//!   the limit it set, and it ends once as many of them as the limit (at least `WINDOW_LEAST`) have
//!   finished. Each read's latency over its size class's floor is its ratio; the window's median
//!   ratio decides (one slow answer of a long tail, or a few, is not the source loaded: the review
//!   of 11 Nov found the NAS's limit halved 290 times in the first 36 s of a terrain-lo run, held at
//!   1, when each read's own ratio decided):
//!   - within `NEAR`: up, doubling until the first halving (TCP's slow start), then one more
//!     (congestion avoidance);
//!   - past `SLOW`: halved;
//!   - between: held.
//! - **Halved at once** when a read fails as the source does when it's overloaded or stuck (a
//!   timeout, a NAS error of the share's; an HTTP 429 or 5xx: the caller says so,
//!   `Slot::congested`), once a window: a read begun before the last halving doesn't halve again.
//! - **The floor** of a size class (`class`: a 4 KB read and a 32 MB span aren't compared) is the
//!   least its latency smoothed (`SMOOTH`, about the last eight reads, kept through halvings) was
//!   lately (`FLOOR_WINDOW`: its minimum over this window and the one before, taken once the class
//!   has `WARM` reads; one lucky answer isn't a run of reads' best), never under the source's
//!   `least`, so it follows a source that has become slower for good instead of halving for ever
//!   against an old best. A read answered quicker than the source's `cached` (this Mac's own cache,
//!   in microseconds) is no sample: against it every real read would look slow (the review found
//!   64 KB reads of 20 µs as the floor of the NAS's real ones of 5–36 ms). A read of no bytes, or
//!   whose latency isn't its source's (a whole download's, a copy's), is none either
//!   (`Slot::unmeasured`).
//! - **No hang on a stuck source**: a read waits for its turn at most `TAKE_WAIT`, then goes
//!   anyway (a call stuck in the kernel holds its place until the kernel lets it go; the reads
//!   behind it aren't held longer than that).

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// A window whose median read is within this many times its class's floor counts as near the best.
pub const NEAR: f64 = 1.25;
/// A window whose median read is past this many times its class's floor halves the limit. (Not
/// two: on the M1's Wi-Fi the NAS's whole reads of a few KB to a few MB took one to four times
/// their floor at 8–16 in flight, the link full, not the NAS loaded; at two the limit was halved
/// every few seconds, down to 2–3, and `terrain-same 3/7/6` was 3 % slower than main, whose 16
/// readers had no limit; at three as quick: docs/phase-times.md §4f.)
pub const SLOW: f64 = 3.0;
/// The weight of a read's latency in its class's smoothed one (an exponential average: about the
/// last eight reads).
pub const SMOOTH: f64 = 0.125;
/// The reads a class has had before its smoothed latency is taken as a floor.
pub const WARM: u32 = 4;
/// The fewest reads that end a window (at a limit of 1 or 2, a window of one or two reads would
/// be one or two answers of a long tail).
pub const WINDOW_LEAST: usize = 4;
/// The floor's window (its minimum over this window and the one before).
pub const FLOOR_WINDOW: Duration = Duration::from_secs(30);
/// The longest a read waits for its turn before it goes anyway.
pub const TAKE_WAIT: Duration = Duration::from_secs(120);
/// Size classes: a power of two each (`class`), to 2^39 bytes.
const CLASSES: usize = 40;

/// A read of `bytes`' size class (its power of two), and its latency as a read of the class's
/// least size (latency × that over `bytes`: within a class a read twice as long would otherwise
/// look twice as slow, and a source reading whole files of every size never looked near its best,
/// task #1096's first measures).
fn class(bytes: u64) -> (usize, f64) {
    let k = (63 - bytes.max(1).leading_zeros() as usize).min(CLASSES - 1);
    (k, (1u64 << k) as f64 / bytes.max(1) as f64)
}

/// One source's limit: the reads in flight, the limit, the floors.
pub struct Limiter {
    st: Mutex<State>,
    cv: Condvar,
    ceiling: f64,
    /// The least a floor is taken to be (`Limiter::new`).
    least: Duration,
    /// Answers quicker than this are this Mac's cache's, not the source's (`Limiter::new`).
    cached: Duration,
}

struct State {
    held: usize,
    limit: f64,
    /// The window (`Limiter`'s doc): a read begun in an earlier one counts toward no decision.
    window: u64,
    /// Whether the limit has been halved yet (until then, the slow start's doubling).
    halved: bool,
    /// The ratios (latency over its class's floor) of the reads of this window finished so far.
    ratios: Vec<f64>,
    /// By size class (`class`): its reads' latency smoothed and its floor.
    classes: [Class; CLASSES],
}

/// A size class's latency (seconds), smoothed (`SMOOTH`), its reads so far, and its floor: the
/// least the smoothed latency was in this window and in the one before, and when this window
/// began. (The smoothed latency's floor, not a single read's: one lucky answer of a source with a
/// long tail isn't its best for a run of reads.)
#[derive(Clone, Copy)]
struct Class {
    smooth: Option<f64>,
    n: u32,
    min: Option<f64>,
    before: Option<f64>,
    since: Option<Instant>,
}

const NO_CLASS: Class = Class { smooth: None, n: 0, min: None, before: None, since: None };

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

impl State {
    /// Halved (never below one), and a new window.
    fn halve(&mut self) {
        self.limit = (self.limit / 2.0).max(1.0);
        self.halved = true;
        self.next_window();
    }

    fn next_window(&mut self) {
        self.window += 1;
        self.ratios.clear();
    }
}

impl Limiter {
    /// A limiter starting at `start` reads in flight, at most `ceiling`, its floors never under
    /// `least` (the source's quickest true answer: a LAN round trip, an internet one), an answer
    /// quicker than `cached` no sample (this Mac's cache's; zero for a source no cache of this
    /// Mac's answers).
    pub const fn new(start: usize, ceiling: usize, least: Duration, cached: Duration) -> Limiter {
        let st = State {
            held: 0,
            limit: start as f64,
            window: 0,
            halved: false,
            ratios: Vec::new(),
            classes: [NO_CLASS; CLASSES],
        };
        Limiter { st: Mutex::new(st), cv: Condvar::new(), ceiling: ceiling as f64, least, cached }
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
        Slot { of: self, window: g.window, t0: Instant::now(), bytes: 0, outcome: None }
    }

    /// A read's end: given back, and what it tells (begun in `window`) taken in at time `now`.
    pub fn record(&self, window: u64, outcome: Outcome, now: Instant) {
        let mut g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        g.held = g.held.saturating_sub(1);
        match outcome {
            Outcome::Unmeasured => {}
            Outcome::Congested => {
                if window == g.window {
                    g.halve();
                }
            }
            Outcome::Done { bytes, latency } if latency >= self.cached => {
                let (k, scale) = class(bytes);
                let c = &mut g.classes[k];
                let x = (latency.as_secs_f64() * scale).max(self.least.as_secs_f64());
                let sm = c.smooth.map_or(x, |m| m + SMOOTH * (x - m));
                c.smooth = Some(sm);
                c.n = c.n.saturating_add(1);
                // (A new window: this one's minimum becomes the one before; two or more windows
                // on, both are forgotten.)
                match c.since.map(|t| now.duration_since(t)) {
                    Some(age) if age >= FLOOR_WINDOW * 2 => (c.min, c.before, c.since) = (None, None, Some(now)),
                    Some(age) if age >= FLOOR_WINDOW => (c.min, c.before, c.since) = (None, c.min, Some(now)),
                    Some(_) => {}
                    None => c.since = Some(now),
                }
                if c.n >= WARM {
                    c.min = Some(c.min.map_or(sm, |m| m.min(sm)));
                }
                // (A class not yet warm is measured against its own smoothed latency: near.)
                let floor = [c.min, c.before].into_iter().flatten().fold(sm, f64::min);
                let ratio = x / floor.max(1e-9);
                if window == g.window {
                    g.ratios.push(ratio);
                    if g.ratios.len() >= (g.limit.floor() as usize).max(WINDOW_LEAST) {
                        let mut r = std::mem::take(&mut g.ratios);
                        r.sort_by(f64::total_cmp);
                        let median = r[r.len() / 2];
                        if median > SLOW {
                            g.halve();
                        } else {
                            if median <= NEAR {
                                let up = if g.halved { g.limit + 1.0 } else { g.limit * 2.0 };
                                g.limit = up.min(self.ceiling);
                            }
                            g.next_window();
                        }
                        r.clear();
                        g.ratios = r;
                    }
                }
            }
            // (An answer of this Mac's cache: not the source's.)
            Outcome::Done { .. } => {}
        }
        self.cv.notify_all();
    }

    /// The limit now (reads in flight it lets be), and the reads in flight.
    pub fn now(&self) -> (f64, usize) {
        let g = self.st.lock().unwrap_or_else(|e| e.into_inner());
        (g.limit, g.held)
    }

    /// The window now (tests: a read recorded as begun in it).
    #[cfg(test)]
    fn window(&self) -> u64 {
        self.st.lock().unwrap().window
    }
}

/// A read's turn (`Limiter::take`): given back when dropped, its latency taken from the take to
/// then, as a read of `bytes` (`Slot::read`), unless it says otherwise.
pub struct Slot<'a> {
    of: &'a Limiter,
    window: u64,
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
        self.of.record(self.window, outcome, now);
    }
}

/// The NAS's limiter, this process's: every read of a stored file, a pack, a mid, a whole copy
/// (store::range's NAS reads, crate::nasfs's reads and copies here) takes its turn from it.
pub static NAS: Limiter = Limiter::new(NAS_START, NAS_CEILING, NAS_LEAST, NAS_LEAST);
/// The NAS's start and ceiling (task #534 measured 8 streams at 57 MB/s against one at 25; past 16
/// SMB gains nothing and the share's other users lose).
pub const NAS_START: usize = 2;
pub const NAS_CEILING: usize = 16;
/// The least a NAS read's floor is taken to be, and the quickest answer taken as the NAS's: about
/// an SMB round trip on the LAN (a read this Mac's cache answers takes microseconds: no sample).
pub const NAS_LEAST: Duration = Duration::from_millis(2);
/// The least a host's request's floor is taken to be: about an internet round trip.
pub const NET_LEAST: Duration = Duration::from_millis(20);

/// `read` of `bytes` from the NAS under a turn of the process's NAS limit (`NAS`), for a reader
/// outside store::range and crate::nasfs (a base pack's ranges, codecs::basepart): its time a
/// sample when it succeeds, a failure of the share's (`nas_congested`) a halving.
pub fn nas_read<T>(bytes: u64, read: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut turn = NAS.take();
    let r = read();
    match &r {
        Ok(_) => turn.read(bytes),
        Err(e) if nas_congested(e) => turn.congested(),
        Err(_) => turn.unmeasured(),
    }
    r
}

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

    fn lim(start: usize, least: u64) -> Limiter {
        Limiter::new(start, 16, ms(least), ms(least))
    }

    /// `n` reads of `bytes` begun in the window now, each taking `latency`, recorded at `now`.
    fn reads(l: &Limiter, n: usize, bytes: u64, latency: Duration, now: Instant) {
        for _ in 0..n {
            l.record(l.window(), Outcome::Done { bytes, latency }, now);
        }
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
                let w = s.window;
                std::mem::forget(s);
                now += ms(1);
                l.record(w, Outcome::Done { bytes: 100_000, latency }, now);
            }
            seen.push(l.now().0);
        }
        seen
    }

    #[test]
    fn the_limit_climbs_to_the_sources_capacity_and_stays_near_it() {
        let l = lim(2, 1);
        let seen = settle(&l, 6, 20, 200);
        let last = &seen[100..];
        assert!(last.iter().all(|&v| (3.0..=8.0).contains(&v)), "around 6: {:?}", &seen[190..]);
        assert!(last.iter().any(|&v| v >= 6.0), "reaches the capacity");
        // (A source with room for more than the ceiling: the ceiling.)
        let l = lim(2, 1);
        let seen = settle(&l, 100, 20, 400);
        assert_eq!(*seen.last().unwrap(), 16.0);
    }

    #[test]
    fn a_window_decides_on_its_median_read_not_one_slow_answer() {
        let l = lim(4, 1);
        let t = Instant::now();
        // (Four alike: the class warm, near its best, doubled before any halving.)
        reads(&l, 4, 1000, ms(10), t);
        assert_eq!(l.now().0, 8.0, "slow start: doubled a window");
        // (A long tail: two of eight answers five times slower, the median near: up again.)
        reads(&l, 2, 1000, ms(50), t);
        reads(&l, 6, 1000, ms(10), t);
        assert_eq!(l.now().0, 16.0);
        // (A window mostly slow: halved once, however many slow reads of the window before it end
        // after.)
        let w = l.window();
        reads(&l, 16, 1000, ms(40), t);
        assert_eq!(l.now().0, 8.0);
        for _ in 0..8 {
            l.record(w, Outcome::Done { bytes: 1000, latency: ms(40) }, t);
        }
        assert_eq!(l.now().0, 8.0, "a read begun in an earlier window decides nothing");
        // (Past the first halving, one more a window.)
        reads(&l, 8, 1000, ms(10), t);
        assert_eq!(l.now().0, 9.0);
        // (Between near and slow: held.)
        reads(&l, 9, 1000, ms(16), t);
        assert_eq!(l.now().0, 9.0);
    }

    #[test]
    fn a_congested_read_halves_once_a_window_never_below_one_and_it_recovers() {
        let l = lim(8, 1);
        let t = Instant::now();
        let w = l.window();
        l.record(w, Outcome::Congested, t);
        assert_eq!(l.now().0, 4.0);
        for _ in 0..10 {
            l.record(w, Outcome::Congested, t);
        }
        assert_eq!(l.now().0, 4.0, "begun before the last halving: no more");
        for _ in 0..10 {
            l.record(l.window(), Outcome::Congested, t);
        }
        assert_eq!(l.now().0, 1.0, "never below one");
        // (Answers near the best again: back up, one a window of at least `WINDOW_LEAST`.)
        reads(&l, WINDOW_LEAST, 1000, ms(10), t);
        assert_eq!(l.now().0, 2.0);
        reads(&l, 400, 1000, ms(10), t);
        assert_eq!(l.now().0, 16.0);
    }

    #[test]
    fn reads_of_mixed_sizes_in_a_class_look_near_their_best() {
        assert_eq!(class(4096), (12, 1.0));
        assert_eq!(class(0).0, 0);
        assert!((class(6144).1 - 4096.0 / 6144.0).abs() < 1e-12);
        // (Whole files from 1 to 2 MB read at one rate: never slow, always near the best.)
        let l = lim(2, 1);
        let t = Instant::now();
        for i in 0..64u64 {
            let bytes = (1 << 20) + (i % 8) * (1 << 17);
            l.record(l.window(), Outcome::Done { bytes, latency: ms(bytes / 100_000) }, t);
        }
        assert_eq!(l.now().0, 16.0);
    }

    #[test]
    fn this_macs_cache_answers_are_no_samples() {
        // (64 KB reads this Mac's cache answers in microseconds among the NAS's real ones of
        // 5–36 ms, as the review's trace had them: the floor is the real ones', and the limit
        // climbs, where with the cache's answers as the floor it was halved again and again.)
        let l = lim(2, 2);
        let t = Instant::now();
        for i in 0..200u64 {
            let latency = if i % 2 == 0 { Duration::from_micros(20) } else { ms(5 + (i * 7) % 12) };
            l.record(l.window(), Outcome::Done { bytes: 65536, latency }, t);
        }
        assert_eq!(l.now().0, 16.0);
    }

    #[test]
    fn sizes_are_compared_within_their_class_and_the_floor_follows_a_slower_source() {
        let l = lim(4, 1);
        let t = Instant::now();
        reads(&l, 4, 4096, ms(5), t);
        let up = l.now().0;
        // (32 MB spans taking 400 ms aren't slow against 4 KB reads of 5 ms: their own class.)
        reads(&l, 8, 32 << 20, ms(400), t);
        assert!(l.now().0 >= up, "no halving across classes");
        // (The source six times slower for good: halved while the old best is in the floor's
        // window, then, the floor's windows on, 30 ms reads are the floor, near: up again.)
        let v = l.now().0;
        let later = t + FLOOR_WINDOW * 2 + ms(1);
        for k in 0..32 {
            l.record(l.window(), Outcome::Done { bytes: 4096, latency: ms(30) }, later + ms(k));
        }
        assert!(l.now().0 < v, "slower than the floor: halved");
        let v = l.now().0;
        let later = later + FLOOR_WINDOW * 2 + ms(1);
        for k in 0..32 {
            l.record(l.window(), Outcome::Done { bytes: 4096, latency: ms(30) }, later + ms(k));
        }
        assert!(l.now().0 > v, "near the new floor: up");
        // (Unmeasured: nothing changes.)
        let v = l.now().0;
        l.record(l.window(), Outcome::Unmeasured, later);
        assert_eq!(l.now().0, v);
    }

    #[test]
    fn readers_wait_for_their_turn_and_a_quiet_one_frees_it_at_once() {
        let l = lim(2, 1);
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
        let (a, b) = (lim(2, 1), lim(2, 1));
        let mut now = Instant::now();
        let (cap, base) = (8usize, 10u64);
        for _ in 0..400 {
            let total = a.now().0.floor() as usize + b.now().0.floor() as usize;
            let latency = ms(base * (1 + total.saturating_sub(cap) as u64));
            for l in [&a, &b] {
                for _ in 0..l.now().0.floor() as usize {
                    let s = l.take_within(ms(0));
                    let w = s.window;
                    std::mem::forget(s);
                    now += ms(1);
                    l.record(w, Outcome::Done { bytes: 100_000, latency }, now);
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
        let l = lim(2, 1);
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
