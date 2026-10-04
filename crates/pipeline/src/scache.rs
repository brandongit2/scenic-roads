//! Caches that let the scenic steps redo only what is new when regions are added (or OSM
//! updated).
//!
//! A step's previous outputs are still in the build directory when it starts (outputs are swapped
//! in at the end), so the cache only records what each row of them was: the key of each road
//! sample (`<step>.keys`, in the previous run's sample order) and the analysis-grid tiles
//! (`<step>.tiles`, in the previous grid order). A sample whose key is found takes its previous
//! row; a grid tile found takes its previous values.
//!
//! A sample's key hashes its position, eye height and flags (and whatever else the step's result
//! depends on, via `mix`), so an unchanged road keeps its results and a moved or new one misses.
//! Results also depend on the terrain, canopy and land cover around a sample: those only change
//! where the analysis grid gains tiles (a new region or new roads), so a previous result is not
//! used when a grid tile within reach is new (`GridChange::near`).
//!
//! Files live in data/cache/scenic (outside the build). A unit's build folder starts empty each
//! run, so its results are kept between runs by `Carry`.

use anyhow::{Context, Result};
use roadcore::scenic::Sample;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Bumped when what the canopy or view step computes changes: results kept from before are then
/// not reused (`Carry`).
pub const SCENIC_V: u32 = 1;

/// The cache directory (data/cache/scenic, next to the build directory; `SCENIC_SCACHE` when set,
/// as a unit's build folder sets it to its own).
pub fn dir(build: &Path) -> PathBuf {
    let d = match std::env::var_os("SCENIC_SCACHE") {
        Some(d) => PathBuf::from(d),
        None => build.parent().unwrap_or(Path::new(".")).join("cache/scenic"),
    };
    let _ = std::fs::create_dir_all(&d);
    d
}

/// A unit's build folder's cache (its steps' `SCENIC_SCACHE`).
pub fn unit_dir(build: &Path) -> PathBuf {
    build.join("scache")
}

/// Key of a road sample: position (1e-7°), eye height (dm) and flags.
pub fn sample_key(s: &Sample) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in [s.lon as u32 as u64, s.lat as u32 as u64, ((s.eye * 10.0).round() as i32) as u32 as u64, s.flags as u64] {
        h ^= v;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        h ^= h >> 29;
    }
    h
}

/// Mix extra bytes (inputs a result depends on) into a key.
pub fn mix(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ (h >> 31)
}

/// The previous run's sample keys, in its order: key → row.
pub struct Prev {
    sorted: Vec<(u64, u32)>,
    n: usize,
}

impl Prev {
    /// Load `<step>.keys` (empty when missing).
    pub fn load(cache_dir: &Path, step: &str) -> Prev {
        let keys: Vec<u64> = std::fs::read(cache_dir.join(format!("{step}.keys")))
            .ok()
            .filter(|b| b.len() % 8 == 0)
            .map(|b| bytemuck::cast_slice::<u8, u64>(&b).to_vec())
            .unwrap_or_default();
        let n = keys.len();
        let mut sorted: Vec<(u64, u32)> = keys.into_iter().enumerate().map(|(i, k)| (k, i as u32)).collect();
        sorted.sort_unstable();
        Prev { sorted, n }
    }

    /// Rows in the previous run.
    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The previous row of a key.
    pub fn row(&self, k: u64) -> Option<usize> {
        let i = self.sorted.partition_point(|e| e.0 < k);
        (i < self.sorted.len() && self.sorted[i].0 == k).then(|| self.sorted[i].1 as usize)
    }

    pub fn save(cache_dir: &Path, step: &str, keys: &[u64]) -> Result<()> {
        let p = cache_dir.join(format!("{step}.keys"));
        std::fs::write(p.with_extension("tmp"), bytemuck::cast_slice(keys))?;
        std::fs::rename(p.with_extension("tmp"), p)?;
        Ok(())
    }
}

/// The analysis-grid tiles of a step's previous run, and which of today's are new.
pub struct GridChange {
    prev: Vec<[u32; 2]>,
    new: HashSet<[u32; 2]>,
    /// No previous run: everything is new.
    pub first: bool,
}

impl GridChange {
    pub fn load(cache_dir: &Path, step: &str, tiles: &[[u32; 2]]) -> GridChange {
        let own = cache_dir.join(format!("{step}.tiles"));
        let mut c = match std::fs::read(&own).ok().filter(|b| b.len() % 8 == 0 && !b.is_empty()) {
            Some(b) => {
                let prev = bytemuck::cast_slice::<u8, [u32; 2]>(&b).to_vec();
                let ps: HashSet<[u32; 2]> = prev.iter().copied().collect();
                GridChange { new: tiles.iter().filter(|t| !ps.contains(*t)).copied().collect(), prev, first: false }
            }
            None => GridChange { prev: Vec::new(), new: HashSet::new(), first: true },
        };
        // Terrain tiles repaired in place since this step's last run (terrain.rs): their grid
        // tiles count as new (z11 as itself, z12 as its parent).
        let steps = cache_dir.parent().unwrap_or(Path::new(".")).join("steps");
        for k in roadcore::archive::terrain_repaired_since(&steps, &own) {
            let (z, x, y) = ((k >> 58) as u8, ((k >> 29) & 0x1fff_ffff) as u32, (k & 0x1fff_ffff) as u32);
            match z {
                11 => c.new.insert([x, y]),
                12 => c.new.insert([x >> 1, y >> 1]),
                _ => false,
            };
        }
        c
    }

    pub fn save(cache_dir: &Path, step: &str, tiles: &[[u32; 2]]) -> Result<()> {
        std::fs::write(cache_dir.join(format!("{step}.tiles")), bytemuck::cast_slice(tiles))?;
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.new.len()
    }

    /// Previous slot of each tile (for copying last run's grid values), by tile.
    pub fn prev_slots(&self) -> HashMap<[u32; 2], usize> {
        self.prev.iter().enumerate().map(|(i, t)| (*t, i)).collect()
    }

    pub fn prev_len(&self) -> usize {
        self.prev.len()
    }

    /// Whether a new grid tile lies within `ring` z11 tiles of the point.
    pub fn near(&self, lon: f64, lat: f64, ring: i64) -> bool {
        if self.first {
            return true;
        }
        if self.new.is_empty() {
            return false;
        }
        let (gx, gy) = roadcore::grid::cell_of(lon, lat);
        let (tx, ty) = ((gx / 256.0).floor() as i64, (gy / 256.0).floor() as i64);
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                let (x, y) = (tx + dx, ty + dy);
                if x >= 0 && y >= 0 && self.new.contains(&[x as u32, y as u32]) {
                    return true;
                }
            }
        }
        false
    }
}

/// A unit's scenic results kept between its runs (in the agent's cache), so a rerun (a new pass,
/// a region nearby changed) redoes only the samples that are new or near what changed:
/// - kept after a run: the canopy and view steps' sample keys and grid tiles, their per-sample
///   outputs, the canopy and cover grids (zstd), and `basis`, what the unit read of the map's
///   layers then (per z6 tile, the terrain and grid packs' content names);
/// - restored before the next run's canopy step, as that step's previous run, with the grid tiles
///   under z6 tiles whose packs have changed since counted as new (`GridChange`), so samples
///   near them are done again. Results kept under another `SCENIC_V` aren't used.
pub struct Carry {
    pub dir: PathBuf,
    /// "x-y" (z6) → the content names of its packs the unit reads.
    pub basis: BTreeMap<String, String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    v: u32,
    basis: BTreeMap<String, String>,
}

const KEPT_OUTPUTS: [&str; 3] = ["near.i8", "roadside.u8", "samples.metrics.u8"];
const KEPT_GRIDS: [&str; 2] = ["grid.canopy.u8", "grid.cover.u8"];
const KEPT_CACHE: [&str; 4] = ["canopy.keys", "canopy.tiles", "view.keys", "view.tiles"];

impl Carry {
    /// Keeps the results of the run in `build` (replacing what was kept), or drops what was kept
    /// when the run made none (a unit left without ways).
    pub fn save(&self, build: &Path) -> Result<()> {
        let cache = unit_dir(build);
        let have = KEPT_OUTPUTS.iter().chain(&KEPT_GRIDS).all(|f| build.join(f).exists()) && KEPT_CACHE.iter().all(|f| cache.join(f).exists());
        let tmp = self.dir.with_extension("tmp");
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp)?;
        }
        if !have {
            if self.dir.exists() {
                std::fs::remove_dir_all(&self.dir)?;
            }
            return Ok(());
        }
        std::fs::create_dir_all(&tmp)?;
        for f in KEPT_OUTPUTS {
            std::fs::copy(build.join(f), tmp.join(f)).with_context(|| format!("keep {f}"))?;
        }
        for f in KEPT_CACHE {
            std::fs::copy(cache.join(f), tmp.join(f)).with_context(|| format!("keep {f}"))?;
        }
        for f in KEPT_GRIDS {
            let raw = std::fs::read(build.join(f))?;
            std::fs::write(tmp.join(format!("{f}.zst")), zstd::bulk::compress(&raw, 3)?)?;
        }
        std::fs::write(tmp.join("basis.json"), serde_json::to_vec(&Kept { v: SCENIC_V, basis: self.basis.clone() })?)?;
        if self.dir.exists() {
            std::fs::remove_dir_all(&self.dir)?;
        }
        std::fs::rename(&tmp, &self.dir)?;
        Ok(())
    }

    /// Puts the kept results into `build` as the canopy and view steps' previous run, with the
    /// grid tiles under changed packs counted as new. The samples kept, or None when nothing
    /// usable was kept.
    pub fn restore(&self, build: &Path) -> Result<Option<usize>> {
        let Some(kept) = std::fs::read(self.dir.join("basis.json")).ok().and_then(|b| serde_json::from_slice::<Kept>(&b).ok()) else { return Ok(None) };
        if kept.v != SCENIC_V {
            return Ok(None);
        }
        let cache = unit_dir(build);
        std::fs::create_dir_all(&cache)?;
        for f in KEPT_OUTPUTS {
            std::fs::copy(self.dir.join(f), build.join(f)).with_context(|| format!("restore {f}"))?;
        }
        for f in KEPT_CACHE {
            std::fs::copy(self.dir.join(f), cache.join(f)).with_context(|| format!("restore {f}"))?;
        }
        for f in KEPT_GRIDS {
            let z = std::fs::read(self.dir.join(format!("{f}.zst")))?;
            std::fs::write(build.join(f), zstd::stream::decode_all(&z[..])?)?;
        }
        // The z6 tiles whose packs differ from then: their grid tiles are new.
        let changed: HashSet<[u32; 2]> = self
            .basis
            .iter()
            .filter(|(t, names)| kept.basis.get(*t) != Some(*names))
            .map(|(t, _)| t)
            .chain(kept.basis.keys().filter(|t| !self.basis.contains_key(*t)))
            .filter_map(|t| t.split_once('-').and_then(|(x, y)| Some([x.parse().ok()?, y.parse().ok()?])))
            .collect();
        for step in ["canopy", "view"] {
            let p = cache.join(format!("{step}.tiles"));
            let mut tiles: Vec<[u32; 2]> = bytemuck::cast_slice::<u8, [u32; 2]>(&std::fs::read(&p)?).to_vec();
            for t in tiles.iter_mut() {
                if changed.contains(&[t[0] >> 5, t[1] >> 5]) {
                    *t = [u32::MAX, u32::MAX];
                }
            }
            std::fs::write(&p, bytemuck::cast_slice(&tiles))?;
        }
        Ok(Some(std::fs::metadata(cache.join("canopy.keys"))?.len() as usize / 8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_with(d: &Path, keys: &[u64], tiles: &[[u32; 2]]) {
        let cache = d.join("scache");
        std::fs::create_dir_all(&cache).unwrap();
        for f in KEPT_OUTPUTS {
            std::fs::write(d.join(f), format!("{f} of {} samples", keys.len())).unwrap();
        }
        for f in KEPT_GRIDS {
            std::fs::write(d.join(f), vec![7u8; tiles.len() * 16]).unwrap();
        }
        for step in ["canopy", "view"] {
            Prev::save(&cache, step, keys).unwrap();
            GridChange::save(&cache, step, tiles).unwrap();
        }
    }

    #[test]
    fn a_rerun_takes_the_last_runs_results() {
        let d = tempfile::tempdir().unwrap();
        let (run1, run2, kept) = (d.path().join("run1"), d.path().join("run2"), d.path().join("kept/6-32-21"));
        // Grid tiles under z6 tiles 32/21 and 33/21.
        let tiles = [[32 * 32 + 1, 21 * 32 + 1], [33 * 32, 21 * 32 + 5]];
        build_with(&run1, &[11, 22, 33], &tiles);
        let basis = |a: &str| -> BTreeMap<String, String> { [("32-21".to_string(), a.to_string()), ("33-21".to_string(), "b1".to_string())].into() };
        Carry { dir: kept.clone(), basis: basis("a1") }.save(&run1).unwrap();
        // The next run, in a new folder: the same packs, so nothing is new.
        std::fs::create_dir_all(&run2).unwrap();
        assert_eq!(Carry { dir: kept.clone(), basis: basis("a1") }.restore(&run2).unwrap(), Some(3));
        assert_eq!(std::fs::read_to_string(run2.join("near.i8")).unwrap(), "near.i8 of 3 samples");
        assert_eq!(std::fs::read(run2.join("grid.cover.u8")).unwrap(), vec![7u8; 32]);
        let prev = Prev::load(&run2.join("scache"), "view");
        assert_eq!(prev.row(22), Some(1));
        let ch = GridChange::load(&run2.join("scache"), "canopy", &tiles);
        assert_eq!(ch.count(), 0);
        // 32/21's packs changed since: its grid tile is new, 33/21's isn't.
        let run3 = d.path().join("run3");
        std::fs::create_dir_all(&run3).unwrap();
        Carry { dir: kept.clone(), basis: basis("a2") }.restore(&run3).unwrap();
        let ch = GridChange::load(&run3.join("scache"), "canopy", &tiles);
        assert_eq!(ch.count(), 1);
        assert_eq!(ch.prev_len(), 2, "the kept grids' slots stay");
        assert!(ch.prev_slots().contains_key(&tiles[1]) && !ch.prev_slots().contains_key(&tiles[0]));
    }

    #[test]
    fn nothing_kept_or_another_version_is_a_first_run() {
        let d = tempfile::tempdir().unwrap();
        let kept = d.path().join("kept");
        let run = d.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        assert_eq!(Carry { dir: kept.clone(), basis: BTreeMap::new() }.restore(&run).unwrap(), None);
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(kept.join("basis.json"), serde_json::to_vec(&Kept { v: SCENIC_V + 1, basis: BTreeMap::new() }).unwrap()).unwrap();
        assert_eq!(Carry { dir: kept.clone(), basis: BTreeMap::new() }.restore(&run).unwrap(), None);
        // A run that made no scenic results drops what was kept.
        Carry { dir: kept.clone(), basis: BTreeMap::new() }.save(&run).unwrap();
        assert!(!kept.exists());
    }
}
