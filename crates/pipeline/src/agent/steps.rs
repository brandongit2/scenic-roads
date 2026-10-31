//! The steps table (docs/pipeline.md): one row per step the agent runs as a job, every row the
//! same fields, so the whole pipeline is read with a few concepts:
//!
//! - **id** (`name`): the step's stored name, in the records, the journal, the leases, the timings
//!   and the floors, and the name every reader shows (task #659: no display names); renamed only
//!   with `RENAMED` (a rename would re-key what's built: so a z6 tile's roads are still `unit`).
//! - **does**: one sentence of what it makes from what.
//! - **kind**: where its output goes (`Kind`: a fetch from outside, a derived work file, a pack the
//!   map serves, a publish, upkeep).
//! - **scope**: what one target is (`Scope`: the world, a z3 tile, a z6 tile, an input's inputs
//!   folder); its noun counts a job's targets everywhere ("8 z6 tiles").
//! - **stage**: where it sits in the build's order (`STAGES`): the checklist's lines, the page's
//!   groups and colours.
//! - **place**: which Mac runs it (`Place`: the lead alone, or any member), derived from the pool's
//!   rules (crate::agent::every), never kept by hand.
//! - **resources**: the memory a job of it is first guessed to take until a target's own run says,
//!   whether members take it (the shared steps; which ready job goes first is derived from the
//!   reads, `readers_first`, never kept by hand), whether it's the lead's alone (a
//!   run alone of the whole build, or one that keeps the pass's answers: crate::agent::every), and
//!   what it may write (its write-set: the logical names its jobs may add, change or remove in the
//!   build's manifest). How many targets go in a job is measured, not kept here: as many as fit a
//!   quarter of an hour (crate::coord::JOB_S), a step kept to the lead's in one job.
//!
//! Every human text about a step is templated from its row (its id, `said`, `counted`): the jobs'
//! names, the checklist, the build page, `scenic status`. The tasks a job offers (`TASKS`) are
//! described alike.
//!
//! Not in it: the disk a job starts with (the owner's room target and the job's measured disk, the disk guard
//! behind it), groups of steps never run at once on one Mac (the raw tiles' accessor, the budgets'
//! proxy and the answers' lease guard what they share where it's used), and which jobs run beside
//! which (the coordinator packs a Mac's jobs by their measures; a cold start by its provisional
//! need, `mem_mb` times 3/2: docs/pool.md §7.7).
//!
//! The step sets (`SHARED`, `ANSWERED`) are made from the table at compile time (a set whose size
//! isn't what the table gives fails the build), and a step's first guess of memory is read from its
//! row (`row`).
//!
//! The write-sets: one outputs function per step (`writes`, its row's), which both ends use, so
//! they can't disagree (docs/pool.md §7.3, §12 phase 4's sixth batch): a job's `Out`, bound to its
//! step (crate::out::Out::for_step), puts and removes only names it gives, a name outside failing
//! the job; and the pool's lead refuses every journal entry with a change outside it (`outside`,
//! in crate::agent::pool's check). A shared step's files for its targets (`saves`, which
//! crate::coord's check of a member's hand-off and the merge's check use) are checked beside it.

use crate::legacy::TileId;
use serde::{Deserialize, Serialize};

/// Where a step's output goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Brings sources in from outside (`sources/`).
    Fetch,
    /// Makes work files other steps read (`work/`, `global/`, `sources/osm/<date>/`).
    Derive,
    /// Makes what the map serves (`layers/`, `base/`, `hidata/`, …).
    Pack,
    /// Makes a catalogue: the map's new data goes out.
    Publish,
    /// Keeps the build in order: checks, lists, removals, backups; nothing the map reads.
    Keep,
}

/// What one target of a step is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Everything at once: one target, the step's own name (or a pass's date).
    World,
    /// A z3 tile ("3/x/y"): 8 × 8 z6 tiles. (Stored and sent as "area": `/work/steps`' `scope`.)
    #[serde(rename = "area")]
    Z3,
    /// A z6 tile ("6/x/y"). (Stored and sent as "tile".)
    #[serde(rename = "tile")]
    Z6,
    /// A z10 tile ("10/x/y"): a deep pack's (crate::terrain_deep::PACK_Z).
    Z10,
    /// A polar block ("s/4/x/y": a pole and a z4 tile of its grid, crate::polar::target).
    Polar,
    /// An input's inputs folder (docs/inputs.md §4.3).
    Input,
    /// A task's piece of a job (`TASKS`).
    Task,
}

impl Scope {
    /// The noun a target is counted in.
    pub const fn noun(self) -> &'static str {
        match self {
            Scope::World => "run",
            Scope::Z3 => "z3 tile",
            Scope::Z6 => "z6 tile",
            Scope::Z10 => "z10 tile",
            Scope::Polar => "polar block",
            Scope::Input => "input",
            Scope::Task => "task",
        }
    }
}

/// Which Mac runs a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    /// The lead alone.
    Lead,
    /// Any member whose measured room fits (crate::agent::every).
    Any,
}

/// A stage of the build: a group of steps in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Stage {
    pub id: &'static str,
    /// What the stage makes, in a sentence.
    pub does: &'static str,
}

/// The stages, in the build's order (docs/pipeline.md §2).
pub const STAGES: [Stage; 14] = [
    Stage { id: "pass", does: "The newest planet read once for the whole world: its sets, outlines, pieces and road values, the basemap, the water, and what the regions' steps read." },
    Stage { id: "sites", does: "The heritage sites and designated areas the regions' tiles read." },
    Stage { id: "terrain", does: "The terrain packs, a tile at a time down to each place's native zoom, assembled zoomed out by z3 tile." },
    Stage { id: "slope", does: "The shading packs (steepness and direction) from the terrain, a tile at a time down to each place's native zoom, assembled zoomed out by z3 tile." },
    Stage { id: "trees", does: "The tree cover, height and leaf type, a tile at a time, assembled zoomed out by z3 tile." },
    Stage { id: "depth", does: "The depth under the seas and lakes and its shading from the depth sources: a tile at a time where a source has fine detail, then zoomed out by z3 tile as the means of the finer." },
    Stage { id: "roads", does: "Each tile's roads, their elevations and scenery: the base packs the map's details and profiles read." },
    Stage { id: "tiles", does: "The road and rail map tiles drawn from the base packs, by tile and zoomed out by z3 tile." },
    Stage { id: "finish", does: "What publishing adds around the tiles: the road index, rail stops, ferries, the world levels, and a prune." },
    Stage { id: "trains", does: "Trains a day on the regions' rail, from timetables." },
    Stage { id: "landmarks", does: "The landmarks: candidates, peaks, Wikidata facts, heritage details, ranked and drawn, and the area overlays." },
    Stage { id: "buildings", does: "Every building in the coverage, raised in 3D, a tile at a time." },
    Stage { id: "publish", does: "A catalogue: the map's new data goes out." },
    Stage { id: "upkeep", does: "Checks, to-do lists, removals and backups; nothing the map reads." },
];

/// A step's row.
#[derive(Clone, Copy, Debug)]
pub struct Step {
    /// Its id: the stored name, the name every reader shows (task #659: no display names).
    pub name: &'static str,
    /// What it makes from what, in a sentence.
    pub does: &'static str,
    pub kind: Kind,
    pub scope: Scope,
    /// Its stage's id (`STAGES`).
    pub stage: &'static str,
    /// The memory a job of it is expected to take (MB) before a target's own run has said
    /// (`SCENIC_COSTS`, crate::coord::Cost): offered so to the members, and a cold start packed
    /// beside other jobs at 3/2 of it (crate::coord::sched::provisional_mb). (Z6 tiles and candidates are offered by their extract's size, terrain by its z3 tile's,
    /// the 3D buildings by their rows: crate::agent's offers.)
    pub mem_mb: u64,
    /// Other members take its jobs (the shared steps). No preference among them is kept here:
    /// which ready job goes first is derived from the reads (`readers_first`).
    pub shared: bool,
    /// A run of the whole build's, the lead's alone (not offered to members: crate::agent::every).
    pub alone: bool,
    /// It keeps the pass's Wikidata and Wikipedia answers (crate::answers): none starts while the
    /// agent sends them as it starts.
    pub answered: bool,
    /// Its write-set: whether a job of it may write `logical` (`removal`: removes it), for some
    /// target of its own.
    pub writes: fn(&str, bool) -> bool,
    /// Its jobs only change build records (no file made, read or removed): never a cold start,
    /// packed beside any jobs at one thread and little memory (task #838, crate::coord::sched).
    pub records_only: bool,
    /// Everything one target of it reads, and so waits for (`Read`, docs/readiness.md §2): the
    /// planner, the forecast, the page and `scenic status` all answer readiness per target from it
    /// (`readiness`, `read_targets`). Empty: it reads no other step's targets.
    pub reads: &'static [Read],
    /// The way it runs now, as far as its memory and time go (crate::coord::cost_version): a cost
    /// measured another way says nothing of what a run takes now. Bumped with a change that moves
    /// them; never a build key.
    pub cost_v: u32,
}

/// One thing a target's run reads (docs/readiness.md §2).
#[derive(Clone, Copy, Debug)]
pub enum Read {
    /// Some targets of another step, by this target (any order, repeats allowed; one listed that
    /// isn't among its step's targets as built is never left).
    Targets { step: &'static str, of: fn(&str) -> Vec<String> },
    /// The mids of some targets of another step (the z9 means an assembly reads of its z6 tiles):
    /// left while a target's mid isn't current (`View::mid_left`: stale, or current without its
    /// mid, made again for it), which what reads its packs doesn't wait for.
    Mids { step: &'static str, of: fn(&str) -> Vec<String> },
    /// Every target of another step (a chain's: it reads what all its targets make, e.g.
    /// depth-root every depth-lo assembly).
    Whole { step: &'static str },
    /// An input that is no step's target (a spacing file, a pass's set): `Ready::Unknown` while
    /// the caller's `View` can't work it out.
    Input { name: &'static str },
}

/// Whether a target may run (`readiness`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ready {
    Yes,
    /// The reads still left: (step, target), a `Read::Whole`'s target its step's own name.
    Waits(Vec<(&'static str, String)>),
    /// The inputs that can't be worked out now.
    Unknown(Vec<&'static str>),
}

/// What a caller knows of the build, for `readiness` (docs/readiness.md §3).
pub trait View {
    /// Target `t` of `step` is left (stale or unbuilt, runnable or not).
    fn left(&self, step: &str, t: &str) -> bool;
    /// Target `t`'s mid isn't current (a `Read::Mids`): it's left, or current without its mid.
    fn mid_left(&self, step: &str, t: &str) -> bool {
        self.left(step, t)
    }
    /// Any target of `step` is left (a `Read::Whole`).
    fn any_left(&self, step: &str) -> bool;
    /// Input `name` can be worked out for target `t` (`Read::Input`); known unless the view says.
    fn known(&self, _name: &str, _t: &str) -> bool {
        true
    }
}

/// A read target (`read_targets`): one target of a step, or a step's every target.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadTarget {
    One(&'static str, String),
    Whole(&'static str),
}

/// What target `t` of `step` waits for, its row's declaration (`Step::reads`): the one list the
/// planner and the forecast both wait on. Sorted, no repeats; inputs aren't targets.
pub fn read_targets(step: &str, t: &str) -> Vec<ReadTarget> {
    let mut v: Vec<ReadTarget> = Vec::new();
    for r in row(step).map_or(&[][..], |r| r.reads) {
        match *r {
            Read::Targets { step, of } | Read::Mids { step, of } => {
                v.extend(of(t).into_iter().map(|u| ReadTarget::One(step, u)))
            }
            Read::Whole { step } => v.push(ReadTarget::Whole(step)),
            Read::Input { .. } => {}
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// Whether target `t` of `step` may run in `view` (docs/readiness.md §3): `Waits` while a target or
/// step it reads is left, else `Unknown` while an input it reads can't be worked out, else `Yes`.
pub fn readiness(step: &str, t: &str, view: &dyn View) -> Ready {
    let reads = row(step).map_or(&[][..], |r| r.reads);
    let mut waits: Vec<(&'static str, String)> = Vec::new();
    for r in reads {
        match *r {
            Read::Targets { step, of } => {
                waits.extend(of(t).into_iter().filter(|u| view.left(step, u)).map(|u| (step, u)))
            }
            Read::Mids { step, of } => {
                waits.extend(of(t).into_iter().filter(|u| view.mid_left(step, u)).map(|u| (step, u)))
            }
            Read::Whole { step } => waits.extend(view.any_left(step).then(|| (step, step.to_string()))),
            Read::Input { .. } => {}
        }
    }
    waits.sort_unstable();
    waits.dedup();
    if !waits.is_empty() {
        return Ready::Waits(waits);
    }
    let unknown: Vec<&'static str> = reads
        .iter()
        .filter_map(|r| match *r {
            Read::Input { name } => (!view.known(name, t)).then_some(name),
            _ => None,
        })
        .collect();
    if unknown.is_empty() {
        Ready::Yes
    } else {
        Ready::Unknown(unknown)
    }
}

/// The key derivation's scheme (docs/readiness.md §4, §6): 1 gives today's keys byte for byte
/// (each step's legacy adapter, `build::legacy_keys`); 2, the lines of the declared reads, comes
/// with #862's rebuild.
pub const KEY_SCHEME: u32 = 1;

/// Target `t`'s key of `step` (docs/readiness.md §4) under `KEY_SCHEME`; None while it can't be
/// worked out, or for a step scheme 1 doesn't derive (`build::KEYS_FROM_THE_PLAN`, the pass's and
/// upkeep's own).
pub fn key(step: &str, t: &str, x: &super::build::KeyCtx) -> Option<String> {
    super::build::legacy_keys(step, x)?.into_iter().find(|(u, _)| u == t).map(|(_, k)| k)
}

/// The steps a target of `step` reads whole (`Read::Whole`), in declared order.
pub fn wholes(step: &str) -> impl Iterator<Item = &'static str> {
    row(step).map_or(&[][..], |r| r.reads).iter().filter_map(|r| match *r {
        Read::Whole { step } => Some(step),
        _ => None,
    })
}

/// Whether `readiness` says `Yes`.
pub fn ready(step: &str, t: &str, view: &dyn View) -> bool {
    readiness(step, t, view) == Ready::Yes
}

/// The waiting note of a step's targets held (`Plan::held`, the status's `waiting` list, the
/// checklist's notes), from what they wait for (their `Ready::Waits`, any number of targets'):
/// "wait for terrain-hi (12), terrain-lo 3/1/2, summits". Never hand-written: what a target waits
/// for is its declared reads.
pub fn wait_note<'a>(waits: impl IntoIterator<Item = &'a (&'static str, String)>) -> String {
    let mut by: std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>> = Default::default();
    for (s, t) in waits {
        by.entry(s).or_default().insert(t);
    }
    let parts: Vec<String> = by
        .into_iter()
        .map(|(s, ts)| match ts.len() {
            1 if ts.contains(s) => s.to_string(),
            1 => format!("{s} {}", ts.first().unwrap()),
            n => format!("{s} ({n})"),
        })
        .collect();
    format!("wait for {}", parts.join(", "))
}

/// Left targets by step, as the planner gathers them (its `View`): a step in `whole` has every
/// target left (a world step's own), an input in `unknown` can't be worked out; `mids`, by step,
/// the targets whose mid isn't current (stale, or current without it: a `Read::Mids`'s).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Left {
    pub targets: std::collections::BTreeMap<&'static str, std::collections::BTreeSet<String>>,
    pub mids: std::collections::BTreeMap<&'static str, std::collections::BTreeSet<String>>,
    pub whole: std::collections::BTreeSet<&'static str>,
    pub unknown: std::collections::BTreeSet<&'static str>,
}

impl Left {
    /// `step`'s targets `ts` left (with those it has).
    pub fn set(&mut self, step: &'static str, ts: impl IntoIterator<Item = String>) {
        self.targets.entry(step).or_default().extend(ts);
    }

    /// World step `step` left, when `left`.
    pub fn world(&mut self, step: &'static str, left: bool) {
        if left {
            self.whole.insert(step);
        }
    }
}

impl View for Left {
    fn left(&self, step: &str, t: &str) -> bool {
        self.whole.contains(step) || self.targets.get(step).is_some_and(|s| s.contains(t))
    }

    fn mid_left(&self, step: &str, t: &str) -> bool {
        self.whole.contains(step) || self.mids.get(step).is_some_and(|s| s.contains(t))
    }

    fn any_left(&self, step: &str) -> bool {
        self.whole.contains(step) || self.targets.get(step).is_some_and(|s| !s.is_empty())
    }

    fn known(&self, name: &str, _t: &str) -> bool {
        !self.unknown.contains(name)
    }
}

/// The z6 tiles of z3 tile (x, y).
fn z6_under(x: u32, y: u32) -> impl Iterator<Item = (u32, u32)> {
    (0..64u32).map(move |i| (x * 8 + i % 8, y * 8 + i / 8))
}

/// (z, x, y) of a target, at zoom `z` only.
fn at(t: &str, z: u8) -> Option<(u32, u32)> {
    TileId::parse(t).filter(|u| u.z == z).map(|u| (u.x, u.y))
}

/// A tile and its edge neighbours (Horn's border: crate::slope_pack::slope_tile).
fn with_edges(z: u8, x: u32, y: u32) -> impl Iterator<Item = (u32, u32)> {
    std::iter::once((x, y)).chain(super::build::edge_neighbours(z, x, y))
}

/// The tiles of `v` at zoom `z`, as targets.
fn named(z: u8, v: impl IntoIterator<Item = (u32, u32)>) -> Vec<String> {
    v.into_iter().map(|(x, y)| format!("{z}/{x}/{y}")).collect()
}

/// A z3 tile's z6 tiles (terrain-lo's terrain-hi, slope-lo's slope-hi, trees-lo's trees,
/// depth-lo's depth, bldfield-lo's bldtiles: their mids or packs).
fn z6_of_z3(t: &str) -> Vec<String> {
    named(6, at(t, 3).into_iter().flat_map(|(x, y)| z6_under(x, y)))
}

/// slope-deep 10/x/y: the deep packs of it and its edge neighbours, the hi packs of their z6 tiles
/// and the lo packs of their z3 tiles (`slope_deep_from`).
fn slope_deep_reads(t: &str, z: u8) -> Vec<String> {
    let pz = crate::terrain_deep::PACK_Z;
    let k = pz - 6;
    let s = match z {
        6 => k,
        3 => k + 3,
        _ => 0,
    };
    named(z, at(t, pz).into_iter().flat_map(|(x, y)| with_edges(pz, x, y)).map(|(x, y)| (x >> s, y >> s)))
}
fn sd_deep(t: &str) -> Vec<String> {
    slope_deep_reads(t, crate::terrain_deep::PACK_Z)
}
fn sd_hi(t: &str) -> Vec<String> {
    slope_deep_reads(t, 6)
}
fn sd_lo(t: &str) -> Vec<String> {
    slope_deep_reads(t, 3)
}

/// slope-hi 6/x/y: the hi packs of it and its edge neighbours, their z3 tiles' lo packs
/// (`slope_piece_from`), and its z10 tiles' slope-deep parts.
fn sh_hi(t: &str) -> Vec<String> {
    named(6, at(t, 6).into_iter().flat_map(|(x, y)| with_edges(6, x, y)))
}
fn sh_lo(t: &str) -> Vec<String> {
    named(3, at(t, 6).into_iter().flat_map(|(x, y)| with_edges(6, x, y)).map(|(x, y)| (x >> 3, y >> 3)))
}
fn sh_deep(t: &str) -> Vec<String> {
    let pz = crate::terrain_deep::PACK_Z;
    let s = 1u32 << (pz - 6);
    named(pz, at(t, 6).into_iter().flat_map(|(x, y)| (0..s * s).map(move |i| (x * s + i % s, y * s + i / s))))
}

/// slope-lo 3/x/y: the lo terrain of it and its edge neighbours (`slope_lo_key`'s terrain).
fn sl_lo(t: &str) -> Vec<String> {
    named(3, at(t, 3).into_iter().flat_map(|(x, y)| with_edges(3, x, y)))
}

/// unit 6/x/y: the terrain packs over its tile + 30 km (crate::stage::MARGIN_KM) its staged tiles
/// and its roads' heights read (`build::unit_terrain_from`, `build::terrain_packs`): the lo packs
/// of z4–8's z3 tiles, the hi packs of its box's z6 tiles (z9–12's), its box's deep packs.
fn unit_reads(t: &str, z: u8) -> Vec<String> {
    let Some((x, y)) = at(t, 6) else { return Vec::new() };
    let b = crate::stage::tile_box_grown(6, x, y, crate::stage::MARGIN_KM);
    let mut v: Vec<(u32, u32)> = Vec::new();
    let mut under = |zs: std::ops::RangeInclusive<u8>, to: u8| {
        for z in zs {
            let [x0, x1, y0, y1] = crate::stage::tile_range(z, b);
            let s = z - to;
            for a in x0 >> s..=x1 >> s {
                v.extend((y0 >> s..=y1 >> s).map(|c| (a, c)));
            }
        }
    };
    match z {
        3 => under(4..=8, 3),
        6 => under(9..=12, 6),
        _ => {}
    }
    if z == 6 || z == crate::terrain_deep::PACK_Z {
        v.extend(crate::stage::tiles_in(z, b));
    }
    v.sort_unstable();
    v.dedup();
    named(z, v)
}
fn u_lo(t: &str) -> Vec<String> {
    unit_reads(t, 3)
}
fn u_hi(t: &str) -> Vec<String> {
    unit_reads(t, 6)
}
fn u_deep(t: &str) -> Vec<String> {
    unit_reads(t, crate::terrain_deep::PACK_Z)
}

/// polar-slope s/4/x/y: its block's terrain and its neighbours' (crate::polar::around).
fn ps_terrain(t: &str) -> Vec<String> {
    crate::polar::parse_target(t).map_or_else(Vec::new, |(pole, b)| {
        std::iter::once(b).chain(crate::polar::around(pole, b)).map(|n| crate::polar::target(pole, n)).collect()
    })
}

/// polar-root `s`: its pole's blocks (crate::polar::blocks).
fn pole_blocks(t: &str) -> Vec<String> {
    roadcore::polar::Pole::of_letter(t)
        .map_or_else(Vec::new, |p| crate::polar::blocks(p).into_iter().map(|b| crate::polar::target(p, b)).collect())
}

/// bldtiles 6/x/y: the normalized files of it and its 8 neighbours (`build::bld_around`).
fn bt_prep(t: &str) -> Vec<String> {
    at(t, 6).map_or_else(Vec::new, |(x, y)| {
        super::build::bld_around(x, y)
            .into_iter()
            .filter_map(|n| match n {
                (Some(a), Some(b)) => Some(format!("6/{a}/{b}")),
                _ => None,
            })
            .collect()
    })
}

/// peaks 6/x/y: its own tile's candidates.
fn own(t: &str) -> Vec<String> {
    vec![t.to_string()]
}

use Read::{Input as In, Mids, Targets as Of, Whole as All};
const R_TERRAIN_LO: &[Read] = &[Mids { step: "terrain-hi", of: z6_of_z3 }];
const R_SLOPE_DEEP: &[Read] = &[
    Of { step: "terrain-deep", of: sd_deep },
    Of { step: "terrain-hi", of: sd_hi },
    Of { step: "terrain-lo", of: sd_lo },
];
const R_SLOPE_HI: &[Read] = &[
    Of { step: "terrain-hi", of: sh_hi },
    Of { step: "terrain-lo", of: sh_lo },
    Of { step: "slope-deep", of: sh_deep },
];
const R_SLOPE_LO: &[Read] = &[Of { step: "slope-hi", of: z6_of_z3 }, Of { step: "terrain-lo", of: sl_lo }];
const R_TREES_LO: &[Read] = &[Mids { step: "trees", of: z6_of_z3 }];
// (The roads wait for the pass's heritage sites, reaches and roadside buildings: their slices, which
// z6 tiles the coverage builds, the roadside index.)
const R_UNIT: &[Read] = &[
    Of { step: "terrain-lo", of: u_lo },
    Of { step: "terrain-hi", of: u_hi },
    Of { step: "terrain-deep", of: u_deep },
    All { step: "heritage-sites" },
    All { step: "reach" },
    All { step: "buildings" },
];
const R_POLAR_SLOPE: &[Read] = &[Of { step: "polar-terrain", of: ps_terrain }];
// (A pole's root reads its own blocks, not the other pole's.)
const R_POLAR_ROOT: &[Read] =
    &[Of { step: "polar-terrain", of: pole_blocks }, Of { step: "polar-slope", of: pole_blocks }];
const R_DEPTH_LO: &[Read] = &[Of { step: "depth", of: z6_of_z3 }];
// (Both: an assembly whose z6 tiles are stale isn't stale itself till they're made.)
const R_DEPTH_ROOT: &[Read] = &[All { step: "depth" }, All { step: "depth-lo" }];
const R_RAIL: &[Read] = &[All { step: "rail-feeds" }];
const R_POIS: &[Read] = &[All { step: "trailends" }, In { name: "spacing" }];
// (The summits and peaks read the terrain packs' z8, a digest of every lo pack
// (crate::peaks::z8::digest), and lo packs are made from the hi packs: they wait for all of both.)
const R_SUMMITS: &[Read] = &[All { step: "terrain-hi" }, All { step: "terrain-lo" }];
// (A read's own reads where its key names them, so a target isn't made on what's about to change:
// a candidate's key the hiking-route ends, the heritage chain's the sites.)
const R_PEAKS: &[Read] = &[
    Of { step: "pois", of: own },
    All { step: "trailends" },
    All { step: "summits" },
    All { step: "terrain-hi" },
    All { step: "terrain-lo" },
    In { name: "spacing" },
];
const R_ITEMS: &[Read] = &[All { step: "pois" }, All { step: "trailends" }, In { name: "spacing" }];
// (The accepted heritage registers: without them the sites have no key.)
const R_HERITAGE: &[Read] = &[All { step: "heritage-sites" }, In { name: "registers" }];
const R_MARKS: &[Read] = &[
    All { step: "pois" },
    All { step: "peaks" },
    All { step: "items" },
    All { step: "heritage" },
    All { step: "trailends" },
    All { step: "summits" },
    All { step: "heritage-sites" },
    In { name: "spacing" },
    In { name: "registers" },
];
// (The overlays read the heritage chain and the marks' dots, from the last marks run: held while
// the marks' inputs are left, not for the marks themselves.)
const R_OVERLAYS: &[Read] = &[
    All { step: "pois" },
    All { step: "peaks" },
    All { step: "items" },
    All { step: "heritage" },
    All { step: "trailends" },
    All { step: "summits" },
    All { step: "heritage-sites" },
    In { name: "spacing" },
    In { name: "registers" },
    In { name: "heritage-dots" },
];
// (The normalized files read the declared release, not what the fetch puts on the NAS: they go
// beside it. The tiles read the sources downloaded: none before, every tile would come out empty.)
const R_BLDPREP: &[Read] = &[In { name: "bld-key" }];
const R_BLDTILES: &[Read] = &[Of { step: "bldprep", of: bt_prep }, In { name: "bld-sources" }];
const R_BLDFIELD_LO: &[Read] = &[Of { step: "bldtiles", of: z6_of_z3 }, In { name: "bld-sources" }];

/// A row with what most steps have: 1.5 GB until measured, none of the sets.
const fn step(
    stage: &'static str,
    kind: Kind,
    scope: Scope,
    name: &'static str,
    does: &'static str,
    writes: fn(&str, bool) -> bool,
) -> Step {
    Step {
        name,
        does,
        kind,
        scope,
        stage,
        mem_mb: 1500,
        shared: false,
        alone: false,
        answered: false,
        writes,
        records_only: false,
        reads: &[],
        cost_v: 0,
    }
}

use Kind::{Derive, Fetch, Keep, Pack, Publish};
use Scope::{Input, Polar, World, Z10, Z3, Z6};

/// The table, by stage (the ranks order what's ranked).
pub const TABLE: [Step; 59] = [
    // The OSM pass: the planet filtered, its sets, extracts and road values; then its worldwide steps.
    Step {
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "osm-pass",
            "Filters the newest planet, cuts its sets and z6 pieces, makes the basemap and the road values.",
            w_osm_pass,
        )
    },
    Step {
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "pass-sets",
            "Cuts the sets a finished pass lacks in their current filters.",
            w_pass_sets,
        )
    },
    // (osmium's export of the outline set and the assembly: 8.3 GB at most measured, the
    // 2026-09-28 set, 5 min 20 s on the M4, 2026-10-15.)
    Step {
        mem_mb: 9216,
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "outlines",
            "Assembles the countries' and regions' outlines from the pass's outline set.",
            w_outlines,
        )
    },
    Step {
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "trailends",
            "Finds the ends of the world's hiking routes.",
            w_trailends,
        )
    },
    Step {
        alone: true,
        ..step("pass", Derive, World, "reach", "Works out how far each tile's roads reach.", w_reach)
    },
    // (`buildtiles::WORKERS` files read at once: 2.1 GB at the peak on the Mac mini, 23 Oct, task
    // #579.)
    Step {
        mem_mb: 2600,
        alone: true,
        ..step(
            "pass",
            Fetch,
            World,
            "buildings",
            "Collects the world's roadside building boxes from the declared buildings release.",
            w_buildings,
        )
    },
    Step { alone: true, reads: R_SUMMITS, ..step("pass", Derive, World, "summits", "Finds the world's summits.", w_summits) },
    // (The labels' 6.7M rows held with their tiles: the process peaked at 2.6 GB on the 2026-09-28
    // pass, 23 Oct, measured by sampling its RSS; osmium's own processes, run beside it, are
    // not in that.)
    Step {
        mem_mb: 3072,
        alone: true,
        ..step(
            "pass",
            Pack,
            World,
            "labels",
            "Ranks the world's place labels and packs them.",
            w_labels,
        )
    },
    // (The lakes filled from Canada's own data: a band's lakes and their pieces held, and the
    // merged fill's, 10.4 GB at most measured, 2026-10-15.)
    Step {
        mem_mb: 12288,
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "lakefill",
            "Fills in Canada's lakes missing from OpenStreetMap.",
            w_lakefill,
        )
    },
    // (The worldwide water: its z14 directory and stored tiles held, 7.7 GB at most measured,
    // 2026-10-08.)
    Step {
        mem_mb: 8192,
        alone: true,
        ..step("pass", Pack, World, "water", "Draws the world's water at every zoom.", w_water)
    },
    Step {
        ..step(
            "pass",
            Derive,
            World,
            "spoken",
            "Maps the languages spoken where, for names.",
            w_spoken,
        )
    },
    // The regions' extract (crate::inputs::regions::extract): the pass's planet read three times,
    // a heavy NAS reader.
    Step {
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "regions-extract",
            "Extracts the regions' OSM relations and ways.",
            w_regions_extract,
        )
    },
    // The coastline's land (crate::inputs::regions::land): the pass's coastline set assembled and
    // cut by 2° cell, every node and ring held at once.
    Step {
        mem_mb: 8192,
        alone: true,
        ..step(
            "pass",
            Derive,
            World,
            "regions-land",
            "Assembles the coastline's land from the pass's coastline.",
            w_regions_land,
        )
    },
    Step {
        alone: true,
        answered: true,
        ..step(
            "sites",
            Derive,
            World,
            "heritage-sites",
            "Finds the regions' heritage sites and designated areas.",
            w_heritage_sites,
        )
    },
    // Terrain's z6 tiles (a z6 tile each, eight a job; a z3 tile is its whole run, the scheme
    // before z6 tiles).
    Step {
        ..step(
            "terrain",
            Derive,
            World,
            "terrain-water",
            "Reads the water the terrain flattens, for the terrain's keys.",
            w_terrain_water,
        )
    },
    // (The lead's: its archives go under `sources/pyramids/`, outside the records, handed off for
    // the lead to name in their stores' indexes, as AWS's raw tiles' are: crate::pyramids. A z6
    // tile's fetch at most 4,096 tiles in memory at once, its wanted set a few MB.)
    Step {
        ..step(
            "terrain",
            Fetch,
            Z6,
            "pyramids-fill",
            "Fetches a tile's elevation tiles from the pyramids read tile by tile (GSI's) once, onto the NAS, for the terrain to read there.",
            w_none,
        )
    },
    // (Its memory offered by target, the agent's terrain_peak: a z6 tile's levels' bound,
    // crate::terrain_pack::piece_peak_mb, and its deep z12's north of 67° where a fine dataset meets
    // it, crate::terrain_deep::peak_mb; with the coverage its process reads,
    // crate::coverage::Coverage::read_peak_mb. Until a z6 tile's own run says.)
    Step {
        cost_v: 7,
        shared: true,
        ..step(
            "terrain",
            Pack,
            Z6,
            "terrain-hi",
            "Builds a tile's terrain hi pack (z9–12), its water flattened.",
            w_terrain_hi,
        )
    },
    // (Its memory from the code's bounds: its base made as a z6 tile's run at most, then its deep
    // tiles, crate::terrain_deep::target_peak_mb; the agent adds the coverage its process reads,
    // crate::coverage::Coverage::read_peak_mb. Until a target's own run says.)
    Step {
        mem_mb: crate::terrain_deep::target_peak_mb(),
        shared: true,
        ..step(
            "terrain",
            Pack,
            Z10,
            "terrain-deep",
            "Builds a z10 tile's terrain past the base's zoom (z13–16) where a declared elevation dataset has the detail, from the base made as the tiles' terrain is.",
            w_terrain_deep,
        )
    },
    // (Its z3 tile's 1,365 zoomed-out tiles, ~270 KB each made, its z6 tiles' z9 means, up to 4 MB a
    // z6 tile, and half a GB besides.)
    Step {
        cost_v: 2,
        reads: R_TERRAIN_LO,
        mem_mb: 1200,
        ..step(
            "terrain",
            Pack,
            Z3,
            "terrain-lo",
            "Assembles a z3 tile's zoomed-out terrain from its tiles.",
            w_terrain_lo,
        )
    },
    // (Slope's memory from the tiles each job holds: crate::slope_pack::deep_peak_mb, peak_mb,
    // lo_peak_mb; slope-deep's and slope-hi's with the coverage their process reads added by the
    // agent, crate::coverage::Coverage::read_peak_mb.)
    Step {
        reads: R_SLOPE_DEEP,
        mem_mb: crate::slope_pack::deep_peak_mb(),
        shared: true,
        ..step(
            "slope",
            Pack,
            Z10,
            "slope-deep",
            "Works out a z10 tile's shading from its terrain down to its deep levels (z10–16), its z10–12 handed on to its tile's.",
            w_slope_deep,
        )
    },
    Step {
        cost_v: 4,
        reads: R_SLOPE_HI,
        mem_mb: crate::slope_pack::peak_mb(),
        shared: true,
        ..step(
            "slope",
            Pack,
            Z6,
            "slope-hi",
            "Works out a tile's shading from its terrain (z6–12), its z10 tiles' deep parts taken in.",
            w_slope_hi,
        )
    },
    Step {
        cost_v: 1,
        reads: R_SLOPE_LO,
        mem_mb: crate::slope_pack::lo_peak_mb(),
        ..step(
            "slope",
            Pack,
            Z3,
            "slope-lo",
            "Assembles a z3 tile's zoomed-out slope from its tiles.",
            w_slope_lo,
        )
    },
    // The polar caps past the Web Mercator tiles' 85.05° (crate::polar; docs/polar.md): a block's
    // terrain, then its shading, then a pole's root of both. (A block's z9 tiles held whole: its
    // heights and shading, 1 GB, and the pool's cache: 3.4 GB at most measured on the M4, 25 Oct.)
    Step {
        mem_mb: 4096,
        ..step(
            "terrain",
            Pack,
            Polar,
            "polar-terrain",
            "Samples a polar block's terrain past 85° from the elevation pool (z4–9).",
            w_polar_terrain,
        )
    },
    Step {
        reads: R_POLAR_SLOPE,
        mem_mb: 2048,
        ..step(
            "slope",
            Pack,
            Polar,
            "polar-slope",
            "Works out a polar block's shading from its terrain and its neighbours' (z4–9).",
            w_polar_slope,
        )
    },
    Step {
        reads: R_POLAR_ROOT,
        ..step(
            "slope",
            Pack,
            World,
            "polar-root",
            "Builds a pole's zoomed-out terrain and shading (z0–3) from its blocks.",
            w_polar_root,
        )
    },
    // (Tree cover's program holds a band of a block's rows on each thread and the blocks made but
    // not yet written: 1.05 GB on 14 threads for 3/2/2's 792 blocks, 2026-10-05; a z6 tile has 16
    // blocks at most. Its assembly its z3 tile's blocks' zoom-8 values, compressed. A job that makes
    // a leaf-type square holds it whole and four chunks' source windows: 2.3 GB at most for six
    // NALCMS squares, 0.8 GB for two of the EEA's, task #580, 2026-10-23. A first guess only: each
    // target's measured peak replaces it.)
    // (Cost version 3: the canopy rows read where they lie and counted, its blocks paced, task #606:
    // 2's copied whole squares uncounted, so its NAS weight was its uploads alone.)
    Step {
        cost_v: 3,
        mem_mb: 2500,
        shared: true,
        ..step("trees", Pack, Z6, "trees", "Maps a tile's tree cover, height and leaf type.", w_trees)
    },
    Step {
        cost_v: 1,
        reads: R_TREES_LO,
        mem_mb: 500,
        ..step(
            "trees",
            Pack,
            Z3,
            "trees-lo",
            "Assembles a z3 tile's zoomed-out tree cover from its tiles.",
            w_trees_lo,
        )
    },
    // (Depth: a block's arrays and distance fields, ~0.3 GB; the sources' blocks read, 0.5 GB at
    // most; the tiles made, kept as PNG; the water of a level's tiles, 0.13 GB at most. Until a
    // target's own run says.)
    // (Cost version 2: composed only at a grid's own zoom, the coarser levels the means of the finer,
    // and the floor's shading's assembly made in depth-lo, task #804; 1: a grid lined up with no
    // overview sampled from one askew, task #433.)
    Step {
        cost_v: 2,
        mem_mb: 2500,
        shared: true,
        ..step(
            "depth",
            Pack,
            Z6,
            "depth",
            "Composes a tile's depth and its shading where a source has detail finer than the zoomed-out levels.",
            w_depth,
        )
    },
    // (A z3 tile's assembly: its composing, ~2 GB, then its floor's shading, from what that holds
    // (crate::depth::build::lo_peak_mb).)
    Step {
        cost_v: 2,
        reads: R_DEPTH_LO,
        mem_mb: crate::depth::build::lo_peak_mb(),
        ..step(
            "depth",
            Pack,
            Z3,
            "depth-lo",
            "Assembles a z3 tile's zoomed-out depth and its shading from its tiles' finer ones, composed from the depth sources where there are none.",
            w_depth_lo,
        )
    },
    // (The root: 64 z3 tiles of each layer read, 21 made.)
    Step {
        reads: R_DEPTH_ROOT,
        ..step(
            "depth",
            Pack,
            World,
            "depth-root",
            "Builds the world-level depth and its shading from the z3 tiles' zoomed-out packs.",
            w_depth_root,
        )
    },
    // (A z6 tile's most its jobs took here, 8.4 GB, 2026-10-05; offered by its extract's size.)
    // (Cost version: a z6 tile's measure its job's processes together, as the memory guard measures a
    // job; 0 was its programs' most, one at a time; 2: its processes started without malloc's large
    // cache, store::spawn::NO_LARGE_CACHE, where 1's measures counted 6–7 GB of freed blocks; 3: its
    // elevations sampled in batches, crate::dem::generic::BATCH, where 2's sampled a z6 tile's
    // vertices at once: 16 GB for the densest.)
    Step {
        cost_v: 3,
        reads: R_UNIT,
        mem_mb: 8600,
        shared: true,
        ..step(
            "roads",
            Pack,
            Z6,
            "unit",
            "Builds a tile's roads, their elevations and scenery: its base pack and road values.",
            w_unit,
        )
    },
    Step {
        ..step("tiles", Pack, Z6, "pack", "Draws a tile's road and rail map tiles and its data.", w_pack)
    },
    Step {
        ..step(
            "tiles",
            Pack,
            Z3,
            "lo",
            "Draws a z3 tile's zoomed-out road and rail map tiles.",
            w_lo,
        )
    },
    Step { ..step("finish", Keep, World, "prune", "Removes what the regions no longer cover.", w_prune) },
    Step {
        ..step(
            "finish",
            Derive,
            World,
            "roadunits",
            "Works out which tiles each road crosses.",
            w_roadunits,
        )
    },
    Step {
        ..step("finish", Pack, World, "stations", "Places the rail stops near the regions.", w_stations)
    },
    Step { ..step("finish", Pack, World, "ferries", "Maps the world's ferries.", w_ferries) },
    Step {
        ..step(
            "finish",
            Pack,
            World,
            "terrain-root",
            "Builds the world-level terrain from the z3 tiles' zoomed-out packs.",
            w_terrain_root,
        )
    },
    Step {
        ..step(
            "finish",
            Pack,
            World,
            "slope-root",
            "Builds the world-level slope from the z3 tiles' zoomed-out packs.",
            w_slope_root,
        )
    },
    Step {
        // (The coverage read from the regions, the pass's outlines for its countries, then the
        // feeds' checks, in one process: 4.46 GB at its peak on the Mac mini, 23 Oct, task #578;
        // the coverage itself, shared with every step, is most of it.)
        mem_mb: 5632,
        ..step(
            "trains",
            Fetch,
            World,
            "rail-feeds",
            "Fetches the regions' rail and ferry timetables.",
            w_rail_feeds,
        )
    },
    Step {
        reads: R_RAIL,
        mem_mb: 6144,
        ..step("trains", Derive, World, "rail", "Counts trains a day on the regions' rail.", w_rail)
    },
    Step {
        mem_mb: 2048,
        shared: true,
        reads: R_POIS,
        ..step("landmarks", Derive, Z6, "pois", "Finds a tile's landmark candidates.", w_pois)
    },
    Step {
        reads: R_PEAKS,
        mem_mb: 2500,
        shared: true,
        ..step("landmarks", Derive, Z6, "peaks", "Measures a tile's peaks' prominence and isolation.", w_peaks)
    },
    Step {
        reads: R_ITEMS,
        mem_mb: 3072,
        answered: true,
        ..step(
            "landmarks",
            Fetch,
            World,
            "items",
            "Fetches the landmarks' Wikidata facts and Wikipedia pageviews.",
            w_items,
        )
    },
    // (Cost version 1: the World Heritage outlines clipped from the pass's `whs` set, not the filtered
    // planet; 0's floor, 21.9 GB, no run with its memory timed came near: 6.4 GB, the clip's 4 GB.)
    Step {
        cost_v: 1,
        reads: R_HERITAGE,
        mem_mb: 6144,
        answered: true,
        ..step(
            "landmarks",
            Derive,
            World,
            "heritage",
            "Adds details, fame and outlines to the heritage sites.",
            w_heritage,
        )
    },
    Step {
        reads: R_MARKS,
        mem_mb: 4096,
        ..step(
            "landmarks",
            Pack,
            World,
            "marks",
            "Ranks the landmarks and draws them on the map.",
            w_marks,
        )
    },
    Step {
        mem_mb: 4096,
        reads: R_OVERLAYS,
        ..step("landmarks", Pack, World, "overlays", "Draws the area overlays.", w_overlays)
    },
    // (The coverage read, its outlines indexed (GEOS), the release's footers read, in one process:
    // 4.7 GB at its peak on the Mac mini, 23 Oct, task #579, the coverage most of it; the downloads
    // stream.)
    // (Cost version 1: the coverage asked by distance, never grown, task #493: 0's floors, 43 GB from
    // buffering the regions' outlines, no run since came near: 0.2 GB.)
    Step {
        cost_v: 1,
        mem_mb: 5632,
        ..step(
            "buildings",
            Fetch,
            World,
            "bld-fetch",
            "Fetches the 3D buildings' sources.",
            w_none,
        )
    },
    // (The 3D buildings, until a target's own run says: the densest tile's, Kantō's 6/56/25 and its
    // 30.3 M rows, by the models of task #579's reads and B5's pilot. Offered by the rows they read.)
    Step {
        reads: R_BLDPREP,
        mem_mb: super::bldprep_peak(30_300_000),
        shared: true,
        ..step(
            "buildings",
            Derive,
            Z6,
            "bldprep",
            "Reads a tile's buildings from the sources into normalized files.",
            w_bldprep,
        )
    },
    Step {
        reads: R_BLDTILES,
        mem_mb: super::bldtiles_peak(30_300_000),
        shared: true,
        ..step(
            "buildings",
            Pack,
            Z6,
            "bldtiles",
            "Raises a tile's 3D buildings and its far buildings' field.",
            w_bldtiles,
        )
    },
    // (Its field's lo levels a z3 tile: what crate::bld::field's lo holds, from its sizes.)
    Step {
        reads: R_BLDFIELD_LO,
        mem_mb: crate::bld::field::LO_PEAK_MB,
        ..step(
            "buildings",
            Pack,
            Z3,
            "bldfield-lo",
            "Assembles a z3 tile's zoomed-out buildings' field from its tiles.",
            w_bldfield_lo,
        )
    },
    Step {
        ..step(
            "publish",
            Publish,
            World,
            store::catalogue::NAME,
            "Makes the catalogue the map serves: the new data goes out.",
            w_none,
        )
    },
    Step {
        ..step(
            "publish",
            Publish,
            World,
            store::catalogue::HELD_NAME,
            "Makes a catalogue held for review (inputs/hold-catalog).",
            w_none,
        )
    },
    // The gate's checks (crate::inputs::gate): one per gate unit, ordered before the rest; reading
    // the inputs folders and the accepted copies on the NAS.
    Step {
        ..step(
            "upkeep",
            Keep,
            Input,
            "inputs",
            "Checks an input's files and takes them in.",
            w_inputs,
        )
    },
    Step {
        ..step(
            "upkeep",
            Keep,
            World,
            "names-todo",
            "Lists the names to translate and the descriptions to write.",
            w_none,
        )
    },
    Step {
        ..step(
            "upkeep",
            Keep,
            World,
            "timetables-todo",
            "Lists the lines with no timetable.",
            w_timetables_todo,
        )
    },
    Step {
        alone: true,
        ..step("upkeep", Keep, World, "gc", "Removes replaced files from the NAS.", w_gc)
    },
    Step {
        alone: true,
        records_only: true,
        mem_mb: 256,
        ..step(
            "upkeep",
            Keep,
            World,
            "redo",
            "Forgets the records of targets the owner named (`scenic redo`), so they're built again.",
            w_none,
        )
    },
    Step {
        ..step(
            "upkeep",
            Keep,
            World,
            "backup",
            "Backs up translations, descriptions, inputs and the acceptances.",
            w_none,
        )
    },
];

/// A task a running job offers to any worker (docs/workers.md §3), described as a step is.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Task {
    /// Its kind: the stored name.
    pub name: &'static str,
    pub does: &'static str,
    /// The step whose jobs offer it.
    pub step: &'static str,
    /// The free disk a member's job of it needs (bytes; crate::agent's `helper_need`).
    pub disk: u64,
}

pub const TASKS: [Task; 4] = [
    // (A z6 tile's last steps, its files fetched from the coordinator: 5 GB.)
    Task {
        name: "tail",
        does: "A tile's last steps (its scenic metrics), offered as the lead builds its roads.",
        step: "unit",
        disk: 5 << 30,
    },
    // (A file of tens of MB, and their tiles: 1 GB.)
    Task {
        name: crate::terrain_task::KIND,
        does: "A group of a terrain tile's z8 subtrees.",
        step: "terrain-hi",
        disk: 1 << 30,
    },
    // (It reads the squares where they lie, and writes ~80 MB: 1 GB.)
    Task { name: crate::trees::task::KIND, does: "A row of a tree cover tile's blocks.", step: "trees", disk: 1 << 30 },
    // (A 3D buildings' z8 tile, its blocks: 5 GB.)
    Task { name: crate::bld::task::KIND, does: "A z8 tile of a 3D buildings tile.", step: "bldtiles", disk: 5 << 30 },
];

/// The row of `step` (by its name now: `current`); None for one the table doesn't know (a task's
/// kind, or a step of a newer app's).
pub fn row(step: &str) -> Option<&'static Step> {
    let step = current(step);
    TABLE.iter().find(|s| s.name == step)
}

/// The steps renamed (task #639: by the pack each makes), old name to new. What an older app
/// recorded under an old name (the records' keys, a hand-off's done targets, a journal entry) is
/// the step's under its new one (`current`), so nothing is built again for the name alone.
pub const RENAMED: [(&str, &str); 2] = [("terrain", "terrain-hi"), ("slope", "slope-hi")];

/// `step`'s name now: its new name where it was renamed (`RENAMED`), else itself.
pub fn current(step: &str) -> &str {
    RENAMED.iter().find(|(old, _)| *old == step).map_or(step, |(_, new)| new)
}

/// The task of kind `kind`.
pub fn task(kind: &str) -> Option<&'static Task> {
    TASKS.iter().find(|t| t.name == kind)
}

/// The stage `id` names.
pub fn stage(id: &str) -> Option<&'static Stage> {
    STAGES.iter().find(|s| s.id == id)
}

/// The stage of `step` (a task's its step's).
pub fn stage_of(step: &str) -> Option<&'static Stage> {
    let id = match row(step) {
        Some(s) => s.stage,
        None => task(step).and_then(|t| row(t.step)).map(|s| s.stage)?,
    };
    stage(id)
}

/// What one target of `step` is called ("z6 tile", "z3 tile", "run", "input"; a task's "task"; an
/// unknown step's "target").
pub fn noun(step: &str) -> &'static str {
    match row(step) {
        Some(s) => s.scope.noun(),
        None if task(step).is_some() => Scope::Task.noun(),
        None => "target",
    }
}

/// `n` targets of `step`, counted in its noun: "8 z6 tiles", "1 z3 tile".
pub fn counted(step: &str, n: usize) -> String {
    format!("{n} {}{}", noun(step), if n == 1 { "" } else { "s" })
}

/// A job of `step` over `n` targets, said by its step's name: "terrain-hi: 8 z6 tiles", "outlines"
/// (a worldwide step's one run).
pub fn said(step: &str, n: usize) -> String {
    let t = current(step);
    if row(step).is_some_and(|s| s.scope == World) || n == 0 {
        return t.to_string();
    }
    format!("{t}: {}", counted(step, n))
}

/// Which Mac runs `step`: any member the lead offers it to (crate::agent::every), else the lead.
pub fn place(step: &str) -> Place {
    if super::every::offered(step) {
        Place::Any
    } else {
        Place::Lead
    }
}

/// A step's row as the build page and `scenic status` read it (`/work/steps`): everything but the
/// write-set.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Info {
    pub id: String,
    pub does: String,
    pub kind: Kind,
    pub scope: Scope,
    pub noun: String,
    pub stage: String,
    pub place: Place,
    /// The memory a job of it is first guessed to take (MB).
    pub mem_mb: u64,
    /// A task's: the step whose jobs offer it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<String>,
}

/// Every step's row, in the table's order, then every task's (`of` its step).
pub fn table() -> Vec<Info> {
    let steps = TABLE.iter().map(|s| Info {
        id: s.name.into(),
        does: s.does.into(),
        kind: s.kind,
        scope: s.scope,
        noun: s.scope.noun().into(),
        stage: s.stage.into(),
        place: place(s.name),
        mem_mb: s.mem_mb,
        of: None,
    });
    let tasks = TASKS.iter().map(|t| {
        let s = row(t.step).expect("a task's step has a row");
        Info {
            id: t.name.into(),
            does: t.does.into(),
            kind: s.kind,
            scope: Scope::Task,
            noun: Scope::Task.noun().into(),
            stage: s.stage.into(),
            place: Place::Any,
            mem_mb: 0,
            of: Some(t.step.into()),
        }
    });
    steps.chain(tasks).collect()
}

/// The memory a job of `step` is expected to take before its own run has said (MB; 1.5 GB for a
/// step the table doesn't know).
pub fn mem_mb(step: &str) -> u64 {
    row(step).map_or(1500, |s| s.mem_mb)
}

/// Whether `step`'s jobs only change build records (`Step::records_only`).
pub fn records_only(step: &str) -> bool {
    row(step).is_some_and(|s| s.records_only)
}

/// Whether `step` is a run of the whole build's, the lead's alone.
pub fn alone(step: &str) -> bool {
    row(step).is_some_and(|s| s.alone)
}

/// Which of a row's sets a step set is made of.
#[derive(Clone, Copy)]
enum Set {
    Shared,
    Answered,
}

/// Whether row `s` is in `set`.
const fn in_set(s: &Step, set: Set) -> bool {
    match set {
        Set::Shared => s.shared,
        Set::Answered => s.answered,
    }
}

/// The steps of `set`, in the table's order: N of them, or the build fails.
const fn set_of<const N: usize>(set: Set) -> [&'static str; N] {
    let mut out = [""; N];
    let mut n = 0;
    let mut i = 0;
    while i < TABLE.len() {
        if in_set(&TABLE[i], set) {
            assert!(n < N, "the table has more steps in a set than its size");
            out[n] = TABLE[i].name;
            n += 1;
        }
        i += 1;
    }
    assert!(n == N, "the table has fewer steps in a set than its size");
    out
}

/// Whether some step reads several of `step`'s targets for one of its own (task #909): a slope
/// tile its terrain and its edge neighbours', an assembly its z6 tiles. Probed from the declared
/// reads (`Step::reads`) at a tile of each zoom, never listed by hand. Such a step's targets are
/// planned in compact blocks (`block_order`), so what reads them is ready a neighbourhood at a time
/// rather than once a row's done.
pub fn read_together(step: &str) -> bool {
    const PROBES: [&str; 4] = ["3/4/2", "6/32/20", "10/512/300", "12/2048/1200"];
    TABLE.iter().flat_map(|r| r.reads).any(|x| match *x {
        Read::Targets { step: s, of } | Read::Mids { step: s, of } if s == step => {
            PROBES.iter().any(|t| of(t).len() > 1)
        }
        _ => false,
    })
}

/// Where tile target `t` goes in a compact block order (task #909): its zoom, then its (x, y)
/// interleaved bit by bit (a Z-order curve), so any run of targets taken in order covers squares
/// of tiles, not rows. None for a target that isn't a tile.
pub fn block_order(t: &str) -> Option<(u8, u64)> {
    let u = TileId::parse(t)?;
    let spread = |v: u32| {
        let mut v = v as u64 & 0xffff_ffff;
        v = (v | (v << 16)) & 0x0000_ffff_0000_ffff;
        v = (v | (v << 8)) & 0x00ff_00ff_00ff_00ff;
        v = (v | (v << 4)) & 0x0f0f_0f0f_0f0f_0f0f;
        v = (v | (v << 2)) & 0x3333_3333_3333_3333;
        (v | (v << 1)) & 0x5555_5555_5555_5555
    };
    Some((u.z, spread(u.x) | (spread(u.y) << 1)))
}

/// Whether a target of `reader` reads, directly or through other steps, targets of `step`
/// (`Step::reads`; task #909). Derived from the table alone.
pub fn reads_from(reader: &str, step: &str) -> bool {
    fn go(r: &str, step: &str, seen: &mut Vec<&'static str>) -> bool {
        let Some(row) = row(r) else { return false };
        if seen.contains(&row.name) {
            return false;
        }
        seen.push(row.name);
        row.reads.iter().any(|x| match *x {
            Read::Targets { step: s, .. } | Read::Mids { step: s, .. } | Read::Whole { step: s } => {
                s == step || go(s, step, seen)
            }
            Read::Input { .. } => false,
        })
    }
    reader != step && go(reader, step, &mut Vec::new())
}

/// `v` in the order ready work is granted (docs/pool.md §7.2, task #909): as given (the plan's
/// order), but each item ahead of the first before it of a step it reads (`reads_from`):
/// finishing what's started. Nothing upstream needs a rank to go first: what reads it isn't ready
/// until it's built (`readiness`), so a hand-set order could only starve what's downstream (a
/// ready slope-deep job waited while any terrain-deep job fitted). Steps the reads don't relate
/// keep the plan's order.
pub fn readers_first<T>(v: Vec<T>, step: impl Fn(&T) -> &str) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(v.len());
    for x in v {
        let at = out.iter().position(|y| reads_from(step(&x), step(y))).unwrap_or(out.len());
        out.insert(at, x);
    }
    out
}

/// The shared steps: other members take their jobs (crate::coord leases them), in the table's
/// order (no preference: which ready job goes first is derived from the reads, `readers_first`).
pub const SHARED: [&str; 11] = set_of(Set::Shared);
/// The steps that keep the pass's Wikidata and Wikipedia answers.
pub const ANSWERED: [&str; 3] = set_of(Set::Answered);

/// Whether a job of `step` may write `logical` (`removal`: remove it): its outputs function, the
/// one a job's `Out` and the lead's merge both use. None for a step the table doesn't know.
pub fn writes(step: &str, logical: &str, removal: bool) -> Option<bool> {
    row(step).map(|s| (s.writes)(logical, removal))
}

/// Whether any step of the table writes `logical`: a name no step makes (a file an older pipeline
/// left in the manifest) is no output of today's build.
pub fn made(logical: &str) -> bool {
    TABLE.iter().any(|s| (s.writes)(logical, false))
}

/// Whether journal entry `e`'s changes are all within its step's write-set; why not when they
/// aren't (the first outside it, and how many), or when the table has no row for its step.
pub fn outside(e: &crate::pool::journal::Entry) -> Option<String> {
    outside_by(e, &|l, removal| writes(&e.step, l, removal))
}

/// `outside`, by outputs function `writes` (None: the step isn't known): the simulator's steps have
/// a function of their own (crate::pool::sim).
pub fn outside_by(e: &crate::pool::journal::Entry, writes: &dyn Fn(&str, bool) -> Option<bool>) -> Option<String> {
    let out: Vec<&String> = match e
        .handoff
        .changes
        .iter()
        .map(|(l, v)| writes(l, v.is_none()).map(|ok| (l, ok)))
        .collect::<Option<Vec<_>>>()
    {
        Some(v) => v.into_iter().filter(|(_, ok)| !ok).map(|(l, _)| l).collect(),
        None => {
            return Some(format!(
                "{} isn't a step the steps table knows ({} change{})",
                e.step,
                e.handoff.changes.len(),
                if e.handoff.changes.len() == 1 { "" } else { "s" }
            ))
        }
    };
    let first = out.first()?;
    let what = if e.handoff.changes[*first].is_none() { "removes" } else { "writes" };
    Some(format!(
        "{what} {first}, outside {}'s write-set{}",
        e.step,
        if out.len() > 1 { format!(" (and {} more)", out.len() - 1) } else { String::new() }
    ))
}

/// Whether `l` is a file a job of `step` saves for `target` (what a shared step's hand-off may
/// change for each target it did): a z6 tile's base pack, road values, English and the grids its
/// packs lacked; candidates' and peaks' own files; a tree cover z6 tile's (a z6 tile's) hi packs of
/// the tree layers and its mid, an assembly's (a z3 tile's) lo packs of them; terrain's and slope's
/// z6 tiles (a z6 tile's) hi pack of their layer and its mid, an assembly's (a z3 tile's) lo pack; an
/// a z3 tile's lo pack and its z6 tiles' hi packs of terrain, slope, or the tree layers (a
/// z3 tile's whole run: a lease of the scheme before z6 tiles); a z6 tile's normalized buildings
/// (`bldprep`) or 3D buildings' hi pack, its field's hi pack and mid (`bldtiles`); a z3 tile's field
/// lo pack (`bldfield-lo`). False for the other steps.
pub fn saves(step: &str, target: &str, l: &str) -> bool {
    let dash = target.replace('/', "-");
    let tile = TileId::parse(target);
    let step = current(step);
    match step {
        "unit" => crate::unit::saved_files(&dash).iter().any(|f| f == l),
        "pois" | "peaks" => l == format!("work/{step}/{dash}"),
        // (A z6 tile's normalized buildings, and its 3D buildings' hi pack.)
        "bldprep" => tile.is_some_and(|t| t.z == 6 && l == crate::bld::work_logical(t.x, t.y)),
        "bldtiles" => tile.is_some_and(|t| {
            t.z == 6
                && (l == crate::bld::pack_logical(t.x, t.y)
                    || l == crate::bld::field::hi_logical(t.x, t.y)
                    || l == crate::bld::field::mid_logical(t.x, t.y))
        }),
        "bldfield-lo" => tile.is_some_and(|t| t.z == 3 && l == crate::bld::field::lo_logical(t.x, t.y)),
        // (A z6 tile's depth hi packs, a z3 tile's lo packs.)
        "depth" => tile.is_some_and(|t| {
            t.z == 6 && DEPTH_LAYERS.iter().any(|ly| l == crate::depth::build::hi_logical(ly, t.x, t.y))
        }),
        "depth-lo" => tile.is_some_and(|t| {
            t.z == 3 && DEPTH_LAYERS.iter().any(|ly| l == crate::depth::build::lo_logical(ly, t.x, t.y))
        }),
        "trees" if tile.is_some_and(|u| u.z == 6) => tile.is_some_and(|t| {
            l == crate::treepacks::mid_logical(t.x, t.y)
                || crate::treepacks::LAYERS.iter().any(|layer| l == format!("layers/{layer}/hi/{dash}"))
        }),
        "trees-lo" => {
            tile.is_some_and(|u| u.z == 3)
                && crate::treepacks::LAYERS.iter().any(|layer| l == format!("layers/{layer}/lo/{dash}"))
        }
        "terrain-hi" if tile.is_some_and(|u| u.z == 6) => tile.is_some_and(|t| {
            l == crate::terrain_pack::mid_logical(t.x, t.y) || l == format!("layers/terrain/hi/{dash}")
        }),
        "slope-hi" if tile.is_some_and(|u| u.z == 6) => tile
            .is_some_and(|t| l == crate::slope_pack::mid_logical(t.x, t.y) || l == format!("layers/slope/hi/{dash}")),
        // (A z10 tile's deep pack, a retired one too; slope's and its deep mid.)
        "terrain-deep" => tile.is_some_and(|t| {
            t.z == crate::terrain_deep::PACK_Z && crate::terrain_deep::pack_tile(l) == Some((t.x, t.y))
        }),
        "slope-deep" => tile.is_some_and(|t| {
            t.z == crate::terrain_deep::PACK_Z
                && (crate::slope_pack::deep_tile(l) == Some((t.x, t.y))
                    || crate::slope_pack::deep_mid_tile(l) == Some((t.x, t.y)))
        }),
        "terrain-lo" | "slope-lo" => {
            tile.is_some_and(|u| u.z == 3) && l == format!("layers/{}/lo/{dash}", step.trim_end_matches("-lo"))
        }
        "terrain-hi" | "slope-hi" | "trees" => {
            let Some(q) = tile.filter(|u| u.z == 3) else { return false };
            let layers: &[&str] = match step {
                "terrain-hi" => &["terrain"],
                "slope-hi" => &["slope"],
                _ => &crate::treepacks::LAYERS,
            };
            layers.iter().any(|layer| {
                l == format!("layers/{layer}/lo/{dash}")
                    || l.strip_prefix(&format!("layers/{layer}/hi/6-"))
                        .and_then(|r| r.split_once('-'))
                        .and_then(|(x, y)| Some((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?)))
                        .is_some_and(|(x, y)| (x >> 3, y >> 3) == (q.x, q.y))
            })
        }
        _ => false,
    }
}

/// Whether manifest `m` names a published file (under `layers/` or `base/`) a job of `step` saves
/// for tile `target` (`saves`' files of it, looked up by name): false for a target recorded built
/// that made nothing to publish (a deep tile with no fine data there: crate::terrain_deep,
/// `PRESENCE`; task #839). None for a step whose files aren't told by name here (a worldwide or
/// root step's, a work step's, a z3 tile's whole run of the scheme before z6 tiles).
pub fn output_named(step: &str, target: &str, m: &std::collections::BTreeMap<String, String>) -> Option<bool> {
    Some(output_names(step, target)?.iter().any(|l| m.contains_key(l)))
}

/// The published files' names (under `layers/` or `base/`) a job of `step` saves for tile `target`,
/// any of which names its output (`output_named`; the build page's map asks the latest catalogue,
/// crate::agent::builtview, task #866). None for a step whose files aren't told by name here.
pub fn output_names(step: &str, target: &str) -> Option<Vec<String>> {
    let t = TileId::parse(target)?;
    let dash = target.replace('/', "-");
    let layers =
        |ls: &[&str], scope: &str| ls.iter().map(|ly| format!("layers/{ly}/{scope}/{dash}")).collect::<Vec<_>>();
    let step = current(step);
    let names: Vec<String> = match (step, t.z) {
        ("unit", 6) => crate::unit::saved_files(&dash)
            .into_iter()
            .filter(|l| l.starts_with("layers/") || l.starts_with("base/"))
            .collect(),
        ("terrain-deep", z) if z == crate::terrain_deep::PACK_Z => {
            vec![crate::terrain_deep::pack_logical(t.x, t.y), format!("{}{dash}", crate::terrain_deep::RETIRED)]
        }
        ("slope-deep", z) if z == crate::terrain_deep::PACK_Z => vec![crate::slope_pack::deep_logical(t.x, t.y)],
        ("terrain-hi", 6) => layers(&["terrain"], "hi"),
        ("slope-hi", 6) => layers(&["slope"], "hi"),
        ("trees", 6) => layers(&crate::treepacks::LAYERS, "hi"),
        ("terrain-lo", 3) => layers(&["terrain"], "lo"),
        ("slope-lo", 3) => layers(&["slope"], "lo"),
        ("trees-lo", 3) => layers(&crate::treepacks::LAYERS, "lo"),
        ("depth", 6) => DEPTH_LAYERS.iter().map(|ly| crate::depth::build::hi_logical(ly, t.x, t.y)).collect(),
        ("depth-lo", 3) => DEPTH_LAYERS.iter().map(|ly| crate::depth::build::lo_logical(ly, t.x, t.y)).collect(),
        ("bldtiles", 6) => vec![crate::bld::pack_logical(t.x, t.y), crate::bld::field::hi_logical(t.x, t.y)],
        ("bldfield-lo", 3) => vec![crate::bld::field::lo_logical(t.x, t.y)],
        _ => return None,
    };
    Some(names)
}

// The write-sets. A name's parts, split at '/'; a tile in a name is `z-x-y` (`tile`), a pass by its
// planet's date (`date`).

/// Whether `s` is tile `z-x-y` of zoom `z` (x and y within it, no leading zeros but "0").
fn tile(s: &str, z: u32) -> bool {
    let mut p = s.split('-');
    let num = |t: Option<&str>| {
        t.filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && (t.len() == 1 || !t.starts_with('0')))
            .and_then(|t| t.parse::<u32>().ok())
    };
    match (num(p.next()), num(p.next()), num(p.next()), p.next()) {
        (Some(tz), Some(x), Some(y), None) => tz == z && (x as u64) < (1u64 << z) && (y as u64) < (1u64 << z),
        _ => false,
    }
}

/// Whether `s` is a day, `YYYY-MM-DD`.
fn date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Whether `l` is a pack of layer `layer` (or of a layer `layer` accepts) at one of `scopes` ("root",
/// "lo", "hi"): `layers/<layer>/root/0-0-0`, `…/lo/3-x-y`, `…/hi/6-x-y`.
fn pack_of(l: &str, layer: &dyn Fn(&str) -> bool, scopes: &[&str]) -> bool {
    let p: Vec<&str> = l.split('/').collect();
    match p.as_slice() {
        ["layers", ly, scope, t] if layer(ly) && scopes.contains(scope) => match *scope {
            "root" => *t == "0-0-0",
            "lo" => tile(t, 3),
            "hi" => tile(t, 6),
            _ => false,
        },
        _ => false,
    }
}

/// `pack_of` for one layer by name.
fn pack(l: &str, layer: &str, scopes: &[&str]) -> bool {
    pack_of(l, &|ly| ly == layer, scopes)
}

const ALL: [&str; 3] = ["root", "lo", "hi"];

fn parts(l: &str) -> Vec<&str> {
    l.split('/').collect()
}

fn w_none(_: &str, _: bool) -> bool {
    false
}

/// The clean-up's: it removes the names removed steps made (crate::agent::gc::RETIRED) and the
/// depth packs no current target names (crate::agent::gc::DepthKeep), and writes nothing.
fn w_gc(l: &str, removal: bool) -> bool {
    removal && (crate::agent::gc::retired(l) || w_depth(l, true) || w_depth_lo(l, true))
}

/// The pass's sources (`sources/osm/<date>/…`: its filtered planet, sets, outlines, extracts, road
/// values, the mark that it's complete) and its basemap; as it completes, the older passes' entries
/// removed (crate::osmpass::retire_older: their sources, items, heritage, summits, route ends).
fn w_osm_pass(l: &str, removal: bool) -> bool {
    match parts(l).as_slice() {
        ["sources", "osm", d, _, ..] => date(d),
        ["layers", "basemap", w] => w.strip_prefix("world-").is_some_and(date),
        ["sources", "items", d, _, ..] | ["work", "heritage", d, _, ..] => removal && date(d),
        ["work", "summits", d] | ["work", "trailends", d] => removal && date(d),
        _ => false,
    }
}

/// The sets a pass lacks in their current filters.
fn w_pass_sets(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "sets", _] if date(d))
}

/// The pass's outlines, written (never removed: the pass retires them).
fn w_outlines(l: &str, removal: bool) -> bool {
    !removal && matches!(parts(l).as_slice(), ["sources", "osm", d, "outlines"] if date(d))
}

fn w_trailends(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "trailends", d] if date(d))
}

fn w_reach(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "reach"] if date(d))
}

/// The world's roadside buildings' index, per Overture release.
fn w_buildings(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "buildings", _, "index"])
}

fn w_summits(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "summits", d] if date(d))
}

/// The labels' packs, every zoom (those it didn't write removed).
fn w_labels(l: &str, _: bool) -> bool {
    pack(l, "labels", &ALL)
}

/// The pass's lakes filled from Canada's own data (the other passes' removed).
fn w_lakefill(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "lakefill", d] if date(d))
}

/// The water's packs, every zoom (those it didn't write removed, and the small water's before it,
/// which it replaced).
fn w_water(l: &str, removal: bool) -> bool {
    pack(l, "water", &ALL) || (removal && pack(l, "smallwater", &ALL))
}

/// The heritage sites by z6 tile, their positions and areas (those of the pass it didn't write
/// removed), and the base files the chain reads.
fn w_heritage_sites(l: &str, _: bool) -> bool {
    match parts(l).as_slice() {
        ["work", "heritage", d, "pos" | "areas" | "parts", t] => date(d) && tile(t, 6),
        ["work", "heritage", d, "base", _] => date(d),
        _ => false,
    }
}

/// A gate unit's accepted index, its held report and its check's state (crate::inputs::record_of:
/// `sources/inputs/<unit>/@index`, `…/@held`, `…/@listed`, the unit's name nested or not), the
/// report removed once nothing's held.
fn w_inputs(l: &str, _: bool) -> bool {
    crate::inputs::record_of(l).is_some()
}

fn w_regions_land(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "regions-land"] if crate::osmpass::is_date(d))
}

fn w_regions_extract(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "osm", d, "regions-extract"] if crate::osmpass::is_date(d))
}

fn w_spoken(l: &str, _: bool) -> bool {
    l == "global/spoken"
}

/// A z6 tile's hi pack and mid (a z6 tile); a z3 tile's whole run's lo pack and its z6 tiles' hi
/// packs.
fn w_terrain_hi(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["lo", "hi"]) || matches!(parts(l).as_slice(), ["work", "terrain-mid", t] if tile(t, 6))
}

/// A z10 tile's deep pack (crate::terrain_deep), and the retired fine pack there it drops.
fn w_terrain_deep(l: &str, _: bool) -> bool {
    crate::terrain_deep::pack_tile(l).is_some()
}

fn w_terrain_lo(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["lo"])
}

/// A z6 tile's depth, class and shading hi packs (removed where it makes none).
fn w_depth(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| DEPTH_LAYERS.contains(&ly), &["hi"])
}

/// A z3 tile's depth, class and shading lo packs.
fn w_depth_lo(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| DEPTH_LAYERS.contains(&ly), &["lo"])
}

fn w_depth_root(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| DEPTH_LAYERS.contains(&ly), &["root"])
}

/// The depth steps' layers: the depth, its classes and the floor's shading (crate::depth).
const DEPTH_LAYERS: [&str; 3] = [crate::depth::LAYER, crate::depth::CLASS_LAYER, crate::depth::SHADE_LAYER];

/// The water's index for the terrain's repairs, by the content it's made of.
fn w_terrain_water(l: &str, _: bool) -> bool {
    l.strip_prefix(crate::terrain_water::IDX_PREFIX).is_some_and(|k| !k.is_empty() && !k.contains('/'))
}

fn w_terrain_root(l: &str, _: bool) -> bool {
    pack(l, "terrain", &["root"])
}

/// A z6 tile's hi pack and mid (a z6 tile); a z3 tile's whole run's lo pack and its z6 tiles' hi
/// packs.
fn w_slope_hi(l: &str, _: bool) -> bool {
    pack(l, "slope", &["lo", "hi"]) || matches!(parts(l).as_slice(), ["work", "slope-mid", t] if tile(t, 6))
}

/// A z10 tile's deep pack and deep mid (crate::slope_pack::build_deep).
fn w_slope_deep(l: &str, _: bool) -> bool {
    crate::slope_pack::deep_tile(l).is_some() || crate::slope_pack::deep_mid_tile(l).is_some()
}

fn w_slope_lo(l: &str, _: bool) -> bool {
    pack(l, "slope", &["lo"])
}

fn w_slope_root(l: &str, _: bool) -> bool {
    pack(l, "slope", &["root"])
}

/// A polar layer's (`kind`) block packs (`layers/polar-<kind>-<pole>/polar/4-x-y`) or root.
fn polar_pack(l: &str, kind: &str, scope: &str) -> bool {
    crate::polar::POLES.iter().any(|&p| match parts(l).as_slice() {
        ["layers", ly, sc, t] => {
            *ly == crate::polar::layer(kind, p)
                && *sc == scope
                && (if scope == "root" { *t == "0-0-0" } else { tile(t, u32::from(crate::polar::BLOCK_Z)) })
        }
        _ => false,
    })
}

fn w_polar_terrain(l: &str, _: bool) -> bool {
    polar_pack(l, "terrain", "polar")
}

fn w_polar_slope(l: &str, _: bool) -> bool {
    polar_pack(l, "slope", "polar")
}

fn w_polar_root(l: &str, _: bool) -> bool {
    polar_pack(l, "terrain", "root") || polar_pack(l, "slope", "root")
}

/// The tree layers' hi packs and a z6 tile's mid (a z6 tile); a z3 tile's whole run their lo packs
/// too.
fn w_trees(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| crate::treepacks::LAYERS.contains(&ly), &["lo", "hi"])
        || matches!(parts(l).as_slice(), ["work", "trees-mid", t] if tile(t, 6))
}

fn w_trees_lo(l: &str, _: bool) -> bool {
    pack_of(l, &|ly| crate::treepacks::LAYERS.contains(&ly), &["lo"])
}

/// A z6 tile's files (crate::unit::saved_files), of a z6 tile.
fn w_unit(l: &str, _: bool) -> bool {
    l.rsplit_once('/').is_some_and(|(_, t)| tile(t, 6) && crate::unit::saved_files(t).iter().any(|f| f == l))
}

fn w_pois(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "pois", t] if tile(t, 6))
}

fn w_peaks(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "peaks", t] if tile(t, 6))
}

/// A z6 tile's map tiles: its roads' and rails' hi packs and its data.
fn w_pack(l: &str, _: bool) -> bool {
    pack(l, "roads", &["hi"]) || pack(l, "rails", &["hi"]) || matches!(parts(l).as_slice(), ["hidata", t] if tile(t, 6))
}

fn w_lo(l: &str, _: bool) -> bool {
    pack(l, "roads", &["lo"]) || pack(l, "rails", &["lo"])
}

/// The landmarks' Wikidata facts, pageviews and their meta, for the pass.
fn w_items(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "items", d, "facts" | "views" | "meta"] if date(d))
}

/// The heritage chain's files, at the top of the pass's heritage folder (those it didn't write
/// removed).
fn w_heritage(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "heritage", d, stem] if date(d) && !stem.is_empty())
}

/// The landmarks drawn: each kind's zoomed-out tiles as a bundle, their data by z6 tile as one
/// bundle (task #449; the packs and files per tile from before, removed), the heritage dots and the
/// summary (what it didn't write removed).
fn w_marks(l: &str, _: bool) -> bool {
    let kind = |ly: &str| ly.strip_prefix("marks-").is_some_and(|k| !k.is_empty());
    pack_of(l, &kind, &["root", "lo"])
        || matches!(parts(l).as_slice(), ["layers", ly, "all"] if kind(ly))
        || matches!(parts(l).as_slice(), ["markdata", t] if tile(t, 6))
        || l == crate::marks::MARKDATA_BUNDLE
        || l == crate::markconv::HERITAGE_DOTS
        || l == "global/marks/summary"
}

/// The area overlays' tiles, every zoom, a bundle per layer (task #449; the packs from before,
/// removed), their data by z3 tile, and the heritage summaries (what it didn't write removed).
fn w_overlays(l: &str, _: bool) -> bool {
    let layer = |ly: &str| ["ov-heritage-areas", "ov-indigenous", "ov-special", "ov-whs"].contains(&ly);
    pack_of(l, &layer, &ALL)
        || matches!(parts(l).as_slice(), ["layers", ly, "all"] if layer(ly))
        || matches!(parts(l).as_slice(), ["ovdata", t] if tile(t, 3))
        || matches!(parts(l).as_slice(), ["global", "heritage", s] if !s.is_empty())
}

fn w_roadunits(l: &str, _: bool) -> bool {
    l == "global/roadunits"
}

fn w_stations(l: &str, _: bool) -> bool {
    pack(l, "stations", &ALL)
}

fn w_ferries(l: &str, _: bool) -> bool {
    pack(l, "ferries", &ALL) || l == crate::timetables::FERRY_LINES
}

/// The timetable feeds: each feed's zip, the lists of those checked, fetched and kept (the rail
/// feeds' and the ferry feeds'), and the catalogue when it was fetched.
fn w_rail_feeds(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["sources", "rail", "checked" | "fetched" | "feeds" | "ferry-feeds" | "catalogue"])
        || matches!(parts(l).as_slice(), ["sources", "rail", "gtfs", id] if !id.is_empty())
}

fn w_rail(l: &str, _: bool) -> bool {
    l == "work/rail/used" || l == "global/railfreq" || l == crate::timetables::RAIL_LINES
}

/// The timetables' report (its to-do list is written into the inputs folder's todo/, outside the
/// records).
fn w_timetables_todo(l: &str, _: bool) -> bool {
    l == crate::timetables::REPORT
}

fn w_bldprep(l: &str, _: bool) -> bool {
    matches!(parts(l).as_slice(), ["work", "bld", t] if tile(t, 6))
}

/// The 3D buildings' hi packs, their field's and its mids.
fn w_bldtiles(l: &str, _: bool) -> bool {
    pack(l, crate::bld::LAYER, &["hi"])
        || pack(l, crate::bld::field::LAYER, &["hi"])
        || matches!(parts(l).as_slice(), ["work", "bldfield-mid", t] if tile(t, 6))
}

fn w_bldfield_lo(l: &str, _: bool) -> bool {
    pack(l, crate::bld::field::LAYER, &["lo"])
}

/// What the regions no longer cover, removed: a z6 tile's files, candidates and peaks, map tiles, the
/// 3D buildings'. Nothing written.
fn w_prune(l: &str, removal: bool) -> bool {
    removal
        && (w_unit(l, false)
            || w_pois(l, false)
            || w_peaks(l, false)
            || w_pack(l, false)
            || w_lo(l, false)
            || w_bldprep(l, false)
            || w_bldtiles(l, false)
            || w_bldfield_lo(l, false))
}

#[cfg(test)]
mod tests {
    /// A pole's root waits for its own pole's blocks only, both of their steps; a block's slope for
    /// its own and its neighbours' terrain (task #879).
    #[test]
    fn a_poles_root_waits_for_its_own_blocks() {
        use super::{read_targets, readiness, Left, ReadTarget, Ready};
        use roadcore::polar::Pole;
        let reads = read_targets("polar-root", "s");
        let south = crate::polar::blocks(Pole::South);
        assert_eq!(reads.len(), 2 * south.len());
        assert!(reads.iter().all(|r| matches!(r, ReadTarget::One(_, t) if t.starts_with("s/"))), "{reads:?}");
        let mut left = Left::default();
        left.set("polar-terrain", [crate::polar::target(Pole::North, crate::polar::blocks(Pole::North)[0])]);
        assert_eq!(readiness("polar-root", "s", &left), Ready::Yes, "the other pole's block");
        let b = crate::polar::target(Pole::South, south[0]);
        left.set("polar-slope", [b.clone()]);
        assert_eq!(readiness("polar-root", "s", &left), Ready::Waits(vec![("polar-slope", b)]));
    }

    /// Task #701: no step makes a removed step's names (crate::agent::gc::RETIRED); the clean-up
    /// removes them, and nothing else.
    #[test]
    fn no_step_makes_a_retired_name() {
        for p in crate::agent::gc::RETIRED {
            for l in [format!("{p}v1"), format!("{p}v3-max")] {
                assert!(!made(&l), "{l}");
                assert_eq!(writes("gc", &l, true), Some(true));
            }
        }
        assert_eq!(writes("gc", "base/6-1-1", true), Some(false));
        // (Task #52: the legacy folder only, as removals.)
        assert_eq!(writes("gc", "global/legacy/ferries", true), Some(true));
        assert_eq!(writes("gc", "global/legacy/ferries", false), Some(false));
        for l in ["global/railfreq", "global/legacyx/ferries", "global/heritage/layer-summary"] {
            assert_eq!(writes("gc", l, true), Some(false), "{l}");
        }
    }

    /// Task #679: the clean-up removes the depth's packs (a z6 tile's hi, a z3 tile's lo), never
    /// writes one, and removes no other layer's.
    #[test]
    fn the_clean_up_removes_depth_packs_only() {
        for l in [
            "layers/depth/hi/6-1-33",
            "layers/depth-class/hi/6-1-33",
            "layers/depth-shade/hi/6-1-33",
            "layers/depth/lo/3-0-4",
            "layers/depth-shade/lo/3-0-4",
        ] {
            assert_eq!(writes("gc", l, true), Some(true), "{l}");
            assert_eq!(writes("gc", l, false), Some(false), "{l}");
        }
        for l in ["layers/terrain/hi/6-1-33", "layers/slope/lo/3-0-4", "hidata/6-1-33"] {
            assert_eq!(writes("gc", l, true), Some(false), "{l}");
        }
    }

    use super::*;
    use crate::handoff::Handoff;
    use crate::pool::journal::{Entry, LeaseId};

    /// The worker page's schedule colours each stage from STAGE_COLOURS (web/work/dash.js), falling
    /// back to upkeep's grey: every stage has a colour there, and no two stages share one.
    #[test]
    fn every_stage_has_a_colour_of_its_own_on_the_worker_page() {
        // index.html sets each stage's --st-<id> once per theme: the first of each is the dark theme's, the second the light's.
        let html = include_str!("../../../../web/work/index.html");
        let mut themes: [std::collections::BTreeMap<&str, String>; 2] = Default::default();
        for decl in html.split(';') {
            let Some(at) = decl.find("--st-") else { continue };
            let (k, v) = decl[at + 5..].split_once(':').expect("--st-<id>: colour");
            let theme = if themes[0].contains_key(k.trim()) { 1 } else { 0 };
            themes[theme].insert(k.trim(), v.trim().to_ascii_lowercase());
        }
        for colours in &themes {
            for s in &STAGES {
                assert!(colours.contains_key(s.id), "stage {} has no --st-{} in index.html", s.id, s.id);
            }
            let mut seen = std::collections::BTreeMap::new();
            for (k, v) in colours {
                if let Some(other) = seen.insert(v, k) {
                    panic!("stages {other} and {k} share the colour {v} in index.html");
                }
            }
        }
    }

    #[test]
    fn made_is_what_todays_steps_write() {
        // The worldwide files the build makes (each family the NAS's manifest holds today).
        for l in [
            "global/roads/6-28-16",
            "global/roadids/6-28-16",
            "global/roaden/6-28-16",
            "global/heritage/layer-summary",
            "global/heritage/heritage-sources",
            "global/marks/summary",
            "global/railfreq",
            "global/roadunits",
            "global/spoken",
        ] {
            assert!(made(l), "{l}");
        }
        // Names no step makes under global/: not today's outputs.
        for l in ["global/pois", "global/other/x"] {
            assert!(!made(l), "{l}");
        }
    }

    #[test]
    fn the_sets_are_todays() {
        // (As they were before the table: the agent's and the coordinator's rules unchanged; their
        // order is the table's, no preference, task #909.)
        let set = |v: &[&'static str]| v.iter().copied().collect::<std::collections::BTreeSet<&str>>();
        assert_eq!(
            set(&SHARED),
            set(&[
                "terrain-hi",
                "terrain-deep",
                "slope-deep",
                "slope-hi",
                "trees",
                "unit",
                "pois",
                "peaks",
                "bldprep",
                "bldtiles",
                "depth"
            ])
        );
        assert_eq!(set(&ANSWERED), set(&["items", "heritage-sites", "heritage"]));
    }

    #[test]
    fn steps_read_together_are_known_from_the_reads() {
        // (Task #909: slope-deep reads terrain-deep's 3×3, slope-hi terrain-hi's and slope-deep's,
        // an assembly its z6 tiles.)
        for s in ["terrain-deep", "terrain-hi", "terrain-lo", "slope-deep", "slope-hi", "trees"] {
            assert!(read_together(s), "{s}");
        }
        assert!(!read_together("catalog"));
        // A run of four targets in block order is a 2×2 square.
        let mut ts: Vec<String> = (0..4).flat_map(|y| (0..4).map(move |x| format!("10/{x}/{y}"))).collect();
        ts.sort_by_key(|t| block_order(t));
        assert_eq!(ts[..4], ["10/0/0", "10/1/0", "10/0/1", "10/1/1"]);
        assert_eq!(block_order("s/x"), None);
    }

    #[test]
    fn readers_go_before_what_they_read() {
        // (Task #909: the owner's case, a ready slope-deep job before terrain-deep's; slope-hi
        // before both; steps the reads don't relate in the order given.)
        assert!(reads_from("slope-deep", "terrain-deep"));
        assert!(reads_from("slope-lo", "terrain-hi"), "through terrain-lo");
        assert!(!reads_from("terrain-deep", "slope-deep"));
        let v = vec!["terrain-deep", "trees", "slope-deep", "slope-hi"];
        assert_eq!(readers_first(v, |s| s), ["slope-hi", "slope-deep", "terrain-deep", "trees"]);
        let v = vec!["trees", "terrain-hi", "pois"];
        assert_eq!(readers_first(v.clone(), |s| s), v);
    }

    #[test]
    fn the_rows_are_todays() {
        // The memory first guessed (what the agent's first_peak and second_peak gave, the latter's
        // where they differed: the network steps', a z6 tile's and candidates', which nothing offered
        // by the former).
        let mem: [(&str, u64); 22] = [
            ("lakefill", 12288),
            ("outlines", 9216),
            ("heritage", 6144),
            ("rail", 6144),
            ("items", 3072),
            ("rail-feeds", 5632),
            ("buildings", 2600),
            ("bld-fetch", 5632),
            ("marks", 4096),
            ("overlays", 4096),
            ("unit", 8600),
            ("pois", 2048),
            ("trees", 2500),
            ("trees-lo", 500),
            ("terrain-lo", 1200),
            ("slope", crate::slope_pack::peak_mb()),
            ("slope-lo", crate::slope_pack::lo_peak_mb()),
            ("depth-lo", crate::depth::build::lo_peak_mb()),
            ("peaks", 2500),
            ("water", 8192),
            ("bldprep", super::super::bldprep_peak(30_300_000)),
            ("bldtiles", super::super::bldtiles_peak(30_300_000)),
        ];
        for (s, mb) in mem {
            assert_eq!(mem_mb(s), mb, "{s}");
        }
        assert_eq!(mem_mb("labels"), 3072);
        assert_eq!(mem_mb("not a step"), 1500);
        // No row twice.
        for s in TABLE.iter().map(|s| s.name) {
            assert_eq!(TABLE.iter().filter(|r| r.name == s).count(), 1, "{s}");
        }
    }

    #[test]
    fn a_shared_steps_saves_are_in_its_write_set() {
        for (step, t) in [
            ("unit", "6/3/4"),
            ("pois", "6/3/4"),
            ("peaks", "6/3/4"),
            ("bldprep", "6/3/4"),
            ("bldtiles", "6/3/4"),
            ("trees", "6/3/4"),
            ("trees", "3/1/1"),
            ("trees-lo", "3/1/1"),
            ("terrain-hi", "6/3/4"),
            ("terrain-hi", "3/1/1"),
            ("slope-hi", "6/3/4"),
            ("slope-hi", "3/1/1"),
            ("terrain-lo", "3/1/1"),
            ("slope-lo", "3/1/1"),
            // (An older app's names, read as the new ones'.)
            ("terrain", "6/3/4"),
            ("slope", "3/1/1"),
            ("terrain-deep", "10/3/4"),
            ("slope-deep", "10/3/4"),
        ] {
            let dash = t.replace('/', "-");
            let mut names: Vec<String> = Vec::new();
            for l in [
                "base",
                "global/roads",
                "global/roaden",
                "layers/grid-class/hi",
                "layers/grid-canopy/hi",
                "layers/grid-cover/hi",
                "work/pois",
                "work/peaks",
                "work/bld",
                "layers/buildings/hi",
                "work/trees-mid",
                "work/terrain-mid",
                "work/slope-mid",
                "layers/terrain/hi",
                "layers/terrain/lo",
                "layers/slope/hi",
                "layers/slope/lo",
                "layers/trees-cover/hi",
                "layers/trees-leaf/lo",
                "layers/terrain/deep",
                "layers/terrain/fine",
                "layers/slope/deep",
                "work/slope-deep-mid",
            ] {
                names.push(format!("{l}/{dash}"));
                names.push(format!("{l}/6-8-9"));
            }
            let saved: Vec<&String> = names.iter().filter(|l| saves(step, t, l)).collect();
            assert!(!saved.is_empty(), "{step} {t} saves something");
            for l in saved {
                assert!((row(step).unwrap().writes)(l, false), "{step} {t}: {l}");
            }
        }
        assert!(!saves("pack", "6/3/4", "hidata/6-3-4"), "not a shared step");
    }

    #[test]
    fn a_targets_published_output_is_one_it_saves() {
        use std::collections::BTreeMap;
        // Each name output_named looks for is one `saves` counts as the target's (one mechanism),
        // and a target with none of them named made nothing to publish.
        for (step, t) in [
            ("unit", "6/3/4"),
            ("terrain-deep", "10/3/4"),
            ("slope-deep", "10/3/4"),
            ("terrain-hi", "6/3/4"),
            ("slope-hi", "6/3/4"),
            ("trees", "6/3/4"),
            ("terrain-lo", "3/3/4"),
            ("slope-lo", "3/3/4"),
            ("trees-lo", "3/3/4"),
            ("depth", "6/3/4"),
            ("depth-lo", "3/3/4"),
            ("bldtiles", "6/3/4"),
            ("bldfield-lo", "3/3/4"),
        ] {
            assert_eq!(output_named(step, t, &BTreeMap::new()), Some(false), "{step} {t}: nothing named");
            // (Every published name `saves` has for it, among the shapes the steps write.)
            let dash = t.replace('/', "-");
            let (x, y) = (3, 4);
            let mut cands: Vec<String> = crate::unit::saved_files(&dash).to_vec();
            for ly in ["terrain", "slope", "trees-cover", "trees-leaf", "trees-height", "buildings", "buildings-field"]
            {
                for sc in ["hi", "lo", "deep", "fine"] {
                    cands.push(format!("layers/{ly}/{sc}/{dash}"));
                }
            }
            cands.extend(
                DEPTH_LAYERS.iter().flat_map(|ly| {
                    [crate::depth::build::hi_logical(ly, x, y), crate::depth::build::lo_logical(ly, x, y)]
                }),
            );
            cands.extend([
                crate::bld::pack_logical(x, y),
                crate::bld::field::hi_logical(x, y),
                crate::bld::field::lo_logical(x, y),
            ]);
            let mut found = false;
            for l in cands.iter().filter(|l| saves(step, t, l) && (l.starts_with("layers/") || l.starts_with("base/")))
            {
                let m: BTreeMap<String, String> = [(l.clone(), "f".into())].into();
                assert_eq!(output_named(step, t, &m), Some(true), "{step} {t}: {l}");
                found = true;
            }
            assert!(found, "{step} {t}: a published name");
            let other: BTreeMap<String, String> = [("layers/terrain/deep/10-9-9".into(), "f".into())].into();
            assert_eq!(output_named(step, t, &other), Some(false), "{step} {t}: another tile's");
        }
        assert_eq!(output_named("pois", "6/3/4", &BTreeMap::new()), None, "a work step's: not told");
        assert_eq!(output_named("terrain-hi", "3/3/4", &BTreeMap::new()), None, "a z3 whole run: not told");
    }

    #[test]
    fn every_step_the_agent_runs_has_a_row() {
        // The steps the agent makes jobs of, as its source names them: the planner's work
        // (`Work { step: "…"`), the jobs it titles (`job(format!("…`, `id: "…"`), the steps the
        // checklist counts (build::COUNTED), and those whose names it puts together.
        // (Each source with its trimmed lines joined, as rustfmt breaks a call across lines.)
        let flat = |s: &str| s.lines().map(str::trim).collect::<String>();
        // (The agent's: its round's and its plan's, agent::plan.)
        let agent = flat(include_str!("mod.rs")) + &flat(include_str!("plan.rs"));
        let build = flat(include_str!("build.rs"));
        let (agent, build) = (agent.as_str(), build.as_str());
        let mut named = std::collections::BTreeSet::<String>::new();
        let mut take = |src: &str, pat: &str| {
            for (i, _) in src.match_indices(pat) {
                let name: String = src[i + pat.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                    .collect();
                if name.len() > 1 {
                    named.insert(name);
                }
            }
        };
        take(build, "Work { step: \"");
        take(agent, "Work { step: \"");
        take(agent, "job(format!(\"");
        take(agent, "id: \"");
        take(agent, "id: format!(\"");
        named.extend(super::super::build::COUNTED.iter().map(|s| s.to_string()));
        named.extend(
            [
                "stations",
                "ferries",
                "terrain-lo",
                "slope-lo",
                "terrain-root",
                "slope-root",
                "polar-terrain",
                "polar-slope",
                "polar-root",
                "unit",
                "osm-pass",
                "regions-extract",
                // (The owner's redos: `redo::job_id`.)
                "redo",
            ]
            .map(String::from),
        );
        // (Not steps: a task's lease.)
        named.remove("task");
        assert!(named.len() >= 40, "{named:?}");
        let missing: Vec<&String> = named.iter().filter(|s| row(s).is_none()).collect();
        assert!(missing.is_empty(), "steps the agent runs with no row: {missing:?} (of {named:?})");
        // And no row for a step the agent never runs.
        let unused: Vec<&str> = TABLE.iter().map(|s| s.name).filter(|s| !named.contains(*s)).collect();
        assert!(unused.is_empty(), "rows of no step the agent runs: {unused:?}");
    }

    #[test]
    fn every_row_is_described_alike() {
        // A sentence each, ids unique, every stage known and used, every task's step a row; the
        // nouns by scope; a worldwide step is one run.
        let mut ids = std::collections::BTreeSet::new();
        for s in TABLE.iter() {
            assert!(s.does.ends_with('.') && s.does.len() > 20, "{}: {:?}", s.name, s.does);
            assert!(ids.insert(s.name), "{} twice", s.name);
            assert!(stage(s.stage).is_some(), "{}: stage {}", s.name, s.stage);
            if !super::super::every::names_target(s.name) {
                assert_eq!(s.scope, World, "{}: a step whose target isn't named is worldwide", s.name);
            }
        }
        for st in STAGES {
            assert!(TABLE.iter().any(|s| s.stage == st.id), "stage {} has no step", st.id);
        }
        for t in TASKS {
            assert!(row(t.step).is_some(), "{}: step {}", t.name, t.step);
            assert_eq!(stage_of(t.name).map(|g| g.id), Some(row(t.step).unwrap().stage));
        }
        assert_eq!(
            (noun("unit"), noun("terrain-lo"), noun("outlines"), noun("inputs"), noun("tail"), noun("a newer step")),
            ("z6 tile", "z3 tile", "run", "input", "task", "target")
        );
        assert_eq!(said("terrain", 8), "terrain-hi: 8 z6 tiles", "an old name said by its new");
        assert_eq!(said("terrain-lo", 1), "terrain-lo: 1 z3 tile");
        assert_eq!(said("outlines", 1), "outlines");
        assert_eq!(stage_of("publishing"), None, "no lane of its own: publishing goes per unit");
        // The place, from the pool's rules: the shared steps and the map tiles any member's, the
        // lead's own and the worldwide steps the lead's.
        for s in ["terrain-hi", "terrain-deep", "slope-deep", "slope-hi", "unit", "pack", "lo", "bldprep"] {
            assert_eq!(place(s), Place::Any, "{s}");
        }
        for s in ["catalog", "osm-pass", "items", "marks", "rail", "terrain-root", "gc", "inputs", "pyramids-fill"] {
            assert_eq!(place(s), Place::Lead, "{s}");
        }
        // The view: a row per step then per task, serializable, the task's step named.
        let v = table();
        assert_eq!(v.len(), TABLE.len() + TASKS.len());
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j[0]["id"], "osm-pass");
        assert_eq!(j[0]["scope"], "world");
        assert_eq!(j[0]["place"], "lead");
        let tail = v.iter().find(|i| i.id == "tail").unwrap();
        assert_eq!((tail.of.as_deref(), tail.stage.as_str(), tail.noun.as_str()), (Some("unit"), "roads", "task"));
    }

    #[test]
    fn the_ids_are_the_stored_names() {
        // The ids are what the records, journal, timings, leases and floors hold (docs/pipeline.md
        // §3): a change here would call every target stale, so none is made. The table's, in its
        // order, and the tasks'.
        let ids: Vec<&str> = TABLE.iter().map(|s| s.name).collect();
        assert_eq!(
            ids,
            [
                "osm-pass",
                "pass-sets",
                "outlines",
                "trailends",
                "reach",
                "buildings",
                "summits",
                "labels",
                "lakefill",
                "water",
                "spoken",
                "regions-extract",
                "regions-land",
                "heritage-sites",
                "terrain-water",
                "pyramids-fill",
                "terrain-hi",
                "terrain-deep",
                "terrain-lo",
                "slope-deep",
                "slope-hi",
                "slope-lo",
                "polar-terrain",
                "polar-slope",
                "polar-root",
                "trees",
                "trees-lo",
                "depth",
                "depth-lo",
                "depth-root",
                "unit",
                "pack",
                "lo",
                "prune",
                "roadunits",
                "stations",
                "ferries",
                "terrain-root",
                "slope-root",
                "rail-feeds",
                "rail",
                "pois",
                "peaks",
                "items",
                "heritage",
                "marks",
                "overlays",
                "bld-fetch",
                "bldprep",
                "bldtiles",
                "bldfield-lo",
                "catalog",
                "catalog-held",
                "inputs",
                "names-todo",
                "timetables-todo",
                "gc",
                "redo",
                "backup"
            ]
        );
        assert_eq!(TASKS.map(|t| t.name), ["tail", "terrainsub", "treeblock", "bldtile"]);
    }

    fn entry(step: &str, changes: &[(&str, bool)]) -> Entry {
        let mut h = Handoff::default();
        for (l, removed) in changes {
            h.changes.insert(l.to_string(), (!removed).then(|| format!("{l}.0123456789abcdef.bin")));
        }
        Entry {
            member: "m-0000000000000001".into(),
            lease: LeaseId { term: 1, n: 1 },
            step: step.into(),
            handoff: h,
            at: 1_791_500_000,
        }
    }

    #[test]
    fn entries_within_their_write_sets() {
        let ok: [(&str, &[(&str, bool)]); 29] = [
            ("lakefill", &[("work/lakefill/2026-09-28", false), ("work/lakefill/2026-03-01", true)]),
            (
                "inputs",
                &[
                    ("sources/inputs/_gate-test/@index", false),
                    ("sources/inputs/_gate-test/@held", true),
                    ("sources/inputs/_gate-test/@listed", false),
                    ("sources/inputs/_gate-nest/inner/@index", false),
                    ("sources/inputs/_gate-nest/inner/@listed", false),
                ],
            ),
            (
                "osm-pass",
                &[
                    ("sources/osm/2026-09-28/pieces/6-1-2", false),
                    ("sources/osm/2026-09-28/pass", false),
                    ("layers/basemap/world-2026-09-28", false),
                    ("sources/osm/2026-08-01/sets/hikes", true),
                    ("work/heritage/2026-08-01/base/heritage", true),
                    ("work/summits/2026-08-01", true),
                ],
            ),
            ("pass-sets", &[("sources/osm/2026-09-28/sets/summits-v2", false)]),
            ("outlines", &[("sources/osm/2026-09-28/outlines", false)]),
            (
                "water",
                &[
                    ("layers/water/root/0-0-0", false),
                    ("layers/water/lo/3-1-2", false),
                    ("layers/water/hi/6-1-2", false),
                    ("layers/smallwater/hi/6-1-2", true),
                ],
            ),
            ("labels", &[("layers/labels/hi/6-1-2", false), ("layers/labels/root/0-0-0", true)]),
            ("spoken", &[("global/spoken", false)]),
            ("regions-extract", &[("sources/osm/2026-09-28/regions-extract", false)]),
            ("regions-land", &[("sources/osm/2026-09-28/regions-land", false)]),
            (
                "heritage-sites",
                &[
                    ("work/heritage/2026-09-28/pos/6-1-2", false),
                    ("work/heritage/2026-09-28/base/area-shapes", false),
                    ("work/heritage/2026-09-28/areas/6-3-3", true),
                ],
            ),
            (
                "terrain",
                &[
                    ("layers/terrain/hi/6-10-20", false),
                    ("work/terrain-mid/6-10-20", false),
                    ("layers/terrain/lo/3-1-2", false),
                ],
            ),
            ("terrain-lo", &[("layers/terrain/lo/3-1-2", false)]),
            ("terrain-water", &[("work/water-idx/0f1a2b3c4d5e6f70", false)]),
            ("slope-root", &[("layers/slope/root/0-0-0", false)]),
            ("trees", &[("layers/trees-cover/hi/6-1-2", false), ("work/trees-mid/6-1-2", true)]),
            ("unit", &[("base/6-1-2", false), ("global/roaden/6-1-2", true), ("layers/grid-canopy/hi/6-1-2", false)]),
            ("pack", &[("hidata/6-1-2", false), ("layers/roads/hi/6-1-2", false), ("layers/rails/hi/6-1-2", true)]),
            ("lo", &[("layers/roads/lo/3-1-2", false)]),
            ("items", &[("sources/items/2026-09-28/facts", false)]),
            (
                "heritage",
                &[("work/heritage/2026-09-28/details-harea", false), ("work/heritage/2026-09-28/layer-whs", true)],
            ),
            (
                "marks",
                &[
                    ("layers/marks-peak/lo/3-1-2", true),
                    ("layers/marks-peak/all", false),
                    ("markdata/6-1-2", true),
                    ("markdata/all", false),
                    ("global/marks/summary", false),
                    ("work/marks/heritage-dots", false),
                ],
            ),
            (
                "overlays",
                &[
                    ("layers/ov-whs/hi/6-1-2", true),
                    ("layers/ov-whs/all", false),
                    ("ovdata/3-1-2", false),
                    ("global/heritage/layer-summary", false),
                ],
            ),
            (
                "rail-feeds",
                &[
                    ("sources/rail/gtfs/de-db", false),
                    ("sources/rail/feeds", false),
                    ("sources/rail/ferry-feeds", false),
                    ("sources/rail/catalogue", false),
                ],
            ),
            ("rail", &[("global/railfreq", false)]),
            ("bldprep", &[("work/bld/6-56-25", true)]),
            ("bldtiles", &[("layers/buildings/hi/6-56-25", false)]),
            ("prune", &[("base/6-1-2", true), ("layers/roads/lo/3-1-2", true)]),
            ("catalog", &[]),
        ];
        for (step, ch) in ok {
            assert_eq!(outside(&entry(step, ch)), None, "{step}");
        }
        // Outside: another step's files, a name of the wrong zoom, another layer, a pass's files
        // removed by a step that doesn't retire passes, a prune that writes, a removal the step
        // doesn't make.
        let bad: [(&str, &[(&str, bool)]); 14] = [
            // (Bundles of another step's layer, task #449.)
            ("marks", &[("layers/ov-whs/all", false)]),
            ("overlays", &[("layers/stations/all", false)]),
            ("lakefill", &[("work/lakefill", false), ("work/lakefill/x", false)]),
            (
                "inputs",
                &[
                    ("sources/inputs/_gate-test/a", false),
                    ("sources/inputs/_gate-test/index", false),
                    ("sources/inputs/x/y/@other", false),
                ],
            ),
            ("pack", &[("hidata/6-1-2", false), ("layers/terrain/hi/6-1-2", false)]),
            ("terrain", &[("work/terrain-mid/3-1-2", false)]),
            ("lo", &[("layers/roads/lo/6-1-2", false)]),
            ("labels", &[("layers/stations/hi/6-1-2", false)]),
            ("items", &[("sources/items/2026-08-01/facts", false), ("work/summits/2026-08-01", true)]),
            ("prune", &[("base/6-1-2", false)]),
            (
                "outlines",
                &[
                    ("sources/osm/2026-09-28/sets/outlines", false),
                    ("sources/osm/2026-09-28/pieces/6-1-2", false),
                    ("sources/osm/2026-08-01/outlines", true),
                ],
            ),
            ("water", &[("layers/smallwater/hi/6-1-2", false)]),
            ("catalog", &[("catalog/1", false)]),
            ("unit", &[("base/6-64-2", false)]),
        ];
        for (step, ch) in bad {
            assert!(outside(&entry(step, ch)).is_some(), "{step}: {ch:?}");
        }
        let why =
            outside(&entry("pack", &[("hidata/6-1-2", false), ("layers/terrain/hi/6-1-2", false), ("work/x", true)]))
                .unwrap();
        assert!(why.contains("layers/terrain/hi/6-1-2") && why.contains("and 1 more"), "{why}");
        // A step the table doesn't know: outside, if it changes anything.
        assert!(outside(&entry("newer-step", &[("x/y", false)])).unwrap().contains("isn't a step"));
        assert_eq!(outside(&entry("newer-step", &[])), None);
    }

    /// Every entry of a copy of the NAS's journal (`SCENIC_JOURNAL`: a folder of its day folders)
    /// within its step's write-set: `SCENIC_JOURNAL=… cargo test -p pipeline real_journal --
    /// --ignored`.
    #[test]
    #[ignore]
    fn real_journal_entries_are_within_their_write_sets() {
        let Some(dir) = std::env::var_os("SCENIC_JOURNAL") else { panic!("SCENIC_JOURNAL names no folder") };
        let (mut n, mut bad) = (0, Vec::new());
        let mut steps = std::collections::BTreeMap::<String, usize>::new();
        for day in std::fs::read_dir(&dir).unwrap().flatten() {
            for f in std::fs::read_dir(day.path()).into_iter().flatten().flatten() {
                let Ok(b) = std::fs::read(f.path()) else { continue };
                let Ok(e) = serde_json::from_slice::<Entry>(&b) else { continue };
                n += 1;
                *steps.entry(e.step.clone()).or_default() += 1;
                if let Some(why) = outside(&e) {
                    bad.push(format!("{}: {why}", f.path().display()));
                }
            }
        }
        eprintln!("{n} entries: {steps:?}");
        assert!(n > 0);
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
