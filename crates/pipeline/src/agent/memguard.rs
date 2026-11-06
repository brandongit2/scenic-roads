//! The memory guard (docs/pool.md §7.1–7.2): macOS swaps rather than kills, so a Mac short of
//! memory swaps for as long as it stays short, every job and the owner's own work with it. The
//! guard keeps the Mac's memory floor (crate::agent::memfloor) available, whatever takes the rest.
//!
//! - **Sampled on a thread of its own** (`Sampler`, every `EVERY`, whatever the agent's loop is
//!   waiting on: a share that stalls stalls the loop, not this): each running job's processes'
//!   physical footprints summed (crate::sys::footprint_of_group), kept per target as the most the
//!   job held while that target was under way (`Watch::current`); and with them the Mac's memory
//!   (`Reading`): what is available (crate::sys::memory_available: what macOS hands out at no
//!   cost, and what it frees by compressing idle memory with its pressure left normal), the swap
//!   grown lately, and the kernel's pressure level.
//! - **Learned:** what a job held while a target was under way is that target's
//!   **floor** (`Floor`: crate::coord's floors), the same kind of figure as a run's own measure (a
//!   job's processes together, sampled), which takes its place.
//! - **Guarded:** while the Mac is below its floor (`Reading::below`: its available memory, the
//!   swap grown lately off, under the floor, or the brake on), jobs are given back by measure
//!   (`decide_by`, against what the jobs may hold before the Mac is below: `Reading::room_mb`):
//!   when the largest fits alone, another stops at its next safe point (the newest, then the least
//!   progressed, one at a time while those left still pass); the largest, past it by itself, stops
//!   at once only while the brake is on (`Reading::brake`: the kernel's memory pressure at warning
//!   or worse, or swap grown past what was available above the floor; the thread freezes it then,
//!   at once, the agent's loop stopping it), else at its next safe point too. A Mac whose memory
//!   can't be read guards nothing.
//! - **Cooled down:** a job given back holds new work off the Mac (`Reading::cooling`) until what's
//!   available is back past the floor and that job's peak, or `COOL_WAIT` has passed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the jobs' memory is sampled.
pub const EVERY: Duration = Duration::from_secs(5);

/// How often the top of each job's scratch folder is looked at for the disk it takes (docs/pool.md
/// §7.7: the allocated sizes of the files there, no walk of its tree), and the window a job's disk
/// growth is judged over.
pub const DISK_EVERY: Duration = Duration::from_secs(60);

/// The window the swap grown is judged over (`Reading`). Recent, so the owner's own use growing
/// swap hours ago doesn't leave the Mac below its floor for good.
pub const SWAP_WINDOW: Duration = Duration::from_secs(300);

/// The longest a give-back's cool-down holds new work off the Mac (`Reading::cooling`): the swap
/// window, so the swap a give-back's shortage grew has left the reading new work is let in by. It
/// outlasts most outside swings: on the M4's live series (the 10 s ring, the 7.8 h of 4 Nov since
/// the available was recorded), other processes' rises of 2 GB or more fell back within 60 s at
/// the median and 190 s at the 90th percentile (docs/pool.md §7.1).
pub const COOL_WAIT: Duration = SWAP_WINDOW;

/// The Mac's memory as one sample read it (MB), against its floor (crate::agent::memfloor), with
/// the jobs' memory sampled beside it: what the guard gives back by, and the asks' room
/// (docs/pool.md §7.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Reading {
    /// The memory available (crate::sys::memory_available), and the jobs' memory together.
    pub avail_mb: u64,
    pub held_mb: u64,
    /// Of `avail_mb`, what macOS frees by compressing idle memory with its pressure left normal
    /// (crate::sys::Available's `compressible`).
    pub compress_mb: u64,
    /// The floor in force.
    pub floor_mb: u64,
    /// The swap grown within `SWAP_WINDOW`: its use now less the least in the window.
    pub swap_grown_mb: u64,
    /// The kernel's memory pressure level (1 normal, 2 warning, 4 critical; 0 unknown).
    pub pressure: u32,
    /// A give-back's cool-down (crate::agent's `mem_cool`): the memory available it waits for (MB:
    /// the floor and the given-back job's peak) and when it ends at the latest (unix s); 0, none.
    pub cool_mb: u64,
    pub cool_until: u64,
}

impl Reading {
    /// The memory available less the swap grown lately (MB): what was pushed to swap in the window
    /// is memory the Mac didn't have.
    pub fn usable_mb(&self) -> u64 {
        self.avail_mb.saturating_sub(self.swap_grown_mb)
    }

    /// The emergency brake: the kernel's pressure at warning or critical, or swap grown past what
    /// was available above the floor (`usable_mb` below it for the swap). While it's on no job
    /// starts, whatever the number says; it freezes jobs only with evidence (`acts`). Why, in
    /// words; None when it's off.
    pub fn brake(&self) -> Option<String> {
        let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
        match self.pressure {
            p if p >= 4 => return Some("macOS's memory pressure is critical".into()),
            p if p >= 2 => return Some("macOS's memory pressure is at warning".into()),
            _ => {}
        }
        (self.swap_grown_mb > 0 && self.usable_mb() < self.floor_mb).then(|| {
            format!(
                "swap grew {} in the last {} min, past the {} available above the floor",
                gb(self.swap_grown_mb),
                SWAP_WINDOW.as_secs() / 60,
                gb(self.avail_mb.saturating_sub(self.floor_mb))
            )
        })
    }

    /// Whether the brake acts on the running jobs (freezes one at once, gives it back if the Mac
    /// isn't back: the agent's `pressure`): the kernel's pressure critical, or the brake on with
    /// the Mac below its floor by the numbers (its usable memory under it: the swap grown among
    /// them). The pressure at warning alone only holds new work: macOS sits at warning for long
    /// stretches while it compresses, and a job frozen for it, then given back each minute, would
    /// lose work the Mac had room for.
    pub fn acts(&self) -> bool {
        self.pressure >= 4 || (self.brake().is_some() && self.usable_mb() < self.floor_mb)
    }

    /// Whether the Mac is below its floor: its usable memory under it, or the brake acting.
    pub fn below(&self) -> bool {
        self.usable_mb() < self.floor_mb || self.acts()
    }

    /// What the jobs may hold together before the Mac is below its floor (MB): what they hold and
    /// what's available, less the swap grown lately and the floor (the swap off the sum, not off
    /// what's available alone: what's available stops at nothing, the swap goes on growing); the
    /// guard gives back past it (`decide_by`). The brake acting (`acts`), a megabyte less than they
    /// hold: below the floor whatever the number says.
    pub fn room_mb(&self) -> u64 {
        let room = (self.avail_mb + self.held_mb).saturating_sub(self.swap_grown_mb + self.floor_mb);
        if self.acts() {
            room.min(self.held_mb.saturating_sub(1))
        } else {
            room
        }
    }

    /// What the jobs may hold together for a job to start (MB): `room_mb`, 0 while no job may start
    /// whatever it needs (`held_back`: the brake, the Mac under its floor, a give-back's
    /// cool-down). A job starts where this, less every running job's expected peak (or what it
    /// holds, the larger), fits its own expected peak: available − the running jobs' growth to
    /// come − its peak ≥ the floor (crate::coord::sched's `pack`). The floor is the only figure:
    /// the jobs' growth is reserved before a new job, and nothing past it.
    pub fn admit_mb(&self) -> u64 {
        if self.held_back().is_some() {
            return 0;
        }
        self.room_mb()
    }

    /// Whether a give-back's cool-down holds new work off the Mac: one began (`cool_mb`), what's
    /// available (the swap grown off) still short of the floor and the given-back job's peak, and
    /// `cool_until` not passed. A job given back because the Mac fell below its floor isn't
    /// followed at once by a smaller one the same fall would give back too.
    pub fn cooling(&self) -> bool {
        self.cool_mb > 0 && self.usable_mb() < self.cool_mb && crate::agent::jobs::now_s() < self.cool_until
    }

    /// Why the Mac is below its floor, in words: "the memory brake is on (…)", else "this Mac has
    /// 4.1 GB available (swap grew 1.0 GB), below its 6.0 GB floor".
    pub fn short_words(&self) -> String {
        let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
        match self.brake() {
            Some(b) => format!("the memory brake is on ({b})"),
            None => format!(
                "this Mac has {} available{}, below its {} floor",
                gb(self.usable_mb()),
                if self.swap_grown_mb > 0 { format!(" (swap grew {})", gb(self.swap_grown_mb)) } else { String::new() },
                gb(self.floor_mb)
            ),
        }
    }

    /// Why no job starts here whatever it needs, in words: the brake, the usable memory under the
    /// floor, or a give-back's cool-down (`cooling`); None when one may.
    pub fn held_back(&self) -> Option<String> {
        let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
        if let Some(b) = self.brake() {
            return Some(format!("the memory brake: {b}"));
        }
        if self.usable_mb() < self.floor_mb {
            return Some(format!(
                "{} of memory available{}, under the {} floor",
                gb(self.usable_mb()),
                if self.swap_grown_mb > 0 { format!(" (swap grew {})", gb(self.swap_grown_mb)) } else { String::new() },
                gb(self.floor_mb)
            ));
        }
        self.cooling().then(|| {
            let left = self.cool_until.saturating_sub(crate::agent::jobs::now_s());
            format!(
                "a job was given back: no new job until {} is available (the {} floor and its {} peak; {} now), or for {} min more",
                gb(self.cool_mb),
                gb(self.floor_mb),
                gb(self.cool_mb.saturating_sub(self.floor_mb)),
                gb(self.usable_mb()),
                left.div_ceil(60)
            )
        })
    }
}

/// What the guard does about the jobs in a Mac's slots, by what each holds now (MB; None: no job in
/// that slot, or its memory unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Nothing: the jobs fit the room together (the Mac at or above its floor).
    Nothing,
    /// Slot k's job stops at its next safe point: the job beside it fits the room alone.
    Drain(usize),
    /// Slot k's job holds more than the room by itself.
    Over(usize),
}

/// The guard's decision (`Act`), by measure alone, for any number of jobs: nothing while the jobs
/// together fit `room` (what they may hold before the Mac is below its floor, `Reading::room_mb`);
/// else the largest when it alone passes it; else, it fitting alone, one other drained at a time:
/// the one ranked highest (`rank`: the agent ranks the newest job highest, then the least
/// progressed: the work lost least), not draining yet, and only while the jobs not draining (what's
/// left once those draining reach their safe points) still pass the room. A job that `yields`
/// (started beside one that waited on a worker, which then went on) is the one drained, larger or
/// not, and while it drains no other is: the job it was started beside isn't stopped for it.
pub fn decide_by(held: &[Option<u64>], room: u64, draining: &[bool], yields: &[bool], rank: &[u64]) -> Act {
    let total: u64 = held.iter().flatten().sum();
    if total <= room {
        return Act::Nothing;
    }
    let Some((big, mb)) =
        held.iter().enumerate().filter_map(|(k, m)| m.map(|m| (k, m))).max_by_key(|&(k, m)| (m, std::cmp::Reverse(k)))
    else {
        return Act::Nothing;
    };
    if mb > room {
        return Act::Over(big);
    }
    let yielding = |k: usize| yields.get(k).copied().unwrap_or(false) && held[k].is_some();
    if let Some(k) = (0..held.len()).find(|&k| yielding(k)) {
        return if draining.get(k).copied().unwrap_or(false) { Act::Nothing } else { Act::Drain(k) };
    }
    let drains = |k: usize| draining.get(k).copied().unwrap_or(false);
    // (What stays once the jobs draining stop: within the room, nothing more drains.)
    let staying: u64 = held.iter().enumerate().filter(|&(k, _)| !drains(k)).filter_map(|(_, m)| *m).sum();
    if staying <= room {
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

/// The target a job is on, as its costs file says (`SCENIC_COSTS`: a `started` line as each target
/// begins, a `peak_mb` line as it ends): the last begun and not ended, by its cost key
/// (crate::coord::cost_key); None when the file says none (a step that notes no costs, or between
/// targets).
pub fn current(costs: &Path) -> Option<String> {
    targets(costs).on.map(|o| o.0)
}

/// A job's targets as its costs file says (`current`): the one under way and when it began (unix
/// s), and how many ended (each a `peak_mb` line after the `started` line of its target: the
/// step's own per-target completion, the one notion of a target done for the batch's percent, its
/// time left and the flags' count); and the threads the last to begin runs on, where its `started`
/// line says (a target run on its job's grant as it began, crate::control::on_grant: docs/pool.md
/// §7.1, Threads re-granted).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Targets {
    pub on: Option<(String, u64)>,
    pub ended: usize,
    pub threads: Option<u32>,
}

/// Costs file `costs`'s `Targets` (none when it can't be read). A target's `peak_mb` line ends it
/// whichever target is under way: a step that lands a target's uploads beside the next target's
/// work (scenic-build's `Landing`: terrain-hi, slope-hi, pack, bldtiles, terrain-deep) writes its
/// line after the next's `started` one, and counted only against the target under way it never
/// ended any (a batch of 13 tiles read 0 done with 9 made, 5 Nov).
pub fn targets(costs: &Path) -> Targets {
    let mut t = Targets::default();
    let Ok(s) = std::fs::read_to_string(costs) else { return t };
    let mut begun: Vec<String> = Vec::new();
    for l in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let Some(key) = v["unit"].as_str() else { continue };
        if let Some(at) = v.get("started") {
            t.on = Some((key.to_string(), at.as_u64().unwrap_or(0)));
            begun.push(key.to_string());
            // (A drop's line, run outside its grant, says none: the last said stands.)
            t.threads = v["threads"].as_u64().map(|n| n as u32).or(t.threads);
        } else if v.get("peak_mb").is_some() {
            if let Some(i) = begun.iter().position(|b| b == key) {
                begun.swap_remove(i);
                t.ended += 1;
                if t.on.as_ref().is_some_and(|o| o.0 == key) {
                    t.on = None;
                }
            }
        }
    }
    t
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

/// The step of a cost key (crate::coord::CostKey): a z6 tile's is its target alone.
pub fn step_of_key(key: &str) -> &str {
    crate::coord::CostKey::parse(key).step
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
    /// Whether the guard froze it (the brake on with it past the jobs' room alone: the agent's loop
    /// stops it).
    pub frozen: bool,
    /// Where its accessors keep the bytes they've added to the caches (store::cachefile's
    /// `WRITTEN_ENV`), and its scratch folder (its top looked at every `DISK_EVERY`).
    pub written: PathBuf,
    pub scratch: Option<PathBuf>,
    /// The folder its processes count their NAS traffic in (store::nasio::ENV), for the live store.
    pub nas_counts: PathBuf,
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
    /// Its CPU time (ns) as last sampled: what its threads used, for its re-grant (crate::agent's
    /// `tend_grant`).
    pub cpu_ns: Option<u64>,
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
    /// The floor and the Mac's memory (MB; 0 unknown: the guard then guards nothing), as the
    /// agent's loop last set them.
    floor_mb: u64,
    total_mb: u64,
    /// As a test sets them: what each slot's job holds (MB), and whether the Mac is in trouble (the
    /// kernel's pressure at warning, else normal).
    held_set: Option<Vec<Option<u64>>>,
    trouble_set: Option<bool>,
    /// As a test sets them: the memory available and the swap in use (MB); or what a test's Mac has
    /// available with no job running, less what its jobs hold (the agent's `test_idle_mb`).
    avail_set: Option<u64>,
    swap_set: Option<u64>,
    idle_set: Option<u64>,
    /// As a test sets it: when each slot's job was last busy (`Watch::cpu_mark`).
    busy_set: Option<Vec<Option<std::time::Instant>>>,
    /// What the thread did last (unix seconds, in words), for the agent's loop to say.
    froze: Vec<(u64, String)>,
    /// The swap in use (MB) as each sample read it over the last `SWAP_WINDOW`; and the latest
    /// sample's `Reading` (None: the Mac's memory unknown).
    swap: std::collections::VecDeque<(std::time::Instant, u64)>,
    reading: Option<Reading>,
    /// A give-back's cool-down as the agent's loop last set it (`Reading::cool_mb`, `cool_until`).
    cool: (u64, u64),
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
/// (unix seconds), whether it guards (the Mac's memory known), the Mac's memory against its floor
/// (`Reading`), what each slot's job holds (MB) and whether the thread froze it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Live {
    pub at: u64,
    pub on: bool,
    pub reading: Option<Reading>,
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
/// `tick` itself), freezing one past the jobs' room alone while the brake is on.
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

    /// This Mac's floor (crate::agent::memfloor) and memory (MB; 0: unknown, the guard guards
    /// nothing), as the agent's loop read them.
    pub fn set(&self, floor_mb: u64, total_mb: u64) {
        if let Ok(mut g) = self.inner.lock() {
            (g.floor_mb, g.total_mb) = (floor_mb, total_mb);
        }
    }

    /// A give-back's cool-down (`Reading::cool_mb`, `cool_until`; (0, 0) none), as the agent's loop
    /// keeps it, for each sample's reading.
    pub fn set_cool(&self, cool_mb: u64, until: u64) {
        if let Ok(mut g) = self.inner.lock() {
            g.cool = (cool_mb, until);
        }
    }

    /// A test's figures: what each slot's job holds (MB), and whether the Mac is in trouble (the
    /// kernel's pressure at warning; not: normal).
    pub fn set_test(&self, held: Option<Vec<Option<u64>>>, trouble: Option<bool>) {
        if let Ok(mut g) = self.inner.lock() {
            (g.held_set, g.trouble_set) = (held, trouble);
        }
    }

    /// A test's memory: what's available and the swap in use (MB).
    pub fn set_test_mem(&self, avail_mb: Option<u64>, swap_mb: Option<u64>) {
        if let Ok(mut g) = self.inner.lock() {
            (g.avail_set, g.swap_set) = (avail_mb, swap_mb);
        }
    }

    /// A test's Mac: what it has available with no job running (MB), less what its jobs hold.
    pub fn set_test_idle(&self, idle_mb: Option<u64>) {
        if let Ok(mut g) = self.inner.lock() {
            g.idle_set = idle_mb;
        }
    }

    /// The Mac's memory against its floor, as the last sample read it (None: its memory unknown,
    /// or not sampled yet).
    pub fn reading(&self) -> Option<Reading> {
        self.inner.lock().ok()?.reading
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

    /// Slot `k`'s job's CPU time as last sampled (ns, `Watch::cpu_ns`); None: not sampled yet, or
    /// unreadable.
    pub fn cpu_ns(&self, k: usize) -> Option<u64> {
        self.inner.lock().ok()?.watches.get(k)?.as_ref()?.cpu_ns
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

    /// The jobs watched now: each one's slot, process group and the folder its NAS counts are in
    /// (the live store's, crate::agent::live).
    pub fn jobs(&self) -> Vec<(usize, i32, PathBuf)> {
        let Ok(g) = self.inner.lock() else { return Vec::new() };
        g.watches
            .iter()
            .enumerate()
            .filter_map(|(k, w)| w.as_ref().map(|w| (k, w.pgid, w.nas_counts.clone())))
            .collect()
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
    /// freezes one past the room alone: the agent's loop, the brake on with the jobs
    /// together past the room. True when it froze it now.
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

    /// Whether the brake acts now (`Reading::acts`), as the last sample read it.
    pub fn acting(&self) -> bool {
        self.inner.lock().ok().and_then(|g| g.reading).is_some_and(|r| r.acts())
    }

    /// Whether the Mac is in trouble now: the memory brake on (`Reading::brake`), as the last sample
    /// read it (a test's figure, as it set it: its pressure at warning, a test Mac's reading's).
    pub fn in_trouble(&self) -> bool {
        let Ok(g) = self.inner.lock() else { return false };
        match g.trouble_set {
            Some(t) => t,
            None => g.reading.is_some_and(|r| r.brake().is_some()),
        }
    }

    /// The Mac's memory now against its floor, the jobs holding `held_mb` together (`Reading`): the
    /// memory available (what compressing frees among it), the swap's samples kept over
    /// `SWAP_WINDOW` for the swap grown. None (and nothing kept) while its memory or the available
    /// can't be read.
    fn read_memory(g: &mut Inner, held_mb: u64) -> Option<Reading> {
        if g.total_mb == 0 {
            return None;
        }
        let set = g.avail_set.or(g.idle_set.map(|i| i.saturating_sub(held_mb)));
        let (avail_mb, compress_mb) = match set {
            Some(mb) => (mb, 0),
            None => crate::sys::memory_available().map(|a| (a.total() >> 20, a.compressible >> 20))?,
        };
        let now = std::time::Instant::now();
        let swap = g.swap_set.or_else(|| crate::sys::swap_used().map(|b| b >> 20));
        if let Some(n) = swap {
            g.swap.push_back((now, n));
        }
        while g.swap.front().is_some_and(|(t, _)| now.duration_since(*t) > SWAP_WINDOW) {
            g.swap.pop_front();
        }
        let low = g.swap.iter().map(|(_, n)| *n).min();
        let pressure = match g.trouble_set {
            Some(true) => 2,
            Some(false) => 1,
            None => crate::sys::memory_pressure().unwrap_or(0),
        };
        Some(Reading {
            avail_mb,
            held_mb,
            compress_mb,
            floor_mb: g.floor_mb,
            swap_grown_mb: match (swap, low) {
                (Some(n), Some(l)) => n.saturating_sub(l),
                _ => 0,
            },
            pressure,
            cool_mb: g.cool.0,
            cool_until: g.cool.1,
        })
    }

    /// What the thread did since the last call, for the agent's loop to note.
    pub fn take_froze(&self) -> Vec<(u64, String)> {
        self.inner.lock().map(|mut g| std::mem::take(&mut g.froze)).unwrap_or_default()
    }

    /// One sample: each watched job's memory, kept against its target under way, and the Mac's
    /// against its floor (`reading`); a job past the jobs' room alone (`Reading::room_mb`), while
    /// the brake is on, frozen at once (the agent's loop stops it).
    pub fn tick(&self) {
        let watches: Vec<(usize, i32)> = match self.inner.lock() {
            Ok(g) => g.watches.iter().enumerate().filter_map(|(k, w)| w.as_ref().map(|w| (k, w.pgid))).collect(),
            Err(_) => return,
        };
        let set = self.inner.lock().ok().and_then(|g| g.held_set.clone());
        // (Read outside the lock: the footprints and the files take time.)
        let mut got: Vec<(usize, i32, Option<u64>, Option<String>)> = Vec::new();
        let mut cpu: Vec<(usize, i32, Option<u64>)> = Vec::new();
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
                w.cpu_ns = *ns;
                if let Some(ns) = *ns {
                    match w.cpu_mark {
                        Some((_, m)) if ns >= m && ns - m < BUSY_CPU => {}
                        _ => w.cpu_mark = Some((now, ns)),
                    }
                }
            }
            let held_mb: u64 = g.watches.iter().flatten().filter_map(|w| w.held).sum();
            let reading = Self::read_memory(&mut g, held_mb);
            g.reading = reading;
            // (Written once the lock is let go: the agent's loop doesn't wait on the write.)
            live = g.file.clone().map(|f| {
                (
                    f,
                    Live {
                        at: crate::agent::jobs::now_s(),
                        on: reading.is_some(),
                        reading,
                        held_mb: g.watches.iter().map(|w| w.as_ref().and_then(|w| w.held)).collect(),
                        frozen: g.watches.iter().map(|w| w.as_ref().is_some_and(|w| w.frozen)).collect(),
                    },
                )
            });
            match reading {
                Some(r) => {
                    let room = r.room_mb();
                    g.watches
                        .iter()
                        .enumerate()
                        .filter_map(|(k, w)| {
                            w.as_ref().filter(|w| !w.frozen && w.held.is_some_and(|m| m > room)).map(|_| k)
                        })
                        .collect()
                }
                None => Vec::new(),
            }
        };
        if let Some((f, l)) = live {
            if let Ok(b) = serde_json::to_vec(&l) {
                store::whole::write(&f, &b).ok();
            }
        }
        self.tick_disk();
        for k in over {
            if !self.acting() {
                continue;
            }
            let Ok(mut g) = self.inner.lock() else { return };
            let (reading, real) = (g.reading.unwrap_or_default(), g.held_set.is_none());
            let Some(w) = g.watches.get_mut(k).and_then(Option::as_mut) else { continue };
            if real {
                crate::sys::signal_group(w.pgid, crate::sys::Signal::Stop);
            }
            w.frozen = true;
            let what = format!(
                "froze slot {k}'s job: it held {:.1} GB with {:.1} GB available against this Mac's {:.1} GB floor, the memory brake on ({})",
                w.held.unwrap_or(0) as f64 / 1024.0,
                reading.avail_mb as f64 / 1024.0,
                reading.floor_mb as f64 / 1024.0,
                reading.brake().unwrap_or_default()
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

/// The guard in the status (the build page, the menu bar, `scenic status`): whether it guards (the
/// Mac's memory known; else why not), the Mac's memory against its floor (`Reading`: available,
/// what compressing frees among it, the floor, the swap grown, the pressure), whether the floor is the owner's or the
/// default, what the jobs hold and what's reserved for their growth to come (each running job's
/// expected peak less what it holds), why no job starts here now (`Reading::held_back`: the brake,
/// the floor, a give-back's cool-down), what it last did (when, unix seconds, and in words: a job given back and why), and
/// all that in a line (`words`) the menu bar prints as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct View {
    pub on: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading: Option<Reading>,
    pub floor_set: bool,
    pub held_mb: u64,
    pub reserved_mb: u64,
    /// What a new job's expected peak may be here now (MB): `Reading::admit_mb` less each running
    /// job's expected peak or what it holds, the larger (crate::coord::Capacity's `mem_mb`); None
    /// where the running jobs' peaks aren't known (a sample alone, `Live::words`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brake: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_back: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why_off: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<(u64, String)>,
    pub said: String,
}

impl Live {
    /// The sample in a line (`words`, from the sample alone: nothing reserved said), a job frozen
    /// said: "Memory: 9.3 GB available, floor 6.0 GB (default); the jobs hold 12.1 GB; a job
    /// frozen". `floor_set`: whether the floor is the owner's.
    pub fn words(&self, floor_set: bool) -> String {
        let r = self.reading;
        let v = View {
            on: self.on,
            reading: r,
            floor_set,
            held_mb: self.held_mb.iter().flatten().sum(),
            brake: r.and_then(|r| r.brake()),
            held_back: r.and_then(|r| r.held_back()),
            why_off: (!self.on).then(|| "this Mac's memory can't be read".into()),
            ..Default::default()
        };
        let frozen = self.frozen.iter().filter(|f| **f).count();
        match frozen {
            0 => words(&v),
            1 => format!("{}; a job frozen", words(&v)),
            n => format!("{}; {n} jobs frozen", words(&v)),
        }
    }
}

/// The memory in a line: "Memory: 9.3 GB available (3.4 GB by compressing), floor 6.0 GB
/// (default); the jobs hold 12.1 GB, 3.0 GB more reserved for their growth" and, when no job
/// starts here, why
/// ("; no new job: …"); the brake first when it's on ("Memory brake: …"). Unknown memory: why.
pub fn words(v: &View) -> String {
    let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
    let Some(r) = v.reading.filter(|_| v.on) else {
        return format!("Memory: not guarded ({})", v.why_off.as_deref().unwrap_or("not sampled yet"));
    };
    let mut s = format!(
        "Memory: {} available{}, floor {}{}",
        gb(r.avail_mb),
        if r.compress_mb > 0 { format!(" ({} by compressing)", gb(r.compress_mb)) } else { String::new() },
        gb(r.floor_mb),
        if v.floor_set { "" } else { " (default)" }
    );
    if v.held_mb > 0 || v.reserved_mb > 0 {
        s += &format!("; the jobs hold {}", gb(v.held_mb));
        if v.reserved_mb > 0 {
            s += &format!(", {} more reserved for their growth", gb(v.reserved_mb));
        }
    }
    match (&v.brake, &v.held_back) {
        (Some(b), _) => format!("Memory brake: {b}. {s}"),
        (None, Some(h)) => format!("{s}; no new job: {h}"),
        (None, None) => match v.new_mb {
            Some(n) => format!("{s}; {} for a new job", gb(n)),
            None => s,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `decide_by` with each slot's rank its order: of two, the one beside the largest drains.
    fn decide(held: &[Option<u64>], room: u64, draining: &[bool], yields: &[bool]) -> Act {
        let rank: Vec<u64> = (0..held.len()).map(|k| u64::MAX - k as u64).collect();
        decide_by(held, room, draining, yields, &rank)
    }

    const GB: u64 = 1 << 10;

    /// A reading of the M4 (48 GB): `avail` GB available, its jobs holding `held` GB, a 6 GB floor.
    fn m4(avail: u64, held: u64) -> Reading {
        Reading { avail_mb: avail * GB, held_mb: held * GB, floor_mb: 6 * GB, pressure: 1, ..Default::default() }
    }

    /// The jobs' room is what they hold and what's available, less the floor: below the floor, the
    /// jobs past it. New work by the floor alone (task #945: no margin): a job starts once its peak
    /// and the running jobs' growth fit above it, never under the brake.
    #[test]
    fn the_room_is_the_available_and_the_held_less_the_floor() {
        let r = m4(10, 20);
        assert_eq!((r.room_mb(), r.admit_mb(), r.below(), r.brake()), (24 * GB, 24 * GB, false, None));
        assert_eq!(r.held_back(), None);
        // (Other apps grew: 4 GB available, below the 6 GB floor: the jobs past their room by 2 GB,
        // and no new job.)
        let r = m4(4, 20);
        assert!(r.below() && r.room_mb() == 18 * GB && r.admit_mb() == 0);
        assert_eq!(r.held_back().as_deref(), Some("4.0 GB of memory available, under the 6.0 GB floor"));
        // (At the floor: new work, as much as is above it; the floor taken as set, nothing past it.)
        let r = m4(6, 20);
        assert_eq!((r.held_back(), r.admit_mb(), r.below()), (None, 20 * GB, false));
        // (The swap grown lately comes off what's available.)
        let r = Reading { swap_grown_mb: 3 * GB, ..m4(8, 20) };
        assert_eq!((r.usable_mb(), r.room_mb()), (5 * GB, 19 * GB));
        // (Nothing held, nothing available: no room, nothing given back.)
        assert_eq!(m4(0, 0).room_mb(), 0);
    }

    /// A give-back's cool-down (task #945): no new work until what's available is back past the
    /// floor and the given-back job's peak, or `COOL_WAIT` has passed, whichever comes first; it
    /// gives nothing back itself.
    #[test]
    fn a_give_back_cools_the_mac_down_until_its_peak_is_back_or_the_wait_is_over() {
        let now = crate::agent::jobs::now_s();
        let cool = |avail: u64, until: u64| Reading { cool_mb: 6 * GB + 7 * GB, cool_until: until, ..m4(avail, 10) };
        // (A 7 GB job given back with a 6 GB floor: 10 GB available is above the floor, but short
        // of 13: held.)
        let r = cool(10, now + COOL_WAIT.as_secs());
        assert!(r.cooling() && !r.below() && r.admit_mb() == 0);
        assert_eq!(
            r.held_back().as_deref(),
            Some("a job was given back: no new job until 13.0 GB is available (the 6.0 GB floor and its 7.0 GB peak; 10.0 GB now), or for 5 min more")
        );
        // (Back past the floor and its peak: over.)
        let r = cool(13, now + COOL_WAIT.as_secs());
        assert!(!r.cooling() && r.held_back().is_none() && r.admit_mb() == 17 * GB);
        // (The wait over, short of it still: over too.)
        let r = cool(10, now);
        assert!(!r.cooling() && r.admit_mb() == 14 * GB);
        // (The swap grown counts off what's available for it too.)
        assert!(Reading { swap_grown_mb: GB, ..cool(13, now + 60) }.cooling());
        assert_eq!(COOL_WAIT, SWAP_WINDOW);
    }

    /// The brake (the kernel's pressure at warning or critical, or swap grown past what was
    /// available above the floor) holds new work whatever the number says; it acts on the running
    /// jobs (the room a megabyte under what they hold) only with evidence: the pressure critical,
    /// or the Mac below its floor by the numbers. Warning alone, with room, freezes nothing (task
    /// #932 review: macOS sits at warning for long stretches while it compresses).
    #[test]
    fn the_brake_holds_new_work_and_acts_on_jobs_only_with_evidence() {
        let r = Reading { pressure: 2, ..m4(20, 10) };
        assert_eq!(r.brake().as_deref(), Some("macOS's memory pressure is at warning"));
        assert!(!r.acts() && !r.below() && r.room_mb() == 24 * GB && r.admit_mb() == 0);
        assert_eq!(r.held_back().as_deref(), Some("the memory brake: macOS's memory pressure is at warning"));
        // (Warning with the Mac below its floor by the numbers: it acts.)
        let r = Reading { pressure: 2, ..m4(4, 10) };
        assert!(r.acts() && r.below() && r.room_mb() == 8 * GB);
        let r = Reading { pressure: 2, ..m4(4, 20) };
        assert!(r.acts() && r.room_mb() == 18 * GB);
        // (Critical: it acts whatever the number says.)
        let r = Reading { pressure: 4, ..m4(20, 10) };
        assert_eq!(r.brake().as_deref(), Some("macOS's memory pressure is critical"));
        assert!(r.acts() && r.below() && r.room_mb() == 10 * GB - 1 && r.admit_mb() == 0);
        // (Swap grew 3 GB with 8 available: 5 usable, under the floor, so the brake; 1 GB grown
        // with 8 available: 7 usable, above it, no brake.)
        let r = Reading { swap_grown_mb: 3 * GB, ..m4(8, 10) };
        assert_eq!(
            r.brake().as_deref(),
            Some("swap grew 3.0 GB in the last 5 min, past the 2.0 GB available above the floor")
        );
        assert!(r.acts(), "swap grown past the floor's room is below it by the numbers");
        assert_eq!(Reading { swap_grown_mb: GB, ..m4(8, 10) }.brake(), None);
        // (Below the floor by the number alone, no swap grown: given back at safe points, no brake.)
        assert_eq!(m4(4, 10).brake(), None);
        // (No job: the room is nothing, and nothing to give back.)
        assert_eq!(Reading { pressure: 4, ..m4(20, 0) }.room_mb(), 0);
    }

    /// The M4's night of 4 Nov replayed under the floor alone and the compressible memory counted
    /// (task #945). Recorded (the 10 s ring): at 00:07:30 one terrain-deep job held 3.2 GB, the
    /// kernel's level 75 % and its compressor 4.4 GB (40.4 GB not wired); four more started at
    /// 00:07:40 under the old jobs cap, the compressor reached two fifths of the unwired at 00:10:05
    /// and the pressure warning at 00:10:30; at 01:10 five held 37 GB, the compressor 23.1 GB of
    /// 39.8, the swap growing 3.6 GB in 90 s. The pages between those figures are a model (Docker's
    /// VM 8.2 GB and 4 GB of the owner's apps in use, 6 GB of them idle, the 5.4 GB file cache macOS
    /// kept, the compressor's ratio the M4's 1.79 read on 4 Nov), 7.5 GB a terrain-deep job's peak,
    /// the owner's 4 GB floor.
    #[test]
    fn the_fourth_of_november_replayed() {
        let page = |gb: f64| (gb * (1u64 << 30) as f64 / 16384.0).round() as u64;
        // (Memory, in GB: free, file cache, active and idle anonymous, the compressor and the GB
        // it holds.)
        let mac = |free: f64, file: f64, active: f64, idle: f64, comp: f64, stored: f64| -> Reading {
            let p = crate::sys::Pages {
                free: page(free),
                file: page(file),
                active: page(active),
                inactive: page(idle + file),
                compressor: page(comp),
                stored: page(stored),
                ..Default::default()
            };
            let a = p.available();
            let mb = |pages: u64| pages * 16384 >> 20;
            Reading {
                avail_mb: mb(a.total()),
                compress_mb: mb(a.compressible),
                floor_mb: 4 * GB,
                pressure: 1,
                ..Default::default()
            }
        };
        let peak = 7680;
        // (How many jobs a reading lets start with `running` jobs at their peaks, as `pack` does.)
        let admits = |r: &Reading, running: u64| {
            (0..8u64).take_while(|&n| r.admit_mb().saturating_sub((running + n) * peak) >= peak).count()
        };
        // 00:07:30: 9.2 GB free and 5.4 of cache, 2.6 more by compressing the 6 GB idle: 17.2 GB
        // available; the job's growth (4.3) and the floor leave room for one more, not four.
        let r = Reading { held_mb: 3277, ..mac(9.2, 5.4, 3.2 + 8.2 + 4.0, 6.0, 4.4, 4.4 * 1.79) };
        assert_eq!(r.compress_mb / 100, 27, "{r:?}");
        assert_eq!(admits(&r, 1), 1);
        // (Without the compressible memory, the floor alone, none: 14.6 − 4.3 − 4 < 7.5.)
        assert_eq!(admits(&Reading { avail_mb: r.avail_mb - r.compress_mb, compress_mb: 0, ..r }, 1), 0);
        // Both at their peaks (15 GB): the idle 6 GB compressed (the compressor 7.7 GB, 19 % of the
        // unwired: the pressure normal), the cache kept: 5.4 GB available, above the floor, room for
        // no third.
        let r = Reading { held_mb: 2 * peak, ..mac(0.0, 5.4, 15.0 + 8.2 + 4.0, 0.1, 7.7, 13.8) };
        assert!(!r.below() && r.held_back().is_none() && admits(&r, 2) == 0, "{r:?}");
        // (The compiles' 4 GB more met by compressing the jobs' and Docker's: the compressor 12.8 GB,
        // under two fifths, nothing more counted; still above the owner's 4 GB floor, so nothing
        // given back, nothing started. A Mac at its default floor, 6 GB, is below it: the newest
        // drains before the pressure leaves normal.)
        let r = Reading { held_mb: 2 * peak, ..mac(0.0, 5.4, 15.0 + 8.2 + 4.0 + 4.0 - 9.1, 0.0, 12.8, 22.9) };
        assert!(r.compress_mb == 0 && !r.below() && admits(&r, 2) == 0, "{r:?}");
        let at_default = Reading { floor_mb: 6 * GB, ..r };
        assert!(at_default.below() && !at_default.acts());
        let two = [Some(peak), Some(peak)];
        assert_eq!(decide_by(&two, at_default.room_mb(), &[false; 2], &[false; 2], &[1, 2]), Act::Drain(1));
        // (But past two fifths the M4's series kept more cache than the model's 5.4 GB: 6.4 GB at
        // the median at warning. There even the default floor isn't reached: nothing given back by
        // it, no job started; the brake (the pressure at warning, swap grown past the floor's room)
        // is what acts.)
        let kept = Reading {
            held_mb: 2 * peak,
            floor_mb: 6 * GB,
            ..mac(0.0, 6.4, 15.0 + 8.2 + 4.0 + 4.0 - 9.1 - 1.0, 0.0, 12.8, 22.9)
        };
        assert!(kept.compress_mb == 0 && !kept.below() && admits(&kept, 2) == 0, "{kept:?}");
        assert!(Reading { pressure: 2, swap_grown_mb: GB, ..kept }.acts(), "swap grown past the floor's room");
        // 01:10 as it was, five jobs at 37 GB: the compressor 23.1 GB of 39.8, past two fifths, so
        // compression counts nothing (0.2 GB free, the 5.4 kept); the pressure at warning: no job
        // starts; the swap grown 3.6 GB leaves 2.0 usable, under the floor: the brake acts, the
        // newest frozen at once and given back if the Mac isn't back within the minute.
        let r = Reading {
            held_mb: 37 * GB,
            pressure: 2,
            swap_grown_mb: 3686,
            ..mac(0.2, 5.4, 39.8 - 23.1 - 0.2 - 5.4, 0.0, 23.1, 23.1 * 1.79)
        };
        assert_eq!(r.compress_mb, 0, "{r:?}");
        assert!(r.brake().is_some() && r.acts() && r.below() && r.admit_mb() == 0, "{r:?}");
        let held = [Some(7 * GB), Some(8 * GB), Some(7 * GB), Some(8 * GB), Some(7 * GB)];
        assert_eq!(decide_by(&held, r.room_mb(), &[false; 5], &[false; 5], &[1, 2, 3, 4, 5]), Act::Drain(4));
        // (Given back, the 7 GB job's cool-down: the Mac back above its floor holds new work until
        // 11 GB is available, or five minutes.)
        let now = crate::agent::jobs::now_s();
        let after = Reading {
            held_mb: 30 * GB,
            pressure: 1,
            swap_grown_mb: 0,
            avail_mb: 6 * GB,
            cool_mb: 4 * GB + 7 * GB,
            cool_until: now + COOL_WAIT.as_secs(),
            ..r
        };
        assert!(!after.below() && after.cooling() && after.admit_mb() == 0);
    }

    /// Another process grows (Chrome, Docker's VM) while the jobs hold steady: the Mac falls below
    /// its floor, the newest job is given back; once it's gone the Mac is above the floor again, but
    /// the give-back's cool-down holds new work until the floor and that job's peak are available.
    #[test]
    fn another_process_growing_gives_a_job_back_and_cools_the_mac_down() {
        let s = Sampler::new(2);
        s.set(6 * GB, 48 * GB);
        for k in 0..2 {
            s.watch(k, Watch { pgid: 999_990 + k as i32, ..Default::default() });
        }
        let no = [false; 2];
        let rank = [1, 2];
        // (The jobs hold 5 and 3 GB; 10 GB available: 12 GB of room, nothing to do.)
        s.set_test(Some(vec![Some(5 * GB), Some(3 * GB)]), Some(false));
        s.set_test_mem(Some(10 * GB), Some(0));
        s.tick();
        let r = s.reading().unwrap();
        assert!(!r.below() && r.held_back().is_none());
        assert_eq!(decide_by(&[Some(5 * GB), Some(3 * GB)], r.room_mb(), &no, &no, &rank), Act::Nothing);
        // (Another process takes 6 GB: 4 available, under the 6 GB floor; the jobs past their
        // room, 6 GB: the newest is given back at its next safe point, not the largest.)
        s.set_test_mem(Some(4 * GB), Some(0));
        s.tick();
        let r = s.reading().unwrap();
        assert!(r.below() && r.brake().is_none(), "{r:?}");
        assert_eq!(r.short_words(), "this Mac has 4.0 GB available, below its 6.0 GB floor");
        assert_eq!(decide_by(&[Some(5 * GB), Some(3 * GB)], r.room_mb(), &no, &no, &rank), Act::Drain(1));
        // (It ends, given back at 3 GB: 7 GB available, above the floor, so nothing more is given
        // back; the cool-down (the agent's, set on the sampler) holds new work until 9 GB, the
        // floor and its 3 GB, are available.)
        s.unwatch(1);
        s.set_cool(9 * GB, crate::agent::jobs::now_s() + COOL_WAIT.as_secs());
        s.set_test(Some(vec![Some(5 * GB), None]), Some(false));
        s.set_test_mem(Some(7 * GB), Some(0));
        s.tick();
        let r = s.reading().unwrap();
        assert!(!r.below() && r.cooling(), "{r:?}");
        assert_eq!(decide_by(&[Some(5 * GB), None], r.room_mb(), &no, &no, &rank), Act::Nothing);
        assert!(r.held_back().is_some_and(|h| h.starts_with("a job was given back: no new job until 9.0 GB")));
        assert_eq!(r.admit_mb(), 0);
        // (The other process gives 2 GB back: 9 GB available, the cool-down over, 8 GB of room.)
        s.set_test_mem(Some(9 * GB), Some(0));
        s.tick();
        let r = s.reading().unwrap();
        assert!(!r.cooling() && r.held_back().is_none() && r.admit_mb() == 8 * GB, "{r:?}");
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
        // No room at all (the Mac at or under its floor with nothing to spare): the job, alone.
        assert_eq!(decide(&[Some(50 << 10)], 0, &[false], &[false]), Act::Over(0));
        // A job started beside one that waited on a worker yields when that one goes on, the
        // larger or not; while it drains, the other isn't drained for it; past the room alone,
        // either is stopped as any.
        let y = &[false, true];
        assert_eq!(decide(&[Some(10 << 10), Some(35 << 10)], l, &[false, false], y), Act::Drain(1));
        assert_eq!(decide(&[Some(35 << 10), Some(10 << 10)], l, &[false, false], y), Act::Drain(1));
        assert_eq!(decide(&[Some(10 << 10), Some(35 << 10)], l, &[false, true], y), Act::Nothing);
        assert_eq!(decide(&[Some(30 << 10), Some(10 << 10)], l, &[false, false], y), Act::Nothing);
        assert_eq!(decide(&[Some(43 << 10), Some(1 << 10)], l, &[false, false], y), Act::Over(0));
    }

    /// Several jobs (a member's packed ones, task #240): past the room together, the largest fitting
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
        // (The largest by itself past the room: it, whoever is newest.)
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

    /// The sampler reads the Mac's memory with its jobs': the swap grown over its window, and the
    /// cool-down the agent set; a real Mac's available, what compressing frees among it.
    #[test]
    fn the_sampler_reads_the_swap_grown_and_the_cool_down() {
        let s = Sampler::new(1);
        s.set_test(Some(vec![Some(10 * GB)]), Some(false));
        s.watch(0, Watch { pgid: 999_999, ..Default::default() });
        // (Its memory unknown: no reading, nothing guarded.)
        s.tick();
        assert_eq!(s.reading(), None);
        s.set(6 * GB, 48 * GB);
        s.set_test_mem(Some(20 * GB), Some(4 * GB));
        s.tick();
        let r = s.reading().unwrap();
        assert_eq!((r.avail_mb, r.held_mb, r.floor_mb, r.swap_grown_mb, r.cool_mb), (20 * GB, 10 * GB, 6 * GB, 0, 0));
        // (The swap grew 1 GB; a cool-down set.)
        s.set_cool(14 * GB, 99);
        s.set_test_mem(Some(17 * GB), Some(5 * GB));
        s.tick();
        let r = s.reading().unwrap();
        assert_eq!((r.swap_grown_mb, r.cool_mb, r.cool_until), (GB, 14 * GB, 99));
        assert!(!s.in_trouble());
        // (This Mac's own memory: its available, what compressing frees part of it.)
        if cfg!(target_os = "macos") {
            s.set_test_mem(None, None);
            s.tick();
            let r = s.reading().unwrap();
            assert!(r.avail_mb > 0 && r.compress_mb <= r.avail_mb, "{r:?}");
        }
    }

    #[test]
    fn floors_read_as_written_before_they_said_more() {
        let old: BTreeMap<String, Floor> =
            serde_json::from_str(r#"{"a": 9000, "b": {"mb": 5, "alone": true, "v": 3}}"#).unwrap();
        assert_eq!((old["a"], old["b"]), (Floor { mb: 9000, alone: false, v: 0 }, Floor { mb: 5, alone: true, v: 3 }));
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
        assert_eq!(targets(&p), Targets { on: Some(("terrain 6/1/2".into(), 4)), ended: 1, threads: None });
        // (Task #948: the threads the last target to begin runs on, as its line says; a drop's line,
        // run outside its job's grant, says none and leaves them.)
        std::fs::write(&p, "{\"unit\":\"slope-deep 10/1/1\",\"started\":1,\"threads\":6}\n{\"unit\":\"slope-deep 10/1/1\",\"peak_mb\":900,\"secs\":3}\n{\"unit\":\"slope-deep 10/1/2\",\"started\":4,\"threads\":2}\n{\"unit\":\"slope-deep 10/1/2\",\"peak_mb\":900,\"secs\":3}\n{\"unit\":\"slope-deep 10/1/3\",\"started\":5}\n").unwrap();
        assert_eq!(targets(&p).threads, Some(2));
        // (A step that lands a target's uploads beside the next target's work writes its end line
        // after the next's start: each still ended, the next under way; the packing at the end
        // after the last: all ended, none under way.)
        std::fs::write(&p, "{\"unit\":\"terrain-hi 6/1/1\",\"started\":1}\n{\"unit\":\"terrain-hi 6/1/2\",\"started\":5}\n{\"unit\":\"terrain-hi 6/1/1\",\"peak_mb\":9}\n{\"unit\":\"terrain-hi 6/1/3\",\"started\":9}\n{\"unit\":\"terrain-hi 6/1/2\",\"peak_mb\":9}\n").unwrap();
        assert_eq!(targets(&p), Targets { on: Some(("terrain-hi 6/1/3".into(), 9)), ended: 2, threads: None });
        std::fs::write(&p, "{\"unit\":\"terrain-hi 6/1/1\",\"started\":1}\n{\"unit\":\"terrain-hi 6/1/2\",\"started\":5}\n{\"unit\":\"terrain-hi 6/1/1\",\"peak_mb\":9}\n{\"unit\":\"terrain-hi 6/1/2\",\"peak_mb\":9}\n{\"unit\":\"terrain-hi 6/1/2\",\"peak_mb\":9}\n{\"unit\":\"never begun\",\"peak_mb\":9}\n").unwrap();
        assert_eq!(targets(&p), Targets { on: None, ended: 2, threads: None }, "each ends once, only after its start");
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
        // (A key of no space that isn't a z6 tile is no z6 tile's: task #849.)
        assert_eq!((step_of_key("gc"), step_of_key("3/1/1")), ("gc", "3/1/1"));
    }

    #[test]
    fn the_sampler_keeps_the_most_held_per_target_and_freezes_only_in_trouble() {
        let d = tempfile::tempdir().unwrap();
        let s = Sampler::new(2);
        // (A 16 GB Mac, a 2 GB floor, 8 GB available.)
        s.set(2 << 10, 16 << 10);
        s.set_test_mem(Some(8 << 10), Some(0));
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
            l.on && l.reading.is_some_and(|r| r.floor_mb == 2 << 10 && r.held_mb == 3000)
                && l.held_mb == [Some(3000), None]
                && l.frozen == [false, false],
            "{l:?}"
        );
        // Past the room alone (13 GB held, 1 GB available: 12 GB of room above the floor), the
        // brake off: not frozen.
        s.set_test_mem(Some(1 << 10), Some(0));
        s.set_test(Some(vec![Some(13 << 10), None]), Some(false));
        s.tick();
        assert!(!s.frozen(0));
        // The brake on: frozen, said.
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
        // The Mac's memory unknown: learning only.
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
        s.set(2 << 10, 0);
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
}
