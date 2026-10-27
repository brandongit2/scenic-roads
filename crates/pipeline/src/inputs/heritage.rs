//! The heritage registers (docs/inputs.md §6.5): a declare input. Its inputs folder, `inputs/heritage/`,
//! holds one pair of files per register, `<register>.geojson` (its entries: any geometry, with the
//! standard properties) and `<register>.toml` (its description, §5.1, with its `class`,
//! `territory`, `wikidata_property` and `osm_ref_tags`); the register's id is the files' stem. The
//! heritage-sites job reads every accepted register whose box meets its cover, generically
//! (dem/heritage.py; crate::heritage::registers), and the catalogue credits each by its description.
//!
//! The checks:
//! - **Errors:** a file that isn't a register's `.geojson` or `.toml`, or whose stem isn't a
//!   register id (lower-case letters, digits and `-`); a register's file without its partner; a
//!   `.geojson` that isn't a FeatureCollection; entries with an invalid geometry or a coordinate
//!   out of range, without an `id`, with a property of the wrong type (a `level` outside 1–5, a
//!   `wikidata` not `Q<digits>`, a `tier` the map doesn't know, a `kind` that isn't `biosphere`,
//!   `geopark` or `dark_sky` …), and entries missing what their register's class needs (`level`,
//!   and `designation` for `heritage`; `kind` for `special`), one finding per file and check listing
//!   them; an id twice; a description without its credit or licence, or with the licence
//!   `unverified`, an unknown `class`, a
//!   `territory` that isn't ISO codes or `"world"`, a `wikidata_property` that isn't `P<digits>`,
//!   `osm_ref_tags` that aren't strings.
//! - **Warnings** (each finding's id names its counts and versions, or the entries it lists): a
//!   register losing more than 10 % of its entries since its accepted version, or removed; more than
//!   half of its entries `approx`; entries without a name; entries more than 20 km outside the
//!   outlines of its declared territory (the newest pass's: NRHP's legation in Tangier is real).
//!
//! What's derived: each `.geojson`'s box and entry count (`facts.box`, `facts.entries`), and each
//! description's extent, its register's box (`facts.extent`), which the catalogue's credits list it by
//! (crate::inputs::credits).

use super::{Checks, Derived, FileCheck, FileEntry, Finding, Level, Version};
use crate::outlines::{OutlineRec, Outlines};
use anyhow::Result;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// The unit's name.
pub const UNIT: &str = "heritage";

/// The classes a register can be (how the map shows its entries).
pub const CLASSES: [&str; 3] = ["heritage", "world-heritage", "special"];

/// The kinds of designation the map's Heritage sites toggles group by (web/src/basemap.ts
/// HERITAGE_TIERS), an entry's `tier`.
pub const TIERS: [&str; 16] = [
    "w.c", "w.n", "n.top", "n.second", "n.lower", "n.mon", "n.land", "n.hist", "n.fed", "p.des", "p.reg", "p.area",
    "m.des", "m.reg", "m.area", "m.agr",
];

/// The kinds of special place the map knows (web/src/overlays.ts SPECIAL_LABEL).
pub const KINDS: [&str; 3] = ["biosphere", "geopark", "dark_sky"];

/// How far outside its territory's outlines an entry may lie before it's a warning (km).
pub const FAR_KM: f64 = 20.0;

/// The share of its entries a register may lose before it's a warning.
pub const LOSS: f64 = 0.10;

const SHAPE: &str = "he-shape";
const PAIR: &str = "he-pair";
const JSON: &str = "he-json";
const ENTRY: &str = "he-entry";
const TWICE: &str = "he-twice";
const CLASS: &str = "he-class";
const DESC: &str = "he-desc";
const LICENCE: &str = "he-licence";
const NEEDS: &str = "he-needs";
const LOST: &str = "he-loss";
const GONE: &str = "he-removed";
const APPROX: &str = "he-approx";
const UNNAMED: &str = "he-unnamed";
const FAR: &str = "he-far";

/// The registers' checks: the newest pass's outlines when they're known, and the accepted version's
/// entry counts by `.geojson` (with the copy they were counted in), for the losses
/// (`Checks::prepared` reads them for the check job).
#[derive(Default)]
pub struct Heritage {
    pub outlines: Option<Outlines>,
    pub prev: BTreeMap<String, (u64, String)>,
}

/// Whether `s` is a register id: lower-case letters, digits and `-`, a letter or digit first.
pub fn valid_id(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A register's id and which of its files `path` is (`toml` or `geojson`), when it's one.
pub fn split(path: &str) -> Option<(&str, &str)> {
    let (stem, ext) = path.rsplit_once('.')?;
    (valid_id(stem) && (ext == "toml" || ext == "geojson")).then_some((stem, ext))
}

/// An entry as the checks see it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    /// Where it is: a point's place, else its geometry's first position (lon, lat).
    pub at: [f64; 2],
    pub level: Option<i64>,
    pub designation: bool,
    pub kind: bool,
    pub approx: bool,
    pub named: bool,
}

/// A register's entries read: those in its shape, the faults of the others (feature number from 1,
/// its id or "", why), the ids twice, and its box.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub entries: Vec<Entry>,
    pub faults: Vec<(usize, String, String)>,
    pub twice: Vec<(usize, String)>,
    /// Why it isn't a FeatureCollection at all.
    pub broken: Option<String>,
    pub bbox: Option<[f64; 4]>,
}

/// Registers read, by their bytes' hash: the file check, the whole version's and the derived
/// facts read each file once.
static READ: Mutex<Option<HashMap<String, Arc<Summary>>>> = Mutex::new(None);

/// A register's entries (`summary_of`), read once per content.
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

/// A position's fault, or None.
fn position(v: &Value) -> Option<&'static str> {
    let a = v.as_array().filter(|a| a.len() >= 2 && a.iter().all(Value::is_number));
    let Some(a) = a else { return Some("a position that isn't two numbers") };
    let (x, y) = (a[0].as_f64().unwrap_or(f64::NAN), a[1].as_f64().unwrap_or(f64::NAN));
    if !(-180.0..=180.0).contains(&x) || !(-90.0..=90.0).contains(&y) {
        return Some("a coordinate out of range");
    }
    None
}

/// The positions of coordinates nested `depth` deep (0: one position), with the fewest a ring or
/// line needs at its innermost list (`least`), as `f` sees each; the first fault.
fn positions(v: &Value, depth: u8, least: usize, f: &mut impl FnMut([f64; 2])) -> Option<&'static str> {
    if depth == 0 {
        if let Some(e) = position(v) {
            return Some(e);
        }
        let a = v.as_array().unwrap();
        f([a[0].as_f64().unwrap(), a[1].as_f64().unwrap()]);
        return None;
    }
    let Some(a) = v.as_array() else { return Some("coordinates that aren't nested lists") };
    if a.is_empty() || (depth == 1 && a.len() < least) {
        return Some("a geometry without enough positions");
    }
    a.iter().find_map(|x| positions(x, depth - 1, least, f))
}

/// A geometry's fault, or None; its positions to `f`.
fn geometry(g: &Value, f: &mut impl FnMut([f64; 2])) -> Option<&'static str> {
    let (depth, least) = match g.get("type").and_then(Value::as_str) {
        Some("Point") => (0, 1),
        Some("MultiPoint") => (1, 1),
        Some("LineString") => (1, 2),
        Some("MultiLineString") => (2, 2),
        Some("Polygon") => (2, 4),
        Some("MultiPolygon") => (3, 4),
        _ => return Some("no geometry of a GeoJSON type"),
    };
    positions(g.get("coordinates").unwrap_or(&Value::Null), depth, least, f)
}

/// A property's fault, or None: the shape's properties by their types (null is absent).
fn property(k: &str, v: &Value) -> Option<String> {
    if v.is_null() {
        return None;
    }
    let s = || v.as_str().is_some();
    let ok = match k {
        "id" => v.as_str().is_some_and(|s| !s.is_empty()),
        "name" | "name_en" | "designation" | "date" | "municipality" | "type" | "authority" | "url" | "ref"
        | "site" | "osm_name" => s(),
        "level" => v.as_i64().is_some_and(|l| (1..=5).contains(&l)),
        "kind" => v.as_str().is_some_and(|x| KINDS.contains(&x)),
        "tier" => v.as_str().is_some_and(|x| TIERS.contains(&x)),
        "wikidata" => v.as_str().is_some_and(|q| {
            q.len() > 1 && q.starts_with('Q') && q[1..].bytes().all(|b| b.is_ascii_digit()) && !q[1..].starts_with('0')
        }),
        "approx" | "dot" => v.is_boolean(),
        "area_km2" | "site_area_km2" => v.is_number(),
        // (Anything else is the register's own, shown in the popup as it is.)
        _ => true,
    };
    (!ok).then(|| match k {
        "level" => "a level outside 1–5".to_string(),
        "kind" => format!("a kind not one of {}", KINDS.join(", ")),
        "tier" => "a tier the map doesn't know (web/src/basemap.ts HERITAGE_TIERS)".to_string(),
        "wikidata" => "a wikidata not Q<digits>".to_string(),
        "id" => "an id not a string".to_string(),
        _ => format!("{k} not of its type"),
    })
}

/// A register's `.geojson` read (`summary`).
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
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (i, f) in feats.iter().enumerate() {
        let n = i + 1;
        let p = f.get("properties").and_then(Value::as_object);
        let id = p.and_then(|p| p.get("id")).and_then(Value::as_str).unwrap_or("");
        let mut fault = |why: String| s.faults.push((n, id.to_string(), why));
        let Some(p) = p else {
            fault("no properties".into());
            continue;
        };
        let mut first: Option<[f64; 2]> = None;
        let mut fb = bbox;
        let g = f.get("geometry").unwrap_or(&Value::Null);
        if let Some(e) = geometry(g, &mut |c| {
            first.get_or_insert(c);
            fb = [fb[0].min(c[0]), fb[1].min(c[1]), fb[2].max(c[0]), fb[3].max(c[1])];
        }) {
            fault(e.into());
            continue;
        }
        if id.is_empty() {
            fault("no id".into());
            continue;
        }
        if let Some(e) = p.iter().find_map(|(k, v)| property(k, v)) {
            fault(e);
            continue;
        }
        if !seen.insert(id) {
            s.twice.push((n, id.to_string()));
            continue;
        }
        bbox = fb;
        let has = |k: &str| p.get(k).is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
        s.entries.push(Entry {
            id: id.to_string(),
            at: first.unwrap_or([0.0, 0.0]),
            level: p.get("level").and_then(Value::as_i64),
            designation: has("designation"),
            kind: has("kind"),
            approx: p.get("approx").and_then(Value::as_bool) == Some(true),
            named: has("name"),
        });
    }
    s.bbox = (!s.entries.is_empty()).then_some(bbox);
    s
}

/// A register's description's own fields, beside §5.1's (crate::inputs::credits::parse_description).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declared {
    pub class: String,
    pub territory: Vec<String>,
    pub wikidata_property: Option<String>,
    pub osm_ref_tags: Vec<String>,
}

/// A description's own fields, or what's wrong with them.
pub fn declared(bytes: &[u8]) -> std::result::Result<Declared, Vec<String>> {
    let Ok(t) =
        std::str::from_utf8(bytes).map_err(|_| ()).and_then(|t| toml::from_str::<toml::Table>(t).map_err(|_| ()))
    else {
        return Err(vec!["not TOML".into()]);
    };
    let mut bad = Vec::new();
    let class = t.get("class").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !CLASSES.contains(&class.as_str()) {
        bad.push(format!(
            "`class` is {}, not one of {}",
            if class.is_empty() { "missing".into() } else { format!("{class:?}") },
            CLASSES.join(", ")
        ));
    }
    let territory: Vec<String> = match t.get("territory") {
        Some(v) if v.as_str() == Some("world") => vec!["world".into()],
        Some(v) => {
            v.as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect()).unwrap_or_default()
        }
        None => Vec::new(),
    };
    if territory.is_empty() {
        bad.push("`territory` is required: ISO 3166 codes, or \"world\"".into());
    }
    let wikidata_property = t.get("wikidata_property").map(|v| v.as_str().unwrap_or("").to_string());
    if wikidata_property.as_deref().is_some_and(|p| {
        !(p.len() > 1 && p.starts_with('P') && p[1..].bytes().all(|b| b.is_ascii_digit()) && !p[1..].starts_with('0'))
    }) {
        bad.push("`wikidata_property` isn't P<digits>".into());
    }
    let osm_ref_tags = match t.get("osm_ref_tags") {
        None => Vec::new(),
        Some(v) => match v.as_array().and_then(|a| {
            a.iter().map(|x| x.as_str().filter(|s| !s.is_empty()).map(str::to_string)).collect::<Option<Vec<_>>>()
        }) {
            Some(tags) => tags,
            None => {
                bad.push("`osm_ref_tags` isn't a list of OSM keys".into());
                Vec::new()
            }
        },
    };
    if bad.is_empty() {
        Ok(Declared { class, territory, wikidata_property, osm_ref_tags })
    } else {
        Err(bad)
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// One finding listing entries of a register's `.geojson` (feature number, why): its id names them.
fn about_entries(check: &str, level: Level, path: &str, ls: &[(usize, String, String)], message: String) -> Finding {
    let raw: Vec<String> = ls.iter().map(|(_, id, why)| format!("{id} {why}")).collect();
    let refs: Vec<&str> = raw.iter().map(String::as_str).collect();
    let mut f = Finding::new(check, level, &[path], &refs, vec![path.to_string()], message);
    f.lines =
        ls.iter().map(|(n, id, why)| (*n, if id.is_empty() { why.clone() } else { format!("{id}: {why}") })).collect();
    f
}

/// Distance from `p` to the segment `a`–`b` (lon, lat), km, flat over a few km.
fn segment_km(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let k = p[1].to_radians().cos() * 111.32;
    let (ax, ay, bx, by) = ((a[0] - p[0]) * k, (a[1] - p[1]) * 110.57, (b[0] - p[0]) * k, (b[1] - p[1]) * 110.57);
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (-(ax * dx + ay * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (ax + t * dx).hypot(ay + t * dy)
}

/// A territory's outlines, simplified: each polygon's box and rings (degrees, and E7 for the
/// inside test).
type Shapes = Vec<([f64; 4], Vec<Vec<[f64; 2]>>, Vec<Vec<[i32; 2]>>)>;

impl Heritage {
    /// The outlines drawn under `codes`.
    fn shapes(&self, o: &Outlines, codes: &[String]) -> Result<Shapes> {
        let mut out = Vec::new();
        let recs: Vec<&OutlineRec> =
            o.recs.iter().filter(|r| o.code_of(r).is_some_and(|c| codes.contains(&c))).collect();
        for r in recs {
            for poly in o.simple_polygons(r)? {
                let rings: Vec<Vec<[f64; 2]>> = poly
                    .iter()
                    .map(|ring| ring.iter().map(|p| [p[0] as f64 * 1e-7, p[1] as f64 * 1e-7]).collect())
                    .collect();
                let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
                for p in rings.iter().flatten() {
                    b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                }
                out.push((b, rings, poly));
            }
        }
        Ok(out)
    }

    /// The entries more than `FAR_KM` outside `shapes`.
    fn far<'a>(shapes: &Shapes, entries: &'a [Entry]) -> Vec<&'a Entry> {
        let pad = FAR_KM / 110.0;
        entries
            .iter()
            .filter(|e| {
                let p = e.at;
                let near = |b: &[f64; 4]| {
                    let padx = pad / p[1].to_radians().cos().max(0.05);
                    p[0] >= b[0] - padx && p[0] <= b[2] + padx && p[1] >= b[1] - pad && p[1] <= b[3] + pad
                };
                let pe7 = [(p[0] * 1e7) as i32, (p[1] * 1e7) as i32];
                let within = shapes.iter().filter(|(b, ..)| near(b)).any(|(b, rings, e7)| {
                    let inside = p[0] >= b[0]
                        && p[0] <= b[2]
                        && p[1] >= b[1]
                        && p[1] <= b[3]
                        && crate::outlines::inside(&e7.iter().map(Vec::as_slice).collect::<Vec<_>>(), pe7);
                    inside || rings.iter().any(|r| r.windows(2).any(|w| segment_km(p, w[0], w[1]) <= FAR_KM))
                });
                !within
            })
            .collect()
    }
}

/// The content name of the newest pass's outlines the records `manifest` name.
fn outlines_of(manifest: &BTreeMap<String, String>) -> Option<&String> {
    let date = crate::osmpass::latest_pass_in(manifest)?;
    manifest.get(&format!("sources/osm/{date}/outlines"))
}

impl Checks for Heritage {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        1
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        let Some((rid, ext)) = split(path) else {
            c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not a register's file"], vec![path.into()], format!("{path} isn't the inputs folder's shape: heritage takes <register>.geojson and <register>.toml, the register's id lower-case letters, digits and - (docs/inputs.md §6.5); notes go in how/")));
            return c;
        };
        if ext == "toml" {
            let (d, findings, keyed) = super::credits::parse_description(path, bytes);
            c.findings = findings;
            // (A licence nobody checked never reaches the credits and popups: the register is held
            // until the owner sets the real one.)
            if let Some(l) = d.as_ref().map(|d| d.licence.trim()).filter(|l| l.eq_ignore_ascii_case("unverified")) {
                c.findings.push(Finding::new(LICENCE, Level::Error, &[path], &[l], vec![path.into()], format!("{path}: the licence is \"{l}\": check the source's terms (its notes say what) and set the real licence; the register is held until then")));
            }
            match declared(bytes) {
                Ok(own) => {
                    if let Some(d) = d {
                        c.facts.insert("description".into(), serde_json::to_value(&d).unwrap_or_default());
                        c.facts.insert("class".into(), own.class.clone().into());
                        // (What enters builds: the class, territory and how its ids are known; its
                        // credit reaches the catalogue through the credits' digest.)
                        let k = format!(
                            "{keyed}\nclass {}\nterritory {}\nwikidata {}\nosm {}",
                            own.class,
                            own.territory.join(","),
                            own.wikidata_property.as_deref().unwrap_or("-"),
                            own.osm_ref_tags.join(",")
                        );
                        c.keyed = Some(store::naming::hash16(k.as_bytes()));
                    }
                }
                Err(why) => {
                    let check = if why.iter().any(|w| w.starts_with("`class`")) { CLASS } else { DESC };
                    let refs: Vec<&str> = why.iter().map(String::as_str).collect();
                    c.findings.push(Finding::new(
                        check,
                        Level::Error,
                        &[path],
                        &refs,
                        vec![path.into()],
                        format!("{path}: {}", why.join("; ")),
                    ));
                }
            }
            return c;
        }
        let s = summary(bytes);
        if let Some(why) = &s.broken {
            c.findings.push(Finding::new(
                JSON,
                Level::Error,
                &[path],
                &[why],
                vec![path.into()],
                format!("{path}: {why}"),
            ));
            return c;
        }
        if !s.faults.is_empty() {
            c.findings.push(about_entries(
                ENTRY,
                Level::Error,
                path,
                &s.faults,
                format!(
                    "{path}: {} not in the shape (docs/inputs.md §6.5)",
                    plural(s.faults.len(), "entry", "entries")
                ),
            ));
        }
        if !s.twice.is_empty() {
            let ls: Vec<(usize, String, String)> =
                s.twice.iter().map(|(n, id)| (*n, id.clone(), "an id an earlier entry has".into())).collect();
            c.findings.push(about_entries(
                TWICE,
                Level::Error,
                path,
                &ls,
                format!(
                    "{path}: {} an earlier entry's id (one entry an id)",
                    plural(ls.len(), "entry has", "entries have")
                ),
            ));
        }
        let n = s.entries.len();
        let unnamed: Vec<(usize, String, String)> = s
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.named)
            .map(|(i, e)| (i + 1, e.id.clone(), "no name".into()))
            .collect();
        if !unnamed.is_empty() {
            c.findings.push(about_entries(
                UNNAMED,
                Level::Warning,
                path,
                &unnamed,
                format!(
                    "{rid}: {} of {n} entries without a name (they show as their designation alone)",
                    unnamed.len()
                ),
            ));
        }
        let approx = s.entries.iter().filter(|e| e.approx).count();
        if n > 0 && approx * 2 > n {
            let counts = format!("{approx} of {n}");
            c.findings.push(Finding::new(
                APPROX,
                Level::Warning,
                &[path],
                &[&counts],
                vec![path.into()],
                format!("{rid}: {approx} of its {n} entries located loosely (approx)"),
            ));
        }
        if let Some((was, copy)) = self.prev.get(path) {
            if (n as f64) < (*was as f64) * (1.0 - LOSS) {
                let found = [format!("{was} → {n}"), copy.clone(), store::naming::hash16(bytes)];
                let refs: Vec<&str> = found.iter().map(String::as_str).collect();
                let pct = 100.0 * (*was as f64 - n as f64) / *was as f64;
                c.findings.push(Finding::new(
                    LOST,
                    Level::Warning,
                    &[path],
                    &refs,
                    vec![path.into()],
                    format!("{rid}: {was} → {n} entries (a loss of {pct:.0} %)"),
                ));
            }
        }
        c.facts.insert("entries".into(), n.into());
        if approx > 0 {
            c.facts.insert("approx".into(), approx.into());
        }
        if let Some(b) = s.bbox {
            c.facts.insert("box".into(), serde_json::json!(b));
        }
        c
    }

    fn removed(&self, path: &str, was: &FileEntry) -> Vec<Finding> {
        let Some((rid, "geojson")) = split(path) else { return Vec::new() };
        let n = was.facts.get("entries").and_then(Value::as_u64).unwrap_or(0);
        vec![Finding::new(
            GONE,
            Level::Warning,
            &[path],
            &[&was.file],
            vec![path.into()],
            format!("{rid} removed: its {n} entries go from the map"),
        )]
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        let regs: BTreeSet<&str> = v.paths.iter().filter_map(|p| split(p)).map(|(r, _)| r).collect();
        let mut shapes: HashMap<Vec<String>, Shapes> = HashMap::new();
        for rid in regs {
            let (t, g) = (format!("{rid}.toml"), format!("{rid}.geojson"));
            let (has_t, has_g) = (v.paths.contains(&t), v.paths.contains(&g));
            if !(has_t && has_g) {
                let (have, lacks) = if has_t { (&t, &g) } else { (&g, &t) };
                out.push(Finding::new(
                    PAIR,
                    Level::Error,
                    &[rid],
                    &[lacks],
                    vec![have.clone()],
                    format!("{have} without {lacks}: a register is its entries and its description, added together"),
                ));
                continue;
            }
            let Ok(own) = declared(&(v.read)(&t)?) else { continue };
            let s = summary(&(v.read)(&g)?);
            let mut need: Vec<(usize, String, String)> = Vec::new();
            for (i, e) in s.entries.iter().enumerate() {
                let why = match own.class.as_str() {
                    "heritage" if e.level.is_none() || !e.designation => {
                        "no level or designation (a heritage register's entries need both)"
                    }
                    "world-heritage" if e.level.is_none() => "no level",
                    "special" if !e.kind => "no kind (a special place is a biosphere, geopark or dark_sky)",
                    _ => continue,
                };
                need.push((i + 1, e.id.clone(), why.into()));
            }
            if !need.is_empty() {
                out.push(about_entries(
                    NEEDS,
                    Level::Error,
                    &g,
                    &need,
                    format!(
                        "{g}: {} without what a {} register's entries need",
                        plural(need.len(), "entry", "entries"),
                        own.class
                    ),
                ));
            }
            // Entries far outside its territory (the newest pass's outlines, when they're known).
            if let (Some(o), false) = (&self.outlines, own.territory == ["world"]) {
                if !shapes.contains_key(&own.territory) {
                    shapes.insert(own.territory.clone(), self.shapes(o, &own.territory)?);
                }
                let sh = &shapes[&own.territory];
                if sh.is_empty() {
                    continue;
                }
                let far = Self::far(sh, &s.entries);
                if !far.is_empty() {
                    let ls: Vec<(usize, String, String)> =
                        far.iter().map(|e| (0, e.id.clone(), format!("{:.4}, {:.4}", e.at[1], e.at[0]))).collect();
                    let mut f = about_entries(
                        FAR,
                        Level::Warning,
                        &g,
                        &ls,
                        format!(
                            "{rid}: {} more than {FAR_KM} km outside {} (its territory)",
                            plural(far.len(), "entry", "entries"),
                            own.territory.join(", ")
                        ),
                    );
                    f.lines = far.iter().map(|e| (0, format!("{}: {:.4}, {:.4}", e.id, e.at[1], e.at[0]))).collect();
                    f.at = Some(far[0].at);
                    out.push(f);
                }
            }
        }
        Ok(out)
    }

    fn partners(&self, path: &str, all: &BTreeSet<String>) -> Vec<String> {
        let Some((rid, ext)) = split(path) else { return Vec::new() };
        let other = format!("{rid}.{}", if ext == "toml" { "geojson" } else { "toml" });
        all.contains(&other).then_some(other).into_iter().collect()
    }

    fn derive(&self, v: &Version, keep: &BTreeSet<String>) -> Result<Derived> {
        let mut d = Derived::default();
        for p in &v.paths {
            let Some((rid, "toml")) = split(p) else { continue };
            let g = format!("{rid}.geojson");
            if keep.contains(p) || !v.paths.contains(&g) {
                continue;
            }
            // (The description's extent: its register's box, which the catalogue's credits list it by.)
            let s = summary(&(v.read)(&g)?);
            let mut facts = serde_json::Map::new();
            if let Some(b) = s.bbox {
                facts.insert("extent".into(), serde_json::json!(b));
            }
            d.facts.insert(p.clone(), (facts, None));
        }
        Ok(d)
    }

    fn context(&self, manifest: &BTreeMap<String, String>) -> Vec<String> {
        // (The newest pass's outlines: a new pass checks the registers' territories again.)
        outlines_of(manifest).map(|o| format!("outlines {o}")).into_iter().collect()
    }

    fn prepared(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
    ) -> Result<Option<Box<dyn Checks>>> {
        let mut h = Heritage::default();
        if let Some(a) = super::open(root, manifest, UNIT)? {
            for (p, f) in &a.index.files {
                if let Some(n) = f.facts.get("entries").and_then(Value::as_u64) {
                    h.prev.insert(p.clone(), (n, f.file.clone()));
                }
            }
        }
        // (Outlines that can't be read now don't fail the check: it goes on without the
        // territories' warnings, which the next pass's check raises.)
        if let Some(o) = outlines_of(manifest) {
            match Outlines::open(&root.join(o)) {
                Ok(x) => h.outlines = Some(x),
                Err(e) => eprintln!(
                    "inputs: {UNIT}: the pass's outlines, {o}, can't be read ({e:#}): checked without the territories"
                ),
            }
        }
        Ok(Some(Box::new(h)))
    }
}

/// A register as the heritage jobs read it from the accepted version: its id, its copies and its
/// box (None: no entries).
#[derive(Clone, Debug, PartialEq)]
pub struct Register {
    pub id: String,
    pub geojson: String,
    pub toml: String,
    pub keyed: String,
    pub bbox: Option<[f64; 4]>,
}

/// The accepted registers (`a`'s pairs), in the order of their ids.
pub fn registers(a: &super::Accepted) -> Vec<Register> {
    let mut out = Vec::new();
    for (p, f) in &a.index.files {
        let Some((rid, "geojson")) = split(p) else { continue };
        let Some(t) = a.index.files.get(&format!("{rid}.toml")) else { continue };
        let bbox = f.facts.get("box").and_then(|b| serde_json::from_value::<[f64; 4]>(b.clone()).ok());
        out.push(Register {
            id: rid.to_string(),
            geojson: f.file.clone(),
            toml: t.file.clone(),
            keyed: format!("{} {}", f.keyed, t.keyed),
            bbox,
        });
    }
    out
}

/// An accepted version of one register meeting everywhere (`fixture`), its index named in records
/// `m`: the tests' (the agent's, whose plans read the registers).
#[cfg(test)]
pub fn accept_fixture(root: &store::nasfs::NasRoot, m: &mut BTreeMap<String, String>) {
    let mut files = BTreeMap::new();
    for (p, facts) in [
        ("fixture.geojson", serde_json::json!({"box": [-180.0, -85.0, 180.0, 85.0], "entries": 1})),
        ("fixture.toml", serde_json::json!({})),
    ] {
        let file = format!("{}/{}", super::store_dir(UNIT), p.replace('.', ".0000000000000000."));
        files.insert(
            p.to_string(),
            FileEntry { file, size: 1, keyed: format!("k {p}"), facts: facts.as_object().cloned().unwrap_or_default() },
        );
    }
    let index = super::Index {
        fmt: 1,
        unit: UNIT.into(),
        checks: format!("{UNIT} 1"),
        files,
        accepted: BTreeMap::new(),
        derived_from: None,
    };
    let body = serde_json::to_vec(&index).unwrap();
    let name = format!("{}/@index.{}.json", super::store_dir(UNIT), store::naming::hash16(&body));
    root.join(super::store_dir(UNIT)).create_dir_all().unwrap();
    root.join(&name).write(body).unwrap();
    m.insert(super::logical(UNIT), name);
}

#[cfg(test)]
mod tests;
