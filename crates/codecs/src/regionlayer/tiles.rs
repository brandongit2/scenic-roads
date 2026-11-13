//! The borders as vector tiles (docs/steps.md §15, the tiles; docs/formats.md, The region layer).
//!
//! Zoom by zoom, each drawn arc's simplified segments are put in the tiles they pass within the
//! buffer of; a tile's lines are its arcs' runs of segments, clipped to the tile and its buffer
//! and to the plane (y within 0–1: nothing past ±85.05°, where the map has no ground, so a border
//! reaching a pole stops at the tiles' edge instead of running along it). Only arcs are drawn,
//! never a ring's outline, so a tile's edges, the cuts along ±180° within one region and the poles
//! never show. A tile no line reaches isn't made.
//!
//! A feature is an arc (or the part of it in the tile): a line string or several, its properties
//! `l` and `r` (the ids of the regions left and right of it as it runs, comma-joined; "" for none:
//! a coast with no region beyond) and `lv` (the level it separates, `Topology::level`). In tile
//! units y runs down, so "left" is as on a map with north up: the arc's left as it runs.

use super::simplify::Simplified;
use super::topo::Topology;
use super::{extent, BUFFER, MAXZ, SOURCE_LAYER};
use names::mvt::{Feature, Layer, Tile, Value};
use rayon::prelude::*;
use std::collections::HashMap;

/// A tile made: its zoom, x, y and MVT bytes (not gzip'd).
pub type Made = (u8, u32, u32, Vec<u8>);

/// The tiles (`x0 ≤ x < x1` … within the zoom) a segment passes within the buffer of, as (x, y).
fn tiles_of(p: [f64; 2], q: [f64; 2], n: f64, out: &mut Vec<(u32, u32)>) {
    let (p, q) = ([p[0] * n, p[1] * n], [q[0] * n, q[1] * n]);
    let max = n as i64 - 1;
    let (x0, x1) = (p[0].min(q[0]) - BUFFER, p[0].max(q[0]) + BUFFER);
    for c in (x0.floor() as i64).max(0)..=(x1.floor() as i64).min(max) {
        let (xa, xb) = ((c as f64 - BUFFER).max(p[0].min(q[0])), (c as f64 + 1.0 + BUFFER).min(p[0].max(q[0])));
        let y = |x: f64| if q[0] == p[0] { None } else { Some(p[1] + (q[1] - p[1]) * (x - p[0]) / (q[0] - p[0])) };
        let (ya, yb) = match (y(xa), y(xb)) {
            (Some(a), Some(b)) => (a.min(b), a.max(b)),
            _ => (p[1].min(q[1]), p[1].max(q[1])),
        };
        for r in ((ya - BUFFER).floor() as i64).max(0)..=((yb + BUFFER).floor() as i64).min(max) {
            out.push((c as u32, r as u32));
        }
    }
}

/// Segment a–b cut to the box (Liang–Barsky): its part inside, None when it misses.
fn clip(a: [f64; 2], b: [f64; 2], bx: [f64; 4]) -> Option<([f64; 2], [f64; 2])> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a[0] - bx[0]), (dx, bx[2] - a[0]), (-dy, a[1] - bx[1]), (dy, bx[3] - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
        if t0 > t1 {
            return None;
        }
    }
    let at = |t: f64| [a[0] + dx * t, a[1] + dy * t];
    Some((if t0 > 0.0 { at(t0) } else { a }, if t1 < 1.0 { at(t1) } else { b }))
}

fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

/// One tile's lines (`runs`: each an arc's index and its points in world units, in order) as MVT.
fn encode(topo: &Topology, z: u8, x: u32, y: u32, runs: &[(u32, Vec<[f64; 2]>)]) -> Option<Vec<u8>> {
    let n = f64::from(1u32 << z);
    let e = extent(z) as f64;
    let bx = [
        x as f64 / n - BUFFER / n,
        (y as f64 / n - BUFFER / n).max(0.0),
        (x + 1) as f64 / n + BUFFER / n,
        ((y + 1) as f64 / n + BUFFER / n).min(1.0),
    ];
    let to = |p: [f64; 2]| -> (i32, i32) { (((p[0] * n - x as f64) * e).round() as i32, ((p[1] * n - y as f64) * e).round() as i32) };
    let mut keys: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    let mut vix: HashMap<Value, u32> = HashMap::new();
    let mut by_arc: Vec<(u32, Vec<Vec<(i32, i32)>>)> = Vec::new();
    for (arc, pts) in runs {
        let mut parts: Vec<Vec<(i32, i32)>> = Vec::new();
        let mut cur: Vec<(i32, i32)> = Vec::new();
        for w in pts.windows(2) {
            match clip(w[0], w[1], bx) {
                Some((a, b)) => {
                    let (pa, pb) = (to(a), to(b));
                    if cur.last() != Some(&pa) {
                        if cur.len() >= 2 {
                            parts.push(std::mem::take(&mut cur));
                        }
                        cur = vec![pa];
                    }
                    if cur.last() != Some(&pb) {
                        cur.push(pb);
                    }
                }
                None => {
                    if cur.len() >= 2 {
                        parts.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                }
            }
        }
        if cur.len() >= 2 {
            parts.push(cur);
        }
        if parts.is_empty() {
            continue;
        }
        match by_arc.last_mut() {
            Some((a, ps)) if a == arc => ps.extend(parts),
            _ => by_arc.push((*arc, parts)),
        }
    }
    if by_arc.is_empty() {
        return None;
    }
    let mut features = Vec::with_capacity(by_arc.len());
    let mut value = |v: Value| -> u32 {
        *vix.entry(v.clone()).or_insert_with(|| {
            values.push(v);
            values.len() as u32 - 1
        })
    };
    keys.extend(["l", "r", "lv"].map(String::from));
    for (arc, parts) in by_arc {
        let a = &topo.arcs[arc as usize];
        let tags = vec![
            0,
            value(Value::String(topo.set_text(a.ld))),
            1,
            value(Value::String(topo.set_text(a.rd))),
            2,
            value(Value::Uint(topo.level(a) as u64)),
        ];
        let mut geom = Vec::new();
        let mut cur = (0i32, 0i32);
        for p in parts {
            geom.push((1 << 3) | 1);
            geom.push(zigzag(p[0].0 - cur.0));
            geom.push(zigzag(p[0].1 - cur.1));
            cur = p[0];
            geom.push(((p.len() as u32 - 1) << 3) | 2);
            for &q in &p[1..] {
                geom.push(zigzag(q.0 - cur.0));
                geom.push(zigzag(q.1 - cur.1));
                cur = q;
            }
        }
        features.push(Feature { id: None, tags, geom_type: Some(2), geometry: geom, unknown: Vec::new() });
    }
    let t = Tile {
        layers: vec![Layer {
            name: SOURCE_LAYER.to_string(),
            version: 2,
            extent: extent(z),
            keys,
            values,
            features,
            unknown: Vec::new(),
        }],
        unknown: Vec::new(),
    };
    Some(t.encode())
}

/// Zoom `z`'s tiles, each handed to `emit` (from several threads) as it's made.
pub fn zoom(topo: &Topology, s: &Simplified, z: u8, emit: &(dyn Fn(Made) + Sync)) -> usize {
    let n = f64::from(1u32 << z);
    // (Tile, arc, a segment's first kept point and its last.)
    let mut at: Vec<(u64, u32, u32, u32)> = s
        .lines
        .par_iter()
        .enumerate()
        .filter(|(k, _)| topo.arcs[*k].drawn)
        .flat_map_iter(|(k, l)| {
            let kept: Vec<usize> = l.kept(z).collect();
            let mut out = Vec::new();
            let mut ts = Vec::new();
            for w in kept.windows(2) {
                ts.clear();
                tiles_of(l.pts[w[0]], l.pts[w[1]], n, &mut ts);
                out.extend(ts.iter().map(|&(x, y)| (((x as u64) << 32) | y as u64, k as u32, w[0] as u32, w[1] as u32)));
            }
            out
        })
        .collect();
    at.par_sort_unstable();
    let mut runs = Vec::new();
    let mut s0 = 0;
    for i in 1..=at.len() {
        if i == at.len() || at[i].0 != at[s0].0 {
            runs.push((s0, i));
            s0 = i;
        }
    }
    runs.par_iter()
        .map(|&(a, b)| {
            let t = at[a].0;
            let (x, y) = ((t >> 32) as u32, t as u32);
            // (Each arc's segments in order, a run where each starts at the last one's end.)
            let mut lines: Vec<(u32, Vec<[f64; 2]>)> = Vec::new();
            let mut last: Option<(u32, u32)> = None;
            for &(_, arc, i, j) in &at[a..b] {
                let l = &s.lines[arc as usize];
                if last != Some((arc, i)) {
                    lines.push((arc, vec![l.pts[i as usize]]));
                }
                lines.last_mut().unwrap().1.push(l.pts[j as usize]);
                last = Some((arc, j));
            }
            match encode(topo, z, x, y, &lines) {
                Some(b) => {
                    emit((z, x, y, b));
                    1
                }
                None => 0,
            }
        })
        .sum()
}

/// Every zoom's tiles (`zoom`), zoom by zoom.
pub fn all(topo: &Topology, s: &Simplified, emit: &(dyn Fn(Made) + Sync), report: &dyn Fn(u8)) -> usize {
    let mut n = 0;
    for z in 0..=MAXZ {
        n += zoom(topo, s, z, emit);
        report(z);
    }
    n
}
