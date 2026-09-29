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
//! Files live in data/cache/scenic (outside the build).

use anyhow::Result;
use roadcore::scenic::Sample;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The cache directory (data/cache/scenic, next to the build directory).
pub fn dir(build: &Path) -> PathBuf {
    let d = build.parent().unwrap_or(Path::new(".")).join("cache/scenic");
    let _ = std::fs::create_dir_all(&d);
    d
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
        match std::fs::read(cache_dir.join(format!("{step}.tiles"))).ok().filter(|b| b.len() % 8 == 0 && !b.is_empty()) {
            Some(b) => {
                let prev = bytemuck::cast_slice::<u8, [u32; 2]>(&b).to_vec();
                let ps: HashSet<[u32; 2]> = prev.iter().copied().collect();
                GridChange { new: tiles.iter().filter(|t| !ps.contains(*t)).copied().collect(), prev, first: false }
            }
            None => GridChange { prev: Vec::new(), new: HashSet::new(), first: true },
        }
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
