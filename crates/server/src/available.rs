//! The data-available layers, served (docs/steps.md §14): the step `available`'s tiles hold, per
//! pixel, a set of the source extents' pieces (codecs::available); the score, the best piece and
//! each layer's value are worked out here, as a tile is asked for, from the theme's `score.toml`
//! and the pieces' metrics in the extents' accepted version (docs/inputs.md §6.11), kept on this
//! Mac as the translations are (crate::accepted). So a weight tuned is drawn within a minute of the
//! input check, with nothing built again. The colours are the app's: a tile carries values.
//!
//! - `/tiles/available/<theme>/<set>/<layer>/z/x/y?avail=open|all` and, past 85.05°,
//!   `/tiles/polar-available/<pole>/<theme>/<set>/<layer>/z/x/y`: the tile as a PNG, each pixel the
//!   layer's value of the best piece there that passes the switch (`avail=open`, the default:
//!   those that download freely; `all`: every one; `Theme::best`), Terrarium-coded (`code`),
//!   transparent where none does; 204 where no piece is. `layer`: `score` (the best piece, at its
//!   primary's own value: its grid spacing), or another metric of the theme's score. `set`: `best`
//!   (the best data any declared source has).
//! - `/api/available/<theme>/<set>?lng&lat&avail`: the pieces at a point, best first, and the one
//!   each layer draws there (`painted`).
//! - `/api/available/<theme>`: the score as declared, each layer's scale (its unit, kind, words and
//!   the range of its values present), and the sources with their pieces counted by class (those
//!   of class `unknown` named: they're drawn nowhere).

use crate::cache::{self, Lru};
use crate::S;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use codecs::available::{self as fmt, Grid, Tile, PIXELS, SIDE};
use det::Det;
use inputs::inputs::extents::{self as ex, Ranked, Scale, Score, SCORE_LAYER};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;

/// The sets served, each with its name in the app: the best data any declared source has. (The
/// data built into the map, `built`, is planned: task #1349.)
pub const SETS: [(&str, &str); 1] = [("best", "Best available anywhere")];

/// Whether `set` is one served.
fn set_served(set: &str) -> bool {
    SETS.iter().any(|(k, _)| *k == set)
}

/// The score layer's name where `score.toml` gives none.
const DEFAULT_LABEL: &str = "Best data";

/// The tiles' format, in the layers' versions (`Available::version`): a page made for another
/// format (the painted tiles before it) reads the theme again at the new version, and an old one
/// can't, so it shows nothing until it's loaded again rather than values as colours; and a tile
/// kept for good under an older one (`?v=`) isn't read again. `values2`: the score layer's pixels
/// are the best piece's own primary value, no longer the value its score equals.
const TILE_FORMAT: &str = "values2";

/// What a tile's pixel carries for a value `v` of a layer on `scale` (docs/formats.md, Data
/// available): log10(v) on a log scale (a grid spacing of 0.1 m to 1,000 m: −1 to 3), `v` on a
/// linear one, the word's value on a `values` one (0 to 1).
pub fn code(scale: &Scale, v: f64) -> f64 {
    match scale {
        Scale::Log { .. } => v.dlog10(),
        _ => v,
    }
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
        self.availability() == Some("open")
    }

    fn availability(&self) -> Option<&str> {
        self.props.get("availability").and_then(Value::as_str)
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
    /// Whether `layer` is one of the theme's (`Score::layers`): the score or a metric but the
    /// primary.
    pub fn has_layer(&self, layer: &str) -> bool {
        self.score.as_ref().is_some_and(|s| s.metric_of(layer).is_some())
    }

    /// The pieces `ids` that pass the switch (`all`: every one; else those whose availability is
    /// `open`), by id.
    fn passing<'a>(&self, ids: impl IntoIterator<Item = &'a str>, all: bool) -> Vec<(&'a str, &PieceInfo)> {
        let mut ps: Vec<(&str, &PieceInfo)> = ids
            .into_iter()
            .filter_map(|id| Some((id, self.pieces.get(id)?)))
            .filter(|(_, p)| all || p.open())
            .collect();
        ps.sort_by_key(|(id, _)| *id);
        ps
    }

    /// The piece `layer` draws among pieces `ids` that pass the switch, None where none does:
    /// - the score's: the best (`Score::best`: of the pieces whose primary ties the finest's,
    ///   those of the best availability, then the best score; of equals, the first id);
    /// - a metric's: the best 0–1 value of it (a piece not stating it takes no part), then the
    ///   best availability; of equals, the first id.
    pub fn best<'a>(&self, layer: &str, ids: impl IntoIterator<Item = &'a str>, all: bool) -> Option<&PieceInfo> {
        self.best_id(layer, ids, all).map(|(_, p)| p)
    }

    /// `best`, with the piece's id.
    fn best_id<'a>(
        &self,
        layer: &str,
        ids: impl IntoIterator<Item = &'a str>,
        all: bool,
    ) -> Option<(&'a str, &PieceInfo)> {
        let s = self.score.as_ref()?;
        let ps = self.passing(ids, all);
        if layer == SCORE_LAYER {
            let (at, rs) = self.scored(&ps);
            return s.best(&rs).map(|k| ps[at[k]]);
        }
        let mut best: Option<(f64, usize, (&str, &PieceInfo))> = None;
        for &(id, p) in &ps {
            let Some(v) = p.values.get(layer).copied().flatten() else { continue };
            let r = s.avail_rank(p.availability());
            if best.is_none_or(|(bv, br, _)| v > bv || (v == bv && r < br)) {
                best = Some((v, r, (id, p)));
            }
        }
        best.map(|(_, _, b)| b)
    }

    /// The pieces of `ps` with a score, as `Score::best` ranks them, and where each is in `ps`.
    fn scored<'p>(&self, ps: &'p [(&str, &PieceInfo)]) -> (Vec<usize>, Vec<Ranked<'p>>) {
        let primary = self.score.as_ref().map_or("", |s| s.primary.as_str());
        ps.iter()
            .enumerate()
            .filter_map(|(k, (_, p))| {
                let lead = p.values.get(primary).copied().flatten()?;
                Some((k, Ranked { lead, availability: p.availability(), score: p.score? }))
            })
            .unzip()
    }

    /// The piece each layer draws among pieces `ids` (`best`), its id by layer; a layer drawing
    /// none there absent. The click marks it, so the app ranks nothing itself.
    pub fn painted<'a>(&self, ids: &[&'a str], all: bool) -> Map<String, Value> {
        let Some(s) = &self.score else { return Map::new() };
        s.layers().filter_map(|l| Some((l.to_string(), json!(self.best_id(l, ids.iter().copied(), all)?.0)))).collect()
    }

    /// The value a tile carries for a piece on `layer` (`code`): the piece's own value of the
    /// metric the layer draws (`Score::metric_of`: the score's, the primary). None where the
    /// piece doesn't state it.
    pub fn code(&self, layer: &str, p: &PieceInfo) -> Option<f64> {
        let (name, m) = self.score.as_ref()?.metric_of(layer)?;
        let raw = match m.scale {
            Scale::Values(_) => p.values.get(name).copied().flatten()?,
            _ => p.props.get(name).and_then(Value::as_f64).filter(|v| v.is_finite())?,
        };
        let c = code(&m.scale, raw);
        c.is_finite().then_some(c)
    }

    /// The pieces `ids` that pass the switch, best first (`Score::ranked`; a piece with no score
    /// after them, by id), as the click shows them.
    pub fn listed<'a>(&self, ids: impl IntoIterator<Item = &'a str>, all: bool) -> Vec<Value> {
        let ps = self.passing(ids, all);
        let (at, rs) = self.scored(&ps);
        let mut order: Vec<usize> = match &self.score {
            Some(s) => s.ranked(&rs).into_iter().map(|k| at[k]).collect(),
            None => Vec::new(),
        };
        order.extend((0..ps.len()).filter(|k| !at.contains(k)));
        order
            .into_iter()
            .map(|k| {
                let (id, p) = ps[k];
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
                    "stated": p.props.get("stated"),
                    "values": p.values,
                    "score": p.score,
                })
            })
            .collect()
    }

    /// The values `layer`'s tiles can carry, in the layer's unit: their least and greatest over the
    /// pieces drawn (those with an extent); None where no piece states it.
    fn present(&self, layer: &str) -> Option<[f64; 2]> {
        let (_, m) = self.score.as_ref()?.metric_of(layer)?;
        let mut r: Option<[f64; 2]> = None;
        for p in self.pieces.values().filter(|p| p.props.get("extent").and_then(Value::as_str) != Some("unknown")) {
            let Some(c) = self.code(layer, p) else { continue };
            let v = if matches!(m.scale, Scale::Log { .. }) { 10f64.dpowf(c) } else { c };
            r = Some(r.map_or([v, v], |[lo, hi]| [lo.min(v), hi.max(v)]));
        }
        r
    }

    /// `/api/available/<theme>`'s answer: the score as declared, each layer's scale, the sources.
    /// A layer is its `label`, `unit`, `scale` (`log`, `linear` or `values`: what its tiles carry,
    /// `code`), which end is the better (`best`: `low` or `high`), its `words` (a `values` one's,
    /// each with its value) and `range`, the least and greatest value present (in its unit; none
    /// where no piece states it). The score's is the primary's, under the score's `label`; the
    /// primary has no other.
    pub fn described(&self, theme: &str) -> Value {
        let Some(s) = &self.score else { return json!({"theme": theme, "layers": [], "sources": []}) };
        let layer = |id: &str, label: &str, m: &ex::Metric| {
            let (kind, best, words) = match &m.scale {
                Scale::Log { worst, best } => ("log", if best < worst { "low" } else { "high" }, None),
                Scale::Linear { worst, best } => ("linear", if best < worst { "low" } else { "high" }, None),
                Scale::Values(w) => {
                    let mut w: Vec<(&String, &f64)> = w.iter().collect();
                    w.sort_by(|a, b| a.1.total_cmp(b.1));
                    let words: Vec<Value> = w.iter().map(|(k, v)| json!({"value": v, "label": k})).collect();
                    ("values", "high", Some(words))
                }
            };
            json!({"id": id, "label": label, "unit": m.unit, "scale": kind, "best": best, "words": words,
                "range": self.present(id)})
        };
        let layers: Vec<Value> = s
            .layers()
            .filter_map(|id| {
                let (_, m) = s.metric_of(id)?;
                Some(layer(
                    id,
                    if id == SCORE_LAYER { s.label.as_deref().unwrap_or(DEFAULT_LABEL) } else { &m.label },
                    m,
                ))
            })
            .collect();
        let mut metrics = Vec::new();
        // (The primary first, then the others by name.)
        let mut names: Vec<&String> = s.metrics.keys().collect();
        names.sort_by_key(|n| (**n != s.primary, n.as_str()));
        for name in names {
            let m = &s.metrics[name];
            let (scale, worst, best, values) = match &m.scale {
                Scale::Log { worst, best } => ("log", Some(*worst), Some(*best), None),
                Scale::Linear { worst, best } => ("linear", Some(*worst), Some(*best), None),
                Scale::Values(w) => ("values", None, None, Some(w.clone())),
            };
            metrics.push(json!({"id": name, "label": m.label, "unit": m.unit, "weight": m.weight, "scale": scale,
                "worst": worst, "best": best, "values": values}));
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
        // (The words the app shows for the sets' ids, the pieces' availability and extent classes.)
        let words =
            |w: &[(&str, &str)]| -> Map<String, Value> { w.iter().map(|(k, v)| (k.to_string(), json!(v))).collect() };
        json!({"theme": theme, "sets": SETS.map(|(k, _)| k), "primary": s.primary, "within": s.within, "share": s.share,
            "availability": s.availability, "metrics": metrics, "layers": layers, "sources": sources,
            "labels": {"sets": words(&SETS), "availability": words(&ex::AVAILABILITY), "extent": words(&ex::CLASSES)}})
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
                let Ok(d) = std::str::from_utf8(b)
                    .map_err(|_| ())
                    .and_then(|s| toml::from_str::<toml::Table>(s).map_err(|_| ()))
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
                    let read =
                        reads.iter().find(|r| r["piece"].as_str() == Some(block)).map_or(Value::Null, |r| (*r).clone());
                    if f["geometry"].is_null() {
                        let label = props.get("name").and_then(Value::as_str).unwrap_or(piece);
                        t.unknown.entry(id.to_string()).or_default().push(label.to_string());
                    }
                    t.pieces.insert(
                        format!("{id}/{piece}"),
                        PieceInfo {
                            source: id.to_string(),
                            props: props.clone(),
                            read,
                            score: None,
                            values: BTreeMap::new(),
                        },
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

    /// The tiles' format and the version kept's hash ("" before one): every tile's values change
    /// with it.
    pub fn version(&self) -> String {
        let v = self.state.read().unwrap().0.clone();
        format!("{TILE_FORMAT}-{}", inputs::inputs::hash_of(&v).unwrap_or(""))
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

/// A tile of values: each pixel the layer's value of its set's best piece (`Theme::best`,
/// `Theme::code`), Terrarium-coded in RGB (value + 32,768 = R × 256 + G + B ÷ 256: 1/256 of a unit,
/// a factor of 1.009 on a log layer), alpha 255; transparent (all 0) where none. None where every
/// pixel is.
pub fn values(t: &Tile, theme: &Theme, layer: &str, all: bool) -> anyhow::Result<Option<Vec<u8>>> {
    let codes: Vec<Option<f32>> = t
        .sets
        .iter()
        .map(|s| {
            let p = theme.best(layer, s.iter().map(|&k| t.ids[k as usize].as_str()), all)?;
            theme.code(layer, p).map(|c| c as f32)
        })
        .collect();
    // (A tile not stored is its ancestor's pixels over it, the ancestor's palette whole: a set
    // drawn may hold no pixel of it.)
    let rgb: Vec<Option<[u8; 3]>> = codes
        .iter()
        .map(|c| {
            c.map(|c| {
                let v = roadcore::grid::terrarium_rgb(&[c]);
                [v[0], v[1], v[2]]
            })
        })
        .collect();
    let mut rgba = vec![0u8; PIXELS * 4];
    let mut any = false;
    for (k, &p) in t.px.iter().enumerate() {
        if let Some(c) = rgb[p as usize] {
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

/// Tiles of values made here, by layer, theme layer, switch, tile and versions.
type Made = Lru<(String, String, bool, u8, u32, u32, u64, String), bytes::Bytes>;
static MADE: LazyLock<Mutex<Made>> = LazyLock::new(|| Mutex::new(Lru::new(64 << 20)));

#[allow(clippy::too_many_arguments)]
async fn tile(
    s: S,
    grid: Grid,
    theme: String,
    set: String,
    layer: String,
    z: u8,
    x: u32,
    y: u32,
    q: Option<String>,
    headers: HeaderMap,
) -> Response {
    let Some(th) = s.available.theme(&theme) else { return StatusCode::NOT_FOUND.into_response() };
    if !set_served(&set) || !th.has_layer(&layer) || z > fmt::maxz(grid) || x >> z != 0 || y >> z != 0 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let stl = stored_layer(&theme, grid);
    let all = all_of(q.as_deref());
    let g = s.data.generation.load(std::sync::atomic::Ordering::Relaxed);
    let uv = s.available.version();
    let etag =
        format!("\"av-{}-{uv}-{layer}-{}-{z}-{x}-{y}\"", s.data.layer_version(&stl), if all { "all" } else { "open" });
    let key = (stl.clone(), layer.clone(), all, z, x, y, g, uv);
    let v = cache::versioned(q.as_deref());
    let s2 = s.clone();
    cache::serve_made(&MADE, key, &etag, &headers, v, "image/png", format!("{stl} {layer} {z}/{x}/{y}"), move || {
        if !s2.data.catalogue().layers.contains_key(&stl) {
            return Ok(None);
        }
        let Some(t) = fmt::tile_at(grid, z, x, y, &mut |z, x, y| stored(&s2, &stl, z, x, y))? else { return Ok(None) };
        values(&t, &th, &layer, all)
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
    if !set_served(&set) || !q.lng.is_finite() || !q.lat.is_finite() || q.lat.abs() > 90.0 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let all = q.avail.as_deref() == Some("all");
    let (lng, lat) = (q.lng, q.lat);
    let r = cache::blocking("available: a click", move || {
        let (grid, px, py) = pixel_of(lng, lat);
        let stl = stored_layer(&theme, grid);
        let ids = fmt::pieces_at(grid, px, py, &mut |z, x, y| stored(&s, &stl, z, x, y))?;
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        Ok(json!({"lng": lng, "lat": lat, "avail": if all { "all" } else { "open" },
            "pieces": th.listed(ids.iter().copied(), all), "painted": th.painted(&ids, all)}))
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
