//! A region's stored shapes as rings with its inside on their left, without an overlay where none
//! is needed (docs/steps.md §15, reading).
//!
//! A shape is even–odd: a ring is an outer one (turned counter-clockwise) when an even number of
//! the shape's other rings hold it, a hole (clockwise) when odd, counted by casting a ray west from
//! the midpoint of one of its edges (one on no other ring's edge). A region's shapes that only
//! touch (its land and its water along their coast, a cell's piece and the next) need nothing
//! more: their shared edges have the region on both sides and the topology drops them (taking a
//! point within its `NEAR` of an edge as on it: the land's and the water's coasts were each rounded
//! to the grid, and a point of one lies up to 1.5 units inside the other). Shapes that overlap (a
//! ring's point or an edge's midpoint inside another shape, farther than that from its edges)
//! can't be read that way: that region's
//! shapes are unioned instead (`union`), the one costly path.

use codecs::regionlayer as fmt;
use std::cmp::Ordering;

type P = [i64; 2];

/// A shape's edges by bands of latitude, for its rays: each edge in every band its y-range meets,
/// an edge across more than `LONG` bands in `long` instead (scanned for every ray).
struct Bands {
    y0: i64,
    h: i64,
    bands: Vec<Vec<(P, P, u32)>>,
    long: Vec<(P, P, u32)>,
}

const LONG: i64 = 64;

impl Bands {
    /// `rings`' edges (points doubled, so an edge's midpoint is a point), each with its ring.
    fn of(rings: &[Vec<P>]) -> Bands {
        let (mut lo, mut hi, mut n) = (i64::MAX, i64::MIN, 0usize);
        for r in rings {
            for p in r {
                lo = lo.min(p[1]);
                hi = hi.max(p[1]);
            }
            n += r.len();
        }
        if lo > hi {
            return Bands { y0: 0, h: 1, bands: vec![], long: vec![] };
        }
        let count = (n / 8).clamp(1, 1 << 14) as i64;
        let h = ((hi - lo) / count + 1).max(1);
        let mut b = Bands { y0: lo, h, bands: vec![Vec::new(); ((hi - lo) / h + 1) as usize], long: vec![] };
        for (i, r) in rings.iter().enumerate() {
            for k in 0..r.len() {
                let (a, c) = (r[k], r[(k + 1) % r.len()]);
                let (b0, b1) = (b.band(a[1].min(c[1])), b.band(a[1].max(c[1])));
                if b1 - b0 > LONG as usize {
                    b.long.push((a, c, i as u32));
                } else {
                    for band in &mut b.bands[b0..=b1] {
                        band.push((a, c, i as u32));
                    }
                }
            }
        }
        b
    }

    fn band(&self, y: i64) -> usize {
        (((y - self.y0) / self.h).max(0) as usize).min(self.bands.len().saturating_sub(1))
    }

    /// The edges whose band holds `y` (none outside the shape's latitudes), and the long ones.
    fn near(&self, y: i64) -> impl Iterator<Item = &(P, P, u32)> {
        let inside = !self.bands.is_empty() && y >= self.y0 && y <= self.y0 + self.h * self.bands.len() as i64;
        let band: &[(P, P, u32)] = if inside { &self.bands[self.band(y)] } else { &[] };
        band.iter().chain(self.long.iter())
    }

    /// Where `p` is for the rings other than `skip`: on one of their edges (None), else whether an
    /// odd number of them hold it (a ray west, half-open in y).
    fn parity(&self, p: P, skip: Option<u32>) -> Option<bool> {
        let mut odd = false;
        for &(a, b, r) in self.near(p[1]) {
            if Some(r) == skip {
                continue;
            }
            if on_segment(a, b, p) {
                return None;
            }
            if (a[1] > p[1]) != (b[1] > p[1]) {
                // (The edge's x at p's y, left of p: the sign of a cross product, exactly.)
                let (lo, hi) = if a[1] < b[1] { (a, b) } else { (b, a) };
                if cross(lo, hi, p) > 0 {
                    odd = !odd;
                }
            }
        }
        Some(odd)
    }

    /// Whether an edge lies within `d` of `p` (doubled units both).
    fn within(&self, p: P, d: i64) -> bool {
        let close = |e: &(P, P, u32)| dist_at_most(e.0, e.1, p, d);
        if self.bands.is_empty() {
            return self.long.iter().any(close);
        }
        let (b0, b1) = (self.band(p[1] - d), self.band(p[1] + d));
        self.bands[b0..=b1].iter().flatten().chain(self.long.iter()).any(close)
    }
}

/// Whether segment a–b comes within `d` of `p`, exactly.
fn dist_at_most(a: P, b: P, p: P, d: i64) -> bool {
    let d2 = (d as i128) * (d as i128);
    let (dx, dy) = ((b[0] - a[0]) as i128, (b[1] - a[1]) as i128);
    let len2 = dx * dx + dy * dy;
    let t = (p[0] - a[0]) as i128 * dx + (p[1] - a[1]) as i128 * dy;
    let to = |q: P| {
        let (x, y) = ((p[0] - q[0]) as i128, (p[1] - q[1]) as i128);
        x * x + y * y
    };
    if len2 == 0 || t <= 0 {
        to(a) <= d2
    } else if t >= len2 {
        to(b) <= d2
    } else {
        let c = cross(a, b, p);
        c * c <= d2 * len2
    }
}

fn cross(o: P, a: P, b: P) -> i128 {
    (a[0] - o[0]) as i128 * (b[1] - o[1]) as i128 - (a[1] - o[1]) as i128 * (b[0] - o[0]) as i128
}

fn on_segment(a: P, b: P, p: P) -> bool {
    cross(a, b, p) == 0
        && p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// Twice a ring's signed area (positive: counter-clockwise).
fn area2(r: &[P]) -> i128 {
    (0..r.len()).map(|i| cross([0, 0], r[i], r[(i + 1) % r.len()])).sum()
}

/// The midpoints (doubled points) of a ring's edges.
fn mids(r: &[P]) -> impl Iterator<Item = P> + '_ {
    (0..r.len()).map(move |k| {
        let (a, b) = (r[k], r[(k + 1) % r.len()]);
        [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2]
    })
}

/// A region's shapes (E7, unclosed, each even–odd) as rings with its inside on their left, or
/// None when two of them overlap (and the caller unions them).
pub fn oriented(shapes: &[Vec<Vec<P>>]) -> Option<Vec<Vec<P>>> {
    // (Points doubled: each edge's midpoint a point on the grid.)
    let doubled: Vec<Vec<Vec<P>>> = shapes
        .iter()
        .map(|s| s.iter().filter(|r| r.len() >= 3).map(|r| r.iter().map(|p| [p[0] * 2, p[1] * 2]).collect()).collect())
        .collect();
    let bands: Vec<Bands> = doubled.iter().map(|s| Bands::of(s)).collect();
    let boxes: Vec<Option<[i64; 4]>> = doubled
        .iter()
        .map(|s| {
            let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
            for p in s.iter().flatten() {
                b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
            }
            (b[0] <= b[2]).then_some(b)
        })
        .collect();
    let mut out = Vec::new();
    for (si, s) in doubled.iter().enumerate() {
        for (ri, r) in s.iter().enumerate() {
            // Overlap: a point or an edge's midpoint inside another shape, farther than the
            // topology's NEAR from its edges (nearer, the topology takes it as on them).
            for (ti, t) in bands.iter().enumerate() {
                let Some(b) = boxes[ti].filter(|_| ti != si) else { continue };
                let meets = |p: &P| p[0] > b[0] && p[0] < b[2] && p[1] > b[1] && p[1] < b[3];
                let near = 2 * fmt::topo::NEAR;
                if r.iter()
                    .copied()
                    .chain(mids(r))
                    .any(|m| meets(&m) && t.parity(m, None) == Some(true) && !t.within(m, near))
                {
                    return None;
                }
            }
            // Its nesting in its own shape, from a midpoint on no other ring.
            let hole = mids(r).find_map(|m| bands[si].parity(m, Some(ri as u32))).unwrap_or(false);
            let ccw = match area2(r).cmp(&0) {
                Ordering::Equal => continue,
                o => o == Ordering::Greater,
            };
            let mut ring: Vec<P> = r.iter().map(|p| [p[0] / 2, p[1] / 2]).collect();
            if ccw == hole {
                ring.reverse();
            }
            out.push(ring);
        }
    }
    Some(out)
}
