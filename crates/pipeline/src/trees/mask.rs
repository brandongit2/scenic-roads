//! Which pixels of a zoom-8 block are inside the coverage, as trees.py's `shape_mask` has rasterio
//! (GDAL 3.12) burn them: each ring of a shape a polygon of its own, added up (MERGE_ALG=ADD), a
//! pixel inside a shape where an odd number of its rings hold it, inside the coverage where it's
//! inside any shape.
//! - **Rings** as trees.py loads them (`load_shapes`): densified so no edge spans more than 0.05° on
//!   either axis, in Mercator metres, closed as shapely closes them, and walked clockwise (OGR's
//!   test, crate::areaflags).
//! - **Burn:** GDAL's scanline fill (GDALdllImageFilledPolygon): a pixel whose centre the ring holds;
//!   a horizontal edge on a row of centres, walked right to left, burns along it, unless a span of
//!   that row starts where it does (GDAL's guard against adding twice).
//! - **Arithmetic:** GDAL's, as built for the Mac's Python (as crate::areaflags): rasterio's
//!   `from_bounds` transform inverted (GDALInvGeoTransform) and applied with fused multiply-adds.
//!   Mercator's tangent and logarithm are det's where trees.py's are Apple's (a few ulp apart): a
//!   pixel could differ only where an edge passes within about 1e-9 pixels of its centre.

use super::{merc, BS};
use anyhow::{ensure, Context, Result};

/// A coverage ring as trees.py loads it: its box in degrees (west, south, east, north, of its
/// points densified) and its points in Mercator metres, closed and clockwise.
pub struct Ring {
    pub bbox: [f64; 4],
    merc: Vec<[f64; 2]>,
}

/// The coverage's shapes (cov.json, crate::treepacks): each shape's rings.
pub struct Shapes {
    pub shapes: Vec<Vec<Ring>>,
}

/// A ring (degrees) with points added so no edge spans more than `most` degrees on either axis,
/// as trees.py's `densify` adds them (its edges, not the closing one).
fn densify(r: &[[f64; 2]], most: f64) -> Vec<[f64; 2]> {
    let mut out = vec![r[0]];
    for e in r.windows(2) {
        let (a, b) = (e[0], e[1]);
        let d = [b[0] - a[0], b[1] - a[1]];
        let k = ((d[0].abs().max(d[1].abs()) / most).ceil() as i64).max(1);
        for i in 1..=k {
            let t = i as f64 / k as f64;
            out.push([a[0] + t * d[0], a[1] + t * d[1]]);
        }
    }
    out
}

impl Ring {
    fn new(r: &[[f64; 2]]) -> Result<Ring> {
        let d = densify(r, 0.05);
        let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in &d {
            bbox = [bbox[0].min(p[0]), bbox[1].min(p[1]), bbox[2].max(p[0]), bbox[3].max(p[1])];
        }
        let mut m: Vec<[f64; 2]> = d.iter().map(|p| merc(p[0], p[1])).collect();
        // (shapely closes a ring that isn't; OGR walks it clockwise.)
        if m.first() != m.last() {
            m.push(m[0]);
        }
        ensure!(m.len() >= 4, "a ring of {} points", m.len());
        if !crate::areaflags::is_clockwise(&m) {
            m.reverse();
        }
        Ok(Ring { bbox, merc: m })
    }

    fn meets(&self, b: [f64; 4]) -> bool {
        self.bbox[0] <= b[2] && self.bbox[2] >= b[0] && self.bbox[1] <= b[3] && self.bbox[3] >= b[1]
    }
}

impl Shapes {
    /// From cov.json's text: `{"shapes": [[ring, …], …]}`, each ring's points in degrees; a ring of
    /// fewer than 3 points is left out, as trees.py leaves it.
    pub fn parse(json: &str) -> Result<Shapes> {
        #[derive(serde::Deserialize)]
        struct Cov {
            shapes: Vec<Vec<Vec<[f64; 2]>>>,
        }
        let c: Cov = serde_json::from_str(json).context("the coverage's shapes")?;
        let shapes = c.shapes.iter().map(|rings| rings.iter().filter(|r| r.len() >= 3).map(|r| Ring::new(r)).collect::<Result<Vec<_>>>()).collect::<Result<_>>()?;
        Ok(Shapes { shapes })
    }

    /// Each shape's rings whose box meets box `b` (degrees: a ring that doesn't can't change which
    /// points there are inside), trees.py's `shapes_meeting`.
    pub fn meeting(&self, b: [f64; 4]) -> Vec<Vec<&Ring>> {
        self.shapes.iter().map(|rings| rings.iter().filter(|r| r.meets(b)).collect()).collect()
    }

    /// Whether any ring's box meets box `b`.
    pub fn meets(&self, b: [f64; 4]) -> bool {
        self.shapes.iter().flatten().any(|r| r.meets(b))
    }
}

/// Words of a block's mask: a bit per pixel, row by row, 64 a word.
const WORDS: usize = BS * BS / 64;

/// The pixels of the block of box `b` (degrees) inside `shapes` (`Shapes::meeting`'s).
pub fn inside(shapes: &[Vec<&Ring>], b: [f64; 4]) -> Vec<u64> {
    let mut out = vec![0u64; WORDS];
    // rasterio's from_bounds(west, south, east, north, BS, BS), inverted by GDAL.
    let ([west, south], [east, north]) = (merc(b[0], b[1]), merc(b[2], b[3]));
    let (sx, sy) = ((east - west) / BS as f64, (south - north) / BS as f64);
    let (ix0, ix1, iy0, iy1) = (-west / sx, 1.0 / sx, -north / sy, 1.0 / sy);
    let mut odd = vec![0u64; WORDS];
    let mut s = Scratch::default();
    for rings in shapes.iter().filter(|r| !r.is_empty()) {
        odd.fill(0);
        for r in rings {
            s.pts.clear();
            s.pts.extend(r.merc.iter().map(|q| [q[0].mul_add(ix1, ix0), q[1].mul_add(iy1, iy0)]));
            fill(&mut s, &mut odd);
        }
        for (o, v) in out.iter_mut().zip(&odd) {
            *o |= v;
        }
    }
    out
}

#[derive(Default)]
struct Scratch {
    /// The ring's points in pixels.
    pts: Vec<[f64; 2]>,
    /// Per row of the ring, the edges (their second point) that can cross its line of centres.
    rows: Vec<Vec<u32>>,
    ints: Vec<i32>,
    ints2: Vec<i32>,
}

/// Flips in `bits` the pixels GDAL's scanline fill burns for the closed ring in `s.pts` (pixels):
/// adding one to each, of which only whether it's odd is kept.
fn fill(s: &mut Scratch, bits: &mut [u64]) {
    let pts = &s.pts;
    let n = pts.len();
    if n == 0 {
        return;
    }
    let (mut dminy, mut dmaxy) = (pts[0][1], pts[0][1]);
    for p in &pts[1..] {
        if p[1] < dminy {
            dminy = p[1];
        } else if p[1] > dmaxy {
            dmaxy = p[1];
        }
    }
    let size = BS as i32;
    let miny = (if 0.0 < dminy { dminy } else { 0.0 }) as i32;
    let maxy = (if ((size - 1) as f64) < dmaxy { (size - 1) as f64 } else { dmaxy }) as i32;
    if miny > maxy {
        return;
    }
    let maxx = size - 1;
    // Edge i runs from point i - 1 (the last, for the first) to point i; it goes to the rows whose
    // line of centres it can reach, where GDAL's test (below) decides.
    let nrows = (maxy - miny + 1) as usize;
    if s.rows.len() < nrows {
        s.rows.resize_with(nrows, Vec::new);
    }
    for r in &mut s.rows[..nrows] {
        r.clear();
    }
    for i in 0..n {
        let (a, b) = (pts[if i == 0 { n - 1 } else { i - 1 }][1], pts[i][1]);
        let lo = a.min(b).floor().max(miny as f64);
        let hi = a.max(b).ceil().min(maxy as f64);
        if lo > hi {
            continue;
        }
        for y in lo as i32..=hi as i32 {
            s.rows[(y - miny) as usize].push(i as u32);
        }
    }
    let mut burn = |y: i32, x0: i32, x1: i32| {
        if x0 > x1 {
            return;
        }
        let (x0, x1) = (x0.max(0) as usize, x1.min(size - 1) as usize);
        let row = &mut bits[y as usize * BS / 64..(y as usize + 1) * BS / 64];
        let (w0, w1) = (x0 / 64, x1 / 64);
        for (w, v) in row.iter_mut().enumerate().take(w1 + 1).skip(w0) {
            let lo = if w == w0 { x0 % 64 } else { 0 };
            let hi = if w == w1 { x1 % 64 } else { 63 };
            *v ^= (u64::MAX >> (63 - hi)) & (u64::MAX << lo);
        }
    };
    for y in miny..=maxy {
        let dy = y as f64 + 0.5;
        s.ints.clear();
        s.ints2.clear();
        for &i in &s.rows[(y - miny) as usize] {
            let (i1, i2) = (if i == 0 { n - 1 } else { i as usize - 1 }, i as usize);
            let (mut dy1, mut dy2) = (pts[i1][1], pts[i2][1]);
            if (dy1 < dy && dy2 < dy) || (dy1 > dy && dy2 > dy) {
                continue;
            }
            let (dx1, dx2);
            if dy1 < dy2 {
                (dx1, dx2) = (pts[i1][0], pts[i2][0]);
            } else if dy1 > dy2 {
                std::mem::swap(&mut dy1, &mut dy2);
                (dx1, dx2) = (pts[i2][0], pts[i1][0]);
            } else {
                // A horizontal edge: burnt when walked right to left (the polygon's bottom), apart.
                if pts[i1][0] > pts[i2][0] {
                    let (h1, h2) = ((pts[i2][0] + 0.5).floor(), (pts[i1][0] + 0.5).floor());
                    if h1 > maxx as f64 || h2 <= 0.0 {
                        continue;
                    }
                    s.ints2.push((if h1 < 0.0 { 0.0 } else { h1 }) as i32);
                    s.ints2.push((if (size as f64) < h2 { size as f64 } else { h2 }) as i32);
                }
                continue;
            }
            if dy < dy2 && dy >= dy1 {
                let x = (dy - dy1) * (dx2 - dx1) / (dy2 - dy1) + dx1;
                let x = x.clamp(i32::MIN as f64, i32::MAX as f64);
                s.ints.push((x + 0.5).floor() as i32);
            }
        }
        s.ints.sort_unstable();
        s.ints2.sort_unstable();
        for w in s.ints.as_chunks::<2>().0 {
            if w[0] <= maxx && w[1] > 0 {
                burn(y, w[0], w[1] - 1);
            }
        }
        // A horizontal edge's run, unless a span starts where it does.
        let mut i = 0;
        for w in s.ints2.as_chunks::<2>().0 {
            if w[0] <= maxx && w[1] > 0 {
                while i + 1 < s.ints.len() && s.ints[i] < w[0] {
                    i += 2;
                }
                if i + 1 >= s.ints.len() || s.ints[i] != w[0] {
                    burn(y, w[0], w[1] - 1);
                }
            }
        }
    }
}

#[cfg(test)]
impl Ring {
    pub(crate) fn merc_for_test(&self) -> &[[f64; 2]] {
        &self.merc
    }
}

#[cfg(test)]
pub(crate) fn fill_ring_for_test(pts: &[[f64; 2]]) -> Vec<u64> {
    let mut s = Scratch { pts: pts.to_vec(), ..Default::default() };
    let mut bits = vec![0u64; WORDS];
    fill(&mut s, &mut bits);
    bits
}
