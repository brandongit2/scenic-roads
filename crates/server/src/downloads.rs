//! Downloads (docs/macs.md, "Mirror, per Mac"; docs/surface.md, the Regions panel): what the owner has
//! downloaded to this Mac for when it's away from the NAS. Nothing else is copied here, and nothing
//! downloaded goes until the owner removes it (store::mirror).
//!
//!   GET    /api/downloads                 this Mac's room, the World download, every region the
//!                                         catalog has (a region still building with its `tiles`
//!                                         built of total) with its size, how much of it is here
//!                                         and whether it's downloaded, the copy under way
//!   PUT    /api/downloads/world           {on}: download the World, zoomed out, or remove it
//!   PUT    /api/downloads/regions/{id}    {on}: download a region, or remove it (with its
//!                                         terrain detail)
//!   PUT    /api/downloads/regions/{id}/detail {on}: download a downloaded region's terrain detail,
//!                                         or remove it
//!   PUT    /api/downloads/reserve         {gb}: the free space downloads leave on the disk
//!                                         (crate::reserve)
//!
//! A download that wouldn't fit (all that's downloaded, less what's here, more than the free space
//! above the reserve) is refused, with the numbers. What's downloaded is this Mac's alone:
//! `<home>/downloads.json` (docs/formats.md), never the NAS.
//!
//! - **The World, zoomed out** (`world_part`): the worldwide files, every layer's root and lo
//!   packs, the landmark points and area details (store::mirror::essentials), and the basemap's
//!   zooms 0–10 (its `lo` piece, store::pieces). Without it the map works only while the NAS is
//!   reachable. It's downloaded on its own: a region doesn't bring it, and it can be removed while
//!   regions are downloaded (away from the NAS, zoomed out, the map then shows nothing: the panel
//!   warns of it).
//! - **A region** (`area_part`): every layer's hi pack (zooms
//!   9–14), and the base pack and road values, of each z6 tile within 2 km of it (its outlines are
//!   simplified), and the basemap's zooms 11–14 over those tiles; the terrain's and the grids' hi
//!   packs within 25 km (the viewshed's reach, so one from inside works offline); and the hi data
//!   of the z6 tiles within 50 km (what the lists of a view read around it); and what the map reads
//!   zoomed in from the World's files (pipeline::areafiles::files_of), so only zoomed out needs it.
//! - **A region's terrain detail** (`detail_of`), an add-on to its download: the terrain's deep
//!   packs (pipeline::terrain_deep, z13 and finer, by z10 tile) within 2 km of it. They cover only
//!   parts of a region; its terrain to z12 (the hi packs) is the rest and every zoom to z12, so it
//!   needs the region: downloaded only while the region is. The server makes the slope, and the map the hill-shading and contours,
//!   from them as from any terrain tile (crate::terrain). Copied after every other download.

use crate::data::Data;
use crate::S;
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use pipeline::areafiles::{detail_of, files_of, outer_rings, tiles_meeting, Files, NEAR_KM};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use store::catalog::Catalog;
use store::mirror::{Control, Item, Mirror, SyncEnd, Wanted};
use store::pieces::Piece;

const FILE: &str = "downloads.json";
/// `downloads.json`'s format: 3 since downloaded views went (2 had `views`).
const FMT: u32 = 3;

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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AskedRegion {
    pub id: String,
    pub name: String,
    /// When it was downloaded (seconds since 1970).
    pub at: u64,
    /// Its terrain detail downloaded too (`detail_of`). (`fine` in downloads.json files from before
    /// one terrain, task #633.)
    #[serde(default, alias = "fine")]
    pub detail: bool,
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
        let by_content: std::collections::HashMap<&str, &str> =
            cat.files.iter().map(|(l, f)| (f.file.as_str(), l.as_str())).collect();
        let mut logicals: Vec<&str> =
            files.names.iter().filter_map(|(n, _)| by_content.get(n.as_str()).copied()).collect();
        store::mirror::copy_order(cat, &mut logicals);
        let mut items: Vec<Item> = logicals
            .iter()
            .filter_map(|l| cat.files.get(*l))
            .map(|f| Item::File { name: f.file.clone(), size: f.size })
            .collect();
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

/// What a region's downloads copy: the region, and its terrain detail (an add-on), with the share
/// of the region the detail covers (per cent).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionParts {
    pub coarse: Part,
    pub detail: Part,
    pub cover: f64,
}

/// Every region the catalog records, what downloading it copies, by id.
type Regions = Arc<BTreeMap<String, RegionParts>>;

/// What the mirror copies for one catalog and one set of downloads.
pub struct Plan {
    /// The catalog generation and the downloads' version it was made for.
    generation: u64,
    version: u64,
    /// What's downloaded, in copy order: the World, then the regions as they were asked, then their
    /// terrain detail.
    pub wanted: Wanted,
    pub world: Part,
    /// Every region the catalog records (for its size, downloaded or not).
    pub regions: Regions,
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
    /// `downloads.json.bad`. One from before views went (`fmt` 2) is written again without them:
    /// their files no region needs go at the mirror's next round.
    pub fn load(home: &std::path::Path) -> Arc<Downloads> {
        let path = home.join(FILE);
        let (asked, old) = match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice::<Asked>(&b) {
                Ok(a) => {
                    let views = serde_json::from_slice::<Value>(&b)
                        .ok()
                        .and_then(|v| v["views"].as_array().map(Vec::len))
                        .unwrap_or(0);
                    if views > 0 {
                        eprintln!("downloads: the downloaded views ({views}) are removed: views went in fmt {FMT}");
                    }
                    let old = a.fmt < FMT;
                    (a, old)
                }
                Err(e) => {
                    eprintln!("downloads: {} doesn't read ({e}); set aside as {FILE}.bad", path.display());
                    let _ = std::fs::rename(&path, home.join(format!("{FILE}.bad")));
                    (Asked::default(), false)
                }
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("downloads: can't read {}: {e}", path.display());
                }
                (Asked::default(), false)
            }
        };
        let d = Arc::new(Downloads {
            path,
            asked: Mutex::new(asked.clone()),
            version: AtomicU64::new(1),
            plan: Mutex::new(None),
            regions: Mutex::new(None),
            wake: (Mutex::new(false), Condvar::new()),
        });
        if old {
            if let Err(e) = d.edit(|_| Ok(())) {
                eprintln!("downloads: {e:#}");
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
        next.fmt = FMT;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&next)?).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("rename {}", tmp.display()))?;
        *g = next;
        self.version.fetch_add(1, Ordering::SeqCst);
        drop(g);
        self.poke();
        Ok(out)
    }

    /// Wakes the mirror thread for another round (the reserve changed).
    pub fn poke(&self) {
        let (lock, cv) = &self.wake;
        *lock.lock().unwrap() = true;
        cv.notify_all();
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
                    let r: Regions = Arc::new(
                        crate::regions::recorded(&cat)
                            .iter()
                            .map(|r| (r.id.clone(), region_parts(&cat, &outer_rings(r))))
                            .collect(),
                    );
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
    let mut wanted = Wanted::default();
    let mut seen = HashSet::new();
    let parts = asked
        .world
        .is_some()
        .then_some(&world)
        .into_iter()
        .chain(asked.regions.iter().filter_map(|r| regions.get(&r.id).map(|p| &p.coarse)))
        .chain(asked.regions.iter().filter(|r| r.detail).filter_map(|r| regions.get(&r.id).map(|p| &p.detail)));
    for p in parts {
        for i in &p.items {
            wanted.push(i.clone(), &mut seen);
        }
    }
    Plan { generation, version, wanted, world, regions, asked }
}

/// The World, zoomed out: the essentials and the basemap's zooms 0–10.
fn world_part(cat: &Catalog) -> Part {
    let ess = store::mirror::essentials(cat);
    let files = of_contents(cat, ess.iter().map(String::as_str));
    Part::new(cat, files, &basemap_archives(cat), &[Piece::Lo])
}

/// A region's download (`rings`, each read as a filled area): its files, then its basemap pieces.
fn area_part(cat: &Catalog, rings: &[Vec<[f64; 2]>]) -> Part {
    let pieces: Vec<Piece> = tiles_meeting(rings, NEAR_KM).into_iter().map(|(x, y)| Piece::Tile(x, y)).collect();
    Part::new(cat, files_of(cat, rings), &basemap_archives(cat), &pieces)
}

/// A region's downloads (`rings`, its outer rings): the region, and its terrain detail.
fn region_parts(cat: &Catalog, rings: &[Vec<[f64; 2]>]) -> RegionParts {
    let (files, cover) = detail_of(cat, rings);
    RegionParts { coarse: area_part(cat, rings), detail: Part::new(cat, files, &[], &[]), cover }
}

fn basemap_archives(cat: &Catalog) -> Vec<String> {
    cat.basemap.iter().filter_map(|l| cat.content(l)).map(str::to_string).collect()
}

/// The catalog's files of these content names.
fn of_contents<'a>(cat: &Catalog, contents: impl IntoIterator<Item = &'a str>) -> Files {
    let sizes: std::collections::HashMap<&str, u64> = cat.files.values().map(|f| (f.file.as_str(), f.size)).collect();
    Files::from_map(contents.into_iter().filter_map(|c| sizes.get(c).map(|&s| (c.to_string(), s))).collect())
}

// ---- the mirror thread --------------------------------------------------------------------------

/// Keeps the mirror (store::mirror) on its own thread: every minute, or as soon as the downloads
/// change, it lets go of what isn't downloaded and copies what is; a new catalog or a change of the
/// downloads has it plan again at once. Away from home, it only lets go. While the build runs (the
/// lead's heartbeat on the NAS says a job runs, as the copies' pause read it before
/// downloads), copies keep to 20 MB/s; a file this Mac's pack or lo job holds isn't deleted
/// (store::mirror).
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

/// The mirror's syncs said in publishing's log, per process.
static MIRROR_MARKS: std::sync::Mutex<pipeline::pubtimes::MirrorMarks> =
    std::sync::Mutex::new(pipeline::pubtimes::MirrorMarks::new());

/// One round of the mirror; whether to go again at once (the catalog or the downloads changed
/// meanwhile).
fn once(s: &crate::AppState, m: &Mirror) -> bool {
    let plan = s.downloads.plan(&s.data);
    let changed =
        || s.data.generation.load(Ordering::SeqCst) != plan.generation || s.downloads.version() != plan.version;
    let cat = s.data.catalog();
    // (Nothing known of the map yet: what's here can't be told from what isn't wanted.)
    if cat.n == 0 {
        return false;
    }
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
            // (Each catalog's sync begun and through, once, in publishing's log: pipeline::pubtimes.)
            let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            if MIRROR_MARKS.lock().unwrap().start(cat.n, now()) {
                crate::data::publish_event(&root, "mirror-start", cat.n, None);
            }
            match m.sync(&cat, &plan.wanted, &root, &pool, &Control { stop: &changed, slow: &slow }) {
                Ok(st) => {
                    let through =
                        st.end == store::mirror::SyncEnd::Done && st.waiting == 0 && st.failed == 0 && st.pending == 0;
                    if let Some(secs) = MIRROR_MARKS.lock().unwrap().end(cat.n, now(), through) {
                        crate::data::publish_event(&root, "mirror-end", cat.n, Some(secs));
                    }
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
            let all = plan.world.pieces().chain(plan.regions.values().flat_map(|r| r.coarse.pieces()));
            if let Err(e) = size_pieces(m, all, &root, &pool, &changed) {
                eprintln!("mirror: sizing the basemap's pieces: {e:#}");
            }
        }
        // Away from home: what isn't downloaded goes all the same.
        _ => match m.sync_away(&cat, &plan.wanted) {
            Ok(st) if st.removed > 0 => eprintln!("mirror (away): {st:?}"),
            Ok(_) => {}
            Err(e) => eprintln!("mirror: {e:#}"),
        },
    }
    changed()
}

/// Works out the sizes of these pieces not known yet, from their archives on the NAS (their
/// directories alone), until `stop()`.
fn size_pieces<'a>(
    m: &Mirror,
    pieces: impl Iterator<Item = (&'a str, Piece)>,
    root: &std::path::Path,
    pool: &Arc<store::IoPool>,
    stop: &dyn Fn() -> bool,
) -> Result<()> {
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
        return Ok(json!({ "mirror": false, "online": s.data.online(), "regions": {} }));
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
    let listed: BTreeMap<String, Value> = crate::regions::recorded_list(&s.data.catalog())
        .into_iter()
        .filter_map(|r| Some((r["id"].as_str()?.to_string(), r)))
        .collect();
    let mut regions = serde_json::Map::new();
    for (id, rp) in plan.regions.iter() {
        let p = &rp.coarse;
        let mut v = tally(p);
        let downloaded = on.contains(id.as_str());
        // (Its terrain detail, where it has any: its size, the share of the region it covers.)
        let detail_on = plan.asked.regions.iter().any(|r| r.id == *id && r.detail);
        if !rp.detail.items.is_empty() || detail_on {
            let mut f = tally(&rp.detail);
            f["cover"] = json!((rp.cover * 10.0).round() / 10.0);
            f["on"] = json!(detail_on);
            f["state"] = json!(detail_on.then(|| state(&rp.detail)));
            v["detail"] = f;
        }
        let r = listed.get(id);
        v["name"] = json!(r.and_then(|r| r["name"].as_str()).unwrap_or_default());
        // (Still building: its tiles built of total; its files grow with each catalog.)
        if let Some(t) = r.and_then(|r| r.get("tiles")) {
            v["tiles"] = t.clone();
        }
        v["on"] = json!(downloaded);
        v["state"] = json!(downloaded.then(|| state(p)));
        regions.insert(id.clone(), v);
    }
    // Downloaded regions the catalog doesn't have (not built yet, or removed since).
    for r in &plan.asked.regions {
        regions.entry(r.id.clone()).or_insert_with(
            || json!({ "name": r.name, "bytes": 0, "here": 0, "unknown": 0, "on": true, "state": "missing" }),
        );
    }
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
            return format!(
                "the basemap's {}",
                rest.split(' ').nth(1).map_or("piece".into(), |p| if p == "lo" {
                    "zooms 0–10".to_string()
                } else {
                    format!("zooms 11–14 of {}", p.replacen('-', "/", 2))
                })
            );
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
        "reserve_set": m.reserve_set(),
        "reserve_min": crate::reserve::MIN_GB * 1_000_000_000,
        "disk": store::sys::disk_size(m.root()).ok(),
        "here": m.usage().1,
        "world": world,
        "wanted": { "bytes": room.missing + wanted_here, "here": wanted_here, "more": room.more, "unknown": room.unknown },
        "copying": copying.as_ref().map(|c| json!({ "what": what(c), "bytes": c.size, "have": c.have, "slow": c.slow })),
        "last": last,
        "regions": regions,
    }))
}

/// What this Mac has downloaded, in brief, for the menu bar (`/api/build`'s `offline`): whether the
/// World is, how many regions, their bytes and how many of those are here, and whether
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
    json!({ "world": plan.asked.world.is_some(), "regions": plan.asked.regions.len(), "bytes": bytes, "here": here, "nas": s.data.online() })
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
        let (Some(root), Some(pool)) = (s.data.nas_root(), s.data.pool().filter(|p| p.is_online())) else {
            bail!("The NAS isn't reachable, so this Mac can't download from it now")
        };
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
/// for).
fn check_fits(s: &crate::AppState, next: &Asked, what: &str) -> Result<()> {
    let Some(m) = s.data.mirror.as_ref() else { bail!("this server keeps no copy of the map (--no-mirror)") };
    let (need, room) = need(s, m, next)?;
    let already = if s.downloads.asked() != Asked::default() { ", with what's downloaded already" } else { "" };
    ensure!(
        need <= room,
        "Downloading {what} takes {} more on this Mac{already}, and it has {} free above the reserve ({}): free some space, or remove a download",
        size(need),
        size(room),
        size(m.reserve())
    );
    Ok(())
}

#[derive(Deserialize)]
pub struct Reserve {
    gb: u64,
}

/// Sets the reserve to `gb` on a disk of `disk_bytes`: kept in the home, the mirror told, and its
/// thread woken (downloads waiting for room may start; raised, nothing is deleted).
fn set_reserve(s: &crate::AppState, gb: u64, disk_bytes: u64) -> Result<Value> {
    let Some(m) = s.data.mirror.as_ref() else { bail!("this server keeps no copy of the map (--no-mirror)") };
    s.reserve.set(gb, disk_bytes, m)?;
    s.downloads.poke();
    Ok(json!({ "ok": true, "reserve": m.reserve(), "reserve_set": m.reserve_set() }))
}

/// Sets the reserve (crate::reserve), bounded by the size of the mirror's disk.
pub async fn put_reserve(State(s): State<S>, Json(r): Json<Reserve>) -> Response {
    blocking(s, move |s| {
        let Some(m) = s.data.mirror.as_ref() else { bail!("this server keeps no copy of the map (--no-mirror)") };
        let disk = store::sys::disk_size(m.root()).context("the disk's size")?;
        set_reserve(s, r.gb, disk)
    })
    .await
}

#[derive(Deserialize)]
pub struct On {
    on: bool,
}

/// Downloads the World, zoomed out, or removes it.
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

/// Downloads a region, or removes it (with its terrain detail). A region the catalog doesn't have yet
/// can be downloaded: its files come once a catalog has it. One still building (its `tiles`) is
/// downloaded as its tiles come: its files are those of the catalog served, more with each catalog
/// (what's here already never fetched again), copied at the slow rate while the build runs.
pub async fn put_region(State(s): State<S>, Path(id): Path<String>, Json(q): Json<On>) -> Response {
    blocking(s, move |s| {
        ensure!(pipeline::agent::recipes::valid_id(&id), "{id:?} isn't a region id");
        ensure!(s.data.mirror.is_some(), "this server keeps no copy of the map (--no-mirror)");
        ensure!(
            !(q.on && crate::regions::is_disabled(s, &id)),
            "{id:?} is disabled in its file (disabled = true): nothing is built for it to download"
        );
        let cat = s.data.catalog();
        let name = crate::regions::recorded_list(&cat)
            .iter()
            .find(|r| r["id"] == id.as_str())
            .and_then(|r| r["name"].as_str().map(str::to_string))
            .unwrap_or_else(|| id.clone());
        let mut next = s.downloads.asked();
        let was = next.regions.iter().find(|r| r.id == id).cloned();
        next.regions.retain(|r| r.id != id);
        if q.on {
            let detail = was.as_ref().is_some_and(|r| r.detail);
            next.regions.push(AskedRegion { id: id.clone(), name: name.clone(), at: now(), detail });
            if was.is_none() {
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

/// Downloads a region's terrain detail (refused unless the region is downloaded: it needs it), or
/// removes it (the region stays).
pub async fn put_region_detail(State(s): State<S>, Path(id): Path<String>, Json(q): Json<On>) -> Response {
    blocking(s, move |s| {
        ensure!(pipeline::agent::recipes::valid_id(&id), "{id:?} isn't a region id");
        ensure!(s.data.mirror.is_some(), "this server keeps no copy of the map (--no-mirror)");
        let mut next = s.downloads.asked();
        let at = next.regions.iter().position(|r| r.id == id);
        if q.on {
            let Some(i) = at else { bail!("Download the region first: its terrain detail needs it") };
            let plan = s.downloads.plan(&s.data);
            let has = plan.regions.get(&id).is_some_and(|p| !p.detail.items.is_empty());
            ensure!(has, "{id:?} has no terrain detail on the map");
            if !std::mem::replace(&mut next.regions[i].detail, true) {
                let name = next.regions[i].name.clone();
                check_fits(s, &next, &format!("the terrain detail of “{name}”"))?;
            }
        } else if let Some(i) = at {
            next.regions[i].detail = false;
        }
        s.downloads.edit(|a| {
            *a = next;
            Ok(())
        })?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[cfg(test)]
mod tests;
