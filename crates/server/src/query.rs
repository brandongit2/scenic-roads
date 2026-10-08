//! Scenic drives, scenic rides and rail lines in view, from the query parts of the z6 tiles in view
//! (docs/formats.md "Hi data"): each road's ~100 m samples with their offsets along the road. Parts
//! of one road from several tiles join by offset, so a stretch is scored the same whichever tiles
//! it crosses. A drive's midpoint must be in view, so tiles within half the stretch length of the
//! view are read too.
//!
//! Zoomed out (`approx=1`; docs/phase5.md "Zoomed-out queries"), from the tiles' summaries instead:
//! 500 m bins of the long roads and of the rail, each standing for its samples spread evenly along
//! it, so the same scans run on a tenth of the bytes. Every answer says which it came from.
//!
//! Scores as before (`web/src/scenic.ts`, `web/src/rail.ts`): `roadcore::scenic`'s components.

use crate::views::{HiView, LTile, QTile};
use crate::ways::{find_way, len_ok, tiles_in};
use crate::{AppState, Region, S};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rayon::prelude::*;
use roadcore::lsum::{self, LO_MIN_ROAD, LSUM_V, NO_RINFO};
use roadcore::packs::{LBin, LPart, PSample};
use roadcore::scenic::{self, ch, FREQ, NCOMP, RNCOMP};
use roadcore::{class, E7};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Samples further apart than this along a road aren't consecutive (a gap, or the road leaving the
/// tiles read).
const GAP_M: f32 = 300.0;

pub fn score(c: &[u8; ch::N], w: &[f32; NCOMP], wsum: f32) -> f32 {
    score_of(&scenic::drive_components(c), w, wsum)
}

fn score_of(k: &[f32; NCOMP], w: &[f32; NCOMP], wsum: f32) -> f32 {
    let s: f32 = k.iter().zip(w).map(|(a, b)| a * b).sum();
    (s / wsum).clamp(0.0, 1.0)
}

fn ride_score_of(k: &[f32; RNCOMP], known: bool, w: &[f32; RNCOMP]) -> f32 {
    let pos: f32 = w.iter().enumerate().map(|(i, v)| if i == FREQ && !known { 0.0 } else { v.max(0.0) }).sum::<f32>().max(1e-6);
    (k.iter().zip(w).map(|(a, b)| a * b).sum::<f32>() / pos).clamp(0.0, 1.0)
}

/// Trains a day of a way, from `AppState::rail_freqs`.
pub fn freq_in(v: &[(u32, f32)], id: u64) -> f32 {
    v.binary_search_by_key(&(id as u32), |x| x.0).map(|k| v[k].1).unwrap_or(0.0)
}

// ---- cancellation ---------------------------------------------------------------------------

/// Set once the request's client has gone: the work stops at its next check.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    fn check(&self) -> anyhow::Result<()> {
        if self.is_set() {
            Err(Cancelled.into())
        } else {
            Ok(())
        }
    }
}

/// Held by a handler: a client closing the connection makes hyper drop the handler's future, and
/// this sets the flag (the blocking work isn't stopped by the drop itself).
struct SetOnDrop(Cancel);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0 .0.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A query's answer: 503 when the NAS couldn't be read, 400 for a bad request.
fn answer<T: serde::Serialize>(r: Result<anyhow::Result<Option<T>>, tokio::task::JoinError>, what: &str) -> Response {
    match r {
        Ok(Ok(Some(o))) => Json(o).into_response(),
        Ok(Ok(None)) => StatusCode::BAD_REQUEST.into_response(),
        // (Nobody's waiting for it.)
        Ok(Err(e)) if e.is::<Cancelled>() => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Ok(Err(e)) => {
            eprintln!("{what}: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Runs a query on a blocking thread, stopping it if the client goes.
async fn run<T: Serialize + Send + 'static>(what: &'static str, f: impl FnOnce(&Cancel) -> anyhow::Result<Option<T>> + Send + 'static) -> Response {
    let cancel = Cancel::default();
    let _stop = SetOnDrop(cancel.clone());
    let r = tokio::task::spawn_blocking(move || f(&cancel)).await;
    answer(r, what)
}

/// The first error met in a query's (parallel) lookups: a NAS read that failed makes the answer a
/// 503, never a list that looks complete without what couldn't be read.
#[derive(Default)]
struct FirstErr(std::sync::Mutex<Option<anyhow::Error>>);

impl FirstErr {
    fn keep<T>(&self, r: anyhow::Result<Option<T>>) -> Option<T> {
        match r {
            Ok(v) => v,
            Err(e) => {
                self.0.lock().unwrap().get_or_insert(e);
                None
            }
        }
    }
    fn check(&self) -> anyhow::Result<()> {
        match self.0.lock().unwrap().take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

// ---- tiles ----------------------------------------------------------------------------------

/// The view's box grown by `margin_km` (E7).
fn grown(region: &Region, margin_km: f64) -> [i32; 4] {
    let lat = (region.bb[1].unsigned_abs().max(region.bb[3].unsigned_abs()) as f64 * E7).min(85.0);
    let dlat = (margin_km / 111.32 / E7) as i32;
    let dlon = (margin_km / 111.32 / lat.to_radians().cos().max(0.05) / E7) as i32;
    [region.bb[0].saturating_sub(dlon), region.bb[1].saturating_sub(dlat), region.bb[2].saturating_add(dlon), region.bb[3].saturating_add(dlat)]
}

/// The hidata of the z6 tiles in view plus a margin; an error when one can't be read (a query
/// over part of the view would look complete).
fn tiles_for(s: &AppState, region: &Region, margin_km: f64, cancel: &Cancel) -> anyhow::Result<Vec<QTile>> {
    let got: Vec<anyhow::Result<Option<QTile>>> = tiles_in(grown(region, margin_km))
        .into_par_iter()
        .map(|(x, y)| {
            cancel.check()?;
            s.data.hidata(&format!("6/{x}/{y}"))?.map(QTile::new).transpose()
        })
        .collect();
    let mut out = Vec::with_capacity(got.len());
    for g in got {
        if let Some(h) = g? {
            out.push(h);
        }
    }
    Ok(out)
}

/// The summaries (roads' or rail's) of the z6 tiles in view plus a margin; None when one of those
/// tiles has none (older hidata): the query is answered exactly then, never half and half.
fn ltiles_for(s: &AppState, region: &Region, margin_km: f64, rail: bool, cancel: &Cancel) -> anyhow::Result<Option<Vec<LTile>>> {
    let views: Vec<anyhow::Result<Option<Arc<HiView>>>> = tiles_in(grown(region, margin_km))
        .into_par_iter()
        .map(|(x, y)| {
            cancel.check()?;
            s.data.hidata(&format!("6/{x}/{y}"))
        })
        .collect();
    let mut hvs = Vec::with_capacity(views.len());
    for v in views {
        if let Some(h) = v? {
            if h.lsum != LSUM_V {
                return Ok(None);
            }
            hvs.push(h);
        }
    }
    let got: Vec<anyhow::Result<LTile>> = hvs
        .par_iter()
        .map(|h| {
            cancel.check()?;
            LTile::new(h, rail)
        })
        .collect();
    Ok(Some(got.into_iter().collect::<anyhow::Result<Vec<_>>>()?))
}

// ---- runs and windows -----------------------------------------------------------------------

/// One sample of a road, wherever its tile.
#[derive(Clone, Copy)]
struct Smp {
    road: u64,
    off: f32,
    tile: u32,
    k: u32,
}

/// Every sample of the tiles' parts passing `keep(tile, part sample)`, in road and offset order,
/// cut into runs of consecutive samples. Roads shorter than `min_len` (m) are left out: no window
/// that long fits on them (drives, rides; rail lines want every run).
fn runs(tiles: &[QTile], rail: bool, min_len: f32, keep: &(dyn Fn(&QTile, &PSample, f32) -> bool + Sync)) -> Vec<Vec<Smp>> {
    let mut all: Vec<Smp> = tiles
        .par_iter()
        .enumerate()
        .flat_map_iter(|(ti, hv)| {
            let ps = hv.psamples();
            let mut out = Vec::new();
            for p in hv.parts() {
                if class::is_rail(p.class) != rail || p.road_len < min_len {
                    continue;
                }
                for k in p.first..p.first + p.count {
                    let smp = &ps[k as usize];
                    if keep(hv, smp, p.road_len) {
                        out.push(Smp { road: p.road, off: smp.offset, tile: ti as u32, k });
                    }
                }
            }
            out.into_iter()
        })
        .collect();
    all.par_sort_unstable_by(|a, b| a.road.cmp(&b.road).then(a.off.total_cmp(&b.off)).then(a.tile.cmp(&b.tile)).then(a.k.cmp(&b.k)));
    let mut out: Vec<Vec<Smp>> = Vec::new();
    for (i, s) in all.iter().enumerate() {
        if i == 0 || all[i - 1].road != s.road || s.off - all[i - 1].off > GAP_M {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push(*s);
    }
    out
}

/// The best window (mean score) at least `len` long whose middle sample passes `mid_ok`, over
/// samples at ascending offsets `off`: (mean, first index, last index).
fn best_window(off: &[f32], sc: &[f32], len: f32, mid_ok: &dyn Fn(usize) -> bool) -> Option<(f32, usize, usize)> {
    let mut pre = vec![0f64; sc.len() + 1];
    for (i, v) in sc.iter().enumerate() {
        pre[i + 1] = pre[i] + *v as f64;
    }
    let mut best: Option<(f32, usize, usize)> = None;
    let mut j = 0usize;
    for i in 0..sc.len() {
        j = j.max(i);
        while j + 1 < sc.len() && off[j] - off[i] < len {
            j += 1;
        }
        if off[j] - off[i] < len {
            break;
        }
        let m = ((pre[j + 1] - pre[i]) / (j + 1 - i) as f64) as f32;
        if mid_ok((i + j) / 2) && best.is_none_or(|b| m > b.0) {
            best = Some((m, i, j));
        }
    }
    best
}

fn sample<'a>(tiles: &'a [QTile], s: &Smp) -> (&'a QTile, &'a PSample, &'a [u8; ch::N]) {
    let hv = &tiles[s.tile as usize];
    (hv, &hv.psamples()[s.k as usize], &hv.pch()[s.k as usize])
}

fn deg(lon: i32, lat: i32) -> [f64; 2] {
    [lon as f64 * E7, lat as f64 * E7]
}

/// A summary bin in a run: its tile and index, and its span along the road.
#[derive(Clone, Copy)]
struct BRef {
    road: u64,
    off0: f32,
    off1: f32,
    tile: u32,
    k: u32,
}

fn bin_of<'a>(tiles: &'a [LTile], r: &BRef) -> &'a LBin {
    &tiles[r.tile as usize].bins()[r.k as usize]
}

/// Every summary bin of the tiles' parts passing `keep(part, bin)`, in road and offset order, cut
/// into runs: one ends where the next bin starts more than `GAP_M` past the furthest the run has
/// reached (bins of a road zigzagging over a tile edge overlap). Roads shorter than `min_len` are
/// left out.
fn lruns(tiles: &[LTile], min_len: f32, keep: &(dyn Fn(&LPart, &LBin) -> bool + Sync)) -> Vec<Vec<BRef>> {
    let mut all: Vec<BRef> = tiles
        .par_iter()
        .enumerate()
        .flat_map_iter(|(ti, t)| {
            let bins = t.bins();
            let mut out = Vec::new();
            for p in t.parts() {
                if p.road_len < min_len {
                    continue;
                }
                for k in p.first..p.first + p.count {
                    let b = &bins[k as usize];
                    if keep(p, b) {
                        out.push(BRef { road: p.road, off0: b.off0, off1: b.off0 + b.len, tile: ti as u32, k });
                    }
                }
            }
            out.into_iter()
        })
        .collect();
    all.par_sort_unstable_by(|a, b| a.road.cmp(&b.road).then(a.off0.total_cmp(&b.off0)).then(a.tile.cmp(&b.tile)).then(a.k.cmp(&b.k)));
    let mut out: Vec<Vec<BRef>> = Vec::new();
    let mut reach = 0f32;
    for (i, b) in all.iter().enumerate() {
        if i == 0 || all[i - 1].road != b.road || b.off0 - reach > GAP_M {
            out.push(Vec::new());
            reach = b.off1;
        }
        reach = reach.max(b.off1);
        out.last_mut().unwrap().push(*b);
    }
    out
}

/// A run's bins as the samples they stand for: each bin's `n` spread evenly from its first sample
/// to its last, merged in offset order: (offset, the bin's index in the run, place along it 0–1).
fn pseudo(run: &[BRef], tiles: &[LTile]) -> Vec<(f32, u32, f32)> {
    let mut v = Vec::new();
    for (bi, r) in run.iter().enumerate() {
        let b = bin_of(tiles, r);
        let n = b.n.max(1) as usize;
        for t in 0..n {
            let u = if n > 1 { t as f32 / (n - 1) as f32 } else { 0.0 };
            v.push((b.off0 + b.len * u, bi as u32, u));
        }
    }
    // (Stable: the bins come in offset order; only overlapping ones interleave.)
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

/// Where the middle sample sits along a bin (0–1).
fn mid_u(b: &LBin) -> f32 {
    if b.n > 1 {
        (b.n / 2) as f32 / (b.n - 1) as f32
    } else {
        0.0
    }
}

/// The point at `u` (0–1) along a bin, on its first → middle → last samples (E7).
fn bin_point(b: &LBin, u: f32) -> (i32, i32) {
    let lerp = |a: (i32, i32), c: (i32, i32), t: f32| {
        let t = t.clamp(0.0, 1.0) as f64;
        ((a.0 as f64 + (c.0 - a.0) as f64 * t).round() as i32, (a.1 as f64 + (c.1 - a.1) as f64 * t).round() as i32)
    };
    let um = mid_u(b);
    let (p0, pm, p1) = ((b.lon0, b.lat0), (b.lonm, b.latm), (b.lon1, b.lat1));
    if u <= um {
        if um <= 0.0 {
            p0
        } else {
            lerp(p0, pm, u / um)
        }
    } else {
        lerp(pm, p1, (u - um) / (1.0 - um))
    }
}

/// A bin's real samples (first, middle, last; fewer when it has fewer): offset and position.
fn real_points(b: &LBin) -> Vec<(f32, [f64; 2])> {
    match b.n {
        0 | 1 => vec![(b.off0, deg(b.lon0, b.lat0))],
        2 => vec![(b.off0, deg(b.lon0, b.lat0)), (b.off0 + b.len, deg(b.lon1, b.lat1))],
        _ => vec![(b.off0, deg(b.lon0, b.lat0)), (b.off0 + b.len * mid_u(b), deg(b.lonm, b.latm)), (b.off0 + b.len, deg(b.lon1, b.lat1))],
    }
}

/// A window's geometry from real sample positions only (a chord can leave a hairpin road): its
/// bins' first, middle and last samples, from the one nearest its start to the one nearest its
/// end, in offset order.
fn window_geom(run: &[BRef], tiles: &[LTile], ps: &[(f32, u32, f32)], i: usize, j: usize) -> Vec<[f64; 2]> {
    let (a, b) = (ps[i].0, ps[j].0);
    let mut bins: Vec<u32> = ps[i..=j].iter().map(|x| x.1).collect();
    bins.sort_unstable();
    bins.dedup();
    let mut pts: Vec<(f32, [f64; 2])> = bins.iter().flat_map(|&k| real_points(bin_of(tiles, &run[k as usize]))).collect();
    pts.sort_by(|x, y| x.0.total_cmp(&y.0));
    let nearest = |at: f32| pts.iter().enumerate().min_by(|x, y| (x.1 .0 - at).abs().total_cmp(&(y.1 .0 - at).abs())).map_or(0, |x| x.0);
    let (s, e) = (nearest(a), nearest(b));
    pts[s.min(e)..=s.max(e)].iter().map(|p| p.1).collect()
}

/// The real sample of a bin nearest an offset along the road.
fn nearest_point(b: &LBin, at: f32) -> [f64; 2] {
    real_points(b).into_iter().min_by(|x, y| (x.0 - at).abs().total_cmp(&(y.0 - at).abs())).map_or(deg(b.lon0, b.lat0), |p| p.1)
}

fn comps<const N: usize>(b: &LBin) -> [f32; N] {
    let mut k = [0f32; N];
    for (v, c) in k.iter_mut().zip(b.comp) {
        *v = c as f32 / 255.0;
    }
    k
}

// ---- drives ---------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct Q {
    bbox: String,
    poly: Option<String>,
    w: String,
    len: Option<f32>,
    limit: Option<usize>,
    classes: Option<u32>,
    surface: Option<u8>,
    toll: Option<u8>,
    unnamed: Option<u32>,
    lmin: Option<f32>,
    lmax: Option<f32>,
    /// 1: from the summaries when every tile has them (zoomed out).
    approx: Option<u8>,
}

#[derive(Serialize)]
pub struct Drive {
    score: f32,
    length_m: f32,
    way: u64,
    /// A point on the first way (for `?at=`).
    at: [f64; 2],
    name: String,
    main: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    name_en: String,
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
    /// From the summaries (zoomed out).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    approx: bool,
}

/// What a drives query asks for.
pub struct DriveParams {
    pub len: f32,
    pub w: [f32; NCOMP],
    pub wsum: f32,
    pub classes: u32,
    pub surface: u8,
    pub toll: u8,
    pub unnamed: u32,
    pub lmin: Option<f32>,
    pub lmax: Option<f32>,
    pub limit: usize,
}

impl DriveParams {
    /// Whether ways of this class and these attributes (`LBin::flags` bits) are searched.
    fn keeps(&self, class: u8, flags: u8) -> bool {
        let (unp, tl, unn) = (flags & 1, (flags >> 1) & 1, flags & 4 != 0);
        class != class::FERRY
            && (self.classes >> class) & 1 == 1
            && (self.surface >> unp) & 1 == 1
            && (self.toll >> tl) & 1 == 1
            && !((self.unnamed >> class) & 1 == 1 && unn)
    }
}

/// A drive found: its window, and what the list shows but the names (from `mid`'s way).
pub struct DriveHit {
    /// Mean score, 0–1.
    pub score: f32,
    pub length_m: f32,
    /// The road and the window's offsets along it (what the tests match hits by).
    #[cfg_attr(not(test), allow(dead_code))]
    pub road: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub a: f32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub b: f32,
    /// The first way and a point on it, and the middle's.
    pub first: (u64, [f64; 2]),
    pub mid: (u64, [f64; 2]),
    pub parts: [f32; NCOMP],
    pub geom: Vec<[f64; 2]>,
}

/// The best drives from the tiles' samples: (how many roads have one, the best `limit`).
fn drives_exact(tiles: &[QTile], region: &Region, p: &DriveParams, cancel: &Cancel) -> anyhow::Result<(usize, Vec<DriveHit>)> {
    let keep = |hv: &QTile, s: &PSample, road_len: f32| {
        let (c, f) = lsum::attrs(&hv.here()[s.way as usize]);
        p.keeps(c, f) && len_ok(road_len, p.lmin, p.lmax)
    };
    let runs = runs(tiles, false, p.len, &keep);
    cancel.check()?;
    let in_view = |s: &Smp| {
        let (_, ps, _) = sample(tiles, s);
        region.contains(ps.lon, ps.lat)
    };
    let mut hits: Vec<(f32, usize, usize, usize)> = runs
        .par_iter()
        .enumerate()
        .filter_map(|(ri, run)| {
            if cancel.is_set() || run.len() < 2 || run[run.len() - 1].off - run[0].off < p.len {
                return None;
            }
            let sc: Vec<f32> = run.iter().map(|s| score(sample(tiles, s).2, &p.w, p.wsum)).collect();
            let off: Vec<f32> = run.iter().map(|s| s.off).collect();
            best_window(&off, &sc, p.len, &|m| in_view(&run[m])).map(|(m, i, j)| (m, ri, i, j))
        })
        .collect();
    cancel.check()?;
    let total = hits.len();
    hits.sort_by(|x, y| y.0.total_cmp(&x.0));
    hits.truncate(p.limit);
    let out = hits
        .into_iter()
        .map(|(m, ri, i, j)| {
            let run = &runs[ri];
            let (hv_m, pm, _) = sample(tiles, &run[(i + j) / 2]);
            let (hv_f, pf, _) = sample(tiles, &run[i]);
            let mut parts = [0f32; NCOMP];
            for s in &run[i..=j] {
                for (q, v) in parts.iter_mut().zip(scenic::drive_components(sample(tiles, s).2)) {
                    *q += v;
                }
            }
            let n = (j + 1 - i) as f32;
            parts.iter_mut().for_each(|q| *q /= n);
            DriveHit {
                score: m,
                length_m: run[j].off - run[i].off,
                road: run[i].road,
                a: run[i].off,
                b: run[j].off,
                first: (hv_f.here()[pf.way as usize].id, deg(pf.lon, pf.lat)),
                mid: (hv_m.here()[pm.way as usize].id, deg(pm.lon, pm.lat)),
                parts,
                geom: run[i..=j].iter().map(|s| {
                    let q = sample(tiles, s).1;
                    deg(q.lon, q.lat)
                }).collect(),
            }
        })
        .collect();
    Ok((total, out))
}

/// The best drives from the tiles' summaries, as `drives_exact`.
fn drives_approx(tiles: &[LTile], region: &Region, p: &DriveParams, cancel: &Cancel) -> anyhow::Result<(usize, Vec<DriveHit>)> {
    let keep = |lp: &LPart, b: &LBin| p.keeps(b.class, b.flags) && len_ok(lp.road_len, p.lmin, p.lmax);
    let runs = lruns(tiles, p.len, &keep);
    cancel.check()?;
    let mut hits: Vec<(f32, usize, usize, usize)> = runs
        .par_iter()
        .enumerate()
        .filter_map(|(ri, run)| {
            if cancel.is_set() || run.iter().map(|b| b.off1).fold(f32::MIN, f32::max) - run[0].off0 < p.len {
                return None;
            }
            let ps = pseudo(run, tiles);
            if ps.len() < 2 {
                return None;
            }
            let bsc: Vec<f32> = run.iter().map(|r| score_of(&comps::<NCOMP>(bin_of(tiles, r)), &p.w, p.wsum)).collect();
            let off: Vec<f32> = ps.iter().map(|x| x.0).collect();
            let sc: Vec<f32> = ps.iter().map(|x| bsc[x.1 as usize]).collect();
            let mid_ok = |m: usize| {
                let (lon, lat) = bin_point(bin_of(tiles, &run[ps[m].1 as usize]), ps[m].2);
                region.contains(lon, lat)
            };
            best_window(&off, &sc, p.len, &mid_ok).map(|(m, i, j)| (m, ri, i, j))
        })
        .collect();
    cancel.check()?;
    let total = hits.len();
    hits.sort_by(|x, y| y.0.total_cmp(&x.0));
    hits.truncate(p.limit);
    let out = hits
        .into_iter()
        .map(|(m, ri, i, j)| {
            let run = &runs[ri];
            let ps = pseudo(run, tiles);
            let (bf, bm) = (bin_of(tiles, &run[ps[i].1 as usize]), bin_of(tiles, &run[ps[(i + j) / 2].1 as usize]));
            let mut parts = [0f32; NCOMP];
            for x in &ps[i..=j] {
                for (q, v) in parts.iter_mut().zip(comps::<NCOMP>(bin_of(tiles, &run[x.1 as usize]))) {
                    *q += v;
                }
            }
            let n = (j + 1 - i) as f32;
            parts.iter_mut().for_each(|q| *q /= n);
            DriveHit {
                score: m,
                length_m: ps[j].0 - ps[i].0,
                road: run[0].road,
                a: ps[i].0,
                b: ps[j].0,
                // (A road bin's middle sample is on its way.)
                first: (bf.way, deg(bf.lonm, bf.latm)),
                mid: (bm.way, deg(bm.lonm, bm.latm)),
                parts,
                geom: window_geom(run, tiles, &ps, i, j),
            }
        })
        .collect();
    Ok((total, out))
}

pub async fn drives(State(s): State<S>, Query(q): Query<Q>) -> Response {
    run("drives", move |c| compute_drives(&s, q, c)).await
}

fn compute_drives(st: &AppState, q: Q, cancel: &Cancel) -> anyhow::Result<Option<Out>> {
    let Some(region) = Region::parse(&q.bbox, q.poly.as_deref()) else { return Ok(None) };
    let wv: Vec<f32> = q.w.split(',').filter_map(|x| x.parse().ok()).collect();
    if wv.len() != NCOMP {
        return Ok(None);
    }
    let mut w = [0f32; NCOMP];
    w.copy_from_slice(&wv);
    let p = DriveParams {
        len: q.len.unwrap_or(5.0).clamp(0.5, 100.0) * 1000.0,
        w,
        wsum: w.iter().map(|x| x.max(0.0)).sum::<f32>().max(1e-6),
        classes: q.classes.unwrap_or(u32::MAX),
        surface: q.surface.unwrap_or(3),
        toll: q.toll.unwrap_or(3),
        unnamed: q.unnamed.unwrap_or(0),
        lmin: q.lmin,
        lmax: q.lmax,
        limit: q.limit.unwrap_or(20).min(100),
    };
    let margin = p.len as f64 / 2000.0;
    let lt = if q.approx == Some(1) && p.len >= LO_MIN_ROAD { ltiles_for(st, &region, margin, false, cancel)? } else { None };
    let approx = lt.is_some();
    let (total, hits) = match lt {
        Some(lt) => drives_approx(&lt, &region, &p, cancel)?,
        None => drives_exact(&tiles_for(st, &region, margin, cancel)?, &region, &p, cancel)?,
    };
    // Names, from each drive's middle way.
    let fe = FirstErr::default();
    let drives = hits
        .into_iter()
        .filter_map(|h| {
            if cancel.is_set() {
                return None;
            }
            let f = fe.keep(find_way(st, h.mid.0, h.mid.1))?;
            let rec = f.rec();
            let name = f.base.string(rec.name).to_string();
            let name_en = st.road_en(h.mid.0);
            let d = st.names.display(names::Kind::Road, &name, (!name_en.is_empty()).then_some(name_en.as_str()), &[], h.mid.1[0], h.mid.1[1]);
            Some(Drive {
                score: h.score * 100.0,
                length_m: h.length_m,
                way: h.first.0,
                at: h.first.1,
                name,
                main: d.main,
                sub: d.sub,
                name_en,
                r#ref: f.base.string(rec.ref_).to_string(),
                route: f.base.string(rec.route).to_string(),
                class: class::NAMES[rec.class as usize],
                parts: h.parts,
                geom: h.geom,
            })
        })
        .collect();
    fe.check()?;
    cancel.check()?;
    Ok(Some(Out { total, drives, approx }))
}

// ---- rides and rail lines -------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RQ {
    bbox: String,
    poly: Option<String>,
    w: String,
    len: Option<f32>,
    limit: Option<usize>,
    /// Bitmask of the rail service groups shown (bit k = class TRAM + k).
    groups: Option<u8>,
    /// Rail lines: "score", "trains" or "length".
    sort: Option<String>,
    /// 1: from the summaries when every tile has them (zoomed out).
    approx: Option<u8>,
}

fn weights(q: &RQ) -> Option<[f32; RNCOMP]> {
    let v: Vec<f32> = q.w.split(',').filter_map(|x| x.parse().ok()).collect();
    (v.len() == RNCOMP).then(|| {
        let mut w = [0f32; RNCOMP];
        w.copy_from_slice(&v);
        w
    })
}

/// Whether a rail way's services are among the groups shown.
fn shown(class: u8, rail: u8, groups: u8) -> bool {
    (rail & groups) != 0 || (rail == 0 && class >= class::TRAM && (groups >> (class - class::TRAM)) & 1 == 1)
}

/// A rail line's identity: its name without a route's direction ("Highland Sleeper: London Euston
/// => Fort William" → "Highland Sleeper"; a bare "A => B" and its return are one line, "A – B"),
/// else the first service using the track.
pub fn rail_ident(name: &str, route: &str) -> String {
    let raw = if !name.is_empty() { name } else { route.split(" · ").next().unwrap_or("") };
    let n = raw.split(':').next().unwrap_or("").trim();
    if n.contains("=>") {
        let mut ends: Vec<&str> = n.split("=>").map(str::trim).filter(|x| !x.is_empty()).collect();
        ends.sort_unstable();
        ends.dedup();
        ends.join(" – ")
    } else {
        n.to_string()
    }
}

/// A rail run's line details, from one of its ways.
#[derive(Clone)]
pub struct LineInfo {
    pub ident: String,
    pub services: String,
    pub colour: u32,
    pub rel: i64,
    pub way: u64,
    pub rail: u8,
    pub class: u8,
}

/// What the rail queries look up: trains a day, and the line of a sample's way or a bin's.
pub trait Rail: Sync {
    fn freq(&self, way: u64) -> f32;
    fn line(&self, t: &QTile, p: &PSample) -> anyhow::Result<Option<LineInfo>>;
    fn bin_line(&self, t: &LTile, b: &LBin) -> Option<LineInfo>;
}

/// The server's: timetables from global/railfreq, lines from hidata (else base packs).
struct Live<'a> {
    st: &'a AppState,
    freqs: Arc<Vec<(u32, f32)>>,
}

impl Rail for Live<'_> {
    fn freq(&self, way: u64) -> f32 {
        freq_in(&self.freqs, way)
    }

    fn line(&self, t: &QTile, p: &PSample) -> anyhow::Result<Option<LineInfo>> {
        let id = t.here()[p.way as usize].id;
        // From the tile's hidata when it has the lines (no base pack read: cold rides took 7–10 s).
        if t.hv.has_railinfo() {
            let Some((r, name, route)) = t.hv.rail_info(p.way)? else { return Ok(None) };
            return Ok(Some(LineInfo { ident: rail_ident(&name, &route), services: route, colour: r.colour, rel: r.rel, way: id, rail: r.rail, class: r.class }));
        }
        let Some(f) = find_way(self.st, id, deg(p.lon, p.lat))? else { return Ok(None) };
        let r = f.rec();
        Ok(Some(LineInfo {
            ident: rail_ident(f.base.string(r.name), f.base.string(r.route)),
            services: f.base.string(r.route).to_string(),
            colour: r.colour,
            rel: f.base.rail_rel(f.index)?.unwrap_or(0),
            way: id,
            rail: r.rail,
            class: r.class,
        }))
    }

    fn bin_line(&self, t: &LTile, b: &LBin) -> Option<LineInfo> {
        if b.rinfo == NO_RINFO {
            return None;
        }
        let (r, name, route) = t.rail_row(b.rinfo)?;
        Some(LineInfo { ident: rail_ident(name, route), services: route.to_string(), colour: r.colour, rel: r.rel, way: b.way, rail: r.rail, class: r.class })
    }
}

/// The ride components of sample `t` of a run (its neighbours give the gradient), and whether its
/// trains a day are known.
fn ride_comps(r: &dyn Rail, tiles: &[QTile], run: &[Smp], t: usize) -> ([f32; RNCOMP], bool) {
    let (hv, p, c) = sample(tiles, &run[t]);
    let (i0, i1) = (t.saturating_sub(1), (t + 1).min(run.len() - 1));
    let (a, b) = (sample(tiles, &run[i0]).1, sample(tiles, &run[i1]).1);
    let mut k = scenic::ride_components(c, p.eye, p.flags, scenic::grade(a.eye, run[i0].off, b.eye, run[i1].off));
    let (f, known) = scenic::freq_component(r.freq(hv.here()[p.way as usize].id));
    k[FREQ] = f;
    (k, known)
}

/// A rail bin's ride components (trains a day by its way), and whether those are known.
fn bin_ride_comps(r: &dyn Rail, b: &LBin) -> ([f32; RNCOMP], bool) {
    let mut k = comps::<RNCOMP>(b);
    let (f, known) = scenic::freq_component(r.freq(b.way));
    k[FREQ] = f;
    (k, known)
}

/// What a rides query asks for.
pub struct RideParams {
    pub len: f32,
    pub w: [f32; RNCOMP],
    pub groups: u8,
    pub limit: usize,
}

/// A ride found (the best stretch of its line): its window and what the list shows.
pub struct RideHit {
    pub score: f32,
    pub length_m: f32,
    /// The road and the window's offsets along it (what the tests match hits by).
    #[cfg_attr(not(test), allow(dead_code))]
    pub road: u64,
    #[cfg_attr(not(test), allow(dead_code))]
    pub a: f32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub b: f32,
    /// The first way and a point on it.
    pub first: (u64, [f64; 2]),
    /// The line (from the run's first way), and its middle way's and position.
    pub info: LineInfo,
    pub mid_info: Option<LineInfo>,
    pub mid_pos: [f64; 2],
    pub trains: f32,
    pub parts: [f32; RNCOMP],
    pub geom: Vec<[f64; 2]>,
}

/// Each line's best stretch from the tiles' samples: (how many lines have one, the best `limit`).
fn rides_exact(tiles: &[QTile], region: &Region, p: &RideParams, r: &dyn Rail, cancel: &Cancel) -> anyhow::Result<(usize, Vec<RideHit>)> {
    let runs = runs(tiles, true, p.len, &|_, _, _| true);
    cancel.check()?;
    let in_view = |s: &Smp| {
        let (_, ps, _) = sample(tiles, s);
        region.contains(ps.lon, ps.lat)
    };
    let fe = FirstErr::default();
    let mut hits: Vec<(f32, usize, usize, usize, LineInfo)> = runs
        .par_iter()
        .enumerate()
        .filter_map(|(ri, run)| {
            if cancel.is_set() || run.len() < 2 || run[run.len() - 1].off - run[0].off < p.len {
                return None;
            }
            let (t0, p0, _) = sample(tiles, &run[0]);
            let info = fe.keep(r.line(t0, p0))?;
            if !shown(info.class, info.rail, p.groups) {
                return None;
            }
            let sc: Vec<f32> = (0..run.len())
                .map(|t| {
                    let (k, known) = ride_comps(r, tiles, run, t);
                    ride_score_of(&k, known, &p.w)
                })
                .collect();
            let off: Vec<f32> = run.iter().map(|s| s.off).collect();
            best_window(&off, &sc, p.len, &|m| in_view(&run[m])).map(|(m, i, j)| (m, ri, i, j, info))
        })
        .collect();
    fe.check()?;
    cancel.check()?;
    // The best stretch of each line (a line can be several runs, split at junctions).
    hits.sort_by(|x, y| y.0.total_cmp(&x.0));
    let mut seen = std::collections::HashSet::new();
    hits.retain(|h| seen.insert(h.4.ident.clone()));
    let total = hits.len();
    hits.truncate(p.limit);
    let out = hits
        .into_iter()
        .map(|(m, ri, i, j, info)| {
            let run = &runs[ri];
            let mut parts = [0f32; RNCOMP];
            let mut trains = 0f32;
            for t in i..=j {
                let (k, _) = ride_comps(r, tiles, run, t);
                parts.iter_mut().zip(k).for_each(|(q, v)| *q += v);
                let (hv, ps, _) = sample(tiles, &run[t]);
                trains = trains.max(r.freq(hv.here()[ps.way as usize].id));
            }
            let n = (j + 1 - i) as f32;
            parts.iter_mut().for_each(|q| *q /= n);
            let (hv_f, pf, _) = sample(tiles, &run[i]);
            let (hv_m, pm, _) = sample(tiles, &run[(i + j) / 2]);
            RideHit {
                score: m,
                length_m: run[j].off - run[i].off,
                road: run[i].road,
                a: run[i].off,
                b: run[j].off,
                first: (hv_f.here()[pf.way as usize].id, deg(pf.lon, pf.lat)),
                info,
                mid_info: fe.keep(r.line(hv_m, pm)),
                mid_pos: deg(pm.lon, pm.lat),
                trains,
                parts,
                geom: run[i..=j].iter().map(|s| {
                    let q = sample(tiles, s).1;
                    deg(q.lon, q.lat)
                }).collect(),
            }
        })
        .collect();
    fe.check()?;
    Ok((total, out))
}

/// Each line's best stretch from the tiles' summaries, as `rides_exact`.
fn rides_approx(tiles: &[LTile], region: &Region, p: &RideParams, r: &dyn Rail, cancel: &Cancel) -> anyhow::Result<(usize, Vec<RideHit>)> {
    let runs = lruns(tiles, p.len, &|_, _| true);
    cancel.check()?;
    let mut hits: Vec<(f32, usize, usize, usize, LineInfo)> = runs
        .par_iter()
        .enumerate()
        .filter_map(|(ri, run)| {
            if cancel.is_set() || run.iter().map(|b| b.off1).fold(f32::MIN, f32::max) - run[0].off0 < p.len {
                return None;
            }
            let info = r.bin_line(&tiles[run[0].tile as usize], bin_of(tiles, &run[0]))?;
            if !shown(info.class, info.rail, p.groups) {
                return None;
            }
            let ps = pseudo(run, tiles);
            if ps.len() < 2 {
                return None;
            }
            let bsc: Vec<f32> = run
                .iter()
                .map(|x| {
                    let (k, known) = bin_ride_comps(r, bin_of(tiles, x));
                    ride_score_of(&k, known, &p.w)
                })
                .collect();
            let off: Vec<f32> = ps.iter().map(|x| x.0).collect();
            let sc: Vec<f32> = ps.iter().map(|x| bsc[x.1 as usize]).collect();
            let mid_ok = |m: usize| {
                let (lon, lat) = bin_point(bin_of(tiles, &run[ps[m].1 as usize]), ps[m].2);
                region.contains(lon, lat)
            };
            best_window(&off, &sc, p.len, &mid_ok).map(|(m, i, j)| (m, ri, i, j, info))
        })
        .collect();
    cancel.check()?;
    hits.sort_by(|x, y| y.0.total_cmp(&x.0));
    let mut seen = std::collections::HashSet::new();
    hits.retain(|h| seen.insert(h.4.ident.clone()));
    let total = hits.len();
    hits.truncate(p.limit);
    let out = hits
        .into_iter()
        .map(|(m, ri, i, j, info)| {
            let run = &runs[ri];
            let ps = pseudo(run, tiles);
            let mut parts = [0f32; RNCOMP];
            for x in &ps[i..=j] {
                let (k, _) = bin_ride_comps(r, bin_of(tiles, &run[x.1 as usize]));
                parts.iter_mut().zip(k).for_each(|(q, v)| *q += v);
            }
            let n = (j + 1 - i) as f32;
            parts.iter_mut().for_each(|q| *q /= n);
            let mut bins: Vec<u32> = ps[i..=j].iter().map(|x| x.1).collect();
            bins.dedup();
            let trains = bins.iter().map(|&k| r.freq(bin_of(tiles, &run[k as usize]).way)).fold(0f32, f32::max);
            let (bf, bm) = (bin_of(tiles, &run[ps[i].1 as usize]), &run[ps[(i + j) / 2].1 as usize]);
            let bmb = bin_of(tiles, bm);
            RideHit {
                score: m,
                length_m: ps[j].0 - ps[i].0,
                road: run[0].road,
                a: ps[i].0,
                b: ps[j].0,
                // (A rail bin is one way: any of its samples is on it.)
                first: (bf.way, nearest_point(bf, ps[i].0)),
                info,
                mid_info: r.bin_line(&tiles[bm.tile as usize], bmb),
                mid_pos: deg(bmb.lonm, bmb.latm),
                trains,
                parts,
                geom: window_geom(run, tiles, &ps, i, j),
            }
        })
        .collect();
    Ok((total, out))
}

#[derive(Serialize)]
pub struct Ride {
    score: f32,
    length_m: f32,
    way: u64,
    at: [f64; 2],
    rel: i64,
    name: String,
    main: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<String>,
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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    approx: bool,
}

pub async fn rides(State(s): State<S>, Query(q): Query<RQ>) -> Response {
    run("rides", move |c| compute_rides(&s, q, c)).await
}

fn compute_rides(st: &AppState, q: RQ, cancel: &Cancel) -> anyhow::Result<Option<RidesOut>> {
    let Some(region) = Region::parse(&q.bbox, q.poly.as_deref()) else { return Ok(None) };
    let Some(w) = weights(&q) else { return Ok(None) };
    let p = RideParams { len: q.len.unwrap_or(5.0).clamp(0.5, 100.0) * 1000.0, w, groups: q.groups.unwrap_or(0xff), limit: q.limit.unwrap_or(30).min(100) };
    let rail = Live { st, freqs: st.rail_freqs() };
    let margin = p.len as f64 / 2000.0;
    let lt = if q.approx == Some(1) && p.len >= LO_MIN_ROAD { ltiles_for(st, &region, margin, true, cancel)? } else { None };
    let approx = lt.is_some();
    let (total, hits) = match lt {
        Some(lt) => rides_approx(&lt, &region, &p, &rail, cancel)?,
        None => rides_exact(&tiles_for(st, &region, margin, cancel)?, &region, &p, &rail, cancel)?,
    };
    let rides = hits
        .into_iter()
        .map(|h| {
            let d = st.names.display(names::Kind::Other, &h.info.ident, None, &[], h.mid_pos[0], h.mid_pos[1]);
            Ride {
                score: h.score * 100.0,
                length_m: h.length_m,
                way: h.first.0,
                at: h.first.1,
                rel: h.mid_info.as_ref().map_or(h.info.rel, |x| x.rel),
                name: h.info.ident.clone(),
                main: d.main,
                sub: d.sub,
                services: h.mid_info.as_ref().map_or(h.info.services.clone(), |x| x.services.clone()),
                colour: h.mid_info.as_ref().map_or(h.info.colour, |x| x.colour),
                trains: h.trains,
                parts: h.parts,
                geom: h.geom,
            }
        })
        .collect();
    cancel.check()?;
    Ok(Some(RidesOut { total, rides, approx }))
}

/// A rail line's share of the view from one run: its length and score × length in view, its
/// trains a day, where it starts, its stretches in view.
pub struct LineAcc {
    pub len: f32,
    pub sc: f32,
    pub trains: f32,
    pub info: LineInfo,
    pub at: [f64; 2],
    pub geom: Vec<Vec<[f64; 2]>>,
}

/// The runs' shares of the rail lines in view, from the tiles' samples.
fn lines_exact(tiles: &[QTile], region: &Region, w: &[f32; RNCOMP], groups: u8, r: &dyn Rail, cancel: &Cancel) -> anyhow::Result<Vec<LineAcc>> {
    let runs = runs(tiles, true, 0.0, &|_, _, _| true);
    cancel.check()?;
    let fe = FirstErr::default();
    let accs: Vec<LineAcc> = runs
        .par_iter()
        .filter_map(|run| {
            if cancel.is_set() {
                return None;
            }
            let (t0, p0, _) = sample(tiles, &run[0]);
            let info = fe.keep(r.line(t0, p0))?;
            if info.ident.is_empty() || !shown(info.class, info.rail, groups) {
                return None;
            }
            // On the way `info` names (the run's first), so the way APIs find it there.
            let mut acc = LineAcc { len: 0.0, sc: 0.0, trains: 0.0, info, at: deg(p0.lon, p0.lat), geom: Vec::new() };
            let mut cur: Vec<[f64; 2]> = Vec::new();
            for t in 0..run.len() {
                let (hv, p, _) = sample(tiles, &run[t]);
                if !region.contains(p.lon, p.lat) {
                    if cur.len() > 1 {
                        acc.geom.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    continue;
                }
                let step = if t + 1 < run.len() { run[t + 1].off - run[t].off } else { 0.0 }.max(0.0);
                let (k, known) = ride_comps(r, tiles, run, t);
                acc.len += step;
                acc.sc += ride_score_of(&k, known, w) * step;
                acc.trains = acc.trains.max(r.freq(hv.here()[p.way as usize].id));
                cur.push(deg(p.lon, p.lat));
            }
            if cur.len() > 1 {
                acc.geom.push(cur);
            }
            (acc.len > 0.0).then_some(acc)
        })
        .collect();
    fe.check()?;
    cancel.check()?;
    Ok(accs)
}

/// The runs' shares of the rail lines in view, from the tiles' summaries: a bin is in view when
/// its middle sample is, and stands for its samples' steps (to the next bin's first sample; a
/// run's last bin, its own length).
fn lines_approx(tiles: &[LTile], region: &Region, w: &[f32; RNCOMP], groups: u8, r: &dyn Rail, cancel: &Cancel) -> anyhow::Result<Vec<LineAcc>> {
    let runs = lruns(tiles, 0.0, &|_, _| true);
    cancel.check()?;
    let accs: Vec<LineAcc> = runs
        .par_iter()
        .filter_map(|run| {
            if cancel.is_set() {
                return None;
            }
            let b0 = bin_of(tiles, &run[0]);
            let info = r.bin_line(&tiles[run[0].tile as usize], b0)?;
            if info.ident.is_empty() || !shown(info.class, info.rail, groups) {
                return None;
            }
            let mut acc = LineAcc { len: 0.0, sc: 0.0, trains: 0.0, info, at: deg(b0.lon0, b0.lat0), geom: Vec::new() };
            let mut cur: Vec<[f64; 2]> = Vec::new();
            for (bi, x) in run.iter().enumerate() {
                let b = bin_of(tiles, x);
                if !region.contains(b.lonm, b.latm) {
                    if cur.len() > 1 {
                        acc.geom.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    continue;
                }
                let step = if bi + 1 < run.len() { run[bi + 1].off0 - b.off0 } else { b.len }.max(0.0);
                let (k, known) = bin_ride_comps(r, b);
                acc.len += step;
                acc.sc += ride_score_of(&k, known, w) * step;
                acc.trains = acc.trains.max(r.freq(b.way));
                cur.extend(real_points(b).into_iter().map(|p| p.1));
            }
            if cur.len() > 1 {
                acc.geom.push(cur);
            }
            (acc.len > 0.0).then_some(acc)
        })
        .collect();
    cancel.check()?;
    Ok(accs)
}

/// The lines (each run's share summed by line identity), at least 2 km in view, by identity (so
/// lines tied on the sort keep one order).
fn lines_by_ident(accs: Vec<LineAcc>) -> (usize, Vec<LineAcc>) {
    let mut by: std::collections::BTreeMap<String, LineAcc> = std::collections::BTreeMap::new();
    for a in accs {
        match by.get_mut(&a.info.ident) {
            Some(e) => {
                e.len += a.len;
                e.sc += a.sc;
                e.trains = e.trains.max(a.trains);
                e.geom.extend(a.geom);
            }
            None => {
                by.insert(a.info.ident.clone(), a);
            }
        }
    }
    let total = by.len();
    (total, by.into_values().filter(|a| a.len >= 2000.0).collect())
}

#[derive(Serialize)]
pub struct Line {
    name: String,
    main: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<String>,
    services: String,
    colour: u32,
    length_m: f32,
    score: f32,
    trains: f32,
    way: u64,
    at: [f64; 2],
    rel: i64,
    geom: Vec<Vec<[f64; 2]>>,
}

#[derive(Serialize)]
pub struct LinesOut {
    total: usize,
    lines: Vec<Line>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    approx: bool,
}

pub async fn lines(State(s): State<S>, Query(q): Query<RQ>) -> Response {
    run("lines", move |c| compute_lines(&s, q, c)).await
}

fn compute_lines(st: &AppState, q: RQ, cancel: &Cancel) -> anyhow::Result<Option<LinesOut>> {
    let Some(region) = Region::parse(&q.bbox, q.poly.as_deref()) else { return Ok(None) };
    let Some(w) = weights(&q) else { return Ok(None) };
    let groups = q.groups.unwrap_or(0xff);
    let rail = Live { st, freqs: st.rail_freqs() };
    let lt = if q.approx == Some(1) { ltiles_for(st, &region, 0.0, true, cancel)? } else { None };
    let approx = lt.is_some();
    let accs = match lt {
        Some(lt) => lines_approx(&lt, &region, &w, groups, &rail, cancel)?,
        None => lines_exact(&tiles_for(st, &region, 0.0, cancel)?, &region, &w, groups, &rail, cancel)?,
    };
    let (total, accs) = lines_by_ident(accs);
    let mut lines: Vec<Line> = accs
        .into_iter()
        .map(|a| {
            let d = st.names.display(names::Kind::Other, &a.info.ident, None, &[], a.at[0], a.at[1]);
            Line {
                name: a.info.ident.clone(),
                main: d.main,
                sub: d.sub,
                services: a.info.services,
                colour: a.info.colour,
                length_m: a.len,
                score: a.sc / a.len.max(1.0) * 100.0,
                trains: a.trains,
                way: a.info.way,
                at: a.at,
                rel: a.info.rel,
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
    cancel.check()?;
    Ok(Some(LinesOut { total, lines, approx }))
}

#[cfg(test)]
mod approx_check;
