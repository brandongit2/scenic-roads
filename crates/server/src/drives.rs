//! "Scenic drives": the best stretch of each road stroke in view, scored with the client's
//! current weights.
//!
//! Score components (all 0..1) — must match `web/src/scenic.ts`:
//!   0 views      VIEW/255              6 unblocked  1 − ENCLOSURE/255
//!   1 water      WATER/255             7 forest     COVER/255
//!   2 vista      VISTA/255             8 built-up   BUILT/255 (usually a negative weight)
//!   3 relief     min(1, RELIEF·3/600)  9 roadside buildings  BLDG/255 (negative by default)
//!   4 ridge      clamp((TPI−128)·2/60) 10 scenic route  flag
//!   5 curvy      min(1, CURVY·4/400)   11 viewpoint     flag
//! score = Σ wᵢcᵢ / Σ max(wᵢ, 0), clamped to 0..1.

use crate::S;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rayon::prelude::*;
use roadcore::scenic::{ch, flag, Sample};
use roadcore::{class, Array, Ways, E7};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const NCOMP: usize = 12;

pub fn components(c: &[u8; ch::N]) -> [f32; NCOMP] {
    let f = c[ch::FLAGS];
    let b = |m: u8| (f & m != 0) as u8 as f32;
    [
        c[ch::VIEW] as f32 / 255.0,
        c[ch::WATER] as f32 / 255.0,
        c[ch::VISTA] as f32 / 255.0,
        (c[ch::RELIEF] as f32 * 3.0 / 600.0).min(1.0),
        ((c[ch::TPI] as f32 - 128.0) * 2.0 / 60.0).clamp(0.0, 1.0),
        (c[ch::CURVY] as f32 * 4.0 / 400.0).min(1.0),
        1.0 - c[ch::ENCLOSURE] as f32 / 255.0,
        c[ch::COVER] as f32 / 255.0,
        c[ch::BUILT] as f32 / 255.0,
        c[ch::BLDG] as f32 / 255.0,
        b(flag::SCENIC_ROUTE),
        b(flag::VIEWPOINT),
    ]
}

pub fn score(c: &[u8; ch::N], w: &[f32; NCOMP], wsum: f32) -> f32 {
    let k = components(c);
    let s: f32 = k.iter().zip(w).map(|(a, b)| a * b).sum();
    (s / wsum).clamp(0.0, 1.0)
}

pub struct DriveIndex {
    samples: Array<Sample>,
    ch: Array<[u8; ch::N]>,
    /// Per stroke: range into `seq`/`dist`.
    off: Vec<u32>,
    seq: Vec<u32>,
    dist: Vec<f32>,
    bbox: Vec<[i32; 4]>,
}

impl DriveIndex {
    pub fn open(dir: &Path, ways: &Ways) -> anyhow::Result<Self> {
        let samples = Array::<Sample>::open(&dir.join("samples.bin"))?;
        let chs = Array::<[u8; ch::N]>::open(&dir.join("samples.ch.u8"))?;
        let s = samples.get();
        anyhow::ensure!(chs.get().len() == s.len(), "samples.ch.u8 does not match samples.bin");
        let n_ways = ways.ways().len();
        let mut range = vec![(0u32, 0u32); n_ways];
        let mut k = 0;
        while k < s.len() {
            let w = s[k].way as usize;
            let a = k;
            while k < s.len() && s[k].way as usize == w {
                k += 1;
            }
            range[w] = (a as u32, k as u32);
        }
        let so: Vec<u32> = bytemuck::pod_collect_to_vec(&std::fs::read(dir.join("strokes.off"))?);
        let si: Vec<u32> = bytemuck::pod_collect_to_vec(&std::fs::read(dir.join("strokes.u32"))?);
        let (mut off, mut seq, mut dist, mut bbox) = (vec![0u32], Vec::new(), Vec::new(), Vec::new());
        for st in so.windows(2) {
            let mut acc = 0f32;
            let mut bb = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for &item in &si[st[0] as usize..st[1] as usize] {
                let (w, rev) = ((item & 0x7fff_ffff) as usize, item >> 31 == 1);
                let (a, b) = range[w];
                if a == b {
                    continue;
                }
                // Way length from its symmetric samples: first + last distance.
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
            bbox.push(bb);
        }
        Ok(Self { samples, ch: chs, off, seq, dist, bbox })
    }
}

#[derive(Deserialize)]
pub struct Q {
    bbox: String,
    /// Outline of the ground in view (see `Region`).
    poly: Option<String>,
    /// Comma-separated weights, NCOMP values.
    w: String,
    /// Stretch length, km.
    len: Option<f32>,
    limit: Option<usize>,
    classes: Option<u32>,
    surface: Option<u8>,
    /// Bit 0 toll-free, bit 1 toll.
    toll: Option<u8>,
    /// Classes (bits) whose unnamed roads (no name, no ref) are left out.
    unnamed: Option<u32>,
    /// Whole-road length filter, metres (0 = no limit).
    lmin: Option<f32>,
    lmax: Option<f32>,
}

#[derive(Serialize)]
pub struct Drive {
    score: f32,
    length_m: f32,
    way: u32,
    name: String,
    r#ref: String,
    route: String,
    class: &'static str,
    parts: [f32; NCOMP],
    geom: Vec<[f64; 2]>,
}

#[derive(Serialize)]
pub struct Out {
    total: usize,
    drives: Vec<Drive>,
}

pub async fn drives(State(s): State<S>, Query(q): Query<Q>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || compute(&s2, q)).await {
        Ok(Some(o)) => Json(o).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn compute(st: &crate::AppState, q: Q) -> Option<Out> {
    let ix = st.drives.as_ref()?;
    let region = crate::Region::parse(&q.bbox, q.poly.as_deref())?;
    let bb = region.bb;
    let wv: Vec<f32> = q.w.split(',').filter_map(|x| x.parse().ok()).collect();
    if wv.len() != NCOMP {
        return None;
    }
    let mut w = [0f32; NCOMP];
    w.copy_from_slice(&wv);
    let wsum = w.iter().map(|x| x.max(0.0)).sum::<f32>().max(1e-6);
    let len = q.len.unwrap_or(5.0).clamp(0.5, 100.0) * 1000.0;
    let classes = q.classes.unwrap_or(u32::MAX);
    let surface = q.surface.unwrap_or(3);
    let toll = q.toll.unwrap_or(3);
    let unnamed = q.unnamed.unwrap_or(0);
    let (samples, chs) = (ix.samples.get(), ix.ch.get());
    let ways = st.ways.ways();

    let hits: Vec<(f32, usize, usize, usize)> = (0..ix.bbox.len())
        .into_par_iter()
        .filter_map(|k| {
            let sb = ix.bbox[k];
            if sb[2] < bb[0] || sb[0] > bb[2] || sb[3] < bb[1] || sb[1] > bb[3] {
                return None;
            }
            let (a, e) = (ix.off[k] as usize, ix.off[k + 1] as usize);
            if e <= a {
                return None;
            }
            let w0 = &ways[samples[ix.seq[a] as usize].way as usize];
            let unp = (w0.flags & roadcore::flag::UNPAVED != 0) as u8;
            let tl = (w0.flags & roadcore::flag::TOLL != 0) as u8;
            if (classes >> w0.class) & 1 == 0 || (surface >> unp) & 1 == 0 || (toll >> tl) & 1 == 0 {
                return None;
            }
            if (unnamed >> w0.class) & 1 == 1 && w0.name == 0 && w0.ref_ == 0 {
                return None;
            }
            if !st.road_len_ok(samples[ix.seq[a] as usize].way, q.lmin, q.lmax) {
                return None;
            }
            // Only roads with a continuous stretch of the chosen length ("best n km of each road").
            let total = ix.dist[e - 1] - ix.dist[a];
            if total < len {
                return None;
            }
            // Prefix sums of score × spacing for windowed means.
            let sc: Vec<f32> = (a..e).map(|i| score(&chs[ix.seq[i] as usize], &w, wsum)).collect();
            let mut pre = vec![0f64; sc.len() + 1];
            for (i, v) in sc.iter().enumerate() {
                pre[i + 1] = pre[i] + *v as f64;
            }
            let mut best: Option<(f32, usize, usize)> = None;
            let mut j = 0usize;
            for i in 0..sc.len() {
                if j < i {
                    j = i;
                }
                while j + 1 < sc.len() && ix.dist[a + j] - ix.dist[a + i] < len {
                    j += 1;
                }
                if ix.dist[a + j] - ix.dist[a + i] < len {
                    break;
                }
                let m = ((pre[j + 1] - pre[i]) / (j + 1 - i) as f64) as f32;
                let mid = &samples[ix.seq[a + (i + j) / 2] as usize];
                let inside = region.contains(mid.lon, mid.lat);
                if inside && best.is_none_or(|b| m > b.0) {
                    best = Some((m, a + i, a + j));
                }
            }
            best.map(|(m, i, j)| (m, k, i, j))
        })
        .collect();
    let total = hits.len();
    let mut hits = hits;
    let limit = q.limit.unwrap_or(20).min(100);
    hits.sort_by(|x, y| y.0.total_cmp(&x.0));
    hits.truncate(limit);
    let drives = hits
        .into_iter()
        .map(|(m, _k, i, j)| {
            let mid = &samples[ix.seq[(i + j) / 2] as usize];
            let lw = &ways[mid.way as usize];
            let mut parts = [0f32; NCOMP];
            for t in i..=j {
                for (p, v) in parts.iter_mut().zip(components(&chs[ix.seq[t] as usize])) {
                    *p += v;
                }
            }
            let n = (j + 1 - i) as f32;
            parts.iter_mut().for_each(|p| *p /= n);
            let first = &samples[ix.seq[i] as usize];
            Drive {
                score: m * 100.0,
                length_m: ix.dist[j] - ix.dist[i],
                way: first.way,
                name: st.strings[lw.name as usize].clone(),
                r#ref: st.strings[lw.ref_ as usize].clone(),
                route: st.strings[lw.route as usize].clone(),
                class: class::NAMES[lw.class as usize],
                parts,
                geom: (i..=j).map(|t| {
                    let p = &samples[ix.seq[t] as usize];
                    [p.lon as f64 * E7, p.lat as f64 * E7]
                }).collect(),
            }
        })
        .collect();
    Some(Out { total, drives })
}
