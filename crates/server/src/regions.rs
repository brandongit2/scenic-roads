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

    /// An outline's rings (`simple`: the simplified ones), read from the file, with their kinds
    /// (0 outer, 1 inner).
    fn rings_kinds(&self, o: &OutlineRec, simple: bool) -> Result<Vec<(u32, Vec<[i32; 2]>)>> {
        let (rs, ps, first, n) = if simple { ("srings", "spoints", o.srings, o.nsrings) } else { ("rings", "points", o.rings, o.nrings) };
        let rb = self.sect.get_part(rs, first as u64 * 16, n as usize * 16)?;
        let rings: Vec<Ring> = bytemuck::pod_collect_to_vec(&rb);
        let (Some(a), Some(b)) = (rings.first(), rings.last()) else { return Ok(Vec::new()) };
        let (start, end) = (a.start, b.start + b.count as u64);
        let pb = self.sect.get_part(ps, start * 8, ((end - start) * 8) as usize)?;
        let pts: Vec<[i32; 2]> = bytemuck::pod_collect_to_vec(&pb);
        Ok(rings.iter().map(|r| (r.kind, pts[(r.start - start) as usize..(r.start - start) as usize + r.count as usize].to_vec())).collect())
    }

    /// An outline's rings (`simple`: the simplified ones).
    pub fn rings(&self, o: &OutlineRec, simple: bool) -> Result<Vec<Vec<[i32; 2]>>> {
        Ok(self.rings_kinds(o, simple)?.into_iter().map(|(_, r)| r).collect())
    }

    /// The simplified rings as GeoJSON polygons: an outer ring, then its holes. (Outlines from passes
    /// before ring kinds were recorded have every ring outer: each its own polygon.)
    pub fn polygons(&self, o: &OutlineRec) -> Result<Vec<Vec<Vec<[f64; 2]>>>> {
        let mut out: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
        for (kind, r) in self.rings_kinds(o, true)? {
            let ring: Vec<[f64; 2]> = r.iter().map(|p| [p[0] as f64 * 1e-7, p[1] as f64 * 1e-7]).collect();
            match out.last_mut() {
                Some(p) if kind == 1 => p.push(ring),
                _ => out.push(vec![ring]),
            }
        }
        Ok(out)
    }

    /// A country's English (else own) name, from its ISO 3166-1 code's string index.
    fn country_name(&self, code: u32) -> String {
        let c = self.string(code);
        if c.is_empty() {
            return String::new();
        }
        self.recs
            .iter()
            .find(|r| r.flags & flag::ISO1 != 0 && self.string(r.iso) == c)
            .map(|r| if self.string(r.name_en).is_empty() { self.string(r.name).to_string() } else { self.string(r.name_en).to_string() })
            .unwrap_or_default()
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
            // The country it lies in (its ISO 3166-1 code, and the country's English or own name).
            "in": self.string(o.country),
            "in_name": self.country_name(o.country),
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

// ---- edits made away from home ------------------------------------------------------------------
//
// Away from home (the NAS unreachable), an edit is kept on this Mac (`regions-queue/`, one file per
// edit, in order) and the list shows it as pending; it goes to the NAS once it's back (`flush`).
// The last list read from the NAS is kept too (`regions.json`), so the panel works offline.

/// An edit made while the NAS was away.
#[derive(serde::Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Queued {
    Add { recipe: Recipe },
    Edit { id: String, name: Option<String>, outline: Option<Vec<String>> },
    Remove { id: String },
}

fn queue_dir(s: &crate::AppState) -> std::path::PathBuf {
    s.data.home.join("regions-queue")
}

fn queued(s: &crate::AppState) -> Vec<(std::path::PathBuf, Queued)> {
    let mut v: Vec<(std::path::PathBuf, Queued)> = std::fs::read_dir(queue_dir(s))
        .map(|rd| rd.flatten().filter_map(|e| {
            let p = e.path();
            let q: Queued = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((p, q))
        }).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn enqueue(s: &crate::AppState, q: &Queued) -> Result<()> {
    let d = queue_dir(s);
    std::fs::create_dir_all(&d)?;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let p = d.join(format!("{t:024}.json"));
    let tmp = d.join(format!("{t:024}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(q)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// Applies one edit to the recipes folder.
fn apply(dir: &std::path::Path, q: &Queued) -> Result<()> {
    match q {
        Queued::Add { recipe } => recipes::add(dir, recipe),
        Queued::Remove { id } => recipes::remove(dir, id),
        Queued::Edit { id, name, outline } => {
            let (ok, _) = recipes::load(dir);
            let mut r = ok.into_iter().find(|r| &r.id == id).with_context(|| format!("no region {id}"))?;
            if let Some(n) = name {
                r.name = n.clone();
            }
            if let Some(o) = outline {
                r.outline = o.clone();
            }
            r.validate()?;
            let p = dir.join(format!("{id}.toml"));
            let tmp = dir.join(format!("{id}.toml.tmp"));
            std::fs::write(&tmp, toml::to_string(&r)?)?;
            std::fs::rename(&tmp, &p)?;
            Ok(())
        }
    }
}

/// The list as these edits leave it (offline: the pending ones).
fn with_queued(mut list: Vec<Recipe>, q: &[(std::path::PathBuf, Queued)]) -> Vec<Recipe> {
    for (_, e) in q {
        match e {
            Queued::Add { recipe } => {
                list.retain(|r| r.id != recipe.id);
                list.push(recipe.clone());
            }
            Queued::Remove { id } => list.retain(|r| &r.id != id),
            Queued::Edit { id, name, outline } => {
                if let Some(r) = list.iter_mut().find(|r| &r.id == id) {
                    if let Some(n) = name {
                        r.name = n.clone();
                    }
                    if let Some(o) = outline {
                        r.outline = o.clone();
                    }
                }
            }
        }
    }
    list.sort_by(|a, b| a.id.cmp(&b.id));
    list
}

/// Sends the edits made away from home to the NAS, in order, once it's reachable. One that can't
/// apply any more (its region removed or taken meanwhile) is dropped, with a note in the log.
pub fn flush(s: &crate::AppState) {
    let Some(dir) = regions_dir(s) else { return };
    for (p, q) in queued(s) {
        match apply(&dir, &q) {
            Ok(()) => eprintln!("regions: sent an edit made away from home ({q:?})"),
            Err(e) => eprintln!("regions: an edit made away from home no longer applies, dropped: {e:#} ({q:?})"),
        }
        std::fs::remove_file(&p).ok();
    }
}

/// Every minute: send queued edits when the NAS is back.
pub fn spawn_flusher(s: S) {
    std::thread::Builder::new()
        .name("regions".into())
        .spawn(move || loop {
            if !queued(&s).is_empty() {
                flush(&s);
            }
            std::thread::sleep(std::time::Duration::from_secs(60));
        })
        .ok();
}

pub async fn list(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let q = queued(&s2);
        let cache = s2.data.home.join("regions.json");
        match regions_dir(&s2) {
            Some(dir) => {
                let (ok, bad) = recipes::load(&dir);
                if let Ok(b) = serde_json::to_vec(&ok) {
                    let tmp = cache.with_extension("json.tmp");
                    if std::fs::write(&tmp, b).is_ok() {
                        std::fs::rename(&tmp, &cache).ok();
                    }
                }
                let pending: Vec<String> = q.iter().map(|(_, e)| format!("{e:?}")).collect();
                Json(json!({ "regions": with_queued(ok, &q), "bad": bad, "pending": pending.len() })).into_response()
            }
            None => {
                // Away: the last list read, with the edits waiting to go.
                let last: Vec<Recipe> = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                Json(json!({ "regions": with_queued(last, &q), "bad": [], "pending": q.len(), "offline": true })).into_response()
            }
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// An edit: applied on the NAS, or kept on this Mac until it's reachable (202).
fn edit_or_queue(s: &crate::AppState, q: Queued) -> Response {
    match regions_dir(s) {
        Some(dir) => match apply(&dir, &q) {
            Ok(()) => Json(json!({ "done": true })).into_response(),
            Err(e) => {
                let msg = format!("{e:#}");
                let code = if msg.contains("exists") { StatusCode::CONFLICT } else if msg.contains("no region") { StatusCode::NOT_FOUND } else { StatusCode::BAD_REQUEST };
                err(code, msg)
            }
        },
        None => match enqueue(s, &q) {
            Ok(()) => (StatusCode::ACCEPTED, Json(json!({ "queued": true }))).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        },
    }
}

pub async fn add(State(s): State<S>, Json(r): Json<Recipe>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = r.validate() {
            return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
        }
        edit_or_queue(&s2, Queued::Add { recipe: r })
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
        if let Some(o) = &e.outline {
            for x in o {
                if let Err(e) = recipes::parse_outline(x) {
                    return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
                }
            }
        }
        edit_or_queue(&s2, Queued::Edit { id, name: e.name, outline: e.outline })
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn remove(State(s): State<S>, Path(id): Path<String>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || edit_or_queue(&s2, Queued::Remove { id }))
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
    let coords = ix.polygons(o)?;
    let mut props = ix.summary(o);
    if let (Some(p), Some(x)) = (props.as_object_mut(), extra.as_object()) {
        p.extend(x.clone());
    }
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
