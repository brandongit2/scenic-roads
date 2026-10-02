//! Scenic drives, scenic rides and rail lines in view, from the query parts of the z6 tiles in view
//! (docs/formats.md "Hi data"): each road's ~100 m samples with their offsets along the road. Parts
//! of one road from several tiles join by offset, so a stretch is scored the same whichever tiles
//! it crosses. A drive's midpoint must be in view, so tiles within half the stretch length of the
//! view are read too.
//!
//! Scores as before (`web/src/scenic.ts`, `web/src/rail.ts`): see `components` and `ride_components`.

use crate::views::HiView;
use crate::ways::{find_way, len_ok, tiles_in};
use crate::{AppState, Region, S};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rayon::prelude::*;
use roadcore::packs::{here_extra, PSample};
use roadcore::scenic::{ch, flag, sflag};
use roadcore::{class, E7};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub const NCOMP: usize = 12;
pub const RNCOMP: usize = 11;
const FREQ: usize = 10;
/// Samples further apart than this along a road aren't consecutive (a gap, or the road leaving the
/// tiles read).
const GAP_M: f32 = 300.0;

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

/// One sample of a road, wherever its tile.
#[derive(Clone, Copy)]
struct Smp {
    road: u64,
    off: f32,
    tile: u32,
    k: u32,
}

/// Tiles read for a query: the view's, plus those within `margin_km` of it.
fn tiles_for(s: &AppState, region: &Region, margin_km: f64) -> Vec<Arc<HiView>> {
    let lat = (region.bb[1].unsigned_abs().max(region.bb[3].unsigned_abs()) as f64 * E7).min(85.0);
    let dlat = (margin_km / 111.32 / E7) as i32;
    let dlon = (margin_km / 111.32 / lat.to_radians().cos().max(0.05) / E7) as i32;
    let bb = [region.bb[0].saturating_sub(dlon), region.bb[1].saturating_sub(dlat), region.bb[2].saturating_add(dlon), region.bb[3].saturating_add(dlat)];
    tiles_in(bb).into_par_iter().filter_map(|(x, y)| s.data.hidata(&format!("6/{x}/{y}")).ok().flatten()).collect()
}

/// Every sample of the tiles' parts passing `keep(tile, part sample)`, in road and offset order,
/// cut into runs of consecutive samples.
fn runs(tiles: &[Arc<HiView>], rail: bool, keep: &(dyn Fn(&HiView, &PSample, f32) -> bool + Sync)) -> Vec<Vec<Smp>> {
    let mut all: Vec<Smp> = tiles
        .par_iter()
        .enumerate()
        .flat_map_iter(|(ti, hv)| {
            let ps = hv.psamples();
            let mut out = Vec::new();
            for p in hv.parts() {
                if class::is_rail(p.class) != rail {
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

/// The best window (mean score) at least `len` long whose middle sample is in view:
/// (mean, first index, last index).
fn best_window(run: &[Smp], sc: &[f32], len: f32, mid_in_view: &dyn Fn(&Smp) -> bool) -> Option<(f32, usize, usize)> {
    let mut pre = vec![0f64; sc.len() + 1];
    for (i, v) in sc.iter().enumerate() {
        pre[i + 1] = pre[i] + *v as f64;
    }
    let mut best: Option<(f32, usize, usize)> = None;
    let mut j = 0usize;
    for i in 0..sc.len() {
        j = j.max(i);
        while j + 1 < sc.len() && run[j].off - run[i].off < len {
            j += 1;
        }
        if run[j].off - run[i].off < len {
            break;
        }
        let m = ((pre[j + 1] - pre[i]) / (j + 1 - i) as f64) as f32;
        if mid_in_view(&run[(i + j) / 2]) && best.is_none_or(|b| m > b.0) {
            best = Some((m, i, j));
        }
    }
    best
}

fn sample<'a>(tiles: &'a [Arc<HiView>], s: &Smp) -> (&'a HiView, &'a PSample, &'a [u8; ch::N]) {
    let hv = &*tiles[s.tile as usize];
    (hv, &hv.psamples()[s.k as usize], &hv.pch()[s.k as usize])
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
}

pub async fn drives(State(s): State<S>, Query(q): Query<Q>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || compute_drives(&s2, q)).await {
        Ok(Some(o)) => Json(o).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn compute_drives(st: &AppState, q: Q) -> Option<Out> {
    let region = Region::parse(&q.bbox, q.poly.as_deref())?;
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
    let tiles = tiles_for(st, &region, len as f64 / 2000.0);
    let keep = |hv: &HiView, p: &PSample, road_len: f32| {
        let h = &hv.here()[p.way as usize];
        let unp = (h.flags & roadcore::flag::UNPAVED != 0) as u8;
        let tl = (h.flags & roadcore::flag::TOLL != 0) as u8;
        h.class != class::FERRY
            && (classes >> h.class) & 1 == 1
            && (surface >> unp) & 1 == 1
            && (toll >> tl) & 1 == 1
            && !((unnamed >> h.class) & 1 == 1 && h.extra & here_extra::UNNAMED != 0)
            && len_ok(road_len, q.lmin, q.lmax)
    };
    let runs = runs(&tiles, false, &keep);
    let in_view = |s: &Smp| {
        let (_, p, _) = sample(&tiles, s);
        region.contains(p.lon, p.lat)
    };
    let mut hits: Vec<(f32, usize, usize, usize)> = runs
        .par_iter()
        .enumerate()
        .filter_map(|(ri, run)| {
            if run.len() < 2 || run[run.len() - 1].off - run[0].off < len {
                return None;
            }
            let sc: Vec<f32> = run.iter().map(|s| score(sample(&tiles, s).2, &w, wsum)).collect();
            best_window(run, &sc, len, &in_view).map(|(m, i, j)| (m, ri, i, j))
        })
        .collect();
    let total = hits.len();
    hits.sort_by(|x, y| y.0.total_cmp(&x.0));
    hits.truncate(q.limit.unwrap_or(20).min(100));
    let drives = hits
        .into_iter()
        .filter_map(|(m, ri, i, j)| {
            let run = &runs[ri];
            let (hv_m, pm, _) = sample(&tiles, &run[(i + j) / 2]);
            let (hv_f, pf, _) = sample(&tiles, &run[i]);
            let mid_id = hv_m.here()[pm.way as usize].id;
            let first_id = hv_f.here()[pf.way as usize].id;
            let mid_at = [pm.lon as f64 * E7, pm.lat as f64 * E7];
            let f = find_way(st, mid_id, mid_at)?;
            let rec = f.rec();
            let name = f.base.string(rec.name).to_string();
            let name_en = st.road_en(mid_id);
            let d = st.names.display(names::Kind::Road, &name, (!name_en.is_empty()).then_some(name_en.as_str()), mid_at[0], mid_at[1]);
            let mut parts = [0f32; NCOMP];
            for s in &run[i..=j] {
                for (p, v) in parts.iter_mut().zip(components(sample(&tiles, s).2)) {
                    *p += v;
                }
            }
            let n = (j + 1 - i) as f32;
            parts.iter_mut().for_each(|p| *p /= n);
            Some(Drive {
                score: m * 100.0,
                length_m: run[j].off - run[i].off,
                way: first_id,
                at: [pf.lon as f64 * E7, pf.lat as f64 * E7],
                name,
                main: d.main,
                sub: d.sub,
                name_en,
                r#ref: f.base.string(rec.ref_).to_string(),
                route: f.base.string(rec.route).to_string(),
                class: class::NAMES[rec.class as usize],
                parts,
                geom: run[i..=j].iter().map(|s| {
                    let p = sample(&tiles, s).1;
                    [p.lon as f64 * E7, p.lat as f64 * E7]
                }).collect(),
            })
        })
        .collect();
    Some(Out { total, drives })
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
}

fn weights(q: &RQ) -> Option<[f32; RNCOMP]> {
    let v: Vec<f32> = q.w.split(',').filter_map(|x| x.parse().ok()).collect();
    (v.len() == RNCOMP).then(|| {
        let mut w = [0f32; RNCOMP];
        w.copy_from_slice(&v);
        w
    })
}

/// The ride components of sample `t` of a run (its neighbours give the gradient), and whether its
/// trains a day are known.
fn ride_components(st: &AppState, tiles: &[Arc<HiView>], run: &[Smp], t: usize) -> ([f32; RNCOMP], bool) {
    let (hv, p, c) = sample(tiles, &run[t]);
    let (i0, i1) = (t.saturating_sub(1), (t + 1).min(run.len() - 1));
    let (a, b) = (sample(tiles, &run[i0]).1, sample(tiles, &run[i1]).1);
    let dd = (run[i1].off - run[i0].off).abs().max(1.0);
    let grade = ((b.eye - a.eye).abs() / dd * 100.0).min(30.0);
    let f = st.rail_freq(hv.here()[p.way as usize].id);
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

fn ride_score(st: &AppState, tiles: &[Arc<HiView>], run: &[Smp], t: usize, w: &[f32; RNCOMP]) -> f32 {
    let (c, known) = ride_components(st, tiles, run, t);
    let pos: f32 = w.iter().enumerate().map(|(i, v)| if i == FREQ && !known { 0.0 } else { v.max(0.0) }).sum::<f32>().max(1e-6);
    (c.iter().zip(w).map(|(a, b)| a * b).sum::<f32>() / pos).clamp(0.0, 1.0)
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

/// A rail run's line details, from the way of sample `t`.
struct LineInfo {
    ident: String,
    services: String,
    colour: u32,
    rel: i64,
    way: u64,
    rail: u8,
    class: u8,
}

fn line_info(st: &AppState, tiles: &[Arc<HiView>], s: &Smp) -> Option<LineInfo> {
    let (hv, p, _) = sample(tiles, s);
    let id = hv.here()[p.way as usize].id;
    let f = find_way(st, id, [p.lon as f64 * E7, p.lat as f64 * E7])?;
    let r = f.rec();
    Some(LineInfo {
        ident: rail_ident(f.base.string(r.name), f.base.string(r.route)),
        services: f.base.string(r.route).to_string(),
        colour: r.colour,
        rel: f.base.rail_rel(f.index).unwrap_or(0),
        way: id,
        rail: r.rail,
        class: r.class,
    })
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
}

pub async fn rides(State(s): State<S>, Query(q): Query<RQ>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || compute_rides(&s2, q)).await {
        Ok(Some(o)) => Json(o).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn compute_rides(st: &AppState, q: RQ) -> Option<RidesOut> {
    let region = Region::parse(&q.bbox, q.poly.as_deref())?;
    let w = weights(&q)?;
    let len = q.len.unwrap_or(5.0).clamp(0.5, 100.0) * 1000.0;
    let groups = q.groups.unwrap_or(0xff);
    let tiles = tiles_for(st, &region, len as f64 / 2000.0);
    let runs = runs(&tiles, true, &|_, _, _| true);
    let in_view = |s: &Smp| {
        let (_, p, _) = sample(&tiles, s);
        region.contains(p.lon, p.lat)
    };
    let mut hits: Vec<(f32, usize, usize, usize, LineInfo)> = runs
        .par_iter()
        .enumerate()
        .filter_map(|(ri, run)| {
            if run.len() < 2 || run[run.len() - 1].off - run[0].off < len {
                return None;
            }
            let info = line_info(st, &tiles, &run[0])?;
            if !shown(info.class, info.rail, groups) {
                return None;
            }
            let sc: Vec<f32> = (0..run.len()).map(|t| ride_score(st, &tiles, run, t, &w)).collect();
            best_window(run, &sc, len, &in_view).map(|(m, i, j)| (m, ri, i, j, info))
        })
        .collect();
    // The best stretch of each line (a line can be several runs, split at junctions).
    hits.sort_by(|x, y| y.0.total_cmp(&x.0));
    let mut seen = std::collections::HashSet::new();
    hits.retain(|h| seen.insert(h.4.ident.clone()));
    let total = hits.len();
    hits.truncate(q.limit.unwrap_or(30).min(100));
    let rides = hits
        .into_iter()
        .map(|(m, ri, i, j, info)| {
            let run = &runs[ri];
            let mut parts = [0f32; RNCOMP];
            let mut trains = 0f32;
            for t in i..=j {
                let (c, _) = ride_components(st, &tiles, run, t);
                parts.iter_mut().zip(c).for_each(|(p, v)| *p += v);
                let (hv, p, _) = sample(&tiles, &run[t]);
                trains = trains.max(st.rail_freq(hv.here()[p.way as usize].id));
            }
            let n = (j + 1 - i) as f32;
            parts.iter_mut().for_each(|p| *p /= n);
            let (hv_f, pf, _) = sample(&tiles, &run[i]);
            let mid = sample(&tiles, &run[(i + j) / 2]).1;
            let mid_info = line_info(st, &tiles, &run[(i + j) / 2]);
            let d = st.names.display(names::Kind::Place, &info.ident, None, mid.lon as f64 * E7, mid.lat as f64 * E7);
            Ride {
                score: m * 100.0,
                length_m: run[j].off - run[i].off,
                way: hv_f.here()[pf.way as usize].id,
                at: [pf.lon as f64 * E7, pf.lat as f64 * E7],
                rel: mid_info.as_ref().map_or(info.rel, |x| x.rel),
                name: info.ident.clone(),
                main: d.main,
                sub: d.sub,
                services: mid_info.as_ref().map_or(info.services.clone(), |x| x.services.clone()),
                colour: mid_info.as_ref().map_or(info.colour, |x| x.colour),
                trains,
                parts,
                geom: run[i..=j].iter().map(|s| {
                    let p = sample(&tiles, s).1;
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
}

pub async fn lines(State(s): State<S>, Query(q): Query<RQ>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || compute_lines(&s2, q)).await {
        Ok(Some(o)) => Json(o).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn compute_lines(st: &AppState, q: RQ) -> Option<LinesOut> {
    let region = Region::parse(&q.bbox, q.poly.as_deref())?;
    let w = weights(&q)?;
    let groups = q.groups.unwrap_or(0xff);
    let tiles = tiles_for(st, &region, 0.0);
    let runs = runs(&tiles, true, &|_, _, _| true);
    struct Acc {
        len: f32,
        sc: f32,
        trains: f32,
        info: LineInfo,
        at: [f64; 2],
        geom: Vec<Vec<[f64; 2]>>,
    }
    let parts: Vec<Acc> = runs
        .par_iter()
        .filter_map(|run| {
            let info = line_info(st, &tiles, &run[0])?;
            if info.ident.is_empty() || !shown(info.class, info.rail, groups) {
                return None;
            }
            // On the way `info` names (the run's first), so the way APIs find it there.
            let p0 = sample(&tiles, &run[0]).1;
            let mut acc = Acc { len: 0.0, sc: 0.0, trains: 0.0, info, at: [p0.lon as f64 * E7, p0.lat as f64 * E7], geom: Vec::new() };
            let mut cur: Vec<[f64; 2]> = Vec::new();
            for t in 0..run.len() {
                let (hv, p, _) = sample(&tiles, &run[t]);
                if !region.contains(p.lon, p.lat) {
                    if cur.len() > 1 {
                        acc.geom.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    continue;
                }
                let step = if t + 1 < run.len() { run[t + 1].off - run[t].off } else { 0.0 }.max(0.0);
                acc.len += step;
                acc.sc += ride_score(st, &tiles, run, t, &w) * step;
                acc.trains = acc.trains.max(st.rail_freq(hv.here()[p.way as usize].id));
                cur.push([p.lon as f64 * E7, p.lat as f64 * E7]);
            }
            if cur.len() > 1 {
                acc.geom.push(cur);
            }
            (acc.len > 0.0).then_some(acc)
        })
        .collect();
    let mut by: HashMap<String, Acc> = HashMap::new();
    for a in parts {
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
    let mut lines: Vec<Line> = by
        .into_values()
        .filter(|a| a.len >= 2000.0)
        .map(|a| {
            let d = st.names.display(names::Kind::Place, &a.info.ident, None, a.at[0], a.at[1]);
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
    Some(LinesOut { total, lines })
}
