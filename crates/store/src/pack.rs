//! Packs (docs/formats.md, RDPACK v1): the tiles of one layer under one root tile, in one file.
//! SMB manages about 80 random reads a second per file, so tiles are never stored one per file.
//!
//! A pack is a header with the meta JSON, then the blobs (identical blobs stored once, several
//! index entries sharing an offset), then an index of (tile key, offset, length, raw length, XXH3)
//! sorted by key. Readers fetch the header and index once (two range reads), keep the index
//! locally (`PackIndex::to_bytes`, the mirror's `idx/`), and then serve a tile with one range
//! read, and a 304 with none: the XXH3 is the tile's ETag.

use crate::naming::xxh3;
use crate::range::RangeRead;
use anyhow::{bail, ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use crate::sys::PosIo;
use std::path::Path;

pub use roadcore::archive::tile_key;

pub const MAGIC: &[u8; 8] = b"RDPACK01";
/// The format version written.
pub const VERSION: u32 = 1;
/// The oldest version read (plan §8: readers take the current and the previous version).
pub const MIN_VERSION: u32 = 1;
/// Flag bit 0: the blobs are gzip'd (served as is, with `Content-Encoding: gzip`).
pub const FLAG_GZIP: u32 = 1;

const HEADER_LEN: usize = 36;
const ENTRY_LEN: usize = 32;
/// The first read of a pack: the header and meta, and the index too when the pack is small.
const FIRST_READ: u64 = 64 << 10;
/// Magic of the local index cache encoding (`PackIndex::to_bytes`).
const IDX_MAGIC: &[u8; 8] = b"RDPKIDX1";

const MASK29: u64 = (1 << 29) - 1;

/// The zoom, x and y of a tile key.
pub fn tile_zxy(key: u64) -> (u8, u32, u32) {
    ((key >> 58) as u8, ((key >> 29) & MASK29) as u32, (key & MASK29) as u32)
}

/// Whether z/x/y is a tile a key can hold: x and y below 2^z, z at most 29.
pub fn valid_tile(z: u8, x: u32, y: u32) -> bool {
    z <= 29 && u64::from(x) < 1 << z && u64::from(y) < 1 << z
}

/// One index entry (32 bytes, as stored).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct Entry {
    /// `tile_key(z, x, y)`.
    pub key: u64,
    /// Absolute offset of the blob in the pack.
    pub offset: u64,
    pub len: u32,
    /// The tile's size before gzip, 0 if unknown.
    pub raw_len: u32,
    /// XXH3-64 of the stored blob.
    pub hash: u64,
}

const _: () = assert!(std::mem::size_of::<Entry>() == ENTRY_LEN);

impl Entry {
    pub fn zxy(&self) -> (u8, u32, u32) {
        tile_zxy(self.key)
    }

    /// The blob hash as an ETag value (16 hex digits).
    pub fn etag(&self) -> String {
        format!("{:016x}", self.hash)
    }
}

/// What `PackWriter::finish` wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackStats {
    /// Index entries.
    pub tiles: u64,
    /// Distinct blobs stored.
    pub blobs: u64,
    pub blob_bytes: u64,
    /// Entries that share a blob stored for an earlier one, and the bytes that saved.
    pub deduped: u64,
    pub deduped_bytes: u64,
    pub file_len: u64,
}

/// Writes a pack. Add tiles in a deterministic order (plan §8: the same inputs give the same
/// bytes); the index is sorted at `finish`. Until then the header's index offset is zero, so a
/// half-written pack never parses. After an error the writer refuses further use.
pub struct PackWriter {
    w: BufWriter<File>,
    pos: u64,
    gzip: bool,
    index: Vec<Entry>,
    keys: HashSet<u64>,
    /// (XXH3, length) → offsets of the distinct blobs stored with them.
    blobs: HashMap<(u64, u32), Vec<u64>>,
    stats: PackStats,
    failed: bool,
}

impl PackWriter {
    /// Starts a pack at `path` (replacing any file there). `meta` is the pack's meta JSON
    /// (docs/formats.md: layer, scope, root, encoding, …; zoom ranges are the catalog's).
    pub fn create(path: &Path, meta: Value, gzip_blobs: bool) -> Result<Self> {
        let meta = serde_json::to_vec(&meta)?;
        let mlen = u32::try_from(meta.len()).context("meta too large")?;
        // Read access too: deduplication compares candidates with the bytes already written.
        let f = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path).with_context(|| format!("create {}", path.display()))?;
        let mut w = BufWriter::with_capacity(4 << 20, f);
        let flags = if gzip_blobs { FLAG_GZIP } else { 0 };
        w.write_all(MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&flags.to_le_bytes())?;
        w.write_all(&0u64.to_le_bytes())?; // index offset, set by finish
        w.write_all(&0u64.to_le_bytes())?; // entry count, set by finish
        w.write_all(&mlen.to_le_bytes())?;
        w.write_all(&meta)?;
        let pos = (HEADER_LEN + meta.len()) as u64;
        Ok(Self { w, pos, gzip: gzip_blobs, index: Vec::new(), keys: HashSet::new(), blobs: HashMap::new(), stats: PackStats::default(), failed: false })
    }

    /// Adds tile z/x/y. `blob` is stored as given (gzip'd already when the pack's blobs are);
    /// `raw_len` is its size before compression, 0 if unknown. A blob identical to one stored
    /// before is not stored again.
    pub fn add(&mut self, z: u8, x: u32, y: u32, blob: &[u8], raw_len: u32) -> Result<()> {
        ensure!(!self.failed, "pack writer used after an error");
        ensure!(valid_tile(z, x, y), "no tile {z}/{x}/{y}");
        let key = tile_key(z, x, y);
        ensure!(!self.keys.contains(&key), "tile {z}/{x}/{y} added twice");
        let len = u32::try_from(blob.len()).with_context(|| format!("tile {z}/{x}/{y} is over 4 GiB"))?;
        if self.gzip {
            ensure!(blob.starts_with(&[0x1f, 0x8b]), "tile {z}/{x}/{y} isn't gzip'd, but this pack's blobs are");
        }
        let hash = xxh3(blob);
        let offset = match self.store(hash, blob) {
            Ok(o) => o,
            Err(e) => {
                self.failed = true;
                return Err(e);
            }
        };
        self.keys.insert(key);
        self.index.push(Entry { key, offset, len, raw_len, hash });
        self.stats.tiles += 1;
        Ok(())
    }

    /// The offset of a stored blob byte-identical to `blob`, else of `blob` written now.
    fn store(&mut self, hash: u64, blob: &[u8]) -> Result<u64> {
        let len = blob.len() as u32;
        if let Some(offsets) = self.blobs.get(&(hash, len)) {
            self.w.flush()?;
            let mut buf = vec![0u8; blob.len()];
            for &off in offsets {
                self.w.get_ref().read_exact_at(&mut buf, off)?;
                if buf == blob {
                    self.stats.deduped += 1;
                    self.stats.deduped_bytes += u64::from(len);
                    return Ok(off);
                }
            }
        }
        let off = self.pos;
        self.w.write_all(blob)?;
        self.pos += u64::from(len);
        self.blobs.entry((hash, len)).or_default().push(off);
        self.stats.blobs += 1;
        self.stats.blob_bytes += u64::from(len);
        Ok(off)
    }

    /// Writes the sorted index (8-byte aligned), sets the header's index offset and count, and
    /// syncs the file.
    pub fn finish(mut self) -> Result<PackStats> {
        ensure!(!self.failed, "pack writer used after an error");
        self.index.sort_unstable_by_key(|e| e.key);
        let pad = (8 - self.pos % 8) % 8;
        self.w.write_all(&[0u8; 8][..pad as usize])?;
        let index_off = self.pos + pad;
        self.w.write_all(bytemuck::cast_slice(&self.index))?;
        let f = self.w.into_inner().map_err(|e| e.into_error())?;
        f.write_all_at(&index_off.to_le_bytes(), 16)?;
        f.write_all_at(&(self.index.len() as u64).to_le_bytes(), 24)?;
        f.sync_all()?;
        self.stats.file_len = index_off + (self.index.len() * ENTRY_LEN) as u64;
        Ok(self.stats)
    }
}

/// A pack's header, meta and index: everything needed to find a tile, kept locally per pack.
#[derive(Clone, Debug, PartialEq)]
pub struct PackIndex {
    /// The pack's format version.
    pub version: u32,
    /// `FLAG_GZIP`, or 0.
    pub flags: u32,
    /// The pack's meta JSON (layer, scope, root, zooms, encoding …).
    pub meta: Value,
    /// Where the blobs start (just after the meta JSON).
    pub data_start: u64,
    /// Where the index starts (and the blobs end).
    pub index_offset: u64,
    /// Sorted by key.
    pub entries: Vec<Entry>,
}

/// The fixed header fields.
struct Head {
    version: u32,
    flags: u32,
    index_offset: u64,
    count: u64,
    meta_len: usize,
}

impl Head {
    fn parse(b: &[u8]) -> Result<Self> {
        ensure!(b.len() >= HEADER_LEN && &b[..8] == MAGIC, "not a pack");
        let version = u32::from_le_bytes(b[8..12].try_into()?);
        ensure!((MIN_VERSION..=VERSION).contains(&version), "pack format version {version} isn't supported (this app reads {MIN_VERSION}–{VERSION})");
        Ok(Self {
            version,
            flags: u32::from_le_bytes(b[12..16].try_into()?),
            index_offset: u64::from_le_bytes(b[16..24].try_into()?),
            count: u64::from_le_bytes(b[24..32].try_into()?),
            meta_len: u32::from_le_bytes(b[32..36].try_into()?) as usize,
        })
    }
}

impl PackIndex {
    /// Whether the blobs are gzip'd.
    pub fn gzip(&self) -> bool {
        self.flags & FLAG_GZIP != 0
    }

    /// The entry for tile z/x/y.
    pub fn find(&self, z: u8, x: u32, y: u32) -> Option<Entry> {
        if !valid_tile(z, x, y) {
            return None;
        }
        self.find_key(tile_key(z, x, y))
    }

    pub fn find_key(&self, key: u64) -> Option<Entry> {
        self.entries.binary_search_by_key(&key, |e| e.key).ok().map(|i| self.entries[i])
    }

    /// Parses a pack's first bytes (header and meta; more is fine) and its index bytes.
    pub fn parse(head: &[u8], index: &[u8]) -> Result<Self> {
        let h = Head::parse(head)?;
        let data_start = HEADER_LEN + h.meta_len;
        ensure!(head.len() >= data_start, "pack header cut short");
        let meta = serde_json::from_slice(&head[HEADER_LEN..data_start]).context("pack meta")?;
        ensure!(
            h.count.checked_mul(ENTRY_LEN as u64) == Some(index.len() as u64),
            "pack index is {} bytes, expected {} entries",
            index.len(),
            h.count
        );
        let ix = PackIndex {
            version: h.version,
            flags: h.flags,
            meta,
            data_start: data_start as u64,
            index_offset: h.index_offset,
            entries: bytemuck::pod_collect_to_vec(index),
        };
        ix.validate()?;
        Ok(ix)
    }

    /// Parses the header and index of a whole pack in memory (an mmapped local file).
    pub fn parse_bytes(pack: &[u8]) -> Result<Self> {
        let h = Head::parse(pack)?;
        let data_start = HEADER_LEN + h.meta_len;
        let index = h
            .count
            .checked_mul(ENTRY_LEN as u64)
            .and_then(|n| h.index_offset.checked_add(n))
            .filter(|&end| end <= pack.len() as u64)
            .map(|end| &pack[h.index_offset as usize..end as usize]);
        let Some(index) = index else {
            bail!("pack index ({} entries at {}) runs past the end ({} bytes); unfinished or cut short?", h.count, h.index_offset, pack.len());
        };
        ensure!(data_start <= pack.len(), "pack meta runs past the end");
        Self::parse(&pack[..data_start], index)
    }

    /// Reads the header and index from a pack: two range reads (one for a small pack).
    pub fn read_from(src: &dyn RangeRead) -> Result<Self> {
        let file_len = src.len()?;
        ensure!(file_len >= HEADER_LEN as u64, "not a pack ({file_len} bytes)");
        let first = src.read_at(0, file_len.min(FIRST_READ) as usize)?;
        let h = Head::parse(&first)?;
        let data_start = (HEADER_LEN + h.meta_len) as u64;
        ensure!(data_start <= file_len, "pack meta runs past the end");
        let ilen = h.count.checked_mul(ENTRY_LEN as u64).filter(|&n| h.index_offset.checked_add(n).is_some_and(|end| end <= file_len));
        let Some(ilen) = ilen else {
            bail!("pack index ({} entries at {}) runs past the end ({file_len} bytes); unfinished or cut short?", h.count, h.index_offset);
        };
        let head_owned;
        let head = if first.len() as u64 >= data_start {
            &first[..data_start as usize]
        } else {
            head_owned = src.read_at(0, data_start as usize)?;
            &head_owned[..]
        };
        let index_owned;
        let index = if h.index_offset + ilen <= first.len() as u64 {
            &first[h.index_offset as usize..(h.index_offset + ilen) as usize]
        } else {
            index_owned = src.read_at(h.index_offset, ilen as usize)?;
            &index_owned[..]
        };
        Self::parse(head, index)
    }

    /// The stored bytes of an entry, checked against its XXH3.
    pub fn read_blob(&self, src: &dyn RangeRead, e: &Entry) -> Result<Vec<u8>> {
        let b = src.read_at(e.offset, e.len as usize)?;
        if xxh3(&b) != e.hash {
            let (z, x, y) = e.zxy();
            bail!("tile {z}/{x}/{y}: checksum mismatch (a damaged pack or read)");
        }
        Ok(b)
    }

    /// Tile z/x/y's entry and stored bytes, if the pack has it.
    pub fn get(&self, src: &dyn RangeRead, z: u8, x: u32, y: u32) -> Result<Option<(Entry, Vec<u8>)>> {
        match self.find(z, x, y) {
            Some(e) => Ok(Some((e, self.read_blob(src, &e)?))),
            None => Ok(None),
        }
    }

    fn validate(&self) -> Result<()> {
        ensure!(self.flags & !FLAG_GZIP == 0, "unknown pack flags {:#x}", self.flags);
        ensure!(
            self.index_offset >= self.data_start && self.index_offset.is_multiple_of(8),
            "bad pack index offset {} (blobs start at {}); unfinished?",
            self.index_offset,
            self.data_start
        );
        for w in self.entries.windows(2) {
            ensure!(w[0].key < w[1].key, "pack index not sorted, or a tile appears twice");
        }
        for e in &self.entries {
            let (z, x, y) = e.zxy();
            ensure!(valid_tile(z, x, y) && tile_key(z, x, y) == e.key, "bad tile key {:#x} in pack index", e.key);
            ensure!(
                e.offset >= self.data_start && e.offset.checked_add(u64::from(e.len)).is_some_and(|end| end <= self.index_offset),
                "tile {z}/{x}/{y} points outside the pack's blobs"
            );
        }
        Ok(())
    }

    /// A compact, self-checking encoding of the whole index, for the local cache
    /// (`idx/<file hash>.idx`): magic, header fields, meta, the entries as stored, then the XXH3 of
    /// everything before.
    pub fn to_bytes(&self) -> Vec<u8> {
        let meta = serde_json::to_vec(&self.meta).unwrap_or_else(|_| b"null".to_vec());
        let mut b = Vec::with_capacity(48 + meta.len() + self.entries.len() * ENTRY_LEN);
        b.extend_from_slice(IDX_MAGIC);
        b.extend_from_slice(&self.version.to_le_bytes());
        b.extend_from_slice(&self.flags.to_le_bytes());
        b.extend_from_slice(&self.data_start.to_le_bytes());
        b.extend_from_slice(&self.index_offset.to_le_bytes());
        b.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
        b.extend_from_slice(&(meta.len() as u64).to_le_bytes());
        b.extend_from_slice(&meta);
        b.resize(b.len().next_multiple_of(8), 0);
        b.extend_from_slice(bytemuck::cast_slice(&self.entries));
        let sum = xxh3(&b);
        b.extend_from_slice(&sum.to_le_bytes());
        b
    }

    /// Decodes `to_bytes`; an error for anything damaged or cut short.
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        const FIXED: usize = 48;
        ensure!(b.len() >= FIXED + 8 && &b[..8] == IDX_MAGIC, "not a pack index cache");
        let (body, sum) = b.split_at(b.len() - 8);
        ensure!(xxh3(body) == u64::from_le_bytes(sum.try_into()?), "pack index cache is damaged (checksum)");
        let u32_at = |o: usize| u32::from_le_bytes(body[o..o + 4].try_into().unwrap_or_default());
        let u64_at = |o: usize| u64::from_le_bytes(body[o..o + 8].try_into().unwrap_or_default());
        let (version, flags) = (u32_at(8), u32_at(12));
        let (data_start, index_offset, count, meta_len) = (u64_at(16), u64_at(24), u64_at(32), u64_at(40));
        ensure!((MIN_VERSION..=VERSION).contains(&version), "pack index cache of version {version}");
        let meta_end = usize::try_from(meta_len).ok().and_then(|n| FIXED.checked_add(n)).filter(|&e| e <= body.len()).context("pack index cache cut short")?;
        let meta = serde_json::from_slice(&body[FIXED..meta_end]).context("pack index cache meta")?;
        let start = meta_end.next_multiple_of(8);
        let entries = count.checked_mul(ENTRY_LEN as u64).and_then(|n| (start as u64).checked_add(n));
        ensure!(entries == Some(body.len() as u64), "pack index cache has the wrong size");
        let ix = PackIndex { version, flags, meta, data_start, index_offset, entries: bytemuck::pod_collect_to_vec(&body[start..]) };
        ix.validate()?;
        Ok(ix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::hash16_file;
    use crate::range::MmapFile;
    use flate2::{write::GzEncoder, Compression};
    use serde_json::json;

    fn gz(b: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::new(6));
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    /// Tiles of a small hi pack: z9–10 under 6/32/21, with some repeated content.
    fn tiles() -> Vec<(u8, u32, u32, Vec<u8>)> {
        let mut t = Vec::new();
        for z in 9..=10u8 {
            let s = 1 << (z - 6);
            for x in 32 * s..32 * s + s {
                for y in 21 * s..21 * s + s {
                    // Every third tile is the same "sea" tile.
                    let body = if (x + y) % 3 == 0 { b"sea".to_vec() } else { format!("tile {z}/{x}/{y}").into_bytes() };
                    t.push((z, x, y, body));
                }
            }
        }
        t
    }

    fn write(path: &Path, gzip: bool) -> PackStats {
        let mut w = PackWriter::create(path, json!({"layer": "roads", "scope": "hi", "root": "6/32/21", "minzoom": 9, "maxzoom": 10, "encoding": "rt7"}), gzip).unwrap();
        for (z, x, y, body) in tiles() {
            let blob = if gzip { gz(&body) } else { body.clone() };
            w.add(z, x, y, &blob, body.len() as u32).unwrap();
        }
        w.finish().unwrap()
    }

    #[test]
    fn round_trip_and_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.pack");
        let stats = write(&p, true);
        let all = tiles();
        let sea = all.iter().filter(|t| t.3 == b"sea").count() as u64;
        assert_eq!(stats.tiles, all.len() as u64);
        assert_eq!(stats.blobs, all.len() as u64 - sea + 1);
        assert_eq!(stats.deduped, sea - 1);
        assert_eq!(stats.file_len, std::fs::metadata(&p).unwrap().len());

        let m = MmapFile::open(&p).unwrap();
        let ix = PackIndex::read_from(&m).unwrap();
        assert!(ix.gzip());
        assert_eq!(ix.meta["root"], "6/32/21");
        assert_eq!(ix.index_offset % 8, 0);
        assert_eq!(ix.entries.len(), all.len());
        let mut sea_offsets = HashSet::new();
        for (z, x, y, body) in &all {
            let (e, blob) = ix.get(&m, *z, *x, *y).unwrap().unwrap();
            assert_eq!(blob, gz(body));
            assert_eq!(e.raw_len as usize, body.len());
            assert_eq!(e.etag(), format!("{:016x}", xxh3(&blob)));
            assert_eq!(e.zxy(), (*z, *x, *y));
            if body == b"sea" {
                sea_offsets.insert(e.offset);
            }
        }
        assert_eq!(sea_offsets.len(), 1, "identical blobs share one offset");
        assert!(ix.find(9, 0, 0).is_none());
        assert!(ix.find(8, 16, 10).is_none());
        assert!(ix.find(30, 0, 0).is_none());

        // The same tiles in the same order give the same bytes.
        let q = dir.path().join("b.pack");
        write(&q, true);
        assert_eq!(hash16_file(&p).unwrap(), hash16_file(&q).unwrap());

        // Read from memory too (a NAS reader goes through the same code), and parsed in place.
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(PackIndex::read_from(&bytes).unwrap(), ix);
        assert_eq!(PackIndex::parse_bytes(m.bytes()).unwrap(), ix);
    }

    #[test]
    fn writer_rejects_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = PackWriter::create(&dir.path().join("x.pack"), json!({}), true).unwrap();
        w.add(3, 1, 2, &gz(b"a"), 1).unwrap();
        assert!(w.add(3, 1, 2, &gz(b"b"), 1).is_err(), "duplicate key");
        assert!(w.add(3, 8, 0, &gz(b"b"), 1).is_err(), "x out of range");
        assert!(w.add(30, 0, 0, &gz(b"b"), 1).is_err(), "zoom too deep");
        assert!(w.add(3, 1, 3, b"plain", 5).is_err(), "not gzip'd");
        w.add(3, 1, 3, &gz(b"b"), 1).unwrap();
        assert_eq!(w.finish().unwrap().tiles, 2);

        // Without gzip, anything goes, empty blobs included.
        let p = dir.path().join("y.pack");
        let mut w = PackWriter::create(&p, json!({"encoding": "terrarium-png"}), false).unwrap();
        w.add(0, 0, 0, b"", 0).unwrap();
        w.add(1, 0, 0, b"", 0).unwrap();
        w.add(1, 1, 1, b"png", 0).unwrap();
        let s = w.finish().unwrap();
        assert_eq!((s.tiles, s.blobs, s.deduped), (3, 2, 1));
        let ix = PackIndex::read_from(&std::fs::read(&p).unwrap()).unwrap();
        assert!(!ix.gzip());
        assert_eq!(ix.find(1, 0, 0).unwrap().len, 0);
    }

    #[test]
    fn same_hash_different_bytes_are_kept_apart() {
        // Pretend "bbb" collides with "aaa": the byte comparison must keep them apart.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.pack");
        let mut w = PackWriter::create(&p, json!({}), false).unwrap();
        w.add(1, 0, 0, b"aaa", 3).unwrap();
        let a_off = w.index[0].offset;
        w.blobs.insert((xxh3(b"bbb"), 3), vec![a_off]);
        w.add(1, 0, 1, b"bbb", 3).unwrap();
        w.add(1, 1, 0, b"bbb", 3).unwrap();
        let s = w.finish().unwrap();
        assert_eq!((s.blobs, s.deduped), (2, 1));
        let bytes = std::fs::read(&p).unwrap();
        let ix = PackIndex::read_from(&bytes).unwrap();
        assert_eq!(ix.get(&bytes, 1, 0, 0).unwrap().unwrap().1, b"aaa");
        assert_eq!(ix.get(&bytes, 1, 0, 1).unwrap().unwrap().1, b"bbb");
        assert_eq!(ix.find(1, 0, 1).unwrap().offset, ix.find(1, 1, 0).unwrap().offset);
    }

    #[test]
    fn damaged_packs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.pack");
        write(&p, true);
        let good = std::fs::read(&p).unwrap();

        // Cut short: the index runs past the end.
        let cut = good[..good.len() - 5].to_vec();
        assert!(PackIndex::read_from(&cut).is_err());
        assert!(PackIndex::parse_bytes(&cut).is_err());
        assert!(PackIndex::parse_bytes(&good[..20]).is_err());

        // Never finished: the header still says index offset 0.
        let q = dir.path().join("b.pack");
        let mut w = PackWriter::create(&q, json!({}), false).unwrap();
        w.add(0, 0, 0, b"x", 1).unwrap();
        drop(w);
        assert!(PackIndex::read_from(&std::fs::read(&q).unwrap()).is_err());

        // Not a pack, and a future version.
        assert!(PackIndex::read_from(&b"RDTILES1............................................".to_vec()).is_err());
        let mut v2 = good.clone();
        v2[8] = 2;
        assert!(PackIndex::read_from(&v2).unwrap_err().to_string().contains("version 2"));

        // A flipped byte in a blob fails that tile's checksum, not the index.
        let ix = PackIndex::read_from(&good).unwrap();
        let e = ix.entries[5];
        let mut bad = good.clone();
        bad[e.offset as usize + 3] ^= 0x40;
        let ix2 = PackIndex::read_from(&bad).unwrap();
        assert!(ix2.read_blob(&bad, &e).unwrap_err().to_string().contains("checksum"));
        let other = ix.entries.iter().find(|o| o.offset != e.offset).unwrap();
        assert!(ix2.read_blob(&bad, other).is_ok());

        // An index entry pointing outside the blobs.
        let mut bad = good.clone();
        let at = ix.index_offset as usize + 8; // first entry's offset field
        bad[at..at + 8].copy_from_slice(&(good.len() as u64).to_le_bytes());
        assert!(PackIndex::read_from(&bad).is_err());
    }

    #[test]
    fn big_meta_takes_another_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.pack");
        let long = "x".repeat(100_000);
        let mut w = PackWriter::create(&p, json!({"note": long}), false).unwrap();
        w.add(2, 1, 1, b"t", 1).unwrap();
        w.finish().unwrap();
        let ix = PackIndex::read_from(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(ix.meta["note"].as_str().unwrap().len(), 100_000);
        assert!(ix.find(2, 1, 1).is_some());
    }

    #[test]
    fn index_cache_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.pack");
        write(&p, true);
        let ix = PackIndex::read_from(&std::fs::read(&p).unwrap()).unwrap();
        let b = ix.to_bytes();
        assert_eq!(PackIndex::from_bytes(&b).unwrap(), ix);
        for cut in [0, 7, 40, b.len() / 2, b.len() - 1] {
            assert!(PackIndex::from_bytes(&b[..cut]).is_err(), "cut at {cut}");
        }
        let mut bad = b.clone();
        bad[60] ^= 1;
        assert!(PackIndex::from_bytes(&bad).is_err());
        // An empty pack round-trips too.
        let q = dir.path().join("e.pack");
        PackWriter::create(&q, json!(null), false).unwrap().finish().unwrap();
        let e = PackIndex::read_from(&std::fs::read(&q).unwrap()).unwrap();
        assert!(e.entries.is_empty());
        assert_eq!(PackIndex::from_bytes(&e.to_bytes()).unwrap(), e);
    }

    #[test]
    fn keys() {
        for (z, x, y) in [(0, 0, 0), (6, 32, 21), (14, 16383, 1), (29, (1 << 29) - 1, 7)] {
            assert_eq!(tile_zxy(tile_key(z, x, y)), (z, x, y));
            assert!(valid_tile(z, x, y));
        }
        assert!(!valid_tile(0, 1, 0));
        assert!(!valid_tile(30, 0, 0));
    }
}
