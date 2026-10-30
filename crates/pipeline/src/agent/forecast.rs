//! The build's forecast (docs/building.md §2, The forecast; the worker page's road to done): when each
//! step, each region and the whole build will be done, when each region reaches the map, when the
//! cadenced publishes may next go out, and what each machine does next.
//!
//! The work left is run through in the order the agent runs it: the lead's first slot takes the
//! first it can (the pass's worldwide jobs, then the map tiles of the z6 tiles built, then a region at
//! a time: its terrain, then its z6 tiles; with none it can do now, the slope, tree cover and the
//! chains' work). Every other slot, the lead's and each member's (a lane each, `equal`; as many as
//! each Mac may run now, and the slots the lead's ramp grows to later, `Machine::from_s`), takes
//! from the front of the plan's order the first work that it may run (a member never the lead's own
//! steps) and that fits its Mac's room beside what the Mac runs, as the coordinator packs an ask
//! (`MacRoom`: its whole memory and its cores at the packed share each; a step not measured by its provisional need, one of it at a time;
//! a job an ask while a member runs an older app); the pages' lane (`helper`) the far end of the
//! first shared step it can do (terrain's near end). Each lane starts with its slot's job under
//! way. The trains' and the landmarks' chains run from the start, each step once what it reads is
//! built. Publishing goes per z6 tile (docs/building.md §2, Publishing): each map tile of a region
//! (`RegionLeft::packs`, at its own measured time) is drawn once the z6 tiles that draw it are built,
//! ahead of the regions' work, and a catalogue follows within the minute (build::CATALOGUE_EVERY_S):
//! the region is all on the map then (none said once only slope and tree cover are left). The whole-map steps go by their cadence (`Forecast::publishes`:
//! build::low_gate, build::gate_open). Each target takes its last run's time (else its step's mean,
//! else a first guess), at the speed measured for the machine doing it. It's run three times: as
//! estimated, and for a range, the times measured a little off and those guessed much more.

use super::build::{RegionLeft, CATALOGUE_EVERY_S, LOW_EVERY_S, MARKS_EVERY_S, OVERLAYS_EVERY_S};
use crate::coord::sched::{Clock, Sched, State};
use crate::coord::{cost_key, Ask, Capacity, Offer};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

// The shared steps a helper takes, in its order of preference.
use super::steps::SHARED;

/// A machine the work is shared among.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub name: String,
    /// Its speed against the lead's (1), and whether that's measured.
    pub speed: f64,
    pub measured: bool,
    /// A helper: the shared steps alone, what fits its memory.
    pub helper: bool,
    /// A lane of any work in the lead's order that fits its memory (and that it may run:
    /// `lead_steps`): the lead's other slots, each member's slots (as the coordinator packs them:
    /// any step offered that fits).
    pub equal: bool,
    pub mem_mb: u64,
    /// Seconds from now until it's free (its job under way's time left).
    pub busy_s: f64,
    /// What each z6 tile takes it more (seconds): the lead's, while a worker that takes tails is
    /// around, the moment its job gives one to take its tail (crate::offload::LEASE_WAIT). (A
    /// terrain z6 tile's subtrees out with workers, crate::terrain_task, take none: they're offered as
    /// the z6 tile begins and waited for only after its own run, by then taken or not, and its
    /// measured time, the job's, has those waits in it.)
    pub unit_extra_s: f64,
    /// It may run the lead's own steps (`steps::Place::Lead`): the lead's slots; a member's take
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

/// A Mac's room when the scheduler's state records no ask of it (`Input::macs`): its memory (MB; 0,
/// not known) and cores.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MacRoom {
    pub mem_mb: u64,
    pub cores: u32,
}

/// What a target takes: its time at the lead's pace (seconds), whether that time was measured,
/// and its memory (MB).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub secs: f64,
    pub known: bool,
    pub peak_mb: u64,
}

/// A job of the lead's alone, or one of the chains': (step, target, cost).
pub type Job = (String, String, Cost);

/// The work left, as the plan sees it (build::Plan), and what each target takes: all of it data,
/// so the lead's, written to the NAS each time it's made (`Record`), gives another Mac the same
/// forecast.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Input<'a> {
    pub now: u64,
    /// The lead's jobs before the regions' (the pass's worldwide jobs, the heritage sites).
    pub before: Vec<Job>,
    /// The regions' work left, in the order they're built (build::Plan::regions).
    pub regions: Cow<'a, [RegionLeft]>,
    /// The drops (build::Plan::drops: z10 tiles no deep target any more, their packs dropped),
    /// each at its drop's cost: first of the regions' work, waiting on nothing, shown as drops
    /// (`Lane::drop`). A region whose terrain they're in waits on them.
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
    /// it reads is built (`chain_deps`).
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
    /// Jobs of `before` not ready at the start (`Release`): none live (the plan's own items carry
    /// their dependencies); the backtest's, each once what it waits on is done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub release: Vec<Release>,
    /// What held each Mac back over the last day (`held_back`), by Mac: its lanes are cut to it
    /// (`hold_back`) before the run; said here for the forecast's readers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub held: BTreeMap<String, Held>,
    /// How an earlier forecast's run fared against the work done since (`pace`): the live error,
    /// said beside the finish. None: no earlier forecast to hold to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
    /// Each step's pace against its costs (`step_pace`): what its jobs of the last
    /// `STEP_PACE_S` took, at the lead's pace, over what the forecasts before them gave their
    /// targets, a step that ran faster than them alone. Every item of the step is run at its cost
    /// times this; a step not in it, at its cost.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub step_pace: BTreeMap<String, f64>,
    /// The build is paused (crate::control::Pause; code 3 on): nothing new starts until it's
    /// resumed, when no one can say, so the forecast places nothing (`PAUSED`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    /// The lanes' time each Mac gave each step over the last day, hours (`took`; code 3 on): the
    /// run offers a Mac none of the steps it gave little of while others took them (`skips`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub took: BTreeMap<String, BTreeMap<String, f64>>,
}

/// What held a Mac back over the last `HELD_S` (task #563; `held_back`): the jobs it ran at once
/// while it ran any (`lanes`, the mean, rounded; one at least). The lead's ramp, its slot cap, the
/// NAS budget, a member's memory and the plan feeding work gradually all show in it; the forecast's
/// run, which releases work by the plan's dependencies alone, sees none of that otherwise.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Held {
    pub lanes: usize,
    /// Code 2 and before: the share of its lanes' time it kept busy, which the run slowed its
    /// lanes to (a record's speeds hold it; the backtest takes it off). 0 from code 3 on: slowed,
    /// the run placed later still the work the pool did soon (docs/pool.md §7.9).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub util: f64,
}

fn is_zero(x: &f64) -> bool {
    *x == 0.0
}

/// A worker's Mac: a slot's worker counted for its Mac, an old second slot's name too.
fn worker_mac(w: &str) -> String {
    w.split(" #").next().unwrap_or(w).trim_end_matches(" (second job)").to_string()
}

/// The span `held_back` and `took` read.
pub const HELD_S: u64 = 86_400;

/// Each Mac's `Held` from the history (`events`: the jobs done, ended and failed, a slot's worker
/// counted for its Mac) over the `HELD_S` before `now`. A Mac that ran none is left out.
pub fn held_back(events: &[crate::coord::history::Event], now: u64) -> BTreeMap<String, Held> {
    let from = now.saturating_sub(HELD_S);
    let n = (HELD_S / 60) as usize;
    let mac = worker_mac;
    // (Each Mac's jobs running each minute.)
    let mut per: BTreeMap<String, Vec<u16>> = BTreeMap::new();
    for e in events.iter().filter(|e| e.t > from && matches!(e.kind.as_str(), "done" | "end" | "fail")) {
        let (Some(w), Some(secs)) = (e.worker.as_deref(), e.secs) else { continue };
        let a = (e.t.saturating_sub(secs as u64).max(from) - from) as usize / 60;
        let b = ((e.t.min(now) - from) as usize).div_ceil(60).min(n);
        let mine = per.entry(mac(w)).or_insert_with(|| vec![0; n]);
        for m in a..b {
            mine[m] += 1;
        }
    }
    per.into_iter()
        .filter_map(|(m, c)| {
            let busy = c.iter().filter(|&&c| c > 0).count();
            let sum: f64 = c.iter().map(|&c| c as f64).sum();
            (busy > 0).then(|| (m, Held { lanes: ((sum / busy as f64).round() as usize).max(1), util: 0.0 }))
        })
        .collect()
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

/// A Mac's share of a step's lane time over the last day (`Input::took`) under which the run
/// offers it none of the step (`skips`), unless it gave the step `SKIP_KEEP_H` or more.
pub const SKIP_SHARE: f64 = 0.1;
pub const SKIP_KEEP_H: f64 = 1.0;
/// How long into the run a Mac goes on skipping (`skips`): the day its shares were read over. Past
/// it every Mac is offered every step again, so a step only a busy Mac took isn't left to it for
/// weeks (the backtest, train: depth's end p90 78,057 min skipping throughout, 33,136 a day).
pub const SKIP_FOR_S: f64 = HELD_S as f64;

/// The lanes' time each of `macs` gave each step (hours, to the hundredth), over the `HELD_S`
/// before `now` (`Input::took`): its jobs done well, of each planned step (not `CLOCKED`) `macs`
/// did `STEP_PACE_JOBS` jobs of at least; a Mac that did none of such a step has it at 0.
pub fn took(
    events: &[crate::coord::history::Event],
    now: u64,
    macs: &[String],
) -> BTreeMap<String, BTreeMap<String, f64>> {
    let from = now.saturating_sub(HELD_S);
    let mac = worker_mac;
    // (Each step's jobs, and its seconds by Mac.)
    let mut per: BTreeMap<&str, (usize, BTreeMap<String, f64>)> = BTreeMap::new();
    for e in events.iter().filter(|e| e.t > from && e.t <= now && e.ok == Some(true)) {
        let (Some(step), Some(w), Some(secs)) = (e.step.as_deref(), e.worker.as_deref(), e.secs) else { continue };
        if !matches!(e.kind.as_str(), "done" | "end") || CLOCKED.contains(&step) || !macs.contains(&mac(w)) {
            continue;
        }
        let p = per.entry(step).or_default();
        p.0 += 1;
        *p.1.entry(mac(w)).or_default() += secs.max(0.0);
    }
    let mut out: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for (step, (_, by)) in per.into_iter().filter(|p| p.1 .0 >= STEP_PACE_JOBS) {
        for m in macs {
            let h = by.get(m).copied().unwrap_or(0.0) / 3600.0;
            out.entry(m.clone()).or_default().insert(step.to_string(), (h * 100.0).round() / 100.0);
        }
    }
    out
}

/// Each Mac's steps the run offers it none of (`Input::took`): those it gave under `SKIP_SHARE` of
/// their lane time over the last day and under `SKIP_KEEP_H` (a Mac asleep or away gave none), for
/// the run's first `SKIP_FOR_S`. What
/// the scheduler gives each Mac live (its memory and cores against each step's, the NAS budget, a
/// member's steps in its ask, which Mac asks first) shows in it; the run's scheduler, whose Macs all
/// ask at once with their room free, gives any step to any Mac (docs/pool.md §7.9).
fn skips(took: &BTreeMap<String, BTreeMap<String, f64>>) -> BTreeMap<String, Vec<String>> {
    let mut all: BTreeMap<&str, f64> = BTreeMap::new();
    for by in took.values() {
        for (s, h) in by {
            *all.entry(s).or_default() += h;
        }
    }
    took.iter()
        .map(|(m, by)| {
            let skip = by.iter().filter(|&(s, &h)| h < SKIP_SHARE * all[s.as_str()] && h < SKIP_KEEP_H);
            (m.clone(), skip.map(|(s, _)| s.clone()).collect())
        })
        .collect()
}

/// `machines` held to `held` (`held_back`): each Mac's lanes past its `lanes` dropped, but one
/// running a job.
pub fn hold_back(machines: Vec<Machine>, held: &BTreeMap<String, Held>) -> Vec<Machine> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    machines
        .into_iter()
        .filter(|m| {
            let Some(h) = held.get(&mac_of(m)) else { return true };
            let k = seen.entry(mac_of(m)).or_default();
            *k += 1;
            *k <= h.lanes || m.busy_s > 0.0 || m.running.is_some()
        })
        .collect()
}

/// The span of the history `step_pace` reads, and how long `Noted` keeps a target's cost after it
/// last saw it.
pub const STEP_PACE_S: u64 = 12 * 3600;
/// The jobs of a step `step_pace` needs before it says the step's pace.
const STEP_PACE_JOBS: usize = 3;
/// `step_pace`'s floor: a step's few jobs never make it more than fourfold faster.
const STEP_PACE_MIN: f64 = 0.25;

/// The cost each target was last forecast at while it was work left (`Input::costs`, and the
/// shared steps' targets of `chains`, `after` and `idle`), with when: what `step_pace` holds the
/// jobs done to. A target leaves the work left once built, so its cost stays the one the forecasts
/// before its job gave it, not the measure its job left. Kept `STEP_PACE_S` after it was last seen.
#[derive(Clone, Debug, Default)]
pub struct Noted {
    costs: std::collections::HashMap<(String, String), (f64, u64)>,
}

impl Noted {
    /// `inp`'s costs noted (at `inp.now`), those not seen for `STEP_PACE_S` dropped.
    pub fn note(&mut self, inp: &Input) {
        let now = inp.now;
        for (step, ts) in &inp.costs {
            for (t, c) in ts {
                self.costs.insert((step.clone(), t.clone()), (c.secs, now));
            }
        }
        for (step, t, c) in inp.chains.iter().chain(&inp.after).chain(&inp.idle) {
            if SHARED.contains(&step.as_str()) {
                self.costs.insert((step.clone(), t.clone()), (c.secs, now));
            }
        }
        self.costs.retain(|_, (_, at)| now.saturating_sub(*at) <= STEP_PACE_S);
    }

    /// The cost noted for `step`'s `target`, if any.
    pub fn cost(&self, step: &str, target: &str) -> Option<f64> {
        self.costs.get(&(step.to_string(), target.to_string())).map(|c| c.0)
    }
}

/// Each step's pace against its costs (`Input::step_pace`): over the jobs done well in the
/// `STEP_PACE_S` before `now` whose every target has a cost `noted`, the time they took at the
/// lead's pace (their seconds times their worker's `speed`) over the sum of their targets' costs;
/// a step with `STEP_PACE_JOBS` such jobs at least, `STEP_PACE_MIN` at least. A cost is its
/// target's last measure (an earlier build's) or its step's mean, and a step's code, its Mac's load
/// and its jobs' size change what a target takes now. Only a step whose jobs ran faster than their
/// costs is said: one slower (pack, pois, depth, terrain-lo on 24-27 Oct, 2-4 times) is so by what
/// each job takes whatever its targets (opening its packs, its inputs), which a target's pace would
/// lay on the run's jobs, many targets each: in the backtest that made the forecast worse, pack's
/// own the most (docs/pool.md §7.9).
pub fn step_pace(
    events: &[crate::coord::history::Event],
    now: u64,
    noted: &Noted,
    speed: &dyn Fn(&str) -> f64,
) -> BTreeMap<String, f64> {
    let from = now.saturating_sub(STEP_PACE_S);
    let mut sums: BTreeMap<&str, (f64, f64, usize)> = BTreeMap::new();
    for e in events.iter().filter(|e| e.t > from && e.t <= now && e.ok == Some(true)) {
        if !matches!(e.kind.as_str(), "done" | "end") || e.targets.is_empty() {
            continue;
        }
        let (Some(step), Some(w), Some(secs)) = (e.step.as_deref(), e.worker.as_deref(), e.secs) else { continue };
        let Some(cost) = e.targets.iter().map(|t| noted.cost(step, t)).sum::<Option<f64>>() else { continue };
        if cost <= 0.0 || secs <= 0.0 {
            continue;
        }
        let s = sums.entry(step).or_default();
        (s.0, s.1, s.2) = (s.0 + secs * speed(w), s.1 + cost, s.2 + 1);
    }
    sums.into_iter()
        .filter(|(_, s)| s.2 >= STEP_PACE_JOBS)
        .map(|(step, (took, cost, _))| (step.to_string(), ((took / cost * 100.0).round() / 100.0).max(STEP_PACE_MIN)))
        .filter(|(_, p)| *p < 1.0)
        .collect()
}

/// Each step's longest job (wall seconds) of those done well in the `STEP_PACE_S` before `now`,
/// a step with `STEP_PACE_JOBS` at least: what `cap_busy` holds a lane's job under way to.
pub fn longest_jobs(events: &[crate::coord::history::Event], now: u64) -> BTreeMap<String, f64> {
    let from = now.saturating_sub(STEP_PACE_S);
    let mut per: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
    for e in events.iter().filter(|e| e.t > from && e.t <= now && e.ok == Some(true)) {
        if !matches!(e.kind.as_str(), "done" | "end") {
            continue;
        }
        let (Some(step), Some(secs)) = (e.step.as_deref(), e.secs) else { continue };
        let p = per.entry(step).or_default();
        (p.0, p.1) = (p.0.max(secs), p.1 + 1);
    }
    per.into_iter().filter(|(_, p)| p.1 >= STEP_PACE_JOBS).map(|(s, p)| (s.to_string(), p.0)).collect()
}

/// Each lane's time left on its job under way (`Machine::busy_s`) held to its step's longest job
/// of late (`longest_jobs`), a minute at least: a job's time left is its targets left at its slot's
/// pace (an unmeasured slot's a guess, its Mac's hold on top) or its own estimate, whichever is
/// longer, and on 24-27 Oct a lane said busy 4-24 h was free after about a quarter of that (the
/// median), one said busy a day or more after 0.2 %, its lane blocked meanwhile (docs/pool.md §7.9). Its step: its `running`'s, else that of the targets `running` puts on it.
pub fn cap_busy(inp: &mut Input, longest: &BTreeMap<String, f64>) {
    for (k, m) in inp.machines.iter_mut().enumerate() {
        let step = m
            .running
            .as_ref()
            .map(|r| r.0.as_str())
            .or_else(|| inp.running.iter().find(|(_, &on)| on == k).map(|((s, _), _)| s.as_str()));
        if let Some(&most) = step.and_then(|s| longest.get(s)) {
            m.busy_s = m.busy_s.min(most.max(60.0));
        }
    }
}

/// The lead's steps on its own clock, not the plan's dependencies: the catalogues and low zooms
/// published, the backups, the sweeps, the inputs' refreshes, the names' and timetables' todo
/// lists, the tasks. Left out of `Forecast::curve` and of the work `pace` counts done.
pub const CLOCKED: [&str; 8] =
    [store::catalogue::NAME, "lo", "backup", "gc", "inputs", "names-todo", "timetables-todo", "task"];

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
/// early). Said, not applied: what's known to hold work back is in the run (`Held`).
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

/// A job of `Input::before` (`step`, `target`) not ready before `at` (unix s), nor before the
/// run has done the jobs of `before` it waits on (`after`, (step, target); one not among them
/// counts done).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub step: String,
    pub target: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<(String, String)>,
}

/// A slot's job under way, as its lane's time left is worked out (`SlotJob::busy_s`): the lead's
/// forecast (crate::agent's `forecast_now`) and each Mac's page data (crate::agent::pageview, its
/// own lanes) alike.
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

    /// What its lane runs (`Machine::running`): its step and targets.
    pub fn running(&self) -> (String, usize) {
        (self.step.clone(), self.targets.len().max(1))
    }
}

/// A lane's time left (s): a job's own estimate (`eta`) or the time its work left takes
/// (`left`), whichever's longer, a minute at least.
pub fn busy_s(eta: f64, left: f64) -> f64 {
    eta.max(left).max(60.0)
}

/// What each target of the regions takes, by step, then target.
pub type CostTable = BTreeMap<String, BTreeMap<String, Cost>>;

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
    /// Its lanes' Macs, each once, in order.
    pub fn macs_named(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for m in &self.machines {
            let g = mac_of(m);
            if !out.contains(&g) {
                out.push(g);
            }
        }
        out
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
    pub fn forecast(&self) -> Forecast {
        let mut f = forecast(&self.input);
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
/// (build::LastCatalogue::low_at), the landmark points and the area overlays
/// (build::LastCatalogue::since).
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
    /// Each machine's speed against the lead's, as used, and the machines whose speed is a
    /// guess (not yet measured).
    pub speed: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guessed: Vec<String>,
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
    /// Each machine's work to the end, a step's run together (`Lane`): the schedule.
    #[serde(default)]
    pub lanes: BTreeMap<String, Vec<Lane>>,
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
    /// The live error (`Input::pace`) and what held each Mac back (`Input::held`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub held: BTreeMap<String, Held>,
    /// Every target the expected run placed: its lane (`Input::machines`' index), step, target and
    /// from and until when (unix s). Not in the status (it's large): the forecasts' archive's
    /// (crate::agent::fcarchive).
    #[serde(skip)]
    pub placed: Vec<Placed>,
    /// The schedule's runs (`lanes`), their targets and what each waits on: the build page's
    /// (`/work/tocome`, with `placed`'s tiles: `ToCome::runs`). Not in the status.
    #[serde(skip)]
    pub runs: Runs,
    /// The items' dependencies, what each job started from now on waited on is read from
    /// (crate::coord::history::Waits: the history's starts and leases name it). Not in the status.
    #[serde(skip)]
    pub graph: crate::coord::history::Graph,
}

/// The schedule's runs (`Forecast::lanes`) numbered in turn: the lanes by name, each lane's runs in
/// order (run k is the k-th of that list). Each run's targets (its items', in order; a job under way
/// that no item is, none), and the runs each waits on: [a, b] when an item of run b waits on one of
/// run a (its own deps, a time-ordered release's, or a set's it waits on whole: `Items::groups`),
/// each pair once, sorted; a run never on itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runs {
    pub targets: Vec<Vec<String>>,
    pub deps: Vec<[u32; 2]>,
    /// Each item's run (`Forecast::graph`'s order): for the agent's forecast thread alone, taken
    /// out before the answer's kept.
    pub item_run: Vec<Option<u32>>,
    /// Each run's finished dependencies, the history's event numbers (`Waits::by_run`; empty
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
/// world map's queue, every tile left to build (task #685; `/work/tocome`, asked once a forecast;
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

/// Whether `t` names a tile ("z/x/y", z under 32, x and y in its grid), as the page's `tileOf`.
fn is_tile(t: &str) -> bool {
    let mut it = t.split('/').map(|p| p.parse::<u64>().ok());
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(Some(z)), Some(Some(x)), Some(Some(y)), None) => z < 32 && x < 1 << z && y < 1 << z,
        _ => false,
    }
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

    /// `/work/tocome`'s answer: {at, macs: {mac: {step: [[tile, s from at]…]}}, runs: [[target…]…],
    /// deps: [[a, b]…], after: [[seq…]…]}, every Mac's; runs numbered as `Runs` says, `after` each
    /// run's finished dependencies (the history's event numbers; the coordinator adds the `epoch`
    /// they're numbered in).
    pub fn answer(&self) -> serde_json::Value {
        serde_json::json!({ "at": self.at, "macs": self.macs, "runs": self.runs.targets, "deps": self.runs.deps, "after": self.runs.after })
    }

    /// `f`'s tiles to come and runs (at `f.at`: the forecast's own, task #836), taken out of it:
    /// its `placed` dropped, its runs' `item_run` handed back for the waits (`Waits::by_run`);
    /// `machines` the lanes it was run on (each lane's Mac, else its name). The lead's agent's and
    /// a pageview's alike.
    pub fn take(f: &mut Forecast, machines: &[Machine]) -> (ToCome, Vec<Option<u32>>) {
        let macs: Vec<String> = machines.iter().map(|m| m.mac.clone().unwrap_or_else(|| m.name.clone())).collect();
        let mut tc = ToCome::of(&f.placed, &macs, f.at);
        let item_run = std::mem::take(&mut f.runs.item_run);
        tc.runs = std::mem::take(&mut f.runs);
        f.placed = Vec::new();
        (tc, item_run)
    }
}

/// The forecast's code version, archived with each forecast (crate::agent::fcarchive): raised
/// whenever a change to the run (its order, its costs, its scheduler) would move its times, so the
/// accuracy work (`scenic forecast-check`) can tell one model's error from another's.
pub const VERSION: u32 = 3;

/// A run of a machine's work of one step: from and until when (unix seconds), and how many
/// targets. A run is one or more jobs as the scheduler grants them in the forecast's run, one after
/// another on the lane (two minutes apart at most; task #834): its targets (`Runs::targets`, in
/// time order) its jobs' in turn. `cuts`, each job's after the first: [when it starts, s from
/// `from`; how many of the run's named targets come before it (a job under way that no item is
/// names none)]; a job ends where the next starts (its gap
/// to it, under two minutes, drawn as its own). Left out for a run of one job. Every job to come is
/// so on the page, its targets named, at ~10 bytes a job past the first of a run (29 Oct's record,
/// 5,397 jobs over 2.5 days in 571 runs: the lanes 35 KB without the cuts, 91 KB with them).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lane {
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
    /// Its time at the lead's pace (seconds), and how many of its targets' times are measured.
    pub work_s: u64,
    pub known: usize,
    /// Whether helpers may do it.
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

/// What a chain's step waits for (crate::agent::build::landmarks_work): the candidates for the
/// pass's hiking-route ends; the summits, while held for the terrain packs (agent::plan puts them
/// in the landmarks chain then), for those; the peaks for every candidate, the pass's summits and the terrain packs; the
/// items' facts for every candidate; the rest of the heritage chain for the heritage sites; the
/// landmark points for those four; trains a day for their feeds; the 3D buildings' tiles for the
/// normalized files. By step: (the jobs before the
/// regions', the chains', the terrain's).
fn chain_deps(step: &str) -> (&'static [&'static str], &'static [&'static str], bool) {
    // (The chain steps before it: its row's `after`, crate::agent::steps; task #864.)
    let earlier = crate::agent::steps::row(step).map_or(&[][..], |r| r.after);
    let (pre, reads_terrain): (&'static [&'static str], bool) = match step {
        "pois" => (&["trailends"], false),
        "summits" => (&[], true),
        "peaks" => (&["summits"], true),
        "heritage" => (&["heritage-sites"], false),
        _ => (&[], false),
    };
    (pre, earlier, reads_terrain)
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
    /// Whether a helper may do it (a shared step's, not before the regions').
    shared: bool,
    /// Being built now (its machine free when it's done).
    running: bool,
    /// Which machine, from and until when (seconds from now).
    by: Option<usize>,
    from: f64,
    end: f64,
    /// The job it was granted in (`run`'s numbering, from 1; 0: none yet): the lane's runs
    /// (`Lane`) are its jobs.
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
/// and targets, and when (s from the forecast's start; unix seconds in a `Forecast`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub mac: String,
    pub step: String,
    pub targets: Vec<String>,
    pub at: f64,
}

/// The share of a region's z6 tiles reading stale terrain, built as the coverage wants them now, taken
/// to go stale once that terrain is built (`RegionLeft::expected`; which ones is known only then).
/// A round figure, not measured each time: the terrain fix (8 Oct 2026) made most of the north's and
/// every coast's z6 tiles stale, a little over half of those reading changed terrain, and the forecast
/// had counted none of them until their terrain was built (32 z6 tiles at 17:00, 149 by 19:00).
pub const EXPECTED_STALE: f64 = 0.6;

/// The jobs before the regions' that the z6 tiles wait for (build::plan).
const UNITS_NEED: [&str; 3] = ["heritage-sites", "reach", "buildings"];

/// The items listed so far (`items`): by step and target.
#[derive(Default)]
struct Index {
    of: BTreeMap<(String, String), usize>,
}

/// The work as items (`items`): each region's, and the sets of items many wait for, each listed
/// once (sorted, no repeats): a region's terrain that its z6 tiles wait for, all the terrain, all
/// before the steps after. (Lead memory, 25 Oct: each slope target had its own copy of its z3
/// tiles' terrain, about 6,750 items, 8.7 GB of the agents' 17 GB. Since #864 a slope target's own
/// list is only what it reads: a slope-deep's 15 at most, a slope-hi's 10 and its 256 z10 tiles'.)
struct Items {
    items: Vec<Item>,
    region_items: Vec<Vec<usize>>,
    groups: Vec<Vec<usize>>,
}

/// The work as items, each with the items it waits for.
fn items(inp: &Input) -> Items {
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
        if let Some(&i) = by.of.get(&(step.to_string(), target.to_string())) {
            return i;
        }
        let shared = SHARED.contains(&step) && phase != Phase::Before;
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
        by.of.insert((step.to_string(), target.to_string()), i);
        i
    };
    for (step, target, cost) in &inp.before {
        add(&mut out, &mut by_target, step, target, *cost, Phase::Before, (Vec::new(), Vec::new()));
    }
    let before: Vec<usize> = (0..out.len()).collect();
    // (The z6 tiles wait for the pass's heritage sites, reaches and roadside buildings, as the plan's
    // do; terrain for nothing.)
    let units_need: Vec<usize> =
        before.iter().copied().filter(|&i| UNITS_NEED.contains(&out[i].step.as_str())).collect();
    // A region at a time: its own terrain (the z6 tiles, then each z3 tile's assembly once its z6 tiles
    // are made), then its own z6 tiles (each once its region's terrain is built: the terrain it
    // reads); then every region's slope and tree cover, each target once the targets it reads are
    // (its row's declaration, as the planner's waits).
    let mut region_items: Vec<Vec<usize>> = vec![Vec::new(); inp.regions.len()];
    let item_of = |by: &Index, step: &str, t: &str| by.of.get(&(step.to_string(), t.to_string())).copied();
    // (The items a target of `step` waits on: those of the targets it reads, listed by now, in
    // order, once: crate::agent::steps::reads_of.)
    let reads = |by: &Index, step: &str, t: &str| -> Vec<usize> {
        let mut v: Vec<usize> =
            crate::agent::steps::reads_of(step, t).iter().filter_map(|(s, u)| item_of(by, s, u)).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
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
            add(
                &mut out,
                &mut by_target,
                "terrain-deep",
                t,
                inp.cost("terrain-deep", t),
                Phase::Region,
                (Vec::new(), Vec::new()),
            );
        }
        for q in &r.own_terrain_lo {
            let deps = reads(&by_target, "terrain-lo", q);
            add(
                &mut out,
                &mut by_target,
                "terrain-lo",
                q,
                inp.cost("terrain-lo", q),
                Phase::Region,
                (deps, Vec::new()),
            );
        }
        // (Each z6 tile once the terrain targets it reads are built, its row's declaration as the
        // planner's waits (its own region's or one's before it: listed by now), and the jobs all
        // wait for, one group.)
        let g = vec![group(&mut groups, units_need.clone())];
        for u in r
            .own_units
            .iter()
            .chain(r.expected.iter().take((r.expected.len() as f64 * EXPECTED_STALE).round() as usize))
        {
            // (The expected: those its stale terrain may make stale, a share, the first in its
            // order; `add` lists a z6 tile two regions name once.)
            let deps = reads(&by_target, "unit", u);
            add(&mut out, &mut by_target, "unit", u, inp.cost("unit", u), Phase::Region, (deps, g.clone()));
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
    // The slope: each target once what it reads is built, its row's declaration
    // (crate::agent::steps::Reads), the planner's waits.
    for r in inp.regions.iter() {
        for t in &r.slope_deep {
            let deps = reads(&by_target, "slope-deep", t);
            add(&mut out, &mut by_target, "slope-deep", t, inp.cost("slope-deep", t), Phase::Late, (deps, Vec::new()));
        }
        for t in &r.slope {
            let deps = reads(&by_target, "slope-hi", t);
            add(&mut out, &mut by_target, "slope-hi", t, inp.cost("slope-hi", t), Phase::Late, (deps, Vec::new()));
        }
        for q in &r.slope_lo {
            let deps = reads(&by_target, "slope-lo", q);
            add(&mut out, &mut by_target, "slope-lo", q, inp.cost("slope-lo", q), Phase::Late, (deps, Vec::new()));
        }
        for t in &r.trees {
            add(&mut out, &mut by_target, "trees", t, inp.cost("trees", t), Phase::Late, (Vec::new(), Vec::new()));
        }
        // A z3 tile's tree cover assembled once its z6 tiles are made (a region lists every z6 tile of
        // its z3 tiles: listed by now).
        for q in &r.trees_lo {
            let deps = reads(&by_target, "trees-lo", q);
            add(&mut out, &mut by_target, "trees-lo", q, inp.cost("trees-lo", q), Phase::Late, (deps, Vec::new()));
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
    // The chains: from the start, each step once what it reads is built.
    let terrain: Vec<usize> = (0..out.len())
        .filter(|&i| matches!(out[i].step.as_str(), "terrain-hi" | "terrain-deep" | "terrain-lo"))
        .collect();
    let terrain = vec![group(&mut groups, terrain)];
    // (What the live plan holds back (task #867): a chain's job its scheduler's offers lack, not
    // running, waits on something even where `chain_deps` says nothing: on its chain's jobs before
    // it (build::chains_left lists each chain's works as if each before it succeeded; a chain's
    // steps are its stage's, crate::agent::steps). None without a scheduler (a test, a backtest).)
    let offered: Option<BTreeSet<(&str, &str)>> = inp.sched.as_ref().map(|s| {
        s.offers.iter().flat_map(|o| o.targets.iter().map(move |t| (o.step.as_str(), t.0.as_str()))).collect()
    });
    let stage = |s: &str| super::steps::row(s).map(|r| r.stage);
    let mut chain: Vec<usize> = Vec::new();
    // (What a step's jobs wait on, one set for all of them (task #867: 1,328 peaks each listing the
    // 568 candidates were 0.75M of the kept waits' 1.07M deps): by step, its length when made.)
    let mut shared_waits: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (step, target, cost) in &inp.chains {
        let (pre, earlier, reads_terrain) = chain_deps(step);
        let mut deps: Vec<usize> = before.iter().copied().filter(|&i| pre.contains(&out[i].step.as_str())).collect();
        deps.extend(chain.iter().copied().filter(|&i| earlier.contains(&out[i].step.as_str())));
        let mut g = if reads_terrain { terrain.clone() } else { Vec::new() };
        if deps.len() > 1 {
            // (The list only grows, so one as long is the same.)
            let n = deps.len();
            let k = match shared_waits.get(step.as_str()) {
                Some(&(len, k)) if len == n => k,
                _ => {
                    let k = group(&mut groups, std::mem::take(&mut deps));
                    shared_waits.insert(step.as_str(), (n, k));
                    k
                }
            };
            deps.clear();
            g.push(k);
        }
        let held = offered.as_ref().is_some_and(|o| !o.contains(&(step.as_str(), target.as_str())))
            && !inp.running.contains_key(&(step.clone(), target.clone()));
        // (Not on its own step's jobs: its targets don't wait on each other, and a held step's all
        // after one another would be a line, not a step. Not the heritage chain: it runs beside
        // the candidates, build::landmarks_work.)
        if held && deps.is_empty() && g.is_empty() && step != "heritage" {
            let mine: Vec<usize> = chain
                .iter()
                .copied()
                .filter(|&i| out[i].step != *step)
                .filter(|&i| stage(&out[i].step).is_some() && stage(&out[i].step) == stage(step))
                .collect();
            if !mine.is_empty() {
                g.push(group(&mut groups, mine));
            }
        }
        let i = add(&mut out, &mut by_target, step, target, *cost, Phase::Chain, (deps, g));
        chain.push(i);
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
    Items { items: out, region_items, groups }
}

/// The items ready to take (their deps done, not yet taken), each phase's by step in order (an
/// ask's offers: `offers`), and the shared steps' by step for the pages' lane.
#[derive(Default)]
struct Ready {
    phase: [BTreeMap<String, BTreeSet<usize>>; 6],
    shared: BTreeMap<String, BTreeSet<usize>>,
}

impl Ready {
    fn insert(&mut self, i: usize, it: &Item, p: usize) {
        self.phase[p].entry(it.step.clone()).or_default().insert(i);
        if it.shared {
            self.shared.entry(it.step.clone()).or_default().insert(i);
        }
    }

    fn remove(&mut self, i: usize, it: &Item, p: usize) {
        let drop = |m: &mut BTreeMap<String, BTreeSet<usize>>| {
            if let Some(s) = m.get_mut(&it.step) {
                s.remove(&i);
                if s.is_empty() {
                    m.remove(&it.step);
                }
            }
        };
        drop(&mut self.phase[p]);
        if it.shared {
            drop(&mut self.shared);
        }
    }

    /// Phase `p`'s items ready, a step's together, the steps in the order their first comes.
    fn steps(&self, p: usize) -> Vec<Vec<usize>> {
        let mut v: Vec<Vec<usize>> =
            self.phase[p].values().map(|s| s.iter().copied().take(OFFER_MAX).collect()).collect();
        v.sort_by_key(|s| s[0]);
        v
    }
}

/// The most targets of a step one ask is offered in the run (`offers`): the scheduler takes a run
/// from the front of them (`JOB_S` of measured time at most), so the rest change nothing but its
/// time.
const OFFER_MAX: usize = 64;

/// How long a lane refused work waits before it asks again (s), when nothing ends sooner: an
/// agent asks every few seconds, and a wait on the NAS budget ages (crate::budget::nas::OWN_AGE).
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
}

impl Pool {
    /// The scheduler for `inp`'s run over `items`: the lead's state as recorded (`Input::sched`), or,
    /// with none, the measured items' costs and nothing out. Each Mac asks as its last ask said (one
    /// job a lane: `Ask::want` 1), its room as with none of its jobs reserved (the scheduler counts
    /// the leases out), no load from outside the pool; a Mac whose ask isn't recorded with its room
    /// (`Input::macs`), else a lane's memory and cores past any job's.
    fn new(inp: &Input, items: &[Item]) -> Pool {
        let t0 = Instant::now();
        let clock = Clock { now: t0, unix: inp.now };
        let mut pool = Pool {
            sched: Sched::from_state(inp.sched.as_ref().unwrap_or(&State::default()), clock, RUN_TTL),
            asks: BTreeMap::new(),
            t0,
            now: inp.now,
        };
        if inp.sched.is_none() {
            for it in items.iter().filter(|it| it.cost.known) {
                pool.learn(it);
            }
        }
        let sched = &mut pool.sched;
        if sched.lead_name.is_empty() {
            sched.lead_name = inp.machines.first().map(mac_of).unwrap_or_default();
        }
        for m in inp.machines.iter().filter(|m| !m.helper) {
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
            let cores = inp.sched.as_ref().map(|s| s.cores_left(&a));
            if let Some(c) = a.cap.as_mut() {
                a.disk_mb = a.disk_mb.map(|d| d + c.reserved_mb);
                c.cores = cores.unwrap_or(c.cores);
                (c.held_mb, c.reserved_mb, c.cores_free) = (0, 0, c.cores);
                c.leases.clear();
            }
            pool.asks.insert(g, a);
        }
        pool
    }

    /// Item `it`'s measure taken (its job ended, or it was measured before the run): its memory
    /// and time, as a hand-off's costs, where none is kept already.
    fn learn(&mut self, it: &Item) {
        let k = cost_key(&it.step, &it.target);
        if self.sched.costs.contains_key(&k) {
            return;
        }
        self.sched.costs.insert(
            k,
            crate::coord::Cost {
                peak_mb: it.cost.peak_mb,
                secs: it.cost.secs.round() as u64,
                v: crate::coord::cost_version(&it.step),
                ..Default::default()
            },
        );
        self.sched.needs_rev += 1;
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

/// One run through the work, each item's time `scale`d. Each lane, as it's free, asks the
/// coordinator's scheduler for a job (crate::coord::sched::Sched::pack, the live coordinator's own
/// decision) on its Mac's behalf, the work ready offered in its plan order; the job granted runs
/// its targets one after another at the lane's pace, its lease out until it ends. Refused, a lane
/// asks again once a job ends (or `ASK_AGAIN_S`). The pages' lane takes the shared steps' targets
/// alone, as tasks.
/// Cut once its thread has computed `budget`, or after `BACKSTOP` times it on the wall clock
/// (`Sim::cut`, `thread_cpu`).
fn run(inp: &Input, scale: &dyn Fn(&Cost) -> f64, budget: Duration) -> Sim {
    run_on(inp, items(inp), scale, budget)
}

/// `run` on the items given.
fn run_on(inp: &Input, its: Items, scale: &dyn Fn(&Cost) -> f64, budget: Duration) -> Sim {
    let (cpu0, wall0) = (thread_cpu(), Instant::now());
    let Items { mut items, region_items, groups } = its;
    for it in &mut items {
        it.cost.secs = (scale(&it.cost) * inp.step_pace.get(&it.step).copied().unwrap_or(1.0)).max(1.0);
    }
    let mut pool = Pool::new(inp, &items);
    let skip = skips(&inp.took);
    let n = inp.machines.len();
    let mut free: Vec<f64> = inp.machines.iter().map(|m| m.busy_s.max(m.from_s).max(0.0)).collect();
    // What's being built now: done when its machine is free.
    let at: BTreeMap<(String, String), usize> =
        items.iter().enumerate().map(|(i, it)| ((it.step.clone(), it.target.clone()), i)).collect();
    let running: Vec<(usize, usize)> = inp.running.iter().filter_map(|(k, &m)| at.get(k).map(|&i| (i, m))).collect();
    for r in &inp.release {
        if let Some(&i) = at.get(&(r.step.clone(), r.target.clone())) {
            let more: Vec<usize> = r.after.iter().filter_map(|k| at.get(k).copied()).filter(|&d| d != i).collect();
            items[i].deps.extend(more);
        }
    }
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
    // (Those not ready before a time (`Input::release`): counted as waiting until then, the
    // latest first.)
    let mut timed: Vec<(f64, usize)> = inp
        .release
        .iter()
        .filter(|r| r.at > inp.now)
        .filter_map(|r| at.get(&(r.step.clone(), r.target.clone())).map(|&i| ((r.at - inp.now) as f64, i)))
        .filter(|&(_, i)| items[i].by.is_none())
        .collect();
    timed.sort_by(|a, b| b.0.total_cmp(&a.0));
    for &(_, i) in &timed {
        pending[i] += 1;
    }
    let mut ready = Ready::default();
    for (i, it) in items.iter().enumerate() {
        if it.by.is_none() && pending[i] == 0 {
            ready.insert(i, it, pi(it.phase));
        }
    }
    // The regions' terrain, z6 tiles and map tiles: how many not yet taken, and when the last taken
    // ends; each region's slope and tree cover; each item's regions.
    let mut rp_left = vec![0usize; region_items.len()];
    let mut rp_end = vec![0.0f64; region_items.len()];
    let mut late_of: Vec<Vec<usize>> = vec![Vec::new(); region_items.len()];
    let mut regions_of: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    for (k, mine) in region_items.iter().enumerate() {
        for &i in mine {
            regions_of[i].push(k);
            match items[i].phase {
                Phase::Region | Phase::Publish if items[i].by.is_none() => rp_left[k] += 1,
                Phase::Region | Phase::Publish => rp_end[k] = rp_end[k].max(items[i].end),
                Phase::Late => late_of[k].push(i),
                _ => {}
            }
        }
    }
    let mut region_left = items.iter().filter(|i| i.phase == Phase::Region && i.by.is_none()).count();
    let mut region_end =
        items.iter().filter(|i| i.phase == Phase::Region && i.by.is_some()).map(|i| i.end).fold(0.0, f64::max);
    let mut before_left = items.iter().filter(|i| i.phase == Phase::Before && i.by.is_none()).count();
    let mut grants: Vec<Grant> = Vec::new();
    // (The state's changes counted (a job granted or ended), and each Mac's ask refused (by its
    // Mac, whether it may run the lead's own steps, and its lane's order) at which count and time:
    // asked again unchanged, at the same time, the scheduler answers the same, so the lanes of a
    // Mac woken together ask it once, task #544.)
    let mut changes = 0u64;
    let mut pruned_at = f64::NEG_INFINITY;
    let mut refused: BTreeMap<(String, bool, bool), (u64, f64)> = BTreeMap::new();
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
        // The machine free first (the lead on a tie).
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
            g.retain(|_, (_, last)| now.saturating_duration_since(*last) < Duration::from_secs(120));
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
        while timed.last().is_some_and(|r| r.0 <= t + 1e-9) {
            let (_, w) = timed.pop().unwrap_or_default();
            changes += 1;
            pending[w] -= 1;
            if pending[w] == 0 {
                ready.insert(w, &items[w], pi(items[w].phase));
            }
        }
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
                    ready.insert(w, &items[w], pi(items[w].phase));
                }
            }
        }
        // (A job of a step never measured leaves its targets' measures, as a real one's hand-off
        // does: the step's next jobs go beside others by them. Its later jobs' measures change
        // little, and would make the measures' models again each time.)
        out.retain(|(end, id, its)| {
            let on = *end > t + 1e-9;
            if !on {
                changes += 1;
                pool.sched.end(*id);
                for &i in its.iter() {
                    if pool.sched.never_measured(&items[i].step) {
                        pool.learn(&items[i]);
                    }
                }
            }
            on
        });
        let mac = &inp.machines[m];
        // (A lane given nothing asks again once an item or another lane's job ends, or a lease
        // out, or in a while.)
        let next_end = |items: &[Item], free: &[f64], out: &[(f64, u64, Vec<usize>)]| {
            let others = free.iter().enumerate().filter(|&(k, &f)| k != m && f > t + 1e-9).map(|(_, &f)| f);
            inflight
                .iter()
                .map(|&i| items[i].end)
                .chain(others)
                .chain(out.iter().map(|o| o.0))
                .chain(timed.last().map(|r| r.0))
                .fold(f64::INFINITY, f64::min)
        };
        // The order the lane takes the work ready in: the lead's first lane, while the regions have
        // work left, the pass's first, then the map tiles of the z6 tiles built (publishing comes
        // before the regions' work), then the slope and tree cover of a region on the map whose
        // catalogue doesn't record it built yet, then the regions' (in order), the chains' with none
        // it can do now; after the last z6 tile and terrain, the map tiles left, the slope and tree
        // cover, the chains', then what comes after. Every other lane: the plan's order.
        let key = (!mac.helper).then(|| (mac_of(mac), m == 0 || mac.lead_steps, mac.equal));
        let asked = key.as_ref().is_some_and(|k| refused.get(k) == Some(&(changes, t)));
        let order: Vec<Vec<usize>> = if mac.helper || asked {
            Vec::new()
        } else if mac.equal {
            PHASES.iter().flat_map(|&p| ready.steps(pi(p))).collect()
        } else if region_left > 0 || region_end > t + 1e-9 || before_left > 0 {
            let prep: Vec<usize> = (0..inp.regions.len())
                .filter(|&k| inp.regions[k].on_map != Some(true) && rp_left[k] == 0 && rp_end[k] <= t + 1e-9)
                .flat_map(|k| late_of[k].iter().copied())
                .filter(|&i| items[i].by.is_none() && pending[i] == 0)
                .take(OFFER_MAX)
                .collect();
            let mut o = ready.steps(pi(Phase::Before));
            o.extend(ready.steps(pi(Phase::Publish)));
            o.extend(prep.into_iter().map(|i| vec![i]));
            o.extend(ready.steps(pi(Phase::Region)));
            o.extend(ready.steps(pi(Phase::Chain)));
            o
        } else {
            [Phase::Publish, Phase::Late, Phase::Chain, Phase::After].iter().flat_map(|&p| ready.steps(pi(p))).collect()
        };
        // What it takes: a job the scheduler grants its Mac (its targets one after another), or
        // for the pages' lane the far end of the first shared step it can do (terrain-hi's near end:
        // the lead's next z6 tiles wait on it, as crate::coord picks).
        let took: Vec<(usize, u32)> = if mac.helper {
            SHARED
                .iter()
                .find_map(|s| {
                    let set = ready.shared.get(*s)?;
                    let fits = |&i: &usize| items[i].cost.peak_mb <= mac.mem_mb;
                    if *s == "terrain-hi" {
                        set.iter().copied().find(fits)
                    } else {
                        set.iter().rev().copied().find(fits)
                    }
                })
                .into_iter()
                .map(|i| {
                    jobs += 1;
                    (i, jobs)
                })
                .collect()
        } else {
            let key = key.unwrap_or_default();
            let (g, lead) = (key.0.clone(), key.1);
            let picks = if asked {
                Vec::new()
            } else {
                let ask = &pool.asks[&g];
                let skips = if t < SKIP_FOR_S { skip.get(&g).map_or(&[][..], |s| &s[..]) } else { &[][..] };
                let offers = offers(&items, &order, ask, skips);
                // (Every member asking, as the live ones do each loop: placement draws among those
                // asking lately, which aged out as the run's seconds passed.)
                let idle = |g: &str| {
                    let on = |k: usize| !inp.machines[k].helper && mac_of(&inp.machines[k]) == g;
                    (0..free.len()).filter(|&k| on(k) && free[k] <= t + 1e-9).count()
                };
                let room = |g: &str| pool.asks[g].cap.as_ref().map_or(0, |c| c.mem_mb);
                let who: Vec<(&str, usize, u64)> = pool.asks.keys().map(|g| (g.as_str(), idle(g), room(g))).collect();
                pool.sched.asking(who, pool.clock(t).now);
                pool.sched.pack(ask, &offers, lead, pool.clock(t)).0
            };
            if picks.is_empty() {
                refused.insert(key, (changes, t));
            } else {
                changes += 1;
            }
            let mut took = Vec::new();
            for p in picks {
                jobs += 1;
                let mut end = t;
                let ids: Vec<usize> =
                    p.targets.iter().filter_map(|x| at.get(&(p.step.clone(), x.0.clone())).copied()).collect();
                for &i in &ids {
                    let extra = if items[i].step == "unit" { mac.unit_extra_s } else { 0.0 };
                    end += items[i].cost.secs / mac.speed.max(0.01) + extra;
                }
                out.push((end, p.lease, ids.clone()));
                if t < GRANTS_S {
                    grants.push(Grant {
                        mac: g.clone(),
                        step: p.step.clone(),
                        targets: p.targets.iter().map(|x| x.0.clone()).collect(),
                        at: t,
                    });
                }
                took.extend(ids.iter().map(|&i| (i, jobs)));
            }
            took
        };
        if took.is_empty() {
            // (Nothing in flight anywhere: nothing it waits on will change.)
            let next = next_end(&items, &free, &out);
            free[m] = if mac.helper || !next.is_finite() { next } else { next.min(t + ASK_AGAIN_S) };
            continue;
        }
        // Taken: one after another at the machine's pace; the counts kept.
        let mut start = t;
        for (i, job) in took {
            let extra = if items[i].step == "unit" { mac.unit_extra_s } else { 0.0 };
            let end = start + items[i].cost.secs / mac.speed.max(0.01) + extra;
            (items[i].by, items[i].from, items[i].end, items[i].job) = (Some(m), start, end, job);
            start = end;
            inflight.push(i);
            unassigned -= 1;
            match items[i].phase {
                Phase::Region => {
                    region_left -= 1;
                    region_end = region_end.max(end);
                }
                Phase::Before => before_left -= 1,
                _ => {}
            }
            if matches!(items[i].phase, Phase::Region | Phase::Publish) {
                for &k in &regions_of[i] {
                    rp_left[k] -= 1;
                    rp_end[k] = rp_end[k].max(end);
                }
            }
            ready.remove(i, &items[i], pi(items[i].phase));
        }
        free[m] = start;
    }
    let done = items.iter().all(|i| i.by.is_some());
    Sim { items, region_items, done, grants, cut, groups }
}

/// How far into the run its grants are kept (`Forecast::grants`, s): what the lead's real grants are
/// held to (`departures`).
const GRANTS_S: f64 = 3600.0;

/// The work ready, a step's together in `order`, as the coordinator offers it to `ask`'s Mac
/// (crate::agent's `offer_plan`): a step offered to every member one offer, its targets in order
/// (`OFFER_MAX` at most), where its work first comes; the lead's own steps' each a planned job of
/// its own (`Offer::job`); only the steps the Mac does (`Ask::can`, all when it says none), none it
/// skips (`skips`).
fn offers(items: &[Item], order: &[Vec<usize>], ask: &Ask, skip: &[String]) -> Vec<Offer> {
    let mut out: Vec<Offer> = Vec::new();
    let mut of_step: BTreeMap<&str, usize> = BTreeMap::new();
    // (Whether the last step seen is offered to every member: a group is one step's.)
    let mut last: Option<(&str, bool)> = None;
    for group in order {
        for &i in group {
            let it = &items[i];
            if (!ask.can.is_empty() && !ask.can.contains(&it.step)) || skip.contains(&it.step) {
                break;
            }
            // (Its size as offered: a z6 tile's or candidates' extract bytes, sizing their first
            // guess at the memory its cost says; another step's its memory.)
            let size = match it.step.as_str() {
                "unit" | "pois" => (it.cost.peak_mb << 20) / 10,
                _ => it.cost.peak_mb,
            };
            let target = (it.target.clone(), KEY.to_string(), size);
            let offered = match last {
                Some((st, o)) if st == it.step => o,
                _ => {
                    let o = super::every::offered(&it.step);
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

/// How often a forecast is made again at most: the lead's for its status (crate::agent), a page's
/// view's on every Mac while its input is unchanged (crate::agent::pageview).
pub const EVERY: Duration = Duration::from_secs(60);

/// How long each of a forecast's three runs may compute (its own thread's CPU time, not wall
/// time): past it, the run stops where it is (`Sim::cut`) and the forecast is partial
/// (`Forecast::partial`). It runs off the agent's loop (crate::agent's `start_forecast`); this bounds
/// a run at a scale it was never measured at (task #544). A quarter of `EVERY`: the three runs side
/// by side, a forecast takes a quarter of the cadence's CPU at most on each of three cores, on every
/// Mac (the pages' views run it too). CPU time, task #584: at the utility QoS a loaded Mac starves
/// the runs (the mini at load 34: 4 s of CPU took 15 s of wall), and a wall-time cut then hid the
/// finish of an input no bigger than usual. A run at live scale (the worldwide rebuild's ~30,000
/// items, 25 Oct) computes 4-7 s on the M4.
pub const BUDGET: Duration = Duration::from_secs(EVERY.as_secs() / 4);

/// A run is cut too after this many times its budget on the wall clock (120 s for `BUDGET`), so a
/// starved one can't hold the lead's next forecast (`Agent::forecasting`) nor the NAS self-heal's
/// wait for idle threads indefinitely.
const BACKSTOP: u32 = 8;

/// The calling thread's CPU time so far (in the browser's build, which has no such clock, the
/// wall time since the first call: a page's forecast runs alone in its worker).
#[cfg(target_family = "wasm")]
fn thread_cpu() -> Duration {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed()
}

/// The calling thread's CPU time so far.
#[cfg(not(target_family = "wasm"))]
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
    move |c| c.secs * if c.known { known } else { guess }
}

/// The forecast for `inp`, within `BUDGET`.
pub fn forecast(inp: &Input) -> Forecast {
    forecast_by(inp, BUDGET)
}

/// The forecast for `inp`, each run cut once it has computed `budget`.
pub fn forecast_by(inp: &Input, budget: Duration) -> Forecast {
    // (Two machines under one name would share its lanes while `run_of` numbers both: task #816's
    // panic in `runs_of`. A bug upstream; keep each name's first, say so, and go on.)
    let mut seen = std::collections::BTreeSet::new();
    if !inp.machines.iter().all(|m| seen.insert(m.name.as_str())) {
        let mut seen = std::collections::BTreeSet::new();
        let mut once = inp.clone();
        once.machines.retain(|m| seen.insert(m.name.clone()));
        eprintln!("forecast: machines listed twice under one name, kept the first of each (a bug)");
        return forecast_by(&once, budget);
    }
    if inp.paused {
        return Forecast {
            at: inp.now,
            why: Some(PAUSED.into()),
            measured: 1.0,
            pace: inp.pace.clone(),
            held: inp.held.clone(),
            ..Default::default()
        };
    }
    let now = inp.now as f64;
    // (The three runs side by side, each on a thread of its own.)
    let (expected, soon, late) = std::thread::scope(|s| {
        let soon = s.spawn(|| {
            low_priority();
            run(inp, &scaled(SCALINGS[1]), budget)
        });
        let late = s.spawn(|| {
            low_priority();
            run(inp, &scaled(SCALINGS[2]), budget)
        });
        let expected = run(inp, &scaled(SCALINGS[0]), budget);
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
        held: inp.held.clone(),
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
    f.speed = inp.machines.iter().map(|m| (m.name.clone(), (m.speed * 100.0).round() / 100.0)).collect();
    f.guessed = inp.machines.iter().filter(|m| !m.measured).map(|m| m.name.clone()).collect();
    // (From 0, not the empty sum's -0.)
    let total: f64 = expected.items.iter().map(|i| i.cost.secs).fold(0.0, |a, b| a + b);
    let known: f64 = expected.items.iter().filter(|i| i.cost.known).map(|i| i.cost.secs).fold(0.0, |a, b| a + b);
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
            known: mine.iter().filter(|i| i.cost.known).count(),
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
    // or terrain is left (build::Plan::idle), whichever comes first.
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
        // its jobs (`Lane`).
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
        for (k, r) in lane_runs(runs.collect(), now).into_iter().enumerate() {
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

/// A lane's runs (`Lane`) from its placings, `now` the forecast's time (unix s): each its items in
/// time order. A step's jobs one after another (two minutes apart at most) are one run, each job
/// after the first a cut.
fn lane_runs(mut placed: Vec<Placing>, now: f64) -> Vec<(Lane, Vec<usize>)> {
    // (The job under way first.)
    placed.sort_by(|a, b| a.from.total_cmp(&b.from).then(a.item.is_some().cmp(&b.item.is_some())));
    let at = |s: f64| (now + s).round() as u64;
    let mut out: Vec<(Lane, Vec<usize>, u32)> = Vec::new();
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
                Lane {
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
    lanes: &BTreeMap<String, Vec<Lane>>,
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
    // (Each run's targets in time order, as its jobs' cuts count them: `lane_runs`.)
    let mut by_run: Vec<Vec<usize>> = vec![Vec::new(); total as usize];
    for (k, r) in ids.iter().enumerate() {
        if let Some(r) = r {
            by_run[*r as usize].push(k);
        }
    }
    let targets: Vec<Vec<String>> = by_run
        .into_iter()
        .map(|mut v| {
            v.sort_by(|&a, &b| sim.items[a].from.total_cmp(&sim.items[b].from));
            v.into_iter().map(|k| sim.items[k].target.clone()).collect()
        })
        .collect();
    // (A set's runs, worked out once: the sets are shared by many items.)
    let group_runs: Vec<Vec<u32>> = sim
        .groups
        .iter()
        .map(|g| {
            let mut v: Vec<u32> = g.iter().filter_map(|&d| ids.get(d).copied().flatten()).collect();
            v.sort_unstable();
            v.dedup();
            v
        })
        .collect();
    let mut deps: std::collections::BTreeSet<[u32; 2]> = std::collections::BTreeSet::new();
    for (k, it) in sim.items.iter().enumerate() {
        let Some(b) = ids[k] else { continue };
        let on = it.deps.iter().filter_map(|&d| ids.get(d).copied().flatten());
        let on_sets = it.groups.iter().filter_map(|&g| group_runs.get(g)).flatten().copied();
        deps.extend(on.chain(on_sets).filter(|&a| a != b).map(|a| [a, b]));
    }
    Runs { targets, deps: deps.into_iter().collect(), item_run: ids, after: Vec::new() }
}

/// Where the lead's real grants departed from forecast `f`'s next steps: each Mac and step granted
/// more or fewer jobs from when it was made until `until` (unix s) than its run granted by then
/// (`Forecast::grants`, its first hour), by 2 or more and by more than half the forecast's
/// (`departed`): ((Mac, step), in words). `real`: the grants made, (unix s, Mac, step).
/// What the scheduler can't see moves them (docs/pool.md §7.8): a job ending sooner or later than
/// measured, the owner's pause, the NAS slower than its probe.
pub fn departures(f: &Forecast, real: &[(u64, String, String)], until: u64) -> Vec<((String, String), String)> {
    let until = until.min(f.at + GRANTS_S as u64);
    let mut n: BTreeMap<(&str, &str), (usize, usize)> = BTreeMap::new();
    for g in f.grants.iter().filter(|g| (g.at as u64) < until) {
        n.entry((&g.mac, &g.step)).or_default().0 += 1;
    }
    for (_, mac, step) in real.iter().filter(|r| r.0 >= f.at && r.0 < until) {
        n.entry((mac, step)).or_default().1 += 1;
    }
    n.into_iter()
        .filter(|(_, (fc, got))| departed(*fc, *got))
        .map(|((mac, step), (fc, got))| {
            ((mac.to_string(), step.to_string()), format!("{mac}: {got} job(s) of {step} granted, the forecast's {fc}"))
        })
        .collect()
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
    events: &[crate::coord::history::Event],
    lead: &str,
    helpers: &[String],
    default: f64,
) -> BTreeMap<String, (f64, bool)> {
    // (worker, step) → seconds a target.
    let mut per: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for e in events {
        let (Some(w), Some(step), Some(secs)) = (&e.worker, &e.step, e.secs) else { continue };
        let ok = matches!(e.kind.as_str(), "end" | "done") && e.ok == Some(true) && !e.targets.is_empty();
        if ok && SHARED.contains(&step.as_str()) {
            per.entry((w.clone(), step.clone())).or_default().push(secs / e.targets.len() as f64);
        }
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    helpers
        .iter()
        .map(|h| {
            let mut ratios: Vec<f64> = SHARED
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
    #[test]
    fn a_machine_listed_twice_does_not_panic() {
        // (Task #816 review: two machines under one name shared its lanes while run_of numbered
        // both; runs_of indexed past them and the forecast thread panicked.)
        let regions = many(40);
        let c = mixed(false);
        let mut inp = two_macs(&regions, &c);
        let dup: Vec<Machine> = inp.machines[..4].to_vec();
        inp.machines.extend(dup);
        let fc = forecast(&inp);
        assert_eq!(fc, forecast(&two_macs(&regions, &c)));
        assert_eq!(fc.runs.targets.len(), fc.lanes.values().map(Vec::len).sum::<usize>());
    }

    /// A run's jobs (`Lane::cuts`): each one's start (unix s) and targets.
    fn jobs_of<'a>(l: &Lane, targets: &'a [String]) -> Vec<(u64, &'a [String])> {
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
        let f = forecast(&inp);
        assert!(!f.grants.is_empty());
        let runs: Vec<&Lane> = f.lanes.values().flatten().collect();
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
        let runs = lane_runs(placed, now);
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
        let l = Lane { step: "x".into(), from: 1, until: 2, n: 1, cuts: Vec::new(), drop: false };
        assert_eq!(serde_json::to_value(&l).unwrap(), serde_json::json!({"step": "x", "from": 1, "until": 2, "n": 1}));
        assert_eq!(
            serde_json::from_value::<Lane>(serde_json::json!({"step": "x", "from": 1, "until": 2, "n": 1})).unwrap(),
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

    /// The drops (`Input::drops`, build::Plan::drops) go first at their own cost, a run of their
    /// own shown as drops (`Lane::drop`), never merged with the step's builds: 29 Oct's 4,275 Alaska
    /// drops were the yukon's, last, at a build's 76 s each.
    #[test]
    fn drops_go_first_as_drops() {
        let mut r = region("yukon", &[], &[], &[]);
        r.own_terrain_deep = vec!["10/256/352".into(), "10/256/353".into()];
        r.terrain_deep = vec!["10/256/352".into(), "10/256/353".into(), "10/100/252".into()];
        let regions = vec![r];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        let drop = Cost { secs: 2.0, known: true, peak_mb: 150 };
        inp.drops = vec![
            ("terrain-deep".into(), "10/100/252".into(), drop),
            ("terrain-deep".into(), "10/100/253".into(), drop),
        ];
        let f = forecast(&inp);
        let lanes: Vec<&Lane> = f.lanes.values().flatten().filter(|l| l.step == "terrain-deep").collect();
        assert_eq!(lanes.len(), 2, "{lanes:?}");
        assert!(lanes[0].drop && lanes[0].n == 2 && lanes[0].until - lanes[0].from <= 5, "{lanes:?}");
        assert!(!lanes[1].drop && lanes[1].n == 2 && lanes[1].from >= lanes[0].until, "{lanes:?}");
        assert!(serde_json::to_value(lanes[1]).unwrap().get("drop").is_none(), "said only of a drop");
    }

    /// A map tile and the z6 tiles that draw it.
    fn tile(t: &str, units: &[&str]) -> (String, Vec<String>) {
        (t.into(), units.iter().map(|u| u.to_string()).collect())
    }

    fn mac(name: &str, speed: f64, helper: bool) -> Machine {
        Machine {
            name: name.into(),
            speed,
            measured: true,
            helper,
            equal: false,
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
        Cost { secs, known: step != "slope-hi", peak_mb: 1000 }
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
            after: vec![("marks".into(), "marks".into(), Cost { secs: 300.0, known: true, peak_mb: 0 })],
            idle: Vec::new(),
            machines,
            running: BTreeMap::new(),
            macs: BTreeMap::new(),
            sched: None,
            release: Vec::new(),
            held: BTreeMap::new(),
            pace: None,
            step_pace: Default::default(),
            paused: false,
            took: Default::default(),
        }
    }

    /// A job released later (`Release`, task #539) starts no sooner, its lane idle until then,
    /// and the job beside it goes now.
    #[test]
    fn a_released_job_waits_for_its_time_and_the_rest_go_now() {
        let c = Cost { secs: 100.0, known: true, peak_mb: 100 };
        let mut inp = input(&[], vec![mac("m4", 1.0, false)], &cost);
        inp.after.clear();
        inp.before = vec![("rail".into(), "a".into(), c), ("trains".into(), "b".into(), c)];
        inp.release = vec![Release { step: "rail".into(), target: "a".into(), at: inp.now + 1000, after: Vec::new() }];
        let sim = run(&inp, &|c| c.secs, far());
        let of = |s: &str| sim.items.iter().find(|i| i.step == s).map(|i| (i.from, i.end));
        assert_eq!(of("trains"), Some((0.0, 100.0)));
        assert_eq!(of("rail"), Some((1000.0, 1100.0)));
    }

    #[test]
    fn a_record_read_back_gives_the_same_forecast_compressed_or_not() {
        // (Task #409) its input, a target running.
        let regions =
            [region("a", &["6/1/1", "6/1/2"], &["u1", "u2"], &["6/1/1"]), region("b", &["6/2/1"], &["u3", "u4"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        inp.running.insert(("unit".into(), "u1".into()), 0);
        let mut lead = forecast(&inp);
        lead.each_s.insert("unit".into(), 300.0);
        let rec = Record { input: inp, each_s: lead.each_s.clone(), waiting: BTreeMap::new() };
        let b = rec.bytes().unwrap();
        assert_eq!(b[0], b'{', "small: plain JSON");
        assert_eq!(Record::from_bytes(&b).unwrap().forecast(), lead);
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
        let inp = input(&regions, vec![mac("m4", 1.0, false)], &big);
        let mut running = BTreeMap::new();
        running.insert(("unit".to_string(), "u1".to_string()), 1);
        let small = Machine { busy_s: 300.0, ..mac("m1", 1.0, false) };
        // (The member's slot a lane of any work that fits, `equal`.)
        let small = Machine { equal: true, lead_steps: true, ..small };
        let machines = vec![Machine { mem_mb: u64::MAX, ..mac("m4", 1.0, false) }, small];
        let f = forecast(&Input { machines, running, ..inp });
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
        let inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        let slot = |k: usize, busy_s: f64| Machine {
            equal: k > 0,
            lead_steps: true,
            mem_mb: u64::MAX,
            busy_s,
            running: Some(("unit".into(), 1)),
            mac: Some("m4".into()),
            ..mac(&super::super::slot_worker("m4", k), 1.0, false)
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
        let f = forecast(&Input { machines, running, ..inp });
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
        // Region a: three tree cover z6 tiles of z3 tile 3/3/2 and its assembly; a helper beside the
        // lead; and a z6 tile's mid made in idle time.
        let mut a = region("a", &[], &[], &[]);
        a.trees = vec!["6/28/16".into(), "6/28/17".into(), "6/29/16".into()];
        a.trees_lo = vec!["3/3/2".into()];
        let regions = [a];
        // (Each piece longer than a job's quarter hour: one a job.)
        let long = |step: &str, t: &str| Cost { secs: 1000.0, ..cost(step, t) };
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &long);
        inp.idle = vec![("trees".into(), "6/40/20".into(), Cost { secs: 100.0, known: false, peak_mb: 1000 })];
        let sim = run(&inp, &|c| c.secs, far());
        let item = |step: &str, t: &str| sim.items.iter().find(|i| i.step == step && i.target == t).unwrap();
        // The assembly once its z6 tiles are made (the member takes some), by the lead alone.
        let lo = item("trees-lo", "3/3/2");
        assert!(["6/28/16", "6/28/17", "6/29/16"].iter().all(|t| item("trees", t).end <= lo.from + 1e-9));
        assert!(sim.items.iter().any(|i| i.step == "trees" && i.by == Some(1)), "the helper's pieces");
        assert_eq!(lo.by, Some(0));
        // The mid last, after everything else.
        let idle = item("trees", "6/40/20");
        assert!(sim.items.iter().filter(|i| i.target != "6/40/20").all(|i| i.end <= idle.from + 1e-9));
        let f = forecast(&inp);
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
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        busy(&mut inp);
        inp.running.insert(("labels".into(), "labels".into()), 0);
        let f = forecast(&inp);
        assert_ne!(f.why.as_deref(), Some(NOTHING_LEFT));
        assert_eq!(f.done_at, Some(1_000_000 + 3600));
        // As the agent has it now, a running item of its own: on the schedule, and its step's.
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        busy(&mut inp);
        inp.before =
            vec![("labels".into(), "labels 2026-09-28".into(), Cost { secs: 2200.0, known: true, peak_mb: 0 })];
        inp.running.insert(("labels".into(), "labels 2026-09-28".into()), 0);
        let f = forecast(&inp);
        assert_eq!((f.why.as_deref(), f.done_at), (None, Some(1_000_000 + 3600)));
        assert_eq!(
            f.lanes["m4"].iter().map(|l| (l.step.as_str(), l.until - 1_000_000)).collect::<Vec<_>>(),
            [("labels", 3600)]
        );
        assert_eq!(f.steps.iter().map(|s| (s.step.as_str(), s.left)).collect::<Vec<_>>(), [("labels", 1)]);
        // A daily job (no item): not done until it ends either.
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        busy(&mut inp);
        let f = forecast(&inp);
        assert_eq!((f.why.as_deref(), f.done_at), (None, Some(1_000_000 + 3600)));
        // Free: nothing left.
        inp.machines[0].busy_s = 0.0;
        assert_eq!(forecast(&inp).why.as_deref(), Some(NOTHING_LEFT));
    }

    #[test]
    fn a_page_around_takes_each_unit_the_moment_its_job_gives_it() {
        // As below, a worker that takes tails around: each of the two z6 tiles 30 s longer.
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &["3/1/1"])];
        let f = forecast(&input(&regions, vec![Machine { unit_extra_s: 30.0, ..mac("m4", 1.0, false) }], &cost));
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 1460));
    }

    #[test]
    fn one_mac_runs_a_region_and_its_map_tiles_then_it_is_on_the_map() {
        let regions = [RegionLeft {
            packs: vec![tile("6/8/8", &["6/8/8"]), tile("6/8/9", &["6/8/9"])],
            ..region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &["3/1/1"])
        }];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
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
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        let unit = f.steps.iter().find(|s| s.step == "unit").unwrap();
        assert_eq!((unit.left, unit.work_s), (4, 1200));
        // The region's ready once they're built too: terrain 600, four z6 tiles 1200.
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 1800));
        // None once the terrain's built (none expected).
        let regions = [region("a", &[], &["6/8/8"], &[])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        assert_eq!(f.steps.iter().find(|s| s.step == "unit").unwrap().left, 1);
    }

    #[test]
    fn a_helper_takes_the_far_end_then_what_it_frees() {
        let a = RegionLeft {
            packs: vec![tile("6/8/8", &["6/8/8"]), tile("6/8/7", &["6/8/8"])],
            ..region("a", &["3/1/1"], &["6/8/8"], &["3/1/1"])
        };
        let regions = [a, region("b", &["3/2/2"], &["6/16/16"], &[])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost));
        // The M1 takes b's terrain (the far end) at half the pace (1,200 s), then b's z6 tile, its
        // terrain built (600 s at its pace); the lead a's terrain and z6 tile (900 s), its two map
        // tiles (200 s), then its slope (200 s), on the map by then. Each region on the map a
        // catalogue's minute after its last z6 tile and map tiles.
        assert_eq!(
            f.next["m1"].iter().map(|n| (n.step.as_str(), n.targets[0].as_str())).collect::<Vec<_>>(),
            [("terrain-hi", "3/2/2"), ("unit", "6/16/16")]
        );
        assert_eq!((f.regions[0].ready_at, f.regions[0].map_at), (Some(1_000_000 + 1300), Some(1_000_000 + 1160)));
        assert_eq!((f.regions[1].ready_at, f.regions[1].map_at), (Some(1_000_000 + 1800), Some(1_000_000 + 1860)));
    }

    #[test]
    fn a_helper_takes_the_next_regions_terrain_not_the_last_ones() {
        let regions = [
            region("a", &["3/1/1"], &["6/8/8"], &[]),
            region("b", &["3/2/2"], &["6/16/16"], &[]),
            region("c", &["3/3/3"], &["6/24/24"], &[]),
        ];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost));
        // The lead takes a's terrain; the M1 b's (whose z6 tiles the lead builds next), not c's.
        assert_eq!(
            f.next["m1"].first().map(|n| (n.step.as_str(), n.targets[0].as_str())),
            Some(("terrain-hi", "3/2/2"))
        );
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
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        let on = |k: usize| f.regions[k].map_at.map(|t| t - 1_000_000);
        assert_eq!([on(0), on(1), on(2)], [Some(300 + 100 + 60), Some(800 + 60), Some(1200 + 60)]);
        assert_eq!(
            f.lanes["m4"].iter().map(|l| l.step.as_str()).collect::<Vec<_>>(),
            ["unit", "pack", "unit", "pack", "unit", "pack", "marks"]
        );
        // A map tile two regions draw: with the first, and the second waits for it.
        let regions =
            [packs(region("a", &[], &["6/1/1"], &[]), "6/1/1"), packs(region("b", &[], &["6/2/2"], &[]), "6/1/1")];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        assert_eq!(f.steps.iter().find(|s| s.step == "pack").map(|s| s.left), Some(1));
        assert_eq!(f.regions[1].map_at, Some(1_000_000 + 700 + 60));
    }

    #[test]
    fn a_region_with_only_slope_or_tree_cover_left_has_no_time_to_the_map() {
        // (Task #259 review) its z6 tiles and map tiles done: on the map as it's built now already,
        // not "in a minute" each time the forecast is made; nor one on the map as it is.
        let regions =
            [region("a", &[], &[], &["3/1/1"]), RegionLeft { on_map: Some(true), ..region("b", &[], &["6/2/2"], &[]) }];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        assert_eq!((f.regions[0].map_at, f.regions[1].map_at), (None, None));
        assert!(f.regions[0].ready_at.is_some());
    }

    #[test]
    fn the_cadenced_publishes_go_by_their_cadence_or_once_nothing_is_left_to_build() {
        // Thirty z6 tiles (9,000 s) left. The low zooms last made an hour ago (due in two more), the
        // landmarks ten minutes ago (due in fifty), the overlays never seen run (due now).
        let units: Vec<String> = (0..30).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        inp.cadence = Cadence { since_low: Some(3600), since_marks: Some(600), since_overlays: None };
        let at = |f: &Forecast| f.publishes.iter().map(|p| (p.what.clone(), p.at - 1_000_000)).collect::<Vec<_>>();
        let f = forecast(&inp);
        assert_eq!(
            at(&f),
            [("low".into(), LOW_EVERY_S - 3600), ("marks".into(), MARKS_EVERY_S - 600), ("overlays".into(), 0)]
        );
        // Two z6 tiles left (600 s): each once they're built, if sooner.
        let regions = [region("a", &[], &refs[..2], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        inp.cadence = Cadence { since_low: Some(3600), since_marks: Some(600), since_overlays: Some(0) };
        assert_eq!(at(&forecast(&inp)), [("low".into(), 600), ("marks".into(), 600), ("overlays".into(), 600)]);
    }

    #[test]
    fn regions_are_built_one_at_a_time() {
        let regions = [region("a", &["3/1/1"], &["6/8/8"], &[]), region("b", &["3/2/2"], &["6/16/16"], &[])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        // a's terrain and z6 tile, then b's terrain and z6 tile: a done first.
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 900));
        assert_eq!(f.regions[1].ready_at, Some(1_000_000 + 900 + 900));
        assert_eq!(f.measured, 1.0);
    }

    #[test]
    fn a_region_whose_terrain_a_helper_builds_waits_for_it() {
        // One region: the M1 has its terrain now (900 s left); the lead waits for it.
        let regions = [region("a", &["6/8/8"], &["6/8/8"], &[])];
        let mut inp =
            input(&regions, vec![mac("m4", 1.0, false), Machine { busy_s: 900.0, ..mac("m1", 0.5, true) }], &cost);
        inp.running.insert(("terrain-hi".into(), "6/8/8".into()), 1);
        let f = forecast(&inp);
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 900 + 300));
    }

    #[test]
    fn the_runs_say_their_targets_and_what_each_waits_on() {
        // (Task #787.) One Mac, one region: its terrain, then its two z6 tiles (they wait on the
        // region's terrain, a set: `Items::groups`), after the reach they wait on too (its own dep).
        let regions = [region("a", &["6/8/8"], &["6/8/8", "6/8/9"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        inp.before = vec![("reach".into(), "reach".into(), Cost { secs: 600.0, known: true, peak_mb: 0 })];
        let f = forecast(&inp);
        let steps: Vec<&str> = f.lanes["m4"].iter().map(|l| l.step.as_str()).collect();
        let at = |s: &str| steps.iter().position(|&x| x == s).unwrap() as u32;
        assert_eq!(f.runs.targets.len(), steps.len(), "{steps:?}");
        // (Each z6 tile a job of its own, as the scheduler grants it: task #834.)
        assert_eq!(f.runs.targets[at("unit") as usize], ["6/8/8", "6/8/9"]);
        assert_eq!(f.lanes["m4"][at("unit") as usize].cuts.len(), 1);
        assert!(f.runs.deps.contains(&[at("terrain-hi"), at("unit")]), "{:?} {steps:?}", f.runs.deps);
        assert!(f.runs.deps.contains(&[at("reach"), at("unit")]), "{:?} {steps:?}", f.runs.deps);
        assert!(f.runs.deps.iter().all(|[a, b]| a != b), "never a run on itself");
        // Two Macs: the runs numbered by lane name, so a run on "m1" comes before "m4"'s.
        let mut two = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 1.0, true)], &cost);
        two.before = inp.before.clone();
        let f = forecast(&two);
        let n1 = f.lanes["m1"].len();
        assert_eq!(f.runs.targets.len(), n1 + f.lanes["m4"].len());
        let tc = ToCome { runs: f.runs.clone(), ..ToCome::default() }.answer();
        assert_eq!(tc["runs"].as_array().unwrap().len(), f.runs.targets.len());
        assert_eq!(tc["deps"].as_array().unwrap().len(), f.runs.deps.len());
    }

    #[test]
    fn a_helper_isnt_stranded_by_a_worldwide_job() {
        // The lead runs a worldwide job (no item) for an hour; the member takes the terrain at
        // once (terrain waits for nothing), then the z6 tiles once their region's terrain is built
        // and the job the z6 tiles need is done: it wakes when the lead is free.
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &[])];
        let mut inp =
            input(&regions, vec![Machine { busy_s: 0.0, ..mac("m4", 1.0, false) }, mac("m1", 1.0, true)], &cost);
        inp.before = vec![("reach".into(), "reach".into(), Cost { secs: 3600.0, known: true, peak_mb: 0 })];
        let f = forecast(&inp);
        assert_eq!(f.next["m1"][0].step, "terrain-hi");
        assert_eq!(f.next["m1"][0].from, 1_000_000);
        // Both z6 tiles after the hour: one each.
        let lanes: Vec<&str> = f.lanes["m1"].iter().map(|l| l.step.as_str()).collect();
        assert_eq!(lanes, ["terrain-hi", "unit"]);
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 3600 + 300));
    }

    #[test]
    fn nothing_left_is_no_finish_and_a_rebuilt_region_goes_on_the_map_again() {
        let none: [RegionLeft; 0] = [];
        let mut inp = input(&none, vec![mac("m4", 1.0, false)], &cost);
        inp.after = Vec::new();
        let f = forecast(&inp);
        assert_eq!((f.done_at, f.why.as_deref()), (None, Some("nothing left to build")));
        assert!(f.regions.is_empty());
        // A region on the map as it was (its outline changed): on the map as it is a minute after
        // its z6 tile; one the map has as it is, rebuilt z6 tiles or none left, no time.
        let regions = [
            RegionLeft { on_map: Some(false), ..region("a", &[], &["6/8/8"], &[]) },
            RegionLeft { on_map: Some(true), ..region("b", &[], &["6/9/8"], &[]) },
            RegionLeft { id: "c".into(), on_map: Some(true), ..Default::default() },
        ];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        let at = |k: usize| f.regions[k].map_at;
        assert_eq!((at(0), at(1), at(2)), (Some(1_000_000 + 300 + 60), None, None));
        // Work that can't all be listed: no finish, and why.
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        inp.blind = Some("the z6 tiles wait for the pass's reaches".into());
        let f = forecast(&inp);
        assert_eq!((f.done_at, f.why.as_deref()), (None, Some("the z6 tiles wait for the pass's reaches")));
    }

    /// A member's slot as the coordinator packs it: any step any Mac may run that fits it.
    fn member(name: &str) -> Machine {
        Machine { equal: true, lead_steps: false, ..mac(name, 1.0, false) }
    }

    #[test]
    fn a_members_slots_take_any_work_offered_to_it_but_the_leads_own() {
        // (Task #383) the lead's two slots and a member's two: the member's take z6 tiles (no helper's
        // far-end rule), never the landmark points (the lead's own step).
        let units: Vec<String> = (0..8).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let lead2 = Machine { equal: true, lead_steps: true, ..mac("m4 #2", 1.0, false) };
        let inp = input(&regions, vec![mac("m4", 1.0, false), lead2, member("m1"), member("m1 #2")], &cost);
        let f = forecast(&inp);
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
        let lead2 = || Machine { equal: true, lead_steps: true, ..mac("m4 #2", 1.0, false) };
        let mut one = input(&regions, vec![mac("m4", 1.0, false), lead2()], &cost);
        one.after = Vec::new();
        let mut two = input(&regions, vec![mac("m4", 1.0, false), lead2(), member("m1"), member("m1 #2")], &cost);
        two.after = Vec::new();
        let left = |i: &Input| (forecast(i).done_at.unwrap() - 1_000_000) as f64;
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
        let lead2 = Machine {
            equal: true,
            lead_steps: true,
            busy_s: 2400.0,
            running: Some(("depth".into(), 3)),
            ..mac("m4 #2", 1.0, false)
        };
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), lead2, member("m1")], &cost);
        inp.chains = vec![("pois".into(), "6/8/0".into(), Cost { secs: 100.0, known: true, peak_mb: 1000 })];
        let f = forecast(&inp);
        let first = &f.lanes["m4 #2"][0];
        assert_eq!((first.step.as_str(), first.from, first.until, first.n), ("depth", 1_000_000, 1_002_400, 3));
        let pois = f.lanes.values().flatten().find(|l| l.step == "pois").unwrap();
        assert_eq!(pois.from, 1_000_900, "after the three units each lane took first: {:?}", f.lanes);
    }

    #[test]
    fn a_macs_slots_share_its_room_as_the_coordinator_packs() {
        // (Task #383 review) a member of 8 cores (jobs packed at 2 each) and 8 GB, four slots;
        // z6 tiles of 3 GB: two at once by memory, not four.
        let units: Vec<String> = (0..8).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let big = |step: &str, t: &str| Cost { peak_mb: 3000, ..cost(step, t) };
        let slot =
            |k: usize| Machine { mac: Some("m1".into()), mem_mb: 8000, ..member(&super::super::slot_worker("m1", k)) };
        let lead = Machine { busy_s: 1e6, ..mac("m4", 1.0, false) };
        let mut inp = input(&regions, vec![lead, slot(0), slot(1), slot(2), slot(3)], &big);
        inp.after = Vec::new();
        inp.macs.insert("m1".into(), MacRoom { mem_mb: 8000, cores: 8 });
        let sim = run(&inp, &|c| c.secs, far());
        let most = |sim: &Sim| {
            (0..40)
                .map(|k| k as f64 * 100.0 + 1.0)
                .map(|t| sim.items.iter().filter(|i| i.by.is_some_and(|m| m > 0) && i.from <= t && i.end > t).count())
                .max()
                .unwrap()
        };
        assert_eq!(most(&sim), 2, "8 GB of 3 GB jobs");
        // By cores: 1 GB jobs on 8 cores at 2 each, four at once; on 4 cores, two.
        let small = |step: &str, t: &str| Cost { peak_mb: 1000, ..cost(step, t) };
        inp.costs = cost_table(&regions, &small);
        assert_eq!(most(&run(&inp, &|c| c.secs, far())), 4);
        inp.macs.insert("m1".into(), MacRoom { mem_mb: 8000, cores: 4 });
        assert_eq!(most(&run(&inp, &|c| c.secs, far())), 2);
        // A step never measured: its first job alone on its Mac, nothing beside it (task #499);
        // once it ends its measure packs the rest as above.
        let cold = |step: &str, t: &str| Cost { known: false, ..small(step, t) };
        inp.costs = cost_table(&regions, &cold);
        let sim = run(&inp, &|c| c.secs, far());
        let first =
            sim.items.iter().filter(|i| i.by.is_some_and(|m| m > 0)).min_by(|a, b| a.from.total_cmp(&b.from)).unwrap();
        assert!(sim
            .items
            .iter()
            .filter(|i| i.by.is_some_and(|m| m > 0))
            .all(|i| i.from >= first.end - 1e-9 || std::ptr::eq(i, first)));
        assert_eq!(most(&sim), 2, "on its 4 cores, as above");
    }

    #[test]
    fn a_slot_the_ramp_grows_to_opens_later() {
        // (Task #383 review) the lead's third slot opens in an hour: nothing on it before.
        let units: Vec<String> = (0..40).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let lead2 = Machine { equal: true, lead_steps: true, ..mac("m4 #2", 1.0, false) };
        let lead3 = Machine { equal: true, lead_steps: true, from_s: 3600.0, ..mac("m4 #3", 1.0, false) };
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false), lead2, lead3], &cost));
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
            equal: k > 0 || !lead,
            lead_steps: lead,
            mac: Some(host.into()),
            mem_mb: 16_000,
            ..mac(&super::super::slot_worker(host, k), 1.0, false)
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
        let f = forecast(&inp);
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
            equal: k > 0 || !lead,
            lead_steps: lead,
            mac: Some(host.into()),
            mem_mb: 16_000,
            ..mac(&super::super::slot_worker(host, k), 1.0, false)
        };
        let mut machines: Vec<Machine> =
            (0..4).map(|k| slot("m4", k, true)).chain((0..4).map(|k| slot("m1", k, false))).collect();
        // (A member slot busy with a job no item is, for 20 min.)
        machines[5].busy_s = 1200.0;
        machines[5].running = Some(("depth".into(), 1));
        machines.push(Machine { mem_mb: 4000, ..mac("pages", 1.0, true) });
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
            Cost { known: !(all_cold || k % 3 == 0), peak_mb: if k % 5 == 0 { 6000 } else { 1500 }, ..c }
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
            let f = forecast(&inp);
            let ms = super::thread_cpu().as_millis() - took;
            eprintln!(
                "the forecast took {ms} ms (3,000 units, {})",
                if all_cold { "all cold" } else { "cold and big mixed" }
            );
            assert!(f.done_at.is_some());
            assert!(ms < CPU_BOUND_MS, "{ms} ms of CPU");
        }
    }

    /// The trains' and the landmarks' chains, as the plan lists them.
    fn chains() -> Vec<Job> {
        let c = |secs: f64| Cost { secs, known: true, peak_mb: 1000 };
        vec![
            ("rail-feeds".into(), "rail-feeds".into(), c(200.0)),
            ("rail".into(), "rail".into(), c(500.0)),
            ("pois".into(), "6/8/8".into(), c(100.0)),
            ("pois".into(), "6/8/9".into(), c(100.0)),
            ("items".into(), "items".into(), c(1000.0)),
            ("heritage".into(), "heritage".into(), c(3000.0)),
            ("marks".into(), "marks".into(), c(300.0)),
        ]
    }

    #[test]
    fn the_leads_other_slots_build_units_beside_the_first() {
        let units: Vec<String> = (0..8).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let others = Machine { equal: true, mem_mb: 12_000, ..mac("m4 #2", 1.0, false) };
        // Half the z6 tiles, whoever's at the Mac (the owner's choice: no rule for a Mac in use).
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false), others.clone()], &cost));
        assert_eq!(
            f.lanes.get("m4 #2").map_or(0, |l| l.iter().filter(|x| x.step == "unit").map(|x| x.n).sum::<usize>()),
            4
        );
        // The lead's first job two hours from done: its other slots take the z6 tiles from now.
        let f = forecast(&input(&regions, vec![Machine { busy_s: 7200.0, ..mac("m4", 1.0, false) }, others], &cost));
        let lane = &f.lanes["m4 #2"];
        assert_eq!(
            (lane[0].step.as_str(), lane[0].from, lane.iter().filter(|x| x.step == "unit").map(|x| x.n).sum::<usize>()),
            ("unit", 1_000_000, 8)
        );
    }

    #[test]
    fn a_helper_takes_the_candidates_while_the_units_wait() {
        let regions = [region("a", &["6/8/8"], &["6/8/8", "6/8/9"], &["6/8/8"])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        (inp.chains, inp.after) = (chains(), Vec::new());
        let f = forecast(&inp);
        // While the lead builds the terrain the z6 tiles wait for, the member makes the candidates
        // (from the far end), 200 s each at its pace, each a job of its own (task #834).
        let m1 = &f.lanes["m1"][0];
        assert_eq!((m1.step.as_str(), m1.from, m1.until, m1.n), ("pois", 1_000_000, 1_000_400, 2));
        assert_eq!(m1.cuts, [[200, 1]]);
    }

    /// The guard of task #535: the forecast, fed the coordinator's state as recorded
    /// (`Coordinator::sched_state`, as written with the lead's forecast), makes the same first grants
    /// the coordinator then makes: the lead's and a member's, by memory, cores and the NAS budget
    /// (each z6 tile weighing 400 of its 1000: two at once, whatever their room says).
    #[test]
    fn fed_a_recorded_state_the_forecast_grants_what_the_coordinator_grants() {
        use crate::coord::{self, client, cost_version};
        let d = tempfile::tempdir().unwrap();
        let (c, port) = coord::start_for_test(&d.path().join("coord"), "m4", "");
        let m1 = client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m1");
        c.set_nas(Some(crate::budget::nas::Limits { bytes_s: 100e6, creates_s: 100.0, ..Default::default() }));
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
        let mut real: Vec<(String, Vec<String>)> = Vec::new();
        let mut take = |g: Option<crate::coord::Grant>, who: &str| {
            for g in g.into_iter().flat_map(|mut g| {
                let more = std::mem::take(&mut g.more);
                std::iter::once(g).chain(more)
            }) {
                if let crate::coord::Granted::Job { targets, .. } = g.work {
                    real.push((who.to_string(), targets.into_iter().map(|t| t.0).collect()));
                }
            }
        };
        take(c.ask_own(&ask(16_000, 14, 4), &Default::default()), "m4");
        take(m1.ask(&ask(8000, 8, 4)).unwrap(), "m1");
        assert!(!real.is_empty() && real.len() < 8, "the budget holds some back: {real:?}");
        // The forecast: the same z6 tiles, the lead's four slots and the member's, the state fed.
        let regions = [region("a", &[], &units.iter().map(String::as_str).collect::<Vec<_>>(), &[])];
        let slot = |host: &str, k: usize, lead: bool| Machine {
            equal: k > 0 || !lead,
            lead_steps: lead,
            mac: Some(host.into()),
            ..mac(&super::super::slot_worker(host, k), 1.0, false)
        };
        let machines = (0..4).map(|k| slot("m4", k, true)).chain((0..4).map(|k| slot("m1", k, false))).collect();
        let mut inp = input(&regions, machines, &cost);
        inp.after = Vec::new();
        inp.macs.insert("m4".into(), MacRoom { mem_mb: 16_000, cores: 14 });
        inp.sched = Some(st);
        let f = forecast(&inp);
        let first: Vec<(String, Vec<String>)> =
            f.grants.iter().filter(|g| g.at as u64 == inp.now).map(|g| (g.mac.clone(), g.targets.clone())).collect();
        assert_eq!(first, real);
    }

    /// The backtest of tasks #535, #539 and #563 (by hand: `SCENIC_BACKTEST=<the NAS's state
    /// folder>`, read only, and `SCENIC_BACKTEST_LOGS=<the leads' agent.log, comma-separated>`):
    /// from each term's recorded state since 18 Oct (state/coord/term/<E>/state.json: the leases out
    /// and the measures, as current then) the forecast of the jobs the history shows started from
    /// then until the plan's work first ran out (no job of it running for `GAP` s, within `H`), on
    /// each Mac's slots then (the most it ran at once the day before), against when it ran out.
    /// Whichever Mac led (`lead_at`: the Mac whose own jobs, "start" and "end", come next; a Mac
    /// renamed, `ALIAS`, one), its members the Macs that ran jobs from a day before to the end.
    /// Three runs of each, on the history's job set (the jobs started in the window) and order:
    /// "as live" (each Mac held back, `held_back` over the day before; each job released once the
    /// run has done what it waits on, `Release::after`, those the history shows started before it,
    /// and one waiting on a job under way at the start once that really ended), the same with
    /// nothing held back (each Mac's slots the most it ran at once the day before), and with the
    /// history's release times (`Release`: each job ready when the history shows what it waits on
    /// done: a map tile its "planned" event's z6 tiles; a z3 tile's assembly its z6 tiles; a z6
    /// tile's slope and a z6 tile the terrain of its z3 tile and its neighbours', a z6 tile also
    /// the pass's heritage sites, reaches and buildings; a chain's step the steps it reads,
    /// `chain_deps`; the lead's jobs on its own clock, `CLOCKED`, when they started, left out of
    /// the finish), whose busy shares (1, 2, 3,
    /// 4+ jobs at once) are printed beside the history's. The NAS's limits: the lead's own held to
    /// the best of the last day's probes the logs show by then (`nas::Probes`; each probe timed by
    /// the term the log last named), the members' not (the switch, state/pool/nas-budget, was off).
    /// A start whose window holds a pause is skipped.
    /// A z3 step's replayed dependencies (`backtest_against_the_history`): the steps whose targets
    /// in its z3 tile it waits for.
    fn z3_reads(step: &str) -> &'static [&'static str] {
        match step {
            "terrain-lo" => &["terrain-hi"],
            "slope-lo" => &["slope-hi", "terrain-hi"],
            "trees-lo" => &["trees"],
            "depth-lo" => &["depth"],
            _ => &[],
        }
    }

    #[test]
    fn the_depth_chains_replay_waits_on_earlier_steps_only() {
        // The chain's order, as depth_next takes it: each step waits on earlier ones alone (#611).
        let order = ["depth", "depth-lo"];
        for (i, s) in order.iter().enumerate() {
            let waits: Vec<&str> = z3_reads(s).iter().chain(super::chain_deps(s).1).copied().collect();
            for w in &waits {
                let at = order.iter().position(|o| o == w).unwrap_or_else(|| panic!("{s} waits on {w}"));
                assert!(at < i, "{s} waits on {w}, a later step");
            }
            assert_eq!(waits.is_empty(), i == 0, "{s}: {waits:?}");
        }
    }

    #[test]
    fn backtest_against_the_history() {
        use crate::coord::history::Event;
        use crate::coord::lease::{Lease, Work};
        const H: u64 = 24 * 3600;
        const GAP: u64 = 1800;
        const ALIAS: [(&str, &str); 1] = [("Mac-mini", "Mac-mini-de-Brandon")];
        let Ok(root) = std::env::var("SCENIC_BACKTEST") else {
            eprintln!("backtest skipped: SCENIC_BACKTEST unset (no bound checked)");
            return;
        };
        let root = std::path::Path::new(&root);
        // (Each Mac's room as it asks: its build memory and cores.)
        let room = |h: &str| match h {
            "Brandons-MacBook-Pro" => (36_000, 14),
            "Mac-mini-de-Brandon" => (26_000, 12),
            _ => (10_240, 10),
        };
        let mut events: Vec<Event> = Vec::new();
        for day in std::fs::read_dir(root.join("coord/history")).unwrap().flatten() {
            for f in std::fs::read_dir(day.path()).unwrap().flatten() {
                let text = std::fs::read_to_string(f.path()).unwrap_or_default();
                events.extend(text.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()));
            }
        }
        // (A Mac renamed: one; a second slot's old name, its slot's.)
        for e in &mut events {
            if let Some(w) = e.worker.as_mut() {
                if let Some(a) = ALIAS.iter().find(|a| w.split(" #").next() == Some(a.0)) {
                    *w = w.replacen(a.0, a.1, 1);
                }
                if let Some(h) = w.strip_suffix(" (second job)") {
                    *w = format!("{h} #2");
                }
            }
        }
        // (Each member's file holds the events of the terms it led: the same event twice.)
        events.sort_by(|a, b| {
            (a.t, &a.kind, a.lease, &a.worker, &a.targets).cmp(&(b.t, &b.kind, b.lease, &b.worker, &b.targets))
        });
        events.dedup_by(|a, b| {
            (a.t, &a.kind, a.lease, &a.worker, &a.targets) == (b.t, &b.kind, b.lease, &b.worker, &b.targets)
        });
        // Each job: its Mac, step, targets, from, until: a job's end (the lead's own) or done or
        // failed (a member's), back its time (its slot's worker named for its Mac).
        let host = |w: &str| w.split(" #").next().unwrap_or(w).to_string();
        let jobs: Vec<(String, String, Vec<String>, u64, u64)> = events
            .iter()
            .filter(|e| matches!(e.kind.as_str(), "end" | "done" | "fail"))
            .filter_map(|e| {
                let secs = e.secs? as u64;
                Some((
                    host(e.worker.as_deref()?),
                    e.step.clone()?,
                    if e.targets.is_empty() { vec![e.step.clone()?] } else { e.targets.clone() },
                    e.t.saturating_sub(secs),
                    e.t,
                ))
            })
            .collect();
        let own: Vec<(u64, String)> = events
            .iter()
            .filter(|e| matches!(e.kind.as_str(), "start" | "end"))
            .filter_map(|e| Some((e.t, host(e.worker.as_deref()?))))
            .collect();
        let lead_at = |t: u64| {
            own.iter()
                .find(|o| o.0 >= t && o.0 < t + 7200)
                .or_else(|| own.iter().rev().find(|o| o.0 < t))
                .map(|o| o.1.clone())
        };
        // (Each target's job's end, by its step; each map tile's z6 tiles, by its "planned" events.)
        let mut done_at: BTreeMap<(String, String), Vec<u64>> = BTreeMap::new();
        for j in &jobs {
            for t in &j.2 {
                done_at.entry((j.1.clone(), t.clone())).or_default().push(j.4);
            }
        }
        let mut pack_units: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for e in events.iter().filter(|e| e.kind == "planned") {
            let Some(units) = e.data.as_ref().and_then(|d| d["units"].as_object()) else { continue };
            for (u, packs) in units {
                for p in packs.as_array().into_iter().flatten().filter_map(|p| p.as_str()) {
                    pack_units.entry(p.to_string()).or_default().insert(u.clone());
                }
            }
        }
        // (The probes the logs show, each timed by the term the log last named.)
        let term_at: BTreeMap<u64, u64> = std::fs::read_dir(root.join("coord/term"))
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let n: u64 = e.file_name().to_str()?.parse().ok()?;
                let v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(e.path().join("view.json")).ok()?).ok()?;
                Some((n, v["at"].as_u64()?))
            })
            .collect();
        let mut probes: Vec<crate::budget::nas::Probe> = Vec::new();
        for log in std::env::var("SCENIC_BACKTEST_LOGS").unwrap_or_default().split(',').filter(|l| !l.is_empty()) {
            let mut at = None;
            for l in std::fs::read_to_string(log).unwrap_or_default().lines() {
                if let Some(n) = l
                    .split("term ")
                    .nth(1)
                    .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse::<u64>().ok()))
                {
                    at = term_at.get(&n).copied().or(at);
                }
                let Some(r) = l.strip_prefix("coordinator: the NAS probed: ") else { continue };
                let nums: Vec<f64> =
                    r.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter_map(|x| x.parse().ok()).collect();
                if let (Some(at), [mb, creates, ..]) = (at, nums.as_slice()) {
                    probes.push(crate::budget::nas::Probe {
                        at,
                        bytes_s: mb * 1e6,
                        creates_s: *creates,
                        ..Default::default()
                    });
                }
            }
        }
        let limits_at = |t: u64| {
            let p = crate::budget::nas::Probes(
                probes.iter().filter(|p| p.at <= t && p.at + crate::budget::nas::KEEP_S > t).copied().collect(),
            );
            p.limits(None)
        };
        let pauses: Vec<u64> = events.iter().filter(|e| e.kind == "pause" || e.kind == "resume").map(|e| e.t).collect();
        let busy_share = |runs: &[(u64, u64)], from: u64, to: u64| {
            let mut n = [0u64; 5];
            for t in (from..to).step_by(60) {
                let c = runs.iter().filter(|r| r.0 <= t && t < r.1).count();
                n[c.min(4)] += 1;
            }
            let busy: u64 = n[1..].iter().sum();
            n[1..].iter().map(|&x| x as f64 / busy.max(1) as f64).collect::<Vec<f64>>()
        };
        let mut terms: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(root.join("coord/term"))
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(e.path().join("view.json")).ok()?).ok()?;
                Some((v["at"].as_u64()?, e.path()))
            })
            .filter(|t| t.0 >= 1_792_281_600)
            .collect();
        terms.sort();
        // (Each start's lead, the three forecasts' errors (as live, nothing held back, released as
        // the history shows) and the live error said then.)
        type Out = (String, f64, f64, f64, Option<f64>);
        let mut out: Vec<Out> = Vec::new();
        let mut curves: Vec<(u64, Vec<u32>)> = Vec::new();
        let mut progress: Vec<(String, f64)> = Vec::new();
        let last_t = events.last().map_or(0, |e| e.t);
        let mut sum: BTreeMap<(String, &str), Vec<f64>> = BTreeMap::new();
        for (t0, dir) in terms {
            let Ok(raw) = std::fs::read(dir.join("state.json")) else { continue };
            let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            let Some(lead) = lead_at(t0) else { continue };
            if !v["pause"].is_null() {
                continue;
            }
            fn get<T: serde::de::DeserializeOwned + Default>(v: &serde_json::Value, k: &str) -> T {
                serde_json::from_value(v[k].clone()).unwrap_or_default()
            }
            // (Its measures as current then: a step's way changed since (`cost_version`, as unit's
            // did on 21 Oct) would leave the backtest's run every target of it cold.)
            let mut costs: BTreeMap<String, crate::coord::Cost> = get(&v, "costs");
            for (k, c) in costs.iter_mut() {
                c.v = crate::coord::cost_version(k.split_once(' ').map_or("unit", |s| s.0));
            }
            let leases: Vec<Lease> = serde_json::from_value(v["leases"]["leases"].clone()).unwrap_or_default();
            let running: Vec<_> = jobs.iter().filter(|j| j.3 <= t0 && t0 < j.4).collect();
            // (The work: every job started from then until the plan's work first ran out, no job of it
            // running for `GAP` s; for a start whose work doesn't run out within `H`, until then: its
            // run an earlier forecast for the live error, no finish of its own.)
            let planned = |step: &str| !CLOCKED.contains(&step);
            let mut until = running.iter().filter(|j| planned(&j.1)).map(|j| j.4).max().unwrap_or(t0);
            let mut ran_out = None;
            let mut later: Vec<_> = jobs.iter().filter(|j| j.3 > t0 && planned(&j.1)).collect();
            later.sort_by_key(|j| j.3);
            for j in later {
                if j.3 > until + GAP {
                    ran_out = Some(until);
                    break;
                }
                until = until.max(j.4);
            }
            let ran_out = ran_out.filter(|&e| e > t0 && e <= t0 + H);
            let end = ran_out.unwrap_or(t0 + H);
            let work: Vec<_> = jobs.iter().filter(|j| j.3 > t0 && j.3 <= end).collect();
            if !work.iter().any(|j| planned(&j.1)) {
                continue;
            }
            let mut macs: Vec<String> = vec![lead.clone()];
            for j in jobs.iter().filter(|j| j.4 + 86_400 > t0 && j.3 <= end) {
                if !macs.contains(&j.0) {
                    macs.push(j.0.clone());
                }
            }
            let ask = |w: &str| {
                let (mem_mb, cores) = room(w);
                crate::coord::sched::Asked {
                    ask: crate::coord::Ask {
                        worker: w.into(),
                        kind: "native".into(),
                        mem_mb,
                        cores,
                        cap: Some(Capacity {
                            cores,
                            cores_free: cores,
                            mem_mb,
                            build_mb: mem_mb,
                            limit_mb: mem_mb,
                            ..Default::default()
                        }),
                        want: 1,
                        ..Default::default()
                    },
                    ..Default::default()
                }
            };
            let st = State {
                leases: leases
                    .iter()
                    .filter_map(|l| match &l.work {
                        Work::Job { step, targets } => Some(crate::coord::sched::Leased {
                            worker: l.worker.clone(),
                            step: step.clone(),
                            targets: targets.clone(),
                            threads: l.threads,
                        }),
                        _ => None,
                    })
                    .collect(),
                workers: macs.iter().map(|m| ask(m)).collect(),
                costs: costs.clone(),
                floors: get(&v, "floors"),
                disks: get(&v, "disks"),
                job_disks: get(&v, "job_peaks"),
                fills: get(&v, "fills"),
                lead_name: lead.clone(),
                nas: None,
                nas_own: limits_at(t0),
                ..Default::default()
            };
            let before = &events[..events.partition_point(|e| e.t < t0)];
            let sp = speeds(before, &lead, &macs[1..], 0.5);
            let pace_of = |w: Option<&String>| w.and_then(|w| sp.get(&host(w))).map_or(1.0, |s| s.0);
            let mean = |step: &str| {
                let v: Vec<f64> = before
                    .iter()
                    .filter(|e| matches!(e.kind.as_str(), "done" | "end") && e.step.as_deref() == Some(step))
                    .filter_map(|e| Some(e.secs? / e.targets.len().max(1) as f64))
                    .collect();
                if v.is_empty() {
                    600.0
                } else {
                    v.iter().sum::<f64>() / v.len() as f64
                }
            };
            // (An unmeasured target's memory: its step's measured targets' mean peak, as the book
            // sizes it; none such, 0.)
            let peak = |step: &str| {
                let v: Vec<u64> = costs
                    .iter()
                    .filter(|(k, c)| k.split_once(' ').map_or("unit", |s| s.0) == step && c.peak_mb > 0)
                    .map(|(_, c)| c.peak_mb)
                    .collect();
                v.iter().sum::<u64>() / v.len().max(1) as u64
            };
            let at_lead = |c: &crate::coord::Cost| c.secs as f64 * pace_of(c.worker.as_ref());
            // (An unmeasured target's time, as the live forecast takes it: its step's measured
            // targets' mean, else what its jobs took a target, else a first guess.)
            let each = |step: &str| {
                let v: Vec<f64> = costs
                    .iter()
                    .filter(|(k, _)| k.split_once(' ').map_or("unit", |s| s.0) == step)
                    .map(|(_, c)| at_lead(c))
                    .collect();
                if v.is_empty() {
                    mean(step)
                } else {
                    v.iter().sum::<f64>() / v.len() as f64
                }
            };
            let cost_of = |step: &str, t: &str| match costs.get(&cost_key(step, t)) {
                Some(c) => Cost { secs: at_lead(c), known: true, peak_mb: c.peak_mb },
                None => Cost { secs: each(step), known: false, peak_mb: peak(step) },
            };
            let mut seen = BTreeSet::new();
            let items: Vec<Job> = work
                .iter()
                .flat_map(|j| j.2.iter().map(move |t| (j.1.clone(), t.clone())))
                .filter(|k| seen.insert(k.clone()))
                .map(|(s, t)| {
                    let c = cost_of(&s, &t);
                    (s, t, c)
                })
                .collect();
            // (What held each Mac back over the day before: `held_back`, as live.)
            let held = held_back(before, t0);
            // (Nothing held back: each Mac's slots the most jobs it ran at once in the day before.)
            let most = |host: &str| {
                let mine: Vec<_> = jobs.iter().filter(|j| j.0 == host && j.4 > t0 - 86_400 && j.3 <= t0).collect();
                mine.iter().map(|a| mine.iter().filter(|b| b.3 <= a.3 && a.3 < b.4).count()).max().unwrap_or(1).max(1)
            };
            let lanes_n = |host: &str, lead: bool, n: usize| -> Vec<Machine> {
                let mine: Vec<_> = running.iter().filter(|j| j.0 == host).collect();
                (0..n.max(mine.len()))
                    .map(|k| Machine {
                        name: super::super::slot_worker(host, k),
                        speed: if lead { 1.0 } else { sp.get(host).map_or(0.5, |s| s.0) },
                        measured: true,
                        equal: k > 0 || !lead,
                        lead_steps: lead,
                        mem_mb: u64::MAX >> 2,
                        busy_s: mine.get(k).map_or(0.0, |j| (j.4 - t0) as f64),
                        running: mine.get(k).map(|j| (j.1.clone(), j.2.len())),
                        mac: Some(host.into()),
                        ..Default::default()
                    })
                    .collect()
            };
            let lanes = |host: &str, lead: bool| lanes_n(host, lead, held.get(host).map_or(1, |h| h.lanes));
            let lanes_max = |host: &str, lead: bool| lanes_n(host, lead, most(host));
            let last_of = |s: &Sim| s.items.iter().filter(|i| planned(&i.step)).map(|i| i.end).fold(0.0, f64::max);
            let mut inp = Input {
                now: t0,
                before: items,
                drops: Vec::new(),
                regions: Cow::Owned(Vec::new()),
                costs: CostTable::new(),
                cadence: Cadence::default(),
                blind: None,
                chains: Vec::new(),
                after: Vec::new(),
                idle: Vec::new(),
                machines: hold_back(macs.iter().enumerate().flat_map(|(k, m)| lanes(m, k == 0)).collect(), &held),
                running: BTreeMap::new(),
                macs: BTreeMap::new(),
                sched: Some(st),
                release: Vec::new(),
                held: held.clone(),
                pace: None,
                step_pace: Default::default(),
                paused: false,
                took: Default::default(),
            };
            // Each job's dependencies, as the plan has them (`items`): a map tile its "planned" event's
            // z6 tiles; a z3 tile's assembly its z6 tiles; a z6 tile's slope and a z6 tile the
            // terrain of its z3 tile and its neighbours', a z6 tile also the pass's heritage sites,
            // reaches and buildings; a chain's step the steps it reads (`chain_deps`). Those the
            // history shows started before it only: the plan's order then.
            let mut start: BTreeMap<(String, String), u64> = BTreeMap::new();
            for j in &work {
                for t in &j.2 {
                    start.entry((j.1.clone(), t.clone())).or_insert(j.3);
                }
            }
            // (The targets running at the start: what a job may wait on too.)
            let ran: BTreeSet<(String, String)> =
                running.iter().flat_map(|j| j.2.iter().map(move |t| (j.1.clone(), t.clone()))).collect();
            let z3s = |t: &str| -> Vec<String> {
                match crate::legacy::TileId::parse(t) {
                    Some(u) if u.z == 6 => std::iter::once((u.x, u.y))
                        .chain(crate::agent::build::edge_neighbours(6, u.x, u.y))
                        .map(|(x, y)| format!("3/{}/{}", x >> 3, y >> 3))
                        .collect(),
                    _ => Vec::new(),
                }
            };
            let in_z3 = |step: &str, qs: &[String]| -> Vec<(String, String)> {
                start
                    .keys()
                    .chain(ran.iter())
                    .filter(|k| {
                        crate::agent::steps::current(&k.0) == step
                            && crate::agent::build::z3_of(&k.1).is_some_and(|q| qs.contains(&q))
                    })
                    .cloned()
                    .collect()
            };
            let deps_of: Vec<((String, String), u64, Vec<(String, String)>)> = start
                .iter()
                .map(|((step, t), &s0)| {
                    let me = (step.clone(), t.clone());
                    if CLOCKED.contains(&step.as_str()) {
                        return (me, s0, Vec::new());
                    }
                    let q = || vec![t.clone()];
                    // (The history's names: a step renamed since read as its new one, steps::current.)
                    let deps: Vec<(String, String)> = match crate::agent::steps::current(step) {
                        "pack" => {
                            pack_units.get(t).into_iter().flatten().map(|u| ("unit".to_string(), u.clone())).collect()
                        }
                        s if !z3_reads(s).is_empty() => z3_reads(s).iter().flat_map(|r| in_z3(r, &q())).collect(),
                        "slope-hi" => in_z3("terrain-hi", &z3s(t)),
                        "unit" => {
                            let mut d = in_z3("terrain-hi", &z3s(t));
                            d.extend(
                                start.keys().chain(ran.iter()).filter(|k| UNITS_NEED.contains(&k.0.as_str())).cloned(),
                            );
                            d
                        }
                        s => {
                            let (pre, earlier, _) = chain_deps(s);
                            let reads =
                                |k: &&(String, String)| pre.contains(&k.0.as_str()) || earlier.contains(&k.0.as_str());
                            start.keys().chain(ran.iter()).filter(reads).cloned().collect()
                        }
                    };
                    let deps = deps.into_iter().filter(|k| *k != me && start.get(k).is_none_or(|&s| s < s0)).collect();
                    (me, s0, deps)
                })
                .collect();
            // As live: each job once the run has done what it waits on (the lead's on its own clock,
            // `CLOCKED`, when it started; one waiting on a job under way at the start, once that
            // ended); the live error from the earlier terms' runs.
            inp.release = deps_of
                .iter()
                .map(|((step, t), s0, deps)| {
                    let at = if CLOCKED.contains(&step.as_str()) { *s0 } else { 0 };
                    let at = deps
                        .iter()
                        .filter(|k| ran.contains(*k))
                        .filter_map(|k| done_at.get(k)?.iter().copied().filter(|&d| d > t0).min())
                        .fold(at, u64::max);
                    let after = deps.iter().filter(|k| start.contains_key(*k)).cloned().collect();
                    Release { step: step.clone(), target: t.clone(), at, after }
                })
                .filter(|r| r.at > t0 || !r.after.is_empty())
                .collect();
            let live = run(&inp, &|c| c.secs, far());
            let said = pace(&curves, before, t0);
            curves.retain(|c| c.0 + PACE_MAX_S > t0);
            curves.push((t0, curve(&live.items)));
            // (Its progress: the time its run took to do the targets the history shows done in the
            // `PACE_MAX_S` after (or until the work ran out), against that time (`pace`); every
            // start the history reaches past, the work run out or not.)
            let to = ran_out.unwrap_or(u64::MAX).min(t0 + PACE_MAX_S);
            if to >= t0 + PACE_MIN_S && to <= last_t {
                let upto = &events[..events.partition_point(|e| e.t <= to)];
                if let Some(p) = pace(&[(t0, curve(&live.items))], upto, to) {
                    progress.push((lead.clone(), p.error));
                }
            }
            // (The plan alone, nothing held back: what the live forecast was before task #563.)
            let unheld = {
                let mut free = inp.clone();
                free.machines = macs.iter().enumerate().flat_map(|(k, m)| lanes_max(m, k == 0)).collect();
                last_of(&run(&free, &|c| c.secs, far()))
            };
            let Some(ran_out) = ran_out else { continue };
            if pauses.iter().any(|&p| p >= t0 && p < ran_out) {
                continue;
            }
            // Released as it was: each job once the history shows what it waits on done (since the
            // start and before it started).
            inp.release = deps_of
                .iter()
                .map(|((step, t), s0, deps)| {
                    let at = if CLOCKED.contains(&step.as_str()) { *s0 } else { 0 };
                    let at = deps
                        .iter()
                        .filter_map(|k| done_at.get(k)?.iter().copied().filter(|&d| d > t0 && d <= *s0).max())
                        .fold(at, u64::max);
                    Release { step: step.clone(), target: t.clone(), at, after: Vec::new() }
                })
                .filter(|r| r.at > t0)
                .collect();
            let timed = run(&inp, &|c| c.secs, far());
            let real = (ran_out - t0) as f64;
            let err = |fc: f64| (fc - real) / real;
            let (live_s, timed_s) = (last_of(&live), last_of(&timed));
            for host in &macs {
                let real: Vec<(u64, u64)> = running
                    .iter()
                    .chain(&work)
                    .filter(|j| j.0 == *host)
                    .map(|j| (j.3.max(t0) - t0, j.4 - t0))
                    .collect();
                let lanes_of: Vec<usize> =
                    (0..inp.machines.len()).filter(|&k| inp.machines[k].mac.as_deref() == Some(host)).collect();
                let mut fc: Vec<(u64, u64)> = lanes_of
                    .iter()
                    .filter(|&&k| inp.machines[k].busy_s > 0.0)
                    .map(|&k| (0, inp.machines[k].busy_s as u64))
                    .collect();
                for &k in &lanes_of {
                    fc.extend(timed.items.iter().filter(|i| i.by == Some(k)).map(|i| (i.from as u64, i.end as u64)));
                }
                let to = (ran_out - t0).max(timed_s as u64).min(H);
                let (r, f) = (busy_share(&real, 0, to), busy_share(&fc, 0, to));
                for (k, (a, b)) in r.iter().zip(&f).enumerate() {
                    sum.entry((host.clone(), ["1", "2", "3", "4+"][k])).or_default().extend([*a, *b]);
                }
            }
            eprintln!(
                "{t0} led by {lead}: {} jobs, ran out {real:.0} s in; as live {:+.0} % (nothing held back {:+.0} %), released as it was {:+.0} %; the live error then {}",
                work.len(),
                err(live_s) * 100.0,
                err(unheld) * 100.0,
                err(timed_s) * 100.0,
                said.as_ref().map_or("none".into(), |p| format!("{:+.0} % over {} s", p.error * 100.0, p.real_s)),
            );
            out.push((lead, err(live_s), err(unheld), err(timed_s), said.map(|p| p.error)));
        }
        for ((m, k), v) in &sum {
            let (h, f): (Vec<f64>, Vec<f64>) =
                (v.iter().step_by(2).copied().collect(), v.iter().skip(1).step_by(2).copied().collect());
            let mean = |x: &[f64]| x.iter().sum::<f64>() / x.len().max(1) as f64 * 100.0;
            eprintln!("{m} {k} job(s): history {:.0} %, released as it was {:.0} %", mean(&h), mean(&f));
        }
        // (The median and 80th percentile of the errors' size, and their mean, in %.)
        let stats = |e: &[f64]| {
            let mut abs: Vec<f64> = e.iter().map(|e| e.abs()).collect();
            abs.sort_by(f64::total_cmp);
            let at = |q: usize| abs.get(abs.len() * q / 100).copied().unwrap_or(0.0) * 100.0;
            [at(50), at(80), e.iter().sum::<f64>() / e.len().max(1) as f64 * 100.0]
        };
        let leads: BTreeSet<&str> =
            out.iter().map(|o| o.0.as_str()).chain(progress.iter().map(|o| o.0.as_str())).collect();
        let mut all = Vec::new();
        for who in leads.into_iter().map(Some).chain([None]) {
            let mine: Vec<&Out> = out.iter().filter(|o| who.is_none_or(|w| o.0 == w)).collect();
            let col = |f: &dyn Fn(&Out) -> Option<f64>| stats(&mine.iter().filter_map(|o| f(o)).collect::<Vec<_>>());
            let [live, unheld, timed, said] =
                [col(&|o| Some(o.1)), col(&|o| Some(o.2)), col(&|o| Some(o.3)), col(&|o| o.4)];
            eprintln!(
                "{} ({} starts): finish error, median / 80th percentile / mean: as live {:.0} / {:.0} / {:+.0} %; nothing held back {:.0} / {:.0} / {:+.0} %; released as it was {:.0} / {:.0} / {:+.0} %; the live error said ({} starts) {:.0} / {:.0} / {:+.0} %",
                who.unwrap_or("all"),
                mine.len(),
                live[0], live[1], live[2], unheld[0], unheld[1], unheld[2], timed[0], timed[1], timed[2],
                mine.iter().filter(|o| o.4.is_some()).count(),
                said[0], said[1], said[2]
            );
            let p = stats(&progress.iter().filter(|o| who.is_none_or(|w| o.0 == w)).map(|o| o.1).collect::<Vec<_>>());
            eprintln!(
                "{}: progress error over the next {} h ({} starts): median / 80th percentile / mean {:.0} / {:.0} / {:+.0} %",
                who.unwrap_or("all"),
                PACE_MAX_S / 3600,
                progress.iter().filter(|o| who.is_none_or(|w| o.0 == w)).count(),
                p[0], p[1], p[2]
            );
            all = vec![live, unheld];
        }
        // (Held back, the live forecast's median error at most 40 %, and at most two thirds of
        // the plan's alone.)
        assert!(
            all[0][0] <= 40.0 && all[0][0] * 1.5 <= all[1][0],
            "as live {:?}, nothing held back {:?}",
            all[0],
            all[1]
        );
    }

    /// Task #563: a Mac that ran one job at a time for half the hours the pool worked is held to
    /// one lane at half its pace; a lane running a job is never dropped; the live error says how
    /// an earlier run fared against the targets done since.
    #[test]
    fn held_back_by_the_last_day_and_the_live_error_said() {
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
        // (m4: two slots at once for 2 h, its second by its old name; m1: one job at a time.)
        let events = vec![
            job("m4", now - 3600, 7200.0, 1),
            job("m4 (second job)", now - 3600, 7200.0, 1),
            job("m1", now - 7200, 3600.0, 1),
            job("ipad", now - 9000, 600.0, 1),
        ];
        let h = held_back(&events, now);
        assert_eq!(h["m4"], Held { lanes: 2, util: 0.0 });
        assert_eq!((h["m1"].lanes, h["ipad"].lanes), (1, 1));
        let lane = |host: &str, k: usize, busy: f64| Machine {
            busy_s: busy,
            mac: Some(host.into()),
            ..mac(&super::super::slot_worker(host, k), 1.0, false)
        };
        let ms = vec![lane("m1", 0, 0.0), lane("m1", 1, 0.0), lane("m1", 2, 50.0), lane("mini", 0, 0.0)];
        let kept = hold_back(ms, &h);
        assert_eq!(kept.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["m1", "m1 #3", "mini"]);
        assert!(kept.iter().all(|m| m.speed == 1.0), "cut, not slowed");
        // (A run that had done 4 targets 600 s in; the history 2 in the 7200 s since: 300 s.)
        let p = pace(&[(now - 7200, vec![0, 4, 8])], &events, now).unwrap();
        assert_eq!((p.real_n, p.run_s, p.real_s), (2, 300, 7200));
        assert!(pace(&[(now - 600, vec![0, 4])], &events, now).is_none(), "too young");
    }

    /// Each Mac's time by step over the last day; the run offers a Mac none of a step it gave
    /// under a tenth of its time and under an hour (a Mac asleep, none), and the rest as before.
    #[test]
    fn a_mac_that_took_little_of_a_step_is_offered_none_of_it() {
        use crate::coord::history::Event;
        let now = 100_000;
        let job = |w: &str, step: &str, secs: f64| Event {
            t: now - 100,
            kind: "done".into(),
            worker: Some(w.into()),
            step: Some(step.into()),
            secs: Some(secs),
            ok: Some(true),
            targets: vec!["x".into()],
            ..Default::default()
        };
        // (unit: m4 20 h, a quarter by its old second slot's name, mini 1.5 h, m1 none; pois: mini alone, 2 jobs: too few to say.)
        let mut events: Vec<Event> = (0..3).map(|_| job("m4 #2", "unit", 18_000.0)).collect();
        events.push(job("m4 (second job)", "unit", 18_000.0));
        events.push(job("mini", "unit", 5400.0));
        events.extend((0..2).map(|_| job("mini", "pois", 600.0)));
        let macs = ["m4".to_string(), "mini".to_string(), "m1".to_string()];
        let t = took(&events, now, &macs);
        assert_eq!(t["m4"]["unit"], 20.0);
        assert_eq!(t["m1"]["unit"], 0.0);
        assert!(!t["mini"].contains_key("pois"));
        let s = skips(&t);
        assert_eq!((s["m4"].len(), s["mini"].len()), (0, 0), "mini's 1.5 h keeps it");
        assert_eq!(s["m1"], ["unit"]);
        // The run: m1 (the first, idle) takes none of the units; m4 does them all.
        let regions = [region("r", &[], &["6/1/1", "6/1/2"], &[])];
        let mut inp = input(&regions, vec![member("m1"), member("m4")], &cost);
        inp.took = t;
        let f = forecast(&inp);
        let m1 = inp.machines.iter().position(|m| m.name == "m1");
        assert!(!f.placed.is_empty() && f.placed.iter().all(|p| Some(p.lane) != m1), "{:?}", f.placed);
        // m4 busy past the day: m1 takes them once its skip is over, not before.
        inp.machines[1].busy_s = 2.0 * HELD_S as f64;
        let f = forecast(&inp);
        let on_m1: Vec<_> = f.placed.iter().filter(|p| Some(p.lane) == m1).collect();
        assert!(!on_m1.is_empty() && on_m1.iter().all(|p| p.from >= inp.now + HELD_S), "{:?}", f.placed);
    }

    /// A step's jobs that took half their noted costs at the lead's pace run its items at half;
    /// a step with too few jobs, a job with a target never noted, or one done before the span,
    /// aren't counted; the costs noted are those the targets had while left, not after.
    #[test]
    fn a_steps_pace_from_its_jobs_against_their_noted_costs() {
        use crate::coord::history::Event;
        let now = 200_000;
        let job = |w: &str, t: u64, step: &str, targets: &[&str], secs: f64| Event {
            t,
            kind: "done".into(),
            worker: Some(w.into()),
            step: Some(step.into()),
            secs: Some(secs),
            ok: Some(true),
            targets: targets.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        };
        let regions = [region("r", &["6/1/1", "6/1/2", "6/1/3", "6/1/4"], &["6/9/9"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        inp.now = now - 3600;
        let mut noted = Noted::default();
        noted.note(&inp);
        assert_eq!(noted.cost("terrain-hi", "6/1/1"), Some(600.0));
        // (Later the targets are built: no longer work left, their noted costs stay.)
        let later = input(&[], vec![mac("m4", 1.0, false)], &cost);
        noted.note(&Input { now, ..later });
        // terrain-hi: three jobs at the lead's pace 300 s a 600 s target (m1 at half the pace in
        // 600 s); one too old; one with a target never noted. unit: one job, too few.
        let events = vec![
            job("m4", now - 100, "terrain-hi", &["6/1/1"], 300.0),
            job("m1", now - 200, "terrain-hi", &["6/1/2"], 600.0),
            job("m4 #2", now - 300, "terrain-hi", &["6/1/3", "6/1/4"], 600.0),
            job("m4", now - STEP_PACE_S - 10, "terrain-hi", &["6/1/1"], 6000.0),
            job("m4", now - 50, "terrain-hi", &["6/7/7"], 6000.0),
            job("m4", now - 50, "unit", &["6/9/9"], 30.0),
        ];
        let speed = |w: &str| if w == "m1" { 0.5 } else { 1.0 };
        let p = step_pace(&events, now, &noted, &speed);
        assert_eq!(p, [("terrain-hi".to_string(), 0.5)].into());
        // The run takes it: the terrain at 300 s a target.
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        inp.step_pace = p;
        let f = forecast_by(&inp, far());
        let t = f.placed.iter().find(|p| p.step == "terrain-hi").unwrap();
        assert_eq!(t.until - t.from, 300);
        // A step slower than its costs isn't said; a fast one never more than fourfold.
        let slow: Vec<Event> = (0..3).map(|k| job("m4", now - k, "unit", &["6/9/9"], 30_000.0)).collect();
        assert!(step_pace(&slow, now, &noted, &speed).is_empty());
        let fast: Vec<Event> = (0..3).map(|k| job("m4", now - k, "unit", &["6/9/9"], 1.0)).collect();
        assert_eq!(step_pace(&fast, now, &noted, &speed)["unit"], STEP_PACE_MIN);
        // (A cost not seen for the span is dropped.)
        noted.note(&Input { now: now + STEP_PACE_S + 1, ..input(&[], vec![], &cost) });
        assert_eq!(noted.cost("terrain-hi", "6/1/1"), None);
    }

    /// A lane's job under way is held to its step's longest job of late (three at least): its
    /// `running`'s step, else its running targets'; a step not timed, as said.
    #[test]
    fn a_lanes_time_left_is_held_to_its_steps_longest_job() {
        use crate::coord::history::Event;
        let now = 100_000;
        let done = |step: &str, secs: f64| Event {
            t: now - 60,
            kind: "end".into(),
            worker: Some("m4".into()),
            step: Some(step.into()),
            secs: Some(secs),
            ok: Some(true),
            ..Default::default()
        };
        let events = vec![done("slope", 600.0), done("slope", 900.0), done("slope", 300.0), done("unit", 50.0)];
        let longest = longest_jobs(&events, now);
        assert_eq!(longest, [("slope".to_string(), 900.0)].into());
        let regions = [region("r", &["6/1/1"], &[], &[])];
        let lane = |busy_s: f64, running: Option<(&str, usize)>| Machine {
            busy_s,
            running: running.map(|(s, n)| (s.to_string(), n)),
            ..mac("m", 1.0, false)
        };
        let mut inp = input(
            &regions,
            vec![lane(340_000.0, Some(("slope", 28))), lane(5000.0, None), lane(5000.0, Some(("unit", 1)))],
            &cost,
        );
        inp.running.insert(("slope".into(), "6/1/1".into()), 1);
        cap_busy(&mut inp, &longest);
        assert_eq!(inp.machines.iter().map(|m| m.busy_s).collect::<Vec<_>>(), [900.0, 900.0, 5000.0]);
    }

    #[test]
    fn a_departure_is_two_jobs_and_half_the_forecasts_apart() {
        assert!(!departed(3, 4) && !departed(10, 13) && departed(0, 2) && departed(4, 1) && departed(3, 6));
        let g = |at: f64| Grant { mac: "m4".into(), step: "unit".into(), targets: vec![], at };
        let f = Forecast { at: 1000, grants: vec![g(1000.0), g(1100.0), g(1200.0), g(9000.0)], ..Default::default() };
        let real =
            |n: usize| (0..n).map(|k| (1000 + k as u64, "m4".to_string(), "unit".to_string())).collect::<Vec<_>>();
        assert!(departures(&f, &real(4), 2000).is_empty(), "one more: not said");
        assert_eq!(departures(&f, &real(0), 2000).len(), 1, "none of three");
    }

    #[test]
    fn helpers_speed_from_the_history() {
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
        let s = speeds(&events, "m4", &["m1".into(), "ipad".into()], 0.5);
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
            Some(c) => Cost { secs: c.secs as f64, known: true, peak_mb: c.peak_mb },
            None => Cost { secs: 120.0, known: false, peak_mb: 1000 },
        };
        let chains: Vec<Job> = ["pois", "bldtiles", "depth", "peaks"]
            .iter()
            .flat_map(|s| {
                (0..n_chain).map(move |i| (s.to_string(), z6(i), Cost { secs: 90.0, known: true, peak_mb: 700 }))
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
        let limits = crate::budget::nas::Limits {
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
                    equal: k > 0 || !lead,
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
            release: Vec::new(),
            held: BTreeMap::new(),
            pace: None,
            step_pace: Default::default(),
            paused: false,
            took: Default::default(),
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
            let job = |s: &str, t: String| (s.to_string(), t, Cost { secs: 50.0, known: true, peak_mb: 500 });
            inp.before = (0..next(3)).map(|i| job(UNITS_NEED[i], "all".into())).collect();
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
            let groups = items(&inp);
            let mut spread = items(&inp);
            for it in spread.items.iter_mut() {
                for &g in &it.groups {
                    it.deps.extend(&spread.groups[g]);
                }
                it.groups.clear();
            }
            spread.groups.clear();
            assert!(groups.groups.iter().any(|g| !g.is_empty()), "round {round}: no group");
            let a = run_on(&inp, groups, &scaled(SCALINGS[0]), Duration::from_secs(600));
            let b = run_on(&inp, spread, &scaled(SCALINGS[0]), Duration::from_secs(600));
            assert!(a.done && b.done && !a.cut && !b.cut, "round {round}");
            let at = |s: &Sim| s.items.iter().map(|i| (i.by, i.from.to_bits(), i.end.to_bits())).collect::<Vec<_>>();
            assert_eq!(at(&a), at(&b), "round {round}");
            assert_eq!(a.grants, b.grants, "round {round}");
        }
    }

    #[test]
    fn a_live_scale_forecast_runs_whole_and_bounded() {
        let inp = live_scale_input(400, 1500);
        let n = items(&inp).items.len();
        assert!(n > 30_000, "{n} targets");
        // (Whole, its CPU time bounded well past what it takes (50 to 73 s for the three runs in
        // the test build under load; 21 Oct's code took over 200 s in one run at a tenth of the
        // scale). CPU time, not wall: a loaded machine stretches the wall, not this. The three
        // runs `forecast_by` makes, one after another on this thread.)
        let cpu0 = thread_cpu();
        let runs = SCALINGS.map(|k| run(&inp, &scaled(k), Duration::from_secs(600)));
        let took = thread_cpu() - cpu0;
        eprintln!("live scale, {n} targets: {} ms of CPU", took.as_millis());
        assert!(runs.iter().all(|r| r.done && !r.cut), "a run was cut or unfinished");
        assert!(took < Duration::from_secs(180), "{took:?}");
        // Past its budget: cut there, partial and said so.
        let f = forecast_by(&inp, Duration::from_millis(50));
        assert!(f.partial && f.done_at.is_none(), "{:?}", f.why);
    }

    /// At 22 Oct's live scale (134 regions, about 4,600 chain targets): whole within its budget;
    /// and the budget is CPU time (task #584): a thread's wait counts nothing.
    #[test]
    fn a_live_scale_forecast_is_whole_and_a_wait_is_no_cpu_time() {
        let inp = live_scale_input(134, 1150);
        // (The test build computes ~4x slower than the app's, release: its budget so much longer.
        // Release, on the M1 at load 140: 48 s of wall, whole.)
        let budget = if cfg!(debug_assertions) { BUDGET * 4 } else { BUDGET };
        let t = Instant::now();
        let f = forecast_by(&inp, budget);
        eprintln!("22 Oct's live scale: {} ms", t.elapsed().as_millis());
        assert!(!f.partial && f.done_at.is_some(), "{:?}", f.why);
        let cpu0 = thread_cpu();
        std::thread::sleep(Duration::from_millis(300)); // (sleep ok: a wait whose CPU time is measured)
        assert!(thread_cpu() - cpu0 < Duration::from_millis(50), "a wait is no CPU time");
    }

    /// The chains' and the lead's jobs before the regions' the live plan holds back (its
    /// scheduler's offers lack them, none running) that wait on nothing in the forecast's items:
    /// none should (task #867).
    fn held_but_ready(inp: &Input) -> Vec<(String, String)> {
        let Some(st) = inp.sched.as_ref() else { return Vec::new() };
        let offered: BTreeSet<(&str, &str)> =
            st.offers.iter().flat_map(|o| o.targets.iter().map(move |t| (o.step.as_str(), t.0.as_str()))).collect();
        let listed: BTreeSet<(&str, &str)> =
            inp.chains.iter().chain(inp.before.iter()).map(|j| (j.0.as_str(), j.1.as_str())).collect();
        let Items { items, groups, .. } = items(inp);
        items
            .iter()
            .filter(|i| listed.contains(&(i.step.as_str(), i.target.as_str())))
            .filter(|i| !offered.contains(&(i.step.as_str(), i.target.as_str())))
            .filter(|i| !inp.running.contains_key(&(i.step.clone(), i.target.clone())))
            .filter(|i| i.deps.is_empty() && i.groups.iter().all(|&g| groups[g].is_empty()))
            .map(|i| (i.step.clone(), i.target.clone()))
            .collect()
    }

    /// Task #867 (term 604's record: a cold depth-root ready at the start though the live plan
    /// held it, so the lead refused every ask until its jobs ended): a replay never places a
    /// target the live plan holds back before what it waits on is done. The depth root waits for
    /// the depth and its assemblies (`chain_deps`, as build::depth_work); a held job of a chain
    /// `chain_deps` says nothing of waits on its chain's jobs before it. With
    /// SCENIC_FORECAST_RECORD, that record's input too.
    #[test]
    fn a_replay_never_places_a_target_the_live_plan_holds_back() {
        let c = |secs: f64| Cost { secs, known: true, peak_mb: 1000 };
        let regions: [RegionLeft; 0] = [];
        let unit = |_: &str, _: &str| c(1.0);
        let base = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 1.0, false)], &unit);
        let offer =
            |step: &str, t: &str| Offer { step: step.into(), targets: vec![(t.into(), "k".into(), 1000)], job: None };
        let job = |s: &str, t: &str, secs: f64| (s.to_string(), t.to_string(), c(secs));
        // (bldfield-lo stands for a chain step `chain_deps` would know nothing of: held, so after
        // the chain's jobs before it.)
        let chains = vec![
            job("depth", "6/1/1", 300.0),
            job("depth-lo", "3/0/0", 100.0),
            job("depth-root", "depth-root", 50.0),
            job("bldtiles", "6/2/2", 200.0),
        ];
        let st = State { offers: vec![offer("depth", "6/1/1"), offer("bldtiles", "6/2/2")], ..Default::default() };
        let inp = Input { chains, sched: Some(st), ..base };
        assert!(held_but_ready(&inp).is_empty(), "{:?}", held_but_ready(&inp));
        let f = forecast(&inp);
        let at = |s: &str| f.placed.iter().find(|p| p.step == s).map(|p| (p.from, p.until)).unwrap();
        assert!(at("depth-root").0 >= at("depth-lo").1 && at("depth-lo").0 >= at("depth").1, "{:?}", f.placed);
        // (The polar caps' chain, one step after another, as build::polar_next; and a held step
        // with no line in `chain_deps` (bldprep after bld-fetch) after its chain's jobs before it.)
        let chains = vec![
            job("polar-terrain", "n", 300.0),
            job("polar-slope", "n", 100.0),
            job("polar-root", "n", 50.0),
            job("bld-fetch", "all", 200.0),
            job("bldprep", "6/2/2", 20.0),
        ];
        let st = State { offers: vec![offer("polar-terrain", "n"), offer("bld-fetch", "all")], ..Default::default() };
        let inp = Input { chains, sched: Some(st), ..inp };
        assert!(held_but_ready(&inp).is_empty(), "{:?}", held_but_ready(&inp));
        let f = forecast(&inp);
        let at = |s: &str| f.placed.iter().find(|p| p.step == s).map(|p| (p.from, p.until)).unwrap();
        assert!(at("polar-root").0 >= at("polar-slope").1 && at("polar-slope").0 >= at("polar-terrain").1);
        assert!(at("bldprep").0 >= at("bld-fetch").1, "{:?}", f.placed);
        // (Held targets of one step wait on what's before them, not on each other; the heritage
        // chain runs beside the candidates (build::landmarks_work), held or not.)
        let chains = vec![
            job("bld-fetch", "all", 200.0),
            job("bldprep", "6/2/2", 20.0),
            job("bldprep", "6/3/3", 20.0),
            job("pois", "6/1/1", 300.0),
            job("heritage", "heritage", 50.0),
        ];
        let st = State { offers: vec![offer("bld-fetch", "all"), offer("pois", "6/1/1")], ..Default::default() };
        let inp = Input { chains, sched: Some(st), ..inp };
        let Items { items: its, groups, .. } = items(&inp);
        let waits = |s: &str, t: &str| -> BTreeSet<String> {
            let it = its.iter().find(|i| i.step == s && i.target == t).unwrap();
            it.deps
                .iter()
                .chain(it.groups.iter().flat_map(|&g| groups[g].iter()))
                .map(|&d| its[d].step.clone())
                .collect()
        };
        assert_eq!(waits("bldprep", "6/3/3"), BTreeSet::from(["bld-fetch".to_string()]));
        assert!(waits("heritage", "heritage").is_empty());
        if let Ok(p) = std::env::var("SCENIC_FORECAST_RECORD") {
            let r = Record::from_bytes(&std::fs::read(p).unwrap()).unwrap();
            let left = held_but_ready(&r.input);
            assert!(left.is_empty(), "{} held but ready: {:?}", left.len(), &left[..left.len().min(20)]);
            let f = forecast_by(&r.input, Duration::from_secs(60));
            let now = r.input.now;
            let offered: BTreeSet<(String, String)> = r
                .input
                .sched
                .as_ref()
                .unwrap()
                .offers
                .iter()
                .flat_map(|o| o.targets.iter().map(move |t| (o.step.clone(), t.0.clone())))
                .collect();
            let listed: BTreeSet<(String, String)> =
                r.input.chains.iter().map(|j| (j.0.clone(), j.1.clone())).collect();
            let early: Vec<&Placed> = f
                .placed
                .iter()
                .filter(|p| p.from <= now && listed.contains(&(p.step.clone(), p.target.clone())))
                .filter(|p| !offered.contains(&(p.step.clone(), p.target.clone())))
                .filter(|p| !r.input.running.contains_key(&(p.step.clone(), p.target.clone())))
                .collect();
            assert!(early.is_empty(), "held but placed at the start: {early:?}");
            // (The kept waits' size, for the record: the graph, compact (KeptWaits), and every set
            // spread into its items' deps.)
            let g = &f.graph;
            let n_deps = |g: &crate::coord::history::Graph| {
                g.deps.iter().map(Vec::len).sum::<usize>() + g.groups.iter().map(Vec::len).sum::<usize>()
            };
            let c = g.compact(&Default::default());
            assert!(serde_json::to_vec(&c).unwrap().len() <= serde_json::to_vec(g).unwrap().len(), "compact grew");
            let mut spread = g.clone();
            for (i, s) in g.sets.iter().enumerate() {
                for &k in s {
                    spread.deps[i].extend(&g.groups[k as usize]);
                }
            }
            spread.sets = vec![Vec::new(); g.keys.len()];
            spread.groups.clear();
            for (name, g) in [("graph", g), ("compact", &c), ("spread", &spread)] {
                eprintln!(
                    "{name}: {} items, {} deps, {} bytes",
                    g.keys.len(),
                    n_deps(g),
                    serde_json::to_vec(g).unwrap().len()
                );
            }
        }
    }

    /// A recorded forecast (SCENIC_FORECAST_RECORD: a forecast-input.json) timed. For profiling at live scale (unset: nothing);
    /// SCENIC_FORECAST_OUT and SCENIC_TOCOME_OUT write its forecast and its `/work/tocome` answer.
    #[test]
    fn recorded_timing() {
        let Ok(p) = std::env::var("SCENIC_FORECAST_RECORD") else { return };
        let r = Record::from_bytes(&std::fs::read(p).unwrap()).unwrap();
        let limit: u64 = std::env::var("SCENIC_FORECAST_LIMIT_S").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
        let mb = || crate::sys::footprint().unwrap_or(0) >> 20;
        let base = mb();
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watch = {
            let (peak, stop) = (peak.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    peak.fetch_max(crate::sys::footprint().unwrap_or(0) >> 20, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20)); // (sleep ok: a sampler)
                }
            })
        };
        let Items { items, groups, .. } = items(&r.input);
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
        let f = forecast_by(&r.input, Duration::from_secs(limit));
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
        // (`/work/tocome`'s answer as the lead would make it: for the page's tests, touch-test.mjs --tocome.)
        if let Ok(out) = std::env::var("SCENIC_TOCOME_OUT") {
            let (tc, _) = ToCome::take(&mut f.clone(), &r.input.machines);
            std::fs::write(out, serde_json::to_vec(&tc.answer()).unwrap()).unwrap();
        }
    }
}
