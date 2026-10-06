//! The build's forecast (docs/plan.md §8, The forecast; the worker page's road to done): when each
//! step, each region and the whole build will be done, when each region reaches the map, and what
//! each machine does next.
//!
//! The work left is run through in the order the agent runs it: the build Mac takes the first it
//! can (the pass's worldwide jobs, then a region at a time: its terrain, then its units; with none
//! it can do now, the slope, tree cover and the chains' work), its second job the first of its
//! steps (crate::agent::SECOND: the trains' and the landmarks' network steps, the candidates and
//! peaks, then units and slope) that fits beside it, each helper the far end of the first shared
//! step with work it can do that fits its memory (terrain's near end: the next region's; slope once
//! its area's terrain is built, a unit once its region's terrain is). The trains' and the
//! landmarks' chains run from the start, each step once what it reads is built. The round under way
//! goes first, with its own regions. A round of publishing goes out as the plan makes one: once a
//! region not on the map is done, an hour after the last round began (its slope and tree cover,
//! made by the build Mac while it waits, then the round's chain); after the last unit and terrain
//! area, the slope and tree cover left, the last round, then the overlays and a catalog with what
//! the chains made since. Each target takes its last run's time (else its step's mean, else a first
//! guess), at the speed measured for the machine doing it. It's run three times: as estimated, and
//! for a range, the times measured a little off and those guessed much more.

use super::build::RegionLeft;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The shared steps a helper takes, in its order of preference (crate::agent::claims::SHARED).
const SHARED: [&str; 6] = ["terrain", "slope", "trees", "unit", "pois", "peaks"];

// The steps the build Mac's second job takes, in its order of preference, and those it takes while
// the Mac is in use: the agent's.
use super::{LIGHT, SECOND};

/// A machine the work is shared among.
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub name: String,
    /// Its speed against the build Mac's (1), and whether that's measured.
    pub speed: f64,
    pub measured: bool,
    /// A helper: the shared steps alone, what fits its memory.
    pub helper: bool,
    /// The build Mac's second job: its steps alone (`SECOND`), what fits its memory; while the Mac is
    /// in use, only those that mostly wait on the network (`LIGHT`): for the first `light_s` seconds
    /// (in use now: as long as it's likely to stay so, `IN_USE_S`; the forecast is made every
    /// minute, and taken whole its finish jumped each time the owner came or went).
    pub second: bool,
    pub light_s: f64,
    pub mem_mb: u64,
    /// Seconds from now until it's free (its job under way's time left).
    pub busy_s: f64,
}

/// What a target takes: its time at the build Mac's pace (seconds), whether that time was measured,
/// and its memory (MB).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    pub secs: f64,
    pub known: bool,
    pub peak_mb: u64,
}

/// A job of the build Mac's alone, or one of the chains': (step, target, cost).
pub type Job = (String, String, Cost);

/// The work left, as the plan sees it (build::Plan), and what each target takes.
pub struct Input<'a> {
    pub now: u64,
    /// The build Mac's jobs before the regions' (the pass's worldwide jobs, the heritage sites).
    pub before: Vec<Job>,
    /// The regions' work left, in the order they're built (build::Plan::regions).
    pub regions: &'a [RegionLeft],
    /// What a shared step's target takes.
    pub cost: &'a dyn Fn(&str, &str) -> Cost,
    /// A round's chain (the map tiles, the road index, rail stops, ferries, a catalog): seconds; and
    /// the last round's, the roads' chain as it stands (0: none stale, and no round unless a region
    /// waits to be published).
    pub round_s: f64,
    pub last_round_s: f64,
    /// Why the work left can't all be listed now, when it can't (the units waiting for the pass's
    /// heritage sites, reaches or buildings; a new pass under way): no finish is forecast.
    pub blind: Option<String>,
    /// The trains' and the landmarks' chains (but the overlays): from the start, each step once what
    /// it reads is built (`chain_deps`).
    pub chains: Vec<Job>,
    /// After the last round: the overlays (they read the built units) and a catalog with what the
    /// chains made since.
    pub after: Vec<Job>,
    /// Seconds since the last round began (None: none has).
    pub since_last: Option<u64>,
    /// The round under way (crate::agent::build::Round): its regions, whether it's the last, and its
    /// chain's time left (the map tiles, road index, rail stops and catalog it has still to make).
    pub under_way: Option<(Vec<String>, bool, f64)>,
    /// The build Mac first, then the helpers.
    pub machines: Vec<Machine>,
    /// Targets being built now: (step, target) → the machine building it.
    pub running: BTreeMap<(String, String), usize>,
}

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
    /// Each machine's speed against the build Mac's, as used, and the machines whose speed is a
    /// guess (not yet measured).
    pub speed: BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guessed: Vec<String>,
    pub steps: Vec<StepFc>,
    pub regions: Vec<RegionFc>,
    /// The rounds of publishing to come, in order.
    pub rounds: Vec<RoundFc>,
    /// What each machine does next: its first few jobs.
    pub next: BTreeMap<String, Vec<NextFc>>,
    /// Each machine's work to the end, a step's run together (`Lane`): the schedule.
    #[serde(default)]
    pub lanes: BTreeMap<String, Vec<Lane>>,
}

/// A run of a machine's work of one step (or a round of publishing, "round"): from and until when
/// (unix seconds), and how many targets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub step: String,
    pub from: u64,
    pub until: u64,
    pub n: usize,
}

/// A step's work left.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StepFc {
    pub step: String,
    pub left: usize,
    /// Its time at the build Mac's pace (seconds), and how many of its targets' times are measured.
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
    /// Its targets left by step ("unit", "terrain", "slope", "trees").
    pub left: BTreeMap<String, usize>,
    /// When it'll be done, and when it'll be on the map (its round out).
    pub ready_at: Option<u64>,
    pub map_at: Option<u64>,
}

/// A round of publishing: when it goes out, the regions it adds, whether it's the last.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoundFc {
    pub at: u64,
    pub regions: Vec<String>,
    pub last: bool,
}

/// A machine's next job: its step and targets, from when to when (unix seconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NextFc {
    pub step: String,
    pub targets: Vec<String>,
    pub from: u64,
    pub until: u64,
}

/// When a job runs: before the regions, a region's (terrain, units), late (slope and tree cover:
/// the build Mac's in rounds and after the last unit), a chain's (the trains' and the landmarks':
/// from the start, once what it reads is built), or after the last round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Before,
    Region,
    Late,
    Chain,
    After,
}

/// What a chain's step waits for (crate::agent::build::landmarks_work): the candidates for the
/// pass's hiking-route ends; the peaks for every candidate, the pass's summits and the terrain; the
/// items' facts for every candidate; the rest of the heritage chain for the heritage sites; the
/// landmark points for those four; trains a day for their feeds. By step: (the jobs before the
/// regions', the chains', the terrain's).
fn chain_deps(step: &str) -> (&'static [&'static str], &'static [&'static str], bool) {
    match step {
        "pois" => (&["trailends"], &[], false),
        "peaks" => (&["summits"], &["pois"], true),
        "items" => (&[], &["pois"], false),
        "heritage" => (&["heritage-sites"], &[], false),
        "marks" => (&[], &["pois", "peaks", "items", "heritage"], false),
        "rail" => (&[], &["rail-feeds"], false),
        _ => (&[], &[], false),
    }
}

#[derive(Clone, Debug)]
struct Item {
    step: String,
    target: String,
    cost: Cost,
    phase: Phase,
    /// The items it waits for.
    deps: Vec<usize>,
    /// Whether a helper may do it (a shared step's, not before the regions').
    shared: bool,
    /// Being built now (its machine free when it's done).
    running: bool,
    /// Which machine, from and until when (seconds from now).
    by: Option<usize>,
    from: f64,
    end: f64,
}

/// One run through the work (`run`): each item's machine and times, and the rounds.
struct Sim {
    items: Vec<Item>,
    /// (start, end, regions, last)
    rounds: Vec<(f64, f64, Vec<usize>, bool)>,
    /// Region → its items.
    region_items: Vec<Vec<usize>>,
    done: bool,
}

const HOUR: f64 = 3600.0;
/// How long a Mac in use now is taken to stay in use, for its second job's work (`Machine::light_s`).
pub const IN_USE_S: f64 = 30.0 * 60.0;

/// The jobs before the regions' that the units wait for (build::plan).
const UNITS_NEED: [&str; 3] = ["heritage-sites", "reach", "buildings"];

/// The work as items, each with the items it waits for.
fn items(inp: &Input) -> (Vec<Item>, Vec<Vec<usize>>) {
    let mut out: Vec<Item> = Vec::new();
    let mut by_target: BTreeMap<(String, String), usize> = BTreeMap::new();
    let add = |out: &mut Vec<Item>, by: &mut BTreeMap<(String, String), usize>, step: &str, target: &str, cost: Cost, phase: Phase, deps: Vec<usize>| -> usize {
        if let Some(&i) = by.get(&(step.to_string(), target.to_string())) {
            return i;
        }
        let shared = SHARED.contains(&step) && phase != Phase::Before;
        out.push(Item { step: step.into(), target: target.into(), cost, phase, deps, shared, running: false, by: None, from: 0.0, end: 0.0 });
        by.insert((step.to_string(), target.to_string()), out.len() - 1);
        out.len() - 1
    };
    for (step, target, cost) in &inp.before {
        add(&mut out, &mut by_target, step, target, *cost, Phase::Before, Vec::new());
    }
    let before: Vec<usize> = (0..out.len()).collect();
    // (The units wait for the pass's heritage sites, reaches and roadside buildings, as the plan's
    // do; terrain for nothing.)
    let units_need: Vec<usize> = before.iter().copied().filter(|&i| UNITS_NEED.contains(&out[i].step.as_str())).collect();
    // A region at a time: its own terrain, then its own units (each once its region's terrain is
    // built: the terrain it reads); then every region's slope (once its area's terrain is) and
    // tree cover.
    let mut region_items: Vec<Vec<usize>> = vec![Vec::new(); inp.regions.len()];
    let terrain_of = |by: &BTreeMap<(String, String), usize>, t: &str| by.get(&("terrain".to_string(), t.to_string())).copied();
    for r in inp.regions {
        for t in &r.own_terrain {
            add(&mut out, &mut by_target, "terrain", t, (inp.cost)("terrain", t), Phase::Region, Vec::new());
        }
        // (Its terrain is its own or a region's before it: listed by now.)
        let mut deps: Vec<usize> = r.terrain.iter().filter_map(|t| terrain_of(&by_target, t)).collect();
        deps.extend(&units_need);
        for u in &r.own_units {
            add(&mut out, &mut by_target, "unit", u, (inp.cost)("unit", u), Phase::Region, deps.clone());
        }
    }
    for r in inp.regions {
        for a in &r.slope {
            let deps: Vec<usize> = terrain_of(&by_target, a).into_iter().collect();
            add(&mut out, &mut by_target, "slope", a, (inp.cost)("slope", a), Phase::Late, deps);
        }
        for a in &r.trees {
            add(&mut out, &mut by_target, "trees", a, (inp.cost)("trees", a), Phase::Late, Vec::new());
        }
    }
    // Each region's items: its terrain, all its units (some come with a region before it), its
    // slope and tree cover.
    for (k, r) in inp.regions.iter().enumerate() {
        let mut mine: Vec<usize> = Vec::new();
        for (step, list) in [("terrain", &r.terrain), ("unit", &r.units), ("slope", &r.slope), ("trees", &r.trees)] {
            mine.extend(list.iter().filter_map(|t| by_target.get(&(step.to_string(), t.clone())).copied()));
        }
        region_items[k] = mine;
    }
    // The chains: from the start, each step once what it reads is built.
    let terrain: Vec<usize> = (0..out.len()).filter(|&i| out[i].step == "terrain").collect();
    let mut chain: Vec<usize> = Vec::new();
    for (step, target, cost) in &inp.chains {
        let (pre, earlier, reads_terrain) = chain_deps(step);
        let mut deps: Vec<usize> = before.iter().copied().filter(|&i| pre.contains(&out[i].step.as_str())).collect();
        deps.extend(chain.iter().copied().filter(|&i| earlier.contains(&out[i].step.as_str())));
        if reads_terrain {
            deps.extend(&terrain);
        }
        let i = add(&mut out, &mut by_target, step, target, *cost, Phase::Chain, deps);
        chain.push(i);
    }
    // (Each after all before it: the catalog after the overlays.)
    let mut all: Vec<usize> = (0..out.len()).collect();
    for (step, target, cost) in &inp.after {
        let i = add(&mut out, &mut by_target, step, target, *cost, Phase::After, all.clone());
        all.push(i);
    }
    (out, region_items)
}

/// One run through the work, each item's time `scale`d.
fn run(inp: &Input, scale: &dyn Fn(&Cost) -> f64) -> Sim {
    let (mut items, region_items) = items(inp);
    for it in &mut items {
        it.cost.secs = scale(&it.cost).max(1.0);
    }
    let n = inp.machines.len();
    let mut free: Vec<f64> = inp.machines.iter().map(|m| m.busy_s.max(0.0)).collect();
    // What's being built now: done when its machine is free.
    for ((step, target), &m) in &inp.running {
        if let Some(it) = items.iter_mut().find(|i| &i.step == step && &i.target == target) {
            (it.by, it.from, it.end, it.running) = (Some(m), 0.0, free.get(m).copied().unwrap_or(0.0), true);
        }
    }
    let regionals: Vec<usize> = (0..items.len()).filter(|&i| matches!(items[i].phase, Phase::Region)).collect();
    // (A region on the map as it is, with work left (a new pass), goes out again once it's done.)
    let mut published: BTreeSet<usize> = (0..inp.regions.len()).filter(|&k| inp.regions[k].on_map == Some(true) && region_items[k].is_empty()).collect();
    let mut last_round: f64 = inp.since_last.map_or(f64::NEG_INFINITY, |s| -(s as f64));
    let mut rounds: Vec<(f64, f64, Vec<usize>, bool)> = Vec::new();
    let mut final_round_done = false;
    let mut under_way = inp.under_way.as_ref().map(|(ids, last, left)| (ids.iter().filter_map(|id| inp.regions.iter().position(|r| &r.id == id)).collect::<Vec<usize>>(), *last, *left));
    // (A machine with nothing it can do waits for the next item to end, or another machine to be
    // free: a round or a job that's no item may free work.)
    let next_end = |items: &[Item], free: &[f64], m: usize, t: f64| {
        let others = free.iter().enumerate().filter(|&(k, &f)| k != m && f > t + 1e-9).map(|(_, &f)| f);
        items.iter().filter(|i| i.by.is_some() && i.end > t + 1e-9).map(|i| i.end).chain(others).fold(f64::INFINITY, f64::min)
    };
    let done_by = |items: &[Item], i: usize, t: f64| items[i].by.is_some() && items[i].end <= t + 1e-9;
    let runnable = |items: &[Item], i: usize, t: f64| items[i].by.is_none() && items[i].deps.iter().all(|&d| done_by(items, d, t));
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 20 * (items.len() + 10) * n.max(1) {
            break;
        }
        let all_assigned = items.iter().all(|i| i.by.is_some());
        if all_assigned && final_round_done {
            break;
        }
        // The machine free first (the build Mac on a tie).
        let (m, t) = free.iter().copied().enumerate().fold((0, f64::INFINITY), |a, (i, f)| if f < a.1 - 1e-9 { (i, f) } else { a });
        if !t.is_finite() {
            break;
        }
        let mac = &inp.machines[m];
        let take = |items: &mut Vec<Item>, i: usize, from: f64| -> f64 {
            let end = from + items[i].cost.secs / mac.speed.max(0.01);
            (items[i].by, items[i].from, items[i].end) = (Some(m), from, end);
            end
        };
        if mac.helper {
            // The far end of the first shared step it can do (terrain's near end: the build Mac's
            // next units wait on it, as crate::coord picks).
            let pick = SHARED.iter().find_map(|s| {
                let fits = |&i: &usize| items[i].shared && items[i].step == *s && items[i].cost.peak_mb <= mac.mem_mb && runnable(&items, i, t);
                if *s == "terrain" { (0..items.len()).find(fits) } else { (0..items.len()).rev().find(fits) }
            });
            free[m] = match pick {
                Some(i) => take(&mut items, i, t),
                None => next_end(&items, &free, m, t),
            };
            continue;
        }
        if mac.second {
            // The first of its steps it can do, in their order (a step's in the plan's).
            let steps: &[&str] = if t < mac.light_s { &LIGHT } else { &SECOND };
            let pick = steps.iter().find_map(|s| (0..items.len()).find(|&i| items[i].phase != Phase::Before && items[i].step == *s && items[i].cost.peak_mb <= mac.mem_mb && runnable(&items, i, t)));
            // (With no network work while the Mac's in use, it looks again when that time's up.)
            free[m] = match pick {
                Some(i) => take(&mut items, i, t),
                None if t < mac.light_s => next_end(&items, &free, m, t).min(mac.light_s),
                None => next_end(&items, &free, m, t),
            };
            continue;
        }
        // The build Mac. The round under way first: its regions' slope and tree cover left (after
        // the last, all that's left), then its chain.
        if let Some((ks, last, left)) = under_way.take() {
            let mut at = t;
            let mut wait = t;
            for i in 0..items.len() {
                if items[i].phase == Phase::Late && (last || ks.iter().any(|&k| region_items[k].contains(&i))) {
                    if items[i].by.is_none() {
                        at = take(&mut items, i, at);
                    } else {
                        wait = wait.max(items[i].end);
                    }
                }
            }
            let end = at.max(wait) + left;
            let out: Vec<usize> = if last { (0..inp.regions.len()).filter(|k| !published.contains(k)).collect() } else { ks };
            published.extend(out.iter().copied());
            rounds.push((t, end, out, last));
            final_round_done |= last;
            free[m] = end;
            continue;
        }
        // A round when one's due: a region done that the map hasn't as it is, an hour after the last
        // began (its slope and tree cover first, if they're not made by then; the catalog waits for
        // those a helper builds).
        let regional_left = regionals.iter().any(|&i| !done_by(&items, i, t));
        let ready: Vec<usize> = (0..inp.regions.len())
            .filter(|k| !published.contains(k))
            .filter(|&k| region_items[k].iter().filter(|&&i| matches!(items[i].phase, Phase::Region)).all(|&i| done_by(&items, i, t)))
            .collect();
        // (Only a region the map hasn't as it is now makes a round due, as the plan's: one on it
        // that's rebuilt (a new pass) goes out with another's round, or the last.)
        let due: Vec<usize> = ready.iter().copied().filter(|&k| inp.regions[k].on_map != Some(true)).collect();
        if regional_left && !due.is_empty() && t - last_round >= HOUR {
            let mut at = t;
            let mut wait = t;
            for &k in &due {
                for &i in &region_items[k] {
                    if items[i].phase == Phase::Late {
                        if items[i].by.is_none() {
                            at = take(&mut items, i, at);
                        } else {
                            wait = wait.max(items[i].end);
                        }
                    }
                }
            }
            let end = at.max(wait) + inp.round_s;
            // Its catalog carries those, and the rebuilt ones ready by then whose slope and tree
            // cover are done too (the catalog records a region built only then).
            let mut out = due;
            out.extend(ready.iter().copied().filter(|&k| inp.regions[k].on_map == Some(true) && region_items[k].iter().all(|&i| done_by(&items, i, end))));
            published.extend(out.iter().copied());
            rounds.push((t, end, out, false));
            last_round = t;
            free[m] = end;
            continue;
        }
        if regional_left || items.iter().any(|i| i.phase == Phase::Before && i.by.is_none()) {
            // The first it can do of the pass's; then the slope and tree cover of a region done and
            // waiting for its round (so the round only draws); then the regions' (in order); with
            // none it can do now, the chains' (listed after them).
            let prep = || ready.iter().filter(|&&k| inp.regions[k].on_map != Some(true)).flat_map(|&k| region_items[k].iter().copied()).find(|&i| items[i].phase == Phase::Late && runnable(&items, i, t));
            let pick = (0..items.len())
                .find(|&i| items[i].phase == Phase::Before && runnable(&items, i, t))
                .or_else(prep)
                .or_else(|| (0..items.len()).find(|&i| items[i].phase == Phase::Region && runnable(&items, i, t)))
                .or_else(|| (0..items.len()).find(|&i| items[i].phase == Phase::Chain && runnable(&items, i, t)));
            free[m] = match pick {
                Some(i) => take(&mut items, i, t),
                None => next_end(&items, &free, m, t),
            };
            continue;
        }
        // After the last unit and terrain: the slope and tree cover left, then the last round (once
        // every region's work is done: the chains' work while others build what it waits for); then
        // the chains' work left, the overlays and a catalog.
        if let Some(i) = (0..items.len()).find(|&i| items[i].phase == Phase::Late && runnable(&items, i, t)) {
            free[m] = take(&mut items, i, t);
            continue;
        }
        if !final_round_done {
            let late_open = items.iter().any(|i| i.phase == Phase::Late && !(i.by.is_some() && i.end <= t + 1e-9));
            if late_open {
                free[m] = match (0..items.len()).find(|&i| items[i].phase == Phase::Chain && runnable(&items, i, t)) {
                    Some(i) => take(&mut items, i, t),
                    None => next_end(&items, &free, m, t),
                };
                continue;
            }
            // (None when nothing's stale and no region waits to go out.)
            let rest: Vec<usize> = (0..inp.regions.len()).filter(|k| !published.contains(k)).collect();
            final_round_done = true;
            if rest.is_empty() && inp.last_round_s <= 0.0 {
                continue;
            }
            let end = t + if inp.last_round_s > 0.0 { inp.last_round_s } else { inp.round_s };
            published.extend(rest.iter().copied());
            rounds.push((t, end, rest, true));
            free[m] = end;
            continue;
        }
        let pick = (0..items.len()).find(|&i| matches!(items[i].phase, Phase::Chain | Phase::After) && runnable(&items, i, t));
        free[m] = match pick {
            Some(i) => take(&mut items, i, t),
            None => next_end(&items, &free, m, t),
        };
    }
    let done = items.iter().all(|i| i.by.is_some()) && final_round_done;
    Sim { items, rounds, region_items, done }
}

/// The forecast for `inp`.
pub fn forecast(inp: &Input) -> Forecast {
    let now = inp.now as f64;
    let expected = run(inp, &|c| c.secs);
    let soon = run(inp, &|c| c.secs * if c.known { 0.95 } else { 0.75 });
    let late = run(inp, &|c| c.secs * if c.known { 1.15 } else { 1.6 });
    let end_of = |s: &Sim| -> Option<f64> {
        s.done.then(|| s.items.iter().map(|i| i.end).chain(s.rounds.iter().map(|r| r.1)).fold(0.0, f64::max))
    };
    let at = |secs: f64| (now + secs).round() as u64;
    let mut f = Forecast { at: inp.now, ..Default::default() };
    let nothing = expected.items.is_empty() && expected.rounds.is_empty();
    f.why = match (&inp.blind, nothing, expected.done) {
        (Some(b), _, _) => Some(b.clone()),
        (None, true, _) => Some("nothing left to build".into()),
        (None, false, false) => Some("there's work no machine can do (none that fits it is around)".into()),
        _ => None,
    };
    if f.why.is_none() {
        f.done_at = end_of(&expected).map(at);
        f.range = match (end_of(&soon), end_of(&late)) {
            (Some(a), Some(b)) => Some([at(a.min(b)), at(a.max(b))]),
            _ => None,
        };
    }
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
    // Each region: its work left, when it's done and when it's on the map.
    let round_of = |k: usize| expected.rounds.iter().find(|r| r.2.contains(&k)).map(|r| r.1);
    for (k, r) in inp.regions.iter().enumerate() {
        let mine = &expected.region_items[k];
        let mut left: BTreeMap<String, usize> = BTreeMap::new();
        for &i in mine {
            *left.entry(expected.items[i].step.clone()).or_default() += 1;
        }
        let ready = mine.iter().all(|&i| expected.items[i].by.is_some()).then(|| mine.iter().map(|&i| expected.items[i].end).fold(0.0, f64::max));
        f.regions.push(RegionFc {
            id: r.id.clone(),
            rank: k,
            on_map: r.on_map,
            left,
            ready_at: ready.map(at),
            map_at: if r.on_map == Some(true) && mine.is_empty() { None } else { round_of(k).map(at) },
        });
    }
    f.rounds = expected.rounds.iter().map(|(_, t, ks, last)| RoundFc { at: at(*t), regions: ks.iter().map(|&k| inp.regions[k].id.clone()).collect(), last: *last }).collect();
    // Each machine's next jobs: its items in time order (not those it's building now), a step's run
    // together (at most three).
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
                _ => next.push(NextFc { step: i.step.clone(), targets: vec![i.target.clone()], from: at(i.from), until: at(i.end) }),
            }
        }
        f.next.insert(mac.name.clone(), next);
        // Its schedule: its runs of a step, and the build Mac's rounds.
        let mut runs: Vec<(String, f64, f64, usize)> = expected.items.iter().filter(|i| i.by == Some(m)).map(|i| (i.step.clone(), i.from, i.end, 1)).collect();
        if m == 0 {
            runs.extend(expected.rounds.iter().map(|r| ("round".to_string(), r.0, r.1, r.2.len())));
        }
        runs.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut lane: Vec<Lane> = Vec::new();
        for (step, from, until, n) in runs {
            match lane.last_mut() {
                Some(l) if l.step == step && step != "round" && from <= l.until as f64 - now + 120.0 => {
                    l.until = at(until);
                    l.n += n;
                }
                _ => lane.push(Lane { step, from: at(from), until: at(until), n }),
            }
        }
        f.lanes.insert(mac.name.clone(), lane);
    }
    f
}

/// About how long a round took lately (seconds): the last five catalogs', each the jobs of the
/// round's chain since the catalog before (the history's ends) and itself. None before one's gone.
pub fn round_secs(events: &[crate::coord::history::Event]) -> Option<f64> {
    const CHAIN: [&str; 9] = ["prune", "pack", "lo", "roadunits", "stations", "ferries", "terrain-root", "slope-root", "catalog"];
    let mut rounds: Vec<f64> = Vec::new();
    let (mut sum, mut chain) = (0.0, false);
    for e in events {
        if e.kind == "end" && e.step.as_deref().is_some_and(|s| CHAIN.contains(&s)) {
            sum += e.secs.unwrap_or(0.0);
            chain |= e.step.as_deref() != Some("catalog");
        }
        // (A catalog after the trains' or the landmarks' chain alone is no round.)
        if e.kind == "catalog" {
            if chain {
                rounds.push(sum);
            }
            (sum, chain) = (0.0, false);
        }
    }
    let last = &rounds[rounds.len().saturating_sub(5)..];
    (!last.is_empty()).then(|| last.iter().sum::<f64>() / last.len() as f64)
}

/// Each helper's speed against the build Mac's, from the history: for each shared step both did,
/// the build Mac's mean time a target over the helper's (the last week's, three or more each),
/// their middle; `default` where there's too little to tell.
pub fn speeds(events: &[crate::coord::history::Event], build_mac: &str, helpers: &[String], default: f64) -> BTreeMap<String, (f64, bool)> {
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
                    let theirs = per.get(&(build_mac.to_string(), s.to_string())).filter(|v| v.len() >= 3)?;
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
    use super::*;

    fn region(id: &str, terrain: &[&str], units: &[&str], slope: &[&str]) -> RegionLeft {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        RegionLeft { id: id.into(), on_map: None, units: v(units), own_units: v(units), terrain: v(terrain), own_terrain: v(terrain), slope: v(slope), trees: Vec::new() }
    }

    fn mac(name: &str, speed: f64, helper: bool) -> Machine {
        Machine { name: name.into(), speed, measured: true, helper, second: false, light_s: 0.0, mem_mb: 6000, busy_s: 0.0 }
    }

    fn cost(step: &str, _t: &str) -> Cost {
        let secs = match step {
            "terrain" => 600.0,
            "unit" => 300.0,
            "slope" => 200.0,
            _ => 100.0,
        };
        Cost { secs, known: step != "slope", peak_mb: 1000 }
    }

    fn input<'a>(regions: &'a [RegionLeft], machines: Vec<Machine>, c: &'a dyn Fn(&str, &str) -> Cost) -> Input<'a> {
        Input { now: 1_000_000, before: Vec::new(), regions, cost: c, round_s: 600.0, last_round_s: 600.0, blind: None, chains: Vec::new(), after: vec![("marks".into(), "marks".into(), Cost { secs: 300.0, known: true, peak_mb: 0 })], since_last: None, under_way: None, machines, running: BTreeMap::new() }
    }

    #[test]
    fn one_mac_runs_a_region_then_publishes_it() {
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &["3/1/1"])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        // Terrain 600, two units 600: the last of the regions' work, so the last round follows: its
        // slope 200 (it's done then) and the chain 600; then the landmarks (300).
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 1400));
        assert_eq!(f.regions[0].map_at, Some(1_000_000 + 2000));
        assert_eq!(f.rounds.len(), 1);
        assert!(f.rounds[0].last && f.rounds[0].regions == ["a"]);
        assert_eq!(f.done_at, Some(1_000_000 + 2000 + 300));
        let steps: Vec<(&str, usize, u64)> = f.steps.iter().map(|s| (s.step.as_str(), s.left, s.work_s)).collect();
        assert_eq!(steps, [("terrain", 1, 600), ("unit", 2, 600), ("slope", 1, 200), ("marks", 1, 300)]);
        // The slope was guessed: its share of the time isn't measured.
        assert!((f.measured - 1500.0 / 1700.0).abs() < 0.01);
        let r = f.range.unwrap();
        assert!(r[0] < f.done_at.unwrap() && f.done_at.unwrap() < r[1]);
        assert_eq!(f.next["m4"].iter().map(|n| (n.step.as_str(), n.targets.len())).collect::<Vec<_>>(), [("terrain", 1), ("unit", 2), ("slope", 1)]);
        let lane: Vec<(&str, u64, u64, usize)> = f.lanes["m4"].iter().map(|l| (l.step.as_str(), l.from - 1_000_000, l.until - 1_000_000, l.n)).collect();
        assert_eq!(lane, [("terrain", 0, 600, 1), ("unit", 600, 1200, 2), ("slope", 1200, 1400, 1), ("round", 1400, 2000, 1), ("marks", 2000, 2300, 1)]);
    }

    #[test]
    fn a_helper_takes_the_far_end_then_what_it_frees() {
        let regions = [region("a", &["3/1/1"], &["6/8/8"], &[]), region("b", &["3/2/2"], &["6/16/16"], &[])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost));
        // The M1 takes b's terrain (the far end) at half the pace (1,200 s), then b's unit, its terrain
        // built (600 s); the build Mac a's terrain and unit (900 s), then a's round (600), then waits.
        assert_eq!(f.next["m1"].iter().map(|n| (n.step.as_str(), n.targets[0].as_str())).collect::<Vec<_>>(), [("terrain", "3/2/2"), ("unit", "6/16/16")]);
        assert_eq!((f.regions[0].ready_at, f.regions[0].map_at), (Some(1_000_000 + 900), Some(1_000_000 + 1500)));
        // b done at 1,800: the last of the regions', so the last round goes out at once after.
        assert_eq!((f.regions[1].ready_at, f.regions[1].map_at), (Some(1_000_000 + 1800), Some(1_000_000 + 2400)));
        assert!(!f.rounds[0].last && f.rounds[1].last && f.rounds[1].regions == ["b"]);
    }

    #[test]
    fn a_helper_takes_the_next_regions_terrain_not_the_last_ones() {
        let regions = [region("a", &["3/1/1"], &["6/8/8"], &[]), region("b", &["3/2/2"], &["6/16/16"], &[]), region("c", &["3/3/3"], &["6/24/24"], &[])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost));
        // The build Mac takes a's terrain; the M1 b's (whose units the build Mac builds next), not c's.
        assert_eq!(f.next["m1"].first().map(|n| (n.step.as_str(), n.targets[0].as_str())), Some(("terrain", "3/2/2")));
    }

    #[test]
    fn a_round_goes_out_at_most_hourly() {
        // a (300 s), its round from 300, out at 900; b and c done within the hour after: they wait
        // for the first pick an hour after a's round began (d's units, 300 s each, go on meanwhile).
        let d: Vec<String> = (0..20).map(|i| format!("6/9/{i}")).collect();
        let d: Vec<&str> = d.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &["6/1/1"], &[]), region("b", &[], &["6/2/2"], &[]), region("c", &[], &["6/3/3"], &[]), region("d", &[], &d, &[])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        assert_eq!(f.regions[0].map_at, Some(1_000_000 + 900));
        assert_eq!(f.regions[1].ready_at, Some(1_000_000 + 1200));
        assert_eq!(f.regions[1].map_at, Some(1_000_000 + 3900 + 600));
        assert_eq!(f.rounds[1].regions, ["b", "c"]);
    }

    #[test]
    fn the_round_under_way_goes_first_with_its_own_regions() {
        // a and b done, a round under way with a alone (b was done after it began), its chain 400 s
        // from done; c's units after it. b's slope is made while it waits (200 s); its round, an
        // hour after a's began (2,000 s ago), at the first pick from 1,600: 1,800, after c's units.
        let regions = [region("a", &[], &[], &[]), region("b", &[], &[], &["3/2/2"]), region("c", &[], &["6/3/3", "6/3/4", "6/3/5", "6/3/6", "6/3/7", "6/3/8"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        (inp.since_last, inp.under_way) = (Some(2000), Some((vec!["a".to_string()], false, 400.0)));
        let f = forecast(&inp);
        assert_eq!((f.rounds[0].regions.as_slice(), f.rounds[0].at), (["a".to_string()].as_slice(), 1_000_000 + 400));
        let lane: Vec<(&str, u64)> = f.lanes["m4"].iter().map(|l| (l.step.as_str(), l.from - 1_000_000)).collect();
        assert_eq!(&lane[..4], [("round", 0), ("slope", 400), ("unit", 600), ("round", 1800)]);
        assert_eq!((f.rounds[1].regions.as_slice(), f.rounds[1].at), (["b".to_string()].as_slice(), 1_000_000 + 1800 + 600));
    }

    #[test]
    fn regions_are_built_one_at_a_time() {
        let regions = [region("a", &["3/1/1"], &["6/8/8"], &[]), region("b", &["3/2/2"], &["6/16/16"], &[])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        // a's terrain and unit, a's round, then b's terrain and unit: a done first.
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 900));
        assert_eq!(f.regions[1].ready_at, Some(1_000_000 + 900 + 600 + 900));
        assert_eq!(f.measured, 1.0);
    }

    #[test]
    fn a_region_whose_terrain_a_helper_builds_waits_for_it() {
        // One region: the M1 has its terrain now (900 s left); the build Mac waits for it.
        let regions = [region("a", &["3/1/1"], &["6/8/8"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), Machine { busy_s: 900.0, ..mac("m1", 0.5, true) }], &cost);
        inp.running.insert(("terrain".into(), "3/1/1".into()), 1);
        let f = forecast(&inp);
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 900 + 300));
    }

    #[test]
    fn a_helper_isnt_stranded_by_a_round_or_a_worldwide_job() {
        // The build Mac runs a worldwide job (no item) for an hour; the helper takes the terrain at
        // once (terrain waits for nothing), then the units once their region's terrain is built
        // and the job the units need is done: it wakes when the build Mac is free.
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &[])];
        let mut inp = input(&regions, vec![Machine { busy_s: 0.0, ..mac("m4", 1.0, false) }, mac("m1", 1.0, true)], &cost);
        inp.before = vec![("reach".into(), "reach".into(), Cost { secs: 3600.0, known: true, peak_mb: 0 })];
        let f = forecast(&inp);
        assert_eq!(f.next["m1"][0].step, "terrain");
        assert_eq!(f.next["m1"][0].from, 1_000_000);
        // Both units after the hour: one each.
        let lanes: Vec<&str> = f.lanes["m1"].iter().map(|l| l.step.as_str()).collect();
        assert_eq!(lanes, ["terrain", "unit"]);
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 3600 + 300));
    }

    #[test]
    fn nothing_left_is_no_finish_and_regions_on_the_map_go_out_again() {
        let none: [RegionLeft; 0] = [];
        let mut inp = input(&none, vec![mac("m4", 1.0, false)], &cost);
        (inp.after, inp.last_round_s) = (Vec::new(), 0.0);
        let f = forecast(&inp);
        assert_eq!((f.done_at, f.why.as_deref()), (None, Some("nothing left to build")));
        assert!(f.rounds.is_empty());
        // A region on the map as it is, rebuilt (a new pass): out again once it's done.
        let regions = [RegionLeft { on_map: Some(true), ..region("a", &[], &["6/8/8"], &[]) }];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        assert_eq!(f.regions[0].map_at, Some(1_000_000 + 300 + 600));
        // Work that can't all be listed: no finish, and why.
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        inp.blind = Some("the areas wait for the pass's reaches".into());
        let f = forecast(&inp);
        assert_eq!((f.done_at, f.why.as_deref()), (None, Some("the areas wait for the pass's reaches")));
    }

    #[test]
    fn a_rebuilt_region_goes_out_with_another_round_or_the_last() {
        // Two regions on the map as they are, rebuilt (a new pass), and one the map hasn't: only the
        // new one makes a round due; the rebuilt ones ready by then go out with it, the other with the
        // last round.
        let d: Vec<String> = (0..20).map(|i| format!("6/9/{i}")).collect();
        let d: Vec<&str> = d.iter().map(String::as_str).collect();
        let on = |r: RegionLeft| RegionLeft { on_map: Some(true), ..r };
        let regions = [on(region("a", &[], &["6/1/1"], &[])), region("b", &[], &["6/2/2"], &[]), on(region("c", &[], &d, &[]))];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        // a (300 s), b (300 s): b's round at 600 carries a too; c after it (6,000 s), in the last.
        assert_eq!(f.rounds.len(), 2);
        assert_eq!(f.rounds[0].regions, ["b", "a"]);
        assert!(f.rounds[1].last && f.rounds[1].regions == ["c"]);
        assert_eq!(f.regions[0].map_at, Some(1_000_000 + 1200));
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
    fn the_chains_go_on_beside_the_regions_on_the_second_job() {
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &["3/1/1"])];
        // The build Mac alone: terrain 600, the units 600, the slope 200 and the last round 600;
        // then the chains, one after another (3,200 s).
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        (inp.chains, inp.after) = (chains(), Vec::new());
        let alone = forecast(&inp);
        assert_eq!(alone.done_at, Some(1_000_000 + 2000 + 5200));
        // With its second job: the heritage chain from the start, beside the regions' work; the
        // rest after the last round, the landmark points once the heritage chain is done too.
        let second = Machine { second: true, mem_mb: 12_000, ..mac("m4 (second job)", 1.0, false) };
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), second], &cost);
        (inp.chains, inp.after) = (chains(), Vec::new());
        let f = forecast(&inp);
        let lane = &f.lanes["m4 (second job)"];
        assert_eq!((lane[0].step.as_str(), lane[0].from, lane[0].until), ("heritage", 1_000_000, 1_003_000));
        assert_eq!(f.done_at, Some(1_000_000 + 4200));
        assert_eq!(f.rounds.len(), 1);
        assert_eq!(f.rounds[0].at, 1_000_000 + 2000, "the chains don't hold up the last round");
    }

    #[test]
    fn the_second_job_builds_units_only_while_the_mac_isnt_in_use() {
        let units: Vec<String> = (0..8).map(|i| format!("6/8/{i}")).collect();
        let refs: Vec<&str> = units.iter().map(String::as_str).collect();
        let regions = [region("a", &[], &refs, &[])];
        let second = |light_s: f64| Machine { second: true, light_s, mem_mb: 12_000, ..mac("m4 (second job)", 1.0, false) };
        let took = |light_s: f64| {
            let f = forecast(&input(&regions, vec![mac("m4", 1.0, false), second(light_s)], &cost));
            f.lanes.get("m4 (second job)").map_or(0, |l| l.iter().filter(|x| x.step == "unit").map(|x| x.n).sum::<usize>())
        };
        // Away: half the units; in use throughout: none (it waits for network work); in use for the
        // first unit's time: one fewer.
        assert_eq!(took(0.0), 4);
        assert_eq!(took(f64::INFINITY), 0);
        assert_eq!(took(cost("unit", "6/8/0").secs), 3);
        // The build Mac's first job two hours from done, the Mac in use for ten minutes: the second
        // job takes the units from then, not once the first job ends.
        let f = forecast(&input(&regions, vec![Machine { busy_s: 7200.0, ..mac("m4", 1.0, false) }, second(600.0)], &cost));
        let lane = &f.lanes["m4 (second job)"];
        assert_eq!((lane[0].step.as_str(), lane[0].from, lane.iter().filter(|x| x.step == "unit").map(|x| x.n).sum::<usize>()), ("unit", 1_000_000 + 600, 8));
    }

    #[test]
    fn a_helper_takes_the_candidates_while_the_units_wait() {
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &["3/1/1"])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        (inp.chains, inp.after) = (chains(), Vec::new());
        let f = forecast(&inp);
        // While the build Mac builds the terrain the units wait for, the helper makes the candidates
        // (from the far end), 200 s each at its pace.
        let m1 = &f.lanes["m1"];
        assert_eq!((m1[0].step.as_str(), m1[0].from, m1[0].until, m1[0].n), ("pois", 1_000_000, 1_000_400, 2));
    }

    #[test]
    fn helpers_speed_from_the_history() {
        use crate::coord::history::Event;
        let e = |w: &str, secs: f64, n: usize| Event { kind: "done".into(), worker: Some(w.into()), step: Some("unit".into()), secs: Some(secs), ok: Some(true), targets: vec!["x".into(); n], ..Default::default() };
        let mut events = vec![e("m4", 300.0, 1), e("m4", 600.0, 2), e("m4", 300.0, 1)];
        events.extend([e("m1", 600.0, 1), e("m1", 600.0, 1), e("m1", 600.0, 1)]);
        let s = speeds(&events, "m4", &["m1".into(), "ipad".into()], 0.5);
        assert!((s["m1"].0 - 0.5).abs() < 1e-9 && s["m1"].1);
        assert_eq!(s["ipad"], (0.5, false));
    }
}
