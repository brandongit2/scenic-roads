//! The wall time this process spends blocked in I/O, measured where it blocks (task #870): every
//! NAS call (crate::nasfs's paths and files, crate::range's NAS reads, crate::iopool's operations,
//! crate::naming's and crate::blobs' copies), every network request (buildstore::fetch's `Net`, the
//! coordinator's client and the other HTTP callers) and the local disk calls the store makes
//! (crate::cachefile, crate::range's local files, the NAS's copies to this Mac's disk count as the
//! NAS's), and the waits on other work a job's code says it blocks in (`other()`: another worker's
//! subtrees, a thread copying ahead: a wait on a condition or a join, which no I/O call shows). A
//! caller holds a guard (`nas()`, `net()`, `disk()`) across the call; jobrt::timings
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
//! meanwhile, at most that time: an estimate, which also counts the waiting threads' own CPU in
//! their calls, and other waits' threads'; not counted for the local disk, whose reads from memory
//! are nearly all their own CPU: its pair with computing stays 0). A pair's seconds lie inside the first kind's time (the instant was
//! given to it), except two waits' while a third, earlier kind held the instant. They are extra:
//! the kinds' times alone add up to the wall time.
//!
//! A guard also lends its thread's turn of a job's computing (`Turns`, task #1097), measured or not:
//! a stage that reads as it computes gives the cores its waits leave idle to a stage that only
//! computes.
//!
//! Cost: nothing until `enable` (a relaxed load and a thread-local read per call); then a lock and a clock read as a guard
//! is taken and as it's dropped, and a read of the process's CPU time, outside the lock, when the
//! kind holding the instant changes (hundreds of ns a guard: `waits_count_by_wall_clock_and_the_guards_cost_little`
//! measures it), against NAS and network calls of hundreds of µs and more.
//!
//! As intervals too (task #1104, a job's spans: jobrt::spans), once `intervals_from` is called:
//! each kind's waits as the times some thread was in one (`Interval`), by the wall clock, kinds
//! apart (a NAS call beside a request is in both kinds' intervals). Those of a kind less than the
//! merge gap apart (`set_gap`) are one interval, its `n` waits and their `busy` time: a loop of
//! small reads is one bar, not thousands. The intervals ended, and the notes the store makes of
//! its retries (`note`, crate::retry::log), wait for jobrt::spans to take them (`take`). Recorded
//! only where a kind starts or stops being waited on, under the lock a guard takes anyway.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
    /// The kind holding the instant (`active`'s first held).
    top: Option<usize>,
    /// Changes of `top` so far: a CPU time read after one is taken only while no later one came.
    changes: u64,
    /// The span whose CPU time is counted next: the kind that held it (none: computing), since
    /// when, and the process's CPU time then.
    span: Option<usize>,
    span_since: Instant,
    span_cpu: u64,
    acc: Totals,
    /// The intervals, once asked for (`intervals_from`).
    iv: Option<Box<Intervals>>,
}

/// A kind's waits from `t0` to `t1` (ns since `intervals_from`'s base): `n` of them, waited on
/// `busy` ns of it (all of it when `n` is 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub on: On,
    pub t0: u64,
    pub t1: u64,
    pub busy: u64,
    pub n: u32,
}

/// The intervals being recorded (`State::iv`).
struct Intervals {
    base: Instant,
    /// Two of a kind this close (ns) are merged.
    gap: u64,
    /// Since when each kind has been waited on, while it is (ns since `base`).
    since: [Option<u64>; KINDS],
    /// Each kind's last interval, while the next may still merge into it.
    open: [Option<Interval>; KINDS],
    /// Those ended, for `take`.
    done: Vec<Interval>,
    /// The notes made (`note`): when (ns since `base`), what, the line.
    notes: Vec<(u64, &'static str, String)>,
}

/// The most notes kept between two `take`s (a retry storm's: the rest counted in the last).
const NOTES_MAX: usize = 200;

impl Intervals {
    fn at(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.base).as_nanos() as u64
    }

    /// Kind `k`'s waits ended at `t1` (began at `since[k]`): into its open interval when within the
    /// gap of it, else the open one done and this one open.
    fn ended(&mut self, k: usize, t1: u64) {
        let Some(t0) = self.since[k].take() else { return };
        let on = [On::Nas, On::Net, On::Disk, On::Other][k];
        match &mut self.open[k] {
            Some(o) if t0 <= o.t1 + self.gap => {
                o.t1 = t1;
                o.busy += t1 - t0;
                o.n += 1;
            }
            o => {
                if let Some(d) = o.replace(Interval { on, t0, t1, busy: t1 - t0, n: 1 }) {
                    self.done.push(d);
                }
            }
        }
    }
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

    /// After `active` changed: whether another kind holds the instant now, and if so the change's
    /// number, for `close` once the CPU time is read (outside the lock: the process clock takes
    /// hundreds of ns, more with more threads, and every other guard would wait on it).
    fn retop(&mut self) -> Option<u64> {
        let top = self.active.iter().position(|&n| n > 0);
        (top != self.top).then(|| {
            self.top = top;
            self.changes += 1;
            self.changes
        })
    }

    /// The open span's time with computing, to change `change` at `now`, `cpu` the process's CPU
    /// time read just after it: that CPU time less the span's start's, at most the span's wall
    /// time, to the kind that held it; a new span from there, of the kind holding the instant now.
    /// A reading overtaken by a later change's is dropped: that one closes the span whole, as the
    /// kind it opened with (off by the changes' microsecond or so apart).
    fn close(&mut self, change: u64, now: Instant, cpu: u64) {
        if change != self.changes {
            return;
        }
        // (Not the disk's: a local read served from memory is its own thread's CPU, which would
        // show as computing alongside even on one thread.)
        if let Some(k) = self.span.filter(|&k| k != On::Disk as usize) {
            let d = now.saturating_duration_since(self.span_since).as_nanos() as u64;
            self.acc[KINDS + WAIT_PAIRS + k] += cpu.saturating_sub(self.span_cpu).min(d);
        }
        self.span = self.top;
        self.span_since = now;
        self.span_cpu = cpu;
    }
}

/// After a guard changed `active`, the lock released: when the kind holding the instant changed
/// with it, the CPU time read and the span closed; nothing else (most guards at once).
fn reread(change: Option<(u64, Instant)>) {
    if let Some((c, now)) = change {
        let cpu = cpu_ns();
        lock().close(c, now, cpu);
    }
}

/// The process's CPU time (ns: user and system, all its threads).
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
            changes: 0,
            span: None,
            span_since: now,
            span_cpu: 0,
            acc: [0; KINDS + PAIRS.len()],
            iv: None,
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts measuring (jobrt::timings::start: a timed run). Before it every guard is a no-op.
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
    let (now, change) = {
        let mut s = lock();
        let now = Instant::now();
        s.settle(now);
        (now, s.changes)
    };
    // (The open span closed here too, so that a phase's end has its time with computing so far.)
    let cpu = cpu_ns();
    let mut s = lock();
    s.close(change, now, cpu);
    s.acc
}

/// A wait in progress: it ends when dropped. On a thread computing on a job's turns (`Turns`), the
/// outermost of its waits but the local disk's lends its turn while it lasts.
#[must_use]
pub struct Guard {
    on: Option<On>,
    /// The turns it lent its thread's turn to (None: it lent none: a nested wait, the disk's, or a
    /// thread holding no turn).
    lent: Option<Arc<TurnsState>>,
    /// Whether it counts among its thread's open waits (`HELD`'s depth).
    nested: bool,
    /// (Its thread's: the turn it lent is its thread's.)
    _thread: std::marker::PhantomData<*const ()>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.nested {
            HELD.with(|h| {
                if let Some((_, depth)) = h.borrow_mut().as_mut() {
                    *depth -= 1;
                }
            });
        }
        if let Some(t) = self.lent.take() {
            // (Taken back without waiting: a wait never blocks on a turn, so no turn holder waits
            // on a thread that waits for one.)
            t.change(1);
        }
        if let Some(k) = self.on {
            let change = {
                let mut s = lock();
                let now = Instant::now();
                s.settle(now);
                s.active[k as usize] -= 1;
                if s.active[k as usize] == 0 {
                    if let Some(iv) = s.iv.as_mut() {
                        let t = iv.at(now);
                        iv.ended(k as usize, t);
                    }
                }
                s.retop().map(|c| (c, now))
            };
            reread(change);
        }
    }
}

/// A wait on `on` from now until the guard is dropped (`nas`, `net`, `disk`, `other`).
fn wait(on: On) -> Guard {
    let (lent, nested) = lend(on);
    let mut g = Guard { on: None, lent, nested, _thread: std::marker::PhantomData };
    if !enabled() {
        return g;
    }
    let change = {
        let mut s = lock();
        let now = Instant::now();
        s.settle(now);
        s.active[on as usize] += 1;
        if s.active[on as usize] == 1 {
            if let Some(iv) = s.iv.as_mut() {
                iv.since[on as usize] = Some(iv.at(now));
            }
        }
        s.retop().map(|c| (c, now))
    };
    reread(change);
    g.on = Some(on);
    g
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

/// Records the waits as intervals from now (`Interval`, times since `base`), two of a kind `gap_ns`
/// apart or less merged, and the store's notes (`note`); measuring on (`enable`). Again: the base
/// kept, the gap set.
pub fn intervals_from(base: Instant, gap_ns: u64) {
    enable();
    let mut s = lock();
    let now = Instant::now();
    match s.iv.as_mut() {
        Some(iv) => iv.gap = gap_ns,
        None => {
            let mut iv = Intervals {
                base,
                gap: gap_ns,
                since: [None; KINDS],
                open: [None; KINDS],
                done: Vec::new(),
                notes: Vec::new(),
            };
            // (A kind waited on now: from now.)
            for k in 0..KINDS {
                if s.active[k] > 0 {
                    iv.since[k] = Some(iv.at(now));
                }
            }
            s.iv = Some(Box::new(iv));
        }
    }
}

/// The merge gap from now (ns): wider as a job's records grow (jobrt::spans thins them).
pub fn set_gap(gap_ns: u64) {
    if let Some(iv) = lock().iv.as_mut() {
        iv.gap = gap_ns;
    }
}

/// A note of `what` (a retry's line, crate::retry::log) at this moment, for the job's spans; nothing
/// while no intervals are recorded.
pub fn note(what: &'static str, line: String) {
    if !enabled() {
        return;
    }
    let mut s = lock();
    let now = Instant::now();
    if let Some(iv) = s.iv.as_mut() {
        let t = iv.at(now);
        if iv.notes.len() < NOTES_MAX {
            iv.notes.push((t, what, line));
        }
    }
}

/// The intervals ended (those that can't merge any more: their kind's next would come past the
/// gap from now; every one, a kind waited on now to now, when `all`, as a job ends), and the notes
/// made, since the last `take`.
pub fn take(all: bool) -> (Vec<Interval>, Vec<(u64, &'static str, String)>) {
    let mut s = lock();
    let now = Instant::now();
    let active = s.active;
    let Some(iv) = s.iv.as_mut() else { return Default::default() };
    let t = iv.at(now);
    for k in 0..KINDS {
        if all && active[k] > 0 {
            iv.ended(k, t);
            iv.since[k] = Some(t);
        }
        if iv.open[k].is_some_and(|o| all || (o.t1 + iv.gap < t && iv.since[k].is_none())) {
            let o = iv.open[k].take().unwrap();
            iv.done.push(o);
        }
    }
    iv.done.sort_by_key(|i| i.t0);
    (std::mem::take(&mut iv.done), std::mem::take(&mut iv.notes))
}

/// A job's computing held to so many threads at once, its waits lending their turns (task #1097):
/// the threads of a stage that reads as it computes (the terrain's sampling) each hold a turn
/// while they run (`computing`) and lend it while they wait on the NAS, the network or other work
/// (a guard of `nas`, `net` or `other`; the local disk's, mostly reads from memory, lend none);
/// a stage that only computes (the terrain's tiles made from the samples) takes a turn for each
/// piece of its work (`take`), waiting while every turn is held. So the second computes on the
/// cores the first's waits leave idle, and the two together on no more threads than there are
/// turns (a job's grant: docs/pool.md §7.1, Threads granted), but for a thread back from a wait
/// while its turn was taken, until that piece of work ends: a turn is taken back without waiting,
/// so nothing holding a turn ever waits on a thread waiting for one.
#[derive(Clone)]
pub struct Turns(Arc<TurnsState>);

/// How many turns there are and how many are held (`Turns`).
pub struct TurnsState {
    n: usize,
    held: Mutex<isize>,
    cv: std::sync::Condvar,
}

impl TurnsState {
    /// The turns held changed by `d` (a turn given back wakes a taker).
    fn change(&self, d: isize) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) += d;
        if d < 0 {
            self.cv.notify_all();
        }
    }
}

thread_local! {
    /// The turns this thread holds one of (`Turns::computing`, `Turns::take`) and how many of its
    /// waits are open (the outermost lends the turn).
    static HELD: std::cell::RefCell<Option<(Arc<TurnsState>, u32)>> = const { std::cell::RefCell::new(None) };
}

/// A wait's turn lent (`Guard`): the turns, where this thread holds one and this is its outermost
/// wait but the disk's; and whether it counts among this thread's open waits.
fn lend(on: On) -> (Option<Arc<TurnsState>>, bool) {
    if on == On::Disk {
        return (None, false);
    }
    HELD.with(|h| match h.borrow_mut().as_mut() {
        None => (None, false),
        Some((t, depth)) => {
            *depth += 1;
            if *depth > 1 {
                return (None, true);
            }
            t.change(-1);
            (Some(t.clone()), true)
        }
    })
}

/// A turn held by this thread (`Turns::computing`, `Turns::take`), given back as it's dropped.
#[must_use]
pub struct Turn {
    turns: Option<Arc<TurnsState>>,
    _thread: std::marker::PhantomData<*const ()>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Some(t) = self.turns.take() {
            HELD.with(|h| *h.borrow_mut() = None);
            t.change(-1);
        }
    }
}

impl Turns {
    /// `n` turns (at least one).
    pub fn new(n: usize) -> Turns {
        Turns(Arc::new(TurnsState { n: n.max(1), held: Mutex::new(0), cv: Default::default() }))
    }

    /// How many turns are held now (those lent to waits not counted).
    pub fn held(&self) -> isize {
        *self.0.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `f` run holding a turn, taken without waiting (a stage reading as it computes: its threads
    /// are the turns'), lent while it waits (`Guard`). On a thread holding one already, `f` alone.
    pub fn computing<R>(&self, f: impl FnOnce() -> R) -> R {
        let _t = self.hold(false);
        f()
    }

    /// A turn, waited for while every one is held (a stage that only computes); given back as it's
    /// dropped. On a thread holding one already, nothing more.
    pub fn take(&self) -> Turn {
        self.hold(true)
    }

    fn hold(&self, wait: bool) -> Turn {
        if HELD.with(|h| h.borrow().is_some()) {
            return Turn { turns: None, _thread: std::marker::PhantomData };
        }
        {
            let mut g = self.0.held.lock().unwrap_or_else(|e| e.into_inner());
            while wait && *g >= self.0.n as isize {
                g = self.0.cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
            *g += 1;
        }
        HELD.with(|h| *h.borrow_mut() = Some((self.0.clone(), 0)));
        Turn { turns: Some(self.0.clone()), _thread: std::marker::PhantomData }
    }
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
        // ...and the 20 ms they ran at once counted as the NAS's and the network's pair (extra,
        // within the NAS's time).
        let both = (t2[KINDS] - t1[KINDS]) / 1_000_000;
        assert!(both >= 20 && both <= nas2, "{both} ms at once, {nas2} on the NAS");
        // A thread computing beside a NAS call: the NAS's time with computing.
        let g = nas();
        let c0 = cpu_ns();
        let mut x = 0u64;
        let busy = 30_000_000; // (bound ok: 30 ms of CPU, however loaded the Mac)
        while cpu_ns() - c0 < busy {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        drop(g);
        let tc = totals();
        let with_cpu = (tc[KINDS + 6] - t2[KINDS + 6]) / 1_000_000;
        let nas3 = (tc[0] - t2[0]) / 1_000_000;
        assert!(with_cpu >= 25 && with_cpu <= nas3, "{with_cpu} ms of computing beside {nas3} ms on the NAS");
        // A NAS call alone, nothing computing: next to none.
        let g = nas();
        std::thread::sleep(Duration::from_millis(30)); // (sleep ok: a NAS call's stand-in)
        drop(g);
        let t2 = totals();
        assert!((t2[KINDS + 6] - tc[KINDS + 6]) / 1_000_000 < 10, "{:?}", t2);
        // Computing under a local disk call: not counted (a read from memory is its own CPU).
        let g = disk();
        let c0 = cpu_ns();
        while cpu_ns() - c0 < 10_000_000 {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        drop(g);
        assert_eq!(totals()[KINDS + 8], t2[KINDS + 8]);
        // Nested on one thread: counted once.
        {
            let _a = disk();
            let _b = disk();
            std::thread::sleep(Duration::from_millis(10)); // (sleep ok: a disk call's stand-in)
        }
        let t3 = totals();
        assert!((t3[2] - t2[2]) / 1_000_000 >= 10);
        // The cost: a guard taken and dropped, in this thread's CPU time (task #921: a loaded Mac
        // stretches the wall clock, not the work done per guard).
        let n = 200_000;
        let c0 = crate::testkit::thread_cpu_ns();
        for _ in 0..n {
            drop(disk());
        }
        let each = (crate::testkit::thread_cpu_ns() - c0) as f64 / n as f64;
        eprintln!("a guard costs {each:.0} ns");
        assert!(each < 20_000.0, "{each} ns of CPU a guard"); // (bound ok: CPU time, not wall)
    }

    /// Task #1104: the waits as intervals, each kind's apart (a request beside a NAS call is in
    /// both); two of a kind within the gap one interval, `n` 2, `busy` their time; past it, two; a
    /// kind waited on as the job ends cut there (`take(true)`); the store's notes with them.
    #[test]
    fn waits_come_as_intervals_merged_within_the_gap() {
        // (Alone in a process of its own: the waits are the process's.)
        const ALONE: &str = "SCENIC_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let out = crate::spawn::output(
                crate::spawn::command(std::env::current_exe().unwrap())
                    .args(["--exact", "waits::tests::waits_come_as_intervals_merged_within_the_gap"])
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
        // (No note before intervals are asked for.)
        note("retry", "unheard".into());
        // A gap no sleep can pass: the two NAS calls merge, however loaded the Mac.
        intervals_from(Instant::now(), 3_600_000_000_000);
        for _ in 0..2 {
            let _g = nas();
            let _n = net();
            std::thread::sleep(Duration::from_millis(5)); // (sleep ok: a NAS call's stand-in)
        }
        note("retry", "a read: EIO".into());
        // (Still within the gap: nothing ended yet.)
        let (ended, notes) = take(false);
        assert!(ended.is_empty());
        let (iv, _) = take(true);
        assert_eq!(iv.len(), 2, "{iv:?}");
        for (i, on) in iv.iter().zip([On::Nas, On::Net]) {
            assert_eq!((i.on, i.n), (on, 2), "{i:?}");
            assert!(i.busy >= 10_000_000 && i.busy <= i.t1 - i.t0, "{i:?}");
        }
        assert_eq!(notes.iter().map(|n| (n.1, n.2.as_str())).collect::<Vec<_>>(), [("retry", "a read: EIO")]);
        // No gap: two calls, two intervals; one open as the job ends, cut there.
        set_gap(0);
        for _ in 0..2 {
            let _g = disk();
            std::thread::sleep(Duration::from_millis(2)); // (sleep ok: a disk call's stand-in)
        }
        let held = other();
        let (iv, _) = take(true);
        drop(held);
        let kinds: Vec<On> = iv.iter().map(|i| i.on).collect();
        assert_eq!(kinds, [On::Disk, On::Disk, On::Other], "{iv:?}");
        assert!(iv.iter().all(|i| i.n == 1 && i.t1 >= i.t0));
    }

    /// Task #1097: two threads computing on two turns hold both; a taker waits until one of them
    /// waits (on the NAS, the network or other work, not the disk), and runs on the turn it lends;
    /// the lender back takes its turn without waiting (three held, until the taker's ends), and
    /// nested waits lend one turn.
    #[test]
    fn a_taker_computes_on_the_turns_waits_lend() {
        use crate::testkit::{eventually, Signal};
        let turns = Turns::new(2);
        let (go_wait, waiting, go_back, back, go_end) =
            (Signal::new(), Signal::new(), Signal::new(), Signal::new(), Signal::new());
        let took = Signal::new();
        std::thread::scope(|s| {
            // A: computing; lends its turn while it waits on other work (a disk wait inside lends
            // nothing more), then takes it back.
            s.spawn(|| {
                turns.computing(|| {
                    go_wait.wait("A told to wait");
                    {
                        let _d = disk();
                        let _w = other();
                        let _n = nas();
                        waiting.set();
                        go_back.wait("A told back");
                    }
                    back.set();
                    go_end.wait("A told to end");
                })
            });
            // B: computing throughout.
            s.spawn(|| turns.computing(|| go_end.wait("B told to end")));
            eventually("both computing", || turns.held() == 2);
            // C: takes a turn, which waits while both are held.
            s.spawn(|| {
                let _t = turns.take();
                took.set();
                go_end.wait("C told to end");
            });
            assert!(!took.is_set(), "every turn held: the taker waits");
            go_wait.set();
            waiting.wait("A waiting");
            took.wait("C on A's turn");
            assert_eq!(turns.held(), 2, "A's turn lent once for its nested waits, C's taken");
            go_back.set();
            back.wait("A back");
            assert_eq!(turns.held(), 3, "A back without waiting: over by one until C ends");
            go_end.set();
        });
        assert_eq!(turns.held(), 0);
        // A thread holding none lends none.
        drop(other());
        assert_eq!(turns.held(), 0);
    }
}
