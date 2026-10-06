//! A z6 tile's normalized buildings (`work/bld/6-<x>-<y>.<h>.sect`, RDSECT v1; docs/formats.md):
//! what `bldprep` writes and `bldtiles` reads, a z14 tile's records at a time.
//!
//! Meta: `{"fmt": 1, "tile": "6/x/y", "release", "buildings", "parts", "srcs", "classes",
//! "subtypes", "roofs", "ghsl": "R2023A", "read"}`: the counts, the strings the records' codes
//! index (code 0: none; code k: the list's k-th, each list sorted), and what was read.
//!
//! Sections:
//! - `index`: [`IndexEntry`] per block, sorted by key.
//! - `blocks`: a zstd block (level [`ZSTD_LEVEL`]) per z14 tile: the buildings and parts whose
//!   centroid is in the tile, sorted by Overture id, column by column ([`Block`]).

use anyhow::{bail, ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use std::path::Path;
use store::range::PlainFile;
use store::sect::SectReader;

/// The blocks' zstd level (fixed: the same bytes everywhere, plan.md §8).
pub const ZSTD_LEVEL: i32 = 9;

/// A block's place: its z14 tile key, where it is in `blocks`, its length and its records.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct IndexEntry {
    pub key: u64,
    pub offset: u64,
    pub len: u32,
    pub count: u32,
}

const _: () = assert!(std::mem::size_of::<IndexEntry>() == 24);

/// A record's flags.
pub mod flag {
    /// A building part (Overture's `building_part`: OSM's `building:part`).
    pub const PART: u8 = 1;
    /// A building whose parts are in the files (`has_parts`, and at least one of its parts read):
    /// drawn by its parts, its outline by the flat layer only.
    pub const HAS_PARTS: u8 = 2;
}

/// The OSM id's type, in its top two bits.
pub mod osm {
    pub const WAY: u64 = 1 << 62;
    pub const RELATION: u64 = 2 << 62;
    pub const ID: u64 = (1 << 62) - 1;
}

/// A block's records, column by column. Vertices are E7; each record's rings, the first of each
/// polygon its exterior (as Overture's WKB had them, the closing point left out).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    /// Centroid (E7).
    pub cen: Vec<[i32; 2]>,
    /// Footprint area, m².
    pub area: Vec<f32>,
    /// Per record: its rings; per polygon, its rings (the first the exterior).
    pub npolys: Vec<u32>,
    /// Per polygon: its rings.
    pub nrings: Vec<u32>,
    /// Per ring: its vertices.
    pub ring_len: Vec<u32>,
    /// The vertices (stored: each ring's first absolute, the rest as deltas).
    pub verts: Vec<[i32; 2]>,
    /// Height and base, decimetres (0: none); floors and base floor (0: none).
    pub h: Vec<u16>,
    pub m: Vec<u16>,
    pub f: Vec<u8>,
    pub mf: Vec<u8>,
    /// Overture's class, subtype and roof shape (codes into the meta's lists), [`flag`]s, and the
    /// height's source dataset (into `srcs`).
    pub class: Vec<u8>,
    pub subtype: Vec<u8>,
    pub roof: Vec<u8>,
    pub flags: Vec<u8>,
    pub hsrc: Vec<u8>,
    /// GHSL's average building height at the centroid, decimetres (0: none).
    pub ghsl: Vec<u16>,
    /// The OSM way or relation that gave the footprint ([`osm`]; 0: not OSM's).
    pub osm: Vec<u64>,
    /// Per record, its first polygon; per polygon, its first ring; per ring, its first vertex
    /// (from the counts: [`Block::index`]).
    pub poly0: Vec<u32>,
    pub ring0: Vec<u32>,
    pub vert0: Vec<u32>,
}

/// A little-endian reader over a block's bytes.
struct Cur<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len()).context("a building block cut short")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn vec<T: Pod>(&mut self, n: usize) -> Result<Vec<T>> {
        let s = self.take(n.checked_mul(std::mem::size_of::<T>()).context("a building block's count overflows")?)?;
        Ok(bytemuck::pod_collect_to_vec(s))
    }
}

impl Block {
    pub fn len(&self) -> usize {
        self.cen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cen.is_empty()
    }

    /// The first-polygon, first-ring and first-vertex indexes, from the counts.
    pub fn index(&mut self) {
        let prefix = |v: &[u32]| {
            let mut out = Vec::with_capacity(v.len() + 1);
            let mut s = 0u32;
            out.push(0);
            for &x in v {
                s += x;
                out.push(s);
            }
            out
        };
        self.poly0 = prefix(&self.npolys);
        self.ring0 = prefix(&self.nrings);
        self.vert0 = prefix(&self.ring_len);
    }

    /// Record `i`'s polygons, each as its rings' vertices (indexed: [`Block::index`]).
    pub fn polygons(&self, i: usize) -> impl Iterator<Item = impl Iterator<Item = &[[i32; 2]]> + '_> + '_ {
        (self.poly0[i]..self.poly0[i + 1]).map(move |p| (self.ring0[p as usize]..self.ring0[p as usize + 1]).map(move |r| &self.verts[self.vert0[r as usize] as usize..self.vert0[r as usize + 1] as usize]))
    }

    /// Record `i`'s vertices, every ring's.
    pub fn verts_of(&self, i: usize) -> &[[i32; 2]] {
        let r0 = self.ring0[self.poly0[i] as usize] as usize;
        let r1 = self.ring0[self.poly0[i + 1] as usize] as usize;
        &self.verts[self.vert0[r0] as usize..self.vert0[r1] as usize]
    }

    /// The block's bytes before compression: counts (records, polygons, rings, vertices), then
    /// each column (centroids, areas, polygons per record, rings per polygon, vertices per ring,
    /// vertices, h, m, f, mf, class, subtype, roof, flags, hsrc, ghsl, osm), little-endian.
    pub fn encode(&self) -> Vec<u8> {
        let n = self.len();
        let mut out = Vec::with_capacity(16 + n * 40 + self.verts.len() * 8);
        for c in [n, self.nrings.len(), self.ring_len.len(), self.verts.len()] {
            out.extend_from_slice(&(c as u32).to_le_bytes());
        }
        out.extend_from_slice(bytemuck::cast_slice(&self.cen));
        out.extend_from_slice(bytemuck::cast_slice(&self.area));
        out.extend_from_slice(bytemuck::cast_slice(&self.npolys));
        out.extend_from_slice(bytemuck::cast_slice(&self.nrings));
        out.extend_from_slice(bytemuck::cast_slice(&self.ring_len));
        let mut at = 0usize;
        for &len in &self.ring_len {
            let ring = &self.verts[at..at + len as usize];
            let mut prev = [0i32; 2];
            for (k, v) in ring.iter().enumerate() {
                let d = if k == 0 { *v } else { [v[0].wrapping_sub(prev[0]), v[1].wrapping_sub(prev[1])] };
                out.extend_from_slice(&d[0].to_le_bytes());
                out.extend_from_slice(&d[1].to_le_bytes());
                prev = *v;
            }
            at += len as usize;
        }
        for col in [&self.h, &self.m] {
            out.extend_from_slice(bytemuck::cast_slice(col));
        }
        for col in [&self.f, &self.mf, &self.class, &self.subtype, &self.roof, &self.flags, &self.hsrc] {
            out.extend_from_slice(col);
        }
        out.extend_from_slice(bytemuck::cast_slice(&self.ghsl));
        out.extend_from_slice(bytemuck::cast_slice(&self.osm));
        out
    }

    /// A block from its bytes (`encode`'s), indexed.
    pub fn decode(b: &[u8]) -> Result<Block> {
        let mut c = Cur { b, at: 0 };
        let (n, np, nr, nv) = (c.u32()? as usize, c.u32()? as usize, c.u32()? as usize, c.u32()? as usize);
        let mut k = Block { cen: c.vec(n)?, area: c.vec(n)?, npolys: c.vec(n)?, nrings: c.vec(np)?, ring_len: c.vec(nr)?, ..Default::default() };
        ensure!(k.npolys.iter().map(|&x| x as usize).sum::<usize>() == np, "a building block's polygons don't add up");
        ensure!(k.nrings.iter().map(|&x| x as usize).sum::<usize>() == nr, "a building block's rings don't add up");
        ensure!(k.ring_len.iter().map(|&x| x as usize).sum::<usize>() == nv, "a building block's vertices don't add up");
        let raw: Vec<[i32; 2]> = c.vec(nv)?;
        k.verts = Vec::with_capacity(nv);
        let mut at = 0usize;
        for &len in &k.ring_len {
            let mut prev = [0i32; 2];
            for (j, d) in raw[at..at + len as usize].iter().enumerate() {
                let v = if j == 0 { *d } else { [prev[0].wrapping_add(d[0]), prev[1].wrapping_add(d[1])] };
                k.verts.push(v);
                prev = v;
            }
            at += len as usize;
        }
        k.h = c.vec(n)?;
        k.m = c.vec(n)?;
        k.f = c.vec(n)?;
        k.mf = c.vec(n)?;
        k.class = c.vec(n)?;
        k.subtype = c.vec(n)?;
        k.roof = c.vec(n)?;
        k.flags = c.vec(n)?;
        k.hsrc = c.vec(n)?;
        k.ghsl = c.vec(n)?;
        k.osm = c.vec(n)?;
        ensure!(c.at == b.len(), "a building block has {} bytes too many", b.len() - c.at);
        k.index();
        Ok(k)
    }
}

/// A normalized file's meta (its lists of strings: code k is the list's (k − 1)th).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Meta {
    pub fmt: u32,
    pub tile: String,
    pub release: String,
    pub buildings: u64,
    pub parts: u64,
    pub srcs: Vec<String>,
    pub classes: Vec<String>,
    pub subtypes: Vec<String>,
    pub roofs: Vec<String>,
    pub ghsl: String,
    /// What bldprep.py read: the files, their row groups, the GHSL tiles.
    #[serde(default)]
    pub read: serde_json::Value,
}

impl Meta {
    /// The string a code stands for ("" for 0, or one past the list).
    pub fn name(list: &[String], code: u8) -> &str {
        if code == 0 {
            ""
        } else {
            list.get(code as usize - 1).map(String::as_str).unwrap_or("")
        }
    }
}

/// A normalized file, open: its meta and index; blocks read on demand.
pub struct WorkFile {
    r: SectReader<PlainFile>,
    pub meta: Meta,
    pub index: Vec<IndexEntry>,
}

impl WorkFile {
    pub fn open(path: &Path) -> Result<WorkFile> {
        let f = PlainFile::open(path).with_context(|| format!("open {}", path.display()))?;
        let r = SectReader::open(f).with_context(|| format!("read {}", path.display()))?;
        let meta: Meta = serde_json::from_value(r.meta().clone()).with_context(|| format!("{}: its meta", path.display()))?;
        ensure!(meta.fmt == 1, "{}: normalized buildings fmt {} (this reads 1)", path.display(), meta.fmt);
        let index: Vec<IndexEntry> = r.read_pod("index")?;
        ensure!(index.windows(2).all(|w| w[0].key < w[1].key), "{}: its index isn't sorted", path.display());
        Ok(WorkFile { r, meta, index })
    }

    /// The block of the z14 tile `key`, if it has one.
    pub fn find(&self, key: u64) -> Option<&IndexEntry> {
        self.index.binary_search_by_key(&key, |e| e.key).ok().map(|i| &self.index[i])
    }

    /// A block, read and decoded.
    pub fn block(&self, e: &IndexEntry) -> Result<Block> {
        let z = self.r.read_part("blocks", e.offset, e.len as usize)?;
        let raw = zstd::decode_all(&z[..]).context("a building block's zstd")?;
        let b = Block::decode(&raw)?;
        if b.len() != e.count as usize {
            bail!("a building block holds {} records, its index says {}", b.len(), e.count);
        }
        Ok(b)
    }
}

/// Writes a normalized file: its meta, then the blocks (z14 tile key, zstd'd bytes, records), in
/// key order.
pub fn write(path: &Path, meta: &Meta, blocks: &mut dyn Iterator<Item = Result<(u64, Vec<u8>, u32)>>) -> Result<()> {
    let mut w = store::sect::SectWriter::create(path, serde_json::to_value(meta)?)?;
    let mut index: Vec<IndexEntry> = Vec::new();
    w.add_with("blocks", |s| {
        let mut at = 0u64;
        for b in blocks {
            let (key, z, count) = b?;
            ensure!(index.last().is_none_or(|e| e.key < key), "building blocks out of order");
            std::io::Write::write_all(s, &z)?;
            index.push(IndexEntry { key, offset: at, len: u32::try_from(z.len()).context("a building block over 4 GiB")?, count });
            at += z.len() as u64;
        }
        Ok(())
    })?;
    w.add_pod("index", &index)?;
    w.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Block {
        let mut b = Block {
            cen: vec![[10, 20], [-5, 7]],
            area: vec![120.5, 30.0],
            npolys: vec![1, 2],
            nrings: vec![2, 1, 1],
            ring_len: vec![4, 3, 3, 4],
            verts: vec![
                [0, 0], [100, 0], [100, 100], [0, 100],
                [10, 10], [20, 10], [20, 20],
                [-50, -50], [-40, -50], [-40, -40],
                [i32::MAX - 1, i32::MIN + 1], [i32::MIN + 2, i32::MAX - 3], [0, 0], [5, -5],
            ],
            h: vec![120, 0],
            m: vec![0, 30],
            f: vec![4, 0],
            mf: vec![0, 1],
            class: vec![2, 0],
            subtype: vec![1, 0],
            roof: vec![0, 3],
            flags: vec![0, flag::PART],
            hsrc: vec![1, 2],
            ghsl: vec![0, 215],
            osm: vec![osm::WAY | 123, 0],
            ..Default::default()
        };
        b.index();
        b
    }

    #[test]
    fn blocks_round_trip() {
        let b = sample();
        let d = Block::decode(&b.encode()).unwrap();
        assert_eq!(d, b);
        let p: Vec<Vec<Vec<[i32; 2]>>> = d.polygons(1).map(|p| p.map(<[[i32; 2]]>::to_vec).collect()).collect();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0], vec![vec![[-50, -50], [-40, -50], [-40, -40]]]);
        assert_eq!(d.verts_of(1).len(), 7);
        assert_eq!(d.verts_of(0).len(), 7);
        // Cut short, or with bytes over: refused.
        let e = b.encode();
        assert!(Block::decode(&e[..e.len() - 1]).is_err());
        assert!(Block::decode(&[e.clone(), vec![0]].concat()).is_err());
    }

    #[test]
    fn files_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("w.sect");
        let b = sample();
        let z = zstd::bulk::compress(&b.encode(), ZSTD_LEVEL).unwrap();
        let meta = Meta { fmt: 1, tile: "6/1/2".into(), srcs: vec!["OpenStreetMap".into()], ..Default::default() };
        write(&p, &meta, &mut vec![Ok((5u64, z.clone(), 2u32)), Ok((9u64, z, 2u32))].into_iter()).unwrap();
        let w = WorkFile::open(&p).unwrap();
        assert_eq!(w.meta.tile, "6/1/2");
        assert_eq!(w.index.len(), 2);
        assert_eq!(w.block(w.find(9).unwrap()).unwrap(), b);
        assert!(w.find(7).is_none());
        assert_eq!(Meta::name(&w.meta.srcs, 1), "OpenStreetMap");
        assert_eq!(Meta::name(&w.meta.srcs, 0), "");
        assert_eq!(Meta::name(&w.meta.srcs, 9), "");
    }
}
