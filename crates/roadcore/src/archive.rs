//! Single-file tile archive: gzip'd tile blobs followed by a sorted index.
//!
//! Layout:
//!   [0..8)    magic "RDTILES1"
//!   [8..16)   u64 index offset
//!   [16..24)  u64 index entry count
//!   [24..28)  u32 metadata JSON length, then the JSON bytes
//!   blobs …
//!   index: `Entry` × count, sorted by key

use anyhow::{bail, Result};
use bytemuck::{Pod, Zeroable};
use crate::Mmap;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"RDTILES1";

/// In data/cache/steps: the terrain tiles the last terrain run changed under the same key
/// (repaired), u64 tile keys. A step that recomputes only where terrain is new also takes these
/// while the list is newer than its own record of its last run.
pub const TERRAIN_REPAIRED: &str = "terrain.repaired";

/// The keys in a TERRAIN_REPAIRED list newer than `since` (a step's record of its last run), if any.
pub fn terrain_repaired_since(steps_dir: &Path, since: &Path) -> Vec<u64> {
    let list = steps_dir.join(TERRAIN_REPAIRED);
    let newer = match (std::fs::metadata(&list).and_then(|m| m.modified()), std::fs::metadata(since).and_then(|m| m.modified())) {
        (Ok(a), Ok(b)) => a > b,
        (Ok(_), Err(_)) => true,
        _ => false,
    };
    if !newer {
        return Vec::new();
    }
    std::fs::read(list).ok().filter(|b| b.len() % 8 == 0).map(|b| bytemuck::cast_slice::<u8, u64>(&b).to_vec()).unwrap_or_default()
}

#[inline]
pub fn tile_key(z: u8, x: u32, y: u32) -> u64 {
    ((z as u64) << 58) | ((x as u64) << 29) | y as u64
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Entry {
    pub key: u64,
    pub offset: u64,
    pub len: u32,
    /// Uncompressed size, useful for stats.
    pub raw_len: u32,
}

pub struct ArchiveWriter {
    w: BufWriter<File>,
    pos: u64,
    index: Vec<Entry>,
}

impl ArchiveWriter {
    pub fn create(path: &Path, meta_json: &str) -> Result<Self> {
        let mut w = BufWriter::with_capacity(8 << 20, File::create(path)?);
        w.write_all(MAGIC)?;
        w.write_all(&[0u8; 16])?;
        w.write_all(&(meta_json.len() as u32).to_le_bytes())?;
        w.write_all(meta_json.as_bytes())?;
        let pos = 28 + meta_json.len() as u64;
        Ok(Self { w, pos, index: Vec::new() })
    }

    pub fn add(&mut self, z: u8, x: u32, y: u32, gz: &[u8], raw_len: usize) -> Result<()> {
        self.w.write_all(gz)?;
        self.index.push(Entry {
            key: tile_key(z, x, y),
            offset: self.pos,
            len: gz.len() as u32,
            raw_len: raw_len as u32,
        });
        self.pos += gz.len() as u64;
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64> {
        self.index.sort_unstable_by_key(|e| e.key);
        // Keep the index 8-byte aligned so it can be cast straight from the mmap.
        let pad = (8 - (self.pos % 8)) % 8;
        self.w.write_all(&vec![0u8; pad as usize])?;
        let index_off = self.pos + pad;
        self.w.write_all(bytemuck::cast_slice(&self.index))?;
        let mut f = self.w.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(8))?;
        f.write_all(&index_off.to_le_bytes())?;
        f.write_all(&(self.index.len() as u64).to_le_bytes())?;
        f.sync_all()?;
        Ok(index_off + (self.index.len() * std::mem::size_of::<Entry>()) as u64)
    }
}

pub struct Archive {
    map: Mmap,
    index_off: usize,
    count: usize,
    pub meta_json: String,
}

impl Archive {
    /// The archive at `path`; an error (never a panic later) when it's cut short or garbled: its
    /// metadata, index and every tile inside it, the index aligned.
    pub fn open(path: &Path) -> Result<Self> {
        let map = crate::mmap(path)?;
        if map.len() < 28 || &map[..8] != MAGIC {
            bail!("{}: not a tile archive", path.display());
        }
        let index_off = u64::from_le_bytes(map[8..16].try_into()?) as usize;
        let count = u64::from_le_bytes(map[16..24].try_into()?) as usize;
        let mlen = u32::from_le_bytes(map[24..28].try_into()?) as usize;
        let index_end = count.checked_mul(std::mem::size_of::<Entry>()).and_then(|n| n.checked_add(index_off));
        if 28 + mlen > index_off || index_off % 8 != 0 || index_end.is_none_or(|e| e > map.len()) {
            bail!("{}: a tile archive cut short or garbled", path.display());
        }
        let meta_json = String::from_utf8(map[28..28 + mlen].to_vec())?;
        let a = Self { map, index_off, count, meta_json };
        if a.entries().iter().any(|e| e.offset.checked_add(e.len as u64).is_none_or(|end| e.offset < 28 || end > index_off as u64)) {
            bail!("{}: a tile archive with a tile outside it", path.display());
        }
        Ok(a)
    }

    pub fn entries(&self) -> &[Entry] {
        let n = self.count * std::mem::size_of::<Entry>();
        bytemuck::cast_slice(&self.map[self.index_off..self.index_off + n])
    }

    pub fn get(&self, z: u8, x: u32, y: u32) -> Option<&[u8]> {
        let key = tile_key(z, x, y);
        let idx = self.entries();
        let i = idx.binary_search_by_key(&key, |e| e.key).ok()?;
        Some(self.get_entry(&idx[i]))
    }

    /// An entry's bytes.
    pub fn get_entry(&self, e: &Entry) -> &[u8] {
        &self.map[e.offset as usize..e.offset as usize + e.len as usize]
    }
}
