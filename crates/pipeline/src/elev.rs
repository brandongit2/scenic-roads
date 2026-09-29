//! Elevation post-processing on the road network.
//!
//! 1. Fill DEM gaps along each way (linear by distance), then remove DEM spikes with a
//!    Hampel filter over ±50 m of road.
//! 2. Smooth ordinary roads with a Gaussian (σ = 15 m). The window continues across
//!    junctions into the road's natural continuation (same name/ref, else straightest),
//!    so OSM way boundaries don't create edge effects; it never reaches into bridges or
//!    tunnels, whose DEM samples are meaningless.
//! 3. Junction continuity: every endpoint is pulled to the mean of the ordinary roads
//!    meeting there, blended over 60 m.
//! 4. Bridges, tunnels, ferries: interior node elevations of structure networks are solved
//!    harmonically (weights 1/length) between fixed ends; each structure way is then
//!    interpolated linearly, so decks and tunnel floors are straight.
//! 5. |grade| over ±25 m, again continuing across junctions (using final elevations).

use crate::count_bar;
use rayon::prelude::*;
use roadcore::{class, dist_m, flag, WayRec, E7};
use std::collections::HashMap;

const SMOOTH_SIGMA_M: f64 = 15.0;
const JUNCTION_BLEND_M: f64 = 60.0;
const GRADE_HALF_WINDOW_M: f64 = 25.0;
const MAX_HOPS: usize = 6;

pub fn is_structure(w: &WayRec) -> bool {
    w.class == class::FERRY || w.flags & (flag::BRIDGE | flag::TUNNEL) != 0
}

pub struct Processed {
    pub elev: Vec<f32>,
    pub grade: Vec<u8>,
}

#[inline]
fn d(v: &[[i32; 2]], a: usize, b: usize) -> f64 {
    dist_m(v[a][0] as f64 * E7, v[a][1] as f64 * E7, v[b][0] as f64 * E7, v[b][1] as f64 * E7)
}

pub(crate) fn heading(v: &[[i32; 2]], a: usize, b: usize) -> f64 {
    let lat = v[a][1] as f64 * E7;
    let dx = (v[b][0] - v[a][0]) as f64 * lat.to_radians().cos();
    let dy = (v[b][1] - v[a][1]) as f64;
    dy.atan2(dx)
}

pub(crate) fn turn(a: f64, b: f64) -> f64 {
    let mut t = (a - b).abs();
    if t > std::f64::consts::PI {
        t = 2.0 * std::f64::consts::PI - t;
    }
    t
}

pub struct Net<'a> {
    pub ways: &'a [WayRec],
    pub verts: &'a [[i32; 2]],
    /// Node id of [start, end] of each way.
    pub ends: Vec<[u32; 2]>,
    /// Continuation at [start, end]: (way, true if that way's *start* is at the shared node).
    pub cont: Vec<[Option<(u32, bool)>; 2]>,
    /// Ways at each node: (way, true if the way *starts* there).
    pub inc: Vec<Vec<(u32, bool)>>,
}

impl<'a> Net<'a> {
    pub fn build(ways: &'a [WayRec], verts: &'a [[i32; 2]]) -> Self {
        let mut node_of: HashMap<[i32; 2], u32> = HashMap::with_capacity(ways.len() * 2);
        let mut ends = Vec::with_capacity(ways.len());
        for w in ways {
            let a = verts[w.vstart as usize];
            let b = verts[(w.vstart + w.vcount as u64 - 1) as usize];
            let n = node_of.len() as u32;
            let na = *node_of.entry(a).or_insert(n);
            let n = node_of.len() as u32;
            let nb = *node_of.entry(b).or_insert(n);
            ends.push([na, nb]);
        }
        let n_nodes = node_of.len();
        drop(node_of);
        let mut inc: Vec<Vec<(u32, bool)>> = vec![Vec::new(); n_nodes];
        for (i, e) in ends.iter().enumerate() {
            inc[e[0] as usize].push((i as u32, true));
            inc[e[1] as usize].push((i as u32, false));
        }
        let cont: Vec<[Option<(u32, bool)>; 2]> = (0..ways.len())
            .into_par_iter()
            .map(|i| {
                let w = &ways[i];
                let s = w.vstart as usize;
                let e = s + w.vcount as usize - 1;
                let pick = |node: u32, inward_from: usize, at: usize| -> Option<(u32, bool)> {
                    let h_in = heading(verts, inward_from, at);
                    let cands: Vec<&(u32, bool)> = inc[node as usize].iter().filter(|(c, _)| *c as usize != i).collect();
                    if cands.is_empty() {
                        return None;
                    }
                    let score = |&&(c, starts): &&(u32, bool)| {
                        let cw = &ways[c as usize];
                        let cs = cw.vstart as usize;
                        let ce = cs + cw.vcount as usize - 1;
                        let h_out = if starts { heading(verts, cs, cs + 1) } else { heading(verts, ce, ce - 1) };
                        turn(h_in, h_out)
                    };
                    let same = |c: u32| {
                        let cw = &ways[c as usize];
                        (w.ref_ != 0 && cw.ref_ == w.ref_) || (w.name != 0 && cw.name == w.name)
                    };
                    let best_of = |f: &dyn Fn(u32) -> bool, max_turn: f64| {
                        cands
                            .iter()
                            .filter(|c| f(c.0))
                            .map(|c| (score(c), **c))
                            .filter(|(t, _)| *t < max_turn)
                            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
                            .map(|x| x.1)
                    };
                    if cands.len() == 1 {
                        return best_of(&|_| true, 100f64.to_radians());
                    }
                    best_of(&same, 70f64.to_radians()).or_else(|| best_of(&|_| true, 35f64.to_radians()))
                };
                [pick(ends[i][0], s + 1, s), pick(ends[i][1], e - 1, e)]
            })
            .collect();
        Net { ways, verts, ends, cont, inc }
    }

    /// Samples (distance from the endpoint, vertex index) walking outward from way `w`'s
    /// start (`at_end = false`) or end, following continuations up to `max_m`.
    fn context(&self, w: usize, at_end: bool, max_m: f64, into_structures: bool) -> Vec<(f64, usize)> {
        let mut out = Vec::new();
        let mut acc = 0.0;
        let mut cur = w;
        let mut side = at_end as usize;
        let mut prev_vertex = if at_end {
            (self.ways[w].vstart + self.ways[w].vcount as u64 - 1) as usize
        } else {
            self.ways[w].vstart as usize
        };
        for _ in 0..MAX_HOPS {
            let Some((c, starts)) = self.cont[cur][side] else { break };
            let c = c as usize;
            if c == w || (!into_structures && is_structure(&self.ways[c])) {
                break;
            }
            let cw = &self.ways[c];
            let cs = cw.vstart as usize;
            let n = cw.vcount as usize;
            let idx = |k: usize| if starts { cs + k } else { cs + n - 1 - k };
            for k in 1..n {
                let vi = idx(k);
                acc += d(self.verts, prev_vertex, vi);
                prev_vertex = vi;
                if acc > max_m {
                    return out;
                }
                out.push((acc, vi));
            }
            // Continue from c's far end.
            cur = c;
            side = if starts { 1 } else { 0 };
        }
        out
    }
}

fn cumdist(v: &[[i32; 2]], s: usize, n: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(n);
    let mut acc = 0.0;
    out.push(0.0);
    for k in 1..n {
        acc += d(v, s + k - 1, s + k);
        out.push(acc);
    }
    out
}

fn fill_gaps(e: &mut [f32], dist: &[f64]) -> bool {
    let valid: Vec<usize> = (0..e.len()).filter(|&i| e[i].is_finite()).collect();
    if valid.is_empty() {
        return false;
    }
    if valid.len() == e.len() {
        return true;
    }
    for i in 0..valid[0] {
        e[i] = e[valid[0]];
    }
    let last = *valid.last().unwrap();
    for i in last + 1..e.len() {
        e[i] = e[last];
    }
    for w in valid.windows(2) {
        let (a, b) = (w[0], w[1]);
        for i in a + 1..b {
            let t = ((dist[i] - dist[a]) / (dist[b] - dist[a]).max(1e-9)) as f32;
            e[i] = e[a] + (e[b] - e[a]) * t;
        }
    }
    true
}

/// Extended (distance, value) series for way `w`: context before, the way itself, context after.
fn extended(net: &Net<'_>, w: usize, dist: &[f64], vals: &[f32], reach: f64, into_structures: bool) -> (Vec<f64>, Vec<f32>, usize) {
    let s = net.ways[w].vstart as usize;
    let n = net.ways[w].vcount as usize;
    let before = net.context(w, false, reach, into_structures);
    let after = net.context(w, true, reach, into_structures);
    let total = dist[n - 1];
    let mut xd = Vec::with_capacity(before.len() + n + after.len());
    let mut xv = Vec::with_capacity(xd.capacity());
    for &(dd, vi) in before.iter().rev() {
        if vals[vi].is_finite() {
            xd.push(-dd);
            xv.push(vals[vi]);
        }
    }
    let off = xd.len();
    for k in 0..n {
        xd.push(dist[k]);
        xv.push(vals[s + k]);
    }
    for &(dd, vi) in &after {
        if vals[vi].is_finite() {
            xd.push(total + dd);
            xv.push(vals[vi]);
        }
    }
    (xd, xv, off)
}

/// Hampel filter: replace samples far from the median of the surrounding ±50 m of road
/// (DEM spikes, e.g. bad 3DEP pixels). The threshold (≥ 8 m, 5 scaled MADs) leaves even
/// 40 % ramps untouched.
fn despike(xd: &[f64], xv: &[f32]) -> (Vec<f32>, usize) {
    const HALF: f64 = 50.0;
    let m = xd.len();
    let mut out = xv.to_vec();
    let mut fixed = 0;
    let (mut lo, mut hi) = (0usize, 0usize);
    let mut win: Vec<f32> = Vec::with_capacity(32);
    for i in 0..m {
        while xd[i] - xd[lo] > HALF {
            lo += 1;
        }
        if hi < i {
            hi = i;
        }
        while hi + 1 < m && xd[hi + 1] - xd[i] <= HALF {
            hi += 1;
        }
        if hi - lo < 4 {
            continue;
        }
        win.clear();
        win.extend_from_slice(&xv[lo..=hi]);
        win.sort_unstable_by(|a, b| a.total_cmp(b));
        let med = win[win.len() / 2];
        for x in win.iter_mut() {
            *x = (*x - med).abs();
        }
        win.sort_unstable_by(|a, b| a.total_cmp(b));
        let mad = win[win.len() / 2] * 1.4826;
        if (xv[i] - med).abs() > (5.0 * mad).max(8.0) {
            out[i] = med;
            fixed += 1;
        }
    }
    (out, fixed)
}

fn gaussian_core(xd: &[f64], xv: &[f32], off: usize, n: usize, sigma: f64) -> Vec<f32> {
    let r = 3.0 * sigma;
    let m = xd.len();
    let mut out = vec![0f32; n];
    let (mut lo, mut hi) = (0usize, 0usize);
    for k in 0..n {
        let i = off + k;
        while xd[i] - xd[lo] > r {
            lo += 1;
        }
        if hi < i {
            hi = i;
        }
        while hi + 1 < m && xd[hi + 1] - xd[i] <= r {
            hi += 1;
        }
        let (mut s, mut ws) = (0f64, 0f64);
        for j in lo..=hi {
            let dl = if j > 0 { xd[j] - xd[j - 1] } else { 0.0 };
            let dr = if j + 1 < m { xd[j + 1] - xd[j] } else { 0.0 };
            let span = ((dl + dr) * 0.5).max(0.5);
            let x = (xd[j] - xd[i]) / sigma;
            let w = (-0.5 * x * x).exp() * span;
            s += w * xv[j] as f64;
            ws += w;
        }
        out[k] = (s / ws) as f32;
    }
    out
}

fn grade_core(xd: &[f64], xv: &[f32], off: usize, n: usize) -> Vec<u8> {
    let m = xd.len();
    let mut out = vec![0u8; n];
    let (mut lo, mut hi) = (0usize, 0usize);
    for k in 0..n {
        let i = off + k;
        while xd[i] - xd[lo] > GRADE_HALF_WINDOW_M {
            lo += 1;
        }
        if hi < i {
            hi = i;
        }
        while hi + 1 < m && xd[hi + 1] - xd[i] <= GRADE_HALF_WINDOW_M {
            hi += 1;
        }
        let run = xd[hi] - xd[lo];
        if run >= 10.0 {
            let g = ((xv[hi] - xv[lo]) as f64 / run).abs() * 100.0;
            out[k] = (g * 2.0).round().min(255.0) as u8;
        }
    }
    // Isolated short stubs: fall back to the whole-series slope.
    if out.iter().all(|&g| g == 0) && m >= 2 {
        let run = xd[m - 1] - xd[0];
        if run >= 5.0 {
            let g = ((xv[m - 1] - xv[0]) as f64 / run).abs() * 100.0;
            out.fill((g * 2.0).round().min(255.0) as u8);
        }
    }
    out
}

pub fn process(net: &Net, raw: &[f32]) -> Processed {
    let (ways, verts) = (net.ways, net.verts);
    let n_ways = ways.len();
    let t0 = std::time::Instant::now();

    // 1. gap fill
    let dists: Vec<Vec<f64>> = ways.par_iter().map(|w| cumdist(verts, w.vstart as usize, w.vcount as usize)).collect();
    let filled_parts: Vec<(Vec<f32>, bool)> = ways
        .par_iter()
        .zip(&dists)
        .map(|(w, dist)| {
            let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
            let mut e = raw[r].to_vec();
            let ok = fill_gaps(&mut e, dist);
            (e, ok)
        })
        .collect();
    let ok: Vec<bool> = filled_parts.iter().map(|p| p.1).collect();
    let mut filled: Vec<f32> = Vec::with_capacity(verts.len());
    for (e, good) in &filled_parts {
        if *good {
            filled.extend_from_slice(e);
        } else {
            filled.extend(std::iter::repeat_n(f32::NAN, e.len()));
        }
    }
    drop(filled_parts);

    // 2. despike + smoothing with cross-junction context
    let spikes = std::sync::atomic::AtomicUsize::new(0);
    let pb = count_bar(n_ways as u64, "despike + smooth");
    let smoothed: Vec<Vec<f32>> = (0..n_ways)
        .into_par_iter()
        .map(|i| {
            let w = &ways[i];
            let n = w.vcount as usize;
            let s = w.vstart as usize;
            pb.inc(1);
            if !ok[i] || is_structure(w) {
                return filled[s..s + n].to_vec();
            }
            let (xd, xv, off) = extended(&net, i, &dists[i], &filled, 3.0 * SMOOTH_SIGMA_M + 50.0, false);
            let (xv, fixed) = despike(&xd, &xv);
            if fixed > 0 {
                spikes.fetch_add(fixed, std::sync::atomic::Ordering::Relaxed);
            }
            gaussian_core(&xd, &xv, off, n, SMOOTH_SIGMA_M)
        })
        .collect();
    pb.finish_and_clear();

    // 3. junction values
    let n_nodes = net.ends.iter().map(|e| e[0].max(e[1])).max().unwrap_or(0) as usize + 1;
    let mut sum = vec![0f64; n_nodes];
    let mut cnt = vec![0u32; n_nodes];
    let mut sdeg = vec![0u32; n_nodes];
    for (i, w) in ways.iter().enumerate() {
        let [a, b] = net.ends[i];
        if is_structure(w) {
            sdeg[a as usize] += 1;
            sdeg[b as usize] += 1;
        } else if ok[i] {
            let e = &smoothed[i];
            sum[a as usize] += e[0] as f64;
            cnt[a as usize] += 1;
            sum[b as usize] += *e.last().unwrap() as f64;
            cnt[b as usize] += 1;
        }
    }
    let mut val: Vec<f64> = (0..n_nodes).map(|k| if cnt[k] > 0 { sum[k] / cnt[k] as f64 } else { f64::NAN }).collect();
    let mut unknown = vec![false; n_nodes];
    for (i, w) in ways.iter().enumerate() {
        if !is_structure(w) {
            continue;
        }
        let e = &smoothed[i];
        for (k, ev) in [(net.ends[i][0] as usize, e[0]), (net.ends[i][1] as usize, *e.last().unwrap())] {
            if cnt[k] == 0 {
                if sdeg[k] >= 2 {
                    unknown[k] = true;
                } else if ok[i] {
                    val[k] = ev as f64;
                }
            }
        }
    }
    // 4. harmonic solve for structure-interior nodes
    let mut adj: Vec<Vec<(u32, f64)>> = vec![Vec::new(); n_nodes];
    for (i, w) in ways.iter().enumerate() {
        if !is_structure(w) {
            continue;
        }
        let len = dists[i].last().copied().unwrap_or(0.0).max(1.0);
        let [a, b] = net.ends[i];
        if unknown[a as usize] {
            adj[a as usize].push((b, 1.0 / len));
        }
        if unknown[b as usize] {
            adj[b as usize].push((a, 1.0 / len));
        }
    }
    let unk: Vec<usize> = (0..n_nodes).filter(|&k| unknown[k]).collect();
    for &k in &unk {
        val[k] = f64::NAN;
    }
    for _ in 0..2000 {
        let mut delta = 0f64;
        for &k in &unk {
            let (mut s, mut ws) = (0f64, 0f64);
            for &(o, w) in &adj[k] {
                let v = val[o as usize];
                if v.is_finite() {
                    s += v * w;
                    ws += w;
                }
            }
            if ws > 0.0 {
                let nv = s / ws;
                let old = val[k];
                delta = delta.max(if old.is_finite() { (nv - old).abs() } else { 1e9 });
                val[k] = nv;
            }
        }
        if delta < 0.005 {
            break;
        }
    }

    // 5. final elevations
    let finals: Vec<Vec<f32>> = smoothed
        .into_par_iter()
        .enumerate()
        .map(|(i, mut e)| {
            let w = &ways[i];
            let dist = &dists[i];
            let total = *dist.last().unwrap();
            let (va, vb) = (val[net.ends[i][0] as usize], val[net.ends[i][1] as usize]);
            if is_structure(w) {
                let a = if va.is_finite() { va } else if ok[i] { e[0] as f64 } else { 0.0 };
                let b = if vb.is_finite() { vb } else if ok[i] { *e.last().unwrap() as f64 } else { a };
                for (k, x) in e.iter_mut().enumerate() {
                    let t = if total > 0.0 { dist[k] / total } else { 0.0 };
                    *x = (a + (b - a) * t) as f32;
                }
            } else if ok[i] {
                let cs = if va.is_finite() { va - e[0] as f64 } else { 0.0 };
                let ce = if vb.is_finite() { vb - *e.last().unwrap() as f64 } else { 0.0 };
                let l = JUNCTION_BLEND_M.min(total).max(1e-6);
                for (k, x) in e.iter_mut().enumerate() {
                    let c = cs * (1.0 - dist[k] / l).max(0.0) + ce * (1.0 - (total - dist[k]) / l).max(0.0);
                    *x += c as f32;
                }
            } else {
                e.iter_mut().for_each(|x| *x = 0.0);
            }
            e
        })
        .collect();
    let mut elev: Vec<f32> = Vec::with_capacity(verts.len());
    for e in &finals {
        elev.extend_from_slice(e);
    }
    drop(finals);

    // 6. grade with cross-junction context on final elevations
    let pb = count_bar(n_ways as u64, "grade");
    let grades: Vec<Vec<u8>> = (0..n_ways)
        .into_par_iter()
        .map(|i| {
            let n = ways[i].vcount as usize;
            let (xd, xv, off) = extended(&net, i, &dists[i], &elev, GRADE_HALF_WINDOW_M, true);
            pb.inc(1);
            grade_core(&xd, &xv, off, n)
        })
        .collect();
    pb.finish_and_clear();
    let mut grade = Vec::with_capacity(verts.len());
    for g in grades {
        grade.extend_from_slice(&g);
    }
    eprintln!(
        "elevation processing: {} ways, {} DEM spike samples replaced, {} structure-interior nodes solved ({:.1?})",
        n_ways,
        spikes.into_inner(),
        unk.len(),
        t0.elapsed()
    );
    Processed { elev, grade }
}
