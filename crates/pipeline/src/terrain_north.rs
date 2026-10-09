//! The terrain north of 60°N from Copernicus DEM GLO-30 (docs/plan.md §6, Terrain; the NAS's
//! `sources/copernicus-dem/NOTES.md`). AWS's Terrain Tiles mix sources there: one with ellipsoidal
//! heights (ArcticDEM's, most likely) beside sea-level ones, so lakes and patches stand the geoid's
//! height (10–50 m) above or below the land around them in steps ("Minecraft" terraces, plateaus),
//! and water missing from the detailed source is filled from a coarse one that blurs the hills into
//! it. GLO-30 is one source, heights above EGM2008 (sea level), 30 m, its water bodies flattened.
//!
//! Each terrain tile of z9 and finer (coarser ones are made from them) is resampled from GLO-30's 1° tiles (bilinear between its pixel centres, the
//! mean of k × k such samples over a pixel wider than 30 m), and blended into AWS's (repaired) tile
//! by latitude: AWS's alone south of 59.5°N, GLO-30's alone from 60°N, a smoothstep between (55 km:
//! the band lies south of 60°, where AWS's sources are sea-level ones too, so the two agree to a few
//! metres and the blend shows no seam; north of 60° AWS's ellipsoidal heights would put a step of
//! the geoid's height into any band). Where GLO-30 was filled from an ancillary DEM (its FLM, 3 and
//! up: ASTER, GMTED2010, AW3D30 … in steep mountains and on ice), AWS's tile is taken instead,
//! moved by the median difference between the two over the tile's unfilled pixels (so its heights
//! are GLO-30's datum's), in proportion to the samples filled. A 1° cell GLO-30 doesn't list is the
//! open sea (0 m); one it lists that the store hasn't (and can't fetch) leaves AWS's tile as it is.
//!
//! GLO-30 is a surface model: the boreal forest reads 1.4–2.7 m higher than FABDEM's bare earth at
//! the median and 4–8 m at p90 (four cells in Alaska, Yukon and the NWT), soft patches in a 3×
//! hillshade; FABDEM, which takes them off, draws blocky stair-steps instead over flat ground (the
//! Tanana flats), stops at 80°N and comes at ~0.4 MB/s. GLO-30 it is (8 October).

use anyhow::{Context, Result};
use det::Det;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// GLO-30 alone from here north (°).
pub const NORTH: f64 = 60.0;
/// AWS's alone south of here (°): GLO-30 blended in between, by a smoothstep.
pub const SOUTH: f64 = 59.5;
/// The most samples a side over a pixel (k × k).
const MAX_K: usize = 6;
/// GLO-30's spacing north–south (m), for how many samples a pixel takes.
const SPACING_M: f64 = 30.87;
/// An elevation GLO-30 never has: its nodata, or no tile.
const BAD: f32 = -1000.0;

/// GLO-30's share of a pixel at latitude `lat`: 0 south of SOUTH, 1 from NORTH, a smoothstep
/// between.
pub fn weight(lat: f64) -> f32 {
    let t = ((lat - SOUTH) / (NORTH - SOUTH)).clamp(0.0, 1.0);
    (t * t * (3.0 - 2.0 * t)) as f32
}

/// One of GLO-30's 1° tiles, decoded: 3600 rows (row r's centre at `lat0 + 1 − r/3600`), `w`
/// columns (column c's centre at `lon0 + c/w`: PixelIsPoint), and which pixels were filled from an
/// ancillary DEM.
pub struct Cell {
    pub w: usize,
    pub e: Vec<f32>,
    pub filled: Vec<bool>,
}

pub const ROWS: usize = 3600;

/// A 1° cell as the source has it.
#[derive(Clone)]
pub enum Got {
    /// GLO-30 has no tile there: the open sea.
    Sea,
    /// It has one, but not here: AWS's terrain stays.
    Missing,
    Cell(Arc<Cell>),
}

/// GLO-30's 1° tiles by their south-west corner (latitude, longitude, whole degrees).
pub trait Cells: Sync {
    fn cell(&self, lat0: i32, lon0: i32) -> Got;
    /// What pins the source (for the terrain's key): its name and version.
    fn pin(&self) -> String;
}

/// A tile name's stem, as the bucket names it: `Copernicus_DSM_COG_10_N64_00_W148_00`.
pub fn stem(lat0: i32, lon0: i32) -> String {
    format!("Copernicus_DSM_COG_10_{}{:02}_00_{}{:03}_00", if lat0 >= 0 { 'N' } else { 'S' }, lat0.abs(), if lon0 >= 0 { 'E' } else { 'W' }, lon0.abs())
}

pub const BUCKET: &str = "https://copernicus-dem-30m.s3.amazonaws.com";

/// GLO-30's tiles in a folder (the NAS's `sources/copernicus-dem/`: `<stem>_DEM.tif`,
/// `<stem>_FLM.tif`, and `tileList.txt`, the bucket's list), decoded when first wanted and the
/// most recent `keep` of them kept. With `fetch`, a tile the list has and the folder hasn't is
/// downloaded into it first (written beside its name, then renamed).
pub struct GloStore {
    dir: PathBuf,
    listed: HashSet<(i32, i32)>,
    fetch: bool,
    keep: usize,
    cells: Mutex<(u64, HashMap<(i32, i32), (u64, Arc<OnceLock<Got>>)>)>,
}

impl GloStore {
    pub fn open(dir: &Path, fetch: bool) -> Result<GloStore> {
        let list = std::fs::read_to_string(dir.join("tileList.txt")).with_context(|| format!("{}: GLO-30's tile list", dir.display()))?;
        let mut listed = HashSet::new();
        for l in list.lines() {
            // Copernicus_DSM_COG_10_N64_00_W148_00_DEM
            let p: Vec<&str> = l.trim().split('_').collect();
            if p.len() < 9 {
                continue;
            }
            let num = |s: &str| s[1..].parse::<i32>().ok().map(|v| if s.starts_with('S') || s.starts_with('W') { -v } else { v });
            if let (Some(la), Some(lo)) = (num(p[4]), num(p[6])) {
                listed.insert((la, lo));
            }
        }
        anyhow::ensure!(listed.len() > 20_000, "{}: GLO-30's tile list has {} tiles", dir.display(), listed.len());
        Ok(GloStore { dir: dir.to_path_buf(), listed, fetch, keep: 48, cells: Mutex::new((0, HashMap::new())) })
    }

    fn load(&self, la: i32, lo: i32) -> Got {
        if !self.listed.contains(&(la, lo)) {
            return Got::Sea;
        }
        let s = stem(la, lo);
        let (dem, flm) = (self.dir.join(format!("{s}_DEM.tif")), self.dir.join(format!("{s}_FLM.tif")));
        if self.fetch && !(dem.exists() && flm.exists()) {
            for (key, to) in [(format!("{s}_DEM/{s}_DEM.tif"), &dem), (format!("{s}_DEM/AUXFILES/{s}_FLM.tif"), &flm)] {
                if !to.exists() {
                    if let Err(e) = download(&format!("{BUCKET}/{key}"), to) {
                        eprintln!("terrain: GLO-30 {s}: {e:#}");
                        return Got::Missing;
                    }
                }
            }
        }
        match read_cell(&dem, &flm) {
            Ok(c) => Got::Cell(Arc::new(c)),
            Err(e) => {
                if dem.exists() {
                    eprintln!("terrain: GLO-30 {s}: {e:#}");
                }
                Got::Missing
            }
        }
    }
}

impl Cells for GloStore {
    fn cell(&self, la: i32, lo: i32) -> Got {
        let slot = {
            let mut g = self.cells.lock().unwrap();
            g.0 += 1;
            let tick = g.0;
            if let Some(e) = g.1.get_mut(&(la, lo)) {
                e.0 = tick;
                e.1.clone()
            } else {
                if g.1.len() >= self.keep {
                    // (The least recently wanted, of those decoded.)
                    if let Some(old) = g.1.iter().filter(|(_, (_, s))| s.get().is_some()).min_by_key(|(_, (t, _))| *t).map(|(k, _)| *k) {
                        g.1.remove(&old);
                    }
                }
                let s = Arc::new(OnceLock::new());
                g.1.insert((la, lo), (tick, s.clone()));
                s
            }
        };
        slot.get_or_init(|| self.load(la, lo)).clone()
    }

    fn pin(&self) -> String {
        crate::terrain_pack::NORTH_PIN.into()
    }
}

/// Downloads `url` to `to` (beside it first, then renamed), as the map's other sources are.
fn download(url: &str, to: &Path) -> Result<()> {
    crate::fetch::online(url)?;
    let agent = crate::terrain_pack::agent();
    let mut r = agent.get(url).call().with_context(|| url.to_string())?;
    let b = r.body_mut().with_config().limit(200_000_000).read_to_vec().with_context(|| url.to_string())?;
    let part = to.with_extension("tif.part");
    std::fs::write(&part, &b)?;
    std::fs::rename(&part, to)?;
    Ok(())
}

/// A tile's heights and its filling mask, decoded whole.
pub fn read_cell(dem: &Path, flm: &Path) -> Result<Cell> {
    let open = |p: &Path| -> Result<crate::geotiff::Tiff> { crate::geotiff::Tiff::open(Arc::new(store::range::PlainFile::open(p).with_context(|| p.display().to_string())?)) };
    let t = open(dem)?;
    let img = t.level(0)?;
    let (w, h) = (img.width as usize, img.height as usize);
    anyhow::ensure!(h == ROWS && w >= 360, "{}: {w}×{h}", dem.display());
    let e = t.read_window(0, 0, 0, w as u32, h as u32)?;
    let filled = match open(flm) {
        Ok(f) => {
            let m = f.read_window(0, 0, 0, w as u32, h as u32)?;
            m.iter().map(|&v| v >= 3.0).collect()
        }
        Err(_) => vec![false; w * h],
    };
    Ok(Cell { w, e, filled })
}

/// GLO-30 resampled onto a terrain tile (256 × 256), with GLO-30's share of each pixel.
pub struct North {
    /// Its heights (NaN where it has none: a missing cell).
    pub g: Vec<f32>,
    /// The share of each pixel's samples filled from an ancillary DEM.
    pub filled: Vec<f32>,
    /// GLO-30's share by latitude (`weight`).
    pub w: Vec<f32>,
}

/// The cells around a tile, gathered once (a lookup a sample would cost too much).
struct Near {
    lat0: i32,
    lon0: i32,
    nlon: i32,
    got: Vec<Got>,
}

impl Near {
    fn get(&self, la: i32, lo: i32) -> &Got {
        let (i, j) = (la - self.lat0, lo.rem_euclid(360) - self.lon0.rem_euclid(360));
        let j = j.rem_euclid(360);
        if i < 0 || j >= self.nlon || i as usize * self.nlon as usize + j as usize >= self.got.len() {
            return &Got::Missing;
        }
        &self.got[i as usize * self.nlon as usize + j as usize]
    }

    /// Global row `rr` (0 at 90°N, 3600 a degree) at longitude `lon`: its height between the two
    /// nearest columns, and whether either was filled; None when a cell is missing.
    fn row(&self, rr: i64, lon: f64) -> Option<(f32, f32)> {
        let la = 89 - rr.div_euclid(ROWS as i64) as i32;
        let r = rr.rem_euclid(ROWS as i64) as usize;
        let lo = lon.floor() as i32;
        let at = |la: i32, lo: i32, c: usize| -> Option<(f32, f32)> {
            match self.get(la, lo) {
                Got::Sea => Some((0.0, 0.0)),
                Got::Missing => None,
                Got::Cell(cell) => {
                    let v = cell.e[r * cell.w + c.min(cell.w - 1)];
                    if !(v > BAD) {
                        return None;
                    }
                    Some((v, if cell.filled[r * cell.w + c.min(cell.w - 1)] { 1.0 } else { 0.0 }))
                }
            }
        };
        let w = match self.get(la, lo) {
            Got::Cell(c) => c.w,
            Got::Sea => return Some((0.0, 0.0)),
            Got::Missing => return None,
        };
        let fc = (lon - lo as f64) * w as f64;
        let c0 = (fc.floor() as usize).min(w - 1);
        let s = (fc - c0 as f64) as f32;
        let a = at(la, lo, c0)?;
        let b = if c0 + 1 < w { at(la, lo, c0 + 1)? } else { at(la, lo + 1, 0)? };
        Some((a.0 + (b.0 - a.0) * s, a.1 + (b.1 - a.1) * s))
    }

    /// Bilinear at (lat, lon) between GLO-30's pixel centres.
    fn sample(&self, lat: f64, lon: f64) -> Option<(f32, f32)> {
        let fr = (90.0 - lat) * ROWS as f64;
        let r0 = fr.floor() as i64;
        let t = (fr - r0 as f64) as f32;
        let a = self.row(r0, lon)?;
        let b = self.row(r0 + 1, lon)?;
        Some((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t))
    }
}

/// Mercator y (0–1, down) to latitude.
fn lat_of(v: f64) -> f64 {
    (std::f64::consts::PI * (1.0 - 2.0 * v)).dsinh().datan().to_degrees()
}

/// GLO-30 on tile z/x/y; None when the tile lies wholly south of SOUTH.
pub fn north_tile(cells: &dyn Cells, z: u8, x: u32, y: u32) -> Option<North> {
    let n = (1u64 << z) as f64;
    let (top, bottom) = (lat_of(y as f64 / n), lat_of((y + 1) as f64 / n));
    if top <= SOUTH {
        return None;
    }
    let (west, east) = (x as f64 / n * 360.0 - 180.0, (x + 1) as f64 / n * 360.0 - 180.0);
    // (The cells it meets, and a row and column more for the samples between them.)
    let lat0 = (bottom.floor() as i32 - 1).max(-90);
    let lat1 = (top.floor() as i32 + 1).min(89);
    let lon0 = west.floor() as i32 - 1;
    let nlon = east.floor() as i32 + 2 - lon0;
    let mut got = Vec::new();
    for la in lat0..=lat1 {
        for k in 0..nlon {
            let lo = (lon0 + k + 180).rem_euclid(360) - 180;
            got.push(if la < SOUTH as i32 - 1 { Got::Missing } else { cells.cell(la, lo) });
        }
    }
    let near = Near { lat0, lon0: (lon0 + 180).rem_euclid(360) - 180, nlon, got };
    let mut out = North { g: vec![f32::NAN; 65536], filled: vec![0.0; 65536], w: vec![0.0; 65536] };
    for j in 0..256usize {
        let lat_c = lat_of((y as f64 + (j as f64 + 0.5) / 256.0) / n);
        let wt = weight(lat_c);
        if wt == 0.0 {
            continue;
        }
        let px_m = 40_075_016.7 * lat_c.to_radians().dcos() / (n * 256.0);
        let k = ((px_m / SPACING_M).ceil() as usize).clamp(1, MAX_K);
        let lats: Vec<f64> = (0..k).map(|b| lat_of((y as f64 + (j as f64 + (b as f64 + 0.5) / k as f64) / 256.0) / n)).collect();
        for i in 0..256usize {
            let (mut s, mut f, mut m) = (0f32, 0f32, 0u32);
            for a in 0..k {
                let lon = (x as f64 + (i as f64 + (a as f64 + 0.5) / k as f64) / 256.0) / n * 360.0 - 180.0;
                for &lat in &lats {
                    if let Some((v, fl)) = near.sample(lat, lon) {
                        s += v;
                        f += fl;
                        m += 1;
                    }
                }
            }
            let p = j * 256 + i;
            out.w[p] = wt;
            // (Every sample, or none: a pixel half over a missing cell is AWS's.)
            if m as usize == k * k {
                out.g[p] = s / m as f32;
                out.filled[p] = f / m as f32;
            }
        }
    }
    Some(out)
}

/// Blends GLO-30 (`nt`) into the tile `e` (AWS's, repaired), but for the pixels `keep` (made from
/// finer tiles already blended): GLO-30's share by latitude; where GLO-30 was filled, AWS's moved
/// onto GLO-30's datum (the median difference over the tile's unfilled pixels; none: AWS's as it
/// is); where GLO-30 has none, AWS's. Returns the pixels changed.
pub fn blend(e: &mut [f32], nt: &North, keep: Option<&[bool]>) -> usize {
    let mut d: Vec<f32> = (0..e.len()).filter(|&p| nt.w[p] > 0.0 && nt.g[p].is_finite() && nt.filled[p] == 0.0 && e[p].is_finite() && keep.is_none_or(|k| !k[p])).map(|p| e[p] - nt.g[p]).collect();
    let off = if d.len() >= 64 {
        let m = d.len() / 2;
        *d.select_nth_unstable_by(m, |a, b| a.total_cmp(b)).1
    } else {
        0.0
    };
    let mut n = 0;
    for p in 0..e.len() {
        if nt.w[p] == 0.0 || !nt.g[p].is_finite() || keep.is_some_and(|k| k[p]) {
            continue;
        }
        let fl = nt.filled[p];
        let g = if fl > 0.0 { (1.0 - fl) * nt.g[p] + fl * (e[p] - off) } else { nt.g[p] };
        let v = nt.w[p] * g + (1.0 - nt.w[p]) * e[p];
        if v != e[p] {
            e[p] = v;
            n += 1;
        }
    }
    n
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Cells made by a function of (lat, lon) (heights) and a filled test, `w` columns; `sea` cells
    /// the list hasn't, `missing` ones it has but the store hasn't.
    pub struct FnCells {
        pub h: fn(f64, f64) -> f32,
        pub filled: fn(f64, f64) -> bool,
        pub sea: fn(i32, i32) -> bool,
        pub missing: fn(i32, i32) -> bool,
        pub made: Mutex<HashMap<(i32, i32), Arc<Cell>>>,
    }

    impl FnCells {
        pub fn new(h: fn(f64, f64) -> f32) -> Self {
            FnCells { h, filled: |_, _| false, sea: |_, _| false, missing: |_, _| false, made: Mutex::new(HashMap::new()) }
        }
    }

    pub fn width(la: i32) -> usize {
        match la.abs() {
            0..=49 => 3600,
            50..=59 => 2400,
            60..=69 => 1800,
            70..=79 => 1200,
            80..=84 => 720,
            _ => 360,
        }
    }

    impl Cells for FnCells {
        fn cell(&self, la: i32, lo: i32) -> Got {
            if (self.sea)(la, lo) {
                return Got::Sea;
            }
            if (self.missing)(la, lo) {
                return Got::Missing;
            }
            let mut m = self.made.lock().unwrap();
            let c = m.entry((la, lo)).or_insert_with(|| {
                let w = width(la);
                let mut e = vec![0f32; w * ROWS];
                let mut filled = vec![false; w * ROWS];
                for r in 0..ROWS {
                    let lat = la as f64 + 1.0 - r as f64 / ROWS as f64;
                    for c in 0..w {
                        let lon = lo as f64 + c as f64 / w as f64;
                        e[r * w + c] = (self.h)(lat, lon);
                        filled[r * w + c] = (self.filled)(lat, lon);
                    }
                }
                Arc::new(Cell { w, e, filled })
            });
            Got::Cell(c.clone())
        }
        fn pin(&self) -> String {
            "test".into()
        }
    }

    fn tile_of(lat: f64, lon: f64, z: u8) -> (u32, u32) {
        let n = (1u64 << z) as f64;
        let y = (1.0 - (lat.to_radians().tan() + 1.0 / lat.to_radians().cos()).ln() / std::f64::consts::PI) / 2.0 * n;
        (((lon + 180.0) / 360.0 * n) as u32, y as u32)
    }

    #[test]
    fn weights_by_latitude() {
        assert_eq!(weight(59.0), 0.0);
        assert_eq!(weight(59.5), 0.0);
        assert!((weight(59.75) - 0.5).abs() < 1e-6);
        assert_eq!(weight(60.0), 1.0);
        assert_eq!(weight(75.0), 1.0);
        // Smooth: no step anywhere in the band.
        let mut last = 0.0;
        for k in 0..=100 {
            let w = weight(59.5 + k as f64 * 0.005);
            assert!(w >= last && w - last < 0.02);
            last = w;
        }
    }

    #[test]
    fn a_plane_is_resampled_exactly_across_cells_and_bands() {
        // A plane in (lat, lon): bilinear between pixel centres gives it back wherever the sample
        // falls, across cells and the 1800 → 1200 columns' change at 70°N.
        let c = FnCells::new(|lat, lon| (100.0 + (lat - 60.0) * 50.0 + (lon + 150.0) * 20.0) as f32);
        for (lat, lon, z) in [(64.3, -147.9, 12u8), (70.0, -140.0, 11), (61.0, -136.0, 10), (69.99, -150.0, 9)] {
            let (x, y) = tile_of(lat, lon, z);
            let nt = north_tile(&c, z, x, y).unwrap();
            let n = (1u64 << z) as f64;
            for &(i, j) in &[(0usize, 0usize), (128, 128), (255, 255), (17, 230)] {
                let p = j * 256 + i;
                // (The pixel's mean over its samples: the plane at its centre in longitude, and the
                // mean of its samples' latitudes.)
                let k = ((40_075_016.7 * lat.to_radians().cos() / (n * 256.0)) / SPACING_M).ceil().clamp(1.0, MAX_K as f64) as usize;
                let lat_m: f64 = (0..k).map(|b| lat_of((y as f64 + (j as f64 + (b as f64 + 0.5) / k as f64) / 256.0) / n)).sum::<f64>() / k as f64;
                let lon_c = (x as f64 + (i as f64 + 0.5) / 256.0) / n * 360.0 - 180.0;
                let want = 100.0 + (lat_m - 60.0) * 50.0 + (lon_c + 150.0) * 20.0;
                assert!((nt.g[p] as f64 - want).abs() < 0.01, "{z}/{x}/{y} ({i},{j}): {} vs {want}", nt.g[p]);
                assert_eq!(nt.w[p], 1.0);
            }
        }
    }

    #[test]
    fn south_of_the_band_nothing_and_in_it_a_smooth_blend() {
        let c = FnCells::new(|_, _| 500.0);
        let (x, y) = tile_of(55.0, -130.0, 10);
        assert!(north_tile(&c, 10, x, y).is_none());
        // A z8 tile across the band: AWS's 400 m blended with GLO-30's 500 m by latitude, with no
        // step between rows.
        let (x, y) = tile_of(59.75, -135.0, 8);
        let nt = north_tile(&c, 8, x, y).unwrap();
        let mut e = vec![400f32; 65536];
        blend(&mut e, &nt, None);
        let n = 256.0f64;
        for j in 0..256 {
            let lat = lat_of((y as f64 + (j as f64 + 0.5) / 256.0) / n);
            let want = 400.0 + 100.0 * weight(lat);
            assert!((e[j * 256 + 9] - want).abs() < 0.01, "row {j} {lat}: {} vs {want}", e[j * 256 + 9]);
            if j > 0 {
                assert!((e[j * 256 + 9] - e[(j - 1) * 256 + 9]).abs() < 3.0, "no step");
            }
        }
    }

    #[test]
    fn filled_pixels_take_aws_on_glos_datum_and_missing_cells_keep_aws() {
        // GLO-30 at 300 m, filled west of −148.5; AWS 47 m higher everywhere (ellipsoidal), with a
        // bump of 30 m where GLO-30 was filled: the filled part is AWS's, moved down by 47 m.
        let mut c = FnCells::new(|_, _| 300.0);
        c.filled = |_, lon| lon < -148.5;
        let (x, y) = tile_of(64.5, -148.5, 10);
        let nt = north_tile(&c, 10, x, y).unwrap();
        let mut e: Vec<f32> = (0..65536).map(|p| if (nt.filled[p] - 1.0).abs() < 1e-6 { 377.0 } else { 347.0 }).collect();
        blend(&mut e, &nt, None);
        for p in 0..65536 {
            let want = if nt.filled[p] == 1.0 { 330.0 } else if nt.filled[p] == 0.0 { 300.0 } else { continue };
            assert!((e[p] - want).abs() < 0.01, "{p}: {} vs {want}", e[p]);
        }
        // A cell the store hasn't: AWS's as it is; the open sea: 0.
        let mut c = FnCells::new(|_, _| 300.0);
        c.missing = |_, lo| lo == -150;
        c.sea = |_, lo| lo == -149;
        let (x, y) = tile_of(64.5, -148.0, 6);
        let nt = north_tile(&c, 6, x, y).unwrap();
        let mut e = vec![50f32; 65536];
        blend(&mut e, &nt, None);
        let lon = |i: usize| (x as f64 + (i as f64 + 0.5) / 256.0) / 64.0 * 360.0 - 180.0;
        let row = 128 * 256;
        for i in 0..256 {
            let l = lon(i);
            let v = e[row + i];
            if (-149.95..-149.05).contains(&l) {
                assert_eq!(v, 50.0, "missing cell at {l}");
            }
            if (-147.95..-147.05).contains(&l) {
                assert!((v - 300.0).abs() < 0.01, "GLO-30 at {l}: {v}");
            }
            if (-148.95..-148.05).contains(&l) {
                assert_eq!(v, 0.0, "sea at {l}");
            }
        }
        // Pixels kept (made from finer tiles) stay as they are.
        let keep: Vec<bool> = (0..65536).map(|p| p % 2 == 0).collect();
        let mut e = vec![50f32; 65536];
        blend(&mut e, &nt, Some(&keep));
        assert!((0..65536).filter(|p| p % 2 == 0).all(|p| e[p] == 50.0));
    }
}
