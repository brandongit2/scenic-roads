//! What the lead builds for the regions (docs/plan.md §6, Job keys; §8, Order): per z3 pack,
//! the terrain and then the slope of its z6 tiles near the coverage; base(U) for every unit whose
//! piece meets the coverage, a region at a time; pack(T) for the z6 tiles near changed units, the
//! lo packs above them, as soon as those units are built; then a catalog, at most one a minute (`plan`).
//!
//! Each job's key is a hash of what it reads: its step's version, the coverage near it, and the
//! content names (from the build manifest) of its inputs. A job whose key matches the one recorded
//! when it last succeeded (`state/build/jobs.json`) isn't run again; an unchanged output keeps its
//! content name, so what depends on it keeps its key too.

use super::tiles::{Reader, TerrainTiles, Tile, Unread};
use crate::coverage::Coverage;
use crate::legacy::Unit;
use crate::reach::{Reach, Reaches};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Step versions: bumping one rebuilds that step everywhere (oldest first, when idle).
/// Terrain 2: the one-pass repair (roadcore::grid::repair_terrain), on AWS's values before
/// bathymetry goes to sea level. 3: GLO-30 north of 60°N, the water flattened from the pass's
/// basemap (the digest of what each target reads of it in the key: `water_key`), the seam spikes
/// and walled patches (docs/plan.md §6, Terrain).
pub const TERRAIN_V: u32 = 3;
pub const SLOPE_V: u32 = 1;
/// 2: elevations up to 6,053 m (`final.u16`, base packs' `elevu`; were clamped at ±3,200 m).
/// 3: heritage sites and area flags from the pass's heritage-sites job (crate::heritage), the
/// flags rasterised per unit.
/// 4: the roads' own English (`global/roaden/<u>`); a unit left with no ways drops its outputs.
pub const UNIT_V: u32 = 4;
/// 3: keyed on the tile's owners alone; hidata of the tile's own ways (here, query parts, rail
/// lines, zoomed-out summaries).
pub const PACK_V: u32 = 3;
/// lo's own version: lo isn't keyed on pack's, so a change in what they share (hipack's tiling)
/// bumps both.
pub const LO_V: u32 = 1;

/// The keys of the jobs that last succeeded, by step and target ("3/4/2", "6/31/20").
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Keys {
    /// Terrain's and slope's pieces, per z6 tile ("6/x/y": `terrain_slope_targets`; records of
    /// their z3 tiles' whole runs, "3/x/y", from before the pieces: agent::rekey).
    #[serde(default)]
    pub terrain: BTreeMap<String, String>,
    #[serde(default)]
    pub slope: BTreeMap<String, String>,
    /// Their assemblies, per z3 tile ("3/x/y").
    #[serde(default)]
    pub terrain_lo: BTreeMap<String, String>,
    #[serde(default)]
    pub slope_lo: BTreeMap<String, String>,
    #[serde(default)]
    pub unit: BTreeMap<String, String>,
    /// The units' landmark candidates.
    #[serde(default)]
    pub pois: BTreeMap<String, String>,
    /// The units' peaks' prominence and isolation.
    #[serde(default)]
    pub peaks: BTreeMap<String, String>,
    #[serde(default)]
    pub pack: BTreeMap<String, String>,
    #[serde(default)]
    pub lo: BTreeMap<String, String>,
    /// The tree cover layers' pieces, per z6 tile ("6/x/y", crate::treepacks::targets; records of
    /// the z3 tiles' whole runs, "3/x/y", from before the pieces: agent::rekey).
    #[serde(default)]
    pub trees: BTreeMap<String, String>,
    /// Their assemblies, per z3 tile ("3/x/y").
    #[serde(default)]
    pub trees_lo: BTreeMap<String, String>,
    /// The 3D buildings' normalized files (`bldprep`) and tiles (`bldtiles`), per z6 tile ("6/x/y":
    /// `bld_targets`).
    #[serde(default)]
    pub bldprep: BTreeMap<String, String>,
    #[serde(default)]
    pub bldtiles: BTreeMap<String, String>,
    /// The buildings field's lo levels, per z3 tile ("3/x/y": `bld_targets`).
    #[serde(default)]
    pub bldfield_lo: BTreeMap<String, String>,
    /// The depth's pieces, per z6 tile ("6/x/y"), and its assemblies, per z3 tile ("3/x/y":
    /// `depth_targets`).
    #[serde(default)]
    pub depth: BTreeMap<String, String>,
    #[serde(default)]
    pub depth_lo: BTreeMap<String, String>,
    /// The served files the last catalog was made from.
    #[serde(default)]
    pub catalog: Option<String>,
    /// The same for the last catalog held for review (`inputs/hold-catalog`: written to
    /// catalog-held/, not served).
    #[serde(default)]
    pub catalog_held: Option<String>,
    /// Records of steps this app doesn't know (a newer app's), kept as they are when the keys are
    /// saved again: an older app's agent leading the build doesn't drop them.
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
    /// For planning (`load_with`; never saved): the files the hand-offs waiting to be merged save,
    /// by logical name. A tree cover piece's mid there counts as made (`tree_work`).
    #[serde(skip)]
    pub handed: BTreeSet<String>,
}

impl Keys {
    /// The keys for showing (the status): none when they can't be read now.
    pub fn load(root: &Path) -> Keys {
        Self::load_strict(root).unwrap_or_default()
    }

    /// The keys for planning and recording: none when there are none yet, an error when they can't
    /// be read now (so a job isn't started again, nor the keys written back, from empty ones).
    pub fn load_strict(root: &Path) -> anyhow::Result<Keys> {
        crate::out::read_record(&root.join("state/build/jobs.json"))
    }

    /// The keys with the done records of the hand-offs waiting under each of `bases` on top (the
    /// coordinator's journal on this Mac: crate::handoff), and the files they save (`handed`).
    pub fn load_with(root: &Path, bases: &[std::path::PathBuf]) -> anyhow::Result<Keys> {
        let mut hs = Vec::new();
        for b in bases {
            hs.extend(crate::handoff::waiting_in(b)?);
        }
        let mut k = Keys::load_strict(root)?;
        for (_, h) in hs {
            for (l, c) in &h.changes {
                if c.is_some() {
                    k.handed.insert(l.clone());
                } else {
                    k.handed.remove(l);
                }
            }
            if let Some((step, targets)) = &h.done {
                k.record(step, targets);
            }
        }
        Ok(k)
    }

    pub fn save(&self, root: &Path) -> anyhow::Result<()> {
        let p = root.join("state/build/jobs.json");
        std::fs::create_dir_all(p.parent().unwrap())?;
        crate::whole::write(&p, &serde_json::to_vec_pretty(self)?)
    }

    fn map(&mut self, step: &str) -> &mut BTreeMap<String, String> {
        match step {
            "terrain" => &mut self.terrain,
            "slope" => &mut self.slope,
            "terrain-lo" => &mut self.terrain_lo,
            "slope-lo" => &mut self.slope_lo,
            "unit" => &mut self.unit,
            "pois" => &mut self.pois,
            "peaks" => &mut self.peaks,
            "pack" => &mut self.pack,
            "trees" => &mut self.trees,
            "trees-lo" => &mut self.trees_lo,
            "bldprep" => &mut self.bldprep,
            "bldtiles" => &mut self.bldtiles,
            "bldfield-lo" => &mut self.bldfield_lo,
            "depth" => &mut self.depth,
            "depth-lo" => &mut self.depth_lo,
            _ => &mut self.lo,
        }
    }

    /// The key recorded for `target` of a per-target step.
    pub fn recorded(&self, step: &str, target: &str) -> Option<&str> {
        let m = match step {
            "terrain" => &self.terrain,
            "slope" => &self.slope,
            "terrain-lo" => &self.terrain_lo,
            "slope-lo" => &self.slope_lo,
            "unit" => &self.unit,
            "pois" => &self.pois,
            "peaks" => &self.peaks,
            "pack" => &self.pack,
            "lo" => &self.lo,
            "trees" => &self.trees,
            "trees-lo" => &self.trees_lo,
            "bldprep" => &self.bldprep,
            "bldtiles" => &self.bldtiles,
            "bldfield-lo" => &self.bldfield_lo,
            "depth" => &self.depth,
            "depth-lo" => &self.depth_lo,
            _ => return None,
        };
        m.get(target).map(String::as_str)
    }

    /// Records a job's targets as done with their keys. A prune forgets its targets' keys instead
    /// ("unit 6/x/y", "pois 6/x/y", "pack 6/x/y", "lo 3/x/y", "bldprep 6/x/y", "bldtiles 6/x/y",
    /// "bldfield-lo 3/x/y"), so a
    /// region added back is built again.
    pub fn record(&mut self, step: &str, done: &[(String, String)]) {
        if step == "prune" {
            for (t, _) in done {
                let Some((kind, at)) = t.split_once(' ') else { continue };
                match kind {
                    "unit" => {
                        self.unit.remove(at);
                    }
                    "pois" => {
                        self.pois.remove(at);
                        self.peaks.remove(at);
                    }
                    "pack" => {
                        self.pack.remove(at);
                    }
                    "lo" => {
                        self.lo.remove(at);
                    }
                    "bldprep" => {
                        self.bldprep.remove(at);
                    }
                    "bldtiles" => {
                        self.bldtiles.remove(at);
                    }
                    "bldfield-lo" => {
                        self.bldfield_lo.remove(at);
                    }
                    _ => {}
                }
            }
            return;
        }
        if step == "catalog" {
            self.catalog = done.first().map(|d| d.1.clone());
            return;
        }
        if step == "catalog-held" {
            self.catalog_held = done.first().map(|d| d.1.clone());
            return;
        }
        if step.ends_with("-root")
            || matches!(
                step,
                "inputs"
                    | "outlines"
                    | "regions-extract"
                    | "labels"
                    | "spoken"
                    | "names-todo"
                    | "timetables-todo"
                    | "lakefill"
                    | "water"
                    | "trailends"
                    | "reach"
                    | "summits"
                    | "items"
                    | "marks"
                    | "roadunits"
                    | "stations"
                    | "ferries"
                    | "heritage-sites"
                    | "heritage"
                    | "overlays"
                    | "rail-feeds"
                    | "rail"
                    | "bld-fetch"
                    | "terrain-water"
            )
        {
            // Kept with the lo keys, under the step's own name.
            for (t, k) in done {
                self.lo.insert(t.clone(), k.clone());
            }
            return;
        }
        let m = self.map(step);
        for (t, k) in done {
            m.insert(t.clone(), k.clone());
        }
    }
}

/// One step for some targets, with the key each target will be recorded under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Work {
    /// "terrain", "slope", "unit", "pack", "lo", "catalog".
    pub step: String,
    pub targets: Vec<(String, String)>,
}

pub(crate) fn h(parts: &[&str]) -> String {
    store::naming::hash16(parts.join("\n").as_bytes())
}

/// The whole coverage, for the jobs that read all of it: its shapes by their geometry (buffer and
/// rings) alone. (Jobs that read part of it are keyed on `Coverage::fingerprint` of their box.)
/// Which region or outline entry a shape came from never enters a key, so renaming a region's id,
/// or splitting and merging regions with the same outlines, reruns nothing.
pub(crate) fn coverage_all(cov: &Coverage) -> String {
    let mut v: Vec<String> = cov
        .shapes
        .iter()
        .map(|s| format!("{}:{}", s.buffer_m, store::naming::hash16(bytemuck::cast_slice(&s.rings.concat()))))
        .collect();
    v.sort();
    v.dedup();
    v.join(",")
}

pub(crate) fn grown_e7(z: u8, x: u32, y: u32, km: f64) -> [i32; 4] {
    let b = crate::stage::tile_box_grown(z, x, y, km);
    let e7 = |v: f64| (v * 1e7).round() as i32;
    [e7(b[0]), e7(b[1]), e7(b[2]), e7(b[3])]
}

/// The z6 tiles near the coverage (20 km), by z3 pack.
pub fn coverage_tiles(cov: &Coverage) -> BTreeMap<(u32, u32), Vec<(u32, u32)>> {
    let mut by_q: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for x in 0..64u32 {
        for y in 0..64u32 {
            if crate::terrain_pack::near_coverage(cov, 6, x, y, 20.0) {
                by_q.entry((x >> 3, y >> 3)).or_default().push((x, y));
            }
        }
    }
    by_q
}

/// The outlines step's version (crate::osmpass::make_outlines, crate::outlines::assemble): bumped
/// with any change to what the assembly makes, so the pass's outlines are made again then, not at
/// the next pass. 1: assembled inside the OSM pass. 2: #199's (countries osmium's export leaves
/// out assembled here, a territory by its own ISO code, a country kept from the previous pass's
/// outlines when it can't be), the first made by the step.
pub const OUTLINES_V: u32 = 2;

/// The outlines step's key for the pass of `date`: its version, the date, the pass's outline set and
/// the previous outlines it may keep a country from (crate::osmpass::previous_outlines), by content
/// name. None without the set (pass-sets makes it).
pub fn outlines_key(date: &str, m: &BTreeMap<String, String>) -> Option<String> {
    let set = m.get(&crate::osmpass::set_name(date, "outlines"))?;
    let previous = crate::osmpass::previous_outlines(m, date)
        .map_or("previous -".to_string(), |(d, c)| format!("previous {d} {c}"));
    Some(h(&[&format!("outlines {OUTLINES_V}"), date, set, &previous]))
}

/// The pass's administrative and ISO 3166 outlines (`sources/osm/<date>/outlines`), once per pass,
/// outline set, previous outlines or `OUTLINES_V`; made again when missing.
pub fn outlines_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let k = outlines_key(date, m)?;
    let made = m.contains_key(&format!("sources/osm/{date}/outlines"));
    (!made || done.lo.get("outlines").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "outlines".into(), targets: vec![("outlines".into(), k)] })
}

/// Failures in a row of the outlines job after which the rest of the build no longer waits for it
/// (`outlines_hold`), while the pass has outlines to go on with.
pub const OUTLINES_HOLDS: u32 = 3;

/// Whether the rest of the build waits for the outlines job (which has work): while the pass has
/// no outlines (`made`), or until the job has failed `OUTLINES_HOLDS` times in a row.
pub fn outlines_hold(made: bool, failures: u32) -> bool {
    !made || failures < OUTLINES_HOLDS
}

/// Every hiking route's ends, worldwide, once per pass (crate::trailends): what the units' extract
/// reads.
pub const TRAILENDS_V: u32 = 1;

pub fn trailends_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let set = m.get(&crate::osmpass::set_name(date, "hikes"))?;
    let k = h(&[&format!("trailends {TRAILENDS_V}"), set]);
    (done.lo.get("trailends").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "trailends".into(), targets: vec![("trailends".into(), k)] })
}

/// Each unit's reach (crate::reach), once per pass: which units the coverage builds, and what their
/// keys read of it.
pub fn reach_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let k = reach_key(date, m)?;
    let made = m.contains_key(&crate::reach::logical(date));
    (!made || done.lo.get("reach").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "reach".into(), targets: vec![("reach".into(), k)] })
}

/// The reach job's key: its version and the pieces, by content (a piece cut again changes it).
pub fn reach_key(date: &str, m: &BTreeMap<String, String>) -> Option<String> {
    let prefix = format!("sources/osm/{date}/pieces/");
    let pieces: Vec<&str> =
        m.range(prefix.clone()..).take_while(|(l, _)| l.starts_with(&prefix)).map(|(_, c)| c.as_str()).collect();
    if pieces.is_empty() {
        return None;
    }
    let mut ins = vec![format!("reach {}", crate::reach::REACH_V)];
    ins.extend(pieces.iter().map(|s| s.to_string()));
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    Some(h(&refs))
}

/// The labels by importance, worldwide, once per pass (or labels step version): independent of the
/// regions. (2: each label's OSM object, the languages OSM gives its name, its romanised name or
/// kana reading: docs/plan.md §7.)
pub const LABELS_V: u32 = 2;

pub fn labels_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let set = m.get(&crate::osmpass::set_name(date, "labels"))?;
    let k = h(&[&format!("labels {LABELS_V}"), set]);
    (done.lo.get("labels").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "labels".into(), targets: vec![("labels".into(), k)] })
}

/// The languages spoken where (names::spoken), worldwide, once per pass's outlines, accepted
/// version of the languages input (docs/inputs.md §6.2: its index's content name, which the job is
/// given) or rules version: `global/spoken`, which the servers read. None while the input has no
/// accepted version.
pub fn spoken_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let outlines = m.get(&format!("sources/osm/{date}/outlines"))?;
    let table = m.get(&crate::inputs::logical(crate::inputs::languages::UNIT))?;
    let k = h(&[names::spoken::RULES, outlines, &format!("table {table}")]);
    (done.lo.get("spoken").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "spoken".into(), targets: vec![("spoken".into(), k)] })
}

/// The to-do lists of names to translate and descriptions to write (crate::namestodo), once per
/// catalog the map serves, accepted version of the translations and of the descriptions (it reads
/// them: docs/inputs.md §6.3, §6.4), or step version: after each build, and after each batch taken
/// in. None while either input has no accepted version yet.
pub fn names_todo_work(served: Option<u64>, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let n = served?;
    let tr = m.get(&crate::inputs::logical(crate::inputs::translations::UNIT))?;
    let de = m.get(&crate::inputs::logical(crate::inputs::descriptions::UNIT))?;
    let k = h(&[&format!("names-todo {}", crate::namestodo::VERSION), &n.to_string(), tr, de]);
    (done.lo.get("names-todo").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "names-todo".into(), targets: vec![("names-todo".into(), k)] })
}

/// The timetables' to-do list and report (crate::timetables), once per rail and ferries pieces
/// (`work/rail/lines`, `work/ferries/lines`: after each rail and ferries run), coverage (it keeps
/// the ferry lines in it) or step version. None until both pieces are made.
pub fn timetables_todo_work(cov: &Coverage, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let rail = m.get(crate::timetables::RAIL_LINES)?;
    let ferries = m.get(crate::timetables::FERRY_LINES)?;
    let k = h(&[&format!("timetables-todo {}", crate::timetables::VERSION), rail, ferries, &coverage_all(cov)]);
    (done.lo.get("timetables-todo").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "timetables-todo".into(), targets: vec![("timetables-todo".into(), k)] })
}

/// The lakefill step's version (crate::lakefill::run): bumped with any change to what it makes.
pub const LAKEFILL_V: u32 = 1;

/// The lakes filled from Canada's own data (crate::lakefill), once per pass (or step version, or
/// source): from the pass's basemap and outlines and the declared source's zips (`source`, their pin
/// and what the key names of the declaration: crate::lakefill::run::planned_source; None without
/// zips, and no fill made).
pub fn lakefill_work(date: &str, m: &BTreeMap<String, String>, done: &Keys, source: Option<&str>) -> Option<Work> {
    let source = source?;
    let basemap = m.get(&format!("layers/basemap/world-{date}"))?;
    let outlines = m.get(&format!("sources/osm/{date}/outlines"))?;
    let k = h(&[&format!("lakefill {LAKEFILL_V}"), basemap, outlines, source]);
    (done.lo.get("lakefill").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "lakefill".into(), targets: vec![("lakefill".into(), k)] })
}

/// Failures in a row of the lakefill job after which the water no longer waits for it
/// (`water_waits_for_lakefill`).
pub const LAKEFILL_HOLDS: u32 = 3;

/// Whether the water layer's job waits for the lakefill job (`lakefill`: whether it has work):
/// while it has, until it has failed `LAKEFILL_HOLDS` times in a row (`failures`); then the water
/// is drawn with the fill the manifest has for the pass (or none), and drawn again once the fill
/// is made.
pub fn water_waits_for_lakefill(lakefill: bool, failures: u32) -> bool {
    lakefill && failures < LAKEFILL_HOLDS
}

/// The steps that read the declared sources (crate::inputs::sources), which wait for their first
/// accepted version (agent's `hold_for_sources`): the units (their land cover and canopy), the tree
/// cover (its canopy and leaf-type squares), the items and the heritage chain (Wikidata, Wikipedia, the
/// pageviews).
pub fn reads_sources(step: &str) -> bool {
    matches!(step, "unit" | "trees" | "items" | "heritage")
}

/// The steps that read the pass's lake fill, which wait for it (agent's `hold_for_lakefill`).
pub fn reads_lakefill(step: &str) -> bool {
    matches!(step, "water" | "terrain-water" | "catalog" | "catalog-held")
}

/// The water layer (crate::water), worldwide, once per pass (or layer version, or fill): drawn
/// from the pass's basemap's z14 water, and the lakes filled from Canada's own data made from that
/// basemap when the manifest has them (the agent runs it once the lakefill step is done:
/// `water_waits_for_lakefill`).
pub fn water_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let basemap = m.get(&format!("layers/basemap/world-{date}"))?;
    // (The lakes filled from Canada's own data, when the manifest has them: crate::lakefill.)
    let k = match m.get(&crate::lakefill::logical(date)) {
        Some(fill) => h(&[&format!("water {}", crate::water::VERSION), basemap, &format!("lakefill {fill}")]),
        None => h(&[&format!("water {}", crate::water::VERSION), basemap]),
    };
    (done.lo.get("water").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "water".into(), targets: vec![("water".into(), k)] })
}

/// Whether unit `u` is built for the coverage: some road it owns may touch it (`Reach::builds`;
/// the unit step then keeps exactly the ways that do).
pub fn builds(cov: &Coverage, reach: &Reaches, u: Unit) -> bool {
    reach.get(u).is_some_and(|r| r.builds(cov))
}

/// The units the coverage builds (`builds`), each with its key (`unit_key`); None for one whose
/// terrain can't be worked out now (a terrain pack's index unread: agent::tiles), which waits. None
/// before the pass's reaches are made.
pub fn unit_keys(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    reach: Option<&Reaches>,
    tiles: &TerrainTiles,
) -> Vec<(Unit, Option<String>)> {
    let mut units = Vec::new();
    let Some(reach) = reach else { return units };
    for (l, c) in m.range(format!("sources/osm/{date}/pieces/")..) {
        let Some(u) = l.strip_prefix(&format!("sources/osm/{date}/pieces/")).and_then(Unit::parse) else { break };
        let Some(r) = reach.get(u).filter(|r| r.builds(cov)) else { continue };
        // (Without the declared elevation datasets, before the switch or while they can't be read,
        // no key: the unit waits.)
        // (Nor without the declared buildings source's release.)
        let elevation = tiles.elevation(m).ok().flatten().zip(tiles.keyed(m));
        units.push((
            u,
            unit_terrain(u, r, m, tiles)
                .ok()
                .zip(elevation)
                .map(|(t, (e, k))| unit_key(cov, date, m, u, c, r, &t, &e, &k.release, k.unit.as_deref())),
        ));
    }
    units
}

/// Unit `u`'s key (its piece `piece`, its reach `r`): what it reads. Its piece and road values, the
/// coverage as its ways meet it (`Reach::coverage_key`), the location rules where they go
/// (`crate::rules`) and the `keyed` digests of the declared elevation datasets meeting its ways
/// (`elevation`: crate::inputs::elevation), the roadside buildings' index (of the declared release
/// `buildings`: crate::buildtiles), the land-cover and canopy declarations where they aren't as
/// staged (`sources`: crate::inputs::sources::unit_extra), the terrain tiles it reads by their contents (`terrain`: `unit_terrain`'s
/// digest), and the heritage sites' and areas' slices near it, as the manifest has them, which is
/// what the unit step stages from.
#[allow(clippy::too_many_arguments)]
pub fn unit_key(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    u: Unit,
    piece: &str,
    r: &Reach,
    terrain: &str,
    elevation: &[(crate::dem::declared::Accepted, crate::dem::declared::Cover)],
    buildings: &str,
    sources: Option<&str>,
) -> String {
    let ext = r.extent(u);
    let deg = |v: i32| v as f64 * 1e-7;
    let e = format!(
        "elevation {}",
        crate::dem::declared::meeting(elevation, [deg(ext[0]), deg(ext[1]), deg(ext[2]), deg(ext[3])]).join(",")
    );
    unit_key_with(cov, date, m, u, piece, r, terrain, crate::rules::versions_meeting(ext), Some(e), buildings, sources)
}

/// `unit_key` of the location rules' versions `rules` and the elevations' line `elevation` (None:
/// before the declared datasets, when the DEM rules were among the rules: agent::rekey), the
/// roadside buildings of release `buildings`, and the declared sources' line `sources` (None: as
/// staged, or before them).
#[allow(clippy::too_many_arguments)]
pub(crate) fn unit_key_with(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    u: Unit,
    piece: &str,
    r: &Reach,
    terrain: &str,
    rules: String,
    elevation: Option<String>,
    buildings: &str,
    sources: Option<&str>,
) -> String {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let mut inputs = vec![
        format!("unit {UNIT_V}"),
        piece.to_string(),
        get(&format!("sources/osm/{date}/roads/{}", u.dash())).to_string(),
        r.coverage_key(cov, u),
        rules,
    ];
    inputs.extend(elevation);
    // The roadside buildings it reads (crate::buildtiles: the release's tiles near its roads).
    inputs.push(format!("buildings {}", get(&crate::buildtiles::index_logical(buildings))));
    // (The land cover and canopy it reads, once their declarations aren't as staged.)
    inputs.extend(sources.map(|s| format!("sources {s}")));
    // The terrain it reads. Not the analysis grids' packs (grid-class, -canopy, -cover): the units
    // write those where they're missing, so each built unit would change its own key and its
    // neighbours' (built again, over and over); a grid read from its pack or made afresh is the
    // same, from fixed datasets (WorldCover, Meta's canopy squares).
    inputs.push(format!("terrain-tiles {terrain}"));
    let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
    for (x, y) in crate::stage::tiles_in(6, b) {
        inputs.push(get(&crate::heritage::pos_logical(date, x, y)).to_string());
        inputs.push(get(&crate::heritage::areas_logical(date, x, y)).to_string());
    }
    let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
    h(&refs)
}

/// `unit_terrain_tiles` as one digest, kept by `tiles` with what it reads (the unit's reach and the
/// content names of the packs its staged tiles are in): a plan works out only the units next to
/// what changed.
pub fn unit_terrain(u: Unit, r: &Reach, m: &BTreeMap<String, String>, tiles: &TerrainTiles) -> Result<String, Unread> {
    let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
    let mut packs: BTreeSet<String> = BTreeSet::new();
    for z in 4..=12u8 {
        let [x0, x1, y0, y1] = crate::stage::tile_range(z, b);
        let (scope, s) = if z <= 8 { ("lo", z - 3) } else { ("hi", z - 6) };
        for x in x0 >> s..=x1 >> s {
            for y in y0 >> s..=y1 >> s {
                let l = format!("layers/terrain/{scope}/{}-{x}-{y}", z - s);
                packs.insert(format!("{l}={}", m.get(&l).map(String::as_str).unwrap_or("-")));
            }
        }
    }
    let from =
        [serde_json::to_string(r).unwrap_or_default(), packs.into_iter().collect::<Vec<_>>().join(",")].join("\n");
    tiles.memo(&u.slash(), &store::naming::hash16(from.as_bytes()), || {
        Ok(terrain_digest(&unit_terrain_tiles(u, r, m, tiles)?))
    })
}

/// Tiles (`unit_terrain_tiles`) as one digest.
pub fn terrain_digest(tiles: &[Tile]) -> String {
    let lines: Vec<String> = tiles.iter().map(|(z, x, y, t)| format!("{z}/{x}/{y} {t:016x}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    h(&refs)
}

/// The terrain tiles unit `u` reads, by their contents (from the packs' indexes: agent::tiles),
/// sorted. A unit stages terrain z0–12 over its tile + 30 km, `b` (crate::stage), which two steps
/// read:
/// - its grid (canopy, view and flags: `grid.terrain.i16`), every z11 tile staged, as it is (a
///   missing one reads as zeros);
/// - `prep` (scenic-metrics), at each point of the ways it owns (the drape at every vertex, a
///   sample's ground), the z12 tile there or its finest staged ancestor up to eight levels up
///   (crate::terr). Those points lie in its owned box (its ordinary ways) and on the segments of its
///   own long ways (extract puts points along them in degrees; a ferry's are its nodes alone, but
///   its segments are taken), each tile tested grown by `EDGE_DEG`.
///
/// Nothing else a unit makes reads terrain. A tile staged but missing is named by its absence: one
/// appearing (the coverage grown) changes the list, as does any tile read changing.
pub fn unit_terrain_tiles(
    u: Unit,
    r: &Reach,
    m: &BTreeMap<String, String>,
    tiles: &TerrainTiles,
) -> Result<Vec<Tile>, Unread> {
    let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
    let staged: Vec<[u32; 4]> = (0..=12u8).map(|z| crate::stage::tile_range(z, b)).collect();
    let mut walk = Walk { staged: &staged, read: tiles.reader(m), out: BTreeSet::new() };
    let [x0, x1, y0, y1] = staged[11];
    for x in x0..=x1 {
        for y in y0..=y1 {
            if let Some(t) = walk.read.hash(11, x, y)? {
                walk.out.insert((11, x, y, t));
            }
        }
    }
    let deg = |v: i32| v as f64 * 1e-7;
    let mut reached: Vec<Reached> = Vec::new();
    if let Some(o) = r.owned {
        reached.push(Reached::Box([deg(o[0]), deg(o[1]), deg(o[2]), deg(o[3])]));
    }
    for w in r.long.iter().filter(|w| w.owned) {
        let p: Vec<[f64; 2]> = w.verts.iter().map(|v| [deg(v[0]), deg(v[1])]).collect();
        match p.len() {
            0 => {}
            1 => reached.push(Reached::Seg(p[0], p[0])),
            _ => reached.extend(p.windows(2).map(|s| Reached::Seg(s[0], s[1]))),
        }
    }
    for s in &reached {
        let [x0, x1, y0, y1] = crate::stage::tile_range(4, s.bbox());
        for x in x0..=x1 {
            for y in y0..=y1 {
                if s.meets(tile_deg(4, x, y)) {
                    walk.visit(s, 4, x, y, None)?;
                }
            }
        }
    }
    Ok(walk.out.into_iter().collect())
}

/// How far a tile's box is grown when tested against where a unit's points are (degrees, ~11 cm):
/// extract rounds the points it puts along a segment to E7, and a point on a tile's edge may read
/// either tile.
const EDGE_DEG: f64 = 1e-6;

/// Tile (z, x, y)'s box (w, s, e, n, degrees) grown by `EDGE_DEG`; the top row's to the pole (a
/// point past 85.05° reads it: crate::terr).
fn tile_deg(z: u8, x: u32, y: u32) -> [f64; 4] {
    use det::Det;
    let n = (1u64 << z) as f64;
    let lon = |t: f64| t / n * 360.0 - 180.0;
    let lat = |t: f64| (std::f64::consts::PI * (1.0 - 2.0 * t / n)).dsinh().datan().to_degrees();
    let top = if y == 0 { 90.0 } else { lat(y as f64) };
    [lon(x as f64) - EDGE_DEG, lat(y as f64 + 1.0) - EDGE_DEG, lon(x as f64 + 1.0) + EDGE_DEG, top + EDGE_DEG]
}

/// Where the points of a unit's own ways lie: its owned box, or a segment of one of its long ways
/// (degrees).
enum Reached {
    Box([f64; 4]),
    Seg([f64; 2], [f64; 2]),
}

impl Reached {
    /// Its box, grown by `EDGE_DEG`.
    fn bbox(&self) -> [f64; 4] {
        let b = match self {
            Reached::Box(b) => *b,
            Reached::Seg(a, c) => [a[0].min(c[0]), a[1].min(c[1]), a[0].max(c[0]), a[1].max(c[1])],
        };
        [b[0] - EDGE_DEG, b[1] - EDGE_DEG, b[2] + EDGE_DEG, b[3] + EDGE_DEG]
    }

    /// Whether it meets box `t` (w, s, e, n).
    fn meets(&self, t: [f64; 4]) -> bool {
        match self {
            Reached::Box(b) => b[0] <= t[2] && t[0] <= b[2] && b[1] <= t[3] && t[1] <= b[3],
            // (Liang–Barsky: the part of the segment inside each of the box's slabs.)
            Reached::Seg(a, c) => {
                let (mut lo, mut hi) = (0.0f64, 1.0f64);
                let (dx, dy) = (c[0] - a[0], c[1] - a[1]);
                for (p, q) in [(-dx, a[0] - t[0]), (dx, t[2] - a[0]), (-dy, a[1] - t[1]), (dy, t[3] - a[1])] {
                    if p == 0.0 {
                        if q < 0.0 {
                            return false;
                        }
                    } else if p < 0.0 {
                        lo = lo.max(q / p);
                    } else {
                        hi = hi.min(q / p);
                    }
                }
                lo <= hi
            }
        }
    }
}

/// A walk down the tiles a unit's points are in (`unit_terrain_tiles`), each point's z12 tile at the
/// end with the tile it reads.
struct Walk<'a> {
    /// The tiles staged, by zoom: [x0, x1, y0, y1].
    staged: &'a [[u32; 4]],
    read: Reader<'a>,
    out: BTreeSet<Tile>,
}

impl Walk<'_> {
    fn staged(&self, z: u8, x: u32, y: u32) -> bool {
        let r = self.staged[z as usize];
        (r[0]..=r[1]).contains(&x) && (r[2]..=r[3]).contains(&y)
    }

    /// Whether (z, x, y) or a tile under it, down to z12, is staged.
    fn staged_under(&self, z: u8, x: u32, y: u32) -> bool {
        (z..=12).any(|zz| {
            let (d, r) = (zz - z, self.staged[zz as usize]);
            x << d <= r[1] && ((x + 1) << d) > r[0] && y << d <= r[3] && ((y + 1) << d) > r[2]
        })
    }

    /// Tile (z, x, y), which `s` meets, its points reading `best` (the finest tile staged and there
    /// above it) unless one at or under it is.
    fn visit(&mut self, s: &Reached, z: u8, x: u32, y: u32, best: Option<Tile>) -> Result<(), Unread> {
        if !self.staged_under(z, x, y) {
            self.out.extend(best);
            return Ok(());
        }
        let mut best = best;
        if self.staged(z, x, y) {
            if let Some(t) = self.read.hash(z, x, y)? {
                best = Some((z, x, y, t));
            }
        }
        if z == 12 {
            self.out.extend(best);
            return Ok(());
        }
        for (cx, cy) in [(2 * x, 2 * y), (2 * x + 1, 2 * y), (2 * x, 2 * y + 1), (2 * x + 1, 2 * y + 1)] {
            if s.meets(tile_deg(z + 1, cx, cy)) {
                self.visit(s, z + 1, cx, cy, best)?;
            }
        }
        Ok(())
    }
}

/// The heritage sites and designated areas the units read (crate::heritage), once per pass,
/// coverage, accepted registers meeting it and accepted matches. (2: an area across the
/// antimeridian sliced by its parts; 3: the World Heritage components' `parts` slices too, for the
/// 3D buildings' heritage lists. The `pos` and `areas` slices are the same bytes, so the units'
/// keys don't change. 4: the registers read generically from the input check's accepted version,
/// docs/inputs.md §6.5.)
pub const HERITAGE_SITES_V: u32 = 4;

/// The key of the heritage sites: the pass's areas set, the registers meeting the cover by their
/// keyed digests (`inputs` "heritage": crate::heritage::registers_line, from the records' accepted
/// version, which the agent reads; a plan given no such line keys on the accepted version whole),
/// the matches' accepted version and the coverage. None while the registers have no accepted
/// version, or it can't be read now ("?").
pub fn heritage_sites_key(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    inputs: &BTreeMap<String, String>,
) -> Option<String> {
    let set = m.get(&crate::osmpass::set_name(date, "areas"))?;
    let index = m.get(&crate::inputs::logical(crate::inputs::heritage::UNIT))?;
    let regs: Vec<String> = match inputs.get("heritage").map(String::as_str) {
        Some("?") => return None,
        Some(line) => {
            let cover = crate::heritage::cover_boxes(cov);
            crate::heritage::parse_registers(line)
                .into_iter()
                .filter(|(_, _, b)| b.is_some_and(|b| crate::heritage::meets(&cover, b)))
                .map(|(id, keyed, _)| format!("{id} {keyed}"))
                .collect()
        }
        None => vec![format!("index {index}")],
    };
    let matches =
        m.get(&crate::inputs::logical(crate::inputs::heritage_matches::UNIT)).map(String::as_str).unwrap_or("-");
    Some(h(&[
        &format!("heritage-sites {HERITAGE_SITES_V}"),
        date,
        set,
        &regs.join(","),
        &format!("matches {matches}"),
        &coverage_all(cov),
    ]))
}

/// Why the units wait when the pass has no heritage sites and the registers have no accepted
/// version (no key: just after the publish that made them an input), in words; None otherwise.
/// The hold is the owner's (heritage must not go out wrong), the reason said so it isn't a silent
/// stall.
pub fn heritage_awaits_acceptance(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    inputs: &BTreeMap<String, String>,
) -> Option<String> {
    (!m.contains_key(&crate::heritage::base_logical(date, "heritage-sources")) && heritage_sites_key(cov, date, m, inputs).is_none())
        .then(|| "the heritage registers await their first acceptance (inputs/heritage/ on the NAS: `scenic inputs check heritage`, then `scenic inputs accept heritage`)".to_string())
}

pub fn heritage_sites_work(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
) -> Option<Work> {
    let k = heritage_sites_key(cov, date, m, inputs)?;
    (done.lo.get("heritage-sites").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "heritage-sites".into(), targets: vec![("heritage-sites".into(), k)] })
}

/// Every summit worldwide with its z8 height, once per pass (crate::summits): what the units' peaks
/// read.
pub const SUMMITS_V: u32 = 1;

pub fn summits_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let set = m.get(&crate::osmpass::set_name(date, "summits"))?;
    let (z8, z8m) = (m.get(&crate::terrain_z8::logical())?, m.get(&crate::terrain_z8::max_logical())?);
    let k = h(&[&format!("summits {SUMMITS_V}"), set, z8, z8m]);
    (done.lo.get("summits").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "summits".into(), targets: vec![("summits".into(), k)] })
}

/// The peaks' version (crate::peaks::unit).
pub const PEAKS_V: u32 = 1;

/// The z6 tiles meeting a box (degrees), x wrapping at the antimeridian.
fn tiles_in_wrapped(z: u8, b: [f64; 4]) -> Vec<(u32, u32)> {
    let mut out = crate::stage::tiles_in(z, [b[0].max(-180.0), b[1], b[2].min(180.0), b[3]]);
    if b[0] < -180.0 {
        out.extend(crate::stage::tiles_in(z, [b[0] + 360.0, b[1], 180.0, b[3]]));
    }
    if b[2] > 180.0 {
        out.extend(crate::stage::tiles_in(z, [-180.0, b[1], b[2] - 360.0, b[3]]));
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The coverage's units with candidates, each with its peaks' key: the candidates, the summits,
/// the z8, the terrain hi packs within 30 km of it, across the antimeridian too, and the terrain's
/// version (its peaks' z12: the packs', else AWS's raw tiles, which don't change, processed by the
/// terrain's code as it is now: `terrain_pack::process`, which `TERRAIN_V` versions).
pub fn peaks_keys(cov: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
    peaks_keys_at(cov, date, m, TERRAIN_V)
}

/// `peaks_keys` with the terrain's version given (tests).
fn peaks_keys_at(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, terrain_v: u32) -> Vec<(Unit, String)> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let common = [
        get(&format!("work/summits/{date}")),
        get(&crate::terrain_z8::logical()),
        get(&crate::terrain_z8::max_logical()),
    ]
    .join(",");
    let mut out = Vec::new();
    for (u, _) in pois_keys(cov, date, m) {
        let Some(c) = m.get(&format!("work/pois/{}", u.dash())) else { continue };
        let mut inputs = vec![format!("peaks {PEAKS_V}"), format!("terrain {terrain_v}"), c.clone(), common.clone()];
        for (x, y) in tiles_in_wrapped(6, crate::stage::tile_box_grown(u.z, u.x, u.y, 30.0)) {
            inputs.push(get(&format!("layers/terrain/hi/6-{x}-{y}")).to_string());
        }
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        out.push((u, h(&refs)));
    }
    out
}

/// The candidates' version (crate::candidates, extract `--candidates`): bumping it makes every
/// unit's candidates again, not the units. 2: the trailhead searches reach as far east and west
/// at every latitude; a covered bridge's length only for a line.
pub const POIS_V: u32 = 2;

/// The facts and pageviews of the candidates' items, per pass (dem/items.py).
pub const ITEMS_V: u32 = 1;
/// The road → units index from every unit's road values (the server's whole-road lookups).
pub const ROADUNITS_V: u32 = 1;
/// Rail stops near the built units (pipeline::ovconv::stations_job), and ferries worldwide
/// (ferries_job).
/// (Ferries 2: the looked-up timetables read from the accepted static files, docs/inputs.md §6.7.2.
/// 3: the lines no timetable counts and the routes matching none, `work/ferries/lines`, §6.7.3.)
pub const STATIONS_V: u32 = 1;
pub const FERRIES_V: u32 = 3;
/// The landmark points from the candidates (crate::marksjob).
pub const MARKS_V: u32 = 1;
/// The rest of the heritage chain on the heritage-sites outputs (scenic-build heritage), and the
/// area overlays from it with the marks' World Heritage dots (ovconv::overlays). (2: the registers
/// read generically, their caches the pass's answers alone, docs/inputs.md §6.5. 3: no seeds, the
/// layers' `en` the things' own, §6.5.3.)
pub const HERITAGE_V: u32 = 3;
pub const OVERLAYS_V: u32 = 1;

/// The rail service (crate::rail): the feeds for the coverage, fetched once (dem/railfeeds.py), and
/// trains a day on its rail ways (dem/railgtfs.py, railfreq: global/railfreq). (Rail 2: the static
/// files counted with the feeds, the MTR's among them, docs/inputs.md §6.7.2. 3: the lines in the
/// coverage and the routes matching nothing, `work/rail/lines`, §6.7.3.)
pub const RAIL_FEEDS_V: u32 = 1;
pub const RAIL_V: u32 = 3;

/// The current units' candidates (their content names), for the worldwide jobs' keys.
fn current_pois<'a>(cov: &Coverage, date: &str, m: &'a BTreeMap<String, String>) -> Vec<&'a str> {
    pois_keys(cov, date, m)
        .into_iter()
        .filter_map(|(u, _)| m.get(&format!("work/pois/{}", u.dash())).map(String::as_str))
        .collect()
}

/// The units whose piece meets the coverage, each with its candidates' key: its piece, the
/// coverage over it (the clip) and the pass's hiking-route ends.
pub fn pois_keys(cov: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let ends = get(&format!("work/trailends/{date}"));
    let mut units = Vec::new();
    for (l, c) in m.range(format!("sources/osm/{date}/pieces/")..) {
        let Some(u) = l.strip_prefix(&format!("sources/osm/{date}/pieces/")).and_then(Unit::parse) else { break };
        // (The coverage over the tile and 10 km around it: the clip tests the candidates' points,
        // which are the tile's, and their ways' nodes, which reach a little past it.)
        let near = grown_e7(u.z, u.x, u.y, 10.0);
        if !cov.meets_rect(near) {
            continue;
        }
        units.push((u, h(&[&format!("pois {POIS_V}"), c, &cov.fingerprint(near), ends])));
    }
    units
}

/// How far each region is built: of the units its outlines meet, how many are built as the whole
/// coverage now wants them.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RegionState {
    pub built: usize,
    pub total: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn region_states(
    cov: &Coverage,
    regions: &[(String, Coverage)],
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    reach: Option<&Reaches>,
    tiles: &TerrainTiles,
) -> BTreeMap<String, RegionState> {
    let keys = unit_keys(cov, date, m, reach, tiles);
    regions
        .iter()
        .map(|(id, rc)| {
            let mine: Vec<&(Unit, Option<String>)> =
                keys.iter().filter(|(u, _)| reach.is_some_and(|r| builds(rc, r, *u))).collect();
            // (One whose key can't be worked out now isn't counted as built.)
            let built = mine.iter().filter(|(u, k)| k.is_some() && done.unit.get(&u.slash()) == k.as_ref()).count();
            (id.clone(), RegionState { built, total: mine.len() })
        })
        .collect()
}

/// The assemblies' versions (their keys): bumped on any change to the bytes of the lo packs an
/// assembly makes from the same pieces' mids.
pub const TERRAIN_LO_V: u32 = 1;
/// Slope's 2: an assembly's z6 tiles that aren't its pieces' made from their terrain, never kept
/// from the lo pack as an earlier run stored them, and their z7–8 tiles not kept
/// (crate::slope_pack::assemble).
pub const SLOPE_LO_V: u32 = 2;

/// Terrain's and slope's targets, each with its key, done or not (docs/plan.md §6, Terrain): a
/// piece per z6 tile near the coverage (`coverage_tiles`), an assembly per z3 tile with pieces.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TerrainTargets {
    /// Terrain's pieces ("6/x/y"): keyed on the terrain's version, the coverage within 20 km of the
    /// tile (which decides its tiles), the terrain's declared sources and the water it reads
    /// (`water_key`); `UNKNOWN` while either isn't worked out.
    pub terrain: Vec<(String, String)>,
    /// Its assemblies ("3/x/y"): keyed on their pieces' mids by content ("-" for one without: it
    /// can't be assembled until each has one) and the water it reads (`water_key`); `UNKNOWN`
    /// likewise.
    pub terrain_lo: Vec<(String, String)>,
    /// Slope's pieces ("6/x/y"): keyed on the terrain tiles each can read (`slope_piece_key`); None
    /// while that can't be told (a terrain pack's index unread).
    pub slope: Vec<(String, Option<String>)>,
    /// Its assemblies ("3/x/y"): keyed on their pieces' mids by content and the area's terrain lo
    /// pack.
    pub slope_lo: Vec<(String, String)>,
    /// The z6 tiles ("6/x/y") with a terrain hi pack or mid that the coverage has left (no piece):
    /// "none" pieces, whose runs drop them (`none_piece_key`). Not in `terrain`: terrain's assembly
    /// doesn't wait for them (it reads no hi pack).
    #[serde(default)]
    pub terrain_none: Vec<(String, String)>,
    /// The same of slope's hi packs and mids (their area's slope assembly waits for them, as for
    /// any of its slope pieces left).
    #[serde(default)]
    pub slope_none: Vec<(String, String)>,
    /// The z3 tiles ("3/x/y") with a terrain lo pack that the coverage has left (no piece): "none"
    /// assemblies, whose runs drop it (`none_lo_key`); the root is then made again (its key names
    /// the lo packs).
    #[serde(default)]
    pub terrain_lo_none: Vec<(String, String)>,
    /// The same of slope's lo packs.
    #[serde(default)]
    pub slope_lo_none: Vec<(String, String)>,
}

/// The z3 tile ("3/x/y") of a z6 tile ("6/x/y").
pub fn area_of(piece: &str) -> Option<String> {
    Unit::parse(piece).filter(|u| u.z == 6).map(|u| format!("3/{}/{}", u.x >> 3, u.y >> 3))
}

/// Terrain's and slope's targets (`TerrainTargets`) for the coverage and the build manifest `m`;
/// `tiles`, the terrain packs' indexes the slope pieces' keys read.
pub fn terrain_slope_targets(cov: &Coverage, m: &BTreeMap<String, String>, tiles: &TerrainTiles) -> TerrainTargets {
    // (Kept by `tiles` with what they're worked out from: the coverage's shapes, the terrain packs
    // and the mids by content, the basemap. A plan works them out again only when one changes; not
    // kept while a slope piece's key can't be told.)
    let named = |prefix: &str| {
        m.range(prefix.to_string()..)
            .take_while(|(l, _)| l.starts_with(prefix))
            .map(|(l, c)| format!("{l}={c}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let from = h(&[
        &coverage_all(cov),
        &named("layers/terrain/"),
        &named("layers/slope/"),
        &named("work/terrain-mid/"),
        &named("work/slope-mid/"),
        &named(crate::terrain_water::IDX_PREFIX),
        &crate::terrain_pack::water_source_pin(m).unwrap_or_default(),
        tiles.terrain_sources(m).map_or("-", |p| p.pin.as_str()),
    ]);
    let made = std::cell::RefCell::new(None);
    let kept = tiles.memo("terrain-targets", &from, || {
        let t = terrain_slope_targets_made(cov, m, tiles);
        let whole = t.slope.iter().all(|(_, k)| k.is_some())
            && !t.terrain.iter().chain(&t.terrain_lo).any(|(_, k)| k == UNKNOWN);
        let json = serde_json::to_string(&t).unwrap_or_default();
        *made.borrow_mut() = Some(t);
        if whole {
            Ok(json)
        } else {
            Err(Unread("a slope piece's key, or the water or declared sources a terrain target reads".into()))
        }
    });
    match (made.into_inner(), kept) {
        (Some(t), _) => t,
        (None, Ok(json)) => serde_json::from_str(&json).unwrap_or_else(|_| terrain_slope_targets_made(cov, m, tiles)),
        (None, Err(_)) => terrain_slope_targets_made(cov, m, tiles),
    }
}

/// `terrain_slope_targets`, worked out.
fn terrain_slope_targets_made(cov: &Coverage, m: &BTreeMap<String, String>, tiles: &TerrainTiles) -> TerrainTargets {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    // (The water's source: the latest pass's basemap, its digests' file.)
    let pin = crate::terrain_pack::water_source_pin(m);
    let idx = pin.as_deref().and_then(|p| tiles.water_idx(m, p));
    let water = |t: &str, from: &str| water_key(pin.as_deref(), idx, t, from);
    // (The terrain's declared sources: its pieces wait while there are none.)
    let sources = tiles.terrain_sources(m).map(|p| p.pin.as_str());
    let mut out = TerrainTargets::default();
    let near = coverage_tiles(cov);
    for (q, ts) in &near {
        let qs = format!("3/{}/{}", q.0, q.1);
        let (mut tmids, mut smids) = (Vec::new(), Vec::new());
        for &(x, y) in ts {
            let t = format!("6/{x}/{y}");
            let fp = cov.fingerprint(grown_e7(6, x, y, 20.0));
            let hi = crate::terrain_pack::makes_hi(cov, (x, y));
            out.terrain.push((
                t.clone(),
                water(&t, &fp)
                    .zip(sources)
                    .map_or_else(|| UNKNOWN.to_string(), |(w, s)| piece_key_made((x, y), &fp, s, &w, hi)),
            ));
            out.slope.push((t.clone(), slope_piece_key((x, y), m, tiles).ok()));
            tmids.push(format!("{t}={}", get(&crate::terrain_pack::mid_logical(x, y))));
            smids.push(format!("{t}={}", get(&crate::slope_pack::mid_logical(x, y))));
        }
        out.terrain_lo.push((
            qs.clone(),
            water(&qs, crate::terrain_water::AREA_FROM)
                .map_or_else(|| UNKNOWN.to_string(), |w| terrain_lo_key(*q, &w, &tmids)),
        ));
        out.slope_lo.push((qs.clone(), slope_lo_key(*q, &smids, get(&format!("layers/terrain/lo/3-{}-{}", q.0, q.1)))));
    }
    // The z6 tiles the coverage has left with a hi pack or a mid still named: "none" pieces.
    let pieces: BTreeSet<(u32, u32)> = near.values().flatten().copied().collect();
    let left = |step: &str, prefixes: [&str; 2]| -> Vec<(String, String)> {
        let mut ts: BTreeSet<(u32, u32)> = BTreeSet::new();
        for p in prefixes {
            ts.extend(
                m.range(p.to_string()..)
                    .take_while(|(l, _)| l.starts_with(p))
                    .filter_map(|(l, _)| Unit::parse(&l[p.len()..]))
                    .filter(|u| u.z == 6)
                    .map(|u| (u.x, u.y)),
            );
        }
        ts.into_iter()
            .filter(|t| !pieces.contains(t))
            .map(|(x, y)| format!("6/{x}/{y}"))
            .map(|t| (t.clone(), none_piece_key(step, &t)))
            .collect()
    };
    out.terrain_none = left("terrain", ["layers/terrain/hi/", "work/terrain-mid/"]);
    out.slope_none = left("slope", ["layers/slope/hi/", "work/slope-mid/"]);
    // The z3 tiles the coverage has left with a lo pack still named: "none" assemblies.
    let lo_left = |layer: &str| -> Vec<(String, String)> {
        let p = format!("layers/{layer}/lo/");
        m.range(p.clone()..)
            .take_while(|(l, _)| l.starts_with(&p))
            .filter_map(|(l, _)| Unit::parse(&l[p.len()..]))
            .filter(|u| u.z == 3 && !near.contains_key(&(u.x, u.y)))
            .map(|u| format!("3/{}/{}", u.x, u.y))
            .map(|q| (q.clone(), none_lo_key(layer, &q)))
            .collect()
    };
    out.terrain_lo_none = lo_left("terrain");
    out.slope_lo_none = lo_left("slope");
    out
}

/// The key of a terrain or slope assembly (of `layer`) of a z3 tile the coverage has left ("3/x/y"),
/// with a lo pack still named: its run drops it (crate::terrain_pack::drop_lo,
/// crate::slope_pack::drop_lo).
pub fn none_lo_key(layer: &str, q: &str) -> String {
    let v = if layer == "terrain" { TERRAIN_LO_V } else { SLOPE_LO_V };
    h(&[&format!("{layer}-lo {v}"), q, "none"])
}

/// The key of a terrain or slope piece (`step`) the coverage has left ("6/x/y"), with a hi pack or a
/// mid still named: its run drops them (crate::terrain_pack::drop_piece,
/// crate::slope_pack::drop_piece).
pub fn none_piece_key(step: &str, t: &str) -> String {
    let v = if step == "terrain" { TERRAIN_V } else { SLOPE_V };
    h(&[&format!("{step} {v}"), t, "none"])
}

/// A terrain piece's key as the targets have it: `terrain_piece_key`, and for a piece that makes no
/// hi tile (`hi` false: crate::terrain_pack::makes_hi) "no hi tiles" after it, its run dropping any
/// hi pack an earlier run left (so a record from before that drop isn't current).
pub fn piece_key_made(t: (u32, u32), fp: &str, sources: &str, water: &str, hi: bool) -> String {
    let k = terrain_piece_key(t, fp, sources, water);
    if hi {
        k
    } else {
        h(&[&k, "no hi tiles"])
    }
}

/// A terrain target's key while the water it reads isn't worked out (the `terrain-water` job hasn't
/// made its digest yet, or the digests can't be read now): it's neither current nor made, and its
/// area waits (`terrain_work`).
pub const UNKNOWN: &str = "?";

/// The terrain-water job's version (`terrain_water_work`): bumped with any change to the digests it
/// makes (crate::terrain_water::tiles_digest), which changes every terrain key.
pub const TERRAIN_WATER_V: u32 = 1;

/// The water terrain target `t` (a piece "6/x/y", an assembly "3/x/y") reads, as its key names it:
/// "water <digest>" from the water source's digests (`idx`, pinned `pin`) made from `from` (a
/// piece's: the coverage's fingerprint that decides its tiles; an assembly's,
/// crate::terrain_water::AREA_FROM); "-" without a water source (none read); None while its digest
/// isn't there.
pub fn water_key(
    pin: Option<&str>,
    idx: Option<&crate::terrain_water::WaterIdx>,
    t: &str,
    from: &str,
) -> Option<String> {
    match pin {
        None => Some("-".into()),
        Some(_) => idx?.get(t, from).map(|d| format!("water {TERRAIN_WATER_V} {d}")),
    }
}

/// Terrain piece `t`'s key (z6 tile (x, y)): the version, the coverage within 20 km (`fp`: which
/// decides its tiles), the terrain's declared sources (`sources`: crate::terrain_sources::pin) and
/// the water it reads (`water_key`).
pub fn terrain_piece_key(t: (u32, u32), fp: &str, sources: &str, water: &str) -> String {
    h(&[&format!("terrain {TERRAIN_V}"), &format!("6/{}/{}", t.0, t.1), fp, sources, water])
}

/// What the terrain's targets read of the water, as the `terrain-water` job works their digests
/// out (crate::terrain_water::WaterIdx): each piece ("6/x/y") with the coverage's fingerprint that
/// decides its tiles, each assembly ("3/x/y") with AREA_FROM.
pub fn water_wants(cov: &Coverage) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (q, ts) in &coverage_tiles(cov) {
        for &(x, y) in ts {
            out.push((format!("6/{x}/{y}"), cov.fingerprint(grown_e7(6, x, y, 20.0))));
        }
        out.push((format!("3/{}/{}", q.0, q.1), crate::terrain_water::AREA_FROM.to_string()));
    }
    out
}

/// `water_wants` with the tiles each reads: a piece's z9–12 (crate::terrain_pack::piece_levels),
/// an assembly's z6–8 (crate::terrain_water::area_tiles).
pub fn water_reads(cov: &Coverage) -> Vec<crate::terrain_water::Reads> {
    water_wants(cov)
        .into_iter()
        .map(|(t, from)| {
            let u = Unit::parse(&t).unwrap();
            let tiles = if u.z == 6 {
                crate::terrain_pack::piece_levels(cov, (u.x, u.y))
                    .into_iter()
                    .flat_map(|(z, ts)| ts.into_iter().map(move |(x, y)| (z, x, y)))
                    .collect()
            } else {
                crate::terrain_water::area_tiles((u.x, u.y))
            };
            (t, from, tiles)
        })
        .collect()
}

/// The terrain-water job (one, "terrain-water"): the water source's digests of what the terrain's
/// targets read (`water_wants`), when its digests' file lacks one of them (the source new, or the
/// coverage grown); keyed on its version, the source's pin and what's wanted. None with nothing
/// lacking, no water source, or once run for the same.
pub fn terrain_water_work(
    cov: &Coverage,
    m: &BTreeMap<String, String>,
    done: &Keys,
    tiles: &TerrainTiles,
) -> Option<Work> {
    let pin = crate::terrain_pack::water_source_pin(m)?;
    let mut wants = water_wants(cov);
    // (The depth's targets' too, once its sources are read: crate::depth.)
    if let Some(d) = tiles.depth(m) {
        wants.extend(depth_water_reads(cov, &d).into_iter().map(|(t, f, _)| (t, f)));
        wants.sort();
        wants.dedup();
    }
    let idx = tiles.water_idx(m, &pin);
    if wants.iter().all(|(t, from)| idx.is_some_and(|w| w.get(t, from).is_some())) {
        return None;
    }
    // (A file the manifest names that can't be read now isn't made again for it: its targets wait.)
    if m.contains_key(&crate::terrain_water::idx_logical(&pin)) && idx.is_none() {
        return None;
    }
    let mut parts = vec![format!("terrain-water {TERRAIN_WATER_V}"), pin];
    parts.extend(wants.iter().map(|(t, f)| format!("{t} {f}")));
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    let k = h(&refs);
    (done.lo.get("terrain-water").map(String::as_str) != Some(k.as_str()))
        .then(|| Work { step: "terrain-water".into(), targets: vec![("terrain-water".into(), k)] })
}

/// Terrain's assembly of z3 tile `q`'s key: `mids`, its pieces' as "6/x/y=<content name | ->", in
/// column then row order; `water`, the water it reads (`water_key`).
pub fn terrain_lo_key(q: (u32, u32), water: &str, mids: &[String]) -> String {
    h(&[
        &format!("terrain-lo {TERRAIN_LO_V}"),
        &format!("terrain {TERRAIN_V}"),
        &format!("3/{}/{}", q.0, q.1),
        water,
        &mids.join(","),
    ])
}

/// Slope's assembly of z3 tile `q`'s key: `mids` as `terrain_lo_key`'s; `terrain_lo`, the area's
/// terrain lo pack's content name.
pub fn slope_lo_key(q: (u32, u32), mids: &[String], terrain_lo: &str) -> String {
    h(&[
        &format!("slope-lo {SLOPE_LO_V}"),
        &format!("slope {SLOPE_V}"),
        &format!("3/{}/{}", q.0, q.1),
        &mids.join(","),
        terrain_lo,
    ])
}

/// Slope piece `t`'s key (z6 tile (x, y)): the step's version, its terrain hi pack (which tiles of
/// it there are: what it works out) and every terrain tile it can read, by content
/// (`slope_piece_reads`). Kept by `tiles` with the packs those can be in.
pub fn slope_piece_key(t: (u32, u32), m: &BTreeMap<String, String>, tiles: &TerrainTiles) -> Result<String, Unread> {
    let ts = format!("6/{}/{}", t.0, t.1);
    let hi = m.get(&format!("layers/terrain/hi/6-{}-{}", t.0, t.1)).map(String::as_str).unwrap_or("-").to_string();
    tiles.memo(&format!("slope {ts}"), &slope_piece_from(t, m), || {
        let reads = slope_piece_reads(t, m, tiles)?;
        let lines: Vec<String> =
            reads.iter().map(|r| r.map_or("-".to_string(), |(z, x, y, h)| format!("{z}/{x}/{y} {h:016x}"))).collect();
        let mut parts = vec![format!("slope {SLOPE_V}"), ts.clone(), hi.clone()];
        parts.extend(lines);
        let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
        Ok(h(&refs))
    })
}

/// What slope piece `t`'s reads (`slope_piece_reads`) are worked out from, as a digest, for
/// `TerrainTiles::memo`: the content names of the packs they can be in (its own and its edge
/// neighbours' hi packs, their areas' lo packs, the root).
pub fn slope_piece_from(t: (u32, u32), m: &BTreeMap<String, String>) -> String {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-").to_string();
    let mut packs: BTreeSet<String> = BTreeSet::new();
    for (x, y) in std::iter::once(t).chain(edge_neighbours(6, t.0, t.1)) {
        packs.insert(get(&format!("layers/terrain/hi/6-{x}-{y}")));
        packs.insert(get(&format!("layers/terrain/lo/3-{}-{}", x >> 3, y >> 3)));
    }
    packs.insert(get("layers/terrain/root/0-0-0"));
    let from = format!(
        "{}\n{}",
        get(&format!("layers/terrain/hi/6-{}-{}", t.0, t.1)),
        packs.into_iter().collect::<Vec<_>>().join(",")
    );
    store::naming::hash16(from.as_bytes())
}

/// The tiles west, east, north and south of (z, x, y) (x wrapping, none past the poles), as
/// crate::slope_pack::slope_tile reads them.
pub fn edge_neighbours(z: u8, x: u32, y: u32) -> Vec<(u32, u32)> {
    let n = 1u32 << z;
    let mut v = vec![((x + n - 1) % n, y), ((x + 1) % n, y)];
    if y > 0 {
        v.push((x, y - 1));
    }
    if y + 1 < n {
        v.push((x, y + 1));
    }
    v
}

/// The terrain tiles slope piece `t` (z6 tile (x, y)) can read, by content, sorted, without
/// repeats (None: one reading no tile). It works out the tiles of its terrain hi pack (z9–12) and
/// their ancestors down to z6, and its z6 tile (crate::slope_pack::piece); of those, each z12 one and
/// each its children don't all cover reads its own terrain and that of the tiles west, east, north
/// and south of it, at its zoom (Horn's method's border), each resolved as crate::slope_pack::Terrain
/// resolves it, the tile or its nearest ancestor up to eight levels up (`TerrainTiles::resolve`):
/// the tiles along its edges in its neighbours' packs, others' areas' and the root too. (A stored
/// tile that doesn't decode reads as its ancestor: named by its own content here.)
pub fn slope_piece_reads(
    t: (u32, u32),
    m: &BTreeMap<String, String>,
    tiles: &TerrainTiles,
) -> Result<Vec<Option<Tile>>, Unread> {
    let mut r = tiles.reader(m);
    let mut set: BTreeSet<(u8, u32, u32)> = BTreeSet::new();
    set.insert((6, t.0, t.1));
    for z in 9..=12u8 {
        let s = 1u32 << (z - 6);
        for x in t.0 * s..(t.0 + 1) * s {
            for y in t.1 * s..(t.1 + 1) * s {
                if r.hash(z, x, y)?.is_some() {
                    for dz in 0..=(z - 6) {
                        set.insert((z - dz, x >> dz, y >> dz));
                    }
                }
            }
        }
    }
    let mut read: BTreeSet<(u8, u32, u32)> = BTreeSet::new();
    for &(z, x, y) in &set {
        let full = z < 12 && (0..4u32).all(|k| set.contains(&(z + 1, 2 * x + (k & 1), 2 * y + (k >> 1))));
        if !full {
            read.insert((z, x, y));
            read.extend(edge_neighbours(z, x, y).into_iter().map(|(a, b)| (z, a, b)));
        }
    }
    let mut out: BTreeSet<Option<Tile>> = BTreeSet::new();
    for (z, x, y) in read {
        out.insert(r.resolve(z, x, y)?);
    }
    Ok(out.into_iter().collect())
}

/// Terrain's and slope's work as the plan lists it (`terrain_slope_targets`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainWork {
    /// Terrain's pieces to make: those stale, and those current without a mid (in the manifest, or
    /// in a hand-off waiting to be merged) in an area whose assembly will run (it's stale, or a
    /// piece of it is): made again as they are, expected the same; and the "none" pieces not yet
    /// run (`TerrainTargets::terrain_none`), last.
    pub terrain: Vec<(String, String)>,
    /// Its assemblies that can run: stale, every piece of theirs current with its mid; and the
    /// "none" assemblies not yet run (`TerrainTargets::terrain_lo_none`), whenever, last.
    pub terrain_lo: Vec<(String, String)>,
    /// Slope's pieces that can run: stale, their key known, the terrain of their area and of their
    /// edge neighbours' areas built (none of them in `terrain_left`); and the "none" pieces not yet
    /// run (`TerrainTargets::slope_none`), whenever, last.
    pub slope: Vec<(String, String)>,
    /// Its assemblies that can run: stale, every piece of theirs current, their area's terrain
    /// built (one current without a mid has its tiles in the lo pack); and the "none" assemblies
    /// not yet run (`TerrainTargets::slope_lo_none`), whenever, last.
    pub slope_lo: Vec<(String, String)>,
    /// The current pieces without a mid nothing else makes (those the re-keying recorded:
    /// agent::rekey): their mids made in idle time, after all other work, expected the same:
    /// terrain's in areas whose assembly won't run, slope's all.
    pub backfill: Vec<Work>,
    /// The areas whose terrain is to be made: a piece stale or wanted for its mid, or the assembly
    /// stale (what the units and peaks there wait for, and their regions).
    pub terrain_left: BTreeSet<String>,
    /// What a region lacks before it's published, runnable or not: the stale pieces, by target, and
    /// the stale assemblies, by area; and the areas with either, of slope.
    pub terrain_pieces_left: BTreeSet<String>,
    pub terrain_lo_left: BTreeSet<String>,
    pub slope_pieces_left: BTreeSet<String>,
    pub slope_lo_left: BTreeSet<String>,
    pub slope_left: BTreeSet<String>,
}

/// Terrain's and slope's work (`TerrainWork`) for their targets `tt`, the manifest `m` and what was
/// done (with the hand-offs waiting to be merged: a piece current by one of them has its mid
/// there, not in `m`, until it's merged).
pub fn terrain_work(tt: &TerrainTargets, m: &BTreeMap<String, String>, done: &Keys) -> TerrainWork {
    let has_mid = |mid: &dyn Fn(u32, u32) -> String, t: &str| {
        Unit::parse(t).is_some_and(|u| m.contains_key(&mid(u.x, u.y)) || done.handed.contains(&mid(u.x, u.y)))
    };
    let tmid = |x, y| crate::terrain_pack::mid_logical(x, y);
    let smid = |x, y| crate::slope_pack::mid_logical(x, y);
    let mut w = TerrainWork::default();
    // Terrain.
    // (A target whose key isn't known yet, UNKNOWN, is neither current nor made: its area waits.)
    let current = |t: &str, k: &str| k != UNKNOWN && done.terrain.get(t).map(String::as_str) == Some(k);
    // (A "none" piece, the coverage gone from its z6 tile, counts as one left until its run has
    // dropped its hi pack and mid: the units reading its tiles wait for it.)
    w.terrain_pieces_left =
        tt.terrain.iter().chain(&tt.terrain_none).filter(|(t, k)| !current(t, k)).map(|p| p.0.clone()).collect();
    let lo_none: Vec<(String, String)> =
        tt.terrain_lo_none.iter().filter(|(q, k)| done.terrain_lo.get(q) != Some(k)).cloned().collect();
    w.terrain_lo_left = tt
        .terrain_lo
        .iter()
        .filter(|(q, k)| k == UNKNOWN || done.terrain_lo.get(q) != Some(k))
        .map(|l| l.0.clone())
        .chain(lo_none.iter().map(|l| l.0.clone()))
        .collect();
    let changing: BTreeSet<String> =
        w.terrain_lo_left.iter().cloned().chain(w.terrain_pieces_left.iter().filter_map(|t| area_of(t))).collect();
    let mut idle = Vec::new();
    for (t, k) in &tt.terrain {
        let needs_mid = current(t, k) && !has_mid(&tmid, t);
        if k == UNKNOWN {
            continue;
        }
        if !current(t, k) || (needs_mid && area_of(t).is_some_and(|q| changing.contains(&q))) {
            w.terrain.push((t.clone(), k.clone()));
        } else if needs_mid {
            idle.push((t.clone(), k.clone()));
        }
    }
    w.terrain.extend(tt.terrain_none.iter().filter(|(t, k)| !current(t, k)).cloned());
    w.terrain_lo = tt
        .terrain_lo
        .iter()
        .filter(|(q, k)| {
            k != UNKNOWN
                && w.terrain_lo_left.contains(q)
                && tt
                    .terrain
                    .iter()
                    .filter(|(t, _)| area_of(t).as_deref() == Some(q.as_str()))
                    .all(|(t, k)| current(t, k) && has_mid(&tmid, t))
        })
        .cloned()
        .collect();
    // (A "none" assembly reads nothing: it runs whenever, its area left until it has.)
    w.terrain_lo.extend(lo_none);
    w.terrain_left = changing;
    if !idle.is_empty() {
        w.backfill.push(Work { step: "terrain".into(), targets: idle });
    }
    // Slope.
    let scurrent = |t: &str, k: &Option<String>| k.as_ref().is_some_and(|k| done.slope.get(t) == Some(k));
    let snone: Vec<(String, String)> =
        tt.slope_none.iter().filter(|(t, k)| done.slope.get(t) != Some(k)).cloned().collect();
    w.slope_pieces_left = tt
        .slope
        .iter()
        .filter(|(t, k)| !scurrent(t, k))
        .map(|p| p.0.clone())
        .chain(snone.iter().map(|p| p.0.clone()))
        .collect();
    let slo_none: Vec<(String, String)> =
        tt.slope_lo_none.iter().filter(|(q, k)| done.slope_lo.get(q) != Some(k)).cloned().collect();
    w.slope_lo_left = tt
        .slope_lo
        .iter()
        .filter(|(q, k)| done.slope_lo.get(q) != Some(k))
        .map(|l| l.0.clone())
        .chain(slo_none.iter().map(|l| l.0.clone()))
        .collect();
    w.slope_left =
        w.slope_lo_left.iter().cloned().chain(w.slope_pieces_left.iter().filter_map(|t| area_of(t))).collect();
    let built = |t: &str| {
        Unit::parse(t).is_some_and(|u| {
            std::iter::once((u.x, u.y))
                .chain(edge_neighbours(6, u.x, u.y))
                .all(|(x, y)| !w.terrain_left.contains(&format!("3/{}/{}", x >> 3, y >> 3)))
        })
    };
    let mut idle = Vec::new();
    for (t, k) in &tt.slope {
        let Some(k) = k else { continue };
        if !scurrent(t, &Some(k.clone())) {
            if built(t) {
                w.slope.push((t.clone(), k.clone()));
            }
        } else if !has_mid(&smid, t) {
            idle.push((t.clone(), k.clone()));
        }
    }
    // (A "none" piece reads no terrain: it runs whenever.)
    w.slope.extend(snone);
    w.slope_lo = tt
        .slope_lo
        .iter()
        .filter(|(q, _)| {
            w.slope_lo_left.contains(q)
                && !w.terrain_left.contains(q)
                && !w.slope_pieces_left.iter().any(|t| area_of(t).as_deref() == Some(q.as_str()))
        })
        .cloned()
        .collect();
    w.slope_lo.extend(slo_none);
    if !idle.is_empty() {
        w.backfill.push(Work { step: "slope".into(), targets: idle });
    }
    w
}

/// Tree cover's work as the plan lists it (crate::treepacks::targets: a piece per z6 tile, an
/// assembly per z3 tile from its pieces' mids).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeWork {
    /// The pieces to make: those stale, and those current without a mid (in the manifest, or in a
    /// hand-off waiting to be merged: made again as they are, expected the same) in an area whose
    /// assembly will run: it's stale, or a piece of it is.
    pub pieces: Vec<(String, String)>,
    /// The assemblies that can run: stale, every piece of theirs current with its mid (one of
    /// "none", whenever).
    pub lo: Vec<(String, String)>,
    /// The current pieces without a mid in an area whose assembly won't run (those the re-keying
    /// recorded: agent::rekey): their mids made in idle time, after all other work, expected the
    /// same (docs/plan.md §8, Order).
    pub backfill: Vec<(String, String)>,
    /// What a region lacks before it's published: the stale pieces and assemblies, runnable or not.
    pub stale_pieces: BTreeSet<String>,
    pub stale_lo: BTreeSet<String>,
}

/// Tree cover's work (`TreeWork`) for its targets `tt`, the manifest `m` and what was done (with
/// the hand-offs waiting to be merged: a piece current by one of them has its mid there, not in
/// `m`, until it's merged: not made again for it, while its assembly waits for it).
pub fn tree_work(tt: &crate::treepacks::Targets, m: &BTreeMap<String, String>, done: &Keys) -> TreeWork {
    use crate::treepacks::{area_of, mid_logical};
    let current = |t: &str, k: &str| done.trees.get(t).map(String::as_str) == Some(k);
    let has_mid = |t: &str| Unit::parse(t).is_some_and(|u| m.contains_key(&mid_logical(u.x, u.y)));
    let mid_handed = |t: &str| Unit::parse(t).is_some_and(|u| done.handed.contains(&mid_logical(u.x, u.y)));
    let stale_pieces: BTreeSet<String> =
        tt.pieces.iter().filter(|(t, k, _)| !current(t, k)).map(|p| p.0.clone()).collect();
    let stale_lo: BTreeSet<String> =
        tt.lo.iter().filter(|(q, k, _)| done.trees_lo.get(q) != Some(k)).map(|l| l.0.clone()).collect();
    let changing: BTreeSet<String> =
        stale_lo.iter().cloned().chain(stale_pieces.iter().filter_map(|t| area_of(t))).collect();
    let mut w = TreeWork::default();
    for (t, k, none) in &tt.pieces {
        let needs_mid = !none && current(t, k) && !has_mid(t) && !mid_handed(t);
        if !current(t, k) || (needs_mid && area_of(t).is_some_and(|q| changing.contains(&q))) {
            w.pieces.push((t.clone(), k.clone()));
        } else if needs_mid {
            w.backfill.push((t.clone(), k.clone()));
        }
    }
    w.lo = tt
        .lo
        .iter()
        .filter(|(q, _, none)| {
            stale_lo.contains(q) && (*none || tt.pieces_of(q).all(|(t, k, _)| current(t, k) && has_mid(t)))
        })
        .map(|(q, k, _)| (q.clone(), k.clone()))
        .collect();
    w.stale_pieces = stale_pieces;
    w.stale_lo = stale_lo;
    w
}

/// A catalog at most this often (seconds; the owner's, 17 Oct): what's drawn meanwhile goes out
/// with the next.
pub const CATALOG_EVERY_S: u64 = 60;

/// With the low zooms later (`Publishing::low_later`): how long after they were last made they're
/// made again while units are still being built (the owner's "every few hours"; a policy, not a
/// measure). Sooner when much has changed (`low_due`), and once everything is built.
pub const LOW_EVERY_S: u64 = 3 * 3600;

/// How publishing goes, as the switches under `state/pool/` say (crate::agent::publish).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Publishing {
    /// The low zooms later (`state/pool/low-zoom-later`): the roads' and rails' lo packs and the
    /// terrain's and slope's roots are made only `LOW_EVERY_S` after they were last made
    /// (`since_low`), once half the lo packs are stale (`low_due`), or once everything is built;
    /// the map tiles (z9 up) go out meanwhile with the low zooms as they were last made.
    pub low_later: bool,
    /// Seconds since a catalog last went out with the low zooms made (None: none kept).
    pub since_low: Option<u64>,
    /// A done region's slope and tree cover by the pieces near it (`state/pool/near-slope`): the
    /// slope pieces whose z6 tile is within 20 km of it (what its slope reads), the tree cover
    /// pieces meeting it; not every stale piece of the z3 tiles around it. Their assemblies (z3–8)
    /// are made with the low zooms (always, without `low_later`), and a region is done with its
    /// pieces.
    pub near: bool,
}

/// The lead's record of its catalogs (its folder's `publishing.json`): when the last went out, and
/// the last that went out with the low zooms made (`Publishing::since_low`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LastCatalog {
    /// When the last catalog was made (seconds since the epoch).
    pub at: u64,
    /// When the last catalog made with nothing left of the low zooms (`Plan::low`) was made.
    #[serde(default)]
    pub low_at: Option<u64>,
}

/// Publishing as the plan has it now, for the agent's log and the build status (`publishing`;
/// docs/plan.md §8, Publishing).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishLine {
    /// What publishing does now, or waits for, in plain words.
    pub why: String,
    /// The map tiles to draw (pack(T) targets whose owners changed), and the units whose change made
    /// them stale (`by_unit`).
    #[serde(default)]
    pub tiles: usize,
    #[serde(default)]
    pub units: usize,
    /// The regions done whose catalog hasn't them as they are now, each with what it still waits
    /// for.
    #[serde(default)]
    pub regions: Vec<PublishRegion>,
    /// The low zooms are made now (`Publishing::low_later` off, or theirs due).
    #[serde(default)]
    pub low: bool,
    /// Seconds until the next catalog may be made (`CATALOG_EVERY_S`; 0: it may now).
    #[serde(default)]
    pub wait_s: u64,
}

/// A region done whose catalog hasn't it as it is now (`PublishLine::regions`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PublishRegion {
    pub id: String,
    /// "new region" or "rebuilt" (its outline or units changed), and "slope and tree cover to make"
    /// while those wait.
    pub kinds: Vec<String>,
}

impl PublishLine {
    /// One line for the agent's log.
    pub fn line(&self) -> String {
        let regions = if self.regions.is_empty() {
            String::new()
        } else {
            format!(
                "; done, not on the map as they are: {}",
                self.regions
                    .iter()
                    .map(|r| format!("{} ({})", r.id, r.kinds.join(", ")))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        format!(
            "publishing: {}{regions}{}",
            self.why,
            if self.low { "" } else { "; the low zooms wait for their turn" }
        )
    }

    /// What the log line is logged again on: what publishing does and why, not the seconds counting
    /// down nor the tiles' count as they're drawn, nor an error's text in what it waits for.
    pub fn key(&self) -> String {
        let why = self.why.split(" (").next().unwrap_or("");
        format!("{why} {:?} {}", self.regions, self.low)
    }
}

/// What the plan builds the regions by, one at a time, and publishes them by, as they're done:
/// each region's own coverage, as the recipes list them (crate::coverage::Coverage::by_region); the
/// regions the map's catalog has, and its units; when the last catalog went out.
#[derive(Clone, Copy)]
pub struct Publish<'a> {
    pub each: &'a [(String, Coverage)],
    /// The last catalog's regions by id: whether it has the region as its recipe is now (false: an
    /// older outline, the region on the map as it was).
    pub on_map: &'a BTreeMap<String, bool>,
    /// The served catalog's units' base packs and road values, by logical name
    /// (crate::pubtimes::served_units): the map tiles to draw are grouped by the unit whose change
    /// made them stale (`by_unit`).
    pub served: &'a BTreeMap<String, String>,
    /// Seconds since the last catalog went out (or one last started); None when none has.
    pub since_catalog: Option<u64>,
    /// Catalogs are held for review (`inputs/hold-catalog`): what's new is weighed against the last
    /// held one's.
    pub held: bool,
    /// How publishing goes (the switches).
    pub way: Publishing,
}

/// The work there is, in order, and the regions a catalog made now records as built (`ready`: every
/// unit of theirs built as the coverage wants it, and their areas' slope and tree cover; the
/// catalog keeps the others as the last one had them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub work: Vec<Work>,
    pub ready: Vec<String>,
    /// Every region's work left, in the order they're built (those with none last), whether or not
    /// it can start now: what the forecast schedules (crate::agent::forecast).
    pub regions: Vec<RegionLeft>,
    /// The catalog planned goes out with the low zooms made (`Publishing::low_later`: they were
    /// due, and nothing's left of the publishing chain, or no lo pack is stale), noted so once made
    /// (`LastCatalog::low_at`). Without a catalog planned: the low zooms are made now.
    pub low: bool,
    /// The units whose key can't be worked out now (a terrain pack's index unread: `unit_keys`):
    /// neither built nor counted as built until it can.
    pub unknown: Vec<String>,
    /// The pieces whose mids are made in idle time (`TreeWork::backfill`, `TerrainWork::backfill`:
    /// the work's last): for the forecast.
    pub backfill: Vec<Work>,
    /// Publishing as planned now, for the log and the status.
    pub publishing: PublishLine,
}

/// A region's work left (`Plan::regions`), as targets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegionLeft {
    pub id: String,
    /// Whether the map's catalog has it: as its recipe is now (true), as it was (false), or not at
    /// all (None).
    pub on_map: Option<bool>,
    /// Its units not built as the coverage wants them, all of them (a region is done when they
    /// are), and those it builds itself (the rest come with a region before it), in the order
    /// they're built.
    pub units: Vec<String>,
    pub own_units: Vec<String>,
    /// The terrain pieces of the areas it reads to make (stale, or their mids for an assembly),
    /// and those it makes itself (the rest come with a region before it); those areas' stale
    /// assemblies, and those it makes itself.
    pub terrain: Vec<String>,
    pub own_terrain: Vec<String>,
    #[serde(default)]
    pub terrain_lo: Vec<String>,
    #[serde(default)]
    pub own_terrain_lo: Vec<String>,
    /// Its areas' stale slope pieces and assemblies; the tree cover pieces of its areas to make
    /// (stale, or their mids for an assembly), and its areas whose tree cover assembly is stale.
    pub slope: Vec<String>,
    #[serde(default)]
    pub slope_lo: Vec<String>,
    pub trees: Vec<String>,
    #[serde(default)]
    pub trees_lo: Vec<String>,
    /// Its units built as the coverage wants them now but reading terrain that's stale: whether
    /// each goes stale too is known only once that terrain is built (its key names the tiles'
    /// contents). For the forecast, which takes a share of them as work to come
    /// (`forecast::EXPECTED_STALE`).
    #[serde(default)]
    pub expected: Vec<String>,
}

/// The plan for the coverage `cov`, the pass of `date`, the build manifest `m` (logical → content)
/// and what was done (`done`). `inputs`: digests of what jobs read from `inputs/` (not in the
/// manifest), by name: "keys" (the key names inputs/keys.env holds), "regions" (the regions' accepted version). `tiles`: the
/// terrain packs' indexes, which the units' keys read (`unit_terrain`). The agent runs
/// the first work not waiting out a failure; a helper takes a step's from the far end (the agent
/// offers each step's targets together, in this order). The order (docs/plan.md §8, Order):
/// - the heritage sites the units read;
/// - publishing, as soon as the records change (`publish_chain`): what the coverage no longer
///   builds pruned, the map tiles whose owners changed (a unit's together: `by_unit`), the road
///   index, the low zooms (`Publishing::low_later`), rail stops and ferries, the world-level terrain
///   and slope; once none of that is left, a catalog when what it would list changed
///   (`catalog_key`), at most one each `CATALOG_EVERY_S`;
/// - the slope and tree cover of the regions done that the map hasn't as they are now;
/// - a region at a time (the regions the map hasn't at all first, the one with the fewest units
///   left first): the terrain areas it reads that are stale, then its units whose terrain is built
///   (a unit's key reads the terrain near it: one built before would be built again);
/// - slope (each area once its terrain is built) and tree cover after them (its pieces, then each
///   area's assembly once its pieces are built: `tree_work`);
/// - the trains' and the landmarks' chains from the start, each step once what it reads is built,
///   after all that in the order (a second job beside the regions' takes them: crate::agent), the
///   overlays after the last unit; what they make goes out with the next catalog;
/// - the 3D buildings' chain from the start too (`bld_work`: the sources' fetch, each z6 tile's
///   normalized file, then its tiles once it and its neighbours are prepared), listed after the
///   other chains, its tiles in the regions' order; it holds no region;
/// - last, in idle time, the mids of current tree cover pieces without one (`TreeWork::backfill`).
#[allow(clippy::too_many_arguments)]
pub fn plan(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
    reach: Option<&Reaches>,
    tiles: &TerrainTiles,
    updates: Publish,
) -> Plan {
    let mut work = Vec::new();
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    let push = |work: &mut Vec<Work>, step: &str, targets: Vec<(String, String)>| {
        if !targets.is_empty() {
            work.push(Work { step: step.into(), targets });
        }
    };

    // The heritage sites and designated areas the units read: first, once per pass and coverage (one
    // job). Not waited for: terrain still runs while it waits out a failure.
    let sites = heritage_sites_work(cov, date, m, done, inputs);
    let sites_pending = sites.is_some();
    work.extend(sites);
    // What the terrain's targets read of the water, as digests (one job, minutes, once a basemap
    // and again as the coverage grows): the terrain whose keys need them waits for it.
    work.extend(terrain_water_work(cov, m, done, tiles));

    // Terrain and slope (pieces per z6 tile, then each z3 tile's assembly; slope's once the terrain
    // it reads is built) and the tree cover layers (likewise), what a region needs before it's
    // published.
    let tw = terrain_work(&terrain_slope_targets(cov, m, tiles), m, done);
    let TerrainWork { terrain, terrain_lo, slope, slope_lo, terrain_left, slope_left, .. } = tw.clone();
    let tt = crate::treepacks::targets_with(cov, m, tiles.keyed(m).and_then(|k| k.trees.as_deref()));
    let TreeWork {
        pieces: trees,
        lo: trees_lo,
        backfill: tree_backfill,
        stale_pieces: trees_left,
        stale_lo: trees_lo_left,
    } = tree_work(&tt, m, done);
    let mut backfill: Vec<Work> = Vec::new();
    if !tree_backfill.is_empty() {
        backfill.push(Work { step: "trees".into(), targets: tree_backfill });
    }
    backfill.extend(tw.backfill.iter().cloned());
    // A tree cover, terrain or slope target's area (a piece's z3 tile, an assembly's own).
    let tree_area = |t: &(String, String)| crate::treepacks::area_of(&t.0).unwrap_or_else(|| t.0.clone());
    let area = |t: &(String, String)| area_of(&t.0).unwrap_or_else(|| t.0.clone());

    // The units wait for the heritage sites, the pass's reaches (which units the coverage builds)
    // and the release's roadside buildings (a worldwide job, once): the terrain, slope and tree
    // cover meanwhile.
    let waiting = sites_pending
        || !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources"))
        || tiles.buildings_release(m).is_none_or(|b| !m.contains_key(&crate::buildtiles::index_logical(b)));
    // The trains' and the landmarks' chains: from the start, each step once what it reads is built
    // (`landmarks_work`: none waits for the units but the overlays), listed after the regions'
    // work. A second job beside the regions' takes them first (crate::agent, Two jobs at once), a
    // helper the candidates and peaks, the lead's own job once the regions' work is done.
    let chains = |last: bool| -> Vec<Work> {
        rail_chain(cov, date, m, done, inputs)
            .into_iter()
            .chain(landmarks_work(cov, date, m, done, inputs, &terrain_left, last))
            .collect()
    };
    let Some(reach) = reach.filter(|_| !waiting) else {
        push(&mut work, "terrain", terrain);
        push(&mut work, "terrain-lo", terrain_lo);
        push(&mut work, "slope", slope);
        push(&mut work, "slope-lo", slope_lo);
        push(&mut work, "trees", trees);
        push(&mut work, "trees-lo", trees_lo);
        work.extend(chains(false));
        let each: Vec<&Coverage> = updates.each.iter().map(|(_, c)| c).collect();
        work.extend(bld_work(cov, m, done, inputs, &bld_rank(&each)));
        work.extend(depth_work(cov, m, done, tiles));
        work.extend(backfill.iter().cloned());
        let why = match heritage_awaits_acceptance(cov, date, m, inputs) {
            Some(r) => format!("the regions' units wait for the pass's heritage sites: {r}"),
            None => "the regions' units wait for the pass's heritage sites, reaches or roadside buildings".into(),
        };
        let publishing = PublishLine { why, ..Default::default() };
        return Plan { work, backfill, publishing, ..Default::default() };
    };

    // base(U): the units the coverage builds. Each region's: those it builds itself (a road of theirs
    // may touch its outline), the ones of them not built as the coverage now wants, its areas (the
    // z3 tiles its slope and tree cover are in) and the terrain areas it reads (its areas and those
    // near its units).
    let units = unit_keys(cov, date, m, Some(reach), tiles);
    // (One whose key can't be worked out now is left, and isn't built: `unknown`.)
    let unit_stale: Vec<bool> =
        units.iter().map(|(u, k)| k.as_ref().is_none_or(|k| stale(&done.unit, &u.slash(), k))).collect();
    // A unit's terrain areas: those of the z6 tiles near it, whose packs hold the tiles its key
    // reads (every tile it stages is in the lo or hi pack of a z3 tile it meets).
    let reads: Vec<BTreeSet<String>> = units
        .iter()
        .map(|(u, _)| {
            crate::stage::tiles_in(6, crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM))
                .into_iter()
                .map(|(x, y)| format!("3/{}/{}", x >> 3, y >> 3))
                .collect()
        })
        .collect();
    let buildable = |i: usize| units[i].1.is_some() && reads[i].is_disjoint(&terrain_left);
    let unknown: Vec<String> = units.iter().filter(|(_, k)| k.is_none()).map(|(u, _)| u.slash()).collect();
    let target = |i: usize| -> Option<(String, String)> { Some((units[i].0.slash(), units[i].1.clone()?)) };
    struct Region<'a> {
        id: &'a str,
        /// Its units, and those not built as the coverage wants them.
        all: Vec<usize>,
        stale: Vec<usize>,
        /// Its slope's areas (the z3 tiles within 20 km of it, as the terrain's and slope's targets
        /// go) and its tree cover's (those it meets: their assemblies, and every piece of them,
        /// which the assemblies read), and its tree cover's pieces (the z6 tiles it meets).
        areas: BTreeSet<String>,
        tree_areas: BTreeSet<String>,
        tree_pieces: BTreeSet<String>,
        terrain: BTreeSet<String>,
        /// Its slope pieces left whose z6 tile is within 20 km of it (`Publishing::near`).
        slope_near: BTreeSet<String>,
    }
    let regions: Vec<Region> = updates
        .each
        .iter()
        .map(|(id, rc)| {
            let all: Vec<usize> = (0..units.len()).filter(|&i| builds(rc, reach, units[i].0)).collect();
            let stale: Vec<usize> = all.iter().copied().filter(|&i| unit_stale[i]).collect();
            let areas: BTreeSet<String> = (0..8u32)
                .flat_map(|x| (0..8u32).map(move |y| (x, y)))
                .filter(|&(x, y)| rc.meets_rect(grown_e7(3, x, y, 20.0)))
                .map(|(x, y)| format!("3/{x}/{y}"))
                .collect();
            let tree_areas: BTreeSet<String> = areas
                .iter()
                .filter(|a| {
                    crate::legacy::Unit::parse(a)
                        .is_some_and(|q| rc.meets_rect(crate::hipack::tile_bounds(3, q.x, q.y)))
                })
                .cloned()
                .collect();
            let tree_pieces: BTreeSet<String> = tree_areas
                .iter()
                .flat_map(|q| tt.pieces_of(q))
                .filter(|p| Unit::parse(&p.0).is_some_and(|u| rc.meets_rect(crate::hipack::tile_bounds(6, u.x, u.y))))
                .map(|p| p.0.clone())
                .collect();
            let terrain = areas
                .iter()
                .cloned()
                .chain(stale.iter().flat_map(|&i| reads[i].iter().cloned()))
                .filter(|a| terrain_left.contains(a))
                .collect();
            // (Its stale slope pieces whose z6 tile is within 20 km of it: what its slope reads, the
            // near way's.)
            let slope_near = tw
                .slope_pieces_left
                .iter()
                .filter(|t| Unit::parse(t).is_some_and(|u| u.z == 6 && rc.meets_rect(grown_e7(6, u.x, u.y, 20.0))))
                .cloned()
                .collect();
            Region { id, all, stale, areas, tree_areas, tree_pieces, terrain, slope_near }
        })
        .collect();
    // The regions a catalog made now records as built: every unit of theirs built as the coverage
    // wants it, and their areas' slope and tree cover.
    // (The near way: its slope and tree cover pieces near it, their assemblies later.)
    let near = updates.way.near;
    let slope_done = |r: &Region| if near { r.slope_near.is_empty() } else { r.areas.is_disjoint(&slope_left) };
    let ready: Vec<String> = regions
        .iter()
        .filter(|r| {
            r.stale.is_empty()
                && r.terrain.is_empty()
                && slope_done(r)
                && r.tree_pieces.is_disjoint(&trees_left)
                && (near || r.tree_areas.is_disjoint(&trees_lo_left))
        })
        .map(|r| r.id.to_string())
        .collect();

    // A region at a time: those the map hasn't at all first, then those it has (redrawn, or their
    // units' keys changed: on the map as they were meanwhile); of each, the one with the fewest units
    // left first, so regions are done (and published) as soon as they can be. Each region's stale
    // terrain areas, then its units whose terrain is built, neighbours together (`spatial_order`), so
    // the downloads and caches one fills serve the next; a unit or an area two regions share comes
    // with the first. (A unit whose terrain isn't built comes once it is: the plan's made again after
    // each job.)
    let mut order: Vec<&Region> = regions.iter().filter(|r| !r.stale.is_empty() || !r.terrain.is_empty()).collect();
    order.sort_by_key(|r| {
        (
            updates.on_map.contains_key(r.id),
            r.stale.len(),
            r.stale.iter().map(|&i| spatial_order(units[i].0)).min(),
            r.id,
        )
    });
    let mut taken = vec![false; units.len()];
    let mut listed: BTreeSet<String> = BTreeSet::new();
    let mut by_region: Vec<Work> = Vec::new();
    for r in &order {
        let t: Vec<(String, String)> =
            terrain.iter().filter(|t| r.terrain.contains(&area(t)) && listed.insert(t.0.clone())).cloned().collect();
        push(&mut by_region, "terrain", t);
        let t: Vec<(String, String)> = terrain_lo
            .iter()
            .filter(|t| r.terrain.contains(&t.0) && listed.insert(format!("lo {}", t.0)))
            .cloned()
            .collect();
        push(&mut by_region, "terrain-lo", t);
        let mut mine: Vec<usize> = r.stale.iter().copied().filter(|&i| !taken[i] && buildable(i)).collect();
        mine.sort_by_key(|&i| spatial_order(units[i].0));
        for &i in &mine {
            taken[i] = true;
        }
        push(&mut by_region, "unit", mine.into_iter().filter_map(target).collect());
    }
    // Every region's work left, buildable now or not, for the forecast: its units (each with the
    // first region in this order that builds it) and terrain likewise, its slope and tree cover.
    let mut claimed = vec![false; units.len()];
    let mut claimed_terrain: BTreeSet<String> = BTreeSet::new();
    let mut lefts: Vec<RegionLeft> = Vec::new();
    for r in order.iter().copied().chain(regions.iter().filter(|r| r.stale.is_empty() && r.terrain.is_empty())) {
        let mut all = r.stale.clone();
        all.sort_by_key(|&i| spatial_order(units[i].0));
        let own: Vec<usize> = all.iter().copied().filter(|&i| !std::mem::replace(&mut claimed[i], true)).collect();
        let own_terrain: Vec<String> = tw
            .terrain
            .iter()
            .filter(|t| r.terrain.contains(&area(t)) && claimed_terrain.insert(t.0.clone()))
            .map(|t| t.0.clone())
            .collect();
        let own_terrain_lo: Vec<String> = tw
            .terrain_lo_left
            .iter()
            .filter(|q| r.terrain.contains(*q) && claimed_terrain.insert(format!("lo {q}")))
            .cloned()
            .collect();
        lefts.push(RegionLeft {
            id: r.id.to_string(),
            on_map: updates.on_map.get(r.id).copied(),
            units: all.iter().map(|&i| units[i].0.slash()).collect(),
            own_units: own.iter().map(|&i| units[i].0.slash()).collect(),
            terrain: tw.terrain.iter().filter(|t| r.terrain.contains(&area(t))).map(|t| t.0.clone()).collect(),
            own_terrain,
            terrain_lo: tw.terrain_lo_left.iter().filter(|q| r.terrain.contains(*q)).cloned().collect(),
            own_terrain_lo,
            slope: tw
                .slope_pieces_left
                .iter()
                .filter(|t| area_of(t).is_some_and(|a| r.areas.contains(&a)))
                .cloned()
                .collect(),
            slope_lo: tw.slope_lo_left.iter().filter(|q| r.areas.contains(*q)).cloned().collect(),
            trees: trees.iter().filter(|t| r.tree_areas.contains(&tree_area(t))).map(|t| t.0.clone()).collect(),
            trees_lo: r.tree_areas.iter().filter(|a| trees_lo_left.contains(*a)).cloned().collect(),
            expected: r
                .all
                .iter()
                .copied()
                .filter(|&i| !unit_stale[i] && !reads[i].is_disjoint(&terrain_left))
                .map(|i| units[i].0.slash())
                .collect(),
        });
    }

    // (Terrain no region with work left reads, and a unit in no region's own coverage, last.)
    push(&mut by_region, "terrain", terrain.iter().filter(|t| !listed.contains(&t.0)).cloned().collect());
    push(
        &mut by_region,
        "terrain-lo",
        terrain_lo.iter().filter(|t| !listed.contains(&format!("lo {}", t.0))).cloned().collect(),
    );
    let mut rest: Vec<usize> = (0..units.len()).filter(|&i| unit_stale[i] && !taken[i] && buildable(i)).collect();
    rest.sort_by_key(|&i| spatial_order(units[i].0));
    push(&mut by_region, "unit", rest.into_iter().filter_map(target).collect());
    // Slope and tree cover in the same order: the areas of the region built first, first.
    let rank = |t: &(String, String)| order.iter().position(|r| r.areas.contains(&area(t))).unwrap_or(usize::MAX);
    let tree_rank =
        |t: &(String, String)| order.iter().position(|r| r.tree_areas.contains(&tree_area(t))).unwrap_or(usize::MAX);
    let (mut slope, mut slope_lo, mut trees, mut trees_lo) = (slope, slope_lo, trees, trees_lo);
    slope.sort_by_key(rank);
    slope_lo.sort_by_key(rank);
    trees.sort_by_key(tree_rank);
    trees_lo.sort_by_key(tree_rank);

    // Publishing (docs/plan.md §8, Publishing): as soon as the records change, before the regions'
    // work, what goes out with the units: what the coverage no longer builds pruned, then the
    // publishing chain (`publish_chain`: the map tiles whose owners changed, a unit's together, the
    // road index, the low zooms, rail stops and ferries, the world-level terrain and slope); once
    // none of that is left, a catalog when what it would list changed, at most one a minute
    // (`CATALOG_EVERY_S`; with the trains', the landmarks' and the 3D buildings' work made by then).
    let done_now = |r: &Region| r.stale.is_empty() && r.terrain.is_empty();
    // (Done, and not on the map as it is now: its slope and tree cover come first.)
    let to_publish: Vec<&Region> =
        regions.iter().filter(|r| done_now(r) && updates.on_map.get(r.id) != Some(&true)).collect();
    let last_now = !unit_stale.iter().any(|&s| s) && terrain_left.is_empty();
    // The 3D buildings' chain (`bld_work`): its tiles in the regions' order, those the regions
    // being built first.
    let bld_regions: Vec<&Coverage> = order
        .iter()
        .map(|r| r.id)
        .chain(regions.iter().map(|r| r.id))
        .filter_map(|id| updates.each.iter().find(|(x, _)| x == id).map(|(_, c)| c))
        .collect();
    let bld = bld_work(cov, m, done, inputs, &bld_rank(&bld_regions));
    let depth = depth_work(cov, m, done, tiles);
    // The low zooms (the roads' and rails' lo packs, the terrain's and slope's roots): always, or
    // with them later, `LOW_EVERY_S` after they were last made, once half the lo packs are stale, and
    // once everything is built.
    let near = updates.way.near;
    let low = last_now
        || !updates.way.low_later
        || updates.way.since_low.is_none_or(|s| s >= LOW_EVERY_S)
        || low_due(m, done, Some(reach));
    // The terrain's and slope's roots wait for what makes the lo packs they read: terrain's for the
    // terrain left (its pieces' mids, its assemblies), slope's for that and the slope left, so a
    // root isn't made while an assembly runs (or waits to be merged) and again after it.
    let roots_held = if low {
        [!tw.terrain_left.is_empty(), !tw.terrain_left.is_empty() || !tw.slope_left.is_empty()]
    } else {
        [true, true]
    };
    let mut plan_low = low;
    let mut publishing = PublishLine {
        low,
        regions: to_publish
            .iter()
            .map(|r| {
                let mut kinds = vec![if updates.on_map.contains_key(r.id) {
                    "rebuilt".to_string()
                } else {
                    "new region".to_string()
                }];
                if !ready.iter().any(|x| x == r.id) {
                    kinds.push("slope and tree cover to make".into());
                }
                PublishRegion { id: r.id.to_string(), kinds }
            })
            .collect(),
        ..Default::default()
    };
    // (Listed before the regions' work, not holding it: while one waits out a failure, the units
    // go on.)
    match prune_units(cov, date, m, &units) {
        Some(w) => {
            publishing.why = format!("pruning what the coverage no longer builds ({} targets)", w.targets.len());
            work.push(w);
        }
        // (Nothing before a unit is built, while the regions have work left: none of theirs to show
        // yet.)
        None if done.unit.is_empty() && !last_now => {
            publishing.why = "nothing to publish until a unit is built".into();
        }
        None => match publish_chain(date, m, done, Some(reach), roots_held, low) {
            Some(mut w) => {
                if w.step == "pack" {
                    let (targets, n) = by_unit(std::mem::take(&mut w.targets), m, Some(reach), updates.served);
                    w.targets = targets;
                    (publishing.tiles, publishing.units) = (w.targets.len(), n);
                    publishing.why = format!(
                        "drawing the map tiles whose owners changed ({} for {n} unit{})",
                        w.targets.len(),
                        if n == 1 { "" } else { "s" }
                    );
                } else {
                    publishing.why = format!("{} next, then a catalog", super::steps::title(&w.step));
                }
                work.push(w);
            }
            None => match catalog_work(m, done, inputs, &ready, updates.held) {
                Some(w) => {
                    // (With the low zooms due, the chain's done made them; else they're made if no
                    // lo pack is stale.)
                    plan_low = low || pack_lo_targets(m, Some(reach)).1.iter().all(|(t, k)| done.lo.get(t) == Some(k));
                    let wait = updates.since_catalog.map_or(0, |s| CATALOG_EVERY_S.saturating_sub(s));
                    publishing.wait_s = wait;
                    if wait == 0 {
                        publishing.why = "a catalog now: what it would list changed".into();
                        work.push(w);
                    } else {
                        publishing.why = format!("a catalog once a minute has passed since the last (in {wait} s)");
                    }
                }
                None if inputs.get("regions").map(String::as_str) == Some("?") => {
                    publishing.why = "the catalog waits for the regions to be read".into();
                }
                None => publishing.why = "caught up: the catalog has everything built".into(),
            },
        },
    }
    // The slope and tree cover of the regions done that the map hasn't as they are now: before the
    // regions' work, so they're on the map as built soon after their units. (The near way: their
    // pieces near them; the assemblies with the low zooms.)
    let slope_waits = |t: &(String, String)| {
        to_publish.iter().any(|r| if near { r.slope_near.contains(&t.0) } else { r.areas.contains(&area(t)) })
    };
    let slope_lo_waits = |t: &(String, String)| (!near || low) && to_publish.iter().any(|r| r.areas.contains(&area(t)));
    let trees_waits = |t: &(String, String)| {
        to_publish
            .iter()
            .any(|r| if near { r.tree_pieces.contains(&t.0) } else { r.tree_areas.contains(&tree_area(t)) })
    };
    let trees_lo_waits =
        |t: &(String, String)| (!near || low) && to_publish.iter().any(|r| r.tree_areas.contains(&tree_area(t)));
    let (prep, rest): (Vec<_>, Vec<_>) = slope.into_iter().partition(slope_waits);
    push(&mut work, "slope", prep);
    let slope = rest;
    let (prep, rest): (Vec<_>, Vec<_>) = slope_lo.into_iter().partition(slope_lo_waits);
    push(&mut work, "slope-lo", prep);
    let slope_lo = rest;
    let (prep, rest): (Vec<_>, Vec<_>) = trees.into_iter().partition(trees_waits);
    push(&mut work, "trees", prep);
    let trees = rest;
    let (prep, rest): (Vec<_>, Vec<_>) = trees_lo.into_iter().partition(trees_lo_waits);
    push(&mut work, "trees-lo", prep);
    let trees_lo = rest;
    work.extend(by_region);
    push(&mut work, "slope", slope);
    push(&mut work, "slope-lo", slope_lo);
    push(&mut work, "trees", trees);
    push(&mut work, "trees-lo", trees_lo);
    work.extend(chains(last_now));
    work.extend(bld);
    work.extend(depth);
    // Last of all, in idle time: the mids of current pieces that have none (expected the same).
    work.extend(backfill.iter().cloned());
    Plan { work, ready, regions: lefts, low: plan_low, unknown, backfill, publishing }
}

/// The map tiles `targets` to draw, a unit's together: grouped by the unit whose change made each
/// stale (its base pack or road values in `m` not what the served catalog `served` has: the
/// owners as crate::pubtimes::restaled_by takes them), the units in the order they're built
/// (`spatial_order`), a tile two units made stale with the first; those no unit's change explains
/// (a version's change, a reach's) last, in their order. And how many units.
fn by_unit(
    targets: Vec<(String, String)>,
    m: &BTreeMap<String, String>,
    reach: Option<&Reaches>,
    served: &BTreeMap<String, String>,
) -> (Vec<(String, String)>, usize) {
    let at: BTreeMap<&str, usize> = targets.iter().enumerate().map(|(i, t)| (t.0.as_str(), i)).collect();
    let mut changed: Vec<Unit> = m
        .keys()
        .filter_map(|l| l.strip_prefix("base/").and_then(Unit::parse))
        .filter(|u| {
            [format!("base/{}", u.dash()), format!("global/roads/{}", u.dash())]
                .iter()
                .any(|l| m.get(l) != served.get(l))
        })
        .collect();
    changed.sort_by_key(|&u| spatial_order(u));
    let deg = |b: [i32; 4]| [b[0] as f64 * 1e-7, b[1] as f64 * 1e-7, b[2] as f64 * 1e-7, b[3] as f64 * 1e-7];
    let mut taken = vec![false; targets.len()];
    let mut order: Vec<usize> = Vec::new();
    let mut units = 0;
    for u in changed {
        let ext = unit_reach(reach, m, u);
        let mut mine: Vec<usize> = crate::stage::tiles_in(6, deg(ext))
            .into_iter()
            .filter(|&(x, y)| owns(ext, crate::hipack::tile_bounds(6, x, y)))
            .filter_map(|(x, y)| at.get(format!("6/{x}/{y}").as_str()).copied())
            .filter(|&i| !taken[i])
            .collect();
        mine.sort_unstable();
        if mine.is_empty() {
            continue;
        }
        units += 1;
        for &i in &mine {
            taken[i] = true;
        }
        order.extend(mine);
    }
    order.extend((0..targets.len()).filter(|&i| !taken[i]));
    let mut slots: Vec<Option<(String, String)>> = targets.into_iter().map(Some).collect();
    (order.into_iter().filter_map(|i| slots[i].take()).collect(), units)
}

/// Where a unit comes in a run of units: by the 10° square its tile's centre is in (column, then
/// row from the north), then by tile.
fn spatial_order(u: Unit) -> (i32, i32, u32, u32) {
    let b = crate::hipack::tile_bounds(u.z, u.x, u.y);
    let (lon, lat) = ((b[0] as f64 + b[2] as f64) / 2.0 * 1e-7, (b[1] as f64 + b[3] as f64) / 2.0 * 1e-7);
    ((lon / 10.0).floor() as i32, -(lat / 10.0).floor() as i32, u.x, u.y)
}

/// The rail service's chain (crate::rail), its first stale step (`rail_next`).
fn rail_chain(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
) -> Option<Work> {
    rail_next(cov, date, m, done, inputs).flatten()
}

/// The rail service's chain (crate::rail): Some(its first stale step, or None when it's done), or
/// None while it can't go on: before the timetable feeds' declarations have an accepted version
/// (crate::inputs::feeds), while inputs/keys.env can't be read (`inputs` "keys" "?"), and,
/// rail-feeds done, without the feeds' list, the static files' accepted version
/// (crate::inputs::gtfs: rail waits for it rather than count without them) or the pass's rail set.
/// - rail-feeds reads what decides which feeds there are: the declarations (their accepted index),
///   the catalogue's copy (none: it's fetched), the coverage, the pass's outlines (the countries
///   it's in) and which keys inputs/keys.env holds (`inputs` "keys": their names, never their
///   values). Not what it writes (the feeds' lists, checks and zips), so it doesn't run again for
///   its own sake.
/// - rail reads the feeds' list (each feed's zip by content name, and its day), the static files'
///   accepted index (each zip by content), the pass's rail set and the coverage.
fn rail_next(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
) -> Option<Option<Work>> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let declared = m.get(&crate::inputs::logical(crate::inputs::feeds::UNIT))?;
    let keys = inputs.get("keys").map(String::as_str).unwrap_or("");
    let used = inputs.get("feed-keys").map(String::as_str).unwrap_or("");
    if keys == "?" || used == "?" {
        return None;
    }
    // (Of the keys inputs/keys.env holds, those the declarations fetch with.)
    let used: Vec<&str> = used.split(',').filter(|k| !k.is_empty()).collect();
    let keys = keys.split(',').filter(|k| used.contains(k)).collect::<Vec<_>>().join(",");
    let cover = coverage_all(cov);
    let k = h(&[
        &format!("rail-feeds {RAIL_FEEDS_V}"),
        declared,
        get(crate::rail::CATALOGUE),
        get(&format!("sources/osm/{date}/outlines")),
        &cover,
        &format!("keys {keys}"),
    ]);
    if done.lo.get("rail-feeds").map(String::as_str) != Some(k.as_str()) {
        return Some(Some(Work { step: "rail-feeds".into(), targets: vec![("rail-feeds".into(), k)] }));
    }
    let feeds = m.get(crate::rail::FEEDS)?;
    let set = m.get(&crate::osmpass::set_name(date, "rail"))?;
    let statics = m.get(&crate::inputs::logical(crate::inputs::gtfs::UNIT))?;
    let k = h(&[&format!("rail {RAIL_V}"), feeds, statics, set, &cover]);
    Some(
        (done.lo.get("rail").map(String::as_str) != Some(k.as_str()))
            .then(|| Work { step: "rail".into(), targets: vec![("rail".into(), k)] }),
    )
}

/// Where unit `u`'s ways lie (E7): its owned extent (`Reach::owned_extent`), without a reach its
/// tile + 20 km (`reach::near_box`). A map tile it meets (`owns`) is one of its owners'.
pub fn unit_ways(reach: Option<&Reaches>, u: Unit) -> [i32; 4] {
    reach.and_then(|r| r.get(u)).map(|r| r.owned_extent(u)).unwrap_or_else(|| crate::reach::near_box(u))
}

/// Whether a unit whose ways lie in `ways` owns the tile of box `tile` (both E7): they meet.
pub fn owns(ways: [i32; 4], tile: [i32; 4]) -> bool {
    ways[0] <= tile[2] && ways[2] >= tile[0] && ways[1] <= tile[3] && ways[3] >= tile[1]
}

/// Where unit `u`'s ways lie, its base pack in manifest `m` taken in: `unit_ways`, grown by the
/// pack's own extent when the reaches keep it (crate::extents) and it lies further (a pack built by
/// other reaches). A pack whose extent isn't kept yet is taken to lie inside (the pack and lo jobs
/// keep it: then its tiles are keyed again).
pub fn unit_reach(reach: Option<&Reaches>, m: &BTreeMap<String, String>, u: Unit) -> [i32; 4] {
    let w = unit_ways(reach, u);
    match reach.zip(m.get(&format!("base/{}", u.dash()))).and_then(|(r, c)| r.extents.get(c)) {
        Some(e) => [w[0].min(e[0]), w[1].min(e[1]), w[2].max(e[2]), w[3].max(e[3])],
        None => w,
    }
}

/// Of the units of manifest `m`, those owning any of `tiles` ((z, x, y)): whose ways
/// (`unit_reach`) meet its box (`owns`), the units a tile's key in `pack_lo_targets` reads. What a
/// pack or lo job opens.
pub fn owners_of(reach: Option<&Reaches>, m: &BTreeMap<String, String>, tiles: &[(u8, u32, u32)]) -> BTreeSet<Unit> {
    let boxes: Vec<[i32; 4]> = tiles.iter().map(|&(z, x, y)| crate::hipack::tile_bounds(z, x, y)).collect();
    m.keys()
        .filter_map(|l| l.strip_prefix("base/").and_then(Unit::parse))
        .filter(|&u| boxes.iter().any(|b| owns(unit_reach(reach, m, u), *b)))
        .collect()
}

/// pack(T)'s targets (the z6 tiles the built units' ways reach) and lo's (their z3 tiles), each with
/// its key, done or not. Both are keyed on what they read, the base packs and road values of the
/// units whose ways come into the tile (its owners: pack(T)'s `here` index points into their base
/// packs, way by way, and its catalog lists them). A unit's
/// ways lie in its owned extent (`Reach::owned_extent`: long ways such as ferries reach far), or
/// without a reach within its tile + 20 km. An identical rebuild of a unit (same content names)
/// reruns neither.
fn pack_lo_targets(
    m: &BTreeMap<String, String>,
    reach: Option<&Reaches>,
) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let base_units: Vec<(Unit, String, [i32; 4])> = m
        .iter()
        .filter_map(|(l, c)| {
            l.strip_prefix("base/")
                .and_then(Unit::parse)
                .map(|u| (u, format!("{c}+{}", get(&format!("global/roads/{}", u.dash())))))
        })
        .map(|(u, c)| (u, c, unit_reach(reach, m, u)))
        .collect();
    let deg = |b: [i32; 4]| [b[0] as f64 * 1e-7, b[1] as f64 * 1e-7, b[2] as f64 * 1e-7, b[3] as f64 * 1e-7];
    let mut tiles: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (_, _, ext) in &base_units {
        tiles.extend(crate::stage::tiles_in(6, deg(*ext)));
    }
    // The units whose extent meets a tile's box.
    let owners = |z: u8, x: u32, y: u32| -> Vec<String> {
        let gb = crate::hipack::tile_bounds(z, x, y);
        base_units.iter().filter(|(_, _, e)| owns(*e, gb)).map(|(_, c, _)| c.clone()).collect()
    };
    let key = |head: String, ins: Vec<String>| {
        let mut all = vec![head];
        all.extend(ins);
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        h(&refs)
    };
    let packs: Vec<(String, String)> =
        tiles.iter().map(|&(x, y)| (format!("6/{x}/{y}"), key(format!("pack {PACK_V}"), owners(6, x, y)))).collect();
    let qs: BTreeSet<(u32, u32)> = tiles.iter().map(|&(x, y)| (x >> 3, y >> 3)).collect();
    let lo = qs.into_iter().map(|(x, y)| (format!("3/{x}/{y}"), key(format!("lo {LO_V}"), owners(3, x, y)))).collect();
    (packs, lo)
}

/// What the coverage no longer builds (`units`: the units it does): the outputs of units it doesn't
/// (their base pack, road values and English), and the candidates and peaks of units out of the
/// candidates' set, as prune targets ("unit 6/x/y", "pois 6/x/y"). Catalogs then stop listing them
/// and GC frees them.
fn prune_units(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    units: &[(Unit, Option<String>)],
) -> Option<Work> {
    let built: BTreeSet<String> = units.iter().map(|(u, _)| u.dash()).collect();
    let cands: BTreeSet<String> = pois_keys(cov, date, m).iter().map(|(u, _)| u.dash()).collect();
    let mut t: BTreeSet<String> = BTreeSet::new();
    for l in m.keys() {
        for (prefix, set, kind) in [
            ("base/", &built, "unit"),
            ("global/roads/", &built, "unit"),
            ("global/roaden/", &built, "unit"),
            ("work/pois/", &cands, "pois"),
            ("work/peaks/", &cands, "pois"),
        ] {
            if let Some(u) = l.strip_prefix(prefix).and_then(Unit::parse) {
                if !set.contains(&u.dash()) {
                    t.insert(format!("{kind} {}", u.slash()));
                }
            }
        }
    }
    (!t.is_empty()).then(|| Work { step: "prune".into(), targets: t.into_iter().map(|x| (x, String::new())).collect() })
}

/// Map tiles no unit's ways reach any more: pack(T)'s outputs (hidata, road and rail hi packs) of
/// tiles that aren't pack targets, and lo packs of z3 tiles that aren't lo targets, as prune targets
/// ("pack 6/x/y", "lo 3/x/y").
fn prune_tiles(m: &BTreeMap<String, String>, reach: Option<&Reaches>) -> Option<Work> {
    let (packs, lo) = pack_lo_targets(m, reach);
    let packs: BTreeSet<String> = packs.into_iter().map(|(t, _)| t.replace('/', "-")).collect();
    let lo: BTreeSet<String> = lo.into_iter().map(|(t, _)| t.replace('/', "-")).collect();
    let mut t: BTreeSet<String> = BTreeSet::new();
    for l in m.keys() {
        let hi = l
            .strip_prefix("hidata/")
            .or_else(|| l.strip_prefix("layers/roads/hi/"))
            .or_else(|| l.strip_prefix("layers/rails/hi/"));
        if let Some(k) = hi.filter(|k| !packs.contains(*k)) {
            t.insert(format!("pack {}", k.replace('-', "/")));
        }
        let q = l.strip_prefix("layers/roads/lo/").or_else(|| l.strip_prefix("layers/rails/lo/"));
        if let Some(k) = q.filter(|k| !lo.contains(*k)) {
            t.insert(format!("lo {}", k.replace('-', "/")));
        }
    }
    (!t.is_empty()).then(|| Work { step: "prune".into(), targets: t.into_iter().map(|x| (x, String::new())).collect() })
}

/// The publishing chain, every step drawn (the forecast's and the checklist's: `publish_chain`).
fn roads_chain(date: &str, m: &BTreeMap<String, String>, done: &Keys, reach: Option<&Reaches>) -> Option<Work> {
    publish_chain(date, m, done, reach, [false, false], true)
}

/// Whether so much of the low zooms is stale that they're made however long since they last were
/// (`Publishing::low_later`): half the roads' lo packs or more (a z3 tile's, by its key: what the
/// units near it are now).
pub fn low_due(m: &BTreeMap<String, String>, done: &Keys, reach: Option<&Reaches>) -> bool {
    let (_, lo) = pack_lo_targets(m, reach);
    let stale = lo.iter().filter(|(t, k)| done.lo.get(t) != Some(k)).count();
    !lo.is_empty() && stale * 2 >= lo.len()
}

/// The publishing chain after the units, its first stale step: map tiles no unit reaches pruned,
/// the map tiles whose owners changed (pack(T)), the road → units index, the roads' and rails' lo
/// packs, rail stops and ferries, the terrain and slope roots. `low` false: the low zooms later
/// (`Publishing::low_later`): no lo packs (the roots are held by `roots_held`).
fn publish_chain(
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    reach: Option<&Reaches>,
    roots_held: [bool; 2],
    low: bool,
) -> Option<Work> {
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    // Map tiles no unit's ways reach any more (a region removed) leave the manifest.
    if let Some(w) = prune_tiles(m, reach) {
        return Some(w);
    }
    let (packs, lo) = pack_lo_targets(m, reach);
    let packs: Vec<(String, String)> = packs.into_iter().filter(|(t, k)| stale(&done.pack, t, k)).collect();
    if !packs.is_empty() {
        return Some(Work { step: "pack".into(), targets: packs });
    }
    // The road → units index, once the units' road values are made.
    let roads: Vec<String> = m
        .range("global/roads/".to_string()..)
        .take_while(|(l, _)| l.starts_with("global/roads/"))
        .map(|(l, c)| format!("{l}={c}"))
        .collect();
    if !roads.is_empty() {
        let mut ins = vec![format!("roadunits {ROADUNITS_V}")];
        ins.extend(roads);
        let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
        let k = h(&refs);
        if done.lo.get("roadunits").map(String::as_str) != Some(k.as_str()) {
            return Some(Work { step: "roadunits".into(), targets: vec![("roadunits".into(), k)] });
        }
    }
    let lo: Vec<(String, String)> = lo.into_iter().filter(|(t, k)| low && stale(&done.lo, t, k)).collect();
    if !lo.is_empty() {
        return Some(Work { step: "lo".into(), targets: lo });
    }

    // Rail stops near the built units, and ferries worldwide, from the pass's sets (once there are
    // units: before them there are no roads to ride to).
    let built: Vec<&str> =
        m.range("base/".to_string()..).take_while(|(l, _)| l.starts_with("base/")).map(|(l, _)| l.as_str()).collect();
    if !built.is_empty() {
        for (step, v, set, extra, per_unit) in [
            ("stations", STATIONS_V, "rail", String::new(), true),
            ("ferries", FERRIES_V, "ferries", String::new(), false),
        ] {
            let Some(set_c) = m.get(&crate::osmpass::set_name(date, set)) else { continue };
            let mut ins = vec![format!("{step} {v}"), set_c.clone(), extra];
            // (The ferries count the ferry feeds' zips and the static files: they wait for the
            // list, the zips' content names and days, and for the static files' accepted index,
            // rather than be made without them.)
            if step == "ferries" {
                let (Some(f), Some(s)) =
                    (m.get(crate::rail::FERRY_FEEDS), m.get(&crate::inputs::logical(crate::inputs::gtfs::UNIT)))
                else {
                    continue;
                };
                ins.push(f.clone());
                ins.push(s.clone());
            }
            // The stops are clipped to the built units' tiles; the ferries aren't clipped at all.
            if per_unit {
                ins.extend(built.iter().map(|s| s.to_string()));
            }
            let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
            let k = h(&refs);
            if done.lo.get(step).map(String::as_str) != Some(k.as_str()) {
                return Some(Work { step: step.into(), targets: vec![(step.to_string(), k)] });
            }
        }
    }

    // The terrain and slope roots (z0–2), from their lo packs.
    for ((layer, step), held) in [("terrain", "terrain-root"), ("slope", "slope-root")].into_iter().zip(roots_held) {
        if held {
            continue;
        }
        let Some(k) = root_key(layer, m) else { continue };
        if done.lo.get(step).map(String::as_str) != Some(k.as_str()) {
            return Some(Work { step: step.into(), targets: vec![(step.to_string(), k)] });
        }
    }
    None
}

/// The terrain's or slope's root key (`layer`): its lo packs by content name (a lo pack dropped,
/// the coverage gone from its z3 tile, makes it again); None without one.
pub fn root_key(layer: &str, m: &BTreeMap<String, String>) -> Option<String> {
    let step = format!("{layer}-root");
    let p = format!("layers/{layer}/lo/");
    let mut ins = vec![format!("{step} 1")];
    ins.extend(m.range(p.clone()..).take_while(|(l, _)| l.starts_with(&p)).map(|(_, c)| c.clone()));
    if ins.len() == 1 {
        return None;
    }
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    Some(h(&refs))
}

/// The items' facts and pageviews' key (network; only new items within a pass): the current units'
/// candidates (`current_pois`).
fn items_key(date: &str, pois_now: &[&str]) -> String {
    let mut ins = vec![format!("items {ITEMS_V}"), date.to_string()];
    ins.extend(pois_now.iter().map(|s| s.to_string()));
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    h(&refs)
}

/// The rest of the heritage chain's key (network), on the pass's heritage sites (the registers'
/// fields the chain reads are in their heritage-sources); None before they're made (the marks wait).
fn heritage_key(cov: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Option<String> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    if !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources")) {
        return None;
    }
    let mut ins = vec![format!("heritage {HERITAGE_V}"), date.to_string()];
    for stem in ["heritage", "heritage-areas", "special", "indigenous", "heritage-sources"] {
        ins.push(get(&crate::heritage::base_logical(date, stem)).to_string());
    }
    for l in [
        crate::osmpass::set_name(date, "named"),
        crate::osmpass::set_name(date, "areas"),
        format!("sources/osm/{date}/filtered"),
    ] {
        ins.push(get(&l).to_string());
    }
    ins.push(coverage_all(cov));
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    Some(h(&refs))
}

/// The landmark points' key: every current unit's candidates and peaks (`units`: pois_keys), the
/// items' facts and pageviews, the heritage sites (the files markconv reads: the pass's heritage
/// job's).
fn marks_key(date: &str, m: &BTreeMap<String, String>, units: &[(Unit, String)]) -> String {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let src = crate::markconv::heritage_source(date);
    let mut ins = vec![
        format!("marks {MARKS_V}"),
        get(&format!("sources/items/{date}/facts")).to_string(),
        get(&format!("sources/items/{date}/views")).to_string(),
    ];
    for stem in ["layer-heritage", "details-heritage", "props-heritage"] {
        ins.push(get(&format!("{src}/{stem}")).to_string());
    }
    for (u, _) in units {
        for p in ["work/pois", "work/peaks"] {
            ins.push(get(&format!("{p}/{}", u.dash())).to_string());
        }
    }
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    h(&refs)
}

/// The area overlays' key: the pass's heritage, with the dots the marks gave the World Heritage
/// sites, and the built units (their hi tiles are where the units are); None before the pass's
/// heritage job has made its outputs.
fn overlays_key(date: &str, m: &BTreeMap<String, String>) -> Option<String> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let src = crate::markconv::heritage_source(date);
    if !["layer-heritage", "details-heritage", "props-heritage"].iter().all(|s| m.contains_key(&format!("{src}/{s}"))) {
        return None;
    }
    let mut ins = vec![format!("overlays {OVERLAYS_V}"), get(crate::markconv::HERITAGE_DOTS).to_string()];
    for stem in [
        "layer-heritage-areas",
        "layer-indigenous",
        "layer-special",
        "layer-whs-shapes",
        "details-harea",
        "details-indigenous",
        "details-special",
        "details-park",
        "layer-summary",
        "heritage-sources",
    ] {
        ins.push(get(&format!("{src}/{stem}")).to_string());
    }
    ins.extend(m.range("base/".to_string()..).take_while(|(l, _)| l.starts_with("base/")).map(|(l, _)| l.clone()));
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    Some(h(&refs))
}

/// The z3 terrain areas a unit's peaks read (the terrain's z6 tiles within 30 km: peaks_keys).
fn peaks_terrain(u: Unit) -> BTreeSet<String> {
    tiles_in_wrapped(6, crate::stage::tile_box_grown(u.z, u.x, u.y, 30.0))
        .into_iter()
        .map(|(x, y)| format!("3/{}/{}", x >> 3, y >> 3))
        .collect()
}

/// The landmarks' chain, a step at a time: candidates, peaks, the items' facts and pageviews, the
/// rest of the heritage chain, the landmark points, the area overlays; its first stale step (the
/// forecast's and the checklist's view of what's left: `landmarks_work` is what runs).
fn landmarks_chain(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
) -> Option<Work> {
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    let units: Vec<(Unit, String)> = pois_keys(cov, date, m);
    // The candidates (once the pass's hiking-route ends exist).
    if !m.contains_key(&format!("work/trailends/{date}")) {
        return None;
    }
    let stale_pois: Vec<(String, String)> =
        units.iter().filter(|(u, k)| stale(&done.pois, &u.slash(), k)).map(|(u, k)| (u.slash(), k.clone())).collect();
    if !stale_pois.is_empty() {
        return Some(Work { step: "pois".into(), targets: stale_pois });
    }
    // Their peaks (once the pass's summits exist).
    if !m.contains_key(&format!("work/summits/{date}")) {
        return None;
    }
    let stale_peaks: Vec<(String, String)> = peaks_keys(cov, date, m)
        .into_iter()
        .filter(|(u, k)| stale(&done.peaks, &u.slash(), k))
        .map(|(u, k)| (u.slash(), k))
        .collect();
    if !stale_peaks.is_empty() {
        return Some(Work { step: "peaks".into(), targets: stale_peaks });
    }
    let pois_now = current_pois(cov, date, m);
    if pois_now.is_empty() {
        return None;
    }
    let k = items_key(date, &pois_now);
    if done.lo.get("items").map(String::as_str) != Some(k.as_str()) {
        return Some(Work { step: "items".into(), targets: vec![("items".into(), k)] });
    }
    // (The heritage chain once the pass's heritage sites are made; the marks wait for it. Nothing
    // of it while the sites have no key (no registers accepted yet): an older pass's
    // heritage-sources isn't what the chain reads now.)
    heritage_sites_key(cov, date, m, inputs)?;
    let k = heritage_key(cov, date, m)?;
    if done.lo.get("heritage") != Some(&k) {
        return Some(Work { step: "heritage".into(), targets: vec![("heritage".into(), k)] });
    }
    let k = marks_key(date, m, &units);
    if done.lo.get("marks").map(String::as_str) != Some(k.as_str()) {
        return Some(Work { step: "marks".into(), targets: vec![("marks".into(), k)] });
    }
    overlays_key(date, m)
        .filter(|k| done.lo.get("overlays") != Some(k))
        .map(|k| Work { step: "overlays".into(), targets: vec![("overlays".into(), k)] })
}

/// The landmarks' work that can run now (docs/plan.md §8, Order), each step once what it reads is
/// built as it will stay (a worldwide job of the pass that's stale, about to make it again, counts
/// as not built: what read it would be built twice), none waiting for the units but the overlays
/// (they read the built units: after the last, `last`): the candidates (once the pass's
/// hiking-route ends are made); their peaks once every candidate is, the pass's summits are made
/// and the terrain they read is built (`terrain_left`), each unit's as its own is; the items' facts
/// and pageviews once every candidate is; the rest of the heritage chain once the heritage sites
/// are; the landmark points once those four are (on the pass's heritage, never today's meanwhile);
/// then the overlays. They don't wait for each other otherwise: the heritage chain (an hour and
/// more of network) runs beside the candidates.
fn landmarks_work(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
    terrain_left: &BTreeSet<String>,
    last: bool,
) -> Vec<Work> {
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    let mut out = Vec::new();
    let units: Vec<(Unit, String)> = pois_keys(cov, date, m);
    let ends = m.contains_key(&format!("work/trailends/{date}")) && trailends_work(date, m, done).is_none();
    let stale_pois: Vec<(String, String)> = if ends {
        units.iter().filter(|(u, k)| stale(&done.pois, &u.slash(), k)).map(|(u, k)| (u.slash(), k.clone())).collect()
    } else {
        Vec::new()
    };
    let pois_built = ends && stale_pois.is_empty();
    if !stale_pois.is_empty() {
        out.push(Work { step: "pois".into(), targets: stale_pois });
    }
    let mut peaks_built = false;
    if pois_built && m.contains_key(&format!("work/summits/{date}")) && summits_work(date, m, done).is_none() {
        let stale_peaks: Vec<(Unit, String)> =
            peaks_keys(cov, date, m).into_iter().filter(|(u, k)| stale(&done.peaks, &u.slash(), k)).collect();
        peaks_built = stale_peaks.is_empty();
        let ready: Vec<(String, String)> = stale_peaks
            .into_iter()
            .filter(|(u, _)| peaks_terrain(*u).is_disjoint(terrain_left))
            .map(|(u, k)| (u.slash(), k))
            .collect();
        if !ready.is_empty() {
            out.push(Work { step: "peaks".into(), targets: ready });
        }
    }
    let pois_now = if pois_built { current_pois(cov, date, m) } else { Vec::new() };
    let mut items_built = false;
    if !pois_now.is_empty() {
        let k = items_key(date, &pois_now);
        items_built = done.lo.get("items").map(String::as_str) == Some(k.as_str());
        if !items_built {
            out.push(Work { step: "items".into(), targets: vec![("items".into(), k)] });
        }
    }
    // (Not before the heritage sites are made as the coverage wants them; with no key for them (no
    // registers accepted yet) nothing heritage-related runs, an older heritage-sources there or
    // not, and the marks and overlays wait.)
    let sites_due =
        heritage_sites_key(cov, date, m, inputs).is_none() || heritage_sites_work(cov, date, m, done, inputs).is_some();
    let heritage_built = match heritage_key(cov, date, m) {
        _ if sites_due => false,
        None => false,
        Some(k) if done.lo.get("heritage") != Some(&k) => {
            out.push(Work { step: "heritage".into(), targets: vec![("heritage".into(), k)] });
            false
        }
        Some(_) => true,
    };
    if peaks_built && items_built && heritage_built {
        let k = marks_key(date, m, &units);
        if done.lo.get("marks").map(String::as_str) != Some(k.as_str()) {
            out.push(Work { step: "marks".into(), targets: vec![("marks".into(), k)] });
        } else if let Some(k) = overlays_key(date, m).filter(|k| last && done.lo.get("overlays") != Some(k)) {
            out.push(Work { step: "overlays".into(), targets: vec![("overlays".into(), k)] });
        }
    }
    out
}

/// The 3D buildings' sources fetched onto the NAS (dem/bldfetch.py, `scenic-build bld-fetch`): the
/// pinned release's files and GHSL's tiles meeting the coverage grown by 20 km.
pub const BLD_FETCH_V: u32 = 1;

/// bld-fetch's key (network): the declared release (`inputs` "bld-key": the release while the
/// declarations name what the code had, crate::bld::sources::declared) and the whole coverage. Not the footers it reads and
/// writes (`footers.json.gz`): a release's files never change, so the release names them, and a key
/// on what the job writes would run it a second time for its own sake.
fn bld_fetch_key(cov: &Coverage, inputs: &BTreeMap<String, String>) -> String {
    h(&[&format!("bld-fetch {BLD_FETCH_V}"), bld_key(inputs), &coverage_all(cov)])
}

/// What the 3D buildings' keys name of the declared sources (`inputs` "bld-key").
fn bld_key(inputs: &BTreeMap<String, String>) -> &str {
    inputs.get("bld-key").map_or("-", String::as_str)
}

/// Whether the release's sources are here (`inputs` "bld-release", crate::bld::sources::digests:
/// the declared release when its indexes read, "" before anything was downloaded); None when they can't be
/// read now ("?"), or weren't asked about (no entry, or no "bld-key": the sources not declared): no
/// 3D buildings' work then.
fn bld_sources(inputs: &BTreeMap<String, String>) -> Option<bool> {
    match inputs.get("bld-release").map(String::as_str) {
        None | Some("?") => None,
        Some(_) if !inputs.contains_key("bld-key") => None,
        Some(r) => Some(!r.is_empty()),
    }
}

/// The 3D buildings' targets (docs/buildings3d.md §3.1–3.2), each with its key, done or not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BldTargets {
    /// `bldprep T`: the z6 tiles within 1 km of the coverage (so a tile's neighbours within the
    /// fill's 620 m are prepared too) with a downloaded row group or GHSL tile meeting them
    /// (`inputs` "bldprep 6/x/y": what it reads, crate::bld::sources). Its key: the version, the
    /// release and that.
    pub prep: Vec<(String, String)>,
    /// `bldtiles T`: the z6 tiles meeting the coverage (its buffers included). Its key: the version,
    /// the content names of T's and its 8 neighbours' normalized files ("-" none), the coverage's
    /// shapes over T grown by 1 km in the recipes' order with their countries
    /// (`Coverage::shapes_key`: which shape a building is in sets its country's fits), and the
    /// content names of the newest pass's heritage slices of T, its sites and its World Heritage
    /// components (for the field's heritage lists; "-" none).
    pub tiles: Vec<(String, String)>,
    /// `bldfield-lo Q`: the z3 tiles holding a `bldtiles` target, each with its key (the version,
    /// Q, and its z6 tiles' mids' content names, "-" none) and its `bldtiles` targets.
    pub lo: Vec<(String, String, Vec<String>)>,
}

/// The 3D buildings' targets for the coverage `cov`, the manifest `m` and the sources (`inputs`).
pub fn bld_targets(cov: &Coverage, m: &BTreeMap<String, String>, inputs: &BTreeMap<String, String>) -> BldTargets {
    use crate::bld::{field, work_logical, BLDPREP_V, BUILDINGS_V};
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let sources = bld_sources(inputs) == Some(true);
    let mut out = BldTargets::default();
    let her = crate::heritage::latest_date(m);
    for (x, y, shapes) in bld_tiles(cov).iter() {
        let t = format!("6/{x}/{y}");
        if let Some(d) = inputs.get(&format!("bldprep {t}")).filter(|_| sources) {
            out.prep.push((t.clone(), h(&[&format!("bldprep {BLDPREP_V}"), bld_key(inputs), d])));
        }
        if let Some(shapes) = shapes {
            let mut ins = vec![format!("bldtiles {BUILDINGS_V}")];
            for (nx, ny) in bld_around(*x, *y) {
                ins.push(match (nx, ny) {
                    (Some(nx), Some(ny)) => get(&work_logical(nx, ny)).to_string(),
                    _ => "-".to_string(),
                });
            }
            ins.push(shapes.clone());
            ins.push(her.as_deref().map_or("-", |d| get(&crate::heritage::pos_logical(d, *x, *y))).to_string());
            ins.push(her.as_deref().map_or("-", |d| get(&crate::heritage::parts_logical(d, *x, *y))).to_string());
            let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
            out.tiles.push((t, h(&refs)));
        }
    }
    let mut by_q: BTreeMap<(u32, u32), Vec<String>> = BTreeMap::new();
    for (t, _) in &out.tiles {
        if let Some(u) = Unit::parse(t) {
            by_q.entry((u.x >> 3, u.y >> 3)).or_default().push(t.clone());
        }
    }
    for ((qx, qy), ts) in by_q {
        let mut ins = vec![format!("bldfield-lo {}", field::LO_V), format!("3/{qx}/{qy}")];
        for y in qy * 8..qy * 8 + 8 {
            for x in qx * 8..qx * 8 + 8 {
                ins.push(get(&field::mid_logical(x, y)).to_string());
            }
        }
        let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
        out.lo.push((format!("3/{qx}/{qy}"), h(&refs), ts));
    }
    out
}

/// The z6 tiles within 1 km of the coverage, each with, when it meets the coverage itself, the
/// coverage's shapes over it grown by 1 km (`Coverage::shapes_key`): what `bld_targets` reads of
/// the coverage. Kept for the coverage last asked about (the plan, the checklist and the forecast
/// each ask, several times a loop; the shapes' fingerprints over 380 tiles are seconds of work).
type BldTiles = std::sync::Arc<Vec<(u32, u32, Option<String>)>>;
fn bld_tiles(cov: &Coverage) -> BldTiles {
    static KEPT: std::sync::Mutex<Option<(String, BldTiles)>> = std::sync::Mutex::new(None);
    // (The coverage by what shapes_key and meets_rect read: each shape's outline, buffer and
    // country, in order.)
    let mut id = Vec::new();
    for s in &cov.shapes {
        id.extend_from_slice(s.buffer_m.to_le_bytes().as_slice());
        id.extend_from_slice(s.country.as_bytes());
        id.push(0);
        for r in &s.rings {
            id.extend_from_slice(bytemuck::cast_slice(r));
            id.push(1);
        }
        id.push(2);
    }
    let id = store::naming::hash16(&id);
    let mut g = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, v)) = g.as_ref() {
        if *k == id {
            return v.clone();
        }
    }
    let mut v = Vec::new();
    for x in 0..64u32 {
        for y in 0..64u32 {
            let b = crate::hipack::tile_bounds(6, x, y);
            let near = crate::hipack::grow(b, 1.0);
            if cov.meets_rect(near) {
                v.push((x, y, cov.meets_rect(b).then(|| cov.shapes_key(near))));
            }
        }
    }
    let v = std::sync::Arc::new(v);
    *g = Some((id, v.clone()));
    v
}

/// Z6 tile (x, y) and its 8 neighbours, rows from the north (None past the world's edge: not
/// wrapped across the antimeridian, as bldtiles reads them: crate::bld::job::work_files).
fn bld_around(x: u32, y: u32) -> Vec<(Option<u32>, Option<u32>)> {
    let c = |v: u32, d: i64| u32::try_from(v as i64 + d).ok().filter(|&v| v < 64);
    (-1i64..=1)
        .flat_map(|dy| (-1i64..=1).map(move |dx| (dx, dy)))
        .map(|(dx, dy)| (c(x, dx), c(y, dy)))
        .map(|(a, b)| if a.is_some() && b.is_some() { (a, b) } else { (None, None) })
        .collect()
}

/// The normalized files and tiles no target has any more (the coverage shrank): prune targets
/// ("bldprep 6/x/y", "bldtiles 6/x/y": its packs and the field's mid, "bldfield-lo 3/x/y"). The
/// files only while the sources read (an index missing would make every tile seem to read nothing).
fn prune_bld(m: &BTreeMap<String, String>, tt: &BldTargets, sources: bool) -> Option<Work> {
    let prep: BTreeSet<String> = tt.prep.iter().map(|t| t.0.replace('/', "-")).collect();
    let tiles: BTreeSet<String> = tt.tiles.iter().map(|t| t.0.replace('/', "-")).collect();
    let lo: BTreeSet<String> = tt.lo.iter().map(|t| t.0.replace('/', "-")).collect();
    let mut t: BTreeSet<String> = BTreeSet::new();
    let pack = format!("layers/{}/hi/", crate::bld::LAYER);
    let fpack = format!("layers/{}/hi/", crate::bld::field::LAYER);
    let flo = format!("layers/{}/lo/", crate::bld::field::LAYER);
    for (prefix, keep, kind) in [
        ("work/bld/", &prep, "bldprep"),
        (pack.as_str(), &tiles, "bldtiles"),
        (fpack.as_str(), &tiles, "bldtiles"),
        ("work/bldfield-mid/", &tiles, "bldtiles"),
        (flo.as_str(), &lo, "bldfield-lo"),
    ] {
        if kind == "bldprep" && !sources {
            continue;
        }
        for (l, _) in m.range(prefix.to_string()..).take_while(|(l, _)| l.starts_with(prefix)) {
            let d = &l[prefix.len()..];
            if Unit::parse(d).is_some() && !keep.contains(d) {
                t.insert(format!("{kind} {}", d.replace('-', "/")));
            }
        }
    }
    (!t.is_empty()).then(|| Work { step: "prune".into(), targets: t.into_iter().map(|x| (x, String::new())).collect() })
}

/// The 3D buildings' chain (docs/buildings3d.md §3.3), the work that can run now: the sources'
/// fetch when its key changed (beside the rest: what's here is prepared meanwhile, a file fetched
/// later changes the keys of the tiles it meets), what no target has any more pruned, the stale
/// `bldprep` targets, then the stale `bldtiles` targets whose tile and neighbours are prepared as
/// they will stay (none before the sources are here); each step's tiles by `rank` (the regions' order). None of it while the sources'
/// indexes can't be read now.
pub fn bld_work(
    cov: &Coverage,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
    rank: &dyn Fn(&str) -> BldRank,
) -> Vec<Work> {
    let Some(sources) = bld_sources(inputs) else { return Vec::new() };
    let mut out = Vec::new();
    let k = bld_fetch_key(cov, inputs);
    if done.lo.get("bld-fetch") != Some(&k) {
        out.push(Work { step: "bld-fetch".into(), targets: vec![("bld-fetch".into(), k)] });
    }
    let tt = bld_targets(cov, m, inputs);
    out.extend(prune_bld(m, &tt, sources));
    let mut prep: Vec<(String, String)> =
        tt.prep.iter().filter(|(t, k)| done.bldprep.get(t) != Some(k)).cloned().collect();
    let unprepared: BTreeSet<&str> = prep.iter().map(|t| t.0.as_str()).collect();
    let ready = |t: &str| {
        Unit::parse(t).is_some_and(|u| {
            bld_around(u.x, u.y).into_iter().all(|n| match n {
                (Some(x), Some(y)) => !unprepared.contains(format!("6/{x}/{y}").as_str()),
                _ => true,
            })
        })
    };
    // (None before anything's downloaded: every tile would come out empty.)
    let mut tiles: Vec<(String, String)> =
        tt.tiles.iter().filter(|(t, k)| sources && done.bldtiles.get(t) != Some(k) && ready(t)).cloned().collect();
    prep.sort_by_cached_key(|t| rank(&t.0));
    tiles.sort_by_cached_key(|t| rank(&t.0));
    // The field's lo levels once every bldtiles target of their z3 tile is done as it will stay.
    let tiles_stale: BTreeSet<&str> =
        tt.tiles.iter().filter(|(t, k)| done.bldtiles.get(t) != Some(k)).map(|t| t.0.as_str()).collect();
    let lo: Vec<(String, String)> = tt
        .lo
        .iter()
        .filter(|(q, k, ts)| {
            sources && done.bldfield_lo.get(q) != Some(k) && ts.iter().all(|t| !tiles_stale.contains(t.as_str()))
        })
        .map(|(q, k, _)| (q.clone(), k.clone()))
        .collect();
    for (step, targets) in [("bldprep", prep), ("bldtiles", tiles), ("bldfield-lo", lo)] {
        if !targets.is_empty() {
            out.push(Work { step: step.into(), targets });
        }
    }
    out
}

/// The 3D buildings' chain's first stale step, as if each before it succeeded (`remaining`: the
/// checklist's and the forecast's view): the fetch, every stale `bldprep`, every stale `bldtiles`.
fn bld_next(
    cov: &Coverage,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
) -> Option<Work> {
    bld_sources(inputs)?;
    let k = bld_fetch_key(cov, inputs);
    if done.lo.get("bld-fetch") != Some(&k) {
        return Some(Work { step: "bld-fetch".into(), targets: vec![("bld-fetch".into(), k)] });
    }
    let tt = bld_targets(cov, m, inputs);
    let prep: Vec<(String, String)> = tt.prep.into_iter().filter(|(t, k)| done.bldprep.get(t) != Some(k)).collect();
    if !prep.is_empty() {
        return Some(Work { step: "bldprep".into(), targets: prep });
    }
    let tiles: Vec<(String, String)> = tt.tiles.into_iter().filter(|(t, k)| done.bldtiles.get(t) != Some(k)).collect();
    if !tiles.is_empty() {
        return Some(Work { step: "bldtiles".into(), targets: tiles });
    }
    let lo: Vec<(String, String)> =
        tt.lo.into_iter().filter(|(q, k, _)| done.bldfield_lo.get(q) != Some(k)).map(|(q, k, _)| (q, k)).collect();
    (!lo.is_empty()).then(|| Work { step: "bldfield-lo".into(), targets: lo })
}

/// A z6 tile's place in the 3D buildings' order (`bld_rank`).
pub type BldRank = (usize, (i32, i32, u32, u32));

/// A z6 tile's place in the 3D buildings' order: the first of `regions` (in the order they're
/// built) whose coverage meets it, then `spatial_order`.
fn bld_rank<'a>(regions: &'a [&'a Coverage]) -> impl Fn(&str) -> BldRank + 'a {
    move |t: &str| {
        let Some(u) = Unit::parse(t) else { return (usize::MAX, (0, 0, 0, 0)) };
        let b = crate::hipack::tile_bounds(6, u.x, u.y);
        (regions.iter().position(|c| c.meets_rect(b)).unwrap_or(usize::MAX), spatial_order(u))
    }
}

/// The depth chain's targets (docs/plan.md §6, Depth): every z3 tile within `REACH_KM` of the
/// coverage that a depth source meets (`depth-lo`: zooms 3–8), and every z6 tile within it that a
/// source with a grid finer than z8 meets (`depth`: zooms 9–16), each with its key; the targets
/// whose water digests aren't worked out yet (`unknown`) wait.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DepthTargets {
    pub lo: Vec<(String, String)>,
    pub hi: Vec<(String, String)>,
    pub unknown: usize,
}

/// The depth's version line in its keys.
fn depth_versions(step: &str) -> String {
    format!("{step} {} {}", crate::depth::build::DEPTH_V, crate::depth::build::DEPTH_LO_V)
}

/// What a depth piece's water digest is made from (`WaterIdx`'s from): its z6–12 tiles.
pub const DEPTH_WATER_FROM: &str = "depth z6-12";

/// The z3 tiles (assemblies) and z6 tiles (pieces) the depth builds for the coverage and its
/// declared sources (`DepthTargets`).
pub fn depth_tiles(cov: &Coverage, sources: &[crate::depth::grids::Source]) -> (Vec<(u32, u32)>, Vec<(u32, u32)>) {
    let deg =
        |b: [i32; 4]| [f64::from(b[0]) * 1e-7, f64::from(b[1]) * 1e-7, f64::from(b[2]) * 1e-7, f64::from(b[3]) * 1e-7];
    let (mut lo, mut hi) = (Vec::new(), Vec::new());
    if sources.is_empty() {
        return (lo, hi);
    }
    for qx in 0..8u32 {
        for qy in 0..8u32 {
            let b = crate::hipack::tile_bounds(3, qx, qy);
            if !cov.meets_rect(crate::hipack::grow(b, crate::depth::build::REACH_KM))
                || !sources.iter().any(|s| s.max_zoom_in(deg(b)).is_some())
            {
                continue;
            }
            lo.push((qx, qy));
            for x in qx * 8..qx * 8 + 8 {
                for y in qy * 8..qy * 8 + 8 {
                    let b6 = crate::hipack::tile_bounds(6, x, y);
                    if cov.meets_rect(crate::hipack::grow(b6, crate::depth::build::REACH_KM))
                        && sources
                            .iter()
                            .any(|s| s.max_zoom_in(deg(b6)).is_some_and(|z| z > crate::depth::compose::ALL_UPTO))
                    {
                        hi.push((x, y));
                    }
                }
            }
        }
    }
    (lo, hi)
}

/// The water the depth's targets read, as the terrain-water job works their digests out: an
/// assembly's z6–8 tiles (the terrain's assembly's, `AREA_FROM`), a piece's z6–12 ("depth 6/x/y").
pub fn depth_water_reads(cov: &Coverage, sources: &[crate::depth::grids::Source]) -> Vec<crate::terrain_water::Reads> {
    let (lo, hi) = depth_tiles(cov, sources);
    let mut out: Vec<crate::terrain_water::Reads> = lo
        .into_iter()
        .map(|q| {
            (
                format!("3/{}/{}", q.0, q.1),
                crate::terrain_water::AREA_FROM.to_string(),
                crate::terrain_water::area_tiles(q),
            )
        })
        .collect();
    for (x, y) in hi {
        let tiles: Vec<(u8, u32, u32)> = (6..=12u8)
            .flat_map(|z| {
                let s = 1u32 << (z - 6);
                (x * s..(x + 1) * s).flat_map(move |tx| (y * s..(y + 1) * s).map(move |ty| (z, tx, ty)))
            })
            .collect();
        out.push((format!("depth 6/{x}/{y}"), DEPTH_WATER_FROM.to_string(), tiles));
    }
    out
}

/// The depth's targets with their keys; None while its sources can't be read or there's no water
/// source (no basemap yet).
///
/// - an assembly's key: the versions, the z3 tile, the sources meeting it (their ids and digests),
///   the water it reads (`water_key` with `AREA_FROM`);
/// - a piece's: the versions, the z6 tile, the sources meeting it, the lo packs of its z3 tile and
///   its eight neighbours (what its capped pixels are scaled up from), its water ("depth 6/x/y").
pub fn depth_targets(cov: &Coverage, m: &BTreeMap<String, String>, tiles: &TerrainTiles) -> Option<DepthTargets> {
    let sources = tiles.depth(m)?;
    let pin = crate::terrain_pack::water_source_pin(m)?;
    let idx = tiles.water_idx(m, &pin);
    let deg =
        |b: [i32; 4]| [f64::from(b[0]) * 1e-7, f64::from(b[1]) * 1e-7, f64::from(b[2]) * 1e-7, f64::from(b[3]) * 1e-7];
    let pins = |b: [f64; 4]| -> String {
        sources.iter().filter(|s| s.meeting(b).next().is_some()).map(|s| s.pin.as_str()).collect::<Vec<_>>().join(",")
    };
    let (lo, hi) = depth_tiles(cov, &sources);
    let mut out = DepthTargets::default();
    for q in lo {
        let t = format!("3/{}/{}", q.0, q.1);
        let Some(w) = water_key(Some(&pin), idx, &t, crate::terrain_water::AREA_FROM) else {
            out.unknown += 1;
            continue;
        };
        out.lo.push((
            t.clone(),
            h(&[&depth_versions("depth-lo"), &t, &pins(deg(crate::hipack::tile_bounds(3, q.0, q.1))), &w]),
        ));
    }
    for (x, y) in hi {
        let t = format!("6/{x}/{y}");
        let Some(w) = water_key(Some(&pin), idx, &format!("depth {t}"), DEPTH_WATER_FROM) else {
            out.unknown += 1;
            continue;
        };
        let (qx, qy) = (i64::from(x >> 3), i64::from(y >> 3));
        let mut los = Vec::new();
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                let (ax, ay) = ((qx + dx).rem_euclid(8), qy + dy);
                if (0..8).contains(&ay) {
                    los.push(
                        m.get(&crate::depth::build::lo_logical(crate::depth::LAYER, ax as u32, ay as u32))
                            .map_or("-", String::as_str)
                            .to_string(),
                    );
                }
            }
        }
        // (A margin: the block's pad reads a little past the tile.)
        let b = crate::hipack::grow(crate::hipack::tile_bounds(6, x, y), 2.0);
        out.hi.push((t.clone(), h(&[&depth_versions("depth"), &t, &pins(deg(b)), &los.join(","), &w])));
    }
    Some(out)
}

/// The depth chain's work that can run now: the stale assemblies, then the stale pieces whose z3
/// tile and its neighbours' assemblies are done as they'll stay (their lo packs are in the pieces'
/// keys).
pub fn depth_work(cov: &Coverage, m: &BTreeMap<String, String>, done: &Keys, tiles: &TerrainTiles) -> Vec<Work> {
    let Some(dt) = depth_targets(cov, m, tiles) else { return Vec::new() };
    let lo: Vec<(String, String)> = dt.lo.iter().filter(|(t, k)| done.depth_lo.get(t) != Some(k)).cloned().collect();
    let stale_q: BTreeSet<(u32, u32)> = lo.iter().filter_map(|(t, _)| Unit::parse(t)).map(|u| (u.x, u.y)).collect();
    let ready = |t: &str| {
        Unit::parse(t).is_some_and(|u| {
            let (qx, qy) = (i64::from(u.x >> 3), i64::from(u.y >> 3));
            (-1..=1i64).all(|dy| {
                (-1..=1i64).all(|dx| !stale_q.contains(&((qx + dx).rem_euclid(8) as u32, (qy + dy).clamp(0, 7) as u32)))
            })
        })
    };
    let hi: Vec<(String, String)> =
        dt.hi.iter().filter(|(t, k)| done.depth.get(t) != Some(k) && ready(t)).cloned().collect();
    let mut out = Vec::new();
    for (step, targets) in [("depth-lo", lo), ("depth", hi)] {
        if !targets.is_empty() {
            out.push(Work { step: step.into(), targets });
        }
    }
    out
}

/// The depth chain's first stale step, as if each before it succeeded (`remaining`).
fn depth_next(cov: &Coverage, m: &BTreeMap<String, String>, done: &Keys, tiles: &TerrainTiles) -> Option<Work> {
    let dt = depth_targets(cov, m, tiles)?;
    let lo: Vec<(String, String)> = dt.lo.into_iter().filter(|(t, k)| done.depth_lo.get(t) != Some(k)).collect();
    if !lo.is_empty() {
        return Some(Work { step: "depth-lo".into(), targets: lo });
    }
    let hi: Vec<(String, String)> = dt.hi.into_iter().filter(|(t, k)| done.depth.get(t) != Some(k)).collect();
    (!hi.is_empty()).then(|| Work { step: "depth".into(), targets: hi })
}

/// One step's line of the build's checklist: its targets done of all (`total` None until an earlier
/// step makes them known), or how many are left (a worldwide step's: 1 or 0; a chain's: the targets
/// of its works to come). Neither: to come, not sized yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub step: String,
    /// Its step's title and the noun its targets are counted in (crate::agent::steps), filled as
    /// the lines are folded: readers without the table (the menu bar) say it so.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub noun: String,
    #[serde(default)]
    pub done: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<usize>,
}

impl Item {
    fn sized(step: &str, done: usize, total: Option<usize>) -> Item {
        Item { step: step.into(), done, total, left: total.map(|t| t.saturating_sub(done)), ..Default::default() }
    }

    fn left(step: &str, left: Option<usize>) -> Item {
        Item { step: step.into(), left, ..Default::default() }
    }

    /// Whether it's sized: its targets known.
    pub fn known(&self) -> bool {
        self.left.is_some()
    }

    pub fn finished(&self) -> bool {
        self.left == Some(0)
    }
}

/// One stage's line of the build's checklist (the status, the menu bar, the build page): its steps'
/// items, summed. `done` and `total` count the targets of the items sharing the line's noun
/// (`unit`: the first sized item's; the other nouns' items are said apart, `counts`), `left` every
/// item's targets left; `total` and `left` None while an item of it isn't sized yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Line {
    /// The stage's id (crate::agent::steps::STAGES).
    #[serde(default)]
    pub id: String,
    /// The stage's title.
    pub what: String,
    /// The stage's steps (a job's id starts with one: "unit 6/31/20").
    pub steps: Vec<String>,
    #[serde(default)]
    pub done: usize,
    #[serde(default)]
    pub total: Option<usize>,
    #[serde(default)]
    pub left: Option<usize>,
    /// The noun its targets are counted in, plural ("tiles").
    #[serde(default)]
    pub unit: String,
    /// Its steps' lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    /// The jobs still to do, by name, in the order they'll run (a step's runs together, its targets
    /// counted: "Peaks: 178 tiles"); the first is the one under way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// While it has work left and isn't this Mac's job now: why (another Mac is on it, it waits
    /// for the home network or out a failure, for the steps above). The pass's: the countries it
    /// kept as the previous pass's outlines, done or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Which of its steps a helper may do (crate::agent::steps::SHARED, `mark_shared`): "all", or
    /// the parts by their targets' noun ("candidates and peaks"); none, the lead's alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<String>,
}

impl Line {
    pub fn finished(&self) -> bool {
        self.left == Some(0) || self.total.is_some_and(|t| self.done >= t && self.left.is_none())
    }

    /// Its counts in words, a noun at a time in its items' order: "398 of 398 tiles", "18 of 18
    /// areas", "2 runs left"; "done" when nothing is left, "to come" while it isn't sized. (The
    /// line's own `done`/`total` are the first noun's: what readers of the line alone show.)
    pub fn counts(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut nouns: Vec<&str> = Vec::new();
        for i in &self.items {
            let noun = super::steps::noun(&i.step);
            if nouns.contains(&noun) {
                continue;
            }
            nouns.push(noun);
            let of: Vec<&Item> = self.items.iter().filter(|j| super::steps::noun(&j.step) == noun).collect();
            if of.iter().any(|j| j.total.is_some()) {
                if of.iter().all(|j| j.total.is_some()) {
                    let (d, t): (usize, usize) =
                        (of.iter().map(|j| j.done).sum(), of.iter().map(|j| j.total.unwrap_or(0)).sum());
                    out.push(format!("{d} of {t} {noun}s"));
                } else {
                    out.push(format!("{noun}s to come"));
                }
            } else if of.iter().all(|j| j.left.is_some()) {
                let l: usize = of.iter().map(|j| j.left.unwrap_or(0)).sum();
                if l > 0 {
                    out.push(format!("{l} {noun}{} left", if l == 1 { "" } else { "s" }));
                }
            } else {
                out.push(format!("{noun}s to come"));
            }
        }
        if out.is_empty() {
            out.push(if self.finished() { "done".into() } else { "to come".into() });
        }
        out
    }
}

/// The checklist's lines from its items: a line per stage with an item, in the stages' order
/// (crate::agent::steps::STAGES), its steps every one of the stage's, its items in the table's
/// order, and its jobs to come (`next`, by stage) and the notes (`note`, by stage).
pub fn fold(items: &[Item], next: &BTreeMap<&str, Vec<String>>, notes: &BTreeMap<&str, String>) -> Vec<Line> {
    use super::steps::{self, Scope, STAGES, TABLE};
    let mut out = Vec::new();
    for st in STAGES {
        let steps: Vec<&str> = TABLE.iter().filter(|s| s.stage == st.id).map(|s| s.name).collect();
        let mut its: Vec<Item> = steps.iter().filter_map(|s| items.iter().find(|i| i.step == *s)).cloned().collect();
        // (An item of a step the table doesn't place: left out.)
        its.extend(
            items
                .iter()
                .filter(|i| steps::row(&i.step).is_none() && steps::stage_of(&i.step).is_some_and(|g| g.id == st.id))
                .cloned(),
        );
        if its.is_empty() {
            continue;
        }
        for i in its.iter_mut() {
            i.title = steps::title(&i.step).into_owned();
            i.noun = steps::noun(&i.step).into();
        }
        let known = its.iter().all(Item::known);
        // (The line's noun: the first sized item's, else the first item's; its count the items of
        // that noun, so tiles and areas are never summed as one.)
        let noun =
            its.iter().find(|i| i.total.is_some()).or(its.first()).map(|i| steps::noun(&i.step)).unwrap_or("run");
        let sized: Vec<&Item> = its.iter().filter(|i| i.total.is_some() && steps::noun(&i.step) == noun).collect();
        out.push(Line {
            id: st.id.into(),
            what: st.title.into(),
            steps: steps.iter().map(|s| s.to_string()).collect(),
            done: sized.iter().map(|i| i.done).sum(),
            total: (known && !sized.is_empty()).then(|| sized.iter().map(|i| i.total.unwrap_or(0)).sum()),
            left: known.then(|| its.iter().map(|i| i.left.unwrap_or(0)).sum()),
            unit: if its.iter().all(|i| steps::row(&i.step).is_some_and(|r| r.scope == Scope::World)) {
                "runs".into()
            } else {
                format!("{noun}s")
            },
            items: its,
            next: next.get(st.id).cloned().unwrap_or_default(),
            note: notes.get(st.id).cloned(),
            shared: None,
        });
    }
    mark_shared(&mut out);
    out
}

/// The jobs `works` would run, by name, a step's runs together, its targets counted.
fn next_of(works: &[Work]) -> Vec<String> {
    let mut runs: Vec<(&str, usize)> = Vec::new();
    for w in works {
        match runs.last_mut() {
            Some((s, n)) if *s == w.step => *n += w.targets.len(),
            _ => runs.push((&w.step, w.targets.len())),
        }
    }
    runs.into_iter().map(|(s, n)| super::steps::said(s, n, None)).collect()
}

/// Marks each line whose steps a helper may do: all of them, or which (by their targets' noun, a
/// step's title where the noun says nothing).
pub fn mark_shared(lines: &mut [Line]) {
    for l in lines {
        let shared: Vec<&str> =
            l.steps.iter().map(String::as_str).filter(|s| super::steps::SHARED.contains(s)).collect();
        l.shared = match shared.len() {
            0 => None,
            n if n == l.steps.len() => Some("all".into()),
            _ => Some(shared.iter().map(|s| super::steps::title(s)).collect::<Vec<_>>().join(" and ")),
        };
    }
}

/// The steps the checklist counts, in order: the regions' build to the end (the pass's own are the
/// agent's, before them).
pub const COUNTED: [&str; 32] = [
    "heritage-sites",
    "terrain",
    "terrain-lo",
    "slope",
    "slope-lo",
    "trees",
    "trees-lo",
    "depth-lo",
    "depth",
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
];

/// The regions' steps' items, unsized, for before there's a pass to size them by.
pub fn checklist_to_come() -> Vec<Item> {
    COUNTED.iter().filter(|s| **s != "catalog-held").map(|s| Item::left(s, None)).collect()
}

/// The works a chain would still run, one after another, as if each succeeded (its targets recorded
/// with their keys as they are now).
fn remaining(done: &Keys, next: impl Fn(&Keys) -> Option<Work>) -> Vec<Work> {
    let mut d = done.clone();
    let mut out: Vec<Work> = Vec::new();
    while let Some(w) = next(&d) {
        // (A work recording doesn't change (a prune: it reads the manifest alone) is the chain's
        // last that can be told now.)
        if out.len() >= 64 || out.last().is_some_and(|l| l.step == w.step && l.targets == w.targets) {
            break;
        }
        d.record(&w.step, &w.targets);
        out.push(w);
    }
    out
}

/// The chains' work still to run, as if each step succeeded (`remaining`): the roads' (the map
/// tiles, the road index, rail stops and ferries, the world-level terrain and slope), the trains',
/// the landmarks' and the 3D buildings', for the forecast (crate::agent::forecast).
pub fn chains_left(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
    reach: Option<&Reaches>,
    tiles: &TerrainTiles,
) -> [Vec<Work>; 5] {
    [
        remaining(done, |d| roads_chain(date, m, d, reach)),
        remaining(done, |d| rail_chain(cov, date, m, d, inputs)),
        remaining(done, |d| landmarks_chain(cov, date, m, d, inputs)),
        remaining(done, |d| bld_next(cov, m, d, inputs)),
        remaining(done, |d| depth_next(cov, m, d, tiles)),
    ]
}

/// A chain's items: a step's targets left (its works' to come), for each of `steps`; None while
/// the chain can't be told (`known` false).
fn chain_items(works: &[Work], steps: &[&str], known: bool) -> Vec<Item> {
    steps
        .iter()
        .map(|s| Item::left(s, known.then(|| works.iter().filter(|w| w.step == *s).map(|w| w.targets.len()).sum())))
        .collect()
}

/// The checklist's items and each stage's jobs to come (`next`): the regions' build to the end,
/// step by step (the pass's own steps are the agent's): the heritage sites, terrain, slope, tree
/// cover, the roads, the map tiles, the map's finishing, trains a day, the landmarks, the 3D
/// buildings, publishing.
/// `held`: the catalog is held for review (inputs/hold-catalog): publishing is its held copy.
/// `ready`: the regions a catalog would record as built now (`Plan::ready`).
#[allow(clippy::too_many_arguments)]
pub fn checklist(
    cov: &Coverage,
    date: &str,
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
    held: bool,
    reach: Option<&Reaches>,
    ready: &[String],
    tiles: &TerrainTiles,
) -> (Vec<Item>, BTreeMap<&'static str, Vec<String>>) {
    let count = |all: &[(String, String)], map: &BTreeMap<String, String>| {
        all.iter().filter(|(t, k)| map.get(t) == Some(k)).count()
    };
    let sized = |step: &str, all: &[(String, String)], map: &BTreeMap<String, String>, known: bool| {
        Item::sized(step, count(all, map), known.then_some(all.len()))
    };
    let mut out = Vec::new();
    let mut next: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let ts = terrain_slope_targets(cov, m, tiles);
    let sites_left = heritage_sites_work(cov, date, m, done, inputs).is_some()
        || !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources"));
    out.push(Item::left("heritage-sites", Some(sites_left as usize)));
    if sites_left {
        next.insert("sites", vec![super::steps::title("heritage-sites").into()]);
    }
    // (A slope piece whose key can't be told now isn't done.)
    out.push(sized("terrain", &ts.terrain, &done.terrain, true));
    out.push(sized("terrain-lo", &ts.terrain_lo, &done.terrain_lo, true));
    let known: Vec<(String, String)> = ts.slope.iter().filter_map(|(t, k)| Some((t.clone(), k.clone()?))).collect();
    let mut slope = sized("slope", &known, &done.slope, true);
    slope.total = Some(ts.slope.len());
    slope.left = Some(ts.slope.len() - slope.done);
    out.push(slope);
    out.push(sized("slope-lo", &ts.slope_lo, &done.slope_lo, true));
    let tt = crate::treepacks::targets_with(cov, m, tiles.keyed(m).and_then(|k| k.trees.as_deref()));
    let pieces: Vec<(String, String)> = tt.pieces.iter().map(|(t, k, _)| (t.clone(), k.clone())).collect();
    let assemblies: Vec<(String, String)> = tt.lo.iter().map(|(q, k, _)| (q.clone(), k.clone())).collect();
    out.push(sized("trees", &pieces, &done.trees, true));
    out.push(sized("trees-lo", &assemblies, &done.trees_lo, true));
    // (Known once the depth sources are read and the water's digests are worked out.)
    let dt = depth_targets(cov, m, tiles);
    let dk = dt.as_ref().is_some_and(|d| d.unknown == 0);
    let empty = DepthTargets::default();
    let d = dt.as_ref().unwrap_or(&empty);
    out.push(sized("depth-lo", &d.lo, &done.depth_lo, dk));
    out.push(sized("depth", &d.hi, &done.depth, dk));
    next.insert("depth", next_of(&remaining(done, |k| depth_next(cov, m, k, tiles))));
    let pieces = m.keys().any(|l| l.starts_with(&format!("sources/osm/{date}/pieces/")));
    let units = unit_keys(cov, date, m, reach, tiles);
    // (One whose key can't be worked out now isn't done.)
    let known: Vec<(String, String)> = units.iter().filter_map(|(u, k)| Some((u.slash(), k.clone()?))).collect();
    let mut line = sized("unit", &known, &done.unit, pieces && reach.is_some());
    if line.total.is_some() {
        line.total = Some(units.len());
        line.left = Some(units.len() - line.done);
    }
    out.push(line);
    let (packs, lo) = pack_lo_targets(m, reach);
    let built = m.keys().any(|l| l.starts_with("base/"));
    let fresh = packs.iter().filter(|(t, k)| done.pack.get(t) == Some(k)).count();
    out.push(Item::sized("pack", fresh, built.then_some(packs.len())));
    out.push(sized("lo", &lo, &done.lo, built));
    let roads: Vec<Work> = remaining(done, |d| roads_chain(date, m, d, reach))
        .into_iter()
        .filter(|w| !matches!(w.step.as_str(), "pack" | "lo"))
        .collect();
    out.extend(chain_items(
        &roads,
        &["prune", "roadunits", "stations", "ferries", "terrain-root", "slope-root"],
        built,
    ));
    next.insert("finish", next_of(&roads));
    // (A run of rail-feeds is followed by rail, whose key reads what it writes. Unknown while the
    // chain can't go on: before the rail sources are seeded, while inputs/keys.env can't be read,
    // without the feeds' list or the pass's rail set.)
    let rail = rail_next(cov, date, m, done, inputs);
    let trains: Vec<Work> = match &rail {
        Some(Some(w)) if w.step == "rail-feeds" => {
            vec![w.clone(), Work { step: "rail".into(), targets: vec![("rail".into(), String::new())] }]
        }
        Some(Some(w)) => vec![w.clone()],
        _ => Vec::new(),
    };
    out.extend(chain_items(&trains, &["rail-feeds", "rail"], rail.is_some()));
    next.insert("trains", next_of(&trains));
    let landmarks = remaining(done, |d| landmarks_chain(cov, date, m, d, inputs));
    out.extend(chain_items(&landmarks, &["pois", "peaks", "items", "heritage", "marks", "overlays"], pieces));
    next.insert("landmarks", next_of(&landmarks));
    // (Its tiles' normalized files and tiles together; known once the sources' indexes read.)
    let bt = bld_targets(cov, m, inputs);
    let bld_known = bld_sources(inputs) == Some(true);
    let bld = remaining(done, |d| bld_next(cov, m, d, inputs));
    out.extend(chain_items(&bld, &["bld-fetch"], bld_known));
    out.push(sized("bldprep", &bt.prep, &done.bldprep, bld_known));
    out.push(sized("bldtiles", &bt.tiles, &done.bldtiles, bld_known));
    let lo: Vec<(String, String)> = bt.lo.iter().map(|(q, k, _)| (q.clone(), k.clone())).collect();
    out.push(sized("bldfield-lo", &lo, &done.bldfield_lo, bld_known));
    next.insert("buildings", next_of(&bld));
    let key = catalog_key(m, inputs, ready);
    let publish_left = if held {
        done.catalog_held.as_deref() != Some(key.as_str())
    } else {
        done.catalog.as_deref() != Some(key.as_str())
    } as usize;
    // (Its one job is the line itself: no `next`.)
    out.push(Item::left(if held { "catalog-held" } else { "catalog" }, built.then_some(publish_left)));
    (out, next)
}

/// A catalog when what it would list or record has changed since the last one (not while the
/// regions can't be read: `inputs` "regions" "?"). `ready`: the regions it records as built
/// (`Plan::ready`).
/// The catalog, when the last (`held`: the last held one) has other files or regions than one made
/// now would; none while the regions can't be read.
fn catalog_work(
    m: &BTreeMap<String, String>,
    done: &Keys,
    inputs: &BTreeMap<String, String>,
    ready: &[String],
    held: bool,
) -> Option<Work> {
    if inputs.get("regions").map(String::as_str) == Some("?") {
        return None;
    }
    let k = catalog_key(m, inputs, ready);
    let last = if held { &done.catalog_held } else { &done.catalog };
    (last.as_deref() != Some(k.as_str())).then(|| Work { step: "catalog".into(), targets: vec![("catalog".into(), k)] })
}

/// What a catalog would list and record, hashed: the served files' logical and content names, the
/// regions (`inputs` "regions": their recipes and outline files), so a region renamed, or drawn
/// inside another, gets a catalog that records it, which of them it records as built (`ready`),
/// and, once any input has a description, the descriptions' credits (`inputs` "credits":
/// crate::inputs::credits::digest), so an edited credit gets a new catalog without rebuilding
/// anything.
pub fn catalog_key(m: &BTreeMap<String, String>, inputs: &BTreeMap<String, String>, ready: &[String]) -> String {
    let mut served: Vec<String> = m
        .iter()
        .filter(|(l, _)| {
            ["layers/", "base/", "hidata/", "markdata/", "ovdata/", "global/"].iter().any(|p| l.starts_with(p))
                || l.ends_with("/outlines")
        })
        .map(|(l, c)| format!("{l}={c}"))
        .collect();
    served.push(format!("regions {}", inputs.get("regions").map(String::as_str).unwrap_or("-")));
    served.push(format!("ready {}", ready.join(",")));
    if let Some(c) = inputs.get("credits") {
        served.push(format!("credits {c}"));
    }
    let refs: Vec<&str> = served.iter().map(String::as_str).collect();
    h(&refs)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;
    use crate::reach::{LongWay, Reach};

    pub(crate) fn e7box(w: f64, s: f64, e: f64, n: f64) -> [i32; 4] {
        [(w * 1e7) as i32, (s * 1e7) as i32, (e * 1e7) as i32, (n * 1e7) as i32]
    }

    /// The tests' reaches: 6/28/16's roads around Reykjavik (where `cov` is), 6/40/20's far away.
    fn reach() -> Reaches {
        let mut r = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        r.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![] });
        r.units.insert("6/40/20".into(), Reach { owned: Some(e7box(45.0, 40.0, 46.0, 41.0)), long: vec![] });
        r
    }

    // The planner with the tests' reaches, nothing on the map yet: its work.
    fn plan(
        c: &Coverage,
        date: &str,
        m: &BTreeMap<String, String>,
        done: &Keys,
        inputs: &BTreeMap<String, String>,
    ) -> Vec<Work> {
        plan_with(c, date, m, done, inputs, &BTreeMap::new(), None).work
    }
    fn plan_with(
        c: &Coverage,
        date: &str,
        m: &BTreeMap<String, String>,
        done: &Keys,
        inputs: &BTreeMap<String, String>,
        on_map: &BTreeMap<String, bool>,
        since_publish: Option<u64>,
    ) -> Plan {
        let each = c.by_region();
        super::plan(
            c,
            date,
            m,
            done,
            inputs,
            Some(&reach()),
            &tiles_for(m),
            Publish {
                each: &each,
                on_map,
                served: &BTreeMap::new(),
                since_catalog: since_publish,
                held: false,
                way: Default::default(),
            },
        )
    }
    /// `plan`'s work once the publishing chain's and catalogs listed first are done (as their jobs
    /// do them): the regions' work after them.
    fn past_publishing(c: &Coverage, date: &str, m: &mut BTreeMap<String, String>, done: &mut Keys) -> Vec<Work> {
        loop {
            let w = plan(c, date, m, done, &BTreeMap::new());
            match w.first() {
                Some(x) if CHAIN.contains(&x.step.as_str()) || x.step == "catalog" => did(m, done, x),
                _ => return w,
            }
        }
    }
    fn unit_keys(c: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
        ukeys(c, date, m, Some(&reach()))
    }
    /// The checklist's lines, folded by stage (as the agent's status has them).
    fn checklist(
        c: &Coverage,
        date: &str,
        m: &BTreeMap<String, String>,
        done: &Keys,
        inputs: &BTreeMap<String, String>,
        held: bool,
    ) -> Vec<Line> {
        let ready = plan_with(c, date, m, done, inputs, &BTreeMap::new(), None).ready;
        let (items, next) = super::checklist(c, date, m, done, inputs, held, Some(&reach()), &ready, &tiles_for(m));
        fold(&items, &next, &BTreeMap::new())
    }

    /// The indexes of the terrain packs `m` names, each a whole pyramid (a hi pack's z9–12 under
    /// its z6 tile, a lo pack's z3–8 under its z3 tile, the root's z0–2), every tile's XXH3 made from
    /// its pack's content name: a pack made again with other bytes changes all its tiles.
    pub(crate) fn tiles_for(m: &BTreeMap<String, String>) -> TerrainTiles {
        let mut t = TerrainTiles::new(None);
        for (l, c) in TerrainTiles::packs(m) {
            let Some(root) = l.rsplit('/').next().and_then(Unit::parse) else { continue };
            let (z0, z1) = match l.split('/').nth(2) {
                Some("hi") => (9, 12),
                Some("lo") => (3, 8),
                _ => (0, 2),
            };
            let mut tiles = Vec::new();
            for z in z0..=z1 {
                let d = z - root.z;
                for x in root.x << d..(root.x + 1) << d {
                    for y in root.y << d..(root.y + 1) << d {
                        tiles.push((z, x, y, store::naming::xxh3(format!("{c} {z}/{x}/{y}").as_bytes())));
                    }
                }
            }
            t.hold(c, tiles);
        }
        // (The declared elevation datasets `elevation_inputs` names: one, the world's.)
        if let Some(i) = m.get(&crate::inputs::logical(crate::inputs::elevation::UNIT)) {
            t.hold_elevation(i, vec![dataset("world", "w1", None)]);
        }
        t
    }

    /// The declared elevation datasets switched to, as the units need them (crate::inputs::elevation).
    pub(crate) fn elevation_inputs(m: &mut BTreeMap<String, String>) {
        m.insert(
            crate::inputs::logical(crate::inputs::elevation::UNIT),
            "sources/inputs/elevation/@index.5656565656565656.json".into(),
        );
        m.insert(
            crate::inputs::switched_logical(crate::inputs::elevation::UNIT),
            "sources/inputs/elevation/@switched.5757575757575757.json".into(),
        );
    }

    /// `unit_keys` with `tiles_for(m)` (every unit's key known; the declared elevation datasets
    /// switched to when `m` doesn't say).
    pub(crate) fn ukeys(
        c: &Coverage,
        date: &str,
        m: &BTreeMap<String, String>,
        reach: Option<&Reaches>,
    ) -> Vec<(Unit, String)> {
        let mut m = m.clone();
        if !m.contains_key(&crate::inputs::switched_logical(crate::inputs::elevation::UNIT)) {
            elevation_inputs(&mut m);
        }
        super::unit_keys(c, date, &m, reach, &tiles_for(&m))
            .into_iter()
            .map(|(u, k)| (u, k.expect("its key")))
            .collect()
    }

    fn cov() -> Coverage {
        let d = tempfile::tempdir().unwrap();
        Coverage::fixture(
            &[Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:-21.9,64.13,20".into()] }],
            None,
            d.path(),
        )
        .unwrap()
    }

    /// What the units wait for besides terrain and slope: the heritage-sites job's inputs (the
    /// pass's areas set, the heritage registers' accepted version), and the release's roadside
    /// buildings.
    pub(crate) fn unit_inputs(m: &mut BTreeMap<String, String>, date: &str) {
        m.insert(
            crate::osmpass::set_name(date, "areas"),
            format!("sources/osm/{date}/sets/areas.1212121212121212.osm.pbf"),
        );
        m.insert(
            crate::inputs::logical(crate::inputs::heritage::UNIT),
            "sources/inputs/heritage/@index.3434343434343434.json".into(),
        );
        m.insert(
            crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE),
            format!("{}.6767676767676767.json", crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE)),
        );
        elevation_inputs(m);
    }

    /// The heritage-sites job done: its key recorded, its outputs in the manifest.
    pub(crate) fn heritage_done(m: &mut BTreeMap<String, String>, done: &mut Keys, date: &str, w: &Work) {
        assert_eq!(w.step, "heritage-sites");
        done.record(&w.step, &w.targets);
        m.insert(
            crate::heritage::base_logical(date, "heritage-sources"),
            format!("work/heritage/{date}/base/heritage-sources.5656565656565656.json"),
        );
    }

    /// The rest of the heritage chain done too (it runs beside the regions' work from the start:
    /// the tests of the regions' order leave it out).
    pub(crate) fn heritage_chain_done(c: &Coverage, m: &BTreeMap<String, String>, done: &mut Keys, date: &str) {
        let k = heritage_key(c, date, m).unwrap();
        done.record("heritage", &[("heritage".to_string(), k)]);
    }

    /// Work done as its job does it: its targets recorded, and a tree cover, terrain or slope
    /// piece's mid in the manifest (named by its key), or dropped for one of "none".
    pub(crate) fn did(m: &mut BTreeMap<String, String>, done: &mut Keys, w: &Work) {
        done.record(&w.step, &w.targets);
        // (Terrain's and slope's pieces: their mids, named by their keys; a "none" piece's hi pack
        // and mid dropped.)
        if w.step == "terrain" || w.step == "slope" {
            for (t, k) in &w.targets {
                let Some(u) = Unit::parse(t).filter(|u| u.z == 6) else { continue };
                let l = if w.step == "terrain" {
                    crate::terrain_pack::mid_logical(u.x, u.y)
                } else {
                    crate::slope_pack::mid_logical(u.x, u.y)
                };
                if *k == none_piece_key(&w.step, t) {
                    m.remove(&l);
                    m.remove(&format!("layers/{}/hi/{}", w.step, u.dash()));
                } else {
                    m.insert(l.clone(), format!("{l}.{k}.sect"));
                }
            }
        }
        if w.step == "trees" {
            for (t, k) in &w.targets {
                let Some(u) = Unit::parse(t).filter(|u| u.z == 6) else { continue };
                let l = crate::treepacks::mid_logical(u.x, u.y);
                if *k == store::naming::hash16(format!("trees {}|{t}|none", crate::treepacks::TREES_V).as_bytes()) {
                    m.remove(&l);
                } else {
                    m.insert(l.clone(), format!("{l}.{k}.sect"));
                }
            }
        }
    }

    /// Terrain's or slope's work done as its job does it (`did`), with the packs it writes: a
    /// piece's hi pack, an assembly's lo pack (named by its key).
    pub(crate) fn terrain_did(m: &mut BTreeMap<String, String>, done: &mut Keys, w: &Work) {
        did(m, done, w);
        let layer = w.step.trim_end_matches("-lo");
        for (t, k) in &w.targets {
            let Some(u) = Unit::parse(t) else { continue };
            if *k == none_piece_key(layer, t) {
                continue;
            }
            if *k == none_lo_key(layer, t) {
                m.remove(&format!("layers/{layer}/lo/{}", u.dash()));
                continue;
            }
            let l = format!("layers/{layer}/{}/{}", if u.z == 6 { "hi" } else { "lo" }, u.dash());
            m.insert(l.clone(), format!("{l}.{k}.pack"));
        }
    }

    #[test]
    fn a_slope_pieces_key_reads_its_neighbours_edges_alone() {
        // Piece 6/28/16 (its z9 tiles, no finer) beside 6/29/16, in area 3/3/2: its z9 tiles on
        // the east edge read 6/29/16's west column of z9 tiles, not its others.
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for (l, c) in
            [("layers/terrain/hi/6-28-16", "a"), ("layers/terrain/hi/6-29-16", "b"), ("layers/terrain/lo/3-3-2", "l")]
        {
            m.insert(l.into(), c.into());
        }
        let key = |edge: u64, inside: u64, own: u64| {
            let mut t = TerrainTiles::new(None);
            t.hold("a", (224..232u32).flat_map(|x| (128..136u32).map(move |y| (9, x, y, own))));
            t.hold(
                "b",
                (232..240u32).flat_map(|x| (128..136u32).map(move |y| (9, x, y, if x == 232 { edge } else { inside }))),
            );
            t.hold(
                "l",
                (3..=8u8).flat_map(|z| {
                    let s = 1u32 << (z - 3);
                    (3 * s..4 * s).flat_map(move |x| (2 * s..3 * s).map(move |y| (z, x, y, 7)))
                }),
            );
            slope_piece_key((28, 16), &m, &t).unwrap()
        };
        let k = key(1, 1, 1);
        assert_eq!(key(1, 2, 1), k, "the neighbour's inside: not read");
        assert_ne!(key(2, 1, 1), k, "its edge: read");
        assert_ne!(key(1, 1, 2), k, "its own");
        // The tiles it reads: its own z9 tiles and z6–8 (from the lo pack), the neighbours' edges.
        let mut t = TerrainTiles::new(None);
        t.hold("a", (224..232u32).flat_map(|x| (128..136u32).map(move |y| (9, x, y, 1))));
        t.hold("b", (232..240u32).flat_map(|x| (128..136u32).map(move |y| (9, x, y, 1))));
        assert!(slope_piece_reads((28, 16), &m, &t).is_err(), "the lo pack's index not read yet: can't be told");
        t.hold("l", [(6, 28, 16, 6)]);
        let reads = slope_piece_reads((28, 16), &m, &t).unwrap();
        assert!(reads.contains(&Some((9, 232, 130, 1))) && !reads.contains(&Some((9, 233, 130, 1))), "{reads:?}");
        // (Its z6–8 tiles are each covered whole by their children: worked out from them, their
        // terrain unread. Its other edges' neighbours have no tile: read as none.)
        assert!(!reads.contains(&Some((6, 28, 16, 6))) && reads.contains(&None), "{reads:?}");
    }

    /// The terrain and slope hi packs and mids of z6 tiles the coverage has left are "none" pieces,
    /// run whenever (no assembly waits for them), their areas left until they are: the units
    /// reading them wait. Their runs drop them, and then they're gone from the targets. A piece
    /// near the coverage that makes no hi tile is keyed apart: a record from before isn't current.
    #[test]
    fn packs_the_coverage_has_left_are_dropped_by_none_pieces() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let pieces: Vec<(u32, u32)> = coverage_tiles(&c).into_values().flatten().collect();
        // Everything built: the coverage's pieces and assembly, and two z6 tiles it has left, one
        // in its area (6/24/23), one in another (6/40/20, terrain alone, its slope mid alone).
        for &(x, y) in &pieces {
            for l in [format!("layers/terrain/hi/6-{x}-{y}"), format!("layers/slope/hi/6-{x}-{y}")] {
                m.insert(l.clone(), format!("{l}.1111111111111111.pack"));
            }
            for l in [crate::terrain_pack::mid_logical(x, y), crate::slope_pack::mid_logical(x, y)] {
                m.insert(l.clone(), format!("{l}.1111111111111111.sect"));
            }
        }
        for l in [
            "layers/terrain/lo/3-3-2",
            "layers/slope/lo/3-3-2",
            "layers/terrain/hi/6-24-23",
            "layers/slope/hi/6-24-23",
            "layers/terrain/hi/6-40-20",
        ] {
            m.insert(l.into(), format!("{l}.2222222222222222.pack"));
        }
        for l in [crate::terrain_pack::mid_logical(24, 23), crate::slope_pack::mid_logical(40, 20)] {
            m.insert(l.clone(), format!("{l}.2222222222222222.sect"));
        }
        // And 3/5/2's lo packs, a z3 tile the coverage has left: "none" assemblies.
        for l in ["layers/terrain/lo/3-5-2", "layers/slope/lo/3-5-2"] {
            m.insert(l.into(), format!("{l}.3333333333333333.pack"));
        }
        let tt = terrain_slope_targets(&c, &m, &tiles_for(&m));
        assert_eq!(tt.terrain_lo_none, vec![("3/5/2".to_string(), none_lo_key("terrain", "3/5/2"))]);
        assert_eq!(tt.slope_lo_none, vec![("3/5/2".to_string(), none_lo_key("slope", "3/5/2"))]);
        assert!(!tt.terrain_lo.iter().any(|(q, _)| q == "3/5/2"));
        assert!(!pieces.contains(&(24, 23)) && !pieces.contains(&(40, 20)));
        assert_eq!(
            tt.terrain_none,
            vec![
                ("6/24/23".to_string(), none_piece_key("terrain", "6/24/23")),
                ("6/40/20".to_string(), none_piece_key("terrain", "6/40/20"))
            ]
        );
        assert_eq!(
            tt.slope_none,
            vec![
                ("6/24/23".to_string(), none_piece_key("slope", "6/24/23")),
                ("6/40/20".to_string(), none_piece_key("slope", "6/40/20"))
            ]
        );
        assert!(!tt.terrain.iter().any(|(t, _)| t == "6/24/23" || t == "6/40/20"), "not pieces");
        let mut done = Keys::default();
        done.record("terrain", &tt.terrain);
        done.record("terrain-lo", &tt.terrain_lo);
        done.record("slope", &tt.slope.iter().map(|(t, k)| (t.clone(), k.clone().unwrap())).collect::<Vec<_>>());
        done.record("slope-lo", &tt.slope_lo);
        let tw = terrain_work(&tt, &m, &done);
        assert_eq!(tw.terrain, tt.terrain_none, "the none pieces alone");
        assert_eq!(tw.slope, tt.slope_none, "slope's whenever: they read no terrain");
        assert_eq!(tw.terrain_lo, tt.terrain_lo_none, "the none assembly alone, whenever");
        assert_eq!(tw.slope_lo, tt.slope_lo_none, "3/3/2's slope assembly waits for its none piece");
        assert_eq!(tw.terrain_left, ["3/3/2", "3/5/2"].map(String::from).into(), "their areas left until they've run");
        assert!(tw.terrain_pieces_left.contains("6/24/23") && tw.slope_pieces_left.contains("6/40/20"));
        // Run: their packs and mids dropped, gone from the targets.
        terrain_did(&mut m, &mut done, &Work { step: "terrain".into(), targets: tw.terrain.clone() });
        terrain_did(&mut m, &mut done, &Work { step: "slope".into(), targets: tw.slope.clone() });
        terrain_did(&mut m, &mut done, &Work { step: "terrain-lo".into(), targets: tw.terrain_lo.clone() });
        terrain_did(&mut m, &mut done, &Work { step: "slope-lo".into(), targets: tw.slope_lo.clone() });
        assert!(!m.keys().any(|l| l.ends_with("6-24-23") || l.ends_with("6-40-20") || l.ends_with("3-5-2")), "{m:?}");
        let tt = terrain_slope_targets(&c, &m, &tiles_for(&m));
        assert!(
            tt.terrain_none.is_empty()
                && tt.slope_none.is_empty()
                && tt.terrain_lo_none.is_empty()
                && tt.slope_lo_none.is_empty()
        );
        let tw = terrain_work(&tt, &m, &done);
        assert!(tw.terrain.is_empty() && tw.terrain_left.is_empty(), "{tw:?}");
        // A run's hand-off waiting to be merged (its record on top, the manifest not yet changed):
        // not run again.
        m.insert("layers/terrain/hi/6-40-20".into(), "layers/terrain/hi/6-40-20.2222222222222222.pack".into());
        let tt = terrain_slope_targets(&c, &m, &tiles_for(&m));
        assert_eq!(tt.terrain_none.len(), 1);
        assert!(terrain_work(&tt, &m, &done).terrain.is_empty());
        // A piece making no hi tile (crate::terrain_pack::makes_hi): keyed apart from one that does.
        let d = tempfile::tempdir().unwrap();
        let near = crate::terrain_pack::tests::near_without_hi(d.path());
        let tt = terrain_slope_targets(&near, &BTreeMap::new(), &TerrainTiles::new(None));
        let fp = near.fingerprint(grown_e7(6, 21, 18, 20.0));
        let k = tt.terrain.iter().find(|(t, _)| t == "6/21/18").unwrap().1.clone();
        assert_eq!(k, piece_key_made((21, 18), &fp, super::super::tiles::TEST_SOURCES, "-", false));
        assert_ne!(
            k,
            terrain_piece_key((21, 18), &fp, super::super::tiles::TEST_SOURCES, "-"),
            "a record from before its hi pack was dropped isn't current"
        );
        let fp = near.fingerprint(grown_e7(6, 22, 18, 20.0));
        assert_eq!(
            tt.terrain.iter().find(|(t, _)| t == "6/22/18").unwrap().1,
            terrain_piece_key((22, 18), &fp, super::super::tiles::TEST_SOURCES, "-"),
            "one making hi tiles: keyed as before"
        );
    }

    /// Every step of the plan done as its job does it (`did`, `terrain_did`), a unit's candidates in
    /// the manifest once they're made; the steps run, in order, until nothing's left.
    fn run_all(c: &Coverage, reach: &Reaches, m: &mut BTreeMap<String, String>, done: &mut Keys) -> Vec<String> {
        let mut ran = Vec::new();
        for _ in 0..60 {
            let w = super::plan(
                c,
                "d",
                m,
                done,
                &BTreeMap::new(),
                Some(reach),
                &tiles_for(m),
                Publish {
                    each: &c.by_region(),
                    on_map: &BTreeMap::new(),
                    served: &BTreeMap::new(),
                    since_catalog: None,
                    held: false,
                    way: Default::default(),
                },
            )
            .work;
            let Some(w) = w.into_iter().next() else { return ran };
            ran.extend(w.targets.iter().map(|t| format!("{} {}", w.step, t.0)));
            match w.step.as_str() {
                "heritage-sites" => heritage_done(m, done, "d", &w),
                s if s.starts_with("terrain") || s.starts_with("slope") => terrain_did(m, done, &w),
                "pois" => {
                    did(m, done, &w);
                    for (t, _) in &w.targets {
                        let u = Unit::parse(t).unwrap();
                        m.insert(
                            format!("work/pois/{}", u.dash()),
                            format!("work/pois/{}.5151515151515151.json", u.dash()),
                        );
                    }
                }
                _ => did(m, done, &w),
            }
        }
        panic!("the plan never ran out: {ran:?}");
    }

    /// A terrain and slope hi pack the coverage has left within a unit's 30 km (6/29/16, beside
    /// Reykjavik's 6/28/16), and the lo packs of a z3 tile it has left (3/4/2): their "none" pieces
    /// and assemblies first, the unit and its peaks waiting for them, then built again once each,
    /// and the roots made again once; nothing after.
    #[test]
    fn a_units_terrain_dropped_where_the_coverage_has_left_rebuilds_it_once() {
        let c = cov();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![] });
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        unit_inputs(&mut m, "d");
        // (The pass's route ends, summits and worldwide z8: the candidates and their peaks run.)
        for l in [
            "work/trailends/d".to_string(),
            "work/summits/d".to_string(),
            crate::terrain_z8::logical(),
            crate::terrain_z8::max_logical(),
        ] {
            m.insert(l.clone(), format!("{l}.6161616161616161.bin"));
        }
        let mut done = Keys::default();
        let first = run_all(&c, &reach, &mut m, &mut done);
        assert!(
            first.contains(&"unit 6/28/16".to_string()) && first.contains(&"peaks 6/28/16".to_string()),
            "{first:?}"
        );
        assert!(!coverage_tiles(&c).into_values().flatten().any(|t| t == (29, 16)));
        // The stale packs, as an earlier, larger coverage left them, the unit and its peaks built
        // reading them (recorded under the keys that name them).
        for l in
            ["layers/terrain/hi/6-29-16", "layers/slope/hi/6-29-16", "layers/terrain/lo/3-4-2", "layers/slope/lo/3-4-2"]
        {
            m.insert(l.into(), format!("{l}.2222222222222222.pack"));
        }
        for layer in ["terrain", "slope"] {
            done.record(&format!("{layer}-root"), &[(format!("{layer}-root"), root_key(layer, &m).unwrap())]);
        }
        let (uk, pk) = (ukeys(&c, "d", &m, Some(&reach)), peaks_keys(&c, "d", &m));
        assert!(uk[0].1 != done.unit["6/28/16"] && pk[0].1 != done.peaks["6/28/16"], "their keys see the packs");
        done.record("unit", &[("6/28/16".to_string(), uk[0].1.clone())]);
        done.record("peaks", &[("6/28/16".to_string(), pk[0].1.clone())]);
        let w = super::plan(
            &c,
            "d",
            &m,
            &done,
            &BTreeMap::new(),
            Some(&reach),
            &tiles_for(&m),
            Publish {
                each: &c.by_region(),
                on_map: &BTreeMap::new(),
                served: &BTreeMap::new(),
                since_catalog: None,
                held: false,
                way: Default::default(),
            },
        )
        .work;
        let steps: Vec<String> =
            w.iter().flat_map(|w| w.targets.iter().map(|t| format!("{} {}", w.step, t.0))).collect();
        assert!(
            steps.contains(&"terrain 6/29/16".to_string()) && steps.contains(&"slope 6/29/16".to_string()),
            "{steps:?}"
        );
        assert!(
            !steps.iter().any(|s| s.starts_with("unit ") || s.starts_with("peaks ")),
            "they wait for the terrain: {steps:?}"
        );
        let then = run_all(&c, &reach, &mut m, &mut done);
        let n = |s: &str| then.iter().filter(|t| *t == s).count();
        assert_eq!(
            (n("terrain 6/29/16"), n("slope 6/29/16"), n("unit 6/28/16"), n("peaks 6/28/16")),
            (1, 1, 1, 1),
            "{then:?}"
        );
        assert_eq!(
            (n("terrain-lo 3/4/2"), n("slope-lo 3/4/2"), n("terrain-root terrain-root"), n("slope-root slope-root")),
            (1, 1, 1, 1),
            "{then:?}"
        );
        let at = |s: &str| then.iter().position(|t| t == s).unwrap();
        assert!(at("terrain 6/29/16") < at("unit 6/28/16") && at("terrain 6/29/16") < at("peaks 6/28/16"), "{then:?}");
        assert!(!m.keys().any(|l| l.ends_with("6-29-16") || l.ends_with("3-4-2")), "{m:?}");
        assert!(run_all(&c, &reach, &mut m, &mut done).is_empty());
    }

    /// A root isn't made while what makes the lo packs it reads is left (an assembly to run, or run
    /// and waiting to be merged: its record not yet there), only once after it.
    #[test]
    fn the_roots_wait_for_their_lo_packs() {
        let c = cov();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![] });
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        unit_inputs(&mut m, "d");
        let mut done = Keys::default();
        run_all(&c, &reach, &mut m, &mut done);
        let plan = |m: &BTreeMap<String, String>, done: &Keys| -> Vec<Work> {
            super::plan(
                &c,
                "d",
                m,
                done,
                &BTreeMap::new(),
                Some(&reach),
                &tiles_for(m),
                Publish {
                    each: &c.by_region(),
                    on_map: &BTreeMap::new(),
                    served: &BTreeMap::new(),
                    since_catalog: None,
                    held: false,
                    way: Default::default(),
                },
            )
            .work
        };
        assert!(plan(&m, &done).is_empty());
        // An assembly made again (its version bumped, say): its new lo pack written, its job not
        // yet merged (its record the older key), so its root's key is stale.
        for (step, layer, roots) in [
            ("slope-lo", "slope", vec!["slope-root slope-root"]),
            ("terrain-lo", "terrain", vec!["terrain-root terrain-root", "slope-root slope-root"]),
        ] {
            let mut d2 = done.clone();
            let map = if step == "slope-lo" { &mut d2.slope_lo } else { &mut d2.terrain_lo };
            map.insert("3/3/2".into(), "an older key".into());
            let mut m2 = m.clone();
            let l = format!("layers/{layer}/lo/3-3-2");
            m2.insert(l.clone(), format!("{l}.7777777777777777.pack"));
            let w = plan(&m2, &d2);
            let steps: Vec<&str> = w.iter().map(|w| w.step.as_str()).collect();
            assert!(steps.contains(&step) && !steps.iter().any(|s| s.ends_with("-root")), "{step}: {steps:?}");
            // Merged: the roots its lo pack feeds, once each.
            let w = w.into_iter().find(|w| w.step == step).unwrap();
            d2.record(&w.step, &w.targets);
            let after = run_all(&c, &reach, &mut m2, &mut d2);
            let made: Vec<&str> = after.iter().map(String::as_str).filter(|s| s.ends_with("-root")).collect();
            assert_eq!(made, roots, "{step}: {after:?}");
        }
    }

    /// The coverage leaving a z6 tile, coming back, and leaving again: a piece, then a "none"
    /// piece, then a piece, then "none" again, each run once, its records never taken for another's.
    #[test]
    fn a_z6_tile_the_coverage_leaves_and_comes_back_to() {
        let d = tempfile::tempdir().unwrap();
        let small = cov();
        let big = Coverage::fixture(
            &[Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:-21.9,64.13,20".into(), "place:47.8,54.2,5".into()],
            }],
            None,
            d.path(),
        )
        .unwrap();
        assert!(coverage_tiles(&big).into_values().flatten().any(|t| t == (40, 20)));
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let step = |c: &Coverage, m: &mut BTreeMap<String, String>, done: &mut Keys| -> Vec<String> {
            let mut ran = Vec::new();
            for _ in 0..10 {
                let tw = terrain_work(&terrain_slope_targets(c, m, &tiles_for(m)), m, done);
                let works = [
                    ("terrain", tw.terrain),
                    ("terrain-lo", tw.terrain_lo),
                    ("slope", tw.slope),
                    ("slope-lo", tw.slope_lo),
                ];
                let Some((s, ts)) = works.into_iter().find(|(_, t)| !t.is_empty()) else { return ran };
                ran.extend(
                    ts.iter().filter(|t| t.0.ends_with("40/20") || t.0 == "3/5/2").map(|t| format!("{s} {}", t.0)),
                );
                terrain_did(m, done, &Work { step: s.into(), targets: ts });
            }
            panic!("never done: {ran:?}");
        };
        assert_eq!(
            step(&big, &mut m, &mut done),
            ["terrain 6/40/20", "terrain-lo 3/5/2", "slope 6/40/20", "slope-lo 3/5/2"]
        );
        assert!(m.contains_key("layers/terrain/hi/6-40-20") && m.contains_key("layers/terrain/lo/3-5-2"));
        assert_eq!(
            step(&small, &mut m, &mut done),
            ["terrain 6/40/20", "terrain-lo 3/5/2", "slope 6/40/20", "slope-lo 3/5/2"],
            "none"
        );
        assert!(!m.keys().any(|l| l.ends_with("6-40-20") || l.ends_with("3-5-2")), "{m:?}");
        assert_eq!(
            step(&big, &mut m, &mut done),
            ["terrain 6/40/20", "terrain-lo 3/5/2", "slope 6/40/20", "slope-lo 3/5/2"],
            "a piece again"
        );
        assert_eq!(
            step(&small, &mut m, &mut done),
            ["terrain 6/40/20", "terrain-lo 3/5/2", "slope 6/40/20", "slope-lo 3/5/2"],
            "none again"
        );
        assert!(step(&small, &mut m, &mut done).is_empty());
    }

    /// A "none" piece's or assembly's hand-off removes files and writes none: the lead's merge
    /// check (agent::pool::check) and the steps table's write-sets take it.
    #[test]
    fn a_removal_only_hand_off_is_taken() {
        use crate::pool::journal::Entry;
        for (step, t, ls) in [
            ("terrain", "6/40/20", vec!["layers/terrain/hi/6-40-20", "work/terrain-mid/6-40-20"]),
            ("slope", "6/40/20", vec!["layers/slope/hi/6-40-20", "work/slope-mid/6-40-20"]),
            ("terrain-lo", "3/5/2", vec!["layers/terrain/lo/3-5-2"]),
            ("slope-lo", "3/5/2", vec!["layers/slope/lo/3-5-2"]),
        ] {
            let mut h = crate::handoff::Handoff::default();
            for l in &ls {
                h.changes.insert(l.to_string(), None);
            }
            let k = if step.ends_with("-lo") {
                none_lo_key(step.trim_end_matches("-lo"), t)
            } else {
                none_piece_key(step, t)
            };
            h.done = Some((step.to_string(), vec![(t.to_string(), k)]));
            let e = Entry {
                member: "m-0000000000000001".into(),
                lease: crate::pool::journal::LeaseId { term: 1, n: 1 },
                step: step.into(),
                handoff: h,
                at: 1_791_500_000,
            };
            assert_eq!(crate::agent::pool::check(&e, &crate::pool::records::Records::default()), Ok(()), "{step}");
            assert_eq!(crate::agent::steps::outside(&e), None, "{step}");
        }
    }

    #[test]
    fn terrain_first_then_slope_then_catalog() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        unit_inputs(&mut m, "2026-09-28");
        let mut done = Keys::default();
        // The heritage sites first (one job: a helper's units then wait only for the terrain).
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w.iter().map(|x| x.step.as_str()).collect::<Vec<_>>(), vec!["heritage-sites", "terrain", "trees"]);
        heritage_done(&mut m, &mut done, "2026-09-28", &w[0]);
        // (The rest of the heritage chain, from now on, after the regions' work.)
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w.iter().map(|x| x.step.as_str()).collect::<Vec<_>>(), vec!["terrain", "trees", "heritage"]);
        heritage_chain_done(&c, &m, &mut done, "2026-09-28");
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w.len(), 2);
        // The terrain's pieces (its z6 tiles near the coverage), then their area's assembly from
        // their mids; slope's pieces once that's built, then its assembly.
        assert_eq!(w[0].step, "terrain");
        let pieces: Vec<String> =
            coverage_tiles(&c).into_values().flatten().map(|(x, y)| format!("6/{x}/{y}")).collect();
        assert_eq!(w[0].targets.iter().map(|t| t.0.clone()).collect::<Vec<_>>(), pieces);
        terrain_did(&mut m, &mut done, &w[0]);
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(
            (w[0].step.as_str(), w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>()),
            ("terrain-lo", vec!["3/3/2"])
        );
        assert!(!w.iter().any(|x| x.step == "slope"), "slope once the terrain's assembled");
        terrain_did(&mut m, &mut done, &w[0]);
        // (No units here: everything built, so each layer is published as it's made, the terrain's
        // root once its assembly is.)
        assert_eq!(plan(&c, "2026-09-28", &m, &done, &BTreeMap::new())[0].step, "terrain-root");
        let w = past_publishing(&c, "2026-09-28", &mut m, &mut done);
        assert_eq!(w[0].step, "slope");
        assert_eq!(w[0].targets.len(), pieces.len());
        assert!(!w.iter().any(|x| x.step == "slope-lo"), "its pieces first");
        terrain_did(&mut m, &mut done, &w[0]);
        let w = past_publishing(&c, "2026-09-28", &mut m, &mut done);
        assert_eq!((w[0].step.as_str(), w[0].targets[0].0.as_str()), ("slope-lo", "3/3/2"));
        terrain_did(&mut m, &mut done, &w[0]);
        // (The slope's root once its assembly is.)
        assert_eq!(plan(&c, "2026-09-28", &m, &done, &BTreeMap::new())[0].step, "slope-root");
        // The tree cover of the coverage's z6 tiles (pieces), then its z3 tile's assembly from their
        // mids, before the units.
        let w = past_publishing(&c, "2026-09-28", &mut m, &mut done);
        assert_eq!(
            (w[0].step.as_str(), w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>()),
            ("trees", vec!["6/28/16", "6/28/17"])
        );
        assert!(!w.iter().any(|x| x.step == "trees-lo"), "its pieces first");
        did(&mut m, &mut done, &w[0]);
        let w = past_publishing(&c, "2026-09-28", &mut m, &mut done);
        assert_eq!((w[0].step.as_str(), w[0].targets[0].0.as_str()), ("trees-lo", "3/3/2"));
        did(&mut m, &mut done, &w[0]);
        // Then a catalog with it all.
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "catalog");
        done.record("catalog", &w[0].targets);
        assert!(plan(&c, "2026-09-28", &m, &done, &BTreeMap::new()).is_empty(), "nothing more to do");
        // A region renamed (or drawn inside another): a catalog that records it, and nothing else.
        let renamed: BTreeMap<String, String> = [("regions".to_string(), "5a5a5a5a5a5a5a5a".to_string())].into();
        let w = plan(&c, "2026-09-28", &m, &done, &renamed);
        assert_eq!(w.iter().map(|w| w.step.as_str()).collect::<Vec<_>>(), vec!["catalog"]);
        // The regions unreadable for now: no catalog on that.
        let unread: BTreeMap<String, String> = [("regions".to_string(), "?".to_string())].into();
        assert!(plan(&c, "2026-09-28", &m, &done, &unread).is_empty());
        // New terrain content in one piece: the slope of it and of the pieces whose edges read it
        // again, then their assembly.
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.3333333333333333.pack".into());
        let w = past_publishing(&c, "2026-09-28", &mut m, &mut done);
        assert_eq!(w[0].step, "slope");
        let again: Vec<&str> = w[0].targets.iter().map(|t| t.0.as_str()).collect();
        assert!(again.contains(&"6/28/16") && again.len() < pieces.len(), "{again:?}");
        assert!(
            again
                .iter()
                .all(|t| *t == "6/28/16" || edge_neighbours(6, 28, 16).iter().any(|(x, y)| format!("6/{x}/{y}") == *t)),
            "{again:?}"
        );
    }

    /// Three regions in Iceland, all in z3 tile 3/3/2: a (Reykjavik: unit 6/28/16), b (Akureyri and
    /// Egilsstaðir: 6/29/16 and 6/30/16) and c (Vík: 6/31/16); their pieces and reaches, the units'
    /// inputs, and the heritage sites and the terrain done.
    fn three() -> (Coverage, Reaches, BTreeMap<String, String>, Keys) {
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, places: &[&str]| Recipe {
            id: id.into(),
            name: id.to_uppercase(),
            outline: places.iter().map(|p| format!("place:{p},20")).collect(),
        };
        let c = Coverage::fixture(
            &[r("a", &["-21.9,64.13"]), r("b", &["-18.1,65.68", "-14.4,65.26"]), r("c", &["-19.0,63.42"])],
            None,
            d.path(),
        )
        .unwrap();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        for (u, b) in [
            ("6/28/16", e7box(-22.0, 64.0, -21.7, 64.16)),
            ("6/29/16", e7box(-18.3, 65.6, -17.9, 65.8)),
            ("6/30/16", e7box(-14.6, 65.2, -14.2, 65.35)),
            ("6/31/16", e7box(-19.2, 63.35, -18.8, 63.5)),
        ] {
            reach.units.insert(u.into(), Reach { owned: Some(b), long: vec![] });
        }
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-16", "6-29-16", "6-30-16", "6-31-16"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        unit_inputs(&mut m, "d");
        let mut done = Keys::default();
        loop {
            let w = super::plan(
                &c,
                "d",
                &m,
                &done,
                &BTreeMap::new(),
                Some(&reach),
                &tiles_for(&m),
                Publish {
                    each: &c.by_region(),
                    on_map: &BTreeMap::new(),
                    served: &BTreeMap::new(),
                    since_catalog: None,
                    held: false,
                    way: Default::default(),
                },
            )
            .work;
            match w[0].step.as_str() {
                "heritage-sites" => heritage_done(&mut m, &mut done, "d", &w[0]),
                "terrain" | "terrain-lo" => terrain_did(&mut m, &mut done, &w[0]),
                _ => break,
            }
        }
        heritage_chain_done(&c, &m, &mut done, "d");
        (c, reach, m, done)
    }

    /// The tests' plan for `three`'s regions: those `on_map`, the last catalog `since` seconds ago,
    /// the switches' `way`, catalogs `held` or not, nothing served.
    #[allow(clippy::too_many_arguments)]
    fn pplan(
        c: &Coverage,
        reach: &Reaches,
        m: &BTreeMap<String, String>,
        done: &Keys,
        on_map: &BTreeMap<String, bool>,
        since: Option<u64>,
        way: Publishing,
        held: bool,
    ) -> Plan {
        let each = c.by_region();
        super::plan(
            c,
            "d",
            m,
            done,
            &BTreeMap::new(),
            Some(reach),
            &tiles_for(m),
            Publish { each: &each, on_map, served: &BTreeMap::new(), since_catalog: since, held, way },
        )
    }
    /// A unit of theirs built (as `content`).
    fn rbuild(
        c: &Coverage,
        reach: &Reaches,
        m: &mut BTreeMap<String, String>,
        done: &mut Keys,
        u: &str,
        content: &str,
    ) {
        let k = ukeys(c, "d", m, Some(reach)).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        done.record("unit", &[(u.to_string(), k)]);
        let d = u.replace('/', "-");
        m.insert(format!("base/{d}"), format!("base/{d}.{content}.base"));
        m.insert(format!("global/roads/{d}"), format!("global/roads/{d}.{content}.roads"));
    }
    /// A plan's steps, a step's works one after another as one (the plan lists a step's by region).
    fn steps(p: &Plan) -> Vec<String> {
        let mut v: Vec<String> = p.work.iter().map(|w| w.step.clone()).collect();
        v.dedup();
        v
    }
    /// The publishing chain's steps.
    const CHAIN: [&str; 8] = ["prune", "pack", "roadunits", "lo", "stations", "ferries", "terrain-root", "slope-root"];
    /// Through the publishing chain to its catalog, each step done as the plan lists it first (the
    /// slope and tree cover after it as `prep` says): the chain's steps run, and the plan the catalog
    /// came in (not recorded).
    fn through_the_chain(
        c: &Coverage,
        reach: &Reaches,
        m: &mut BTreeMap<String, String>,
        done: &mut Keys,
        way: Publishing,
        held: bool,
    ) -> (Vec<String>, Plan) {
        let mut ran = Vec::new();
        for _ in 0..30 {
            let p = pplan(c, reach, m, done, &BTreeMap::new(), Some(600), way, held);
            let w = p.work[0].clone();
            if w.step == "catalog" {
                return (ran, p);
            }
            assert!(CHAIN.contains(&w.step.as_str()), "{:?}", steps(&p));
            did(m, done, &w);
            ran.push(w.step);
        }
        panic!("no catalog");
    }

    #[test]
    fn regions_are_built_one_at_a_time_those_the_map_lacks_first() {
        let (c, reach, m, done) = three();
        let plan =
            |on_map: &BTreeMap<String, bool>| pplan(&c, &reach, &m, &done, on_map, None, Default::default(), false);
        let units = |p: &Plan| {
            p.work
                .iter()
                .filter(|w| w.step == "unit")
                .flat_map(|w| w.targets.iter().map(|t| t.0.clone()))
                .collect::<Vec<_>>()
        };
        // Nothing on the map: the regions with the fewest units left first (a and c, one each: by
        // place), then b's two together; their slope and tree cover after them (nothing to publish
        // before a unit is built).
        let p = plan(&BTreeMap::new());
        assert_eq!(steps(&p), ["unit", "slope", "trees"]);
        assert_eq!(p.publishing.why, "nothing to publish until a unit is built");
        assert_eq!(units(&p), ["6/28/16", "6/31/16", "6/29/16", "6/30/16"]);
        assert!(p.ready.is_empty());
        // a and c on the map already (as they are, or redrawn): b first, though it has more left.
        let on: BTreeMap<String, bool> = [("a".to_string(), true), ("c".to_string(), false)].into();
        assert_eq!(units(&plan(&on)), ["6/29/16", "6/30/16", "6/28/16", "6/31/16"]);
    }

    #[test]
    fn a_region_done_is_published_at_once_its_map_tiles_before_the_units_a_catalog_at_most_each_minute() {
        let (c, reach, mut m, mut done) = three();
        let none = BTreeMap::new();
        // Reykjavik's unit built: a is done. Its map tiles at once, then its area's slope and tree
        // cover, then the units.
        rbuild(&c, &reach, &mut m, &mut done, "6/28/16", "6666666666666666");
        let p = pplan(&c, &reach, &m, &done, &none, None, Default::default(), false);
        assert_eq!(steps(&p)[..4], ["pack", "slope", "trees", "unit"]);
        assert_eq!((p.publishing.tiles, p.publishing.units), (p.work[0].targets.len(), 1));
        assert!(p.publishing.why.starts_with("drawing the map tiles"), "{}", p.publishing.why);
        assert_eq!(
            p.publishing.regions,
            [PublishRegion { id: "a".into(), kinds: vec!["new region".into(), "slope and tree cover to make".into()] }]
        );
        // Through the chain (no units fixed as of a time: the records as they are), then a catalog
        // with the units still to build after it; a isn't recorded built yet (its slope and tree cover).
        let (ran, p) = through_the_chain(&c, &reach, &mut m, &mut done, Default::default(), false);
        assert_eq!(ran.first().map(String::as_str), Some("pack"), "{ran:?}");
        assert!(ran.contains(&"roadunits".to_string()) && ran.contains(&"lo".to_string()), "{ran:?}");
        assert!(p.ready.is_empty());
        assert!(steps(&p).contains(&"unit".to_string()));
        assert_eq!(p.publishing.why, "a catalog now: what it would list changed");
        done.record("catalog", &p.work[0].targets);
        // Caught up: the units again.
        let p = pplan(&c, &reach, &m, &done, &none, Some(5), Default::default(), false);
        assert_eq!(steps(&p)[..3], ["slope", "trees", "unit"]);
        assert!(p.publishing.why.starts_with("caught up"), "{}", p.publishing.why);
        // c done 30 s after that catalog: its map tiles at once; the catalog once a minute has
        // passed since the last.
        rbuild(&c, &reach, &mut m, &mut done, "6/31/16", "8888888888888888");
        assert_eq!(pplan(&c, &reach, &m, &done, &none, Some(30), Default::default(), false).work[0].step, "pack");
        through_the_chain(&c, &reach, &mut m, &mut done, Default::default(), false);
        let p = pplan(&c, &reach, &m, &done, &none, Some(30), Default::default(), false);
        assert!(!p.work.iter().any(|w| w.step == "catalog"), "{:?}", steps(&p));
        assert_eq!(p.publishing.wait_s, 30);
        assert!(p.publishing.why.contains("in 30 s"), "{}", p.publishing.why);
        let p = pplan(&c, &reach, &m, &done, &none, Some(CATALOG_EVERY_S), Default::default(), false);
        assert_eq!((p.work[0].step.as_str(), p.publishing.wait_s), ("catalog", 0));
        // (The log line is the same as the seconds count down, and as the tiles are drawn.)
        let at = |s: u64| pplan(&c, &reach, &m, &done, &none, Some(s), Default::default(), false).publishing.key();
        assert_eq!(at(10), at(40));
        // a's slope and tree cover made: recorded built by the next catalog.
        loop {
            let p = pplan(&c, &reach, &m, &done, &none, Some(600), Default::default(), false);
            if p.ready.contains(&"a".to_string()) {
                let (_, p) = through_the_chain(&c, &reach, &mut m, &mut done, Default::default(), false);
                assert!(p.ready.contains(&"a".to_string()) && p.work[0].step == "catalog");
                break;
            }
            let w = p.work.iter().find(|w| w.step != "catalog" && !CHAIN.contains(&w.step.as_str())).unwrap().clone();
            assert!(["slope", "slope-lo", "trees", "trees-lo"].contains(&w.step.as_str()), "{:?}", steps(&p));
            if w.step.starts_with("slope") {
                terrain_did(&mut m, &mut done, &w);
            } else {
                did(&mut m, &mut done, &w);
            }
        }
    }

    #[test]
    fn the_map_tiles_are_drawn_a_units_together_those_its_change_made_stale() {
        let (c, reach, mut m, mut done) = three();
        rbuild(&c, &reach, &mut m, &mut done, "6/28/16", "6666666666666666");
        rbuild(&c, &reach, &mut m, &mut done, "6/31/16", "8888888888888888");
        let (packs, _) = pack_lo_targets(&m, Some(&reach));
        let owned = |u: &str, t: &str| {
            let (u, t) = (Unit::parse(u).unwrap(), Unit::parse(t).unwrap());
            owns(unit_reach(Some(&reach), &m, u), crate::hipack::tile_bounds(t.z, t.x, t.y))
        };
        let sorted = |mut v: Vec<(String, String)>| {
            v.sort();
            v
        };
        // Nothing served: both units changed; a's (west of c) first, then c's.
        let (t, n) = by_unit(packs.clone(), &m, Some(&reach), &BTreeMap::new());
        assert_eq!((n, sorted(t.clone())), (2, sorted(packs.clone())));
        let first_c = t.iter().position(|x| !owned("6/28/16", &x.0)).unwrap();
        assert!(first_c > 0 && t[first_c..].iter().all(|x| owned("6/31/16", &x.0)), "{t:?}");
        // a served as it is: c's change alone; its tiles first, a's (no change explains them) after.
        let served: BTreeMap<String, String> =
            ["base/6-28-16", "global/roads/6-28-16"].into_iter().map(|l| (l.to_string(), m[l].clone())).collect();
        let (t, n) = by_unit(packs.clone(), &m, Some(&reach), &served);
        assert_eq!(n, 1);
        let k = t.iter().take_while(|x| owned("6/31/16", &x.0)).count();
        assert!(k > 0 && t[k..].iter().all(|x| !owned("6/31/16", &x.0)), "{t:?}");
        // The plan's pack work so ordered, its targets all there.
        let each = c.by_region();
        let p = super::plan(
            &c,
            "d",
            &m,
            &done,
            &BTreeMap::new(),
            Some(&reach),
            &tiles_for(&m),
            Publish {
                each: &each,
                on_map: &BTreeMap::new(),
                served: &served,
                since_catalog: None,
                held: false,
                way: Default::default(),
            },
        );
        assert_eq!((p.work[0].step.as_str(), &p.work[0].targets), ("pack", &t));
        assert_eq!((p.publishing.tiles, p.publishing.units), (t.len(), 1));
    }

    #[test]
    fn a_tile_whose_owners_changed_is_drawn_at_once_though_a_unit_near_it_is_still_to_build() {
        let (c, mut reach, mut m, mut done) = three();
        // Reykjavik's ferry reaches 6/29/17, where b's eastern unit (6/30/16) is still to build: its
        // tile drawn now, and again once that unit is built.
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -16.5, 64.16)), long: vec![] });
        rbuild(&c, &reach, &mut m, &mut done, "6/28/16", "6666666666666666");
        let p = pplan(&c, &reach, &m, &done, &BTreeMap::new(), None, Default::default(), false);
        assert_eq!(p.work[0].step, "pack");
        let drawn: Vec<&str> = p.work[0].targets.iter().map(|t| t.0.as_str()).collect();
        assert!(drawn.contains(&"6/28/17") && drawn.contains(&"6/29/17"), "{drawn:?}");
    }

    #[test]
    fn with_catalogs_held_the_catalog_weighs_against_the_last_held() {
        let (c, reach, mut m, mut done) = three();
        for u in ["6/28/16", "6/29/16", "6/30/16", "6/31/16"] {
            rbuild(&c, &reach, &mut m, &mut done, u, "6666666666666666");
        }
        let (_, p) = through_the_chain(&c, &reach, &mut m, &mut done, Default::default(), true);
        done.record("catalog-held", &p.work[0].targets);
        // Its held catalog made, nothing new: no catalog again.
        let on: BTreeMap<String, bool> =
            [("a".to_string(), true), ("b".to_string(), true), ("c".to_string(), true)].into();
        for since in [5, 25, 4000] {
            let p = pplan(&c, &reach, &m, &done, &on, Some(since), Default::default(), true);
            assert!(!p.work.iter().any(|w| w.step == "catalog"), "{:?}", steps(&p));
        }
        // (Served, not held: the served one is weighed, and there's none.)
        let p = pplan(&c, &reach, &m, &done, &on, Some(4000), Default::default(), false);
        assert!(p.work.iter().any(|w| w.step == "catalog"), "{:?}", steps(&p));
    }

    #[test]
    fn with_the_low_zooms_later_the_map_tiles_go_out_and_the_low_zooms_wait_their_turn() {
        let (c, mut reach, mut m, mut done) = three();
        // Reykjavik's roads reach nine z3 tiles (3/1/1 to 3/3/3), Vík's 3/3/2: c built
        // again stales less than half the lo packs.
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-95.0, 30.0, -21.7, 75.0)), long: vec![] });
        for u in ["6/28/16", "6/31/16"] {
            rbuild(&c, &reach, &mut m, &mut done, u, "6666666666666666");
        }
        let (_, lo) = pack_lo_targets(&m, Some(&reach));
        assert!(lo.len() >= 9, "{lo:?}");
        let later = Publishing { low_later: true, since_low: Some(60), ..Default::default() };
        // The first: every lo pack stale, so the low zooms are made.
        let (ran, p) = through_the_chain(&c, &reach, &mut m, &mut done, later, false);
        assert!(p.low && ran.contains(&"lo".to_string()), "{ran:?}");
        done.record("catalog", &p.work[0].targets);
        // c built again: its map tiles, the road index and rail stops; the low zooms wait.
        rbuild(&c, &reach, &mut m, &mut done, "6/31/16", "8888888888888888");
        assert!(!low_due(&m, &done, Some(&reach)));
        let (mut m2, mut done2) = (m.clone(), done.clone());
        let (ran, p) = through_the_chain(&c, &reach, &mut m2, &mut done2, later, false);
        assert!(!p.low && !p.publishing.low, "{:?}", p.publishing);
        assert!(
            ran.contains(&"pack".to_string())
                && !ran.iter().any(|x| ["lo", "terrain-root", "slope-root"].contains(&x.as_str())),
            "{ran:?}"
        );
        // Their turn (LOW_EVERY_S since), or the switch off: the lo packs too.
        for way in [Publishing { since_low: Some(LOW_EVERY_S), ..later }, Publishing::default()] {
            let (mut m2, mut done2) = (m.clone(), done.clone());
            let (ran, p) = through_the_chain(&c, &reach, &mut m2, &mut done2, way, false);
            assert!(p.low && ran.contains(&"lo".to_string()), "{ran:?}");
        }
        // Everything built: made whatever.
        for u in ["6/29/16", "6/30/16"] {
            rbuild(&c, &reach, &mut m, &mut done, u, "6666666666666666");
        }
        assert!(pplan(&c, &reach, &m, &done, &BTreeMap::new(), Some(600), later, false).low);
    }

    #[test]
    fn the_low_zooms_are_due_once_half_the_lo_packs_are_stale() {
        let (c, reach, mut m, mut done) = three();
        rbuild(&c, &reach, &mut m, &mut done, "6/28/16", "6666666666666666");
        let (_, lo) = pack_lo_targets(&m, Some(&reach));
        assert!(!lo.is_empty() && low_due(&m, &done, Some(&reach)));
        done.record("lo", &lo);
        assert!(!low_due(&m, &done, Some(&reach)));
        // (Nothing built: none due.)
        assert!(!low_due(&BTreeMap::new(), &Keys::default(), Some(&reach)));
    }

    #[test]
    fn near_a_done_regions_slope_is_the_pieces_near_it_and_it_is_ready_with_them() {
        let (c, reach, mut m, mut done) = three();
        rbuild(&c, &reach, &mut m, &mut done, "6/28/16", "6666666666666666");
        let near = Publishing { near: true, ..Default::default() };
        // (The slope listed before the regions' units: the done regions'.)
        let prep = |p: &Plan, step: &str| {
            p.work
                .iter()
                .take_while(|w| w.step != "unit")
                .filter(|w| w.step == step)
                .flat_map(|w| w.targets.iter().map(|t| t.0.clone()))
                .collect::<BTreeSet<_>>()
        };
        let on = BTreeMap::new();
        let before = prep(&pplan(&c, &reach, &m, &done, &on, Some(600), Default::default(), false), "slope");
        let p = pplan(&c, &reach, &m, &done, &on, Some(600), near, false);
        let now = prep(&p, "slope");
        // (The pieces within 20 km of a: fewer than its z3 tiles' stale pieces, none left out that it reads.)
        let a = &c.by_region().into_iter().find(|(id, _)| id == "a").unwrap().1;
        assert!(!now.is_empty() && now.is_subset(&before) && now.len() < before.len(), "{now:?} of {before:?}");
        assert!(now.iter().all(|t| Unit::parse(t).is_some_and(|u| a.meets_rect(grown_e7(6, u.x, u.y, 20.0)))));
        // Those and its tree cover pieces made: a ready, its far pieces and assemblies still to make.
        for step in ["slope", "trees"] {
            let w = Work {
                step: step.into(),
                targets: p
                    .work
                    .iter()
                    .filter(|w| w.step == step)
                    .flat_map(|w| w.targets.clone())
                    .filter(|t| prep(&p, step).contains(&t.0))
                    .collect(),
            };
            if step == "slope" {
                terrain_did(&mut m, &mut done, &w);
            } else {
                did(&mut m, &mut done, &w);
            }
        }
        let p = pplan(&c, &reach, &m, &done, &on, Some(600), near, false);
        assert_eq!(p.ready, ["a"]);
        assert!(p.work.iter().any(|w| w.step == "slope" && w.targets.iter().any(|t| !now.contains(&t.0))));
    }

    #[test]
    fn a_region_done_has_its_slope_and_tree_cover_made_before_the_units() {
        let (c, reach, mut m, mut done) = three();
        let k = ukeys(&c, "d", &m, Some(&reach)).into_iter().find(|(x, _)| x.slash() == "6/28/16").unwrap().1;
        done.record("unit", &[("6/28/16".to_string(), k)]);
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        let p = pplan(&c, &reach, &m, &done, &BTreeMap::new(), Some(600), Default::default(), false);
        let s: Vec<String> = steps(&p).into_iter().filter(|s| !CHAIN.contains(&s.as_str())).collect();
        assert_eq!(&s[..3], ["slope", "trees", "unit"]);
    }

    #[test]
    fn a_regions_terrain_comes_with_it_and_its_units_once_theirs_is_built() {
        // Two regions in two terrain areas: g (Nuuk: unit 6/22/17, area 3/2/2) and a (Reykjavik:
        // 6/28/16, 3/3/2); one unit each, g's first by place.
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, place: &str| Recipe {
            id: id.into(),
            name: id.to_uppercase(),
            outline: vec![format!("place:{place},20")],
        };
        let c = Coverage::fixture(&[r("a", "-21.9,64.13"), r("g", "-51.7,64.18")], None, d.path()).unwrap();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![] });
        reach.units.insert("6/22/17".into(), Reach { owned: Some(e7box(-51.9, 64.1, -51.5, 64.3)), long: vec![] });
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-16", "6-22-17"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        unit_inputs(&mut m, "d");
        let each = c.by_region();
        let plan = |m: &BTreeMap<String, String>, done: &Keys| {
            super::plan(
                &c,
                "d",
                m,
                done,
                &BTreeMap::new(),
                Some(&reach),
                &tiles_for(m),
                Publish {
                    each: &each,
                    on_map: &BTreeMap::new(),
                    served: &BTreeMap::new(),
                    since_catalog: None,
                    held: false,
                    way: Default::default(),
                },
            )
            .work
        };
        let mut done = Keys::default();
        let w = plan(&m, &done);
        heritage_done(&mut m, &mut done, "d", &w[0]);
        heritage_chain_done(&c, &m, &mut done, "d");
        // Each region's terrain area, g's first; neither's unit until its terrain is built.
        let w = plan(&m, &done);
        let line = |w: &[Work]| {
            w.iter()
                .map(|x| format!("{} {}", x.step, x.targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(",")))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            line(&w),
            [
                "terrain 6/22/16,6/22/17",
                "terrain 6/27/16,6/27/17,6/28/16,6/28/17",
                "trees 6/22/16,6/22/17,6/28/16,6/28/17"
            ]
        );
        // g's terrain pieces made: their area's assembly, ahead of a's terrain (a helper takes a's
        // from the far end); then g's unit.
        terrain_did(&mut m, &mut done, &w[0]);
        let w = plan(&m, &done);
        assert_eq!(
            line(&w),
            ["terrain-lo 3/2/2", "terrain 6/27/16,6/27/17,6/28/16,6/28/17", "trees 6/22/16,6/22/17,6/28/16,6/28/17"]
        );
        terrain_did(&mut m, &mut done, &w[0]);
        let w = plan(&m, &done);
        assert_eq!(
            line(&w),
            [
                "unit 6/22/17",
                "terrain 6/27/16,6/27/17,6/28/16,6/28/17",
                "slope 6/22/16,6/22/17",
                "trees 6/22/16,6/22/17,6/28/16,6/28/17"
            ]
        );
    }

    #[test]
    fn units_whose_piece_meets_the_coverage() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        // Terrain, slope and tree cover's pieces and assemblies done, tree cover's assembly last.
        let mut w = plan(&c, "d", &m, &done, &BTreeMap::new());
        for _ in 0..8 {
            if w.iter().all(|x| x.step == "trees-lo") {
                break;
            }
            for x in w.iter().filter(|x| x.step != "trees-lo") {
                if x.step.starts_with("terrain") || x.step.starts_with("slope") {
                    terrain_did(&mut m, &mut done, x);
                } else {
                    did(&mut m, &mut done, x);
                }
            }
            w = plan(&c, "d", &m, &done, &BTreeMap::new());
        }
        assert_eq!(w.iter().map(|x| x.step.as_str()).collect::<Vec<_>>(), ["trees-lo"]);
        did(&mut m, &mut done, &w[0]);
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        m.insert("sources/osm/d/pieces/6-40-20".into(), "sources/osm/d/pieces/6-40-20.5555555555555555.osm.pbf".into());
        // No heritage inputs yet: the units wait.
        assert!(plan(&c, "d", &m, &done, &BTreeMap::new()).is_empty());
        unit_inputs(&mut m, "d");
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        heritage_done(&mut m, &mut done, "d", &w[0]);
        // Without the release's buildings (the worldwide job), the units wait for them.
        let index = m.remove(&crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE)).unwrap();
        assert!(!plan(&c, "d", &m, &done, &BTreeMap::new()).iter().any(|w| w.step == "unit"));
        m.insert(crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE), index);
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "unit");
        assert_eq!(w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["6/28/16"]);
        // Buildings made again: every unit again.
        let k = unit_keys(&c, "d", &m)[0].1.clone();
        let mut m2 = m.clone();
        m2.insert(
            crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE),
            format!("{}.6868686868686868.json", crate::buildtiles::index_logical(crate::buildtiles::TEST_RELEASE)),
        );
        assert_ne!(unit_keys(&c, "d", &m2)[0].1, k);
        // A unit's key names the heritage slices near it, and only those.
        let key = |m: &BTreeMap<String, String>| unit_keys(&c, "d", m)[0].1.clone();
        let k0 = key(&m);
        m.insert(crate::heritage::pos_logical("d", 40, 20), "work/heritage/d/pos/6-40-20.7878787878787878.json".into());
        assert_eq!(key(&m), k0, "a slice far away");
        m.insert(
            crate::heritage::areas_logical("d", 28, 16),
            "work/heritage/d/areas/6-28-16.9090909090909090.jsonl".into(),
        );
        assert_ne!(key(&m), k0, "its own tile's areas");
    }

    /// A declared elevation dataset for the keys: its id, `keyed` digest and cover.
    pub(crate) fn dataset(
        id: &str,
        keyed: &str,
        cover: Option<Vec<[f64; 4]>>,
    ) -> (crate::dem::declared::Accepted, crate::dem::declared::Cover) {
        let declaration = crate::dem::declared::Declaration {
            surface: "ground".into(),
            heights: "EGM2008".into(),
            rank: None,
            code: 1,
            terrain: false,
            data: crate::dem::declared::Data::Tiles {
                url: "https://e.org/{z}/{x}/{y}.png".into(),
                zoom: 10,
                encoding: "terrarium".into(),
            },
        };
        let measured = crate::dem::declared::Measured { crs: None, resolution: 30.0, cover: cover.clone() };
        (
            crate::dem::declared::Accepted {
                id: id.into(),
                path: format!("{id}.toml"),
                declaration,
                measured,
                keyed: keyed.into(),
            },
            crate::dem::declared::Cover::of(cover.as_deref()),
        )
    }

    /// A unit's and a tree cover piece's keys are as they were while the land-cover, canopy and
    /// leaf-type declarations are as staged, and new once one isn't (the units then built again with
    /// it, never mixed); none while the declared sources can't be read (they wait).
    #[test]
    fn a_changed_land_cover_canopy_or_leaf_declaration_is_a_new_key() {
        use crate::agent::tiles::KeyedSources;
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        elevation_inputs(&mut m);
        m.insert(
            crate::inputs::logical(crate::inputs::sources::UNIT),
            "sources/inputs/sources/@index.1212121212121212.json".into(),
        );
        let key = |k: KeyedSources| {
            let mut t = tiles_for(&m);
            t.hold_keyed(&m[&crate::inputs::logical(crate::inputs::sources::UNIT)], k);
            let tt = crate::treepacks::targets_with(&c, &m, t.keyed(&m).and_then(|k| k.trees.as_deref()));
            (super::unit_keys(&c, "d", &m, Some(&reach()), &t)[0].1.clone(), tt.pieces[0].1.clone())
        };
        let staged = KeyedSources { release: crate::buildtiles::TEST_RELEASE.into(), unit: None, trees: None };
        let (u0, t0) = key(staged.clone());
        assert_eq!(
            u0,
            super::unit_keys(&c, "d", &m, Some(&reach()), &tiles_for(&m))[0].1,
            "as staged: the key as it was"
        );
        assert_eq!(t0, crate::treepacks::targets(&c, &m).pieces[0].1);
        let (u1, t1) = key(KeyedSources {
            unit: Some("source esa-worldcover k".into()),
            trees: Some("source meta-canopy k".into()),
            ..staged
        });
        assert!(u1.is_some() && u1 != u0 && t1 != t0);
    }

    #[test]
    fn a_units_key_names_the_declared_elevation_datasets_meeting_it() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        let key = |m: &BTreeMap<String, String>,
                   e: Option<Vec<(crate::dem::declared::Accepted, crate::dem::declared::Cover)>>| {
            // (The terrain's tiles alone, and the datasets `e` when given.)
            let mut bare = m.clone();
            bare.remove(&crate::inputs::logical(crate::inputs::elevation::UNIT));
            let mut t = tiles_for(&bare);
            if let Some(e) = e {
                t.hold_elevation(&m[&crate::inputs::logical(crate::inputs::elevation::UNIT)], e);
            }
            super::unit_keys(&c, "d", m, Some(&reach()), &t)[0].1.clone()
        };
        // No key before the switch (the units wait for it), nor with the accepted version named alone.
        assert_eq!(key(&m, None), None);
        m.insert(
            crate::inputs::logical(crate::inputs::elevation::UNIT),
            "sources/inputs/elevation/@index.1111111111111111.json".into(),
        );
        assert_eq!(key(&m, None), None);
        m.insert(
            crate::inputs::switched_logical(crate::inputs::elevation::UNIT),
            "sources/inputs/elevation/@switched.2222222222222222.json".into(),
        );
        // Switched, the datasets unread: the key waits.
        assert_eq!(key(&m, None), None);
        let world = || dataset("world", "w1", None);
        let far = |k: &str| dataset("far", k, Some(vec![[100.0, 10.0, 101.0, 11.0]]));
        let near = |k: &str| dataset("near", k, Some(vec![[-23.0, 63.0, -20.0, 66.0]]));
        let k1 = key(&m, Some(vec![far("f1"), world()])).unwrap();
        assert_eq!(key(&m, Some(vec![far("f2"), world()])).unwrap(), k1, "a dataset far away");
        let k2 = key(&m, Some(vec![near("n1"), world()])).unwrap();
        assert_ne!(k2, k1, "a dataset meeting it");
        assert_ne!(key(&m, Some(vec![near("n2"), world()])).unwrap(), k2, "its declaration edited");
    }

    #[test]
    fn a_units_key_stays_when_units_write_grid_packs() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        let key = |m: &BTreeMap<String, String>| unit_keys(&c, "d", m)[0].1.clone();
        let k0 = key(&m);
        // The unit (or a neighbour) writes the grid packs its tile lacked: no rebuild.
        for layer in ["grid-class", "grid-canopy", "grid-cover"] {
            m.insert(format!("layers/{layer}/hi/6-28-16"), format!("layers/{layer}/hi/6-28-16.1212121212121212.pack"));
            m.insert(format!("layers/{layer}/hi/6-29-16"), format!("layers/{layer}/hi/6-29-16.1313131313131313.pack"));
        }
        assert_eq!(key(&m), k0);
        // The terrain changing near it does rebuild it.
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.1414141414141414.pack".into());
        assert_ne!(key(&m), k0);
    }

    /// A whole pyramid of tiles under (z, x, y), zooms z0 to z1, each tile's XXH3 from its z/x/y.
    fn pyramid(z: u8, x: u32, y: u32, z0: u8, z1: u8) -> BTreeMap<(u8, u32, u32), u64> {
        let mut out = BTreeMap::new();
        for zz in z0..=z1 {
            let d = zz - z;
            for tx in x << d..(x + 1) << d {
                for ty in y << d..(y + 1) << d {
                    out.insert((zz, tx, ty), store::naming::xxh3(format!("{zz}/{tx}/{ty}").as_bytes()));
                }
            }
        }
        out
    }

    #[test]
    fn a_units_terrain_names_the_tiles_its_ways_read() {
        // 6/28/16 (west Iceland; its tile + 30 km reaches 16.2° W): its owned box at (-20, 65), and a
        // road it owns from there east along 65° N to 10° W.
        let u = Unit { z: 6, x: 28, y: 16 };
        let r = Reach {
            owned: Some(e7box(-20.1, 64.95, -19.9, 65.05)),
            long: vec![LongWay {
                owned: true,
                ferry: false,
                verts: vec![[-190_000_000, 650_000_000], [-100_000_000, 650_000_000]],
            }],
        };
        let at = |z: u8, lon: f64, lat: f64| {
            let t = Unit::of_point(z, [(lon * 1e7) as i32, (lat * 1e7) as i32]);
            (z, t.x, t.y)
        };
        // Its area's lo pack, its tile's hi pack, and a hi pack past its tile + 30 km (6/30/16).
        let names = [
            ("layers/terrain/lo/3-3-2", "1111111111111111"),
            ("layers/terrain/hi/6-28-16", "2222222222222222"),
            ("layers/terrain/hi/6-30-16", "3333333333333333"),
        ];
        let m: BTreeMap<String, String> = names.iter().map(|(l, h)| (l.to_string(), format!("{l}.{h}.pack"))).collect();
        let packs = [pyramid(3, 3, 2, 3, 8), pyramid(6, 28, 16, 9, 12), pyramid(6, 30, 16, 9, 12)];
        let digest = |packs: &[BTreeMap<(u8, u32, u32), u64>]| {
            let mut t = TerrainTiles::new(None);
            for ((l, _), p) in names.iter().zip(packs) {
                t.hold(&m[*l], p.iter().map(|(&(z, x, y), &h)| (z, x, y, h)));
            }
            super::unit_terrain(u, &r, &m, &t).unwrap()
        };
        let d0 = digest(&packs);
        // Tile `t` of pack `k` changed (Some: its new hash) or gone (None).
        let with = |k: usize, t: (u8, u32, u32), h: Option<u64>| {
            let mut p = packs.clone();
            match h {
                Some(h) => *p[k].get_mut(&t).unwrap() = h,
                None => assert!(p[k].remove(&t).is_some()),
            }
            digest(&p)
        };
        // Under its owned box: a z12 tile changed, or gone (its points then read the z11 above; and
        // one appearing, the other way round).
        assert_ne!(with(1, at(12, -20.0, 65.0), Some(7)), d0);
        assert_ne!(with(1, at(12, -20.0, 65.0), None), d0);
        // Any z11 tile in its tile + 30 km (its grid), though no way of its is there; not a z12 tile
        // there, nor a z11 tile past it.
        assert_ne!(with(1, at(11, -17.5, 66.4), Some(7)), d0);
        assert_eq!(with(1, at(12, -17.5, 66.4), Some(7)), d0);
        assert_eq!(with(2, at(11, -8.0, 66.0), Some(7)), d0);
        // Under its long road past its tile + 30 km, where no hi pack is: the z8 tile its points read
        // there; not one beside the road. Further on, the z6 tile, then the z4 tile (the finest
        // staged there); not the z5 tile under the far end, which isn't staged.
        assert_ne!(with(0, at(8, -15.5, 65.0), Some(7)), d0);
        assert_eq!(with(0, at(8, -15.5, 66.0), Some(7)), d0);
        assert_ne!(with(0, at(6, -12.0, 65.0), Some(7)), d0);
        assert_ne!(with(0, at(4, -10.5, 65.0), Some(7)), d0);
        assert_eq!(with(0, at(5, -10.5, 65.0), Some(7)), d0);
        // Without the long road, only its grid and the z12 tiles under its owned box (all in its
        // tile's hi pack).
        let near = Reach { long: vec![], ..r.clone() };
        let mut t = TerrainTiles::new(None);
        for ((l, _), p) in names.iter().zip(&packs) {
            t.hold(&m[*l], p.iter().map(|(&(z, x, y), &h)| (z, x, y, h)));
        }
        let tiles = super::unit_terrain_tiles(u, &near, &m, &t).unwrap();
        assert!(
            tiles.iter().all(|t| t.0 == 11 || t.0 == 12),
            "{:?}",
            tiles.iter().filter(|t| t.0 != 11 && t.0 != 12).collect::<Vec<_>>()
        );
        assert!(tiles.contains(&(12, at(12, -20.0, 65.0).1, at(12, -20.0, 65.0).2, packs[1][&at(12, -20.0, 65.0)])));
        // Kept with what it was made from: the same packs, the same; another pack (a new content
        // name) or another reach, worked out again.
        let k = super::unit_terrain(u, &r, &m, &t).unwrap();
        assert_eq!(k, d0);
        assert_ne!(super::unit_terrain(u, &near, &m, &t).unwrap(), k);
        let mut m2 = m.clone();
        m2.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.4444444444444444.pack".into());
        t.hold(
            "layers/terrain/hi/6-28-16.4444444444444444.pack",
            packs[1].iter().filter(|(t, _)| t.0 != 12).map(|(&(z, x, y), &h)| (z, x, y, h)),
        );
        assert_ne!(super::unit_terrain(u, &r, &m2, &t).unwrap(), k);
        assert_eq!(super::unit_terrain(u, &r, &m, &t).unwrap(), k);
        // A pack the manifest names whose index isn't held: unknown, never worked out without it.
        m2.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.5555555555555555.pack".into());
        assert!(super::unit_terrain(u, &r, &m2, &t)
            .unwrap_err()
            .0
            .starts_with("layers/terrain/hi/6-28-16.5555555555555555.pack"));
    }

    #[test]
    fn a_unit_whose_terrain_cant_be_read_now_waits() {
        let (c, reach, m, done) = three();
        let each = c.by_region();
        let plan = |t: &TerrainTiles| {
            super::plan(
                &c,
                "d",
                &m,
                &done,
                &BTreeMap::new(),
                Some(&reach),
                t,
                Publish {
                    each: &each,
                    on_map: &BTreeMap::new(),
                    served: &BTreeMap::new(),
                    since_catalog: None,
                    held: false,
                    way: Default::default(),
                },
            )
        };
        // The terrain's lo pack's index not read: no unit is built, nor counted as built, nor pruned.
        let p = plan(&TerrainTiles::new(None));
        assert!(!p.work.iter().any(|w| matches!(w.step.as_str(), "unit" | "prune")), "{:?}", p.work);
        assert_eq!(p.unknown, ["6/28/16", "6/29/16", "6/30/16", "6/31/16"]);
        assert!(p.ready.is_empty());
        // Read: they're built.
        let p = plan(&tiles_for(&m));
        assert!(p.unknown.is_empty());
        assert_eq!(p.work.iter().filter(|w| w.step == "unit").map(|w| w.targets.len()).sum::<usize>(), 4);
        let states = region_states(&c, &each, "d", &m, &done, Some(&reach), &TerrainTiles::new(None));
        assert_eq!(states["a"], RegionState { built: 0, total: 1 });
    }

    #[test]
    fn units_come_in_map_order() {
        let u = |x, y| Unit { z: 6, x, y };
        let mut v = vec![u(10, 20), u(2, 20), u(1, 20), u(11, 20), u(10, 21)];
        v.sort_by_key(|&x| spatial_order(x));
        // West to east by 10° square (the manifest's order would put 10 and 11 between 1 and 2), and
        // 10/20 and 10/21, one square, together.
        assert_eq!(v, vec![u(1, 20), u(2, 20), u(10, 20), u(10, 21), u(11, 20)]);
    }

    #[test]
    fn a_chain_whose_next_work_doesnt_change_stops_there() {
        // (A prune reads the manifest alone: recording it changes nothing.)
        let prune = Work { step: "prune".into(), targets: vec![("tiles".into(), "k".into())] };
        assert_eq!(remaining(&Keys::default(), |_| Some(prune.clone())).len(), 1);
    }

    #[test]
    fn a_steps_runs_are_named_together_with_their_areas() {
        let w = |step: &str, n: usize| Work {
            step: step.into(),
            targets: (0..n).map(|i| (format!("6/{i}/1"), "k".into())).collect(),
        };
        assert_eq!(
            next_of(&[w("peaks", 12), w("peaks", 166), w("items", 1), w("heritage", 1)]),
            ["Peaks: 178 tiles", "Wikidata facts", "Heritage details"]
        );
        assert_eq!(next_of(&[w("pois", 1), w("marks", 1)]), ["Landmark candidates: 1 tile", "Landmarks"]);
    }

    #[test]
    fn checklist_counts_to_the_end() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let line = |l: &[Line], id: &str| l.iter().find(|s| s.id == id).cloned().unwrap();
        let item =
            |l: &[Line], step: &str| l.iter().flat_map(|s| s.items.iter()).find(|i| i.step == step).cloned().unwrap();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        unit_inputs(&mut m, "d");
        let l = checklist(&c, "d", &m, &done, &BTreeMap::new(), false);
        assert_eq!(
            l.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            [
                "sites",
                "terrain",
                "slope",
                "trees",
                "depth",
                "roads",
                "tiles",
                "finish",
                "trains",
                "landmarks",
                "buildings",
                "publish"
            ]
        );
        assert_eq!(line(&l, "buildings").total, None, "no sources asked about: not known");
        assert_eq!(line(&l, "trains").left, None, "no rail sources: not known");
        let pieces = coverage_tiles(&c).into_values().flatten().count();
        assert_eq!(
            (line(&l, "terrain").done, line(&l, "terrain").total, line(&l, "terrain").left),
            (0, Some(pieces), Some(pieces + 1)),
            "its pieces counted as the line's, its assembly apart; everything left"
        );
        assert_eq!(
            (item(&l, "terrain").total, item(&l, "terrain-lo").total),
            (Some(pieces), Some(1)),
            "each step its own line"
        );
        assert_eq!(
            line(&l, "terrain").counts(),
            [format!("0 of {pieces} tiles"), "0 of 1 areas".to_string()],
            "a noun at a time"
        );
        assert_eq!(
            (line(&l, "roads").done, line(&l, "roads").total, line(&l, "roads").unit.as_str()),
            (0, Some(1), "tiles")
        );
        assert_eq!(line(&l, "tiles").total, None, "no tiles built: the map tiles aren't known yet");
        assert_eq!(line(&l, "sites").left, Some(1));
        assert_eq!(
            (line(&l, "trees").done, line(&l, "trees").total, line(&l, "trees").left),
            (0, Some(2), Some(3)),
            "two pieces, their assembly apart"
        );
        // Every line names every step of its stage, its items in the table's order.
        assert_eq!(line(&l, "terrain").steps, ["terrain-water", "terrain", "terrain-lo"]);
        assert_eq!(
            line(&l, "terrain").items.iter().map(|i| i.step.as_str()).collect::<Vec<_>>(),
            ["terrain", "terrain-lo"]
        );
        // Terrain, slope, the heritage sites and the area done.
        for step in ["heritage-sites", "terrain", "terrain-lo", "unit", "slope", "slope-lo", "trees", "trees-lo"] {
            let w = past_publishing(&c, "d", &mut m, &mut done);
            assert_eq!(w[0].step, step);
            match step {
                "heritage-sites" => heritage_done(&mut m, &mut done, "d", &w[0]),
                "terrain" | "terrain-lo" | "slope" | "slope-lo" => terrain_did(&mut m, &mut done, &w[0]),
                _ => did(&mut m, &mut done, &w[0]),
            }
        }
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        let l = checklist(&c, "d", &m, &done, &BTreeMap::new(), false);
        for what in ["terrain", "slope", "sites", "roads", "trees"] {
            assert!(line(&l, what).finished(), "{what} done");
        }
        let tiles = line(&l, "tiles");
        assert!(tiles.total.is_some_and(|t| t > 1) && tiles.done == 0 && !tiles.finished());
        assert!(line(&l, "finish").left.is_some_and(|n| n >= 1));
        // Each stage's jobs left, by name, in their order (a worldwide step's run one each).
        let roads = line(&l, "finish");
        assert_eq!(Some(roads.next.len()), roads.left, "{:?}", roads.next);
        assert!(roads.next.iter().all(|n| !n.is_empty() && n != "Catalog"), "{:?}", roads.next);
        assert_eq!(roads.unit, "runs");
        assert!(roads.counts()[0].starts_with(&format!("{} run", roads.left.unwrap())), "{:?}", roads.counts());
        assert_eq!(
            line(&l, "terrain").counts(),
            [format!("{pieces} of {pieces} tiles"), "1 of 1 areas".to_string()],
            "a done stage still says its counts"
        );
        assert_eq!(line(&l, "publish").left, Some(1));
        // Held for review: publishing is the held catalog.
        let k = catalog_key(&m, &BTreeMap::new(), &["r".to_string()]);
        // The descriptions' credits in it once there are any: an edit gets a new catalog.
        let with = |d: &str| catalog_key(&m, &[("credits".to_string(), d.to_string())].into(), &["r".to_string()]);
        assert!(with("a") != k && with("a") != with("b"));
        done.catalog_held = Some(k);
        assert!(line(&checklist(&c, "d", &m, &done, &BTreeMap::new(), true), "publish").finished());
        assert!(!line(&checklist(&c, "d", &m, &done, &BTreeMap::new(), false), "publish").finished());
    }

    #[test]
    fn tree_cover_pieces_their_mids_and_their_assembly() {
        let (c, reach, mut m, mut done) = three();
        let each = c.by_region();
        let plan = |m: &BTreeMap<String, String>, done: &Keys| {
            super::plan(
                &c,
                "d",
                m,
                done,
                &BTreeMap::new(),
                Some(&reach),
                &tiles_for(m),
                Publish {
                    each: &each,
                    on_map: &BTreeMap::new(),
                    served: &BTreeMap::new(),
                    since_catalog: None,
                    held: false,
                    way: Default::default(),
                },
            )
        };
        let ts = |v: &[(String, String)]| v.iter().map(|t| t.0.clone()).collect::<Vec<_>>();
        // Iceland's regions: z3 tile 3/3/2's pieces 6/28/16, 6/28/17 and 6/29/16.
        let tt = crate::treepacks::targets(&c, &m);
        assert_eq!(
            (
                tt.pieces.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
                tt.lo.iter().map(|l| l.0.as_str()).collect::<Vec<_>>()
            ),
            (vec!["6/28/16", "6/28/17", "6/29/16"], vec!["3/3/2"])
        );
        // Each piece first, then the assembly once every piece has its mid.
        let w = tree_work(&tt, &m, &done);
        assert_eq!(
            (ts(&w.pieces), w.lo.len(), w.backfill.len()),
            (vec!["6/28/16".to_string(), "6/28/17".into(), "6/29/16".into()], 0, 0)
        );
        assert_eq!(w.stale_lo.iter().collect::<Vec<_>>(), ["3/3/2"]);
        // Re-keyed without their mids, as agent::rekey records them, the assembly too ("-" for
        // each mid): nothing stale; their mids made last, in idle time, after the chains.
        for (t, k, _) in &tt.pieces {
            done.trees.insert(t.clone(), k.clone());
        }
        done.trees_lo.insert("3/3/2".into(), tt.lo[0].1.clone());
        let w = tree_work(&tt, &m, &done);
        assert!(w.pieces.is_empty() && w.lo.is_empty() && w.stale_pieces.is_empty() && w.stale_lo.is_empty());
        assert_eq!(ts(&w.backfill), ["6/28/16", "6/28/17", "6/29/16"]);
        // One's mid in a hand-off waiting to be merged (the keys planned with have its record on
        // top, Keys::load_with): not made again for it.
        let mut handed = done.clone();
        handed.handed.insert(crate::treepacks::mid_logical(28, 16));
        assert_eq!(ts(&tree_work(&tt, &m, &handed).backfill), ["6/28/17", "6/29/16"]);
        let p = plan(&m, &done);
        assert_eq!(p.work.last(), Some(&Work { step: "trees".into(), targets: w.backfill.clone() }));
        assert_eq!(p.backfill, [Work { step: "trees".into(), targets: w.backfill.clone() }]);
        assert!(!p.work[..p.work.len() - 1].iter().any(|x| x.step == "trees" || x.step == "trees-lo"), "{:?}", p.work);
        // One mid made: the assembly's key names it, so it's stale, and the other pieces' mids are
        // made for it (with the area's work, no longer idle work); then it runs.
        did(&mut m, &mut done, &Work { step: "trees".into(), targets: vec![w.backfill[0].clone()] });
        let tt = crate::treepacks::targets(&c, &m);
        let w = tree_work(&tt, &m, &done);
        assert_eq!(
            (ts(&w.pieces), w.lo.len(), w.backfill.len()),
            (vec!["6/28/17".to_string(), "6/29/16".into()], 0, 0),
            "not assembled until each piece has its mid"
        );
        assert!(w.stale_pieces.is_empty() && w.stale_lo.contains("3/3/2"));
        // Their mids handed off, not yet merged: neither made again, and the assembly waits for them
        // to be merged.
        let mut handed = done.clone();
        handed.handed.extend([crate::treepacks::mid_logical(28, 17), crate::treepacks::mid_logical(29, 16)]);
        let wh = tree_work(&tt, &m, &handed);
        assert!(
            wh.pieces.is_empty() && wh.backfill.is_empty() && wh.lo.is_empty() && wh.stale_lo.contains("3/3/2"),
            "{wh:?}"
        );
        did(&mut m, &mut done, &Work { step: "trees".into(), targets: w.pieces.clone() });
        let w = tree_work(&crate::treepacks::targets(&c, &m), &m, &done);
        assert_eq!((w.pieces.len(), ts(&w.lo)), (0, vec!["3/3/2".to_string()]));
        did(&mut m, &mut done, &Work { step: "trees-lo".into(), targets: w.lo.clone() });
        assert_eq!(tree_work(&crate::treepacks::targets(&c, &m), &m, &done), TreeWork::default());
        // A piece made again (its coverage changed) and another without its mid: both, then the
        // assembly.
        done.trees.insert("6/28/16".into(), "0000000000000000".into());
        m.remove(&crate::treepacks::mid_logical(29, 16));
        let tt = crate::treepacks::targets(&c, &m);
        let w = tree_work(&tt, &m, &done);
        assert_eq!(
            (ts(&w.pieces), w.lo.len(), w.backfill.len()),
            (vec!["6/28/16".to_string(), "6/29/16".into()], 0, 0)
        );
        assert_eq!(w.stale_pieces.iter().collect::<Vec<_>>(), ["6/28/16"]);
        // (Its regions aren't ready meanwhile: a's piece is stale, and b's area's assembly will be.)
        let p = plan(&m, &done);
        assert!(
            p.ready.is_empty() && p.regions.iter().all(|r| r.trees.contains(&"6/28/16".to_string())),
            "{:?}",
            p.regions
        );
        // The coverage gone from 3/3/2 altogether: each piece drops its packs and mid ("none"),
        // and the assembly its lo packs.
        let far = Coverage::fixture(
            &[crate::agent::recipes::Recipe {
                id: "f".into(),
                name: "F".into(),
                outline: vec!["place:20,40,10".into()],
            }],
            None,
            std::path::Path::new("/nonexistent"),
        )
        .unwrap();
        m.insert("layers/trees-cover/lo/3-3-2".into(), "layers/trees-cover/lo/3-3-2.1111111111111111.pack".into());
        let tt = crate::treepacks::targets(&far, &m);
        let w = tree_work(&tt, &m, &done);
        let none: Vec<&(String, String, bool)> = tt.pieces.iter().filter(|p| p.2).collect();
        assert_eq!(none.len(), 2, "6/28/16 and 6/28/17 have mids; 6/29/16 none");
        assert!(none.iter().all(|p| w.pieces.iter().any(|x| x.0 == p.0)));
        assert!(w.lo.iter().any(|(q, _)| q == "3/3/2"), "a none assembly runs whenever");
    }

    #[test]
    fn keys_dont_name_regions() {
        let d = tempfile::tempdir().unwrap();
        let place = "place:-21.9,64.13,20";
        let cov_of = |rs: &[(&str, &str)]| {
            Coverage::fixture(
                &rs.iter()
                    .map(|(id, o)| Recipe { id: id.to_string(), name: "R".into(), outline: vec![o.to_string()] })
                    .collect::<Vec<_>>(),
                None,
                d.path(),
            )
            .unwrap()
        };
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        let keys = |c: &Coverage| {
            (
                terrain_slope_targets(c, &m, &tiles_for(&m)),
                unit_keys(c, "d", &m).into_iter().map(|(u, k)| (u.slash(), k)).collect::<Vec<_>>(),
                pois_keys(c, "d", &m).into_iter().map(|(_, k)| k).collect::<Vec<_>>(),
            )
        };
        let one = keys(&cov_of(&[("r", place)]));
        // Renamed, or the same outline in two regions: nothing to rerun.
        assert_eq!(keys(&cov_of(&[("renamed", place)])), one);
        assert_eq!(keys(&cov_of(&[("a", place), ("b", place)])), one);
        // Another outline is another coverage.
        assert_ne!(keys(&cov_of(&[("r", "place:-21.9,64.13,25")])), one);
    }

    #[test]
    fn units_by_their_roads_reach() {
        let d = tempfile::tempdir().unwrap();
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-16", "6-29-16", "6-30-16"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        let mut r = reach();
        // 6/29/16's tile is far from the outline, but a road it owns runs west into it; 6/30/16 owns
        // roads only in its own tile, and a long one that stays east.
        let long = |verts: &[(f64, f64)]| LongWay {
            owned: true,
            ferry: false,
            verts: verts.iter().map(|&(x, y)| [(x * 1e7) as i32, (y * 1e7) as i32]).collect(),
        };
        r.units.insert(
            "6/29/16".into(),
            Reach { owned: Some(e7box(-15.0, 64.1, -14.0, 64.2)), long: vec![long(&[(-15.0, 64.15), (-21.9, 64.13)])] },
        );
        r.units.insert(
            "6/30/16".into(),
            Reach { owned: Some(e7box(-10.0, 63.0, -6.0, 65.0)), long: vec![long(&[(-8.0, 64.0), (-2.0, 60.0)])] },
        );
        let built: Vec<String> = ukeys(&c, "d", &m, Some(&r)).into_iter().map(|(u, _)| u.slash()).collect();
        assert_eq!(built, vec!["6/28/16", "6/29/16"]);
        // No reaches yet: no units.
        assert!(ukeys(&c, "d", &m, None).is_empty());
        // An outline's change outside a unit's whole reach leaves its key; inside it, not.
        let poly = |east: f64| {
            std::fs::write(
                d.path().join("p.poly"),
                format!("p\n1\n -22.2 64.0\n -21.8 64.0\n {east} 64.3\n -22.2 64.3\nEND\nEND\n"),
            )
            .unwrap();
            Coverage::fixture(
                &[Recipe { id: "p".into(), name: "P".into(), outline: vec!["poly:p.poly".into()] }],
                None,
                d.path(),
            )
            .unwrap()
        };
        let key =
            |c: &Coverage| ukeys(c, "d", &m, Some(&r)).into_iter().find(|(u, _)| u.slash() == "6/28/16").unwrap().1;
        let k0 = key(&poly(30.0));
        assert_ne!(key(&poly(31.0)), k0, "the moved edge crosses 6/28/16's tile");
        let far = |east: f64| {
            std::fs::write(d.path().join("q.poly"), format!("q\n1\n -22.2 64.0\n -21.8 64.0\n -21.8 64.1\n -15.0 64.1\n {east} 64.1\n {east} 70.0\n -22.2 70.0\nEND\nEND\n")).unwrap();
            Coverage::fixture(
                &[Recipe { id: "q".into(), name: "Q".into(), outline: vec!["poly:q.poly".into()] }],
                None,
                d.path(),
            )
            .unwrap()
        };
        let k1 = key(&far(10.0));
        assert_eq!(key(&far(12.0)), k1, "only vertices far east of the tile moved");
    }

    #[test]
    fn removing_coverage_prunes_what_it_built() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        unit_inputs(&mut m, "d");
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        m.insert("sources/osm/d/pieces/6-40-20".into(), "sources/osm/d/pieces/6-40-20.5555555555555555.osm.pbf".into());
        for step in ["heritage-sites", "terrain", "terrain-lo", "unit", "slope", "slope-lo", "trees", "trees-lo"] {
            let w = past_publishing(&c, "d", &mut m, &mut done);
            assert_eq!(w[0].step, step);
            match step {
                "heritage-sites" => heritage_done(&mut m, &mut done, "d", &w[0]),
                "terrain" | "terrain-lo" | "slope" | "slope-lo" => terrain_did(&mut m, &mut done, &w[0]),
                _ => did(&mut m, &mut done, &w[0]),
            }
        }
        // Built once for a region since removed: 6/40/20's outputs, its candidates, a map tile far
        // from any unit and the zoomed-out tile over it.
        for l in [
            "base/6-40-20",
            "global/roads/6-40-20",
            "global/roaden/6-40-20",
            "work/pois/6-40-20",
            "hidata/6-40-20",
            "layers/roads/hi/6-40-20",
            "layers/roads/lo/3-5-2",
        ] {
            m.insert(l.into(), format!("{l}.1212121212121212.x"));
        }
        done.unit.insert("6/40/20".into(), "old".into());
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "prune");
        let t: Vec<&str> = w[0].targets.iter().map(|t| t.0.as_str()).collect();
        assert_eq!(t, vec!["pois 6/40/20", "unit 6/40/20"]);
        done.record("prune", &w[0].targets);
        assert!(!done.unit.contains_key("6/40/20"), "a region added back is built again");
        for l in ["base/6-40-20", "global/roads/6-40-20", "global/roaden/6-40-20", "work/pois/6-40-20"] {
            m.remove(l);
        }
        // The roads' chain: the tiles nothing is near any more go first.
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "prune");
        let t: Vec<&str> = w[0].targets.iter().map(|t| t.0.as_str()).collect();
        assert_eq!(t, vec!["lo 3/5/2", "pack 6/40/20"]);
    }

    #[test]
    fn map_tiles_follow_the_base_packs_near_them() {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let put = |m: &mut BTreeMap<String, String>, u: &str, h: &str| {
            m.insert(format!("base/{u}"), format!("base/{u}.{h}.sect"));
            m.insert(format!("global/roads/{u}"), format!("global/roads/{u}.{h}.sect"));
        };
        put(&mut m, "6-28-16", "1111111111111111");
        put(&mut m, "6-50-20", "2222222222222222");
        let lo_of = |m: &BTreeMap<String, String>, q: &str| {
            pack_lo_targets(m, None).1.into_iter().find(|(t, _)| t == q).unwrap().1
        };
        let (near, far) = (lo_of(&m, "3/3/2"), lo_of(&m, "3/6/2"));
        // A unit rebuilt with new content: the zoomed-out tile over it is stale, the far one isn't.
        put(&mut m, "6-28-16", "3333333333333333");
        assert_ne!(lo_of(&m, "3/3/2"), near);
        assert_eq!(lo_of(&m, "3/6/2"), far);
    }

    /// A base pack whose extent (kept: crate::extents) lies past where the reaches put its ways (a
    /// pack built by other reaches): the tiles it spills into are targets, keyed on it too.
    /// The extents file is the lead's (crate::extents::keep, against its manifest): a job with
    /// another manifest reads what it lacks in memory and writes nothing, so the lead's entries all
    /// stay and the map tiles' keys don't move while the job runs.
    #[test]
    fn a_jobs_extents_leave_the_leads_file_and_the_keys_alone() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |m: &mut BTreeMap<String, String>, u: &str, ext: [i32; 4]| {
            let tmp = root.join("tmp.sect");
            let mut w =
                store::sect::SectWriter::create(&tmp, serde_json::json!({"unit": u.replace('-', "/"), "extent": ext}))
                    .unwrap();
            w.add("ways", &[]).unwrap();
            w.finish().unwrap();
            let name =
                store::naming::content_name(&format!("base/{u}"), &store::naming::hash16_file(&tmp).unwrap(), "sect");
            std::fs::create_dir_all(root.join(&name).parent().unwrap()).unwrap();
            std::fs::rename(&tmp, root.join(&name)).unwrap();
            m.insert(format!("base/{u}"), name);
            m.insert(format!("global/roads/{u}"), format!("global/roads/{u}.1111111111111111.sect"));
        };
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut lead = BTreeMap::new();
        put(&mut lead, "6-28-16", crate::hipack::tile_bounds(6, 28, 16));
        // (Spilling far east: the extent matters to the keys.)
        put(&mut lead, "6-30-16", [crate::hipack::tile_bounds(6, 30, 16)[0], 600_000_000, 50_000_000, 641_000_000]);
        let mut job = lead.clone();
        job.remove("base/6-28-16");
        put(&mut job, "6-31-16", crate::hipack::tile_bounds(6, 31, 16));
        assert_eq!(crate::extents::keep(root, &lead).unwrap(), 2);
        let mut r = reach();
        r.extents = crate::extents::load(root);
        let before = pack_lo_targets(&lead, Some(&r));
        let file = std::fs::read(root.join(crate::extents::FILE)).unwrap();
        // The job's run: its own view, nothing written.
        let (mine, read) = crate::extents::fill(root, &job, crate::extents::load(root)).unwrap();
        assert_eq!((read, mine.len()), (1, 2));
        assert_eq!(std::fs::read(root.join(crate::extents::FILE)).unwrap(), file, "the lead's file as it was");
        r.extents = crate::extents::load(root);
        assert_eq!(r.extents.len(), 2, "no entry of the lead's lost");
        assert_eq!(pack_lo_targets(&lead, Some(&r)), before, "the keys as they were");
    }

    #[test]
    fn a_pack_spilling_past_its_reach_keys_the_tiles_it_reaches() {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("base/6-28-16".into(), "base/6-28-16.1111111111111111.sect".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.1111111111111111.sect".into());
        let mut r = reach();
        r.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![] });
        let far = crate::legacy::Unit::of_point(6, [50_000_000, 600_000_000]).slash();
        assert!(!pack_lo_targets(&m, Some(&r)).0.iter().any(|(t, _)| *t == far));
        r.extents
            .insert("base/6-28-16.1111111111111111.sect".into(), [-219_000_000, 600_000_000, 50_000_000, 641_000_000]);
        let (packs, _) = pack_lo_targets(&m, Some(&r));
        assert!(packs.iter().any(|(t, _)| *t == far), "the tile it spills into is drawn");
        // A pack's extent kept for another content (an older build): not this one's.
        r.extents.clear();
        r.extents
            .insert("base/6-28-16.2222222222222222.sect".into(), [-219_000_000, 600_000_000, 50_000_000, 641_000_000]);
        assert!(!pack_lo_targets(&m, Some(&r)).0.iter().any(|(t, _)| *t == far));
    }

    #[test]
    fn a_long_way_reaches_its_map_tiles() {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("base/6-28-16".into(), "base/6-28-16.1111111111111111.sect".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.1111111111111111.sect".into());
        let mut r = reach();
        // 6/28/16 owns a ferry to Norway (5° E, 60° N), far past 110 km.
        r.units.insert(
            "6/28/16".into(),
            Reach {
                owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)),
                long: vec![LongWay {
                    owned: true,
                    ferry: true,
                    verts: vec![[-219_000_000, 641_000_000], [50_000_000, 600_000_000]],
                }],
            },
        );
        let (packs, _) = pack_lo_targets(&m, Some(&r));
        let norway = crate::legacy::Unit::of_point(6, [50_000_000, 600_000_000]).slash();
        assert!(packs.iter().any(|(t, _)| *t == norway), "the ferry's far end is drawn");
        // Without the ferry, that tile isn't a target.
        r.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![] });
        assert!(!pack_lo_targets(&m, Some(&r)).0.iter().any(|(t, _)| *t == norway));
    }

    #[test]
    fn ferries_dont_depend_on_the_built_units() {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for set in ["rail", "ferries"] {
            m.insert(crate::osmpass::set_name("d", set), format!("sources/osm/d/sets/{set}.5555555555555555.osm.pbf"));
        }
        m.insert("base/6-28-16".into(), "base/6-28-16.1111111111111111.sect".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.1111111111111111.sect".into());
        let find = |m: &BTreeMap<String, String>, step: &str| {
            remaining(&Keys::default(), |d| roads_chain("d", m, d, None)).into_iter().find(|w| w.step == step)
        };
        // (The ferries wait for the ferry feeds' list and the static files' accepted version.)
        assert!(find(&m, "ferries").is_none());
        m.insert(crate::rail::FERRY_FEEDS.into(), "sources/rail/ferry-feeds.3333333333333333.json".into());
        assert!(find(&m, "ferries").is_none());
        m.insert(
            crate::inputs::logical(crate::inputs::gtfs::UNIT),
            "sources/inputs/timetables/gtfs/@index.1111111111111111.json".into(),
        );
        let key = |m: &BTreeMap<String, String>, step: &str| find(m, step).unwrap().targets[0].1.clone();
        let (stations, ferries) = (key(&m, "stations"), key(&m, "ferries"));
        m.insert("base/6-40-20".into(), "base/6-40-20.2222222222222222.sect".into());
        m.insert("global/roads/6-40-20".into(), "global/roads/6-40-20.2222222222222222.sect".into());
        assert_ne!(key(&m, "stations"), stations, "stops are clipped to the built units");
        assert_eq!(key(&m, "ferries"), ferries);
        // Another list (a ferry feed fetched again), other static files: the ferries again.
        let mut m2 = m.clone();
        m2.insert(
            crate::inputs::logical(crate::inputs::gtfs::UNIT),
            "sources/inputs/timetables/gtfs/@index.2222222222222222.json".into(),
        );
        assert_ne!(key(&m2, "ferries"), ferries);
        m.insert(crate::rail::FERRY_FEEDS.into(), "sources/rail/ferry-feeds.4444444444444444.json".into());
        assert_ne!(key(&m, "ferries"), ferries);
    }

    #[test]
    fn trains_a_day_follow_their_feeds() {
        let d = tempfile::tempdir().unwrap();
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let none = BTreeMap::new();
        let lta: BTreeMap<String, String> = [
            ("keys".to_string(), "LTA_ACCOUNT_KEY".to_string()),
            ("feed-keys".to_string(), "LTA_ACCOUNT_KEY".to_string()),
        ]
        .into();
        // No accepted declarations: nothing.
        assert!(rail_chain(&c, "d", &m, &done, &lta).is_none());
        m.insert(
            crate::inputs::logical(crate::inputs::feeds::UNIT),
            "sources/inputs/timetables/feeds/@index.1111111111111111.json".into(),
        );
        m.insert(crate::osmpass::set_name("d", "rail"), "sources/osm/d/sets/rail.2222222222222222.osm.pbf".into());
        let w = rail_chain(&c, "d", &m, &done, &lta).unwrap();
        assert_eq!(w.step, "rail-feeds");
        done.record(&w.step, &w.targets);
        // Its list made: then, once the static files have an accepted version, the trains; then
        // nothing, until what decides them changes.
        m.insert(crate::rail::FEEDS.into(), "sources/rail/feeds.3333333333333333.json".into());
        assert!(rail_next(&c, "d", &m, &done, &lta).is_none(), "the trains wait for the static files");
        m.insert(
            crate::inputs::logical(crate::inputs::gtfs::UNIT),
            "sources/inputs/timetables/gtfs/@index.1111111111111111.json".into(),
        );
        let w = rail_chain(&c, "d", &m, &done, &lta).unwrap();
        assert_eq!(w.step, "rail");
        done.record(&w.step, &w.targets);
        assert!(rail_chain(&c, "d", &m, &done, &lta).is_none());
        // What rail-feeds writes besides the list (the checks, the zips) isn't what anything reads:
        // no rerun for its own sake.
        m.insert(crate::rail::CHECKED.into(), "sources/rail/checked.4444444444444444.json".into());
        m.insert(crate::rail::zip_logical("sncf"), "sources/rail/gtfs/sncf.5555555555555555.zip".into());
        assert!(rail_chain(&c, "d", &m, &done, &lta).is_none());
        // Keys no declaration uses: no rerun. A key it uses gone, other declarations, a catalogue's
        // copy, or the coverage grown: the feeds again. The declarations' keys unread now: nothing.
        let more: BTreeMap<String, String> = [
            ("keys".to_string(), "LTA_ACCOUNT_KEY,ODPT_KEY,TDX_CLIENT_ID".to_string()),
            ("feed-keys".to_string(), "LTA_ACCOUNT_KEY".to_string()),
        ]
        .into();
        assert!(rail_chain(&c, "d", &m, &done, &more).is_none());
        assert_eq!(rail_chain(&c, "d", &m, &done, &none).unwrap().step, "rail-feeds");
        assert!(rail_chain(
            &c,
            "d",
            &m,
            &done,
            &[("keys".to_string(), "LTA_ACCOUNT_KEY".to_string()), ("feed-keys".to_string(), "?".to_string())].into()
        )
        .is_none());
        for (l, f) in [
            (
                crate::inputs::logical(crate::inputs::feeds::UNIT),
                "sources/inputs/timetables/feeds/@index.9999999999999999.json",
            ),
            (crate::rail::CATALOGUE.to_string(), "sources/rail/catalogue.9999999999999999.csv"),
        ] {
            let mut m2 = m.clone();
            m2.insert(l, f.into());
            assert_eq!(rail_chain(&c, "d", &m2, &done, &lta).unwrap().step, "rail-feeds");
        }
        let bigger = Coverage::fixture(
            &[Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:-21.9,64.13,25".into()] }],
            None,
            d.path(),
        )
        .unwrap();
        assert_eq!(rail_chain(&bigger, "d", &m, &done, &lta).unwrap().step, "rail-feeds");
        // A new list (a feed fetched), a new pass's rail set, other static files: the trains again.
        for (l, f) in [
            (crate::rail::FEEDS.to_string(), "sources/rail/feeds.6666666666666666.json"),
            (crate::osmpass::set_name("d", "rail"), "sources/osm/d/sets/rail.7777777777777777.osm.pbf"),
            (
                crate::inputs::logical(crate::inputs::gtfs::UNIT),
                "sources/inputs/timetables/gtfs/@index.8888888888888888.json",
            ),
        ] {
            let mut m2 = m.clone();
            m2.insert(l, f.into());
            assert_eq!(rail_chain(&c, "d", &m2, &done, &lta).unwrap().step, "rail");
        }
        // inputs/keys.env unreadable for now: nothing, rather than the feeds without their keys.
        assert!(rail_chain(&c, "d", &m, &Keys::default(), &[("keys".to_string(), "?".to_string())].into()).is_none());
    }

    #[test]
    fn trains_a_day_beside_the_roads() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let steps = |w: &[Work]| w.iter().map(|x| x.step.clone()).collect::<Vec<_>>();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        unit_inputs(&mut m, "d");
        m.insert(
            crate::inputs::logical(crate::inputs::feeds::UNIT),
            "sources/inputs/timetables/feeds/@index.1111111111111111.json".into(),
        );
        m.insert(
            crate::inputs::logical(crate::inputs::gtfs::UNIT),
            "sources/inputs/timetables/gtfs/@index.1111111111111111.json".into(),
        );
        m.insert(crate::osmpass::set_name("d", "rail"), "sources/osm/d/sets/rail.2222222222222222.osm.pbf".into());
        // From the start, listed after the regions' work (the units don't wait for it, nor it for them).
        for step in ["heritage-sites", "terrain", "terrain-lo", "unit", "slope", "slope-lo", "trees", "trees-lo"] {
            let w = past_publishing(&c, "d", &mut m, &mut done);
            assert_eq!(w[0].step, step);
            assert_eq!(w.last().unwrap().step, "rail-feeds", "{:?}", steps(&w));
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
                heritage_chain_done(&c, &m, &mut done, "d");
            } else if step.starts_with("terrain") || step.starts_with("slope") {
                terrain_did(&mut m, &mut done, &w[0]);
            } else {
                did(&mut m, &mut done, &w[0]);
            }
        }
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        // Then beside the roads' chain, after it in the order.
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(steps(&w), vec!["pack", "rail-feeds"]);
        // The checklist: two jobs left, then one.
        let line = |m: &BTreeMap<String, String>, done: &Keys| {
            checklist(&c, "d", m, done, &BTreeMap::new(), false).into_iter().find(|s| s.id == "trains").unwrap()
        };
        assert_eq!(line(&m, &done).left, Some(2));
        done.record("rail-feeds", &w[1].targets);
        m.insert(crate::rail::FEEDS.into(), "sources/rail/feeds.3333333333333333.json".into());
        assert_eq!(line(&m, &done).left, Some(1));
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(steps(&w), vec!["pack", "rail"]);
        done.record("rail", &w[1].targets);
        assert!(line(&m, &done).finished());
    }

    #[test]
    fn trains_a_day_unknown_while_blocked() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let lta: BTreeMap<String, String> = [("keys".to_string(), "LTA_ACCOUNT_KEY".to_string())].into();
        let line = |m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>| {
            checklist(&c, "d", m, done, inputs, false).into_iter().find(|s| s.id == "trains").unwrap()
        };
        assert_eq!(line(&m, &done, &lta).left, None, "no accepted declarations");
        m.insert(
            crate::inputs::logical(crate::inputs::feeds::UNIT),
            "sources/inputs/timetables/feeds/@index.1111111111111111.json".into(),
        );
        let w = rail_chain(&c, "d", &m, &done, &lta).unwrap();
        // inputs/keys.env unreadable: not known, not done.
        let unreadable: BTreeMap<String, String> = [("keys".to_string(), "?".to_string())].into();
        assert_eq!(line(&m, &done, &unreadable).left, None);
        assert!(!line(&m, &done, &unreadable).finished());
        assert_eq!(line(&m, &done, &lta).left, Some(2));
        done.record(&w.step, &w.targets);
        // rail-feeds done, but no feeds' list, or no rail set for the pass: not known, not done.
        assert_eq!(line(&m, &done, &lta).left, None, "no rail set, no list");
        m.insert(crate::rail::FEEDS.into(), "sources/rail/feeds.3333333333333333.json".into());
        assert_eq!(line(&m, &done, &lta).left, None, "no rail set");
        assert!(!line(&m, &done, &lta).finished());
        m.insert(crate::osmpass::set_name("d", "rail"), "sources/osm/d/sets/rail.2222222222222222.osm.pbf".into());
        assert_eq!(line(&m, &done, &lta).left, None, "no static files' accepted version");
        m.insert(
            crate::inputs::logical(crate::inputs::gtfs::UNIT),
            "sources/inputs/timetables/gtfs/@index.1111111111111111.json".into(),
        );
        assert_eq!(line(&m, &done, &lta).left, Some(1));
        let w = rail_chain(&c, "d", &m, &done, &lta).unwrap();
        done.record(&w.step, &w.targets);
        assert_eq!(line(&m, &done, &lta).left, Some(0));
        assert!(line(&m, &done, &lta).finished());
        let mut m2 = m.clone();
        m2.remove(crate::rail::FEEDS);
        assert_eq!(line(&m2, &done, &lta).left, None, "the list gone");
    }

    #[test]
    fn roads_and_landmarks_dont_wait_for_each_other() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let steps = |w: &[Work]| w.iter().map(|x| x.step.clone()).collect::<Vec<_>>();
        // Terrain, slope, the heritage sites and the unit done.
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        unit_inputs(&mut m, "d");
        for step in ["heritage-sites", "terrain", "terrain-lo", "unit", "slope", "slope-lo", "trees", "trees-lo"] {
            let w = past_publishing(&c, "d", &mut m, &mut done);
            assert_eq!(w[0].step, step);
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
                heritage_chain_done(&c, &m, &mut done, "d");
            } else if step.starts_with("terrain") || step.starts_with("slope") {
                terrain_did(&mut m, &mut done, &w[0]);
            } else {
                did(&mut m, &mut done, &w[0]);
            }
        }
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        m.insert("work/trailends/d".into(), "work/trailends/d.8888888888888888.json".into());
        // The roads' and the landmarks' first steps (no rail sources here); no catalog while the
        // roads' chain has work.
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(steps(&w), vec!["pack", "pois"]);
        // The roads' chain to its end (the landmarks' still waiting): then a catalog first.
        for _ in 0..10 {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            if w[0].step == "catalog" {
                break;
            }
            assert_eq!(w.last().unwrap().step, "pois");
            done.record(&w[0].step, &w[0].targets);
        }
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(steps(&w), vec!["catalog", "pois"]);
        done.record("catalog", &w[0].targets);
        // The candidates made: the peaks wait for the pass's summits, the items' facts don't.
        done.record("pois", &w[1].targets);
        m.insert("work/pois/6-28-16".into(), "work/pois/6-28-16.9999999999999999.json".into());
        assert_eq!(steps(&plan(&c, "d", &m, &done, &BTreeMap::new())), vec!["items"]);
        m.insert("work/summits/d".into(), "work/summits/d.aaaaaaaaaaaaaaaa.bin".into());
        assert_eq!(steps(&plan(&c, "d", &m, &done, &BTreeMap::new())), vec!["peaks", "items"]);
    }

    #[test]
    fn the_peaks_key_names_the_terrains_version() {
        // (Its peaks' z12 outside the packs are AWS's raw tiles processed by the terrain's code:
        // a new version of it runs them again, the packs and all else the same.)
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        m.insert("work/trailends/d".into(), "work/trailends/d.8888888888888888.json".into());
        m.insert("work/pois/6-28-16".into(), "work/pois/6-28-16.9999999999999999.json".into());
        m.insert("work/summits/d".into(), "work/summits/d.aaaaaaaaaaaaaaaa.bin".into());
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.bbbbbbbbbbbbbbbb.pack".into());
        let now = peaks_keys(&c, "d", &m);
        assert_eq!(now.len(), 1);
        assert_eq!(now, peaks_keys_at(&c, "d", &m, TERRAIN_V));
        let next = peaks_keys_at(&c, "d", &m, TERRAIN_V + 1);
        assert_eq!(next[0].0, now[0].0);
        assert_ne!(next[0].1, now[0].1);
        // A hi pack within 30 km changing changes it too.
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.cccccccccccccccc.pack".into());
        assert_ne!(peaks_keys(&c, "d", &m)[0].1, now[0].1);
    }

    #[test]
    fn the_heritage_sites_key_names_the_registers_meeting_the_cover() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        unit_inputs(&mut m, "d");
        let reg = |id: &str, keyed: &str, b: [f64; 4]| crate::inputs::heritage::Register {
            id: id.into(),
            geojson: String::new(),
            toml: String::new(),
            keyed: keyed.into(),
            bbox: Some(b),
        };
        let iceland = [-22.5, 63.8, -21.0, 64.5];
        let japan = [129.0, 31.0, 146.0, 45.0];
        let ins = |regs: &[crate::inputs::heritage::Register]| {
            BTreeMap::from([("heritage".to_string(), crate::heritage::registers_line(regs))])
        };
        // (The line read back as written.)
        let line = crate::heritage::registers_line(&[
            reg("is-minjar", "a b", iceland),
            crate::inputs::heritage::Register { bbox: None, ..reg("empty", "c d", japan) },
        ]);
        assert_eq!(
            crate::heritage::parse_registers(&line),
            [
                ("is-minjar".to_string(), "a b".to_string(), Some(iceland)),
                ("empty".to_string(), "c d".to_string(), None)
            ]
        );
        let k = heritage_sites_key(
            &c,
            "d",
            &m,
            &ins(&[reg("is-minjar", "a b", iceland), reg("jp-bunkacho", "x y", japan)]),
        )
        .unwrap();
        // A register on another continent changed, added or removed: the same key.
        assert_eq!(
            heritage_sites_key(
                &c,
                "d",
                &m,
                &ins(&[reg("is-minjar", "a b", iceland), reg("jp-bunkacho", "x z", japan)])
            ),
            Some(k.clone())
        );
        assert_eq!(heritage_sites_key(&c, "d", &m, &ins(&[reg("is-minjar", "a b", iceland)])), Some(k.clone()));
        // One meeting the cover changed: another; the matches' accepted version too.
        assert_ne!(heritage_sites_key(&c, "d", &m, &ins(&[reg("is-minjar", "a c", iceland)])), Some(k.clone()));
        let mut mm = m.clone();
        mm.insert(
            crate::inputs::logical(crate::inputs::heritage_matches::UNIT),
            "sources/inputs/heritage-matches/@index.1111111111111111.json".into(),
        );
        assert_ne!(heritage_sites_key(&c, "d", &mm, &ins(&[reg("is-minjar", "a b", iceland)])), Some(k.clone()));
        // Registers that can't be read now, or none accepted: none (the sites wait).
        assert_eq!(heritage_sites_key(&c, "d", &m, &BTreeMap::from([("heritage".to_string(), "?".to_string())])), None);
        m.remove(&crate::inputs::logical(crate::inputs::heritage::UNIT));
        assert_eq!(heritage_sites_key(&c, "d", &m, &ins(&[reg("is-minjar", "a b", iceland)])), None);
    }

    #[test]
    fn the_heritage_jobs_key_names_no_seeds() {
        // (docs/inputs.md §6.5.3: the seeds' archive, while the NAS still has it, is read by nothing.)
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert(
            crate::heritage::base_logical("d", "heritage-sources"),
            "work/heritage/d/heritage-sources.1111111111111111.json".into(),
        );
        let k = heritage_key(&c, "d", &m).unwrap();
        m.insert(
            "sources/registers/legacy-seeds".into(),
            "sources/registers/legacy-seeds.5555555555555555.tar.zst".into(),
        );
        assert_eq!(heritage_key(&c, "d", &m).unwrap(), k);
    }

    #[test]
    fn the_chains_wait_for_the_passs_jobs_they_read_to_be_made_again() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        unit_inputs(&mut m, "d");
        let mut done = Keys::default();
        let steps = |w: &[Work]| w.iter().map(|x| x.step.clone()).collect::<Vec<_>>();
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        heritage_done(&mut m, &mut done, "d", &w[0]);
        assert!(steps(&plan(&c, "d", &m, &done, &BTreeMap::new())).contains(&"heritage".to_string()));
        // No registers accepted (just after the publish), an older pass's heritage-sources there:
        // nothing heritage-related runs, nor is it forecast; the chain done, not the marks either.
        {
            let mut m0 = m.clone();
            m0.remove(&crate::inputs::logical(crate::inputs::heritage::UNIT));
            let held = ["heritage-sites", "heritage", "marks", "overlays"];
            let w = plan(&c, "d", &m0, &done, &BTreeMap::new());
            assert!(!steps(&w).iter().any(|s| held.contains(&s.as_str())), "{:?}", steps(&w));
            let left = chains_left(&c, "d", &m0, &done, &BTreeMap::new(), None, &TerrainTiles::new(None));
            assert!(!left[2].iter().any(|x| held.contains(&x.step.as_str())), "{:?}", left[2]);
            let mut d0 = done.clone();
            d0.record("heritage", &[("heritage".to_string(), heritage_key(&c, "d", &m0).unwrap())]);
            let w = plan(&c, "d", &m0, &d0, &BTreeMap::new());
            assert!(!steps(&w).iter().any(|s| held.contains(&s.as_str())), "{:?}", steps(&w));
            // The units' wait says why while the pass has no heritage sites: the registers await
            // their first acceptance (with an older heritage-sources there, they don't wait).
            assert_eq!(heritage_awaits_acceptance(&c, "d", &m0, &BTreeMap::new()), None);
            m0.remove(&crate::heritage::base_logical("d", "heritage-sources"));
            assert!(heritage_awaits_acceptance(&c, "d", &m0, &BTreeMap::new())
                .is_some_and(|w| w.contains("scenic inputs accept heritage")));
            assert_eq!(heritage_awaits_acceptance(&c, "d", &m, &BTreeMap::new()), None, "accepted");
        }
        // The registers' accepted version changed: the heritage sites are made again first, the
        // rest of the chain (an hour and a half on them) only after.
        m.insert(
            crate::inputs::logical(crate::inputs::heritage::UNIT),
            "sources/inputs/heritage/@index.ffffffffffffffff.json".into(),
        );
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "heritage-sites");
        assert!(!steps(&w).contains(&"heritage".to_string()), "{:?}", steps(&w));
        heritage_done(&mut m, &mut done, "d", &w[0]);
        // Likewise the candidates and the hiking routes' ends: a new hikes set, the ends made again
        // first.
        m.insert("work/trailends/d".into(), "work/trailends/d.8888888888888888.json".into());
        m.insert(crate::osmpass::set_name("d", "hikes"), "sources/osm/d/sets/hikes.1111111111111111.osm.pbf".into());
        assert!(!steps(&plan(&c, "d", &m, &done, &BTreeMap::new())).contains(&"pois".to_string()));
        let ends = trailends_work("d", &m, &done).unwrap();
        done.record(&ends.step, &ends.targets);
        assert!(steps(&plan(&c, "d", &m, &done, &BTreeMap::new())).contains(&"pois".to_string()));
    }

    #[test]
    fn the_landmarks_go_on_beside_the_regions_each_step_once_what_it_reads_is_built() {
        // Two units in two terrain areas (Nuuk 6/22/17 in 3/2/2, Reykjavik 6/28/16 in 3/3/2).
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, place: &str| Recipe {
            id: id.into(),
            name: id.to_uppercase(),
            outline: vec![format!("place:{place},20")],
        };
        let c = Coverage::fixture(&[r("a", "-21.9,64.13"), r("g", "-51.7,64.18")], None, d.path()).unwrap();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![] });
        reach.units.insert("6/22/17".into(), Reach { owned: Some(e7box(-51.9, 64.1, -51.5, 64.3)), long: vec![] });
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-16", "6-22-17"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        unit_inputs(&mut m, "d");
        m.insert("work/trailends/d".into(), "work/trailends/d.8888888888888888.json".into());
        m.insert("work/summits/d".into(), "work/summits/d.aaaaaaaaaaaaaaaa.bin".into());
        let each = c.by_region();
        let plan = |m: &BTreeMap<String, String>, done: &Keys| {
            super::plan(
                &c,
                "d",
                m,
                done,
                &BTreeMap::new(),
                Some(&reach),
                &tiles_for(m),
                Publish {
                    each: &each,
                    on_map: &BTreeMap::new(),
                    served: &BTreeMap::new(),
                    since_catalog: None,
                    held: false,
                    way: Default::default(),
                },
            )
            .work
        };
        let line = |w: &[Work]| {
            w.iter()
                .map(|x| format!("{} {}", x.step, x.targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(",")))
                .collect::<Vec<_>>()
        };
        let mut done = Keys::default();
        // Before the heritage sites (the units wait for them): the candidates already, after the
        // terrain and tree cover.
        let w = plan(&m, &done);
        assert_eq!(
            line(&w),
            [
                "heritage-sites heritage-sites",
                "terrain 6/22/16,6/22/17,6/27/16,6/27/17,6/28/16,6/28/17",
                "trees 6/22/16,6/22/17,6/28/16,6/28/17",
                "pois 6/22/17,6/28/16"
            ]
        );
        heritage_done(&mut m, &mut done, "d", &w[0]);
        // The heritage sites made: the rest of the heritage chain too, beside the candidates, both
        // after the regions' work.
        let w = plan(&m, &done);
        assert_eq!(
            line(&w),
            [
                "terrain 6/22/16,6/22/17",
                "terrain 6/27/16,6/27/17,6/28/16,6/28/17",
                "trees 6/22/16,6/22/17,6/28/16,6/28/17",
                "pois 6/22/17,6/28/16",
                "heritage heritage"
            ]
        );
        // One unit's candidates made: the items' facts wait for the other's (they read them all).
        done.record("pois", &[w[3].targets[0].clone()]);
        m.insert("work/pois/6-22-17".into(), "work/pois/6-22-17.9999999999999999.json".into());
        let w = plan(&m, &done);
        assert_eq!(line(&w)[3..], ["pois 6/28/16", "heritage heritage"]);
        // All made: the items' facts; the peaks only where the terrain they read is built (Nuuk's,
        // once 3/2/2 is: Reykjavik's terrain isn't yet).
        done.record("pois", &[w[3].targets[0].clone()]);
        m.insert("work/pois/6-28-16".into(), "work/pois/6-28-16.9999999999999999.json".into());
        let w = plan(&m, &done);
        assert_eq!(line(&w)[2..], ["trees 6/22/16,6/22/17,6/28/16,6/28/17", "items items", "heritage heritage"]);
        terrain_did(&mut m, &mut done, &w[0]);
        let w = plan(&m, &done);
        assert_eq!(w[0].step, "terrain-lo");
        terrain_did(&mut m, &mut done, &w[0]);
        let w = plan(&m, &done);
        assert_eq!(line(&w).last().unwrap(), "heritage heritage");
        assert!(line(&w).contains(&"peaks 6/22/17".to_string()), "{:?}", line(&w));
        // The landmark points wait for the peaks, the items' facts and the heritage chain; the
        // overlays for the last unit.
        let rest: Vec<Work> =
            w.iter().filter(|x| matches!(x.step.as_str(), "peaks" | "items" | "heritage")).cloned().collect();
        for x in &rest {
            done.record(&x.step, &x.targets);
        }
        let marks = |m: &BTreeMap<String, String>, done: &Keys| {
            plan(m, done)
                .into_iter()
                .filter(|x| matches!(x.step.as_str(), "marks" | "overlays"))
                .map(|x| x.step)
                .collect::<Vec<_>>()
        };
        assert!(marks(&m, &done).is_empty(), "Reykjavik's peaks are still to come");
        while let Some(t) = plan(&m, &done).into_iter().find(|x| x.step == "terrain" || x.step == "terrain-lo") {
            terrain_did(&mut m, &mut done, &t);
        }
        let peaks = plan(&m, &done).into_iter().find(|x| x.step == "peaks").unwrap();
        done.record("peaks", &peaks.targets);
        assert_eq!(marks(&m, &done), ["marks"]);
    }

    /// The 3D buildings' sources as agent::input_digests has them: every z6 tile within 1 km of
    /// `c` reading something (its digest `d`), and their rows.
    fn bld_inputs(c: &Coverage, d: &str) -> BTreeMap<String, String> {
        let mut inputs = BTreeMap::from([
            ("bld-release".to_string(), crate::buildtiles::TEST_RELEASE.to_string()),
            ("bld-key".to_string(), crate::buildtiles::TEST_RELEASE.to_string()),
        ]);
        for x in 0..64u32 {
            for y in 0..64u32 {
                if c.meets_rect(crate::hipack::grow(crate::hipack::tile_bounds(6, x, y), 1.0)) {
                    inputs.insert(format!("bldprep 6/{x}/{y}"), format!("{d}{x}{y}"));
                    inputs.insert(format!("bldprep-rows 6/{x}/{y}"), "1000".into());
                }
            }
        }
        inputs
    }

    fn by_spatial(t: &str) -> BldRank {
        (0, spatial_order(Unit::parse(t).unwrap()))
    }

    #[test]
    fn the_3d_buildings_targets_and_keys() {
        // Reykjavik's 20 km circle: within 1 km of z6 tiles 6/28/16 and 6/28/17 (its southern
        // edge), meeting 6/28/16 alone.
        let c = cov();
        let m: BTreeMap<String, String> = BTreeMap::new();
        let inputs = bld_inputs(&c, "a");
        let tt = bld_targets(&c, &m, &inputs);
        let names = |v: &[(String, String)]| v.iter().map(|t| t.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&tt.prep), names(&bld_targets(&c, &m, &inputs).prep), "the same each time");
        assert!(names(&tt.prep).contains(&"6/28/16".to_string()));
        assert!(names(&tt.tiles).iter().all(|t| names(&tt.prep).contains(t)), "every tile prepared");
        assert!(tt.prep.len() >= tt.tiles.len());
        // No sources asked about, or none readable now: no bldprep target. (Its tiles still known.)
        assert!(bld_targets(&c, &m, &BTreeMap::new()).prep.is_empty());
        assert_eq!(
            bld_targets(&c, &m, &BTreeMap::from([("bld-release".to_string(), "?".to_string())])).tiles.len(),
            tt.tiles.len()
        );
        // A source file fetched for a tile changes its bldprep key alone.
        let mut more = inputs.clone();
        more.insert("bldprep 6/28/16".into(), "b".into());
        let t2 = bld_targets(&c, &m, &more);
        for ((t, k), (_, k2)) in tt.prep.iter().zip(&t2.prep) {
            assert_eq!(k == k2, t != "6/28/16", "{t}");
        }
        // A tile's normalized file, or a neighbour's, changes its bldtiles key; a file further
        // away doesn't.
        let tile = tt.tiles[0].0.clone();
        let u = Unit::parse(&tile).unwrap();
        for (l, changes) in [
            (crate::bld::work_logical(u.x, u.y), true),
            (crate::bld::work_logical(u.x + 1, u.y + 1), true),
            (crate::bld::work_logical(u.x + 2, u.y), false),
        ] {
            let mut m2 = m.clone();
            m2.insert(l.clone(), format!("{l}.0123456789abcdef.sect"));
            let k2 = bld_targets(&c, &m2, &inputs).tiles.iter().find(|t| t.0 == tile).unwrap().1.clone();
            assert_eq!(k2 != tt.tiles[0].1, changes, "{l}");
        }
        // So does the coverage's country there (the fill's fits go by it).
        let mut c2 = c.clone();
        c2.shapes[0].country = "IS".into();
        assert_ne!(bld_targets(&c2, &m, &inputs).tiles[0].1, tt.tiles[0].1);
        assert_eq!(bld_targets(&c2, &m, &inputs).prep, tt.prep, "not what bldprep reads");
    }

    /// A depth source over the sea off Reykjavik (`cov`): a coarse grid over the z3 tile, and a fine
    /// one over a z6 tile near the coverage.
    fn depth_sources() -> Vec<crate::depth::grids::Source> {
        let g = |bbox: [f64; 4], zoom: u8| crate::depth::grids::Grid {
            bbox,
            zoom,
            file: "g.tif".into(),
            zoom_file: None,
            low_file: None,
        };
        let s = |code: u8, grids| crate::depth::grids::Source {
            id: format!("s{code}"),
            code,
            lake: false,
            band_m: 0.0,
            intertidal: false,
            datum_exact: true,
            label: "S".into(),
            datum: "MSL".into(),
            notice: None,
            grids,
            pin: format!("s{code} d{code}"),
        };
        vec![s(1, vec![g([-45.0, 40.9, 0.0, 66.5], 8)]), s(2, vec![g([-22.4, 63.9, -21.6, 64.3], 13)])]
    }

    /// The depth chain waits for its sources and its water's digests, then makes the assemblies
    /// before the pieces that read them; a source's change re-keys what it meets.
    #[test]
    fn the_depth_chain() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let done = Keys::default();
        let mut tiles = TerrainTiles::new(None);
        // No basemap (no water source): nothing.
        m.insert(crate::inputs::logical(crate::inputs::sources::UNIT), "x".into());
        tiles.hold_depth("x", depth_sources());
        assert!(depth_work(&c, &m, &done, &tiles).is_empty());
        m.insert(
            "layers/basemap/world-2026-09-28".into(),
            "layers/basemap/world-2026-09-28.0101010101010101.pmtiles".into(),
        );
        // The water's digests not worked out yet: the targets wait, and the terrain-water job is
        // asked for them.
        let dt = depth_targets(&c, &m, &tiles).unwrap();
        assert!(dt.lo.is_empty() && dt.hi.is_empty() && dt.unknown > 0);
        let pin = crate::terrain_pack::water_source_pin(&m).unwrap();
        let reads = depth_water_reads(&c, &depth_sources());
        assert!(
            reads.iter().any(|r| r.0 == "3/3/2") && reads.iter().any(|r| r.0.starts_with("depth 6/")),
            "{:?}",
            reads.iter().map(|r| &r.0).collect::<Vec<_>>()
        );
        let mut idx = crate::terrain_water::WaterIdx {
            fmt: 1,
            step: "terrain-water".into(),
            pin: pin.clone(),
            digests: Default::default(),
        };
        for (t, from, _) in &reads {
            idx.digests.insert(t.clone(), (from.clone(), "d".into()));
        }
        let name = format!("{}.0202020202020202.json", crate::terrain_water::idx_logical(&pin));
        m.insert(crate::terrain_water::idx_logical(&pin), name.clone());
        tiles.hold_water(&name, idx);
        let dt = depth_targets(&c, &m, &tiles).unwrap();
        assert_eq!(dt.unknown, 0);
        assert!(dt.lo.iter().any(|t| t.0 == "3/3/2"));
        // Pieces only where the fine source is.
        assert!(
            !dt.hi.is_empty()
                && dt.hi.iter().all(|(t, _)| {
                    let u = Unit::parse(t).unwrap();
                    (u.x, u.y) == (28, 16) || (u.x, u.y) == (28, 17) || (u.x, u.y) == (27, 16) || (u.x, u.y) == (27, 17)
                }),
            "{:?}",
            dt.hi
        );
        // The assemblies first; the pieces once theirs are done.
        let w = depth_work(&c, &m, &done, &tiles);
        assert_eq!(w.iter().map(|x| x.step.as_str()).collect::<Vec<_>>(), ["depth-lo"]);
        let mut d2 = done.clone();
        d2.record("depth-lo", &w[0].targets);
        let w = depth_work(&c, &m, &d2, &tiles);
        assert_eq!(w.iter().map(|x| x.step.as_str()).collect::<Vec<_>>(), ["depth"]);
        d2.record("depth", &w[0].targets);
        assert!(depth_work(&c, &m, &d2, &tiles).is_empty());
        assert!(remaining(&d2, |d| depth_next(&c, &m, d, &tiles)).is_empty());
        // An assembly's lo pack made new changes its pieces' keys; a fine source changed, its
        // pieces' alone.
        let mut m2 = m.clone();
        m2.insert(
            crate::depth::build::lo_logical(crate::depth::LAYER, 3, 2),
            "layers/depth/lo/3-3-2.0303030303030303.pack".into(),
        );
        let after = depth_targets(&c, &m2, &tiles).unwrap();
        assert!(after.hi.iter().zip(&dt.hi).all(|(a, b)| a.1 != b.1) && after.lo == dt.lo);
        let mut moved = depth_sources();
        moved[1].pin = "s2 changed".into();
        tiles.hold_depth("x", moved);
        let after = depth_targets(&c, &m, &tiles).unwrap();
        assert!(after.hi.iter().zip(&dt.hi).all(|(a, b)| a.1 != b.1));
    }

    #[test]
    fn the_3d_buildings_chain() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let inputs = bld_inputs(&c, "a");
        let mut done = Keys::default();
        let steps = |w: &[Work]| w.iter().map(|x| x.step.clone()).collect::<Vec<_>>();
        // Not asked about (no "bld-release"), or the indexes unreadable: nothing.
        assert!(bld_work(&c, &m, &done, &BTreeMap::new(), &by_spatial).is_empty());
        assert!(bld_work(&c, &m, &done, &BTreeMap::from([("bld-release".to_string(), "?".to_string())]), &by_spatial)
            .is_empty());
        // Nothing downloaded yet: the fetch alone.
        assert_eq!(
            steps(&bld_work(
                &c,
                &m,
                &done,
                &BTreeMap::from([
                    ("bld-release".to_string(), String::new()),
                    ("bld-key".to_string(), crate::buildtiles::TEST_RELEASE.to_string())
                ]),
                &by_spatial
            )),
            ["bld-fetch"]
        );
        // The sources here: the fetch, and every tile's normalized file beside it; no tile until
        // it and its neighbours are prepared.
        let w = bld_work(&c, &m, &done, &inputs, &by_spatial);
        assert_eq!(steps(&w), ["bld-fetch", "bldprep"]);
        done.record("bld-fetch", &w[0].targets);
        // One tile prepared, a neighbour not: still no tile.
        let prep = w[1].targets.clone();
        done.record("bldprep", &prep[..1]);
        let w = bld_work(&c, &m, &done, &inputs, &by_spatial);
        if prep.len() > 1 {
            assert_eq!(steps(&w), ["bldprep"]);
        }
        // All prepared (their files in the manifest): the tiles, keyed on those files.
        done.record("bldprep", &prep);
        for (t, k) in &prep {
            let u = Unit::parse(t).unwrap();
            let l = crate::bld::work_logical(u.x, u.y);
            m.insert(l.clone(), format!("{l}.{k}.sect"));
        }
        let w = bld_work(&c, &m, &done, &inputs, &by_spatial);
        assert_eq!(steps(&w), ["bldtiles"]);
        assert_eq!(w[0].targets, bld_targets(&c, &m, &inputs).tiles);
        done.record("bldtiles", &w[0].targets);
        for (t, k) in &w[0].targets {
            let u = Unit::parse(t).unwrap();
            for l in [
                crate::bld::pack_logical(u.x, u.y),
                crate::bld::field::hi_logical(u.x, u.y),
                crate::bld::field::mid_logical(u.x, u.y),
            ] {
                m.insert(l.clone(), format!("{l}.{k}.pack"));
            }
        }
        // Then the field's lo levels of their z3 tiles, keyed on their mids.
        let tt = bld_targets(&c, &m, &inputs);
        assert!(
            !tt.lo.is_empty()
                && tt.lo.iter().all(|(q, _, ts)| ts.iter().all(|t| Unit::parse(t).is_some_and(|u| format!(
                    "3/{}/{}",
                    u.x >> 3,
                    u.y >> 3
                ) == *q)))
        );
        let w = bld_work(&c, &m, &done, &inputs, &by_spatial);
        assert_eq!(steps(&w), ["bldfield-lo"]);
        let lo: Vec<(String, String)> = tt.lo.iter().map(|(q, k, _)| (q.clone(), k.clone())).collect();
        assert_eq!(w[0].targets, lo);
        assert_eq!(remaining(&done, |d| bld_next(&c, &m, d, &inputs)).len(), 1);
        done.record("bldfield-lo", &w[0].targets);
        for (q, k) in &lo {
            let u = Unit::parse(q).unwrap();
            let l = crate::bld::field::lo_logical(u.x, u.y);
            m.insert(l.clone(), format!("{l}.{k}.pack"));
        }
        assert!(bld_work(&c, &m, &done, &inputs, &by_spatial).is_empty());
        assert!(remaining(&done, |d| bld_next(&c, &m, d, &inputs)).is_empty());
        // A tile no longer in the coverage (its pack and file left from a bigger one) is pruned;
        // its records forgotten with it.
        m.insert("work/bld/6-40-20".into(), "work/bld/6-40-20.0123456789abcdef.sect".into());
        m.insert("layers/buildings/hi/6-40-20".into(), "layers/buildings/hi/6-40-20.0123456789abcdef.pack".into());
        done.record("bldtiles", &[("6/40/20".to_string(), "k".to_string())]);
        let w = bld_work(&c, &m, &done, &inputs, &by_spatial);
        assert_eq!(
            w,
            [Work {
                step: "prune".into(),
                targets: vec![("bldprep 6/40/20".into(), String::new()), ("bldtiles 6/40/20".into(), String::new())]
            }]
        );
        done.record("prune", &w[0].targets);
        assert!(!done.bldtiles.contains_key("6/40/20"));
        // (Not the normalized files while the sources aren't here: every tile would seem to read
        // nothing.)
        let none = BTreeMap::from([
            ("bld-release".to_string(), String::new()),
            ("bld-key".to_string(), crate::buildtiles::TEST_RELEASE.to_string()),
        ]);
        assert_eq!(
            bld_work(&c, &m, &done, &none, &by_spatial)
                .iter()
                .flat_map(|w| w.targets.iter().map(|t| t.0.clone()))
                .filter(|t| t.starts_with("bldprep"))
                .count(),
            0
        );
        // A new fit (BUILDINGS_V) or a neighbour's file made again: its tiles again, nothing else.
        let (t, _) = prep[0].clone();
        let u = Unit::parse(&t).unwrap();
        m.insert(crate::bld::work_logical(u.x, u.y), "work/bld/again.1111111111111111.sect".into());
        let w = bld_work(&c, &m, &done, &inputs, &by_spatial);
        assert!(steps(&w).iter().all(|s| s == "bldtiles" || s == "prune"), "{w:?}");
    }

    #[test]
    fn the_3d_buildings_come_after_the_other_chains_and_in_the_checklist() {
        let c = cov();
        let date = "2026-09-28";
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        unit_inputs(&mut m, date);
        let done = Keys::default();
        let inputs = bld_inputs(&c, "a");
        let w = plan(&c, date, &m, &done, &inputs);
        let s: Vec<&str> = w.iter().map(|x| x.step.as_str()).collect();
        assert_eq!(&s[s.len() - 2..], ["bld-fetch", "bldprep"], "{s:?}");
        assert_eq!(s[0], "heritage-sites");
        // The checklist's line: its tiles' files and tiles, none done; what's next.
        let cl = checklist(&c, date, &m, &done, &inputs, false);
        let b = cl.iter().find(|x| x.id == "buildings").unwrap();
        let tt = bld_targets(&c, &m, &inputs);
        assert_eq!(
            (b.done, b.total, b.unit.as_str()),
            (0, Some(tt.prep.len() + tt.tiles.len()), "tiles"),
            "its tiles the line's, its areas and the fetch's run apart"
        );
        assert_eq!(b.left, Some(1 + tt.prep.len() + tt.tiles.len() + tt.lo.len()), "the fetch's run too");
        assert_eq!(
            b.counts(),
            [
                "1 run left".to_string(),
                format!("0 of {} tiles", tt.prep.len() + tt.tiles.len()),
                format!("0 of {} areas", tt.lo.len())
            ]
        );
        assert_eq!(b.next[0], "Buildings' sources");
        assert!(b.next[1].starts_with("Buildings read: "));
        // Its steps' targets go by tile in the jobs' names.
        assert_eq!(
            next_of(&[Work {
                step: "bldtiles".into(),
                targets: vec![("6/1/1".into(), "k".into()), ("6/1/2".into(), "k".into())]
            }]),
            ["3D buildings: 2 tiles"]
        );
        let st = fold(&checklist_to_come(), &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(
            st.iter().find(|x| x.id == "buildings").unwrap().shared.as_deref(),
            Some("Buildings read and 3D buildings")
        );
        assert!(st.iter().all(|x| x.left.is_none() && x.total.is_none() && !x.finished()), "to come: not sized");
        assert_eq!(st.iter().find(|x| x.id == "roads").unwrap().counts(), ["tiles to come"]);
    }

    #[test]
    fn the_3d_buildings_never_hold_up_a_region_nor_its_catalog() {
        let (c, reach, mut m, mut done) = three();
        let each = c.by_region();
        let inputs = bld_inputs(&c, "a");
        let plan = |m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>| {
            super::plan(
                &c,
                "d",
                m,
                done,
                &inputs,
                Some(&reach),
                &tiles_for(m),
                Publish {
                    each: &each,
                    on_map,
                    served: &BTreeMap::new(),
                    since_catalog: Some(600),
                    held: false,
                    way: Default::default(),
                },
            )
        };
        for u in ["6/28/16", "6/29/16", "6/30/16", "6/31/16"] {
            rbuild(&c, &reach, &mut m, &mut done, u, "6666666666666666");
        }
        let on = |ids: &[&str]| -> BTreeMap<String, bool> { ids.iter().map(|i| (i.to_string(), true)).collect() };
        // Every unit built, the 3D buildings being raised: the publishing chain first, then its
        // catalog, their work after it.
        loop {
            let p = plan(&m, &done, &on(&["a", "b"]));
            assert!(p.work.iter().any(|w| w.step == "bldprep"), "{:?}", steps(&p));
            let w = p.work[0].clone();
            if w.step == "catalog" {
                break;
            }
            assert!(
                CHAIN.contains(&w.step.as_str())
                    || ["slope", "slope-lo", "trees", "trees-lo"].contains(&w.step.as_str())
            );
            if w.step.starts_with("slope") {
                terrain_did(&mut m, &mut done, &w);
            } else {
                did(&mut m, &mut done, &w);
            }
        }
        // Only their sources' fetch left (failing, say, once the release has left S3): it holds
        // nothing back.
        let tt = bld_targets(&c, &m, &inputs);
        done.record("bldprep", &tt.prep);
        done.record("bldtiles", &tt.tiles);
        done.record("bldfield-lo", &tt.lo.iter().map(|(q, k, _)| (q.clone(), k.clone())).collect::<Vec<_>>());
        let p = plan(&m, &done, &on(&["a", "b"]));
        assert!(p.work.iter().any(|w| w.step == "bld-fetch"), "{:?}", p.work);
        assert_eq!(p.work[0].step, "catalog");
        // A tile of theirs made: a catalog with it, once the minute has passed.
        done.record("catalog", &p.work[0].targets);
        let tile = tt.tiles[0].0.replace('/', "-");
        let pack = format!("layers/buildings/hi/{tile}");
        m.insert(pack.clone(), format!("{pack}.1111111111111111.pack"));
        assert_eq!(plan(&m, &done, &on(&["a", "b", "c"])).work[0].step, "catalog");
    }

    #[test]
    fn the_records_keep_what_this_app_doesnt_know() {
        // A newer app's step's records, saved again by this one: kept as they were.
        let json = r#"{"unit": {"6/1/1": "k"}, "bldtiles": {"6/2/2": "b"}, "someday": {"6/3/3": "s"}, "catalog": "c"}"#;
        let mut k: Keys = serde_json::from_str(json).unwrap();
        assert_eq!((k.bldtiles.get("6/2/2").map(String::as_str), k.other.len()), (Some("b"), 1));
        k.record("unit", &[("6/4/4".to_string(), "k4".to_string())]);
        let v: serde_json::Value = serde_json::to_value(&k).unwrap();
        assert_eq!(
            (&v["someday"]["6/3/3"], &v["bldtiles"]["6/2/2"], &v["unit"]["6/4/4"]),
            (&serde_json::json!("s"), &serde_json::json!("b"), &serde_json::json!("k4"))
        );
    }

    /// The spoken job's key: the pass's outlines and the languages' accepted version (none: no job).
    #[test]
    fn the_spoken_job_follows_the_languages_accepted_version() {
        let mut m = BTreeMap::from([("sources/osm/d/outlines".to_string(), "o1".to_string())]);
        let mut done = Keys::default();
        assert!(spoken_work("d", &m, &done).is_none());
        m.insert(
            crate::inputs::logical(crate::inputs::languages::UNIT),
            "sources/inputs/languages/@index.0123456789abcdef.json".into(),
        );
        let w = spoken_work("d", &m, &done).unwrap();
        done.lo.insert("spoken".into(), w.targets[0].1.clone());
        assert!(spoken_work("d", &m, &done).is_none());
        m.insert(
            crate::inputs::logical(crate::inputs::languages::UNIT),
            "sources/inputs/languages/@index.fedcba9876543210.json".into(),
        );
        assert!(spoken_work("d", &m, &done).is_some());
        assert!(spoken_work("e", &m, &done).is_none());
    }

    /// The outlines step's key: its version, the pass, its outline set and the previous outlines;
    /// made again when missing, never without the set.
    #[test]
    fn the_outlines_step_follows_its_set_previous_and_version() {
        let mut m = BTreeMap::new();
        let mut done = Keys::default();
        assert!(outlines_work("2026-09-28", &m, &done).is_none(), "no set, no job");
        m.insert("sources/osm/2026-09-28/sets/outlines".to_string(), "s1".to_string());
        let w = outlines_work("2026-09-28", &m, &done).unwrap();
        assert_eq!((w.step.as_str(), w.targets[0].0.as_str()), ("outlines", "outlines"));
        // Made under its key (the outlines and the record): nothing to do.
        let k = w.targets[0].1.clone();
        m.insert("sources/osm/2026-09-28/outlines".to_string(), "o1".to_string());
        done.record("outlines", &w.targets);
        assert_eq!(done.lo.get("outlines"), Some(&k));
        assert!(outlines_work("2026-09-28", &m, &done).is_none());
        // Outlines made before the step (by the pass, no record): made again, once.
        assert!(outlines_work("2026-09-28", &m, &Keys::default()).is_some());
        // Missing again, or another set, or another previous pass's outlines: made again.
        let gone = BTreeMap::from([("sources/osm/2026-09-28/sets/outlines".to_string(), "s1".to_string())]);
        assert!(outlines_work("2026-09-28", &gone, &done).is_some());
        let mut set = m.clone();
        set.insert("sources/osm/2026-09-28/sets/outlines".to_string(), "s2".to_string());
        assert!(outlines_work("2026-09-28", &set, &done).is_some());
        let mut prev = m.clone();
        prev.insert("sources/osm/2026-03-28/outlines".to_string(), "p1".to_string());
        assert!(outlines_work("2026-09-28", &prev, &done).is_some());
        // A newer pass's outlines aren't this one's previous ones.
        let mut newer = m.clone();
        newer.insert("sources/osm/2027-03-28/outlines".to_string(), "n1".to_string());
        assert_eq!(outlines_key("2026-09-28", &newer), Some(k.clone()));
        // The version and the date are in the key.
        assert_ne!(
            outlines_key(
                "2026-09-29",
                &BTreeMap::from([("sources/osm/2026-09-29/sets/outlines".to_string(), "s1".to_string())])
            ),
            Some(k.clone())
        );
        assert_eq!(k, h(&[&format!("outlines {OUTLINES_V}"), "2026-09-28", "s1", "previous -"]));
        // The rest waits for it while the pass has none, else until it has failed OUTLINES_HOLDS times.
        assert!(
            outlines_hold(false, 99)
                && outlines_hold(true, 0)
                && outlines_hold(true, OUTLINES_HOLDS - 1)
                && !outlines_hold(true, OUTLINES_HOLDS)
        );
    }

    /// The plan's phases timed on a real root, read only (`SCENIC_BENCH_ROOT`: the NAS's project
    /// folder; `SCENIC_BENCH_IDX`: a copy of an agent's `pack-idx`, which the terrain indexes are
    /// read from and kept in). Twice over one coverage, as the agent's loops reuse theirs.
    #[test]
    #[ignore]
    fn bench_plan_on_a_real_root() {
        let Ok(root) = std::env::var("SCENIC_BENCH_ROOT") else { return };
        let root = std::path::PathBuf::from(root);
        let t = std::time::Instant::now();
        let lap = |what: &str, t: &mut std::time::Instant| {
            eprintln!("bench: {what}: {:.2} s", t.elapsed().as_secs_f64());
            *t = std::time::Instant::now();
        };
        let mut t = t;
        let m: BTreeMap<String, String> = crate::out::read_record(&root.join("state/build/manifest.json")).unwrap();
        let date = crate::osmpass::latest_pass_by(&root, false).unwrap();
        let keys = Keys::load(&root);
        let a = crate::inputs::regions::read::source(&root, &m).unwrap();
        lap("records read", &mut t);
        let cov = Coverage::accepted(&root, &a, None).unwrap();
        let each = cov.by_region();
        lap(&format!("coverage made ({} shapes, {} regions)", cov.shapes.len(), each.len()), &mut t);
        let reach = crate::reach::Reaches::load(&root, &m, &date).ok().flatten();
        let mut tiles = TerrainTiles::new(std::env::var("SCENIC_BENCH_IDX").ok().map(std::path::PathBuf::from));
        tiles.load(&root, &m);
        let inputs = crate::agent::input_digests(&root);
        lap("reaches, terrain indexes, input digests", &mut t);
        for round in 1..=2 {
            let times = crate::agent::rekey::FileTimes::new(&root);
            let mut k = keys.clone();
            crate::agent::rekey::as_read(&mut k, &cov, &date, &m, reach.as_ref(), &inputs, &tiles, &times);
            lap(&format!("[{round}] rekey::as_read"), &mut t);
            let dg = |v: String| store::naming::hash16(v.as_bytes());
            let n = dg(format!("{:?}", pois_keys(&cov, &date, &m)));
            lap(&format!("[{round}] pois_keys ({n})"), &mut t);
            let n = dg(format!("{:?}", super::unit_keys(&cov, &date, &m, reach.as_ref(), &tiles)));
            lap(&format!("[{round}] unit_keys ({n})"), &mut t);
            let n = dg(format!("{:?}", region_states(&cov, &each, &date, &m, &k, reach.as_ref(), &tiles)));
            lap(&format!("[{round}] region_states ({n})"), &mut t);
            let n =
                dg(format!("{:?}", super::checklist(&cov, &date, &m, &k, &inputs, false, reach.as_ref(), &[], &tiles)));
            lap(&format!("[{round}] checklist ({n})"), &mut t);
            let on_map = BTreeMap::new();
            let p = super::plan(
                &cov,
                &date,
                &m,
                &k,
                &inputs,
                reach.as_ref(),
                &tiles,
                Publish {
                    each: &each,
                    on_map: &on_map,
                    served: &BTreeMap::new(),
                    since_catalog: Some(0),
                    held: false,
                    way: Default::default(),
                },
            );
            lap(&format!("[{round}] plan ({}; keys {})", dg(format!("{p:?}")), dg(format!("{k:?}"))), &mut t);
            eprintln!("bench: [{round}] the coverage's memos hold {} KB", crate::coverage::memo_bytes() >> 10);
        }
    }
}
