//! The buildings' vector tiles (docs/buildings3d.md §3.4): MVT 2.1 over `names::mvt`, gzip'd, extent
//! 4096, one layer `b`, a feature a building or part.
//!
//! - A feature is whole in the tile holding its centroid (not clipped: its coordinates may run past
//!   the extent), so MapLibre's extrusion has one centroid a building and nothing is drawn twice.
//! - Quantized to the tile's grid (z14: 0.6 m at the equator), at z12–13 simplified to one grid
//!   unit first (Douglas–Peucker); each ring's repeated points dropped, a ring of fewer than three
//!   points or no area dropped (an exterior with its holes). Exteriors wind positive (y down), holes
//!   negative.
//! - Properties: `h` the top and `m` the base (dm; `m` left out when 0), `s` where the height comes
//!   from (0–5), `f` the floors (when `s` is 1), `c` the kind, `k` (1 a part, 2 an outline with
//!   parts; left out when 0). No feature ids.
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
pub const KEYS: [&str; 6] = ["h", "m", "s", "f", "c", "k"];

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
            Feat { polys: vec![vec![&outer[..], &hole[..]]], cen: [e7(lon + 0.00025), e7(lat + 0.00025)], order: (1, 0), h: 215, m: 0, s: 0, f: 0, c: 1, k: 0 },
            Feat { polys: vec![vec![&other[..]]], cen: [e7(lon + 0.00315), e7(lat + 0.00315)], order: (1, 1), h: 64, m: 30, s: 1, f: 2, c: 3, k: 1 },
            Feat { polys: vec![vec![&sliver[..]]], cen: [e7(lon + 0.006), e7(lat + 0.0001)], order: (1, 2), h: 40, m: 0, s: 5, f: 0, c: 0, k: 0 },
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
