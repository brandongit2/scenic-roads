//! Regions and the areas they're made of (docs/plan.md §1, the Regions panel; §5).
//!
//!   GET    /api/regions             the recipes in inputs/regions on the NAS, and the ones that
//!                                   don't parse
//!   POST   /api/regions             {id, name, outline}: a new region (created exclusively: 409 when
//!                                   the id is taken)
//!   PUT    /api/regions/{id}        {name, outline}: rename or redraw
//!   DELETE /api/regions/{id}        removed (its recipe kept as .removed)
//!   GET    /api/areas?at=lon,lat    the outlines containing a point, smallest first
//!   GET    /api/areas/search?q=     outlines by name, largest first
//!   GET    /api/areas/{id}          one outline, simplified (a GeoJSON feature)
//!   GET    /api/coverage            every region's outlines, simplified (GeoJSON)
//!
//! Outlines come from the latest OSM pass (the catalog's `global.outlines`); records and names are
//! read once per catalog, rings when an outline is drawn or tested.

use crate::S;
use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use pipeline::agent::recipes::{self, Recipe};
use pipeline::outlines::{flag, inside, OutlineRec, Ring};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The outlines of the current catalog: records and names in memory, rings read on demand.
pub struct OutlineIndex {
    content: String,
    sect: Arc<crate::views::SectView>,
    pub recs: Vec<OutlineRec>,
    strings: Vec<String>,
    /// Folded names (lower case, no accents) with record indexes, sorted, for search.
    names: Vec<(String, u32)>,
}

fn fold(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfkd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

impl OutlineIndex {
    fn open(s: &crate::AppState) -> Result<Option<OutlineIndex>> {
        let cat = s.data.catalog();
        let Some(logical) = cat.global.get("outlines") else { return Ok(None) };
        let Some(sect) = s.data.sect(logical)? else { return Ok(None) };
        let content = s.data.content(logical).unwrap_or_default();
        let recs: Vec<OutlineRec> = bytemuck::pod_collect_to_vec(sect.get("recs")?.bytes());
        let strings: Vec<String> = String::from_utf8_lossy(sect.get("strings")?.bytes()).split('\n').map(str::to_string).collect();
        let mut names: Vec<(String, u32)> = Vec::with_capacity(recs.len() * 2);
        for (i, r) in recs.iter().enumerate() {
            for k in [r.name, r.name_en] {
                if let Some(n) = strings.get(k as usize).filter(|n| !n.is_empty()) {
                    names.push((fold(n), i as u32));
                }
            }
        }
        names.sort();
        names.dedup();
        Ok(Some(OutlineIndex { content, sect, recs, strings, names }))
    }

    pub fn string(&self, i: u32) -> &str {
        self.strings.get(i as usize).map(String::as_str).unwrap_or("")
    }

    /// An outline's rings (`simple`: the simplified ones), read from the file.
    pub fn rings(&self, o: &OutlineRec, simple: bool) -> Result<Vec<Vec<[i32; 2]>>> {
        let (rs, ps, first, n) = if simple { ("srings", "spoints", o.srings, o.nsrings) } else { ("rings", "points", o.rings, o.nrings) };
        let rb = self.sect.get_part(rs, first as u64 * 16, n as usize * 16)?;
        let rings: Vec<Ring> = bytemuck::pod_collect_to_vec(&rb);
        let (Some(a), Some(b)) = (rings.first(), rings.last()) else { return Ok(Vec::new()) };
        let (start, end) = (a.start, b.start + b.count as u64);
        let pb = self.sect.get_part(ps, start * 8, ((end - start) * 8) as usize)?;
        let pts: Vec<[i32; 2]> = bytemuck::pod_collect_to_vec(&pb);
        Ok(rings.iter().map(|r| pts[(r.start - start) as usize..(r.start - start) as usize + r.count as usize].to_vec()).collect())
    }

    pub fn by_id(&self, id: u64) -> Option<&OutlineRec> {
        self.recs.binary_search_by_key(&id, |r| r.id).ok().map(|k| &self.recs[k])
    }

    fn summary(&self, o: &OutlineRec) -> Value {
        json!({
            "id": o.id,
            "name": self.string(o.name),
            "en": self.string(o.name_en),
            "level": o.level,
            "iso": self.string(o.iso),
            "country": o.flags & flag::ISO1 != 0,
            "km2": o.area_km2.round(),
            "bbox": o.bbox.map(|v| v as f64 * 1e-7),
        })
    }
}

/// The outline index of the current catalog, opened once per catalog.
#[derive(Default)]
pub struct Areas {
    cur: Mutex<Option<Arc<OutlineIndex>>>,
}

impl Areas {
    pub fn get(&self, s: &crate::AppState) -> Result<Option<Arc<OutlineIndex>>> {
        let cat = s.data.catalog();
        let want = cat.global.get("outlines").and_then(|l| s.data.content(l));
        let mut g = self.cur.lock().unwrap();
        if let Some(cur) = g.as_ref() {
            if Some(&cur.content) == want.as_ref() {
                return Ok(Some(cur.clone()));
            }
        }
        let opened = OutlineIndex::open(s)?.map(Arc::new);
        *g = opened.clone();
        Ok(opened)
    }
}

fn err(code: StatusCode, msg: impl std::fmt::Display) -> Response {
    (code, Json(json!({ "error": msg.to_string() }))).into_response()
}

/// The recipes folder on the NAS (None while it's away).
fn regions_dir(s: &crate::AppState) -> Option<std::path::PathBuf> {
    s.data.online().then(|| s.data.nas_root()).flatten().map(|r| r.join("inputs/regions"))
}

pub async fn list(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let Some(dir) = regions_dir(&s2) else { return err(StatusCode::SERVICE_UNAVAILABLE, "the NAS isn't reachable") };
        let (ok, bad) = recipes::load(&dir);
        Json(json!({ "regions": ok, "bad": bad })).into_response()
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn add(State(s): State<S>, Json(r): Json<Recipe>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let Some(dir) = regions_dir(&s2) else { return err(StatusCode::SERVICE_UNAVAILABLE, "the NAS isn't reachable: edits wait until it's back") };
        if let Err(e) = r.validate() {
            return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
        }
        if dir.join(format!("{}.toml", r.id)).exists() {
            return err(StatusCode::CONFLICT, format!("a region {} exists already", r.id));
        }
        match recipes::add(&dir, &r) {
            Ok(()) => Json(json!({ "added": r.id })).into_response(),
            Err(e) => err(StatusCode::CONFLICT, format!("{e:#}")),
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Deserialize)]
pub struct Edit {
    name: Option<String>,
    outline: Option<Vec<String>>,
}

pub async fn edit(State(s): State<S>, Path(id): Path<String>, Json(e): Json<Edit>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let Some(dir) = regions_dir(&s2) else { return err(StatusCode::SERVICE_UNAVAILABLE, "the NAS isn't reachable") };
        let (ok, _) = recipes::load(&dir);
        let Some(mut r) = ok.into_iter().find(|r| r.id == id) else { return err(StatusCode::NOT_FOUND, format!("no region {id}")) };
        if let Some(n) = e.name {
            r.name = n;
        }
        if let Some(o) = e.outline {
            r.outline = o;
        }
        if let Err(e) = r.validate() {
            return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
        }
        let p = dir.join(format!("{id}.toml"));
        let tmp = dir.join(format!("{id}.toml.tmp"));
        let res = toml::to_string(&r).map_err(anyhow::Error::from).and_then(|t| Ok(std::fs::write(&tmp, t)?)).and_then(|_| Ok(std::fs::rename(&tmp, &p)?));
        match res {
            Ok(()) => Json(json!({ "edited": id })).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn remove(State(s): State<S>, Path(id): Path<String>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let Some(dir) = regions_dir(&s2) else { return err(StatusCode::SERVICE_UNAVAILABLE, "the NAS isn't reachable") };
        match recipes::remove(&dir, &id) {
            Ok(()) => Json(json!({ "removed": id })).into_response(),
            Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Deserialize)]
pub struct AtQ {
    at: String,
}

pub async fn at(State(s): State<S>, Query(q): Query<AtQ>) -> Response {
    let v: Vec<f64> = q.at.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if v.len() != 2 {
        return err(StatusCode::BAD_REQUEST, "at=lon,lat");
    }
    let p = [(v[0] * 1e7).round() as i32, (v[1] * 1e7).round() as i32];
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let Some(ix) = s2.areas.get(&s2)? else { return Ok(err(StatusCode::NOT_FOUND, "no outlines yet (the OSM pass makes them)")) };
        let mut found: Vec<&OutlineRec> = Vec::new();
        for o in ix.recs.iter().filter(|o| p[0] >= o.bbox[0] && p[0] <= o.bbox[2] && p[1] >= o.bbox[1] && p[1] <= o.bbox[3]) {
            let rings = ix.rings(o, false)?;
            let r: Vec<&[[i32; 2]]> = rings.iter().map(Vec::as_slice).collect();
            if inside(&r, p) {
                found.push(o);
            }
        }
        found.sort_by(|a, b| a.area_km2.total_cmp(&b.area_km2));
        Ok(Json(json!({ "areas": found.iter().map(|o| ix.summary(o)).collect::<Vec<_>>() })).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Deserialize)]
pub struct SearchQ {
    q: String,
}

pub async fn search(State(s): State<S>, Query(q): Query<SearchQ>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let Some(ix) = s2.areas.get(&s2)? else { return Ok(err(StatusCode::NOT_FOUND, "no outlines yet")) };
        let k = fold(q.q.trim());
        if k.is_empty() {
            return Ok(Json(json!({ "areas": [] })).into_response());
        }
        let from = ix.names.partition_point(|(n, _)| n.as_str() < k.as_str());
        let mut hits: Vec<u32> = ix.names[from..].iter().take_while(|(n, _)| n.starts_with(&k)).map(|(_, i)| *i).collect();
        hits.sort_unstable();
        hits.dedup();
        let mut recs: Vec<&OutlineRec> = hits.iter().map(|&i| &ix.recs[i as usize]).collect();
        recs.sort_by(|a, b| b.area_km2.total_cmp(&a.area_km2));
        recs.truncate(30);
        Ok(Json(json!({ "areas": recs.iter().map(|o| ix.summary(o)).collect::<Vec<_>>() })).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn feature(ix: &OutlineIndex, o: &OutlineRec, extra: Value) -> Result<Value> {
    let rings = ix.rings(o, true)?;
    let coords: Vec<Vec<Vec<[f64; 2]>>> = rings.iter().map(|r| vec![r.iter().map(|p| [p[0] as f64 * 1e-7, p[1] as f64 * 1e-7]).collect()]).collect();
    let mut props = ix.summary(o);
    if let (Some(p), Some(x)) = (props.as_object_mut(), extra.as_object()) {
        p.extend(x.clone());
    }
    // Even–odd rings as separate polygons: MapLibre fills them with the same rule.
    Ok(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": props }))
}

pub async fn one(State(s): State<S>, Path(id): Path<u64>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let Some(ix) = s2.areas.get(&s2)? else { return Ok(err(StatusCode::NOT_FOUND, "no outlines yet")) };
        let Some(o) = ix.by_id(id) else { return Ok(err(StatusCode::NOT_FOUND, format!("no outline {id}"))) };
        Ok(Json(feature(&ix, o, json!({}))?).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Every region's outlines for drawing: osm: entries simplified from the outlines, the others as
/// written (poly files, place circles).
pub async fn coverage(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let Some(dir) = regions_dir(&s2) else { return Ok(err(StatusCode::SERVICE_UNAVAILABLE, "the NAS isn't reachable")) };
        let (regions, _) = recipes::load(&dir);
        let ix = s2.areas.get(&s2)?;
        let mut feats = Vec::new();
        for r in &regions {
            for entry in &r.outline {
                let extra = json!({ "region": r.id, "region_name": r.name, "entry": entry });
                match recipes::parse_outline(entry)? {
                    recipes::Outline::Osm(id) => {
                        if let Some((ix, o)) = ix.as_ref().and_then(|ix| ix.by_id(id).map(|o| (ix, o))) {
                            feats.push(feature(ix, o, extra)?);
                        }
                    }
                    other => {
                        let rings = match other {
                            recipes::Outline::Place { lon, lat, km } => vec![(0..65)
                                .map(|i| {
                                    let t = i as f64 / 64.0 * std::f64::consts::TAU;
                                    [lon + km / (111.32 * lat.to_radians().cos().max(0.01)) * t.cos(), lat + km / 110.574 * t.sin()]
                                })
                                .collect::<Vec<_>>()],
                            recipes::Outline::Poly(f) => poly_rings(&dir.parent().unwrap_or(&dir).join("outlines").join(f))?,
                            recipes::Outline::Geofabrik(g) => poly_rings(&dir.parent().unwrap_or(&dir).join("outlines/geofabrik").join(format!("{}.poly", g.replace('/', "-"))))?,
                            recipes::Outline::Osm(_) => unreachable!(),
                        };
                        let coords: Vec<Vec<Vec<[f64; 2]>>> = rings.into_iter().map(|r| vec![r]).collect();
                        feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": extra }));
                    }
                }
            }
        }
        Ok(Json(json!({ "type": "FeatureCollection", "features": feats })).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn poly_rings(p: &std::path::Path) -> Result<Vec<Vec<[f64; 2]>>> {
    let text = std::fs::read_to_string(p).with_context(|| format!("{}", p.display()))?;
    Ok(pipeline::coverage::read_poly(&text)?.into_iter().map(|r| r.into_iter().map(|q| [q[0] as f64 * 1e-7, q[1] as f64 * 1e-7]).collect()).collect())
}
