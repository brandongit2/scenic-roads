//! Whole roads, for the road-length filter: ways chained end to end when they share a route
//! ref (any one of a multi-ref), else the same name, else (both unnamed) the same class; at
//! each junction the straightest continuation within 100° is taken, and oneways are entered
//! only in their direction of travel. This is the server's `road_chain` (what hovering a road
//! highlights) applied once to partition the network: each way belongs to exactly one road.

use crate::elev::{heading, turn, Net};
use roadcore::{dist_m, flag, E7};

enum Key {
    Refs(Vec<u32>),
    Name(u32),
    Unnamed(u8),
}

/// Total length (m) of the road each way belongs to.
pub fn lengths(net: &Net, strings: &[String]) -> Vec<f32> {
    let (ways, v) = (net.ways, net.verts);
    let n = ways.len();
    let way_len: Vec<f64> = ways
        .iter()
        .map(|w| {
            let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
            v[r].windows(2).map(|p| dist_m(p[0][0] as f64 * E7, p[0][1] as f64 * E7, p[1][0] as f64 * E7, p[1][1] as f64 * E7)).sum()
        })
        .collect();
    // Individual route refs, interned ("A 1;E 15" → two ids).
    let mut ref_ids: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let refs_of: Vec<Vec<u32>> = {
        let mut cache: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
        ways.iter()
            .map(|w| {
                if w.ref_ == 0 {
                    return Vec::new();
                }
                cache
                    .entry(w.ref_)
                    .or_insert_with(|| {
                        strings[w.ref_ as usize]
                            .split(';')
                            .map(|x| {
                                let k = ref_ids.len() as u32;
                                *ref_ids.entry(x.trim()).or_insert(k)
                            })
                            .collect()
                    })
                    .clone()
            })
            .collect()
    };
    let key_of = |i: usize| {
        let w = &ways[i];
        if !refs_of[i].is_empty() {
            Key::Refs(refs_of[i].clone())
        } else if w.name != 0 {
            Key::Name(w.name)
        } else {
            Key::Unnamed(w.class)
        }
    };
    let same = |k: &Key, j: usize| match k {
        Key::Refs(r) => refs_of[j].iter().any(|x| r.contains(x)),
        Key::Name(nm) => ways[j].name == *nm,
        Key::Unnamed(c) => ways[j].name == 0 && ways[j].ref_ == 0 && ways[j].class == *c,
    };
    let first = |i: usize| ways[i].vstart as usize;
    let last = |i: usize| (ways[i].vstart + ways[i].vcount as u64 - 1) as usize;

    let mut road = vec![u32::MAX; n];
    let mut out = vec![0f32; n];
    let mut members: Vec<usize> = Vec::new();
    for w0 in 0..n {
        if road[w0] != u32::MAX {
            continue;
        }
        let key = key_of(w0);
        road[w0] = w0 as u32;
        members.clear();
        members.push(w0);
        let mut total = way_len[w0];
        // Forward from the end (with the direction of travel), then backward from the start.
        for (backwards, node, at, from) in [(false, net.ends[w0][1], last(w0), last(w0) - 1), (true, net.ends[w0][0], first(w0), first(w0) + 1)] {
            let (mut node, mut at, mut from) = (node, at, from);
            if ways[w0].vcount < 2 {
                break;
            }
            loop {
                let h_in = heading(v, from, at);
                let mut best: Option<(f64, usize, bool)> = None;
                for &(c, starts) in &net.inc[node as usize] {
                    let c = c as usize;
                    if road[c] != u32::MAX || !same(&key, c) || ways[c].vcount < 2 {
                        continue;
                    }
                    let rev = !starts;
                    if ways[c].flags & flag::ONEWAY != 0 && rev != backwards {
                        continue;
                    }
                    let (a, b) = if rev { (last(c), last(c) - 1) } else { (first(c), first(c) + 1) };
                    let t = turn(heading(v, a, b), h_in);
                    if t <= 100f64.to_radians() && best.is_none_or(|(bt, _, _)| t < bt) {
                        best = Some((t, c, rev));
                    }
                }
                let Some((_, c, rev)) = best else { break };
                road[c] = w0 as u32;
                members.push(c);
                total += way_len[c];
                (node, at, from) = if rev { (net.ends[c][0], first(c), first(c) + 1) } else { (net.ends[c][1], last(c), last(c) - 1) };
            }
        }
        for &m in &members {
            out[m] = total as f32;
        }
    }
    out
}
