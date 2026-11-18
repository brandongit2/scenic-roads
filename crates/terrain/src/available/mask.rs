//! Pieces drawn from cells of the build's own data rather than from outlines (docs/steps.md §14,
//! class `mask`): the buildings' Overture pieces, a cell a z14 tile holding buildings of the piece's
//! kind (bld::bld::extents). A mask is held by z6 tile, each its rows of cells as runs, and keyed
//! by each z6 tile's digest of them, so the build's data moving in one z6 tile makes again that
//! tile alone. Its cells are added to the outlines' runs of each row before the cut, a pixel in the
//! piece when its centre lies in a held cell.

use super::{Runs, Sets, BAND};
use codecs::available as fmt;
use std::collections::BTreeMap;

/// A mask's cells' zoom: a cell a z14 tile (64 z12 pixels a side, 2.4 km at the equator), the
/// buildings' blocks (bld::bld::work: a block per z14 tile).
pub const MASK_Z: u8 = 14;

/// A mask's cell rows (and columns) in a z6 tile.
pub const CELLS: u32 = 1 << (MASK_Z - fmt::PACK_Z);

/// A cell's side in z12 pixels.
const CELL_PX: u32 = BAND / CELLS;

/// A piece drawn from cells: its id (`<source>/<piece>`) and, by z6 tile (x, y), its cells there.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    pub id: String,
    pub tiles: BTreeMap<(u32, u32), MaskTile>,
}

/// A mask's cells in one z6 tile: row r's runs are `runs[start[r]..start[r + 1]]`, each the cells
/// [a, b) of the row held (`CELLS` rows, from the tile's top; sorted, apart), and their digest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MaskTile {
    pub start: Vec<u32>,
    pub runs: Vec<(u16, u16)>,
    pub digest: String,
}

impl MaskTile {
    /// A tile's cells from its rows' runs (`CELLS` rows), its digest made.
    pub fn of(rows: &[Vec<(u16, u16)>]) -> MaskTile {
        debug_assert_eq!(rows.len(), CELLS as usize);
        let mut start = Vec::with_capacity(rows.len() + 1);
        let mut runs = Vec::new();
        let mut h = blake3::Hasher::new();
        for (r, row) in rows.iter().enumerate() {
            start.push(runs.len() as u32);
            for &(a, b) in row {
                runs.push((a, b));
                h.update(&[r as u8, (r >> 8) as u8]);
                h.update(&a.to_le_bytes());
                h.update(&b.to_le_bytes());
            }
        }
        start.push(runs.len() as u32);
        MaskTile { start, runs, digest: h.finalize().to_hex()[..16].to_string() }
    }

    /// Whether it holds no cell.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Row r's runs.
    pub fn row(&self, r: usize) -> &[(u16, u16)] {
        &self.runs[self.start[r] as usize..self.start[r + 1] as usize]
    }
}

/// Each z6 tile a mask holds cells in, its key's lines (`<id> <digest>`).
pub(super) fn tile_lines(masks: &[Mask], lines: &mut BTreeMap<(u32, u32), Vec<String>>) {
    for m in masks {
        for (t, c) in &m.tiles {
            lines.entry(*t).or_default().push(format!("{} {}", m.id, c.digest));
        }
    }
}

/// The runs of `masks` (their pieces numbered from `first`) in z6 tiles `xs` of row `y`, for each
/// cell row of the band: a run's value a set of mask pieces (`sets`' numbers); None where no mask
/// holds a cell in the row's tiles.
pub(super) fn band_runs(
    masks: &[Mask],
    first: u32,
    y: u32,
    xs: &[u32],
    width: u32,
    sets: &mut Sets,
) -> Option<Vec<Runs>> {
    let held: Vec<(u32, u32, &MaskTile)> = masks
        .iter()
        .enumerate()
        .flat_map(|(k, m)| xs.iter().filter_map(move |&x| m.tiles.get(&(x, y)).map(|t| (first + k as u32, x, t))))
        .collect();
    if held.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(CELLS as usize);
    let mut edges: Vec<(u32, bool, u32)> = Vec::new();
    for r in 0..CELLS as usize {
        // (Each run's ends in the plane, a piece starting or ending there; the sets between.)
        edges.clear();
        for &(piece, x, t) in &held {
            for &(a, b) in t.row(r) {
                let x0 = x * BAND + u32::from(a) * CELL_PX;
                edges.push((x0, true, piece));
                edges.push((x0 + (u32::from(b) - u32::from(a)) * CELL_PX, false, piece));
            }
        }
        edges.sort_unstable();
        let mut on: Vec<u32> = Vec::new();
        let mut runs: Runs = vec![(0, 0)];
        let mut i = 0;
        while i < edges.len() {
            let at = edges[i].0;
            while i < edges.len() && edges[i].0 == at {
                let (_, starts, piece) = edges[i];
                if starts {
                    on.push(piece);
                } else if let Some(k) = on.iter().position(|&p| p == piece) {
                    on.swap_remove(k);
                }
                i += 1;
            }
            let mut v = on.clone();
            v.sort_unstable();
            v.dedup();
            let s = sets.id(v);
            if at >= width {
                break;
            }
            match runs.last_mut() {
                Some(l) if l.0 == at => l.1 = s,
                Some(l) if l.1 == s => {}
                _ => runs.push((at, s)),
            }
        }
        runs.dedup_by(|b, a| a.1 == b.1);
        out.push(runs);
    }
    Some(out)
}

/// Row `r`'s runs (of the band of z6 row `y`, at z12) with the masks' runs of its cell row added:
/// each pixel's set the union of the two.
pub(super) fn add(runs: &Runs, masks: &[Runs], y: u32, r: u32, sets: &mut Sets) -> Runs {
    let m = &masks[((r - y * BAND) / CELL_PX) as usize];
    if m.len() == 1 && m[0].1 == 0 {
        return runs.clone();
    }
    let mut out: Runs = Vec::with_capacity(runs.len() + m.len());
    let (mut i, mut j, mut x) = (0, 0, 0);
    loop {
        let v = sets.union(runs[i].1, m[j].1);
        if out.last().is_none_or(|l| l.1 != v) {
            out.push((x, v));
        }
        match (runs.get(i + 1).map(|q| q.0), m.get(j + 1).map(|q| q.0)) {
            (None, None) => break,
            (Some(p), Some(q)) if p == q => (i, j, x) = (i + 1, j + 1, p),
            (Some(p), Some(q)) if p < q => (i, x) = (i + 1, p),
            (Some(p), None) => (i, x) = (i + 1, p),
            (_, Some(q)) => (j, x) = (j + 1, q),
        }
    }
    out
}
