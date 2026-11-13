//! The regions at a point, from the layer's tiles alone (docs/steps.md §15, the regions at a point).
//!
//! From the finest zoom up, the first tile holding the point that was made (one with a border line
//! within its buffer); in it, the line nearest the point, and the point's side of it: the regions
//! on that side. The segment from the point to its nearest line crosses no other line, so the
//! point is in the face that side bounds. A tile not made has no border within its buffer, so the
//! point is at least that far (a 64th of the tile) from every border, eight times the tolerance of
//! the zoom above, whose lines therefore put it on the right side. The answer is the regions as the
//! geometry has them, to the finest zoom's detail.

use super::{MAXZ, SOURCE_LAYER};
use anyhow::{Context, Result};
use names::mvt::Tile;

/// A tile's line part: its points in tile units (y down) and the regions left and right of it.
struct Part {
    pts: Vec<[f64; 2]>,
    l: String,
    r: String,
}

fn parts(raw: &[u8]) -> Result<(u32, Vec<Part>)> {
    let t = Tile::decode(raw)?;
    let Some(layer) = t.layers.iter().find(|l| l.name == SOURCE_LAYER) else { return Ok((0, Vec::new())) };
    let prop = |f: &names::mvt::Feature, key: &str| -> String {
        f.tags
            .chunks(2)
            .find(|kv| layer.keys.get(kv[0] as usize).is_some_and(|k| k == key))
            .and_then(|kv| layer.values.get(kv[1] as usize))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let un = |v: u32| ((v >> 1) as i32) ^ -((v & 1) as i32);
    let mut out = Vec::new();
    for f in &layer.features {
        let (l, r) = (prop(f, "l"), prop(f, "r"));
        let g = &f.geometry;
        let (mut i, mut cur) = (0usize, (0i32, 0i32));
        while i < g.len() {
            let (cmd, n) = (g[i] & 7, (g[i] >> 3) as usize);
            i += 1;
            match cmd {
                1 => {
                    let (dx, dy) = (*g.get(i).context("a short geometry")?, *g.get(i + 1).context("a short geometry")?);
                    cur = (cur.0 + un(dx), cur.1 + un(dy));
                    i += 2;
                    out.push(Part { pts: vec![[cur.0 as f64, cur.1 as f64]], l: l.clone(), r: r.clone() });
                }
                2 => {
                    let p = out.last_mut().context("a line without its start")?;
                    for _ in 0..n {
                        let (dx, dy) = (*g.get(i).context("a short geometry")?, *g.get(i + 1).context("a short geometry")?);
                        cur = (cur.0 + un(dx), cur.1 + un(dy));
                        i += 2;
                        p.pts.push([cur.0 as f64, cur.1 as f64]);
                    }
                }
                _ => {}
            }
        }
    }
    Ok((layer.extent, out))
}

/// The point on segment a–b nearest `p`: its parameter (0–1) and squared distance.
fn nearest(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) };
    let (x, y) = (a[0] + dx * t, a[1] + dy * t);
    (t, (p[0] - x).powi(2) + (p[1] - y).powi(2))
}

/// Which side of the lines in tile units (y down) `p` lies on: the regions there ("" for none), or
/// None with no line.
fn side(parts: &[Part], p: [f64; 2]) -> Option<String> {
    // (The nearest segment: its part, its start and the point on it.)
    let mut best: Option<(f64, usize, usize, f64)> = None;
    for (k, part) in parts.iter().enumerate() {
        for i in 1..part.pts.len() {
            let (t, d) = nearest(p, part.pts[i - 1], part.pts[i]);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, k, i - 1, t));
            }
        }
    }
    let (_, k, i, t) = best?;
    let part = &parts[k];
    let (a, b) = (part.pts[i], part.pts[i + 1]);
    // Left of a line running a → b on a map with north up: in tile units (y down), the cross
    // product is negative.
    let left = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) < 0.0;
    let pick = |part: &Part, l: bool| Some(if l { part.l.clone() } else { part.r.clone() });
    if t > 0.0 && t < 1.0 {
        return pick(part, left(a, b, p));
    }
    // The nearest point is a vertex: the lines leaving it, by angle (north up, counter-clockwise);
    // the first one turning counter-clockwise from the point's direction bounds its face, which is on
    // that line's right as it leaves the vertex.
    let v = if t <= 0.0 { a } else { b };
    let ang = |q: [f64; 2]| (-(q[1] - v[1])).atan2(q[0] - v[0]);
    let dir = ang(p);
    // (Each line leaving v: its angle, its part, and whether the part runs out from v.)
    let mut leaving: Vec<(f64, usize, bool)> = Vec::new();
    for (k, part) in parts.iter().enumerate() {
        for (j, &q) in part.pts.iter().enumerate() {
            if q != v {
                continue;
            }
            if let Some(&n) = part.pts.get(j + 1).filter(|&&n| n != v) {
                leaving.push((ang(n), k, true));
            }
            if let Some(&n) = j.checked_sub(1).and_then(|j| part.pts.get(j)).filter(|&&n| n != v) {
                leaving.push((ang(n), k, false));
            }
        }
    }
    if leaving.len() < 2 || p == v {
        // (A line's end, cut at the tile's buffer: its side as a straight line.)
        return pick(part, left(a, b, p));
    }
    let turn = |x: f64| (x - dir).rem_euclid(std::f64::consts::TAU);
    let &(_, k, out) = leaving.iter().min_by(|x, y| turn(x.0).total_cmp(&turn(y.0)))?;
    // (Right of the line as it leaves v: its right if it runs out from v, else its left.)
    pick(&parts[k], !out)
}

/// The regions at (`lon`, `lat`) by the layer's tiles (`get`: a tile's MVT bytes, not gzip'd, or None
/// for one not made), their ids; empty where no region is, or past the tiles (±85.05°).
pub fn regions_at(get: &mut dyn FnMut(u8, u32, u32) -> Result<Option<Vec<u8>>>, lon: f64, lat: f64) -> Result<Vec<String>> {
    if !(-roadcore::geo::MERCATOR_LAT..=roadcore::geo::MERCATOR_LAT).contains(&lat) {
        return Ok(Vec::new());
    }
    let (wx, wy) = roadcore::geo::merc(lon.clamp(-180.0, 180.0), lat);
    for z in (0..=MAXZ).rev() {
        let n = f64::from(1u32 << z);
        let (x, y) = (((wx * n).floor() as u32).min((1 << z) - 1), ((wy * n).floor() as u32).min((1 << z) - 1));
        let Some(raw) = get(z, x, y)? else { continue };
        let (extent, ps) = parts(&raw)?;
        let e = extent as f64;
        let p = [(wx * n - x as f64) * e, (wy * n - y as f64) * e];
        if let Some(s) = side(&ps, p) {
            return Ok(s.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect());
        }
    }
    Ok(Vec::new())
}
