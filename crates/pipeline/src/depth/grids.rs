//! The depth sources as the build reads them (docs/inputs.md §6.10): each accepted declaration of
//! the `depth` role with its index of depth grids, the grids GeoTIFFs on the global Web Mercator
//! pixel grid of their zoom, kept in `sources/fetched/<id>/`, read by pixel arithmetic alone.
//!
//! A window of the zoom-z pixel grid is sampled from a grid of zoom zs: finer grids (zs ≥ z) by the
//! mean of the cells each pixel covers, from the overview whose cells line up with it; coarser ones
//! by bilinear interpolation of the cells' centres (no data where the nearest of the four has none,
//! as the map's fallback does). A grid's per-cell zoom (`zoom_file`) is the most any covered cell
//! supports, or the nearest cell's; its chart low water (`low_file`) is averaged like the depth.

use crate::geotiff::{BlockCache, Tiff};
use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Web Mercator's half extent (m).
pub const HALF: f64 = 20_037_508.342_789_244;

/// A Web Mercator pixel's size (m) at zoom z.
pub fn px(z: u8) -> f64 {
    2.0 * HALF / (256.0 * f64::from(1u32 << z))
}

/// Lon/lat of a global pixel corner (column, row) at zoom z.
pub fn lonlat(z: u8, c: f64, r: f64) -> (f64, f64) {
    let p = px(z);
    let (x, y) = (-HALF + c * p, HALF - r * p);
    (
        x / HALF * 180.0,
        (2.0 * (y / HALF * std::f64::consts::PI).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees(),
    )
}

/// The lon/lat box (w, s, e, n) of global pixels `c0..c0 + w`, `r0..r0 + h` at zoom z.
pub fn window_box(z: u8, c0: i64, r0: i64, w: usize, h: usize) -> [f64; 4] {
    let (w0, n0) = lonlat(z, c0 as f64, r0 as f64);
    let (e0, s0) = lonlat(z, (c0 + w as i64) as f64, (r0 + h as i64) as f64);
    [w0, s0, e0, n0]
}

/// One grid of a source.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    /// Its footprint (w, s, e, n, degrees).
    pub bbox: [f64; 4],
    pub zoom: u8,
    pub file: PathBuf,
    pub zoom_file: Option<PathBuf>,
    pub low_file: Option<PathBuf>,
}

/// A depth source, as its declaration and index say.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub id: String,
    /// Its byte in the class tiles (1–15).
    pub code: u8,
    /// Drawn in lakes (else on the sea).
    pub lake: bool,
    /// The ground metres it's feathered in over where it ends in the water.
    pub band_m: f32,
    /// Its positive values are drying heights.
    pub intertidal: bool,
    /// On the others' datum (false: it fills gaps only, hard-edged).
    pub datum_exact: bool,
    /// Drawn in rivers too: its depths are below the water's own surface there (a river's), not a
    /// fixed level's.
    pub rivers: bool,
    /// Its every cell an estimate (class `C_ESTIMATED`), not a measurement.
    pub estimated: bool,
    pub label: String,
    pub datum: String,
    pub notice: Option<String>,
    pub grids: Vec<Grid>,
    /// What keys name of it: its id and the digest of its declaration and index.
    pub pin: String,
}

impl Source {
    /// Declaration `s` (of the depth role), its grids in `sources/fetched/<id>/` under `root`.
    pub fn from_declared(root: &Path, s: &crate::inputs::sources::Source) -> Result<Source> {
        let dir = root.join("sources/fetched").join(&s.id);
        let p = &s.data.params;
        let code = s.param_u64("code")?;
        ensure!((1..=15).contains(&code), "the depth source {}: code {code} isn't 1–15", s.id);
        let mut grids = Vec::with_capacity(s.index.len());
        for f in &s.index {
            let name = |k: &str| f.props.get(k).and_then(|v| v.as_str()).map(|n| dir.join(n));
            grids.push(Grid {
                bbox: f.bbox,
                zoom: f
                    .props
                    .get("zoom")
                    .and_then(|v| v.as_u64())
                    .filter(|&z| z <= 18)
                    .with_context(|| format!("the depth source {}: a grid without a zoom", s.id))?
                    as u8,
                file: name("file").with_context(|| format!("the depth source {}: a grid without a file", s.id))?,
                zoom_file: name("zoom_file"),
                low_file: name("low_file"),
            });
        }
        // (Coarse first: a finer grid's cells win where grids overlap.)
        grids.sort_by(|a, b| a.zoom.cmp(&b.zoom).then(a.file.cmp(&b.file)));
        Ok(Source {
            id: s.id.clone(),
            code: code as u8,
            lake: s.param_str("water")? == "lake",
            band_m: p.get("band_m").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            intertidal: p.get("intertidal").and_then(|v| v.as_bool()).unwrap_or(false),
            datum_exact: p.get("datum_exact").and_then(|v| v.as_bool()).unwrap_or(true),
            rivers: p.get("rivers").and_then(|v| v.as_bool()).unwrap_or(false),
            estimated: p.get("estimated").and_then(|v| v.as_bool()).unwrap_or(false),
            label: s.param_str("label")?.to_string(),
            datum: s.param_str("datum")?.to_string(),
            notice: p.get("notice").and_then(|v| v.as_str()).map(str::to_string),
            grids,
            pin: format!("{} {}", s.id, s.digest()),
        })
    }

    /// Its grids meeting lon/lat box `b`.
    pub fn meeting(&self, b: [f64; 4]) -> impl Iterator<Item = &Grid> {
        self.grids
            .iter()
            .filter(move |g| g.bbox[0] <= b[2] && g.bbox[2] >= b[0] && g.bbox[1] <= b[3] && g.bbox[3] >= b[1])
    }

    /// The finest zoom of its grids meeting `b`: at most what any pixel there can have (None: it
    /// has nothing there).
    pub fn max_zoom_in(&self, b: [f64; 4]) -> Option<u8> {
        self.meeting(b).map(|g| g.zoom).max()
    }
}

/// The accepted depth sources of the records `manifest`, in code order; none before any version
/// of the declarations is accepted.
pub fn declared(root: &Path, manifest: &std::collections::BTreeMap<String, String>) -> Result<Vec<Source>> {
    let Some(s) = crate::inputs::sources::open(root, manifest)? else { return Ok(Vec::new()) };
    let mut out: Vec<Source> =
        s.many(crate::inputs::sources::DEPTH).iter().map(|d| Source::from_declared(root, d)).collect::<Result<_>>()?;
    out.sort_by_key(|s| s.code);
    Ok(out)
}

/// The GeoTIFFs open, sharing one block cache: at most `keep` of them, the least recently used
/// closed past that (task #438: a z3 tile's depth-lo met thousands of GLOBathy's grids, every one
/// kept open, and failed with "Too many open files"). A closed one is opened again when met again.
pub struct Reader {
    open: Mutex<Open>,
    cache: Arc<Mutex<BlockCache>>,
    keep: usize,
}

#[derive(Default)]
struct Open {
    /// Path → the file open; `uses`: when each was last used.
    files: HashMap<PathBuf, Arc<Tiff>>,
    /// The next cache id, and the use clock: ids never reused, so a reopened file never reads an
    /// evicted one's blocks under another's id.
    next_id: u64,
    clock: u64,
    uses: HashMap<PathBuf, u64>,
}

impl Default for Reader {
    fn default() -> Self {
        Reader::keeping(open_grids_limit())
    }
}

/// How many grids a reader keeps open: half the process's open-files limit, past 64 held for the
/// rest (packs, sockets, logs), 16 at least and 2,000 at most.
fn open_grids_limit() -> usize {
    let lim = crate::sys::open_files_limit().unwrap_or(256);
    (lim.saturating_sub(64) / 2).clamp(16, 2000) as usize
}

impl Reader {
    /// A reader keeping at most `keep` grids open.
    pub fn keeping(keep: usize) -> Self {
        Reader { open: Mutex::new(Open::default()), cache: Arc::new(Mutex::new(BlockCache::new(512 << 20))), keep: keep.max(1) }
    }

    /// How many grids are open now.
    pub fn open_count(&self) -> usize {
        self.open.lock().unwrap().files.len()
    }

    fn get(o: &mut Open, p: &Path) -> Option<Arc<Tiff>> {
        let t = o.files.get(p)?.clone();
        o.clock += 1;
        let c = o.clock;
        o.uses.insert(p.to_path_buf(), c);
        Some(t)
    }

    fn tiff(&self, p: &Path) -> Result<Arc<Tiff>> {
        if let Some(t) = Self::get(&mut self.open.lock().unwrap(), p) {
            return Ok(t);
        }
        let tiff = open_retrying(p, &|p| within(p, OPEN_DEADLINE, open_file), &OPEN_WAITS)
            .with_context(|| format!("{}", p.display()))?;
        // (Its cache id given as it's kept, under the one lock: two threads opening files at once
        // never share one.)
        let mut o = self.open.lock().unwrap();
        if let Some(t) = Self::get(&mut o, p) {
            return Ok(t);
        }
        // The least recently used closed (as the last of its users drops it) to make room.
        while o.files.len() >= self.keep {
            let Some(old) = o.uses.iter().min_by_key(|(_, &c)| c).map(|(k, _)| k.clone()) else { break };
            o.uses.remove(&old);
            o.files.remove(&old);
        }
        let id = o.next_id;
        o.next_id += 1;
        let t = Arc::new(tiff.with_shared_cache(id, self.cache.clone()));
        o.files.insert(p.to_path_buf(), t.clone());
        o.clock += 1;
        let c = o.clock;
        o.uses.insert(p.to_path_buf(), c);
        Ok(t)
    }

    /// `paths` opened (those not yet), `GRIDS_AT_ONCE` at once: over SMB each open and header read
    /// waits round trips, and a block can meet hundreds of a source's grids (BlueTopo's).
    fn open_all(&self, paths: &[&Path]) -> Result<()> {
        let todo: Vec<&Path> = {
            let o = self.open.lock().unwrap();
            paths.iter().copied().filter(|p| !o.files.contains_key(*p)).collect()
        };
        // (More than it keeps: opening them ahead would only close the first again. Each is opened
        // as it's sampled instead.)
        if todo.len() > self.keep {
            return Ok(());
        }
        if todo.is_empty() {
            return Ok(());
        }
        crate::timings::count(0, todo.len() as u64);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let failed: Mutex<Option<anyhow::Error>> = Mutex::new(None);
        std::thread::scope(|s| {
            for _ in 0..GRIDS_AT_ONCE.min(todo.len()) {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= todo.len() || failed.lock().unwrap().is_some() {
                        return;
                    }
                    if let Err(e) = self.tiff(todo[i]) {
                        failed.lock().unwrap().get_or_insert(e);
                        return;
                    }
                });
            }
        });
        match failed.into_inner().unwrap() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// How many of a source's grids are opened, or sampled, at once.
pub const GRIDS_AT_ONCE: usize = 8;

/// The waits between a source file's opens (task #398): an open on the NAS's SMB share can fail
/// for a moment on a file that's there ("No such file or directory", 19 Oct: two depth-lo jobs in
/// the same minute).
pub const OPEN_WAITS: [std::time::Duration; 4] = [
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(5),
    std::time::Duration::from_secs(15),
    std::time::Duration::from_secs(30),
];

/// A GeoTIFF opened, its header read. (Not on crate::agent::nasread's readers: their four
/// threads, busy, fail a call at once, and the grids are opened `GRIDS_AT_ONCE` at once.)
/// An I/O error in its header's read keeps its kind (a transient one is tried again); a header
/// that isn't a GeoTIFF's is `InvalidData` (never tried again).
fn open_file(p: &Path) -> std::io::Result<Tiff> {
    let f = store::range::PlainFile::open(p)?;
    Tiff::open(Arc::new(f)).map_err(|e| {
        let kind = e
            .chain()
            .find_map(|c| c.downcast_ref::<std::io::Error>().map(|i| (i.kind(), i.raw_os_error())))
            .or_else(|| {
                e.chain().find_map(|c| match c.downcast_ref::<store::IoError>() {
                    Some(store::IoError::Io(i)) => Some((i.kind(), i.raw_os_error())),
                    _ => None,
                })
            });
        match kind {
            Some((_, Some(code))) => std::io::Error::from_raw_os_error(code),
            Some((k, None)) => std::io::Error::new(k, format!("{e:#}")),
            None => std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e:#}")),
        }
    })
}

/// How long a source file's open and header read may take before the job fails (an SMB open can
/// stay stuck in the kernel for many minutes; the job's own retry takes over).
pub const OPEN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// `open(p)` on a thread of its own, waited for `deadline` at most: past it, `TimedOut` (the
/// thread stuck in it left behind).
pub fn within<T: Send + 'static>(
    p: &Path,
    deadline: std::time::Duration,
    open: fn(&Path) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let owned = p.to_path_buf();
    std::thread::Builder::new().name("depth-open".into()).spawn(move || {
        let _ = tx.send(open(&owned));
    })?;
    rx.recv_timeout(deadline).unwrap_or_else(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("its open took over {} s (the NAS's share stuck?)", deadline.as_secs()),
        ))
    })
}

/// Whether an open's failure may pass: SMB's moment of "No such file or directory" on a file
/// that's there, an interrupted or reset call, EIO, ESTALE. Not a deadline passed (the job fails,
/// its retry takes over), nor a file that isn't a GeoTIFF.
pub fn transient(e: &std::io::Error) -> bool {
    use std::io::ErrorKind as K;
    matches!(
        e.kind(),
        K::NotFound
            | K::Interrupted
            | K::WouldBlock
            | K::ConnectionReset
            | K::ConnectionAborted
            | K::NotConnected
            | K::BrokenPipe
    ) || matches!(e.raw_os_error(), Some(libc::EIO | libc::ESTALE | libc::EAGAIN | libc::EINTR))
}

/// `open(p)`, tried again after each of `waits` while it fails transiently (`transient`; its
/// last error returned), each retry logged.
pub fn open_retrying<T>(
    p: &Path,
    open: &dyn Fn(&Path) -> std::io::Result<T>,
    waits: &[std::time::Duration],
) -> std::io::Result<T> {
    let mut tries = 0;
    loop {
        match open(p) {
            Ok(t) => return Ok(t),
            Err(e) if tries < waits.len() && transient(&e) => {
                eprintln!("{}: {e}; opened again in {:?}", p.display(), waits[tries]);
                std::thread::sleep(waits[tries]);
                tries += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// How many of a window's blocks are read at once (`Tiff::prefetch_window`; 1: in turn, as
/// before task #398, which a measure sets to compare).
pub static WINDOW_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(8);

/// Whether a source's grids are opened and sampled `GRIDS_AT_ONCE` at once (false: in turn, as
/// before task #398, which a measure sets to compare).
pub static GRIDS_TOGETHER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The grids sampled for the block under way, and those met so far (`sample`), for its progress.
static SAMPLED: [std::sync::atomic::AtomicU64; 2] =
    [std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)];

/// The share of the block under way's grids sampled (the grids met so far: its later sources' not
/// yet counted), below 1.
pub fn sampled() -> f64 {
    let (d, m) =
        (SAMPLED[0].load(std::sync::atomic::Ordering::Relaxed), SAMPLED[1].load(std::sync::atomic::Ordering::Relaxed));
    if m == 0 {
        0.0
    } else {
        (d as f64 / m as f64).min(0.99)
    }
}

/// A new block: none sampled.
pub fn sampled_reset() {
    for a in &SAMPLED {
        a.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A source sampled onto a window: depth (NaN: none), the zoom each pixel's data supports (0:
/// none), its class (measured where its grid's own zoom, else interpolated), and the chart's low
/// water below MSL (NaN: none).
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub v: Vec<f32>,
    pub zoom: Vec<u8>,
    pub class: Vec<u8>,
    pub low: Vec<f32>,
}

/// Where a grid's image 0 sits on its zoom's global pixel grid: its first column and row.
fn origin(t: &Tiff, zoom: u8) -> Result<(i64, i64)> {
    let gt = t.transform(0)?;
    let p = px(zoom);
    ensure!(
        (gt[1] - p).abs() < p * 1e-6 && (gt[5] + p).abs() < p * 1e-6 && gt[2] == 0.0 && gt[4] == 0.0,
        "its pixels aren't zoom {zoom}'s ({} m)",
        gt[1]
    );
    let (c, r) = ((gt[0] + HALF) / p, (HALF - gt[3]) / p);
    ensure!((c - c.round()).abs() < 1e-3 && (r - r.round()).abs() < 1e-3, "it isn't on zoom {zoom}'s pixel grid");
    Ok((c.round() as i64, r.round() as i64))
}

/// The image of `t` whose cells are 2^l of image 0's (l ≤ `most`) and line up with (c0, r0)
/// (image 0's origin on the global grid), the largest such: (its index, l).
fn level_for(t: &Tiff, most: u32, c0: i64, r0: i64) -> (usize, u32) {
    let im = t.images();
    let (w0, h0) = (im[0].width, im[0].height);
    let mut best = (0usize, 0u32);
    for (i, img) in im.iter().enumerate().skip(1) {
        let r = (f64::from(w0) / f64::from(img.width)).log2().round();
        if !(1.0..=16.0).contains(&r) {
            continue;
        }
        let l = r as u32;
        let f = 1i64 << l;
        if l <= most
            && l > best.1
            && img.width == w0.div_ceil(f as u32)
            && img.height == h0.div_ceil(f as u32)
            && c0 % f == 0
            && r0 % f == 0
        {
            best = (i, l);
        }
    }
    best
}

/// Image `level` of `t` over its own pixels `x0..x1`, `y0..y1` (clamped to it): the values, the
/// window's first pixel and width; None when nothing of it is inside.
fn read(t: &Tiff, level: usize, x0: i64, y0: i64, x1: i64, y1: i64) -> Result<Option<(Vec<f32>, i64, i64, usize)>> {
    let img = t.level(level)?;
    let (a0, b0) = (x0.max(0), y0.max(0));
    let (a1, b1) = (x1.min(i64::from(img.width)), y1.min(i64::from(img.height)));
    if a1 <= a0 || b1 <= b0 {
        return Ok(None);
    }
    let (ww, wh) = ((a1 - a0) as u32, (b1 - b0) as u32);
    let v = {
        let reads = WINDOW_READS.load(std::sync::atomic::Ordering::Relaxed);
        if reads > 1 {
            t.prefetch_window(level, a0 as u32, b0 as u32, ww, wh, reads, GRIDS_AT_ONCE)?;
        }
        t.read_window(level, a0 as u32, b0 as u32, ww, wh)?
    };
    let nod = t.nodata().map(|n| n as f32);
    let v = v.into_iter().map(|x| if Some(x) == nod || !x.is_finite() { f32::NAN } else { x }).collect();
    Ok(Some((v, a0, b0, (a1 - a0) as usize)))
}

/// Source `src` on the zoom-z window of global pixels `c0..c0 + w`, `r0..r0 + h` (beyond the
/// world's edge: none); None when it has nothing there.
pub fn sample(rd: &Reader, src: &Source, z: u8, c0: i64, r0: i64, w: usize, h: usize) -> Result<Option<Sample>> {
    use crate::timings::{sub, Class};
    let b = window_box(z, c0, r0, w, h);
    let grids: Vec<&Grid> = src.meeting(b).collect();
    SAMPLED[1].fetch_add(grids.len() as u64, std::sync::atomic::Ordering::Relaxed);
    let together = GRIDS_TOGETHER.load(std::sync::atomic::Ordering::Relaxed);
    if together {
        let _p = sub("depth source files opened", Class::NasRead);
        let paths: Vec<&Path> = grids
            .iter()
            .flat_map(|g| std::iter::once(g.file.as_path()).chain(g.zoom_file.as_deref()).chain(g.low_file.as_deref()))
            .collect();
        rd.open_all(&paths).with_context(|| format!("the depth source {}", src.id))?;
    }
    let _p = sub("depth sources' windows read", Class::NasRead);
    let one = |g: &Grid| {
        sample_grid(rd, g, z, c0, r0, w, h)
            .with_context(|| format!("the depth source {}: {}", src.id, g.file.display()))
    };
    let mut out: Option<Sample> = None;
    // (In order, a chunk at a time: a finer grid's cells win where grids overlap.)
    for chunk in grids.chunks(if together { GRIDS_AT_ONCE } else { 1 }) {
        let made: Vec<Result<Option<Sample>>> = if chunk.len() == 1 {
            vec![one(chunk[0])]
        } else {
            std::thread::scope(|s| {
                let hs: Vec<_> = chunk.iter().map(|g| s.spawn(|| one(g))).collect();
                hs.into_iter()
                    .map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("a grid's sampling panicked"))))
                    .collect()
            })
        };
        SAMPLED[0].fetch_add(chunk.len() as u64, std::sync::atomic::Ordering::Relaxed);
        for s in made {
            let Some(s) = s? else { continue };
            match &mut out {
                None => out = Some(s),
                Some(o) => {
                    for i in 0..w * h {
                        if s.v[i].is_finite() {
                            o.v[i] = s.v[i];
                            o.zoom[i] = s.zoom[i];
                            o.class[i] = s.class[i];
                            o.low[i] = s.low[i];
                        }
                    }
                }
            }
        }
    }
    Ok(out.filter(|o| o.v.iter().any(|v| v.is_finite())))
}

/// One grid on the window.
fn sample_grid(rd: &Reader, g: &Grid, z: u8, c0: i64, r0: i64, w: usize, h: usize) -> Result<Option<Sample>> {
    let t = rd.tiff(&g.file)?;
    let (gc, gr) = origin(&t, g.zoom)?;
    let zt = g.zoom_file.as_deref().map(|p| rd.tiff(p)).transpose()?;
    let lt = g.low_file.as_deref().map(|p| rd.tiff(p)).transpose()?;
    for x in zt.iter().chain(lt.iter()) {
        ensure!(
            origin(x, g.zoom)? == (gc, gr)
                && x.images()[0].width == t.images()[0].width
                && x.images()[0].height == t.images()[0].height,
            "its zoom or low-water file isn't on its own grid"
        );
    }
    let mut s =
        Sample { v: vec![f32::NAN; w * h], zoom: vec![0; w * h], class: vec![0; w * h], low: vec![f32::NAN; w * h] };
    if g.zoom >= z {
        let k = u32::from(g.zoom - z);
        if k > 24 {
            bail!("zoom {} sampled at {z}", g.zoom);
        }
        let (lv, l) = level_for(&t, k, gc, gr);
        let f = 1i64 << (k - l); // an output pixel's cells across, at that level
        let sh = |v: i64| v >> l;
        // (The level's cells covering the window, in its own pixels.)
        let (x0, y0) = (sh((c0 << k) - gc), sh((r0 << k) - gr));
        let (x1, y1) = (sh(((c0 + w as i64) << k) - gc), sh(((r0 + h as i64) << k) - gr));
        let Some((v, a0, b0, rw)) = read(&t, lv, x0, y0, x1, y1)? else { return Ok(None) };
        let zv = match &zt {
            Some(zt) => read(zt, level_for(zt, k, gc, gr).0.min(lv), x0, y0, x1, y1)?.map(|r| r.0),
            None => None,
        };
        let lw = match &lt {
            Some(lt) => read(lt, level_for(lt, k, gc, gr).0.min(lv), x0, y0, x1, y1)?.map(|r| r.0),
            None => None,
        };
        let rh = v.len() / rw.max(1);
        let mut sum = vec![0f64; w * h];
        let mut n = vec![0u32; w * h];
        let mut lsum = vec![0f64; w * h];
        let mut ln = vec![0u32; w * h];
        for j in 0..rh {
            let oy = (b0 + j as i64 - y0) / f;
            if !(0..h as i64).contains(&oy) {
                continue;
            }
            for i in 0..rw {
                let ox = (a0 + i as i64 - x0) / f;
                if !(0..w as i64).contains(&ox) {
                    continue;
                }
                let x = v[j * rw + i];
                if !x.is_finite() {
                    continue;
                }
                let o = oy as usize * w + ox as usize;
                sum[o] += f64::from(x);
                n[o] += 1;
                let cz = zv.as_ref().map_or(g.zoom, |zv| zv[j * rw + i].clamp(0.0, 30.0) as u8).min(g.zoom);
                s.zoom[o] = s.zoom[o].max(cz);
                if let Some(lw) = &lw {
                    let y = lw[j * rw + i];
                    if y.is_finite() {
                        lsum[o] += f64::from(y);
                        ln[o] += 1;
                    }
                }
            }
        }
        for o in 0..w * h {
            if n[o] > 0 {
                s.v[o] = (sum[o] / f64::from(n[o])) as f32;
                if ln[o] > 0 {
                    s.low[o] = (lsum[o] / f64::from(ln[o])) as f32;
                }
            } else {
                s.zoom[o] = 0;
            }
        }
    } else {
        let k = u32::from(z - g.zoom);
        let scale = 1.0 / f64::from(1u32 << k);
        // (Each output pixel's centre in the grid's own pixels, cell centres at +0.5.)
        let at = |c: i64, origin: i64| (c as f64 + 0.5) * scale - 0.5 - origin as f64;
        let (fx0, fy0) = (at(c0, gc).floor() as i64, at(r0, gr).floor() as i64);
        let (fx1, fy1) = (at(c0 + w as i64, gc).floor() as i64 + 2, at(r0 + h as i64, gr).floor() as i64 + 2);
        let Some((v, a0, b0, rw)) = read(&t, 0, fx0, fy0, fx1, fy1)? else { return Ok(None) };
        let zv = match &zt {
            Some(zt) => read(zt, 0, fx0, fy0, fx1, fy1)?.map(|r| r.0),
            None => None,
        };
        let lw = match &lt {
            Some(lt) => read(lt, 0, fx0, fy0, fx1, fy1)?.map(|r| r.0),
            None => None,
        };
        let rh = v.len() / rw.max(1);
        let get = |vv: &[f32], x: i64, y: i64| -> f32 {
            let (i, j) = (x - a0, y - b0);
            if i < 0 || j < 0 || i >= rw as i64 || j >= rh as i64 {
                f32::NAN
            } else {
                vv[j as usize * rw + i as usize]
            }
        };
        for oy in 0..h {
            let sy = at(r0 + oy as i64, gr);
            let y = sy.floor();
            let ty = (sy - y) as f32;
            for ox in 0..w {
                let sx = at(c0 + ox as i64, gc);
                let x = sx.floor();
                let tx = (sx - x) as f32;
                let (xi, yi) = (x as i64, y as i64);
                let q = [get(&v, xi, yi), get(&v, xi + 1, yi), get(&v, xi, yi + 1), get(&v, xi + 1, yi + 1)];
                let wt = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
                let near = (0..4).max_by(|&a, &b| wt[a].total_cmp(&wt[b])).unwrap_or(0);
                if !q[near].is_finite() {
                    continue;
                }
                let (mut sum, mut ws) = (0f32, 0f32);
                for i in 0..4 {
                    if q[i].is_finite() {
                        sum += q[i] * wt[i];
                        ws += wt[i];
                    }
                }
                let o = oy * w + ox;
                s.v[o] = sum / ws;
                let (nx, ny) = (xi + (near & 1) as i64, yi + (near >> 1) as i64);
                s.zoom[o] = zv.as_ref().map_or(g.zoom, |zv| get(zv, nx, ny).clamp(0.0, 30.0) as u8).min(g.zoom);
                if let Some(lw) = &lw {
                    s.low[o] = get(lw, nx, ny);
                }
            }
        }
    }
    for o in 0..w * h {
        if s.v[o].is_finite() {
            s.class[o] = if s.zoom[o] >= g.zoom { super::C_MEASURED } else { super::C_INTERP };
        }
    }
    Ok(s.v.iter().any(|v| v.is_finite()).then_some(s))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A depth grid file of zoom `zoom` at global pixel (`c`, `r`), `w` × `h`, with overviews of
    /// 2× and 4×, values `f(col, row)` (global pixels), NaN where it says.
    pub(crate) fn grid_file(path: &Path, zoom: u8, c: i64, r: i64, w: u32, h: u32, f: &dyn Fn(i64, i64) -> f32) {
        let p = px(zoom);
        let gt = [-HALF + c as f64 * p, p, 0.0, HALF - r as f64 * p, 0.0, -p];
        let mut levels = Vec::new();
        let mut f_ = 1u32;
        while f_ <= 4 {
            let (lw, lh) = (w.div_ceil(f_), h.div_ceil(f_));
            let mut v = vec![f32::NAN; (lw * lh) as usize];
            for j in 0..lh {
                for i in 0..lw {
                    let (mut s, mut n) = (0f64, 0);
                    for dy in 0..f_ {
                        for dx in 0..f_ {
                            let (x, y) = (i * f_ + dx, j * f_ + dy);
                            if x < w && y < h {
                                let a = f(c + x as i64, r + y as i64);
                                if a.is_finite() {
                                    s += f64::from(a);
                                    n += 1;
                                }
                            }
                        }
                    }
                    if n > 0 {
                        v[(j * lw + i) as usize] = (s / f64::from(n)) as f32;
                    }
                }
            }
            levels.push((lw, lh, v));
            f_ *= 2;
        }
        use crate::geotiff::testtiff::{make_images, Opts};
        let sizes: Vec<(u32, u32)> = levels.iter().map(|l| (l.0, l.1)).collect();
        let (b, _) = make_images(&sizes, Opts { tile: Some(64), ..Opts::default() }, gt, Some("nan"), &|k, x, y, _| {
            f64::from(levels[k].2[(y * levels[k].0 + x) as usize])
        });
        std::fs::write(path, b).unwrap();
    }

    fn source(grids: Vec<Grid>) -> Source {
        Source {
            id: "t".into(),
            code: 1,
            lake: false,
            band_m: 0.0,
            intertidal: false,
            datum_exact: true,
            rivers: false,
            estimated: false,
            label: "T".into(),
            datum: "MSL".into(),
            notice: None,
            grids,
            pin: "t".into(),
        }
    }

    fn bbox(zoom: u8, c: i64, r: i64, w: u32, h: u32) -> [f64; 4] {
        window_box(zoom, c, r, w as usize, h as usize)
    }

    /// A plane sampled coarser is its cells' mean, finer its bilinear interpolation: the plane.
    #[test]
    fn a_grid_is_averaged_down_and_interpolated_up() {
        let d = tempfile::tempdir().unwrap();
        let (zoom, c, r) = (12u8, 1200 * 256i64, 1500 * 256i64);
        let plane = |x: i64, y: i64| -((x - c) as f32) * 0.5 - (y - r) as f32 * 0.25 - 10.0;
        let p = d.path().join("g.tif");
        grid_file(&p, zoom, c, r, 1024, 512, &plane);
        let s = source(vec![Grid {
            bbox: bbox(zoom, c, r, 1024, 512),
            zoom,
            file: p.clone(),
            zoom_file: None,
            low_file: None,
        }]);
        let rd = Reader::default();
        // Two zooms out: each pixel the mean of 4 × 4 cells, from the 4× overview.
        let got = sample(&rd, &s, 10, c / 4 + 3, r / 4 + 5, 8, 8).unwrap().unwrap();
        for (o, v) in got.v.iter().enumerate() {
            let (x, y) = (c / 4 + 3 + (o % 8) as i64, r / 4 + 5 + (o / 8) as i64);
            let want = plane(x * 4, y * 4) - 0.5 * 1.5 - 0.25 * 1.5;
            assert!((v - want).abs() < 1e-3, "{o}: {v} {want}");
            assert_eq!(got.zoom[o], zoom);
        }
        // One zoom in: the plane at each pixel's centre.
        let got = sample(&rd, &s, 13, 2 * c + 40, 2 * r + 40, 16, 16).unwrap().unwrap();
        for (o, v) in got.v.iter().enumerate() {
            let (x, y) = ((2 * c + 40 + (o % 16) as i64) as f64 + 0.5, (2 * r + 40 + (o / 16) as i64) as f64 + 0.5);
            let want = -((x / 2.0 - 0.5) - c as f64) * 0.5 - ((y / 2.0 - 0.5) - r as f64) * 0.25 - 10.0;
            assert!((f64::from(*v) - want).abs() < 1e-3, "{o}: {v} {want}");
        }
        // Outside it: nothing.
        assert!(sample(&rd, &s, 12, c + 5000, r, 8, 8).unwrap().is_none());
    }

    /// Task #438: a reader keeps at most its share of grids open (a z3 tile met thousands of
    /// GLOBathy's, kept open, and failed "Too many open files"), closing the least recently used,
    /// and samples the same as one that keeps them all, also when a source has more grids in a
    /// window than it keeps.
    #[test]
    fn a_reader_keeps_few_grids_open_and_samples_the_same() {
        let d = tempfile::tempdir().unwrap();
        let (zoom, c, r) = (12u8, 1200 * 256i64, 1500 * 256i64);
        let grids: Vec<Grid> = (0..7i64)
            .map(|i| {
                let (gc, gr) = (c + 40 * i, r);
                let p = d.path().join(format!("g{i}.tif"));
                grid_file(&p, zoom, gc, gr, 64, 64, &|x, y| -(i as f32) - ((x - gc) as f32) * 0.01 - (y - gr) as f32 * 0.02);
                Grid { bbox: bbox(zoom, gc, gr, 64, 64), zoom, file: p, zoom_file: None, low_file: None }
            })
            .collect();
        let s = source(grids);
        let all = Reader::keeping(100);
        let few = Reader::keeping(3);
        // Windows over all seven, then over a few in turn (closing and opening again).
        let mut wins = vec![(c, r, 320usize, 64usize)];
        for i in 0..7i64 {
            wins.push((c + 40 * i + 10, r + 5, 30, 30));
        }
        wins.push((c, r, 320, 64));
        for (x, y, w, h) in wins {
            let a = sample(&all, &s, zoom, x, y, w, h).unwrap().unwrap();
            let b = sample(&few, &s, zoom, x, y, w, h).unwrap().unwrap();
            assert_eq!(a.v.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), b.v.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
            assert!(few.open_count() <= 3, "{}", few.open_count());
        }
        assert_eq!(all.open_count(), 7);
    }

    /// Task #398 review: grids opened and sampled `GRIDS_AT_ONCE` at once, and windows' blocks read
    /// at once, give the same sample bit for bit as in turn: eleven overlapping grids (a chunk's
    /// boundary crossed), each with holes, the later ones winning where they have data.
    #[test]
    fn grids_sampled_together_are_the_same_as_in_turn() {
        use std::sync::atomic::Ordering;
        let d = tempfile::tempdir().unwrap();
        let (zoom, c, r) = (12u8, 1200 * 256i64, 1500 * 256i64);
        let grids: Vec<Grid> = (0..11i64)
            .map(|i| {
                let (gc, gr) = (c + 20 * i, r + 12 * i);
                let p = d.path().join(format!("g{i}.tif"));
                grid_file(&p, zoom, gc, gr, 320, 256, &|x, y| {
                    if (x + y) % (i + 3) == 0 {
                        f32::NAN
                    } else {
                        -(i as f32) * 10.0 - ((x - gc) as f32) * 0.01 - ((y - gr) as f32) * 0.02
                    }
                });
                Grid { bbox: bbox(zoom, gc, gr, 320, 256), zoom, file: p, zoom_file: None, low_file: None }
            })
            .collect();
        let s = source(grids);
        let bits = |x: &Sample| -> Vec<u32> {
            x.v.iter()
                .chain(&x.low)
                .map(|v| v.to_bits())
                .chain(x.zoom.iter().chain(&x.class).map(|&b| u32::from(b)))
                .collect()
        };
        let mut got = Vec::new();
        for (together, reads) in [(false, 1), (true, 8)] {
            GRIDS_TOGETHER.store(together, Ordering::Relaxed);
            WINDOW_READS.store(reads, Ordering::Relaxed);
            let rd = Reader::default();
            got.push(bits(&sample(&rd, &s, zoom, c - 10, r - 10, 600, 420).unwrap().unwrap()));
        }
        GRIDS_TOGETHER.store(true, Ordering::Relaxed);
        WINDOW_READS.store(8, Ordering::Relaxed);
        assert!(got[0] == got[1], "sampled together, not the same as in turn");
    }

    /// Task #398 review: only a transient failure is tried again; a deadline passed fails at once.
    #[test]
    fn only_transient_opens_are_tried_again_and_a_stuck_one_times_out() {
        use std::io;
        let waits = [std::time::Duration::from_millis(1); 3];
        let tries = std::cell::Cell::new(0);
        let bad = |_: &Path| -> io::Result<()> {
            tries.set(tries.get() + 1);
            Err(io::Error::new(io::ErrorKind::InvalidData, "not a TIFF"))
        };
        assert!(open_retrying(Path::new("x"), &bad, &waits).is_err());
        assert_eq!(tries.get(), 1);
        assert!(transient(&io::Error::from_raw_os_error(libc::EIO)));
        fn stuck(_: &Path) -> io::Result<u8> {
            std::thread::sleep(std::time::Duration::from_secs(30));
            Ok(1)
        }
        let t0 = std::time::Instant::now();
        let e = within(Path::new("x"), std::time::Duration::from_millis(50), stuck).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(!transient(&e));
        store::clock::ended_before(t0, std::time::Duration::from_secs(30), "the stuck open ends at its deadline");
        // A file that isn't a GeoTIFF: InvalidData, not tried again.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("n.tif"), b"nope").unwrap();
        assert!(!transient(&open_file(&d.path().join("n.tif")).map(|_| ()).unwrap_err()));
        assert_eq!(open_file(&d.path().join("missing.tif")).map(|_| ()).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
