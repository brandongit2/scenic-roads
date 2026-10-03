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
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Step versions: bumping one rebuilds that step everywhere (oldest first, when idle).
pub const TERRAIN_V: u32 = 1;
pub const SLOPE_V: u32 = 1;
/// 2: elevations up to 6,053 m (`final.u16`, base packs' `elevu`; were clamped at ±3,200 m).
pub const UNIT_V: u32 = 2;
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
    #[serde(default)]
    pub pack: BTreeMap<String, String>,
    #[serde(default)]
    pub lo: BTreeMap<String, String>,
    /// The served files the last catalog was made from.
    #[serde(default)]
    pub catalog: Option<String>,
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
            "pack" => &mut self.pack,
            _ => &mut self.lo,
        }
    }

    /// Records a job's targets as done with their keys.
    pub fn record(&mut self, step: &str, done: &[(String, String)]) {
        if step == "catalog" {
            self.catalog = done.first().map(|d| d.1.clone());
            return;
        }
        if step.ends_with("-root") || matches!(step, "labels" | "trailends") {
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

/// The coverage near a box (w, s, e, n, E7): the shapes meeting it, by source and rings.
fn cov_fp(cov: &Coverage, b: [i32; 4]) -> String {
    let mut v: Vec<String> = cov
        .shapes
        .iter()
        .filter(|s| s.bbox[0] <= b[2] && s.bbox[2] >= b[0] && s.bbox[1] <= b[3] && s.bbox[3] >= b[1])
        .map(|s| format!("{}:{}", s.source, store::naming::hash16(bytemuck::cast_slice(&s.rings.concat()))))
        .collect();
    v.sort();
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

/// The labels by importance, worldwide, once per pass (or labels step version): independent of the
/// regions.
pub const LABELS_V: u32 = 1;

pub fn labels_work(date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Option<Work> {
    let set = m.get(&crate::osmpass::set_name(date, "labels"))?;
    let k = h(&[&format!("labels {LABELS_V}"), set]);
    (done.lo.get("labels").map(String::as_str) != Some(k.as_str())).then(|| Work { step: "labels".into(), targets: vec![("labels".into(), k)] })
}

/// The units whose piece meets the coverage, each with its key: what it reads (its piece and road
/// values, the coverage near it, the heritage sites, the staged layers near it as the manifest has
/// them, which is what the unit step stages from).
pub fn unit_keys(cov: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let mut units: Vec<(Unit, String)> = Vec::new();
    for (l, c) in m.range(format!("sources/osm/{date}/pieces/")..) {
        let Some(u) = l.strip_prefix(&format!("sources/osm/{date}/pieces/")).and_then(Unit::parse) else { break };
        let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
        if !cov.meets_box(tb) {
            continue;
        }
        // (The heritage sites: the flags step's.)
        let mut inputs = vec![
            format!("unit {UNIT_V}"),
            c.clone(),
            get(&format!("sources/osm/{date}/roads/{}", u.dash())).to_string(),
            cov_fp(cov, grown_e7(u.z, u.x, u.y, 10.0)),
            get("global/legacy/heritage").to_string(),
        ];
        let b = crate::stage::tile_box_grown(u.z, u.x, u.y, crate::stage::MARGIN_KM);
        for (x, y) in crate::stage::tiles_in(6, b) {
            for layer in ["terrain", "grid-class", "grid-areas", "grid-canopy", "grid-cover"] {
                inputs.push(get(&format!("layers/{layer}/hi/6-{x}-{y}")).to_string());
            }
        }
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        units.push((u, h(&refs)));
    }
    units
}

/// The candidates' version (crate::candidates, extract `--candidates`): bumping it makes every
/// unit's candidates again, not the units.
pub const POIS_V: u32 = 1;

/// The units whose piece meets the coverage, each with its candidates' key: its piece, the
/// coverage over it (the clip) and the pass's hiking-route ends.
pub fn pois_keys(cov: &Coverage, date: &str, m: &BTreeMap<String, String>) -> Vec<(Unit, String)> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let ends = get(&format!("work/trailends/{date}"));
    let mut units = Vec::new();
    for (l, c) in m.range(format!("sources/osm/{date}/pieces/")..) {
        let Some(u) = l.strip_prefix(&format!("sources/osm/{date}/pieces/")).and_then(Unit::parse) else { break };
        let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
        if !cov.meets_box(tb) {
            continue;
        }
        units.push((u, h(&[&format!("pois {POIS_V}"), c, &cov_fp(cov, tb), ends])));
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

pub fn region_states(cov: &Coverage, regions: &[(String, Coverage)], date: &str, m: &BTreeMap<String, String>, done: &Keys) -> BTreeMap<String, RegionState> {
    let keys = unit_keys(cov, date, m);
    regions
        .iter()
        .map(|(id, rc)| {
            let mine: Vec<&(Unit, String)> = keys.iter().filter(|(u, _)| rc.meets_box(crate::hipack::tile_bounds(u.z, u.x, u.y))).collect();
            let built = mine.iter().filter(|(u, k)| done.unit.get(&u.slash()) == Some(k)).count();
            (id.clone(), RegionState { built, total: mine.len() })
        })
        .collect()
}

/// The work there is, in order, for the coverage `cov`, the pass of `date`, the build manifest
/// `m` (logical → content) and what was done (`done`).
pub fn plan(cov: &Coverage, date: &str, m: &BTreeMap<String, String>, done: &Keys) -> Vec<Work> {
    let get = |l: &str| m.get(l).map(String::as_str).unwrap_or("-");
    let by_q = coverage_tiles(cov);
    let mut work = Vec::new();
    let stale = |map: &BTreeMap<String, String>, t: &str, k: &str| map.get(t).map(String::as_str) != Some(k);

    // Terrain, then slope, per z3 pack.
    let mut terrain = Vec::new();
    let mut slope = Vec::new();
    for (q, ts) in &by_q {
        let qs = format!("3/{}/{}", q.0, q.1);
        let tlist: Vec<String> = ts.iter().map(|t| format!("6/{}/{}", t.0, t.1)).collect();
        let k = h(&[&format!("terrain {TERRAIN_V}"), &tlist.join(" "), &cov_fp(cov, grown_e7(3, q.0, q.1, 20.0))]);
        if stale(&done.terrain, &qs, &k) {
            terrain.push((qs.clone(), k));
        }
        // Slope reads the terrain packs of q (as they are now; a terrain job changes them first).
        let mut inputs = vec![format!("slope {SLOPE_V}"), get(&format!("layers/terrain/lo/3-{}-{}", q.0, q.1)).to_string()];
        inputs.extend(ts.iter().map(|t| get(&format!("layers/terrain/hi/6-{}-{}", t.0, t.1)).to_string()));
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let k = h(&refs);
        if stale(&done.slope, &qs, &k) {
            slope.push((qs, k));
        }
    }
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

    // base(U): units whose piece meets the coverage.
    let units = unit_keys(cov, date, m);
    let stale_units: Vec<(String, String)> = units.iter().filter(|(u, k)| stale(&done.unit, &u.slash(), k)).map(|(u, k)| (u.slash(), k.clone())).collect();
    if !stale_units.is_empty() {
        work.push(Work { step: "unit".into(), targets: stale_units });
        return work;
    }
    // Their landmark candidates (once the pass's hiking-route ends exist).
    if m.contains_key(&format!("work/trailends/{date}")) {
        let stale_pois: Vec<(String, String)> = pois_keys(cov, date, m).into_iter().filter(|(u, k)| stale(&done.pois, &u.slash(), k)).map(|(u, k)| (u.slash(), k)).collect();
        if !stale_pois.is_empty() {
            work.push(Work { step: "pois".into(), targets: stale_pois });
            return work;
        }
    }

    // pack(T): z6 tiles within 100 km (plus the pieces' buffer) of a unit with a base pack.
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
    let mut packs = Vec::new();
    let mut los: BTreeMap<(u32, u32), Vec<String>> = BTreeMap::new();
    for &(x, y) in &tiles {
        let gb = grown_e7(6, x, y, 110.0);
        let mut inputs = vec![format!("pack {PACK_V}")];
        for (u, c) in &base_units {
            let ub = crate::hipack::tile_bounds(u.z, u.x, u.y);
            if ub[0] <= gb[2] && ub[2] >= gb[0] && ub[1] <= gb[3] && ub[3] >= gb[1] {
                inputs.push(c.clone());
            }
        }
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let k = h(&refs);
        let ts = format!("6/{x}/{y}");
        if stale(&done.pack, &ts, &k) {
            packs.push((ts, k.clone()));
        }
        los.entry((x >> 3, y >> 3)).or_default().push(k);
    }
    if !packs.is_empty() {
        work.push(Work { step: "pack".into(), targets: packs });
        return work;
    }
    let mut lo = Vec::new();
    for (q, ks) in los {
        let mut inputs = vec![format!("lo {LO_V}")];
        inputs.extend(ks);
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let k = h(&refs);
        let qs = format!("3/{}/{}", q.0, q.1);
        if stale(&done.lo, &qs, &k) {
            lo.push((qs, k));
        }
    }
    if !lo.is_empty() {
        work.push(Work { step: "lo".into(), targets: lo });
        return work;
    }

    // The terrain and slope roots (z0–2), from their lo packs.
    for (layer, step) in [("terrain", "terrain-root"), ("slope", "slope-root")] {
        let mut inputs = vec![format!("{step} 1")];
        inputs.extend(m.range(format!("layers/{layer}/lo/")..).take_while(|(l, _)| l.starts_with(&format!("layers/{layer}/lo/"))).map(|(_, c)| c.clone()));
        if inputs.len() == 1 {
            continue;
        }
        let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        let k = h(&refs);
        if done.lo.get(step).map(String::as_str) != Some(k.as_str()) {
            work.push(Work { step: step.into(), targets: vec![(step.to_string(), k)] });
            return work;
        }
    }

    // A catalog when what it would list has changed since the last one.
    let served: Vec<String> = m
        .iter()
        .filter(|(l, _)| {
            ["layers/", "base/", "hidata/", "markdata/", "ovdata/", "global/"].iter().any(|p| l.starts_with(p)) || l.ends_with("/outlines")
        })
        .map(|(l, c)| format!("{l}={c}"))
        .collect();
    let refs: Vec<&str> = served.iter().map(String::as_str).collect();
    let k = h(&refs);
    if done.catalog.as_deref() != Some(k.as_str()) {
        work.push(Work { step: "catalog".into(), targets: vec![("catalog".into(), k)] });
    }
    work
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::recipes::Recipe;

    fn cov() -> Coverage {
        let d = tempfile::tempdir().unwrap();
        Coverage::from_recipes(&[Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:-21.9,64.13,20".into()] }], None, d.path()).unwrap()
    }

    #[test]
    fn terrain_first_then_slope_then_catalog() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        let w = plan(&c, "2026-09-28", &m, &done);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].step, "terrain");
        assert_eq!(w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["3/3/2"]);
        done.record("terrain", &w[0].targets);
        // The terrain job's outputs.
        m.insert("layers/terrain/lo/3-3-2".into(), "layers/terrain/lo/3-3-2.1111111111111111.pack".into());
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.2222222222222222.pack".into());
        let w = plan(&c, "2026-09-28", &m, &done);
        assert_eq!(w[0].step, "slope");
        done.record("slope", &w[0].targets);
        // The root from the lo pack (no slope lo pack in this test: no slope root).
        let w = plan(&c, "2026-09-28", &m, &done);
        assert_eq!(w[0].step, "terrain-root");
        done.record("terrain-root", &w[0].targets);
        let w = plan(&c, "2026-09-28", &m, &done);
        assert_eq!(w[0].step, "catalog");
        done.record("catalog", &w[0].targets);
        assert!(plan(&c, "2026-09-28", &m, &done).is_empty(), "nothing more to do");
        // New terrain content: slope again, then a catalog.
        m.insert("layers/terrain/hi/6-28-16".into(), "layers/terrain/hi/6-28-16.3333333333333333.pack".into());
        let w = plan(&c, "2026-09-28", &m, &done);
        assert_eq!(w[0].step, "slope");
    }

    #[test]
    fn units_whose_piece_meets_the_coverage() {
        let c = cov();
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        let mut done = Keys::default();
        // Terrain and slope done.
        for w in [plan(&c, "d", &m, &done), {
            let mut d2 = done.clone();
            d2.record("terrain", &plan(&c, "d", &m, &done)[0].targets);
            plan(&c, "d", &m, &d2)
        }] {
            for x in &w {
                done.record(&x.step, &x.targets);
            }
        }
        m.insert("sources/osm/d/pieces/6-28-16".into(), "sources/osm/d/pieces/6-28-16.4444444444444444.osm.pbf".into());
        m.insert("sources/osm/d/pieces/6-40-20".into(), "sources/osm/d/pieces/6-40-20.5555555555555555.osm.pbf".into());
        let w = plan(&c, "d", &m, &done);
        assert_eq!(w[0].step, "unit");
        assert_eq!(w[0].targets.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["6/28/16"]);
    }
}
