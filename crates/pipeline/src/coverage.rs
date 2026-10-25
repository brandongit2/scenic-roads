//! Coverage (docs/regions.md): the union of the regions' outlines. A feature is built if it touches
//! the coverage (for a way: any vertex inside, as Geofabrik does).
//!
//! Each outline becomes a [`Shape`]: its rings, a buffer (1 km for `osm:` outlines, so piers and
//! coastal roads on a boundary that follows the coastline are in), and a cell grid over its bounding
//! box. Every cell knows whether its centre is inside and which edges cross it (or pass within the
//! buffer), so a point test reads one cell: the centre's state, flipped by each of the cell's edges
//! crossing the segment from the centre to the point, then the buffer's distance to the cell's
//! edges. Building a grid costs one pass over the edges plus a scanline per row.
//!
//! A catalog records the coverage it was built for, simplified for drawing ([`DrawnRegion`]), so
//! the map draws it from the catalog rather than from the recipes.

#[cfg(test)]
use crate::agent::recipes::Recipe;
use crate::outlines::inside;
#[cfg(test)]
use crate::outlines::Outlines;
use anyhow::{bail, Context, Result};
use det::Det;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(test)]
use std::path::Path;

/// Metres per E7 unit of latitude (and of longitude at the equator).
const M_PER_E7: f64 = 111_320.0 * 1e-7;

/// One outline of the coverage, indexed for point tests.
#[derive(Clone)]
pub struct Shape {
    /// The region and outline entry it came from ("borders: osm:1877178").
    pub source: String,
    /// Shared by its clones, as `grid` and `memo` are: a region's coverage holds the whole
    /// coverage's shapes, so the outlines (GBs for the world's) aren't held twice (#619).
    pub rings: std::sync::Arc<Vec<Vec<[i32; 2]>>>,
    pub buffer_m: f64,
    /// Bounding box grown by the buffer (E7).
    pub bbox: [i32; 4],
    /// The ISO 3166-1 code of the country or territory it's of ("" when unknown: an outline that
    /// isn't the pass's), as `Outlines::country_code` has it: what the 3D buildings' fill fits by.
    pub country: String,
    /// The water of a region of land and water (#468): held like any shape (`contains`, `touches`,
    /// `shape_at`, `fingerprint`: a bridge, a pier, a ferry's ways at sea stay the region's), but
    /// no target (`Coverage::meets_rect`, `meets_box`): only the depth's (`meets_rect_depth`).
    pub water: bool,
    grid: std::sync::Arc<Grid>,
    /// What's been worked out of it (`Memo`), shared by its clones (a region's coverage holds the
    /// whole coverage's shapes): its geometry is fixed once made.
    memo: std::sync::Arc<std::sync::Mutex<Memo>>,
}

/// A shape's answers kept, each a function of its geometry (fixed once made) and the question: its
/// fingerprints by box (`Shape::fingerprint`), and whether a way's points (`Shape::touches_way`, by
/// the points' digest) meet it. The plan asks the same of every unit on every loop (the coverage
/// kept between loops while its recipes and outlines don't change): worked out once, a plan took
/// seconds rather than minutes (2026-10-15). What every shape's memo holds together is capped
/// (`MEMO_CAP`): past it, every memo starts again (`Budget`).
struct Memo {
    fingerprints: std::collections::HashMap<[i32; 4], Option<String>>,
    ways: std::collections::HashMap<[u8; 32], bool>,
    /// The budget it counts toward (`MEMOS`), the generation its answers are of, and the bytes they
    /// count.
    budget: &'static Budget,
    gen: u64,
    bytes: usize,
}

impl Default for Memo {
    fn default() -> Memo {
        Memo {
            fingerprints: Default::default(),
            ways: Default::default(),
            budget: &MEMOS,
            gen: MEMOS.gen.load(std::sync::atomic::Ordering::SeqCst),
            bytes: 0,
        }
    }
}

/// The bytes all shapes' memos hold at most, together: past it, every memo is emptied (each as it
/// next answers). Measured on the build's coverage (2026-10-15: 122 shapes, a plan, the checklist
/// and the regions' progress, worked out twice): 13 MB (`memo_bytes`, the bench in
/// crate::agent::build's tests); this leaves room for a coverage about five times as big before
/// the memos start again, and is little to a Mac.
pub const MEMO_CAP: usize = 64 << 20;

/// What memos hold together, against a cap: the bytes counted, and the generation (begun again
/// past the cap: every memo of an earlier one is stale).
struct Budget {
    cap: usize,
    bytes: std::sync::atomic::AtomicUsize,
    gen: std::sync::atomic::AtomicU64,
}

static MEMOS: Budget =
    Budget { cap: MEMO_CAP, bytes: std::sync::atomic::AtomicUsize::new(0), gen: std::sync::atomic::AtomicU64::new(0) };

/// The bytes the shapes' memos hold now (of the current generation), about.
pub fn memo_bytes() -> usize {
    MEMOS.bytes.load(std::sync::atomic::Ordering::Relaxed)
}

impl Memo {
    /// Ready for an answer of about `bytes`: emptied when of an earlier generation; counted, and past
    /// the cap a new generation begun (every memo emptied as it next answers, this one at once).
    fn take(&mut self, bytes: usize) {
        use std::sync::atomic::Ordering::SeqCst;
        let b = self.budget;
        if !self.current() {
            (self.fingerprints, self.ways, self.bytes, self.gen) =
                (Default::default(), Default::default(), 0, b.gen.load(SeqCst));
        }
        if b.bytes.fetch_add(bytes, SeqCst) + bytes > b.cap {
            b.gen.fetch_add(1, SeqCst);
            b.bytes.store(0, SeqCst);
            (self.fingerprints, self.ways, self.bytes, self.gen) =
                (Default::default(), Default::default(), 0, b.gen.load(SeqCst));
            b.bytes.fetch_add(bytes, SeqCst);
        }
        self.bytes += bytes;
    }

    /// Whether its answers are of the current generation (an earlier one's are stale: none).
    fn current(&self) -> bool {
        self.gen == self.budget.gen.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for Memo {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::SeqCst;
        if self.current() {
            let b = &self.budget.bytes;
            b.fetch_sub(self.bytes.min(b.load(SeqCst)), SeqCst);
        }
    }
}

#[derive(Clone)]
struct Grid {
    x0: f64,
    y0: f64,
    /// Cell size (E7 units).
    cw: f64,
    ch: f64,
    nx: usize,
    ny: usize,
    /// Per cell: the centre inside?
    centre: Vec<bool>,
    /// Per cell: edges (ring, index of first point) crossing it or within the buffer of it.
    edges: Vec<Vec<(u32, u32)>>,
}

impl Shape {
    pub fn new(source: String, rings: Vec<Vec<[i32; 2]>>, buffer_m: f64) -> Shape {
        let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for r in &rings {
            for p in r {
                bb = [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])];
            }
        }
        // The buffer in E7 units: latitude at the box's widest point.
        let lat = (bb[1].unsigned_abs().max(bb[3].unsigned_abs()) as f64 * 1e-7).min(85.0);
        let bx = (buffer_m / (M_PER_E7 * lat.to_radians().dcos())).ceil() as i32;
        let by = (buffer_m / M_PER_E7).ceil() as i32;
        let bbox =
            [bb[0].saturating_sub(bx), bb[1].saturating_sub(by), bb[2].saturating_add(bx), bb[3].saturating_add(by)];
        let grid = Grid::build(&rings, bbox, bx.max(by) as f64);
        Shape {
            source,
            rings: std::sync::Arc::new(rings),
            buffer_m,
            bbox,
            country: String::new(),
            water: false,
            grid: std::sync::Arc::new(grid),
            memo: Default::default(),
        }
    }

    /// The grid cell of `p`, which must be inside the box.
    fn cell(&self, p: [i32; 2]) -> usize {
        let g = &self.grid;
        let cx = (((p[0] as f64 - g.x0) / g.cw) as usize).min(g.nx - 1);
        let cy = (((p[1] as f64 - g.y0) / g.ch) as usize).min(g.ny - 1);
        cy * g.nx + cx
    }

    /// Whether `p` (E7) is inside the rings (even–odd), the buffer aside.
    pub fn inside(&self, p: [i32; 2]) -> bool {
        if p[0] < self.bbox[0] || p[0] > self.bbox[2] || p[1] < self.bbox[1] || p[1] > self.bbox[3] {
            return false;
        }
        let g = &self.grid;
        let c = self.cell(p);
        let (cx, cy) = (c % g.nx, c / g.nx);
        let centre = [(g.x0 + (cx as f64 + 0.5) * g.cw) as i64, (g.y0 + (cy as f64 + 0.5) * g.ch) as i64];
        let pp = [p[0] as i64, p[1] as i64];
        let mut ins = g.centre[c];
        for &(r, i) in &g.edges[c] {
            let ring = &self.rings[r as usize];
            let (a, b) = (ring[i as usize], ring[(i as usize + 1) % ring.len()]);
            if segments_cross(centre, pp, [a[0] as i64, a[1] as i64], [b[0] as i64, b[1] as i64]) {
                ins = !ins;
            }
        }
        ins
    }

    /// Whether `p` (E7) is inside, or within the buffer of the boundary.
    pub fn contains(&self, p: [i32; 2]) -> bool {
        self.inside(p) || self.near_edge(p)
    }

    /// Whether any of a way's points (`points`, lying in the box `b`, w, s, e, n E7) is in the shape
    /// (`contains`), the points known by their digest `key` (the same points, the same digest): kept
    /// (`Memo`).
    pub fn touches_way<I: Iterator<Item = [i32; 2]>>(
        &self,
        key: &[u8; 32],
        b: [i32; 4],
        points: impl FnOnce() -> I,
    ) -> bool {
        if b[0] > self.bbox[2] || b[2] < self.bbox[0] || b[1] > self.bbox[3] || b[3] < self.bbox[1] {
            return false;
        }
        {
            let m = self.memo.lock().unwrap();
            if let Some(&t) = m.ways.get(key).filter(|_| m.current()) {
                return t;
            }
        }
        let t = points().any(|p| self.contains(p));
        let mut m = self.memo.lock().unwrap();
        m.take(32 + 1 + 16);
        m.ways.insert(*key, t);
        t
    }

    /// Whether `p` (E7) is within the buffer of the boundary.
    fn near_edge(&self, p: [i32; 2]) -> bool {
        if self.buffer_m <= 0.0
            || p[0] < self.bbox[0]
            || p[0] > self.bbox[2]
            || p[1] < self.bbox[1]
            || p[1] > self.bbox[3]
        {
            return false;
        }
        let (g, c) = (&self.grid, self.cell(p));
        let kx = M_PER_E7 * (p[1] as f64 * 1e-7).to_radians().dcos();
        let lim = self.buffer_m * self.buffer_m;
        g.edges[c].iter().any(|&(r, i)| {
            let ring = &self.rings[r as usize];
            let (a, b) = (ring[i as usize], ring[(i as usize + 1) % ring.len()]);
            dist2_m(p, a, b, kx) <= lim
        })
    }

    /// Whether the box w, s, e, n (E7, edges included) meets the shape or its buffer (the buffer
    /// as a box around the box: up to √2 × the buffer at its corners). Exact otherwise: an edge
    /// meeting the box, else the box wholly inside or wholly outside, which one point tells.
    pub fn meets_rect(&self, r: [i32; 4]) -> bool {
        if r[0] > self.bbox[2] || r[2] < self.bbox[0] || r[1] > self.bbox[3] || r[3] < self.bbox[1] {
            return false;
        }
        if self.edges_meeting(self.grown(r), &mut |_, _, _| true) {
            return true;
        }
        // No edge meets it: the box is all inside or all outside.
        self.contains([((r[0] as i64 + r[2] as i64) / 2) as i32, ((r[1] as i64 + r[3] as i64) / 2) as i32])
    }

    /// Whether the box w, s, e, n (E7, edges included) meets the rings themselves (the buffer
    /// aside): an edge meets it, else its centre is inside.
    pub fn meets_rect_inside(&self, r: [i32; 4]) -> bool {
        if r[0] > self.bbox[2] || r[2] < self.bbox[0] || r[1] > self.bbox[3] || r[3] < self.bbox[1] {
            return false;
        }
        self.edges_meeting([r[0] as i64, r[1] as i64, r[2] as i64, r[3] as i64], &mut |_, _, _| true)
            || self.inside([((r[0] as i64 + r[2] as i64) / 2) as i32, ((r[1] as i64 + r[3] as i64) / 2) as i32])
    }

    /// Whether every point of the box w, s, e, n (E7) is inside the rings (the buffer aside): no
    /// edge comes within the buffer of it, and its centre is inside.
    pub fn covers_rect(&self, r: [i32; 4]) -> bool {
        if r[0] < self.bbox[0] || r[2] > self.bbox[2] || r[1] < self.bbox[1] || r[3] > self.bbox[3] {
            return false;
        }
        !self.edges_meeting(self.grown(r), &mut |_, _, _| true)
            && self.inside([((r[0] as i64 + r[2] as i64) / 2) as i32, ((r[1] as i64 + r[3] as i64) / 2) as i32])
    }

    /// The box w, s, e, n (E7) grown by the buffer, at its latitude furthest from the equator.
    fn grown(&self, r: [i32; 4]) -> [i64; 4] {
        let lat = (r[1].unsigned_abs().max(r[3].unsigned_abs()) as f64 * 1e-7).min(85.0);
        let (bx, by) = if self.buffer_m > 0.0 {
            (
                (self.buffer_m / (M_PER_E7 * lat.to_radians().dcos())).ceil() as i64,
                (self.buffer_m / M_PER_E7).ceil() as i64,
            )
        } else {
            (0, 0)
        };
        [r[0] as i64 - bx, r[1] as i64 - by, r[2] as i64 + bx, r[3] as i64 + by]
    }

    /// Calls `f` with each ring edge meeting the box `rg`, with its place (ring, index), from the
    /// grid cells the box overlaps (so an edge crossing several cells comes several times), until it
    /// returns true; whether one did.
    fn edges_meeting(&self, rg: [i64; 4], f: &mut dyn FnMut((u32, u32), [i32; 2], [i32; 2]) -> bool) -> bool {
        let g = &self.grid;
        let cell = |v: f64, v0: f64, size: f64, n: usize| (((v - v0) / size).floor().max(0.0) as usize).min(n - 1);
        let (cx0, cx1) = (cell(rg[0] as f64, g.x0, g.cw, g.nx), cell(rg[2] as f64, g.x0, g.cw, g.nx));
        let (cy0, cy1) = (cell(rg[1] as f64, g.y0, g.ch, g.ny), cell(rg[3] as f64, g.y0, g.ch, g.ny));
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                for &(ri, i) in &g.edges[cy * g.nx + cx] {
                    let ring = &self.rings[ri as usize];
                    let (a, b) = (ring[i as usize], ring[(i as usize + 1) % ring.len()]);
                    if segment_meets_box([a[0] as i64, a[1] as i64], [b[0] as i64, b[1] as i64], rg) && f((ri, i), a, b)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The shape as it is inside the box w, s, e, n (E7): whether a reference point of the box (its
    /// south-west corner, or the first point up its diagonal on no edge) is inside the rings, and
    /// every ring edge meeting the box grown by the buffer, each as often as the rings hold it. That
    /// decides which points of the box the shape contains: a point's inside-ness is the reference
    /// point's, flipped by the edges crossing the line between them (inside the box), and its buffer
    /// reads the edges within the buffer of it. None when the shape has nothing in the box.
    fn fingerprint(&self, r: [i32; 4]) -> Option<String> {
        {
            let m = self.memo.lock().unwrap();
            if let Some(f) = m.fingerprints.get(&r).filter(|_| m.current()) {
                return f.clone();
            }
        }
        let f = self.fingerprint_made(r);
        let mut m = self.memo.lock().unwrap();
        m.take(16 + 24 + f.as_ref().map_or(0, String::capacity) + 16);
        m.fingerprints.insert(r, f.clone());
        f
    }

    /// `fingerprint`, worked out.
    fn fingerprint_made(&self, r: [i32; 4]) -> Option<String> {
        let rg = self.grown(r);
        if rg[0] > self.bbox[2] as i64
            || rg[2] < self.bbox[0] as i64
            || rg[1] > self.bbox[3] as i64
            || rg[3] < self.bbox[1] as i64
        {
            return None;
        }
        // Each edge once per place in the rings (a cell grid lists an edge in every cell it crosses);
        // an edge the rings hold twice stays twice, since under even–odd the two cancel.
        let mut by_place: std::collections::BTreeMap<(u32, u32), [i32; 4]> = std::collections::BTreeMap::new();
        self.edges_meeting(rg, &mut |at, a, b| {
            // (Direction doesn't matter to which points are inside.)
            by_place.insert(
                at,
                if (a[0], a[1]) <= (b[0], b[1]) { [a[0], a[1], b[0], b[1]] } else { [b[0], b[1], a[0], a[1]] },
            );
            false
        });
        let mut edges: Vec<[i32; 4]> = by_place.into_values().collect();
        edges.sort_unstable();
        // A reference point on no edge, so its inside-ness doesn't depend on how the grid's cells
        // fall (on an edge, either answer would be right, and the cell decides).
        let on_edge = |p: [i32; 2]| {
            edges.iter().any(|e| {
                let (a, b, p) = ([e[0] as i64, e[1] as i64], [e[2] as i64, e[3] as i64], [p[0] as i64, p[1] as i64]);
                (b[0] - a[0]) as i128 * (p[1] - a[1]) as i128 == (b[1] - a[1]) as i128 * (p[0] - a[0]) as i128
                    && p[0] >= a[0].min(b[0])
                    && p[0] <= a[0].max(b[0])
                    && p[1] >= a[1].min(b[1])
                    && p[1] <= a[1].max(b[1])
            })
        };
        let mut reference = [r[0], r[1]];
        while on_edge(reference) && reference[0] < r[2] && reference[1] < r[3] {
            reference = [reference[0] + 1, reference[1] + 1];
        }
        let inside = self.inside(reference);
        if edges.is_empty() && !inside {
            return None;
        }
        Some(format!("{}:{}:{}", self.buffer_m, inside as u8, store::naming::hash16(bytemuck::cast_slice(&edges))))
    }
}

/// Whether segment a–b meets the box w, s, e, n (edges included).
fn segment_meets_box(a: [i64; 2], b: [i64; 2], r: [i64; 4]) -> bool {
    let inside = |p: [i64; 2]| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3];
    if inside(a) || inside(b) {
        return true;
    }
    if a[0].max(b[0]) < r[0] || a[0].min(b[0]) > r[2] || a[1].max(b[1]) < r[1] || a[1].min(b[1]) > r[3] {
        return false;
    }
    // Both ends outside, boxes overlapping: it meets the box when the box's corners aren't all on
    // one side of its line.
    let side = |p: [i64; 2]| {
        ((b[0] - a[0]) as i128 * (p[1] - a[1]) as i128 - (b[1] - a[1]) as i128 * (p[0] - a[0]) as i128).signum()
    };
    let s: Vec<i128> = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]].iter().map(|&p| side(p)).collect();
    !(s.iter().all(|&v| v > 0) || s.iter().all(|&v| v < 0))
}

/// Squared distance (m²) from `p` to the segment a–b, on a local projection (`kx`: metres per E7
/// unit of longitude here).
fn dist2_m(p: [i32; 2], a: [i32; 2], b: [i32; 2], kx: f64) -> f64 {
    let (px, py) = (p[0] as f64 * kx, p[1] as f64 * M_PER_E7);
    let (ax, ay) = (a[0] as f64 * kx, a[1] as f64 * M_PER_E7);
    let (bx, by) = (b[0] as f64 * kx, b[1] as f64 * M_PER_E7);
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) };
    (px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)
}

/// Whether the segment p1–p2 (from a grid cell's centre to a point) crosses the edge q1–q2, for the
/// even–odd test. An end of the edge lying on the segment's line counts as being on its left, a
/// fixed side whatever the segment's direction: a ring passing through a vertex on the line is
/// counted once, and one touching it not at all. (A point exactly on an edge is on the boundary,
/// where either answer is right.)
fn segments_cross(p1: [i64; 2], p2: [i64; 2], q1: [i64; 2], q2: [i64; 2]) -> bool {
    let o = |a: [i64; 2], b: [i64; 2], c: [i64; 2]| {
        ((b[0] - a[0]) as i128 * (c[1] - a[1]) as i128 - (b[1] - a[1]) as i128 * (c[0] - a[0]) as i128).signum()
    };
    let side = |q: [i64; 2]| if o(p1, p2, q) < 0 { -1 } else { 1 };
    if side(q1) == side(q2) {
        return false;
    }
    let (d1, d2) = (o(q1, q2, p1), o(q1, q2, p2));
    d1 != 0 && d2 != 0 && d1 != d2
}

impl Grid {
    fn build(rings: &[Vec<[i32; 2]>], bbox: [i32; 4], buf_e7: f64) -> Grid {
        // About 1,024 cells along the longer side, never smaller than twice the buffer.
        let (w, h) = ((bbox[2] as f64 - bbox[0] as f64).max(1.0), (bbox[3] as f64 - bbox[1] as f64).max(1.0));
        let cell = (w.max(h) / 1024.0).max(2.0 * buf_e7).max(100.0);
        let (nx, ny) = (((w / cell).ceil() as usize).clamp(1, 4096), ((h / cell).ceil() as usize).clamp(1, 4096));
        let (cw, ch) = (w / nx as f64, h / ny as f64);
        let (x0, y0) = (bbox[0] as f64, bbox[1] as f64);
        let mut edges: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nx * ny];
        for (ri, r) in rings.iter().enumerate() {
            for i in 0..r.len() {
                let (a, b) = (r[i], r[(i + 1) % r.len()]);
                if a == b {
                    continue;
                }
                // Every cell the edge's box (grown by the buffer) touches: a superset, fine for the
                // crossing test and needed for the buffer.
                let cx0 = ((((a[0].min(b[0]) as f64 - buf_e7) - x0) / cw).floor().max(0.0) as usize).min(nx - 1);
                let cx1 = ((((a[0].max(b[0]) as f64 + buf_e7) - x0) / cw).floor().max(0.0) as usize).min(nx - 1);
                let cy0 = ((((a[1].min(b[1]) as f64 - buf_e7) - y0) / ch).floor().max(0.0) as usize).min(ny - 1);
                let cy1 = ((((a[1].max(b[1]) as f64 + buf_e7) - y0) / ch).floor().max(0.0) as usize).min(ny - 1);
                for cy in cy0..=cy1 {
                    for cx in cx0..=cx1 {
                        edges[cy * nx + cx].push((ri as u32, i as u32));
                    }
                }
            }
        }
        // Each row's centres: crossings of the row's centre line, sorted, give the parity.
        let mut centre = vec![false; nx * ny];
        let mut xs: Vec<f64> = Vec::new();
        for cy in 0..ny {
            let y = y0 + (cy as f64 + 0.5) * ch;
            xs.clear();
            for r in rings {
                for i in 0..r.len() {
                    let (a, b) = (r[i], r[(i + 1) % r.len()]);
                    let (ay, by) = (a[1] as f64, b[1] as f64);
                    if (ay > y) != (by > y) {
                        xs.push(a[0] as f64 + (y - ay) * (b[0] as f64 - a[0] as f64) / (by - ay));
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            let mut k = 0;
            for cx in 0..nx {
                let x = x0 + (cx as f64 + 0.5) * cw;
                while k < xs.len() && xs[k] < x {
                    k += 1;
                }
                centre[cy * nx + cx] = k % 2 == 1;
            }
        }
        Grid { x0, y0, cw, ch, nx, ny, centre, edges }
    }
}

/// The coverage: every region's outlines.
#[derive(Clone, Default)]
pub struct Coverage {
    /// Every region's shapes, the water of regions of land and water among them (`Shape::water`).
    pub shapes: Vec<Shape>,
    /// The regions with no land (sea zones, the high seas: docs/inputs.md §6.1.3): only the depth's
    /// targets read them (`meets_rect_depth`); every other step's coverage is `shapes` alone.
    pub depth_only: Vec<Shape>,
}

impl Coverage {
    /// The memory a step's process takes to read this coverage (MB; `coverage_of` in scenic-build,
    /// through `Coverage::accepted`), worked out from the coverage itself: every shape's rings (8 B a
    /// point), its grid (`Grid`: per cell a bool and an edge list's 24 B header, 8 B an entry), the
    /// kept bytes read whole while the shapes are made again from them (`from_kept`; or written,
    /// `kept_bytes`, when the cache misses), and the memos' cap (`MEMO_CAP`). A job's memory bound
    /// adds it to what its step holds besides (the agent's `guess` and `peak`, task #639).
    pub fn read_peak_mb(&self) -> u64 {
        let mut bytes = MEMO_CAP as u64 + KEPT_MAGIC.len() as u64;
        for list in [&self.shapes, &self.depth_only] {
            bytes += 8;
            for sh in list {
                let points: u64 = sh.rings.iter().map(|r| r.len() as u64).sum();
                let g = &sh.grid;
                let entries: u64 = g.edges.iter().map(|e| e.capacity() as u64).sum();
                let rings = points * 8 + sh.rings.len() as u64 * 24;
                let grid = (g.nx * g.ny) as u64 * 25 + entries * 8;
                let kept = 8
                    + sh.source.len() as u64
                    + 8
                    + sh.country.len() as u64
                    + 8
                    + 1
                    + 8
                    + sh.rings.len() as u64 * 8
                    + points * 8;
                bytes += rings + grid + kept;
            }
        }
        bytes.div_ceil(1 << 20)
    }
}

/// The buffer around `osm:` outlines (coasts), metres.
pub const OSM_BUFFER_M: f64 = 1000.0;

/// Reads a `.poly` file (Osmosis polygon format): rings, with `!`-prefixed sections as holes.
pub fn read_poly(text: &str) -> Result<Vec<Vec<[i32; 2]>>> {
    let mut rings = Vec::new();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.next().context("empty .poly")?; // its name
    while let Some(l) = lines.next() {
        if l == "END" {
            break;
        }
        let mut ring = Vec::new();
        for l in lines.by_ref() {
            if l == "END" {
                break;
            }
            let v: Vec<f64> = l
                .split_whitespace()
                .map(|x| x.parse())
                .collect::<Result<_, _>>()
                .with_context(|| format!("bad .poly line {l:?}"))?;
            if v.len() < 2 {
                bail!("bad .poly line {l:?}");
            }
            ring.push([(v[0] * 1e7).round() as i32, (v[1] * 1e7).round() as i32]);
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    Ok(rings)
}

/// A fixture's circle (`Coverage::fixture`, `inputs::regions::accept_fixture`): the 64 points the
/// old `place:` parts had, `km` round `lon`, `lat`.
pub fn fixture_circle(lon: f64, lat: f64, km: f64) -> Vec<[i32; 2]> {
    let dy = km * 1000.0 / 110_574.0;
    let dx = km * 1000.0 / (111_320.0 * lat.to_radians().dcos().max(0.01));
    (0..64)
        .map(|i| {
            let t = i as f64 / 64.0 * std::f64::consts::TAU;
            [
                ((lon + dx * t.dcos()) * 1e7).round() as i32,
                ((lat + dy * t.dsin()).clamp(-89.9, 89.9) * 1e7).round() as i32,
            ]
        })
        .collect()
}

/// The environment variable the agent names its jobs' cache folder in (`<home>/cache`), where
/// `Coverage::accepted` keeps the coverage it reads.
pub const CACHE_ENV: &str = "SCENIC_CACHE";

const KEPT_MAGIC: &[u8; 8] = b"RDCOV004";

/// The coverage's shapes as kept (`Coverage::accepted`): their source, country, buffer, whether
/// water, and rings.
pub(crate) fn kept_bytes(c: &Coverage) -> Vec<u8> {
    let mut b = KEPT_MAGIC.to_vec();
    let s = |b: &mut Vec<u8>, t: &str| {
        b.extend_from_slice(&(t.len() as u64).to_le_bytes());
        b.extend_from_slice(t.as_bytes());
    };
    for list in [&c.shapes, &c.depth_only] {
        b.extend_from_slice(&(list.len() as u64).to_le_bytes());
        for sh in list {
            s(&mut b, &sh.source);
            s(&mut b, &sh.country);
            b.extend_from_slice(&sh.buffer_m.to_le_bytes());
            b.push(sh.water as u8);
            b.extend_from_slice(&(sh.rings.len() as u64).to_le_bytes());
            for r in sh.rings.iter() {
                b.extend_from_slice(&(r.len() as u64).to_le_bytes());
                b.extend_from_slice(bytemuck::cast_slice(r));
            }
        }
    }
    b
}

/// `kept_bytes` read back: the shapes made again from their rings (`Shape::new`, as
/// `Coverage::accepted` makes them); None when they don't read.
pub(crate) fn from_kept(b: &[u8]) -> Option<Coverage> {
    struct Rd<'a>(&'a [u8]);
    impl<'a> Rd<'a> {
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            if n > self.0.len() {
                return None;
            }
            let (a, rest) = self.0.split_at(n);
            self.0 = rest;
            Some(a)
        }
        fn n(&mut self) -> Option<usize> {
            usize::try_from(u64::from_le_bytes(self.take(8)?.try_into().ok()?)).ok()
        }
        fn text(&mut self) -> Option<String> {
            let n = self.n()?;
            String::from_utf8(self.take(n)?.to_vec()).ok()
        }
    }
    let mut r = Rd(b);
    if r.take(8)? != KEPT_MAGIC {
        return None;
    }
    let mut lists = [Vec::new(), Vec::new()];
    for shapes in lists.iter_mut() {
        for _ in 0..r.n()? {
            let source = r.text()?;
            let country = r.text()?;
            let buffer_m = f64::from_le_bytes(r.take(8)?.try_into().ok()?);
            let water = r.take(1)?[0] != 0;
            let mut rings = Vec::new();
            for _ in 0..r.n()? {
                let n = r.n()?;
                rings.push(bytemuck::pod_collect_to_vec::<u8, [i32; 2]>(r.take(n.checked_mul(8)?)?));
            }
            let mut sh = Shape::new(source, rings, buffer_m);
            sh.country = country;
            sh.water = water;
            shapes.push(sh);
        }
    }
    let [shapes, depth_only] = lists;
    r.0.is_empty().then_some(Coverage { shapes, depth_only })
}

impl Coverage {
    /// A coverage for the tests, from fixtures as regions name them (`<id>: <entry>`): `place:<lon>,
    /// <lat>,<km>`, a circle of the 64 points the old `place:` parts had, and `poly:<file>`, a
    /// `.poly` in `dir` (a square, mostly). Never the app's: its regions are the accepted version's
    /// (`Coverage::accepted`).
    #[cfg(test)]
    pub fn fixture(recipes: &[Recipe], _outlines: Option<&Outlines>, dir: &Path) -> Result<Coverage> {
        let mut shapes = Vec::new();
        for r in recipes {
            for entry in &r.outline {
                let rings = if let Some(v) = entry.strip_prefix("place:") {
                    let n: Vec<f64> = v
                        .split(',')
                        .map(|x| x.trim().parse::<f64>())
                        .collect::<Result<_, _>>()
                        .with_context(|| entry.clone())?;
                    vec![fixture_circle(n[0], n[1], n[2])]
                } else if let Some(f) = entry.strip_prefix("poly:") {
                    read_poly(&std::fs::read_to_string(dir.join(f)).with_context(|| f.to_string())?)?
                } else {
                    bail!("{}: {entry}: a fixture is place: or poly:", r.id)
                };
                shapes.push(Shape::new(format!("{}: {entry}", r.id), rings, 0.0));
            }
        }
        Ok(Coverage { shapes, ..Default::default() })
    }

    /// Each region's own coverage (its shapes, as `Coverage::accepted` made them for it: "<id>: <from>"),
    /// in the order its first shape comes: no outline read again.
    pub fn by_region(&self) -> Vec<(String, Coverage)> {
        let mut out: Vec<(String, Coverage)> = Vec::new();
        for s in &self.shapes {
            let id = s.source.split_once(": ").map_or(s.source.as_str(), |(id, _)| id);
            match out.iter_mut().find(|(r, _)| r == id) {
                Some((_, c)) => c.shapes.push(s.clone()),
                None => out.push((id.to_string(), Coverage { shapes: vec![s.clone()], ..Default::default() })),
            }
        }
        out
    }

    pub fn contains(&self, p: [i32; 2]) -> bool {
        self.shapes.iter().any(|s| s.contains(p))
    }

    /// The shape a point is of: the first (in the recipes' order) whose outline holds it, else the
    /// first whose buffer reaches it. A town at a border is its own region's, not a neighbour's
    /// whose 1 km buffer reaches over (Monaco isn't France's, Windsor isn't Michigan's); where
    /// outlines overlap, the first's.
    pub fn shape_at(&self, p: [i32; 2]) -> Option<usize> {
        let mut buffered = None;
        for (i, s) in self.shapes.iter().enumerate() {
            if s.inside(p) {
                return Some(i);
            }
            if buffered.is_none() && s.near_edge(p) {
                buffered = Some(i);
            }
        }
        buffered
    }

    /// The shape `shape_at` gives every point of the box w, s, e, n (E7), when it's one: a shape
    /// whose outline covers the whole box (`Shape::covers_rect`) when no outline before it meets
    /// the box. None otherwise (the points then each asked).
    pub fn box_shape(&self, b: [i32; 4]) -> Option<usize> {
        for (i, s) in self.shapes.iter().enumerate() {
            if s.covers_rect(b) {
                return Some(i);
            }
            if s.meets_rect_inside(b) {
                return None;
            }
        }
        None
    }

    /// The shapes meeting the box w, s, e, n (E7) in the recipes' order, each as its fingerprint
    /// there (`Shape::fingerprint`) and its country: what decides a point's shape (`shape_at`, by
    /// the order) and its country, where `fingerprint` (sorted, by geometry alone) misses a change
    /// of order. The 3D buildings' tiles are keyed by it (docs/buildings3d.md §3.2).
    pub fn shapes_key(&self, b: [i32; 4]) -> String {
        let v: Vec<String> = self
            .shapes
            .iter()
            .filter_map(|s| s.fingerprint(b).map(|f| format!("{}{f}:{}", if s.water { "w" } else { "" }, s.country)))
            .collect();
        v.join(",")
    }

    /// Whether any shape's (buffered) box meets the box w, s, e, n (E7): a quick filter. (Its
    /// water aside, as `meets_rect`.)
    pub fn meets_box(&self, b: [i32; 4]) -> bool {
        self.shapes
            .iter()
            .filter(|s| !s.water)
            .any(|s| s.bbox[0] <= b[2] && s.bbox[2] >= b[0] && s.bbox[1] <= b[3] && s.bbox[3] >= b[1])
    }

    /// Whether the box w, s, e, n (E7) meets the coverage itself (`Shape::meets_rect`): its land,
    /// the regions' water aside (`Shape::water`), so a tile only water meets is no step's target
    /// but the depth's.
    pub fn meets_rect(&self, b: [i32; 4]) -> bool {
        self.shapes.iter().filter(|s| !s.water).any(|s| s.meets_rect(b))
    }

    /// `meets_rect` with the water too, the regions' and the regions with no land: the depth's
    /// targets.
    pub fn meets_rect_depth(&self, b: [i32; 4]) -> bool {
        self.shapes.iter().chain(&self.depth_only).any(|s| s.meets_rect(b))
    }

    /// The coverage as it is inside the box w, s, e, n (E7): its shapes' fingerprints there
    /// (`Shape::fingerprint`), each once, by geometry alone (not which region a shape came from).
    /// Equal fingerprints mean the same points of the box are covered, so a job keyed on the box it
    /// reads reruns only when the coverage changes there.
    pub fn fingerprint(&self, b: [i32; 4]) -> String {
        // (A region's water marked: which of the covered points are targets is the key's too.)
        let mut v: Vec<String> = self
            .shapes
            .iter()
            .filter_map(|s| s.fingerprint(b).map(|f| if s.water { format!("w{f}") } else { f }))
            .collect();
        v.sort();
        v.dedup();
        v.join(",")
    }

    /// Whether a way (its vertices) touches the coverage.
    pub fn touches(&self, verts: &[[i32; 2]]) -> bool {
        verts.iter().any(|&p| self.contains(p))
    }
}

// ---- the coverage a catalog records ------------------------------------------------------------

/// A region as a catalog records it (docs/formats.md, Catalog `coverage`): its recipe when the
/// catalog was made, and each outline entry's polygons simplified for drawing, as GeoJSON
/// MultiPolygon coordinates (degrees to 5 decimals, about a metre; rings closed).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrawnRegion {
    pub id: String,
    pub name: String,
    pub outline: Vec<String>,
    /// By outline entry; an entry that couldn't be read then has none.
    #[serde(default)]
    pub shapes: BTreeMap<String, Vec<Vec<Vec<[f64; 2]>>>>,
    /// A region still building (not recorded built yet): its units (z6 tiles) built of those it
    /// builds, by the records when the catalog was made; on the map, and downloadable, as they come.
    /// None for a built region, and in catalogs made before this was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiles: Option<Tiles>,
}

/// A region's tiles built of its total (`DrawnRegion::tiles`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tiles {
    pub built: u32,
    pub total: u32,
}

impl DrawnRegion {
    /// Whether its outlines meet the box w, s, e, n (E7, edges included): an edge meeting the box,
    /// else the box wholly inside an outline (even–odd), which its centre tells.
    pub fn meets_rect(&self, r: [i32; 4]) -> bool {
        let rb = [r[0] as i64, r[1] as i64, r[2] as i64, r[3] as i64];
        let (cx, cy) = ((rb[0] + rb[2]) as f64 / 2.0, (rb[1] + rb[3]) as f64 / 2.0);
        let e7 = |p: [f64; 2]| [(p[0] * 1e7).round() as i64, (p[1] * 1e7).round() as i64];
        self.shapes.values().any(|polygons| {
            let mut inside = false;
            for ring in polygons.iter().flatten() {
                for i in 0..ring.len() {
                    let (a, b) = (e7(ring[i]), e7(ring[(i + 1) % ring.len()]));
                    if segment_meets_box(a, b, rb) {
                        return true;
                    }
                    let (ay, by) = (a[1] as f64, b[1] as f64);
                    if (ay > cy) != (by > cy) && cx < (b[0] - a[0]) as f64 * (cy - ay) / (by - ay) + a[0] as f64 {
                        inside = !inside;
                    }
                }
            }
            inside
        })
    }
}

/// How far a drawn outline may stray from its file's: about a pixel when the whole outline fills a
/// screen, within the pass's own range for the panel (1 km for countries, 60 m for the smallest
/// z3 tiles).
pub(crate) fn draw_tolerance_m(rings: &[Vec<[i32; 2]>]) -> f64 {
    let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for p in rings.iter().flatten() {
        bb = [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])];
    }
    if bb[0] > bb[2] {
        return 60.0;
    }
    let lat = ((bb[1] as f64 + bb[3] as f64) / 2.0 * 1e-7).to_radians();
    let w = (bb[2] as f64 - bb[0] as f64) * M_PER_E7 * lat.dcos();
    let h = (bb[3] as f64 - bb[1] as f64) * M_PER_E7;
    (w.max(h) / 2000.0).clamp(60.0, 1000.0)
}

/// A ring in degrees to 5 decimals, closed as GeoJSON has it.
pub(crate) fn ring_degrees(r: &[[i32; 2]]) -> Vec<[f64; 2]> {
    let d = |v: i32| (v as f64 / 100.0).round() / 1e5;
    let mut out: Vec<[f64; 2]> = r.iter().map(|p| [d(p[0]), d(p[1])]).collect();
    if out.len() > 1 && out.first() != out.last() {
        out.push(out[0]);
    }
    out
}

/// A plain even–odd test, for checking the grid.
pub fn inside_plain(rings: &[Vec<[i32; 2]>], p: [i32; 2]) -> bool {
    let r: Vec<&[[i32; 2]]> = rings.iter().map(Vec::as_slice).collect();
    inside(&r, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e7(x: f64, y: f64) -> [i32; 2] {
        [(x * 1e7).round() as i32, (y * 1e7).round() as i32]
    }

    /// A star-ish polygon with a hole, its points on a coarse lattice so tests hit vertices and
    /// edges exactly too.
    fn shape_rings() -> Vec<Vec<[i32; 2]>> {
        let outer: Vec<[i32; 2]> = (0..40)
            .map(|i| {
                let t = i as f64 / 40.0 * std::f64::consts::TAU;
                let r = if i % 2 == 0 { 1.0 } else { 0.6 };
                e7(-2.0 + r * t.dcos(), 55.0 + r * t.dsin() * 0.6)
            })
            .collect();
        let hole = vec![e7(-2.1, 54.95), e7(-1.9, 54.95), e7(-1.9, 55.05), e7(-2.1, 55.05)];
        vec![outer, hole]
    }

    #[test]
    fn grid_agrees_with_plain_test() {
        let rings = shape_rings();
        let s = Shape::new("t".into(), rings.clone(), 0.0);
        let mut n = 0;
        // Pseudo-random points over the box and a lattice through vertices.
        let mut seed = 12345u64;
        for _ in 0..20000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = -3.2 + (seed >> 33) as f64 / (1u64 << 31) as f64 * 2.4;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let y = 54.2 + (seed >> 33) as f64 / (1u64 << 31) as f64 * 1.6;
            let p = e7(x, y);
            assert_eq!(s.contains(p), inside_plain(&rings, p), "at {x},{y}");
            n += inside_plain(&rings, p) as usize;
        }
        assert!(n > 1000, "{n} inside");
        for r in &rings {
            for &p in r {
                // On the boundary either answer is fine; it must not panic.
                let _ = s.contains(p);
            }
        }
    }

    #[test]
    fn buffer_reaches_a_kilometre() {
        let rings = vec![vec![e7(0.0, 50.0), e7(1.0, 50.0), e7(1.0, 51.0), e7(0.0, 51.0)]];
        let s = Shape::new("t".into(), rings, 1000.0);
        // 0.009° of latitude ≈ 1.0 km: just inside the buffer north of the top edge; 0.011° outside.
        assert!(s.contains(e7(0.5, 51.0085)));
        assert!(!s.contains(e7(0.5, 51.011)));
        // West of the west edge at 50.5° N, a km is 0.01412° of longitude.
        assert!(s.contains(e7(-0.0135, 50.5)));
        assert!(!s.contains(e7(-0.0150, 50.5)));
        assert!(s.contains(e7(0.5, 50.5)));
    }

    #[test]
    fn poly_files() {
        let text = "test\n1\n  0.0 50.0\n  1.0 50.0\n  1.0 51.0\n  0.0 51.0\nEND\n!2\n  0.4 50.4\n  0.6 50.4\n  0.6 50.6\n  0.4 50.6\nEND\nEND\n";
        let rings = read_poly(text).unwrap();
        assert_eq!(rings.len(), 2);
        let s = Shape::new("t".into(), rings, 0.0);
        assert!(s.contains(e7(0.2, 50.2)));
        assert!(!s.contains(e7(0.5, 50.5)));
    }

    #[test]
    fn a_drawn_region_meets_boxes_and_reads_back() {
        let ring = |pts: &[[f64; 2]]| pts.to_vec();
        let mut shapes = BTreeMap::new();
        shapes.insert(
            "shape:box.geojson".to_string(),
            vec![vec![ring(&[[10.0, 10.0], [11.0, 10.0], [11.0, 11.0], [10.0, 11.0], [10.0, 10.0]])]],
        );
        let r = DrawnRegion {
            id: "a".into(),
            name: "A".into(),
            outline: vec!["shape:box.geojson".into()],
            shapes,
            tiles: None,
        };
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        assert!(r.meets_rect(b(10.9, 10.4, 11.2, 10.6)));
        assert!(r.meets_rect(b(10.4, 10.4, 10.6, 10.6)), "wholly inside");
        assert!(!r.meets_rect(b(11.1, 10.4, 11.2, 10.6)));
        let back: DrawnRegion = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        // A built region is written without `tiles` (as before it was recorded); one still building
        // with them; either read by this app, an unknown field left alone.
        assert!(serde_json::to_value(&r).unwrap().get("tiles").is_none());
        let building = DrawnRegion { tiles: Some(Tiles { built: 3, total: 8 }), ..r.clone() };
        let v = serde_json::to_value(&building).unwrap();
        assert_eq!(v["tiles"], serde_json::json!({"built": 3, "total": 8}));
        assert_eq!(serde_json::from_value::<DrawnRegion>(v).unwrap(), building);
        let mut later = serde_json::to_value(&r).unwrap();
        later["later"] = serde_json::json!(1);
        assert_eq!(serde_json::from_value::<DrawnRegion>(later).unwrap(), r);
        // Simplified by size: a country's outline by up to a kilometre, a town's by 60 m.
        assert_eq!(draw_tolerance_m(&[vec![e7(0.0, 40.0), e7(30.0, 40.0), e7(30.0, 60.0)]]), 1000.0);
        assert_eq!(draw_tolerance_m(&[vec![e7(0.0, 40.0), e7(0.01, 40.0), e7(0.01, 40.01)]]), 60.0);
    }

    #[test]
    fn each_regions_coverage_is_its_own_shapes() {
        let d = tempfile::tempdir().unwrap();
        let rs = vec![
            Recipe {
                id: "a".into(),
                name: "A".into(),
                outline: vec!["place:20,20,10".into(), "place:21,20,10".into()],
            },
            Recipe { id: "b".into(), name: "B".into(), outline: vec!["place:40,40,10".into()] },
        ];
        let all = Coverage::fixture(&rs, None, d.path()).unwrap();
        let each = all.by_region();
        assert_eq!(each.iter().map(|(id, c)| (id.as_str(), c.shapes.len())).collect::<Vec<_>>(), [("a", 2), ("b", 1)]);
        // Its shapes' outlines and grids are the whole coverage's, not copies (#619: the agent held the
        // world's twice, 8 GB).
        let shared = |c: &Coverage| {
            c.shapes
                .iter()
                .map(|s| (std::sync::Arc::as_ptr(&s.rings), std::sync::Arc::as_ptr(&s.grid)))
                .collect::<Vec<_>>()
        };
        assert_eq!(each.iter().flat_map(|(_, c)| shared(c)).collect::<Vec<_>>(), shared(&all));
        // The same as made from each recipe alone.
        for (r, (_, c)) in rs.iter().zip(&each) {
            let alone = Coverage::fixture(std::slice::from_ref(r), None, d.path()).unwrap();
            let b = [190_000_000, 190_000_000, 420_000_000, 420_000_000];
            assert_eq!(c.fingerprint(b), alone.fingerprint(b));
            assert_eq!(c.contains([200_000_000, 200_000_000]), alone.contains([200_000_000, 200_000_000]));
        }
    }

    #[test]
    fn a_shapes_kept_answers_are_its_answers() {
        // (Its fingerprints and ways' answers kept, `Memo`: the same as one made afresh gives,
        // shared by its clones, and each question its own.)
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        let ring = || vec![vec![e7(0.0, 50.0), e7(1.0, 50.0), e7(1.0, 51.0), e7(0.0, 51.0)]];
        let kept = Shape::new("r: poly:x".into(), ring(), 1000.0);
        let clone = kept.clone();
        let boxes =
            [b(0.2, 49.9, 0.4, 50.1), b(0.4, 50.4, 0.6, 50.6), b(2.0, 50.4, 2.2, 50.6), b(0.9, 50.9, 1.1, 51.1)];
        for bx in boxes {
            let fresh = Shape::new("r: poly:x".into(), ring(), 1000.0).fingerprint_made(bx);
            assert_eq!(kept.fingerprint(bx), fresh);
            assert_eq!(clone.fingerprint(bx), fresh, "a clone's, kept by the first");
            assert_eq!(kept.fingerprint(bx), fresh, "asked again");
        }
        assert_eq!(kept.memo.lock().unwrap().fingerprints.len(), 4);
        // A way's points: in, out (its box apart: not kept), and one asked again by its digest.
        let (inside, outside) = ([7u8; 32], [8u8; 32]);
        let never = || -> std::vec::IntoIter<[i32; 2]> { panic!("not asked: kept, or its box misses the shape's") };
        assert!(kept.touches_way(&inside, b(0.5, 50.5, 0.6, 50.6), || vec![e7(0.5, 50.5), e7(0.6, 50.6)].into_iter()));
        assert!(
            !kept.touches_way(&outside, b(-0.5, 49.5, 0.5, 50.5), || vec![e7(-0.5, 50.5), e7(0.5, 49.5)].into_iter())
        );
        assert!(!kept.touches_way(&outside, b(3.0, 50.5, 3.1, 50.6), never));
        assert!(clone.touches_way(&inside, b(0.5, 50.5, 0.6, 50.6), never));
        assert_eq!(kept.memo.lock().unwrap().ways.len(), 2);
    }

    #[test]
    fn the_memos_together_stay_under_their_cap() {
        // (Every shape's memo counts toward one budget: past its cap, all start again. A budget of
        // this test's own, so no other test's memos are emptied.)
        let budget: &'static Budget =
            Box::leak(Box::new(Budget { cap: 1000, bytes: Default::default(), gen: Default::default() }));
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        let ring = |x: f64| vec![vec![e7(x, 50.0), e7(x + 1.0, 50.0), e7(x + 1.0, 51.0), e7(x, 51.0)]];
        let shape = |x: f64| {
            let s = Shape::new("a".into(), ring(x), 0.0);
            s.memo.lock().unwrap().budget = budget;
            s
        };
        let (one, two) = (shape(0.0), shape(5.0));
        let f1 = one.fingerprint(b(0.2, 49.9, 0.4, 50.1));
        two.fingerprint(b(5.2, 49.9, 5.4, 50.1));
        let held = budget.bytes.load(std::sync::atomic::Ordering::SeqCst);
        assert!(held > 0 && held < budget.cap && one.memo.lock().unwrap().current());
        // Past the cap: a new generation; the other's answers are stale, none.
        let mut i = 0.0;
        while two.memo.lock().unwrap().gen == 0 {
            two.fingerprint(b(5.2 + i, 49.9, 5.3 + i, 50.1));
            i += 0.01;
        }
        assert!(budget.bytes.load(std::sync::atomic::Ordering::SeqCst) <= budget.cap);
        assert!(!one.memo.lock().unwrap().current(), "emptied as it next answers");
        assert_eq!(one.fingerprint(b(0.2, 49.9, 0.4, 50.1)), f1, "worked out again, the same");
        assert!(one.memo.lock().unwrap().current() && one.memo.lock().unwrap().fingerprints.len() == 1);
        drop(one);
        assert!(budget.bytes.load(std::sync::atomic::Ordering::SeqCst) < budget.cap);
    }

    #[test]
    fn fingerprints_see_only_their_box() {
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        let quad = |south: f64, north: f64| vec![vec![e7(0.0, south), e7(1.0, south), e7(1.0, north), e7(0.0, north)]];
        let fp = |rings: Vec<Vec<[i32; 2]>>, bx: [i32; 4]| {
            Coverage { shapes: vec![Shape::new("r: poly:x".into(), rings, 0.0)], ..Default::default() }.fingerprint(bx)
        };
        // A box across the south edge.
        let across = b(0.2, 49.9, 0.4, 50.1);
        let f0 = fp(quad(50.0, 51.0), across);
        assert!(!f0.is_empty());
        assert_eq!(fp(quad(50.0, 51.3), across), f0, "the north edge moved, far from the box");
        assert_ne!(fp(quad(50.05, 51.0), across), f0, "the south edge moved through the box");
        // A box wholly inside: covered, whatever the edges far away do; wholly outside: nothing.
        let inner = b(0.4, 50.4, 0.6, 50.6);
        assert!(!fp(quad(50.0, 51.0), inner).is_empty());
        assert_eq!(fp(quad(50.0, 51.0), inner), fp(quad(49.0, 52.0), inner));
        assert_eq!(fp(quad(50.0, 51.0), b(2.0, 50.4, 2.2, 50.6)), "");
        // Same geometry from another region or entry: the same fingerprint.
        let two = Coverage {
            shapes: vec![
                Shape::new("a: poly:x".into(), quad(50.0, 51.0), 0.0),
                Shape::new("b: osm:1".into(), quad(50.0, 51.0), 0.0),
            ],
            ..Default::default()
        };
        assert_eq!(two.fingerprint(across), f0);
        // The buffer: an edge 0.5 km outside the box is within a 1 km buffer of it.
        let near = b(0.2, 49.9, 0.4, 49.995);
        let buffered = |south: f64| {
            Coverage { shapes: vec![Shape::new("o".into(), quad(south, 51.0), 1000.0)], ..Default::default() }
                .fingerprint(near)
        };
        assert_ne!(buffered(50.0), buffered(50.001), "the edge moved within the buffer's reach");
    }

    #[test]
    fn a_line_through_a_vertex_crosses_once() {
        // The review's case: from (0, -5) up to (0, 5), through the vertex (0, 0) of a ring
        // ...(-1, 1) -> (0, 0) -> (1, 1)...: one crossing; a ring touching the line there, none.
        let (p1, p2) = ([0, -5], [0, 5]);
        let through = [segments_cross(p1, p2, [-1, 1], [0, 0]), segments_cross(p1, p2, [0, 0], [1, 1])];
        assert_eq!(through.iter().filter(|&&c| c).count(), 1);
        let touching = [segments_cross(p1, p2, [-1, 1], [0, 0]), segments_cross(p1, p2, [0, 0], [-1, -1])];
        assert_eq!(touching.iter().filter(|&&c| c).count(), 0);
        // And the same along a slanted line.
        let (p1, p2) = ([-5, -5], [5, 5]);
        let through = [segments_cross(p1, p2, [-2, 2], [0, 0]), segments_cross(p1, p2, [0, 0], [2, -2])];
        assert_eq!(through.iter().filter(|&&c| c).count(), 1);
    }

    #[test]
    fn a_ring_held_twice_isnt_the_ring() {
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        let quad = vec![e7(0.0, 50.0), e7(1.0, 50.0), e7(1.0, 51.0), e7(0.0, 51.0)];
        let once = Coverage { shapes: vec![Shape::new("a".into(), vec![quad.clone()], 0.0)], ..Default::default() };
        let twice =
            Coverage { shapes: vec![Shape::new("a".into(), vec![quad.clone(), quad], 0.0)], ..Default::default() };
        // A box across the south edge, its corner outside: under even-odd the doubled ring covers
        // nothing, so the fingerprints must differ.
        let across = b(0.2, 49.9, 0.4, 50.1);
        assert!(twice.shapes[0].contains(e7(0.3, 50.05)) != once.shapes[0].contains(e7(0.3, 50.05)));
        assert_ne!(once.fingerprint(across), twice.fingerprint(across));
    }

    #[test]
    fn boxes_meet_exactly() {
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        // Singapore-sized (0.4° × 0.2°), between the points an 8×8 sample of a z6 tile would test.
        let small =
            Shape::new("s".into(), vec![vec![e7(103.6, 1.2), e7(104.0, 1.2), e7(104.0, 1.4), e7(103.6, 1.4)]], 0.0);
        assert!(small.meets_rect(b(101.25, 0.0, 106.875, 5.6)));
        assert!(!small.meets_rect(b(104.1, 1.0, 105.0, 2.0)));
        // A box wholly inside a big shape, no edge near it; one wholly in its hole.
        let big = Shape::new("b".into(), shape_rings(), 0.0);
        assert!(big.meets_rect(b(-2.45, 54.98, -2.35, 55.02)));
        assert!(!big.meets_rect(b(-2.05, 54.98, -2.0, 55.02)));
        // Every box agrees with points: a box meets the shape when a point inside it is inside, and
        // a box with none of a fine lattice inside and no edge near is outside.
        for i in 0..60 {
            for j in 0..40 {
                let (x, y) = (-3.2 + i as f64 * 0.04, 54.3 + j as f64 * 0.035);
                let r = b(x, y, x + 0.03, y + 0.02);
                let any =
                    (0..=6).any(|a| (0..=4).any(|c| big.contains(e7(x + a as f64 * 0.005, y + c as f64 * 0.005))));
                if any {
                    assert!(big.meets_rect(r), "box at {x},{y}");
                }
            }
        }
        // A thin strip crossing a box with no vertex in it and no corner inside.
        let strip =
            Shape::new("t".into(), vec![vec![e7(0.0, 10.0), e7(2.0, 10.0), e7(2.0, 10.001), e7(0.0, 10.001)]], 0.0);
        assert!(strip.meets_rect(b(0.9, 9.9, 1.1, 10.1)));
        assert!(!strip.meets_rect(b(0.9, 10.01, 1.1, 10.1)));
        // The buffer: 1 km around an osm: outline.
        let buffered =
            Shape::new("o".into(), vec![vec![e7(0.0, 50.0), e7(1.0, 50.0), e7(1.0, 51.0), e7(0.0, 51.0)]], 1000.0);
        assert!(buffered.meets_rect(b(1.008, 50.5, 1.02, 50.6)));
        assert!(!buffered.meets_rect(b(1.03, 50.5, 1.04, 50.6)));
    }

    #[test]
    fn shapes_at_points_and_boxes() {
        let b = |w: f64, s: f64, e: f64, n: f64| [e7(w, s)[0], e7(w, s)[1], e7(e, n)[0], e7(e, n)[1]];
        let square = |x: f64| vec![vec![e7(x, 50.0), e7(x + 1.0, 50.0), e7(x + 1.0, 51.0), e7(x, 51.0)]];
        // Two shapes side by side, the second overlapping the first's east edge.
        let c = Coverage {
            shapes: vec![Shape::new("a".into(), square(0.0), 1000.0), Shape::new("b".into(), square(0.9), 0.0)],
            ..Default::default()
        };
        assert_eq!(c.shape_at(e7(0.5, 50.5)), Some(0));
        assert_eq!(c.shape_at(e7(0.95, 50.5)), Some(0), "in both: the first");
        assert_eq!(c.shape_at(e7(1.5, 50.5)), Some(1));
        assert_eq!(c.shape_at(e7(3.0, 50.5)), None);
        // Within the first's buffer but inside the second: the second's (its outline before any
        // buffer); within the first's buffer only: the first's.
        assert_eq!(c.shape_at(e7(1.005, 50.5)), Some(1));
        assert_eq!(c.shape_at(e7(0.5, 51.005)), Some(0));
        assert_eq!(c.shape_at(e7(0.5, 51.02)), None);
        // A box there likewise: in the second's outline, the first's buffer no matter.
        assert_eq!(c.box_shape(b(1.002, 50.4, 1.008, 50.6)), Some(1));
        // A box wholly inside the first, far from its edges: every point of it the first's.
        assert!(c.shapes[0].covers_rect(b(0.4, 50.4, 0.6, 50.6)));
        assert_eq!(c.box_shape(b(0.4, 50.4, 0.6, 50.6)), Some(0));
        // Near an edge (within the buffer), across one, or where the first meets it first: asked
        // point by point.
        assert!(!c.shapes[0].covers_rect(b(0.4, 50.4, 0.995, 50.6)));
        assert_eq!(c.box_shape(b(0.95, 50.4, 1.5, 50.6)), None);
        assert_eq!(c.box_shape(b(1.3, 50.4, 1.6, 50.6)), Some(1));
        assert_eq!(c.box_shape(b(3.0, 50.4, 3.2, 50.6)), None);
        // Every point of a box box_shape answers for has that shape.
        for (w, e) in [(0.4, 0.6), (1.002, 1.008), (1.3, 1.6)] {
            let s = c.box_shape(b(w, 50.4, e, 50.6)).unwrap();
            for k in 0..=10 {
                let x = w + (e - w) * k as f64 / 10.0;
                assert_eq!(c.shape_at(e7(x, 50.4 + 0.02 * k as f64)), Some(s), "{x}");
            }
        }
        // The key of a box: its shapes in order, with their countries; the order matters.
        let mut c2 = c.clone();
        c2.shapes.swap(0, 1);
        let near = b(0.95, 50.4, 1.0, 50.6);
        assert_ne!(c.shapes_key(near), c2.shapes_key(near));
        assert_eq!(c.fingerprint(near), c2.fingerprint(near), "fingerprint misses the order");
        c2.shapes[0].country = "FR".into();
        assert_ne!(c2.shapes_key(near), {
            let mut c3 = c2.clone();
            c3.shapes[0].country = "MC".into();
            c3.shapes_key(near)
        });
        assert_eq!(c.shapes_key(b(3.0, 50.4, 3.2, 50.6)), "");
    }

    /// A region's water (#468): held (`contains`, `touches`, the fingerprint), no target
    /// (`meets_rect`, `meets_box`) but the depth's; kept with its mark.
    #[test]
    fn a_regions_water_is_held_but_no_target() {
        let sq = |x0: f64, x1: f64| vec![vec![e7(x0, 50.0), e7(x1, 50.0), e7(x1, 51.0), e7(x0, 51.0)]];
        let land = Shape::new("r: x".into(), sq(0.0, 1.0), 0.0);
        let mut sea = Shape::new("r: x (water)".into(), sq(1.0, 2.0), 0.0);
        sea.water = true;
        let c = Coverage { shapes: vec![land.clone(), sea], depth_only: Vec::new() };
        let r = |x: f64| [e7(x, 50.5)[0], e7(x, 50.5)[1], e7(x + 0.1, 50.5)[0], e7(x, 50.6)[1]];
        assert!(c.meets_rect(r(0.4)) && c.meets_rect_depth(r(0.4)));
        assert!(!c.meets_rect(r(1.4)) && c.meets_rect_depth(r(1.4)) && c.contains(e7(1.5, 50.5)));
        assert!(!c.meets_box(r(1.4)) && c.touches(&[e7(1.5, 50.5)]));
        let alone = Coverage { shapes: vec![land], depth_only: Vec::new() };
        assert_ne!(c.fingerprint(r(1.4)), alone.fingerprint(r(1.4)));
        let back = from_kept(&kept_bytes(&c)).unwrap();
        assert_eq!(back.shapes.iter().map(|s| s.water).collect::<Vec<_>>(), vec![false, true]);
        assert!(!back.meets_rect(r(1.4)) && back.meets_rect_depth(r(1.4)));
    }

    /// A coverage's read bound (`Coverage::read_peak_mb`, task #639) covers what reading it holds, on
    /// a real-shaped one: a coast of 300,000 points over a country's box with the coasts' buffer,
    /// many small outlines besides, read back from its kept bytes.
    #[test]
    fn the_read_bound_covers_a_real_shaped_coverage() {
        let coast: Vec<[i32; 2]> = (0..300_000)
            .map(|i| {
                let t = i as f64 / 300_000.0 * std::f64::consts::TAU;
                let r = 4.0 + 0.3 * (t * 997.0).dsin() + 0.1 * (t * 13_001.0).dsin();
                e7(10.0 + r * t.dcos(), 50.0 + r * 0.6 * t.dsin())
            })
            .collect();
        let mut shapes = vec![Shape::new("r: osm:1".into(), vec![coast], OSM_BUFFER_M)];
        for k in 0..200 {
            let (x, y) = (-5.0 + (k % 20) as f64, 40.0 + (k / 20) as f64);
            shapes.push(Shape::new(format!("r{k}: place"), vec![fixture_circle(x, y, 20.0)], 0.0));
        }
        let c = from_kept(&kept_bytes(&Coverage { shapes, depth_only: Vec::new() })).unwrap();
        let points: u64 = c.shapes.iter().flat_map(|s| s.rings.iter()).map(|r| r.len() as u64).sum();
        let entries: u64 = c.shapes.iter().flat_map(|s| s.grid.edges.iter()).map(|e| e.len() as u64).sum();
        let cells: u64 = c.shapes.iter().map(|s| (s.grid.nx * s.grid.ny) as u64).sum();
        // Held at once while it's read: the kept bytes, the rings made from them, every grid, the memos.
        let held = kept_bytes(&c).len() as u64 + points * 8 + cells * 25 + entries * 8 + MEMO_CAP as u64;
        let mb = c.read_peak_mb();
        assert!(mb << 20 >= held, "{mb} MB under {held} B");
        assert!(entries > points, "the coast's buffer lists its edges in several cells");
        assert!(mb > (MEMO_CAP >> 20) as u64 + 5, "{mb} MB: the coverage's own part counted");
    }
}
