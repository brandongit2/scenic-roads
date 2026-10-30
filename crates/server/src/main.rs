//! The map's server: tiles, layers and the APIs over the catalogue's data (docs/macs.md).
//!
//! Its command line: `Cli` below (`server --help`).
//!
//! Data comes from the NAS project folder (found and mounted by itself), read from this Mac's mirror
//! when it's there. `--root` serves a local folder laid out like the project folder instead
//! (development, tests), without a mirror unless `--mirror` (then copied into `--home`'s as from
//! the NAS: a `--home` of its own, never this Mac's app folder). The free space the mirror leaves on
//! the disk is the owner's setting in the Regions panel (crate::reserve). `--listen`: the one address to answer on (a test
//! server's 127.0.0.1), else every IPv4 address and IPv6's loopback. Nothing is loaded at start:
//! the catalogue says where everything is, and files are opened on first use.

mod accepted;
mod cache;
mod data;
mod depth;
mod descriptions;
mod details;
mod downloads;
mod inview;
mod marks;
mod markview;
mod names_live;
mod ovdata;
mod pages;
mod places;
mod profiles;
mod query;
mod regions;
mod remote;
mod reserve;
mod terrain;
mod tiles;
mod updater;
mod views;
mod viewshed;
mod water;
mod ways;

use anyhow::Result;
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
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
    /// Roads' English names from OSM (name:en), by OSM way id, per catalogue.
    road_en: Mutex<Option<(u64, Arc<HashMap<u64, String>>)>>,
    /// Trains a day per rail way (OSM id), sorted, per catalogue.
    rail_freq: Mutex<Option<(u64, Arc<Vec<(u32, f32)>>)>>,
    /// The build agent's heartbeat (state/status.json), re-read at most every 30 s.
    agent: Mutex<Option<(std::time::Instant, serde_json::Value)>>,
    /// This Mac's app folder (`~/Library/Application Support/scenic`): its own agent's status.
    home: PathBuf,
    /// The outlines of the latest OSM pass (the Regions panel).
    pub areas: regions::Areas,
    /// The regions' outlines simplified per zoom (/api/regions/outlines).
    pub outlines: regions::Outlines,
    /// The user's descriptions, laid over popup details.
    pub descriptions: Arc<descriptions::Descriptions>,
    /// The map's places, for its search (made when first searched, again when the labels or the
    /// translations change).
    pub places: Arc<places::Index>,
    /// What this Mac has downloaded for offline use, and what the mirror copies for it.
    pub downloads: Arc<downloads::Downloads>,
    /// The mirror's reserve, the owner's setting (reserve.rs).
    pub reserve: reserve::Setting,
    /// The settings profiles, shared by every user (profiles.rs).
    pub profiles: profiles::Profiles,
    /// The current version tokens of the app's URLs, per (catalogue generation, translations version).
    tokens: Mutex<Option<((u64, u64), Arc<std::collections::HashSet<String>>)>>,
}

pub type S = Arc<AppState>;

impl AppState {
    fn generation(&self) -> u64 {
        self.data.generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A global file, read whole: None when the catalogue has none. A failed read (the NAS away) is
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

    /// A road's own English (OSM's `name:en`), or "": each built unit's `global/roaden/<u>`.
    pub fn road_en(&self, id: u64) -> String {
        let g = self.generation();
        let mut cur = self.road_en.lock().unwrap();
        if cur.as_ref().is_none_or(|(gg, _)| *gg != g) {
            let failed = std::cell::Cell::new(false);
            let units: Vec<String> =
                self.data.catalogue().files.keys().filter(|l| l.starts_with("global/roaden/")).cloned().collect();
            let mut map: HashMap<u64, String> = HashMap::new();
            for l in units {
                let part = self
                    .global_or_note(&l, &failed)
                    .and_then(|b| serde_json::from_slice::<HashMap<String, String>>(&b).ok())
                    .unwrap_or_default();
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
                .map(|b| {
                    b.chunks_exact(8)
                        .map(|c| {
                            (
                                u32::from_le_bytes(c[..4].try_into().unwrap()),
                                f32::from_le_bytes(c[4..].try_into().unwrap()).abs(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            if failed.get() {
                return Arc::new(Vec::new());
            }
            *cur = Some((g, Arc::new(v)));
        }
        cur.as_ref().unwrap().1.clone()
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
        let tok = |v: &serde_json::Value| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string()));
        let mut t: std::collections::HashSet<String> = meta
            .get("versions")
            .and_then(|v| v.as_object())
            .map(|m| m.values().filter_map(tok).collect())
            .unwrap_or_default();
        // The packs' tokens (tiles' `?v=`).
        for l in meta.get("packs").and_then(|v| v.as_object()).into_iter().flat_map(|m| m.values()) {
            t.extend(l.get("root").and_then(tok));
            for scope in ["lo", "hi", "deep"] {
                t.extend(l.get(scope).and_then(|v| v.as_object()).into_iter().flat_map(|m| m.values()).filter_map(tok));
            }
        }
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
            (Some(root), Some(pool)) if pool.is_online() => pool
                .read_all(&root.join("state/status.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };
        *cur = Some((std::time::Instant::now(), v.clone()));
        v
    }

    /// The build agent's status for the menu bar (tools/status): this Mac's own agent's when it runs
    /// here (written every few seconds), else the heartbeat it copies to the NAS (on change, and
    /// every five minutes); whether it's this Mac's; for this Mac's, the running job's log; and
    /// what this Mac has downloaded (downloads::summary). (The build page: this Mac's own agent's,
    /// on its port, tools/status's panel.)
    fn build_status(&self) -> serde_json::Value {
        self.build_status_at(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        )
    }

    /// `build_status` at `now` (unix seconds).
    fn build_status_at(&self, now: u64) -> serde_json::Value {
        let own: Option<serde_json::Value> =
            std::fs::read(self.home.join("agent/status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let fresh = own.as_ref().and_then(|v| v["beat"].as_u64()).is_some_and(|b| now.saturating_sub(b) < 120);
        // A helper on this Mac (docs/building.md §2, The pool; a pool's member): its own status, fresh.
        let helper: Option<serde_json::Value> = std::fs::read(self.home.join("agent/helper.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .filter(|h: &serde_json::Value| h["beat"].as_u64().is_some_and(|b| now.saturating_sub(b) < 120));
        // (The lead's heartbeat on the NAS, written every two minutes at least while there's a
        // lead: older than ten, there's none to show, and a member shows its own status in its place,
        // so its menu bar stays fresh.)
        let (mut status, local, helping) = match own {
            Some(v) if fresh => (v, true, false),
            _ => {
                let lead = self.agent_status();
                let lead_fresh = lead["beat"].as_u64().is_some_and(|b| now.saturating_sub(b) < 600);
                match &helper {
                    Some(h) if !lead_fresh => (h.clone(), true, true),
                    _ => (lead, false, false),
                }
            }
        };
        // Its own status, fresh, in place of what the lead last read of it (which stops while
        // the lead sleeps); not when it's the status shown.
        if let (Some(h), Some(obj)) =
            (helper.filter(|h| !helping && h["host"] != status["host"]), status.as_object_mut())
        {
            let host = h["host"].clone();
            let mut list: Vec<serde_json::Value> =
                obj.get("helpers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            list.retain(|x| x["host"] != host);
            list.push(serde_json::json!({"host": host, "beat": h["beat"], "job": h["job"], "pause": h["pause"], "held": h["held"], "here": true}));
            obj.insert("helpers".into(), serde_json::Value::Array(list));
        }
        let log = if local {
            status["job"]["id"].as_str().map(|id| {
                self.home.join("agent/logs").join(format!("{}.log", id.replace([' ', '/'], "-"))).display().to_string()
            })
        } else {
            None
        };
        // This Mac's ask to pause or go on, while its agent hasn't taken it up (pipeline::control).
        let asked = pipeline::control::peek_request(&self.home.join("agent"))
            .map(|r| serde_json::json!({ "pause": r.pause.is_some(), "at": r.at }));
        // What this Mac has downloaded (the menu says when the map needs the NAS).
        let offline = downloads::summary(self);
        // This Mac in the pool (docs/pool.md §10, §11), as its own agent says (fresh: its status, or
        // its pool's own view, written each step of the pool's whatever its loop is doing).
        let pool = pipeline::agent::lead::own_pool(&self.home.join("agent"), now, 120)
            .map_or(serde_json::Value::Null, |(p, _)| serde_json::to_value(p).unwrap_or_default());
        // This Mac paused alone (pipeline::control::Hold, task #224), as its agent keeps it, and its
        // ask while the agent hasn't taken it up.
        let agent_home = self.home.join("agent");
        let held_here = pipeline::control::read_hold(&agent_home).held;
        let hold_asked = agent_home.join(pipeline::control::HOLD_REQUEST).exists();
        serde_json::json!({"status": status, "local": local, "now": now, "log": log, "asked": asked, "offline": offline, "pool": pool, "held_here": held_here, "hold_asked": hold_asked})
    }

    /// Lets go of everything mapped from what the mirror has just deleted (`names`: content
    /// names, and basemap pieces by their paths in it), so the disk gets their room back.
    fn forget_removed(&self, names: &[String]) {
        self.data.forget_removed(names);
        self.basemap.forget(names);
        self.areas.forget(names);
    }

    /// Whether the rail frequencies and roads' English names of this catalogue are loaded.
    fn loaded(&self) -> bool {
        let g = self.generation();
        self.rail_freq.lock().unwrap().as_ref().is_some_and(|(gg, _)| *gg == g)
            && self.road_en.lock().unwrap().as_ref().is_some_and(|(gg, _)| *gg == g)
    }
}

/// A server for tests over `root`, a local folder laid out like the NAS project folder (as with
/// `--root`: the real NAS is never looked for), with this Mac's files in `home`. Nothing runs in
/// the background.
#[cfg(test)]
pub fn test_state(home: &std::path::Path, root: &store::nasfs::NasRoot) -> S {
    test_state_with(home, root, false)
}

/// `test_state`, with a mirror in `home` when `mirror` (nothing copies into it unless a test
/// syncs it).
#[cfg(test)]
pub fn test_state_with(home: &std::path::Path, root: &store::nasfs::NasRoot, mirror: bool) -> S {
    test_state_from(
        home,
        data::Data::open(data::Options { home: home.to_owned(), nas_root: Some(root.to_owned()), mirror, reserve: 0 })
            .unwrap(),
    )
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
        road_en: Mutex::new(None),
        rail_freq: Mutex::new(None),
        agent: Mutex::new(None),
        home: home.to_owned(),
        areas: regions::Areas::default(),
        outlines: regions::Outlines::default(),
        descriptions: descriptions::Descriptions::new(home),
        places: Default::default(),
        downloads: downloads::Downloads::load(home),
        reserve: reserve::Setting::load(home),
        profiles: profiles::Profiles::new(home),
        tokens: Mutex::new(None),
    })
}

/// `--mirror` with `--root` needs a `--home` of its own: mirrored into this Mac's own app folder
/// (`default`), a root would let its mirror's files go, tidy its indexes away and add the root's
/// catalogue to its own.
fn check_root_mirror(
    root: bool,
    mirror: bool,
    home: Option<&std::path::Path>,
    default: &std::path::Path,
) -> Result<()> {
    if root && mirror {
        anyhow::ensure!(
            home.is_some_and(|h| !same_dir(h, default)),
            "--mirror with --root needs a --home of its own, not this Mac's app folder ({})",
            default.display()
        );
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

/// The map's server: tiles, layers and the APIs over the catalogue's data (docs/macs.md). Its command
/// line, checked before anything starts (clap: docs/building.md, "Command lines"), so a mistyped or
/// unknown option never runs the server with its defaults (this Mac's own app folder and the NAS).
#[derive(clap::Parser, Debug)]
#[command(name = "server")]
struct Cli {
    /// The port
    #[arg(long, default_value_t = pipeline::net::DEFAULT_PORT)]
    port: u16,
    /// The web app's built files
    #[arg(long, value_name = "DIR", default_value = "web/dist")]
    web: PathBuf,
    /// The map's fonts
    #[arg(long, value_name = "DIR", default_value = "data/fonts")]
    fonts: PathBuf,
    /// Its folder (default: this Mac's app folder)
    #[arg(long, value_name = "DIR")]
    home: Option<PathBuf>,
    /// A local folder laid out like the project folder, served instead of the NAS's (no mirror
    /// unless --mirror)
    #[arg(long, value_name = "DIR")]
    root: Option<PathBuf>,
    /// No mirror of the NAS's files on this Mac
    #[arg(long, conflicts_with = "mirror")]
    no_mirror: bool,
    /// --root's files mirrored into --home's as from the NAS (a --home of its own)
    #[arg(long)]
    mirror: bool,
    /// The one address to answer on (a test server's 127.0.0.1), else every IPv4 address and IPv6's
    /// loopback
    #[arg(long, value_name = "IPV4 ADDRESS")]
    listen: Option<std::net::Ipv4Addr>,
    /// Taken and ignored (warned): a run file written before the reserve became the Regions panel's
    /// setting passes it until tools/app/install.sh writes it again
    #[arg(long, value_name = "GB", hide = true)]
    reserve_gb: Option<String>,
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
    let Cli { port, web, fonts, home: home_arg, root, no_mirror, mirror: mirror_root, listen, reserve_gb } =
        <Cli as clap::Parser>::parse();
    raise_open_files();
    // (A catalogue read through deltas kept on this Mac for the next read: store::catalogue::keep_read_in.)
    if let Some(d) = store::catalogue::kept_dir() {
        store::catalogue::keep_read_in(d);
    }
    let home = home_arg.clone().unwrap_or_else(data::default_home);
    if reserve_gb.is_some() {
        eprintln!(
            "server: --reserve-gb is ignored: the reserve is set in the Regions panel ({} GB now)",
            reserve::load(&home)
        );
    }
    check_root_mirror(root.is_some(), mirror_root, home_arg.as_deref(), &data::default_home())?;
    let mirror = !no_mirror && (root.is_none() || mirror_root);
    // Soft mounts whichever name the share is mounted by.
    for host in [data::NAS_HOST, store::nas::LAN_HOST] {
        if let Err(e) = store::nas::ensure_nsmb_conf(host, data::NAS_SHARE) {
            eprintln!("nsmb.conf: {e:#}");
        }
    }
    let d = data::Data::open(data::Options {
        home: home.clone(),
        nas_root: root.clone().map(|r| store::nasfs::NasRoot::from_arg(r).short_retry()),
        mirror,
        reserve: reserve::Setting::load(&home).bytes(),
    })?;
    eprintln!(
        "catalogue {} · NAS {}",
        d.catalogue().n,
        d.nas_root().map(|p| p.display().to_string()).unwrap_or_else(|| "not mounted".into())
    );
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
        road_en: Mutex::new(None),
        rail_freq: Mutex::new(None),
        agent: Mutex::new(None),
        home: home.clone(),
        areas: regions::Areas::default(),
        outlines: regions::Outlines::default(),
        descriptions: descs,
        places: Default::default(),
        downloads: downloads::Downloads::load(&home),
        reserve: reserve::Setting::load(&home),
        profiles: profiles::Profiles::new(&home),
        tokens: Mutex::new(None),
    });

    // What the mirror deletes is let go of at once (its maps would hold the room).
    if let Some(m) = &state.data.mirror {
        let s = Arc::downgrade(&state);
        m.on_remove(move |names| {
            if let Some(s) = s.upgrade() {
                s.forget_removed(names);
            }
        });
    }
    // The mirror: what's downloaded (downloads.rs).
    downloads::spawn_mirror(state.clone());
    tokio::spawn(warm(state.clone()));
    regions::spawn_syncer(state.clone());
    // Other devices (remote.rs): the address to open there, kept current (tailscale serve may start
    // proxying the server any time). (A `remote-key` an older server kept: unused, removed.)
    std::fs::remove_file(home.join("remote-key")).ok();
    {
        let r = Arc::new(remote::Remote::new(&home));
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
        .route("/tiles/water/{z}/{x}/{y}", get(water::water_tile))
        .route("/tiles/ov/{name}/{z}/{x}/{y}", get(ovdata::ov_tile))
        .route("/tiles/stations/{z}/{x}/{y}", get(tiles::station_tile))
        .route("/tiles/ferries/{z}/{x}/{y}", get(tiles::ferry_block))
        .route("/api/overlays/detail/{layer}/{id}", get(ovdata::detail))
        .route("/tiles/base/{z}/{x}/{y}", get(tiles::base_tile))
        .route("/tiles/trees/{var}/{z}/{x}/{y}", get(tiles::tree_tile))
        .route("/tiles/buildings/{z}/{x}/{y}", get(tiles::building_tile))
        .route("/tiles/buildings-field/{z}/{x}/{y}", get(tiles::building_field_tile))
        .route("/tiles/terrain/{z}/{x}/{y}", get(terrain::terrain_tile))
        .route("/tiles/slope/{z}/{x}/{y}", get(terrain::slope_tile))
        .route("/tiles/polar-terrain/{pole}/{z}/{x}/{y}", get(terrain::polar_terrain_tile))
        .route("/tiles/polar-slope/{pole}/{z}/{x}/{y}", get(terrain::polar_slope_tile))
        .route("/tiles/depth/{z}/{x}/{y}", get(depth::depth_tile))
        .route("/tiles/depth-class/{z}/{x}/{y}", get(depth::class_tile))
        .route("/tiles/depth-shade/{z}/{x}/{y}", get(terrain::depth_shade_tile))
        .route("/api/railfreq", get(rail_freq_h))
        .route("/api/park", get(details::park))
        .route("/api/marks/view", axum::routing::post(marks::view))
        .route("/api/marks/tile/{kind}/{z}/{x}/{y}", get(marks::tile))
        .route("/api/marks/block/{kind}/{z}/{x}/{y}", get(marks::block))
        .route("/api/marks/specks/{kind}/{z}/{x}/{y}", get(marks::specks))
        .route("/api/marks/count", get(marks::count))
        .route("/api/marks/detail/{kind}/{id}", get(marks::detail))
        .route("/api/meta", get(meta_h))
        .route("/api/catalog", get(catalogue_h))
        .route("/api/catalog/refresh", axum::routing::post(data::refresh_h))
        .route("/api/way/{id}", get(ways::way_h))
        .route("/api/profile/{id}", get(ways::profile_h))
        .route("/api/road/{id}", get(ways::road_h))
        .route("/api/viewshed", get(viewshed::viewshed))
        .route("/api/drives", get(query::drives))
        .route("/api/rides", get(query::rides))
        .route("/api/raillines", get(query::lines))
        .route("/api/layer/{name}", get(layer_h))
        .route("/api/names", get(names_h))
        .route("/api/places", get(places::search))
        .route("/api/regions", get(regions::list))
        .route("/api/regions/outlines", get(regions::outlines))
        .route("/api/coverage", get(regions::coverage))
        .route("/api/downloads", get(downloads::get_status))
        .route("/api/downloads/world", axum::routing::put(downloads::put_world))
        .route("/api/downloads/regions/{id}", axum::routing::put(downloads::put_region))
        .route("/api/downloads/regions/{id}/detail", axum::routing::put(downloads::put_region_detail))
        .route("/api/downloads/world/update", axum::routing::post(downloads::post_world_update))
        .route("/api/downloads/regions/{id}/update", axum::routing::post(downloads::post_region_update))
        .route("/api/downloads/regions/{id}/detail/update", axum::routing::post(downloads::post_detail_update))
        .route("/api/downloads/reserve", axum::routing::put(downloads::put_reserve))
        .route("/api/profiles", get(profiles::get_list).post(profiles::post))
        .route("/api/profiles/{id}", axum::routing::put(profiles::put).delete(profiles::delete))
        .route("/api/profiles/{id}/default", axum::routing::post(profiles::set_default))
        .route("/api/ping", get(|| async { ([(header::CACHE_CONTROL, "no-store")], "ok") }))
        .route("/api/build", get(build_h))
        .route("/api/bench", axum::routing::post(bench_h))
        .nest_service(
            "/fonts",
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=2592000"),
                ))
                .service(ServeDir::new(fonts)),
        )
        // App files revalidate on every load (cheap 304s) so a new app is picked up.
        .fallback_service(
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::if_not_present(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ))
                .service(ServeDir::new(&web).fallback(ServeFile::new(web.join("index.html")))),
        )
        .layer(tower_http::compression::CompressionLayer::new().gzip(true))
        // Versioned URLs (?v=…) never change: cached for good when the version is current.
        .layer(axum::middleware::from_fn_with_state(state.clone(), versioned_caching))
        // Data from other host names of this machine (the app spreads its downloads over several:
        // roads.localhost …), for this machine's own pages alone (remote::local_origin).
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::predicate(|o, _| {
                    o.to_str().is_ok_and(remote::local_origin)
                }))
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        // First of all: this Mac, its LAN and the tailnet alone, never a page elsewhere (remote.rs).
        .layer(axum::middleware::from_fn(remote::gate))
        .with_state(state);

    // This Mac, and devices on its LAN and the tailnet (the gate answers them alone).
    let addr = std::net::SocketAddr::from((listen.unwrap_or(std::net::Ipv4Addr::UNSPECIFIED), port));
    eprintln!("listening on http://{addr} (devices: the map's address, <home>/map-page)");
    // (Made, and their connections taken, while no child is being started, store::spawn: a child
    // started between would hold the port.)
    use axum::serve::ListenerExt;
    let listener = pipeline::net::Listener::new(store::spawn::listen(addr)?)?.tap_io(|_| ());
    let svc = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    if listen.is_none() {
        if let Ok(l6) = store::spawn::listen(std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)))
            .and_then(pipeline::net::Listener::new)
        {
            let svc6 = svc.clone();
            tokio::spawn(async move { axum::serve(l6.tap_io(|_| ()), svc6).await });
        }
    }
    axum::serve(listener, svc).await?;
    Ok(())
}

/// Rail service frequency per way: sorted (u32 OSM way id, f32 trains a day each way) pairs.
async fn rail_freq_h(State(s): State<S>, RawQuery(q): RawQuery) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || s2.data.global("global/railfreq")).await {
        Ok(Ok(Some(b))) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
                (header::CACHE_CONTROL, cache::cache_control(cache::versioned(q.as_deref()), "no-cache")),
            ],
            b.to_vec(),
        )
            .into_response(),
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// The layer files the app loads whole (the overlays job's summary), with display names attached to
/// their features.
async fn layer_h(State(s): State<S>, Path(name): Path<String>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let Some(logical) = layer_logical(&s.data.catalogue(), &name) else { return StatusCode::NOT_FOUND.into_response() };
    match cache::respond_layer(&s, &logical, &headers, cache::versioned(q.as_deref())).await {
        Some(r) => r,
        // In the catalogue but not read (the NAS away): try again later.
        None if s.data.content(&logical).is_some() => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        None => ([(header::CONTENT_TYPE, "application/geo+json")], r#"{"type":"FeatureCollection","features":[]}"#)
            .into_response(),
    }
}

/// The file a layer name is served from: the overlays job's (`global/heritage/`).
fn layer_logical(cat: &store::catalogue::Catalogue, name: &str) -> Option<String> {
    let l = match name {
        "summary" => "global/heritage/layer-summary",
        _ => return None,
    };
    cat.files.contains_key(l).then(|| l.to_string())
}

/// Loads what a first click or page load would otherwise wait for: the rail frequencies, roads'
/// English names and the layer files (with names attached, gzipped).
/// Again whenever the catalogue or the translations change.
async fn warm(s: S) {
    // Nothing until the map is first used: an idle server loads nothing (docs/surface.md).
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
                s2.rail_freq(0);
                s2.road_en(0);
                s2.loaded()
            })
            .await
            .unwrap_or(false);
            let cat = s.data.catalogue();
            // Prepared layer files of earlier catalogues go.
            s.packs.retain_contents(&cat.files.values().map(|f| f.file.clone()).collect());
            let layers: Vec<String> = ["summary"].iter().filter_map(|name| layer_logical(&cat, name)).collect();
            for l in &layers {
                ok &= cache::warm(&s, l).await;
            }
            if ok {
                eprintln!("warmed {} layer files in {:.1?}", layers.len(), t0.elapsed());
                done = Some(now);
            }
        }
        // Again soon after a failure (the NAS away), else on a change.
        tokio::time::sleep(std::time::Duration::from_secs(if done == Some(now) { 30 } else { 15 })).await;
    }
}

/// Whether a request is the map in use. An open page's polls of the catalogue aren't (a tab left open
/// would hold an update off), nor the menu bar item's of the build's status (every five seconds),
/// nor the Regions panel's of the downloads' (`/api/downloads`), nor the place search's while its places
/// are made (`poll=1`: places.rs).
fn is_use(uri: &axum::http::Uri) -> bool {
    match uri.path() {
        "/api/catalog" | "/api/ping" | "/api/build" | "/api/downloads" => false,
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
/// under an old version during a catalogue or translations switch may hold the new data, and
/// mustn't be pinned to the old URL for a year.
async fn versioned_caching(State(s): State<S>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if is_use(req.uri()) {
        updater::touch();
    }
    let v = version_token(req.uri().query());
    let mut res = next.run(req).await;
    if let Some(v) = v {
        if res.status().is_success() {
            let current = res.extensions().get::<cache::NotForGood>().is_none() && s.current_tokens().contains(&v);
            res.headers_mut().insert(header::CACHE_CONTROL, cache::cache_control(current, "no-cache"));
        }
    }
    res
}

/// What the app needs to start: the build's meta, each layer's version (for its URLs) and zooms,
/// the NAS status.
/// The area overlays served as vector tiles (`/tiles/ov/{name}`, catalogue layers `ov-{name}`).
const OV_LAYERS: [&str; 4] = ["heritage-areas", "indigenous", "special", "whs"];

fn meta_json(s: &AppState) -> serde_json::Value {
    let cat = s.data.catalogue();
    // An object even before the first catalogue (the app then shows an empty map, not an error).
    // (The fields the app reads at start that only a built catalogue's meta has: without them it
    // stopped at setSource, "bounds is not iterable".)
    let mut meta = if cat.meta.is_object() { cat.meta.clone() } else { serde_json::json!({}) };
    let empty = serde_json::json!({"minzoom": 0, "maxzoom": 0, "bounds": [-180.0, -85.0511, 180.0, 85.0511],
        "ways": 0, "vertices": 0, "elev_min": 0, "elev_max": 0, "elev_hist_10m_km": [], "built": 0});
    for (k, v) in empty.as_object().into_iter().flatten() {
        if meta.get(k).is_none() {
            meta[k] = v.clone();
        }
    }
    let mut versions = serde_json::Map::new();
    // Data served with display names changes with the translations too: their versions carry the
    // translations' version, so a browser's copy cached for good under the old URL isn't used.
    let nv = s.names.version_all();
    let named = |v: String| serde_json::Value::from(format!("{v}-{nv:x}"));
    for l in cat.layers.keys() {
        versions.insert(l.clone(), serde_json::Value::from(s.data.layer_version(l)));
    }
    // The layer files' versions: their content names' hashes, under their file names
    // ("layer-summary.json"), which the app's URLs use.
    for (k, f) in &cat.files {
        let Some(c) = store::naming::parse_content_name(&f.file) else { continue };
        if let Some(stem) = k.strip_prefix("global/heritage/") {
            let v = if c.ext == "json" {
                named(c.hash16.to_string())
            } else {
                serde_json::Value::from(c.hash16.to_string())
            };
            versions.insert(format!("{stem}.{}", c.ext), v);
        } else if k == "global/railfreq" {
            versions.insert("rail-freq.bin".into(), serde_json::Value::from(c.hash16.to_string()));
        }
    }
    // Tile archives by their old names.
    for (old, layer) in [
        ("roads.tiles", "roads"),
        ("rails.tiles", "rails"),
        ("terrain.tiles", "terrain"),
        ("slope.tiles", "slope"),
        ("labels.tiles", "labels"),
        ("trees-cover.tiles", "trees-cover"),
        ("trees-height.tiles", "trees-height"),
        ("trees-leaf.tiles", "trees-leaf"),
        ("buildings.tiles", "buildings"),
        ("depth.tiles", "depth"),
        ("depth-class.tiles", "depth-class"),
        ("depth-shade.tiles", "depth-shade"),
    ] {
        if cat.layers.contains_key(layer) {
            let mut v = s.data.layer_version(layer);
            // (The shading layers' in the encoding served too: a browser's copy from a pack before it,
            // cached for good under the same pack's version, is never reused, terrain::shade_tile.)
            if layer == "slope" || layer == "depth-shade" {
                v = format!("{v}-{}", pipeline::shade_codec::ENCODING);
            }
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
    // Way info and whole roads change with any data (the catalogue's number) and carry names.
    // (And with what's read of the downloads behind it: Data::older_token.)
    let older = s.data.older_token();
    versions.insert("ways.bin".into(), named(format!("c{}{older}", cat.n)));
    // The basemap: its archives' content names.
    let mut bm = blake3::Hasher::new();
    for l in &cat.basemap {
        if let Some(c) = s.data.content(l) {
            bm.update(c.as_bytes());
        }
    }
    bm.update(older.as_bytes());
    let bmv = bm.finalize().to_hex()[..12].to_string();
    versions.insert("basemap".into(), named(bmv.clone()));
    versions.insert("base.pmtiles".into(), named(bmv));
    // The water's tiles: its packs and the basemap they're drawn from deeper.
    versions.insert("water".into(), serde_json::Value::from(water::version(s)));
    // Each tiled layer's packs' versions (a tile's `?v=` is that of the pack holding it): a new
    // catalogue changes only the tokens of the packs it changed, so a browser asks again only for
    // their tiles. Root: z0–2; lo: z3–8, by z3 tile; hi: z9–14, by z6 tile; the terrain's deep
    // packs past z12, by z10 tile; a polar layer's root z0–3 and blocks z4–9, by z4 tile of its
    // grid (Data::pack_of) ("z/x/y").
    let mut packs = serde_json::Map::new();
    for (k, l) in &cat.layers {
        let names = k == "labels" || k == "stations" || k == "ferries" || k.starts_with("ov-");
        let tok = |logical: &String| {
            s.data.content(logical).map(|f| {
                let v = pack_token(&f);
                if names {
                    named(v)
                } else {
                    serde_json::Value::from(v)
                }
            })
        };
        let by = |m: &std::collections::BTreeMap<String, String>| {
            m.iter().filter_map(|(t, p)| tok(p).map(|v| (t.clone(), v))).collect::<serde_json::Map<_, _>>()
        };
        let mut o = serde_json::Map::new();
        if let Some(v) = l.root.as_ref().and_then(tok) {
            o.insert("root".into(), v);
        }
        o.insert("lo".into(), serde_json::Value::Object(by(&l.lo)));
        o.insert("hi".into(), serde_json::Value::Object(by(&l.hi)));
        if !l.deep.is_empty() {
            o.insert("deep".into(), serde_json::Value::Object(by(&l.deep)));
        }
        // (A polar layer's blocks, z4–9 by z4 tile of its pole's grid; its root z0–3: pipeline::polar.)
        if !l.polar.is_empty() {
            o.insert("polar".into(), serde_json::Value::Object(by(&l.polar)));
        }
        packs.insert(k.clone(), serde_json::Value::Object(o));
    }
    let zooms: serde_json::Map<String, serde_json::Value> =
        cat.layers.iter().map(|(k, l)| (k.clone(), serde_json::json!([l.minzoom, l.maxzoom]))).collect();
    if let Some(m) = meta.as_object_mut() {
        m.insert("versions".into(), serde_json::Value::Object(versions));
        m.insert("packs".into(), serde_json::Value::Object(packs));
        m.insert("layers".into(), serde_json::Value::Object(zooms));
        // Where the terrain goes past the base's finest zoom (pipeline::terrain_deep): its z10
        // tiles with deep packs and their native zoom, "x/y" (the map's zoom by area:
        // web/src/basemap.ts); elsewhere the base's.
        if let Some(l) = cat.layers.get("terrain").filter(|l| !l.native_zoom.is_empty()) {
            let tiles: serde_json::Map<String, serde_json::Value> = l
                .native_zoom
                .iter()
                .filter_map(|(k, z)| Some((k.split_once('/')?.1.to_string(), serde_json::json!(z))))
                .collect();
            m.insert(
                "terrainAreas".into(),
                serde_json::json!({"zoom": pipeline::terrain_deep::PACK_Z, "tiles": tiles}),
            );
        }
        m.insert("catalog".into(), serde_json::json!(cat.n));
        m.insert("online".into(), serde_json::json!(s.data.online()));
        m.insert("labelTiles".into(), serde_json::json!(cat.layers.contains_key("labels")));
        // The water as exact coverage (pipeline::water; water.rs), where the catalogue has it.
        m.insert("water".into(), serde_json::json!(cat.layers.contains_key("water")));
    }
    meta
}

async fn meta_h(State(s): State<S>) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(meta_json(&s))).into_response()
}

/// A pack's version token: its content name's hash, 12 hex digits.
fn pack_token(content: &str) -> String {
    blake3::hash(content.as_bytes()).to_hex()[..12].to_string()
}

/// A fingerprint of every version the app's URLs use: it changes with the catalogue and with the
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

/// The credits of the sources a catalogue's data comes from (© Credits). A catalogue made before
/// catalogues carried them has none, and gets every credit the app knows: what the map showed then.
fn credits_of(cat: &store::catalogue::Catalogue) -> serde_json::Value {
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

/// A result of the in-page self-benchmark (web/src/selfbench.ts): its JSON saved as
/// `<home>/bench/<name>.json` (`?name=`, letters, digits, `-` and `_` alone), for analysis
/// (tools/bench/inpage.mjs). At most 2 MB each (axum's body limit), and the folder at most
/// BENCH_MAX_FILES results and BENCH_MAX_BYTES (save_bench).
async fn bench_h(State(s): State<S>, RawQuery(q): RawQuery, b: axum::body::Bytes) -> Response {
    let name = q.as_deref().and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("name="))).filter(|n| {
        !n.is_empty() && n.len() <= 120 && n.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    });
    let Some(name) = name.map(str::to_owned) else {
        return (StatusCode::BAD_REQUEST, "?name= (letters, digits, - and _)").into_response();
    };
    if serde_json::from_slice::<serde_json::Value>(&b).is_err() {
        return (StatusCode::BAD_REQUEST, "a JSON body").into_response();
    }
    let dir = s.home.join("bench");
    match tokio::task::spawn_blocking(move || save_bench(&dir, &name, &b)).await {
        Ok(Ok(())) => ([(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({ "ok": true }))).into_response(),
        Ok(Err(BenchSave::Full(why))) => (StatusCode::INSUFFICIENT_STORAGE, why).into_response(),
        Ok(Err(BenchSave::Io(e))) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The most benchmark results `<home>/bench/` keeps, and their most bytes: past either, a new
/// result is refused (a result saved again under its own name replaces it).
const BENCH_MAX_FILES: usize = 500;
const BENCH_MAX_BYTES: u64 = 200 << 20;

#[derive(Debug)]
enum BenchSave {
    Full(String),
    Io(std::io::Error),
}

/// Saves one result as `<dir>/<name>.json`, unless the folder would then hold more than
/// BENCH_MAX_FILES results or BENCH_MAX_BYTES.
fn save_bench(dir: &std::path::Path, name: &str, body: &[u8]) -> std::result::Result<(), BenchSave> {
    std::fs::create_dir_all(dir).map_err(BenchSave::Io)?;
    let file = format!("{name}.json");
    let (mut files, mut bytes) = (0usize, 0u64);
    for e in std::fs::read_dir(dir).map_err(BenchSave::Io)?.flatten() {
        if e.file_name().to_str() == Some(file.as_str()) {
            continue;
        }
        files += 1;
        bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
    }
    if files + 1 > BENCH_MAX_FILES || bytes + body.len() as u64 > BENCH_MAX_BYTES {
        return Err(BenchSave::Full(format!(
            "{} holds {files} results, {} MB: at most {BENCH_MAX_FILES} and {} MB; move old results away first",
            dir.display(),
            bytes >> 20,
            BENCH_MAX_BYTES >> 20
        )));
    }
    std::fs::write(dir.join(file), body).map_err(BenchSave::Io)
}

async fn catalogue_h(State(s): State<S>) -> Response {
    let s2 = s.clone();
    let agent = tokio::task::spawn_blocking(move || s2.agent_status()).await.unwrap_or(serde_json::Value::Null);
    let cat = s.data.catalogue();
    // Landmarks by view (docs/phase5.md): the z6 tiles with points, the kinds with tiles, and their
    // totals; absent while the catalogue has none.
    let s2 = s.clone();
    let marks = tokio::task::spawn_blocking(move || -> serde_json::Value {
        let cat = s2.data.catalogue();
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
        // The depth's sources, by the code its class tiles carry (pipeline::depth): their labels,
        // datums and the notices their licences ask the map to show.
        "depth": cat.meta.get("depth"),
    });
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

/// Display names for a batch of names at points: `?n=<name>&at=lon,lat` repeated, each optionally
/// with `en=<own English>`, `k=road|settlement|other` (other when not given) and `l=<languages OSM
/// gives the name, comma-separated>` before its `at`.
async fn names_h(State(s): State<S>, RawQuery(q): RawQuery) -> Response {
    let mut out = Vec::new();
    let (mut name, mut own, mut kind, mut langs): (Option<String>, Option<String>, names::Kind, Vec<names::Lang>) =
        (None, None, names::Kind::Other, Vec::new());
    for kv in q.as_deref().unwrap_or("").split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let v = urlencoding_decode(v);
        match k {
            "n" => name = Some(v),
            "en" => own = Some(v),
            "k" => kind = names::Kind::parse(&v).unwrap_or(names::Kind::Other),
            "l" => langs = v.split(',').filter_map(names::Lang::parse).collect(),
            "at" => {
                let p: Vec<f64> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                if let (Some(n), true) = (name.take(), p.len() == 2) {
                    let d = s.names.display(
                        std::mem::replace(&mut kind, names::Kind::Other),
                        &n,
                        own.take().as_deref().filter(|x| !x.is_empty()),
                        &std::mem::take(&mut langs),
                        p[0],
                        p[1],
                    );
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

pub use inview::Region;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_results_are_capped() {
        let dir = std::env::temp_dir().join(format!("bench-cap-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        for i in 0..BENCH_MAX_FILES {
            save_bench(&dir, &format!("r{i}"), b"{}").unwrap();
        }
        // full: a new one refused, one saved again under its own name still replaced
        assert!(matches!(save_bench(&dir, "new", b"{}"), Err(BenchSave::Full(w)) if w.contains("at most 500")));
        save_bench(&dir, "r0", b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(dir.join("r0.json")).unwrap(), b"{\"a\":1}");
        std::fs::remove_dir_all(&dir).ok();
        // bytes: past BENCH_MAX_BYTES refused
        let big = vec![b' '; (BENCH_MAX_BYTES / 2 + 1) as usize];
        save_bench(&dir, "a", &big).unwrap();
        assert!(matches!(save_bench(&dir, "b", &big), Err(BenchSave::Full(_))));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_default_port_is_not_8080() {
        // Other tools default to 8080; the docs, the status app, the bench and the dev proxy repeat this number.
        assert_eq!(pipeline::net::DEFAULT_PORT, 7311);
    }

    #[test]
    fn help_and_unknown_options_never_start_it() {
        use clap::error::ErrorKind;
        let v = |a: &[&str]| {
            <Cli as clap::Parser>::try_parse_from(std::iter::once("server").chain(a.iter().copied()))
                .map_err(|e| e.kind())
        };
        assert!(v(&[]).is_ok());
        assert!(v(&["--port", "18150", "--home", "/tmp/h", "--root", "/tmp/r", "--no-mirror"]).is_ok());
        assert_eq!(v(&["--help"]).unwrap_err(), ErrorKind::DisplayHelp);
        assert_eq!(v(&["-h"]).unwrap_err(), ErrorKind::DisplayHelp);
        // An old run file's reserve: taken (and ignored), never a server that won't start.
        assert!(v(&["--web", "w", "--reserve-gb", "150"]).is_ok());
        assert_eq!(v(&["--verbose"]).unwrap_err(), ErrorKind::UnknownArgument);
        assert!(v(&["--home"]).is_err());
        assert!(v(&["--home", "--root", "/tmp/r"]).is_err());
        assert!(v(&["--mirror", "--no-mirror"]).is_err());
        assert!(v(&["--listen", "nowhere"]).is_err());
    }

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
    fn a_members_own_status_stands_in_for_a_lead_not_heard_from() {
        // 12 Oct: no lead for three days, the lead's heartbeat on the NAS frozen, and this member's
        // menu bar with it. Its own status, fresh, is shown in its place.
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let now = 1_791_300_000;
        std::fs::create_dir_all(nas.path().join("state")).unwrap();
        std::fs::create_dir_all(home.path().join("agent")).unwrap();
        std::fs::write(
            home.path().join("agent/helper.json"),
            serde_json::json!({ "host": "member", "beat": now - 5, "job": null }).to_string(),
        )
        .unwrap();
        // The lead heard from: its status, this member's beside it as a helper.
        std::fs::write(
            nas.path().join("state/status.json"),
            serde_json::json!({ "host": "lead", "beat": now - 100 }).to_string(),
        )
        .unwrap();
        let body = test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path())).build_status_at(now);
        assert_eq!((body["status"]["host"].as_str(), body["local"].as_bool()), (Some("lead"), Some(false)));
        assert_eq!(body["status"]["helpers"][0]["host"], "member");
        // Not for ten minutes: this member's own, as this Mac's.
        std::fs::write(
            nas.path().join("state/status.json"),
            serde_json::json!({ "host": "lead", "beat": now - 3 * 86_400 }).to_string(),
        )
        .unwrap();
        let body = test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path())).build_status_at(now);
        assert_eq!((body["status"]["host"].as_str(), body["local"].as_bool()), (Some("member"), Some(true)));
        assert!(body["status"]["helpers"].is_null() && body.get("pages").is_none(), "{body}");
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
    fn credits_of_older_catalogues_too() {
        let mut c = store::catalogue::Catalogue::new(1);
        // A catalogue made before catalogues carried credits: every credit the app knows.
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

    /// A catalogue `n` whose roads have a root, a lo pack (3/1/2) and a hi pack (6/8/16), the hi
    /// pack's content named after `hi`.
    fn roads_catalogue(n: u64, hi: &str) -> store::catalogue::Catalogue {
        let mut c = store::catalogue::Catalogue::new(n);
        let mut l = store::catalogue::Layer {
            encoding: "mvt".into(),
            minzoom: 0,
            maxzoom: 14,
            root: Some("layers/roads/root".into()),
            ..Default::default()
        };
        l.lo.insert("3/1/2".into(), "layers/roads/lo-3-1-2".into());
        l.hi.insert("6/8/16".into(), "layers/roads/hi-6-8-16".into());
        for (logical, file) in [
            ("layers/roads/root", "layers/roads/root.00000000000000aa.pack".to_string()),
            ("layers/roads/lo-3-1-2", "layers/roads/lo-3-1-2.00000000000000bb.pack".into()),
            ("layers/roads/hi-6-8-16", format!("layers/roads/hi-6-8-16.{hi}.pack")),
        ] {
            c.files.insert(logical.into(), store::catalogue::FileRef { file, ..Default::default() });
        }
        c.layers.insert("roads".into(), l);
        c
    }

    #[tokio::test]
    async fn only_the_current_packs_tokens_are_cached_for_good() {
        use tower::ServiceExt;
        let nas = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        store::catalogue::write_copy(&nas.path().join("catalog"), &roads_catalogue(1, "00000000000000c1")).unwrap();
        let s = test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        let packs = |s: &S| meta_json(s)["packs"]["roads"].clone();
        let p1 = packs(&s);
        let (root, lo, hi1) = (
            p1["root"].as_str().unwrap().to_string(),
            p1["lo"]["3/1/2"].as_str().unwrap().to_string(),
            p1["hi"]["6/8/16"].as_str().unwrap().to_string(),
        );
        assert_eq!(hi1.len(), 12);
        let cache = |s: &S, v: String| {
            let app = Router::new()
                .route("/tiles/roads/{z}/{x}/{y}", get(|| async { "tile" }))
                .layer(axum::middleware::from_fn_with_state(s.clone(), versioned_caching));
            async move {
                let r = app
                    .oneshot(
                        axum::http::Request::builder()
                            .uri(format!("/tiles/roads/9/130/260?v={v}"))
                            .body(axum::body::Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                r.headers()[header::CACHE_CONTROL].to_str().unwrap().to_string()
            }
        };
        const GOOD: &str = "public, max-age=31536000, immutable";
        assert_eq!(cache(&s, hi1.clone()).await, GOOD);
        // A catalogue that changed the hi pack alone: its old token is no longer cached; the others are.
        store::catalogue::write_copy(&nas.path().join("catalog"), &roads_catalogue(2, "00000000000000c2")).unwrap();
        assert!(s.data.refresh_catalogue());
        let p2 = packs(&s);
        let hi2 = p2["hi"]["6/8/16"].as_str().unwrap().to_string();
        assert_ne!(hi1, hi2);
        assert_eq!((p2["root"].as_str().unwrap(), p2["lo"]["3/1/2"].as_str().unwrap()), (root.as_str(), lo.as_str()));
        assert_eq!(cache(&s, hi1).await, "no-cache");
        for v in [hi2, lo, root] {
            assert_eq!(cache(&s, v).await, GOOD);
        }
        assert_eq!(cache(&s, "000000000000".into()).await, "no-cache");
        // A tile the server makes (terrain the catalogue hasn't: from ancestors, here flat) isn't
        // cached for good under a current token: a catalogue changing only its sources would leave it stale.
        let app = Router::new()
            .route("/tiles/terrain/{z}/{x}/{y}", get(terrain::terrain_tile))
            .layer(axum::middleware::from_fn_with_state(s.clone(), versioned_caching))
            .with_state(s.clone());
        let r = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/tiles/terrain/9/130/260?v={}", p2["hi"]["6/8/16"].as_str().unwrap()))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            (r.status(), r.headers()[header::CACHE_CONTROL].to_str().unwrap()),
            (axum::http::StatusCode::OK, "no-cache")
        );
    }
}
