//! What the build Mac builds for the regions (docs/plan.md §6, Job keys; §8, Order): per z3 pack,
//! the terrain and then the slope of its z6 tiles near the coverage; base(U) for every unit whose
//! piece meets the coverage; pack(T) for the z6 tiles near changed units, the lo packs above them;
//! then a catalog.
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
pub const LO_V: u32 = 1;

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
    /// The served files the last catalog was made from.
    #[serde(default)]
    pub catalog: Option<String>,
    /// The same for the last catalog held for review (`inputs/hold-catalog`: written to
    /// catalog-held/, not served).
    #[serde(default)]
    pub catalog_held: Option<String>,
}

impl Keys {
    pub fn load(root: &Path) -> Keys {
        std::fs::read(root.join("state/build/jobs.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> anyhow::Result<()> {
        let p = root.join("state/build/jobs.json");
        let tmp = root.join(format!("state/build/jobs.json.{}.tmp", std::process::id()));
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    fn map(&mut self, step: &str) -> &mut BTreeMap<String, String> {
        match step {
            "terrain" => &mut self.terrain,
            "slope" => &mut self.slope,
            "unit" => &mut self.unit,
            "pois" => &mut self.pois,
            "peaks" => &mut self.peaks,
            "pack" => &mut self.pack,
            _ => &mut self.lo,
        }
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
        if step.ends_with("-root") || matches!(step, "labels" | "trailends" | "reach" | "summits" | "items" | "marks" | "roadunits" | "stations" | "ferries" | "heritage-sites" | "heritage" | "overlays") {
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
    let pieces = m.get(&format!("sources/osm/{date}/pieces"))?;
    let k = h(&[&format!("reach {}", crate::reach::REACH_V), pieces]);
    (done.lo.get("reach").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "reach".into(), targets: vec![("reach".into(), k)] })
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
        let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
        for (x, y) in crate::stage::tiles_in(6, b) {
            for layer in ["terrain", "grid-class", "grid-canopy", "grid-cover"] {
                inputs.push(get(&format!("layers/{layer}/hi/6-{x}-{y}")).to_string());
            }
            inputs.push(get(&crate::heritage::pos_logical(date, x, y)).to_string());
            inputs.push(get(&crate::heritage::areas_logical(date, x, y)).to_string());
        }
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        units.push((u, h(&refs)));
    }
    units
}

/// The heritage sites and designated areas the units read (crate::heritage), once per pass,
/// coverage and registers' snapshot.
pub const HERITAGE_SITES_V: u32 = 1;

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
/// Rail stops (pipeline::ovconv::stations_job) and ferries (ferries_job) near the built units.
pub const STATIONS_V: u32 = 1;
pub const FERRIES_V: u32 = 1;
/// The landmark points from the candidates (crate::marksjob).
pub const MARKS_V: u32 = 1;
/// The rest of the heritage chain on the heritage-sites outputs (scenic-build heritage), and the
/// area overlays from it with the marks' World Heritage dots (ovconv::overlays).
pub const HERITAGE_V: u32 = 1;
pub const OVERLAYS_V: u32 = 1;

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

/// The work there is, in order, for the coverage `cov`, the pass of `date`, the build manifest
/// `m` (logical → content) and what was done (`done`).
/// `inputs`: digests of what jobs read from `inputs/` (not in the manifest), by name:
/// "ferries-freq" (the ferry timetables).
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

pub fn plan(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, reach: Option<&Reaches>) -> Vec<Work> {
    let mut work = Vec::new();
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);

    // Terrain, then slope, per z3 pack.
    let (terrain, slope) = terrain_slope_targets(cov, m);
    let terrain: Vec<(String, String)> = terrain.into_iter().filter(|(t, k)| stale(&done.terrain, t, k)).collect();
    let slope: Vec<(String, String)> = slope.into_iter().filter(|(t, k)| stale(&done.slope, t, k)).collect();
    let had_terrain = !terrain.is_empty();
    if had_terrain {
        work.push(Work { step: "terrain".into(), targets: terrain });
    }
    // Slope waits for the terrain it reads (its key changes when terrain does).
    if !had_terrain && !slope.is_empty() {
        work.push(Work { step: "slope".into(), targets: slope });
    }
    if had_terrain {
        return work;
    }

    // The heritage sites and designated areas the units read: before them, once per pass and
    // coverage (the units wait until there are some).
    if let Some(w) = heritage_sites_work(cov, date, m, done) {
        work.push(w);
        return work;
    }
    if !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources")) {
        return work;
    }

    // base(U): the units the coverage builds, once the pass's reaches say which.
    if reach.is_none() {
        return work;
    }
    let units = unit_keys(cov, date, m, reach, inputs);
    let stale_units: Vec<(String, String)> = units.iter().filter(|(u, k)| stale(&done.unit, &u.slash(), k)).map(|(u, k)| (u.slash(), k.clone())).collect();
    if !stale_units.is_empty() {
        work.push(Work { step: "unit".into(), targets: stale_units });
        return work;
    }
    // What the coverage no longer builds leaves the manifest (and so the next catalog).
    if let Some(w) = prune_units(cov, date, m, &units) {
        work.push(w);
        return work;
    }
    // After the units, two chains that don't wait for each other: the roads', then a catalog
    // once it's done (new roads with the landmarks as they were; another catalog follows the
    // landmarks), then the landmarks'. The agent runs the first of these not waiting out a
    // failure, so a landmark job failing (Wikidata or a pageview dump down) doesn't hold up the
    // roads, nor the roads the landmarks.
    match roads_chain(date, m, done, inputs) {
        Some(w) => work.push(w),
        None => work.extend(catalog_work(m, done)),
    }
    work.extend(landmarks_chain(cov, date, m, done));
    work
}

/// pack(T)'s targets (z6 tiles within 100 km, plus the pieces' buffer, of a unit with a base pack)
/// and lo's (their z3 tiles), each with its key, done or not. Both are keyed on what they read: the
/// base packs and road values of the units within 110 km (a unit's ways reach no further), so an
/// identical rebuild of a unit (same content names) reruns neither.
fn pack_lo_targets(m: &BTreeMap<String, String>) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let base_units: Vec<(Unit, String)> = m
        .iter()
        .filter_map(|(l, c)| l.strip_prefix("base/").and_then(Unit::parse).map(|u| (u, format!("{c}+{}", get(&format!("global/roads/{}", u.dash()))))))
        .collect();
    let mut tiles: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (u, _) in &base_units {
        for t in crate::stage::tiles_in(6, crate::stage::tile_box_grown(u.z, u.x, u.y, 110.0)) {
            tiles.insert(t);
        }
    }
    // The base units whose box meets a tile's box grown by 110 km.
    let near = |z: u8, x: u32, y: u32| -> Vec<String> {
        let gb = grown_e7(z, x, y, 110.0);
        base_units
            .iter()
            .filter(|(u, _)| {
                let ub = crate::hipack::tile_bounds(u.z, u.x, u.y);
                ub[0] <= gb[2] && ub[2] >= gb[0] && ub[1] <= gb[3] && ub[3] >= gb[1]
            })
            .map(|(_, c)| c.clone())
            .collect()
    };
    let key = |head: String, ins: Vec<String>| {
        let mut all = vec![head];
        all.extend(ins);
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        h(&refs)
    };
    let packs: Vec<(String, String)> = tiles.iter().map(|&(x, y)| (format!("6/{x}/{y}"), key(format!("pack {PACK_V}"), near(6, x, y)))).collect();
    let qs: BTreeSet<(u32, u32)> = tiles.iter().map(|&(x, y)| (x >> 3, y >> 3)).collect();
    let lo = qs.into_iter().map(|(x, y)| (format!("3/{x}/{y}"), key(format!("lo {LO_V}"), near(3, x, y)))).collect();
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

/// Map tiles no unit is within 110 km of any more: pack(T)'s outputs (hidata, road and rail hi
/// packs) of tiles that aren't pack targets, and lo packs of z3 tiles that aren't lo targets, as prune
/// targets ("pack 6/x/y", "lo 3/x/y").
fn prune_tiles(m: &BTreeMap<String, String>) -> Option<Work> {
    let (packs, lo) = pack_lo_targets(m);
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
fn roads_chain(date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>) -> Option<Work> {
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);
    // Map tiles with no unit within 110 km any more (a region removed) leave the manifest.
    if let Some(w) = prune_tiles(m) {
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

    let (packs, lo) = pack_lo_targets(m);
    let packs: Vec<(String, String)> = packs.into_iter().filter(|(t, k)| stale(&done.pack, t, k)).collect();
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

/// The landmarks' chain after the units: candidates, peaks, the items' facts and pageviews, the
/// landmark points; its first stale step.
fn landmarks_chain(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
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
    // The candidates' items' facts and pageviews (network; only new items within a pass).
    let pois_now = current_pois(cov, date, m);
    if pois_now.is_empty() {
        return None;
    }
    let mut ins = vec![format!("items {ITEMS_V}"), date.to_string()];
    ins.extend(pois_now.iter().map(|s| s.to_string()));
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    let k = h(&refs);
    if done.lo.get("items").map(String::as_str) != Some(k.as_str()) {
        return Some(Work { step: "items".into(), targets: vec![("items".into(), k)] });
    }
    // The rest of the heritage chain (network), on the heritage sites.
    if m.contains_key(&crate::heritage::base_logical(date, "heritage-sources")) {
        let mut ins = vec![format!("heritage {HERITAGE_V}"), date.to_string()];
        for stem in ["heritage", "heritage-areas", "special", "indigenous", "heritage-sources"] {
            ins.push(get(&crate::heritage::base_logical(date, stem)).to_string());
        }
        for l in [crate::osmpass::set_name(date, "named"), crate::osmpass::set_name(date, "areas"), format!("sources/osm/{date}/filtered"), "sources/registers/legacy".into(), "sources/registers/legacy-seeds".into()] {
            ins.push(get(&l).to_string());
        }
        ins.push(coverage_all(cov));
        let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
        let k = h(&refs);
        if done.lo.get("heritage").map(String::as_str) != Some(k.as_str()) {
            return Some(Work { step: "heritage".into(), targets: vec![("heritage".into(), k)] });
        }
    }
    // The landmark points, from every current unit's candidates and peaks, the items' facts and
    // pageviews, the heritage sites (the files markconv reads: the pass's or today's).
    let src = heritage_src(m, date);
    let mut ins = vec![format!("marks {MARKS_V}"), get(&format!("sources/items/{date}/facts")).to_string(), get(&format!("sources/items/{date}/views")).to_string()];
    for stem in ["layer-heritage", "details-heritage", "props-heritage"] {
        ins.push(get(&format!("{src}/{stem}")).to_string());
    }
    for (u, _) in &units {
        for p in ["work/pois", "work/peaks"] {
            ins.push(get(&format!("{p}/{}", u.dash())).to_string());
        }
    }
    let refs: Vec<&str> = ins.iter().map(String::as_str).collect();
    let k = h(&refs);
    if done.lo.get("marks").map(String::as_str) != Some(k.as_str()) {
        return Some(Work { step: "marks".into(), targets: vec![("marks".into(), k)] });
    }
    // The area overlays from the pass's heritage, with the dots the marks gave the World Heritage
    // sites; their hi tiles where the units are.
    if src != crate::markconv::LEGACY {
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
        let k = h(&refs);
        if done.lo.get("overlays").map(String::as_str) != Some(k.as_str()) {
            return Some(Work { step: "overlays".into(), targets: vec![("overlays".into(), k)] });
        }
    }
    None
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
}

impl Step {
    pub fn finished(&self) -> bool {
        self.left == Some(0) || self.total.is_some_and(|t| self.done >= t && self.left.is_none())
    }
}

/// The regions' steps (build::checklist's lines), for before there's a pass to size them by.
pub fn checklist_to_come() -> Vec<Step> {
    [
        ("Terrain", &["terrain"][..]),
        ("Slope", &["slope"]),
        ("Heritage sites and designated areas", &["heritage-sites"]),
        ("Roads, elevations and scenery", &["unit"]),
        ("Map tiles", &["pack", "lo"]),
        ("Road index, rail stops, ferries, world terrain", &["roadunits", "stations", "ferries", "terrain-root", "slope-root"]),
        ("Landmarks", &["pois", "peaks", "items", "heritage", "marks", "overlays"]),
        ("Publishing the new map data", &["catalog", "catalog-held"]),
    ]
    .iter()
    .map(|(what, steps)| Step { what: what.to_string(), steps: steps.iter().map(|s| s.to_string()).collect(), ..Default::default() })
    .collect()
}

/// The works a chain would still run, one after another, as if each succeeded (its targets recorded
/// with their keys as they are now).
fn remaining(done: &Keys, next: impl Fn(&Keys) -> Option<Work>) -> Vec<Work> {
    let mut d = done.clone();
    let mut out = Vec::new();
    while let Some(w) = next(&d) {
        if out.len() >= 64 {
            break;
        }
        d.record(&w.step, &w.targets);
        out.push(w);
    }
    out
}

/// The regions' build to the end, step by step (the pass's own steps are the agent's): terrain,
/// slope, the heritage sites, the areas, the map tiles, the road index, rail stops and ferries, the
/// landmarks, publishing.
/// `held`: the catalog is held for review (inputs/hold-catalog): publishing is its held copy.
pub fn checklist(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, held: bool, reach: Option<&Reaches>) -> Vec<Step> {
    let count = |all: &[(String, String)], map: &BTreeMap<String, String>| all.iter().filter(|(t, k)| map.get(t) == Some(k)).count();
    let per = |what: &str, steps: &[&str], all: &[(String, String)], map: &BTreeMap<String, String>, unit: &str, known: bool| Step {
        what: what.into(),
        steps: steps.iter().map(|s| s.to_string()).collect(),
        done: count(all, map),
        total: known.then_some(all.len()),
        left: None,
        unit: unit.into(),
    };
    let group = |what: &str, steps: &[&str], left: Option<usize>| Step { what: what.into(), steps: steps.iter().map(|s| s.to_string()).collect(), left, ..Default::default() };
    let mut out = Vec::new();
    let (terrain, slope) = terrain_slope_targets(cov, m);
    out.push(per("Terrain", &["terrain"], &terrain, &done.terrain, "parts", true));
    out.push(per("Slope", &["slope"], &slope, &done.slope, "parts", true));
    let sites_left = heritage_sites_work(cov, date, m, done).is_some() || !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources"));
    out.push(group("Heritage sites and designated areas", &["heritage-sites"], Some(sites_left as usize)));
    let pieces = m.keys().any(|l| l.starts_with(&format!("sources/osm/{date}/pieces/")));
    let units: Vec<(String, String)> = unit_keys(cov, date, m, reach, inputs).into_iter().map(|(u, k)| (u.slash(), k)).collect();
    out.push(per("Roads, elevations and scenery", &["unit"], &units, &done.unit, "areas", pieces && reach.is_some()));
    let (packs, lo) = pack_lo_targets(m);
    let built = m.keys().any(|l| l.starts_with("base/"));
    let mut tiles = per("Map tiles", &["pack", "lo"], &packs, &done.pack, "tiles", built);
    tiles.done += count(&lo, &done.lo);
    tiles.total = tiles.total.map(|t| t + lo.len());
    out.push(tiles);
    let roads_left = remaining(done, |d| roads_chain(date, m, d, inputs)).iter().filter(|w| !matches!(w.step.as_str(), "pack" | "lo")).count();
    out.push(group("Road index, rail stops, ferries, world terrain", &["roadunits", "stations", "ferries", "terrain-root", "slope-root"], built.then_some(roads_left)));
    let landmarks = remaining(done, |d| landmarks_chain(cov, date, m, d));
    let lm_left: usize = landmarks.iter().map(|w| if matches!(w.step.as_str(), "pois" | "peaks") { w.targets.len() } else { 1 }).sum();
    out.push(group("Landmarks", &["pois", "peaks", "items", "heritage", "marks", "overlays"], pieces.then_some(lm_left)));
    let key = catalog_key(m);
    let publish_left = if held { done.catalog_held.as_deref() != Some(key.as_str()) } else { done.catalog.as_deref() != Some(key.as_str()) } as usize;
    out.push(group("Publishing the new map data", &["catalog", "catalog-held"], built.then_some(publish_left)));
    out
}

/// A catalog when what it would list has changed since the last one.
fn catalog_work(m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let k = catalog_key(m);
    (done.catalog.as_deref() != Some(k.as_str())).then(|| Work { step: "catalog".into(), targets: vec![("catalog".into(), k)] })
}

/// What a catalog would list: the served files' logical and content names, hashed.
fn catalog_key(m: &BTreeMap<String, String>) -> String {
    let served: Vec<String> = m
        .iter()
        .filter(|(l, _)| {
            ["layers/", "base/", "hidata/", "markdata/", "ovdata/", "global/"].iter().any(|p| l.starts_with(p)) || l.ends_with("/outlines")
        })
        .map(|(l, c)| format!("{l}={c}"))
        .collect();
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

    // The planner with the tests' reaches.
    fn plan(c: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>) -> Vec<Work> {
        super::plan(c, date, m, done, inputs, Some(&reach()))
    }
    fn unit_keys(c: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
        super::unit_keys(c, date, m, Some(&reach()), &BTreeMap::new())
    }
    fn checklist(c: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>, held: bool) -> Vec<Step> {
        super::checklist(c, date, m, done, inputs, held, Some(&reach()))
    }

    fn cov() -> Coverage {
        let d = tempfile::tempdir().unwrap();
        Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:-21.9,64.13,20".into()] }], None, d.path()).unwrap()
    }

    /// The heritage-sites job's inputs (the pass's areas set, the registers' snapshot).
    fn heritage_inputs(m: &mut BTreeMap<String, String>, date: &str) {
        m.insert(crate::osmpass::set_name(date, "areas"), format!("sources/osm/{date}/sets/areas.1212121212121212.osm.pbf"));
        m.insert("sources/registers/legacy".into(), "sources/registers/legacy.3434343434343434.tar.zst".into());
    }

    /// The heritage-sites job done: its key recorded, its outputs in the manifest.
    fn heritage_done(m: &mut BTreeMap<String, String>, done: &mut Keys, date: &str, w: &Work) {
        assert_eq!(w.step, "heritage-sites");
        done.record(&w.step, &w.targets);
        m.insert(crate::heritage::base_logical(date, "heritage-sources"), format!("work/heritage/{date}/base/heritage-sources.5656565656565656.json"));
    }

    #[test]
    fn terrain_first_then_slope_then_catalog() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        heritage_inputs(&mut m, "2026-09-28");
        let mut done = Keys::default();
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].step, "terrain");
        assert_eq!(w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2"]);
        done.record("terrain", &w[0].targets);
        // The terrain job's outputs.
        m.insert("layers/terrain/lo/3-3-2".into(), "layers/terrain/lo/3-3-2.1111111111111111.pack".into());
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.2222222222222222.pack".into());
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "slope");
        done.record("slope", &w[0].targets);
        // The heritage sites and areas, before any unit (there are none here).
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        heritage_done(&mut m, &mut done, "2026-09-28", &w[0]);
        // The root from the lo pack (no slope lo pack in this test: no slope root).
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "terrain-root");
        done.record("terrain-root", &w[0].targets);
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "catalog");
        done.record("catalog", &w[0].targets);
        assert!(plan(&c, "2026-09-28", &m, &done, &BTreeMap::new()).is_empty(), "nothing more to do");
        // New terrain content: slope again, then a catalog.
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.3333333333333333.pack".into());
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "slope");
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
        heritage_inputs(&mut m, "d");
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        heritage_done(&mut m, &mut done, "d", &w[0]);
        let w = plan(&c, "d", &m, &done, &BTreeMap::new());
        assert_eq!(w[0].step, "unit");
        assert_eq!(w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["6/28/16"]);
        // A unit's key names the heritage slices near it, and only those.
        let key = |m: &BTreeMap<String, String>| unit_keys(&c, "d", m)[0].1.clone();
        let k0 = key(&m);
        m.insert(crate::heritage::pos_logical("d", 40, 20), "work/heritage/d/pos/6-40-20.7878787878787878.json".into());
        assert_eq!(key(&m), k0, "a slice far away");
        m.insert(crate::heritage::areas_logical("d", 28, 16), "work/heritage/d/areas/6-28-16.9090909090909090.jsonl".into());
        assert_ne!(key(&m), k0, "its own tile's areas");
    }

    #[test]
    fn checklist_counts_to_the_end() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let line = |l: &[Step], what: &str| l.iter().find(|s| s.what.starts_with(what)).cloned().unwrap();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        heritage_inputs(&mut m, "d");
        let l = checklist(&c, "d", &m, &done, &BTreeMap::new(), false);
        assert_eq!(l.len(), 8);
        assert_eq!((line(&l, "Terrain").done, line(&l, "Terrain").total), (0, Some(1)));
        assert_eq!((line(&l, "Roads").done, line(&l, "Roads").total), (0, Some(1)));
        assert_eq!(line(&l, "Map tiles").total, None, "no areas built: the tiles aren't known yet");
        assert_eq!(line(&l, "Heritage").left, Some(1));
        // Terrain, slope, the heritage sites and the area done.
        for step in ["terrain", "slope", "heritage-sites", "unit"] {
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
        for what in ["Terrain", "Slope", "Heritage", "Roads"] {
            assert!(line(&l, what).finished(), "{what} done");
        }
        let tiles = line(&l, "Map tiles");
        assert!(tiles.total.is_some_and(|t| t > 1) && tiles.done == 0 && !tiles.finished());
        assert!(line(&l, "Road index").left.is_some_and(|n| n >= 1));
        assert_eq!(line(&l, "Publishing").left, Some(1));
        // Held for review: publishing is the held catalog.
        let k = catalog_key(&m);
        done.catalog_held = Some(k);
        assert!(line(&checklist(&c, "d", &m, &done, &BTreeMap::new(), true), "Publishing").finished());
        assert!(!line(&checklist(&c, "d", &m, &done, &BTreeMap::new(), false), "Publishing").finished());
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
        let long = |verts: &[(f64, f64)]| LongWay { owned: true, verts: verts.iter().map(|&(x, y)| [(x * 1e7) as i32, (y * 1e7) as i32]).collect() };
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
        heritage_inputs(&mut m, "d");
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        m.insert("sources/osm/d/pieces/6-40-20".into(), "sources/osm/d/pieces/6-40-20.5555555555555555.osm.pbf".into());
        for step in ["terrain", "slope", "heritage-sites", "unit"] {
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
        let lo_of = |m: &BTreeMap<String, String>, q: &str| pack_lo_targets(m).1.into_iter().find(|(t, _)| t == q).unwrap().1;
        let (near, far) = (lo_of(&m, "3/3/2"), lo_of(&m, "3/6/2"));
        // A unit rebuilt with new content: the zoomed-out tile over it is stale, the far one isn't.
        put(&mut m, "6-28-16", "3333333333333333");
        assert_ne!(lo_of(&m, "3/3/2"), near);
        assert_eq!(lo_of(&m, "3/6/2"), far);
    }

    #[test]
    fn ferries_dont_depend_on_the_built_units() {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for set in ["rail", "ferries"] {
            m.insert(crate::osmpass::set_name("d", set), format!("sources/osm/d/sets/{set}.5555555555555555.osm.pbf"));
        }
        m.insert("base/6-28-16".into(), "base/6-28-16.1111111111111111.sect".into());
        m.insert("global/roads/6-28-16".into(), "global/roads/6-28-16.1111111111111111.sect".into());
        let key = |m: &BTreeMap<String, String>, step: &str| remaining(&Keys::default(), |d| roads_chain("d", m, d, &BTreeMap::new())).into_iter().find(|w| w.step == step).unwrap().targets[0].1.clone();
        let (stations, ferries) = (key(&m, "stations"), key(&m, "ferries"));
        m.insert("base/6-40-20".into(), "base/6-40-20.2222222222222222.sect".into());
        m.insert("global/roads/6-40-20".into(), "global/roads/6-40-20.2222222222222222.sect".into());
        assert_ne!(key(&m, "stations"), stations, "stops are clipped to the built units");
        assert_eq!(key(&m, "ferries"), ferries);
    }

    #[test]
    fn roads_and_landmarks_dont_wait_for_each_other() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let steps = |w: &[Work]| w.iter().map(|x| x.step.clone()).collect::<Vec<_>>();
        // Terrain, slope, the heritage sites and the unit done.
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        heritage_inputs(&mut m, "d");
        for step in ["terrain", "slope", "heritage-sites", "unit"] {
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
        m.insert("work/trailends/d".into(), "work/trailends/d.8888888888888888.json".into());
        // Both chains' first steps; no catalog while the roads' chain has work.
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
        // The candidates made: the peaks wait for the pass's summits; nothing else to do.
        done.record("pois", &w[1].targets);
        m.insert("work/pois/6-28-16".into(), "work/pois/6-28-16.9999999999999999.json".into());
        assert!(plan(&c, "d", &m, &done, &BTreeMap::new()).is_empty());
        m.insert("work/summits/d".into(), "work/summits/d.aaaaaaaaaaaaaaaa.bin".into());
        assert_eq!(steps(&plan(&c, "d", &m, &done, &BTreeMap::new())), vec!["peaks"]);
    }
}
