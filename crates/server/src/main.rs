//! The map's server: tiles, layers and the APIs over the catalog's data (docs/plan.md §4).
//!
//! usage: server [--port 8080] [--web web/dist] [--fonts data/fonts] [--home <dir>] [--root <dir>]
//!               [--no-mirror | --mirror] [--reserve-gb 50] [--listen <IPv4 address>]
//!
//! Data comes from the NAS project folder (found and mounted by itself), read from this Mac's mirror
//! when it's there. `--root` serves a local folder laid out like the project folder instead
//! (development, tests), without a mirror unless `--mirror` (then copied into `--home`'s as from
//! the NAS: a `--home` of its own, never this Mac's app folder). `--reserve-gb`: the free space the
//! mirror leaves on the disk, in GB (10⁹ bytes). `--listen`: the one address to answer on (a test
//! server's 127.0.0.1), else every IPv4 address and IPv6's loopback. Nothing is loaded at start:
//! the catalog says where everything is, and files are opened on first use.

mod cache;
mod data;
mod descriptions;
mod details;
mod keep;
mod livefolder;
mod names_live;
mod ovdata;
mod marks;
mod markview;
mod pages;
mod places;
mod query;
mod regions;
mod remote;
mod terrain;
mod tiles;
mod updater;
mod views;
mod viewshed;
mod ways;

use anyhow::Result;
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use roadcore::E7;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

pub struct AppState {
    pub data: Arc<data::Data>,
    pub updater: Arc<updater::Updater>,
    pub names: Arc<names_live::NamesState>,
    pub basemap: tiles::Basemap,
    /// Overlay and layer files gzipped once (cache.rs), by content name.
    pub packs: Arc<cache::Packs>,
    /// Details for hover and popups, loaded on first use per catalog.
    details: Mutex<Option<(u64, Arc<details::Details>)>>,
    /// Roads' English names from OSM (name:en), by OSM way id, per catalog.
    road_en: Mutex<Option<(u64, Arc<HashMap<u64, String>>)>>,
    /// Trains a day per rail way (OSM id), sorted, per catalog.
    rail_freq: Mutex<Option<(u64, Arc<Vec<(u32, f32)>>)>>,
    /// The build agent's heartbeat (state/status.json), re-read at most every 30 s.
    agent: Mutex<Option<(std::time::Instant, serde_json::Value)>>,
    /// This Mac's app folder (`~/Library/Application Support/scenic`): its own agent's status.
    home: PathBuf,
    /// The outlines of the latest OSM pass (the Regions panel).
    pub areas: regions::Areas,
    /// The user's descriptions, laid over popup details.
    pub descriptions: Arc<descriptions::Descriptions>,
    /// The map's places, for its search (made when first searched, again when the labels or the
    /// translations change).
    pub places: Arc<places::Index>,
    /// The areas this Mac keeps for offline use, and what the mirror keeps for them.
    pub keep: Arc<keep::Keep>,
    /// The current version tokens of the app's URLs, per (catalog generation, translations version).
    tokens: Mutex<Option<((u64, u64), Arc<std::collections::HashSet<String>>)>>,
}

pub type S = Arc<AppState>;

impl AppState {
    fn generation(&self) -> u64 {
        self.data.generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A global file, read whole: None when the catalog has none. A failed read (the NAS away) is
    /// logged and noted in `failed`, so what's built from it isn't kept.
    fn global_or_note(&self, logical: &str, failed: &std::cell::Cell<bool>) -> Option<Arc<Vec<u8>>> {
        match self.data.global(logical) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{logical}: {e:#}");
                failed.set(true);
                None
            }
        }
    }

    /// OSM's English name of a road (name:en), or "": today's converted table and each built unit's
    /// own (`global/roaden/<u>`, which win: they're newer).
    pub fn road_en(&self, id: u64) -> String {
        let g = self.generation();
        let mut cur = self.road_en.lock().unwrap();
        if cur.as_ref().is_none_or(|(gg, _)| *gg != g) {
            let failed = std::cell::Cell::new(false);
            let units: Vec<String> = self.data.catalog().files.keys().filter(|l| l.starts_with("global/roaden/")).cloned().collect();
            let mut map: HashMap<u64, String> = HashMap::new();
            for l in std::iter::once("global/legacy/road-en".to_string()).chain(units) {
                let part = self.global_or_note(&l, &failed).and_then(|b| serde_json::from_slice::<HashMap<String, String>>(&b).ok()).unwrap_or_default();
                map.extend(part.into_iter().filter_map(|(k, v)| k.parse().ok().map(|k| (k, v))));
            }
            if failed.get() {
                return map.get(&id).cloned().unwrap_or_default();
            }
            *cur = Some((g, Arc::new(map)));
        }
        cur.as_ref().and_then(|(_, m)| m.get(&id).cloned()).unwrap_or_default()
    }

    /// Trains a day each way on a rail way (0 when unknown).
    pub fn rail_freq(&self, id: u64) -> f32 {
        query::freq_in(&self.rail_freqs(), id)
    }

    /// Trains a day per rail way (global/railfreq: (way id as u32, trains) sorted by way), for
    /// lookups without the lock; empty when it couldn't be read (asked again next time).
    pub fn rail_freqs(&self) -> Arc<Vec<(u32, f32)>> {
        let g = self.generation();
        let mut cur = self.rail_freq.lock().unwrap();
        if cur.as_ref().is_none_or(|(gg, _)| *gg != g) {
            let failed = std::cell::Cell::new(false);
            let v: Vec<(u32, f32)> = self
                .global_or_note("global/railfreq", &failed)
                .map(|b| b.chunks_exact(8).map(|c| (u32::from_le_bytes(c[..4].try_into().unwrap()), f32::from_le_bytes(c[4..].try_into().unwrap()).abs())).collect())
                .unwrap_or_default();
            if failed.get() {
                return Arc::new(Vec::new());
            }
            *cur = Some((g, Arc::new(v)));
        }
        cur.as_ref().unwrap().1.clone()
    }

    /// The details behind popups, kept per catalog once read in full; None when a file couldn't
    /// be read (the NAS away).
    pub fn details(&self) -> Option<Arc<details::Details>> {
        let g = self.generation();
        let mut cur = self.details.lock().unwrap();
        if let Some((gg, d)) = cur.as_ref() {
            if *gg == g {
                return Some(d.clone());
            }
        }
        let failed = std::cell::Cell::new(false);
        let d = Arc::new(details::Details::load(&|name: &str| self.global_or_note(&format!("global/legacy/{name}"), &failed)));
        if failed.get() {
            return None;
        }
        *cur = Some((g, d.clone()));
        Some(d)
    }

    /// The version tokens meta gives out now (what the app puts in `?v=`).
    fn current_tokens(&self) -> Arc<std::collections::HashSet<String>> {
        let key = (self.generation(), self.names.version_all());
        if let Some((k, t)) = self.tokens.lock().unwrap().as_ref() {
            if *k == key {
                return t.clone();
            }
        }
        let meta = meta_json(self);
        let t: std::collections::HashSet<String> = meta
            .get("versions")
            .and_then(|v| v.as_object())
            .map(|m| m.values().filter_map(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string()))).collect())
            .unwrap_or_default();
        let t = Arc::new(t);
        *self.tokens.lock().unwrap() = Some((key, t.clone()));
        t
    }

    /// The build agent's heartbeat, as it wrote it (null when there's none or the NAS is away).
    fn agent_status(&self) -> serde_json::Value {
        let mut cur = self.agent.lock().unwrap();
        if let Some((t, v)) = cur.as_ref() {
            if t.elapsed() < std::time::Duration::from_secs(30) {
                return v.clone();
            }
        }
        let v = match (self.data.nas_root(), self.data.pool()) {
            (Some(root), Some(pool)) if pool.is_online() => pool.read_all(&root.join("state/status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };
        *cur = Some((std::time::Instant::now(), v.clone()));
        v
    }

    /// The build agent's status for the menu bar (tools/status): this Mac's own agent's when it runs
    /// here (written every few seconds), else the heartbeat it copies to the NAS (on change, and
    /// every five minutes); whether it's this Mac's; and, for this Mac's, the running job's log.
    fn build_status(&self) -> serde_json::Value {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let own: Option<serde_json::Value> = std::fs::read(self.home.join("agent/status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let fresh = own.as_ref().and_then(|v| v["beat"].as_u64()).is_some_and(|b| now.saturating_sub(b) < 120);
        let (mut status, local) = match own {
            Some(v) if fresh => (v, true),
            _ => (self.agent_status(), false),
        };
        // A helper on this Mac (docs/plan.md §8, Two Macs): its own status, fresh, in place of what
        // the build Mac last read of it (which stops while the build Mac sleeps).
        let helper: Option<serde_json::Value> = std::fs::read(self.home.join("agent/helper.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        if let (Some(h), Some(obj)) = (helper.filter(|h| h["beat"].as_u64().is_some_and(|b| now.saturating_sub(b) < 120)), status.as_object_mut()) {
            let host = h["host"].clone();
            let mut list: Vec<serde_json::Value> = obj.get("helpers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            list.retain(|x| x["host"] != host);
            list.push(serde_json::json!({"host": host, "beat": h["beat"], "job": h["job"], "pause": h["pause"], "here": true}));
            obj.insert("helpers".into(), serde_json::Value::Array(list));
        }
        let log = if local {
            status["job"]["id"].as_str().map(|id| self.home.join("agent/logs").join(format!("{}.log", id.replace([' ', '/'], "-"))).display().to_string())
        } else {
            None
        };
        // This Mac's ask to pause or go on, while its agent hasn't taken it up (pipeline::control).
        let asked = pipeline::control::peek_request(&self.home.join("agent")).map(|r| serde_json::json!({ "pause": r.pause.is_some(), "at": r.at }));
        serde_json::json!({"status": status, "local": local, "now": now, "log": log, "asked": asked})
    }

    /// Lets go of everything mapped from files the mirror has just evicted (`names`, content
    /// names), so the disk gets their room back.
    fn forget_evicted(&self, names: &[String]) {
        self.data.forget_evicted(names);
        self.basemap.forget(names);
        self.areas.forget(names);
    }

    /// Whether the details, rail frequencies and roads' English names of this catalog are loaded.
    fn loaded(&self) -> bool {
        let g = self.generation();
        let is = |x: bool| x;
        is(self.details.lock().unwrap().as_ref().is_some_and(|(gg, _)| *gg == g))
            && self.rail_freq.lock().unwrap().as_ref().is_some_and(|(gg, _)| *gg == g)
            && self.road_en.lock().unwrap().as_ref().is_some_and(|(gg, _)| *gg == g)
    }
}

/// A server for tests over `root`, a local folder laid out like the NAS project folder (as with
/// `--root`: the real NAS is never looked for), with this Mac's files in `home`. Nothing runs in
/// the background.
#[cfg(test)]
pub fn test_state(home: &std::path::Path, root: &std::path::Path) -> S {
    test_state_with(home, root, false)
}

/// `test_state`, with a mirror in `home` when `mirror` (nothing copies into it unless a test
/// syncs it).
#[cfg(test)]
pub fn test_state_with(home: &std::path::Path, root: &std::path::Path, mirror: bool) -> S {
    test_state_from(home, data::Data::open(data::Options { home: home.to_owned(), nas_root: Some(root.to_owned()), mirror, reserve: 0 }).unwrap())
}

/// A server for tests over `data`, with this Mac's files in `home`.
#[cfg(test)]
pub fn test_state_from(home: &std::path::Path, data: Arc<data::Data>) -> S {
    Arc::new(AppState {
        updater: updater::Updater::new(home),
        data,
        names: names_live::NamesState::new(home),
        basemap: tiles::Basemap::default(),
        packs: Arc::new(cache::Packs::default()),
        details: Mutex::new(None),
        road_en: Mutex::new(None),
        rail_freq: Mutex::new(None),
        agent: Mutex::new(None),
        home: home.to_owned(),
        areas: regions::Areas::default(),
        descriptions: descriptions::Descriptions::new(home),
        places: Default::default(),
        keep: keep::Keep::load(home),
        tokens: Mutex::new(None),
    })
}

/// `--mirror` with `--root` needs a `--home` of its own: mirrored into this Mac's own app folder
/// (`default`), a root would let its mirror's files go, tidy its indexes away and add the root's
/// catalog to its own.
fn check_root_mirror(root: bool, mirror: bool, home: Option<&std::path::Path>, default: &std::path::Path) -> Result<()> {
    if root && mirror {
        anyhow::ensure!(home.is_some_and(|h| !same_dir(h, default)), "--mirror with --root needs a --home of its own, not this Mac's app folder ({})", default.display());
    }
    Ok(())
}

/// Whether two paths are the same folder (as given, when either isn't there).
fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// The signal that asked the server to stop, once one has (0 until then).
static STOP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn on_stop(sig: libc::c_int) {
    STOP.store(sig, std::sync::atomic::Ordering::SeqCst);
}

/// On SIGTERM, SIGINT or SIGHUP (the launcher passes them on: a stop, or a restart), writes the
/// mirror's use times out, then exits, as the updater does before it exits for a new app. The
/// handlers only note the signal for the thread that acts on it: without that thread they'd
/// swallow it, so they're set only once it runs (else the signals keep their default action).
fn exit_on_signals(data: Arc<data::Data>) {
    let watch = std::thread::Builder::new().name("signals".into()).spawn(move || loop {
        let sig = STOP.load(std::sync::atomic::Ordering::SeqCst);
        if sig != 0 {
            if let Some(m) = &data.mirror {
                if let Err(e) = m.flush() {
                    eprintln!("mirror: {e:#}");
                }
            }
            eprintln!("stopping (signal {sig})");
            std::process::exit(0);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    });
    if let Err(e) = watch {
        eprintln!("signals: {e}: a stop won't write the mirror's use times out");
        return;
    }
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: a handler that only stores to an atomic, which is async-signal-safe.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_stop as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

/// Raises the open-file limit (launchd starts us at 256): NAS handles, mapped files and
/// connections all count.
fn raise_open_files() {
    // SAFETY: plain getrlimit/setrlimit on this process.
    unsafe {
        let mut l: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) == 0 {
            let want = l.rlim_max.min(10_240);
            if l.rlim_cur < want {
                l.rlim_cur = want;
                libc::setrlimit(libc::RLIMIT_NOFILE, &l);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    raise_open_files();
    let web = PathBuf::from(arg("--web").unwrap_or_else(|| "web/dist".into()));
    let fonts = PathBuf::from(arg("--fonts").unwrap_or_else(|| "data/fonts".into()));
    let port: u16 = arg("--port").unwrap_or_else(|| "8080".into()).parse()?;
    let home = arg("--home").map(PathBuf::from).unwrap_or_else(data::default_home);
    let reserve_gb: f64 = arg("--reserve-gb").map(|v| v.parse()).transpose()?.unwrap_or(50.0);
    anyhow::ensure!(reserve_gb.is_finite() && reserve_gb >= 0.0, "--reserve-gb: a number of GB");
    let no_mirror = std::env::args().any(|a| a == "--no-mirror");
    let root = arg("--root").map(PathBuf::from);
    let mirror_root = std::env::args().any(|a| a == "--mirror");
    check_root_mirror(root.is_some(), mirror_root, arg("--home").map(PathBuf::from).as_deref(), &data::default_home())?;
    let mirror = !no_mirror && (root.is_none() || mirror_root);
    // Soft mounts whichever name the share is mounted by.
    for host in [data::NAS_HOST, store::nas::LAN_HOST] {
        if let Err(e) = store::nas::ensure_nsmb_conf(host, data::NAS_SHARE) {
            eprintln!("nsmb.conf: {e:#}");
        }
    }
    let d = data::Data::open(data::Options { home: home.clone(), nas_root: root.clone(), mirror, reserve: (reserve_gb * 1e9) as u64 })?;
    eprintln!("catalog {} · NAS {}", d.catalog().n, d.nas_root().map(|p| p.display().to_string()).unwrap_or_else(|| "not mounted".into()));
    if root.is_none() {
        d.spawn_background();
    }
    let names = names_live::NamesState::new(&home);
    names.spawn(d.clone());
    let descs = descriptions::Descriptions::new(&home);
    descs.spawn(d.clone());
    let up = updater::Updater::new(&home);
    eprintln!("app: {}", up.running().unwrap_or("development (not from the published app)"));
    up.spawn(d.clone());
    let state = Arc::new(AppState {
        updater: up,
        data: d,
        names,
        basemap: tiles::Basemap::default(),
        packs: Arc::new(cache::Packs::default()),
        details: Mutex::new(None),
        road_en: Mutex::new(None),
        rail_freq: Mutex::new(None),
        agent: Mutex::new(None),
        home: home.clone(),
        areas: regions::Areas::default(),
        descriptions: descs,
        places: Default::default(),
        keep: keep::Keep::load(&home),
        tokens: Mutex::new(None),
    });

    // A stop asked for by a signal (the launcher passes them on) writes the use times out first.
    exit_on_signals(state.data.clone());
    // What the mirror evicts to make room is let go of at once (its maps would hold the room).
    if let Some(m) = &state.data.mirror {
        let s = Arc::downgrade(&state);
        m.on_evict(move |names| {
            if let Some(s) = s.upgrade() {
                s.forget_evicted(names);
            }
        });
    }
    // The mirror: the essentials and the kept areas first (keep.rs).
    keep::spawn_mirror(state.clone());
    tokio::spawn(warm(state.clone()));
    regions::spawn_flusher(state.clone());
    // Other devices (remote.rs): the key, and the address to open there, kept current (tailscale
    // serve may start proxying the server any time).
    let remote = Arc::new(remote::Remote::new(&home)?);
    {
        let r = remote.clone();
        tokio::spawn(async move {
            loop {
                let r2 = r.clone();
                tokio::task::spawn_blocking(move || r2.write_page(port)).await.ok();
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            }
        });
    }

    let app = Router::new()
        .route("/tiles/roads/{z}/{x}/{y}", get(tiles::road_tile))
        .route("/tiles/rails/{z}/{x}/{y}", get(tiles::rail_tile))
        .route("/tiles/labels/{z}/{x}/{y}", get(tiles::label_tile))
        .route("/tiles/smallwater/{z}/{x}/{y}", get(tiles::smallwater_tile))
        .route("/tiles/ov/{name}/{z}/{x}/{y}", get(ovdata::ov_tile))
        .route("/tiles/stations/{z}/{x}/{y}", get(tiles::station_tile))
        .route("/tiles/ferries/{z}/{x}/{y}", get(tiles::ferry_block))
        .route("/api/overlays/detail/{layer}/{id}", get(ovdata::detail))
        .route("/tiles/base/{z}/{x}/{y}", get(tiles::base_tile))
        .route("/tiles/trees/{var}/{z}/{x}/{y}", get(tiles::tree_tile))
        .route("/tiles/buildings/{z}/{x}/{y}", get(tiles::building_tile))
        .route("/tiles/terrain/{z}/{x}/{y}", get(terrain::terrain_tile))
        .route("/tiles/slope/{z}/{x}/{y}", get(terrain::slope_tile))
        .route("/api/railfreq", get(rail_freq_h))
        .route("/api/detail/{layer}/{i}", get(details::detail))
        .route("/api/park", get(details::park))
        .route("/api/marks/view", axum::routing::post(marks::view))
        .route("/api/marks/tile/{kind}/{z}/{x}/{y}", get(marks::tile))
        .route("/api/marks/block/{kind}/{z}/{x}/{y}", get(marks::block))
        .route("/api/marks/specks/{kind}/{z}/{x}/{y}", get(marks::specks))
        .route("/api/marks/count", get(marks::count))
        .route("/api/marks/detail/{kind}/{id}", get(marks::detail))
        .route("/api/meta", get(meta_h))
        .route("/api/catalog", get(catalog_h))
        .route("/api/way/{id}", get(ways::way_h))
        .route("/api/profile/{id}", get(ways::profile_h))
        .route("/api/road/{id}", get(ways::road_h))
        .route("/api/climbs", get(ways::climbs_h))
        .route("/api/viewshed", get(viewshed::viewshed))
        .route("/api/drives", get(query::drives))
        .route("/api/rides", get(query::rides))
        .route("/api/raillines", get(query::lines))
        .route("/api/layer/{name}", get(layer_h))
        .route("/api/names", get(names_h))
        .route("/api/places", get(places::search))
        .route("/api/regions", get(regions::list).post(regions::add))
        .route("/api/regions/{id}", axum::routing::put(regions::edit).delete(regions::remove))
        .route("/api/areas", get(regions::at))
        .route("/api/areas/search", get(regions::search))
        .route("/api/areas/{id}", get(regions::one))
        .route("/api/coverage", get(regions::coverage))
        .route("/api/keep", get(keep::get_status))
        .route("/api/keep/regions/{id}", axum::routing::put(keep::put_region))
        .route("/api/keep/views", axum::routing::post(keep::post_view))
        .route("/api/keep/views/size", axum::routing::post(keep::post_view_size))
        .route("/api/keep/views/{id}", axum::routing::put(keep::put_view).delete(keep::delete_view))
        .route("/api/ping", get(|| async { ([(header::CACHE_CONTROL, "no-store")], "ok") }))
        .route("/api/auth", axum::routing::post({
            let r = remote.clone();
            move |h: HeaderMap, b: axum::body::Bytes| remote::auth(State(r.clone()), h, b)
        }))
        .route("/api/build", get(build_h))
        .route("/api/build/pause", axum::routing::post(build_pause_h))
        .nest_service(
            "/fonts",
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=2592000")))
                .service(ServeDir::new(fonts)),
        )
        // App files revalidate on every load (cheap 304s) so a new app is picked up.
        .fallback_service(
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, HeaderValue::from_static("no-cache")))
                .service(ServeDir::new(&web).fallback(ServeFile::new(web.join("index.html")))),
        )
        .layer(tower_http::compression::CompressionLayer::new().gzip(true))
        // Versioned URLs (?v=…) never change: cached for good when the version is current.
        .layer(axum::middleware::from_fn_with_state(state.clone(), versioned_caching))
        // Data from other host names of this machine (the app spreads its downloads over several:
        // roads.localhost …), for this machine's own pages alone (remote::local_origin).
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::predicate(|o, _| o.to_str().is_ok_and(remote::local_origin)))
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        // First of all: another device's request needs the map's key (remote.rs).
        .layer(axum::middleware::from_fn_with_state(remote.clone(), remote::gate))
        .with_state(state);

    // This Mac, and devices on its LAN and the tailnet (the gate answers them alone).
    let listen: Option<std::net::Ipv4Addr> = arg("--listen").map(|a| a.parse()).transpose()?;
    let addr = std::net::SocketAddr::from((listen.unwrap_or(std::net::Ipv4Addr::UNSPECIFIED), port));
    eprintln!("listening on http://{addr} (devices: the map's address with its key, <home>/map-page)");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let svc = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    if listen.is_none() {
        if let Ok(l6) = tokio::net::TcpListener::bind(std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port))).await {
            let svc6 = svc.clone();
            tokio::spawn(async move { axum::serve(l6, svc6).await });
        }
    }
    axum::serve(listener, svc).await?;
    Ok(())
}

/// Rail service frequency per way: sorted (u32 OSM way id, f32 trains a day each way) pairs.
async fn rail_freq_h(State(s): State<S>, RawQuery(q): RawQuery) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || s2.data.global("global/railfreq")).await {
        Ok(Ok(Some(b))) => ([(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")), (header::CACHE_CONTROL, cache::cache_control(cache::versioned(q.as_deref()), "no-cache"))], b.to_vec()).into_response(),
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// The layer files the app loads whole (pipeline outputs: overlays, stops, stations, ferries …),
/// with display names attached to their features.
async fn layer_h(State(s): State<S>, Path(name): Path<String>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let Some(logical) = layer_logical(&s.data.catalog(), &name) else { return StatusCode::NOT_FOUND.into_response() };
    match cache::respond_layer(&s, &logical, &headers, cache::versioned(q.as_deref())).await {
        Some(r) => r,
        // In the catalog but not read (the NAS away): try again later.
        None if s.data.content(&logical).is_some() => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        None => ([(header::CONTENT_TYPE, "application/geo+json")], r#"{"type":"FeatureCollection","features":[]}"#).into_response(),
    }
}

/// The file a layer name is served from: the lean one the map draws (dem/layers.py) when built.
fn layer_logical(cat: &store::catalog::Catalog, name: &str) -> Option<String> {
    let file = match name {
        "pois" | "heritage" | "special" | "indigenous" | "heritage-areas" | "ferries" | "ferry-lines" | "stations" | "whs-shapes" | "summits" => name.to_string(),
        "sources" => "heritage-sources".to_string(),
        "summary" => "layer-summary".to_string(),
        n if n.starts_with("pois-") && n[5..].chars().all(|c| c.is_ascii_lowercase() || c == '_') => n.to_string(),
        _ => return None,
    };
    // The overlays job's copies (global/heritage: the summary, the sources), else today's.
    for dir in ["global/heritage", "global/legacy"] {
        for l in [format!("{dir}/layer-{file}"), format!("{dir}/{file}")] {
            if cat.files.contains_key(&l) {
                return Some(l);
            }
        }
    }
    Some(format!("global/legacy/{file}"))
}

/// Loads what a first click or page load would otherwise wait for: the details behind popups, the
/// rail frequencies, roads' English names and every layer file (with names attached, gzipped).
/// Again whenever the catalog or the translations change.
async fn warm(s: S) {
    // Nothing until the map is first used: an idle server loads nothing (plan §1).
    while !updater::in_use(u64::MAX) {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    let mut done = None;
    loop {
        let now = (s.generation(), s.names.version_all());
        if done != Some(now) {
            let t0 = std::time::Instant::now();
            let s2 = s.clone();
            let mut ok = tokio::task::spawn_blocking(move || {
                s2.details();
                s2.rail_freq(0);
                s2.road_en(0);
                s2.loaded()
            })
            .await
            .unwrap_or(false);
            let cat = s.data.catalog();
            // Prepared layer files of earlier catalogs go.
            s.packs.retain_contents(&cat.files.values().map(|f| f.file.clone()).collect());
            let mut layers: Vec<String> = cat
                .files
                .keys()
                .filter_map(|k| k.strip_prefix("global/legacy/"))
                .map(|stem| match stem {
                    "heritage-sources" => "sources",
                    _ => stem.strip_prefix("layer-").unwrap_or(stem),
                })
                .filter_map(|name| layer_logical(&cat, name))
                .filter(|l| cat.files.contains_key(l))
                .collect();
            layers.sort();
            layers.dedup();
            for l in &layers {
                ok &= cache::warm(&s, l).await;
            }
            if ok {
                eprintln!("warmed details and {} layer files in {:.1?}", layers.len(), t0.elapsed());
                done = Some(now);
            }
        }
        // Again soon after a failure (the NAS away), else on a change.
        tokio::time::sleep(std::time::Duration::from_secs(if done == Some(now) { 30 } else { 15 })).await;
    }
}

/// Whether a request is the map in use. An open page's polls of the catalog aren't (a tab left open
/// would hold an update off), nor the menu bar item's of the build's status (every five seconds),
/// nor the Regions panel's of the mirror's (`/api/keep`), nor the place search's while its places
/// are made (`poll=1`: places.rs).
fn is_use(uri: &axum::http::Uri) -> bool {
    match uri.path() {
        "/api/catalog" | "/api/ping" | "/api/build" | "/api/keep" => false,
        "/api/places" => !uri.query().is_some_and(|q| q.split('&').any(|kv| kv == "poll=1")),
        _ => true,
    }
}

/// A URL's version token (`v=` in its query).
fn version_token(query: Option<&str>) -> Option<String> {
    query?.split('&').find_map(|kv| kv.strip_prefix("v=")).map(|v| v.replace("%2D", "-"))
}

/// Records the request (for the updater and "in use", polls aside: `is_use`), and caches responses
/// to versioned URLs for good, but only when their version is the current one: an answer fetched
/// under an old version during a catalog or translations switch may hold the new data, and
/// mustn't be pinned to the old URL for a year.
async fn versioned_caching(State(s): State<S>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if is_use(req.uri()) {
        updater::touch();
    }
    let v = version_token(req.uri().query());
    let mut res = next.run(req).await;
    if let Some(v) = v {
        if res.status().is_success() {
            let current = s.current_tokens().contains(&v);
            res.headers_mut().insert(header::CACHE_CONTROL, cache::cache_control(current, "no-cache"));
        }
    }
    res
}

/// What the app needs to start: the build's meta, each layer's version (for its URLs) and zooms,
/// the NAS status.
/// The area overlays served as vector tiles (`/tiles/ov/{name}`, catalog layers `ov-{name}`).
const OV_LAYERS: [&str; 4] = ["heritage-areas", "indigenous", "special", "whs"];

fn meta_json(s: &AppState) -> serde_json::Value {
    let cat = s.data.catalog();
    // An object even before the first catalog (the app then shows an empty map, not an error).
    let mut meta = if cat.meta.is_object() { cat.meta.clone() } else { serde_json::json!({}) };
    let mut versions = serde_json::Map::new();
    // Data served with display names changes with the translations too: their versions carry the
    // translations' version, so a browser's copy cached for good under the old URL isn't used.
    let nv = s.names.version_all();
    let named = |v: String| serde_json::Value::from(format!("{v}-{nv:x}"));
    for l in cat.layers.keys() {
        versions.insert(l.clone(), serde_json::Value::from(s.data.layer_version(l)));
    }
    // The layer files' versions: their content names' hashes, under their old file names
    // ("layer-pois.json", "details-poi.jsonl"), which the app's URLs use.
    for (k, f) in &cat.files {
        let Some(c) = store::naming::parse_content_name(&f.file) else { continue };
        if let Some(stem) = k.strip_prefix("global/legacy/") {
            let v = if c.ext == "json" { named(c.hash16.to_string()) } else { serde_json::Value::from(c.hash16.to_string()) };
            versions.insert(format!("{stem}.{}", c.ext), v);
        } else if k == "global/railfreq" {
            versions.insert("rail-freq.bin".into(), serde_json::Value::from(c.hash16.to_string()));
        }
    }
    // Tile archives by their old names.
    for (old, layer) in [("roads.tiles", "roads"), ("rails.tiles", "rails"), ("terrain.tiles", "terrain"), ("slope.tiles", "slope"), ("labels.tiles", "labels"), ("trees-cover.tiles", "trees-cover"), ("trees-height.tiles", "trees-height"), ("trees-leaf.tiles", "trees-leaf"), ("buildings.tiles", "buildings")] {
        if cat.layers.contains_key(layer) {
            let v = s.data.layer_version(layer);
            versions.insert(old.into(), if layer == "labels" { named(v) } else { serde_json::Value::from(v) });
        }
    }
    // The area overlays' and the rail stops' tiles (names attached).
    for l in ["stations", "ferries"] {
        if cat.layers.contains_key(l) {
            versions.insert(format!("{l}.tiles"), named(s.data.layer_version(l)));
        }
    }
    for l in OV_LAYERS {
        if cat.layers.contains_key(&format!("ov-{l}")) {
            versions.insert(format!("ov-{l}.tiles"), named(s.data.layer_version(&format!("ov-{l}"))));
        }
    }
    // Way info and whole roads change with any data (the catalog's number) and carry names.
    versions.insert("ways.bin".into(), named(format!("c{}", cat.n)));
    // The basemap: its archives' content names.
    let mut bm = blake3::Hasher::new();
    for l in &cat.basemap {
        if let Some(c) = cat.files.get(l) {
            bm.update(c.file.as_bytes());
        }
    }
    let bmv = bm.finalize().to_hex()[..12].to_string();
    versions.insert("basemap".into(), named(bmv.clone()));
    versions.insert("base.pmtiles".into(), named(bmv));
    let zooms: serde_json::Map<String, serde_json::Value> = cat.layers.iter().map(|(k, l)| (k.clone(), serde_json::json!([l.minzoom, l.maxzoom]))).collect();
    if let Some(m) = meta.as_object_mut() {
        m.insert("versions".into(), serde_json::Value::Object(versions));
        m.insert("layers".into(), serde_json::Value::Object(zooms));
        m.insert("catalog".into(), serde_json::json!(cat.n));
        m.insert("online".into(), serde_json::json!(s.data.online()));
        m.insert("labelTiles".into(), serde_json::json!(cat.layers.contains_key("labels")));
        m.insert("smallWater".into(), serde_json::json!(cat.layers.contains_key("smallwater")));
        // The area overlays as vector tiles by view (all of them, or today's files).
        m.insert("stationTiles".into(), serde_json::json!(cat.layers.contains_key("stations")));
        m.insert("ferryBlocks".into(), serde_json::json!(cat.layers.contains_key("ferries")));
        m.insert("ovTiles".into(), serde_json::json!(OV_LAYERS.iter().all(|l| cat.layers.contains_key(&format!("ov-{l}"))) && !cat.ovdata.is_empty()));
        m.insert("labels".into(), serde_json::json!(false));
        m.insert("baseParts".into(), serde_json::json!([]));
    }
    meta
}

async fn meta_h(State(s): State<S>) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(meta_json(&s))).into_response()
}

/// A fingerprint of every version the app's URLs use: it changes with the catalog and with the
/// translations, so an open page knows to switch.
fn versions_fingerprint(s: &AppState) -> String {
    let tokens = s.current_tokens();
    let mut t: Vec<&String> = tokens.iter().collect();
    t.sort();
    let mut h = blake3::Hasher::new();
    for x in t {
        h.update(x.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex()[..12].to_string()
}

/// The credits of the sources a catalog's data comes from (© Credits). A catalog made before
/// catalogs carried them has none, and gets every credit the app knows: what the map showed then.
fn credits_of(cat: &store::catalog::Catalog) -> serde_json::Value {
    match cat.credits.as_array() {
        Some(c) if !c.is_empty() => cat.credits.clone(),
        _ => serde_json::to_value(pipeline::rules::CREDITS).unwrap_or_default(),
    }
}

/// The build agent's status (AppState::build_status), for the menu bar.
async fn build_h(State(s): State<S>) -> Response {
    let body = tokio::task::spawn_blocking(move || s.build_status()).await.unwrap_or(serde_json::Value::Null);
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

/// The whole build paused (`{"mode": "drain"}`: every Mac's job at its next safe point; `"freeze"`:
/// frozen at once) or going on (`{"mode": null}`), from the map: an ask to this Mac's agent, which
/// passes it on to the build Mac (pipeline::control).
async fn build_pause_h(State(s): State<S>, b: axum::body::Bytes) -> Response {
    use pipeline::control::{Mode, Pause};
    let v: serde_json::Value = serde_json::from_slice(&b).unwrap_or_default();
    let mode = match v.get("mode") {
        Some(serde_json::Value::String(m)) if m == "drain" => Some(Mode::Drain),
        Some(serde_json::Value::String(m)) if m == "freeze" => Some(Mode::Freeze),
        Some(serde_json::Value::Null) => None,
        _ => return (StatusCode::BAD_REQUEST, "{\"mode\": \"drain\" | \"freeze\" | null}").into_response(),
    };
    let pause = mode.map(|m| Pause::new(m, &format!("the map on {}", pipeline::agent::cond::host_name())));
    let home = s.home.join("agent");
    match tokio::task::spawn_blocking(move || pipeline::control::request(&home, pause)).await {
        Ok(Ok(())) => ([(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({ "ok": true }))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn catalog_h(State(s): State<S>) -> Response {
    let s2 = s.clone();
    let agent = tokio::task::spawn_blocking(move || s2.agent_status()).await.unwrap_or(serde_json::Value::Null);
    let cat = s.data.catalog();
    // Landmarks by view (docs/phase5.md): the z6 tiles with points, the kinds with tiles, and their
    // totals; absent while the catalog has only today's whole files.
    let s2 = s.clone();
    let marks = tokio::task::spawn_blocking(move || -> serde_json::Value {
        let cat = s2.data.catalog();
        if cat.markdata.is_empty() {
            return serde_json::Value::Null;
        }
        let kinds: Vec<&str> = cat.layers.keys().filter_map(|l| l.strip_prefix("marks-")).collect();
        let summary = s2.data.global("global/marks/summary").ok().flatten().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).unwrap_or_default();
        serde_json::json!({"v": marks::marks_version(&s2), "tiles": cat.markdata.keys().collect::<Vec<_>>(), "kinds": kinds, "summary": summary})
    })
    .await
    .unwrap_or(serde_json::Value::Null);
    let fingerprint = versions_fingerprint(&s);
    // Bytes read from the NAS held in memory (files the mirror doesn't have yet).
    let (pages_b, sections_b) = pages::held();
    let held = serde_json::json!({"pages": pages_b, "sections": sections_b});
    let body = serde_json::json!({
        "n": cat.n,
        "created": cat.created,
        "units": cat.units.len(),
        "layers": cat.layers.iter().map(|(k, l)| (k.clone(), serde_json::json!({"encoding": l.encoding, "minzoom": l.minzoom, "maxzoom": l.maxzoom, "version": s.data.layer_version(k)}))).collect::<serde_json::Map<_, _>>(),
        // The regions it was built for; their outlines are /api/coverage's (this is asked every
        // minute).
        "coverage": {"regions": regions::recorded_list(&cat)},
        "credits": credits_of(&cat),
        "online": s.data.online(),
        "nas": s.data.nas_root().map(|p| p.display().to_string()),
        "held": held,
        "app": s.updater.running(),
        "agent": agent,
        "names": s.names.versions(),
        "v": fingerprint,
        "marks": marks,
    });
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

/// Display names for a batch of names at points: `?n=<name>&at=lon,lat` repeated (each optionally
/// with `en=<own English>` and `k=road` before its `at`; places otherwise).
async fn names_h(State(s): State<S>, RawQuery(q): RawQuery) -> Response {
    let mut out = Vec::new();
    let (mut name, mut own, mut kind): (Option<String>, Option<String>, names::Kind) = (None, None, names::Kind::Place);
    for kv in q.as_deref().unwrap_or("").split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let v = urlencoding_decode(v);
        match k {
            "n" => name = Some(v),
            "en" => own = Some(v),
            "k" => kind = if v == "road" { names::Kind::Road } else { names::Kind::Place },
            "at" => {
                let p: Vec<f64> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                if let (Some(n), true) = (name.take(), p.len() == 2) {
                    let d = s.names.display(std::mem::replace(&mut kind, names::Kind::Place), &n, own.take().as_deref().filter(|x| !x.is_empty()), p[0], p[1]);
                    out.push(serde_json::json!({"main": d.main, "sub": d.sub}));
                }
            }
            _ => {}
        }
    }
    Json(out).into_response()
}

fn urlencoding_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---- the area in view -----------------------------------------------------------------

/// The part of the map in view: a bounding box and, when the client sends it, the outline of the
/// ground on screen. On a tilted globe the bounding box of a view that reaches a pole spans every
/// longitude, far more than is on screen.
pub struct Region {
    /// west, south, east, north (E7 degrees).
    pub bb: [i32; 4],
    poly: Vec<[f64; 2]>,
}

impl Region {
    /// `bbox`: west,south,east,north; `poly`: lon,lat,lon,lat,… (degrees), at least 3 points.
    pub fn parse(bbox: &str, poly: Option<&str>) -> Option<Region> {
        let pts: Vec<[f64; 2]> = poly
            .map(|p| {
                let v: Vec<f64> = p.split(',').filter_map(|x| x.parse().ok()).collect();
                v.chunks_exact(2).map(|c| [c[0], c[1]]).collect()
            })
            .unwrap_or_default();
        if pts.len() >= 3 {
            let (mut w, mut so, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in &pts {
                (w, e, so, n) = (w.min(p[0]), e.max(p[0]), so.min(p[1]), n.max(p[1]));
            }
            let bb = [(w / E7) as i32, (so / E7) as i32, (e / E7) as i32, (n / E7) as i32];
            return Some(Region { bb, poly: pts });
        }
        let b: Vec<f64> = bbox.split(',').filter_map(|x| x.parse().ok()).collect();
        (b.len() == 4).then(|| Region { bb: [(b[0] / E7) as i32, (b[1] / E7) as i32, (b[2] / E7) as i32, (b[3] / E7) as i32], poly: Vec::new() })
    }

    /// Point (E7 degrees) in view.
    pub fn contains(&self, lon: i32, lat: i32) -> bool {
        if lon < self.bb[0] || lon > self.bb[2] || lat < self.bb[1] || lat > self.bb[3] {
            return false;
        }
        if self.poly.is_empty() {
            return true;
        }
        let (x, y) = (lon as f64 * E7, lat as f64 * E7);
        let mut inside = false;
        let n = self.poly.len();
        for i in 0..n {
            let (a, b) = (self.poly[i], self.poly[(i + n - 1) % n]);
            if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
                inside = !inside;
            }
        }
        inside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_is_mirrored_only_into_a_home_of_its_own() {
        let (app, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let d = app.path();
        assert!(check_root_mirror(true, true, None, d).is_err(), "no --home: this Mac's own");
        assert!(check_root_mirror(true, true, Some(d), d).is_err());
        assert!(check_root_mirror(true, true, Some(&d.join(".")), d).is_err(), "the same folder, written otherwise");
        assert!(check_root_mirror(true, true, Some(other.path()), d).is_ok());
        assert!(check_root_mirror(false, true, None, d).is_ok() && check_root_mirror(true, false, None, d).is_ok());
    }

    #[test]
    fn polls_arent_use() {
        let use_ = |u: &str| is_use(&u.parse().unwrap());
        assert!(use_("/tiles/roads/8/1/2?v=abc") && use_("/api/places?q=banff&near=1,2"));
        assert!(!use_("/api/catalog") && !use_("/api/build"));
        // The search box asking again while the places are made.
        assert!(!use_("/api/places?q=banff&near=1,2&n=8&poll=1"));
        assert!(use_("/api/places?q=poll%3D1"));
    }

    #[test]
    fn credits_of_older_catalogs_too() {
        let mut c = store::catalog::Catalog::new(1);
        // A catalog made before catalogs carried credits: every credit the app knows.
        for none in [serde_json::json!([]), serde_json::Value::Null] {
            c.credits = none;
            let all = credits_of(&c);
            assert_eq!(all.as_array().map(Vec::len), Some(pipeline::rules::CREDITS.len()));
            assert_eq!(all[0]["terms"], "© OpenStreetMap contributors, ODbL");
        }
        // Else its own.
        c.credits = serde_json::json!([{"what": "Roads", "source": "OpenStreetMap", "terms": "ODbL"}]);
        assert_eq!(credits_of(&c), c.credits);
    }
}
