//! The far buildings' field (docs/buildings3d.md §4.8, §4.8.1): every building as per-texel densities,
//! 512 × 512 texels a tile, levels 4–13, for five height classes (by a building's or part's top:
//! under 8 m, 8–20, 20–40, 40–100, 100 m and up), three a class:
//! - `f` roof coverage: the share of the texel under the class's roofs (exact areas, as font
//!   renderers rasterize; overlapping footprints summed, then clamped to 1);
//! - `w` wall area per ground area: Σ wall length × (top − base) ÷ the texel's area;
//! - `v` Σ roof share × top (m), so `v ÷ f` is the class's mean top (scaled with `f`'s clamp).
//!
//! Densities, so a coarser level is the mean of its four children. Every level holds every building
//! (the meshes are z14's alone, from map zoom 14); level 14 is drawn from zoom 13 to 14.
//!
//! - **`bldtiles T`** makes a z8 area's levels 14–9 and its level-8 planes ([`area`]) from the same records as its meshes
//!   (filled, `job::area_tiles`): every building and part extruded (not an outline with parts, not a
//!   copy), the area's own and those around it reaching in, each rasterized into the z13 cells of the
//!   area it meets at level 14, in fixed point (i64: the sums don't depend on their order), the cells'
//!   level 13 from their level 14, then levels 12–8 by means of four in a fixed order. The level-8
//!   planes are kept exactly ([`write_mid`]: `work/bldfield-mid/6-x-y`).
//! - **`bldfield-lo Q`** (a z3 tile) makes levels 8–4 from its z6 tiles' mids ([`lo`]): the lo packs
//!   hold levels 4–8, the hi packs 9–14 (as the server takes zooms 3–8 from lo packs).
//! - **Encoding `field3-webp`** ([`encode`]): a mask byte (the classes held, bit k class k), then one
//!   lossless WebP (`crate::webp`), 512 wide and 512 × (the classes held) tall, a class's rows after
//!   the one before: R `f` as √f, G `w` as log2(1 + w) ÷ log2(4097) (to 4096), B the class's mean top
//!   linearly in its class's range ([`LO_M`], [`HI_M`]), each × 255 rounded. A class whose `f` and `w`
//!   both round to 0 everywhere is left out; a tile with none isn't written. A level-8 tile (a z8
//!   area) may carry its heritage list after the WebP ([`heritage`]).

use super::world7;
use anyhow::{ensure, Context, Result};
use det::Det;
use rayon::prelude::*;
use std::collections::BTreeMap;

/// Texels a tile side.
pub const T: usize = 512;
/// Height classes.
pub const K: usize = 5;
/// The classes' least tops (dm) after the first.
pub const CLASS_TOP_DM: [u16; 4] = [80, 200, 400, 1000];
/// Each class's range of mean tops (m), as the encoding codes them.
pub const LO_M: [f64; K] = [0.0, 8.0, 20.0, 40.0, 100.0];
pub const HI_M: [f64; K] = [8.0, 20.0, 40.0, 100.0, 1000.0];
/// The finest level, and the hi packs' coarsest (a z8 area's), and the lo packs' coarsest.
pub const LEVEL_MAX: u8 = 14;
pub const LEVEL_HI_MIN: u8 = 9;
pub const LEVEL_MIN: u8 = 4;
pub const ALL_CLASSES: u8 = 0b11111;
/// The served layer (`layers/buildings-field/hi/6-x-y`, `lo/3-x-y`) and its encoding.
pub const LAYER: &str = "buildings-field";
pub const ENCODING: &str = "field3-webp";
/// Bumped when the lo step's output changes (every `bldfield-lo` target runs again).
pub const LO_V: u32 = 1;
/// A tile's planes, every class (MB).
const PLANES_MB: u64 = ((K * 3 * T * T * 4) >> 20) as u64;
/// What `bldfield-lo` holds at most (MB): one z6 tile's mid at a time, its 16 areas' planes both
/// decompressed and read (`read_mid`), the four level-7 tiles and the parent, the levels above's
/// four children each (levels 4 and 5), and 256 MB for the program.
pub const LO_PEAK_MB: u64 = 256 + 16 * PLANES_MB * 2 + (5 + 4 * 2) * PLANES_MB;

/// Fixed-point scales: coverage, coverage × top (m), wall area (m²).
const SF: f64 = (1u64 << 32) as f64;
const SV: f64 = (1u64 << 24) as f64;
const SW: f64 = (1u64 << 16) as f64;
/// Level-14 texels a z13 cell side, and z13 cells an area side.
const N: usize = 1024;
const STRIDE: usize = N + 2;
const CELLS: u32 = 32;
/// Level 14's texels a world side.
const WORLD_TEXELS: f64 = (1u64 << 23) as f64;

/// A class's planes: f, w, v.
pub type Class = [Vec<f32>; 3];

/// A level's planes over an `n` × `n` grid, each class's only where it holds something.
#[derive(Clone, Debug, PartialEq)]
pub struct Planes {
    pub n: usize,
    pub c: [Option<Box<Class>>; K],
}

impl Planes {
    pub fn empty(n: usize) -> Planes {
        Planes { n, c: Default::default() }
    }

    fn class_mut(&mut self, k: usize) -> &mut Class {
        let n = self.n;
        self.c[k].get_or_insert_with(|| Box::new([vec![0.0; n * n], vec![0.0; n * n], vec![0.0; n * n]]))
    }

    pub fn is_empty(&self) -> bool {
        self.c.iter().all(Option::is_none)
    }

    pub fn mask(&self) -> u8 {
        (0..K).filter(|&k| self.c[k].is_some()).fold(0, |m, k| m | 1 << k)
    }

    /// Quadrant `q` (0 nw, 1 ne, 2 sw, 3 se) of this level's parent, half as fine: each texel the
    /// mean of four, in a fixed order.
    fn half_into(&self, parent: &mut Planes, q: usize) {
        let h = self.n / 2;
        debug_assert_eq!(parent.n, self.n);
        let (ox, oy) = ((q % 2) * h, (q / 2) * h);
        for (k, c) in self.c.iter().enumerate() {
            let Some(c) = c else { continue };
            let pc = parent.class_mut(k);
            for p in 0..3 {
                let (a, b) = (&c[p], &mut pc[p]);
                for y in 0..h {
                    for x in 0..h {
                        let i = |dx: usize, dy: usize| (2 * y + dy) * self.n + 2 * x + dx;
                        b[(oy + y) * self.n + ox + x] = 0.25 * (((a[i(0, 0)] + a[i(1, 0)]) + a[i(0, 1)]) + a[i(1, 1)]);
                    }
                }
            }
        }
    }

    /// The parent of four tiles of this size (nw, ne, sw, se; None: nothing there), at the same
    /// size; None when all four are.
    pub fn parent(children: [Option<&Planes>; 4]) -> Option<Planes> {
        let n = children.iter().flatten().next()?.n;
        let mut p = Planes::empty(n);
        for (q, c) in children.iter().enumerate() {
            if let Some(c) = c {
                c.half_into(&mut p, q);
            }
        }
        Some(p)
    }

    /// The `T` × `T` window at (`tx`, `ty`) tiles.
    fn window(&self, tx: usize, ty: usize) -> Planes {
        let mut o = Planes::empty(T);
        for (k, c) in self.c.iter().enumerate() {
            let Some(c) = c else { continue };
            let oc = o.class_mut(k);
            for p in 0..3 {
                for y in 0..T {
                    let s = (ty * T + y) * self.n + tx * T;
                    oc[p][y * T..(y + 1) * T].copy_from_slice(&c[p][s..s + T]);
                }
            }
        }
        o
    }
}

/// The class of a top (dm).
pub fn class_of(h_dm: u16) -> usize {
    CLASS_TOP_DM.iter().filter(|&&t| h_dm >= t).count()
}

/// Metres a level-14 texel side at global texel row `y` (fractional; Web Mercator).
fn side_m(y: f64) -> f64 {
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / WORLD_TEXELS)).dsinh().datan();
    super::EQ * lat.dcos() / WORLD_TEXELS
}

/// A building or part as the field takes it: its polygons (each its rings, the first the exterior,
/// E7), top and base (dm).
pub struct Foot<'a> {
    pub polys: Vec<Vec<&'a [[i32; 2]]>>,
    pub h: u16,
    pub m: u16,
}

/// A z13 cell's accumulators for a class.
struct Acc {
    f: Vec<i64>,
    v: Vec<i64>,
    w: Vec<i64>,
}

/// Deposits the coverage of the edge p0–p1 (cell texels, x already in [0, N]) as font-rs's
/// accumulation does, `dir`ected: c into f, c × top into v.
fn line(acc: &mut Acc, p0: (f64, f64), p1: (f64, f64), wgt: f64, top: f64) {
    if p0.1 == p1.1 {
        return;
    }
    let (dir, p0, p1) = if p0.1 < p1.1 { (wgt, p0, p1) } else { (-wgt, p1, p0) };
    if p1.1 <= 0.0 || p0.1 >= N as f64 {
        return;
    }
    let mut put = |i: usize, c: f64| {
        acc.f[i] += (c * SF).round() as i64;
        acc.v[i] += (c * top * SV).round() as i64;
    };
    let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
    let mut x = p0.0;
    if p0.1 < 0.0 {
        x -= p0.1 * dxdy;
    }
    let y0 = p0.1.max(0.0) as usize;
    let y1 = (p1.1.ceil() as usize).min(N);
    for y in y0..y1 {
        let ls = y * STRIDE;
        let dy = ((y + 1) as f64).min(p1.1) - (y as f64).max(p0.1);
        let xnext = (x + dxdy * dy).clamp(0.0, N as f64);
        let d = dy * dir;
        let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
        let x0f = x0.floor();
        let x0i = x0f as usize;
        let x1c = x1.ceil();
        let x1i = x1c as usize;
        if x1i <= x0i + 1 {
            let xmf = 0.5 * (x + xnext) - x0f;
            put(ls + x0i, d - d * xmf);
            put(ls + x0i + 1, d * xmf);
        } else {
            let s = 1.0 / (x1 - x0);
            let x0r = x0 - x0f;
            let a0 = 0.5 * s * (1.0 - x0r) * (1.0 - x0r);
            let x1r = x1 - x1c + 1.0;
            let am = 0.5 * s * x1r * x1r;
            put(ls + x0i, d * a0);
            if x1i == x0i + 2 {
                put(ls + x0i + 1, d * (1.0 - a0 - am));
            } else {
                let a1 = s * (1.5 - x0r);
                put(ls + x0i + 1, d * (a1 - a0));
                for xi in x0i + 2..x1i - 1 {
                    put(ls + xi, d * s);
                }
                let a2 = a1 + (x1i - x0i - 3) as f64 * s;
                put(ls + x1i - 1, d * (1.0 - a2 - am));
            }
            put(ls + x1i, d * am);
        }
        x = xnext;
    }
}

/// An edge clipped to x in [0, N]: the parts left of 0 drawn on x = 0 (they cover every texel to
/// their right), those right of N on x = N (they cover none).
fn edge(acc: &mut Acc, a: (f64, f64), b: (f64, f64), wgt: f64, top: f64) {
    let w = N as f64;
    let mut pts = [a, a, a, b];
    let mut n = 1;
    let mut ts = [2.0f64; 2];
    let mut nt = 0;
    for xc in [0.0, w] {
        if (a.0 - xc) * (b.0 - xc) < 0.0 {
            ts[nt] = (xc - a.0) / (b.0 - a.0);
            nt += 1;
        }
    }
    if nt == 2 && ts[1] < ts[0] {
        ts.swap(0, 1);
    }
    for &t in &ts[..nt] {
        pts[n] = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        n += 1;
    }
    pts[n] = b;
    for i in 0..n {
        let c = |q: (f64, f64)| (q.0.clamp(0.0, w), q.1);
        line(acc, c(pts[i]), c(pts[i + 1]), wgt, top);
    }
}

/// Deposits a wall's area (`len_m` × `wh` m) into the cell's texels it crosses, piece by piece.
fn wall(wd: &mut [i64], a: (f64, f64), b: (f64, f64), area_m2: f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let mut ts = vec![0.0, 1.0];
    let (xa, xb) = (a.0.min(b.0), a.0.max(b.0));
    let (ya, yb) = (a.1.min(b.1), a.1.max(b.1));
    let n = N as f64;
    if xb < 0.0 || yb < 0.0 || xa >= n || ya >= n {
        return;
    }
    if dx != 0.0 {
        let mut g = xa.max(-1.0).floor() + 1.0;
        while g < xb.min(n + 1.0) {
            ts.push((g - a.0) / dx);
            g += 1.0;
        }
    }
    if dy != 0.0 {
        let mut g = ya.max(-1.0).floor() + 1.0;
        while g < yb.min(n + 1.0) {
            ts.push((g - a.1) / dy);
            g += 1.0;
        }
    }
    ts.sort_by(f64::total_cmp);
    for t in ts.windows(2) {
        let (t0, t1) = (t[0], t[1]);
        if t1 <= t0 {
            continue;
        }
        let tm = 0.5 * (t0 + t1);
        let (mx, my) = (a.0 + dx * tm, a.1 + dy * tm);
        if mx < 0.0 || my < 0.0 || mx >= n || my >= n {
            continue;
        }
        wd[my as usize * N + mx as usize] += ((t1 - t0) * area_m2 * SW).round() as i64;
    }
}

/// Twice a ring's signed area (its own units).
fn area2(r: &[(f64, f64)]) -> f64 {
    let mut a = 0.0;
    for i in 0..r.len() {
        let (p, q) = (r[i], r[(i + 1) % r.len()]);
        a += p.0 * q.1 - q.0 * p.1;
    }
    a
}

/// Z13 cell (`cx`, `cy`, global)'s level-14 planes (N × N), from the footprints meeting it.
fn cell(feet: &[Foot], list: &[u32], cx: u32, cy: u32) -> Planes {
    let (ox, oy) = (cx as f64 * N as f64, cy as f64 * N as f64);
    let mut acc: [Option<Acc>; K] = Default::default();
    for &fi in list {
        let ft = &feet[fi as usize];
        let k = class_of(ft.h);
        let a = acc[k].get_or_insert_with(|| Acc { f: vec![0; N * STRIDE], v: vec![0; N * STRIDE], w: vec![0; N * N] });
        let top = ft.h as f64 / 10.0;
        let wh = ft.h.saturating_sub(ft.m) as f64 / 10.0;
        for poly in &ft.polys {
            let rings: Vec<Vec<(f64, f64)>> = poly
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|&p| {
                            let (wx, wy) = world7(p);
                            (wx * WORLD_TEXELS - ox, wy * WORLD_TEXELS - oy)
                        })
                        .collect()
                })
                .collect();
            // (A wall's length in metres at its polygon's middle row.)
            let (mut y0, mut y1) = (f64::MAX, f64::MIN);
            for p in rings.iter().flatten() {
                y0 = y0.min(p.1);
                y1 = y1.max(p.1);
            }
            if rings.is_empty() || y0 > y1 {
                continue;
            }
            let side = side_m(oy + 0.5 * (y0 + y1));
            for (ri, r) in rings.iter().enumerate() {
                if r.len() < 3 {
                    continue;
                }
                // The exterior counted positive, the holes negative, whichever way they wind (the
                // accumulation counts a ring of negative area2, y down, positive).
                let s = area2(r);
                if s == 0.0 {
                    continue;
                }
                let wgt = if (ri == 0) == (s < 0.0) { 1.0 } else { -1.0 };
                for i in 0..r.len() {
                    let (p, q) = (r[i], r[(i + 1) % r.len()]);
                    edge(a, p, q, wgt, top);
                    if wh > 0.0 {
                        let len = (q.0 - p.0).dhypot(q.1 - p.1) * side;
                        wall(&mut a.w, p, q, len * wh);
                    }
                }
            }
        }
    }
    let mut out = Planes::empty(N);
    let ta: Vec<f64> = (0..N).map(|y| side_m(oy + y as f64 + 0.5).powi(2)).collect();
    for (k, a) in acc.iter().enumerate() {
        let Some(a) = a else { continue };
        let c = out.class_mut(k);
        for y in 0..N {
            let (mut cf, mut cv) = (0i64, 0i64);
            for x in 0..N {
                cf += a.f[y * STRIDE + x];
                cv += a.v[y * STRIDE + x];
                let (mut f, mut v) = (cf as f64 / SF, cv as f64 / SV);
                if f <= 0.0 || v < 0.0 {
                    (f, v) = (0.0, 0.0);
                } else if f > 1.0 {
                    // (overlapping roofs: the share clamped to 1, their tops' sum scaled with it,
                    // so v / f stays their mean top)
                    v /= f;
                    f = 1.0;
                }
                let i = y * N + x;
                c[0][i] = f as f32;
                c[1][i] = (a.w[i] as f64 / SW / ta[y]) as f32;
                c[2][i] = v as f32;
            }
        }
    }
    out
}

/// What an area's field made: its tiles, levels 14–8 ((level, x, y, `field3-webp` bytes), in that
/// order, each level by x, y), and its level-8 planes (the mid's; None: no building).
#[derive(Debug, Default, PartialEq)]
pub struct AreaField {
    pub tiles: Vec<(u8, u32, u32, Vec<u8>)>,
    pub mid: Option<Mid>,
}

/// An area's level-8 planes and its heritage list ([`heritage`]): what its mid keeps, and what
/// `bldfield-lo` makes its level-8 tile of.
#[derive(Clone, Debug, PartialEq)]
pub struct Mid {
    pub planes: Planes,
    pub heritage: Vec<u8>,
}

/// Area `a`'s field (a z8 tile) from the footprints meeting it (any order: the sums are integers),
/// its level-8 tile carrying the heritage list `her` ([`heritage`]).
pub fn area(a: (u32, u32), feet: &[Foot], her: &[u8]) -> AreaField {
    // The footprints by the area's z13 cells their boxes meet.
    let (ax, ay) = (a.0 as f64 * CELLS as f64 * N as f64, a.1 as f64 * CELLS as f64 * N as f64);
    let mut lists: Vec<Vec<u32>> = vec![Vec::new(); (CELLS * CELLS) as usize];
    for (fi, ft) in feet.iter().enumerate() {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for p in ft.polys.iter().flatten().flat_map(|r| r.iter()) {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        if x0 > x1 {
            continue;
        }
        // (World y grows southward: the box's north edge is its least y.)
        let (wx0, wy0) = world7([x0, y1]);
        let (wx1, wy1) = world7([x1, y0]);
        let c = |v: f64, o: f64| ((v * WORLD_TEXELS - o) / N as f64).floor();
        let (cx0, cx1, cy0, cy1) = (c(wx0, ax), c(wx1, ax), c(wy0, ay), c(wy1, ay));
        let top = CELLS as f64 - 1.0;
        if cx1 < 0.0 || cy1 < 0.0 || cx0 > top || cy0 > top {
            continue;
        }
        for cy in cy0.max(0.0) as u32..=cy1.min(top) as u32 {
            for cx in cx0.max(0.0) as u32..=cx1.min(top) as u32 {
                lists[(cy * CELLS + cx) as usize].push(fi as u32);
            }
        }
    }
    // Levels 14 and 13 a cell, 12 a z12 block, 11 a z11 block, 10 a z10 block, then 9 and 8: a
    // tree, so a block's finer planes are dropped once its parent has them.
    type Tiles = Vec<(u8, u32, u32, Vec<u8>)>;
    let base = (a.0 * CELLS, a.1 * CELLS);
    let z12 = |bx: u32, by: u32| -> (Tiles, Option<Planes>) {
        let kids: Vec<(Tiles, Option<Planes>)> = (0..4u32)
            .into_par_iter()
            .map(|q| {
                let (cx, cy) = (bx * 2 + q % 2, by * 2 + q / 2);
                let list = &lists[((cy - base.1) * CELLS + cx - base.0) as usize];
                if list.is_empty() {
                    return (Vec::new(), None);
                }
                let p14 = cell(feet, list, cx, cy);
                let mut tiles = Vec::new();
                for ty in 0..2 {
                    for tx in 0..2 {
                        if let Some(b) = encode(&p14.window(tx, ty), ALL_CLASSES, &[]) {
                            tiles.push((14, cx * 2 + tx as u32, cy * 2 + ty as u32, b));
                        }
                    }
                }
                // (Level 13 from the cell's 1024 × 1024: its four 512s' parent.)
                let w: Vec<Planes> = (0..4).map(|q| p14.window(q % 2, q / 2)).collect();
                drop(p14);
                let p13 = Planes::parent([Some(&w[0]), Some(&w[1]), Some(&w[2]), Some(&w[3])]).unwrap();
                if let Some(b) = encode(&p13, ALL_CLASSES, &[]) {
                    tiles.push((13, cx, cy, b));
                }
                (tiles, (!p13.is_empty()).then_some(p13))
            })
            .collect();
        up(12, bx, by, kids)
    };
    let z11 = |bx: u32, by: u32| -> (Tiles, Option<Planes>) {
        let kids: Vec<(Tiles, Option<Planes>)> = (0..4u32).into_par_iter().map(|q| z12(bx * 2 + q % 2, by * 2 + q / 2)).collect();
        up(11, bx, by, kids)
    };
    let z10: Vec<(Tiles, Option<Planes>)> = (0..16u32)
        .into_par_iter()
        .map(|i| {
            let (bx, by) = (a.0 * 4 + i % 4, a.1 * 4 + i / 4);
            let kids: Vec<(Tiles, Option<Planes>)> = (0..4u32).into_par_iter().map(|q| z11(bx * 2 + q % 2, by * 2 + q / 2)).collect();
            up(10, bx, by, kids)
        })
        .collect();
    let mut z10: Vec<Option<(Tiles, Option<Planes>)>> = z10.into_iter().map(Some).collect();
    let z9: Vec<(Tiles, Option<Planes>)> = (0..4u32)
        .map(|q| {
            let (bx, by) = (a.0 * 2 + q % 2, a.1 * 2 + q / 2);
            let kids: Vec<(Tiles, Option<Planes>)> = (0..4u32).map(|c| {
                let (x, y) = (bx * 2 + c % 2 - a.0 * 4, by * 2 + c / 2 - a.1 * 4);
                z10[(y * 4 + x) as usize].take().unwrap()
            }).collect();
            up(9, bx, by, kids)
        })
        .collect();
    // (Level 8 a z8 area's own tile: the lo packs', made by `lo` from the mid.)
    let mut tiles = Vec::new();
    let mut planes: Vec<Option<Planes>> = Vec::with_capacity(4);
    for (t, p) in z9 {
        tiles.extend(t);
        planes.push(p);
    }
    let mid = Planes::parent([planes[0].as_ref(), planes[1].as_ref(), planes[2].as_ref(), planes[3].as_ref()]).map(|planes| Mid { planes, heritage: her.to_vec() });
    tiles.sort_by(|p, q| q.0.cmp(&p.0).then((p.1, p.2).cmp(&(q.1, q.2))));
    AreaField { tiles, mid }
}

/// A level's tile (`z`, `x`, `y`) from its four children's (nw, ne, sw, se: their tiles and planes):
/// the children's tiles and its own, and its planes.
fn up(z: u8, x: u32, y: u32, kids: Vec<(Vec<(u8, u32, u32, Vec<u8>)>, Option<Planes>)>) -> (Vec<(u8, u32, u32, Vec<u8>)>, Option<Planes>) {
    let mut tiles = Vec::new();
    let mut planes: Vec<Option<Planes>> = Vec::with_capacity(4);
    for (t, p) in kids {
        tiles.extend(t);
        planes.push(p);
    }
    let p = Planes::parent([planes[0].as_ref(), planes[1].as_ref(), planes[2].as_ref(), planes[3].as_ref()]);
    if let Some(p) = &p {
        if let Some(b) = encode(p, ALL_CLASSES, &[]) {
            tiles.push((z, x, y, b));
        }
    }
    (tiles, p)
}

/// Quantized texels of a class: (√f, log2(1 + w) ÷ log2(4097), the mean top in its class's range),
/// × 255 rounded.
fn quantize(c: &Class, k: usize, i: usize) -> [u8; 3] {
    let (f, w, v) = (c[0][i] as f64, c[1][i] as f64, c[2][i] as f64);
    let f = f.clamp(0.0, 1.0);
    let qf = (f.sqrt() * 255.0).round();
    let qw = ((1.0 + w.max(0.0)).dlog2() / 4097f64.dlog2() * 255.0).round().min(255.0);
    if qf == 0.0 && qw == 0.0 {
        return [0; 3];
    }
    let h = if f > 0.0 { v / f } else { 0.0 };
    let qh = ((h - LO_M[k]) / (HI_M[k] - LO_M[k])).clamp(0.0, 1.0) * 255.0;
    [qf as u8, qw as u8, qh.round() as u8]
}

/// A tile's `field3-webp` bytes (the classes in `allowed` it holds), `tail` after them; None when
/// it holds nothing.
pub fn encode(p: &Planes, allowed: u8, tail: &[u8]) -> Option<Vec<u8>> {
    assert_eq!(p.n, T);
    let mut mask = 0u8;
    let mut rgb: Vec<u8> = Vec::new();
    for k in 0..K {
        let Some(c) = p.c[k].as_ref().filter(|_| allowed & 1 << k != 0) else { continue };
        let at = rgb.len();
        rgb.reserve(T * T * 3);
        let mut any = false;
        for i in 0..T * T {
            let q = quantize(c, k, i);
            any |= q[0] != 0 || q[1] != 0;
            rgb.extend_from_slice(&q);
        }
        if any {
            mask |= 1 << k;
        } else {
            rgb.truncate(at);
        }
    }
    if mask == 0 {
        return None;
    }
    let n = mask.count_ones();
    // (Regions of 128 px for the codes' groups: 32 px took 6.8× the time for 0.25 % less, Annecy's tiles.)
    let webp = crate::webp::encode_rgb_regions(&rgb, T as u32, T as u32 * n, 7);
    let mut out = Vec::with_capacity(1 + webp.len() + tail.len());
    out.push(mask);
    out.extend_from_slice(&webp);
    out.extend_from_slice(tail);
    Some(out)
}

/// A `field3-webp` tile's parts: its class mask, its WebP, and what follows (a heritage list).
pub fn split(b: &[u8]) -> Result<(u8, &[u8], &[u8])> {
    ensure!(b.len() >= 13 && &b[1..5] == b"RIFF", "not a field3-webp tile");
    let riff = u32::from_le_bytes(b[5..9].try_into().unwrap()) as usize;
    let end = 1 + 8 + riff;
    ensure!(end <= b.len(), "a field3-webp tile cut short");
    Ok((b[0], &b[1..end], &b[end..]))
}

// ---- heritage ------------------------------------------------------------------------------------

/// Points a heritage footprint keeps at most.
pub const HERITAGE_PTS: usize = 16;

/// Whether E7 point `p` is inside a polygon (even-odd over its rings).
fn inside(poly: &[&[[i32; 2]]], p: [i32; 2]) -> bool {
    let mut c = false;
    for r in poly {
        let n = r.len();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + n - 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let x = a[0] as i128 + (b[0] as i128 - a[0] as i128) * (p[1] as i128 - a[1] as i128) / (b[1] as i128 - a[1] as i128);
                if (p[0] as i128) < x {
                    c = !c;
                }
            }
        }
    }
    c
}

/// Douglas–Peucker of a closed ring, its tolerance doubled until it keeps at most `max` points
/// (the first kept).
fn simplify_to(r: &[[f64; 2]], max: usize) -> Vec<[f64; 2]> {
    if r.len() <= max {
        return r.to_vec();
    }
    let mut ring = r.to_vec();
    ring.push(r[0]);
    let mut tol = 0.5;
    loop {
        let mut keep = vec![false; ring.len()];
        keep[0] = true;
        *keep.last_mut().unwrap() = true;
        let mut stack = vec![(0usize, ring.len() - 1)];
        while let Some((a, b)) = stack.pop() {
            if b <= a + 1 {
                continue;
            }
            let (mut best, mut far) = (0.0f64, a);
            for i in a + 1..b {
                let (p, s, e) = (ring[i], ring[a], ring[b]);
                let (dx, dy) = (e[0] - s[0], e[1] - s[1]);
                let l2 = dx * dx + dy * dy;
                let t = if l2 == 0.0 { 0.0 } else { (((p[0] - s[0]) * dx + (p[1] - s[1]) * dy) / l2).clamp(0.0, 1.0) };
                let (ex, ey) = (p[0] - s[0] - t * dx, p[1] - s[1] - t * dy);
                let d = ex * ex + ey * ey;
                if d > best {
                    best = d;
                    far = i;
                }
            }
            if best > tol * tol {
                keep[far] = true;
                stack.push((a, far));
                stack.push((far, b));
            }
        }
        let mut out: Vec<[f64; 2]> = ring.iter().zip(&keep).filter(|(_, &k)| k).map(|(p, _)| *p).collect();
        out.pop();
        if out.len() <= max {
            return out;
        }
        tol *= 2.0;
    }
}

/// Area `a`'s heritage list: of the footprints given (the area's own buildings and parts, in their
/// order), those holding one of the sites' points `sites` (E7; a building's polygons, any), each as
/// u16 top (dm), u16 base (dm), u8 n, then n points (u16 x, u16 y) in the z8 tile (its first
/// polygon's exterior, simplified to at most [`HERITAGE_PTS`]); after a u32 count. Empty: none.
pub fn heritage(a: (u32, u32), feet: &[Foot], sites: &[[i32; 2]]) -> Vec<u8> {
    if sites.is_empty() {
        return Vec::new();
    }
    let mut sorted = sites.to_vec();
    sorted.sort_unstable();
    let mut out = Vec::new();
    let mut n = 0u32;
    for ft in feet {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for p in ft.polys.iter().flatten().flat_map(|r| r.iter()) {
            x0 = x0.min(p[0]);
            x1 = x1.max(p[0]);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
        let from = sorted.partition_point(|s| s[0] < x0);
        let hit = sorted[from..].iter().take_while(|s| s[0] <= x1).any(|&s| s[1] >= y0 && s[1] <= y1 && ft.polys.iter().any(|poly| inside(poly, s)));
        if !hit {
            continue;
        }
        let Some(ext) = ft.polys.first().and_then(|p| p.first()) else { continue };
        let scale = 256.0 * 65536.0;
        let ring: Vec<[f64; 2]> = ext.iter().map(|&p| {
            let (wx, wy) = world7(p);
            [wx * scale - a.0 as f64 * 65536.0, wy * scale - a.1 as f64 * 65536.0]
        }).collect();
        let pts = simplify_to(&ring, HERITAGE_PTS);
        out.extend_from_slice(&ft.h.to_le_bytes());
        out.extend_from_slice(&ft.m.to_le_bytes());
        out.push(pts.len() as u8);
        for p in pts {
            for v in p {
                out.extend_from_slice(&(v.round().clamp(0.0, 65535.0) as u16).to_le_bytes());
            }
        }
        n += 1;
    }
    if n == 0 {
        return Vec::new();
    }
    let mut b = n.to_le_bytes().to_vec();
    b.extend(out);
    b
}

/// A z6 tile's heritage sites from its markdata (`marks::KINDS`' heritage, a World Heritage Site's
/// components left out: they aren't dots), E7.
pub fn sites_of_markdata(path: &std::path::Path) -> Result<Vec<[i32; 2]>> {
    let f = store::range::PlainFile::open(path).with_context(|| format!("open {}", path.display()))?;
    let r = store::sect::SectReader::open(f)?;
    let kinds: Vec<[u32; 2]> = r.read_pod("kinds")?;
    let pts: Vec<crate::marks::MarkPt> = r.read_pod("pts")?;
    let k = crate::marks::kind_index("heritage").unwrap();
    let Some(&[first, n]) = kinds.get(k) else { return Ok(Vec::new()) };
    let mut out: Vec<[i32; 2]> = pts.get(first as usize..(first + n) as usize).context("markdata's kinds past its points")?.iter().filter(|p| p.flags & crate::marks::flag::COMPONENT == 0).map(|p| [p.lon, p.lat]).collect();
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

// ---- mids and the lo levels ------------------------------------------------------------------------

const MID_MAGIC: &[u8; 8] = b"BFMID02\0";

/// A z6 tile's mid: its areas' level-8 planes, exactly, and their heritage lists (magic, then each
/// area by (x, y): u32 x, u32 y, u32 the heritage list's bytes and those bytes, u8 class mask,
/// each class's f, w, v as 512 × 512 f32 LE), zstd'd.
pub fn write_mid(areas: &BTreeMap<(u32, u32), Mid>) -> Result<Vec<u8>> {
    let mut raw = MID_MAGIC.to_vec();
    for (&(x, y), m) in areas {
        let p = &m.planes;
        ensure!(p.n == T, "a mid's planes are a tile's");
        raw.extend_from_slice(&x.to_le_bytes());
        raw.extend_from_slice(&y.to_le_bytes());
        raw.extend_from_slice(&(m.heritage.len() as u32).to_le_bytes());
        raw.extend_from_slice(&m.heritage);
        raw.push(p.mask());
        for c in p.c.iter().flatten() {
            for plane in c.iter() {
                raw.extend_from_slice(bytemuck::cast_slice(plane));
            }
        }
    }
    Ok(zstd::bulk::compress(&raw, 3)?)
}

pub fn read_mid(b: &[u8]) -> Result<BTreeMap<(u32, u32), Mid>> {
    let raw = zstd::stream::decode_all(b)?;
    ensure!(raw.starts_with(MID_MAGIC), "not a field mid");
    let mut at = MID_MAGIC.len();
    let mut out = BTreeMap::new();
    while at < raw.len() {
        ensure!(at + 12 <= raw.len(), "a field mid cut short");
        let x = u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
        let y = u32::from_le_bytes(raw[at + 4..at + 8].try_into().unwrap());
        let hl = u32::from_le_bytes(raw[at + 8..at + 12].try_into().unwrap()) as usize;
        at += 12;
        ensure!(at + hl < raw.len(), "a field mid cut short");
        let heritage = raw[at..at + hl].to_vec();
        at += hl;
        let mask = raw[at];
        at += 1;
        let mut p = Planes::empty(T);
        for k in 0..K {
            if mask & 1 << k == 0 {
                continue;
            }
            let c = p.class_mut(k);
            for plane in c.iter_mut() {
                let len = T * T * 4;
                ensure!(at + len <= raw.len(), "a field mid cut short");
                for (i, ch) in raw[at..at + len].chunks_exact(4).enumerate() {
                    plane[i] = f32::from_le_bytes(ch.try_into().unwrap());
                }
                at += len;
            }
        }
        out.insert((x, y), Mid { planes: p, heritage });
    }
    Ok(out)
}

/// The lo levels (8–4) of z3 tile `q`, from its z6 tiles' mids (`mid(x, y)`: None where there's
/// none; level 8 each area's planes, its heritage list after them): (level, x, y, `field3-webp`),
/// by level from 8, then x, y.
pub fn lo(q: (u32, u32), mid: &(dyn Fn(u32, u32) -> Result<Option<Vec<u8>>> + Sync)) -> Result<Vec<(u8, u32, u32, Vec<u8>)>> {
    type Out = (Vec<(u8, u32, u32, Vec<u8>)>, Option<Planes>);
    fn node(z: u8, x: u32, y: u32, mid: &(dyn Fn(u32, u32) -> Result<Option<Vec<u8>>> + Sync)) -> Result<Out> {
        if z == 6 {
            let Some(b) = mid(x, y)? else { return Ok((Vec::new(), None)) };
            let areas = read_mid(&b)?;
            let get = |ax: u32, ay: u32| areas.get(&(ax, ay)).map(|m| &m.planes);
            let mut tiles = Vec::new();
            for (&(ax, ay), m) in &areas {
                if let Some(b) = encode(&m.planes, ALL_CLASSES, &m.heritage) {
                    tiles.push((8, ax, ay, b));
                }
            }
            let mut l7: Vec<Option<Planes>> = Vec::new();
            for q in 0..4u32 {
                let (x7, y7) = (x * 2 + q % 2, y * 2 + q / 2);
                let p = Planes::parent([get(x7 * 2, y7 * 2), get(x7 * 2 + 1, y7 * 2), get(x7 * 2, y7 * 2 + 1), get(x7 * 2 + 1, y7 * 2 + 1)]);
                if let Some(b) = p.as_ref().and_then(|p| encode(p, ALL_CLASSES, &[])) {
                    tiles.push((7, x7, y7, b));
                }
                l7.push(p);
            }
            let p6 = Planes::parent([l7[0].as_ref(), l7[1].as_ref(), l7[2].as_ref(), l7[3].as_ref()]);
            if let Some(b) = p6.as_ref().and_then(|p| encode(p, ALL_CLASSES, &[])) {
                tiles.push((6, x, y, b));
            }
            return Ok((tiles, p6));
        }
        // (One z6 tile's mid at a time: `LO_PEAK_MB`.)
        let kids: Vec<Out> = (0..4u32).map(|q| node(z + 1, x * 2 + q % 2, y * 2 + q / 2, mid)).collect::<Result<_>>()?;
        Ok(up(z, x, y, kids))
    }
    let mut tiles = Vec::new();
    for qq in 0..4u32 {
        tiles.extend(node(4, q.0 * 2 + qq % 2, q.1 * 2 + qq / 2, mid)?.0);
    }
    tiles.sort_by(|p, q| q.0.cmp(&p.0).then((p.1, p.2).cmp(&(q.1, q.2))));
    Ok(tiles)
}

/// The logical names: z6 tile T's hi pack and mid, z3 tile Q's lo pack.
pub fn hi_logical(x: u32, y: u32) -> String {
    format!("layers/{LAYER}/hi/6-{x}-{y}")
}
pub fn mid_logical(x: u32, y: u32) -> String {
    format!("work/bldfield-mid/6-{x}-{y}")
}
pub fn lo_logical(x: u32, y: u32) -> String {
    format!("layers/{LAYER}/lo/3-{x}-{y}")
}
/// Z6 tile T's markdata (its heritage sites).
pub fn markdata_logical(x: u32, y: u32) -> String {
    format!("markdata/6-{x}-{y}")
}

#[cfg(test)]
mod tests;
