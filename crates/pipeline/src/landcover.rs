//! Land cover on the z11 analysis grid (`grid.idx`) from ESA WorldCover 2021 (10 m): the
//! `landcover` program. WorldCover's 1/4 overview (~40 m) is read over each grid tile
//! and sampled at the cells' centres (the pixel they fall in); its classes collapse to
//! `roadcore::grid::class`: 1 trees (and mangroves), 2 shrub, 3 open (grass, crop, bare,
//! moss/lichen), 4 built-up, 5 water, 6 herbaceous wetland, 7 snow/ice, 0 no data.
//!
//! With `only` (a unit's build): just the grid slots listed (grid.idx order: the tiles the packs
//! lack), the rest of grid.class.u8 kept as staged. Without: every tile, those of the last run
//! copied from its grid.class.u8 (its tile order in `<build>/../cache/steps/landcover.tiles`).

use crate::fetch::Fetch;
use crate::geotiff::Tiff;
use anyhow::{bail, Context, Result};
use det::Det;
use rayon::prelude::*;
use roadcore::grid::{CELLS, TS, WORLD};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub fn url(tile: &str) -> String {
    format!("https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_{tile}_Map.tif")
}

/// WorldCover's classes to the grid's.
const LUT: [u8; 256] = {
    let mut l = [0u8; 256];
    let pairs = [(10, 1), (95, 1), (20, 2), (30, 3), (40, 3), (60, 3), (100, 3), (50, 4), (80, 5), (90, 6), (70, 7)];
    let mut i = 0;
    while i < pairs.len() {
        l[pairs[i].0] = pairs[i].1;
        i += 1;
    }
    l
};

/// A WorldCover tile's name (3° squares, by their south-west corner).
pub fn tile_name(lat0: i64, lon0: i64) -> String {
    crate::dem::fabdem::name(lat0, lon0)
}

/// WorldCover's tiles, opened as they're needed (None: the open sea, where there's none).
pub struct WorldCover<'a> {
    fetch: &'a dyn Fetch,
    open: Mutex<BTreeMap<String, Option<Arc<Tiff>>>>,
}

/// Decoded overview tiles kept for the grid tiles beside them, per WorldCover tile.
const TILE_CACHE: usize = 24 << 20;

impl<'a> WorldCover<'a> {
    pub fn new(fetch: &'a dyn Fetch) -> Self {
        WorldCover { fetch, open: Mutex::new(BTreeMap::new()) }
    }

    fn tile(&self, name: &str) -> Result<Option<Arc<Tiff>>> {
        if let Some(t) = self.open.lock().unwrap().get(name) {
            return Ok(t.clone());
        }
        let u = url(name);
        let t = match self.fetch.open(&u)? {
            None => None,
            Some(src) => {
                let t = Tiff::open(src).with_context(|| u.clone())?.with_cache(TILE_CACHE);
                t.level(2).with_context(|| u.clone())?;
                Some(Arc::new(t))
            }
        };
        Ok(self.open.lock().unwrap().entry(name.to_string()).or_insert(t).clone())
    }

    /// Grid tile `tx`, `ty`'s classes, row by row.
    pub fn classes(&self, tx: u32, ty: u32) -> Result<Vec<u8>> {
        let g = |i: usize| i as f64 + 0.5;
        let lon: Vec<f64> = (0..TS).map(|j| ((tx as u64 * TS as u64) as f64 + g(j)) / WORLD * 360.0 - 180.0).collect();
        let lat: Vec<f64> = (0..TS).map(|i| (std::f64::consts::PI * (1.0 - 2.0 * ((ty as u64 * TS as u64) as f64 + g(i)) / WORLD)).dsinh().datan().to_degrees()).collect();
        let lat0: Vec<i64> = lat.iter().map(|&v| (v / 3.0).floor() as i64 * 3).collect();
        let lon0: Vec<i64> = lon.iter().map(|&v| (v / 3.0).floor() as i64 * 3).collect();
        let uniq = |v: &[i64]| -> Vec<i64> {
            let mut u = v.to_vec();
            u.sort_unstable();
            u.dedup();
            u
        };
        let mut out = vec![0u8; CELLS];
        for la in uniq(&lat0) {
            for lo in uniq(&lon0) {
                let rows_in: Vec<usize> = (0..TS).filter(|&i| lat0[i] == la).collect();
                let cols_in: Vec<usize> = (0..TS).filter(|&j| lon0[j] == lo).collect();
                if rows_in.is_empty() || cols_in.is_empty() {
                    continue;
                }
                let Some(d) = self.tile(&tile_name(la, lo))? else { continue };
                let img = d.level(2)?;
                let t = d.transform(2)?;
                let cols: Vec<i64> = cols_in.iter().map(|&j| ((lon[j] - t[0]) / t[1]) as i64).collect();
                let rows: Vec<i64> = rows_in.iter().map(|&i| ((lat[i] - t[3]) / t[5]) as i64).collect();
                let c0 = (*cols.iter().min().unwrap()).max(0);
                let c1 = (*cols.iter().max().unwrap()).min(img.width as i64 - 1);
                let r0 = (*rows.iter().min().unwrap()).max(0);
                let r1 = (*rows.iter().max().unwrap()).min(img.height as i64 - 1);
                if c1 < c0 || r1 < r0 {
                    continue;
                }
                let (w, h) = (c1 - c0 + 1, r1 - r0 + 1);
                let a = d.read_window(2, c0 as u32, r0 as u32, w as u32, h as u32)?;
                for (&i, &r) in rows_in.iter().zip(&rows) {
                    let rr = (r - r0).clamp(0, h - 1);
                    for (&j, &c) in cols_in.iter().zip(&cols) {
                        let cc = (c - c0).clamp(0, w - 1);
                        out[i * TS + j] = LUT[a[(rr * w + cc) as usize] as u8 as usize];
                    }
                }
            }
        }
        Ok(out)
    }
}

/// Counts of each class (0 to 7).
pub type Counts = [u64; 8];

fn count(cells: &[u8], counts: &mut Counts) {
    for &c in cells {
        counts[(c as usize).min(7)] += 1;
    }
}

/// The classes' shares, printed.
pub fn report(counts: &Counts) {
    let names = ["none", "trees", "shrub", "open", "built", "water", "wetland", "snow"];
    let tot = counts.iter().sum::<u64>().max(1);
    let parts: Vec<String> = names.iter().zip(counts).map(|(n, &c)| format!("'{n}': '{:.1} %'", c as f64 / tot as f64 * 100.0)).collect();
    println!("classes: {{{}}}", parts.join(", "));
}

fn read_tiles(b: &Path) -> Result<Vec<[u32; 2]>> {
    let raw = std::fs::read(b.join("grid.idx")).context("grid.idx")?;
    Ok(bytemuck::pod_collect_to_vec(&raw[..raw.len() / 8 * 8]))
}

/// Classifies `slots` (grid.idx order) into a copy of the staged grid.class.u8, with `workers`
/// threads; the classes of those slots.
pub fn only(b: &Path, slots: &[u32], fetch: &dyn Fetch, workers: usize) -> Result<Counts> {
    let tiles = read_tiles(b)?;
    let mut out = std::fs::read(b.join("grid.class.u8")).context("grid.class.u8")?;
    if out.len() != tiles.len() * CELLS {
        bail!("grid.class.u8 has {} cells for {} grid tiles", out.len(), tiles.len());
    }
    if let Some(&m) = slots.iter().max() {
        if m as usize >= tiles.len() {
            bail!("slot {m} beyond the {} grid tiles", tiles.len());
        }
    }
    println!("land cover: {} grid tiles, {} from the packs, {} to classify", tiles.len(), tiles.len() as i64 - slots.len() as i64, slots.len());
    classify(&WorldCover::new(fetch), &tiles, slots, &mut out, workers)?;
    let tmp = b.join("grid.class.u8.tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, b.join("grid.class.u8"))?;
    let mut counts = [0u64; 8];
    for &s in slots {
        count(&out[s as usize * CELLS..(s as usize + 1) * CELLS], &mut counts);
    }
    report(&counts);
    Ok(counts)
}

/// Every grid tile's classes (the last run's tiles copied from its grid.class.u8), with
/// `workers` threads; the classes of them all.
pub fn full(b: &Path, fetch: &dyn Fetch, workers: usize) -> Result<Counts> {
    let tiles = read_tiles(b)?;
    let mut out = vec![0u8; tiles.len() * CELLS];
    let cache = b.parent().unwrap_or(Path::new(".")).join("cache").join("steps").join("landcover.tiles");
    std::fs::create_dir_all(cache.parent().unwrap())?;
    let mut todo: Vec<u32> = (0..tiles.len() as u32).collect();
    if let (Ok(prev), Ok(old)) = (std::fs::read(&cache), std::fs::read(b.join("grid.class.u8"))) {
        let prev: Vec<[u32; 2]> = bytemuck::pod_collect_to_vec(&prev[..prev.len() / 8 * 8]);
        if old.len() == prev.len() * CELLS {
            let slot: BTreeMap<[u32; 2], usize> = prev.iter().enumerate().map(|(i, &t)| (t, i)).collect();
            todo.clear();
            for (i, t) in tiles.iter().enumerate() {
                match slot.get(t) {
                    Some(&j) => out[i * CELLS..(i + 1) * CELLS].copy_from_slice(&old[j * CELLS..(j + 1) * CELLS]),
                    None => todo.push(i as u32),
                }
            }
        }
    }
    println!("land cover: {} grid tiles, {} from the last run, {} to classify", tiles.len(), tiles.len() - todo.len(), todo.len());
    classify(&WorldCover::new(fetch), &tiles, &todo, &mut out, workers)?;
    let tmp = b.join("grid.class.u8.tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, b.join("grid.class.u8"))?;
    std::fs::write(&cache, bytemuck::cast_slice::<[u32; 2], u8>(&tiles))?;
    let mut counts = [0u64; 8];
    count(&out, &mut counts);
    report(&counts);
    Ok(counts)
}

/// Grid tiles `slots` classified into their places in `out` (65,536 cells a slot).
fn classify(wc: &WorldCover, tiles: &[[u32; 2]], slots: &[u32], out: &mut [u8], workers: usize) -> Result<()> {
    let want: std::collections::BTreeSet<u32> = slots.iter().copied().collect();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(workers.max(1)).build().ok();
    crate::dem::sample::within(pool.as_ref(), || {
        out.par_chunks_mut(CELLS).enumerate().filter(|(s, _)| want.contains(&(*s as u32))).try_for_each(|(s, cells)| -> Result<()> {
            cells.copy_from_slice(&wc.classes(tiles[s][0], tiles[s][1])?);
            Ok(())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::MapFetch;
    use crate::geotiff::testtiff::{make_images, Opts};

    #[test]
    fn classes_from_worldcover_tiles() {
        // A WorldCover tile N45W069 (3°; here 1/400° pixels, so overview level 1 has 1/100°): 10
        // (trees) west of 67.6° W, 80 (water) east.
        let gt = [-69.0, 0.0025, 0.0, 48.0, 0.0, -0.0025];
        let sizes = [(1200u32, 1200u32), (600, 600), (300, 300)];
        let o = Opts { tile: Some(128), compression: 8, kind: (1, 8), ..Opts::default() };
        let (b, _) = make_images(&sizes, o, gt, Some("0"), &|k, x, _y, _| {
            let lon = -69.0 + (x as f64 + 0.5) * 0.0025 * (1u32 << k) as f64;
            if lon < -67.6 { 10.0 } else { 80.0 }
        });
        let mut m = MapFetch::default();
        m.0.insert(url("N45W069"), Some(b));
        let wc = WorldCover::new(&m);
        // z11 tile 639, 724: 67.68° to 67.5° W, around 46.5° N.
        let (tx, ty) = (639, 724);
        let c = wc.classes(tx, ty).unwrap();
        let lon_of = |j: usize| ((tx as u64 * 256) as f64 + j as f64 + 0.5) / WORLD * 360.0 - 180.0;
        let mut seen = [false; 2];
        for j in 0..256 {
            let want = if lon_of(j) < -67.6 { 1 } else { 5 };
            seen[(want == 5) as usize] = true;
            assert_eq!(c[128 * 256 + j], want, "column {j} at {}", lon_of(j));
        }
        assert!(seen[0] && seen[1]);
        // Over the open sea: no data.
        let mut sea = MapFetch::default();
        sea.0.insert(url("N45W069"), None);
        assert!(WorldCover::new(&sea).classes(tx, ty).unwrap().iter().all(|&c| c == 0));

        // A unit's grid: the slots listed classified, the rest as staged.
        let d = tempfile::tempdir().unwrap();
        let tiles: [[u32; 2]; 3] = [[639, 724], [640, 724], [639, 725]];
        std::fs::write(d.path().join("grid.idx"), bytemuck::cast_slice(&tiles)).unwrap();
        std::fs::write(d.path().join("grid.class.u8"), vec![9u8; 3 * CELLS]).unwrap();
        let counts = only(d.path(), &[2, 0], &m, 2).unwrap();
        assert_eq!(counts.iter().sum::<u64>(), 2 * CELLS as u64);
        let g = std::fs::read(d.path().join("grid.class.u8")).unwrap();
        assert_eq!(&g[..CELLS], &c[..]);
        assert!(g[CELLS..2 * CELLS].iter().all(|&v| v == 9));
        assert!(g[2 * CELLS..].iter().all(|&v| v == 1 || v == 5));
        assert!(only(d.path(), &[3], &m, 2).unwrap_err().to_string().contains("beyond the 3 grid tiles"));
        std::fs::write(d.path().join("grid.class.u8"), vec![9u8; CELLS]).unwrap();
        assert!(only(d.path(), &[0], &m, 2).unwrap_err().to_string().contains("cells for 3 grid tiles"));
    }
}
