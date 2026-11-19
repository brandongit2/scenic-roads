//! The unit tests' allocator: the system's. Once a test asks (`counting`), it counts what is asked
//! of it: the bytes and blocks, and by block size the bytes asked for, those live and the most
//! that were live at once (`sizes`). While a test turns sampling on (`on`), it keeps the call
//! stack of one allocation per `EVERY` bytes a thread asks for (in blocks of a least size, when
//! given one), so a run on real data (`plan::tests::frozen_plans_memory`) ranks the code by the
//! bytes it allocates, freed since or not, rather than by what is live (`report`). In every other
//! test, an allocation or a free costs one relaxed load.
//!
//! And this process's memory as macOS counts it (`Memory::now`): its footprint, its resident size
//! and its reusable pages (freed, told to the kernel as no longer needed, still resident:
//! docs/pool.md §7.1).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU64, Ordering::Relaxed};

/// A stack sampled per this many bytes asked for by a thread.
const EVERY: u64 = 4 << 20;

/// Blocks are counted by size in powers of two: bucket `k` holds those of 2^k bytes to under
/// 2^(k+1), the last everything from 2^31.
const BUCKETS: usize = 32;

fn bucket(size: usize) -> usize {
    (size.max(1).ilog2() as usize).min(BUCKETS - 1)
}

pub struct Counting;

#[global_allocator]
static COUNTING: Counting = Counting;

/// Whether it counts (`counting`), and whether it samples (`on`).
static COUNTS: AtomicBool = AtomicBool::new(false);
static ON: AtomicBool = AtomicBool::new(false);
/// The least block sampled, in bytes.
static LEAST: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
/// By bucket: the bytes asked for, the bytes live (asked for less freed) and the most they were.
static ASKED: [AtomicU64; BUCKETS] = [const { AtomicU64::new(0) }; BUCKETS];
static LIVE: [AtomicI64; BUCKETS] = [const { AtomicI64::new(0) }; BUCKETS];
static MOST: [AtomicI64; BUCKETS] = [const { AtomicI64::new(0) }; BUCKETS];
/// The bytes live in all, and the most they were.
static LIVE_ALL: AtomicI64 = AtomicI64::new(0);
static MOST_ALL: AtomicI64 = AtomicI64::new(0);
/// The sampled stacks, each standing for `EVERY` bytes: a list the samplers push onto and `report`
/// takes whole (no lock: a thread sampling holds none while it allocates).
static SAMPLES: AtomicPtr<Sample> = AtomicPtr::new(std::ptr::null_mut());
/// Whether the peaks are sampled (`peaks_on`): by bucket, the stack of the block that took the
/// bucket's live bytes `EVERY` past the last one sampled (`PEAK_AT`), so a bucket's stacks are who
/// held its most (`peaks`).
static PEAKS_ON: AtomicBool = AtomicBool::new(false);
static PEAK_AT: [AtomicI64; BUCKETS] = [const { AtomicI64::new(0) }; BUCKETS];
static PEAK_SAMPLES: AtomicPtr<Sample> = AtomicPtr::new(std::ptr::null_mut());

struct Sample {
    stack: std::backtrace::Backtrace,
    /// Its block's bucket (a peak's sample).
    bucket: usize,
    next: *mut Sample,
}

thread_local! {
    /// The bytes this thread asked for since its last sample.
    static SINCE: Cell<u64> = const { Cell::new(0) };
    /// Set while this thread samples: the sample's own allocations are counted, never sampled.
    static SAMPLING: Cell<bool> = const { Cell::new(false) };
}

fn freed(size: usize) {
    if !COUNTS.load(Relaxed) {
        return;
    }
    LIVE[bucket(size)].fetch_sub(size as i64, Relaxed);
    LIVE_ALL.fetch_sub(size as i64, Relaxed);
}

fn count(size: usize) {
    if !COUNTS.load(Relaxed) {
        return;
    }
    BYTES.fetch_add(size as u64, Relaxed);
    CALLS.fetch_add(1, Relaxed);
    let k = bucket(size);
    ASKED[k].fetch_add(size as u64, Relaxed);
    let live = LIVE[k].fetch_add(size as i64, Relaxed) + size as i64;
    MOST[k].fetch_max(live, Relaxed);
    MOST_ALL.fetch_max(LIVE_ALL.fetch_add(size as i64, Relaxed) + size as i64, Relaxed);
    if PEAKS_ON.load(Relaxed) {
        let at = PEAK_AT[k].load(Relaxed);
        if live >= at + EVERY as i64 && PEAK_AT[k].compare_exchange(at, live, Relaxed, Relaxed).is_ok() {
            sample(&PEAK_SAMPLES, k);
        }
    }
    if !ON.load(Relaxed) || (size as u64) < LEAST.load(Relaxed) {
        return;
    }
    // (A thread ending has no thread-locals left: not sampled.)
    let due = SINCE.try_with(|s| {
        let n = s.get() + size as u64;
        s.set(n % EVERY);
        n >= EVERY
    });
    if due == Ok(true) {
        sample(&SAMPLES, k);
    }
}

/// This thread's stack pushed onto `list`, its block of bucket `k`; none while it samples already.
fn sample(list: &AtomicPtr<Sample>, bucket: usize) {
    if SAMPLING.try_with(|b| b.replace(true)) != Ok(false) {
        return;
    }
    let stack = std::backtrace::Backtrace::force_capture();
    let new = Box::into_raw(Box::new(Sample { stack, bucket, next: std::ptr::null_mut() }));
    let mut head = list.load(Relaxed);
    loop {
        // SAFETY: `new` is this thread's alone until the exchange below hands it over.
        unsafe { (*new).next = head };
        match list.compare_exchange_weak(head, new, Relaxed, Relaxed) {
            Ok(_) => break,
            Err(h) => head = h,
        }
    }
    SAMPLING.try_with(|b| b.set(false)).ok();
}

/// The samples pushed onto `list` since it was last taken.
fn take(list: &AtomicPtr<Sample>) -> Vec<(usize, std::backtrace::Backtrace)> {
    let mut out = Vec::new();
    let mut at = list.swap(std::ptr::null_mut(), Relaxed);
    while !at.is_null() {
        // SAFETY: the list taken whole is this call's alone; each node was a `Box` made in `sample`.
        let s = unsafe { Box::from_raw(at) };
        at = s.next;
        out.push((s.bucket, s.stack));
    }
    out
}

// SAFETY: every call goes to the system's allocator as given; the counting allocates only while
// `SAMPLING` keeps it from sampling itself.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        count(l.size());
        System.alloc(l)
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        count(l.size());
        System.alloc_zeroed(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        freed(l.size());
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // (A block of another size: the old one freed, the new one asked for.)
        freed(l.size());
        count(new);
        System.realloc(p, l, new)
    }
}

/// Counts from now on, for the rest of the process: called as a test starts, before what it
/// measures allocates (a block from before it, freed after, is taken off the live bytes without
/// having been added).
pub fn counting() {
    COUNTS.store(true, Relaxed);
}

/// Sampling on or off; on, of the blocks of `least` bytes or more (the stacks then rank who asks
/// for those).
pub fn on(yes: bool, least: u64) {
    LEAST.store(least, Relaxed);
    ON.store(yes, Relaxed);
}

/// The bytes asked for and the blocks, since `counting`, whole process.
pub fn asked() -> (u64, u64) {
    (BYTES.load(Relaxed), CALLS.load(Relaxed))
}

/// The blocks by size since the last call, as lines: for each bucket that held 1 MB or more, the
/// bytes asked for, the most that were live at once and what is live now; then the same in all, beside the sum
/// of the buckets' mosts (what an allocator that never gives one size's freed pages to another
/// size keeps resident). The mosts start again from what is live now.
pub fn sizes() -> Vec<String> {
    let mb = |b: i64| b as f64 / (1u64 << 20) as f64;
    let mut out = Vec::new();
    let mut sum = 0;
    for k in 0..BUCKETS {
        let now = LIVE[k].load(Relaxed);
        let (asked, most) = (ASKED[k].swap(0, Relaxed), MOST[k].swap(now, Relaxed));
        sum += most.max(0);
        if most >= 1 << 20 || asked >= 1 << 20 {
            out.push(format!(
                "blocks of {} B and up: asked for {:.0} MB, at most {:.0} MB live, {:.0} MB now",
                1u64 << k,
                mb(asked as i64),
                mb(most),
                mb(now)
            ));
        }
    }
    let now = LIVE_ALL.load(Relaxed);
    out.push(format!(
        "at most {:.0} MB live in all, {:.0} MB now; the sizes' mosts add up to {:.0} MB",
        mb(MOST_ALL.swap(now, Relaxed)),
        mb(now),
        mb(sum)
    ));
    out
}

/// The samples taken since the last call, as lines: the functions
/// of this workspace's crates ranked by the bytes allocated under them (a stack counts once for
/// each function on it, so a caller's line holds its callees'), `top` of them, then the innermost
/// workspace function of each stack ranked alike (the sites), each in MB per `rounds`.
pub fn report(rounds: u64, top: usize) -> Vec<String> {
    let stacks = take(&SAMPLES);
    let mb = |n: u64| n as f64 * EVERY as f64 / rounds.max(1) as f64 / (1u64 << 20) as f64;
    let mut out = vec![format!("{} stacks sampled, one per {} KB, over {rounds} round(s)", stacks.len(), EVERY >> 10)];
    let (mut under, mut site): (BTreeMap<String, u64>, BTreeMap<String, u64>) = Default::default();
    for (_, s) in &stacks {
        let ours = our_frames(&s.to_string());
        *site.entry(site_of(&ours)).or_default() += 1;
        let mut seen = std::collections::BTreeSet::new();
        for f in ours {
            if seen.insert(f.clone()) {
                *under.entry(f).or_default() += 1;
            }
        }
    }
    for (name, map) in [("under", under), ("site", site)] {
        let mut v: Vec<(String, u64)> = map.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out.extend(v.into_iter().take(top).map(|(f, n)| format!("{name} {:8.1} MB a round  {f}", mb(n))));
    }
    out
}

/// Since the last call: the most the live bytes rose over what was live then, and the sum of each
/// bucket's most rise (what an allocator keeping each size's freed pages for that size takes on
/// for it, over what it had); the mosts start again from now.
pub fn climb() -> (u64, u64) {
    static BASE: [AtomicI64; BUCKETS] = [const { AtomicI64::new(0) }; BUCKETS];
    static BASE_ALL: AtomicI64 = AtomicI64::new(0);
    let mut sum = 0;
    for k in 0..BUCKETS {
        let now = LIVE[k].load(Relaxed);
        sum += (MOST[k].swap(now, Relaxed) - BASE[k].swap(now, Relaxed)).max(0);
    }
    let now = LIVE_ALL.load(Relaxed);
    let all = (MOST_ALL.swap(now, Relaxed) - BASE_ALL.swap(now, Relaxed)).max(0);
    (all as u64, sum as u64)
}

/// A stack's site: its innermost workspace function with its two callers (one helper is reached
/// from many places).
fn site_of(ours: &[String]) -> String {
    match ours.is_empty() {
        true => "(no workspace function on the stack)".to_string(),
        false => ours.iter().take(3).cloned().collect::<Vec<_>>().join(" < "),
    }
}

/// The peaks sampled (`peaks_on`) or not from now on; on, from the bytes live now.
pub fn peaks_on(yes: bool) {
    for k in 0..BUCKETS {
        PEAK_AT[k].store(LIVE[k].load(Relaxed), Relaxed);
    }
    PEAKS_ON.store(yes, Relaxed);
}

/// The peaks sampled since the last call, as lines: for each bucket by the most of its bytes they
/// stand for, the `top` sites whose blocks took its live bytes higher, each in MB of that climb
/// (a stack per `EVERY` of it).
pub fn peaks(top: usize) -> Vec<String> {
    let mb = |n: u64| (n * EVERY) as f64 / (1u64 << 20) as f64;
    let mut by: BTreeMap<usize, BTreeMap<String, u64>> = BTreeMap::new();
    for (k, s) in take(&PEAK_SAMPLES) {
        *by.entry(k).or_default().entry(site_of(&our_frames(&s.to_string()))).or_default() += 1;
    }
    let mut buckets: Vec<(usize, BTreeMap<String, u64>)> = by.into_iter().collect();
    buckets.sort_by_key(|(k, s)| (std::cmp::Reverse(s.values().sum::<u64>()), *k));
    let mut out = Vec::new();
    for (k, sites) in buckets {
        out.push(format!(
            "peak of blocks of {} B and up: {:.0} MB of climb sampled",
            1u64 << k,
            mb(sites.values().sum())
        ));
        let mut v: Vec<(String, u64)> = sites.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out.extend(v.into_iter().take(top).map(|(f, n)| format!("  {:8.1} MB  {f}", mb(n))));
    }
    out
}

/// The functions of this workspace's crates on a printed stack, innermost first, without their
/// hashes and closures' marks.
fn our_frames(stack: &str) -> Vec<String> {
    // (The workspace's crates, and its programs' names' start.)
    const OURS: &str =
        "agentcore pipeline buildstore inputs terrain trees roads names bld landmarks offload store jobrt \
                        codecs coordwire roadcore det mapview pyport webpenc server scenic";
    stack
        .lines()
        .filter_map(|l| l.trim_start().split_once(": ").filter(|(n, _)| n.parse::<u32>().is_ok()).map(|(_, f)| f))
        .filter(|f| !f.contains("agent::allocs::"))
        .filter(|f| {
            let f = f.trim_start_matches('<');
            OURS.split(' ').any(|o| f.strip_prefix(o).is_some_and(|rest| rest.starts_with("::") || o == "scenic"))
        })
        .map(|f| f.replace("::{{closure}}", "").to_string())
        .collect()
}

/// This process's memory, in bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Memory {
    pub footprint: u64,
    /// The footprint's most since the process started.
    pub peak: u64,
    pub resident: u64,
    /// Freed pages still resident (`MADV_FREE_REUSABLE`).
    pub reusable: u64,
}

impl Memory {
    pub fn now() -> Memory {
        /// xnu's `task_vm_info` up to `phys_footprint` (osfmk/mach/task_info.h, packed to 4).
        #[repr(C, packed(4))]
        #[derive(Default)]
        struct TaskVm {
            virtual_size: u64,
            region_count: i32,
            page_size: i32,
            resident_size: u64,
            resident_size_peak: u64,
            device: [u64; 2],
            internal: [u64; 2],
            external: [u64; 2],
            reusable: u64,
            reusable_peak: u64,
            purgeable: [u64; 3],
            compressed: [u64; 3],
            phys_footprint: u64,
        }
        const TASK_VM_INFO: u32 = 22;
        let mut v = TaskVm::default();
        let mut n = (std::mem::size_of::<TaskVm>() / 4) as u32;
        // SAFETY: both calls fill a struct we own, of the flavour and size asked for.
        let ok = unsafe {
            #[allow(deprecated)]
            let me = libc::mach_task_self();
            libc::task_info(me, TASK_VM_INFO, &mut v as *mut TaskVm as *mut i32, &mut n) == 0
        };
        Memory {
            footprint: if ok { v.phys_footprint } else { 0 },
            peak: jobrt::sys::rusage(std::process::id() as i32).map_or(0, |r| r.peak),
            resident: if ok { v.resident_size } else { 0 },
            reusable: if ok { v.reusable } else { 0 },
        }
    }
}
