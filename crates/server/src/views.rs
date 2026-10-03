//! Typed views of the per-area files (docs/formats.md): sectioned files read from the Mac's mirror
//! (memory-mapped) or from the NAS (never mapped: read through the I/O pool, a page at a time for
//! the few records a request needs, or whole for what a query scans; see `pages`).

use crate::pages;
use anyhow::{bail, ensure, Context, Result};
use bytemuck::Pod;
use memmap2::Mmap;
use roadcore::packs::{Climb, Here, PSample, Part, RailRel, RoadRec};
use roadcore::scenic::ch;
use roadcore::WayRec;
use std::borrow::Cow;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Bytes of one section: a slice of a local file's map, or aligned memory read from the NAS.
#[derive(Clone)]
pub enum Blob {
    Map(Arc<Mmap>, usize, usize),
    /// 8-byte aligned storage and the byte length used.
    Own(Arc<Vec<u64>>, usize),
}

impl Blob {
    pub fn from_vec(v: Vec<u8>) -> Blob {
        let n = v.len();
        let mut words = vec![0u64; n.div_ceil(8)];
        bytemuck::cast_slice_mut::<u64, u8>(&mut words)[..n].copy_from_slice(&v);
        Blob::Own(Arc::new(words), n)
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Blob::Map(m, o, l) => &m[*o..*o + *l],
            Blob::Own(w, l) => &bytemuck::cast_slice::<u64, u8>(w)[..*l],
        }
    }

    /// The bytes as records (sections are 64-byte aligned, owned memory 8-byte aligned).
    pub fn cast<T: Pod>(&self) -> &[T] {
        bytemuck::try_cast_slice(self.bytes()).unwrap_or(&[])
    }
}

/// A local map as a range source (for readers that take one, e.g. PMTiles).
pub struct MapRange(pub Arc<Mmap>);

impl store::range::RangeRead for MapRange {
    fn len(&self) -> Result<u64, store::iopool::IoError> {
        Ok(self.0.len() as u64)
    }
    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::iopool::IoError> {
        let end = off.checked_add(len as u64).filter(|&e| e <= self.0.len() as u64);
        match end {
            Some(e) => Ok(self.0[off as usize..e as usize].to_vec()),
            None => Err(store::iopool::IoError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read past the end"))),
        }
    }
}

/// A file on the NAS, read through the pool. Its handle (and length: content-named files never
/// change) is opened once and kept; a read that fails with an I/O error (a handle gone stale after
/// sleep or a reconnect) reopens it and tries again. Reads go in pieces of at most `PIECE`, each
/// its own pool operation, so a big read on a busy NAS doesn't overrun the pool's timeout (which
/// would mark the NAS offline).
pub struct RemoteFile {
    pub path: PathBuf,
    pub pool: Arc<store::iopool::IoPool>,
    /// Unique per instance: its pages' key in `pages`.
    pub id: u64,
    file: Mutex<Option<(Arc<std::fs::File>, u64)>>,
}

const PIECE: usize = 1 << 20;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl RemoteFile {
    pub fn new(path: PathBuf, pool: Arc<store::iopool::IoPool>) -> RemoteFile {
        RemoteFile { path, pool, id: NEXT_ID.fetch_add(1, Ordering::Relaxed), file: Mutex::new(None) }
    }

    fn handle(&self, fresh: bool) -> Result<(Arc<std::fs::File>, u64), store::iopool::IoError> {
        let mut g = self.file.lock().unwrap();
        if fresh {
            *g = None;
        }
        if let Some(h) = g.as_ref() {
            return Ok(h.clone());
        }
        let h = self.pool.open_len(&self.path)?;
        *g = Some(h.clone());
        Ok(h)
    }

    fn read_piece(&self, off: u64, len: usize) -> Result<Vec<u8>, store::iopool::IoError> {
        let (f, _) = self.handle(false)?;
        match self.pool.read_at(&f, off, len) {
            Err(store::iopool::IoError::Io(_)) => {
                let (f, _) = self.handle(true)?;
                self.pool.read_at(&f, off, len)
            }
            Err(e) => {
                // A handle that hung (sleep, a reconnect) isn't used again: the next read reopens.
                *self.file.lock().unwrap() = None;
                Err(e)
            }
            r => r,
        }
    }

    pub fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::iopool::IoError> {
        if len <= PIECE {
            return self.read_piece(off, len);
        }
        let mut v = Vec::with_capacity(len);
        while v.len() < len {
            let n = (len - v.len()).min(PIECE);
            v.extend_from_slice(&self.read_piece(off + v.len() as u64, n)?);
        }
        Ok(v)
    }

    pub fn len(&self) -> Result<u64, store::iopool::IoError> {
        Ok(self.handle(false)?.1)
    }

    pub fn read_all(&self) -> Result<Vec<u8>, store::iopool::IoError> {
        let n = self.len()? as usize;
        self.read_at(0, n)
    }
}

impl store::range::RangeRead for RemoteFile {
    fn len(&self) -> Result<u64, store::iopool::IoError> {
        RemoteFile::len(self)
    }
    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::iopool::IoError> {
        RemoteFile::read_at(self, off, len)
    }
}

/// Where a file lives.
#[derive(Clone)]
pub enum Src {
    Local(Arc<Mmap>),
    /// On the NAS: reads go through the pool.
    Remote(Arc<RemoteFile>),
}

/// A sectioned file (RDSECT v1): its table, and its sections read on first use.
pub struct SectView {
    pub meta: serde_json::Value,
    table: HashMap<String, (u64, u64)>,
    src: Src,
    loaded: Mutex<HashMap<String, Blob>>,
}

impl SectView {
    /// Parse the header and table: from the mapped file, or with two reads from the NAS.
    pub fn open(src: Src) -> Result<SectView> {
        let head: Vec<u8> = match &src {
            Src::Local(m) => m[..m.len().min(1 << 16)].to_vec(),
            Src::Remote(r) => {
                let len = r.len()?;
                r.read_at(0, (len as usize).min(1 << 16))?
            }
        };
        ensure!(head.len() >= 28 && &head[..8] == b"RDSECT01", "not a sectioned file");
        let version = u32::from_le_bytes(head[8..12].try_into()?);
        ensure!(version == 1, "sectioned file version {version}");
        let count = u32::from_le_bytes(head[12..16].try_into()?) as usize;
        let table_off = u64::from_le_bytes(head[16..24].try_into()?);
        let mlen = u32::from_le_bytes(head[24..28].try_into()?) as usize;
        let meta: serde_json::Value = if 28 + mlen <= head.len() {
            serde_json::from_slice(&head[28..28 + mlen])?
        } else {
            match &src {
                Src::Local(m) => serde_json::from_slice(&m[28..28 + mlen])?,
                Src::Remote(r) => serde_json::from_slice(&r.read_at(28, mlen)?)?,
            }
        };
        let tbytes: Vec<u8> = match &src {
            Src::Local(m) => {
                let a = table_off as usize;
                ensure!(a + count * 48 <= m.len(), "section table out of bounds");
                m[a..a + count * 48].to_vec()
            }
            Src::Remote(r) => r.read_at(table_off, count * 48)?,
        };
        let mut table = HashMap::new();
        for e in tbytes.chunks_exact(48) {
            let name = String::from_utf8_lossy(&e[..24]).trim_end_matches('\0').to_string();
            table.insert(name, (u64::from_le_bytes(e[24..32].try_into()?), u64::from_le_bytes(e[32..40].try_into()?)));
        }
        Ok(SectView { meta, table, src, loaded: Mutex::new(HashMap::new()) })
    }

    pub fn has(&self, name: &str) -> bool {
        self.table.contains_key(name)
    }

    /// Read from the NAS (not a mapped local copy).
    pub fn is_remote(&self) -> bool {
        matches!(self.src, Src::Remote(_))
    }

    /// What it may hold in memory: its sections' sizes when read from the NAS, else nothing.
    pub fn remote_bytes(&self) -> u64 {
        if self.is_remote() {
            self.table.values().map(|&(_, len)| len).sum()
        } else {
            0
        }
    }

    /// A section as records of `T`, read on demand (an empty one when the file hasn't it).
    pub fn sect<T: Pod>(&self, name: &str) -> Result<Sect<T>> {
        let Some(&(off, len)) = self.table.get(name) else { return Ok(Sect::empty()) };
        let at = match &self.src {
            Src::Local(m) => {
                ensure!(off.checked_add(len).is_some_and(|e| e <= m.len() as u64), "section {name} out of bounds");
                At::Blob(Blob::Map(m.clone(), off as usize, len as usize))
            }
            Src::Remote(r) => At::Remote { file: r.clone(), off, len },
        };
        Sect::new(at, len, name)
    }

    /// `len` bytes from `start` within a section (for sections too big to read whole).
    pub fn get_part(&self, name: &str, start: u64, len: usize) -> Result<Vec<u8>> {
        let &(off, slen) = self.table.get(name).ok_or_else(|| anyhow::anyhow!("no section {name}"))?;
        ensure!(start.checked_add(len as u64).is_some_and(|e| e <= slen), "bytes {start}+{len} outside section {name} ({slen})");
        match &self.src {
            Src::Local(m) => Ok(m[(off + start) as usize..(off + start) as usize + len].to_vec()),
            Src::Remote(r) => Ok(r.read_at(off + start, len)?),
        }
    }

    /// A section's bytes (an empty blob when it's missing).
    pub fn get(&self, name: &str) -> Result<Blob> {
        let Some(&(off, len)) = self.table.get(name) else { return Ok(Blob::from_vec(Vec::new())) };
        match &self.src {
            Src::Local(m) => {
                let (o, l) = (off as usize, len as usize);
                ensure!(o + l <= m.len(), "section {name} out of bounds");
                Ok(Blob::Map(m.clone(), o, l))
            }
            Src::Remote(r) => {
                if let Some(b) = self.loaded.lock().unwrap().get(name) {
                    return Ok(b.clone());
                }
                // Large sections in 8 MB reads, so one slow read times out alone.
                let mut v = Vec::with_capacity(len as usize);
                let mut at = 0u64;
                while at < len {
                    let n = (len - at).min(8 << 20) as usize;
                    v.extend_from_slice(&r.read_at(off + at, n)?);
                    at += n as u64;
                }
                let b = Blob::from_vec(v);
                self.loaded.lock().unwrap().insert(name.to_string(), b.clone());
                Ok(b)
            }
        }
    }
}

/// Where a section's bytes are.
#[derive(Clone)]
enum At {
    /// Mapped (a mirrored file), or empty.
    Blob(Blob),
    /// Bytes [off, off + len) of a file on the NAS.
    Remote { file: Arc<RemoteFile>, off: u64, len: u64 },
}

/// A section as records of `T`: a slice of the map when the file is on this Mac; else read from
/// the NAS as asked, a few records through the page cache or the whole section at once.
pub struct Sect<T> {
    at: At,
    n: usize,
    _t: PhantomData<fn() -> T>,
}

/// The records of several ranges of a section (`Sect::gather`).
pub enum Gathered<T> {
    /// Ranges of a mapped or whole section.
    In(Blob, Vec<Range<usize>>),
    Owned(Vec<Vec<T>>),
}

impl<T: Pod> Gathered<T> {
    pub fn get(&self, k: usize) -> &[T] {
        match self {
            Gathered::In(b, rs) => &b.cast::<T>()[rs[k].clone()],
            Gathered::Owned(v) => &v[k],
        }
    }
}

impl<T: Pod> Sect<T> {
    fn new(at: At, bytes: u64, name: &str) -> Result<Sect<T>> {
        let sz = std::mem::size_of::<T>() as u64;
        ensure!(bytes % sz == 0, "section {name}: {bytes} bytes isn't a whole number of {sz}-byte records");
        let n = (bytes / sz) as usize;
        if let At::Blob(b) = &at {
            ensure!(b.cast::<T>().len() == n, "section {name} isn't aligned for its records");
        }
        Ok(Sect { at, n, _t: PhantomData })
    }

    pub fn empty() -> Sect<T> {
        Sect { at: At::Blob(Blob::from_vec(Vec::new())), n: 0, _t: PhantomData }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    fn check(&self, r: &Range<usize>) -> Result<()> {
        ensure!(r.start <= r.end && r.end <= self.n, "records {}..{} outside a section of {}", r.start, r.end, self.n);
        Ok(())
    }

    /// Records `r` (borrowed from the map, else read: from the section if it's in memory whole,
    /// else from its pages).
    pub fn range(&self, r: Range<usize>) -> Result<Cow<'_, [T]>> {
        self.check(&r)?;
        match &self.at {
            At::Blob(b) => Ok(Cow::Borrowed(&b.cast::<T>()[r])),
            At::Remote { file, off, .. } => {
                if let Some(b) = pages::cached_whole(file, *off) {
                    return Ok(Cow::Owned(b.cast::<T>()[r].to_vec()));
                }
                let sz = std::mem::size_of::<T>();
                let bytes = pages::read(file, off + (r.start * sz) as u64, r.len() * sz)?;
                Ok(Cow::Owned(bytemuck::pod_collect_to_vec(&bytes)))
            }
        }
    }

    pub fn get(&self, i: usize) -> Result<T> {
        Ok(self.range(i..i + 1)?[0])
    }

    /// Several records by index, in the order asked.
    pub fn get_many(&self, idx: &[usize]) -> Result<Vec<T>> {
        let g = self.gather(&idx.iter().map(|&i| i..i + 1).collect::<Vec<_>>())?;
        Ok((0..idx.len()).map(|k| g.get(k)[0]).collect())
    }

    /// The whole section (from the NAS: read once and kept while memory allows).
    pub fn all(&self) -> Result<Blob> {
        match &self.at {
            At::Blob(b) => Ok(b.clone()),
            At::Remote { file, off, len } => Ok(pages::whole(file, *off, *len)?),
        }
    }

    /// Many ranges at once (a road's ways in a unit): from the NAS, their pages in parallel, or
    /// the whole section when they'd touch much of it.
    pub fn gather(&self, ranges: &[Range<usize>]) -> Result<Gathered<T>> {
        for r in ranges {
            self.check(r)?;
        }
        match &self.at {
            At::Blob(b) => Ok(Gathered::In(b.clone(), ranges.to_vec())),
            At::Remote { file, off, len } => {
                if let Some(b) = pages::cached_whole(file, *off) {
                    return Ok(Gathered::In(b, ranges.to_vec()));
                }
                let sz = std::mem::size_of::<T>() as u64;
                let bytes: Vec<(u64, u64)> = ranges.iter().map(|r| (off + r.start as u64 * sz, r.len() as u64 * sz)).collect();
                let need = pages::missing(file, &bytes);
                if need > len / 2 || need > pages::PAGES_PER_BATCH {
                    return Ok(Gathered::In(pages::whole(file, *off, *len)?, ranges.to_vec()));
                }
                pages::prefetch(file, &bytes)?;
                Ok(Gathered::Owned(ranges.iter().map(|r| self.range(r.clone()).map(Cow::into_owned)).collect::<Result<_>>()?))
            }
        }
    }

    /// The first index whose record fails `pred` (records sorted so that it holds for a prefix).
    pub fn partition_point(&self, mut pred: impl FnMut(&T) -> bool) -> Result<usize> {
        if let At::Blob(b) = &self.at {
            return Ok(b.cast::<T>().partition_point(pred));
        }
        let (mut lo, mut hi) = (0, self.n);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if pred(&self.get(mid)?) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }

    /// The records whose key is `k`, in a section sorted by key.
    pub fn equal_range<K: Ord>(&self, k: K, key: impl Fn(&T) -> K) -> Result<Cow<'_, [T]>> {
        let a = self.partition_point(|x| key(x) < k)?;
        let b = self.partition_point(|x| key(x) <= k)?;
        self.range(a..b.max(a))
    }
}

/// One unit's base pack and road values. Sections are read as asked, never up front.
pub struct BaseView {
    pub unit: String,
    pub ways: Sect<WayRec>,
    pub verts: Sect<[i32; 2]>,
    pub elev: Sect<i16>,
    pub grade: Sect<u8>,
    pub src: Sect<u8>,
    pub scenic: Option<Sect<[u8; ch::N]>>,
    strings: Vec<String>,
    rail: Sect<RailRel>,
    pub roads: Sect<RoadRec>,
    /// (road, way index), sorted: the road values file's index, when it has one.
    byroad: Option<Sect<[u64; 2]>>,
    /// The same made from the road values, for files without it.
    made_byroad: Mutex<Option<Arc<Vec<[u64; 2]>>>>,
    remote: bool,
}

impl BaseView {
    pub fn new(base: SectView, roads: SectView) -> Result<BaseView> {
        let unit = base.meta.get("unit").and_then(|v| v.as_str()).context("base pack without a unit")?.to_string();
        let strings = String::from_utf8_lossy(base.get("strings")?.bytes()).split('\n').map(str::to_owned).collect();
        let v = BaseView {
            unit,
            ways: base.sect("ways")?,
            verts: base.sect("verts")?,
            elev: base.sect("elev")?,
            grade: base.sect("grade")?,
            src: base.sect("src")?,
            scenic: if base.has("scenic") { Some(base.sect("scenic")?) } else { None },
            strings,
            rail: base.sect("rail")?,
            roads: roads.sect("roads")?,
            byroad: if roads.has("byroad") { Some(roads.sect("byroad")?) } else { None },
            made_byroad: Mutex::new(None),
            remote: base.is_remote() || roads.is_remote(),
        };
        if v.roads.len() != v.ways.len() {
            bail!("road values out of step with the base pack of {}", v.unit);
        }
        if let Some(b) = &v.byroad {
            ensure!(b.len() == v.ways.len(), "road index out of step with the base pack of {}", v.unit);
        }
        Ok(v)
    }

    pub fn is_remote(&self) -> bool {
        self.remote
    }
    pub fn way(&self, i: u32) -> Result<WayRec> {
        self.ways.get(i as usize)
    }
    pub fn road_val(&self, i: u32) -> Result<RoadRec> {
        self.roads.get(i as usize)
    }
    pub fn string(&self, i: u32) -> &str {
        self.strings.get(i as usize).map(String::as_str).unwrap_or("")
    }
    pub fn range(&self, w: &WayRec) -> Range<usize> {
        w.vstart as usize..(w.vstart + w.vcount as u64) as usize
    }
    /// The bytes these views hold in memory (the names; sections are mapped or paged).
    pub fn weight(&self) -> u64 {
        self.strings.iter().map(|s| s.len() as u64 + 24).sum()
    }

    /// This unit's ways on road `road`.
    pub fn on_road(&self, road: u64) -> Result<Vec<u32>> {
        if let Some(ix) = &self.byroad {
            return Ok(ix.equal_range(road, |e| e[0])?.iter().map(|e| e[1] as u32).collect());
        }
        let ix = {
            let mut g = self.made_byroad.lock().unwrap();
            match g.as_ref() {
                Some(ix) => ix.clone(),
                None => {
                    let all = self.roads.all()?;
                    let mut ix: Vec<[u64; 2]> = all.cast::<RoadRec>().iter().enumerate().map(|(i, r)| [r.road, i as u64]).collect();
                    ix.sort_unstable();
                    let ix = Arc::new(ix);
                    *g = Some(ix.clone());
                    ix
                }
            }
        };
        let (a, b) = (ix.partition_point(|e| e[0] < road), ix.partition_point(|e| e[0] <= road));
        Ok(ix[a..b].iter().map(|e| e[1] as u32).collect())
    }

    /// The rail way's primary route relation, if known.
    pub fn rail_rel(&self, way: u32) -> Result<Option<i64>> {
        Ok(self.rail.equal_range(way, |x| x.way)?.first().map(|r| r.rel()))
    }
}

/// One z6 tile's hidata, read as asked.
pub struct HiView {
    pub tile: String,
    remote: bool,
    pub here: Sect<Here>,
    pub parts: Sect<Part>,
    pub psamples: Sect<PSample>,
    pub pch: Sect<[u8; ch::N]>,
    pub climbs: Sect<Climb>,
    pub climbgeom: Sect<[i32; 2]>,
}

impl HiView {
    pub fn new(s: SectView) -> Result<HiView> {
        Ok(HiView {
            tile: s.meta.get("tile").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            remote: s.is_remote(),
            here: s.sect("here")?,
            parts: s.sect("parts")?,
            psamples: s.sect("psamples")?,
            pch: s.sect("pch")?,
            climbs: s.sect("climbs")?,
            climbgeom: s.sect("climbgeom")?,
        })
    }
    pub fn is_remote(&self) -> bool {
        self.remote
    }
    /// The `here` entry of an OSM way id.
    pub fn find(&self, id: u64) -> Result<Option<Here>> {
        Ok(self.here.equal_range(id, |x| x.id)?.first().copied())
    }
}

/// A tile's query parts, read whole (what a query scans).
pub struct QTile {
    here: Blob,
    parts: Blob,
    psamples: Blob,
    pch: Blob,
}

impl QTile {
    pub fn new(hv: Arc<HiView>) -> Result<QTile> {
        Ok(QTile { here: hv.here.all()?, parts: hv.parts.all()?, psamples: hv.psamples.all()?, pch: hv.pch.all()? })
    }
    pub fn here(&self) -> &[Here] {
        self.here.cast()
    }
    pub fn parts(&self) -> &[Part] {
        self.parts.cast()
    }
    pub fn psamples(&self) -> &[PSample] {
        self.psamples.cast()
    }
    pub fn pch(&self) -> &[[u8; ch::N]] {
        self.pch.cast()
    }
}

/// The road → units index (global/roadunits): sorted (road id, unit tile key) pairs.
pub struct RoadUnits {
    pairs: Sect<[u64; 2]>,
}

impl RoadUnits {
    pub fn new(s: &SectView) -> Result<RoadUnits> {
        Ok(RoadUnits { pairs: s.sect("pairs")? })
    }
    /// The unit keys a road has ways in.
    pub fn units(&self, road: u64) -> Result<Vec<u64>> {
        Ok(self.pairs.equal_range(road, |x| x[0])?.iter().map(|x| x[1]).collect())
    }
}
