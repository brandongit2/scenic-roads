//! Elevations at every road vertex from national DEMs: dem/sample.py's port (the `elev` program),
//! its command line and outputs, so a unit's build can run anywhere (docs/workers.md).
//!
//! Priority per vertex (the first source with valid data wins; the numbers are `src.u8`'s codes):
//!
//! - North America (west of 40° W): 1. NRCan HRDEM 2 m lidar mosaic, read at its 8 m overview
//!   (Canada, where lidar exists); 2. USGS 3DEP 1/3 arc-second, ~10 m (the United States);
//!   3. NRCan MRDEM 30 m (Canada and the border).
//! - Japan, in GSI's own order per pixel (`gsi`): 5. lidar (1A, then 5A), 6. photogrammetry
//!   (5B, 5C), at z15; 7. the 10 m DEM (z14).
//! - Taiwan: 8. the MOI 20 m DTM, GeoTIFFs put by hand in the NAS's inputs/moi-dtm/
//!   ($SCENIC_MOI_DTM, else data/sources/moi-dtm); FABDEM without them, and cached FABDEM values
//!   in Taiwan are sampled again once they're there.
//! - Elsewhere, and points none of the above cover: 4. FABDEM v1-2 30 m (`fabdem`).
//!
//! Only the blocks (and GSI tiles) holding vertices are read (`crate::fetch`). Incremental: the
//! sorted (vertex → elevation, source) cache of the last run serves the vertices it has; only the
//! others are sampled. Outputs go to `.tmp` files, checkpointed after each DEM file
//! (`dem-progress.json`: an interrupted run resumes), and are renamed into place at the end.
//!
//! Changing which DEM serves where, or how: bump the rule's version in `crate::rules`
//! ("dem-north-america", "dem-japan", "dem-taiwan", "dem-fabdem"), so its units run again.

pub mod fabdem;
pub mod gsi;
pub mod proj;
pub mod sample;

use crate::fetch::Fetch;
use crate::geotiff::{key, Tiff};
use anyhow::{bail, ensure, Context, Result};
use rayon::prelude::*;
use roadcore::DemSource;
use sample::sample_raster;
use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::PlainFile;
use store::sys::PosIo;

pub const NRCAN: &str = "https://canelevation-dem.s3.ca-central-1.amazonaws.com";

/// North America: the national DEMs; elsewhere FABDEM.
pub const NA_WEST_OF: f64 = -40.0;

/// The HRDEM mosaic's tiles (500 km squares, EPSG:3979), and those it has at 2 m.
const HRDEM_INDEX: &str = include_str!("../../../../dem/hrdem_tile_index.geojson");
const HRDEM_TILES: &str = include_str!("../../../../dem/hrdem_2m_tiles.txt");

pub fn hrdem_url(tid: &str) -> String {
    format!("{NRCAN}/hrdem-mosaic-2m/{tid}-mosaic-2m-dtm.tif")
}

pub fn mrdem_url() -> String {
    format!("{NRCAN}/mrdem-30/mrdem-30-dtm.tif")
}

pub fn usgs_url(t: &str) -> String {
    format!("https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/13/TIFF/current/{t}/USGS_13_{t}.tif")
}

pub fn in_japan(lon: f64, lat: f64) -> bool {
    lon > 122.5 && lon < 154.0 && lat > 20.0 && lat < 46.5
}

pub fn in_taiwan(lon: f64, lat: f64) -> bool {
    lon > 118.0 && lon <= 122.5 && lat > 21.5 && lat < 26.6
}

/// A vertex's cache key: `(lon + 2³¹) << 32 | (lat + 2³¹)` (E7), as `crate::unit`'s DEM cache.
pub fn pack(v: [i32; 2]) -> u64 {
    (((v[0] as i64 + (1i64 << 31)) as u64) << 32) | ((v[1] as i64 + (1i64 << 31)) as u64)
}

/// The HRDEM mosaic tiles there are at 2 m, with their boxes (EPSG:3979: min x, min y, max x,
/// max y), in the index's order.
pub fn hrdem_tiles() -> Result<Vec<(String, [f64; 4])>> {
    let have: BTreeSet<&str> = HRDEM_TILES.split_whitespace().filter_map(|l| l.rsplit('/').next()?.split('-').next()).collect();
    let index: serde_json::Value = serde_json::from_str(HRDEM_INDEX)?;
    let mut out = Vec::new();
    for f in index["features"].as_array().context("HRDEM index: no features")? {
        let id = f["properties"]["id"].as_str().context("HRDEM index: a tile without an id")?;
        if !have.contains(id) {
            continue;
        }
        let ring = f["geometry"]["coordinates"][0].as_array().context("HRDEM index: a tile without a polygon")?;
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in ring {
            let (x, y) = (p[0].as_f64().context("HRDEM index: a coordinate")?, p[1].as_f64().context("HRDEM index: a coordinate")?);
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        out.push((id.to_string(), b));
    }
    Ok(out)
}

/// The points `idx` grouped by USGS 1° tile (named by its upper-left corner: n46w068), in the
/// tiles' order.
pub fn usgs_groups(lon: &[f64], lat: &[f64], idx: &[u32]) -> Vec<(String, Vec<u32>)> {
    let mut keyed: Vec<(i64, u32)> = idx.iter().map(|&i| (lat[i as usize].ceil() as i64 * 1000 + (-lon[i as usize]).ceil() as i64, i)).collect();
    keyed.sort_by_key(|k| k.0);
    let mut out: Vec<(String, Vec<u32>)> = Vec::new();
    let mut last = None;
    for (k, i) in keyed {
        if last != Some(k) {
            out.push((format!("n{:02}w{:03}", k.div_euclid(1000), k.rem_euclid(1000)), Vec::new()));
            last = Some(k);
        }
        out.last_mut().unwrap().1.push(i);
    }
    out
}

/// The MOI DTM's GeoTIFFs in `dir` (`*.tif`, by name).
pub fn moi_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "tif")).collect();
    v.sort();
    v
}

/// How a raster's coordinates follow from WGS 84 longitude and latitude.
enum Crs {
    Geographic,
    Tm(proj::TransverseMercator),
}

impl Crs {
    fn project(&self, lon: f64, lat: f64) -> (f64, f64) {
        match self {
            Crs::Geographic => (lon, lat),
            Crs::Tm(t) => t.project(lon, lat).unwrap_or((f64::NAN, f64::NAN)),
        }
    }
}

/// A raster's CRS from its GeoKeys: geographic, or a transverse Mercator (Taiwan's TM2 zones on
/// TWD97 or TWD67, UTM, or one the keys define), with no datum shift, as PROJ has none for these.
fn crs_of(t: &Tiff) -> Result<Crs> {
    let k = t.geo_keys();
    if k.short(key::MODEL_TYPE) == Some(2) {
        return Ok(Crs::Geographic);
    }
    let tm = |lon0: f64, k0: f64, x0: f64, (a, rf): (f64, f64)| Crs::Tm(proj::TransverseMercator::new(lon0, 0.0, k0, x0, 0.0, a, rf));
    let aust = (6378160.0, 298.25);
    Ok(match k.short(key::PROJECTED_CS_TYPE) {
        Some(3826) => tm(121.0, 0.9999, 250000.0, proj::Ellipsoid::GRS80),
        Some(3825) => tm(119.0, 0.9999, 250000.0, proj::Ellipsoid::GRS80),
        Some(3828) => tm(121.0, 0.9999, 250000.0, aust),
        Some(3827) => tm(119.0, 0.9999, 250000.0, aust),
        Some(c @ 32601..=32660) => tm((c - 32600) as f64 * 6.0 - 183.0, 0.9996, 500000.0, proj::Ellipsoid::WGS84),
        c if (c.is_none() || c == Some(32767)) && k.short(key::PROJ_COORD_TRANS) == Some(1) => {
            ensure!(k.short(key::PROJ_LINEAR_UNITS).is_none_or(|u| u == 9001), "linear units {:?}", k.short(key::PROJ_LINEAR_UNITS));
            let ell = match k.short(key::GEOG_ELLIPSOID) {
                Some(7030) => proj::Ellipsoid::WGS84,
                Some(7003) => aust,
                Some(7019) | None => match (k.double(key::GEOG_SEMI_MAJOR_AXIS), k.double(key::GEOG_INV_FLATTENING)) {
                    (Some(a), Some(rf)) => (a, rf),
                    _ => proj::Ellipsoid::GRS80,
                },
                Some(e) => bail!("ellipsoid {e}"),
            };
            let d = |id: u16, default: f64| k.double(id).unwrap_or(default);
            Crs::Tm(proj::TransverseMercator::new(d(key::PROJ_NAT_ORIGIN_LONG, 0.0), d(key::PROJ_NAT_ORIGIN_LAT, 0.0), d(key::PROJ_SCALE_AT_NAT_ORIGIN, 1.0), d(key::PROJ_FALSE_EASTING, 0.0), d(key::PROJ_FALSE_NORTHING, 0.0), ell.0, ell.1))
        }
        c => bail!("projected CRS {c:?} isn't one this step knows"),
    })
}

/// The run's settings: sample.py's arguments and environment.
pub struct Config {
    pub build: PathBuf,
    /// Threads reading DEM blocks (they mostly wait on the servers).
    pub workers: usize,
    /// Where the last run's per-vertex cache is (`dem-cache.*`), and this run's goes.
    pub cache: PathBuf,
    pub no_cache: bool,
    /// Taiwan's MOI DTM files.
    pub moi_dtm: Vec<PathBuf>,
    /// FABDEM's store, where each tile is downloaded once (None: read in place at Bristol).
    pub fabdem_store: Option<PathBuf>,
}

/// dem-stats.json: vertices by source.
#[derive(Debug, Default, serde::Serialize)]
pub struct Stats {
    pub vertices: usize,
    pub hrdem: usize,
    pub usgs3dep: usize,
    pub mrdem: usize,
    pub fabdem: usize,
    pub gsi5a: usize,
    pub gsi5: usize,
    pub gsi10: usize,
    pub moi: usize,
    pub missing: usize,
    pub sampled_this_run: usize,
    pub seconds: f64,
}

/// `n` with thousands separators.
fn th(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The outputs being made (`elev.f32.tmp`, `src.u8.tmp`) and the DEM files done
/// (`dem-progress.json`), so an interrupted run resumes where it stopped. What changed since the
/// last checkpoint is written then, by pages.
struct Progress {
    dir: PathBuf,
    stamp: String,
    elev: Vec<f32>,
    src: Vec<u8>,
    done: BTreeSet<String>,
    dirty: Vec<bool>,
    resumed: bool,
    files: (File, File),
}

const PAGE: usize = 16384;

impl Progress {
    fn open(dir: &Path, stamp: String, n: usize) -> Result<Progress> {
        let (te, ts) = (dir.join("elev.f32.tmp"), dir.join("src.u8.tmp"));
        let prev = std::fs::read_to_string(dir.join("dem-progress.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let pages = n.div_ceil(PAGE);
        if let Some(p) = prev.filter(|p| p["stamp"].as_str() == Some(stamp.as_str())) {
            if let (Ok(e), Ok(s)) = (std::fs::read(&te), std::fs::read(&ts)) {
                if e.len() == 4 * n && s.len() == n {
                    let done = p["done"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                    let files = (OpenOptions::new().write(true).open(&te)?, OpenOptions::new().write(true).open(&ts)?);
                    return Ok(Progress { dir: dir.to_owned(), stamp, elev: bytemuck::pod_collect_to_vec(&e), src: s, done, dirty: vec![false; pages], resumed: true, files });
                }
            }
        }
        let (fe, fs) = (File::create(&te)?, File::create(&ts)?);
        fe.set_len(4 * n as u64)?;
        fs.set_len(n as u64)?;
        Ok(Progress { dir: dir.to_owned(), stamp, elev: vec![f32::NAN; n], src: vec![0; n], done: BTreeSet::new(), dirty: vec![true; pages], resumed: false, files: (fe, fs) })
    }

    fn set(&mut self, i: usize, e: f32, s: u8) {
        if self.elev[i].to_bits() != e.to_bits() || self.src[i] != s {
            self.elev[i] = e;
            self.src[i] = s;
            self.dirty[i / PAGE] = true;
        }
    }

    /// The sampled values (`loc`, over `miss`) into the outputs.
    fn scatter(&mut self, miss: &[u32], loc_elev: &[f32], loc_src: &[u8]) {
        for (k, &i) in miss.iter().enumerate() {
            if loc_elev[k].is_finite() {
                self.set(i as usize, loc_elev[k], loc_src[k]);
            }
        }
    }

    fn flush(&mut self) -> Result<()> {
        let n = self.elev.len();
        let mut p = 0;
        while p < self.dirty.len() {
            if !self.dirty[p] {
                p += 1;
                continue;
            }
            let q = (p..self.dirty.len()).find(|&q| !self.dirty[q]).unwrap_or(self.dirty.len());
            let (s, e) = (p * PAGE, (q * PAGE).min(n));
            self.files.0.write_all_at(bytemuck::cast_slice(&self.elev[s..e]), 4 * s as u64)?;
            self.files.1.write_all_at(&self.src[s..e], s as u64)?;
            self.dirty[p..q].fill(false);
            p = q;
        }
        Ok(())
    }

    fn done(&self, name: &str) -> bool {
        self.done.contains(name)
    }

    /// A checkpoint: the outputs so far, and `name` done.
    fn mark(&mut self, name: &str) -> Result<()> {
        self.flush()?;
        self.done.insert(name.to_string());
        let j = serde_json::json!({ "stamp": self.stamp, "done": self.done });
        std::fs::write(self.dir.join("dem-progress.json"), j.to_string())?;
        Ok(())
    }

    /// The outputs renamed into place.
    fn finish(mut self) -> Result<(Vec<f32>, Vec<u8>)> {
        self.flush()?;
        drop(self.files);
        std::fs::rename(self.dir.join("elev.f32.tmp"), self.dir.join("elev.f32"))?;
        std::fs::rename(self.dir.join("src.u8.tmp"), self.dir.join("src.u8"))?;
        Ok((self.elev, self.src))
    }
}

/// The last run's cache (`dem-cache.*` in `cache`: sorted keys, elevations, sources) for the
/// vertices it has. With the MOI DTM here, Taiwan's vertices sampled from FABDEM are dropped, to
/// be sampled again from it.
fn reuse(cache: &Path, verts: &[[i32; 2]], prog: &mut Progress, moi: bool) -> Result<()> {
    let ck = cache.join("dem-cache.keys.u64");
    if !std::fs::metadata(&ck).is_ok_and(|m| m.len() > 0) {
        return Ok(());
    }
    let km = roadcore::mmap(&ck)?;
    let em = roadcore::mmap(&cache.join("dem-cache.elev.f32"))?;
    let sm = roadcore::mmap(&cache.join("dem-cache.src.u8"))?;
    let (ckeys, celev): (&[u64], &[f32]) = (bytemuck::cast_slice(&km[..]), bytemuck::cast_slice(&em[..]));
    let csrc: &[u8] = &sm;
    ensure!(ckeys.len() == celev.len() && ckeys.len() == csrc.len(), "{}: DEM cache files out of step", cache.display());
    let found: Vec<Option<usize>> = verts
        .par_iter()
        .map(|&v| {
            let k = pack(v);
            let p = ckeys.partition_point(|&x| x < k);
            (p < ckeys.len() && ckeys[p] == k).then_some(p)
        })
        .collect();
    let mut hit = 0;
    for (i, f) in found.into_iter().enumerate() {
        if let Some(p) = f {
            prog.set(i, celev[p], csrc[p]);
            hit += 1;
        }
    }
    let n = verts.len();
    println!("cache: reused {} of {} vertices ({:.1} %)", th(hit), th(n), hit as f64 / n.max(1) as f64 * 100.0);
    if moi {
        let mut redo = 0;
        for (i, v) in verts.iter().enumerate() {
            if prog.src[i] == DemSource::Fabdem as u8 && in_taiwan(v[0] as f64 * 1e-7, v[1] as f64 * 1e-7) {
                prog.set(i, f32::NAN, 0);
                redo += 1;
            }
        }
        if redo > 0 {
            println!("cache: {} Taiwanese vertices sampled from FABDEM, now from the MOI DTM", th(redo));
        }
    }
    Ok(())
}

/// A thread pool of `n` threads, when threads can be made (not in WebAssembly).
fn pool(n: usize) -> Option<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new().num_threads(n.max(1)).build().ok()
}

/// File `path` sampled at points (in its CRS).
fn sample_file(path: &Path, px: &[f64], py: &[f64], pool: Option<&rayon::ThreadPool>) -> Result<Vec<f32>> {
    let t = Tiff::open(Arc::new(PlainFile::open(path)?)).with_context(|| path.display().to_string())?;
    sample_raster(&t, 0, px, py, pool).with_context(|| path.display().to_string())
}

/// The points' sampled values (`vals`, for points `idx`) where they're valid, from source `code`;
/// how many.
fn put(loc_elev: &mut [f32], loc_src: &mut [u8], idx: &[u32], vals: &[f32], code: DemSource) -> usize {
    let mut got = 0;
    for (&i, &v) in idx.iter().zip(vals) {
        if v.is_finite() {
            loc_elev[i as usize] = v;
            loc_src[i as usize] = code as u8;
            got += 1;
        }
    }
    got
}

fn mtime_ns(p: &Path) -> Result<u128> {
    Ok(std::fs::metadata(p)?.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_nanos())
}

/// Samples the DEMs at the build folder's vertices (`verts.bin`) not in the cache: writes
/// `elev.f32`, `src.u8`, `dem-stats.json` and the cache for the next run.
pub fn run(cfg: &Config, fetch: &dyn Fetch) -> Result<Stats> {
    let t_start = std::time::Instant::now();
    let b = &cfg.build;
    let va = roadcore::Array::<[i32; 2]>::open(&b.join("verts.bin"))?;
    let verts = va.get();
    let n = verts.len();
    let stamp = format!("{n}:{}", mtime_ns(&b.join("verts.bin"))?);
    println!("{} vertices", th(n));
    let mut prog = Progress::open(b, stamp, n)?;
    if !prog.resumed {
        if !cfg.no_cache {
            reuse(&cfg.cache, verts, &mut prog, !cfg.moi_dtm.is_empty())?;
        }
        prog.mark("cache")?;
    }
    let miss: Vec<u32> = (0..n as u32).filter(|&i| prog.elev[i as usize].is_nan()).collect();
    println!("{} vertices need DEM sampling", th(miss.len()));
    let lon: Vec<f64> = miss.iter().map(|&i| verts[i as usize][0] as f64 * 1e-7).collect();
    let lat: Vec<f64> = miss.iter().map(|&i| verts[i as usize][1] as f64 * 1e-7).collect();
    // Indexed by position in `miss`; scattered back through it.
    let mut loc_elev = vec![f32::NAN; miss.len()];
    let mut loc_src = vec![0u8; miss.len()];
    let gather = |v: &[f64], idx: &[u32]| -> Vec<f64> { idx.iter().map(|&i| v[i as usize]).collect() };
    let left = |loc: &[f32], idx: &[u32]| -> Vec<u32> { idx.iter().copied().filter(|&i| loc[i as usize].is_nan()).collect() };
    let (local, elsewhere): (Vec<u32>, Vec<u32>) = (0..miss.len() as u32).partition(|&i| lon[i as usize] < NA_WEST_OF);
    println!("  {} in North America, {} elsewhere", th(local.len()), th(elsewhere.len()));
    // EPSG:3979 (Canada Atlas Lambert, HRDEM's and MRDEM's); NaN outside North America.
    let atlas = proj::Atlas::new();
    let (mut x, mut y) = (vec![f64::NAN; miss.len()], vec![f64::NAN; miss.len()]);
    let xy: Vec<(f64, f64)> = local.par_iter().map(|&i| atlas.project(lon[i as usize], lat[i as usize])).collect();
    for (&i, p) in local.iter().zip(xy) {
        (x[i as usize], y[i as usize]) = p;
    }
    let pool = pool(cfg.workers);

    // ---- 1. HRDEM lidar (the 2 m mosaic's 8 m overview) ----------------------------------------
    let mut tiles: Vec<(String, Vec<u32>)> = Vec::new();
    for (tid, bb) in hrdem_tiles()? {
        let sel: Vec<u32> = local.iter().copied().filter(|&i| x[i as usize] >= bb[0] && x[i as usize] < bb[2] && y[i as usize] > bb[1] && y[i as usize] <= bb[3]).collect();
        if !sel.is_empty() {
            tiles.push((tid, sel));
        }
    }
    tiles.sort_by_key(|t| Reverse(t.1.len()));
    println!("HRDEM: {} mosaic tiles contain roads to sample", tiles.len());
    for (tid, sel) in &tiles {
        let name = format!("hrdem:{tid}");
        if prog.done(&name) {
            continue;
        }
        let url = hrdem_url(tid);
        let t = Tiff::open(fetch.open(&url)?.with_context(|| format!("{url}: the server has no such file"))?).with_context(|| url.clone())?;
        let v = sample_raster(&t, 2, &gather(&x, sel), &gather(&y, sel), pool.as_ref()).with_context(|| url.clone())?;
        let got = put(&mut loc_elev, &mut loc_src, sel, &v, DemSource::Hrdem);
        println!("  HRDEM {tid}: {}/{} vertices with lidar", th(got), th(sel.len()));
        prog.scatter(&miss, &loc_elev, &loc_src);
        prog.mark(&name)?;
    }

    // ---- 2. USGS 3DEP 1/3" ------------------------------------------------------------------------
    let rest = left(&loc_elev, &local);
    let mut groups = usgs_groups(&lon, &lat, &rest);
    println!("3DEP: {} vertices left, {} candidate 1° tiles", th(rest.len()), groups.len());
    groups.sort_by_key(|g| Reverse(g.1.len()));
    for (tname, sel) in &groups {
        let name = format!("3dep:{tname}");
        if prog.done(&name) {
            continue;
        }
        let url = usgs_url(tname);
        // (No 3DEP tile here: Canada, or the ocean.)
        let got = match fetch.open(&url)? {
            None => 0,
            Some(src) => {
                let t = Tiff::open(src).with_context(|| url.clone())?;
                let v = sample_raster(&t, 0, &gather(&lon, sel), &gather(&lat, sel), pool.as_ref()).with_context(|| url.clone())?;
                put(&mut loc_elev, &mut loc_src, sel, &v, DemSource::Usgs3dep)
            }
        };
        if got > 0 {
            println!("  3DEP {tname}: {}/{}", th(got), th(sel.len()));
        }
        prog.scatter(&miss, &loc_elev, &loc_src);
        prog.mark(&name)?;
    }

    // ---- 3. MRDEM 30 m ----------------------------------------------------------------------------
    if !prog.done("mrdem") {
        let rest = left(&loc_elev, &local);
        println!("MRDEM: {} vertices left", th(rest.len()));
        if !rest.is_empty() {
            let url = mrdem_url();
            let t = Tiff::open(fetch.open(&url)?.with_context(|| format!("{url}: the server has no such file"))?).with_context(|| url.clone())?;
            let v = sample_raster(&t, 0, &gather(&x, &rest), &gather(&y, &rest), pool.as_ref()).with_context(|| url.clone())?;
            let got = put(&mut loc_elev, &mut loc_src, &rest, &v, DemSource::Mrdem);
            println!("  MRDEM: {}/{}", th(got), th(rest.len()));
        }
        prog.scatter(&miss, &loc_elev, &loc_src);
        prog.mark("mrdem")?;
    }

    // ---- 5. Japan: GSI 5 m (lidar, then photogrammetry), then 10 m ------------------------------
    let jp: Vec<u32> = elsewhere.iter().copied().filter(|&i| in_japan(lon[i as usize], lat[i as usize])).collect();
    if !jp.is_empty() {
        let gpool = self::pool(gsi::WORKERS);
        for (layer, z, code) in gsi::LAYERS {
            let name = format!("gsi:{layer}");
            if prog.done(&name) {
                continue;
            }
            let rest = left(&loc_elev, &jp);
            let v = gsi::pass(fetch, layer, z, &gather(&lon, &rest), &gather(&lat, &rest), gpool.as_ref())?;
            let code = match code {
                5 => DemSource::Gsi5a,
                6 => DemSource::Gsi5,
                _ => DemSource::Gsi10,
            };
            let got = put(&mut loc_elev, &mut loc_src, &rest, &v, code);
            println!("  GSI {layer}: {}/{}", th(got), th(rest.len()));
            prog.scatter(&miss, &loc_elev, &loc_src);
            prog.mark(&name)?;
        }
    }

    // ---- 6. Taiwan: the MOI 20 m DTM (a file per island group) ---------------------------------
    let tw: Vec<u32> = elsewhere.iter().copied().filter(|&i| in_taiwan(lon[i as usize], lat[i as usize])).collect();
    if !tw.is_empty() {
        for path in &cfg.moi_dtm {
            let file = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let name = format!("moi:{file}");
            if prog.done(&name) {
                continue;
            }
            let rest = left(&loc_elev, &tw);
            let t = Tiff::open(Arc::new(PlainFile::open(path).with_context(|| path.display().to_string())?)).with_context(|| path.display().to_string())?;
            let crs = crs_of(&t).with_context(|| format!("MOI {file}"))?;
            let (px, py): (Vec<f64>, Vec<f64>) = rest.iter().map(|&i| crs.project(lon[i as usize], lat[i as usize])).unzip();
            let v = sample_raster(&t, 0, &px, &py, pool.as_ref()).with_context(|| format!("MOI {file}"))?;
            let got = put(&mut loc_elev, &mut loc_src, &rest, &v, DemSource::Moi);
            println!("  MOI {file}: {}/{}", th(got), th(rest.len()));
            prog.scatter(&miss, &loc_elev, &loc_src);
            prog.mark(&name)?;
        }
    }

    // ---- 4. FABDEM 30 m (the rest outside North America, and North American points none of the
    // national DEMs cover, e.g. Saint-Pierre-et-Miquelon) -------------------------------------------
    let uncovered = left(&loc_elev, &local);
    if !uncovered.is_empty() {
        println!("FABDEM fallback: {} North American vertices without a national DEM", th(uncovered.len()));
    }
    let rest = left(&loc_elev, &elsewhere);
    let mut all = rest.clone();
    all.extend_from_slice(&uncovered);
    let mut groups = fabdem::groups(&lon, &lat, &all);
    println!("FABDEM: {} vertices, {} 1° tiles", th(rest.len()), groups.len());
    groups.sort_by_key(|g| Reverse(g.1.len()));
    for ((tname, zname), sel) in &groups {
        let name = format!("fabdem:{tname}");
        if prog.done(&name) {
            continue;
        }
        let (lo, la) = (gather(&lon, sel), gather(&lat, sel));
        let v = match &cfg.fabdem_store {
            // From the store, where each tile is downloaded once.
            Some(store) => match fabdem::stored(fetch, store, zname, tname, false)? {
                None => None,
                Some(p) => match sample_file(&p, &lo, &la, pool.as_ref()) {
                    Ok(v) => Some(v),
                    // The stored copy is damaged: taken again (once).
                    Err(_) => match fabdem::stored(fetch, store, zname, tname, true)? {
                        Some(p) => Some(sample_file(&p, &lo, &la, pool.as_ref())?),
                        None => None,
                    },
                },
            },
            // Read in place inside Bristol's zip.
            None => match fabdem::remote(fetch, zname, tname)? {
                None => None,
                Some(src) => Some(sample_raster(&Tiff::open(src).with_context(|| format!("FABDEM {tname}"))?, 0, &lo, &la, pool.as_ref()).with_context(|| format!("FABDEM {tname}"))?),
            },
        };
        // (None: no tile, the open sea.)
        let got = v.map_or(0, |v| put(&mut loc_elev, &mut loc_src, sel, &v, DemSource::Fabdem));
        println!("  FABDEM {tname}: {}/{}", th(got), th(sel.len()));
        prog.scatter(&miss, &loc_elev, &loc_src);
        prog.mark(&name)?;
    }
    drop(pool);

    // ---- the outputs, their stats, and the cache for the next run --------------------------------
    let mut counts = [0usize; 256];
    for &s in &prog.src {
        counts[s as usize] += 1;
    }
    let stats = Stats {
        vertices: n,
        hrdem: counts[DemSource::Hrdem as usize],
        usgs3dep: counts[DemSource::Usgs3dep as usize],
        mrdem: counts[DemSource::Mrdem as usize],
        fabdem: counts[DemSource::Fabdem as usize],
        gsi5a: counts[DemSource::Gsi5a as usize],
        gsi5: counts[DemSource::Gsi5 as usize],
        gsi10: counts[DemSource::Gsi10 as usize],
        moi: counts[DemSource::Moi as usize],
        missing: counts[0],
        sampled_this_run: miss.len(),
        seconds: (t_start.elapsed().as_secs_f64() * 10.0).round() / 10.0,
    };
    let (elev, src) = prog.finish()?;
    std::fs::write(b.join("dem-stats.json"), serde_json::to_string_pretty(&stats)?)?;
    std::fs::remove_file(b.join("dem-progress.json")).ok();
    println!("updating elevation cache");
    std::fs::create_dir_all(&cfg.cache)?;
    let keys: Vec<u64> = verts.par_iter().map(|&v| pack(v)).collect();
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.par_sort_unstable_by_key(|&i| (keys[i as usize], i));
    let ok: Vec<u64> = order.iter().map(|&i| keys[i as usize]).collect();
    let oe: Vec<f32> = order.iter().map(|&i| elev[i as usize]).collect();
    let os: Vec<u8> = order.iter().map(|&i| src[i as usize]).collect();
    for (name, bytes) in [("keys.u64", bytemuck::cast_slice::<u64, u8>(&ok)), ("elev.f32", bytemuck::cast_slice(&oe)), ("src.u8", &os[..])] {
        let tmp = cfg.cache.join(format!("dem-cache.{name}.tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, cfg.cache.join(format!("dem-cache.{name}")))?;
    }
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(stats)
}

#[cfg(test)]
mod tests;
