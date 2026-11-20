//! The build's forecast (docs/building.md §2, The forecast; the build page's road to done): when each
//! step, each region and the whole build will be done, when each region reaches the map, when the
//! cadenced publishes may next go out, and what each machine does next.
//!
//! The work left is run through as the pool runs it: each Mac's slots (a lane each: as many as it
//! may run now, and the slots the lead's ramp grows to later, `Machine::from_s`) ask, as one frees,
//! the coordinator's own scheduler for a job each (`run`), offered the work ready in the plan's
//! order (the pass's worldwide jobs, the map tiles of the z6 tiles built, the slope and tree cover of
//! a region waiting for the map, the regions' terrain and z6 tiles a region at a time, the slope and
//! tree cover, the chains'), and granted what fits the Mac's room beside what it runs, as the
//! coordinator packs an ask (a member never the lead's own steps; `MacRoom`: its memory and its
//! cores, a thread each where no run said its threads; a step not measured by its provisional
//! need, one of it at a time; a job an ask while a member runs an older app). Each lane starts with
//! its slot's job under way. The trains' and the landmarks' chains run from the start, each step once what it reads is
//! built. Publishing goes per z6 tile (docs/building.md §2, Updating the map): each map tile of a region
//! (`RegionLeft::packs`, at its own measured time) is drawn once the z6 tiles that draw it are built,
//! ahead of the regions' work, and a catalogue follows within the minute (publish::CATALOGUE_EVERY_S):
//! the region is all on the map then (none said once only slope and tree cover are left). The whole-map steps go by their cadence (`Forecast::publishes`:
//! build::low_gate, build::gate_open). Each target of a job granted takes its needs over its Mac's
//! supply then (`needs_secs`, task #1413: its own run at the two Macs' speeds, else its step's needs
//! model's work at the Mac's speed over the threads the job was given, plus the rest of its time, or
//! its NAS moves at the NAS budget where longer; its job's start on the first), and each job under way
//! its needs less how long it has run (`lease_busy`); what is left of a stopped run, or a target of a
//! step not measured the way it runs now (a first guess), at its cost at its lane's pace. It's run
//! three times: as estimated, and for a range, the times measured a little off and those guessed much
//! more.

use super::publish::{CATALOGUE_EVERY_S, LOW_EVERY_S, MARKS_EVERY_S, OVERLAYS_EVERY_S};
use super::seam::RegionLeft;
use crate::coord::sched::{Clock, Sched, State};
use crate::coord::{cost_key, Ask, Capacity, Offer};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// A read target (`read_targets`): one target of a step, or a step's every target.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadTarget {
    One(&'static str, String),
    Whole(&'static str),
}

/// A machine the work is shared among.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub name: String,
    /// Its speed against the lead's (1), and whether that's measured.
    pub speed: f64,
    pub measured: bool,
    pub mem_mb: u64,
    /// Seconds from now until it's free (its job under way's time left).
    pub busy_s: f64,
    /// What each z6 tile takes it more (seconds): the lead's, while a worker that takes tails is
    /// around, the moment its job gives one to take its tail (::offload::LEASE_WAIT). (A
    /// terrain z6 tile's subtrees out with workers, terrain::terrain_task, take none: they're offered as
    /// the z6 tile begins and waited for only after its own run, by then taken or not, and its
    /// measured time, the job's, has those waits in it.)
    pub unit_extra_s: f64,
    /// It may run the lead's own steps (`every::Place::Lead`): the lead's slots; a member's take
    /// only those any Mac may (crate::agent::every::grants).
    pub lead_steps: bool,
    /// The job its slot runs now, when no item of the work left is it (a step's job the plan lists
    /// no target of, a daily one): its step and targets, until `busy_s`. On its lane first.
    pub running: Option<(String, usize)>,
    /// The Mac its slot is on: the scheduler grants its Mac's jobs (`run`), packed with its other
    /// slots'. None: a Mac of its own.
    pub mac: Option<String>,
    /// Seconds from now until its slot opens: a slot the lead's ramp grows to later (one each
    /// clean stretch, crate::agent's RAMP_CLEAN_S); 0, open now.
    pub from_s: f64,
}

/// A Mac's room for the pool's jobs: its memory (MB; 0, not known) and cores. When the scheduler's
/// state records no ask of it (`Input::macs`), as it is now; its sustained best (`sustained`, a
/// member's ask's `Capacity::best`), what the run takes it to have from now on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MacRoom {
    pub mem_mb: u64,
    pub cores: u32,
}

/// How long a Mac's room must hold to count as its sustained best (`sustained`), and how far back
/// it's looked for: the owner's choices (4 Nov, task #951: the best value held continuously for 30
/// minutes over the last 48 hours), not measured.
pub const BEST_WINDOW_S: u64 = 30 * 60;
pub const BEST_SPAN_S: u64 = 48 * 3600;

/// The highest value `points` held continuously for `BEST_WINDOW_S` (the highest of the rolling
/// minimums over that window) in the `BEST_SPAN_S` to `now`: each point (time, value) stands until
/// the next, at most `step` s (and never past `now`), a later point past that breaking the run.
/// None: no run that long. A
/// Mac's room as the forecast takes it from now on (`MacRoom`): its quietest sustained state, a
/// best case that foresees no use of the owner's and no passing load.
pub fn sustained(points: &[(u64, f64)], step: u64, now: u64) -> Option<f64> {
    let from = now.saturating_sub(BEST_SPAN_S);
    let mut p: Vec<(u64, f64)> =
        points.iter().copied().filter(|&(t, v)| t > from && t <= now && v.is_finite()).collect();
    p.sort_by_key(|x| x.0);
    let mut best: Option<f64> = None;
    for i in 0..p.len() {
        let (t0, mut lo) = p[i];
        for j in i..p.len() {
            if j > i {
                // (A gap: the run ends short of the window.)
                if p[j].0 > p[j - 1].0 + step {
                    break;
                }
                lo = lo.min(p[j].1);
            }
            let stands = match p.get(j + 1) {
                Some(n) if n.0 <= p[j].0 + step => n.0 - p[j].0,
                _ => step.min(now - p[j].0),
            };
            if p[j].0 + stands >= t0 + BEST_WINDOW_S {
                best = Some(best.map_or(lo, |b: f64| b.max(lo)));
                break;
            }
        }
    }
    best
}

/// Which figure a target's time is (`Cost::basis`): the build page counts a step's targets by it.
/// Declared in the order of how far they are measured: the first three (`measured`) of the target
/// itself or of the lead's own jobs of its step, the last three estimates. The order they're tried
/// in is another: a stopped run's, a run of its own, its step's needs model, then its step's mean
/// over its other targets', where that has none the lead's jobs of the step, then the guess (a step
/// only the lead runs is costed by its jobs here, then the guess, never by a mean over targets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// What is left of a run that stopped short (a pause, a lapse, a restart: its progress's done,
    /// total and time, `partials`), at that run's own pace.
    Partial,
    /// The target's own finished run, the way its step runs now (crate::needs::Book::own).
    Own,
    /// What a target of its step took in the lead's own jobs lately (`Memory::step_secs`): measured
    /// of the step itself where the lead alone runs it.
    Here,
    /// Its step's needs model for its size, an estimate for a target not run itself, as the
    /// coordinator sizes it (crate::needs::Book::secs_on).
    Model,
    /// Its step's mean over its other targets' runs, where its step has no needs model (a drop's:
    /// its step's drops').
    Sibling,
    /// A first guess (`first_secs`): nothing of its step measured.
    #[default]
    Guess,
}

impl Basis {
    /// Whether the time is measured of the target itself, or of the lead's own run of its step.
    pub fn measured(self) -> bool {
        self <= Basis::Here
    }
}

/// What a target takes: its time at the lead's pace (seconds), which figure it comes from
/// (`Basis`), and its memory (MB). A record from before the basis said `known`: its own run's, or a
/// guess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "CostRead")]
pub struct Cost {
    pub secs: f64,
    pub basis: Basis,
    pub peak_mb: u64,
}

impl Cost {
    /// Whether its time is measured (`Basis::measured`): the forecast's guess spread (`scaled`)
    /// and its measured share leave the rest out.
    pub fn known(&self) -> bool {
        self.basis.measured()
    }
}

#[derive(Deserialize)]
struct CostRead {
    secs: f64,
    #[serde(default)]
    known: Option<bool>,
    #[serde(default)]
    basis: Option<Basis>,
    #[serde(default)]
    peak_mb: u64,
}

impl From<CostRead> for Cost {
    fn from(r: CostRead) -> Cost {
        let basis = r.basis.unwrap_or(if r.known == Some(true) { Basis::Own } else { Basis::Guess });
        Cost { secs: r.secs, basis, peak_mb: r.peak_mb }
    }
}

/// A job of the lead's alone, or one of the chains': (step, target, cost).
pub type Job = (String, String, Cost);

/// The work left, as the plan sees it (seam::Planned), and what each target takes: all of it data,
/// so the lead's, written to the NAS each time it's made (`Record`), gives another Mac the same
/// forecast.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Input<'a> {
    pub now: u64,
    /// The lead's jobs before the regions' (the pass's worldwide jobs, the heritage sites).
    pub before: Vec<Job>,
    /// The regions' work left, in the order they're built (seam::Planned::regions).
    pub regions: Cow<'a, [RegionLeft]>,
    /// The drops (seam::Planned::drops: targets of the "none" keys, their packs dropped),
    /// each at its drop's cost: first of the regions' work, waiting on nothing, shown as drops
    /// (`Stretch::drop`). A region whose terrain they're in waits on them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drops: Vec<Job>,
    /// What each region's target takes, by step and target (`cost_table`: every target of
    /// `regions` the forecast lists).
    pub costs: CostTable,
    /// The cadenced publishes' last runs.
    pub cadence: Cadence,
    /// Why the work left can't all be listed now, when it can't (the z6 tiles waiting for the pass's
    /// heritage sites, reaches or buildings; a new pass under way): no finish is forecast.
    pub blind: Option<String>,
    /// The trains' and the landmarks' chains (but the overlays): from the start, each step once what
    /// it reads is built (`Waits::of`).
    pub chains: Vec<Job>,
    /// After everything else: the overlays (they read the built z6 tiles) and a catalogue with what the
    /// chains made since.
    pub after: Vec<Job>,
    /// Last of all, in idle time: the tree cover z6 tiles' mids (build::TreeWork::backfill), each
    /// once everything else is done, side by side.
    pub idle: Vec<Job>,
    /// The lead first, then the members.
    pub machines: Vec<Machine>,
    /// Targets being built now: (step, target) → the machine building it.
    #[serde(with = "pairs")]
    pub running: BTreeMap<(String, String), usize>,
    /// Each Mac's room, by the name its lanes give (`Machine::mac`), for a Mac `sched` records no
    /// ask of.
    pub macs: BTreeMap<String, MacRoom>,
    /// The lead's coordinator's scheduler as the forecast was made (crate::coord::sched::State):
    /// its leases, measures, NAS budget and each Mac's last ask. The run asks a scheduler in this
    /// state for every job (`run`). None (a test, or no coordinator): the work's measures alone.
    #[serde(default)]
    pub sched: Option<State>,
    /// Each step's jobs' time against what their grants sized them to (`needs_pace`, code 9 on):
    /// every item the run times by its needs is at its needs times this; a step not in it, at its
    /// needs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub needs_pace: BTreeMap<String, f64>,
    /// How an earlier forecast's run fared against the work done since (`pace`): the live error,
    /// said beside the finish. None: no earlier forecast to hold to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
    /// Targets the plan holds until another is made (`Unknown`; code 5 on).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<Unknown>,
    /// The build is paused (jobrt::control::Pause; code 3 on): nothing new starts until it's
    /// resumed, when no one can say, so the forecast places nothing (`PAUSED`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    /// What the regions' targets read along their long ways (crate::agent::steps::Read::Along), the
    /// plan's (`Left::along`): waited for as their other reads (`Waits::of`).
    #[serde(default, skip_serializing_if = "crate::agent::seam::Along::is_empty")]
    pub along: crate::agent::seam::Along,
    /// The forecast code it's run as (`VERSION`; not recorded): the backtest's replay sets the
    /// code before (crate::agent::fccheck::replay) to rerun a record as that code made it.
    #[serde(skip, default = "version")]
    pub code: u32,
}

/// `VERSION`, as an `Input::code` read back.
fn version() -> u32 {
    VERSION
}

/// Targets of `step` the plan holds until `until` is made (crate::agent::seam::UnknownFor: an
/// input of theirs can't be worked out before it, a slope tile reading the terrain's root while it's
/// stale): each waits for it, a job of its own in the run once what it reads is built.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unknown {
    pub step: String,
    pub targets: Vec<String>,
    pub until: Job,
}

/// Whether the build was paused at `now` by the history (`events`): its last "pause" or "resume"
/// by then a pause.
pub fn paused(events: &[crate::coord::history::Event], now: u64) -> bool {
    events
        .iter()
        .filter(|e| e.t <= now && matches!(e.kind.as_str(), "pause" | "resume"))
        .max_by_key(|e| (e.t, e.seq))
        .is_some_and(|e| e.kind == "pause")
}

/// The forecast's `why` while the build is paused (`Input::paused`).
pub const PAUSED: &str = "the build is paused: nothing new starts until it's resumed";

/// What target `target` of `step` takes Mac `mac` on `threads` (s; 0: as many as its step can use):
/// its needs over the Mac's supply now (task #1413). Its time there by its needs
/// (crate::coord::sched::Sched::secs_on: its own run at the two Macs' speeds, else its step's model's
/// work at the Mac's speed over the cores it is given, plus the rest of its time), or the time its
/// NAS moves take at the NAS budget (`nas_secs`), the longer. None: none of its step ran the way it
/// runs now.
pub fn needs_secs(s: &Sched, step: &str, target: &str, mac: &str, threads: u32) -> Option<f64> {
    let (secs, _) = s.secs_on(step, target, mac, threads)?;
    Some((secs as f64).max(nas_secs(s, step, target, mac)))
}

/// The span of the history `needs_pace` reads (s), and the jobs of a step it needs.
pub const NEEDS_PACE_S: u64 = 12 * 3600;
const NEEDS_PACE_JOBS: usize = 3;
/// `needs_pace`'s floor: a step's few jobs never cut its needs more than fourfold.
const NEEDS_PACE_FLOOR: f64 = 0.25;

/// Each step's pace against its needs (`Input::needs_pace`, task #1413): over its jobs done well in
/// the `NEEDS_PACE_S` before `now` whose grant said the time it was sized to (the history's "lease"
/// event's `sized_s`: its needs over its Mac's supply then, crate::coord::sched::Sched::batch_secs),
/// their time over that, summed; a step with `NEEDS_PACE_JOBS` such jobs at least, no less than
/// `NEEDS_PACE_FLOOR`. Only a step quicker than its needs is said: the needs' figures run high
/// (#1415), and a job that ran long mostly waited on what else ran (the NAS, its Mac's other jobs),
/// which the run already times; scaling slow steps up too did worse on the backtest's train set
/// (pool.md §7.9, code 9).
pub fn needs_pace(events: &[crate::coord::history::Event], now: u64) -> BTreeMap<String, f64> {
    let from = now.saturating_sub(NEEDS_PACE_S);
    let mut sized: std::collections::HashMap<jobrt::jobid::JobId, f64> = Default::default();
    let mut sums: BTreeMap<&str, (f64, f64, usize)> = BTreeMap::new();
    for e in events.iter().filter(|e| e.t <= now) {
        let Some(job) = e.job else { continue };
        match e.kind.as_str() {
            "lease" => {
                // (In memory its field, in a file its `data`: crate::coord::history's `keep`.)
                let in_data = || e.data.as_ref()?.get("sized_s")?.as_f64();
                if let Some(s) = e.sized_s.map(f64::from).or_else(in_data) {
                    sized.insert(job, s);
                }
            }
            "done" | "end" if e.t > from && e.ok == Some(true) => {
                let (Some(step), Some(secs), Some(&want)) = (e.step.as_deref(), e.secs, sized.get(&job)) else {
                    continue;
                };
                if want > 0.0 && secs > 0.0 {
                    let x = sums.entry(step).or_default();
                    (x.0, x.1, x.2) = (x.0 + secs, x.1 + want, x.2 + 1);
                }
            }
            _ => {}
        }
    }
    sums.into_iter()
        .filter(|(_, x)| x.2 >= NEEDS_PACE_JOBS)
        .map(|(step, (took, want, _))| {
            let p = (took / want).max(NEEDS_PACE_FLOOR);
            (step.to_string(), (p * 100.0).round() / 100.0)
        })
        .filter(|(_, p)| *p < 1.0)
        .collect()
}

/// The least time (s) target `target` of `step`'s NAS moves take Mac `mac` at the NAS budget: what
/// it moves (crate::needs::Book::nas_load: its rate there over its time there) as a share of what
/// the NAS moves a second (`Limits`, the pool's measure of it, each kind of move added up as
/// `Limits::weight` adds them, not capped), over the share the pacing's window lets the pool's jobs
/// hold together (`Limits::window`). 0 while no probe measured the NAS, or no run of its step moved
/// anything and took time.
pub fn nas_secs(s: &Sched, step: &str, target: &str, mac: &str) -> f64 {
    let Some(l) = s.nas.as_ref().or(s.nas_own.as_ref()) else { return 0.0 };
    let Some((r, Some(secs))) = s.book().nas_load(step, target, Some(mac)) else { return 0.0 };
    let part = |x: f64, of: f64| if of > 0.0 { x / of } else { 0.0 };
    let whole_s = (part(r.bytes_s, l.bytes_s)
        + part(r.creates_s, l.creates_s)
        + part(r.listed_s, l.listed_s)
        + part(r.stats_s, l.stats_s)
        + part(r.deletes_s, l.deletes_s))
        * secs;
    let budget = f64::from(l.window.max(1)) / f64::from(jobrt::budget::nas::WHOLE);
    let t = whole_s / budget;
    if t.is_finite() {
        t
    } else {
        0.0
    }
}

/// Each lane's job under way timed by its needs over its Mac's supply (code 9 on, task #1413): for
/// each job leased in the record's scheduler (`Input::sched`), its targets' times on its Mac at the
/// threads it holds (`needs_secs`, its job's start at its step on top: crate::paces::Paces::start)
/// less how long it has run, a minute at least, as its lane's `busy_s`. Its lane: the one running
/// its targets, else its Mac's lane running its step. A job none of whose step ran the way it runs
/// now keeps its lane's `busy_s` (its slot's own estimate).
pub fn lease_busy(the_plan: &'static dyn crate::agent::seam::Plan, inp: &mut Input) {
    let Some(st) = inp.sched.as_ref() else { return };
    let now = Instant::now();
    let s = Sched::from_state(the_plan, st, Clock { now, unix: inp.now }, RUN_TTL);
    let mut set: Vec<(usize, f64)> = Vec::new();
    for l in s.leases.all(now) {
        let crate::coord::lease::Work::Job { step, targets } = &l.work else { continue };
        // (Its lane by a target it runs, else a lane of its worker's Mac (a slot's worker, "m4 #2",
        // is its Mac's) running its step that no other lease has timed.)
        let lane = targets.iter().find_map(|t| inp.running.get(&(step.clone(), t.0.clone())).copied()).or_else(|| {
            (0..inp.machines.len()).find(|&k| {
                mac_of(&inp.machines[k]) == crate::paces::mac_of(&l.worker)
                    && inp.machines[k].running.as_ref().is_some_and(|r| r.0 == *step)
                    && !set.iter().any(|x| x.0 == k)
            })
        });
        let Some(k) = lane.filter(|&k| k < inp.machines.len() && !set.iter().any(|x| x.0 == k)) else { continue };
        let mac = mac_of(&inp.machines[k]);
        let Some(secs) = targets.iter().map(|t| needs_secs(&s, step, &t.0, &mac, l.threads)).sum::<Option<f64>>()
        else {
            continue;
        };
        let pace = inp.needs_pace.get(step.as_str()).copied().unwrap_or(1.0);
        let left = (s.paces.start(&mac, step) + secs) * pace - l.ran(now).as_secs_f64();
        set.push((k, left.max(60.0)));
    }
    for (k, b) in set {
        inp.machines[k].busy_s = b;
    }
}

/// The lead's steps on its own clock, not the plan's dependencies: the catalogues and low zooms
/// published, the backups, the sweeps, the inputs' refreshes, the names' and timetables' todo
/// lists, the tasks. Left out of `Forecast::curve` and of the work `pace` counts done.
pub const CLOCKED: [&str; 9] = [
    store::catalogue::NAME,
    store::catalogue::HELD_NAME,
    "lo",
    "backup",
    "gc",
    "inputs",
    "names-todo",
    "timetables-todo",
    "task",
];

/// The lead's duties the forecast doesn't model (docs/building.md, The forecast): the jobs its
/// plan makes outside the regions' work (crate::agent::plan's `plan_unchecked`, besides
/// `region_work`), whose times the plan can't foresee: the OSM planet's fetch and a new pass (the
/// forecast says it's blind to a new pass's work instead), the daily backup, the owner's redos and
/// GC (its chunks due as the clients' cursors and the leases move, granted by the NAS budget's
/// room then). Never in the forecast's input (the plan's test
/// `the_jobs_planned_outside_the_forecast_are_the_ones_it_says_it_doesnt_model` holds every job
/// made outside the regions' work to this list), and the departure check leaves them out
/// (`departures`): its zero for them is no forecast at all.
pub const UNFORESEEN: [&str; 5] = ["planet-fetch", "osm-pass", "backup", super::redo::STEP, "gc"];

/// Whether the forecast models jobs of `step` (`UNFORESEEN`).
pub fn foreseen(step: &str) -> bool {
    !UNFORESEEN.contains(&step)
}

/// The spacing of a forecast's `curve` (s), and how far it reaches (its points).
pub const CURVE_S: u64 = 600;
const CURVE_MAX: usize = 288;

/// The span of an earlier forecast `pace` holds to: made at least `PACE_MIN_S` and at most
/// `PACE_MAX_S` ago (the oldest such).
pub const PACE_MIN_S: u64 = 3600;
pub const PACE_MAX_S: u64 = 6 * 3600;

/// The live error (task #563): an earlier forecast's run (made at `since`) against the planned
/// work done since (the history's targets done, `CLOCKED` steps' aside). Its run did `real_n`
/// targets in `run_s` where they took `real_s`; `error` = run_s / real_s − 1 (−0.6: the run 60 %
/// early). Said, not applied: the run models what holds work back (each Mac's room, the
/// scheduler's rules), not the pace of the day before.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pace {
    pub since: u64,
    pub real_n: u32,
    pub real_s: u64,
    pub run_s: u64,
    pub error: f64,
}

/// The live error (`Pace`) at `now`: the oldest of `past` (each forecast's `at` and `curve`) made
/// `PACE_MIN_S`–`PACE_MAX_S` before, with no pause since, held to the targets `events` show done
/// since it (a job of none, one). None: no such forecast, or no planned work done since.
pub fn pace(past: &[(u64, Vec<u32>)], events: &[crate::coord::history::Event], now: u64) -> Option<Pace> {
    let paused =
        |a: u64| events.iter().any(|e| e.t >= a && e.t <= now && matches!(e.kind.as_str(), "pause" | "resume"));
    let (since, curve) = past
        .iter()
        .filter(|(a, c)| now >= a + PACE_MIN_S && now <= a + PACE_MAX_S && !c.is_empty() && !paused(*a))
        .min_by_key(|(a, _)| *a)?;
    let real_n: u32 = events
        .iter()
        .filter(|e| e.t > *since && e.t <= now && matches!(e.kind.as_str(), "done" | "end"))
        .filter(|e| e.step.as_deref().is_some_and(|s| !CLOCKED.contains(&s)))
        .map(|e| e.targets.len().max(1) as u32)
        .sum();
    if real_n == 0 {
        return None;
    }
    // (When the run had done as many: between its points; past its last, at its last's rate.)
    let k = curve.iter().position(|&n| n >= real_n);
    let run_s = match k {
        Some(0) => 0.0,
        Some(k) => {
            let (a, b) = (curve[k - 1] as f64, curve[k] as f64);
            ((k - 1) as f64 + (real_n as f64 - a) / (b - a).max(1.0)) * CURVE_S as f64
        }
        None => {
            let last = *curve.last()? as f64;
            ((curve.len() - 1) as f64 * CURVE_S as f64) * real_n as f64 / last.max(1.0)
        }
    }
    .max(1.0);
    let real_s = (now - since) as f64;
    Some(Pace {
        since: *since,
        real_n,
        real_s: real_s as u64,
        run_s: run_s.round() as u64,
        error: run_s / real_s - 1.0,
    })
}

/// A run's `curve`: the planned targets (`CLOCKED` steps' aside) its items had done by each
/// `CURVE_S` from its start, to its last's end (`CURVE_MAX` points at most).
fn curve(items: &[Item]) -> Vec<u32> {
    let mut ends: Vec<f64> =
        items.iter().filter(|i| i.by.is_some() && !CLOCKED.contains(&i.step.as_str())).map(|i| i.end).collect();
    ends.sort_by(f64::total_cmp);
    let Some(&last) = ends.last() else { return Vec::new() };
    let n = ((last / CURVE_S as f64).ceil() as usize + 1).min(CURVE_MAX);
    (0..n).map(|k| ends.partition_point(|&e| e <= (k as u64 * CURVE_S) as f64) as u32).collect()
}

/// A slot's job under way, as its lane's time left is worked out (`SlotJob::busy_s`) for the lead's
/// forecast (crate::agent::plan's `forecast_record`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotJob {
    pub step: String,
    /// Its targets, each whether it's done.
    pub targets: Vec<(String, bool)>,
    /// A job with no targets listed (the pass's, a worldwide one): its step's time here.
    pub alone_s: Option<f64>,
    /// How long it has run (s), and its own estimate of its time left (s), when it said one.
    pub elapsed_s: f64,
    pub eta: Option<f64>,
}

impl SlotJob {
    /// Its time left (s) at `speed`: its targets not done (`cost` each, at the lead's pace) less
    /// what it spent on the one under way beyond those done, its own estimate if longer, a minute
    /// at least (`busy_s`).
    pub fn busy_s(&self, cost: &dyn Fn(&str, &str) -> f64, speed: f64) -> f64 {
        let left = match self.alone_s {
            Some(a) => a - self.elapsed_s,
            None => {
                let (mut todo, mut did) = (0.0, 0.0);
                for (t, done) in &self.targets {
                    let c = cost(&self.step, t);
                    if *done {
                        did += c
                    } else {
                        todo += c
                    }
                }
                todo - (self.elapsed_s - did).max(0.0)
            }
        };
        busy_s(self.eta.unwrap_or(0.0), left / speed.max(0.1))
    }
}

/// A lane's time left (s): a job's own estimate (`eta`) or the time its work left takes
/// (`left`), whichever's longer, a minute at least.
pub fn busy_s(eta: f64, left: f64) -> f64 {
    eta.max(left).max(60.0)
}

/// A measured cost `c` at the lead's pace (s): its time over its worker's Mac's time against the
/// lead's (crate::paces: a Mac not measured the lead's; a cost of no worker as it is);
/// with no paces worked out (no coordinator, a record before code 4), times its worker's `speed`
/// (the forecast's, against the lead's: `speeds`).
pub fn at_lead(c: &crate::coord::Cost, paces: &crate::paces::Paces, speed: &dyn Fn(Option<&str>) -> f64) -> f64 {
    if !paces.known() {
        return c.secs as f64 * speed(c.worker.as_deref());
    }
    c.secs as f64 / c.worker.as_deref().map_or(1.0, |w| paces.get(w))
}

/// Each step's measured costs at the lead's pace (`at`), weighed and summed, and their weights
/// (step → (Σ secs × weight, Σ weight)): each measured the way its step runs now
/// (crate::coord::cost_version), weighed by crate::needs::time_weight (a PER_WORK step's by the
/// work it did, task #830). A step's mean time a target is the one over the other.
pub fn step_sums(
    the_plan: &dyn crate::agent::seam::Plan,
    costs: &BTreeMap<String, crate::coord::Cost>,
    at: &dyn Fn(&crate::coord::Cost, &str) -> f64,
) -> BTreeMap<String, (f64, f64)> {
    let mut sums: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for (k, c) in costs {
        let step = crate::agent::memguard::step_of_key(k);
        if c.v < the_plan.cost_version(step) {
            continue;
        }
        let Some(w) = crate::needs::time_weight(step, c) else { continue };
        let e = sums.entry(step.to_string()).or_default();
        (e.0, e.1) = (e.0 + at(c, step) * w, e.1 + w);
    }
    sums
}

/// The time left of each target whose run stopped short (`Event::progress`), at the lead's pace,
/// by (step, target): the run's own pace (what its part made past what its first beat said, over
/// the time that took) over what is left, from the history's events, oldest first. A "start" or
/// "lease" names its job's targets; a stopped "end", "done" or "fail" of that job (not ok, of the
/// way its step runs now, `Event::v`) sets it for its progress's target (a cost key, its step the
/// event's: `CostKey::parse`; its one target when it names none). What is left is the part's
/// `total` less `done` where the step resumes, the whole `total` where it doesn't: a step resumes
/// when a later run of a stopped target counted a smaller total or began past where the stop was
/// (measured from the records, so a step with no run since a stop is taken to redo its target:
/// the longer time). A job that ended well clears its finished targets (`Event::targets`), or all
/// its job's when it lists none, a later stop replaces it, and a not-ok "done" that finished some
/// targets keeps the stop of the rest. `at` puts a time of a worker's at the lead's pace (`at_lead`).
pub fn partials(
    the_plan: &dyn crate::agent::seam::Plan,
    events: &[crate::coord::history::Event],
    at: &dyn Fn(f64, Option<&str>, &str) -> f64,
) -> BTreeMap<(String, String), f64> {
    struct Stop {
        done: f64,
        total: f64,
        s_per_unit: f64,
        worker: Option<String>,
    }
    let mut jobs: BTreeMap<jobrt::jobid::JobId, Vec<String>> = BTreeMap::new();
    let mut stops: BTreeMap<(String, String), Stop> = BTreeMap::new();
    let mut resumes: BTreeSet<String> = BTreeSet::new();
    for e in events {
        let (Some(job), Some(step)) = (e.job, e.step.as_deref()) else { continue };
        match e.kind.as_str() {
            "start" | "lease" => {
                jobs.insert(job, e.targets.clone());
            }
            "end" | "done" | "fail" => {
                let named = jobs.get(&job).filter(|t| !t.is_empty()).unwrap_or(&e.targets);
                if e.ok == Some(true) {
                    let cleared = if e.targets.is_empty() { named } else { &e.targets };
                    for t in cleared {
                        stops.remove(&(step.to_string(), t.clone()));
                    }
                    // (A job paused after finishing some of its targets is "done" well: its stop
                    // is of the rest, if its progress stood short.)
                    if e.targets.is_empty() || named.iter().all(|t| e.targets.contains(t)) {
                        continue;
                    }
                }
                let Some(p) = e.progress.as_ref().filter(|p| p.done > p.since && p.done < p.total && p.secs > 0.0)
                else {
                    continue;
                };
                if e.v.is_some_and(|v| v != the_plan.cost_version(step)) {
                    continue;
                }
                let key = p.target.as_deref().map(crate::coord::CostKey::parse).filter(|k| k.step == step);
                let target = match (key, named.as_slice()) {
                    (Some(k), _) => k.target,
                    (None, [t]) => t,
                    _ => continue,
                };
                let at_key = (step.to_string(), target.to_string());
                if stops.get(&at_key).is_some_and(|was| p.total < was.total || (p.since > 0.0 && p.since >= was.done)) {
                    resumes.insert(step.to_string());
                }
                let s_per_unit = p.secs / (p.done - p.since);
                stops.insert(at_key, Stop { done: p.done, total: p.total, s_per_unit, worker: e.worker.clone() });
            }
            _ => {}
        }
    }
    stops
        .into_iter()
        .map(|((step, t), s)| {
            let units = if resumes.contains(&step) { s.total - s.done } else { s.total };
            let secs = at(units * s.s_per_unit, s.worker.as_deref(), &step);
            ((step, t), secs)
        })
        .collect()
}

/// What a region's target of `step` takes (`cost_table`'s `cost`), at the lead's pace: what is left
/// of a run that stopped short, at its own pace (`partial`: `partials`); else its needs on the lead's
/// Mac `lead` (crate::needs::Book::secs_on: its own run where it ran the way its step runs now, the
/// measure the coordinator sizes its jobs by, `Basis::Own`; else its step's needs model for its
/// size, `Basis::Model`); else `per`, the nearest figure of its step's and where it comes from
/// (`Basis`); its memory `peak_mb`. (Task #1165: the table had known any run of a target, so 155 of
/// 165 terrain-deep targets on 12 Nov were known at 0-8 s from runs of cost version 0 that found no
/// tiffs. Task #1449: a target with a paused run, available-bathymetry's 2,169 s on 18 Nov, counted as
/// the guess's 300 s though its run said how far it had got.)
pub fn target_cost(
    book: &crate::needs::Book,
    lead: &str,
    step: &str,
    target: &str,
    partial: &dyn Fn(&str, &str) -> Option<f64>,
    per: &dyn Fn(&str) -> (f64, Basis),
    peak_mb: u64,
) -> Cost {
    if let Some(secs) = partial(step, target) {
        return Cost { secs, basis: Basis::Partial, peak_mb };
    }
    let own = book.own(step, target);
    let basis = if own.is_some() { Basis::Own } else { Basis::Model };
    // (Its own run on another Mac than the lead's at the two Macs' speeds; on the lead's, or of no
    // Mac said, as it ran; its step's model at the lead's.)
    let other =
        own.and_then(|c| c.worker.as_deref()).is_some_and(|w| crate::paces::mac_of(w) != crate::paces::mac_of(lead));
    match book.secs_on(step, target, 0, other.then_some(lead)) {
        Some((secs, _)) => Cost { secs: secs as f64, basis, peak_mb },
        None => {
            let (secs, basis) = per(step);
            Cost { secs, basis, peak_mb }
        }
    }
}

/// What each target of the regions takes, by step, then target.
pub type CostTable = BTreeMap<String, BTreeMap<String, Cost>>;

/// The targets of `works` done at the key they're planned at whose records the plan hasn't read
/// (the coordinator's done set, `done`: crate::coord::Coordinator::done_unread): a plan made from
/// records read before a job's hand-off was merged (such as one under way as the job ended) still offers
/// them, the coordinator grants none of them again, and none is work to come (task #1375).
pub fn unread<'a>(
    done: &BTreeMap<(String, String), String>,
    works: impl IntoIterator<Item = &'a super::jobs::Work>,
) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    if done.is_empty() {
        return out;
    }
    for w in works {
        for (t, k) in &w.targets {
            let at = (w.step.clone(), t.clone());
            if done.get(&at) == Some(k) {
                out.insert(at);
            }
        }
    }
    out
}

/// `regions` without the targets in `unread` (`unread`), each list by the step whose targets it
/// holds, as the run reads them.
pub fn without_unread(regions: &mut [RegionLeft], unread: &BTreeSet<(String, String)>) {
    if unread.is_empty() {
        return;
    }
    let keep = |step: &str, v: &mut Vec<String>| v.retain(|t| !unread.contains(&(step.to_string(), t.clone())));
    for r in regions {
        for (step, v) in [
            ("terrain-hi", &mut r.terrain),
            ("terrain-hi", &mut r.own_terrain),
            ("terrain-lo", &mut r.terrain_lo),
            ("terrain-lo", &mut r.own_terrain_lo),
            ("terrain-deep", &mut r.terrain_deep),
            ("terrain-deep", &mut r.own_terrain_deep),
            ("unit", &mut r.units),
            ("unit", &mut r.own_units),
            ("unit", &mut r.expected),
            ("slope-deep", &mut r.slope_deep),
            ("slope-hi", &mut r.slope),
            ("slope-lo", &mut r.slope_lo),
            ("trees", &mut r.trees),
            ("trees-lo", &mut r.trees_lo),
        ] {
            keep(step, v);
        }
        r.packs.retain(|(t, _)| !unread.contains(&("pack".to_string(), t.clone())));
    }
}

/// The cost table of `regions`' targets the forecast lists (`forecast`: each region's own terrain
/// and assemblies, own and expected z6 tiles, map tiles, slope and tree cover), each by `cost`.
pub fn cost_table(regions: &[RegionLeft], cost: &dyn Fn(&str, &str) -> Cost) -> CostTable {
    let mut t = CostTable::new();
    let mut put = |step: &str, ts: &mut dyn Iterator<Item = &String>| {
        let e = t.entry(step.to_string()).or_default();
        for x in ts {
            e.entry(x.clone()).or_insert_with(|| cost(step, x));
        }
    };
    for r in regions {
        put("terrain-hi", &mut r.own_terrain.iter());
        put("terrain-deep", &mut r.own_terrain_deep.iter());
        put("terrain-lo", &mut r.own_terrain_lo.iter());
        put("unit", &mut r.own_units.iter().chain(&r.expected));
        put("pack", &mut r.packs.iter().map(|p| &p.0));
        put("slope-deep", &mut r.slope_deep.iter());
        put("slope-hi", &mut r.slope.iter());
        put("slope-lo", &mut r.slope_lo.iter());
        put("trees", &mut r.trees.iter());
        put("trees-lo", &mut r.trees_lo.iter());
    }
    t.retain(|_, m| !m.is_empty());
    t
}

impl Input<'_> {
    /// The time (s) an item taking `secs` at the lead's pace takes lane `m`: at its Mac's time
    /// against the lead's (the scheduler's, `State::paces`, by the Macs' speeds; a Mac not measured
    /// the lead's); with no paces worked out (no scheduler, a record before them), at its speed.
    pub fn lane_secs(&self, m: &Machine, secs: f64) -> f64 {
        match self.sched.as_ref().filter(|s| s.paces.known()) {
            Some(s) => secs * s.paces.get(&mac_of(m)),
            None => secs / m.speed.max(0.01),
        }
    }

    /// What a region's target takes (`costs`). One not in it is a bug (`cost_table` lists them
    /// all): logged once a process, taken as nothing.
    pub fn cost(&self, step: &str, target: &str) -> Cost {
        if let Some(c) = self.costs.get(step).and_then(|m| m.get(target)) {
            return *c;
        }
        static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("forecast: no cost for {step} {target} in its input (a bug: cost_table lists every target)");
        }
        Cost::default()
    }
}

/// A map keyed by pairs, as a list of [key, value] (JSON's keys are strings).
mod pairs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::collections::BTreeMap;
    type M = BTreeMap<(String, String), usize>;
    pub fn serialize<S: Serializer>(m: &M, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(m.iter())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<M, D::Error> {
        Ok(Vec::<((String, String), usize)>::deserialize(d)?.into_iter().collect())
    }
}

/// The lead's forecast as data (docs/pool.md §10: `state/build/term/<E>/forecast-input.json`,
/// written each time the lead makes it): its input, and what the lead adds to `forecast`'s answer
/// (each step's time a target, the slots waiting on a tail). `Record::forecast` gives the lead's
/// forecast on any Mac.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record<'a> {
    pub input: Input<'a>,
    pub each_s: BTreeMap<String, f64>,
    pub waiting: BTreeMap<String, String>,
}

impl Record<'_> {
    /// The forecast it holds: the lead's.
    pub fn forecast(&self, the_plan: &'static dyn crate::agent::seam::Plan) -> Forecast {
        let mut f = forecast(the_plan, &self.input);
        f.each_s = self.each_s.clone();
        f.waiting = self.waiting.clone();
        f
    }

    /// Every time and speed in it made finite (JSON has no NaN nor infinity: one would read back
    /// as null and fail): NaN as 0, an infinity as the largest finite number of its sign.
    pub fn clamp(&mut self) {
        fn f(x: &mut f64) {
            if x.is_nan() {
                *x = 0.0
            } else if x.is_infinite() {
                *x = x.signum() * f64::MAX
            }
        }
        let m = |m: &mut Machine| {
            for x in [&mut m.speed, &mut m.busy_s, &mut m.unit_extra_s, &mut m.from_s] {
                f(x)
            }
        };
        let inp = &mut self.input;
        inp.machines.iter_mut().for_each(m);
        for j in inp.before.iter_mut().chain(&mut inp.chains).chain(&mut inp.after).chain(&mut inp.idle) {
            f(&mut j.2.secs)
        }
        inp.costs.values_mut().flat_map(|t| t.values_mut()).for_each(|c| f(&mut c.secs));
        self.each_s.values_mut().for_each(f);
    }

    /// As written (finite: `clamp`): JSON, zstd-compressed when it's over `ZSTD_OVER` bytes.
    pub fn bytes(&self) -> anyhow::Result<Vec<u8>> {
        let mut r = self.clone();
        r.clamp();
        let b = serde_json::to_vec(&r)?;
        if b.len() <= ZSTD_OVER {
            return Ok(b);
        }
        Ok(zstd::encode_all(&b[..], 3)?)
    }

    /// One read back from its bytes (compressed or not: zstd's frames start with its magic number).
    pub fn from_bytes(b: &[u8]) -> anyhow::Result<Record<'static>> {
        if b.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            return Ok(serde_json::from_slice(&zstd::decode_all(b)?)?);
        }
        Ok(serde_json::from_slice(b)?)
    }
}

/// A forecast record over this many bytes of JSON is written compressed (zstd).
pub const ZSTD_OVER: usize = 2 << 20;

/// Seconds since each cadenced publish last went out (None: not known, so due now): the low zooms
/// (the map log's last batch saying so: crate::agent::publish::LOW), the landmark points and the area overlays
/// (publish::LastCatalogue::since).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cadence {
    pub since_low: Option<u64>,
    pub since_marks: Option<u64>,
    pub since_overlays: Option<u64>,
}

/// The forecast's `why` when the build is done: no work left, nothing to publish and no machine
/// busy.
pub const NOTHING_LEFT: &str = "nothing left to build";

/// The forecast (`forecast`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Forecast {
    /// When it was made (unix seconds).
    pub at: u64,
    /// When everything will be done (unix seconds), and the range: soon, late. None: nothing's left,
    /// or the work can't all be listed now, or there's work no machine can do (`why`).
    pub done_at: Option<u64>,
    pub range: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The share of the time left that was measured (the rest guessed).
    pub measured: f64,
    pub steps: Vec<StepFc>,
    /// Each step's time for a target at the lead's pace (seconds), as the forecast takes it: its
    /// targets' measured mean, else what its jobs here took a target, else a first guess. The
    /// build page weighs the targets done by it (its headline: the work done of the whole).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub each_s: BTreeMap<String, f64>,
    pub regions: Vec<RegionFc>,
    /// When each cadenced publish may next go out: the low zooms ("low"), the landmark points
    /// ("marks"), the area overlays ("overlays").
    #[serde(default)]
    pub publishes: Vec<PublishFc>,
    /// What each machine does next: its first few jobs.
    pub next: BTreeMap<String, Vec<NextFc>>,
    /// Each lane's work to the end, by its name, a step's jobs together (`Stretch`): the schedule.
    #[serde(default)]
    pub lanes: BTreeMap<String, Vec<Stretch>>,
    /// The machines (this Mac's slots) whose job waits on a worker holding its z6 tile's tail, and on
    /// what (the agent's `waiting_on`): their room free for a job beside it meanwhile.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub waiting: BTreeMap<String, String>,
    /// The jobs the scheduler granted in its run's first hour (`GRANTS_S`), when (unix seconds): what
    /// the lead's real grants are held to (`departures`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    /// Its run was cut at its time budget (`BUDGET`): the work it placed by then as placed, no
    /// finish nor range (`why` says so).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// Its run's planned targets done by each `CURVE_S` from `at`, unstretched (`curve`): what a
    /// later forecast's `pace` holds it to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curve: Vec<u32>,
    /// The live error (`Input::pace`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
    /// Every target the expected run placed: its lane (`Input::machines`' index), step, target and
    /// from and until when (unix s). Not in the status (it's large): the forecasts' archive's
    /// (crate::agent::fcarchive).
    #[serde(skip)]
    pub placed: Vec<Placed>,
    /// The schedule's runs (`lanes`), their targets and what each waits on: the build page's
    /// (`/api/tocome`, with `placed`'s tiles: `ToCome::runs`). Not in the status.
    #[serde(skip)]
    pub runs: Runs,
    /// The items' dependencies, what each job started from now on waited on is read from
    /// (crate::coord::history::Waits: the history's starts and leases name it). Not in the status.
    #[serde(skip)]
    pub graph: crate::coord::history::Graph,
}

/// The schedule's runs (`Forecast::lanes`) numbered in turn: the lanes by name, each lane's runs in
/// order (run k is the k-th of that list), each run's jobs (`Stretch::cuts`) numbered in turn after
/// the runs before it's (job j is the j-th of every run's jobs in that order). Each run's targets
/// (its items', in order; a job under way that no item is, none) and how many jobs it is, and the
/// jobs each waits on, from the items' readiness (crate::agent::steps::read_targets): [a, b] when
/// an item of job b waits on one of job a, a set it reads whole (`Items::groups`) by the job of
/// its item ending last (what it waits for till then), each pair once, sorted; a job never on itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runs {
    pub targets: Vec<Vec<String>>,
    pub jobs: Vec<u32>,
    pub deps: Vec<[u32; 2]>,
    /// Each item's job (`Forecast::graph`'s order): for the agent's forecast thread alone, taken
    /// out before the answer's kept.
    pub item_job: Vec<Option<u32>>,
    /// Each job's finished dependencies, the history's event numbers (`Waits::by_job`; empty
    /// until the forecast thread works them out).
    pub after: Vec<Vec<u64>>,
}

/// A target placed by the forecast's run (`Forecast::placed`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placed {
    pub lane: usize,
    pub step: String,
    pub target: String,
    pub from: u64,
    pub until: u64,
}

/// The tiles each Mac will work on, from a forecast's `placed` (task #622): the build page's
/// world map's queue, every tile left to build (task #685; `/api/tocome`, asked once a forecast;
/// never in the status). Each Mac's (its lanes' `Machine::mac`, else name), by step, each tile once (a target that names
/// no z/x/y tile, a region's or the world's, left out) at its first placing's start, in time order:
/// [tile, seconds from `at`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToCome {
    pub at: u64,
    pub macs: BTreeMap<String, BTreeMap<String, Vec<(String, u32)>>>,
    /// The forecast's runs (`Forecast::runs`): the schedule's dependency arrows and each run's
    /// tiles on the map (task #787).
    pub runs: Runs,
}

/// Whether `t` names a tile ("z/x/y", codecs::tile::TileId::parse; not its dash form), as the
/// page's `tileOf`.
fn is_tile(t: &str) -> bool {
    t.contains('/') && codecs::tile::TileId::parse(t).is_some()
}

impl ToCome {
    /// `placed`'s tiles by Mac: `macs[lane]` each lane's Mac.
    pub fn of(placed: &[Placed], macs: &[String], at: u64) -> ToCome {
        let mut first: BTreeMap<(&str, &str, &str), u64> = BTreeMap::new();
        for p in placed {
            let (Some(mac), true) = (macs.get(p.lane), is_tile(&p.target)) else { continue };
            let e = first.entry((mac.as_str(), p.step.as_str(), p.target.as_str())).or_insert(p.from);
            *e = (*e).min(p.from);
        }
        let mut out = ToCome { at, macs: BTreeMap::new(), runs: Runs::default() };
        for ((mac, step, tile), from) in first {
            let off = u32::try_from(from.saturating_sub(at)).unwrap_or(u32::MAX);
            out.macs
                .entry(mac.to_string())
                .or_default()
                .entry(step.to_string())
                .or_default()
                .push((tile.to_string(), off));
        }
        for steps in out.macs.values_mut() {
            for tiles in steps.values_mut() {
                tiles.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            }
        }
        out
    }

    /// `/api/tocome`'s answer: {at, macs: {mac: {step: [[tile, s from at]…]}}, runs: [[target…]…],
    /// jobs: [n…], deps: [[a, b]…], after: [[seq…]…]}, every Mac's; runs and their jobs numbered as
    /// `Runs` says, `deps` job to job, `after` each job's finished dependencies (the history's event
    /// numbers; the coordinator adds the `epoch` they're numbered in).
    pub fn answer(&self) -> serde_json::Value {
        serde_json::json!({
            "at": self.at,
            "macs": self.macs,
            "runs": self.runs.targets,
            "jobs": self.runs.jobs,
            "deps": self.runs.deps,
            "after": self.runs.after,
        })
    }

    /// `f`'s tiles to come and runs (at `f.at`: the forecast's own, task #836), taken out of it:
    /// its `placed` dropped, its jobs' `item_job` handed back for the waits (`Waits::by_job`) with
    /// how many jobs there are; `machines` the lanes it was run on (each lane's Mac, else its name).
    pub fn take(f: &mut Forecast, machines: &[Machine]) -> (ToCome, Vec<Option<u32>>, usize) {
        let macs: Vec<String> = machines.iter().map(|m| m.mac.clone().unwrap_or_else(|| m.name.clone())).collect();
        let mut tc = ToCome::of(&f.placed, &macs, f.at);
        let item_job = std::mem::take(&mut f.runs.item_job);
        tc.runs = std::mem::take(&mut f.runs);
        f.placed = Vec::new();
        let jobs = tc.runs.jobs.iter().map(|&n| n as usize).sum();
        (tc, item_job, jobs)
    }
}

/// The forecast's code version, archived with each forecast (crate::agent::fcarchive): raised
/// whenever a change to the run (its order, its costs, its scheduler) would move its times, so the
/// accuracy work (`scenic forecast-check`) can tell one model's error from another's.
pub const VERSION: u32 = 10;

/// Whether the code before `VERSION` has a way of its own in this app (`Input::code`), so that a
/// backtest's old run (crate::agent::fccheck::backtest) differs from its new. False since code 9,
/// which replaced code 2's paces and caps with each job's needs over its Mac's supply rather than
/// keeping them beside it, and code 10 (task #1505: a lease with no measured time left timed by
/// its discounted progress, else its slowest target's high percentile, where the room held for a
/// waiting job weighs it, crate::coord::sched's `run_end`; the record's `Leased::left_s` and
/// `State::lefts`) kept no way before either: a record rerun as code 9 runs as code 10, and the backtest
/// says so. Set it with the first change that keeps the code before's way behind `Input::code`.
pub const BEFORE_HAS_PATH: bool = false;

/// A stretch of a lane's work of one step (a "run" of the schedule: `Runs`, the page's run
/// numbers): from and until when (unix seconds), and how many targets. A stretch is one or more jobs as the scheduler grants them in the forecast's run, one after
/// another on the lane (two minutes apart at most; task #834): its targets (`Runs::targets`, in
/// time order) its jobs' in turn. `cuts`, each job's after the first: [when it starts, s from
/// `from`; how many of the run's named targets come before it (a job under way that no item is
/// names none)]; a job ends where the next starts (its gap
/// to it, under two minutes, drawn as its own). Left out for a run of one job. Every job to come is
/// so on the page, its targets named, at ~10 bytes a job past the first of a run (29 Oct's record,
/// 5,397 jobs over 2.5 days in 571 runs: the lanes 35 KB without the cuts, 91 KB with them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stretch {
    pub step: String,
    pub from: u64,
    pub until: u64,
    pub n: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<[u32; 2]>,
    /// Its jobs are drops (`Input::drops`): packs dropped, nothing built.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drop: bool,
}

/// A step's work left.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StepFc {
    pub step: String,
    pub left: usize,
    /// Its time at the lead's pace (seconds).
    pub work_s: u64,
    /// Its targets by the figure their time comes from (`Basis`): the page says it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub basis: BTreeMap<Basis, usize>,
    /// Whether members take its jobs (a shared step's).
    pub shared: bool,
    pub done_at: Option<u64>,
}

/// A region's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegionFc {
    pub id: String,
    /// Its place in the build order (0: the one being built).
    pub rank: usize,
    pub on_map: Option<bool>,
    /// Its targets left by step ("unit", "terrain-hi", "slope-hi", "trees", "trees-lo", "pack", …).
    pub left: BTreeMap<String, usize>,
    /// When it'll be done, and when it'll be on the map as it's built now: its last z6 tile's end,
    /// then its map tiles drawn, then a catalogue within the minute (None: no work left of its own).
    pub ready_at: Option<u64>,
    pub map_at: Option<u64>,
}

/// A cadenced publish: what ("low", "marks", "overlays") and when it may next go out (unix
/// seconds): once its cadence has passed since it last went, or once no z6 tile or terrain is left to
/// build, whichever comes first. (The low zooms may go sooner, once half their packs are stale:
/// build::low_gate.)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishFc {
    pub what: String,
    pub at: u64,
}

/// A machine's next job: its step and targets, from when to when (unix seconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NextFc {
    pub step: String,
    pub targets: Vec<String>,
    pub from: u64,
    pub until: u64,
}

/// When a job runs: before the regions, a region's map tiles (once its z6 tiles are built, ahead of
/// the regions' work), a region's (terrain, z6 tiles), late (slope and tree cover: the lead's once a
/// region is on the map, and after the last z6 tile), a chain's (the trains' and the landmarks': from
/// the start, once what it reads is built), or after everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Before,
    Publish,
    Region,
    Late,
    Chain,
    After,
}

/// What the items wait on (`Waits::of`): the input's reads along long ways (the plan's), each step
/// read whole as one set in `groups`, shared by every item reading it (`whole`: by step, the set's
/// length when made and its index; task #867: 1,328 peaks each listing the 568 candidates were
/// 0.75M of the kept waits' 1.07M deps), and what a target read that isn't listed waits on
/// (`held`, by its step and target's hash, `held_key`; those that wait on
/// nothing in `free`: 6 Nov's record had some 1 M slope-deep parts the slope-hi tiles read, 2,300
/// reads waiting in all; and each step's `may_wait`, with the items listed when it was worked out).
struct Waits<'a> {
    the_plan: &'static dyn crate::agent::seam::Plan,
    along: &'a crate::agent::seam::Along,
    whole: BTreeMap<&'static str, (usize, usize)>,
    held: std::collections::HashMap<u64, Deps>,
    free: std::collections::HashSet<u64>,
    may: BTreeMap<&'static str, (usize, bool)>,
}

/// A read target's key in `Waits::held` and `Waits::free`: its step's and target's XXH3.
fn held_key(step: &str, t: &str) -> u64 {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    h.update(step.as_bytes());
    h.update(&[0]);
    h.update(t.as_bytes());
    h.digest()
}

/// The items and sets an item waits on (`Waits::of`): its own deps, and its sets in `Items::groups`.
type Deps = (Vec<usize>, Vec<usize>);

impl<'a> Waits<'a> {
    fn new(the_plan: &'static dyn crate::agent::seam::Plan, inp: &'a Input) -> Self {
        Waits {
            the_plan,
            along: &inp.along,
            whole: BTreeMap::new(),
            held: Default::default(),
            free: Default::default(),
            may: BTreeMap::new(),
        }
    }

    /// The items a target of `step` waits on, of those listed by now (`by`), as the planner's
    /// readiness holds it (crate::agent::steps::readiness): what it reads
    /// (crate::agent::steps::read_targets), a target's own item, a step it reads whole its set in
    /// `groups`; and through a read that isn't listed (built as it stands) but passes waits on
    /// (`read_targets_passing`), what that read waits on, the planner's `held`: its own record is
    /// current, but what it reads is left, so it is made again once that is and its readers wait
    /// (task #988: a depth tile reading the lake-depth packs of a tile whose terrain is left; the
    /// forecast ran it at once, the plan held it). Likewise a step read whole none of whose targets
    /// is listed waits on the steps it reads on (`read_steps`, the planner's `whole_left`).
    fn of(&mut self, by: &Index, groups: &mut Vec<Vec<usize>>, step: &str, t: &str) -> Deps {
        let (mut deps, mut gs) = (Vec::new(), Vec::new());
        for (r, passes) in self.the_plan.read_targets_passing(step, t, Some(self.along)) {
            match r {
                ReadTarget::One(s, u) => match by.get(s, &u) {
                    Some(i) => deps.push(i),
                    None if passes => {
                        let (d, g) = self.held(by, groups, s, u);
                        deps.extend(d);
                        gs.extend(g);
                    }
                    None => {}
                },
                ReadTarget::Whole(s) if passes => self.whole_left(by, groups, s, &mut gs),
                ReadTarget::Whole(s) => gs.extend(self.group(by, groups, s)),
            }
        }
        deps.sort_unstable();
        deps.dedup();
        gs.sort_unstable();
        gs.dedup();
        (deps, gs)
    }

    /// What target `u` of `step`, not listed, waits on (`of`), worked out the first time it's asked
    /// (what it reads is listed ahead of what reads it: a region's terrain before its slope, the
    /// regions' before the chains; some 65,000 slope tiles read z10 tiles' parts, each read by
    /// many); nothing, without a look, for a step none of whose reads may wait (`may_wait`).
    fn held(&mut self, by: &Index, groups: &mut Vec<Vec<usize>>, step: &'static str, u: String) -> Deps {
        if !self.may_wait(by, step) {
            return Deps::default();
        }
        let key = held_key(step, &u);
        if self.free.contains(&key) {
            return Deps::default();
        }
        if let Some(d) = self.held.get(&key) {
            return d.clone();
        }
        let got = self.of(by, groups, step, &u);
        if got.0.is_empty() && got.1.is_empty() {
            self.free.insert(key);
        } else {
            self.held.insert(key, got.clone());
        }
        got
    }

    /// Whether a target of `step` not listed may wait on an item: a step it reads has one listed, or
    /// one it reads through passes waits on and may wait itself (crate::agent::steps::read_steps);
    /// worked out again once more items are listed.
    fn may_wait(&mut self, by: &Index, step: &'static str) -> bool {
        let n = by.len;
        if let Some(&(at, m)) = self.may.get(step) {
            if at == n {
                return m;
            }
        }
        // (No cycle in the declared reads; marked first all the same.)
        self.may.insert(step, (n, false));
        let listed = |s: &str| by.of_step(s).next().is_some();
        let m =
            self.the_plan.read_steps(step).into_iter().any(|(s, passes)| listed(s) || (passes && self.may_wait(by, s)));
        self.may.insert(step, (n, m));
        m
    }

    /// The sets of a step read whole: its own items', else those of the steps it reads on
    /// (`read_steps`, those passing waits on), as far as some are listed.
    fn whole_left(&mut self, by: &Index, groups: &mut Vec<Vec<usize>>, step: &'static str, gs: &mut Vec<usize>) {
        match self.group(by, groups, step) {
            Some(g) => gs.push(g),
            None => {
                for (s, passes) in self.the_plan.read_steps(step) {
                    if passes {
                        self.whole_left(by, groups, s, gs);
                    }
                }
            }
        }
    }

    /// The set of `step`'s items listed by now, made once for each length it has (items are only
    /// added: a set as long is the same); None while none is listed.
    fn group(&mut self, by: &Index, groups: &mut Vec<Vec<usize>>, step: &'static str) -> Option<usize> {
        let n = by.of.get(step).map_or(0, |m| m.len());
        if n == 0 {
            return None;
        }
        Some(match self.whole.get(step) {
            Some(&(at, g)) if at == n => g,
            _ => {
                let mut all: Vec<usize> = by.of_step(step).collect();
                all.sort_unstable();
                groups.push(all);
                self.whole.insert(step, (n, groups.len() - 1));
                groups.len() - 1
            }
        })
    }
}

#[derive(Clone, Debug)]
struct Item {
    step: String,
    target: String,
    cost: Cost,
    phase: Phase,
    /// The items it waits for: its own, and the shared sets in `Items::groups` (each waited for
    /// whole).
    deps: Vec<usize>,
    groups: Vec<usize>,
    /// Whether members take it (a shared step's, not before the regions').
    shared: bool,
    /// Being built now (its machine free when it's done).
    running: bool,
    /// Which machine, from and until when (seconds from now).
    by: Option<usize>,
    from: f64,
    end: f64,
    /// The job it was granted in (`run`'s numbering, from 1; 0: none yet): the lane's runs
    /// (`Stretch`) are its jobs.
    job: u32,
    /// A drop (`Input::drops`).
    drop: bool,
}

/// One run through the work (`run`): each item's machine and times.
struct Sim {
    items: Vec<Item>,
    /// Region → its items.
    region_items: Vec<Vec<usize>>,
    done: bool,
    /// The jobs the scheduler granted in the run's first `GRANTS_S` (`Forecast::grants`).
    grants: Vec<Grant>,
    /// Cut at its time budget before every item was placed (`forecast_by`).
    cut: bool,
    /// The sets items wait on whole (`Items::groups`): what the schedule's runs wait on (`Runs`).
    groups: Vec<Vec<usize>>,
}

/// A job the scheduler granted in the forecast's run (`Forecast::grants`): to which Mac, its step
/// and targets, when (s from the forecast's start; unix seconds in a `Forecast`), and the threads
/// it starts on (the scheduler's grant rule, crate::coord::sched::Sched::lease_cores; 0: an older
/// forecast's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub mac: String,
    pub step: String,
    pub targets: Vec<String>,
    pub at: f64,
    #[serde(default)]
    pub threads: u32,
}

/// The share of a region's z6 tiles reading stale terrain, built as the coverage wants them now, taken
/// to go stale once that terrain is built (`RegionLeft::expected`; which ones is known only then).
/// A round figure, not measured each time: the terrain fix (8 Oct 2026) made most of the north's and
/// every coast's z6 tiles stale, a little over half of those reading changed terrain, and the forecast
/// had counted none of them until their terrain was built (32 z6 tiles at 17:00, 149 by 19:00).
pub const EXPECTED_STALE: f64 = 0.6;

/// The items listed so far (`items`): by step, then target (looked up without making a key, by a
/// quick hash, roadcore::fx: the listing looks up each of the millions of reads its targets
/// declare), and how many.
#[derive(Default)]
struct Index {
    of: roadcore::fx::FxMap<String, roadcore::fx::FxMap<String, usize>>,
    len: usize,
}

impl Index {
    fn get(&self, step: &str, t: &str) -> Option<usize> {
        self.of.get(step).and_then(|m| m.get(t)).copied()
    }

    fn insert(&mut self, step: &str, t: &str, i: usize) {
        if self.of.entry(step.to_string()).or_default().insert(t.to_string(), i).is_none() {
            self.len += 1;
        }
    }

    /// `step`'s items listed.
    fn of_step(&self, step: &str) -> impl Iterator<Item = usize> + '_ {
        self.of.get(step).into_iter().flat_map(|m| m.values().copied())
    }
}

/// The work as items (`items`): each region's, and the sets of items many wait for, each listed
/// once (sorted, no repeats): a region's terrain that its z6 tiles wait for, all the terrain, all
/// before the steps after. (Lead memory, 25 Oct: each slope target had its own copy of its z3
/// tiles' terrain, about 6,750 items, 8.7 GB of the agents' 17 GB. Since #864 a slope target's own
/// list is only what it reads: a slope-deep's 5 at most, a slope-hi's 10 and its 256 z10 tiles'.)
/// Made once a forecast, each of its three runs taking a copy (`forecast_by`).
#[derive(Clone)]
struct Items {
    items: Vec<Item>,
    region_items: Vec<Vec<usize>>,
    groups: Vec<Vec<usize>>,
}

/// The work as items, each with the items it waits for.
fn items(the_plan: &'static dyn crate::agent::seam::Plan, inp: &Input) -> Items {
    let mut out: Vec<Item> = Vec::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut by_target = Index::default();
    let add = |out: &mut Vec<Item>,
               by: &mut Index,
               step: &str,
               target: &str,
               cost: Cost,
               phase: Phase,
               (mut deps, gs): (Vec<usize>, Vec<usize>)|
     -> usize {
        if let Some(i) = by.get(step, target) {
            return i;
        }
        let shared = the_plan.shared().contains(&step) && phase != Phase::Before;
        deps.shrink_to_fit();
        out.push(Item {
            step: step.into(),
            target: target.into(),
            cost,
            phase,
            deps,
            groups: gs,
            shared,
            running: false,
            by: None,
            from: 0.0,
            end: 0.0,
            job: 0,
            drop: false,
        });
        let i = out.len() - 1;
        by.insert(step, target, i);
        i
    };
    for (step, target, cost) in &inp.before {
        add(&mut out, &mut by_target, step, target, *cost, Phase::Before, (Vec::new(), Vec::new()));
    }
    // A region at a time: its own terrain (the z6 tiles, then each z3 tile's assembly once its z6 tiles
    // are made), then its own z6 tiles (each once its region's terrain is built: the terrain it
    // reads); then every region's slope and tree cover, each target once the targets it reads are
    // (its row's declaration, as the planner's waits: `Waits::of`).
    let mut region_items: Vec<Vec<usize>> = vec![Vec::new(); inp.regions.len()];
    let item_of = |by: &Index, step: &str, t: &str| by.get(step, t);
    let mut waits = Waits::new(the_plan, inp);
    let mut reads = |by: &Index, groups: &mut Vec<Vec<usize>>, step: &str, t: &str| waits.of(by, groups, step, t);
    // (A set many wait for, listed once: its index in `groups`.)
    let group = |groups: &mut Vec<Vec<usize>>, mut v: Vec<usize>| -> usize {
        v.sort_unstable();
        v.dedup();
        v.shrink_to_fit();
        groups.push(v);
        groups.len() - 1
    };
    // (The drops first, waiting on nothing: a region's terrain listing them waits on them.)
    for (step, target, cost) in &inp.drops {
        let i = add(&mut out, &mut by_target, step, target, *cost, Phase::Region, (Vec::new(), Vec::new()));
        out[i].drop = true;
    }
    for r in inp.regions.iter() {
        for t in &r.own_terrain {
            add(
                &mut out,
                &mut by_target,
                "terrain-hi",
                t,
                inp.cost("terrain-hi", t),
                Phase::Region,
                (Vec::new(), Vec::new()),
            );
        }
        // (The deep levels wait for nothing: their base is made from the pool.)
        for t in &r.own_terrain_deep {
            let deps = reads(&by_target, &mut groups, "terrain-deep", t);
            add(&mut out, &mut by_target, "terrain-deep", t, inp.cost("terrain-deep", t), Phase::Region, deps);
        }
        for q in &r.own_terrain_lo {
            let deps = reads(&by_target, &mut groups, "terrain-lo", q);
            add(&mut out, &mut by_target, "terrain-lo", q, inp.cost("terrain-lo", q), Phase::Region, deps);
        }
        for u in r
            .own_units
            .iter()
            .chain(r.expected.iter().take((r.expected.len() as f64 * EXPECTED_STALE).round() as usize))
        {
            // (The expected: those its stale terrain may make stale, a share, the first in its
            // order; `add` lists a z6 tile two regions name once.)
            let deps = reads(&by_target, &mut groups, "unit", u);
            add(&mut out, &mut by_target, "unit", u, inp.cost("unit", u), Phase::Region, deps);
        }
        // Its map tiles, each once the z6 tiles that draw it are built (a tile two regions draw, with
        // the first).
        for (t, us) in &r.packs {
            let mut built: Vec<usize> = us.iter().filter_map(|u| item_of(&by_target, "unit", u)).collect();
            built.sort_unstable();
            built.dedup();
            add(&mut out, &mut by_target, "pack", t, inp.cost("pack", t), Phase::Publish, (built, Vec::new()));
        }
    }
    // What works out an input the plan's targets wait for (`Input::unknown`: the terrain's root),
    // once what it reads is built (the regions' terrain, listed by now): ahead of the slope and the
    // regions' work, as the plan lists it (build::plan: "the terrain's root first of them").
    let until: Vec<usize> = inp
        .unknown
        .iter()
        .map(|u| {
            let (step, t, cost) = &u.until;
            let deps = reads(&by_target, &mut groups, step, t);
            add(&mut out, &mut by_target, step, t, *cost, Phase::Before, deps)
        })
        .collect();
    // The slope: each target once what it reads is built, its row's declaration
    // (crate::agent::steps::Read), the planner's waits.
    for r in inp.regions.iter() {
        for t in &r.slope_deep {
            let deps = reads(&by_target, &mut groups, "slope-deep", t);
            add(&mut out, &mut by_target, "slope-deep", t, inp.cost("slope-deep", t), Phase::Late, deps);
        }
        for t in &r.slope {
            let deps = reads(&by_target, &mut groups, "slope-hi", t);
            add(&mut out, &mut by_target, "slope-hi", t, inp.cost("slope-hi", t), Phase::Late, deps);
        }
        for q in &r.slope_lo {
            let deps = reads(&by_target, &mut groups, "slope-lo", q);
            add(&mut out, &mut by_target, "slope-lo", q, inp.cost("slope-lo", q), Phase::Late, deps);
        }
        for t in &r.trees {
            add(&mut out, &mut by_target, "trees", t, inp.cost("trees", t), Phase::Late, (Vec::new(), Vec::new()));
        }
        // A z3 tile's tree cover assembled once its z6 tiles are made (a region lists every z6 tile of
        // its z3 tiles: listed by now).
        for q in &r.trees_lo {
            let deps = reads(&by_target, &mut groups, "trees-lo", q);
            add(&mut out, &mut by_target, "trees-lo", q, inp.cost("trees-lo", q), Phase::Late, deps);
        }
    }
    // Each region's items: its terrain, all its z6 tiles (some come with a region before it), its
    // slope and tree cover.
    for (k, r) in inp.regions.iter().enumerate() {
        let mut mine: Vec<usize> = Vec::new();
        for (step, list) in [
            ("terrain-hi", &r.terrain),
            ("terrain-deep", &r.terrain_deep),
            ("terrain-lo", &r.terrain_lo),
            ("unit", &r.units),
            ("unit", &r.expected),
            ("slope-deep", &r.slope_deep),
            ("slope-hi", &r.slope),
            ("slope-lo", &r.slope_lo),
            ("trees", &r.trees),
            ("trees-lo", &r.trees_lo),
        ] {
            mine.extend(list.iter().filter_map(|t| item_of(&by_target, step, t)));
        }
        mine.extend(r.packs.iter().filter_map(|(t, _)| item_of(&by_target, "pack", t)));
        region_items[k] = mine;
    }
    // The chains: from the start, each target once what it reads is built (its declared reads, as
    // the planner's: listed by now, the jobs before the regions', the regions' and the chains'
    // before it).
    for (step, target, cost) in &inp.chains {
        let deps = reads(&by_target, &mut groups, step, target);
        add(&mut out, &mut by_target, step, target, *cost, Phase::Chain, deps);
    }

    // (Each after all before it: the catalogue after the overlays.)
    let all = vec![group(&mut groups, (0..out.len()).collect())];
    let mut after: Vec<usize> = Vec::new();
    for (step, target, cost) in &inp.after {
        let i = add(&mut out, &mut by_target, step, target, *cost, Phase::After, (after.clone(), all.clone()));
        after.push(i);
    }
    // (After all that, not each other.)
    for (step, target, cost) in &inp.idle {
        add(&mut out, &mut by_target, step, target, *cost, Phase::After, (after.clone(), all.clone()));
    }
    // (The targets an input of which isn't known yet: each waits for what works it out too.)
    for (u, &w) in inp.unknown.iter().zip(&until) {
        for t in &u.targets {
            if let Some(i) = item_of(&by_target, &u.step, t).filter(|&i| i != w) {
                out[i].deps.push(w);
                out[i].deps.sort_unstable();
                out[i].deps.dedup();
            }
        }
    }
    Items { items: out, region_items, groups }
}

/// The items ready to take (their deps done, not yet taken), each phase's by step in order (an
/// ask's offers: `offers`).
/// `rev` counts its changes: the offers made of it kept while it stands (`run_on`).
#[derive(Default)]
struct Ready {
    phase: [BTreeMap<String, BTreeSet<usize>>; 6],
    rev: u64,
}

impl Ready {
    fn insert(&mut self, i: usize, it: &Item, p: usize) {
        self.rev += 1;
        self.phase[p].entry(it.step.clone()).or_default().insert(i);
    }

    fn remove(&mut self, i: usize, it: &Item, p: usize) {
        self.rev += 1;
        let drop = |m: &mut BTreeMap<String, BTreeSet<usize>>| {
            if let Some(s) = m.get_mut(&it.step) {
                s.remove(&i);
                if s.is_empty() {
                    m.remove(&it.step);
                }
            }
        };
        drop(&mut self.phase[p]);
    }

    /// Phase `p`'s items ready, a step's together, the steps in the order their first comes.
    fn steps(&self, p: usize) -> Vec<Vec<usize>> {
        let mut v: Vec<Vec<usize>> =
            self.phase[p].values().map(|s| s.iter().copied().take(OFFER_MAX).collect()).collect();
        v.sort_by_key(|s| s[0]);
        v
    }
}

/// The most targets of a step one ask is offered in the run (`offers`), the first ready; the live
/// plan offers all of them. Two gaps (docs/pool.md §7.7, Batch length): where more than this many
/// fit the batch length (`Sched::batch_len`: slope-deep's empty targets, a second or so each), the
/// coordinator grants up to `JOB_MAX` in one job, the forecast this many; and placement's start
/// (`Sched::placed_start`, the target measured to take longest) is drawn among these alone. Its
/// bound is the run's CPU: offered `JOB_MAX`, the live-scale test's three runs took 3.4 times the
/// CPU (463 s against 135, past its bound), `placed_start`'s times of each target kept or not
/// (task #951: the cost is in the work each ask does per target offered, not those times alone).
const OFFER_MAX: usize = 64;

/// How long a lane refused work waits before it asks again (s), when nothing ends sooner: an
/// agent asks every few seconds, and a wait on the NAS budget ages (jobrt::budget::nas::OWN_AGE).
const ASK_AGAIN_S: f64 = 300.0;

/// How long the run's leases last without a beat: past any run (the scheduler's leases lapse only
/// when a worker goes quiet, which the forecast doesn't foresee).
const RUN_TTL: Duration = Duration::from_secs(400 * 86_400);

/// The scheduler the run asks (crate::coord::sched): its state (the lead's, `Input::sched`; with
/// none, one of the work's measures alone), each Mac's ask, and the clock the run's seconds are on.
struct Pool {
    sched: Sched,
    asks: BTreeMap<String, Ask>,
    t0: Instant,
    now: u64,
    /// The steps with a measure of the way they run now (a cost at their `cost_version`): a job of
    /// another leaves its targets' measures as it ends (`learn`).
    current: BTreeSet<String>,
}

impl Pool {
    /// The scheduler for `inp`'s run over `items`: the lead's state as recorded (`Input::sched`), or,
    /// with none, the measured items' costs and nothing out. Each Mac asks as its last ask said (one
    /// job a lane: `Ask::want` 1), its room as with none of its jobs reserved (the scheduler counts
    /// the leases out), no load from outside the pool; a Mac whose ask isn't recorded with its room
    /// (`Input::macs`), else a lane's memory and cores past any job's.
    fn new(the_plan: &'static dyn crate::agent::seam::Plan, inp: &Input, items: &[Item]) -> Pool {
        let t0 = Instant::now();
        let clock = Clock { now: t0, unix: inp.now };
        let mut pool = Pool {
            sched: Sched::from_state(the_plan, inp.sched.as_ref().unwrap_or(&State::default()), clock, RUN_TTL),
            asks: BTreeMap::new(),
            t0,
            now: inp.now,
            current: BTreeSet::new(),
        };
        let v = |k: &str| the_plan.cost_version(crate::agent::memguard::step_of_key(k));
        pool.current = pool
            .sched
            .costs
            .iter()
            .filter(|(k, c)| c.v >= v(k))
            .map(|(k, _)| crate::agent::memguard::step_of_key(k).to_string())
            .collect();
        if inp.sched.is_none() {
            for it in items.iter().filter(|it| it.cost.known()) {
                pool.learn(it);
            }
        }
        let sched = &mut pool.sched;
        if sched.lead_name.is_empty() {
            sched.lead_name = inp.machines.first().map(mac_of).unwrap_or_default();
        }
        for m in &inp.machines {
            let g = mac_of(m);
            if pool.asks.contains_key(&g) {
                continue;
            }
            let recorded = inp.sched.as_ref().and_then(|s| s.workers.iter().find(|w| w.ask.worker == g));
            let mut a = match (recorded, inp.macs.get(&g)) {
                (Some(w), _) => w.ask.clone(),
                (None, Some(r)) => Ask {
                    mem_mb: if r.mem_mb > 0 { r.mem_mb } else { m.mem_mb },
                    cores: r.cores,
                    cap: Some(Capacity { cores: r.cores, cores_free: r.cores, ..Default::default() }),
                    ..Default::default()
                },
                (None, None) => Ask {
                    mem_mb: m.mem_mb,
                    cores: LANE_CORES,
                    cap: Some(Capacity { cores: LANE_CORES, cores_free: LANE_CORES, ..Default::default() }),
                    ..Default::default()
                },
            };
            a.worker = g.clone();
            a.kind = "native".into();
            a.want = 1;
            // (Its cores less those busy outside the pool as its last ask said, as the coordinator
            // counts them (task #684): the run's jobs are packed into what's left, as granted.)
            let cores = inp.sched.as_ref().map(|s| s.cores_left(the_plan, &a));
            if let Some(c) = a.cap.as_mut() {
                a.disk_mb = a.disk_mb.map(|d| d + c.reserved_mb);
                c.cores = cores.unwrap_or(c.cores);
                (c.held_mb, c.reserved_mb, c.cores_free) = (0, 0, c.cores);
                c.leases.clear();
                // (Its sustained best room from now on (`sustained`, `Capacity::best`): its memory
                // for jobs and its cores less those busy outside the pool then, no brake, no
                // owner's use. None said (an older app's, or not 30 minutes measured yet): its room
                // as it asked.)
                if let Some(b) = c.best {
                    a.mem_mb = b.mem_mb;
                    (c.mem_mb, c.build_mb, c.cores, c.cores_free) = (b.mem_mb, b.mem_mb, b.cores, b.cores);
                    (c.in_use, c.kept_cores, c.mem_held_back) = (false, 0, None);
                }
            }
            pool.asks.insert(g, a);
        }
        pool
    }

    /// Item `it`'s measure taken (its job ended, or it was measured before the run): its memory
    /// and time, as a hand-off's costs, where none of the way its step runs now is kept already
    /// (one of an older `cost_version` is replaced, as a hand-off's is).
    fn learn(&mut self, it: &Item) {
        if crate::needs::own(self.sched.the_plan, &self.sched.costs, &it.step, &it.target).is_some() {
            return;
        }
        let v = self.sched.the_plan.cost_version(&it.step);
        self.sched.costs.insert(
            cost_key(&it.step, &it.target),
            crate::coord::Cost { peak_mb: it.cost.peak_mb, secs: it.cost.secs.round() as u64, v, ..Default::default() },
        );
        self.sched.needs_rev += 1;
        self.current.insert(it.step.clone());
    }

    fn clock(&self, t: f64) -> Clock {
        Clock { now: self.t0 + Duration::from_secs_f64(t.max(0.0)), unix: self.now + t.max(0.0) as u64 }
    }
}

/// The cores a Mac is taken to have whose ask isn't recorded nor its room said: past any job's.
const LANE_CORES: u32 = 64;

/// The Mac a lane's slot is on (`Machine::mac`; none: its own).
fn mac_of(m: &Machine) -> String {
    m.mac.clone().unwrap_or_else(|| m.name.clone())
}

/// `run_on` on `inp`'s items (the tests': a forecast lists them once for its three runs).
#[cfg(test)]
fn run(
    the_plan: &'static dyn crate::agent::seam::Plan,
    inp: &Input,
    scale: &dyn Fn(&Cost) -> f64,
    budget: Duration,
) -> Sim {
    run_on(the_plan, inp, items(the_plan, inp), scale, budget)
}

/// One run through the work (`its`, `items`), each item's time `scale`d. As a lane frees, its Mac asks the
/// coordinator's scheduler (crate::coord::sched::Sched::pack, the live coordinator's own decision)
/// for a job each of its lanes free then, the work ready offered in the plan's order: pulled, as a
/// live agent asks when a slot frees, never pushed to every Mac at once. Each job granted runs its
/// targets one after another on a lane of its own at the lane's pace, its lease out until it ends.
/// A lane given none asks again once a job ends (or `ASK_AGAIN_S`).
/// Cut once its thread has computed `budget`, or after `BACKSTOP` times it on the wall clock
/// (`Sim::cut`, `thread_cpu`).
fn run_on(
    the_plan: &'static dyn crate::agent::seam::Plan,
    inp: &Input,
    its: Items,
    scale: &dyn Fn(&Cost) -> f64,
    budget: Duration,
) -> Sim {
    let (cpu0, wall0) = (thread_cpu(), Instant::now());
    let Items { mut items, region_items, groups } = its;
    for it in &mut items {
        it.cost.secs = scale(&it.cost).max(1.0);
    }
    // (A time its needs give, `needs_secs`, is measured: the runs' spread of a known time. With no
    // scheduler's state (a test's input, no coordinator) there are no needs to read: its cost.)
    let known = scale(&Cost { secs: 1.0, basis: Basis::Own, peak_mb: 0 });
    let needs = inp.sched.is_some();
    let mut pool = Pool::new(the_plan, inp, &items);
    let n = inp.machines.len();
    let mut free: Vec<f64> = inp.machines.iter().map(|m| m.busy_s.max(m.from_s).max(0.0)).collect();
    // What's being built now: done when its machine is free.
    let at: BTreeMap<(String, String), usize> =
        items.iter().enumerate().map(|(i, it)| ((it.step.clone(), it.target.clone()), i)).collect();
    let running: Vec<(usize, usize)> = inp.running.iter().filter_map(|(k, &m)| at.get(k).map(|&i| (i, m))).collect();
    // (The jobs granted, numbered from 1: a lane's targets under way its one job.)
    let mut jobs = n as u32;
    for &(i, m) in &running {
        (items[i].by, items[i].from, items[i].end, items[i].running, items[i].job) =
            (Some(m), 0.0, free.get(m).copied().unwrap_or(0.0), true, m as u32 + 1);
    }
    // The leases out at the start: each ends with the lane running its targets, else with its Mac's
    // first lane busy with a job no item is (its step's), else now.
    let mut out: Vec<(f64, u64, Vec<usize>)> = pool
        .sched
        .leases
        .all(pool.t0)
        .into_iter()
        .filter_map(|l| match &l.work {
            crate::coord::lease::Work::Job { step, targets } => {
                let lane =
                    targets.iter().find_map(|t| inp.running.get(&(step.clone(), t.0.clone())).copied()).or_else(|| {
                        (0..n).find(|&k| {
                            mac_of(&inp.machines[k]) == l.worker
                                && inp.machines[k].running.as_ref().is_some_and(|r| r.0 == *step)
                        })
                    });
                Some((lane.map_or(0.0, |k| inp.machines[k].busy_s.max(0.0)), l.id, Vec::new()))
            }
            _ => None,
        })
        .collect();
    // (The pacing's window as recorded (`Limits::window`) is a reading of now: a window cut under
    // the whole holds while the jobs out now run, the whole after them, as no passing load is
    // foreseen. Code 7: before it, a held gate shut for the whole run.)
    let mut gate_on: Vec<u64> = if inp.code >= 7 { out.iter().map(|o| o.1).collect() } else { Vec::new() };
    let mut inflight: Vec<usize> = running.iter().map(|r| r.0).collect();
    let mut unassigned = items.iter().filter(|i| i.by.is_none()).count();
    // Each phase's items in order, and the first not yet taken.
    const PHASES: [Phase; 6] = [Phase::Before, Phase::Publish, Phase::Region, Phase::Late, Phase::Chain, Phase::After];
    let pi = |p: Phase| PHASES.iter().position(|&q| q == p).unwrap_or(0);
    // Each item's deps not yet done, the items waiting on each, and each phase's items ready (their
    // deps done, not yet taken), in order.
    // (A group, `Items::groups`, counts as one dep, done once its items left are: the groups each
    // item is in, each group's items not yet done and the items waiting on it.)
    let done_now = |d: usize| items[d].by.is_some() && items[d].end <= 1e-9;
    let mut waiting_on: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    let mut pending: Vec<usize> = vec![0; items.len()];
    let mut in_groups: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    let mut group_left: Vec<usize> = vec![0; groups.len()];
    let mut group_waiting: Vec<Vec<usize>> = vec![Vec::new(); groups.len()];
    for (g, members) in groups.iter().enumerate() {
        for &d in members.iter().filter(|&&d| !done_now(d)) {
            group_left[g] += 1;
            in_groups[d].push(g);
        }
    }
    for (i, it) in items.iter().enumerate() {
        for &d in it.deps.iter().filter(|&&d| !done_now(d)) {
            pending[i] += 1;
            waiting_on[d].push(i);
        }
        for &g in it.groups.iter().filter(|&&g| group_left[g] > 0) {
            pending[i] += 1;
            group_waiting[g].push(i);
        }
    }
    // The regions' terrain, z6 tiles and map tiles: how many not yet taken, and when the last taken
    // ends; each item's regions.
    let mut rp_left = vec![0usize; region_items.len()];
    let mut rp_end = vec![0.0f64; region_items.len()];
    let mut regions_of: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    for (k, mine) in region_items.iter().enumerate() {
        for &i in mine {
            regions_of[i].push(k);
            match items[i].phase {
                Phase::Region | Phase::Publish if items[i].by.is_none() => rp_left[k] += 1,
                Phase::Region | Phase::Publish => rp_end[k] = rp_end[k].max(items[i].end),
                _ => {}
            }
        }
    }
    // The items ready, and each region's slope and tree cover ready (what goes ahead of the regions'
    // work once its region waits for the map alone), kept as they come and go.
    let mut ready = Ready::default();
    let mut late_ready: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); region_items.len()];
    let put_ready = |ready: &mut Ready, late_ready: &mut Vec<BTreeSet<usize>>, i: usize, it: &Item| {
        ready.insert(i, it, pi(it.phase));
        if it.phase == Phase::Late {
            for &k in &regions_of[i] {
                late_ready[k].insert(i);
            }
        }
    };
    for (i, it) in items.iter().enumerate() {
        if it.by.is_none() && pending[i] == 0 {
            put_ready(&mut ready, &mut late_ready, i, it);
        }
    }
    let mut region_left = items.iter().filter(|i| i.phase == Phase::Region && i.by.is_none()).count();
    let mut region_end =
        items.iter().filter(|i| i.phase == Phase::Region && i.by.is_some()).map(|i| i.end).fold(0.0, f64::max);
    let mut grants: Vec<Grant> = Vec::new();
    // (The state's changes counted (a job granted or ended), and each Mac's ask refused (by its
    // Mac and whether it may run the lead's own steps) at which count and time: asked again
    // unchanged, at the same time, the scheduler answers the same, task #544.)
    let mut changes = 0u64;
    let mut pruned_at = f64::NEG_INFINITY;
    let mut refused: BTreeMap<(String, bool), (u64, f64)> = BTreeMap::new();
    // (Each Mac's offers as last made, of the work ready at which change and the regions' slope
    // and tree cover offered ahead.)
    let mut offered: BTreeMap<String, (u64, Vec<usize>, Vec<Offer>)> = BTreeMap::new();
    let mut guard = 0usize;
    let mut cut = false;
    loop {
        guard += 1;
        if guard > 40 * (items.len() + 10) * n.max(1) || unassigned == 0 {
            break;
        }
        if guard.is_multiple_of(32)
            && (thread_cpu().saturating_sub(cpu0) >= budget || wall0.elapsed() >= budget * BACKSTOP)
        {
            cut = true;
            break;
        }
        // The lane free first (the lead's on a tie), and its Mac.
        let (m, t) = free
            .iter()
            .copied()
            .enumerate()
            .fold((0, f64::INFINITY), |a, (i, f)| if f < a.1 - 1e-9 { (i, f) } else { a });
        if !t.is_finite() {
            break;
        }
        // (The NAS budget's waits passed over no more lately let go, as the live coordinator lets
        // them go each status (crate::coord's `nas_waits`): the waits it weighs stay few.)
        if t > pruned_at + 1e-9 {
            pruned_at = t;
            let now = pool.clock(t).now;
            let mut g = pool.sched.nas_waits.lock().unwrap_or_else(|e| e.into_inner());
            g.retain(|_, (_, last, _)| now.saturating_duration_since(*last) < Duration::from_secs(120));
        }
        // (Those ended by now: what waits on them counted down, ready once nothing else is left;
        // the jobs' leases ended.)
        let mut ended: Vec<usize> = Vec::new();
        inflight.retain(|&i| {
            let on = items[i].end > t + 1e-9;
            if !on {
                ended.push(i);
            }
            on
        });
        changes += ended.len() as u64;
        for d in ended {
            let mut freed: Vec<usize> = waiting_on[d].clone();
            for &g in &in_groups[d] {
                group_left[g] -= 1;
                if group_left[g] == 0 {
                    freed.extend(&group_waiting[g]);
                }
            }
            for w in freed {
                pending[w] -= 1;
                if pending[w] == 0 && items[w].by.is_none() {
                    put_ready(&mut ready, &mut late_ready, w, &items[w]);
                }
            }
        }
        // (A job of a step with no measure of the way it runs now (never measured, or only under
        // an older `cost_version`: each target a cold start, one at a time in the pool) leaves its
        // targets' measures, as a real one's hand-off does: the step's next jobs go beside others
        // by them. Its later jobs' measures change little, and would make the measures' models
        // again each time.)
        out.retain(|(end, id, its)| {
            let on = *end > t + 1e-9;
            if !on {
                changes += 1;
                pool.sched.end(*id);
                let cold: Vec<usize> =
                    its.iter().copied().filter(|&i| !pool.current.contains(&items[i].step)).collect();
                for i in cold {
                    pool.learn(&items[i]);
                }
            }
            on
        });
        if inp.code >= 7 {
            gate_on.retain(|id| out.iter().any(|o| o.1 == *id));
            if gate_on.is_empty() {
                for l in [&mut pool.sched.nas, &mut pool.sched.nas_own].into_iter().flatten() {
                    l.window = l.window.max(jobrt::budget::nas::WHOLE);
                }
            }
        }
        // Its Mac asks once, for a job each of its lanes free now, as a live agent asks for its free
        // slots when one frees (crate::agent's asks: `Ask::want`); granted fewer, the rest wait for
        // the next change.
        let g = mac_of(&inp.machines[m]);
        let lanes: Vec<usize> = (0..n).filter(|&k| free[k] <= t + 1e-9 && mac_of(&inp.machines[k]) == g).collect();
        let lead = lanes.iter().any(|&k| k == 0 || inp.machines[k].lead_steps);
        let key = (g.clone(), lead);
        let asked = refused.get(&key) == Some(&(changes, t));
        // The work ready, in the plan's order (build::plan, as the coordinator offers it): the
        // pass's, the map tiles of the z6 tiles built (publishing before the regions' work), the slope
        // and tree cover of a region whose z6 tiles and map tiles are done but isn't on the map yet
        // (the plan's `slope_waits`), the regions' (in order), the slope and tree cover, the chains',
        // then what comes after.
        let picks = if asked {
            Vec::new()
        } else {
            let prep: Vec<usize> = if region_left > 0 || region_end > t + 1e-9 {
                (0..inp.regions.len())
                    .filter(|&k| inp.regions[k].on_map != Some(true) && rp_left[k] == 0 && rp_end[k] <= t + 1e-9)
                    .flat_map(|k| late_ready[k].iter().copied())
                    .take(OFFER_MAX)
                    .collect()
            } else {
                Vec::new()
            };
            let ask = Ask { want: lanes.len(), ..pool.asks[&g].clone() };
            // (Its Mac's offers made again only once the work ready has changed: most asks are
            // refused, and come again at each item's end.)
            let kept = offered.get(&g).is_some_and(|(rev, p, _)| *rev == ready.rev && *p == prep);
            if !kept {
                let mut order = ready.steps(pi(Phase::Before));
                order.extend(ready.steps(pi(Phase::Publish)));
                order.extend(prep.iter().map(|&i| vec![i]));
                for p in [Phase::Region, Phase::Late, Phase::Chain, Phase::After] {
                    order.extend(ready.steps(pi(p)));
                }
                offered.insert(g.clone(), (ready.rev, prep, offers(the_plan, &items, &order, &ask)));
            }
            let offers = &offered[&g].2;
            // (Every Mac with a lane free asking, as the live ones do each loop: placement draws
            // among those asking lately, which aged out as the run's seconds passed.)
            let idle = |g: &str| {
                let on = |k: usize| mac_of(&inp.machines[k]) == g;
                (0..free.len()).filter(|&k| on(k) && free[k] <= t + 1e-9).count()
            };
            let room = |g: &str| pool.asks[g].cap.as_ref().map_or(0, |c| c.mem_mb);
            let who: Vec<(&str, usize, u64)> = pool.asks.keys().map(|g| (g.as_str(), idle(g), room(g))).collect();
            pool.sched.asking(who, pool.clock(t).now);
            pool.sched.pack(&ask, offers, lead, pool.clock(t)).0
        };
        if picks.is_empty() {
            refused.insert(key, (changes, t));
        } else {
            changes += 1;
        }
        // Each job granted on a lane of its own, its targets one after another (in plan order, as
        // crate::coord picks) at the lane's pace; the counts kept.
        let mut idle: Vec<usize> = Vec::new();
        for (j, &k) in lanes.iter().enumerate() {
            let Some(p) = picks.get(j) else {
                idle.push(k);
                continue;
            };
            jobs += 1;
            let mac = &inp.machines[k];
            let ids: Vec<usize> =
                p.targets.iter().filter_map(|x| at.get(&(p.step.clone(), x.0.clone())).copied()).collect();
            let mut start = t;
            for (n_i, &i) in ids.iter().enumerate() {
                let extra = if items[i].step == "unit" { mac.unit_extra_s } else { 0.0 };
                // (Its needs over its Mac's supply now (`needs_secs`), its job's start on the first;
                // what is left of a stopped run, or a step not measured the way it runs now, at its
                // cost at the lane's pace.)
                let step = items[i].step.as_str();
                let at_needs = (needs && items[i].cost.basis != Basis::Partial)
                    .then(|| needs_secs(&pool.sched, step, &items[i].target, &g, p.threads))
                    .flatten()
                    .map(|s| {
                        let first = if n_i == 0 { pool.sched.paces.start(&g, step) } else { 0.0 };
                        (s + first) * known * inp.needs_pace.get(step).copied().unwrap_or(1.0)
                    });
                let end = start + at_needs.unwrap_or_else(|| inp.lane_secs(mac, items[i].cost.secs)).max(1.0) + extra;
                (items[i].by, items[i].from, items[i].end, items[i].job) = (Some(k), start, end, jobs);
                start = end;
                inflight.push(i);
                unassigned -= 1;
                if items[i].phase == Phase::Region {
                    region_left -= 1;
                    region_end = region_end.max(end);
                }
                if matches!(items[i].phase, Phase::Region | Phase::Publish) {
                    for &r in &regions_of[i] {
                        rp_left[r] -= 1;
                        rp_end[r] = rp_end[r].max(end);
                    }
                }
                ready.remove(i, &items[i], pi(items[i].phase));
                for &r in &regions_of[i] {
                    late_ready[r].remove(&i);
                }
            }
            out.push((start, p.lease, ids));
            if t < GRANTS_S {
                grants.push(Grant {
                    mac: g.clone(),
                    step: p.step.clone(),
                    targets: p.targets.iter().map(|x| x.0.clone()).collect(),
                    at: t,
                    threads: p.threads,
                });
            }
            free[k] = start;
        }
        // (A lane given nothing asks again once an item or a lane's job ends, or a lease out, or
        // in a while; with nothing in flight anywhere, nothing it waits on will change.)
        if !idle.is_empty() {
            let next = inflight
                .iter()
                .map(|&i| items[i].end)
                .chain(free.iter().copied().filter(|&f| f > t + 1e-9))
                .chain(out.iter().map(|o| o.0))
                .fold(f64::INFINITY, f64::min);
            let at = if next.is_finite() { next.min(t + ASK_AGAIN_S) } else { next };
            for k in idle {
                free[k] = at;
            }
        }
    }
    let done = items.iter().all(|i| i.by.is_some());
    Sim { items, region_items, done, grants, cut, groups }
}

/// How far into the run its grants are kept (`Forecast::grants`, s): what the lead's real grants are
/// held to (`departures`).
pub const GRANTS_S: f64 = 3600.0;

/// The work ready, a step's together in `order`, as the coordinator offers it to `ask`'s Mac
/// (crate::agent's `offer_plan`): a step offered to every member one offer, its targets in order
/// (`OFFER_MAX` at most), where its work first comes, each once; the lead's own steps' each a
/// planned job of its own (`Offer::job`); only the steps the Mac does (`Ask::can`, all when it says
/// none).
fn offers(the_plan: &dyn crate::agent::seam::Plan, items: &[Item], order: &[Vec<usize>], ask: &Ask) -> Vec<Offer> {
    let mut out: Vec<Offer> = Vec::new();
    let mut of_step: BTreeMap<&str, usize> = BTreeMap::new();
    // (Whether the last step seen is offered to every member: a group is one step's.)
    let mut last: Option<(&str, bool)> = None;
    // (An item in `order` twice, a region's slope ahead of the regions' work and again with the
    // rest of its step, offered once.)
    let mut seen = vec![false; items.len()];
    for group in order {
        for &i in group {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            let it = &items[i];
            if !ask.can.is_empty() && !ask.can.contains(&it.step) {
                break;
            }
            // (Its size as offered: a z6 tile's or candidates' extract bytes, sizing their first
            // guess at the memory its cost says; another step's its memory; none for a step offered
            // by an input measure (crate::needs::OFFERED_BY), which its item doesn't carry: its
            // step's measures size it.)
            let size = match it.step.as_str() {
                "unit" | "pois" => (it.cost.peak_mb << 20) / 10,
                s if crate::needs::offered_by(s).is_some() => 0,
                _ => it.cost.peak_mb,
            };
            let target = (it.target.clone(), KEY.to_string(), size);
            let offered = match last {
                Some((st, o)) if st == it.step => o,
                _ => {
                    let o = super::every::offered(the_plan, &it.step);
                    last = Some((&it.step, o));
                    o
                }
            };
            if !offered {
                out.push(Offer {
                    step: it.step.clone(),
                    job: Some(format!("{} {}", it.step, it.target)),
                    targets: vec![target],
                });
                continue;
            }
            match of_step.get(it.step.as_str()) {
                Some(&k) if out[k].targets.len() < OFFER_MAX => out[k].targets.push(target),
                Some(_) => break,
                None => {
                    of_step.insert(&it.step, out.len());
                    out.push(Offer { step: it.step.clone(), targets: vec![target], job: None });
                }
            }
        }
    }
    out
}

/// The key the run's targets are offered under (no work of the run is done under it yet).
const KEY: &str = "forecast";

/// How often a forecast is made again at most: the lead's, for its status and its build page
/// (crate::agent::plan's `refresh_forecast`): the page's forecast is up to this old.
pub const EVERY: Duration = Duration::from_secs(60);

/// How long each of a forecast's three runs may compute (its own thread's CPU time, not wall
/// time): past it, the run stops where it is (`Sim::cut`) and the forecast is partial
/// (`Forecast::partial`). It runs off the agent's loop (crate::agent's `start_forecast`); this bounds
/// a run at a scale it was never measured at (task #544). A quarter of `EVERY`: the three runs side
/// by side, a forecast takes a quarter of the cadence's CPU at most on each of three cores, on every
/// Mac (the pages' views run it too). CPU time, task #584: at the utility QoS a loaded Mac starves
/// the runs (the mini at load 34: 4 s of CPU took 15 s of wall), and a wall-time cut then hid the
/// finish of an input no bigger than usual. A run at live scale (7 Nov's queue with every region on,
/// 19,000-20,300 items on 30 lanes) computes 5-12 s on the M4 under load (task #995; docs/pool.md
/// §7.9, Where the forecast's time goes).
pub const BUDGET: Duration = Duration::from_secs(EVERY.as_secs() / 4);

/// A run is cut too after this many times its budget on the wall clock (120 s for `BUDGET`), so a
/// starved one can't hold the lead's next forecast (`Agent::forecasting`) nor the NAS self-heal's
/// wait for idle threads indefinitely.
const BACKSTOP: u32 = 8;

/// The calling thread's CPU time so far.
fn thread_cpu() -> Duration {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: writes only `t`.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

/// The calling thread at the utility QoS (macOS): a forecast's runs give way to the jobs and the
/// owner's own work. Elsewhere, nothing.
pub fn low_priority() {
    #[cfg(target_os = "macos")]
    // SAFETY: sets only the calling thread's QoS class; no pointers.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
}

/// How the three runs scale a cost's seconds, (known, guessed) each: expected, soon, late.
const SCALINGS: [(f64, f64); 3] = [(1.0, 1.0), (0.95, 0.75), (1.15, 1.6)];

fn scaled((known, guess): (f64, f64)) -> impl Fn(&Cost) -> f64 {
    move |c| c.secs * if c.known() { known } else { guess }
}

/// The forecast for `inp`, within `BUDGET`.
pub fn forecast(the_plan: &'static dyn crate::agent::seam::Plan, inp: &Input) -> Forecast {
    forecast_by(the_plan, inp, BUDGET)
}

/// The forecast for `inp`, each run cut once it has computed `budget`.
pub fn forecast_by(the_plan: &'static dyn crate::agent::seam::Plan, inp: &Input, budget: Duration) -> Forecast {
    // (Two machines under one name would share its lanes while `run_of` numbers both: task #816's
    // panic in `runs_of`. A bug upstream; keep each name's first, say so, and go on.)
    let mut seen = std::collections::BTreeSet::new();
    if !inp.machines.iter().all(|m| seen.insert(m.name.as_str())) {
        let mut seen = std::collections::BTreeSet::new();
        let mut once = inp.clone();
        once.machines.retain(|m| seen.insert(m.name.clone()));
        eprintln!("forecast: machines listed twice under one name, kept the first of each (a bug)");
        return forecast_by(the_plan, &once, budget);
    }
    if inp.paused {
        return Forecast {
            at: inp.now,
            why: Some(PAUSED.into()),
            measured: 1.0,
            pace: inp.pace.clone(),
            ..Default::default()
        };
    }
    let now = inp.now as f64;
    // (The three runs side by side, each on a thread of its own, on one listing of the work: its
    // waits are the most of its making, task #988.)
    let its = items(the_plan, inp);
    let (expected, soon, late) = std::thread::scope(|s| {
        let soon = s.spawn(|| {
            low_priority();
            run_on(the_plan, inp, its.clone(), &scaled(SCALINGS[1]), budget)
        });
        let late = s.spawn(|| {
            low_priority();
            run_on(the_plan, inp, its.clone(), &scaled(SCALINGS[2]), budget)
        });
        let expected = run_on(the_plan, inp, its.clone(), &scaled(SCALINGS[0]), budget);
        let join =
            |h: std::thread::ScopedJoinHandle<'_, Sim>| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
        (expected, join(soon), join(late))
    });
    // (Done once every machine is free too: a job under way that's no item, a daily one's, ends then.)
    let end_of = |s: &Sim| -> Option<f64> {
        s.done.then(|| s.items.iter().map(|i| i.end).chain(inp.machines.iter().map(|m| m.busy_s)).fold(0.0, f64::max))
    };
    let at = |secs: f64| (now + secs).round() as u64;
    let mut f = Forecast {
        at: inp.now,
        partial: expected.cut || soon.cut || late.cut,
        curve: curve(&expected.items),
        pace: inp.pace.clone(),
        ..Default::default()
    };
    f.grants = expected.grants.iter().map(|g| Grant { at: at(g.at) as f64, ..g.clone() }).collect();
    // (Never while a machine is busy: a job under way is work left, items or not.)
    let busy = inp.machines.iter().any(|m| m.busy_s > 0.0) || !inp.running.is_empty();
    let nothing = expected.items.is_empty() && !busy;
    f.why = match (&inp.blind, nothing, expected.done) {
        (Some(b), _, _) => Some(b.clone()),
        _ if expected.cut => Some(format!(
            "the forecast's run was cut at its time budget ({} s): the work placed by then is shown, no finish",
            BUDGET.as_secs()
        )),
        (None, true, _) => Some(NOTHING_LEFT.into()),
        (None, false, false) => Some("there's work no machine can do (none that fits it is around)".into()),
        _ => None,
    };
    if f.why.is_none() {
        f.done_at = end_of(&expected).map(at);
        f.range = match (end_of(&soon), end_of(&late)) {
            (Some(a), Some(b)) if !soon.cut && !late.cut => Some([at(a.min(b)), at(a.max(b))]),
            _ => None,
        };
    }
    f.placed = expected
        .items
        .iter()
        .filter_map(|i| {
            Some(Placed {
                lane: i.by?,
                step: i.step.clone(),
                target: i.target.clone(),
                from: at(i.from),
                until: at(i.end),
            })
        })
        .collect();
    // (From 0, not the empty sum's -0.)
    let total: f64 = expected.items.iter().map(|i| i.cost.secs).fold(0.0, |a, b| a + b);
    let known: f64 = expected.items.iter().filter(|i| i.cost.known()).map(|i| i.cost.secs).fold(0.0, |a, b| a + b);
    f.measured = if total > 0.0 { (known / total * 100.0).round() / 100.0 } else { 1.0 };
    // Each step's work left, in the order the steps first come.
    let mut order: Vec<String> = Vec::new();
    for i in &expected.items {
        if !order.contains(&i.step) {
            order.push(i.step.clone());
        }
    }
    for s in order {
        let mine: Vec<&Item> = expected.items.iter().filter(|i| i.step == s).collect();
        let ends = mine.iter().filter(|i| i.by.is_some()).map(|i| i.end).fold(0.0, f64::max);
        f.steps.push(StepFc {
            shared: mine.iter().any(|i| i.shared),
            left: mine.len(),
            work_s: mine.iter().map(|i| i.cost.secs).sum::<f64>().round() as u64,
            basis: mine.iter().fold(BTreeMap::new(), |mut m, i| {
                *m.entry(i.cost.basis).or_default() += 1;
                m
            }),
            done_at: mine.iter().all(|i| i.by.is_some()).then(|| at(ends)),
            step: s,
        });
    }
    // Each region: its work left, when it's done, and when it's on the map as it's built now: its
    // z6 tiles and map tiles done, and a catalogue within the minute.
    let on_map_at = |mine: &[usize]| {
        let its: Vec<&Item> = mine
            .iter()
            .map(|&i| &expected.items[i])
            .filter(|i| matches!(i.phase, Phase::Region | Phase::Publish))
            .collect();
        (!its.is_empty() && its.iter().all(|i| i.by.is_some()))
            .then(|| its.iter().map(|i| i.end).fold(0.0, f64::max) + CATALOGUE_EVERY_S as f64)
    };
    for (k, r) in inp.regions.iter().enumerate() {
        let mine = &expected.region_items[k];
        let mut left: BTreeMap<String, usize> = BTreeMap::new();
        for &i in mine {
            *left.entry(expected.items[i].step.clone()).or_default() += 1;
        }
        let ready = mine
            .iter()
            .all(|&i| expected.items[i].by.is_some())
            .then(|| mine.iter().map(|&i| expected.items[i].end).fold(0.0, f64::max));
        f.regions.push(RegionFc {
            id: r.id.clone(),
            rank: k,
            on_map: r.on_map,
            left,
            ready_at: ready.map(at),
            // (None with no terrain, z6 tile or map tile of its left, or on the map as it is.)
            map_at: if r.on_map == Some(true) { None } else { on_map_at(mine).map(at) },
        });
    }
    // The cadenced publishes: each once its cadence has passed since it last went, or once no z6 tile
    // or terrain is left (seam::Planned::idle), whichever comes first.
    let idle_s = expected.items.iter().filter(|i| i.phase == Phase::Region).map(|i| i.end).fold(0.0, f64::max);
    let c = &inp.cadence;
    f.publishes = [
        ("low", c.since_low, LOW_EVERY_S),
        ("marks", c.since_marks, MARKS_EVERY_S),
        ("overlays", c.since_overlays, OVERLAYS_EVERY_S),
    ]
    .into_iter()
    .map(|(what, since, every)| {
        let due = since.map_or(0, |s| every.saturating_sub(s)) as f64;
        PublishFc { what: what.into(), at: at(due.min(idle_s)) }
    })
    .collect();
    // Each machine's next jobs: its items in time order (not those it's building now), a step's run
    // together (at most three).
    let mut run_of: Vec<Option<(usize, usize)>> = vec![None; expected.items.len()];
    for (m, mac) in inp.machines.iter().enumerate() {
        let mut mine: Vec<&Item> = expected.items.iter().filter(|i| i.by == Some(m) && !i.running).collect();
        mine.sort_by(|a, b| a.from.total_cmp(&b.from));
        let mut next: Vec<NextFc> = Vec::new();
        for i in mine {
            let full = next.len() >= 3;
            match next.last_mut() {
                Some(l) if l.step == i.step && l.targets.len() < 12 => {
                    l.targets.push(i.target.clone());
                    l.until = at(i.end);
                }
                _ if full => break,
                _ => next.push(NextFc {
                    step: i.step.clone(),
                    targets: vec![i.target.clone()],
                    from: at(i.from),
                    until: at(i.end),
                }),
            }
        }
        f.next.insert(mac.name.clone(), next);
        // Its schedule: the job its slot runs now that no item is (its time left), then its runs:
        // its jobs (`Stretch`).
        // (Each run's items, for `Runs`.)
        let mine: Vec<(usize, &Item)> = expected.items.iter().enumerate().filter(|(_, i)| i.by == Some(m)).collect();
        let under_way = mac.running.as_ref().filter(|_| !mine.iter().any(|(_, i)| i.running));
        let runs = mine
            .iter()
            .map(|&(k, i)| Placing {
                step: &i.step,
                from: i.from,
                until: i.end,
                n: 1,
                job: i.job,
                item: Some(k),
                drop: i.drop,
            })
            .chain(under_way.map(|(step, n)| Placing {
                step,
                from: 0.0,
                until: mac.busy_s.max(0.0),
                n: *n,
                job: 0,
                item: None,
                drop: false,
            }));
        for (k, r) in lane_stretches(runs.collect(), now).into_iter().enumerate() {
            for i in r.1 {
                run_of[i] = Some((m, k));
            }
            f.lanes.entry(mac.name.clone()).or_default().push(r.0);
        }
        f.lanes.entry(mac.name.clone()).or_default();
    }
    f.runs = runs_of(&f.lanes, &inp.machines, &expected, &run_of);
    f.graph = graph_of(&expected);
    f
}

/// A target placed on a lane, or its job under way that no item is (`item` None): its step, from
/// and until when (s from now), how many targets, its job (`Item::job`; 0, the job under way).
struct Placing<'a> {
    step: &'a str,
    from: f64,
    until: f64,
    n: usize,
    job: u32,
    item: Option<usize>,
    drop: bool,
}

/// A lane's stretches (`Stretch`) from its placings, `now` the forecast's time (unix s): each its items in
/// time order. A step's jobs one after another (two minutes apart at most) are one run, each job
/// after the first a cut.
fn lane_stretches(mut placed: Vec<Placing>, now: f64) -> Vec<(Stretch, Vec<usize>)> {
    // (The job under way first.)
    placed.sort_by(|a, b| a.from.total_cmp(&b.from).then(a.item.is_some().cmp(&b.item.is_some())));
    let at = |s: f64| (now + s).round() as u64;
    let mut out: Vec<(Stretch, Vec<usize>, u32)> = Vec::new();
    for p in placed {
        match out.last_mut() {
            Some((l, items, job)) if l.step == p.step && l.drop == p.drop && p.from <= l.until as f64 - now + 120.0 => {
                if *job != p.job {
                    let off = (at(p.from).max(l.from) - l.from) as u32;
                    l.cuts.push([off, items.len() as u32]);
                    *job = p.job;
                }
                l.until = l.until.max(at(p.until));
                l.n += p.n;
                items.extend(p.item);
            }
            _ => out.push((
                Stretch {
                    step: p.step.to_string(),
                    from: at(p.from),
                    until: at(p.until),
                    n: p.n,
                    cuts: Vec::new(),
                    drop: p.drop,
                },
                p.item.into_iter().collect(),
                p.job,
            )),
        }
    }
    out.into_iter().map(|(l, items, _)| (l, items)).collect()
}

/// The run's items and what each waits on (`Forecast::graph`).
fn graph_of(sim: &Sim) -> crate::coord::history::Graph {
    let ids = |v: &[usize]| v.iter().map(|&i| i as u32).collect();
    crate::coord::history::Graph {
        keys: sim.items.iter().map(|it| (it.step.clone(), it.target.clone())).collect(),
        deps: sim.items.iter().map(|it| ids(&it.deps)).collect(),
        sets: sim.items.iter().map(|it| ids(&it.groups)).collect(),
        groups: sim.groups.iter().map(|g| ids(g)).collect(),
    }
}

/// The schedule's runs (`Runs`): `run_of` each item's (machine, its run in its lane), the
/// machines' lanes in `lanes` by their names.
fn runs_of(
    lanes: &BTreeMap<String, Vec<Stretch>>,
    machines: &[Machine],
    sim: &Sim,
    run_of: &[Option<(usize, usize)>],
) -> Runs {
    // (Each lane's first run's number: the lanes by name, as `lanes` holds them.)
    let mut first: BTreeMap<&str, u32> = BTreeMap::new();
    let mut total = 0u32;
    for (name, l) in lanes {
        first.insert(name, total);
        total += l.len() as u32;
    }
    let id = |k: usize| -> Option<u32> {
        let (m, r) = run_of.get(k).copied().flatten()?;
        Some(first.get(machines.get(m)?.name.as_str())? + r as u32)
    };
    let ids: Vec<Option<u32>> = (0..sim.items.len()).map(id).collect();
    // (Each run's targets in time order, as its jobs' cuts count them: `lane_stretches`.)
    let mut by_run: Vec<Vec<usize>> = vec![Vec::new(); total as usize];
    for (k, r) in ids.iter().enumerate() {
        if let Some(r) = r {
            by_run[*r as usize].push(k);
        }
    }
    // (Each run's cuts, in the runs' order: its jobs after the first, by the targets before each.)
    let cuts: Vec<&[[u32; 2]]> = lanes.values().flat_map(|l| l.iter().map(|l| l.cuts.as_slice())).collect();
    let jobs: Vec<u32> = cuts.iter().map(|c| c.len() as u32 + 1).collect();
    let first_job: Vec<u32> = jobs.iter().scan(0u32, |n, &j| Some(std::mem::replace(n, *n + j))).collect();
    let mut item_job: Vec<Option<u32>> = vec![None; sim.items.len()];
    let targets: Vec<Vec<String>> = by_run
        .into_iter()
        .enumerate()
        .map(|(r, mut v)| {
            v.sort_by(|&a, &b| sim.items[a].from.total_cmp(&sim.items[b].from));
            for (p, &k) in v.iter().enumerate() {
                let j = cuts.get(r).map_or(0, |c| c.iter().filter(|c| c[1] as usize <= p).count() as u32);
                item_job[k] = Some(first_job.get(r).copied().unwrap_or(0) + j);
            }
            v.into_iter().map(|k| sim.items[k].target.clone()).collect()
        })
        .collect();
    // (A set's job, worked out once: the sets are shared by many items. Its item ending last's: a
    // reader of it waits till then.)
    let group_job: Vec<Option<u32>> = sim
        .groups
        .iter()
        .map(|g| {
            g.iter()
                .filter(|&&d| item_job.get(d).copied().flatten().is_some())
                .max_by(|&&a, &&b| sim.items[a].end.total_cmp(&sim.items[b].end))
                .and_then(|&d| item_job[d])
        })
        .collect();
    let mut deps: std::collections::BTreeSet<[u32; 2]> = std::collections::BTreeSet::new();
    for (k, it) in sim.items.iter().enumerate() {
        let Some(b) = item_job[k] else { continue };
        let on = it.deps.iter().filter_map(|&d| item_job.get(d).copied().flatten());
        let on_sets = it.groups.iter().filter_map(|&g| group_job.get(g).copied().flatten());
        deps.extend(on.chain(on_sets).filter(|&a| a != b).map(|a| [a, b]));
    }
    Runs { targets, jobs, deps: deps.into_iter().collect(), item_job, after: Vec::new() }
}

/// Where the lead's real grants departed from forecast `f`'s next steps: each Mac and step granted
/// more or fewer jobs from when it was made until `until` (unix s) than its run granted by then
/// (`Forecast::grants`, its first hour), by 2 or more and by more than half the forecast's
/// (`departed`): ((Mac, step), in words). `real`: the grants made, (unix s, Mac, step). A Mac
/// granted fewer says what the lead answered its asks in that span (`answers`, each Mac's:
/// crate::coord::Coordinator::answers; `held_back`). The steps it doesn't model (`UNFORESEEN`)
/// are left out: their grants depart from no forecast. What the scheduler can't see moves them
/// (docs/pool.md §7.2, One scheduler): a job ending sooner or later than measured, a Mac's asks
/// held or refused, the work offered only once a plan lists it, the live state its rules read.
pub fn departures(
    f: &Forecast,
    real: &[(u64, String, String)],
    answers: &BTreeMap<String, Vec<crate::coord::Answer>>,
    until: u64,
) -> Vec<((String, String), String)> {
    let until = until.min(f.at + GRANTS_S as u64);
    let mut n: BTreeMap<(&str, &str), (usize, usize)> = BTreeMap::new();
    for g in f.grants.iter().filter(|g| (g.at as u64) < until) {
        n.entry((&g.mac, &g.step)).or_default().0 += 1;
    }
    for (_, mac, step) in real.iter().filter(|r| r.0 >= f.at && r.0 < until && foreseen(&r.2)) {
        n.entry((mac, step)).or_default().1 += 1;
    }
    n.into_iter()
        .filter(|(_, (fc, got))| departed(*fc, *got))
        .map(|((mac, step), (fc, got))| {
            let mut d = format!("{mac}: {got} job(s) of {step} granted, the forecast's {fc}");
            if got < fc {
                let its: Vec<&crate::coord::Answer> =
                    answers.get(mac).into_iter().flatten().filter(|a| a.at >= f.at && a.at < until).collect();
                d.push_str(&held_back(step, &its));
            }
            ((mac.to_string(), step.to_string()), d)
        })
        .collect()
}

/// The jobs of a step the forecast doesn't model a Mac may be granted in an hour before the lead
/// says so (`unforeseen_many`): one every two minutes. GC goes on through its chunks in one job and
/// the backup is daily, so more is jobs started again and again (task #1392: hundreds of empty GC
/// and backup jobs an hour).
pub const UNFORESEEN_HOURLY: usize = 30;

/// Each Mac and step the forecast doesn't model (`UNFORESEEN`) granted more than
/// `UNFORESEEN_HOURLY` jobs in the hour before `now` (unix s): ((Mac, step), in words). `real`:
/// the grants made, (unix s, Mac, step), as for `departures`.
pub fn unforeseen_many(real: &[(u64, String, String)], now: u64) -> Vec<((String, String), String)> {
    let mut n: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (_, mac, step) in real.iter().filter(|r| r.0 + 3600 > now && r.0 < now && !foreseen(&r.2)) {
        *n.entry((mac, step)).or_default() += 1;
    }
    n.into_iter()
        .filter(|(_, got)| *got > UNFORESEEN_HOURLY)
        .map(|((mac, step), got)| {
            let d = format!("{mac}: {got} job(s) of {step} granted in the last hour, more than {UNFORESEEN_HOURLY}");
            ((mac.to_string(), step.to_string()), d)
        })
        .collect()
}

/// Why a Mac was granted fewer jobs of `step` than the forecast foresaw, from its asks answered
/// since (`departures`), in words: none heard (its agent asks only while a slot is free and nothing
/// holds it back); else how many, the jobs they wanted and got, how many were offered the step (the
/// lead offers a target once a plan of its lists it, the run as soon as what it waits on ends), and
/// the rules that held them back, each counted, the one deciding by live state the forecast can't
/// know said so (`sched::Rule::live`), with the last one's words.
fn held_back(step: &str, its: &[&crate::coord::Answer]) -> String {
    if its.is_empty() {
        return "; no ask from it since (its agent asks only with a slot free and nothing holding it back: \
                its jobs running past their measures, an update's restart, its memory guard or a stalled loop, \
                none of which the forecast foresees)"
            .to_string();
    }
    let want: usize = its.iter().map(|a| a.want).sum();
    let got: usize = its.iter().map(|a| a.got).sum();
    let offered = its.iter().filter(|a| a.offered.iter().any(|s| s == step)).count();
    let mut out =
        format!("; its {} ask(s) since wanted {want} job(s) and got {got}, {offered} offered {step}", its.len());
    if offered == 0 {
        out.push_str(
            " (not listed by the lead's plan yet: the forecast's run offers a target as soon as what it waits on ends)",
        );
    }
    // (Each rule counted, the most first; the refusals before any packing together.)
    let mut by: BTreeMap<Option<crate::coord::sched::Rule>, usize> = BTreeMap::new();
    for a in its {
        match (&a.refused, &a.held) {
            (Some(_), _) => *by.entry(None).or_default() += 1,
            (None, Some(h)) => *by.entry(Some(h.rule)).or_default() += 1,
            (None, None) => {}
        }
    }
    if by.is_empty() {
        if got >= want {
            out.push_str("; each got all it wanted: its slots freed later than the forecast's lanes");
        }
        return out;
    }
    let mut rules: Vec<(Option<crate::coord::sched::Rule>, usize)> = by.into_iter().collect();
    rules.sort_by_key(|r| std::cmp::Reverse(r.1));
    let named: Vec<String> = rules
        .iter()
        .map(|&(r, k)| match r {
            None => format!("refused before packing ×{k} (the lead's or its agent's state now)"),
            Some(r) => match r.live() {
                Some(live) => format!("{} ×{k} (live: {live})", r.name()),
                None => format!("{} ×{k}", r.name()),
            },
        })
        .collect();
    let last = its.iter().rev().find_map(|a| a.refused.clone().or_else(|| a.held.as_ref().map(|h| h.words.clone())));
    out.push_str(&format!("; held back by {}", named.join(", ")));
    if let Some(w) = last {
        out.push_str(&format!("; the last: {w}"));
    }
    out
}

/// Whether `got` jobs granted against the forecast's `fc` is a departure worth saying: 2 or more
/// apart, and more than half the forecast's (one job early or late is the clock's grain).
fn departed(fc: usize, got: usize) -> bool {
    let d = fc.abs_diff(got);
    d >= 2 && d * 2 > fc
}

/// The lane each lease (its worker, its step, whether the worker names its slot) runs on, in order
/// (task #863): one naming its slot (crate::coord::lease::Lease::slot) on that slot's lane; one
/// naming its Mac alone (granted, its slot not said yet, or an older app's) on a lane of that Mac
/// (`Machine::mac`, else its name) running a job of its step and not taken, else on the lane named
/// as the worker. (Each on its Mac's first lane put the jobs of its other slots there, over the job
/// it ran: its time left drawn as a job to come of theirs.)
pub fn lease_lanes(machines: &[Machine], leases: &[(&str, &str, bool)]) -> Vec<Option<usize>> {
    let mut taken = vec![false; machines.len()];
    let mut out: Vec<Option<usize>> = leases
        .iter()
        .map(|&(worker, _, slot)| {
            let k = machines.iter().position(|m| m.name == worker).filter(|_| slot)?;
            taken[k] = true;
            Some(k)
        })
        .collect();
    for (o, &(worker, step, slot)) in out.iter_mut().zip(leases) {
        if slot {
            continue;
        }
        let on = |m: &Machine| m.name == worker || m.mac.as_deref() == Some(worker);
        *o = (0..machines.len())
            .find(|&k| !taken[k] && on(&machines[k]) && machines[k].running.as_ref().is_some_and(|r| r.0 == step))
            .or_else(|| machines.iter().position(|m| m.name == worker));
        if let Some(k) = *o {
            taken[k] = true;
        }
    }
    out
}

/// A Mac's lanes: the slots it may run now (`slots`: the lead's ramp's, a member's
/// crate::agent::job_slots), never fewer than its last running slot's number (`running`: the slots
/// running a job, 0 the first; they may be sparse).
pub fn slot_lanes(slots: usize, running: &[usize]) -> usize {
    let top = running.iter().map(|k| k + 1).max().unwrap_or(0);
    slots.max(top).max(1)
}

/// Each member's speed against the lead's, from the history: for each shared step both did,
/// the lead's mean time a target over the member's (the last week's, three or more each),
/// their middle; `default` where there's too little to tell.
pub fn speeds(
    the_plan: &dyn crate::agent::seam::Plan,
    events: &[crate::coord::history::Event],
    lead: &str,
    members: &[String],
    default: f64,
) -> BTreeMap<String, (f64, bool)> {
    // (worker, step) → seconds a target.
    let mut per: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for e in events {
        let (Some(w), Some(step), Some(secs)) = (&e.worker, &e.step, e.secs) else { continue };
        let ok = matches!(e.kind.as_str(), "end" | "done") && e.ok == Some(true) && !e.targets.is_empty();
        if ok && the_plan.shared().contains(&step.as_str()) {
            per.entry((w.clone(), step.clone())).or_default().push(secs / e.targets.len() as f64);
        }
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    members
        .iter()
        .map(|h| {
            let mut ratios: Vec<f64> = the_plan
                .shared()
                .iter()
                .filter_map(|s| {
                    let mine = per.get(&(h.clone(), s.to_string())).filter(|v| v.len() >= 3)?;
                    let theirs = per.get(&(lead.to_string(), s.to_string())).filter(|v| v.len() >= 3)?;
                    Some(mean(theirs) / mean(mine))
                })
                .collect();
            ratios.sort_by(f64::total_cmp);
            let speed = ratios.get(ratios.len() / 2).copied().unwrap_or(default).clamp(0.1, 3.0);
            (h.clone(), (speed, !ratios.is_empty()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::coord::client::LeaseCalls;

    #[test]
    fn a_machine_listed_twice_does_not_panic() {
        // (Task #816 review: two machines under one name shared its lanes while run_of numbered
        // both; runs_of indexed past them and the forecast thread panicked.)
        let regions = many(40);
        let c = mixed(false);
        let mut inp = two_macs(&regions, &c);
        let dup: Vec<Machine> = inp.machines[..4].to_vec();
        inp.machines.extend(dup);
        let fc = forecast(crate::agent::seam::for_tests(), &inp);
        assert_eq!(fc, forecast(crate::agent::seam::for_tests(), &two_macs(&regions, &c)));
        assert_eq!(fc.runs.targets.len(), fc.lanes.values().map(Vec::len).sum::<usize>());
    }

    /// A run's jobs (`Stretch::cuts`): each one's start (unix s) and targets.
    fn jobs_of<'a>(l: &Stretch, targets: &'a [String]) -> Vec<(u64, &'a [String])> {
        let mut at: Vec<(u64, usize)> = vec![(l.from, 0)];
        at.extend(l.cuts.iter().map(|c| (l.from + c[0] as u64, c[1] as usize)));
        (0..at.len()).map(|k| (at[k].0, &targets[at[k].1..at.get(k + 1).map_or(targets.len(), |a| a.1)])).collect()
    }

    #[test]
    fn each_job_to_come_is_one_the_scheduler_granted() {
        // (Task #834) every job granted in the run's first hour is one job of a lane's run, its
        // targets the grant's in its order.
        let regions = many(40);
        let c = mixed(false);
        let inp = two_macs(&regions, &c);
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert!(!f.grants.is_empty());
        let runs: Vec<&Stretch> = f.lanes.values().flatten().collect();
        assert_eq!(runs.len(), f.runs.targets.len());
        let jobs: Vec<(&str, u64, &[String])> = runs
            .iter()
            .zip(&f.runs.targets)
            .flat_map(|(l, t)| jobs_of(l, t).into_iter().map(|(at, t)| (l.step.as_str(), at, t)))
            .collect();
        assert!(jobs.len() > runs.len(), "some run holds several jobs");
        for g in &f.grants {
            let k = jobs.iter().filter(|j| j.0 == g.step && j.2 == g.targets.as_slice()).count();
            assert_eq!(k, 1, "{} {:?}: one job of its own", g.step, g.targets);
        }
        // (The status: ~10 bytes a job past a run's first.)
        let n: usize = runs.iter().map(|l| l.cuts.len()).sum();
        let with = serde_json::to_vec(&f.lanes).unwrap().len();
        let mut bare = f.lanes.clone();
        bare.values_mut().flatten().for_each(|l| l.cuts.clear());
        assert!(with - serde_json::to_vec(&bare).unwrap().len() <= 14 * n + 10 * runs.len());
    }

    #[test]
    fn lane_runs_cut_a_steps_jobs_one_after_another() {
        let p = |job, from: f64, item| Placing {
            step: "terrain-deep",
            from,
            until: from + 600.0,
            n: 1,
            job,
            item: Some(item),
            drop: false,
        };
        let now = 1_000_000.0;
        let late = 20_000.0;
        let placed = vec![
            p(2, 0.0, 0),
            p(2, 600.0, 1),
            p(3, 1200.0, 2),
            // (Another step between: a run of its own.)
            Placing { step: "pois", from: 1800.0, until: 1810.0, n: 1, job: 4, item: Some(3), drop: false },
            p(5, 1810.0, 4),
            p(6, late, 5),
            // (Up to two minutes later: the run's next job.)
            p(7, late + 700.0, 6),
            p(7, late + 1300.0, 7),
            // (Past a gap: a run again.)
            p(8, late + 9000.0, 8),
            // (The job under way that no item is: names no target.)
            Placing { step: "terrain-deep", from: 0.0, until: 0.0, n: 3, job: 0, item: None, drop: false },
        ];
        let runs = lane_stretches(placed, now);
        let got: Vec<(u64, usize, Vec<[u32; 2]>, Vec<usize>)> =
            runs.into_iter().map(|(l, i)| (l.from - now as u64, l.n, l.cuts, i)).collect();
        let l = late as u64;
        assert_eq!(
            got,
            [
                (0, 6, vec![[0, 0], [1200, 2]], vec![0, 1, 2]),
                (1800, 1, vec![], vec![3]),
                (1810, 1, vec![], vec![4]),
                (l, 3, vec![[700, 1]], vec![5, 6, 7]),
                (l + 9000, 1, vec![], vec![8]),
            ]
        );
        // (A run of one job: no `cuts` in the status.)
        let l = Stretch { step: "x".into(), from: 1, until: 2, n: 1, cuts: Vec::new(), drop: false };
        assert_eq!(serde_json::to_value(&l).unwrap(), serde_json::json!({"step": "x", "from": 1, "until": 2, "n": 1}));
        assert_eq!(
            serde_json::from_value::<Stretch>(serde_json::json!({"step": "x", "from": 1, "until": 2, "n": 1})).unwrap(),
            l
        );
    }

    #[test]
    fn to_come_by_mac_and_step_each_tile_once_at_its_first_start() {
        let p = |lane, step: &str, target: &str, from| Placed {
            lane,
            step: step.into(),
            target: target.into(),
            from,
            until: from + 60,
        };
        let placed = vec![
            p(0, "terrain-hi", "6/33/20", 1300),
            p(1, "terrain-hi", "6/33/20", 1100),
            p(1, "terrain-hi", "6/34/20", 1050),
            p(2, "slope-hi", "3/4/2", 1500),
            p(2, "unit", "norway", 1000),
            p(9, "terrain-hi", "6/1/1", 1000),
            p(2, "terrain-hi", "6/99/1", 1000),
        ];
        let macs = ["m4".to_string(), "m4".to_string(), "m6".to_string()];
        let tc = ToCome::of(&placed, &macs, 1000);
        assert_eq!(tc.macs["m4"]["terrain-hi"], vec![("6/34/20".to_string(), 50), ("6/33/20".to_string(), 100)]);
        assert_eq!(tc.macs["m6"].keys().collect::<Vec<_>>(), ["slope-hi"]);
        let a = tc.answer();
        assert_eq!(a["at"], 1000);
        assert_eq!(a["macs"]["m6"]["slope-hi"][0], serde_json::json!(["3/4/2", 500]));
        assert_eq!(a["macs"].as_object().unwrap().keys().collect::<Vec<_>>(), ["m4", "m6"]);
    }

    /// The CPU time (ms) a timing test's forecast may take (a debug build: well under a second
    /// in a release one); every job of its runs the coordinator's scheduler's decision.
    const CPU_BOUND_MS: u128 = 8000;

    use super::*;
    use std::collections::BTreeSet;

    fn region(id: &str, terrain: &[&str], units: &[&str], slope: &[&str]) -> RegionLeft {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        RegionLeft {
            id: id.into(),
            on_map: None,
            units: v(units),
            own_units: v(units),
            terrain: v(terrain),
            own_terrain: v(terrain),
            slope: v(slope),
            ..Default::default()
        }
    }

    /// The drops (`Input::drops`, seam::Planned::drops) go first at their own cost, a run of their
    /// own shown as drops (`Stretch::drop`), never merged with the step's builds: 29 Oct's 4,275 Alaska
    /// drops were the yukon's, last, at a build's 76 s each.
    #[test]
    fn drops_go_first_as_drops() {
        let mut r = region("yukon", &[], &[], &[]);
        r.own_terrain_deep = vec!["10/256/352".into(), "10/256/353".into()];
        r.terrain_deep = vec!["10/256/352".into(), "10/256/353".into(), "10/100/252".into()];
        let regions = vec![r];
        let mut inp = input(&regions, vec![mac("m4", 1.0)], &cost);
        let drop = Cost { secs: 2.0, basis: Basis::Own, peak_mb: 150 };
        inp.drops = vec![
            ("terrain-deep".into(), "10/100/252".into(), drop),
            ("terrain-deep".into(), "10/100/253".into(), drop),
        ];
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        let lanes: Vec<&Stretch> = f.lanes.values().flatten().filter(|l| l.step == "terrain-deep").collect();
        assert_eq!(lanes.len(), 2, "{lanes:?}");
        assert!(lanes[0].drop && lanes[0].n == 2 && lanes[0].until - lanes[0].from <= 5, "{lanes:?}");
        assert!(!lanes[1].drop && lanes[1].n == 2 && lanes[1].from >= lanes[0].until, "{lanes:?}");
        assert!(serde_json::to_value(lanes[1]).unwrap().get("drop").is_none(), "said only of a drop");
    }

    fn stopped(kind: &str, job: u64, step: &str, targets: &[&str]) -> crate::coord::history::Event {
        crate::coord::history::Event {
            job: Some(jobrt::jobid::JobId { epoch: 1, n: job }),
            step: Some(step.into()),
            targets: targets.iter().map(|t| t.to_string()).collect(),
            ..crate::coord::history::Event::new(kind)
        }
    }

    /// `e` as production writes a stopped job's end: not ok, its step's cost version, and its
    /// progress: `(done, total, since, secs)`, on `target` (a cost key) if any.
    fn at_progress(
        e: crate::coord::history::Event,
        target: Option<&str>,
        (done, total, since, secs): (f64, f64, f64, f64),
    ) -> crate::coord::history::Event {
        let progress = crate::coord::history::Progress {
            target: target.map(str::to_string),
            done,
            total,
            since,
            unit: "z6 tiles".into(),
            secs,
        };
        let v = e.step.as_deref().map(|s| crate::agent::seam::for_tests().cost_version(s));
        crate::coord::history::Event { ok: Some(false), v, progress: Some(progress), ..e }
    }

    /// Task #1449, the first source of a target's time: a run that stopped short says how far it got
    /// (available-bathymetry on 18 Nov: 2,169 s paused, its z6 tiles made and to make): its pace
    /// (past what its first beat said) over what is left, put at the lead's by its worker. The
    /// whole total where no later run showed the step resuming (it redoes its target), the rest of
    /// it where one did. Its progress's target is a cost key of the event's step; its one target
    /// when it names none.
    #[test]
    fn a_stopped_runs_pace_gives_the_time_left_of_its_target() {
        let plan = crate::agent::seam::for_tests();
        let none = |_: f64, _: Option<&str>, _: &str| unreachable!();
        assert!(partials(plan, &[], &none).is_empty());
        let key = |s: &str, t: &str| (s.to_string(), t.to_string());
        let bathy = key("available", "available-bathymetry");
        let half = |s: f64, w: Option<&str>, _: &str| if w == Some("m1") { s / 0.5 } else { s };
        let at = |e: &[crate::coord::history::Event]| partials(plan, e, &half);
        // (800 of 2,000 tiles in 2,169 s: 2.711 s each; a step not seen to resume redoes all 2,000.)
        let lease = stopped("lease", 1, "available", &["available-bathymetry"]);
        let fail = at_progress(stopped("fail", 1, "available", &[]), None, (800.0, 2000.0, 0.0, 2169.0));
        let redo = 2000.0 * 2169.0 / 800.0;
        assert_eq!(at(&[lease.clone(), fail.clone()]).get(&bathy), Some(&redo));
        // (The lead's own job's "end" on its own Mac's pace: put at the lead's.)
        let end = crate::coord::history::Event { worker: Some("m1".into()), ..fail.clone() };
        let got = at(&[crate::coord::history::Event { kind: "start".into(), ..lease.clone() }, end]);
        assert_eq!(got.get(&bathy), Some(&(redo / 0.5)));
        // (A first beat that said 300 already: the pace is of the 500 made since.)
        let late = at_progress(stopped("fail", 1, "available", &[]), None, (800.0, 2000.0, 300.0, 1000.0));
        assert_eq!(at(&[lease.clone(), late]).get(&bathy), Some(&(2000.0 * 1000.0 / 500.0)));
        // (Its progress's own target, a cost key, of a job with several; another step's key ignored.)
        let two = stopped("lease", 2, "available", &["available-terrain", "available-bathymetry"]);
        let named = |t| at_progress(stopped("fail", 2, "available", &[]), Some(t), (1.0, 4.0, 0.0, 10.0));
        let got = at(&[two.clone(), named("available available-bathymetry")]);
        assert_eq!(got.keys().collect::<Vec<_>>(), [&bathy]);
        assert!(at(&[two.clone(), named("pack available-bathymetry")]).is_empty(), "another step's key");
        // (None of a job with several targets and no target said.)
        assert!(at(&[two, at_progress(stopped("fail", 2, "available", &[]), None, (1.0, 4.0, 0.0, 10.0))]).is_empty());
        // (None of a run that said nothing, one that went through its total, or one of an older way.)
        let nothing = stopped("fail", 1, "available", &[]);
        let all = at_progress(stopped("fail", 1, "available", &[]), None, (5.0, 5.0, 0.0, 10.0));
        let old = crate::coord::history::Event { v: Some(plan.cost_version("available") + 1), ..fail.clone() };
        for e in [nothing, all, old] {
            assert!(at(&[lease.clone(), e]).is_empty());
        }
        // (A later run of it that counted a smaller total shows the step resuming: what is left.)
        let l3 = stopped("lease", 3, "available", &["available-bathymetry"]);
        let again = at_progress(stopped("fail", 3, "available", &[]), None, (500.0, 1200.0, 0.0, 1000.0));
        let got = at(&[lease.clone(), fail.clone(), l3.clone(), again]);
        assert_eq!(got.get(&bathy), Some(&(700.0 * 1000.0 / 500.0)));
        // (A run that ended well clears its finished targets: all its job's when it lists none, only
        // those it lists otherwise; and a done that finished only some keeps the stop of the rest.)
        let ok = |t: &[&str]| crate::coord::history::Event { ok: Some(true), ..stopped("done", 3, "available", t) };
        assert!(at(&[lease.clone(), fail.clone(), l3.clone(), ok(&[])]).is_empty());
        assert!(at(&[lease.clone(), fail.clone(), l3.clone(), ok(&["available-bathymetry"])]).is_empty());
        let got = at(&[lease.clone(), fail.clone(), l3, ok(&["available-terrain"])]);
        assert_eq!(got.get(&bathy), Some(&redo), "a done of another target leaves the stop");
        // (A member's job paused after finishing one of its two targets: a "done" with ok true and
        // that target alone, its progress the other's.)
        let two = stopped("lease", 4, "available", &["available-terrain", "available-bathymetry"]);
        let paused = at_progress(
            stopped("done", 4, "available", &["available-terrain"]),
            Some("available available-bathymetry"),
            (800.0, 2000.0, 0.0, 2169.0),
        );
        let paused = crate::coord::history::Event { ok: Some(true), ..paused };
        assert_eq!(at(&[two, paused]).into_iter().collect::<Vec<_>>(), [(bathy.clone(), redo)], "the rest's stop");
    }

    /// Task #1449: a target's time comes from its stopped run, else its own run, else the nearest
    /// figure of its step (its basis said), the stopped run's first.
    #[test]
    fn a_targets_time_is_its_stopped_runs_then_its_own_then_its_steps() {
        let plan = crate::agent::seam::for_tests();
        let v = plan.cost_version("terrain-deep");
        let run = |secs: u64| crate::coord::Cost { peak_mb: 1000, secs, v, ..Default::default() };
        let costs: BTreeMap<String, crate::coord::Cost> =
            [(cost_key("terrain-deep", "10/1/0"), run(40))].into_iter().collect();
        let book = crate::needs::Book::new(plan, &costs);
        let partial = |_: &str, t: &str| (t == "10/1/0" || t == "10/2/0").then_some(7.0);
        let per = |_: &str| (99.0, Basis::Sibling);
        let cost = |t: &str| target_cost(&book, "m4", "terrain-deep", t, &partial, &per, 500);
        let c = |secs, basis| Cost { secs, basis, peak_mb: 500 };
        assert_eq!(cost("10/1/0"), c(7.0, Basis::Partial), "a stopped run before its own");
        assert_eq!(cost("10/2/0"), c(7.0, Basis::Partial), "and where it has no run of its own");
        let none = |_: &str, _: &str| None;
        let cost =
            |t: &str, per: &dyn Fn(&str) -> (f64, Basis)| target_cost(&book, "m4", "terrain-deep", t, &none, per, 500);
        assert_eq!(cost("10/1/0", &per), c(40.0, Basis::Own));
        // (One run of its step: its model says a target not run itself, about the one that ran.)
        assert_eq!(cost("10/3/0", &per), c(40.0, Basis::Model));
        // (None of its step run the way it runs now: its step's nearest figure.)
        let none_run = BTreeMap::new();
        let book = crate::needs::Book::new(plan, &none_run);
        let cost =
            |t: &str, per: &dyn Fn(&str) -> (f64, Basis)| target_cost(&book, "m4", "terrain-deep", t, &none, per, 500);
        assert_eq!(cost("10/3/0", &per), c(99.0, Basis::Sibling));
        assert_eq!(cost("10/3/0", &|_| (30.0, Basis::Here)), c(30.0, Basis::Here));
        assert_eq!(cost("10/3/0", &|_| (300.0, Basis::Guess)), c(300.0, Basis::Guess));
        // (Said by the page, a step's targets by basis; measured but a guess and its step's mean.)
        assert!([Basis::Partial, Basis::Own, Basis::Here].iter().all(|b| c(1.0, *b).known()));
        assert!(![Basis::Model, Basis::Sibling, Basis::Guess].iter().any(|b| c(1.0, *b).known()));
        // (A record from before the basis: its `known` said it.)
        let old: Cost = serde_json::from_str(r#"{"secs":5.0,"known":true,"peak_mb":9}"#).unwrap();
        assert_eq!(old, Cost { secs: 5.0, basis: Basis::Own, peak_mb: 9 });
        let old: Cost = serde_json::from_str(r#"{"secs":5.0,"known":false,"peak_mb":9}"#).unwrap();
        assert_eq!(old.basis, Basis::Guess);
        let new: Cost = serde_json::from_str(&serde_json::to_string(&c(3.0, Basis::Partial)).unwrap()).unwrap();
        assert_eq!(new, c(3.0, Basis::Partial));
    }

    /// A map tile and the z6 tiles that draw it.
    fn tile(t: &str, units: &[&str]) -> (String, Vec<String>) {
        (t.into(), units.iter().map(|u| u.to_string()).collect())
    }

    fn mac(name: &str, speed: f64) -> Machine {
        Machine {
            name: name.into(),
            speed,
            measured: true,
            mem_mb: 6000,
            busy_s: 0.0,
            unit_extra_s: 0.0,
            ..Default::default()
        }
    }

    /// A budget no test's run meets.
    fn far() -> Duration {
        Duration::from_secs(3600)
    }

    fn cost(step: &str, _t: &str) -> Cost {
        let secs = match step {
            "terrain-hi" => 600.0,
            "unit" => 300.0,
            "slope-hi" => 200.0,
            _ => 100.0,
        };
        Cost { secs, basis: if step != "slope-hi" { Basis::Own } else { Basis::Guess }, peak_mb: 1000 }
    }

    fn input<'a>(regions: &'a [RegionLeft], machines: Vec<Machine>, c: &'a dyn Fn(&str, &str) -> Cost) -> Input<'a> {
        Input {
            now: 1_000_000,
            before: Vec::new(),
            drops: Vec::new(),
            regions: Cow::Borrowed(regions),
            costs: cost_table(regions, c),
            cadence: Cadence::default(),
            blind: None,
            chains: Vec::new(),
            after: vec![("marks".into(), "marks".into(), Cost { secs: 300.0, basis: Basis::Own, peak_mb: 0 })],
            idle: Vec::new(),
            machines,
            running: BTreeMap::new(),
            macs: BTreeMap::new(),
            sched: None,
            needs_pace: Default::default(),
            pace: None,
            unknown: Vec::new(),
            paused: false,
            along: Default::default(),
            code: crate::agent::forecast::VERSION,
        }
    }

    #[test]
    fn a_record_read_back_gives_the_same_forecast_compressed_or_not() {
        // (Task #409) its input, a target running.
        let regions =
            [region("a", &["6/1/1", "6/1/2"], &["u1", "u2"], &["6/1/1"]), region("b", &["6/2/1"], &["u3", "u4"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0), Machine { speed: 0.5, ..member("m1") }], &cost);
        inp.running.insert(("unit".into(), "u1".into()), 0);
        let mut lead = forecast(crate::agent::seam::for_tests(), &inp);
        lead.each_s.insert("unit".into(), 300.0);
        let rec = Record { input: inp, each_s: lead.each_s.clone(), waiting: BTreeMap::new() };
        let b = rec.bytes().unwrap();
        assert_eq!(b[0], b'{', "small: plain JSON");
        assert_eq!(Record::from_bytes(&b).unwrap().forecast(crate::agent::seam::for_tests()), lead);
        let z = zstd::encode_all(&b[..], 3).unwrap();
        assert_eq!(Record::from_bytes(&z).unwrap(), rec);
        // (An older lead's record, with the equal machines' lanes it no longer writes: read, the
        // field passed over.)
        let mut old: serde_json::Value = serde_json::from_slice(&b).unwrap();
        old["equal"] = serde_json::json!({ "lanes": [], "running": [] });
        assert_eq!(Record::from_bytes(&serde_json::to_vec(&old).unwrap()).unwrap(), rec);
        // (Task #409 review) a time not finite is written finite: it reads back.
        let mut bad = rec.clone();
        bad.input.machines[1].busy_s = f64::INFINITY;
        bad.input.machines[0].speed = f64::NAN;
        bad.each_s.insert("pack".into(), f64::NEG_INFINITY);
        let back = Record::from_bytes(&bad.bytes().unwrap()).unwrap();
        assert_eq!((back.input.machines[1].busy_s, back.input.machines[0].speed), (f64::MAX, 0.0));
        assert_eq!(back.each_s["pack"], -f64::MAX);
    }

    #[test]
    fn sparse_running_slots_keep_their_lanes() {
        // (Task #246: the first and the fourth slots running, two slots packed: four lanes, so the
        // fourth's job has its lane.)
        assert_eq!(slot_lanes(2, &[0, 3]), 4);
        assert_eq!(slot_lanes(2, &[3]), 4);
        assert_eq!(slot_lanes(2, &[0]), 2);
        // (Task #382) the slots it may run now, not as many as its cores would pack.
        assert_eq!(slot_lanes(2, &[]), 2, "the lead at ramp 2");
        assert_eq!(slot_lanes(4, &[0, 1]), 4, "a member's job_slots");
    }

    #[test]
    fn an_equal_lane_takes_only_what_fits_its_memory_and_its_running_targets_are_its_own() {
        let regions = [region("a", &[], &["u1", "u2"], &[])];
        let big = |step: &str, t: &str| Cost { peak_mb: if t == "u2" { 9000 } else { 1000 }, ..cost(step, t) };
        let inp = input(&regions, vec![mac("m4", 1.0)], &big);
        let mut running = BTreeMap::new();
        running.insert(("unit".to_string(), "u1".to_string()), 1);
        let small = Machine { busy_s: 300.0, ..mac("m1", 1.0) };
        // (The member's slot, a lane of any work that fits.)
        let small = Machine { lead_steps: true, ..small };
        let machines = vec![Machine { mem_mb: u64::MAX, ..mac("m4", 1.0) }, small];
        let f = forecast(crate::agent::seam::for_tests(), &Input { machines, running, ..inp });
        assert!(f.lanes["m1"].iter().all(|l| l.step != "unit" || l.n == 1), "{:?}", f.lanes["m1"]);
        assert!(
            f.next["m1"].iter().all(|n| n.step != "unit"),
            "u1 running there, u2 too big for it: {:?}",
            f.next["m1"]
        );
        assert!(f.next["m4"].iter().any(|n| n.targets.contains(&"u2".to_string())));
    }

    #[test]
    fn a_macs_leases_go_on_its_slots_running_their_steps_and_a_lane_starts_with_its_running_job() {
        // (Task #863) the Mac's first slot runs a long u1 job (5 h left), its second u2 (a minute
        // left); the coordinator's leases name the Mac alone, u1's first. Each lease goes on the
        // slot running its job, so the first slot's lane starts with u1 till its end (one job, no
        // other's targets), never with u2 drawn over it.
        let regions = [region("a", &[], &["u1", "u2", "u3"], &[])];
        let inp = input(&regions, vec![mac("m4", 1.0)], &cost);
        let slot = |k: usize, busy_s: f64| Machine {
            lead_steps: true,
            mem_mb: u64::MAX,
            busy_s,
            running: Some(("unit".into(), 1)),
            mac: Some("m4".into()),
            ..mac(&super::super::slot_worker("m4", k), 1.0)
        };
        let machines = vec![slot(0, 18_000.0), slot(1, 60.0)];
        // (The second slot's lease first: on the first lane free running its step.)
        assert_eq!(lease_lanes(&machines, &[("m4", "unit", false), ("m4", "unit", false)]), [Some(0), Some(1)]);
        assert_eq!(lease_lanes(&machines, &[("m4", "pois", false), ("m4 #2", "unit", true)]), [Some(0), Some(1)]);
        assert_eq!(lease_lanes(&machines, &[("m1", "unit", false)]), [None]);
        // (A lease naming its slot keeps that lane; one not said yet takes another running its step.)
        assert_eq!(lease_lanes(&machines, &[("m4", "unit", false), ("m4 #2", "unit", true)]), [Some(0), Some(1)]);
        assert_eq!(lease_lanes(&machines, &[("m4", "unit", false), ("m4", "unit", true)]), [Some(1), Some(0)]);
        let lanes = lease_lanes(&machines, &[("m4", "unit", true), ("m4 #2", "unit", true)]);
        let mut running = BTreeMap::new();
        for ((t, _), m) in [("u1", ()), ("u2", ())].iter().zip(lanes) {
            running.insert(("unit".to_string(), t.to_string()), m.unwrap());
        }
        let f = forecast(crate::agent::seam::for_tests(), &Input { machines, running, ..inp });
        let names: Vec<&String> = f.lanes.keys().collect();
        let run = |name: &str, k: usize| {
            let first: usize = names.iter().take_while(|n| n.as_str() != name).map(|n| f.lanes[*n].len()).sum();
            (&f.lanes[name][k], &f.runs.targets[first + k])
        };
        let (l, t) = run("m4", 0);
        assert_eq!((l.step.as_str(), l.from, l.until), ("unit", 1_000_000, 1_018_000), "{:?}", f.lanes);
        assert_eq!(jobs_of(l, t), [(1_000_000, &["u1".to_string()][..])], "the running job alone, its own target");
        let (l, t) = run("m4 #2", 0);
        assert_eq!(jobs_of(l, t)[0], (1_000_000, &["u2".to_string()][..]));
    }

    #[test]
    fn a_tree_cover_assembly_after_its_pieces_and_mids_in_idle_time_last() {
        // Region a: three tree cover z6 tiles of z3 tile 3/3/2 and its assembly; a member beside the
        // lead; and a z6 tile's mid made in idle time.
        let mut a = region("a", &[], &[], &[]);
        a.trees = vec!["6/28/16".into(), "6/28/17".into(), "6/29/16".into()];
        a.trees_lo = vec!["3/3/2".into()];
        let regions = [a];
        // (Each piece longer than a job's quarter hour: one a job.)
        let long = |step: &str, t: &str| Cost { secs: 1000.0, ..cost(step, t) };
        let mut inp = input(&regions, vec![mac("m4", 1.0), Machine { speed: 0.5, ..member("m1") }], &long);
        inp.idle = vec![("trees".into(), "6/40/20".into(), Cost { secs: 100.0, basis: Basis::Guess, peak_mb: 1000 })];
        let sim = run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far());
        let item = |step: &str, t: &str| sim.items.iter().find(|i| i.step == step && i.target == t).unwrap();
        // The assembly once its z6 tiles are made (the member takes some), by the lead alone.
        let lo = item("trees-lo", "3/3/2");
        assert!(["6/28/16", "6/28/17", "6/29/16"].iter().all(|t| item("trees", t).end <= lo.from + 1e-9));
        assert!(sim.items.iter().any(|i| i.step == "trees" && i.by == Some(1)), "the member's pieces");
        assert_eq!(lo.by, Some(0));
        // The mid last, after everything else.
        let idle = item("trees", "6/40/20");
        assert!(sim.items.iter().filter(|i| i.target != "6/40/20").all(|i| i.end <= idle.from + 1e-9));
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert_eq!((f.regions[0].left.get("trees"), f.regions[0].left.get("trees-lo")), (Some(&3), Some(&1)));
    }

    #[test]
    fn a_worldwide_job_running_alone_is_work_left() {
        // (The review's case.) The lead runs the pass's labels, an hour left; every region
        // built and on the map, no chain work.
        let regions = [RegionLeft { id: "a".into(), on_map: Some(true), ..Default::default() }];
        let busy = |inp: &mut Input| {
            inp.machines[0].busy_s = 3600.0;
            inp.after = Vec::new();
        };
        // Out of the jobs before the regions' (as the agent used to leave a running step): not
        // done, but when the Mac is free.
        let mut inp = input(&regions, vec![mac("m4", 1.0), Machine { speed: 0.5, ..member("m1") }], &cost);
        busy(&mut inp);
        inp.running.insert(("labels".into(), "labels".into()), 0);
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert_ne!(f.why.as_deref(), Some(NOTHING_LEFT));
        assert_eq!(f.done_at, Some(1_000_000 + 3600));
        // As the agent has it now, a running item of its own: on the schedule, and its step's.
        let mut inp = input(&regions, vec![mac("m4", 1.0), Machine { speed: 0.5, ..member("m1") }], &cost);
        busy(&mut inp);
        inp.before =
            vec![("labels".into(), "labels 2026-09-28".into(), Cost { secs: 2200.0, basis: Basis::Own, peak_mb: 0 })];
        inp.running.insert(("labels".into(), "labels 2026-09-28".into()), 0);
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert_eq!((f.why.as_deref(), f.done_at), (None, Some(1_000_000 + 3600)));
        assert_eq!(
            f.lanes["m4"].iter().map(|l| (l.step.as_str(), l.until - 1_000_000)).collect::<Vec<_>>(),
            [("labels", 3600)]
        );
        assert_eq!(f.steps.iter().map(|s| (s.step.as_str(), s.left)).collect::<Vec<_>>(), [("labels", 1)]);
        // A daily job (no item): not done until it ends either.
        let mut inp = input(&regions, vec![mac("m4", 1.0)], &cost);
        busy(&mut inp);
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert_eq!((f.why.as_deref(), f.done_at), (None, Some(1_000_000 + 3600)));
        // Free: nothing left.
        inp.machines[0].busy_s = 0.0;
        assert_eq!(forecast(crate::agent::seam::for_tests(), &inp).why.as_deref(), Some(NOTHING_LEFT));
    }

    #[test]
    fn a_page_around_takes_each_unit_the_moment_its_job_gives_it() {
        // As below, a worker that takes tails around: each of the two z6 tiles 30 s longer.
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &["3/1/1"])];
        let f = forecast(
            crate::agent::seam::for_tests(),
            &input(&regions, vec![Machine { unit_extra_s: 30.0, ..mac("m4", 1.0) }], &cost),
        );
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 1460));
    }

    #[test]
    fn one_mac_runs_a_region_and_its_map_tiles_then_it_is_on_the_map() {
        let regions = [RegionLeft {
            packs: vec![tile("6/8/8", &["6/8/8"]), tile("6/8/9", &["6/8/9"])],
            ..region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &["3/1/1"])
        }];
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0)], &cost));
        // Terrain 600, two z6 tiles 600, each z6 tile's map tile (100 s, measured per tile) drawn once
        // that z6 tile is built, before the next z6 tile (each tile waits for its own z6 tiles alone): on
        // the map a catalogue's minute after the last. Then its slope (200), then the landmarks (300).
        assert_eq!(f.regions[0].map_at, Some(1_000_000 + 1200 + 200 + 60));
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 1600));
        assert_eq!(f.done_at, Some(1_000_000 + 1900));
        let steps: Vec<(&str, usize, u64)> = f.steps.iter().map(|s| (s.step.as_str(), s.left, s.work_s)).collect();
        assert_eq!(
            steps,
            [("terrain-hi", 1, 600), ("unit", 2, 600), ("pack", 2, 200), ("slope-hi", 1, 200), ("marks", 1, 300)]
        );
        assert_eq!(f.regions[0].left.get("pack"), Some(&2));
        // The slope was guessed: its share of the time isn't measured.
        assert!((f.measured - 1700.0 / 1900.0).abs() < 0.01);
        let r = f.range.unwrap();
        assert!(r[0] < f.done_at.unwrap() && f.done_at.unwrap() < r[1]);
        assert_eq!(
            f.next["m4"].iter().map(|n| (n.step.as_str(), n.targets.len())).collect::<Vec<_>>(),
            [("terrain-hi", 1), ("unit", 1), ("pack", 1)]
        );
        // No lane of publishing's own: the map tiles are a step's run like any other.
        let lane: Vec<(&str, u64, u64, usize)> =
            f.lanes["m4"].iter().map(|l| (l.step.as_str(), l.from - 1_000_000, l.until - 1_000_000, l.n)).collect();
        assert_eq!(
            lane,
            [
                ("terrain-hi", 0, 600, 1),
                ("unit", 600, 900, 1),
                ("pack", 900, 1000, 1),
                ("unit", 1000, 1300, 1),
                ("pack", 1300, 1400, 1),
                ("slope-hi", 1400, 1600, 1),
                ("marks", 1600, 1900, 1)
            ]
        );
    }

    #[test]
    fn a_share_of_the_units_reading_stale_terrain_is_counted_as_work_to_come() {
        // One z6 tile stale, five more reading the stale terrain: three of them (0.6) counted too.
        let expected = ["6/9/8", "6/9/9", "6/10/8", "6/10/9", "6/11/8"].map(String::from).to_vec();
        let regions = [RegionLeft { expected, ..region("a", &["3/1/1"], &["6/8/8"], &[]) }];
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0)], &cost));
        let unit = f.steps.iter().find(|s| s.step == "unit").unwrap();
        assert_eq!((unit.left, unit.work_s), (4, 1200));
        // The region's ready once they're built too: terrain 600, four z6 tiles 1200.
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 1800));
        // None once the terrain's built (none expected).
        let regions = [region("a", &[], &["6/8/8"], &[])];
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0)], &cost));
        assert_eq!(f.steps.iter().find(|s| s.step == "unit").unwrap().left, 1);
    }

    #[test]
    fn a_regions_map_tiles_come_before_the_next_regions_work() {
        // a, b and c (a z6 tile of 300 s and a map tile of 100 s each): each region's tile drawn once
        // its z6 tile is built, before the next region's z6 tile; each on the map a minute after.
        let packs = |r: RegionLeft, t: &str| {
            let us: Vec<&str> = r.units.iter().map(String::as_str).collect();
            RegionLeft { packs: vec![tile(t, &us)], ..r }
        };
        let regions = [
            packs(region("a", &[], &["6/1/1"], &[]), "6/1/1"),
            packs(region("b", &[], &["6/2/2"], &[]), "6/2/2"),
            packs(region("c", &[], &["6/3/3"], &[]), "6/3/3"),
        ];
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0)], &cost));
        let on = |k: usize| f.regions[k].map_at.map(|t| t - 1_000_000);
        assert_eq!([on(0), on(1), on(2)], [Some(300 + 100 + 60), Some(800 + 60), Some(1200 + 60)]);
        assert_eq!(
            f.lanes["m4"].iter().map(|l| l.step.as_str()).collect::<Vec<_>>(),
            ["unit", "pack", "unit", "pack", "unit", "pack", "marks"]
        );
        // A map tile two regions draw: with the first, and the second waits for it.
        let regions =
            [packs(region("a", &[], &["6/1/1"], &[]), "6/1/1"), packs(region("b", &[], &["6/2/2"], &[]), "6/1/1")];
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0)], &cost));
        assert_eq!(f.steps.iter().find(|s| s.step == "pack").map(|s| s.left), Some(1));
        assert_eq!(f.regions[1].map_at, Some(1_000_000 + 700 + 60));
    }

    #[test]
    fn a_region_with_only_slope_or_tree_cover_left_has_no_time_to_the_map() {
        // (Task #259 review) its z6 tiles and map tiles done: on the map as it's built now already,
        // not "in a minute" each time the forecast is made; nor one on the map as it is.
        let regions =
            [region("a", &[], &[], &["3/1/1"]), RegionLeft { on_map: Some(true), ..region("b", &[], &["6/2/2"], &[]) }];
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0)], &cost));
        assert_eq!((f.regions[0].map_at, f.regions[1].map_at), (None, None));
        assert!(f.regions[0].ready_at.is_some());
    }

    /// Task #951 (seen 5 Nov): a slope tile reading the terrain's root while it's stale waits for it
    /// as the plan holds it (`Input::unknown`), the root a job of the lead's; the rest don't wait.
    #[test]
    fn a_slope_tile_reading_the_stale_root_waits_for_it() {
        let regions = [region("a", &[], &[], &["6/0/48", "6/1/48"])];
        let mut inp = input(&regions, vec![mac("m4", 1.0), member("m1")], &cost);
        let root = Cost { secs: 500.0, basis: Basis::Own, peak_mb: 100 };
        inp.unknown = vec![Unknown {
            step: "slope-hi".into(),
            targets: vec!["6/0/48".into()],
            until: ("terrain-root".into(), "terrain-root".into(), root),
        }];
        let sim = run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far());
        let of = |s: &str, t: &str| sim.items.iter().find(|i| i.step == s && i.target == t).unwrap();
        let (r, held, free) = (of("terrain-root", "terrain-root"), of("slope-hi", "6/0/48"), of("slope-hi", "6/1/48"));
        assert_eq!((r.by, r.from), (Some(0), 0.0), "the root, the lead's, at once");
        assert!(held.from >= r.end, "{} before the root's end {}", held.from, r.end);
        assert_eq!(free.from, 0.0);
    }

    /// A value's sustained best: the highest of its 30-minute rolling minimums over the 48 hours
    /// (the owner's), a gap breaking a run, a run shorter than the window counting for nothing.
    #[test]
    fn a_sustained_best_is_the_highest_rolling_minimum() {
        let now = 1_000_000;
        let minutes = |from: u64, n: u64, v: f64| (0..n).map(move |k| (from + k * 60, v));
        // (8 for 29 minutes, later 5 for 40, and 9 for an hour before the span.)
        let mut p: Vec<(u64, f64)> = minutes(now - 7200, 29, 8.0).chain(minutes(now - 3600, 40, 5.0)).collect();
        assert_eq!(sustained(&p, 60, now), Some(5.0), "29 minutes of 8 aren't a window");
        p.extend(minutes(now - 4 * 86_400, 60, 9.0));
        assert_eq!(sustained(&p, 60, now), Some(5.0), "past 48 h, not counted");
        // (A minute of 8 more, after a gap of one: still no run of 30.)
        p.push((now - 7200 + 30 * 60, 8.0));
        assert_eq!(sustained(&p, 60, now), Some(5.0));
        p.push((now - 7200 + 29 * 60, 7.0));
        assert_eq!(sustained(&p, 60, now), Some(7.0), "now 31 minutes on end, the least 7");
        assert_eq!(sustained(&p[..5], 60, now), None);
    }

    /// Task #951 review: a step whose measures are all of an older `cost_version` (slope-deep after
    /// #929 raised it) is cold target by target, one job of it at a time in the pool; its first job
    /// leaves its measures as it ends, as a live hand-off does, and the rest go beside each other.
    /// (Kept, the stale measures stood: the replays of 4-5 Nov ran all slope-deep on one lane.)
    #[test]
    fn a_step_measured_only_its_old_way_fans_out_after_its_first_job() {
        use crate::coord::sched::{Asked, State};
        use crate::coord::{Ask, Capacity};
        let ts: Vec<String> = (0..8).map(|i| format!("10/{i}/0")).collect();
        let regions = [RegionLeft { slope_deep: ts.clone(), ..region("a", &[], &[], &[]) }];
        let c = |_: &str, _: &str| Cost { secs: 600.0, basis: Basis::Own, peak_mb: 1000 };
        let v = crate::coord::cost_version("slope-deep");
        assert!(v > 0);
        let mut st = State::default();
        for t in &ts {
            st.costs.insert(
                cost_key("slope-deep", t),
                crate::coord::Cost { peak_mb: 1000, secs: 600, v: v - 1, ..Default::default() },
            );
        }
        st.workers = vec![Asked {
            ask: Ask {
                worker: "m1".into(),
                kind: "native".into(),
                mem_mb: 16_000,
                cores: 8,
                want: 4,
                cap: Some(Capacity { mem_mb: 16_000, build_mb: 16_000, cores: 8, cores_free: 8, ..Default::default() }),
                ..Default::default()
            },
            ..Default::default()
        }];
        let lane = |k: usize| Machine { mac: Some("m1".into()), ..member(&super::super::slot_worker("m1", k)) };
        let lead = Machine { busy_s: 1e6, ..mac("m4", 1.0) };
        let mut inp = input(&regions, vec![lead, lane(0), lane(1), lane(2), lane(3)], &c);
        inp.after.clear();
        inp.sched = Some(st);
        let sim = run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far());
        let mut spans: Vec<(f64, f64)> =
            sim.items.iter().filter(|i| i.step == "slope-deep").map(|i| (i.from, i.end)).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(spans.len(), 8);
        let first_end = spans[0].1;
        let beside = spans.iter().filter(|s| s.0 >= first_end - 1e-9 && s.0 < first_end + 1.0).count();
        assert!(beside > 1, "after the first job, its targets go beside each other: {spans:?}");
        assert!(sim.items.iter().all(|i| i.by.is_some()));
    }

    /// Task #993: a pacing window cut under the whole (`Limits::window`) is a reading of now: the
    /// run holds it while the jobs out now run, not for its whole length (code 6 held the gate
    /// before it so: two records of 5 Nov so shut ran every job that weighs anything one at a time,
    /// their depth a day late).
    #[test]
    fn a_cut_nas_window_opens_once_the_jobs_out_end() {
        use crate::coord::sched::{Asked, Leased, State};
        use crate::coord::{Ask, Capacity};
        let ts: Vec<String> = (0..8).map(|i| format!("10/{i}/0")).collect();
        let regions = [RegionLeft { slope_deep: ts, ..region("a", &[], &[], &[]) }];
        let c = |_: &str, _: &str| Cost { secs: 600.0, basis: Basis::Own, peak_mb: 1000 };
        // (Targets of 100 thousandths of the NAS each, a member's four lanes free, the lead's lane
        // busy, a member's job out for 900 s, the window cut to 150: one at a time while it runs.)
        let limits = jobrt::budget::nas::Limits { bytes_s: 100e6, creates_s: 100.0, window: 150, ..Default::default() };
        let v = crate::coord::cost_version("slope-deep");
        let mut st = State { nas: Some(limits), nas_own: Some(limits), lead_name: "m4".into(), ..Default::default() };
        for t in &regions[0].slope_deep {
            let sizes = [("threads".to_string(), 1), ("bytes".to_string(), 6_000_000_000)].into();
            st.costs.insert(
                cost_key("slope-deep", t),
                crate::coord::Cost { peak_mb: 1000, secs: 600, v, sizes, ..Default::default() },
            );
        }
        st.leases = vec![Leased {
            worker: "m2".into(),
            step: "terrain-hi".into(),
            targets: vec![("6/9/9".into(), "k".into())],
            threads: 1,
            granted_at: 1_000_000 - 7200,
            busy_tenths: None,
            left_s: None,
        }];
        let cap = Capacity { mem_mb: 16_000, build_mb: 16_000, cores: 8, cores_free: 8, ..Default::default() };
        st.workers = vec![Asked {
            ask: Ask {
                worker: "m1".into(),
                kind: "native".into(),
                mem_mb: 16_000,
                cores: 8,
                want: 4,
                cap: Some(cap),
                ..Default::default()
            },
            ..Default::default()
        }];
        let lane = |k: usize| Machine { mac: Some("m1".into()), ..member(&super::super::slot_worker("m1", k)) };
        let out =
            Machine { busy_s: 900.0, running: Some(("terrain-hi".into(), 1)), mac: Some("m2".into()), ..member("m2") };
        let lead = Machine { busy_s: 1e6, ..mac("m4", 1.0) };
        let mut inp = input(&regions, vec![lead, out, lane(0), lane(1), lane(2), lane(3)], &c);
        inp.after.clear();
        inp.sched = Some(st);
        let spans = |code: u32| {
            let inp = Input { code, ..inp.clone() };
            let sim = run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far());
            let mut v: Vec<(f64, f64)> =
                sim.items.iter().filter(|i| i.step == "slope-deep").map(|i| (i.from, i.end)).collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            v
        };
        let now = spans(VERSION);
        assert_eq!(now.len(), 8);
        let (during, after): (Vec<(f64, f64)>, Vec<(f64, f64)>) = now.iter().partition(|x| x.0 < 900.0);
        assert!(during.windows(2).all(|w| w[1].0 >= w[0].1 - 1e-9), "shut while the job out runs: {now:?}");
        assert!(after.windows(2).any(|w| w[1].0 < w[0].1), "then they go beside each other: {now:?}");
        let before = spans(6);
        assert!(before.windows(2).all(|w| w[1].0 >= w[0].1 - 1e-9), "code 6, one at a time: {before:?}");
    }

    /// Task #951: each Mac's room is its sustained best from now on (`Capacity::best`): a member
    /// whose last ask had the brake on takes work at its best, one whose best can't hold a target
    /// none of it. The 4 Nov case: the mini's memory fixed mid-day (#931/#935: its floor), its
    /// best room follows within the half hour, and its share of slope-deep with it, where the copy
    /// rule of #808 (gone) gave it none for a day.
    #[test]
    fn each_mac_runs_at_its_sustained_best_room() {
        use crate::coord::sched::{Asked, State};
        use crate::coord::{Ask, Capacity};
        let ts: Vec<String> = (0..6).map(|i| format!("10/{i}/0")).collect();
        let regions = [RegionLeft { slope_deep: ts.clone(), ..region("a", &[], &[], &[]) }];
        let c = |_: &str, _: &str| Cost { secs: 600.0, basis: Basis::Own, peak_mb: 3000 };
        let mut st = State::default();
        for t in &ts {
            let v = crate::coord::cost_version("slope-deep");
            st.costs.insert(
                cost_key("slope-deep", t),
                crate::coord::Cost { peak_mb: 3000, secs: 600, v, ..Default::default() },
            );
        }
        let asked = |best: Option<MacRoom>, brake: bool| Asked {
            ask: Ask {
                worker: "mini".into(),
                kind: "native".into(),
                mem_mb: 8000,
                cores: 8,
                cap: Some(Capacity {
                    limit_mb: 14_000,
                    mem_mb: 8000,
                    build_mb: 8000,
                    cores: 8,
                    cores_free: 8,
                    mem_held_back: brake.then(|| "4.0 GB of memory available, under the 6.0 GB floor".into()),
                    best,
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let lead = Machine { busy_s: 1e6, ..mac("m4", 1.0) };
        let mini = Machine { mac: Some("mini".into()), ..member("mini") };
        let mut inp = input(&regions, vec![lead, mini], &c);
        inp.after.clear();
        let mut on_mini = |a: Asked| {
            st.workers = vec![a];
            inp.sched = Some(st.clone());
            run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far())
                .items
                .iter()
                .filter(|i| i.by == Some(1))
                .count()
        };
        assert_eq!(on_mini(asked(None, true)), 0, "the brake on as it asked, no best said");
        assert_eq!(on_mini(asked(Some(MacRoom { mem_mb: 8000, cores: 8 }), true)), 6, "at its best");
        assert_eq!(on_mini(asked(Some(MacRoom { mem_mb: 2000, cores: 8 }), false)), 0, "its best holds no target");
        // The 4 Nov case, its minutes as its live store samples them: 16 GB, the rest of the Mac
        // using 8, its floor 6 GB (2 GB for jobs) until noon, then 2 GB (6 GB for jobs).
        let noon = 2_000_000 / 86_400 * 86_400 + 43_200;
        let store = super::super::live::Store::new(None, "mini");
        for t in (noon - 86_400..noon + 3600).step_by(60) {
            store.record(super::super::live::OTHERS_MEM, t, 8.0);
            store.record(super::super::live::MEM_FLOOR, t, if t < noon { 6.0 } else { 2.0 });
            store.record(super::super::live::OTHERS_CORES, t, 1.5);
            store.record(super::super::live::BUILD_CORES_MAX, t, 8.0);
        }
        let best = |at: u64| super::super::live::Store::best(&store, at, 16_384, 10);
        assert_eq!(best(noon + 20 * 60), Some(MacRoom { mem_mb: 2048, cores: 8 }), "20 minutes on: as before");
        assert_eq!(on_mini(asked(best(noon + 20 * 60), false)), 0);
        let fixed = best(noon + 31 * 60);
        assert_eq!(fixed, Some(MacRoom { mem_mb: 6144, cores: 8 }), "half an hour on: fixed");
        assert_eq!(on_mini(asked(fixed, false)), 6);
    }

    #[test]
    fn the_cadenced_publishes_go_by_their_cadence_or_once_nothing_is_left_to_build() {
        // Thirty z6 tiles (9,000 s) left. The low zooms last made an hour ago (due in two more), the
        // landmarks ten minutes ago (due in fifty), the overlays never seen run (due now).
        let units: Vec<String> = (0..30).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0)], &cost);
        inp.cadence = Cadence { since_low: Some(3600), since_marks: Some(600), since_overlays: None };
        let at = |f: &Forecast| f.publishes.iter().map(|p| (p.what.clone(), p.at - 1_000_000)).collect::<Vec<_>>();
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert_eq!(
            at(&f),
            [("low".into(), LOW_EVERY_S - 3600), ("marks".into(), MARKS_EVERY_S - 600), ("overlays".into(), 0)]
        );
        // Two z6 tiles left (600 s): each once they're built, if sooner.
        let regions = [region("a", &[], &refs[..2], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0)], &cost);
        inp.cadence = Cadence { since_low: Some(3600), since_marks: Some(600), since_overlays: Some(0) };
        assert_eq!(
            at(&forecast(crate::agent::seam::for_tests(), &inp)),
            [("low".into(), 600), ("marks".into(), 600), ("overlays".into(), 600)]
        );
    }

    #[test]
    fn regions_are_built_one_at_a_time() {
        let regions = [region("a", &["3/1/1"], &["6/8/8"], &[]), region("b", &["3/2/2"], &["6/16/16"], &[])];
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0)], &cost));
        // a's terrain and z6 tile, then b's terrain and z6 tile: a done first.
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 900));
        assert_eq!(f.regions[1].ready_at, Some(1_000_000 + 900 + 900));
        assert_eq!(f.measured, 1.0);
    }

    #[test]
    fn a_region_whose_terrain_a_member_builds_waits_for_it() {
        // One region: the M1 has its terrain now (900 s left); the lead waits for it.
        let regions = [region("a", &["6/8/8"], &["6/8/8"], &[])];
        let mut inp = input(
            &regions,
            vec![mac("m4", 1.0), Machine { busy_s: 900.0, ..Machine { speed: 0.5, ..member("m1") } }],
            &cost,
        );
        inp.running.insert(("terrain-hi".into(), "6/8/8".into()), 1);
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 900 + 300));
    }

    #[test]
    fn the_runs_say_their_targets_and_what_each_waits_on() {
        // (Task #787.) One Mac, one region: its terrain, then its two z6 tiles (they wait on the
        // region's terrain, a set: `Items::groups`), after the reach they wait on too (its own dep).
        let regions = [region("a", &["6/8/8"], &["6/8/8", "6/8/9"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0)], &cost);
        inp.before = vec![("reach".into(), "reach".into(), Cost { secs: 600.0, basis: Basis::Own, peak_mb: 0 })];
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        let steps: Vec<&str> = f.lanes["m4"].iter().map(|l| l.step.as_str()).collect();
        let at = |s: &str| steps.iter().position(|&x| x == s).unwrap() as u32;
        assert_eq!(f.runs.targets.len(), steps.len(), "{steps:?}");
        // (Each z6 tile a job of its own, as the scheduler grants it: task #834.)
        assert_eq!(f.runs.targets[at("unit") as usize], ["6/8/8", "6/8/9"]);
        assert_eq!(f.lanes["m4"][at("unit") as usize].cuts.len(), 1);
        // (Job to job, as the page draws them: each z6 tile's job on the terrain's and the reach's.)
        let job = |run: u32| f.runs.jobs[..run as usize].iter().sum::<u32>();
        assert_eq!(f.runs.jobs[at("unit") as usize], 2);
        let (t, r, u) = (job(at("terrain-hi")), job(at("reach")), job(at("unit")));
        for d in [[t, u], [r, u], [t, u + 1], [r, u + 1]] {
            assert!(f.runs.deps.contains(&d), "{d:?}: {:?} {steps:?}", f.runs.deps);
        }
        assert!(!f.runs.deps.contains(&[u, u + 1]), "the z6 tiles don't wait on each other");
        assert!(f.runs.deps.iter().all(|[a, b]| a != b), "never a job on itself");
        // Two Macs: the runs numbered by lane name, so a run on "m1" comes before "m4"'s.
        let mut two = input(&regions, vec![mac("m4", 1.0), member("m1")], &cost);
        two.before = inp.before.clone();
        let f = forecast(crate::agent::seam::for_tests(), &two);
        let n1 = f.lanes["m1"].len();
        assert_eq!(f.runs.targets.len(), n1 + f.lanes["m4"].len());
        let tc = ToCome { runs: f.runs.clone(), ..ToCome::default() }.answer();
        assert_eq!(tc["runs"].as_array().unwrap().len(), f.runs.targets.len());
        assert_eq!(tc["deps"].as_array().unwrap().len(), f.runs.deps.len());
        assert_eq!(tc["jobs"].as_array().unwrap().len(), f.runs.targets.len());
    }

    #[test]
    fn nothing_left_is_no_finish_and_a_rebuilt_region_goes_on_the_map_again() {
        let none: [RegionLeft; 0] = [];
        let mut inp = input(&none, vec![mac("m4", 1.0)], &cost);
        inp.after = Vec::new();
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert_eq!((f.done_at, f.why.as_deref()), (None, Some("nothing left to build")));
        assert!(f.regions.is_empty());
        // A region on the map as it was (its outline changed): on the map as it is a minute after
        // its z6 tile; one the map has as it is, rebuilt z6 tiles or none left, no time.
        let regions = [
            RegionLeft { on_map: Some(false), ..region("a", &[], &["6/8/8"], &[]) },
            RegionLeft { on_map: Some(true), ..region("b", &[], &["6/9/8"], &[]) },
            RegionLeft { id: "c".into(), on_map: Some(true), ..Default::default() },
        ];
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0)], &cost));
        let at = |k: usize| f.regions[k].map_at;
        assert_eq!((at(0), at(1), at(2)), (Some(1_000_000 + 300 + 60), None, None));
        // Work that can't all be listed: no finish, and why.
        let mut inp = input(&regions, vec![mac("m4", 1.0)], &cost);
        inp.blind = Some("the z6 tiles wait for the pass's reaches".into());
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        assert_eq!((f.done_at, f.why.as_deref()), (None, Some("the z6 tiles wait for the pass's reaches")));
    }

    /// A member's slot as the coordinator packs it: any step any Mac may run that fits it.
    fn member(name: &str) -> Machine {
        Machine { lead_steps: false, ..mac(name, 1.0) }
    }

    /// A lane takes an item at its Mac's time against the lead's (the scheduler's paces, by the
    /// Macs' speeds), its slots alike; a Mac the paces don't know, at the lead's; with no paces
    /// worked out, at its speed.
    #[test]
    fn a_lane_runs_at_its_macs_time() {
        let regions = [region("a", &[], &["u1"], &[])];
        let m1 = Machine { mac: Some("m1".into()), ..mac("m1 #2", 0.5) };
        let m2 = mac("m2", 0.25);
        let mut inp = input(&regions, vec![mac("m4", 1.0), m1.clone(), m2.clone()], &cost);
        assert_eq!(inp.lane_secs(&m1, 100.0), 200.0, "no paces: its speed");
        inp.sched = Some(State {
            paces: crate::paces::Paces {
                lead: "m4".into(),
                macs: [("m1".to_string(), 1.5)].into(),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(inp.lane_secs(&m1, 100.0), 150.0, "its Mac's time");
        assert_eq!(inp.lane_secs(&inp.machines[0].clone(), 100.0), 100.0, "the lead's");
        assert_eq!(inp.lane_secs(&m2, 100.0), 100.0, "not measured: the lead's, as everywhere");
    }

    /// Task #1413: a scheduler's state (the lead's, `Input::sched`) with the targets' runs on the lead
    /// and the M1 at twice its time: each item the M1 runs takes its needs over the M1's supply
    /// (`needs_secs`: its run at the M1's speed, 1,200 s), not its cost at the lane's pace; the time
    /// its NAS moves take at the NAS budget where that's longer (`nas_secs`); its step's job start on
    /// the first of a job. A target its step's model can't say (none of it measured) runs
    /// at its cost.
    #[test]
    fn a_jobs_time_is_its_needs_over_its_macs_supply() {
        let ts = ["6/1/1", "6/1/2", "6/1/3"];
        let regions = [region("a", &ts, &[], &[])];
        let c = |_: &str, _: &str| Cost { secs: 50.0, basis: Basis::Own, peak_mb: 1000 };
        let state = |bytes: u64, start: f64| {
            let mut st = State {
                lead_name: "m4".into(),
                paces: crate::paces::Paces {
                    lead: "m4".into(),
                    macs: [("m4".to_string(), 1.0), ("m1".to_string(), 2.0)].into(),
                    starts: [("m1".to_string(), [("terrain-hi".to_string(), start)].into())].into(),
                    ..Default::default()
                },
                nas: Some(jobrt::budget::nas::Limits { bytes_s: 100e6, ..Default::default() }),
                ..Default::default()
            };
            let v = crate::coord::cost_version("terrain-hi");
            for t in ts {
                let sizes = [("bytes".to_string(), bytes)].into();
                let run = crate::coord::Cost {
                    peak_mb: 1000,
                    secs: 600,
                    worker: Some("m4".into()),
                    v,
                    sizes,
                    ..Default::default()
                };
                st.costs.insert(cost_key("terrain-hi", t), run);
            }
            st
        };
        let lead = Machine { busy_s: 1e6, ..mac("m4", 1.0) };
        let times = |st: Option<State>| {
            let mut inp = input(&regions, vec![lead.clone(), member("m1")], &c);
            inp.after.clear();
            inp.sched = st;
            let f = forecast_by(crate::agent::seam::for_tests(), &inp, far());
            let mut v: Vec<(u64, u64)> =
                f.placed.iter().filter(|p| p.step == "terrain-hi").map(|p| (p.from, p.until - p.from)).collect();
            v.sort();
            v.into_iter().map(|x| x.1).collect::<Vec<_>>()
        };
        assert_eq!(times(Some(state(0, 0.0))), [1200, 1200, 1200], "its run at the M1's speed");
        // (90 GB takes the whole NAS, 100 MB/s, 900 s: within the M1's 1,200 s; 180 GB, 1,800 s.)
        assert_eq!(times(Some(state(90_000_000_000, 0.0))), [1200, 1200, 1200], "within the NAS on the M1");
        assert_eq!(times(Some(state(180_000_000_000, 0.0))), [1800, 1800, 1800], "past it: at the NAS's pace");
        // (Each its own job, past the batch length: each pays its job's start.)
        assert_eq!(times(Some(state(0, 60.0))), [1260, 1260, 1260], "its job's start on the first");
        assert_eq!(times(None), [50, 50, 50], "no scheduler: its cost at its speed");
    }

    /// Task #1413: a lane's job under way is timed by its needs over its Mac's supply
    /// (`lease_busy`): its targets on its Mac at the threads it holds, less how long it has run; a
    /// minute at least; a job of a step none of whose runs is of the way it runs now keeps its
    /// lane's time left as its slot said it.
    #[test]
    fn a_leased_jobs_time_left_is_its_needs_less_its_run() {
        use crate::coord::sched::Leased;
        let regions = [region("a", &["6/1/1", "6/1/2"], &[], &[])];
        let v = crate::coord::cost_version("terrain-hi");
        let mut st = State {
            lead_name: "m4".into(),
            paces: crate::paces::Paces {
                lead: "m4".into(),
                macs: [("m4".to_string(), 1.0), ("m1".to_string(), 2.0)].into(),
                ..Default::default()
            },
            ..Default::default()
        };
        for t in ["6/1/1", "6/1/2"] {
            let run =
                crate::coord::Cost { peak_mb: 1000, secs: 600, worker: Some("m4".into()), v, ..Default::default() };
            st.costs.insert(cost_key("terrain-hi", t), run);
        }
        let leased = |worker: &str, step: &str, ts: &[&str], ran: u64| Leased {
            worker: worker.into(),
            step: step.into(),
            targets: ts.iter().map(|t| (t.to_string(), "k".to_string())).collect(),
            threads: 1,
            granted_at: 1_000_000 - ran,
            busy_tenths: None,
            left_s: None,
        };
        st.leases = vec![
            leased("m1", "terrain-hi", &["6/1/1", "6/1/2"], 300),
            leased("m2", "slope-hi", &["6/9/9"], 300),
            leased("m3", "terrain-hi", &["6/1/1"], 5000),
        ];
        let lane =
            |name: &str, step: &str| Machine { busy_s: 99_999.0, running: Some((step.to_string(), 1)), ..member(name) };
        let mut inp = input(
            &regions,
            vec![mac("m4", 1.0), lane("m1", "terrain-hi"), lane("m2", "slope-hi"), lane("m3", "terrain-hi")],
            &cost,
        );
        inp.sched = Some(st);
        lease_busy(crate::agent::seam::for_tests(), &mut inp);
        let busy: Vec<f64> = inp.machines.iter().map(|m| m.busy_s).collect();
        // (The M1's two at 1,200 s each less its 300 s run; slope-hi never measured; the third Mac's
        // speed not known: twice the lead's 600 s, run past it.)
        assert_eq!(busy, [0.0, 2100.0, 99_999.0, 60.0]);
    }

    /// Task #1413 review: a job under way whose targets no lane runs is timed on a lane of its
    /// worker's Mac running its step, a slot's worker ("m1 #2") its Mac's, each lane timed by one
    /// job alone: two jobs on one Mac time its two lanes.
    #[test]
    fn two_jobs_under_way_on_one_mac_time_its_two_lanes() {
        use crate::coord::sched::Leased;
        let regions = [region("a", &["6/1/1"], &[], &[])];
        let v = crate::coord::cost_version("terrain-hi");
        let mut st = State {
            lead_name: "m4".into(),
            paces: crate::paces::Paces {
                lead: "m4".into(),
                macs: [("m4".to_string(), 1.0), ("m1".to_string(), 2.0)].into(),
                ..Default::default()
            },
            ..Default::default()
        };
        for t in ["6/9/1", "6/9/2"] {
            let run =
                crate::coord::Cost { peak_mb: 1000, secs: 600, worker: Some("m4".into()), v, ..Default::default() };
            st.costs.insert(cost_key("terrain-hi", t), run);
        }
        let leased = |worker: &str, t: &str, ran: u64| Leased {
            worker: worker.into(),
            step: "terrain-hi".into(),
            targets: vec![(t.to_string(), "k".to_string())],
            threads: 1,
            granted_at: 1_000_000 - ran,
            busy_tenths: None,
            left_s: None,
        };
        st.leases = vec![leased("m1 #2", "6/9/1", 300), leased("m1", "6/9/2", 600)];
        let lane = |name: &str| Machine {
            busy_s: 99_999.0,
            mac: Some("m1".into()),
            running: Some(("terrain-hi".to_string(), 1)),
            ..member(name)
        };
        let mut inp = input(&regions, vec![mac("m4", 1.0), lane("m1"), lane("m1 #2")], &cost);
        inp.sched = Some(st);
        lease_busy(crate::agent::seam::for_tests(), &mut inp);
        let mut busy: Vec<f64> = inp.machines[1..].iter().map(|m| m.busy_s).collect();
        busy.sort_by(f64::total_cmp);
        // (Each 1,200 s on the M1, less its 600 s and 300 s run.)
        assert_eq!(busy, [600.0, 900.0]);
    }

    /// Task #1413: a step's pace against its needs is its jobs' time over what their grants were
    /// sized to, over the last 12 h's jobs done well, three at least; only a step quicker than its
    /// needs is said, and never under a quarter.
    #[test]
    fn a_steps_pace_against_its_needs_is_its_quicker_jobs_against_their_sizing() {
        use crate::coord::history::Event;
        let now = 1_000_000;
        let mut events = Vec::new();
        let mut n = 0;
        let mut job = |step: &str, t: u64, sized: f64, took: f64, ok: bool| {
            n += 1;
            let id = Some(jobrt::jobid::JobId { epoch: 1, n });
            events.push(Event {
                kind: "lease".into(),
                t: t - took as u64,
                job: id,
                data: Some(serde_json::json!({ "sized_s": sized })),
                ..Default::default()
            });
            let (step, secs, ok) = (Some(step.into()), Some(took), Some(ok));
            events.push(Event { kind: "done".into(), t, job: id, step, secs, ok, ..Default::default() });
        };
        for _ in 0..3 {
            job("terrain-hi", now - 600, 600.0, 400.0, true);
            job("slope-deep", now - 600, 100.0, 900.0, true);
            job("unit", now - 600, 1000.0, 10.0, true);
        }
        job("terrain-hi", now - 600, 600.0, 6000.0, false);
        job("terrain-hi", now - 13 * 3600, 600.0, 6000.0, true);
        job("slope-hi", now - 600, 600.0, 60.0, true);
        job("slope-hi", now - 600, 600.0, 60.0, true);
        let pace = needs_pace(&events, now);
        let want: BTreeMap<String, f64> = [("terrain-hi".to_string(), 0.67), ("unit".to_string(), 0.25)].into();
        assert_eq!(pace, want);
    }

    #[test]
    fn a_members_slots_take_any_work_offered_to_it_but_the_leads_own() {
        // (Task #383) the lead's two slots and a member's two: the member's take z6 tiles (no
        // far-end rule), never the landmark points (the lead's own step).
        let units: Vec<String> = (0..8).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let lead2 = Machine { lead_steps: true, ..mac("m4 #2", 1.0) };
        let inp = input(&regions, vec![mac("m4", 1.0), lead2, member("m1"), member("m1 #2")], &cost);
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        let n = |lane: &str, step: &str| f.lanes[lane].iter().filter(|l| l.step == step).map(|l| l.n).sum::<usize>();
        assert_eq!(n("m1", "unit") + n("m1 #2", "unit"), 4, "{:?}", f.lanes);
        assert_eq!(n("m1", "marks") + n("m1 #2", "marks"), 0);
        assert_eq!(n("m4", "marks") + n("m4 #2", "marks"), 1);
    }

    #[test]
    fn two_macs_finish_in_about_half_the_time_of_one() {
        // (Task #384) forty z6 tiles: one Mac's two slots, then two Macs' two each, all as fast.
        let units: Vec<String> = (0..40).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let lead2 = || Machine { lead_steps: true, ..mac("m4 #2", 1.0) };
        let mut one = input(&regions, vec![mac("m4", 1.0), lead2()], &cost);
        one.after = Vec::new();
        let mut two = input(&regions, vec![mac("m4", 1.0), lead2(), member("m1"), member("m1 #2")], &cost);
        two.after = Vec::new();
        let left = |i: &Input| (forecast(crate::agent::seam::for_tests(), i).done_at.unwrap() - 1_000_000) as f64;
        let (a, b) = (left(&one), left(&two));
        assert_eq!(a, 20.0 * 300.0);
        assert!((b / a - 0.5).abs() < 0.05, "{b} of {a}");
    }

    #[test]
    fn a_lane_starts_with_its_slots_running_job_then_the_plans_work_of_every_kind() {
        // (Task #385) the lead's second slot runs a depth job no item is (40 min left). Its lane
        // says the depth job first; a chain's step (the candidates) comes in the plan's order,
        // from the front, as the coordinator offers it (pick_every), once the z6 tiles ahead of it are
        // taken.
        let units: Vec<String> = (0..6).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let lead2 =
            Machine { lead_steps: true, busy_s: 2400.0, running: Some(("depth".into(), 3)), ..mac("m4 #2", 1.0) };
        let mut inp = input(&regions, vec![mac("m4", 1.0), lead2, member("m1")], &cost);
        inp.chains = vec![("pois".into(), "6/8/0".into(), Cost { secs: 100.0, basis: Basis::Own, peak_mb: 1000 })];
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        let first = &f.lanes["m4 #2"][0];
        assert_eq!((first.step.as_str(), first.from, first.until, first.n), ("depth", 1_000_000, 1_002_400, 3));
        let pois = f.lanes.values().flatten().find(|l| l.step == "pois").unwrap();
        assert_eq!(pois.from, 1_000_900, "after the three units each lane took first: {:?}", f.lanes);
    }

    #[test]
    fn a_macs_slots_share_its_room_as_the_coordinator_packs() {
        // (Task #383 review) a member of 8 cores (jobs packed at a thread each, task #896: their runs
        // said no threads) and 8 GB, four slots;
        // z6 tiles of 3 GB: two at once by memory, not four.
        let units: Vec<String> = (0..8).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let big = |step: &str, t: &str| Cost { peak_mb: 3000, ..cost(step, t) };
        let slot =
            |k: usize| Machine { mac: Some("m1".into()), mem_mb: 8000, ..member(&super::super::slot_worker("m1", k)) };
        let lead = Machine { busy_s: 1e6, ..mac("m4", 1.0) };
        let mut inp = input(&regions, vec![lead, slot(0), slot(1), slot(2), slot(3)], &big);
        inp.after = Vec::new();
        inp.macs.insert("m1".into(), MacRoom { mem_mb: 8000, cores: 8 });
        let sim = run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far());
        let most = |sim: &Sim| {
            (0..40)
                .map(|k| k as f64 * 100.0 + 1.0)
                .map(|t| sim.items.iter().filter(|i| i.by.is_some_and(|m| m > 0) && i.from <= t && i.end > t).count())
                .max()
                .unwrap()
        };
        assert_eq!(most(&sim), 2, "8 GB of 3 GB jobs");
        // By cores: 1 GB jobs at a thread each, four at once (its slots) on 8 cores; on 2 cores, two.
        let small = |step: &str, t: &str| Cost { peak_mb: 1000, ..cost(step, t) };
        inp.costs = cost_table(&regions, &small);
        assert_eq!(most(&run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far())), 4);
        inp.macs.insert("m1".into(), MacRoom { mem_mb: 8000, cores: 2 });
        assert_eq!(most(&run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far())), 2);
        // A step never measured: its first job alone on its Mac, nothing beside it (task #499);
        // once it ends its measure packs the rest as above.
        let cold = |step: &str, t: &str| Cost { basis: Basis::Guess, ..small(step, t) };
        inp.costs = cost_table(&regions, &cold);
        let sim = run(crate::agent::seam::for_tests(), &inp, &|c| c.secs, far());
        let first =
            sim.items.iter().filter(|i| i.by.is_some_and(|m| m > 0)).min_by(|a, b| a.from.total_cmp(&b.from)).unwrap();
        assert!(sim
            .items
            .iter()
            .filter(|i| i.by.is_some_and(|m| m > 0))
            .all(|i| i.from >= first.end - 1e-9 || std::ptr::eq(i, first)));
        assert_eq!(most(&sim), 2, "on its 2 cores, as above");
    }

    #[test]
    fn a_slot_the_ramp_grows_to_opens_later() {
        // (Task #383 review) the lead's third slot opens in an hour: nothing on it before.
        let units: Vec<String> = (0..40).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let lead2 = Machine { lead_steps: true, ..mac("m4 #2", 1.0) };
        let lead3 = Machine { lead_steps: true, from_s: 3600.0, ..mac("m4 #3", 1.0) };
        let f = forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0), lead2, lead3], &cost));
        assert_eq!(f.lanes["m4 #3"].first().map(|l| l.from), Some(1_000_000 + 3600), "{:?}", f.lanes);
    }

    #[test]
    fn three_thousand_units_forecast_in_well_under_a_second() {
        // (Task #384 review) 3,000 z6 tiles in 30 regions, each z6 tile's map tile, two Macs of four
        // slots packed by room (`MacRoom`): the whole forecast (three runs) well under a second.
        let regions: Vec<RegionLeft> = (0..30)
            .map(|r| {
                let units: Vec<String> = (0..100).map(|i| format!("6/{r}/{i}")).collect();
                let refs: Vec<&str> = units.iter().map(String::as_str).collect();
                RegionLeft {
                    packs: units.iter().map(|u| (u.clone(), vec![u.clone()])).collect(),
                    ..region(&format!("r{r}"), &[], &refs, &[])
                }
            })
            .collect();
        let slot = |host: &str, k: usize, lead: bool| Machine {
            lead_steps: lead,
            mac: Some(host.into()),
            mem_mb: 16_000,
            ..mac(&super::super::slot_worker(host, k), 1.0)
        };
        let machines: Vec<Machine> =
            (0..4).map(|k| slot("m4", k, true)).chain((0..4).map(|k| slot("m1", k, false))).collect();
        let mut inp = input(&regions, machines, &cost);
        inp.macs = [
            ("m4".to_string(), MacRoom { mem_mb: 16_000, cores: 14 }),
            ("m1".to_string(), MacRoom { mem_mb: 8_000, cores: 8 }),
        ]
        .into();
        let took = super::thread_cpu().as_millis();
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        let ms = super::thread_cpu().as_millis() - took;
        eprintln!("the forecast took {ms} ms (3,000 units, 3,000 map tiles, 8 lanes)");
        assert!(f.done_at.is_some() && f.regions.iter().all(|r| r.map_at.is_some()));
        assert!(ms < CPU_BOUND_MS, "{ms} ms of CPU");
    }

    /// Inputs for the run's tests at scale: `units` z6 tiles in regions of 100, each its map tile, two
    /// Macs of four slots packed by room; each z6 tile's cost by `cost_of(k)`.
    fn many(units: usize) -> Vec<RegionLeft> {
        (0..units.div_ceil(100))
            .map(|r| {
                let us: Vec<String> = (0..100.min(units - r * 100)).map(|i| format!("6/{r}/{i}")).collect();
                let refs: Vec<&str> = us.iter().map(String::as_str).collect();
                RegionLeft {
                    packs: us.iter().map(|u| (u.clone(), vec![u.clone()])).collect(),
                    ..region(&format!("r{r}"), &[], &refs, &[])
                }
            })
            .collect()
    }

    fn two_macs<'a>(regions: &'a [RegionLeft], c: &'a dyn Fn(&str, &str) -> Cost) -> Input<'a> {
        let slot = |host: &str, k: usize, lead: bool| Machine {
            lead_steps: lead,
            mac: Some(host.into()),
            mem_mb: 16_000,
            ..mac(&super::super::slot_worker(host, k), 1.0)
        };
        let mut machines: Vec<Machine> =
            (0..4).map(|k| slot("m4", k, true)).chain((0..4).map(|k| slot("m1", k, false))).collect();
        // (A member slot busy with a job no item is, for 20 min.)
        machines[5].busy_s = 1200.0;
        machines[5].running = Some(("depth".into(), 1));
        let mut inp = input(regions, machines, c);
        inp.macs = [
            ("m4".to_string(), MacRoom { mem_mb: 16_000, cores: 14 }),
            ("m1".to_string(), MacRoom { mem_mb: 8_000, cores: 8 }),
        ]
        .into();
        inp
    }

    /// Z6 tile `t`'s kind for the tests at scale: every third cold, every fifth big (6 GB), the rest
    /// measured and small; `all_cold`: every one cold.
    fn mixed(all_cold: bool) -> impl Fn(&str, &str) -> Cost {
        move |step: &str, t: &str| {
            let k: usize = t.rsplit('/').next().and_then(|x| x.parse().ok()).unwrap_or(0);
            let c = cost(step, t);
            if step != "unit" {
                return c;
            }
            Cost {
                basis: if all_cold || k % 3 == 0 { Basis::Guess } else { Basis::Own },
                peak_mb: if k % 5 == 0 { 6000 } else { 1500 },
                ..c
            }
        }
    }

    #[test]
    fn three_thousand_cold_or_big_units_forecast_in_well_under_a_second() {
        // (Task #384 review) the worst cases: every z6 tile cold (each alone on its Mac), and cold and
        // big mixed.
        let regions = many(3000);
        for all_cold in [true, false] {
            let c = mixed(all_cold);
            let inp = two_macs(&regions, &c);
            let took = super::thread_cpu().as_millis();
            let f = forecast(crate::agent::seam::for_tests(), &inp);
            let ms = super::thread_cpu().as_millis() - took;
            eprintln!(
                "the forecast took {ms} ms (3,000 units, {})",
                if all_cold { "all cold" } else { "cold and big mixed" }
            );
            assert!(f.done_at.is_some());
            assert!(ms < CPU_BOUND_MS, "{ms} ms of CPU");
        }
    }

    #[test]
    fn the_leads_other_slots_build_units_beside_the_first() {
        let units: Vec<String> = (0..8).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let others = Machine { mem_mb: 12_000, ..mac("m4 #2", 1.0) };
        // Half the z6 tiles, whoever's at the Mac (the owner's choice: no rule for a Mac in use).
        let f =
            forecast(crate::agent::seam::for_tests(), &input(&regions, vec![mac("m4", 1.0), others.clone()], &cost));
        assert_eq!(
            f.lanes.get("m4 #2").map_or(0, |l| l.iter().filter(|x| x.step == "unit").map(|x| x.n).sum::<usize>()),
            4
        );
        // The lead's first job two hours from done: its other slots take the z6 tiles from now.
        let f = forecast(
            crate::agent::seam::for_tests(),
            &input(&regions, vec![Machine { busy_s: 7200.0, ..mac("m4", 1.0) }, others], &cost),
        );
        let lane = &f.lanes["m4 #2"];
        assert_eq!(
            (lane[0].step.as_str(), lane[0].from, lane.iter().filter(|x| x.step == "unit").map(|x| x.n).sum::<usize>()),
            ("unit", 1_000_000, 8)
        );
    }

    /// The guard of task #535: the forecast, fed the coordinator's state as recorded
    /// (`Coordinator::sched_state`, as written with the lead's forecast), makes the same first grants
    /// the coordinator then makes: the lead's and a member's, by memory, cores and the NAS budget
    /// (each z6 tile weighing 400 of its 1000: two at once, whatever their room says), each with the
    /// threads the coordinator grants it to start on (its targets' runs', 1).
    #[test]
    fn fed_a_recorded_state_the_forecast_grants_what_the_coordinator_grants() {
        use crate::coord::{self, client, cost_version};
        let d = tempfile::tempdir().unwrap();
        let (c, port) = coord::start_for_test(&d.path().join("coord"), "m4", "");
        let m1 = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        c.set_nas(Some(jobrt::budget::nas::Limits { bytes_s: 100e6, creates_s: 100.0, ..Default::default() }));
        let units: Vec<String> = (0..12).map(|i| format!("6/8/{i}")).collect();
        for (i, u) in units.iter().enumerate() {
            // (40 MB/s of the NAS's 100: 400 of its weight; every other one 3 GB, the rest 1 GB.)
            let sizes = [("threads".to_string(), 1), ("bytes".to_string(), 40 << 20 << 10)].into();
            let peak_mb = if i % 2 == 0 { 3000 } else { 1000 };
            let cost = crate::coord::Cost { peak_mb, secs: 1000, v: cost_version("unit"), sizes, ..Default::default() };
            c.add_costs_by(&[(crate::coord::cost_key("unit", u), cost)], "m1");
        }
        let ask = |mem_mb: u64, cores: u32, want: usize| crate::coord::Ask {
            kind: "native".into(),
            can: vec!["unit".into()],
            mem_mb,
            cores,
            disk_mb: Some(100_000),
            cap: Some(crate::coord::Capacity { cores, cores_free: cores, ..Default::default() }),
            want,
            ..Default::default()
        };
        // (The member's ask before there's work: recorded, nothing granted.)
        assert!(m1.ask(&ask(8000, 8, 4)).unwrap().is_none());
        let offered = || {
            vec![crate::coord::Offer {
                step: "unit".into(),
                targets: units.iter().map(|u| (u.clone(), super::KEY.to_string(), 0)).collect(),
                job: None,
            }]
        };
        c.offer("p", offered());
        c.set_capacity(ask(16_000, 14, 4).cap);
        // The state as data.
        let st: State = serde_json::from_str(&serde_json::to_string(&c.sched_state()).unwrap()).unwrap();
        let mut real: Vec<(String, Vec<String>, u32)> = Vec::new();
        let mut take = |g: Option<crate::coord::Grant>, who: &str| {
            for g in g.into_iter().flat_map(|mut g| {
                let more = std::mem::take(&mut g.more);
                std::iter::once(g).chain(more)
            }) {
                if let crate::coord::Granted::Job { targets, .. } = g.work {
                    real.push((who.to_string(), targets.into_iter().map(|t| t.0).collect(), g.threads));
                }
            }
        };
        take(c.ask_own(&ask(16_000, 14, 4), &Default::default(), 0), "m4");
        take(m1.ask(&ask(8000, 8, 4)).unwrap(), "m1");
        assert!(!real.is_empty() && real.len() < 8, "the budget holds some back: {real:?}");
        assert!(real.iter().all(|(_, _, t)| *t == 1), "{real:?}");
        // The forecast: the same z6 tiles, the lead's four slots and the member's, the state fed.
        let regions = [region("a", &[], &units.iter().map(String::as_str).collect::<Vec<_>>(), &[])];
        let slot = |host: &str, k: usize, lead: bool| Machine {
            lead_steps: lead,
            mac: Some(host.into()),
            ..mac(&super::super::slot_worker(host, k), 1.0)
        };
        let machines = (0..4).map(|k| slot("m4", k, true)).chain((0..4).map(|k| slot("m1", k, false))).collect();
        let mut inp = input(&regions, machines, &cost);
        inp.after = Vec::new();
        inp.macs.insert("m4".into(), MacRoom { mem_mb: 16_000, cores: 14 });
        inp.sched = Some(st);
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        let first: Vec<(String, Vec<String>, u32)> = f
            .grants
            .iter()
            .filter(|g| g.at as u64 == inp.now)
            .map(|g| (g.mac.clone(), g.targets.clone(), g.threads))
            .collect();
        assert_eq!(first, real);
    }

    /// Task #927: the forecast sizes a job as the coordinator does. Fed its recorded state, each
    /// Mac's first grant is the coordinator's: the batch length the pool's setting says (20
    /// minutes), a Mac with runs of the step sized by them (m1, 300 s a z6 tile: four), one with
    /// none as before (the lead, m4: m1's runs counted twice, two).
    #[test]
    fn the_forecast_sizes_jobs_as_the_coordinator_does() {
        use crate::coord::{self, asks, client, cost_version};
        let d = tempfile::tempdir().unwrap();
        let (c, port) = coord::start_for_test(&d.path().join("coord"), "m4", "");
        let m1 = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        let batch = asks::SettingAsk { name: asks::BATCH.into(), value: 20.into(), by: "t".into(), at: 1 };
        c.take_ask(&asks::Ask::Setting(batch)).unwrap();
        let units: Vec<String> = (0..24).map(|i| format!("6/8/{i}")).collect();
        for u in &units {
            let sizes = [("threads".to_string(), 1)].into();
            let cost = coord::Cost { peak_mb: 1000, secs: 300, v: cost_version("unit"), sizes, ..Default::default() };
            c.add_costs_by(&[(coord::cost_key("unit", u), cost)], "m1");
        }
        let ask = |mem_mb: u64, cores: u32| coord::Ask {
            kind: "native".into(),
            can: vec!["unit".into()],
            mem_mb,
            cores,
            disk_mb: Some(100_000),
            cap: Some(coord::Capacity { cores, cores_free: cores, ..Default::default() }),
            want: 1,
            ..Default::default()
        };
        assert!(m1.ask(&ask(8000, 8)).unwrap().is_none());
        c.offer(
            "p",
            vec![coord::Offer {
                step: "unit".into(),
                targets: units.iter().map(|u| (u.clone(), super::KEY.to_string(), 0)).collect(),
                job: None,
            }],
        );
        c.set_capacity(ask(16_000, 14).cap);
        let st: State = serde_json::from_str(&serde_json::to_string(&c.sched_state()).unwrap()).unwrap();
        assert_eq!(st.batch_s, 1200, "the setting, recorded");
        let mut real: Vec<(String, Vec<String>)> = Vec::new();
        for (who, g) in
            [("m4", c.ask_own(&ask(16_000, 14), &Default::default(), 0)), ("m1", m1.ask(&ask(8000, 8)).unwrap())]
        {
            if let Some(coord::Granted::Job { targets, .. }) = g.map(|g| g.work) {
                real.push((who.to_string(), targets.into_iter().map(|t| t.0).collect()));
            }
        }
        assert_eq!(real.iter().map(|r| r.1.len()).collect::<Vec<_>>(), [2, 4], "{real:?}");
        let regions = [region("a", &[], &units.iter().map(String::as_str).collect::<Vec<_>>(), &[])];
        let one = |host: &str, lead: bool| Machine { lead_steps: lead, mac: Some(host.into()), ..mac(host, 1.0) };
        let mut inp = input(&regions, vec![one("m4", true), one("m1", false)], &cost);
        inp.after = Vec::new();
        inp.macs.insert("m4".into(), MacRoom { mem_mb: 16_000, cores: 14 });
        inp.sched = Some(st);
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        let first: Vec<(String, Vec<String>)> =
            f.grants.iter().filter(|g| g.at as u64 == inp.now).map(|g| (g.mac.clone(), g.targets.clone())).collect();
        assert_eq!(first, real);
    }

    /// Task #1163: placement on, a member's ask tries the long poles' steps first (their slack in
    /// the forecast, `sched::Sched::pack`), and the forecast's run so too: fed the coordinator's
    /// recorded state, the member's first grant is the coordinator's, the step plan order puts
    /// second (overnight on 12 Nov the lead granted the MacBooks slope-deep where the forecast,
    /// trying plan order, granted them terrain-deep: its lines said "fewer" for one and "more" for
    /// the other). Placement off, both grant the step plan order puts first.
    #[test]
    fn placement_on_the_forecast_tries_a_members_offers_as_the_coordinator_does() {
        use crate::coord::{self, client, cost_version};
        let units: Vec<String> = (0..4).map(|i| format!("6/8/{i}")).collect();
        let terrain: Vec<String> = (0..4).map(|i| format!("6/1/{i}")).collect();
        // (The coordinator offered the work in `order`; the run offers it as its plan lists it.)
        let first_grants = |slack: Option<&[(&str, u64)]>, order: [&str; 2]| {
            let d = tempfile::tempdir().unwrap();
            let (c, port) = coord::start_for_test(&d.path().join("coord"), "m4", "");
            let m1 = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
            let of = |step: &str| if step == "unit" { &units } else { &terrain };
            for step in order {
                for t in of(step) {
                    let sizes = [("threads".to_string(), 1)].into();
                    let cost =
                        coord::Cost { peak_mb: 1000, secs: 300, v: cost_version(step), sizes, ..Default::default() };
                    c.add_costs_by(&[(coord::cost_key(step, t), cost)], "m1");
                }
            }
            // (The member's room holds one job; the lead's none.)
            let ask = coord::Ask {
                kind: "native".into(),
                can: vec!["terrain-hi".into(), "unit".into()],
                mem_mb: 1500,
                cores: 8,
                disk_mb: Some(100_000),
                cap: Some(coord::Capacity { cores: 8, cores_free: 8, ..Default::default() }),
                want: 1,
                ..Default::default()
            };
            assert!(m1.ask(&ask).unwrap().is_none());
            if let Some(s) = slack {
                let slack = s.iter().map(|(k, v)| (k.to_string(), *v)).collect();
                c.set_placement(Some(crate::agent::placement::Placement { slack }));
            }
            let offer = |step: &str| coord::Offer {
                step: step.into(),
                targets: of(step).iter().map(|t| (t.clone(), super::KEY.to_string(), 0)).collect(),
                job: None,
            };
            c.offer("p", order.iter().map(|s| offer(s)).collect());
            let st: State = serde_json::from_str(&serde_json::to_string(&c.sched_state()).unwrap()).unwrap();
            let real = match m1.ask(&ask).unwrap().map(|g| g.work) {
                Some(coord::Granted::Job { step, targets, .. }) => (step, targets[0].0.clone()),
                other => panic!("{other:?}"),
            };
            let t: Vec<&str> = terrain.iter().map(String::as_str).collect();
            let u: Vec<&str> = units.iter().map(String::as_str).collect();
            let regions = [region("a", &t, &[], &[]), region("b", &[], &u, &[])];
            let one = |host: &str, lead: bool| Machine { lead_steps: lead, mac: Some(host.into()), ..mac(host, 1.0) };
            let mut inp = input(&regions, vec![one("m4", true), one("m1", false)], &cost);
            inp.after = Vec::new();
            inp.macs.insert("m4".into(), MacRoom { mem_mb: 500, cores: 14 });
            inp.sched = Some(st);
            let f = forecast(crate::agent::seam::for_tests(), &inp);
            let run: Vec<(String, String)> = f
                .grants
                .iter()
                .filter(|g| g.at as u64 == inp.now && g.mac == "m1")
                .map(|g| (g.step.clone(), g.targets[0].clone()))
                .collect();
            (real, run)
        };
        // (The step the run's plan order puts first, `first`; the other, `second`.)
        let (_, run) = first_grants(None, ["terrain-hi", "unit"]);
        assert_eq!(run.len(), 1, "{run:?}");
        let first = run[0].0.clone();
        let second = if first == "unit" { "terrain-hi" } else { "unit" };
        let order = [first.as_str(), second];
        let (real, run) = first_grants(None, order);
        assert_eq!((real.0.as_str(), &run), (first.as_str(), &vec![real.clone()]), "off: plan order, both");
        // On: the second has no slack, the first an hour's.
        let (real, run) = first_grants(Some(&[(second, 0), (first.as_str(), 3600)]), order);
        assert_eq!(real.0, second, "the coordinator: the long pole first");
        assert_eq!(run, [real], "the forecast's run: as the coordinator");
    }

    /// Task #563: the live error says how an earlier run fared against the targets done since.
    #[test]
    fn the_live_error_said() {
        use crate::coord::history::Event;
        let now = 100_000;
        let job = |w: &str, t: u64, secs: f64, n: usize| Event {
            t,
            kind: "done".into(),
            worker: Some(w.into()),
            step: Some("unit".into()),
            secs: Some(secs),
            targets: vec!["x".into(); n],
            ..Default::default()
        };
        // (Two jobs done since the run was made, two at or before it.)
        let events = vec![
            job("m4", now - 3600, 7200.0, 1),
            job("m4 (second job)", now - 3600, 7200.0, 1),
            job("m1", now - 7200, 3600.0, 1),
            job("ipad", now - 9000, 600.0, 1),
        ];
        // (A run that had done 4 targets 600 s in; the history 2 in the 7200 s since: 300 s.)
        let p = pace(&[(now - 7200, vec![0, 4, 8])], &events, now).unwrap();
        assert_eq!((p.real_n, p.run_s, p.real_s), (2, 300, 7200));
        assert!(pace(&[(now - 600, vec![0, 4])], &events, now).is_none(), "too young");
    }

    #[test]
    fn a_departure_is_two_jobs_and_half_the_forecasts_apart() {
        assert!(!departed(3, 4) && !departed(10, 13) && departed(0, 2) && departed(4, 1) && departed(3, 6));
        let g = |at: f64| Grant { mac: "m4".into(), step: "unit".into(), targets: vec![], at, threads: 1 };
        let f = Forecast { at: 1000, grants: vec![g(1000.0), g(1100.0), g(1200.0), g(9000.0)], ..Default::default() };
        let real =
            |n: usize| (0..n).map(|k| (1000 + k as u64, "m4".to_string(), "unit".to_string())).collect::<Vec<_>>();
        let none = BTreeMap::new();
        assert!(departures(&f, &real(4), &none, 2000).is_empty(), "one more: not said");
        assert_eq!(departures(&f, &real(0), &none, 2000).len(), 1, "none of three");
    }

    /// Task #1188: the grants of a step the forecast doesn't model (`UNFORESEEN`: GC, the backup…)
    /// depart from no forecast and aren't said; a modelled step's are said as before, more granted
    /// than foreseen (none foreseen among them) word for word.
    #[test]
    fn a_step_the_forecast_doesnt_model_never_departs() {
        let g = |step: &str, at: f64| Grant { mac: "m4".into(), step: step.into(), targets: vec![], at, threads: 1 };
        let f = Forecast { at: 1000, grants: vec![g("unit", 1000.0), g("unit", 1100.0)], ..Default::default() };
        let real = |step: &str, n: usize| -> Vec<(u64, String, String)> {
            (0..n).map(|k| (1000 + k as u64, "m6".to_string(), step.to_string())).collect()
        };
        let none = BTreeMap::new();
        for step in UNFORESEEN {
            assert!(!foreseen(step), "{step}");
            let quiet = Forecast { at: 1000, ..Default::default() };
            assert!(departures(&quiet, &real(step, 446), &none, 2000).is_empty(), "{step}");
        }
        let mut all: Vec<(u64, String, String)> = UNFORESEEN.iter().flat_map(|s| real(s, 40)).collect();
        all.extend(real("inputs", 3));
        all.extend((0..6).map(|k| (1000 + k, "m4".to_string(), "unit".to_string())));
        let d = departures(&f, &all, &none, 2000);
        assert_eq!(
            d.iter().map(|(k, d)| (k.clone(), d.as_str())).collect::<Vec<_>>(),
            [
                (("m4".to_string(), "unit".to_string()), "m4: 6 job(s) of unit granted, the forecast's 2"),
                (("m6".to_string(), "inputs".to_string()), "m6: 3 job(s) of inputs granted, the forecast's 0"),
            ]
        );
        // Many of an unmodelled step in the hour before now are said; as many as the line allows,
        // older ones, the modelled steps' and one at `now` itself aren't.
        let mut many = real("gc", UNFORESEEN_HOURLY + 1);
        many.extend(real("backup", UNFORESEEN_HOURLY));
        many.extend(real("unit", 100));
        many.extend((0..50).map(|_| (1000 - 1, "m6".to_string(), "osm-pass".to_string())));
        many.extend((0..50).map(|_| (1000 + 3599, "m6".to_string(), "redo".to_string())));
        assert_eq!(
            unforeseen_many(&many, 1000 + 3599),
            [(
                ("m6".to_string(), "gc".to_string()),
                "m6: 31 job(s) of gc granted in the last hour, more than 30".to_string()
            )]
        );
    }

    /// Task #1163: a Mac granted fewer than the forecast foresaw is said with what the lead answered
    /// its asks since: none heard; the step not offered; the rules that held them back, counted,
    /// those deciding by live state said so, and the last one's words; a refusal before any
    /// packing; each ask granted all it wanted. Asks before the forecast count for nothing.
    #[test]
    fn a_departure_of_fewer_names_what_held_the_grants_back() {
        use crate::coord::sched::{Held, Rule};
        use crate::coord::Answer;
        let g = |at: f64| Grant { mac: "m4".into(), step: "unit".into(), targets: vec![], at, threads: 1 };
        let f = Forecast { at: 1000, grants: vec![g(1000.0), g(1100.0), g(1200.0)], ..Default::default() };
        let ask = |at: u64, want: usize, got: usize, offered: &[&str], held: Option<(Rule, &str)>| Answer {
            at,
            want,
            got,
            offered: offered.iter().map(|s| s.to_string()).collect(),
            held: held.map(|(rule, w)| Held { rule, words: w.into(), disk_mb: None, held_for: None }),
            refused: None,
        };
        let line = |answers: Vec<Answer>| {
            let d = departures(&f, &[], &[("m4".to_string(), answers)].into(), 2000);
            assert_eq!(d.len(), 1, "{d:?}");
            d[0].1.clone()
        };
        let d = line(Vec::new());
        assert!(d.starts_with("m4: 0 job(s) of unit granted, the forecast's 3; no ask from it since"), "{d}");
        let d = line(vec![
            ask(900, 4, 0, &["unit"], Some((Rule::Cores, "before the forecast"))),
            ask(1010, 2, 0, &["unit"], Some((Rule::Memory, "memory: the brake is on"))),
            ask(1020, 2, 0, &["pack"], Some((Rule::Memory, "memory: the brake is on"))),
            ask(1030, 1, 0, &["pack", "unit"], Some((Rule::Cores, "cores: 0.5 of 8 left, a job of unit takes 1.0"))),
        ]);
        assert_eq!(
            d,
            "m4: 0 job(s) of unit granted, the forecast's 3; its 3 ask(s) since wanted 5 job(s) and got 0, 2 offered \
             unit; held back by memory ×2 (live: macOS's memory now; the forecast takes each Mac at its sustained \
             best), cores ×1; the last: cores: 0.5 of 8 left, a job of unit takes 1.0"
        );
        let d = line(vec![ask(1010, 2, 2, &["pack"], None)]);
        assert!(d.contains("0 offered unit (not listed by the lead's plan yet"), "{d}");
        assert!(d.ends_with("each got all it wanted: its slots freed later than the forecast's lanes"), "{d}");
        let refused = Answer {
            refused: Some("on the app of 11 Nov 23:10, the lead on 12 Nov 00:15's".into()),
            ..ask(1010, 3, 0, &[], None)
        };
        let d = line(vec![refused]);
        assert!(d.contains("held back by refused before packing ×1"), "{d}");
        assert!(d.ends_with("the last: on the app of 11 Nov 23:10, the lead on 12 Nov 00:15's"), "{d}");
        // (More granted: as it was, its cause #1094's.)
        let more: Vec<(u64, String, String)> =
            (0..6).map(|k| (1000 + k, "m4".to_string(), "unit".to_string())).collect();
        let d = departures(
            &f,
            &more,
            &[("m4".to_string(), vec![ask(1010, 2, 0, &[], Some((Rule::Cores, "x")))])].into(),
            2000,
        );
        assert_eq!(d[0].1, "m4: 6 job(s) of unit granted, the forecast's 3");
    }

    #[test]
    fn members_speed_from_the_history() {
        use crate::coord::history::Event;
        let e = |w: &str, secs: f64, n: usize| Event {
            kind: "done".into(),
            worker: Some(w.into()),
            step: Some("unit".into()),
            secs: Some(secs),
            ok: Some(true),
            targets: vec!["x".into(); n],
            ..Default::default()
        };
        let mut events = vec![e("m4", 300.0, 1), e("m4", 600.0, 2), e("m4", 300.0, 1)];
        events.extend([e("m1", 600.0, 1), e("m1", 600.0, 1), e("m1", 600.0, 1)]);
        let s = speeds(crate::agent::seam::for_tests(), &events, "m4", &["m1".into(), "ipad".into()], 0.5);
        assert!((s["m1"].0 - 0.5).abs() < 1e-9 && s["m1"].1);
        assert_eq!(s["ipad"], (0.5, false));
    }

    /// An input at live scale: `n_regions` regions of 15 z6 tiles each (their units, slope, trees,
    /// packs), `n_chain` targets in each of four chains, twenty lanes on two Macs, the scheduler's
    /// state with measures for a sixth of the regions' targets (their NAS rates too), the NAS
    /// budget on and leases out.
    fn live_scale_input(n_regions: usize, n_chain: usize) -> Input<'static> {
        use crate::coord::sched::{Asked, Leased};
        let z6 = |i: usize| format!("6/{}/{}", i / 64, i % 64);
        let z8 = |i: usize, j: usize| format!("8/{i}/{j}");
        let regions: Vec<RegionLeft> = (0..n_regions)
            .map(|r| {
                let terrain: Vec<String> = (0..15).map(|j| z6(r * 15 + j)).collect();
                let units: Vec<String> = terrain[..3].to_vec();
                let tiles = |n: usize, off: usize| (0..n).map(|j| z8(r, off + j)).collect::<Vec<_>>();
                RegionLeft {
                    id: format!("r{r}"),
                    own_units: units.clone(),
                    units,
                    own_terrain: terrain.clone(),
                    terrain: terrain.clone(),
                    slope: tiles(15, 0),
                    trees: tiles(25, 100),
                    packs: (0..15).map(|j| (z8(r, 200 + j), vec![terrain[j % 3].clone()])).collect(),
                    ..Default::default()
                }
            })
            .collect();
        // (Measures for a sixth of the regions' targets, each with its NAS rate; the rest by their
        // step's.)
        let mut costs: BTreeMap<String, crate::coord::Cost> = BTreeMap::new();
        for (k, r) in regions.iter().enumerate().filter(|(k, _)| k % 6 == 0) {
            let each = r.terrain.iter().map(|t| ("terrain-hi", t)).chain(r.units.iter().map(|t| ("unit", t)));
            for (step, t) in
                each.chain(r.slope.iter().map(|t| ("slope-hi", t))).chain(r.trees.iter().map(|t| ("trees", t)))
            {
                let sizes = [
                    ("threads".to_string(), 2),
                    ("bytes".to_string(), 2_000_000_000 + k as u64),
                    ("creates".to_string(), 3000),
                    ("listed".to_string(), 50_000),
                ]
                .into();
                let c = crate::coord::Cost {
                    peak_mb: 800 + (k as u64 % 7) * 300,
                    secs: 60 + k as u64 % 500,
                    v: crate::coord::cost_version(step),
                    sizes,
                    ..Default::default()
                };
                costs.insert(cost_key(step, t), c);
            }
        }
        let cost = |step: &str, t: &str| match costs.get(&cost_key(step, t)) {
            Some(c) => Cost { secs: c.secs as f64, basis: Basis::Own, peak_mb: c.peak_mb },
            None => Cost { secs: 120.0, basis: Basis::Guess, peak_mb: 1000 },
        };
        let chains: Vec<Job> = ["pois", "bldtiles", "depth", "peaks"]
            .iter()
            .flat_map(|s| {
                (0..n_chain).map(move |i| (s.to_string(), z6(i), Cost { secs: 90.0, basis: Basis::Own, peak_mb: 700 }))
            })
            .collect();
        let ask = |w: &str, mem_mb: u64, cores: u32| Asked {
            ask: Ask {
                worker: w.into(),
                kind: "native".into(),
                mem_mb,
                cores,
                disk_mb: Some(400_000),
                cap: Some(Capacity { cores, cores_free: cores, ..Default::default() }),
                want: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let limits = jobrt::budget::nas::Limits {
            bytes_s: 37e6,
            creates_s: 30.0,
            listed_s: 836.0,
            stats_s: 174.0,
            deletes_s: 30.0,
            ..Default::default()
        };
        let leased = |w: &str, step: &str, ts: &[String]| Leased {
            worker: w.into(),
            step: step.into(),
            targets: ts.iter().map(|t| (t.clone(), "k".to_string())).collect(),
            threads: 2,
            granted_at: 0,
            busy_tenths: None,
            left_s: None,
        };
        let packs: Vec<String> = regions[0].packs.iter().map(|p| p.0.clone()).take(5).collect();
        let st = State {
            leases: vec![
                leased("m4", "pack", &packs),
                leased("m1", "trees", &regions[1].trees[..15]),
                leased("m1", "terrain-hi", &regions[1].terrain[..6]),
            ],
            workers: vec![ask("m4", 36_000, 14), ask("m1", 10_240, 10)],
            costs: costs.clone(),
            lead_name: "m4".into(),
            nas: Some(limits),
            nas_own: Some(limits),
            ..Default::default()
        };
        let lanes = |host: &str, n: usize, lead: bool| -> Vec<Machine> {
            (0..n)
                .map(|k| Machine {
                    name: super::super::slot_worker(host, k),
                    speed: if lead { 1.0 } else { 0.6 },
                    measured: true,
                    lead_steps: lead,
                    mem_mb: if lead { 36_000 } else { 10_240 },
                    mac: Some(host.into()),
                    ..Default::default()
                })
                .collect()
        };
        let machines: Vec<Machine> = lanes("m4", 12, true).into_iter().chain(lanes("m1", 8, false)).collect();
        Input {
            now: 1_792_605_430,
            before: Vec::new(),
            drops: Vec::new(),
            costs: cost_table(&regions, &cost),
            regions: Cow::Owned(regions.clone()),
            cadence: Cadence::default(),
            blind: None,
            chains,
            after: Vec::new(),
            idle: Vec::new(),
            machines: machines.clone(),
            running: BTreeMap::new(),
            macs: BTreeMap::new(),
            sched: Some(st),
            needs_pace: Default::default(),
            pace: None,
            unknown: Vec::new(),
            paused: false,
            along: Default::default(),
            code: crate::agent::forecast::VERSION,
        }
    }

    /// At live scale (task #544: the scheduler's run of 21 Oct's lead took a quarter hour): tens
    /// of thousands of targets across regions and chains, twenty lanes on two Macs, the
    /// scheduler's state with thousands of measures (their NAS rates too), the NAS budget on and
    /// leases out. The forecast runs whole in bounded time; past its budget, cut and said so.
    /// The shared groups wait as the per-target lists they replaced (lead memory, 25 Oct): each
    /// input run with its groups, and again with every group spread into its items' own deps (the
    /// old lists), places every item on the same machine at the same times.
    #[test]
    fn groups_wait_as_the_lists_they_replaced() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for round in 0..6 {
            let mut inp = live_scale_input(2 + next(5), 3 + next(20));
            let job = |s: &str, t: String| (s.to_string(), t, Cost { secs: 50.0, basis: Basis::Own, peak_mb: 500 });
            inp.before = (0..next(3)).map(|i| job(["heritage-sites", "reach", "buildings"][i], "all".into())).collect();
            inp.after = (0..next(3)).map(|i| job("catalog", format!("a{i}"))).collect();
            inp.idle = (0..next(3)).map(|i| job("idle", format!("i{i}"))).collect();
            let mut regions = inp.regions.to_vec();
            for r in regions.iter_mut() {
                r.slope_deep = r.slope.iter().take(next(4)).map(|t| t.replace("8/", "10/")).collect();
                r.own_terrain_lo = vec!["3/0/0".into()];
                r.terrain_lo = r.own_terrain_lo.clone();
                r.slope_lo = vec!["3/0/0".into()];
                r.expected = r.terrain.iter().skip(3).take(next(6)).cloned().collect();
            }
            inp.regions = Cow::Owned(regions);
            let groups = items(crate::agent::seam::for_tests(), &inp);
            let mut spread = items(crate::agent::seam::for_tests(), &inp);
            for it in spread.items.iter_mut() {
                for &g in &it.groups {
                    it.deps.extend(&spread.groups[g]);
                }
                it.groups.clear();
            }
            spread.groups.clear();
            assert!(groups.groups.iter().any(|g| !g.is_empty()), "round {round}: no group");
            let a =
                run_on(crate::agent::seam::for_tests(), &inp, groups, &scaled(SCALINGS[0]), Duration::from_secs(600));
            let b =
                run_on(crate::agent::seam::for_tests(), &inp, spread, &scaled(SCALINGS[0]), Duration::from_secs(600));
            assert!(a.done && b.done && !a.cut && !b.cut, "round {round}");
            let at = |s: &Sim| s.items.iter().map(|i| (i.by, i.from.to_bits(), i.end.to_bits())).collect::<Vec<_>>();
            assert_eq!(at(&a), at(&b), "round {round}");
            assert_eq!(a.grants, b.grants, "round {round}");
        }
    }

    /// The live-scale input's shape at a tenth of 22 Oct's scale (13 regions, over a thousand
    /// targets: every kind of item, measure, lease and lane the live ones have): the forecast whole
    /// within its budget, each run whole and uncut; past its budget (none), cut there, partial and
    /// said so; and the budget is CPU time (task #584): a thread's wait counts nothing. The same at
    /// live scale, its CPU time bounded, is the benchmark `a_live_scale_forecast_runs_whole_and_bounded`
    /// (#1112: 11 and 4 s alone on the M4 as tests, up to 11 min on the loaded test Mac).
    #[test]
    fn a_forecast_is_whole_within_its_budget_and_cut_past_it() {
        let inp = live_scale_input(13, 115);
        let n = items(crate::agent::seam::for_tests(), &inp).items.len();
        assert!(n > 1_000, "{n} targets");
        let budget = if cfg!(debug_assertions) { BUDGET * 4 } else { BUDGET };
        let f = forecast_by(crate::agent::seam::for_tests(), &inp, budget);
        assert!(!f.partial && f.done_at.is_some(), "{:?}", f.why);
        let r = run(crate::agent::seam::for_tests(), &inp, &scaled(SCALINGS[2]), Duration::from_secs(600));
        assert!(r.done && !r.cut, "the run was cut or unfinished");
        // (No budget: cut at its first look, every 32 placements.)
        let f = forecast_by(crate::agent::seam::for_tests(), &inp, Duration::ZERO);
        assert!(f.partial && f.done_at.is_none(), "{:?}", f.why);
        let cpu0 = thread_cpu();
        std::thread::sleep(Duration::from_millis(300)); // (sleep ok: a wait whose CPU time is measured)
        assert!(thread_cpu() - cpu0 < Duration::from_millis(50), "a wait is no CPU time");
    }

    /// A benchmark (run by hand: `-- --ignored`), #1112: at a live scale of 400 regions (over
    /// 30,000 targets) a run of the forecast's whole, its CPU time bounded; past its budget, cut
    /// there; and at 22 Oct's (134 regions, about 4,600 chain targets), whole within its budget.
    #[test]
    #[ignore]
    fn a_live_scale_forecast_runs_whole_and_bounded() {
        let inp = live_scale_input(400, 1500);
        let n = items(crate::agent::seam::for_tests(), &inp).items.len();
        assert!(n > 30_000, "{n} targets");
        // (Whole, its CPU time bounded well past what it takes (50 to 73 s for three runs in the
        // test build under load, a third each; 21 Oct's code took over 200 s in one run at a tenth
        // of the scale). CPU time, not wall: a loaded machine stretches the wall, not this. One
        // run, #969: `forecast_by`'s budget is each run's, and its three runs differ only in their
        // costs' scale; the three took 132 s of CPU, and 360 s once on the loaded test Mac, in two
        // check.sh runs of three. The late one, the costliest: its costs scaled up the most, the
        // longest timeline (M1, 5 Nov: 28.5 s of CPU against the expected's 25.7, the soon's 25.3).)
        let cpu0 = thread_cpu();
        let r = run(crate::agent::seam::for_tests(), &inp, &scaled(SCALINGS[2]), Duration::from_secs(600));
        let took = thread_cpu() - cpu0;
        eprintln!("live scale, {n} targets: {} ms of CPU", took.as_millis());
        assert!(r.done && !r.cut, "the run was cut or unfinished");
        assert!(took < Duration::from_secs(120), "{took:?}");
        // Past its budget: cut there, partial and said so.
        let f = forecast_by(crate::agent::seam::for_tests(), &inp, Duration::from_millis(50));
        assert!(f.partial && f.done_at.is_none(), "{:?}", f.why);
        // At 22 Oct's live scale: whole within its budget. (The test build computes ~4x slower than
        // the app's, release: its budget so much longer. Release, on the M1 at load 140: 48 s of
        // wall, whole.)
        let inp = live_scale_input(134, 1150);
        let budget = if cfg!(debug_assertions) { BUDGET * 4 } else { BUDGET };
        let t = Instant::now();
        let f = forecast_by(crate::agent::seam::for_tests(), &inp, budget);
        eprintln!("22 Oct's live scale: {} ms", t.elapsed().as_millis());
        assert!(!f.partial && f.done_at.is_some(), "{:?}", f.why);
    }

    /// Each item's waits, its own deps and its sets' items: as (step, target).
    fn waits_listed(its: &Items) -> Vec<BTreeSet<(String, String)>> {
        its.items
            .iter()
            .map(|it| {
                it.deps
                    .iter()
                    .chain(it.groups.iter().flat_map(|&g| its.groups[g].iter()))
                    .map(|&d| (its.items[d].step.clone(), its.items[d].target.clone()))
                    .collect()
            })
            .collect()
    }

    /// What an item of `inp`'s regions and chains waits on as the planner holds it
    /// (crate::agent::steps::readiness), of the items listed before it (its view's left): a target
    /// read listed, every item of a step read whole; through a read not listed that waits itself
    /// (`held`), what it waits on, and through a step read whole none of whose targets is listed,
    /// the items of the steps it reads on (`whole_left`).
    fn declared_waits(its: &Items, along: &crate::agent::seam::Along, i: usize) -> BTreeSet<(String, String)> {
        use crate::agent::steps::{read_steps, readiness, Ready, View};
        struct Listed<'a>(&'a [Item], &'a crate::agent::seam::Along);
        impl View for Listed<'_> {
            fn left(&self, step: &str, t: &str) -> bool {
                self.0.iter().any(|d| d.step == step && d.target == t)
            }
            fn any_left(&self, step: &str) -> bool {
                self.0.iter().any(|d| d.step == step)
            }
            fn along(&self) -> Option<&crate::agent::seam::Along> {
                Some(self.1)
            }
        }
        fn whole(v: &Listed, step: &str, out: &mut BTreeSet<(String, String)>) {
            if v.any_left(step) {
                out.extend(v.0.iter().filter(|d| d.step == step).map(|d| (d.step.clone(), d.target.clone())));
            } else {
                for (s, passes) in read_steps(step) {
                    if passes {
                        whole(v, s, out);
                    }
                }
            }
        }
        fn expand(v: &Listed, step: &str, t: &str, out: &mut BTreeSet<(String, String)>) {
            let Ready::Waits(w) = readiness(step, t, v) else { return };
            for (s, u) in w {
                if v.left(s, &u) {
                    out.insert((s.to_string(), u));
                } else if u == s {
                    whole(v, s, out);
                } else {
                    expand(v, s, &u, out);
                }
            }
        }
        let it = &its.items[i];
        let mut out = BTreeSet::new();
        expand(&Listed(&its.items[..i], along), &it.step, &it.target, &mut out);
        out
    }

    /// Task #988 (code 6): a target waits through a read built as it stands for what that read
    /// waits on, as the plan holds it (crate::agent::steps::readiness: `held`, `whole_left`). The
    /// 3D buildings' z3 field reads the mids of its z6 tiles' 3D tiles, none left, one of which reads
    /// normalized files left; the depth's root reads every z3 assembly, none left, which read a z6
    /// tile's depth left. Each waits for those, and is placed after them; code 5 ran them at once.
    #[test]
    fn a_target_waits_through_a_read_built_as_it_stands() {
        let c = |secs: f64| Cost { secs, basis: Basis::Own, peak_mb: 1000 };
        let regions: [RegionLeft; 0] = [];
        let unit = |_: &str, _: &str| c(1.0);
        let base = input(&regions, vec![mac("m4", 1.0)], &unit);
        let job = |s: &str, t: &str, secs: f64| (s.to_string(), t.to_string(), c(secs));
        let chains = vec![
            job("bldprep", "6/2/2", 600.0),
            job("bldfield-lo", "3/0/0", 60.0),
            job("depth", "6/1/1", 300.0),
            job("depth-root", "depth-root", 50.0),
        ];
        let inp = Input { chains, ..base };
        let pair = |s: &str, t: &str| (s.to_string(), t.to_string());
        let waits = |inp: &Input, s: &str, t: &str| {
            let its = items(crate::agent::seam::for_tests(), inp);
            let w = waits_listed(&its);
            w[its.items.iter().position(|i| i.step == s && i.target == t).unwrap()].clone()
        };
        assert_eq!(waits(&inp, "bldfield-lo", "3/0/0"), BTreeSet::from([pair("bldprep", "6/2/2")]));
        assert_eq!(waits(&inp, "depth-root", "depth-root"), BTreeSet::from([pair("depth", "6/1/1")]));
        let its = items(crate::agent::seam::for_tests(), &inp);
        for (i, it) in its.items.iter().enumerate().filter(|(_, it)| it.phase == Phase::Chain) {
            assert_eq!(waits_listed(&its)[i], declared_waits(&its, &inp.along, i), "{}", it.target);
        }
        let f = forecast(crate::agent::seam::for_tests(), &inp);
        let at = |s: &str, t: &str| f.placed.iter().find(|p| p.step == s && p.target == t).map(|p| (p.from, p.until));
        let ((fl, _), (_, prep)) = (at("bldfield-lo", "3/0/0").unwrap(), at("bldprep", "6/2/2").unwrap());
        let ((root, _), (_, depth)) = (at("depth-root", "depth-root").unwrap(), at("depth", "6/1/1").unwrap());
        assert!(fl >= prep && root >= depth, "{fl} {prep} {root} {depth}");
    }

    /// Docs/readiness.md §7 (supersedes task #867's held-chain guard): the forecast waits for what
    /// the planner does, both from the steps' declared reads: each item of the regions' slope and
    /// tree cover, the z6 tiles and the chains waits on exactly the items the planner's readiness
    /// holds it for (`declared_waits`), and a replay places none before what it waits on is done. With
    /// SCENIC_FORECAST_RECORD, that record's input too.
    #[test]
    fn the_forecast_waits_for_what_the_planner_does() {
        let c = |secs: f64| Cost { secs, basis: Basis::Own, peak_mb: 1000 };
        let regions: [RegionLeft; 0] = [];
        let unit = |_: &str, _: &str| c(1.0);
        let base = input(&regions, vec![mac("m4", 1.0), mac("m1", 1.0)], &unit);
        let job = |s: &str, t: &str, secs: f64| (s.to_string(), t.to_string(), c(secs));
        let check = |inp: &Input| {
            let its = items(crate::agent::seam::for_tests(), inp);
            let listed = waits_listed(&its);
            for (i, it) in its.items.iter().enumerate() {
                if matches!(it.phase, Phase::Before | Phase::After | Phase::Publish) {
                    continue;
                }
                assert_eq!(listed[i], declared_waits(&its, &inp.along, i), "{} {}", it.step, it.target);
            }
            let f = forecast(crate::agent::seam::for_tests(), inp);
            let at =
                |s: &str, t: &str| f.placed.iter().find(|p| p.step == s && p.target == t).map(|p| (p.from, p.until));
            for (i, it) in its.items.iter().enumerate() {
                let Some((from, _)) = at(&it.step, &it.target) else { continue };
                for (s, t) in &listed[i] {
                    if let Some((_, until)) = at(s, t) {
                        assert!(from >= until, "{} {} placed before {s} {t} ends", it.step, it.target);
                    }
                }
            }
        };
        let before = vec![job("trailends", "trailends", 30.0), job("heritage-sites", "heritage-sites", 30.0)];
        let chains = vec![
            job("depth", "6/1/1", 300.0),
            job("depth", "6/9/9", 300.0),
            job("depth-lo", "3/0/0", 100.0),
            job("depth-root", "depth-root", 50.0),
            job("polar-terrain", "s/4/0/0", 300.0),
            job("polar-terrain", "s/4/9/9", 300.0),
            job("polar-slope", "s/4/0/0", 100.0),
            job("polar-root", "s", 50.0),
            job("bld-fetch", "bld-fetch", 200.0),
            job("bldprep", "6/2/2", 20.0),
            job("bldprep", "6/9/9", 20.0),
            job("bldtiles", "6/2/2", 200.0),
            job("pois", "6/1/1", 300.0),
            job("pois", "6/9/9", 300.0),
            job("summits", "summits", 30.0),
            job("peaks", "6/1/1", 60.0),
            job("items", "items", 60.0),
            job("heritage", "heritage", 50.0),
            job("marks", "marks", 50.0),
        ];
        let inp = Input { before, chains, ..base };
        check(&inp);
        // (Waits as declared: the depth-lo assembly on its own z6 tile's depth, not 6/9/9's; the
        // polar slope on its own block and neighbours; the 3D tiles on their own normalized files
        // and neighbours'; the candidates on the hiking-route ends; the heritage chain beside them.)
        let its = items(crate::agent::seam::for_tests(), &inp);
        let w = waits_listed(&its);
        let of = |s: &str, t: &str| &w[its.items.iter().position(|i| i.step == s && i.target == t).unwrap()];
        let pair = |s: &str, t: &str| (s.to_string(), t.to_string());
        assert_eq!(*of("depth-lo", "3/0/0"), BTreeSet::from([pair("depth", "6/1/1")]));
        assert_eq!(*of("polar-slope", "s/4/0/0"), BTreeSet::from([pair("polar-terrain", "s/4/0/0")]));
        assert_eq!(*of("bldtiles", "6/2/2"), BTreeSet::from([pair("bldprep", "6/2/2")]));
        assert!(of("bldprep", "6/2/2").is_empty(), "the normalized files go beside the fetch");
        assert_eq!(*of("pois", "6/1/1"), BTreeSet::from([pair("trailends", "trailends")]));
        assert_eq!(*of("heritage", "heritage"), BTreeSet::from([pair("heritage-sites", "heritage-sites")]));
        assert!(
            of("peaks", "6/1/1").contains(&pair("pois", "6/1/1"))
                && !of("peaks", "6/1/1").contains(&pair("pois", "6/9/9"))
        );
        assert!(
            of("marks", "marks").contains(&pair("pois", "6/9/9"))
                && of("marks", "marks").contains(&pair("heritage", "heritage"))
        );
        if let Ok(p) = std::env::var("SCENIC_FORECAST_RECORD") {
            let r = Record::from_bytes(&std::fs::read(p).unwrap()).unwrap();
            check(&r.input);
        }
    }

    /// A recorded forecast's scheduler (SCENIC_PACK_RECORD: a forecast-input.json, its leases'
    /// `granted_at` where said) asked once by its lead as its last ask said, at the record's time:
    /// what it grants and why it grants no more, printed (unset: nothing). How task #973 showed the
    /// mini's held room taking work on a frozen copy of its state.
    #[test]
    fn recorded_lead_pack() {
        let Ok(p) = std::env::var("SCENIC_PACK_RECORD") else { return };
        let r = Record::from_bytes(&std::fs::read(p).unwrap()).unwrap();
        let st = r.input.sched.as_ref().expect("the lead's scheduler's state");
        let clock = Clock { now: Instant::now(), unix: r.input.now };
        let mut s = Sched::from_state(crate::agent::seam::for_tests(), st, clock, RUN_TTL);
        let a = st.workers.iter().find(|w| w.ask.worker == st.lead_name).expect("the lead's ask").ask.clone();
        let (picks, why) = s.pack(&a, &st.offers, true, clock);
        for p in &picks {
            eprintln!("granted: {} ({} targets, {} threads)", p.step, p.targets.len(), p.threads);
        }
        eprintln!("granted {} of {} wanted; why: {}", picks.len(), a.want, why.map(|h| h.words).unwrap_or_default());
    }

    /// A recorded forecast (SCENIC_FORECAST_RECORD: a forecast-input.json) timed. For profiling at live scale (unset: nothing);
    /// SCENIC_FORECAST_OUT and SCENIC_TOCOME_OUT write its forecast and its `/api/tocome` answer;
    /// with SCENIC_FORECAST_RECOST, its cost table made again by this build's rule first
    /// (crate::agent::fccheck::recost).
    #[test]
    fn recorded_timing() {
        let Ok(p) = std::env::var("SCENIC_FORECAST_RECORD") else { return };
        let mut r = Record::from_bytes(&std::fs::read(p).unwrap()).unwrap();
        if std::env::var("SCENIC_FORECAST_RECOST").is_ok() {
            crate::agent::fccheck::recost(crate::agent::seam::for_tests(), &mut r.input);
        }
        let limit: u64 = std::env::var("SCENIC_FORECAST_LIMIT_S").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
        let mb = || jobrt::sys::footprint().unwrap_or(0) >> 20;
        let base = mb();
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watch = {
            let (peak, stop) = (peak.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    peak.fetch_max(jobrt::sys::footprint().unwrap_or(0) >> 20, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20)); // (sleep ok: a sampler)
                }
            })
        };
        let Items { items, groups, .. } = items(crate::agent::seam::for_tests(), &r.input);
        let held: usize = items.iter().map(|i| i.deps.capacity() + i.groups.capacity()).sum::<usize>()
            + groups.iter().map(|g| g.capacity()).sum::<usize>();
        eprintln!(
            "items {}, machines {}, dep entries held {held}, after items {} MB",
            items.len(),
            r.input.machines.len(),
            mb().saturating_sub(base)
        );
        drop(items);
        let t = Instant::now();
        let f = forecast_by(crate::agent::seam::for_tests(), &r.input, Duration::from_secs(limit));
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = watch.join();
        eprintln!(
            "lead: {} ms, partial {}, done_at {:?}; footprint {base} MB before, peak {} MB",
            t.elapsed().as_millis(),
            f.partial,
            f.done_at,
            peak.load(std::sync::atomic::Ordering::SeqCst)
        );
        if let Ok(out) = std::env::var("SCENIC_FORECAST_OUT") {
            std::fs::write(out, serde_json::to_vec_pretty(&f).unwrap()).unwrap();
        }
        // (`/api/tocome`'s answer as the lead would make it: for the page's tests, touch-test.mjs --tocome.)
        if let Ok(out) = std::env::var("SCENIC_TOCOME_OUT") {
            let (tc, _, _) = ToCome::take(&mut f.clone(), &r.input.machines);
            std::fs::write(out, serde_json::to_vec(&tc.answer()).unwrap()).unwrap();
        }
    }

    /// Every archived forecast input under a project folder (SCENIC_FORECAST_INPUTS: its
    /// `state/coord/forecast-inputs`, read only; SCENIC_FORECAST_FROM and SCENIC_FORECAST_TO, unix s,
    /// the span, by default the archive's days kept to now)
    /// timed as the lead runs it, a line each: its listing's CPU time and each of its three runs'
    /// (each on a thread of its own, as `forecast_by` runs them), cut or not at `BUDGET`
    /// (SCENIC_FORECAST_LIMIT_S another), the items placed of all. With SCENIC_FORECAST_HASH, its
    /// whole forecast made at that budget instead, a hash of its JSON and its wall time: two builds'
    /// forecasts of the same inputs compared. For profiling at live scale (unset: nothing).
    #[test]
    fn archived_timing() {
        let Ok(root) = std::env::var("SCENIC_FORECAST_INPUTS") else { return };
        let budget = std::env::var("SCENIC_FORECAST_LIMIT_S")
            .ok()
            .and_then(|s| s.parse().ok())
            .map_or(BUDGET, Duration::from_secs);
        let hash = std::env::var("SCENIC_FORECAST_HASH").is_ok();
        let the_plan = crate::agent::seam::for_tests();
        let cpu = |f: &dyn Fn() -> Sim| {
            let c0 = thread_cpu();
            let s = f();
            (s, thread_cpu().saturating_sub(c0))
        };
        let (mut most, mut cuts, mut n) = (Duration::ZERO, 0, 0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let to: u64 = std::env::var("SCENIC_FORECAST_TO").ok().and_then(|s| s.parse().ok()).unwrap_or(now);
        // (Not said: the archive's days kept. Its reader opens each day's file of the span.)
        let from: u64 = std::env::var("SCENIC_FORECAST_FROM")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(to.saturating_sub(crate::agent::fcinputs::KEEP_S));
        for i in crate::agent::fcinputs::read(&store::nasfs::NasRoot::from_arg(&root), from, to) {
            let r = i.record().unwrap();
            let inp = &r.input;
            if hash {
                use std::hash::{Hash, Hasher};
                let t = Instant::now();
                let f = forecast_by(the_plan, inp, budget);
                let mut h = std::collections::hash_map::DefaultHasher::new();
                serde_json::to_vec(&f).unwrap().hash(&mut h);
                let took = t.elapsed();
                most = most.max(took);
                (cuts, n) = (cuts + usize::from(f.partial), n + 1);
                eprintln!(
                    "{} term {}: hash {:016x}, {} ms, partial {}",
                    i.at,
                    i.term,
                    h.finish(),
                    took.as_millis(),
                    f.partial
                );
                continue;
            }
            let c0 = thread_cpu();
            let its = items(the_plan, inp);
            let listing = thread_cpu().saturating_sub(c0);
            let runs: Vec<(Sim, Duration)> = std::thread::scope(|s| {
                let hs: Vec<_> = SCALINGS
                    .iter()
                    .map(|&sc| {
                        let its = its.clone();
                        s.spawn(move || cpu(&|| run_on(the_plan, inp, its.clone(), &scaled(sc), budget)))
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let slowest = runs.iter().map(|r| r.1).max().unwrap_or_default();
            let cut = runs.iter().any(|r| r.0.cut);
            (most, cuts, n) = (most.max(slowest), cuts + usize::from(cut), n + 1);
            let placed = runs[0].0.items.iter().filter(|i| i.by.is_some()).count();
            eprintln!(
                "{} term {}: items {}, machines {}, listing {} ms, runs {} ms, cut {cut}, placed {placed}",
                i.at,
                i.term,
                runs[0].0.items.len(),
                inp.machines.len(),
                listing.as_millis(),
                runs.iter().map(|r| r.1.as_millis().to_string()).collect::<Vec<_>>().join("/"),
            );
        }
        eprintln!("{n} inputs, {cuts} cut; the slowest {} ms", most.as_millis());
    }
}
