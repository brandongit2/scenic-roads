//! Climb detection on the road network.
//!
//! Ways are chained into "strokes" (mutual best continuations: same name/ref, else the
//! straightest), each stroke is resampled every 25 m, and climbing stretches are found in
//! every allowed direction of travel:
//!   1. 200 m-window grade ≥ 1 % marks a sample as climbing
//!   2. runs are merged across short interruptions (< 400 m that lose < 15 m)
//!   3. each run is refined to the lowest point just before and the highest just after
//!   4. kept if gain ≥ 40 m, length ≥ 200 m and average grade 3–25 %

use crate::elev::{is_structure, Net};
use rayon::prelude::*;
use roadcore::climb::ClimbRec;
use roadcore::{class, dist_m, flag, E7};

const STEP_M: f64 = 25.0;
const MIN_GAIN_M: f64 = 40.0;
const MIN_AVG_GRADE: f64 = 0.03;
/// Sustained averages above this over ≥ 200 m are data errors (OSM geometry traced across a
/// slope, DEMs older than a mine pit), not drivable roads.
const MAX_AVG_GRADE: f64 = 0.25;
const MIN_LEN_M: f64 = 200.0;

/// Oriented way within a stroke: (way index, traversed in reverse).
pub type OWay = (u32, bool);

pub fn strokes(net: &Net) -> Vec<Vec<OWay>> {
    let n = net.ways.len();
    let mut used = vec![false; n];
    let usable = |w: usize| net.ways[w].class != class::FERRY;
    let mutual = |from: usize, c: usize, c_starts: bool| {
        let side = if c_starts { 0 } else { 1 };
        net.cont[c][side].is_some_and(|(b, _)| b as usize == from)
    };
    let mut out = Vec::new();
    for w in 0..n {
        if used[w] || !usable(w) {
            continue;
        }
        used[w] = true;
        let mut fwd: Vec<OWay> = vec![(w as u32, false)];
        // Forward from w's end.
        let (mut cur, mut rev) = (w, false);
        loop {
            let side = if rev { 0 } else { 1 };
            let Some((c, starts)) = net.cont[cur][side] else { break };
            let c = c as usize;
            if used[c] || !usable(c) || !mutual(cur, c, starts) {
                break;
            }
            used[c] = true;
            rev = !starts;
            fwd.push((c as u32, rev));
            cur = c;
        }
        // Backward from w's start.
        let mut back: Vec<OWay> = Vec::new();
        let (mut cur, mut rev) = (w, false);
        loop {
            let side = if rev { 1 } else { 0 };
            let Some((c, starts)) = net.cont[cur][side] else { break };
            let c = c as usize;
            if used[c] || !usable(c) || !mutual(cur, c, starts) {
                break;
            }
            used[c] = true;
            // c must end at the junction in walking order.
            rev = starts;
            back.push((c as u32, rev));
            cur = c;
        }
        back.reverse();
        back.extend(fwd);
        out.push(back);
    }
    out
}

struct Series {
    /// Vertex indices along the stroke, and which stroke element each belongs to.
    vi: Vec<usize>,
    owner: Vec<u32>,
    dist: Vec<f64>,
}

fn series(net: &Net, stroke: &[OWay]) -> Series {
    let v = net.verts;
    let mut vi: Vec<usize> = Vec::new();
    let mut owner: Vec<u32> = Vec::new();
    for (k, &(w, rev)) in stroke.iter().enumerate() {
        let wr = &net.ways[w as usize];
        let s = wr.vstart as usize;
        let n = wr.vcount as usize;
        let it: Box<dyn Iterator<Item = usize>> = if rev { Box::new((s..s + n).rev()) } else { Box::new(s..s + n) };
        for (j, x) in it.enumerate() {
            if j == 0 && vi.last().is_some_and(|&l| v[l] == v[x]) {
                continue;
            }
            vi.push(x);
            owner.push(k as u32);
        }
    }
    let mut dist = Vec::with_capacity(vi.len());
    let mut acc = 0.0;
    dist.push(0.0);
    for k in 1..vi.len() {
        let (a, b) = (vi[k - 1], vi[k]);
        acc += dist_m(v[a][0] as f64 * E7, v[a][1] as f64 * E7, v[b][0] as f64 * E7, v[b][1] as f64 * E7);
        dist.push(acc);
    }
    Series { vi, owner, dist }
}

/// Climbing stretches as (start distance, end distance) along `d`/`e` (increasing distance).
fn detect(d: &[f64], e: &[f32]) -> Vec<(f64, f64)> {
    let total = *d.last().unwrap();
    if total < 300.0 {
        return Vec::new();
    }
    // Resample.
    let m = (total / STEP_M).floor() as usize + 1;
    let mut rs = Vec::with_capacity(m);
    let mut j = 0;
    for k in 0..m {
        let x = k as f64 * STEP_M;
        while j + 2 < d.len() && d[j + 1] < x {
            j += 1;
        }
        let t = ((x - d[j]) / (d[j + 1] - d[j]).max(1e-9)).clamp(0.0, 1.0);
        rs.push(e[j] as f64 + (e[j + 1] as f64 - e[j] as f64) * t);
    }
    let w = 4; // ±100 m
    let climbing: Vec<bool> = (0..m)
        .map(|k| {
            let (a, b) = (k.saturating_sub(w), (k + w).min(m - 1));
            b > a && (rs[b] - rs[a]) / ((b - a) as f64 * STEP_M) >= 0.01
        })
        .collect();
    // Runs, merged across short, shallow interruptions.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < m {
        if !climbing[k] {
            k += 1;
            continue;
        }
        let s = k;
        while k < m && climbing[k] {
            k += 1;
        }
        let e_ = k - 1;
        if let Some(last) = runs.last_mut() {
            let gap = s - last.1;
            let lo = rs[last.1..=s].iter().cloned().fold(f64::MAX, f64::min);
            if (gap as f64) * STEP_M < 400.0 && rs[last.1] - lo < 15.0 {
                last.1 = e_;
                continue;
            }
        }
        runs.push((s, e_));
    }
    let mut out = Vec::new();
    for (s, e_) in runs {
        let lo_range = s.saturating_sub(w)..=(s + w).min(m - 1);
        let hi_range = e_.saturating_sub(w)..=(e_ + w).min(m - 1);
        let a = lo_range.min_by(|&x, &y| rs[x].partial_cmp(&rs[y]).unwrap()).unwrap();
        let b = hi_range.max_by(|&x, &y| rs[x].partial_cmp(&rs[y]).unwrap()).unwrap();
        if b <= a {
            continue;
        }
        let gain = rs[b] - rs[a];
        let len = (b - a) as f64 * STEP_M;
        if gain >= MIN_GAIN_M && len >= MIN_LEN_M && gain / len >= MIN_AVG_GRADE && gain / len <= MAX_AVG_GRADE {
            out.push((a as f64 * STEP_M, b as f64 * STEP_M));
        }
    }
    out
}

pub struct Climbs {
    pub recs: Vec<ClimbRec>,
    pub geom: Vec<[i32; 2]>,
    pub strokes: Vec<Vec<OWay>>,
}

pub fn find(net: &Net, elev: &[f32]) -> Climbs {
    let st = strokes(net);
    let v = net.verts;
    let per: Vec<(Vec<ClimbRec>, Vec<Vec<[i32; 2]>>)> = st
        .par_iter()
        .map(|stroke| {
            let mut recs = Vec::new();
            let mut geoms = Vec::new();
            if stroke.iter().all(|&(w, _)| is_structure(&net.ways[w as usize])) {
                return (recs, geoms);
            }
            let s = series(net, stroke);
            if s.vi.len() < 3 {
                return (recs, geoms);
            }
            let e: Vec<f32> = s.vi.iter().map(|&i| elev[i]).collect();
            let total = *s.dist.last().unwrap();
            // Allowed directions: oneway ways must be traversed forwards.
            let oneway = |forward: bool| {
                stroke.iter().all(|&(w, rev)| net.ways[w as usize].flags & flag::ONEWAY == 0 || rev != forward)
            };
            for forward in [true, false] {
                if !oneway(forward) {
                    continue;
                }
                let (d, ee): (Vec<f64>, Vec<f32>) = if forward {
                    (s.dist.clone(), e.clone())
                } else {
                    (s.dist.iter().rev().map(|x| total - x).collect(), e.iter().rev().cloned().collect())
                };
                for (a, b) in detect(&d, &ee) {
                    // Map distances back to series indices (in forward order).
                    let (fa, fb) = if forward { (a, b) } else { (total - b, total - a) };
                    let ia = s.dist.partition_point(|&x| x < fa).min(s.vi.len() - 1);
                    let ib = s.dist.partition_point(|&x| x < fb).min(s.vi.len() - 1);
                    if ib <= ia {
                        continue;
                    }
                    let (lo_i, hi_i) = if forward { (ia, ib) } else { (ib, ia) };
                    let start_elev = e[lo_i];
                    let top_elev = e[hi_i];
                    let gain = (top_elev - start_elev) as f64;
                    let len = s.dist[ib] - s.dist[ia];
                    if gain < MIN_GAIN_M || len < MIN_LEN_M || gain / len > MAX_AVG_GRADE {
                        continue;
                    }
                    // Steepest 100 m.
                    let mut max_g = 0f64;
                    let mut j = ia;
                    for i in ia..=ib {
                        while s.dist[i] - s.dist[j] > 100.0 {
                            j += 1;
                        }
                        if s.dist[i] - s.dist[j] >= 60.0 {
                            let g = ((e[i] - e[j]) as f64 / (s.dist[i] - s.dist[j])) * if forward { 1.0 } else { -1.0 };
                            max_g = max_g.max(g);
                        }
                    }
                    let mid = (ia + ib) / 2;
                    let mut cls = 0u8;
                    let mut unpaved = 0u8;
                    for k in s.owner[ia]..=s.owner[ib] {
                        let w = &net.ways[stroke[k as usize].0 as usize];
                        cls = cls.max(if w.class == class::FERRY { 0 } else { w.class });
                        if w.flags & flag::UNPAVED != 0 {
                            unpaved = 1;
                        }
                    }
                    // Geometry in travel order, decimated to ≤ 400 points.
                    let idx: Vec<usize> = if forward { (ia..=ib).collect() } else { (ia..=ib).rev().collect() };
                    let step = (idx.len() / 400).max(1);
                    let mut g: Vec<[i32; 2]> = idx.iter().step_by(step).map(|&k| v[s.vi[k]]).collect();
                    if g.last() != Some(&v[s.vi[*idx.last().unwrap()]]) {
                        g.push(v[s.vi[*idx.last().unwrap()]]);
                    }
                    recs.push(ClimbRec {
                        way: stroke[s.owner[if forward { ia } else { ib }] as usize].0,
                        label_way: stroke[s.owner[mid] as usize].0,
                        gain_m: gain as f32,
                        length_m: (s.dist[ib] - s.dist[ia]) as f32,
                        start_elev,
                        top_elev,
                        max_grade: (max_g * 100.0) as f32,
                        mid: v[s.vi[mid]],
                        geom_start: 0,
                        geom_count: g.len() as u32,
                        class: cls,
                        unpaved,
                        _pad: [0; 2],
                    });
                    geoms.push(g);
                }
            }
            (recs, geoms)
        })
        .collect();
    let mut recs = Vec::new();
    let mut geom = Vec::new();
    for (r, g) in per {
        for (mut rec, gg) in r.into_iter().zip(g) {
            rec.geom_start = geom.len() as u32;
            geom.extend_from_slice(&gg);
            recs.push(rec);
        }
    }
    eprintln!("climbs: {} strokes, {} climbs (≥ {MIN_GAIN_M} m, ≥ {:.0} %)", st.len(), recs.len(), MIN_AVG_GRADE * 100.0);
    Climbs { recs, geom, strokes: st }
}
