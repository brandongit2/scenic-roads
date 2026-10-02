//! Sectioned files (docs/formats.md, RDSECT v1): named arrays in one file, for base packs, hi
//! data, road values and other non-tile data. Every section starts on a 64-byte boundary, so the
//! sections of a local, mmapped file can be cast in place to any record type; a NAS file is read
//! section by section, each checked against its XXH3.

use crate::naming::xxh3;
use crate::range::{Mapped, RangeRead};
use anyhow::{anyhow, bail, ensure, Context, Result};
use bytemuck::{Pod, Zeroable};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use xxhash_rust::xxh3::Xxh3Default;

pub const MAGIC: &[u8; 8] = b"RDSECT01";
/// The format version written.
pub const VERSION: u32 = 1;
/// The oldest version read (plan §8).
pub const MIN_VERSION: u32 = 1;
/// Sections start at multiples of this.
pub const ALIGN: u64 = 64;
/// Longest section name, in bytes.
pub const NAME_LEN: usize = 24;

const HEADER_LEN: usize = 28;
const REC_LEN: usize = 48;
/// The first read of a file: header and meta, and the table too when the file is small.
const FIRST_READ: u64 = 64 << 10;

/// A section table record, as stored.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Rec {
    name: [u8; NAME_LEN],
    offset: u64,
    len: u64,
    hash: u64,
}

const _: () = assert!(std::mem::size_of::<Rec>() == REC_LEN);

/// One section: where it is and its XXH3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub offset: u64,
    pub len: u64,
    pub hash: u64,
}

fn check_name(name: &str) -> Result<()> {
    ensure!(!name.is_empty() && name.len() <= NAME_LEN && !name.contains('\0'), "bad section name {name:?} (1–{NAME_LEN} bytes, no NUL)");
    Ok(())
}

/// Writes a sectioned file. The header's table offset stays zero until `finish`, so a
/// half-written file never parses. After an error the writer refuses further use.
pub struct SectWriter {
    w: BufWriter<File>,
    pos: u64,
    sections: Vec<Section>,
    names: HashSet<String>,
    failed: bool,
}

/// The writer handed to `SectWriter::add_with`: counts and hashes what goes through it.
pub struct SectionSink<'a> {
    w: &'a mut BufWriter<File>,
    len: u64,
    hash: Xxh3Default,
}

impl Write for SectionSink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.w.write(buf)?;
        self.hash.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

impl SectWriter {
    /// Starts a sectioned file at `path` (replacing any file there) with its meta JSON.
    pub fn create(path: &Path, meta: Value) -> Result<Self> {
        let meta = serde_json::to_vec(&meta)?;
        let mlen = u32::try_from(meta.len()).context("meta too large")?;
        let f = File::create(path).with_context(|| format!("create {}", path.display()))?;
        let mut w = BufWriter::with_capacity(4 << 20, f);
        w.write_all(MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&0u32.to_le_bytes())?; // section count, set by finish
        w.write_all(&0u64.to_le_bytes())?; // table offset, set by finish
        w.write_all(&mlen.to_le_bytes())?;
        w.write_all(&meta)?;
        Ok(Self { w, pos: (HEADER_LEN + meta.len()) as u64, sections: Vec::new(), names: HashSet::new(), failed: false })
    }

    /// Adds a section holding `bytes`.
    pub fn add(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        self.add_with(name, |w| Ok(w.write_all(bytes)?)).map(drop)
    }

    /// Adds a section holding `items` as stored in memory.
    pub fn add_pod<T: Pod>(&mut self, name: &str, items: &[T]) -> Result<()> {
        self.add(name, bytemuck::cast_slice(items))
    }

    /// Adds a section written by `f`, streamed (for sections too big to build in memory).
    /// Returns the section's length.
    pub fn add_with(&mut self, name: &str, f: impl FnOnce(&mut SectionSink<'_>) -> Result<()>) -> Result<u64> {
        ensure!(!self.failed, "sectioned file writer used after an error");
        check_name(name)?;
        ensure!(!self.names.contains(name), "section {name:?} added twice");
        let r = self.write_section(name, f);
        if r.is_err() {
            self.failed = true;
        }
        r
    }

    fn write_section(&mut self, name: &str, f: impl FnOnce(&mut SectionSink<'_>) -> Result<()>) -> Result<u64> {
        self.pad_to(ALIGN)?;
        let offset = self.pos;
        let mut sink = SectionSink { w: &mut self.w, len: 0, hash: Xxh3Default::new() };
        f(&mut sink).with_context(|| format!("section {name:?}"))?;
        let (len, hash) = (sink.len, sink.hash.digest());
        self.pos += len;
        self.names.insert(name.to_string());
        self.sections.push(Section { name: name.to_string(), offset, len, hash });
        Ok(len)
    }

    fn pad_to(&mut self, align: u64) -> io::Result<()> {
        const ZEROS: [u8; ALIGN as usize] = [0; ALIGN as usize];
        let pad = (align - self.pos % align) % align;
        self.w.write_all(&ZEROS[..pad as usize])?;
        self.pos += pad;
        Ok(())
    }

    /// Writes the section table, sets the header's count and table offset, and syncs the file.
    /// Returns the file's length.
    pub fn finish(mut self) -> Result<u64> {
        ensure!(!self.failed, "sectioned file writer used after an error");
        self.pad_to(8)?;
        let table = self.pos;
        for s in &self.sections {
            let mut name = [0u8; NAME_LEN];
            name[..s.name.len()].copy_from_slice(s.name.as_bytes());
            self.w.write_all(bytemuck::bytes_of(&Rec { name, offset: s.offset, len: s.len, hash: s.hash }))?;
        }
        let count = u32::try_from(self.sections.len()).context("too many sections")?;
        let f = self.w.into_inner().map_err(|e| e.into_error())?;
        f.write_all_at(&count.to_le_bytes(), 12)?;
        f.write_all_at(&table.to_le_bytes(), 16)?;
        f.sync_all()?;
        Ok(table + (self.sections.len() * REC_LEN) as u64)
    }
}

/// Reads a sectioned file from any range source: an `MmapFile` locally (then `slice` and `cast`
/// borrow sections in place), a `PooledFile` on the NAS.
pub struct SectReader<R> {
    src: R,
    version: u32,
    meta: Value,
    sections: Vec<Section>,
}

impl<R> std::fmt::Debug for SectReader<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SectReader").field("version", &self.version).field("meta", &self.meta).field("sections", &self.sections).finish_non_exhaustive()
    }
}

impl<R: RangeRead> SectReader<R> {
    /// Reads and checks the header, meta and section table (two range reads, one for a small file).
    pub fn open(src: R) -> Result<Self> {
        let file_len = src.len();
        ensure!(file_len >= HEADER_LEN as u64, "not a sectioned file ({file_len} bytes)");
        let first = src.read_at(0, file_len.min(FIRST_READ) as usize)?;
        ensure!(&first[..8] == MAGIC, "not a sectioned file");
        let version = u32::from_le_bytes(first[8..12].try_into()?);
        ensure!((MIN_VERSION..=VERSION).contains(&version), "sectioned file version {version} isn't supported (this app reads {MIN_VERSION}–{VERSION})");
        let count = u32::from_le_bytes(first[12..16].try_into()?) as u64;
        let table = u64::from_le_bytes(first[16..24].try_into()?);
        let data_start = (HEADER_LEN + u32::from_le_bytes(first[24..28].try_into()?) as usize) as u64;
        ensure!(data_start <= file_len, "sectioned file meta runs past the end");
        let table_end = table.checked_add(count * REC_LEN as u64).filter(|&e| e <= file_len);
        let Some(table_end) = table_end.filter(|_| table >= data_start) else {
            bail!("section table ({count} sections at {table}) is out of place; unfinished or cut short?");
        };
        let owned;
        let head = if first.len() as u64 >= data_start {
            &first[..data_start as usize]
        } else {
            owned = src.read_at(0, data_start as usize)?;
            &owned[..]
        };
        let meta = serde_json::from_slice(&head[HEADER_LEN..]).context("sectioned file meta")?;
        let owned;
        let tbytes = if table_end <= first.len() as u64 {
            &first[table as usize..table_end as usize]
        } else {
            owned = src.read_at(table, (table_end - table) as usize)?;
            &owned[..]
        };
        let mut sections = Vec::with_capacity(count as usize);
        let mut names = HashSet::new();
        for chunk in tbytes.chunks_exact(REC_LEN) {
            let r: Rec = bytemuck::pod_read_unaligned(chunk);
            let used = r.name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
            ensure!(r.name[used..].iter().all(|&c| c == 0), "bad section name padding");
            let name = std::str::from_utf8(&r.name[..used]).context("section name isn't UTF-8")?.to_string();
            check_name(&name)?;
            ensure!(names.insert(name.clone()), "section {name:?} appears twice");
            ensure!(
                r.offset.is_multiple_of(ALIGN) && r.offset >= data_start && r.offset.checked_add(r.len).is_some_and(|e| e <= table),
                "section {name:?} is out of place"
            );
            sections.push(Section { name, offset: r.offset, len: r.len, hash: r.hash });
        }
        let mut by_offset: Vec<&Section> = sections.iter().collect();
        by_offset.sort_by_key(|s| (s.offset, s.len));
        for w in by_offset.windows(2) {
            ensure!(w[0].offset + w[0].len <= w[1].offset, "sections {:?} and {:?} overlap", w[0].name, w[1].name);
        }
        Ok(Self { src, version, meta, sections })
    }

    pub fn meta(&self) -> &Value {
        &self.meta
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    /// The sections in file order.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    fn need(&self, name: &str) -> Result<&Section> {
        self.section(name).ok_or_else(|| anyhow!("no section {name:?}"))
    }

    /// A whole section, checked against its XXH3.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let s = self.need(name)?;
        let b = self.src.read_at(s.offset, usize::try_from(s.len)?)?;
        ensure!(xxh3(&b) == s.hash, "section {name:?} is damaged (checksum mismatch)");
        Ok(b)
    }

    /// A whole section as records, checked.
    pub fn read_pod<T: Pod>(&self, name: &str) -> Result<Vec<T>> {
        let b = self.read(name)?;
        ensure!(b.len() % std::mem::size_of::<T>().max(1) == 0, "section {name:?} isn't a whole number of {}-byte records", std::mem::size_of::<T>());
        Ok(bytemuck::pod_collect_to_vec(&b))
    }

    /// `len` bytes from `start` within a section (not checksummed: only whole sections can be).
    pub fn read_part(&self, name: &str, start: u64, len: usize) -> Result<Vec<u8>> {
        let s = self.need(name)?;
        ensure!(start.checked_add(len as u64).is_some_and(|e| e <= s.len), "bytes {start}..+{len} are outside section {name:?} ({} bytes)", s.len);
        Ok(self.src.read_at(s.offset + start, len)?)
    }

    pub fn source(&self) -> &R {
        &self.src
    }

    pub fn into_source(self) -> R {
        self.src
    }
}

impl<R: RangeRead + Mapped> SectReader<R> {
    /// A section's bytes, in place. In an mmapped file a section starts 64-byte aligned, so it
    /// casts (`bytemuck::cast_slice`, or `cast`) to any record type of a suitable size.
    pub fn slice(&self, name: &str) -> Option<&[u8]> {
        let s = self.section(name)?;
        self.src.bytes().get(s.offset as usize..(s.offset + s.len) as usize)
    }

    /// A section as records, in place (not checksummed; see `verify`).
    pub fn cast<T: Pod>(&self, name: &str) -> Result<&[T]> {
        let b = self.slice(name).ok_or_else(|| anyhow!("no section {name:?}"))?;
        bytemuck::try_cast_slice(b).map_err(|e| anyhow!("section {name:?} as {}-byte records: {e:?}", std::mem::size_of::<T>()))
    }

    /// Checks every section against its XXH3.
    pub fn verify(&self) -> Result<()> {
        for s in &self.sections {
            let b = self.slice(&s.name).ok_or_else(|| anyhow!("section {:?} is out of range", s.name))?;
            ensure!(xxh3(b) == s.hash, "section {:?} is damaged (checksum mismatch)", s.name);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iopool::IoPool;
    use crate::range::{MmapFile, PooledFile};
    use roadcore::WayRec;
    use serde_json::json;
    use std::time::Duration;

    /// Base-pack `ways` records (`WayRec`, 48 bytes).
    fn ways() -> Vec<WayRec> {
        (0..5u32)
            .map(|i| WayRec {
                id: 1000 + i as i64,
                vstart: u64::from(i) * 10,
                vcount: 10,
                name: i,
                ref_: 0,
                surface: 0,
                maxspeed: 50,
                class: i as u8,
                flags: 1,
                lanes: 2,
                network: 0,
                rail: 0,
                _pad: 0,
                route: 0,
                colour: 0,
            })
            .collect()
    }

    fn bytes(w: &[WayRec]) -> &[u8] {
        bytemuck::cast_slice(w)
    }

    fn write(path: &Path) -> u64 {
        let mut w = SectWriter::create(path, json!({"fmt": 1, "unit": "6/32/21"})).unwrap();
        w.add("strings", b"\nA1\nHigh Street").unwrap(); // 15 bytes: the next section must be realigned
        w.add_pod("ways", &ways()).unwrap();
        w.add("empty", b"").unwrap();
        let n = w
            .add_with("verts", |s| {
                for i in 0..1000i32 {
                    s.write_all(bytemuck::bytes_of(&[i, -i]))?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(n, 8000);
        w.add("exactly-24-bytes-long-ab", &[7u8; 3]).unwrap();
        w.finish().unwrap()
    }

    #[test]
    fn round_trip_mmapped_and_cast() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("base.sect");
        let len = write(&p);
        assert_eq!(len, std::fs::metadata(&p).unwrap().len());

        let r = SectReader::open(MmapFile::open(&p).unwrap()).unwrap();
        assert_eq!(r.meta()["unit"], "6/32/21");
        assert_eq!(r.version(), 1);
        let names: Vec<_> = r.sections().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["strings", "ways", "empty", "verts", "exactly-24-bytes-long-ab"]);
        for s in r.sections() {
            assert_eq!(s.offset % ALIGN, 0, "{}", s.name);
        }
        r.verify().unwrap();

        // In place: 64-byte aligned, castable straight from the map.
        let raw = r.slice("ways").unwrap();
        assert_eq!(raw.as_ptr() as usize % 64, 0);
        let recs: &[WayRec] = bytemuck::cast_slice(raw);
        assert_eq!((recs.len(), recs[3].id, recs[3].vstart), (5, 1003, 30));
        assert_eq!(bytes(r.cast::<WayRec>("ways").unwrap()), bytes(&ways()));
        let verts: &[[i32; 2]] = r.cast("verts").unwrap();
        assert_eq!((verts.len(), verts[999]), (1000, [999, -999]));
        assert_eq!(r.slice("empty").unwrap(), b"");
        assert!(r.cast::<WayRec>("strings").is_err(), "15 bytes aren't whole records");
        assert!(r.slice("nope").is_none());

        assert_eq!(r.read("strings").unwrap(), b"\nA1\nHigh Street");
        assert_eq!(bytes(&r.read_pod::<WayRec>("ways").unwrap()), bytes(&ways()));
        assert_eq!(r.read("exactly-24-bytes-long-ab").unwrap(), [7, 7, 7]);
        assert_eq!(r.read_part("strings", 4, 4).unwrap(), b"High");
        assert!(r.read_part("strings", 12, 4).is_err());
        assert!(r.read("nope").is_err());
    }

    #[test]
    fn reads_through_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("base.sect");
        write(&p);
        let pool = IoPool::new(2, Duration::from_secs(5), dir.path().to_owned());
        let r = SectReader::open(PooledFile::open(&pool, &p).unwrap()).unwrap();
        assert_eq!(bytes(&r.read_pod::<WayRec>("ways").unwrap()), bytes(&ways()));
        assert_eq!(r.read_pod::<[i32; 2]>("verts").unwrap()[10], [10, -10]);
    }

    #[test]
    fn writer_rejects_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = SectWriter::create(&dir.path().join("x.sect"), json!({})).unwrap();
        assert!(w.add("", b"x").is_err());
        assert!(w.add("this-name-is-25-bytes-xyz", b"x").is_err());
        assert!(w.add("nul\0", b"x").is_err());
        w.add("a", b"x").unwrap();
        assert!(w.add("a", b"y").is_err());
        // A failing streamed section poisons the writer.
        assert!(w.add_with("b", |_| bail!("source failed")).is_err());
        assert!(w.add("c", b"z").is_err());
        assert!(w.finish().is_err());
    }

    #[test]
    fn damage_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.sect");
        write(&p);
        let good = std::fs::read(&p).unwrap();
        let r = SectReader::open(good.clone()).unwrap();
        let ways = r.section("ways").unwrap().clone();

        let mut bad = good.clone();
        bad[ways.offset as usize + 9] ^= 0x10;
        let r = SectReader::open(bad).unwrap();
        assert!(r.read("ways").unwrap_err().to_string().contains("checksum"));
        assert!(r.verify().is_err());
        assert!(r.read("verts").is_ok());

        // Cut short, unfinished, not ours, a future version.
        assert!(SectReader::open(good[..good.len() - 1].to_vec()).is_err());
        let q = dir.path().join("b.sect");
        let mut w = SectWriter::create(&q, json!({})).unwrap();
        w.add("a", b"x").unwrap();
        drop(w);
        assert!(SectReader::open(std::fs::read(&q).unwrap()).is_err());
        assert!(SectReader::open(vec![0u8; 100]).is_err());
        let mut v2 = good.clone();
        v2[8] = 2;
        assert!(SectReader::open(v2).is_err());

        // A section that isn't aligned, and two that overlap.
        let table = u64::from_le_bytes(good[16..24].try_into().unwrap()) as usize;
        let mut bad = good.clone();
        let off = u64::from_le_bytes(bad[table + 24..table + 32].try_into().unwrap());
        bad[table + 24..table + 32].copy_from_slice(&(off + 8).to_le_bytes());
        assert!(SectReader::open(bad).is_err());
        let mut bad = good.clone();
        let second = table + REC_LEN;
        bad[second + 24..second + 32].copy_from_slice(&off.to_le_bytes());
        assert!(SectReader::open(bad).unwrap_err().to_string().contains("overlap"));
    }

    #[test]
    fn empty_file_and_big_meta() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("e.sect");
        SectWriter::create(&p, json!({"note": "y".repeat(80_000)})).unwrap().finish().unwrap();
        let r = SectReader::open(std::fs::read(&p).unwrap()).unwrap();
        assert!(r.sections().is_empty());
        assert_eq!(r.meta()["note"].as_str().unwrap().len(), 80_000);
    }
}
