//! The scheduler (docs/pool.md §7.2, §7.7): the one place a job is granted. Every rule that decides
//! which jobs a Mac's ask gets lives here and nowhere else (tests/grant_rules.rs keeps it so): the
//! offers in plan order, the lead's own planned jobs to the lead alone (crate::agent::every::grants),
//! a step's targets as a run in plan order (`pick_every`) and the lead's planned jobs whole
//! (`pick_own`), the packing by measured memory, disk and cores (crate::needs), cold starts by their
//! provisional need, one at a time on a Mac and never-measured steps alone, the NAS budget
//! (jobrt::budget::nas: its weights and limits, the aged reservations, quiet steps, the downloads'
//! cap, the pacing of the NAS by its latency) and placement (crate::agent::placement).
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
    /// What a thread of it past `thread_covers` adds to its memory (MB), and the threads `mem_mb`
    /// covers (`Sched::thread_mb`; crate::coord::Grant::thread_mb).
    pub thread_mb: u64,
    pub thread_covers: u32,
    pub disk_mb: u64,
    pub job: Option<String>,
    /// The time it was sized to (s), and whether by its Mac's own runs (`Sched::batch_secs`); None:
    /// a planned job of the lead's own (`pick_own`), or a target of no time measured.
    pub sized: Option<(u64, bool)>,
    /// Why it went to this Mac (`placed_start`'s words, placement on); None: placement off or a
    /// job it didn't place (a cold start, the lead's own).
    pub why: Option<String>,
    /// The NAS weight it was granted under (thousandths of the NAS: `lease_nas`, by its targets'
    /// rates, until its beats outweigh them, `job_weight`), and whether any of them was measured
    /// (none: its step's default, the whole for a NAS step, else nothing); None: the NAS budget off
    /// for its asker.
    pub nas: Option<(u32, bool)>,
}

/// Why `Sched::pack` granted an ask fewer jobs than it wanted: the rule that held the rest back,
/// and in words (the lead's status says them, `Shared::refused`).
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub rule: Rule,
    pub words: String,
    /// The job held back with its disk short now (the first refused for its room, `Rule::Room`, or
    /// the one room is held for, `Rule::Waiting`) whose memory fits its Mac once its jobs end, the
    /// disk left out (the most its jobs could hold, `ask_limit`; its cores always fit, a job granted
    /// within its Mac's for jobs, `ask_cores`), and whose disk its caches can give (`Capacity::freeable_mb`): the disk it
    /// needs (MB: its step's disk floor and its shared cache's fill among it, the whole copies
    /// counted free for it taken off), which its Mac's agent frees its caches toward past its room
    /// target and reservations (crate::agent's `held_room`); room is held for it meanwhile
    /// (`Backfill`, `room_wait`). None: no such job. A rule that doesn't weigh the disk
    /// (`Rule::decides_disk`) names none, and the coordinator keeps the worker's last through it
    /// (`Shared::pack`).
    pub disk_mb: Option<u64>,
    /// That job's step and target (with `disk_mb`): a hold kept through answers that don't weigh
    /// the disk ends once it's leased, done or offered no more (`Shared::pack`).
    pub held_for: Option<(String, String)>,
}

/// The grant rule that held an ask back (`Held`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    /// Its Mac's memory guard: the brake, under its floor or a give-back's cool-down
    /// (`Capacity::mem_held_back`).
    Memory,
    /// A cold start: one at a time on a Mac, a step with nothing to size it by alone, a cold job of
    /// the lead's own waiting for its jobs to end.
    Cold,
    /// Room held for the first offer in plan order refused for its room (`Backfill`).
    Waiting,
    /// Past the cores its jobs' threads leave, the minute's wait for the load (`SETTLE`).
    Settle,
    /// Its cores.
    Cores,
    /// Its cores, with some busy outside the pool (`outside_cores`).
    Outside,
    /// Nothing offered fitting the room left (memory, disk) or free to take (`Refused`).
    Room,
    /// Work is offered, none of it to this Mac: the lead's own planned jobs to a member, a step it
    /// doesn't run (`Sched::apart`); the lead's status adds the plan's waits
    /// (crate::agent::flags::nothing_words).
    Away,
    /// Nothing is offered at all (`Sched::apart`; an ask while the plan has no pass): the plan may
    /// be complete or its work wait behind other work or an input; the lead's status says which
    /// (crate::agent::flags::nothing_words).
    Empty,
    /// The NAS budget: targets it would take waiting for the NAS's room.
    Nas,
}

impl Rule {
    /// Whether an answer held back by it decides the disk hold (`Held::disk_mb`): the room rules
    /// and the NAS budget's (its answer is the room rule's, the NAS's waits said first) weigh the
    /// disk, and work for other Macs or none at all holds nothing for this one. The others (memory
    /// held back, a cold start, the load, the cores) hold back before the disk is weighed: the
    /// worker's last hold stands through them while its job waits (`Shared::pack`).
    pub fn decides_disk(self) -> bool {
        matches!(self, Rule::Room | Rule::Waiting | Rule::Away | Rule::Empty | Rule::Nas)
    }

    /// Whether an answer held back by it offered its worker no work at all (none offered, or none
    /// for it): nothing waits for that Mac's cores (crate::agent::threads, the next thread's rule).
    pub fn offers_none(self) -> bool {
        matches!(self, Rule::Away | Rule::Empty)
    }

    /// The rule in a few words.
    pub fn name(self) -> &'static str {
        match self {
            Rule::Memory => "memory",
            Rule::Cold => "a cold start",
            Rule::Waiting => "room held for a waiting job",
            Rule::Settle => "the load's minute",
            Rule::Cores => "cores",
            Rule::Outside => "cores busy outside the pool",
            Rule::Room => "the room left",
            Rule::Away => "work for other Macs",
            Rule::Empty => "no job ready",
            Rule::Nas => "the NAS budget",
        }
    }

    /// Where the rule decides by live state the forecast can't know (docs/pool.md §7.2, One
    /// scheduler): what it is, in words; None, a rule the forecast's run packs by too.
    pub fn live(self) -> Option<&'static str> {
        match self {
            Rule::Memory => Some("macOS's memory now; the forecast takes each Mac at its sustained best"),
            Rule::Outside => Some("load outside the pool now; the forecast takes each Mac at its sustained best"),
            Rule::Nas => Some(
                "its pacing window, cut by the NAS's latency or load, is a reading of now; the forecast holds it only while the jobs out run",
            ),
            _ => None,
        }
    }
}

/// `Rule::Empty`'s words, as the coordinator knows them (the lead's status words it from the plan's
/// waits, crate::agent::flags::nothing_words).
pub const EMPTY: &str = "no job is offered";

fn held_by(rule: Rule, words: String) -> Held {
    Held { rule, words, disk_mb: None, held_for: None }
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
    /// Work done (a member's hand-off taken, or this Mac's job ended), (step, target) → key, its
    /// record not yet read by a plan: a plan that read the records before its hand-off was merged
    /// (such as one under way as the job ended) still offers it at that key. Never granted again meanwhile
    /// (`pack`, Coordinator::hold_for), said "done (its record not read yet)" in a refusal
    /// (`Refused::words`), and no work to come in the forecast (crate::agent::forecast::unread);
    /// let go by the first plan that doesn't offer it at that key (Coordinator::offer), or as the
    /// lead's merge refuses its hand-off or passes it over with the records under another key: its
    /// record never to be read (Coordinator::merged).
    pub done: BTreeMap<(String, String), String>,
    /// A worker's failures: (worker, cost_key(step, target)) → (the last, how many): not offered to
    /// it again for a while (`backoff`, as `fail_hold` says: an hour by default, doubling each time);
    /// and its brakes on a step (`brake_key`), each holding the whole step from it so.
    pub failed: BTreeMap<(String, String), (Instant, u32)>,
    /// The failures' backoff as the pool's settings say (jobrt::backoff: `backoff-start-hours`,
    /// `backoff-ceiling-hours`), set by the lead as it packs.
    pub fail_hold: jobrt::backoff::Backoff,
    pub workers: BTreeMap<String, Worker>,
    /// Each step's Macs' most memory for jobs at their best (`set_step_rooms`, MB): what a measured
    /// target's spread is bounded by.
    pub step_rooms: BTreeMap<String, u64>,
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
    /// (crate::needs::CACHES) count for nothing past their cache's fill (`note_fill`). Kept in the
    /// lead's store (crate::coord::kept).
    pub job_disks: BTreeMap<String, Vec<u64>>,
    /// What filling each shared cache (crate::needs::CACHES) on a Mac takes (MB, by cache): the
    /// last run that filled it, its disk past its targets' own output (`note_fill`). A Mac is
    /// reserved what of it its capacity says it lacks (`missing`). Kept in the lead's store.
    pub fills: BTreeMap<String, u64>,
    /// The steps' models of their needs (crate::needs), kept while `needs_rev` stands: it moves with
    /// each cost, floor or disk floor taken or cleared.
    pub needs: crate::needs::Cache,
    pub needs_rev: u64,
    /// The app this Mac's agent runs: an agent on another builds with other code than the keys it
    /// would record say, so it gets no work ("" in tests: any).
    pub app: String,
    /// Placement (docs/pool.md §7.2: crate::agent::placement), while the lead's switch has it on,
    /// and when it was first told (the tasks' draws' windows are reckoned from then): the offers
    /// tried by their steps' slack, each of a member's targets to the Mac asking that finishes it
    /// soonest (`placed_start`), the tasks drawn by their measured pace at the kind, a task offered
    /// ahead given to a slower worker its head start covers. None: off.
    pub placement: Option<(crate::agent::placement::Placement, Instant)>,
    /// The NAS budget's limits (docs/pool.md §7.7, jobrt::budget::nas): the NAS's as probed, and the
    /// downloads' cap. Read by `pick_every` (so `pack` too). None: the switch off, or no probe yet.
    pub nas: Option<jobrt::budget::nas::Limits>,
    /// The NAS's limits as probed whatever the switch says: the lead's own jobs are always held to
    /// the budget (`nas_room`). None: not probed yet.
    pub nas_own: Option<jobrt::budget::nas::Limits>,
    /// This Mac's member name, its own jobs' leases held under it (`Coordinator::ask_own`): the NAS
    /// budget lets a member's job go past the whole while the lead's own alone hold weight.
    pub lead_name: String,
    /// Targets passed over for the NAS budget alone ((step, target) → (first, last time, the room
    /// its last refusal needed of the window beside what was in use: `NasRefused::need`; None:
    /// taken up from a record, not refused since)), for the status (`Coordinator::nas_waits`) and
    /// the pacing (`nas_wants`); a target granted is let go.
    pub nas_waits: Mutex<BTreeMap<(String, String), (Instant, Instant, Option<u32>)>>,
    /// `nas_reserved`'s wait (since when, step and target) as last worked out, at which time: kept
    /// while the time stands, an aged wait passed over again taking its place when it was held
    /// back first, worked out again when the one kept is granted (task #544: a pick weighs it for
    /// every target).
    pub nas_reserved_at: Mutex<Option<(Instant, Option<Reserved>)>>,
    /// When each quiet step's job (jobrt::budget::nas::QUIET_STEPS) was first held back for a
    /// quiet NAS since it was last granted: past `QUIET_AGE` it goes as an aged job (task #451).
    pub quiet_since: Mutex<BTreeMap<String, Instant>>,
    /// The quiet steps' jobs held back for a quiet NAS (step → when last, its weight): noted for
    /// the pacing alone (`nas_wants`: a clean-up waiting for the window grows it as any job held
    /// back does), never holding room.
    pub quiet_waits: Mutex<BTreeMap<String, (Instant, u32)>>,
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
    /// What placement worked out within one `pack` (`Placed`), cleared at each.
    pub placed: Mutex<Placed>,
    /// What's worked out of each leased job (`LeaseKept`), by lease, as the measures stood (`rev`),
    /// and as offered (`set_offers` clears it).
    pub kept_leases: Mutex<(u64, FxMap<u64, Vec<LeaseKept>>)>,
    /// Each Mac's time against the lead's, by their speeds (crate::paces; set through `set_paces`,
    /// which moves `needs_rev`), and each Mac's jobs' start at each step: the measures' CPU is read
    /// lead's pace by it, and a worker's at its own. Empty: as measured.
    pub paces: crate::paces::Paces,
    /// What the lead's timings records say of each job run (crate::needs::Records: the model learns
    /// only from runs whose record, where kept, is clean; set through `set_records`), and the needs
    /// model's percentiles and windows as the pool's settings say (`set_tuning`): each moves
    /// `needs_rev`.
    pub records: crate::needs::Records,
    pub tuning: jobrt::needs::Tuning,
    /// The batch length the pool's setting says (s; `batch-minutes`, crate::coord::asks; 0: not set,
    /// `JOB_S`): `batch_len`.
    pub batch_s: u64,
    /// How the progress estimates of each step's jobs ended (`Lefts`; set through `set_lefts`).
    pub lefts: Lefts,
    /// What is left of each target whose run stopped short (s, at the lead's pace), by step, then
    /// target: crate::agent::forecast::partials of the history (`set_stopped`), so placement takes it
    /// by its time left, not as never measured (`secs_of`, task #1454).
    pub stopped: BTreeMap<String, BTreeMap<String, f64>>,
    /// The jobs the history's events named at the last `set_stopped`: those of them that resumed
    /// blocks keep their records (`set_records`).
    pub event_jobs: BTreeSet<String>,
}

/// How jobs' progress estimates ended (task #1505 review): what was left in fact over what their
/// beats said (crate::coord::history::History::left_ratios), by step, sorted, and every step's
/// together (the pool's, under ""); or, in a scheduler made from a record (`Sched::from_state`),
/// the discounts its lead took from them then (`State::lefts`). A progress end is the time left
/// its beats say times its step's ratio at the low percentile the needs model's high one mirrors
/// (`Sched::left_discount`): an estimate that grows as its job slows, or ends early, is held to
/// how such estimates have ended.
#[derive(Clone, Debug, Default)]
pub struct Lefts {
    by: BTreeMap<String, Vec<f64>>,
    taken: BTreeMap<String, f64>,
}

impl Lefts {
    /// Of what was left in fact over what the beats said, by step (crate::coord::history::History::left_ratios):
    /// each step's sorted, and every step's together under "".
    pub fn of(by: BTreeMap<String, Vec<f64>>) -> Lefts {
        let mut by: BTreeMap<String, Vec<f64>> = by
            .into_iter()
            .map(|(k, mut v)| {
                v.retain(|r| r.is_finite());
                v.sort_by(f64::total_cmp);
                (k, v)
            })
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let mut all: Vec<f64> = by.values().flatten().copied().collect();
        all.sort_by(f64::total_cmp);
        if !all.is_empty() {
            by.insert(String::new(), all);
        }
        Lefts { by, taken: BTreeMap::new() }
    }

    /// `ratios` of a job of `step` that ended, put in among its step's and every step's, in order.
    pub fn add(&mut self, step: &str, ratios: &[f64]) {
        for &r in ratios.iter().filter(|r| r.is_finite()) {
            for k in [step, ""] {
                let v = self.by.entry(k.to_string()).or_default();
                let at = v.partition_point(|&x| x < r);
                v.insert(at, r);
            }
        }
    }
}

/// What placement (`Sched::placed_start`) keeps within one `Sched::pack`: why it left each target it
/// gave a worker to it ((worker, step, target) → why, in words), until its grant says it (`grant`);
/// and how many targets it passed to each other member, which hold that many of its free slots
/// for the rest of the pack's offers.
#[derive(Debug, Default)]
pub struct Placed {
    why: BTreeMap<(String, String, String), String>,
    passed: BTreeMap<String, usize>,
}

/// What one `Sched::pack` keeps of the leases while it packs, made again at each grant: the NAS
/// budget's sums at (now, lead) (`nas_room_heaviest`) and each step's targets held (`held_of`).
#[derive(Debug, Default)]
pub struct Packing {
    nas: Vec<((Instant, bool), (u32, u32, usize, [u32; 2]))>,
    held: BTreeMap<(String, Instant), std::sync::Arc<BTreeSet<String>>>,
}

/// What's kept of a leased job (`Sched::kept_leases`), of its worker, work and start's threads: its NAS weight by
/// the limits it was weighed by (`Sched::lease_nas`), and its packing need and time
/// (`Sched::lease_need`).
#[derive(Debug)]
pub struct LeaseKept {
    worker: String,
    work: Work,
    start: u32,
    nas: Vec<(jobrt::budget::nas::Limits, (u32, bool))>,
    need: Option<(Need, Option<u64>)>,
}

/// `Sched::measured_of`'s targets' measures: step → target → each threads asked (None: its own
/// start's) and its step's room (`Sched::step_room`), and `Sched::measures`. (Looked in for every
/// target an ask weighs, never walked: roadcore::fx.)
pub type Measures = FxMap<String, FxMap<String, Vec<((Option<u32>, Option<u64>), (u64, Option<Measured>))>>>;

/// The NAS budget's weights for one placement (`Shared::nas_room`): the limits and one Book (its
/// steps' NAS bytes per unit kept with its models: crate::needs::model).
pub(crate) struct NasWeigher<'a> {
    pub(super) limits: jobrt::budget::nas::Limits,
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
        // (crate::coord::cost_version) noted its rate: the whole budget, so its first job never
        // overloads the NAS unseen, task #499 review; runs of an earlier way count for nothing,
        // crate::needs's `nas_rate`.)
        r.map_or(if NAS_STEPS.contains(&step) { jobrt::budget::nas::WHOLE } else { 0 }, |r| self.limits.weight(r))
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
        self.limits.weight(jobrt::budget::nas::Rate {
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
    moved: jobrt::budget::nas::Rate,
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
    /// A job held: the threads its lease counts it for (`Lease::threads`, as granted and as its
    /// agent says it runs on since; 0: not said), which it's counted on and its busy cores never
    /// pass (`need_cores`).
    held: u32,
    /// A job held: the cores it keeps busy now, in tenths, once its run says it (`busy_now`), which
    /// it counts for in place of its measure's.
    busy_now: Option<u32>,
}

/// The room a job of the first offer refused for its room alone waits for on a Mac (`Sched::pack`,
/// task #965): its step and first target, its memory and disk (a shared cache's fill among it) and
/// the cores it counts for (tenths); and the whole copies counted free for it (`Copies`, MB).
struct RoomWait {
    step: String,
    target: String,
    mem_mb: u64,
    disk_mb: u64,
    cores: u32,
    credit: u64,
}

/// A job on the Mac an ask packs for, as the room held for a waiting job weighs it (`Backfill`):
/// its memory, disk and cores (tenths), and in how long it's expected to end (s from now; None: it
/// may end any moment, nothing saying when) and by what (`End`, `Sched::run_end`).
#[derive(Clone, Copy, Debug)]
struct Run {
    mem_mb: u64,
    disk_mb: u64,
    cores: u32,
    ends: Option<u64>,
    by: End,
}

impl Run {
    /// A job measured `m`, counting for `cores` (tenths), expected to end as `end` says.
    fn of(m: Measured, cores: u32, end: (Option<u64>, End)) -> Run {
        Run { mem_mb: m.mem_mb, disk_mb: m.disk_mb, cores, ends: end.0, by: end.1 }
    }
}

/// What a job's expected end (`Run::ends`) is taken from (`Sched::run_end`, task #1505), as the
/// room held for a waiting job says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// Its measured time (`Sched::batch_secs`) less what it has run.
    Measured,
    /// Its progress's pace, as its worker's last beat said it (`Lease::left_by_progress`), times
    /// its step's estimates' ratio at percentile `pct` (`Sched::left_discount`; `pool`: every
    /// step's, its own having none): a job not measured or past its measured time.
    Progress { pct: u32, pool: bool },
    /// Past its measured time with no end by its progress: its measured time and its slowest target's
    /// excess at its step's runs' 90th percentile (`Sched::batch_slow`) less what it has run, while
    /// it's within it.
    Slow,
    /// No end: past its measured time and its step's slower runs, and none by its progress.
    Overdue,
    /// No end: not measured, and none by its progress (none said, or no estimate ended yet).
    Unmeasured,
}

impl End {
    /// Its words, of a job whose pronoun is `its`.
    fn words(self, its: &str) -> String {
        match self {
            End::Measured => format!("by {its} measured time"),
            End::Progress { pct, pool } => {
                let whose = if pool { "the pool's".to_string() } else { format!("{its} step's") };
                let th = match (pct % 100 / 10, pct % 10) {
                    (1, _) => "th",
                    (_, 1) => "st",
                    (_, 2) => "nd",
                    (_, 3) => "rd",
                    _ => "th",
                };
                format!("by {its} progress, as {whose} estimates ended at their {pct}{th} percentile")
            }
            End::Slow => format!("past {its} measured time, by {its} step's slower runs"),
            End::Overdue => {
                format!("past {its} measured time and {its} step's slower runs, and no end by {its} progress")
            }
            End::Unmeasured => format!("with no measured time and no end by {its} progress"),
        }
    }
}

/// The room held for a waiting job (`RoomWait`) and what may still go while it waits (`Sched::pack`,
/// task #973: backfilling, so a hold never idles a Mac's cores for as long as its running jobs
/// last). The jobs it waits for (`waits_on`) are those of its Mac expected to end soonest, as few
/// as free its room once they end, any with no end said first (it may end any moment); `shadow`
/// is when they're expected to have ended (s from now, by each one's `Run::ends`: its measured
/// time, else its discounted progress, else its step's slower runs, task #1505; 0 where one has
/// none) and `by` what the last of them to end (or the one with none) is timed by. A later job goes
/// when it fits beside the waiting one once those end, every other job still running (`beside`),
/// or, its time measured, ends before `shadow` by its runs' 90th percentile. A job of the second
/// kind ends before the jobs waited for, so it's among them next time without putting off when
/// they end; once one of them has no end said, no more of that kind goes. A measured end only
/// comes nearer; a progress end may move later (its job slowing, its estimate said again each
/// beat), so the jobs of the second kind admitted meanwhile may end later than those first waited
/// for. Either way, when the jobs waited for end before their end said, the waiting job waits past
/// them at most for the longest job admitted beside it (its own 90th percentile, as admitted), and
/// it never waits for a long job while a shorter one frees its room.
struct Backfill {
    w: RoomWait,
    waits_on: usize,
    shadow: u64,
    by: End,
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
            rs.iter().fold((room.mem_mb, room.disk_mb.unwrap_or(0) + w.credit, cores), |(m, d, c), r| {
                (m + r.mem_mb, d + r.disk_mb, c + r.cores)
            })
        };
        // (Soonest to end first: one with no end said as ending now, so it's waited for, which
        // stops the second kind.)
        let mut runs = runs.to_vec();
        runs.sort_by_key(|r| r.ends.unwrap_or(0));
        let waits_on = (0..=runs.len()).find(|&k| fits(free(&runs[..k]))).unwrap_or(runs.len());
        let waited = &runs[..waits_on];
        // (The last of them to end, and what times it; one with none, sorted first, makes it 0.)
        let (shadow, by) = match waited.first() {
            Some(r) if r.ends.is_none() => (0, r.by),
            _ => waited.iter().map(|r| (r.ends.unwrap_or(0), r.by)).max_by_key(|e| e.0).unwrap_or((0, End::Measured)),
        };
        let left = free(waited);
        let beside = fits(left).then(|| {
            let (m, d, c) = left;
            (m - w.mem_mb, room.disk_mb.map(|_| d - w.disk_mb), c - w.cores)
        });
        Backfill { shadow, by, waits_on, beside, w }
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

    /// The refusal of `room` and the `cores` left (`words`'s), held as `Rule::Waiting`: with the
    /// disk the waiting job needs when its disk is short now and its memory fits its Mac once its
    /// jobs end (`whole_mb`) and its disk is within `reach` (`disk_reach`), so the disk freed now
    /// isn't what it waits for then (`Held::disk_mb`).
    fn held(&self, room: &Ask, cores: u32, whole_mb: u64, reach: u64, later: bool) -> Held {
        let w = &self.w;
        let need = w.disk_mb.saturating_sub(w.credit);
        let short = room.disk_mb.is_some_and(|d| d + w.credit < w.disk_mb);
        let rest = w.mem_mb <= whole_mb && need <= reach;
        let named = short && rest;
        Held {
            disk_mb: named.then_some(need),
            held_for: named.then(|| (w.step.clone(), w.target.clone())),
            ..held_by(Rule::Waiting, self.words(room, cores, later))
        }
    }

    /// The refusal in words, of `room` and the `cores` left; `later`: an offer after it was held
    /// back for it (else nothing after it was free to take or fitted anyway).
    fn words(&self, room: &Ask, cores: u32, later: bool) -> String {
        let w = &self.w;
        let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
        let job = if w.target.is_empty() { w.step.clone() } else { format!("{} {}", w.step, w.target) };
        let mut s = format!("room held for a job of {job} ({} of memory, {} left", gb(w.mem_mb), gb(room.mem_mb));
        // (Its disk too where that's what it's short of: a wait for disk alone read as one for
        // memory with room to spare.)
        if let Some(d) = room.disk_mb.map(|d| d + w.credit).filter(|&d| d < w.disk_mb) {
            s += &format!("; {} of disk, {} left", gb(w.disk_mb), gb(d));
        }
        if w.cores > cores {
            s += &format!("; {} cores, {} left", tenths(w.cores), tenths(cores));
        }
        // (Which end the hold took, task #1505: of several, the last to end's, or the one with none.)
        let by = self.by.words("its");
        let when = match (self.waits_on, self.shadow) {
            // (Short of its room with no job of the pool's running: the load outside the pool or the
            // disk holds it.)
            (0, _) => "no job of the pool's running for it to wait for".to_string(),
            (1, 0) => format!("the job it waits for running {by}"),
            (1, t) => format!("the job it waits for expected to end in {} min, {by}", t.div_ceil(60)),
            (n, 0) => format!("the {n} jobs it waits for running, one {by}"),
            (n, t) => {
                format!("the {n} jobs it waits for expected to end in {} min, the last of them {by}", t.div_ceil(60))
            }
        };
        let then = if later {
            "nothing offered after it fits beside it or ends before then"
        } else {
            "nothing else offered is free to take or fits"
        };
        s + &format!(", {when}): {then}")
    }
}

/// The whole copies (crate::needs::CACHES) a member's Mac holds of the shared caches no job it
/// holds reads (task #1170), but those in use (task #1173: `CacheHere::in_use_mb`, held by a
/// process there, the pool's or not, which no freeing deletes while it runs). Its ask's disk leaves them out (crate::agent's `freeable`: held by
/// their own rule, so a job reading them isn't counted their room twice), yet room-making and the
/// freeing toward its target and reservations delete them for a job reading none of them
/// (crate::agent::room: those no job there reads first, then those its forecast's jobs read, the
/// furthest ahead first; the largest only once the others have freed its bytes). So a job of a
/// step reading none of them is packed into the ask's disk and them (`credit`); what a grant takes
/// of them is spent (`take`), and a job reading one granted after reserves what was spent of it
/// beside its fill (`spent`). So is, first, what's owed of them past the ask's disk (`pack`'s
/// `owed`: the jobs the lead takes off it past 0, and at an ask of 0 what its target and
/// reservations take past its free space): an earlier answer's grant on them, its disk reserved
/// while they're still there, isn't counted them again. Nothing on the lead (alone, its disk unbounded; beside its jobs, a
/// job of its own needs its room free as it starts) nor on a Mac with no room target (its caches
/// aren't counted there: the guard holds only a full disk), as for its other caches.
struct Copies {
    /// Each cache's: what the Mac holds of it not in use, and what's left of that unspent (MB).
    left: BTreeMap<&'static str, (u64, u64)>,
}

impl Copies {
    /// `a`'s (`lead`: the lead's own ask), its jobs reading the caches `read`.
    fn of(a: &Ask, lead: bool, read: &BTreeSet<&'static str>) -> Copies {
        let left = match a.cap.as_ref().filter(|c| !lead && c.target_mb > 0 && a.disk_mb.is_some()) {
            Some(c) => crate::needs::CACHES
                .iter()
                .map(|(n, _)| *n)
                .filter(|n| !read.contains(n))
                .filter_map(|n| c.caches.get(n).map(|h| (n, h.have_mb.saturating_sub(h.in_use_mb))))
                .map(|(n, free)| (n, (free, free)))
                .filter(|(_, (have, _))| *have > 0)
                .collect(),
            None => BTreeMap::new(),
        };
        Copies { left }
    }

    /// What a job of `step` may count of them (MB): what's left of those of the caches it doesn't
    /// read.
    fn credit(&self, step: &str) -> u64 {
        let own = crate::needs::cache_of(step);
        self.left.iter().filter(|(n, _)| Some(**n) != own).map(|(_, (_, l))| l).sum()
    }

    /// What of the cache a job of `step` reads was spent on the jobs granted before it (MB).
    fn spent(&self, step: &str) -> u64 {
        crate::needs::cache_of(step).and_then(|c| self.left.get(c)).map_or(0, |(have, l)| have - l)
    }

    /// A job of `step` granted, `mb` of it past the ask's disk left: spent of those it doesn't
    /// read; the one it reads held by its rule from then.
    fn take(&mut self, step: &str, mut mb: u64) {
        let own = crate::needs::cache_of(step);
        for (_, (_, l)) in self.left.iter_mut().filter(|(n, _)| Some(**n) != own) {
            let t = mb.min(*l);
            *l -= t;
            mb -= t;
        }
        if let Some(c) = own {
            self.left.remove(c);
        }
    }
}

/// What became of the targets of the offers an ask was granted none of (`Sched::refused`, task
/// #1170): how many were held by jobs, done, or kept from its worker after it failed them
/// (`backoff`), the first of the rest whose measure is past the room left: its step, its target and
/// its measure (its step's disk floor and its shared cache's fill among it); and how many of the
/// rest before it weren't past it (`other`: unmeasured, or held back by the NAS budget or placement).
#[derive(Default)]
struct Refused {
    held: usize,
    done: usize,
    backoff: usize,
    other: usize,
    blocked: Option<(String, String, Measured)>,
    /// The steps braked on its Mac (`Sched::braked`), in words (`brake_words`).
    braked: Vec<String>,
}

impl Refused {
    /// The disk the blocked target needs (MB, the whole copies counted for it, `copies`, taken off)
    /// when its disk is past the room left `room` (`Held::disk_mb`) and its memory fits its Mac's
    /// `whole_mb` once its jobs end (as `Backfill::held` weighs it: short of memory now beside them,
    /// it's held for once the disk is freed, `room_wait`) and within `reach`, the most its Mac's
    /// disk can give (`disk_reach`); None when it would never fit there, or none is blocked.
    fn disk_alone(&self, room: &Ask, copies: u64, reach: u64, whole_mb: u64) -> Option<u64> {
        let (_, _, m) = self.blocked.as_ref()?;
        let need = m.disk_mb.saturating_sub(copies);
        (room.disk_mb.is_some_and(|d| m.disk_mb > d + copies) && need <= reach && m.mem_mb <= whole_mb).then_some(need)
    }

    /// The disk the blocked target needs past `reach` (MB), when it does: no cache is freed for it
    /// (`disk_alone`).
    fn past_reach(&self, copies: u64, reach: u64) -> Option<u64> {
        let (_, _, m) = self.blocked.as_ref()?;
        Some(m.disk_mb.saturating_sub(copies)).filter(|&n| n > reach)
    }

    /// The room rule's refusal in words, of `a` and the room left `room` (its disk with the whole
    /// copies counted for the blocked target, `copies` of it): the target refused for its room, what
    /// it needs and what its Mac is short of, each by how much; then what became of the other
    /// targets offered.
    fn words(&self, a: &Ask, room: &Ask, copies: u64) -> String {
        let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
        // (Done, its hand-off merged after the records the plan offering it read: `Sched::done`.)
        let done = if self.done == 1 { "done (its record not read yet)" } else { "done (their records not read yet)" };
        let others: Vec<String> = [
            (self.held, "held by jobs"),
            (self.done, done),
            (self.backoff, "in backoff after failing or being refused here"),
            (self.other, "free but held back otherwise (no measure yet, the NAS budget or placement)"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .chain(self.braked.iter().cloned())
        .collect();
        let mem = || match a.cap.as_ref() {
            Some(c) => mem_words(c, room.mem_mb),
            None => format!("{} left", gb(room.mem_mb)),
        };
        let disk = |d: u64| {
            let copies = if copies > 0 {
                format!(", {} of it whole copies room-making frees", gb(copies))
            } else {
                String::new()
            };
            match a.cap.as_ref() {
                Some(c) => format!("{}{copies}", disk_words(c, d)),
                None => format!("{} left{copies}", gb(d)),
            }
        };
        let Some((step, target, m)) = &self.blocked else {
            let what = if self.other == 0 { "is free to take" } else { "is taken" };
            return format!("nothing offered {what}: {}", others.join(", "));
        };
        let mut short = Vec::new();
        if m.mem_mb > room.mem_mb {
            short.push(format!("memory short by {} ({})", gb(m.mem_mb - room.mem_mb), mem()));
        }
        if let Some(d) = room.disk_mb.filter(|&d| m.disk_mb > d) {
            short.push(format!("disk short by {} ({})", gb(m.disk_mb - d), disk(d)));
        }
        let job = if target.is_empty() { step.clone() } else { format!("{step} {target}") };
        let rest = if others.is_empty() {
            String::new()
        } else {
            format!("; of the other targets offered, {}", others.join(", "))
        };
        format!("{job} needs {} of memory and {} of disk: {}{rest}", gb(m.mem_mb), gb(m.disk_mb), short.join("; "))
    }
}

/// `a` with `less` taken off its disk and `more` added (MB; a disk not said stays so), copied only
/// when that changes it: an ask weighs every offer.
fn disk_with(a: &Ask, less: u64, more: u64) -> std::borrow::Cow<'_, Ask> {
    match a.disk_mb {
        Some(d) if less > 0 || more > 0 => {
            std::borrow::Cow::Owned(Ask { disk_mb: Some((d + more).saturating_sub(less)), ..a.clone() })
        }
        _ => std::borrow::Cow::Borrowed(a),
    }
}

/// The steps known to honour the threads they're granted (task #499 review): their cold starts
/// take a thread (task #896: their agent grows them from there while they keep their threads busy,
/// docs/pool.md §7.1, Threads controlled live), the others' the whole Mac. `slope-hi` and `slope-deep`: Rust steps
/// on the rayon pool the agent sizes (`RAYON_NUM_THREADS`, crate::agent::jobs); `terrain-deep`: its
/// band past z12 made on that pool, its reads ahead waiting on the network (terrain::terrain_deep); `bld-fetch`: its
/// downloads bounded by its `--jobs` (3 by default, at most
/// 4), waiting on the network; `pyramids-fill`: its fetches a host's requests in flight (buildstore::fetch's limit a host, store::aimd),
/// waiting on the network, its own work (whole-PNG checks, packing) a fraction of a core (task #823:
/// counted as the whole Mac, its first job held 6 of the mini's 12 cores).
pub(super) const THREADS_HONOURED: &[&str] = &["slope-hi", "slope-deep", "terrain-deep", "bld-fetch", "pyramids-fill"];

/// Whether jobs of `step` keep to the threads they're granted, so fewer threads hold less memory:
/// the steps honouring their threads (`THREADS_HONOURED`) and the paced ones (crate::agent::PACED:
/// their threads file read every second). Such a job's memory is read at the threads it's granted
/// (`Sched::mem_threads`, `pack`, `lease_need`).
pub(super) fn keeps_threads(step: &str) -> bool {
    THREADS_HONOURED.contains(&step) || crate::agent::PACED.contains(&step)
}

/// A cold start's provisional memory (MB) from `mb`, the most its step's runs of its latest earlier
/// way measured or it was offered with: times 3/2 (task #499; `Sched::provisional`, the forecast's
/// packing too).
pub(super) fn provisional_mb(mb: u64) -> u64 {
    mb.saturating_mul(3) / 2
}

/// The cores a job of `step` over `targets` takes by the cost book (crate::needs::Book::cores):
/// the most of its targets', each of them; None when one's aren't said.
fn book_cores(b: &crate::needs::Book, step: &str, targets: &[String]) -> Option<crate::needs::Cores> {
    let v = targets.iter().map(|t| b.cores(step, t)).collect::<Option<Vec<crate::needs::Cores>>>()?;
    Some(crate::needs::Cores {
        grant: v.iter().map(|c| c.grant).max().unwrap_or(0),
        busy_tenths: v.iter().map(|c| c.busy_tenths).collect::<Option<Vec<u32>>>().and_then(|b| b.into_iter().max()),
    })
}

/// The most disk `a`'s Mac can give a job held back for its disk (MB): its ask's disk (its free
/// space less its room target and its jobs' reservations) and what its caches can free past that
/// (`Capacity::freeable_mb`). Not said (an older app's, or not counted yet), no bound: such a hold
/// names the disk as before, and no room is held for it (`room_wait`).
fn disk_reach(a: &Ask) -> u64 {
    match a.cap.as_ref().and_then(|c| c.freeable_mb) {
        Some(f) => a.disk_mb.unwrap_or(0).saturating_add(f),
        None => u64::MAX,
    }
}

/// What `a`'s Mac can hold for jobs at its best (MB; task #1501), not as busy as it is now: its
/// memory less its floor, nothing else on it (`ask_limit`, the most its jobs could hold together;
/// without it, the ask's memory), and its ask's disk for jobs with what its jobs have reserved and
/// its caches can free given back (`Capacity::reserved_mb`, `caches_free`). The disk None: the ask
/// bounds none (the lead's own while it runs nothing), which any disk fits, as `pack` reads it.
/// What `too_big` weighs a target against: a target past a busy Mac's room now but within this
/// waits for it, and isn't flagged.
fn best_room(a: &Ask) -> (u64, Option<u64>) {
    let mem = super::ask_limit(a).unwrap_or(a.mem_mb);
    let reserved = a.cap.as_ref().map_or(0, |c| c.reserved_mb);
    (mem, a.disk_mb.map(|d| d + reserved + caches_free(a)))
}

/// What `a`'s Mac's caches can free past its ask's disk (MB, `Capacity::freeable_mb`; 0 not said):
/// counted as the disk a job its room is held for has (`room_wait`).
fn caches_free(a: &Ask) -> u64 {
    a.cap.as_ref().and_then(|c| c.freeable_mb).unwrap_or(0)
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
/// busy can't tell a test build (tools/app/remote-tests.sh's, at utility priority) from work
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
/// target isn't counted as using nothing. A measure is its targets' last runs': on 10 Nov one counted
/// the M4's terrain-deep jobs at 3.8 cores each, from runs at 5 threads two days before, while they
/// kept 2.2–3.5 busy at 4. A lease of a
/// scheduler made from a record (`Sched::from_state`, the forecast's), no beat of its own saying,
/// counts for what the record's lead counted it for (`Leased::busy_tenths`, task #1094).
fn busy_now(l: &Lease, secs: Option<u64>, now: Instant) -> Option<u32> {
    let started = l.live.as_ref().is_some_and(|v| v.targets_done > 0)
        || l.ran(now).as_secs() > secs.unwrap_or(jobrt::budget::nas::OWN_AGE.as_secs());
    let beats = || {
        let cores = l.cores_mean()?.max(l.cores_lately().unwrap_or(0.0));
        Some(((cores * 10.0).round() as u32).max(1))
    };
    started.then(beats).flatten().or(l.busy_said)
}

/// The threads a job wanting `threads` and counting for `busy` tenths of a core is granted where
/// `left` tenths are left by the rest of its Mac's work: within the cores left and the threads it
/// leaves idle (its threads less its busy cores, whole: they add no load), one at least
/// (`Sched::pack`).
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

/// The steps that load the NAS as they go (reads; the clean-up's deletes, task #1000; the planet's
/// download, written and read back, task #1120): one none of whose runs the way it runs now
/// measured its rate weighs the NAS budget's whole (`NasWeigher`).
pub(super) const NAS_STEPS: &[&str] = &["unit", "pack", "lo", "gc", "planet-fetch"];

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
    /// a shared cache is copied only where there's room (buildstore::localread), its jobs there reading
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
        self.floors.get(&cost_key(step, target)).filter(|f| f.v == self.the_plan.cost_version(step)).copied()
    }

    /// The memory a job of `step` for `target` is expected to take (MB), as `job_peak` sizes it from
    /// its costs and `size`, and never below its floor of the way its step runs now
    /// (crate::coord::cost_version): a cost or floor of another way never raises it.
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
    pub(crate) fn book(&self) -> crate::needs::Book<'_> {
        let inputs = crate::needs::Inputs { paces: &self.paces, records: &self.records, tuning: self.tuning };
        crate::needs::Book::kept(self.the_plan, &self.costs, &self.floors, &self.disks, inputs, &self.needs, self.rev())
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

    /// `step`'s work done whose record the plan hasn't read yet (`done`): target → key.
    fn done_of(&self, step: &str) -> std::collections::HashMap<&str, &str> {
        let from = (step.to_string(), String::new());
        self.done
            .range(from..)
            .take_while(|((st, _), _)| st == step)
            .map(|((_, t), k)| (t.as_str(), k.as_str()))
            .collect()
    }

    /// Whether target `t` of `step` is kept from `worker` after it failed it there (`failed`): the
    /// backoff's start after its first failure, twice as long after each more, at most its ceiling
    /// (`fail_hold`).
    /// Its step braked there too (`braked`) holds every target of it.
    pub(super) fn backoff(&self, worker: &str, step: &str, t: &str, now: Instant) -> bool {
        self.held_by(worker, &cost_key(step, t), now) || self.braked(worker, step, now)
    }

    /// Whether `step` is kept whole from `worker`'s Mac (its brake, `brake_key`, by Mac:
    /// crate::paces::mac_of; `Brakes`), as a target's failure holds it.
    pub(super) fn braked(&self, worker: &str, step: &str, now: Instant) -> bool {
        self.held_by(crate::paces::mac_of(worker), &brake_key(step), now)
    }

    /// Whether `failed`'s entry of `worker` and `key` holds now.
    fn held_by(&self, worker: &str, key: &str, now: Instant) -> bool {
        self.failed
            .get(&(worker.to_string(), key.to_string()))
            .is_some_and(|(at, n)| now.saturating_duration_since(*at) < self.fail_hold.hold(*n))
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
        self.measures(step, target, None).1
    }

    /// The threads a job of `step` over `targets` starts on as its memory is read (`measures`):
    /// the most of its targets' grants by the cost book (crate::needs::Book::cores: the cores its
    /// runs keep busy and a thread more), whatever cores its Mac has, so a target's memory is the
    /// same for every Mac (`too_big`'s too); a records-only step's one; 0 (as its runs ran) where
    /// none says.
    pub(super) fn start_threads<'t>(&self, step: &str, targets: impl IntoIterator<Item = &'t str>) -> u32 {
        if self.the_plan.records_only(step) {
            return 1;
        }
        let b = self.book();
        targets.into_iter().filter_map(|t| b.cores(step, t)).map(|c| c.grant).max().unwrap_or(0)
    }

    /// The threads a job of `step` over `targets` has its memory read at where its Mac's room for
    /// it is `room_mb` (MB): those it starts on (`start_threads`); for a step whose jobs keep to the
    /// threads they're granted (`keeps_threads`), whose job there passes `room_mb`, the most fewer
    /// whose memory fits, else one, the least it could start on (task #1582: the regions check,
    /// 4.6 GB on the one thread it ran on, its start's thread more booked at its whole peak, 9.0 GB
    /// on two, waited hours on a lead with 4.6 GB for a new job). It's granted that many threads
    /// at most, its memory read there (`pack`), and its agent grows it past them only where its Mac's
    /// memory covers each (its `thread_mb`, docs/pool.md §7.1, Threads controlled live). A cold
    /// start (no measure): those it starts on.
    pub(super) fn mem_threads(&self, step: &str, targets: &[String], room_mb: u64) -> u32 {
        let start = self.start_threads(step, targets.iter().map(String::as_str));
        if start <= 1 || !keeps_threads(step) {
            return start;
        }
        (1..=start)
            .rev()
            .find(|&t| self.job_need_on(step, targets, Some(t)).is_some_and(|m| m.mem_mb <= room_mb))
            .unwrap_or(1)
    }

    /// The threads the memory of a job of `step` over `targets` granted `granted` threads to start
    /// on was read at: those for a step whose jobs keep to their threads (`keeps_threads`:
    /// `mem_threads`, or fewer for the cores left), else those it starts on by the cost book
    /// (`start_threads`) or `granted` where more.
    fn read_at<'t>(&self, step: &str, targets: impl IntoIterator<Item = &'t str>, granted: u32) -> u32 {
        if keeps_threads(step) && granted > 0 {
            granted
        } else {
            self.start_threads(step, targets).max(granted)
        }
    }

    /// `measured` on the threads its memory fits `room_mb` on (`mem_threads`).
    pub(super) fn measured_in(&self, step: &str, target: &str, room_mb: u64) -> Option<Measured> {
        let ts = [target.to_string()];
        self.measures(step, target, Some(self.mem_threads(step, &ts, room_mb))).1
    }

    /// `job_need` on the threads its memory fits `room_mb` on (`mem_threads`).
    pub(super) fn job_need_in(&self, step: &str, targets: &[String], room_mb: u64) -> Option<Measured> {
        self.job_need_on(step, targets, Some(self.mem_threads(step, targets, room_mb)))
    }

    /// `target` of `step`'s memory by the book (a step never measured: its offered guess, `lease_mem`)
    /// and `measured`, on `threads` (None: its own `start_threads`): the memory a job of it is
    /// admitted and packed at (crate::needs::Book::mem: its step's runs' `needs-mem-pct` percentile
    /// at those threads), whichever Mac asks; kept while the measures stand (`measured_of`). A job
    /// grown past the threads it was read at reserves `thread_mb` for each (`Grant::thread_mb`,
    /// docs/pool.md §7.1). A measured target's spread is bounded by its step's room (`step_room`).
    fn measures(&self, step: &str, target: &str, threads: Option<u32>) -> (u64, Option<Measured>) {
        let rev = self.rev();
        let room = self.step_room(step);
        let at = (threads, room);
        {
            let mut g = self.measured_of.lock().unwrap_or_else(|e| e.into_inner());
            if g.0 != rev {
                *g = (rev, Measures::default());
            }
            if let Some(&(_, m)) = g.1.get(step).and_then(|t| t.get(target)).and_then(|v| v.iter().find(|e| e.0 == at))
            {
                return m;
            }
        }
        let b = self.book();
        // (A drop not run yet: its step's drops', `drop_of`.)
        let got = if let Some((_, mem_mb)) = self.drop_of(&b, step, target) {
            (mem_mb, Some(Measured { mem_mb, disk_mb: 0, own: false }))
        } else {
            let on = threads.unwrap_or_else(|| self.start_threads(step, [target]));
            let (mem_mb, from) = b.mem_within(step, target, on, self.offered_guess(step, target), room);
            let m = (from != crate::needs::From::Table).then(|| Measured {
                mem_mb,
                disk_mb: b.disk(step, target).unwrap_or(0),
                own: from == crate::needs::From::Measured,
            });
            (mem_mb, m)
        };
        let mut g = self.measured_of.lock().unwrap_or_else(|e| e.into_inner());
        g.1.entry(step.to_string()).or_default().entry(target.to_string()).or_default().push((at, got));
        got
    }

    /// The most memory a Mac that runs `step` holds for jobs at its best (MB, `step_rooms`); None
    /// with none known. A measured target's own run is booked at most this times its spread
    /// (crate::needs::Book::mem_within, task #1586 review): derived from the Macs, so a spread
    /// never books a target that fits one past them all.
    fn step_room(&self, step: &str) -> Option<u64> {
        self.step_rooms.get(step).copied()
    }

    /// The Macs' rooms each step's spread is bounded by (`step_rooms`), from `macs`: the Macs
    /// `too_big` weighs (those heard from lately, the lead's own with its capacity: crate::coord's
    /// `Shared::macs`), each step the most memory one that runs it holds for jobs at its best, as
    /// its capacity's limit says (a Mac whose capacity says none counts for none: its ask's memory
    /// is its room now, not at its best, and moves with every ask). Set as the lead packs, weighs
    /// what is too big and records its state, so a packing reads each once.
    pub fn set_step_rooms<'a>(&mut self, macs: impl IntoIterator<Item = &'a Ask>) {
        let mut rooms: BTreeMap<String, u64> = BTreeMap::new();
        for a in macs {
            let Some(mb) = super::ask_limit(a) else { continue };
            for step in &a.can {
                let e = rooms.entry(step.clone()).or_default();
                *e = (*e).max(mb);
            }
        }
        self.step_rooms = rooms;
    }

    /// Each Mac's time and jobs' start now (`paces`): the models made again when they changed.
    pub fn set_paces(&mut self, p: crate::paces::Paces) {
        if p != self.paces {
            self.paces = p;
            self.needs_rev += 1;
        }
    }

    /// The jobs' records now (`records`): the models made again when they changed. Only those of
    /// jobs the costs hold are kept, the only ones a model reads: the timings log's every job's
    /// would grow without end, with each forecast's input archived (crate::agent::fcinputs) and
    /// every model made again at each new job's. (A cost come after its record is read with it at
    /// the next round's.) Kept too: those of jobs the history's events name (`event_jobs`) that
    /// resumed blocks, which tell the stopped runs' time left (`set_stopped`). Whether those
    /// changed: the stopped runs' times are to be worked out again.
    pub fn set_records(&mut self, mut r: crate::needs::Records) -> bool {
        let held: std::collections::BTreeSet<String> =
            self.costs.values().filter_map(|c| c.job).map(|j| j.to_string()).collect();
        r.retain(|j, f| held.contains(j) || (f.resumed > 0 && self.event_jobs.contains(j)));
        let resumed = |r: &crate::needs::Records| {
            r.iter().filter(|(_, f)| f.resumed > 0).map(|(j, _)| j.clone()).collect::<Vec<_>>()
        };
        let again = resumed(&r) != resumed(&self.records);
        if r != self.records {
            self.records = r;
            self.needs_rev += 1;
        }
        again
    }

    /// The needs model's percentiles and windows now (`tuning`): the models made again when they
    /// changed.
    pub fn set_tuning(&mut self, t: jobrt::needs::Tuning) {
        if t != self.tuning {
            self.tuning = t;
            self.needs_rev += 1;
        }
    }

    /// How long `target` of `step` is measured to take `worker` (s): what is left of its run that
    /// stopped short (`stopped`) at the worker's Mac's pace, as the forecast takes it
    /// (crate::agent::forecast::target_cost); else as it ran its threads (`secs_on`).
    pub(super) fn secs_of(&self, step: &str, target: &str, worker: &str) -> Option<u64> {
        if let Some(&left) = self.stopped.get(step).and_then(|t| t.get(target)) {
            return Some((left * self.paces.get(worker)).round() as u64);
        }
        self.secs_on(step, target, worker, 0).map(|s| s.0)
    }

    /// What is left of each target whose run stopped short, from the history's `events` (oldest
    /// first; crate::agent::forecast::partials, a step resuming by its jobs' `records`), each at
    /// the lead's pace by the Macs' speeds (`paces`); the jobs the events name kept
    /// (`event_jobs`, for `set_records`).
    pub fn set_stopped(&mut self, events: &[crate::coord::history::Event]) {
        self.event_jobs = events.iter().filter_map(|e| e.job).map(|j| j.to_string()).collect();
        let paces = &self.paces;
        let left = crate::agent::forecast::partials(self.the_plan, events, &self.records, &|secs, worker, _| {
            secs / worker.map_or(1.0, |w| paces.get(w))
        });
        self.stopped.clear();
        for ((step, t), secs) in left {
            self.stopped.entry(step).or_default().insert(t, secs);
        }
    }

    /// `targets` of `step` finished: none has a stopped run left (`stopped`).
    pub fn clear_stopped(&mut self, step: &str, targets: &[String]) {
        if let Some(t) = self.stopped.get_mut(step) {
            for x in targets {
                t.remove(x);
            }
            if t.is_empty() {
                self.stopped.remove(step);
            }
        }
    }

    /// How long `target` of `step` is measured to take `worker` on `threads` (s; 0: as it ran), and
    /// whether at its Mac's measured speed: a drop its own run hasn't measured, its step's drops'
    /// (`drop_of`); else crate::needs::Book::secs_on (its own run at the worker's Mac's speed, else
    /// its step's model there); None when none of its step's has run the way it runs now.
    pub(crate) fn secs_on(&self, step: &str, target: &str, worker: &str, threads: u32) -> Option<(u64, bool)> {
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
        let own = self.book().own(step, target).is_some();
        if own || self.drops.contains(&(step.to_string(), target.to_string())) {
            return Some(s);
        }
        Some(s.max(self.book().secs_p90(step, target, threads, Some(worker)).unwrap_or(0)))
    }

    /// The threads a job of `step` starting with `first` is sized at on a Mac of `total` cores: the
    /// grant rule's (`lease_cores`) for it alone.
    pub fn sizing_threads(&self, step: &str, first: &str, total: u32) -> u32 {
        self.lease_cores(step, &[first.to_string()], total).0
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
    /// itself as a drop, or none of its step's drops ran yet (its step's model stands). Its own
    /// run as a build (a target that ran before it became a drop, task #1087's hollow deep targets)
    /// says nothing of it.
    fn drop_of(&self, b: &crate::needs::Book, step: &str, target: &str) -> Option<(u64, u64)> {
        if !self.drops.contains(&(step.to_string(), target.to_string())) {
            return None;
        }
        if b.own(step, target).is_some_and(crate::needs::is_drop) {
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
    /// over their times together, `NasWeigher::job`, jobrt::budget::nas; 0 for a target none of whose step's runs measured
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
        let k = match v.iter().position(|k| k.worker == l.worker && k.work == l.work && k.start == l.start_threads) {
            Some(k) => k,
            None => {
                v.push(LeaseKept {
                    worker: l.worker.clone(),
                    work: l.work.clone(),
                    start: l.start_threads,
                    nas: Vec::new(),
                    need: None,
                });
                v.len() - 1
            }
        };
        f(&mut v[k])
    }

    /// `job_nas` of leased job `l` (of `step` over `targets`) on its worker, kept (`kept_leases`):
    /// the NAS budget weighs every job leased for each offer an ask weighs, a job of 30 targets each
    /// time.
    fn lease_nas(&self, nw: &NasWeigher, l: &Lease, step: &str, targets: &[(String, String)]) -> (u32, bool) {
        if let Some(w) = self.kept_lease(l, |k| k.nas.iter().find(|n| n.0.weighs_as(&nw.limits)).map(|n| n.1)) {
            return w;
        }
        let got = Self::job_nas(nw, step, Some(&l.worker), targets.iter().map(|t| t.0.as_str()));
        self.kept_lease(l, |k| k.nas.push((nw.limits, got)));
        got
    }

    /// Leased job `l`'s (of `step` over `targets`) packing need on its worker (`pack_need_on`: its
    /// memory on the threads its grant was read at: those it started on for a step whose jobs keep
    /// to their threads (`keeps_threads`), else the threads it starts on by the cost book or those
    /// it was granted where more, a resumed job's the whole Mac's) and its measured time there as
    /// it ran its threads (`batch_secs`), kept (`kept_leases`): each ask packs beside every job its
    /// Mac holds.
    fn lease_need(&self, l: &Lease, step: &str, targets: &[(String, String)]) -> (Need, Option<u64>) {
        if let Some(n) = self.kept_lease(l, |k| k.need) {
            return n;
        }
        let ts: Vec<String> = targets.iter().map(|t| t.0.clone()).collect();
        let start = if l.start_threads > 0 { l.start_threads } else { l.threads };
        let on = self.read_at(step, ts.iter().map(String::as_str), start);
        let got = (
            self.pack_need_on(step, &ts, Some(on)),
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
            .map_or(jobrt::budget::nas::OWN_AGE.as_secs(), |s| s.0);
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
                let aged_by = now.checked_sub(jobrt::budget::nas::OWN_AGE);
                let fresh_from = now.checked_sub(Duration::from_secs(120));
                let held = self
                    .nas_waits
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    // (Aged `OWN_AGE` and passed over in the last two minutes, as instants compared.)
                    .filter(|(_, (since, last, _))| {
                        aged_by.is_some_and(|a| *since <= a) && fresh_from.is_none_or(|f| *last > f)
                    })
                    .min_by(|x, y| (x.1 .0, x.0).cmp(&(y.1 .0, y.0)))
                    .map(|(k, (since, ..))| (*since, k.clone()));
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
    /// nothing's held, it's the one held for, or both fit within the window `win`.
    pub(super) fn nas_reserve_ok(
        reserved: Option<&(String, String, u32)>,
        step: &str,
        targets: &[String],
        running: u32,
        w: u32,
        win: u32,
    ) -> bool {
        w == 0
            || reserved.is_none_or(|(st, t, rw)| {
                (st == step && targets.contains(t)) || running.saturating_add(w).saturating_add(*rw) <= win
            })
    }

    /// The least room a job passed over for the NAS budget alone in the last two minutes needed of
    /// the window beside what was in use, as the refusal itself weighed it (`nas_wait`'s `need`: at
    /// the asker's pace, the whole job it would have made, the room held for an aged target added
    /// where that held it back, `nas_reserve_ok`), a quiet step's held back for a quiet NAS among
    /// them (`quiet_waits`): what the pacing's window must take beside what's in use to be what
    /// limits (jobrt::budget::nas::Pace::read). One number from the decision that held the work
    /// back, never weighed again another way (task #1417 round 4: weighed again per target at the
    /// lead's pace, the lead's own job of two held back read as needing its lighter target's room,
    /// and the window, read as not what limits, never grew). None: none waits.
    pub(super) fn nas_wants(&self, now: Instant) -> Option<u32> {
        let fresh = |last: &Instant| now.saturating_duration_since(*last) < Duration::from_secs(120);
        let waits: Vec<u32> = self
            .nas_waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|w| fresh(&w.1))
            .filter_map(|w| w.2)
            .collect();
        let quiet = self.quiet_waits.lock().unwrap_or_else(|e| e.into_inner());
        waits.into_iter().chain(quiet.values().filter(|q| fresh(&q.0)).map(|q| q.1)).min()
    }

    /// Whether a target, or a quiet step, was passed over for the NAS budget alone in the last two
    /// minutes (`nas_wants`): the pacing has work to judge.
    pub(super) fn nas_waiting(&self, now: Instant) -> bool {
        self.nas_wants(now).is_some()
    }

    /// How many targets were passed over for the NAS budget alone at `now` (`nas_wait`).
    pub(super) fn nas_waits_at(&self, now: Instant) -> usize {
        self.nas_waits.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|w| w.1 == now).count()
    }

    /// Notes `target` of `step` passed over for the NAS budget alone now, the refusal needing
    /// `need` of the window beside what was in use (`nas_wants`).
    pub(super) fn nas_wait(&self, step: &str, target: &str, need: u32, now: Instant) {
        let mut g = self.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
        let e = g
            .entry((step.to_string(), target.to_string()))
            .and_modify(|e| (e.1, e.2) = (now, Some(need)))
            .or_insert((now, now, Some(need)));
        let since = e.0;
        drop(g);
        let aged = now.saturating_duration_since(since) >= jobrt::budget::nas::OWN_AGE;
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
    /// the most of their memory, each on the threads the job starts on (`start_threads`: a target
    /// runs on its job's threads), their disks summed or the most a job of the step took); None
    /// when any is a cold start.
    pub(super) fn job_need(&self, step: &str, targets: &[String]) -> Option<Measured> {
        self.job_need_on(step, targets, None)
    }

    /// `job_need` on `threads` (None: those it starts on, `start_threads`).
    pub(super) fn job_need_on(&self, step: &str, targets: &[String], threads: Option<u32>) -> Option<Measured> {
        let on = match (threads, targets) {
            (Some(t), _) => Some(t),
            (None, [_]) => None,
            (None, _) => Some(self.start_threads(step, targets.iter().map(String::as_str))),
        };
        let each: Vec<Measured> = targets.iter().map(|t| self.measures(step, t, on).1).collect::<Option<_>>()?;
        let job = self.job_floor(step);
        Some(Measured {
            mem_mb: each.iter().map(|m| m.mem_mb).max().unwrap_or(0),
            disk_mb: crate::needs::batch_disk(step, each.iter().map(|m| m.disk_mb)).max(job),
            own: each.iter().all(|m| m.own),
        })
    }

    /// What a job of `step` over `targets` is packed as (task #499): its measure (`job_need`), else,
    /// a cold start (`Need::cold`), its provisional need (`provisional`); its cores, measured, the
    /// cost book's (`book_cores`), the threads it starts on (its agent controls them from there:
    /// docs/pool.md §7.1, Threads controlled live).
    pub(super) fn pack_need(&self, step: &str, targets: &[String]) -> Need {
        self.pack_need_on(step, targets, None)
    }

    /// `pack_need` where its Mac's room for it is `room_mb` (MB): its memory on the threads it fits
    /// on (`mem_threads`) and its grant within them, its busy cores too.
    pub(super) fn pack_need_in(&self, step: &str, targets: &[String], room_mb: u64) -> Need {
        let on = self.mem_threads(step, targets, room_mb);
        let mut n = self.pack_need_on(step, targets, Some(on));
        if let Some(c) = n.cores.as_mut().filter(|c| on > 0 && c.grant > on) {
            c.grant = on;
            c.busy_tenths = c.busy_tenths.map(|b| b.min(on * 10));
        }
        n
    }

    /// `pack_need` with its memory on `threads` (None: those it starts on, `job_need_on`).
    pub(super) fn pack_need_on(&self, step: &str, targets: &[String], threads: Option<u32>) -> Need {
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
            return Need { m, cores, cold, waits: false, idle: false, held: 0, busy_now: None };
        }
        match self.job_need_on(step, targets, threads) {
            Some(m) => {
                let cores = book_cores(&self.book(), step, targets);
                Need { m, cores, cold: false, waits: false, idle: false, held: 0, busy_now: None }
            }
            // (No figure (`no_figure`): it runs alone, so it's counted at none.)
            None => Need {
                m: self.provisional(step, targets).unwrap_or_default(),
                cores: None,
                cold: true,
                waits: false,
                idle: false,
                held: 0,
                busy_now: None,
            },
        }
    }

    /// The threads a job of `step` over `targets`, on a Mac of `total` cores for its jobs
    /// (`ask_cores`: the in-use share's taken off), is granted to start on, and the cores it counts
    /// for there in tenths: `need_cores` of its `pack_need`. The grant rule in one place: the live
    /// scheduler's (`pack`) and the forecast's (`pack` on this scheduler as recorded, `State`). What
    /// the job would be granted alone: the cores the Mac's other jobs count for aren't taken off
    /// (the caller's).
    pub fn lease_cores(&self, step: &str, targets: &[String], total: u32) -> (u32, u32) {
        self.need_cores(step, &self.pack_need(step, targets), total)
    }

    /// The threads a job packed as `n` is granted to start on a Mac of `total` cores, and the cores
    /// it counts for there, in tenths of a core (task #688; tenths, task #697: a job's busy cores
    /// rounded to the nearest undercounted up to half a core each). Measured: its grant
    /// (crate::needs::Book::cores: the cores it kept busy and a thread more) within the Mac's
    /// cores, counted for the cores it kept busy (`busy_tenths`, a climb's thread among them; its
    /// threads where its runs said no CPU time). No fixed share caps or floors a measured job (task
    /// #888: the M4's terrain jobs were held to 3 of 14 cores, 2.4 of them busy, while the Mac sat at
    /// a load of 4; task #896: its runs, not a share, say what it uses). Not said (no run said its
    /// threads), or a cold start of a step honouring its threads (`THREADS_HONOURED`), a thread both
    /// (task #896: its agent grows it from there while it keeps them busy, docs/pool.md §7.1, Threads
    /// controlled live); another cold start the whole Mac both (task #499 review: one that runs as
    /// many threads as the Mac has would swamp the jobs beside it).
    /// One only waiting on a worker, or leaving its cores idle, counts for none. A job held counts for
    /// the threads its lease counts (`Need::held`, what its agent says it runs on: docs/pool.md §7.1,
    /// Threads controlled live), not its grant: task #948, a job granted 3 of 14 threads beside
    /// others counted for its measured 13 kept its Mac looking full after they ended; a job its
    /// agent grew counts for its threads grown.
    pub(super) fn need_cores(&self, step: &str, n: &Need, total: u32) -> (u32, u32) {
        let (threads, busy) = if self.the_plan.records_only(step) {
            (1, 0)
        } else if n.cold && !THREADS_HONOURED.contains(&step) {
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
        let threads = if n.held > 0 && !self.the_plan.records_only(step) { n.held } else { threads };
        let busy = n.busy_now.unwrap_or(busy);
        (threads, if n.waits || n.idle { 0 } else { busy.min(threads * 10) })
    }

    /// `need_cores` of a job of `step` over `targets` held on `held` threads (0: not held), for a
    /// test.
    #[cfg(test)]
    pub(super) fn held_cores(&self, step: &str, targets: &[String], held: u32, total: u32) -> (u32, u32) {
        self.need_cores(step, &Need { held, ..self.pack_need(step, targets) }, total)
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
    /// its step's runs of its latest earlier way measured (crate::needs::Book::older_mem: its cost
    /// version moved, task #979) or it was offered with (`offered_mb`), the disk its step's
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
        let p = (b.older_mem(step, target).unwrap_or(0).max(self.offered_mb(step, target)), b.disk(step, target));
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
    /// (a member's, placement on, by their steps' slack first: `placement`), each by `pick_every`,
    /// the lead's own planned jobs by `pick_own`, those to `lead` alone
    /// (crate::agent::every::grants), up to the jobs it wants (`Ask::want`, at least one). Each after
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
    /// of the lead's own: `own_cold_waits`). And why it granted fewer jobs than the ask wanted:
    /// the rule and in words (`Held`; None: as many as it wanted).
    pub fn pack(&mut self, a: &Ask, offers: &[Offer], lead: bool, clock: Clock) -> (Vec<Pick>, Option<Held>) {
        *self.packing.lock().unwrap_or_else(|e| e.into_inner()) = Some(Packing::default());
        *self.placed.lock().unwrap_or_else(|e| e.into_inner()) = Placed::default();
        let got = self.pack_kept(a, offers, lead, clock);
        *self.packing.lock().unwrap_or_else(|e| e.into_inner()) = None;
        got
    }

    /// `pack`, its look-ups of the leases kept while it packs (`packing`).
    fn pack_kept(&mut self, a: &Ask, offers: &[Offer], lead: bool, clock: Clock) -> (Vec<Pick>, Option<Held>) {
        let now = clock.now;
        // (Its Mac below its memory floor, the brake on or a give-back cooling it down (docs/pool.md
        // §7.1): nothing, a cold start to a Mac running nothing among it.)
        if let Some(why) = a.cap.as_ref().and_then(|c| c.mem_held_back.as_ref()) {
            return (Vec::new(), Some(held_by(Rule::Memory, format!("memory: {why}"))));
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
                // (Its agent's grows past its measure: `grown_mb`.)
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
            let words = format!("{st} has no measure to size it by: nothing goes beside its first job");
            return (Vec::new(), Some(held_by(Rule::Cold, words)));
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
        // (Its Mac's memory once its jobs end, the disk left out: what a job held back for its disk
        // must fit for its disk to be named, `Held::disk_mb`; its cores always do, `need_cores`
        // granting within them. Its ceiling, not the room the memory has now: a Mac under pressure
        // still frees its caches for a job its memory takes once the pressure eases.)
        let whole = super::ask_limit(a).unwrap_or(a.mem_mb);
        // (And the disk it can give such a job, its caches freed: `disk_reach`.)
        let reach = disk_reach(a);
        // (The jobs it holds as room held for a waiting job weighs them (`Backfill`): how long each
        // has run, `Lease::ran`.)
        let mut runs: Vec<Run> = jobs
            .iter()
            .zip(&needs)
            .map(|(&(l, st, ts), n)| Run::of(n.m, self.need_cores(st, n, total).1, self.run_end(l, st, ts, now)))
            .collect();
        // (The cores left were its jobs counted at their threads, as before task #688: a grant
        // past them is an overcommit, trusting its busy cores (task #688 review). At most one per
        // ask, and none while its Mac's load hasn't had `SETTLE` to show its last grant or shows
        // load past its jobs' busy cores by more than a core (the minute's whole cores): the jobs go up one at a time, each measured by the load
        // before the next, so jobs busier than their runs said (or #690's reads in parallel) stop
        // the climb within a minute instead of a whole ask's worth landing at once. A job older
        // than its lease (`Lease::taken_up`: read back at a lead's restart, or from a record, the
        // forecast's) has had its load show since it began (task #1094: the forecast held back
        // the overcommit the lead made, its record's leases all granted that instant). Its
        // agent restarted too, the load still shows it: its busy cores are its CPU's ticks a
        // second, the minute's read back from its disk (crate::agent::live), else the kernel's
        // load average.)
        let mut by_threads = needs
            .iter()
            .zip(&jobs)
            .fold(total, |left, (n, (_, st, _))| {
                left.saturating_sub(if n.waits || n.idle { 0 } else { self.need_cores(st, n, total).0 })
            })
            .saturating_sub(outside);
        let settled = mine.iter().all(|l| now.saturating_duration_since(l.granted.min(l.began())) >= SETTLE);
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
        // (What's taken off its disk past none left (MB): owed of the whole copies before any is
        // counted free (`Copies`). Its own ask's none is its agent's count of the disk at 0, what
        // its target and reservations take past its free space at most (its cheap caches not said).)
        let mut owed = a
            .cap
            .as_ref()
            .filter(|_| a.disk_mb == Some(0))
            .map_or(0, |c| (c.target_mb + c.reserved_mb).saturating_sub(c.free_mb));
        let mut take_disk = |room: &mut Ask, mb: u64| {
            if let Some(d) = room.disk_mb.as_mut() {
                owed += mb.saturating_sub(*d);
                *d = d.saturating_sub(mb);
            }
        };
        for ((l, _, _), n) in jobs.iter().zip(&needs) {
            room.mem_mb = room.mem_mb.saturating_sub(n.m.mem_mb);
            if !reserved_by_agent(l) {
                take_disk(&mut room, n.m.disk_mb);
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
                    take_disk(&mut room, self.missing(c, a));
                }
            }
        }
        // (The whole copies of those none of them reads: freed for a job reading none either, less
        // what's owed of them.)
        let mut copies = Copies::of(a, lead, &caches);
        copies.take("", owed);
        // (The jobs it may be granted: as many as it wants, one at least.)
        let want = a.want.max(1);
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
                let words = format!("a cold job of {} waits for the lead's jobs to end", o.step);
                return (Vec::new(), Some(held_by(Rule::Cold, words)));
            }
            let fill = self.cache_fill(&o.step, &caches, a);
            let room_o = Ask { disk_mb: room.disk_mb.map(|d| d.saturating_sub(fill)), ..room.clone() };
            if self.pick_own(o, &room_o, now).is_empty() {
                cold_own.clear();
            }
        }
        let offers = if cold_own.is_empty() { offers } else { &cold_own[..] };
        // (Placement on, a member's ask tries the long poles' steps first, by their slack in the
        // forecast (crate::agent::placement), plan order between equals; the lead's own in plan
        // order. Here, so the forecast's run tries them in the order the lead does: task #1163, its
        // members granted terrain-deep where the lead granted them slope-deep.)
        let mut offers: Vec<&Offer> = offers.iter().collect();
        if let Some((p, _)) = self.placement.as_ref().filter(|_| !lead) {
            offers.sort_by_key(|o| p.slack_of(&o.step));
        }
        let mut held = mine.len();
        let mut grants: Vec<Pick> = Vec::new();
        // (Why the last round granted nothing: a job open but short of cores, or nothing open in
        // the room left.)
        let mut why: Option<Held> = None;
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
            let mut held_why: Option<Held> = None;
            // (The offers granted nothing, each with its fill and the whole copies counted for it:
            // the room rule's words, `refused`.)
            let mut refused: Vec<(&Offer, u64, u64)> = Vec::new();
            for &o in &offers {
                if !crate::agent::every::grants(self.the_plan, &o.step, o.job.is_some(), lead) {
                    continue;
                }
                // (A job reading a shared cache none here reads yet: its room less the cache's, and
                // less what the jobs granted before it spent of the whole copies held of it; one
                // reading none of them: its room with them, `Copies`.)
                let fill = self.cache_fill(&o.step, &caches, a) + copies.spent(&o.step);
                let credit = copies.credit(&o.step);
                let room_o = disk_with(&room, fill, credit);
                let pick =
                    if o.job.is_some() { self.pick_own(o, &room_o, now) } else { self.pick_every(o, &room_o, now) };
                if pick.is_empty() {
                    refused.push((o, fill, credit));
                    if waiting.is_none() {
                        // (Its whole room, and of the disk what its caches can free too: a job
                        // whose disk they give is held for, its memory and cores kept as they're
                        // freed, `Held::disk_mb`.)
                        let whole = disk_with(a, fill, credit.saturating_add(caches_free(a)));
                        waiting = self.room_wait(o, &whole, &room_o, now).map(|(m, ts)| {
                            // (Its cores on the threads its memory was read at in that room.)
                            let n = self.pack_need_in(&o.step, &ts, whole.mem_mb);
                            let w = RoomWait {
                                step: o.step.clone(),
                                target: ts.first().cloned().unwrap_or_default(),
                                mem_mb: m.mem_mb,
                                disk_mb: m.disk_mb + fill,
                                cores: self.need_cores(&o.step, &n, total).1,
                                credit,
                            };
                            Backfill::of(w, &room, cores, &runs)
                        });
                    }
                    continue;
                }
                let ts: Vec<String> = pick.iter().map(|t| t.0.clone()).collect();
                let n = self.pack_need_in(&o.step, &ts, room_o.mem_mb);
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
                            target: ts.first().cloned().unwrap_or_default(),
                            mem_mb: n.m.mem_mb,
                            disk_mb: n.m.disk_mb + fill,
                            cores: need,
                            credit,
                        };
                        waiting = Some(Backfill::of(w, &room, cores, &runs));
                    }
                    continue;
                }
                let p90 = || self.batch_p90(&o.step, ts.iter().map(String::as_str), &a.worker);
                if let Some(w) = waiting.as_ref().filter(|w| !w.lets(&n.m, fill, need, p90)) {
                    held_why.get_or_insert_with(|| w.held(&room, cores, whole, reach, true));
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
                let nas_waits = self.nas_waits_at(now);
                why = Some(match short {
                    _ if held_why.is_some() => held_why.unwrap_or_else(|| held_by(Rule::Waiting, String::new())),
                    None if cold_why.is_some() => held_by(Rule::Cold, cold_why.unwrap_or_default()),
                    None if ramp_why.is_some() => held_by(Rule::Settle, ramp_why.unwrap_or_default()),
                    Some((st, c)) if outside > 0 => held_by(
                        Rule::Outside,
                        format!(
                            "cores: {} of {total} left ({outside} busy outside the pool), a job of {st} takes {}",
                            tenths(cores),
                            tenths(c)
                        ),
                    ),
                    Some((st, c)) => held_by(
                        Rule::Cores,
                        format!("cores: {} of {total} left, a job of {st} takes {}", tenths(cores), tenths(c)),
                    ),
                    // (Room held for the first offer refused for its room, nothing after it held
                    // back for it: that hold is why, task #1170; a lead whose last offer waited had
                    // said only the room left, and no hold.)
                    None if nas_waits == 0 && waiting.is_some() => waiting.as_ref().map_or_else(
                        || held_by(Rule::Waiting, String::new()),
                        |w| w.held(&room, cores, whole, reach, false),
                    ),
                    None if refused.iter().all(|(o, ..)| o.targets.is_empty()) => self.apart(a, lead),
                    None => {
                        let (r, credit) = self.refused(&refused, a, &room, now);
                        let disk_mb = r.disk_alone(&room, credit, reach, whole);
                        // (Past what its disk can give, its caches freed: none freed for it.)
                        let past = r.past_reach(credit, reach).map_or(String::new(), |n| {
                            format!(
                                "; its {:.1} GB is past what this Mac's disk can give with its caches freed ({:.1} GB): none are freed for it",
                                n as f64 / 1024.0,
                                reach as f64 / 1024.0
                            )
                        });
                        let held = held_by(
                            if nas_waits == 0 { Rule::Room } else { Rule::Nas },
                            format!(
                                "{}{}{past}",
                                // (Targets it would take passed over for the NAS budget alone in
                                // this answer (`nas_wait`): said first, so a wait for the NAS
                                // doesn't read as one for memory (task #931: on 4 Nov every
                                // slope-deep job weighed the whole NAS, and the Macs idle beside
                                // one said only the room left).)
                                match nas_waits {
                                    0 => String::new(),
                                    n => format!(
                                        "the NAS budget: {n} target{} it would take wait for the NAS's room; else ",
                                        if n == 1 { "" } else { "s" }
                                    ),
                                },
                                r.words(a, &disk_with(&room, 0, credit), credit)
                            ),
                        );
                        let held_for =
                            r.blocked.as_ref().filter(|_| disk_mb.is_some()).map(|b| (b.0.clone(), b.1.clone()));
                        Held { disk_mb, held_for, ..held }
                    }
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
            let fill = self.cache_fill(&o.step, &caches, a) + copies.spent(&o.step);
            // (A job keeping to its threads: its memory on those it's granted, `read_at`.)
            let m = match keeps_threads(&o.step) && !n.cold {
                true => self.job_need_on(&o.step, &ts, Some(threads)).unwrap_or(n.m),
                false => n.m,
            };
            runs.push(self.run_of(&o.step, &ts, &a.worker, Measured { disk_mb: m.disk_mb + fill, ..m }, busy));
            grants.push(self.grant(a, o, pick, threads, m.mem_mb, fill, clock));
            held += 1;
            caches.extend(crate::needs::cache_of(&o.step));
            cold_here |= n.cold;
            room.mem_mb = room.mem_mb.saturating_sub(m.mem_mb);
            // (Past the disk left, it takes the whole copies counted for it: `Copies`.)
            copies.take(&o.step, room.disk_mb.map_or(0, |d| (m.disk_mb + fill).saturating_sub(d)));
            room.disk_mb = room.disk_mb.map(|d| d.saturating_sub(m.disk_mb + fill));
            cores = cores.saturating_sub(busy);
            if beside && threads > by_threads {
                over_ok = false;
            }
            by_threads = by_threads.saturating_sub(threads);
        }
        let why = why.filter(|_| grants.len() < want);
        (grants, why)
    }

    /// What each thread a job of `step` over `targets`, admitted at `mem_mb` on `on` threads (those
    /// its memory was read at, `read_at`), adds to its memory past them (MB), and
    /// the threads its memory covers: its targets' most (crate::needs::Book::thread_mb: a target's
    /// own run's peak over its threads by its step's rule, the fitted memory per thread, at least
    /// `needs-thread-share` of that, or that whole where none is fitted; one never
    /// run, its step's memory per thread, fitted over its runs' sizes and threads, else its runs'
    /// peaks over their threads); the threads, `on`, or its own runs' where every target ran on
    /// more (`needs::own_threads`: its peak came on them). Where a target's isn't said, the whole
    /// figure a thread and one covered (one more thread of a regions check once needed about a
    /// region's worth, docs/inputs.md §4.3); none (0) where it has no figure, a cold start's,
    /// which its agent grows past no thread its grant doesn't cover (crate::agent::threads::decide).
    pub(super) fn thread_mb<'t>(
        &self,
        step: &str,
        targets: impl IntoIterator<Item = &'t str> + Clone,
        mem_mb: u64,
        on: u32,
    ) -> (u64, u32) {
        let b = self.book();
        let per = targets.clone().into_iter().map(|t| b.thread_mb(step, t)).collect::<Option<Vec<u64>>>();
        let Some(per) = per.map(|v| v.into_iter().max().unwrap_or(0)) else { return (mem_mb, 1) };
        let ran = targets
            .into_iter()
            .map(|t| b.own(step, t).and_then(crate::needs::own_threads))
            .collect::<Option<Vec<u32>>>()
            .and_then(|v| v.into_iter().min())
            .unwrap_or(0);
        (per, on.max(ran).max(1))
    }

    /// The memory (MB) lease `l`'s job of `step` over `targets`, admitted at `mem_mb` (`pack_need`:
    /// on the threads it starts on), may hold past it: its threads grown by its agent (docs/pool.md
    /// §7.1, Threads controlled live) past those its memory covers (the threads it started on,
    /// `Lease::start_threads`, and those it was read at), each its `thread_mb`. Its Mac's agent
    /// counts it at its admission alone in its capacity (crate::agent's `capacity`; it reserves the
    /// same for its own grows, crate::agent::threads), so the lead adds this (`pack`).
    pub(super) fn grown_mb<'t>(
        &self,
        l: &Lease,
        step: &str,
        targets: impl IntoIterator<Item = &'t str> + Clone,
        mem_mb: u64,
    ) -> u64 {
        let start = if l.start_threads > 0 { l.start_threads } else { l.threads };
        // (Not grown past its start: nothing, whatever its memory covers.)
        if l.threads <= start {
            return 0;
        }
        let on = self.read_at(step, targets.clone(), start);
        let (per, covered) = self.thread_mb(step, targets, mem_mb, on);
        per * u64::from(l.threads.saturating_sub(start.max(covered)))
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
        self.leases.asked(lease, a.sent);
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
            let aged_by = clock.now.checked_sub(jobrt::budget::nas::OWN_AGE);
            let from = (o.step.clone(), String::new());
            let aged: Vec<String> = g
                .range(from..)
                .take_while(|((st, _), _)| *st == o.step)
                .filter(|(_, (since, ..))| aged_by.is_some_and(|a| *since <= a))
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
            self.quiet_waits.lock().unwrap_or_else(|e| e.into_inner()).remove(&o.step);
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
            let at = self.sizing_threads(&o.step, first, ask_cores(a));
            self.batch_secs(&o.step, ts.iter().map(String::as_str), &a.worker, at)
        });
        let on = self.read_at(&o.step, ts.iter().map(String::as_str), threads);
        let (thread_mb, covered) = self.thread_mb(&o.step, ts.iter().map(String::as_str), mem_mb, on);
        let why = ts.first().and_then(|t| {
            self.placed.lock().unwrap_or_else(|e| e.into_inner()).why.remove(&(
                a.worker.clone(),
                o.step.clone(),
                t.clone(),
            ))
        });
        let nas = self.nas_room(clock.now, a.worker == self.lead_name).and_then(|(nw, ..)| {
            let l = self.leases.get(lease, &a.worker, clock.now)?;
            let (w, _) = self.lease_nas(&nw, l, &o.step, &pick);
            let names: Vec<&str> = if ts.is_empty() { vec![""] } else { ts.iter().map(String::as_str).collect() };
            Some((w, names.iter().any(|t| nw.book.nas_for(&o.step, t, &a.worker).is_some())))
        });
        Pick {
            lease,
            step: o.step.clone(),
            targets: pick,
            threads,
            mem_mb,
            thread_mb,
            thread_covers: threads.max(covered),
            disk_mb,
            job: o.job.clone(),
            sized,
            why,
            nas,
        }
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
            !held.contains(t)
                && self.done.get(&(o.step.clone(), t.clone())) != Some(k)
                && !self.backoff(&a.worker, &o.step, t, now)
        })
    }

    /// What offer `o` waits for when refused for its room alone (`pack`, task #965): the measure
    /// (memory, disk) of the job `a`'s worker would be granted of it with its whole room `whole`
    /// (every job it holds ended, its disk with what its caches can free: `caches_free`), when the
    /// room left `room` doesn't fit it: the lead's own planned job whole, or the target a step's
    /// pick starts with there (`pick_run`, its waits on the NAS not noted: placement on, the
    /// longest that finishes soonest on this Mac, not the step's first in plan order, which placement may take
    /// last). None when there's none (nothing open, a cold start: `own_cold_waits`'s, or none
    /// that ever fits this Mac: the room isn't held for what can't go here), when the pick there
    /// starts with a target not measured (a cold start: the cold rules decide it, no room held),
    /// or when that one fits the room left (held back for another reason, the NAS budget's say).
    /// With it, the targets it's of.
    pub(super) fn room_wait(
        &self,
        o: &Offer,
        whole: &Ask,
        room: &Ask,
        now: Instant,
    ) -> Option<(Measured, Vec<String>)> {
        let job_disk = self.job_floor(&o.step);
        let fits = |m: &Measured, r: &Ask| m.mem_mb <= r.mem_mb && r.disk_mb.is_none_or(|d| m.disk_mb <= d);
        // (Each on the threads it fits that room on, `mem_threads`.)
        let need = |ts: &[String], r: &Ask| {
            let m = self.job_need_in(&o.step, ts, r.mem_mb)?;
            Some(Measured { disk_mb: m.disk_mb.max(job_disk), ..m })
        };
        let ts: Vec<String> = if o.job.is_some() {
            if !self.own_open(o, whole, now) {
                return None;
            }
            o.targets.iter().map(|t| t.0.clone()).collect()
        } else {
            vec![self.pick_run(o, whole, now, false).into_iter().next()?.0]
        };
        let m = need(&ts, whole)?;
        (fits(&m, whole) && !need(&ts, room).is_some_and(|r| fits(&r, room))).then_some((m, ts))
    }

    /// The offered targets (not held by a job nor done) whose measure (`measured`, its step's disk
    /// floor among its disk; the lead's own planned job's whole, `job_need`; each on the threads it
    /// fits the most memory among them on, `mem_threads`) fits no Mac in `macs`
    /// that runs its step at its best (`best_room`, by its last ask): past its memory or its disk
    /// on each (a Mac whose ask bounds no disk, on its memory alone, as `pack` grants it); one
    /// `TooBig` an offer with any, with the Mac of the most memory at its best among them. Targets not measured say nothing.
    pub(super) fn too_big(&self, macs: &[(&str, &Ask)], now: Instant) -> Vec<super::TooBig> {
        let mut out = Vec::new();
        for o in &self.offers {
            let can: Vec<(&str, u64, Option<u64>)> = macs
                .iter()
                .filter(|(_, a)| a.can.contains(&o.step))
                .map(|(n, a)| {
                    let (mem, disk) = best_room(a);
                    (*n, mem, disk)
                })
                .collect();
            let Some(&(mac, mac_mem_mb, mac_disk_mb)) = can.iter().max_by_key(|c| (c.1, c.2)) else { continue };
            let past = |m: &Measured| {
                !can.iter().any(|&(_, mem, disk)| m.mem_mb <= mem && disk.is_none_or(|d| m.disk_mb <= d))
            };
            let (held, done) = (self.held_of(&o.step, now), self.done_of(&o.step));
            let open: Vec<String> = o
                .targets
                .iter()
                .filter(|(t, k, _)| !held.contains(t) && done.get(t.as_str()) != Some(&k.as_str()))
                .map(|t| t.0.clone())
                .collect();
            let job_disk = self.job_floor(&o.step);
            let big: Vec<(String, Measured)> = if o.job.is_some() {
                self.job_need_in(&o.step, &open, mac_mem_mb)
                    .filter(&past)
                    .map(|m| (String::new(), m))
                    .into_iter()
                    .collect()
            } else {
                open.iter()
                    .filter_map(|t| {
                        let m = self.measured_in(&o.step, t, mac_mem_mb)?;
                        Some((t.clone(), Measured { disk_mb: m.disk_mb.max(job_disk), ..m }))
                    })
                    .filter(|(_, m)| past(m))
                    .collect()
            };
            if let Some((target, m)) = big.first() {
                out.push(super::TooBig {
                    step: o.step.clone(),
                    targets: if o.job.is_some() { open.len() } else { big.len() },
                    target: target.clone(),
                    mem_mb: m.mem_mb,
                    disk_mb: m.disk_mb,
                    mac: mac.to_string(),
                    mac_mem_mb,
                    mac_disk_mb,
                });
            }
        }
        out
    }

    /// What became of the targets of the offers `a`'s worker was granted none of in the room left
    /// `room` (`pack`'s room rule, task #1170), each offer with its shared cache's fill and the
    /// whole copies counted free for it (`Copies`): those held by jobs, done, or kept from it after
    /// it failed them, counted, and the first of the rest whose measure (its step's disk floor and
    /// the fill among it; the lead's own planned job's, whole) is past the room; with the whole
    /// copies counted for that one.
    fn refused(&self, offers: &[(&Offer, u64, u64)], a: &Ask, room: &Ask, now: Instant) -> (Refused, u64) {
        let mut r = Refused::default();
        let mut credit = 0;
        for &(o, fill, copies) in offers {
            let (held, done) = (self.held_of(&o.step, now), self.done_of(&o.step));
            let job_disk = self.job_floor(&o.step);
            let own = o.job.as_ref().and_then(|_| {
                self.job_need_in(&o.step, &o.targets.iter().map(|t| t.0.clone()).collect::<Vec<_>>(), room.mem_mb)
            });
            for (t, k, _) in &o.targets {
                if held.contains(t) {
                    r.held += 1;
                } else if done.get(t.as_str()) == Some(&k.as_str()) {
                    r.done += 1;
                } else if self.backoff(&a.worker, &o.step, t, now) {
                    r.backoff += 1;
                    if self.braked(&a.worker, &o.step, now) {
                        let w = brake_words(&o.step, &self.fail_hold);
                        if !r.braked.contains(&w) {
                            r.braked.push(w);
                        }
                    }
                } else if r.blocked.is_none() {
                    let m = if o.job.is_some() { own } else { self.measured_in(&o.step, t, room.mem_mb) };
                    let past =
                        |m: &Measured| m.mem_mb > room.mem_mb || room.disk_mb.is_some_and(|d| m.disk_mb > d + copies);
                    match m.map(|m| Measured { disk_mb: m.disk_mb.max(job_disk) + fill, ..m }).filter(past) {
                        Some(m) => {
                            r.blocked = Some((o.step.clone(), t.clone(), m));
                            credit = copies;
                        }
                        None => r.other += 1,
                    }
                }
            }
        }
        (r, credit)
    }

    /// Why `a`'s worker is offered nothing to take when no offer reached the room rule (none was
    /// refused for its room, held, done or in backoff: none had a target to weigh): the plan's
    /// offered jobs that are not for it, by step in plan order (the lead's own planned jobs to a
    /// member, `every::grants`; a step it doesn't run), or, with none, that nothing is offered.
    /// Worked out from the offers the scheduler holds already, so a status asks nothing more.
    fn apart(&self, a: &Ask, lead: bool) -> Held {
        let mut for_lead: Vec<&str> = Vec::new();
        let mut not_here: Vec<&str> = Vec::new();
        for o in self.offers.iter().filter(|o| !o.targets.is_empty()) {
            let (list, step) = if !crate::agent::every::grants(self.the_plan, &o.step, o.job.is_some(), lead) {
                (&mut for_lead, o.step.as_str())
            } else if !a.can.contains(&o.step) {
                (&mut not_here, o.step.as_str())
            } else {
                continue;
            };
            if !list.contains(&step) {
                list.push(step);
            }
        }
        let say = |list: &[&str], one: &str, many: &str| {
            const SHOWN: usize = 6;
            let n = list.len();
            let names = list.iter().take(SHOWN).copied().collect::<Vec<_>>().join(", ");
            let more = if n > SHOWN { format!(" and {} more", n - SHOWN) } else { String::new() };
            let (s, what) = if n == 1 { ("", one) } else { ("s", many) };
            format!("{n} step{s} offered {what}: {names}{more}")
        };
        let mut said: Vec<String> = Vec::new();
        if !for_lead.is_empty() {
            said.push(say(&for_lead, "is the lead's alone", "are the lead's alone"));
        }
        if !not_here.is_empty() {
            said.push(say(&not_here, "isn't one this Mac runs", "aren't ones this Mac runs"));
        }
        if said.is_empty() {
            held_by(Rule::Empty, EMPTY.to_string())
        } else {
            held_by(Rule::Away, said.join("; "))
        }
    }

    /// A job of `step` over `targets` on `worker`'s Mac, measured `m` and counting for `cores`
    /// (tenths), just granted, as room held for a waiting job weighs it (`Run`).
    fn run_of(&self, step: &str, targets: &[String], worker: &str, m: Measured, cores: u32) -> Run {
        let end = match self.batch_secs(step, targets.iter().map(String::as_str), worker, 0) {
            Some((s, _)) if s > 0 => (Some(s), End::Measured),
            Some(_) => (None, End::Overdue),
            None => (None, End::Unmeasured),
        };
        Run::of(m, cores, end)
    }

    /// When held job `l` of `step` over `targets` is expected to end, at `now` (s from now; None:
    /// it may end any moment), and by what (task #1505): within its measured time (`lease_need`'s,
    /// `batch_secs`), by it; else by its progress's pace where its worker's beats say it
    /// (`Lease::left_by_progress`: the time left its status shows) times how such estimates have
    /// ended (`left_discount`; none ended yet, or a discount to nothing, none from it); else,
    /// measured and past it, by its slowest target's high percentile (`batch_slow`) while it's
    /// within it; else none. A job not measured has no model either (`secs_on`: its step's model
    /// stands in for a target's own run).
    fn run_end(&self, l: &Lease, step: &str, targets: &[(String, String)], now: Instant) -> (Option<u64>, End) {
        let ran = l.ran(now).as_secs();
        let secs = self.lease_need(l, step, targets).1;
        let left = |s: u64| s.checked_sub(ran).filter(|&x| x > 0);
        if let Some(x) = secs.and_then(left) {
            return (Some(x), End::Measured);
        }
        if let (Some(said), Some((ratio, pool))) = (l.left_by_progress(now), self.left_discount(step)) {
            let x = (said as f64 * ratio).floor() as u64;
            if x > 0 {
                return (Some(x), End::Progress { pct: self.left_pct().round() as u32, pool });
            }
        }
        if secs.is_none() {
            return (None, End::Unmeasured);
        }
        match self.batch_slow(step, targets.iter().map(|t| t.0.as_str()), &l.worker).and_then(left) {
            Some(x) => (Some(x), End::Slow),
            None => (None, End::Overdue),
        }
    }

    /// A job of `step` over `targets` on `worker` past its measured time, as the room held for a
    /// waiting job times it (`run_end`): its measured time (`batch_secs`) and its slowest target's
    /// excess at its step's runs' high percentile over its own time (crate::needs::Book::secs_p90):
    /// one target running long, the rest as measured. None when a target has no time.
    pub(super) fn batch_slow<'t>(
        &self,
        step: &str,
        targets: impl IntoIterator<Item = &'t str>,
        worker: &str,
    ) -> Option<u64> {
        let b = self.book();
        let start = self.paces.start(worker, step).round() as u64;
        let (sum, most) = targets.into_iter().try_fold((start, 0), |(sum, most), t| {
            let s = self.secs_on(step, t, worker, 0)?.0;
            let p = b.secs_p90(step, t, 0, Some(worker))?;
            Some((sum + s, most.max(p.saturating_sub(s))))
        })?;
        Some(sum + most)
    }

    /// The percentile a progress end is discounted at: the needs model's low one, as far below the
    /// middle as its high one (`Tuning::high_pct`, the pool's `needs-high-pct`) is above it.
    fn left_pct(&self) -> f64 {
        100.0 - self.tuning.high_pct
    }

    /// What a progress end of a job of `step` is discounted by (`Lefts`): what was left in fact
    /// over what its step's jobs' beats said, at `left_pct` (nearest rank), and whether every step's
    /// (its own having none). None while no estimate has ended.
    fn left_discount(&self, step: &str) -> Option<(f64, bool)> {
        let at = |v: &Vec<f64>| v[((v.len() as f64 * self.left_pct() / 100.0).ceil() as usize).clamp(1, v.len()) - 1];
        let l = &self.lefts;
        match (l.by.get(step), l.by.get(""), l.taken.get(step), l.taken.get("")) {
            (Some(v), _, _, _) => Some((at(v), false)),
            (None, Some(v), _, _) => Some((at(v), true)),
            (None, None, Some(&r), _) => Some((r, false)),
            (None, None, None, Some(&r)) => Some((r, true)),
            _ => None,
        }
    }

    /// How each step's jobs' progress estimates ended (crate::coord::history::History::left_ratios).
    pub fn set_lefts(&mut self, by: BTreeMap<String, Vec<f64>>) {
        self.lefts = Lefts::of(by);
    }

    /// The discounts `left_discount` takes now, each step's that has its own and the pool's under ""
    /// (`State::lefts`).
    fn lefts_taken(&self) -> BTreeMap<String, f64> {
        let keys: Vec<String> = self.lefts.by.keys().chain(self.lefts.taken.keys()).cloned().collect();
        keys.into_iter().filter_map(|k| Some((k.clone(), self.left_discount(&k)?.0))).collect()
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
                || (!self.the_plan.records_only(&o.step) && !THREADS_HONOURED.contains(&o.step.as_str())))
            && self.own_open(o, a, now)
            && self.own_nas_fits(o, now, true)
    }

    /// Whether the NAS budget has room for the lead's own planned job of offer `o`: its weight
    /// within the window or beside nothing weighing anything, a download under the cap (the lead's
    /// own always held to it, `nas_room`); `alone`: as if the lead held nothing (the members' jobs
    /// alone counted: what its cold job would meet once its own jobs end, `own_cold_waits`), a test
    /// that changes nothing (no wait noted, no quiet clock started). Not `alone`, held back for the budget alone, it's noted waiting (`nas_wait`), and goes beside leased jobs
    /// none of which weighs the whole once it weighs the whole itself or has waited `OWN_AGE`
    /// (jobrt::budget::nas::Limits::fits_own_job: members' small jobs never hold it for good).
    pub(super) fn own_nas_fits(&self, o: &Offer, now: Instant, alone: bool) -> bool {
        let ts: Vec<String> = o.targets.iter().map(|t| t.0.clone()).collect();
        let waited = {
            let g = self.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
            ts.iter().filter_map(|t| g.get(&(o.step.clone(), t.clone())).map(|e| e.0)).min()
        }
        .is_some_and(|since| now.duration_since(since) >= jobrt::budget::nas::OWN_AGE);
        self.nas_room_heaviest(now, true).is_none_or(|(nw, running, members, dl, most)| {
            let (w, fetch) = Self::job_nas(&nw, &o.step, Some(&self.lead_name), ts.iter().map(String::as_str));
            let (running, heaviest) = if alone { (members, most[1]) } else { (running, most[0]) };
            if fetch && nw.limits.downloads.is_some_and(|c| dl >= c) {
                return false;
            }
            let own = running.saturating_sub(members);
            // (A quiet step, the clean-up, measured the way it runs now: alone or within the window
            // by its measured weight, never aged past it, its wait not noted: it never holds room,
            // jobrt::budget::nas::QUIET_STEPS.) Held back `QUIET_AGE` (a pool that stays busy), it
            // goes as an aged job of the lead's own, its wait noted so its room is held. Unmeasured,
            // it weighs the whole (`NAS_STEPS`) and goes as the lead's own unmeasured NAS steps do,
            // beside no job weighing the whole (`fits_own_job`), so its first run measures it and no
            // burst of grants lands beside it meanwhile (task #1000: held for a quiet NAS instead,
            // a busy pool never had one, and gc never ran).
            let quiet_step = jobrt::budget::nas::QUIET_STEPS.contains(&o.step.as_str());
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
                now.saturating_duration_since(since) < jobrt::budget::nas::QUIET_AGE
            };
            let waited = waited || (quiet_step && measured && !quiet);
            let rules = if quiet {
                jobrt::budget::nas::Limits::fits_quiet(running, w, nw.limits.win())
            } else {
                jobrt::budget::nas::Limits::fits_own_job(running, own, heaviest, w, waited, nw.limits.win())
            } && nw.limits.paced(running, w);
            let reserved = if alone { None } else { self.nas_reserved(&nw, now) };
            let reserve_ok = Self::nas_reserve_ok(reserved.as_ref(), &o.step, &ts, running, w, nw.limits.window);
            let fits = rules && reserve_ok;
            // (Held back, the room it needs of the window beside what's in use, as weighed here:
            // the job's, and the room held for an aged target where that held it back.)
            let need = w.saturating_add(if reserve_ok { 0 } else { reserved.map_or(0, |r| r.2) });
            if !fits && !alone && !quiet {
                for t in &ts {
                    self.nas_wait(&o.step, t, need, now);
                }
            }
            // (A quiet step held back where a window would take it: for the pacing's growth alone,
            // `nas_wants`. Weighing nothing, only nothing running lets it go: no window helps.)
            if !fits && !alone && quiet && need > 0 {
                self.quiet_waits.lock().unwrap_or_else(|e| e.into_inner()).insert(o.step.clone(), (now, need));
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
        let fits = match self.job_need_in(&o.step, &ts, a.mem_mb) {
            Some(m) => m.mem_mb <= a.mem_mb && a.disk_mb.is_none_or(|d| m.disk_mb <= d),
            None => self.cold_fits(&o.step, &ts, a, now),
        };
        // (The budget asked last: its wait noted only when the job would go but for it.)
        if !fits || !self.own_nas_fits(o, now, false) {
            return Vec::new();
        }
        o.targets.iter().map(|(t, k, _)| (t.clone(), k.clone())).collect()
    }

    /// Every step to every member (docs/pool.md §7.2, §7.7): the targets of offer
    /// `o` that `a`'s worker may do now, as a run contiguous in plan order (one held, done, kept from
    /// it after it failed, or past its room ends the run), from the front of the plan, or just after
    /// its last target of the step when that one's open; each measured (memory, disk) within what it
    /// spares, as many as fit the batch length (`batch_len`) by their measured times on its Mac at the
    /// threads it'd be granted with its start (`batch_secs`: one with none ends the job after its first), their disk summed (or the most a job of the step took, the larger) within
    /// its room. A cold start (`measured` None) by its provisional need, one job of its step in the
    /// pool at a time.
    pub(super) fn pick_every(&self, o: &Offer, a: &Ask, now: Instant) -> Vec<(String, String)> {
        self.pick_run(o, a, now, true)
    }

    /// `pick_every`, a target it would take but for the NAS budget noted waiting (`nas_wait`) when
    /// `note`: not for the job a hold waits for (`room_wait`), weighed in a room the Mac hasn't got.
    fn pick_run(&self, o: &Offer, a: &Ask, now: Instant, note: bool) -> Vec<(String, String)> {
        let held = self.held_of(&o.step, now);
        // (Each target's failures looked up only where the worker failed any; the step's work done
        // taken out once, each target looked up in it: an ask weighs every target of every offer.)
        let (any_failed, done) = (self.failed_any(&a.worker), self.done_of(&o.step));
        let open = |t: &str, k: &str| {
            !held.contains(t) && done.get(t) != Some(&k) && !(any_failed && self.backoff(&a.worker, &o.step, t, now))
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
        // (The lead's own asks held within the window, or alone; a member's past it beside the
        // lead's own alone: jobrt::budget::nas::Limits. Weighed as the job it would make: `t` added
        // to the targets picked before it, `with`, task #939.)
        // (Held back: Some(the room it needs of the window beside what's in use, as weighed here),
        // `nas_wants`'s.)
        let nas_need = |t: &str, with: &JobLoad| -> Option<u32> {
            let (nw, running, members, _, most) = nas.as_ref()?;
            let mut j = *with;
            nw.add(&mut j, &o.step, t, Some(&a.worker));
            let w = nw.job(&j);
            // (The window cut under the whole by the NAS's latency or load: what fits it alone,
            // jobrt::budget::nas::Pace.)
            if !nw.limits.paced(*running, w) {
                return Some(w);
            }
            // (Room held for a target aged in the budget left to it: `nas_reserved`.)
            let reserved = self.nas_reserved(nw, now);
            if !Self::nas_reserve_ok(reserved.as_ref(), &o.step, &[t.to_string()], *running, w, nw.limits.window) {
                return Some(w.saturating_add(reserved.map_or(0, |r| r.2)));
            }
            // (One aged `OWN_AGE` goes beside jobs none of which weighs the whole, the lead's only
            // once aged (`fits_own_target`): the room held for it is always let go.)
            let waited = {
                let g = self.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
                g.get(&(o.step.clone(), t.to_string()))
                    .is_some_and(|e| now.duration_since(e.0) >= jobrt::budget::nas::OWN_AGE)
            };
            let fits = if lead {
                let own = running.saturating_sub(*members);
                jobrt::budget::nas::Limits::fits_own_target(*running, own, most[0], w, waited, nw.limits.win())
            } else {
                jobrt::budget::nas::Limits::fits_job(*members, *running, most[0], w, waited, nw.limits.win())
            };
            (!fits).then_some(w)
        };
        let nas_fits = |t: &str, with: &JobLoad| nas_need(t, with).is_none();
        let takeable = |i: usize| {
            let (t, k, _) = &o.targets[i];
            // (Its wait noted only when this asker would take it but for the budget: a target no
            // asker can take never ages into holding room, `nas_reserved`.)
            open(t, k)
                && match self.measured_in(&o.step, t, a.mem_mb) {
                    Some(m) => fits(&m),
                    // (A cold start: by its provisional need (`provisional`), while no job of its
                    // step is under way anywhere (the lead's own held by a lease too): one learns
                    // its measure before the step fans out.)
                    None => held.is_empty() && self.cold_fits(&o.step, std::slice::from_ref(t), a, now),
                }
                && match nas_need(t, &JobLoad::default()) {
                    None => true,
                    Some(need) => {
                        if note {
                            self.nas_wait(&o.step, t, need, now);
                        }
                        false
                    }
                }
        };
        let after = self
            .last
            .get(&(a.worker.clone(), o.step.clone()))
            .and_then(|l| o.targets.iter().position(|t| t.0 == *l))
            .map(|i| i + 1);
        let start = match &self.placement {
            // (Placement on: from the target measured to take longest that finishes soonest here.)
            Some(_) => self.placed_start(o, a, now, &held, after, &takeable),
            None => after
                .filter(|&i| i < o.targets.len() && takeable(i))
                .or_else(|| (0..o.targets.len()).find(|&i| takeable(i))),
        };
        let Some(start) = start else { return Vec::new() };
        let first = &o.targets[start];
        let Some(m0) = self.measured_in(&o.step, &first.0, a.mem_mb) else {
            return vec![(first.0.clone(), first.1.clone())];
        };
        let mut pick = vec![(first.0.clone(), first.1.clone())];
        // (Its time as `batch_secs` sums it, target by target, at the threads it'd be granted.)
        let threads = self.sizing_threads(&o.step, &first.0, ask_cores(a));
        let (mut secs, mut disk) =
            (self.batch_secs(&o.step, [first.0.as_str()], &a.worker, threads).map(|s| s.0), m0.disk_mb);
        let mut load = JobLoad::default();
        let add = |load: &mut JobLoad, t: &str| {
            if let Some((nw, ..)) = &nas {
                nw.add(load, &o.step, t, Some(&a.worker));
            }
        };
        add(&mut load, &first.0);
        // (The job's memory as it's granted, each target added: its targets' together, on the
        // threads the most of them starts on (`job_need`), not each on its own: one on few threads
        // beside one starting on many runs on the many.)
        let mut names: Vec<String> = vec![first.0.clone()];
        for (t, k, _) in &o.targets[start + 1..] {
            let (Some(s), Some(m)) = (secs, self.measured_in(&o.step, t, a.mem_mb)) else { break };
            let Some((more, _)) = self.secs_on(&o.step, t, &a.worker, threads) else { break };
            names.push(t.clone());
            if pick.len() >= JOB_MAX
                || !open(t, k)
                || !fits(&m)
                || self.job_need_in(&o.step, &names, a.mem_mb).is_none_or(|b| b.mem_mb > a.mem_mb)
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
        let backoff = self.failed_any(&a.worker) && self.backoff(&a.worker, &o.step, t, now);
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
    /// measured) with those cores fill its Mac's (task #440): placement passes
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
                // (As its Mac's packing counts it; with no CPU time measured, now or by its runs, its
                // lease's threads.)
                Work::Job { step, targets } => {
                    let n = Need { idle: idle(l), ..self.held_need(l, step, targets, now) };
                    let counted = self.need_cores(step, &n, total).1;
                    match (n.busy_now, n.cores.and_then(|c| c.busy_tenths)) {
                        _ if counted == 0 || l.threads == 0 => counted,
                        (None, None) => l.threads * 10,
                        _ => counted,
                    }
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
    /// measured to take longest (`secs_of`, a stopped run's time left among them; plan order between
    /// equals, those never measured last).
    /// The first that finishes soonest on its Mac (`finish_s`: its needs over that Mac's supply now)
    /// among the members asking lately that would take it now (`would_take_now`), each other member
    /// counted for as many targets before it as it has slots free (its ask's, less the jobs granted
    /// it since, less the targets this `pack` passed to it already: `Placed::passed`): a target
    /// finishing sooner on another passes to it, and once that one's slots are spoken for the rest
    /// are this one's. A cold start (none of its step measured) is taken, by its provisional need
    /// (`cold_fits`). Why it went here is kept for its grant (`placed`).
    ///
    /// What it doesn't foresee: another member is judged by its last ask, a target at a time
    /// (`would_take_now`: a slot and the memory free then, not the NAS budget, nor what the targets
    /// passed to it take together), and what this pack passed to it is forgotten at the next pack,
    /// while its own grants show only once it asks again. So a target may pass to a Mac that, at its
    /// next ask, has no room for it: it waits that one ask's cycle (a member asks every few seconds
    /// while it has slots free) before it is offered again, to whoever asks.
    pub(super) fn placed_start(
        &self,
        o: &Offer,
        a: &Ask,
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
        // (Each other member asking lately on this app, by its last ask, with its slots free.)
        let mut slots: BTreeMap<String, usize> = self
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
                let (x, at) = (w.ask.as_ref()?, w.asked?);
                let since = self.worker_jobs(n, now).filter(|l| l.granted >= at).count();
                Some((n.clone(), x.want.max(1).saturating_sub(since)))
            })
            .collect();
        // (Within one `pack` alone, the targets it passed to each already: a pick outside one
        // starts afresh.)
        let packing = self.packing.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if packing {
            let placed = self.placed.lock().unwrap_or_else(|e| e.into_inner());
            for (n, k) in slots.iter_mut() {
                *k = k.saturating_sub(placed.passed.get(n).copied().unwrap_or(0));
            }
        }
        slots.retain(|_, k| *k > 0);
        near.iter().copied().chain(rest).find(|&i| {
            if !takeable(i) {
                return false;
            }
            let (t, k, _) = &o.targets[i];
            if self.measured(&o.step, t).is_none() {
                return true;
            }
            // (Each other member by the rules its own ask would meet, and only one that would take
            // it now: a target passed to it is one it takes. Its memory as that ask would read it,
            // on the threads its room fits it on, `measured_in`.)
            let mut who: Vec<(String, f64, Option<f64>)> = slots
                .keys()
                .filter_map(|n| {
                    let w = self.workers.get(n)?;
                    let x = w.ask.as_ref()?;
                    let m = self.measured_in(&o.step, t, x.mem_mb)?;
                    let takes = x.can.contains(&o.step)
                        && self.may_take_measured(o, x, t, k, &m, held, now)
                        && !self.swamped(n, x, now)
                        && self.would_take_now(n, w.asked?, x, &m, now);
                    if !takes {
                        return None;
                    }
                    let (needs, cache) = self.finish_s(&o.step, t, n, x)?;
                    Some((n.clone(), needs / cache.unwrap_or(1.0), cache))
                })
                .collect();
            let Some((needs, cache)) = self.finish_s(&o.step, t, &a.worker, a) else { return true };
            let mine = needs / cache.unwrap_or(1.0);
            who.sort_by(|x, y| x.1.total_cmp(&y.1).then(x.0.cmp(&y.0)));
            if let Some((n, _, _)) = who.first().filter(|w| w.1 < mine) {
                if let Some(k) = slots.get_mut(n) {
                    *k -= 1;
                    if *k == 0 {
                        slots.remove(n);
                    }
                }
                if packing {
                    *self.placed.lock().unwrap_or_else(|e| e.into_inner()).passed.entry(n.clone()).or_default() += 1;
                }
                return false;
            }
            // (Its needs' time on each Mac, and where its Mac holds the step's cache, how it was
            // counted: the time over CACHE_PREFER is a weight, not a time.)
            let said = |counted: f64, cache: Option<f64>| match cache {
                Some(c) => format!("{:.1} min (holds its cache: counted ÷{c})", counted * c / 60.0),
                None => format!("{:.1} min", counted / 60.0),
            };
            let why = if who.is_empty() {
                format!("the only Mac asking that would take it now ({} here)", said(mine, cache))
            } else {
                let others: Vec<String> = who.iter().map(|(n, s, c)| format!("{n} {}", said(*s, *c))).collect();
                format!(
                    "soonest done here of the Macs asking that would take it now: {} here, {}",
                    said(mine, cache),
                    others.join(", ")
                )
            };
            self.placed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .why
                .insert((a.worker.clone(), o.step.clone(), t.clone()), why);
            true
        })
    }

    /// How long target `target` of `step` takes worker `n`'s Mac as its ask `x` says (s), and
    /// `CACHE_PREFER` where its Mac holds the step's shared cache whole (`holds_cache`: no copy, no
    /// NAS read in place): placement's measure of where it finishes soonest is the time over that.
    /// The time is its needs over that Mac's supply now (crate::agent::forecast::needs_secs: its
    /// speed, the threads it would be granted alone of the cores its ask says, the NAS budget).
    /// None: none of its step measured.
    pub(super) fn finish_s(&self, step: &str, target: &str, n: &str, x: &Ask) -> Option<(f64, Option<f64>)> {
        let threads = self.sizing_threads(step, target, ask_cores(x));
        let secs = crate::agent::forecast::needs_secs(self, step, target, n, threads)?;
        Some((secs, self.holds_cache(step, x).then_some(crate::agent::placement::CACHE_PREFER)))
    }

    /// The memory (MB) a job under lease `l` takes: what the packing counts it at (`lease_need`:
    /// its targets together on the threads it started on; a cold start's, its provisional need).
    pub(super) fn lease_mem(&self, l: &Lease) -> u64 {
        let Work::Job { step, targets } = &l.work else { return 0 };
        self.lease_need(l, step, targets).0.m.mem_mb
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
    /// Each step's Macs' room at their best (`Sched::step_rooms`; a record before them has none).
    pub step_rooms: BTreeMap<String, u64>,
    pub costs: BTreeMap<String, Cost>,
    pub floors: BTreeMap<String, Floor>,
    pub disks: BTreeMap<String, u64>,
    pub job_disks: BTreeMap<String, Vec<u64>>,
    pub fills: BTreeMap<String, u64>,
    pub app: String,
    pub lead_name: String,
    pub placement: Option<(crate::agent::placement::Placement, u64)>,
    pub nas: Option<jobrt::budget::nas::Limits>,
    pub nas_own: Option<jobrt::budget::nas::Limits>,
    /// (step, target, age first held back, age last).
    pub nas_waits: Vec<(String, String, u64, u64)>,
    /// (step, age).
    pub quiet_since: Vec<(String, u64)>,
    /// (worker, step, its last target).
    pub last: Vec<(String, String, String)>,
    /// Each Mac's time and jobs' start (`Sched::paces`; a record before them has none), the jobs'
    /// records and the needs model's tuning (`Sched::records`, `Sched::tuning`; task #1416: a
    /// record before them has none, and the defaults).
    pub paces: crate::paces::Paces,
    pub records: crate::needs::Records,
    pub tuning: jobrt::needs::Tuning,
    /// The failures' backoff (`Sched::fail_hold`, task #1447: a record before it has none, and the
    /// defaults).
    pub fail_hold: jobrt::backoff::Backoff,
    /// The batch length set (`Sched::batch_s`, task #927: 0, not set or a record before it).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub batch_s: u64,
    /// What a progress end of each step's jobs was discounted by (`Sched::left_discount`; the
    /// pool's under ""; task #1505 review: a record before it has none, and no progress end).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lefts: BTreeMap<String, f64>,
    /// What is left of each target whose run stopped short (`Sched::stopped`; task #1454: a record
    /// before it has none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stopped: BTreeMap<String, BTreeMap<String, f64>>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl State {
    /// What a Book of its costs reads them by (crate::needs::Inputs): its paces, records and tuning.
    pub fn inputs(&self) -> crate::needs::Inputs<'_> {
        crate::needs::Inputs { paces: &self.paces, records: &self.records, tuning: self.tuning }
    }

    /// The cores `a`'s Mac has past those busy outside the pool as its ask says (`outside_cores`,
    /// past the cores the jobs it holds here count for: each the cores the lead counted it as
    /// keeping busy now (`Leased::busy_tenths`, task #1094), else its targets' most kept busy
    /// (crate::needs::Cores::busy_tenths),
    /// summed in tenths, within its threads, else its threads): the forecast's room for its jobs
    /// (task #684).
    pub fn cores_left(&self, the_plan: &'static dyn crate::agent::seam::Plan, a: &Ask) -> u32 {
        let b = crate::needs::Book::pool(the_plan, &self.costs, Some(&self.disks), self.inputs());
        let ours: u32 = self
            .leases
            .iter()
            .filter(|l| l.worker == a.worker)
            .map(|l| {
                let measured = || {
                    let ts: Vec<String> = l.targets.iter().map(|t| t.0.clone()).collect();
                    book_cores(&b, &l.step, &ts).and_then(|c| c.busy_tenths)
                };
                l.busy_tenths.or_else(measured).map_or(l.threads * 10, |c| c.min(l.threads * 10))
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
    /// The cores its job keeps busy now, in tenths, as the lead counts it (`busy_now`, task #1091),
    /// once its run says it: what the forecast's scheduler counts it for (`Lease::busy_said`) and
    /// `State::cores_left` takes off (task #1094). None (not said yet, or a record before task #1094):
    /// its measure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_tenths: Option<u32>,
    /// Its job's time left by its progress as its last beat said it, less the time since
    /// (`Lease::left_by_progress`, s; task #1505 review, forecast code 10): a record's scheduler
    /// times it by it as the lead's does. None: not said, or a record before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_s: Option<u64>,
}

/// A worker's last ask (`State::workers`): the ask, and the ages of its last request and ask.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub ask: Ask,
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
                    match &l.work {
                        Work::Job { step, targets } => Some(Leased {
                            granted_at,
                            busy_tenths: self.held_need(&l, step, targets, now).busy_now,
                            left_s: l.left_by_progress(now),
                            worker: l.worker.clone(),
                            step: step.clone(),
                            targets: targets.clone(),
                            threads: l.threads,
                        }),
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
                .filter_map(|w| Some(Asked { ask: w.ask.clone()?, seen_s: age(w.seen), asked_s: w.asked.map(age) }))
                .collect(),
            step_rooms: self.step_rooms.clone(),
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
                .map(|((s, t), (a, b, _))| (s.clone(), t.clone(), age(*a), age(*b)))
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
            records: self.records.clone(),
            tuning: self.tuning,
            fail_hold: self.fail_hold,
            batch_s: self.batch_s,
            lefts: self.lefts_taken(),
            stopped: self.stopped.clone(),
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
            leases.from_record(id, at, l.busy_tenths, clock.unix, now);
            leases.set_threads(id, l.threads);
            if let Some(left) = l.left_s {
                leases.said_left(id, left, now);
            }
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
            fail_hold: s.fail_hold,
            workers,
            step_rooms: s.step_rooms.clone(),
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
                    .map(|(st, t, a, b)| ((st.clone(), t.clone()), (ago(now, *a), ago(now, *b), None)))
                    .collect(),
            ),
            nas_reserved_at: Default::default(),
            quiet_since: Mutex::new(s.quiet_since.iter().map(|(st, a)| (st.clone(), ago(now, *a))).collect()),
            quiet_waits: Default::default(),
            last: s.last.iter().map(|(w, st, t)| ((w.clone(), st.clone()), t.clone())).collect(),
            measured_steps: Default::default(),
            measured_of: Default::default(),
            drop_needs: Default::default(),
            kept_leases: Default::default(),
            packing: Default::default(),
            placed: Default::default(),
            provisional_at: Default::default(),
            paces: s.paces.clone(),
            records: s.records.clone(),
            tuning: s.tuning,
            batch_s: s.batch_s,
            lefts: Lefts { by: BTreeMap::new(), taken: s.lefts.clone() },
            stopped: s.stopped.clone(),
            event_jobs: Default::default(),
        };
        sched.set_offers(s.offers.clone());
        sched
    }

    /// The members in `idle` (name, slots free now, the memory its Mac has with no job of the pool
    /// running, MB) ask again at `now`, as the live ones do each loop, wanting a job a free slot
    /// (and those granted them this instant, which count since their ask), their memory less that
    /// of every job running that began before. Only a member with a slot free asks: a busy one
    /// keeps its last ask and ages out of placement's Macs asking (`ASKING`), as a live one's does. (The
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

/// The key of a Mac's brake on `step` in `Sched::failed` (its "target" `*`, no target's: a
/// cost key of unit is its z6 tile alone), the Mac its worker (crate::paces::mac_of).
pub fn brake_key(step: &str) -> String {
    format!("{step} *")
}

/// Each Mac's failed jobs of each step lately and its last good job of each, toward its brake on
/// the step (jobrt::backoff::Backoff::brakes, its `step-brake-jobs` within `step-brake-minutes`),
/// by Mac (crate::paces::mac_of: its second job's its own): the lead's (`Shared::brakes`), counted
/// again from its history at its start (`seed_backoff`). A brake is for a faulty Mac, not a
/// faulty step or data: a failed job counts only once every target it was held on has been built
/// well on another Mac since (`Pending`), within the window (a target failing wherever it runs is
/// its own fault, held by its own backoff alone), and brakes only while one of the Macs that did
/// the step well within the window isn't braked on it itself, so the last Mac a step goes well on
/// is never braked. A failure that isn't its Mac's (`excused`: stopped by the memory guard, z6
/// tiles set aside alone) isn't brought here.
#[derive(Debug, Default)]
pub struct Brakes {
    /// (Mac, step) → its failed jobs' times counted (unix s).
    fails: BTreeMap<(String, String), Vec<u64>>,
    /// (step, Mac) → its last good job's time (unix s).
    good: BTreeMap<(String, String), u64>,
    /// By step, its failed jobs not counted yet.
    pending: BTreeMap<String, Vec<Pending>>,
}

/// A failed job not counted yet toward its Mac's brake (`Brakes`): its Mac, its time (unix s),
/// and the targets it was held on not built well on another Mac since.
#[derive(Debug)]
struct Pending {
    mac: String,
    at: u64,
    left: BTreeSet<String>,
}

impl Brakes {
    /// A job of `step` failed on `worker` at `at` (unix s), held on `held` (crate::coord's
    /// `held_of`): counted toward its Mac's brake once each of those is built well on another Mac
    /// (`job_done`), within `b`'s window.
    pub fn job_failed(&mut self, b: &jobrt::backoff::Backoff, worker: &str, step: &str, held: &[&String], at: u64) {
        let window = (b.brake_minutes * 60.0) as u64;
        let p = self.pending.entry(step.to_string()).or_default();
        p.retain(|p| at.saturating_sub(p.at) < window);
        if !held.is_empty() {
            let left = held.iter().map(|t| t.to_string()).collect();
            p.push(Pending { mac: crate::paces::mac_of(worker).to_string(), at, left });
        }
    }

    /// A job of `step` done well on `worker` at `at` (unix s), its targets `done`: the other Macs'
    /// failed jobs it leaves nothing they were held on unbuilt counted, as `b` sets the brake; the
    /// Macs that braked so, `braked` saying whether a Mac is braked on the step now.
    pub fn job_done(
        &mut self,
        b: &jobrt::backoff::Backoff,
        worker: &str,
        step: &str,
        done: &[String],
        at: u64,
        braked: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let mac = crate::paces::mac_of(worker);
        let window = (b.brake_minutes * 60.0) as u64;
        let e = self.good.entry((step.to_string(), mac.to_string())).or_insert(at);
        *e = (*e).max(at);
        let mut counted: Vec<(String, u64)> = Vec::new();
        if let Some(ps) = self.pending.get_mut(step) {
            ps.retain_mut(|p| {
                if at.saturating_sub(p.at) >= window {
                    return false;
                }
                if p.mac != mac {
                    for t in done {
                        p.left.remove(t);
                    }
                }
                if p.left.is_empty() {
                    counted.push((p.mac.clone(), p.at));
                }
                !p.left.is_empty()
            });
        }
        let mut out: Vec<String> = Vec::new();
        for (m, t) in counted {
            let others = self
                .good
                .range((step.to_string(), String::new())..)
                .take_while(|((s, _), _)| s == step)
                .any(|((_, o), g)| *o != m && at.saturating_sub(*g) < window && !braked(o));
            if b.brakes(self.fails.entry((m.clone(), step.to_string())).or_default(), t) && others && !out.contains(&m)
            {
                out.push(m);
            }
        }
        out
    }

    /// A page's retry of `step`: its failed jobs counted afresh on every Mac, those not counted
    /// yet forgotten.
    pub fn forget(&mut self, step: &str) {
        self.fails.retain(|(_, s), _| s != step);
        self.pending.remove(step);
    }
}

/// The Mac and step of a job `e` ended well (an "end" or "done" ok, its targets done), for
/// `Brakes::job_done`.
pub fn good_job(e: &super::history::Event) -> Option<(&str, &str)> {
    let ok = matches!(e.kind.as_str(), "end" | "done") && e.ok == Some(true);
    Some((e.worker.as_deref().filter(|_| ok)?, e.step.as_deref().filter(|s| !s.is_empty())?))
}

/// A brake on `step`, in words, as `b` sets it (the status's, the history's).
pub fn brake_words(step: &str, b: &jobrt::backoff::Backoff) -> String {
    format!(
        "{step} kept from this Mac after {} of its jobs failed here within {} min (step-brake-jobs, step-brake-minutes)",
        b.brake_jobs, b.brake_minutes
    )
}
