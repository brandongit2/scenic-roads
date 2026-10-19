//! A job far over its memory (docs/pool.md §7.2): macOS swaps rather than kills, so a job holding
//! more than its Mac has leaves the Mac swapping for as long as it runs, every other job and the
//! owner's own work with it.
//!
//! - **Sampled on a thread of its own** (`Sampler`, every `EVERY`, whatever the agent's loop is
//!   waiting on: a share that stalls stalls the loop, not this): each running job's processes'
//!   physical footprints summed (crate::sys::footprint_of_group), kept per target as the most the
//!   job held while that target was under way (`Watch::current`).
//! - **Learned whatever the switch:** what a job held while a target was under way is that target's
//!   **floor** (`Floor`: crate::coord's floors), the same kind of figure as a run's own measure (a
//!   job's processes together, sampled), which takes its place.
//! - **Guarded, the switch on** (`on`, `SWITCH`): while the jobs on a Mac hold more together than its
//!   limit (`limit_mb`), by measure alone (`decide_by`): when the largest fits alone, another stops
//!   at its next safe point (the newest, then the least progressed, one at a time while those left
//!   still pass the limit); the largest, past the limit by itself, stops at once
//!   only while the Mac is in trouble (`trouble`: the kernel's memory pressure at warning or worse,
//!   or a GB more swap since the job began; the thread freezes it then, at once, the agent's loop
//!   stopping it), else at its next safe point too. A Mac whose memory can't be read guards nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The switch, on the NAS: `off` in it turns the guard off, `on` on; missing, the guard is as
/// `DEFAULT` says.
pub const SWITCH: &str = "state/pool/memory-guard";

/// Whether the guard stops jobs when its switch is missing (the owner's choice: on).
pub const DEFAULT: bool = true;

/// How often the jobs' memory is sampled.
pub const EVERY: Duration = Duration::from_secs(5);

/// How often the top of each job's scratch folder is looked at for the disk it takes (docs/pool.md
/// §7.7: the allocated sizes of the files there, no walk of its tree), and the window a job's disk
/// growth is judged over.
pub const DISK_EVERY: Duration = Duration::from_secs(60);

/// The swap that, grown within `SWAP_WINDOW`, takes the Mac to be in trouble.
const SWAP_GROWTH: u64 = 1 << 30;
/// The window swap growth is judged over: recent, so the owner's own use growing swap hours ago
/// doesn't leave a long job in trouble for good.
pub const SWAP_WINDOW: Duration = Duration::from_secs(300);

/// The guard as the switch at `root` says: on or off; None when it can't be read now (the share not
/// answering: the caller keeps what it had).
pub fn on(root: &Path) -> Option<bool> {
    match super::nasread::read_switch(&root.join(SWITCH)) {
        Ok(s) => Some(match s.trim() {
            "off" => false,
            "on" => true,
            _ => DEFAULT,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(DEFAULT),
        Err(_) => None,
    }
}

/// The most memory the jobs on a Mac of `total_mb` may hold together (MB): its memory less an eighth
/// for macOS and the owner's own work, 4 GB at least (42 GB of the M4's 48, 12 of the M1's 16); 0 for
/// a Mac whose memory isn't known (the guard then guards nothing).
pub fn limit_mb(total_mb: u64) -> u64 {
    total_mb.saturating_sub((total_mb / 8).max(4096))
}

/// What the guard does about the jobs in a Mac's slots, by what each holds now (MB; None: no job in
/// that slot, or its memory unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Nothing: the jobs fit the limit together.
    Nothing,
    /// Slot k's job stops at its next safe point: the job beside it fits the limit alone.
    Drain(usize),
    /// Slot k's job holds more than the limit by itself.
    Over(usize),
}

/// The guard's decision (`Act`) for jobs in slot order, the first slot's drained first among the
/// others (`decide_by` with each slot's rank its order): two jobs, the one beside the largest.
pub fn decide(held: &[Option<u64>], limit: u64, draining: &[bool], yields: &[bool]) -> Act {
    let rank: Vec<u64> = (0..held.len()).map(|k| u64::MAX - k as u64).collect();
    decide_by(held, limit, draining, yields, &rank)
}

/// The guard's decision (`Act`), by measure alone, for any number of jobs: nothing while the jobs
/// together fit `limit`; else the largest when it alone passes it; else, it fitting alone, one other
/// drained at a time: the one ranked highest (`rank`: the agent ranks the newest job highest, then
/// the least progressed), not draining yet, and only while the jobs not draining (what's left once
/// those draining reach their safe points) still pass the limit. A job that `yields` (started beside
/// one that waited on a worker, which then went on) is the one drained, larger or not, and while it
/// drains no other is: the job it was started beside isn't stopped for it.
pub fn decide_by(held: &[Option<u64>], limit: u64, draining: &[bool], yields: &[bool], rank: &[u64]) -> Act {
    let total: u64 = held.iter().flatten().sum();
    if limit == 0 || total <= limit {
        return Act::Nothing;
    }
    let Some((big, mb)) =
        held.iter().enumerate().filter_map(|(k, m)| m.map(|m| (k, m))).max_by_key(|&(k, m)| (m, std::cmp::Reverse(k)))
    else {
        return Act::Nothing;
    };
    if mb > limit {
        return Act::Over(big);
    }
    let yielding = |k: usize| yields.get(k).copied().unwrap_or(false) && held[k].is_some();
    if let Some(k) = (0..held.len()).find(|&k| yielding(k)) {
        return if draining.get(k).copied().unwrap_or(false) { Act::Nothing } else { Act::Drain(k) };
    }
    let drains = |k: usize| draining.get(k).copied().unwrap_or(false);
    // (What stays once the jobs draining stop: within the limit, nothing more drains.)
    let staying: u64 = held.iter().enumerate().filter(|&(k, _)| !drains(k)).filter_map(|(_, m)| *m).sum();
    if staying <= limit {
        return Act::Nothing;
    }
    match held
        .iter()
        .enumerate()
        .filter(|&(k, m)| k != big && m.is_some() && !drains(k))
        .max_by_key(|&(k, _)| (rank.get(k).copied().unwrap_or(0), std::cmp::Reverse(k)))
    {
        Some((k, _)) => Act::Drain(k),
        None => Act::Nothing,
    }
}

/// Whether the Mac is in trouble for its memory: the kernel's memory pressure at warning or worse
/// (`pressure`), or a GB more swap in use (`swap`) than the least in use in the last few minutes
/// (`swap_low`, over `SWAP_WINDOW`).
pub fn trouble(pressure: Option<u32>, swap: Option<u64>, swap_low: Option<u64>) -> bool {
    pressure.is_some_and(|p| p >= 2)
        || matches!((swap, swap_low), (Some(now), Some(low)) if now >= low.saturating_add(SWAP_GROWTH))
}

/// The target a job is on, as its costs file says (`SCENIC_COSTS`: a `started` line as each target
/// begins, a `peak_mb` line as it ends): the last begun and not ended, by its cost key
/// (crate::coord::cost_key); None when the file says none (a step that notes no costs, or between
/// targets).
pub fn current(costs: &Path) -> Option<String> {
    let s = std::fs::read_to_string(costs).ok()?;
    let mut on: Option<String> = None;
    for l in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let Some(key) = v["unit"].as_str() else { continue };
        if v.get("started").is_some() {
            on = Some(key.to_string());
        } else if v.get("peak_mb").is_some() && on.as_deref() == Some(key) {
            on = None;
        }
    }
    on
}

/// What a target takes at least (crate::coord's floors): the most a job held while it was under way
/// (MB); whether that job held it alone (one target: a batch's caches, filled over its earlier
/// targets, aren't the last's, so only a floor learned alone holds a target off a Mac); and the way
/// its step ran then (crate::coord::cost_version).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "FloorRead")]
pub struct Floor {
    pub mb: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alone: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub v: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// A floor as read: as written now, or a bare MB (a build before floors said more), learned in a
/// batch the way its step ran first.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum FloorRead {
    Mb(u64),
    Full {
        mb: u64,
        #[serde(default)]
        alone: bool,
        #[serde(default)]
        v: u32,
    },
}

impl From<FloorRead> for Floor {
    fn from(r: FloorRead) -> Floor {
        match r {
            FloorRead::Mb(mb) => Floor { mb, alone: false, v: 0 },
            FloorRead::Full { mb, alone, v } => Floor { mb, alone, v },
        }
    }
}

/// The step of a cost key (crate::coord::cost_key): a unit's is its target alone.
pub fn step_of_key(key: &str) -> &str {
    match key.split_once(' ') {
        Some((s, _)) => s,
        None => "unit",
    }
}

/// A running job as the guard watches it.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    pub pgid: i32,
    /// Its costs file (`SCENIC_COSTS`) and where it notes its targets done (`SCENIC_DONE`).
    pub costs: PathBuf,
    pub done: PathBuf,
    pub step: String,
    pub targets: Vec<String>,
    /// The cost key of a job of no targets of its own (the OSM pass: `osm-pass <date>`), else None.
    pub own_key: Option<String>,
    /// The most it held while each target was under way (MB, by cost key), and what it holds now.
    pub seen: BTreeMap<String, u64>,
    pub held: Option<u64>,
    /// Whether the guard froze it (the Mac in trouble with it past the limit alone: the agent's loop
    /// stops it).
    pub frozen: bool,
    /// Where its accessors keep the bytes they've added to the caches (store::cachefile's
    /// `WRITTEN_ENV`), and its scratch folder (its top looked at every `DISK_EVERY`).
    pub written: PathBuf,
    pub scratch: Option<PathBuf>,
    /// The disk it takes now, by those counts (bytes): what its accessors wrote, and the files at
    /// the top of its scratch folder as last looked at.
    pub disk_now: u64,
    /// The most it took on the disk while each target was under way (bytes, by cost key): its
    /// scratch folder's top and what its accessors wrote since that target began.
    pub disk_seen: BTreeMap<String, u64>,
    /// The disk's free space as it began (bytes; None: unknown), and whether it has run alone so far.
    pub free_start: Option<u64>,
    pub alone: bool,
    /// The disk guard froze it (the Mac short of room, it growing fastest: the agent's loop makes
    /// room, then lets it go on or gives it back).
    pub disk_frozen: bool,
    /// The most it took on the disk over its whole run, by its counts (bytes): a batch's, for the
    /// next jobs of its step (crate::coord's `job_disks`).
    pub disk_peak: u64,
    /// The most the disk's free space fell while it ran alone (bytes): a weaker signal, logged
    /// beside its counts, never a floor.
    pub disk_fell: u64,
    /// Its disk counts over the last `DISK_EVERY` (when, bytes), for how fast it grows.
    pub disk_hist: std::collections::VecDeque<(std::time::Instant, u64)>,
    /// The target under way as its disk was last counted, and what its accessors had written and the
    /// disk had free as it began.
    pub disk_key: Option<(String, u64, Option<u64>)>,
    /// The top of its scratch folder as last looked at (bytes), and when.
    pub scratch_top: u64,
    pub looked: Option<std::time::Instant>,
    /// When it last used a core-minute more (`BUSY_CPU`), and its CPU time (ns) then (the first
    /// sample, or a sum that fell as a process ended, starts it again): how long it has been idle,
    /// for an update's wait (crate::agent's `update_due`), whatever its progress lines say.
    pub cpu_mark: Option<(std::time::Instant, u64)>,
}

/// The CPU time (ns) a job uses for its sampler to call it busy again: a core-minute.
pub const BUSY_CPU: u64 = 60_000_000_000;

/// The allocated bytes of the files at the top of folder `dir` (not its subfolders': no walk of its
/// tree; Planetiler's and osmium's outputs, the largest a job makes, are there).
pub fn top_bytes(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                m.blocks() * 512
            }
            #[cfg(not(unix))]
            m.len()
        })
        .sum()
}

/// The disk each target of a job took, as its costs file notes it (`disk_mb` on a target's end:
/// what its accessors wrote meanwhile), by cost key (MB).
pub fn noted_disks(costs: &Path) -> Vec<(String, u64)> {
    let Ok(s) = std::fs::read_to_string(costs) else { return Vec::new() };
    s.lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            Some((v["unit"].as_str()?.to_string(), v["disk_mb"].as_u64()?))
        })
        .collect()
}

impl Watch {
    /// The cost key of the target it's on: the last its costs file says began and hasn't ended;
    /// else the first of its targets it hasn't noted done; else its own (`own_key`).
    pub fn current(&self) -> Option<String> {
        if let Some(k) = current(&self.costs) {
            return Some(k);
        }
        if !self.targets.is_empty() {
            let done = crate::control::read_done(&self.done, &self.step);
            return self.targets.iter().find(|t| !done.contains(*t)).map(|t| crate::coord::cost_key(&self.step, t));
        }
        self.own_key.clone()
    }

    /// The floors its run teaches (cost key → floor), from what it held while each target was under
    /// way, but for the targets its run measured itself (`measured`: a measure takes a floor's place).
    pub fn floors(&self, measured: &[String]) -> Vec<(String, Floor)> {
        let alone = self.targets.len() <= 1;
        let v = crate::coord::cost_version(&self.step);
        self.seen
            .iter()
            .filter(|(k, mb)| !measured.contains(k) && **mb > 0)
            .map(|(k, mb)| (k.clone(), Floor { mb: *mb, alone, v }))
            .collect()
    }

    /// The disk floors its run teaches (cost key → MB): the most it was seen to take while each
    /// target was under way (`disk_seen`), or what its costs file notes its accessors wrote for it,
    /// the larger.
    pub fn disk_floors(&self) -> Vec<(String, u64)> {
        let mut d: BTreeMap<String, u64> = self.disk_seen.iter().map(|(k, b)| (k.clone(), b >> 20)).collect();
        for (k, mb) in noted_disks(&self.costs) {
            let e = d.entry(k).or_insert(0);
            *e = (*e).max(mb);
        }
        // (A target that took nothing keeps its 0: its own measure, not its step's prior.)
        d.into_iter().collect()
    }

    /// The most it took on the disk over its run (MB).
    pub fn job_disk_mb(&self) -> u64 {
        self.disk_peak >> 20
    }

    /// How fast it grows on the disk: its count now less its count `DISK_EVERY` ago (or at the first
    /// count within that).
    pub fn disk_growth(&self) -> u64 {
        match (self.disk_hist.front(), self.disk_hist.back()) {
            (Some((_, a)), Some((_, b))) => b.saturating_sub(*a),
            _ => 0,
        }
    }
}

/// What the sampler's thread and the agent's loop share.
#[derive(Default)]
struct Inner {
    watches: Vec<Option<Watch>>,
    on: bool,
    limit_mb: u64,
    /// As a test sets them: what each slot's job holds (MB), and whether the Mac is in trouble.
    held_set: Option<Vec<Option<u64>>>,
    trouble_set: Option<bool>,
    /// As a test sets it: when each slot's job was last busy (`Watch::cpu_mark`).
    busy_set: Option<Vec<Option<std::time::Instant>>>,
    /// What the thread did last (unix seconds, in words), for the agent's loop to say.
    froze: Vec<(u64, String)>,
    /// The swap in use as each sample read it, over the last `SWAP_WINDOW`.
    swap: std::collections::VecDeque<(std::time::Instant, u64)>,
    /// Where each sample is written (`Live`: the agent's folder's `memory.json`), for the status to
    /// show the latest whatever the agent's loop waits on.
    file: Option<PathBuf>,
    /// The disk guard: whether it freezes a job when the Mac runs short (the agent's switch), the
    /// free space short of which it does (the owner's room target, room::Target: `floor`), and the
    /// folder whose disk it watches.
    disk_on: bool,
    floor: u64,
    disk_at: Option<PathBuf>,
    /// As a test sets them: the disk's free space, and what each slot's job's disk counts (bytes).
    free_set: Option<u64>,
    disk_set: Option<Vec<Option<u64>>>,
    /// The disk's free space as last read (bytes).
    free: Option<u64>,
}

/// The guard's latest sample, written each time by its thread (`LIVE` in the agent's folder): when
/// (unix seconds), what each slot's job holds (MB), whether the thread froze it, and the limit and
/// switch as the agent's loop last set them.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Live {
    pub at: u64,
    pub on: bool,
    pub limit_mb: u64,
    pub held_mb: Vec<Option<u64>>,
    pub frozen: Vec<bool>,
}

/// How far below the owner's room target the disk guard freezes (its hysteresis): a thirtieth of
/// it, 1 GB of a 30 GB target. A job starts with the target free past its own room, so at the
/// target it may be one just let in: frozen there, it would be given back before it ran. The thaw
/// is at the target, so a frozen job doesn't flap; a thirtieth is small beside the room kept.
pub fn disk_slack(floor: u64) -> u64 {
    floor / 30
}

/// The free space short of which the disk guard freezes a job, for the owner's room target
/// `floor`: the target less its slack (`disk_slack`). No target (0): only a full disk (no byte
/// free), no other floor.
pub fn disk_short_of(floor: u64) -> u64 {
    floor.saturating_sub(disk_slack(floor)).max(1)
}

/// The guard's latest sample's file, in the agent's folder.
pub const LIVE: &str = "memory.json";

/// The guard's latest sample in the agent's folder `home`, when there's one.
pub fn live(home: &Path) -> Option<Live> {
    std::fs::read(home.join(LIVE)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// The guard's sampler: a thread sampling the watched jobs every `EVERY` (`spawn`; a test calls
/// `tick` itself), freezing one past the limit alone while the Mac is in trouble.
#[derive(Clone, Default)]
pub struct Sampler {
    inner: Arc<Mutex<Inner>>,
}

impl Sampler {
    pub fn new(slots: usize) -> Sampler {
        Sampler { inner: Arc::new(Mutex::new(Inner { watches: vec![None; slots], ..Default::default() })) }
    }

    /// Writes each sample to `file` (the agent's folder's `LIVE`).
    pub fn write_to(&self, file: PathBuf) {
        if let Ok(mut g) = self.inner.lock() {
            g.file = Some(file);
        }
    }

    /// Starts its thread (once a process: the agent's).
    pub fn spawn(&self) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("memory-guard".into())
            .spawn(move || loop {
                me.tick();
                std::thread::sleep(EVERY);
            })
            .ok();
    }

    /// Watches slot `k`'s job, from its start.
    pub fn watch(&self, k: usize, mut w: Watch) {
        if let Ok(mut g) = self.inner.lock() {
            // (Alone so far when no other job runs; the disk's free space as it begins.)
            w.alone = g.watches.iter().enumerate().all(|(j, o)| j == k || o.is_none());
            w.free_start = g
                .free_set
                .or_else(|| g.disk_at.as_ref().and_then(|d| crate::agent::room::disk_free(d).ok()))
                .or(w.free_start);
            // (A slot past those watched so far, a member's packed jobs: watched from now.)
            if g.watches.len() <= k {
                g.watches.resize(k + 1, None);
            }
            g.watches[k] = Some(w);
        }
    }

    /// Stops watching slot `k`'s job (it ended): what it saw.
    pub fn unwatch(&self, k: usize) -> Option<Watch> {
        self.inner.lock().ok()?.watches.get_mut(k)?.take()
    }

    /// The disk guard: on or off (the agent's switch), the free space short of which it freezes the
    /// job growing fastest (bytes: the owner's room target, 0 none), and the folder on the disk it
    /// watches (the agent's).
    pub fn set_disk(&self, on: bool, floor: u64, at: &Path) {
        if let Ok(mut g) = self.inner.lock() {
            (g.disk_on, g.floor, g.disk_at) = (on, floor, Some(at.to_path_buf()));
        }
    }

    /// A test's disk: its free space, and what each slot's job's disk counts (bytes).
    pub fn set_test_disk(&self, free: Option<u64>, counts: Option<Vec<Option<u64>>>) {
        if let Ok(mut g) = self.inner.lock() {
            (g.free_set, g.disk_set) = (free, counts);
        }
    }

    /// The disk's free space as the last sample read it (bytes).
    pub fn free(&self) -> Option<u64> {
        self.inner.lock().ok()?.free
    }

    /// Whether the disk guard froze slot `k`'s job.
    pub fn disk_frozen(&self, k: usize) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.watches.get(k).and_then(|w| w.as_ref().map(|w| w.disk_frozen)))
            .unwrap_or(false)
    }

    /// What slot `k`'s job takes on the disk now, by its counts (bytes).
    pub fn disk_now(&self, k: usize) -> Option<u64> {
        self.inner.lock().ok()?.watches.get(k)?.as_ref().map(|w| w.disk_now)
    }

    /// Lets slot `k`'s job go on, if the disk guard froze it (room made, or the guard off); true when
    /// it did.
    pub fn thaw_disk(&self, k: usize) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        let real = g.held_set.is_none() && g.disk_set.is_none();
        let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut).filter(|w| w.disk_frozen) else { return false };
        w.disk_frozen = false;
        w.disk_hist.clear();
        // (Still frozen for its memory: it stays so.)
        if w.frozen || !real {
            return true;
        }
        crate::sys::signal_group(w.pgid, crate::sys::Signal::Cont)
    }

    /// Whether it guards (the switch, as the agent read it) and this Mac's limit (MB; 0: unknown).
    pub fn set(&self, on: bool, limit_mb: u64) {
        if let Ok(mut g) = self.inner.lock() {
            (g.on, g.limit_mb) = (on, limit_mb);
        }
    }

    /// A test's figures: what each slot's job holds (MB), and whether the Mac is in trouble.
    pub fn set_test(&self, held: Option<Vec<Option<u64>>>, trouble: Option<bool>) {
        if let Ok(mut g) = self.inner.lock() {
            (g.held_set, g.trouble_set) = (held, trouble);
        }
    }

    /// When slot `k`'s job last used a core-minute (`Watch::cpu_mark`); None: not sampled yet, or
    /// unreadable.
    pub fn busy_at(&self, k: usize) -> Option<std::time::Instant> {
        let g = self.inner.lock().ok()?;
        match &g.busy_set {
            Some(v) => v.get(k).copied().flatten(),
            None => g.watches.get(k)?.as_ref()?.cpu_mark.map(|m| m.0),
        }
    }

    /// As a test sets it: when each slot's job was last busy.
    pub fn set_test_busy(&self, busy: Option<Vec<Option<std::time::Instant>>>) {
        if let Ok(mut g) = self.inner.lock() {
            g.busy_set = busy;
        }
    }

    /// The cost key of the target slot `k`'s job is on (`Watch::current`).
    pub fn current_of(&self, k: usize) -> Option<String> {
        let w = self.inner.lock().ok()?.watches.get(k)?.clone()?;
        w.current()
    }

    /// What slot `k`'s job holds now (MB), as last sampled.
    pub fn held(&self, k: usize) -> Option<u64> {
        self.inner.lock().ok()?.watches.get(k)?.as_ref()?.held
    }

    /// Whether the thread froze slot `k`'s job.
    pub fn frozen(&self, k: usize) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.watches.get(k).and_then(|w| w.as_ref().map(|w| w.frozen)))
            .unwrap_or(false)
    }

    /// Freezes slot `k`'s job (SIGSTOP to its group; a test's figures: marked alone), as the thread
    /// freezes one past the limit alone: the agent's loop, the Mac short of memory with the jobs
    /// together past the limit. True when it froze it now.
    pub fn freeze(&self, k: usize) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        let real = g.held_set.is_none();
        let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut).filter(|w| !w.frozen) else { return false };
        if real {
            crate::sys::signal_group(w.pgid, crate::sys::Signal::Stop);
        }
        w.frozen = true;
        true
    }

    /// Lets slot `k`'s job go on, if the thread froze it (the guard turned off since).
    pub fn thaw(&self, k: usize) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut).filter(|w| w.frozen) else { return false };
        w.frozen = false;
        // (Still frozen for the disk: it stays so.)
        if w.disk_frozen {
            return true;
        }
        crate::sys::signal_group(w.pgid, crate::sys::Signal::Cont)
    }

    /// Whether the Mac is in trouble now (`trouble`: its pressure, or swap grown within the last few
    /// minutes, as the samples read it).
    pub fn in_trouble(&self) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        if let Some(t) = g.trouble_set {
            return t;
        }
        let now = crate::sys::swap_used();
        if let Some(n) = now {
            g.swap.push_back((std::time::Instant::now(), n));
        }
        while g.swap.front().is_some_and(|(t, _)| t.elapsed() > SWAP_WINDOW) {
            g.swap.pop_front();
        }
        let low = g.swap.iter().map(|(_, n)| *n).min();
        drop(g);
        trouble(crate::sys::memory_pressure(), now, low)
    }

    /// What the thread did since the last call, for the agent's loop to note.
    pub fn take_froze(&self) -> Vec<(u64, String)> {
        self.inner.lock().map(|mut g| std::mem::take(&mut g.froze)).unwrap_or_default()
    }

    /// One sample: each watched job's memory, kept against its target under way; a job past the
    /// limit alone, while the Mac is in trouble, frozen at once (the agent's loop stops it).
    pub fn tick(&self) {
        let watches: Vec<(usize, i32)> = match self.inner.lock() {
            Ok(g) => g.watches.iter().enumerate().filter_map(|(k, w)| w.as_ref().map(|w| (k, w.pgid))).collect(),
            Err(_) => return,
        };
        let set = self.inner.lock().ok().and_then(|g| g.held_set.clone());
        // (Read outside the lock: the footprints and the files take time.)
        let mut got: Vec<(usize, i32, Option<u64>, Option<String>)> = Vec::new();
        let mut cpu: Vec<(usize, i32, Option<u64>)> = Vec::new();
        // (Each sample reads the swap too: its window's least, for `in_trouble`.)
        self.in_trouble();
        for (k, pgid) in watches {
            let held = match &set {
                Some(v) => v.get(k).copied().flatten(),
                None => crate::sys::footprint_of_group(pgid).map(|b| b >> 20),
            };
            let key =
                self.inner.lock().ok().and_then(|g| g.watches.get(k).and_then(|w| w.clone())).and_then(|w| w.current());
            got.push((k, pgid, held, key));
            cpu.push((k, pgid, crate::sys::cpu_of_group(pgid)));
        }
        let live: Option<(PathBuf, Live)>;
        let over: Vec<usize> = {
            let Ok(mut g) = self.inner.lock() else { return };
            for (k, pgid, held, key) in &got {
                // (Only the job read: another started in its slot meanwhile takes none of it.)
                let Some(w) = g.watches.get_mut(*k).and_then(Option::as_mut).filter(|w| w.pgid == *pgid) else {
                    continue;
                };
                w.held = *held;
                if let (Some(mb), Some(key)) = (held, key) {
                    let e = w.seen.entry(key.clone()).or_insert(0);
                    *e = (*e).max(*mb);
                }
            }
            let now = std::time::Instant::now();
            for (k, pgid, ns) in &cpu {
                let Some(w) = g.watches.get_mut(*k).and_then(Option::as_mut).filter(|w| w.pgid == *pgid) else {
                    continue;
                };
                if let Some(ns) = *ns {
                    match w.cpu_mark {
                        Some((_, m)) if ns >= m && ns - m < BUSY_CPU => {}
                        _ => w.cpu_mark = Some((now, ns)),
                    }
                }
            }
            let limit = g.limit_mb;
            // (Written once the lock is let go: the agent's loop doesn't wait on the write.)
            live = g.file.clone().map(|f| {
                (
                    f,
                    Live {
                        at: crate::agent::jobs::now_s(),
                        on: g.on,
                        limit_mb: limit,
                        held_mb: g.watches.iter().map(|w| w.as_ref().and_then(|w| w.held)).collect(),
                        frozen: g.watches.iter().map(|w| w.as_ref().is_some_and(|w| w.frozen)).collect(),
                    },
                )
            });
            if !g.on || limit == 0 {
                Vec::new()
            } else {
                g.watches
                    .iter()
                    .enumerate()
                    .filter_map(|(k, w)| {
                        w.as_ref().filter(|w| !w.frozen && w.held.is_some_and(|m| m > limit)).map(|_| k)
                    })
                    .collect()
            }
        };
        if let Some((f, l)) = live {
            if let Ok(b) = serde_json::to_vec(&l) {
                crate::whole::write(&f, &b).ok();
            }
        }
        self.tick_disk();
        for k in over {
            if !self.in_trouble() {
                continue;
            }
            let Ok(mut g) = self.inner.lock() else { return };
            let (limit, real) = (g.limit_mb, g.held_set.is_none());
            let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut) else { continue };
            if real {
                crate::sys::signal_group(w.pgid, crate::sys::Signal::Stop);
            }
            w.frozen = true;
            let what = format!(
                "froze slot {k}'s job: it held {:.1} GB, past this Mac's limit of {:.1} GB, the Mac short of memory",
                w.held.unwrap_or(0) as f64 / 1024.0,
                limit as f64 / 1024.0
            );
            eprintln!("agent: memory guard {what}");
            g.froze.push((crate::agent::jobs::now_s(), what));
        }
    }
}

impl Sampler {
    /// The disk's part of a sample (docs/pool.md §7.7): each job's disk counted (what its accessors
    /// wrote, `WRITTEN_ENV`'s file; the top of its scratch folder, every `DISK_EVERY`), kept against
    /// its target under way (the disk's fall while it runs alone kept apart, logged, not a floor); and, the disk guard on and the
    /// disk's free space short of the owner's room target, the job growing fastest frozen at once (the agent's
    /// loop makes room from what no job holds, then lets it go on, or gives it back).
    pub fn tick_disk(&self) {
        let (at, free_set, set) = match self.inner.lock() {
            Ok(g) => (g.disk_at.clone(), g.free_set, g.disk_set.clone()),
            Err(_) => return,
        };
        let free = free_set.or_else(|| at.as_ref().and_then(|d| crate::agent::room::disk_free(d).ok()));
        let now = std::time::Instant::now();
        // (Read outside the lock: the files take time.)
        let jobs: Vec<(usize, i32, PathBuf, Option<PathBuf>, bool)> = match self.inner.lock() {
            Ok(g) => g
                .watches
                .iter()
                .enumerate()
                .filter_map(|(k, w)| {
                    w.as_ref().map(|w| {
                        (
                            k,
                            w.pgid,
                            w.written.clone(),
                            w.scratch.clone(),
                            w.looked.is_none_or(|t| now.duration_since(t) >= DISK_EVERY),
                        )
                    })
                })
                .collect(),
            Err(_) => return,
        };
        let mut got: Vec<(usize, i32, u64, Option<u64>, Option<String>)> = Vec::new();
        for (k, pgid, written, scratch, look) in jobs {
            let w = match &set {
                Some(v) => v.get(k).copied().flatten().unwrap_or(0),
                None => store::cachefile::read_written(&written),
            };
            let top = if set.is_some() { Some(0) } else { scratch.filter(|_| look).map(|d| top_bytes(&d)) };
            let key =
                self.inner.lock().ok().and_then(|g| g.watches.get(k).and_then(|w| w.clone())).and_then(|w| w.current());
            got.push((k, pgid, w, top, key));
        }
        let Ok(mut g) = self.inner.lock() else { return };
        g.free = free;
        let n = g.watches.iter().filter(|w| w.is_some()).count();
        for (k, pgid, written, top, key) in got {
            let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut).filter(|w| w.pgid == pgid) else { continue };
            if let Some(t) = top {
                (w.scratch_top, w.looked) = (t, Some(now));
            }
            w.alone &= n <= 1;
            w.disk_now = written + w.scratch_top;
            w.disk_peak = w.disk_peak.max(w.disk_now);
            w.disk_hist.push_back((now, w.disk_now));
            while w.disk_hist.len() > 1 && w.disk_hist.front().is_some_and(|(t, _)| now.duration_since(*t) > DISK_EVERY)
            {
                w.disk_hist.pop_front();
            }
            let Some(key) = key else { continue };
            // (What its accessors had written, and the disk's free space, as the target began: the
            // first as the job did.)
            let (since, free_then) = match &w.disk_key {
                Some((k2, at, f)) if *k2 == key => (*at, *f),
                last => {
                    let (at, f) = if last.is_none() { (0, w.free_start) } else { (written, free) };
                    w.disk_key = Some((key.clone(), at, f));
                    (at, f)
                }
            };
            let fell = match (w.alone, free_then, free) {
                (true, Some(a), Some(b)) => a.saturating_sub(b),
                _ => 0,
            };
            // (The disk's fall kept apart, a weaker signal: other programs on the Mac move it too,
            // and one outlier would hold the target, and its step's prior, to it.)
            w.disk_fell = w.disk_fell.max(fell);
            let took = w.scratch_top + written.saturating_sub(since);
            let e = w.disk_seen.entry(key).or_insert(0);
            *e = (*e).max(took);
        }
        // The disk guard: short of the owner's room target, none frozen for it yet: the job growing
        // fastest (else the largest) frozen at once.
        // (Below the target by `disk_slack` at least: a job let in at the target, its first
        // bytes written, isn't frozen at once; the agent lets one go on at the target again. No
        // target: at a full disk alone, `disk_short_of`.)
        let short = g.disk_on && free.is_some_and(|f| f < disk_short_of(g.floor));
        if !short {
            return;
        }
        // One frozen already, the disk still falling past half the target (the other job filling
        // it meanwhile; no target: the disk full): the others frozen too, until room is made.
        if g.watches.iter().flatten().any(|w| w.disk_frozen) {
            if free.is_some_and(|f| f < (g.floor / 2).max(1)) {
                let real = g.held_set.is_none() && g.disk_set.is_none();
                let (mut said, floor) = (Vec::new(), g.floor);
                for (k, w) in g.watches.iter_mut().enumerate() {
                    let Some(w) = w.as_mut().filter(|w| !w.disk_frozen) else { continue };
                    if real && !w.frozen {
                        crate::sys::signal_group(w.pgid, crate::sys::Signal::Stop);
                    }
                    w.disk_frozen = true;
                    said.push(format!("froze slot {k}'s job too: the disk has {:.1} GB free, under half the {} GB room target, while another job is frozen for it", free.unwrap_or(0) as f64 / (1u64 << 30) as f64, floor >> 30));
                }
                for what in said {
                    eprintln!("agent: disk guard {what}");
                    g.froze.push((crate::agent::jobs::now_s(), what));
                }
            }
            return;
        }
        let Some(k) = g
            .watches
            .iter()
            .enumerate()
            .filter_map(|(k, w)| w.as_ref().map(|w| (k, w.disk_growth(), w.disk_now)))
            .max_by_key(|&(k, grew, now)| (grew, now, std::cmp::Reverse(k)))
            .map(|(k, _, _)| k)
        else {
            return;
        };
        let real = g.held_set.is_none() && g.disk_set.is_none();
        let floor = g.floor;
        let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut) else { return };
        if real && !w.frozen {
            crate::sys::signal_group(w.pgid, crate::sys::Signal::Stop);
        }
        w.disk_frozen = true;
        let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
        let what = format!("froze slot {k}'s job: the disk has {:.1} GB free, short of the {} GB room target, and it grew fastest ({:.1} GB in the last minute, {:.1} GB in all)", gb(free.unwrap_or(0)), floor >> 30, gb(w.disk_growth()), gb(w.disk_now));
        eprintln!("agent: disk guard {what}");
        g.froze.push((crate::agent::jobs::now_s(), what));
    }
}

/// The guard in the status: its switch, this Mac's limit (0: its memory unknown, guarding nothing),
/// what the jobs hold now, and what it last did (when, unix seconds, and in words).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct View {
    pub on: bool,
    pub limit_mb: u64,
    pub held_mb: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_off: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<(u64, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_the_macs_memory_less_an_eighth() {
        assert_eq!(limit_mb(48 << 10), 42 << 10);
        assert_eq!(limit_mb(16 << 10), 12 << 10);
        assert_eq!(limit_mb(64 << 10), 56 << 10);
        assert_eq!(limit_mb(0), 0, "its memory unknown");
    }

    #[test]
    fn decisions_by_measure_alone() {
        let l = 42 << 10;
        assert_eq!(decide(&[Some(30 << 10), Some(12 << 10)], l, &[false, false], &[false, false]), Act::Nothing);
        assert_eq!(decide(&[None, None], l, &[false, false], &[false, false]), Act::Nothing);
        assert_eq!(decide(&[Some(35 << 10), Some(10 << 10)], l, &[false, false], &[false, false]), Act::Drain(1));
        assert_eq!(decide(&[Some(10 << 10), Some(35 << 10)], l, &[false, false], &[false, false]), Act::Drain(0));
        assert_eq!(decide(&[Some(35 << 10), Some(10 << 10)], l, &[false, true], &[false, false]), Act::Nothing);
        assert_eq!(decide(&[Some(43 << 10), Some(1 << 10)], l, &[false, true], &[false, false]), Act::Over(0));
        assert_eq!(decide(&[None, Some(50 << 10)], l, &[false, false], &[false, false]), Act::Over(1));
        assert_eq!(decide(&[Some(25 << 10), Some(25 << 10)], l, &[false, false], &[false, false]), Act::Drain(1));
        // A limit unknown: nothing, however much.
        assert_eq!(decide(&[Some(50 << 10)], 0, &[false], &[false]), Act::Nothing);
        // A job started beside one that waited on a worker yields when that one goes on, the
        // larger or not; while it drains, the other isn't drained for it; past the limit alone,
        // either is stopped as any.
        let y = &[false, true];
        assert_eq!(decide(&[Some(10 << 10), Some(35 << 10)], l, &[false, false], y), Act::Drain(1));
        assert_eq!(decide(&[Some(35 << 10), Some(10 << 10)], l, &[false, false], y), Act::Drain(1));
        assert_eq!(decide(&[Some(10 << 10), Some(35 << 10)], l, &[false, true], y), Act::Nothing);
        assert_eq!(decide(&[Some(30 << 10), Some(10 << 10)], l, &[false, false], y), Act::Nothing);
        assert_eq!(decide(&[Some(43 << 10), Some(1 << 10)], l, &[false, false], y), Act::Over(0));
    }

    /// Several jobs (a member's packed ones, task #240): past the limit together, the largest fitting
    /// alone, the one ranked highest (the newest, then the least progressed) drains, never the
    /// largest; one at a time, and none more while what stays once those draining stop fits.
    #[test]
    fn several_jobs_the_newest_drains_one_at_a_time() {
        let l = 12 << 10;
        let no = &[false; 4];
        // (4 + 3 + 3 + 3 = 13 GB, past 12: the newest, slot 2 (rank 30), not the largest, slot 0.)
        let held = [Some(4 << 10), Some(3 << 10), Some(3 << 10), Some(3 << 10)];
        let rank = [10, 20, 30, 25];
        assert_eq!(decide_by(&held, l, no, no, &rank), Act::Drain(2));
        // (It drains: the 10 GB that stay fit, so nothing more.)
        assert_eq!(decide_by(&held, l, &[false, false, true, false], no, &rank), Act::Nothing);
        // (Grown to 16 GB: what stays past it with slot 2 draining, so the next newest, slot 3.)
        let grown = [Some(7 << 10), Some(3 << 10), Some(3 << 10), Some(3 << 10)];
        assert_eq!(decide_by(&grown, l, &[false, false, true, false], no, &rank), Act::Drain(3));
        // (Every other draining: nothing left to drain, the largest fits alone.)
        assert_eq!(decide_by(&grown, l, &[false, true, true, true], no, &rank), Act::Nothing);
        // (The largest by itself past the limit: it, whoever is newest.)
        let over = [Some(1 << 10), Some(13 << 10), Some(1 << 10), None];
        assert_eq!(decide_by(&over, l, no, no, &rank), Act::Over(1));
        // (A job that yields drains first, ranked low or not.)
        assert_eq!(decide_by(&held, l, no, &[false, true, false, false], &rank), Act::Drain(1));
        // (Equal ranks: the first slot of them.)
        assert_eq!(decide_by(&held, l, no, no, &[0, 5, 5, 5]), Act::Drain(1));
        // (A slot with no job is never drained, however it ranks.)
        let gap = [Some(8 << 10), None, Some(5 << 10), Some(1 << 10)];
        assert_eq!(decide_by(&gap, l, no, no, &[0, 99, 1, 2]), Act::Drain(3));
    }

    /// A slot past those watched (a member's packed jobs) is watched, sampled and let go.
    #[test]
    fn a_sampler_watches_slots_past_its_first() {
        let s = Sampler::new(2);
        s.set_test(Some(vec![Some(1), None, None, None, Some(7)]), Some(false));
        s.watch(4, Watch { pgid: 999_999, ..Default::default() });
        s.tick();
        assert_eq!(s.held(4), Some(7));
        assert!(s.unwatch(4).is_some() && s.held(4).is_none());
    }

    #[test]
    fn trouble_is_the_kernels_pressure_or_swap_grown() {
        // (Floors read as written before they said more: a bare MB.)
        let old: BTreeMap<String, Floor> =
            serde_json::from_str(r#"{"a": 9000, "b": {"mb": 5, "alone": true, "v": 3}}"#).unwrap();
        assert_eq!((old["a"], old["b"]), (Floor { mb: 9000, alone: false, v: 0 }, Floor { mb: 5, alone: true, v: 3 }));
        assert!(!trouble(Some(1), Some(5 << 30), Some(5 << 30)));
        assert!(trouble(Some(2), None, None) && trouble(Some(4), None, None));
        assert!(trouble(Some(1), Some(6 << 30), Some(5 << 30)));
        assert!(!trouble(Some(1), Some((5 << 30) + (512 << 20)), Some(5 << 30)));
        assert!(!trouble(None, None, None));
    }

    #[test]
    fn the_target_under_way() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("costs.jsonl");
        assert_eq!(current(&p), None);
        std::fs::write(&p, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n").unwrap();
        assert_eq!(current(&p).as_deref(), Some("terrain 6/1/1"));
        std::fs::write(
            &p,
            "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n{\"unit\":\"terrain 6/1/1\",\"peak_mb\":900,\"secs\":3}\n",
        )
        .unwrap();
        assert_eq!(current(&p), None);
        std::fs::write(&p, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n{\"unit\":\"terrain 6/1/1\",\"peak_mb\":900,\"secs\":3}\n{\"unit\":\"terrain 6/1/2\",\"started\":4}\nnot json\n").unwrap();
        assert_eq!(current(&p).as_deref(), Some("terrain 6/1/2"));
        // Its fallbacks: between targets, or a step that notes no costs, the first target not
        // noted done; a job of no targets, its own key; else none.
        let done = d.path().join("done.txt");
        let mut w = Watch {
            costs: d.path().join("none.jsonl"),
            done: done.clone(),
            step: "pack".into(),
            targets: vec!["6/1/1".into(), "6/1/2".into()],
            ..Default::default()
        };
        assert_eq!(w.current().as_deref(), Some("pack 6/1/1"));
        std::fs::write(&done, "pack 6/1/1\n").unwrap();
        assert_eq!(w.current().as_deref(), Some("pack 6/1/2"));
        std::fs::write(&done, "pack 6/1/1\npack 6/1/2\n").unwrap();
        assert_eq!(w.current(), None);
        w.targets.clear();
        w.own_key = Some("osm-pass 2026-09-28".into());
        assert_eq!(w.current().as_deref(), Some("osm-pass 2026-09-28"));
        w.own_key = None;
        assert_eq!(w.current(), None);
    }

    #[test]
    fn floors_from_what_a_run_held_but_what_it_measured() {
        let w = Watch {
            step: "terrain".into(),
            targets: vec!["6/1/1".into(), "6/1/2".into()],
            seen: [("terrain 6/1/1".to_string(), 9000), ("terrain 6/1/2".to_string(), 7000)].into(),
            ..Default::default()
        };
        let v = crate::coord::cost_version("terrain");
        assert_eq!(
            w.floors(&["terrain 6/1/2".to_string()]),
            [("terrain 6/1/1".to_string(), Floor { mb: 9000, alone: false, v })]
        );
        let w = Watch {
            step: "water".into(),
            targets: vec!["water".into()],
            seen: [("water water".to_string(), 33000)].into(),
            ..Default::default()
        };
        assert_eq!(w.floors(&[]), [("water water".to_string(), Floor { mb: 33000, alone: true, v: 0 })]);
        assert_eq!((step_of_key("6/1/1"), step_of_key("terrain 6/1/1")), ("unit", "terrain"));
    }

    #[test]
    fn the_sampler_keeps_the_most_held_per_target_and_freezes_only_in_trouble() {
        let d = tempfile::tempdir().unwrap();
        let s = Sampler::new(2);
        s.set(true, 12 << 10);
        s.write_to(d.path().join(LIVE));
        s.watch(
            0,
            Watch {
                pgid: 0,
                costs: d.path().join("c.jsonl"),
                done: d.path().join("d.txt"),
                step: "pack".into(),
                targets: vec!["6/1/1".into()],
                ..Default::default()
            },
        );
        s.set_test(Some(vec![Some(5000), None]), Some(false));
        s.tick();
        s.set_test(Some(vec![Some(3000), None]), Some(false));
        s.tick();
        assert_eq!(s.held(0), Some(3000));
        // Each sample written for the status, whatever the agent's loop waits on.
        let l = live(d.path()).unwrap();
        assert!(
            l.on && l.limit_mb == 12 << 10 && l.held_mb == [Some(3000), None] && l.frozen == [false, false],
            "{l:?}"
        );
        // Past the limit alone, the Mac not in trouble: not frozen.
        s.set_test(Some(vec![Some(13 << 10), None]), Some(false));
        s.tick();
        assert!(!s.frozen(0));
        // In trouble: frozen, said.
        s.set_test(Some(vec![Some(13 << 10), None]), Some(true));
        s.tick();
        assert!(s.frozen(0) && s.take_froze().len() == 1);
        let w = s.unwatch(0).unwrap();
        assert_eq!(w.seen.get("pack 6/1/1"), Some(&(13 << 10)));
        // A new job in the slot (another process group) starts with nothing of the last's.
        s.watch(
            0,
            Watch {
                pgid: 7,
                costs: d.path().join("c2.jsonl"),
                done: d.path().join("d3.txt"),
                step: "pack".into(),
                targets: vec!["6/1/3".into()],
                ..Default::default()
            },
        );
        s.set_test(Some(vec![Some(100), None]), Some(false));
        s.tick();
        let w = s.unwatch(0).unwrap();
        assert!(!w.frozen && w.seen.get("pack 6/1/1").is_none() && w.seen.get("pack 6/1/3") == Some(&100));
        // Off, or the limit unknown: learning only.
        s.watch(
            1,
            Watch {
                pgid: 0,
                step: "pack".into(),
                targets: vec!["6/2/2".into()],
                done: d.path().join("d2.txt"),
                ..Default::default()
            },
        );
        s.set(true, 0);
        s.set_test(Some(vec![None, Some(50 << 10)]), Some(true));
        s.tick();
        assert!(!s.frozen(1) && s.unwatch(1).unwrap().seen.get("pack 6/2/2") == Some(&(50 << 10)));
    }

    /// A job's disk counted by its accessors' bytes and the top of its scratch folder, kept per
    /// target under way as its disk floor (with what its costs file notes), the disk's fall counting
    /// only while it runs alone; the disk guard, short of the room target, freezes the job growing
    /// fastest, once, and lets it go on when told.
    #[test]
    fn a_jobs_disk_is_counted_per_target_and_the_fastest_frozen_short_of_room() {
        let d = tempfile::tempdir().unwrap();
        let g: u64 = 1 << 30;
        let s = Sampler::new(2);
        s.set_disk(true, 30 * g, d.path());
        s.set_test(Some(vec![None, None]), Some(false));
        let costs = d.path().join("c.jsonl");
        std::fs::write(&costs, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n").unwrap();
        s.set_test_disk(Some(100 * g), None);
        s.watch(
            0,
            Watch {
                pgid: 0,
                costs: costs.clone(),
                done: d.path().join("d.txt"),
                step: "terrain".into(),
                targets: vec!["6/1/1".into(), "6/1/2".into()],
                ..Default::default()
            },
        );
        // Alone: its accessors wrote 2 GB, the disk fell 5 GB (outside programs'): 5 GB its target's.
        s.set_test_disk(Some(95 * g), Some(vec![Some(2 * g), None]));
        s.tick();
        // The next target: what its accessors wrote since it began.
        std::fs::write(&costs, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n{\"unit\":\"terrain 6/1/1\",\"peak_mb\":900,\"secs\":3,\"disk_mb\":6144}\n{\"unit\":\"terrain 6/1/2\",\"started\":4}\n").unwrap();
        s.set_test_disk(Some(95 * g), Some(vec![Some(2 * g), None]));
        s.tick();
        // A second job beside: the fall no longer counts for either.
        s.watch(
            1,
            Watch {
                pgid: 1,
                costs: d.path().join("c2.jsonl"),
                done: d.path().join("d2.txt"),
                step: "pack".into(),
                targets: vec!["6/2/2".into()],
                ..Default::default()
            },
        );
        s.set_test_disk(Some(60 * g), Some(vec![Some(3 * g), Some(g / 2)]));
        s.tick();
        assert_eq!(s.disk_now(0), Some(3 * g));
        assert!(!s.disk_frozen(0) && !s.disk_frozen(1));
        // Short of the target by less than its slack (a job just let in at it): none frozen.
        s.set_test_disk(Some(30 * g - disk_slack(30 * g) / 2), Some(vec![Some(3 * g), Some(g / 2)]));
        s.tick();
        assert!(!s.disk_frozen(0) && !s.disk_frozen(1));
        // Short of the target: the job growing fastest frozen (slot 0, 1 GB against half a GB).
        s.set_test_disk(Some(20 * g), Some(vec![Some(4 * g), Some(g)]));
        s.tick();
        assert!(s.disk_frozen(0) && !s.disk_frozen(1));
        assert!(s.take_froze().iter().any(|(_, w)| w.contains("grew fastest")));
        // Once: none other frozen while it is, while the disk keeps half the target.
        s.set_test_disk(Some(18 * g), Some(vec![Some(4 * g), Some(2 * g)]));
        s.tick();
        assert!(!s.disk_frozen(1));
        // The other filling it meanwhile, past half the target: frozen too, said.
        s.set_test_disk(Some(10 * g), Some(vec![Some(4 * g), Some(3 * g)]));
        s.tick();
        assert!(s.disk_frozen(1) && s.take_froze().iter().any(|(_, w)| w.contains("too")));
        assert!(s.thaw_disk(1));
        assert!(s.thaw_disk(0) && !s.disk_frozen(0));
        let w = s.unwatch(0).unwrap();
        let floors: BTreeMap<String, u64> = w.disk_floors().into_iter().collect();
        assert_eq!(floors.get("terrain 6/1/1"), Some(&6144), "its costs file's, past the 5 GB fall: {floors:?}");
        assert_eq!(floors.get("terrain 6/1/2"), Some(&2048), "what its accessors wrote since it began: {floors:?}");
        // The guard off: nothing frozen, the counts learned all the same.
        s.set_disk(false, 30 * g, d.path());
        s.set_test_disk(Some(g), Some(vec![None, Some(5 * g)]));
        s.tick();
        assert!(!s.disk_frozen(1) && s.unwatch(1).unwrap().disk_floors() == [("pack 6/2/2".to_string(), 5120)]);
        // The allocated bytes at the top of a folder, not its subfolders'.
        std::fs::write(d.path().join("top.bin"), vec![1u8; 1 << 20]).unwrap();
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/deep.bin"), vec![1u8; 4 << 20]).unwrap();
        let top = top_bytes(d.path());
        assert!(top >= 1 << 20 && top < 4 << 20, "{top}");
    }

    /// The disk guard's threshold is the owner's room target alone (task #421): with a 20 GB
    /// target it fires short of 20 GB (less its slack), whatever else is free; with none (0) only
    /// at a full disk, no hidden floor.
    #[test]
    fn the_disk_guard_fires_at_the_room_target_and_with_none_at_a_full_disk() {
        let d = tempfile::tempdir().unwrap();
        let g: u64 = 1 << 30;
        assert_eq!((disk_short_of(30 * g), disk_short_of(0)), (29 * g, 1));
        let watch = |s: &Sampler| {
            s.set_test(Some(vec![None]), Some(false));
            s.set_test_disk(Some(100 * g), None);
            s.watch(0, Watch { pgid: 0, costs: d.path().join("c.jsonl"), step: "pack".into(), ..Default::default() });
        };
        let s = Sampler::new(1);
        s.set_disk(true, 20 * g, d.path());
        watch(&s);
        s.set_test_disk(Some(20 * g), Some(vec![Some(g)]));
        s.tick();
        assert!(!s.disk_frozen(0), "at the target: not frozen");
        s.set_test_disk(Some(19 * g), Some(vec![Some(2 * g)]));
        s.tick();
        assert!(s.disk_frozen(0), "short of the target past its slack: frozen");
        assert!(s.take_froze().iter().any(|(_, w)| w.contains("20 GB room target")));
        let s = Sampler::new(1);
        s.set_disk(true, 0, d.path());
        watch(&s);
        s.set_test_disk(Some(g / 4), Some(vec![Some(g)]));
        s.tick();
        assert!(!s.disk_frozen(0), "no target, a byte free: not frozen");
        s.set_test_disk(Some(0), Some(vec![Some(2 * g)]));
        s.tick();
        assert!(s.disk_frozen(0), "no target, the disk full: frozen");
    }

    #[test]
    fn the_switch() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(on(d.path()), Some(DEFAULT));
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        std::fs::write(d.path().join(SWITCH), "off\n").unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::write(d.path().join(SWITCH), "on").unwrap();
        assert_eq!(on(d.path()), Some(true));
    }
}
