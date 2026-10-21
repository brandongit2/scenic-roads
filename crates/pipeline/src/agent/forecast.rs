//! The build's forecast (docs/building.md §2, The forecast; the worker page's road to done): when each
//! step, each region and the whole build will be done, when each region reaches the map, when the
//! cadenced publishes may next go out, and what each machine does next.
//!
//! The work left is run through in the order the agent runs it: the lead's first slot takes the
//! first it can (the pass's worldwide jobs, then the map tiles of the units built, then a region at
//! a time: its terrain, then its units; with none it can do now, the slope, tree cover and the
//! chains' work). Every other slot, the lead's and each member's (a lane each, `equal`; as many as
//! each Mac may run now, and the slots the lead's ramp grows to later, `Machine::from_s`), takes
//! from the front of the plan's order the first work that it may run (a member never the lead's own
//! steps) and that fits its Mac's room beside what the Mac runs, as the coordinator packs an ask
//! (`MacRoom`: its whole memory and its cores at the packed share each; a step not measured alone;
//! a job an ask while a member runs an older app); the pages' lane (`helper`) the far end of the
//! first shared step it can do (terrain's near end). Each lane starts with its slot's job under
//! way. The trains' and the landmarks' chains run from the start, each step once what it reads is
//! built. Publishing goes per unit (docs/building.md §2, Publishing): each map tile of a region
//! (`RegionLeft::packs`, at its own measured time) is drawn once the units that draw it are built,
//! ahead of the regions' work, and a catalog follows within the minute (build::CATALOG_EVERY_S):
//! the region is all on the map then (none said once only slope and tree cover are left). The whole-map steps go by their cadence (`Forecast::publishes`:
//! build::low_gate, build::gate_open). Each target takes its last run's time (else its step's mean,
//! else a first guess), at the speed measured for the machine doing it. It's run three times: as
//! estimated, and for a range, the times measured a little off and those guessed much more.

use super::build::{RegionLeft, CATALOG_EVERY_S, LOW_EVERY_S, MARKS_EVERY_S, OVERLAYS_EVERY_S};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;

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
    /// any step offered that fits), and every lane but the first of the forecast for equal machines
    /// (`equal_machines`).
    pub equal: bool,
    pub mem_mb: u64,
    /// Seconds from now until it's free (its job under way's time left).
    pub busy_s: f64,
    /// What each unit takes it more (seconds): the lead's, while a worker that takes tails is
    /// around, the moment its job gives one to take its tail (crate::offload::LEASE_WAIT). (A
    /// terrain piece's subtrees out with workers, crate::terrain_task, take none: they're offered as
    /// the piece begins and waited for only after its own run, by then taken or not, and its
    /// measured time, the job's, has those waits in it.)
    pub unit_extra_s: f64,
    /// It may run the lead's own steps (`steps::Place::Lead`): the lead's slots; a member's take
    /// only those any Mac may (crate::agent::every::grants).
    pub lead_steps: bool,
    /// The job its slot runs now, when no item of the work left is it (a step's job the plan lists
    /// no target of, a daily one): its step and targets, until `busy_s`. On its lane first.
    pub running: Option<(String, usize)>,
    /// That job's memory (MB: its step's first target's peak), held on its Mac until `busy_s`.
    pub running_mb: u64,
    /// The Mac whose room (`Input::macs`) its slot shares with that Mac's other slots, as the
    /// coordinator packs an ask (crate::coord's `pack`); None: its own `mem_mb` alone.
    pub mac: Option<String>,
    /// Seconds from now until its slot opens: a slot the lead's ramp grows to later (one each
    /// clean stretch, crate::agent's RAMP_CLEAN_S); 0, open now.
    pub from_s: f64,
}

/// A Mac's room, as the coordinator packs its asks: its memory (MB; 0, not known) and cores, its
/// jobs packed at the packed share each (crate::coord::packed_share).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MacRoom {
    pub mem_mb: u64,
    pub cores: u32,
}

/// While a member runs an older app the coordinator grants one job an ask (crate::coord's
/// `old_member`): a Mac's jobs then start this far apart at least (its asks', the coordinator's
/// `ASKING`).
pub const OLD_APP_GAP_S: f64 = 30.0;

/// What a target takes: its time at the lead's pace (seconds), whether that time was measured,
/// and its memory (MB).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub secs: f64,
    pub known: bool,
    pub peak_mb: u64,
    /// The cores it's measured to take (crate::needs::Book::cores; None: not said, the packed
    /// share).
    pub cores: Option<u32>,
}

/// What a Mac runs at a moment of the run (`run`'s `snap`): its room, jobs and their memory and
/// packed cores, whether one is a cold start, when its last job started, and the steps under way
/// anywhere.
#[derive(Default)]
struct Snap {
    room: MacRoom,
    share: u32,
    jobs: usize,
    mem_mb: u64,
    cores: u32,
    cold: bool,
    last_start: f64,
    steps: std::collections::BTreeSet<String>,
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
    /// What each region's target takes, by step and target (`cost_table`: every target of
    /// `regions` the forecast lists).
    pub costs: CostTable,
    /// The cadenced publishes' last runs.
    pub cadence: Cadence,
    /// Why the work left can't all be listed now, when it can't (the units waiting for the pass's
    /// heritage sites, reaches or buildings; a new pass under way): no finish is forecast.
    pub blind: Option<String>,
    /// The trains' and the landmarks' chains (but the overlays): from the start, each step once what
    /// it reads is built (`chain_deps`).
    pub chains: Vec<Job>,
    /// After everything else: the overlays (they read the built units) and a catalog with what the
    /// chains made since.
    pub after: Vec<Job>,
    /// Last of all, in idle time: the tree cover pieces' mids (build::TreeWork::backfill), each
    /// once everything else is done, side by side.
    pub idle: Vec<Job>,
    /// The lead first, then the members.
    pub machines: Vec<Machine>,
    /// Targets being built now: (step, target) → the machine building it.
    #[serde(with = "pairs")]
    pub running: BTreeMap<(String, String), usize>,
    /// Each Mac's room, by the name its lanes give (`Machine::mac`).
    pub macs: BTreeMap<String, MacRoom>,
    /// A member runs an older app: one job an ask (`OLD_APP_GAP_S`).
    pub old_app: bool,
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
/// and assemblies, own and expected units, map tiles, slope and tree cover), each by `cost`.
pub fn cost_table(regions: &[RegionLeft], cost: &dyn Fn(&str, &str) -> Cost) -> CostTable {
    let mut t = CostTable::new();
    let mut put = |step: &str, ts: &mut dyn Iterator<Item = &String>| {
        let e = t.entry(step.to_string()).or_default();
        for x in ts {
            e.entry(x.clone()).or_insert_with(|| cost(step, x));
        }
    };
    for r in regions {
        put("terrain", &mut r.own_terrain.iter());
        put("terrain-lo", &mut r.own_terrain_lo.iter());
        put("unit", &mut r.own_units.iter().chain(&r.expected));
        put("pack", &mut r.packs.iter().map(|p| &p.0));
        put("slope", &mut r.slope.iter());
        put("slope-lo", &mut r.slope_lo.iter());
        put("trees", &mut r.trees.iter());
        put("trees-lo", &mut r.trees_lo.iter());
    }
    t.retain(|_, m| !m.is_empty());
    t
}

impl Input<'_> {
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
/// (each step's time a target, the slots waiting on a tail, the lanes of the forecast for equal
/// machines and what each builds now). `Record::forecast` gives the lead's forecast on any Mac.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record<'a> {
    pub input: Input<'a>,
    pub each_s: BTreeMap<String, f64>,
    pub waiting: BTreeMap<String, String>,
    pub equal: Option<EqualLanes>,
}

/// The lanes of the forecast for equal machines (`pool_lanes`) and what each builds now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqualLanes {
    pub lanes: Vec<Machine>,
    #[serde(with = "pairs")]
    pub running: BTreeMap<(String, String), usize>,
}

impl Record<'_> {
    /// The forecast it holds: the lead's.
    pub fn forecast(&self) -> Forecast {
        let mut f = forecast(&self.input);
        f.each_s = self.each_s.clone();
        f.waiting = self.waiting.clone();
        f.equal =
            self.equal.as_ref().map(|e| Box::new(equal_machines(&self.input, e.lanes.clone(), e.running.clone())));
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
        if let Some(e) = &mut self.equal {
            e.lanes.iter_mut().for_each(m);
        }
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
/// (build::LastCatalog::low_at), the landmark points and the area overlays
/// (build::LastCatalog::since).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cadence {
    pub since_low: Option<u64>,
    pub since_marks: Option<u64>,
    pub since_overlays: Option<u64>,
}

/// The forecast's `why` when the build is done: no work left, nothing to publish and no machine
/// busy (each Mac's agent then trims its caches: crate::agent::room::trim).
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
    /// The forecast for equal machines (docs/pool.md §7.2, §12 4.12; `equal_machines`), in the
    /// pool: reported beside this one, nothing placed or decided by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equal: Option<Box<Forecast>>,
    /// The machines (this Mac's slots) whose job waits on a worker holding its unit's tail, and on
    /// what (the agent's `waiting_on`): their room free for a job beside it meanwhile.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub waiting: BTreeMap<String, String>,
}

impl Forecast {
    /// Whether the build is done: nothing left to build or publish, no machine busy.
    pub fn nothing_left(&self) -> bool {
        self.why.as_deref() == Some(NOTHING_LEFT)
    }
}

/// A run of a machine's work of one step: from and until when (unix seconds), and how many
/// targets.
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
    /// Its targets left by step ("unit", "terrain", "slope", "trees", "trees-lo", "pack").
    pub left: BTreeMap<String, usize>,
    /// When it'll be done, and when it'll be on the map as it's built now: its last unit's end,
    /// then its map tiles drawn, then a catalog within the minute (None: no work left of its own).
    pub ready_at: Option<u64>,
    pub map_at: Option<u64>,
}

/// A cadenced publish: what ("low", "marks", "overlays") and when it may next go out (unix
/// seconds): once its cadence has passed since it last went, or once no unit or terrain is left to
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

/// When a job runs: before the regions, a region's map tiles (once its units are built, ahead of
/// the regions' work), a region's (terrain, units), late (slope and tree cover: the lead's once a
/// region is on the map, and after the last unit), a chain's (the trains' and the landmarks': from
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
/// pass's hiking-route ends; the peaks for every candidate, the pass's summits and the terrain; the
/// items' facts for every candidate; the rest of the heritage chain for the heritage sites; the
/// landmark points for those four; trains a day for their feeds; the 3D buildings' tiles for the
/// normalized files. By step: (the jobs before the
/// regions', the chains', the terrain's).
fn chain_deps(step: &str) -> (&'static [&'static str], &'static [&'static str], bool) {
    match step {
        "pois" => (&["trailends"], &[], false),
        "peaks" => (&["summits"], &["pois"], true),
        "items" => (&[], &["pois"], false),
        "heritage" => (&["heritage-sites"], &[], false),
        "marks" => (&[], &["pois", "peaks", "items", "heritage"], false),
        "rail" => (&[], &["rail-feeds"], false),
        "bldtiles" => (&[], &["bldprep"], false),
        "bldfield-lo" => (&[], &["bldtiles"], false),
        "depth" => (&[], &["depth-lo"], false),
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

/// One run through the work (`run`): each item's machine and times.
struct Sim {
    items: Vec<Item>,
    /// Region → its items.
    region_items: Vec<Vec<usize>>,
    done: bool,
}

/// The share of a region's units reading stale terrain, built as the coverage wants them now, taken
/// to go stale once that terrain is built (`RegionLeft::expected`; which ones is known only then).
/// A round figure, not measured each time: the terrain fix (8 Oct 2026) made most of the north's and
/// every coast's units stale, a little over half of those reading changed terrain, and the forecast
/// had counted none of them until their terrain was built (32 units at 17:00, 149 by 19:00).
pub const EXPECTED_STALE: f64 = 0.6;

/// The jobs before the regions' that the units wait for (build::plan).
const UNITS_NEED: [&str; 3] = ["heritage-sites", "reach", "buildings"];

/// The work as items, each with the items it waits for.
fn items(inp: &Input) -> (Vec<Item>, Vec<Vec<usize>>) {
    let mut out: Vec<Item> = Vec::new();
    let mut by_target: BTreeMap<(String, String), usize> = BTreeMap::new();
    let add = |out: &mut Vec<Item>,
               by: &mut BTreeMap<(String, String), usize>,
               step: &str,
               target: &str,
               cost: Cost,
               phase: Phase,
               deps: Vec<usize>|
     -> usize {
        if let Some(&i) = by.get(&(step.to_string(), target.to_string())) {
            return i;
        }
        let shared = SHARED.contains(&step) && phase != Phase::Before;
        out.push(Item {
            step: step.into(),
            target: target.into(),
            cost,
            phase,
            deps,
            shared,
            running: false,
            by: None,
            from: 0.0,
            end: 0.0,
        });
        by.insert((step.to_string(), target.to_string()), out.len() - 1);
        out.len() - 1
    };
    for (step, target, cost) in &inp.before {
        add(&mut out, &mut by_target, step, target, *cost, Phase::Before, Vec::new());
    }
    let before: Vec<usize> = (0..out.len()).collect();
    // (The units wait for the pass's heritage sites, reaches and roadside buildings, as the plan's
    // do; terrain for nothing.)
    let units_need: Vec<usize> =
        before.iter().copied().filter(|&i| UNITS_NEED.contains(&out[i].step.as_str())).collect();
    // A region at a time: its own terrain (the pieces, then each z3 tile's assembly once its pieces
    // are made), then its own units (each once its region's terrain is built: the terrain it
    // reads); then every region's slope (a piece once the terrain of its z3 tile and its neighbours'
    // is, a z3 tile's assembly once its pieces are) and tree cover.
    let mut region_items: Vec<Vec<usize>> = vec![Vec::new(); inp.regions.len()];
    let item_of = |by: &BTreeMap<(String, String), usize>, step: &str, t: &str| {
        by.get(&(step.to_string(), t.to_string())).copied()
    };
    // (A step's items in z3 tile `q`: its pieces there, or its target that's the z3 tile itself.)
    let of_z3 = |out: &Vec<Item>, step: &str, q: &str| -> Vec<usize> {
        (0..out.len())
            .filter(|&i| {
                out[i].step == step
                    && (out[i].target == q || crate::agent::build::z3_of(&out[i].target).as_deref() == Some(q))
            })
            .collect()
    };
    for r in inp.regions.iter() {
        for t in &r.own_terrain {
            add(&mut out, &mut by_target, "terrain", t, inp.cost("terrain", t), Phase::Region, Vec::new());
        }
        for q in &r.own_terrain_lo {
            let deps = of_z3(&out, "terrain", q);
            add(&mut out, &mut by_target, "terrain-lo", q, inp.cost("terrain-lo", q), Phase::Region, deps);
        }
        // (Its terrain is its own or a region's before it: listed by now.)
        let mut deps: Vec<usize> = r.terrain.iter().filter_map(|t| item_of(&by_target, "terrain", t)).collect();
        deps.extend(r.terrain_lo.iter().filter_map(|q| item_of(&by_target, "terrain-lo", q)));
        deps.extend(&units_need);
        for u in &r.own_units {
            add(&mut out, &mut by_target, "unit", u, inp.cost("unit", u), Phase::Region, deps.clone());
        }
        // Those its stale terrain may make stale: a share, the first in its order (`add` lists a
        // unit two regions expect once).
        let n = (r.expected.len() as f64 * EXPECTED_STALE).round() as usize;
        for u in r.expected.iter().take(n) {
            add(&mut out, &mut by_target, "unit", u, inp.cost("unit", u), Phase::Region, deps.clone());
        }
        // Its map tiles, each once the units that draw it are built (a tile two regions draw, with
        // the first).
        for (t, us) in &r.packs {
            let mut built: Vec<usize> = us.iter().filter_map(|u| item_of(&by_target, "unit", u)).collect();
            built.sort_unstable();
            built.dedup();
            add(&mut out, &mut by_target, "pack", t, inp.cost("pack", t), Phase::Publish, built);
        }
    }
    for r in inp.regions.iter() {
        for t in &r.slope {
            // (The terrain of its z3 tile and its edge neighbours' z3 tiles.)
            let mut deps: Vec<usize> = Vec::new();
            let z3s: Vec<String> = match crate::legacy::TileId::parse(t) {
                Some(u) if u.z == 6 => std::iter::once((u.x, u.y))
                    .chain(crate::agent::build::edge_neighbours(6, u.x, u.y))
                    .map(|(x, y)| format!("3/{}/{}", x >> 3, y >> 3))
                    .collect(),
                _ => vec![t.clone()],
            };
            for q in z3s {
                deps.extend(of_z3(&out, "terrain", &q));
                deps.extend(item_of(&by_target, "terrain-lo", &q));
            }
            deps.sort_unstable();
            deps.dedup();
            add(&mut out, &mut by_target, "slope", t, inp.cost("slope", t), Phase::Late, deps);
        }
        for q in &r.slope_lo {
            let mut deps = of_z3(&out, "slope", q);
            deps.extend(of_z3(&out, "terrain", q));
            deps.extend(item_of(&by_target, "terrain-lo", q));
            add(&mut out, &mut by_target, "slope-lo", q, inp.cost("slope-lo", q), Phase::Late, deps);
        }
        for t in &r.trees {
            add(&mut out, &mut by_target, "trees", t, inp.cost("trees", t), Phase::Late, Vec::new());
        }
        // A z3 tile's tree cover assembled once its pieces are made (a region lists every piece of
        // its z3 tiles: listed by now).
        for q in &r.trees_lo {
            let deps: Vec<usize> = (0..out.len())
                .filter(|&i| out[i].step == "trees" && crate::treepacks::z3_of(&out[i].target).as_ref() == Some(q))
                .collect();
            add(&mut out, &mut by_target, "trees-lo", q, inp.cost("trees-lo", q), Phase::Late, deps);
        }
    }
    // Each region's items: its terrain, all its units (some come with a region before it), its
    // slope and tree cover.
    for (k, r) in inp.regions.iter().enumerate() {
        let mut mine: Vec<usize> = Vec::new();
        for (step, list) in [
            ("terrain", &r.terrain),
            ("terrain-lo", &r.terrain_lo),
            ("unit", &r.units),
            ("unit", &r.expected),
            ("slope", &r.slope),
            ("slope-lo", &r.slope_lo),
            ("trees", &r.trees),
            ("trees-lo", &r.trees_lo),
        ] {
            mine.extend(list.iter().filter_map(|t| by_target.get(&(step.to_string(), t.clone())).copied()));
        }
        mine.extend(r.packs.iter().filter_map(|(t, _)| by_target.get(&("pack".to_string(), t.clone())).copied()));
        region_items[k] = mine;
    }
    // The chains: from the start, each step once what it reads is built.
    let terrain: Vec<usize> =
        (0..out.len()).filter(|&i| out[i].step == "terrain" || out[i].step == "terrain-lo").collect();
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
    // (After all that, not each other.)
    for (step, target, cost) in &inp.idle {
        add(&mut out, &mut by_target, step, target, *cost, Phase::After, all.clone());
    }
    (out, region_items)
}

/// The items ready to take (their deps done, not yet taken), each phase's in order: the measured
/// apart from the cold starts (by step: a cold start's room is its step's alone), and the shared
/// steps' by step for the pages' lane. Each pick's scan then skips what its Mac can't take as a
/// whole.
#[derive(Default)]
struct Ready {
    known: [std::collections::BTreeSet<usize>; 6],
    cold: [BTreeMap<String, std::collections::BTreeSet<usize>>; 6],
    shared: BTreeMap<String, std::collections::BTreeSet<usize>>,
}

impl Ready {
    fn insert(&mut self, i: usize, it: &Item, p: usize) {
        if it.cost.known {
            self.known[p].insert(i);
        } else {
            self.cold[p].entry(it.step.clone()).or_default().insert(i);
        }
        if it.shared {
            self.shared.entry(it.step.clone()).or_default().insert(i);
        }
    }

    fn remove(&mut self, i: usize, it: &Item, p: usize) {
        let drop = |m: &mut BTreeMap<String, std::collections::BTreeSet<usize>>| {
            if let Some(s) = m.get_mut(&it.step) {
                s.remove(&i);
                if s.is_empty() {
                    m.remove(&it.step);
                }
            }
        };
        if it.cost.known {
            self.known[p].remove(&i);
        } else {
            drop(&mut self.cold[p]);
        }
        if it.shared {
            drop(&mut self.shared);
        }
    }
}

/// One run through the work, each item's time `scale`d. Kept to about O(items × lanes): the items
/// under way (`inflight`) held apart, each item's deps not yet done counted down as they end, each
/// phase's items whose deps are done (`ready`) scanned in order (none at all while its Mac's room
/// takes nothing more: a cold start under way, the older app's gap, no core left), the regions'
/// counts kept as items are taken.
fn run(inp: &Input, scale: &dyn Fn(&Cost) -> f64) -> Sim {
    let (mut items, region_items) = items(inp);
    for it in &mut items {
        it.cost.secs = scale(&it.cost).max(1.0);
    }
    let n = inp.machines.len();
    let mut free: Vec<f64> = inp.machines.iter().map(|m| m.busy_s.max(m.from_s).max(0.0)).collect();
    // (A slot busy with a job no item is, until when: it holds its Mac's room meanwhile.)
    let held_until: Vec<f64> = inp.machines.iter().map(|m| m.busy_s.max(0.0)).collect();
    // What's being built now: done when its machine is free.
    let at: BTreeMap<(&str, &str), usize> =
        items.iter().enumerate().map(|(i, it)| ((it.step.as_str(), it.target.as_str()), i)).collect();
    let running: Vec<(usize, usize)> = inp
        .running
        .iter()
        .filter_map(|((step, target), &m)| at.get(&(step.as_str(), target.as_str())).map(|&i| (i, m)))
        .collect();
    drop(at);
    for &(i, m) in &running {
        (items[i].by, items[i].from, items[i].end, items[i].running) =
            (Some(m), 0.0, free.get(m).copied().unwrap_or(0.0), true);
    }
    let mut inflight: Vec<usize> = running.iter().map(|r| r.0).collect();
    let mut unassigned = items.iter().filter(|i| i.by.is_none()).count();
    // Each phase's items in order, and the first not yet taken.
    const PHASES: [Phase; 6] = [Phase::Before, Phase::Publish, Phase::Region, Phase::Late, Phase::Chain, Phase::After];
    let pi = |p: Phase| PHASES.iter().position(|&q| q == p).unwrap_or(0);
    // Each item's deps not yet done, the items waiting on each, and each phase's items ready (their
    // deps done, not yet taken), in order.
    let mut waiting_on: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    let mut pending: Vec<usize> = vec![0; items.len()];
    for (i, it) in items.iter().enumerate() {
        for &d in &it.deps {
            if !(items[d].by.is_some() && items[d].end <= 1e-9) {
                pending[i] += 1;
                waiting_on[d].push(i);
            }
        }
    }
    let mut ready = Ready::default();
    for (i, it) in items.iter().enumerate() {
        if it.by.is_none() && pending[i] == 0 {
            ready.insert(i, it, pi(it.phase));
        }
    }
    // The regions' terrain, units and map tiles: how many not yet taken, and when the last taken
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
    // Each Mac's last start (a job an ask while a member runs an older app).
    let mut last_start: BTreeMap<String, f64> = BTreeMap::new();
    let runnable = |items: &[Item], pending: &[usize], i: usize| items[i].by.is_none() && pending[i] == 0;
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 20 * (items.len() + 10) * n.max(1) || unassigned == 0 {
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
        // (Those ended by now: what waits on them counted down, ready once nothing else is left.)
        let mut ended: Vec<usize> = Vec::new();
        inflight.retain(|&i| {
            let on = items[i].end > t + 1e-9;
            if !on {
                ended.push(i);
            }
            on
        });
        for d in ended {
            for &w in &waiting_on[d] {
                pending[w] -= 1;
                if pending[w] == 0 && items[w].by.is_none() {
                    ready.insert(w, &items[w], pi(items[w].phase));
                }
            }
        }
        let mac = &inp.machines[m];
        // (A machine with nothing it can do waits for the next item to end, or another machine to
        // be free: a job that's no item may free work.)
        let next_end = |items: &[Item], free: &[f64]| {
            let others = free.iter().enumerate().filter(|&(k, &f)| k != m && f > t + 1e-9).map(|(_, &f)| f);
            inflight.iter().map(|&i| items[i].end).chain(others).fold(f64::INFINITY, f64::min)
        };
        // What its Mac runs now (`Snap`), once a pick: each candidate's check then O(1).
        let sn: Option<Snap> = mac.mac.as_deref().map(|g| {
            let room = inp.macs.get(g).copied().unwrap_or_default();
            let share = crate::coord::packed_share(room.cores.max(1));
            let on_mac = |k: usize| inp.machines[k].mac.as_deref() == Some(g);
            let mut sn = Snap {
                room,
                share,
                last_start: last_start.get(g).copied().unwrap_or(f64::NEG_INFINITY),
                ..Default::default()
            };
            let mut running_on = vec![false; n];
            for &i in &inflight {
                let it = &items[i];
                sn.steps.insert(it.step.clone());
                let Some(k) = it.by.filter(|&k| on_mac(k)) else { continue };
                running_on[k] |= it.running;
                sn.jobs += 1;
                sn.mem_mb += it.cost.peak_mb;
                sn.cores += crate::coord::packed_cores_of(&it.step, it.cost.cores, share);
                sn.cold |= !it.cost.known;
            }
            // (A slot busy with a job no item is: its memory (its step's first target's peak), cores.)
            for k in 0..n {
                let mk = &inp.machines[k];
                if k != m && on_mac(k) && held_until[k] > t + 1e-9 && !running_on[k] {
                    sn.jobs += 1;
                    sn.mem_mb += mk.running_mb;
                    sn.cores +=
                        mk.running.as_ref().map_or(share, |(st, _)| crate::coord::packed_cores_of(st, None, share));
                }
            }
            sn
        });
        // Whether item `i` may start beside what the Mac runs, as the coordinator packs
        // (crate::coord's `pack_why`): within its memory and its cores (each job its packed cores,
        // crate::coord::packed_cores_of); a cold start (its time not measured) alone on its Mac,
        // none of its step under way anywhere, and nothing beside one; while a member runs an older
        // app, a job an ask (`OLD_APP_GAP_S`).
        let room_ok = |items: &[Item], i: usize| -> bool {
            let Some(sn) = &sn else { return true };
            let it = &items[i];
            if sn.cold {
                return false;
            }
            if !it.cost.known {
                return sn.jobs == 0 && !sn.steps.contains(&it.step);
            }
            if sn.room.mem_mb > 0 && sn.mem_mb + it.cost.peak_mb > sn.room.mem_mb {
                return false;
            }
            if sn.jobs > 0
                && sn.cores + crate::coord::packed_cores_of(&it.step, it.cost.cores, sn.share) > sn.room.cores.max(1)
            {
                return false;
            }
            !(inp.old_app && t < sn.last_start + OLD_APP_GAP_S - 1e-9)
        };
        // (When the Mac's next job may start, a job an ask, `OLD_APP_GAP_S` after its last.)
        let ask_wake = match &sn {
            Some(sn) if inp.old_app && sn.last_start + OLD_APP_GAP_S > t + 1e-9 => sn.last_start + OLD_APP_GAP_S,
            _ => f64::INFINITY,
        };
        // (Its Mac takes nothing more now, whatever the item: a cold start under way, the older
        // app's gap, or no core left beside its jobs.)
        let shut = sn.as_ref().is_some_and(|sn| {
            sn.cold
                || (inp.old_app && t < sn.last_start + OLD_APP_GAP_S - 1e-9)
                || (sn.jobs > 0 && sn.cores >= sn.room.cores.max(1))
        });
        // The first of phase `p` it may take (`ok`), in order among those ready.
        let scan = |items: &[Item], p: Phase, ok: &dyn Fn(usize) -> bool| -> Option<usize> {
            if shut {
                return None;
            }
            let known = ready.known[pi(p)].iter().copied().find(|&i| ok(i) && room_ok(items, i));
            // (A cold start: only on a Mac running nothing, none of its step under way anywhere;
            // each step's first it may take.)
            let cold = ready.cold[pi(p)]
                .iter()
                .filter(|(step, _)| sn.as_ref().is_none_or(|sn| sn.jobs == 0 && !sn.steps.contains(*step)))
                .filter_map(|(_, set)| set.iter().copied().find(|&i| ok(i) && room_ok(items, i)))
                .min();
            match (known, cold) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        };
        let pick = if mac.helper {
            // The far end of the first shared step it can do (terrain's near end: the lead's
            // next units wait on it, as crate::coord picks).
            SHARED.iter().find_map(|s| {
                let set = ready.shared.get(*s)?;
                let fits = |&i: &usize| items[i].cost.peak_mb <= mac.mem_mb;
                if *s == "terrain" {
                    set.iter().copied().find(fits)
                } else {
                    set.iter().rev().copied().find(fits)
                }
            })
        } else if mac.equal {
            // Any work it can do that fits its Mac's room and that it may run, from the front of the
            // plan's order, as the coordinator offers it (crate::coord's `pick_every`): the pass's,
            // the map tiles, the regions' (in order), slope and tree cover, the chains', then what
            // comes after.
            let may = |i: usize| {
                items[i].cost.peak_mb <= mac.mem_mb
                    && (mac.lead_steps || super::steps::place(&items[i].step) == super::steps::Place::Any)
            };
            PHASES.iter().find_map(|&p| scan(&items, p, &may))
        } else {
            // The lead. While the regions have work left: the first it can do of the pass's; then the
            // map tiles of the units built (publishing comes before the regions' work); then the slope
            // and tree cover of a region on the map whose catalog doesn't record it built yet; then the
            // regions' (in order); with none it can do now, the chains' (listed after them).
            let any = |_: usize| true;
            if region_left > 0 || region_end > t + 1e-9 || before_left > 0 {
                let prep = || {
                    (0..inp.regions.len())
                        .filter(|&k| inp.regions[k].on_map != Some(true) && rp_left[k] == 0 && rp_end[k] <= t + 1e-9)
                        .flat_map(|k| late_of[k].iter().copied())
                        .find(|&i| runnable(&items, &pending, i) && room_ok(&items, i))
                };
                scan(&items, Phase::Before, &any)
                    .or_else(|| scan(&items, Phase::Publish, &any))
                    .or_else(prep)
                    .or_else(|| scan(&items, Phase::Region, &any))
                    .or_else(|| scan(&items, Phase::Chain, &any))
            } else {
                // After the last unit and terrain: the map tiles left, the slope and tree cover, the
                // chains' work, then what comes after.
                [Phase::Publish, Phase::Late, Phase::Chain, Phase::After].iter().find_map(|&p| scan(&items, p, &any))
            }
        };
        let Some(i) = pick else {
            // (A Mac's lane waits for the older app's gap too; the pages' lane for the next end.)
            free[m] = if mac.helper { next_end(&items, &free) } else { next_end(&items, &free).min(ask_wake) };
            continue;
        };
        // Taken: its end at the machine's pace; the counts kept.
        let extra = if items[i].step == "unit" { mac.unit_extra_s } else { 0.0 };
        let end = t + items[i].cost.secs / mac.speed.max(0.01) + extra;
        (items[i].by, items[i].from, items[i].end) = (Some(m), t, end);
        free[m] = end;
        inflight.push(i);
        unassigned -= 1;
        if let Some(g) = &mac.mac {
            last_start.insert(g.clone(), t);
        }
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
    let done = items.iter().all(|i| i.by.is_some());
    Sim { items, region_items, done }
}

/// The forecast for `inp`.
pub fn forecast(inp: &Input) -> Forecast {
    let now = inp.now as f64;
    let expected = run(inp, &|c| c.secs);
    let soon = run(inp, &|c| c.secs * if c.known { 0.95 } else { 0.75 });
    let late = run(inp, &|c| c.secs * if c.known { 1.15 } else { 1.6 });
    // (Done once every machine is free too: a job under way that's no item, a daily one's, ends then.)
    let end_of = |s: &Sim| -> Option<f64> {
        s.done.then(|| s.items.iter().map(|i| i.end).chain(inp.machines.iter().map(|m| m.busy_s)).fold(0.0, f64::max))
    };
    let at = |secs: f64| (now + secs).round() as u64;
    let mut f = Forecast { at: inp.now, ..Default::default() };
    // (Never while a machine is busy: a job under way is work left, items or not.)
    let busy = inp.machines.iter().any(|m| m.busy_s > 0.0) || !inp.running.is_empty();
    let nothing = expected.items.is_empty() && !busy;
    f.why = match (&inp.blind, nothing, expected.done) {
        (Some(b), _, _) => Some(b.clone()),
        (None, true, _) => Some(NOTHING_LEFT.into()),
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
    // Each region: its work left, when it's done, and when it's on the map as it's built now: its
    // units and map tiles done, and a catalog within the minute.
    let on_map_at = |mine: &[usize]| {
        let its: Vec<&Item> = mine
            .iter()
            .map(|&i| &expected.items[i])
            .filter(|i| matches!(i.phase, Phase::Region | Phase::Publish))
            .collect();
        (!its.is_empty() && its.iter().all(|i| i.by.is_some()))
            .then(|| its.iter().map(|i| i.end).fold(0.0, f64::max) + CATALOG_EVERY_S as f64)
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
            // (None with no terrain, unit or map tile of its left, or on the map as it is.)
            map_at: if r.on_map == Some(true) { None } else { on_map_at(mine).map(at) },
        });
    }
    // The cadenced publishes: each once its cadence has passed since it last went, or once no unit
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
        // Its schedule: the job its slot runs now that no item is (its time left), then its runs
        // of a step.
        let mut runs: Vec<(String, f64, f64, usize)> =
            expected.items.iter().filter(|i| i.by == Some(m)).map(|i| (i.step.clone(), i.from, i.end, 1)).collect();
        if let Some((step, n)) =
            mac.running.as_ref().filter(|_| !expected.items.iter().any(|i| i.running && i.by == Some(m)))
        {
            runs.push((step.clone(), 0.0, mac.busy_s.max(0.0), *n));
        }
        runs.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut lane: Vec<Lane> = Vec::new();
        for (step, from, until, n) in runs {
            match lane.last_mut() {
                Some(l) if l.step == step && from <= l.until as f64 - now + 120.0 => {
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

/// The forecast for equal machines (docs/pool.md §7.2, §12 4.12): the same work as `inp`'s, run
/// through on `lanes` (each member's slots, the lead's first slot first: it alone publishes; and
/// the pages as one lane, a helper's: the shared steps alone), what each lane builds
/// now `running`. Reports only: nothing is placed or decided by it.
pub fn equal_machines(inp: &Input, lanes: Vec<Machine>, running: BTreeMap<(String, String), usize>) -> Forecast {
    let lanes: Vec<Machine> = lanes
        .into_iter()
        .enumerate()
        .map(|(k, m)| Machine { equal: k > 0 && !m.helper, lead_steps: true, ..m })
        .collect();
    forecast(&Input {
        now: inp.now,
        before: inp.before.clone(),
        regions: inp.regions.clone(),
        costs: inp.costs.clone(),
        cadence: inp.cadence,
        blind: inp.blind.clone(),
        chains: inp.chains.clone(),
        after: inp.after.clone(),
        idle: inp.idle.clone(),
        machines: lanes,
        running,
        macs: inp.macs.clone(),
        old_app: inp.old_app,
    })
}

/// A worker the lead's coordinator knows, for `pool_lanes`: its name, whether it's a page, the
/// memory it spares (MB: 0, not said).
pub type PoolWorker = (String, bool, u64);

/// A Mac's lanes: the slots it may run now (`slots`: the lead's ramp's, a member's
/// crate::agent::job_slots), never fewer than its last running slot's number (`running`: the slots
/// running a job, 0 the first; they may be sparse).
pub fn slot_lanes(slots: usize, running: &[usize]) -> usize {
    let top = running.iter().map(|k| k + 1).max().unwrap_or(0);
    slots.max(top).max(1)
}

/// The memory a lane is taken to have when its worker says none (MB), as a helper's.
pub const LANE_MEM_MB: u64 = 6144;

/// The lanes of the forecast for equal machines (`equal_machines`): the lead's slots (`lead`, then
/// `others`, its other slots' lanes), each other native worker a lane, the pages one lane (their measured speeds summed,
/// the largest memory one said), each at its speed in `speeds` (one not in it: the lead's, 1) and
/// busy with its leases' targets left (`leased`: worker, step, targets, seconds since granted; a
/// page's, the pages' lane's); and the leased targets as running on their lanes.
pub fn pool_lanes(
    lead: Machine,
    others: Vec<Machine>,
    workers: &[PoolWorker],
    speeds: &BTreeMap<String, f64>,
    leased: &[(String, String, Vec<String>, u64)],
    cost: &dyn Fn(&str, &str) -> Cost,
) -> (Vec<Machine>, BTreeMap<(String, String), usize>) {
    let speed_of = |n: &str| speeds.get(n).copied().map_or((1.0, false), |s| (s, true));
    let left = |names: &[&str], speed: f64| {
        leased
            .iter()
            .filter(|l| names.contains(&l.0.as_str()))
            .map(|(_, step, ts, age)| ts.iter().map(|t| cost(step, t).secs).sum::<f64>() / speed.max(0.1) - *age as f64)
            .fold(0.0, f64::max)
    };
    let lane = |name: &str, speed: f64, measured: bool, helper: bool, mem_mb: u64, busy_s: f64| Machine {
        name: name.to_string(),
        speed,
        measured,
        helper,
        equal: false,
        mem_mb,
        busy_s,
        unit_extra_s: 0.0,
        ..Default::default()
    };
    let own: Vec<String> = std::iter::once(lead.name.clone()).chain(others.iter().map(|m| m.name.clone())).collect();
    let mut lanes: Vec<Machine> = std::iter::once(lead).chain(others).collect();
    let (mut speed, mut measured, mut mem, mut pages): (f64, bool, u64, Vec<&str>) = (0.0, true, 0, Vec::new());
    for (n, web, mem_mb) in workers {
        if own.contains(n) {
            continue;
        }
        let (sp, m) = speed_of(n);
        if *web {
            (speed, measured, mem) = (speed + sp, measured && m, mem.max(*mem_mb));
            pages.push(n);
        } else {
            lanes.push(lane(n, sp, m, false, if *mem_mb > 0 { *mem_mb } else { LANE_MEM_MB }, left(&[n.as_str()], sp)));
        }
    }
    if !pages.is_empty() {
        lanes.push(lane("pages", speed, measured, true, if mem > 0 { mem } else { LANE_MEM_MB }, left(&pages, speed)));
    }
    let mut running: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (worker, step, targets, _) in leased {
        let name = if pages.contains(&worker.as_str()) { "pages" } else { worker.as_str() };
        if let Some(m) = lanes.iter().position(|l| l.name == name) {
            running.extend(targets.iter().map(|t| ((step.clone(), t.clone()), m)));
        }
    }
    (lanes, running)
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
    /// This thread's CPU time (ms): a timing test's measure, not wall time, so a loaded Mac (tests
    /// in parallel) doesn't fail it; its bound catches a complexity regression (an O(n³) run took
    /// 16 s), not jitter.
    fn thread_cpu_ms() -> u128 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a valid timespec for the call to fill.
        assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) }, 0);
        ts.tv_sec as u128 * 1000 + ts.tv_nsec as u128 / 1_000_000
    }

    /// The CPU time (ms) a timing test's forecast may take (well under a second unloaded).
    const CPU_BOUND_MS: u128 = 3000;

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

    /// A map tile and the units that draw it.
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

    fn cost(step: &str, _t: &str) -> Cost {
        let secs = match step {
            "terrain" => 600.0,
            "unit" => 300.0,
            "slope" => 200.0,
            _ => 100.0,
        };
        Cost { secs, known: step != "slope", peak_mb: 1000, cores: None }
    }

    fn input<'a>(regions: &'a [RegionLeft], machines: Vec<Machine>, c: &'a dyn Fn(&str, &str) -> Cost) -> Input<'a> {
        Input {
            now: 1_000_000,
            before: Vec::new(),
            regions: Cow::Borrowed(regions),
            costs: cost_table(regions, c),
            cadence: Cadence::default(),
            blind: None,
            chains: Vec::new(),
            after: vec![("marks".into(), "marks".into(), Cost { secs: 300.0, known: true, peak_mb: 0, cores: None })],
            idle: Vec::new(),
            machines,
            running: BTreeMap::new(),
            macs: BTreeMap::new(),
            old_app: false,
        }
    }

    #[test]
    fn equal_machines_share_every_step() {
        // Two regions; the work before them (a step no helper takes); the lead's slot, a member's
        // slot as fast, and the pages as one lane.
        let regions =
            [region("a", &["6/1/1", "6/1/2"], &["u1", "u2"], &["6/1/1"]), region("b", &["6/2/1"], &["u3", "u4"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        inp.before = vec![
            ("reach".into(), "reach".into(), Cost { secs: 900.0, known: true, peak_mb: 0, cores: None }),
            (
                "heritage-sites".into(),
                "heritage-sites".into(),
                Cost { secs: 900.0, known: true, peak_mb: 0, cores: None },
            ),
        ];
        let lanes =
            vec![mac("m4", 1.0, false), mac("m1", 1.0, false), Machine { mem_mb: 4096, ..mac("pages", 2.0, true) }];
        let f = equal_machines(&inp, lanes, BTreeMap::new());
        assert!(f.why.is_none() && f.done_at.is_some(), "{:?}", f.why);
        assert!(f.equal.is_none() && forecast(&inp).equal.is_none(), "reports only: no forecast nested");
        let steps = |lane: &str| f.lanes[lane].iter().map(|l| l.step.clone()).collect::<BTreeSet<_>>();
        // The member's slot takes what a helper never does: the work before the regions, units.
        // (Both free at once: the lead's slot takes the first, the member's the second, which no
        // helper would.)
        assert!(
            steps("m4").contains("reach") && steps("m1").contains("heritage-sites"),
            "{:?} {:?}",
            steps("m4"),
            steps("m1")
        );
        assert!(steps("m1").contains("unit"), "{:?}", steps("m1"));
        // The pages' only shared steps.
        assert!(steps("pages").iter().all(|s| SHARED.contains(&s.as_str())), "{:?}", steps("pages"));
        // Every lane reported, at its speed; the same input the same forecast.
        assert_eq!(f.speed.keys().cloned().collect::<Vec<_>>(), vec!["m1", "m4", "pages"]);
        assert_eq!(
            f,
            equal_machines(
                &inp,
                vec![mac("m4", 1.0, false), mac("m1", 1.0, false), Machine { mem_mb: 4096, ..mac("pages", 2.0, true) }],
                BTreeMap::new()
            )
        );
        // More lanes finish no later than the lead's slot alone.
        let alone = equal_machines(&inp, vec![mac("m4", 1.0, false)], BTreeMap::new());
        assert!(f.done_at.unwrap() <= alone.done_at.unwrap());
    }

    #[test]
    fn a_record_read_back_gives_the_same_forecast_compressed_or_not() {
        // (Task #409) its input, a target running, the forecast for equal machines beside it.
        let regions =
            [region("a", &["6/1/1", "6/1/2"], &["u1", "u2"], &["6/1/1"]), region("b", &["6/2/1"], &["u3", "u4"], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        inp.running.insert(("unit".into(), "u1".into()), 0);
        let equal = EqualLanes {
            lanes: vec![mac("m4", 1.0, false), mac("m1", 1.0, false)],
            running: [(("unit".to_string(), "u1".to_string()), 0)].into(),
        };
        let mut lead = forecast(&inp);
        lead.each_s.insert("unit".into(), 300.0);
        lead.equal = Some(Box::new(equal_machines(&inp, equal.lanes.clone(), equal.running.clone())));
        let rec = Record { input: inp, each_s: lead.each_s.clone(), waiting: BTreeMap::new(), equal: Some(equal) };
        let b = rec.bytes().unwrap();
        assert_eq!(b[0], b'{', "small: plain JSON");
        assert_eq!(Record::from_bytes(&b).unwrap().forecast(), lead);
        let z = zstd::encode_all(&b[..], 3).unwrap();
        assert_eq!(Record::from_bytes(&z).unwrap(), rec);
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
    fn a_pages_lease_is_the_pages_lanes_running_work_and_its_time_left() {
        let workers: Vec<PoolWorker> = vec![
            ("m1".into(), false, 0),
            ("page a".into(), true, 0),
            ("page b".into(), true, 0),
            ("m4".into(), false, 9),
        ];
        let speeds: BTreeMap<String, f64> = [("page a".to_string(), 0.5)].into();
        let leased =
            vec![("page b".to_string(), "trees".to_string(), vec!["6/1/1".to_string(), "6/1/2".to_string()], 50)];
        let (lanes, running) = pool_lanes(mac("m4", 1.0, false), Vec::new(), &workers, &speeds, &leased, &cost);
        assert_eq!(lanes.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["m4", "m1", "pages"]);
        let pages = &lanes[2];
        // (Speeds summed: one measured, one the lead's; no memory said, a helper's.)
        assert!(pages.helper && (pages.speed - 1.5).abs() < 1e-9 && !pages.measured && pages.mem_mb == LANE_MEM_MB);
        assert_eq!(lanes[1].mem_mb, LANE_MEM_MB);
        // Two trees targets of 100 s at 1.5, less the 50 s since.
        assert!((pages.busy_s - (200.0 / 1.5 - 50.0)).abs() < 1e-9, "{}", pages.busy_s);
        assert_eq!(running.get(&("trees".to_string(), "6/1/2".to_string())), Some(&2));
        // A page saying its memory: the largest said.
        let workers: Vec<PoolWorker> = vec![("page a".into(), true, 2048), ("page b".into(), true, 4096)];
        assert_eq!(pool_lanes(mac("m4", 1.0, false), Vec::new(), &workers, &speeds, &[], &cost).0[1].mem_mb, 4096);
    }

    #[test]
    fn an_equal_lane_takes_only_what_fits_its_memory_and_its_running_targets_are_its_own() {
        let regions = [region("a", &[], &["u1", "u2"], &[])];
        let big = |step: &str, t: &str| Cost { peak_mb: if t == "u2" { 9000 } else { 1000 }, ..cost(step, t) };
        let inp = input(&regions, vec![mac("m4", 1.0, false)], &big);
        let mut running = BTreeMap::new();
        running.insert(("unit".to_string(), "u1".to_string()), 1);
        let small = Machine { busy_s: 300.0, ..mac("m1", 1.0, false) };
        let f = equal_machines(&inp, vec![Machine { mem_mb: u64::MAX, ..mac("m4", 1.0, false) }, small], running);
        assert!(f.lanes["m1"].iter().all(|l| l.step != "unit" || l.n == 1), "{:?}", f.lanes["m1"]);
        assert!(
            f.next["m1"].iter().all(|n| n.step != "unit"),
            "u1 running there, u2 too big for it: {:?}",
            f.next["m1"]
        );
        assert!(f.next["m4"].iter().any(|n| n.targets.contains(&"u2".to_string())));
    }

    #[test]
    fn a_tree_cover_assembly_after_its_pieces_and_mids_in_idle_time_last() {
        // Region a: three tree cover pieces of z3 tile 3/3/2 and its assembly; a helper beside the
        // lead; and a piece's mid made in idle time.
        let mut a = region("a", &[], &[], &[]);
        a.trees = vec!["6/28/16".into(), "6/28/17".into(), "6/29/16".into()];
        a.trees_lo = vec!["3/3/2".into()];
        let regions = [a];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        inp.idle =
            vec![("trees".into(), "6/40/20".into(), Cost { secs: 100.0, known: false, peak_mb: 1000, cores: None })];
        let sim = run(&inp, &|c| c.secs);
        let item = |step: &str, t: &str| sim.items.iter().find(|i| i.step == step && i.target == t).unwrap();
        // The assembly once its pieces are made (the member takes some), by the lead alone.
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
        assert!(!f.nothing_left(), "{:?}", f.why);
        assert_eq!(f.done_at, Some(1_000_000 + 3600));
        // As the agent has it now, a running item of its own: on the schedule, and its step's.
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        busy(&mut inp);
        inp.before = vec![(
            "labels".into(),
            "labels 2026-09-28".into(),
            Cost { secs: 2200.0, known: true, peak_mb: 0, cores: None },
        )];
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
        assert!(forecast(&inp).nothing_left());
    }

    #[test]
    fn a_page_around_takes_each_unit_the_moment_its_job_gives_it() {
        // As below, a worker that takes tails around: each of the two units 30 s longer.
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
        // Terrain 600, two units 600, each unit's map tile (100 s, measured per tile) drawn once
        // that unit is built, before the next unit (each tile waits for its own units alone): on
        // the map a catalog's minute after the last. Then its slope (200), then the landmarks (300).
        assert_eq!(f.regions[0].map_at, Some(1_000_000 + 1200 + 200 + 60));
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 1600));
        assert_eq!(f.done_at, Some(1_000_000 + 1900));
        let steps: Vec<(&str, usize, u64)> = f.steps.iter().map(|s| (s.step.as_str(), s.left, s.work_s)).collect();
        assert_eq!(
            steps,
            [("terrain", 1, 600), ("unit", 2, 600), ("pack", 2, 200), ("slope", 1, 200), ("marks", 1, 300)]
        );
        assert_eq!(f.regions[0].left.get("pack"), Some(&2));
        // The slope was guessed: its share of the time isn't measured.
        assert!((f.measured - 1700.0 / 1900.0).abs() < 0.01);
        let r = f.range.unwrap();
        assert!(r[0] < f.done_at.unwrap() && f.done_at.unwrap() < r[1]);
        assert_eq!(
            f.next["m4"].iter().map(|n| (n.step.as_str(), n.targets.len())).collect::<Vec<_>>(),
            [("terrain", 1), ("unit", 1), ("pack", 1)]
        );
        // No lane of publishing's own: the map tiles are a step's run like any other.
        let lane: Vec<(&str, u64, u64, usize)> =
            f.lanes["m4"].iter().map(|l| (l.step.as_str(), l.from - 1_000_000, l.until - 1_000_000, l.n)).collect();
        assert_eq!(
            lane,
            [
                ("terrain", 0, 600, 1),
                ("unit", 600, 900, 1),
                ("pack", 900, 1000, 1),
                ("unit", 1000, 1300, 1),
                ("pack", 1300, 1400, 1),
                ("slope", 1400, 1600, 1),
                ("marks", 1600, 1900, 1)
            ]
        );
    }

    #[test]
    fn a_share_of_the_units_reading_stale_terrain_is_counted_as_work_to_come() {
        // One unit stale, five more reading the stale terrain: three of them (0.6) counted too.
        let expected = ["6/9/8", "6/9/9", "6/10/8", "6/10/9", "6/11/8"].map(String::from).to_vec();
        let regions = [RegionLeft { expected, ..region("a", &["3/1/1"], &["6/8/8"], &[]) }];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        let unit = f.steps.iter().find(|s| s.step == "unit").unwrap();
        assert_eq!((unit.left, unit.work_s), (4, 1200));
        // The region's ready once they're built too: terrain 600, four units 1200.
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
        // The M1 takes b's terrain (the far end) at half the pace (1,200 s), then b's unit, its
        // terrain built (600 s at its pace); the lead a's terrain and unit (900 s), its two map
        // tiles (200 s), then its slope (200 s), on the map by then. Each region on the map a
        // catalog's minute after its last unit and map tiles.
        assert_eq!(
            f.next["m1"].iter().map(|n| (n.step.as_str(), n.targets[0].as_str())).collect::<Vec<_>>(),
            [("terrain", "3/2/2"), ("unit", "6/16/16")]
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
        // The lead takes a's terrain; the M1 b's (whose units the lead builds next), not c's.
        assert_eq!(f.next["m1"].first().map(|n| (n.step.as_str(), n.targets[0].as_str())), Some(("terrain", "3/2/2")));
    }

    #[test]
    fn a_regions_map_tiles_come_before_the_next_regions_work() {
        // a, b and c (a unit of 300 s and a map tile of 100 s each): each region's tile drawn once
        // its unit is built, before the next region's unit; each on the map a minute after.
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
        // (Task #259 review) its units and map tiles done: on the map as it's built now already,
        // not "in a minute" each time the forecast is made; nor one on the map as it is.
        let regions =
            [region("a", &[], &[], &["3/1/1"]), RegionLeft { on_map: Some(true), ..region("b", &[], &["6/2/2"], &[]) }];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        assert_eq!((f.regions[0].map_at, f.regions[1].map_at), (None, None));
        assert!(f.regions[0].ready_at.is_some());
    }

    #[test]
    fn the_cadenced_publishes_go_by_their_cadence_or_once_nothing_is_left_to_build() {
        // Thirty units (9,000 s) left. The low zooms last made an hour ago (due in two more), the
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
        // Two units left (600 s): each once they're built, if sooner.
        let regions = [region("a", &[], &refs[..2], &[])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false)], &cost);
        inp.cadence = Cadence { since_low: Some(3600), since_marks: Some(600), since_overlays: Some(0) };
        assert_eq!(at(&forecast(&inp)), [("low".into(), 600), ("marks".into(), 600), ("overlays".into(), 600)]);
    }

    #[test]
    fn regions_are_built_one_at_a_time() {
        let regions = [region("a", &["3/1/1"], &["6/8/8"], &[]), region("b", &["3/2/2"], &["6/16/16"], &[])];
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false)], &cost));
        // a's terrain and unit, then b's terrain and unit: a done first.
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 900));
        assert_eq!(f.regions[1].ready_at, Some(1_000_000 + 900 + 900));
        assert_eq!(f.measured, 1.0);
    }

    #[test]
    fn a_region_whose_terrain_a_helper_builds_waits_for_it() {
        // One region: the M1 has its terrain now (900 s left); the lead waits for it.
        let regions = [region("a", &["3/1/1"], &["6/8/8"], &[])];
        let mut inp =
            input(&regions, vec![mac("m4", 1.0, false), Machine { busy_s: 900.0, ..mac("m1", 0.5, true) }], &cost);
        inp.running.insert(("terrain".into(), "3/1/1".into()), 1);
        let f = forecast(&inp);
        assert_eq!(f.regions[0].ready_at, Some(1_000_000 + 900 + 300));
    }

    #[test]
    fn a_helper_isnt_stranded_by_a_worldwide_job() {
        // The lead runs a worldwide job (no item) for an hour; the member takes the terrain at
        // once (terrain waits for nothing), then the units once their region's terrain is built
        // and the job the units need is done: it wakes when the lead is free.
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &[])];
        let mut inp =
            input(&regions, vec![Machine { busy_s: 0.0, ..mac("m4", 1.0, false) }, mac("m1", 1.0, true)], &cost);
        inp.before =
            vec![("reach".into(), "reach".into(), Cost { secs: 3600.0, known: true, peak_mb: 0, cores: None })];
        let f = forecast(&inp);
        assert_eq!(f.next["m1"][0].step, "terrain");
        assert_eq!(f.next["m1"][0].from, 1_000_000);
        // Both units after the hour: one each.
        let lanes: Vec<&str> = f.lanes["m1"].iter().map(|l| l.step.as_str()).collect();
        assert_eq!(lanes, ["terrain", "unit"]);
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
        // its unit; one the map has as it is, rebuilt units or none left, no time.
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
        // (Task #383) the lead's two slots and a member's two: the member's take units (no helper's
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
        // (Task #384) forty units: one Mac's two slots, then two Macs' two each, all as fast.
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
        // from the front, as the coordinator offers it (pick_every), once the units ahead of it are
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
        inp.chains =
            vec![("pois".into(), "6/8/0".into(), Cost { secs: 100.0, known: true, peak_mb: 1000, cores: None })];
        let f = forecast(&inp);
        let first = &f.lanes["m4 #2"][0];
        assert_eq!((first.step.as_str(), first.from, first.until, first.n), ("depth", 1_000_000, 1_002_400, 3));
        let pois = f.lanes.values().flatten().find(|l| l.step == "pois").unwrap();
        assert_eq!(pois.from, 1_000_900, "after the three units each lane took first: {:?}", f.lanes);
    }

    #[test]
    fn a_macs_slots_share_its_room_as_the_coordinator_packs() {
        // (Task #383 review) a member of 8 cores (jobs packed at 2 each) and 8 GB, four slots;
        // units of 3 GB: two at once by memory, not four.
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
        let sim = run(&inp, &|c| c.secs);
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
        assert_eq!(most(&run(&inp, &|c| c.secs)), 4);
        inp.macs.insert("m1".into(), MacRoom { mem_mb: 8000, cores: 4 });
        assert_eq!(most(&run(&inp, &|c| c.secs)), 2);
        // A step not measured runs alone on its Mac, none of it elsewhere.
        let cold = |step: &str, t: &str| Cost { known: false, ..small(step, t) };
        inp.costs = cost_table(&regions, &cold);
        assert_eq!(most(&run(&inp, &|c| c.secs)), 1);
        // A member on an older app: a job an ask, the Mac's starts 30 s apart at least.
        inp.costs = cost_table(&regions, &small);
        inp.old_app = true;
        let sim = run(&inp, &|c| c.secs);
        let mut starts: Vec<f64> = sim.items.iter().filter(|i| i.by.is_some_and(|m| m > 0)).map(|i| i.from).collect();
        starts.sort_by(f64::total_cmp);
        assert!(starts.windows(2).all(|w| w[1] - w[0] >= OLD_APP_GAP_S - 1e-9), "{starts:?}");
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
        // (Task #384 review) 3,000 units in 30 regions, each unit's map tile, two Macs of four
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
        let took = thread_cpu_ms();
        let f = forecast(&inp);
        let ms = thread_cpu_ms() - took;
        eprintln!("the forecast took {ms} ms (3,000 units, 3,000 map tiles, 8 lanes)");
        assert!(f.done_at.is_some() && f.regions.iter().all(|r| r.map_at.is_some()));
        assert!(ms < CPU_BOUND_MS, "{ms} ms of CPU");
    }

    /// The run as first written (task #384): every pick scans every item, each candidate's room
    /// worked out from all the items. The reference `run` must match item for item.
    fn run_reference(inp: &Input, scale: &dyn Fn(&Cost) -> f64) -> Sim {
        let (mut items, region_items) = items(inp);
        for it in &mut items {
            it.cost.secs = scale(&it.cost).max(1.0);
        }
        let n = inp.machines.len();
        let mut free: Vec<f64> = inp.machines.iter().map(|m| m.busy_s.max(m.from_s).max(0.0)).collect();
        let held_until: Vec<f64> = inp.machines.iter().map(|m| m.busy_s.max(0.0)).collect();
        for ((step, target), &m) in &inp.running {
            if let Some(it) = items.iter_mut().find(|i| &i.step == step && &i.target == target) {
                (it.by, it.from, it.end, it.running) = (Some(m), 0.0, free.get(m).copied().unwrap_or(0.0), true);
            }
        }
        let regionals: Vec<usize> = (0..items.len()).filter(|&i| items[i].phase == Phase::Region).collect();
        let next_end = |items: &[Item], free: &[f64], m: usize, t: f64| {
            let others = free.iter().enumerate().filter(|&(k, &f)| k != m && f > t + 1e-9).map(|(_, &f)| f);
            items
                .iter()
                .filter(|i| i.by.is_some() && i.end > t + 1e-9)
                .map(|i| i.end)
                .chain(others)
                .fold(f64::INFINITY, f64::min)
        };
        let done_by = |items: &[Item], i: usize, t: f64| items[i].by.is_some() && items[i].end <= t + 1e-9;
        let runnable = |items: &[Item], i: usize, t: f64| {
            items[i].by.is_none() && items[i].deps.iter().all(|&d| done_by(items, d, t))
        };
        let last_of = |items: &[Item], g: &str, t: f64| {
            items
                .iter()
                .filter(|it| {
                    it.by.is_some_and(|k| inp.machines[k].mac.as_deref() == Some(g))
                        && !it.running
                        && it.from <= t + 1e-9
                })
                .map(|it| it.from)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        let room_ok = |items: &[Item], m: usize, i: usize, t: f64| -> bool {
            let Some(g) = inp.machines[m].mac.as_deref() else { return true };
            let room = inp.macs.get(g).copied().unwrap_or_default();
            let share = crate::coord::packed_share(room.cores.max(1));
            let on_mac = |k: usize| inp.machines[k].mac.as_deref() == Some(g);
            let now_on = |it: &Item| it.by.is_some_and(on_mac) && it.from <= t + 1e-9 && it.end > t + 1e-9;
            let on: Vec<&Item> = items.iter().filter(|it| now_on(it)).collect();
            let busy: Vec<usize> = (0..n)
                .filter(|&k| {
                    k != m
                        && on_mac(k)
                        && held_until[k] > t + 1e-9
                        && !items.iter().any(|it| it.by == Some(k) && it.running && it.end > t + 1e-9)
                })
                .collect();
            let jobs = on.len() + busy.len();
            if on.iter().any(|it| !it.cost.known) {
                return false;
            }
            if !items[i].cost.known {
                let step_on = items
                    .iter()
                    .any(|it| it.step == items[i].step && it.by.is_some() && it.from <= t + 1e-9 && it.end > t + 1e-9);
                return jobs == 0 && !step_on;
            }
            let mem: u64 = on.iter().map(|it| it.cost.peak_mb).sum::<u64>()
                + busy.iter().map(|&k| inp.machines[k].running_mb).sum::<u64>();
            if room.mem_mb > 0 && mem + items[i].cost.peak_mb > room.mem_mb {
                return false;
            }
            let cores: u32 =
                on.iter().map(|it| crate::coord::packed_cores_of(&it.step, it.cost.cores, share)).sum::<u32>()
                    + busy
                        .iter()
                        .map(|&k| {
                            inp.machines[k]
                                .running
                                .as_ref()
                                .map_or(share, |(st, _)| crate::coord::packed_cores_of(st, None, share))
                        })
                        .sum::<u32>();
            if jobs > 0
                && cores + crate::coord::packed_cores_of(&items[i].step, items[i].cost.cores, share) > room.cores.max(1)
            {
                return false;
            }
            !(inp.old_app && t < last_of(items, g, t) + OLD_APP_GAP_S - 1e-9)
        };
        let ask_wake = |items: &[Item], m: usize, t: f64| -> f64 {
            let Some(g) = inp.machines[m].mac.as_deref().filter(|_| inp.old_app) else { return f64::INFINITY };
            let last = last_of(items, g, t);
            if last + OLD_APP_GAP_S > t + 1e-9 {
                last + OLD_APP_GAP_S
            } else {
                f64::INFINITY
            }
        };
        let first = |items: &[Item], m: usize, p: Phase, t: f64| {
            (0..items.len()).find(|&i| items[i].phase == p && runnable(items, i, t) && room_ok(items, m, i, t))
        };
        let mut guard = 0usize;
        loop {
            guard += 1;
            if guard > 20 * (items.len() + 10) * n.max(1) || items.iter().all(|i| i.by.is_some()) {
                break;
            }
            let (m, t) =
                free.iter()
                    .copied()
                    .enumerate()
                    .fold((0, f64::INFINITY), |a, (i, f)| if f < a.1 - 1e-9 { (i, f) } else { a });
            if !t.is_finite() {
                break;
            }
            let mac = &inp.machines[m];
            let pick = if mac.helper {
                SHARED.iter().find_map(|s| {
                    let fits = |&i: &usize| {
                        items[i].shared
                            && items[i].step == *s
                            && items[i].cost.peak_mb <= mac.mem_mb
                            && runnable(&items, i, t)
                    };
                    if *s == "terrain" {
                        (0..items.len()).find(fits)
                    } else {
                        (0..items.len()).rev().find(fits)
                    }
                })
            } else if mac.equal {
                let may =
                    |step: &str| mac.lead_steps || super::super::steps::place(step) == super::super::steps::Place::Any;
                let fits = |i: usize| {
                    items[i].cost.peak_mb <= mac.mem_mb
                        && may(&items[i].step)
                        && runnable(&items, i, t)
                        && room_ok(&items, m, i, t)
                };
                [Phase::Before, Phase::Publish, Phase::Region, Phase::Late, Phase::Chain, Phase::After]
                    .iter()
                    .find_map(|&p| (0..items.len()).find(|&i| items[i].phase == p && fits(i)))
            } else {
                let regional_left = regionals.iter().any(|&i| !done_by(&items, i, t));
                if regional_left || items.iter().any(|i| i.phase == Phase::Before && i.by.is_none()) {
                    let prep = || {
                        (0..inp.regions.len())
                            .filter(|&k| inp.regions[k].on_map != Some(true))
                            .filter(|&k| {
                                region_items[k]
                                    .iter()
                                    .filter(|&&i| matches!(items[i].phase, Phase::Region | Phase::Publish))
                                    .all(|&i| done_by(&items, i, t))
                            })
                            .flat_map(|k| region_items[k].iter().copied())
                            .find(|&i| {
                                items[i].phase == Phase::Late && runnable(&items, i, t) && room_ok(&items, m, i, t)
                            })
                    };
                    first(&items, m, Phase::Before, t)
                        .or_else(|| first(&items, m, Phase::Publish, t))
                        .or_else(prep)
                        .or_else(|| first(&items, m, Phase::Region, t))
                        .or_else(|| first(&items, m, Phase::Chain, t))
                } else {
                    [Phase::Publish, Phase::Late, Phase::Chain, Phase::After]
                        .iter()
                        .find_map(|&p| first(&items, m, p, t))
                }
            };
            free[m] = match pick {
                Some(i) => {
                    let extra = if items[i].step == "unit" { mac.unit_extra_s } else { 0.0 };
                    let end = t + items[i].cost.secs / mac.speed.max(0.01) + extra;
                    (items[i].by, items[i].from, items[i].end) = (Some(m), t, end);
                    end
                }
                None if mac.helper => next_end(&items, &free, m, t),
                None => next_end(&items, &free, m, t).min(ask_wake(&items, m, t)),
            };
        }
        let done = items.iter().all(|i| i.by.is_some());
        Sim { items, region_items, done }
    }

    /// Inputs for the run's tests at scale: `units` units in regions of 100, each its map tile, two
    /// Macs of four slots packed by room; each unit's cost by `cost_of(k)`.
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

    fn two_macs<'a>(regions: &'a [RegionLeft], c: &'a dyn Fn(&str, &str) -> Cost, old: bool) -> Input<'a> {
        let slot = |host: &str, k: usize, lead: bool| Machine {
            equal: k > 0 || !lead,
            lead_steps: lead,
            mac: Some(host.into()),
            mem_mb: 16_000,
            ..mac(&super::super::slot_worker(host, k), 1.0, false)
        };
        let mut machines: Vec<Machine> =
            (0..4).map(|k| slot("m4", k, true)).chain((0..4).map(|k| slot("m1", k, false))).collect();
        // (A member slot busy with a job no item is, holding 2 GB for 20 min.)
        machines[5].busy_s = 1200.0;
        machines[5].running = Some(("depth".into(), 1));
        machines[5].running_mb = 2000;
        machines.push(Machine { mem_mb: 4000, ..mac("pages", 1.0, true) });
        let mut inp = input(regions, machines, c);
        inp.macs = [
            ("m4".to_string(), MacRoom { mem_mb: 16_000, cores: 14 }),
            ("m1".to_string(), MacRoom { mem_mb: 8_000, cores: 8 }),
        ]
        .into();
        inp.old_app = old;
        inp
    }

    /// Unit `t`'s kind for the tests at scale: every third cold, every fifth big (6 GB), the rest
    /// measured and small; `all_cold`: every one cold.
    fn mixed(all_cold: bool) -> impl Fn(&str, &str) -> Cost {
        move |step: &str, t: &str| {
            let k: usize = t.rsplit('/').next().and_then(|x| x.parse().ok()).unwrap_or(0);
            let c = cost(step, t);
            if step != "unit" {
                return c;
            }
            Cost {
                known: !(all_cold || k % 3 == 0),
                peak_mb: if k % 5 == 0 { 6000 } else { 1500 },
                cores: Some(if k % 2 == 0 { 2 } else { 1 }),
                ..c
            }
        }
    }

    #[test]
    fn the_run_matches_a_full_scan_item_for_item() {
        // (Task #384 review) cold, big and older-app inputs: the run's picks the same as scanning
        // every item each pick, each item's machine, start and end.
        let regions = many(150);
        for (all_cold, old) in [(false, false), (true, false), (false, true), (true, true)] {
            let c = mixed(all_cold);
            let inp = two_macs(&regions, &c, old);
            let (a, b) = (run(&inp, &|c| c.secs), run_reference(&inp, &|c| c.secs));
            assert_eq!(a.done, b.done);
            for (x, y) in a.items.iter().zip(&b.items) {
                assert_eq!(
                    (x.by, x.from, x.end),
                    (y.by, y.from, y.end),
                    "{} {} (cold {all_cold}, old {old})",
                    x.step,
                    x.target
                );
            }
        }
    }

    #[test]
    fn three_thousand_cold_or_big_units_forecast_in_well_under_a_second() {
        // (Task #384 review) the worst cases: every unit cold (each alone on its Mac), and cold and
        // big mixed.
        let regions = many(3000);
        for all_cold in [true, false] {
            let c = mixed(all_cold);
            let inp = two_macs(&regions, &c, false);
            let took = thread_cpu_ms();
            let f = forecast(&inp);
            let ms = thread_cpu_ms() - took;
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
        let c = |secs: f64| Cost { secs, known: true, peak_mb: 1000, cores: None };
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
        // Half the units, whoever's at the Mac (the owner's choice: no rule for a Mac in use).
        let f = forecast(&input(&regions, vec![mac("m4", 1.0, false), others.clone()], &cost));
        assert_eq!(
            f.lanes.get("m4 #2").map_or(0, |l| l.iter().filter(|x| x.step == "unit").map(|x| x.n).sum::<usize>()),
            4
        );
        // The lead's first job two hours from done: its other slots take the units from now.
        let f = forecast(&input(&regions, vec![Machine { busy_s: 7200.0, ..mac("m4", 1.0, false) }, others], &cost));
        let lane = &f.lanes["m4 #2"];
        assert_eq!(
            (lane[0].step.as_str(), lane[0].from, lane.iter().filter(|x| x.step == "unit").map(|x| x.n).sum::<usize>()),
            ("unit", 1_000_000, 8)
        );
    }

    #[test]
    fn a_helper_takes_the_candidates_while_the_units_wait() {
        let regions = [region("a", &["3/1/1"], &["6/8/8", "6/8/9"], &["3/1/1"])];
        let mut inp = input(&regions, vec![mac("m4", 1.0, false), mac("m1", 0.5, true)], &cost);
        (inp.chains, inp.after) = (chains(), Vec::new());
        let f = forecast(&inp);
        // While the lead builds the terrain the units wait for, the member makes the candidates
        // (from the far end), 200 s each at its pace.
        let m1 = &f.lanes["m1"];
        assert_eq!((m1[0].step.as_str(), m1[0].from, m1[0].until, m1[0].n), ("pois", 1_000_000, 1_000_400, 2));
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
}
