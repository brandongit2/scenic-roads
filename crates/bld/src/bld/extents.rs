//! The buildings' data-available pieces drawn from the build's own copy of Overture's buildings
//! (docs/steps.md §14, extent class `mask`; docs/inputs.md §6.11, the `overture` method): a piece
//! per Overture dataset and what its buildings hold (`holds`: footprints, storeys or heights),
//! whose cells are the z14 tiles of the normalized files (`work/bld/6-x-y`, crate::bld::work, a
//! block per z14 tile) the cells' rule gives it.
//!
//! A record's kind, from the columns the files keep (crate::bld::work::Tags, and the side file's
//! `fsrc`, crate::bld::side; building parts left out): its **footprint**, counted for the
//! footprint's dataset (`fsrc`); a **height** (`h`), counted for the height's dataset (`hsrc`);
//! **storeys** (floors and no height), counted for the footprint's dataset.
//!
//! **The cells' rule** ([`credit`]), a cell's buildings counted:
//! - **footprints:** the cell is the footprints' piece of the dataset holding the most of its
//!   buildings (ties to the dataset first by name), and no other's: a stray building gives its
//!   dataset nothing. Where the most are of no dataset (code 0), or code 0 ties for the most, no
//!   dataset has the cell;
//! - **heights:** where half its buildings or more have a height ([`HELD`]), the cell is the
//!   heights' piece of each dataset holding a quarter or more of those heights ([`CREDITED`]): two
//!   datasets of 44 % each both have it;
//! - **storeys:** likewise, of the buildings with no height: where half of those or more have
//!   floors, each dataset holding a quarter or more of the storeys (a cell of 60 % heights and 40 %
//!   storeys alone has its storeys).
//!
//! The step's record of what it read (codecs::available::masks_logical: each tile's normalized
//! file's and side file's content names and its pieces' cells, for the pieces declared as they
//! are) is read first, and only the files it doesn't name are read: a file is read whole, a run of
//! blocks at a time ([`READ_RUN`]), its geometry left unread. The record holds the current files
//! alone, and the z6 tiles left out: a tile whose side file is missing, or names another
//! normalized file than the tile's (one `bldprep` hasn't written yet), has no cells until it has
//! its own.

use anyhow::{bail, ensure, Context, Result};
use buildstore::out::Out;
use inputs::inputs::extents::{MaskBlock, HOLDS};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use terrain::available::mask::{Mask, MaskTile, CELLS, MASK_Z};

use super::side::SideFile;
use super::work::{flag, IndexEntry, Meta, Tags, WorkFile};

/// What the record holds: bumped with a change to how a file's cells are found (every file read
/// again). (2: the footprint's own dataset, from the side file, and the cells' rule.)
pub const MASKS_V: u32 = 2;

/// The share of a cell's buildings that must hold heights (or storeys) for the cell to have them.
pub const HELD: f64 = 0.5;

/// The share of a cell's height-holding (or storey-holding) buildings a dataset must hold to be
/// credited with them there.
pub const CREDITED: f64 = 0.25;

/// The most stored bytes read at once from a file's blocks (a run of whole blocks; one block
/// larger is read alone): few reads over the NAS, little held.
pub const READ_RUN: usize = 32 << 20;

/// A mask piece: its id (`<source>/<piece>`) and its `overture` block's reading
/// (inputs::inputs::extents::MaskBlock).
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub id: String,
    pub block: MaskBlock,
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
            out.push(Spec { id: format!("{source}/{}", p.id), block: o.clone() });
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
    /// The z6 tiles left out: a side file missing, or naming another normalized file.
    pub left_out: Vec<(u32, u32)>,
    /// The buildings read of a (holds, dataset) no spec names: what a declaration could add.
    pub undeclared: BTreeMap<(String, String), u64>,
}

/// A file's cells: by spec (its index), the tile's cells held.
type FileCells = Vec<(usize, MaskTile)>;

/// The masks of `specs` (theme `theme`'s), from the normalized files and their side files the
/// records name, the step's record (`masks_logical`) read and written again; `report` hears the
/// files read of those to read, and the z6 tiles left out so far.
pub fn masks(out: &mut Out, theme: &str, specs: &[Spec], report: &(dyn Fn(u64, u64, usize) + Sync)) -> Result<Masks> {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
    let logical = codecs::available::masks_logical(theme);
    // (No mask piece: nothing read, and no record.)
    if specs.is_empty() {
        if out.get(&logical).is_some() {
            out.remove(&logical);
        }
        return Ok(Masks::default());
    }
    // (Each tile's normalized file and its side file, where it has one.)
    let files: Vec<((u32, u32), String, Option<String>)> = out
        .manifest
        .range(super::WORK.to_string()..)
        .take_while(|(l, _)| l.starts_with(super::WORK))
        .filter_map(|(l, c)| {
            let t = tile_of(l)?;
            Some((t, c.clone(), out.get(&super::side_logical(t.0, t.1)).map(str::to_string)))
        })
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
    let mut left_out: Vec<(u32, u32)> = files.iter().filter(|(_, _, s)| s.is_none()).map(|(t, _, _)| *t).collect();
    let sided: Vec<((u32, u32), String, String)> =
        files.iter().filter_map(|(t, c, s)| Some((*t, c.clone(), s.clone()?))).collect();
    let todo: Vec<&((u32, u32), String, String)> =
        sided.iter().filter(|(_, c, s)| !old.contains_key(&pair(c, s))).collect();
    let (done, unmatched) = (AtomicU64::new(0), AtomicUsize::new(0));
    let n = todo.len() as u64;
    type Read = ((u32, u32), String, Option<(FileCells, BTreeMap<(String, String), u64>)>);
    let read: Vec<Read> = todo
        .par_iter()
        .map(|((x, y), c, s)| {
            let got = read_file(&out.path(c), c, &out.path(s), (*x, *y), specs)
                .with_context(|| format!("the buildings of 6/{x}/{y} ({c}, {s})"))?;
            if got.is_none() {
                unmatched.fetch_add(1, Relaxed);
            }
            report(done.fetch_add(1, Relaxed) + 1, n, left_out.len() + unmatched.load(Relaxed));
            Ok(((*x, *y), pair(c, s), got))
        })
        .collect::<Result<_>>()?;
    let mut made = Masks { kept: sided.len() - todo.len(), ..Default::default() };
    // (A tile whose side file names another normalized file isn't recorded: it's left out until
    // `bldprep` writes its own.)
    for (t, k, got) in read {
        let Some((cells, u)) = got else {
            left_out.push(t);
            continue;
        };
        made.read += 1;
        old.insert(k, cells);
        for (k, v) in u {
            *made.undeclared.entry(k).or_default() += v;
        }
    }
    left_out.sort_unstable();
    // (The record of the current files alone, in their order; the masks from it.)
    let mut rec: Vec<(String, (u32, u32), &FileCells)> = Vec::with_capacity(sided.len());
    let mut masks: Vec<Mask> = specs.iter().map(|s| Mask { id: s.id.clone(), tiles: BTreeMap::new() }).collect();
    for (t, c, s) in &sided {
        let k = pair(c, s);
        let Some(cells) = old.get(&k) else { continue };
        for (i, m) in cells {
            masks[*i].tiles.insert(*t, m.clone());
        }
        rec.push((k, *t, cells));
    }
    let bytes = encode(&digest, &rec, &left_out)?;
    if out.get(&logical).is_none_or(|c| out.path(c).read().ok().as_deref() != Some(&bytes[..])) {
        out.put_bytes(&logical, "bin", &bytes)?;
    }
    made.masks = masks;
    made.left_out = left_out;
    Ok(made)
}

/// A tile's key in the record: its normalized file's and its side file's content names.
fn pair(work: &str, side: &str) -> String {
    format!("{work} {side}")
}

/// A normalized file's z6 tile, from its logical name (`work/bld/6-x-y`).
fn tile_of(l: &str) -> Option<(u32, u32)> {
    let mut p = l.strip_prefix(super::WORK)?.strip_prefix("6-")?.split('-');
    let (x, y) = (p.next()?.parse().ok()?, p.next()?.parse().ok()?);
    p.next().is_none().then_some((x, y))
}

/// The specs as the record holds them: a file's cells stand for these alone.
fn specs_digest(specs: &[Spec]) -> String {
    let mut t = format!("masks {MASKS_V} {MASK_Z} {HELD} {CREDITED}\n");
    for s in specs {
        t.push_str(&format!("{}\t{}\t{}\n", s.id, s.block.dataset, s.block.holds));
    }
    store::naming::hash16(t.as_bytes())
}

/// The datasets (codes) a cell credits with what `HOLDS[h]` names, by the cells' rule (the module's
/// doc), from its buildings' counts by dataset code (`count[code][h]`; code 0, no dataset, counted
/// but never credited) and its buildings (`all`).
pub fn credit(count: &[[u32; 3]], all: u32, h: usize) -> Vec<u8> {
    if h == FOOTPRINTS {
        // (The plurality; the first code of equals, the first by name, code 0 before any: a cell
        // whose most are of no dataset, or tie with them, credits none.)
        let best = count.iter().enumerate().fold((0usize, 0u32), |b, (c, n)| if n[h] > b.1 { (c, n[h]) } else { b });
        return if best.0 != 0 && best.1 > 0 { vec![best.0 as u8] } else { Vec::new() };
    }
    // (Storeys of the buildings with no height: a height outranks them.)
    let of = if h == STOREYS { all - count.iter().map(|n| n[HEIGHTS]).sum::<u32>() } else { all };
    let total: u32 = count.iter().map(|n| n[h]).sum();
    if total == 0 || f64::from(total) < HELD * f64::from(of) {
        return Vec::new();
    }
    (1..count.len())
        .filter(|&c| count[c][h] > 0 && f64::from(count[c][h]) >= CREDITED * f64::from(total))
        .map(|c| c as u8)
        .collect()
}

/// A normalized file's (`path`, its content name `work`) cells of `specs` with its side file's
/// (`side`), and the buildings read of a (holds, dataset) none names; None where the side file
/// names another normalized file.
fn read_file(
    path: &store::nasfs::NasPath,
    work: &str,
    side: &store::nasfs::NasPath,
    t: (u32, u32),
    specs: &[Spec],
) -> Result<Option<(FileCells, BTreeMap<(String, String), u64>)>> {
    let s = SideFile::open(side)?;
    if s.meta.work != work {
        return Ok(None);
    }
    let f = WorkFile::open(path)?;
    let tile = format!("6/{}/{}", t.0, t.1);
    ensure!(
        f.meta.tile == tile && s.meta.tile == tile,
        "a file of tile {}, its side file's {}",
        f.meta.tile,
        s.meta.tile
    );
    let records: usize = f.index.iter().map(|e| e.count as usize).sum();
    ensure!(s.fsrc.len() == records, "a side file of {} records, its normalized file's {records}", s.fsrc.len());
    // (The side file's records, in the file's order: each block's from `at`.)
    let mut at = 0usize;
    // (Each spec's dataset as the file's code: 0 where the file holds none of it.)
    let code = |d: &str| f.meta.srcs.iter().position(|s| s == d).map_or(0, |i| i as u8 + 1);
    let want: Vec<(u8, usize)> = specs.iter().map(|s| (code(&s.block.dataset), holds_index(&s.block.holds))).collect();
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
            let fsrc = &s.fsrc[at..at + b.n];
            at += b.n;
            // (Per dataset code, per holds: the buildings; and the cell's buildings.)
            let mut count = vec![[0u32; 3]; codes];
            let mut all = 0u32;
            for i in 0..b.n {
                if b.flags(i) & flag::PART != 0 {
                    continue;
                }
                all += 1;
                let (fs, hs) = (usize::from(fsrc[i]), usize::from(b.hsrc(i)));
                ensure!(fs < codes && hs < codes, "a dataset code {} past the file's {}", fs.max(hs), codes - 1);
                count[fs][FOOTPRINTS] += 1;
                if b.h(i) > 0 {
                    count[hs][HEIGHTS] += 1;
                } else if b.f(i) > 0 {
                    count[fs][STOREYS] += 1;
                }
            }
            if all == 0 {
                continue;
            }
            let (_, x14, y14) = roadcore::archive::tile_zxy(e.key);
            ensure!((x14 / CELLS, y14 / CELLS) == t, "a block of z14 tile {x14}/{y14} outside the file's tile");
            let (cx, cy) = ((x14 % CELLS) as u16, (y14 % CELLS) as usize);
            for h in [FOOTPRINTS, STOREYS, HEIGHTS] {
                let credited = credit(&count, all, h);
                for (k, (d, wh)) in want.iter().enumerate() {
                    if *wh == h && *d != 0 && credited.contains(d) {
                        rows[k][cy].push(cx);
                    }
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
    Ok(Some((cells, undeclared)))
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

/// inputs::inputs::extents::HOLDS's indexes (its order: footprints, storeys, heights).
const FOOTPRINTS: usize = 0;
const STOREYS: usize = 1;
const HEIGHTS: usize = 2;

/// A `holds` word's index in HOLDS (one the unit took).
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

/// The record's bytes (zstd'd): its magic, the specs' digest, then per tile its files' content
/// names ([`pair`]), its tile and its pieces' cells (each spec's index, its rows' run counts, its
/// runs); then the tiles left out.
fn encode(digest: &str, files: &[(String, (u32, u32), &FileCells)], left_out: &[(u32, u32)]) -> Result<Vec<u8>> {
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
    b.extend_from_slice(&(left_out.len() as u32).to_le_bytes());
    for (x, y) in left_out {
        b.extend_from_slice(&x.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
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
    // (The tiles left out: the record says them, the run finds them afresh.)
    let left = r.u32()? as usize;
    r.take(8 * left)?;
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
