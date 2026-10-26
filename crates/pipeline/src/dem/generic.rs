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
//!   (its edges clamping). A run fetches each tile once (`TileMemo`, whatever stage reads it), up
//!   to `TILE_FETCHES` at once kept in flight ahead of the sampling, each tile's points sampled as
//!   it arrives (`TileSet::rolling`), so a stage reading few points a tile (the blends) waits on
//!   the network no longer than it must, and a slow tile holds up its own points alone. A tile's
//!   request is held to `TILE_WITHIN`, and asked again past it. A GeoTIFF dataset's blocks are read
//!   likewise ahead of their sampling (`read_ahead`): `GEO_FETCHES` at once, up to `GEO_AHEAD`
//!   blocks past the chunk being sampled, each once (crate::geotiff::Tiff::block); so the sampler
//!   waits on several requests at once, not on a chunk's few blocks in turn.
//! - **The distance to the invalid area** is looked for pixel by pixel only where it can be found:
//!   each block (or tile) read for sampling is noted as clean when every pixel of it holds data
//!   (crate::dem::sample::Clean), and a point whose band lies inside clean blocks of its file is at
//!   the band's full distance, no pixel read again. The others share a map of the invalid pixels
//!   around them, a GeoTIFF's by file and square of `DIST_CELL` pixels, a pyramid's by tile: by
//!   row, each pixel's nearest invalid one either side (`nearest_invalid`), so a point looks at two
//!   pixels a row of its band, not every one (what a scan of the whole band finds, to the bit).
//!   The files' decoded blocks share one cache (`CACHE_BYTES`), whatever their number.

use super::declared::{Cover, Dataset, Plan, Source};
use super::proj::{mercator_tile, projected, Projection};
use super::sample::{bilinear, blocks_read, sample_raster_noting, window_clean, Clean, Seen, NODATA_BELOW};
use crate::fetch::Fetch;
use crate::geotiff::{BlockCache, Tiff};
use anyhow::{bail, ensure, Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use store::range::{PlainFile, RangeRead};

/// How the files are read.
pub struct Opts<'a> {
    pub fetch: &'a dyn Fetch,
    /// The stores' folder (`sources/fetched/`): a dataset declared `store` is read from
    /// `<it>/<dataset>/` alone (`stored`).
    pub store: Option<store::nasfs::NasPath>,
    /// This Mac's whole copies of the stored files (crate::localread, task #689): a stored file read
    /// at a level holding `COPY_SHARE` of it or more is copied there once and read there.
    pub local: Option<&'a crate::localread::Local>,
}

/// The share of a stored file's pixels (its levels' together) the level read must hold for the
/// file to be copied whole to this Mac (`Opts::local`), not read by ranges on the NAS. A copy runs
/// at ~100 MB/s; a range read waits ~40 ms on the NAS's seeks, so copying pays from a few percent
/// read, and this keeps a run over many files at a coarse level (terrain-lo's: 1,648 at its 90th
/// percentile) to its ranges, its disk unused. GLO-30's levels hold 75, 19, 4.7 and 1.2 % of a
/// file: the two finest copy. A fill mask, read whole (`Mask::of`), copies.
pub const COPY_SHARE: f64 = 0.1;

/// The share of `t`'s pixels image `level` holds.
fn level_share(t: &Tiff, level: usize) -> f64 {
    let px = |i: &crate::geotiff::Image| i.width as f64 * i.height as f64;
    let all: f64 = t.images().iter().map(px).sum();
    t.images().get(level).map_or(0.0, |i| px(i) / all.max(1.0))
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
    /// Heights left in their dataset's reference: no declared geoid source has a value there.
    pub unconverted: usize,
    /// Blended heights whose first dataset's height was converted and whose second's stayed in its
    /// reference (no geoid source has a value there): a seam mixing references.
    pub unconverted_seam: usize,
}

/// Metres per degree, as the resolutions and distances of geographic files are reckoned.
pub const M_PER_DEG: f64 = 111_320.0;

/// The decoded blocks kept across every open file of a run (one cache for all: a unit opens
/// tens of files a dataset, and sampling reads each block once).
pub const CACHE_BYTES: usize = 512 << 20;

/// How many of a fetched GeoTIFF dataset's blocks are fetched at once ahead of their sampling
/// (natively; a stored one's are read as sampled), and how many blocks past the end of the chunk being sampled they go
/// at most (`ahead_for` holds that to a quarter of the cache).
pub const GEO_FETCHES: usize = 32;
pub const GEO_AHEAD: usize = 128;

/// How many blocks of `block_bytes` (decoded) to read ahead with a cache of `cache_bytes`:
/// `GEO_AHEAD`, but no more than a quarter of the cache holds, so the blocks read ahead never let
/// the chunk being sampled lose its own.
pub fn ahead_for(cache_bytes: usize, block_bytes: usize) -> usize {
    GEO_AHEAD.min(cache_bytes / block_bytes.max(1) / 4)
}

/// What to read ahead: the blocks, where each part's begin in that list (and its end), and how
/// many blocks past the part being sampled to go (`ahead_for`).
type Ahead = (Vec<AheadBlock>, Vec<usize>, usize);

/// A file's blocks to read ahead: the file, its level, the block's column and row.
type AheadBlock = (Arc<Tiff>, usize, u32, u32);

/// Runs `run` (the sampling of parts in turn, calling its argument with a part's number before
/// sampling it) while `GEO_FETCHES` threads read `ahead`'s blocks into the cache, in the list's
/// order, at most its count past the end of the part being sampled. A failed read stops the
/// reading ahead (the sampling's own read of that block says why); so does `run`'s end, however
/// it ends, a read waiting to retry too (crate::fetch::cancellable): a host that gives no answer
/// doesn't keep the threads, or the step, waiting out their retries. No list: `run` alone.
fn read_ahead<R>(ahead: Option<&Ahead>, run: impl FnOnce(&dyn Fn(usize)) -> R) -> R {
    let Some((blocks, starts, n_ahead)) = ahead.filter(|a| a.0.len() > 1) else { return run(&|_| {}) };
    // (The next block to read, how far reading may go, and whether to stop.)
    let st = Mutex::new((0usize, 0usize, false));
    let cv = Condvar::new();
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = || {
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        st.lock().unwrap_or_else(|e| e.into_inner()).2 = true;
        cv.notify_all();
    };
    // (Stopped however `run` ends, a panic too, so the threads end.)
    struct Stop<F: Fn()>(F);
    impl<F: Fn()> Drop for Stop<F> {
        fn drop(&mut self) {
            (self.0)()
        }
    }
    std::thread::scope(|s| {
        for _ in 0..GEO_FETCHES.min(blocks.len()) {
            s.spawn(|| loop {
                let i = {
                    let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
                    while !g.2 && g.0 < blocks.len() && g.0 >= g.1 {
                        g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                    }
                    if g.2 || g.0 >= blocks.len() {
                        return;
                    }
                    g.0 += 1;
                    g.0 - 1
                };
                let (t, level, bx, by) = &blocks[i];
                if crate::fetch::cancellable(&cancel, || t.block(*level, *bx, *by)).is_err() {
                    stop();
                    return;
                }
            });
        }
        let _stop = Stop(&stop);
        run(&|part: usize| {
            let mut g = st.lock().unwrap_or_else(|e| e.into_inner());
            g.1 = g.1.max(starts[(part + 1).min(starts.len() - 1)] + n_ahead);
            cv.notify_all();
        })
    })
}

/// Each open file's id in the shared cache.
static FILE_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Pixel size in metres of a geotransform in CRS `crs`, at latitude `lat`: (x, y).
pub(crate) fn pixel_m(crs: u16, gt: &[f64; 6], lat: f64) -> (f64, f64) {
    if projected(crs) {
        (gt[1].abs(), gt[5].abs())
    } else {
        (gt[1].abs() * M_PER_DEG * lat.to_radians().cos(), gt[5].abs() * M_PER_DEG)
    }
}

/// A file's resolution in metres (its pixels' height), at level `level`.
pub fn resolution(t: &Tiff, crs: u16, level: usize) -> Result<f64> {
    let gt = t.transform(level)?;
    Ok(if projected(crs) { gt[1].abs().max(gt[5].abs()) } else { gt[5].abs() * M_PER_DEG })
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

/// Opens a file: a URL, or a local path; None when the server has none.
pub fn open_url(fetch: &dyn Fetch, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
    if url.starts_with('/') {
        return Ok(Some(Arc::new(PlainFile::open(std::path::Path::new(url))?)));
    }
    fetch.open(url)
}

/// A file's name in a store: the index's URL's last part (after a `#`, a zip's member's name).
pub fn stored_name(url: &str) -> String {
    let u = url.rsplit_once('#').map_or(url, |(_, m)| m);
    u.rsplit('/').next().unwrap_or(u).to_string()
}

/// File `url` of dataset `id`, kept in store `dir` (`[data] store = true`): its stored copy, read
/// where it lies; None for one the store notes the source doesn't have (`<name>.none`). Never
/// fetched here: a file the store lacks, or holds cut short, is an error naming it (a stored
/// dataset is fetched once, by `scenic-build store-fetch <id> --index <its index>`: crate::dem::store_fetch).
/// A read the share fails for a moment is tried again (store::nasfs; store::retry): a "not found"
/// too, when the store's listing has the file (listed only when neither the file nor its `.none`
/// is found). A file the listing lacks fails at once, as does one cut short. With `local`, this
/// Mac's whole copy where it's here (task #689); else the NAS's file with its length (`Stored::nas`),
/// for the caller to copy.
pub fn stored(
    id: &str,
    dir: &store::nasfs::NasPath,
    url: &str,
    local: Option<&crate::localread::Local>,
) -> Result<Option<Stored>> {
    let name = stored_name(url);
    let f = dir.join(&name);
    let what = format!("{id}: tile {name} in the store ({})", dir.display());
    let len = match f.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(anyhow::Error::from(e).context(format!("{what}: its size")));
            }
            // (A tile with no data, its `.none` marker, is common: looked at by its name, not a listing.)
            match dir.join(format!("{name}.none")).metadata() {
                Ok(_) => return Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(anyhow::Error::from(e).context(format!("{what}: its .none"))),
            }
            let listed = dir.read_dir().map(|rd| {
                let none = format!("{name}.none");
                (rd.iter().any(|d| d.name == name), rd.iter().any(|d| d.name == none))
            });
            match listed {
            Ok((_, true)) => return Ok(None),
            Ok((false, false)) => bail!("{id}: tile {name} isn't in the store ({}): z6 tile builds don't download it; fetch it into the store first (scenic-build store-fetch {id} --index <its index>, docs/inputs.md §6.8)", dir.display()),
            Ok((true, false)) => eprintln!("{what}: not found, yet listed: the share's moment"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("{id}: the store {} isn't there", dir.display()),
            Err(e) => return Err(anyhow::Error::from(e).context(format!("{id}: listing the store ({})", dir.display()))),
        }
            f.metadata().with_context(|| format!("{what}: its size"))?.len()
        }
    };
    // (A copy here is the NAS's bytes: checked whole here, the NAS not read.)
    if let Some(l) = local.and_then(|c| c.here(&f, len)) {
        if std::fs::File::open(&l).and_then(|h| crate::whole::tiff_whole_checked(&h)).unwrap_or(false) {
            return Ok(Some(Stored { read: Arc::new(PlainFile::open(&l)?), nas: None }));
        }
    }
    let read = f
        .retry_with(&what, &store::retry::waits(), store::retry::transient_or_missing, || {
            Ok(crate::whole::tiff_whole_checked(&f.open()?.into_file())?.then(|| f.open_plain()).transpose()?)
        })
        .with_context(|| format!("{id}: reading tile {name} in the store ({})", dir.display()))?;
    let Some(file) = read else {
        bail!(
            "{id}: tile {name} in the store ({}) isn't whole: remove it and fetch it again (scenic-build store-fetch {id} --index <its index>)",
            dir.display()
        );
    };
    Ok(Some(Stored { read: Arc::new(file), nas: Some((f, len)) }))
}

/// A stored file as it's read (`stored`).
pub struct Stored {
    pub read: Arc<dyn RangeRead>,
    /// The NAS's file and its length when that is what's read (no copy of it here).
    pub nas: Option<(store::nasfs::NasPath, u64)>,
}

/// One GeoTIFF dataset's files as they're read.
struct GeoSet<'a> {
    id: &'a str,
    crs: u16,
    files: &'a [(String, Option<[f64; 4]>)],
    /// By file, its fill mask's URL (empty: none has one; `Source::Geotiff::masks`).
    masks: &'a [Option<String>],
    read_resolution: Option<f64>,
    store: Option<store::nasfs::NasPath>,
    o: &'a Opts<'a>,
    /// By file: the open file and the level read (None: the server has none).
    opened: Mutex<HashMap<usize, Option<(Arc<Tiff>, usize)>>>,
    /// By file: its fill mask's cell in the memo shared by every sampler of the run
    /// (`TileMemo::masks`), looked up once.
    opened_masks: Vec<std::sync::OnceLock<Arc<MaskCell>>>,
    memo: Arc<TileMemo>,
    /// The files by where their footprints lie, for a plan of many (`FileGrid`).
    grid: Option<FileGrid>,
    /// By file: which of its blocks are known to hold data in every pixel.
    clean: Vec<Clean>,
    /// The decoded blocks of every file, under one budget.
    cache: Arc<Mutex<BlockCache>>,
}

/// A file's fill mask, read whole once: by block, the pixels holding a void (a value `MASK_FILLED`
/// or more) as bits, row by row `block_w` wide (None: the block has none), so a point is looked up
/// without reading the file again.
struct Mask {
    gt: [f64; 6],
    width: u32,
    height: u32,
    block_w: u32,
    block_h: u32,
    across: u32,
    voids: Vec<Option<Box<[u64]>>>,
}

impl Mask {
    /// The mask of `t`, each block read once.
    fn of(t: Tiff) -> Result<Mask> {
        let gt = t.transform(0)?;
        let img = t.level(0)?.clone();
        let (across, down) = (img.width.div_ceil(img.block_w), img.height.div_ceil(img.block_h));
        let n = (img.block_w * img.block_h) as usize;
        let mut voids = Vec::with_capacity((across * down) as usize);
        for by in 0..down {
            for bx in 0..across {
                let b = t.block(0, bx, by)?;
                let mut bits = vec![0u64; n.div_ceil(64)];
                let mut any = false;
                for y in 0..b.height {
                    for x in 0..b.width {
                        if b.f32_at(x, y) >= super::declared::MASK_FILLED {
                            let i = y * img.block_w as usize + x;
                            bits[i / 64] |= 1 << (i % 64);
                            any = true;
                        }
                    }
                }
                voids.push(any.then(|| bits.into_boxed_slice()));
            }
        }
        Ok(Mask { gt, width: img.width, height: img.height, block_w: img.block_w, block_h: img.block_h, across, voids })
    }

    fn masked(&self, x: f64, y: f64) -> bool {
        let (c, r) = (((x - self.gt[0]) / self.gt[1]).floor(), ((y - self.gt[3]) / self.gt[5]).floor());
        if !(c >= 0.0 && r >= 0.0 && c < self.width as f64 && r < self.height as f64) {
            return false;
        }
        let (c, r) = (c as u32, r as u32);
        let Some(bits) = &self.voids[((r / self.block_h) * self.across + c / self.block_w) as usize] else {
            return false;
        };
        let i = ((r % self.block_h) * self.block_w + c % self.block_w) as usize;
        bits[i / 64] >> (i % 64) & 1 == 1
    }
}

/// A plan's files by where their footprints lie, for a plan of many (a terrain's base over a z3
/// tile or the world lists thousands): each square of the grid (as wide as the footprints'
/// median) with the files whose footprint meets it, and those without one, so a point's files are
/// looked for among a few, in the plan's order.
struct FileGrid {
    side: f64,
    squares: HashMap<(i64, i64), Vec<usize>>,
    everywhere: Vec<usize>,
}

impl FileGrid {
    /// The files a plan has past which they're found by the grid.
    const FROM: usize = 64;

    fn of(files: &[(String, Option<[f64; 4]>)]) -> FileGrid {
        let mut w: Vec<f64> = files.iter().filter_map(|(_, b)| b.map(|b| (b[2] - b[0]).max(b[3] - b[1]))).collect();
        w.sort_by(f64::total_cmp);
        let side = w.get(w.len() / 2).copied().filter(|s| *s > 0.0 && s.is_finite()).unwrap_or(1.0);
        let mut g = FileGrid { side, squares: HashMap::new(), everywhere: Vec::new() };
        for (k, (_, b)) in files.iter().enumerate() {
            match b {
                None => g.everywhere.push(k),
                Some(b) => {
                    let (x0, y0) = g.square(b[0], b[1]);
                    let (x1, y1) = g.square(b[2], b[3]);
                    // (A footprint far wider than the rest: everywhere, not in thousands of squares.)
                    if (x1 - x0 + 1) * (y1 - y0 + 1) > 4096 {
                        g.everywhere.push(k);
                        continue;
                    }
                    for sx in x0..=x1 {
                        for sy in y0..=y1 {
                            g.squares.entry((sx, sy)).or_default().push(k);
                        }
                    }
                }
            }
        }
        // (Each square's list with the files everywhere merged in, in the plan's order, once.)
        let every = g.everywhere.clone();
        for v in g.squares.values_mut() {
            v.extend(&every);
            v.sort_unstable();
            v.dedup();
        }
        g
    }

    fn square(&self, x: f64, y: f64) -> (i64, i64) {
        ((x / self.side).floor() as i64, (y / self.side).floor() as i64)
    }

    /// The files whose footprint may hold (x, y), in the plan's order.
    fn near(&self, x: f64, y: f64) -> &[usize] {
        if !(x.is_finite() && y.is_finite()) {
            return &[];
        }
        self.squares.get(&self.square(x, y)).map_or(&self.everywhere, Vec::as_slice)
    }
}

/// A point in a footprint (min x ≤ x < max x, min y < y ≤ max y).
fn in_box(b: &[f64; 4], x: f64, y: f64) -> bool {
    x >= b[0] && x < b[2] && y > b[1] && y <= b[3]
}

impl GeoSet<'_> {
    /// A file by URL, from the store where the dataset is kept there, open, with its image read
    /// (`level`'s answer): a stored file whose level holds `COPY_SHARE` of it or more copied to
    /// this Mac first (`Opts::local`; the same bytes) and read there.
    fn source(&self, url: &str, level: impl Fn(&Tiff) -> Result<usize>) -> Result<Option<(Tiff, usize)>> {
        let (read, nas) = match &self.store {
            Some(dir) if !url.starts_with('/') => match stored(self.id, dir, url, self.o.local)? {
                Some(s) => (s.read, s.nas),
                None => return Ok(None),
            },
            _ => match open_url(self.o.fetch, url)? {
                Some(r) => (r, None),
                None => return Ok(None),
            },
        };
        let t = Tiff::open(read).with_context(|| url.to_string())?;
        let k = level(&t)?;
        if let (Some(local), Some((f, len))) = (self.o.local, nas) {
            if level_share(&t, k) >= COPY_SHARE {
                if let store::nasfs::Place::Local(l) = local.path_sized(&f, len) {
                    let t = Tiff::open(Arc::new(PlainFile::open(&l)?)).with_context(|| url.to_string())?;
                    return Ok(Some((t, k)));
                }
            }
        }
        Ok(Some((t, k)))
    }

    /// File `k`'s fill mask, read once (None: it has none).
    fn mask(&self, k: usize) -> Result<Option<Arc<Mask>>> {
        let Some(Some(url)) = self.masks.get(k) else { return Ok(None) };
        self.opened_masks[k]
            .get_or_init(|| self.memo.masks.lock().unwrap().entry(format!("{} {url}", self.id)).or_default().clone())
            .get_or_init(|| {
                (|| -> Result<Option<Arc<Mask>>> {
                    let (t, _) = self
                        .source(url, |_| Ok(0))?
                        .with_context(|| format!("{}: its fill mask {url} isn't on the server", self.id))?;
                    Ok(Some(Arc::new(Mask::of(t).with_context(|| url.clone())?)))
                })()
                .map_err(|e| format!("{e:#}"))
            })
            .clone()
            .map_err(anyhow::Error::msg)
    }

    /// Whether file `k` has voids by its fill mask anywhere (a mask with none counts as none).
    fn has_voids(&self, k: usize) -> Result<bool> {
        Ok(self.mask(k)?.is_some_and(|m| m.voids.iter().any(Option::is_some)))
    }

    /// Whether native point (x, y) is a void by file `k`'s fill mask (its nearest pixel there
    /// `MASK_FILLED` or more); false without a mask, outside it, or in a block of it with none.
    fn masked(&self, k: usize, x: f64, y: f64) -> Result<bool> {
        Ok(self.mask(k)?.is_some_and(|m| m.masked(x, y)))
    }

    fn open(&self, k: usize) -> Result<Option<(Arc<Tiff>, usize)>> {
        if let Some(t) = self.opened.lock().unwrap().get(&k) {
            return Ok(t.clone());
        }
        let url = &self.files[k].0;
        let t = match self.source(url, |t| level_for(t, self.crs, self.read_resolution))? {
            None => bail!("{}: {url}, which its index lists, isn't on the server (the index is out of date?)", self.id),
            Some((t, level)) => {
                let id = FILE_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some((Arc::new(t.with_shared_cache(id, self.cache.clone())), level))
            }
        };
        self.opened.lock().unwrap().insert(k, t.clone());
        Ok(t)
    }

    /// The files whose footprint holds native point (x, y), in order.
    fn holding(&self, x: f64, y: f64) -> impl Iterator<Item = usize> + '_ {
        // (Among the grid's few where it has one, else every file.)
        let (near, all) = match &self.grid {
            Some(g) => (Some(g.near(x, y).iter().copied()), None),
            None => (None, Some(0..self.files.len())),
        };
        near.into_iter()
            .flatten()
            .chain(all.into_iter().flatten())
            .filter(move |&k| self.files[k].1.is_none_or(|b| in_box(&b, x, y)))
    }

    /// Points `sel` (native `x`, `y`) sampled: each one's value (NaN: none) and the file it came from;
    /// a fill mask's voids none unless `masks` is false (`Sampler::fill_voids`).
    fn sample(&self, sel: &[u32], x: &[f64], y: &[f64], masks: bool) -> Result<Vec<(f32, u32)>> {
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
            let Some((t, level)) = self.open(k)? else { continue };
            // (A file whose fill mask has voids: no block of it is known to hold data in every pixel.)
            let has_mask = masks && self.has_voids(k)?;
            let clean = (!has_mask).then(|| &self.clean[k]);
            let v = sample_raster_noting(&t, level, &xs, &ys, clean).with_context(|| self.files[k].0.clone())?;
            for (n, (&j, v)) in mine.iter().zip(v).enumerate() {
                if v.is_finite() && !(has_mask && self.masked(k, xs[n], ys[n])?) {
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

    /// The blocks sampling `parts` in turn reads first (each point's first file's: `blocks_read`),
    /// in that order, each once; and where each part's begin in that list (and its end).
    fn blocks_ahead(&self, parts: &[&[u32]], x: &[f64], y: &[f64]) -> Result<Ahead> {
        let mut seen: HashMap<usize, Seen> = HashMap::new();
        let (mut out, mut starts) = (Vec::new(), Vec::with_capacity(parts.len() + 1));
        for part in parts {
            starts.push(out.len());
            let mut by: std::collections::BTreeMap<usize, (Vec<f64>, Vec<f64>)> = Default::default();
            for &i in *part {
                let (px, py) = (x[i as usize], y[i as usize]);
                if let Some(k) = self.holding(px, py).next() {
                    let e = by.entry(k).or_default();
                    e.0.push(px);
                    e.1.push(py);
                }
            }
            for (k, (xs, ys)) in by {
                let Some((t, level)) = self.open(k)? else { continue };
                let mut b = Vec::new();
                blocks_read(&t, level, &xs, &ys, seen.entry(k).or_default(), &mut b)?;
                out.extend(b.into_iter().map(|(bx, by)| (t.clone(), level, bx, by)));
            }
        }
        starts.push(out.len());
        let biggest =
            out.iter().map(|(t, level, ..)| t.level(*level).map_or(0, |i| i.block_bytes())).max().unwrap_or(0);
        let n_ahead = ahead_for(self.cache.lock().unwrap().cap(), biggest);
        Ok((out, starts, n_ahead))
    }

    /// The value at native point (x, y) of the files but `skip`: the first holding it whose raster
    /// has it (its nearest pixel); None when none does.
    fn value_at(&self, x: f64, y: f64, skip: usize) -> Result<Option<f32>> {
        for k in self.holding(x, y).filter(|&k| k != skip) {
            let Some((t, level)) = self.open(k)? else { continue };
            let gt = t.transform(level)?;
            let img = t.level(level)?;
            let (c, r) = (((x - gt[0]) / gt[1]).floor(), ((y - gt[3]) / gt[5]).floor());
            if c >= 0.0 && r >= 0.0 && c < img.width as f64 && r < img.height as f64 {
                let v = t.pixel(level, c as u32, r as u32)?;
                return Ok(Some(if valid(v, t.nodata()) && !self.masked(k, x, y)? { v } else { f32::NAN }));
            }
        }
        Ok(None)
    }

    /// The distance (m) from each point (native `x`, `y`, latitude `lat`; its file `file`) to the
    /// nearest pixel where the dataset has no data, up to `radius` (m): `radius` when none is nearer.
    fn distance(
        &self,
        sel: &[u32],
        x: &[f64],
        y: &[f64],
        lat: &[f64],
        file: &[u32],
        radius: &[f64],
        step: Said,
    ) -> Result<Vec<f64>> {
        let tally = Tally::new(sel.len(), step);
        let mut out = vec![0.0; sel.len()];
        // Each point's band in its file's pixels; one inside blocks holding data in every pixel is
        // at the radius (nothing in it is invalid), the others grouped by file and by a square of
        // `DIST_CELL` pixels, so a group's points share one map of the invalid pixels around them.
        let mut groups: std::collections::BTreeMap<(u32, i64, i64), Vec<Band>> = Default::default();
        for j in 0..sel.len() {
            let i = sel[j] as usize;
            let k = file[j] as usize;
            let (t, level) = self.open(k)?.context("an open file")?;
            let gt = t.transform(level)?;
            let img = t.level(level)?;
            let (w, h) = (img.width as i64, img.height as i64);
            let (mx, my) = pixel_m(self.crs, &gt, lat[i]);
            let cf = (x[i] - gt[0]) / gt[1] - 0.5;
            let rf = (y[i] - gt[3]) / gt[5] - 0.5;
            let rx = (radius[j] / mx).ceil() as i64 + 1;
            let ry = (radius[j] / my).ceil() as i64 + 1;
            let (c0, r0) = (cf.floor() as i64, rf.floor() as i64);
            let win = [c0 - rx, c0 + 1 + rx, r0 - ry, r0 + 1 + ry];
            let [cs, ce, rs, re] = win;
            if !self.has_voids(k)?
                && cs >= 0
                && rs >= 0
                && ce < w
                && re < h
                && window_clean(&t, level, &self.clean[k], cs, ce, rs, re)?
            {
                out[j] = radius[j];
                continue;
            }
            let key = (file[j], c0.div_euclid(DIST_CELL), r0.div_euclid(DIST_CELL));
            groups.entry(key).or_default().push(Band { j, cf, rf, mx, my, win });
        }
        tally.add(sel.len() - groups.values().map(Vec::len).sum::<usize>());
        let groups: Vec<((u32, i64, i64), Vec<Band>)> = groups.into_iter().collect();
        let got: Vec<Vec<(usize, f64)>> = groups
            .par_iter()
            .map(|((k, ..), bands)| {
                let d = self.distance_group(*k as usize, bands, radius);
                tally.add(bands.len());
                d
            })
            .collect::<Result<_>>()?;
        for (j, d) in got.into_iter().flatten() {
            out[j] = d;
        }
        Ok(out)
    }

    /// The distances of `bands`' points (all of file `k`) to the nearest pixel where the dataset
    /// has no data, each up to its `radius`: by the map of the invalid pixels over their windows
    /// (inside the file from its raster, past its edges from the files holding them: `value_at`),
    /// by row each pixel's nearest invalid one at or left of it and at or right of it. The nearest
    /// invalid pixel of a row to a point is one of those two (the distance grows with the column's
    /// offset), so a point looks at two pixels a row of its window, not every one: what a scan of
    /// the whole window finds, to the bit.
    fn distance_group(&self, k: usize, bands: &[Band], radius: &[f64]) -> Result<Vec<(usize, f64)>> {
        let (t, level) = self.open(k)?.context("an open file")?;
        let gt = t.transform(level)?;
        let img = t.level(level)?;
        let (w, h) = (img.width as i64, img.height as i64);
        let x0 = bands.iter().map(|b| b.win[0]).min().unwrap();
        let x1 = bands.iter().map(|b| b.win[1]).max().unwrap();
        let y0 = bands.iter().map(|b| b.win[2]).min().unwrap();
        let y1 = bands.iter().map(|b| b.win[3]).max().unwrap();
        let (mw, mh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
        let (ics, ice, irs, ire) = (x0.max(0), x1.min(w - 1), y0.max(0), y1.min(h - 1));
        let inside = if ics <= ice && irs <= ire {
            Some(t.read_window(level, ics as u32, irs as u32, (ice - ics + 1) as u32, (ire - irs + 1) as u32)?)
        } else {
            None
        };
        let iw = (ice - ics + 1).max(0) as usize;
        let nodata = t.nodata();
        let mut ok = vec![false; mw * mh];
        for r in y0..=y1 {
            for c in x0..=x1 {
                ok[(r - y0) as usize * mw + (c - x0) as usize] = if c >= ics && c <= ice && r >= irs && r <= ire {
                    valid(inside.as_ref().unwrap()[(r - irs) as usize * iw + (c - ics) as usize], nodata) && {
                        let (wx, wy) = (gt[0] + (c as f64 + 0.5) * gt[1], gt[3] + (r as f64 + 0.5) * gt[5]);
                        !self.masked(k, wx, wy)?
                    }
                } else {
                    let (wx, wy) = (gt[0] + (c as f64 + 0.5) * gt[1], gt[3] + (r as f64 + 0.5) * gt[5]);
                    self.value_at(wx, wy, k)?.is_some_and(|v| v.is_finite())
                };
            }
        }
        let (left, right) = nearest_invalid(&ok, mw, mh);
        Ok(bands
            .iter()
            .map(|b| {
                let [cs, ce, rs, re] = b.win;
                let at = (b.cf.floor() as i64 - x0) as usize;
                let mut best = radius[b.j];
                for r in rs..=re {
                    let o = (r - y0) as usize * mw;
                    let dy = ((r as f64 - b.rf).abs() - 0.5).max(0.0) * b.my;
                    let (l, rt) = (left[o + at], right[o + at + 1]);
                    for c in [(l != i32::MIN).then(|| x0 + l as i64), (rt != i32::MAX).then(|| x0 + rt as i64)]
                        .into_iter()
                        .flatten()
                    {
                        if c < cs || c > ce {
                            continue;
                        }
                        let dx = ((c as f64 - b.cf).abs() - 0.5).max(0.0) * b.mx;
                        let d = dx.hypot(dy);
                        if d < best {
                            best = d;
                        }
                    }
                }
                (b.j, best)
            })
            .collect())
    }
}

/// The side (pixels) of the squares a GeoTIFF dataset's distance measures group points by: a
/// group's map of the invalid pixels spans its points' windows.
const DIST_CELL: i64 = 256;

/// A point's distance window in its file (`GeoSet::distance`): its place in the selection, its
/// pixel coordinates (from pixel centres), the pixels' size (m) and its window [cs, ce, rs, re].
struct Band {
    j: usize,
    cf: f64,
    rf: f64,
    mx: f64,
    my: f64,
    win: [i64; 4],
}

/// By pixel of a `w` × `h` map of which pixels hold data (`ok`): the column of its row's nearest
/// invalid pixel at or left of it (i32::MIN: none), and at or right of it (i32::MAX: none).
fn nearest_invalid(ok: &[bool], w: usize, h: usize) -> (Vec<i32>, Vec<i32>) {
    let mut left = vec![i32::MIN; w * h];
    let mut right = vec![i32::MAX; w * h];
    for row in 0..h {
        let (o, mut last) = (row * w, i32::MIN);
        for c in 0..w {
            if !ok[o + c] {
                last = c as i32;
            }
            left[o + c] = last;
        }
        last = i32::MAX;
        for c in (0..w).rev() {
            if !ok[o + c] {
                last = c as i32;
            }
            right[o + c] = last;
        }
    }
    (left, right)
}

/// A distance measure's points done, told to its `step` as a share each time another 1 % is
/// done (an atomic count: no lock a point).
struct Tally<'a> {
    done: std::sync::atomic::AtomicUsize,
    total: usize,
    every: usize,
    step: Said<'a>,
}

/// How many points of a tile are sampled between counts told to a `Tally`.
const TALLY_RUN: usize = 1024;

impl<'a> Tally<'a> {
    fn new(total: usize, step: Said<'a>) -> Tally<'a> {
        Tally { done: Default::default(), total, every: (total / 100).max(1), step }
    }

    fn add(&self, k: usize) {
        let before = self.done.fetch_add(k, std::sync::atomic::Ordering::Relaxed);
        if (before + k) / self.every > before / self.every {
            (self.step)((before + k) as f64 / self.total as f64);
        }
    }
}

/// The tile pyramids' files fetched in a run (their bytes as fetched, or that the server has none),
/// kept up to a size, the oldest let go first: each is fetched once, however many stages read it
/// (its sampling, the distances of its own and its neighbours' points, the blends' second
/// heights). A tile asked for while it's being fetched waits for that fetch, not another.
pub(crate) struct TileMemo {
    cap: usize,
    inner: Mutex<MemoIn>,
    /// The fill masks read, by dataset and URL: each read once however many samplers share the
    /// memo (a pole's levels, a base's), its bits held once (at most 2 MiB for a GLO-30 mask:
    /// 16 blocks of 1024², a bit a pixel, only blocks holding a void).
    masks: Mutex<HashMap<String, Arc<MaskCell>>>,
}

/// A fill mask, read once (None: it has none; Err: why it couldn't be).
type MaskCell = std::sync::OnceLock<Result<Option<Arc<Mask>>, String>>;

/// A memo's entry: None while it's being fetched (or its fetch failed), then the file or None.
type Slot = Arc<Mutex<Option<Option<Arc<[u8]>>>>>;

#[derive(Default)]
struct MemoIn {
    map: HashMap<String, Slot>,
    order: std::collections::VecDeque<(String, Slot, usize)>,
    bytes: usize,
}

/// What a run's tile memo keeps at most.
pub const TILE_MEMO_BYTES: usize = 512 << 20;

/// How many tiles are fetched at once ahead of their sampling (natively, whatever the threads
/// sampling: task #665, where 3 threads fetching a tile each at a time held GSI's to 2 a second): kept in
/// flight all along (`TileSet::rolling`), a slow tile holding up its own points alone.
pub const TILE_FETCHES: usize = 64;

/// A tile request's limits (crate::fetch::Fetch::get_within): its answer's head within 30 s (a
/// connection silent that long is dead, not slow: it fails and is asked again, where it held its
/// fetch for the client's 90 s a try), then its whole body within 64 s (512 KB's time at
/// 8 KB/s: a pyramid's tiles are tens to a couple of hundred KB, so even 64 at once on a link of
/// 512 KB/s come in time). Not a cap on each request's whole time; a host whose every failure was
/// a body past its time isn't noted down, one whose head never came is. (The budget proxy's idle
/// end, crate::budget::proxy::Limits, 600 s, is left as it is: its long query tunnels need it,
/// and the client's limits end a tile's tunnel first.)
pub const TILE_WITHIN: crate::fetch::Within =
    crate::fetch::Within { head: std::time::Duration::from_secs(30), floor: 8 << 10, most: 512 << 10 };

impl TileMemo {
    pub(crate) fn new(cap: usize) -> TileMemo {
        TileMemo { cap, inner: Mutex::new(MemoIn::default()), masks: Default::default() }
    }

    /// `url`'s file, fetched by `fetch` unless kept.
    fn get(&self, url: &str, fetch: &dyn Fetch) -> Result<Option<Arc<[u8]>>> {
        let slot = self.inner.lock().unwrap().map.entry(url.to_string()).or_default().clone();
        // (A slot a panicking fetch left poisoned holds None still: fetched again.)
        let mut got = slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = &*got {
            return Ok(v.clone());
        }
        let v: Option<Arc<[u8]>> = match fetch.get_within(url, TILE_WITHIN) {
            Ok(v) => v.map(Into::into),
            Err(e) => {
                // (Its empty slot dropped, unless another took its place, or another thread is
                // fetching into it now: try_lock, never waiting on a slot under the memo's lock.)
                drop(got);
                let mut m = self.inner.lock().unwrap();
                if m.map.get(url).is_some_and(|k| Arc::ptr_eq(k, &slot) && k.try_lock().is_ok_and(|g| g.is_none())) {
                    m.map.remove(url);
                }
                return Err(e);
            }
        };
        *got = Some(v.clone());
        drop(got);
        let size = v.as_ref().map_or(0, |b| b.len()) + url.len() + 64;
        let mut m = self.inner.lock().unwrap();
        m.order.push_back((url.to_string(), slot, size));
        m.bytes += size;
        while m.bytes > self.cap {
            let Some((u, s, b)) = m.order.pop_front() else { break };
            m.bytes -= b;
            if m.map.get(&u).is_some_and(|k| Arc::ptr_eq(k, &s)) {
                m.map.remove(&u);
            }
        }
        Ok(v)
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
    /// The run's fetched tiles (every pyramid's).
    memo: Arc<TileMemo>,
}

impl TileSet<'_> {
    fn tile_url(&self, tx: i64, ty: i64) -> String {
        self.url.replace("{z}", &self.zoom.to_string()).replace("{x}", &tx.to_string()).replace("{y}", &ty.to_string())
    }

    /// Items `need` (each the tiles it reads) worked by `work`, each as soon as its tiles are in
    /// the memo: `TILE_FETCHES` fetches kept in flight all along, in the items' order, so the
    /// network's waits overlap and one slow tile holds up the items that read it alone, not a
    /// batch. The tiles fetched and not yet done with (an item to work still reads them) are held
    /// to a quarter of the memo, so none is let go before its items are worked; the earliest item
    /// not yet worked always has its tiles fetched, whatever that holds. The first failed fetch
    /// stops the fetching (its requests' retries too: crate::fetch::cancellable) and is the error.
    /// (WebAssembly, or a single tile: none ahead; the work fetches them.)
    fn rolling<T: Send>(&self, need: &[Vec<(i64, i64)>], work: impl Fn(usize) -> Result<T> + Sync) -> Result<Vec<T>> {
        let n_items = need.len();
        if cfg!(target_arch = "wasm32") || need.iter().map(Vec::len).sum::<usize>() < 2 {
            return (0..n_items).into_par_iter().map(&work).collect();
        }
        // (The tiles in the order the items first read them; by item its tiles' places in that
        // list, and the last of them; by tile, the items reading it.)
        let mut place: HashMap<(i64, i64), usize> = HashMap::new();
        let mut tiles: Vec<(i64, i64)> = Vec::new();
        let mut readers: Vec<Vec<usize>> = Vec::new();
        let mut reads: Vec<Vec<usize>> = Vec::with_capacity(n_items);
        for (i, keys) in need.iter().enumerate() {
            let mut mine = Vec::with_capacity(keys.len());
            for k in keys {
                let p = *place.entry(*k).or_insert_with(|| {
                    tiles.push(*k);
                    readers.push(Vec::new());
                    tiles.len() - 1
                });
                if readers[p].last() != Some(&i) {
                    readers[p].push(i);
                    mine.push(p);
                }
            }
            reads.push(mine);
        }
        let last: Vec<usize> = reads.iter().map(|r| r.iter().copied().max().unwrap_or(0)).collect();
        // (A quarter of the memo, less the decoded tiles the working threads hold at once.)
        let decoded = TILE_PX * TILE_PX * std::mem::size_of::<f32>();
        let workers = rayon::current_num_threads();
        let hold = (self.memo.cap / 4).saturating_sub(workers * decoded);
        /// The fetching's and the working's shared state.
        struct St {
            /// The next tile to fetch.
            next: usize,
            /// By item, its tiles not yet in.
            missing: Vec<usize>,
            /// Items whose tiles are all in, not yet taken to work.
            ready: Vec<usize>,
            /// By tile, its items not yet worked, and its bytes.
            users: Vec<usize>,
            size: Vec<usize>,
            /// The bytes of the tiles fetched whose items aren't all worked.
            held: usize,
            worked: Vec<bool>,
            /// The earliest item not yet worked.
            first: usize,
            failed: Option<anyhow::Error>,
            stop: bool,
        }
        let st = Mutex::new(St {
            next: 0,
            missing: reads.iter().map(Vec::len).collect(),
            ready: (0..n_items).filter(|&i| reads[i].is_empty()).collect(),
            users: readers.iter().map(Vec::len).collect(),
            size: vec![0; tiles.len()],
            held: 0,
            worked: vec![false; n_items],
            first: 0,
            failed: None,
            stop: false,
        });
        let cv = Condvar::new();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let lock = || st.lock().unwrap_or_else(|e| e.into_inner());
        let stop = || {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            lock().stop = true;
            cv.notify_all();
        };
        // (Stopped however the working ends, a panic too, so the fetching threads end.)
        struct Stop<F: Fn()>(F);
        impl<F: Fn()> Drop for Stop<F> {
            fn drop(&mut self) {
                (self.0)()
            }
        }
        std::thread::scope(|s| {
            for _ in 0..TILE_FETCHES.min(tiles.len()) {
                s.spawn(|| loop {
                    let p = {
                        let mut g = lock();
                        // (Past the hold, only the earliest unworked item's tiles.)
                        while !g.stop
                            && g.next < tiles.len()
                            && g.held >= hold
                            && g.next > last[g.first.min(n_items - 1)]
                        {
                            g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                        if g.stop || g.next >= tiles.len() {
                            return;
                        }
                        g.next += 1;
                        g.next - 1
                    };
                    let (tx, ty) = tiles[p];
                    let url = self.tile_url(tx, ty);
                    // (A fetch that panics is a failed fetch: the working, waiting on its tile,
                    // would otherwise wait on forever.)
                    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        crate::fetch::cancellable(&cancel, || self.memo.get(&url, self.o.fetch))
                    }))
                    .unwrap_or_else(|p| {
                        let why = p
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| p.downcast_ref::<String>().cloned())
                            .unwrap_or_default();
                        Err(anyhow::anyhow!("{url}: its fetch panicked: {why}"))
                    });
                    let mut g = lock();
                    match got {
                        Ok(v) => {
                            let b = v.map_or(0, |v| v.len());
                            g.size[p] = b;
                            g.held += b;
                            for &i in &readers[p] {
                                g.missing[i] -= 1;
                                if g.missing[i] == 0 {
                                    g.ready.push(i);
                                }
                            }
                        }
                        Err(e) => {
                            if !g.stop {
                                g.failed.get_or_insert(e);
                            }
                            g.stop = true;
                            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    cv.notify_all();
                });
            }
            let _stop = Stop(&stop);
            let mut out: Vec<Option<T>> = (0..n_items).map(|_| None).collect();
            let mut done = 0;
            while done < n_items {
                let batch = {
                    let mut g = lock();
                    while g.ready.is_empty() && g.failed.is_none() {
                        g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                    }
                    if let Some(e) = g.failed.take() {
                        return Err(e);
                    }
                    std::mem::take(&mut g.ready)
                };
                let got: Vec<(usize, T)> = batch.par_iter().map(|&i| work(i).map(|v| (i, v))).collect::<Result<_>>()?;
                let mut g = lock();
                for (i, v) in got {
                    out[i] = Some(v);
                    done += 1;
                    g.worked[i] = true;
                    for &p in &reads[i] {
                        g.users[p] -= 1;
                        if g.users[p] == 0 {
                            g.held -= g.size[p];
                        }
                    }
                }
                while g.first < n_items && g.worked[g.first] {
                    g.first += 1;
                }
                cv.notify_all();
            }
            Ok(out.into_iter().map(|v| v.expect("every item worked")).collect())
        })
    }

    fn tile(&self, tx: i64, ty: i64) -> Result<Option<(usize, usize, Vec<f32>)>> {
        let u = self.tile_url(tx, ty);
        let Some(png) = self.memo.get(&u, self.o.fetch)? else { return Ok(None) };
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

    /// Points `sel` sampled, each tile on its own (its edges clamping), as its tile arrives
    /// (`rolling`); `step` told the share of the points done, about every 1 %. The points are
    /// taken by runs of `sel` in one tile (one run a tile, `sel` in reading order), each run's
    /// tile coordinates worked out as it's sampled: what's kept for the whole selection is a run's
    /// bounds and its heights, 4 bytes a point, not each point's place in its tile.
    fn sample(&self, sel: &[u32], lon: &[f64], lat: &[f64], step: Said) -> Result<Vec<f32>> {
        let n = 1i64 << self.zoom;
        let at = |i: u32| mercator_tile(lon[i as usize], lat[i as usize], self.zoom);
        let key = |i: u32| {
            let (fx, fy) = at(i);
            (fx.floor() as i64, fy.floor() as i64)
        };
        // (Runs: the first point of each, and its tile.)
        let mut runs: Vec<(usize, (i64, i64))> = Vec::new();
        for (j, &i) in sel.iter().enumerate() {
            let k = key(i);
            if runs.last().is_none_or(|r| r.1 != k) {
                runs.push((j, k));
            }
        }
        let end = |r: usize| runs.get(r + 1).map_or(sel.len(), |x| x.0);
        let tally = Tally::new(sel.len(), step);
        let need: Vec<Vec<(i64, i64)>> = runs.iter().map(|r| vec![r.1]).collect();
        let sampled = self.rolling(&need, |r| -> Result<Vec<f32>> {
            let (s, e, (tx, ty)) = (runs[r].0, end(r), runs[r].1);
            debug_assert!(tx < n && ty < n);
            let Some((w, h, a)) = self.tile_noted(tx, ty)? else {
                tally.add(e - s);
                return Ok(vec![f32::NAN; e - s]);
            };
            // (Told by runs of points: a tile may hold most of a unit's.)
            let mut v = Vec::with_capacity(e - s);
            for part in sel[s..e].chunks(TALLY_RUN) {
                v.extend(part.iter().map(|&i| {
                    let (fx, fy) = at(i);
                    bilinear(&a, w, h, (fx - tx as f64) * 256.0 - 0.5, (fy - ty as f64) * 256.0 - 0.5)
                }));
                tally.add(part.len());
            }
            Ok(v)
        })?;
        Ok(sampled.concat())
    }

    /// The tiles `distance` reads for `groups`' points: each band's that isn't all in tiles known
    /// clean, within the world, those not known clean (what its loop fetches, or more: a tile
    /// another group finds clean meanwhile).
    fn band_tiles(
        &self,
        groups: &[Vec<(i64, usize, f64, f64)>],
        sel: &[u32],
        lat: &[f64],
        radius: &[f64],
    ) -> Vec<(i64, i64)> {
        let n = 1i64 << self.zoom;
        let clean = self.clean.lock().unwrap();
        let mut keys = std::collections::BTreeSet::new();
        for g in groups {
            for &(_, j, fx, fy) in g {
                let pm = tile_resolution(self.zoom, lat[sel[j] as usize]);
                let (gx, gy) = (fx * 256.0 - 0.5, fy * 256.0 - 0.5);
                let rp = (radius[j] / pm).ceil() as i64 + 1;
                let (c0, r0) = (gx.floor() as i64, gy.floor() as i64);
                let (cs, ce, rs, re) = (c0 - rp, c0 + 1 + rp, r0 - rp, r0 + 1 + rp);
                let mut here = Vec::new();
                let mut all_clean = true;
                for ty in rs.div_euclid(256)..=re.div_euclid(256) {
                    for tx in cs.div_euclid(256)..=ce.div_euclid(256) {
                        if tx < 0 || ty < 0 || tx >= n || ty >= n {
                            all_clean = false;
                        } else if clean.get(&(tx, ty)) != Some(&true) {
                            all_clean = false;
                            here.push((tx, ty));
                        }
                    }
                }
                if !all_clean {
                    keys.extend(here);
                }
            }
        }
        keys.into_iter().collect()
    }

    /// As GeoSet::distance, on the pyramid's pixels (each tile's neighbours read where a band
    /// crosses into them).
    ///
    /// A tile's points share one map of the invalid pixels around it: by row, each pixel's nearest
    /// invalid one at or left of it and at or right of it. The nearest invalid pixel of a row to a
    /// point is one of those two (the distance grows with the column's offset), so a point looks at
    /// two pixels a row of its band, not every one: what a scan of the whole band finds, to the bit.
    fn distance(&self, sel: &[u32], lon: &[f64], lat: &[f64], radius: &[f64], step: Said) -> Result<Vec<f64>> {
        let n = 1i64 << self.zoom;
        let groups = self.groups(sel, lon, lat);
        let tally = Tally::new(sel.len(), step);
        // (Each group measured once the tiles its bands reach are in: `rolling`.)
        let need: Vec<Vec<(i64, i64)>> =
            groups.iter().map(|g| self.band_tiles(std::slice::from_ref(g), sel, lat, radius)).collect();
        let got: Vec<Vec<(usize, f64)>> = self.rolling(&need, |k| -> Result<Vec<(usize, f64)>> {
            let g = &groups[k];
            let clean_tile = |tx: i64, ty: i64| self.clean.lock().unwrap().get(&(tx, ty)).copied();
            let mut out = Vec::with_capacity(g.len());
            // (Each point's band in the pyramid's pixels: (j, gx, gy, metres a pixel, c0,
            // [cs, ce] × [rs, re]), for those whose band isn't all in clean tiles.)
            let mut bands = Vec::new();
            for &(_, j, fx, fy) in g {
                let i = sel[j] as usize;
                let pm = tile_resolution(self.zoom, lat[i]);
                let (gx, gy) = (fx * 256.0 - 0.5, fy * 256.0 - 0.5);
                let rp = (radius[j] / pm).ceil() as i64 + 1;
                let (c0, r0) = (gx.floor() as i64, gy.floor() as i64);
                // (The band's tiles: one known clean has no invalid pixel, so none of its
                // pixels is looked at, nor is it fetched again.)
                let (cs, ce, rs, re) = (c0 - rp, c0 + 1 + rp, r0 - rp, r0 + 1 + rp);
                let all_clean = (rs.div_euclid(256)..=re.div_euclid(256)).all(|ty| {
                    (cs.div_euclid(256)..=ce.div_euclid(256))
                        .all(|tx| tx >= 0 && ty >= 0 && tx < n && ty < n && clean_tile(tx, ty) == Some(true))
                });
                if all_clean {
                    out.push((j, radius[j]));
                } else {
                    bands.push((j, gx, gy, pm, c0, [cs, ce, rs, re]));
                }
            }
            if bands.is_empty() {
                tally.add(g.len());
                return Ok(out);
            }
            // The pixels the bands cover, [x0, x1] × [y0, y1]: which hold data.
            let x0 = bands.iter().map(|b| b.5[0]).min().unwrap();
            let x1 = bands.iter().map(|b| b.5[1]).max().unwrap();
            let y0 = bands.iter().map(|b| b.5[2]).min().unwrap();
            let y1 = bands.iter().map(|b| b.5[3]).max().unwrap();
            let (w, h) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
            let mut ok = vec![false; w * h];
            for ty in y0.div_euclid(256)..=y1.div_euclid(256) {
                for tx in x0.div_euclid(256)..=x1.div_euclid(256) {
                    // (Outside the world: no data. A tile no band reaches isn't fetched.)
                    if tx < 0 || ty < 0 || tx >= n || ty >= n {
                        continue;
                    }
                    let (px0, px1, py0, py1) = (tx * 256, tx * 256 + 255, ty * 256, ty * 256 + 255);
                    if !bands.iter().any(|b| b.5[0] <= px1 && b.5[1] >= px0 && b.5[2] <= py1 && b.5[3] >= py0) {
                        continue;
                    }
                    let tile = if clean_tile(tx, ty) == Some(true) { None } else { Some(self.tile_noted(tx, ty)?) };
                    for r in py0.max(y0)..=py1.min(y1) {
                        let row = (r - y0) as usize * w;
                        for c in px0.max(x0)..=px1.min(x1) {
                            ok[row + (c - x0) as usize] = match &tile {
                                None => true,
                                Some(None) => false,
                                Some(Some((_, _, a))) => {
                                    valid(a[(r - py0) as usize * TILE_PX + (c - px0) as usize], None)
                                }
                            };
                        }
                    }
                }
            }
            let (left, right) = nearest_invalid(&ok, w, h);
            for (j, gx, gy, pm, c0, [cs, ce, rs, re]) in bands {
                let mut best = radius[j];
                let at = (c0 - x0) as usize;
                for r in rs..=re {
                    let o = (r - y0) as usize * w;
                    let dy = ((r as f64 - gy).abs() - 0.5).max(0.0);
                    let (l, rt) = (left[o + at], right[o + at + 1]);
                    for c in [(l != i32::MIN).then(|| x0 + l as i64), (rt != i32::MAX).then(|| x0 + rt as i64)]
                        .into_iter()
                        .flatten()
                    {
                        if c < cs || c > ce {
                            continue;
                        }
                        let d = ((c as f64 - gx).abs() - 0.5).max(0.0).hypot(dy) * pm;
                        if d < best {
                            best = d;
                        }
                    }
                }
                out.push((j, best));
            }
            tally.add(g.len());
            Ok(out)
        })?;
        let mut out = vec![0.0; sel.len()];
        for (j, d) in got.into_iter().flatten() {
            out[j] = d;
        }
        Ok(out)
    }
}

/// A dataset as a run reads it, kept across its batches of points (`Sampler`): its open files
/// and the blocks or tiles known to be whole.
enum Set<'a> {
    Geo(GeoSet<'a>),
    Tiles(TileSet<'a>),
}

impl<'a> Set<'a> {
    fn new(ds: &'a Dataset, o: &'a Opts<'a>, cache: &Arc<Mutex<BlockCache>>, memo: &Arc<TileMemo>) -> Result<Set<'a>> {
        Ok(match &ds.source {
            Source::Tiles { url, zoom, encoding } => Set::Tiles(TileSet {
                url,
                zoom: *zoom,
                encoding,
                o,
                clean: Mutex::new(HashMap::new()),
                memo: memo.clone(),
            }),
            Source::Geotiff { crs, read_resolution, store, files, masks } => {
                ensure!(super::declared::known_crs(*crs), "{}: EPSG:{crs} isn't a CRS the app knows", ds.id);
                let store = match (*store, &o.store) {
                    (false, _) => None,
                    (true, Some(s)) => Some(s.join(&ds.id)),
                    (true, None) => {
                        bail!("{}: its files are read from the store, and no store is given (SCENIC_STORE)", ds.id)
                    }
                };
                Set::Geo(GeoSet {
                    id: &ds.id,
                    crs: *crs,
                    files,
                    masks,
                    read_resolution: *read_resolution,
                    store,
                    o,
                    opened: Mutex::new(HashMap::new()),
                    opened_masks: (0..files.len()).map(|_| std::sync::OnceLock::new()).collect(),
                    memo: memo.clone(),
                    grid: (files.len() > FileGrid::FROM).then(|| FileGrid::of(files)),
                    clean: (0..files.len()).map(|_| Clean::default()).collect(),
                    cache: cache.clone(),
                })
            }
        })
    }

    /// The set reading points `lon`, `lat` (one batch's): a GeoTIFF dataset in a CRS but the
    /// points' own with their native coordinates where `cover` holds them (NaN elsewhere), made
    /// straight into their two columns (no list of pairs beside them).
    fn reader<'s>(&'s self, lon: &[f64], lat: &[f64], cover: &Cover) -> Reader<'s, 'a> {
        match self {
            Set::Tiles(t) => Reader::Tiles(t),
            Set::Geo(g) if projected(g.crs) => {
                let atlas = Projection::of(g.crs).expect("a projected CRS");
                let xy = lon
                    .par_iter()
                    .zip(lat.par_iter())
                    .map(|(&lo, &la)| if cover.contains(lo, la) { atlas.project(lo, la) } else { (f64::NAN, f64::NAN) })
                    .unzip();
                Reader::Geo(g, Some(xy))
            }
            Set::Geo(g) => Reader::Geo(g, None),
        }
    }
}

/// `sel` (ascending) ordered by `key` (each point's, `same` its equality), ties by index: the same
/// order as sorting every point, but sorting only the runs of neighbours sharing a key (a sample
/// row's points in one file or tile), a few thousand where the points are hundreds of thousands. A
/// run's indices ascend and two runs with one key don't overlap, so ordering runs by key then first
/// index orders the points.
fn by_runs<K: Sync>(
    sel: &[u32],
    key: &[K],
    same: impl Fn(&K, &K) -> bool,
    cmp: impl Fn(&K, &K) -> std::cmp::Ordering + Sync,
) -> Vec<u32> {
    debug_assert!(sel.windows(2).all(|w| w[0] < w[1]));
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for j in 0..sel.len() {
        match runs.last_mut() {
            Some(r) if same(&key[r.0], &key[j]) => r.1 = j + 1,
            _ => runs.push((j, j + 1)),
        }
    }
    runs.par_sort_unstable_by(|a, b| cmp(&key[a.0], &key[b.0]).then(a.0.cmp(&b.0)));
    runs.into_iter().flat_map(|(a, b)| sel[a..b].iter().copied()).collect()
}

/// A dataset reading one batch of points.
enum Reader<'s, 'a> {
    /// A GeoTIFF dataset, with its points' native coordinates where its CRS isn't the points'
    /// own (EPSG:4326 and 4269 read `lon` and `lat` as given: no copy of them, 16 bytes a point, for
    /// each such dataset).
    Geo(&'s GeoSet<'a>, Option<(Vec<f64>, Vec<f64>)>),
    Tiles(&'s TileSet<'a>),
}

impl Reader<'_, '_> {
    /// `sel` in reading order: a tile's points together (tile pyramids), a file's together and by
    /// row (GeoTIFFs). Each point is sampled alone, so the values are the same in any order.
    fn in_reading_order(&self, sel: Vec<u32>, lon: &[f64], lat: &[f64]) -> Vec<u32> {
        match self {
            Reader::Tiles(t) => {
                let n = 1i64 << t.zoom;
                let keys: Vec<i64> = sel
                    .par_iter()
                    .map(|&i| {
                        let (fx, fy) = mercator_tile(lon[i as usize], lat[i as usize], t.zoom);
                        fx.floor() as i64 * n + fy.floor() as i64
                    })
                    .collect();
                by_runs(&sel, &keys, |a, b| a == b, |a, b| a.cmp(b))
            }
            Reader::Geo(g, xy) => {
                let (x, y) = native(xy, lon, lat);
                let keys: Vec<(usize, f64)> = sel
                    .par_iter()
                    .map(|&i| (g.holding(x[i as usize], y[i as usize]).next().unwrap_or(usize::MAX), y[i as usize]))
                    .collect();
                by_runs(
                    &sel,
                    &keys,
                    |a, b| a.0 == b.0 && a.1.total_cmp(&b.1).is_eq(),
                    |a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)),
                )
            }
        }
    }

    /// What to read ahead of sampling `parts` in turn (`read_ahead`): a GeoTIFF dataset's blocks,
    /// natively, where they're fetched (not the store's: those are read on the NAS as sampled); None
    /// otherwise (a pyramid fetches its tiles ahead itself).
    fn ahead(&self, parts: &[&[u32]], lon: &[f64], lat: &[f64]) -> Result<Option<Ahead>> {
        match self {
            Reader::Geo(g, xy) if g.store.is_none() && !cfg!(target_arch = "wasm32") => {
                let (x, y) = native(xy, lon, lat);
                Ok(Some(g.blocks_ahead(parts, x, y)?))
            }
            _ => Ok(None),
        }
    }

    /// Points `sel` sampled: each one's value and (GeoTIFFs) its file. A pyramid tells `step` the
    /// share of them done as it goes (a GeoTIFF dataset's chunks are said by its caller).
    fn sample(&self, sel: &[u32], lon: &[f64], lat: &[f64], step: Said) -> Result<Vec<(f32, u32)>> {
        match self {
            Reader::Geo(g, xy) => {
                let (x, y) = native(xy, lon, lat);
                g.sample(sel, x, y, true)
            }
            Reader::Tiles(t) => Ok(t.sample(sel, lon, lat, step)?.into_iter().map(|v| (v, 0)).collect()),
        }
    }

    /// The points a chunk of `n` to sample holds: a pyramid's all of them (its fetches roll on
    /// through them, `TileSet::rolling`, and it says its progress itself); a GeoTIFF dataset's a
    /// hundredth, within `CHUNK_MIN` and `CHUNK_MAX`.
    fn chunk(&self, n: usize) -> usize {
        match self {
            Reader::Geo(..) => (n / 100).clamp(CHUNK_MIN, CHUNK_MAX),
            Reader::Tiles(_) => n.max(1),
        }
    }

    /// As GeoSet::distance; `step` told the share of the points done, about every 1 %.
    fn distance(
        &self,
        sel: &[u32],
        lon: &[f64],
        lat: &[f64],
        file: &[u32],
        radius: &[f64],
        step: Said,
    ) -> Result<Vec<f64>> {
        match self {
            Reader::Geo(g, xy) => {
                let (x, y) = native(xy, lon, lat);
                g.distance(sel, x, y, lat, file, radius, step)
            }
            Reader::Tiles(t) => t.distance(sel, lon, lat, radius, step),
        }
    }
}

/// A GeoTIFF dataset's points in its CRS: its own columns, else the points' `lon` and `lat`.
fn native<'b>(xy: &'b Option<(Vec<f64>, Vec<f64>)>, lon: &'b [f64], lat: &'b [f64]) -> (&'b [f64], &'b [f64]) {
    match xy {
        Some((x, y)) => (x, y),
        None => (lon, lat),
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

/// Points one chunk of a dataset's sampling holds at most (a progress line each), and at least.
const CHUNK_MAX: usize = 1 << 20;
const CHUNK_MIN: usize = 1 << 14;

/// A progress callback: how far, 0 to 1. Called from any thread (a rayon loop may call it), and by
/// `sample` only forwards, never backwards.
pub type Said<'a> = &'a (dyn Fn(f64) + Sync);

/// `Said` made monotonic and clamped to 0..=1.
struct Mono<'a> {
    last: Mutex<f64>,
    to: Said<'a>,
}

impl Mono<'_> {
    fn say(&self, f: f64) {
        let mut l = self.last.lock().unwrap();
        let f = f.clamp(0.0, 1.0);
        if f > *l {
            *l = f;
            (self.to)(f);
        }
    }
}

/// Points `lon`, `lat` (degrees) sampled from `plan` (`Sampler::sample`, in one batch).
pub fn sample(plan: &Plan, lon: &[f64], lat: &[f64], o: &Opts, said: Said) -> Result<Sampled> {
    Sampler::new(plan, o)?.sample(lon, lat, said)
}

/// A plan's datasets as a run reads them, over its batches of points (`run`): the decoded blocks,
/// the fetched tiles, the open files, the blocks and tiles known whole and the geoid grids kept
/// from one batch to the next, so a batch reads again only what the caches let go. Each point is
/// sampled alone, so its height is the same in any batch.
pub struct Sampler<'a> {
    plan: &'a Plan,
    covers: Vec<Cover>,
    sets: Vec<Set<'a>>,
    grids: super::geoid::Grids<'a>,
    memo: Arc<TileMemo>,
    cache: Arc<Mutex<BlockCache>>,
}

impl<'a> Sampler<'a> {
    pub fn new(plan: &'a Plan, o: &'a Opts<'a>) -> Result<Sampler<'a>> {
        Self::with_memo(
            plan,
            o,
            Arc::new(TileMemo::new(TILE_MEMO_BYTES)),
            Arc::new(Mutex::new(BlockCache::new(CACHE_BYTES))),
        )
    }

    /// `new`, sharing `other`'s fetched tiles and decoded blocks: a pyramid both read is fetched
    /// once, and both hold one cache's worth of blocks, not two.
    pub fn sharing(plan: &'a Plan, o: &'a Opts<'a>, other: &Sampler) -> Result<Sampler<'a>> {
        Self::with_memo(plan, o, other.memo.clone(), other.cache.clone())
    }

    fn with_memo(
        plan: &'a Plan,
        o: &'a Opts<'a>,
        memo: Arc<TileMemo>,
        cache: Arc<Mutex<BlockCache>>,
    ) -> Result<Sampler<'a>> {
        Ok(Sampler {
            plan,
            covers: plan.datasets.iter().map(Cover::of_dataset).collect(),
            sets: plan.datasets.iter().map(|d| Set::new(d, o, &cache, &memo)).collect::<Result<_>>()?,
            grids: super::geoid::Grids::new(o),
            memo,
            cache,
        })
    }

    /// Points `lon`, `lat` (degrees) sampled. `said`: how far it is, 0 to 1, never backwards,
    /// ending at 1: once per chunk of a dataset's points and per blend step (the blends' weight is
    /// their points to measure against the points to sample), not per point. The blend stage hands
    /// `step` (the share of its call done, 0 to 1) to a distance measure that reports.
    pub fn sample(&self, lon: &[f64], lat: &[f64], said: Said) -> Result<Sampled> {
        use crate::timings::{phase, Class};
        let (plan, covers) = (self.plan, &self.covers);
        let n = lon.len();
        let mono = Mono { last: Mutex::new(0.0), to: said };
        // (The work to say by: points to sample (those some dataset covers), and points to measure (the
        // first dataset covering one is followed by another).)
        let (to_sample, to_blend) = (0..n)
            .into_par_iter()
            .map(|i| {
                let mut c = (0..covers.len()).filter(|&k| covers[k].contains(lon[i], lat[i]));
                match (c.next(), c.next()) {
                    (None, _) => (0usize, 0usize),
                    (Some(_), None) => (1, 0),
                    (Some(_), Some(_)) => (1, 1),
                }
            })
            .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        // (The blends' points counted twice: their distances measured, then their second heights.)
        let total = ((to_sample + 2 * to_blend) as f64).max(1.0);
        let mut a = vec![f32::NAN; n];
        let mut first = vec![u16::MAX; n];
        let mut file = vec![u32::MAX; n];
        let mut readers = Vec::new();
        // (What each phase fetches over the network: its count.)
        let fetched =
            |p: &crate::timings::Phase, since: u64| p.count(crate::fetch::fetched_bytes().saturating_sub(since), 0);
        for (k, ds) in plan.datasets.iter().enumerate() {
            let p = phase(&format!("{} sampled", ds.id), Class::Net);
            let since = crate::fetch::fetched_bytes();
            let r = self.sets[k].reader(lon, lat, &covers[k]);
            let sel: Vec<u32> = (0..n as u32)
                .filter(|&i| first[i as usize] == u16::MAX && covers[k].contains(lon[i as usize], lat[i as usize]))
                .collect();
            // (By tile or file, so a chunk reads each tile once, not once per chunk.)
            let sel = r.in_reading_order(sel, lon, lat);
            if !sel.is_empty() {
                // (In chunks, a progress line after each: the datasets share the first part equally.)
                let parts: Vec<&[u32]> = sel.chunks(r.chunk(sel.len())).collect();
                let mut here = 0;
                let mut done = 0;
                read_ahead(r.ahead(&parts, lon, lat)?.as_ref(), |at| -> Result<()> {
                    for (j, part) in parts.iter().enumerate() {
                        at(j);
                        let step = |f: f64| {
                            let share = (k as f64 + (done as f64 + f * part.len() as f64) / sel.len() as f64)
                                / plan.datasets.len() as f64;
                            mono.say(share * to_sample as f64 / total);
                        };
                        let v = r.sample(part, lon, lat, &step)?;
                        for (&i, (v, f)) in part.iter().zip(v) {
                            if v.is_finite() {
                                (a[i as usize], first[i as usize], file[i as usize]) = (v, k as u16, f);
                                here += 1;
                            }
                        }
                        done += part.len();
                        let share = (k as f64 + done as f64 / sel.len() as f64) / plan.datasets.len() as f64;
                        mono.say(share * to_sample as f64 / total);
                    }
                    Ok(())
                })?;
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
        mono.say(to_sample as f64 / total);
        let mut blended = 0usize;
        for k in 0..plan.datasets.len() {
            let mut sel = Vec::new();
            let mut radius = Vec::new();
            for i in 0..n {
                if first[i] as usize != k {
                    continue;
                }
                let coarsest = (k + 1..plan.datasets.len())
                    .filter(|&m| covers[m].contains(lon[i], lat[i]))
                    .map(|m| res[m])
                    .fold(0.0f64, f64::max);
                if coarsest > 0.0 {
                    sel.push(i as u32);
                    radius.push(4.0 * coarsest.max(res[k]));
                }
            }
            if sel.is_empty() {
                continue;
            }
            let files: Vec<u32> = sel.iter().map(|&i| file[i as usize]).collect();
            // (What a distance measure calls with the share of its points done.)
            let step = |f: f64| mono.say((to_sample as f64 + blended as f64 + f * sel.len() as f64) / total);
            let d = readers[k].distance(&sel, lon, lat, &files, &radius, &step)?;
            for ((&i, d), r) in sel.iter().zip(d).zip(radius) {
                if d < r {
                    dist[i as usize] = d;
                }
            }
            step(1.0);
            blended += sel.len();
        }
        let mut b = vec![f32::NAN; n];
        let mut second = vec![u16::MAX; n];
        for (k, r) in readers.iter().enumerate() {
            let sel: Vec<u32> = (0..n as u32)
                .filter(|&i| {
                    dist[i as usize].is_finite()
                        && second[i as usize] == u16::MAX
                        && (first[i as usize] as usize) < k
                        && covers[k].contains(lon[i as usize], lat[i as usize])
                })
                .collect();
            if sel.is_empty() {
                continue;
            }
            // (In reading order and in chunks, a progress line after each: the datasets share the
            // last part equally.)
            let sel = r.in_reading_order(sel, lon, lat);
            let parts: Vec<&[u32]> = sel.chunks(r.chunk(sel.len())).collect();
            let mut done = 0;
            read_ahead(r.ahead(&parts, lon, lat)?.as_ref(), |at| -> Result<()> {
                for (j, part) in parts.iter().enumerate() {
                    at(j);
                    let step = |f: f64| {
                        let share = (k as f64 + (done as f64 + f * part.len() as f64) / sel.len() as f64)
                            / readers.len() as f64;
                        mono.say((to_sample as f64 + to_blend as f64 * (1.0 + share)) / total);
                    };
                    for (&i, (v, _)) in part.iter().zip(r.sample(part, lon, lat, &step)?) {
                        if v.is_finite() {
                            (b[i as usize], second[i as usize]) = (v, k as u16);
                        }
                    }
                    done += part.len();
                    let share = (k as f64 + done as f64 / sel.len() as f64) / readers.len() as f64;
                    mono.say((to_sample as f64 + to_blend as f64 * (1.0 + share)) / total);
                }
                Ok(())
            })?;
        }
        fetched(&p, since);
        drop(p);
        // Each dataset's heights brought to EGM2008 (crate::dem::geoid) before the blend: the first's
        // and the second's alike.
        let p = phase("the heights brought to EGM2008", Class::Net);
        let since = crate::fetch::fetched_bytes();
        let grids = &self.grids;
        let mut unconverted = 0usize;
        let mut b_kept = vec![false; n];
        let mut a_converted = vec![false; n];
        for (k, ds) in plan.datasets.iter().enumerate() {
            let Some(c) = &ds.convert else { continue };
            for (vals, from, firsts) in [(&mut a, &first, true), (&mut b, &second, false)] {
                let sel: Vec<usize> = (0..n).filter(|&i| from[i] as usize == k && vals[i].is_finite()).collect();
                if sel.is_empty() {
                    continue;
                }
                let (lo, la): (Vec<f64>, Vec<f64>) = sel.iter().map(|&i| (lon[i], lat[i])).unzip();
                let off = grids.offsets(c, &lo, &la)?;
                for (&i, d) in sel.iter().zip(off) {
                    if d.is_finite() {
                        vals[i] = (vals[i] as f64 + d) as f32;
                        if firsts {
                            a_converted[i] = true;
                        }
                    } else if firsts {
                        unconverted += 1;
                    } else {
                        b_kept[i] = true;
                    }
                }
            }
        }
        if unconverted > 0 {
            println!("  {unconverted} vertices' heights not converted: no declared geoid source has a value there");
        }
        fetched(&p, since);
        drop(p);
        mono.say(1.0);
        let mut out = Sampled { elev: a, src: vec![0; n], blended: vec![false; n], unconverted, unconverted_seam: 0 };
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
                    out.unconverted_seam += (a_converted[i] && b_kept[i]) as usize;
                }
            }
        }
        if out.unconverted_seam > 0 {
            println!(
            "  {} blended vertices mix references: the first converted, the one beneath without a geoid value there",
            out.unconverted_seam
        );
        }
        Ok(out)
    }
}

impl Sampler<'_> {
    /// Fills `got`'s points no dataset had data for (`sample`'s NaN) from the datasets' fill
    /// masks' voids: the first GeoTIFF dataset in the order holding the point gives its own filled
    /// value there (GLO-30's FLM 3 and up: filled from another DEM), brought to EGM2008 as
    /// `sample`'s are, with its code; no blend. For the polar caps alone (crate::polar), where no
    /// dataset lies beneath GLO-30 near the South Pole; the Mercator terrain and the roads read
    /// `sample` as it is. The points filled.
    pub fn fill_voids(&self, lon: &[f64], lat: &[f64], got: &mut Sampled) -> Result<usize> {
        let plan = self.plan;
        let mut filled = 0;
        for (k, ds) in plan.datasets.iter().enumerate() {
            let Set::Geo(_) = &self.sets[k] else { continue };
            let sel: Vec<u32> = (0..lon.len() as u32)
                .filter(|&i| got.elev[i as usize].is_nan() && self.covers[k].contains(lon[i as usize], lat[i as usize]))
                .collect();
            if sel.is_empty() {
                continue;
            }
            let r = self.sets[k].reader(lon, lat, &self.covers[k]);
            let Reader::Geo(g, xy) = &r else { continue };
            let (x, y) = native(xy, lon, lat);
            let sel = r.in_reading_order(sel, lon, lat);
            let v = g.sample(&sel, x, y, false)?;
            let mut now: Vec<(usize, f32)> =
                sel.iter().zip(v).filter(|(_, (v, _))| v.is_finite()).map(|(&i, (v, _))| (i as usize, v)).collect();
            if let Some(c) = &ds.convert {
                let (lo, la): (Vec<f64>, Vec<f64>) = now.iter().map(|&(i, _)| (lon[i], lat[i])).unzip();
                for ((_, v), d) in now.iter_mut().zip(self.grids.offsets(c, &lo, &la)?) {
                    if d.is_finite() {
                        *v = (*v as f64 + d) as f32;
                    }
                }
            }
            for (i, v) in now {
                got.elev[i] = v;
                got.src[i] = ds.code;
                filled += 1;
            }
        }
        Ok(filled)
    }
}

#[cfg(test)]
#[path = "generic_tests.rs"]
mod tests;
