//! Each arc simplified once per zoom (docs/steps.md §15, the simplification).
//!
//! In Web Mercator (the plane the map draws in), each arc's points get their Douglas–Peucker
//! importance once, its ends fixed (a ring meeting no other: its first point, the point farthest
//! from it, and the farthest from the chord on each side, so a ring kept is never under four
//! points); a zoom keeps the points at least its tolerance (`tolerance`, half a pixel) from the
//! chord. Every ring and border using an arc uses it as simplified, so neighbours' fills meet and
//! their shared border is one line at every zoom.
//!
//! Then, per zoom, no two arcs may cross or touch other than at the nodes they share, nor one cross
//! itself: each simplified segment is put in a grid of cells an eighth of the zoom's tile across
//! (a cell holding more than 64 split in 8 × 8, again as needed),
//! and a segment skipping points that meets another gets back its most important point skipped,
//! until none does (a segment of the full geometry is never at fault; one crossing another in the
//! geometry itself, as overlapping regions' borders do, is left). So no zoom shows a gap or an
//! overlap the geometry doesn't have, and no ring collapses.
//!
//! A ring meeting no other arc (an island, a lake, an enclave's border: one closed arc) smaller
//! than `DROP_PX` across at a zoom is left out there, for both its sides at once (its inside's
//! region drawn no more, its outside's filling it), unless it is the largest ring of a region every
//! ring of which is such a ring (a small island region stays on the map as a quadrilateral at
//! least). The finest zoom keeps every point.

use super::topo::Topology;
use super::{tolerance, DROP_PX, MAXZ};
use rayon::prelude::*;

/// An arc in the plane, simplified: its points in Web Mercator (x east, y down, 0–1 across the
/// world; past ±85.05° y runs on beyond 0 and 1), each point's importance (squared distance in
/// world units; infinite for those always kept), and the zooms keeping each point (bit z).
#[derive(Clone, Debug)]
pub struct Line {
    pub pts: Vec<[f64; 2]>,
    pub imp: Vec<f64>,
    pub keep: Vec<u16>,
}

impl Line {
    /// The indices of the points zoom `z` keeps (none when the arc is left out there).
    pub fn kept(&self, z: u8) -> impl Iterator<Item = usize> + '_ {
        let bit = 1u16 << z;
        self.keep.iter().enumerate().filter(move |(_, k)| *k & bit != 0).map(|(i, _)| i)
    }
}

/// What simplifying did, per zoom: points kept, points given back to keep arcs apart and the rounds
/// that took, rings left out.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ZoomStats {
    pub kept: u64,
    pub restored: u64,
    pub rounds: u32,
    pub dropped: u64,
}

/// The arcs simplified for every zoom: a `Line` per arc of the topology (`lines[i]` is
/// `topo.arcs[i]`'s), and what each zoom did.
pub struct Simplified {
    pub lines: Vec<Line>,
    pub zooms: Vec<ZoomStats>,
}

fn seg_dist2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (mut x, mut y) = (a[0], a[1]);
    let (dx, dy) = (b[0] - x, b[1] - y);
    if dx != 0.0 || dy != 0.0 {
        let t = ((p[0] - x) * dx + (p[1] - y) * dy) / (dx * dx + dy * dy);
        if t > 1.0 {
            x = b[0];
            y = b[1];
        } else if t > 0.0 {
            x += dx * t;
            y += dy * t;
        }
    }
    (p[0] - x) * (p[0] - x) + (p[1] - y) * (p[1] - y)
}

/// The point between `a` and `b` (exclusive) farthest from their chord, and its squared distance.
fn farthest(pts: &[[f64; 2]], a: usize, b: usize) -> Option<(usize, f64)> {
    (a + 1..b).map(|i| (i, seg_dist2(pts[i], pts[a], pts[b]))).fold(None, |m, (i, d)| match m {
        Some((_, md)) if md >= d => m,
        _ => Some((i, d)),
    })
}

/// Douglas–Peucker importances between anchors `a` and `b`: a point's own distance, but never more
/// than the one that split its span (so a zoom's points are those Douglas–Peucker keeps at its
/// tolerance).
fn dp(pts: &[[f64; 2]], imp: &mut [f64], a: usize, b: usize) {
    let mut stack = vec![(a, b, f64::INFINITY)];
    while let Some((a, b, cap)) = stack.pop() {
        let Some((at, d)) = farthest(pts, a, b) else { continue };
        let v = d.min(cap);
        imp[at] = v;
        stack.push((a, at, v));
        stack.push((at, b, v));
    }
}

/// A point (E7) in Web Mercator.
pub fn merc(p: [i32; 2]) -> [f64; 2] {
    let (x, y) = roadcore::geo::merc(p[0] as f64 * 1e-7, p[1] as f64 * 1e-7);
    [x, y]
}

/// An arc's line before its zooms: points and importances.
fn line_of(pts: &[[i32; 2]], closed: bool) -> Line {
    let pts: Vec<[f64; 2]> = pts.iter().map(|&p| merc(p)).collect();
    let n = pts.len();
    let mut imp = vec![0.0; n];
    imp[0] = f64::INFINITY;
    imp[n - 1] = f64::INFINITY;
    if closed && n >= 4 {
        // (A ring: its first point, the farthest from it, and the farthest from the chord either side.)
        let far = (1..n - 1)
            .map(|i| (i, (pts[i][0] - pts[0][0]).powi(2) + (pts[i][1] - pts[0][1]).powi(2)))
            .fold((1, -1.0), |m, (i, d)| if d > m.1 { (i, d) } else { m })
            .0;
        imp[far] = f64::INFINITY;
        let mut anchors = vec![0, far, n - 1];
        for (a, b) in [(0, far), (far, n - 1)] {
            if let Some((m, _)) = farthest(&pts, a, b) {
                imp[m] = f64::INFINITY;
                anchors.push(m);
            }
        }
        anchors.sort_unstable();
        for w in anchors.windows(2) {
            dp(&pts, &mut imp, w[0], w[1]);
        }
    } else {
        dp(&pts, &mut imp, 0, n - 1);
    }
    Line { pts, imp, keep: vec![0; n] }
}

/// The largest extent of a line's points (world units).
fn extent(pts: &[[f64; 2]]) -> f64 {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in pts {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    (b[2] - b[0]).max(b[3] - b[1])
}

/// Twice a closed arc's signed area in E7 (positive: counter-clockwise, its left inside).
fn area2(pts: &[[i32; 2]]) -> f64 {
    let mut a = 0.0;
    for w in pts.windows(2) {
        a += w[0][0] as f64 * w[1][1] as f64 - w[1][0] as f64 * w[0][1] as f64;
    }
    a
}

/// The arcs that stay at every zoom however small: for each region every ring of which is a ring
/// meeting no other arc, its largest such ring with it inside.
fn kept_always(topo: &Topology, lines: &[Line]) -> Vec<bool> {
    let n = topo.ids.len();
    let mut open = vec![false; n];
    let mut best: Vec<Option<(f64, usize)>> = vec![None; n];
    for (k, a) in topo.arcs.iter().enumerate() {
        let sides = [&topo.sets[a.l as usize], &topo.sets[a.r as usize]];
        if !a.closed() {
            for &i in sides.into_iter().flatten() {
                open[i as usize] = true;
            }
            continue;
        }
        let inside = if area2(&a.pts) > 0.0 { sides[0] } else { sides[1] };
        let e = extent(&lines[k].pts);
        for &i in inside {
            if best[i as usize].is_none_or(|(be, _)| e > be) {
                best[i as usize] = Some((e, k));
            }
        }
    }
    let mut keep = vec![false; topo.arcs.len()];
    for i in 0..n {
        if let (false, Some((_, k))) = (open[i], best[i]) {
            keep[k] = true;
        }
    }
    keep
}

/// Simplifies every arc of `topo` for every zoom (see the module's comment), reporting each zoom
/// done (`report(done, total)`).
pub fn simplify(topo: &Topology, report: &dyn Fn(u64, u64)) -> Simplified {
    let mut lines: Vec<Line> = topo.arcs.par_iter().map(|a| line_of(&a.pts, a.closed())).collect();
    let always = kept_always(topo, &lines);
    let mut zooms = Vec::new();
    let total = MAXZ as u64 + 1;
    for z in 0..=MAXZ {
        let bit = 1u16 << z;
        if z == MAXZ {
            lines.par_iter_mut().for_each(|l| l.keep.iter_mut().for_each(|k| *k |= bit));
            let kept = lines.iter().map(|l| l.pts.len() as u64).sum();
            zooms.push(ZoomStats { kept, ..Default::default() });
            report(z as u64 + 1, total);
            break;
        }
        let tol2 = tolerance(z).powi(2);
        let drop = DROP_PX / (512.0 * f64::from(1u32 << z));
        let dropped: u64 = lines
            .par_iter_mut()
            .zip(&topo.arcs)
            .zip(&always)
            .map(|((l, a), &always)| {
                if a.closed() && !always && extent(&l.pts) < drop {
                    return 1;
                }
                for (k, &i) in l.keep.iter_mut().zip(&l.imp) {
                    if i >= tol2 {
                        *k |= bit;
                    }
                }
                0
            })
            .sum();
        let (restored, rounds) = untangle(&mut lines, z);
        let kept = lines.iter().map(|l| l.kept(z).count() as u64).sum();
        zooms.push(ZoomStats { kept, restored, rounds, dropped });
        report(z as u64 + 1, total);
    }
    Simplified { lines, zooms }
}

/// A segment of zoom z's lines: its line and the indices of its two kept points.
/// A cell holding more segments than this is split in 8 × 8 (up to `SPLIT_DEPTH` times) before
/// its segments are tested in pairs: a coarse zoom's cell over a dense coast (the Canadian Arctic's
/// at z0) holds hundreds of thousands.
const SPLIT_AT: usize = 64;
const SPLIT_DEPTH: u32 = 6;

/// The segments' pairs tested for meeting, in a cell.
struct Clash<'a> {
    lines: &'a [Line],
    segs: &'a [Seg],
    dirty: &'a [bool],
}

impl Clash<'_> {
    fn pts(&self, s: &Seg) -> ([f64; 2], [f64; 2]) {
        let l = &self.lines[s.line as usize];
        (l.pts[s.a as usize], l.pts[s.b as usize])
    }

    /// Same line, next to each other: they share a point by construction (a ring's first and last
    /// too).
    fn neighbours(&self, s: &Seg, t: &Seg) -> bool {
        s.line == t.line && {
            let l = &self.lines[s.line as usize];
            let n = l.pts.len() as u32 - 1;
            let ring = l.pts[0] == l.pts[n as usize];
            s.b == t.a || t.b == s.a || (ring && s.a == 0 && t.b == n) || (ring && t.a == 0 && s.b == n)
        }
    }

    /// The segments among `ids` (in cell `cell` of side `size`) that skip points and meet another
    /// there, into `out`.
    fn check(&self, ids: &[u32], cell: (i64, i64), size: f64, depth: u32, out: &mut Vec<u32>) {
        let skips = |s: &Seg| s.b > s.a + 1;
        if ids.len() > SPLIT_AT && depth < SPLIT_DEPTH {
            let sub = size / 8.0;
            let mut by: std::collections::HashMap<(i64, i64), Vec<u32>> = std::collections::HashMap::new();
            let mut cs = Vec::new();
            for &k in ids {
                let (a, b) = self.pts(&self.segs[k as usize]);
                cs.clear();
                cells(a, b, sub, &mut cs);
                for &c in &cs {
                    if c.0.div_euclid(8) == cell.0 && c.1.div_euclid(8) == cell.1 {
                        by.entry(c).or_default().push(k);
                    }
                }
            }
            for (c, v) in by {
                self.check(&v, c, sub, depth + 1, out);
            }
            return;
        }
        for i in 0..ids.len() {
            let si = &self.segs[ids[i] as usize];
            for j in i + 1..ids.len() {
                let sj = &self.segs[ids[j] as usize];
                if !(skips(si) || skips(sj)) || !(self.dirty[si.line as usize] || self.dirty[sj.line as usize]) {
                    continue;
                }
                if self.neighbours(si, sj) {
                    continue;
                }
                let ((a, b), (c, d)) = (self.pts(si), self.pts(sj));
                if meet(a, b, c, d) {
                    if skips(si) {
                        out.push(ids[i]);
                    }
                    if skips(sj) {
                        out.push(ids[j]);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Seg {
    line: u32,
    a: u32,
    b: u32,
}

/// The cells (`size` world units across) a segment from `p` to `q` passes through (those it comes
/// within a millionth of a cell of, for rounding), as (column, row).
fn cells(p: [f64; 2], q: [f64; 2], size: f64, out: &mut Vec<(i64, i64)>) {
    let (x0, x1) = (p[0].min(q[0]), p[0].max(q[0]));
    let eps = size * 1e-6;
    let (c0, c1) = (((x0 - eps) / size).floor() as i64, ((x1 + eps) / size).floor() as i64);
    for c in c0..=c1 {
        let (xa, xb) = ((c as f64 * size).max(x0), ((c + 1) as f64 * size).min(x1));
        let y = |x: f64| if q[0] == p[0] { None } else { Some(p[1] + (q[1] - p[1]) * (x - p[0]) / (q[0] - p[0])) };
        let (ya, yb) = match (y(xa), y(xb)) {
            (Some(a), Some(b)) => (a.min(b), a.max(b)),
            _ => (p[1].min(q[1]), p[1].max(q[1])),
        };
        for r in ((ya - eps) / size).floor() as i64..=((yb + eps) / size).floor() as i64 {
            out.push((c, r));
        }
    }
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn on_seg(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    p[0] >= a[0].min(b[0]) && p[0] <= a[0].max(b[0]) && p[1] >= a[1].min(b[1]) && p[1] <= a[1].max(b[1])
}

/// Whether segments a–b and c–d meet anywhere but at a shared end (or, sharing one, run along each
/// other from it).
pub fn meet(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let shared = a == c || a == d || b == c || b == d;
    let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
    if shared {
        // (The same segment twice, or one along the other from the shared end.)
        let (p, q, r) = if a == c {
            (a, b, d)
        } else if a == d {
            (a, b, c)
        } else if b == c {
            (b, a, d)
        } else {
            (b, a, c)
        };
        return orient(p, q, r) == 0.0 && (q[0] - p[0]) * (r[0] - p[0]) + (q[1] - p[1]) * (r[1] - p[1]) > 0.0;
    }
    if ((o1 > 0.0 && o2 < 0.0) || (o1 < 0.0 && o2 > 0.0)) && ((o3 > 0.0 && o4 < 0.0) || (o3 < 0.0 && o4 > 0.0)) {
        return true;
    }
    (o1 == 0.0 && on_seg(a, b, c)) || (o2 == 0.0 && on_seg(a, b, d)) || (o3 == 0.0 && on_seg(c, d, a)) || (o4 == 0.0 && on_seg(c, d, b))
}

/// Zoom `z`'s lines kept apart (the module's comment): points given back to the segments skipping
/// points that meet another, until none does. The points given back and the rounds.
fn untangle(lines: &mut [Line], z: u8) -> (u64, u32) {
    let bit = 1u16 << z;
    let size = 1.0 / f64::from(1u32 << z) / 8.0;
    let mut dirty: Vec<bool> = vec![true; lines.len()];
    let (mut restored, mut rounds) = (0u64, 0u32);
    loop {
        rounds += 1;
        let segs: Vec<Seg> = lines
            .par_iter()
            .enumerate()
            .flat_map_iter(|(k, l)| {
                let kept: Vec<usize> = l.kept(z).collect();
                (1..kept.len()).map(move |i| Seg { line: k as u32, a: kept[i - 1] as u32, b: kept[i] as u32 }).collect::<Vec<_>>()
            })
            .collect();
        let mut at: Vec<(i64, i64, u32)> = segs
            .par_iter()
            .enumerate()
            .flat_map_iter(|(k, s)| {
                let l = &lines[s.line as usize];
                let mut cs = Vec::new();
                cells(l.pts[s.a as usize], l.pts[s.b as usize], size, &mut cs);
                cs.into_iter().map(move |(c, r)| (c, r, k as u32))
            })
            .collect();
        at.par_sort_unstable();
        let mut runs = Vec::new();
        let mut s = 0;
        for i in 1..=at.len() {
            if i == at.len() || (at[i].0, at[i].1) != (at[s].0, at[s].1) {
                runs.push((s, i));
                s = i;
            }
        }
        let ctx = Clash { lines, segs: &segs, dirty: &dirty };
        let mut bad: Vec<u32> = runs
            .par_iter()
            .flat_map_iter(|&(s, e)| {
                let mut out = Vec::new();
                let ids: Vec<u32> = at[s..e].iter().map(|c| c.2).collect();
                ctx.check(&ids, (at[s].0, at[s].1), size, 0, &mut out);
                out
            })
            .collect();
        bad.par_sort_unstable();
        bad.dedup();
        if bad.is_empty() {
            return (restored, rounds);
        }
        dirty.iter_mut().for_each(|d| *d = false);
        for k in bad {
            let s = segs[k as usize];
            let l = &mut lines[s.line as usize];
            let m = (s.a as usize + 1..s.b as usize).max_by(|&i, &j| l.imp[i].total_cmp(&l.imp[j])).expect("a segment skipping points");
            l.keep[m] |= bit;
            dirty[s.line as usize] = true;
            restored += 1;
        }
    }
}
