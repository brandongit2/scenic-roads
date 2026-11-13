//! The unit tests' allocator: the system's, counting what is asked of it and, while a test turns
//! it on (`on`), sampling the call stack of one allocation per `EVERY` bytes a thread asks for
//! (in blocks of a least size, when given one), so
//! a run on real data (`plan::tests::frozen_plans_memory`) ranks the code by the bytes it
//! allocates, freed since or not, rather than by what is live. Off (every other test), an
//! allocation costs two relaxed adds.
//!
//! And this process's memory as macOS counts it (`Memory::now`): its footprint, its resident size
//! and its reusable pages (freed, told to the kernel as no longer needed, still resident:
//! docs/pool.md §7.1).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

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
/// The sampled stacks, each standing for `EVERY` bytes.
static SAMPLES: Mutex<Vec<std::backtrace::Backtrace>> = Mutex::new(Vec::new());

thread_local! {
    /// The bytes this thread asked for since its last sample.
    static SINCE: Cell<u64> = const { Cell::new(0) };
    /// Set while this thread samples: the sample's own allocations aren't counted.
    static SAMPLING: Cell<bool> = const { Cell::new(false) };
}

fn freed(size: usize) {
    LIVE[bucket(size)].fetch_sub(size as i64, Relaxed);
    LIVE_ALL.fetch_sub(size as i64, Relaxed);
}

fn count(size: usize) {
    BYTES.fetch_add(size as u64, Relaxed);
    CALLS.fetch_add(1, Relaxed);
    let k = bucket(size);
    ASKED[k].fetch_add(size as u64, Relaxed);
    MOST[k].fetch_max(LIVE[k].fetch_add(size as i64, Relaxed) + size as i64, Relaxed);
    MOST_ALL.fetch_max(LIVE_ALL.fetch_add(size as i64, Relaxed) + size as i64, Relaxed);
    if !ON.load(Relaxed) || (size as u64) < LEAST.load(Relaxed) {
        return;
    }
    // (A thread ending has no thread-locals left: not sampled.)
    let due = SINCE.try_with(|s| {
        let n = s.get() + size as u64;
        s.set(n % EVERY);
        n >= EVERY
    });
    if due != Ok(true) || SAMPLING.try_with(|b| b.replace(true)) != Ok(false) {
        return;
    }
    let stack = std::backtrace::Backtrace::force_capture();
    if let Ok(mut s) = SAMPLES.lock() {
        s.push(stack);
    }
    SAMPLING.try_with(|b| b.set(false)).ok();
}

// SAFETY: every call goes to the system's allocator as given; the counting allocates only while
// `SAMPLING` keeps it from counting itself.
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

/// Sampling on or off; on, of the blocks of `least` bytes or more (the stacks then rank who asks
/// for those).
pub fn on(yes: bool, least: u64) {
    LEAST.store(least, Relaxed);
    ON.store(yes, Relaxed);
}

/// The bytes asked for and the calls so far, whole process.
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
    let stacks = std::mem::take(&mut *SAMPLES.lock().unwrap());
    let mb = |n: u64| n as f64 * EVERY as f64 / rounds.max(1) as f64 / (1u64 << 20) as f64;
    let mut out = vec![format!("{} stacks sampled, one per {} KB, over {rounds} round(s)", stacks.len(), EVERY >> 10)];
    let (mut under, mut site): (BTreeMap<String, u64>, BTreeMap<String, u64>) = Default::default();
    for s in &stacks {
        let ours = our_frames(&s.to_string());
        // (A site with its two callers: one helper is reached from many places.)
        let at = match ours.is_empty() {
            true => "(no workspace function on the stack)".to_string(),
            false => ours.iter().take(3).cloned().collect::<Vec<_>>().join(" < "),
        };
        *site.entry(at).or_default() += 1;
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

/// The functions of this workspace's crates on a printed stack, innermost first, without their
/// hashes and closures' marks.
fn our_frames(stack: &str) -> Vec<String> {
    const OURS: [&str; 22] = [
        "agentcore::",
        "pipeline::",
        "buildstore::",
        "inputs::",
        "terrain::",
        "trees::",
        "roads::",
        "names::",
        "bld::",
        "landmarks::",
        "offload::",
        "store::",
        "jobrt::",
        "codecs::",
        "coordwire::",
        "roadcore::",
        "det::",
        "mapview::",
        "pyport::",
        "webpenc::",
        "server::",
        "scenic",
    ];
    stack
        .lines()
        .filter_map(|l| l.trim_start().split_once(": ").filter(|(n, _)| n.parse::<u32>().is_ok()).map(|(_, f)| f))
        .filter(|f| !f.contains("agent::allocs::"))
        .filter(|f| {
            let f = f.trim_start_matches('<');
            OURS.iter().any(|o| f.starts_with(o))
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
        let mut r: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        let got = unsafe {
            libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V4,
                &mut r as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
            )
        } == 0;
        Memory {
            footprint: if ok { v.phys_footprint } else { 0 },
            peak: if got { r.ri_lifetime_max_phys_footprint } else { 0 },
            resident: if ok { v.resident_size } else { 0 },
            reusable: if ok { v.reusable } else { 0 },
        }
    }
}
