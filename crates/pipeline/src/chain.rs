//! The one chaining (docs/plan.md §6, docs/formats.md "Road values"): which ways form one road.
//! Whole-road lengths, strokes, drives, the hover highlight, profiles and rail rides all use it.
//!
//! **Pairing.** At each node (a shared way end), way ends pair by mutual best continuation. Two ends
//! can continue each other when their ways are the same kind (roads with roads, rail with rail)
//! and, best first:
//!   3. both have route refs and share one (any token of a multi-ref);
//!   2. both are named, with the same name (one may also have a ref the other lacks);
//!   1. neither has a name or a ref, and they're the same class;
//!   0. one has neither name nor ref, the other has one, same class, nearly straight (≤ 35°):
//!      an unnamed bridge or a gap in the tagging, not a different road.
//! Within a level the straightest wins (levels 1–3 up to 100°), then the lower way id. A oneway
//! is only continued in its direction of travel. The decision at a node depends only on the ways
//! through it, so it can be made per area for the nodes inside the area.
//!
//! **Walking.** Pairs form paths and cycles. A path is walked from the end whose way has the lower
//! id; a cycle from its lowest way id, in that way's direction. Each way gets its road's id (the
//! road's lowest way id), the road's length, and its offset and direction along it.

use rayon::prelude::*;
use roadcore::{dist_m, E7};

/// No partner.
pub const NONE: u32 = u32::MAX;

/// Never chained (ferries; degenerate ways).
pub const KIND_NONE: u8 = 255;
pub const KIND_ROAD: u8 = 0;
pub const KIND_RAIL: u8 = 1;

/// A way as the chaining sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Link {
    /// OSM way id.
    pub id: u64,
    pub kind: u8,
    pub class: u8,
    pub oneway: bool,
    /// Interned name (0 = none). Rail: the line's identity.
    pub name: u32,
    /// Interned route-ref tokens: `refs_len` entries of the shared pool from `refs_at`, sorted.
    pub refs_at: u32,
    pub refs_len: u16,
    /// First and last vertex.
    pub ends: [[i32; 2]; 2],
    /// Heading leaving each end into the way (radians, east = 0, north = π/2).
    pub heading: [f32; 2],
    /// Length, metres.
    pub len: f32,
}

/// One way's place on its road.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadVal {
    /// The road's lowest OSM way id.
    pub road: u64,
    /// The whole road's length, metres.
    pub len: f32,
    /// Metres from the road's start to this way's start in the road's direction.
    pub offset: f32,
    /// 0: the way runs with the road; 1: against it.
    pub dir: u8,
}

/// Heading from vertex a to vertex b (as `elev::heading`).
pub fn heading(a: [i32; 2], b: [i32; 2]) -> f32 {
    let lat = a[1] as f64 * E7;
    let dx = (b[0] - a[0]) as f64 * lat.to_radians().cos();
    let dy = (b[1] - a[1]) as f64;
    dy.atan2(dx) as f32
}

/// The chaining's view of a polyline: its ends, the headings leaving them, and its length.
pub fn shape(v: &[[i32; 2]]) -> ([[i32; 2]; 2], [f32; 2], f32) {
    let n = v.len();
    let len: f64 = v.windows(2).map(|p| dist_m(p[0][0] as f64 * E7, p[0][1] as f64 * E7, p[1][0] as f64 * E7, p[1][1] as f64 * E7)).sum();
    if n < 2 {
        return ([v[0], v[0]], [0.0, 0.0], 0.0);
    }
    // The heading over the first and last few metres: the end vertices of densified geometry can
    // be centimetres apart.
    let lead = |it: &mut dyn Iterator<Item = [i32; 2]>, from: [i32; 2]| {
        let mut far = from;
        for p in it {
            far = p;
            if dist_m(from[0] as f64 * E7, from[1] as f64 * E7, p[0] as f64 * E7, p[1] as f64 * E7) >= 10.0 {
                break;
            }
        }
        heading(from, far)
    };
    let h0 = lead(&mut v[1..].iter().copied(), v[0]);
    let h1 = lead(&mut v[..n - 1].iter().rev().copied(), v[n - 1]);
    ([v[0], v[n - 1]], [h0, h1], len as f32)
}

fn angle_between(a: f32, b: f32) -> f32 {
    let mut t = (a - b).abs() % std::f32::consts::TAU;
    if t > std::f32::consts::PI {
        t = std::f32::consts::TAU - t;
    }
    t
}

/// How well two ways continue each other by identity (see the module doc), or None.
fn level(a: &Link, b: &Link, refs: &[u32]) -> Option<u8> {
    if a.kind != b.kind || a.kind == KIND_NONE {
        return None;
    }
    let ra = &refs[a.refs_at as usize..a.refs_at as usize + a.refs_len as usize];
    let rb = &refs[b.refs_at as usize..b.refs_at as usize + b.refs_len as usize];
    if !ra.is_empty() && !rb.is_empty() {
        // Sorted: a merge finds a shared token.
        let (mut i, mut j) = (0, 0);
        while i < ra.len() && j < rb.len() {
            match ra[i].cmp(&rb[j]) {
                std::cmp::Ordering::Equal => return Some(3),
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
            }
        }
        // Different numbered roads, whatever their names.
        return None;
    }
    if a.name != 0 && a.name == b.name {
        return Some(2);
    }
    let bare = |l: &Link, r: &[u32]| l.name == 0 && r.is_empty();
    match (bare(a, ra), bare(b, rb)) {
        (true, true) if a.class == b.class => Some(1),
        (true, false) | (false, true) if a.class == b.class => Some(0),
        _ => None,
    }
}

/// Can travel arrive at the node along way end `ea` of `a` and leave along end `eb` of `b`?
fn through(a: &Link, ea: usize, b: &Link, eb: usize) -> bool {
    // Arriving at a's end `ea`: a oneway must end there. Leaving by b's end `eb`: a oneway must start there.
    (!a.oneway || ea == 1) && (!b.oneway || eb == 0)
}

/// Pair way ends at every node `at_node` accepts. Returns each way's partner per end, encoded as
/// `way << 1 | end`, or `NONE`. Deterministic: independent of the order of `links`' processing.
pub fn pair(links: &[Link], refs: &[u32], at_node: &(dyn Fn([i32; 2]) -> bool + Sync)) -> Vec<[u32; 2]> {
    let key = |p: [i32; 2]| ((p[0] as u32 as u64) << 32) | p[1] as u32 as u64;
    let mut ends: Vec<(u64, u32)> = Vec::with_capacity(links.len() * 2);
    for (i, l) in links.iter().enumerate() {
        if l.kind == KIND_NONE {
            continue;
        }
        for e in 0..2 {
            ends.push((key(l.ends[e]), (i as u32) << 1 | e as u32));
        }
    }
    ends.par_sort_unstable();
    // Node groups.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut s = 0;
    for i in 1..=ends.len() {
        if i == ends.len() || ends[i].0 != ends[s].0 {
            if i - s >= 2 {
                groups.push((s, i));
            }
            s = i;
        }
    }
    let pairs: Vec<(u32, u32)> = groups
        .par_iter()
        .flat_map_iter(|&(s, e)| {
            let g = &ends[s..e];
            let p = links[(g[0].1 >> 1) as usize].ends[(g[0].1 & 1) as usize];
            if !at_node(p) {
                return Vec::new().into_iter();
            }
            // Best partner of each end at this node.
            let best: Vec<Option<usize>> = (0..g.len())
                .map(|ia| {
                    let (wa, ea) = ((g[ia].1 >> 1) as usize, (g[ia].1 & 1) as usize);
                    let a = &links[wa];
                    let mut best: Option<(u8, f32, u64, u32, usize)> = None;
                    for (ib, &(_, eb_code)) in g.iter().enumerate() {
                        let (wb, eb) = ((eb_code >> 1) as usize, (eb_code & 1) as usize);
                        if wb == wa {
                            continue;
                        }
                        let b = &links[wb];
                        let Some(lv) = level(a, b, refs) else { continue };
                        if !(through(a, ea, b, eb) || through(b, eb, a, ea)) {
                            continue;
                        }
                        // Arriving along a means travelling against a's heading out of this node.
                        let t = angle_between(a.heading[ea] + std::f32::consts::PI, b.heading[eb]);
                        let limit = if lv == 0 { 35f32 } else { 100f32 }.to_radians();
                        if t > limit {
                            continue;
                        }
                        let cand = (lv, t, b.id, eb_code, ib);
                        let better = match &best {
                            None => true,
                            Some(c) => (std::cmp::Reverse(cand.0), cand.1, cand.2, cand.3) < (std::cmp::Reverse(c.0), c.1, c.2, c.3),
                        };
                        if better {
                            best = Some(cand);
                        }
                    }
                    best.map(|c| c.4)
                })
                .collect();
            let mut out = Vec::new();
            for ia in 0..g.len() {
                if let Some(ib) = best[ia] {
                    if ib > ia && best[ib] == Some(ia) {
                        out.push((g[ia].1, g[ib].1));
                    }
                }
            }
            out.into_iter()
        })
        .collect();
    let mut partner = vec![[NONE; 2]; links.len()];
    for (a, b) in pairs {
        partner[(a >> 1) as usize][(a & 1) as usize] = b;
        partner[(b >> 1) as usize][(b & 1) as usize] = a;
    }
    partner
}

/// Walk the paired ways into roads.
pub fn walk(links: &[Link], partner: &[[u32; 2]]) -> Vec<RoadVal> {
    walk_by(links.len(), |i| links[i].id, |i| links[i].len, |i| links[i].kind != KIND_NONE, partner)
}

/// `walk` over any way storage: each way's OSM id, length and whether it chains at all (the
/// worldwide walk keeps only ids and lengths, a planet's worth in memory).
pub fn walk_by(n: usize, id: impl Fn(usize) -> u64 + Sync, len: impl Fn(usize) -> f32, chained: impl Fn(usize) -> bool, partner: &[[u32; 2]]) -> Vec<RoadVal> {
    let mut out = vec![RoadVal { road: 0, len: 0.0, offset: 0.0, dir: 0 }; n];
    let mut done = vec![false; n];
    // Lowest id first, so a cycle is met at its lowest way (no order needed when sorted already).
    let sorted = (1..n).all(|i| id(i - 1) <= id(i));
    let order: Vec<u32> = if sorted {
        Vec::new()
    } else {
        let mut o: Vec<u32> = (0..n as u32).collect();
        o.par_sort_unstable_by_key(|&i| (id(i as usize), i));
        o
    };
    let next = |w: usize, end: usize| -> Option<(usize, usize)> {
        let p = partner[w][end];
        (p != NONE).then(|| ((p >> 1) as usize, (p & 1) as usize))
    };
    let mut chain: Vec<(usize, u8)> = Vec::new();
    for k in 0..n {
        let i = if sorted { k } else { order[k] as usize };
        if done[i] {
            continue;
        }
        if !chained(i) {
            done[i] = true;
            out[i] = RoadVal { road: id(i), len: len(i), offset: 0.0, dir: 0 };
            continue;
        }
        // The terminal reached leaving `i` by `end`: (way, its free end), or None for a cycle.
        let terminal = |end: usize| -> Option<(usize, usize)> {
            let (mut w, mut x) = (i, end);
            loop {
                match next(w, x) {
                    None => return Some((w, x)),
                    Some((w2, x2)) => {
                        if w2 == i {
                            return None;
                        }
                        (w, x) = (w2, 1 - x2);
                    }
                }
            }
        };
        // Where the walk starts: a way and the end it enters by.
        let start = match (terminal(0), terminal(1)) {
            (Some(a), Some(b)) => {
                if a.0 == b.0 {
                    (a.0, 0) // a single way: in its own direction
                } else if id(a.0) < id(b.0) || (id(a.0) == id(b.0) && a.0 < b.0) {
                    a
                } else {
                    b
                }
            }
            // A cycle: `i` is its lowest way (ways are visited lowest id first).
            _ => (i, 0),
        };
        chain.clear();
        let (mut w, mut x) = start;
        loop {
            chain.push((w, x as u8));
            done[w] = true;
            match next(w, 1 - x) {
                Some((w2, x2)) if !done[w2] => (w, x) = (w2, x2),
                _ => break,
            }
        }
        let road = chain.iter().map(|&(w, _)| id(w)).min().unwrap_or(0);
        let total: f64 = chain.iter().map(|&(w, _)| len(w) as f64).sum();
        let mut off = 0f64;
        for &(w, dir) in &chain {
            out[w] = RoadVal { road, len: total as f32, offset: off as f32, dir };
            off += len(w) as f64;
        }
    }
    out
}

/// Interns strings into small ids (0 is the empty string).
#[derive(Default)]
pub struct Interner {
    map: std::collections::HashMap<String, u32>,
}

impl Interner {
    pub fn id(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        let n = self.map.len() as u32 + 1;
        *self.map.entry(s.to_string()).or_insert(n)
    }
}

/// A route ref's tokens ("A 1;E 15" → ["A 1", "E 15"]), interned, sorted and deduplicated, appended
/// to `pool`; returns (offset, count).
pub fn intern_refs(r: &str, interner: &mut Interner, pool: &mut Vec<u32>) -> (u32, u16) {
    let at = pool.len() as u32;
    let mut toks: Vec<u32> = r.split(';').map(str::trim).filter(|t| !t.is_empty()).map(|t| interner.id(t)).collect();
    toks.sort_unstable();
    toks.dedup();
    let n = toks.len().min(u16::MAX as usize) as u16;
    pool.extend_from_slice(&toks[..n as usize]);
    (at, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A way along points given in metres east/north of 45° N 0° E.
    fn way(id: u64, pts: &[(f64, f64)], name: u32, refs: (u32, u16), class: u8, oneway: bool) -> Link {
        let m = 1e7 / 111_320.0;
        let v: Vec<[i32; 2]> = pts.iter().map(|&(x, y)| [(x * m / (45f64).to_radians().cos()).round() as i32, (450_000_000.0 + y * m).round() as i32]).collect();
        let (ends, heading, len) = shape(&v);
        Link { id, kind: KIND_ROAD, class, oneway, name, refs_at: refs.0, refs_len: refs.1, ends, heading, len }
    }

    fn run(links: &[Link], refs: &[u32]) -> Vec<RoadVal> {
        let p = pair(links, refs, &|_| true);
        walk(links, &p)
    }

    #[test]
    fn straight_road_in_three_ways() {
        let l = [
            way(30, &[(0.0, 0.0), (100.0, 0.0)], 1, (0, 0), 5, false),
            way(10, &[(100.0, 0.0), (200.0, 0.0)], 1, (0, 0), 5, false),
            way(20, &[(300.0, 0.0), (200.0, 0.0)], 1, (0, 0), 5, false),
        ];
        let r = run(&l, &[]);
        assert!(r.iter().all(|v| v.road == 10));
        assert!((r[0].len - 300.0).abs() < 1.0);
        // The path starts at the end whose way has the lower id: way 30 (west end) vs way 20 (east end).
        assert_eq!(r[2].dir, 0); // way 20 is drawn east→west; the walk starts at its free east end
        assert!(r[2].offset.abs() < 0.5);
        assert!((r[1].offset - 100.0).abs() < 1.0 && r[1].dir == 1);
        assert!((r[0].offset - 200.0).abs() < 1.0 && r[0].dir == 1);
    }

    #[test]
    fn t_junction_keeps_the_straight_road() {
        let l = [
            way(1, &[(0.0, 0.0), (100.0, 0.0)], 1, (0, 0), 5, false),
            way(2, &[(100.0, 0.0), (200.0, 0.0)], 1, (0, 0), 5, false),
            way(3, &[(100.0, 0.0), (100.0, 100.0)], 1, (0, 0), 5, false),
        ];
        let r = run(&l, &[]);
        assert_eq!(r[0].road, r[1].road);
        assert_ne!(r[2].road, r[0].road);
    }

    #[test]
    fn different_names_dont_join_but_an_unnamed_bridge_does() {
        let l = [
            way(1, &[(0.0, 0.0), (100.0, 0.0)], 1, (0, 0), 5, false),
            way(2, &[(100.0, 0.0), (200.0, 0.0)], 0, (0, 0), 5, false), // unnamed bridge
            way(3, &[(200.0, 0.0), (300.0, 0.0)], 1, (0, 0), 5, false),
            way(4, &[(300.0, 0.0), (400.0, 0.0)], 2, (0, 0), 5, false), // another street
        ];
        let r = run(&l, &[]);
        assert_eq!(r[0].road, r[1].road);
        assert_eq!(r[1].road, r[2].road);
        assert_ne!(r[3].road, r[0].road);
    }

    #[test]
    fn refs_win_over_names_and_differ_is_a_break() {
        // Pool: token 7 = "A 1", token 8 = "B 2".
        let refs = [7u32, 8u32];
        let l = [
            way(1, &[(0.0, 0.0), (100.0, 0.0)], 1, (0, 1), 5, false),
            way(2, &[(100.0, 0.0), (200.0, 0.0)], 1, (1, 1), 5, false), // same name, other ref: a break
            way(3, &[(100.0, 0.0), (180.0, 60.0)], 0, (0, 1), 5, false), // same ref, turning
        ];
        let r = run(&l, &refs);
        assert_eq!(r[0].road, r[2].road);
        assert_ne!(r[0].road, r[1].road);
    }

    #[test]
    fn oneway_only_in_its_direction() {
        // Two oneways meeting head to head can't be one road.
        let l = [
            way(1, &[(0.0, 0.0), (100.0, 0.0)], 1, (0, 0), 7, true),
            way(2, &[(200.0, 0.0), (100.0, 0.0)], 1, (0, 0), 7, true),
        ];
        let r = run(&l, &[]);
        assert_ne!(r[0].road, r[1].road);
        let l2 = [
            way(1, &[(0.0, 0.0), (100.0, 0.0)], 1, (0, 0), 7, true),
            way(2, &[(100.0, 0.0), (200.0, 0.0)], 1, (0, 0), 7, true),
        ];
        let r2 = run(&l2, &[]);
        assert_eq!(r2[0].road, r2[1].road);
    }

    #[test]
    fn a_cycle_starts_at_its_lowest_way() {
        let l = [
            way(9, &[(0.0, 0.0), (100.0, 0.0)], 1, (0, 0), 3, false),
            way(5, &[(100.0, 0.0), (100.0, 100.0)], 1, (0, 0), 3, false),
            way(7, &[(100.0, 100.0), (0.0, 100.0)], 1, (0, 0), 3, false),
            way(8, &[(0.0, 100.0), (0.0, 0.0)], 1, (0, 0), 3, false),
        ];
        let r = run(&l, &[]);
        assert!(r.iter().all(|v| v.road == 5));
        assert_eq!(r[1].offset, 0.0);
        assert_eq!(r[1].dir, 0);
        assert!((r[0].len - 400.0).abs() < 1.0);
    }

    #[test]
    fn deterministic_under_permutation() {
        let mut l = vec![
            way(4, &[(0.0, 0.0), (100.0, 0.0)], 1, (0, 0), 5, false),
            way(3, &[(100.0, 0.0), (200.0, 0.0)], 1, (0, 0), 5, false),
            way(2, &[(100.0, 0.0), (200.0, 2.0)], 1, (0, 0), 5, false),
            way(1, &[(200.0, 0.0), (300.0, 0.0)], 1, (0, 0), 5, false),
        ];
        let a = run(&l, &[]);
        let mut by_id: Vec<(u64, RoadVal)> = l.iter().zip(&a).map(|(w, v)| (w.id, *v)).collect();
        by_id.sort_by_key(|x| x.0);
        l.reverse();
        let b = run(&l, &[]);
        let mut by_id2: Vec<(u64, RoadVal)> = l.iter().zip(&b).map(|(w, v)| (w.id, *v)).collect();
        by_id2.sort_by_key(|x| x.0);
        assert_eq!(by_id, by_id2);
    }
}
