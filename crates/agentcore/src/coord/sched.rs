//! The scheduler (docs/pool.md §7.2, §7.7): the one place a job is granted. Every rule that decides
//! which jobs a Mac's ask gets lives here and nowhere else (tests/grant_rules.rs keeps it so): the
//! offers in plan order, the lead's own planned jobs to the lead alone (crate::agent::every::grants),
//! a step's targets as a run in plan order (`pick_every`) and the lead's planned jobs whole
//! (`pick_own`), the packing by measured memory, disk and cores (crate::needs), cold starts by their
//! provisional need, one at a time on a Mac and never-measured steps alone, the NAS budget
//! (crate::budget::nas: its weights and limits, the aged reservations, quiet steps, the downloads'
//! cap, the gate on the NAS's own load), the mixed-version hold (one job an ask while a member runs
//! an older app) and placement (crate::agent::placement).
//!
//! It's pure: `Sched::pack` decides from its state (`Sched`), the ask and a clock it's handed
//! (`Clock`), and changes nothing but its state (the leases it grants, the targets' waits on the
//! budget, each worker's last target). No file, no network, no wall clock, no log: the live
//! coordinator (crate::coord's `Shared::pack`) saves, logs and records its grants; the forecast
//! (crate::agent::forecast) runs the same `pack` over simulated time on a copy of the live state
//! (`State`), so its schedule is the coordinator's by construction.

use super::lease::{Lease, Leases, Work};
use super::{cost_key, disk_words, first_guess, mem_words, Ask, Cost, Measured, Offer, Worker};
use super::{AROUND, ASKING};
use crate::agent::memguard::Floor;
use roadcore::fx::FxMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The time a decision is made at: the coordinator's clock (`now`, what leases and waits are
/// reckoned in) and the wall clock's unix second with it (a lease's `granted_at`). The live
/// coordinator reads both; the forecast steps them through simulated time.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub now: Instant,
    pub unix: u64,
}

/// A job granted (`Sched::pack`): its lease (held in `Sched::leases` from then), step and targets
/// (target, key), the threads it runs, the memory it was packed at (`pack_need`: its measure, else a
/// cold start's provisional need), the disk it reserves (its measure and its shared cache's fill),
/// and the lead's planned job it is (`Offer::job`).
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub lease: u64,
    pub step: String,
    pub targets: Vec<(String, String)>,
    pub threads: u32,
    pub mem_mb: u64,
    pub disk_mb: u64,
    pub job: Option<String>,
    /// The time it was sized to (s), and whether by its Mac's own runs (`Sched::batch_secs`); None:
    /// a planned job of the lead's own (`pick_own`), or a target of no time measured.
    pub sized: Option<(u64, bool)>,
}

/// What every grant is decided from: the leases out, the plan's offers, the work done and failed,
/// the workers' last asks, the measures (costs, floors, disk floors, jobs' disks, caches' fills)
/// and the NAS budget's limits and waits.
#[derive(Debug)]
pub struct Sched {
    /// The plan the steps are asked of (crate::agent::seam::Plan).
    pub the_plan: &'static dyn crate::agent::seam::Plan,
    pub leases: Leases,
    /// The plan's work a worker that mounts the NAS may do, a step at a time in plan order, and the
    /// lead's own planned jobs (`Offer::job`). Set through `set_offers`, which keeps `offered`.
    pub offers: Vec<Offer>,
    /// Each step's offered targets, the size each was first offered with (`offers` indexed: the
    /// placement looks up every target it weighs, and a worldwide pass offers tens of thousands).
    pub offered: FxMap<String, FxMap<String, u64>>,
    /// The offered targets that are drops (offered under their step's "none" key,
    /// crate::agent::build::is_none_key): sized by their step's drops (crate::needs::Book::drop_need)
    /// until their own run says, never by its builds.
    pub drops: std::collections::HashSet<(String, String)>,
    /// Work done (journaled, or recorded by this Mac's job), (step, target) → key, under a key the
    /// plan hasn't shown yet: not offered again meanwhile.
    pub done: BTreeMap<(String, String), String>,
    /// A worker's failures: (worker, cost_key(step, target)) → (the last, how many): not offered to
    /// it again for an hour, doubling each time.
    pub failed: BTreeMap<(String, String), (Instant, u32)>,
    pub workers: BTreeMap<String, Worker>,
    pub costs: BTreeMap<String, Cost>,
    /// What a target takes at least (by cost key: crate::agent::memguard::Floor), as a run that
    /// didn't measure it saw it hold: its predicted memory is never below it. A measure of the
    /// target, a run's own (the same kind of figure: its job's processes together, sampled), takes
    /// its place. Kept in `floors.json`.
    pub floors: BTreeMap<String, Floor>,
    /// The disk a target takes at least (MB, by cost key): the most a run of it was seen to take
    /// (counted by the accessors that write a job's files, the largest at the top of its scratch
    /// folder, the disk's fall while it ran alone: crate::agent::memguard). Only rises. Kept in
    /// `disks.json`.
    pub disks: BTreeMap<String, u64>,
    /// The disk each step's last jobs took over their whole runs (MB, by step, oldest first, the
    /// last `JOB_PEAKS`): a batch's cache writes add up over its targets, and outside programs'
    /// files can outlast a target, so a batch is reckoned by the larger of its targets' disk
    /// summed and these runs' 90th percentile (`job_floor`); a step reading a shared cache's
    /// (crate::needs::CACHES) count for nothing past their cache's fill (`note_fill`). Kept in
    /// `job-disks-v6.json` (`JOB_DISKS`).
    pub job_disks: BTreeMap<String, Vec<u64>>,
    /// What filling each shared cache (crate::needs::CACHES) on a Mac takes (MB, by cache): the
    /// last run that filled it, its disk past its targets' own output (`note_fill`). A Mac is
    /// reserved what of it its capacity says it lacks (`missing`). Kept in `cache-fills.json`.
    pub fills: BTreeMap<String, u64>,
    /// The steps' models of their needs (crate::needs), kept while `needs_rev` stands: it moves with
    /// each cost, floor or disk floor taken or cleared.
    pub needs: crate::needs::Cache,
    pub needs_rev: u64,
    /// The app this Mac's agent runs: an agent on another builds with other code than the keys it
    /// would record say, so it gets no work ("" in tests: any).
    pub app: String,
    /// Placement by how crucial a job is (docs/pool.md §12, phase 4's eleventh batch:
    /// crate::agent::placement), while the lead's switch has it on, and when it was first told (its
    /// draws' windows are reckoned from then): the offers tried by their steps' slack, a member's
    /// targets drawn among those asking by their paces (crate::paces), the tasks by their measured
    /// pace at the kind, a task offered ahead given
    /// to a slower worker its head start covers. None: as before.
    pub placement: Option<(crate::agent::placement::Placement, Instant)>,
    /// The NAS budget's limits (docs/pool.md §7.7, crate::budget::nas): the NAS's as probed, and the
    /// downloads' cap. Read by `pick_every` (so `pack` too). None: the switch off, or no probe yet.
    pub nas: Option<crate::budget::nas::Limits>,
    /// The NAS's limits as probed whatever the switch says: the lead's own jobs are always held to
    /// the budget (`nas_room`). None: not probed yet.
    pub nas_own: Option<crate::budget::nas::Limits>,
    /// This Mac's member name, its own jobs' leases held under it (`Coordinator::ask_own`): the NAS
    /// budget lets a member's job go past the whole while the lead's own alone hold weight.
    pub lead_name: String,
    /// Targets passed over for the NAS budget alone ((step, target) → (first, last time)), for the
    /// status (`Coordinator::nas_waits`); a target granted is let go.
    pub nas_waits: Mutex<BTreeMap<(String, String), (Instant, Instant)>>,
    /// `nas_reserved`'s wait (since when, step and target) as last worked out, at which time: kept
    /// while the time stands, an aged wait passed over again taking its place when it was held
    /// back first, worked out again when the one kept is granted (task #544: a pick weighs it for
    /// every target).
    pub nas_reserved_at: Mutex<Option<(Instant, Option<Reserved>)>>,
    /// When each quiet step's job (crate::budget::nas::QUIET_STEPS) was first held back for a
    /// quiet NAS since it was last granted: past `QUIET_AGE` it goes as an aged job (task #451).
    pub quiet_since: Mutex<BTreeMap<String, Instant>>,
    /// Each worker's last target granted of a step ((worker, step) → target): its next run starts
    /// just after it in plan order when it can (`pick_every`).
    pub last: BTreeMap<(String, String), String>,
    /// The steps with a measure of any kind, as the measures stood (`never_measured`).
    pub measured_steps: Mutex<(u64, BTreeSet<String>)>,
    /// Each target's `measured` (step → target → it), as the measures stood (`rev`) and as offered
    /// (`set_offers` clears it): a placement weighs each target it may grant for every ask.
    pub measured_of: Mutex<(u64, Measures)>,
    /// Each target's look for `provisional` (`provisional_of`: step → target → its memory and
    /// disk), as the measures stood (`rev`) and as offered (`set_offers` clears it).
    pub provisional_at: Mutex<(u64, FxMap<String, FxMap<String, (u64, Option<u64>)>>)>,
    /// Each step's drops' need (crate::needs::Book::drop_need) as the measures stood (`rev`).
    pub drop_needs: Mutex<(u64, BTreeMap<String, Option<(u64, u64)>>)>,
    /// While `pack` packs, what it looks up of the leases (`Packing`); None otherwise.
    pub packing: Mutex<Option<Packing>>,
    /// What's worked out of each leased job (`LeaseKept`), by lease, as the measures stood (`rev`),
    /// as offered and as the steps' uses stood (`set_offers`, `set_use_runs` and `add_use` clear it).
    pub kept_leases: Mutex<(u64, FxMap<u64, Vec<LeaseKept>>)>,
    /// Each Mac's pace at each step against the lead's (crate::paces, task #926; set through
    /// `set_paces`, which moves `needs_rev`): the measures' times and NAS rates are read at the
    /// lead's pace by it, and a worker's at its own. Empty: as measured.
    pub paces: crate::paces::Paces,
    /// The runs of the build's timings in the window (crate::uses, task #947; `add_use`), and each
    /// step's use on each Mac they say: what a job is granted threads by (`job_cores`).
    pub use_runs: Vec<crate::uses::Run>,
    pub uses: crate::uses::Uses,
    /// The batch length the pool's setting says (s; `batch-minutes`, crate::coord::asks; 0: not set,
    /// `JOB_S`): `batch_len`.
    pub batch_s: u64,
}

/// What one `Sched::pack` keeps of the leases while it packs, made again at each grant: the NAS
/// budget's sums at (now, lead) (`nas_room_heaviest`) and each step's targets held (`held_of`).
#[derive(Debug, Default)]
pub struct Packing {
    nas: Vec<((Instant, bool), (u32, u32, usize, [u32; 2]))>,
    held: BTreeMap<(String, Instant), std::sync::Arc<BTreeSet<String>>>,
}

/// What's kept of a leased job (`Sched::kept_leases`), of its worker and work: its NAS weight by
/// the limits it was weighed by (`Sched::lease_nas`), and its packing need and time
/// (`Sched::lease_need`).
#[derive(Debug)]
pub struct LeaseKept {
    worker: String,
    work: Work,
    nas: Vec<(crate::budget::nas::Limits, (u32, bool))>,
    need: Option<(Need, Option<u64>)>,
}

/// `Sched::measured_of`'s targets' measures: step → target → `Sched::measures`.
/// (Looked in for every target an ask weighs, never walked: roadcore::fx.)
pub type Measures = FxMap<String, FxMap<String, (u64, Option<Measured>)>>;

/// The NAS budget's weights for one placement (`Shared::nas_room`): the limits and one Book (its
/// steps' 90th percentiles kept with its models: crate::needs::Book::nas_prior).
pub(crate) struct NasWeigher<'a> {
    limits: crate::budget::nas::Limits,
    book: crate::needs::Book<'a>,
}

impl NasWeigher<'_> {
    /// The weight of `target` of `step` on `worker` (its pace's, `Book::nas_for`; None: at the
    /// lead's pace), none of its step measured: the whole for a NAS step, else 0.
    fn weight(&self, step: &str, target: &str, worker: Option<&str>) -> u32 {
        let r = match worker {
            Some(w) => self.book.nas_for(step, target, w),
            None => self.book.nas(step, target),
        };
        // (A step known to load the NAS (`NAS_STEPS`) none of whose runs the way it runs now
        // (`cost_v`) noted its rate: the whole budget, so its first job never overloads the NAS
        // unseen, task #499 review; runs of an earlier way count for nothing, crate::needs's
        // `nas_rate`.)
        r.map_or(if NAS_STEPS.contains(&step) { crate::budget::nas::WHOLE } else { 0 }, |r| self.limits.weight(r))
    }

    /// Target `target` added to job `j` (`JobLoad`): its weight, and its rate over the time it lasts
    /// (crate::needs::Book::nas_load).
    fn add(&self, j: &mut JobLoad, step: &str, target: &str, worker: Option<&str>) {
        j.heaviest = j.heaviest.max(self.weight(step, target, worker));
        match self.book.nas_load(step, target, worker) {
            Some((r, Some(s))) => {
                j.moved.bytes_s += r.bytes_s * s;
                j.moved.creates_s += r.creates_s * s;
                j.moved.listed_s += r.listed_s * s;
                j.moved.stats_s += r.stats_s * s;
                j.moved.deletes_s += r.deletes_s * s;
                j.secs += s;
            }
            _ => j.untimed = true,
        }
    }

    /// The weight of job `j`: what its targets move over their times together, as a job runs them
    /// one after another (task #939: weighed by its heaviest target, a slope-deep job of 52 targets
    /// on the M4, 4 Nov, weighed the whole NAS for its 32 minutes, its targets of a second or two
    /// each measured past the NAS's rate, while it moved 12 MB/s, a seventh of the NAS, over its
    /// run); one with a target of no measured rate or no known time, its heaviest target's.
    fn job(&self, j: &JobLoad) -> u32 {
        if j.untimed || j.secs <= 0.0 {
            return j.heaviest;
        }
        let per = |x: f64| x / j.secs;
        self.limits.weight(crate::budget::nas::Rate {
            bytes_s: per(j.moved.bytes_s),
            creates_s: per(j.moved.creates_s),
            listed_s: per(j.moved.listed_s),
            stats_s: per(j.moved.stats_s),
            deletes_s: per(j.moved.deletes_s),
        })
    }
}

/// A job's NAS load as its targets are added (`NasWeigher::add`, `NasWeigher::job`): what they
/// move (each rate times its time, summed) and their time, their heaviest weight, and whether
/// one's rate or time isn't known.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct JobLoad {
    moved: crate::budget::nas::Rate,
    secs: f64,
    heaviest: u32,
    untimed: bool,
}

/// A wait that holds the NAS budget's room (`Sched::nas_reserved`): since when, its step and target.
pub type Reserved = (Instant, (String, String));

/// What a job is packed as (`Shared::pack_need`): its memory and disk, its cores as measured
/// (crate::needs::Cores; None: not said), and whether it's a cold start (none of it measured, `m` its provisional need).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Need {
    pub(super) m: Measured,
    cores: Option<crate::needs::Cores>,
    pub(super) cold: bool,
    /// Only waiting on a worker (its Mac's `Capacity::waiting`): counted at the memory it holds
    /// now, within its measure, and none of its cores (docs/pool.md §7.2, A job waiting on a worker).
    waits: bool,
    /// Leaving its Mac's cores idle (its Mac's `Capacity::idle`): none of its cores (docs/pool.md
    /// §7.2, A job leaving the cores idle).
    idle: bool,
    /// Its cores are its step's use on its Mac (crate::uses, `job_cores`), cold or not.
    by_use: bool,
    /// A job held: the threads its lease counts it for (`Lease::threads`, as granted and
    /// re-granted; 0: not said), which its threads and busy cores never pass (`need_cores`).
    held: u32,
    /// A job held: the cores it keeps busy now, in tenths, once its run says it (`busy_now`), which
    /// it counts for in place of its measure's.
    busy_now: Option<u32>,
}

/// The room a job of the first offer refused for its room alone waits for on a Mac (`Sched::pack`,
/// task #965): its memory and disk (a shared cache's fill among it) and the cores it counts for
/// (tenths).
struct RoomWait {
    step: String,
    mem_mb: u64,
    disk_mb: u64,
    cores: u32,
}

/// A job on the Mac an ask packs for, as the room held for a waiting job weighs it (`Backfill`):
/// its memory, disk and cores (tenths), and in how long it's expected to end (s from now) by its
/// measured time (`Sched::batch_secs`) less what it has run (0: past it; None: not measured).
#[derive(Clone, Copy, Debug)]
struct Run {
    mem_mb: u64,
    disk_mb: u64,
    cores: u32,
    ends: Option<u64>,
}

impl Run {
    /// A job measured `m`, counting for `cores` (tenths), of measured time `secs`, run `ran` s.
    fn of(m: Measured, cores: u32, secs: Option<u64>, ran: u64) -> Run {
        Run { mem_mb: m.mem_mb, disk_mb: m.disk_mb, cores, ends: secs.map(|s| s.saturating_sub(ran)) }
    }
}

/// The room held for a waiting job (`RoomWait`) and what may still go while it waits (`Sched::pack`,
/// task #973: backfilling, so a hold never idles a Mac's cores for as long as its running jobs
/// last). The jobs it waits for (`waits_on`) are those of its Mac expected to end soonest, as few
/// as free its room once they end, any past its time or not measured first (it may end any
/// moment); `shadow` is when they're expected to have ended (s from now, by their measured times;
/// 0 where one is past its time or not measured). A later job goes when it fits beside the
/// waiting one once those end, every other job still running (`beside`), or, its time measured,
/// ends before `shadow` by its runs' 90th percentile. A job of the second kind ends before the
/// jobs waited for, so it's among them next time without putting off when they end; once one of
/// them overruns, no more of that kind goes. So the waiting job waits only for a finite set of
/// jobs, ending no later than those it first waited for while none overruns, and never for a
/// long job while a shorter one frees its room.
struct Backfill {
    w: RoomWait,
    waits_on: usize,
    shadow: u64,
    /// The room left beside it once the jobs it waits for end, every other job still running:
    /// memory, disk (None: not said), cores (tenths); None: short of it even then.
    beside: Option<(u64, Option<u64>, u32)>,
}

impl Backfill {
    /// The room held for `w` on a Mac of `room` and `cores` (tenths) left, running `runs`.
    fn of(w: RoomWait, room: &Ask, cores: u32, runs: &[Run]) -> Backfill {
        let fits =
            |(m, d, c): (u64, u64, u32)| m >= w.mem_mb && room.disk_mb.is_none_or(|_| d >= w.disk_mb) && c >= w.cores;
        let free = |rs: &[Run]| {
            rs.iter().fold((room.mem_mb, room.disk_mb.unwrap_or(0), cores), |(m, d, c), r| {
                (m + r.mem_mb, d + r.disk_mb, c + r.cores)
            })
        };
        // (Soonest to end first: one past its time or not measured as ending now, so it's waited
        // for, which stops the second kind.)
        let mut runs = runs.to_vec();
        runs.sort_by_key(|r| r.ends.unwrap_or(0));
        let waits_on = (0..=runs.len()).find(|&k| fits(free(&runs[..k]))).unwrap_or(runs.len());
        let waited = &runs[..waits_on];
        let shadow = waited.iter().try_fold(0, |s, r| r.ends.filter(|&x| x > 0).map(|x| s.max(x))).unwrap_or(0);
        let left = free(waited);
        let beside = fits(left).then(|| {
            let (m, d, c) = left;
            (m - w.mem_mb, room.disk_mb.map(|_| d - w.disk_mb), c - w.cores)
        });
        Backfill { shadow, waits_on, beside, w }
    }

    /// Whether a job of measure `m` (and `fill` of a shared cache's disk), counting for `busy`
    /// tenths of a core and expected to take `p90` s (its runs' 90th percentile; None: not
    /// measured), may go: it fits beside the waiting job once the jobs that one waits for end, or
    /// ends before they're expected to.
    fn lets(&self, m: &Measured, fill: u64, busy: u32, p90: impl FnOnce() -> Option<u64>) -> bool {
        self.beside
            .is_some_and(|(mem, disk, c)| m.mem_mb <= mem && disk.is_none_or(|d| m.disk_mb + fill <= d) && busy <= c)
            || p90().is_some_and(|s| s < self.shadow)
    }

    /// The refusal in words, of `room` and the `cores` left.
    fn words(&self, room: &Ask, cores: u32) -> String {
        let w = &self.w;
        let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
        let mut s = format!("room held for a job of {} ({} of memory, {} left", w.step, gb(w.mem_mb), gb(room.mem_mb));
        // (Its disk too where that's what it's short of: a wait for disk alone read as one for
        // memory with room to spare.)
        if let Some(d) = room.disk_mb.filter(|&d| d < w.disk_mb) {
            s += &format!("; {} of disk, {} left", gb(w.disk_mb), gb(d));
        }
        if w.cores > cores {
            s += &format!("; {} cores, {} left", tenths(w.cores), tenths(cores));
        }
        let (jobs, its) = match self.waits_on {
            1 => ("the job it waits for".to_string(), "its"),
            n => (format!("the {n} jobs it waits for"), "their"),
        };
        let when = match self.shadow {
            0 => format!("{jobs} running past {its} measured time or not measured"),
            t => format!("{jobs} expected to end in {} min", t.div_ceil(60)),
        };
        s + &format!(", {when}): nothing offered after it fits beside it or ends before then")
    }
}

/// The steps known to honour the threads they're granted (task #499 review): their cold starts
/// take a thread (task #896: their run measures what they use, and climbs from there), the others'
/// the whole Mac; only where the step has no use on the Mac (crate::uses, task #947: its timings
/// say what it keeps busy, honoured or not). `slope-hi` and `slope-deep`: Rust steps
/// on the rayon pool the agent sizes (`RAYON_NUM_THREADS`, crate::agent::jobs); `terrain-deep`: its
/// band past z12 made on that pool, its reads ahead waiting on the network (crate::terrain_deep); `bld-fetch`: its
/// downloads bounded by its `--jobs` (3 by default, at most
/// 4), waiting on the network; `pyramids-fill`: its fetches a host's requests in flight (crate::fetch::HOST_REQUESTS),
/// waiting on the network, its own work (whole-PNG checks, packing) a fraction of a core (task #823:
/// counted as the whole Mac, its first job held 6 of the mini's 12 cores).
pub(super) const THREADS_HONOURED: &[&str] = &["slope-hi", "slope-deep", "terrain-deep", "bld-fetch", "pyramids-fill"];

/// A cold start's provisional memory (MB) from `mb`, the most its step's runs of its latest earlier
/// way measured or it was offered with: times 3/2 (task #499; `Sched::provisional`, the forecast's
/// packing too).
pub(super) fn provisional_mb(mb: u64) -> u64 {
    mb.saturating_mul(3) / 2
}

/// The cores a job of `step` over `targets` takes by the cost book (crate::needs::Book::cores):
/// the most of its targets', each of them; None when one's aren't said. What its step's use on its
/// Mac (crate::uses) stands in for where none is known.
fn book_cores(b: &crate::needs::Book, step: &str, targets: &[String]) -> Option<crate::needs::Cores> {
    let v = targets.iter().map(|t| b.cores(step, t)).collect::<Option<Vec<crate::needs::Cores>>>()?;
    Some(crate::needs::Cores {
        grant: v.iter().map(|c| c.grant).max().unwrap_or(0),
        busy_tenths: v.iter().map(|c| c.busy_tenths).collect::<Option<Vec<u32>>>().and_then(|b| b.into_iter().max()),
    })
}

/// The cores `a`'s Mac has for its jobs together: its capacity's (else the ask's), less the in-use
/// share's (`Capacity::kept_cores`) while its owner uses it; one at least.
pub fn ask_cores(a: &Ask) -> u32 {
    let cap = a.cap.as_ref();
    let total = cap.map(|c| c.cores).filter(|&c| c > 0).unwrap_or(a.cores).max(1);
    total.saturating_sub(cap.filter(|c| c.in_use).map_or(0, |c| c.kept_cores)).max(1)
}

/// The cores `a`'s Mac has busy outside the pool: those its last minute's CPU keeps busy
/// (its capacity's `cores` less `cores_free`) past `ours`, the threads of the pool's jobs it holds,
/// less the cores the in-use share keeps from its jobs already while its owner uses it
/// (`ask_cores` takes those off). 0 for an ask without a capacity.
/// The minute's busy cores are rounded up to whole cores: an excess of one is taken for that and counts
/// none (task #896: no share of the Mac stands for a job just ended any more; one still in the
/// minute counts as outside it until the minute shows it gone, a minute at most, and it keeps half the
/// cores at most meanwhile).
/// At most half the cores its Mac has for its jobs (`ask_cores`) count (task #684): the cores
/// busy can't tell a niced test build (tools/app/remote-tests.sh runs at nice 10) from work
/// that competes, and on 26 Oct an agent's check build held the
/// lead's load at 47 of 12 cores, so it granted itself nothing for two hours. The owner's own use
/// keeps the in-use share besides.
pub fn outside_cores(a: &Ask, ours: u32) -> u32 {
    a.cap.as_ref().map_or(0, |c| {
        let busy = c.cores.saturating_sub(c.cores_free);
        let kept = if c.in_use { c.kept_cores } else { 0 };
        let excess = busy.saturating_sub(ours).saturating_sub(kept);
        let total = ask_cores(a);
        if excess <= 1 {
            0
        } else {
            excess.min(total / 2)
        }
    })
}

/// The cores held job `l` keeps busy at `now`, in tenths (a tenth at least; task #1091), once its run
/// says it: the more of the cores it kept busy since its lease's first beat and over its last minute
/// or so of beats (`Lease::cores_mean`, `Lease::cores_lately`: a job busy late counts for its busy
/// stretch within two minutes, one quiet a moment between busy stretches for its mean), once it
/// has ended a target (one read and computed) or run past the time its targets are predicted to
/// take its Mac (`secs`; none known, `OWN_AGE`). Before, or with no CPU time said (an older
/// agent's), None: it counts for its measure (`need_cores`), so a job still reading its first
/// target isn't counted as using nothing. A step's measure is its busiest thread count's over the
/// window's runs (crate::uses): on 10 Nov it counted the M4's terrain-deep jobs at 3.8 cores each,
/// from runs at 5 threads two days before, while they kept 2.2–3.5 busy at 4.
fn busy_now(l: &Lease, secs: Option<u64>, now: Instant) -> Option<u32> {
    let started = l.live.as_ref().is_some_and(|v| v.targets_done > 0)
        || l.ran(now).as_secs() > secs.unwrap_or(crate::budget::nas::OWN_AGE.as_secs());
    if !started {
        return None;
    }
    let cores = l.cores_mean()?.max(l.cores_lately().unwrap_or(0.0));
    Some(((cores * 10.0).round() as u32).max(1))
}

/// The threads a job wanting `threads` and counting for `busy` tenths of a core is granted where
/// `left` tenths are left by the rest of its Mac's work: within the cores left and the threads it
/// leaves idle (its threads less its busy cores, whole: they add no load), one at least. A grant's
/// (`Sched::pack`) and a re-grant's (`Sched::regrant`).
fn fit_threads(threads: u32, busy: u32, left: u32) -> u32 {
    let idle_ok = threads.min((left + (threads * 10).saturating_sub(busy)) / 10);
    threads.min((left / 10).max(idle_ok)).max(1)
}

/// The whole cores `t` tenths of a core make, to the nearest (a Mac's jobs' busy cores summed
/// before rounding, task #697: beside its busy cores' whole cores).
fn tenths_to_cores(t: u32) -> u32 {
    (t + 5) / 10
}

/// `t` tenths of a core in words: "1.2".
fn tenths(t: u32) -> String {
    format!("{}.{}", t / 10, t % 10)
}

/// How long a Mac's load is given to show its last grant before it's granted a job past the cores
/// its jobs' threads leave (`Sched::pack`, task #688 review): the minute its busy cores span.
pub const SETTLE: Duration = Duration::from_secs(60);

/// The steps that load the NAS as they go (reads; the clean-up's deletes, task #1000): one none of
/// whose runs the way it runs now measured its rate weighs the NAS budget's whole (`NasWeigher`).
pub(super) const NAS_STEPS: &[&str] = &["unit", "pack", "lo", "gc"];

/// The batch length while the pool's setting `batch-minutes` isn't set (s; docs/pool.md §7.7, Batch
/// length): a quarter of an hour, the owner's choice (`Sched::batch_len`).
pub const JOB_S: u64 = 15 * 60;

/// The most targets a job so offered takes, whatever their times (a step measured at no time).
pub(super) const JOB_MAX: usize = 256;

impl Sched {
    /// The 90th percentile of the disk `step`'s last jobs took (MB; 0: none seen).
    pub(super) fn job_p90(&self, step: &str) -> u64 {
        self.job_disks.get(step).and_then(|v| crate::needs::p90(v.clone())).unwrap_or(0)
    }

    /// The disk a job of `step` takes at least, whatever its targets (MB): its last jobs' 90th
    /// percentile; 0 for a step reading a shared cache, whose jobs' runs measure its fill (`fills`,
    /// reserved where a Mac lacks it) and its targets' own output (`measured`).
    pub(super) fn job_floor(&self, step: &str) -> u64 {
        if crate::needs::cache_of(step).is_some() {
            0
        } else {
            self.job_p90(step)
        }
    }

    /// What filling shared cache `cache` (crate::needs::CACHES) on a Mac takes (MB; 0: never
    /// measured).
    pub(super) fn fill_mb(&self, cache: &str) -> u64 {
        self.fills.get(cache).copied().unwrap_or(0)
    }

    /// What of shared cache `cache` the Mac asking `a` lacks (MB): its fill less what its capacity
    /// says its cache folder holds of it; the whole fill when its ask says nothing of its caches
    /// (an older agent's). Nothing on a Mac whose room past its target is short of what it lacks:
    /// a shared cache is copied only where there's room (crate::localread), its jobs there reading
    /// the NAS in place.
    pub(super) fn missing(&self, cache: &str, a: &Ask) -> u64 {
        let have = a.cap.as_ref().and_then(|c| c.caches.get(cache)).map_or(0, |h| h.have_mb);
        let lacks = self.fill_mb(cache).saturating_sub(have);
        if a.disk_mb.is_some_and(|d| d < lacks) {
            return 0;
        }
        lacks
    }

    /// The floor of `target` of `step` that holds it off a Mac: one learned alone, the way its step
    /// runs now (crate::coord::cost_version).
    pub(super) fn holding_floor(&self, step: &str, target: &str) -> Option<Floor> {
        self.floors.get(&cost_key(step, target)).filter(|f| f.v >= self.the_plan.cost_version(step)).copied()
    }

    /// The memory a job of `step` for `target` is expected to take (MB), as `job_peak` sizes it from
    /// its costs and `size`, and never below its floor (one of an older way of its step too: the best
    /// estimate there is until it runs again).
    pub(super) fn peak_of(&self, step: &str, target: &str, size: u64, threads: u32) -> u64 {
        self.book()
            .mem(
                step,
                target,
                threads,
                first_guess(self.the_plan.cost_version("unit"), &self.costs, step, target, size),
            )
            .0
    }

    /// The measures crate::needs reads (the costs, the floors and the disk floors), its models kept
    /// in `needs` until they change (`needs_rev`; their counts too, for a change made in place).
    pub(super) fn book(&self) -> crate::needs::Book<'_> {
        crate::needs::Book::kept(self.the_plan, &self.costs, &self.floors, &self.disks, &self.needs, self.rev())
            .paced(&self.paces)
    }

    /// Where the measures stand: it moves with each cost, floor or disk floor taken or cleared
    /// (`needs_rev`, and their counts for a change made in place).
    fn rev(&self) -> u64 {
        self.needs_rev.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ ((self.costs.len() as u64) << 40 | (self.floors.len() as u64) << 20 | self.disks.len() as u64)
            ^ (self.job_disks.len() as u64).rotate_left(60)
    }

    /// The first guess of offered `target` of `step` (`first_guess` by the size it was offered
    /// with), 0 when it isn't offered: for a step offered by an input measure
    /// (crate::needs::OFFERED_BY), that measure, which crate::needs::Book::mem reads as such.
    pub(super) fn offered_guess(&self, step: &str, target: &str) -> u64 {
        self.offered_size(step, target)
            .map_or(0, |size| first_guess(self.the_plan.cost_version("unit"), &self.costs, step, target, size))
    }

    /// The memory (MB) offered `target` of `step` was guessed at (`offered_guess`): none for a step
    /// offered by an input measure (crate::needs::OFFERED_BY) or not offered.
    fn offered_mb(&self, step: &str, target: &str) -> u64 {
        if crate::needs::offered_by(step).is_some() {
            return 0;
        }
        self.offered_guess(step, target)
    }

    /// The size `target` of `step` was first offered with; None when it isn't offered.
    pub(super) fn offered_size(&self, step: &str, target: &str) -> Option<u64> {
        self.offered.get(step)?.get(target).copied()
    }

    /// `step`'s work done under a key the plan hasn't shown yet (`done`): target → key.
    fn done_of(&self, step: &str) -> std::collections::HashMap<&str, &str> {
        let from = (step.to_string(), String::new());
        self.done
            .range(from..)
            .take_while(|((st, _), _)| st == step)
            .map(|((_, t), k)| (t.as_str(), k.as_str()))
            .collect()
    }

    /// Whether `worker` failed any target (`failed`): its first in the map's order, by worker.
    pub(super) fn failed_any(&self, worker: &str) -> bool {
        self.failed.range((worker.to_string(), String::new())..).next().is_some_and(|((w, _), _)| w == worker)
    }

    /// Whether any target of `step` is done (`done`): its first in the map's order, by step.
    pub(super) fn done_any(&self, step: &str) -> bool {
        self.done.range((step.to_string(), String::new())..).next().is_some_and(|((s, _), _)| s == step)
    }

    /// The plan's offers, and their index (`offered`).
    pub(super) fn set_offers(&mut self, offers: Vec<Offer>) {
        self.offers = offers;
        self.offered.clear();
        self.drops.clear();
        self.measured_of.lock().unwrap_or_else(|e| e.into_inner()).1.clear();
        self.provisional_at.lock().unwrap_or_else(|e| e.into_inner()).1.clear();
        self.kept_leases.lock().unwrap_or_else(|e| e.into_inner()).1.clear();
        for o in &self.offers {
            let of = self.offered.entry(o.step.clone()).or_default();
            for t in &o.targets {
                of.entry(t.0.clone()).or_insert(t.2);
                if self.the_plan.is_none_key(&o.step, &t.0, &t.1) {
                    self.drops.insert((o.step.clone(), t.0.clone()));
                }
            }
        }
    }

    /// `Coordinator::measured`, under its lock: its step's model's (crate::needs) memory and disk (a
    /// z6 tile's or candidates' extract-size guess first while their fit isn't good, by the size it was
    /// offered with); None while nothing of its step was measured. Floors of an older way of the
    /// step count for nothing (crate::needs).
    pub(super) fn measured(&self, step: &str, target: &str) -> Option<Measured> {
        self.measures(step, target).1
    }

    /// `target` of `step`'s memory by the book (a step never measured: its offered guess, `lease_mem`)
    /// and `measured`; kept while the measures stand (`measured_of`).
    fn measures(&self, step: &str, target: &str) -> (u64, Option<Measured>) {
        let rev = self.rev();
        let mut g = self.measured_of.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != rev {
            *g = (rev, Measures::default());
        }
        if let Some(&m) = g.1.get(step).and_then(|t| t.get(target)) {
            return m;
        }
        let b = self.book();
        // (A drop not run yet: its step's drops', `drop_of`.)
        if let Some((_, mem_mb)) = self.drop_of(&b, step, target) {
            let m = Measured { mem_mb, disk_mb: 0, own: false };
            g.1.entry(step.to_string()).or_default().insert(target.to_string(), (mem_mb, Some(m)));
            return (mem_mb, Some(m));
        }
        let (mem_mb, from) = b.mem(step, target, 0, self.offered_guess(step, target));
        let m = (from != crate::needs::From::Table).then(|| Measured {
            mem_mb,
            disk_mb: b.disk(step, target, 0).unwrap_or(0),
            own: from == crate::needs::From::Measured,
        });
        g.1.entry(step.to_string()).or_default().insert(target.to_string(), (mem_mb, m));
        (mem_mb, m)
    }

    /// Each Mac's pace at each step now (`paces`): the models made again when they changed.
    pub fn set_paces(&mut self, p: crate::paces::Paces) {
        if p != self.paces {
            self.paces = p;
            self.needs_rev += 1;
        }
    }

    /// How long `target` of `step` is measured to take `worker` (s), as it ran its threads
    /// (`secs_on`).
    pub(super) fn secs_of(&self, step: &str, target: &str, worker: &str) -> Option<u64> {
        self.secs_on(step, target, worker, 0).map(|s| s.0)
    }

    /// How long `target` of `step` is measured to take `worker` on `threads` (s; 0: as it ran), and
    /// whether by its Mac's own runs of the step: a drop its own run hasn't measured, its step's
    /// drops' (`drop_of`); else crate::needs::Book::secs_on (its own run at the worker's Mac's
    /// pace, its source's bin of the step's runs, its step's model; a Mac with no runs of the step,
    /// as before task #927); None when none of its step's has run the way it runs now.
    pub(super) fn secs_on(&self, step: &str, target: &str, worker: &str, threads: u32) -> Option<(u64, bool)> {
        let b = self.book();
        match self.drop_of(&b, step, target) {
            Some(d) => Some((d.0, true)),
            None => b.secs_on(step, target, threads, Some(worker)),
        }
    }

    /// How long `target` of `step` takes on `worker` at `threads`, at most as measured (s): its own
    /// run's (`secs_on`, its Mac's pace and threads), a drop's its step's drops'; a target never run
    /// itself, the larger of that and its step's runs' 90th percentile at the worker's pace
    /// (crate::needs::Book::secs_p90: a long-tailed step's big target unmeasured runs past its
    /// step's mean healthy, trees on 6 Nov, task #978). A job's stillness can't outlast its
    /// target's time while it works (a target's start and end each move it): the stuck flag's
    /// usual for a step none of whose jobs measured its stillness yet (`Coordinator::runs`). None
    /// when none of its step's has run the way it runs now.
    pub(super) fn target_most(&self, step: &str, target: &str, worker: &str, threads: u32) -> Option<u64> {
        let (s, _) = self.secs_on(step, target, worker, threads)?;
        let own = self.costs.get(&cost_key(step, target)).is_some_and(|c| c.v >= self.the_plan.cost_version(step));
        if own || self.drops.contains(&(step.to_string(), target.to_string())) {
            return Some(s);
        }
        Some(s.max(self.book().secs_p90(step, target, threads, Some(worker)).unwrap_or(0)))
    }

    /// The threads a job of `step` starting with `first` is sized at on `worker`'s Mac of `total`
    /// cores: the grant rule's (`lease_cores`) for it alone.
    pub fn sizing_threads(&self, worker: &str, step: &str, first: &str, total: u32) -> u32 {
        self.lease_cores(worker, step, &[first.to_string()], total).0
    }

    /// The batch length (s): the pool's setting `batch-minutes` (`batch_s`), else `JOB_S`.
    pub fn batch_len(&self) -> u64 {
        if self.batch_s > 0 {
            self.batch_s
        } else {
            JOB_S
        }
    }

    /// How long a job of `step` over `targets` is measured to take `worker` on `threads` (s): its
    /// Mac's jobs' start at the step (crate::paces::Paces::start) and its targets' times (`secs_on`)
    /// summed; and whether by its Mac's own runs (every target's). None when a target has none.
    /// What a job is sized by (`pick_every`, the same sum), a leased job's predicted time
    /// (`job_weight`) and the replay's (crate::agent::fccheck::batches).
    pub fn batch_secs<'t>(
        &self,
        step: &str,
        targets: impl IntoIterator<Item = &'t str>,
        worker: &str,
        threads: u32,
    ) -> Option<(u64, bool)> {
        let start = self.paces.start(worker, step).round() as u64;
        targets.into_iter().try_fold((start, true), |(s, own), t| {
            let (x, o) = self.secs_on(step, t, worker, threads)?;
            Some((s + x, own && o))
        })
    }

    /// `batch_secs` (as its targets ran) by its step's runs' 90th percentile of their times
    /// (crate::needs::Book::secs_p90, an update's drain's, where a long tail's long jobs must not be
    /// cut short; a target's own run's time where it has one): what a job granted while room is
    /// held for another must end within (`Backfill`). None when a target has none.
    pub(super) fn batch_p90<'t>(
        &self,
        step: &str,
        targets: impl IntoIterator<Item = &'t str>,
        worker: &str,
    ) -> Option<u64> {
        let b = self.book();
        let start = self.paces.start(worker, step).round() as u64;
        // (Never under its time as `batch_secs` has it: a run of another Mac's counting twice there.)
        targets.into_iter().try_fold(start, |s, t| {
            let p90 = b.secs_p90(step, t, 0, Some(worker))?;
            Some(s + p90.max(self.secs_on(step, t, worker, 0).map_or(0, |x| x.0)))
        })
    }

    /// What target `target` of `step` takes as a drop (`drops`) its own run hasn't measured: its
    /// step's drops' time and memory (crate::needs::Book::drop_need); None: no drop, or measured
    /// itself, or none of its step's drops ran yet (its step's model stands).
    fn drop_of(&self, b: &crate::needs::Book, step: &str, target: &str) -> Option<(u64, u64)> {
        if !self.drops.contains(&(step.to_string(), target.to_string())) {
            return None;
        }
        let own = self.costs.get(&cost_key(step, target)).is_some_and(|c| c.v >= self.the_plan.cost_version(step));
        if own {
            return None;
        }
        let rev = self.rev();
        let mut g = self.drop_needs.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != rev {
            *g = (rev, BTreeMap::new());
        }
        *g.1.entry(step.to_string()).or_insert_with(|| b.drop_need(step))
    }

    /// `worker`'s live jobs' leases.
    pub(super) fn job_leases(&self, worker: &str, now: Instant) -> Vec<Lease> {
        self.worker_jobs(worker, now).cloned().collect()
    }

    fn worker_jobs<'s>(&'s self, worker: &'s str, now: Instant) -> impl Iterator<Item = &'s Lease> + 's {
        self.leases.live(now).filter(move |l| l.worker == worker && matches!(l.work, Work::Job { .. }))
    }

    /// Whether `worker` holds a live job's lease.
    fn holds_job(&self, worker: &str, now: Instant) -> bool {
        self.worker_jobs(worker, now).next().is_some()
    }

    /// A job of `step` over `targets` on `worker` (None: at the lead's pace): its NAS weight under `l` (its targets' moves
    /// over their times together, `NasWeigher::job`, crate::budget::nas; 0 for a target none of whose step's runs measured
    /// it; with none, its step's), and whether it
    /// downloads (its step's kind a fetch, crate::agent::steps::Kind::Fetch).
    /// (`nw` keeps the Book and the steps' percentiles for one placement: `NasWeigher`.)
    pub(super) fn job_nas<'t>(
        nw: &NasWeigher,
        step: &str,
        worker: Option<&str>,
        targets: impl IntoIterator<Item = &'t str>,
    ) -> (u32, bool) {
        let mut j = JobLoad::default();
        let mut any = false;
        for t in targets {
            nw.add(&mut j, step, t, worker);
            any = true;
        }
        // (A job of no targets of its own, the clean-up's lease: its step's measured weight, task #451.)
        let w = if any { nw.job(&j) } else { nw.weight(step, "", worker) };
        (w, nw.book.the_plan.fetches(step))
    }

    /// Leased job `l`'s entry in `kept_leases` (made empty where none is of its worker and work, or
    /// the measures moved), handed to `f`.
    fn kept_lease<T>(&self, l: &Lease, f: impl FnOnce(&mut LeaseKept) -> T) -> T {
        let rev = self.rev();
        let mut g = self.kept_leases.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != rev {
            *g = (rev, Default::default());
        }
        let v = g.1.entry(l.id).or_default();
        let k = match v.iter().position(|k| k.worker == l.worker && k.work == l.work) {
            Some(k) => k,
            None => {
                v.push(LeaseKept { worker: l.worker.clone(), work: l.work.clone(), nas: Vec::new(), need: None });
                v.len() - 1
            }
        };
        f(&mut v[k])
    }

    /// `job_nas` of leased job `l` (of `step` over `targets`) on its worker, kept (`kept_leases`):
    /// the NAS budget weighs every job leased for each offer an ask weighs, a job of 30 targets each
    /// time.
    fn lease_nas(&self, nw: &NasWeigher, l: &Lease, step: &str, targets: &[(String, String)]) -> (u32, bool) {
        if let Some(w) = self.kept_lease(l, |k| k.nas.iter().find(|n| n.0 == nw.limits).map(|n| n.1)) {
            return w;
        }
        let got = Self::job_nas(nw, step, Some(&l.worker), targets.iter().map(|t| t.0.as_str()));
        self.kept_lease(l, |k| k.nas.push((nw.limits, got)));
        got
    }

    /// Leased job `l`'s (of `step` over `targets`) packing need on its worker (`pack_need`) and its
    /// measured time there as it ran its threads (`batch_secs`), kept (`kept_leases`): each ask
    /// packs beside every job its Mac holds.
    fn lease_need(&self, l: &Lease, step: &str, targets: &[(String, String)]) -> (Need, Option<u64>) {
        if let Some(n) = self.kept_lease(l, |k| k.need) {
            return n;
        }
        let ts: Vec<String> = targets.iter().map(|t| t.0.clone()).collect();
        let got = (
            self.pack_need(step, &ts, &l.worker),
            self.batch_secs(step, ts.iter().map(String::as_str), &l.worker, 0).map(|s| s.0),
        );
        self.kept_lease(l, |k| k.need = Some(got));
        got
    }

    /// Held job `l` of `step` over `targets` as its Mac's packing counts it at `now`: its need
    /// (`lease_need`) within the threads its lease counts, at the cores it keeps busy now once its
    /// run says it (`busy_now`).
    fn held_need(&self, l: &Lease, step: &str, targets: &[(String, String)], now: Instant) -> Need {
        let (n, secs) = self.lease_need(l, step, targets);
        Need { held: l.threads, busy_now: busy_now(l, secs, now), ..n }
    }

    /// What leased job `l` weighs of the NAS budget now, and whether it downloads (`job_nas`): its
    /// targets' measured rates, as predicted, until it has run past the time they predict it takes
    /// its worker (`secs_of`, summed; a target of no known time, `OWN_AGE`); then the greater of its
    /// mean rate since its lease's first beat (`Lease::mean`: its job's counts run from its start,
    /// before a lease taken up at a restart or a take-over) and its rate over its last minute or so of beats
    /// (`Lease::lately`), by what its job has moved on the NAS as its worker's beats said
    /// (`Lease::live`; a job quiet early and heavy late is weighed by its heavy stretch within two
    /// minutes, where its mean would lag; one quiet a moment between heavy stretches keeps its
    /// mean). A beat not saying all it moved (an older agent's: bytes alone) leaves it weighed by
    /// its targets. A job running slower than
    /// its targets' runs elsewhere moves their bytes over a longer time: weighed by their rates for
    /// its whole run, one of a step measured at the whole NAS (slope-deep, its targets read in a
    /// few seconds on the other MacBook) kept every other job that reads the NAS from starting while it
    /// ran for most of an hour on a Mac moving a fraction of that (task #925).
    pub(super) fn job_weight(&self, nw: &NasWeigher, l: &Lease, now: Instant) -> (u32, bool) {
        let Work::Job { step, targets } = &l.work else { return (0, false) };
        let (w, dl) = self.lease_nas(nw, l, step, targets);
        if l.live.is_none() {
            return (w, dl);
        }
        let names: Vec<&str> =
            if targets.is_empty() { vec![""] } else { targets.iter().map(|t| t.0.as_str()).collect() };
        // (No time known for a target: `OWN_AGE`, so an unmeasured job is weighed by what it does
        // once it has run that long, never by its step's guess for good.)
        let predicted: u64 = self
            .batch_secs(step, names.iter().copied(), &l.worker, l.threads)
            .map_or(crate::budget::nas::OWN_AGE.as_secs(), |s| s.0);
        if l.ran(now).as_secs() <= predicted {
            return (w, dl);
        }
        match (l.mean(), l.lately()) {
            (None, _) => (w, dl),
            (Some(m), r) => (nw.limits.weight(m).max(r.map_or(0, |r| nw.limits.weight(r))), dl),
        }
    }

    /// The NAS budget now (docs/pool.md §7.7) for an ask of the lead's own (`lead`) or a member's:
    /// its weigher, the NAS weight of every job leased in the pool together, of the members' among
    /// them (all but the lead's own), and the downloading jobs; None while it's off for the asker
    /// (the lead's own: only before the NAS was first probed, `nas_own`).
    pub(super) fn nas_room(&self, now: Instant, lead: bool) -> Option<(NasWeigher<'_>, u32, u32, usize)> {
        self.nas_room_heaviest(now, lead).map(|(nw, w, members, dl, _)| (nw, w, members, dl))
    }

    /// `nas_room`, and the most one leased job weighs, of all and of the members' ([all, members]).
    pub(super) fn nas_room_heaviest(
        &self,
        now: Instant,
        lead: bool,
    ) -> Option<(NasWeigher<'_>, u32, u32, usize, [u32; 2])> {
        let limits = if lead { self.nas_own.or(self.nas)? } else { self.nas? };
        let nw = NasWeigher { limits, book: self.book() };
        // (Within one `pack`, the leases change only as it grants: each offer it weighs asks this.)
        let kept = self
            .packing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|p| p.nas.iter().find(|k| k.0 == (now, lead)).map(|k| k.1));
        if let Some((w, members, dl, most)) = kept {
            return Some((nw, w, members, dl, most));
        }
        // (The switch `off`: the members' jobs aren't held, so the lead's own aren't held back by
        // them either, only by its own (task #440: else a member's unheld jobs keep the lead idle).)
        let members_held = self.nas.is_some();
        let (mut w, mut members, mut dl, mut most) = (0u32, 0u32, 0usize, [0u32; 2]);
        for lease in self.leases.live(now) {
            if !members_held && lease.worker != self.lead_name {
                continue;
            }
            if matches!(lease.work, Work::Job { .. }) {
                let (jw, d) = self.job_weight(&nw, lease, now);
                w = w.saturating_add(jw);
                most[0] = most[0].max(jw);
                if lease.worker != self.lead_name {
                    members = members.saturating_add(jw);
                    most[1] = most[1].max(jw);
                }
                dl += usize::from(d);
            }
        }
        if let Some(p) = self.packing.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            p.nas.push(((now, lead), (w, members, dl, most)));
        }
        Some((nw, w, members, dl, most))
    }

    /// `Leases::held` of `step`, kept within one `pack` (`packing`) as nas_room_heaviest is.
    fn held_of(&self, step: &str, now: Instant) -> std::sync::Arc<BTreeSet<String>> {
        let mut g = self.packing.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = g.as_ref().and_then(|p| p.held.get(&(step.to_string(), now))) {
            return h.clone();
        }
        let h = std::sync::Arc::new(self.leases.held(step, now));
        if let Some(p) = g.as_mut() {
            p.held.insert((step.to_string(), now), h.clone());
        }
        h
    }

    /// The room the NAS budget holds for a target aged in it (task #444): of the targets held back
    /// for the budget alone for `OWN_AGE` or more and passed over in the last two minutes, the one
    /// first held back: (step, target, its weight). While it's held, no other job weighing anything
    /// starts unless it leaves that room (`nas_reserve_ok`): the leased jobs end, and it fits. An
    /// aged target always has a way in once no leased job weighs the whole (`fits_job`,
    /// `fits_own_job`), so the room is never held for one no grant would let start (task #603).
    pub(super) fn nas_reserved(&self, nw: &NasWeigher, now: Instant) -> Option<(String, String, u32)> {
        // (One lock at a time: never one inside the other.)
        let kept = self.nas_reserved_at.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let held = match kept {
            Some((at, held)) if at == now => held.map(|h| h.1),
            _ => {
                let aged_by = now.checked_sub(crate::budget::nas::OWN_AGE);
                let fresh_from = now.checked_sub(Duration::from_secs(120));
                let held = self
                    .nas_waits
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    // (Aged `OWN_AGE` and passed over in the last two minutes, as instants compared.)
                    .filter(|(_, (since, last))| {
                        aged_by.is_some_and(|a| *since <= a) && fresh_from.is_none_or(|f| *last > f)
                    })
                    .min_by(|x, y| (x.1 .0, x.0).cmp(&(y.1 .0, y.0)))
                    .map(|(k, (since, _))| (*since, k.clone()));
                *self.nas_reserved_at.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, held.clone()));
                held.map(|h| h.1)
            }
        };
        held.map(|(st, t)| {
            // (At the lead's pace: whoever takes it, it holds the room it would there.)
            let w = nw.weight(&st, &t, None);
            (st, t, w)
        })
    }

    /// Whether a job of `step` over `targets` of weight `w`, beside leased jobs weighing `running`,
    /// leaves the room held for an aged target (`reserved`, `nas_reserved`'s): it weighs nothing,
    /// nothing's held, it's the one held for, or both fit within the whole.
    pub(super) fn nas_reserve_ok(
        reserved: Option<&(String, String, u32)>,
        step: &str,
        targets: &[String],
        running: u32,
        w: u32,
    ) -> bool {
        w == 0
            || reserved.is_none_or(|(st, t, rw)| {
                (st == step && targets.contains(t))
                    || running.saturating_add(w).saturating_add(*rw) <= crate::budget::nas::WHOLE
            })
    }

    /// How many targets were passed over for the NAS budget alone at `now` (`nas_wait`).
    pub(super) fn nas_waits_at(&self, now: Instant) -> usize {
        self.nas_waits.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|w| w.1 == now).count()
    }

    /// Notes `target` of `step` passed over for the NAS budget alone now.
    pub(super) fn nas_wait(&self, step: &str, target: &str, now: Instant) {
        let mut g = self.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
        let e = g.entry((step.to_string(), target.to_string())).and_modify(|e| e.1 = now).or_insert((now, now));
        let since = e.0;
        drop(g);
        let aged = now.saturating_duration_since(since) >= crate::budget::nas::OWN_AGE;
        // (Aged, it holds room now: in the kept one's place when it was held back first.)
        if aged {
            let mut kept = self.nas_reserved_at.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, held)) = kept.as_mut().filter(|(at, _)| *at == now) {
                let me = (since, (step.to_string(), target.to_string()));
                if held.as_ref().is_none_or(|h| (h.0, &h.1) > (me.0, &me.1)) {
                    *held = Some(me);
                }
            }
        }
    }

    /// What a job of `step` over `targets` is measured to take (as `Coordinator::measured_batch`:
    /// the most of their memory, their disks summed or the most a job of the step took); None when
    /// any is a cold start.
    pub(super) fn job_need(&self, step: &str, targets: &[String]) -> Option<Measured> {
        let each: Vec<Measured> = targets.iter().map(|t| self.measured(step, t)).collect::<Option<_>>()?;
        let job = self.job_floor(step);
        Some(Measured {
            mem_mb: each.iter().map(|m| m.mem_mb).max().unwrap_or(0),
            disk_mb: crate::needs::batch_disk(step, each.iter().map(|m| m.disk_mb)).max(job),
            own: each.iter().all(|m| m.own),
        })
    }

    /// What a job of `step` over `targets` on `worker`'s Mac is packed as (task #499): its measure
    /// (`job_need`), else, a cold start (`Need::cold`), its provisional need (`provisional`); its
    /// cores its step's use on the Mac (crate::uses, task #947), else, measured, the cost book's
    /// (`book_cores`).
    pub(super) fn pack_need(&self, step: &str, targets: &[String], worker: &str) -> Need {
        // (A step whose jobs only change build records (task #838): one thread counted for no cores
        // (it waits on the NAS), no disk; its memory measured as any step's (task #979), so once
        // measured it goes beside any jobs, a long one's included.)
        if self.the_plan.records_only(step) {
            let cores = Some(crate::needs::Cores { grant: 1, busy_tenths: Some(0) });
            let (m, cold) = match self.job_need(step, targets) {
                Some(m) => (m, false),
                None => (self.provisional(step, targets).unwrap_or_default(), true),
            };
            let m = Measured { disk_mb: 0, ..m };
            return Need { m, cores, cold, waits: false, idle: false, by_use: false, held: 0, busy_now: None };
        }
        let used = self.uses.cores(step, crate::paces::mac_of(worker));
        let by_use = used.is_some();
        match self.job_need(step, targets) {
            Some(m) => {
                let cores = used.or_else(|| book_cores(&self.book(), step, targets));
                Need { m, cores, cold: false, waits: false, idle: false, by_use, held: 0, busy_now: None }
            }
            // (No figure (`no_figure`): it runs alone, so it's counted at none.)
            None => Need {
                m: self.provisional(step, targets).unwrap_or_default(),
                cores: used,
                cold: true,
                waits: false,
                idle: false,
                by_use,
                held: 0,
                busy_now: None,
            },
        }
    }

    /// The threads a job of `step` over `targets` on `worker`'s Mac, of `total` cores for its jobs
    /// (`ask_cores`: the in-use share's taken off), is granted, and the cores it counts for there in
    /// tenths: `need_cores` of its `pack_need`. The grant rule in one place: the live scheduler's
    /// (`pack`), the forecast's (`pack` on this scheduler as recorded, `State`), and a running
    /// job's, decided again (task #948). What the job would be granted alone: the cores the Mac's
    /// other jobs count for aren't taken off (the caller's). Its step's use groups runs by the
    /// threads their record says (crate::uses): a run whose threads change as it goes must say
    /// what it held over its run, or its use is read at the wrong threads.
    pub fn lease_cores(&self, worker: &str, step: &str, targets: &[String], total: u32) -> (u32, u32) {
        self.need_cores(step, &self.pack_need(step, targets, worker), total)
    }

    /// The runs of the build's timings in the window (a coordinator's log as it starts,
    /// crate::uses::Run::read_log): each step's use on each Mac from them.
    pub fn set_use_runs(&mut self, runs: Vec<crate::uses::Run>) {
        self.use_runs = runs;
        self.uses = crate::uses::Uses::of(&self.use_runs);
        self.kept_leases.lock().unwrap_or_else(|e| e.into_inner()).1.clear();
    }

    /// A job's timings `rec` come in at `now` (unix s): its runs among the uses' (crate::uses: one
    /// for each thread count it held), those ended before the window let go.
    pub fn add_use(&mut self, rec: &crate::timings::RunRec, now: u64) {
        let runs = crate::uses::Run::of(self.the_plan, rec);
        if runs.is_empty() {
            return;
        }
        self.use_runs.retain(|r| crate::uses::in_window(r, now));
        self.use_runs.extend(runs);
        self.uses = crate::uses::Uses::of(&self.use_runs);
        self.kept_leases.lock().unwrap_or_else(|e| e.into_inner()).1.clear();
    }

    /// The threads a job packed as `n` is granted on a Mac of `total` cores, and the cores it counts
    /// for there, in tenths of a core (task #688; tenths, task #697: a job's busy cores rounded to the
    /// nearest undercounted up to half a core each). Measured: its grant (its step's use on the Mac,
    /// crate::uses: the most cores its runs there kept busy over their whole runs, rounded up, a
    /// thread more where they needed every thread; none there, crate::needs::Book::cores: the cores
    /// it computed on and a thread more) within the Mac's cores, counted for the cores it kept busy
    /// (`busy_tenths`, a climb's thread among them; its threads where its runs said no CPU time). No
    /// fixed share caps or floors a measured job (task #888: the M4's terrain jobs were held to 3 of
    /// 14 cores, 2.4 of them busy, while the Mac sat at a load of 4; task #896: its runs, not a
    /// share, say what it uses). Not said (no run said its threads), or a cold start of a step
    /// honouring its threads (`THREADS_HONOURED`), a thread both (task #896: its run measures it,
    /// and climbs from there); another cold start with no use on the Mac the whole Mac both (task
    /// #499 review: one that runs as many threads as the Mac has would swamp the jobs beside it).
    /// One only waiting on a worker, or leaving its cores idle, counts for none. A job held counts for
    /// no more than the threads its lease counts (`Need::held`: task #948, a job granted 3 of 14
    /// threads beside others counted for its measured 13 kept its Mac looking full after they ended).
    pub(super) fn need_cores(&self, step: &str, n: &Need, total: u32) -> (u32, u32) {
        let (threads, busy) = self.need_cores_as_granted(step, n, total);
        match n.held {
            0 => (threads, busy),
            h => (threads.min(h), busy.min(h * 10)),
        }
    }

    /// `need_cores` as a grant would have it now, whatever its lease counts.
    fn need_cores_as_granted(&self, step: &str, n: &Need, total: u32) -> (u32, u32) {
        let (threads, busy) = if self.the_plan.records_only(step) {
            (1, 0)
        } else if n.cold && !n.by_use && !THREADS_HONOURED.contains(&step) {
            (total, total * 10)
        } else {
            match n.cores {
                Some(c) => {
                    let threads = c.grant.min(total).max(1);
                    (threads, c.busy_tenths.unwrap_or(threads * 10))
                }
                None => (1, 10),
            }
        };
        let busy = n.busy_now.unwrap_or(busy);
        (threads, if n.waits || n.idle { 0 } else { busy.min(threads * 10) })
    }

    /// Whether no run of `step` has left a measure of any kind, whatever the way it ran (task #499
    /// review): no cost, floor, disk floor or job's disk. A step that has run before (its memory's
    /// measure gone with a new way or a cleared floor) isn't; one never measured runs alone,
    /// nothing beside it, as before task #499.
    pub fn never_measured(&self, step: &str) -> bool {
        // (The steps measured kept while the measures stand, `rev`: an ask asks it for every job.)
        let mut g = self.measured_steps.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != self.rev() || g.1.is_empty() {
            let of = |k: &String| crate::agent::memguard::step_of_key(k).to_string();
            let mut steps: BTreeSet<String> =
                self.costs.keys().chain(self.floors.keys()).chain(self.disks.keys()).map(of).collect();
            steps.extend(self.job_disks.keys().cloned());
            *g = (self.rev(), steps);
        }
        !g.1.contains(step)
    }

    /// A cold start's provisional need (task #499): `provisional_mb` of the most, over its targets,
    /// its step's runs of its latest earlier way measured (crate::needs::Book::older_mem: a cost
    /// version raised, task #979) or it was offered with (`offered_mb`), the disk its step's
    /// measures say (none: 0). None when neither says anything: no figure, so it runs alone
    /// (`no_figure`). It goes beside other jobs when that fits; the memory guard
    /// (crate::agent::memguard) stops one that overruns, and its floor is learned.
    pub(super) fn provisional(&self, step: &str, targets: &[String]) -> Option<Measured> {
        let each: Vec<(u64, Option<u64>)> = targets.iter().map(|t| self.provisional_of(step, t)).collect();
        let mem = each.iter().map(|e| e.0).max().filter(|&mb| mb > 0)?;
        Some(Measured {
            mem_mb: provisional_mb(mem),
            disk_mb: each.iter().filter_map(|e| e.1).max().unwrap_or(0),
            own: false,
        })
    }

    /// `provisional`'s look at `target` of `step`: its runs' memory of their latest earlier way
    /// or the memory it was offered with, the larger, and its disk by the measures; kept while
    /// the measures stand and as offered (`provisional_of`: an ask weighs each cold target it may
    /// grant).
    fn provisional_of(&self, step: &str, target: &str) -> (u64, Option<u64>) {
        let rev = self.rev();
        {
            let mut g = self.provisional_at.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != rev {
                *g = (rev, Default::default());
            }
            if let Some(&p) = g.1.get(step).and_then(|t| t.get(target)) {
                return p;
            }
        }
        let b = self.book();
        let p = (b.older_mem(step, target).unwrap_or(0).max(self.offered_mb(step, target)), b.disk(step, target, 0));
        let mut g = self.provisional_at.lock().unwrap_or_else(|e| e.into_inner());
        g.1.entry(step.to_string()).or_default().insert(target.to_string(), p);
        p
    }

    /// Whether a cold start of `step` over `targets` has no figure to go beside other jobs by: its
    /// step never measured in any way (`never_measured`), or no provisional need (`provisional`).
    /// Such a job runs alone, nothing beside it, as before task #499.
    pub(super) fn no_figure(&self, step: &str, targets: &[String]) -> bool {
        self.never_measured(step) || self.provisional(step, targets).is_none()
    }

    /// Whether a cold start of `step` over `targets` may go to `a`'s worker (task #499): its
    /// provisional need fits the room `a` spares, or the worker holds no job (one bigger than its
    /// whole room, or with no figure, goes alone, as every cold start did before: the guard learns
    /// its floor).
    pub(super) fn cold_fits(&self, step: &str, targets: &[String], a: &Ask, now: Instant) -> bool {
        let fits = |m: Measured| m.mem_mb <= a.mem_mb && a.disk_mb.is_none_or(|d| m.disk_mb <= d);
        self.provisional(step, targets).is_some_and(fits) || !self.holds_job(&a.worker, now)
    }

    /// The disk a job of `step` reserves for its shared cache (crate::needs::CACHES) on the Mac
    /// asking `a`, whose jobs read `caches` already: what of the cache it lacks (`missing`) when
    /// it's the first there to read it, else none (MB).
    pub(super) fn cache_fill(&self, step: &str, caches: &BTreeSet<&'static str>, a: &Ask) -> u64 {
        crate::needs::cache_of(step).filter(|c| !caches.contains(c)).map_or(0, |c| self.missing(c, a))
    }

    /// The jobs `a`'s worker is granted (docs/pool.md §7.2), packed into its room in `offers`' order
    /// (each by `pick_every`, the lead's own planned jobs by `pick_own`, those to `lead` alone:
    /// crate::agent::every::grants), up to the jobs it wants (`Ask::want`, at least one). Each after
    /// the first it holds goes beside the others: measured (memory, disk, cores, crate::needs),
    /// within the room left (the ask's memory and disk, and its Mac's cores, less every job it
    /// holds and those granted in this answer: a lease held counts until it ends or lapses, and
    /// never more than what its jobs hold now leaves); a cold start by its provisional need
    /// (`pack_need`), task #499. Each granted holds its lease from then (`grant`). The first offer
    /// in plan order refused for its room alone (memory, disk or cores: `room_wait`) has that room
    /// held, so later small jobs filling each slot as it opens never starve a big one (task #965):
    /// a later offer is granted only beside it once the jobs it waits for end, or ending before
    /// they're expected to (`Backfill`, task #973), so a hold never idles its Mac for as long as
    /// those jobs last, nor delays the big one past them; held only while it would fit its
    /// Mac once the jobs there end, so a Mac never idles for a job that can't go there (a cold job
    /// of the lead's own: `own_cold_waits`). And why it granted fewer jobs than the ask wanted
    /// (None: as many as it wanted).
    pub fn pack(&mut self, a: &Ask, offers: &[Offer], lead: bool, clock: Clock) -> (Vec<Pick>, Option<String>) {
        *self.packing.lock().unwrap_or_else(|e| e.into_inner()) = Some(Packing::default());
        let got = self.pack_kept(a, offers, lead, clock);
        *self.packing.lock().unwrap_or_else(|e| e.into_inner()) = None;
        got
    }

    /// `pack`, its look-ups of the leases kept while it packs (`packing`).
    fn pack_kept(&mut self, a: &Ask, offers: &[Offer], lead: bool, clock: Clock) -> (Vec<Pick>, Option<String>) {
        let now = clock.now;
        // (Its Mac below its memory floor, the brake on or a give-back cooling it down (docs/pool.md
        // §7.1): nothing, a cold start to a Mac running nothing among it.)
        if let Some(why) = a.cap.as_ref().and_then(|c| c.mem_held_back.as_ref()) {
            return (Vec::new(), Some(format!("memory: {why}")));
        }
        let mine: Vec<&Lease> = self.worker_jobs(&a.worker, now).collect();
        // (A job held: its measure, else its provisional need, `pack_need`; one its Mac says only
        // waits on a worker (`Capacity::waiting`), at what it holds now within that, none of its
        // cores: the room past it free for a job beside it, which yields to it when it goes on
        // (crate::agent's `yielding`); one leaving its Mac's cores idle (`Capacity::idle`, a job
        // waiting on the network), at its measure but none of its cores.)
        let waits =
            |l: &Lease| a.cap.as_ref().and_then(|c| c.waiting.iter().find(|w| w.lease == l.id)).map(|w| w.held_mb);
        let holds =
            |l: &Lease| a.cap.as_ref().and_then(|c| c.held_each.iter().find(|w| w.lease == l.id)).map(|w| w.held_mb);
        let jobs: Vec<(&Lease, &str, &[(String, String)])> = mine
            .iter()
            .filter_map(|&l| match &l.work {
                Work::Job { step, targets } => Some((l, step.as_str(), &targets[..])),
                Work::Task { .. } => None,
            })
            .collect();
        let names = |ts: &[(String, String)]| -> Vec<String> { ts.iter().map(|t| t.0.clone()).collect() };
        let needs: Vec<Need> = jobs
            .iter()
            .map(|&(l, step, targets)| {
                let mut n = self.held_need(l, step, targets, now);
                // (Its re-grants' growth past its measure: `grown_mb`.)
                n.m.mem_mb += self.grown_mb(l, step, targets.iter().map(|t| t.0.as_str()), n.m.mem_mb);
                if let Some(held) = waits(l) {
                    n.m.mem_mb = n.m.mem_mb.min(held);
                    n.waits = true;
                } else if let Some(held) = holds(l) {
                    // (Its growth to come reserved: its expected peak, or what it holds past it.)
                    n.m.mem_mb = n.m.mem_mb.max(held);
                }
                n.idle = a.cap.as_ref().is_some_and(|c| c.idle.contains(&l.id));
                n
            })
            .collect();
        // (A cold start with no figure, its step never measured in any way or nothing to size it
        // by (`no_figure`): nothing beside it, as before task #499.)
        if let Some(st) =
            needs.iter().zip(&jobs).find(|(n, (_, st, ts))| n.cold && self.no_figure(st, &names(ts))).map(|(_, j)| j.1)
        {
            return (Vec::new(), Some(format!("{st} has no measure to size it by: nothing goes beside its first job")));
        }
        // (At most one cold start on a Mac (task #499 review): two unknown needs never meet.)
        let mut cold_here = needs.iter().any(|n| n.cold);
        let total = ask_cores(a);
        // (Cores left, in tenths of a core: each job's, the cores it counts for (`need_cores`).)
        let mut cores = needs
            .iter()
            .zip(&jobs)
            .fold(total * 10, |left, (n, (_, st, _))| left.saturating_sub(self.need_cores(st, n, total).1));
        // (Its Mac's load from outside the pool (other processes: builds, tests): the cores its
        // last minute's CPU keeps busy (`Capacity::cores_free`) past those the jobs it
        // holds count for, less those the in-use share keeps already, come off the cores left
        // (task #440), at most half its cores (task #684): a Mac swamped by other work is granted
        // jobs in the other half only. A job keeping more busy than it was measured to shows here.)
        let ours = tenths_to_cores((total * 10).saturating_sub(cores));
        let outside = outside_cores(a, ours);
        cores = cores.saturating_sub(outside * 10);
        // (The jobs it holds as room held for a waiting job weighs them (`Backfill`): how long each
        // has run, `Lease::ran`.)
        let mut runs: Vec<Run> = jobs
            .iter()
            .zip(&needs)
            .map(|(&(l, st, ts), n)| {
                let secs = self.lease_need(l, st, ts).1;
                Run::of(n.m, self.need_cores(st, n, total).1, secs, l.ran(now).as_secs())
            })
            .collect();
        // (The cores left were its jobs counted at their threads, as before task #688: a grant
        // past them is an overcommit, trusting its busy cores (task #688 review). At most one per
        // ask, and none while its Mac's load hasn't had `SETTLE` to show its last grant or shows
        // load past its jobs' busy cores by more than a core (the minute's whole cores): the jobs go up one at a time, each measured by the load
        // before the next, so jobs busier than their runs said (or #690's reads in parallel) stop
        // the climb within a minute instead of a whole ask's worth landing at once.)
        let mut by_threads = needs
            .iter()
            .zip(&jobs)
            .fold(total, |left, (n, (_, st, _))| {
                left.saturating_sub(if n.waits || n.idle { 0 } else { self.need_cores(st, n, total).0 })
            })
            .saturating_sub(outside);
        let settled = mine.iter().all(|l| now.saturating_duration_since(l.granted) >= SETTLE);
        let unexplained = a.cap.as_ref().is_some_and(|c| {
            let kept = if c.in_use { c.kept_cores } else { 0 };
            c.cores.saturating_sub(c.cores_free).saturating_sub(kept) > ours + 1
        });
        let mut over_ok = settled && !unexplained;
        // (The ask's memory and disk are its Mac's whole room for jobs, whatever it runs (the
        // memory its owner gives the build, the agent's `build_room`; its free disk): every job
        // it holds is taken off its memory here. Its disk is taken off already for the jobs its agent reserves for (its capacity's
        // leases: the disk said is the free space less their reservations); the others it holds
        // (granted since it asked, or an older agent's) are taken off here.)
        let reserved_by_agent = |l: &Lease| a.cap.as_ref().is_some_and(|c| c.leases.contains(&l.id));
        let mut room = a.clone();
        for ((l, _, _), n) in jobs.iter().zip(&needs) {
            room.mem_mb = room.mem_mb.saturating_sub(n.m.mem_mb);
            if !reserved_by_agent(l) {
                room.disk_mb = room.disk_mb.map(|d| d.saturating_sub(n.m.disk_mb));
            }
        }
        // (And never more than what its jobs hold now, as its memory guard last sampled them
        // (`Capacity::held_mb`), leaves: a cold start past its provisional need is counted at what
        // it holds as soon as it's sampled, task #499.)
        if let Some(c) = a.cap.as_ref().filter(|_| !mine.is_empty()) {
            room.mem_mb = room.mem_mb.min(a.mem_mb.saturating_sub(c.held_mb));
        }
        // (The shared caches its Mac's jobs read (crate::needs::CACHES), each reserved once: with
        // the first job granted reading it, in its grant's disk. One no job its agent reserves for
        // holds is taken off here.)
        let mut caches: BTreeSet<&'static str> = BTreeSet::new();
        for (_, st, _) in &jobs {
            if let Some(c) = crate::needs::cache_of(st) {
                if caches.insert(c)
                    && !mine.iter().any(|l| {
                        reserved_by_agent(l)
                            && matches!(&l.work, Work::Job { step, .. } if crate::needs::cache_of(step) == Some(c))
                    })
                {
                    room.disk_mb = room.disk_mb.map(|d| d.saturating_sub(self.missing(c, a)));
                }
            }
        }
        // (The jobs it may be granted: one to an ask without a capacity (an older app's), and to
        // every ask while any member around is on one (`old_member`); else as many as it wants.)
        let old = if a.cap.is_none() { Some(a.worker.clone()) } else { self.old_member(now) };
        let want = if old.is_some() { 1 } else { a.want.max(1) };
        // (A cold job of the lead's own that can't go beside its jobs (`own_cold_waits`): nothing
        // more until they end; then it, before any other work, so the work offered doesn't starve
        // it. Once they've ended, if it still can't be granted (its disk, say), the other offers
        // go: the lead never sits idle for it.)
        let mut cold_own: Vec<Offer> = if lead {
            offers
                .iter()
                .find(|o| {
                    crate::agent::every::grants(self.the_plan, &o.step, true, lead)
                        && self.own_cold_waits(o, a, room.mem_mb, cold_here, now)
                })
                .cloned()
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        if let Some(o) = cold_own.first() {
            if !mine.is_empty() {
                return (Vec::new(), Some(format!("a cold job of {} waits for the lead's jobs to end", o.step)));
            }
            let fill = self.cache_fill(&o.step, &caches, a);
            let room_o = Ask { disk_mb: room.disk_mb.map(|d| d.saturating_sub(fill)), ..room.clone() };
            if self.pick_own(o, &room_o, now).is_empty() {
                cold_own.clear();
            }
        }
        let offers = if cold_own.is_empty() { offers } else { &cold_own[..] };
        let mut held = mine.len();
        let mut grants: Vec<Pick> = Vec::new();
        // (Why the last round granted nothing: a job open but short of cores, or nothing open in
        // the room left.)
        let mut why: Option<String> = None;
        while grants.len() < want {
            let mut short: Option<(String, u32)> = None;
            let mut cold_why: Option<String> = None;
            let mut ramp_why: Option<String> = None;
            let beside = held > 0;
            let mut got = None;
            // (The first offer in plan order refused for its room alone (`room_wait`, or short of
            // cores below): the room it waits for is held for it, so jobs of later offers that fit
            // each slot as it opens don't starve it (task #965: a 4.4 GB job, 2 threads, behind the
            // 3D buildings' small jobs); a later one goes only beside it or ending
            // before its room frees (`Backfill`, task #973: the mini ran one 40-minute-old
            // terrain-hi job, 7 of its 12 cores idle, while room was held for timetables-todo).)
            let mut waiting: Option<Backfill> = None;
            let mut held_why: Option<String> = None;
            for o in offers {
                if !crate::agent::every::grants(self.the_plan, &o.step, o.job.is_some(), lead) {
                    continue;
                }
                // (A job reading a shared cache none here reads yet: its room less the cache's.)
                let fill = self.cache_fill(&o.step, &caches, a);
                // (Copied only when its cache's fill changes it: an ask weighs every offer.)
                let room_o = match room.disk_mb {
                    Some(d) if fill > 0 => {
                        std::borrow::Cow::Owned(Ask { disk_mb: Some(d.saturating_sub(fill)), ..room.clone() })
                    }
                    _ => std::borrow::Cow::Borrowed(&room),
                };
                let pick =
                    if o.job.is_some() { self.pick_own(o, &room_o, now) } else { self.pick_every(o, &room_o, now) };
                if pick.is_empty() {
                    if waiting.is_none() {
                        let whole = match a.disk_mb {
                            Some(d) if fill > 0 => {
                                std::borrow::Cow::Owned(Ask { disk_mb: Some(d.saturating_sub(fill)), ..a.clone() })
                            }
                            _ => std::borrow::Cow::Borrowed(a),
                        };
                        waiting = self.room_wait(o, &whole, &room_o, now).map(|(m, ts)| {
                            let n = self.pack_need(&o.step, &ts, &a.worker);
                            let w = RoomWait {
                                step: o.step.clone(),
                                mem_mb: m.mem_mb,
                                disk_mb: m.disk_mb + fill,
                                cores: self.need_cores(&o.step, &n, total).1,
                            };
                            Backfill::of(w, &room, cores, &runs)
                        });
                    }
                    continue;
                }
                let ts: Vec<String> = pick.iter().map(|t| t.0.clone()).collect();
                let n = self.pack_need(&o.step, &ts, &a.worker);
                if beside && n.cold && (cold_here || self.no_figure(&o.step, &ts)) {
                    cold_why.get_or_insert_with(|| {
                        if cold_here {
                            format!("{} isn't measured: one cold start on a Mac at a time", o.step)
                        } else {
                            format!(
                                "{} has no measure to size it by: its first job goes only to a Mac running nothing",
                                o.step
                            )
                        }
                    });
                    continue;
                }
                let (need_threads, need) = self.need_cores(&o.step, &n, total);
                if beside && need > cores {
                    if short.is_none() {
                        short = Some((o.step.clone(), need));
                    }
                    // (Held for only while its Mac's cores, less those busy outside the pool, take
                    // it once its jobs end: never for one that can't go here.)
                    if waiting.is_none() && need <= total.saturating_sub(outside) * 10 {
                        let w = RoomWait {
                            step: o.step.clone(),
                            mem_mb: n.m.mem_mb,
                            disk_mb: n.m.disk_mb + fill,
                            cores: need,
                        };
                        waiting = Some(Backfill::of(w, &room, cores, &runs));
                    }
                    continue;
                }
                let p90 = || self.batch_p90(&o.step, ts.iter().map(String::as_str), &a.worker);
                if let Some(w) = waiting.as_ref().filter(|w| !w.lets(&n.m, fill, need, p90)) {
                    held_why.get_or_insert_with(|| w.words(&room, cores));
                    continue;
                }
                // (One counted for no cores (a records-only step's, task #838) adds no load to wait
                // for.)
                if beside && need > 0 && need_threads > by_threads && !over_ok {
                    if ramp_why.is_none() {
                        ramp_why = Some(format!(
                            "cores: {} counted at their threads; a job of {} past them waits for the load to show the last ({}s)",
                            by_threads,
                            o.step,
                            SETTLE.as_secs()
                        ));
                    }
                    continue;
                }
                got = Some((o, pick, n));
                break;
            }
            let Some((o, pick, n)) = got else {
                let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
                why = Some(match short {
                    _ if held_why.is_some() => held_why.unwrap_or_default(),
                    None if cold_why.is_some() => cold_why.unwrap_or_default(),
                    None if ramp_why.is_some() => ramp_why.unwrap_or_default(),
                    Some((st, c)) if outside > 0 => {
                        format!(
                            "cores: {} of {total} left ({outside} busy outside the pool), a job of {st} takes {}",
                            tenths(cores),
                            tenths(c)
                        )
                    }
                    Some((st, c)) => {
                        format!("cores: {} of {total} left, a job of {st} takes {}", tenths(cores), tenths(c))
                    }
                    None => format!(
                        "{}nothing {}offered fits the room left (memory: {}{}) or is free to take",
                        // (Targets it would take passed over for the NAS budget alone in this
                        // answer (`nas_wait`): said first, so a wait for the NAS doesn't read as
                        // one for memory (task #931: on 4 Nov every slope-deep job weighed the
                        // whole NAS, and the Macs idle beside one said only the room left).)
                        // (Those fit the room: so nothing else does.)
                        match self.nas_waits_at(now) {
                            0 => String::new(),
                            n => format!(
                                "the NAS budget: {n} target{} it would take wait for the NAS's room; ",
                                if n == 1 { "" } else { "s" }
                            ),
                        },
                        if self.nas_waits_at(now) == 0 { "" } else { "else " },
                        match a.cap.as_ref() {
                            Some(c) => mem_words(c, room.mem_mb),
                            None => gb(room.mem_mb),
                        },
                        room.disk_mb
                            .map(|d| match a.cap.as_ref().filter(|c| c.free_mb > 0) {
                                Some(c) => format!("; disk: {}", disk_words(c, d)),
                                None => format!(", {} disk", gb(d)),
                            })
                            .unwrap_or_default()
                    ),
                });
                break;
            };
            // (Its threads (`need_cores`) within the cores left and the threads it leaves idle, one
            // at least; and the cores it counts for.)
            let (threads, busy) = self.need_cores(&o.step, &n, total);
            let threads = fit_threads(threads, busy, cores);
            // (A planned job of no targets of its own is offered under its id's, keyless, to be
            // measured by it: its lease holds none, as its hand-off names none.)
            let ts: Vec<String> = pick.iter().map(|t| t.0.clone()).collect();
            let pick = if o.job.is_some() { pick.into_iter().filter(|t| !t.1.is_empty()).collect() } else { pick };
            // (A cold start's too: its step unmeasured, its cache's fill may be.)
            let fill = self.cache_fill(&o.step, &caches, a);
            let m = n.m;
            runs.push(self.run_of(&o.step, &ts, &a.worker, Measured { disk_mb: m.disk_mb + fill, ..m }, busy, 0));
            grants.push(self.grant(a, o, pick, threads, m.mem_mb, fill, clock));
            held += 1;
            caches.extend(crate::needs::cache_of(&o.step));
            cold_here |= n.cold;
            room.mem_mb = room.mem_mb.saturating_sub(m.mem_mb);
            room.disk_mb = room.disk_mb.map(|d| d.saturating_sub(m.disk_mb + fill));
            cores = cores.saturating_sub(busy);
            if beside && threads > by_threads {
                over_ok = false;
            }
            by_threads = by_threads.saturating_sub(threads);
        }
        let why = why.filter(|_| grants.len() < want).or_else(|| {
            old.filter(|_| a.want > grants.len() && !grants.is_empty()).map(|w| {
                format!("one job at a time: {w} runs an older app (its asks say no capacity); packing waits for every member on the new one")
            })
        });
        (grants, why)
    }

    /// What a thread of a job of `step` over `targets` adds to its memory at most (MB), and the
    /// threads its measure covers: its measured peak `peak_mb` over the threads the runs that measured
    /// it had (its targets' cost records' `threads`, the fewest among them), their memory in the peak;
    /// the whole peak a thread, and one covered, where a target's aren't said (one more thread of a
    /// regions check once needed about a region's worth, docs/inputs.md §4.3). Never the peak over
    /// the threads a job runs on now: past those it was measured at, that spreads the peak thinner
    /// than any thread of it is.
    fn per_thread_mb<'t>(&self, step: &str, targets: impl IntoIterator<Item = &'t str>, peak_mb: u64) -> (u64, u32) {
        let v = self.the_plan.cost_version(step);
        let measured = targets
            .into_iter()
            .map(|t| {
                let c = self.costs.get(&cost_key(step, t)).filter(|c| c.v >= v)?;
                c.sizes.get("threads").map(|&n| n as u32).filter(|&n| n > 0)
            })
            .collect::<Option<Vec<u32>>>()
            .and_then(|n| n.into_iter().min());
        match measured {
            Some(n) => ((peak_mb / u64::from(n)).max(1), n),
            None => (peak_mb.max(1), 1),
        }
    }

    /// The memory (MB) lease `l`'s job of `step` over `targets`, measured at `peak_mb`, may hold past
    /// its measure: its threads re-granted past those the memory it was admitted with covers (the
    /// threads it started on, `Lease::start_threads`, and those its measure was taken at), each
    /// `per_thread_mb`. Its Mac's agent counts it at its measure alone (crate::agent's `capacity`),
    /// so the lead adds this (`pack`, `regrant`).
    pub(super) fn grown_mb<'t>(
        &self,
        l: &Lease,
        step: &str,
        targets: impl IntoIterator<Item = &'t str>,
        peak_mb: u64,
    ) -> u64 {
        let start = if l.start_threads > 0 { l.start_threads } else { l.threads };
        // (Not re-granted past its start: nothing, whatever its measure covers.)
        if l.threads <= start {
            return 0;
        }
        let (per, covered) = self.per_thread_mb(step, targets, peak_mb);
        per * u64::from(l.threads.saturating_sub(start.max(covered)))
    }

    /// The threads of each running job of `a`'s Mac that takes a re-grant (docs/pool.md §7.1, Threads
    /// re-granted: its capacity's `threads`, each with the threads its target under way runs on and
    /// what it used on them) decided again by the grant rule (task #947): its own run on its threads
    /// now as a run of its step on its Mac, among its step's runs there (crate::uses::Uses::cores:
    /// a cut a trial their runs at fewer threads decide, never under their phases' floor), and where
    /// that climbs, the more of it and the grant a job of its step gets there now (`lease_cores`);
    /// within the cores its Mac has less those the rest of its work counts for and the load outside
    /// the pool (`fit_threads`, a grant's), and the most it can run on (`JobThreads::most`). It
    /// **grows** past what its agent has told it (`JobThreads::granted`) where its Mac's memory
    /// admits work (no brake, floor or cool-down: `Capacity::mem_held_back`) and covers each extra
    /// thread (`per_thread_mb`, within the capacity's `mem_mb` less what its jobs' re-grants have
    /// grown them by already, `grown_mb`); it **shrinks** to fewer only where its Mac's jobs' threads
    /// pass the cores it has for them (its owner back: the in-use share's) or `wanted` (its last ask
    /// was short of cores: other work could use them). A job is decided on at least `SETTLE` of use
    /// on its threads now, and at most once a `SETTLE`. Each lease counts for the most of the threads
    /// it runs on, those its agent has told it and those still to tell (`Leases::regranted`). The
    /// re-grants to tell its Mac: each its agent hasn't told its job yet.
    pub fn regrant(&mut self, a: &Ask, wanted: bool, now: Instant) -> Vec<(u64, u32)> {
        let Some(cap) = a.cap.as_ref().filter(|c| !c.threads.is_empty()) else { return Vec::new() };
        let total = ask_cores(a);
        let mine = self.job_leases(&a.worker, now);
        // (Each job as its Mac's packing counts it: its measure within its lease's threads, none of
        // its cores while it waits on a worker or leaves them idle.)
        let need_of = |l: &Lease| match &l.work {
            Work::Job { step, targets } => {
                let ts: Vec<String> = targets.iter().map(|t| t.0.clone()).collect();
                let idle = cap.idle.contains(&l.id) || cap.waiting.iter().any(|w| w.lease == l.id);
                Some((step.clone(), ts, Need { idle, ..self.held_need(l, step, targets, now) }))
            }
            Work::Task { .. } => None,
        };
        // (Each job's threads as its lease counts them, its measured ones where it says none, and
        // the cores it counts for; and the memory its re-grants have grown it by, which its Mac's
        // memory doesn't count.)
        let mut grown = 0;
        let counted: Vec<(u64, u32, u32)> = mine
            .iter()
            .filter_map(|l| {
                let (st, ts, n) = need_of(l)?;
                grown += self.grown_mb(l, &st, ts.iter().map(String::as_str), n.m.mem_mb);
                let (t, b) = self.need_cores(&st, &n, total);
                Some((
                    l.id,
                    if n.idle {
                        0
                    } else if l.threads > 0 {
                        l.threads
                    } else {
                        t
                    },
                    b,
                ))
            })
            .collect();
        let room_mb = cap.mem_mb.saturating_sub(grown);
        let outside = outside_cores(a, tenths_to_cores(counted.iter().map(|c| c.2).sum()));
        let over = counted.iter().map(|c| c.1).sum::<u32>() + outside > total;
        let mut decided = Vec::new();
        for u in &cap.threads {
            let Some(l) = mine.iter().find(|l| l.id == u.lease) else { continue };
            let Some((step, ts, n)) = need_of(l) else { continue };
            // (What its next target takes as things stand.)
            let next = if u.granted > 0 { u.granted } else { u.threads };
            let settled = l.told.is_none_or(|(_, at)| now.saturating_duration_since(at) >= SETTLE);
            let mut told = None;
            // (Its run so far among its step's runs on its Mac, by their rule: a cut a trial its
            // step's runs at fewer threads decide, never under their phases' floor.)
            let mac = crate::paces::mac_of(&a.worker);
            let live = (settled && u.threads > 0 && u.ran_ms >= SETTLE.as_millis() as u64)
                .then(|| {
                    let run = crate::uses::Run {
                        end: 0,
                        step: step.clone(),
                        mac: mac.to_string(),
                        threads: u.threads,
                        cpu_s: u.cpu_ms as f64 / 1000.0,
                        wall_s: u.ran_ms as f64 / 1000.0,
                        ..Default::default()
                    };
                    let runs = self.use_runs.iter().filter(|r| r.step == step && r.mac == mac).chain([&run]);
                    crate::uses::Uses::of(runs).cores(&step, mac)
                })
                .flatten();
            if let Some(live) = live {
                let want = if live.grant > u.threads {
                    live.grant.max(self.lease_cores(&a.worker, &step, &ts, total).0)
                } else {
                    live.grant
                };
                let others: u32 = counted.iter().filter(|c| c.0 != l.id).map(|c| c.2).sum();
                let left = (total * 10).saturating_sub(others).saturating_sub(outside * 10);
                let mut fit = fit_threads(want, live.busy_tenths.unwrap_or(want * 10), left).min(total);
                if u.most > 0 {
                    fit = fit.min(u.most);
                }
                if fit > next {
                    fit = match &cap.mem_held_back {
                        Some(_) => next,
                        None => {
                            // (Each thread past those whose memory is counted already: its lease's,
                            // and those its grant was admitted with.)
                            let (per, covered) = self.per_thread_mb(&step, ts.iter().map(String::as_str), n.m.mem_mb);
                            let start = if l.start_threads > 0 { l.start_threads } else { l.threads };
                            let paid = l.threads.max(start).max(covered);
                            fit.min(paid.saturating_add(u32::try_from(room_mb / per).unwrap_or(u32::MAX))).max(next)
                        }
                    };
                } else if fit < next && !(over || wanted) {
                    fit = next;
                }
                // (Decided anew: a re-grant not told yet that it now keeps is dropped.)
                if Some(fit) != l.told.map(|t| t.0) {
                    if fit != next {
                        eprintln!(
                            "coordinator: {}'s lease {} ({step}) re-granted {fit} threads at its next target: it runs on {}, {:.1} cores busy",
                            a.worker,
                            l.id,
                            u.threads,
                            u.cpu_ms as f64 / u.ran_ms.max(1) as f64
                        );
                    }
                    told = Some((fit, now));
                }
            }
            decided.push((l.id, u.threads, u.granted, told));
        }
        decided
            .into_iter()
            .filter_map(|(id, on, granted, told)| Some((id, self.leases.regranted(id, on, granted, told)?)))
            .collect()
    }

    /// A job of offer `o`'s step over `pick` granted to `a`'s worker with `threads`, packed at
    /// `mem_mb` (`fill`: the disk its shared cache is reserved, added to its own: `cache_fill`): its
    /// lease, the worker's last target of the step, its targets' waits on the NAS budget let go.
    #[allow(clippy::too_many_arguments)]
    fn grant(
        &mut self,
        a: &Ask,
        o: &Offer,
        pick: Vec<(String, String)>,
        threads: u32,
        mem_mb: u64,
        fill: u64,
        clock: Clock,
    ) -> Pick {
        if let Some(l) = pick.last() {
            self.last.insert((a.worker.clone(), o.step.clone()), l.0.clone());
        }
        let lease = self.leases.grant_at(
            &a.worker,
            Work::Job { step: o.step.clone(), targets: pick.clone() },
            clock.now,
            clock.unix,
        );
        // (The leases changed: what `pack` kept of them made again.)
        if let Some(p) = self.packing.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            *p = Packing::default();
        }
        self.leases.set_threads(lease, threads);
        {
            // (Its step's other waits weighing no more than this job restart now: the step isn't
            // starved of room for them, it took it. Else a step granted target by target in its
            // waits' order holds the aged reservation for good, each grant handing it to its next
            // target, and every other step waits behind it (task #552: 52 units aged an hour kept
            // the lead idle while the members ran them back to back).)
            // (Weighed by the NAS's own limits (`nas_own`), whoever asked: the lead's own aged waits
            // restart on a member's grant too.)
            // (Only its aged ones: the others hold no room yet, and the step's next grant restarts
            // them once they've aged. Its own range of the waits: a grant doesn't weigh them all.)
            let ts: Vec<String> = pick.iter().map(|t| t.0.clone()).collect();
            let mut g = self.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
            for t in &ts {
                g.remove(&(o.step.clone(), t.clone()));
            }
            let aged_by = clock.now.checked_sub(crate::budget::nas::OWN_AGE);
            let from = (o.step.clone(), String::new());
            let aged: Vec<String> = g
                .range(from..)
                .take_while(|((st, _), _)| *st == o.step)
                .filter(|(_, (since, _))| aged_by.is_some_and(|a| *since <= a))
                .map(|((_, t), _)| t.clone())
                .collect();
            if !aged.is_empty() {
                if let Some((nw, ..)) = self.nas_room(clock.now, true) {
                    let w = Self::job_nas(&nw, &o.step, None, ts.iter().map(String::as_str)).0;
                    for t in aged.into_iter().filter(|t| nw.weight(&o.step, t, None) <= w) {
                        if let Some(e) = g.get_mut(&(o.step.clone(), t)) {
                            e.0 = clock.now;
                        }
                    }
                }
            }
            self.quiet_since.lock().unwrap_or_else(|e| e.into_inner()).remove(&o.step);
        }
        // (The wait kept let go: worked out again.)
        let mut kept = self.nas_reserved_at.lock().unwrap_or_else(|e| e.into_inner());
        if kept
            .as_ref()
            .and_then(|k| k.1.as_ref())
            .is_some_and(|h| h.1 .0 == o.step && pick.iter().any(|t| t.0 == h.1 .1))
        {
            *kept = None;
        }
        drop(kept);
        let ts: Vec<String> = pick.iter().map(|t| t.0.clone()).collect();
        let disk_mb = self.job_need(&o.step, &ts).map_or(0, |m| m.disk_mb) + fill;
        // (What `pick_every` sized it to, as it summed it.)
        let sized = ts.first().filter(|_| o.job.is_none()).and_then(|first| {
            let at = self.sizing_threads(&a.worker, &o.step, first, ask_cores(a));
            self.batch_secs(&o.step, ts.iter().map(String::as_str), &a.worker, at)
        });
        Pick { lease, step: o.step.clone(), targets: pick, threads, mem_mb, disk_mb, job: o.job.clone(), sized }
    }

    /// A member around on an older app (its last ask said no capacity, `Ask::cap`), the first by name; None when every one reports the new app. While there's
    /// one, every ask is granted one job (`pack`) and the old wire is still written
    /// (`crate::agent::Status::all_new`).
    pub(super) fn old_member(&self, now: Instant) -> Option<String> {
        self.workers
            .iter()
            .filter(|(_, w)| w.kind == "native" && now.saturating_duration_since(w.seen) < AROUND)
            .find(|(_, w)| w.ask.is_some() && w.old)
            .map(|(n, _)| n.clone())
    }

    /// Whether the lead's own planned job of offer `o` (`Offer::job`) is open to `a`'s worker: its
    /// targets (some), none held (a cancel's hold until a newer app among them: `Leases::held`),
    /// done under its key or kept from the worker after it failed, and no other job of its step
    /// runs anywhere (one job of a step kept to the lead at a time).
    pub(super) fn own_open(&self, o: &Offer, a: &Ask, now: Instant) -> bool {
        if o.targets.is_empty()
            || self.leases.live(now).any(|l| matches!(&l.work, Work::Job { step, .. } if *step == o.step))
        {
            return false;
        }
        let held = self.leases.held(&o.step, now);
        o.targets.iter().all(|(t, k, _)| {
            let backoff = self.failed.get(&(a.worker.clone(), cost_key(&o.step, t))).is_some_and(|(at, n)| {
                now.duration_since(*at) < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5))
            });
            !held.contains(t) && self.done.get(&(o.step.clone(), t.clone())) != Some(k) && !backoff
        })
    }

    /// What offer `o` waits for when refused for its room alone (`pack`, task #965): the measure
    /// (memory, disk) of its first target in plan order `a`'s worker could take with its whole room
    /// `whole` (every job it holds ended), when the room left `room` doesn't fit it. None when
    /// there's none (nothing open, a cold start: `own_cold_waits`'s, or none that ever fits this
    /// Mac: the room isn't held for what can't go here), or that one fits the room left (held back
    /// for another reason, the NAS budget's say). With it, the targets it's of (the job's).
    pub(super) fn room_wait(
        &self,
        o: &Offer,
        whole: &Ask,
        room: &Ask,
        now: Instant,
    ) -> Option<(Measured, Vec<String>)> {
        let job_disk = self.job_floor(&o.step);
        let fits = |m: &Measured, r: &Ask| m.mem_mb <= r.mem_mb && r.disk_mb.is_none_or(|d| m.disk_mb <= d);
        let (m, ts) = if o.job.is_some() {
            if !self.own_open(o, whole, now) {
                return None;
            }
            let ts: Vec<String> = o.targets.iter().map(|t| t.0.clone()).collect();
            (self.job_need(&o.step, &ts)?, ts)
        } else {
            let held = self.leases.held(&o.step, now);
            o.targets.iter().find_map(|(t, k, _)| {
                let m = self.measured(&o.step, t)?;
                self.may_take_measured(o, whole, t, k, &m, &held, now).then(|| (m, vec![t.clone()]))
            })?
        };
        let m = Measured { disk_mb: m.disk_mb.max(job_disk), ..m };
        (fits(&m, whole) && !fits(&m, room)).then_some((m, ts))
    }

    /// A job of `step` over `targets` on `worker`'s Mac, measured `m` and counting for `cores`
    /// (tenths), that has run `ran` s, as room held for a waiting job weighs it (`Run`).
    fn run_of(&self, step: &str, targets: &[String], worker: &str, m: Measured, cores: u32, ran: u64) -> Run {
        Run::of(m, cores, self.batch_secs(step, targets.iter().map(String::as_str), worker, 0).map(|s| s.0), ran)
    }

    /// Whether the lead's own planned job of offer `o` waits for room: open (`own_open`), a cold
    /// start (none of it measured) that can't go beside the lead's jobs: its provisional need past
    /// the room `left` or none (`no_figure`), a cold start held there already (`cold_held`), or its
    /// cores the whole Mac (`need_cores`; never a records-only step's, one thread counted for none).
    /// The lead's ask is then granted nothing more until its jobs end enough for it, or all of them
    /// (`pack`, `pick_own`), so the offered work it would otherwise keep taking doesn't starve it.
    pub(super) fn own_cold_waits(&self, o: &Offer, a: &Ask, left: u64, cold_held: bool, now: Instant) -> bool {
        let ts: Vec<String> = o.targets.iter().map(|t| t.0.clone()).collect();
        o.job.is_some()
            && self.job_need(&o.step, &ts).is_none()
            && (self.provisional(&o.step, &ts).is_none_or(|m| m.mem_mb > left)
                || cold_held
                || self.no_figure(&o.step, &ts)
                || (!self.the_plan.records_only(&o.step)
                    && !THREADS_HONOURED.contains(&o.step.as_str())
                    && self.uses.cores(&o.step, crate::paces::mac_of(&a.worker)).is_none()))
            && self.own_open(o, a, now)
            && self.own_nas_fits(o, now, true)
    }

    /// Whether the NAS budget has room for the lead's own planned job of offer `o`: its weight
    /// within the whole or beside nothing weighing anything, a download under the cap (the lead's
    /// own always held to it, `nas_room`); `alone`: as if the lead held nothing (the members' jobs
    /// alone counted: what its cold job would meet once its own jobs end, `own_cold_waits`), a test
    /// that changes nothing (no wait noted, no quiet clock started). Not `alone`, held back for the budget alone, it's noted waiting (`nas_wait`), and goes beside leased jobs
    /// none of which weighs the whole once it weighs the whole itself or has waited `OWN_AGE`
    /// (crate::budget::nas::Limits::fits_own_job: members' small jobs never hold it for good).
    pub(super) fn own_nas_fits(&self, o: &Offer, now: Instant, alone: bool) -> bool {
        let ts: Vec<String> = o.targets.iter().map(|t| t.0.clone()).collect();
        let waited = {
            let g = self.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
            ts.iter().filter_map(|t| g.get(&(o.step.clone(), t.clone())).map(|e| e.0)).min()
        }
        .is_some_and(|since| now.duration_since(since) >= crate::budget::nas::OWN_AGE);
        self.nas_room_heaviest(now, true).is_none_or(|(nw, running, members, dl, most)| {
            let (w, fetch) = Self::job_nas(&nw, &o.step, Some(&self.lead_name), ts.iter().map(String::as_str));
            let (running, heaviest) = if alone { (members, most[1]) } else { (running, most[0]) };
            if fetch && nw.limits.downloads.is_some_and(|c| dl >= c) {
                return false;
            }
            let own = running.saturating_sub(members);
            // (A quiet step, the clean-up, measured the way it runs now: alone or within the whole
            // by its measured weight, never aged past it, its wait not noted: it never holds room,
            // crate::budget::nas::QUIET_STEPS.) Held back `QUIET_AGE` (a pool that stays busy), it
            // goes as an aged job of the lead's own, its wait noted so its room is held. Unmeasured,
            // it weighs the whole (`NAS_STEPS`) and goes as the lead's own unmeasured NAS steps do,
            // beside no job weighing the whole (`fits_own_job`), so its first run measures it and no
            // burst of grants lands beside it meanwhile (task #1000: held for a quiet NAS instead,
            // a busy pool never had one, and step 8b's gc never ran).
            let quiet_step = crate::budget::nas::QUIET_STEPS.contains(&o.step.as_str());
            let measured = if ts.is_empty() {
                nw.book.nas(&o.step, "").is_some()
            } else {
                ts.iter().all(|t| nw.book.nas(&o.step, t).is_some())
            };
            let quiet = quiet_step && measured && {
                let mut g = self.quiet_since.lock().unwrap_or_else(|e| e.into_inner());
                let since = if alone {
                    g.get(&o.step).copied().unwrap_or(now)
                } else {
                    *g.entry(o.step.clone()).or_insert(now)
                };
                now.saturating_duration_since(since) < crate::budget::nas::QUIET_AGE
            };
            let waited = waited || (quiet_step && measured && !quiet);
            let fits = if quiet {
                crate::budget::nas::Limits::fits_quiet(running, w)
            } else {
                crate::budget::nas::Limits::fits_own_job(running, own, heaviest, w, waited)
            } && nw.limits.open(running, w)
                && (alone || Self::nas_reserve_ok(self.nas_reserved(&nw, now).as_ref(), &o.step, &ts, running, w));
            if !fits && !alone && !quiet {
                for t in &ts {
                    self.nas_wait(&o.step, t, now);
                }
            }
            fits
        })
    }

    /// The lead's own planned job of offer `o` (`Offer::job`), whole: all its targets, when it's open
    /// (`own_open`), the NAS budget has room for it (its weight, and a download under the cap: the
    /// lead's own always held to it, `nas_room`), and its measures (memory, disk) fit what the ask
    /// spares; a cold start (none of it measured) by its provisional need (`pack_need`), or alone
    /// (`cold_fits`). Empty: not now.
    pub(super) fn pick_own(&self, o: &Offer, a: &Ask, now: Instant) -> Vec<(String, String)> {
        if !self.own_open(o, a, now) {
            return Vec::new();
        }
        let ts: Vec<String> = o.targets.iter().map(|t| t.0.clone()).collect();
        let fits = match self.job_need(&o.step, &ts) {
            Some(m) => m.mem_mb <= a.mem_mb && a.disk_mb.is_none_or(|d| m.disk_mb <= d),
            None => self.cold_fits(&o.step, &ts, a, now),
        };
        // (The budget asked last: its wait noted only when the job would go but for it.)
        if !fits || !self.own_nas_fits(o, now, false) {
            return Vec::new();
        }
        o.targets.iter().map(|(t, k, _)| (t.clone(), k.clone())).collect()
    }

    /// Every step to every member (docs/pool.md §12, phase 4's ninth batch): the targets of offer
    /// `o` that `a`'s worker may do now, as a run contiguous in plan order (one held, done, kept from
    /// it after it failed, or past its room ends the run), from the front of the plan, or just after
    /// its last target of the step when that one's open; each measured (memory, disk) within what it
    /// spares, as many as fit the batch length (`batch_len`) by their measured times on its Mac at the
    /// threads it'd be granted with its start (`batch_secs`: one with none ends the job after its first), their disk summed (or the most a job of the step took, the larger) within
    /// its room. A cold start (`measured` None) by its provisional need, one job of its step in the
    /// pool at a time.
    pub(super) fn pick_every(&self, o: &Offer, a: &Ask, now: Instant) -> Vec<(String, String)> {
        let held = self.held_of(&o.step, now);
        // (Each target's failures looked up only where the worker failed any; the step's work done
        // taken out once, each target looked up in it: an ask weighs every target of every offer.)
        let (any_failed, done) = (self.failed_any(&a.worker), self.done_of(&o.step));
        let open = |t: &str, k: &str| {
            let backoff = any_failed
                && self.failed.get(&(a.worker.clone(), cost_key(&o.step, t))).is_some_and(|(at, n)| {
                    now.duration_since(*at)
                        < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5))
                });
            !held.contains(t) && done.get(t) != Some(&k) && !backoff
        };
        let job_disk = self.job_floor(&o.step);
        let fits = |m: &Measured| m.mem_mb <= a.mem_mb && a.disk_mb.is_none_or(|d| m.disk_mb.max(job_disk) <= d);
        // (The NAS budget on: a target whose weight the pool's leased jobs leave no room for, or a
        // download past the cap, is left for later.)
        let lead = a.worker == self.lead_name;
        let nas = self.nas_room_heaviest(now, lead);
        let downloads_full = nas.as_ref().is_some_and(|(nw, _, _, dl, _)| {
            nw.limits.downloads.is_some_and(|c| *dl >= c) && Self::job_nas(nw, &o.step, None, []).1
        });
        if downloads_full {
            return Vec::new();
        }
        // (The lead's own asks held within the whole, or alone; a member's past it beside the
        // lead's own alone: crate::budget::nas::Limits. Weighed as the job it would make: `t` added
        // to the targets picked before it, `with`, task #939.)
        let nas_fits = |t: &str, with: &JobLoad| {
            nas.as_ref().is_none_or(|(nw, running, members, _, most)| {
                let mut j = *with;
                nw.add(&mut j, &o.step, t, Some(&a.worker));
                let w = nw.job(&j);
                // (The NAS's own load over the owner's target: nothing new that weighs anything
                // but beside nothing, crate::budget::nas::Loop.)
                if !nw.limits.open(*running, w) {
                    return false;
                }
                // (Room held for a target aged in the budget left to it: `nas_reserved`.)
                if !Self::nas_reserve_ok(self.nas_reserved(nw, now).as_ref(), &o.step, &[t.to_string()], *running, w) {
                    return false;
                }
                // (One aged `OWN_AGE` goes beside jobs none of which weighs the whole, the lead's
                // only once aged (`fits_own_target`): the room held for it is always let go.)
                let waited = {
                    let g = self.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
                    g.get(&(o.step.clone(), t.to_string()))
                        .is_some_and(|e| now.duration_since(e.0) >= crate::budget::nas::OWN_AGE)
                };
                if lead {
                    let own = running.saturating_sub(*members);
                    crate::budget::nas::Limits::fits_own_target(*running, own, most[0], w, waited)
                } else {
                    crate::budget::nas::Limits::fits_job(*members, *running, most[0], w, waited)
                }
            })
        };
        let takeable = |i: usize| {
            let (t, k, _) = &o.targets[i];
            // (Its wait noted only when this asker would take it but for the budget: a target no
            // asker can take never ages into holding room, `nas_reserved`.)
            open(t, k)
                && match self.measured(&o.step, t) {
                    Some(m) => fits(&m),
                    // (A cold start: by its provisional need (`provisional`), while no job of its
                    // step is under way anywhere (the lead's own held by a lease too): one learns
                    // its measure before the step fans out.)
                    None => held.is_empty() && self.cold_fits(&o.step, std::slice::from_ref(t), a, now),
                }
                && (nas_fits(t, &JobLoad::default()) || {
                    self.nas_wait(&o.step, t, now);
                    false
                })
        };
        let after = self
            .last
            .get(&(a.worker.clone(), o.step.clone()))
            .and_then(|l| o.targets.iter().position(|t| t.0 == *l))
            .map(|i| i + 1);
        let start = match &self.placement {
            // (Placement on: from the target measured to take longest that's drawn to this worker.)
            Some((_, since)) => self.placed_start(o, a, *since, now, &held, after, &takeable),
            None => after
                .filter(|&i| i < o.targets.len() && takeable(i))
                .or_else(|| (0..o.targets.len()).find(|&i| takeable(i))),
        };
        let Some(start) = start else { return Vec::new() };
        let first = &o.targets[start];
        let Some(m0) = self.measured(&o.step, &first.0) else { return vec![(first.0.clone(), first.1.clone())] };
        let mut pick = vec![(first.0.clone(), first.1.clone())];
        // (Its time as `batch_secs` sums it, target by target, at the threads it'd be granted.)
        let threads = self.sizing_threads(&a.worker, &o.step, &first.0, ask_cores(a));
        let (mut secs, mut disk) =
            (self.batch_secs(&o.step, [first.0.as_str()], &a.worker, threads).map(|s| s.0), m0.disk_mb);
        let mut load = JobLoad::default();
        let add = |load: &mut JobLoad, t: &str| {
            if let Some((nw, ..)) = &nas {
                nw.add(load, &o.step, t, Some(&a.worker));
            }
        };
        add(&mut load, &first.0);
        for (t, k, _) in &o.targets[start + 1..] {
            let (Some(s), Some(m)) = (secs, self.measured(&o.step, t)) else { break };
            let Some((more, _)) = self.secs_on(&o.step, t, &a.worker, threads) else { break };
            if pick.len() >= JOB_MAX
                || !open(t, k)
                || !fits(&m)
                || !nas_fits(t, &load)
                || s + more > self.batch_len()
                || a.disk_mb.is_some_and(|d| crate::needs::batch_disk(&o.step, [disk, m.disk_mb]).max(job_disk) > d)
            {
                break;
            }
            pick.push((t.clone(), k.clone()));
            add(&mut load, t);
            (secs, disk) = (Some(s + more), crate::needs::batch_disk(&o.step, [disk, m.disk_mb]));
        }
        pick
    }

    /// Whether `a`'s worker could take measured target `t` (its key `k`) of offer `o` now, by
    /// `pick_every`'s rules: not held (`held`), not done, not kept from it after it failed it, and
    /// its measures within the room its ask says.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn may_take_measured(
        &self,
        o: &Offer,
        a: &Ask,
        t: &str,
        k: &str,
        m: &Measured,
        held: &BTreeSet<String>,
        now: Instant,
    ) -> bool {
        let backoff = self.failed_any(&a.worker)
            && self.failed.get(&(a.worker.clone(), cost_key(&o.step, t))).is_some_and(|(at, n)| {
                now.duration_since(*at) < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5))
            });
        let job_disk = self.job_floor(&o.step);
        !held.contains(t)
            && !(self.done_any(&o.step)
                && self.done.get(&(o.step.clone(), t.to_string())).map(String::as_str) == Some(k))
            && !backoff
            && m.mem_mb <= a.mem_mb
            && a.disk_mb.is_none_or(|d| m.disk_mb.max(job_disk) <= d)
    }

    /// Whether `worker`, as its last ask `x` said, is kept from more jobs by work outside the pool:
    /// it holds a job, and the cores busy outside the pool (`outside_cores`, past those its jobs
    /// count for: `need_cores`'s within their threads, their threads where no CPU time was
    /// measured) with those cores fill its Mac's (task #440): placement draws
    /// targets among the others. Its own jobs alone filling its cores don't count.
    pub(super) fn swamped(&self, worker: &str, x: &Ask, now: Instant) -> bool {
        let mine = self.job_leases(worker, now);
        if mine.is_empty() {
            return false;
        }
        let total = ask_cores(x);
        let idle = |l: &Lease| {
            x.cap.as_ref().is_some_and(|c| c.idle.contains(&l.id) || c.waiting.iter().any(|w| w.lease == l.id))
        };
        let ours: u32 = mine
            .iter()
            .map(|l| match &l.work {
                // (As its Mac's packing counts it.)
                Work::Job { step, targets } => {
                    self.need_cores(step, &Need { idle: idle(l), ..self.held_need(l, step, targets, now) }, total).1
                }
                Work::Task { .. } => 0,
            })
            .sum();
        let ours = tenths_to_cores(ours);
        let outside = outside_cores(x, ours);
        outside > 0 && ours.saturating_add(outside) >= total
    }

    /// Placement on (crate::agent::placement): where `a`'s worker's run of offer `o` starts. The
    /// targets it may take (`takeable`): those in its last one's neighbourhood (`after`, its last
    /// of the step's index past it; `placement::group`: a z6 tile's z3 tile) first, from just after
    /// it in plan order, so its consecutive jobs share their inputs (task #552); then from the one
    /// measured to take longest (`secs_of`; plan order between equals, those never measured last).
    /// The first drawn to it among the members asking lately that would take it now
    /// (`would_take_now`; `placement::draw`, each by `draw_weight`, in the window `now` is in since
    /// `since`); a cold start (none of its step measured) undrawn, by its provisional need
    /// (`cold_fits`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn placed_start(
        &self,
        o: &Offer,
        a: &Ask,
        since: Instant,
        now: Instant,
        held: &BTreeSet<String>,
        after: Option<usize>,
        takeable: &dyn Fn(usize) -> bool,
    ) -> Option<usize> {
        use crate::agent::placement;
        let mut order: Vec<(Option<u64>, usize)> =
            (0..o.targets.len()).map(|i| (self.secs_of(&o.step, &o.targets[i].0, &a.worker), i)).collect();
        order.sort_by(|x, y| y.0.is_some().cmp(&x.0.is_some()).then(y.0.cmp(&x.0)).then(x.1.cmp(&y.1)));
        // (Its last one's neighbourhood first, in plan order from just after it.)
        let near: Vec<usize> = match after.and_then(|i| o.targets.get(i.checked_sub(1)?)) {
            // (A target with no neighbourhood but itself (not a z6 tile): none.)
            Some(last) if placement::group(&last.0) != last.0.as_str() => {
                let g = placement::group(&last.0);
                let from = after.unwrap_or(0);
                (from..o.targets.len()).chain(0..from).filter(|&i| placement::group(&o.targets[i].0) == g).collect()
            }
            _ => Vec::new(),
        };
        let rest = order.into_iter().map(|(_, i)| i).filter(|i| !near.contains(i));
        let window = now.saturating_duration_since(since).as_secs();
        near.iter().copied().chain(rest).find(|&i| {
            if !takeable(i) {
                return false;
            }
            let (t, k, _) = &o.targets[i];
            let Some(m) = self.measured(&o.step, t) else { return true };
            // (Each other member by the rules its own ask would meet, and only one that would take
            // it now: a draw's winner never leaves it untaken.)
            let mut who: Vec<(String, f64)> = self
                .workers
                .iter()
                .filter(|(n, w)| {
                    *n != &a.worker
                        && w.kind == "native"
                        && now.duration_since(w.seen) < AROUND
                        && w.asked.is_some_and(|at| now.duration_since(at) < ASKING)
                        && w.app.as_ref().is_none_or(|x| *x == self.app)
                })
                .filter_map(|(n, w)| {
                    let x = w.ask.as_ref()?;
                    let takes = x.can.contains(&o.step)
                        && self.may_take_measured(o, x, t, k, &m, held, now)
                        && !self.swamped(n, x, now)
                        && self.would_take_now(n, w.asked?, x, &m, now);
                    takes.then(|| (n.clone(), self.draw_weight(&o.step, n, x)))
                })
                .collect();
            who.push((a.worker.clone(), self.draw_weight(&o.step, &a.worker, a)));
            placement::draw(&placement::key(&format!("{} {t}", o.step), window), &who) == Some(a.worker.as_str())
        })
    }

    /// The memory (MB) a job under lease `l` takes: its heaviest target's, by the book's estimate
    /// (a step never measured: the guess it was offered with, else 0).
    pub(super) fn lease_mem(&self, l: &Lease) -> u64 {
        let Work::Job { step, targets } = &l.work else { return 0 };
        targets.iter().map(|t| self.measures(step, &t.0).0).max().unwrap_or(0)
    }

    /// Whether worker `n`, by its last ask `x` (made at `asked`), would take a target measured `m`
    /// now: a slot free (fewer jobs granted it since than the ask wanted, one for an ask saying
    /// none) and the memory its Mac had free then (`Capacity::mem_mb`, when its capacity is said),
    /// less that of the jobs granted it since (`lease_mem`), fits it.
    pub(super) fn would_take_now(&self, n: &str, asked: Instant, x: &Ask, m: &Measured, now: Instant) -> bool {
        let since: Vec<&Lease> = self.worker_jobs(n, now).filter(|l| l.granted >= asked).collect();
        let taken: u64 = since.iter().map(|l| self.lease_mem(l)).sum();
        since.len() < x.want.max(1)
            && x.cap.as_ref().filter(|c| c.limit_mb > 0).is_none_or(|c| m.mem_mb.saturating_add(taken) <= c.mem_mb)
    }

    /// Worker `n`'s weight in placement's draw for a target of `step`, as its last ask `x` says:
    /// its Mac's speed at the step, the inverse of its pace there (`paces`, crate::paces::Paces::get:
    /// at the step, else over its steps, else the lead's), `CACHE_PREFER` times that where its Mac
    /// holds the step's shared cache whole (`holds_cache`).
    pub(super) fn draw_weight(&self, step: &str, n: &str, x: &Ask) -> f64 {
        let cache = if self.holds_cache(step, x) { crate::agent::placement::CACHE_PREFER } else { 1.0 };
        cache / self.paces.get(n, step)
    }

    /// Whether the Mac asking `a` holds the shared cache a job of `step` reads whole (its ask's
    /// capacity says at least the cache's last fill): placement's `CACHE_PREFER` (task #552). A
    /// fill not measured yet, or a step reading none: no.
    pub(super) fn holds_cache(&self, step: &str, a: &Ask) -> bool {
        crate::needs::cache_of(step).is_some_and(|c| {
            let fill = self.fill_mb(c);
            fill > 0 && a.cap.as_ref().and_then(|x| x.caches.get(c)).is_some_and(|h| h.have_mb >= fill)
        })
    }
}

/// The scheduler's state as data (`Sched::state`, `Sched::from_state`): what the forecast runs the
/// scheduler on (crate::agent::forecast::Input::sched, written with the lead's forecast), and what a
/// recorded status gives a test. Its times as ages: seconds before it was taken.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The jobs' leases out: their workers, steps, targets (target, key) and threads.
    pub leases: Vec<Leased>,
    pub offers: Vec<Offer>,
    /// (step, target, key) done under a key the plan hasn't shown yet.
    pub done: Vec<(String, String, String)>,
    /// (worker, cost key, age of the last, how many).
    pub failed: Vec<(String, String, u64, u32)>,
    /// Each worker's last ask (the lead's own with its capacity: `Coordinator::sched_state`).
    pub workers: Vec<Asked>,
    pub costs: BTreeMap<String, Cost>,
    pub floors: BTreeMap<String, Floor>,
    pub disks: BTreeMap<String, u64>,
    pub job_disks: BTreeMap<String, Vec<u64>>,
    pub fills: BTreeMap<String, u64>,
    pub app: String,
    pub lead_name: String,
    pub placement: Option<(crate::agent::placement::Placement, u64)>,
    pub nas: Option<crate::budget::nas::Limits>,
    pub nas_own: Option<crate::budget::nas::Limits>,
    /// (step, target, age first held back, age last).
    pub nas_waits: Vec<(String, String, u64, u64)>,
    /// (step, age).
    pub quiet_since: Vec<(String, u64)>,
    /// (worker, step, its last target).
    pub last: Vec<(String, String, String)>,
    /// Each Mac's pace at each step (`Sched::paces`; code 4 on: a record before it has none).
    pub paces: crate::paces::Paces,
    /// Each step's use on each Mac (`Sched::uses`, task #947: a record before it has none).
    pub uses: crate::uses::Uses,
    /// The batch length set (`Sched::batch_s`, task #927: 0, not set or a record before it).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub batch_s: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl State {
    /// The cores `a`'s Mac has past those busy outside the pool as its ask says (`outside_cores`,
    /// past the cores the jobs it holds here count for: each its step's use on the Mac's busy cores
    /// (crate::uses), else its targets' most kept busy (crate::needs::Cores::busy_tenths), summed in
    /// tenths, within its threads, else its threads): the forecast's room for its jobs (task #684).
    pub fn cores_left(&self, the_plan: &'static dyn crate::agent::seam::Plan, a: &Ask) -> u32 {
        let b = crate::needs::Book::new(the_plan, &self.costs);
        let ours: u32 = self
            .leases
            .iter()
            .filter(|l| l.worker == a.worker)
            .map(|l| {
                let ts: Vec<String> = l.targets.iter().map(|t| t.0.clone()).collect();
                let c =
                    self.uses.cores(&l.step, crate::paces::mac_of(&l.worker)).or_else(|| book_cores(&b, &l.step, &ts));
                c.and_then(|c| c.busy_tenths).map_or(l.threads * 10, |c| c.min(l.threads * 10))
            })
            .sum();
        let ours = tenths_to_cores(ours);
        // (A capacity saying no cores falls back to the ask's, as `ask_cores` does.)
        let cores = a.cap.as_ref().map(|c| c.cores).filter(|&c| c > 0).unwrap_or(a.cores);
        cores.saturating_sub(outside_cores(a, ours)).max(1)
    }
}

/// A job's lease out (`State::leases`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Leased {
    pub worker: String,
    pub step: String,
    pub targets: Vec<(String, String)>,
    pub threads: u32,
    /// When it was granted (`Lease::granted_at`, unix s; 0: not said, a record before task #973),
    /// or when its job began if that's before (a lease taken up: `Lease::began`, task #992): a
    /// scheduler made from the state has its job run that long (what room held for a waiting job
    /// weighs, `Backfill`), not just started.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub granted_at: u64,
}

/// A worker's last ask (`State::workers`): the ask, whether it ran an older app (`Worker::old`),
/// and the ages of its last request and ask.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub ask: Ask,
    pub old: bool,
    pub seen_s: u64,
    pub asked_s: Option<u64>,
}

/// `now` less `age` seconds (as far back as an `Instant` goes).
fn ago(now: Instant, age: u64) -> Instant {
    now.checked_sub(Duration::from_secs(age)).unwrap_or(now)
}

impl Sched {
    /// Its state as data at `clock` (`State`).
    pub fn state(&self, clock: Clock) -> State {
        let now = clock.now;
        let age = |t: Instant| now.saturating_duration_since(t).as_secs();
        State {
            leases: self
                .leases
                .all(now)
                .into_iter()
                .filter_map(|l| {
                    // (Its job's start when it's older than its lease: `Lease::began`, task #992.)
                    let granted_at = l.granted_at.min(clock.unix.saturating_sub(l.ran(now).as_secs()));
                    match l.work {
                        Work::Job { step, targets } => {
                            Some(Leased { granted_at, worker: l.worker, step, targets, threads: l.threads })
                        }
                        Work::Task { .. } => None,
                    }
                })
                .collect(),
            offers: self.offers.clone(),
            done: self.done.iter().map(|((s, t), k)| (s.clone(), t.clone(), k.clone())).collect(),
            failed: self.failed.iter().map(|((w, k), (at, n))| (w.clone(), k.clone(), age(*at), *n)).collect(),
            workers: self
                .workers
                .values()
                .filter_map(|w| {
                    Some(Asked { ask: w.ask.clone()?, old: w.old, seen_s: age(w.seen), asked_s: w.asked.map(age) })
                })
                .collect(),
            costs: self.costs.clone(),
            floors: self.floors.clone(),
            disks: self.disks.clone(),
            job_disks: self.job_disks.clone(),
            fills: self.fills.clone(),
            app: self.app.clone(),
            lead_name: self.lead_name.clone(),
            placement: self.placement.as_ref().map(|(p, since)| (p.clone(), age(*since))),
            nas: self.nas,
            nas_own: self.nas_own,
            nas_waits: self
                .nas_waits
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|((s, t), (a, b))| (s.clone(), t.clone(), age(*a), age(*b)))
                .collect(),
            quiet_since: self
                .quiet_since
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|(s, a)| (s.clone(), age(*a)))
                .collect(),
            last: self.last.iter().map(|((w, s), t)| (w.clone(), s.clone(), t.clone())).collect(),
            paces: self.paces.clone(),
            uses: self.uses.clone(),
            batch_s: self.batch_s,
        }
    }

    /// A scheduler in state `s` at `now` (`clock`), its leases lasting `ttl` without a beat: the
    /// leases out granted again (fresh ids), each worker's ask as it was.
    pub fn from_state(
        the_plan: &'static dyn crate::agent::seam::Plan,
        s: &State,
        clock: Clock,
        ttl: Duration,
    ) -> Sched {
        let now = clock.now;
        let mut leases = Leases::new(ttl);
        leases.app = s.app.clone();
        for l in &s.leases {
            // (Granted, by the wall clock, when it was where the record says: its job has run
            // since. Its lease counts from now, as before.)
            let at = if l.granted_at > 0 { l.granted_at.min(clock.unix) } else { clock.unix };
            let id =
                leases.grant_at(&l.worker, Work::Job { step: l.step.clone(), targets: l.targets.clone() }, now, at);
            leases.began_at(id, at, clock.unix, now);
            leases.set_threads(id, l.threads);
        }
        let workers = s
            .workers
            .iter()
            .map(|w| {
                let a = &w.ask;
                (
                    a.worker.clone(),
                    Worker {
                        kind: a.kind.clone(),
                        label: a.label.clone().unwrap_or_else(|| a.worker.clone()),
                        can: a.can.clone(),
                        mem_mb: a.mem_mb,
                        cores: a.cores,
                        app: a.app.clone(),
                        cap: a.cap.clone(),
                        ask: Some(a.clone()),
                        old: w.old,
                        seen: ago(now, w.seen_s),
                        asked: w.asked_s.map(|x| ago(now, x)),
                        what: String::new(),
                        done: 0,
                        failed: 0,
                    },
                )
            })
            .collect();
        let mut sched = Sched {
            the_plan,
            leases,
            offers: Vec::new(),
            offered: Default::default(),
            drops: Default::default(),
            done: s.done.iter().map(|(st, t, k)| ((st.clone(), t.clone()), k.clone())).collect(),
            failed: s.failed.iter().map(|(w, k, a, n)| ((w.clone(), k.clone()), (ago(now, *a), *n))).collect(),
            workers,
            costs: s.costs.clone(),
            floors: s.floors.clone(),
            disks: s.disks.clone(),
            job_disks: s.job_disks.clone(),
            fills: s.fills.clone(),
            needs: Default::default(),
            needs_rev: 0,
            app: s.app.clone(),
            placement: s.placement.as_ref().map(|(p, a)| (p.clone(), ago(now, *a))),
            nas: s.nas,
            nas_own: s.nas_own,
            lead_name: s.lead_name.clone(),
            nas_waits: Mutex::new(
                s.nas_waits
                    .iter()
                    .map(|(st, t, a, b)| ((st.clone(), t.clone()), (ago(now, *a), ago(now, *b))))
                    .collect(),
            ),
            nas_reserved_at: Default::default(),
            quiet_since: Mutex::new(s.quiet_since.iter().map(|(st, a)| (st.clone(), ago(now, *a))).collect()),
            last: s.last.iter().map(|(w, st, t)| ((w.clone(), st.clone()), t.clone())).collect(),
            measured_steps: Default::default(),
            measured_of: Default::default(),
            drop_needs: Default::default(),
            kept_leases: Default::default(),
            packing: Default::default(),
            provisional_at: Default::default(),
            paces: s.paces.clone(),
            use_runs: Vec::new(),
            uses: s.uses.clone(),
            batch_s: s.batch_s,
        };
        sched.set_offers(s.offers.clone());
        sched
    }

    /// The members in `idle` (name, slots free now, the memory its Mac has with no job of the pool
    /// running, MB) ask again at `now`, as the live ones do each loop, wanting a job a free slot
    /// (and those granted them this instant, which count since their ask), their memory less that
    /// of every job running that began before. Only a member with a slot free asks: a busy one
    /// keeps its last ask and ages out of placement's draws (`ASKING`), as a live one's does. (The
    /// forecast's lanes ask only as they free, so its members would otherwise age out as simulated
    /// time passes, and one granted a job would read as full.)
    pub fn asking<'a>(&mut self, idle: impl IntoIterator<Item = (&'a str, usize, u64)>, now: Instant) {
        for (n, free, room) in idle {
            if free == 0 {
                continue;
            }
            let granted = self.worker_jobs(n, now).filter(|l| l.granted >= now).count();
            let running: u64 = self.worker_jobs(n, now).filter(|l| l.granted < now).map(|l| self.lease_mem(l)).sum();
            if let Some(w) = self.workers.get_mut(n) {
                (w.seen, w.asked) = (now, Some(now));
                if let Some(x) = w.ask.as_mut() {
                    x.want = free + granted;
                    if let Some(c) = x.cap.as_mut() {
                        c.mem_mb = room.saturating_sub(running);
                    }
                }
            }
        }
    }

    /// Lease `id` ended (its job done): its work no longer held.
    pub fn end(&mut self, id: u64) {
        self.leases.cancel(id);
    }
}
