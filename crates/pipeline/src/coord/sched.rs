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
use super::{cost_key, cost_version, disk_words, first_guess, job_secs, Ask, Cost, Measured, Offer, Worker};
use super::{AROUND, ASKING};
use crate::agent::memguard::Floor;
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
/// (target, key), the threads it runs, the disk it reserves (its measure and its shared cache's
/// fill), and the lead's planned job it is (`Offer::job`).
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub lease: u64,
    pub step: String,
    pub targets: Vec<(String, String)>,
    pub threads: u32,
    pub disk_mb: u64,
    pub job: Option<String>,
}

/// What every grant is decided from: the leases out, the plan's offers, the work done and failed,
/// the workers' last asks, the measures (costs, floors, disk floors, jobs' disks, caches' fills)
/// and the NAS budget's limits and waits.
#[derive(Debug, Default)]
pub struct Sched {
    pub leases: Leases,
    /// The plan's work a worker that mounts the NAS may do, a step at a time in plan order, and the
    /// lead's own planned jobs (`Offer::job`). Set through `set_offers`, which keeps `offered`.
    pub offers: Vec<Offer>,
    /// Each step's offered targets, the size each was first offered with (`offers` indexed: the
    /// placement looks up every target it weighs, and a worldwide pass offers tens of thousands).
    pub offered: std::collections::HashMap<String, std::collections::HashMap<String, u64>>,
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
    /// targets and the tasks drawn among those asking by measured speed, a task offered ahead given
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
    /// Each step's drops' need (crate::needs::Book::drop_need) as the measures stood (`rev`).
    pub drop_needs: Mutex<(u64, BTreeMap<String, Option<(u64, u64)>>)>,
}

/// `Sched::measured_of`'s targets' measures: step → target → `Sched::measures`.
pub type Measures = std::collections::HashMap<String, std::collections::HashMap<String, (u64, Option<Measured>)>>;

/// The NAS budget's weights for one placement (`Shared::nas_room`): the limits and one Book (its
/// steps' 90th percentiles kept with its models: crate::needs::Book::nas_prior).
pub(crate) struct NasWeigher<'a> {
    limits: crate::budget::nas::Limits,
    book: crate::needs::Book<'a>,
}

impl NasWeigher<'_> {
    /// The weight of `target` of `step` (none of its step measured: the whole for a NAS step, else 0).
    fn weight(&self, step: &str, target: &str) -> u32 {
        let r = self.book.nas(step, target);
        // (A step known to read the NAS (`NAS_STEPS`) none of whose runs noted its rate, in any
        // way it ran: the whole budget, so its first job never overloads the NAS unseen, task #499
        // review; its step's runs of earlier ways count by `nas_prior`.)
        r.map_or(if NAS_STEPS.contains(&step) { crate::budget::nas::WHOLE } else { 0 }, |r| self.limits.weight(r))
    }
}

/// A wait that holds the NAS budget's room (`Sched::nas_reserved`): since when, its step and target.
pub type Reserved = (Instant, (String, String));

/// What a job is packed as (`Shared::pack_need`): its memory and disk, its cores as measured
/// (crate::needs::Cores; None: not said), and whether it's a cold start (none of it measured, `m` its provisional need).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Need {
    m: Measured,
    cores: Option<crate::needs::Cores>,
    cold: bool,
    /// Only waiting on a worker (its Mac's `Capacity::waiting`): counted at the memory it holds
    /// now, within its measure, and none of its cores (docs/pool.md §7.2, A job waiting on a worker).
    waits: bool,
    /// Leaving its Mac's cores idle (its Mac's `Capacity::idle`): none of its cores (docs/pool.md
    /// §7.2, A job leaving the cores idle).
    idle: bool,
}

/// The steps known to honour the threads they're granted (task #499 review): their cold starts
/// take a thread (task #896: their run measures what they use, and climbs from there), the others'
/// the whole Mac. `slope-hi` and `slope-deep`: Rust steps
/// on the rayon pool the agent sizes (`RAYON_NUM_THREADS`, crate::agent::jobs); `terrain-deep`: its
/// base and deep tiles made on that pool, its reads ahead waiting on the network
/// (crate::terrain_deep); `bld-fetch`: its downloads bounded by its `--jobs` (3 by default, at most
/// 4), waiting on the network; `pyramids-fill`: its fetches a host's requests in flight (crate::fetch::HOST_REQUESTS),
/// waiting on the network, its own work (whole-PNG checks, packing) a fraction of a core (task #823:
/// counted as the whole Mac, its first job held 6 of the mini's 12 cores).
pub(super) const THREADS_HONOURED: &[&str] = &["slope-hi", "slope-deep", "terrain-deep", "bld-fetch", "pyramids-fill"];

/// A cold start's provisional memory (MB) from the most its step declares or it was offered with,
/// `mb`: times 3/2 (task #499; `Shared::provisional`, the forecast's packing too).
pub(super) fn provisional_mb(mb: u64) -> u64 {
    mb.saturating_mul(3) / 2
}

/// The cores `a`'s Mac has for its jobs together: its capacity's (else the ask's), less the in-use
/// share's (`Capacity::kept_cores`) while its owner uses it; one at least.
pub fn ask_cores(a: &Ask) -> u32 {
    let cap = a.cap.as_ref();
    let total = cap.map(|c| c.cores).filter(|&c| c > 0).unwrap_or(a.cores).max(1);
    total.saturating_sub(cap.filter(|c| c.in_use).map_or(0, |c| c.kept_cores)).max(1)
}

/// The cores `a`'s Mac has busy outside the pool: those its last minute's load average keeps busy
/// (its capacity's `cores` less `cores_free`) past `ours`, the threads of the pool's jobs it holds,
/// less the cores the in-use share keeps from its jobs already while its owner uses it
/// (`ask_cores` takes those off). 0 for an ask without a capacity.
/// The load average is rounded up to whole cores: an excess of one is taken for that and counts
/// none (task #896: no share of the Mac stands for a job just ended any more; one still in the
/// load counts as outside it until the load shows it gone, a minute at most, and it keeps half the
/// cores at most meanwhile).
/// At most half the cores its Mac has for its jobs (`ask_cores`) count (task #684): the load
/// average can't tell a niced test build (tools/app/remote-tests.sh runs at nice 10) from work
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

/// The whole cores `t` tenths of a core make, to the nearest (a Mac's jobs' busy cores summed
/// before rounding, task #697: beside its load average's whole cores).
fn tenths_to_cores(t: u32) -> u32 {
    (t + 5) / 10
}

/// `t` tenths of a core in words: "1.2".
fn tenths(t: u32) -> String {
    format!("{}.{}", t / 10, t % 10)
}

/// How long a Mac's load is given to show its last grant before it's granted a job past the cores
/// its jobs' threads leave (`Sched::pack`, task #688 review): the minute its load average spans.
pub const SETTLE: Duration = Duration::from_secs(60);

/// The steps that read the NAS as they go: one none of whose runs measured its rate weighs the NAS
/// budget's whole (`NasWeigher`), and one running may load it unseen (`nas_unknown_running`).
pub(super) const NAS_STEPS: &[&str] = &["unit", "elev", "pack", "lo"];

/// How long a job offered runs, at most, by its targets' measured
/// times (s): a quarter of an hour (docs/pool.md §7.7, batch). Its first target is taken whatever
/// its time.
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
        self.floors.get(&cost_key(step, target)).filter(|f| f.v >= cost_version(step)).copied()
    }

    /// The memory a job of `step` for `target` is expected to take (MB), as `job_peak` sizes it from
    /// its costs and `size`, and never below its floor (one of an older way of its step too: the best
    /// estimate there is until it runs again).
    pub(super) fn peak_of(&self, step: &str, target: &str, size: u64, threads: u32) -> u64 {
        self.book().mem(step, target, threads, first_guess(&self.costs, step, target, size)).0
    }

    /// The measures crate::needs reads (the costs, the floors and the disk floors), its models kept
    /// in `needs` until they change (`needs_rev`; their counts too, for a change made in place).
    pub(super) fn book(&self) -> crate::needs::Book<'_> {
        crate::needs::Book::kept(&self.costs, &self.floors, &self.disks, &self.needs, self.rev())
    }

    /// Where the measures stand: it moves with each cost, floor or disk floor taken or cleared
    /// (`needs_rev`, and their counts for a change made in place).
    fn rev(&self) -> u64 {
        self.needs_rev.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ ((self.costs.len() as u64) << 40 | (self.floors.len() as u64) << 20 | self.disks.len() as u64)
            ^ (self.job_disks.len() as u64).rotate_left(60)
    }

    /// The first guess of offered `target` of `step` (`first_guess` by the size it was offered
    /// with), 0 when it isn't offered.
    pub(super) fn offered_guess(&self, step: &str, target: &str) -> u64 {
        self.offered_size(step, target).map_or(0, |size| first_guess(&self.costs, step, target, size))
    }

    /// The size `target` of `step` was first offered with; None when it isn't offered.
    pub(super) fn offered_size(&self, step: &str, target: &str) -> Option<u64> {
        self.offered.get(step)?.get(target).copied()
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
        for o in &self.offers {
            let of = self.offered.entry(o.step.clone()).or_default();
            for t in &o.targets {
                of.entry(t.0.clone()).or_insert(t.2);
                if crate::agent::build::is_none_key(&o.step, &t.0, &t.1) {
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

    /// `target` of `step`'s memory by the book (a step never measured: its first guess, `lease_mem`)
    /// and `measured`; kept while the measures stand (`measured_of`).
    fn measures(&self, step: &str, target: &str) -> (u64, Option<Measured>) {
        let rev = self.rev();
        let mut g = self.measured_of.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != rev {
            *g = (rev, Measures::new());
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

    /// How long `target` of `step` is measured to take `worker` (s): its own run's (`job_secs`), else
    /// its step's model's (crate::needs: its fit, else the mean of its step's runs); None when
    /// none of its step's has run the way it runs now.
    pub(super) fn secs_of(&self, step: &str, target: &str, worker: &str) -> Option<u64> {
        job_secs(&self.costs, step, target, worker).or_else(|| {
            let b = self.book();
            self.drop_of(&b, step, target).map(|d| d.0).or_else(|| b.secs(step, target, 0))
        })
    }

    /// What target `target` of `step` takes as a drop (`drops`) its own run hasn't measured: its
    /// step's drops' time and memory (crate::needs::Book::drop_need); None: no drop, or measured
    /// itself, or none of its step's drops ran yet (its step's model stands).
    fn drop_of(&self, b: &crate::needs::Book, step: &str, target: &str) -> Option<(u64, u64)> {
        if !self.drops.contains(&(step.to_string(), target.to_string())) {
            return None;
        }
        let own = self.costs.get(&cost_key(step, target)).is_some_and(|c| c.v >= crate::coord::cost_version(step));
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
        self.leases.live(now).filter(|l| l.worker == worker && matches!(l.work, Work::Job { .. })).cloned().collect()
    }

    /// A job of `step` over `targets`: its NAS weight under `l` (its heaviest target's,
    /// crate::budget::nas; 0 for a target none of whose step's runs measured it; with none, its
    /// step's), and whether it
    /// downloads (its step's kind a fetch, crate::agent::steps::Kind::Fetch).
    /// (`nw` keeps the Book and the steps' percentiles for one placement: `NasWeigher`.)
    pub(super) fn job_nas<'t>(nw: &NasWeigher, step: &str, targets: impl IntoIterator<Item = &'t str>) -> (u32, bool) {
        // (A job of no targets of its own, the clean-up's lease: its step's measured weight, task #451.)
        let w = targets.into_iter().map(|t| nw.weight(step, t)).max().unwrap_or_else(|| nw.weight(step, ""));
        (w, crate::agent::steps::row(step).is_some_and(|r| r.kind == crate::agent::steps::Kind::Fetch))
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
        // (The switch `off`: the members' jobs aren't held, so the lead's own aren't held back by
        // them either, only by its own (task #440: else a member's unheld jobs keep the lead idle).)
        let members_held = self.nas.is_some();
        let (mut w, mut members, mut dl, mut most) = (0u32, 0u32, 0usize, [0u32; 2]);
        for lease in self.leases.live(now) {
            if !members_held && lease.worker != self.lead_name {
                continue;
            }
            if let Work::Job { step, targets } = &lease.work {
                let (jw, d) = Self::job_nas(&nw, step, targets.iter().map(|t| t.0.as_str()));
                w = w.saturating_add(jw);
                most[0] = most[0].max(jw);
                if lease.worker != self.lead_name {
                    members = members.saturating_add(jw);
                    most[1] = most[1].max(jw);
                }
                dl += usize::from(d);
            }
        }
        Some((nw, w, members, dl, most))
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
            let w = nw.weight(&st, &t);
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

    /// Whether a leased job may load the NAS unseen by its weight: one of a step known to read it
    /// (`NAS_STEPS`), or one none of whose step's runs measured its NAS rate (a cold one).
    pub(super) fn nas_unknown_running(&self, nw: &NasWeigher, now: Instant) -> bool {
        self.leases.all(now).into_iter().any(|l| match &l.work {
            Work::Job { step, targets } => {
                NAS_STEPS.contains(&step.as_str())
                    || if targets.is_empty() {
                        nw.book.nas(step, "").is_none()
                    } else {
                        targets.iter().any(|t| nw.book.nas(step, &t.0).is_none())
                    }
            }
            _ => false,
        })
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
    /// the most of their memory, their disks summed or the most a job of the step took), and its
    /// cores (crate::needs::Book::cores: the most of its targets', each of them; None when one's
    /// aren't said); None when any is a cold start.
    pub(super) fn job_need(&self, step: &str, targets: &[String]) -> Option<(Measured, Option<crate::needs::Cores>)> {
        let each: Vec<Measured> = targets.iter().map(|t| self.measured(step, t)).collect::<Option<_>>()?;
        let job = self.job_floor(step);
        let b = self.book();
        let cores = targets.iter().map(|t| b.cores(step, t)).collect::<Option<Vec<crate::needs::Cores>>>().map(|v| {
            crate::needs::Cores {
                grant: v.iter().map(|c| c.grant).max().unwrap_or(0),
                busy_tenths: v
                    .iter()
                    .map(|c| c.busy_tenths)
                    .collect::<Option<Vec<u32>>>()
                    .and_then(|b| b.into_iter().max()),
            }
        });
        Some((
            Measured {
                mem_mb: each.iter().map(|m| m.mem_mb).max().unwrap_or(0),
                disk_mb: crate::needs::batch_disk(step, each.iter().map(|m| m.disk_mb)).max(job),
                own: each.iter().all(|m| m.own),
            },
            cores,
        ))
    }

    /// What a job of `step` over `targets` is packed as (task #499): its measure (`job_need`), else,
    /// a cold start (`Need::cold`), its provisional need (`provisional`).
    pub(super) fn pack_need(&self, step: &str, targets: &[String]) -> Need {
        // (A step whose jobs only change build records (task #838): never cold, one thread counted for no
        // cores (it waits on the NAS), its declared memory, no disk: it goes beside any jobs, a long one's included.)
        if crate::agent::steps::records_only(step) {
            let m = Measured { mem_mb: crate::agent::steps::mem_mb(step), disk_mb: 0, own: false };
            let cores = Some(crate::needs::Cores { grant: 1, busy_tenths: Some(0) });
            return Need { m, cores, cold: false, waits: false, idle: false };
        }
        match self.job_need(step, targets) {
            Some((m, cores)) => Need { m, cores, cold: false, waits: false, idle: false },
            None => Need { m: self.provisional(step, targets), cores: None, cold: true, waits: false, idle: false },
        }
    }

    /// The threads a job packed as `n` is granted on a Mac of `total` cores, and the cores it counts
    /// for there, in tenths of a core (task #688; tenths, task #697: a job's busy cores rounded to the
    /// nearest undercounted up to half a core each). Measured: its grant (crate::needs::Book::cores:
    /// the cores it computed on and a thread more) within the Mac's cores, counted for the cores it
    /// kept busy (`busy_tenths`, a climb's thread among them; its threads where its runs said no CPU
    /// time). No fixed share caps or floors a measured job (task #888: the M4's terrain jobs were
    /// held to 3 of 14 cores, 2.4 of them busy, while the Mac sat at a load of 4; task #896: its
    /// runs, not a share, say what it uses). Not said (no run said its threads), or a cold start of
    /// a step honouring its threads (`THREADS_HONOURED`), a thread both (task #896: its run measures
    /// it, and climbs from there); another cold start the whole Mac both (task #499 review: one
    /// that runs as many threads as the Mac has would swamp the jobs beside it). One only waiting on
    /// a worker, or leaving its cores idle, counts for none.
    pub(super) fn need_cores(&self, step: &str, n: &Need, total: u32) -> (u32, u32) {
        let (threads, busy) = if crate::agent::steps::records_only(step) {
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

    /// A cold start's provisional need (task #499): `provisional_mb` of the most its step declares
    /// (crate::agent::steps' `mem_mb`) or it was offered with (`offered_guess`), the disk its step's
    /// measures say (none: 0). It goes beside other jobs when that fits; the memory guard
    /// (crate::agent::memguard) stops one that overruns, and its floor is learned.
    pub(super) fn provisional(&self, step: &str, targets: &[String]) -> Measured {
        let b = self.book();
        let declared = crate::agent::steps::mem_mb(step);
        let each = targets.iter().map(|t| self.offered_guess(step, t).max(declared)).max().unwrap_or(declared);
        Measured {
            mem_mb: provisional_mb(each),
            disk_mb: targets.iter().filter_map(|t| b.disk(step, t, 0)).max().unwrap_or(0),
            own: false,
        }
    }

    /// Whether a cold start of `step` over `targets` may go to `a`'s worker (task #499): its
    /// provisional need fits the room `a` spares, or the worker holds no job (one bigger than its
    /// whole room goes alone, as every cold start did before: the guard learns its floor).
    pub(super) fn cold_fits(&self, step: &str, targets: &[String], a: &Ask, now: Instant) -> bool {
        let m = self.provisional(step, targets);
        (m.mem_mb <= a.mem_mb && a.disk_mb.is_none_or(|d| m.disk_mb <= d)) || self.job_leases(&a.worker, now).is_empty()
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
    /// (`pack_need`), task #499. Each granted holds its lease from then (`grant`). And why it
    /// granted fewer jobs than the ask wanted (None: as many as it wanted).
    pub fn pack(&mut self, a: &Ask, offers: &[Offer], lead: bool, clock: Clock) -> (Vec<Pick>, Option<String>) {
        let now = clock.now;
        let mine = self.job_leases(&a.worker, now);
        // (A job held: its measure, else its provisional need, `pack_need`; one its Mac says only
        // waits on a worker (`Capacity::waiting`), at what it holds now within that, none of its
        // cores: the room past it free for a job beside it, which yields to it when it goes on
        // (crate::agent's `yielding`); one leaving its Mac's cores idle (`Capacity::idle`, a job
        // waiting on the network), at its measure but none of its cores.)
        let waits =
            |l: &Lease| a.cap.as_ref().and_then(|c| c.waiting.iter().find(|w| w.lease == l.id)).map(|w| w.held_mb);
        let jobs: Vec<(&Lease, String, Vec<String>)> = mine
            .iter()
            .filter_map(|l| match &l.work {
                Work::Job { step, targets } => Some((l, step.clone(), targets.iter().map(|t| t.0.clone()).collect())),
                Work::Task { .. } => None,
            })
            .collect();
        let needs: Vec<Need> = jobs
            .iter()
            .map(|(l, step, targets)| {
                let mut n = self.pack_need(step, targets);
                if let Some(held) = waits(l) {
                    n.m.mem_mb = n.m.mem_mb.min(held);
                    n.waits = true;
                }
                n.idle = a.cap.as_ref().is_some_and(|c| c.idle.contains(&l.id));
                n
            })
            .collect();
        let held_steps: Vec<(String, Vec<String>)> = jobs.iter().map(|(_, st, ts)| (st.clone(), ts.clone())).collect();
        // (A cold start of a step never measured in any way: nothing beside it, as before task
        // #499.)
        if let Some((st, _)) =
            needs.iter().zip(&held_steps).find(|(n, (st, _))| n.cold && self.never_measured(st)).map(|(_, s)| s.clone())
        {
            return (Vec::new(), Some(format!("{st} was never measured: nothing goes beside its first job")));
        }
        // (At most one cold start on a Mac (task #499 review): two unknown needs never meet.)
        let mut cold_here = needs.iter().any(|n| n.cold);
        let total = ask_cores(a);
        // (Cores left, in tenths of a core: each job's, the cores it counts for (`need_cores`).)
        let mut cores = needs
            .iter()
            .zip(&held_steps)
            .fold(total * 10, |left, (n, (st, _))| left.saturating_sub(self.need_cores(st, n, total).1));
        // (Its Mac's load from outside the pool (other processes: builds, tests): the cores its
        // last minute's load average keeps busy (`Capacity::cores_free`) past those the jobs it
        // holds count for, less those the in-use share keeps already, come off the cores left
        // (task #440), at most half its cores (task #684): a Mac swamped by other work is granted
        // jobs in the other half only. A job keeping more busy than it was measured to shows here.)
        let ours = tenths_to_cores((total * 10).saturating_sub(cores));
        let outside = outside_cores(a, ours);
        cores = cores.saturating_sub(outside * 10);
        // (The cores left were its jobs counted at their threads, as before task #688: a grant
        // past them is an overcommit, trusting its busy cores (task #688 review). At most one per
        // ask, and none while its Mac's load hasn't had `SETTLE` to show its last grant or shows
        // load past its jobs' busy cores by more than a core (the load average's whole cores): the jobs go up one at a time, each measured by the load
        // before the next, so jobs busier than their runs said (or #690's reads in parallel) stop
        // the climb within a minute instead of a whole ask's worth landing at once.)
        let mut by_threads = needs
            .iter()
            .zip(&held_steps)
            .fold(total, |left, (n, (st, _))| {
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
        for (st, _) in &held_steps {
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
                    crate::agent::every::grants(&o.step, true, lead)
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
            for o in offers {
                if !crate::agent::every::grants(&o.step, o.job.is_some(), lead) {
                    continue;
                }
                // (A job reading a shared cache none here reads yet: its room less the cache's.)
                let fill = self.cache_fill(&o.step, &caches, a);
                let room_o = match room.disk_mb {
                    Some(d) if fill > 0 => Ask { disk_mb: Some(d.saturating_sub(fill)), ..room.clone() },
                    _ => room.clone(),
                };
                let pick =
                    if o.job.is_some() { self.pick_own(o, &room_o, now) } else { self.pick_every(o, &room_o, now) };
                if pick.is_empty() {
                    continue;
                }
                let ts: Vec<String> = pick.iter().map(|t| t.0.clone()).collect();
                let n = self.pack_need(&o.step, &ts);
                if beside && n.cold && (cold_here || self.never_measured(&o.step)) {
                    cold_why.get_or_insert_with(|| {
                        if cold_here {
                            format!("{} isn't measured: one cold start on a Mac at a time", o.step)
                        } else {
                            format!("{} was never measured: its first job goes only to a Mac running nothing", o.step)
                        }
                    });
                    continue;
                }
                let (need_threads, need) = self.need_cores(&o.step, &n, total);
                if beside && need > cores {
                    if short.is_none() {
                        short = Some((o.step.clone(), need));
                    }
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
                        "nothing offered fits the room left ({} memory{}) or is free to take",
                        gb(room.mem_mb),
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
            // (Its threads past the cores left by those it leaves idle (its threads less its busy
            // cores), whole: they add no load.)
            let idle_ok = threads.min((cores + (threads * 10).saturating_sub(busy)) / 10);
            let threads = threads.min((cores / 10).max(idle_ok)).max(1);
            // (A planned job of no targets of its own is offered under its id's, keyless, to be
            // measured by it: its lease holds none, as its hand-off names none.)
            let pick = if o.job.is_some() { pick.into_iter().filter(|t| !t.1.is_empty()).collect() } else { pick };
            // (A cold start's too: its step unmeasured, its cache's fill may be.)
            let fill = self.cache_fill(&o.step, &caches, a);
            grants.push(self.grant(a, o, pick, threads, fill, clock));
            held += 1;
            caches.extend(crate::needs::cache_of(&o.step));
            cold_here |= n.cold;
            let m = n.m;
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

    /// A job of offer `o`'s step over `pick` granted to `a`'s worker with `threads` (`fill`: the disk
    /// its shared cache is reserved, added to its own: `cache_fill`): its lease, the worker's last
    /// target of the step, its targets' waits on the NAS budget let go.
    fn grant(
        &mut self,
        a: &Ask,
        o: &Offer,
        pick: Vec<(String, String)>,
        threads: u32,
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
                    let w = Self::job_nas(&nw, &o.step, ts.iter().map(String::as_str)).0;
                    for t in aged.into_iter().filter(|t| nw.weight(&o.step, t) <= w) {
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
        let disk_mb = self.job_need(&o.step, &ts).map_or(0, |(m, _)| m.disk_mb) + fill;
        Pick { lease, step: o.step.clone(), targets: pick, threads, disk_mb, job: o.job.clone() }
    }

    /// A member around on an older app (its last ask said no capacity, `Ask::cap`; an agent's,
    /// not a page's), the first by name; None when every one reports the new app. While there's
    /// one, every ask is granted one job (`pack`) and the old wire is still written
    /// (`crate::agent::Status::all_new`).
    pub(super) fn old_member(&self, now: Instant) -> Option<String> {
        self.workers
            .iter()
            .filter(|(_, w)| w.kind == "native" && !w.bad && now.saturating_duration_since(w.seen) < AROUND)
            .find(|(_, w)| w.ask.is_some() && w.old)
            .map(|(n, _)| n.clone())
    }

    /// Whether the lead's own planned job of offer `o` (`Offer::job`) is open to `a`'s worker: its
    /// targets (some), none held (a cancel's hold until a newer app among them: `Leases::held`),
    /// done under its key or kept from the worker after it failed, and no other job of its step
    /// runs anywhere (one job of a step kept to the lead at a time).
    pub(super) fn own_open(&self, o: &Offer, a: &Ask, now: Instant) -> bool {
        if o.targets.is_empty()
            || self.leases.all(now).iter().any(|l| matches!(&l.work, Work::Job { step, .. } if *step == o.step))
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

    /// Whether the lead's own planned job of offer `o` waits for room: open (`own_open`), a cold
    /// start (none of it measured) that can't go beside the lead's jobs: its provisional need past
    /// the room `left`, its step never measured, a cold start held there already (`cold_held`), or
    /// its cores the whole Mac (`need_cores`). The lead's ask is then granted nothing more until
    /// its jobs end enough for it, or all of them (`pack`, `pick_own`), so the offered work it would
    /// otherwise keep taking doesn't starve it.
    pub(super) fn own_cold_waits(&self, o: &Offer, a: &Ask, left: u64, cold_held: bool, now: Instant) -> bool {
        let ts: Vec<String> = o.targets.iter().map(|t| t.0.clone()).collect();
        o.job.is_some()
            && !crate::agent::steps::records_only(&o.step)
            && self.job_need(&o.step, &ts).is_none()
            && (self.provisional(&o.step, &ts).mem_mb > left
                || cold_held
                || self.never_measured(&o.step)
                || !THREADS_HONOURED.contains(&o.step.as_str()))
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
            let (w, fetch) = Self::job_nas(&nw, &o.step, ts.iter().map(String::as_str));
            let (running, heaviest) = if alone { (members, most[1]) } else { (running, most[0]) };
            if fetch && nw.limits.downloads.is_some_and(|c| dl >= c) {
                return false;
            }
            let own = running.saturating_sub(members);
            // (A quiet step, the clean-up: alone or within the whole by its measured weight, never
            // aged past it, its wait not noted: it never holds room, crate::budget::nas::QUIET_STEPS.)
            // Held back `QUIET_AGE` (a pool that stays busy), it goes as an aged job of the lead's
            // own, its wait noted so its room is held: never starved for good.
            let quiet = crate::budget::nas::QUIET_STEPS.contains(&o.step.as_str()) && {
                let mut g = self.quiet_since.lock().unwrap_or_else(|e| e.into_inner());
                let since = if alone {
                    g.get(&o.step).copied().unwrap_or(now)
                } else {
                    *g.entry(o.step.clone()).or_insert(now)
                };
                now.saturating_duration_since(since) < crate::budget::nas::QUIET_AGE
            };
            let waited = waited || (crate::budget::nas::QUIET_STEPS.contains(&o.step.as_str()) && !quiet);
            let fits = if quiet {
                // (Unmeasured, it goes only beside no NAS-heavy job: none of a step known to read
                // the NAS, none whose weight isn't measured yet.)
                crate::budget::nas::Limits::fits_quiet(running, w) && (w > 0 || !self.nas_unknown_running(&nw, now))
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
            Some((m, _)) => m.mem_mb <= a.mem_mb && a.disk_mb.is_none_or(|d| m.disk_mb <= d),
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
    /// spares, as many as fit `JOB_S` by their measured times (`secs_of`: one with none ends the job
    /// after its first), their disk summed (or the most a job of the step took, the larger) within
    /// its room. A cold start (`measured` None) by its provisional need, one job of its step in the
    /// pool at a time.
    pub(super) fn pick_every(&self, o: &Offer, a: &Ask, now: Instant) -> Vec<(String, String)> {
        let held = self.leases.held(&o.step, now);
        // (Each target's look-ups only where the worker failed any or the step has any done.)
        let (any_failed, any_done) = (self.failed_any(&a.worker), self.done_any(&o.step));
        let open = |t: &str, k: &str| {
            let backoff = any_failed
                && self.failed.get(&(a.worker.clone(), cost_key(&o.step, t))).is_some_and(|(at, n)| {
                    now.duration_since(*at)
                        < Duration::from_secs(3600) * 2u32.saturating_pow(n.saturating_sub(1).min(5))
                });
            !held.contains(t)
                && !(any_done && self.done.get(&(o.step.clone(), t.to_string())).map(String::as_str) == Some(k))
                && !backoff
        };
        let job_disk = self.job_floor(&o.step);
        let fits = |m: &Measured| m.mem_mb <= a.mem_mb && a.disk_mb.is_none_or(|d| m.disk_mb.max(job_disk) <= d);
        // (The NAS budget on: a target whose weight the pool's leased jobs leave no room for, or a
        // download past the cap, is left for later.)
        let lead = a.worker == self.lead_name;
        let nas = self.nas_room_heaviest(now, lead);
        let downloads_full = nas.as_ref().is_some_and(|(nw, _, _, dl, _)| {
            nw.limits.downloads.is_some_and(|c| *dl >= c) && Self::job_nas(nw, &o.step, []).1
        });
        if downloads_full {
            return Vec::new();
        }
        // (The lead's own asks held within the whole, or alone; a member's past it beside the
        // lead's own alone: crate::budget::nas::Limits.)
        let nas_fits = |t: &str| {
            nas.as_ref().is_none_or(|(nw, running, members, _, most)| {
                let w = nw.weight(&o.step, t);
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
                && (nas_fits(t) || {
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
            Some((p, since)) => self.placed_start(o, a, p, *since, now, &held, after, &takeable),
            None => after
                .filter(|&i| i < o.targets.len() && takeable(i))
                .or_else(|| (0..o.targets.len()).find(|&i| takeable(i))),
        };
        let Some(start) = start else { return Vec::new() };
        let first = &o.targets[start];
        let Some(m0) = self.measured(&o.step, &first.0) else { return vec![(first.0.clone(), first.1.clone())] };
        let mut pick = vec![(first.0.clone(), first.1.clone())];
        let (mut secs, mut disk) = (self.secs_of(&o.step, &first.0, &a.worker), m0.disk_mb);
        for (t, k, _) in &o.targets[start + 1..] {
            let (Some(s), Some(m)) = (secs, self.measured(&o.step, t)) else { break };
            let Some(more) = self.secs_of(&o.step, t, &a.worker) else { break };
            if pick.len() >= JOB_MAX
                || !open(t, k)
                || !fits(&m)
                || !nas_fits(t)
                || s + more > JOB_S
                || a.disk_mb.is_some_and(|d| crate::needs::batch_disk(&o.step, [disk, m.disk_mb]).max(job_disk) > d)
            {
                break;
            }
            pick.push((t.clone(), k.clone()));
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
                // (The cores it kept busy within the threads it was granted, else those threads.)
                Work::Job { step, targets } => {
                    let ts: Vec<String> = targets.iter().map(|t| t.0.clone()).collect();
                    let n = Need { idle: idle(l), ..self.pack_need(step, &ts) };
                    let counted = self.need_cores(step, &n, total).1;
                    match n.cores.and_then(|c| c.busy_tenths) {
                        _ if counted == 0 || l.threads == 0 => counted,
                        Some(_) => counted.min(l.threads * 10),
                        None => l.threads * 10,
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
        p: &crate::agent::placement::Placement,
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
                        && !w.bad
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
                    takes.then(|| (n.clone(), self.draw_weight(p, &o.step, n, x)))
                })
                .collect();
            who.push((a.worker.clone(), self.draw_weight(p, &o.step, &a.worker, a)));
            placement::draw(&placement::key(&format!("{} {t}", o.step), window), &who) == Some(a.worker.as_str())
        })
    }

    /// The memory (MB) a job under lease `l` takes: its heaviest target's, by the book's estimate
    /// (a step never measured: its table's or the offer's guess, not 0).
    pub(super) fn lease_mem(&self, l: &Lease) -> u64 {
        let Work::Job { step, targets } = &l.work else { return 0 };
        targets.iter().map(|t| self.measures(step, &t.0).0).max().unwrap_or(0)
    }

    /// Whether worker `n`, by its last ask `x` (made at `asked`), would take a target measured `m`
    /// now: a slot free (fewer jobs granted it since than the ask wanted, one for an ask saying
    /// none) and the memory its Mac had free then (`Capacity::mem_mb`, when its capacity is said),
    /// less that of the jobs granted it since (`lease_mem`), fits it.
    pub(super) fn would_take_now(&self, n: &str, asked: Instant, x: &Ask, m: &Measured, now: Instant) -> bool {
        let since: Vec<Lease> = self.job_leases(n, now).into_iter().filter(|l| l.granted >= asked).collect();
        let taken: u64 = since.iter().map(|l| self.lease_mem(l)).sum();
        since.len() < x.want.max(1)
            && x.cap.as_ref().filter(|c| c.limit_mb > 0).is_none_or(|c| m.mem_mb.saturating_add(taken) <= c.mem_mb)
    }

    /// Worker `n`'s weight in placement's draw for a target of `step`, as its last ask `x` says:
    /// its measured speed at the step (`Placement::weight`), `CACHE_PREFER` times that where its
    /// Mac holds the step's shared cache whole (`holds_cache`).
    pub(super) fn draw_weight(&self, p: &crate::agent::placement::Placement, step: &str, n: &str, x: &Ask) -> f64 {
        let cache = if self.holds_cache(step, x) { crate::agent::placement::CACHE_PREFER } else { 1.0 };
        p.weight(n, step) * cache
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
}

impl State {
    /// The cores `a`'s Mac has past those busy outside the pool as its ask says (`outside_cores`,
    /// past the cores the jobs it holds here count for: each its targets' most kept busy
    /// (crate::needs::Cores::busy_tenths, summed in tenths) within its threads, else its threads): the forecast's room for
    /// its jobs (task #684).
    pub fn cores_left(&self, a: &Ask) -> u32 {
        let b = crate::needs::Book::new(&self.costs);
        let ours: u32 = self
            .leases
            .iter()
            .filter(|l| l.worker == a.worker)
            .map(|l| {
                let busy = l.targets.iter().map(|t| b.cores(&l.step, &t.0)?.busy_tenths).collect::<Option<Vec<u32>>>();
                busy.and_then(|v| v.into_iter().max()).map_or(l.threads * 10, |c| c.min(l.threads * 10))
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
}

/// A worker's last ask (`State::workers`): the ask, whether it ran an older app (`Worker::old`) or
/// was found wrong (`Worker::bad`), and the ages of its last request and ask.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub ask: Ask,
    pub old: bool,
    pub bad: bool,
    pub seen_s: u64,
    pub asked_s: Option<u64>,
}

/// `now` less `age` seconds (as far back as an `Instant` goes).
fn ago(now: Instant, age: u64) -> Instant {
    now.checked_sub(Duration::from_secs(age)).unwrap_or(now)
}

impl Sched {
    /// Its state as data at `now` (`State`).
    pub fn state(&self, now: Instant) -> State {
        let age = |t: Instant| now.saturating_duration_since(t).as_secs();
        State {
            leases: self
                .leases
                .all(now)
                .into_iter()
                .filter_map(|l| match l.work {
                    Work::Job { step, targets } => Some(Leased { worker: l.worker, step, targets, threads: l.threads }),
                    Work::Task { .. } => None,
                })
                .collect(),
            offers: self.offers.clone(),
            done: self.done.iter().map(|((s, t), k)| (s.clone(), t.clone(), k.clone())).collect(),
            failed: self.failed.iter().map(|((w, k), (at, n))| (w.clone(), k.clone(), age(*at), *n)).collect(),
            workers: self
                .workers
                .values()
                .filter_map(|w| {
                    Some(Asked {
                        ask: w.ask.clone()?,
                        old: w.old,
                        bad: w.bad,
                        seen_s: age(w.seen),
                        asked_s: w.asked.map(age),
                    })
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
        }
    }

    /// A scheduler in state `s` at `now` (`clock`), its leases lasting `ttl` without a beat: the
    /// leases out granted again (fresh ids), each worker's ask as it was.
    pub fn from_state(s: &State, clock: Clock, ttl: Duration) -> Sched {
        let now = clock.now;
        let mut leases = Leases::new(ttl);
        leases.app = s.app.clone();
        for l in &s.leases {
            let id = leases.grant_at(
                &l.worker,
                Work::Job { step: l.step.clone(), targets: l.targets.clone() },
                now,
                clock.unix,
            );
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
                        visible: a.visible,
                        cap: a.cap.clone(),
                        ask: Some(a.clone()),
                        old: w.old,
                        seen: ago(now, w.seen_s),
                        asked: w.asked_s.map(|x| ago(now, x)),
                        what: String::new(),
                        done: 0,
                        failed: 0,
                        checked: 0,
                        bad: w.bad,
                    },
                )
            })
            .collect();
        let mut sched = Sched {
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
            let mine = self.job_leases(n, now);
            let granted = mine.iter().filter(|l| l.granted >= now).count();
            let running: u64 = mine.iter().filter(|l| l.granted < now).map(|l| self.lease_mem(l)).sum();
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
