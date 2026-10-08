//! What the build Mac builds for the regions (docs/plan.md §6, Job keys; §8, Order): per z3 pack,
//! the terrain and then the slope of its z6 tiles near the coverage; base(U) for every unit whose
//! piece meets the coverage, a region at a time; pack(T) for the z6 tiles near changed units, the
//! lo packs above them; then a catalog, as each region is done (`plan`).
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
pub const TERRAIN_V: u32 = 1;
pub const SLOPE_V: u32 = 1;
/// 2: elevations up to 6,053 m (`final.u16`, base packs' `elevu`; were clamped at ±3,200 m).
/// 3: heritage sites and area flags from the pass's heritage-sites job (crate::heritage), the
/// flags rasterised per unit.
/// 4: the roads' own English (`global/roaden/<u>`); a unit left with no ways drops its outputs.
pub const UNIT_V: u32 = 4;
/// 2: hidata with rail lines' identity (`railinfo`) and the zoomed-out summaries (`lsum`).
pub const PACK_V: u32 = 2;
/// lo's own version: lo isn't keyed on pack's, so a change in what they share (hipack's tiling)
/// bumps both.
pub const LO_V: u32 = 1;

/// A map tile's key (pack(T)): over the units whose ways come within its 100 km halo (what it
/// reads), then, after a dot, over its owners alone, the units whose ways come into the tile itself:
/// its `here` index points into their base packs, way by way. A round may leave a tile whose halo
/// changed for later, never one whose owners did (`owners_changed`): its catalog lists the owners'
/// base packs as they are, which the tile must index.
fn pack_key(halo: String, owners: String) -> String {
    format!("{halo}.{owners}")
}

/// Whether a map tile drawn under `done` is current for `key`. (A key from before tiles had their
/// owners' part counts by its halo's: a tile whose halo hasn't changed has the same owners.)
pub fn pack_fresh(done: Option<&str>, key: &str) -> bool {
    match done {
        Some(d) if d == key => true,
        Some(d) => !d.contains('.') && key.split_once('.').is_some_and(|(halo, _)| halo == d),
        None => false,
    }
}

/// Whether a tile drawn under `done` had other owners than `key`'s: one of their base packs changed,
/// came or went. One never drawn indexes none. (One drawn before keys had their owners' part:
/// whenever its halo changed.)
fn owners_changed(done: Option<&str>, key: &str) -> bool {
    let Some(d) = done else { return false };
    let owners = |k: &str| k.split_once('.').map(|(_, o)| o.to_string());
    match owners(d) {
        Some(o) => owners(key).as_deref() != Some(o.as_str()),
        None => key.split_once('.').map_or(key, |(halo, _)| halo) != d,
    }
}

/// The keys of the jobs that last succeeded, by step and target ("3/4/2", "6/31/20").
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Keys {
    #[serde(default)]
    pub terrain: BTreeMap<String, String>,
    #[serde(default)]
    pub slope: BTreeMap<String, String>,
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

    /// The keys with every waiting hand-off's done records on top (crate::handoff): what both agents
    /// plan with, so neither builds again what the helper built and the build Mac hasn't merged yet.
    /// The hand-offs are listed first: a merge meanwhile has then put them in the keys read after.
    pub fn load_with_handoffs(root: &Path) -> anyhow::Result<Keys> {
        Keys::load_with(root, &[crate::handoff::nas_base(root)])
    }

    /// `load_with_handoffs` for the hand-offs under each of `bases` (the NAS's, the coordinator's
    /// journal on this Mac), and the files they save (`handed`).
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
            "unit" => &mut self.unit,
            "pois" => &mut self.pois,
            "peaks" => &mut self.peaks,
            "pack" => &mut self.pack,
            "trees" => &mut self.trees,
            "trees-lo" => &mut self.trees_lo,
            "bldprep" => &mut self.bldprep,
            "bldtiles" => &mut self.bldtiles,
            _ => &mut self.lo,
        }
    }

    /// The key recorded for `target` of a per-target step.
    pub fn recorded(&self, step: &str, target: &str) -> Option<&str> {
        let m = match step {
            "terrain" => &self.terrain,
            "slope" => &self.slope,
            "unit" => &self.unit,
            "pois" => &self.pois,
            "peaks" => &self.peaks,
            "pack" => &self.pack,
            "lo" => &self.lo,
            "trees" => &self.trees,
            "trees-lo" => &self.trees_lo,
            "bldprep" => &self.bldprep,
            "bldtiles" => &self.bldtiles,
            _ => return None,
        };
        m.get(target).map(String::as_str)
    }

    /// Records a job's targets as done with their keys. A prune forgets its targets' keys instead
    /// ("unit 6/x/y", "pois 6/x/y", "pack 6/x/y", "lo 3/x/y", "bldprep 6/x/y", "bldtiles 6/x/y"), so a
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
        if step.ends_with("-root") || matches!(step, "labels" | "water" | "trailends" | "reach" | "summits" | "items" | "marks" | "roadunits" | "stations" | "ferries" | "heritage-sites" | "heritage" | "overlays" | "rail-feeds" | "rail" | "bld-fetch") {
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
fn coverage_all(cov: &Coverage) -> String {
    let mut v: Vec<String> = cov.shapes.iter().map(|s| format!("{}:{}", s.buffer_m, store::naming::hash16(bytemuck::cast_slice(&s.rings.concat())))).collect();
    v.sort();
    v.dedup();
    v.join(",")
}

fn grown_e7(z: u8, x: u32, y: u32, km: f64) -> [i32; 4] {
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

/// Every hiking route's ends, worldwide, once per pass (crate::trailends): what the units' extract
/// reads.
pub const TRAILENDS_V: u32 = 1;

pub fn trailends_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let set = m.get(&crate::osmpass::set_name(date, "hikes"))?;
    let k = h(&[&format!("trailends {TRAILENDS_V}"), set]);
    (done.lo.get("trailends").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "trailends".into(), targets: vec![("trailends".into(), k)] })
}

/// Each unit's reach (crate::reach), once per pass: which units the coverage builds, and what their
/// keys read of it.
pub fn reach_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let k = reach_key(date, m)?;
    let made = m.contains_key(&crate::reach::logical(date));
    (!made || done.lo.get("reach").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "reach".into(), targets: vec![("reach".into(), k)] })
}

/// The reach job's key: its version and the pieces, by content (a piece cut again changes it).
pub fn reach_key(date: &str, m: &BTreeMap<String, String>) -> Option<String> {
    let prefix = format!("sources/osm/{date}/pieces/");
    let pieces: Vec<&str> = m.range(prefix.clone()..).take_while(|(l, _)| l.starts_with(&prefix)).map(|(_, c)| c.as_str()).collect();
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
    (done.lo.get("labels").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "labels".into(), targets: vec![("labels".into(), k)] })
}

/// The water layer (crate::water), worldwide, once per pass (or layer version): drawn from the
/// pass's basemap's z14 water.
pub fn water_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let basemap = m.get(&format!("layers/basemap/world-{date}"))?;
    let k = h(&[&format!("water {}", crate::water::VERSION), basemap]);
    (done.lo.get("water").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "water".into(), targets: vec![("water".into(), k)] })
}

/// Whether unit `u` is built for the coverage: some road it owns may touch it (`Reach::builds`;
/// the unit step then keeps exactly the ways that do).
pub fn builds(cov: &Coverage, reach: &Reaches, u: Unit) -> bool {
    reach.get(u).is_some_and(|r| r.builds(cov))
}

/// The units the coverage builds (`builds`), each with its key (`unit_key`); None for one whose
/// terrain can't be worked out now (a terrain pack's index unread: agent::tiles), which waits. None
/// before the pass's reaches are made.
pub fn unit_keys(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, reach: Option<&Reaches>, digests: &BTreeMap<String, String>, tiles: &TerrainTiles) -> Vec<(Unit, Option<String>)> {
    let mut units = Vec::new();
    let Some(reach) = reach else { return units };
    for (l, c) in m.range(format!("sources/osm/{date}/pieces/")..) {
        let Some(u) = l.strip_prefix(&format!("sources/osm/{date}/pieces/")).and_then(Unit::parse) else { break };
        let Some(r) = reach.get(u).filter(|r| r.builds(cov)) else { continue };
        units.push((u, unit_terrain(u, r, m, tiles).ok().map(|t| unit_key(cov, date, m, u, c, r, digests, &t))));
    }
    units
}

/// Unit `u`'s key (its piece `piece`, its reach `r`): what it reads. Its piece and road values, the
/// coverage as its ways meet it (`Reach::coverage_key`) and the location rules where they go
/// (`crate::rules`), the roadside buildings' index, in Taiwan the MOI DTM's files
/// (`digests["moi-dtm"]`), the terrain tiles it reads by their contents (`terrain`: `unit_terrain`'s
/// digest), and the heritage sites' and areas' slices near it, as the manifest has them, which is
/// what the unit step stages from.
#[allow(clippy::too_many_arguments)]
pub fn unit_key(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, u: Unit, piece: &str, r: &Reach, digests: &BTreeMap<String, String>, terrain: &str) -> String {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let mut inputs = vec![format!("unit {UNIT_V}"), piece.to_string(), get(&format!("sources/osm/{date}/roads/{}", u.dash())).to_string(), r.coverage_key(cov, u), crate::rules::versions_meeting(r.extent(u))];
    // Taiwan's DEM when it's there (a file dropped in reruns the units it covers).
    if crate::rules::meets_taiwan(r.extent(u)) {
        inputs.push(format!("moi-dtm {}", digests.get("moi-dtm").map(String::as_str).unwrap_or("-")));
    }
    // The roadside buildings it reads (crate::buildtiles: the release's tiles near its roads).
    inputs.push(format!("buildings {}", get(&crate::buildtiles::index_logical())));
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
    let from = [serde_json::to_string(r).unwrap_or_default(), packs.into_iter().collect::<Vec<_>>().join(",")].join("\n");
    tiles.memo(&u.slash(), &store::naming::hash16(from.as_bytes()), || Ok(terrain_digest(&unit_terrain_tiles(u, r, m, tiles)?)))
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
pub fn unit_terrain_tiles(u: Unit, r: &Reach, m: &BTreeMap<String, String>, tiles: &TerrainTiles) -> Result<Vec<Tile>, Unread> {
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
/// coverage and registers' snapshot. (2: an area across the antimeridian sliced by its parts.)
pub const HERITAGE_SITES_V: u32 = 2;

pub fn heritage_sites_work(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let set = m.get(&crate::osmpass::set_name(date, "areas"))?;
    let registers = m.get("sources/registers/legacy")?;
    let k = h(&[&format!("heritage-sites {HERITAGE_SITES_V}"), date, set, registers, &coverage_all(cov)]);
    (done.lo.get("heritage-sites").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "heritage-sites".into(), targets: vec![("heritage-sites".into(), k)] })
}

/// Every summit worldwide with its z8 height, once per pass (crate::summits): what the units' peaks
/// read.
pub const SUMMITS_V: u32 = 1;

pub fn summits_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let set = m.get(&crate::osmpass::set_name(date, "summits"))?;
    let (z8, z8m) = (m.get(&crate::terrain_z8::logical())?, m.get(&crate::terrain_z8::max_logical())?);
    let k = h(&[&format!("summits {SUMMITS_V}"), set, z8, z8m]);
    (done.lo.get("summits").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "summits".into(), targets: vec![("summits".into(), k)] })
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
/// the z8, and the terrain hi packs within 30 km of it, across the antimeridian too (its peaks'
/// z12: the packs', else AWS's raw tiles, which don't change).
pub fn peaks_keys(cov: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let common = [get(&format!("work/summits/{date}")), get(&crate::terrain_z8::logical()), get(&crate::terrain_z8::max_logical())].join(",");
    let mut out = Vec::new();
    for (u, _) in pois_keys(cov, date, m) {
        let Some(c) = m.get(&format!("work/pois/{}", u.dash())) else { continue };
        let mut inputs = vec![format!("peaks {PEAKS_V}"), c.clone(), common.clone()];
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
pub const STATIONS_V: u32 = 1;
pub const FERRIES_V: u32 = 1;
/// The landmark points from the candidates (crate::marksjob).
pub const MARKS_V: u32 = 1;
/// The rest of the heritage chain on the heritage-sites outputs (scenic-build heritage), and the
/// area overlays from it with the marks' World Heritage dots (ovconv::overlays).
pub const HERITAGE_V: u32 = 1;
pub const OVERLAYS_V: u32 = 1;

/// The rail service (crate::rail): the feeds for the coverage, fetched once (dem/railfeeds.py), and
/// trains a day on its rail ways (dem/railgtfs.py, railfreq: global/railfreq).
pub const RAIL_FEEDS_V: u32 = 1;
pub const RAIL_V: u32 = 1;

/// Where the marks' and overlays' heritage comes from (markconv::heritage_source): the pass's
/// heritage job's outputs when there are any, else today's.
fn heritage_src(m: &BTreeMap<String, String>, date: &str) -> String {
    let job = format!("work/heritage/{date}");
    if ["layer-heritage", "details-heritage", "props-heritage"].iter().all(|s| m.contains_key(&format!("{job}/{s}"))) {
        job
    } else {
        crate::markconv::LEGACY.to_string()
    }
}

/// The current units' candidates (their content names), for the worldwide jobs' keys.
fn current_pois<'a>(cov: &Coverage, date: &str, m: &'a BTreeMap<String, String>) -> Vec<&'a str> {
    pois_keys(cov, date, m).into_iter().filter_map(|(u, _)| m.get(&format!("work/pois/{}", u.dash())).map(String::as_str)).collect()
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
pub fn region_states(cov: &Coverage, regions: &[(String, Coverage)], date: &str, m: &BTreeMap<String, String>, done: &Keys, reach: Option<&Reaches>, digests: &BTreeMap<String, String>, tiles: &TerrainTiles) -> BTreeMap<String, RegionState> {
    let keys = unit_keys(cov, date, m, reach, digests, tiles);
    regions
        .iter()
        .map(|(id, rc)| {
            let mine: Vec<&(Unit, Option<String>)> = keys.iter().filter(|(u, _)| reach.is_some_and(|r| builds(rc, r, *u))).collect();
            // (One whose key can't be worked out now isn't counted as built.)
            let built = mine.iter().filter(|(u, k)| k.is_some() && done.unit.get(&u.slash()) == k.as_ref()).count();
            (id.clone(), RegionState { built, total: mine.len() })
        })
        .collect()
}

/// Terrain's and slope's targets (their z3 packs near the coverage), each with its key, done or not.
pub fn terrain_slope_targets(cov: &Coverage, m: &BTreeMap<String, String>) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let (mut terrain, mut slope) = (Vec::new(), Vec::new());
    for (q, ts) in &coverage_tiles(cov) {
        let qs = format!("3/{}/{}", q.0, q.1);
        let tlist: Vec<String> = ts.iter().map(|t| format!("6/{}/{}", t.0, t.1)).collect();
        terrain.push((qs.clone(), h(&[&format!("terrain {TERRAIN_V}"), &tlist.join(" "), &cov.fingerprint(grown_e7(3, q.0, q.1, 20.0))])));
        // Slope reads the terrain packs of q (as they are now; a terrain job changes them first).
        let mut inputs = vec![format!("slope {SLOPE_V}"), get(&format!("layers/terrain/lo/3-{}-{}", q.0, q.1)).to_string()];
        inputs.extend(ts.iter().map(|t| get(&format!("layers/terrain/hi/6-{}-{}", t.0, t.1)).to_string()));
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        slope.push((qs, h(&refs)));
    }
    (terrain, slope)
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
    let stale_pieces: BTreeSet<String> = tt.pieces.iter().filter(|(t, k, _)| !current(t, k)).map(|p| p.0.clone()).collect();
    let stale_lo: BTreeSet<String> = tt.lo.iter().filter(|(q, k, _)| done.trees_lo.get(q) != Some(k)).map(|l| l.0.clone()).collect();
    let changing: BTreeSet<String> = stale_lo.iter().cloned().chain(stale_pieces.iter().filter_map(|t| area_of(t))).collect();
    let mut w = TreeWork::default();
    for (t, k, none) in &tt.pieces {
        let needs_mid = !none && current(t, k) && !has_mid(t) && !mid_handed(t);
        if !current(t, k) || (needs_mid && area_of(t).is_some_and(|q| changing.contains(&q))) {
            w.pieces.push((t.clone(), k.clone()));
        } else if needs_mid {
            w.backfill.push((t.clone(), k.clone()));
        }
    }
    w.lo = tt.lo.iter().filter(|(q, _, none)| stale_lo.contains(q) && (*none || tt.pieces_of(q).all(|(t, k, _)| current(t, k) && has_mid(t)))).map(|(q, k, _)| (q.clone(), k.clone())).collect();
    w.stale_pieces = stale_pieces;
    w.stale_lo = stale_lo;
    w
}

/// How long after a round began the next may begin while the units are still being built
/// (docs/plan.md §8, Order), a region being done: so a round goes out about every hour, its
/// regions' slope and tree cover made before it as they're done, and it draws (the map tiles near
/// what changed, the road index, rail stops, a catalog). After the last unit, at once.
pub const PUBLISH_EVERY_S: u64 = 3600;

/// A round under way: what it publishes is fixed as it begins, so the regions done and the units
/// built meanwhile wait for the next. The agent keeps it (its folder's `round.json`) from when it
/// begins until its catalog (or held catalog) is made: then it's over, whatever changed meanwhile
/// (that goes out with the next).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Round {
    /// When it began (seconds since the epoch).
    pub began: u64,
    /// The regions it publishes: those done, and not on the map as they are now, when it began.
    pub regions: Vec<String>,
    /// Whether it's the last: nothing was left to build when it began.
    pub last: bool,
    /// The units' outputs and the 3D buildings' packs in the manifest when it began
    /// (crate::out::AS_OF_OUTPUTS): its map tiles, road index, rail stops and catalog are made from
    /// them (crate::out::units_as_of; its jobs,
    /// `AS_OF_STEPS`, read them through crate::out::UNITS_AS_OF_ENV).
    pub units: BTreeMap<String, String>,
    /// It's over (its catalog made, or nothing left it would publish): kept, without its units, for
    /// when it began.
    #[serde(default)]
    pub over: bool,
}

/// The steps a round alone plans after its slope and tree cover (the roads' chain and its
/// catalog): run with the units as they were when it began (those of them that read none, ferries
/// and the world-level terrain and slope, all the same).
pub const AS_OF_STEPS: [&str; 9] = ["roadunits", "pack", "lo", "stations", "ferries", "terrain-root", "slope-root", "catalog", "catalog-held"];

/// What the plan builds the regions by, one at a time, and publishes them by, as they're done:
/// each region's own coverage, as the recipes list them (crate::coverage::Coverage::by_region); the
/// regions the map's catalog has; when the last round began; the round under way.
#[derive(Clone, Copy)]
pub struct Rounds<'a> {
    pub each: &'a [(String, Coverage)],
    /// The last catalog's regions by id: whether it has the region as its recipe is now (false: an
    /// older outline, the region on the map as it was).
    pub on_map: &'a BTreeMap<String, bool>,
    /// Seconds since the last round began (or the last catalog went out, before the agent kept
    /// rounds); None when none has.
    pub since_last: Option<u64>,
    /// The round under way, if one is.
    pub current: Option<&'a Round>,
    /// Catalogs are held for review (`inputs/hold-catalog`): what's new is weighed against the last
    /// held one's.
    pub held: bool,
}

/// The work there is, in order, and the regions a catalog made now records as built (`ready`: every
/// unit of theirs built as the coverage wants it, and their areas' slope and tree cover; the
/// catalog keeps the others as the last one had them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub work: Vec<Work>,
    pub ready: Vec<String>,
    /// A round's slope and tree cover (step, target) its catalog waits for: while another worker
    /// builds one (a helper's lease), the catalog waits rather than go out without its region.
    pub publish_waits: Vec<(String, String)>,
    /// Every region's work left, in the order they're built (those with none last), whether or not
    /// it can start now: what the forecast schedules (crate::agent::forecast).
    pub regions: Vec<RegionLeft>,
    /// A round begins (none was under way): the agent keeps it, its `began` set, and plans with it
    /// as `Rounds::current` until its catalog is out. The work is the round's already.
    pub begins: Option<Round>,
    /// The round under way has nothing left to do and no catalog to make (its catalog would be the
    /// last's): it's over. (Its catalog made, the agent ends it.)
    pub ends: bool,
    /// The round under way's chain to its end, as if each step succeeded (its map tiles, road index,
    /// rail stops, ferries, the world-level terrain and slope, its catalog): for the forecast.
    pub round_left: Vec<Work>,
    /// The units whose key can't be worked out now (a terrain pack's index unread: `unit_keys`):
    /// neither built nor counted as built until it can.
    pub unknown: Vec<String>,
    /// The tree cover pieces whose mids are made in idle time (`TreeWork::backfill`: the work's
    /// last): for the forecast.
    pub backfill: Vec<(String, String)>,
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
    /// The stale terrain areas it reads, and those it builds itself (the rest come with a region
    /// before it).
    pub terrain: Vec<String>,
    pub own_terrain: Vec<String>,
    /// Its areas whose slope is stale; the tree cover pieces of its areas to make (stale, or their
    /// mids for an assembly), and its areas whose tree cover assembly is stale.
    pub slope: Vec<String>,
    pub trees: Vec<String>,
    #[serde(default)]
    pub trees_lo: Vec<String>,
}

/// The plan for the coverage `cov`, the pass of `date`, the build manifest `m` (logical → content)
/// and what was done (`done`). `inputs`: digests of what jobs read from `inputs/` (not in the
/// manifest), by name: "ferries-freq" (the ferry timetables), "regions" (the recipes). `tiles`: the
/// terrain packs' indexes, which the units' keys read (`unit_terrain`). The agent runs
/// the first work not waiting out a failure; a helper takes a step's from the far end (the agent
/// offers each step's targets together, in this order). The order (docs/plan.md §8, Order):
/// - the heritage sites the units read;
/// - a region at a time (the regions the map hasn't at all first, the one with the fewest units
///   left first): the terrain areas it reads that are stale, then its units whose terrain is built
///   (a unit's key reads the terrain near it: one built before would be built again);
/// - slope (each area once its terrain is built) and tree cover after them (its pieces, then each
///   area's assembly once its pieces are built: `tree_work`), but a region's done and waiting for
///   its round before all that;
/// - a round as a region is done, PUBLISH_EVERY_S after the last began, fixed as it begins
///   (`Round`): its areas' slope and tree cover, the map tiles, the road index, rail stops and
///   ferries, and a catalog with its regions, from the units as they were when it began;
/// - after the last unit and terrain area, the same for everything;
/// - the trains' and the landmarks' chains from the start, each step once what it reads is built,
///   after all that in the order (a second job beside the regions' takes them: crate::agent), the
///   overlays after the last unit; what they make goes out with the next round's catalog, or one
///   of its own after the last;
/// - the 3D buildings' chain from the start too (`bld_work`: the sources' fetch, each z6 tile's
///   normalized file, then its tiles once it and its neighbours are prepared), listed after the
///   other chains, its tiles in the regions' order; it holds no round and no region, and while it
///   has work left after the last unit, a round (and its catalog) at most an hour after the last;
/// - last, in idle time, the mids of current tree cover pieces without one (`TreeWork::backfill`).
#[allow(clippy::too_many_arguments)]
pub fn plan(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, reach: Option<&Reaches>, tiles: &TerrainTiles, rounds: Rounds) -> Plan {
    let mut work = Vec::new();
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    let push = |work: &mut Vec<Work>, step: &str, targets: Vec<(String, String)>| {
        if !targets.is_empty() {
            work.push(Work { step: step.into(), targets });
        }
    };

    // The heritage sites and designated areas the units read: first, once per pass and coverage (one
    // job). Not waited for: terrain still runs while it waits out a failure.
    let sites = heritage_sites_work(cov, date, m, done);
    let sites_pending = sites.is_some();
    work.extend(sites);

    // Terrain per z3 pack; slope (it reads the terrain: each area once its terrain is built) and the
    // tree cover layers (pieces per z6 tile, then each z3 tile's assembly), what a region needs
    // before it's published.
    let (terrain, slope) = terrain_slope_targets(cov, m);
    let terrain: Vec<(String, String)> = terrain.into_iter().filter(|(t, k)| stale(&done.terrain, t, k)).collect();
    let terrain_left: BTreeSet<String> = terrain.iter().map(|t| t.0.clone()).collect();
    let slope: Vec<(String, String)> = slope.into_iter().filter(|(t, k)| stale(&done.slope, t, k)).collect();
    let tt = crate::treepacks::targets(cov, m);
    let TreeWork { pieces: trees, lo: trees_lo, backfill, stale_pieces: trees_left, stale_lo: trees_lo_left } = tree_work(&tt, m, done);
    // (What a region lacks before it's published: stale slope counts, built or not yet buildable.)
    let slope_left: BTreeSet<String> = slope.iter().map(|t| t.0.clone()).collect();
    let slope: Vec<(String, String)> = slope.into_iter().filter(|t| !terrain_left.contains(&t.0)).collect();
    // A tree cover target's area (a piece's z3 tile, an assembly's own).
    let tree_area = |t: &(String, String)| crate::treepacks::area_of(&t.0).unwrap_or_else(|| t.0.clone());

    // The units wait for the heritage sites, the pass's reaches (which units the coverage builds)
    // and the release's roadside buildings (a worldwide job, once): the terrain, slope and tree
    // cover meanwhile.
    let waiting = sites_pending || !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources")) || !m.contains_key(&crate::buildtiles::index_logical());
    // The trains' and the landmarks' chains: from the start, each step once what it reads is built
    // (`landmarks_work`: none waits for the units but the overlays), listed after the regions'
    // work. A second job beside the regions' takes them first (crate::agent, Two jobs at once), a
    // helper the candidates and peaks, the build Mac's own job once the regions' work is done.
    let chains = |last: bool| -> Vec<Work> { rail_chain(cov, date, m, done, inputs).into_iter().chain(landmarks_work(cov, date, m, done, &terrain_left, last)).collect() };
    let Some(reach) = reach.filter(|_| !waiting) else {
        push(&mut work, "terrain", terrain);
        push(&mut work, "slope", slope);
        push(&mut work, "trees", trees);
        push(&mut work, "trees-lo", trees_lo);
        work.extend(chains(false));
        let each: Vec<&Coverage> = rounds.each.iter().map(|(_, c)| c).collect();
        work.extend(bld_work(cov, m, done, inputs, &bld_rank(&each)));
        push(&mut work, "trees", backfill.clone());
        return Plan { work, backfill, ..Default::default() };
    };

    // base(U): the units the coverage builds. Each region's: those it builds itself (a road of theirs
    // may touch its outline), the ones of them not built as the coverage now wants, its areas (the
    // z3 tiles its slope and tree cover are in) and the terrain areas it reads (its areas and those
    // near its units).
    let units = unit_keys(cov, date, m, Some(reach), inputs, tiles);
    // (One whose key can't be worked out now is left, and isn't built: `unknown`.)
    let unit_stale: Vec<bool> = units.iter().map(|(u, k)| k.as_ref().is_none_or(|k| stale(&done.unit, &u.slash(), k))).collect();
    // A unit's terrain areas: those of the z6 tiles near it, whose packs hold the tiles its key
    // reads (every tile it stages is in the lo or hi pack of a z3 tile it meets).
    let reads: Vec<BTreeSet<String>> = units
        .iter()
        .map(|(u, _)| crate::stage::tiles_in(6, crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM)).into_iter().map(|(x, y)| format!("3/{}/{}", x >> 3, y >> 3)).collect())
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
    }
    let regions: Vec<Region> = rounds
        .each
        .iter()
        .map(|(id, rc)| {
            let all: Vec<usize> = (0..units.len()).filter(|&i| builds(rc, reach, units[i].0)).collect();
            let stale: Vec<usize> = all.iter().copied().filter(|&i| unit_stale[i]).collect();
            let areas: BTreeSet<String> = (0..8u32).flat_map(|x| (0..8u32).map(move |y| (x, y))).filter(|&(x, y)| rc.meets_rect(grown_e7(3, x, y, 20.0))).map(|(x, y)| format!("3/{x}/{y}")).collect();
            let tree_areas: BTreeSet<String> = areas.iter().filter(|a| crate::legacy::Unit::parse(a).is_some_and(|q| rc.meets_rect(crate::hipack::tile_bounds(3, q.x, q.y)))).cloned().collect();
            let tree_pieces: BTreeSet<String> = tree_areas.iter().flat_map(|q| tt.pieces_of(q)).filter(|p| Unit::parse(&p.0).is_some_and(|u| rc.meets_rect(crate::hipack::tile_bounds(6, u.x, u.y)))).map(|p| p.0.clone()).collect();
            let terrain = areas.iter().cloned().chain(stale.iter().flat_map(|&i| reads[i].iter().cloned())).filter(|a| terrain_left.contains(a)).collect();
            Region { id, all, stale, areas, tree_areas, tree_pieces, terrain }
        })
        .collect();
    // A unit built since the round under way began (rebuilt, or new): the round has it as it was.
    let built_since: Vec<bool> = match rounds.current {
        Some(rd) => units
            .iter()
            .map(|(u, _)| {
                crate::out::UNIT_OUTPUTS.iter().any(|p| {
                    let l = format!("{p}{}", u.dash());
                    m.get(&l) != rd.units.get(&l)
                })
            })
            .collect(),
        None => vec![false; units.len()],
    };
    // A region the round under way publishes: one of its regions, none of its units built since
    // (one redrawn and built again meanwhile goes out with the next).
    let of_round = |r: &Region| rounds.current.is_some_and(|rd| rd.regions.iter().any(|x| x == r.id)) && !r.all.iter().any(|&i| built_since[i]);
    // The regions a catalog made now records as built: every unit of theirs built as the coverage
    // wants it, and their areas' slope and tree cover; while a round is under way, of those only its
    // own and those on the map as they are (the others' units may be newer than the round's).
    let in_round = |r: &Region| rounds.current.is_none() || of_round(r) || rounds.on_map.get(r.id) == Some(&true);
    let ready: Vec<String> = regions.iter().filter(|r| r.stale.is_empty() && r.terrain.is_empty() && r.areas.is_disjoint(&slope_left) && r.tree_pieces.is_disjoint(&trees_left) && r.tree_areas.is_disjoint(&trees_lo_left) && in_round(r)).map(|r| r.id.to_string()).collect();

    // A region at a time: those the map hasn't at all first, then those it has (redrawn, or their
    // units' keys changed: on the map as they were meanwhile); of each, the one with the fewest units
    // left first, so regions are done (and published) as soon as they can be. Each region's stale
    // terrain areas, then its units whose terrain is built, neighbours together (`spatial_order`), so
    // the downloads and caches one fills serve the next; a unit or an area two regions share comes
    // with the first. (A unit whose terrain isn't built comes once it is: the plan's made again after
    // each job.)
    let mut order: Vec<&Region> = regions.iter().filter(|r| !r.stale.is_empty() || !r.terrain.is_empty()).collect();
    order.sort_by_key(|r| (rounds.on_map.contains_key(r.id), r.stale.len(), r.stale.iter().map(|&i| spatial_order(units[i].0)).min(), r.id));
    let mut taken = vec![false; units.len()];
    let mut listed: BTreeSet<String> = BTreeSet::new();
    let mut by_region: Vec<Work> = Vec::new();
    for r in &order {
        let t: Vec<(String, String)> = terrain.iter().filter(|(q, _)| r.terrain.contains(q) && listed.insert(q.clone())).cloned().collect();
        push(&mut by_region, "terrain", t);
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
        let own_terrain: Vec<String> = terrain.iter().filter(|(q, _)| r.terrain.contains(q) && claimed_terrain.insert(q.clone())).map(|t| t.0.clone()).collect();
        lefts.push(RegionLeft {
            id: r.id.to_string(),
            on_map: rounds.on_map.get(r.id).copied(),
            units: all.iter().map(|&i| units[i].0.slash()).collect(),
            own_units: own.iter().map(|&i| units[i].0.slash()).collect(),
            terrain: r.terrain.iter().cloned().collect(),
            own_terrain,
            slope: r.areas.iter().filter(|a| slope_left.contains(*a)).cloned().collect(),
            trees: trees.iter().filter(|t| r.tree_areas.contains(&tree_area(t))).map(|t| t.0.clone()).collect(),
            trees_lo: r.tree_areas.iter().filter(|a| trees_lo_left.contains(*a)).cloned().collect(),
        });
    }

    // (Terrain no region with work left reads, and a unit in no region's own coverage, last.)
    push(&mut by_region, "terrain", terrain.iter().filter(|(q, _)| !listed.contains(q)).cloned().collect());
    let mut rest: Vec<usize> = (0..units.len()).filter(|&i| unit_stale[i] && !taken[i] && buildable(i)).collect();
    rest.sort_by_key(|&i| spatial_order(units[i].0));
    push(&mut by_region, "unit", rest.into_iter().filter_map(target).collect());
    // Slope and tree cover in the same order: the areas of the region built first, first.
    let rank = |t: &(String, String)| order.iter().position(|r| r.areas.contains(&t.0)).unwrap_or(usize::MAX);
    let tree_rank = |t: &(String, String)| order.iter().position(|r| r.tree_areas.contains(&tree_area(t))).unwrap_or(usize::MAX);
    let (mut slope, mut trees, mut trees_lo) = (slope, trees, trees_lo);
    slope.sort_by_key(rank);
    trees.sort_by_key(tree_rank);
    trees_lo.sort_by_key(tree_rank);

    // A round: when a region is done that the map hasn't as it is now, an hour after the last began
    // while units or terrain are left, and after the last. What it publishes is fixed as it begins
    // (`Round`): its regions, and the units as they were then, which its map tiles, road index, rail
    // stops and catalog are made from; regions done and units built meanwhile wait for the next.
    // First the slope and tree cover its regions' areas lack (after the last, all that's left), then
    // what isn't the units' own and goes out with them: what the coverage no longer builds pruned,
    // the roads' chain (the map tiles, the road index, rail stops and ferries, the world-level
    // terrain and slope) and a catalog (with the trains' and the landmarks' work made by then).
    let done_now = |r: &Region| r.stale.is_empty() && r.terrain.is_empty();
    // (Done, and not on the map as it is now: what a round publishes.)
    let to_publish: Vec<&Region> = regions.iter().filter(|r| done_now(r) && rounds.on_map.get(r.id) != Some(&true)).collect();
    let last_now = !unit_stale.iter().any(|&s| s) && terrain_left.is_empty();
    // The 3D buildings' chain (`bld_work`): its tiles in the regions' order, those the regions
    // being built first.
    let bld_regions: Vec<&Coverage> = order.iter().map(|r| r.id).chain(regions.iter().map(|r| r.id)).filter_map(|id| rounds.each.iter().find(|(x, _)| x == id).map(|(_, c)| c)).collect();
    let bld = bld_work(cov, m, done, inputs, &bld_rank(&bld_regions));
    // (After the last unit, a round as soon as anything changed; but while the 3D buildings are
    // being raised, at most an hour after the last began, not a catalog for each of their jobs,
    // unless a region waits to be published: the buildings never hold one up. Their sources'
    // fetch doesn't count: it changes nothing served, and one failing and tried again (the release
    // gone from S3) would keep the catalogs hourly for good.)
    let hourly = rounds.since_last.is_none_or(|s| s >= PUBLISH_EVERY_S);
    let raising = bld.iter().any(|w| w.step != "bld-fetch");
    if rounds.current.is_none() && ((last_now && (!raising || hourly || !to_publish.is_empty())) || (!to_publish.is_empty() && hourly)) {
        let units_now = m.iter().filter(|(l, _)| crate::out::AS_OF_OUTPUTS.iter().any(|p| l.starts_with(p))).map(|(l, c)| (l.clone(), c.clone())).collect();
        let begun = Round { began: 0, regions: to_publish.iter().map(|r| r.id.to_string()).collect(), last: last_now, units: units_now, over: false };
        let p = plan(cov, date, m, done, inputs, Some(reach), tiles, Rounds { current: Some(&begun), ..rounds });
        // (One with nothing to publish that isn't out already, after the last: none.)
        if !p.ends {
            return Plan { begins: Some(begun), ..p };
        }
    }
    // The slope and tree cover of the regions done and waiting for a round (not this one): before
    // the regions' work, so their round only draws.
    let waiting = |r: &&&Region| rounds.current.is_none_or(|rd| !rd.regions.iter().any(|x| x == r.id));
    let slope_waits = |t: &(String, String)| to_publish.iter().filter(waiting).any(|r| r.areas.contains(&t.0));
    let trees_waits = |t: &(String, String)| to_publish.iter().filter(waiting).any(|r| r.tree_areas.contains(&tree_area(t)));
    let mut publish_waits = Vec::new();
    let mut ends = false;
    let mut round_left = Vec::new();
    if let Some(rd) = rounds.current {
        let last = rd.last;
        // (A region of it no longer done, its recipe edited since, or done again with units built
        // since, waits for another.)
        let publish: Vec<&Region> = regions.iter().filter(|r| done_now(r) && of_round(r)).collect();
        let now = |t: &(String, String)| last || publish.iter().any(|r| r.areas.contains(&t.0));
        let trees_due = |t: &(String, String)| last || publish.iter().any(|r| r.tree_areas.contains(&tree_area(t)));
        let slope_now: Vec<(String, String)>;
        let trees_now: Vec<(String, String)>;
        let lo_now: Vec<(String, String)>;
        (slope_now, slope) = slope.into_iter().partition(now);
        (trees_now, trees) = trees.into_iter().partition(trees_due);
        (lo_now, trees_lo) = trees_lo.into_iter().partition(trees_due);
        publish_waits = slope_now.iter().map(|t| ("slope".to_string(), t.0.clone())).chain(trees_now.iter().map(|t| ("trees".to_string(), t.0.clone()))).chain(lo_now.iter().map(|t| ("trees-lo".to_string(), t.0.clone()))).collect();
        let before = work.len();
        push(&mut work, "slope", slope_now);
        push(&mut work, "trees", trees_now);
        push(&mut work, "trees-lo", lo_now);
        // A round before the last draws the map tiles that go out with it: those meeting a region
        // it publishes, those no unit to build when it began is near (their 100 km halo: what they
        // read), and those whose owners changed (`owners_changed`). The others would be drawn again
        // as those units are built (a region's border tiles, in every round); the last round draws
        // all. (Units to build when it began: those to build now, and those built since.)
        let m_then = crate::out::units_as_of(m, &rd.units);
        let to_build: Vec<[i32; 4]> = (0..units.len()).filter(|&i| unit_stale[i] || built_since[i]).map(|i| reach.get(units[i].0).map(|r| r.owned_extent(units[i].0)).unwrap_or_else(|| crate::reach::near_box(units[i].0))).collect();
        let each: Vec<&Coverage> = publish.iter().filter_map(|r| rounds.each.iter().find(|(id, _)| id == r.id).map(|(_, c)| c)).collect();
        let keep = |x: u32, y: u32| {
            let b = crate::hipack::tile_bounds(6, x, y);
            let gb = crate::hipack::grow(b, 100.0);
            last || each.iter().any(|c| c.meets_rect(b)) || !to_build.iter().any(|e| e[0] <= gb[2] && e[2] >= gb[0] && e[1] <= gb[3] && e[3] >= gb[1])
        };
        // (Listed after them, not held back: while one waits out a failure, the others publish.)
        match prune_units(cov, date, m, &units) {
            Some(w) => work.push(w),
            None => match roads_chain_drawing(date, &m_then, done, inputs, Some(reach), &keep) {
                Some(w) => work.push(w),
                None => work.extend(catalog_work(&m_then, done, inputs, &ready, rounds.held)),
            },
        }
        // (Not while the regions can't be read: its catalog waits for them.)
        ends = work.len() == before && inputs.get("regions").map(String::as_str) != Some("?");
        round_left = remaining(done, |d| roads_chain_drawing(date, &m_then, d, inputs, Some(reach), &keep).or_else(|| catalog_work(&m_then, d, inputs, &ready, rounds.held)));
        // The regions done meanwhile: their slope and tree cover; then the regions' terrain and
        // units: a helper's, and this Mac's while the round's work is another's or waits out a
        // failure.
        let (prep, rest): (Vec<_>, Vec<_>) = slope.into_iter().partition(slope_waits);
        push(&mut work, "slope", prep);
        slope = rest;
        let (prep, rest): (Vec<_>, Vec<_>) = trees.into_iter().partition(trees_waits);
        push(&mut work, "trees", prep);
        trees = rest;
        let (prep, rest): (Vec<_>, Vec<_>) = trees_lo.into_iter().partition(trees_waits);
        push(&mut work, "trees-lo", prep);
        trees_lo = rest;
    } else {
        let (prep, rest): (Vec<_>, Vec<_>) = slope.into_iter().partition(slope_waits);
        push(&mut work, "slope", prep);
        slope = rest;
        let (prep, rest): (Vec<_>, Vec<_>) = trees.into_iter().partition(trees_waits);
        push(&mut work, "trees", prep);
        trees = rest;
        let (prep, rest): (Vec<_>, Vec<_>) = trees_lo.into_iter().partition(trees_waits);
        push(&mut work, "trees-lo", prep);
        trees_lo = rest;
    }
    work.extend(by_region);
    push(&mut work, "slope", slope);
    push(&mut work, "trees", trees);
    push(&mut work, "trees-lo", trees_lo);
    work.extend(chains(last_now));
    work.extend(bld);
    // Last of all, in idle time: the mids of current pieces that have none (expected the same).
    push(&mut work, "trees", backfill.clone());
    Plan { work, ready, publish_waits, regions: lefts, begins: None, ends, round_left, unknown, backfill }
}

/// Where a unit comes in a run of units: by the 10° square its tile's centre is in (column, then
/// row from the north), then by tile.
fn spatial_order(u: Unit) -> (i32, i32, u32, u32) {
    let b = crate::hipack::tile_bounds(u.z, u.x, u.y);
    let (lon, lat) = ((b[0] as f64 + b[2] as f64) / 2.0 * 1e-7, (b[1] as f64 + b[3] as f64) / 2.0 * 1e-7);
    ((lon / 10.0).floor() as i32, -(lat / 10.0).floor() as i32, u.x, u.y)
}

/// The rail service's chain (crate::rail), its first stale step (`rail_next`).
fn rail_chain(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>) -> Option<Work> {
    rail_next(cov, date, m, done, inputs).flatten()
}

/// The rail service's chain (crate::rail): Some(its first stale step, or None when it's done), or
/// None while it can't go on: before the rail sources are seeded (scenic-build rail-seed), while
/// inputs/keys.env can't be read (`inputs` "keys" "?"), and, rail-feeds done, without the feeds'
/// list or the pass's rail set.
/// - rail-feeds reads what decides which feeds there are: the catalogue, the coverage, the pass's
///   outlines (the countries it's in) and which of the keys the feeds use (crate::rail::FEED_KEYS)
///   inputs/keys.env holds (`inputs` "keys": their names, never their values). Not what it writes
///   (the feeds' list, checks and zips), so it doesn't run again for its own sake.
/// - rail reads the feeds' list (each feed's zip by content name, and its day), the MTR's pairs, the
///   pass's rail set and the coverage.
fn rail_next(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>) -> Option<Option<Work>> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let catalogue = m.get(crate::rail::CATALOGUE)?;
    let keys = inputs.get("keys").map(String::as_str).unwrap_or("");
    if keys == "?" {
        return None;
    }
    let keys: Vec<&str> = keys.split(',').filter(|k| crate::rail::FEED_KEYS.contains(k)).collect();
    let cover = coverage_all(cov);
    let k = h(&[&format!("rail-feeds {RAIL_FEEDS_V}"), catalogue, get(&format!("sources/osm/{date}/outlines")), &cover, &format!("keys {}", keys.join(","))]);
    if done.lo.get("rail-feeds").map(String::as_str) != Some(k.as_str()) {
        return Some(Some(Work { step: "rail-feeds".into(), targets: vec![("rail-feeds".into(), k)] }));
    }
    let feeds = m.get(crate::rail::FEEDS)?;
    let set = m.get(&crate::osmpass::set_name(date, "rail"))?;
    let k = h(&[&format!("rail {RAIL_V}"), feeds, get(crate::rail::MTR_PAIRS), set, &cover]);
    Some((done.lo.get("rail").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "rail".into(), targets: vec![("rail".into(), k)] }))
}

/// pack(T)'s targets (the z6 tiles the built units' ways reach) and lo's (their z3 tiles), each with
/// its key, done or not. Both are keyed on what they read, the base packs and road values of the
/// units whose ways come near: for pack(T) within its 100 km halo, for lo inside its tile. A unit's
/// ways lie in its owned extent (`Reach::owned_extent`: long ways such as ferries reach far), or
/// without a reach within its tile + 20 km. An identical rebuild of a unit (same content names)
/// reruns neither.
fn pack_lo_targets(m: &BTreeMap<String, String>, reach: Option<&Reaches>) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let base_units: Vec<(Unit, String, [i32; 4])> = m
        .iter()
        .filter_map(|(l, c)| l.strip_prefix("base/").and_then(Unit::parse).map(|u| (u, format!("{c}+{}", get(&format!("global/roads/{}", u.dash()))))))
        .map(|(u, c)| {
            let ext = reach.and_then(|r| r.get(u)).map(|r| r.owned_extent(u)).unwrap_or_else(|| crate::reach::near_box(u));
            (u, c, ext)
        })
        .collect();
    let deg = |b: [i32; 4]| [b[0] as f64 * 1e-7, b[1] as f64 * 1e-7, b[2] as f64 * 1e-7, b[3] as f64 * 1e-7];
    let mut tiles: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (_, _, ext) in &base_units {
        tiles.extend(crate::stage::tiles_in(6, deg(*ext)));
    }
    // The units whose extent meets a tile's box grown by `km` (pack's halo, what it reads: 100 km;
    // its owners and lo's: none).
    let near = |z: u8, x: u32, y: u32, km: f64| -> Vec<String> {
        let gb = crate::hipack::grow(crate::hipack::tile_bounds(z, x, y), km);
        base_units.iter().filter(|(_, _, e)| e[0] <= gb[2] && e[2] >= gb[0] && e[1] <= gb[3] && e[3] >= gb[1]).map(|(_, c, _)| c.clone()).collect()
    };
    let key = |head: String, ins: Vec<String>| {
        let mut all = vec![head];
        all.extend(ins);
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        h(&refs)
    };
    let packs: Vec<(String, String)> = tiles.iter().map(|&(x, y)| (format!("6/{x}/{y}"), pack_key(key(format!("pack {PACK_V}"), near(6, x, y, 100.0)), key(format!("pack-owners {PACK_V}"), near(6, x, y, 0.0))))).collect();
    let qs: BTreeSet<(u32, u32)> = tiles.iter().map(|&(x, y)| (x >> 3, y >> 3)).collect();
    let lo = qs.into_iter().map(|(x, y)| (format!("3/{x}/{y}"), key(format!("lo {LO_V}"), near(3, x, y, 0.0)))).collect();
    (packs, lo)
}

/// What the coverage no longer builds (`units`: the units it does): the outputs of units it doesn't
/// (their base pack, road values and English), and the candidates and peaks of units out of the
/// candidates' set, as prune targets ("unit 6/x/y", "pois 6/x/y"). Catalogs then stop listing them
/// and GC frees them.
fn prune_units(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, units: &[(Unit, Option<String>)]) -> Option<Work> {
    let built: BTreeSet<String> = units.iter().map(|(u, _)| u.dash()).collect();
    let cands: BTreeSet<String> = pois_keys(cov, date, m).iter().map(|(u, _)| u.dash()).collect();
    let mut t: BTreeSet<String> = BTreeSet::new();
    for l in m.keys() {
        for (prefix, set, kind) in [("base/", &built, "unit"), ("global/roads/", &built, "unit"), ("global/roaden/", &built, "unit"), ("work/pois/", &cands, "pois"), ("work/peaks/", &cands, "pois")] {
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
        let hi = l.strip_prefix("hidata/").or_else(|| l.strip_prefix("layers/roads/hi/")).or_else(|| l.strip_prefix("layers/rails/hi/"));
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

/// The roads' chain after the units: the road → units index, pack(T), lo, rail stops and ferries,
/// the terrain and slope roots; its first stale step.
fn roads_chain(date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, reach: Option<&Reaches>) -> Option<Work> {
    roads_chain_drawing(date, m, done, inputs, reach, &|_, _| true)
}

/// `roads_chain`, drawing the map tiles (pack(T)) `keep` says, by their z6 tile, and those whose
/// owners changed (`owners_changed`); the rest wait.
fn roads_chain_drawing(date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, reach: Option<&Reaches>, keep: &dyn Fn(u32, u32) -> bool) -> Option<Work> {
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    // Map tiles no unit's ways reach any more (a region removed) leave the manifest.
    if let Some(w) = prune_tiles(m, reach) {
        return Some(w);
    }
    // The road → units index, once the units' road values are made.
    let roads: Vec<String> = m.range("global/roads/".to_string()..).take_while(|(l, _)| l.starts_with("global/roads/")).map(|(l, c)| format!("{l}={c}")).collect();
    if !roads.is_empty() {
        let mut ins = vec![format!("roadunits {ROADUNITS_V}")];
        ins.extend(roads);
        let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
        let k = h(&refs);
        if done.lo.get("roadunits").map(String::as_str) != Some(k.as_str()) {
            return Some(Work { step: "roadunits".into(), targets: vec![("roadunits".into(), k)] });
        }
    }

    let (packs, lo) = pack_lo_targets(m, reach);
    let packs: Vec<(String, String)> = packs
        .into_iter()
        .filter(|(t, k)| {
            let d = done.pack.get(t).map(String::as_str);
            !pack_fresh(d, k) && (owners_changed(d, k) || Unit::parse(t).is_some_and(|u| keep(u.x, u.y)))
        })
        .collect();
    if !packs.is_empty() {
        return Some(Work { step: "pack".into(), targets: packs });
    }
    let lo: Vec<(String, String)> = lo.into_iter().filter(|(t, k)| stale(&done.lo, t, k)).collect();
    if !lo.is_empty() {
        return Some(Work { step: "lo".into(), targets: lo });
    }

    // Rail stops near the built units, and ferries worldwide, from the pass's sets (once there are
    // units: before them there are no roads to ride to).
    let built: Vec<&str> = m.range("base/".to_string()..).take_while(|(l, _)| l.starts_with("base/")).map(|(l, _)| l.as_str()).collect();
    if !built.is_empty() {
        for (step, v, set, extra, per_unit) in [
            ("stations", STATIONS_V, "rail", String::new(), true),
            ("ferries", FERRIES_V, "ferries", inputs.get("ferries-freq").cloned().unwrap_or_default(), false),
        ] {
            let Some(set_c) = m.get(&crate::osmpass::set_name(date, set)) else { continue };
            let mut ins = vec![format!("{step} {v}"), set_c.clone(), extra];
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
    for (layer, step) in [("terrain", "terrain-root"), ("slope", "slope-root")] {
        let mut ins = vec![format!("{step} 1")];
        ins.extend(m.range(format!("layers/{layer}/lo/")..).take_while(|(l, _)| l.starts_with(&format!("layers/{layer}/lo/"))).map(|(_, c)| c.clone()));
        if ins.len() == 1 {
            continue;
        }
        let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
        let k = h(&refs);
        if done.lo.get(step).map(String::as_str) != Some(k.as_str()) {
            return Some(Work { step: step.into(), targets: vec![(step.to_string(), k)] });
        }
    }
    None
}

/// The items' facts and pageviews' key (network; only new items within a pass): the current units'
/// candidates (`current_pois`).
fn items_key(date: &str, pois_now: &[&str]) -> String {
    let mut ins = vec![format!("items {ITEMS_V}"), date.to_string()];
    ins.extend(pois_now.iter().map(|s| s.to_string()));
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    h(&refs)
}

/// The rest of the heritage chain's key (network), on the pass's heritage sites; None before
/// they're made (the marks then take today's).
fn heritage_key(cov: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Option<String> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    if !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources")) {
        return None;
    }
    let mut ins = vec![format!("heritage {HERITAGE_V}"), date.to_string()];
    for stem in ["heritage", "heritage-areas", "special", "indigenous", "heritage-sources"] {
        ins.push(get(&crate::heritage::base_logical(date, stem)).to_string());
    }
    for l in [crate::osmpass::set_name(date, "named"), crate::osmpass::set_name(date, "areas"), format!("sources/osm/{date}/filtered"), "sources/registers/legacy".into(), "sources/registers/legacy-seeds".into()] {
        ins.push(get(&l).to_string());
    }
    ins.push(coverage_all(cov));
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    Some(h(&refs))
}

/// The landmark points' key: every current unit's candidates and peaks (`units`: pois_keys), the
/// items' facts and pageviews, the heritage sites (the files markconv reads: the pass's or today's).
fn marks_key(date: &str, m: &BTreeMap<String, String>, units: &[(Unit, String)]) -> String {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let src = heritage_src(m, date);
    let mut ins = vec![format!("marks {MARKS_V}"), get(&format!("sources/items/{date}/facts")).to_string(), get(&format!("sources/items/{date}/views")).to_string()];
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
/// sites, and the built units (their hi tiles are where the units are); None when the heritage is
/// today's (no overlays made from it).
fn overlays_key(date: &str, m: &BTreeMap<String, String>) -> Option<String> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let src = heritage_src(m, date);
    if src == crate::markconv::LEGACY {
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
    tiles_in_wrapped(6, crate::stage::tile_box_grown(u.z, u.x, u.y, 30.0)).into_iter().map(|(x, y)| format!("3/{}/{}", x >> 3, y >> 3)).collect()
}

/// The landmarks' chain, a step at a time: candidates, peaks, the items' facts and pageviews, the
/// rest of the heritage chain, the landmark points, the area overlays; its first stale step (the
/// forecast's and the checklist's view of what's left: `landmarks_work` is what runs).
fn landmarks_chain(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    let units: Vec<(Unit, String)> = pois_keys(cov, date, m);
    // The candidates (once the pass's hiking-route ends exist).
    if !m.contains_key(&format!("work/trailends/{date}")) {
        return None;
    }
    let stale_pois: Vec<(String, String)> = units.iter().filter(|(u, k)| stale(&done.pois, &u.slash(), k)).map(|(u, k)| (u.slash(), k.clone())).collect();
    if !stale_pois.is_empty() {
        return Some(Work { step: "pois".into(), targets: stale_pois });
    }
    // Their peaks (once the pass's summits exist).
    if !m.contains_key(&format!("work/summits/{date}")) {
        return None;
    }
    let stale_peaks: Vec<(String, String)> = peaks_keys(cov, date, m).into_iter().filter(|(u, k)| stale(&done.peaks, &u.slash(), k)).map(|(u, k)| (u.slash(), k)).collect();
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
    if let Some(k) = heritage_key(cov, date, m).filter(|k| done.lo.get("heritage") != Some(k)) {
        return Some(Work { step: "heritage".into(), targets: vec![("heritage".into(), k)] });
    }
    let k = marks_key(date, m, &units);
    if done.lo.get("marks").map(String::as_str) != Some(k.as_str()) {
        return Some(Work { step: "marks".into(), targets: vec![("marks".into(), k)] });
    }
    overlays_key(date, m).filter(|k| done.lo.get("overlays") != Some(k)).map(|k| Work { step: "overlays".into(), targets: vec![("overlays".into(), k)] })
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
fn landmarks_work(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, terrain_left: &BTreeSet<String>, last: bool) -> Vec<Work> {
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    let mut out = Vec::new();
    let units: Vec<(Unit, String)> = pois_keys(cov, date, m);
    let ends = m.contains_key(&format!("work/trailends/{date}")) && trailends_work(date, m, done).is_none();
    let stale_pois: Vec<(String, String)> = if ends { units.iter().filter(|(u, k)| stale(&done.pois, &u.slash(), k)).map(|(u, k)| (u.slash(), k.clone())).collect() } else { Vec::new() };
    let pois_built = ends && stale_pois.is_empty();
    if !stale_pois.is_empty() {
        out.push(Work { step: "pois".into(), targets: stale_pois });
    }
    let mut peaks_built = false;
    if pois_built && m.contains_key(&format!("work/summits/{date}")) && summits_work(date, m, done).is_none() {
        let stale_peaks: Vec<(Unit, String)> = peaks_keys(cov, date, m).into_iter().filter(|(u, k)| stale(&done.peaks, &u.slash(), k)).collect();
        peaks_built = stale_peaks.is_empty();
        let ready: Vec<(String, String)> = stale_peaks.into_iter().filter(|(u, _)| peaks_terrain(*u).is_disjoint(terrain_left)).map(|(u, k)| (u.slash(), k)).collect();
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
    // (Not before the heritage sites are made as the coverage wants them; with none for the pass
    // and none to make, the marks take today's.)
    let sites_due = heritage_sites_work(cov, date, m, done).is_some();
    let heritage_built = match heritage_key(cov, date, m) {
        _ if sites_due => false,
        Some(k) if done.lo.get("heritage") != Some(&k) => {
            out.push(Work { step: "heritage".into(), targets: vec![("heritage".into(), k)] });
            false
        }
        _ => true,
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

/// bld-fetch's key (network): the release and the whole coverage. Not the footers it reads and
/// writes (`footers.json.gz`): a release's files never change, so the release names them, and a key
/// on what the job writes would run it a second time for its own sake.
fn bld_fetch_key(cov: &Coverage) -> String {
    h(&[&format!("bld-fetch {BLD_FETCH_V}"), crate::buildtiles::RELEASE, &coverage_all(cov)])
}

/// Whether the release's sources are here (`inputs` "bld-release", crate::bld::sources::digests:
/// the release when its indexes read, "" before anything was downloaded); None when they can't be
/// read now ("?"), or weren't asked about (no entry): no 3D buildings' work then.
fn bld_sources(inputs: &BTreeMap<String, String>) -> Option<bool> {
    match inputs.get("bld-release").map(String::as_str) {
        None | Some("?") => None,
        Some(r) => Some(r == crate::buildtiles::RELEASE),
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
    /// the content names of T's and its 8 neighbours' normalized files ("-" none), and the coverage's
    /// shapes over T grown by 1 km in the recipes' order with their countries
    /// (`Coverage::shapes_key`: which shape a building is in sets its country's fits).
    pub tiles: Vec<(String, String)>,
}

/// The 3D buildings' targets for the coverage `cov`, the manifest `m` and the sources (`inputs`).
pub fn bld_targets(cov: &Coverage, m: &BTreeMap<String, String>, inputs: &BTreeMap<String, String>) -> BldTargets {
    use crate::bld::{work_logical, BLDPREP_V, BUILDINGS_V};
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let sources = bld_sources(inputs) == Some(true);
    let mut out = BldTargets::default();
    for (x, y, shapes) in bld_tiles(cov).iter() {
        let t = format!("6/{x}/{y}");
        if let Some(d) = inputs.get(&format!("bldprep {t}")).filter(|_| sources) {
            out.prep.push((t.clone(), h(&[&format!("bldprep {BLDPREP_V}"), crate::buildtiles::RELEASE, d])));
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
            let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
            out.tiles.push((t, h(&refs)));
        }
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
    (-1i64..=1).flat_map(|dy| (-1i64..=1).map(move |dx| (dx, dy))).map(|(dx, dy)| (c(x, dx), c(y, dy))).map(|(a, b)| if a.is_some() && b.is_some() { (a, b) } else { (None, None) }).collect()
}

/// The normalized files and tiles no target has any more (the coverage shrank): prune targets
/// ("bldprep 6/x/y", "bldtiles 6/x/y"). The files only while the sources read (an index missing
/// would make every tile seem to read nothing).
fn prune_bld(m: &BTreeMap<String, String>, tt: &BldTargets, sources: bool) -> Option<Work> {
    let prep: BTreeSet<String> = tt.prep.iter().map(|t| t.0.replace('/', "-")).collect();
    let tiles: BTreeSet<String> = tt.tiles.iter().map(|t| t.0.replace('/', "-")).collect();
    let mut t: BTreeSet<String> = BTreeSet::new();
    let pack = format!("layers/{}/hi/", crate::bld::LAYER);
    for (prefix, keep, kind) in [("work/bld/", &prep, "bldprep"), (pack.as_str(), &tiles, "bldtiles")] {
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
pub fn bld_work(cov: &Coverage, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, rank: &dyn Fn(&str) -> BldRank) -> Vec<Work> {
    let Some(sources) = bld_sources(inputs) else { return Vec::new() };
    let mut out = Vec::new();
    let k = bld_fetch_key(cov);
    if done.lo.get("bld-fetch") != Some(&k) {
        out.push(Work { step: "bld-fetch".into(), targets: vec![("bld-fetch".into(), k)] });
    }
    let tt = bld_targets(cov, m, inputs);
    out.extend(prune_bld(m, &tt, sources));
    let mut prep: Vec<(String, String)> = tt.prep.iter().filter(|(t, k)| done.bldprep.get(t) != Some(k)).cloned().collect();
    let unprepared: BTreeSet<&str> = prep.iter().map(|t| t.0.as_str()).collect();
    let ready = |t: &str| {
        Unit::parse(t).is_some_and(|u| bld_around(u.x, u.y).into_iter().all(|n| match n {
            (Some(x), Some(y)) => !unprepared.contains(format!("6/{x}/{y}").as_str()),
            _ => true,
        }))
    };
    // (None before anything's downloaded: every tile would come out empty.)
    let mut tiles: Vec<(String, String)> = tt.tiles.iter().filter(|(t, k)| sources && done.bldtiles.get(t) != Some(k) && ready(t)).cloned().collect();
    prep.sort_by_cached_key(|t| rank(&t.0));
    tiles.sort_by_cached_key(|t| rank(&t.0));
    for (step, targets) in [("bldprep", prep), ("bldtiles", tiles)] {
        if !targets.is_empty() {
            out.push(Work { step: step.into(), targets });
        }
    }
    out
}

/// The 3D buildings' chain's first stale step, as if each before it succeeded (`remaining`: the
/// checklist's and the forecast's view): the fetch, every stale `bldprep`, every stale `bldtiles`.
fn bld_next(cov: &Coverage, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>) -> Option<Work> {
    bld_sources(inputs)?;
    let k = bld_fetch_key(cov);
    if done.lo.get("bld-fetch") != Some(&k) {
        return Some(Work { step: "bld-fetch".into(), targets: vec![("bld-fetch".into(), k)] });
    }
    let tt = bld_targets(cov, m, inputs);
    let prep: Vec<(String, String)> = tt.prep.into_iter().filter(|(t, k)| done.bldprep.get(t) != Some(k)).collect();
    if !prep.is_empty() {
        return Some(Work { step: "bldprep".into(), targets: prep });
    }
    let tiles: Vec<(String, String)> = tt.tiles.into_iter().filter(|(t, k)| done.bldtiles.get(t) != Some(k)).collect();
    (!tiles.is_empty()).then(|| Work { step: "bldtiles".into(), targets: tiles })
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

/// One line of the build's checklist (the status, the menu bar): a step to the end, with how much of
/// it is done: targets done of all (`total` None until an earlier step makes them known), or for a
/// group of single jobs how many are left (`left`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Step {
    pub what: String,
    /// The agent's steps it covers (a job's id starts with one: "unit 6/31/20").
    pub steps: Vec<String>,
    #[serde(default)]
    pub done: usize,
    #[serde(default)]
    pub total: Option<usize>,
    #[serde(default)]
    pub left: Option<usize>,
    #[serde(default)]
    pub unit: String,
    /// The jobs still to do, by name, in the order they'll run (a step's areas counted together:
    /// "Measuring the peaks' prominence and isolation: 178 areas"); the first is the one under way.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// While it has work left and isn't this Mac's job now: why (another Mac is on it, it waits
    /// for the home network or out a failure, for the steps above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Which of its jobs a helper may do (crate::agent::claims::SHARED, `mark_shared`): "all", or
    /// the parts by name ("candidates and peaks"); none, the build Mac's alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<String>,
}

impl Step {
    pub fn finished(&self) -> bool {
        self.left == Some(0) || self.total.is_some_and(|t| self.done >= t && self.left.is_none())
    }
}

/// A step's name, as its jobs are titled, saying what it's doing (the agent adds a job's areas:
/// "Drawing the map tiles (3 areas)").
pub fn label(step: &str) -> &'static str {
    match step {
        "terrain" => "Building the regions' terrain",
        "slope" => "Working out the regions' slope",
        "trees" => "Mapping the tree cover",
        "trees-lo" => "Assembling the zoomed-out tree cover",
        "unit" => "Building the roads, their elevations and scenery",
        "pack" => "Drawing the map tiles",
        "lo" => "Drawing the zoomed-out map tiles",
        "pois" => "Finding the landmark candidates",
        "peaks" => "Measuring the peaks' prominence and isolation",
        "items" => "Fetching the landmarks' Wikidata facts and Wikipedia pageviews",
        "heritage-sites" => "Finding the regions' heritage sites and designated areas",
        "heritage" => "Adding details, fame and outlines to the heritage sites",
        "marks" => "Ranking the landmarks and drawing them on the map",
        "overlays" => "Drawing the area overlays",
        "roadunits" => "Working out which areas each road crosses",
        "stations" => "Placing the rail stops near the regions",
        "ferries" => "Mapping the world's ferries",
        "rail-feeds" => "Fetching the regions' rail timetables",
        "rail" => "Counting trains a day on the regions' rail",
        "bld-fetch" => "Fetching the 3D buildings' sources",
        "bldprep" => "Reading the regions' buildings",
        "bldtiles" => "Raising the 3D buildings",
        "terrain-root" | "slope-root" => "Building the world-level terrain and slope",
        "prune" => "Removing what the regions no longer cover",
        _ => "Publishing the new map data",
    }
}

/// The jobs `works` would run, by name, a step's runs together, its areas counted.
fn next_of(works: &[Work]) -> Vec<String> {
    let mut runs: Vec<(&str, usize)> = Vec::new();
    for w in works {
        match runs.last_mut() {
            Some((s, n)) if *s == w.step => *n += w.targets.len(),
            _ => runs.push((&w.step, w.targets.len())),
        }
    }
    runs.into_iter()
        .map(|(s, n)| match s {
            "pois" | "peaks" | "terrain" | "slope" | "unit" | "pack" | "trees-lo" if n > 1 => format!("{}: {n} areas", label(s)),
            "trees" | "bldprep" | "bldtiles" if n > 1 => format!("{}: {n} tiles", label(s)),
            _ => label(s).to_string(),
        })
        .collect()
}

// The checklist's lines, saying what each does.
pub const SITES: &str = "Finding the heritage sites and designated areas";
pub const TERRAIN: &str = "Building the terrain";
pub const SLOPE: &str = "Working out the slope";
pub const TREES: &str = "Mapping the tree cover";
pub const UNITS: &str = "Building the roads, elevations and scenery";
pub const TILES: &str = "Drawing the map tiles";
pub const ROADS: &str = "Indexing the roads; placing rail stops and ferries; building world terrain";
pub const TRAINS: &str = "Counting trains a day";
pub const LANDMARKS: &str = "Choosing and drawing the landmarks";
pub const BUILDINGS: &str = "Raising the 3D buildings";
pub const PUBLISH: &str = "Publishing the new map data";

/// Marks each step whose jobs a helper may do: all of them, or which.
pub fn mark_shared(steps: &mut [Step]) {
    fn noun(s: &str) -> &str {
        match s {
            "pois" => "candidates",
            "unit" => "areas",
            // (Tree cover's pieces, not its assemblies.)
            "trees" => "tiles",
            "bldprep" => "sources read",
            "bldtiles" => "tiles",
            other => other,
        }
    }
    for st in steps {
        let shared: Vec<&str> = st.steps.iter().map(String::as_str).filter(|s| super::claims::SHARED.contains(s)).collect();
        st.shared = match shared.len() {
            0 => None,
            n if n == st.steps.len() => Some("all".into()),
            _ => Some(shared.iter().map(|s| noun(s)).collect::<Vec<_>>().join(" and ")),
        };
    }
}

/// The regions' steps (build::checklist's lines), for before there's a pass to size them by.
pub fn checklist_to_come() -> Vec<Step> {
    [
        (SITES, &["heritage-sites"][..]),
        (TERRAIN, &["terrain"]),
        (SLOPE, &["slope"]),
        (TREES, &["trees", "trees-lo"]),
        (UNITS, &["unit"]),
        (TILES, &["pack", "lo"]),
        (ROADS, &["prune", "roadunits", "stations", "ferries", "terrain-root", "slope-root"]),
        (TRAINS, &["rail-feeds", "rail"]),
        (LANDMARKS, &["pois", "peaks", "items", "heritage", "marks", "overlays"]),
        (BUILDINGS, &["bld-fetch", "bldprep", "bldtiles"]),
        (PUBLISH, &["catalog", "catalog-held"]),
    ]
    .iter()
    .map(|(what, steps)| Step { what: what.to_string(), steps: steps.iter().map(|s| s.to_string()).collect(), ..Default::default() })
    .collect()
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
pub fn chains_left(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, reach: Option<&Reaches>) -> [Vec<Work>; 4] {
    [
        remaining(done, |d| roads_chain(date, m, d, inputs, reach)),
        remaining(done, |d| rail_chain(cov, date, m, d, inputs)),
        remaining(done, |d| landmarks_chain(cov, date, m, d)),
        remaining(done, |d| bld_next(cov, m, d, inputs)),
    ]
}

/// The regions' build to the end, step by step (the pass's own steps are the agent's): the
/// heritage sites, terrain, slope, tree cover, the areas, the map tiles, the road index, rail stops
/// and ferries, trains a day, the landmarks, the 3D buildings, publishing.
/// `held`: the catalog is held for review (inputs/hold-catalog): publishing is its held copy.
/// `ready`: the regions a catalog would record as built now (`Plan::ready`).
#[allow(clippy::too_many_arguments)]
pub fn checklist(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, held: bool, reach: Option<&Reaches>, ready: &[String], tiles: &TerrainTiles) -> Vec<Step> {
    let count = |all: &[(String, String)], map: &BTreeMap<String, String>| all.iter().filter(|(t, k)| map.get(t) == Some(k)).count();
    let per = |what: &str, steps: &[&str], all: &[(String, String)], map: &BTreeMap<String, String>, unit: &str, known: bool| Step {
        what: what.into(),
        steps: steps.iter().map(|s| s.to_string()).collect(),
        done: count(all, map),
        total: known.then_some(all.len()),
        unit: unit.into(),
        ..Default::default()
    };
    let group = |what: &str, steps: &[&str], left: Option<usize>| Step { what: what.into(), steps: steps.iter().map(|s| s.to_string()).collect(), left, ..Default::default() };
    let mut out = Vec::new();
    let (terrain, slope) = terrain_slope_targets(cov, m);
    let sites_left = heritage_sites_work(cov, date, m, done).is_some() || !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources"));
    let mut sites = group(SITES, &["heritage-sites"], Some(sites_left as usize));
    if sites_left {
        sites.next = vec![label("heritage-sites").into()];
    }
    out.push(sites);
    out.push(per(TERRAIN, &["terrain"], &terrain, &done.terrain, "areas", true));
    out.push(per(SLOPE, &["slope"], &slope, &done.slope, "areas", true));
    // (Tree cover's pieces and assemblies together, as the map tiles' line counts its two steps.)
    let tt = crate::treepacks::targets(cov, m);
    let pieces: Vec<(String, String)> = tt.pieces.iter().map(|(t, k, _)| (t.clone(), k.clone())).collect();
    let assemblies: Vec<(String, String)> = tt.lo.iter().map(|(q, k, _)| (q.clone(), k.clone())).collect();
    let mut trees = per(TREES, &["trees", "trees-lo"], &pieces, &done.trees, "tiles", true);
    trees.done += count(&assemblies, &done.trees_lo);
    trees.total = trees.total.map(|t| t + assemblies.len());
    out.push(trees);
    let pieces = m.keys().any(|l| l.starts_with(&format!("sources/osm/{date}/pieces/")));
    let units = unit_keys(cov, date, m, reach, inputs, tiles);
    // (One whose key can't be worked out now isn't done.)
    let known: Vec<(String, String)> = units.iter().filter_map(|(u, k)| Some((u.slash(), k.clone()?))).collect();
    let mut line = per(UNITS, &["unit"], &known, &done.unit, "areas", pieces && reach.is_some());
    line.total = line.total.map(|_| units.len());
    out.push(line);
    let (packs, lo) = pack_lo_targets(m, reach);
    let built = m.keys().any(|l| l.starts_with("base/"));
    let mut tiles = per(TILES, &["pack", "lo"], &packs, &done.pack, "tiles", built);
    tiles.done = packs.iter().filter(|(t, k)| pack_fresh(done.pack.get(t).map(String::as_str), k)).count() + count(&lo, &done.lo);
    tiles.total = tiles.total.map(|t| t + lo.len());
    out.push(tiles);
    let roads: Vec<Work> = remaining(done, |d| roads_chain(date, m, d, inputs, reach)).into_iter().filter(|w| !matches!(w.step.as_str(), "pack" | "lo")).collect();
    let mut road_steps = group(ROADS, &["prune", "roadunits", "stations", "ferries", "terrain-root", "slope-root"], built.then_some(roads.len()));
    road_steps.next = next_of(&roads);
    out.push(road_steps);
    // (A run of rail-feeds is followed by rail, whose key reads what it writes. Unknown while the
    // chain can't go on: before the rail sources are seeded, while inputs/keys.env can't be read,
    // without the feeds' list or the pass's rail set.)
    let rail = rail_next(cov, date, m, done, inputs);
    let mut trains = group(TRAINS, &["rail-feeds", "rail"], rail.as_ref().map(|w| w.as_ref().map_or(0, |w| if w.step == "rail-feeds" { 2 } else { 1 })));
    if let Some(Some(w)) = &rail {
        trains.next = if w.step == "rail-feeds" { vec![label("rail-feeds").into(), label("rail").into()] } else { vec![label("rail").into()] };
    }
    out.push(trains);
    let landmarks = remaining(done, |d| landmarks_chain(cov, date, m, d));
    // (Steps left, each named in `next` with its areas: not a peaks batch counted as 12 jobs.)
    let mut marks = group(LANDMARKS, &["pois", "peaks", "items", "heritage", "marks", "overlays"], pieces.then_some(landmarks.len()));
    marks.next = next_of(&landmarks);
    out.push(marks);
    // (Its tiles' normalized files and tiles together; known once the sources' indexes read.)
    let bt = bld_targets(cov, m, inputs);
    let mut bld = per(BUILDINGS, &["bld-fetch", "bldprep", "bldtiles"], &bt.prep, &done.bldprep, "tiles", bld_sources(inputs) == Some(true));
    bld.done += count(&bt.tiles, &done.bldtiles);
    bld.total = bld.total.map(|t| t + bt.tiles.len());
    bld.next = next_of(&remaining(done, |d| bld_next(cov, m, d, inputs)));
    out.push(bld);
    let key = catalog_key(m, inputs, ready);
    let publish_left = if held { done.catalog_held.as_deref() != Some(key.as_str()) } else { done.catalog.as_deref() != Some(key.as_str()) } as usize;
    // (Its one job is the line itself: no `next`.)
    out.push(group(PUBLISH, &["catalog", "catalog-held"], built.then_some(publish_left)));
    out
}

/// A catalog when what it would list or record has changed since the last one (not while the
/// regions can't be read: `inputs` "regions" "?"). `ready`: the regions it records as built
/// (`Plan::ready`).
/// The catalog, when the last (`held`: the last held one) has other files or regions than one made
/// now would; none while the regions can't be read.
fn catalog_work(m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, ready: &[String], held: bool) -> Option<Work> {
    if inputs.get("regions").map(String::as_str) == Some("?") {
        return None;
    }
    let k = catalog_key(m, inputs, ready);
    let last = if held { &done.catalog_held } else { &done.catalog };
    (last.as_deref() != Some(k.as_str())).then(|| Work { step: "catalog".into(), targets: vec![("catalog".into(), k)] })
}

/// What a catalog would list and record, hashed: the served files' logical and content names, the
/// regions (`inputs` "regions": their recipes and outline files), so a region renamed, or drawn
/// inside another, gets a catalog that records it, and which of them it records as built (`ready`).
pub fn catalog_key(m: &BTreeMap<String, String>, inputs: &BTreeMap<String, String>, ready: &[String]) -> String {
    let mut served: Vec<String> = m
        .iter()
        .filter(|(l, _)| {
            ["layers/", "base/", "hidata/", "markdata/", "ovdata/", "global/"].iter().any(|p| l.starts_with(p)) || l.ends_with("/outlines")
        })
        .map(|(l, c)| format!("{l}={c}"))
        .collect();
    served.push(format!("regions {}", inputs.get("regions").map(String::as_str).unwrap_or("-")));
    served.push(format!("ready {}", ready.join(",")));
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
    fn plan(c: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>) -> Vec<Work> {
        plan_with(c, date, m, done, inputs, &BTreeMap::new(), None).work
    }
    fn plan_with(c: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, on_map: &BTreeMap<String, bool>, since_publish: Option<u64>) -> Plan {
        let each = c.by_region();
        super::plan(c, date, m, done, inputs, Some(&reach()), &tiles_for(m), Rounds { each: &each, on_map, since_last: since_publish, current: None, held: false })
    }
    fn unit_keys(c: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
        ukeys(c, date, m, Some(&reach()), &BTreeMap::new())
    }
    fn checklist(c: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, held: bool) -> Vec<Step> {
        let ready = plan_with(c, date, m, done, inputs, &BTreeMap::new(), None).ready;
        super::checklist(c, date, m, done, inputs, held, Some(&reach()), &ready, &tiles_for(m))
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
        t
    }

    /// `unit_keys` with `tiles_for(m)` (every unit's key known).
    pub(crate) fn ukeys(c: &Coverage, date: &str, m: &BTreeMap<String, String>, reach: Option<&Reaches>, digests: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
        super::unit_keys(c, date, m, reach, digests, &tiles_for(m)).into_iter().map(|(u, k)| (u, k.expect("its key"))).collect()
    }

    fn cov() -> Coverage {
        let d = tempfile::tempdir().unwrap();
        Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:-21.9,64.13,20".into()] }], None, d.path()).unwrap()
    }

    /// The heritage-sites job's inputs (the pass's areas set, the registers' snapshot).
    /// What the units wait for besides terrain and slope: the heritage-sites job's inputs, and the
    /// release's roadside buildings.
    pub(crate) fn unit_inputs(m: &mut BTreeMap<String, String>, date: &str) {
        m.insert(crate::osmpass::set_name(date, "areas"), format!("sources/osm/{date}/sets/areas.1212121212121212.osm.pbf"));
        m.insert("sources/registers/legacy".into(), "sources/registers/legacy.3434343434343434.tar.zst".into());
        m.insert(crate::buildtiles::index_logical(), format!("{}.6767676767676767.json", crate::buildtiles::index_logical()));
    }

    /// The heritage-sites job done: its key recorded, its outputs in the manifest.
    pub(crate) fn heritage_done(m: &mut BTreeMap<String, String>, done: &mut Keys, date: &str, w: &Work) {
        assert_eq!(w.step, "heritage-sites");
        done.record(&w.step, &w.targets);
        m.insert(crate::heritage::base_logical(date, "heritage-sources"), format!("work/heritage/{date}/base/heritage-sources.5656565656565656.json"));
    }

    /// The rest of the heritage chain done too (it runs beside the regions' work from the start:
    /// the tests of the regions' order leave it out).
    pub(crate) fn heritage_chain_done(c: &Coverage, m: &BTreeMap<String, String>, done: &mut Keys, date: &str) {
        let k = heritage_key(c, date, m).unwrap();
        done.record("heritage", &[("heritage".to_string(), k)]);
    }

    /// Work done as its job does it: its targets recorded, and a tree cover piece's mid in the
    /// manifest (named by its key), or dropped for one of "none".
    pub(crate) fn did(m: &mut BTreeMap<String, String>, done: &mut Keys, w: &Work) {
        done.record(&w.step, &w.targets);
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
        assert_eq!(w[0].step, "terrain");
        assert_eq!(w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2"]);
        done.record("terrain", &w[0].targets);
        // The terrain job's outputs.
        m.insert("layers/terrain/lo/3-3-2".into(), "layers/terrain/lo/3-3-2.1111111111111111.pack".into());
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.2222222222222222.pack".into());
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "slope");
        done.record("slope", &w[0].targets);
        // The tree cover of the coverage's z6 tiles (pieces), then its z3 tile's assembly from their
        // mids, before the units.
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!((w[0].step.as_str(), w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>()), ("trees", vec!["6/28/16", "6/28/17"]));
        assert!(!w.iter().any(|x| x.step == "trees-lo"), "its pieces first");
        did(&mut m, &mut done, &w[0]);
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!((w[0].step.as_str(), w[0].targets[0].0.as_str()), ("trees-lo", "3/3/2"));
        did(&mut m, &mut done, &w[0]);
        // The root from the lo pack (no slope lo pack in this test: no slope root).
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "terrain-root");
        done.record("terrain-root", &w[0].targets);
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
        // New terrain content: slope again, then a catalog.
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.3333333333333333.pack".into());
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "slope");
    }

    /// Three regions in Iceland, all in z3 tile 3/3/2: a (Reykjavik: unit 6/28/16), b (Akureyri and
    /// Egilsstaðir: 6/29/16 and 6/30/16) and c (Vík: 6/31/16); their pieces and reaches, the units'
    /// inputs, and the heritage sites and the terrain done.
    fn three() -> (Coverage, Reaches, BTreeMap<String, String>, Keys) {
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, places: &[&str]| Recipe { id: id.into(), name: id.to_uppercase(), outline: places.iter().map(|p| format!("place:{p},20")).collect() };
        let c = Coverage::from_recipes(&[r("a", &["-21.9,64.13"]), r("b", &["-18.1,65.68", "-14.4,65.26"]), r("c", &["-19.0,63.42"])], None, d.path()).unwrap();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        for (u, b) in [("6/28/16", e7box(-22.0, 64.0, -21.7, 64.16)), ("6/29/16", e7box(-18.3, 65.6, -17.9, 65.8)), ("6/30/16", e7box(-14.6, 65.2, -14.2, 65.35)), ("6/31/16", e7box(-19.2, 63.35, -18.8, 63.5))] {
            reach.units.insert(u.into(), Reach { owned: Some(b), long: vec![] });
        }
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-16", "6-29-16", "6-30-16", "6-31-16"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        unit_inputs(&mut m, "d");
        let mut done = Keys::default();
        loop {
            let w = super::plan(&c, "d", &m, &done, &BTreeMap::new(), Some(&reach), &tiles_for(&m), Rounds { each: &c.by_region(), on_map: &BTreeMap::new(), since_last: None, current: None, held: false }).work;
            match w[0].step.as_str() {
                "heritage-sites" => heritage_done(&mut m, &mut done, "d", &w[0]),
                "terrain" => {
                    done.record("terrain", &w[0].targets);
                    m.insert("layers/terrain/lo/3-3-2".into(), "layers/terrain/lo/3-3-2.1111111111111111.pack".into());
                }
                _ => break,
            }
        }
        heritage_chain_done(&c, &m, &mut done, "d");
        (c, reach, m, done)
    }

    #[test]
    fn regions_are_built_one_at_a_time_those_the_map_lacks_first() {
        let (c, reach, m, done) = three();
        let each = c.by_region();
        let plan = |on_map: &BTreeMap<String, bool>| super::plan(&c, "d", &m, &done, &BTreeMap::new(), Some(&reach), &tiles_for(&m), Rounds { each: &each, on_map, since_last: None, current: None, held: false });
        let units = |p: &Plan| p.work.iter().filter(|w| w.step == "unit").flat_map(|w| w.targets.iter().map(|t| t.0.clone())).collect::<Vec<_>>();
        // Nothing on the map: the regions with the fewest units left first (a and c, one each: by
        // place), then b's two together; their slope and tree cover after them.
        let p = plan(&BTreeMap::new());
        assert_eq!(p.work.iter().map(|w| w.step.as_str()).collect::<Vec<_>>(), ["unit", "unit", "unit", "slope", "trees"]);
        assert_eq!(units(&p), ["6/28/16", "6/31/16", "6/29/16", "6/30/16"]);
        assert!(p.ready.is_empty());
        // a and c on the map already (as they are, or redrawn): b first, though it has more left.
        let on: BTreeMap<String, bool> = [("a".to_string(), true), ("c".to_string(), false)].into();
        assert_eq!(units(&plan(&on)), ["6/29/16", "6/30/16", "6/28/16", "6/31/16"]);
    }

    #[test]
    fn a_region_done_is_published_in_a_round_at_most_hourly() {
        let (c, reach, mut m, mut done) = three();
        let each = c.by_region();
        let plan = |m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>, since: Option<u64>| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), &tiles_for(m), Rounds { each: &each, on_map, since_last: since, current: None, held: false });
        // (A step's works one after another, as one: the plan lists a step's by region.)
        let steps = |p: &Plan| {
            let mut v: Vec<String> = p.work.iter().map(|w| w.step.clone()).collect();
            v.dedup();
            v
        };
        let key = |m: &BTreeMap<String, String>, u: &str| ukeys(&c, "d", m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        let build = |m: &mut BTreeMap<String, String>, done: &mut Keys, u: &str| {
            done.record("unit", &[(u.to_string(), key(m, u))]);
            let d = u.replace('/', "-");
            m.insert(format!("base/{d}"), format!("base/{d}.6666666666666666.base"));
            m.insert(format!("global/roads/{d}"), format!("global/roads/{d}.7777777777777777.roads"));
        };
        // Reykjavik's unit built: a is done, the map lacks it, nothing published yet: a round. Its
        // area's slope and tree cover first, then the map tiles, then a catalog; the units after.
        build(&mut m, &mut done, "6/28/16");
        let p = plan(&m, &done, &BTreeMap::new(), None);
        assert_eq!(steps(&p), ["slope", "trees", "roadunits", "unit"]);
        assert!(p.ready.is_empty(), "its area's slope and tree cover aren't made yet");
        done.record("slope", &p.work[0].targets);
        did(&mut m, &mut done, &p.work[1]);
        // Then its area's tree cover assembled from the pieces' mids.
        let p = plan(&m, &done, &BTreeMap::new(), None);
        assert_eq!(steps(&p)[0], "trees-lo");
        assert!(p.ready.is_empty(), "its area's tree cover isn't assembled yet");
        assert!(p.publish_waits.contains(&("trees-lo".to_string(), "3/3/2".to_string())));
        did(&mut m, &mut done, &p.work[0]);
        let mut p = plan(&m, &done, &BTreeMap::new(), None);
        assert_eq!(p.ready, ["a"]);
        while p.work[0].step != "catalog" {
            assert!(["roadunits", "pack", "lo", "stations", "terrain-root"].contains(&p.work[0].step.as_str()), "{:?}", steps(&p));
            done.record(&p.work[0].step, &p.work[0].targets);
            p = plan(&m, &done, &BTreeMap::new(), None);
        }
        assert_eq!(steps(&p), ["catalog", "unit"]);
        done.record("catalog", &p.work[0].targets);
        // Published: the units again, and no round while none is done that the map lacks.
        let on: BTreeMap<String, bool> = [("a".to_string(), true)].into();
        assert_eq!(steps(&plan(&m, &done, &on, Some(60))), ["unit"]);
        // c done ten minutes later: it waits for the hour; then a round, with a and c.
        build(&mut m, &mut done, "6/31/16");
        assert_eq!(steps(&plan(&m, &done, &on, Some(600))), ["unit"]);
        let p = plan(&m, &done, &on, Some(PUBLISH_EVERY_S));
        assert_eq!(p.ready, ["a", "c"]);
        assert_eq!(p.work[0].step, "roadunits");
        assert_eq!(p.work.last().unwrap().step, "unit");
        // The last unit: a round at once, whatever the hour, with every region.
        build(&mut m, &mut done, "6/29/16");
        build(&mut m, &mut done, "6/30/16");
        let p = plan(&m, &done, &on, Some(60));
        assert_eq!(p.ready, ["a", "b", "c"]);
        assert_eq!(p.work[0].step, "roadunits");
        assert!(!steps(&p).contains(&"unit".to_string()));
    }

    #[test]
    fn a_round_before_the_last_leaves_the_map_tiles_units_still_to_build_would_change() {
        let (c, mut reach, mut m, mut done) = three();
        // Reykjavik's unit has a long way (a ferry) east into the next z6 tile, which no region it
        // publishes meets, and where b's eastern unit (6/30/16) is still to build.
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -16.5, 64.16)), long: vec![] });
        let each = c.by_region();
        let plan = |m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), &tiles_for(m), Rounds { each: &each, on_map, since_last: None, current: None, held: false });
        let key = |m: &BTreeMap<String, String>, u: &str| ukeys(&c, "d", m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        let build = |m: &mut BTreeMap<String, String>, done: &mut Keys, u: &str| {
            done.record("unit", &[(u.to_string(), key(m, u))]);
            let d = u.replace('/', "-");
            m.insert(format!("base/{d}"), format!("base/{d}.6666666666666666.base"));
            m.insert(format!("global/roads/{d}"), format!("global/roads/{d}.7777777777777777.roads"));
        };
        // Through a round's work to its map tiles: what it draws.
        let drawn = |m: &BTreeMap<String, String>, done: &mut Keys, on_map: &BTreeMap<String, bool>| -> Vec<String> {
            let mut m = m.clone();
            loop {
                let p = plan(&m, done, on_map);
                let w = &p.work[0];
                match w.step.as_str() {
                    "pack" => return w.targets.iter().map(|t| t.0.clone()).collect(),
                    "slope" | "trees" | "trees-lo" | "roadunits" => did(&mut m, done, w),
                    s => panic!("{s} before the map tiles"),
                }
            }
        };
        // (The tiles by where they lie: Reykjavik's is 6/28/17, the ferry's east end in 6/29/17.)
        build(&mut m, &mut done, "6/28/16");
        let first = drawn(&m, &mut done, &BTreeMap::new());
        let all: Vec<String> = pack_lo_targets(&m, Some(&reach)).0.into_iter().map(|t| t.0).collect();
        let has = |v: &[String], t: &str| v.iter().any(|x| x == t);
        assert!(has(&first, "6/28/17") && has(&all, "6/29/17") && !has(&first, "6/29/17"), "{first:?} of {all:?}");
        // The last round draws every tile.
        for u in ["6/29/16", "6/30/16", "6/31/16"] {
            build(&mut m, &mut done, u);
        }
        let last = drawn(&m, &mut done, &[("a".to_string(), true)].into());
        assert!(has(&last, "6/29/17"), "{last:?}");
    }

    #[test]
    fn a_rounds_work_is_fixed_when_it_begins() {
        let (c, reach, mut m, mut done) = three();
        let each = c.by_region();
        let plan = |m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>, since: Option<u64>, current: Option<&Round>| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), &tiles_for(m), Rounds { each: &each, on_map, since_last: since, current, held: false });
        let key = |m: &BTreeMap<String, String>, u: &str| ukeys(&c, "d", m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        let build = |m: &mut BTreeMap<String, String>, done: &mut Keys, u: &str| {
            done.record("unit", &[(u.to_string(), key(m, u))]);
            let d = u.replace('/', "-");
            m.insert(format!("base/{d}"), format!("base/{d}.6666666666666666.base"));
            m.insert(format!("global/roads/{d}"), format!("global/roads/{d}.7777777777777777.roads"));
        };
        // a done, nothing published yet: a round begins, with a and the units as they are.
        build(&mut m, &mut done, "6/28/16");
        let p = plan(&m, &done, &BTreeMap::new(), None, None);
        let mut r = p.begins.clone().expect("a round begins");
        r.began = 1;
        assert_eq!(r.regions, ["a"]);
        assert!(r.units.contains_key("base/6-28-16") && !r.last);
        // Its work is already the round's; planned again with it under way, the same. (Its chain to
        // the end, for the forecast: the road index to the catalog.)
        assert_eq!(plan(&m, &done, &BTreeMap::new(), None, Some(&r)).work, p.work);
        assert!(plan(&m, &done, &BTreeMap::new(), None, Some(&r)).begins.is_none());
        let left: Vec<&str> = p.round_left.iter().map(|w| w.step.as_str()).collect();
        assert_eq!((left.first().copied(), left.last().copied()), (Some("roadunits"), Some("catalog")), "{left:?}");
        assert!(left.contains(&"pack"), "{left:?}");
        // c done meanwhile: it isn't in the round (its catalog records a alone), nor are its roads
        // in the round's road index, map tiles and catalog: the round goes on as it began.
        build(&mut m, &mut done, "6/31/16");
        let then = crate::out::units_as_of(&m, &r.units);
        assert!(!then.contains_key("base/6-31-16"));
        let mut steps = Vec::new();
        loop {
            let p = plan(&m, &done, &BTreeMap::new(), None, Some(&r));
            if p.ends {
                break;
            }
            let w = p.work[0].clone();
            assert!(["slope", "trees", "trees-lo", "roadunits", "pack", "lo", "stations", "terrain-root", "catalog"].contains(&w.step.as_str()), "{:?}", p.work);
            if w.step == "catalog" {
                assert_eq!(p.ready, ["a"]);
                assert_eq!(w.targets[0].1, catalog_key(&then, &BTreeMap::new(), &["a".to_string()]));
            }
            if w.step == "pack" {
                // (Keyed on the units as they were.)
                let (packs, _) = pack_lo_targets(&then, Some(&reach));
                assert!(w.targets.iter().all(|t| packs.contains(t)), "{:?}", w.targets);
            }
            did(&mut m, &mut done, &w);
            steps.push(w.step);
            assert!(steps.len() < 20, "{steps:?}");
        }
        assert_eq!(steps.iter().filter(|s| *s == "roadunits").count(), 1, "{steps:?}");
        assert_eq!(steps.last().map(String::as_str), Some("catalog"));
        // Over, and published (a on the map). c waits for the next round, an hour after this one
        // began: the regions' units meanwhile; no round's work.
        let on: BTreeMap<String, bool> = [("a".to_string(), true)].into();
        let p = plan(&m, &done, &on, Some(600), None);
        assert!(p.begins.is_none());
        assert!(p.work.iter().all(|w| !AS_OF_STEPS.contains(&w.step.as_str())), "{:?}", p.work);
        assert_eq!(p.work[0].step, "unit");
        let p = plan(&m, &done, &on, Some(PUBLISH_EVERY_S), None);
        assert_eq!(p.begins.map(|r| r.regions), Some(vec!["c".to_string()]));
        // (The roads index again: c's roads are new to it.)
        assert_eq!(p.work[0].step, "roadunits");
    }

    /// The tests' plan with a round under way, and a unit of theirs built (as `content`).
    fn rplan(c: &Coverage, reach: &Reaches, m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>, since: Option<u64>, current: Option<&Round>, held: bool) -> Plan {
        let each = c.by_region();
        super::plan(c, "d", m, done, &BTreeMap::new(), Some(reach), &tiles_for(m), Rounds { each: &each, on_map, since_last: since, current, held })
    }
    fn rbuild(c: &Coverage, reach: &Reaches, m: &mut BTreeMap<String, String>, done: &mut Keys, u: &str, content: &str) {
        let k = ukeys(c, "d", m, Some(reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        done.record("unit", &[(u.to_string(), k)]);
        let d = u.replace('/', "-");
        m.insert(format!("base/{d}"), format!("base/{d}.{content}.base"));
        m.insert(format!("global/roads/{d}"), format!("global/roads/{d}.{content}.roads"));
    }
    /// A round's work to its catalog, each step done as the plan lists it (`skip`: work that fails
    /// each time it runs, passed over as the agent passes over work waiting out a failure): the
    /// plan the catalog came in.
    fn through_its_catalog(c: &Coverage, reach: &Reaches, m: &mut BTreeMap<String, String>, done: &mut Keys, r: &Round, held: bool, skip: &dyn Fn(&Work) -> bool) -> Plan {
        for _ in 0..30 {
            let p = rplan(c, reach, m, done, &BTreeMap::new(), None, Some(r), held);
            let w = p.work.iter().find(|w| !skip(w) && (AS_OF_STEPS.contains(&w.step.as_str()) || w.step == "prune" || p.publish_waits.iter().any(|(s, t)| *s == w.step && w.targets.iter().any(|x| x.0 == *t)))).cloned().expect("the round's work");
            if w.step == "catalog" {
                done.record(if held { "catalog-held" } else { "catalog" }, &w.targets);
                return p;
            }
            did(m, done, &w);
        }
        panic!("no catalog");
    }

    #[test]
    fn a_round_ends_with_its_catalog_what_changed_meanwhile_and_failing_work_wait_for_the_next() {
        let (c, reach, mut m, mut done) = three();
        rbuild(&c, &reach, &mut m, &mut done, "6/28/16", "6666666666666666");
        let mut r = rplan(&c, &reach, &m, &done, &BTreeMap::new(), None, None, false).begins.expect("a round begins");
        r.began = 1;
        // a's slope fails each time it runs: the round's catalog goes out without a (the agent then
        // ends the round, its catalog made).
        let p = through_its_catalog(&c, &reach, &mut m, &mut done, &r, false, &|w| w.step == "slope");
        assert!(p.ready.is_empty());
        // Meanwhile c is done, and other layers changed (a unit's grids, a terrain area): with the
        // round over, nothing for an hour after it began, then a round with a and c.
        rbuild(&c, &reach, &mut m, &mut done, "6/31/16", "8888888888888888");
        m.insert("layers/grid-class/hi/6-31-16".into(), "layers/grid-class/hi/6-31-16.9999999999999999.pack".into());
        m.insert("layers/terrain/lo/3-2-2".into(), "layers/terrain/lo/3-2-2.2222222222222222.pack".into());
        let p = rplan(&c, &reach, &m, &done, &BTreeMap::new(), Some(600), None, false);
        assert!(p.begins.is_none() && p.work.iter().all(|w| !AS_OF_STEPS.contains(&w.step.as_str())), "{:?}", p.work);
        let p = rplan(&c, &reach, &m, &done, &BTreeMap::new(), Some(PUBLISH_EVERY_S), None, false);
        assert_eq!(p.begins.map(|r| r.regions), Some(vec!["a".to_string(), "c".to_string()]));
    }

    #[test]
    fn a_round_region_built_again_mid_round_goes_out_with_the_next() {
        let (c, reach, mut m, mut done) = three();
        rbuild(&c, &reach, &mut m, &mut done, "6/28/16", "6666666666666666");
        let mut r = rplan(&c, &reach, &m, &done, &BTreeMap::new(), None, None, false).begins.expect("a round begins");
        r.began = 1;
        // a's unit built again during the round (its outline redrawn, say): the round's catalog
        // doesn't record a as built, its copy of the unit being the old one; the next round does.
        rbuild(&c, &reach, &mut m, &mut done, "6/28/16", "aaaaaaaaaaaaaaaa");
        let p = through_its_catalog(&c, &reach, &mut m, &mut done, &r, false, &|_| false);
        assert!(!p.ready.contains(&"a".to_string()), "{:?}", p.ready);
        let p = rplan(&c, &reach, &m, &done, &BTreeMap::new(), Some(PUBLISH_EVERY_S), None, false);
        assert_eq!(p.begins.map(|r| r.regions), Some(vec!["a".to_string()]));
    }

    #[test]
    fn with_catalogs_held_a_round_weighs_its_catalog_against_the_last_held() {
        let (c, reach, mut m, mut done) = three();
        for u in ["6/28/16", "6/29/16", "6/30/16", "6/31/16"] {
            rbuild(&c, &reach, &mut m, &mut done, u, "6666666666666666");
        }
        let mut r = rplan(&c, &reach, &m, &done, &BTreeMap::new(), None, None, true).begins.expect("the last round begins");
        assert!(r.last);
        r.began = 1;
        through_its_catalog(&c, &reach, &mut m, &mut done, &r, true, &|_| false);
        // Its held catalog made, nothing new: no round begins again (none every plan).
        let on: BTreeMap<String, bool> = [("a".to_string(), true), ("b".to_string(), true), ("c".to_string(), true)].into();
        for since in [5, 25, 4000] {
            assert!(rplan(&c, &reach, &m, &done, &on, Some(since), None, true).begins.is_none());
        }
        // (Served, not held: the served one is weighed, and it's older.)
        assert!(rplan(&c, &reach, &m, &done, &on, Some(5), None, false).begins.is_some());
    }

    #[test]
    fn a_region_done_waits_for_its_round_with_its_slope_and_tree_cover_made() {
        let (c, reach, mut m, mut done) = three();
        let each = c.by_region();
        let k = ukeys(&c, "d", &m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == "6/28/16").unwrap().1;
        done.record("unit", &[("6/28/16".to_string(), k)]);
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        // A round began ten minutes ago: a's slope and tree cover first, then the units; no round.
        let p = super::plan(&c, "d", &m, &done, &BTreeMap::new(), Some(&reach), &tiles_for(&m), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: Some(600), current: None, held: false });
        assert!(p.begins.is_none() && p.publish_waits.is_empty());
        let steps: Vec<&str> = p.work.iter().map(|w| w.step.as_str()).collect();
        assert_eq!(&steps[..3], ["slope", "trees", "unit"]);
    }

    #[test]
    fn a_tile_whose_owners_changed_is_drawn_in_any_round() {
        let (c, mut reach, mut m, mut done) = three();
        // Reykjavik's ferry reaches 6/29/17, where b's eastern unit is still to build (as in
        // a_round_before_the_last_leaves_the_map_tiles_units_still_to_build_would_change).
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -16.5, 64.16)), long: vec![] });
        let each = c.by_region();
        let key = |m: &BTreeMap<String, String>, u: &str| ukeys(&c, "d", m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        done.record("unit", &[("6/28/16".to_string(), key(&m, "6/28/16"))]);
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        let packs = pack_lo_targets(&m, Some(&reach)).0;
        let now = packs.iter().find(|t| t.0 == "6/29/17").unwrap().1.clone();
        let (halo, owners) = now.split_once('.').unwrap();
        let drawn = |done: &mut Keys| -> Vec<String> {
            let mut m = m.clone();
            loop {
                let p = super::plan(&c, "d", &m, done, &BTreeMap::new(), Some(&reach), &tiles_for(&m), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: None, current: None, held: false });
                let w = &p.work[0];
                match w.step.as_str() {
                    "pack" => return w.targets.iter().map(|t| t.0.clone()).collect(),
                    "slope" | "trees" | "trees-lo" | "roadunits" => did(&mut m, done, w),
                    s => panic!("{s} before the map tiles"),
                }
            }
        };
        // Drawn before with other owners (6/28/16 rebuilt since): drawn again, though a unit to
        // build is near.
        done.pack.insert("6/29/17".into(), format!("{halo}.0000000000000000"));
        assert!(drawn(&mut done.clone()).contains(&"6/29/17".to_string()));
        // Its owners as they were, its halo not: it waits.
        done.pack.insert("6/29/17".into(), format!("0000000000000000.{owners}"));
        assert!(!drawn(&mut done.clone()).contains(&"6/29/17".to_string()));
    }

    #[test]
    fn tile_keys_from_before_owners_count_by_their_halo() {
        // (Drawn under a key with no owners' part: current while its halo is; owners changed when
        // it isn't.)
        assert!(pack_fresh(Some("aaaa.bbbb"), "aaaa.bbbb") && pack_fresh(Some("aaaa"), "aaaa.bbbb"));
        assert!(!pack_fresh(Some("cccc"), "aaaa.bbbb") && !pack_fresh(Some("aaaa.cccc"), "aaaa.bbbb") && !pack_fresh(None, "aaaa.bbbb"));
        assert!(!owners_changed(Some("aaaa"), "aaaa.bbbb") && owners_changed(Some("cccc"), "aaaa.bbbb"));
        assert!(!owners_changed(Some("cccc.bbbb"), "aaaa.bbbb") && owners_changed(Some("aaaa.cccc"), "aaaa.bbbb"));
        assert!(!owners_changed(None, "aaaa.bbbb"));
    }

    #[test]
    fn a_regions_terrain_comes_with_it_and_its_units_once_theirs_is_built() {
        // Two regions in two terrain areas: g (Nuuk: unit 6/22/17, area 3/2/2) and a (Reykjavik:
        // 6/28/16, 3/3/2); one unit each, g's first by place.
        let d = tempfile::tempdir().unwrap();
        let r = |id: &str, place: &str| Recipe { id: id.into(), name: id.to_uppercase(), outline: vec![format!("place:{place},20")] };
        let c = Coverage::from_recipes(&[r("a", "-21.9,64.13"), r("g", "-51.7,64.18")], None, d.path()).unwrap();
        let mut reach = Reaches { fmt: 1, date: "d".into(), ..Default::default() };
        reach.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![] });
        reach.units.insert("6/22/17".into(), Reach { owned: Some(e7box(-51.9, 64.1, -51.5, 64.3)), long: vec![] });
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-16", "6-22-17"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        unit_inputs(&mut m, "d");
        let each = c.by_region();
        let plan = |m: &BTreeMap<String, String>, done: &Keys| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), &tiles_for(m), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: None, current: None, held: false }).work;
        let mut done = Keys::default();
        let w = plan(&m, &done);
        heritage_done(&mut m, &mut done, "d", &w[0]);
        heritage_chain_done(&c, &m, &mut done, "d");
        // Each region's terrain area, g's first; neither's unit until its terrain is built.
        let w = plan(&m, &done);
        let line = |w: &[Work]| w.iter().map(|x| format!("{} {}", x.step, x.targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(","))).collect::<Vec<_>>();
        assert_eq!(line(&w), ["terrain 3/2/2", "terrain 3/3/2", "trees 6/22/16,6/22/17,6/28/16,6/28/17"]);
        // g's terrain built: g's unit, ahead of a's terrain (a helper takes a's from the far end).
        done.record("terrain", &[w[0].targets[0].clone()]);
        let w = plan(&m, &done);
        assert_eq!(line(&w), ["unit 6/22/17", "terrain 3/3/2", "slope 3/2/2", "trees 6/22/16,6/22/17,6/28/16,6/28/17"]);
    }

    #[test]
    fn units_whose_piece_meets_the_coverage() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        // Terrain, slope and tree cover's pieces done; then its assembly, from their mids.
        for w in [plan(&c, "d", &m, &done, &BTreeMap::new()), {
            let mut d2 = done.clone();
            d2.record("terrain", &plan(&c, "d", &m, &done, &BTreeMap::new())[0].targets);
            plan(&c, "d", &m, &d2, &BTreeMap::new())
        }] {
            for x in &w {
                did(&mut m, &mut done, x);
            }
        }
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
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
        let index = m.remove(&crate::buildtiles::index_logical()).unwrap();
        assert!(!plan(&c, "d", &m, &done, &BTreeMap::new()).iter().any(|w| w.step == "unit"));
        m.insert(crate::buildtiles::index_logical(), index);
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "unit");
        assert_eq!(w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["6/28/16"]);
        // Buildings made again: every unit again.
        let k = unit_keys(&c, "d", &m)[0].1.clone();
        let mut m2 = m.clone();
        m2.insert(crate::buildtiles::index_logical(), format!("{}.6868686868686868.json", crate::buildtiles::index_logical()));
        assert_ne!(unit_keys(&c, "d", &m2)[0].1, k);
        // A unit's key names the heritage slices near it, and only those.
        let key = |m: &BTreeMap<String, String>| unit_keys(&c, "d", m)[0].1.clone();
        let k0 = key(&m);
        m.insert(crate::heritage::pos_logical("d", 40, 20), "work/heritage/d/pos/6-40-20.7878787878787878.json".into());
        assert_eq!(key(&m), k0, "a slice far away");
        m.insert(crate::heritage::areas_logical("d", 28, 16), "work/heritage/d/areas/6-28-16.9090909090909090.jsonl".into());
        assert_ne!(key(&m), k0, "its own tile's areas");
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
        let r = Reach { owned: Some(e7box(-20.1, 64.95, -19.9, 65.05)), long: vec![LongWay { owned: true, ferry: false, verts: vec![[-190_000_000, 650_000_000], [-100_000_000, 650_000_000]] }] };
        let at = |z: u8, lon: f64, lat: f64| {
            let t = Unit::of_point(z, [(lon * 1e7) as i32, (lat * 1e7) as i32]);
            (z, t.x, t.y)
        };
        // Its area's lo pack, its tile's hi pack, and a hi pack past its tile + 30 km (6/30/16).
        let names = [("layers/terrain/lo/3-3-2", "1111111111111111"), ("layers/terrain/hi/6-28-16", "2222222222222222"), ("layers/terrain/hi/6-30-16", "3333333333333333")];
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
        assert!(tiles.iter().all(|t| t.0 == 11 || t.0 == 12), "{:?}", tiles.iter().filter(|t| t.0 != 11 && t.0 != 12).collect::<Vec<_>>());
        assert!(tiles.contains(&(12, at(12, -20.0, 65.0).1, at(12, -20.0, 65.0).2, packs[1][&at(12, -20.0, 65.0)])));
        // Kept with what it was made from: the same packs, the same; another pack (a new content
        // name) or another reach, worked out again.
        let k = super::unit_terrain(u, &r, &m, &t).unwrap();
        assert_eq!(k, d0);
        assert_ne!(super::unit_terrain(u, &near, &m, &t).unwrap(), k);
        let mut m2 = m.clone();
        m2.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.4444444444444444.pack".into());
        t.hold("layers/terrain/hi/6-28-16.4444444444444444.pack", packs[1].iter().filter(|(t, _)| t.0 != 12).map(|(&(z, x, y), &h)| (z, x, y, h)));
        assert_ne!(super::unit_terrain(u, &r, &m2, &t).unwrap(), k);
        assert_eq!(super::unit_terrain(u, &r, &m, &t).unwrap(), k);
        // A pack the manifest names whose index isn't held: unknown, never worked out without it.
        m2.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.5555555555555555.pack".into());
        assert!(super::unit_terrain(u, &r, &m2, &t).unwrap_err().0.starts_with("layers/terrain/hi/6-28-16.5555555555555555.pack"));
    }

    #[test]
    fn a_unit_whose_terrain_cant_be_read_now_waits() {
        let (c, reach, m, done) = three();
        let each = c.by_region();
        let plan = |t: &TerrainTiles| super::plan(&c, "d", &m, &done, &BTreeMap::new(), Some(&reach), t, Rounds { each: &each, on_map: &BTreeMap::new(), since_last: None, current: None, held: false });
        // The terrain's lo pack's index not read: no unit is built, nor counted as built, nor pruned.
        let p = plan(&TerrainTiles::new(None));
        assert!(!p.work.iter().any(|w| matches!(w.step.as_str(), "unit" | "prune")), "{:?}", p.work);
        assert_eq!(p.unknown, ["6/28/16", "6/29/16", "6/30/16", "6/31/16"]);
        assert!(p.ready.is_empty());
        // Read: they're built.
        let p = plan(&tiles_for(&m));
        assert!(p.unknown.is_empty());
        assert_eq!(p.work.iter().filter(|w| w.step == "unit").map(|w| w.targets.len()).sum::<usize>(), 4);
        let states = region_states(&c, &each, "d", &m, &done, Some(&reach), &BTreeMap::new(), &TerrainTiles::new(None));
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
        let w = |step: &str, n: usize| Work { step: step.into(), targets: (0..n).map(|i| (format!("6/{i}/1"), "k".into())).collect() };
        assert_eq!(next_of(&[w("peaks", 12), w("peaks", 166), w("items", 1), w("heritage", 1)]), [format!("{}: 178 areas", label("peaks")).as_str(), label("items"), label("heritage")]);
        assert_eq!(next_of(&[w("pois", 1), w("marks", 1)]), [label("pois"), label("marks")]);
    }

    #[test]
    fn checklist_counts_to_the_end() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let line = |l: &[Step], what: &str| l.iter().find(|s| s.what.starts_with(what)).cloned().unwrap();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        unit_inputs(&mut m, "d");
        let l = checklist(&c, "d", &m, &done, &BTreeMap::new(), false);
        assert_eq!(l.len(), 11);
        assert_eq!(line(&l, BUILDINGS).total, None, "no sources asked about: not known");
        assert_eq!(line(&l, TRAINS).left, None, "no rail sources: not known");
        assert_eq!((line(&l, TERRAIN).done, line(&l, TERRAIN).total), (0, Some(1)));
        assert_eq!((line(&l, UNITS).done, line(&l, UNITS).total), (0, Some(1)));
        assert_eq!(line(&l, TILES).total, None, "no areas built: the tiles aren't known yet");
        assert_eq!(line(&l, SITES).left, Some(1));
        assert_eq!((line(&l, TREES).done, line(&l, TREES).total), (0, Some(3)), "two pieces and their assembly");
        // Terrain, slope, the heritage sites and the area done.
        for step in ["heritage-sites", "terrain", "unit", "slope", "trees", "trees-lo"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
            } else {
                did(&mut m, &mut done, &w[0]);
            }
        }
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        let l = checklist(&c, "d", &m, &done, &BTreeMap::new(), false);
        for what in [TERRAIN, SLOPE, SITES, UNITS, TREES] {
            assert!(line(&l, what).finished(), "{what} done");
        }
        let tiles = line(&l, TILES);
        assert!(tiles.total.is_some_and(|t| t > 1) && tiles.done == 0 && !tiles.finished());
        assert!(line(&l, ROADS).left.is_some_and(|n| n >= 1));
        // Each group's jobs left, by name, in their order.
        let roads = line(&l, ROADS);
        assert_eq!(Some(roads.next.len()), roads.left, "{:?}", roads.next);
        assert!(roads.next.iter().all(|n| !n.is_empty() && n != label("catalog")), "{:?}", roads.next);
        assert_eq!(line(&l, PUBLISH).left, Some(1));
        // Held for review: publishing is the held catalog.
        let k = catalog_key(&m, &BTreeMap::new(), &["r".to_string()]);
        done.catalog_held = Some(k);
        assert!(line(&checklist(&c, "d", &m, &done, &BTreeMap::new(), true), PUBLISH).finished());
        assert!(!line(&checklist(&c, "d", &m, &done, &BTreeMap::new(), false), PUBLISH).finished());
    }

    #[test]
    fn tree_cover_pieces_their_mids_and_their_assembly() {
        let (c, reach, mut m, mut done) = three();
        let each = c.by_region();
        let plan = |m: &BTreeMap<String, String>, done: &Keys| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), &tiles_for(m), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: None, current: None, held: false });
        let ts = |v: &[(String, String)]| v.iter().map(|t| t.0.clone()).collect::<Vec<_>>();
        // Iceland's regions: z3 tile 3/3/2's pieces 6/28/16, 6/28/17 and 6/29/16.
        let tt = crate::treepacks::targets(&c, &m);
        assert_eq!((tt.pieces.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), tt.lo.iter().map(|l| l.0.as_str()).collect::<Vec<_>>()), (vec!["6/28/16", "6/28/17", "6/29/16"], vec!["3/3/2"]));
        // Each piece first, then the assembly once every piece has its mid.
        let w = tree_work(&tt, &m, &done);
        assert_eq!((ts(&w.pieces), w.lo.len(), w.backfill.len()), (vec!["6/28/16".to_string(), "6/28/17".into(), "6/29/16".into()], 0, 0));
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
        assert_eq!(p.backfill, w.backfill);
        assert!(!p.work[..p.work.len() - 1].iter().any(|x| x.step == "trees" || x.step == "trees-lo"), "{:?}", p.work);
        // One mid made: the assembly's key names it, so it's stale, and the other pieces' mids are
        // made for it (with the area's work, no longer idle work); then it runs.
        did(&mut m, &mut done, &Work { step: "trees".into(), targets: vec![w.backfill[0].clone()] });
        let tt = crate::treepacks::targets(&c, &m);
        let w = tree_work(&tt, &m, &done);
        assert_eq!((ts(&w.pieces), w.lo.len(), w.backfill.len()), (vec!["6/28/17".to_string(), "6/29/16".into()], 0, 0), "not assembled until each piece has its mid");
        assert!(w.stale_pieces.is_empty() && w.stale_lo.contains("3/3/2"));
        // Their mids handed off, not yet merged: neither made again, and the assembly waits for them
        // to be merged.
        let mut handed = done.clone();
        handed.handed.extend([crate::treepacks::mid_logical(28, 17), crate::treepacks::mid_logical(29, 16)]);
        let wh = tree_work(&tt, &m, &handed);
        assert!(wh.pieces.is_empty() && wh.backfill.is_empty() && wh.lo.is_empty() && wh.stale_lo.contains("3/3/2"), "{wh:?}");
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
        assert_eq!((ts(&w.pieces), w.lo.len(), w.backfill.len()), (vec!["6/28/16".to_string(), "6/29/16".into()], 0, 0));
        assert_eq!(w.stale_pieces.iter().collect::<Vec<_>>(), ["6/28/16"]);
        // (Its regions aren't ready meanwhile: a's piece is stale, and b's area's assembly will be.)
        let p = plan(&m, &done);
        assert!(p.ready.is_empty() && p.regions.iter().all(|r| r.trees.contains(&"6/28/16".to_string())), "{:?}", p.regions);
        // The coverage gone from 3/3/2 altogether: each piece drops its packs and mid ("none"),
        // and the assembly its lo packs.
        let far = Coverage::from_recipes(&[crate::agent::recipes::Recipe { id: "f".into(), name: "F".into(), outline: vec!["place:20,40,10".into()] }], None, std::path::Path::new("/nonexistent")).unwrap();
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
        let cov_of = |rs: &[(&str, &str)]| Coverage::from_recipes(&rs.iter().map(|(id, o)| Recipe { id: id.to_string(), name: "R".into(), outline: vec![o.to_string()] }).collect::<Vec<_>>(), None, d.path()).unwrap();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        let keys = |c: &Coverage| (terrain_slope_targets(c, &m), unit_keys(c, "d", &m).into_iter().map(|(u, k)| (u.slash(), k)).collect::<Vec<_>>(), pois_keys(c, "d", &m).into_iter().map(|(_, k)| k).collect::<Vec<_>>());
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
        let long = |verts: &[(f64, f64)]| LongWay { owned: true, ferry: false, verts: verts.iter().map(|&(x, y)| [(x * 1e7) as i32, (y * 1e7) as i32]).collect() };
        r.units.insert("6/29/16".into(), Reach { owned: Some(e7box(-15.0, 64.1, -14.0, 64.2)), long: vec![long(&[(-15.0, 64.15), (-21.9, 64.13)])] });
        r.units.insert("6/30/16".into(), Reach { owned: Some(e7box(-10.0, 63.0, -6.0, 65.0)), long: vec![long(&[(-8.0, 64.0), (-2.0, 60.0)])] });
        let built: Vec<String> = ukeys(&c, "d", &m, Some(&r), &BTreeMap::new()).into_iter().map(|(u, _)| u.slash()).collect();
        assert_eq!(built, vec!["6/28/16", "6/29/16"]);
        // No reaches yet: no units.
        assert!(ukeys(&c, "d", &m, None, &BTreeMap::new()).is_empty());
        // An outline's change outside a unit's whole reach leaves its key; inside it, not.
        let poly = |east: f64| {
            std::fs::write(d.path().join("p.poly"), format!("p\n1\n -22.2 64.0\n -21.8 64.0\n {east} 64.3\n -22.2 64.3\nEND\nEND\n")).unwrap();
            Coverage::from_recipes(&[Recipe { id: "p".into(), name: "P".into(), outline: vec!["poly:p.poly".into()] }], None, d.path()).unwrap()
        };
        let key = |c: &Coverage| ukeys(c, "d", &m, Some(&r), &BTreeMap::new()).into_iter().find(|(u, _)| u.slash() == "6/28/16").unwrap().1;
        let k0 = key(&poly(30.0));
        assert_ne!(key(&poly(31.0)), k0, "the moved edge crosses 6/28/16's tile");
        let far = |east: f64| {
            std::fs::write(d.path().join("q.poly"), format!("q\n1\n -22.2 64.0\n -21.8 64.0\n -21.8 64.1\n -15.0 64.1\n {east} 64.1\n {east} 70.0\n -22.2 70.0\nEND\nEND\n")).unwrap();
            Coverage::from_recipes(&[Recipe { id: "q".into(), name: "Q".into(), outline: vec!["poly:q.poly".into()] }], None, d.path()).unwrap()
        };
        let k1 = key(&far(10.0));
        assert_eq!(key(&far(12.0)), k1, "only vertices far east of the tile moved");
    }

    #[test]
    fn taiwans_units_follow_its_dem_files() {
        let d = tempfile::tempdir().unwrap();
        let c = Coverage::from_recipes(&[Recipe { id: "t".into(), name: "T".into(), outline: vec!["place:121.5,25.0,20".into(), "place:-21.9,64.13,20".into()] }], None, d.path()).unwrap();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for u in ["6-28-16", "6-53-27"] {
            m.insert(format!("sources/osm/d/pieces/{u}"), format!("sources/osm/d/pieces/{u}.4444444444444444.osm.pbf"));
        }
        let mut r = reach();
        r.units.insert("6/53/27".into(), Reach { owned: Some(e7box(121.4, 24.9, 121.6, 25.1)), long: vec![] });
        let keys = |dig: &BTreeMap<String, String>| ukeys(&c, "d", &m, Some(&r), dig).into_iter().map(|(u, k)| (u.slash(), k)).collect::<BTreeMap<_, _>>();
        let (none, some) = (keys(&BTreeMap::new()), keys(&[("moi-dtm".to_string(), "1111111111111111".to_string())].into()));
        assert_ne!(none["6/53/27"], some["6/53/27"], "Taipei's unit reruns when the DEM files arrive");
        assert_eq!(none["6/28/16"], some["6/28/16"], "Iceland's doesn't");
    }

    #[test]
    fn removing_coverage_prunes_what_it_built() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        unit_inputs(&mut m, "d");
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        m.insert("sources/osm/d/pieces/6-40-20".into(), "sources/osm/d/pieces/6-40-20.5555555555555555.osm.pbf".into());
        for step in ["heritage-sites", "terrain", "unit", "slope", "trees", "trees-lo"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
            } else {
                did(&mut m, &mut done, &w[0]);
            }
        }
        // Built once for a region since removed: 6/40/20's outputs, its candidates, a map tile far
        // from any unit and the zoomed-out tile over it.
        for l in ["base/6-40-20", "global/roads/6-40-20", "global/roaden/6-40-20", "work/pois/6-40-20", "hidata/6-40-20", "layers/roads/hi/6-40-20", "layers/roads/lo/3-5-2"] {
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
        let lo_of = |m: &BTreeMap<String, String>, q: &str| pack_lo_targets(m, None).1.into_iter().find(|(t, _)| t == q).unwrap().1;
        let (near, far) = (lo_of(&m, "3/3/2"), lo_of(&m, "3/6/2"));
        // A unit rebuilt with new content: the zoomed-out tile over it is stale, the far one isn't.
        put(&mut m, "6-28-16", "3333333333333333");
        assert_ne!(lo_of(&m, "3/3/2"), near);
        assert_eq!(lo_of(&m, "3/6/2"), far);
    }

    #[test]
    fn a_long_way_reaches_its_map_tiles() {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        m.insert("base/6-28-16".into(), "base/6-28-16.1111111111111111.sect".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.1111111111111111.sect".into());
        let mut r = reach();
        // 6/28/16 owns a ferry to Norway (5° E, 60° N), far past 110 km.
        r.units.insert("6/28/16".into(), Reach { owned: Some(e7box(-22.0, 64.0, -21.7, 64.16)), long: vec![LongWay { owned: true, ferry: true, verts: vec![[-219_000_000, 641_000_000], [50_000_000, 600_000_000]] }] });
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
        let key = |m: &BTreeMap<String, String>, step: &str| remaining(&Keys::default(), |d| roads_chain("d", m, d, &BTreeMap::new(), None)).into_iter().find(|w| w.step == step).unwrap().targets[0].1.clone();
        let (stations, ferries) = (key(&m, "stations"), key(&m, "ferries"));
        m.insert("base/6-40-20".into(), "base/6-40-20.2222222222222222.sect".into());
        m.insert("global/roads/6-40-20".into(), "global/roads/6-40-20.2222222222222222.sect".into());
        assert_ne!(key(&m, "stations"), stations, "stops are clipped to the built units");
        assert_eq!(key(&m, "ferries"), ferries);
    }

    #[test]
    fn trains_a_day_follow_their_feeds() {
        let d = tempfile::tempdir().unwrap();
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let none = BTreeMap::new();
        let lta: BTreeMap<String, String> = [("keys".to_string(), "LTA_ACCOUNT_KEY".to_string())].into();
        // Not seeded: nothing.
        assert!(rail_chain(&c, "d", &m, &done, &lta).is_none());
        m.insert(crate::rail::CATALOGUE.into(), "sources/rail/catalogue.1111111111111111.csv".into());
        m.insert(crate::osmpass::set_name("d", "rail"), "sources/osm/d/sets/rail.2222222222222222.osm.pbf".into());
        let w = rail_chain(&c, "d", &m, &done, &lta).unwrap();
        assert_eq!(w.step, "rail-feeds");
        done.record(&w.step, &w.targets);
        // Its list made: then the trains; then nothing, until what decides them changes.
        m.insert(crate::rail::FEEDS.into(), "sources/rail/feeds.3333333333333333.json".into());
        let w = rail_chain(&c, "d", &m, &done, &lta).unwrap();
        assert_eq!(w.step, "rail");
        done.record(&w.step, &w.targets);
        assert!(rail_chain(&c, "d", &m, &done, &lta).is_none());
        // What rail-feeds writes besides the list (the checks, the zips) isn't what anything reads:
        // no rerun for its own sake.
        m.insert(crate::rail::CHECKED.into(), "sources/rail/checked.4444444444444444.json".into());
        m.insert(crate::rail::zip_logical("sncf"), "sources/rail/gtfs/sncf.5555555555555555.zip".into());
        assert!(rail_chain(&c, "d", &m, &done, &lta).is_none());
        // Keys no feed uses yet: no rerun. A key gone, or the coverage grown: the feeds again.
        let more: BTreeMap<String, String> = [("keys".to_string(), "LTA_ACCOUNT_KEY,ODPT_KEY,TDX_CLIENT_ID".to_string())].into();
        assert!(rail_chain(&c, "d", &m, &done, &more).is_none());
        assert_eq!(rail_chain(&c, "d", &m, &done, &none).unwrap().step, "rail-feeds");
        let bigger = Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:-21.9,64.13,25".into()] }], None, d.path()).unwrap();
        assert_eq!(rail_chain(&bigger, "d", &m, &done, &lta).unwrap().step, "rail-feeds");
        // A new list (a feed fetched), a new pass's rail set, other MTR pairs: the trains again.
        for (l, f) in [(crate::rail::FEEDS.to_string(), "sources/rail/feeds.6666666666666666.json"), (crate::osmpass::set_name("d", "rail"), "sources/osm/d/sets/rail.7777777777777777.osm.pbf"), (crate::rail::MTR_PAIRS.to_string(), "sources/rail/mtr-pairs.8888888888888888.bin")] {
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
        m.insert(crate::rail::CATALOGUE.into(), "sources/rail/catalogue.1111111111111111.csv".into());
        m.insert(crate::osmpass::set_name("d", "rail"), "sources/osm/d/sets/rail.2222222222222222.osm.pbf".into());
        // From the start, listed after the regions' work (the units don't wait for it, nor it for them).
        for step in ["heritage-sites", "terrain", "unit", "slope", "trees", "trees-lo"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            assert_eq!(w.last().unwrap().step, "rail-feeds", "{:?}", steps(&w));
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
                heritage_chain_done(&c, &m, &mut done, "d");
            } else {
                did(&mut m, &mut done, &w[0]);
            }
        }
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        // Then beside the roads' chain, after it in the order.
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(steps(&w), vec!["roadunits", "rail-feeds"]);
        // The checklist: two jobs left, then one.
        let line = |m: &BTreeMap<String, String>, done: &Keys| checklist(&c, "d", m, done, &BTreeMap::new(), false).into_iter().find(|s| s.what == TRAINS).unwrap();
        assert_eq!(line(&m, &done).left, Some(2));
        done.record("rail-feeds", &w[1].targets);
        m.insert(crate::rail::FEEDS.into(), "sources/rail/feeds.3333333333333333.json".into());
        assert_eq!(line(&m, &done).left, Some(1));
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(steps(&w), vec!["roadunits", "rail"]);
        done.record("rail", &w[1].targets);
        assert!(line(&m, &done).finished());
    }

    #[test]
    fn trains_a_day_unknown_while_blocked() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let lta: BTreeMap<String, String> = [("keys".to_string(), "LTA_ACCOUNT_KEY".to_string())].into();
        let line = |m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>| checklist(&c, "d", m, done, inputs, false).into_iter().find(|s| s.what == TRAINS).unwrap();
        assert_eq!(line(&m, &done, &lta).left, None, "not seeded");
        m.insert(crate::rail::CATALOGUE.into(), "sources/rail/catalogue.1111111111111111.csv".into());
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
        for step in ["heritage-sites", "terrain", "unit", "slope", "trees", "trees-lo"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
                heritage_chain_done(&c, &m, &mut done, "d");
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
        assert_eq!(steps(&w), vec!["roadunits", "pois"]);
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
        // The registers' snapshot changed: the heritage sites are made again first, the rest of the
        // chain (an hour and a half on them) only after.
        m.insert("sources/registers/legacy".into(), "sources/registers/legacy.ffffffffffffffff.tar.zst".into());
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
        let r = |id: &str, place: &str| Recipe { id: id.into(), name: id.to_uppercase(), outline: vec![format!("place:{place},20")] };
        let c = Coverage::from_recipes(&[r("a", "-21.9,64.13"), r("g", "-51.7,64.18")], None, d.path()).unwrap();
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
        let plan = |m: &BTreeMap<String, String>, done: &Keys| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), &tiles_for(m), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: None, current: None, held: false }).work;
        let line = |w: &[Work]| w.iter().map(|x| format!("{} {}", x.step, x.targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(","))).collect::<Vec<_>>();
        let mut done = Keys::default();
        // Before the heritage sites (the units wait for them): the candidates already, after the
        // terrain and tree cover.
        let w = plan(&m, &done);
        assert_eq!(line(&w), ["heritage-sites heritage-sites", "terrain 3/2/2,3/3/2", "trees 6/22/16,6/22/17,6/28/16,6/28/17", "pois 6/22/17,6/28/16"]);
        heritage_done(&mut m, &mut done, "d", &w[0]);
        // The heritage sites made: the rest of the heritage chain too, beside the candidates, both
        // after the regions' work.
        let w = plan(&m, &done);
        assert_eq!(line(&w), ["terrain 3/2/2", "terrain 3/3/2", "trees 6/22/16,6/22/17,6/28/16,6/28/17", "pois 6/22/17,6/28/16", "heritage heritage"]);
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
        done.record("terrain", &[w[0].targets[0].clone()]);
        let w = plan(&m, &done);
        assert_eq!(line(&w).last().unwrap(), "heritage heritage");
        assert!(line(&w).contains(&"peaks 6/22/17".to_string()), "{:?}", line(&w));
        // The landmark points wait for the peaks, the items' facts and the heritage chain; the
        // overlays for the last unit.
        let rest: Vec<Work> = w.iter().filter(|x| matches!(x.step.as_str(), "peaks" | "items" | "heritage")).cloned().collect();
        for x in &rest {
            done.record(&x.step, &x.targets);
        }
        let marks = |m: &BTreeMap<String, String>, done: &Keys| plan(m, done).into_iter().filter(|x| matches!(x.step.as_str(), "marks" | "overlays")).map(|x| x.step).collect::<Vec<_>>();
        assert!(marks(&m, &done).is_empty(), "Reykjavik's peaks are still to come");
        done.record("terrain", &[("3/3/2".to_string(), plan(&m, &done).iter().find(|x| x.step == "terrain").unwrap().targets[0].1.clone())]);
        let peaks = plan(&m, &done).into_iter().find(|x| x.step == "peaks").unwrap();
        done.record("peaks", &peaks.targets);
        assert_eq!(marks(&m, &done), ["marks"]);
    }

    /// The 3D buildings' sources as agent::input_digests has them: every z6 tile within 1 km of
    /// `c` reading something (its digest `d`), and their rows.
    fn bld_inputs(c: &Coverage, d: &str) -> BTreeMap<String, String> {
        let mut inputs = BTreeMap::from([("bld-release".to_string(), crate::buildtiles::RELEASE.to_string())]);
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
        assert_eq!(bld_targets(&c, &m, &BTreeMap::from([("bld-release".to_string(), "?".to_string())])).tiles.len(), tt.tiles.len());
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
        for (l, changes) in [(crate::bld::work_logical(u.x, u.y), true), (crate::bld::work_logical(u.x + 1, u.y + 1), true), (crate::bld::work_logical(u.x + 2, u.y), false)] {
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

    #[test]
    fn the_3d_buildings_chain() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let inputs = bld_inputs(&c, "a");
        let mut done = Keys::default();
        let steps = |w: &[Work]| w.iter().map(|x| x.step.clone()).collect::<Vec<_>>();
        // Not asked about (no "bld-release"), or the indexes unreadable: nothing.
        assert!(bld_work(&c, &m, &done, &BTreeMap::new(), &by_spatial).is_empty());
        assert!(bld_work(&c, &m, &done, &BTreeMap::from([("bld-release".to_string(), "?".to_string())]), &by_spatial).is_empty());
        // Nothing downloaded yet: the fetch alone.
        assert_eq!(steps(&bld_work(&c, &m, &done, &BTreeMap::from([("bld-release".to_string(), String::new())]), &by_spatial)), ["bld-fetch"]);
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
            let l = crate::bld::pack_logical(u.x, u.y);
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
        assert_eq!(w, [Work { step: "prune".into(), targets: vec![("bldprep 6/40/20".into(), String::new()), ("bldtiles 6/40/20".into(), String::new())] }]);
        done.record("prune", &w[0].targets);
        assert!(!done.bldtiles.contains_key("6/40/20"));
        // (Not the normalized files while the sources aren't here: every tile would seem to read
        // nothing.)
        let none = BTreeMap::from([("bld-release".to_string(), String::new())]);
        assert_eq!(bld_work(&c, &m, &done, &none, &by_spatial).iter().flat_map(|w| w.targets.iter().map(|t| t.0.clone())).filter(|t| t.starts_with("bldprep")).count(), 0);
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
        let b = cl.iter().find(|x| x.what == BUILDINGS).unwrap();
        let tt = bld_targets(&c, &m, &inputs);
        assert_eq!((b.done, b.total), (0, Some(tt.prep.len() + tt.tiles.len())));
        assert_eq!(b.next[0], label("bld-fetch"));
        assert!(b.next[1].starts_with(label("bldprep")));
        // Its steps' targets go by tile in the jobs' names.
        assert_eq!(next_of(&[Work { step: "bldtiles".into(), targets: vec![("6/1/1".into(), "k".into()), ("6/1/2".into(), "k".into())] }]), ["Raising the 3D buildings: 2 tiles"]);
        let mut st = checklist_to_come();
        mark_shared(&mut st);
        assert_eq!(st.iter().find(|x| x.what == BUILDINGS).unwrap().shared.as_deref(), Some("sources read and tiles"));
    }

    #[test]
    fn the_3d_buildings_never_hold_up_a_region_nor_change_a_rounds_catalog() {
        let (c, reach, mut m, mut done) = three();
        let each = c.by_region();
        let inputs = bld_inputs(&c, "a");
        let plan = |m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>, since: Option<u64>, current: Option<&Round>| super::plan(&c, "d", m, done, &inputs, Some(&reach), &tiles_for(m), Rounds { each: &each, on_map, since_last: since, current, held: false });
        for u in ["6/28/16", "6/29/16", "6/30/16", "6/31/16"] {
            rbuild(&c, &reach, &mut m, &mut done, u, "6666666666666666");
        }
        let on = |ids: &[&str]| -> BTreeMap<String, bool> { ids.iter().map(|i| (i.to_string(), true)).collect() };
        // Every unit built, every region on the map, the 3D buildings being raised: no round ten
        // minutes after the last, one an hour after.
        let p = plan(&m, &done, &on(&["a", "b", "c"]), Some(600), None);
        assert!(p.begins.is_none() && p.work.iter().any(|w| w.step == "bldprep"), "{:?}", p.work);
        assert!(plan(&m, &done, &on(&["a", "b", "c"]), Some(PUBLISH_EVERY_S), None).begins.is_some());
        // A region done that the map lacks: its round at once, the buildings or not.
        let p = plan(&m, &done, &on(&["a", "b"]), Some(600), None);
        assert_eq!(p.begins.map(|r| r.regions), Some(vec!["c".to_string()]));
        // Only their sources' fetch left (failing, say, once the release has left S3): it holds
        // nothing back.
        let tt = bld_targets(&c, &m, &inputs);
        done.record("bldprep", &tt.prep);
        done.record("bldtiles", &tt.tiles);
        let p = plan(&m, &done, &on(&["a", "b", "c"]), Some(600), None);
        assert!(p.work.iter().any(|w| w.step == "bld-fetch"), "{:?}", p.work);
        assert!(p.begins.is_some());
        // A round fixes the 3D buildings' packs as it begins: one made meanwhile goes out with the
        // next, its catalog's key unchanged.
        let tile = tt.tiles[0].0.replace('/', "-");
        let pack = format!("layers/buildings/hi/{tile}");
        m.insert(pack.clone(), format!("{pack}.1111111111111111.pack"));
        let mut r = plan(&m, &done, &on(&["a", "b"]), Some(600), None).begins.expect("c's round");
        r.began = 1;
        assert!(r.units.contains_key(&pack));
        m.insert(pack.clone(), format!("{pack}.2222222222222222.pack"));
        let then = crate::out::units_as_of(&m, &r.units);
        assert_eq!(then.get(&pack).map(String::as_str), Some(format!("{pack}.1111111111111111.pack").as_str()));
        let other = format!("layers/buildings/hi/{}", tt.tiles.last().unwrap().0.replace('/', "-"));
        m.insert(other.clone(), format!("{other}.3333333333333333.pack"));
        assert!(!crate::out::units_as_of(&m, &r.units).contains_key(&other) || other == pack);
    }

    #[test]
    fn the_records_keep_what_this_app_doesnt_know() {
        // A newer app's step's records, saved again by this one: kept as they were.
        let json = r#"{"unit": {"6/1/1": "k"}, "bldtiles": {"6/2/2": "b"}, "someday": {"6/3/3": "s"}, "catalog": "c"}"#;
        let mut k: Keys = serde_json::from_str(json).unwrap();
        assert_eq!((k.bldtiles.get("6/2/2").map(String::as_str), k.other.len()), (Some("b"), 1));
        k.record("unit", &[("6/4/4".to_string(), "k4".to_string())]);
        let v: serde_json::Value = serde_json::to_value(&k).unwrap();
        assert_eq!((&v["someday"]["6/3/3"], &v["bldtiles"]["6/2/2"], &v["unit"]["6/4/4"]), (&serde_json::json!("s"), &serde_json::json!("b"), &serde_json::json!("k4")));
    }
}
