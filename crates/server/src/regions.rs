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
//!   GET    /api/coverage            the coverage the catalog was built for: its regions' outlines,
//!                                   simplified (GeoJSON), and the regions
//!
//! Outlines come from the latest OSM pass (the catalog's `global.outlines`); records and names are
//! read once per catalog, rings when an outline is drawn or tested.
//!
//! The recipes, and the `.poly` files the coverage draws, are read and written through the NAS I/O
//! pool like every other access to the share: a mount that hangs fails the request in the pool's
//! time (a 503, as for any read the NAS can't answer) instead of holding it.

use crate::S;
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use pipeline::agent::recipes::{self, Recipe};
use pipeline::coverage::DrawnRegion;
use pipeline::outlines::{flag, inside, OutlineRec, Ring};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use store::iopool::{IoError, IoPool};

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

/// The recipes folder on the NAS, and the I/O pool every access to it goes through (None while the
/// NAS is away).
fn nas(s: &crate::AppState) -> Option<(std::path::PathBuf, Arc<IoPool>)> {
    let (root, pool) = (s.data.nas_root()?, s.data.pool()?);
    pool.is_online().then(|| (root.join("inputs/regions"), pool))
}

// ---- the recipes on the NAS ---------------------------------------------------------------------
//
// Read and written as `recipes` does for the agent and `scenic add` and `scenic remove`, but every
// access through the pool.

/// Whether a failure means the NAS couldn't be reached: the pool gave up waiting or refused (the
/// NAS offline), or the error took the NAS offline (a soft mount's network error). Nothing is wrong
/// with what was asked then, and it can be asked again.
fn nas_unreachable(e: &anyhow::Error, pool: &IoPool) -> bool {
    match IoError::find(e) {
        Some(IoError::Io(_)) => !pool.is_online(),
        Some(_) => true,
        None => false,
    }
}

/// The recipes, sorted by id, and the files that don't parse (file name, problem).
type Recipes = (Vec<Recipe>, Vec<(String, String)>);

/// Every recipe in the folder, and the files that don't parse, as `recipes::load` reads them. The
/// NAS failing midway is an error, never a shorter list.
fn load(pool: &IoPool, dir: &std::path::Path) -> Result<Recipes> {
    let items = match pool.list(dir) {
        Ok(items) => items,
        // No recipes yet.
        Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    };
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for it in items.into_iter().filter(|it| it.name.ends_with(".toml")) {
        let p = dir.join(&it.name);
        let text = match pool.read_all(&p) {
            Ok(b) => String::from_utf8(b).map_err(anyhow::Error::from),
            // Removed or renamed since the listing.
            Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // A file that can't be read is listed with the reason, unless the error took the NAS
            // offline.
            Err(IoError::Io(e)) if pool.is_online() => Err(e.into()),
            Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
        };
        match text.and_then(|t| recipes::parse(&it.name, &t)) {
            Ok(r) => ok.push(r),
            Err(e) => bad.push((it.name, format!("{e:#}"))),
        }
    }
    ok.sort_by(|a, b| a.id.cmp(&b.id));
    bad.sort();
    Ok((ok, bad))
}

/// Applies one edit to the folder. A new recipe is created exclusively (`O_EXCL`, so two Macs can't
/// both create it), a changed one is written to a temporary file renamed over it (no reader sees
/// half of it), and a removed one is renamed to `.removed`, which keeps it for undoing.
fn apply(pool: &IoPool, dir: &std::path::Path, q: &Queued) -> Result<()> {
    match q {
        Queued::Add { recipe } => {
            recipe.validate()?;
            pool.create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
            let p = dir.join(format!("{}.toml", recipe.id));
            match pool.write_new(&p, toml::to_string(recipe)?.into_bytes()) {
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => bail!("region {} exists already", recipe.id),
                r => r.with_context(|| format!("create {}", p.display())),
            }
        }
        Queued::Remove { id } => {
            ensure!(recipes::valid_id(id), "{id:?} isn't a region id");
            let p = dir.join(format!("{id}.toml"));
            match pool.rename(&p, &dir.join(format!("{id}.toml.removed"))) {
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => bail!("no region {id}"),
                r => r.with_context(|| format!("rename {}", p.display())),
            }
        }
        Queued::Edit { id, name, outline } => {
            // (The id makes a path: nothing but a region id may.)
            ensure!(recipes::valid_id(id), "{id:?} isn't a region id");
            let file = format!("{id}.toml");
            let p = dir.join(&file);
            let text = match pool.read_all(&p) {
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => bail!("no region {id}"),
                r => r.with_context(|| format!("read {}", p.display()))?,
            };
            let mut r = recipes::parse(&file, &String::from_utf8(text)?)?;
            if let Some(n) = name {
                r.name = n.clone();
            }
            if let Some(o) = outline {
                r.outline = o.clone();
            }
            r.validate()?;
            let tmp = dir.join(format!("{file}.tmp"));
            pool.write(&tmp, toml::to_string(&r)?.into_bytes()).with_context(|| format!("write {}", tmp.display()))?;
            pool.rename(&tmp, &p).with_context(|| format!("rename {}", tmp.display()))?;
            Ok(())
        }
    }
}

/// A failed edit's status: 503 when the NAS couldn't be reached (as for any read it can't answer;
/// the panel says to try again), 409 when the region exists already, 404 when there's no such
/// region, else 400.
fn status(e: &anyhow::Error, pool: &IoPool) -> StatusCode {
    let msg = format!("{e:#}");
    if nas_unreachable(e, pool) {
        StatusCode::SERVICE_UNAVAILABLE
    } else if msg.contains("exists") {
        StatusCode::CONFLICT
    } else if msg.contains("no region") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    }
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

/// The edits waiting in `queue` (this Mac's folder of them), oldest first.
fn queued(queue: &std::path::Path) -> Vec<(std::path::PathBuf, Queued)> {
    let mut v: Vec<(std::path::PathBuf, Queued)> = std::fs::read_dir(queue)
        .map(|rd| rd.flatten().filter_map(|e| {
            let p = e.path();
            let q: Queued = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((p, q))
        }).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn enqueue(queue: &std::path::Path, q: &Queued) -> Result<()> {
    std::fs::create_dir_all(queue)?;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let p = queue.join(format!("{t:024}.json"));
    let tmp = queue.join(format!("{t:024}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(q)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
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
    let Some((dir, pool)) = nas(s) else { return };
    flush_to(&pool, &dir, &queue_dir(s));
}

/// `flush`, from the folder `queue` to the recipes folder `dir`. An edit the NAS doesn't answer
/// waits, with those after it, for the next time: it's still wanted, and the order is kept.
fn flush_to(pool: &IoPool, dir: &std::path::Path, queue: &std::path::Path) {
    for (p, q) in queued(queue) {
        match apply(pool, dir, &q) {
            Ok(()) => eprintln!("regions: sent an edit made away from home ({q:?})"),
            Err(e) if nas_unreachable(&e, pool) => {
                eprintln!("regions: the edits made away from home wait, the NAS didn't take them: {e:#}");
                return;
            }
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
            if !queued(&queue_dir(&s)).is_empty() {
                flush(&s);
            }
            std::thread::sleep(std::time::Duration::from_secs(60));
        })
        .ok();
}

pub async fn list(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let q = queued(&queue_dir(&s2));
        let cache = s2.data.home.join("regions.json");
        match nas(&s2) {
            Some((dir, pool)) => {
                // The NAS not answering is a 503, as for any read it can't answer; the list kept
                // for going away stays as it was.
                let (ok, bad) = match load(&pool, &dir) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("regions: {e:#}");
                        return err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"));
                    }
                };
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

/// An edit: applied on the NAS, or kept on this Mac until it's reachable (202). A NAS that doesn't
/// answer in time is a 503, as for any request it can't answer, and the edit isn't kept: whether
/// the share still made it is only known once the list is read again.
fn edit_or_queue(s: &crate::AppState, q: Queued) -> Response {
    match nas(s) {
        Some((dir, pool)) => match apply(&pool, &dir, &q) {
            Ok(()) => Json(json!({ "done": true })).into_response(),
            Err(e) => {
                let code = status(&e, &pool);
                if code == StatusCode::SERVICE_UNAVAILABLE {
                    eprintln!("regions: {e:#}");
                }
                err(code, format!("{e:#}"))
            }
        },
        None => match enqueue(&queue_dir(s), &q) {
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

/// The regions a catalog records (its `coverage`), each with its outlines simplified for drawing;
/// none for a catalog made before they were recorded (or one whose coverage this app can't read).
pub fn recorded(cat: &store::catalog::Catalog) -> Vec<DrawnRegion> {
    cat.coverage.get("regions").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// The regions a catalog records, without their outlines (id, name and outline entries), read
/// straight from its coverage: /api/catalog asks for them every minute.
pub fn recorded_list(cat: &store::catalog::Catalog) -> Vec<Value> {
    let regions = cat.coverage.get("regions").and_then(Value::as_array);
    regions.map(|rs| rs.iter().filter(|r| r.is_object()).map(|r| json!({ "id": r["id"], "name": r["name"], "outline": r["outline"] })).collect()).unwrap_or_default()
}

/// The recorded regions' outlines as GeoJSON features, one per entry in each recipe's order, with
/// the properties the panel reads: region, region_name, entry, and an osm: entry's area fields when
/// the pass's outlines can be read (`ix`).
fn recorded_features(regions: &[DrawnRegion], ix: Option<&OutlineIndex>) -> Vec<Value> {
    let mut feats = Vec::new();
    for r in regions {
        for entry in &r.outline {
            let Some(coords) = r.shapes.get(entry) else { continue };
            let mut props = json!({ "region": r.id, "region_name": r.name, "entry": entry });
            let area = match recipes::parse_outline(entry) {
                Ok(recipes::Outline::Osm(id)) => ix.and_then(|ix| ix.by_id(id).map(|o| ix.summary(o))),
                _ => None,
            };
            if let (Some(Value::Object(a)), Some(p)) = (area, props.as_object_mut()) {
                for (k, v) in a {
                    p.entry(k).or_insert(v);
                }
            }
            feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": props }));
        }
    }
    feats
}

/// The coverage for drawing, as the current catalog recorded it, with the regions it was built for
/// (`regions`), so the panel can tell which recipes aren't on the map yet (added or redrawn since)
/// and works away from home. A catalog made before catalogs recorded their coverage has none (and
/// no `recorded` mark: one built for no region yet has it): then it's built from the recipes on the
/// NAS, as it was, without `regions`.
pub async fn coverage(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let cat = s2.data.catalog();
        let regions = recorded(&cat);
        if regions.is_empty() && cat.coverage.get("recorded").and_then(serde_json::Value::as_bool) != Some(true) {
            return coverage_from_recipes(&s2, cat.n);
        }
        // The areas' fields are a nicety: without the pass's outlines (the NAS away, not mirrored
        // yet) the outlines are drawn all the same.
        let ix = s2.areas.get(&s2).unwrap_or_else(|e| {
            eprintln!("coverage: the pass's outlines: {e:#}");
            None
        });
        let feats = recorded_features(&regions, ix.as_deref());
        Ok(Json(json!({ "type": "FeatureCollection", "features": feats, "regions": recorded_list(&cat), "catalog": cat.n })).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Every region's outlines for drawing, from the recipes on the NAS (a catalog that records none):
/// osm: entries simplified from the outlines, the others as written (poly files, place circles).
fn coverage_from_recipes(s: &crate::AppState, n: u64) -> Result<Response> {
    let Some((dir, pool)) = nas(s) else { return Ok(err(StatusCode::SERVICE_UNAVAILABLE, "the NAS isn't reachable")) };
    let (regions, _) = load(&pool, &dir)?;
    let ix = s.areas.get(s)?;
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
                        recipes::Outline::Poly(f) => poly_rings(&pool, &dir.parent().unwrap_or(&dir).join("outlines").join(f))?,
                        recipes::Outline::Geofabrik(g) => poly_rings(&pool, &dir.parent().unwrap_or(&dir).join("outlines/geofabrik").join(format!("{}.poly", g.replace('/', "-"))))?,
                        recipes::Outline::Osm(_) => unreachable!(),
                    };
                    let coords: Vec<Vec<Vec<[f64; 2]>>> = rings.into_iter().map(|r| vec![r]).collect();
                    feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": extra }));
                }
            }
        }
    }
    Ok(Json(json!({ "type": "FeatureCollection", "features": feats, "catalog": n })).into_response())
}

/// A `.poly` file's rings, read whole through the pool.
fn poly_rings(pool: &IoPool, p: &std::path::Path) -> Result<Vec<Vec<[f64; 2]>>> {
    let text = String::from_utf8(pool.read_all(p).with_context(|| format!("{}", p.display()))?).with_context(|| format!("{}", p.display()))?;
    Ok(pipeline::coverage::read_poly(&text)?.into_iter().map(|r| r.into_iter().map(|q| [q[0] as f64 * 1e-7, q[1] as f64 * 1e-7]).collect()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A pool over the share `root` that gives up on an operation after 300 ms.
    fn pool(root: &std::path::Path) -> Arc<IoPool> {
        let cfg = store::iopool::PoolConfig::new(2, Duration::from_millis(300), root.to_owned());
        IoPool::with_config(store::iopool::PoolConfig { probe_interval: Duration::from_millis(50), ..cfg })
    }

    fn recipe(id: &str, name: &str) -> Recipe {
        Recipe { id: id.into(), name: name.into(), outline: vec!["osm:1877178".into()] }
    }

    fn rename(id: &str, name: &str) -> Queued {
        Queued::Edit { id: id.into(), name: Some(name.into()), outline: None }
    }

    /// Makes `p` a named pipe: reading it waits for a writer that never comes, as a read waits on a
    /// hung mount.
    fn hang(p: &std::path::Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo {}", p.display());
    }

    /// Lets the reads waiting on `hang`'s pipe finish (with nothing), so no pool thread stays stuck.
    fn release(p: &std::path::Path) {
        use std::os::unix::fs::OpenOptionsExt;
        let end = Instant::now() + Duration::from_secs(5);
        // (A writer that doesn't wait opens once a reader waits, and closes at once.)
        while let Err(e) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(p) {
            assert!(Instant::now() < end, "nothing reads {}: {e}", p.display());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn edits_through_the_pool() {
        let share = tempfile::tempdir().unwrap();
        let dir = share.path().join("inputs/regions");
        let pool = pool(share.path());
        // No folder yet: no recipes, and the first one added makes it.
        assert_eq!(load(&pool, &dir).unwrap(), (vec![], vec![]));
        apply(&pool, &dir, &Queued::Add { recipe: recipe("borders", "Scottish Borders") }).unwrap();
        // Created exclusively: a taken id is a 409.
        let e = apply(&pool, &dir, &Queued::Add { recipe: recipe("borders", "Other") }).unwrap_err();
        assert_eq!(status(&e, &pool), StatusCode::CONFLICT, "{e:#}");
        // Listed as `recipes::load` lists them: other files left out, broken ones reported.
        std::fs::write(dir.join("bad.toml"), "id = \"other\"\nname = \"x\"\noutline = [\"osm:1\"]").unwrap();
        std::fs::write(dir.join("old.toml.removed"), "").unwrap();
        let (ok, bad) = load(&pool, &dir).unwrap();
        assert_eq!((ok.clone(), bad.clone()), recipes::load(&dir));
        assert_eq!((ok, bad.len()), (vec![recipe("borders", "Scottish Borders")], 1));
        // Changed through a temporary file, also over one an unfinished edit left.
        apply(&pool, &dir, &rename("borders", "The Borders")).unwrap();
        std::fs::write(dir.join("borders.toml.tmp"), "half a recipe, and then some").unwrap();
        apply(&pool, &dir, &Queued::Edit { id: "borders".into(), name: None, outline: Some(vec!["osm:1".into()]) }).unwrap();
        assert_eq!(load(&pool, &dir).unwrap().0, [Recipe { outline: vec!["osm:1".into()], ..recipe("borders", "The Borders") }]);
        assert!(!dir.join("borders.toml.tmp").exists());
        // Removed: renamed to .removed, kept for undoing.
        apply(&pool, &dir, &Queued::Remove { id: "borders".into() }).unwrap();
        assert!(load(&pool, &dir).unwrap().0.is_empty());
        assert!(recipes::parse("borders.toml", &std::fs::read_to_string(dir.join("borders.toml.removed")).unwrap()).is_ok());
        // No such region: 404. An id that isn't one never makes a path: 400.
        for q in [Queued::Remove { id: "borders".into() }, rename("borders", "X")] {
            assert_eq!(status(&apply(&pool, &dir, &q).unwrap_err(), &pool), StatusCode::NOT_FOUND);
        }
        let outside = share.path().join("inputs/x.toml");
        std::fs::write(&outside, toml::to_string(&recipe("x", "X")).unwrap()).unwrap();
        for q in [Queued::Remove { id: "../x".into() }, rename("../x", "Y")] {
            assert_eq!(status(&apply(&pool, &dir, &q).unwrap_err(), &pool), StatusCode::BAD_REQUEST);
        }
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), toml::to_string(&recipe("x", "X")).unwrap());
        assert!(pool.is_online());
    }

    #[test]
    fn a_hung_share_fails_promptly() {
        let share = tempfile::tempdir().unwrap();
        let dir = share.path().join("inputs/regions");
        let pool = pool(share.path());
        apply(&pool, &dir, &Queued::Add { recipe: recipe("borders", "Scottish Borders") }).unwrap();
        // A recipe the share never gives.
        let stuck = dir.join("stuck.toml");
        hang(&stuck);
        let t0 = Instant::now();
        let e = load(&pool, &dir).unwrap_err();
        assert!(nas_unreachable(&e, &pool), "{e:#}");
        let e = apply(&pool, &dir, &rename("stuck", "Stuck")).unwrap_err();
        assert_eq!(status(&e, &pool), StatusCode::SERVICE_UNAVAILABLE, "{e:#}");
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
        release(&stuck);
        // The share still answers, so it's busy rather than away: the rest goes on.
        assert!(pool.is_online());
        apply(&pool, &dir, &rename("borders", "The Borders")).unwrap();
    }

    #[test]
    fn edits_made_away_wait_until_the_nas_takes_them() {
        let (share, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (dir, queue) = (share.path().join("inputs/regions"), home.path().join("regions-queue"));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = pool(share.path());
        enqueue(&queue, &rename("borders", "The Borders")).unwrap();
        enqueue(&queue, &Queued::Add { recipe: recipe("kanto", "Kanto") }).unwrap();
        enqueue(&queue, &Queued::Add { recipe: recipe("kanto", "Kanto again") }).unwrap();
        // The share doesn't give the first edit's recipe: nothing is sent or dropped, out of order.
        let borders = dir.join("borders.toml");
        hang(&borders);
        flush_to(&pool, &dir, &queue);
        release(&borders);
        assert_eq!(queued(&queue).len(), 3);
        assert!(!dir.join("kanto.toml").exists());
        // It answers: they go in order, and the one that no longer applies (its id taken) is dropped.
        std::fs::remove_file(&borders).unwrap();
        std::fs::write(&borders, toml::to_string(&recipe("borders", "Scottish Borders")).unwrap()).unwrap();
        flush_to(&pool, &dir, &queue);
        assert!(queued(&queue).is_empty());
        assert_eq!(load(&pool, &dir).unwrap().0, [recipe("borders", "The Borders"), recipe("kanto", "Kanto")]);
    }

    #[test]
    fn a_network_error_is_the_nas_away() {
        let share = tempfile::tempdir().unwrap();
        let cfg = store::iopool::PoolConfig::new(1, Duration::from_secs(1), share.path().to_owned());
        let pool = IoPool::with_config(store::iopool::PoolConfig { probe_interval: Duration::from_secs(60), ..cfg });
        // The edit's own errors, the NAS there.
        let denied = anyhow::Error::from(IoError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        assert!(!nas_unreachable(&denied, &pool));
        assert_eq!(status(&anyhow::anyhow!("no region x"), &pool), StatusCode::NOT_FOUND);
        // A soft mount's network error takes the NAS offline: the NAS away, nothing wrong with the edit.
        let e = anyhow::Error::from(pool.call(|| -> std::io::Result<()> { Err(std::io::Error::from_raw_os_error(libc::ENOTCONN)) }).unwrap_err());
        assert!(matches!(IoError::find(&e), Some(IoError::Io(_))));
        assert_eq!(status(&e, &pool), StatusCode::SERVICE_UNAVAILABLE);
    }

    async fn json_of(r: Response) -> Value {
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    fn ids(v: &Value) -> Vec<String> {
        v["regions"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
    }

    #[tokio::test]
    async fn away_edits_wait_on_this_mac() {
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, gone) = (nas.path().join("project"), nas.path().join("gone"));
        std::fs::create_dir_all(root.join("inputs/regions")).unwrap();
        std::fs::write(root.join("inputs/regions/borders.toml"), toml::to_string(&recipe("borders", "Scottish Borders")).unwrap()).unwrap();
        let s = crate::test_state(home.path(), &root);
        let v = json_of(list(State(s.clone())).await).await;
        assert_eq!((ids(&v), v.get("offline")), (vec!["borders".to_string()], None));
        // The share goes away: edits wait on this Mac, and the list is the last one read with them.
        std::fs::rename(&root, &gone).unwrap();
        s.data.pool().unwrap().mark_offline("test");
        assert_eq!(add(State(s.clone()), Json(recipe("kanto", "Kanto"))).await.status(), StatusCode::ACCEPTED);
        assert_eq!(remove(State(s.clone()), Path("borders".into())).await.status(), StatusCode::ACCEPTED);
        let v = json_of(list(State(s.clone())).await).await;
        assert_eq!((ids(&v), v["offline"].as_bool(), v["pending"].as_u64()), (vec!["kanto".to_string()], Some(true), Some(2)));
        assert_eq!(coverage(State(s.clone())).await.status(), StatusCode::SERVICE_UNAVAILABLE);
        // Back (the pool's prober sees it): they go to the NAS.
        std::fs::rename(&gone, &root).unwrap();
        let t0 = Instant::now();
        while !s.data.online() {
            assert!(t0.elapsed() < Duration::from_secs(30), "the NAS never came back");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        flush(&s);
        let v = json_of(list(State(s.clone())).await).await;
        assert_eq!((ids(&v), v["pending"].as_u64()), (vec!["kanto".to_string()], Some(0)));
        assert!(root.join("inputs/regions/borders.toml.removed").exists());
    }

    fn catalog(coverage: Value) -> store::catalog::Catalog {
        let mut c = store::catalog::Catalog::new(3);
        c.coverage = coverage;
        c
    }

    #[test]
    fn coverage_as_the_catalog_recorded_it() {
        let ring = json!([[1.0, 50.0], [2.0, 50.0], [2.0, 51.0], [1.0, 50.0]]);
        let c = catalog(json!({"regions": [
            {"id": "b", "name": "B", "outline": ["poly:b.poly", "osm:7"], "shapes": {"poly:b.poly": [[ring]]}},
            {"id": "a", "name": "A", "outline": ["place:1,2,3"], "shapes": {}},
        ]}));
        let r = recorded(&c);
        assert_eq!(r.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        // A feature per entry with an outline, with what the panel reads (an osm: entry's area
        // fields come from the pass's outlines, none here).
        let f = recorded_features(&r, None);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0]["properties"], json!({"region": "b", "region_name": "B", "entry": "poly:b.poly"}));
        assert_eq!(f[0]["geometry"], json!({"type": "MultiPolygon", "coordinates": [[ring]]}));
        // The regions, outlines left out; every region recorded, drawn or not.
        assert_eq!(recorded_list(&c), [json!({"id": "b", "name": "B", "outline": ["poly:b.poly", "osm:7"]}), json!({"id": "a", "name": "A", "outline": ["place:1,2,3"]})]);
        // Catalogs made before catalogs recorded their coverage: none (then it's the recipes').
        for old in [json!({"regions": []}), Value::Null, json!({"regions": ["northumberland"]})] {
            assert!(recorded(&catalog(old.clone())).is_empty() && recorded_list(&catalog(old)).is_empty());
        }
    }
}
