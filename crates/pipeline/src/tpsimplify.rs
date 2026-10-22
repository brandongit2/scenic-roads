//! Douglas–Peucker simplification that keeps a geometry's topology: GEOS 3.13's
//! TopologyPreservingSimplifier (what shapely's `simplify(tol, preserve_topology=True)` runs),
//! ported step for step so the result is the same coordinates. A section of a line is flattened to
//! one segment only when its farthest point is within the tolerance and the new segment crosses no
//! segment of the input left nor of the output so far, and doesn't jump another component (one
//! component's point on the other side of it than of the section); a ring keeps at least 4 points,
//! and its end point goes too when it can. Lines are simplified in order, each against the others.
//!
//! The orientation tests are GEOS's (a floating-point filter, then double-double arithmetic). The
//! segment indexes answer "is there any": any index that finds every segment whose box meets the
//! query's gives the same answers as GEOS's quadtree.

use std::collections::HashMap;

type P = [f64; 2];

/// A line or ring to simplify (a polygon's rings: its shell, then its holes).
pub struct Line {
    pub pts: Vec<P>,
    /// A LinearRing (not a LineString).
    pub ring: bool,
}

/// The lines simplified together within `tol`, in order; each line's new points.
pub fn simplify(lines: &[Line], tol: f64) -> Vec<Vec<P>> {
    let mut s = Simplifier::new(lines);
    for l in 0..lines.len() {
        s.simplify_line(l, tol);
    }
    (0..lines.len()).map(|l| s.result_coords(l)).collect()
}

#[derive(Clone, Copy)]
struct Seg {
    p0: P,
    p1: P,
    /// Its line (`NONE` for a flattened one), and its index there.
    line: usize,
    index: usize,
}

const NONE: usize = usize::MAX;

struct Tagged {
    min_size: usize,
    ring: bool,
    segs: Vec<usize>,
    result: Vec<usize>,
}

struct Simplifier<'a> {
    lines: &'a [Line],
    tagged: Vec<Tagged>,
    segs: Vec<Seg>,
    input: Index,
    output: Index,
}

fn eq(a: P, b: P) -> bool {
    a[0] == b[0] && a[1] == b[1]
}

impl<'a> Simplifier<'a> {
    fn new(lines: &'a [Line]) -> Self {
        let mut segs = Vec::new();
        let mut tagged = Vec::new();
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (li, l) in lines.iter().enumerate() {
            let closed = l.pts.len() > 1 && eq(l.pts[0], l.pts[l.pts.len() - 1]);
            let mut ids = Vec::new();
            for i in 0..l.pts.len().saturating_sub(1) {
                ids.push(segs.len());
                segs.push(Seg { p0: l.pts[i], p1: l.pts[i + 1], line: li, index: i });
            }
            for p in &l.pts {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            tagged.push(Tagged { min_size: if closed { 4 } else { 2 }, ring: l.ring, segs: ids, result: Vec::new() });
        }
        let n = segs.len().max(1);
        let ext = if x1 > x0 || y1 > y0 { (x1 - x0).max(y1 - y0) } else { 1.0 };
        let cell = ext / (n as f64).sqrt().max(1.0);
        let mut s = Simplifier { lines, tagged, segs, input: Index::new(cell), output: Index::new(cell) };
        for id in 0..s.segs.len() {
            let sg = s.segs[id];
            s.input.add(id, &sg);
        }
        s
    }

    fn result_size(&self, l: usize) -> usize {
        let n = self.tagged[l].result.len();
        if n == 0 {
            0
        } else {
            n + 1
        }
    }

    fn result_coords(&self, l: usize) -> Vec<P> {
        let r = &self.tagged[l].result;
        let mut out: Vec<P> = r.iter().map(|&id| self.segs[id].p0).collect();
        if let Some(&last) = r.last() {
            out.push(self.segs[last].p1);
        }
        out
    }

    /// TaggedLineString::getComponentPoint.
    fn component_point(&self, l: usize) -> P {
        match self.tagged[l].result.first() {
            Some(&id) => self.segs[id].p0,
            None => self.lines[l].pts[1],
        }
    }

    fn new_seg(&mut self, s: Seg) -> usize {
        self.segs.push(s);
        self.segs.len() - 1
    }

    fn simplify_line(&mut self, l: usize, tol: f64) {
        let n = self.lines[l].pts.len();
        if n == 0 {
            return;
        }
        self.simplify_section(l, 0, n - 1, 0, tol);
        let pts = &self.lines[l].pts;
        let is_ring = n >= 4 && eq(pts[0], pts[n - 1]);
        if self.tagged[l].ring && is_ring {
            self.simplify_ring_endpoint(l, tol);
        }
    }

    fn simplify_section(&mut self, l: usize, i: usize, j: usize, depth: usize, tol: f64) {
        let depth = depth + 1;
        if i + 1 == j {
            let s = self.segs[self.tagged[l].segs[i]];
            let id = self.new_seg(s);
            self.tagged[l].result.push(id);
            return;
        }
        let mut valid = true;
        if self.result_size(l) < self.tagged[l].min_size && depth + 1 < self.tagged[l].min_size {
            valid = false;
        }
        let pts = &self.lines[l].pts;
        let (furthest, distance) = furthest_point(pts, i, j);
        if distance < 0.0 {
            for k in i..j {
                let s = self.segs[self.tagged[l].segs[k]];
                let id = self.new_seg(s);
                self.tagged[l].result.push(id);
            }
            return;
        }
        if distance > tol {
            valid = false;
        }
        if valid {
            let flat = (pts[i], pts[j]);
            valid = self.topology_valid_section(l, i, j, flat);
        }
        if valid {
            let id = self.new_seg(Seg { p0: pts[i], p1: pts[j], line: NONE, index: 0 });
            let s = self.segs[id];
            self.output.add(id, &s);
            for k in i..j {
                self.input.remove(self.tagged[l].segs[k]);
            }
            self.tagged[l].result.push(id);
            return;
        }
        self.simplify_section(l, i, furthest, depth, tol);
        self.simplify_section(l, furthest, j, depth, tol);
    }

    fn simplify_ring_endpoint(&mut self, l: usize, tol: f64) {
        if self.result_size(l) <= self.tagged[l].min_size {
            return;
        }
        let first = self.tagged[l].result[0];
        let last = *self.tagged[l].result.last().unwrap();
        let (f, la) = (self.segs[first], self.segs[last]);
        let simp = (la.p0, f.p1);
        let end = f.p0;
        if point_to_segment(end, simp.0, simp.1) <= tol && self.topology_valid_pair(l, &f, &la, simp) {
            self.input.remove(first);
            self.input.remove(last);
            self.output.remove(first);
            self.output.remove(last);
            self.segs[first].p0 = la.p0;
            self.tagged[l].result.pop();
            let s = self.segs[first];
            self.output.add(first, &s);
        }
    }

    fn topology_valid_section(&mut self, l: usize, start: usize, end: usize, flat: (P, P)) -> bool {
        if self.has_output_intersection(flat) {
            return false;
        }
        if self.has_input_intersection(Some((l, start, end)), flat) {
            return false;
        }
        !self.has_jump_section(l, start, end, flat)
    }

    fn topology_valid_pair(&mut self, l: usize, s1: &Seg, s2: &Seg, flat: (P, P)) -> bool {
        if orientation(flat.0, flat.1, s1.p0) == 0 {
            return true;
        }
        if self.has_output_intersection(flat) {
            return false;
        }
        if self.has_input_intersection(None, flat) {
            return false;
        }
        !self.has_jump_pair(l, s1, s2, flat)
    }

    fn has_output_intersection(&mut self, flat: (P, P)) -> bool {
        let segs = &self.segs;
        self.output.any(segs, flat, |s| invalid_intersection((s.p0, s.p1), flat))
    }

    fn has_input_intersection(&mut self, section: Option<(usize, usize, usize)>, flat: (P, P)) -> bool {
        let segs = &self.segs;
        self.input.any(segs, flat, |s| {
            if !invalid_intersection((s.p0, s.p1), flat) {
                return false;
            }
            match section {
                Some((l, start, end)) => !(s.line == l && s.index >= start && s.index < end),
                None => true,
            }
        })
    }

    fn has_jump_section(&self, l: usize, start: usize, end: usize, flat: (P, P)) -> bool {
        let pts = &self.lines[l].pts;
        let mut env = Env::empty();
        for p in &pts[start..=end] {
            env.add(*p);
        }
        for c in 0..self.tagged.len() {
            if c == l {
                continue;
            }
            let cp = self.component_point(c);
            if env.contains(cp) {
                let mut a = Crossings::new(cp);
                for i in start..end {
                    a.count(pts[i], pts[i + 1]);
                }
                let mut b = Crossings::new(cp);
                b.count(flat.0, flat.1);
                if a.n % 2 != b.n % 2 {
                    return true;
                }
            }
        }
        false
    }

    fn has_jump_pair(&self, l: usize, s1: &Seg, s2: &Seg, flat: (P, P)) -> bool {
        let mut env = Env::empty();
        for p in [s1.p0, s1.p1, s2.p0, s2.p1] {
            env.add(p);
        }
        for c in 0..self.tagged.len() {
            if c == l {
                continue;
            }
            let cp = self.component_point(c);
            if env.contains(cp) {
                let mut a = Crossings::new(cp);
                a.count(s1.p0, s1.p1);
                a.count(s2.p0, s2.p1);
                let mut b = Crossings::new(cp);
                b.count(flat.0, flat.1);
                if a.n % 2 != b.n % 2 {
                    return true;
                }
            }
        }
        false
    }
}

/// TaggedLineStringSimplifier::findFurthestPoint: the farthest point strictly between i and j from
/// the segment i–j (the first of equals), and its distance (−1 with none).
fn furthest_point(pts: &[P], i: usize, j: usize) -> (usize, f64) {
    let (mut max_d, mut max_i) = (-1.0, i);
    for (k, p) in pts.iter().enumerate().take(j).skip(i + 1) {
        let d = point_to_segment(*p, pts[i], pts[j]);
        if d > max_d {
            max_d = d;
            max_i = k;
        }
    }
    (max_i, max_d)
}

fn dist(a: P, b: P) -> f64 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// Distance::pointToSegment.
fn point_to_segment(p: P, a: P, b: P) -> f64 {
    if eq(a, b) {
        return dist(p, a);
    }
    let len2 = (b[0] - a[0]) * (b[0] - a[0]) + (b[1] - a[1]) * (b[1] - a[1]);
    let r = ((p[0] - a[0]) * (b[0] - a[0]) + (p[1] - a[1]) * (b[1] - a[1])) / len2;
    if r <= 0.0 {
        return dist(p, a);
    }
    if r >= 1.0 {
        return dist(p, b);
    }
    let s = ((a[1] - p[1]) * (b[0] - a[0]) - (a[0] - p[0]) * (b[1] - a[1])) / len2;
    s.abs() * len2.sqrt()
}

/// TaggedLineStringSimplifier::hasInvalidIntersection: the same segment, or LineIntersector's
/// intersection with a point inside either segment (not one of its end points).
fn invalid_intersection(s0: (P, P), s1: (P, P)) -> bool {
    let (p1, p2, q1, q2) = (s0.0, s0.1, s1.0, s1.1);
    if (eq(p1, q1) && eq(p2, q2)) || (eq(p1, q2) && eq(p2, q1)) {
        return true;
    }
    if !env_meets(p1, p2, q1, q2) {
        return false;
    }
    let pq1 = orientation(p1, p2, q1);
    let pq2 = orientation(p1, p2, q2);
    if (pq1 > 0 && pq2 > 0) || (pq1 < 0 && pq2 < 0) {
        return false;
    }
    let qp1 = orientation(q1, q2, p1);
    let qp2 = orientation(q1, q2, p2);
    if (qp1 > 0 && qp2 > 0) || (qp1 < 0 && qp2 < 0) {
        return false;
    }
    let interior = |pts: &[P]| pts.iter().any(|x| !(eq(*x, p1) || eq(*x, p2)) || !(eq(*x, q1) || eq(*x, q2)));
    if pq1 == 0 && pq2 == 0 && qp1 == 0 && qp2 == 0 {
        // computeCollinearIntersection.
        let q1in = env_has(p1, p2, q1);
        let q2in = env_has(p1, p2, q2);
        let p1in = env_has(q1, q2, p1);
        let p2in = env_has(q1, q2, p2);
        let pts: Vec<P> = if q1in && q2in {
            vec![q1, q2]
        } else if p1in && p2in {
            vec![p1, p2]
        } else if q1in && p1in {
            if eq(q1, p1) && !q2in && !p2in {
                vec![q1]
            } else {
                vec![q1, p1]
            }
        } else if q1in && p2in {
            if eq(q1, p2) && !q2in && !p1in {
                vec![q1]
            } else {
                vec![q1, p2]
            }
        } else if q2in && p1in {
            if eq(q2, p1) && !q1in && !p2in {
                vec![q2]
            } else {
                vec![q2, p1]
            }
        } else if q2in && p2in {
            if eq(q2, p2) && !q1in && !p1in {
                vec![q2]
            } else {
                vec![q2, p2]
            }
        } else {
            return false;
        };
        return interior(&pts);
    }
    if pq1 == 0 || pq2 == 0 || qp1 == 0 || qp2 == 0 {
        let p = if eq(p1, q1) || eq(p1, q2) {
            p1
        } else if eq(p2, q1) || eq(p2, q2) {
            p2
        } else if pq1 == 0 {
            q1
        } else if pq2 == 0 {
            q2
        } else if qp1 == 0 {
            p1
        } else {
            p2
        };
        return interior(&[p]);
    }
    // A proper crossing: its point (wherever it's computed) can't be an end point of both segments,
    // as no end point is shared (every orientation is non-zero).
    true
}

/// Envelope::intersects(p1, p2, q1, q2).
fn env_meets(p1: P, p2: P, q1: P, q2: P) -> bool {
    for d in 0..2 {
        let (minq, maxq) = (q1[d].min(q2[d]), q1[d].max(q2[d]));
        let (minp, maxp) = (p1[d].min(p2[d]), p1[d].max(p2[d]));
        if minp > maxq || maxp < minq {
            return false;
        }
    }
    true
}

/// Envelope::intersects(p1, p2, q): q within the box of p1, p2.
fn env_has(p1: P, p2: P, q: P) -> bool {
    (0..2).all(|d| q[d] >= p1[d].min(p2[d]) && q[d] <= p1[d].max(p2[d]))
}

struct Env {
    min: P,
    max: P,
}

impl Env {
    fn empty() -> Env {
        Env { min: [f64::INFINITY; 2], max: [f64::NEG_INFINITY; 2] }
    }

    fn add(&mut self, p: P) {
        for d in 0..2 {
            self.min[d] = self.min[d].min(p[d]);
            self.max[d] = self.max[d].max(p[d]);
        }
    }

    fn contains(&self, p: P) -> bool {
        (0..2).all(|d| p[d] >= self.min[d] && p[d] <= self.max[d])
    }
}

/// RayCrossingCounter's count of crossings by a ray from `pt` to the right.
struct Crossings {
    pt: P,
    n: usize,
    on: bool,
}

impl Crossings {
    fn new(pt: P) -> Self {
        Crossings { pt, n: 0, on: false }
    }

    fn count(&mut self, p1: P, p2: P) {
        let pt = self.pt;
        if p1[0] < pt[0] && p2[0] < pt[0] {
            return;
        }
        if pt[0] == p2[0] && pt[1] == p2[1] {
            self.on = true;
            return;
        }
        if p1[1] == pt[1] && p2[1] == pt[1] {
            let (minx, maxx) = if p1[0] > p2[0] { (p2[0], p1[0]) } else { (p1[0], p2[0]) };
            if pt[0] >= minx && pt[0] <= maxx {
                self.on = true;
            }
            return;
        }
        if (p1[1] > pt[1] && p2[1] <= pt[1]) || (p2[1] > pt[1] && p1[1] <= pt[1]) {
            let mut sign = orientation(p1, p2, pt);
            if sign == 0 {
                self.on = true;
                return;
            }
            if p2[1] < p1[1] {
                sign = -sign;
            }
            if sign > 0 {
                self.n += 1;
            }
        }
    }
}

/// CGAlgorithmsDD::orientationIndex: 1 left, −1 right, 0 on the line.
pub fn orientation(p1: P, p2: P, q: P) -> i32 {
    let (pax, pay, pbx, pby, pcx, pcy) = (p1[0], p1[1], p2[0], p2[1], q[0], q[1]);
    let sign = |d: f64| {
        if d < 0.0 {
            -1
        } else if d > 0.0 {
            1
        } else {
            0
        }
    };
    let detleft = (pax - pcx) * (pby - pcy);
    let detright = (pay - pcy) * (pbx - pcx);
    let det = detleft - detright;
    let detsum = if detleft > 0.0 {
        if detright <= 0.0 {
            return sign(det);
        }
        detleft + detright
    } else if detleft < 0.0 {
        if detright >= 0.0 {
            return sign(det);
        }
        -detleft - detright
    } else {
        return sign(det);
    };
    let errbound = 1e-15 * detsum;
    if det >= errbound || -det >= errbound {
        return sign(det);
    }
    let dx1 = Dd::of(p2[0]).add(Dd::of(-p1[0]));
    let dy1 = Dd::of(p2[1]).add(Dd::of(-p1[1]));
    let dx2 = Dd::of(q[0]).add(Dd::of(-p2[0]));
    let dy2 = Dd::of(q[1]).add(Dd::of(-p2[1]));
    let d = dx1.mul(dy2).add(dy1.mul(dx2).neg());
    if d.hi < 0.0 || (d.hi == 0.0 && d.lo < 0.0) {
        -1
    } else if d.hi > 0.0 || (d.hi == 0.0 && d.lo > 0.0) {
        1
    } else {
        0
    }
}

/// GEOS's double-double (geos::math::DD), the operations the orientation test uses.
#[derive(Clone, Copy)]
struct Dd {
    hi: f64,
    lo: f64,
}

const SPLIT: f64 = 134217729.0;

impl Dd {
    fn of(x: f64) -> Dd {
        Dd { hi: x, lo: 0.0 }
    }

    fn neg(self) -> Dd {
        Dd { hi: -self.hi, lo: -self.lo }
    }

    /// DD::selfAdd(yhi, ylo).
    fn add(self, y: Dd) -> Dd {
        let (hi, lo, yhi, ylo) = (self.hi, self.lo, y.hi, y.lo);
        let s_ = hi + yhi;
        let t_ = lo + ylo;
        let mut e = s_ - hi;
        let f = t_ - lo;
        let mut s = s_ - e;
        let mut t = t_ - f;
        s = (yhi - e) + (hi - s);
        t = (ylo - f) + (lo - t);
        e = s + t_;
        let h_ = s_ + e;
        let h = e + (s_ - h_);
        e = t + h;
        let zhi = h_ + e;
        let zlo = e + (h_ - zhi);
        Dd { hi: zhi, lo: zlo }
    }

    /// DD::selfMultiply(yhi, ylo).
    fn mul(self, y: Dd) -> Dd {
        let (hi, lo, yhi, ylo) = (self.hi, self.lo, y.hi, y.lo);
        let mut c_ = SPLIT * hi;
        let mut hx = c_ - hi;
        let mut c = SPLIT * yhi;
        hx = c_ - hx;
        let tx = hi - hx;
        let mut hy = c - yhi;
        c_ = hi * yhi;
        hy = c - hy;
        let ty = yhi - hy;
        c = ((((hx * hy - c_) + hx * ty) + tx * hy) + tx * ty) + (hi * ylo + lo * yhi);
        let zhi = c_ + c;
        hx = c_ - zhi;
        let zlo = c + hx;
        Dd { hi: zhi, lo: zlo }
    }
}

/// Segments by box, for "is there any meeting this one": a grid per size class, each segment in
/// the cell of its box's lower corner at the smallest class whose cells are at least its size.
struct Index {
    cell: f64,
    cells: HashMap<(u8, i64, i64), Vec<usize>>,
    /// Each class's segments (for a query that would look at more cells than that).
    by_level: Vec<Vec<usize>>,
    /// Whether each segment is in the index (removed ones stay in their cells).
    has: Vec<bool>,
    /// The query a segment was last looked at by (to look at it once).
    seen: Vec<u32>,
    query: u32,
    levels: u8,
}

impl Index {
    fn new(cell: f64) -> Self {
        Index {
            cell,
            cells: HashMap::new(),
            by_level: Vec::new(),
            has: Vec::new(),
            seen: Vec::new(),
            query: 0,
            levels: 0,
        }
    }

    fn grow(&mut self, id: usize) {
        if self.has.len() <= id {
            self.has.resize(id + 1, false);
            self.seen.resize(id + 1, 0);
        }
    }

    fn add(&mut self, id: usize, s: &Seg) {
        self.grow(id);
        self.has[id] = true;
        let size = (s.p0[0] - s.p1[0]).abs().max((s.p0[1] - s.p1[1]).abs());
        let mut level = 0u8;
        while self.cell * f64::powi(2.0, level as i32) < size && level < 60 {
            level += 1;
        }
        self.levels = self.levels.max(level + 1);
        if self.by_level.len() <= level as usize {
            self.by_level.resize(level as usize + 1, Vec::new());
        }
        self.by_level[level as usize].push(id);
        let c = self.cell * f64::powi(2.0, level as i32);
        let key = (level, (s.p0[0].min(s.p1[0]) / c).floor() as i64, (s.p0[1].min(s.p1[1]) / c).floor() as i64);
        self.cells.entry(key).or_default().push(id);
    }

    fn remove(&mut self, id: usize) {
        if id < self.has.len() {
            self.has[id] = false;
        }
    }

    /// Whether `f` holds for any segment in the index whose box meets the query's.
    fn any(&mut self, segs: &[Seg], q: (P, P), mut f: impl FnMut(&Seg) -> bool) -> bool {
        self.query += 1;
        let (query, has, seen) = (self.query, &self.has, &mut self.seen);
        let mut look = |id: usize| {
            if !has[id] || seen[id] == query {
                return false;
            }
            seen[id] = query;
            let s = &segs[id];
            env_meets(s.p0, s.p1, q.0, q.1) && f(s)
        };
        let (qx0, qx1) = (q.0[0].min(q.1[0]), q.0[0].max(q.1[0]));
        let (qy0, qy1) = (q.0[1].min(q.1[1]), q.0[1].max(q.1[1]));
        for level in 0..self.levels {
            let c = self.cell * f64::powi(2.0, level as i32);
            // A segment of this class meeting the box has its lower corner within c below it.
            let (cx0, cx1) = (((qx0 - c) / c).floor() as i64, (qx1 / c).floor() as i64);
            let (cy0, cy1) = (((qy0 - c) / c).floor() as i64, (qy1 / c).floor() as i64);
            let all = &self.by_level[level as usize];
            if (cx1 - cx0 + 1) as f64 * (cy1 - cy0 + 1) as f64 > all.len() as f64 {
                if all.iter().any(|&id| look(id)) {
                    return true;
                }
                continue;
            }
            for cx in cx0..=cx1 {
                for cy in cy0..=cy1 {
                    if self.cells.get(&(level, cx, cy)).is_some_and(|ids| ids.iter().any(|&id| look(id))) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(pts: &[[f64; 2]]) -> Line {
        Line { pts: pts.to_vec(), ring: true }
    }

    #[test]
    fn flattens_within_tolerance_and_keeps_a_ring_of_four() {
        let sq = ring(&[[0.0, 0.0], [5.0, 0.001], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]]);
        let r = simplify(&[sq], 0.01);
        assert_eq!(r[0], vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]]);
        let tri = ring(&[[0.0, 0.0], [10.0, 0.0], [5.0, 0.001], [0.0, 0.0]]);
        assert_eq!(simplify(&[tri], 1.0)[0].len(), 4);
    }

    #[test]
    fn a_hole_is_not_jumped() {
        // The shell's dent would cut past the hole's point if flattened.
        let shell = ring(&[[0.0, 0.0], [5.0, 0.5], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]]);
        let hole = ring(&[[4.9, 0.2], [5.1, 0.2], [5.0, 0.3], [4.9, 0.2]]);
        let r = simplify(&[shell, hole], 1.0);
        assert!(r[0].contains(&[5.0, 0.5]), "{:?}", r[0]);
    }

    #[test]
    fn orientation_signs() {
        assert_eq!(orientation([0.0, 0.0], [1.0, 0.0], [0.5, 1.0]), 1);
        assert_eq!(orientation([0.0, 0.0], [1.0, 0.0], [0.5, -1.0]), -1);
        assert_eq!(orientation([0.0, 0.0], [1.0, 1.0], [0.5, 0.5]), 0);
        // Past the filter: decided in double-double.
        assert_eq!(orientation([1e15, 1e15], [1e15 + 2.0, 1e15 + 2.0], [1e15 + 4.0, 1e15 + 4.0]), 0);
    }
}
