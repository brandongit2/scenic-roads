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
/// lo's own version: lo isn't keyed on pack's, so a change in what they share (hipack's tiling)
/// bumps both.
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
        let hs = crate::handoff::waiting(root)?;
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
        // the same, from the terrain here and fixed datasets.
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

    // The heritage sites and designated areas the units read: first, once per pass and coverage (one
    // job, so the M1's helper builds units as soon as the terrain is there, while the build Mac runs
    // slope and tree cover). Not waited for: terrain still runs while it waits out a failure.
    let sites = heritage_sites_work(cov, date, m, done);
    let sites_pending = sites.is_some();
    work.extend(sites);

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
    // The tree cover layers per z3 tile, before the units: one catalog then has them all, and the
    // canopy squares the jobs fetch (kept on the NAS) are there when the units read them. (Listed,
    // not waited for: units still run while a trees job waits out a failure.)
    work.extend(trees_work(cov, m, done));

    // The units wait for the heritage sites.
    if sites_pending || !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources")) {
        return work;
    }

    // base(U): the units the coverage builds, once the pass's reaches say which.
    if reach.is_none() {
        return work;
    }
    // The roadside buildings the units read: the release's tiles (a worldwide job, once).
    if !m.contains_key(&crate::buildtiles::index_logical()) {
        return work;
    }
    let units = unit_keys(cov, date, m, reach, inputs);
    let mut stale_units: Vec<(Unit, String)> = units.iter().filter(|(u, k)| stale(&done.unit, &u.slash(), k)).cloned().collect();
    // Neighbours together (by the 10° canopy square they're in, then by tile), so the downloads and
    // caches one unit fills serve the next (the manifest's order, "6-1…", "6-10…", "6-2…", jumps
    // about the globe).
    stale_units.sort_by_key(|(u, _)| spatial_order(*u));
    let stale_units: Vec<(String, String)> = stale_units.into_iter().map(|(u, k)| (u.slash(), k)).collect();
    if !stale_units.is_empty() {
        work.push(Work { step: "unit".into(), targets: stale_units });
        return work;
    }
    // What the coverage no longer builds leaves the manifest (and so the next catalog).
    if let Some(w) = prune_units(cov, date, m, &units) {
        work.push(w);
        return work;
    }
    // After the units, three chains that don't wait for each other: the roads', then a catalog
    // once it's done (new roads with the landmarks and trains a day as they were; another catalog
    // follows each of the others), then the rail service's, then the landmarks'. The agent runs
    // the first of these not waiting out a failure, so a landmark or rail feed job failing
    // (Wikidata, a pageview dump or an operator's server down) doesn't hold up the roads, nor the
    // roads the others.
    match roads_chain(date, m, done, inputs, reach) {
        Some(w) => work.push(w),
        None => work.extend(catalog_work(m, done, inputs)),
    }
    work.extend(rail_chain(cov, date, m, done, inputs));
    work.extend(landmarks_chain(cov, date, m, done));
    work
}

/// Where a unit comes in a run of units: by the 10° square its tile's centre is in (column, then
/// row from the north), then by tile.
fn spatial_order(u: Unit) -> (i32, i32, u32, u32) {
    let b = crate::hipack::tile_bounds(u.z, u.x, u.y);
    let (lon, lat) = ((b[0] as f64 + b[2] as f64) / 2.0 * 1e-7, (b[1] as f64 + b[3] as f64) / 2.0 * 1e-7);
    ((lon / 10.0).floor() as i32, -(lat / 10.0).floor() as i32, u.x, u.y)
}

/// The tree cover layers of the z3 tiles whose coverage changed (crate::treepacks).
fn trees_work(cov: &Coverage, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let stale: Vec<(String, String)> = crate::treepacks::targets(cov, m).into_iter().filter(|(t, k)| done.trees.get(t) != Some(k)).collect();
    (!stale.is_empty()).then(|| Work { step: "trees".into(), targets: stale })
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
    // The units whose extent meets a tile's box grown by `km` (pack's halo: what it reads).
    let near = |z: u8, x: u32, y: u32| -> Vec<String> {
        let km = if z == 6 { 100.0 } else { 0.0 };
        let gb = crate::hipack::grow(crate::hipack::tile_bounds(z, x, y), km);
        base_units.iter().filter(|(_, _, e)| e[0] <= gb[2] && e[2] >= gb[0] && e[1] <= gb[3] && e[3] >= gb[1]).map(|(_, c, _)| c.clone()).collect()
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
        ("Heritage sites and designated areas", &["heritage-sites"][..]),
        ("Terrain", &["terrain"]),
        ("Slope", &["slope"]),
        ("Tree cover", &["trees"]),
        ("Roads, elevations and scenery", &["unit"]),
        ("Map tiles", &["pack", "lo"]),
        ("Road index, rail stops, ferries, world terrain", &["roadunits", "stations", "ferries", "terrain-root", "slope-root"]),
        ("Trains a day", &["rail-feeds", "rail"]),
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

/// The regions' build to the end, step by step (the pass's own steps are the agent's): the
/// heritage sites, terrain, slope, tree cover, the areas, the map tiles, the road index, rail stops
/// and ferries, trains a day, the landmarks, publishing.
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
    let sites_left = heritage_sites_work(cov, date, m, done).is_some() || !m.contains_key(&crate::heritage::base_logical(date, "heritage-sources"));
    out.push(group("Heritage sites and designated areas", &["heritage-sites"], Some(sites_left as usize)));
    out.push(per("Terrain", &["terrain"], &terrain, &done.terrain, "parts", true));
    out.push(per("Slope", &["slope"], &slope, &done.slope, "parts", true));
    out.push(per("Tree cover", &["trees"], &crate::treepacks::targets(cov, m), &done.trees, "tiles", true));
    let pieces = m.keys().any(|l| l.starts_with(&format!("sources/osm/{date}/pieces/")));
    let units: Vec<(String, String)> = unit_keys(cov, date, m, reach, inputs).into_iter().map(|(u, k)| (u.slash(), k)).collect();
    out.push(per("Roads, elevations and scenery", &["unit"], &units, &done.unit, "areas", pieces && reach.is_some()));
    let (packs, lo) = pack_lo_targets(m, reach);
    let built = m.keys().any(|l| l.starts_with("base/"));
    let mut tiles = per("Map tiles", &["pack", "lo"], &packs, &done.pack, "tiles", built);
    tiles.done += count(&lo, &done.lo);
    tiles.total = tiles.total.map(|t| t + lo.len());
    out.push(tiles);
    let roads_left = remaining(done, |d| roads_chain(date, m, d, inputs, reach)).iter().filter(|w| !matches!(w.step.as_str(), "pack" | "lo")).count();
    out.push(group("Road index, rail stops, ferries, world terrain", &["roadunits", "stations", "ferries", "terrain-root", "slope-root"], built.then_some(roads_left)));
    // (A run of rail-feeds is followed by rail, whose key reads what it writes. Unknown while the
    // chain can't go on: before the rail sources are seeded, while inputs/keys.env can't be read,
    // without the feeds' list or the pass's rail set.)
    let rail_left = rail_next(cov, date, m, done, inputs).map(|w| w.map_or(0, |w| if w.step == "rail-feeds" { 2 } else { 1 }));
    out.push(group("Trains a day", &["rail-feeds", "rail"], rail_left));
    let landmarks = remaining(done, |d| landmarks_chain(cov, date, m, d));
    let lm_left: usize = landmarks.iter().map(|w| if matches!(w.step.as_str(), "pois" | "peaks") { w.targets.len() } else { 1 }).sum();
    out.push(group("Landmarks", &["pois", "peaks", "items", "heritage", "marks", "overlays"], pieces.then_some(lm_left)));
    let key = catalog_key(m, inputs);
    let publish_left = if held { done.catalog_held.as_deref() != Some(key.as_str()) } else { done.catalog.as_deref() != Some(key.as_str()) } as usize;
    out.push(group("Publishing the new map data", &["catalog", "catalog-held"], built.then_some(publish_left)));
    out
}

/// A catalog when what it would list or record has changed since the last one (not while the
/// regions can't be read: `inputs` "regions" "?").
fn catalog_work(m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>) -> Option<Work> {
    if inputs.get("regions").map(String::as_str) == Some("?") {
        return None;
    }
    let k = catalog_key(m, inputs);
    (done.catalog.as_deref() != Some(k.as_str())).then(|| Work { step: "catalog".into(), targets: vec![("catalog".into(), k)] })
}

/// What a catalog would list and record, hashed: the served files' logical and content names, and
/// the regions (`inputs` "regions": their recipes and outline files), so a region renamed, or drawn
/// inside another, gets a catalog that records it.
fn catalog_key(m: &BTreeMap<String, String>, inputs: &BTreeMap<String, String>) -> String {
    let mut served: Vec<String> = m
        .iter()
        .filter(|(l, _)| {
            ["layers/", "base/", "hidata/", "markdata/", "ovdata/", "global/"].iter().any(|p| l.starts_with(p)) || l.ends_with("/outlines")
        })
        .map(|(l, c)| format!("{l}={c}"))
        .collect();
    served.push(format!("regions {}", inputs.get("regions").map(String::as_str).unwrap_or("-")));
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

    #[test]
    fn terrain_first_then_slope_then_catalog() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        unit_inputs(&mut m, "2026-09-28");
        let mut done = Keys::default();
        // The heritage sites first (one job: a helper's units then wait only for the terrain).
        let w = plan(&c, "2026-09-28", &m, &done, &BTreeMap::new());
        assert_eq!(w.iter().map(|x| x.step.as_str()).collect::<Vec<_>>(), vec!["heritage-sites", "terrain"]);
        heritage_done(&mut m, &mut done, "2026-09-28", &w[0]);
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
    fn checklist_counts_to_the_end() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let line = |l: &[Step], what: &str| l.iter().find(|s| s.what.starts_with(what)).cloned().unwrap();
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        unit_inputs(&mut m, "d");
        let l = checklist(&c, "d", &m, &done, &BTreeMap::new(), false);
        assert_eq!(l.len(), 10);
        assert_eq!(line(&l, "Trains a day").left, None, "no rail sources: not known");
        assert_eq!((line(&l, "Terrain").done, line(&l, "Terrain").total), (0, Some(1)));
        assert_eq!((line(&l, "Roads, elevations").done, line(&l, "Roads, elevations").total), (0, Some(1)));
        assert_eq!(line(&l, "Map tiles").total, None, "no areas built: the tiles aren't known yet");
        assert_eq!(line(&l, "Heritage").left, Some(1));
        // Terrain, slope, the heritage sites and the area done.
        for step in ["heritage-sites", "terrain", "slope", "trees", "unit"] {
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
        let k = catalog_key(&m, &BTreeMap::new());
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
        for step in ["heritage-sites", "terrain", "slope", "trees", "unit"] {
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
        // Not before the units.
        for step in ["heritage-sites", "terrain", "slope", "trees", "unit"] {
            let w = plan(&c, "d", &m, &done, &BTreeMap::new());
            assert_eq!(w[0].step, step);
            assert!(!w.iter().any(|x| x.step.starts_with("rail")), "{:?}", steps(&w));
            if step == "heritage-sites" {
                heritage_done(&mut m, &mut done, "d", &w[0]);
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
        let line = |m: &BTreeMap<String, String>, done: &Keys| checklist(&c, "d", m, done, &BTreeMap::new(), false).into_iter().find(|s| s.what == "Trains a day").unwrap();
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
        let line = |m: &BTreeMap<String, String>, done: &Keys, inputs: &BTreeMap<String, String>| checklist(&c, "d", m, done, inputs, false).into_iter().find(|s| s.what == "Trains a day").unwrap();
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
        for step in ["heritage-sites", "terrain", "slope", "trees", "unit"] {
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
        // The candidates made: the peaks wait for the pass's summits; nothing else to do.
        done.record("pois", &w[1].targets);
        m.insert("work/pois/6-28-16".into(), "work/pois/6-28-16.9999999999999999.json".into());
        assert!(plan(&c, "d", &m, &done, &BTreeMap::new()).is_empty());
        m.insert("work/summits/d".into(), "work/summits/d.aaaaaaaaaaaaaaaa.bin".into());
        assert_eq!(steps(&plan(&c, "d", &m, &done, &BTreeMap::new())), vec!["peaks"]);
    }
}
