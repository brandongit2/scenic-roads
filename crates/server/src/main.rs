//! HTTP backend: road tiles, basemap, fonts, way details and elevation profiles.
//!
//! usage: server [--data data/build] [--web web/dist] [--fonts data/fonts] [--port 8080]

mod cache;
mod details;
mod drives;
mod rides;
mod terrain;
mod viewshed;

use anyhow::Result;
use axum::{
    extract::{Path, Query, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use roadcore::{archive::Archive, class, climb::ClimbRec, dist_m, flag, Array, DemSource, Ways, E7, NDEM};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

pub struct GridData {
    pub idx: roadcore::grid::GridIndex,
    pub terrain: roadcore::grid::Layer<i16>,
    pub canopy: Option<roadcore::grid::Layer<u8>>,
    pub class: Option<roadcore::grid::Layer<u8>>,
}

pub struct AppState {
    ways: Ways,
    terrain: Option<Archive>,
    /// Precomputed slope pyramid (see pipeline `slope`); tiles missing here are computed on the fly.
    slope: Option<Archive>,
    grid: Option<GridData>,
    drives: Option<drives::DriveIndex>,
    rails_ix: Option<rides::RailIndex>,
    /// Per-vertex scenic channels (for profiles).
    vch: Option<Array<[u8; roadcore::scenic::ch::N]>>,
    data_dir: PathBuf,
    strings: Vec<String>,
    elev: Array<i16>,
    grade: Array<u8>,
    src: Array<u8>,
    tiles: Archive,
    /// Passenger rail tiles (absent before the first build with rail).
    rails: Option<Archive>,
    /// Tree cover layer (`dem/trees.py`): cover, canopy height, leaf type; Terrarium WebP tiles.
    trees: [Option<Archive>; 3],
    climbs: Array<ClimbRec>,
    climb_geom: Array<[i32; 2]>,
    /// Per way: length of the whole road it belongs to, metres (pipeline `roads`).
    road_len: Option<Array<f32>>,
    meta: serde_json::Value,
    /// Endpoint coordinate → ways that start or end there.
    ends: HashMap<[i32; 2], Vec<u32>>,
    details: details::Details,
    /// The overlay layer files, gzipped once (cache.rs).
    packs: Arc<cache::Packs>,
    /// Roads' English names from OSM (dem/roadnames.py), by OSM way id: given with the road
    /// (name_en), the app shows it after the name in parentheses.
    road_en: HashMap<i64, String>,
}

pub type S = Arc<AppState>;

fn arg(name: &str, default: &str) -> String {
    let a: Vec<String> = std::env::args().collect();
    a.iter()
        .position(|x| x == name)
        .and_then(|i| a.get(i + 1).cloned())
        .unwrap_or_else(|| default.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let data = PathBuf::from(arg("--data", "data/build"));
    let web = PathBuf::from(arg("--web", "web/dist"));
    let fonts = PathBuf::from(arg("--fonts", "data/fonts"));
    let port: u16 = arg("--port", "8080").parse()?;
    let t0 = std::time::Instant::now();

    let ways = Ways::open(&data)?;
    let strings = roadcore::read_strings(&data)?;
    let mut ends: HashMap<[i32; 2], Vec<u32>> = HashMap::with_capacity(ways.ways().len() * 2);
    {
        let v = ways.verts();
        for (i, w) in ways.ways().iter().enumerate() {
            ends.entry(v[w.vstart as usize]).or_default().push(i as u32);
            ends.entry(v[(w.vstart + w.vcount as u64 - 1) as usize]).or_default().push(i as u32);
        }
    }
    let mut meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(data.join("roads.json"))?)?;
    // Build time of each data file the app fetches from. Tiles and layers are cached by the
    // browser, so the app puts these in their URLs: a rebuilt file gets new URLs.
    let versions: serde_json::Map<String, serde_json::Value> = [
        "roads.tiles", "rails.tiles", "terrain.tiles", "slope.tiles", "base.pmtiles", "ways.bin", "pois.json",
        "heritage.json", "special.json", "indigenous.json", "heritage-areas.json", "heritage-sources.json", "ferries.json",
        "ferry-lines.json", "trees-cover.tiles", "trees-height.tiles", "trees-leaf.tiles", "rail-freq.bin",
        "details-poi.jsonl", "details-heritage.jsonl", "details-harea.jsonl", "details-special.jsonl", "details-indigenous.jsonl",
        "details-park.jsonl", "peaks.json", "props-heritage.jsonl", "layer-summary.json", "layer-pois.json", "layer-heritage.json",
        "layer-special.json", "layer-indigenous.json", "layer-heritage-areas.json", "stations.json", "whs-shapes.json", "names-en.json",
        "labels.pmtiles", "road-en.json",
    ]
    .iter()
    .filter_map(|f| {
        let t = std::fs::metadata(data.join(f)).and_then(|m| m.modified()).ok()?;
        let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        Some((f.to_string(), serde_json::Value::from(secs)))
    })
    .collect();
    let mut versions = versions;
    // Overlay files split per kind (layer-pois-<kind>.json …): every layer file.
    for e in std::fs::read_dir(&data).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("layer-") && n.ends_with(".json") && !versions.contains_key(&n) {
            if let Some(secs) = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
                versions.insert(n, serde_json::Value::from(secs.as_secs()));
            }
        }
    }
    // Basemap parts: regions added after base.pmtiles was built, one archive each (see Makefile).
    let mut parts: Vec<String> = std::fs::read_dir(data.join("base-parts"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".pmtiles")).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    parts.sort();
    for p in &parts {
        if let Some(secs) = std::fs::metadata(data.join("base-parts").join(format!("{p}.pmtiles")))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        {
            versions.insert(format!("base-parts/{p}.pmtiles"), serde_json::Value::from(secs.as_secs()));
        }
    }
    if let Some(m) = meta.as_object_mut() {
        m.insert("versions".into(), serde_json::Value::Object(versions));
        m.insert("baseParts".into(), serde_json::json!(parts));
        // The basemap's labels with their English (names.py patch; Makefile): drawn from their own
        // archive when there is one.
        m.insert("labels".into(), serde_json::json!(data.join("labels.pmtiles").exists()));
    }
    let grid = roadcore::grid::GridIndex::load(&data).ok().and_then(|idx| {
        Some(GridData {
            idx,
            terrain: roadcore::grid::Layer::open(&data.join("grid.terrain.i16")).ok()?,
            canopy: roadcore::grid::Layer::open(&data.join("grid.canopy.u8")).ok(),
            class: roadcore::grid::Layer::open(&data.join("grid.class.u8")).ok(),
        })
    });
    let drives = match drives::DriveIndex::open(&data, &ways) {
        Ok(d) => Some(d),
        Err(e) => {
            eprintln!("scenic drives unavailable: {e}");
            None
        }
    };
    let rails_ix = match rides::RailIndex::open(&data, &ways, &strings) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("rail lines unavailable: {e}");
            None
        }
    };
    let vch = Array::open(&data.join("scenic.u8")).ok().filter(|a: &Array<[u8; roadcore::scenic::ch::N]>| a.get().len() == ways.verts().len());
    let state = Arc::new(AppState {
        terrain: Archive::open(&data.join("terrain.tiles")).ok(),
        slope: Archive::open(&data.join("slope.tiles")).ok(),
        grid,
        drives,
        rails_ix,
        vch,
        data_dir: data.clone(),
        elev: Array::open(&data.join("final.i16"))?,
        grade: Array::open(&data.join("grade.u8"))?,
        src: Array::open(&data.join("src.u8"))?,
        tiles: Archive::open(&data.join("roads.tiles"))?,
        trees: TREE_VARS.map(|v| Archive::open(&data.join(format!("trees-{v}.tiles"))).ok()),
        rails: Archive::open(&data.join("rails.tiles")).ok(),
        climbs: Array::open(&data.join("climbs.bin"))?,
        climb_geom: Array::open(&data.join("climbs.geom"))?,
        road_len: Array::open(&data.join("roadlen.f32")).ok().filter(|a: &Array<f32>| a.get().len() == ways.ways().len()),
        details: details::Details::load(&data),
        road_en: std::fs::read(data.join("road-en.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<HashMap<String, String>>(&b).ok())
            .map(|m| m.into_iter().filter_map(|(k, v)| k.parse().ok().map(|k| (k, v))).collect())
            .unwrap_or_default(),
        packs: Arc::new(cache::Packs::default()),
        ways,
        strings,
        meta,
        ends,
    });
    eprintln!("loaded {} ways in {:.1?}", state.ways.ways().len(), t0.elapsed());
    // Compress the overlay files now, so the first page load finds them ready.
    state.packs.warm(
        std::fs::read_dir(&data)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        n.starts_with("layer-") && n.ends_with(".json")
                            || matches!(n, "ferries.json" | "ferry-lines.json" | "stations.json" | "whs-shapes.json" | "heritage-sources.json" | "names-en.json")
                    })
                    .collect()
            })
            .unwrap_or_default(),
    );

    let app = Router::new()
        .route("/tiles/roads/{z}/{x}/{y}", get(road_tile))
        .route("/tiles/rails/{z}/{x}/{y}", get(rail_tile))
        .route("/tiles/trees/{var}/{z}/{x}/{y}", get(tree_tile))
        .route("/api/railfreq", get(rail_freq_h))
        .route("/api/detail/{layer}/{i}", get(details::detail))
        .route("/api/park", get(details::park))
        .route("/api/meta", get(meta_h))
        .route("/api/way/{idx}", get(way_h))
        .route("/api/profile/{idx}", get(profile_h))
        .route("/api/road/{idx}", get(road_h))
        .route("/api/climbs", get(climbs_h))
        .route("/api/viewshed", get(viewshed::viewshed))
        .route("/api/drives", get(drives::drives))
        .route("/api/rides", get(rides::rides))
        .route("/api/raillines", get(rides::lines))
        .route("/api/layer/{name}", get(layer_h))
        .route("/api/ping", get(|| async { ([(header::CACHE_CONTROL, "no-store")], "ok") }))
        .route("/tiles/terrain/{z}/{x}/{y}", get(terrain::terrain_tile))
        .route("/tiles/slope/{z}/{x}/{y}", get(terrain::slope_tile))
        .route_service("/tiles/base.pmtiles", ServeFile::new(data.join("base.pmtiles")))
        .route_service("/tiles/labels.pmtiles", ServeFile::new(data.join("labels.pmtiles")))
        .nest_service("/tiles/base-parts", ServeDir::new(data.join("base-parts")))
        .nest_service(
            "/fonts",
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=2592000")))
                .service(ServeDir::new(fonts)),
        )
        // App files revalidate on every load (cheap 304s) so a rebuilt frontend is picked up.
        .fallback_service(
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, HeaderValue::from_static("no-cache")))
                .service(ServeDir::new(&web).fallback(ServeFile::new(web.join("index.html")))),
        )
        .layer(tower_http::compression::CompressionLayer::new().gzip(true))
        // Versioned URLs (?v=build time) never change: cached for good, whatever served them.
        .layer(axum::middleware::from_fn(|req: axum::extract::Request, next: axum::middleware::Next| async move {
            let v = cache::versioned(req.uri().query());
            let mut res = next.run(req).await;
            if v && res.status().is_success() {
                res.headers_mut().insert(header::CACHE_CONTROL, cache::cache_control(true, ""));
            }
            res
        }))
        // Data from other host names of this machine (the app spreads its downloads over several:
        // the browser's six connections per host would otherwise queue them one kind at a time).
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Also on IPv6 loopback, which "localhost" names may resolve to first.
    if let Ok(l6) = tokio::net::TcpListener::bind(std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port))).await {
        let app6 = app.clone();
        tokio::spawn(async move { axum::serve(l6, app6).await });
    }
    axum::serve(listener, app).await?;
    Ok(())
}

async fn road_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery) -> Response {
    gz_tile(s.tiles.get(z, x, y), cache::versioned(q.as_deref()))
}

async fn rail_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery) -> Response {
    gz_tile(s.rails.as_ref().and_then(|a| a.get(z, x, y)), cache::versioned(q.as_deref()))
}

/// Rail service frequency per way (pipeline `railfreq`): (u32 way, f32 trains a day each way).
async fn rail_freq_h(State(s): State<S>, RawQuery(q): RawQuery) -> Response {
    match tokio::fs::read(s.data_dir.join("rail-freq.bin")).await {
        Ok(b) => ([(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")), (header::CACHE_CONTROL, cache::cache_control(cache::versioned(q.as_deref()), "public, max-age=86400"))], b).into_response(),
        Err(_) => StatusCode::NO_CONTENT.into_response(),
    }
}

const TREE_VARS: [&str; 3] = ["cover", "height", "leaf"];

/// Tree cover tiles: stored as served (lossless WebP). Missing tiles have nothing to show.
async fn tree_tile(State(s): State<S>, Path((var, z, x, y)): Path<(String, u8, u32, u32)>, RawQuery(q): RawQuery) -> Response {
    let Some(i) = TREE_VARS.iter().position(|v| *v == var) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match s.trees[i].as_ref().and_then(|a| a.get(z, x, y)) {
        Some(b) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("image/webp")),
                (header::CACHE_CONTROL, cache::cache_control(cache::versioned(q.as_deref()), "public, max-age=86400")),
            ],
            b.to_vec(),
        )
            .into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

fn gz_tile(t: Option<&[u8]>, versioned: bool) -> Response {
    match t {
        Some(b) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
                (header::CONTENT_ENCODING, HeaderValue::from_static("gzip")),
                (header::CACHE_CONTROL, cache::cache_control(versioned, "public, max-age=86400")),
            ],
            b.to_vec(),
        )
            .into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

/// Static GeoJSON layers produced by the pipeline (gzipped once, cache.rs).
async fn layer_h(State(s): State<S>, Path(name): Path<String>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let file = match name.as_str() {
        "pois" => "pois.json".to_string(),
        "heritage" => "heritage.json".to_string(),
        "special" => "special.json".to_string(),
        "indigenous" => "indigenous.json".to_string(),
        "heritage-areas" => "heritage-areas.json".to_string(),
        "sources" => "heritage-sources.json".to_string(),
        "ferries" => "ferries.json".to_string(),
        "ferry-lines" => "ferry-lines.json".to_string(),
        "summary" => "layer-summary.json".to_string(),
        "stations" => "stations.json".to_string(),
        "whs-shapes" => "whs-shapes.json".to_string(),
        "summits" => "summits.json".to_string(),
        // English for non-English names (dem/names.py).
        "names-en" => "names-en.json".to_string(),
        // Stops & sights of one kind (dem/layers.py splits them: most of the file is peaks).
        n if n.starts_with("pois-") && n[5..].chars().all(|c| c.is_ascii_lowercase() || c == '_') => format!("{n}.json"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    // The overlays as the map draws them (dem/layers.py: lean properties, draw order, simplified
    // polygons), when built.
    let lean = s.data_dir.join(format!("layer-{file}"));
    let path = if lean.exists() { lean } else { s.data_dir.join(&file) };
    match cache::respond(&s.packs, &path, "application/geo+json", &headers, cache::versioned(q.as_deref())).await {
        Some(r) => r,
        None => ([(header::CONTENT_TYPE, "application/geo+json")], r#"{"type":"FeatureCollection","features":[]}"#).into_response(),
    }
}

async fn meta_h(State(s): State<S>) -> Json<serde_json::Value> {
    Json(s.meta.clone())
}

#[derive(Serialize)]
struct WayInfo {
    idx: u32,
    osm_id: i64,
    class: &'static str,
    name: String,
    /// Its English name (OSM name:en), when it has one that isn't just the name.
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
    /// Designated scenic route this road is part of; rail: the services using the track.
    route: String,
    /// Rail: service groups using the track (class names), line colour (#rrggbb).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rail: Vec<&'static str>,
    #[serde(skip_serializing_if = "String::is_empty")]
    colour: String,
    length_m: f64,
    elev_min: f32,
    elev_max: f32,
    /// Share of vertices per DEM source.
    sources: Vec<(String, f32)>,
}

fn way_info(s: &AppState, idx: u32) -> Option<WayInfo> {
    let w = s.ways.ways().get(idx as usize)?;
    let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
    let v = &s.ways.verts()[r.clone()];
    let e = &s.elev.get()[r.clone()];
    let len: f64 = v
        .windows(2)
        .map(|p| dist_m(p[0][0] as f64 * E7, p[0][1] as f64 * E7, p[1][0] as f64 * E7, p[1][1] as f64 * E7))
        .sum();
    let mut counts = [0u32; NDEM];
    for &c in &s.src.get()[r] {
        counts[(c as usize).min(NDEM - 1)] += 1;
    }
    let n = w.vcount as f32;
    let sources = (1..NDEM)
        .filter(|&k| counts[k] > 0)
        .map(|k| (DemSource::label(k as u8).to_string(), counts[k] as f32 / n))
        .collect();
    let st = |i: u32| s.strings.get(i as usize).cloned().unwrap_or_default();
    Some(WayInfo {
        idx,
        osm_id: w.id,
        class: class::NAMES[w.class as usize],
        name: st(w.name),
        name_en: s.road_en.get(&w.id).cloned().unwrap_or_default(),
        r#ref: st(w.ref_),
        surface: st(w.surface),
        maxspeed: w.maxspeed,
        lanes: w.lanes,
        link: w.flags & flag::LINK != 0,
        bridge: w.flags & flag::BRIDGE != 0,
        tunnel: w.flags & flag::TUNNEL != 0,
        unpaved: w.flags & flag::UNPAVED != 0,
        oneway: w.flags & flag::ONEWAY != 0,
        toll: w.flags & flag::TOLL != 0,
        covered: w.flags & flag::COVERED != 0,
        route: st(w.route),
        rail: (0..5).filter(|k| w.rail >> k & 1 == 1).map(|k| class::NAMES[class::TRAM as usize + k]).collect(),
        colour: if w.colour != 0 { format!("#{:06x}", w.colour & 0xff_ffff) } else { String::new() },
        length_m: len,
        elev_min: e.iter().copied().min().unwrap_or(0) as f32 / 10.0,
        elev_max: e.iter().copied().max().unwrap_or(0) as f32 / 10.0,
        sources,
    })
}

async fn way_h(State(s): State<S>, Path(idx): Path<u32>) -> Response {
    match way_info(&s, idx) {
        Some(w) => ([(header::CACHE_CONTROL, "public, max-age=86400")], Json(w)).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ---- profiles -----------------------------------------------------------------------

/// What makes two ways "the same road": shared route ref, else shared name,
/// else (unnamed) same class.
#[derive(PartialEq)]
enum RoadKey {
    Refs(Vec<String>),
    Name(String),
    Unnamed(u8),
}

fn road_key(s: &AppState, i: usize) -> RoadKey {
    let w = &s.ways.ways()[i];
    let r = &s.strings[w.ref_ as usize];
    let n = &s.strings[w.name as usize];
    if !r.is_empty() {
        RoadKey::Refs(r.split(';').map(|x| x.trim().to_string()).collect())
    } else if !n.is_empty() {
        RoadKey::Name(n.clone())
    } else {
        RoadKey::Unnamed(w.class)
    }
}

fn same_road(key: &RoadKey, s: &AppState, j: usize) -> bool {
    let w = &s.ways.ways()[j];
    match key {
        RoadKey::Refs(refs) => {
            let r = &s.strings[w.ref_ as usize];
            !r.is_empty() && r.split(';').any(|x| refs.iter().any(|y| y == x.trim()))
        }
        RoadKey::Name(n) => &s.strings[w.name as usize] == n,
        RoadKey::Unnamed(c) => w.name == 0 && w.ref_ == 0 && w.class == *c,
    }
}

/// Oriented way: (index, reversed).
type OWay = (u32, bool);

fn way_pts(s: &AppState, ow: OWay) -> Vec<usize> {
    let w = &s.ways.ways()[ow.0 as usize];
    let r = w.vstart as usize..(w.vstart + w.vcount as u64) as usize;
    if ow.1 { r.rev().collect() } else { r.collect() }
}

fn heading(v: &[[i32; 2]], a: usize, b: usize) -> f64 {
    let (x0, y0) = (v[a][0] as f64 * E7, v[a][1] as f64 * E7);
    let (x1, y1) = (v[b][0] as f64 * E7, v[b][1] as f64 * E7);
    (y1 - y0).atan2((x1 - x0) * y0.to_radians().cos())
}

/// Walk from vertex `last` (arrived from `prev`) along ways of the same road, taking the
/// straightest continuation at each junction. `backwards` = walking against the direction
/// of travel, which flips which way a oneway may be entered. Returned ways are oriented in
/// walking order.
fn extend(s: &AppState, key: &RoadKey, used: &mut HashSet<u32>, mut last: usize, mut prev: usize, max_len_m: f64, backwards: bool) -> Vec<OWay> {
    let v = s.ways.verts();
    let mut out = Vec::new();
    let mut total = 0.0;
    loop {
        let Some(cands) = s.ends.get(&v[last]) else { break };
        let h_in = heading(v, prev, last);
        let mut best: Option<(f64, OWay)> = None;
        for &c in cands {
            if used.contains(&c) || !same_road(key, s, c as usize) {
                continue;
            }
            let w = &s.ways.ways()[c as usize];
            let first = w.vstart as usize;
            let lastv = (w.vstart + w.vcount as u64 - 1) as usize;
            let rev = v[first] != v[last];
            // Don't drive a oneway the wrong way (that's the other carriageway).
            if w.flags & flag::ONEWAY != 0 && rev != backwards {
                continue;
            }
            let (a, b) = if rev { (lastv, lastv - 1) } else { (first, first + 1) };
            let mut turn = (heading(v, a, b) - h_in).abs();
            if turn > std::f64::consts::PI {
                turn = 2.0 * std::f64::consts::PI - turn;
            }
            if turn > 100f64.to_radians() {
                continue;
            }
            if best.map_or(true, |(t, _)| turn < t) {
                best = Some((turn, (c, rev)));
            }
        }
        let Some((_, ow)) = best else { break };
        used.insert(ow.0);
        let pts = way_pts(s, ow);
        for p in pts.windows(2) {
            total += dist_m(v[p[0]][0] as f64 * E7, v[p[0]][1] as f64 * E7, v[p[1]][0] as f64 * E7, v[p[1]][1] as f64 * E7);
        }
        last = *pts.last().unwrap();
        prev = pts[pts.len() - 2];
        out.push(ow);
        if total > max_len_m || out.len() > 50_000 {
            break;
        }
    }
    out
}

#[derive(Serialize)]
struct Profile {
    way: WayInfo,
    ways: Vec<u32>,
    /// Flattened [lon, lat] pairs of the displayed geometry (simplified).
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
    /// Scenic channels per displayed point (roadcore::scenic::ch), when available.
    ch: Vec<[u8; roadcore::scenic::ch::N]>,
}

async fn profile_h(State(s): State<S>, Path(idx): Path<u32>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || build_profile(&s2, idx)).await {
        Ok(Some(p)) => Json(p).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The whole road a way belongs to: the ways of the same road continuing from it both ways,
/// oriented in order (see `extend`), and whether the walk was cut short.
fn road_chain(s: &AppState, idx: u32) -> Option<(Vec<OWay>, bool)> {
    let w0 = s.ways.ways().get(idx as usize)?;
    let key = road_key(s, idx as usize);
    let mut used: HashSet<u32> = HashSet::from([idx]);
    let first = w0.vstart as usize;
    let lastv = (w0.vstart + w0.vcount as u64 - 1) as usize;
    const MAX_M: f64 = 400_000.0;
    let fwd = extend(s, &key, &mut used, lastv, lastv - 1, MAX_M, false);
    let back = extend(s, &key, &mut used, first, first + 1, MAX_M, true);
    let truncated = fwd.len() >= 50_000 || back.len() >= 50_000;
    let mut chain: Vec<OWay> = back.into_iter().rev().map(|(i, r)| (i, !r)).collect();
    chain.push((idx, false));
    chain.extend(fwd);
    Some((chain, truncated))
}

/// Way ids of the whole road a way belongs to (for highlighting it on hover).
async fn road_h(State(s): State<S>, Path(idx): Path<u32>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || road_chain(&s2, idx)).await {
        Ok(Some((chain, _))) => ([(header::CACHE_CONTROL, "public, max-age=86400")], Json(chain.iter().map(|o| o.0).collect::<Vec<u32>>())).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn build_profile(s: &AppState, idx: u32) -> Option<Profile> {
    let info = way_info(s, idx)?;
    let v = s.ways.verts();
    let (chain, truncated) = road_chain(s, idx)?;

    // Concatenate vertex indices along the chain.
    let mut vi: Vec<usize> = Vec::new();
    for ow in &chain {
        let pts = way_pts(s, *ow);
        let skip = if vi.last().is_some_and(|&l| v[l] == v[pts[0]]) { 1 } else { 0 };
        vi.extend_from_slice(&pts[skip..]);
    }
    let el = s.elev.get();
    let gr = s.grade.get();
    let src = s.src.get();
    let mut dist = Vec::with_capacity(vi.len());
    let mut acc = 0f64;
    let (mut climb, mut descent) = (0f64, 0f64);
    let mut src_len = [0f64; NDEM];
    dist.push(0.0);
    for k in 1..vi.len() {
        let (a, b) = (vi[k - 1], vi[k]);
        let d = dist_m(v[a][0] as f64 * E7, v[a][1] as f64 * E7, v[b][0] as f64 * E7, v[b][1] as f64 * E7);
        acc += d;
        dist.push(acc);
        let de = (el[b] - el[a]) as f64 / 10.0;
        if de > 0.0 { climb += de } else { descent -= de }
        src_len[(src[b] as usize).min(NDEM - 1)] += d;
    }
    let (mut emin, mut emax, mut gmax) = (i16::MAX, i16::MIN, 0u8);
    for &i in &vi {
        emin = emin.min(el[i]);
        emax = emax.max(el[i]);
        gmax = gmax.max(gr[i]);
    }
    // Downsample for transport: keep ~1 vertex per 10 m, plus local extrema.
    let step = (acc / 20000.0).max(10.0);
    let mut keep = Vec::new();
    let mut next = 0.0;
    for k in 0..vi.len() {
        let is_ext = k > 0 && k + 1 < vi.len() && {
            let (p, c, n) = (el[vi[k - 1]], el[vi[k]], el[vi[k + 1]]);
            (c > p && c >= n) || (c < p && c <= n)
        };
        if dist[k] >= next || k + 1 == vi.len() || (is_ext && step > 10.0 && (dist[k] - dist[*keep.last().unwrap_or(&0)]) > 2.0) {
            keep.push(k);
            next = dist[k] + step;
        }
    }
    let total = acc.max(1e-9);
    Some(Profile {
        way: info,
        ways: chain.iter().map(|o| o.0).collect(),
        coords: keep.iter().map(|&k| [v[vi[k]][0] as f64 * E7, v[vi[k]][1] as f64 * E7]).collect(),
        dist: keep.iter().map(|&k| dist[k] as f32).collect(),
        elev: keep.iter().map(|&k| el[vi[k]] as f32 / 10.0).collect(),
        grade: keep.iter().map(|&k| gr[vi[k]] as f32 / 2.0).collect(),
        length_m: acc,
        elev_min: emin as f32 / 10.0,
        elev_max: emax as f32 / 10.0,
        climb_m: climb,
        descent_m: descent,
        max_grade: gmax as f32 / 2.0,
        avg_grade: ((climb + descent) / total * 100.0) as f32,
        sources: (1..NDEM)
            .filter(|&k| src_len[k] > 0.0)
            .map(|k| (DemSource::label(k as u8).to_string(), src_len[k] / total))
            .collect(),
        truncated,
        ch: s.vch.as_ref().map(|a| keep.iter().map(|&k| a.get()[vi[k]]).collect()).unwrap_or_default(),
    })
}

// ---- the area in view -----------------------------------------------------------------

/// The part of the map in view: a bounding box and, when the client sends it, the outline of the
/// ground on screen. On a tilted globe the bounding box of a view that reaches a pole spans every
/// longitude, far more than is on screen (North America's view would take in Europe).
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

// ---- climbs -------------------------------------------------------------------------

#[derive(Deserialize)]
struct ClimbQuery {
    /// west,south,east,north
    bbox: String,
    /// Outline of the ground in view (see `Region`).
    #[serde(default)]
    poly: Option<String>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    /// Bitmask of visible road classes.
    #[serde(default)]
    classes: Option<u32>,
    /// Bit 0 paved, bit 1 unpaved.
    #[serde(default)]
    surface: Option<u8>,
    /// Bit 0 toll-free, bit 1 toll.
    #[serde(default)]
    toll: Option<u8>,
    /// Classes (bits) whose unnamed roads (no name, no ref) are left out.
    #[serde(default)]
    unnamed: Option<u32>,
    /// Whole-road length filter, metres (0 = no limit).
    #[serde(default)]
    lmin: Option<f32>,
    #[serde(default)]
    lmax: Option<f32>,
}

impl AppState {
    /// The way's whole road is within the length filter [lmin, lmax] (m; 0 = no limit).
    pub fn road_len_ok(&self, way: u32, lmin: Option<f32>, lmax: Option<f32>) -> bool {
        let (lo, hi) = (lmin.unwrap_or(0.0), lmax.filter(|&v| v > 0.0).unwrap_or(f32::INFINITY));
        if lo <= 0.0 && hi == f32::INFINITY {
            return true;
        }
        let Some(rl) = self.road_len.as_ref().and_then(|a| a.get().get(way as usize).copied()) else { return true };
        // Tiles carry whole metres.
        let r = rl.round();
        r >= lo && r <= hi
    }
}

#[derive(Serialize)]
struct ClimbOut {
    way: u32,
    name: String,
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

async fn climbs_h(State(s): State<S>, Query(q): Query<ClimbQuery>) -> Response {
    let Some(region) = Region::parse(&q.bbox, q.poly.as_deref()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let classes = q.classes.unwrap_or(u32::MAX);
    let surface = q.surface.unwrap_or(3);
    let toll = q.toll.unwrap_or(3);
    let unnamed = q.unnamed.unwrap_or(0);
    let mut hits: Vec<(f32, &ClimbRec)> = s
        .climbs
        .get()
        .iter()
        .filter(|c| {
            region.contains(c.mid[0], c.mid[1])
                && (classes >> c.class) & 1 == 1
                && (surface >> (c.unpaved & 1)) & 1 == 1
                && (toll >> (s.ways.ways()[c.label_way as usize].flags & flag::TOLL != 0) as u8) & 1 == 1
                && !((unnamed >> c.class) & 1 == 1 && {
                    let lw = &s.ways.ways()[c.label_way as usize];
                    lw.name == 0 && lw.ref_ == 0
                })
                && s.road_len_ok(c.label_way, q.lmin, q.lmax)
        })
        .map(|c| {
            let avg = c.gain_m / c.length_m.max(1.0);
            let key = match q.sort.as_deref() {
                Some("grade") => avg,
                // FIETS-style difficulty: gain² / length.
                Some("score") => c.gain_m * c.gain_m / c.length_m.max(1.0),
                _ => c.gain_m,
            };
            (key, c)
        })
        .collect();
    let total = hits.len();
    let limit = q.limit.unwrap_or(25).min(100);
    if hits.len() > limit {
        hits.select_nth_unstable_by(limit, |a, b| b.0.total_cmp(&a.0));
        hits.truncate(limit);
    }
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    let geom = s.climb_geom.get();
    let climbs = hits
        .into_iter()
        .map(|(_, c)| {
            let lw = &s.ways.ways()[c.label_way as usize];
            ClimbOut {
                way: c.way,
                name: s.strings[lw.name as usize].clone(),
                name_en: s.road_en.get(&lw.id).cloned().unwrap_or_default(),
                r#ref: s.strings[lw.ref_ as usize].clone(),
                class: class::NAMES[c.class as usize],
                gain_m: c.gain_m,
                length_m: c.length_m,
                avg_grade: c.gain_m / c.length_m.max(1.0) * 100.0,
                max_grade: c.max_grade,
                start_elev: c.start_elev,
                top_elev: c.top_elev,
                unpaved: c.unpaved != 0,
                geom: geom[c.geom_start as usize..(c.geom_start + c.geom_count) as usize]
                    .iter()
                    .map(|p| [p[0] as f64 * E7, p[1] as f64 * E7])
                    .collect(),
            }
        })
        .collect();
    Json(ClimbList { total, climbs }).into_response()
}
