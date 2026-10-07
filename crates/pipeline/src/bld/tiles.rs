//! The buildings' vector tiles (docs/buildings3d.md §3.4): MVT 2.1 over `names::mvt`, gzip'd, extent
//! 4096, one layer `b`, a feature a building or part.
//!
//! - A feature is whole in the tile holding its centroid (not clipped: its coordinates may run past
//!   the extent), so MapLibre's extrusion has one centroid a building and nothing is drawn twice
//!   (the map keeps z14's tiles whole above z14: web/src/buildings.ts).
//! - A building running past its tile is also copied, whole, into each tile it reaches (`o` 1), for
//!   the flat footprints: a fill is cut at its tile's edge, so the neighbour draws the rest. The
//!   extruded layer leaves the copies out.
//! - Quantized to the tile's grid (z14: 0.6 m at the equator), at z12–13 simplified to one grid
//!   unit first (Douglas–Peucker); each ring's repeated points dropped, a ring of fewer than three
//!   points or no area dropped (an exterior with its holes). Exteriors wind positive (y down), holes
//!   negative.
//! - No edge parallel to an axis beyond the extent ([`unclip`]): MapLibre takes such an edge for a
//!   tile's clip line and draws no wall on it, and in whole buildings they're walls. Its points go
//!   outside the ring (or inside, where outside would cross a ring of the polygon; else none).
//! - Properties: `h` the top and `m` the base (dm; `m` left out when 0), `s` where the height comes
//!   from (0–5), `f` the floors (when `s` is 1), `c` the kind, `k` (1 a part, 2 an outline with
//!   parts; left out when 0), `o` 1 for a copy (left out otherwise). No feature ids.
//! - Features sorted by their centroid's Morton code in the tile (12 bits an axis), then id.
//! - gzip by flate2 at level [`GZIP_LEVEL`].

use super::world7;
use anyhow::Result;
use names::mvt::{Feature, Layer, Tile, Value};
use std::collections::HashMap;
use std::io::Write;

pub const EXTENT: u32 = 4096;
/// The tiles' gzip level (fixed: the same bytes everywhere).
pub const GZIP_LEVEL: u32 = 6;
/// The layer's name in a tile.
pub const LAYER: &str = "b";
/// The properties' keys, in this order in every tile.
pub const KEYS: [&str; 7] = ["h", "m", "s", "f", "c", "k", "o"];
/// MapLibre's subdivision of a polygon's walls on the globe, in tile units: at lines every 2,048
/// units (its granularity 2 over its extent of 8,192 at z6 and deeper: two cells a tile), where it
/// cuts an edge and rounds the cut to its grid.
const SUBDIVISION: i64 = 2048;

/// `c`, from Overture's subtype: 0 unknown, 1 residential, 2 outbuilding, 3 commercial,
/// 4 industrial, 5 religious, 6 civic (and education, medical), 7 agricultural, 8 transportation,
/// 9 other (service, entertainment, military).
pub fn kind_of(subtype: &str) -> u8 {
    match subtype {
        "residential" => 1,
        "outbuilding" => 2,
        "commercial" => 3,
        "industrial" => 4,
        "religious" => 5,
        "civic" | "education" | "medical" => 6,
        "agricultural" => 7,
        "transportation" => 8,
        "service" | "entertainment" | "military" => 9,
        _ => 0,
    }
}

/// A building or part to draw.
pub struct Feat<'a> {
    /// Its polygons, each its rings (the first its exterior), E7.
    pub polys: Vec<Vec<&'a [[i32; 2]]>>,
    pub cen: [i32; 2],
    /// Its place among features of the same Morton code: its block's z14 key and its index there
    /// (the blocks are sorted by id).
    pub order: (u64, u32),
    pub h: u16,
    pub m: u16,
    pub s: u8,
    pub f: u8,
    pub c: u8,
    pub k: u8,
    /// A copy, for the flat footprints, of a building whose centroid is in another tile.
    pub copy: bool,
}

fn zz(v: i64) -> u32 {
    ((v << 1) ^ (v >> 63)) as u32
}

/// Squared distance from `p` to the segment a–b (to a when they coincide).
fn seg_d2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) };
    let (ex, ey) = (p[0] - a[0] - t * dx, p[1] - a[1] - t * dy);
    ex * ex + ey * ey
}

/// Douglas–Peucker of a closed ring (its points, the first repeated at the end), tolerance `tol`:
/// the points kept, the closing one included.
fn simplify(r: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    let n = r.len();
    if n < 3 {
        return r.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    let t2 = tol * tol;
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut best, mut far) = (0.0f64, a);
        for i in a + 1..b {
            let d = seg_d2(r[i], r[a], r[b]);
            if d > best {
                best = d;
                far = i;
            }
        }
        if best > t2 {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    r.iter().zip(&keep).filter(|(_, &k)| k).map(|(p, _)| *p).collect()
}

/// A ring on the tile's grid: its points (no repeats, no closing point), wound as `exterior` wants;
/// None when fewer than three are left or it has no area.
fn grid_ring(r: &[[i32; 2]], z: u8, tx: u32, ty: u32, simplified: bool) -> Option<Vec<[i64; 2]>> {
    let scale = (1u64 << z) as f64 * EXTENT as f64;
    let mut pts: Vec<[f64; 2]> = r.iter().map(|&p| {
        let (wx, wy) = world7(p);
        [wx * scale, wy * scale]
    }).collect();
    if simplified {
        if let Some(&first) = pts.first() {
            pts.push(first);
            pts = simplify(&pts, 1.0);
            pts.pop();
        }
    }
    let (ox, oy) = (tx as i64 * EXTENT as i64, ty as i64 * EXTENT as i64);
    let mut out: Vec<[i64; 2]> = Vec::with_capacity(pts.len());
    for p in pts {
        let q = [p[0].round() as i64 - ox, p[1].round() as i64 - oy];
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    if out.len() < 3 {
        return None;
    }
    Some(out)
}

/// A ring (tile units, no closing point) with no edge MapLibre's extrusion would skip: one parallel
/// to an axis beyond the extent (`isBoundaryEdge`: a clip line, in a clipped tile). Its ends stay
/// where they are; between them:
/// - an edge parallel to an axis gets a point at each subdivision line it crosses (MapLibre cuts
///   it there and rounds the cut, which could leave a parallel piece; a piece ending on a line is
///   left whole), and one more midway when their count is even, every other one a unit outside
///   the ring, so no piece is parallel; one a unit long gets a point a unit outside beside its
///   start (on a line along the other axis, inside the extent there), else beyond its end;
/// - an edge beyond the extent at a slant gets a point at each subdivision line it crosses, where
///   it crosses (rounded outward), or a unit further outside than the points beside it where that
///   would leave a piece parallel.
///
/// Outside: away from what the ring encloses (for a hole, into the building), so a ring a unit
/// across can't fold to nothing (B1's first rule, a unit away from the tile, made five holes of no
/// area in Vermont's z14). An edge's points must leave the ring simple and clear of its polygon's
/// other rings (`others`): where they wouldn't, the same a unit inside, else none (the wall left
/// out).
pub fn unclip(r: &[[i64; 2]], others: &[&[[i64; 2]]]) -> Vec<[i64; 2]> {
    let n = r.len();
    // (Positive: clockwise on screen, what it encloses on the right of each edge.)
    let wind = area2(r).signum() as i64;
    let mut ins: Vec<Vec<[i64; 2]>> = vec![Vec::new(); n];
    for i in 0..n {
        for outward in [true, false] {
            let Some(pts) = edge_points(r[i], r[(i + 1) % n], wind, outward) else {
                break;
            };
            if fits(r, &ins, i, &pts, others) {
                ins[i] = pts;
                break;
            }
        }
    }
    let mut out = Vec::with_capacity(n + ins.iter().map(Vec::len).sum::<usize>());
    for (p, more) in r.iter().zip(&ins) {
        out.push(*p);
        out.extend(more);
    }
    out
}

/// The points [`unclip`] puts between edge a–b's ends (None: it needs none), off it toward the
/// outside of a ring wound `wind`, or the inside.
fn edge_points(a: [i64; 2], b: [i64; 2], wind: i64, outward: bool) -> Option<Vec<[i64; 2]>> {
    let e = EXTENT as i64;
    let beyond = |v: i64| v < 0 || v > e;
    // (along: the axis it runs along, mostly; across: the other, beyond the extent at both ends.)
    let side = |k: usize| (a[k] < 0 && b[k] < 0) || (a[k] > e && b[k] > e);
    let (along, across) = if a[0] == b[0] && beyond(a[0]) {
        (1, 0)
    } else if a[1] == b[1] && beyond(a[1]) {
        (0, 1)
    } else if side(0) && (b[1] - a[1]).abs() >= (b[0] - a[0]).abs() {
        (1, 0)
    } else if side(1) && (b[0] - a[0]).abs() > (b[1] - a[1]).abs() {
        (0, 1)
    } else {
        return None;
    };
    let (s, t) = (a[along], b[along]);
    let dir = (t - s).signum();
    // Outside, across it: the enclosed side is right of its direction when clockwise.
    let out_dir = (if across == 1 { -wind * dir } else { wind * dir }) * if outward { 1 } else { -1 };
    let mut pts = Vec::new();
    // The subdivision lines strictly between its ends, in order from a.
    let mut at: Vec<i64> = Vec::new();
    let mut l = if dir > 0 { s.div_euclid(SUBDIVISION) * SUBDIVISION + SUBDIVISION } else { (s - 1).div_euclid(SUBDIVISION) * SUBDIVISION };
    while (t - l) * dir > 0 {
        at.push(l);
        l += dir * SUBDIVISION;
    }
    if a[across] != b[across] {
        // At a slant: where it crosses each line, unless that leaves a piece parallel.
        let mut prev = a[across];
        for (j, &v) in at.iter().enumerate() {
            let f = a[across] as f64 + (b[across] - a[across]) as f64 * (v - s) as f64 / (t - s) as f64;
            let mut c = if out_dir < 0 { f.floor() } else { f.ceil() } as i64;
            let next = if j + 1 < at.len() { None } else { Some(b[across]) };
            if c == prev || Some(c) == next {
                c = if out_dir < 0 { prev.min(next.unwrap_or(prev)) - 1 } else { prev.max(next.unwrap_or(prev)) + 1 };
            }
            let mut p = a;
            p[along] = v;
            p[across] = c;
            pts.push(p);
            prev = c;
        }
    } else if at.is_empty() && (t - s).abs() < 2 {
        // A unit long: a point a unit off beside its start (the piece to it runs along the other
        // axis, inside the extent there), or, with that beyond too, beyond its end.
        let mut p = a;
        p[across] += out_dir;
        if beyond(s) {
            p[along] = t + dir;
        }
        pts.push(p);
    } else {
        if at.len().is_multiple_of(2) {
            // One more, midway in the longest gap (between lines: no line crossed).
            let mut stops = vec![s];
            stops.extend(&at);
            stops.push(t);
            let k = (0..stops.len() - 1).max_by_key(|&k| ((stops[k + 1] - stops[k]).abs(), std::cmp::Reverse(k))).unwrap();
            at.insert(k, stops[k] + (stops[k + 1] - stops[k]) / 2);
        }
        for (j, &v) in at.iter().enumerate() {
            let mut p = a;
            p[along] = v;
            if j % 2 == 0 {
                p[across] += out_dir;
            }
            pts.push(p);
        }
    }
    (!pts.is_empty()).then_some(pts)
}

/// Which side of line a→b point c is on (0: on it).
fn orient(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> i64 {
    ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).signum()
}

/// Whether c is on segment a–b.
fn on_segment(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> bool {
    orient(a, b, c) == 0 && a[0].min(b[0]) <= c[0] && c[0] <= a[0].max(b[0]) && a[1].min(b[1]) <= c[1] && c[1] <= a[1].max(b[1])
}

/// Whether segments p–q and c–d have a point in common.
fn meet(p: [i64; 2], q: [i64; 2], c: [i64; 2], d: [i64; 2]) -> bool {
    let (d1, d2, d3, d4) = (orient(c, d, p), orient(c, d, q), orient(p, q, c), orient(p, q, d));
    (d1 * d2 < 0 && d3 * d4 < 0) || on_segment(c, d, p) || on_segment(c, d, q) || on_segment(p, q, c) || on_segment(p, q, d)
}

/// Whether x→y then y→z turns back over itself.
fn fold(x: [i64; 2], y: [i64; 2], z: [i64; 2]) -> bool {
    orient(x, y, z) == 0 && (x[0] - y[0]) * (z[0] - y[0]) + (x[1] - y[1]) * (z[1] - y[1]) > 0
}

/// Whether ring r's edge i through `pts` (the other edges through theirs, `ins`) keeps the ring
/// simple and clear of `others`: its pieces meet no other piece, but their neighbours, at their
/// shared ends only.
fn fits(r: &[[i64; 2]], ins: &[Vec<[i64; 2]>], i: usize, pts: &[[i64; 2]], others: &[&[[i64; 2]]]) -> bool {
    let n = r.len();
    let mut mine = Vec::with_capacity(pts.len() + 2);
    mine.push(r[i]);
    mine.extend(pts);
    mine.push(r[(i + 1) % n]);
    let m = mine.len() - 1;
    let (mut lo, mut hi) = ([i64::MAX; 2], [i64::MIN; 2]);
    for p in &mine {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let away = |c: [i64; 2], d: [i64; 2]| (0..2).any(|k| c[k].max(d[k]) < lo[k] || c[k].min(d[k]) > hi[k]);
    // Its own pieces.
    for x in 0..m {
        for y in x + 1..m {
            let bad = if y == x + 1 { fold(mine[x], mine[y], mine[y + 1]) } else { meet(mine[x], mine[x + 1], mine[y], mine[y + 1]) };
            if bad {
                return false;
            }
        }
    }
    // The ring's other edges, with their points: the one before ends at its first point, the one
    // after starts at its last.
    let (before, after) = ((i + n - 1) % n, (i + 1) % n);
    for j in (0..n).filter(|&j| j != i) {
        let k = ins[j].len() + 1;
        for y in 0..k {
            let c = if y == 0 { r[j] } else { ins[j][y - 1] };
            let d = if y + 1 == k { r[(j + 1) % n] } else { ins[j][y] };
            if away(c, d) {
                continue;
            }
            for x in 0..m {
                let (p, q) = (mine[x], mine[x + 1]);
                let bad = if j == before && y + 1 == k && x == 0 {
                    fold(c, p, q)
                } else if j == after && y == 0 && x + 1 == m {
                    fold(p, q, d)
                } else {
                    meet(p, q, c, d)
                };
                if bad {
                    return false;
                }
            }
        }
    }
    for o in others {
        for y in 0..o.len() {
            let (c, d) = (o[y], o[(y + 1) % o.len()]);
            if !away(c, d) && (0..m).any(|x| meet(mine[x], mine[x + 1], c, d)) {
                return false;
            }
        }
    }
    true
}

/// Twice a ring's signed area in tile units (positive: clockwise on screen, y down).
fn area2(r: &[[i64; 2]]) -> i128 {
    let mut s = 0i128;
    for i in 0..r.len() {
        let (a, b) = (r[i], r[(i + 1) % r.len()]);
        s += a[0] as i128 * b[1] as i128 - b[0] as i128 * a[1] as i128;
    }
    s
}

/// A feature's geometry commands (none left: None).
fn geometry(f: &Feat, z: u8, tx: u32, ty: u32) -> Option<Vec<u32>> {
    let simplified = z < 14;
    let mut g = Vec::new();
    let (mut cx, mut cy) = (0i64, 0i64);
    for poly in &f.polys {
        let mut rings: Vec<Vec<[i64; 2]>> = Vec::with_capacity(poly.len());
        for (k, ring) in poly.iter().enumerate() {
            let Some(mut r) = grid_ring(ring, z, tx, ty, simplified) else {
                if k == 0 {
                    break; // the exterior gone: its holes with it
                }
                continue;
            };
            let a = area2(&r);
            if a == 0 {
                if k == 0 {
                    break;
                }
                continue;
            }
            if (a > 0) != (k == 0) {
                r[1..].reverse();
            }
            rings.push(r);
        }
        for k in 0..rings.len() {
            let others: Vec<&[[i64; 2]]> = rings.iter().enumerate().filter(|&(j, _)| j != k).map(|(_, o)| o.as_slice()).collect();
            let u = unclip(&rings[k], &others);
            rings[k] = u;
        }
        for r in &rings {
            g.push(9); // MoveTo 1
            g.push(zz(r[0][0] - cx));
            g.push(zz(r[0][1] - cy));
            (cx, cy) = (r[0][0], r[0][1]);
            g.push(2 | ((r.len() as u32 - 1) << 3)); // LineTo
            for p in &r[1..] {
                g.push(zz(p[0] - cx));
                g.push(zz(p[1] - cy));
                (cx, cy) = (p[0], p[1]);
            }
            g.push(15); // ClosePath
        }
    }
    (!g.is_empty()).then_some(g)
}

/// The tiles at zoom `z` other than `own` that a feature's polygons reach (by their exteriors'
/// extent on the zoom's grid, as [`encode`] quantizes them), in order.
pub fn reached(polys: &[Vec<&[[i32; 2]]>], z: u8, own: (u32, u32)) -> Vec<(u32, u32)> {
    let scale = (1u64 << z) as f64 * EXTENT as f64;
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for poly in polys {
        for &p in poly.first().map_or(&[][..], |r| *r) {
            let (wx, wy) = world7(p);
            let (gx, gy) = ((wx * scale).round() as i64, (wy * scale).round() as i64);
            (x0, y0, x1, y1) = (x0.min(gx), y0.min(gy), x1.max(gx), y1.max(gy));
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let (e, n) = (EXTENT as i64, 1i64 << z);
    let t = |v: i64| v.div_euclid(e).clamp(0, n - 1) as u32;
    let mut out = Vec::new();
    for ty in t(y0)..=t(y1 - 1) {
        for tx in t(x0)..=t(x1 - 1) {
            if (tx, ty) != own {
                out.push((tx, ty));
            }
        }
    }
    out
}

/// The Morton code of a point in tile z/tx/ty, 12 bits an axis.
fn morton_in(p: [i32; 2], z: u8, tx: u32, ty: u32) -> u64 {
    let (wx, wy) = world7(p);
    let n = (1u64 << z) as f64;
    let c = |v: f64, t: u32| (((v * n - t as f64) * EXTENT as f64).floor().max(0.0) as u32).min(EXTENT - 1);
    crate::morton(c(wx, tx), c(wy, ty))
}

/// Tile z/x/y of `feats` (each in it by its centroid): the gzip'd tile, its raw length and its
/// features, or None when no feature is left.
pub fn encode(z: u8, x: u32, y: u32, feats: &[Feat]) -> Result<Option<(Vec<u8>, u32, u32)>> {
    let mut order: Vec<(u64, (u64, u32), usize)> = feats.iter().enumerate().map(|(i, f)| (morton_in(f.cen, z, x, y), f.order, i)).collect();
    order.sort_unstable();
    let mut values: Vec<Value> = Vec::new();
    let mut by_value: HashMap<u64, u32> = HashMap::new();
    let mut vi = |v: u64| -> u32 {
        *by_value.entry(v).or_insert_with(|| {
            values.push(Value::Uint(v));
            values.len() as u32 - 1
        })
    };
    let mut features = Vec::with_capacity(feats.len());
    for &(_, _, i) in &order {
        let f = &feats[i];
        let Some(geometry) = geometry(f, z, x, y) else { continue };
        let mut tags = vec![0, vi(f.h as u64)];
        if f.m > 0 {
            tags.extend([1, vi(f.m as u64)]);
        }
        tags.extend([2, vi(f.s as u64)]);
        if f.s == super::fill::src::FLOORS {
            tags.extend([3, vi(f.f as u64)]);
        }
        tags.extend([4, vi(f.c as u64)]);
        if f.k > 0 {
            tags.extend([5, vi(f.k as u64)]);
        }
        if f.copy {
            tags.extend([6, vi(1)]);
        }
        features.push(Feature { id: None, tags, geom_type: Some(3), geometry, unknown: Vec::new() });
    }
    if features.is_empty() {
        return Ok(None);
    }
    let n = features.len() as u32;
    let layer = Layer { name: LAYER.into(), version: 2, extent: EXTENT, keys: KEYS.iter().map(|k| k.to_string()).collect(), values, features, unknown: Vec::new() };
    let raw = Tile { layers: vec![layer], unknown: Vec::new() }.encode();
    let mut e = flate2::write::GzEncoder::new(Vec::with_capacity(raw.len() / 3 + 64), flate2::Compression::new(GZIP_LEVEL));
    e.write_all(&raw)?;
    Ok(Some((e.finish()?, raw.len() as u32, n)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bld::e7;

    fn sq(lon: f64, lat: f64, d: f64, cw: bool) -> Vec<[i32; 2]> {
        let mut v = vec![[e7(lon), e7(lat)], [e7(lon + d), e7(lat)], [e7(lon + d), e7(lat + d)], [e7(lon), e7(lat + d)]];
        if cw {
            v.reverse();
        }
        v
    }

    #[test]
    fn a_tile() {
        // Two buildings in 14/8298/5634 (Paris), one with a hole, wound either way; one a part.
        let (z, x, y) = (14u8, 8298u32, 5634u32);
        let b = crate::bld::tile_box_deg(z, x, y);
        let (lon, lat) = (b[0] + 0.002, b[1] + 0.002);
        let outer = sq(lon, lat, 0.0005, false);
        let hole = sq(lon + 0.0002, lat + 0.0002, 0.0001, false);
        let other = sq(lon + 0.003, lat + 0.003, 0.0003, true);
        // A sliver that rounds to a line: dropped.
        let sliver = [[e7(lon + 0.006), e7(lat)], [e7(lon + 0.006000001), e7(lat)], [e7(lon + 0.006), e7(lat + 0.0003)]];
        let feats = vec![
            Feat { polys: vec![vec![&outer[..], &hole[..]]], cen: [e7(lon + 0.00025), e7(lat + 0.00025)], order: (1, 0), h: 215, m: 0, s: 0, f: 0, c: 1, k: 0, copy: false },
            Feat { polys: vec![vec![&other[..]]], cen: [e7(lon + 0.00315), e7(lat + 0.00315)], order: (1, 1), h: 64, m: 30, s: 1, f: 2, c: 3, k: 1, copy: false },
            Feat { polys: vec![vec![&sliver[..]]], cen: [e7(lon + 0.006), e7(lat + 0.0001)], order: (1, 2), h: 40, m: 0, s: 5, f: 0, c: 0, k: 0, copy: true },
        ];
        let (gz, raw, n) = encode(z, x, y, &feats).unwrap().unwrap();
        assert_eq!(n, 2);
        let t = Tile::decode(&names::mvt::gunzip_if_gzip(&gz).unwrap()).unwrap();
        assert_eq!(raw as usize, t.encode().len());
        let l = &t.layers[0];
        assert_eq!((l.name.as_str(), l.extent, l.version), ("b", 4096, 2));
        assert_eq!(l.features.len(), 2, "the sliver dropped");
        let props = |f: &Feature| -> Vec<(String, u64)> {
            f.tags.chunks(2).map(|kv| (l.keys[kv[0] as usize].clone(), match l.values[kv[1] as usize] { Value::Uint(v) => v, _ => panic!() })).collect()
        };
        // Sorted by Morton code (y's bit first at each level): the part, further north, first.
        assert_eq!(props(&l.features[1]), vec![("h".into(), 215), ("s".into(), 0), ("c".into(), 1)]);
        assert_eq!(props(&l.features[0]), vec![("h".into(), 64), ("m".into(), 30), ("s".into(), 1), ("f".into(), 2), ("c".into(), 3), ("k".into(), 1)]);
        // Rings: the first feature's exterior positive and its hole negative, whatever they were.
        let rings = |g: &[u32]| {
            let mut out: Vec<Vec<[i64; 2]>> = Vec::new();
            let (mut i, mut c) = (0, [0i64, 0i64]);
            let un = |v: u32| ((v >> 1) as i64) ^ -((v & 1) as i64);
            while i < g.len() {
                let (cmd, n) = (g[i] & 7, g[i] >> 3);
                i += 1;
                match cmd {
                    1 => {
                        c = [c[0] + un(g[i]), c[1] + un(g[i + 1])];
                        out.push(vec![c]);
                        i += 2;
                    }
                    2 => {
                        for _ in 0..n {
                            c = [c[0] + un(g[i]), c[1] + un(g[i + 1])];
                            out.last_mut().unwrap().push(c);
                            i += 2;
                        }
                    }
                    _ => {}
                }
            }
            out
        };
        let r = rings(&l.features[1].geometry);
        assert_eq!(r.len(), 2);
        assert!(area2(&r[0]) > 0 && area2(&r[1]) < 0);
        assert!(area2(&rings(&l.features[0].geometry)[0]) > 0);
        // The same bytes again; the features' order given doesn't matter.
        let rev: Vec<Feat> = feats.into_iter().rev().collect();
        assert_eq!(encode(z, x, y, &rev).unwrap().unwrap().0, gz);
        // No feature left: no tile.
        assert!(encode(z, x, y, &[]).unwrap().is_none());
    }

    /// MapLibre 6.11's walls of a ring as it reads a tile (x2: its extent is 8,192): each edge cut
    /// at its subdivision lines (every 4,096 of its units), the cuts rounded
    /// (`subdivideVertexLine`), and the pieces it skips (`isBoundaryEdge`).
    fn skipped_walls(r: &[[i64; 2]]) -> usize {
        let e = 8192i64;
        let cell = 4096.0;
        let mut skipped = 0;
        let n = r.len();
        for i in 0..n {
            let (a, b) = ([r[i][0] * 2, r[i][1] * 2], [r[(i + 1) % n][0] * 2, r[(i + 1) % n][1] * 2]);
            let mut pts = vec![a];
            let (dx, dy) = ((b[0] - a[0]) as f64, (b[1] - a[1]) as f64);
            let (mut lx, mut ly) = (a[0] as f64, a[1] as f64);
            loop {
                let nbx = if dx > 0.0 { ((lx / cell).floor() + 1.0) * cell } else { ((lx / cell).ceil() - 1.0) * cell };
                let nby = if dy > 0.0 { ((ly / cell).floor() + 1.0) * cell } else { ((ly / cell).ceil() - 1.0) * cell };
                let (adx, ady) = ((lx - nbx).abs(), (ly - nby).abs());
                let (ex, ey) = ((lx - b[0] as f64).abs(), (ly - b[1] as f64).abs());
                if (ex <= adx || dx == 0.0) && (ey <= ady || dy == 0.0) {
                    break;
                }
                let (rx, ry) = (if dx != 0.0 { adx / dx.abs() } else { f64::INFINITY }, if dy != 0.0 { ady / dy.abs() } else { f64::INFINITY });
                let p = if (rx < ry && dx != 0.0) || dy == 0.0 {
                    ly += dy * rx;
                    lx = nbx;
                    [lx as i64, ly.round() as i64]
                } else {
                    lx += dx * ry;
                    ly = nby;
                    [lx.round() as i64, ly as i64]
                };
                if *pts.last().unwrap() != p {
                    pts.push(p);
                }
            }
            if *pts.last().unwrap() != b {
                pts.push(b);
            }
            for w in pts.windows(2) {
                let (p, q) = (w[0], w[1]);
                if (p[0] == q[0] && (p[0] < 0 || p[0] > e)) || (p[1] == q[1] && (p[1] < 0 || p[1] > e)) {
                    skipped += 1;
                }
            }
        }
        skipped
    }

    #[test]
    fn no_wall_skipped() {
        // A rectangle reaching 300 units past the west edge, across the subdivision line at 2,048
        // (y 1,900–2,300); one past the south-east corner (two edges out there, one crossing two
        // lines); one wholly inside; a one-unit wall out west.
        let rings: Vec<Vec<[i64; 2]>> = vec![
            vec![[-300, 1900], [200, 1900], [200, 2300], [-300, 2300]],
            vec![[4000, 4000], [4400, 4000], [4400, 8300], [4000, 8300]],
            vec![[100, 100], [200, 100], [200, 200], [100, 200]],
            vec![[-50, 500], [10, 500], [10, 501], [-50, 501]],
            vec![[-50, 10], [10, 10], [10, 12], [-50, 12]],
            // A unit-long wall in the corner beyond both edges; a wall at a slant a unit across
            // (x -41 to -40) over 1,000 units, across the line at 2,048 near its end, which
            // MapLibre's cut would leave parallel there.
            vec![[-50, -21], [10, -21], [10, 10], [-30, 10], [-50, -20]],
            vec![[-40, 1100], [100, 1100], [100, 2100], [-41, 2100]],
        ];
        assert_eq!(skipped_walls(&rings[0]), 2, "as MapLibre would: the west wall, cut in two");
        assert_eq!(skipped_walls(&rings[3]), 1);
        assert_eq!(skipped_walls(&rings[5]), 3, "its north wall cut in two, and the unit-long one");
        assert_eq!(skipped_walls(&rings[6]), 1, "MapLibre's own cut leaves one piece parallel");
        for (i, r) in rings.iter().enumerate() {
            let u = unclip(r, &[]);
            let left = skipped_walls(&u);
            assert_eq!(left, 0, "ring {i}: {u:?}");
            // Its own points kept, in order; the others within a unit of its edges.
            let kept: Vec<[i64; 2]> = u.iter().copied().filter(|p| r.contains(p)).collect();
            assert_eq!(&kept, r);
            assert_eq!(area2(&u).signum(), area2(r).signum());
            assert!((area2(&u) - area2(r)).abs() <= 2 * (u.len() as i128) * 8192, "ring {i}");
        }
        assert_eq!(unclip(&rings[2], &[]), rings[2], "inside: as it was");
    }

    /// Whether a ring (no closing point) is simple: no point twice, no two edges meeting but
    /// neighbours at their shared point, no neighbour folding back over the other.
    fn simple(r: &[[i64; 2]]) -> bool {
        let n = r.len();
        for i in 0..n {
            for j in i + 1..n {
                let (a, b, c, d) = (r[i], r[(i + 1) % n], r[j], r[(j + 1) % n]);
                let bad = if r[i] == r[j] {
                    true
                } else if j == i + 1 {
                    fold(a, b, d)
                } else if i == 0 && j == n - 1 {
                    fold(c, a, b)
                } else {
                    meet(a, b, c, d)
                };
                if bad {
                    return false;
                }
            }
        }
        true
    }

    #[test]
    fn small_rings_stay_whole() {
        // The rings B1's pilot packs had folded to no area (a unit away from the tile, the rule
        // then): Vermont's five at z14, unit triangles out past an edge, and Paris's thin
        // triangle, its point beside the middle of a two-unit edge.
        let found: [Vec<[i64; 2]>; 6] = [
            vec![[2450, -97], [2451, -97], [2451, -98]],
            vec![[1138, 4117], [1137, 4117], [1138, 4118]],
            vec![[1073, -19], [1072, -18], [1073, -18]],
            vec![[4127, 2581], [4127, 2582], [4128, 2582]],
            vec![[-57, 1082], [-56, 1083], [-56, 1082]],
            vec![[4126, 4086], [4125, 4085], [4125, 4087]],
        ];
        for r in &found {
            let u = unclip(r, &[]);
            assert!(simple(&u) && area2(&u).signum() == area2(r).signum() && skipped_walls(&u) == 0, "{r:?} -> {u:?}");
        }
        // A hole a unit inside its exterior, both out past the west edge: its points stay clear.
        let ext = vec![[-20, 100], [-10, 100], [-10, 110], [-20, 110]];
        let hole = vec![[-19, 101], [-19, 109], [-11, 109], [-11, 101]];
        let u = unclip(&hole, &[&ext]);
        assert!((0..u.len()).all(|i| (0..4).all(|j| !meet(u[i], u[(i + 1) % u.len()], ext[j], ext[(j + 1) % 4]))), "{u:?}");
        // Every triangle and quadrilateral on small grids out past an edge, across a subdivision
        // line, and in a corner, both ways round: still simple, wound as it was. Walls left out
        // where no points fit: 647 of 40,440, each a unit-long edge in a notch a unit wide.
        let grids: [(std::ops::RangeInclusive<i64>, std::ops::RangeInclusive<i64>); 4] =
            [(-3..=0, 2046..=2050), (-2..=1, -2..=1), (4095..=4098, 1000..=1003), (2046..=2050, 4096..=4099)];
        let (mut checked, mut left) = (0, 0);
        for (xs, ys) in grids {
            let pts: Vec<[i64; 2]> = xs.clone().flat_map(|x| ys.clone().map(move |y| [x, y])).collect();
            let n = pts.len();
            let mut rings: Vec<Vec<[i64; 2]>> = Vec::new();
            for i in 0..n {
                for j in i + 1..n {
                    for k in j + 1..n {
                        rings.push(vec![pts[i], pts[j], pts[k]]);
                        for l in k + 1..n {
                            let (a, b, c, d) = (pts[i], pts[j], pts[k], pts[l]);
                            rings.extend([vec![a, b, c, d], vec![a, b, d, c], vec![a, c, b, d]]);
                        }
                    }
                }
            }
            for mut r in rings {
                for _ in 0..2 {
                    r.reverse();
                    if area2(&r) == 0 || !simple(&r) {
                        continue;
                    }
                    let u = unclip(&r, &[]);
                    assert!(simple(&u), "{r:?} -> {u:?}");
                    assert_eq!(area2(&u).signum(), area2(&r).signum(), "{r:?} -> {u:?}");
                    if skipped_walls(&u) > 0 {
                        left += 1;
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 10_000, "{checked}");
        assert!(left * 50 < checked, "{left} of {checked}");
    }

    #[test]
    fn simplifying() {
        // A square with a point 0.4 units off its west side: gone at one unit, kept at 0.1.
        let r = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.4, 5.0], [0.0, 0.0]];
        assert_eq!(simplify(&r, 1.0).len(), 5);
        assert_eq!(simplify(&r, 0.1).len(), 6);
        assert_eq!(kind_of("education"), 6);
        assert_eq!(kind_of(""), 0);
    }
}
