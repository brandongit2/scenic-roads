//! The seam between the runtime and the plan (docs/crates.md §6.3, 6f2): what the runtime holds of
//! the plan as data.
//!
//! `Planned` is a plan's answer; `Publish`, what it publishes by; `RegionLeft`, `Left` and `Along`,
//! what its answer says is left; `PassWork`, the pass's worldwide steps' work. None names a family's
//! type, and none reads the steps table.

use super::jobs::Work;
use super::progress::{Checklist, RegionState};
use super::publish::{PublishLine, Publishing};
use crate::coverage::Coverage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What names-to-do is gated by (`names_todo_work`): its inputs' key, the seconds since it last
/// ran, and whether the build is idle.
pub struct NamesTodo {
    pub key: Option<String>,
    pub since: Option<u64>,
    pub idle: bool,
}

/// A job of the pass's worldwide steps (`PassWork::jobs`): its id, its step's command, the
/// arguments after `--root` and `--scratch`, and the work it records.
#[derive(Debug)]
pub struct PassJob {
    pub id: String,
    pub step: &'static str,
    pub args: Vec<String>,
    pub work: Option<Work>,
}

/// The pass's worldwide steps' work (`pass_work`).
#[derive(Debug, Default)]
pub struct PassWork {
    /// Their jobs, in the plan's order.
    pub jobs: Vec<PassJob>,
    /// Why some wait, said.
    pub waits: Vec<super::Waiting>,
    /// The pass's lake fill holds the jobs that read it (the agent's `hold_for_lakefill`).
    pub lakefill_held: bool,
    /// Everything else waits for the pass's outlines: nothing more planned.
    pub outlines_hold: bool,
    /// The 3D buildings' release is made (or none is declared): its scan's parts may go.
    pub buildings_made: bool,
    /// The summits' work (placed after the regions' plan, which says whether the terrain they read
    /// is built).
    pub summits: Option<Work>,
    /// Each of `WORLDWIDE_STEPS` left (a run, or its output missing from the manifest), for the
    /// checklist, held or not.
    pub left: [bool; 9],
}

/// What the plan builds the regions by, one at a time, and publishes them by, as they're done:
/// each region's own coverage, as the recipes list them (crate::coverage::Coverage::by_region); the
/// regions the map's catalogue has, and its z6 tiles; when the last catalogue went out.
#[derive(Clone, Copy)]
pub struct Publish<'a> {
    pub each: &'a [(String, Coverage)],
    /// The last catalogue's regions by id: whether it has the region as its recipe is now (false: an
    /// older outline, the region on the map as it was).
    pub on_map: &'a BTreeMap<String, bool>,
    /// The served catalogue's z6 tiles' base packs and road values, by logical name
    /// (crate::pubtimes::served_units): the map tiles to draw are grouped by the z6 tile whose change
    /// made them stale (`by_unit`).
    pub served: &'a BTreeMap<String, String>,
    /// Seconds since the last catalogue went out (or one last started); None when none has.
    pub since_catalogue: Option<u64>,
    /// Seconds since a catalogue was first held back by the map tiles or the road index to draw
    /// (`Plan::catalogue_held`), or by their hand-offs waiting to be merged (the agent's), since the
    /// last catalogue; None while none is. Past `CATALOGUE_CAP_S`, one goes out with what's drawn.
    pub held_for: Option<u64>,
    /// Catalogues are held for review (`inputs/hold-catalog`): what's new is weighed against the last
    /// held one's.
    pub held: bool,
    /// How publishing goes (the switches).
    pub way: Publishing,
}

/// The work there is, in order, and the regions a catalogue made now records as built (`ready`: every
/// z6 tile of theirs built as the coverage wants it, and their z3 tiles' slope and tree cover; the
/// catalogue keeps the others as the last one had them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Planned {
    pub work: Vec<Work>,
    pub ready: Vec<String>,
    /// The regions not `ready`, each with its z6 tiles built of those it builds (by the records: a
    /// z6 tile current under its key), in the regions' order: what a catalogue records of a region still
    /// building (its `--building`; docs/formats.md, Catalogue `coverage`).
    pub building: Vec<(String, usize, usize)>,
    /// Every region's work left, in the order they're built (those with none last), whether or not
    /// it can start now: what the forecast schedules (crate::agent::forecast).
    pub regions: Vec<RegionLeft>,
    /// The drops left, (step, target), of every step with "none" targets (`is_none_key`: terrain-hi,
    /// slope-hi, terrain-lo, slope-lo, terrain-deep, slope-deep, trees, trees-lo), runnable now,
    /// first of the terrain, slope and tree cover work: the forecast's first items, shown as drops.
    pub drops: Vec<(String, String)>,
    /// The catalogue planned goes out with the low zooms made (`low_gate`: they were due, and
    /// nothing's left of the publishing chain, or no lo pack is stale), its batch saying so once
    /// made (crate::agent::publish::LOW). Without a catalogue planned: the low zooms are made now.
    pub low: bool,
    /// A catalogue is due but held back by the map tiles or the road index to draw (`CATALOGUE_GATE`),
    /// short of `CATALOGUE_CAP_S`: the agent counts how long (`Publish::held_for`).
    pub catalogue_held: bool,
    /// A catalogue is due, not held back, but waits out its minute (`CATALOGUE_EVERY_S`): the hold's
    /// count (`LastCatalogue::held_at`) is kept till it goes.
    pub catalogue_due: bool,
    /// The z6 tiles whose key can't be worked out now (a terrain pack's index unread: `unit_keys`):
    /// neither built nor counted as built until it can.
    pub unknown: Vec<String>,
    /// The z6 tiles whose mids are made in idle time (`TreeWork::backfill`, `TerrainWork::backfill`:
    /// the work's last): for the forecast.
    pub backfill: Vec<Work>,
    /// Publishing as planned now, for the log and the status.
    pub publishing: PublishLine,
    /// No z6 tile or terrain left to build: the steps gated by time (`GATED`) run at once.
    pub idle: bool,
    /// What's left of each step the plan's targets read, runnable or not (crate::agent::steps::View):
    /// every target held is held through its readiness in it (the summits' too, by the agent).
    pub left: Left,
    /// The steps with work left that is held for other steps' targets, each with what it waits for
    /// (a note in the summits' form): terrain, slope and tree cover's, and the landmarks'.
    pub held: Vec<(String, String)>,
    /// The regions' steps' checklist as this plan finds them (`checklist`, with the plan's keys).
    pub checklist: Checklist,
    /// How far each region is built, by id: the build page's and `scenic status`'s counts.
    pub built: BTreeMap<String, RegionState>,
}

/// A region's work left (`Plan::regions`), as targets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegionLeft {
    pub id: String,
    /// Whether the map's catalogue has it: as its recipe is now (true), as it was (false), or not at
    /// all (None).
    pub on_map: Option<bool>,
    /// Its z6 tiles not built as the coverage wants them, all of them (a region is done when they
    /// are), and those it builds itself (the rest come with a region before it), in the order
    /// they're built.
    pub units: Vec<String>,
    pub own_units: Vec<String>,
    /// The terrain z6 tiles of the z3 tiles it reads to make (stale, or their mids for an assembly),
    /// and those it makes itself (the rest come with a region before it); those z3 tiles' stale
    /// assemblies, and those it makes itself.
    pub terrain: Vec<String>,
    pub own_terrain: Vec<String>,
    #[serde(default)]
    pub terrain_lo: Vec<String>,
    #[serde(default)]
    pub own_terrain_lo: Vec<String>,
    /// The terrain-deep z10 tiles of the z3 tiles it reads left to make, and those it makes itself.
    #[serde(default)]
    pub terrain_deep: Vec<String>,
    #[serde(default)]
    pub own_terrain_deep: Vec<String>,
    /// Its z3 tiles' stale slope-deep z10 tiles.
    #[serde(default)]
    pub slope_deep: Vec<String>,
    /// Its z3 tiles' stale slope z6 tiles and assemblies; the tree cover z6 tiles of its z3 tiles to make
    /// (stale, or their mids for an assembly), and its z3 tiles whose tree cover assembly is stale.
    pub slope: Vec<String>,
    #[serde(default)]
    pub slope_lo: Vec<String>,
    pub trees: Vec<String>,
    #[serde(default)]
    pub trees_lo: Vec<String>,
    /// Its z6 tiles built as the coverage wants them now but reading terrain that's stale: whether
    /// each goes stale too is known only once that terrain is built (its key names the tiles'
    /// contents). For the forecast, which takes a share of them as work to come
    /// (`forecast::EXPECTED_STALE`).
    #[serde(default)]
    pub expected: Vec<String>,
    /// The map tiles (pack(T)'s targets, "6/x/y") its z6 tiles draw that are stale now or will be once
    /// its z6 tiles left are built, each with its z6 tiles that draw it (a tile waits for those alone):
    /// what's drawn before it's on the map as it's built now.
    #[serde(default)]
    pub packs: Vec<(String, Vec<String>)>,
}

/// What targets read along their long ways (`Read::Along`), as a caller with the pass's reaches
/// works them out (`Along::add`), and where what's built says (`Read::Built`, `Along::add_built`):
/// by step, target and step read, the targets read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Along(
    std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, Vec<String>>>,
    >,
);

impl Along {
    /// Adds what target `t` of `step` reads of `read` (nothing: nothing added).
    pub fn put(&mut self, step: &str, t: &str, read: &str, v: Vec<String>) {
        if !v.is_empty() {
            let at = self.0.entry(step.to_string()).or_default().entry(t.to_string()).or_default();
            at.entry(read.to_string()).or_default().extend(v);
        }
    }

    /// The targets of `read` that target `t` of `step` reads along its long ways.
    pub fn get(&self, step: &str, t: &str, read: &str) -> &[String] {
        self.0.get(step).and_then(|m| m.get(t)).and_then(|m| m.get(read)).map_or(&[], Vec::as_slice)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
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
    /// By input, the targets it can't be worked out for now (an input some targets read only), and
    /// the target whose making works it out.
    pub unknown_for: std::collections::BTreeMap<&'static str, UnknownFor>,
    /// What targets read along their long ways (`Read::Along`), from the pass's reaches.
    pub along: Along,
}

/// An input's targets that can't be worked out now (`Left::unknown_for`), and the target (step,
/// target) whose making works it out: what they wait for, to the planner and the forecast alike.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnknownFor {
    pub targets: std::collections::BTreeSet<String>,
    pub until: (&'static str, String),
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
