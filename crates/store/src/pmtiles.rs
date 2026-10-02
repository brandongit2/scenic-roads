//! A read-only PMTiles v3 reader (spec: github.com/protomaps/PMTiles, spec/v3/spec.md), for the
//! basemap: Planetiler's worldwide archive, served tile by tile (plan §3). It reads through any
//! boxed `RangeRead`: the mirror's copy mmapped, or the NAS file through the I/O pool.
//!
//! An archive is a 127-byte header, a root directory, JSON metadata, leaf directories and tile
//! data. Directories map Hilbert tile ids to tile data (an entry with a run length covers that
//! many consecutive ids with the same bytes) or, with run length 0, to a leaf directory. The root
//! directory is read at open; leaves are fetched on demand and kept in a small LRU.

use crate::range::RangeRead;
use anyhow::{bail, ensure, Context, Result};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};

/// Header size in bytes.
pub const HEADER_LEN: usize = 127;
const MAGIC: &[u8; 7] = b"PMTiles";
const SPEC_VERSION: u8 = 3;
/// What clients fetch first: the header and, in a well-formed archive, the root directory.
const FIRST_READ: u64 = 16384;
/// Leaf directories kept by default.
const LEAF_CACHE: usize = 128;
/// The root and up to three levels of leaves (as the reference reader).
const MAX_DEPTH: usize = 4;
/// Largest directory or metadata accepted once decompressed (a guard against damaged archives).
const MAX_DECOMPRESSED: u64 = 256 << 20;

/// A compression scheme, for directories and metadata (internal) or tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    Unknown,
    None,
    Gzip,
    Brotli,
    Zstd,
    Other(u8),
}

impl Compression {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::None,
            2 => Self::Gzip,
            3 => Self::Brotli,
            4 => Self::Zstd,
            v => Self::Other(v),
        }
    }

    /// The HTTP `Content-Encoding` of data compressed this way, if any.
    pub fn content_encoding(self) -> Option<&'static str> {
        match self {
            Self::Gzip => Some("gzip"),
            Self::Brotli => Some("br"),
            Self::Zstd => Some("zstd"),
            _ => None,
        }
    }
}

/// What the tiles are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileType {
    Unknown,
    Mvt,
    Png,
    Jpeg,
    Webp,
    Avif,
    Other(u8),
}

impl TileType {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::Mvt,
            2 => Self::Png,
            3 => Self::Jpeg,
            4 => Self::Webp,
            5 => Self::Avif,
            v => Self::Other(v),
        }
    }

    /// The HTTP `Content-Type` of such tiles.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Mvt => "application/vnd.mapbox-vector-tile",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            _ => "application/octet-stream",
        }
    }
}

/// The archive header. Positions are in 1e-7 degrees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub root_offset: u64,
    pub root_length: u64,
    pub metadata_offset: u64,
    pub metadata_length: u64,
    pub leaf_offset: u64,
    pub leaf_length: u64,
    pub data_offset: u64,
    pub data_length: u64,
    pub addressed_tiles: u64,
    pub tile_entries: u64,
    pub tile_contents: u64,
    pub clustered: bool,
    pub internal_compression: Compression,
    pub tile_compression: Compression,
    pub tile_type: TileType,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub min_lon_e7: i32,
    pub min_lat_e7: i32,
    pub max_lon_e7: i32,
    pub max_lat_e7: i32,
    pub center_zoom: u8,
    pub center_lon_e7: i32,
    pub center_lat_e7: i32,
}

impl Header {
    /// Parses the 127-byte header at the start of `b`.
    pub fn parse(b: &[u8]) -> Result<Self> {
        ensure!(b.len() >= HEADER_LEN && &b[..7] == MAGIC, "not a PMTiles archive");
        ensure!(b[7] == SPEC_VERSION, "PMTiles version {} isn't supported (only {SPEC_VERSION})", b[7]);
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or_default());
        let i32_at = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap_or_default());
        Ok(Self {
            version: b[7],
            root_offset: u64_at(8),
            root_length: u64_at(16),
            metadata_offset: u64_at(24),
            metadata_length: u64_at(32),
            leaf_offset: u64_at(40),
            leaf_length: u64_at(48),
            data_offset: u64_at(56),
            data_length: u64_at(64),
            addressed_tiles: u64_at(72),
            tile_entries: u64_at(80),
            tile_contents: u64_at(88),
            clustered: b[96] == 1,
            internal_compression: Compression::from_u8(b[97]),
            tile_compression: Compression::from_u8(b[98]),
            tile_type: TileType::from_u8(b[99]),
            min_zoom: b[100],
            max_zoom: b[101],
            min_lon_e7: i32_at(102),
            min_lat_e7: i32_at(106),
            max_lon_e7: i32_at(110),
            max_lat_e7: i32_at(114),
            center_zoom: b[118],
            center_lon_e7: i32_at(119),
            center_lat_e7: i32_at(123),
        })
    }

    /// West, south, east, north in degrees.
    pub fn bounds(&self) -> [f64; 4] {
        [self.min_lon_e7, self.min_lat_e7, self.max_lon_e7, self.max_lat_e7].map(|v| f64::from(v) / 1e7)
    }

    /// Longitude and latitude in degrees, and zoom.
    pub fn center(&self) -> (f64, f64, u8) {
        (f64::from(self.center_lon_e7) / 1e7, f64::from(self.center_lat_e7) / 1e7, self.center_zoom)
    }
}

/// A directory entry: tile data for `run_length` ids from `tile_id` (offset relative to the tile
/// data section), or with run length 0 a leaf directory (offset relative to the leaf section).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirEntry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u32,
    pub run_length: u32,
}

/// The Hilbert tile id of z/x/y: the tiles of every coarser zoom, then the position along zoom
/// z's Hilbert curve.
pub fn zxy_to_tile_id(z: u8, x: u32, y: u32) -> Result<u64> {
    ensure!(z <= 31, "zoom {z} is beyond 64-bit tile ids");
    let n = 1u64 << z;
    ensure!(u64::from(x) < n && u64::from(y) < n, "no tile {z}/{x}/{y}");
    let base = ((1u64 << (2 * u32::from(z))) - 1) / 3;
    let (mut x, mut y) = (u64::from(x), u64::from(y));
    let mut d = 0;
    let mut s = n >> 1;
    while s > 0 {
        let rx = u64::from(x & s != 0);
        let ry = u64::from(y & s != 0);
        d += s * s * ((3 * rx) ^ ry);
        // Rotate within the quadrant; only the bits below `s` matter from here on.
        x &= s - 1;
        y &= s - 1;
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - x;
                y = s - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s >>= 1;
    }
    Ok(base + d)
}

/// The z/x/y of a Hilbert tile id.
pub fn tile_id_to_zxy(id: u64) -> Result<(u8, u32, u32)> {
    let mut base = 0u64;
    for z in 0..=31u8 {
        let count = 1u64 << (2 * u32::from(z));
        if id - base < count {
            let mut t = id - base;
            let (mut x, mut y) = (0u64, 0u64);
            let mut s = 1u64;
            while s < 1u64 << z {
                let rx = 1 & (t / 2);
                let ry = 1 & (t ^ rx);
                if ry == 0 {
                    if rx == 1 {
                        x = s - 1 - x;
                        y = s - 1 - y;
                    }
                    std::mem::swap(&mut x, &mut y);
                }
                x += s * rx;
                y += s * ry;
                t /= 4;
                s *= 2;
            }
            return Ok((z, x as u32, y as u32));
        }
        base += count;
    }
    bail!("tile id {id} is beyond zoom 31")
}

fn varint(b: &[u8], pos: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*pos).context("directory cut short")?;
        *pos += 1;
        ensure!(shift < 63 || byte <= 1, "varint overflows 64 bits");
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
    }
    bail!("varint longer than 10 bytes")
}

/// Decodes a (decompressed) directory: the entry count, then tile id deltas, run lengths,
/// lengths and offsets, each a column of varints (an offset of 0 after the first entry means
/// "right after the previous entry's data"; others are stored plus one).
pub fn parse_directory(b: &[u8]) -> Result<Vec<DirEntry>> {
    let mut pos = 0;
    let n = varint(b, &mut pos)?;
    // Every entry takes at least four bytes.
    ensure!(n.saturating_mul(4) <= b.len() as u64, "directory claims {n} entries in {} bytes", b.len());
    let mut entries = vec![DirEntry::default(); n as usize];
    let mut id = 0u64;
    for (i, e) in entries.iter_mut().enumerate() {
        let delta = varint(b, &mut pos)?;
        ensure!(i == 0 || delta > 0, "directory tile ids aren't ascending");
        id = id.checked_add(delta).context("tile id overflows")?;
        e.tile_id = id;
    }
    for e in entries.iter_mut() {
        e.run_length = u32::try_from(varint(b, &mut pos)?).context("run length too large")?;
    }
    for e in entries.iter_mut() {
        e.length = u32::try_from(varint(b, &mut pos)?).context("length too large")?;
    }
    for i in 0..entries.len() {
        let v = varint(b, &mut pos)?;
        entries[i].offset = if v == 0 {
            ensure!(i > 0, "directory's first offset is relative");
            let p = entries[i - 1];
            p.offset.checked_add(u64::from(p.length)).context("offset overflows")?
        } else {
            v - 1
        };
    }
    Ok(entries)
}

/// The entry holding `id`: an exact match, else the entry before it when that is a leaf
/// directory or a run that reaches `id`.
fn find_entry(entries: &[DirEntry], id: u64) -> Option<DirEntry> {
    match entries.binary_search_by_key(&id, |e| e.tile_id) {
        Ok(i) => Some(entries[i]),
        Err(0) => None,
        Err(i) => {
            let e = entries[i - 1];
            (e.run_length == 0 || id - e.tile_id < u64::from(e.run_length)).then_some(e)
        }
    }
}

fn decompress(b: &[u8], c: Compression) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match c {
        // The reference reader takes "unknown" as uncompressed.
        Compression::None | Compression::Unknown => return Ok(b.to_vec()),
        Compression::Gzip => {
            GzDecoder::new(b).take(MAX_DECOMPRESSED + 1).read_to_end(&mut out).context("gzip")?;
        }
        Compression::Zstd => {
            zstd::stream::read::Decoder::new(b)?.take(MAX_DECOMPRESSED + 1).read_to_end(&mut out).context("zstd")?;
        }
        c => bail!("internal compression {c:?} isn't supported"),
    }
    ensure!(out.len() as u64 <= MAX_DECOMPRESSED, "directory or metadata over {MAX_DECOMPRESSED} bytes decompressed");
    Ok(out)
}

type Dir = Arc<Vec<DirEntry>>;

/// A tiny LRU of leaf directories by (offset, length).
struct Lru {
    cap: usize,
    tick: u64,
    /// (offset, length) → (last use, directory).
    map: HashMap<(u64, u64), (u64, Dir)>,
}

impl Lru {
    fn get(&mut self, k: (u64, u64)) -> Option<Dir> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(&k).map(|(used, v)| {
            *used = t;
            v.clone()
        })
    }

    fn put(&mut self, k: (u64, u64), v: Dir) {
        if self.cap == 0 {
            return;
        }
        if self.map.len() >= self.cap && !self.map.contains_key(&k) {
            if let Some(old) = self.map.iter().min_by_key(|(_, (used, _))| *used).map(|(k, _)| *k) {
                self.map.remove(&old);
            }
        }
        self.tick += 1;
        self.map.insert(k, (self.tick, v));
    }
}

/// An open archive.
pub struct PmTiles {
    src: Box<dyn RangeRead>,
    header: Header,
    root: Dir,
    leaves: Mutex<Lru>,
}

impl std::fmt::Debug for PmTiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PmTiles").field("header", &self.header).field("root_entries", &self.root.len()).finish_non_exhaustive()
    }
}

impl PmTiles {
    /// Reads the header and root directory (one range read for a well-formed archive).
    pub fn open(src: Box<dyn RangeRead>) -> Result<Self> {
        Self::with_leaf_cache(src, LEAF_CACHE)
    }

    /// `open`, keeping up to `leaves` leaf directories.
    pub fn with_leaf_cache(src: Box<dyn RangeRead>, leaves: usize) -> Result<Self> {
        let len = src.len()?;
        ensure!(len >= HEADER_LEN as u64, "not a PMTiles archive ({len} bytes)");
        let first = src.read_at(0, len.min(FIRST_READ) as usize)?;
        let h = Header::parse(&first)?;
        for (what, off, n) in [
            ("root directory", h.root_offset, h.root_length),
            ("metadata", h.metadata_offset, h.metadata_length),
            ("leaf directories", h.leaf_offset, h.leaf_length),
            ("tile data", h.data_offset, h.data_length),
        ] {
            ensure!(off.checked_add(n).is_some_and(|end| end <= len), "the {what} run past the end of the archive");
        }
        let root_end = h.root_offset + h.root_length;
        let raw = if root_end <= first.len() as u64 {
            first[h.root_offset as usize..root_end as usize].to_vec()
        } else {
            src.read_at(h.root_offset, h.root_length as usize)?
        };
        let root = parse_directory(&decompress(&raw, h.internal_compression)?).context("root directory")?;
        Ok(Self { src, header: h, root: Arc::new(root), leaves: Mutex::new(Lru { cap: leaves, tick: 0, map: HashMap::new() }) })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn source(&self) -> &dyn RangeRead {
        &*self.src
    }

    /// Tile z/x/y as stored (still compressed as `header().tile_compression` says), or None when
    /// the archive has no such tile.
    pub fn get(&self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        match self.locate(z, x, y)? {
            Some((off, len)) => Ok(Some(self.src.read_at(off, len as usize)?)),
            None => Ok(None),
        }
    }

    /// Where tile z/x/y's bytes are (absolute offset and length), or None when the archive has no
    /// such tile (or there is no such tile). Tiles with the same bytes share a location.
    pub fn locate(&self, z: u8, x: u32, y: u32) -> Result<Option<(u64, u32)>> {
        if z < self.header.min_zoom || z > self.header.max_zoom {
            return Ok(None);
        }
        let Ok(id) = zxy_to_tile_id(z, x, y) else { return Ok(None) };
        let h = &self.header;
        let mut dir = self.root.clone();
        for _ in 0..MAX_DEPTH {
            let Some(e) = find_entry(&dir, id) else { return Ok(None) };
            if e.run_length > 0 {
                ensure!(
                    e.offset.checked_add(u64::from(e.length)).is_some_and(|end| end <= h.data_length),
                    "tile {z}/{x}/{y} points outside the tile data"
                );
                return Ok(Some((h.data_offset + e.offset, e.length)));
            }
            dir = self.leaf(e)?;
        }
        bail!("directories nested deeper than {MAX_DEPTH} levels")
    }

    fn leaf(&self, e: DirEntry) -> Result<Dir> {
        let h = &self.header;
        ensure!(
            e.offset.checked_add(u64::from(e.length)).is_some_and(|end| end <= h.leaf_length),
            "a leaf directory lies outside the leaf section"
        );
        let key = (h.leaf_offset + e.offset, u64::from(e.length));
        if let Some(d) = self.cache().get(key) {
            return Ok(d);
        }
        let raw = self.src.read_at(key.0, e.length as usize)?;
        let d = Arc::new(parse_directory(&decompress(&raw, h.internal_compression)?).context("leaf directory")?);
        self.cache().put(key, d.clone());
        Ok(d)
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Lru> {
        self.leaves.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The archive's JSON metadata.
    pub fn metadata(&self) -> Result<Value> {
        let h = &self.header;
        let raw = self.src.read_at(h.metadata_offset, usize::try_from(h.metadata_length)?)?;
        serde_json::from_slice(&decompress(&raw, h.internal_compression)?).context("PMTiles metadata")
    }

    /// Calls `f` with every tile entry of every directory, in tile id order (for checks and tools;
    /// it reads every leaf directory).
    pub fn for_each_entry(&self, mut f: impl FnMut(&DirEntry) -> Result<()>) -> Result<()> {
        self.walk(&self.root.clone(), 1, &mut f)
    }

    fn walk(&self, dir: &[DirEntry], depth: usize, f: &mut impl FnMut(&DirEntry) -> Result<()>) -> Result<()> {
        for e in dir {
            if e.run_length > 0 {
                f(e)?;
            } else {
                ensure!(depth < MAX_DEPTH, "directories nested deeper than {MAX_DEPTH} levels");
                let leaf = self.leaf(*e)?;
                self.walk(&leaf, depth + 1, f)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression as Level};
    use std::io::Write;

    #[test]
    fn tile_ids() {
        // The spec's reference values.
        assert_eq!(zxy_to_tile_id(0, 0, 0).unwrap(), 0);
        assert_eq!(zxy_to_tile_id(1, 0, 0).unwrap(), 1);
        assert_eq!(zxy_to_tile_id(1, 0, 1).unwrap(), 2);
        assert_eq!(zxy_to_tile_id(1, 1, 1).unwrap(), 3);
        assert_eq!(zxy_to_tile_id(1, 1, 0).unwrap(), 4);
        assert_eq!(zxy_to_tile_id(2, 0, 0).unwrap(), 5);
        for (z, x, y, id) in REFERENCE_IDS {
            assert_eq!(zxy_to_tile_id(z, x, y).unwrap(), id, "{z}/{x}/{y}");
            assert_eq!(tile_id_to_zxy(id).unwrap(), (z, x, y));
        }
        // Every tile of the first zooms round-trips, ids are dense.
        let mut next = 0;
        for z in 0..=6u8 {
            let mut ids: Vec<u64> = Vec::new();
            for x in 0..1u32 << z {
                for y in 0..1u32 << z {
                    let id = zxy_to_tile_id(z, x, y).unwrap();
                    assert_eq!(tile_id_to_zxy(id).unwrap(), (z, x, y));
                    ids.push(id);
                }
            }
            ids.sort_unstable();
            assert_eq!(ids, (next..next + (1u64 << (2 * z))).collect::<Vec<_>>());
            next += 1 << (2 * z);
        }
        let max = (1u32 << 31) - 1;
        let id = zxy_to_tile_id(31, max, max).unwrap();
        assert_eq!(tile_id_to_zxy(id).unwrap(), (31, max, max));
        assert!(zxy_to_tile_id(32, 0, 0).is_err());
        assert!(zxy_to_tile_id(3, 8, 0).is_err());
        assert!(tile_id_to_zxy(u64::MAX).is_err());
    }

    /// From the reference JavaScript implementation (pmtiles 4.5.0, `zxyToTileId`).
    const REFERENCE_IDS: [(u8, u32, u32, u64); 10] = [
        (3, 5, 2, 76),
        (7, 100, 27, 21194),
        (10, 513, 1000, 961086),
        (12, 3229, 2031, 19164411),
        (14, 12917, 8130, 306617074),
        (16, 51674, 32521, 4905873026),
        (20, 1000000, 3, 1461989238116),
        (24, 16777215, 16777215, 281474976710655),
        (26, 67108863, 0, 6004799503160660),
        (26, 12345678, 54321098, 3154909521354637),
    ];

    fn put_varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(v as u8 | 0x80);
            v >>= 7;
        }
        out.push(v as u8);
    }

    fn encode_dir(entries: &[DirEntry]) -> Vec<u8> {
        let mut b = Vec::new();
        put_varint(&mut b, entries.len() as u64);
        let mut last = 0;
        for e in entries {
            put_varint(&mut b, e.tile_id - last);
            last = e.tile_id;
        }
        for e in entries {
            put_varint(&mut b, u64::from(e.run_length));
        }
        for e in entries {
            put_varint(&mut b, u64::from(e.length));
        }
        for (i, e) in entries.iter().enumerate() {
            if i > 0 && e.offset == entries[i - 1].offset + u64::from(entries[i - 1].length) {
                put_varint(&mut b, 0);
            } else {
                put_varint(&mut b, e.offset + 1);
            }
        }
        b
    }

    fn gzip(b: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Level::new(6));
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    /// A small archive in the spec's layout: tiles z0–5 (each tile's bytes name it; the z5 tiles of
    /// the bottom half are all "sea", stored once as runs), with leaf directories of `leaf_size`
    /// entries when given.
    /// z, x, y and the tile's bytes.
    type Tiles = Vec<(u8, u32, u32, Vec<u8>)>;

    fn build(leaf_size: Option<usize>) -> (Vec<u8>, Tiles) {
        let mut tiles = Vec::new();
        for z in 0..=5u8 {
            for x in 0..1u32 << z {
                for y in 0..1u32 << z {
                    let body = if z == 5 && y >= 16 { b"sea".to_vec() } else { format!("{z}/{x}/{y}").into_bytes() };
                    tiles.push((zxy_to_tile_id(z, x, y).unwrap(), z, x, y, body));
                }
            }
        }
        tiles.sort_by_key(|t| t.0);
        // Tile data in id order, runs of identical consecutive tiles merged, repeated bytes stored once.
        let mut data = Vec::new();
        let mut stored: HashMap<Vec<u8>, (u64, u32)> = HashMap::new();
        let mut entries: Vec<DirEntry> = Vec::new();
        for (id, _, _, _, body) in &tiles {
            if let (Some(last), Some(&(o, l))) = (entries.last_mut(), stored.get(body)) {
                if last.tile_id + u64::from(last.run_length) == *id && (last.offset, last.length) == (o, l) {
                    last.run_length += 1;
                    continue;
                }
            }
            let (offset, length) = *stored.entry(body.clone()).or_insert_with(|| {
                let at = (data.len() as u64, body.len() as u32);
                data.extend_from_slice(body);
                at
            });
            entries.push(DirEntry { tile_id: *id, offset, length, run_length: 1 });
        }
        let (root, leaves) = match leaf_size {
            None => (encode_dir(&entries), Vec::new()),
            Some(n) => {
                let mut leaves = Vec::new();
                let mut root = Vec::new();
                for chunk in entries.chunks(n) {
                    let leaf = gzip(&encode_dir(chunk));
                    root.push(DirEntry { tile_id: chunk[0].tile_id, offset: leaves.len() as u64, length: leaf.len() as u32, run_length: 0 });
                    leaves.extend_from_slice(&leaf);
                }
                (encode_dir(&root), leaves)
            }
        };
        let root = gzip(&root);
        let meta = gzip(br#"{"name":"test","vector_layers":[]}"#);
        let root_off = HEADER_LEN as u64;
        let meta_off = root_off + root.len() as u64;
        let leaf_off = meta_off + meta.len() as u64;
        let data_off = leaf_off + leaves.len() as u64;
        let mut h = Vec::new();
        h.extend_from_slice(MAGIC);
        h.push(3);
        for v in [root_off, root.len() as u64, meta_off, meta.len() as u64, leaf_off, leaves.len() as u64, data_off, data.len() as u64] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        let addressed = tiles.len() as u64;
        for v in [addressed, entries.len() as u64, stored.len() as u64] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.extend_from_slice(&[1, 2, 1, 1, 0, 5]); // clustered, gzip internal, no tile compression, mvt, z0–5
        for v in [-1_800_000_000i32, -850_511_287, 1_800_000_000, 850_511_287] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        h.push(2);
        h.extend_from_slice(&15_000_000i32.to_le_bytes());
        h.extend_from_slice(&(-25_000_000i32).to_le_bytes());
        assert_eq!(h.len(), HEADER_LEN);
        let mut file = h;
        file.extend_from_slice(&root);
        file.extend_from_slice(&meta);
        file.extend_from_slice(&leaves);
        file.extend_from_slice(&data);
        (file, tiles.into_iter().map(|(_, z, x, y, b)| (z, x, y, b)).collect())
    }

    fn check(archive: &PmTiles, tiles: &Tiles) {
        for (z, x, y, body) in tiles {
            assert_eq!(archive.get(*z, *x, *y).unwrap().as_ref(), Some(body), "{z}/{x}/{y}");
        }
        assert_eq!(archive.get(6, 0, 0).unwrap(), None, "beyond max zoom");
        assert_eq!(archive.get(3, 8, 0).unwrap(), None, "no such tile");
        let mut n = 0;
        let mut addressed = 0;
        archive
            .for_each_entry(|e| {
                n += 1;
                addressed += u64::from(e.run_length);
                Ok(())
            })
            .unwrap();
        assert_eq!(n, archive.header().tile_entries);
        assert_eq!(addressed, archive.header().addressed_tiles);
    }

    #[test]
    fn synthetic_archives() {
        let (file, tiles) = build(None);
        let a = PmTiles::open(Box::new(file)).unwrap();
        let h = a.header().clone();
        assert_eq!((h.tile_type, h.tile_compression, h.internal_compression), (TileType::Mvt, Compression::None, Compression::Gzip));
        assert_eq!((h.min_zoom, h.max_zoom, h.clustered), (0, 5, true));
        assert_eq!(h.bounds(), [-180.0, -85.0511287, 180.0, 85.0511287]);
        assert_eq!(h.center(), (1.5, -2.5, 2));
        assert!(h.tile_entries < h.addressed_tiles, "the sea is stored as runs");
        assert_eq!(a.metadata().unwrap()["name"], "test");
        check(&a, &tiles);

        // With leaf directories (and a cache smaller than their number).
        for size in [1, 7, 50] {
            let (file, tiles) = build(Some(size));
            let a = PmTiles::with_leaf_cache(Box::new(file), 3).unwrap();
            check(&a, &tiles);
        }
    }

    #[test]
    fn damaged_archives() {
        let (file, _) = build(Some(10));
        let mut bad = file.clone();
        bad[7] = 2;
        assert!(PmTiles::open(Box::new(bad)).is_err());
        assert!(PmTiles::open(Box::new(file[..100].to_vec())).is_err());
        assert!(PmTiles::open(Box::new(file[..file.len() - 1].to_vec())).is_err(), "tile data runs past the end");
        assert!(parse_directory(&[5, 1]).is_err());
        assert!(parse_directory(&[0xff; 11]).is_err());
        // An offset of 0 for the first entry has nothing to follow.
        assert!(parse_directory(&[1, 0, 1, 1, 0]).is_err());
        assert_eq!(parse_directory(&[1, 0, 1, 1, 1]).unwrap(), [DirEntry { tile_id: 0, offset: 0, length: 1, run_length: 1 }]);
    }

    /// The Singapore basemap part, when this checkout has it (it isn't in git).
    fn singapore() -> Option<PmTiles> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/build/base-parts/singapore.pmtiles");
        if !p.exists() {
            eprintln!("skipping: {} isn't here", p.display());
            return None;
        }
        Some(PmTiles::open(Box::new(crate::range::MmapFile::open(&p).unwrap())).unwrap())
    }

    #[test]
    fn real_archive_every_entry_resolves() {
        let Some(a) = singapore() else { return };
        let h = a.header().clone();
        assert_eq!((h.tile_type, h.tile_compression, h.internal_compression), (TileType::Mvt, Compression::Gzip, Compression::Gzip));
        assert_eq!((h.min_zoom, h.max_zoom), (0, 14));
        assert!(h.leaf_length > 0, "the archive uses leaf directories");
        let (mut entries, mut addressed) = (0u64, 0u64);
        let mut contents = std::collections::HashSet::new();
        let mut last_id = None;
        a.for_each_entry(|e| {
            entries += 1;
            addressed += u64::from(e.run_length);
            contents.insert(e.offset);
            assert!(last_id.is_none_or(|l| l < e.tile_id), "entries in id order");
            last_id = Some(e.tile_id);
            // The first and last tile of the entry's run resolve to its bytes.
            for id in [e.tile_id, e.tile_id + u64::from(e.run_length) - 1] {
                let (z, x, y) = tile_id_to_zxy(id)?;
                assert_eq!(a.locate(z, x, y)?, Some((h.data_offset + e.offset, e.length)), "{z}/{x}/{y}");
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(entries, h.tile_entries);
        assert_eq!(addressed, h.addressed_tiles);
        assert_eq!(contents.len() as u64, h.tile_contents);
        assert!(a.metadata().unwrap().get("vector_layers").is_some());
    }

    /// Reads a protobuf varint.
    fn pb_varint(b: &[u8], pos: &mut usize) -> u64 {
        varint(b, pos).unwrap()
    }

    /// The names of an MVT tile's layers (field 3 messages, each with its name in field 1 and
    /// version 2 in field 15).
    fn mvt_layers(tile: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        let mut pos = 0;
        while pos < tile.len() {
            let key = pb_varint(tile, &mut pos);
            assert_eq!(key, 3 << 3 | 2, "a tile holds only layers");
            let len = pb_varint(tile, &mut pos) as usize;
            let layer = &tile[pos..pos + len];
            pos += len;
            let (mut name, mut version, mut p) = (None, None, 0);
            while p < layer.len() {
                let key = pb_varint(layer, &mut p);
                match key & 7 {
                    0 => {
                        let v = pb_varint(layer, &mut p);
                        if key >> 3 == 15 {
                            version = Some(v);
                        }
                    }
                    2 => {
                        let n = pb_varint(layer, &mut p) as usize;
                        if key >> 3 == 1 {
                            name = Some(String::from_utf8(layer[p..p + n].to_vec()).unwrap());
                        }
                        p += n;
                    }
                    5 => p += 4,
                    1 => p += 8,
                    w => panic!("wire type {w}"),
                }
            }
            assert_eq!(version, Some(2));
            names.push(name.unwrap());
        }
        names
    }

    fn gunzip(b: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        GzDecoder::new(b).read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn real_archive_tiles_match_the_reference_reader() {
        let Some(a) = singapore() else { return };
        for &(z, x, y, want) in REFERENCE_TILES {
            let got = a.get(z, x, y).unwrap().map(|t| {
                let raw = gunzip(&t);
                let mut crc = flate2::Crc::new();
                crc.update(&raw);
                (raw.len(), crc.sum())
            });
            assert_eq!(got, want, "{z}/{x}/{y}");
        }
        // Tiles over Singapore are MVT, with layers the metadata lists.
        let known: Vec<String> = a.metadata().unwrap()["vector_layers"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap().to_string()).collect();
        for (z, x, y) in [(0, 0, 0), (10, 807, 508), (14, 12917, 8130)] {
            let layers = mvt_layers(&gunzip(&a.get(z, x, y).unwrap().unwrap()));
            assert!(layers.iter().any(|l| l == "water"), "{z}/{x}/{y}: {layers:?}");
            assert!(layers.iter().all(|l| known.contains(l)), "{z}/{x}/{y}: {layers:?} vs {known:?}");
        }
    }

    /// From the reference JavaScript reader (pmtiles 4.5.0 `getZxy`): the decompressed tile's
    /// length and CRC-32, or None for a tile the archive doesn't have.
    /// z, x, y and the decompressed tile's length and CRC-32.
    type RefTile = (u8, u32, u32, Option<(usize, u32)>);

    const REFERENCE_TILES: &[RefTile] = &[
        (0, 0, 0, Some((7966, 1423014720))),
        (1, 1, 0, Some((3910, 1443025907))),
        (2, 3, 1, Some((6564, 1635082071))),
        (3, 6, 3, Some((7622, 3655277861))),
        (4, 12, 7, Some((6306, 3943826392))),
        (5, 25, 15, Some((4529, 3840501083))),
        (6, 50, 31, Some((8408, 3966465908))),
        (7, 100, 63, Some((8497, 1924013743))),
        (8, 201, 127, Some((12769, 2699786131))),
        (9, 403, 254, Some((23878, 2268633891))),
        (10, 807, 508, Some((22147, 2728133573))),
        (10, 808, 508, Some((4507, 3975483082))),
        (10, 807, 509, Some((10793, 3983517986))),
        (11, 1614, 1016, Some((26351, 3839554412))),
        (11, 1615, 1016, Some((14017, 640060603))),
        (11, 1614, 1017, Some((12564, 826957425))),
        (12, 3229, 2032, Some((17017, 1220950706))),
        (12, 3230, 2032, Some((21953, 4185669463))),
        (12, 3229, 2033, Some((20101, 2210277356))),
        (13, 6458, 4065, Some((11911, 687182797))),
        (13, 6459, 4065, Some((14052, 4159881029))),
        (13, 6458, 4066, Some((15015, 3672841058))),
        (14, 12916, 8130, Some((8645, 3400069643))),
        (14, 12917, 8130, Some((12484, 2951155794))),
        (14, 12916, 8131, Some((14984, 680099466))),
        (9, 400, 250, None),
        (12, 3300, 2000, None),
        (14, 13000, 8000, Some((57, 3967828759))),
        (5, 0, 0, None),
        (14, 0, 0, None),
        (8, 200, 125, Some((1096, 1704028209))),
    ];
}
