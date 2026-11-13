//! The regions as a topology (docs/steps.md §15, the topology).
//!
//! Every ring edge of every region, with the region on its left, is put in one list under its
//! two ends (vertices shared by their exact E7 coordinates: after the cover check, neighbours'
//! borders are the same points, docs/inputs.md §6.1.4). An edge with nothing on one side is split
//! at the points of others like it lying on it (a point one ring has and the other dropped as
//! collinear: a T-junction) or within `NEAR` of it (a point rounded to the grid off the other's
//! line, as along a region's coast between its land and its water), so both have the same edges.
//! Each edge then has the
//! regions on its left and on its right (sets: overlapping regions have several, a coast none);
//! an edge with the same set on both sides (within a region, as between its stored cells or its
//! land and water) is no border and goes. Edges join into arcs between nodes: vertices where more
//! than two border edges meet, or where the sets change.
//!
//! The antimeridian: the geometry is cut there (a ring across it is two, docs/inputs.md §6.1.4), so
//! an edge along x = ±180° has nothing on its outer side in the plane. Each such edge is matched with
//! the other meridian's at the same latitudes (both split at each other's points): what lies across
//! it is the regions on the other side. Where they are the same, the edge is a cut, not a border,
//! and is never drawn; where not, it is drawn on both meridians. Edges along the poles' latitudes
//! (±90°) are never drawn.

use super::{Region, HALF, POLE};
use det::Det;
use rayon::prelude::*;
use std::collections::HashMap;

/// An arc: its points (E7) from one node to the next (a closed arc, a ring meeting no other: its
/// first point again at its end), the sets of regions on its left and right in the plane (indices
/// into `Topology::sets`), and, drawn, the sets as drawn (across the antimeridian for an edge on
/// it: the regions on the other side; otherwise as in the plane).
#[derive(Clone, Debug, PartialEq)]
pub struct Arc {
    pub pts: Vec<[i32; 2]>,
    pub l: u32,
    pub r: u32,
    /// Whether it's a border drawn (not a cut along ±180° within one set, nor along a pole).
    pub drawn: bool,
    pub ld: u32,
    pub rd: u32,
}

impl Arc {
    pub fn closed(&self) -> bool {
        self.pts.len() > 2 && self.pts.first() == self.pts.last()
    }
}

/// The topology of a set of regions.
#[derive(Clone, Debug, Default)]
pub struct Topology {
    /// The regions' ids and folders, by index.
    pub ids: Vec<String>,
    pub folders: Vec<Vec<String>>,
    /// The sets of regions an arc's side has (sorted indices); set 0 is the empty one.
    pub sets: Vec<Vec<u32>>,
    pub arcs: Vec<Arc>,
    /// What was found: edges in, edges split at a T-junction, border edges, edges with one side
    /// empty that lie in the plane (no region on one side: a coast of the regions, or two
    /// neighbours' borders that don't quite meet).
    pub stats: Stats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Stats {
    pub points: u64,
    pub vertices: u64,
    pub edges: u64,
    pub split: u64,
    pub borders: u64,
    pub one_sided: u64,
    /// Slivers closed (`slivers`).
    pub slivers: u64,
    pub arcs: u64,
}

impl Topology {
    /// The level arc `a` separates: how many folders the regions either side of it share (0: they
    /// are in different top folders, or a side has no region; for overlapping regions, the least
    /// over the pairs).
    pub fn level(&self, a: &Arc) -> u32 {
        let (l, r) = (&self.sets[a.ld as usize], &self.sets[a.rd as usize]);
        let mut lv = u32::MAX;
        for &i in l {
            for &j in r {
                if i == j {
                    continue;
                }
                let (fi, fj) = (&self.folders[i as usize], &self.folders[j as usize]);
                lv = lv.min(fi.iter().zip(fj).take_while(|(a, b)| a == b).count() as u32);
            }
        }
        if lv == u32::MAX {
            0
        } else {
            lv
        }
    }

    /// A set's regions' ids, comma-joined (sorted by index: the regions come sorted by id).
    pub fn set_text(&self, s: u32) -> String {
        let ids: Vec<&str> = self.sets[s as usize].iter().map(|&i| self.ids[i as usize].as_str()).collect();
        ids.join(",")
    }
}

/// A point's sort key: by x, then y.
fn key(p: [i32; 2]) -> u64 {
    ((p[0] as u32 ^ 0x8000_0000) as u64) << 32 | (p[1] as u32 ^ 0x8000_0000) as u64
}

fn unkey(k: u64) -> [i32; 2] {
    [((k >> 32) as u32 ^ 0x8000_0000) as i32, (k as u32 ^ 0x8000_0000) as i32]
}

/// Twice the signed area of the triangle o, a, b (positive: counter-clockwise), exactly.
fn cross(o: [i32; 2], a: [i32; 2], b: [i32; 2]) -> i128 {
    let (ax, ay) = (a[0] as i128 - o[0] as i128, a[1] as i128 - o[1] as i128);
    let (bx, by) = (b[0] as i128 - o[0] as i128, b[1] as i128 - o[1] as i128);
    ax * by - ay * bx
}

/// How far a point may lie off an edge and still be taken as on it (E7 units: under 5 cm):
/// where two shapes' borders were each rounded to the grid (a region's land and water, cut from one
/// another, docs/inputs.md §6.1.4), a point of one lies up to 1.5 units off the other's edge (measured).
pub const NEAR: i64 = 4;

/// Where `p` lies along segment a–b (the dot product of p − a with b − a), when it is within `NEAR`
/// of it and strictly between its ends.
fn near_on(a: [i32; 2], b: [i32; 2], p: [i32; 2]) -> Option<i128> {
    if p == a || p == b {
        return None;
    }
    let (dx, dy) = (b[0] as i128 - a[0] as i128, b[1] as i128 - a[1] as i128);
    let len2 = dx * dx + dy * dy;
    let t = (p[0] as i128 - a[0] as i128) * dx + (p[1] as i128 - a[1] as i128) * dy;
    let c = cross(a, b, p);
    (t > 0 && t < len2 && c * c <= (NEAR as i128 * NEAR as i128) * len2).then_some(t)
}

/// An edge in the list: its ends' vertex ids (u < v) and a tag: the region's index × 2, plus 1 when
/// the region is on the right of u → v.
type Edge = (u64, u32);

fn edge(a: u32, b: u32, region: u32) -> Edge {
    if a < b {
        (((a as u64) << 32) | b as u64, region << 1)
    } else {
        (((b as u64) << 32) | a as u64, region << 1 | 1)
    }
}

/// The regions on an edge's left and right, from its copies (`edges`: one group, sorted), by each
/// region's winding: its copies along u → v less those against. An edge a region has both ways, as
/// between its own pieces, counts not at all; once more one way than the other, as where a hair of
/// it was split onto its own edge, once.
fn sides(edges: &[Edge]) -> (Vec<u32>, Vec<u32>) {
    let (mut l, mut r) = (Vec::new(), Vec::new());
    let mut k = 0;
    while k < edges.len() {
        let region = edges[k].1 >> 1;
        let mut net = 0i32;
        while k < edges.len() && edges[k].1 >> 1 == region {
            net += if edges[k].1 & 1 == 0 { 1 } else { -1 };
            k += 1;
        }
        match net.cmp(&0) {
            std::cmp::Ordering::Greater => l.push(region),
            std::cmp::Ordering::Less => r.push(region),
            std::cmp::Ordering::Equal => {}
        }
    }
    (l, r)
}

/// The grid cell size for finding points on edges (E7 units: about 700 m).
const CELL_SHIFT: u32 = 16;

/// The points among `cand` (sorted vertex ids) lying on each of `edges` (u, v) strictly between
/// its ends, or within `NEAR` of it, by edge, sorted along it.
fn splits(verts: &[[i32; 2]], cand: &[u32], edges: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for &c in cand {
        let p = verts[c as usize];
        grid.entry((p[0] >> CELL_SHIFT, p[1] >> CELL_SHIFT)).or_default().push(c);
    }
    edges
        .par_iter()
        .map(|&(u, v)| {
            let (a, b) = (verts[u as usize], verts[v as usize]);
            let mut on: Vec<(i128, u32)> = Vec::new();
            let (c0, c1) = (a[0].min(b[0]) >> CELL_SHIFT, a[0].max(b[0]) >> CELL_SHIFT);
            for cx in c0..=c1 {
                // (The edge's y over the column, from its line; a cell more each way for rounding.)
                let x0 = ((cx as i64) << CELL_SHIFT).max(a[0].min(b[0]) as i64) as f64;
                let x1 = (((cx as i64 + 1) << CELL_SHIFT) - 1).min(a[0].max(b[0]) as i64) as f64;
                let y_at = |x: f64| {
                    if a[0] == b[0] {
                        None
                    } else {
                        Some(
                            a[1] as f64 + (b[1] as f64 - a[1] as f64) * (x - a[0] as f64) / (b[0] as f64 - a[0] as f64),
                        )
                    }
                };
                let (ya, yb) = match (y_at(x0), y_at(x1)) {
                    (Some(p), Some(q)) => (p.min(q), p.max(q)),
                    _ => (a[1].min(b[1]) as f64, a[1].max(b[1]) as f64),
                };
                let (r0, r1) = (((ya as i64) >> CELL_SHIFT) - 1, ((yb as i64) >> CELL_SHIFT) + 1);
                for cy in r0..=r1 {
                    if let Some(ps) = grid.get(&(cx, cy as i32)) {
                        on.extend(ps.iter().filter_map(|&w| Some((near_on(a, b, verts[w as usize])?, w))));
                    }
                }
            }
            on.sort_unstable();
            on.dedup();
            on.into_iter().map(|(_, w)| w).collect()
        })
        .collect()
}

/// The topology of `regions`: see the module's comment.
pub fn build(regions: &[Region]) -> Topology {
    let mut stats = Stats::default();
    // The vertices: every point, and its mirror across the antimeridian for a point on it.
    let mut keys: Vec<u64> = regions
        .par_iter()
        .flat_map_iter(|r| {
            r.rings.iter().flatten().flat_map(|&p| {
                let m = (p[0].abs() == HALF).then(|| key([-p[0], p[1]]));
                std::iter::once(key(p)).chain(m)
            })
        })
        .collect();
    stats.points = regions.iter().flat_map(|r| &r.rings).map(|r| r.len() as u64).sum();
    keys.par_sort_unstable();
    keys.dedup();
    let verts: Vec<[i32; 2]> = keys.iter().map(|&k| unkey(k)).collect();
    stats.vertices = verts.len() as u64;
    let id = |p: [i32; 2]| keys.binary_search(&key(p)).expect("every point is a vertex") as u32;

    // The edges, each with its region on its left.
    let mut edges: Vec<Edge> = regions
        .par_iter()
        .enumerate()
        .flat_map_iter(|(k, r)| {
            let id = &id;
            r.rings.iter().flat_map(move |ring| {
                let n = ring.len();
                (0..n).filter_map(move |i| {
                    let (a, b) = (id(ring[i]), id(ring[(i + 1) % n]));
                    (a != b).then(|| edge(a, b, k as u32))
                })
            })
        })
        .collect();
    stats.edges = edges.len() as u64;
    edges.par_sort_unstable();

    // T-junctions: the edges with nothing on one side, split at the points of others like them
    // lying on them or within NEAR of them (and the meridians' at each other's points).
    let groups = group(&edges);
    let one_sided: Vec<(usize, usize)> = groups
        .iter()
        .copied()
        .filter(|&(s, e)| {
            let (l, r) = sides(&edges[s..e]);
            l.is_empty() != r.is_empty()
        })
        .collect();
    let mut cand: Vec<u32> = one_sided
        .iter()
        .flat_map(|&(s, _)| [(edges[s].0 >> 32) as u32, edges[s].0 as u32])
        .chain(verts.iter().enumerate().filter(|(_, p)| p[0].abs() == HALF).map(|(i, _)| i as u32))
        .collect();
    cand.par_sort_unstable();
    cand.dedup();
    let spans: Vec<(u32, u32)> =
        one_sided.iter().map(|&(s, _)| ((edges[s].0 >> 32) as u32, edges[s].0 as u32)).collect();
    let on = splits(&verts, &cand, &spans);
    let mut added: Vec<Edge> = Vec::new();
    let mut gone: Vec<bool> = vec![false; edges.len()];
    for (&(s, e), pts) in one_sided.iter().zip(&on) {
        if pts.is_empty() {
            continue;
        }
        stats.split += (e - s) as u64;
        let (u, v) = ((edges[s].0 >> 32) as u32, edges[s].0 as u32);
        let chain: Vec<u32> = std::iter::once(u).chain(pts.iter().copied()).chain(std::iter::once(v)).collect();
        for t in &edges[s..e] {
            for w in chain.windows(2) {
                // (Each piece runs the same way as the edge: its side is the edge's.)
                added.push((((w[0] as u64) << 32) | w[1] as u64, t.1));
            }
        }
        gone[s..e].iter_mut().for_each(|g| *g = true);
    }
    if !added.is_empty() {
        edges = edges.into_iter().zip(gone).filter(|(_, g)| !g).map(|(e, _)| e).chain(added).collect();
        edges.par_sort_unstable();
    }

    // Each edge's sides; those with no region on either go.
    let mut sets: Vec<Vec<u32>> = vec![Vec::new()];
    let mut set_ids: HashMap<Vec<u32>, u32> = HashMap::from([(Vec::new(), 0)]);
    let mut intern = |v: Vec<u32>| -> u32 {
        if let Some(&i) = set_ids.get(&v) {
            return i;
        }
        let i = sets.len() as u32;
        sets.push(v.clone());
        set_ids.insert(v, i);
        i
    };
    // (u, v, left set, right set)
    let mut border: Vec<(u32, u32, u32, u32)> = Vec::new();
    for (s, e) in group(&edges) {
        let (l, r) = sides(&edges[s..e]);
        if l.is_empty() && r.is_empty() {
            continue;
        }
        let (u, v) = ((edges[s].0 >> 32) as u32, edges[s].0 as u32);
        border.push((u, v, intern(l), intern(r)));
    }
    drop(edges);
    stats.borders = border.len() as u64;

    // The meridians: what lies across each edge on one is the other's at the same latitudes.
    // (Going up the edge, u → v: on x = -180° the plane is on its right, on +180° on its left.)
    let on_meridian = |b: &(u32, u32, u32, u32), x: i32| verts[b.0 as usize][0] == x && verts[b.1 as usize][0] == x;
    let lat = |b: &(u32, u32, u32, u32)| (verts[b.0 as usize][1], verts[b.1 as usize][1]);
    let west: HashMap<(i32, i32), u32> =
        border.iter().filter(|b| on_meridian(b, -HALF)).map(|b| (lat(b), b.3)).collect();
    let east: HashMap<(i32, i32), u32> =
        border.iter().filter(|b| on_meridian(b, HALF)).map(|b| (lat(b), b.2)).collect();
    // (u, v, l, r, drawn, ld, rd)
    type Labelled = (u32, u32, u32, u32, bool, u32, u32);
    let labelled: Vec<Labelled> = border
        .iter()
        .map(|b| {
            let (u, v, l, r) = *b;
            let (pu, pv) = (verts[u as usize], verts[v as usize]);
            if pu[1] == pv[1] && pu[1].abs() == POLE {
                return (u, v, l, r, false, l, r);
            }
            if on_meridian(b, -HALF) {
                let across = east.get(&lat(b)).copied().unwrap_or(0);
                return (u, v, l, r, across != r, across, r);
            }
            if on_meridian(b, HALF) {
                let across = west.get(&lat(b)).copied().unwrap_or(0);
                return (u, v, l, r, across != l, l, across);
            }
            if l == 0 || r == 0 {
                stats.one_sided += 1;
            }
            (u, v, l, r, true, l, r)
        })
        .collect();
    drop(border);

    let mut arcs = arcs_of(&verts, &labelled);
    drop(labelled);
    // Slivers between neighbours' borders closed, twice (closing one can make the next one's two
    // sides two arcs between the same nodes), the arcs joined again across the nodes that go.
    for _ in 0..2 {
        let (merged, n) = slivers(arcs);
        stats.slivers += n;
        arcs = merged;
        if n == 0 {
            break;
        }
        let id = |p: [i32; 2]| keys.binary_search(&key(p)).expect("every point is a vertex") as u32;
        let mut edges: Vec<Labelled> = arcs
            .iter()
            .flat_map(|a| {
                a.pts.windows(2).map(move |w| {
                    let (u, v) = (id(w[0]), id(w[1]));
                    if u < v {
                        (u, v, a.l, a.r, a.drawn, a.ld, a.rd)
                    } else {
                        (v, u, a.r, a.l, a.drawn, a.rd, a.ld)
                    }
                })
            })
            .collect();
        // (In the order the first pass had them: the arcs come out the same on every run.)
        edges.par_sort_unstable();
        arcs = arcs_of(&verts, &edges);
    }
    stats.arcs = arcs.len() as u64;
    Topology {
        ids: regions.iter().map(|r| r.id.clone()).collect(),
        folders: regions.iter().map(|r| r.folders.clone()).collect(),
        sets,
        arcs,
        stats,
    }
}

/// Slivers closed: two arcs in the plane between the same two nodes, the space between them a set
/// of its own (no region: a gap between neighbours; both: an overlap; or a third region's strip),
/// narrower on average than the finest zoom's tolerance (its area in Web Mercator over the arcs'
/// mean length), are one arc, the one with more points, the sets beyond them on its two sides (none
/// when they are the same: a sliver inside one region). A strip of a third region so closed is
/// drawn as no region of its own and isn't found there by `hit` (docs/steps.md §15 has how many).
/// The cover check leaves gaps and overlaps under 0.1 km² between neighbours (docs/inputs.md
/// §6.1.8, `HAIR_KM2`); each of them would be two lines a few metres apart, and nodes at its ends
/// that no zoom could simplify away. The arcs and how many were closed.
fn slivers(arcs: Vec<Arc>) -> (Vec<Arc>, u64) {
    let plain = |a: &Arc| a.drawn && a.ld == a.l && a.rd == a.r && !a.closed() && a.pts.len() >= 2;
    let mut by: HashMap<(u64, u64), Vec<usize>> = HashMap::new();
    for (k, a) in arcs.iter().enumerate() {
        if plain(a) {
            let (p, q) = (key(a.pts[0]), key(*a.pts.last().unwrap()));
            by.entry((p.min(q), p.max(q))).or_default().push(k);
        }
    }
    // Each arc as it runs from the pair's first node: its points and sides.
    let along = |a: &Arc, from: u64| -> (Vec<[i32; 2]>, u32, u32) {
        if key(a.pts[0]) == from {
            (a.pts.clone(), a.l, a.r)
        } else {
            (a.pts.iter().rev().copied().collect(), a.r, a.l)
        }
    };
    let len = |pts: &[[f64; 2]]| pts.windows(2).map(|w| (w[1][0] - w[0][0]).dhypot(w[1][1] - w[0][1])).sum::<f64>();
    let mut gone = vec![false; arcs.len()];
    let mut added = Vec::new();
    let mut pairs: Vec<_> = by.into_iter().filter(|(k, v)| v.len() == 2 && k.0 != k.1).collect();
    pairs.sort_unstable();
    for ((from, _), v) in pairs {
        let (a, b) = (along(&arcs[v[0]], from), along(&arcs[v[1]], from));
        // (The left one's right side is the sliver, as is the right one's left.)
        let (left, right) = if a.2 == b.1 {
            (a, b)
        } else if b.2 == a.1 {
            (b, a)
        } else {
            continue;
        };
        let ml: Vec<[f64; 2]> = left.0.iter().map(|&p| super::simplify::merc(p)).collect();
        let mr: Vec<[f64; 2]> = right.0.iter().map(|&p| super::simplify::merc(p)).collect();
        let ring: Vec<[f64; 2]> = ml.iter().chain(mr.iter().rev()).copied().collect();
        let area = (0..ring.len()).map(|i| {
            let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
            p[0] * q[1] - q[0] * p[1]
        });
        let area = area.sum::<f64>().abs() / 2.0;
        if area >= super::tolerance(super::MAXZ) * (len(&ml) + len(&mr)) / 2.0 {
            continue;
        }
        gone[v[0]] = true;
        gone[v[1]] = true;
        let (l, r) = (left.1, right.2);
        if l != r {
            let pts = if right.0.len() > left.0.len() { right.0 } else { left.0 };
            added.push(Arc { pts, l, r, drawn: true, ld: l, rd: r });
        }
    }
    let n = gone.iter().filter(|g| **g).count() as u64 / 2;
    let mut out: Vec<Arc> = arcs.into_iter().zip(gone).filter(|(_, g)| !g).map(|(a, _)| a).collect();
    out.extend(added);
    (out, n)
}

/// The runs of `edges` (sorted) with the same ends.
fn group(edges: &[Edge]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut s = 0;
    for i in 1..=edges.len() {
        if i == edges.len() || edges[i].0 != edges[s].0 {
            out.push((s, i));
            s = i;
        }
    }
    out
}

/// The border edges joined into arcs.
fn arcs_of(verts: &[[i32; 2]], edges: &[(u32, u32, u32, u32, bool, u32, u32)]) -> Vec<Arc> {
    let n = verts.len();
    // Each vertex's edges.
    let mut start = vec![0u32; n + 1];
    for e in edges {
        start[e.0 as usize + 1] += 1;
        start[e.1 as usize + 1] += 1;
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut at = start.clone();
    let mut inc = vec![0u32; start[n] as usize];
    for (k, e) in edges.iter().enumerate() {
        for w in [e.0, e.1] {
            inc[at[w as usize] as usize] = k as u32;
            at[w as usize] += 1;
        }
    }
    let of = |v: u32| &inc[start[v as usize] as usize..start[v as usize + 1] as usize];
    // An edge's label leaving vertex `from`: (l, r, drawn, ld, rd) as walked.
    let label = |k: u32, from: u32| {
        let e = &edges[k as usize];
        if from == e.0 {
            (e.2, e.3, e.4, e.5, e.6)
        } else {
            (e.3, e.2, e.4, e.6, e.5)
        }
    };
    let other = |k: u32, v: u32| {
        let e = &edges[k as usize];
        if e.0 == v {
            e.1
        } else {
            e.0
        }
    };
    let node: Vec<bool> = (0..n as u32)
        .into_par_iter()
        .map(|v| {
            let es = of(v);
            if es.len() != 2 {
                return !es.is_empty();
            }
            // (Arriving by one and leaving by the other: the same label going on.)
            let (a, b) = (es[0], es[1]);
            let arrive = label(a, other(a, v));
            arrive != label(b, v)
        })
        .collect();
    let mut seen = vec![false; edges.len()];
    let mut arcs = Vec::new();
    let walk = |first: u32, from: u32, seen: &mut Vec<bool>| -> Arc {
        let (l, r, drawn, ld, rd) = label(first, from);
        let mut pts = vec![verts[from as usize]];
        let (mut k, mut cur) = (first, from);
        loop {
            seen[k as usize] = true;
            let next = other(k, cur);
            pts.push(verts[next as usize]);
            if node[next as usize] || next == from {
                break;
            }
            let es = of(next);
            k = if es[0] == k { es[1] } else { es[0] };
            cur = next;
        }
        Arc { pts, l, r, drawn, ld, rd }
    };
    for v in 0..n as u32 {
        if !node[v as usize] {
            continue;
        }
        for &k in of(v) {
            if !seen[k as usize] {
                let mut a = walk(k, v, &mut seen);
                // (Each open arc runs from its smaller end to its larger, its sides swapped to match.)
                if key(*a.pts.last().unwrap()) < key(a.pts[0]) {
                    a.pts.reverse();
                    std::mem::swap(&mut a.l, &mut a.r);
                    std::mem::swap(&mut a.ld, &mut a.rd);
                }
                arcs.push(a);
            }
        }
    }
    // Rings meeting no other arc: each from its smallest point, the way its first edge runs.
    for k in 0..edges.len() as u32 {
        if seen[k as usize] {
            continue;
        }
        let mut a = walk(k, edges[k as usize].0, &mut seen);
        a.pts.pop();
        let m = (0..a.pts.len()).min_by_key(|&i| key(a.pts[i])).unwrap_or(0);
        a.pts.rotate_left(m);
        a.pts.push(a.pts[0]);
        arcs.push(a);
    }
    arcs
}
