//! Coverage (docs/regions.md): the union of the regions' outlines. A feature is built if it touches
//! the coverage (for a way: any vertex inside, as Geofabrik does).
//!
//! Each outline becomes a [`Shape`]: its rings, a buffer (1 km for `osm:` outlines, so piers and
//! coastal roads on a boundary that follows the coastline are in), and a cell grid over its bounding
//! box. Every cell knows whether its centre is inside and which edges cross it (or pass within the
//! buffer), so a point test reads one cell: the centre's state, flipped by each of the cell's edges
//! crossing the segment from the centre to the point, then the buffer's distance to the cell's
//! edges. Building a grid costs one pass over the edges plus a scanline per row. A box test (which
//! edges meet a box: `Shape::meets_rect`, `covers_rect`, `fingerprint`) reads the shape's edge tree
//! instead (`EdgeTree`: boxes of runs of edges along the rings), only where its boxes meet the box.
//! What a whole coverage's tiles are is kept by its version ([`Kept`]).
//!
//! A catalogue records the coverage it was built for, simplified for drawing ([`DrawnRegion`]), so
//! the map draws it from the catalogue rather than from the recipes.

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
    /// Its edges by place (`EdgeTree`): what `edges_meeting` reads.
    tree: std::sync::Arc<EdgeTree>,
    /// Its rings' digest (`Shape::rings_digest`), worked out once.
    rings_digest: std::sync::Arc<std::sync::OnceLock<String>>,
    /// Its rings' digest with where each ring ends (`Shape::rings_print`), worked out once.
    rings_print: std::sync::Arc<std::sync::OnceLock<[u8; 32]>>,
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

#[derive(Clone, PartialEq)]
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
    /// Per cell: edges (ring, index of first point) crossing it or within the buffer of it, cell
    /// `c`'s at `edges[starts[c]..starts[c + 1]]` (`Grid::edges`). (One list, not a list a cell:
    /// a cell's empty list was 24 B, most of a shape's million cells empty; lead memory, 25 Oct:
    /// 6.4 GB of the agent's 17.)
    starts: Vec<u32>,
    edges: Vec<(u32, u32)>,
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
        let tree = EdgeTree::build(&rings);
        Shape {
            source,
            rings: std::sync::Arc::new(rings),
            buffer_m,
            bbox,
            country: String::new(),
            water: false,
            grid: std::sync::Arc::new(grid),
            tree: std::sync::Arc::new(tree),
            rings_digest: Default::default(),
            rings_print: Default::default(),
            memo: Default::default(),
        }
    }

    /// A digest of its rings (`store::naming::hash16` of their points, ring after ring), worked out
    /// once: what keys made of the whole coverage read (crate::agent::build::coverage_all), every
    /// loop (task #894: hashing every point of the world's outlines each time was seconds).
    pub fn rings_digest(&self) -> &str {
        self.rings_digest.get_or_init(|| store::naming::hash16(bytemuck::cast_slice(&self.rings.concat())))
    }

    /// Its rings' digest (`rings_digest`) and where each ring ends (the digest is of the points
    /// alone, ring after ring), worked out once: `Coverage::version`'s part of it, which every
    /// `Coverage::kept` asks (task #898: hashing every ring's length each time was most of the
    /// checklist's deep targets).
    fn rings_print(&self) -> &[u8; 32] {
        self.rings_print.get_or_init(|| {
            let mut h = blake3::Hasher::new();
            h.update(self.rings_digest().as_bytes());
            h.update(&(self.rings.len() as u64).to_le_bytes());
            for r in self.rings.iter() {
                h.update(&(r.len() as u64).to_le_bytes());
            }
            *h.finalize().as_bytes()
        })
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
        for &(r, i) in g.edges(c) {
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
        g.edges(c).iter().any(|&(r, i)| {
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

    /// The ring edges meeting the box `rg` (w, s, e, n, E7, edges included), sorted, each once per
    /// place in the rings as its lesser end
    /// then its other (direction doesn't matter to which points are inside); an edge the rings
    /// hold twice stays twice, since under even–odd the two cancel.
    pub fn edges_in(&self, rg: [i64; 4]) -> Vec<[i32; 4]> {
        let mut by_place: std::collections::BTreeMap<(u32, u32), [i32; 4]> = std::collections::BTreeMap::new();
        self.edges_meeting(rg, &mut |at, a, b| {
            by_place.insert(
                at,
                if (a[0], a[1]) <= (b[0], b[1]) { [a[0], a[1], b[0], b[1]] } else { [b[0], b[1], a[0], a[1]] },
            );
            false
        });
        let mut edges: Vec<[i32; 4]> = by_place.into_values().collect();
        edges.sort_unstable();
        edges
    }

    /// Calls `f` with each ring edge meeting the box `rg`, with its place (ring, index), once each
    /// (by the edge tree, `EdgeTree`), until it returns true; whether one did.
    fn edges_meeting(&self, rg: [i64; 4], f: &mut dyn FnMut((u32, u32), [i32; 2], [i32; 2]) -> bool) -> bool {
        self.tree.meeting(&self.rings, rg, f)
    }

    /// The shape as it is inside the box w, s, e, n (E7): whether a reference point of the box (its
    /// south-west corner, or the first point up its diagonal on no edge) is inside the rings, and
    /// every ring edge meeting the box grown by the buffer, each as often as the rings hold it. That
    /// decides which points of the box the shape contains: a point's inside-ness is the reference
    /// point's, flipped by the edges crossing the line between them (inside the box), and its buffer
    /// reads the edges within the buffer of it. None when the shape has nothing in the box.
    fn fingerprint(&self, r: [i32; 4]) -> Option<String> {
        // (A box its box misses has none: told at once, not kept. Every shape was asked of every
        // z6 tile near the coverage, most far from it, each answer kept: the memos' cap was
        // reached and begun again every loop, task #894.)
        if !self.grown_meets_box(r) {
            return None;
        }
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

    /// Whether the box w, s, e, n (E7) grown by the buffer meets its box.
    fn grown_meets_box(&self, r: [i32; 4]) -> bool {
        let rg = self.grown(r);
        !(rg[0] > self.bbox[2] as i64
            || rg[2] < self.bbox[0] as i64
            || rg[1] > self.bbox[3] as i64
            || rg[3] < self.bbox[1] as i64)
    }

    /// `fingerprint`, worked out (for a box `grown_meets_box`).
    fn fingerprint_made(&self, r: [i32; 4]) -> Option<String> {
        let rg = self.grown(r);
        let edges = self.edges_in(rg);
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
        // Every cell each edge's box (grown by the buffer) touches: a superset, fine for the
        // crossing test and needed for the buffer. Counted, then listed in place.
        let each = |f: &mut dyn FnMut(usize, u32, u32)| {
            for (ri, r) in rings.iter().enumerate() {
                for i in 0..r.len() {
                    let (a, b) = (r[i], r[(i + 1) % r.len()]);
                    if a == b {
                        continue;
                    }
                    let cx0 = ((((a[0].min(b[0]) as f64 - buf_e7) - x0) / cw).floor().max(0.0) as usize).min(nx - 1);
                    let cx1 = ((((a[0].max(b[0]) as f64 + buf_e7) - x0) / cw).floor().max(0.0) as usize).min(nx - 1);
                    let cy0 = ((((a[1].min(b[1]) as f64 - buf_e7) - y0) / ch).floor().max(0.0) as usize).min(ny - 1);
                    let cy1 = ((((a[1].max(b[1]) as f64 + buf_e7) - y0) / ch).floor().max(0.0) as usize).min(ny - 1);
                    for cy in cy0..=cy1 {
                        for cx in cx0..=cx1 {
                            f(cy * nx + cx, ri as u32, i as u32);
                        }
                    }
                }
            }
        };
        let mut starts: Vec<u32> = vec![0; nx * ny + 1];
        each(&mut |c, _, _| starts[c + 1] += 1);
        for c in 0..nx * ny {
            // (Past u32's 4 billion entries the starts would wrap: refused, not read wrong.)
            starts[c + 1] = starts[c + 1].checked_add(starts[c]).expect("a shape's grid over 4 billion edge entries");
        }
        let mut edges: Vec<(u32, u32)> = vec![(0, 0); starts[nx * ny] as usize];
        let mut at: Vec<u32> = starts[..nx * ny].to_vec();
        each(&mut |c, ri, i| {
            edges[at[c] as usize] = (ri, i);
            at[c] += 1;
        });
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
        Grid { x0, y0, cw, ch, nx, ny, centre, starts, edges }
    }

    /// Cell `c`'s edges.
    fn edges(&self, c: usize) -> &[(u32, u32)] {
        &self.edges[self.starts[c] as usize..self.starts[c + 1] as usize]
    }
}

/// A shape's edges by place, for the boxes they meet (`Shape::edges_meeting`: `meets_rect`,
/// `covers_rect`, `fingerprint`): runs of `EdgeTree::RUN` edges one after another along a ring, each
/// run's box, and above them boxes of `EdgeTree::FAN` boxes each, up to a few at the top; a box is
/// read down only where it meets the box asked. (The grid's cells were read edge by edge: a z9–12
/// tile on a long coast read cells of thousands of edges, minutes a loop on the world's coverage,
/// task #894.) Made from the rings when a shape is (`Shape::new`, `from_kept`): about a byte an edge.
struct EdgeTree {
    /// Each run: its ring, its first edge, its edges (≤ RUN), in the order of `levels[0]`.
    runs: Vec<(u32, u32, u32)>,
    /// Boxes (w, s, e, n, E7): `levels[0]` the runs', `levels[k][i]` the box of
    /// `levels[k - 1][i * FAN..(i + 1) * FAN]`; the last level has FAN boxes at most.
    levels: Vec<Vec<[i32; 4]>>,
}

impl EdgeTree {
    const RUN: usize = 16;
    const FAN: usize = 8;

    fn build(rings: &[Vec<[i32; 2]>]) -> EdgeTree {
        let mut runs: Vec<((u32, u32, u32), [i32; 4])> = Vec::new();
        for (ri, r) in rings.iter().enumerate() {
            for start in (0..r.len()).step_by(Self::RUN) {
                let n = Self::RUN.min(r.len() - start);
                let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
                for i in start..=start + n {
                    let p = r[i % r.len()];
                    b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                }
                runs.push(((ri as u32, start as u32, n as u32), b));
            }
        }
        // Near runs together (by their centres' Morton order), so the boxes above them are tight.
        let morton = |b: &[i32; 4]| {
            let c = |lo: i32, hi: i32| (((lo as i64 + hi as i64) / 2 + (1i64 << 31)) >> 16) as u64;
            let spread = |mut v: u64| {
                v &= 0xffff;
                v = (v | (v << 8)) & 0x00ff_00ff;
                v = (v | (v << 4)) & 0x0f0f_0f0f;
                v = (v | (v << 2)) & 0x3333_3333;
                (v | (v << 1)) & 0x5555_5555
            };
            spread(c(b[0], b[2])) | (spread(c(b[1], b[3])) << 1)
        };
        runs.sort_by_key(|(at, b)| (morton(b), *at));
        let mut levels = vec![runs.iter().map(|&(_, b)| b).collect::<Vec<_>>()];
        while levels.last().unwrap().len() > Self::FAN {
            let up = levels
                .last()
                .unwrap()
                .chunks(Self::FAN)
                .map(|c| {
                    c.iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |a, b| {
                        [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
                    })
                })
                .collect();
            levels.push(up);
        }
        EdgeTree { runs: runs.into_iter().map(|(at, _)| at).collect(), levels }
    }

    /// The bytes it holds, about.
    fn bytes(&self) -> u64 {
        (self.runs.len() * 12 + self.levels.iter().map(|l| l.len() * 16).sum::<usize>()) as u64
    }

    /// `Shape::edges_meeting` of `rings` (the rings it was made from): each edge (not of a
    /// point twice) meeting `rg` once, until `f` returns true; whether it did.
    fn meeting(
        &self,
        rings: &[Vec<[i32; 2]>],
        rg: [i64; 4],
        f: &mut dyn FnMut((u32, u32), [i32; 2], [i32; 2]) -> bool,
    ) -> bool {
        let meets =
            |b: &[i32; 4]| b[0] as i64 <= rg[2] && b[2] as i64 >= rg[0] && b[1] as i64 <= rg[3] && b[3] as i64 >= rg[1];
        let top = self.levels.len() - 1;
        let mut stack: Vec<(usize, usize)> = (0..self.levels[top].len()).rev().map(|i| (top, i)).collect();
        while let Some((k, i)) = stack.pop() {
            if !meets(&self.levels[k][i]) {
                continue;
            }
            if k > 0 {
                let below = self.levels[k - 1].len();
                stack.extend((i * Self::FAN..((i + 1) * Self::FAN).min(below)).rev().map(|j| (k - 1, j)));
                continue;
            }
            let (ri, start, n) = self.runs[i];
            let ring = &rings[ri as usize];
            for e in start..start + n {
                let (a, b) = (ring[e as usize], ring[(e as usize + 1) % ring.len()]);
                if a != b
                    && segment_meets_box([a[0] as i64, a[1] as i64], [b[0] as i64, b[1] as i64], rg)
                    && f((ri, e), a, b)
                {
                    return true;
                }
            }
        }
        false
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

/// What's been worked out of a whole coverage, by its version (`Coverage::version`): the answers
/// every loop asks of it again (the z6 tiles near it, which of them make hi tiles), which depend on
/// its shapes alone (task #894: worked out every loop from the full geometry, they were most of the
/// checklist's minutes). Kept for the last `KEPT_VERSIONS` versions (a region's coverage is one).
#[derive(Default)]
pub struct Kept {
    /// crate::agent::build::coverage_tiles.
    pub tiles: std::sync::OnceLock<BTreeMap<(u32, u32), Vec<(u32, u32)>>>,
    /// crate::terrain_pack::makes_hi, by z6 tile.
    pub makes_hi: std::sync::Mutex<std::collections::HashMap<(u32, u32), bool>>,
}

/// The coverage versions `Kept` keeps answers of: the whole coverage, and each region's (the
/// regions' progress asks each its own) with room. A version's answers are at most the 4,096 z6
/// tiles twice (its tiles, 8 B each, and `makes_hi`, ~16 B an entry), under 100 KB; all 512 under
/// 50 MB, in practice far less (a region is near a few z6 tiles).
const KEPT_VERSIONS: usize = 512;

static KEPT: std::sync::Mutex<Vec<(String, std::sync::Arc<Kept>)>> = std::sync::Mutex::new(Vec::new());

impl Coverage {
    /// Its version: a digest of what decides the answers `Kept` keeps (each shape's rings, by
    /// `Shape::rings_print`: their digest and lengths, worked out once a shape; buffer and water, in
    /// each list), worked out without reading the rings' points.
    pub fn version(&self) -> String {
        let mut h = blake3::Hasher::new();
        for (k, list) in [&self.shapes, &self.depth_only].into_iter().enumerate() {
            h.update(&[k as u8]);
            h.update(&(list.len() as u64).to_le_bytes());
            for sh in list {
                h.update(sh.rings_print());
                h.update(&sh.buffer_m.to_bits().to_le_bytes());
                h.update(&[u8::from(sh.water)]);
            }
        }
        h.finalize().to_hex()[..32].to_string()
    }

    /// What's been worked out of this coverage's version (`Kept`), the same for every coverage of it.
    pub fn kept(&self) -> std::sync::Arc<Kept> {
        let v = self.version();
        let mut all = KEPT.lock().unwrap();
        if let Some(i) = all.iter().position(|(k, _)| *k == v) {
            let e = all.remove(i);
            all.push(e);
        } else {
            if all.len() >= KEPT_VERSIONS {
                all.remove(0);
            }
            all.push((v, Default::default()));
        }
        all.last().unwrap().1.clone()
    }

    /// A digest of what the coverage holds (every shape's source, rings, buffer, country and
    /// water): part of a resumed run's identity (crate::resume_log::job_id), task #889.
    pub fn digest(&self) -> String {
        let mut h = blake3::Hasher::new();
        for (k, list) in [&self.shapes, &self.depth_only].into_iter().enumerate() {
            h.update(&[k as u8]);
            h.update(&(list.len() as u64).to_le_bytes());
            for sh in list {
                for t in [&sh.source, &sh.country] {
                    h.update(&(t.len() as u64).to_le_bytes());
                    h.update(t.as_bytes());
                }
                h.update(&sh.buffer_m.to_bits().to_le_bytes());
                h.update(&[u8::from(sh.water)]);
                h.update(&(sh.rings.len() as u64).to_le_bytes());
                for r in sh.rings.iter() {
                    h.update(&(r.len() as u64).to_le_bytes());
                    for p in r {
                        h.update(&p[0].to_le_bytes());
                        h.update(&p[1].to_le_bytes());
                    }
                }
            }
        }
        h.finalize().to_hex().to_string()
    }

    /// The memory a step's process takes to read this coverage (MB; `coverage_of` in scenic-build,
    /// through `Coverage::accepted`), worked out from the coverage itself: every shape's rings (8 B a
    /// point), its grid (`Grid`: per cell a bool and its edges' 4 B start, 8 B an entry), its edge tree
    /// (`EdgeTree::bytes`), the
    /// kept file's stream buffer and one shape's cells in passing (`from_kept`, `write_kept`: read
    /// and written as a stream, task #807), and the memos' cap (`MEMO_CAP`). A job's memory bound
    /// adds it to what its step holds besides (the agent's `guess` and `peak`, task #639).
    pub fn read_peak_mb(&self) -> u64 {
        let mut bytes = MEMO_CAP as u64 + (1 << 20);
        let mut passing = 0u64;
        for list in [&self.shapes, &self.depth_only] {
            for sh in list {
                let points: u64 = sh.rings.iter().map(|r| r.len() as u64).sum();
                let g = &sh.grid;
                let rings = points * 8 + sh.rings.len() as u64 * 24;
                let grid = (g.nx * g.ny) as u64 * 5 + 4 + g.edges.len() as u64 * 8 + sh.tree.bytes();
                passing = passing.max((g.nx * g.ny) as u64);
                bytes += rings + grid + sh.source.len() as u64 + sh.country.len() as u64;
            }
        }
        bytes += passing;
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

const KEPT_MAGIC: &[u8; 8] = b"RDCOV005";

/// The coverage's shapes as kept (`Coverage::accepted`): their source, country, buffer, whether
/// water, rings, box and grid (task #807: the grids made again from the rings were 14 s of CPU a
/// job). Written and read as a stream (`write_kept`, `from_kept`), never whole in memory: the file
/// is about as large as the coverage itself (1.84 GB, 27 Oct).
#[cfg(test)]
pub(crate) fn kept_bytes(c: &Coverage) -> Vec<u8> {
    let mut b = Vec::new();
    write_kept(c, &mut b).unwrap();
    b
}

/// `kept_bytes` written to `w`.
pub(crate) fn write_kept(c: &Coverage, w: &mut dyn std::io::Write) -> std::io::Result<()> {
    let n = |w: &mut dyn std::io::Write, v: usize| w.write_all(&(v as u64).to_le_bytes());
    w.write_all(KEPT_MAGIC)?;
    for list in [&c.shapes, &c.depth_only] {
        n(w, list.len())?;
        for sh in list {
            for t in [&sh.source, &sh.country] {
                n(w, t.len())?;
                w.write_all(t.as_bytes())?;
            }
            w.write_all(&sh.buffer_m.to_le_bytes())?;
            w.write_all(&[sh.water as u8])?;
            n(w, sh.rings.len())?;
            for r in sh.rings.iter() {
                n(w, r.len())?;
                w.write_all(bytemuck::cast_slice(r))?;
            }
            w.write_all(bytemuck::cast_slice(&sh.bbox))?;
            let g = &sh.grid;
            for v in [g.x0, g.y0, g.cw, g.ch] {
                w.write_all(&v.to_le_bytes())?;
            }
            for v in [g.nx, g.ny, g.edges.len()] {
                n(w, v)?;
            }
            w.write_all(&g.centre.iter().map(|&c| c as u8).collect::<Vec<u8>>())?;
            w.write_all(bytemuck::cast_slice(&g.starts))?;
            for &(ri, i) in &g.edges {
                w.write_all(&ri.to_le_bytes())?;
                w.write_all(&i.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

/// `kept_bytes` read back from `r` as a stream: the shapes as `Shape::new` made them, their grids
/// read, not made; None when they don't read (cut short, bytes past the end, another format).
pub(crate) fn from_kept(r: &mut dyn std::io::Read) -> Option<Coverage> {
    // (A length read is checked against the bytes left only by reading them: an absurd one fails
    // its allocation's try_reserve rather than aborting.)
    fn take(r: &mut dyn std::io::Read, n: usize) -> Option<Vec<u8>> {
        let mut v = Vec::new();
        v.try_reserve_exact(n).ok()?;
        std::io::Read::read_to_end(&mut std::io::Read::take(r, n as u64), &mut v).ok()?;
        (v.len() == n).then_some(v)
    }
    fn pod<T: bytemuck::Pod>(r: &mut dyn std::io::Read, count: usize) -> Option<Vec<T>> {
        let mut v = Vec::new();
        v.try_reserve_exact(count).ok()?;
        v.resize(count, T::zeroed());
        r.read_exact(bytemuck::cast_slice_mut(&mut v)).ok()?;
        Some(v)
    }
    fn n(r: &mut dyn std::io::Read) -> Option<usize> {
        usize::try_from(u64::from_le_bytes(pod::<u8>(r, 8)?.try_into().ok()?)).ok()
    }
    fn f(r: &mut dyn std::io::Read) -> Option<f64> {
        Some(f64::from_le_bytes(pod::<u8>(r, 8)?.try_into().ok()?))
    }
    fn text(r: &mut dyn std::io::Read) -> Option<String> {
        let len = n(r)?;
        String::from_utf8(take(r, len)?).ok()
    }
    if take(r, 8)? != KEPT_MAGIC {
        return None;
    }
    let mut lists = [Vec::new(), Vec::new()];
    for shapes in lists.iter_mut() {
        for _ in 0..n(r)? {
            let source = text(r)?;
            let country = text(r)?;
            let buffer_m = f(r)?;
            let water = take(r, 1)?[0] != 0;
            let mut rings = Vec::new();
            for _ in 0..n(r)? {
                let len = n(r)?;
                rings.push(pod::<[i32; 2]>(r, len)?);
            }
            let bbox: [i32; 4] = pod::<[i32; 4]>(r, 1)?[0];
            let (x0, y0, cw, ch) = (f(r)?, f(r)?, f(r)?, f(r)?);
            let (nx, ny, ne) = (n(r)?, n(r)?, n(r)?);
            let cells = nx.checked_mul(ny)?;
            let centre = take(r, cells)?.into_iter().map(|c| c != 0).collect();
            let starts = pod::<u32>(r, cells.checked_add(1)?)?;
            let edges: Vec<(u32, u32)> = pod::<[u32; 2]>(r, ne)?.into_iter().map(|[a, b]| (a, b)).collect();
            if starts.last().copied() != Some(ne as u32)
                || starts.windows(2).any(|w| w[0] > w[1])
                || edges.iter().any(|&(ri, i)| rings.get(ri as usize).is_none_or(|ring| i as usize >= ring.len()))
            {
                return None;
            }
            let grid = Grid { x0, y0, cw, ch, nx, ny, centre, starts, edges };
            let tree = EdgeTree::build(&rings);
            shapes.push(Shape {
                source,
                tree: std::sync::Arc::new(tree),
                rings_digest: Default::default(),
                rings_print: Default::default(),
                rings: std::sync::Arc::new(rings),
                buffer_m,
                bbox,
                country,
                water,
                grid: std::sync::Arc::new(grid),
                memo: Default::default(),
            });
        }
    }
    let [shapes, depth_only] = lists;
    let mut past = [0u8; 1];
    matches!(r.read(&mut past), Ok(0)).then_some(Coverage { shapes, depth_only })
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
        fingerprint_of(&self.shapes, b)
    }

    /// Whether a way (its vertices) touches the coverage.
    pub fn touches(&self, verts: &[[i32; 2]]) -> bool {
        verts.iter().any(|&p| self.contains(p))
    }
}

/// Shapes `shapes` as they are inside the box w, s, e, n (E7), as `Coverage::fingerprint` reads a
/// coverage's: empty where none has anything in the box.
pub fn fingerprint_of(shapes: &[Shape], b: [i32; 4]) -> String {
    // (A region's water marked: which of the covered points are targets is the key's too.)
    let mut v: Vec<String> =
        shapes.iter().filter_map(|s| s.fingerprint(b).map(|f| if s.water { format!("w{f}") } else { f })).collect();
    v.sort();
    v.dedup();
    v.join(",")
}

// ---- the coverage a catalogue records ------------------------------------------------------------

/// A region as a catalogue records it (docs/formats.md, Catalogue `coverage`): its recipe when the
/// catalogue was made, and each outline entry's polygons simplified for drawing, as GeoJSON
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
    /// builds, by the records when the catalogue was made; on the map, and downloadable, as they come.
    /// None for a built region, and in catalogues made before this was recorded.
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
pub(crate) mod tests {
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

    /// The grid's one edge list with per-cell starts holds, cell by cell, the edges the list a
    /// cell held (lead memory, 25 Oct), in the same order: random rings, with and without a
    /// buffer, a shape of one cell and the empty shape.
    #[test]
    fn grid_cells_list_the_edges_a_list_a_cell_did() {
        let mut seed = 777u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        let mut cases: Vec<(Vec<Vec<[i32; 2]>>, f64)> = vec![(Vec::new(), 0.0), (vec![vec![e7(1.0, 1.0); 3]], 500.0)];
        for k in 0..12 {
            let rings: Vec<Vec<[i32; 2]>> = (0..1 + (rnd() * 4.0) as usize)
                .map(|_| {
                    let (cx, cy, r) = (rnd() * 20.0 - 10.0, rnd() * 120.0 - 60.0, 0.01 + rnd() * 3.0);
                    let n = 3 + (rnd() * 300.0) as usize;
                    (0..n)
                        .map(|i| {
                            let t = i as f64 / n as f64 * std::f64::consts::TAU;
                            let rr = r * (0.5 + rnd());
                            e7(cx + rr * t.dcos(), cy + rr * t.dsin())
                        })
                        .collect()
                })
                .collect();
            cases.push((rings, [0.0, 1000.0, OSM_BUFFER_M][k % 3]));
        }
        cases.push((shape_rings(), 1000.0));
        for (n, (rings, buffer_m)) in cases.into_iter().enumerate() {
            let s = Shape::new("t".into(), rings.clone(), buffer_m);
            let g = &s.grid;
            // (The old build, a list a cell, from the same cells per edge; the buffer in E7 units as Shape::new works it out.)
            let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for p in rings.iter().flatten() {
                bb = [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])];
            }
            let lat = (bb[1].unsigned_abs().max(bb[3].unsigned_abs()) as f64 * 1e-7).min(85.0);
            let be = ((buffer_m / (M_PER_E7 * lat.to_radians().dcos())).ceil() as i32)
                .max((buffer_m / M_PER_E7).ceil() as i32) as f64;
            assert_eq!(g.starts.len(), g.nx * g.ny + 1, "case {n}");
            assert_eq!(g.starts[0], 0, "case {n}");
            assert_eq!(*g.starts.last().unwrap() as usize, g.edges.len(), "case {n}");
            assert!(g.starts.windows(2).all(|w| w[0] <= w[1]), "case {n}");
            let mut old: Vec<Vec<(u32, u32)>> = vec![Vec::new(); g.nx * g.ny];
            for (ri, r) in rings.iter().enumerate() {
                for i in 0..r.len() {
                    let (a, b) = (r[i], r[(i + 1) % r.len()]);
                    if a == b {
                        continue;
                    }
                    let c = |v: f64, o: f64, w: f64, m: usize| (((v - o) / w).floor().max(0.0) as usize).min(m - 1);
                    let (cx0, cx1) = (
                        c(a[0].min(b[0]) as f64 - be, g.x0, g.cw, g.nx),
                        c(a[0].max(b[0]) as f64 + be, g.x0, g.cw, g.nx),
                    );
                    let (cy0, cy1) = (
                        c(a[1].min(b[1]) as f64 - be, g.y0, g.ch, g.ny),
                        c(a[1].max(b[1]) as f64 + be, g.y0, g.ch, g.ny),
                    );
                    for cy in cy0..=cy1 {
                        for cx in cx0..=cx1 {
                            old[cy * g.nx + cx].push((ri as u32, i as u32));
                        }
                    }
                }
            }
            for (c, e) in old.iter().enumerate() {
                assert_eq!(g.edges(c), &e[..], "case {n}, cell {c}");
            }
        }
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

    /// The edges meeting the box `rg` by the grid's cells, each edge of each cell read (how
    /// `edges_meeting` worked before the edge tree, task #894), each place once, sorted.
    fn edges_by_cells(s: &Shape, rg: [i64; 4]) -> Vec<(u32, u32)> {
        let g = &s.grid;
        let cell = |v: f64, v0: f64, size: f64, n: usize| (((v - v0) / size).floor().max(0.0) as usize).min(n - 1);
        let (cx0, cx1) = (cell(rg[0] as f64, g.x0, g.cw, g.nx), cell(rg[2] as f64, g.x0, g.cw, g.nx));
        let (cy0, cy1) = (cell(rg[1] as f64, g.y0, g.ch, g.ny), cell(rg[3] as f64, g.y0, g.ch, g.ny));
        let mut out = std::collections::BTreeSet::new();
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                for &(ri, i) in g.edges(cy * g.nx + cx) {
                    let ring = &s.rings[ri as usize];
                    let (a, b) = (ring[i as usize], ring[(i as usize + 1) % ring.len()]);
                    if segment_meets_box([a[0] as i64, a[1] as i64], [b[0] as i64, b[1] as i64], rg) {
                        out.insert((ri, i));
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    fn edges_by_tree(s: &Shape, rg: [i64; 4]) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        s.edges_meeting(rg, &mut |at, _, _| {
            out.push(at);
            false
        });
        let n = out.len();
        out.sort_unstable();
        out.dedup();
        assert_eq!(out.len(), n, "each edge once");
        out
    }

    /// Of `boxes`, each meets the same edges of `s` by the edge tree as by the grid's cells.
    pub(crate) fn tree_agrees(s: &Shape, boxes: impl Iterator<Item = [i64; 4]>) -> usize {
        let mut n = 0;
        for rg in boxes {
            assert_eq!(edges_by_tree(s, rg), edges_by_cells(s, rg), "{}: box {rg:?}", s.source);
            n += 1;
        }
        n
    }

    #[test]
    fn the_edge_tree_meets_the_edges_the_grid_cells_do() {
        // A jagged ring of many points (several runs, several levels), a hole, a ring of one point
        // twice, buffered and not; boxes of every size, inside, across and outside.
        let mut ring: Vec<[i32; 2]> = (0..2000)
            .map(|i| {
                let a = i as f64 / 2000.0 * std::f64::consts::TAU;
                let r = 2.0 + 0.3 * ((i * 7919) % 13) as f64 / 13.0;
                e7(10.0 + r * a.cos(), 50.0 + r * a.sin())
            })
            .collect();
        ring.push(ring[0]);
        let hole = vec![e7(10.0, 50.0), e7(10.5, 50.0), e7(10.5, 50.5), e7(10.0, 50.5)];
        let dup = vec![e7(9.0, 49.0), e7(9.0, 49.0), e7(9.1, 49.0)];
        let mut seed = 12345u64;
        let mut rnd = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        let mut boxes = Vec::new();
        for _ in 0..3000 {
            let (x, y) = (7.0 + 6.0 * rnd(), 47.0 + 6.0 * rnd());
            let size = 10f64.powf(-3.0 + 3.5 * rnd());
            let (a, b) = (e7(x, y), e7(x + size, y + size * rnd()));
            boxes.push([a[0] as i64, a[1] as i64, b[0] as i64, b[1] as i64]);
        }
        for buffer in [0.0, 1000.0] {
            let s = Shape::new("t".into(), vec![ring.clone(), hole.clone(), dup.clone()], buffer);
            assert_eq!(tree_agrees(&s, boxes.iter().copied()), boxes.len());
        }
        // No rings: nothing meets.
        assert!(edges_by_tree(&Shape::new("e".into(), vec![], 0.0), [0, 0, 10, 10]).is_empty());
    }

    #[test]
    fn a_coverage_keeps_its_answers_by_version() {
        let sq = |x: f64| vec![vec![e7(x, 0.0), e7(x + 1.0, 0.0), e7(x + 1.0, 1.0), e7(x, 1.0)]];
        let a = Coverage { shapes: vec![Shape::new("a: x".into(), sq(0.0), 0.0)], ..Default::default() };
        let same = Coverage { shapes: vec![Shape::new("b: y".into(), sq(0.0), 0.0)], ..Default::default() };
        let mut other = a.clone();
        other.shapes[0].water = true;
        let moved = Coverage { shapes: vec![Shape::new("a: x".into(), sq(2.0), 0.0)], ..Default::default() };
        assert_eq!(a.version(), same.version(), "the geometry decides, not the names");
        assert!(std::sync::Arc::ptr_eq(&a.kept(), &same.kept()));
        assert_ne!(a.version(), other.version());
        assert_ne!(a.version(), moved.version());
        // The same points split into other rings: another shape, another version.
        let split = |r: Vec<Vec<[i32; 2]>>| {
            let p = r.concat();
            vec![p[..2].to_vec(), p[2..].to_vec()]
        };
        let resplit = Coverage { shapes: vec![Shape::new("a: x".into(), split(sq(0.0)), 0.0)], ..Default::default() };
        assert_eq!(resplit.shapes[0].rings_digest(), a.shapes[0].rings_digest());
        assert_ne!(a.version(), resplit.version());
        assert!(!std::sync::Arc::ptr_eq(&a.kept(), &moved.kept()));
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
        // (The box its box misses told at once, not kept.)
        assert_eq!(kept.memo.lock().unwrap().fingerprints.len(), 3);
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
        let back = from_kept(&mut &kept_bytes(&c)[..]).unwrap();
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
        let made = Coverage { shapes, depth_only: Vec::new() };
        let c = from_kept(&mut &kept_bytes(&made)[..]).unwrap();
        // The grids read back are those made (task #807: kept, not made again).
        for (a, b) in made.shapes.iter().zip(&c.shapes) {
            assert!(a.grid == b.grid && a.bbox == b.bbox && a.rings == b.rings && a.source == b.source);
        }
        // Cut short anywhere, or an edge naming no ring's point: not read.
        let k = kept_bytes(&made);
        assert!(from_kept(&mut &k[..k.len() - 1]).is_none());
        let points: u64 = c.shapes.iter().flat_map(|s| s.rings.iter()).map(|r| r.len() as u64).sum();
        let entries: u64 = c.shapes.iter().map(|s| s.grid.edges.len() as u64).sum();
        let cells: u64 = c.shapes.iter().map(|s| (s.grid.nx * s.grid.ny) as u64).sum();
        // Held at once while it's read (as a stream): the rings, every grid, the memos.
        let held = points * 8 + cells * 5 + entries * 8 + MEMO_CAP as u64;
        let mb = c.read_peak_mb();
        assert!(mb << 20 >= held, "{mb} MB under {held} B");
        assert!(entries > points, "the coast's buffer lists its edges in several cells");
        assert!(mb > (MEMO_CAP >> 20) as u64 + 5, "{mb} MB: the coverage's own part counted");
    }
}
