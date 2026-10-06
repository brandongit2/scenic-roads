//! A block's tiles, zoom 12 to 8, as trees.py's `pyramid` makes them, a band of 256 zoom-12 rows at
//! a time (each zoom keeps its rows until it has a band of its own: ~30 MB, not trees.py's 1.8 GB);
//! and zoom 7 to 4 from the blocks' zoom-8 values, as its `lower_zooms`.
//! - **Cover and height** are float32 as numpy has them: a zoom-12 pixel `f32(c / 10.0)` (‰ to %,
//!   divided in float64), `f32(h / 100.0)` (cm to m) where cover is at least 5 %; a coarser pixel
//!   the mean of its four, summed as numpy's two-axis mean sums them, `(a + b) + (c + d)` (row by
//!   row), then divided by 4.
//! - **Leaf type** is trees.py's shares of not forest, broadleaf, conifer, mixed and no data, kept
//!   as the count of zoom-12 pixels behind each (a share times its pixels, exact in float32 down to
//!   zoom 4, so any summing order gives numpy's): a pixel shows the commonest leaf type (the first,
//!   tied) where forest is at least half of what's known, 255 where nothing is.

use super::{BS, TS, ZBLOCK, ZMAX, ZMIN};
use anyhow::{ensure, Context, Result};
use rayon::prelude::*;
use std::collections::BTreeMap;

/// A tile made: its layer (0 cover, 1 height, 2 leaf type), zoom, column and row, and its WebP.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub layer: u8,
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub webp: Vec<u8>,
}

/// Rows of one zoom: cover (%) and canopy height (m), and per leaf-type class (not forest,
/// broadleaf, conifer, mixed, no data) the zoom-12 pixels each pixel holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    pub width: usize,
    pub cover: Vec<f32>,
    pub height: Vec<f32>,
    pub leaf: [Vec<u32>; 5],
}

impl Rows {
    fn new(width: usize) -> Rows {
        Rows { width, ..Default::default() }
    }

    fn rows(&self) -> usize {
        self.cover.len() / self.width.max(1)
    }

    /// `rows` rows of nothing: no cover, no height, no data (`unit` zoom-12 pixels a pixel).
    fn empty(width: usize, rows: usize, unit: u32) -> Rows {
        let n = width * rows;
        let mut r = Rows { width, cover: vec![0.0; n], height: vec![0.0; n], leaf: Default::default() };
        for (k, l) in r.leaf.iter_mut().enumerate() {
            *l = vec![if k == 4 { unit } else { 0 }; n];
        }
        r
    }

    fn extend(&mut self, o: &Rows) {
        self.cover.extend_from_slice(&o.cover);
        self.height.extend_from_slice(&o.height);
        for (a, b) in self.leaf.iter_mut().zip(&o.leaf) {
            a.extend_from_slice(b);
        }
    }

    /// Pixel `i`'s leaf-type class.
    fn class(&self, i: usize) -> u8 {
        class([self.leaf[0][i], self.leaf[1][i], self.leaf[2][i], self.leaf[3][i]])
    }

    /// Half the size: each pixel the mean of its four (trees.py's `down`), the leaf counts summed.
    fn down(&self) -> Rows {
        let (w, h) = (self.width / 2, self.rows() / 2);
        let mut out = Rows { width: w, cover: Vec::with_capacity(w * h), height: Vec::with_capacity(w * h), leaf: Default::default() };
        let mean = |a: &[f32], i: usize, j: usize| {
            let (r0, r1) = (2 * i * self.width, (2 * i + 1) * self.width);
            ((a[r0 + 2 * j] + a[r0 + 2 * j + 1]) + (a[r1 + 2 * j] + a[r1 + 2 * j + 1])) / 4.0
        };
        let sum = |a: &[u32], i: usize, j: usize| {
            let (r0, r1) = (2 * i * self.width, (2 * i + 1) * self.width);
            a[r0 + 2 * j] + a[r0 + 2 * j + 1] + a[r1 + 2 * j] + a[r1 + 2 * j + 1]
        };
        for i in 0..h {
            for j in 0..w {
                out.cover.push(mean(&self.cover, i, j));
                out.height.push(mean(&self.height, i, j));
            }
        }
        for (o, a) in out.leaf.iter_mut().zip(&self.leaf) {
            o.reserve(w * h);
            for i in 0..h {
                for j in 0..w {
                    o.push(sum(a, i, j));
                }
            }
        }
        out
    }
}

#[cfg(test)]
impl Rows {
    pub(crate) fn down_for_test(&self) -> Rows {
        self.down()
    }
}

/// The leaf-type class of a pixel holding `c` zoom-12 pixels of not forest, broadleaf, conifer
/// and mixed (trees.py's `leaf_class`).
pub fn class(c: [u32; 4]) -> u8 {
    let forest = c[1] as u64 + c[2] as u64 + c[3] as u64;
    let known = forest + c[0] as u64;
    if known == 0 {
        return 255;
    }
    if 2 * forest < known {
        return 0;
    }
    // (The first of the largest: numpy's argmax.)
    let mut k = 1;
    for t in 2..4 {
        if c[t] > c[k] {
            k = t;
        }
    }
    k as u8
}

/// A zoom-12 pixel's leaf-type class from its sampled value: trees.py's one-hot shares give the
/// class back, any other value none.
fn class12(v: u8) -> u8 {
    if v <= 3 {
        v
    } else {
        255
    }
}

/// 256 × 256 values in whole `step`s, Terrarium-encoded (trees.py's `terrarium`, in float32 as
/// numpy computes it) as lossless WebP.
fn terrarium(v: impl Fn(usize) -> f32, step: f32) -> Vec<u8> {
    let mut rgb = vec![0u8; TS * TS * 3];
    for (i, p) in rgb.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        let e = ((v(i) / step).round_ties_even() * step).clamp(0.0, 30000.0) as i32 + 32768;
        p[0] = (e >> 8) as u8;
        p[1] = (e & 255) as u8;
    }
    crate::webp::encode_rgb(&rgb, TS as u32, TS as u32)
}

/// The tiles of a band of rows at zoom `z` (256 rows, its tile row `ty`, from column `x0`):
/// cover and height where some pixel shows (rounds to a step), leaf type where some is forest.
fn tiles(r: &Rows, z: u8, x0: u32, ty: u32) -> Vec<Tile> {
    let n = r.width / TS;
    let jobs: Vec<(usize, u8)> = (0..n).flat_map(|tx| (0..3u8).map(move |l| (tx, l))).collect();
    jobs.par_iter()
        .filter_map(|&(tx, layer)| {
            let at = |i: usize| (i / TS) * r.width + tx * TS + i % TS;
            let webp = match layer {
                0 | 1 => {
                    let a = if layer == 0 { &r.cover } else { &r.height };
                    if !(0..TS * TS).any(|i| a[at(i)] >= 1.0) {
                        return None;
                    }
                    terrarium(|i| a[at(i)], 2.0)
                }
                _ => {
                    if !(0..TS * TS).any(|i| (1..=3).contains(&r.class(at(i)))) {
                        return None;
                    }
                    terrarium(|i| [0.0, 1.0, 2.0, 3.0].get(r.class(at(i)) as usize).copied().unwrap_or(0.0), 1.0)
                }
            };
            Some(Tile { layer, z, x: x0 + tx as u32, y: ty, webp })
        })
        .collect()
}

/// The zoom-12 tiles of a band: leaf type straight from its classes.
fn tiles12(cover: &[f32], height: &[f32], class: &[u8], x0: u32, ty: u32) -> Vec<Tile> {
    let jobs: Vec<(usize, u8)> = (0..BS / TS).flat_map(|tx| (0..3u8).map(move |l| (tx, l))).collect();
    jobs.par_iter()
        .filter_map(|&(tx, layer)| {
            let at = |i: usize| (i / TS) * BS + tx * TS + i % TS;
            let webp = match layer {
                0 | 1 => {
                    let a = if layer == 0 { cover } else { height };
                    if !(0..TS * TS).any(|i| a[at(i)] >= 1.0) {
                        return None;
                    }
                    terrarium(|i| a[at(i)], 2.0)
                }
                _ => {
                    if !(0..TS * TS).any(|i| (1..=3).contains(&class[at(i)])) {
                        return None;
                    }
                    terrarium(|i| if class[at(i)] == 255 { 0.0 } else { class[at(i)] as f32 }, 1.0)
                }
            };
            Some(Tile { layer, z: ZMAX, x: x0 + tx as u32, y: ty, webp })
        })
        .collect()
}

/// A block's pyramid, fed a band of zoom-12 rows at a time.
pub struct Pyramid {
    bx: u32,
    by: u32,
    /// Zoom 11 to 8: rows made, not yet a band.
    levels: Vec<Rows>,
    /// Bands done at zoom 12.
    bands: u32,
    tiles: Vec<Tile>,
}

impl Pyramid {
    pub fn new(bx: u32, by: u32) -> Pyramid {
        let levels = (ZBLOCK..ZMAX).rev().map(|z| Rows::new(TS << (z - ZBLOCK))).collect();
        Pyramid { bx, by, levels, bands: 0, tiles: Vec::new() }
    }

    /// The next 256 zoom-12 rows: cover (‰) and height (cm) as sampled, leaf type (255 none), and
    /// which pixels are inside (a bit each, 64 a word).
    pub fn band(&mut self, cover: &[u16], height: &[u16], leaf: &[u8], inside: &[u64]) {
        let n = TS * BS;
        assert!(cover.len() == n && height.len() == n && leaf.len() == n && inside.len() == n / 64);
        // No canopy and no leaf type (the sea, or no square): nothing there, its tiles none.
        if cover.iter().all(|&v| v == 0) && leaf.iter().all(|&v| v == 255) {
            self.bands += 1;
            self.push(0, Rows::empty(BS / 2, TS / 2, 4));
            return;
        }
        let ins = |i: usize| inside[i / 64] >> (i % 64) & 1 == 1;
        let c: Vec<f32> = (0..n).map(|i| if ins(i) && cover[i] <= 1000 { (cover[i] as f64 / 10.0) as f32 } else { 0.0 }).collect();
        let h: Vec<f32> = (0..n).map(|i| if ins(i) && c[i] >= 5.0 { (height[i] as f64 / 100.0) as f32 } else { 0.0 }).collect();
        // Leaf type only where there are trees.
        let lft: Vec<u8> = (0..n)
            .map(|i| {
                let l = if ins(i) { leaf[i] } else { 255 };
                if (1..=3).contains(&l) && c[i] < 5.0 {
                    0
                } else {
                    l
                }
            })
            .collect();
        let class: Vec<u8> = lft.iter().map(|&l| class12(l)).collect();
        let k = 1u32 << (ZMAX - ZBLOCK);
        self.tiles.extend(tiles12(&c, &h, &class, self.bx * k, self.by * k + self.bands));
        self.bands += 1;
        // Zoom 11: each class's pixels counted.
        let w = BS / 2;
        let mut r = Rows { width: w, cover: Vec::with_capacity(w * TS / 2), height: Vec::with_capacity(w * TS / 2), leaf: Default::default() };
        for l in r.leaf.iter_mut() {
            *l = vec![0; w * TS / 2];
        }
        for i in 0..TS / 2 {
            for j in 0..w {
                let (a, b) = ((2 * i) * BS + 2 * j, (2 * i + 1) * BS + 2 * j);
                r.cover.push(((c[a] + c[a + 1]) + (c[b] + c[b + 1])) / 4.0);
                r.height.push(((h[a] + h[a + 1]) + (h[b] + h[b + 1])) / 4.0);
                // (A value that's no class has no share.)
                for p in [a, a + 1, b, b + 1] {
                    let k = match lft[p] {
                        255 => 4,
                        v if v <= 3 => v as usize,
                        _ => continue,
                    };
                    r.leaf[k][i * w + j] += 1;
                }
            }
        }
        self.push(0, r);
    }

    /// Rows made at level `k` (zoom 11 - k): a band there once it has 256, its tiles made and its
    /// rows taken down a zoom.
    fn push(&mut self, k: usize, r: Rows) {
        let last = k + 1 == self.levels.len();
        let lv = &mut self.levels[k];
        lv.extend(&r);
        if lv.rows() < TS || last {
            return;
        }
        let band = std::mem::replace(lv, Rows::new(lv.width));
        let z = ZMAX - 1 - k as u8;
        let per = 1u32 << (z - ZBLOCK);
        // (Its band row: how many bands of this zoom came before.)
        let ty = self.by * per + (self.bands * per / (1 << (ZMAX - ZBLOCK)) - 1);
        self.tiles.extend(tiles(&band, z, self.bx * per, ty));
        self.push(k + 1, band.down());
    }

    /// Zoom 8's tile and the tiles made, once every band is in; and the block's zoom-8 values.
    pub fn finish(mut self) -> (Vec<Tile>, Tops) {
        assert_eq!(self.bands as usize, BS / TS, "a block's every band");
        let top = std::mem::take(self.levels.last_mut().unwrap());
        self.tiles.extend(tiles(&top, ZBLOCK, self.bx, self.by));
        (self.tiles, Tops { x: self.bx, y: self.by, rows: top })
    }
}

/// A block's zoom-8 values (trees.py's tops): 256 × 256 of each.
#[derive(Clone, Debug, PartialEq)]
pub struct Tops {
    pub x: u32,
    pub y: u32,
    pub rows: Rows,
}

const TOPS_MAGIC: &[u8; 8] = b"TREETOP1";

impl Tops {
    /// As a block writes them (`TOPS`): "TREETOP1", u32 x, u32 y, then zstd of cover and height
    /// (f32 × 65,536 each) and the leaf counts (u16 × 65,536 per class), little-endian.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let r = &self.rows;
        ensure!(r.width == TS && r.cover.len() == TS * TS, "zoom-8 values of {} pixels", r.cover.len());
        let mut raw = Vec::with_capacity(TS * TS * 18);
        for v in r.cover.iter().chain(&r.height) {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        for l in &r.leaf {
            for &v in l {
                raw.extend_from_slice(&u16::try_from(v).context("a zoom-8 leaf count past 65,535")?.to_le_bytes());
            }
        }
        let mut out = TOPS_MAGIC.to_vec();
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend(zstd::bulk::compress(&raw, 3)?);
        Ok(out)
    }

    /// The block whose values `b` holds.
    pub fn block_of(b: &[u8]) -> Result<(u32, u32)> {
        ensure!(b.len() >= 16 && &b[..8] == TOPS_MAGIC, "not a block's zoom-8 values");
        Ok((u32::from_le_bytes(b[8..12].try_into()?), u32::from_le_bytes(b[12..16].try_into()?)))
    }

    pub fn from_bytes(b: &[u8]) -> Result<Tops> {
        let (x, y) = Tops::block_of(b)?;
        let n = TS * TS;
        let raw = zstd::bulk::decompress(&b[16..], n * 18)?;
        ensure!(raw.len() == n * 18, "zoom-8 values cut short");
        let f = |k: usize| raw[k * n * 4..(k + 1) * n * 4].as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect::<Vec<f32>>();
        let mut leaf: [Vec<u32>; 5] = Default::default();
        for (k, l) in leaf.iter_mut().enumerate() {
            let at = 8 * n + k * n * 2;
            *l = raw[at..at + n * 2].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c) as u32).collect();
        }
        Ok(Tops { x, y, rows: Rows { width: TS, cover: f(0), height: f(1), leaf } })
    }
}

/// Zoom 7 to 4 from the blocks' zoom-8 values (`tops`, each as `Tops::to_bytes` has it), as trees.py's
/// `lower_zooms`: a tile wherever one of its blocks is, its missing quarters no cover, no height
/// and no data; its tiles' children before it. `said` is told how many tiles are made, of how
/// many, at the start and the end and at most once a second between.
pub fn lower(tops: &BTreeMap<(u32, u32), Vec<u8>>, said: &(dyn Fn(u64, u64) + Sync)) -> Result<Vec<Tile>> {
    let mut total = 0u64;
    for s in 1..=ZBLOCK - ZMIN {
        let mut p: Vec<(u32, u32)> = tops.keys().map(|&(x, y)| (x >> s, y >> s)).collect();
        p.sort_unstable();
        p.dedup();
        total += p.len() as u64;
    }
    said(0, total);
    let made = std::sync::atomic::AtomicU64::new(0);
    let at = std::sync::Mutex::new(std::time::Instant::now());
    let tick = || {
        let m = made.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let mut t = at.lock().unwrap();
        if t.elapsed() >= std::time::Duration::from_secs(1) {
            *t = std::time::Instant::now();
            said(m, total);
        }
    };
    let mut roots: Vec<(u32, u32)> = tops.keys().map(|&(x, y)| (x >> (ZBLOCK - ZMIN), y >> (ZBLOCK - ZMIN))).collect();
    roots.sort_unstable();
    roots.dedup();
    let out: Vec<Vec<Tile>> = roots
        .par_iter()
        .map(|&(x, y)| {
            let mut tiles = Vec::new();
            node(tops, ZMIN, x, y, &mut tiles, &tick)?;
            Ok(tiles)
        })
        .collect::<Result<_>>()?;
    said(total, total);
    Ok(out.into_iter().flatten().collect())
}

/// Tile (`z`, `x`, `y`)'s values, its tiles and its descendants' added to `tiles` (zoom 7 to 4);
/// None when no block is under it.
fn node(tops: &BTreeMap<(u32, u32), Vec<u8>>, z: u8, x: u32, y: u32, tiles: &mut Vec<Tile>, tick: &(dyn Fn() + Sync)) -> Result<Option<Rows>> {
    if z == ZBLOCK {
        return tops.get(&(x, y)).map(|b| Tops::from_bytes(b).map(|t| t.rows).with_context(|| format!("block 8/{x}/{y}'s zoom-8 values"))).transpose();
    }
    let kids: Vec<(Option<Rows>, Vec<Tile>)> = [(0, 0), (1, 0), (0, 1), (1, 1)]
        .par_iter()
        .map(|&(dx, dy)| {
            let mut t = Vec::new();
            Ok((node(tops, z + 1, 2 * x + dx, 2 * y + dy, &mut t, tick)?, t))
        })
        .collect::<Result<_>>()?;
    if kids.iter().all(|k| k.0.is_none()) {
        return Ok(None);
    }
    // The four as one, 512 × 512: a missing one no data.
    let unit = 1u32 << (2 * (ZMAX - z - 1));
    let w = 2 * TS;
    let mut big = Rows::empty(w, w, unit);
    for (q, (kid, t)) in kids.into_iter().enumerate() {
        tiles.extend(t);
        let Some(k) = kid else { continue };
        let (ox, oy) = ((q % 2) * TS, (q / 2) * TS);
        for i in 0..TS {
            let (d, s) = ((oy + i) * w + ox, i * TS);
            big.cover[d..d + TS].copy_from_slice(&k.cover[s..s + TS]);
            big.height[d..d + TS].copy_from_slice(&k.height[s..s + TS]);
            for (a, b) in big.leaf.iter_mut().zip(&k.leaf) {
                a[d..d + TS].copy_from_slice(&b[s..s + TS]);
            }
        }
    }
    let r = big.down();
    tiles.extend(self::tiles(&r, z, x, y));
    tick();
    Ok(Some(r))
}
