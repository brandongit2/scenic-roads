//! What the build Mac builds for the regions (docs/plan.md §6, Job keys; §8, Order): per z3 pack,
//! the terrain and then the slope of its z6 tiles near the coverage; base(U) for every unit whose
//! piece meets the coverage, a region at a time; pack(T) for the z6 tiles near changed units, the
//! lo packs above them; then a catalog, as each region is done (`plan`).
//!
//! Each job's key is a hash of what it reads: its step's version, the coverage near it, and the
//! content names (from the build manifest) of its inputs. A job whose key matches the one recorded
//! when it last succeeded (`state/build/jobs.json`) isn't run again; an unchanged output keeps its
//! content name, so what depends on it keeps its key too.

use crate::coverage::Coverage;
use crate::legacy::Unit;
use crate::reach::Reaches;
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
    /// The tree cover layers per z3 tile (crate::treepacks).
    #[serde(default)]
    pub trees: BTreeMap<String, String>,
    /// The served files the last catalog was made from.
    #[serde(default)]
    pub catalog: Option<String>,
    /// The same for the last catalog held for review (`inputs/hold-catalog`: written to
    /// catalog-held/, not served).
    #[serde(default)]
    pub catalog_held: Option<String>,
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
    /// journal on this Mac).
    pub fn load_with(root: &Path, bases: &[std::path::PathBuf]) -> anyhow::Result<Keys> {
        let mut hs = Vec::new();
        for b in bases {
            hs.extend(crate::handoff::waiting_in(b)?);
        }
        let mut k = Keys::load_strict(root)?;
        for (_, h) in hs {
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
            _ => return None,
        };
        m.get(target).map(String::as_str)
    }

    /// Records a job's targets as done with their keys. A prune forgets its targets' keys instead
    /// ("unit 6/x/y", "pois 6/x/y", "pack 6/x/y", "lo 3/x/y"), so a region added back is built again.
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
        if step.ends_with("-root") || matches!(step, "labels" | "trailends" | "reach" | "summits" | "items" | "marks" | "roadunits" | "stations" | "ferries" | "heritage-sites" | "heritage" | "overlays" | "rail-feeds" | "rail") {
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

fn h(parts: &[&str]) -> String {
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
/// regions.
pub const LABELS_V: u32 = 1;

pub fn labels_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let set = m.get(&crate::osmpass::set_name(date, "labels"))?;
    let k = h(&[&format!("labels {LABELS_V}"), set]);
    (done.lo.get("labels").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "labels".into(), targets: vec![("labels".into(), k)] })
}

/// Whether unit `u` is built for the coverage: some road it owns may touch it (`Reach::builds`;
/// the unit step then keeps exactly the ways that do).
pub fn builds(cov: &Coverage, reach: &Reaches, u: Unit) -> bool {
    reach.get(u).is_some_and(|r| r.builds(cov))
}

/// The units the coverage builds (`builds`), each with its key: what it reads (its piece and road
/// values, the coverage as its ways meet it (`Reach::coverage_key`) and the location rules where
/// they go (`crate::rules`), the heritage sites' and areas' slices near it, the staged layers near
/// it as the manifest has them, which is what the unit step stages from, and in Taiwan the MOI
/// DTM's files, `digests["moi-dtm"]`). None before the pass's reaches are made.
pub fn unit_keys(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, reach: Option<&Reaches>, digests: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let mut units: Vec<(Unit, String)> = Vec::new();
    let Some(reach) = reach else { return units };
    for (l, c) in m.range(format!("sources/osm/{date}/pieces/")..) {
        let Some(u) = l.strip_prefix(&format!("sources/osm/{date}/pieces/")).and_then(Unit::parse) else { break };
        let Some(r) = reach.get(u).filter(|r| r.builds(cov)) else { continue };
        let mut inputs = vec![
            format!("unit {UNIT_V}"),
            c.clone(),
            get(&format!("sources/osm/{date}/roads/{}", u.dash())).to_string(),
            r.coverage_key(cov, u),
            crate::rules::versions_meeting(r.extent(u)),
        ];
        // Taiwan's DEM when it's there (a file dropped in reruns the units it covers).
        if crate::rules::meets_taiwan(r.extent(u)) {
            inputs.push(format!("moi-dtm {}", digests.get("moi-dtm").map(String::as_str).unwrap_or("-")));
        }
        // The roadside buildings it reads (crate::buildtiles: the release's tiles near its roads).
        inputs.push(format!("buildings {}", get(&crate::buildtiles::index_logical())));
        // The terrain near it. Not the analysis grids' packs (grid-class, -canopy, -cover): the
        // units write those where they're missing, so each built unit would change its own key and
        // its neighbours' (built again, over and over); a grid read from its pack or made afresh is
        // the same, from fixed datasets (WorldCover, Meta's canopy squares).
        let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
        for (x, y) in crate::stage::tiles_in(6, b) {
            inputs.push(get(&format!("layers/terrain/hi/6-{x}-{y}")).to_string());
            inputs.push(get(&crate::heritage::pos_logical(date, x, y)).to_string());
            inputs.push(get(&crate::heritage::areas_logical(date, x, y)).to_string());
        }
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        units.push((u, h(&refs)));
    }
    units
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

pub fn region_states(cov: &Coverage, regions: &[(String, Coverage)], date: &str, m: &BTreeMap<String, String>, done: &Keys, reach: Option<&Reaches>, digests: &BTreeMap<String, String>) -> BTreeMap<String, RegionState> {
    let keys = unit_keys(cov, date, m, reach, digests);
    regions
        .iter()
        .map(|(id, rc)| {
            let mine: Vec<&(Unit, String)> = keys.iter().filter(|(u, _)| reach.is_some_and(|r| builds(rc, r, *u))).collect();
            let built = mine.iter().filter(|(u, k)| done.unit.get(&u.slash()) == Some(k)).count();
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
    /// The units' outputs in the manifest when it began (crate::out::UNIT_OUTPUTS): its map tiles,
    /// road index, rail stops and catalog are made from them (crate::out::units_as_of; its jobs,
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
    /// Its areas whose slope, and whose tree cover, are stale.
    pub slope: Vec<String>,
    pub trees: Vec<String>,
}

/// The plan for the coverage `cov`, the pass of `date`, the build manifest `m` (logical → content)
/// and what was done (`done`). `inputs`: digests of what jobs read from `inputs/` (not in the
/// manifest), by name: "ferries-freq" (the ferry timetables), "regions" (the recipes). The agent runs
/// the first work not waiting out a failure; a helper takes a step's from the far end (the agent
/// offers each step's targets together, in this order). The order (docs/plan.md §8, Order):
/// - the heritage sites the units read;
/// - a region at a time (the regions the map hasn't at all first, the one with the fewest units
///   left first): the terrain areas it reads that are stale, then its units whose terrain is built
///   (a unit's key reads the terrain near it: one built before would be built again);
/// - slope (each area once its terrain is built) and tree cover after them, but a region's done
///   and waiting for its round before all that;
/// - a round as a region is done, PUBLISH_EVERY_S after the last began, fixed as it begins
///   (`Round`): its areas' slope and tree cover, the map tiles, the road index, rail stops and
///   ferries, and a catalog with its regions, from the units as they were when it began;
/// - after the last unit and terrain area, the same for everything;
/// - the trains' and the landmarks' chains from the start, each step once what it reads is built,
///   after all that in the order (a second job beside the regions' takes them: crate::agent), the
///   overlays after the last unit; what they make goes out with the next round's catalog, or one
///   of its own after the last.
pub fn plan(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, reach: Option<&Reaches>, rounds: Rounds) -> Plan {
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
    // tree cover layers per z3 tile, what a region needs before it's published.
    let (terrain, slope) = terrain_slope_targets(cov, m);
    let terrain: Vec<(String, String)> = terrain.into_iter().filter(|(t, k)| stale(&done.terrain, t, k)).collect();
    let terrain_left: BTreeSet<String> = terrain.iter().map(|t| t.0.clone()).collect();
    let slope: Vec<(String, String)> = slope.into_iter().filter(|(t, k)| stale(&done.slope, t, k)).collect();
    let trees: Vec<(String, String)> = crate::treepacks::targets(cov, m).into_iter().filter(|(t, k)| done.trees.get(t) != Some(k)).collect();
    // (What a region lacks before it's published: stale slope counts, built or not yet buildable.)
    let slope_left: BTreeSet<String> = slope.iter().map(|t| t.0.clone()).collect();
    let trees_left: BTreeSet<String> = trees.iter().map(|t| t.0.clone()).collect();
    let slope: Vec<(String, String)> = slope.into_iter().filter(|t| !terrain_left.contains(&t.0)).collect();

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
        work.extend(chains(false));
        return Plan { work, ..Default::default() };
    };

    // base(U): the units the coverage builds. Each region's: those it builds itself (a road of theirs
    // may touch its outline), the ones of them not built as the coverage now wants, its areas (the
    // z3 tiles its slope and tree cover are in) and the terrain areas it reads (its areas and those
    // near its units).
    let units = unit_keys(cov, date, m, Some(reach), inputs);
    let unit_stale: Vec<bool> = units.iter().map(|(u, k)| stale(&done.unit, &u.slash(), k)).collect();
    // A unit's terrain areas: those of the z6 tiles near it, whose hi packs its key reads.
    let reads: Vec<BTreeSet<String>> = units
        .iter()
        .map(|(u, _)| crate::stage::tiles_in(6, crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM)).into_iter().map(|(x, y)| format!("3/{}/{}", x >> 3, y >> 3)).collect())
        .collect();
    let buildable = |i: usize| reads[i].is_disjoint(&terrain_left);
    struct Region<'a> {
        id: &'a str,
        /// Its units, and those not built as the coverage wants them.
        all: Vec<usize>,
        stale: Vec<usize>,
        /// Its slope's areas (the z3 tiles within 20 km of it, as the terrain's and slope's targets
        /// go) and its tree cover's (those it meets, as treepacks::targets goes).
        areas: BTreeSet<String>,
        tree_areas: BTreeSet<String>,
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
            let terrain = areas.iter().cloned().chain(stale.iter().flat_map(|&i| reads[i].iter().cloned())).filter(|a| terrain_left.contains(a)).collect();
            Region { id, all, stale, areas, tree_areas, terrain }
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
    let ready: Vec<String> = regions.iter().filter(|r| r.stale.is_empty() && r.terrain.is_empty() && r.areas.is_disjoint(&slope_left) && r.tree_areas.is_disjoint(&trees_left) && in_round(r)).map(|r| r.id.to_string()).collect();

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
        push(&mut by_region, "unit", mine.into_iter().map(|i| (units[i].0.slash(), units[i].1.clone())).collect());
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
            trees: r.tree_areas.iter().filter(|a| trees_left.contains(*a)).cloned().collect(),
        });
    }

    // (Terrain no region with work left reads, and a unit in no region's own coverage, last.)
    push(&mut by_region, "terrain", terrain.iter().filter(|(q, _)| !listed.contains(q)).cloned().collect());
    let mut rest: Vec<usize> = (0..units.len()).filter(|&i| unit_stale[i] && !taken[i] && buildable(i)).collect();
    rest.sort_by_key(|&i| spatial_order(units[i].0));
    push(&mut by_region, "unit", rest.into_iter().map(|i| (units[i].0.slash(), units[i].1.clone())).collect());
    // Slope and tree cover in the same order: the areas of the region built first, first.
    let rank = |t: &(String, String)| order.iter().position(|r| r.areas.contains(&t.0)).unwrap_or(usize::MAX);
    let tree_rank = |t: &(String, String)| order.iter().position(|r| r.tree_areas.contains(&t.0)).unwrap_or(usize::MAX);
    let (mut slope, mut trees) = (slope, trees);
    slope.sort_by_key(rank);
    trees.sort_by_key(tree_rank);

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
    if rounds.current.is_none() && (last_now || (!to_publish.is_empty() && rounds.since_last.is_none_or(|s| s >= PUBLISH_EVERY_S))) {
        let units_now = m.iter().filter(|(l, _)| crate::out::UNIT_OUTPUTS.iter().any(|p| l.starts_with(p))).map(|(l, c)| (l.clone(), c.clone())).collect();
        let begun = Round { began: 0, regions: to_publish.iter().map(|r| r.id.to_string()).collect(), last: last_now, units: units_now, over: false };
        let p = plan(cov, date, m, done, inputs, Some(reach), Rounds { current: Some(&begun), ..rounds });
        // (One with nothing to publish that isn't out already, after the last: none.)
        if !p.ends {
            return Plan { begins: Some(begun), ..p };
        }
    }
    // The slope and tree cover of the regions done and waiting for a round (not this one): before
    // the regions' work, so their round only draws.
    let waiting = |r: &&&Region| rounds.current.is_none_or(|rd| !rd.regions.iter().any(|x| x == r.id));
    let slope_waits = |t: &(String, String)| to_publish.iter().filter(waiting).any(|r| r.areas.contains(&t.0));
    let trees_waits = |t: &(String, String)| to_publish.iter().filter(waiting).any(|r| r.tree_areas.contains(&t.0));
    let mut publish_waits = Vec::new();
    let mut ends = false;
    let mut round_left = Vec::new();
    if let Some(rd) = rounds.current {
        let last = rd.last;
        // (A region of it no longer done, its recipe edited since, or done again with units built
        // since, waits for another.)
        let publish: Vec<&Region> = regions.iter().filter(|r| done_now(r) && of_round(r)).collect();
        let now = |t: &(String, String)| last || publish.iter().any(|r| r.areas.contains(&t.0));
        let trees_due = |t: &(String, String)| last || publish.iter().any(|r| r.tree_areas.contains(&t.0));
        let slope_now: Vec<(String, String)>;
        let trees_now: Vec<(String, String)>;
        (slope_now, slope) = slope.into_iter().partition(now);
        (trees_now, trees) = trees.into_iter().partition(trees_due);
        publish_waits = slope_now.iter().map(|t| ("slope".to_string(), t.0.clone())).chain(trees_now.iter().map(|t| ("trees".to_string(), t.0.clone()))).collect();
        let before = work.len();
        push(&mut work, "slope", slope_now);
        push(&mut work, "trees", trees_now);
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
    } else {
        let (prep, rest): (Vec<_>, Vec<_>) = slope.into_iter().partition(slope_waits);
        push(&mut work, "slope", prep);
        slope = rest;
        let (prep, rest): (Vec<_>, Vec<_>) = trees.into_iter().partition(trees_waits);
        push(&mut work, "trees", prep);
        trees = rest;
    }
    work.extend(by_region);
    push(&mut work, "slope", slope);
    push(&mut work, "trees", trees);
    work.extend(chains(last_now));
    Plan { work, ready, publish_waits, regions: lefts, begins: None, ends, round_left }
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
fn prune_units(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, units: &[(Unit, String)]) -> Option<Work> {
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
            "pois" | "peaks" | "terrain" | "slope" | "unit" | "pack" if n > 1 => format!("{}: {n} areas", label(s)),
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
pub const PUBLISH: &str = "Publishing the new map data";

/// Marks each step whose jobs a helper may do: all of them, or which.
pub fn mark_shared(steps: &mut [Step]) {
    fn noun(s: &str) -> &str {
        match s {
            "pois" => "candidates",
            "unit" => "areas",
            "trees" => "tree cover",
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
        (TREES, &["trees"]),
        (UNITS, &["unit"]),
        (TILES, &["pack", "lo"]),
        (ROADS, &["prune", "roadunits", "stations", "ferries", "terrain-root", "slope-root"]),
        (TRAINS, &["rail-feeds", "rail"]),
        (LANDMARKS, &["pois", "peaks", "items", "heritage", "marks", "overlays"]),
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
/// tiles, the road index, rail stops and ferries, the world-level terrain and slope), the trains'
/// and the landmarks', for the forecast (crate::agent::forecast).
pub fn chains_left(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, reach: Option<&Reaches>) -> [Vec<Work>; 3] {
    [remaining(done, |d| roads_chain(date, m, d, inputs, reach)), remaining(done, |d| rail_chain(cov, date, m, d, inputs)), remaining(done, |d| landmarks_chain(cov, date, m, d))]
}

/// The regions' build to the end, step by step (the pass's own steps are the agent's): the
/// heritage sites, terrain, slope, tree cover, the areas, the map tiles, the road index, rail stops
/// and ferries, trains a day, the landmarks, publishing.
/// `held`: the catalog is held for review (inputs/hold-catalog): publishing is its held copy.
/// `ready`: the regions a catalog would record as built now (`Plan::ready`).
pub fn checklist(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, held: bool, reach: Option<&Reaches>, ready: &[String]) -> Vec<Step> {
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
    out.push(per(TREES, &["trees"], &crate::treepacks::targets(cov, m), &done.trees, "tiles", true));
    let pieces = m.keys().any(|l| l.starts_with(&format!("sources/osm/{date}/pieces/")));
    let units: Vec<(String, String)> = unit_keys(cov, date, m, reach, inputs).into_iter().map(|(u, k)| (u.slash(), k)).collect();
    out.push(per(UNITS, &["unit"], &units, &done.unit, "areas", pieces && reach.is_some()));
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
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;
    use crate::reach::{LongWay, Reach};

    fn e7box(w: f64, s: f64, e: f64, n: f64) -> [i32; 4] {
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
        super::plan(c, date, m, done, inputs, Some(&reach()), Rounds { each: &each, on_map, since_last: since_publish, current: None, held: false })
    }
    fn unit_keys(c: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
        super::unit_keys(c, date, m, Some(&reach()), &BTreeMap::new())
    }
    fn checklist(c: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, held: bool) -> Vec<Step> {
        let ready = plan_with(c, date, m, done, inputs, &BTreeMap::new(), None).ready;
        super::checklist(c, date, m, done, inputs, held, Some(&reach()), &ready)
    }

    fn cov() -> Coverage {
        let d = tempfile::tempdir().unwrap();
        Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:-21.9,64.13,20".into()] }], None, d.path()).unwrap()
    }

    /// The heritage-sites job's inputs (the pass's areas set, the registers' snapshot).
    /// What the units wait for besides terrain and slope: the heritage-sites job's inputs, and the
    /// release's roadside buildings.
    fn unit_inputs(m: &mut BTreeMap<String, String>, date: &str) {
        m.insert(crate::osmpass::set_name(date, "areas"), format!("sources/osm/{date}/sets/areas.1212121212121212.osm.pbf"));
        m.insert("sources/registers/legacy".into(), "sources/registers/legacy.3434343434343434.tar.zst".into());
        m.insert(crate::buildtiles::index_logical(), format!("{}.6767676767676767.json", crate::buildtiles::index_logical()));
    }

    /// The heritage-sites job done: its key recorded, its outputs in the manifest.
    fn heritage_done(m: &mut BTreeMap<String, String>, done: &mut Keys, date: &str, w: &Work) {
        assert_eq!(w.step, "heritage-sites");
        done.record(&w.step, &w.targets);
        m.insert(crate::heritage::base_logical(date, "heritage-sources"), format!("work/heritage/{date}/base/heritage-sources.5656565656565656.json"));
    }

    /// The rest of the heritage chain done too (it runs beside the regions' work from the start:
    /// the tests of the regions' order leave it out).
    fn heritage_chain_done(c: &Coverage, m: &BTreeMap<String, String>, done: &mut Keys, date: &str) {
        let k = heritage_key(c, date, m).unwrap();
        done.record("heritage", &[("heritage".to_string(), k)]);
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
        // The tree cover layers of the coverage's z3 tile, before the units.
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!((w[0].step.as_str(), w[0].targets[0].0.as_str()), ("trees", "3/3/2"));
        done.record("trees", &w[0].targets);
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
            let w = super::plan(&c, "d", &m, &done, &BTreeMap::new(), Some(&reach), Rounds { each: &c.by_region(), on_map: &BTreeMap::new(), since_last: None, current: None, held: false }).work;
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
        let plan = |on_map: &BTreeMap<String, bool>| super::plan(&c, "d", &m, &done, &BTreeMap::new(), Some(&reach), Rounds { each: &each, on_map, since_last: None, current: None, held: false });
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
        let plan = |m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>, since: Option<u64>| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), Rounds { each: &each, on_map, since_last: since, current: None, held: false });
        // (A step's works one after another, as one: the plan lists a step's by region.)
        let steps = |p: &Plan| {
            let mut v: Vec<String> = p.work.iter().map(|w| w.step.clone()).collect();
            v.dedup();
            v
        };
        let key = |m: &BTreeMap<String, String>, u: &str| super::unit_keys(&c, "d", m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
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
        done.record("trees", &p.work[1].targets);
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
        let plan = |m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), Rounds { each: &each, on_map, since_last: None, current: None, held: false });
        let key = |m: &BTreeMap<String, String>, u: &str| super::unit_keys(&c, "d", m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        let build = |m: &mut BTreeMap<String, String>, done: &mut Keys, u: &str| {
            done.record("unit", &[(u.to_string(), key(m, u))]);
            let d = u.replace('/', "-");
            m.insert(format!("base/{d}"), format!("base/{d}.6666666666666666.base"));
            m.insert(format!("global/roads/{d}"), format!("global/roads/{d}.7777777777777777.roads"));
        };
        // Through a round's work to its map tiles: what it draws.
        let drawn = |m: &BTreeMap<String, String>, done: &mut Keys, on_map: &BTreeMap<String, bool>| -> Vec<String> {
            loop {
                let p = plan(m, done, on_map);
                let w = &p.work[0];
                match w.step.as_str() {
                    "pack" => return w.targets.iter().map(|t| t.0.clone()).collect(),
                    "slope" | "trees" | "roadunits" => done.record(&w.step, &w.targets),
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
        let plan = |m: &BTreeMap<String, String>, done: &Keys, on_map: &BTreeMap<String, bool>, since: Option<u64>, current: Option<&Round>| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), Rounds { each: &each, on_map, since_last: since, current, held: false });
        let key = |m: &BTreeMap<String, String>, u: &str| super::unit_keys(&c, "d", m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
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
            assert!(["slope", "trees", "roadunits", "pack", "lo", "stations", "terrain-root", "catalog"].contains(&w.step.as_str()), "{:?}", p.work);
            if w.step == "catalog" {
                assert_eq!(p.ready, ["a"]);
                assert_eq!(w.targets[0].1, catalog_key(&then, &BTreeMap::new(), &["a".to_string()]));
            }
            if w.step == "pack" {
                // (Keyed on the units as they were.)
                let (packs, _) = pack_lo_targets(&then, Some(&reach));
                assert!(w.targets.iter().all(|t| packs.contains(t)), "{:?}", w.targets);
            }
            done.record(&w.step, &w.targets);
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
        super::plan(c, "d", m, done, &BTreeMap::new(), Some(reach), Rounds { each: &each, on_map, since_last: since, current, held })
    }
    fn rbuild(c: &Coverage, reach: &Reaches, m: &mut BTreeMap<String, String>, done: &mut Keys, u: &str, content: &str) {
        let k = super::unit_keys(c, "d", m, Some(reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        done.record("unit", &[(u.to_string(), k)]);
        let d = u.replace('/', "-");
        m.insert(format!("base/{d}"), format!("base/{d}.{content}.base"));
        m.insert(format!("global/roads/{d}"), format!("global/roads/{d}.{content}.roads"));
    }
    /// A round's work to its catalog, each step done as the plan lists it (`skip`: work that fails
    /// each time it runs, passed over as the agent passes over work waiting out a failure): the
    /// plan the catalog came in.
    fn through_its_catalog(c: &Coverage, reach: &Reaches, m: &BTreeMap<String, String>, done: &mut Keys, r: &Round, held: bool, skip: &dyn Fn(&Work) -> bool) -> Plan {
        for _ in 0..30 {
            let p = rplan(c, reach, m, done, &BTreeMap::new(), None, Some(r), held);
            let w = p.work.iter().find(|w| !skip(w) && (AS_OF_STEPS.contains(&w.step.as_str()) || w.step == "prune" || p.publish_waits.iter().any(|(s, t)| *s == w.step && w.targets.iter().any(|x| x.0 == *t)))).cloned().expect("the round's work");
            if w.step == "catalog" {
                done.record(if held { "catalog-held" } else { "catalog" }, &w.targets);
                return p;
            }
            done.record(&w.step, &w.targets);
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
        let p = through_its_catalog(&c, &reach, &m, &mut done, &r, false, &|w| w.step == "slope");
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
        let p = through_its_catalog(&c, &reach, &m, &mut done, &r, false, &|_| false);
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
        through_its_catalog(&c, &reach, &m, &mut done, &r, true, &|_| false);
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
        let k = super::unit_keys(&c, "d", &m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == "6/28/16").unwrap().1;
        done.record("unit", &[("6/28/16".to_string(), k)]);
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        // A round began ten minutes ago: a's slope and tree cover first, then the units; no round.
        let p = super::plan(&c, "d", &m, &done, &BTreeMap::new(), Some(&reach), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: Some(600), current: None, held: false });
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
        let key = |m: &BTreeMap<String, String>, u: &str| super::unit_keys(&c, "d", m, Some(&reach), &BTreeMap::new()).into_iter().find(|(x, _)| x.slash() == u).unwrap().1;
        done.record("unit", &[("6/28/16".to_string(), key(&m, "6/28/16"))]);
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        let packs = pack_lo_targets(&m, Some(&reach)).0;
        let now = packs.iter().find(|t| t.0 == "6/29/17").unwrap().1.clone();
        let (halo, owners) = now.split_once('.').unwrap();
        let drawn = |done: &mut Keys| -> Vec<String> {
            loop {
                let p = super::plan(&c, "d", &m, done, &BTreeMap::new(), Some(&reach), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: None, current: None, held: false });
                let w = &p.work[0];
                match w.step.as_str() {
                    "pack" => return w.targets.iter().map(|t| t.0.clone()).collect(),
                    "slope" | "trees" | "roadunits" => done.record(&w.step, &w.targets),
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
        let plan = |m: &BTreeMap<String, String>, done: &Keys| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: None, current: None, held: false }).work;
        let mut done = Keys::default();
        let w = plan(&m, &done);
        heritage_done(&mut m, &mut done, "d", &w[0]);
        heritage_chain_done(&c, &m, &mut done, "d");
        // Each region's terrain area, g's first; neither's unit until its terrain is built.
        let w = plan(&m, &done);
        let line = |w: &[Work]| w.iter().map(|x| format!("{} {}", x.step, x.targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(","))).collect::<Vec<_>>();
        assert_eq!(line(&w), ["terrain 3/2/2", "terrain 3/3/2", "trees 3/2/2,3/3/2"]);
        // g's terrain built: g's unit, ahead of a's terrain (a helper takes a's from the far end).
        done.record("terrain", &[w[0].targets[0].clone()]);
        let w = plan(&m, &done);
        assert_eq!(line(&w), ["unit 6/22/17", "terrain 3/3/2", "slope 3/2/2", "trees 3/2/2,3/3/2"]);
    }

    #[test]
    fn units_whose_piece_meets_the_coverage() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        // Terrain and slope done.
        for w in [plan(&c, "d", &m, &done, &BTreeMap::new()), {
            let mut d2 = done.clone();
            d2.record("terrain", &plan(&c, "d", &m, &done, &BTreeMap::new())[0].targets);
            plan(&c, "d", &m, &d2, &BTreeMap::new())
        }] {
            for x in &w {
                done.record(&x.step, &x.targets);
            }
        }
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
        assert_eq!(l.len(), 10);
        assert_eq!(line(&l, TRAINS).left, None, "no rail sources: not known");
        assert_eq!((line(&l, TERRAIN).done, line(&l, TERRAIN).total), (0, Some(1)));
        assert_eq!((line(&l, UNITS).done, line(&l, UNITS).total), (0, Some(1)));
        assert_eq!(line(&l, TILES).total, None, "no areas built: the tiles aren't known yet");
        assert_eq!(line(&l, SITES).left, Some(1));
        // Terrain, slope, the heritage sites and the area done.
        for step in ["heritage-sites", "terrain", "unit", "slope", "trees"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
            } else {
                done.record(&w[0].step, &w[0].targets);
            }
        }
        m.insert("base/6-28-16".into(), "base/6-28-16.6666666666666666.base".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.7777777777777777.roads".into());
        let l = checklist(&c, "d", &m, &done, &BTreeMap::new(), false);
        for what in [TERRAIN, SLOPE, SITES, UNITS] {
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
        let built: Vec<String> = super::unit_keys(&c, "d", &m, Some(&r), &BTreeMap::new()).into_iter().map(|(u, _)| u.slash()).collect();
        assert_eq!(built, vec!["6/28/16", "6/29/16"]);
        // No reaches yet: no units.
        assert!(super::unit_keys(&c, "d", &m, None, &BTreeMap::new()).is_empty());
        // An outline's change outside a unit's whole reach leaves its key; inside it, not.
        let poly = |east: f64| {
            std::fs::write(d.path().join("p.poly"), format!("p\n1\n -22.2 64.0\n -21.8 64.0\n {east} 64.3\n -22.2 64.3\nEND\nEND\n")).unwrap();
            Coverage::from_recipes(&[Recipe { id: "p".into(), name: "P".into(), outline: vec!["poly:p.poly".into()] }], None, d.path()).unwrap()
        };
        let key = |c: &Coverage| super::unit_keys(c, "d", &m, Some(&r), &BTreeMap::new()).into_iter().find(|(u, _)| u.slash() == "6/28/16").unwrap().1;
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
        let keys = |dig: &BTreeMap<String, String>| super::unit_keys(&c, "d", &m, Some(&r), dig).into_iter().map(|(u, k)| (u.slash(), k)).collect::<BTreeMap<_, _>>();
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
        for step in ["heritage-sites", "terrain", "unit", "slope", "trees"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
            } else {
                done.record(&w[0].step, &w[0].targets);
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
        for step in ["heritage-sites", "terrain", "unit", "slope", "trees"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            assert_eq!(w.last().unwrap().step, "rail-feeds", "{:?}", steps(&w));
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
                heritage_chain_done(&c, &m, &mut done, "d");
            } else {
                done.record(&w[0].step, &w[0].targets);
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
        for step in ["heritage-sites", "terrain", "unit", "slope", "trees"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
                heritage_chain_done(&c, &m, &mut done, "d");
            } else {
                done.record(&w[0].step, &w[0].targets);
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
        let plan = |m: &BTreeMap<String, String>, done: &Keys| super::plan(&c, "d", m, done, &BTreeMap::new(), Some(&reach), Rounds { each: &each, on_map: &BTreeMap::new(), since_last: None, current: None, held: false }).work;
        let line = |w: &[Work]| w.iter().map(|x| format!("{} {}", x.step, x.targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(","))).collect::<Vec<_>>();
        let mut done = Keys::default();
        // Before the heritage sites (the units wait for them): the candidates already, after the
        // terrain and tree cover.
        let w = plan(&m, &done);
        assert_eq!(line(&w), ["heritage-sites heritage-sites", "terrain 3/2/2,3/3/2", "trees 3/2/2,3/3/2", "pois 6/22/17,6/28/16"]);
        heritage_done(&mut m, &mut done, "d", &w[0]);
        // The heritage sites made: the rest of the heritage chain too, beside the candidates, both
        // after the regions' work.
        let w = plan(&m, &done);
        assert_eq!(line(&w), ["terrain 3/2/2", "terrain 3/3/2", "trees 3/2/2,3/3/2", "pois 6/22/17,6/28/16", "heritage heritage"]);
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
        assert_eq!(line(&w)[2..], ["trees 3/2/2,3/3/2", "items items", "heritage heritage"]);
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
}
