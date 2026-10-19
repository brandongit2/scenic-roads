//! The regions' geometry (docs/inputs.md §6.1.4): areas as polygons of E7 integer points, their set
//! operations (`i_overlay`'s 64-bit integer engine, single-threaded, no snapping past a unit), the
//! canonical form equal geometry is stored in, buffers measured on the ground, cuts by a line, and the
//! antimeridian: a region's parts put in one longitude frame before its operations (`Frame`), the
//! result split at ±180° and put back into −180°…180° (`Frame::split`).
//!
//! Coordinates are E7 degrees in `i64`: a frame shifts longitudes by whole turns (3.6e9), past what an
//! `i32` holds, and the engine's 64-bit range (2^62) leaves room for any of it.

use det::Det;
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{Overlay, ShapeType};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::{Precision, Solver, Strategy};
use i_overlay::i_float::int::point::IntPoint;

/// A point: longitude, latitude (E7), in some frame.
pub type Pt = [i64; 2];
/// A ring: its points, unclosed (the last joins the first).
pub type Ring = Vec<Pt>;

/// A whole turn of longitude (E7).
pub const TURN: i64 = 3_600_000_000;
/// Half a turn: the antimeridian.
pub const HALF: i64 = 1_800_000_000;

/// An area: polygons, each an outer ring (counter-clockwise) and its holes (clockwise), in canonical
/// form once made by an operation (`Area::canonical`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Area {
    pub polys: Vec<Vec<Ring>>,
}

fn solver() -> Solver {
    // (No snapping past a unit, and one thread: the same input gives the same output everywhere.)
    Solver { strategy: Strategy::Auto, precision: Precision::ABSOLUTE, multithreading: None }
}

fn ip(p: Pt) -> IntPoint<i64> {
    IntPoint::new(p[0], p[1])
}

/// Twice the signed area of a ring (positive: counter-clockwise).
pub fn area2(r: &[Pt]) -> i128 {
    let n = r.len();
    (0..n)
        .map(|i| {
            let (a, b) = (r[i], r[(i + 1) % n]);
            a[0] as i128 * b[1] as i128 - b[0] as i128 * a[1] as i128
        })
        .sum()
}

fn cross(o: Pt, a: Pt, b: Pt) -> i128 {
    (a[0] - o[0]) as i128 * (b[1] - o[1]) as i128 - (a[1] - o[1]) as i128 * (b[0] - o[0]) as i128
}

/// A ring in canonical form: repeated and collinear points removed, oriented (`ccw`), starting at its
/// smallest point (longitude, then latitude). None when nothing of it is left (under three points).
pub fn canonical_ring(r: &[Pt], ccw: bool) -> Option<Ring> {
    let mut v: Ring = Vec::with_capacity(r.len());
    for &p in r {
        if v.last() != Some(&p) {
            v.push(p);
        }
    }
    while v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    // Collinear points (and spikes) out, until none is left.
    loop {
        let n = v.len();
        if n < 3 {
            return None;
        }
        let keep: Vec<bool> = (0..n).map(|i| cross(v[(i + n - 1) % n], v[i], v[(i + 1) % n]) != 0).collect();
        if keep.iter().all(|&k| k) {
            break;
        }
        v = v.iter().zip(&keep).filter(|(_, &k)| k).map(|(p, _)| *p).collect();
    }
    if (area2(&v) > 0) != ccw {
        v.reverse();
    }
    let start = (0..v.len()).min_by_key(|&i| (v[i][0], v[i][1])).unwrap_or(0);
    v.rotate_left(start);
    Some(v)
}

impl Area {
    /// The area of rings read under the even–odd rule (an outline's rings, a `.poly`'s, a GeoJSON
    /// polygon's), made canonical.
    pub fn from_rings(rings: &[Ring]) -> Area {
        seen(rings.iter().map(Vec::len).sum());
        let mut o = Overlay::<i64>::new_custom(rings.iter().map(Vec::len).sum(), Default::default(), solver());
        for r in rings {
            let c: Vec<IntPoint<i64>> = r.iter().map(|&p| ip(p)).collect();
            o.add_contour(&c, ShapeType::Subject);
        }
        Area::of(o.overlay(OverlayRule::Subject, FillRule::EvenOdd))
    }

    fn of(shapes: Vec<Vec<Vec<IntPoint<i64>>>>) -> Area {
        let polys = shapes
            .into_iter()
            .map(|s| s.into_iter().map(|c| c.into_iter().map(|p| [p.x, p.y]).collect()).collect())
            .collect();
        Area { polys }.canonical()
    }

    /// Canonical: each outer ring counter-clockwise, its holes clockwise, every ring as
    /// `canonical_ring` has it, holes sorted by their first point, polygons by their outer's.
    pub fn canonical(self) -> Area {
        let mut polys: Vec<Vec<Ring>> = Vec::new();
        for p in self.polys {
            let mut it = p.into_iter();
            let Some(outer) = it.next().and_then(|o| canonical_ring(&o, true)) else { continue };
            let mut holes: Vec<Ring> = it.filter_map(|h| canonical_ring(&h, false)).collect();
            holes.sort();
            let mut poly = vec![outer];
            poly.extend(holes);
            polys.push(poly);
        }
        polys.sort();
        Area { polys }
    }

    pub fn is_empty(&self) -> bool {
        self.polys.is_empty()
    }

    /// Every ring, outer then holes, polygon by polygon: what a `Shape` reads (even–odd).
    pub fn rings(&self) -> Vec<Ring> {
        self.polys.iter().flatten().cloned().collect()
    }

    /// West, south, east, north; None when empty.
    pub fn bbox(&self) -> Option<[i64; 4]> {
        bbox_of(self.polys.iter().flatten().flatten())
    }

    fn op(&self, other: &Area, rule: OverlayRule) -> Area {
        let n = self.polys.iter().chain(&other.polys).flatten().map(Vec::len).sum();
        seen(n);
        let mut o = Overlay::<i64>::new_custom(n, Default::default(), solver());
        for (a, t) in [(self, ShapeType::Subject), (other, ShapeType::Clip)] {
            for r in a.polys.iter().flatten() {
                let c: Vec<IntPoint<i64>> = r.iter().map(|&p| ip(p)).collect();
                o.add_contour(&c, t);
            }
        }
        Area::of(o.overlay(rule, FillRule::NonZero))
    }

    pub fn union(&self, other: &Area) -> Area {
        self.op(other, OverlayRule::Union)
    }

    pub fn minus(&self, other: &Area) -> Area {
        self.op(other, OverlayRule::Difference)
    }

    pub fn intersect(&self, other: &Area) -> Area {
        self.op(other, OverlayRule::Intersect)
    }

    /// Shifted by `dx` in longitude.
    pub fn shifted(&self, dx: i64) -> Area {
        Area {
            polys: self
                .polys
                .iter()
                .map(|p| p.iter().map(|r| r.iter().map(|q| [q[0] + dx, q[1]]).collect()).collect())
                .collect(),
        }
    }

    /// Whether `p` is inside (even–odd over every ring; a point on an edge counts as inside).
    pub fn contains(&self, p: Pt) -> bool {
        let rings: Vec<&Ring> = self.polys.iter().flatten().collect();
        if rings.iter().any(|r| on_ring(r, p)) {
            return true;
        }
        inside_evenodd(&rings, p)
    }
}

/// The most points any one overlay was given since `reset_peak` (what bounds an operation's memory:
/// i_overlay's working set grows with its input).
static PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn seen(n: usize) {
    PEAK.fetch_max(n, std::sync::atomic::Ordering::Relaxed);
}

/// `PEAK` read and set back to nothing.
pub fn reset_peak() -> usize {
    PEAK.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// An area's points.
pub fn points(a: &Area) -> usize {
    a.polys.iter().flatten().map(Vec::len).sum()
}

/// `a` with `mask` (the dataset's area in a box, as `mask(box)` gives it) kept (`keep`) or taken
/// away, in pieces small enough that no one operation holds much: while the piece of `a` or the
/// mask has more than `MASK_POINTS` points and the box is wider than a sixteenth of a degree, the box
/// is split in four and each quarter done on its own. The pieces are put back together at the end.
pub fn with_mask(a: &Area, mask: &(dyn Fn([i64; 4]) -> Area + Sync), keep: bool) -> Area {
    let Some(b) = a.bbox() else { return Area::default() };
    let mut pieces = Vec::new();
    mask_rec(a, b, mask, keep, &mut pieces);
    union_all(&pieces)
}

/// Areas kept cut into 2° cells (each area halved until its pieces lie in one cell): what lies in a
/// box is then read from the cells it meets, not clipped from whole areas.
pub struct Tiled {
    cells: std::collections::HashMap<(i64, i64), Vec<Area>>,
}

const TILED_CELL: i64 = 20_000_000;

impl Tiled {
    pub fn of(areas: &[Area]) -> Tiled {
        fn rec(a: &Area, out: &mut std::collections::HashMap<(i64, i64), Vec<Area>>) {
            let Some(b) = a.bbox() else { return };
            let (cx0, cx1) = (b[0].div_euclid(TILED_CELL), (b[2] - 1).div_euclid(TILED_CELL));
            let (cy0, cy1) = (b[1].div_euclid(TILED_CELL), (b[3] - 1).div_euclid(TILED_CELL));
            if cx0 >= cx1 && cy0 >= cy1 {
                out.entry((cx0, cy0)).or_default().push(a.clone());
                return;
            }
            // (Halved at a cell line along its longer side.)
            if cx1 - cx0 >= cy1 - cy0 {
                let m = ((cx0 + cx1 + 1) / 2) * TILED_CELL;
                rec(&a.intersect(&box_area([b[0], b[1], m, b[3]])), out);
                rec(&a.intersect(&box_area([m, b[1], b[2], b[3]])), out);
            } else {
                let m = ((cy0 + cy1 + 1) / 2) * TILED_CELL;
                rec(&a.intersect(&box_area([b[0], b[1], b[2], m])), out);
                rec(&a.intersect(&box_area([b[0], m, b[2], b[3]])), out);
            }
        }
        use rayon::prelude::*;
        let each: Vec<std::collections::HashMap<(i64, i64), Vec<Area>>> = areas
            .par_iter()
            .map(|a| {
                let mut m = std::collections::HashMap::new();
                rec(a, &mut m);
                m
            })
            .collect();
        let mut cells: std::collections::HashMap<(i64, i64), Vec<Area>> = std::collections::HashMap::new();
        for m in each {
            for (k, v) in m {
                cells.entry(k).or_default().extend(v);
            }
        }
        Tiled { cells }
    }

    /// What lies in box `b`, clipped to it.
    pub fn query(&self, b: [i64; 4]) -> Area {
        let bb = box_area(b);
        let mut v = Vec::new();
        for x in b[0].div_euclid(TILED_CELL)..=b[2].div_euclid(TILED_CELL) {
            for y in b[1].div_euclid(TILED_CELL)..=b[3].div_euclid(TILED_CELL) {
                for a in self.cells.get(&(x, y)).into_iter().flatten() {
                    if a.bbox().is_some_and(|ab| ab[0] <= b[2] && b[0] <= ab[2] && ab[1] <= b[3] && b[1] <= ab[3]) {
                        v.push(a.intersect(&bb));
                    }
                }
            }
        }
        union_all(&v)
    }
}

/// The most points a piece is masked with at once.
pub const MASK_POINTS: usize = 200_000;

fn mask_rec(a: &Area, b: [i64; 4], mask: &(dyn Fn([i64; 4]) -> Area + Sync), keep: bool, out: &mut Vec<Area>) {
    if a.is_empty() {
        return;
    }
    let small = b[2] - b[0] <= 625_000 && b[3] - b[1] <= 625_000;
    // (A mask is only asked for a box of 2° or less: a wider one may hold millions of points.)
    let narrow = b[2] - b[0] <= 20_000_000 && b[3] - b[1] <= 20_000_000;
    if (points(a) <= MASK_POINTS && narrow) || small {
        let m = mask(b);
        if points(&m) <= MASK_POINTS || small {
            let m = m.intersect(&box_area(b));
            out.push(if keep { a.intersect(&m) } else { a.minus(&m) });
            return;
        }
    }
    let (mx, my) = ((b[0] + b[2]) / 2, (b[1] + b[3]) / 2);
    for q in [[b[0], b[1], mx, my], [mx, b[1], b[2], my], [b[0], my, mx, b[3]], [mx, my, b[2], b[3]]] {
        let piece = a.intersect(&box_area(q));
        mask_rec(&piece, q, mask, keep, out);
    }
}

/// The union of `areas` (canonical).
pub fn union_all(areas: &[Area]) -> Area {
    let n = areas.iter().flat_map(|a| a.polys.iter().flatten()).map(Vec::len).sum();
    seen(n);
    let mut o = Overlay::<i64>::new_custom(n, Default::default(), solver());
    for a in areas {
        for r in a.polys.iter().flatten() {
            let c: Vec<IntPoint<i64>> = r.iter().map(|&p| ip(p)).collect();
            o.add_contour(&c, ShapeType::Subject);
        }
    }
    Area::of(o.overlay(OverlayRule::Subject, FillRule::NonZero))
}

/// The union of contours each counter-clockwise or clockwise as it comes (discs, quads): made one
/// direction first, so the non-zero rule takes their union.
fn union_contours(cs: Vec<Ring>) -> Area {
    let n = cs.iter().map(Vec::len).sum();
    let mut o = Overlay::<i64>::new_custom(n, Default::default(), solver());
    for mut c in cs {
        if area2(&c) < 0 {
            c.reverse();
        }
        let c: Vec<IntPoint<i64>> = c.iter().map(|&p| ip(p)).collect();
        o.add_contour(&c, ShapeType::Subject);
    }
    Area::of(o.overlay(OverlayRule::Subject, FillRule::NonZero))
}

pub fn bbox_of<'a>(pts: impl Iterator<Item = &'a Pt>) -> Option<[i64; 4]> {
    let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for p in pts {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    (b[0] <= b[2]).then_some(b)
}

fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    cross(a, b, p) == 0
        && p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

fn on_ring(r: &[Pt], p: Pt) -> bool {
    (0..r.len()).any(|i| on_segment(r[i], r[(i + 1) % r.len()], p))
}

/// Even–odd point in rings (a point on an edge: either answer).
pub fn inside_evenodd(rings: &[&Ring], p: Pt) -> bool {
    let mut ins = false;
    for r in rings {
        let n = r.len();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                // x of the crossing compared exactly: p.x < a.x + (p.y - a.y)(b.x - a.x)/(b.y - a.y).
                let lhs = (p[0] - a[0]) as i128 * (b[1] - a[1]) as i128;
                let rhs = (p[1] - a[1]) as i128 * (b[0] - a[0]) as i128;
                if (b[1] > a[1] && lhs < rhs) || (b[1] < a[1] && lhs > rhs) {
                    ins = !ins;
                }
            }
        }
    }
    ins
}

/// Whether segments a–b and c–d meet (touching included).
pub fn segments_meet(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    let (d1, d2, d3, d4) =
        (cross(c, d, a).signum(), cross(c, d, b).signum(), cross(a, b, c).signum(), cross(a, b, d).signum());
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return true;
    }
    (d1 == 0 && on_segment(c, d, a))
        || (d2 == 0 && on_segment(c, d, b))
        || (d3 == 0 && on_segment(a, b, c))
        || (d4 == 0 && on_segment(a, b, d))
}

/// Whether segments a–b and c–d cross properly or overlap (more than meeting at a shared end).
fn segments_cross_inner(a: Pt, b: Pt, c: Pt, d: Pt) -> bool {
    if !segments_meet(a, b, c, d) {
        return false;
    }
    let shared = [a, b].iter().filter(|p| **p == c || **p == d).count();
    if shared == 0 {
        return true;
    }
    // Sharing an end: crossing only if they overlap along a line.
    cross(a, b, c) == 0 && cross(a, b, d) == 0 && {
        let other = if a == c || b == c { d } else { c };
        let mine = if a == c || a == d { b } else { a };
        let s = if a == c || a == d { a } else { b };
        // (Overlap when the two other ends lie on the same side of the shared one.)
        ((other[0] - s[0]) as i128 * (mine[0] - s[0]) as i128 + (other[1] - s[1]) as i128 * (mine[1] - s[1]) as i128)
            > 0
    }
}

/// The first pair of edges of a polyline (`closed`: a ring) that cross or overlap, beyond neighbours
/// meeting at their shared point: (index of the first edge, index of the second). A sweep over the
/// edges by their west ends, so a ring of many points is checked quickly.
pub fn self_crossing(pts: &[Pt], closed: bool) -> Option<(usize, usize)> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let m = if closed { n } else { n - 1 };
    let seg = |i: usize| (pts[i], pts[(i + 1) % n]);
    let mut order: Vec<usize> = (0..m).collect();
    order.sort_by_key(|&i| {
        let (a, b) = seg(i);
        a[0].min(b[0])
    });
    let mut active: Vec<usize> = Vec::new();
    for &i in &order {
        let (a, b) = seg(i);
        let x0 = a[0].min(b[0]);
        active.retain(|&j| {
            let (c, d) = seg(j);
            c[0].max(d[0]) >= x0
        });
        for &j in &active {
            let (c, d) = seg(j);
            let neighbours = (i + 1) % n == j || (j + 1) % n == i;
            let hit = if neighbours { segments_cross_inner(a, b, c, d) } else { segments_meet(a, b, c, d) };
            // (Neighbours of a two-edge closed ring share both ends: not a crossing.)
            if hit && !(closed && m == 2) {
                return Some((i.min(j), i.max(j)));
            }
        }
        active.push(i);
    }
    None
}

// ---- The antimeridian ---------------------------------------------------------------------------

/// A ring or line unwrapped: each step the short way round (two points more than half a turn apart in
/// longitude cross the antimeridian, so the later ones are shifted by a whole turn).
pub fn unwrap(pts: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::with_capacity(pts.len());
    let mut shift = 0i64;
    for (i, &p) in pts.iter().enumerate() {
        if i > 0 {
            let d = p[0] + shift - out[i - 1][0];
            if d > HALF {
                shift -= TURN;
            } else if d < -HALF {
                shift += TURN;
            }
        }
        out.push([p[0] + shift, p[1]]);
    }
    out
}

/// A region's longitude frame: `lo`…`lo + TURN`, every part shifted by whole turns into it, chosen so
/// the band no part covers is the widest outside it; the plain frame (−180°…180°) when ±180° lies in
/// that band, so a region nowhere near the line moves nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub lo: i64,
}

impl Frame {
    pub const PLAIN: Frame = Frame { lo: -HALF };

    /// The frame for parts covering these longitude spans (west, east; unwrapped, so east may pass
    /// 180°; a span of a whole turn or more covers everything).
    pub fn for_spans(spans: &[[i64; 2]]) -> Frame {
        if spans.is_empty() || spans.iter().any(|s| s[1] - s[0] >= TURN) {
            return Frame::PLAIN;
        }
        // Each span on the circle, from its west end (0…TURN), sorted; the widest gap between one's
        // east end and the next's west end, going round.
        let mut iv: Vec<[i64; 2]> = spans
            .iter()
            .map(|s| {
                let w = (s[0] + HALF).rem_euclid(TURN);
                [w, w + (s[1] - s[0])]
            })
            .collect();
        iv.sort();
        // Merged, then the gaps between (and round from the last to the first).
        let mut merged: Vec<[i64; 2]> = Vec::new();
        for s in iv {
            match merged.last_mut() {
                Some(m) if s[0] <= m[1] => m[1] = m[1].max(s[1]),
                _ => merged.push(s),
            }
        }
        // (A span running past a whole turn from the first's start wraps onto it.)
        let first = merged[0][0];
        let mut best: Option<(i64, i64)> = None; // (width, middle)
        for (k, m) in merged.iter().enumerate() {
            let next = if k + 1 < merged.len() { merged[k + 1][0] } else { first + TURN };
            let gap = next - m[1];
            if gap > 0 && best.is_none_or(|(w, _)| gap > w) {
                best = Some((gap, m[1] + gap / 2));
            }
        }
        let Some((_, mid)) = best else { return Frame::PLAIN };
        // Back from the circle's 0…TURN (0 is −180°) to longitude; the plain frame when ±180° is in
        // the widest gap.
        let gap_has_180 = merged.iter().enumerate().any(|(k, m)| {
            let next = if k + 1 < merged.len() { merged[k + 1][0] } else { first + TURN };
            let mid_k = m[1] + (next - m[1]) / 2;
            mid_k == mid && (m[1] <= TURN && next >= TURN || m[1] <= 0 && next >= 0)
        });
        if gap_has_180 {
            return Frame::PLAIN;
        }
        Frame { lo: (mid.rem_euclid(TURN)) - HALF }
    }

    /// A ring or line (unwrapped) shifted by whole turns so its west end is in the frame.
    pub fn place(&self, pts: &[Pt]) -> Vec<Pt> {
        let Some(w) = pts.iter().map(|p| p[0]).min() else { return Vec::new() };
        let k = (w - self.lo).div_euclid(TURN);
        pts.iter().map(|p| [p[0] - k * TURN, p[1]]).collect()
    }

    /// An area of the frame split at every antimeridian it crosses, each piece shifted back into
    /// −180°…180°: (the turn it was shifted by, the piece), west of the line first (the piece with
    /// longitudes up to 180° first). One piece, unshifted, when it crosses none.
    pub fn split(area: &Area) -> Vec<(i64, Area)> {
        let Some(b) = area.bbox() else { return Vec::new() };
        // The bands −180°…180° shifted by k turns that the area meets.
        let k0 = (b[0] + HALF).div_euclid(TURN);
        let k1 = (b[2] + HALF - 1).div_euclid(TURN).max(k0);
        if k0 == k1 {
            let k = k0;
            return vec![(k, if k == 0 { area.clone() } else { area.shifted(-k * TURN) })];
        }
        let mut out = Vec::new();
        for k in k0..=k1 {
            let (w, e) = (-HALF + k * TURN, HALF + k * TURN);
            let band = Area {
                polys: vec![vec![vec![[w, -910_000_000], [e, -910_000_000], [e, 910_000_000], [w, 910_000_000]]]],
            };
            let piece = area.intersect(&band);
            if !piece.is_empty() {
                out.push((k, piece.shifted(-k * TURN)));
            }
        }
        out
    }
}

// ---- Distances on the ground ----------------------------------------------------------------------

const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_223_563;

/// The radii of curvature at latitude `lat` (radians): along the meridian, and of the parallel (the
/// prime vertical's times the latitude's cosine), metres.
fn radii(lat: f64) -> (f64, f64) {
    let e2 = F * (2.0 - F);
    let s = lat.dsin();
    let w = (1.0 - e2 * s * s).sqrt();
    (A * (1.0 - e2) / (w * w * w), A / w * lat.dcos())
}

/// The point `d` metres from `p` (E7, any frame) at bearing `b` (radians, clockwise from north), by
/// the local radii at the step's middle latitude: within a few parts in ten thousand of the
/// geodesic distance up to 200 km.
pub fn destination(p: Pt, b: f64, d: f64) -> Pt {
    let (lon, lat) = (p[0] as f64 * 1e-7, (p[1] as f64 * 1e-7).to_radians());
    let (dn, de) = (d * b.dcos(), d * b.dsin());
    let mut mid = lat;
    let mut dlat = 0.0;
    let mut dlon = 0.0;
    for _ in 0..3 {
        let (m, n) = radii(mid);
        dlat = dn / m;
        dlon = de / n.max(1.0);
        mid = (lat + dlat / 2.0).clamp(-1.5707, 1.5707);
    }
    let lat2 = (lat + dlat).to_degrees().clamp(-89.99, 89.99);
    [((lon + dlon.to_degrees()) * 1e7).round() as i64, (lat2 * 1e7).round() as i64]
}

/// The bearing (radians, clockwise from north) from `a` to `b` (E7, one frame), in the plane of the
/// local radii at their middle.
pub fn bearing(a: Pt, b: Pt) -> f64 {
    let mid = ((a[1] + b[1]) as f64 * 0.5e-7).to_radians();
    let (m, n) = radii(mid);
    let dn = (b[1] - a[1]) as f64 * 1e-7 * std::f64::consts::PI / 180.0 * m;
    let de = (b[0] - a[0]) as f64 * 1e-7 * std::f64::consts::PI / 180.0 * n;
    de.datan2(dn)
}

/// The length (metres) of a–b in the local radii at their middle.
pub fn length_m(a: Pt, b: Pt) -> f64 {
    let mid = ((a[1] + b[1]) as f64 * 0.5e-7).to_radians();
    let (m, n) = radii(mid);
    let dn = (b[1] - a[1]) as f64 * 1e-7 * std::f64::consts::PI / 180.0 * m;
    let de = (b[0] - a[0]) as f64 * 1e-7 * std::f64::consts::PI / 180.0 * n;
    (dn * dn + de * de).sqrt()
}

/// The most a buffer may be: 200 nautical miles (§6.1.3).
pub const MAX_BUFFER_M: f64 = 370_400.0;
/// Past this radius, a buffer's rings are first simplified within a thousandth of it (a ring's
/// detail under that changes nothing a buffer that wide shows): up to it, exactly as before.
const SIMPLIFY_PAST_M: f64 = 200_000.0;
/// The longest edge buffered as it is: longer ones are split first.
const DENSIFY_M: f64 = 10_000.0;
/// How far a round join's or end's chords may stray inside the true arc.
const SAGITTA_M: f64 = 5.0;

/// The sides a circle of radius `r` is drawn with, so its chords stay within `SAGITTA_M` of it.
pub fn circle_sides(r: f64) -> usize {
    if r <= SAGITTA_M * 2.0 {
        return 8;
    }
    let n = (std::f64::consts::PI / (1.0 - SAGITTA_M / r).dacos()).ceil() as usize;
    n.clamp(8, 4096)
}

/// A circle of `n` sides round `p`, radius `r` metres.
fn disc(p: Pt, r: f64, n: usize) -> Ring {
    (0..n).map(|i| destination(p, i as f64 / n as f64 * std::f64::consts::TAU, r)).collect()
}

/// A polyline with no step longer than `DENSIFY_M` (points added along straight lines in the frame).
fn densify(pts: &[Pt], closed: bool) -> Vec<Pt> {
    let n = pts.len();
    let m = if closed { n } else { n.saturating_sub(1) };
    let mut out = Vec::with_capacity(n);
    for i in 0..m {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        out.push(a);
        let k = (length_m(a, b) / DENSIFY_M).ceil() as i64;
        for j in 1..k {
            out.push([a[0] + (b[0] - a[0]) * j / k, a[1] + (b[1] - a[1]) * j / k]);
        }
    }
    if !closed {
        if let Some(&l) = pts.last() {
            out.push(l);
        }
    }
    out
}

/// The discs and quads whose union is everything within `r` of the polyline (`closed`: a ring).
fn sweep(pts: &[Pt], closed: bool, r: f64, out: &mut Vec<Ring>) {
    let pts = densify(pts, closed);
    let n = pts.len();
    if n == 0 {
        return;
    }
    // (Past `SIMPLIFY_PAST_M`, chords within a thousandth of the radius, as the rings are.)
    let sides = if r > SIMPLIFY_PAST_M {
        ((std::f64::consts::PI / (1.0 - 0.001f64).dacos()).ceil() as usize).max(8)
    } else {
        circle_sides(r)
    };
    for &p in &pts {
        out.push(disc(p, r, sides));
    }
    let m = if closed { n } else { n - 1 };
    for i in 0..m {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        if a == b {
            continue;
        }
        let (ba, bb) = (bearing(a, b), bearing(a, b));
        let q = std::f64::consts::FRAC_PI_2;
        out.push(vec![
            destination(a, ba - q, r),
            destination(b, bb - q, r),
            destination(b, bb + q, r),
            destination(a, ba + q, r),
        ]);
    }
}

/// Everything within `r` metres of the area, its inside included, to within a fiftieth of `r`: the
/// buffer a region with water takes (§6.1.3). Each ring is first simplified within `r / 50`, and a
/// round join drawn only where the ring turns outward (left, as canonical rings run: outer rings
/// counter-clockwise, holes clockwise), so a detailed coast doesn't hand the union thousands of
/// discs over each other.
pub fn buffer_area_coarse(a: &Area, r: f64) -> Area {
    let mut cs = Vec::new();
    let a = a.clone().canonical();
    let sides = if r > SIMPLIFY_PAST_M { 71 } else { circle_sides(r).min(256) };
    for ring in a.polys.iter().flatten() {
        let pts = densify(&simplify_ring(ring, (r / 50.0).max(1.0)), true);
        let n = pts.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (o, p, q) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            if cross(o, p, q) >= 0 {
                cs.push(disc(p, r, sides));
            }
            if p != q {
                let b = bearing(p, q);
                let h = std::f64::consts::FRAC_PI_2;
                cs.push(vec![destination(p, b - h, r), destination(q, b - h, r), q, p]);
                cs.push(vec![p, q, destination(q, b + h, r), destination(p, b + h, r)]);
            }
        }
    }
    a.union(&union_contours(cs))
}

/// Everything within `r` metres of the area, its inside included.
pub fn buffer_area(a: &Area, r: f64) -> Area {
    let mut cs = Vec::new();
    for ring in a.polys.iter().flatten() {
        if r > SIMPLIFY_PAST_M {
            sweep(&simplify_ring(ring, r / 1000.0), true, r, &mut cs);
        } else {
            sweep(ring, true, r, &mut cs);
        }
    }
    a.union(&union_contours(cs))
}

/// A ring simplified (Douglas–Peucker, in metres by the local radii) within `tol` metres.
pub fn simplify_ring(r: &[Pt], tol: f64) -> Ring {
    if r.len() < 4 {
        return r.to_vec();
    }
    let mut pts: Vec<Pt> = r.to_vec();
    pts.push(r[0]);
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    *keep.last_mut().unwrap() = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((i, j)) = stack.pop() {
        if j <= i + 1 {
            continue;
        }
        let (a, b) = (pts[i], pts[j]);
        let mut best = (0.0, i);
        for (k, &p) in pts.iter().enumerate().take(j).skip(i + 1) {
            let d = seg_dist_m(p, a, b);
            if d > best.0 {
                best = (d, k);
            }
        }
        if best.0 > tol {
            keep[best.1] = true;
            stack.push((i, best.1));
            stack.push((best.1, j));
        }
    }
    let mut out: Ring = pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
    out.pop();
    if out.len() < 3 {
        return r.to_vec();
    }
    out
}

/// The distance (metres, local plane at `p`) from `p` to the segment a–b.
pub fn seg_dist_m(p: Pt, a: Pt, b: Pt) -> f64 {
    let lat = (p[1] as f64 * 1e-7).to_radians();
    let (m, n) = radii(lat);
    let k = std::f64::consts::PI / 180.0 * 1e-7;
    let xy = |q: Pt| [(q[0] - p[0]) as f64 * k * n, (q[1] - p[1]) as f64 * k * m];
    let (a, b) = (xy(a), xy(b));
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 { (-(a[0] * d[0] + a[1] * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let c = [a[0] + t * d[0], a[1] + t * d[1]];
    (c[0] * c[0] + c[1] * c[1]).sqrt()
}

/// A box (west, south, east, north; E7) as an area.
pub fn box_area(b: [i64; 4]) -> Area {
    Area::from_rings(&[vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]])
}

/// The whole Earth in the plain frame.
pub fn earth() -> Area {
    box_area([-HALF, -900_000_000, HALF, 900_000_000])
}

/// The authalic sphere's radius (WGS 84), metres.
const R_AUTHALIC: f64 = 6_371_007.2;

/// An area's size on the ground, km², on the authalic sphere (edges taken as straight in
/// longitude and latitude).
pub fn area_km2(a: &Area) -> f64 {
    let k = std::f64::consts::PI / 180.0 * 1e-7;
    let mut t = 0.0;
    for ring in a.polys.iter().flatten() {
        let n = ring.len();
        let mut s = 0.0;
        for i in 0..n {
            let (p, q) = (ring[i], ring[(i + 1) % n]);
            // (On the authalic sphere: the band from the equator under the edge.)
            let (s1, s2) = ((p[1] as f64 * k).dsin(), (q[1] as f64 * k).dsin());
            s += (q[0] - p[0]) as f64 * k * (s1 + s2) * 0.5 * R_AUTHALIC * R_AUTHALIC;
        }
        t -= s;
    }
    t / 1e6
}

/// The points of `within` nearer `to` than `than` (each the nearest of their rings' points, those
/// densified to `step` apart), decided cell by cell on a grid of `step` metres: the result is
/// `within` cut along a staircase that keeps within a cell of the true median line. Where `than` is
/// empty, all of `within`; where `to` is, none.
pub fn nearest(within: &Area, to: &Area, than: &Area, step: f64) -> Area {
    if within.is_empty() || to.is_empty() {
        return Area::default();
    }
    if than.is_empty() {
        return within.clone();
    }
    let Some(bb) = within.bbox() else { return Area::default() };
    // Samples near enough to matter: within 8° of the area's box.
    let reach = 80_000_000i64;
    let near =
        |q: &Pt| q[0] >= bb[0] - reach && q[0] <= bb[2] + reach && q[1] >= bb[1] - reach && q[1] <= bb[3] + reach;
    let mut samples: Vec<(Pt, bool)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let deg = step / 111_320.0 * 1e7;
    for (a, mine) in [(to, true), (than, false)] {
        for ring in a.polys.iter().flatten() {
            if !ring.iter().any(near) {
                continue;
            }
            for q in densify_to(ring, step) {
                if near(&q) && seen.insert(((q[0] as f64 / deg) as i64, (q[1] as f64 / deg) as i64, mine)) {
                    samples.push((q, mine));
                }
            }
        }
    }
    if !samples.iter().any(|s| !s.1) {
        return within.clone();
    }
    if !samples.iter().any(|s| s.1) {
        return Area::default();
    }
    // A bucket grid of the samples, half-degree cells.
    let cell = 5_000_000i64;
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    for (k, (q, _)) in samples.iter().enumerate() {
        grid.entry((q[0].div_euclid(cell), q[1].div_euclid(cell))).or_default().push(k);
    }
    let dist = |a: Pt, b: Pt| -> f64 { length_m(a, b) };
    let nearest_is_mine = |p: Pt| -> bool {
        let (cx, cy) = (p[0].div_euclid(cell), p[1].div_euclid(cell));
        // (Ties to the smaller sample point, so the split and its complement agree.)
        let mut best = (f64::MAX, [i64::MAX, i64::MAX], true);
        // (The least cosine of the band searched: a degree of longitude is shortest there.)
        let cos_at =
            |lat_e7: i64| ((lat_e7.clamp(-900_000_000, 900_000_000) as f64) * 1e-7).to_radians().dcos().max(0.02);
        let mut r = 0i64;
        loop {
            for gx in cx - r..=cx + r {
                for gy in cy - r..=cy + r {
                    if (gx - cx).abs() != r && (gy - cy).abs() != r {
                        continue;
                    }
                    if let Some(v) = grid.get(&(gx, gy)) {
                        for &k in v {
                            let d = dist(p, samples[k].0);
                            let q = samples[k].0;
                            if d < best.0 || (d == best.0 && q < best.1) {
                                best = (d, q, samples[k].1);
                            }
                        }
                    }
                }
            }
            // (Every sample past ring r is at least r half-degree cells away: ~55 km of latitude a
            // cell, less of longitude by the latitude's cosine.)
            let reach = (r + 1) * cell;
            let coslat = cos_at(p[1] - reach).min(cos_at(p[1] + reach));
            if best.0 < (r as f64) * 55_000.0 * coslat * 0.95 || r > 400 {
                return best.2;
            }
            r += 1;
        }
    };
    // Rows of `step`, each split into runs of cells whose middles are nearer `to`. The grid is
    // anchored at 0°, 0° (rows and, in each row, cells in whole steps from it), so splits of
    // neighbouring areas fall on the same cells and meet without gaps or overlaps.
    let dy = (step / 111_320.0 * 1e7).max(1.0) as i64;
    let mut rects: Vec<Ring> = Vec::new();
    let mut y = bb[1].div_euclid(dy) * dy;
    while y < bb[3] {
        let y1 = y + dy;
        let mid = (y + y1) / 2;
        let coslat = ((mid as f64) * 1e-7).to_radians().dcos().max(0.01);
        let dx = ((step / (111_320.0 * coslat)) * 1e7).max(1.0) as i64;
        let mut x = bb[0].div_euclid(dx) * dx;
        let mut run: Option<i64> = None;
        while x < bb[2] {
            let x1 = x + dx;
            let mine = nearest_is_mine([(x + x1) / 2, mid]);
            match (mine, run) {
                (true, None) => run = Some(x),
                (false, Some(x0)) => {
                    rects.push(vec![[x0, y], [x, y], [x, y1], [x0, y1]]);
                    run = None;
                }
                _ => {}
            }
            x = x1;
        }
        if let Some(x0) = run {
            rects.push(vec![[x0, y], [x, y], [x, y1], [x0, y1]]);
        }
        y = y1;
    }
    within.intersect(&union_contours(rects))
}

/// A ring's points with none more than `step` metres apart.
fn densify_to(r: &[Pt], step: f64) -> Vec<Pt> {
    let n = r.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let (a, b) = (r[i], r[(i + 1) % n]);
        out.push(a);
        let k = (length_m(a, b) / step).ceil() as i64;
        for j in 1..k {
            out.push([a[0] + (b[0] - a[0]) * j / k, a[1] + (b[1] - a[1]) * j / k]);
        }
    }
    out
}

/// The length (km, roughly) of the edge two areas share: the area of `b` within `w` metres of `a`'s
/// edge, over `w`.
pub fn shared_edge_km(a: &Area, b: &Area, w: f64) -> f64 {
    let (Some(x), Some(y)) = (a.bbox(), b.bbox()) else { return 0.0 };
    let pad = 2_000_000;
    if x[0] > y[2] + pad || y[0] > x[2] + pad || x[1] > y[3] + pad || y[1] > x[3] + pad {
        return 0.0;
    }
    // (Where they meet, in quarters while either is large: no one buffer of a long coast.)
    let m = [x[0].max(y[0]) - pad, x[1].max(y[1]) - pad, x[2].min(y[2]) + pad, x[3].min(y[3]) + pad];
    edge_rec(a, b, m, w)
}

fn edge_rec(a: &Area, b: &Area, q: [i64; 4], w: f64) -> f64 {
    let qa = box_area(q);
    let (a, b) = (a.intersect(&qa), b.intersect(&qa));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if points(&a) + points(&b) > MASK_POINTS / 4 && q[2] - q[0] > 625_000 {
        let (mx, my) = ((q[0] + q[2]) / 2, (q[1] + q[3]) / 2);
        return [[q[0], q[1], mx, my], [mx, q[1], q[2], my], [q[0], my, mx, q[3]], [mx, my, q[2], q[3]]]
            .iter()
            .map(|s| edge_rec(&a, &b, *s, w))
            .sum();
    }
    // (The quarter's own edges aren't the areas': only what lies inside it, less w, counts.)
    let inner = box_area([q[0] + 1, q[1] + 1, q[2] - 1, q[3] - 1]);
    let ring = buffer_area_coarse(&a, w).minus(&a).intersect(&inner);
    area_km2(&ring.intersect(&b)) / (w / 1000.0)
}

/// Everything within `r` metres of a line.
pub fn buffer_line(line: &[Pt], r: f64) -> Area {
    let mut cs = Vec::new();
    sweep(line, false, r, &mut cs);
    union_contours(cs)
}

// ---- Cuts ---------------------------------------------------------------------------------------

/// Why a cut can't be made.
#[derive(Clone, Debug, PartialEq)]
pub enum CutError {
    /// The line ends inside a piece it meets (the end, the piece's first point).
    EndsInside(Pt),
}

/// The side of `line` (drawn first point to last, extended straight from both ends) on the left
/// (`left`) or right, as a polygon over the box `b` (which holds the line and the shape, grown).
fn side(line: &[Pt], b: [i64; 4], left: bool) -> Ring {
    let n = line.len();
    let exit = |p: Pt, q: Pt| -> Pt {
        // From p along p − q to the box's edge.
        let d = [(p[0] - q[0]) as f64, (p[1] - q[1]) as f64];
        let mut t = f64::INFINITY;
        for (axis, (lo, hi)) in [(0usize, (b[0], b[2])), (1, (b[1], b[3]))] {
            if d[axis] > 0.0 {
                t = t.min((hi - p[axis]) as f64 / d[axis]);
            } else if d[axis] < 0.0 {
                t = t.min((lo - p[axis]) as f64 / d[axis]);
            }
        }
        let e = [(p[0] as f64 + t * d[0]).round() as i64, (p[1] as f64 + t * d[1]).round() as i64];
        [e[0].clamp(b[0], b[2]), e[1].clamp(b[1], b[3])]
    };
    let e0 = exit(line[0], line[1]);
    let en = exit(line[n - 1], line[n - 2]);
    // Places round the box's edge, counter-clockwise from its south-west corner: 0…4.
    let w = (b[2] - b[0]) as f64;
    let h = (b[3] - b[1]) as f64;
    let pos = |p: Pt| -> f64 {
        if p[1] == b[1] {
            (p[0] - b[0]) as f64 / w
        } else if p[0] == b[2] {
            1.0 + (p[1] - b[1]) as f64 / h
        } else if p[1] == b[3] {
            2.0 + (b[2] - p[0]) as f64 / w
        } else {
            3.0 + (b[3] - p[1]) as f64 / h
        }
    };
    let corners = [(1.0, [b[2], b[1]]), (2.0, [b[2], b[3]]), (3.0, [b[0], b[3]]), (4.0, [b[0], b[1]])];
    let mut ring: Ring = vec![e0];
    ring.extend_from_slice(line);
    ring.push(en);
    let (a, z) = (pos(en), pos(e0));
    if left {
        // Counter-clockwise from en round to e0.
        let z = if z <= a { z + 4.0 } else { z };
        for k in 0..8 {
            let (cp, c) = corners[k % 4];
            let cp = cp + if k >= 4 { 4.0 } else { 0.0 };
            if cp > a && cp < z {
                ring.push(c);
            }
        }
    } else {
        // Clockwise from en round to e0.
        let z = if z >= a { z - 4.0 } else { z };
        for k in 0..8 {
            let (cp, c) = corners[3 - (k % 4)];
            let cp = cp - if k >= 4 { 4.0 } else { 0.0 };
            if cp < a && cp > z {
                ring.push(c);
            }
        }
    }
    ring
}

/// Area `a` cut by `line` (at least two points, not crossing itself), the side `left` (or right)
/// kept (§6.1.3): each piece the line meets split and its side kept, a piece it ends inside an error,
/// a piece it doesn't meet kept whole when it lies on that side.
pub fn cut(a: &Area, line: &[Pt], left: bool) -> Result<Area, CutError> {
    let Some(ab) = a.bbox() else { return Ok(Area::default()) };
    let lb = bbox_of(line.iter()).expect("a line has points");
    let span = (ab[2].max(lb[2]) - ab[0].min(lb[0])).max(ab[3].max(lb[3]) - ab[1].min(lb[1])).max(1000);
    let b = [ab[0].min(lb[0]) - span, ab[1].min(lb[1]) - span, ab[2].max(lb[2]) + span, ab[3].max(lb[3]) + span];
    let keep = Area::from_rings(&[side(line, b, left)]);
    let mut out: Vec<Area> = Vec::new();
    for poly in &a.polys {
        let piece = Area { polys: vec![poly.clone()] };
        let rings: Vec<&Ring> = poly.iter().collect();
        let met = line
            .windows(2)
            .any(|s| rings.iter().any(|r| (0..r.len()).any(|i| segments_meet(s[0], s[1], r[i], r[(i + 1) % r.len()]))))
            || inside_evenodd(&rings, line[0]);
        if !met {
            // Kept whole when on the side kept: judged by its first point not on the line's side's edge.
            let p = poly[0].iter().copied().find(|&p| !on_ring(&keep.rings().concat(), p)).unwrap_or(poly[0][0]);
            if keep.contains(p) {
                out.push(piece);
            }
            continue;
        }
        for end in [line[0], line[line.len() - 1]] {
            if inside_evenodd(&rings, end) && !rings.iter().any(|r| on_ring(r, end)) {
                return Err(CutError::EndsInside(poly[0][0]));
            }
        }
        out.push(piece.intersect(&keep));
    }
    Ok(union_all(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(x0: i64, y0: i64, s: i64) -> Ring {
        vec![[x0, y0], [x0 + s, y0], [x0 + s, y0 + s], [x0, y0 + s]]
    }

    #[test]
    fn rings_are_made_canonical() {
        let r = vec![[10, 10], [10, 0], [10, 0], [5, 0], [0, 0], [0, 10]];
        assert_eq!(canonical_ring(&r, true), Some(vec![[0, 0], [10, 0], [10, 10], [0, 10]]));
        assert_eq!(canonical_ring(&r, false), Some(vec![[0, 0], [0, 10], [10, 10], [10, 0]]));
        assert_eq!(canonical_ring(&[[0, 0], [1, 1], [2, 2]], true), None);
    }

    #[test]
    fn two_touching_squares_union_into_one() {
        let a = Area::from_rings(&[sq(0, 0, 100)]);
        let b = Area::from_rings(&[sq(100, 0, 100)]);
        let u = a.union(&b);
        assert_eq!(u.polys, vec![vec![vec![[0, 0], [200, 0], [200, 100], [0, 100]]]]);
        // The same input, the same output, twice; and in the other order.
        assert_eq!(u, a.union(&b));
        assert_eq!(u, b.union(&a));
        assert_eq!(u, union_all(&[b.clone(), a.clone()]));
    }

    #[test]
    fn minus_and_intersect() {
        let a = Area::from_rings(&[sq(0, 0, 100)]);
        let b = Area::from_rings(&[sq(50, 50, 100)]);
        assert_eq!(a.intersect(&b).polys, vec![vec![sq(50, 50, 50)]]);
        let m = a.minus(&b);
        assert_eq!(m.polys, vec![vec![vec![[0, 0], [100, 0], [100, 50], [50, 50], [50, 100], [0, 100]]]]);
        // A hole: a small square taken from the middle.
        let h = a.minus(&Area::from_rings(&[sq(40, 40, 20)]));
        assert_eq!(h.polys.len(), 1);
        assert_eq!(h.polys[0].len(), 2);
        assert!(area2(&h.polys[0][1]) < 0, "holes clockwise");
        assert!(!h.contains([50, 50]) && h.contains([10, 10]));
        assert!(a.minus(&a).is_empty());
    }

    #[test]
    fn self_crossings_are_found() {
        assert_eq!(self_crossing(&sq(0, 0, 10), true), None);
        let bow = vec![[0, 0], [10, 10], [10, 0], [0, 10]];
        assert!(self_crossing(&bow, true).is_some());
        assert_eq!(self_crossing(&[[0, 0], [10, 0], [10, 10]], false), None);
        assert!(self_crossing(&[[0, 0], [10, 0], [10, 10], [5, -5]], false).is_some());
        // Doubling back over itself.
        assert!(self_crossing(&[[0, 0], [10, 0], [5, 0]], false).is_some());
    }

    /// The geodesic distance (m) on WGS 84, Vincenty's inverse: the tests' measure of the buffers.
    pub(crate) fn vincenty(a: Pt, b: Pt) -> f64 {
        let (l1, p1, l2, p2) = (
            (a[0] as f64 * 1e-7).to_radians(),
            (a[1] as f64 * 1e-7).to_radians(),
            (b[0] as f64 * 1e-7).to_radians(),
            (b[1] as f64 * 1e-7).to_radians(),
        );
        let bb = A * (1.0 - F);
        let (u1, u2) = (((1.0 - F) * p1.dtan()).datan(), ((1.0 - F) * p2.dtan()).datan());
        let l = l2 - l1;
        let mut lam = l;
        let (mut s_sig, mut c_sig, mut sig, mut c2a, mut c2sm);
        loop {
            let (sl, cl) = (lam.dsin(), lam.dcos());
            s_sig = ((u2.dcos() * sl).powi(2) + (u1.dcos() * u2.dsin() - u1.dsin() * u2.dcos() * cl).powi(2)).sqrt();
            c_sig = u1.dsin() * u2.dsin() + u1.dcos() * u2.dcos() * cl;
            sig = s_sig.datan2(c_sig);
            let sa = u1.dcos() * u2.dcos() * sl / s_sig;
            c2a = 1.0 - sa * sa;
            c2sm = if c2a == 0.0 { 0.0 } else { c_sig - 2.0 * u1.dsin() * u2.dsin() / c2a };
            let c = F / 16.0 * c2a * (4.0 + F * (4.0 - 3.0 * c2a));
            let prev = lam;
            lam = l + (1.0 - c) * F * sa * (sig + c * s_sig * (c2sm + c * c_sig * (-1.0 + 2.0 * c2sm * c2sm)));
            if (lam - prev).abs() < 1e-13 {
                break;
            }
        }
        let u2b = c2a * (A * A - bb * bb) / (bb * bb);
        let aa = 1.0 + u2b / 16384.0 * (4096.0 + u2b * (-768.0 + u2b * (320.0 - 175.0 * u2b)));
        let bk = u2b / 1024.0 * (256.0 + u2b * (-128.0 + u2b * (74.0 - 47.0 * u2b)));
        let ds = bk
            * s_sig
            * (c2sm
                + bk / 4.0
                    * (c_sig * (-1.0 + 2.0 * c2sm * c2sm)
                        - bk / 6.0 * c2sm * (-3.0 + 4.0 * s_sig * s_sig) * (-3.0 + 4.0 * c2sm * c2sm)));
        bb * aa * (sig - ds)
    }

    #[test]
    fn a_12_nmi_buffer_at_60_n_is_12_nmi_on_the_ground() {
        let r = 12.0 * 1852.0;
        let p = [250_000_000, 600_000_000];
        let a = buffer_line(&[p], r);
        // Every vertex of the outline is within 0.1 % of the distance from the line (the disc's are
        // at it), and points 0.2 % inside and outside the distance are in and out.
        for v in a.polys[0][0].iter() {
            let d = vincenty(p, *v);
            assert!((d / r - 1.0).abs() < 0.001, "{v:?} at {d}");
        }
        for b in [0.0f64, 0.7, 1.9, 3.3, 4.4, 5.5] {
            let inside = destination(p, b + std::f64::consts::PI, r * 0.998);
            let outside = destination(p, b + std::f64::consts::PI, r * 1.002);
            assert!(a.contains(inside), "{b}: {inside:?} in");
            assert!(!a.contains(outside), "{b}: {outside:?} out");
            assert!((vincenty(p, inside) / (r * 0.998) - 1.0).abs() < 0.001);
        }
    }

    #[test]
    fn frames_put_parts_astride_the_line_together() {
        // Nowhere near it: the plain frame.
        assert_eq!(Frame::for_spans(&[[-90_000_000, 40_000_000]]), Frame::PLAIN);
        // Fiji: rings from 177° E to 180°, and from −180° to −178°.
        let f = Frame::for_spans(&[[1_770_000_000, 1_800_000_000], [-1_800_000_000, -1_780_000_000]]);
        assert_ne!(f, Frame::PLAIN);
        let east = f.place(&[[-1_800_000_000, 0], [-1_780_000_000, 0]]);
        let west = f.place(&[[1_770_000_000, 0], [1_800_000_000, 0]]);
        assert_eq!(east[0][0], west[1][0], "the two halves meet at 180° in the frame");
        // A ring drawn across the line, unwrapped the short way.
        assert_eq!(unwrap(&[[1_790_000_000, 0], [-1_790_000_000, 0]]), vec![[1_790_000_000, 0], [1_810_000_000, 0]]);
        // A ring astride, split back into two, each within ±180°.
        let a = Area::from_rings(&[sq(1_790_000_000, 0, 20_000_000)]);
        let parts = Frame::split(&a);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].1.bbox().unwrap(), [1_790_000_000, 0, 1_800_000_000, 20_000_000]);
        assert_eq!(parts[1].1.bbox().unwrap(), [-1_800_000_000, 0, -1_790_000_000, 20_000_000]);
    }

    #[test]
    fn cuts_keep_a_side() {
        let a = Area::from_rings(&[sq(0, 0, 100)]);
        // West to east across, past both sides: left is north.
        let line = vec![[-50, 50], [150, 50]];
        assert_eq!(cut(&a, &line, true).unwrap().polys, vec![vec![vec![[0, 50], [100, 50], [100, 100], [0, 100]]]]);
        assert_eq!(cut(&a, &line, false).unwrap().polys, vec![vec![vec![[0, 0], [100, 0], [100, 50], [0, 50]]]]);
        // A line ending inside: an error.
        assert!(matches!(cut(&a, &[[-50, 50], [50, 50]], true), Err(CutError::EndsInside(_))));
        // A piece the line misses: kept whole on its side, dropped on the other.
        let two = Area::from_rings(&[sq(0, 0, 100), sq(300, 200, 50)]);
        let kept = cut(&two, &line, true).unwrap();
        assert_eq!(kept.polys.len(), 2, "the square north of the line's extension kept");
        let other = cut(&two, &line, false).unwrap();
        assert_eq!(other.polys.len(), 1);
    }
}
