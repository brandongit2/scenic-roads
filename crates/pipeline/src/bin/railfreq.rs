//! Passenger rail service frequency on the OSM track network: trains a day each way per rail way.
//!
//! usage: railfreq <build_dir> <pairs.bin>...
//!
//! Input pairs (dem/railgtfs.py, and the hand-researched MTR lines): little-endian records
//! f32 lon_a, lat_a, lon_b, lat_b, u8 mode (0 tram, 1 metro, 2 rail, 3 funicular; bit 0x80: the
//! count is a lower bound; 0x20 / 0x40: stop A / B is beyond the map), f32 trains — the trains from stop A to stop B on a typical weekday. Each pair is matched onto the rail ways
//! of ways.bin: each stop snaps to its nearest few tracks within 300 m, or 1 km when there are none
//! that close (a stop between the two tracks of a double-track line may be nearest the wrong one), then a shortest path runs along
//! the track graph from any of A's to any of B's (junctions and way ends as nodes; tracks of
//! another kind of service cost 4× so paths keep to the right network; dangling track ends are
//! bridged to other tracks within 100 m, since only tracks used by route relations are in
//! ways.bin and relations skip bits of station throats), at most 3× the straight distance + 3 km.
//! Every way on the path gets the pair's trains. When one stop is beyond the map's regions (flagged
//! by railgtfs; a cross-border service, at most 300 km on) and no track is near it, the path runs
//! from the other to the dead-end track nearest it, when that end is at most 0.8× the pair's
//! distance from it.
//!
//! A path takes one track of a multi-track line (OSM maps each track as its own way), so a
//! track's own count says little: the trains are then added up across the corridor. At a few
//! points along each way, every other track running parallel within 40 m (same kind of service,
//! not just touching end to end) is found, and the way gets the sum of their counts (the median
//! over its points); parallel tracks with no trains of their own get the corridor's too.
//!
//! Output: rail-freq.bin, sorted (u32 way index, f32 trains a day each way — both directions added
//! and halved; negative when any of it is a lower bound, i.e. "at least") for the rail ways with
//! any trains.

use anyhow::Result;
use rayon::prelude::*;
use roadcore::{class, dist_m, Ways, E7};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;

const SNAP_M: f64 = 300.0;
const SNAP_FAR_M: f64 = 1000.0;
/// A track end counts toward a stop beyond the tracks when it is at most this share of the pair's
/// straight distance from it.
const BEYOND_SHARE: f64 = 0.8;
const BEYOND_MAX_M: f64 = 300_000.0;
const GAP_M: f64 = 100.0;
const SNAP_CANDIDATES: usize = 4;
const OFF_MODE: f32 = 4.0;
const CORRIDOR_M: f64 = 40.0;
const CELL: f64 = 0.002;

#[derive(Clone, Copy)]
struct Pair {
    a: [f64; 2],
    b: [f64; 2],
    mode: u8,
    n: f32,
    lower: bool,
    /// 1: stop A is beyond the map, 2: stop B is.
    beyond: u8,
}

/// A stretch of one way between two graph nodes (way u32::MAX: a virtual link across a gap).
struct Edge {
    u: u32,
    v: u32,
    len: f32,
    way: u32,
    /// Rail groups using the way (bit k = class TRAM + k).
    groups: u8,
}

/// Where a vertex sits: on which edge, how far from its u end.
#[derive(Clone, Copy)]
struct At {
    edge: u32,
    off: f32,
}

fn mode_bits(mode: u8) -> u8 {
    // Groups: bit0 tram, bit1 metro, bit2 commuter, bit3 intercity, bit4 heritage & mountain.
    match mode {
        0 => 0b10011, // GTFS tram / streetcar / light rail: OSM light_rail is in the metro group
        1 => 0b00011,
        2 => 0b11100,
        _ => 0b10000,
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 3, "usage: railfreq <build_dir> <pairs.bin>...");
    let dir = PathBuf::from(&args[1]);
    let t0 = std::time::Instant::now();
    let mut pairs: Vec<Pair> = Vec::new();
    for p in &args[2..] {
        let b = std::fs::read(p)?;
        for r in b.chunks_exact(21) {
            let f = |i: usize| f32::from_le_bytes(r[i..i + 4].try_into().unwrap()) as f64;
            pairs.push(Pair { a: [f(0), f(4)], b: [f(8), f(12)], mode: r[16] & 0x1f, n: f(17) as f32, lower: r[16] & 0x80 != 0, beyond: (r[16] >> 5) & 3 });
        }
    }
    let located = |p: [f64; 2]| p[0].abs() > 0.01 || p[1].abs() > 0.01; // 0,0: no location
    pairs.retain(|p| located(p.a) && located(p.b));
    eprintln!("{} stop pairs", pairs.len());

    let wv = Ways::open(&dir)?;
    let ways = wv.ways();
    let verts = wv.verts();
    let rail: Vec<u32> = (0..ways.len() as u32).filter(|&i| (class::TRAM..=class::HERITAGE).contains(&ways[i as usize].class)).collect();
    // Graph nodes: way ends and vertices shared by several rail ways (junctions).
    let mut seen: HashMap<[i32; 2], u32> = HashMap::new();
    for &w in &rail {
        let wr = &ways[w as usize];
        let v = &verts[wr.vstart as usize..(wr.vstart + wr.vcount as u64) as usize];
        for (k, p) in v.iter().enumerate() {
            let c = seen.entry(*p).or_insert(0);
            *c += if k == 0 || k + 1 == v.len() { 2 } else { 1 };
        }
    }
    let mut node_of: HashMap<[i32; 2], u32> = HashMap::new();
    for (p, c) in &seen {
        if *c >= 2 {
            let id = node_of.len() as u32;
            node_of.insert(*p, id);
        }
    }
    drop(seen);
    let mut edges: Vec<Edge> = Vec::new();
    // Per rail vertex (for snapping): position, edge, offset, groups.
    let mut grid: HashMap<(i32, i32), Vec<(f64, f64, At, u8)>> = HashMap::new();
    for &w in &rail {
        let wr = &ways[w as usize];
        let v = &verts[wr.vstart as usize..(wr.vstart + wr.vcount as u64) as usize];
        if v.len() < 2 {
            continue;
        }
        let mut start = 0usize;
        let mut acc = 0.0f64;
        let mut pending: Vec<(f64, f64, f64)> = vec![(v[0][0] as f64 * E7, v[0][1] as f64 * E7, 0.0)];
        for k in 1..v.len() {
            let (x0, y0, x1, y1) = (v[k - 1][0] as f64 * E7, v[k - 1][1] as f64 * E7, v[k][0] as f64 * E7, v[k][1] as f64 * E7);
            acc += dist_m(x0, y0, x1, y1);
            pending.push((x1, y1, acc));
            if node_of.contains_key(&v[k]) || k + 1 == v.len() {
                let e = edges.len() as u32;
                edges.push(Edge { u: node_of[&v[start]], v: node_of[&v[k]], len: acc as f32, way: w, groups: wr.rail });
                for &(x, y, off) in &pending {
                    grid.entry(((x / CELL).floor() as i32, (y / CELL).floor() as i32)).or_default().push((x, y, At { edge: e, off: off as f32 }, wr.rail));
                }
                start = k;
                acc = 0.0;
                pending = vec![(x1, y1, 0.0)];
            }
        }
    }
    let n_nodes = node_of.len();
    drop(node_of);
    // Node positions (for gap bridging).
    let mut node_pos = vec![[0f64; 2]; n_nodes];
    for vs in grid.values() {
        for &(x, y, at, _) in vs {
            let e = &edges[at.edge as usize];
            if at.off == 0.0 {
                node_pos[e.u as usize] = [x, y];
            } else if (at.off - e.len).abs() < 1e-3 {
                node_pos[e.v as usize] = [x, y];
            }
        }
    }
    let mut degree = vec![0u32; n_nodes];
    for e in &edges {
        degree[e.u as usize] += 1;
        degree[e.v as usize] += 1;
    }
    let mut node_grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, p) in node_pos.iter().enumerate() {
        node_grid.entry(((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32)).or_default().push(i as u32);
    }
    let n_real = edges.len();
    for i in 0..n_nodes {
        if degree[i] != 1 {
            continue;
        }
        let p = node_pos[i];
        let (cx, cy) = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in node_grid.get(&(cx + dx, cy + dy)).map(Vec::as_slice).unwrap_or(&[]) {
                    if j as usize == i {
                        continue;
                    }
                    let q = node_pos[j as usize];
                    let d = dist_m(p[0], p[1], q[0], q[1]);
                    if d <= GAP_M {
                        edges.push(Edge { u: i as u32, v: j, len: (d * 2.0) as f32, way: u32::MAX, groups: 0xff });
                    }
                }
            }
        }
    }
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n_nodes];
    for (i, e) in edges.iter().enumerate() {
        adj[e.u as usize].push(i as u32);
        adj[e.v as usize].push(i as u32);
    }
    eprintln!("gap links: {}", edges.len() - n_real);
    eprintln!("rail graph: {} ways, {} nodes, {} edges ({:.0?})", rail.len(), n_nodes, edges.len(), t0.elapsed());

    // The nearest point of each nearby track (way), right kind of track first, up to a few; with
    // the distance from the stop, which is added to the path cost.
    let snap_within = |p: [f64; 2], bits: u8, max_m: f64| -> Vec<(At, f32)> {
        let (cx, cy) = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
        let rx = (max_m / (CELL * 111_320.0 * p[1].to_radians().cos().max(0.2))).ceil() as i32;
        let ry = (max_m / (CELL * 110_570.0)).ceil() as i32;
        let mut per_way: HashMap<u32, (f64, bool, At)> = HashMap::new();
        for dx in -rx..=rx {
            for dy in -ry..=ry {
                if let Some(vs) = grid.get(&(cx + dx, cy + dy)) {
                    for &(x, y, at, g) in vs {
                        let d = dist_m(p[0], p[1], x, y);
                        if d > max_m {
                            continue;
                        }
                        let good = g & bits != 0;
                        let way = edges[at.edge as usize].way;
                        let e = per_way.entry(way).or_insert((f64::MAX, good, at));
                        if d < e.0 {
                            *e = (d, good, at);
                        }
                    }
                }
            }
        }
        let mut c: Vec<(f64, bool, At)> = per_way.into_values().collect();
        c.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.total_cmp(&b.0)));
        c.truncate(SNAP_CANDIDATES);
        c.into_iter().map(|(d, _, at)| (at, d as f32)).collect()
    };
    // A stop with no track near it (a rural station whose route relation runs along another track
    // of the line) takes the nearest within SNAP_FAR_M.
    let snap = |p: [f64; 2], bits: u8| -> Vec<(At, f32)> {
        let c = snap_within(p, bits, SNAP_M);
        if c.is_empty() { snap_within(p, bits, SNAP_FAR_M) } else { c }
    };

    let debug = std::env::var("RAILFREQ_DEBUG").is_ok();
    let n_rail = rail.len();
    let rail_index: HashMap<u32, u32> = rail.iter().enumerate().map(|(i, &w)| (w, i as u32)).collect();
    let (sums, lower, matched, partial) = pairs
        .par_chunks(2048)
        .map(|chunk| {
            let mut sums = vec![0f32; n_rail];
            let mut lower = vec![false; n_rail];
            let mut matched = 0usize;
            let mut partial = 0usize;
            let mut dist: HashMap<u32, f32> = HashMap::new();
            let mut prev: HashMap<u32, u32> = HashMap::new(); // node → edge taken to reach it
            let mut src: HashMap<u32, u32> = HashMap::new(); // start node → A's stretch it came from
            for p in chunk {
                let bits = mode_bits(p.mode);
                let (ca, cb) = (snap(p.a, bits), snap(p.b, bits));
                // One stop with no track near it (beyond the map, on a cross-border service): the
                // trains run from the other as far as the track goes toward it.
                let near = dist_m(p.a[0], p.a[1], p.b[0], p.b[1]) <= BEYOND_MAX_M;
                let (ca, cb, beyond) = match (ca.is_empty(), cb.is_empty()) {
                    (false, false) => (ca, cb, None),
                    (false, true) if near && p.beyond == 2 => (ca, cb, Some(p.b)),
                    (true, false) if near && p.beyond == 1 => (cb, ca, Some(p.a)),
                    _ => {
                        if debug {
                            eprintln!("MISS snap {} {:.5},{:.5} {:.5},{:.5} {}", p.mode, p.a[0], p.a[1], p.b[0], p.b[1], p.n);
                        }
                        continue;
                    }
                };
                let cost = |e: &Edge, len: f32| if e.groups & bits != 0 { len } else { len * OFF_MODE };
                let add = |e: u32, sums: &mut Vec<f32>, lower: &mut Vec<bool>| {
                    let w = edges[e as usize].way;
                    if w != u32::MAX {
                        let i = rail_index[&w] as usize;
                        sums[i] += p.n;
                        lower[i] |= p.lower;
                    }
                };
                // Both stops on the same stretch of track.
                if let Some(&(a, _)) = ca.iter().find(|(a, _)| cb.iter().any(|(b, _)| b.edge == a.edge)) {
                    add(a.edge, &mut sums, &mut lower);
                    matched += 1;
                    continue;
                }
                let straight = dist_m(p.a[0], p.a[1], p.b[0], p.b[1]);
                let limit = (straight * 3.0 + 3000.0) as f32;
                dist.clear();
                prev.clear();
                src.clear();
                let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new(); // (cost as bits, node)
                for &(a, d0) in &ca {
                    let ea = &edges[a.edge as usize];
                    for (node, c) in [(ea.u, d0 + cost(ea, a.off)), (ea.v, d0 + cost(ea, ea.len - a.off))] {
                        if dist.get(&node).is_none_or(|&d| c < d) {
                            dist.insert(node, c);
                            src.insert(node, a.edge);
                            heap.push(Reverse((c.to_bits(), node)));
                        }
                    }
                }
                // Reaching either end of one of B's stretches, plus the rest of it.
                let finish = |node: u32, d: f32| -> (f32, u32) {
                    let mut best = (f32::INFINITY, u32::MAX);
                    for &(b, d0) in &cb {
                        let eb = &edges[b.edge as usize];
                        let f = if node == eb.u { d + cost(eb, b.off) + d0 } else if node == eb.v { d + cost(eb, eb.len - b.off) + d0 } else { f32::INFINITY };
                        if f < best.0 {
                            best = (f, b.edge);
                        }
                    }
                    best
                };
                let mut best = (f32::INFINITY, u32::MAX, u32::MAX); // cost, node, B's edge
                let mut end = (BEYOND_SHARE * straight, u32::MAX); // beyond: the track end nearest the stop
                while let Some(Reverse((cbits, u))) = heap.pop() {
                    let c = f32::from_bits(cbits);
                    if c > limit || c >= best.0 {
                        break;
                    }
                    if dist.get(&u).is_some_and(|&d| c > d) {
                        continue;
                    }
                    let (f, eb) = finish(u, c);
                    if f < best.0 {
                        best = (f, u, eb);
                    }
                    if let Some(t) = beyond {
                        if adj[u as usize].len() == 1 {
                            let d = dist_m(node_pos[u as usize][0], node_pos[u as usize][1], t[0], t[1]);
                            if d < end.0 && c + d as f32 <= limit {
                                end = (d, u);
                            }
                        }
                    }
                    for &ei in &adj[u as usize] {
                        let e = &edges[ei as usize];
                        let v = if e.u == u { e.v } else { e.u };
                        let nc = c + cost(e, e.len);
                        if nc <= limit && dist.get(&v).is_none_or(|&d| nc < d) {
                            dist.insert(v, nc);
                            prev.insert(v, ei);
                            src.remove(&v);
                            heap.push(Reverse((nc.to_bits(), v)));
                        }
                    }
                }
                if beyond.is_some() {
                    best.1 = end.1;
                }
                if best.1 == u32::MAX {
                    if debug {
                        let why = if beyond.is_some() { "snap" } else { "path" };
                        eprintln!("MISS {why} {} {:.5},{:.5} {:.5},{:.5} {}", p.mode, p.a[0], p.a[1], p.b[0], p.b[1], p.n);
                    }
                    continue;
                }
                matched += 1;
                if beyond.is_some() {
                    partial += 1;
                    if debug {
                        let q = node_pos[best.1 as usize];
                        eprintln!("BEYOND {} {:.5},{:.5} {:.5},{:.5} {} → end {:.5},{:.5}", p.mode, p.a[0], p.a[1], p.b[0], p.b[1], p.n, q[0], q[1]);
                    }
                } else {
                    add(best.2, &mut sums, &mut lower);
                }
                let mut n = best.1;
                let mut guard = 0;
                loop {
                    if let Some(&se) = src.get(&n) {
                        add(se, &mut sums, &mut lower);
                        break;
                    }
                    let Some(&ei) = prev.get(&n) else { break };
                    add(ei, &mut sums, &mut lower);
                    let e = &edges[ei as usize];
                    n = if e.u == n { e.v } else { e.u };
                    guard += 1;
                    if guard > 100_000 {
                        break;
                    }
                }
            }
            (sums, lower, matched, partial)
        })
        .reduce(
            || (vec![0f32; n_rail], vec![false; n_rail], 0usize, 0usize),
            |(mut a, mut la, ma, pa), (b, lb, mb, pb)| {
                a.iter_mut().zip(&b).for_each(|(x, y)| *x += y);
                la.iter_mut().zip(&lb).for_each(|(x, y)| *x |= y);
                (a, la, ma + mb, pa + pb)
            },
        );
    eprintln!("{partial} pairs with one stop beyond the tracks run to the nearest track end toward it");
    let (sums, lower) = corridors(&rail, ways, verts, &sums, &lower);
    let mut out: Vec<u8> = Vec::new();
    let mut n_ways = 0usize;
    for (i, &s) in sums.iter().enumerate() {
        if s > 0.0 {
            out.extend_from_slice(&rail[i].to_le_bytes());
            out.extend_from_slice(&(if lower[i] { -s / 2.0 } else { s / 2.0 }).to_le_bytes());
            n_ways += 1;
        }
    }
    std::fs::write(roadcore::tmp(&dir, "rail-freq.bin"), &out)?;
    roadcore::commit(&dir, &["rail-freq.bin"])?;
    let km_with = rail.iter().zip(&sums).filter(|(_, &s)| s > 0.0).map(|(&w, _)| {
        let wr = &ways[w as usize];
        let v = &verts[wr.vstart as usize..(wr.vstart + wr.vcount as u64) as usize];
        v.windows(2).map(|p| dist_m(p[0][0] as f64 * E7, p[0][1] as f64 * E7, p[1][0] as f64 * E7, p[1][1] as f64 * E7)).sum::<f64>()
    }).sum::<f64>() / 1000.0;
    eprintln!(
        "railfreq: {}/{} pairs matched ({:.0} %), {} of {} rail ways ({:.0} km) have trains ({:.0?})",
        matched,
        pairs.len(),
        matched as f64 / pairs.len().max(1) as f64 * 100.0,
        n_ways,
        n_rail,
        km_with,
        t0.elapsed()
    );
    Ok(())
}

/// Trains summed across each corridor: per way, at 25/50/75 % of its length, the counts of every
/// track (itself included) running parallel within CORRIDOR_M whose nearest point there is not
/// one of its ends; the median of those sums. Lower-bound flags carry over likewise.
fn corridors(rail: &[u32], ways: &[roadcore::WayRec], verts: &[[i32; 2]], sums: &[f32], lower: &[bool]) -> (Vec<f32>, Vec<bool>) {
    let pts = |i: usize| -> Vec<[f64; 2]> {
        let wr = &ways[rail[i] as usize];
        verts[wr.vstart as usize..(wr.vstart + wr.vcount as u64) as usize].iter().map(|p| [p[0] as f64 * E7, p[1] as f64 * E7]).collect()
    };
    // Segment grid: (way index, segment index).
    let mut grid: HashMap<(i32, i32), Vec<(u32, u32)>> = HashMap::new();
    for i in 0..rail.len() {
        let v = pts(i);
        for k in 0..v.len().saturating_sub(1) {
            let (a, b) = (v[k], v[k + 1]);
            let (x0, x1) = ((a[0].min(b[0]) / CELL).floor() as i32, (a[0].max(b[0]) / CELL).floor() as i32);
            let (y0, y1) = ((a[1].min(b[1]) / CELL).floor() as i32, (a[1].max(b[1]) / CELL).floor() as i32);
            for cx in x0..=x1 {
                for cy in y0..=y1 {
                    grid.entry((cx, cy)).or_default().push((i as u32, k as u32));
                }
            }
        }
    }
    let groups: Vec<u8> = rail.iter().map(|&w| ways[w as usize].rail).collect();
    let res: Vec<(f32, bool)> = (0..rail.len())
        .into_par_iter()
        .map(|i| {
            let v = pts(i);
            if v.len() < 2 {
                return (sums[i], lower[i]);
            }
            // Local metres per degree.
            let ky = 111_320.0;
            let kx = ky * (v[0][1].to_radians()).cos();
            let seg_len: Vec<f64> = v.windows(2).map(|p| (((p[1][0] - p[0][0]) * kx).powi(2) + ((p[1][1] - p[0][1]) * ky).powi(2)).sqrt()).collect();
            let total: f64 = seg_len.iter().sum();
            let mut samples: Vec<(f32, bool)> = Vec::new();
            for f in [0.25, 0.5, 0.75] {
                // The point at fraction f of the way, and its direction.
                let mut d = total * f;
                let mut k = 0;
                while k + 1 < seg_len.len() && d > seg_len[k] {
                    d -= seg_len[k];
                    k += 1;
                }
                let t = if seg_len[k] > 0.0 { d / seg_len[k] } else { 0.0 };
                let p = [v[k][0] + (v[k + 1][0] - v[k][0]) * t, v[k][1] + (v[k + 1][1] - v[k][1]) * t];
                let dir = [(v[k + 1][0] - v[k][0]) * kx, (v[k + 1][1] - v[k][1]) * ky];
                let dn = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt().max(1e-9);
                // Nearest point of each other way near p: (distance, parallel, interior).
                let mut near: HashMap<u32, (f64, bool, bool)> = HashMap::new();
                let (cx, cy) = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for &(j, sk) in grid.get(&(cx + dx, cy + dy)).map(Vec::as_slice).unwrap_or(&[]) {
                            if j as usize == i || groups[j as usize] & groups[i] == 0 {
                                continue;
                            }
                            let wr = &ways[rail[j as usize] as usize];
                            let base = wr.vstart as usize;
                            let (a, b) = (verts[base + sk as usize], verts[base + sk as usize + 1]);
                            let (a, b) = ([a[0] as f64 * E7, a[1] as f64 * E7], [b[0] as f64 * E7, b[1] as f64 * E7]);
                            let s = [(b[0] - a[0]) * kx, (b[1] - a[1]) * ky];
                            let q = [(p[0] - a[0]) * kx, (p[1] - a[1]) * ky];
                            let ss = s[0] * s[0] + s[1] * s[1];
                            let u = if ss > 0.0 { ((q[0] * s[0] + q[1] * s[1]) / ss).clamp(0.0, 1.0) } else { 0.0 };
                            let dist = ((q[0] - s[0] * u).powi(2) + (q[1] - s[1] * u).powi(2)).sqrt();
                            if dist > CORRIDOR_M {
                                continue;
                            }
                            let cos = ((s[0] * dir[0] + s[1] * dir[1]) / (ss.sqrt().max(1e-9) * dn)).abs();
                            let last = wr.vcount as usize - 2;
                            let end = (sk == 0 && u <= 1e-6) || (sk as usize == last && u >= 1.0 - 1e-6);
                            let e = near.entry(j).or_insert((f64::MAX, false, false));
                            if dist < e.0 {
                                *e = (dist, cos > 0.94, !end);
                            }
                        }
                    }
                }
                let mut sum = sums[i];
                let mut lo = lower[i];
                for (&j, &(_, parallel, interior)) in &near {
                    if parallel && interior {
                        sum += sums[j as usize];
                        lo |= lower[j as usize] && sums[j as usize] > 0.0;
                    }
                }
                samples.push((sum, lo));
            }
            samples.sort_by(|a, b| a.0.total_cmp(&b.0));
            samples[samples.len() / 2]
        })
        .collect();
    let n_gain = res.iter().zip(sums).filter(|((c, _), &s)| *c > 0.0 && s <= 0.0).count();
    eprintln!("corridors: {} tracks without trains of their own get their corridor's", n_gain);
    res.into_iter().unzip()
}
