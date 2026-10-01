//! Passenger rail service frequency on the OSM track network: trains a day each way per rail way.
//!
//! usage: railfreq <build_dir> <pairs.bin>...
//!
//! Input pairs (dem/railgtfs.py, and the hand-researched MTR lines): little-endian records
//! f32 lon_a, lat_a, lon_b, lat_b, u8 mode (0 tram, 1 metro, 2 rail, 3 funicular; bit 0x80: the
//! count is a lower bound; 0x20 / 0x40: stop A / B is beyond the map), f32 trains — the trains from stop A to stop B on a typical weekday. Each pair is matched onto the rail ways
//! of ways.bin: each stop snaps to its nearest few tracks within 300 m, or 1 km when there are none
//! that close (a stop between the two tracks of a double-track line may be nearest the wrong one),
//! at the foot of the perpendicular from the stop (so on every track the stop is at the same cross-section: a
//! train arriving on one track and leaving on another isn't counted twice there), then a shortest path runs along
//! the track graph from any of A's to any of B's (junctions and way ends as nodes; tracks of
//! another kind of service cost 4× so paths keep to the right network; dangling track ends are
//! bridged to other tracks within 100 m, since only tracks used by route relations are in
//! ways.bin and relations skip bits of station throats), at most 3× the straight distance + 3 km.
//! The search follows the direction of travel: a train doesn't turn back between two stops, so
//! leaving a node more than 90° off its heading (60° into or out of a gap link) costs 3 km. Before
//! that, a stop in a big station whose nearest tracks didn't connect onward sent every train out
//! along them and back through a gap link, past the station twice (Clapham Junction counted each
//! train about twice). A pair with no such path along its stops' nearest 4 tracks tries their 12
//! nearest, the distance from the stop then counted double so the path doesn't end on a track
//! short of the station.
//! The pair's trains run over the path: the stretches of track from stop A to stop B, the first
//! and last only in part, so every point of the track has its trains (a way is often longer than
//! the gap between stations: a metro line's track can be one 15 km way, and adding up the pairs on
//! it would count each train once per station). When one stop is beyond the map's regions (flagged
//! by railgtfs; a cross-border service, at most 300 km on) and no track is near it, the path runs
//! from the other to the dead-end track nearest it, when that end is at most 0.8× the pair's
//! distance from it.
//!
//! A path takes one track of a multi-track line (OSM maps each track as its own way), not always
//! its direction's, so a track's own count says little: the trains are then added up across the
//! corridor. At a few points along each way, every other track running parallel within 40 m (same
//! kind of service, not just touching end to end) is found, and the way gets the sum of their
//! trains at that cross-section (the median over its points; each train is on one of the tracks
//! there); parallel tracks with no trains of their own get the corridor's too.
//!
//! Debugging: RAILFREQ_AT=lon,lat,metres prints every rail way there with its trains and its
//! stretches' coverage; RAILFREQ_WAY=<OSM way id> the pairs whose trains run on that way and their
//! paths; RAILFREQ_STOP=lon,lat the paths of the pairs from or to a stop there; RAILFREQ_DEBUG the
//! pairs that found no track or path.
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
/// Tracks a stop snaps to, and the weight on its distance from them: then, for a pair with no path
/// along those, more (see the pair loop).
const ATTEMPTS: [(usize, f64); 2] = [(4, 1.0), (12, 2.0)];
/// The cost of turning back at a node (a switch or a gap): a train doesn't reverse between two
/// stops, but a path that can't get through otherwise still may.
const REVERSE_M: f32 = 3000.0;
/// A stretch's heading at each end: toward its first vertex at least this far along.
const DIR_M: f32 = 15.0;
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
    /// Its heading leaving u, and leaving v (unit vectors, metres east and north).
    du: [f32; 2],
    dv: [f32; 2],
}

/// The unit vector from a to b (lon/lat), in metres east and north.
fn heading(a: [f64; 2], b: [f64; 2]) -> [f32; 2] {
    let (dx, dy) = ((b[0] - a[0]) * 111_320.0 * a[1].to_radians().cos(), (b[1] - a[1]) * 110_570.0);
    let n = (dx * dx + dy * dy).sqrt().max(1e-9);
    [(dx / n) as f32, (dy / n) as f32]
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
    // Per rail way: its stretches (first edge, count: made in order along it) and the distance
    // along it at each vertex, for the trains at a point of it.
    let mut way_edges: Vec<(u32, u32)> = Vec::with_capacity(rail.len());
    let mut along: Vec<Vec<f32>> = Vec::with_capacity(rail.len());
    // Per edge: its rail way (index) and where along it the edge starts.
    let mut edge_way: Vec<(u32, f32)> = Vec::new();
    for (ri, &w) in rail.iter().enumerate() {
        let wr = &ways[w as usize];
        let v = &verts[wr.vstart as usize..(wr.vstart + wr.vcount as u64) as usize];
        let first = edges.len() as u32;
        let mut pre = vec![0f32; v.len()];
        if v.len() < 2 {
            way_edges.push((first, 0));
            along.push(pre);
            continue;
        }
        let mut start = 0usize;
        let mut acc = 0.0f64;
        let mut total = 0.0f64;
        let mut pending: Vec<(f64, f64, f64)> = vec![(v[0][0] as f64 * E7, v[0][1] as f64 * E7, 0.0)];
        for k in 1..v.len() {
            let (x0, y0, x1, y1) = (v[k - 1][0] as f64 * E7, v[k - 1][1] as f64 * E7, v[k][0] as f64 * E7, v[k][1] as f64 * E7);
            let d = dist_m(x0, y0, x1, y1);
            acc += d;
            total += d;
            pre[k] = total as f32;
            pending.push((x1, y1, acc));
            if node_of.contains_key(&v[k]) || k + 1 == v.len() {
                let e = edges.len() as u32;
                let pt = |q: [i32; 2]| [q[0] as f64 * E7, q[1] as f64 * E7];
                let j0 = (start + 1..=k).find(|&j| pre[j] - pre[start] >= DIR_M).unwrap_or(k);
                let j1 = (start..k).rev().find(|&j| pre[k] - pre[j] >= DIR_M).unwrap_or(start);
                let (du, dv) = (heading(pt(v[start]), pt(v[j0])), heading(pt(v[k]), pt(v[j1])));
                edges.push(Edge { u: node_of[&v[start]], v: node_of[&v[k]], len: acc as f32, way: w, groups: wr.rail, du, dv });
                edge_way.push((ri as u32, pre[start]));
                for &(x, y, off) in &pending {
                    grid.entry(((x / CELL).floor() as i32, (y / CELL).floor() as i32)).or_default().push((x, y, At { edge: e, off: off as f32 }, wr.rail));
                }
                start = k;
                acc = 0.0;
                pending = vec![(x1, y1, 0.0)];
            }
        }
        way_edges.push((first, edges.len() as u32 - first));
        along.push(pre);
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
                        let du = heading(p, q);
                        edges.push(Edge { u: i as u32, v: j, len: (d * 2.0) as f32, way: u32::MAX, groups: 0xff, du, dv: [-du[0], -du[1]] });
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

    // A stop's point on a track: from its nearest vertex, the foot of the perpendicular on the
    // segments either side (within the vertex's stretch), and the distance to it.
    let foot = |p: [f64; 2], at: At| -> (At, f64) {
        let (ri, start) = edge_way[at.edge as usize];
        let (a, len) = (&along[ri as usize], edges[at.edge as usize].len);
        let wr = &ways[rail[ri as usize] as usize];
        let v = &verts[wr.vstart as usize..(wr.vstart + wr.vcount as u64) as usize];
        let x = start + at.off;
        let k = a.partition_point(|&d| d < x).min(a.len() - 1);
        let k = [k.saturating_sub(1), k, (k + 1).min(a.len() - 1)].into_iter().min_by(|&m, &n| (a[m] - x).abs().total_cmp(&(a[n] - x).abs())).unwrap();
        let (kx, ky) = (111_320.0 * p[1].to_radians().cos(), 110_570.0);
        let pt = |q: [i32; 2]| [q[0] as f64 * E7, q[1] as f64 * E7];
        let mut best = (f64::MAX, x);
        for (s, t) in [(k.wrapping_sub(1), k), (k, k + 1)] {
            if s >= v.len() || t >= v.len() {
                continue;
            }
            let (a0, a1) = (pt(v[s]), pt(v[t]));
            let d = [(a1[0] - a0[0]) * kx, (a1[1] - a0[1]) * ky];
            let q = [(p[0] - a0[0]) * kx, (p[1] - a0[1]) * ky];
            let dd = d[0] * d[0] + d[1] * d[1];
            let u = if dd > 0.0 { ((q[0] * d[0] + q[1] * d[1]) / dd).clamp(0.0, 1.0) } else { 0.0 };
            let dist = ((q[0] - d[0] * u).powi(2) + (q[1] - d[1] * u).powi(2)).sqrt();
            if dist < best.0 {
                best = (dist, a[s] + (a[t] - a[s]) * u as f32);
            }
        }
        (At { edge: at.edge, off: (best.1 - start).clamp(0.0, len) }, best.0)
    };
    // The nearest point of each nearby track (way), right kind of track first, up to a few; with
    // the distance from the stop, which is added to the path cost.
    let snap_within = |p: [f64; 2], bits: u8, max_m: f64, n: usize, w: f64| -> Vec<(At, f32)> {
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
        c.truncate(n);
        c.into_iter().map(|(d, _, at)| {
            let (at, df) = foot(p, at);
            (at, (df.min(d) * w) as f32)
        }).collect()
    };
    // A stop with no track near it (a rural station whose route relation runs along another track
    // of the line) takes the nearest within SNAP_FAR_M.
    let snap = |p: [f64; 2], bits: u8, n: usize, w: f64| -> Vec<(At, f32)> {
        let c = snap_within(p, bits, SNAP_M, n, w);
        if c.is_empty() { snap_within(p, bits, SNAP_FAR_M, n, w) } else { c }
    };

    let debug = std::env::var("RAILFREQ_DEBUG").is_ok();
    // RAILFREQ_WAY=<OSM way id>: every pair whose trains run on that way, and its path.
    let debug_way: Option<i64> = std::env::var("RAILFREQ_WAY").ok().and_then(|s| s.parse().ok());
    // RAILFREQ_STOP=lon,lat: the paths of the pairs from or to a stop within 50 m of there.
    let debug_stop: Option<[f64; 2]> = std::env::var("RAILFREQ_STOP").ok().and_then(|s| {
        let v: Vec<f64> = s.split(',').filter_map(|x| x.parse().ok()).collect();
        (v.len() == 2).then(|| [v[0], v[1]])
    });
    let n_rail = rail.len();
    let (mut runs, matched, partial) = pairs
        .par_chunks(2048)
        .map(|chunk| {
            // Where each pair's trains run: (edge, from, to along it, trains, a lower bound).
            let mut runs: Vec<(u32, f32, f32, f32, bool)> = Vec::new();
            let mut matched = 0usize;
            let mut partial = 0usize;
            // The search's states: a stretch travelled one way (edge × 2, + 1 from v to u), so the
            // turn at each node is known.
            let mut dist: HashMap<u32, f32> = HashMap::new();
            let mut prev: HashMap<u32, u32> = HashMap::new(); // state → the state before it
            let mut src: HashMap<u32, At> = HashMap::new(); // first state → the point of A's stretch it starts from
            for p in chunk {
                let r0 = runs.len();
                let bits = mode_bits(p.mode);
                // First the nearest few tracks at each stop. With no path along them that doesn't turn
                // back (a stop in a big station, nearest tracks its line doesn't reach without
                // reversing), more of them, their distance from the stop counted double so that a path
                // still runs to the station rather than ending on a track short of it.
                for (k, &(n_cands, d0_w)) in ATTEMPTS.iter().enumerate() {
                    let last = k + 1 == ATTEMPTS.len();
                    let (ca, cb) = (snap(p.a, bits, n_cands, d0_w), snap(p.b, bits, n_cands, d0_w));
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
                            break;
                        }
                    };
                    let cost = |e: &Edge, len: f32| if e.groups & bits != 0 { len } else { len * OFF_MODE };
                    let mut run = |e: u32, from: f32, to: f32| runs.push((e, from.min(to), from.max(to), p.n, p.lower));
                    // Both stops on the same stretch of track.
                    if let Some((a, b)) = ca.iter().find_map(|(a, _)| cb.iter().find(|(b, _)| b.edge == a.edge).map(|(b, _)| (*a, *b))) {
                        run(a.edge, a.off, b.off);
                        matched += 1;
                        break;
                    }
                    let straight = dist_m(p.a[0], p.a[1], p.b[0], p.b[1]);
                    let limit = (straight * 3.0 + 3000.0) as f32;
                    dist.clear();
                    prev.clear();
                    src.clear();
                    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new(); // (cost as bits, state)
                    for &(a, d0) in &ca {
                        let ea = &edges[a.edge as usize];
                        for (st, c) in [(a.edge * 2, d0 + cost(ea, ea.len - a.off)), (a.edge * 2 + 1, d0 + cost(ea, a.off))] {
                            if dist.get(&st).is_none_or(|&d| c < d) {
                                dist.insert(st, c);
                                src.insert(st, a);
                                heap.push(Reverse((c.to_bits(), st)));
                            }
                        }
                    }
                    // The node a state ends at, and the train's heading there.
                    let end_of = |st: u32| -> (u32, [f32; 2]) {
                        let e = &edges[(st / 2) as usize];
                        if st % 2 == 0 { (e.v, [-e.dv[0], -e.dv[1]]) } else { (e.u, [-e.du[0], -e.du[1]]) }
                    };
                    // Leaving a node by a stretch heading more than 90° off the train's: turning back (a
                    // switch turns a few degrees). Into or out of a gap, more than 60°: two sideways jumps
                    // across gaps in a row turn a train around as well.
                    let turn = |h: [f32; 2], d: [f32; 2], gap: bool| {
                        if h[0] * d[0] + h[1] * d[1] < if gap { 0.5 } else { 0.0 } { REVERSE_M } else { 0.0 }
                    };
                    let gap = |e: u32| edges[e as usize].way == u32::MAX;
                    // Reaching either end of one of B's stretches, plus the rest of it.
                    let none = At { edge: u32::MAX, off: 0.0 };
                    let finish = |node: u32, h: [f32; 2], via_gap: bool, d: f32| -> (f32, At) {
                        let mut best = (f32::INFINITY, none);
                        for &(b, d0) in &cb {
                            let eb = &edges[b.edge as usize];
                            let f = if node == eb.u {
                                d + turn(h, eb.du, via_gap) + cost(eb, b.off) + d0
                            } else if node == eb.v {
                                d + turn(h, eb.dv, via_gap) + cost(eb, eb.len - b.off) + d0
                            } else {
                                f32::INFINITY
                            };
                            if f < best.0 {
                                best = (f, b);
                            }
                        }
                        best
                    };
                    let mut best = (f32::INFINITY, u32::MAX, none); // cost, state, B's point
                    let mut end = (BEYOND_SHARE * straight, u32::MAX); // beyond: the state reaching the track end nearest the stop
                    while let Some(Reverse((cbits, st))) = heap.pop() {
                        let c = f32::from_bits(cbits);
                        if c > limit || c >= best.0 {
                            break;
                        }
                        if dist.get(&st).is_some_and(|&d| c > d) {
                            continue;
                        }
                        let (u, h) = end_of(st);
                        let (f, eb) = finish(u, h, gap(st / 2), c);
                        if f < best.0 {
                            best = (f, st, eb);
                        }
                        if let Some(t) = beyond {
                            if adj[u as usize].len() == 1 {
                                let d = dist_m(node_pos[u as usize][0], node_pos[u as usize][1], t[0], t[1]);
                                if d < end.0 && c + d as f32 <= limit {
                                    end = (d, st);
                                }
                            }
                        }
                        for &ei in &adj[u as usize] {
                            if ei == st / 2 {
                                continue; // back along the same stretch
                            }
                            let e = &edges[ei as usize];
                            // Leaving u by e: from its u end toward v, or from its v end.
                            for (s2, from, dep) in [(ei * 2, e.u, e.du), (ei * 2 + 1, e.v, e.dv)] {
                                if from != u {
                                    continue;
                                }
                                let nc = c + turn(h, dep, gap(st / 2) || gap(ei)) + cost(e, e.len);
                                if nc <= limit && dist.get(&s2).is_none_or(|&d| nc < d) {
                                    dist.insert(s2, nc);
                                    prev.insert(s2, st);
                                    src.remove(&s2);
                                    heap.push(Reverse((nc.to_bits(), s2)));
                                }
                            }
                        }
                    }
                    if beyond.is_some() {
                        best.1 = end.1;
                    }
                    if best.1 == u32::MAX {
                        if debug && last {
                            let why = if beyond.is_some() { "snap" } else { "path" };
                            eprintln!("MISS {why} {} {:.5},{:.5} {:.5},{:.5} {}", p.mode, p.a[0], p.a[1], p.b[0], p.b[1], p.n);
                        }
                        continue;
                    }
                    matched += 1;
                    // The end of a stretch at a node: its offset there.
                    let at_node = |e: &Edge, node: u32| if node == e.u { 0.0 } else { e.len };
                    let mut st = best.1;
                    if beyond.is_some() {
                        partial += 1;
                        if debug {
                            let q = node_pos[end_of(st).0 as usize];
                            eprintln!("BEYOND {} {:.5},{:.5} {:.5},{:.5} {} → end {:.5},{:.5}", p.mode, p.a[0], p.a[1], p.b[0], p.b[1], p.n, q[0], q[1]);
                        }
                    } else {
                        // B's stretch, from where the path reaches it to the stop.
                        run(best.2.edge, at_node(&edges[best.2.edge as usize], end_of(st).0), best.2.off);
                    }
                    let mut guard = 0;
                    loop {
                        let e = &edges[(st / 2) as usize];
                        if let Some(&a) = src.get(&st) {
                            // A's stretch, from the stop to the end the path leaves it by.
                            run(a.edge, a.off, at_node(e, end_of(st).0));
                            break;
                        }
                        run(st / 2, 0.0, e.len);
                        let Some(&ps) = prev.get(&st) else { break };
                        st = ps;
                        guard += 1;
                        if guard > 100_000 {
                            break;
                        }
                    }
                    break;
                }
                let at_stop = debug_stop.is_some_and(|q| dist_m(q[0], q[1], p.a[0], p.a[1]) < 50.0 || dist_m(q[0], q[1], p.b[0], p.b[1]) < 50.0);
                if debug_way.is_some() || at_stop {
                    let dw = debug_way.unwrap_or(0);
                    let mine = &runs[r0..];
                    if at_stop || mine.iter().any(|r| edges[r.0 as usize].way != u32::MAX && ways[edges[r.0 as usize].way as usize].id == dw) {
                        let path: Vec<String> = mine.iter().rev().map(|r| {
                            let e = &edges[r.0 as usize];
                            let id = if e.way == u32::MAX { 0 } else { ways[e.way as usize].id };
                            format!("{id}[{:.0}-{:.0}/{:.0}]", r.1, r.2, e.len)
                        }).collect();
                        eprintln!("WAY {dw}: {:.5},{:.5} -> {:.5},{:.5} mode {} trains {}: {}", p.a[0], p.a[1], p.b[0], p.b[1], p.mode, p.n, path.join(" "));
                    }
                }
            }
            (runs, matched, partial)
        })
        .reduce(|| (Vec::new(), 0usize, 0usize), |(mut a, ma, pa), (mut b, mb, pb)| {
            a.append(&mut b);
            (a, ma + mb, pa + pb)
        });
    eprintln!("{partial} pairs with one stop beyond the tracks run to the nearest track end toward it");
    // The trains at each point of each stretch: breakpoints (from here on, this many), a sweep over
    // its runs (a run ending where another starts doesn't overlap it; one of no length covers a point).
    runs.par_sort_unstable_by_key(|r| r.0);
    let mut cov_off = vec![0u32; edges.len() + 1];
    let mut cov: Vec<(f32, f32)> = Vec::new();
    let mut edge_lower = vec![false; edges.len()];
    let mut by_edge = runs.chunk_by(|x, y| x.0 == y.0).peekable();
    for e in 0..edges.len() {
        cov_off[e] = cov.len() as u32;
        let Some(rs) = by_edge.next_if(|rs| rs[0].0 as usize == e) else { continue };
        let mut ev: Vec<(f32, f32)> = rs.iter().flat_map(|&(_, from, to, n, _)| [(from, n), (to.max(from + 0.5), -n)]).collect();
        ev.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1))); // ends first where they meet starts
        let mut cur = 0f32;
        for (pos, d) in ev {
            cur += d;
            if cov.len() > cov_off[e] as usize && cov.last().is_some_and(|l| l.0 == pos) {
                cov.last_mut().unwrap().1 = cur;
            } else {
                cov.push((pos, cur));
            }
        }
        edge_lower[e] = rs.iter().any(|r| r.4);
    }
    cov_off[edges.len()] = cov.len() as u32;
    drop(runs);
    let cov_at = |e: usize, x: f32| -> f32 {
        let pts = &cov[cov_off[e] as usize..cov_off[e + 1] as usize];
        let k = pts.partition_point(|p| p.0 <= x);
        if k == 0 { 0.0 } else { pts[k - 1].1.max(0.0) }
    };
    // Trains at a distance along a rail way (and whether it's a lower bound).
    let at = |i: usize, x: f32| -> (f32, bool) {
        let (first, n) = way_edges[i];
        let mut start = 0f32;
        for e in first..first + n {
            let len = edges[e as usize].len;
            if x <= start + len || e + 1 == first + n {
                let c = cov_at(e as usize, (x - start).clamp(0.0, len));
                return (c, c > 0.0 && edge_lower[e as usize]);
            }
            start += len;
        }
        (0.0, false)
    };
    // Each way's most trains anywhere on it (for a way with none at its sampled points).
    let most: Vec<(f32, bool)> = (0..n_rail)
        .map(|i| {
            let (first, n) = way_edges[i];
            (first..first + n).fold((0f32, false), |(m, lo), e| {
                let top = cov[cov_off[e as usize] as usize..cov_off[e as usize + 1] as usize].iter().fold(0f32, |a, p| a.max(p.1));
                (m.max(top), lo || (top > 0.0 && edge_lower[e as usize]))
            })
        })
        .collect();
    let (sums, lower) = corridors(&rail, ways, verts, &along, &at, &most);
    // RAILFREQ_AT=lon,lat,metres: every rail way there, its trains a day each way, and the trains
    // along each of its stretches (both directions: count@metres from the stretch's start).
    if let Some(v) = std::env::var("RAILFREQ_AT").ok().map(|s| s.split(',').filter_map(|x| x.parse::<f64>().ok()).collect::<Vec<_>>()).filter(|v| v.len() == 3) {
        for (i, &w) in rail.iter().enumerate() {
            let wr = &ways[w as usize];
            let vs = &verts[wr.vstart as usize..(wr.vstart + wr.vcount as u64) as usize];
            if !vs.iter().any(|p| dist_m(v[0], v[1], p[0] as f64 * E7, p[1] as f64 * E7) <= v[2]) {
                continue;
            }
            let (first, n) = way_edges[i];
            let stretches: Vec<String> = (first..first + n)
                .map(|e| {
                    let pts = &cov[cov_off[e as usize] as usize..cov_off[e as usize + 1] as usize];
                    format!("[{:.0} m: {}]", edges[e as usize].len, pts.iter().map(|p| format!("{:.0}@{:.0}", p.1, p.0)).collect::<Vec<_>>().join(" "))
                })
                .collect();
            eprintln!("AT way {} ({:.0} m): {:.0} a day each way (own most {:.0}) {}", wr.id, along[i].last().copied().unwrap_or(0.0), sums[i] / 2.0, most[i].0 / 2.0, stretches.join(" "));
        }
    }
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

/// Trains summed across each corridor: per way, at 25/50/75 % of its length, its trains there
/// and those of every track (itself included) running parallel within CORRIDOR_M whose nearest
/// point there is not one of its ends, at that point; the median of those sums (a way with none
/// there: its most anywhere). Lower-bound flags carry over likewise.
fn corridors(
    rail: &[u32],
    ways: &[roadcore::WayRec],
    verts: &[[i32; 2]],
    along: &[Vec<f32>],
    at: &(impl Fn(usize, f32) -> (f32, bool) + Sync),
    most: &[(f32, bool)],
) -> (Vec<f32>, Vec<bool>) {
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
    // The distance along way i at segment k, fraction t of it.
    let dist_along = |i: usize, k: usize, t: f64| -> f32 {
        let a = &along[i];
        a[k] + (a[k + 1] - a[k]) * t as f32
    };
    let groups: Vec<u8> = rail.iter().map(|&w| ways[w as usize].rail).collect();
    let res: Vec<(f32, bool)> = (0..rail.len())
        .into_par_iter()
        .map(|i| {
            let v = pts(i);
            if v.len() < 2 {
                return most[i];
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
                // Nearest point of each other way near p: (distance, parallel, interior, distance along it).
                let mut near: HashMap<u32, (f64, bool, bool, f32)> = HashMap::new();
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
                            let e = near.entry(j).or_insert((f64::MAX, false, false, 0.0));
                            if dist < e.0 {
                                *e = (dist, cos > 0.94, !end, dist_along(j as usize, sk as usize, u));
                            }
                        }
                    }
                }
                let (mut sum, mut lo) = at(i, dist_along(i, k, t));
                for (&j, &(_, parallel, interior, x)) in &near {
                    if parallel && interior {
                        let (c, l) = at(j as usize, x);
                        sum += c;
                        lo |= l;
                    }
                }
                samples.push((sum, lo));
            }
            samples.sort_by(|a, b| a.0.total_cmp(&b.0));
            let m = samples[samples.len() / 2];
            if m.0 > 0.0 { m } else { most[i] }
        })
        .collect();
    let n_gain = res.iter().zip(most).filter(|((c, _), (s, _))| *c > 0.0 && *s <= 0.0).count();
    eprintln!("corridors: {} tracks without trains of their own get their corridor's", n_gain);
    res.into_iter().unzip()
}
