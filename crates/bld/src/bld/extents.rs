//! The buildings' data-available pieces drawn from the build's own copy of Overture's buildings
//! (docs/steps.md §14, extent class `mask`; docs/inputs.md §6.11, the `overture` method): a piece
//! per Overture dataset and what its buildings hold (`holds`: footprints, storeys or heights),
//! whose cells are the z14 tiles of the normalized files (`work/bld/6-x-y`, crate::bld::work, a
//! block per z14 tile) where those buildings are at least the block's `share` of the cell's
//! buildings, and at least one.
//!
//! A record's kind, from the columns the files keep (crate::bld::work::Tags; building parts left
//! out):
//! - its **footprint's dataset**: OpenStreetMap where OSM gave the footprint (its OSM id), else
//!   its `hsrc` (the footprint's dataset where no source gave it a height; else the height's,
//!   which for a footprint and height of one source is the same);
//! - **heights**: a height (`h`), counted for the height's dataset (`hsrc`);
//! - **storeys**: floors and no height, counted for the footprint's dataset;
//! - **footprints**: every building, counted for the footprint's dataset.
//!
//! The step's record of what it read (codecs::available::masks_logical: each file's content name
//! and its pieces' cells, for the pieces declared as they are) is read first, and only the files
//! it doesn't name are read: a file is read whole, a run of blocks at a time
//! ([`READ_RUN`]), its geometry left unread. The record holds the current files alone.

use anyhow::{bail, ensure, Context, Result};
use buildstore::out::Out;
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use terrain::available::mask::{Mask, MaskTile, CELLS, MASK_Z};

use super::work::{flag, IndexEntry, Meta, Tags, WorkFile};

/// What the record holds: bumped with a change to how a file's cells are found (every file read
/// again).
pub const MASKS_V: u32 = 1;

/// The most stored bytes read at once from a file's blocks (a run of whole blocks; one block
/// larger is read alone): few reads over the NAS, little held.
pub const READ_RUN: usize = 32 << 20;

/// The dataset OSM's footprints are (Overture's name for it).
pub const OSM: &str = "OpenStreetMap";

/// A mask piece as its declaration's `overture` block says it (inputs::inputs::extents::Overture):
/// its id (`<source>/<piece>`), its dataset, what it holds and its share.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub id: String,
    pub dataset: String,
    pub holds: String,
    pub share: f64,
}

/// The mask pieces of source `source`: its `.geojson`'s pieces of an `overture` block, each by the
/// block's reading in the file's `read` (the unit's `ex-made` check holds it to the declaration's
/// block), so the pieces' file, which the step's key holds, says them whole.
pub fn specs(source: &str, pieces: &[u8]) -> Result<Vec<Spec>> {
    let s = inputs::inputs::extents::summary(pieces);
    if let Some(why) = &s.broken {
        bail!("{source}'s pieces: {why}");
    }
    let mut out = Vec::new();
    for p in &s.pieces {
        if let Some(o) = s.overture.get(p.id.split('/').next().unwrap_or("")) {
            ensure!(p.extent == "mask", "{source}/{}: an overture block's piece of class {}", p.id, p.extent);
            let (dataset, holds) = (o.dataset.clone(), o.holds.clone());
            out.push(Spec { id: format!("{source}/{}", p.id), dataset, holds, share: o.share });
        }
    }
    Ok(out)
}

/// The masks made, and what the run read.
#[derive(Debug, Default)]
pub struct Masks {
    pub masks: Vec<Mask>,
    /// The normalized files read, and those whose cells the record held.
    pub read: usize,
    pub kept: usize,
    /// The buildings read of a (holds, dataset) no spec names: what a declaration could add.
    pub undeclared: BTreeMap<(String, String), u64>,
}

/// A file's cells: by spec (its index), the tile's cells held.
type FileCells = Vec<(usize, MaskTile)>;

/// The masks of `specs` (theme `theme`'s), from the normalized files the records name, the step's
/// record (`masks_logical`) read and written again; `report` hears the files read of those to read.
pub fn masks(out: &mut Out, theme: &str, specs: &[Spec], report: &(dyn Fn(u64, u64) + Sync)) -> Result<Masks> {
    let logical = codecs::available::masks_logical(theme);
    // (No mask piece: nothing read, and no record.)
    if specs.is_empty() {
        if out.get(&logical).is_some() {
            out.remove(&logical);
        }
        return Ok(Masks::default());
    }
    let files: Vec<((u32, u32), String)> = out
        .manifest
        .range(super::WORK.to_string()..)
        .take_while(|(l, _)| l.starts_with(super::WORK))
        .filter_map(|(l, c)| Some((tile_of(l)?, c.clone())))
        .collect();
    let digest = specs_digest(specs);
    let mut old = match out.get(&logical) {
        Some(c) => {
            let b = out.path(c).read().with_context(|| format!("read {c}"))?;
            decode(&b, &digest, specs.len()).unwrap_or_else(|e| {
                eprintln!("available {theme}: the record {c} isn't readable ({e:#}): every file read again");
                HashMap::new()
            })
        }
        None => HashMap::new(),
    };
    let todo: Vec<&((u32, u32), String)> = files.iter().filter(|(_, c)| !old.contains_key(c)).collect();
    let done = std::sync::atomic::AtomicU64::new(0);
    let n = todo.len() as u64;
    let read: Vec<(String, FileCells, BTreeMap<(String, String), u64>)> = todo
        .par_iter()
        .map(|((x, y), c)| {
            let (cells, undeclared) = read_file(&out.path(c), (*x, *y), specs)
                .with_context(|| format!("the buildings of 6/{x}/{y} ({c})"))?;
            report(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, n);
            Ok((c.clone(), cells, undeclared))
        })
        .collect::<Result<_>>()?;
    let mut made = Masks { read: read.len(), kept: files.len() - read.len(), ..Default::default() };
    for (c, cells, u) in read {
        old.insert(c, cells);
        for (k, v) in u {
            *made.undeclared.entry(k).or_default() += v;
        }
    }
    // (The record of the current files alone, in their order; the masks from it.)
    let mut rec: Vec<(&str, (u32, u32), &FileCells)> = Vec::with_capacity(files.len());
    let mut masks: Vec<Mask> = specs.iter().map(|s| Mask { id: s.id.clone(), tiles: BTreeMap::new() }).collect();
    for (t, c) in &files {
        let cells = &old[c];
        rec.push((c.as_str(), *t, cells));
        for (k, m) in cells {
            masks[*k].tiles.insert(*t, m.clone());
        }
    }
    let bytes = encode(&digest, &rec)?;
    if out.get(&logical).is_none_or(|c| out.path(c).read().ok().as_deref() != Some(&bytes[..])) {
        out.put_bytes(&logical, "bin", &bytes)?;
    }
    made.masks = masks;
    Ok(made)
}

/// A normalized file's z6 tile, from its logical name (`work/bld/6-x-y`).
fn tile_of(l: &str) -> Option<(u32, u32)> {
    let mut p = l.strip_prefix(super::WORK)?.strip_prefix("6-")?.split('-');
    let (x, y) = (p.next()?.parse().ok()?, p.next()?.parse().ok()?);
    p.next().is_none().then_some((x, y))
}

/// The specs as the record holds them: a file's cells stand for these alone.
fn specs_digest(specs: &[Spec]) -> String {
    let mut t = format!("masks {MASKS_V} {MASK_Z}\n");
    for s in specs {
        t.push_str(&format!("{}\t{}\t{}\t{}\n", s.id, s.dataset, s.holds, s.share));
    }
    store::naming::hash16(t.as_bytes())
}

/// A file's cells of `specs`, and the buildings read of a (holds, dataset) none names.
fn read_file(
    path: &store::nasfs::NasPath,
    t: (u32, u32),
    specs: &[Spec],
) -> Result<(FileCells, BTreeMap<(String, String), u64>)> {
    let f = WorkFile::open(path)?;
    ensure!(f.meta.tile == format!("6/{}/{}", t.0, t.1), "a file of tile {}", f.meta.tile);
    // (Each spec's dataset as the file's code: 0 where the file holds none of it.)
    let code = |d: &str| f.meta.srcs.iter().position(|s| s == d).map_or(0, |i| i as u8 + 1);
    let want: Vec<(u8, usize)> = specs.iter().map(|s| (code(&s.dataset), holds_index(&s.holds))).collect();
    let osm = code(OSM);
    // (By spec, by row: the cells held.)
    let mut rows: Vec<Vec<Vec<u16>>> = vec![vec![Vec::new(); CELLS as usize]; specs.len()];
    let codes = f.meta.srcs.len() + 1;
    let mut undeclared: BTreeMap<(String, String), u64> = BTreeMap::new();
    for run in runs(&f.index) {
        let (start, end) = (run[0].offset, run[run.len() - 1].offset + u64::from(run[run.len() - 1].len));
        let bytes = f.stored_run(start, (end - start) as usize)?;
        for e in run {
            let z = &bytes[(e.offset - start) as usize..][..e.len as usize];
            let raw = zstd::decode_all(z).context("a building block's zstd")?;
            let b = Tags::of(&raw)?;
            ensure!(b.n == e.count as usize, "a building block holds {} records, its index says {}", b.n, e.count);
            // (Per holds, per dataset code: the buildings; and the cell's buildings.)
            let mut count = vec![[0u32; 3]; codes];
            let mut all = 0u32;
            for i in 0..b.n {
                if b.flags(i) & flag::PART != 0 {
                    continue;
                }
                all += 1;
                ensure!(usize::from(b.hsrc(i)) < codes, "a dataset code {} past the file's {}", b.hsrc(i), codes - 1);
                let foot = if b.osm(i) != 0 && osm != 0 { osm } else { b.hsrc(i) };
                count[usize::from(foot)][FOOTPRINTS] += 1;
                if b.h(i) > 0 {
                    count[usize::from(b.hsrc(i))][HEIGHTS] += 1;
                } else if b.f(i) > 0 {
                    count[usize::from(foot)][STOREYS] += 1;
                }
            }
            if all == 0 {
                continue;
            }
            let (_, x14, y14) = roadcore::archive::tile_zxy(e.key);
            ensure!((x14 / CELLS, y14 / CELLS) == t, "a block of z14 tile {x14}/{y14} outside the file's tile");
            let (cx, cy) = ((x14 % CELLS) as u16, (y14 % CELLS) as usize);
            for (k, (d, h)) in want.iter().enumerate() {
                let got = count[usize::from(*d)][*h];
                if *d != 0 && got > 0 && f64::from(got) >= specs[k].share * f64::from(all) {
                    rows[k][cy].push(cx);
                }
            }
            for (d, c) in count.iter().enumerate().skip(1) {
                for (h, n) in c.iter().enumerate().filter(|(_, n)| **n > 0) {
                    if !want.contains(&(d as u8, h)) {
                        let name = Meta::name(&f.meta.srcs, d as u8).to_string();
                        *undeclared.entry((HOLDS[h].to_string(), name)).or_default() += u64::from(*n);
                    }
                }
            }
        }
    }
    let cells = rows
        .into_iter()
        .enumerate()
        .map(|(k, r)| (k, MaskTile::of(&r.into_iter().map(row_runs).collect::<Vec<_>>())))
        .filter(|(_, m)| !m.is_empty())
        .collect();
    Ok((cells, undeclared))
}

/// A row's cells as runs [a, b), sorted, cells side by side one run.
pub fn row_runs(mut cells: Vec<u16>) -> Vec<(u16, u16)> {
    cells.sort_unstable();
    cells.dedup();
    let mut out: Vec<(u16, u16)> = Vec::new();
    for c in cells {
        match out.last_mut() {
            Some(l) if l.1 == c => l.1 = c + 1,
            _ => out.push((c, c + 1)),
        }
    }
    out
}

const FOOTPRINTS: usize = 0;
const STOREYS: usize = 1;
const HEIGHTS: usize = 2;
/// inputs::inputs::extents::HOLDS, by index.
const HOLDS: [&str; 3] = ["footprints", "storeys", "heights"];
const _: () = assert!(HOLDS.len() == inputs::inputs::extents::HOLDS.len());

/// A `holds` word's index (one the unit took: HOLDS).
fn holds_index(h: &str) -> usize {
    HOLDS.iter().position(|w| *w == h).unwrap_or(FOOTPRINTS)
}

/// A file's blocks in runs read at once: consecutive blocks up to READ_RUN bytes.
fn runs(index: &[IndexEntry]) -> Vec<&[IndexEntry]> {
    let mut out = Vec::new();
    let mut from = 0;
    let mut bytes = 0usize;
    for (i, e) in index.iter().enumerate() {
        let next = i > from
            && (bytes + e.len as usize > READ_RUN || e.offset != index[i - 1].offset + u64::from(index[i - 1].len));
        if next {
            out.push(&index[from..i]);
            from = i;
            bytes = 0;
        }
        bytes += e.len as usize;
    }
    if from < index.len() {
        out.push(&index[from..]);
    }
    out
}

/// The record's bytes (zstd'd): its magic, the specs' digest, then per file its content name, its
/// tile and its pieces' cells (each spec's index, its rows' run counts, its runs).
fn encode(digest: &str, files: &[(&str, (u32, u32), &FileCells)]) -> Result<Vec<u8>> {
    let mut b = Vec::new();
    b.extend_from_slice(b"BLDMASK1");
    b.extend_from_slice(digest.as_bytes());
    b.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (c, (x, y), cells) in files {
        b.extend_from_slice(&(c.len() as u16).to_le_bytes());
        b.extend_from_slice(c.as_bytes());
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
        b.extend_from_slice(&(cells.len() as u16).to_le_bytes());
        for (k, m) in cells.iter() {
            b.extend_from_slice(&(*k as u16).to_le_bytes());
            for r in 0..CELLS as usize {
                b.extend_from_slice(&(m.row(r).len() as u16).to_le_bytes());
            }
            for &(p, q) in &m.runs {
                b.extend_from_slice(&p.to_le_bytes());
                b.extend_from_slice(&q.to_le_bytes());
            }
        }
    }
    Ok(zstd::bulk::compress(&b, 3)?)
}

/// The record's files' cells, by content name; empty where it was made for other specs.
fn decode(z: &[u8], digest: &str, specs: usize) -> Result<HashMap<String, FileCells>> {
    let b = zstd::decode_all(z).context("the record's zstd")?;
    let mut r = Reader { b: &b, at: 0 };
    ensure!(r.take(8)? == b"BLDMASK1", "not a masks record");
    if r.take(16)? != digest.as_bytes() {
        return Ok(HashMap::new());
    }
    let n = r.u32()?;
    let mut out = HashMap::new();
    for _ in 0..n {
        let len = r.u16()? as usize;
        let c = std::str::from_utf8(r.take(len)?)?.to_string();
        r.take(8)?;
        let k = r.u16()?;
        let mut cells = Vec::with_capacity(k as usize);
        for _ in 0..k {
            let s = r.u16()? as usize;
            ensure!(s < specs, "a spec past the specs");
            let counts: Vec<u16> = (0..CELLS).map(|_| r.u16()).collect::<Result<_>>()?;
            let mut rows = Vec::with_capacity(counts.len());
            for n in counts {
                rows.push((0..n).map(|_| Ok((r.u16()?, r.u16()?))).collect::<Result<Vec<_>>>()?);
            }
            cells.push((s, MaskTile::of(&rows)));
        }
        out.insert(c, cells);
    }
    ensure!(r.at == b.len(), "the record has bytes over");
    Ok(out)
}

/// A little-endian reader over the record's bytes.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.b.get(self.at..self.at + n).context("the record cut short")?;
        self.at += n;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
}

#[cfg(test)]
mod tests;
