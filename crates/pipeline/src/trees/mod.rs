//! The tree cover layers' tiles (docs/plan.md §6, Trees): tree cover, canopy height and leaf type,
//! zoom 4–12, of a z3 tile the coverage meets, clipped to it. dem/trees.py's `--z3` run in Rust (the
//! `trees` program), the same tiles to the pixel:
//! - **Sampling:** each zoom-8 block is sampled at zoom 12, at its pixels' centres (the source pixel
//!   they fall in), from Meta's canopy squares (`cover5m`, ‰ of the ground under trees over 5 m, and
//!   `p95`, canopy height in cm; 0.00025°, 10° squares) and the leaf-type squares (dem/leaftype.py;
//!   0.0005°), inside the coverage (`mask`).
//! - **Pyramid:** zoom 11 to 8 are float32 means of four pixels, summed as numpy sums them; leaf type
//!   goes as each class's share, a pixel showing the commonest type where forest is at least half
//!   of what's known (`pyramid`). Zoom 7 to 4 come from the blocks' zoom-8 values (`Tops`).
//! - **Tiles:** Terrarium-encoded lossless WebP (`crate::webp`): trees.py's pixels, not its bytes
//!   (another encoder).
//!
//! A block runs on its own (`block`, so a worker can be given one: tasks of it are planned): what it
//! reads comes from a `Source` (a folder, or URLs read through `crate::fetch`, whose mirror can hold
//! just the byte ranges the block reads); it gives its zoom 8–12 tiles and its zoom-8 values, from
//! which `assemble` makes the z3 tile's archives. `z3` does both, its blocks on rayon's threads (in
//! WebAssembly one after another), and the archives are the same bytes either way, and as
//! `assemble` writes them.
//!
//! The build makes the layers a z6 tile at a time (a piece, `z6`: its blocks' zoom 9–12 tiles, and
//! its mid, `MID`, their zoom-8 tiles and values), then each z3 tile's zoom 8 to 4 from its pieces'
//! mids (`assemble_lo`): together the same tiles, byte for byte, as `z3` makes.

pub mod mask;
pub mod pyramid;
pub mod squares;
#[cfg(test)]
mod tests;

use crate::fetch::{Fetch, Noted};
use anyhow::{bail, ensure, Context, Result};
use det::Det;
use pyramid::{Tile, Tops};
use roadcore::archive::{Archive, ArchiveWriter};
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::{PlainFile, RangeRead};

pub const ZMAX: u8 = 12;
pub const ZBLOCK: u8 = 8;
pub const ZMIN: u8 = 4;
pub const TS: usize = 256;
/// A block's side in zoom-12 pixels.
pub const BS: usize = TS << (ZMAX - ZBLOCK);
/// The layers, in trees.py's order: their archives are `trees-<layer>.tiles`.
pub const LAYERS: [&str; 3] = ["cover", "height", "leaf"];
/// The archives' metadata, trees.py's.
pub const META: &str = r#"{"source":"Meta/WRI canopy height; Copernicus HRL DLT 2018; NALCMS 2020","encoding":"terrarium","format":"webp"}"#;
/// Where Meta keeps the canopy squares.
pub const CHM10_URL: &str = "https://dataforgood-fb-data.s3.amazonaws.com/forests/v1/alsgedi_global_v6_float_epsg4326_v3_10deg";
/// A block's zoom-8 values beside its archives (`block`).
pub const TOPS: &str = "trees-tops.bin";
/// Web Mercator's sphere.
const R: f64 = 6378137.0;
/// The canopy squares' and the leaf-type squares' pixels (degrees).
const CHM_RES: f64 = 0.00025;
const LEAF_RES: f64 = 0.0005;
/// Decoded strips and tiles kept per square: a band's rows meet at their edges, a leaf-type tile
/// row serves several bands.
const CHM_CACHE: usize = 4 << 20;
const LEAF_CACHE: usize = 8 << 20;

/// A pixel column's longitude at zoom `z` (trees.py's `lon_of`).
fn lon_of(px: f64, z: u8) -> f64 {
    px / (TS << z) as f64 * 360.0 - 180.0
}

/// A pixel row's latitude at zoom `z` (trees.py's `lat_of`; det's sinh and atan where numpy's are
/// Apple's: a few ulp apart in some rows, never another canopy row, checked over all 2^20).
fn lat_of(py: f64, z: u8) -> f64 {
    (PI * (1.0 - 2.0 * py / (TS << z) as f64)).dsinh().datan().to_degrees()
}

/// Tile (`z`, `x`, `y`)'s box in degrees: west, south, east, north (trees.py's `tile_bounds`).
pub fn tile_bounds(z: u8, x: u32, y: u32) -> [f64; 4] {
    let (x, y) = (x as usize, y as usize);
    [lon_of((x * TS) as f64, z), lat_of(((y + 1) * TS) as f64, z), lon_of(((x + 1) * TS) as f64, z), lat_of((y * TS) as f64, z)]
}

/// Degrees to Web Mercator metres (trees.py's `merc`).
pub fn merc(lon: f64, lat: f64) -> [f64; 2] {
    [R * lon.to_radians(), R * (PI / 4.0 + lat.to_radians() / 2.0).dtan().dln()]
}

/// The canopy squares (top, left) a box (degrees) meets, top to bottom then west to east.
pub fn squares_of(b: [f64; 4]) -> Vec<(i32, i32)> {
    let [w, s, e, n] = b;
    let mut out = Vec::new();
    let (mut top, bottom) = ((n / 10.0).ceil() as i32 * 10, (s / 10.0).floor() as i32 * 10);
    while top > bottom {
        let (mut left, right) = ((w / 10.0).floor() as i32 * 10, (e / 10.0).ceil() as i32 * 10);
        while left < right {
            if top as f64 > s && ((top - 10) as f64) < n && (left as f64) < e && (left + 10) as f64 > w {
                out.push((top, left));
            }
            left += 10;
        }
        top -= 10;
    }
    out
}

/// A canopy square's file: `kind` is `cover5m` or `p95`.
pub fn chm_name(top: i32, left: i32, kind: &str) -> String {
    format!("meta_chm_lat={top}.0_lon={left}.0_{kind}.tif")
}

/// Where Meta keeps a canopy square's file (named as `chm_name` names it, which is how it's kept
/// here): its URL, and in the equator's row (top 0) its other spelling after it. Meta names most of
/// that row's files `lat=-0.0` (25 of its 29 squares; at lon −100, 0, 70 and 90 some or all of a
/// square's files are `lat=0.0`, the kinds of one square split between the two at −100 and 0), so a
/// file is asked for under one name, then the other: none there only when both say so. (An empty
/// file kept from before, Meta's "none" under the one name, still says none: docs/plan.md §10.)
pub fn chm_urls(name: &str) -> Vec<String> {
    let mut urls = vec![format!("{CHM10_URL}/{name}")];
    if let Some(rest) = name.strip_prefix("meta_chm_lat=0.0_") {
        urls.push(format!("{CHM10_URL}/meta_chm_lat=-0.0_{rest}"));
    }
    urls
}

/// A leaf-type square's file.
pub fn leaf_name(top: i32, left: i32) -> String {
    format!("lat{top}_lon{left}.tif")
}

/// Where a block's squares are: a folder, or URLs under a prefix, read through `crate::fetch` (its
/// mirror folder first; a `file://` prefix names a folder's files there, as `--record` keeps them).
#[derive(Clone, Debug)]
pub enum Source {
    Dir(PathBuf),
    Url(String),
}

impl Source {
    pub fn parse(s: &str) -> Source {
        if s.contains("://") {
            Source::Url(s.trim_end_matches('/').to_string())
        } else {
            Source::Dir(PathBuf::from(s))
        }
    }

    /// File `name`'s URL: its own, or a folder's as `file://`.
    fn url(&self, name: &str) -> String {
        match self {
            Source::Url(u) => format!("{u}/{name}"),
            Source::Dir(d) => format!("file://{}", std::path::absolute(d.join(name)).unwrap_or_else(|_| d.join(name)).display()),
        }
    }
}

/// What a block reads: the canopy and leaf-type squares, and where the bytes it reads are kept
/// (`record`: a mirror folder for `crate::fetch`, so a run elsewhere is given just those).
pub struct Inputs<'a> {
    pub chm: Source,
    pub leaf: Source,
    pub fetch: &'a dyn Fetch,
    pub record: Option<PathBuf>,
    /// The canopy squares found there before the blocks began (a z3 run's), (top, left): one of them
    /// not there when a block opens it fails the block (trees.py's did), never a block without its
    /// trees. None: what's there is all there is.
    pub there: Option<Vec<(i32, i32)>>,
}

/// The files a block opened, for `Inputs::keep`: each one's URL and its reads (None: not there).
type Opened = Vec<(String, Option<Arc<Noted>>)>;

impl Inputs<'_> {
    /// File `name` of `src`: None when it isn't there.
    fn open(&self, src: &Source, name: &str, opened: &mut Opened) -> Result<Option<Arc<dyn RangeRead>>> {
        let f: Option<Arc<dyn RangeRead>> = match src {
            Source::Dir(d) => {
                let p = d.join(name);
                match PlainFile::open(&p) {
                    Ok(f) => Some(Arc::new(f)),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e).with_context(|| p.display().to_string()),
                }
            }
            Source::Url(_) => self.fetch.open(&src.url(name))?,
        };
        if self.record.is_none() {
            return Ok(f);
        }
        let noted = f.map(Noted::new);
        opened.push((src.url(name), noted.clone()));
        Ok(noted.map(|n| n as Arc<dyn RangeRead>))
    }

    /// Keeps what the block read in the mirror folder `record`: the bytes read of each file, or
    /// that it isn't there. The bytes kept.
    fn keep(&self, opened: &Opened) -> Result<u64> {
        let Some(root) = &self.record else { return Ok(0) };
        let mut n = 0;
        for (url, f) in opened {
            n += match f {
                Some(f) => f.save(root, url)?,
                None => {
                    crate::fetch::save_none(root, url)?;
                    0
                }
            };
        }
        Ok(n)
    }
}

/// One square's band of a layer, sampled: each block row's source row, each block column's source
/// column, and the decoded TIFF.
struct Layer {
    tiff: crate::geotiff::Tiff,
    rows: Vec<Option<u32>>,
    cols: Vec<Option<u32>>,
}

/// A 10° square's source pixels for a block's rows and columns: `n` a side.
struct Pixels {
    n: u32,
    rows: Vec<Option<u32>>,
    cols: Vec<Option<u32>>,
}

/// A 10° square's pixels of `res` degrees from (`left`, `top`) that a block's rows and columns
/// fall in (trees.py's `sample`); None outside it.
fn indices(top: i32, left: i32, res: f64, lon: &[f64], lat: &[f64]) -> Pixels {
    let n = (10.0 / res).round();
    let at = |v: f64| {
        let i = v.floor();
        (i >= 0.0 && i < n).then_some(i as u32)
    };
    Pixels { n: n as u32, rows: lat.iter().map(|&l| at((top as f64 - l) / res)).collect(), cols: lon.iter().map(|&l| at((l - left as f64) / res)).collect() }
}

impl Layer {
    /// The square's layer in `src` at pixels `at`; None when the block doesn't meet the square.
    fn new(src: Arc<dyn RangeRead>, cache: usize, at: &Pixels, what: &str) -> Result<Option<Layer>> {
        if at.rows.iter().all(Option::is_none) || at.cols.iter().all(Option::is_none) {
            return Ok(None);
        }
        let tiff = crate::geotiff::Tiff::open(src).with_context(|| what.to_string())?.with_cache(cache);
        let img = tiff.level(0)?;
        let n = at.n;
        ensure!(img.width == n && img.height == n, "{what}: {}x{} pixels, not {n}x{n}", img.width, img.height);
        Ok(Some(Layer { tiff, rows: at.rows.clone(), cols: at.cols.clone() }))
    }

    /// Block rows `r0..r0 + out.len() / BS` from this square, into `out` (BS a row) where it's still
    /// `fill`, a decoded block's samples as `get` gives them.
    fn sample<T: Copy + PartialEq>(&self, r0: usize, out: &mut [T], fill: T, get: impl Fn(&crate::geotiff::Block) -> Option<&[T]>, what: &str) -> Result<()> {
        let img = self.tiff.level(0)?;
        let (bw, bh) = (img.block_w as usize, img.block_h as usize);
        // The block columns in the square, by the source blocks across holding them.
        let mut runs: Vec<(usize, usize, usize)> = Vec::new();
        for (j, c) in self.cols.iter().enumerate() {
            if let Some(c) = *c {
                let bx = c as usize / bw;
                match runs.last_mut() {
                    Some(r) if r.0 == bx && r.2 == j => r.2 = j + 1,
                    _ => runs.push((bx, j, j + 1)),
                }
            }
        }
        for (i, row) in out.as_chunks_mut::<BS>().0.iter_mut().enumerate() {
            let Some(r) = self.rows[r0 + i] else { continue };
            let (by, y) = (r as usize / bh, r as usize % bh);
            for &(bx, j0, j1) in &runs {
                let b = self.tiff.block(0, bx as u32, by as u32)?;
                let v = get(&b).with_context(|| format!("{what}: not the samples expected"))?;
                let src = &v[y * b.width..(y + 1) * b.width];
                for (o, c) in row[j0..j1].iter_mut().zip(&self.cols[j0..j1]) {
                    if *o == fill {
                        *o = src[c.unwrap() as usize - bx * bw];
                    }
                }
            }
        }
        Ok(())
    }
}

/// The squares a block samples, opened: per square, its canopy cover and height and its leaf type
/// (when there's a leaf-type square).
struct Sources {
    squares: Vec<(Layer, Layer, Option<Layer>)>,
}

/// Opens block (`bx`, `by`)'s squares (trees.py's `z3_block`): a canopy square is there when both
/// its files are, and not empty (Meta's "none"); a leaf-type square, when it is.
fn sources(inp: &Inputs, bx: u32, by: u32, lon: &[f64], lat: &[f64], opened: &mut Opened) -> Result<Sources> {
    let mut squares = Vec::new();
    for (top, left) in sorted(squares_of(tile_bounds(ZBLOCK, bx, by))) {
        let (cn, hn) = (chm_name(top, left, "cover5m"), chm_name(top, left, "p95"));
        let expected = inp.there.as_ref().is_some_and(|t| t.contains(&(top, left)));
        let (Some(c), Some(h)) = (inp.open(&inp.chm, &cn, opened)?, inp.open(&inp.chm, &hn, opened)?) else {
            anyhow::ensure!(!expected, "canopy square {cn} is gone since this run found it");
            continue;
        };
        if c.len()? == 0 || h.len()? == 0 {
            anyhow::ensure!(!expected, "canopy square {cn} is empty since this run found it whole");
            continue;
        }
        let at = indices(top, left, CHM_RES, lon, lat);
        let (Some(c), Some(h)) = (Layer::new(c, CHM_CACHE, &at, &cn)?, Layer::new(h, CHM_CACHE, &at, &hn)?) else { continue };
        let ln = leaf_name(top, left);
        let l = match inp.open(&inp.leaf, &ln, opened)? {
            Some(f) => Layer::new(f, LEAF_CACHE, &indices(top, left, LEAF_RES, lon, lat), &ln)?,
            None => None,
        };
        squares.push((c, h, l));
    }
    Ok(Sources { squares })
}

/// Squares in trees.py's order (sorted).
fn sorted(mut v: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    v.sort_unstable();
    v
}

/// A block's tiles and zoom-8 values.
pub struct BlockOut {
    pub tiles: Vec<Tile>,
    pub tops: Tops,
    /// Bytes read of the squares (kept in the mirror, with `record`).
    pub kept: u64,
}

/// Zoom-8 block (`bx`, `by`)'s tiles, zoom 12 to 8, inside the coverage's shapes (trees.py's
/// `z3_block`), and its zoom-8 values.
pub fn block(shapes: &mask::Shapes, inp: &Inputs, bx: u32, by: u32) -> Result<BlockOut> {
    let px = |i: usize, b: u32| (i + b as usize * BS) as f64 + 0.5;
    let lon: Vec<f64> = (0..BS).map(|j| lon_of(px(j, bx), ZMAX)).collect();
    let lat: Vec<f64> = (0..BS).map(|i| lat_of(px(i, by), ZMAX)).collect();
    let b = tile_bounds(ZBLOCK, bx, by);
    let mut opened = Vec::new();
    let src = sources(inp, bx, by, &lon, &lat, &mut opened)?;
    let inside = mask::inside(&shapes.meeting(b), b);
    let mut pyr = pyramid::Pyramid::new(bx, by);
    const BAND: usize = TS;
    let (mut cover, mut height, mut leaf) = (vec![0u16; BAND * BS], vec![0u16; BAND * BS], vec![255u8; BAND * BS]);
    for band in 0..BS / BAND {
        let r0 = band * BAND;
        cover.fill(0);
        height.fill(0);
        leaf.fill(255);
        for (c, h, l) in &src.squares {
            c.sample(r0, &mut cover, 0, |b| b.u16s(), "canopy cover")?;
            h.sample(r0, &mut height, 0, |b| b.u16s(), "canopy height")?;
            if let Some(l) = l {
                l.sample(r0, &mut leaf, 255, |b| b.u8s(), "leaf type")?;
            }
        }
        pyr.band(&cover, &height, &leaf, &inside[r0 * BS / 64..(r0 + BAND) * BS / 64]);
    }
    let (tiles, tops) = pyr.finish();
    let kept = inp.keep(&opened)?;
    Ok(BlockOut { tiles, tops, kept })
}

/// The tile archives being written, one a layer (by a temporary name, renamed when finished).
pub struct Writers {
    w: Vec<(ArchiveWriter, PathBuf, PathBuf)>,
}

impl Writers {
    pub fn create(dir: &Path) -> Result<Writers> {
        let mut w = Vec::new();
        for l in LAYERS {
            let path = dir.join(format!("trees-{l}.tiles"));
            let tmp = dir.join(format!("trees-{l}.tiles.tmp"));
            w.push((ArchiveWriter::create(&tmp, META).with_context(|| tmp.display().to_string())?, tmp, path));
        }
        Ok(Writers { w })
    }

    pub fn add(&mut self, t: &Tile) -> Result<()> {
        self.w[t.layer as usize].0.add(t.z, t.x, t.y, &t.webp, t.webp.len())
    }

    /// Each archive's tile count.
    pub fn finish(self) -> Result<[usize; 3]> {
        let mut n = [0; 3];
        for (i, (w, tmp, path)) in self.w.into_iter().enumerate() {
            w.finish()?;
            n[i] = Archive::open(&tmp)?.entries().len();
            std::fs::rename(&tmp, &path).with_context(|| path.display().to_string())?;
        }
        Ok(n)
    }
}

/// Writes block (`bx`, `by`)'s tiles to `out`'s archives and its zoom-8 values beside them
/// (`TOPS`): the task a worker runs.
pub fn block_files(shapes: &mask::Shapes, inp: &Inputs, bx: u32, by: u32, out: &Path) -> Result<BlockOut> {
    std::fs::create_dir_all(out)?;
    let b = block(shapes, inp, bx, by)?;
    let mut w = Writers::create(out)?;
    for t in &b.tiles {
        w.add(t)?;
    }
    w.finish()?;
    crate::whole::write(&out.join(TOPS), &b.tops.to_bytes()?)?;
    Ok(b)
}

/// Makes the z3 tile's archives in `out` from its blocks' folders (`block_files`): each block's
/// tiles as it made them, the blocks in order, then zoom 7 to 4 from their zoom-8 values (`said`
/// told how many of those are made, of how many). Each archive's tile count.
pub fn assemble(blocks: &[PathBuf], out: &Path, said: &(dyn Fn(u64, u64) + Sync)) -> Result<[usize; 3]> {
    std::fs::create_dir_all(out)?;
    let mut by_block: BTreeMap<(u32, u32), (PathBuf, Vec<u8>)> = BTreeMap::new();
    for d in blocks {
        let raw = std::fs::read(d.join(TOPS)).with_context(|| d.join(TOPS).display().to_string())?;
        let (x, y) = Tops::block_of(&raw).with_context(|| d.join(TOPS).display().to_string())?;
        if by_block.insert((x, y), (d.clone(), raw)).is_some() {
            bail!("block 8/{x}/{y} given twice");
        }
    }
    let mut w = Writers::create(out)?;
    for (d, _) in by_block.values() {
        for (i, l) in LAYERS.iter().enumerate() {
            let a = Archive::open(&d.join(format!("trees-{l}.tiles")))?;
            // (In the order the block made them: by where they lie in its archive.)
            let mut e = a.entries().to_vec();
            e.sort_unstable_by_key(|e| e.offset);
            for e in e {
                let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
                w.add(&Tile { layer: i as u8, z, x, y, webp: a.get_entry(&e).to_vec() })?;
            }
        }
    }
    let tops: BTreeMap<(u32, u32), Vec<u8>> = by_block.into_iter().map(|(k, (_, raw))| (k, raw)).collect();
    for t in pyramid::lower(&tops, said)? {
        w.add(&t)?;
    }
    w.finish()
}

/// Runs `f` on each of `n` items on as many threads as rayon's pool has (in WebAssembly, or with one,
/// in turn), giving `sink` their results in order, `done` told of each as it finishes: at most `2 ×`
/// threads are held at once. An item that panics on a thread fails the run as one that errs. (On
/// threads of its own, never the pool's: one of the pool's waiting here for room could be one an
/// item's own parallel work was waiting on, and the run would hang.)
fn in_order<T: Send>(n: usize, f: impl Fn(usize) -> Result<T> + Sync, done: impl Fn(usize) + Sync, mut sink: impl FnMut(usize, T) -> Result<()>) -> Result<()> {
    let threads = rayon::current_num_threads();
    if threads <= 1 {
        for i in 0..n {
            let r = f(i)?;
            done(i);
            sink(i, r)?;
        }
        return Ok(());
    }
    use std::sync::{Condvar, Mutex};
    struct State<T> {
        next: usize,
        written: usize,
        ready: BTreeMap<usize, Result<T>>,
        stop: bool,
    }
    let window = 2 * threads;
    let st = Mutex::new(State { next: 0, written: 0, ready: BTreeMap::new(), stop: false });
    let cv = Condvar::new();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = {
                    let mut g = st.lock().unwrap();
                    loop {
                        if g.stop || g.next >= n {
                            return;
                        }
                        if g.next < g.written + window {
                            g.next += 1;
                            break g.next - 1;
                        }
                        g = cv.wait(g).unwrap();
                    }
                };
                // (A panic as an error: the loop below would wait for its result for ever.)
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(i))).unwrap_or_else(|p| {
                    let why = p.downcast_ref::<String>().map(String::as_str).or_else(|| p.downcast_ref::<&str>().copied()).unwrap_or("a panic");
                    Err(anyhow::anyhow!("item {i} panicked: {why}"))
                });
                if r.is_ok() {
                    done(i);
                }
                st.lock().unwrap().ready.insert(i, r);
                cv.notify_all();
            });
        }
        // However the loop ends (a panic in `sink` too), the threads stop: the scope waits for them.
        struct Stop<'a, U>(&'a Mutex<State<U>>, &'a Condvar);
        impl<U> Drop for Stop<'_, U> {
            fn drop(&mut self) {
                self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).stop = true;
                self.1.notify_all();
            }
        }
        let _stop = Stop(&st, &cv);
        let mut out = Ok(());
        for i in 0..n {
            let r = {
                let mut g = st.lock().unwrap();
                loop {
                    if let Some(r) = g.ready.remove(&i) {
                        break r;
                    }
                    g = cv.wait(g).unwrap();
                }
            };
            out = r.and_then(|v| sink(i, v));
            let mut g = st.lock().unwrap();
            g.written = i + 1;
            if out.is_err() {
                g.stop = true;
            }
            drop(g);
            cv.notify_all();
            if out.is_err() {
                break;
            }
        }
        out
    })
}

/// The blocks of tile (`z`, `x`, `y`) (zoom 3 or 6) the coverage meets (a ring's box meets theirs),
/// in trees.py's order: by column, then row.
pub fn blocks_of(shapes: &mask::Shapes, z: u8, x: u32, y: u32) -> Vec<(u32, u32)> {
    let k = 1u32 << (ZBLOCK - z);
    let mut out = Vec::new();
    for bx in x * k..(x + 1) * k {
        for by in y * k..(y + 1) * k {
            if shapes.meets(tile_bounds(ZBLOCK, bx, by)) {
                out.push((bx, by));
            }
        }
    }
    out
}

/// The blocks of z3 tile (`qx`, `qy`) the coverage meets, in trees.py's order (`blocks_of`).
pub fn z3_blocks(shapes: &mask::Shapes, qx: u32, qy: u32) -> Vec<(u32, u32)> {
    blocks_of(shapes, 3, qx, qy)
}

/// A run's arguments: trees.py's `--z3` (a z3 tile: its zoom 4–12 tiles, `z3`), or the same for a z6
/// tile (`--z6`: its zoom 9–12 tiles and its mid, `z6`).
pub struct Run {
    /// The tile: zoom (3 or 6), column, row.
    pub tile: (u8, u32, u32),
    pub coverage: PathBuf,
    /// The canopy squares' cache (the units read it too), filled from `chm_store` (the NAS's).
    pub chm: PathBuf,
    pub chm_store: PathBuf,
    /// The leaf-type squares (the NAS's), made where missing by `dem`'s leaftype.py.
    pub leaf: PathBuf,
    pub out: PathBuf,
    pub dem: PathBuf,
}

/// A run begun (`begin`): the coverage's shapes, the blocks they meet, and the canopy squares there.
type Begun = (mask::Shapes, Vec<(u32, u32)>, Vec<(i32, i32)>);

/// A run begun: its tile's blocks the coverage meets, their canopy squares fetched where missing
/// and their leaf-type squares made; the coverage's shapes, the blocks and the canopy squares
/// there, with trees.py's line.
fn begin(a: &Run) -> Result<Begun> {
    let t0 = std::time::Instant::now();
    for d in [&a.chm, &a.leaf, &a.out] {
        std::fs::create_dir_all(d).with_context(|| d.display().to_string())?;
    }
    let text = std::fs::read_to_string(&a.coverage).with_context(|| a.coverage.display().to_string())?;
    let shapes = mask::Shapes::parse(&text).with_context(|| a.coverage.display().to_string())?;
    let (z, x, y) = a.tile;
    ensure!(z == 3 || z == 6, "a run is of a z3 or a z6 tile, not zoom {z}");
    let blocks = blocks_of(&shapes, z, x, y);
    // The canopy squares they touch, fetched when missing, and their leaf types.
    let mut want: Vec<(i32, i32)> = blocks.iter().flat_map(|&(bx, by)| squares_of(tile_bounds(ZBLOCK, bx, by))).collect();
    want.sort_unstable();
    want.dedup();
    let mut sqs = Vec::new();
    for (i, &(top, left)) in want.iter().enumerate() {
        crate::agent::jobs::report(i as u64, want.len() as u64, "canopy squares");
        // (And within the square, as its files come.)
        let said = |f: f64| crate::agent::jobs::report_f(i as f64 + f, want.len() as u64, "canopy squares");
        if squares::canopy(&a.chm, &a.chm_store, top, left, &said)? {
            sqs.push((top, left));
        }
    }
    crate::agent::jobs::report(want.len() as u64, want.len() as u64, "canopy squares");
    squares::leaf_types(&sqs, &a.leaf, &a.dem)?;
    eprintln!("trees z{z} {x},{y}: {} zoom-8 blocks, {} canopy squares ({:.0} s)", blocks.len(), sqs.len(), t0.elapsed().as_secs_f64());
    Ok((shapes, blocks, sqs))
}

/// Every one of `blocks` (on rayon's threads), its squares in `a`'s folders (`sqs` there), given to
/// `sink` in order as it's made, how many are made said.
fn each_block(a: &Run, shapes: &mask::Shapes, blocks: &[(u32, u32)], sqs: Vec<(i32, i32)>, mut sink: impl FnMut((u32, u32), BlockOut) -> Result<()>) -> Result<()> {
    let fetch = crate::fetch::MapFetch::default();
    let inp = Inputs { chm: Source::Dir(a.chm.clone()), leaf: Source::Dir(a.leaf.clone()), fetch: &fetch, record: None, there: Some(sqs) };
    // (Said before the first block is back, so the stage before's last line isn't shown meanwhile.)
    crate::agent::jobs::report(0, blocks.len() as u64, "zoom-8 blocks");
    let finished = std::sync::atomic::AtomicUsize::new(0);
    in_order(
        blocks.len(),
        |i| block(shapes, &inp, blocks[i].0, blocks[i].1),
        |_| {
            let k = finished.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            crate::agent::jobs::report(k as u64, blocks.len() as u64, "zoom-8 blocks");
        },
        |i, b| sink(blocks[i], b),
    )
}

/// Each archive's tile count, said; the run's last line.
fn end(a: &Run, n: [usize; 3], t0: std::time::Instant) -> [usize; 3] {
    for (l, n) in LAYERS.iter().zip(n) {
        eprintln!("trees-{l}.tiles: {n} tiles");
    }
    let (z, x, y) = a.tile;
    eprintln!("trees z{z} {x},{y}: done in {:.0} s", t0.elapsed().as_secs_f64());
    n
}

/// trees.py's `--z3` run: z3 tile `a.tile`'s blocks the coverage meets, their canopy squares
/// fetched where missing and their leaf-type squares made, then every block (on rayon's threads)
/// and zoom 7 to 4, into `out`'s three archives; the same progress lines.
pub fn z3(a: &Run) -> Result<[usize; 3]> {
    ensure!(a.tile.0 == 3, "a z3 run of a zoom-{} tile", a.tile.0);
    let t0 = std::time::Instant::now();
    let (shapes, blocks, sqs) = begin(a)?;
    let mut w = Writers::create(&a.out)?;
    let mut tops: BTreeMap<(u32, u32), Vec<u8>> = BTreeMap::new();
    each_block(a, &shapes, &blocks, sqs, |at, b| {
        for t in &b.tiles {
            w.add(t)?;
        }
        tops.insert(at, b.tops.to_bytes()?);
        Ok(())
    })?;
    for t in pyramid::lower(&tops, &|done, total| crate::agent::jobs::report(done, total, "zoom 7–4 tiles"))? {
        w.add(&t)?;
    }
    Ok(end(a, w.finish()?, t0))
}

/// A z6 tile's run (docs/plan.md §6, Trees: a piece): z6 tile `a.tile`'s blocks the coverage meets,
/// as `z3` runs them, its zoom 9–12 tiles into `out`'s three archives (its hi packs' tiles), and
/// its blocks' zoom-8 tiles and values into its mid, `out/MID` (`write_mid`), from which its z3
/// tile's assembly (`assemble_lo`) makes zoom 8 to 4. A z3 run's tiles are its z6 tiles' runs' and
/// their assembly's, the same bytes.
pub fn z6(a: &Run) -> Result<[usize; 3]> {
    ensure!(a.tile.0 == 6, "a z6 run of a zoom-{} tile", a.tile.0);
    let t0 = std::time::Instant::now();
    let (shapes, blocks, sqs) = begin(a)?;
    let mut w = Writers::create(&a.out)?;
    let mut mid: MidBlocks = BTreeMap::new();
    each_block(a, &shapes, &blocks, sqs, |at, b| {
        let (z8, hi): (Vec<Tile>, Vec<Tile>) = b.tiles.into_iter().partition(|t| t.z == ZBLOCK);
        for t in &hi {
            w.add(t)?;
        }
        mid.insert(at, (z8, b.tops.to_bytes()?));
        Ok(())
    })?;
    write_mid(&a.out.join(MID), (a.tile.1, a.tile.2), &mid)?;
    Ok(end(a, w.finish()?, t0))
}

/// A z6 tile's mid beside its archives (`z6`).
pub const MID: &str = "trees-mid.sect";
/// The mid's format.
const MID_FMT: u64 = 1;

/// A mid's blocks (`write_mid`): each block's zoom-8 tiles and its zoom-8 values (`Tops::to_bytes`).
pub type MidBlocks = BTreeMap<(u32, u32), (Vec<Tile>, Vec<u8>)>;

/// A z6 tile's mid, as `z6` writes it (`write_mid`): its tile, its blocks' zoom-8 tiles of the three
/// layers, and each block's zoom-8 values (`Tops::to_bytes`).
#[derive(Debug, PartialEq)]
pub struct Mid {
    pub tile: (u32, u32),
    pub z8: Vec<Tile>,
    pub tops: BTreeMap<(u32, u32), Vec<u8>>,
}

/// Writes z6 tile `tile`'s mid to `path`: what its z3 tile's assembly takes of it (`assemble_lo`),
/// its blocks' zoom-8 tiles and their zoom-8 values (which the tiles can't give back: rounded to
/// whole steps). A sectioned file (store::sect), meta `{"fmt": 1, "step": "trees", "tile":
/// "6/x/y", "v": TREES_V}`; per block (`blocks`: its zoom-8 tiles and values), by column then row,
/// a section `<layer>-8-<x>-<y>` for each layer whose zoom-8 tile it made, then `tops-8-<x>-<y>`.
pub fn write_mid(path: &Path, tile: (u32, u32), blocks: &MidBlocks) -> Result<()> {
    let meta = serde_json::json!({ "fmt": MID_FMT, "step": "trees", "tile": format!("6/{}/{}", tile.0, tile.1), "v": crate::treepacks::TREES_V });
    let tmp = path.with_extension("sect.tmp");
    let mut w = store::sect::SectWriter::create(&tmp, meta)?;
    for (&(x, y), (z8, tops)) in blocks {
        ensure!((x >> (ZBLOCK - 6), y >> (ZBLOCK - 6)) == tile, "block 8/{x}/{y} isn't in z6 tile 6/{}/{}", tile.0, tile.1);
        let mut z8: Vec<&Tile> = z8.iter().collect();
        z8.sort_by_key(|t| t.layer);
        for t in z8 {
            ensure!((t.z, t.x, t.y) == (ZBLOCK, x, y), "a zoom-8 tile of block 8/{x}/{y} that's {}/{}/{}", t.z, t.x, t.y);
            w.add(&format!("{}-8-{x}-{y}", LAYERS[t.layer as usize]), &t.webp)?;
        }
        ensure!(Tops::block_of(tops)? == (x, y), "block 8/{x}/{y}'s zoom-8 values are another block's");
        w.add(&format!("tops-8-{x}-{y}"), tops)?;
    }
    w.finish()?;
    std::fs::rename(&tmp, path).with_context(|| path.display().to_string())
}

/// A mid's file (`write_mid`), read and checked: its format and step version (a mid made for another
/// version of the layers isn't one to assemble), every section whole.
pub fn read_mid(path: &Path) -> Result<Mid> {
    let what = || path.display().to_string();
    let r = store::sect::SectReader::open(PlainFile::open(path).with_context(what)?).with_context(what)?;
    let m = r.meta();
    ensure!(m["fmt"].as_u64() == Some(MID_FMT) && m["step"] == "trees", "{}: not a tree cover mid", what());
    ensure!(m["v"].as_u64() == Some(crate::treepacks::TREES_V as u64), "{}: a mid of tree cover version {}, not {}", what(), m["v"], crate::treepacks::TREES_V);
    let tile = m["tile"].as_str().and_then(crate::legacy::Unit::parse).filter(|u| u.z == 6).with_context(|| format!("{}: no z6 tile", what()))?;
    let mut out = Mid { tile: (tile.x, tile.y), z8: Vec::new(), tops: BTreeMap::new() };
    for s in r.sections() {
        let parts: Vec<&str> = s.name.split('-').collect();
        let (kind, x, y) = match parts.as_slice() {
            [kind, "8", x, y] => (*kind, x.parse::<u32>().ok(), y.parse::<u32>().ok()),
            _ => bail!("{}: a section {:?}", what(), s.name),
        };
        let (Some(x), Some(y)) = (x, y) else { bail!("{}: a section {:?}", what(), s.name) };
        ensure!((x >> (ZBLOCK - 6), y >> (ZBLOCK - 6)) == out.tile, "{}: block 8/{x}/{y} isn't in its tile", what());
        let bytes = r.read(&s.name).with_context(what)?;
        if kind == "tops" {
            ensure!(Tops::block_of(&bytes)? == (x, y), "{}: {}'s values are another block's", what(), s.name);
            out.tops.insert((x, y), bytes);
        } else {
            let layer = LAYERS.iter().position(|l| *l == kind).with_context(|| format!("{}: a section {:?}", what(), s.name))?;
            out.z8.push(Tile { layer: layer as u8, z: ZBLOCK, x, y, webp: bytes });
        }
    }
    Ok(out)
}

/// A z3 tile's assembly (docs/plan.md §6, Trees): its zoom 8 to 4 in `out`'s three archives (its lo
/// packs' tiles), from its z6 tiles' mids (`z6`, `MID`): their blocks' zoom-8 tiles as they made
/// them, and zoom 7 to 4 from every block's values (`said` told how many of those are made, of how
/// many), as `z3` makes them. The mids must be of one z3 tile, each z6 tile once. Each archive's
/// tile count.
pub fn assemble_lo(mids: &[PathBuf], out: &Path, said: &(dyn Fn(u64, u64) + Sync)) -> Result<[usize; 3]> {
    std::fs::create_dir_all(out)?;
    let mut z8: Vec<Tile> = Vec::new();
    let mut tops: BTreeMap<(u32, u32), Vec<u8>> = BTreeMap::new();
    let mut tiles: BTreeMap<(u32, u32), PathBuf> = BTreeMap::new();
    for p in mids {
        let m = read_mid(p)?;
        if let Some(other) = tiles.insert(m.tile, p.clone()) {
            bail!("z6 tile 6/{}/{}'s mid given twice ({} and {})", m.tile.0, m.tile.1, other.display(), p.display());
        }
        let q = |t: (u32, u32)| (t.0 >> 3, t.1 >> 3);
        if let Some((first, _)) = tiles.first_key_value() {
            ensure!(q(*first) == q(m.tile), "mids of z3 tiles 3/{}/{} and 3/{}/{}", q(*first).0, q(*first).1, q(m.tile).0, q(m.tile).1);
        }
        z8.extend(m.z8);
        tops.extend(m.tops);
    }
    // (By block and layer, whatever order the mids came in.)
    z8.sort_by_key(|t| (t.x, t.y, t.layer));
    let mut w = Writers::create(out)?;
    for t in &z8 {
        w.add(t)?;
    }
    for t in pyramid::lower(&tops, said)? {
        w.add(&t)?;
    }
    w.finish()
}
