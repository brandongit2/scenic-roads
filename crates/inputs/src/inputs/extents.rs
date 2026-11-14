//! The source extents (docs/inputs.md §6.11): where each known data source has data, and how good it
//! is there. Its inputs folder, `inputs/extents/`, holds a folder per theme (`terrain/`), and in it
//! the theme's `score.toml` and, per source, its declaration `<source>.toml` (a description, §5.1,
//! with its `theme`, `availability` and `[[piece]]` blocks) and its pieces `<source>.geojson` (a
//! FeatureCollection made from the declaration by producers/extents/extents.py, a feature per piece:
//! its outline, extent class, availability and the theme's metrics). The source's id is the files'
//! stem. The step `available` (terrain::available) draws the accepted pieces' tiles; the server
//! reads the score and the pieces' metrics (`Score::of`; docs/steps.md §14).
//!
//! The checks:
//! - **Errors:** a file not of the shape (`<theme>/<source>.toml`, `<theme>/<source>.geojson`,
//!   `<theme>/score.toml`); a theme the app doesn't know, or a file's `theme` not its folder's; a
//!   description field wrong; a declaration's own fields wrong (an unknown method, class or
//!   availability, a block without its id, two blocks of one id, a metric's reading not of its
//!   shape); a `.geojson` that isn't a FeatureCollection, whose `source` isn't its stem or whose
//!   `read` isn't a list of its blocks' readings; pieces without `piece`, `extent` and
//!   `availability` or with one of the wrong kind, a geometry that isn't a MultiPolygon (or null,
//!   for a piece of class `unknown` alone); pieces of invalid geometry (a position not a longitude
//!   and latitude, a ring not closed, of fewer than four positions, of no area, crossing itself,
//!   wound against RFC 7946, or stepping across the antimeridian rather than cut there); a piece's
//!   id twice; a `.geojson` without its declaration, or not made from it (its blocks read aren't the
//!   declaration's, a piece of no block, or not of its block's class, availability or constant
//!   metrics); a `score.toml` not of its shape (below), or breaking the
//!   guarantee; a theme with no `score.toml`; a metric the theme's score doesn't name, in a
//!   declaration or a piece, or a value not of its scale's kind.
//! - **Warnings:** a declaration with no `.geojson` yet; pieces of class `unknown` (listed, not
//!   drawn); a `score.toml` holding fields no longer read (`ramp`, `shade`, a metric's `classes`
//!   and `colours`: read as if it didn't).
//!
//! The facts: a `.geojson`'s pieces, by class and availability, and its box (`pieces`, `classes`,
//! `availability`, `box`); a declaration's theme, availability and blocks; a score's primary metric
//! and its metrics.

use super::{Checks, FileCheck, Finding, Level, Version};
use anyhow::Result;
use det::Det;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// The unit's name.
pub const UNIT: &str = "extents";

/// The themes the app knows (`bathymetry` and `buildings` join when their research is done).
pub const THEMES: [&str; 1] = ["terrain"];

/// The extent classes, best first, each with what it says in plain words (the app's click shows
/// them: crates/server/src/available.rs `described`).
pub const CLASSES: [(&str, &str); 6] = [
    ("mask", "where the map’s own copy has data"),
    ("files", "from the files’ own headers"),
    ("index", "from the publisher’s tile index"),
    ("service", "from the publisher’s footprint service"),
    ("claimed", "claimed area, not checked against the data"),
    ("unknown", "not known"),
];

/// How a source's data can be had, each in plain words (as the extent classes').
pub const AVAILABILITY: [(&str, &str); 4] = [
    ("open", "downloads freely"),
    ("login", "free after a login or a request"),
    ("paid", "paid"),
    ("closed", "exists, not to be had"),
];

/// The availabilities that cost nothing (of AVAILABILITY's): the data-available layers' default
/// view, free data (crates/server/src/available.rs).
pub const FREE: [&str; 2] = ["open", "login"];

/// The producer's readers and the class each one's extents are (None: the block says, `index`,
/// `service` or `claimed`).
pub const METHODS: [(&str, Option<&str>); 8] = [
    ("arcgis", None),
    ("stac", Some("index")),
    ("wfs", None),
    ("file", None),
    ("listing", Some("index")),
    ("raster", Some("files")),
    ("atom", Some("index")),
    ("none", Some("unknown")),
];

/// The classes a block of a method that doesn't fix its class may say.
const SAID: [&str; 3] = ["index", "service", "claimed"];

/// How a metric's reading writes a date.
const DATES: [&str; 5] = ["epoch-ms", "iso", "year", "text", "yymmdd"];

/// A theme's score file.
pub const SCORE: &str = "score.toml";

/// A piece's properties beside the theme's metrics.
const OWN: [&str; 5] = ["piece", "name", "extent", "availability", "stated"];

const SHAPE: &str = "ex-shape";
const THEME: &str = "ex-theme";
const DESC: &str = "ex-desc";
const DECL: &str = "ex-decl";
const PIECES: &str = "ex-pieces";
const PIECE: &str = "ex-piece";
const GEOM: &str = "ex-geom";
const TWICE: &str = "ex-twice";
const SCORE_CHECK: &str = "ex-score";
const RETIRED_CHECK: &str = "ex-retired";
const NO_SCORE: &str = "ex-noscore";
const METRIC: &str = "ex-metric";
const PAIR: &str = "ex-pair";
const MADE: &str = "ex-made";
const NOT_YET: &str = "ex-nopieces";
const UNKNOWN: &str = "ex-unknown";

/// Whether `s` is a source's or a block's id: lower-case letters, digits, `-` and `_`, a letter or
/// digit first.
pub fn valid_id(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// What a path of the inputs folder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind<'a> {
    /// `<theme>/score.toml`.
    Score(&'a str),
    /// `<theme>/<source>.toml`.
    Declaration(&'a str, &'a str),
    /// `<theme>/<source>.geojson`.
    Pieces(&'a str, &'a str),
    Other,
}

/// What `path` is, by its name alone (a theme the app doesn't know included: `file` says so).
pub fn kind(path: &str) -> Kind<'_> {
    let Some((theme, name)) = path.split_once('/') else { return Kind::Other };
    if !valid_id(theme) || name.contains('/') {
        return Kind::Other;
    }
    if name == SCORE {
        return Kind::Score(theme);
    }
    match name.rsplit_once('.') {
        Some((id, "toml")) if valid_id(id) => Kind::Declaration(theme, id),
        Some((id, "geojson")) if valid_id(id) => Kind::Pieces(theme, id),
        _ => Kind::Other,
    }
}

// ---- The score ---------------------------------------------------------------------------------

/// A metric's scale: its value from 0 at `worst` to 1 at `best`, clamped.
#[derive(Clone, Debug, PartialEq)]
pub enum Scale {
    Log {
        worst: f64,
        best: f64,
    },
    Linear {
        worst: f64,
        best: f64,
    },
    /// A word's value.
    Values(BTreeMap<String, f64>),
}

impl Scale {
    /// Why `v` isn't a value of this scale's kind (a positive number on a log scale, a number on a
    /// linear one, one of the words), or None.
    pub fn refuses(&self, v: &Value) -> Option<String> {
        match self {
            Scale::Log { .. } => {
                (!v.as_f64().is_some_and(|x| x.is_finite() && x > 0.0)).then(|| format!("{v} isn't a positive number"))
            }
            Scale::Linear { .. } => (!v.as_f64().is_some_and(f64::is_finite)).then(|| format!("{v} isn't a number")),
            Scale::Values(w) => (!v.as_str().is_some_and(|s| w.contains_key(s)))
                .then(|| format!("{v} isn't one of {}", w.keys().cloned().collect::<Vec<_>>().join(", "))),
        }
    }
}

/// A metric of a theme's score.
#[derive(Clone, Debug, PartialEq)]
pub struct Metric {
    pub label: String,
    pub unit: Option<String>,
    /// None for the primary metric.
    pub weight: Option<f64>,
    pub scale: Scale,
}

/// The fields a `score.toml` once had for drawing its layers as classes (the layers' own colours
/// are the app's now): a file still holding them is read as if it didn't, and the check warns.
const RETIRED: [&str; 2] = ["ramp", "shade"];
const RETIRED_METRIC: [&str; 2] = ["classes", "colours"];

/// A theme's `score.toml` (docs/inputs.md §6.11).
#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub theme: Option<String>,
    pub primary: String,
    pub within: f64,
    pub share: f64,
    pub metrics: BTreeMap<String, Metric>,
    /// The score layer's name in the app (None: the server's own).
    pub label: Option<String>,
    /// How availability ranks pieces whose primary ties (within `within` of the finest), best
    /// first: AVAILABILITY's ids, each once (empty: it ranks none).
    pub availability: Vec<String>,
    /// The retired fields the file holds (RETIRED), read as if it didn't.
    pub ignored: Vec<String>,
}

impl Score {
    /// The largest `share` the guarantee allows: (1 − share) × k > share, k = ln(within) ÷
    /// ln(worst ÷ best) of the primary metric (a log scale).
    pub fn bound(within: f64, worst: f64, best: f64) -> f64 {
        let k = within.dln() / (worst / best).dln().abs();
        k / (1.0 + k)
    }

    /// Metric `name`'s 0–1 value of `v` (producers/extents/score.py's `value`): from 0 at `worst` to
    /// 1 at `best`, clamped (by the logarithm on a log scale), or a word's; None where the piece
    /// doesn't state it (or `v` isn't of the scale's kind, which an accepted unit never has).
    pub fn value(&self, name: &str, v: Option<&Value>) -> Option<f64> {
        let m = self.metrics.get(name)?;
        let v = v?;
        match &m.scale {
            Scale::Values(w) => w.get(v.as_str()?).copied(),
            Scale::Log { worst, best } => {
                let x = v.as_f64().filter(|x| *x > 0.0)?;
                Some(((x.dln() - worst.dln()) / (best.dln() - worst.dln())).clamp(0.0, 1.0))
            }
            Scale::Linear { worst, best } => Some(((v.as_f64()? - worst) / (best - worst)).clamp(0.0, 1.0)),
        }
    }

    /// A piece's score and each metric's 0–1 value (score.py's `score`), from its properties
    /// (`metrics`): (1 − share) × the primary's value + share × the rest, the rest the weighted
    /// mean of the other metrics it states (none stated, or their weights 0: the primary's value);
    /// no score where it doesn't state the primary.
    pub fn of(&self, metrics: &serde_json::Map<String, Value>) -> (Option<f64>, BTreeMap<String, Option<f64>>) {
        let vals: BTreeMap<String, Option<f64>> =
            self.metrics.keys().map(|k| (k.clone(), self.value(k, metrics.get(k)))).collect();
        let Some(lead) = vals.get(&self.primary).copied().flatten() else { return (None, vals) };
        let rest = self.rest(&vals).unwrap_or(lead);
        (Some((1.0 - self.share) * lead + self.share * rest), vals)
    }

    /// The tie-break of a piece's metric values (`of`'s rest): the weighted mean of the other
    /// metrics it states; None where it states none of them (or their weights are 0), which `of`
    /// takes as the primary's value.
    fn rest(&self, vals: &BTreeMap<String, Option<f64>>) -> Option<f64> {
        let (mut w, mut sum) = (0.0, 0.0);
        for (k, v) in vals {
            if let (false, Some(v), Some(weight)) = (*k == self.primary, v, self.metrics.get(k).and_then(|m| m.weight))
            {
                w += weight;
                sum += weight * v;
            }
        }
        (w > 0.0).then(|| sum / w)
    }

    /// How far apart two pieces' primary values (0–1) may be and still tie: a factor of `within`
    /// on the primary's log scale, ln(within) ÷ ln(worst ÷ best).
    fn tie(&self) -> f64 {
        match self.metrics.get(&self.primary).map(|m| &m.scale) {
            Some(Scale::Log { worst, best }) => self.within.dln() / (worst / best).dln().abs(),
            _ => 0.0,
        }
    }

    /// Where availability `a` ranks: its place in `availability`, one it doesn't list (or no order
    /// declared: every one) after them.
    pub fn avail_rank(&self, a: Option<&str>) -> usize {
        a.and_then(|a| self.availability.iter().position(|k| k == a)).unwrap_or(self.availability.len())
    }

    /// Which of `ps` is best (its index; None where `ps` is empty): of the pieces whose primary
    /// ties the finest's (within `within` of it, `tie`), those of the best availability (the
    /// declared order); of them, the best score; of equals, the first. So a grid finer by more
    /// than `within` always wins, whatever its availability, and an open piece wins a tie
    /// (producers/extents/score.py's `best`).
    pub fn best(&self, ps: &[Ranked]) -> Option<usize> {
        let finest = ps.iter().map(|p| p.lead).fold(f64::NEG_INFINITY, f64::max);
        let tie = self.tie() + 1e-12;
        let mut best: Option<(usize, usize, f64)> = None;
        for (k, p) in ps.iter().enumerate().filter(|(_, p)| finest - p.lead <= tie) {
            let r = self.avail_rank(p.availability);
            if best.is_none_or(|(_, br, bs)| r < br || (r == br && p.score > bs)) {
                best = Some((k, r, p.score));
            }
        }
        best.map(|(k, _, _)| k)
    }

    /// `ps` best first (their indices): the best (`best`), then the best of the rest, and so on.
    pub fn ranked(&self, ps: &[Ranked]) -> Vec<usize> {
        let mut left: Vec<usize> = (0..ps.len()).collect();
        let mut out = Vec::with_capacity(ps.len());
        while let Some(k) = self.best(&left.iter().map(|&i| ps[i]).collect::<Vec<_>>()) {
            out.push(left.remove(k));
        }
        out
    }

    /// The metric a layer draws (`SCORE_LAYER`: the primary) and so its values' scale; None for a
    /// layer that isn't one of the score's: the primary has no layer of its own.
    pub fn metric_of<'a>(&'a self, layer: &'a str) -> Option<(&'a str, &'a Metric)> {
        let name = match layer {
            SCORE_LAYER => self.primary.as_str(),
            l if l == self.primary => return None,
            l => l,
        };
        Some((name, self.metrics.get(name)?))
    }

    /// The layers, in the app's order: the score's (`SCORE_LAYER`), then each other metric's,
    /// by name.
    pub fn layers(&self) -> impl Iterator<Item = &str> {
        std::iter::once(SCORE_LAYER).chain(self.metrics.keys().map(String::as_str).filter(|k| *k != self.primary))
    }
}

/// The score's layer: the best piece (`Score::best`) drawn at its primary's own value (the
/// terrain's: its grid spacing). The theme's other layers are its other metrics, each drawing the
/// piece of the best value of it; the primary has none of its own, the score's being it.
pub const SCORE_LAYER: &str = "score";

/// A piece as `Score::best` ranks it: its primary's 0–1 value, its availability and its score.
#[derive(Clone, Copy, Debug)]
pub struct Ranked<'a> {
    pub lead: f64,
    pub availability: Option<&'a str>,
    pub score: f64,
}

fn num(v: &toml::Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).filter(|x| x.is_finite())
}

/// A metric's table, or what's wrong with it.
fn metric(
    name: &str,
    t: &toml::Value,
    primary: bool,
    ignored: &mut Vec<String>,
) -> std::result::Result<Metric, Vec<String>> {
    let Some(t) = t.as_table() else { return Err(vec![format!("metric.{name} isn't a table")]) };
    let mut bad = Vec::new();
    for k in t.keys() {
        if RETIRED_METRIC.contains(&k.as_str()) {
            ignored.push(format!("metric.{name}.{k}"));
        } else if !["label", "unit", "weight", "scale", "worst", "best", "values"].contains(&k.as_str()) {
            bad.push(format!("metric.{name}: `{k}` isn't one of a metric's fields"));
        }
    }
    let label = t.get("label").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty());
    if label.is_none() {
        bad.push(format!("metric.{name}: `label` is required"));
    }
    let unit = match t.get("unit") {
        None => None,
        Some(v) => match v.as_str() {
            Some(s) => Some(s.to_string()),
            None => {
                bad.push(format!("metric.{name}: `unit` isn't a string"));
                None
            }
        },
    };
    let weight = match (t.get("weight"), primary) {
        (Some(_), true) => {
            bad.push(format!(
                "metric.{name}: the primary metric takes no weight (it decides; the others' share is `share`)"
            ));
            None
        }
        (None, true) => None,
        (w, false) => match w.and_then(num) {
            Some(w) if w >= 0.0 => Some(w),
            Some(_) => {
                bad.push(format!("metric.{name}: a negative weight"));
                None
            }
            None => {
                bad.push(format!(
                    "metric.{name}: `weight` is required, a number (0: a layer of its own, no part in the score)"
                ));
                None
            }
        },
    };
    let scale = match (t.get("values"), t.get("scale").map(|s| s.as_str())) {
        (Some(v), None) => match v.as_table() {
            Some(w) if !w.is_empty() => {
                let mut out = BTreeMap::new();
                for (word, x) in w {
                    match num(x).filter(|x| (0.0..=1.0).contains(x)) {
                        Some(x) => {
                            out.insert(word.clone(), x);
                        }
                        None => bad.push(format!("metric.{name}: the value of {word:?} isn't a number from 0 to 1")),
                    }
                }
                Some(Scale::Values(out))
            }
            _ => {
                bad.push(format!("metric.{name}: `values` isn't a table of words' values"));
                None
            }
        },
        (Some(_), Some(_)) => {
            bad.push(format!("metric.{name}: `values` is a scale of its own: no `scale` beside it"));
            None
        }
        (None, Some(Some(s @ ("log" | "linear")))) => {
            match (t.get("worst").and_then(num), t.get("best").and_then(num)) {
                (Some(worst), Some(best)) if worst == best => {
                    bad.push(format!("metric.{name}: `worst` = `best`"));
                    None
                }
                (Some(worst), Some(best)) if s == "log" && (worst <= 0.0 || best <= 0.0) => {
                    bad.push(format!("metric.{name}: a log scale's `worst` and `best` are positive"));
                    None
                }
                (Some(worst), Some(best)) if s == "log" => Some(Scale::Log { worst, best }),
                (Some(worst), Some(best)) => Some(Scale::Linear { worst, best }),
                _ => {
                    bad.push(format!("metric.{name}: a {s} scale needs `worst` and `best`, numbers"));
                    None
                }
            }
        }
        (None, _) => {
            bad.push(format!(
                "metric.{name}: `scale` is \"log\" or \"linear\" (with `worst` and `best`), or a `values` table"
            ));
            None
        }
    };
    match (bad.is_empty(), label, scale) {
        (true, Some(l), Some(scale)) => Ok(Metric { label: l.to_string(), unit, weight, scale }),
        _ => Err(bad),
    }
}

/// A `score.toml` read, or what's wrong with it (its shape, and the guarantee).
pub fn score(bytes: &[u8]) -> std::result::Result<Score, Vec<String>> {
    let Ok(t) =
        std::str::from_utf8(bytes).map_err(|_| ()).and_then(|t| toml::from_str::<toml::Table>(t).map_err(|_| ()))
    else {
        return Err(vec!["not TOML".into()]);
    };
    let mut bad = Vec::new();
    let mut ignored = Vec::new();
    for k in t.keys() {
        if RETIRED.contains(&k.as_str()) {
            ignored.push(k.clone());
        } else if !["theme", "primary", "within", "share", "availability", "metric", "label"].contains(&k.as_str()) {
            bad.push(format!("`{k}` isn't one of a score's fields"));
        }
    }
    let theme = t.get("theme").map(|v| v.as_str().unwrap_or("").to_string());
    let primary = t.get("primary").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let empty = toml::Table::new();
    let tables = match t.get("metric").map(|m| m.as_table()) {
        Some(Some(m)) if !m.is_empty() => m,
        _ => {
            bad.push("no [metric.<name>] tables".into());
            &empty
        }
    };
    if !tables.contains_key(&primary) {
        bad.push(format!(
            "`primary` is {}, not one of its metrics",
            if primary.is_empty() { "missing".into() } else { format!("{primary:?}") }
        ));
    }
    let within = t.get("within").and_then(num).filter(|w| *w > 1.0);
    if within.is_none() {
        bad.push("`within` is required, a factor above 1".into());
    }
    let share = t.get("share").and_then(num).filter(|s| *s >= 0.0 && *s < 1.0);
    if share.is_none() {
        bad.push("`share` is required, a number from 0 to under 1".into());
    }
    let label = match t.get("label") {
        None => None,
        Some(v) => match v.as_str().filter(|s| !s.trim().is_empty()) {
            Some(s) => Some(s.to_string()),
            None => {
                bad.push("`label` (the score layer's name) isn't a name".into());
                None
            }
        },
    };
    let availability: Vec<String> = match t.get("availability") {
        None => Vec::new(),
        Some(v) => {
            let ids: Vec<&str> = v.as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
            let all = AVAILABILITY.map(|(k, _)| k);
            let each_once = ids.len() == all.len() && all.iter().all(|k| ids.contains(k));
            if !each_once || v.as_array().is_none_or(|a| a.len() != ids.len()) {
                bad.push(format!("`availability` isn't each of {} once, best first", all.join(", ")));
            }
            ids.iter().map(|s| s.to_string()).collect()
        }
    };
    let mut metrics = BTreeMap::new();
    for (name, m) in tables {
        match metric(name, m, *name == primary, &mut ignored) {
            Ok(m) => {
                metrics.insert(name.clone(), m);
            }
            Err(why) => bad.extend(why),
        }
    }
    if let (Some(m), Some(within), Some(share)) = (metrics.get(&primary), within, share) {
        match m.scale {
            Scale::Log { worst, best } => {
                let bound = Score::bound(within, worst, best);
                if share >= bound {
                    bad.push(format!(
                        "`share` {share} breaks the guarantee: it must be under {bound:.4} for within = {within}"
                    ));
                }
            }
            _ => bad.push(format!("the primary metric, {primary}, has a log scale (the guarantee is a factor)")),
        }
    }
    match (bad.is_empty(), within, share) {
        (true, Some(within), Some(share)) => {
            Ok(Score { theme, primary, within, share, metrics, label, availability, ignored })
        }
        _ => Err(bad),
    }
}

// ---- A declaration -----------------------------------------------------------------------------

/// A declaration's piece block, as the checks see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: String,
    /// The extent class its pieces are: its method's or the one it says, `claimed` where it says so
    /// or its availability isn't `open` (as producers/extents/extents.py makes them).
    pub extent: String,
    /// Its own availability, or the declaration's.
    pub availability: String,
    /// Its metrics' declarations: a constant, a reading (`{field = …}`) or a list of readings.
    pub metrics: toml::Table,
}

impl Block {
    /// How piece `p` isn't as this block makes it (its class, availability and constant metrics), or
    /// None.
    fn unlike(&self, p: &Piece) -> Option<String> {
        let mut why = Vec::new();
        if p.extent != self.extent {
            why.push(format!("of class {}, its block's {}", p.extent, self.extent));
        }
        if p.availability != self.availability {
            why.push(format!("availability {}, its block's {}", p.availability, self.availability));
        }
        for (name, spec) in self.metrics.iter().filter(|(_, s)| !s.is_table() && !s.is_array()) {
            let want = serde_json::to_value(spec).unwrap_or(Value::Null);
            let same = |got: &Value| match (got.as_f64(), want.as_f64()) {
                (Some(a), Some(b)) => a == b,
                _ => *got == want,
            };
            match p.metrics.get(name) {
                Some(got) if same(got) => {}
                Some(got) => why.push(format!("{name} {got}, its block's {want}")),
                None => why.push(format!("no {name}, its block's {want}")),
            }
        }
        (!why.is_empty()).then(|| why.join(", "))
    }
}

/// A declaration's own fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    pub theme: String,
    pub availability: Option<String>,
    pub blocks: Vec<Block>,
}

/// A metric's reading's faults (`{field = "…", …}`): its own shape, not its metric's.
fn reading(id: &str, name: &str, t: &toml::Table, bad: &mut Vec<String>) {
    let at = format!("piece {id}: metrics.{name}");
    for (k, v) in t {
        let ok = match k.as_str() {
            "field" => v.as_str().is_some_and(|s| !s.is_empty()),
            "scale" => num(v).is_some_and(|x| x > 0.0),
            "map" => v.as_table().is_some_and(|m| !m.is_empty()),
            "index" => v.as_integer().is_some_and(|i| i >= 0),
            "text" | "pattern" => v.as_str().is_some_and(|s| !s.is_empty()),
            "date" => v.as_str().is_some_and(|s| DATES.contains(&s)),
            "density" => v.is_bool(),
            _ => {
                bad.push(format!(
                    "{at}: `{k}` isn't one of a reading's fields (field, pattern, scale, map, index, text, date, density)"
                ));
                continue;
            }
        };
        if !ok {
            bad.push(match k.as_str() {
                "date" => format!("{at}: `date` isn't one of {}", DATES.join(", ")),
                "scale" => format!("{at}: `scale` isn't a positive number"),
                "index" => format!("{at}: `index` isn't a place in a list (0 on)"),
                _ => format!("{at}: `{k}` isn't of its type"),
            });
        }
    }
    if !t.contains_key("field") {
        bad.push(format!("{at}: a reading names its `field`"));
    }
}

/// A declaration's own fields (beside its description's), or what's wrong with them.
pub fn declaration(bytes: &[u8]) -> std::result::Result<Declaration, Vec<String>> {
    let Ok(t) =
        std::str::from_utf8(bytes).map_err(|_| ()).and_then(|t| toml::from_str::<toml::Table>(t).map_err(|_| ()))
    else {
        return Err(vec!["not TOML".into()]);
    };
    let mut bad = Vec::new();
    let theme = t.get("theme").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !THEMES.contains(&theme.as_str()) {
        bad.push(format!(
            "`theme` is {}, not one of {}",
            if theme.is_empty() { "missing".into() } else { format!("{theme:?}") },
            THEMES.join(", ")
        ));
    }
    let avail = |v: Option<&toml::Value>, at: &str, bad: &mut Vec<String>| -> Option<String> {
        let v = v?;
        match v.as_str().filter(|a| AVAILABILITY.iter().any(|(k, _)| k == a)) {
            Some(a) => Some(a.to_string()),
            None => {
                bad.push(format!("{at}`availability` is {v}, not one of {}", AVAILABILITY.map(|(k, _)| k).join(", ")));
                None
            }
        }
    };
    let availability = avail(t.get("availability"), "", &mut bad);
    let mut blocks = Vec::new();
    let mut ids = BTreeSet::new();
    let list = t.get("piece").and_then(|p| p.as_array()).filter(|a| !a.is_empty());
    if list.is_none() {
        bad.push("no [[piece]] block: a declaration says how its pieces are found".into());
    }
    for (i, b) in list.into_iter().flatten().enumerate() {
        let Some(b) = b.as_table() else {
            bad.push(format!("[[piece]] {} isn't a table", i + 1));
            continue;
        };
        let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if !valid_id(&id) {
            bad.push(format!("[[piece]] {}: `id` is required, lower-case letters, digits, - and _", i + 1));
            continue;
        }
        if !ids.insert(id.clone()) {
            bad.push(format!("piece {id}: two blocks of one id"));
        }
        let method = b.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let Some((_, fixed)) = METHODS.iter().find(|(m, _)| *m == method) else {
            bad.push(format!(
                "piece {id}: `method` is {}, not one of {}",
                if method.is_empty() { "missing".into() } else { format!("{method:?}") },
                METHODS.map(|m| m.0).join(", ")
            ));
            continue;
        };
        let class = b.get("class").map(|c| c.as_str().unwrap_or(""));
        let class_ok = match (fixed, class) {
            (Some(_), None) => true,
            (Some(f), Some(c)) => c == *f || (c == "claimed" && method != "none"),
            (None, Some(c)) => SAID.contains(&c),
            (None, None) => false,
        };
        if !class_ok {
            bad.push(match fixed {
                Some(f) => format!(
                    "piece {id}: a {method} block's class is {f}{}",
                    if method == "none" { "" } else { " (or claimed)" }
                ),
                None => format!("piece {id}: a {method} block says its `class`: {}", SAID.join(", ")),
            });
        }
        let own = avail(b.get("availability"), &format!("piece {id}: "), &mut bad);
        if own.is_none() && availability.is_none() && b.get("availability").is_none() {
            bad.push(format!("piece {id}: no `availability`, its own or the declaration's"));
        }
        let piece_avail = own.or_else(|| availability.clone()).unwrap_or_default();
        let extent = match (method, class) {
            ("none", _) => "unknown",
            (_, Some("claimed")) => "claimed",
            _ if piece_avail != "open" => "claimed",
            _ => (*fixed).or(class).unwrap_or(""),
        }
        .to_string();
        if method == "none" && !b.get("why").and_then(|v| v.as_str()).is_some_and(|s| !s.trim().is_empty()) {
            bad.push(format!("piece {id}: a none block says `why` it has no extent"));
        }
        let by: Option<Vec<&str>> = match b.get("by") {
            None => Some(Vec::new()),
            Some(v) => v.as_array().and_then(|a| a.iter().map(|x| x.as_str()).collect()),
        };
        match &by {
            None => bad.push(format!("piece {id}: `by` isn't a list of field names")),
            Some(by) => {
                if let Some(n) = b.get("name") {
                    if !n.as_str().is_some_and(|n| by.contains(&n)) {
                        bad.push(format!(
                            "piece {id}: `name` isn't one of `by` (a piece's features would name it apart)"
                        ));
                    }
                }
            }
        }
        for k in ["label", "where"] {
            if b.get(k).is_some_and(|v| !v.is_str()) {
                bad.push(format!("piece {id}: `{k}` isn't a string"));
            }
        }
        for key in ["tolerance_m", "timeout_s"] {
            if b.get(key).is_some_and(|v| !num(v).is_some_and(|x| x > 0.0)) {
                bad.push(format!("piece {id}: `{key}` isn't a positive number"));
            }
        }
        for key in ["filter", "exclude"] {
            if b.get(key).is_some_and(|v| !v.as_table().is_some_and(|f| f.values().all(|x| x.is_array()))) {
                bad.push(format!("piece {id}: `{key}` isn't a table of lists, {{field = [values]}}"));
            }
        }
        let metrics = match b.get("metrics") {
            None => toml::Table::new(),
            Some(m) => match m.as_table() {
                Some(m) => {
                    for (name, spec) in m {
                        if let Some(r) = spec.as_table() {
                            reading(&id, name, r, &mut bad);
                        } else if let Some(rs) = spec.as_array() {
                            // (Readings in turn, the first giving a value; the last may be a
                            // constant, the value where none of them gives one.)
                            if rs.is_empty() {
                                bad.push(format!("piece {id}: metrics.{name}: an empty list of readings"));
                            }
                            for (k, r) in rs.iter().enumerate() {
                                match r.as_table() {
                                    Some(r) => reading(&id, name, r, &mut bad),
                                    None if k + 1 == rs.len() && !r.is_array() => {}
                                    None => bad.push(format!(
                                        "piece {id}: metrics.{name}: a list holds readings ({{field = …}}), a \
                                         constant only last"
                                    )),
                                }
                            }
                        }
                    }
                    m.clone()
                }
                None => {
                    bad.push(format!("piece {id}: `metrics` isn't a table"));
                    toml::Table::new()
                }
            },
        };
        blocks.push(Block { id, extent, availability: piece_avail, metrics });
    }
    if bad.is_empty() {
        Ok(Declaration { theme, availability, blocks })
    } else {
        Err(bad)
    }
}

// ---- The pieces --------------------------------------------------------------------------------

/// A piece as the checks see it.
#[derive(Clone, Debug)]
pub struct Piece {
    /// Its feature's number, from 1.
    pub n: usize,
    pub id: String,
    pub extent: String,
    pub availability: String,
    /// Its properties but its own (`OWN`) but `stated`: the theme's metrics, and `stated`.
    pub metrics: serde_json::Map<String, Value>,
}

/// A `.geojson` of pieces read: those in their shape, the faults of the others (feature number from
/// 1, its id or "", why), the ids twice and its box.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    /// Why it isn't a collection of pieces at all.
    pub broken: Option<String>,
    pub source: Option<String>,
    pub theme: Option<String>,
    /// The blocks its `read` says it read, by id.
    pub read: Vec<String>,
    /// Why its `read` isn't the list of the blocks' readings.
    pub read_bad: Option<String>,
    pub pieces: Vec<Piece>,
    pub faults: Vec<(usize, String, String)>,
    pub geometry: Vec<(usize, String, String)>,
    pub twice: Vec<(usize, String)>,
    pub bbox: Option<[f64; 4]>,
}

/// Pieces read, by their bytes' hash: the file check, the whole version's and the facts read each
/// file once.
static READ: Mutex<Option<HashMap<String, Arc<Summary>>>> = Mutex::new(None);

/// A `.geojson`'s pieces (`summary_of`), read once per content.
pub fn summary(bytes: &[u8]) -> Arc<Summary> {
    let h = store::naming::hash16(bytes);
    if let Some(s) = READ.lock().unwrap().get_or_insert_with(HashMap::new).get(&h) {
        return s.clone();
    }
    let s = Arc::new(summary_of(bytes));
    let mut g = READ.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    if m.len() > 128 {
        m.clear();
    }
    m.insert(h, s.clone());
    s
}

/// A ring's fault, or None: positions of longitude and latitude, closed, four or more, an area, not
/// crossing itself, cut at the antimeridian (no edge of more than half a turn but one from −180° to
/// 180° along a parallel), wound as RFC 7946 says (`outer`: counter-clockwise; a hole clockwise).
fn ring(r: &Value, outer: bool, bbox: &mut [f64; 4]) -> Option<String> {
    let Some(ps) = r.as_array() else { return Some("a ring that isn't a list of positions".into()) };
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(ps.len());
    for p in ps {
        let a = p.as_array().filter(|a| a.len() >= 2 && a.iter().all(Value::is_number));
        let Some(a) = a else { return Some("a position that isn't two numbers".into()) };
        let (x, y) = (a[0].as_f64().unwrap_or(f64::NAN), a[1].as_f64().unwrap_or(f64::NAN));
        if !(-180.0..=180.0).contains(&x) || !(-90.0..=90.0).contains(&y) {
            return Some(format!("a position out of range ({x}, {y}): not longitude and latitude"));
        }
        pts.push([x, y]);
    }
    if pts.len() < 4 {
        return Some("a ring of fewer than four positions".into());
    }
    if pts.first() != pts.last() {
        return Some("a ring not closed (its last position isn't its first)".into());
    }
    for w in pts.windows(2) {
        // (A long edge from one side of the antimeridian to the other along a parallel, as a piece
        // cut there and spanning the turn has, or one closed along a pole, is fine; any other is a
        // step across.)
        let ends_on_it = w[0][0].abs() == 180.0 && w[1][0].abs() == 180.0 && w[0][1] == w[1][1];
        if (w[1][0] - w[0][0]).abs() > 180.0 && !ends_on_it {
            return Some(format!(
                "a ring stepping across the antimeridian at ({}, {}) (pieces are cut there)",
                w[0][0], w[0][1]
            ));
        }
    }
    let area2: f64 = pts.windows(2).map(|w| w[0][0] * w[1][1] - w[1][0] * w[0][1]).sum();
    if area2 == 0.0 {
        return Some("a ring of no area".into());
    }
    if (area2 > 0.0) != outer {
        return Some(if outer {
            "an exterior ring wound clockwise (RFC 7946: exteriors counter-clockwise)".into()
        } else {
            "a hole wound counter-clockwise (RFC 7946: holes clockwise)".into()
        });
    }
    let mut e7: Vec<[i64; 2]> = Vec::with_capacity(pts.len());
    for p in &pts[..pts.len() - 1] {
        let q = [(p[0] * 1e7).round() as i64, (p[1] * 1e7).round() as i64];
        if e7.last() != Some(&q) {
            e7.push(q);
        }
    }
    if e7.len() > 1 && e7.first() == e7.last() {
        e7.pop();
    }
    if let Some((i, _)) = super::regions::geom::self_crossing(&e7, true) {
        let p = e7[i];
        return Some(format!("a ring crossing itself near ({:.5}, {:.5})", p[0] as f64 * 1e-7, p[1] as f64 * 1e-7));
    }
    for p in &pts {
        *bbox = [bbox[0].min(p[0]), bbox[1].min(p[1]), bbox[2].max(p[0]), bbox[3].max(p[1])];
    }
    None
}

/// A piece's geometry's fault, or None: a MultiPolygon of valid rings (`ring`).
fn multipolygon(g: &Value, bbox: &mut [f64; 4]) -> Option<String> {
    if g.get("type").and_then(Value::as_str) != Some("MultiPolygon") {
        return Some("a geometry that isn't a MultiPolygon".into());
    }
    let Some(polys) = g.get("coordinates").and_then(Value::as_array).filter(|p| !p.is_empty()) else {
        return Some("a MultiPolygon without polygons".into());
    };
    for p in polys {
        let Some(rings) = p.as_array().filter(|r| !r.is_empty()) else {
            return Some("a polygon without rings".into());
        };
        for (k, r) in rings.iter().enumerate() {
            if let Some(why) = ring(r, k == 0, bbox) {
                return Some(why);
            }
        }
    }
    None
}

/// A `.geojson` of pieces read (`summary`).
pub fn summary_of(bytes: &[u8]) -> Summary {
    let mut s = Summary::default();
    let v: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            s.broken = Some(format!("not JSON ({e})"));
            return s;
        }
    };
    let Some(feats) = v
        .get("features")
        .and_then(Value::as_array)
        .filter(|_| v.get("type").and_then(Value::as_str) == Some("FeatureCollection"))
    else {
        s.broken = Some("not a GeoJSON FeatureCollection".into());
        return s;
    };
    s.source = v.get("source").and_then(Value::as_str).map(str::to_string);
    s.theme = v.get("theme").and_then(Value::as_str).map(str::to_string);
    match v.get("read").and_then(Value::as_array) {
        Some(read) => {
            for r in read {
                match (r.get("piece").and_then(Value::as_str), r.get("method").and_then(Value::as_str)) {
                    (Some(p), Some(_)) => s.read.push(p.to_string()),
                    _ => s.read_bad = Some("a reading in `read` without its block's `piece` and `method`".into()),
                }
            }
        }
        None => s.read_bad = Some("no `read`: what the producer read for each block".into()),
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (i, f) in feats.iter().enumerate() {
        let n = i + 1;
        let p = f.get("properties").and_then(Value::as_object);
        let id = p.and_then(|p| p.get("piece")).and_then(Value::as_str).unwrap_or("").to_string();
        let mut fault = |why: String| s.faults.push((n, id.clone(), why));
        if f.get("type").and_then(Value::as_str) != Some("Feature") {
            fault("not a Feature".into());
            continue;
        }
        let Some(p) = p else {
            fault("no properties".into());
            continue;
        };
        if id.is_empty() {
            fault("no `piece`, its id".into());
            continue;
        }
        let extent = p.get("extent").and_then(Value::as_str).filter(|c| CLASSES.iter().any(|(k, _)| k == c));
        let Some(extent) = extent else {
            fault(format!("`extent` isn't one of {}", CLASSES.map(|(k, _)| k).join(", ")));
            continue;
        };
        let Some(availability) =
            p.get("availability").and_then(Value::as_str).filter(|a| AVAILABILITY.iter().any(|(k, _)| k == a))
        else {
            fault(format!("`availability` isn't one of {}", AVAILABILITY.map(|(k, _)| k).join(", ")));
            continue;
        };
        if p.get("name").is_some_and(|n| !n.is_string()) {
            fault("`name` isn't a string".into());
            continue;
        }
        if p.get("stated").is_some_and(|x| !x.as_object().is_some_and(|o| o.values().all(Value::is_string))) {
            fault("`stated` isn't a table of the publisher's classes, by metric".into());
            continue;
        }
        let g = f.get("geometry").unwrap_or(&Value::Null);
        match (g.is_null(), extent == "unknown") {
            (true, false) => {
                fault(format!("no geometry, but of class {extent} (only an unknown extent has none)"));
                continue;
            }
            (false, true) => {
                fault("a geometry, but of class unknown (an unknown extent has none)".into());
                continue;
            }
            (true, true) => {}
            (false, false) => {
                let mut fb = bbox;
                if let Some(why) = multipolygon(g, &mut fb) {
                    s.geometry.push((n, id.clone(), why));
                    continue;
                }
                bbox = fb;
            }
        }
        if !seen.insert(id.clone()) {
            s.twice.push((n, id));
            continue;
        }
        let metrics = p
            .iter()
            .filter(|(k, _)| !OWN.contains(&k.as_str()) || *k == "stated")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        s.pieces.push(Piece { n, id, extent: extent.to_string(), availability: availability.to_string(), metrics });
    }
    s.bbox = bbox[0].is_finite().then_some(bbox);
    s
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// One finding listing pieces of a `.geojson` (feature number, id, why): its id names them.
fn about_pieces(check: &str, level: Level, files: &[&str], ls: &[(usize, String, String)], message: String) -> Finding {
    let raw: Vec<String> = ls.iter().map(|(_, id, why)| format!("{id} {why}")).collect();
    let refs: Vec<&str> = raw.iter().map(String::as_str).collect();
    let mut f = Finding::new(check, level, files, &refs, files.iter().map(|p| p.to_string()).collect(), message);
    f.lines =
        ls.iter().map(|(n, id, why)| (*n, if id.is_empty() { why.clone() } else { format!("{id}: {why}") })).collect();
    f
}

/// Why a metric's declaration in a block isn't of the score's metric (a constant or a `map`'s values
/// of its scale's kind), or None.
fn declared_kind(m: &Metric, spec: &toml::Value) -> Option<String> {
    let json = |v: &toml::Value| serde_json::to_value(v).unwrap_or(Value::Null);
    let mapped = |r: &toml::Table| {
        r.get("map").and_then(|x| x.as_table()).and_then(|map| {
            map.iter().find_map(|(k, v)| m.scale.refuses(&json(v)).map(|why| format!("its map's {k:?}: {why}")))
        })
    };
    match (spec.as_table(), spec.as_array()) {
        (Some(r), _) => mapped(r),
        (None, Some(rs)) => rs.iter().find_map(|r| r.as_table().map_or_else(|| m.scale.refuses(&json(r)), mapped)),
        (None, None) => m.scale.refuses(&json(spec)),
    }
}

/// The source extents' checks.
pub struct Extents;

impl Checks for Extents {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        3
    }

    fn recursive(&self) -> bool {
        true
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        let err = |check: &str, why: &[String]| {
            let refs: Vec<&str> = why.iter().map(String::as_str).collect();
            Finding::new(check, Level::Error, &[path], &refs, vec![path.into()], format!("{path}: {}", why.join("; ")))
        };
        let k = kind(path);
        let theme = match k {
            Kind::Score(t) | Kind::Declaration(t, _) | Kind::Pieces(t, _) => t,
            Kind::Other => {
                c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not the inputs folder's shape"], vec![path.into()], format!("{path} isn't the inputs folder's shape: extents takes <theme>/<source>.toml, <theme>/<source>.geojson and <theme>/score.toml, a source's id lower-case letters, digits, - and _ (docs/inputs.md §6.11); notes go in how/")));
                return c;
            }
        };
        if !THEMES.contains(&theme) {
            c.findings.push(err(THEME, &[format!("{theme} isn't a theme the app knows ({})", THEMES.join(", "))]));
            return c;
        }
        match k {
            Kind::Score(_) => match score(bytes) {
                Ok(s) => {
                    if s.theme.as_deref().is_some_and(|t| t != theme) {
                        c.findings.push(err(THEME, &[format!("its `theme` isn't its folder's, {theme}")]));
                    }
                    if !s.ignored.is_empty() {
                        let refs: Vec<&str> = s.ignored.iter().map(String::as_str).collect();
                        c.findings.push(Finding::new(
                            RETIRED_CHECK,
                            Level::Warning,
                            &[path],
                            &refs,
                            vec![path.into()],
                            format!(
                                "{path}: {} no longer read (the layers' colours are the app's): delete {}",
                                s.ignored.join(", "),
                                if s.ignored.len() > 1 { "them" } else { "it" }
                            ),
                        ));
                    }
                    c.facts.insert("primary".into(), s.primary.clone().into());
                    c.facts.insert("metrics".into(), s.metrics.keys().cloned().collect::<Vec<_>>().into());
                }
                Err(why) => c.findings.push(err(SCORE_CHECK, &why)),
            },
            Kind::Declaration(..) => {
                let (_, findings, _) = super::credits::parse_description(path, bytes);
                c.findings = findings
                    .into_iter()
                    .map(|mut f| {
                        // (Its own check's name: the description's shape.)
                        f.id = f.id.replacen("description.", &format!("{DESC}."), 1);
                        f
                    })
                    .collect();
                match declaration(bytes) {
                    Ok(d) => {
                        if d.theme != theme {
                            c.findings
                                .push(err(THEME, &[format!("its `theme`, {}, isn't its folder's, {theme}", d.theme)]));
                        }
                        c.facts.insert("theme".into(), d.theme.clone().into());
                        if let Some(a) = &d.availability {
                            c.facts.insert("availability".into(), a.clone().into());
                        }
                        c.facts
                            .insert("blocks".into(), d.blocks.iter().map(|b| b.id.clone()).collect::<Vec<_>>().into());
                    }
                    Err(why) => {
                        let check = if why.iter().any(|w| w.starts_with("`theme`")) { THEME } else { DECL };
                        c.findings.push(err(check, &why));
                    }
                }
            }
            Kind::Pieces(_, id) => {
                let s = summary(bytes);
                if let Some(why) = &s.broken {
                    c.findings.push(err(PIECES, &[why.clone()]));
                    return c;
                }
                let mut why = Vec::new();
                if s.source.as_deref() != Some(id) {
                    why.push(format!("its `source` isn't {id:?}, its declaration's id (the file's stem)"));
                }
                if s.theme.as_deref() != Some(theme) {
                    c.findings.push(err(THEME, &[format!("its `theme` isn't its folder's, {theme}")]));
                }
                why.extend(s.read_bad.clone());
                if !why.is_empty() {
                    c.findings.push(err(PIECES, &why));
                }
                if !s.faults.is_empty() {
                    c.findings.push(about_pieces(
                        PIECE,
                        Level::Error,
                        &[path],
                        &s.faults,
                        format!(
                            "{path}: {} not in the shape (docs/inputs.md §6.11)",
                            plural(s.faults.len(), "piece", "pieces")
                        ),
                    ));
                }
                if !s.geometry.is_empty() {
                    c.findings.push(about_pieces(
                        GEOM,
                        Level::Error,
                        &[path],
                        &s.geometry,
                        format!("{path}: {} of invalid geometry", plural(s.geometry.len(), "piece", "pieces")),
                    ));
                }
                if !s.twice.is_empty() {
                    let ls: Vec<(usize, String, String)> =
                        s.twice.iter().map(|(n, id)| (*n, id.clone(), "an id an earlier piece has".into())).collect();
                    c.findings.push(about_pieces(
                        TWICE,
                        Level::Error,
                        &[path],
                        &ls,
                        format!(
                            "{path}: {} an earlier piece's id (one piece an id)",
                            plural(ls.len(), "piece has", "pieces have")
                        ),
                    ));
                }
                let unknown: Vec<(usize, String, String)> = s
                    .pieces
                    .iter()
                    .filter(|p| p.extent == "unknown")
                    .map(|p| (p.n, p.id.clone(), "extent unknown".into()))
                    .collect();
                if !unknown.is_empty() {
                    c.findings.push(about_pieces(
                        UNKNOWN,
                        Level::Warning,
                        &[path],
                        &unknown,
                        format!(
                            "{path}: {} of class unknown: listed in the sources, drawn nowhere",
                            plural(unknown.len(), "piece", "pieces")
                        ),
                    ));
                }
                let mut classes: BTreeMap<&str, u64> = BTreeMap::new();
                let mut avail: BTreeMap<&str, u64> = BTreeMap::new();
                for p in &s.pieces {
                    *classes.entry(&p.extent).or_default() += 1;
                    *avail.entry(&p.availability).or_default() += 1;
                }
                c.facts.insert("pieces".into(), s.pieces.len().into());
                c.facts.insert("classes".into(), serde_json::json!(classes));
                c.facts.insert("availability".into(), serde_json::json!(avail));
                if let Some(b) = s.bbox {
                    c.facts.insert("box".into(), serde_json::json!(b));
                }
            }
            Kind::Other => {}
        }
        c
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        let mut themes: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
        for p in &v.paths {
            if let Kind::Score(t) | Kind::Declaration(t, _) | Kind::Pieces(t, _) = kind(p) {
                if THEMES.contains(&t) {
                    themes.entry(t).or_default().push(p);
                }
            }
        }
        for (theme, paths) in themes {
            let sp = format!("{theme}/{SCORE}");
            let sc = if v.paths.contains(&sp) { score(&(v.read)(&sp)?).ok() } else { None };
            if !v.paths.contains(&sp) {
                let files: Vec<&str> = paths.iter().map(|p| p.as_str()).collect();
                out.push(Finding::new(
                    NO_SCORE,
                    Level::Error,
                    &[theme],
                    &[],
                    files.iter().map(|p| p.to_string()).collect(),
                    format!("{theme}/ has no {SCORE}: the theme's score names its metrics (docs/inputs.md §6.11)"),
                ));
            }
            let ids: BTreeSet<&str> = paths
                .iter()
                .filter_map(|p| match kind(p) {
                    Kind::Declaration(_, id) | Kind::Pieces(_, id) => Some(id),
                    _ => None,
                })
                .collect();
            for id in ids {
                let (t, g) = (format!("{theme}/{id}.toml"), format!("{theme}/{id}.geojson"));
                let (has_t, has_g) = (v.paths.contains(&t), v.paths.contains(&g));
                if has_g && !has_t {
                    out.push(Finding::new(
                        PAIR,
                        Level::Error,
                        &[id],
                        &[&t],
                        vec![g.clone()],
                        format!("{g} without {t}: a source's pieces come with its declaration"),
                    ));
                    continue;
                }
                if !has_g {
                    out.push(Finding::new(
                        NOT_YET,
                        Level::Warning,
                        &[&t],
                        &[],
                        vec![t.clone()],
                        format!("{t}: no {g} yet (made by producers/extents/extents.py): the source has no pieces"),
                    ));
                }
                let Ok(d) = declaration(&(v.read)(&t)?) else { continue };
                // (The declaration's metrics: named by the theme's score, constants of their kind.)
                if let Some(sc) = &sc {
                    let mut why = Vec::new();
                    for b in &d.blocks {
                        for (name, spec) in &b.metrics {
                            match sc.metrics.get(name) {
                                None => why.push(format!("piece {}: {name} isn't a metric of {sp}", b.id)),
                                Some(m) => {
                                    why.extend(declared_kind(m, spec).map(|w| format!("piece {}: {name}: {w}", b.id)))
                                }
                            }
                        }
                    }
                    if !why.is_empty() {
                        let refs: Vec<&str> = why.iter().map(String::as_str).collect();
                        out.push(Finding::new(
                            METRIC,
                            Level::Error,
                            &[&t, &sp],
                            &refs,
                            vec![t.clone(), sp.clone()],
                            format!("{t}: {}", why.join("; ")),
                        ));
                    }
                }
                if !has_g {
                    continue;
                }
                let s = summary(&(v.read)(&g)?);
                if s.broken.is_some() {
                    continue;
                }
                // (Made from this declaration: its blocks read, each piece of one of them and as it
                // makes them.)
                let by_id: BTreeMap<&str, &Block> = d.blocks.iter().map(|b| (b.id.as_str(), b)).collect();
                let blocks: BTreeSet<&str> = by_id.keys().copied().collect();
                let read: BTreeSet<&str> = s.read.iter().map(String::as_str).collect();
                let mut why = Vec::new();
                if s.read_bad.is_none() && read != blocks {
                    let fmt = |x: &BTreeSet<&str>| x.iter().copied().collect::<Vec<_>>().join(", ");
                    why.push(format!("it read the blocks {}, the declaration has {}", fmt(&read), fmt(&blocks)));
                }
                let strays: Vec<(usize, String, String)> = s
                    .pieces
                    .iter()
                    .filter_map(|p| {
                        match by_id.get(p.id.split('/').next().unwrap_or("")) {
                            None => Some("of no block of the declaration".to_string()),
                            Some(b) => b.unlike(p),
                        }
                        .map(|w| (p.n, p.id.clone(), w))
                    })
                    .collect();
                if !strays.is_empty() {
                    why.push(format!("{} not as its blocks make them", plural(strays.len(), "piece", "pieces")));
                }
                if !why.is_empty() {
                    let mut found = why.clone();
                    found.extend(strays.iter().map(|(_, id, _)| id.clone()));
                    let refs: Vec<&str> = found.iter().map(String::as_str).collect();
                    let mut f = Finding::new(
                        MADE,
                        Level::Error,
                        &[&g, &t],
                        &refs,
                        vec![g.clone(), t.clone()],
                        format!(
                            "{g} isn't made from {t} as it is (run producers/extents/extents.py again): {}",
                            why.join("; ")
                        ),
                    );
                    f.lines = strays.iter().map(|(n, id, w)| (*n, format!("{id}: {w}"))).collect();
                    out.push(f);
                }
                // (The pieces' metrics: named by the theme's score, of their scales' kinds.)
                if let Some(sc) = &sc {
                    let mut bad: Vec<(usize, String, String)> = Vec::new();
                    for p in &s.pieces {
                        for (k, val) in &p.metrics {
                            if k == "stated" {
                                if let Some(o) = val.as_object() {
                                    for m in o.keys().filter(|m| !sc.metrics.contains_key(*m)) {
                                        bad.push((p.n, p.id.clone(), format!("stated: {m} isn't a metric of {sp}")));
                                    }
                                }
                                continue;
                            }
                            match sc.metrics.get(k) {
                                None => bad.push((p.n, p.id.clone(), format!("{k} isn't a metric of {sp}"))),
                                Some(m) => {
                                    bad.extend(m.scale.refuses(val).map(|w| (p.n, p.id.clone(), format!("{k}: {w}"))))
                                }
                            }
                        }
                    }
                    if !bad.is_empty() {
                        out.push(about_pieces(
                            METRIC,
                            Level::Error,
                            &[&g, &sp],
                            &bad,
                            format!("{g}: {} with a metric not of {sp}", plural(bad.len(), "piece", "pieces")),
                        ));
                    }
                }
            }
        }
        Ok(out)
    }

    fn partners(&self, path: &str, all: &BTreeSet<String>) -> Vec<String> {
        let other = match kind(path) {
            Kind::Declaration(t, id) => format!("{t}/{id}.geojson"),
            Kind::Pieces(t, id) => format!("{t}/{id}.toml"),
            _ => return Vec::new(),
        };
        all.contains(&other).then_some(other).into_iter().collect()
    }
}

#[cfg(test)]
mod tests;
