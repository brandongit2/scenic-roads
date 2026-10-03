//! Vector tiles (MVT) from features, as geojson-vt makes them: the geometry in Web Mercator units
//! (0–1), each vertex with its Douglas–Peucker importance (computed once, at full resolution), cut
//! tile by tile from the parent tile's cut geometry down to the deepest zoom; a zoom keeps the
//! vertices important at its tolerance. Extent 4096, a buffer of 64 units around each tile.

use names::mvt::{Feature as MvtFeature, Layer, Tile, Value};
use std::collections::HashMap;

pub const EXTENT: u32 = 4096;
pub const BUFFER: f64 = 64.0;
/// Simplification tolerance, in tile units (of EXTENT).
const TOLERANCE: f64 = 3.0;

/// A feature's geometry in lon/lat.
#[derive(Clone, Debug)]
pub enum Geom {
    Points(Vec<[f64; 2]>),
    Lines(Vec<Vec<[f64; 2]>>),
    /// Polygons: each an outer ring then its holes.
    Polygons(Vec<Vec<Vec<[f64; 2]>>>),
}

/// A feature to tile: shown from `minzoom` (the zooms below leave it out).
#[derive(Clone, Debug)]
pub struct Feature {
    pub id: u64,
    pub geom: Geom,
    pub props: Vec<(String, Value)>,
    pub minzoom: u8,
}

/// A vertex: position (Mercator, 0–1) and importance (squared distance, Mercator units; the ends
/// of a line or ring, and points cut in, are always kept).
#[derive(Clone, Copy, Debug)]
struct V {
    x: f64,
    y: f64,
    imp: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Point,
    Line,
    Polygon,
}

/// A feature in tiling: its rings (lines; points one vertex each), a ring's role for polygons
/// (`outer[i]`), its bbox.
#[derive(Clone, Debug)]
struct TF {
    /// Index into the source features (id, properties, minzoom).
    src: u32,
    kind: Kind,
    rings: Vec<Vec<V>>,
    outer: Vec<bool>,
    bbox: [f64; 4],
}

fn merc(lon: f64, lat: f64) -> (f64, f64) {
    let s = (lat.clamp(-85.051_128_78, 85.051_128_78)).to_radians().sin();
    let x = lon / 360.0 + 0.5;
    let y = 0.5 - 0.25 * ((1.0 + s) / (1.0 - s)).ln() / std::f64::consts::PI;
    (x, y.clamp(0.0, 1.0))
}

/// Douglas–Peucker importance of a polyline's inner vertices (squared distances); ends get 1.
fn simplify(v: &mut [V]) {
    let n = v.len();
    if n == 0 {
        return;
    }
    v[0].imp = 1.0;
    v[n - 1].imp = 1.0;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut best, mut at) = (-1.0f64, a);
        for i in a + 1..b {
            let d = seg_dist2(v[i], v[a], v[b]);
            if d > best {
                best = d;
                at = i;
            }
        }
        v[at].imp = best;
        stack.push((a, at));
        stack.push((at, b));
    }
}

fn seg_dist2(p: V, a: V, b: V) -> f64 {
    let (mut x, mut y) = (a.x, a.y);
    let (dx, dy) = (b.x - x, b.y - y);
    if dx != 0.0 || dy != 0.0 {
        let t = ((p.x - x) * dx + (p.y - y) * dy) / (dx * dx + dy * dy);
        if t > 1.0 {
            x = b.x;
            y = b.y;
        } else if t > 0.0 {
            x += dx * t;
            y += dy * t;
        }
    }
    (p.x - x) * (p.x - x) + (p.y - y) * (p.y - y)
}

fn bbox_of(rings: &[Vec<V>]) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for r in rings {
        for v in r {
            b[0] = b[0].min(v.x);
            b[1] = b[1].min(v.y);
            b[2] = b[2].max(v.x);
            b[3] = b[3].max(v.y);
        }
    }
    b
}

fn prepare(fs: &[Feature]) -> Vec<TF> {
    let mut out = Vec::with_capacity(fs.len());
    for (i, f) in fs.iter().enumerate() {
        let to = |r: &[[f64; 2]]| -> Vec<V> {
            r.iter()
                .map(|p| {
                    let (x, y) = merc(p[0], p[1]);
                    V { x, y, imp: 0.0 }
                })
                .collect()
        };
        let (kind, mut rings, outer) = match &f.geom {
            Geom::Points(ps) => (Kind::Point, ps.iter().map(|p| to(std::slice::from_ref(p))).collect::<Vec<_>>(), vec![]),
            Geom::Lines(ls) => (Kind::Line, ls.iter().filter(|l| l.len() >= 2).map(|l| to(l)).collect(), vec![]),
            Geom::Polygons(ps) => {
                let mut rings = Vec::new();
                let mut outer = Vec::new();
                for poly in ps {
                    for (k, r) in poly.iter().enumerate() {
                        if r.len() >= 4 {
                            rings.push(to(r));
                            outer.push(k == 0);
                        }
                    }
                }
                (Kind::Polygon, rings, outer)
            }
        };
        if kind == Kind::Point {
            for r in rings.iter_mut() {
                r[0].imp = 1.0;
            }
        } else {
            for r in rings.iter_mut() {
                simplify(r);
            }
        }
        if rings.is_empty() {
            continue;
        }
        let bbox = bbox_of(&rings);
        out.push(TF { src: i as u32, kind, rings, outer, bbox });
    }
    out
}

/// Cuts a polyline to `lo ≤ coord(axis) ≤ hi`: the parts inside (a line can leave and come back).
fn clip_line(r: &[V], lo: f64, hi: f64, axis: usize) -> Vec<Vec<V>> {
    let c = |v: &V| if axis == 0 { v.x } else { v.y };
    let lerp = |a: V, b: V, t: f64| V { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, imp: 1.0 };
    let mut out = Vec::new();
    let mut cur: Vec<V> = Vec::new();
    for i in 0..r.len().saturating_sub(1) {
        let (a, b) = (r[i], r[i + 1]);
        let (ca, cb) = (c(&a), c(&b));
        // The stretch of a→b inside: t from t0 to t1.
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        if ca == cb {
            if ca < lo || ca > hi {
                (t0, t1) = (1.0, 0.0);
            }
        } else {
            let (ta, tb) = ((lo - ca) / (cb - ca), (hi - ca) / (cb - ca));
            t0 = t0.max(ta.min(tb));
            t1 = t1.min(ta.max(tb));
        }
        if t0 > t1 {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        if cur.is_empty() {
            cur.push(if t0 > 0.0 { lerp(a, b, t0) } else { a });
        }
        if t1 < 1.0 {
            cur.push(lerp(a, b, t1));
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(b);
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

/// Cuts a closed ring to `lo ≤ coord(axis) ≤ hi` (Sutherland–Hodgman on one axis): the ring
/// inside, closed, running along the cut where it was outside.
fn clip_ring(r: &[V], lo: f64, hi: f64, axis: usize) -> Vec<V> {
    let c = |v: &V| if axis == 0 { v.x } else { v.y };
    let at = |a: V, b: V, k: f64| -> V {
        let t = (k - c(&a)) / (c(&b) - c(&a));
        V { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, imp: 1.0 }
    };
    let mut ring: Vec<V> = r.to_vec();
    for (k, keep_above) in [(lo, true), (hi, false)] {
        let inside = |v: &V| if keep_above { c(v) >= k } else { c(v) <= k };
        let n = ring.len();
        if n == 0 {
            break;
        }
        let mut out = Vec::with_capacity(n + 4);
        for i in 0..n {
            let a = ring[i];
            let b = ring[(i + 1) % n];
            match (inside(&a), inside(&b)) {
                (true, true) => out.push(b),
                (true, false) => out.push(at(a, b, k)),
                (false, true) => {
                    out.push(at(a, b, k));
                    out.push(b);
                }
                (false, false) => {}
            }
        }
        ring = out;
    }
    // Closed again (the input's last vertex repeats its first).
    if let (Some(f), Some(l)) = (ring.first().copied(), ring.last().copied()) {
        if f.x != l.x || f.y != l.y {
            ring.push(f);
        }
    }
    ring
}

/// The features cut to `lo ≤ coord(axis) ≤ hi`.
fn clip(fs: &[TF], lo: f64, hi: f64, axis: usize) -> Vec<TF> {
    let mut out = Vec::new();
    for f in fs {
        let (a, b) = if axis == 0 { (f.bbox[0], f.bbox[2]) } else { (f.bbox[1], f.bbox[3]) };
        if a >= lo && b <= hi {
            out.push(f.clone());
            continue;
        }
        if b < lo || a > hi {
            continue;
        }
        let mut rings = Vec::new();
        let mut outer = Vec::new();
        match f.kind {
            Kind::Point => {
                for r in &f.rings {
                    let c = if axis == 0 { r[0].x } else { r[0].y };
                    if (lo..=hi).contains(&c) {
                        rings.push(r.clone());
                    }
                }
            }
            Kind::Line => {
                for r in &f.rings {
                    rings.extend(clip_line(r, lo, hi, axis));
                }
            }
            Kind::Polygon => {
                for (r, o) in f.rings.iter().zip(&f.outer) {
                    let c = clip_ring(r, lo, hi, axis);
                    if c.len() >= 4 {
                        rings.push(c);
                        outer.push(*o);
                    }
                }
            }
        }
        if rings.is_empty() {
            continue;
        }
        let bbox = bbox_of(&rings);
        out.push(TF { src: f.src, kind: f.kind, rings, outer, bbox });
    }
    out
}

/// One layer's tiles: `(z, x, y, MVT bytes)` for zooms `minz..=maxz` wherever a feature shows,
/// `want(z, x, y)` limiting which tiles are made (and descended into).
pub fn tiles(layer: &str, fs: &[Feature], minz: u8, maxz: u8, want: &(dyn Fn(u8, u32, u32) -> bool + Sync), emit: &mut dyn FnMut(u8, u32, u32, Vec<u8>)) {
    let prepared = prepare(fs);
    let mut stack: Vec<(u8, u32, u32, Vec<TF>)> = vec![(0, 0, 0, prepared)];
    while let Some((z, x, y, feats)) = stack.pop() {
        if feats.is_empty() || !want(z, x, y) {
            continue;
        }
        if z >= minz {
            if let Some(t) = encode(layer, fs, &feats, z, x, y) {
                emit(z, x, y, t);
            }
        }
        if z == maxz {
            continue;
        }
        // The four children, cut from this tile's features (with the buffer).
        let n = (1u64 << (z + 1)) as f64;
        let k = BUFFER / EXTENT as f64;
        for (dx, dy) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
            let (cx, cy) = (2 * x + dx, 2 * y + dy);
            let x0 = (cx as f64 - k) / n;
            let x1 = (cx as f64 + 1.0 + k) / n;
            let y0 = (cy as f64 - k) / n;
            let y1 = (cy as f64 + 1.0 + k) / n;
            let sub = clip(&clip(&feats, x0, x1, 0), y0, y1, 1);
            if !sub.is_empty() {
                stack.push((z + 1, cx, cy, sub));
            }
        }
    }
}

/// Signed area (shoelace) of a ring in tile units, y down: positive is clockwise on screen.
fn area(r: &[(i32, i32)]) -> f64 {
    let mut s = 0.0;
    for i in 0..r.len() {
        let (a, b) = (r[i], r[(i + 1) % r.len()]);
        s += a.0 as f64 * b.1 as f64 - b.0 as f64 * a.1 as f64;
    }
    s / 2.0
}

/// A line's or ring's vertices kept at a tolerance (its ends always), in tile units, without
/// repeats.
fn kept(r: &[V], tol2: f64, to: &dyn Fn(&V) -> (i32, i32)) -> Vec<(i32, i32)> {
    let mut pts: Vec<(i32, i32)> = Vec::with_capacity(r.len());
    for (i, v) in r.iter().enumerate() {
        if v.imp >= tol2 || i == 0 || i + 1 == r.len() {
            let p = to(v);
            if pts.last() != Some(&p) {
                pts.push(p);
            }
        }
    }
    pts
}

fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

/// A tile's features as MVT (None: nothing to draw at this zoom).
fn encode(layer: &str, src: &[Feature], feats: &[TF], z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let n = (1u64 << z) as f64;
    let e = EXTENT as f64;
    // Vertices kept at this zoom: importance at least the tolerance (squared, Mercator units).
    let tol = TOLERANCE / (n * e);
    let tol2 = tol * tol;
    let to = |v: &V| -> (i32, i32) { (((v.x * n - x as f64) * e).round() as i32, ((v.y * n - y as f64) * e).round() as i32) };
    let mut keys: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    let mut kix: HashMap<String, u32> = HashMap::new();
    let mut vix: HashMap<Value, u32> = HashMap::new();
    let mut out_feats = Vec::new();
    for f in feats {
        let s = &src[f.src as usize];
        if z < s.minzoom {
            continue;
        }
        let mut geom: Vec<u32> = Vec::new();
        let mut cur = (0i32, 0i32);
        let put = |g: &mut Vec<u32>, cur: &mut (i32, i32), p: (i32, i32)| {
            g.push(zigzag(p.0 - cur.0));
            g.push(zigzag(p.1 - cur.1));
            *cur = p;
        };
        let geom_type = match f.kind {
            Kind::Point => {
                let pts: Vec<(i32, i32)> = f.rings.iter().map(|r| to(&r[0])).collect();
                if pts.is_empty() {
                    continue;
                }
                geom.push(((pts.len() as u32) << 3) | 1);
                for p in pts {
                    put(&mut geom, &mut cur, p);
                }
                1
            }
            Kind::Line => {
                for r in &f.rings {
                    let pts = kept(r, tol2, &to);
                    if pts.len() < 2 {
                        continue;
                    }
                    geom.push((1 << 3) | 1);
                    put(&mut geom, &mut cur, pts[0]);
                    geom.push(((pts.len() as u32 - 1) << 3) | 2);
                    for &p in &pts[1..] {
                        put(&mut geom, &mut cur, p);
                    }
                }
                2
            }
            Kind::Polygon => {
                for (r, &outer) in f.rings.iter().zip(&f.outer) {
                    let mut pts = kept(r, tol2, &to);
                    // Without the closing repeat; rings that round away go.
                    while pts.len() > 1 && pts.first() == pts.last() {
                        pts.pop();
                    }
                    if pts.len() < 3 {
                        continue;
                    }
                    let a = area(&pts);
                    if a.abs() < 1.0 {
                        continue;
                    }
                    // Outer rings clockwise on screen (positive area), holes the other way.
                    if (a > 0.0) != outer {
                        pts.reverse();
                    }
                    geom.push((1 << 3) | 1);
                    put(&mut geom, &mut cur, pts[0]);
                    geom.push(((pts.len() as u32 - 1) << 3) | 2);
                    for &p in &pts[1..] {
                        put(&mut geom, &mut cur, p);
                    }
                    geom.push((1 << 3) | 7);
                }
                3
            }
        };
        if geom.is_empty() {
            continue;
        }
        let mut tags = Vec::with_capacity(s.props.len() * 2);
        for (k, v) in &s.props {
            let ki = *kix.entry(k.clone()).or_insert_with(|| {
                keys.push(k.clone());
                keys.len() as u32 - 1
            });
            let vi = *vix.entry(v.clone()).or_insert_with(|| {
                values.push(v.clone());
                values.len() as u32 - 1
            });
            tags.push(ki);
            tags.push(vi);
        }
        out_feats.push(MvtFeature { id: Some(s.id), tags, geom_type: Some(geom_type), geometry: geom, unknown: Vec::new() });
    }
    if out_feats.is_empty() {
        return None;
    }
    let t = Tile { layers: vec![Layer { name: layer.to_string(), version: 2, extent: EXTENT, keys, values, features: out_feats, unknown: Vec::new() }], unknown: Vec::new() };
    Some(t.encode())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(lon0: f64, lat0: f64, d: f64) -> Vec<[f64; 2]> {
        vec![[lon0, lat0], [lon0 + d, lat0], [lon0 + d, lat0 + d], [lon0, lat0 + d], [lon0, lat0]]
    }

    #[test]
    fn a_polygon_across_tiles() {
        let f = Feature { id: 7, geom: Geom::Polygons(vec![vec![square(-1.0, 50.0, 2.0)]]), props: vec![("n".into(), Value::String("a".into()))], minzoom: 0 };
        let mut got = Vec::new();
        tiles("t", &[f], 0, 6, &|_, _, _| true, &mut |z, x, y, b| got.push((z, x, y, b)));
        // Zoom 6: the square spans the 0° meridian, so two columns of tiles.
        let z6: Vec<_> = got.iter().filter(|t| t.0 == 6).collect();
        assert!(z6.len() >= 2, "{}", z6.len());
        for (_, _, _, b) in &z6 {
            let t = Tile::decode(b).unwrap();
            let l = &t.layers[0];
            assert_eq!(l.features[0].id, Some(7));
            assert_eq!(l.features[0].geom_type, Some(3));
            assert_eq!(l.keys, vec!["n".to_string()]);
        }
        // Every zoom 0–6 has the feature.
        for z in 0..=6 {
            assert!(got.iter().any(|t| t.0 == z), "zoom {z}");
        }
    }

    #[test]
    fn rings_wind_as_mvt_wants() {
        // Counter-clockwise outer (as GeoJSON's right-hand rule has it) comes out clockwise on screen.
        let outer = square(10.0, 10.0, 1.0);
        let mut hole = square(10.25, 10.25, 0.5);
        hole.reverse();
        let f = Feature { id: 1, geom: Geom::Polygons(vec![vec![outer, hole]]), props: vec![], minzoom: 0 };
        let mut got = Vec::new();
        tiles("t", &[f], 4, 4, &|_, _, _| true, &mut |z, x, y, b| got.push((z, x, y, b)));
        assert_eq!(got.len(), 1);
        let t = Tile::decode(&got[0].3).unwrap();
        let g = &t.layers[0].features[0].geometry;
        // Decode the rings and their signed areas.
        let mut rings: Vec<Vec<(i32, i32)>> = Vec::new();
        let (mut i, mut cur) = (0, (0i32, 0i32));
        let un = |v: u32| ((v >> 1) as i32) ^ -((v & 1) as i32);
        while i < g.len() {
            let (cmd, cnt) = (g[i] & 7, g[i] >> 3);
            i += 1;
            match cmd {
                1 => {
                    cur = (cur.0 + un(g[i]), cur.1 + un(g[i + 1]));
                    i += 2;
                    rings.push(vec![cur]);
                }
                2 => {
                    for _ in 0..cnt {
                        cur = (cur.0 + un(g[i]), cur.1 + un(g[i + 1]));
                        i += 2;
                        rings.last_mut().unwrap().push(cur);
                    }
                }
                _ => {}
            }
        }
        assert_eq!(rings.len(), 2);
        assert!(area(&rings[0]) > 0.0);
        assert!(area(&rings[1]) < 0.0);
    }

    #[test]
    fn a_line_leaving_and_coming_back() {
        let r = vec![V { x: 0.1, y: 0.5, imp: 1.0 }, V { x: 0.9, y: 0.5, imp: 1.0 }, V { x: 0.9, y: 0.6, imp: 1.0 }, V { x: 0.1, y: 0.6, imp: 1.0 }];
        let parts = clip_line(&r, 0.0, 0.5, 0);
        assert_eq!(parts.len(), 2);
        assert!((parts[0].last().unwrap().x - 0.5).abs() < 1e-12);
        assert!((parts[1].first().unwrap().x - 0.5).abs() < 1e-12);
    }
}
