//! Typed views of the per-area files (docs/formats.md): sectioned files read from the Mac's mirror
//! (memory-mapped) or from the NAS (sections read on first use, through the I/O pool, into aligned
//! memory: NAS files are never mapped).

use anyhow::{bail, ensure, Context, Result};
use bytemuck::Pod;
use memmap2::Mmap;
use roadcore::packs::{Climb, End, Here, PSample, Part, RailRel, RoadRec, Sub9};
use roadcore::scenic::{ch, Sample};
use roadcore::WayRec;
use std::collections::HashMap;
use std::path::PathBuf;
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
    file: Mutex<Option<(Arc<std::fs::File>, u64)>>,
}

const PIECE: usize = 1 << 20;

impl RemoteFile {
    pub fn new(path: PathBuf, pool: Arc<store::iopool::IoPool>) -> RemoteFile {
        RemoteFile { path, pool, file: Mutex::new(None) }
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

/// One unit's base pack and road values.
pub struct BaseView {
    pub unit: String,
    pub extent: [i32; 4],
    pub ways: Blob,
    pub verts: Blob,
    pub elev: Blob,
    pub grade: Blob,
    pub src: Blob,
    pub scenic: Option<Blob>,
    pub drape: Option<Blob>,
    pub strings: Vec<String>,
    pub rail: Blob,
    pub roads: Blob,
    /// Lazily: the samples, and road id → this unit's ways on it.
    sect: SectView,
    by_road: std::sync::OnceLock<HashMap<u64, Vec<u32>>>,
}

impl BaseView {
    pub fn new(base: SectView, roads: SectView) -> Result<BaseView> {
        let unit = base.meta.get("unit").and_then(|v| v.as_str()).context("base pack without a unit")?.to_string();
        let ext: Vec<i32> = serde_json::from_value(base.meta.get("extent").cloned().unwrap_or_default()).unwrap_or_default();
        let strings = String::from_utf8_lossy(base.get("strings")?.bytes()).split('\n').map(str::to_owned).collect();
        let v = BaseView {
            unit,
            extent: if ext.len() == 4 { [ext[0], ext[1], ext[2], ext[3]] } else { [0; 4] },
            ways: base.get("ways")?,
            verts: base.get("verts")?,
            elev: base.get("elev")?,
            grade: base.get("grade")?,
            src: base.get("src")?,
            scenic: if base.has("scenic") { Some(base.get("scenic")?) } else { None },
            drape: if base.has("drape") { Some(base.get("drape")?) } else { None },
            strings,
            rail: base.get("rail")?,
            roads: roads.get("roads")?,
            sect: base,
            by_road: std::sync::OnceLock::new(),
        };
        if v.road_vals().len() != v.ways().len() {
            bail!("road values out of step with the base pack of {}", v.unit);
        }
        Ok(v)
    }

    pub fn is_remote(&self) -> bool {
        self.sect.is_remote()
    }
    pub fn ways(&self) -> &[WayRec] {
        self.ways.cast()
    }
    pub fn verts(&self) -> &[[i32; 2]] {
        self.verts.cast()
    }
    pub fn elev(&self) -> &[i16] {
        self.elev.cast()
    }
    pub fn grade(&self) -> &[u8] {
        self.grade.bytes()
    }
    pub fn src(&self) -> &[u8] {
        self.src.bytes()
    }
    pub fn scenic(&self) -> Option<&[[u8; ch::N]]> {
        self.scenic.as_ref().map(|b| b.cast())
    }
    pub fn road_vals(&self) -> &[RoadRec] {
        self.roads.cast()
    }
    pub fn rails(&self) -> &[RailRel] {
        self.rail.cast()
    }
    pub fn string(&self, i: u32) -> &str {
        self.strings.get(i as usize).map(String::as_str).unwrap_or("")
    }
    pub fn range(&self, w: &WayRec) -> std::ops::Range<usize> {
        w.vstart as usize..(w.vstart + w.vcount as u64) as usize
    }
    pub fn samples(&self) -> Result<(Blob, Blob)> {
        Ok((self.sect.get("samples")?, self.sect.get("samplech")?))
    }
    pub fn sub9(&self) -> Result<Vec<Sub9>> {
        Ok(self.sect.get("sub9")?.cast::<Sub9>().to_vec())
    }
    /// This unit's ways on road `road`.
    pub fn on_road(&self, road: u64) -> &[u32] {
        let m = self.by_road.get_or_init(|| {
            let mut m: HashMap<u64, Vec<u32>> = HashMap::new();
            for (i, r) in self.road_vals().iter().enumerate() {
                m.entry(r.road).or_default().push(i as u32);
            }
            m
        });
        m.get(&road).map(Vec::as_slice).unwrap_or(&[])
    }
    /// The rail way's primary route relation, if known.
    pub fn rail_rel(&self, way: u32) -> Option<i64> {
        let r = self.rails();
        r.binary_search_by_key(&way, |x| x.way).ok().map(|k| r[k].rel())
    }
    /// The samples of way `i` (indices into the samples section).
    pub fn sample_range(samples: &[Sample], i: u32) -> std::ops::Range<usize> {
        samples.partition_point(|s| s.way < i)..samples.partition_point(|s| s.way <= i)
    }
}

/// One z6 tile's hidata.
pub struct HiView {
    pub tile: String,
    remote: bool,
    pub here: Blob,
    pub ends: Blob,
    pub parts: Blob,
    pub psamples: Blob,
    pub pch: Blob,
    pub climbs: Blob,
    pub climbgeom: Blob,
}

impl HiView {
    pub fn new(s: SectView) -> Result<HiView> {
        Ok(HiView {
            tile: s.meta.get("tile").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            remote: s.is_remote(),
            here: s.get("here")?,
            ends: s.get("ends")?,
            parts: s.get("parts")?,
            psamples: s.get("psamples")?,
            pch: s.get("pch")?,
            climbs: s.get("climbs")?,
            climbgeom: s.get("climbgeom")?,
        })
    }
    pub fn is_remote(&self) -> bool {
        self.remote
    }
    pub fn here(&self) -> &[Here] {
        self.here.cast()
    }
    pub fn ends(&self) -> &[End] {
        self.ends.cast()
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
    pub fn climbs(&self) -> &[Climb] {
        self.climbs.cast()
    }
    pub fn climbgeom(&self) -> &[[i32; 2]] {
        self.climbgeom.cast()
    }
    /// The `here` entry of an OSM way id.
    pub fn find(&self, id: u64) -> Option<&Here> {
        let h = self.here();
        h.binary_search_by_key(&id, |x| x.id).ok().map(|i| &h[i])
    }
    /// Ways with an end at `point` (`roadcore::packs::point_key`).
    pub fn ends_at(&self, point: u64) -> impl Iterator<Item = &Here> {
        let e = self.ends();
        let a = e.partition_point(|x| x.point < point);
        let b = e.partition_point(|x| x.point <= point);
        let here = self.here();
        e[a..b].iter().filter_map(move |x| here.get(x.here as usize))
    }
}

/// The road → units index (global/roadunits): sorted (road id, unit tile key) pairs.
pub struct RoadUnits {
    pairs: Blob,
}

impl RoadUnits {
    pub fn new(s: &SectView) -> Result<RoadUnits> {
        Ok(RoadUnits { pairs: s.get("pairs")? })
    }
    /// The unit keys a road has ways in.
    pub fn units(&self, road: u64) -> Vec<u64> {
        let p: &[[u64; 2]] = self.pairs.cast();
        let a = p.partition_point(|x| x[0] < road);
        let b = p.partition_point(|x| x[0] <= road);
        p[a..b].iter().map(|x| x[1]).collect()
    }
}
