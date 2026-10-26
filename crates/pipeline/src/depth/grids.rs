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
use std::path::Path;
use std::sync::{Arc, Mutex};
use store::nasfs::{NasPath, Place};
use store::range::RangeRead;

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
    pub file: NasPath,
    pub zoom_file: Option<NasPath>,
    pub low_file: Option<NasPath>,
    /// Where its files are packed (task #452): the pack in the source's folder, and each file's
    /// bytes in it (`file`, `zoom_file`, `low_file`); None: loose files, as before.
    pub packed: Option<Packed>,
}

/// A grid's files inside a pack: a tar of whole GeoTIFFs (producers/depth/packs.py), each read in
/// place by its byte range (store::range::Slice), so the TIFF reading is a loose file's.
#[derive(Clone, Debug, PartialEq)]
pub struct Packed {
    pub pack: Arc<NasPath>,
    pub at: [Option<(u64, u64)>; 3],
}

/// Where one of a grid's files is read: its own name (its identity) and, packed, its pack and
/// byte range.
#[derive(Clone, Copy, Debug)]
pub struct Loc<'a> {
    pub path: &'a NasPath,
    pub pack: Option<(&'a NasPath, u64, u64)>,
}

impl<'a> Loc<'a> {
    pub fn loose(path: &'a NasPath) -> Self {
        Loc { path, pack: None }
    }
}

impl Grid {
    /// Its file `k` (0 depth, 1 zoom, 2 low water), if it has it.
    pub fn loc(&self, k: usize) -> Option<Loc<'_>> {
        let path = match k {
            0 => &self.file,
            1 => self.zoom_file.as_ref()?,
            _ => self.low_file.as_ref()?,
        };
        let pack = self.packed.as_ref().and_then(|p| p.at[k].map(|(o, n)| (&*p.pack, o, n)));
        Some(Loc { path, pack })
    }
}

/// The index properties naming where a grid is packed (`pack`, `at`): not what it is, so left out
/// of the source's pin (a repack builds nothing again).
pub const PACK_PROPS: [&str; 2] = ["pack", "at"];

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
    pub fn from_declared(root: &store::nasfs::NasRoot, s: &crate::inputs::sources::Source) -> Result<Source> {
        let dir = root.join("sources/fetched").join(&s.id);
        let p = &s.data.params;
        let code = s.param_u64("code")?;
        ensure!((1..=15).contains(&code), "the depth source {}: code {code} isn't 1–15", s.id);
        let mut grids = Vec::with_capacity(s.index.len());
        let mut packs: HashMap<String, Arc<NasPath>> = HashMap::new();
        for f in &s.index {
            let name = |k: &str| f.props.get(k).and_then(|v| v.as_str()).map(|n| dir.join(n));
            let packed = match f.props.get("pack").and_then(|v| v.as_str()) {
                None => None,
                Some(p) => {
                    let pack = packs.entry(p.to_string()).or_insert_with(|| Arc::new(dir.join(p))).clone();
                    let at = |k: &str| -> Option<(u64, u64)> {
                        let a = f.props.get("at")?.get(k)?.as_array()?;
                        Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?))
                    };
                    let at = [at("file"), at("zoom_file"), at("low_file")];
                    ensure!(at[0].is_some(), "the depth source {}: a packed grid without its file's range", s.id);
                    Some(Packed { pack, at })
                }
            };
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
                packed,
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
            pin: format!("{} {}", s.id, unpacked_digest(s)),
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

/// Declaration `s`'s digest with its grids' packing left out (`PACK_PROPS`): the same packed or
/// loose, so a repack keeps every key.
pub fn unpacked_digest(s: &crate::inputs::sources::Source) -> String {
    let mut s = s.clone();
    for f in &mut s.index {
        for k in PACK_PROPS {
            f.props.remove(k);
        }
    }
    s.digest()
}

/// The accepted depth sources of the records `manifest`, in code order; none before any version
/// of the declarations is accepted.
pub fn declared(
    root: &store::nasfs::NasRoot,
    manifest: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<Source>> {
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
    /// This Mac's whole copies of the packs (task #551: crate::localread), read in place of the
    /// NAS's where it has them or room for them; None, the NAS's read.
    local: Option<Arc<crate::localread::Local>>,
}

#[derive(Default)]
struct Open {
    /// Path → the file open; `uses`: when each was last used.
    files: HashMap<NasPath, Arc<Tiff>>,
    /// The next cache id, and the use clock: ids never reused, so a reopened file never reads an
    /// evicted one's blocks under another's id.
    next_id: u64,
    clock: u64,
    uses: HashMap<NasPath, u64>,
    /// The packs open (task #452), each opened once and its grids read from it by range; the least
    /// recently used let go past the reader's `keep` (closed when its last grid is).
    packs: HashMap<NasPath, (Arc<Counted<store::range::PlainFile>>, u64)>,
}

/// A depth file's reads counted (`io_counts`).
pub struct Counted<R>(R);

impl<R: RangeRead> RangeRead for Counted<R> {
    fn len(&self) -> Result<u64, store::IoError> {
        self.0.len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::IoError> {
        IO[1].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.0.read_at(off, len)
    }
}

/// The depth files opened (a loose grid or a pack) and read (each range read), since the process
/// began: a measure's.
static IO: [std::sync::atomic::AtomicU64; 2] =
    [std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0)];

/// (opens, reads) of depth files so far.
pub fn io_counts() -> (u64, u64) {
    (IO[0].load(std::sync::atomic::Ordering::Relaxed), IO[1].load(std::sync::atomic::Ordering::Relaxed))
}

/// How many grids a reader closes at once as it goes: a close is a round trip on the NAS and holds
/// nothing else up (3/3/2's assembly, its grids closed: 15.6 s one at a time, 3.9 s 8 at once,
/// 1.2 s 32 at once).
const CLOSES_AT_ONCE: usize = 32;

/// Its grids closed `CLOSES_AT_ONCE` at once as it goes: a reader can hold 2,000.
impl Drop for Reader {
    fn drop(&mut self) {
        let (files, packs): (Vec<Arc<Tiff>>, Vec<_>) = {
            let mut o = self.open.lock().unwrap_or_else(|e| e.into_inner());
            o.uses.clear();
            (o.files.drain().map(|(_, t)| t).collect(), o.packs.drain().map(|(_, p)| p.0).collect())
        };
        // (The grids first: a pack is closed with the last of them.)
        close_all(files, CLOSES_AT_ONCE);
        close_all(packs, CLOSES_AT_ONCE);
    }
}

/// `files` dropped (closed, where theirs is the last handle) by `at_once` threads, no more than
/// there are files; those a thread can't be started for closed on this one (it may run in an
/// unwind: never a panic).
fn close_all<T: Send>(files: Vec<T>, at_once: usize) {
    if files.len() < 2 {
        return;
    }
    let n = at_once.clamp(1, files.len());
    let files = Mutex::new(files);
    let close = || loop {
        let Some(t) = files.lock().unwrap_or_else(|e| e.into_inner()).pop() else { return };
        drop(t);
    };
    std::thread::scope(|s| {
        for _ in 0..n {
            if std::thread::Builder::new().spawn_scoped(s, close).is_err() {
                break;
            }
        }
        close();
    });
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
        Reader {
            open: Mutex::new(Open::default()),
            cache: Arc::new(Mutex::new(BlockCache::new(512 << 20))),
            keep: keep.max(1),
            local: None,
        }
    }

    /// It reading the packs from `local`'s copies.
    pub fn with_local(mut self, local: Option<Arc<crate::localread::Local>>) -> Self {
        self.local = local;
        self
    }

    /// How many grids are open now.
    pub fn open_count(&self) -> usize {
        self.open.lock().unwrap().files.len()
    }

    fn get(o: &mut Open, p: &NasPath) -> Option<Arc<Tiff>> {
        let t = o.files.get(p)?.clone();
        o.clock += 1;
        let c = o.clock;
        o.uses.insert(p.clone(), c);
        Some(t)
    }

    /// Loose file `p` (a test's).
    #[cfg(test)]
    fn tiff(&self, p: &NasPath) -> Result<Arc<Tiff>> {
        self.tiff_at(Loc::loose(p))
    }

    /// Pack `p`, open (opened now, or kept from before).
    fn pack(&self, p: &NasPath) -> Result<Arc<Counted<store::range::PlainFile>>> {
        {
            let mut o = self.open.lock().unwrap();
            o.clock += 1;
            let c = o.clock;
            if let Some(e) = o.packs.get_mut(p) {
                e.1 = c;
                return Ok(e.0.clone());
            }
        }
        // (Its whole local copy where there's one or room for one: one sequential read of the NAS,
        // in place of a seek a grid's every block.)
        let at = crate::localread::path(self.local.as_deref(), p);
        let f = match &at {
            Place::Local(l) => {
                open_retrying(l.as_path(), &|l| within(l, OPEN_DEADLINE, plain_local), &store::retry::waits())
            }
            Place::Nas(n) => open_retrying(n, &|n| within(n, OPEN_DEADLINE, plain_nas), &store::retry::waits()),
        }
        .with_context(|| at.loc().display())?;
        IO[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut o = self.open.lock().unwrap();
        if let Some(e) = o.packs.get(p) {
            return Ok(e.0.clone());
        }
        while o.packs.len() >= self.keep {
            let Some(old) = o.packs.iter().min_by_key(|(_, e)| e.1).map(|(k, _)| k.clone()) else { break };
            o.packs.remove(&old);
        }
        let f = Arc::new(Counted(f));
        o.clock += 1;
        let c = o.clock;
        o.packs.insert(p.clone(), (f.clone(), c));
        Ok(f)
    }

    fn tiff_at(&self, l: Loc<'_>) -> Result<Arc<Tiff>> {
        let p = l.path;
        if let Some(t) = Self::get(&mut self.open.lock().unwrap(), p) {
            return Ok(t);
        }
        let tiff = match l.pack {
            None => open_retrying(p, &|p| within(p, OPEN_DEADLINE, open_file), &store::retry::waits())
                .with_context(|| format!("{}", p.display()))?,
            Some((pack, off, len)) => {
                let f = self.pack(pack)?;
                // (Its header read under the same deadline and retries as a loose file's open.)
                open_retrying(
                    p,
                    &|_| {
                        let f = f.clone();
                        within_do(OPEN_DEADLINE, move || {
                            let s = store::range::Slice::new(f, off, len)
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
                            tiff_of(Arc::new(s))
                        })
                    },
                    &store::retry::waits(),
                )
                .with_context(|| format!("{} in {}", p.display(), pack.display()))?
            }
        };
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
        o.files.insert(p.clone(), t.clone());
        o.clock += 1;
        let c = o.clock;
        o.uses.insert(p.clone(), c);
        Ok(t)
    }

    /// `paths` opened (those not yet), `GRIDS_AT_ONCE` at once: over SMB each open and header read
    /// waits round trips, and a block can meet hundreds of a source's grids (BlueTopo's).
    fn open_all(&self, paths: &[Loc<'_>]) -> Result<()> {
        let todo: Vec<Loc<'_>> = {
            let o = self.open.lock().unwrap();
            paths.iter().copied().filter(|l| !o.files.contains_key(l.path)).collect()
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
                    if let Err(e) = self.tiff_at(todo[i]) {
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

/// The most cells of a grid read at once for a window (a band of its rows): 4 MB of values.
pub static STRIP_CELLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1 << 20);

/// How many of a source's grids are opened, or sampled, at once.
pub const GRIDS_AT_ONCE: usize = 8;

/// A GeoTIFF opened, its header read. (Not on crate::agent::nasread's readers: their four
/// threads, busy, fail a call at once, and the grids are opened `GRIDS_AT_ONCE` at once.)
/// An I/O error in its header's read keeps its kind (a transient one is tried again); a header
/// that isn't a GeoTIFF's is `InvalidData` (never tried again).
fn open_file(p: &NasPath) -> std::io::Result<Tiff> {
    let f = p.open_plain()?;
    IO[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tiff_of(Arc::new(Counted(f)))
}

/// A GeoTIFF's header read from `src` (`open_file`'s errors: store::retry::as_io).
fn tiff_of(src: Arc<dyn RangeRead>) -> std::io::Result<Tiff> {
    Tiff::open(src).map_err(store::retry::as_io)
}

/// How long a source file's open and header read may take before the job fails (an SMB open can
/// stay stuck in the kernel for many minutes; the job's own retry takes over).
pub const OPEN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// `open(p)` on a thread of its own, waited for `deadline` at most: past it, `TimedOut` (the
/// thread stuck in it left behind).
pub fn within<P: ?Sized + ToOwned + 'static, T: Send + 'static>(
    p: &P,
    deadline: std::time::Duration,
    open: fn(&P) -> std::io::Result<T>,
) -> std::io::Result<T>
where
    P::Owned: Send + 'static,
{
    let owned = p.to_owned();
    within_do(deadline, move || open(std::borrow::Borrow::borrow(&owned)))
}

/// A pack opened for positioned reads: this Mac's copy, or the NAS's.
fn plain_local(p: &Path) -> std::io::Result<store::range::PlainFile> {
    store::range::PlainFile::open(p)
}

fn plain_nas(p: &NasPath) -> std::io::Result<store::range::PlainFile> {
    p.open_plain()
}

/// A path whose operations `open_retrying` tries again: a local one's or the NAS's.
pub trait Retried {
    fn retried<T>(&self, waits: &[std::time::Duration], f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T>;
}

impl Retried for Path {
    fn retried<T>(&self, waits: &[std::time::Duration], f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
        store::retry::io(&self.display().to_string(), Some(self), waits, store::retry::transient_or_missing, f)
    }
}

impl Retried for NasPath {
    fn retried<T>(&self, waits: &[std::time::Duration], f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
        self.retry_with(&self.display().to_string(), waits, store::retry::transient_or_missing, f)
    }
}

/// `f()` on a thread of its own, waited for `deadline` at most (`within`'s).
pub fn within_do<T: Send + 'static>(
    deadline: std::time::Duration,
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> std::io::Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("depth-open".into()).spawn(move || {
        let _ = tx.send(f());
    })?;
    rx.recv_timeout(deadline).unwrap_or_else(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("its open took over {} s (the NAS's share stuck?)", deadline.as_secs()),
        ))
    })
}

/// `open(p)`, tried again after each of `waits` while it fails as the share does for a moment
/// (store::retry::transient_or_missing: SMB's moment of "No such file or directory" on a file
/// that's there, EIO, ENXIO, ESTALE…; not a deadline passed, nor a file that isn't a GeoTIFF), each
/// retry logged; past the waits, its last error.
pub fn open_retrying<P: ?Sized + Retried, T>(
    p: &P,
    open: &dyn Fn(&P) -> std::io::Result<T>,
    waits: &[std::time::Duration],
) -> std::io::Result<T> {
    p.retried(waits, || open(p))
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

/// How much finer than the window's pixels an overview must be when its cells don't line up with
/// them (`level_for`): its cells 2^`ASKEW_FINER` to a pixel's side. A cell is counted to the pixel
/// its centre is in, so the cells counted to a pixel are the pixel's moved by at most half a cell:
/// an eighth of a pixel at four cells to a side. (The sources' overviews average their cells and
/// the zoom files' take the nearest, producers/depth/grid.py: a coarse cell is a mean of the
/// grid's, as a lined-up one is.)
pub const ASKEW_FINER: u32 = 2;

/// The image of `t` whose cells are 2^l of image 0's (l ≤ `most`), the largest such: one that
/// lines up with (c0, r0) (image 0's origin on the global grid: a cell is whole pixels of the
/// window's zoom, so each pixel's mean is exact), else one at least `ASKEW_FINER` finer than the
/// pixels (task #433: a grid whose origin lines up with no overview, most of BlueTopo's, was read
/// at full resolution for every window, 85 GB for z3 tile 3/2/3's assembly); (its index, l).
fn level_for(t: &Tiff, most: u32, c0: i64, r0: i64) -> (usize, u32) {
    let im = t.images();
    let (w0, h0) = (im[0].width, im[0].height);
    let mut best = (0usize, 0u32);
    let mut askew = (0usize, 0u32);
    for (i, img) in im.iter().enumerate().skip(1) {
        let r = (f64::from(w0) / f64::from(img.width)).log2().round();
        if !(1.0..=16.0).contains(&r) {
            continue;
        }
        let l = r as u32;
        let f = 1i64 << l;
        if l > most || img.width != w0.div_ceil(f as u32) || img.height != h0.div_ceil(f as u32) {
            continue;
        }
        if c0 % f == 0 && r0 % f == 0 {
            if l > best.1 {
                best = (i, l);
            }
        } else if l + ASKEW_FINER <= most && l > askew.1 {
            askew = (i, l);
        }
    }
    if best.1 >= askew.1 {
        best
    } else {
        askew
    }
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
        let paths: Vec<Loc<'_>> = grids.iter().flat_map(|g| (0..3).filter_map(|k| g.loc(k))).collect();
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
        let made: Vec<Result<Option<Part>>> = if chunk.len() == 1 {
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
        for p in made {
            let Some(p) = p? else { continue };
            let o = out.get_or_insert_with(|| Sample {
                v: vec![f32::NAN; w * h],
                zoom: vec![0; w * h],
                class: vec![0; w * h],
                low: vec![f32::NAN; w * h],
            });
            for j in 0..p.h {
                for i in 0..p.w {
                    let (a, b) = (j * p.w + i, (p.y + j) * w + p.x + i);
                    if p.s.v[a].is_finite() {
                        o.v[b] = p.s.v[a];
                        o.zoom[b] = p.s.zoom[a];
                        o.class[b] = p.s.class[a];
                        o.low[b] = p.s.low[a];
                    }
                }
            }
        }
    }
    Ok(out.filter(|o| o.v.iter().any(|v| v.is_finite())))
}

/// One grid on the window.
/// One grid on the window: on the part of it the grid can reach alone (`Part`), its buffers that
/// part's size. (Task #438: each grid's buffers were the whole window's, ~150 MB a grid at a z6
/// block, eight at once, for grids mostly a sliver of it: thousands a block, and the allocator kept
/// the freed ones, 9 GB at depth-lo 3/2/3.) A pixel's value depends on its own place alone, so the
/// part's are the window's.
fn sample_grid(rd: &Reader, g: &Grid, z: u8, c0: i64, r0: i64, w: usize, h: usize) -> Result<Option<Part>> {
    let t = rd.tiff_at(g.loc(0).context("no file")?)?;
    let (gc, gr) = origin(&t, g.zoom)?;
    let (iw, ih) = (i64::from(t.images()[0].width), i64::from(t.images()[0].height));
    // The window's pixels the grid reaches (zoom z): those covering its cells (finer), or whose
    // bilinear neighbours can be its cells (coarser), a pixel more each side.
    let reach = |o: i64, n: i64| -> (i64, i64) {
        if g.zoom >= z {
            let k = u32::from(g.zoom - z).min(62);
            ((o >> k) - 1, ((o + n) >> k) + 2)
        } else {
            let k = u32::from(z - g.zoom).min(30);
            ((o - 1) << k, (o + n + 1) << k)
        }
    };
    let ((ax0, ax1), (ay0, ay1)) = (reach(gc, iw), reach(gr, ih));
    let (x0, x1) = ((ax0 - c0).max(0), (ax1 - c0).min(w as i64));
    let (y0, y1) = ((ay0 - r0).max(0), (ay1 - r0).min(h as i64));
    if x0 >= x1 || y0 >= y1 {
        return Ok(None);
    }
    let (pw, ph) = ((x1 - x0) as usize, (y1 - y0) as usize);
    Ok(sample_grid_on(rd, g, &t, (gc, gr), z, c0 + x0, r0 + y0, pw, ph)?.map(|s| Part {
        x: x0 as usize,
        y: y0 as usize,
        w: pw,
        h: ph,
        s,
    }))
}

/// A grid's sample of a part of a window: at (`x`, `y`) in it, `w` × `h`.
struct Part {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    s: Sample,
}

/// One grid on the window (`sample_grid`'s, on the part it reaches).
#[allow(clippy::too_many_arguments)]
fn sample_grid_on(
    rd: &Reader,
    g: &Grid,
    t: &Tiff,
    (gc, gr): (i64, i64),
    z: u8,
    c0: i64,
    r0: i64,
    w: usize,
    h: usize,
) -> Result<Option<Sample>> {
    let zt = g.loc(1).map(|l| rd.tiff_at(l)).transpose()?;
    let lt = g.loc(2).map(|l| rd.tiff_at(l)).transpose()?;
    for x in zt.iter().chain(lt.iter()) {
        ensure!(
            origin(x, g.zoom)? == (gc, gr)
                && x.images()[0].width == t.images()[0].width
                && x.images()[0].height == t.images()[0].height,
            "its zoom or low-water file isn't on its own grid"
        );
    }
    // (A zoom or low-water file is read at the level its grid is read at, its cells the same:
    // at another level the band's coordinates would mean other cells.)
    let same_level = |x: &Tiff, what: &str, lv: usize| -> Result<usize> {
        let im = &t.images()[lv];
        match x.images().get(lv) {
            Some(xi) if xi.width == im.width && xi.height == im.height => Ok(lv),
            _ => bail!("its {what} file lacks its grid's overview {} ({} × {} cells)", lv, im.width, im.height),
        }
    };
    let mut s =
        Sample { v: vec![f32::NAN; w * h], zoom: vec![0; w * h], class: vec![0; w * h], low: vec![f32::NAN; w * h] };
    if g.zoom >= z {
        let k = u32::from(g.zoom - z);
        if k > 24 {
            bail!("zoom {} sampled at {z}", g.zoom);
        }
        let (lv, l) = level_for(&t, k, gc, gr);
        let zlv = zt.as_ref().map(|x| same_level(x, "zoom", lv)).transpose()?;
        let llv = lt.as_ref().map(|x| same_level(x, "low-water", lv)).transpose()?;
        // (The level's cells covering the window, in its own pixels: the first at or before the
        // window's first cell, the last at or after its last, so a level askew of the pixels,
        // `level_for`, still covers them whole.)
        let (down, up) = (|v: i64| v >> l, |v: i64| (v + (1i64 << l) - 1) >> l);
        let (x0, y0) = (down((c0 << k) - gc), down((r0 << k) - gr));
        let (x1, y1) = (up(((c0 + w as i64) << k) - gc), up(((r0 + h as i64) << k) - gr));
        // (The window pixel a level cell counts to: the one its centre is in, by its place on the
        // global grid. A level lined up with the pixels puts every cell whole in one.)
        let half = (1i64 << l) >> 1;
        let pixel = |origin: i64, cell: i64, from: i64| ((origin + (cell << l) + half) >> k) - from;
        let mut sum = vec![0f64; w * h];
        let mut n = vec![0u32; w * h];
        let mut lsum = vec![0f64; w * h];
        let mut ln = vec![0u32; w * h];
        // Read a band of rows at a time, `STRIP_CELLS` cells at most (task #438: a grid without the
        // overview a window needs is read at full resolution, 240 MB a window at a z6 block, eight
        // at once): the same cells in the same order, so the same sums. A row at least, and a band
        // never past i64 (task #433: a setting past it, over a part a cell wide, made a negative
        // band and a loop that never ended).
        let rows = (STRIP_CELLS.load(std::sync::atomic::Ordering::Relaxed) / (x1 - x0).max(1) as usize)
            .clamp(1, i64::MAX as usize) as i64;
        let mut any = false;
        let mut ys = y0;
        while ys < y1 {
            let ye = ys.saturating_add(rows).min(y1);
            let band = (ys, ye);
            ys = ye;
            let Some((v, a0, b0, rw)) = read(t, lv, x0, band.0, x1, band.1)? else { continue };
            any = true;
            let zv = match (&zt, zlv) {
                (Some(zt), Some(zlv)) => read(zt, zlv, x0, band.0, x1, band.1)?.map(|r| r.0),
                _ => None,
            };
            let lw = match (&lt, llv) {
                (Some(lt), Some(llv)) => read(lt, llv, x0, band.0, x1, band.1)?.map(|r| r.0),
                _ => None,
            };
            let rh = v.len() / rw.max(1);
            for j in 0..rh {
                let oy = pixel(gr, b0 + j as i64, r0);
                if !(0..h as i64).contains(&oy) {
                    continue;
                }
                for i in 0..rw {
                    let ox = pixel(gc, a0 + i as i64, c0);
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
        }
        if !any {
            return Ok(None);
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
    use std::path::{Path, PathBuf};

    /// A reader's grids all closed as it's dropped, several at once, every one exactly once.
    #[test]
    fn a_reader_closes_every_grid_at_once() {
        let live = Arc::new(());
        for n in [0, 1, 2, 7, 100] {
            let files: Vec<Arc<()>> = (0..n).map(|_| live.clone()).collect();
            assert_eq!(Arc::strong_count(&live), n + 1);
            close_all(files, GRIDS_AT_ONCE);
            assert_eq!(Arc::strong_count(&live), 1, "{n}");
        }
        // (At once: the closes overlap, each a round trip on the NAS. Each close waits until a
        // second one is under way, so they can only all end if several run together; one at a
        // time, the first waits out the watchdog and the rest don't wait.)
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
        static NOW: AtomicUsize = AtomicUsize::new(0);
        static MOST: AtomicUsize = AtomicUsize::new(0);
        static ALONE: AtomicBool = AtomicBool::new(false);
        struct Slow;
        impl Drop for Slow {
            fn drop(&mut self) {
                let n = NOW.fetch_add(1, SeqCst) + 1;
                MOST.fetch_max(n, SeqCst);
                let t = std::time::Instant::now();
                while MOST.load(SeqCst) < 2 && !ALONE.load(SeqCst) {
                    if t.elapsed() > store::testkit::WATCHDOG {
                        ALONE.store(true, SeqCst);
                    }
                    std::thread::yield_now();
                }
                NOW.fetch_sub(1, SeqCst);
            }
        }
        close_all((0..16).map(|_| Slow).collect(), CLOSES_AT_ONCE);
        assert!(MOST.load(SeqCst) >= 2, "closed one at a time");
    }

    /// A depth grid file of zoom `zoom` at global pixel (`c`, `r`), `w` × `h`, with overviews of
    /// 2× and 4×, values `f(col, row)` (global pixels), NaN where it says.
    pub(crate) fn grid_file(path: &Path, zoom: u8, c: i64, r: i64, w: u32, h: u32, f: &dyn Fn(i64, i64) -> f32) {
        grid_file_levels(path, zoom, c, r, w, h, 4, f);
    }

    /// `grid_file` with overviews down to `coarsest` cells (1: none; 4: the usual two).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn grid_file_levels(
        path: &Path,
        zoom: u8,
        c: i64,
        r: i64,
        w: u32,
        h: u32,
        coarsest: u32,
        f: &dyn Fn(i64, i64) -> f32,
    ) {
        let p = px(zoom);
        let gt = [-HALF + c as f64 * p, p, 0.0, HALF - r as f64 * p, 0.0, -p];
        let mut levels = Vec::new();
        let mut f_ = 1u32;
        while f_ <= coarsest {
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

    /// A measure on real data, reading the NAS's grid headers alone: `SCENIC_DEPTH_LEVELS="<root>
    /// 3/x/y <zoom> <sample>"` opens up to `sample` grids of each source meeting z3 tile x/y and says,
    /// per source, which overview level a window at `zoom` would read from (`level_for`), how many
    /// grids fall back to their full image, and the bytes those levels hold (task #433: a z3 tile's
    /// assembly read 85 GB).
    #[test]
    fn measure_the_levels_a_zoom_reads() {
        let Ok(m) = std::env::var("SCENIC_DEPTH_LEVELS") else { return };
        let a: Vec<&str> = m.split_whitespace().collect();
        let root = Path::new(a[0]);
        let (x, y) = a[1].strip_prefix("3/").unwrap().split_once('/').unwrap();
        let (x, y): (u32, u32) = (x.parse().unwrap(), y.parse().unwrap());
        let zoom: u8 = a[2].parse().unwrap();
        let sample: usize = a[3].parse().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let out = crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(root), scratch.path()).unwrap();
        let srcs = declared(out.root(), &out.manifest).unwrap();
        let n = 1u32 << (zoom - 3);
        let b = window_box(zoom, i64::from(x * n) * 256, i64::from(y * n) * 256, n as usize * 256, n as usize * 256);
        let rd = Reader::keeping(64);
        for s in &srcs {
            let grids: Vec<&Grid> = s.meeting(b).collect();
            if grids.is_empty() {
                continue;
            }
            let mut by_level: std::collections::BTreeMap<(u8, u32, u32), (usize, u64, u64)> = Default::default();
            let step = (grids.len() / sample.max(1)).max(1);
            let mut opened = 0usize;
            for g in grids.iter().step_by(step).take(sample) {
                let Ok(t) = rd.tiff(&g.file) else { continue };
                opened += 1;
                let (gc, gr) = origin(&t, g.zoom).unwrap();
                let k = u32::from(g.zoom.saturating_sub(zoom));
                let (lv, l) = level_for(&t, k, gc, gr);
                let im = &t.images()[lv];
                let full = &t.images()[0];
                let e = by_level.entry((g.zoom, k, l)).or_default();
                e.0 += 1;
                e.1 += u64::from(im.width) * u64::from(im.height) * 4;
                e.2 += u64::from(full.width) * u64::from(full.height) * 4;
            }
            println!("{}: {} grids meet 3/{x}/{y}, {opened} opened (every {step}th)", s.id, grids.len());
            for ((gz, k, l), (n, bytes, full)) in &by_level {
                println!(
                    "  grid zoom {gz}, {k} zooms finer than z{zoom}: level {l} read by {n} grids, {:.1} MB at that level (their full images {:.1} MB){}",
                    *bytes as f64 / 1e6,
                    *full as f64 / 1e6,
                    if *l < *k { "  <- finer than the zoom: its coarsest level that lines up" } else { "" }
                );
            }
        }
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
            file: store::nasfs::NasPath::local_for_test(p.clone()),
            zoom_file: None,
            low_file: None,
            packed: None,
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

    /// Task #433: a grid whose origin lines up with no overview is sampled from the coarsest
    /// overview at least `ASKEW_FINER` zooms finer than the window (`level_for`), a lined-up one
    /// taken when it's at least as coarse, the full image for a window under two zooms coarser;
    /// each pixel's mean is the plane's within half an overview cell (an eighth of a pixel), and
    /// the grid's edge in pixels moves by at most one overview cell.
    #[test]
    fn an_askew_grid_is_sampled_from_a_coarse_overview_near_the_plane() {
        let d = tempfile::tempdir().unwrap();
        let (zoom, c, r) = (12u8, 1200 * 256i64, 1500 * 256i64);
        let plane = |x: i64, y: i64| -((x - c) as f32) * 0.5 - (y - r) as f32 * 0.25 - 10.0;
        let rd = Reader::default();
        // (Three images each: the full one and overviews of 2 and 4 cells.)
        let at = |dx: i64, dy: i64, name: &str| {
            let p = d.path().join(name);
            grid_file(&p, zoom, c + dx, r + dy, 1024, 512, &plane);
            p
        };
        let askew = at(3, 1, "askew.tif");
        let t = rd.tiff(&store::nasfs::NasPath::local_for_test(&askew)).unwrap();
        assert_eq!(level_for(&t, 4, c + 3, r + 1), (2, 2), "four zooms coarser: the 4-cell overview");
        assert_eq!(level_for(&t, 3, c + 3, r + 1), (1, 1), "three: the 2-cell one");
        assert_eq!(level_for(&t, 1, c + 3, r + 1), (0, 0), "under two: the full image");
        assert_eq!(level_for(&t, 0, c + 3, r + 1), (0, 0));
        let even = at(2, 2, "even.tif");
        let t2 = rd.tiff(&store::nasfs::NasPath::local_for_test(&even)).unwrap();
        assert_eq!(
            level_for(&t2, 4, c + 2, r + 2),
            (2, 2),
            "lined up with the 2-cell one alone: the coarser 4-cell one"
        );
        assert_eq!(level_for(&t2, 3, c + 2, r + 2), (1, 1), "as coarse: the lined-up one");
        let lined = at(4, 0, "lined.tif");
        let t4 = rd.tiff(&store::nasfs::NasPath::local_for_test(&lined)).unwrap();
        assert_eq!(level_for(&t4, 4, c + 4, r), (2, 2));
        assert_eq!(
            level_for(&t4, 2, c + 4, r),
            (2, 2),
            "lined up: a window of its zoom reads it whatever the finer rule"
        );
        // Four zooms out over the askew grid: each pixel (16 × 16 cells) the mean of the 4-cell
        // overview's cells whose centres are in it: the plane at the pixel's centre, moved by at
        // most half a cell (2 cells: 1.5 in the plane's units), and not exact everywhere.
        let s = source(vec![Grid {
            bbox: bbox(zoom, c + 3, r + 1, 1024, 512),
            zoom,
            file: store::nasfs::NasPath::local_for_test(askew.clone()),
            zoom_file: None,
            low_file: None,
            packed: None,
        }]);
        let (px0, py0) = ((c + 3) >> 4, (r + 1) >> 4);
        let got = sample(&rd, &s, 8, px0 - 1, py0 - 1, 70, 38).unwrap().unwrap();
        let (mut off, mut worst, mut seen) = (0usize, 0f32, 0usize);
        for (o, v) in got.v.iter().enumerate() {
            let (px, py) = (px0 - 1 + (o % 70) as i64, py0 - 1 + (o / 70) as i64);
            // (The grid's cells c+3..c+1027 by r+1..r+513: a pixel wholly in them, one past them by a
            // cell or more, or an edge pixel between, which may or may not have a value.)
            let inside = px * 16 >= c + 3 && (px + 1) * 16 <= c + 1027 && py * 16 >= r + 1 && (py + 1) * 16 <= r + 513;
            let past = (px + 1) * 16 + 4 <= c + 3
                || px * 16 >= c + 1027 + 4
                || (py + 1) * 16 + 4 <= r + 1
                || py * 16 >= r + 513 + 4;
            if past {
                assert!(v.is_nan(), "{o}: a cell or more past the grid: {v}");
                continue;
            }
            if !inside {
                continue;
            }
            assert!(v.is_finite(), "{o}: inside the grid");
            seen += 1;
            let want = plane(px * 16, py * 16) - 0.5 * 7.5 - 0.25 * 7.5;
            let e = (v - want).abs();
            assert!(e <= 1.5 + 1e-3, "{o}: {v} against {want}");
            worst = worst.max(e);
            if e > 1e-3 {
                off += 1;
            }
            assert_eq!(got.zoom[o], zoom);
        }
        assert!(
            seen > 1000 && off > 0 && worst > 0.1,
            "askew: the means move ({seen} pixels, {off} off, worst {worst})"
        );
    }

    /// Task #433 review: a zoom file is read at the overview its grid is read at (their cells the
    /// same), an askew grid's too: where its zoom is below the grid's every pixel is interpolated.
    #[test]
    fn a_zoom_file_is_read_at_its_grids_overview() {
        let d = tempfile::tempdir().unwrap();
        let (zoom, c, r) = (12u8, 1200 * 256i64, 1500 * 256i64);
        let p = d.path().join("g.tif");
        grid_file(&p, zoom, c + 3, r + 1, 256, 128, &|x, y| -((x - c) as f32) * 0.1 - (y - r) as f32 * 0.1 - 5.0);
        let zp = d.path().join("g.zoom.tif");
        grid_file(&zp, zoom, c + 3, r + 1, 256, 128, &|x, _| if x < c + 131 { 11.0 } else { 12.0 });
        let s = source(vec![Grid {
            bbox: bbox(zoom, c + 3, r + 1, 256, 128),
            zoom,
            file: store::nasfs::NasPath::local_for_test(p),
            zoom_file: Some(store::nasfs::NasPath::local_for_test(zp)),
            low_file: None,
            packed: None,
        }]);
        let rd = Reader::default();
        let got = sample(&rd, &s, 8, c >> 4, r >> 4, 18, 10).unwrap().unwrap();
        let (mut lo, mut hi) = (0, 0);
        for (o, v) in got.v.iter().enumerate() {
            if v.is_nan() {
                continue;
            }
            let px = (c >> 4) + (o % 18) as i64;
            // (Pixels wholly west of cell c+131 by a cell or more have zoom 11, east 12.)
            if (px + 1) * 16 + 4 <= c + 131 {
                assert_eq!((got.zoom[o], got.class[o]), (11, super::super::C_INTERP), "{o}");
                lo += 1;
            } else if px * 16 >= c + 131 + 4 {
                assert_eq!((got.zoom[o], got.class[o]), (12, super::super::C_MEASURED), "{o}");
                hi += 1;
            }
        }
        assert!(lo > 0 && hi > 0, "{lo} {hi}");
        // A zoom file with fewer overviews than its grid: the window that reads the grid's 4-cell
        // overview fails, saying so (one reading the full image is fine).
        let zp = d.path().join("g.short.zoom.tif");
        grid_file_levels(&zp, zoom, c + 3, r + 1, 256, 128, 2, &|_, _| 12.0);
        let s = source(vec![Grid {
            bbox: bbox(zoom, c + 3, r + 1, 256, 128),
            zoom,
            file: store::nasfs::NasPath::local_for_test(d.path().join("g.tif")),
            zoom_file: Some(store::nasfs::NasPath::local_for_test(zp)),
            low_file: None,
            packed: None,
        }]);
        let e = sample(&rd, &s, 8, c >> 4, r >> 4, 18, 10).unwrap_err();
        assert!(format!("{e:#}").contains("lacks its grid's overview"), "{e:#}");
        assert!(sample(&rd, &s, 12, c, r, 40, 40).unwrap().is_some(), "the full image: fine");
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
                grid_file(&p, zoom, gc, gr, 64, 64, &|x, y| {
                    -(i as f32) - ((x - gc) as f32) * 0.01 - (y - gr) as f32 * 0.02
                });
                Grid {
                    bbox: bbox(zoom, gc, gr, 64, 64),
                    zoom,
                    file: store::nasfs::NasPath::local_for_test(p),
                    zoom_file: None,
                    low_file: None,
                    packed: None,
                }
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
            assert_eq!(
                a.v.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                b.v.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
            assert!(few.open_count() <= 3, "{}", few.open_count());
        }
        assert_eq!(all.open_count(), 7);
    }

    /// Task #452: grids packed (their files' bytes in a pack, read by range) sample bit for bit as
    /// loose, each pack opened once by a reader, and a reader keeping few lets packs go too.
    #[test]
    fn packed_grids_sample_as_loose_and_each_pack_opens_once() {
        let d = tempfile::tempdir().unwrap();
        let (zoom, c, r) = (12u8, 1200 * 256i64, 1500 * 256i64);
        let mut loose = Vec::new();
        let mut packed = Vec::new();
        let mut packs: Vec<Vec<u8>> = vec![vec![7u8; 512], vec![9u8; 1024]];
        for i in 0..7i64 {
            let (gc, gr) = (c + 40 * i, r);
            let (p, l) = (d.path().join(format!("g{i}.tif")), d.path().join(format!("g{i}.low.tif")));
            grid_file(&p, zoom, gc, gr, 64, 64, &|x, y| {
                -(i as f32) - ((x - gc) as f32) * 0.01 - (y - gr) as f32 * 0.02
            });
            grid_file(&l, zoom, gc, gr, 64, 64, &|x, _| 0.5 + (x - gc) as f32 * 0.001);
            let g = Grid {
                bbox: bbox(zoom, gc, gr, 64, 64),
                zoom,
                file: store::nasfs::NasPath::local_for_test(p.clone()),
                zoom_file: None,
                low_file: Some(store::nasfs::NasPath::local_for_test(l.clone())),
                packed: None,
            };
            loose.push(g.clone());
            // (The first four in one pack, the rest in another, each after some other bytes.)
            let k = usize::from(i >= 4);
            let mut at = [None; 3];
            for (j, f) in [(0, &p), (2, &l)] {
                let b = std::fs::read(f).unwrap();
                at[j] = Some((packs[k].len() as u64, b.len() as u64));
                packs[k].extend_from_slice(&b);
                packs[k].extend_from_slice(&[0u8; 100]);
            }
            let pack = Arc::new(store::nasfs::NasPath::local_for_test(d.path().join(format!("pack{k}.tar"))));
            packed.push(Grid {
                file: store::nasfs::NasPath::local_for_test(d.path().join(format!("packed/g{i}.tif"))),
                low_file: Some(store::nasfs::NasPath::local_for_test(d.path().join(format!("packed/g{i}.low.tif")))),
                packed: Some(Packed { pack, at }),
                ..g
            });
        }
        for (k, b) in packs.iter().enumerate() {
            std::fs::write(d.path().join(format!("pack{k}.tar")), b).unwrap();
        }
        let (a, b) = (source(loose), source(packed));
        for keep in [100, 1] {
            let (ra, rb) = (Reader::keeping(keep), Reader::keeping(keep));
            let mut wins = vec![(c, r, 320usize, 64usize)];
            for i in 0..7i64 {
                wins.push((c + 40 * i + 10, r + 5, 30, 30));
            }
            for (x, y, w, h) in wins {
                let sa = sample(&ra, &a, zoom, x, y, w, h).unwrap().unwrap();
                let sb = sample(&rb, &b, zoom, x, y, w, h).unwrap().unwrap();
                let bits = |v: &[f32]| v.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
                assert_eq!(bits(&sa.v), bits(&sb.v));
                assert_eq!(bits(&sa.low), bits(&sb.low));
                assert_eq!((sa.zoom, sa.class), (sb.zoom, sb.class));
                assert!(rb.open.lock().unwrap().packs.len() <= keep.min(2));
            }
            if keep == 100 {
                assert_eq!(rb.open.lock().unwrap().packs.len(), 2);
                assert_eq!(rb.open_count(), 14);
            }
        }
        // Task #551: read from whole local copies of the packs, the same, the copies made once.
        let cache = tempfile::tempdir().unwrap();
        let local =
            Arc::new(crate::localread::Local::new(&store::nasfs::NasRoot::local_for_test(d.path()), cache.path(), 0));
        let (ra, rl) = (Reader::keeping(100), Reader::keeping(100).with_local(Some(local)));
        let sa = sample(&ra, &a, zoom, c, r, 320, 64).unwrap().unwrap();
        let sl = sample(&rl, &b, zoom, c, r, 320, 64).unwrap().unwrap();
        assert_eq!(
            sa.v.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            sl.v.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!((sa.zoom, sa.class), (sl.zoom, sl.class));
        for k in 0..2 {
            assert_eq!(std::fs::read(cache.path().join(format!("blobs/pack{k}.tar"))).unwrap(), packs[k]);
            store::cachefile::release(&cache.path().join(format!("blobs/pack{k}.tar")));
        }
        // A range past its pack's end is an error, never a short read.
        let mut bad = b.grids[0].clone();
        bad.packed.as_mut().unwrap().at[0] = Some((10, 1 << 30));
        bad.file = store::nasfs::NasPath::local_for_test(d.path().join("packed/bad.tif"));
        assert!(sample(&Reader::keeping(4), &source(vec![bad]), zoom, c, r, 64, 64).is_err());
    }

    /// Task #452: a packed index's grids name their pack and ranges, and the source's pin is the
    /// same as loose (a repack builds nothing again).
    #[test]
    fn a_packed_index_is_read_and_keeps_its_pin() {
        let decl = "name = \"N\"\ncredit = \"C\"\nlicence = \"CC0-1.0\"\nterritory = \"world\"\n[data]\nrole = \"depth\"\nkind = \"data\"\n\
                    format = \"depth-grid\"\nindex = \"i.geojson\"\n[data.params]\ncode = 3\nwater = \"sea\"\nlabel = \"L\"\ndatum = \"MSL\"\n";
        let idx = |extra: &str| {
            format!(
                r#"{{"type":"FeatureCollection","features":[{{"type":"Feature","properties":{{"url":"https://a.org/x","file":"a.tif","low_file":"a.low.tif","zoom":8,"hash":"0123"{extra}}},"geometry":{{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}}}}]}}"#
            )
        };
        let src = |extra: &str| {
            let d = crate::inputs::sources::parse(decl.as_bytes()).unwrap();
            let index = crate::inputs::sources::parse_index(idx(extra).as_bytes()).unwrap();
            let s = crate::inputs::sources::Source { id: "s".into(), data: d.data, keyed: "k".into(), index };
            Source::from_declared(&store::nasfs::NasRoot::local_for_test(Path::new("/r")), &s).unwrap()
        };
        let loose = src("");
        let packed = src(r#","pack":"6-1-2.00ff.tar","at":{"file":[512,100],"low_file":[1024,50]}"#);
        assert_eq!(loose.pin, packed.pin);
        assert_eq!(src(r#","hash":"9999""#).pin == loose.pin, false, "the grid's own change changes the pin");
        let g = &packed.grids[0];
        assert_eq!(g.file, store::nasfs::NasPath::local_for_test(Path::new("/r/sources/fetched/s/a.tif")));
        let p = g.packed.as_ref().unwrap();
        assert_eq!(p.pack.unchecked_path(), Path::new("/r/sources/fetched/s/6-1-2.00ff.tar"));
        assert_eq!(p.at, [Some((512, 100)), None, Some((1024, 50))]);
        assert!(loose.grids[0].packed.is_none());
        let l = g.loc(2).unwrap();
        assert_eq!(
            (l.path.unchecked_path(), l.pack.map(|(p, o, n)| (p.unchecked_path(), o, n))),
            (PathBuf::from("/r/sources/fetched/s/a.low.tif"), Some((p.pack.unchecked_path(), 1024, 50)))
        );
        assert!(g.loc(1).is_none());
    }

    /// Task #438: a grid sampled on the part of a window it reaches gives the window what sampling
    /// it on the whole window does, bit for bit: finer, as fine and coarser than the window, the window far larger
    /// than the grid, its edges included (a coarser grid's bilinear reach past its cells).
    #[test]
    fn a_grid_on_the_part_it_reaches_is_as_on_the_whole_window() {
        let d = tempfile::tempdir().unwrap();
        let (zoom, c, r) = (12u8, 1200 * 256i64, 1500 * 256i64);
        let p = d.path().join("g.tif");
        grid_file(&p, zoom, c, r, 40, 24, &|x, y| {
            if (x * 7 + y) % 11 == 0 {
                f32::NAN
            } else {
                -((x - c) as f32) - (y - r) as f32 * 0.3 - 5.0
            }
        });
        let s = source(vec![Grid {
            bbox: bbox(zoom, c, r, 40, 24),
            zoom,
            file: store::nasfs::NasPath::local_for_test(p),
            zoom_file: None,
            low_file: None,
            packed: None,
        }]);
        let rd = Reader::default();
        for z in [10u8, 12, 14] {
            let sc = |v: i64| if z >= zoom { v << (z - zoom) } else { v >> (zoom - z) };
            // A window four times the grid's size each way, the grid inside it off centre.
            let (gw, gh) = (sc(40).max(1), sc(24).max(1));
            let (wc, wr, w, h) = (sc(c) - gw - 3, sc(r) - gh / 2 - 5, (4 * gw) as usize, (3 * gh) as usize);
            let big = sample(&rd, &s, z, wc, wr, w, h).unwrap().unwrap();
            // As before: the grid on the whole window.
            let g = &s.grids[0];
            let t = rd.tiff(&g.file).unwrap();
            let whole = sample_grid_on(&rd, g, &t, origin(&t, g.zoom).unwrap(), z, wc, wr, w, h).unwrap().unwrap();
            for o in 0..w * h {
                assert_eq!(
                    (big.v[o].to_bits(), big.zoom[o], big.class[o], big.low[o].to_bits()),
                    (whole.v[o].to_bits(), whole.zoom[o], whole.class[o], whole.low[o].to_bits()),
                    "z{z} ({}, {})",
                    o % w,
                    o / w
                );
            }
            assert!(big.v.iter().any(|v| v.is_finite()));
        }
    }

    /// Task #438: a finer grid read a band of rows at a time gives the sample read whole, bit for
    /// bit (the same cells summed in the same order), bands of one row, a few, or the window.
    #[test]
    fn a_grid_read_in_bands_is_as_read_whole() {
        use std::sync::atomic::Ordering;
        let d = tempfile::tempdir().unwrap();
        let (zoom, c, r) = (12u8, 1200 * 256i64, 1500 * 256i64);
        let p = d.path().join("g.tif");
        grid_file(&p, zoom, c + 3, r + 1, 300, 200, &|x, y| {
            if (x * 5 + y * 3) % 13 == 0 {
                f32::NAN
            } else {
                -((x - c) as f32) * 0.37 - (y - r) as f32 * 0.11 - 2.0
            }
        });
        let s = source(vec![Grid {
            bbox: bbox(zoom, c + 3, r + 1, 300, 200),
            zoom,
            file: store::nasfs::NasPath::local_for_test(p),
            zoom_file: None,
            low_file: None,
            packed: None,
        }]);
        let bits = |x: &Sample| {
            x.v.iter().map(|v| v.to_bits()).chain(x.zoom.iter().map(|&z| u32::from(z))).collect::<Vec<_>>()
        };
        let mut got = Vec::new();
        // (Bands are a global setting: the others' samples are the same whatever it is.)
        for cells in [usize::MAX, 1, 700, 5000] {
            STRIP_CELLS.store(cells, Ordering::Relaxed);
            let rd = Reader::default();
            got.push((
                bits(&sample(&rd, &s, 10, c / 4 - 4, r / 4 - 4, 90, 60).unwrap().unwrap()),
                bits(&sample(&rd, &s, 12, c - 10, r - 10, 330, 230).unwrap().unwrap()),
            ));
        }
        STRIP_CELLS.store(1 << 20, Ordering::Relaxed);
        assert!(got.iter().all(|g| *g == got[0]));
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
                Grid {
                    bbox: bbox(zoom, gc, gr, 320, 256),
                    zoom,
                    file: store::nasfs::NasPath::local_for_test(p),
                    zoom_file: None,
                    low_file: None,
                    packed: None,
                }
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
        assert!(store::retry::transient_or_missing(&io::Error::from_raw_os_error(libc::EIO)));
        fn stuck(_: &Path) -> io::Result<u8> {
            std::thread::sleep(std::time::Duration::from_secs(30));
            Ok(1)
        }
        let t0 = std::time::Instant::now();
        let e = within(Path::new("x"), std::time::Duration::from_millis(50), stuck).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(!store::retry::transient_or_missing(&e));
        store::testkit::ended_before(t0, std::time::Duration::from_secs(30), "the stuck open ends at its deadline");
        // A file that isn't a GeoTIFF: InvalidData, not tried again.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("n.tif"), b"nope").unwrap();
        assert!(!store::retry::transient_or_missing(
            &open_file(&store::nasfs::NasPath::local_for_test(&d.path().join("n.tif"))).map(|_| ()).unwrap_err()
        ));
        assert_eq!(
            open_file(&store::nasfs::NasPath::local_for_test(&d.path().join("missing.tif")))
                .map(|_| ())
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
