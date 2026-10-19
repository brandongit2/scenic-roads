//! A base pack (and its road values) read only where some areas need it (Task #396): what a pack
//! or lo job takes of a neighbour's base pack is the few ways reaching its tiles, so it reads those
//! byte ranges from the NAS rather than copying the whole file.
//!
//! The ways are cut into chunks of `CHUNK` ways in file order (sorted by the z9 tile and Morton
//! order of their first vertex, docs/formats.md "Base pack", so a chunk's ways lie together), each
//! with the box of its ways' boxes and where its samples start: the pack's *way index*, kept on
//! this Mac by content name (`index_path`, a few kB) once worked out from its `ways`, `verts` and
//! `samples`. The chunks whose box meets an area are read: their ways' records, vertices and
//! per-vertex values, samples, and road values; the small sections (`strings`, `rail`, `sub9`)
//! whole; nothing of the sections pack and lo don't read (`raw`, `src`). Every way whose box meets
//! an area is in such a chunk, so what the job makes is what it made from the whole file.
//!
//! The ranges are copied into a sparse local file the size of the original, at their own offsets,
//! then mapped as a whole file is (crate::basepack::Sect::open_only, only the sections read named):
//! the bytes not read are zeros and take no room. Neighbouring ranges are read as one (`GAP`), in
//! reads of `READ` bytes at most, `READERS` at once. Sections read whole are checked against their
//! XXH3; ranges can't be (a file's sections are checksummed whole).

use crate::basepack::{BasePack, Sect, Span};
use crate::hipack::{meets, Bounds};
use anyhow::{ensure, Context, Result};
use roadcore::scenic::Sample;
use roadcore::WayRec;
use std::fs::File;
use std::path::{Path, PathBuf};
use store::sys::PosIo;

/// Ways per chunk of the way index.
pub const CHUNK: usize = 1024;
/// Ranges of a file closer than this are read as one.
pub const GAP: u64 = 1 << 20;
/// The most read at once.
const READ: usize = 8 << 20;
/// Reads of one file at once.
const READERS: usize = 4;
const MAGIC: &[u8; 8] = b"WAYIDX01";
/// The box of nothing (meets no area).
const EMPTY: Bounds = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];

/// The sections read only where the chunks are, by vertex.
const PER_VERTEX: [&str; 6] = ["verts", "elevu", "elev", "grade", "scenic", "drape"];
/// The sections read whole (small).
const WHOLE: [&str; 3] = ["strings", "rail", "sub9"];
/// The sections the way index is worked out from, read whole (and checked) when it is.
const INDEXED: [&str; 3] = ["ways", "verts", "samples"];

/// What was read from the source files: bytes, and reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Read {
    pub bytes: u64,
    pub reads: u64,
}

impl std::ops::AddAssign for Read {
    fn add_assign(&mut self, o: Read) {
        self.bytes += o.bytes;
        self.reads += o.reads;
    }
}

/// A base pack's way index: each chunk's box (`CHUNK` ways at a time) and where its samples start
/// (one more: the samples' count).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    pub boxes: Vec<Bounds>,
    pub samples: Vec<u64>,
}

impl Index {
    /// From the ways, their vertices and samples (grouped by way, in way order).
    pub fn of(ways: &[WayRec], verts: &[[i32; 2]], samples: &[Sample]) -> Result<Index> {
        let boxes = ways
            .chunks(CHUNK)
            .map(|c| {
                let mut b = EMPTY;
                for w in c {
                    let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
                    for p in verts.get(r).context("a way's vertices past the pack's")? {
                        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                    }
                }
                Ok(b)
            })
            .collect::<Result<Vec<_>>>()?;
        let samples =
            (0..=boxes.len()).map(|i| samples.partition_point(|s| (s.way as usize) < i * CHUNK) as u64).collect();
        Ok(Index { boxes, samples })
    }

    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(16 + self.boxes.len() * 24 + 8);
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&(CHUNK as u32).to_le_bytes());
        b.extend_from_slice(&(self.boxes.len() as u32).to_le_bytes());
        b.extend_from_slice(bytemuck::cast_slice(&self.boxes));
        b.extend_from_slice(bytemuck::cast_slice(&self.samples));
        b
    }

    /// Kept bytes `b`, for a pack of `chunks` chunks; None when they're another's.
    fn decode(b: &[u8], chunks: usize) -> Option<Index> {
        let ok = b.len() == 16 + chunks * 16 + (chunks + 1) * 8
            && &b[..8] == MAGIC
            && u32::from_le_bytes(b[8..12].try_into().ok()?) as usize == CHUNK
            && u32::from_le_bytes(b[12..16].try_into().ok()?) as usize == chunks;
        let mid = 16 + chunks * 16;
        ok.then(|| Index {
            boxes: bytemuck::pod_collect_to_vec(&b[16..mid]),
            samples: bytemuck::pod_collect_to_vec(&b[mid..]),
        })
    }
}

/// Where this Mac keeps the way index of the base pack named `content` (its content name) in
/// cache folder `cache`.
pub fn index_path(cache: &Path, content: &str) -> PathBuf {
    cache.join(format!("{content}.idx"))
}

/// A source file being copied in part to a sparse local one of its size.
struct Copy {
    src: File,
    dst: File,
    len: u64,
    ranges: Vec<(u64, u64)>,
    read: Read,
}

impl Copy {
    fn new(src: &Path, dst: &Path) -> Result<Copy> {
        let src = File::open(src).with_context(|| format!("open {}", src.display()))?;
        let len = src.metadata()?.len();
        let dst = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)
            .with_context(|| format!("create {}", dst.display()))?;
        dst.set_len(len)?;
        Ok(Copy { src, dst, len, ranges: Vec::new(), read: Read::default() })
    }

    /// Bytes `[off, off + len)` of the source, read now (not copied).
    fn get(&mut self, off: u64, len: usize) -> Result<Vec<u8>> {
        ensure!(off + len as u64 <= self.len, "a read past the end of the file");
        let mut b = vec![0u8; len];
        self.src.read_exact_at(&mut b, off)?;
        self.read += Read { bytes: len as u64, reads: 1 };
        Ok(b)
    }

    /// Bytes `b` of the source at `off` (read with `get`), into the copy.
    fn put(&mut self, off: u64, b: &[u8]) -> Result<()> {
        self.dst.write_all_at(b, off)?;
        Ok(())
    }

    /// Bytes `[off, off + len)` of the copy (copied already).
    fn local(&self, off: u64, len: usize) -> Result<Vec<u8>> {
        let mut b = vec![0u8; len];
        self.dst.read_exact_at(&mut b, off)?;
        Ok(b)
    }

    /// Bytes `[a, b)` of the source to copy (`flush`).
    fn want(&mut self, a: u64, b: u64) {
        if a < b {
            self.ranges.push((a, b.min(self.len)));
        }
    }

    /// The ranges wanted so far, neighbouring ones together, copied: in pieces of `READ` bytes at
    /// most, `READERS` at once.
    fn flush(&mut self) -> Result<()> {
        let mut rs = std::mem::take(&mut self.ranges);
        rs.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (a, b) in rs {
            match merged.last_mut() {
                Some(m) if a <= m.1 + GAP => m.1 = m.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        let pieces: Vec<(u64, usize)> = merged
            .iter()
            .flat_map(|&(a, b)| (a..b).step_by(READ).map(move |at| (at, ((b - at) as usize).min(READ))))
            .collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (src, dst) = (&self.src, &self.dst);
        let done: Vec<Result<Read>> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..READERS.min(pieces.len()))
                .map(|_| {
                    s.spawn(|| -> Result<Read> {
                        let mut buf = vec![0u8; READ];
                        let mut r = Read::default();
                        loop {
                            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(&(at, n)) = pieces.get(k) else { return Ok(r) };
                            src.read_exact_at(&mut buf[..n], at)?;
                            dst.write_all_at(&buf[..n], at)?;
                            r += Read { bytes: n as u64, reads: 1 };
                        }
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("a reader panicked")))).collect()
        });
        for r in done {
            self.read += r?;
        }
        Ok(())
    }
}

/// A sectioned file's section table, (offset, length, XXH3) by name, and its header's range (the
/// header and table copied).
struct Layout {
    sections: std::collections::HashMap<String, (u64, u64, u64)>,
    head: (u64, u64),
}

fn layout(c: &mut Copy) -> Result<Layout> {
    ensure!(c.len >= 28, "not a sectioned file");
    let h = c.get(0, 28)?;
    ensure!(&h[..8] == b"RDSECT01", "not a sectioned file");
    let count = u32::from_le_bytes(h[12..16].try_into()?) as u64;
    let table = u64::from_le_bytes(h[16..24].try_into()?);
    let mlen = u32::from_le_bytes(h[24..28].try_into()?) as u64;
    let t = c.get(table, (count * 48) as usize)?;
    let mut sections = std::collections::HashMap::new();
    for e in t.chunks_exact(48) {
        let name = String::from_utf8_lossy(&e[..24]).trim_end_matches('\0').to_string();
        let off = u64::from_le_bytes(e[24..32].try_into()?);
        let len = u64::from_le_bytes(e[32..40].try_into()?);
        let hash = u64::from_le_bytes(e[40..48].try_into()?);
        ensure!(off + len <= c.len, "section {name} out of bounds");
        sections.insert(name, (off, len, hash));
    }
    c.put(0, &h)?;
    c.put(table, &t)?;
    Ok(Layout { sections, head: (0, 28 + mlen) })
}

/// Base pack `base` and road values `roads` (files, on the NAS or anywhere), read where chunks
/// meeting `areas` are (every chunk: None), copied into local files `to` (sparse) and opened. Its
/// way index is read from, or kept in, `index` (`index_path`). What was read of the two files.
pub fn open(
    base: &Path,
    roads: &Path,
    index: &Path,
    to: [&Path; 2],
    areas: Option<&[Bounds]>,
) -> Result<(BasePack, Read)> {
    let [bdst, rdst] = to;
    let mut c = Copy::new(base, bdst)?;
    let l = layout(&mut c)?;
    let sec = |n: &str| l.sections.get(n).copied();
    let (woff, wlen, _) = sec("ways").context("no section ways")?;
    let (_, vlen, _) = sec("verts").context("no section verts")?;
    const W: u64 = std::mem::size_of::<WayRec>() as u64;
    const S: u64 = std::mem::size_of::<Sample>() as u64;
    ensure!(wlen % W == 0 && vlen % 8 == 0, "ways or verts not whole records");
    let (nways, nverts) = ((wlen / W) as usize, (vlen / 8) as usize);
    let nsamples = sec("samples").map_or(0, |s| s.1 / S);
    let chunks = nways.div_ceil(CHUNK);
    // (The way index: kept, else worked out from the whole ways, vertices and samples, those then
    // copied whole.)
    let held = store::cachefile::read(index).ok().flatten();
    let kept = held.as_deref().and_then(|b| Index::decode(b, chunks));
    if held.is_some() && kept.is_none() {
        // (Another chunking's, or cut short: made again.)
        store::cachefile::try_remove(index);
    }
    let mut whole: Vec<&str> = Vec::new();
    let ix = match kept {
        Some(ix) => ix,
        None => {
            let mut got: Vec<Vec<u8>> = Vec::new();
            for n in INDEXED {
                let Some((off, len, hash)) = sec(n) else {
                    got.push(Vec::new());
                    continue;
                };
                let b = c.get(off, len as usize)?;
                ensure!(store::naming::xxh3(&b) == hash, "section {n} is damaged (checksum mismatch)");
                c.put(off, &b)?;
                whole.push(n);
                got.push(b);
            }
            let ix = Index::of(
                &bytemuck::pod_collect_to_vec::<u8, WayRec>(&got[0]),
                &bytemuck::pod_collect_to_vec::<u8, [i32; 2]>(&got[1]),
                &bytemuck::pod_collect_to_vec::<u8, Sample>(&got[2]),
            )?;
            if let Some(d) = index.parent() {
                std::fs::create_dir_all(d)?;
            }
            store::cachefile::put(index, &ix.encode()).with_context(|| format!("keep {}", index.display()))?;
            ix
        }
    };
    ensure!(ix.samples.last().copied() == Some(nsamples) || chunks == 0, "the way index's samples out of step");
    // (The chunks meeting the areas, as spans of chunks, neighbours together.)
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, b) in ix.boxes.iter().enumerate() {
        if !areas.is_none_or(|a| a.iter().any(|a| meets(*b, *a))) {
            continue;
        }
        match spans.last_mut() {
            Some(s) if s.end == i => s.end = i + 1,
            _ => spans.push(i..i + 1),
        }
    }
    let ways_of = |s: &std::ops::Range<usize>| s.start * CHUNK..(s.end * CHUNK).min(nways);
    // (Their ways' records first, for the vertices they span.)
    if !whole.contains(&"ways") {
        for s in &spans {
            let w = ways_of(s);
            c.want(woff + w.start as u64 * W, woff + w.end as u64 * W);
        }
        c.flush()?;
    }
    let mut vr: Vec<(u64, u64)> = Vec::with_capacity(spans.len());
    for s in &spans {
        let w = ways_of(s);
        let recs: Vec<WayRec> =
            bytemuck::pod_collect_to_vec(&c.local(woff + w.start as u64 * W, (w.len() as u64 * W) as usize)?);
        let a = recs.iter().map(|w| w.vstart).min().unwrap_or(0);
        let b = recs.iter().map(|w| w.vstart + w.vcount as u64).max().unwrap_or(0);
        ensure!(b as usize <= nverts, "a way's vertices past the pack's");
        vr.push((a, b));
    }
    for n in PER_VERTEX {
        let Some((off, len, _)) = sec(n) else { continue };
        if whole.contains(&n) {
            continue;
        }
        ensure!(len == 0 || nverts > 0 && len % nverts as u64 == 0, "section {n} isn't one record a vertex");
        let e = if nverts == 0 { 0 } else { len / nverts as u64 };
        for &(a, b) in &vr {
            c.want(off + a * e, off + b * e);
        }
    }
    let out_spans: Vec<Span> = spans
        .iter()
        .map(|s| {
            let w = ways_of(s);
            Span {
                ways: w.start as u32..w.end as u32,
                samples: ix.samples[s.start] as usize..ix.samples[s.end] as usize,
            }
        })
        .collect();
    for n in ["samples", "samplech"] {
        let Some((off, len, _)) = sec(n) else { continue };
        if whole.contains(&n) {
            continue;
        }
        let e = if nsamples > 0 { len / nsamples } else { 0 };
        for s in &out_spans {
            c.want(off + s.samples.start as u64 * e, off + s.samples.end as u64 * e);
        }
    }
    for n in WHOLE {
        if let Some((off, len, _)) = sec(n) {
            c.want(off, off + len);
        }
    }
    c.want(l.head.0, l.head.1);
    c.flush()?;
    let mut read = c.read;
    // The road values: a record a way.
    let mut rc = Copy::new(roads, rdst)?;
    let rl = layout(&mut rc)?;
    let (roff, rlen, _) = rl.sections.get("roads").copied().context("no section roads in the road values")?;
    ensure!(rlen == nways as u64 * 24, "road values out of step with the base pack");
    for s in &out_spans {
        rc.want(roff + s.ways.start as u64 * 24, roff + s.ways.end as u64 * 24);
    }
    rc.want(rl.head.0, rl.head.1);
    rc.flush()?;
    read += rc.read;
    // (Only what was read named: the rest isn't there.)
    let here = |n: &str| INDEXED.contains(&n) || PER_VERTEX.contains(&n) || WHOLE.contains(&n) || n == "samplech";
    let sect = Sect::open_only(bdst, &here)?;
    let rsect = Sect::open_only(rdst, &|n| n == "roads")?;
    // (Every way read: no spans needed.)
    let all = out_spans.len() == 1 && out_spans[0].ways == (0..nways as u32);
    let bp = BasePack::from_sects(sect, rsect).with_context(|| format!("{}", base.display()))?;
    Ok((if all || nways == 0 { bp } else { bp.with_spans(out_spans) }, read))
}

#[cfg(test)]
mod tests;
