//! Ways and whole roads: hover info, the road highlight, elevation profiles, climbs.
//!
//! A way is asked for by its OSM id plus a point near it (`?at=lon,lat`): the z6 tile holding the
//! point says which hidata lists it ("ways here" → its owner unit and index), and the owner's base
//! pack has its record and per-vertex data. A whole road is the ways with the same road id (the one
//! chaining, done when the data was built), found through the road → units index and put in order
//! by their offsets along the road.

use crate::views::{BaseView, HiView};
use crate::{AppState, Region, S};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use roadcore::packs::Here;
use roadcore::{class, dist_m, flag, DemSource, WayRec, E7, NDEM};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// `?at=lon,lat`.
#[derive(Deserialize)]
pub struct At {
    at: Option<String>,
}

impl At {
    pub fn point(&self) -> Option<[f64; 2]> {
        let v: Vec<f64> = self.at.as_deref()?.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (v.len() == 2 && v[0].is_finite() && v[1].is_finite()).then(|| [v[0], v[1]])
    }
}

/// The z6 tile ("6/x/y") of a point.
pub fn tile6(lon: f64, lat: f64) -> (u32, u32) {
    let (x, y) = roadcore::merc(lon, lat);
    let n = 64.0;
    (((x * n).floor().clamp(0.0, 63.0)) as u32, ((y * n).floor().clamp(0.0, 63.0)) as u32)
}

/// A way found: its owner's base pack and its index there.
pub struct Found {
    pub base: Arc<BaseView>,
    pub index: u32,
}

impl Found {
    pub fn rec(&self) -> &WayRec {
        &self.base.ways()[self.index as usize]
    }
}

/// The tile key of a unit string ("6/32/21").
fn unit_key(u: &str) -> Option<u64> {
    let mut it = u.split('/').map(|t| t.parse::<u32>().ok());
    let (z, x, y) = (it.next()??, it.next()??, it.next()??);
    Some(roadcore::archive::tile_key(z as u8, x, y))
}

/// "6/32/21" of a unit tile key.
pub fn unit_str(key: u64) -> String {
    format!("{}/{}/{}", key >> 58, (key >> 29) & ((1 << 29) - 1), key & ((1 << 29) - 1))
}

/// The `here` entry of way `id` in the tile of `at` or one next to it.
pub fn find_here(s: &AppState, id: u64, at: [f64; 2]) -> Option<(Arc<HiView>, Here)> {
    let (tx, ty) = tile6(at[0], at[1]);
    let mut tiles = vec![(tx, ty)];
    for dx in -1i64..=1 {
        for dy in -1i64..=1 {
            if dx != 0 || dy != 0 {
                tiles.push((((tx as i64 + dx).rem_euclid(64)) as u32, (ty as i64 + dy).clamp(0, 63) as u32));
            }
        }
    }
    for (x, y) in tiles {
        if let Ok(Some(hv)) = s.data.hidata(&format!("6/{x}/{y}")) {
            if let Some(h) = hv.find(id) {
                let h = *h;
                return Some((hv, h));
            }
        }
    }
    None
}

/// A way by OSM id and a point near it. The "ways here" entry must name a way of that id in its
/// owner's base pack (a mismatch would mean packs of different catalogs).
pub fn find_way(s: &AppState, id: u64, at: [f64; 2]) -> Option<Found> {
    let (_, h) = find_here(s, id, at)?;
    let base = s.data.base(&unit_str(h.owner)).ok()??;
    let w = base.ways().get(h.index as usize)?;
    if w.id as u64 != id {
        eprintln!("way {id}: the ways-here index points at way {} in {}", w.id, unit_str(h.owner));
        return None;
    }
    Some(Found { base, index: h.index })
}

#[derive(Serialize)]
pub struct WayInfo {
    /// The OSM way id (also `osm_id`; kept for the app's older field name).
    idx: u64,
    osm_id: i64,
    class: &'static str,
    name: String,
    /// Display name: the label (`main`) and its second line (`sub`), see names.
    main: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<String>,
    /// OSM's own English name (name:en), when it has one that isn't just the name.
    #[serde(skip_serializing_if = "String::is_empty")]
    name_en: String,
    r#ref: String,
    surface: String,
    maxspeed: u16,
    lanes: u8,
    link: bool,
    bridge: bool,
    tunnel: bool,
    unpaved: bool,
    oneway: bool,
    toll: bool,
    covered: bool,
    route: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rail: Vec<&'static str>,
    #[serde(skip_serializing_if = "String::is_empty")]
    colour: String,
    length_m: f64,
    elev_min: f32,
    elev_max: f32,
    sources: Vec<(String, f32)>,
    /// The way's road: its id and whole length (the one chaining).
    road: u64,
    road_m: f32,
}

pub fn way_info(s: &AppState, f: &Found) -> WayInfo {
    let w = f.rec();
    let bp = &f.base;
    let r = bp.range(w);
    let v = &bp.verts()[r.clone()];
    let e = &bp.elev()[r.clone()];
    let len: f64 = v.windows(2).map(|p| dist_m(p[0][0] as f64 * E7, p[0][1] as f64 * E7, p[1][0] as f64 * E7, p[1][1] as f64 * E7)).sum();
    let mut counts = [0u32; NDEM];
    for &c in &bp.src()[r] {
        counts[(c as usize).min(NDEM - 1)] += 1;
    }
    let n = w.vcount.max(1) as f32;
    let sources = (1..NDEM).filter(|&k| counts[k] > 0).map(|k| (DemSource::label(k as u8).to_string(), counts[k] as f32 / n)).collect();
    let name = bp.string(w.name).to_string();
    let name_en = s.road_en(w.id as u64);
    let mid = v[v.len() / 2];
    // Rail lines' names are in the places tables, roads' in the roads tables.
    let kind = if class::is_rail(w.class) { names::Kind::Place } else { names::Kind::Road };
    let d = s.names.display(kind, &name, (!name_en.is_empty()).then_some(name_en.as_str()), mid[0] as f64 * E7, mid[1] as f64 * E7);
    let rv = bp.road_vals()[f.index as usize];
    WayInfo {
        idx: w.id as u64,
        osm_id: w.id,
        class: class::NAMES[w.class as usize],
        name,
        main: d.main,
        sub: d.sub,
        name_en,
        r#ref: bp.string(w.ref_).to_string(),
        surface: bp.string(w.surface).to_string(),
        maxspeed: w.maxspeed,
        lanes: w.lanes,
        link: w.flags & flag::LINK != 0,
        bridge: w.flags & flag::BRIDGE != 0,
        tunnel: w.flags & flag::TUNNEL != 0,
        unpaved: w.flags & flag::UNPAVED != 0,
        oneway: w.flags & flag::ONEWAY != 0,
        toll: w.flags & flag::TOLL != 0,
        covered: w.flags & flag::COVERED != 0,
        route: bp.string(w.route).to_string(),
        rail: (0..5).filter(|k| w.rail >> k & 1 == 1).map(|k| class::NAMES[class::TRAM as usize + k]).collect(),
        colour: if w.colour != 0 { format!("#{:06x}", w.colour & 0xff_ffff) } else { String::new() },
        length_m: len,
        elev_min: e.iter().copied().min().unwrap_or(0) as f32 / 10.0,
        elev_max: e.iter().copied().max().unwrap_or(0) as f32 / 10.0,
        sources,
        road: rv.road,
        road_m: rv.len,
    }
}

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

pub async fn way_h(State(s): State<S>, Path(id): Path<u64>, Query(at): Query<At>) -> Response {
    let Some(p) = at.point() else { return (StatusCode::BAD_REQUEST, "at=lon,lat").into_response() };
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || find_way(&s2, id, p).map(|f| way_info(&s2, &f))).await {
        Ok(Some(w)) => ([(header::CACHE_CONTROL, "no-cache")], Json(w)).into_response(),
        _ => not_found(),
    }
}

/// One way of a road, oriented along it.
pub struct RoadWay {
    pub base: Arc<BaseView>,
    pub index: u32,
    pub offset: f32,
    /// Traversed against its own direction.
    pub rev: bool,
}

/// The ways of the road through `f` whose offsets lie within `window` metres of `f`'s, in order
/// along the road (each unit's ways joined by offset). The second value: whether the window cut
/// the road short.
/// An error when a unit the road crosses couldn't be read (the NAS away): a road with gaps would
/// be cached as if whole.
pub fn road_ways(s: &AppState, f: &Found, window: f32) -> anyhow::Result<(Vec<RoadWay>, bool)> {
    let rv = f.base.road_vals()[f.index as usize];
    let (lo, hi) = (rv.offset - window, rv.offset + window);
    let units: Vec<u64> = match s.data.roadunits()? {
        Some(ru) => ru.units(rv.road),
        None => vec![unit_key(&f.base.unit).unwrap_or(0)],
    };
    let mut out: Vec<RoadWay> = Vec::new();
    let mut truncated = false;
    for u in units {
        let Some(bp) = s.data.base(&unit_str(u))? else { continue };
        for &i in bp.on_road(rv.road) {
            let r = bp.road_vals()[i as usize];
            if r.offset < lo || r.offset > hi {
                truncated = true;
                continue;
            }
            out.push(RoadWay { base: bp.clone(), index: i, offset: r.offset, rev: r.dir == 1 });
        }
    }
    out.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    Ok((out, truncated))
}

const WINDOW_M: f32 = 400_000.0;

/// OSM ids of the whole road through a way (for the hover highlight).
pub async fn road_h(State(s): State<S>, Path(id): Path<u64>, Query(at): Query<At>) -> Response {
    let Some(p) = at.point() else { return (StatusCode::BAD_REQUEST, "at=lon,lat").into_response() };
    let s2 = s.clone();
    let got = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<u64>>> {
        let Some(f) = find_way(&s2, id, p) else { return Ok(None) };
        let (ways, _) = road_ways(&s2, &f, WINDOW_M)?;
        Ok(Some(ways.iter().map(|w| w.base.ways()[w.index as usize].id as u64).collect()))
    })
    .await;
    match got {
        Ok(Ok(Some(ids))) => ([(header::CACHE_CONTROL, "no-cache")], Json(ids)).into_response(),
        Ok(Err(e)) => {
            eprintln!("road {id}: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        _ => not_found(),
    }
}

#[derive(Serialize)]
pub struct Profile {
    way: WayInfo,
    ways: Vec<u64>,
    coords: Vec<[f64; 2]>,
    dist: Vec<f32>,
    elev: Vec<f32>,
    grade: Vec<f32>,
    length_m: f64,
    elev_min: f32,
    elev_max: f32,
    climb_m: f64,
    descent_m: f64,
    max_grade: f32,
    avg_grade: f32,
    sources: Vec<(String, f64)>,
    truncated: bool,
    ch: Vec<[u8; roadcore::scenic::ch::N]>,
}

pub async fn profile_h(State(s): State<S>, Path(id): Path<u64>, Query(at): Query<At>) -> Response {
    let Some(p) = at.point() else { return (StatusCode::BAD_REQUEST, "at=lon,lat").into_response() };
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || build_profile(&s2, id, p)).await {
        Ok(Ok(Some(p))) => Json(p).into_response(),
        Ok(Err(e)) => {
            eprintln!("profile {id}: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        _ => not_found(),
    }
}

/// One vertex of the road in order: its base pack and index.
struct V<'a> {
    bp: &'a BaseView,
    i: usize,
}

fn build_profile(s: &AppState, id: u64, at: [f64; 2]) -> anyhow::Result<Option<Profile>> {
    let Some(f) = find_way(s, id, at) else { return Ok(None) };
    let info = way_info(s, &f);
    let (ways, truncated) = road_ways(s, &f, WINDOW_M)?;
    // Concatenate vertices along the road (a way's first vertex repeats the previous way's last).
    let mut vs: Vec<V> = Vec::new();
    for rw in &ways {
        let bp = &*rw.base;
        let w = &bp.ways()[rw.index as usize];
        let r = bp.range(w);
        let idx: Vec<usize> = if rw.rev { r.rev().collect() } else { r.collect() };
        let skip = match (vs.last(), idx.first()) {
            (Some(l), Some(&f0)) if l.bp.verts()[l.i] == bp.verts()[f0] => 1,
            _ => 0,
        };
        vs.extend(idx[skip..].iter().map(|&i| V { bp, i }));
    }
    if vs.len() < 2 {
        return Ok(None);
    }
    let pt = |v: &V| v.bp.verts()[v.i];
    let el = |v: &V| v.bp.elev()[v.i];
    let gr = |v: &V| v.bp.grade()[v.i];
    let src = |v: &V| v.bp.src()[v.i];
    let mut dist = Vec::with_capacity(vs.len());
    let mut acc = 0f64;
    let (mut climb, mut descent) = (0f64, 0f64);
    let mut src_len = [0f64; NDEM];
    dist.push(0.0);
    for k in 1..vs.len() {
        let (a, b) = (pt(&vs[k - 1]), pt(&vs[k]));
        let d = dist_m(a[0] as f64 * E7, a[1] as f64 * E7, b[0] as f64 * E7, b[1] as f64 * E7);
        acc += d;
        dist.push(acc);
        let de = (el(&vs[k]) - el(&vs[k - 1])) as f64 / 10.0;
        if de > 0.0 {
            climb += de
        } else {
            descent -= de
        }
        src_len[(src(&vs[k]) as usize).min(NDEM - 1)] += d;
    }
    let (mut emin, mut emax, mut gmax) = (i16::MAX, i16::MIN, 0u8);
    for v in &vs {
        emin = emin.min(el(v));
        emax = emax.max(el(v));
        gmax = gmax.max(gr(v));
    }
    // Downsample for transport: ~1 vertex per 10 m, plus local extrema.
    let step = (acc / 20000.0).max(10.0);
    let mut keep = Vec::new();
    let mut next = 0.0;
    for k in 0..vs.len() {
        let is_ext = k > 0 && k + 1 < vs.len() && {
            let (p, c, n) = (el(&vs[k - 1]), el(&vs[k]), el(&vs[k + 1]));
            (c > p && c >= n) || (c < p && c <= n)
        };
        if dist[k] >= next || k + 1 == vs.len() || (is_ext && step > 10.0 && (dist[k] - dist[*keep.last().unwrap_or(&0)]) > 2.0) {
            keep.push(k);
            next = dist[k] + step;
        }
    }
    let total = acc.max(1e-9);
    let has_ch = vs.iter().all(|v| v.bp.scenic().is_some());
    Ok(Some(Profile {
        way: info,
        ways: ways.iter().map(|w| w.base.ways()[w.index as usize].id as u64).collect(),
        coords: keep.iter().map(|&k| {
            let p = pt(&vs[k]);
            [p[0] as f64 * E7, p[1] as f64 * E7]
        }).collect(),
        dist: keep.iter().map(|&k| dist[k] as f32).collect(),
        elev: keep.iter().map(|&k| el(&vs[k]) as f32 / 10.0).collect(),
        grade: keep.iter().map(|&k| gr(&vs[k]) as f32 / 2.0).collect(),
        length_m: acc,
        elev_min: emin as f32 / 10.0,
        elev_max: emax as f32 / 10.0,
        climb_m: climb,
        descent_m: descent,
        max_grade: gmax as f32 / 2.0,
        avg_grade: ((climb + descent) / total * 100.0) as f32,
        sources: (1..NDEM).filter(|&k| src_len[k] > 0.0).map(|k| (DemSource::label(k as u8).to_string(), src_len[k] / total)).collect(),
        truncated,
        ch: if has_ch { keep.iter().map(|&k| vs[k].bp.scenic().unwrap()[vs[k].i]).collect() } else { Vec::new() },
    }))
}

// ---- climbs -------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ClimbQuery {
    bbox: String,
    #[serde(default)]
    poly: Option<String>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    classes: Option<u32>,
    #[serde(default)]
    surface: Option<u8>,
    #[serde(default)]
    toll: Option<u8>,
    #[serde(default)]
    unnamed: Option<u32>,
    #[serde(default)]
    lmin: Option<f32>,
    #[serde(default)]
    lmax: Option<f32>,
}

/// The road-length filter: [lmin, lmax] metres, 0 = no limit (tiles carry whole metres).
pub fn len_ok(road_len: f32, lmin: Option<f32>, lmax: Option<f32>) -> bool {
    let (lo, hi) = (lmin.unwrap_or(0.0), lmax.filter(|&v| v > 0.0).unwrap_or(f32::INFINITY));
    let r = road_len.round();
    r >= lo && r <= hi
}

#[derive(Serialize)]
struct ClimbOut {
    way: u64,
    /// A point on the climb's first way (for `?at=`).
    at: [f64; 2],
    name: String,
    main: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    name_en: String,
    r#ref: String,
    class: &'static str,
    gain_m: f32,
    length_m: f32,
    avg_grade: f32,
    max_grade: f32,
    start_elev: f32,
    top_elev: f32,
    unpaved: bool,
    geom: Vec<[f64; 2]>,
}

#[derive(Serialize)]
struct ClimbList {
    total: usize,
    climbs: Vec<ClimbOut>,
}

/// The z6 tiles a region's box touches.
pub fn tiles_in(bb: [i32; 4]) -> Vec<(u32, u32)> {
    let (x0, y0) = tile6(bb[0] as f64 * E7, bb[3] as f64 * E7);
    let (x1, y1) = tile6(bb[2] as f64 * E7, bb[1] as f64 * E7);
    let mut out = Vec::new();
    for x in x0..=x1 {
        for y in y0..=y1 {
            out.push((x, y));
        }
    }
    out
}

pub async fn climbs_h(State(s): State<S>, Query(q): Query<ClimbQuery>) -> Response {
    let Some(region) = Region::parse(&q.bbox, q.poly.as_deref()) else { return StatusCode::BAD_REQUEST.into_response() };
    let s2 = s.clone();
    let out = tokio::task::spawn_blocking(move || climbs(&s2, &q, &region)).await;
    match out {
        Ok(l) => Json(l).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn climbs(s: &AppState, q: &ClimbQuery, region: &Region) -> ClimbList {
    let classes = q.classes.unwrap_or(u32::MAX);
    let surface = q.surface.unwrap_or(3);
    let toll = q.toll.unwrap_or(3);
    let unnamed = q.unnamed.unwrap_or(0);
    let mut hits: Vec<(f32, Arc<HiView>, usize)> = Vec::new();
    for (x, y) in tiles_in(region.bb) {
        let Ok(Some(hv)) = s.data.hidata(&format!("6/{x}/{y}")) else { continue };
        for (i, c) in hv.climbs().iter().enumerate() {
            let ok = region.contains(c.mid[0], c.mid[1])
                && (classes >> c.class) & 1 == 1
                && (surface >> (c.unpaved & 1)) & 1 == 1
                && (toll >> (c.flags & 1)) & 1 == 1
                && !((unnamed >> c.class) & 1 == 1 && c.flags & 2 != 0)
                && len_ok(c.road_len, q.lmin, q.lmax);
            if !ok {
                continue;
            }
            let avg = c.gain_m / c.length_m.max(1.0);
            let key = match q.sort.as_deref() {
                Some("grade") => avg,
                Some("score") => c.gain_m * c.gain_m / c.length_m.max(1.0),
                _ => c.gain_m,
            };
            hits.push((key, hv.clone(), i));
        }
    }
    let total = hits.len();
    let limit = q.limit.unwrap_or(25).min(100);
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    hits.truncate(limit);
    let climbs = hits
        .into_iter()
        .map(|(_, hv, i)| {
            let c = hv.climbs()[i];
            let geom: Vec<[f64; 2]> = hv.climbgeom()[c.geom_start as usize..(c.geom_start + c.geom_count) as usize].iter().map(|p| [p[0] as f64 * E7, p[1] as f64 * E7]).collect();
            let mid = [c.mid[0] as f64 * E7, c.mid[1] as f64 * E7];
            let (name, rf) = find_way(s, c.label, mid)
                .map(|f| (f.base.string(f.rec().name).to_string(), f.base.string(f.rec().ref_).to_string()))
                .unwrap_or_default();
            let name_en = s.road_en(c.label);
            let d = s.names.display(names::Kind::Road, &name, (!name_en.is_empty()).then_some(name_en.as_str()), mid[0], mid[1]);
            ClimbOut {
                way: c.way,
                at: geom.first().copied().unwrap_or(mid),
                name,
                main: d.main,
                sub: d.sub,
                name_en,
                r#ref: rf,
                class: class::NAMES[c.class as usize],
                gain_m: c.gain_m,
                length_m: c.length_m,
                avg_grade: c.gain_m / c.length_m.max(1.0) * 100.0,
                max_grade: c.max_grade,
                start_elev: c.start_elev,
                top_elev: c.top_elev,
                unpaved: c.unpaved != 0,
                geom,
            }
        })
        .collect();
    ClimbList { total, climbs }
}
