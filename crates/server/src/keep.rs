//! Kept areas (docs/plan.md §4, "Mirror, per Mac"; §1, the Regions panel): the regions and views
//! this Mac keeps for offline use, for trips away from the NAS. Their files are copied before the
//! rest of the map and never evicted (store::mirror); the panel shows each region's size and how
//! much of it is here, and the mirror's state.
//!
//!   GET    /api/keep                 the mirror's state: this Mac's room, the files every Mac keeps,
//!                                    every built region's size, how much of it is here and whether
//!                                    it's kept, the kept views, the copy under way
//!   PUT    /api/keep/regions/{id}    {keep}: keep a region on this Mac, or not
//!   POST   /api/keep/views           {outline: [[lon, lat], …], name?}: keep an area (the ground in
//!                                    view), named after the place search's nearest place unless
//!                                    named
//!   PUT    /api/keep/views/{id}      {name}: rename a kept view
//!   DELETE /api/keep/views/{id}      stop keeping it
//!
//! What's kept is this Mac's alone: `<home>/keep.json` (docs/formats.md), never the NAS.
//!
//! An area's files (`files_of`): every layer's hi pack (zooms 9–14), and the base pack and road
//! values, of each z6 tile within 2 km of it (its outlines are simplified), and the hi data of the
//! z6 tiles within 50 km (what a view's lists read around it: a drive's window reaches 50 km). Every
//! area needs the essentials too (store::mirror::essentials: worldwide files, root and lo packs,
//! landmark points and area details, which every Mac keeps) and the basemap's archives, kept while
//! any area is.

use crate::data::Data;
use crate::S;
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::f64::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use store::catalog::Catalog;
use store::mirror::{Mirror, SyncEnd};

/// How far around an area its tiles are kept: its outlines are simplified (60 m to 1 km), and the
/// build reaches 1 km past coasts.
const NEAR_KM: f64 = 2.0;
/// How far around an area its hi data are kept: the lists of a view (scenic drives and rides, rail
/// lines) read the tiles within half their window of it, 50 km at most.
const LISTS_KM: f64 = 50.0;
/// How long a new kept view waits for the place search to name it.
const NAMING: Duration = Duration::from_secs(8);
/// A view's outline: the ground on screen, a few dozen points.
const MAX_POINTS: usize = 1000;
const FILE: &str = "keep.json";

/// What this Mac keeps (`keep.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pins {
    pub fmt: u32,
    /// Kept regions, by id, as they were named when kept (for one the map no longer has).
    #[serde(default)]
    pub regions: Vec<PinRegion>,
    /// Kept views, in the order they were kept.
    #[serde(default)]
    pub views: Vec<PinView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinRegion {
    pub id: String,
    pub name: String,
    /// When it was kept (seconds since 1970).
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinView {
    pub id: String,
    pub name: String,
    /// The ground that was in view: [lon, lat] degrees, a ring (longitudes may run past ±180 where
    /// it crosses the antimeridian).
    pub outline: Vec<[f64; 2]>,
    pub at: u64,
}

/// Some of the catalog's files: (content name, size), sorted by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub names: Vec<(String, u64)>,
    pub bytes: u64,
}

impl Files {
    fn from_map(m: BTreeMap<String, u64>) -> Files {
        let bytes = m.values().sum();
        Files { names: m.into_iter().collect(), bytes }
    }

    /// The catalog's files of these content names.
    fn of_contents<'a>(cat: &Catalog, contents: impl IntoIterator<Item = &'a str>) -> Files {
        let sizes: std::collections::HashMap<&str, u64> = cat.files.values().map(|f| (f.file.as_str(), f.size)).collect();
        Files::from_map(contents.into_iter().filter_map(|c| sizes.get(c).map(|&s| (c.to_string(), s))).collect())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.binary_search_by(|(n, _)| n.as_str().cmp(name)).is_ok()
    }
}

/// Every region the catalog records, its files, by id.
type Regions = Arc<BTreeMap<String, Files>>;

/// What the mirror keeps for one catalog and one set of pins.
pub struct Plan {
    /// The catalog generation and the pins' version it was made for.
    generation: u64,
    pins: u64,
    /// Content names never evicted besides the essentials: the kept areas' files and, while any
    /// area is kept, the basemap's archives.
    pub keep: HashSet<String>,
    /// Every region the catalog records, its files (for its size), by id.
    pub regions: Regions,
    /// The kept views' files, in the pins' order.
    pub views: Vec<Files>,
    /// The files every Mac keeps (store::mirror::essentials), and the basemap's archives.
    pub essentials: Files,
    pub basemap: Files,
    pub pinned: Pins,
}

/// The kept areas: the pins, the plan made of them, and the mirror thread's alarm.
pub struct Keep {
    path: std::path::PathBuf,
    pins: Mutex<Pins>,
    /// Bumped at every change of the pins.
    version: AtomicU64,
    plan: Mutex<Option<Arc<Plan>>>,
    /// The regions' files for a catalog generation (they don't change with the pins).
    regions: Mutex<Option<(u64, Regions)>>,
    /// Set when the pins change, to wake the mirror thread.
    wake: (Mutex<bool>, Condvar),
}

impl Keep {
    /// The pins in `home` (none yet: none kept). A file that doesn't read is set aside as
    /// `keep.json.bad`, and nothing is kept until the panel keeps something again.
    pub fn load(home: &std::path::Path) -> Arc<Keep> {
        let path = home.join(FILE);
        let pins = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice::<Pins>(&b).unwrap_or_else(|e| {
                eprintln!("keep: {} doesn't read ({e}); set aside as {FILE}.bad", path.display());
                let _ = std::fs::rename(&path, home.join(format!("{FILE}.bad")));
                Pins::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Pins::default(),
            Err(e) => {
                eprintln!("keep: can't read {}: {e}", path.display());
                Pins::default()
            }
        };
        Arc::new(Keep { path, pins: Mutex::new(pins), version: AtomicU64::new(1), plan: Mutex::new(None), regions: Mutex::new(None), wake: (Mutex::new(false), Condvar::new()) })
    }

    pub fn pins(&self) -> Pins {
        self.pins.lock().unwrap().clone()
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Changes the pins with `f` and writes them out (through a temporary file); the mirror thread
    /// is woken to copy what's newly kept first. Nothing changes when `f` fails or the file can't
    /// be written.
    fn edit<T>(&self, f: impl FnOnce(&mut Pins) -> Result<T>) -> Result<T> {
        let mut g = self.pins.lock().unwrap();
        let mut next = g.clone();
        let out = f(&mut next)?;
        next.fmt = 1;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&next)?).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("rename {}", tmp.display()))?;
        *g = next;
        self.version.fetch_add(1, Ordering::SeqCst);
        drop(g);
        let (lock, cv) = &self.wake;
        *lock.lock().unwrap() = true;
        cv.notify_all();
        Ok(out)
    }

    /// Waits until the pins change, or `timeout`.
    fn wait(&self, timeout: Duration) {
        let (lock, cv) = &self.wake;
        let g = lock.lock().unwrap();
        let (mut g, _) = cv.wait_timeout_while(g, timeout, |woken| !*woken).unwrap();
        *g = false;
    }

    /// The plan for the catalog being served and the pins as they are.
    pub fn plan(&self, data: &Data) -> Arc<Plan> {
        let generation = data.generation.load(Ordering::SeqCst);
        let version = self.version();
        if let Some(p) = self.plan.lock().unwrap().as_ref() {
            if p.generation == generation && p.pins == version {
                return p.clone();
            }
        }
        let cat = data.catalog();
        let regions = {
            let mut g = self.regions.lock().unwrap();
            match g.as_ref() {
                Some((gen, r)) if *gen == generation => r.clone(),
                _ => {
                    let r: Regions = Arc::new(crate::regions::recorded(&cat).iter().map(|r| (r.id.clone(), files_of(&cat, &outer_rings(r)))).collect());
                    *g = Some((generation, r.clone()));
                    r
                }
            }
        };
        let pinned = self.pins();
        let views: Vec<Files> = pinned.views.iter().map(|v| files_of(&cat, std::slice::from_ref(&v.outline))).collect();
        let essentials = Files::of_contents(&cat, store::mirror::essentials(&cat).iter().map(String::as_str));
        let basemap = Files::of_contents(&cat, cat.basemap.iter().filter_map(|l| cat.content(l)));
        let mut keep = HashSet::new();
        let mut any = false;
        for f in pinned.regions.iter().filter_map(|p| regions.get(&p.id)).chain(&views) {
            keep.extend(f.names.iter().map(|(n, _)| n.clone()));
            any = true;
        }
        if any {
            keep.extend(basemap.names.iter().map(|(n, _)| n.clone()));
        }
        let plan = Arc::new(Plan { generation, pins: version, keep, regions, views, essentials, basemap, pinned });
        *self.plan.lock().unwrap() = Some(plan.clone());
        plan
    }
}

// ---- which files an area needs ---------------------------------------------------------------

/// A region's outer rings, as the catalog records them (simplified; holes left out: a tile in a
/// hole is kept all the same).
fn outer_rings(r: &pipeline::coverage::DrawnRegion) -> Vec<Vec<[f64; 2]>> {
    r.shapes.values().flat_map(|polys| polys.iter().filter_map(|p| p.first().cloned())).collect()
}

/// The files an area (`rings`, each read as a filled area) needs of `cat`, the essentials and the
/// basemap aside (module doc).
pub fn files_of(cat: &Catalog, rings: &[Vec<[f64; 2]>]) -> Files {
    let mut out = BTreeMap::new();
    let mut add = |logical: &String| {
        if let Some(f) = cat.files.get(logical) {
            out.insert(f.file.clone(), f.size);
        }
    };
    for (x, y) in tiles_meeting(rings, NEAR_KM) {
        let key = format!("6/{x}/{y}");
        for l in cat.layers.values() {
            if let Some(h) = l.hi.get(&key) {
                add(h);
            }
        }
        for m in [&cat.base, &cat.roads] {
            if let Some(l) = m.get(&key) {
                add(l);
            }
        }
    }
    for (x, y) in tiles_meeting(rings, LISTS_KM) {
        if let Some(l) = cat.hidata.get(&format!("6/{x}/{y}")) {
            add(l);
        }
    }
    Files::from_map(out)
}

const N6: f64 = 64.0;

fn lon_x(lon: f64) -> f64 {
    (lon + 180.0) / 360.0 * N6
}

fn lat_y(lat: f64) -> f64 {
    let r = lat.clamp(-85.0511, 85.0511).to_radians();
    (1.0 - (r.tan() + 1.0 / r.cos()).ln() / PI) / 2.0 * N6
}

fn x_lon(x: f64) -> f64 {
    x / N6 * 360.0 - 180.0
}

fn y_lat(y: f64) -> f64 {
    (PI * (1.0 - 2.0 * y / N6)).sinh().atan().to_degrees()
}

/// Degrees of longitude a distance spans at a latitude.
fn km_lon(km: f64, lat: f64) -> f64 {
    km / (111.32 * lat.abs().min(89.0).to_radians().cos().max(0.01))
}

/// The z6 tiles (x, y) that `rings` (lon, lat degrees; each read as a filled area; longitudes
/// may run past ±180) meet once grown by `km`: each tile's box grown by `km` tested against each
/// ring (a vertex inside it, its centre inside the ring, or an edge across it).
pub fn tiles_meeting(rings: &[Vec<[f64; 2]>], km: f64) -> BTreeSet<(u32, u32)> {
    let mut out = BTreeSet::new();
    let dlat = km / 111.32;
    for ring in rings.iter().filter(|r| r.len() >= 3 && r.iter().all(|p| p[0].is_finite() && p[1].is_finite())) {
        let (mut w, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in ring {
            (w, e, s, n) = (w.min(p[0]), e.max(p[0]), s.min(p[1]), n.max(p[1]));
        }
        let dl = km_lon(km, s.abs().max(n.abs()) + dlat);
        let (x0, x1) = (lon_x(w - dl).floor() as i64, lon_x(e + dl).floor() as i64);
        let (y0, y1) = (lat_y(n + dlat).floor().max(0.0) as i64, lat_y(s - dlat).floor().min(N6 - 1.0) as i64);
        for x in x0..=x1.min(x0 + 2 * N6 as i64) {
            for y in y0..=y1 {
                let t = (x.rem_euclid(N6 as i64) as u32, y as u32);
                if out.contains(&t) {
                    continue;
                }
                let (bn, bs) = (y_lat(y as f64), y_lat(y as f64 + 1.0));
                let g = km_lon(km, bs.abs().max(bn.abs()));
                if ring_meets_box(ring, [x_lon(x as f64) - g, bs - dlat, x_lon(x as f64 + 1.0) + g, bn + dlat]) {
                    out.insert(t);
                }
            }
        }
    }
    out
}

/// Whether a ring (read as a filled area) and a box [w, s, e, n] meet.
fn ring_meets_box(ring: &[[f64; 2]], b: [f64; 4]) -> bool {
    if ring.iter().any(|p| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]) {
        return true;
    }
    if inside(ring, [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]) {
        return true;
    }
    let c = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]];
    (0..ring.len()).any(|i| (0..4).any(|k| cross(ring[i], ring[(i + 1) % ring.len()], c[k], c[(k + 1) % 4])))
}

/// Point in ring, even–odd.
pub(crate) fn inside(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Whether segments ab and cd cross (properly).
fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let o = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (o1, o2, o3, o4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0) && o1 != 0.0 && o2 != 0.0 && o3 != 0.0 && o4 != 0.0
}

// ---- the mirror thread --------------------------------------------------------------------------

/// Keeps the mirror (store::mirror) on its own thread: every minute, or as soon as the kept areas
/// change, it copies what the plan says, the essentials and the kept areas' files first; a new
/// catalog or a change of the kept areas has it plan again at once. Away from home, it keeps the
/// reserve. What it evicts is let go of at once (`AppState::forget_evicted`, its hook).
pub fn spawn_mirror(s: S) {
    let Some(m) = s.data.mirror.clone() else { return };
    let spawned = std::thread::Builder::new().name("mirror".into()).spawn(move || loop {
        let again = once(&s, &m);
        if let Err(e) = m.flush() {
            eprintln!("mirror: {e:#}");
        }
        if !again {
            s.keep.wait(Duration::from_secs(60));
        }
    });
    if let Err(e) = spawned {
        eprintln!("mirror: can't start its thread: {e}");
    }
}

/// One round of the mirror; whether to go again at once (the catalog or the kept areas changed
/// meanwhile).
fn once(s: &crate::AppState, m: &Mirror) -> bool {
    let plan = s.keep.plan(&s.data);
    let changed = || s.data.generation.load(Ordering::SeqCst) != plan.generation || s.keep.version() != plan.pins;
    let cat = s.data.catalog();
    // (Nothing known of the map yet: no file is the catalog's, so none may go.)
    if cat.n == 0 {
        return false;
    }
    match (s.data.nas_root(), s.data.pool()) {
        (Some(root), Some(pool)) if pool.is_online() => {
            // Paused while the build Mac runs a job: its uploads have the NAS first.
            let pause = || changed() || s.data.agent_busy();
            match m.sync(&cat, &plan.keep, &root, &pool, &pause) {
                Ok(st) => {
                    if st.copied > 0 || st.evicted > 0 || st.short > 0 {
                        eprintln!("mirror: {st:?}");
                    }
                    if st.copied > 0 {
                        s.data.forget_remote();
                    }
                }
                Err(e) => eprintln!("mirror: {e:#}"),
            }
            if changed() {
                return true;
            }
            // Every pack's index on this Mac too, for offline starts.
            if !s.data.agent_busy() {
                s.data.keep_indexes(&cat);
            }
        }
        // Away from home: the reserve all the same.
        _ => match m.keep_reserve(&cat, &plan.keep) {
            Ok(st) if st.evicted > 0 || st.short > 0 => eprintln!("mirror (away): {st:?}"),
            Ok(_) => {}
            Err(e) => eprintln!("mirror: {e:#}"),
        },
    }
    changed()
}

// ---- the state, for the panel ------------------------------------------------------------------

/// A kept area's state: all here, copying (its share here), or why not.
fn area_state(here: u64, bytes: u64, more: u64, online: bool, busy: bool) -> &'static str {
    if here >= bytes {
        "kept"
    } else if more > 0 {
        "room"
    } else if !online {
        "away"
    } else if busy {
        "paused"
    } else {
        "copying"
    }
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn now() -> u64 {
    secs(SystemTime::now())
}

/// The mirror's state for the panel (`GET /api/keep`, module doc).
pub fn status(s: &crate::AppState) -> Result<Value> {
    let Some(m) = s.data.mirror.as_ref() else {
        return Ok(json!({ "mirror": false, "online": s.data.online(), "regions": {}, "views": [] }));
    };
    let cat = s.data.catalog();
    let plan = s.keep.plan(&s.data);
    let room = m.room(&cat, &plan.keep)?;
    let copying = m.copying();
    let (online, busy) = (s.data.online(), s.data.online() && s.data.agent_busy());
    // Bytes of `f` here, the copy under way counted.
    let here = |f: &Files| -> u64 {
        let done = m.bytes_here(f.names.iter().map(|(n, s)| (n.as_str(), *s)));
        done + copying.as_ref().filter(|c| f.contains(&c.name)).map_or(0, |c| c.have)
    };
    let tally = |f: &Files| json!({ "bytes": f.bytes, "here": here(f) });
    let kept_ids: HashSet<&str> = plan.pinned.regions.iter().map(|p| p.id.as_str()).collect();
    let mut regions = serde_json::Map::new();
    let names: BTreeMap<String, String> = crate::regions::recorded_list(&cat).iter().filter_map(|r| Some((r["id"].as_str()?.to_string(), r["name"].as_str().unwrap_or_default().to_string()))).collect();
    for (id, f) in plan.regions.iter() {
        let h = here(f);
        let kept = kept_ids.contains(id.as_str());
        regions.insert(
            id.clone(),
            json!({
                "name": names.get(id).cloned().unwrap_or_default(),
                "bytes": f.bytes,
                "here": h,
                "kept": kept,
                "state": kept.then(|| area_state(h, f.bytes, room.more, online, busy)),
            }),
        );
    }
    // Kept regions the catalog doesn't have (not built yet, or removed since).
    for p in &plan.pinned.regions {
        regions.entry(p.id.clone()).or_insert_with(|| json!({ "name": p.name, "bytes": 0, "here": 0, "kept": true, "state": "missing" }));
    }
    let views: Vec<Value> = plan
        .pinned
        .views
        .iter()
        .zip(&plan.views)
        .map(|(v, f)| {
            let h = here(f);
            json!({ "id": v.id, "name": v.name, "outline": v.outline, "at": v.at, "bytes": f.bytes, "here": h, "state": area_state(h, f.bytes, room.more, online, busy) })
        })
        .collect();
    // The files to keep: the essentials, the kept areas' and (while any area is kept) the basemap.
    let to_keep = Files::of_contents(&cat, plan.essentials.names.iter().map(|(n, _)| n.as_str()).chain(plan.keep.iter().map(String::as_str)));
    let all = Files::of_contents(&cat, cat.files.values().map(|f| f.file.as_str()));
    let logical = |c: &str| store::naming::parse_content_name(c).map(|c| c.logical.to_string()).unwrap_or_default();
    let last = m.last().map(|(st, at)| {
        json!({
            "at": secs(at), "copied": st.copied, "copied_bytes": st.copied_bytes, "evicted": st.evicted, "evicted_bytes": st.evicted_bytes,
            "skipped": st.skipped, "skipped_kept": st.skipped_kept, "pending": st.pending, "short": st.short,
            "end": match st.end { SyncEnd::Done => "done", SyncEnd::Paused => "paused", SyncEnd::Offline => "offline" },
        })
    });
    Ok(json!({
        "mirror": true,
        "online": online,
        "busy": busy,
        "free": room.free,
        "reserve": room.reserve,
        "catalog": tally(&all),
        "essentials": tally(&plan.essentials),
        "basemap": { "bytes": plan.basemap.bytes, "here": here(&plan.basemap), "kept": !plan.keep.is_empty() },
        "kept": { "bytes": to_keep.bytes, "here": here(&to_keep), "more": room.more, "areas": plan.pinned.regions.len() + plan.pinned.views.len() },
        "copying": copying.as_ref().map(|c| json!({ "file": logical(&c.name), "bytes": c.size, "have": c.have, "kept": c.kept })),
        "last": last,
        "regions": regions,
        "views": views,
    }))
}

// ---- the API ----------------------------------------------------------------------------------

fn err(code: StatusCode, msg: impl std::fmt::Display) -> Response {
    (code, Json(json!({ "error": msg.to_string() }))).into_response()
}

fn no_store(v: Value) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response()
}

/// Runs `f` off the async threads: its answer, or a 400 with why (500 if it panicked).
async fn blocking(s: S, f: impl FnOnce(&crate::AppState) -> Result<Value> + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(move || f(&s)).await {
        Ok(Ok(v)) => no_store(v),
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, format!("{e:#}")),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn get_status(State(s): State<S>) -> Response {
    match tokio::task::spawn_blocking(move || status(&s)).await {
        Ok(Ok(v)) => no_store(v),
        Ok(Err(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
pub struct KeepRegion {
    keep: bool,
}

/// Keeps a region on this Mac, or not. A region the catalog doesn't have yet can be kept: its
/// files come once it's built.
pub async fn put_region(State(s): State<S>, Path(id): Path<String>, Json(q): Json<KeepRegion>) -> Response {
    blocking(s, move |s| {
        ensure!(pipeline::agent::recipes::valid_id(&id), "{id:?} isn't a region id");
        ensure!(s.data.mirror.is_some(), "this server keeps no mirror (--no-mirror)");
        let cat = s.data.catalog();
        let name = crate::regions::recorded_list(&cat).iter().find(|r| r["id"] == id.as_str()).and_then(|r| r["name"].as_str().map(str::to_string)).unwrap_or_else(|| id.clone());
        s.keep.edit(|p| {
            p.regions.retain(|r| r.id != id);
            if q.keep {
                p.regions.push(PinRegion { id: id.clone(), name, at: now() });
            }
            Ok(())
        })?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[derive(Deserialize)]
pub struct NewView {
    outline: Vec<[f64; 2]>,
    name: Option<String>,
}

/// A view's name as given: trimmed, one line, 80 characters at most.
fn clean_name(n: &str) -> Result<String> {
    let n: String = n.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(!n.is_empty(), "a name, please");
    Ok(n.chars().take(80).collect())
}

/// Keeps an area (the ground in view): named as given, else after the most important place in it
/// that the place search knows (waiting a few seconds for its index while it's made), else by
/// where it is.
pub async fn post_view(State(s): State<S>, Json(v): Json<NewView>) -> Response {
    blocking(s, move |s| {
        ensure!(s.data.mirror.is_some(), "this server keeps no mirror (--no-mirror)");
        ensure!((3..=MAX_POINTS).contains(&v.outline.len()), "an outline of 3 to {MAX_POINTS} points");
        ensure!(v.outline.iter().all(|p| p[0].is_finite() && p[1].is_finite() && p[0].abs() <= 540.0 && p[1].abs() <= 90.0), "points are [lon, lat] degrees");
        let name = match v.name.as_deref() {
            Some(n) => clean_name(n)?,
            None => name_of(s, &v.outline),
        };
        let at = now();
        let id = s.keep.edit(|p| {
            let mut id = format!("v{at}");
            let mut k = 2;
            while p.views.iter().any(|x| x.id == id) {
                id = format!("v{at}-{k}");
                k += 1;
            }
            p.views.push(PinView { id: id.clone(), name: name.clone(), outline: v.outline.clone(), at });
            Ok(id)
        })?;
        Ok(json!({ "id": id, "name": name }))
    })
    .await
}

/// A kept view's name: the most important place in it the place search knows, else where it is
/// (the middle of its box).
fn name_of(s: &crate::AppState, ring: &[[f64; 2]]) -> String {
    let (mut w, mut so, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in ring {
        (w, e, so, n) = (w.min(p[0]), e.max(p[0]), so.min(p[1]), n.max(p[1]));
    }
    let c = ((w + e) / 2.0, (so + n) / 2.0);
    let until = Instant::now() + NAMING;
    loop {
        match s.places.get(&s.data, &s.names) {
            crate::places::Got::Ready(p) => {
                if let Some(name) = p.naming(ring, c) {
                    return name;
                }
                break;
            }
            crate::places::Got::Making if Instant::now() < until => std::thread::sleep(Duration::from_millis(200)),
            _ => break,
        }
    }
    let lon = (c.0 + 180.0).rem_euclid(360.0) - 180.0;
    format!("{:.2}° {}, {:.2}° {}", c.1.abs(), if c.1 >= 0.0 { "N" } else { "S" }, lon.abs(), if lon >= 0.0 { "E" } else { "W" })
}

#[derive(Deserialize)]
pub struct Rename {
    name: String,
}

pub async fn put_view(State(s): State<S>, Path(id): Path<String>, Json(r): Json<Rename>) -> Response {
    blocking(s, move |s| {
        let name = clean_name(&r.name)?;
        s.keep.edit(|p| match p.views.iter_mut().find(|v| v.id == id) {
            Some(v) => {
                v.name = name.clone();
                Ok(())
            }
            None => bail!("no kept view {id}"),
        })?;
        Ok(json!({ "ok": true, "name": name }))
    })
    .await
}

pub async fn delete_view(State(s): State<S>, Path(id): Path<String>) -> Response {
    blocking(s, move |s| {
        s.keep.edit(|p| {
            let before = p.views.len();
            p.views.retain(|v| v.id != id);
            ensure!(p.views.len() < before, "no kept view {id}");
            Ok(())
        })?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::catalog::{FileRef, Layer};
    use store::naming::{write_atomic, Source};

    /// A box as a closed ring.
    fn rect(w: f64, s: f64, e: f64, n: f64) -> Vec<[f64; 2]> {
        vec![[w, s], [e, s], [e, n], [w, n], [w, s]]
    }

    #[test]
    fn the_tiles_an_area_meets() {
        // A box west of Greenwich, in the z6 tile 31/21: its hi data's margin (50 km) reaches over
        // the meridian into 32/21, its tiles' (2 km) doesn't.
        let london = rect(-0.3, 51.4, -0.1, 51.6);
        assert_eq!(tiles_meeting(std::slice::from_ref(&london), NEAR_KM), [(31, 21)].into());
        assert_eq!(tiles_meeting(std::slice::from_ref(&london), LISTS_KM), [(31, 21), (32, 21)].into());
        // A tile wholly inside an area is met, though no edge or vertex of it is in the tile.
        let big = rect(-40.0, 0.0, 40.0, 60.0);
        assert!(tiles_meeting(&[big], 0.0).contains(&(32, 26)));
        // Across the antimeridian (longitudes past 180): both sides.
        let fiji = rect(179.5, -16.5, 180.5, -15.5);
        assert_eq!(tiles_meeting(&[fiji], 0.0), [(0, 34), (63, 34)].into());
        // A ring only near a tile, beyond the margin, doesn't meet it; a broken ring meets none.
        assert!(!tiles_meeting(&[rect(5.0, 45.0, 5.1, 45.1)], NEAR_KM).contains(&(31, 22)));
        assert!(tiles_meeting(&[vec![[0.0, 0.0], [f64::NAN, 1.0], [1.0, 1.0]]], 1.0).is_empty());
    }

    /// Puts a file of `size` bytes on the "NAS" and lists it in `cat`.
    fn put(root: &std::path::Path, cat: &mut Catalog, logical: &str, size: usize) -> String {
        let body: Vec<u8> = (0..size).map(|i| (i as u8).wrapping_mul(7).wrapping_add(logical.len() as u8)).collect();
        let name = write_atomic(root, logical, "bin", Source::Bytes(&body)).unwrap();
        cat.files.insert(logical.into(), FileRef { file: name.clone(), size: size as u64, fmt: 1, extra: Default::default() });
        name
    }

    /// A catalog of three z6 tiles' files (30/21, 31/21 and 32/21: the Atlantic off Ireland,
    /// England, the Low Countries), a terrain layer's root and lo packs, a worldwide file, a
    /// basemap, and one region, London, recorded in its coverage.
    fn catalog(root: &std::path::Path) -> Catalog {
        let mut c = Catalog::new(1);
        let mut terrain = Layer { encoding: "terrarium-png".into(), maxzoom: 12, ..Default::default() };
        let mut roads = Layer { encoding: "rt7".into(), minzoom: 4, maxzoom: 14, ..Default::default() };
        put(root, &mut c, "layers/terrain/root", 1_000);
        put(root, &mut c, "layers/terrain/lo/3-3-2", 2_000);
        terrain.root = Some("layers/terrain/root".into());
        terrain.lo.insert("3/3/2".into(), "layers/terrain/lo/3-3-2".into());
        for x in [30, 31, 32] {
            let (t, key) = (format!("6-{x}-21"), format!("6/{x}/21"));
            put(root, &mut c, &format!("layers/terrain/hi/{t}"), 10_000 + x);
            put(root, &mut c, &format!("layers/roads/hi/{t}"), 20_000 + x);
            put(root, &mut c, &format!("base/{t}"), 30_000 + x);
            put(root, &mut c, &format!("global/roads/{t}"), 4_000 + x);
            put(root, &mut c, &format!("hidata/{t}"), 5_000 + x);
            terrain.hi.insert(key.clone(), format!("layers/terrain/hi/{t}"));
            roads.hi.insert(key.clone(), format!("layers/roads/hi/{t}"));
            c.base.insert(key.clone(), format!("base/{t}"));
            c.roads.insert(key.clone(), format!("global/roads/{t}"));
            c.hidata.insert(key, format!("hidata/{t}"));
        }
        c.layers.insert("terrain".into(), terrain);
        c.layers.insert("roads".into(), roads);
        put(root, &mut c, "global/railfreq", 300);
        c.global.insert("railfreq".into(), "global/railfreq".into());
        put(root, &mut c, "layers/basemap/world", 50_000);
        c.basemap = vec!["layers/basemap/world".into()];
        c.coverage = json!({"recorded": true, "regions": [
            {"id": "london", "name": "London", "outline": ["osm:175342"], "shapes": {"osm:175342": [[rect(-0.3, 51.4, -0.1, 51.6)]]}},
        ]});
        c.validate().unwrap();
        store::catalog::write(&root.join("catalog"), &c).unwrap();
        c
    }

    fn logicals(f: &Files) -> Vec<String> {
        let mut v: Vec<String> = f.names.iter().map(|(n, _)| store::naming::parse_content_name(n).unwrap().logical.to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn an_areas_files_are_its_tiles_hi_packs_and_base_packs_and_the_hi_data_around() {
        let nas = tempfile::tempdir().unwrap();
        let cat = catalog(nas.path());
        let f = files_of(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]);
        assert_eq!(logicals(&f), ["base/6-31-21", "global/roads/6-31-21", "hidata/6-31-21", "hidata/6-32-21", "layers/roads/hi/6-31-21", "layers/terrain/hi/6-31-21"]);
        assert_eq!(f.bytes, 30_031 + 4_031 + 5_031 + 5_032 + 20_031 + 10_031);
        assert!(f.contains(cat.content("base/6-31-21").unwrap()) && !f.contains(cat.content("base/6-32-21").unwrap()));
    }

    async fn call(r: Response) -> (StatusCode, Value) {
        let code = r.status();
        let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        (code, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    async fn get(s: &S) -> Value {
        let (code, v) = call(get_status(State(s.clone())).await).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        v
    }

    #[tokio::test]
    async fn keeping_regions_and_views() {
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let cat = catalog(nas.path());
        let s = crate::test_state_with(home.path(), nas.path(), true);
        assert_eq!(s.data.catalog().n, 1);
        let london = files_of(&cat, &[rect(-0.3, 51.4, -0.1, 51.6)]);

        // Every region the catalog records, with its size; none kept, nothing here.
        let v = get(&s).await;
        assert_eq!(v["regions"]["london"], json!({"name": "London", "bytes": london.bytes, "here": 0, "kept": false, "state": null}));
        assert_eq!((v["mirror"].as_bool(), v["online"].as_bool(), v["views"].as_array().map(Vec::len)), (Some(true), Some(true), Some(0)));
        let all: u64 = cat.files.values().map(|f| f.size).sum();
        assert_eq!(v["catalog"], json!({"bytes": all, "here": 0}));
        // The essentials: the worldwide file, the root and lo packs.
        assert_eq!(v["essentials"], json!({"bytes": 300 + 1_000 + 2_000, "here": 0}));
        assert_eq!(v["basemap"]["kept"], json!(false));
        assert!(s.keep.plan(&s.data).keep.is_empty());

        // Kept: its files and the basemap are what the mirror keeps; written to keep.json.
        let (code, _) = call(put_region(State(s.clone()), Path("london".into()), Json(KeepRegion { keep: true })).await).await;
        assert_eq!(code, StatusCode::OK);
        let plan = s.keep.plan(&s.data);
        let want: HashSet<String> = london.names.iter().map(|(n, _)| n.clone()).chain([cat.content("layers/basemap/world").unwrap().to_string()]).collect();
        assert_eq!(plan.keep, want);
        assert_eq!(Keep::load(home.path()).pins().regions.iter().map(|r| (r.id.as_str(), r.name.as_str())).collect::<Vec<_>>(), [("london", "London")]);
        let v = get(&s).await;
        assert_eq!((v["regions"]["london"]["kept"].as_bool(), v["regions"]["london"]["state"].as_str()), (Some(true), Some("copying")));
        assert_eq!(v["kept"]["bytes"].as_u64(), Some(3_300 + london.bytes + 50_000));

        // Copied, as the mirror thread would (the store's tests have the order): kept.
        let m = s.data.mirror.clone().unwrap();
        m.sync(&cat, &plan.keep, nas.path(), &s.data.pool().unwrap(), &|| false).unwrap();
        let v = get(&s).await;
        assert_eq!(v["regions"]["london"]["here"].as_u64(), Some(london.bytes));
        assert_eq!(v["regions"]["london"]["state"].as_str(), Some("kept"));
        assert_eq!(v["kept"]["here"], v["kept"]["bytes"]);
        assert_eq!(v["catalog"]["here"].as_u64(), Some(all), "and the rest, with room for it");
        assert_eq!(v["last"]["copied"].as_u64(), Some(cat.files.len() as u64));

        // A kept region the catalog doesn't have (not built yet): listed as kept, nothing to copy.
        call(put_region(State(s.clone()), Path("kanto".into()), Json(KeepRegion { keep: true })).await).await;
        let v = get(&s).await;
        assert_eq!(v["regions"]["kanto"], json!({"name": "kanto", "bytes": 0, "here": 0, "kept": true, "state": "missing"}));
        // Not a region id: refused.
        let (code, v) = call(put_region(State(s.clone()), Path("../x".into()), Json(KeepRegion { keep: true })).await).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{v}");
        // Let go.
        for id in ["london", "kanto"] {
            call(put_region(State(s.clone()), Path(id.into()), Json(KeepRegion { keep: false })).await).await;
        }
        assert!(s.keep.plan(&s.data).keep.is_empty() && s.keep.pins().regions.is_empty());

        // A view: named as given, renamed, removed. Named by where it is when the place search
        // knows no place in it (this catalog has no labels).
        let outline = rect(4.0, 51.7, 4.6, 52.1);
        let (code, v) = call(post_view(State(s.clone()), Json(NewView { outline: outline.clone(), name: None })).await).await;
        assert_eq!(code, StatusCode::OK, "{v}");
        assert_eq!(v["name"].as_str(), Some("51.90° N, 4.30° E"));
        let id = v["id"].as_str().unwrap().to_string();
        let (code, _) = call(put_view(State(s.clone()), Path(id.clone()), Json(Rename { name: "  Rotterdam\n ".into() })).await).await;
        assert_eq!(code, StatusCode::OK);
        let v = get(&s).await;
        let view = &v["views"][0];
        assert_eq!((view["id"].as_str(), view["name"].as_str(), view["state"].as_str()), (Some(id.as_str()), Some("Rotterdam"), Some("kept")));
        assert_eq!(view["bytes"].as_u64(), Some(files_of(&cat, std::slice::from_ref(&outline)).bytes));
        assert!(s.keep.plan(&s.data).keep.contains(cat.content("layers/basemap/world").unwrap()), "the basemap is kept with any area");
        for bad in [NewView { outline: outline[..2].to_vec(), name: None }, NewView { outline: vec![[0.0, 95.0]; 4], name: None }, NewView { outline: outline.clone(), name: Some(" ".into()) }] {
            assert_eq!(call(post_view(State(s.clone()), Json(bad)).await).await.0, StatusCode::BAD_REQUEST);
        }
        assert_eq!(call(put_view(State(s.clone()), Path("v0".into()), Json(Rename { name: "X".into() })).await).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(call(delete_view(State(s.clone()), Path(id.clone())).await).await.0, StatusCode::OK);
        assert_eq!(call(delete_view(State(s.clone()), Path(id)).await).await.0, StatusCode::BAD_REQUEST);
        assert!(get(&s).await["views"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn waiting_for_room_is_said() {
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        catalog(nas.path());
        // A reserve no disk has: nothing fits.
        let data = Data::open(crate::data::Options { home: home.path().to_owned(), nas_root: Some(nas.path().to_owned()), mirror: true, reserve: u64::MAX / 4 }).unwrap();
        let s = crate::test_state_from(home.path(), data);
        call(put_region(State(s.clone()), Path("london".into()), Json(KeepRegion { keep: true })).await).await;
        let v = get(&s).await;
        assert_eq!(v["regions"]["london"]["state"].as_str(), Some("room"));
        assert!(v["kept"]["more"].as_u64().unwrap() > 0);
    }

    #[test]
    fn a_damaged_keep_file_is_set_aside() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(FILE), b"{not json").unwrap();
        let k = Keep::load(home.path());
        assert_eq!(k.pins(), Pins::default());
        assert!(home.path().join("keep.json.bad").exists() && !home.path().join(FILE).exists());
    }
}
