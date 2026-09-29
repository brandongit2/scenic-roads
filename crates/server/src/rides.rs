//! Passenger rail in view: "Scenic rides" (the best stretch of each line, like scenic drives) and
//! "Rail lines" (each line in view with its length, mean ride score and trains a day).
//!
//! Lines are chains of connected rail ways with the same identity (the way's name, else the
//! services using the track), built once from the ways' end points. Each 100 m scenic sample gets
//! the ride score of the client (`web/src/rail.ts`), components 0..1:
//!   0 views    VIEW/255             6 viaduct   bridge (the deck height isn't in the samples: 0.5)
//!   1 water    WATER/255            7 tunnel    in a tunnel (a negative weight by default)
//!   2 vista    VISTA/255            8 gradient  min(1, grade % / 4), grade from the samples' eyes
//!   3 relief   min(1, RELIEF·3/600) 9 curves    min(1, CURVY·4/400)
//!   4 ledge    min(1, |TPI−128|·2/60)  10 trains  log10(trains a day)/2 (100 a day = 1); unknown:
//!   5 altitude (eye − 3 m)/1500                 left out of the score
//! score = Σ wᵢcᵢ / Σ max(wᵢ, 0) (without the trains weight where unknown), clamped to 0..1.

use crate::S;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rayon::prelude::*;
use roadcore::scenic::{ch, sflag, Sample};
use roadcore::{class, Array, Ways, E7};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const RNCOMP: usize = 11;
const FREQ: usize = 10;

pub struct RailIndex {
    samples: Array<Sample>,
    ch: Array<[u8; ch::N]>,
    /// Trains a day each way per way (0: unknown).
    freq: Vec<f32>,
    /// Per line: range into `seq`/`dist`, its identity (string index), bbox (E7).
    off: Vec<u32>,
    seq: Vec<u32>,
    dist: Vec<f32>,
    key: Vec<u32>,
    bbox: Vec<[i32; 4]>,
    /// Line identities (index = key).
    names: Vec<String>,
}

impl RailIndex {
    pub fn open(dir: &Path, ways: &Ways, strings: &[String]) -> anyhow::Result<Self> {
        let samples = Array::<Sample>::open(&dir.join("samples.bin"))?;
        let chs = Array::<[u8; ch::N]>::open(&dir.join("samples.ch.u8"))?;
        let s = samples.get();
        let wr = ways.ways();
        let verts = ways.verts();
        // rail-freq.bin: sorted (u32 way, f32 trains a day; negative = a lower bound) pairs.
        let mut freq = vec![0f32; wr.len()];
        if let Ok(b) = std::fs::read(dir.join("rail-freq.bin")) {
            for rec in b.chunks_exact(8) {
                let w = u32::from_le_bytes(rec[0..4].try_into().unwrap()) as usize;
                if w < freq.len() {
                    freq[w] = f32::from_le_bytes(rec[4..8].try_into().unwrap()).abs();
                }
            }
        }
        // Sample range of each way.
        let mut range = vec![(0u32, 0u32); wr.len()];
        let mut k = 0;
        while k < s.len() {
            let w = s[k].way as usize;
            let a = k;
            while k < s.len() && s[k].way as usize == w {
                k += 1;
            }
            range[w] = (a as u32, k as u32);
        }
        let is_rail = |w: &roadcore::WayRec| w.class >= class::TRAM && w.class <= class::HERITAGE;
        // A line's identity: its name without a route's direction ("Highland Sleeper: London
        // Euston => Fort William" → "Highland Sleeper"), else the first service using the track.
        let mut names: Vec<String> = vec![String::new()];
        let mut name_id: HashMap<String, u32> = HashMap::new();
        let mut ident_of = vec![0u32; wr.len()];
        for (i, w) in wr.iter().enumerate() {
            if !is_rail(w) {
                continue;
            }
            let raw = if w.name != 0 { &strings[w.name as usize] } else { strings[w.route as usize].split(" · ").next().unwrap_or("") };
            let n = raw.split(':').next().unwrap_or("").trim();
            if n.is_empty() {
                continue;
            }
            // A bare route ("Inverness => Kyle of Lochalsh") and its return: one line, "A – B".
            let merged;
            let n = if n.contains("=>") {
                let mut ends: Vec<&str> = n.split("=>").map(str::trim).filter(|x| !x.is_empty()).collect();
                ends.sort_unstable();
                ends.dedup();
                merged = ends.join(" – ");
                merged.as_str()
            } else {
                n
            };
            let id = *name_id.entry(n.to_string()).or_insert_with(|| {
                names.push(n.to_string());
                (names.len() - 1) as u32
            });
            ident_of[i] = id;
        }
        let ident = |i: usize| ident_of[i];
        let ends = |i: usize| {
            let w = &wr[i];
            (verts[w.vstart as usize], verts[(w.vstart + w.vcount as u64 - 1) as usize])
        };
        let mut at: HashMap<[i32; 2], Vec<u32>> = HashMap::new();
        let rail: Vec<usize> = (0..wr.len()).filter(|&i| is_rail(&wr[i]) && range[i].1 > range[i].0).collect();
        for &i in &rail {
            let (a, b) = ends(i);
            at.entry(a).or_default().push(i as u32);
            at.entry(b).or_default().push(i as u32);
        }
        // Chain ways of the same identity through shared end points.
        let mut used = vec![false; wr.len()];
        let (mut off, mut seq, mut dist, mut key, mut bbox) = (vec![0u32], Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for &start in &rail {
            if used[start] {
                continue;
            }
            used[start] = true;
            let id = ident(start);
            let mut chain: std::collections::VecDeque<(usize, bool)> = [(start, false)].into();
            // Forward from the end, then backward from the start.
            for forward in [true, false] {
                loop {
                    let (w, rev) = if forward { *chain.back().unwrap() } else { *chain.front().unwrap() };
                    let (a, b) = ends(w);
                    let tip = if forward != rev { b } else { a };
                    let next = at.get(&tip).and_then(|c| c.iter().map(|&x| x as usize).find(|&x| !used[x] && ident(x) == id));
                    let Some(n) = next else { break };
                    used[n] = true;
                    let (na, _) = ends(n);
                    // Oriented so the chain runs through: forward, the next way starts at the tip.
                    let nrev = if forward { na != tip } else { na == tip };
                    if forward { chain.push_back((n, nrev)) } else { chain.push_front((n, nrev)) }
                }
            }
            let mut acc = 0f32;
            let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for (w, rev) in chain {
                let (a, b) = range[w];
                let len = s[a as usize].dist + s[b as usize - 1].dist;
                let idx: Box<dyn Iterator<Item = u32>> = if rev { Box::new((a..b).rev()) } else { Box::new(a..b) };
                for i in idx {
                    let d = s[i as usize].dist;
                    seq.push(i);
                    dist.push(acc + if rev { len - d } else { d });
                    let p = &s[i as usize];
                    bb = [bb[0].min(p.lon), bb[1].min(p.lat), bb[2].max(p.lon), bb[3].max(p.lat)];
                }
                acc += len;
            }
            off.push(seq.len() as u32);
            key.push(id);
            bbox.push(bb);
        }
        eprintln!("rail lines: {} chains from {} rail ways", key.len(), rail.len());
        Ok(Self { samples, ch: chs, freq, off, seq, dist, key, bbox, names })
    }

    /// Components of sample t of the sequence (neighbours give the gradient).
    fn components(&self, t: usize, lo: usize, hi: usize) -> ([f32; RNCOMP], bool) {
        let s = self.samples.get();
        let p = &s[self.seq[t] as usize];
        let c = &self.ch.get()[self.seq[t] as usize];
        let (i0, i1) = (t.saturating_sub(1).max(lo), (t + 1).min(hi - 1));
        let (a, b) = (&s[self.seq[i0] as usize], &s[self.seq[i1] as usize]);
        let dd = (self.dist[i1] - self.dist[i0]).abs().max(1.0);
        let grade = ((b.eye - a.eye).abs() / dd * 100.0).min(30.0);
        let f = self.freq[p.way as usize];
        (
            [
                c[ch::VIEW] as f32 / 255.0,
                c[ch::WATER] as f32 / 255.0,
                c[ch::VISTA] as f32 / 255.0,
                (c[ch::RELIEF] as f32 * 3.0 / 600.0).min(1.0),
                ((c[ch::TPI] as f32 - 128.0).abs() * 2.0 / 60.0).min(1.0),
                ((p.eye - 3.0) / 1500.0).clamp(0.0, 1.0),
                if p.flags & sflag::BRIDGE != 0 { 0.5 } else { 0.0 },
                (p.flags & sflag::TUNNEL != 0) as u8 as f32,
                (grade / 4.0).min(1.0),
                (c[ch::CURVY] as f32 * 4.0 / 400.0).min(1.0),
                if f > 0.0 { (f.max(1.0).log10() / 2.0).clamp(0.0, 1.0) } else { 0.0 },
            ],
            f > 0.0,
        )
    }

    fn score(&self, t: usize, lo: usize, hi: usize, w: &[f32; RNCOMP]) -> f32 {
        let (c, known) = self.components(t, lo, hi);
        let pos: f32 = w.iter().enumerate().map(|(i, v)| if i == FREQ && !known { 0.0 } else { v.max(0.0) }).sum::<f32>().max(1e-6);
        (c.iter().zip(w).map(|(a, b)| a * b).sum::<f32>() / pos).clamp(0.0, 1.0)
    }
}

#[derive(Deserialize)]
pub struct Q {
    bbox: String,
    poly: Option<String>,
    /// Comma-separated ride-factor weights, RNCOMP values.
    w: String,
    /// Stretch length, km (rides).
    len: Option<f32>,
    limit: Option<usize>,
    /// Bitmask of the rail service groups shown (bit k = class TRAM + k).
    groups: Option<u8>,
    /// Rail lines: "score", "trains" or "length".
    sort: Option<String>,
}

fn weights(q: &Q) -> Option<[f32; RNCOMP]> {
    let v: Vec<f32> = q.w.split(',').filter_map(|x| x.parse().ok()).collect();
    (v.len() == RNCOMP).then(|| {
        let mut w = [0f32; RNCOMP];
        w.copy_from_slice(&v);
        w
    })
}

#[derive(Serialize)]
pub struct Ride {
    score: f32,
    length_m: f32,
    way: u32,
    name: String,
    services: String,
    colour: u32,
    trains: f32,
    parts: [f32; RNCOMP],
    geom: Vec<[f64; 2]>,
}

#[derive(Serialize)]
pub struct RidesOut {
    total: usize,
    rides: Vec<Ride>,
}

pub async fn rides(State(s): State<S>, Query(q): Query<Q>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || compute_rides(&s2, q)).await {
        Ok(Some(o)) => Json(o).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn shown(w: &roadcore::WayRec, groups: u8) -> bool {
    (w.rail & groups) != 0 || (w.rail == 0 && (groups >> (w.class - class::TRAM)) & 1 == 1)
}

fn compute_rides(st: &crate::AppState, q: Q) -> Option<RidesOut> {
    let ix = st.rails_ix.as_ref()?;
    let region = crate::Region::parse(&q.bbox, q.poly.as_deref())?;
    let bb = region.bb;
    let w = weights(&q)?;
    let len = q.len.unwrap_or(5.0).clamp(0.5, 100.0) * 1000.0;
    let groups = q.groups.unwrap_or(0xff);
    let samples = ix.samples.get();
    let ways = st.ways.ways();
    let hits: Vec<(f32, usize, usize)> = (0..ix.bbox.len())
        .into_par_iter()
        .filter_map(|k| {
            let sb = ix.bbox[k];
            if sb[2] < bb[0] || sb[0] > bb[2] || sb[3] < bb[1] || sb[1] > bb[3] {
                return None;
            }
            let (a, e) = (ix.off[k] as usize, ix.off[k + 1] as usize);
            if e <= a || ix.dist[e - 1] - ix.dist[a] < len || !shown(&ways[samples[ix.seq[a] as usize].way as usize], groups) {
                return None;
            }
            let sc: Vec<f32> = (a..e).map(|t| ix.score(t, a, e, &w)).collect();
            let mut pre = vec![0f64; sc.len() + 1];
            for (i, v) in sc.iter().enumerate() {
                pre[i + 1] = pre[i] + *v as f64;
            }
            let mut best: Option<(f32, usize, usize)> = None;
            let mut j = 0usize;
            for i in 0..sc.len() {
                j = j.max(i);
                while j + 1 < sc.len() && ix.dist[a + j] - ix.dist[a + i] < len {
                    j += 1;
                }
                if ix.dist[a + j] - ix.dist[a + i] < len {
                    break;
                }
                let m = ((pre[j + 1] - pre[i]) / (j + 1 - i) as f64) as f32;
                let mid = &samples[ix.seq[a + (i + j) / 2] as usize];
                if region.contains(mid.lon, mid.lat) && best.is_none_or(|b| m > b.0) {
                    best = Some((m, a + i, a + j));
                }
            }
            best
        })
        .collect();
    // The best stretch of each line (a line can be several chains, split at junctions).
    let mut hits = hits;
    hits.sort_by(|x, y| y.0.total_cmp(&x.0));
    let chain = |i: usize| ix.off.partition_point(|&o| o as usize <= i) - 1;
    let mut seen = std::collections::HashSet::new();
    hits.retain(|h| seen.insert(ix.key[chain(h.1)]));
    let total = hits.len();
    hits.truncate(q.limit.unwrap_or(30).min(100));
    let rides = hits
        .into_iter()
        .map(|(m, i, j)| {
            let k = chain(i);
            let (lo, hi) = (ix.off[k] as usize, ix.off[k + 1] as usize);
            let mut parts = [0f32; RNCOMP];
            let mut trains = 0f32;
            for t in i..=j {
                let (c, _) = ix.components(t, lo, hi);
                parts.iter_mut().zip(c).for_each(|(p, v)| *p += v);
                trains = trains.max(ix.freq[samples[ix.seq[t] as usize].way as usize]);
            }
            let n = (j + 1 - i) as f32;
            parts.iter_mut().for_each(|p| *p /= n);
            let first = &samples[ix.seq[i] as usize];
            let lw = &ways[samples[ix.seq[(i + j) / 2] as usize].way as usize];
            Ride {
                score: m * 100.0,
                length_m: ix.dist[j] - ix.dist[i],
                way: first.way,
                name: ix.names[ix.key[k] as usize].clone(),
                services: st.strings[lw.route as usize].clone(),
                colour: lw.colour,
                trains,
                parts,
                geom: (i..=j).map(|t| {
                    let p = &samples[ix.seq[t] as usize];
                    [p.lon as f64 * E7, p.lat as f64 * E7]
                }).collect(),
            }
        })
        .collect();
    Some(RidesOut { total, rides })
}

#[derive(Serialize)]
pub struct Line {
    name: String,
    services: String,
    colour: u32,
    /// Length in view, m.
    length_m: f32,
    score: f32,
    trains: f32,
    way: u32,
    /// The line's pieces in view (for highlighting).
    geom: Vec<Vec<[f64; 2]>>,
}

#[derive(Serialize)]
pub struct LinesOut {
    total: usize,
    lines: Vec<Line>,
}

pub async fn lines(State(s): State<S>, Query(q): Query<Q>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || compute_lines(&s2, q)).await {
        Ok(Some(o)) => Json(o).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn compute_lines(st: &crate::AppState, q: Q) -> Option<LinesOut> {
    let ix = st.rails_ix.as_ref()?;
    let region = crate::Region::parse(&q.bbox, q.poly.as_deref())?;
    let bb = region.bb;
    let w = weights(&q)?;
    let groups = q.groups.unwrap_or(0xff);
    let samples = ix.samples.get();
    let ways = st.ways.ways();
    // Per identity: length, score × length, trains, a way, the pieces in view.
    struct Acc {
        len: f32,
        sc: f32,
        trains: f32,
        way: u32,
        geom: Vec<Vec<[f64; 2]>>,
    }
    let parts: Vec<(u32, Acc)> = (0..ix.bbox.len())
        .into_par_iter()
        .filter_map(|k| {
            let sb = ix.bbox[k];
            if ix.key[k] == 0 || sb[2] < bb[0] || sb[0] > bb[2] || sb[3] < bb[1] || sb[1] > bb[3] {
                return None;
            }
            let (a, e) = (ix.off[k] as usize, ix.off[k + 1] as usize);
            if e <= a || !shown(&ways[samples[ix.seq[a] as usize].way as usize], groups) {
                return None;
            }
            let mut acc = Acc { len: 0.0, sc: 0.0, trains: 0.0, way: 0, geom: Vec::new() };
            let mut cur: Vec<[f64; 2]> = Vec::new();
            for t in a..e {
                let p = &samples[ix.seq[t] as usize];
                if !region.contains(p.lon, p.lat) {
                    if cur.len() > 1 {
                        acc.geom.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    continue;
                }
                let step = if t + 1 < e { ix.dist[t + 1] - ix.dist[t] } else { 0.0 }.max(0.0);
                acc.len += step;
                acc.sc += ix.score(t, a, e, &w) * step;
                acc.trains = acc.trains.max(ix.freq[p.way as usize]);
                acc.way = p.way;
                cur.push([p.lon as f64 * E7, p.lat as f64 * E7]);
            }
            if cur.len() > 1 {
                acc.geom.push(cur);
            }
            (acc.len > 0.0).then_some((ix.key[k], acc))
        })
        .collect();
    let mut by: HashMap<u32, Acc> = HashMap::new();
    for (key, a) in parts {
        let e = by.entry(key).or_insert(Acc { len: 0.0, sc: 0.0, trains: 0.0, way: a.way, geom: Vec::new() });
        e.len += a.len;
        e.sc += a.sc;
        e.trains = e.trains.max(a.trains);
        e.geom.extend(a.geom);
    }
    let total = by.len();
    let mut lines: Vec<Line> = by
        .into_iter()
        .filter(|(_, a)| a.len >= 2000.0)
        .map(|(key, a)| {
            let wr = &ways[a.way as usize];
            Line {
                name: ix.names[key as usize].clone(),
                services: st.strings[wr.route as usize].clone(),
                colour: wr.colour,
                length_m: a.len,
                score: a.sc / a.len.max(1.0) * 100.0,
                trains: a.trains,
                way: a.way,
                geom: a.geom,
            }
        })
        .collect();
    match q.sort.as_deref() {
        Some("trains") => lines.sort_by(|x, y| y.trains.total_cmp(&x.trains)),
        Some("length") => lines.sort_by(|x, y| y.length_m.total_cmp(&x.length_m)),
        _ => lines.sort_by(|x, y| y.score.total_cmp(&x.score)),
    }
    lines.truncate(q.limit.unwrap_or(40).min(100));
    Some(LinesOut { total, lines })
}
