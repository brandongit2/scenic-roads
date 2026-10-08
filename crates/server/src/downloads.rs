//! Downloads (docs/plan.md §4, "Mirror, per Mac"; §1, the Regions panel): what the owner has
//! downloaded to this Mac for when it's away from the NAS. Nothing else is copied here, and nothing
//! downloaded goes until the owner removes it (store::mirror).
//!
//!   GET    /api/downloads                 this Mac's room, the World download, every built
//!                                         region's size, how much of it is here and whether it's
//!                                         downloaded, the downloaded views, the copy under way
//!   PUT    /api/downloads/world           {on}: download the World, zoomed out, or remove it
//!   PUT    /api/downloads/regions/{id}    {on}: download a region (with the World), or remove it
//!   POST   /api/downloads/views/size      {outline}: what downloading that view would take
//!   POST   /api/downloads/views           {outline: [[lon, lat], …], name?}: download an area (the
//!                                         ground in view), named after the place search's most
//!                                         important place in it unless named
//!   PUT    /api/downloads/views/{id}      {name}: rename a downloaded view
//!   DELETE /api/downloads/views/{id}      remove it
//!
//! A download that wouldn't fit (all that's downloaded, less what's here, more than the free space
//! above the reserve) is refused, with the numbers. What's downloaded is this Mac's alone:
//! `<home>/downloads.json` (docs/formats.md), never the NAS.
//!
//! - **The World, zoomed out** (`world_part`): the worldwide files, every layer's root and lo
//!   packs, the landmark points and area details (store::mirror::essentials), and the basemap's
//!   zooms 0–10 (its `lo` piece, store::pieces). Without it the map works only while the NAS is
//!   reachable. A region or view needs it to work offline: downloading one downloads the World too,
//!   and the World isn't removed while one is downloaded.
//! - **An area** (a region, or a view: `files_of`, `pieces_of`): every layer's hi pack (zooms
//!   9–14), and the base pack and road values, of each z6 tile within 2 km of it (its outlines are
//!   simplified), and the basemap's zooms 11–14 over those tiles; the terrain's and the grids' hi
//!   packs within 25 km (the viewshed's reach, so one from inside works offline); and the hi data
//!   of the z6 tiles within 50 km (what a view's lists read around it).

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
use store::mirror::{Control, Item, Mirror, SyncEnd, Wanted};
use store::pieces::Piece;

/// How far around an area its tiles are kept: its outlines are simplified (60 m to 1 km), and the
/// build reaches 1 km past coasts.
const NEAR_KM: f64 = 2.0;
/// How far around an area its hi data are kept: the lists of a view (scenic drives and rides, rail
/// lines) read the tiles within half their window of it, 50 km at most.
const LISTS_KM: f64 = 50.0;
/// How far around an area the layers the viewshed reads are kept (its z11 terrain, canopy heights
/// and land cover): its radius goes to 25 km (viewshed.rs), so one from inside works offline.
const VIEWSHED_KM: f64 = 25.0;
const VIEWSHED_LAYERS: [&str; 3] = ["terrain", "grid-canopy", "grid-class"];
/// How long a new view waits for the place search to name it.
const NAMING: Duration = Duration::from_secs(8);
/// A view's outline: the ground on screen, a few dozen points.
const MAX_POINTS: usize = 1000;
const FILE: &str = "downloads.json";
/// What this Mac kept before downloads (`fmt` 1): read once, made downloads.
const OLD_FILE: &str = "keep.json";

/// What this Mac has downloaded (`downloads.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Asked {
    pub fmt: u32,
    /// When the World, zoomed out, was downloaded (seconds since 1970); None: it isn't.
    #[serde(default)]
    pub world: Option<u64>,
    /// Downloaded regions, by id, as they were named then (for one the map no longer has).
    #[serde(default)]
    pub regions: Vec<AskedRegion>,
    /// Downloaded views, in the order they were downloaded.
    #[serde(default)]
    pub views: Vec<AskedView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskedRegion {
    pub id: String,
    pub name: String,
    /// When it was downloaded (seconds since 1970).
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskedView {
    pub id: String,
    pub name: String,
    /// The ground that was in view: [lon, lat] degrees, a ring (longitudes may run past ±180 where
    /// it crosses the antimeridian).
    pub outline: Vec<[f64; 2]>,
    pub at: u64,
}

impl Asked {
    fn any_area(&self) -> bool {
        !self.regions.is_empty() || !self.views.is_empty()
    }
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

    pub fn contains(&self, name: &str) -> bool {
        self.names.binary_search_by(|(n, _)| n.as_str().cmp(name)).is_ok()
    }
}

/// What a download copies: files of the catalog and pieces of its basemap's archives, in copy order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub files: Files,
    pub items: Vec<Item>,
}

impl Part {
    /// The files (`names`, content names; put in copy order) and the pieces of each of `archives`.
    fn new(cat: &Catalog, files: Files, archives: &[String], pieces: &[Piece]) -> Part {
        let by_content: std::collections::HashMap<&str, &str> = cat.files.iter().map(|(l, f)| (f.file.as_str(), l.as_str())).collect();
        let mut logicals: Vec<&str> = files.names.iter().filter_map(|(n, _)| by_content.get(n.as_str()).copied()).collect();
        store::mirror::copy_order(cat, &mut logicals);
        let mut items: Vec<Item> = logicals.iter().filter_map(|l| cat.files.get(*l)).map(|f| Item::File { name: f.file.clone(), size: f.size }).collect();
        for a in archives {
            items.extend(pieces.iter().map(|&p| Item::Piece { archive: a.clone(), piece: p }));
        }
        Part { files, items }
    }

    /// Its size as far as it's known, how much of it is here, and how many of its pieces aren't
    /// sized yet.
    fn tally(&self, m: &Mirror) -> (u64, u64, u32) {
        let (mut bytes, mut here, mut unknown) = (0, 0, 0);
        for i in &self.items {
            match m.item_size(i) {
                Some(s) => {
                    bytes += s;
                    here += m.item_here(i).min(s);
                }
                None => unknown += 1,
            }
        }
        (bytes, here, unknown)
    }

    fn pieces(&self) -> impl Iterator<Item = (&str, Piece)> {
        self.items.iter().filter_map(|i| match i {
            Item::Piece { archive, piece } => Some((archive.as_str(), *piece)),
            Item::File { .. } => None,
        })
    }
}

/// Every region the catalog records, what downloading it copies, by id.
type Regions = Arc<BTreeMap<String, Part>>;

/// What the mirror copies for one catalog and one set of downloads.
pub struct Plan {
    /// The catalog generation and the downloads' version it was made for.
    generation: u64,
    version: u64,
    /// What's downloaded, in copy order: the World, then the regions and views as they were asked.
    pub wanted: Wanted,
    pub world: Part,
    /// Every region the catalog records (for its size, downloaded or not).
    pub regions: Regions,
    /// The downloaded views', in their order.
    pub views: Vec<Part>,
    pub asked: Asked,
}

/// The downloads: what's asked, the plan made of it, and the mirror thread's alarm.
pub struct Downloads {
    path: std::path::PathBuf,
    asked: Mutex<Asked>,
    /// Bumped at every change of what's asked.
    version: AtomicU64,
    plan: Mutex<Option<Arc<Plan>>>,
    /// The regions' parts for a catalog generation (they don't change with the downloads).
    regions: Mutex<Option<(u64, Regions)>>,
    /// Set when the downloads change, to wake the mirror thread.
    wake: (Mutex<bool>, Condvar),
}

impl Downloads {
    /// The downloads in `home` (none yet: none). A file that doesn't read is set aside as
    /// `downloads.json.bad`. What a Mac kept before downloads (`keep.json`: kept regions and views)
    /// becomes downloads, with the World when there were any, and that file goes.
    pub fn load(home: &std::path::Path) -> Arc<Downloads> {
        let path = home.join(FILE);
        let asked = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice::<Asked>(&b).unwrap_or_else(|e| {
                eprintln!("downloads: {} doesn't read ({e}); set aside as {FILE}.bad", path.display());
                let _ = std::fs::rename(&path, home.join(format!("{FILE}.bad")));
                Asked::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => from_kept(home).unwrap_or_default(),
            Err(e) => {
                eprintln!("downloads: can't read {}: {e}", path.display());
                Asked::default()
            }
        };
        let d = Arc::new(Downloads { path, asked: Mutex::new(asked.clone()), version: AtomicU64::new(1), plan: Mutex::new(None), regions: Mutex::new(None), wake: (Mutex::new(false), Condvar::new()) });
        if asked != Asked::default() && !d.path.exists() {
            match d.edit(|_| Ok(())) {
                Ok(()) => {
                    let _ = std::fs::remove_file(home.join(OLD_FILE));
                    eprintln!("downloads: what this Mac kept is downloaded now ({} regions, {} views, and the World)", asked.regions.len(), asked.views.len());
                }
                Err(e) => eprintln!("downloads: {e:#}"),
            }
        }
        d
    }

    pub fn asked(&self) -> Asked {
        self.asked.lock().unwrap().clone()
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Changes what's asked with `f` and writes it out (through a temporary file); the mirror
    /// thread is woken. Nothing changes when `f` fails or the file can't be written.
    fn edit<T>(&self, f: impl FnOnce(&mut Asked) -> Result<T>) -> Result<T> {
        let mut g = self.asked.lock().unwrap();
        let mut next = g.clone();
        let out = f(&mut next)?;
        next.fmt = 2;
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

    /// Waits until the downloads change, or `timeout`.
    fn wait(&self, timeout: Duration) {
        let (lock, cv) = &self.wake;
        let g = lock.lock().unwrap();
        let (mut g, _) = cv.wait_timeout_while(g, timeout, |woken| !*woken).unwrap();
        *g = false;
    }

    /// The plan for the catalog being served and the downloads as they are.
    pub fn plan(&self, data: &Data) -> Arc<Plan> {
        let generation = data.generation.load(Ordering::SeqCst);
        let version = self.version();
        if let Some(p) = self.plan.lock().unwrap().as_ref() {
            if p.generation == generation && p.version == version {
                return p.clone();
            }
        }
        let cat = data.catalog();
        let regions = {
            let mut g = self.regions.lock().unwrap();
            match g.as_ref() {
                Some((gen, r)) if *gen == generation => r.clone(),
                _ => {
                    let r: Regions = Arc::new(crate::regions::recorded(&cat).iter().map(|r| (r.id.clone(), area_part(&cat, &outer_rings(r)))).collect());
                    *g = Some((generation, r.clone()));
                    r
                }
            }
        };
        let asked = self.asked();
        let plan = Arc::new(make_plan(&cat, generation, version, asked, regions));
        *self.plan.lock().unwrap() = Some(plan.clone());
        plan
    }
}

fn make_plan(cat: &Catalog, generation: u64, version: u64, asked: Asked, regions: Regions) -> Plan {
    let world = world_part(cat);
    let views: Vec<Part> = asked.views.iter().map(|v| area_part(cat, std::slice::from_ref(&v.outline))).collect();
    let mut wanted = Wanted::default();
    let mut seen = HashSet::new();
    let parts = asked.world.is_some().then_some(&world).into_iter().chain(asked.regions.iter().filter_map(|r| regions.get(&r.id))).chain(&views);
    for p in parts {
        for i in &p.items {
            wanted.push(i.clone(), &mut seen);
        }
    }
    Plan { generation, version, wanted, world, regions, views, asked }
}

/// `keep.json` (what a Mac kept before downloads) as downloads: its regions and views, and the
/// World with them.
fn from_kept(home: &std::path::Path) -> Option<Asked> {
    #[derive(Deserialize)]
    struct Kept {
        #[serde(default)]
        regions: Vec<AskedRegion>,
        #[serde(default)]
        views: Vec<AskedView>,
    }
    let k: Kept = serde_json::from_slice(&std::fs::read(home.join(OLD_FILE)).ok()?).ok()?;
    let any = !k.regions.is_empty() || !k.views.is_empty();
    Some(Asked { fmt: 2, world: any.then(now), regions: k.regions, views: k.views })
}

/// The World, zoomed out: the essentials and the basemap's zooms 0–10.
fn world_part(cat: &Catalog) -> Part {
    let ess = store::mirror::essentials(cat);
    let files = of_contents(cat, ess.iter().map(String::as_str));
    Part::new(cat, files, &basemap_archives(cat), &[Piece::Lo])
}

/// An area's download (`rings`, each read as a filled area): its files, then its basemap pieces.
fn area_part(cat: &Catalog, rings: &[Vec<[f64; 2]>]) -> Part {
    let pieces: Vec<Piece> = tiles_meeting(rings, NEAR_KM).into_iter().map(|(x, y)| Piece::Tile(x, y)).collect();
    Part::new(cat, files_of(cat, rings), &basemap_archives(cat), &pieces)
}

fn basemap_archives(cat: &Catalog) -> Vec<String> {
    cat.basemap.iter().filter_map(|l| cat.content(l)).map(str::to_string).collect()
}

/// The catalog's files of these content names.
fn of_contents<'a>(cat: &Catalog, contents: impl IntoIterator<Item = &'a str>) -> Files {
    let sizes: std::collections::HashMap<&str, u64> = cat.files.values().map(|f| (f.file.as_str(), f.size)).collect();
    Files::from_map(contents.into_iter().filter_map(|c| sizes.get(c).map(|&s| (c.to_string(), s))).collect())
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
    for (x, y) in tiles_meeting(rings, VIEWSHED_KM) {
        let key = format!("6/{x}/{y}");
        for l in VIEWSHED_LAYERS.iter().filter_map(|l| cat.layers.get(*l)) {
            if let Some(h) = l.hi.get(&key) {
                add(h);
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

/// Keeps the mirror (store::mirror) on its own thread: every minute, or as soon as the downloads
/// change, it lets go of what isn't downloaded and copies what is; a new catalog or a change of the
/// downloads has it plan again at once. Away from home, it only lets go. While the build runs (the
/// build Mac's heartbeat on the NAS says a job runs, as the copies' pause read it before
/// downloads), copies keep to 20 MB/s; while this Mac's own agent runs a job, nothing is deleted.
/// Between copies it works out the basemap pieces' sizes (for the panel's sizes), those of what's
/// downloaded first.
pub fn spawn_mirror(s: S) {
    let Some(m) = s.data.mirror.clone() else { return };
    let spawned = std::thread::Builder::new().name("mirror".into()).spawn(move || loop {
        if !once(&s, &m) {
            s.downloads.wait(Duration::from_secs(60));
        }
    });
    if let Err(e) = spawned {
        eprintln!("mirror: can't start its thread: {e}");
    }
}

/// One round of the mirror; whether to go again at once (the catalog or the downloads changed
/// meanwhile).
fn once(s: &crate::AppState, m: &Mirror) -> bool {
    let plan = s.downloads.plan(&s.data);
    let changed = || s.data.generation.load(Ordering::SeqCst) != plan.generation || s.downloads.version() != plan.version;
    let cat = s.data.catalog();
    // (Nothing known of the map yet: what's here can't be told from what isn't wanted.)
    if cat.n == 0 {
        return false;
    }
    let hold = || s.data.job_here();
    match (s.data.nas_root(), s.data.pool()) {
        (Some(root), Some(pool)) if pool.is_online() => {
            let wanted = plan.wanted.items.iter().filter_map(|i| match i {
                Item::Piece { archive, piece } => Some((archive.as_str(), *piece)),
                Item::File { .. } => None,
            });
            if let Err(e) = size_pieces(m, wanted, &root, &pool, &changed) {
                eprintln!("mirror: sizing the basemap's pieces: {e:#}");
            }
            let slow = || s.data.agent_busy();
            match m.sync(&cat, &plan.wanted, &root, &pool, &Control { stop: &changed, slow: &slow, hold: &hold }) {
                Ok(st) => {
                    if st.copied > 0 || st.removed > 0 || st.waiting > 0 || st.failed > 0 {
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
            let all = plan.world.pieces().chain(plan.regions.values().flat_map(Part::pieces)).chain(plan.views.iter().flat_map(Part::pieces));
            if let Err(e) = size_pieces(m, all, &root, &pool, &changed) {
                eprintln!("mirror: sizing the basemap's pieces: {e:#}");
            }
        }
        // Away from home: what isn't downloaded goes all the same.
        _ => match m.sync_away(&cat, &plan.wanted, &hold) {
            Ok(st) if st.removed > 0 => eprintln!("mirror (away): {st:?}"),
            Ok(_) => {}
            Err(e) => eprintln!("mirror: {e:#}"),
        },
    }
    changed()
}

/// Works out the sizes of these pieces not known yet, from their archives on the NAS (their
/// directories alone), until `stop()`.
fn size_pieces<'a>(m: &Mirror, pieces: impl Iterator<Item = (&'a str, Piece)>, root: &std::path::Path, pool: &Arc<store::IoPool>, stop: &dyn Fn() -> bool) -> Result<()> {
    let mut by: BTreeMap<&str, BTreeSet<Piece>> = BTreeMap::new();
    for (a, p) in pieces.filter(|(a, p)| m.piece_size(a, *p).is_none()) {
        by.entry(a).or_default().insert(p);
    }
    for (archive, ps) in by {
        let f = store::PooledFile::open(pool, &root.join(archive)).with_context(|| format!("open {archive}"))?;
        let pm = store::pmtiles::PmTiles::with_leaf_cache(Box::new(f), 512)?;
        let ps: Vec<Piece> = ps.into_iter().collect();
        let t = Instant::now();
        let n = m.size_pieces(archive, &ps, &pm, stop)?;
        if n > 10 {
            eprintln!("mirror: sized {n} pieces of {archive} in {:.1} s", t.elapsed().as_secs_f64());
        }
    }
    Ok(())
}

// ---- the state, for the panel ------------------------------------------------------------------

/// A download's state: all here, being copied, waiting its turn, waiting for room, or away.
fn part_state(part: &Part, m: &Mirror, copying: Option<&str>, more: u64, online: bool) -> &'static str {
    let (bytes, here, unknown) = part.tally(m);
    if unknown == 0 && here >= bytes {
        "done"
    } else if !online {
        "away"
    } else if copying.is_some_and(|c| part.items.iter().any(|i| store::mirror::copy_name(i) == c)) {
        "copying"
    } else if more > 0 {
        "room"
    } else {
        "queued"
    }
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn now() -> u64 {
    secs(SystemTime::now())
}

/// The downloads' state for the panel (`GET /api/downloads`, docs/formats.md).
pub fn status(s: &crate::AppState) -> Result<Value> {
    let Some(m) = s.data.mirror.as_ref() else {
        return Ok(json!({ "mirror": false, "online": s.data.online(), "regions": {}, "views": [] }));
    };
    let plan = s.downloads.plan(&s.data);
    let room = m.room(&plan.wanted)?;
    let copying = m.copying();
    let cname = copying.as_ref().map(|c| c.name.as_str());
    let online = s.data.online();
    let slow = online && s.data.agent_busy();
    let tally = |p: &Part| {
        let (bytes, here, unknown) = p.tally(m);
        json!({ "bytes": bytes, "here": here, "unknown": unknown })
    };
    let state = |p: &Part| part_state(p, m, cname, room.more, online);
    let on: HashSet<&str> = plan.asked.regions.iter().map(|r| r.id.as_str()).collect();
    let names: BTreeMap<String, String> = crate::regions::recorded_list(&s.data.catalog()).iter().filter_map(|r| Some((r["id"].as_str()?.to_string(), r["name"].as_str().unwrap_or_default().to_string()))).collect();
    let mut regions = serde_json::Map::new();
    for (id, p) in plan.regions.iter() {
        let mut v = tally(p);
        let downloaded = on.contains(id.as_str());
        v["name"] = json!(names.get(id).cloned().unwrap_or_default());
        v["on"] = json!(downloaded);
        v["state"] = json!(downloaded.then(|| state(p)));
        regions.insert(id.clone(), v);
    }
    // Downloaded regions the catalog doesn't have (not built yet, or removed since).
    for r in &plan.asked.regions {
        regions.entry(r.id.clone()).or_insert_with(|| json!({ "name": r.name, "bytes": 0, "here": 0, "unknown": 0, "on": true, "state": "missing" }));
    }
    let views: Vec<Value> = plan
        .asked
        .views
        .iter()
        .zip(&plan.views)
        .map(|(v, p)| {
            let mut o = tally(p);
            for (k, x) in [("id", json!(v.id)), ("name", json!(v.name)), ("outline", json!(v.outline)), ("at", json!(v.at)), ("state", json!(state(p)))] {
                o[k] = x;
            }
            o
        })
        .collect();
    let mut world = tally(&plan.world);
    world["on"] = json!(plan.asked.world.is_some());
    world["at"] = json!(plan.asked.world);
    world["state"] = json!(plan.asked.world.map(|_| state(&plan.world)));
    let wanted_here: u64 = plan.wanted.items.iter().map(|i| m.item_here(i)).sum();
    let last = m.last().map(|(st, at)| {
        json!({
            "at": secs(at), "copied": st.copied, "copied_bytes": st.copied_bytes, "removed": st.removed, "removed_bytes": st.removed_bytes,
            "waiting": st.waiting, "waiting_bytes": st.waiting_bytes, "failed": st.failed, "pending": st.pending,
            "end": match st.end { SyncEnd::Done => "done", SyncEnd::Paused => "paused", SyncEnd::Offline => "offline" },
        })
    });
    let what = |c: &store::mirror::Copying| -> String {
        if let Some(rest) = c.name.strip_prefix("basemap ") {
            return format!("the basemap's {}", rest.split(' ').nth(1).map_or("piece".into(), |p| if p == "lo" { "zooms 0–10".to_string() } else { format!("zooms 11–14 of {}", p.replacen('-', "/", 2)) }));
        }
        store::naming::parse_content_name(&c.name).map(|c| c.logical.to_string()).unwrap_or_default()
    };
    Ok(json!({
        "mirror": true,
        "online": online,
        "slow": slow,
        "rate": store::mirror::SLOW_RATE,
        "free": room.free,
        "reserve": room.reserve,
        "here": m.usage().1,
        "world": world,
        "wanted": { "bytes": room.missing + wanted_here, "here": wanted_here, "more": room.more, "unknown": room.unknown },
        "copying": copying.as_ref().map(|c| json!({ "what": what(c), "bytes": c.size, "have": c.have, "slow": c.slow })),
        "last": last,
        "regions": regions,
        "views": views,
    }))
}

/// What this Mac has downloaded, in brief, for the menu bar (`/api/build`'s `offline`): whether the
/// World is, how many regions and views, their bytes and how many of those are here, and whether
/// the NAS is reachable from here. Null without a mirror.
pub fn summary(s: &crate::AppState) -> Value {
    let Some(m) = s.data.mirror.as_ref() else { return Value::Null };
    let plan = s.downloads.plan(&s.data);
    let (mut bytes, mut here) = (0, 0);
    for i in &plan.wanted.items {
        if let Some(b) = m.item_size(i) {
            bytes += b;
            here += m.item_here(i).min(b);
        }
    }
    json!({ "world": plan.asked.world.is_some(), "areas": plan.asked.regions.len() + plan.asked.views.len(), "bytes": bytes, "here": here, "nas": s.data.online() })
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

/// Bytes in metric units, as the panel says them: 840 MB, 2.4 GB.
fn size(b: u64) -> String {
    if b < 1_000_000_000 {
        format!("{} MB", (b as f64 / 1e6).round())
    } else {
        format!("{:.1} GB", b as f64 / 1e9)
    }
}

/// What all that `next` downloads would still need on this Mac, and whether that fits the free
/// space above the reserve: (need, room above the reserve). The pieces' sizes are worked out from
/// the NAS first where they aren't known.
fn need(s: &crate::AppState, m: &Mirror, next: &Asked) -> Result<(u64, u64)> {
    let cur = s.downloads.plan(&s.data);
    let cat = s.data.catalog();
    let plan = make_plan(&cat, cur.generation, cur.version, next.clone(), cur.regions.clone());
    let unknown = plan.wanted.items.iter().any(|i| m.item_size(i).is_none());
    if unknown {
        let (Some(root), Some(pool)) = (s.data.nas_root(), s.data.pool().filter(|p| p.is_online())) else { bail!("The NAS isn't reachable, so this Mac can't download from it now") };
        let pieces = plan.wanted.items.iter().filter_map(|i| match i {
            Item::Piece { archive, piece } => Some((archive.as_str(), *piece)),
            Item::File { .. } => None,
        });
        size_pieces(m, pieces, &root, &pool, &|| false)?;
    }
    let room = m.room(&plan.wanted)?;
    Ok((room.missing, room.free.saturating_sub(room.reserve)))
}

/// Refuses `next` when it wouldn't fit, saying why with the numbers (`what` the download asked
/// for; the World named with it when it comes too).
fn check_fits(s: &crate::AppState, next: &Asked, what: &str) -> Result<()> {
    let Some(m) = s.data.mirror.as_ref() else { bail!("this server keeps no copy of the map (--no-mirror)") };
    let (need, room) = need(s, m, next)?;
    let before = s.downloads.asked();
    let with = if before.world.is_none() && next.any_area() { ", with the World, zoomed out," } else { "" };
    let already = if before.world.is_some() { ", with what's downloaded already" } else { "" };
    ensure!(
        need <= room,
        "Downloading {what}{with} takes {} more on this Mac{already}, and it has {} free above the reserve ({}): free some space, or remove a download",
        size(need),
        size(room),
        size(m.reserve())
    );
    Ok(())
}

#[derive(Deserialize)]
pub struct On {
    on: bool,
}

/// Downloads the World, zoomed out, or removes it (refused while a region or view is downloaded:
/// they need it away from the NAS).
pub async fn put_world(State(s): State<S>, Json(q): Json<On>) -> Response {
    blocking(s, move |s| {
        ensure!(s.data.mirror.is_some(), "this server keeps no copy of the map (--no-mirror)");
        let mut next = s.downloads.asked();
        if q.on {
            if next.world.is_none() {
                next.world = Some(now());
                check_fits(s, &next, "the World, zoomed out")?;
            }
        } else {
            ensure!(!next.any_area(), "Remove the downloaded regions and views first: away from the NAS they need the World, zoomed out");
            next.world = None;
        }
        s.downloads.edit(|a| {
            *a = next;
            Ok(())
        })?;
        Ok(json!({ "ok": true }))
    })
    .await
}

/// Downloads a region (and the World with it), or removes it. A region the catalog doesn't have yet
/// can be downloaded: its files come once it's built.
pub async fn put_region(State(s): State<S>, Path(id): Path<String>, Json(q): Json<On>) -> Response {
    blocking(s, move |s| {
        ensure!(pipeline::agent::recipes::valid_id(&id), "{id:?} isn't a region id");
        ensure!(s.data.mirror.is_some(), "this server keeps no copy of the map (--no-mirror)");
        let cat = s.data.catalog();
        let name = crate::regions::recorded_list(&cat).iter().find(|r| r["id"] == id.as_str()).and_then(|r| r["name"].as_str().map(str::to_string)).unwrap_or_else(|| id.clone());
        let mut next = s.downloads.asked();
        let was = next.regions.iter().any(|r| r.id == id);
        next.regions.retain(|r| r.id != id);
        if q.on {
            next.regions.push(AskedRegion { id: id.clone(), name: name.clone(), at: now() });
            next.world.get_or_insert_with(now);
            if !was {
                check_fits(s, &next, &format!("“{name}”"))?;
            }
        }
        s.downloads.edit(|a| {
            *a = next;
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

fn check_outline(outline: &[[f64; 2]]) -> Result<()> {
    ensure!((3..=MAX_POINTS).contains(&outline.len()), "an outline of 3 to {MAX_POINTS} points");
    ensure!(outline.iter().all(|p| p[0].is_finite() && p[1].is_finite() && p[0].abs() <= 540.0 && p[1].abs() <= 90.0), "points are [lon, lat] degrees");
    Ok(())
}

/// The downloads with the view `outline` added (and the World).
fn with_view(s: &crate::AppState, outline: &[[f64; 2]], name: &str) -> Asked {
    let mut next = s.downloads.asked();
    next.world.get_or_insert_with(now);
    next.views.push(AskedView { id: String::new(), name: name.to_string(), outline: outline.to_vec(), at: now() });
    next
}

#[derive(Deserialize)]
pub struct Outline {
    outline: Vec<[f64; 2]>,
}

/// What downloading the view `outline` would take: its own files and pieces (`bytes`, `here` of
/// them, and `with_world`, the World's still missing when it isn't downloaded), all that would
/// still be copied with it (`need`), and the free space above the reserve (`room`).
pub async fn post_view_size(State(s): State<S>, Json(o): Json<Outline>) -> Response {
    blocking(s, move |s| {
        check_outline(&o.outline)?;
        let Some(m) = s.data.mirror.as_ref() else { bail!("this server keeps no copy of the map (--no-mirror)") };
        let next = with_view(s, &o.outline, "");
        let (need, room) = need(s, m, &next)?;
        let own = area_part(&s.data.catalog(), std::slice::from_ref(&o.outline));
        let (bytes, here, _) = own.tally(m);
        let plan = s.downloads.plan(&s.data);
        let world = if plan.asked.world.is_none() {
            let (b, h, _) = plan.world.tally(m);
            b - h.min(b)
        } else {
            0
        };
        Ok(json!({ "bytes": bytes, "here": here, "with_world": world, "need": need, "room": room, "fits": need <= room }))
    })
    .await
}

/// Downloads an area (the ground in view): named as given, else after the most important place in
/// it that the place search knows (waiting a few seconds for its index while it's made), else by
/// where it is. Refused, with why, when it wouldn't fit.
pub async fn post_view(State(s): State<S>, Json(v): Json<NewView>) -> Response {
    blocking(s, move |s| {
        check_outline(&v.outline)?;
        let name = match v.name.as_deref() {
            Some(n) => clean_name(n)?,
            None => name_of(s, &v.outline),
        };
        check_fits(s, &with_view(s, &v.outline, &name), &format!("“{name}”"))?;
        let at = now();
        let id = s.downloads.edit(|p| {
            let mut id = format!("v{at}");
            let mut k = 2;
            while p.views.iter().any(|x| x.id == id) {
                id = format!("v{at}-{k}");
                k += 1;
            }
            p.world.get_or_insert(at);
            p.views.push(AskedView { id: id.clone(), name: name.clone(), outline: v.outline.clone(), at });
            Ok(id)
        })?;
        Ok(json!({ "id": id, "name": name }))
    })
    .await
}

/// A view's name: the most important place in it the place search knows, else where it is (the
/// middle of its box).
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
        s.downloads.edit(|p| match p.views.iter_mut().find(|v| v.id == id) {
            Some(v) => {
                v.name = name.clone();
                Ok(())
            }
            None => bail!("no downloaded view {id}"),
        })?;
        Ok(json!({ "ok": true, "name": name }))
    })
    .await
}

pub async fn delete_view(State(s): State<S>, Path(id): Path<String>) -> Response {
    blocking(s, move |s| {
        s.downloads.edit(|p| {
            let before = p.views.len();
            p.views.retain(|v| v.id != id);
            ensure!(p.views.len() < before, "no downloaded view {id}");
            Ok(())
        })?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[cfg(test)]
mod tests;
