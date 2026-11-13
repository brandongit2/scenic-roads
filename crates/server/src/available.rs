//! The data-available layers, served (docs/steps.md §14): the step `available`'s tiles hold, per
//! pixel, a set of the source extents' pieces (codecs::available); the score and each metric's
//! value are worked out here, as a tile is asked for, from the theme's `score.toml` and the pieces'
//! metrics in the extents' accepted version (docs/inputs.md §6.11), kept on this Mac as the
//! translations are (crate::accepted). So a weight tuned is drawn within a minute of the input check,
//! with nothing built again; and one function (`Theme::best`) colours a tile and answers a click.
//!
//! - `/tiles/available/<theme>/<set>/<layer>/z/x/y?avail=open|all` and, past 85.05°,
//!   `/tiles/polar-available/<pole>/<theme>/<set>/<layer>/z/x/y`: the tile as a PNG, each pixel the
//!   layer's ramp at its best value among the pieces there that pass the switch (`avail=open`, the
//!   default: those that download freely; `all`: every one), transparent where none does; 204 where
//!   no piece is. `layer`: `score`, or a metric of the theme's score. `set`: `best` (the best data
//!   any declared source has).
//! - `/api/available/<theme>/<set>?lng&lat&avail`: the pieces at a point, best first.
//! - `/api/available/<theme>`: the score as declared, each layer's ramp and stops, and the sources
//!   with their pieces counted by class (those of class `unknown` named: they're drawn nowhere).

use crate::cache::{self, Lru};
use crate::S;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use codecs::available::{self as fmt, Grid, Tile, PIXELS, SIDE};
use det::Det;
use inputs::inputs::extents::{self as ex, Scale, Score};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;

/// The sets served: the best data any declared source has. (The data built into the map, `built`,
/// is planned: task #1349.)
pub const SETS: [&str; 1] = ["best"];

/// The score layer's id; the others are the theme's metrics.
pub const SCORE_LAYER: &str = "score";

/// The ramp every layer is drawn with, over its 0–1 value (a perceptual sequential ramp, dark to
/// light: viridis's ends and middle).
const RAMP: [(f64, [u8; 3]); 5] =
    [(0.0, [68, 1, 84]), (0.25, [59, 82, 139]), (0.5, [33, 145, 140]), (0.75, [94, 201, 98]), (1.0, [253, 231, 37])];

/// The ramp's colour at `v` (0–1).
pub fn colour(v: f64) -> [u8; 3] {
    let v = v.clamp(0.0, 1.0);
    let k = RAMP.iter().position(|(at, _)| *at >= v).unwrap_or(RAMP.len() - 1).max(1);
    let ((a, ca), (b, cb)) = (RAMP[k - 1], RAMP[k]);
    let t = if b > a { (v - a) / (b - a) } else { 0.0 };
    [0, 1, 2].map(|i| (f64::from(ca[i]) + t * (f64::from(cb[i]) - f64::from(ca[i]))).round() as u8)
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// A piece as the server has it: what the click shows, and its score and values.
#[derive(Clone, Debug)]
pub struct PieceInfo {
    pub source: String,
    /// Its own properties (`piece`, `name`, `extent`, `availability`, the metrics, `stated`).
    pub props: Map<String, Value>,
    /// What its extent was read from: its block's reading in the `.geojson`'s `read`.
    pub read: Value,
    pub score: Option<f64>,
    pub values: BTreeMap<String, Option<f64>>,
}

impl PieceInfo {
    fn open(&self) -> bool {
        self.props.get("availability").and_then(Value::as_str) == Some("open")
    }
}

/// A theme's score and pieces, from the accepted version.
#[derive(Clone, Debug, Default)]
pub struct Theme {
    pub score: Option<Score>,
    /// By id (`<source>/<piece>`).
    pub pieces: HashMap<String, PieceInfo>,
    /// Each source's description (`name`, `credit`, `licence`, `licence_url`, `source`) and
    /// availability, by id.
    pub sources: BTreeMap<String, Map<String, Value>>,
    /// Each source's pieces with no extent (class `unknown`), by name.
    pub unknown: BTreeMap<String, Vec<String>>,
}

impl Theme {
    /// Whether `layer` is one of the theme's: the score or a metric.
    pub fn has_layer(&self, layer: &str) -> bool {
        self.score.as_ref().is_some_and(|s| layer == SCORE_LAYER || s.metrics.contains_key(layer))
    }

    /// The best value of `layer` among pieces `ids` that pass the switch (`all`: every one; else
    /// those whose availability is `open`): their best score, or their best 0–1 value of the metric
    /// (a piece not stating it takes no part). None where none does.
    pub fn best<'a>(&self, layer: &str, ids: impl IntoIterator<Item = &'a str>, all: bool) -> Option<f64> {
        ids.into_iter()
            .filter_map(|id| self.pieces.get(id))
            .filter(|p| all || p.open())
            .filter_map(|p| if layer == SCORE_LAYER { p.score } else { p.values.get(layer).copied().flatten() })
            .reduce(f64::max)
    }

    /// The pieces `ids` that pass the switch, best first (by score; a piece with none last), as the
    /// click shows them.
    pub fn listed<'a>(&self, ids: impl IntoIterator<Item = &'a str>, all: bool) -> Vec<Value> {
        let mut ps: Vec<(&str, &PieceInfo)> =
            ids.into_iter().filter_map(|id| Some((id, self.pieces.get(id)?))).filter(|(_, p)| all || p.open()).collect();
        ps.sort_by(|a, b| b.1.score.unwrap_or(-1.0).total_cmp(&a.1.score.unwrap_or(-1.0)).then(a.0.cmp(b.0)));
        ps.into_iter()
            .map(|(id, p)| {
                let src = self.sources.get(&p.source).cloned().unwrap_or_default();
                let metrics: Map<String, Value> = self
                    .score
                    .iter()
                    .flat_map(|s| s.metrics.keys())
                    .filter_map(|k| Some((k.clone(), p.props.get(k)?.clone())))
                    .collect();
                json!({
                    "id": id,
                    "source": p.source,
                    "piece": p.props.get("piece"),
                    "name": p.props.get("name"),
                    "source_name": src.get("name"),
                    "credit": src.get("credit"),
                    "licence": src.get("licence"),
                    "licence_url": src.get("licence_url"),
                    "link": src.get("source"),
                    "availability": p.props.get("availability"),
                    "extent": p.props.get("extent"),
                    "read": p.read,
                    "metrics": metrics,
                    "values": p.values,
                    "score": p.score,
                })
            })
            .collect()
    }

    /// `/api/available/<theme>`'s answer: the score as declared, the layers' ramps and stops, the
    /// sources.
    pub fn described(&self, theme: &str) -> Value {
        let Some(s) = &self.score else { return json!({"theme": theme, "layers": [], "sources": []}) };
        let ramp: Vec<Value> = RAMP.iter().map(|(at, c)| json!([at, hex(*c)])).collect();
        let score_stops: Vec<Value> = [0.0, 0.25, 0.5, 0.75, 1.0]
            .map(|v: f64| json!({"value": v, "label": format!("{v:.2}"), "colour": hex(colour(v))}))
            .into();
        let mut layers = vec![json!({"id": SCORE_LAYER, "label": "Score", "ramp": ramp, "stops": score_stops})];
        let mut metrics = Vec::new();
        // (The primary first, then the others by name.)
        let mut names: Vec<&String> = s.metrics.keys().collect();
        names.sort_by_key(|n| (**n != s.primary, n.as_str()));
        for name in names {
            let m = &s.metrics[name];
            let stops: Vec<Value> = match &m.scale {
                Scale::Values(w) => {
                    let mut w: Vec<(&String, &f64)> = w.iter().collect();
                    w.sort_by(|a, b| a.1.total_cmp(b.1));
                    w.iter().map(|(k, v)| json!({"value": v, "label": k, "colour": hex(colour(**v))})).collect()
                }
                Scale::Log { worst, best } | Scale::Linear { worst, best } => {
                    let log = matches!(m.scale, Scale::Log { .. });
                    [0.0, 0.25, 0.5, 0.75, 1.0]
                        .iter()
                        .map(|&v: &f64| {
                            let raw = if log { worst * (best / worst).dpowf(v) } else { worst + v * (best - worst) };
                            json!({"value": v, "label": format!("{}", (raw * 1000.0).round() / 1000.0), "colour": hex(colour(v))})
                        })
                        .collect()
                }
            };
            let (scale, worst, best, values) = match &m.scale {
                Scale::Log { worst, best } => ("log", Some(*worst), Some(*best), None),
                Scale::Linear { worst, best } => ("linear", Some(*worst), Some(*best), None),
                Scale::Values(w) => ("values", None, None, Some(w.clone())),
            };
            metrics.push(json!({"id": name, "label": m.label, "unit": m.unit, "weight": m.weight, "scale": scale,
                "worst": worst, "best": best, "values": values, "primary": *name == s.primary}));
            layers.push(json!({"id": name, "label": m.label, "unit": m.unit, "ramp": ramp, "stops": stops}));
        }
        let mut counted: BTreeMap<&str, BTreeMap<String, usize>> = BTreeMap::new();
        for p in self.pieces.values() {
            let class = p.props.get("extent").and_then(Value::as_str).unwrap_or("").to_string();
            *counted.entry(p.source.as_str()).or_default().entry(class).or_default() += 1;
        }
        let sources: Vec<Value> = self
            .sources
            .iter()
            .map(|(id, d)| {
                let classes = counted.get(id.as_str()).cloned().unwrap_or_default();
                json!({"id": id, "name": d.get("name"), "credit": d.get("credit"), "licence": d.get("licence"),
                    "licence_url": d.get("licence_url"), "link": d.get("source"), "availability": d.get("availability"),
                    "pieces": classes.values().sum::<usize>(), "classes": classes,
                    "unknown": self.unknown.get(id).cloned().unwrap_or_default()})
            })
            .collect();
        json!({"theme": theme, "sets": SETS, "primary": s.primary, "within": s.within, "share": s.share,
            "metrics": metrics, "layers": layers, "sources": sources})
    }
}

/// A theme's accepted files read: `score.toml`, and each source's declaration and pieces (by
/// inputs-folder path; an accepted version is the unit's guarantee of their shape).
pub fn theme_of(files: &BTreeMap<String, Vec<u8>>) -> Theme {
    let mut t = Theme::default();
    for (p, b) in files {
        match ex::kind(p) {
            ex::Kind::Score(_) => t.score = ex::score(b).ok(),
            ex::Kind::Declaration(_, id) => {
                let Ok(d) = std::str::from_utf8(b).map_err(|_| ()).and_then(|s| toml::from_str::<toml::Table>(s).map_err(|_| ()))
                else {
                    continue;
                };
                let m: Map<String, Value> = ["name", "credit", "licence", "licence_url", "source", "availability"]
                    .iter()
                    .filter_map(|k| Some((k.to_string(), serde_json::to_value(d.get(*k)?).ok()?)))
                    .collect();
                t.sources.insert(id.to_string(), m);
            }
            ex::Kind::Pieces(_, id) => {
                let Ok(v) = serde_json::from_slice::<Value>(b) else { continue };
                let reads: Vec<&Value> = v["read"].as_array().map(|r| r.iter().collect()).unwrap_or_default();
                for f in v["features"].as_array().into_iter().flatten() {
                    let Some(props) = f["properties"].as_object() else { continue };
                    let Some(piece) = props.get("piece").and_then(Value::as_str) else { continue };
                    let block = piece.split('/').next().unwrap_or(piece);
                    let read = reads.iter().find(|r| r["piece"].as_str() == Some(block)).map_or(Value::Null, |r| (*r).clone());
                    if f["geometry"].is_null() {
                        let label = props.get("name").and_then(Value::as_str).unwrap_or(piece);
                        t.unknown.entry(id.to_string()).or_default().push(label.to_string());
                    }
                    t.pieces.insert(
                        format!("{id}/{piece}"),
                        PieceInfo { source: id.to_string(), props: props.clone(), read, score: None, values: BTreeMap::new() },
                    );
                }
            }
            ex::Kind::Other => {}
        }
    }
    if let Some(s) = &t.score {
        for p in t.pieces.values_mut() {
            (p.score, p.values) = s.of(&p.props);
        }
    }
    t
}

/// The extents' accepted version, kept on this Mac.
pub struct Available {
    local: PathBuf,
    /// Its version (the index's content name; "" before one) and its themes.
    state: RwLock<Arc<(String, BTreeMap<String, Arc<Theme>>)>>,
}

impl Available {
    pub fn new(home: &std::path::Path) -> Arc<Available> {
        Arc::new(Available {
            local: crate::accepted::local_dir(home, ex::UNIT),
            state: RwLock::new(Arc::new((String::new(), BTreeMap::new()))),
        })
    }

    /// A theme, as the version kept says it.
    pub fn theme(&self, theme: &str) -> Option<Arc<Theme>> {
        self.state.read().unwrap().1.get(theme).cloned()
    }

    /// The version kept's hash ("" before one): every tile's colours change with it.
    pub fn version(&self) -> String {
        let v = self.state.read().unwrap().0.clone();
        inputs::inputs::hash_of(&v).unwrap_or("").to_string()
    }

    /// Reads the version kept here.
    fn load(&self) {
        let Some((version, files)) = inputs::inputs::served::files(&self.local) else { return };
        let mut by: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
        for (p, (local, _)) in files {
            if let (Some((theme, _)), Ok(b)) = (p.split_once('/'), std::fs::read(&local)) {
                by.entry(theme.to_string()).or_default().insert(p.clone(), b);
            }
        }
        let themes: BTreeMap<String, Arc<Theme>> = by.iter().map(|(t, f)| (t.clone(), Arc::new(theme_of(f)))).collect();
        eprintln!(
            "available: {} pieces in {} themes ({version})",
            themes.values().map(|t| t.pieces.len()).sum::<usize>(),
            themes.len()
        );
        *self.state.write().unwrap() = Arc::new((version, themes));
    }

    /// Sets what's served from `files` (by inputs-folder path; for tests).
    #[cfg(test)]
    pub fn set(&self, version: &str, files: &[(&str, &[u8])]) {
        let mut by: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
        for (p, b) in files {
            by.entry(p.split('/').next().unwrap().to_string()).or_default().insert(p.to_string(), b.to_vec());
        }
        let themes = by.iter().map(|(t, f)| (t.clone(), Arc::new(theme_of(f)))).collect();
        *self.state.write().unwrap() = Arc::new((version.to_string(), themes));
    }

    /// Keeps the copy here in step with the records (every minute while the map is in use).
    pub fn spawn(self: &Arc<Self>, data: Arc<crate::data::Data>) {
        let me = self.clone();
        std::thread::Builder::new()
            .name("available".into())
            .spawn(move || {
                me.load();
                let mut first = true;
                loop {
                    if first || crate::updater::in_use(600) {
                        match crate::accepted::synced(&data, ex::UNIT, &me.local) {
                            Ok(Some(inputs::inputs::served::Synced::Kept(true))) => me.load(),
                            Ok(_) => {}
                            Err(e) => eprintln!("available: {e:#}"),
                        }
                    }
                    first = false;
                    std::thread::sleep(Duration::from_secs(60));
                }
            })
            .ok();
    }
}

/// The layer a theme's set and grid are stored in.
fn stored_layer(theme: &str, grid: Grid) -> String {
    match grid {
        Grid::Mercator => fmt::layer(theme),
        Grid::Polar(p) => fmt::polar_layer(theme, p),
    }
}

/// Decoded stored tiles, by layer, tile and catalogue generation (a view's, and their ancestors').
type Decoded = Lru<(String, u8, u32, u32, u64), Option<Arc<Tile>>>;
static DECODED: LazyLock<Mutex<Decoded>> = LazyLock::new(|| Mutex::new(Lru::entries(4 * cache::VIEW_TILES as usize)));

/// Stored tile z/x/y of `layer`, decoded; None where it has none.
fn stored(s: &crate::AppState, layer: &str, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Tile>> {
    let g = s.data.generation.load(std::sync::atomic::Ordering::Relaxed);
    let key = (layer.to_string(), z, x, y, g);
    if let Some(t) = DECODED.lock().unwrap().get(&key) {
        return Ok(t.map(|t| (*t).clone()));
    }
    let t = match s.data.tile(layer, z, x, y)? {
        Some((b, _)) => Some(Arc::new(Tile::decode(b.bytes())?)),
        None => None,
    };
    DECODED.lock().unwrap().put(key, t.clone(), 0);
    Ok(t.map(|t| (*t).clone()))
}

/// Whether the switch lets every piece in (`avail=all`; else only those downloading freely).
fn all_of(q: Option<&str>) -> bool {
    q.is_some_and(|q| q.split('&').any(|kv| kv == "avail=all"))
}

/// A tile coloured: each pixel the ramp at its set's best value, transparent where none; None
/// where every pixel is.
pub fn paint(t: &Tile, theme: &Theme, layer: &str, all: bool) -> anyhow::Result<Option<Vec<u8>>> {
    let colours: Vec<Option<[u8; 3]>> = t
        .sets
        .iter()
        .map(|s| theme.best(layer, s.iter().map(|&k| t.ids[k as usize].as_str()), all).map(colour))
        .collect();
    // (A tile not stored is its ancestor's pixels over it, the ancestor's palette whole: a set
    // coloured may hold no pixel of it.)
    let mut rgba = vec![0u8; PIXELS * 4];
    let mut any = false;
    for (k, &p) in t.px.iter().enumerate() {
        if let Some(c) = colours[p as usize] {
            rgba[k * 4..k * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            any = true;
        }
    }
    if !any {
        return Ok(None);
    }
    let mut png = Vec::new();
    {
        let mut e = png::Encoder::new(&mut png, SIDE as u32, SIDE as u32);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::Fast);
        let mut w = e.write_header()?;
        w.write_image_data(&rgba)?;
    }
    Ok(Some(png))
}

/// Coloured tiles made here, by layer, theme layer, switch, tile and versions.
type Made = Lru<(String, String, bool, u8, u32, u32, u64, String), bytes::Bytes>;
static MADE: LazyLock<Mutex<Made>> = LazyLock::new(|| Mutex::new(Lru::new(64 << 20)));

#[allow(clippy::too_many_arguments)]
async fn tile(s: S, grid: Grid, theme: String, set: String, layer: String, z: u8, x: u32, y: u32, q: Option<String>, headers: HeaderMap) -> Response {
    let Some(th) = s.available.theme(&theme) else { return StatusCode::NOT_FOUND.into_response() };
    if !SETS.contains(&set.as_str()) || !th.has_layer(&layer) || z > fmt::maxz(grid) || x >> z != 0 || y >> z != 0 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let stl = stored_layer(&theme, grid);
    let all = all_of(q.as_deref());
    let g = s.data.generation.load(std::sync::atomic::Ordering::Relaxed);
    let uv = s.available.version();
    let etag = format!("\"av-{}-{uv}-{layer}-{}-{z}-{x}-{y}\"", s.data.layer_version(&stl), if all { "all" } else { "open" });
    let key = (stl.clone(), layer.clone(), all, z, x, y, g, uv);
    let v = cache::versioned(q.as_deref());
    let s2 = s.clone();
    cache::serve_made(&MADE, key, &etag, &headers, v, "image/png", format!("{stl} {layer} {z}/{x}/{y}"), move || {
        if !s2.data.catalogue().layers.contains_key(&stl) {
            return Ok(None);
        }
        let Some(t) = fmt::tile_at(grid, z, x, y, &mut |z, x, y| stored(&s2, &stl, z, x, y))? else { return Ok(None) };
        paint(&t, &th, &layer, all)
    })
    .await
    .into_response()
}

pub async fn mercator_tile(
    State(s): State<S>,
    Path((theme, set, layer, z, x, y)): Path<(String, String, String, u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    tile(s, Grid::Mercator, theme, set, layer, z, x, y, q, headers).await
}

pub async fn polar_tile(
    State(s): State<S>,
    Path((pole, theme, set, layer, z, x, y)): Path<(String, String, String, String, u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(p) = roadcore::polar::Pole::of_letter(&pole) else { return StatusCode::NOT_FOUND.into_response() };
    tile(s, Grid::Polar(p.letter()), theme, set, layer, z, x, y, q, headers).await
}

/// The finest pixel of the layers at a point: (grid, x, y), Web Mercator's within its tiles, a
/// pole's past them.
pub fn pixel_of(lng: f64, lat: f64) -> (Grid, u64, u64) {
    if lat.abs() <= roadcore::geo::MERCATOR_LAT {
        let (x, y) = roadcore::geo::merc(lng, lat);
        let w = (SIDE as f64) * f64::from(1u32 << fmt::MAXZ);
        let c = |v: f64| (v * w).floor().clamp(0.0, w - 1.0) as u64;
        return (Grid::Mercator, c(x), c(y));
    }
    let pole = if lat > 0.0 { roadcore::polar::Pole::North } else { roadcore::polar::Pole::South };
    let (gx, gy) = roadcore::polar::forward(pole, lng, lat);
    let p = roadcore::polar::pixel_m(fmt::POLAR_MAXZ);
    let w = (SIDE as f64) * f64::from(1u32 << fmt::POLAR_MAXZ);
    let c = |v: f64| v.floor().clamp(0.0, w - 1.0) as u64;
    (Grid::Polar(pole.letter()), c((gx + roadcore::polar::HALF) / p), c((roadcore::polar::HALF - gy) / p))
}

#[derive(serde::Deserialize)]
pub struct At {
    lng: f64,
    lat: f64,
    #[serde(default)]
    avail: Option<String>,
}

/// The pieces at a point, best first (`/api/available/<theme>/<set>?lng&lat&avail`).
pub async fn at(State(s): State<S>, Path((theme, set)): Path<(String, String)>, Query(q): Query<At>) -> Response {
    let Some(th) = s.available.theme(&theme) else { return StatusCode::NOT_FOUND.into_response() };
    if !SETS.contains(&set.as_str()) || !q.lng.is_finite() || !q.lat.is_finite() || q.lat.abs() > 90.0 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let all = q.avail.as_deref() == Some("all");
    let (lng, lat) = (q.lng, q.lat);
    let r = cache::blocking("available: a click", move || {
        let (grid, px, py) = pixel_of(lng, lat);
        let stl = stored_layer(&theme, grid);
        let ids = fmt::pieces_at(grid, px, py, &mut |z, x, y| stored(&s, &stl, z, x, y))?;
        Ok(json!({"lng": lng, "lat": lat, "avail": if all { "all" } else { "open" },
            "pieces": th.listed(ids.iter().map(String::as_str), all)}))
    })
    .await;
    match r {
        Ok(v) => ([(axum::http::header::CACHE_CONTROL, "no-cache")], axum::Json(v)).into_response(),
        Err(r) => r,
    }
}

/// The theme's score, layers and sources (`/api/available/<theme>`).
pub async fn about(State(s): State<S>, Path(theme): Path<String>) -> Response {
    match s.available.theme(&theme) {
        Some(t) => ([(axum::http::header::CACHE_CONTROL, "no-cache")], axum::Json(t.described(&theme))).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests;
