//! The one sampler of the declared elevation datasets (docs/inputs.md §6.8): a unit's vertices
//! sampled from its plan (crate::dem::declared::Plan), no source named in code.
//!
//! - **The order at a point:** the plan's datasets, in their order, whose coverage holds it; the
//!   first with valid data there gives its height and its source byte.
//! - **Blended at the edges:** where the next dataset with valid data there (B) lies beneath the
//!   first (A), and the point is within a band of four of the coarser one's pixels of A's invalid
//!   area (its no data, or past its files' edges), the two blend by a smoothstep of the distance d:
//!   h = B + (A − B)·s(d / band), s(t) = t²(3 − 2t), so a seam shows no step. Past the band the height
//!   is A's, to the bit. The distance is to the nearest invalid pixel's cell, in metres.
//! - A GeoTIFF dataset's files are tried in the index's order, a point in those whose footprint
//!   holds it (min x ≤ x < max x, min y < y ≤ max y, in the files' CRS), sampled where they lie (a
//!   range at a time: crate::dem::sample) at the overview `read_resolution` picks; a tile pyramid's
//!   tiles are fetched and decoded by their encoding (crate::dem::encoding), each sampled on its own
//!   (its edges clamping).
//! - **The distance to the invalid area** is looked for pixel by pixel only where it can be found:
//!   each block (or tile) read for sampling is noted as clean when every pixel of it holds data
//!   (crate::dem::sample::Clean), and a point whose band lies inside clean blocks of its file is at
//!   the band's full distance, no pixel read again. The files' decoded blocks share one cache
//!   (`CACHE_BYTES`), whatever their number.

use super::declared::{Cover, Dataset, Plan, Source};
use super::proj::{mercator_tile, Atlas};
use super::sample::{bilinear, sample_raster_noting, window_clean, within, Clean, NODATA_BELOW};
use crate::fetch::{zip_member, Fetch};
use crate::geotiff::{BlockCache, Tiff};
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use store::range::{PlainFile, RangeRead};

/// How the files are read.
pub struct Opts<'a> {
    pub fetch: &'a dyn Fetch,
    /// The stores' folder (`sources/fetched/`): a dataset declared `store` keeps its files in
    /// `<it>/<dataset>/`. None: read in place.
    pub store: Option<PathBuf>,
    /// The store only read (a task's worker).
    pub read_only: bool,
    pub pool: Option<&'a rayon::ThreadPool>,
}

/// The heights sampled at points.
#[derive(Clone, Debug, Default)]
pub struct Sampled {
    /// NaN where no dataset has data.
    pub elev: Vec<f32>,
    /// The first dataset's code (0: none).
    pub src: Vec<u8>,
    /// Whether the height is a blend (within a band of the first dataset's invalid area).
    pub blended: Vec<bool>,
}

/// Metres per degree, as the resolutions and distances of geographic files are reckoned.
pub const M_PER_DEG: f64 = 111_320.0;

/// The decoded blocks kept across every open file of a run (one cache for all: a unit opens
/// tens of files a dataset, and sampling reads each block once).
pub const CACHE_BYTES: usize = 512 << 20;

/// Each open file's id in the shared cache.
static FILE_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Pixel size in metres of a geotransform in CRS `crs`, at latitude `lat`: (x, y).
fn pixel_m(crs: u16, gt: &[f64; 6], lat: f64) -> (f64, f64) {
    if crs == 3979 {
        (gt[1].abs(), gt[5].abs())
    } else {
        (gt[1].abs() * M_PER_DEG * lat.to_radians().cos(), gt[5].abs() * M_PER_DEG)
    }
}

/// A file's resolution in metres (its pixels' height), at level `level`.
pub fn resolution(t: &Tiff, crs: u16, level: usize) -> Result<f64> {
    let gt = t.transform(level)?;
    Ok(if crs == 3979 { gt[1].abs().max(gt[5].abs()) } else { gt[5].abs() * M_PER_DEG })
}

/// The level of `t` read for `read_resolution` (metres): the image whose resolution is nearest it
/// (the first of two as near); the full resolution without one.
pub fn level_for(t: &Tiff, crs: u16, read_resolution: Option<f64>) -> Result<usize> {
    let Some(r) = read_resolution else { return Ok(0) };
    let mut best = (f64::INFINITY, 0);
    for k in 0..t.images().len() {
        let d = (resolution(t, crs, k)? - r).abs();
        if d < best.0 - 1e-9 {
            best = (d, k);
        }
    }
    Ok(best.1)
}

/// A tile pyramid's tiles' size (pixels): Web Mercator's 256. A pyramid of another is refused, by
/// the input check (el-data) and by the sampler.
pub const TILE_PX: usize = 256;

/// A tile pyramid's resolution in metres at latitude `lat`.
pub fn tile_resolution(zoom: u32, lat: f64) -> f64 {
    40_075_016.686 * lat.to_radians().cos() / (256.0 * (1u64 << zoom) as f64)
}

/// Whether a value is data.
pub(crate) fn valid(v: f32, nodata: Option<f64>) -> bool {
    v.is_finite() && v >= NODATA_BELOW && nodata.is_none_or(|nd| v as f64 != nd)
}

/// Opens a file: a URL (`<zip URL>#<member>` for a zip's member), or a local path; None when the
/// server has none.
pub fn open_url(fetch: &dyn Fetch, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
    if url.starts_with('/') {
        return Ok(Some(Arc::new(PlainFile::open(std::path::Path::new(url))?)));
    }
    match url.split_once('#') {
        Some((zip, member)) => match fetch.open(zip)? {
            None => Ok(None),
            Some(z) => zip_member(z, member).with_context(|| format!("{zip}: its file list can't be read")),
        },
        None => fetch.open(url),
    }
}

/// A file's name in a store: a zip member's, else the URL's last part.
fn stored_name(url: &str) -> String {
    let u = url.rsplit_once('#').map_or(url, |(_, m)| m);
    u.rsplit('/').next().unwrap_or(u).to_string()
}

/// One GeoTIFF dataset's files as they're read.
struct GeoSet<'a> {
    id: &'a str,
    crs: u16,
    files: &'a [(String, Option<[f64; 4]>)],
    read_resolution: Option<f64>,
    store: Option<PathBuf>,
    o: &'a Opts<'a>,
    /// By file: the open file and the level read (None: the server has none).
    opened: Mutex<HashMap<usize, Option<(Arc<Tiff>, usize)>>>,
    /// By file: which of its blocks are known to hold data in every pixel.
    clean: Vec<Clean>,
    /// The decoded blocks of every file, under one budget.
    cache: Arc<Mutex<BlockCache>>,
}

/// A point in a footprint (min x ≤ x < max x, min y < y ≤ max y).
fn in_box(b: &[f64; 4], x: f64, y: f64) -> bool {
    x >= b[0] && x < b[2] && y > b[1] && y <= b[3]
}

impl GeoSet<'_> {
    fn open(&self, k: usize, again: bool) -> Result<Option<(Arc<Tiff>, usize)>> {
        if !again {
            if let Some(t) = self.opened.lock().unwrap().get(&k) {
                return Ok(t.clone());
            }
        }
        let url = &self.files[k].0;
        let src: Option<Arc<dyn RangeRead>> = match &self.store {
            Some(dir) if !url.starts_with('/') => {
                let remote = || open_url(self.o.fetch, url);
                match super::kept::kept(dir, &stored_name(url), &remote, again, self.o.read_only)? {
                    None => None,
                    Some(super::kept::Kept::Stored(p)) => Some(Arc::new(PlainFile::open(&p)?)),
                    Some(super::kept::Kept::InPlace(s)) => Some(s),
                }
            }
            _ => open_url(self.o.fetch, url)?,
        };
        let t = match src {
            None => bail!("{}: {url}, which its index lists, isn't on the server (the index is out of date?)", self.id),
            Some(s) => {
                let id = FILE_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let t = Tiff::open(s).with_context(|| url.clone())?.with_shared_cache(id, self.cache.clone());
                let level = level_for(&t, self.crs, self.read_resolution)?;
                Some((Arc::new(t), level))
            }
        };
        self.opened.lock().unwrap().insert(k, t.clone());
        Ok(t)
    }

    /// The files whose footprint holds native point (x, y), in order.
    fn holding(&self, x: f64, y: f64) -> impl Iterator<Item = usize> + '_ {
        self.files.iter().enumerate().filter(move |(_, (_, b))| b.is_none_or(|b| in_box(&b, x, y))).map(|(k, _)| k)
    }

    /// Points `sel` (native `x`, `y`) sampled: each one's value (NaN: none) and the file it came from.
    fn sample(&self, sel: &[u32], x: &[f64], y: &[f64]) -> Result<Vec<(f32, u32)>> {
        let mut out = vec![(f32::NAN, u32::MAX); sel.len()];
        let mut todo: Vec<Vec<usize>> = vec![Vec::new(); self.files.len()];
        for (j, &i) in sel.iter().enumerate() {
            if let Some(k) = self.holding(x[i as usize], y[i as usize]).next() {
                todo[k].push(j);
            }
        }
        // (A point whose first file has no data there tries the next that holds it.)
        for k in 0..self.files.len() {
            let mine: Vec<usize> = std::mem::take(&mut todo[k]);
            if mine.is_empty() {
                continue;
            }
            let (xs, ys): (Vec<f64>, Vec<f64>) = mine.iter().map(|&j| (x[sel[j] as usize], y[sel[j] as usize])).unzip();
            let Some((t, level)) = self.open(k, false)? else { continue };
            let v = match sample_raster_noting(&t, level, &xs, &ys, self.o.pool, Some(&self.clean[k])) {
                Ok(v) => v,
                // A stored copy that won't read: taken again (once).
                Err(_) if self.store.is_some() => {
                    let (t, level) = self.open(k, true)?.context("gone from the server")?;
                    sample_raster_noting(&t, level, &xs, &ys, self.o.pool, Some(&self.clean[k])).with_context(|| self.files[k].0.clone())?
                }
                Err(e) => return Err(e.context(self.files[k].0.clone())),
            };
            for (&j, v) in mine.iter().zip(v) {
                if v.is_finite() {
                    out[j] = (v, k as u32);
                } else {
                    let i = sel[j] as usize;
                    if let Some(n) = self.holding(x[i], y[i]).find(|&n| n > k) {
                        todo[n].push(j);
                    }
                }
            }
        }
        Ok(out)
    }

    /// The value at native point (x, y) of the files but `skip`: the first holding it whose raster
    /// has it (its nearest pixel); None when none does.
    fn value_at(&self, x: f64, y: f64, skip: usize) -> Result<Option<f32>> {
        for k in self.holding(x, y).filter(|&k| k != skip) {
            let Some((t, level)) = self.open(k, false)? else { continue };
            let gt = t.transform(level)?;
            let img = t.level(level)?;
            let (c, r) = (((x - gt[0]) / gt[1]).floor(), ((y - gt[3]) / gt[5]).floor());
            if c >= 0.0 && r >= 0.0 && c < img.width as f64 && r < img.height as f64 {
                let v = t.pixel(level, c as u32, r as u32)?;
                return Ok(Some(if valid(v, t.nodata()) { v } else { f32::NAN }));
            }
        }
        Ok(None)
    }

    /// The distance (m) from each point (native `x`, `y`, latitude `lat`; its file `file`) to the
    /// nearest pixel where the dataset has no data, up to `radius` (m): `radius` when none is nearer.
    fn distance(&self, sel: &[u32], x: &[f64], y: &[f64], lat: &[f64], file: &[u32], radius: &[f64]) -> Result<Vec<f64>> {
        let one = |j: usize| -> Result<f64> {
            let i = sel[j] as usize;
            let k = file[j] as usize;
            let (t, level) = self.open(k, false)?.context("an open file")?;
            let gt = t.transform(level)?;
            let img = t.level(level)?;
            let (w, h) = (img.width as i64, img.height as i64);
            let (mx, my) = pixel_m(self.crs, &gt, lat[i]);
            let cf = (x[i] - gt[0]) / gt[1] - 0.5;
            let rf = (y[i] - gt[3]) / gt[5] - 0.5;
            let rx = (radius[j] / mx).ceil() as i64 + 1;
            let ry = (radius[j] / my).ceil() as i64 + 1;
            let (c0, r0) = (cf.floor() as i64, rf.floor() as i64);
            let (cs, ce, rs, re) = (c0 - rx, c0 + 1 + rx, r0 - ry, r0 + 1 + ry);
            // (A window inside the image, in blocks holding data in every pixel: nothing in it is
            // invalid, so the distance is the radius, as the scan below would find.)
            if cs >= 0 && rs >= 0 && ce < w && re < h && window_clean(&t, level, &self.clean[k], cs, ce, rs, re)? {
                return Ok(radius[j]);
            }
            let (ics, ice, irs, ire) = (cs.max(0), ce.min(w - 1), rs.max(0), re.min(h - 1));
            let inside = if ics <= ice && irs <= ire { Some(t.read_window(level, ics as u32, irs as u32, (ice - ics + 1) as u32, (ire - irs + 1) as u32)?) } else { None };
            let iw = (ice - ics + 1).max(0) as usize;
            let nodata = t.nodata();
            let mut best = radius[j];
            for r in rs..=re {
                for c in cs..=ce {
                    let dx = ((c as f64 - cf).abs() - 0.5).max(0.0) * mx;
                    let dy = ((r as f64 - rf).abs() - 0.5).max(0.0) * my;
                    let d = dx.hypot(dy);
                    if d >= best {
                        continue;
                    }
                    let ok = if c >= ics && c <= ice && r >= irs && r <= ire {
                        let a = inside.as_ref().unwrap();
                        valid(a[(r - irs) as usize * iw + (c - ics) as usize], nodata)
                    } else {
                        let (wx, wy) = (gt[0] + (c as f64 + 0.5) * gt[1], gt[3] + (r as f64 + 0.5) * gt[5]);
                        self.value_at(wx, wy, k)?.is_some_and(|v| v.is_finite())
                    };
                    if !ok {
                        best = d;
                    }
                }
            }
            Ok(best)
        };
        // (In the files' blocks' order, so each thread's run of points reads the blocks its
        // neighbours just read, from the files' caches.)
        let q = if self.crs == 3979 { 1000.0 } else { 0.01 };
        let mut order: Vec<usize> = (0..sel.len()).collect();
        order.sort_by_key(|&j| {
            let i = sel[j] as usize;
            (file[j], (y[i] / q).floor() as i64, (x[i] / q).floor() as i64)
        });
        let got: Vec<f64> = within(self.o.pool, || order.par_iter().map(|&j| one(j)).collect::<Result<_>>())?;
        let mut out = vec![0.0; sel.len()];
        for (&j, d) in order.iter().zip(got) {
            out[j] = d;
        }
        Ok(out)
    }
}

/// One tile pyramid as it's read.
struct TileSet<'a> {
    url: &'a str,
    zoom: u32,
    encoding: &'a str,
    o: &'a Opts<'a>,
    /// By tile: whether every pixel holds data (false too for a tile the server hasn't).
    clean: Mutex<HashMap<(i64, i64), bool>>,
}

impl TileSet<'_> {
    fn tile(&self, tx: i64, ty: i64) -> Result<Option<(usize, usize, Vec<f32>)>> {
        let u = self.url.replace("{z}", &self.zoom.to_string()).replace("{x}", &tx.to_string()).replace("{y}", &ty.to_string());
        let Some(png) = self.o.fetch.get(&u)? else { return Ok(None) };
        let (w, h, a) = super::encoding::decode(self.encoding, &png).with_context(|| u.clone())?;
        if w * h == 0 {
            bail!("{u}: an empty tile");
        }
        // (The pyramid's pixels are found as 256 to a tile, Web Mercator's: a tile of another size
        // would be read at the wrong places. The input check refuses such a pyramid, el-data.)
        if w != TILE_PX || h != TILE_PX {
            bail!("{u}: a {w} × {h} tile; tile pyramids are read as {TILE_PX} × {TILE_PX}");
        }
        Ok(Some((w, h, a)))
    }

    /// `tile`, noted in `clean`: whether every pixel of it holds data.
    fn tile_noted(&self, tx: i64, ty: i64) -> Result<Option<(usize, usize, Vec<f32>)>> {
        let t = self.tile(tx, ty)?;
        let clean = t.as_ref().is_some_and(|(_, _, a)| a.iter().all(|&v| valid(v, None)));
        self.clean.lock().unwrap().insert((tx, ty), clean);
        Ok(t)
    }

    /// Points `sel` grouped by tile: (tile key, the points' positions in `sel`, fx, fy).
    fn groups(&self, sel: &[u32], lon: &[f64], lat: &[f64]) -> Vec<Vec<(i64, usize, f64, f64)>> {
        let n = 1i64 << self.zoom;
        let mut pts: Vec<(i64, usize, f64, f64)> = sel
            .iter()
            .enumerate()
            .map(|(j, &i)| {
                let (fx, fy) = mercator_tile(lon[i as usize], lat[i as usize], self.zoom);
                (fx.floor() as i64 * n + fy.floor() as i64, j, fx, fy)
            })
            .collect();
        pts.sort_by_key(|p| p.0);
        pts.chunk_by(|a, b| a.0 == b.0).map(|g| g.to_vec()).collect()
    }

    /// Points `sel` sampled, each tile on its own (its edges clamping).
    fn sample(&self, sel: &[u32], lon: &[f64], lat: &[f64]) -> Result<Vec<f32>> {
        let n = 1i64 << self.zoom;
        let groups = self.groups(sel, lon, lat);
        let sampled: Vec<Vec<(usize, f32)>> = within(self.o.pool, || {
            groups
                .par_iter()
                .map(|g| -> Result<Vec<(usize, f32)>> {
                    let (tx, ty) = (g[0].0.div_euclid(n), g[0].0.rem_euclid(n));
                    let Some((w, h, a)) = self.tile_noted(tx, ty)? else { return Ok(Vec::new()) };
                    Ok(g.iter().map(|&(_, j, fx, fy)| (j, bilinear(&a, w, h, (fx - tx as f64) * 256.0 - 0.5, (fy - ty as f64) * 256.0 - 0.5))).collect())
                })
                .collect::<Result<_>>()
        })?;
        let mut out = vec![f32::NAN; sel.len()];
        for (j, v) in sampled.into_iter().flatten() {
            out[j] = v;
        }
        Ok(out)
    }

    /// As GeoSet::distance, on the pyramid's pixels (each tile's neighbours read where a band
    /// crosses into them).
    fn distance(&self, sel: &[u32], lon: &[f64], lat: &[f64], radius: &[f64]) -> Result<Vec<f64>> {
        let n = 1i64 << self.zoom;
        let groups = self.groups(sel, lon, lat);
        let got: Vec<Vec<(usize, f64)>> = within(self.o.pool, || {
            groups
                .par_iter()
                .map(|g| -> Result<Vec<(usize, f64)>> {
                    let mut tiles: HashMap<(i64, i64), Option<Arc<(usize, usize, Vec<f32>)>>> = HashMap::new();
                    let mut out = Vec::with_capacity(g.len());
                    for &(_, j, fx, fy) in g {
                        let i = sel[j] as usize;
                        let pm = tile_resolution(self.zoom, lat[i]);
                        let (gx, gy) = (fx * 256.0 - 0.5, fy * 256.0 - 0.5);
                        let rp = (radius[j] / pm).ceil() as i64 + 1;
                        let (c0, r0) = (gx.floor() as i64, gy.floor() as i64);
                        let mut best = radius[j];
                        // (The band's tiles: one known clean has no invalid pixel, so none of its
                        // pixels is looked at, nor is it fetched again.)
                        let (cs, ce, rs, re) = (c0 - rp, c0 + 1 + rp, r0 - rp, r0 + 1 + rp);
                        let clean_tile = |tx: i64, ty: i64| self.clean.lock().unwrap().get(&(tx, ty)).copied();
                        let all_clean = (rs.div_euclid(256)..=re.div_euclid(256)).all(|ty| (cs.div_euclid(256)..=ce.div_euclid(256)).all(|tx| tx >= 0 && ty >= 0 && tx < n && ty < n && clean_tile(tx, ty) == Some(true)));
                        if all_clean {
                            out.push((j, best));
                            continue;
                        }
                        for r in rs..=re {
                            for c in cs..=ce {
                                let d = ((c as f64 - gx).abs() - 0.5).max(0.0).hypot(((r as f64 - gy).abs() - 0.5).max(0.0)) * pm;
                                if d >= best {
                                    continue;
                                }
                                let (tx, ty) = (c.div_euclid(256), r.div_euclid(256));
                                let ok = if tx < 0 || ty < 0 || tx >= n || ty >= n {
                                    false
                                } else if clean_tile(tx, ty) == Some(true) {
                                    true
                                } else {
                                    if !tiles.contains_key(&(tx, ty)) {
                                        let t = self.tile_noted(tx, ty)?.map(Arc::new);
                                        tiles.insert((tx, ty), t);
                                    }
                                    match &tiles[&(tx, ty)] {
                                        None => false,
                                        Some(t) => {
                                            let (w, h, a) = &**t;
                                            let (lc, lr) = (c.rem_euclid(256) as usize, r.rem_euclid(256) as usize);
                                            lc < *w && lr < *h && valid(a[lr * w + lc], None)
                                        }
                                    }
                                };
                                if !ok {
                                    best = d;
                                }
                            }
                        }
                        out.push((j, best));
                    }
                    Ok(out)
                })
                .collect::<Result<_>>()
        })?;
        let mut out = vec![0.0; sel.len()];
        for (j, d) in got.into_iter().flatten() {
            out[j] = d;
        }
        Ok(out)
    }
}

/// A dataset as it's read.
enum Reader<'a> {
    Geo(GeoSet<'a>, Vec<f64>, Vec<f64>),
    Tiles(TileSet<'a>),
}

impl<'a> Reader<'a> {
    fn new(ds: &'a Dataset, o: &'a Opts<'a>, lon: &[f64], lat: &[f64], cover: &Cover, cache: &Arc<Mutex<BlockCache>>) -> Result<Reader<'a>> {
        Ok(match &ds.source {
            Source::Tiles { url, zoom, encoding } => Reader::Tiles(TileSet { url, zoom: *zoom, encoding, o, clean: Mutex::new(HashMap::new()) }),
            Source::Geotiff { crs, read_resolution, store, files } => {
                let set = GeoSet { id: &ds.id, crs: *crs, files, read_resolution: *read_resolution, store: if *store { o.store.as_ref().map(|s| s.join(&ds.id)) } else { None }, o, opened: Mutex::new(HashMap::new()), clean: (0..files.len()).map(|_| Clean::default()).collect(), cache: cache.clone() };
                // (Native coordinates of the points it covers; NaN for the others.)
                let (x, y) = match crs {
                    4326 | 4269 => (lon.to_vec(), lat.to_vec()),
                    3979 => {
                        let atlas = Atlas::new();
                        let xy: Vec<(f64, f64)> = within(o.pool, || lon.par_iter().zip(lat.par_iter()).map(|(&lo, &la)| if cover.contains(lo, la) { atlas.project(lo, la) } else { (f64::NAN, f64::NAN) }).collect());
                        xy.into_iter().unzip()
                    }
                    c => bail!("{}: EPSG:{c} isn't a CRS the app knows", ds.id),
                };
                Reader::Geo(set, x, y)
            }
        })
    }

    /// Points `sel` sampled: each one's value and (GeoTIFFs) its file.
    fn sample(&self, sel: &[u32], lon: &[f64], lat: &[f64]) -> Result<Vec<(f32, u32)>> {
        match self {
            Reader::Geo(g, x, y) => g.sample(sel, x, y),
            Reader::Tiles(t) => Ok(t.sample(sel, lon, lat)?.into_iter().map(|v| (v, 0)).collect()),
        }
    }

    fn distance(&self, sel: &[u32], lon: &[f64], lat: &[f64], file: &[u32], radius: &[f64]) -> Result<Vec<f64>> {
        match self {
            Reader::Geo(g, x, y) => g.distance(sel, x, y, lat, file, radius),
            Reader::Tiles(t) => t.distance(sel, lon, lat, radius),
        }
    }
}

/// s(t) = t²(3 − 2t).
pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A blend of A over B at distance `d` of a band `band` wide: A's value to the bit past it.
pub fn blend(a: f32, b: f32, d: f64, band: f64) -> f32 {
    if d >= band {
        return a;
    }
    let s = smoothstep(d / band);
    (b as f64 + (a as f64 - b as f64) * s) as f32
}

/// Points `lon`, `lat` (degrees) sampled from `plan`. `said`: how many have a height so far.
pub fn sample(plan: &Plan, lon: &[f64], lat: &[f64], o: &Opts, said: &dyn Fn(usize)) -> Result<Sampled> {
    use crate::timings::{phase, Class};
    let n = lon.len();
    let covers: Vec<Cover> = plan.datasets.iter().map(|d| Cover::of(d.cover.as_deref())).collect();
    let mut a = vec![f32::NAN; n];
    let mut first = vec![u16::MAX; n];
    let mut file = vec![u32::MAX; n];
    let mut readers = Vec::new();
    let mut got = 0;
    let cache = Arc::new(Mutex::new(BlockCache::new(CACHE_BYTES)));
    // (What each phase fetches over the network: its count.)
    let fetched = |p: &crate::timings::Phase, since: u64| p.count(crate::fetch::fetched_bytes().saturating_sub(since), 0);
    for (k, ds) in plan.datasets.iter().enumerate() {
        let p = phase(&format!("{} sampled", ds.id), Class::Net);
        let since = crate::fetch::fetched_bytes();
        let r = Reader::new(ds, o, lon, lat, &covers[k], &cache)?;
        let sel: Vec<u32> = (0..n as u32).filter(|&i| first[i as usize] == u16::MAX && covers[k].contains(lon[i as usize], lat[i as usize])).collect();
        if !sel.is_empty() {
            let v = r.sample(&sel, lon, lat)?;
            let mut here = 0;
            for (&i, (v, f)) in sel.iter().zip(v) {
                if v.is_finite() {
                    (a[i as usize], first[i as usize], file[i as usize]) = (v, k as u16, f);
                    here += 1;
                }
            }
            got += here;
            said(got);
            println!("  {}: {here} of {} vertices", ds.id, sel.len());
        }
        readers.push(r);
        fetched(&p, since);
    }
    // The blends: each point's distance to its first dataset's invalid area, up to the widest band
    // it could have (four of the coarsest pixels of the datasets after it there); then the next
    // dataset with data, for those within it.
    let p = phase("the edges blended", Class::Net);
    let since = crate::fetch::fetched_bytes();
    let res: Vec<f64> = plan.datasets.iter().map(|d| d.resolution).collect();
    let mut dist = vec![f64::INFINITY; n];
    for k in 0..plan.datasets.len() {
        let mut sel = Vec::new();
        let mut radius = Vec::new();
        for i in 0..n {
            if first[i] as usize != k {
                continue;
            }
            let coarsest = (k + 1..plan.datasets.len()).filter(|&m| covers[m].contains(lon[i], lat[i])).map(|m| res[m]).fold(0.0f64, f64::max);
            if coarsest > 0.0 {
                sel.push(i as u32);
                radius.push(4.0 * coarsest.max(res[k]));
            }
        }
        if sel.is_empty() {
            continue;
        }
        let files: Vec<u32> = sel.iter().map(|&i| file[i as usize]).collect();
        let d = readers[k].distance(&sel, lon, lat, &files, &radius)?;
        for ((&i, d), r) in sel.iter().zip(d).zip(radius) {
            if d < r {
                dist[i as usize] = d;
            }
        }
    }
    let mut b = vec![f32::NAN; n];
    let mut second = vec![u16::MAX; n];
    for (k, r) in readers.iter().enumerate() {
        let sel: Vec<u32> = (0..n as u32).filter(|&i| dist[i as usize].is_finite() && second[i as usize] == u16::MAX && (first[i as usize] as usize) < k && covers[k].contains(lon[i as usize], lat[i as usize])).collect();
        if sel.is_empty() {
            continue;
        }
        for (&i, (v, _)) in sel.iter().zip(r.sample(&sel, lon, lat)?) {
            if v.is_finite() {
                (b[i as usize], second[i as usize]) = (v, k as u16);
            }
        }
    }
    fetched(&p, since);
    drop(p);
    let mut out = Sampled { elev: a, src: vec![0; n], blended: vec![false; n] };
    for i in 0..n {
        if first[i] == u16::MAX {
            continue;
        }
        out.src[i] = plan.datasets[first[i] as usize].code;
        if second[i] != u16::MAX {
            let band = 4.0 * res[first[i] as usize].max(res[second[i] as usize]);
            if dist[i] < band {
                out.elev[i] = blend(out.elev[i], b[i], dist[i], band);
                out.blended[i] = true;
            }
        }
    }
    Ok(out)
}

/// dem-stats.json of a run from declarations: vertices by dataset.
#[derive(Debug, Default, serde::Serialize)]
pub struct Stats {
    pub vertices: usize,
    pub datasets: std::collections::BTreeMap<String, usize>,
    pub blended: usize,
    pub missing: usize,
    pub sampled_this_run: usize,
    pub seconds: f64,
}

/// `elev` from a plan: samples the build folder's vertices (`verts.bin`) not in the cache; writes
/// `elev.f32`, `src.u8`, `dem-stats.json` and the cache for the next run, as `super::run` does.
pub fn run(cfg: &super::Config, plan: &Plan, fetch: &dyn Fetch) -> Result<Stats> {
    use crate::timings::{phase, Class};
    let t_start = std::time::Instant::now();
    let b = &cfg.build;
    let p = phase("the cache's heights reused", Class::Disk);
    let va = roadcore::Array::<[i32; 2]>::open(&b.join("verts.bin"))?;
    let verts = va.get();
    let n = verts.len();
    let stamp = format!("{n}:{}:{}", super::mtime_ns(&b.join("verts.bin"))?, plan.tag);
    println!("{} vertices", super::th(n));
    let mut prog = super::Progress::open(b, stamp, n)?;
    if !prog.resumed {
        if !cfg.no_cache {
            super::reuse(&cfg.cache, verts, &mut prog)?;
        }
        prog.mark("cache")?;
    }
    drop(p);
    let miss: Vec<u32> = (0..n as u32).filter(|&i| prog.elev[i as usize].is_nan()).collect();
    println!("{} vertices need DEM sampling", super::th(miss.len()));
    super::said(0, miss.len());
    let lon: Vec<f64> = miss.iter().map(|&i| verts[i as usize][0] as f64 * 1e-7).collect();
    let lat: Vec<f64> = miss.iter().map(|&i| verts[i as usize][1] as f64 * 1e-7).collect();
    let pool = super::pool(cfg.workers);
    let o = Opts { fetch, store: cfg.store.clone(), read_only: cfg.read_only, pool: pool.as_ref() };
    let s = sample(plan, &lon, &lat, &o, &|k| super::said(k, miss.len()))?;
    drop(o);
    drop(pool);
    super::said(miss.len(), miss.len());
    prog.scatter(&miss, &s.elev, &s.src);
    let _p = phase("heights and the cache written", Class::Disk);
    let mut by: std::collections::BTreeMap<String, usize> = plan.datasets.iter().map(|d| (d.id.clone(), 0)).collect();
    let mut missing = 0;
    for &c in &prog.src {
        match plan.datasets.iter().find(|d| d.code == c) {
            Some(d) if c != 0 => *by.get_mut(&d.id).unwrap() += 1,
            _ => missing += 1,
        }
    }
    let stats = Stats { vertices: n, datasets: by, blended: s.blended.iter().filter(|b| **b).count(), missing, sampled_this_run: miss.len(), seconds: (t_start.elapsed().as_secs_f64() * 10.0).round() / 10.0 };
    let (elev, src) = prog.finish()?;
    std::fs::write(b.join("dem-stats.json"), serde_json::to_string_pretty(&stats)?)?;
    std::fs::remove_file(b.join("dem-progress.json")).ok();
    super::write_cache(&cfg.cache, verts, &elev, &src)?;
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(stats)
}

#[cfg(test)]
#[path = "generic_tests.rs"]
mod tests;
