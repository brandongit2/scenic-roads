//! The source declarations' inputs folder, `inputs/sources/` (docs/inputs.md §6.9, #145): one
//! `<id>.toml` per source the build reads that isn't an elevation dataset (those are §6.8's), each a
//! description (§5.1) and a `[data]` table: the role the app reads it in (`role`, one source a
//! role), its format, where to fetch it (`url`, `urls` or an `index` beside it), its version, its
//! refresh rule, and the format's own fields (`[data.params]`). An index (`<name>.geojson`, made by
//! a producer: §8) lists a source's files with their footprints, so no naming rule of the source's
//! lives in code.
//!
//! Checked: each declaration's shape (its description's, and its own: a known role and format, one
//! way of naming where it is, a template's placeholders the role's), each index's (a GeoJSON
//! FeatureCollection whose features each name a `url` and have a footprint); over the version, an
//! index named being there, one source a role (errors), and an index nothing names (a warning).
//! What's derived: each description's extent (`facts.extent`, the credits'), the world or its
//! territory's polygons' boxes in the newest pass's outlines.
//!
//! The steps read the accepted declarations through `open`/`of` (`Sources`, by role), never the
//! inputs folder (§7.1). Each step's keys name a source by its declaration only where they named it in
//! code before (`key_part`): while a declaration names what the code did, by digest, the part is
//! what the code's was, so nothing is built again when a source moves into its declaration.

use super::{Checks, Derived, FileCheck, Finding, Level, Version};
use crate::outlines::Outlines;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The unit's name.
pub const UNIT: &str = "sources";

const SHAPE: &str = "src-shape";
const DESC: &str = "src-desc";
const DECL: &str = "src-decl";
const INDEX: &str = "src-index";
const MISSING: &str = "src-missing";
const ROLE: &str = "src-role";
const ORPHAN: &str = "src-orphan";

/// The roles a source is read in, each with the formats it may be in and the placeholders its URL
/// template may hold: the app's slots, each read by one step's generic code.
pub const ROLES: [(&str, &[&str], &[&str]); 17] = [
    // (Land cover on the analysis grid: crate::landcover. 3° tiles by their south-west corner.)
    (LAND_COVER, &["geotiff"], &["{tile}"]),
    // (Canopy height and cover: crate::trees, scenic-metrics' canopy step. By an index of 10° squares.)
    (CANOPY, &["geotiff"], &[]),
    // (Forest leaf type, Europe: crate::leaftype, in the trees program.)
    (LEAF_EUROPE, &["image-service"], &[]),
    // (Forest leaf type, North America: crate::leaftype, one GeoTIFF in a zip read by range.)
    (LEAF_NORTH_AMERICA, &["zip-geotiff"], &[]),
    // (Building footprints, a release: crate::buildtiles, crate::bld.)
    (BUILDINGS, &["geoparquet"], &[]),
    // (Building heights on a grid: crate::bld.)
    (BUILDING_HEIGHTS, &["zip-geotiff"], &[]),
    // (Wikidata's facts: crate::heritagewd, crate::areadetails, crate::whsshapes, crate::items,
    // crate::pageviews.)
    (WIKIDATA, &["sparql"], &[]),
    // (Wikipedia's short descriptions: crate::heritagewd.)
    (WIKIPEDIA, &["mediawiki-api"], &[]),
    // (Wikipedia's monthly pageviews: crate::pageviews, which picks the months.)
    (PAGEVIEWS, &["pageviews-dump"], &["{y}", "{m}"]),
    // (Lakes missing from OSM, Canada: crate::lakefill. Shapefile zips, its files kept by hand.)
    (LAKE_FILL, &["zip-shapefile"], &[]),
    // (Depth under the water: crate::depth. Many sources, each a set of depth grids by an index,
    // made by a producer (producers/depth/) and kept in `sources/fetched/<id>/`: `MANY`.)
    (DEPTH, &["depth-grid"], &[]),
    // (Height references converted: crate::dem::geoid. Many sources, each a separation grid (a
    // GeoTIFF) or a Helmert transformation between two references, chained by the elevation step
    // to bring each roads' dataset's heights to EGM2008: `MANY`.)
    (GEOID, &["geoid-grid", "helmert"], &[]),
    // (The regions' datasets: crate::inputs::regions::data. Each a file in `sources/fetched/<id>/`,
    // its `params.path`.)
    (super::regions::data::SEA, &["geojson"], &[]),
    (super::regions::data::INTERNAL, &["geojson"], &[]),
    (super::regions::data::TERRITORIAL, &["geojson"], &[]),
    (super::regions::data::LAKES, &["zip-shapefile"], &[]),
    (super::regions::data::LAND, &["zip-shapefile"], &[]),
];

/// The roles any number of sources take (each read with the others, by its own `params.code`).
pub const MANY: [&str; 2] = [DEPTH, GEOID];

pub const LAND_COVER: &str = "land-cover";
pub const CANOPY: &str = "canopy";
pub const LEAF_EUROPE: &str = "leaf-type-europe";
pub const LEAF_NORTH_AMERICA: &str = "leaf-type-north-america";
pub const BUILDINGS: &str = "buildings";
pub const BUILDING_HEIGHTS: &str = "building-heights";
pub const WIKIDATA: &str = "wikidata";
pub const WIKIPEDIA: &str = "wikipedia";
pub const PAGEVIEWS: &str = "pageviews";
pub const LAKE_FILL: &str = "lake-fill";
pub const DEPTH: &str = "depth";
pub const GEOID: &str = "geoid";

/// A source's `[data]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    /// The role the app reads it in (`ROLES`).
    pub role: String,
    /// Its format (one of its role's).
    pub format: String,
    /// What fetching it is (crate::fetch::Kind: `api`, `data` or `page`), required: only a page's
    /// host's robots.txt is read first. Kept out of the digests and the accepted index (what's
    /// fetched is the same whatever it's called), so the steps read None.
    #[serde(default, skip_serializing)]
    pub kind: Option<crate::fetch::Kind>,
    /// Where it is: a URL, or a template of the role's placeholders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Its files' URLs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
    /// A GeoJSON index beside it: its files' footprints, each with its `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    /// The pinned release, where the source has releases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `pinned` (read as declared: a new release is a new declaration) or `live` (asked each run:
    /// a query service).
    #[serde(default = "pinned")]
    pub refresh: String,
    /// The format's own fields (a layer's name, the classes kept, a legend).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
}

fn pinned() -> String {
    "pinned".into()
}

/// A declaration's own fields: its `[data]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Declaration {
    pub data: Data,
}

/// Declaration `bytes` parsed: its `[data]`, or why it isn't one.
pub fn parse(bytes: &[u8]) -> std::result::Result<Declaration, Vec<String>> {
    let text = std::str::from_utf8(bytes).map_err(|_| vec!["isn't UTF-8 text".to_string()])?;
    let v: toml::Table = toml::from_str(text).map_err(|e| vec![format!("isn't TOML: {}", e.message())])?;
    const DESCRIPTION: [&str; 10] = [
        "name",
        "what",
        "credit",
        "licence",
        "licence_url",
        "source",
        "retrieved",
        "territory",
        "redistribute",
        "notes",
    ];
    let mut why: Vec<String> = v
        .keys()
        .filter(|k| !DESCRIPTION.contains(&k.as_str()) && k.as_str() != "data")
        .map(|k| format!("`{k}` isn't a field of a source declaration (docs/inputs.md §6.9)"))
        .collect();
    let data: Data = match v.get("data").cloned().map(|d| d.try_into::<Data>()) {
        None => return Err([why, vec!["no `[data]` table".into()]].concat()),
        Some(Err(e)) => return Err([why, vec![format!("`[data]`: {}", e.message())]].concat()),
        Some(Ok(d)) => d,
    };
    match ROLES.iter().find(|r| r.0 == data.role) {
        None => why.push(format!(
            "`role` is \"{}\": one of {}",
            data.role,
            ROLES.iter().map(|r| r.0).collect::<Vec<_>>().join(", ")
        )),
        Some((_, formats, placeholders)) => {
            if !formats.contains(&data.format.as_str()) {
                why.push(format!(
                    "`format` is \"{}\": the {} role reads {}",
                    data.format,
                    data.role,
                    formats.join(", ")
                ));
            }
            let ways = data.url.is_some() as usize + (!data.urls.is_empty()) as usize + data.index.is_some() as usize;
            if ways != 1 {
                why.push("names where it is not in exactly one way: `url`, `urls` or `index`".into());
            }
            for u in data.url.iter().chain(&data.urls) {
                if !(u.starts_with("https://") || u.starts_with("http://")) {
                    why.push(format!("{u} isn't an http or https URL"));
                }
                let mut rest = u.as_str();
                while let Some(i) = rest.find('{') {
                    let end = rest[i..].find('}').map_or(rest.len(), |j| i + j + 1);
                    let p = &rest[i..end];
                    if !placeholders.contains(&p) {
                        why.push(format!(
                            "{u}: {p} isn't a placeholder the {} role fills ({})",
                            data.role,
                            if placeholders.is_empty() { "none".to_string() } else { placeholders.join(", ") }
                        ));
                    }
                    rest = &rest[end..];
                }
            }
            if let Some(i) = &data.index {
                if i.contains('/') || !i.ends_with(".geojson") {
                    why.push(format!("`index` {i} isn't a `<name>.geojson` beside the declaration"));
                }
            }
            if data.role == DEPTH {
                why.extend(depth_params(&data));
            }
            if data.role == GEOID {
                why.extend(crate::dem::geoid::params_problems(&data));
            }
        }
    }
    match data.kind {
        None => why.push("no `kind`: api, data or page (docs/inputs.md §6.9)".into()),
        // (No Rust fetcher reads robots.txt: a page is a producer script's, `FETCHES = "page"`.)
        Some(crate::fetch::Kind::Page) => {
            why.push("`kind` is page: no step's fetcher reads robots.txt; a page is fetched by a producer script (docs/inputs.md §6.9)".into())
        }
        Some(_) => {}
    }
    if !["pinned", "live"].contains(&data.refresh.as_str()) {
        why.push(format!("`refresh` is \"{}\": pinned or live", data.refresh));
    }
    if why.is_empty() {
        Ok(Declaration { data })
    } else {
        Err(why)
    }
}

/// The fields of a depth source's `params` (docs/inputs.md §6.10, crate::depth::sources).
pub const DEPTH_PARAMS: [&str; 10] =
    ["code", "water", "label", "datum", "band_m", "intertidal", "datum_exact", "rivers", "estimated", "notice"];

/// What's wrong with a depth source's `[data]` (docs/inputs.md §6.10): an index (its depth grids),
/// and its params: `code` (1–15, the source byte the class tiles carry), `water` (`sea` or `lake`:
/// where it's drawn), `label` (a short name for the map's popup), `datum` (what its depths are
/// below, in words), and optionally `band_m` (the ground metres it's feathered in over, ≥ 0),
/// `intertidal` (its positive values are drying heights), `datum_exact` (false: its datum isn't
/// corrected to the others', so it fills gaps only, hard-edged), `rivers` (drawn in rivers too: its
/// depths are below a river's own surface there), `estimated` (every cell an estimate) and `notice`
/// (a notice its licence asks every map showing it to carry).
fn depth_params(d: &Data) -> Vec<String> {
    let mut why = Vec::new();
    let p = &d.params;
    if d.index.is_none() {
        why.push("a depth source names its grids by an `index`".into());
    }
    if !matches!(p.get("code").and_then(|v| v.as_u64()), Some(1..=15)) {
        why.push("`params.code` isn't a whole number 1–15 (the source byte the depth's class tiles carry)".into());
    }
    if !matches!(p.get("water").and_then(|v| v.as_str()), Some("sea" | "lake")) {
        why.push("`params.water` isn't \"sea\" or \"lake\"".into());
    }
    for k in ["label", "datum"] {
        if !p.get(k).is_some_and(|v| v.as_str().is_some_and(|s| !s.trim().is_empty())) {
            why.push(format!("`params.{k}` isn't a text"));
        }
    }
    if p.get("band_m").is_some_and(|v| !v.as_f64().is_some_and(|b| b >= 0.0)) {
        why.push("`params.band_m` isn't a number ≥ 0".into());
    }
    for k in ["intertidal", "datum_exact", "rivers", "estimated"] {
        if p.get(k).is_some_and(|v| !v.is_boolean()) {
            why.push(format!("`params.{k}` isn't true or false"));
        }
    }
    if p.get("notice").is_some_and(|v| !v.is_string()) {
        why.push("`params.notice` isn't a text".into());
    }
    for k in p.keys().filter(|k| !DEPTH_PARAMS.contains(&k.as_str())) {
        why.push(format!("`params.{k}` isn't a depth source's field"));
    }
    why
}

/// What's wrong with a depth source's index features: each grid's `file` (its name in
/// `sources/fetched/<id>/`) and `zoom` (the Web Mercator zoom whose pixels it's on), its
/// optional `low_file` (crate::depth::grid), and where packed its `pack` and `at`.
pub fn depth_index_problems(f: &[Feature]) -> Vec<String> {
    let mut why = Vec::new();
    for (i, x) in f.iter().enumerate() {
        let name_ok = |k: &str| {
            x.props
                .get(k)
                .and_then(|v| v.as_str())
                .is_some_and(|n| !n.is_empty() && !n.contains("..") && !n.starts_with('/') && n.ends_with(".tif"))
        };
        if !name_ok("file") {
            why.push(format!("feature {} names no `file` (a .tif in the source's folder)", i + 1));
        }
        if x.props.contains_key("low_file") && !name_ok("low_file") {
            why.push(format!("feature {}: `low_file` isn't a .tif in the source's folder", i + 1));
        }
        // (Packed, task #452: `pack`, a tar in the source's folder, and `at`, each file's byte range
        // in it, [offset, length].)
        if let Some(p) = x.props.get("pack") {
            if !p
                .as_str()
                .is_some_and(|n| !n.is_empty() && !n.contains('/') && !n.contains("..") && n.ends_with(".tar"))
            {
                why.push(format!("feature {}: `pack` isn't a .tar in the source's folder", i + 1));
            }
            for k in ["file", "low_file"] {
                let r = x.props.get("at").and_then(|a| a.get(k));
                // ([offset, length], or with the member's hash as packed: [offset, length, hash16].)
                let ok = r.and_then(|r| r.as_array()).is_some_and(|a| {
                    (a.len() == 2 || (a.len() == 3 && a[2].is_string()))
                        && a[0].as_u64().is_some()
                        && a[1].as_u64().is_some_and(|n| n > 0)
                });
                if x.props.contains_key(k) != r.is_some() || (r.is_some() && !ok) {
                    why.push(format!("feature {}: `at.{k}` isn't its file's [offset, length] in its pack", i + 1));
                }
            }
        } else if x.props.contains_key("at") {
            why.push(format!("feature {}: `at` without a `pack`", i + 1));
        }
        if !x.props.get("zoom").and_then(|v| v.as_u64()).is_some_and(|z| z <= 18) {
            why.push(format!("feature {} names no `zoom` 0–18", i + 1));
        }
        if why.len() > 20 {
            break;
        }
    }
    why
}

/// An index's file: its footprint (w, s, e, n, degrees) and properties (its `url`, and what the
/// role reads of it).
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub bbox: [f64; 4],
    pub url: String,
    pub props: serde_json::Map<String, serde_json::Value>,
}

/// Index `bytes` parsed: a GeoJSON FeatureCollection in WGS 84, each feature with a `url` and a
/// footprint.
pub fn parse_index(bytes: &[u8]) -> std::result::Result<Vec<Feature>, String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("isn't JSON: {e}"))?;
    if v["type"] != "FeatureCollection" {
        return Err("isn't a GeoJSON FeatureCollection".into());
    }
    let feats = v["features"].as_array().ok_or("has no features")?;
    let mut out = Vec::with_capacity(feats.len());
    for (i, f) in feats.iter().enumerate() {
        let props = f["properties"].as_object().cloned().unwrap_or_default();
        let url = props
            .get("url")
            .and_then(|u| u.as_str())
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            .ok_or_else(|| format!("feature {} names no `url` (http or https)", i + 1))?
            .to_string();
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        fn walk(v: &serde_json::Value, b: &mut [f64; 4]) {
            match v.as_array() {
                Some(a) if a.len() >= 2 && a[0].is_number() => {
                    let (x, y) = (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0));
                    *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                }
                Some(a) => a.iter().for_each(|x| walk(x, b)),
                None => {}
            }
        }
        walk(&f["geometry"]["coordinates"], &mut b);
        if !(b[0] <= b[2] && b[1] <= b[3]) {
            return Err(format!("feature {} has no footprint", i + 1));
        }
        out.push(Feature { bbox: b, url, props });
    }
    Ok(out)
}

/// What kind of file an inputs folder path is.
enum Kind<'a> {
    Declaration(&'a str),
    Index,
    Other,
}

fn kind(path: &str) -> Kind<'_> {
    if path.contains('/') {
        return Kind::Other;
    }
    match path.rsplit_once('.') {
        Some((id, "toml")) if crate::dem::declared::valid_id(id) => Kind::Declaration(id),
        Some((_, "geojson")) => Kind::Index,
        _ => Kind::Other,
    }
}

/// The digest of a declaration's fields that enter builds (its description's `keyed`, then its own).
fn keyed(desc: &str, d: &Declaration) -> String {
    store::naming::hash16(format!("{desc}\n{}", serde_json::to_string(d).unwrap_or_default()).as_bytes())
}

/// The checks: with the newest pass's outlines for the check job (`prepared`: a territory's
/// extent); without, the shapes alone.
#[derive(Default)]
pub struct SourcesCheck {
    pub outlines: Option<Outlines>,
}

impl SourcesCheck {
    fn declarations(v: &Version) -> Result<Vec<(String, String, super::credits::Description, Declaration)>> {
        let mut out = Vec::new();
        for p in &v.paths {
            let Kind::Declaration(id) = kind(p) else { continue };
            let b = (v.read)(p)?;
            let (Some(desc), _, _) = super::credits::parse_description(p, &b) else { continue };
            let Ok(d) = parse(&b) else { continue };
            out.push((p.clone(), id.to_string(), desc, d));
        }
        Ok(out)
    }
}

impl Checks for SourcesCheck {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        1
    }

    fn file(&self, path: &str, bytes: &[u8]) -> FileCheck {
        let mut c = FileCheck::default();
        let err = |check: &str, why: &[String]| {
            let refs: Vec<&str> = why.iter().map(String::as_str).collect();
            Finding::new(check, Level::Error, &[path], &refs, vec![path.into()], format!("{path}: {}", why.join("; ")))
        };
        match kind(path) {
            Kind::Declaration(_) => {
                let (desc, findings, dk) = super::credits::parse_description(path, bytes);
                c.findings = findings
                    .into_iter()
                    .map(|mut f| {
                        f.id = f.id.replacen("description.", &format!("{DESC}."), 1);
                        f
                    })
                    .collect();
                match parse(bytes) {
                    Ok(d) => {
                        c.refers.extend(d.data.index.clone());
                        if let Some(desc) = desc {
                            c.facts.insert("description".into(), serde_json::to_value(&desc).unwrap_or_default());
                            c.facts.insert("declaration".into(), serde_json::to_value(&d).unwrap_or_default());
                            c.keyed = Some(keyed(&dk, &d));
                        }
                    }
                    Err(why) => c.findings.push(err(DECL, &why)),
                }
            }
            Kind::Index => match parse_index(bytes) {
                Ok(f) => {
                    c.facts.insert("files".into(), f.len().into());
                }
                Err(e) => c.findings.push(err(INDEX, &[e])),
            },
            Kind::Other => c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not the inputs folder's shape"], vec![path.into()], format!("{path} isn't the inputs folder's shape: sources takes <id>.toml and an index <name>.geojson beside it (docs/inputs.md §6.9); notes go in how/"))),
        }
        c
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        let decls = Self::declarations(v)?;
        let mut named = BTreeSet::new();
        let mut roles: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (path, _, _, d) in &decls {
            roles.entry(d.data.role.as_str()).or_default().push(path);
            if let Some(i) = &d.data.index {
                named.insert(i.clone());
                if !v.paths.contains(i) {
                    out.push(Finding::new(
                        MISSING,
                        Level::Error,
                        &[path],
                        &[i],
                        vec![path.clone()],
                        format!("{path} names {i}, which isn't there"),
                    ));
                }
            }
        }
        let mut codes: BTreeMap<u64, Vec<&str>> = BTreeMap::new();
        for (path, _, _, d) in decls.iter().filter(|x| x.3.data.role == DEPTH) {
            if let Some(c) = d.data.params.get("code").and_then(|v| v.as_u64()) {
                codes.entry(c).or_default().push(path);
            }
            if let Some(i) = d.data.index.as_ref().filter(|i| v.paths.contains(i.as_str())) {
                let why = parse_index(&(v.read)(i)?).map(|f| depth_index_problems(&f)).unwrap_or_default();
                if !why.is_empty() {
                    let refs: Vec<&str> = why.iter().map(String::as_str).collect();
                    out.push(Finding::new(
                        INDEX,
                        Level::Error,
                        &[path, i],
                        &refs,
                        vec![i.clone()],
                        format!("{i} (the depth grids of {path}): {}", why.join("; ")),
                    ));
                }
            }
        }
        for (c, paths) in codes.iter().filter(|(_, p)| p.len() > 1) {
            out.push(Finding::new(
                ROLE,
                Level::Error,
                paths,
                &[&c.to_string()],
                paths.iter().map(|p| p.to_string()).collect(),
                format!("{} each take the depth source code {c}: one a source", paths.join(", ")),
            ));
        }
        for (role, paths) in roles.iter().filter(|(r, p)| p.len() > 1 && !MANY.contains(r)) {
            out.push(Finding::new(
                ROLE,
                Level::Error,
                paths,
                &[role],
                paths.iter().map(|p| p.to_string()).collect(),
                format!("{} each declare the {role} role: the app reads one source a role", paths.join(", ")),
            ));
        }
        for p in &v.paths {
            if matches!(kind(p), Kind::Index) && !named.contains(p) {
                out.push(Finding::new(
                    ORPHAN,
                    Level::Warning,
                    &[p],
                    &[],
                    vec![p.clone()],
                    format!("{p}: no declaration names it as its index"),
                ));
            }
        }
        Ok(out)
    }

    fn derive(&self, v: &Version, keep: &BTreeSet<String>) -> Result<Derived> {
        let mut d = Derived::default();
        for (path, _, desc, _) in Self::declarations(v)? {
            if keep.contains(&path) {
                continue;
            }
            // (The credits' extent: the world, or its territory's polygons' boxes in the pass's
            // outlines; unknown while they can't be read, or with no territory.)
            let extent = if desc.territory == ["world"] {
                Some(serde_json::json!("world"))
            } else if desc.territory.is_empty() {
                None
            } else {
                self.outlines
                    .as_ref()
                    .and_then(|o| super::elevation::territory_boxes(o, &desc.territory).ok())
                    .map(|b| serde_json::json!(b))
            };
            if let Some(e) = extent {
                let mut facts = serde_json::Map::new();
                facts.insert("extent".into(), e);
                d.facts.insert(path, (facts, None));
            }
        }
        Ok(d)
    }

    fn context(&self, manifest: &BTreeMap<String, String>) -> Vec<String> {
        super::elevation::outlines_of(manifest).map(|o| format!("outlines {o}")).into_iter().collect()
    }

    fn prepared(
        &self,
        root: &store::nasfs::NasRoot,
        manifest: &BTreeMap<String, String>,
    ) -> Result<Option<Box<dyn Checks>>> {
        let outlines = match super::elevation::outlines_of(manifest) {
            Some(o) => match Outlines::open(&root.join(o)) {
                Ok(x) => Some(x),
                Err(e) => {
                    eprintln!("inputs: {UNIT}: the pass's outlines, {o}, can't be read ({e:#})");
                    None
                }
            },
            None => None,
        };
        Ok(Some(Box::new(SourcesCheck { outlines })))
    }
}

// ---- The accepted declarations, as the steps read them ------------------------------------------

/// An accepted source.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// Its id (its file's stem).
    pub id: String,
    pub data: Data,
    /// Its `keyed` digest.
    pub keyed: String,
    /// Its index's features, read with it (none without an index).
    pub index: Vec<Feature>,
}

impl Source {
    /// Its URL (`url`), or why it has none.
    pub fn url(&self) -> Result<&str> {
        self.data.url.as_deref().with_context(|| format!("the {} source, {}, names no `url`", self.data.role, self.id))
    }

    /// Its pinned release.
    pub fn version(&self) -> Result<&str> {
        self.data
            .version
            .as_deref()
            .with_context(|| format!("the {} source, {}, names no `version`", self.data.role, self.id))
    }

    /// Its format's field `k`.
    pub fn param(&self, k: &str) -> Result<&serde_json::Value> {
        self.data
            .params
            .get(k)
            .with_context(|| format!("the {} source, {}, has no `params.{k}`", self.data.role, self.id))
    }

    /// Its format's field `k`, a string.
    pub fn param_str(&self, k: &str) -> Result<&str> {
        self.param(k)?
            .as_str()
            .with_context(|| format!("the {} source, {}: `params.{k}` isn't a string", self.data.role, self.id))
    }

    /// Its format's field `k`, an integer.
    pub fn param_u64(&self, k: &str) -> Result<u64> {
        self.param(k)?
            .as_u64()
            .with_context(|| format!("the {} source, {}: `params.{k}` isn't a whole number", self.data.role, self.id))
    }

    /// Its format's field `k`, strings.
    pub fn param_strs(&self, k: &str) -> Result<Vec<String>> {
        let v = self
            .param(k)?
            .as_array()
            .with_context(|| format!("the {} source, {}: `params.{k}` isn't a list", self.data.role, self.id))?;
        v.iter()
            .map(|x| {
                x.as_str().map(str::to_string).with_context(|| {
                    format!("the {} source, {}: `params.{k}` holds something not a string", self.data.role, self.id)
                })
            })
            .collect()
    }

    /// The digest of what it names (its `[data]` and its index's features): what `key_part`
    /// compares with what the code named before it was declared.
    pub fn digest(&self) -> String {
        let feats: Vec<String> = self
            .index
            .iter()
            .map(|f| format!("{:?} {} {}", f.bbox, f.url, serde_json::Value::Object(f.props.clone())))
            .collect();
        store::naming::hash16(
            format!("{}\n{}", serde_json::to_string(&self.data).unwrap_or_default(), feats.join("\n")).as_bytes(),
        )
    }

    /// What a step's key names of it, where the key named it in code before (`before`: the code's
    /// key part, and the digest of what the code named): that part while it names the same, else
    /// its id and `keyed` digest. Nothing is built again for the move into a declaration; a changed
    /// declaration is a new key.
    pub fn key_part(&self, before: (&str, &str)) -> String {
        if self.digest() == before.1 {
            before.0.to_string()
        } else {
            format!("source {} {}", self.id, self.keyed)
        }
    }

    /// `key_part` for a key that named nothing of it in code (`before`: the digest of what the code
    /// read): None while it names the same, so the key is as it was; else its id and `keyed`.
    pub fn key_extra(&self, before: &str) -> Option<String> {
        Some(self.key_part(("", before))).filter(|p| !p.is_empty())
    }
}

/// The accepted sources, by role.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sources {
    by_role: BTreeMap<String, Source>,
    /// The sources of the roles any number take (`MANY`), by role, in id order.
    many: BTreeMap<String, Vec<Source>>,
}

impl Sources {
    /// Sources from declarations read elsewhere (a scratch check's: `regions-check --declarations`),
    /// one a role (those of the `MANY` roles left out).
    pub fn of(v: Vec<Source>) -> Sources {
        Sources { by_role: v.into_iter().map(|s| (s.data.role.clone(), s)).collect(), many: BTreeMap::new() }
    }

    /// The declarations (`<id>.toml`) in folder `dir`, as sources.
    pub fn of_folder(dir: &Path) -> Result<Sources> {
        let mut v = Vec::new();
        for e in std::fs::read_dir(dir).with_context(|| format!("list {}", dir.display()))? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "toml") {
                let b = std::fs::read(&p)?;
                let d = parse(&b).map_err(|w| anyhow::anyhow!("{}: {}", p.display(), w.join("; ")))?;
                let id = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                v.push(Source { id, data: d.data, keyed: store::naming::hash16(&b), index: Vec::new() });
            }
        }
        Ok(Sources::of(v))
    }

    /// The source declared for role `role`, or why there's none.
    pub fn role(&self, role: &str) -> Result<&Source> {
        self.by_role.get(role).with_context(|| {
            format!("no source is declared for the {role} role (inputs/sources/, docs/inputs.md §6.9)")
        })
    }

    /// Every source, by role.
    pub fn all(&self) -> impl Iterator<Item = &Source> {
        self.by_role.values().chain(self.many.values().flatten())
    }

    /// The sources of a role any number take (`MANY`), in id order: none when none is declared.
    pub fn many(&self, role: &str) -> &[Source] {
        self.many.get(role).map_or(&[], Vec::as_slice)
    }
}

/// The sources in accepted version `a` (each index read).
pub fn of(a: &super::Accepted) -> Result<Sources> {
    let mut by_role = BTreeMap::new();
    let mut many: BTreeMap<String, Vec<Source>> = BTreeMap::new();
    for (path, f) in &a.index.files {
        let Some(id) = path.strip_suffix(".toml") else { continue };
        let d: Declaration = serde_json::from_value(
            f.facts.get("declaration").cloned().with_context(|| format!("{path}: no declaration in the index"))?,
        )
        .with_context(|| format!("{path}: its declaration"))?;
        let index = match &d.data.index {
            Some(i) => parse_index(&a.read(i)?).map_err(|e| anyhow::anyhow!("{i}: {e}"))?,
            None => Vec::new(),
        };
        let role = d.data.role.clone();
        let src = Source { id: id.to_string(), data: d.data, keyed: f.keyed.clone(), index };
        if MANY.contains(&role.as_str()) {
            many.entry(role).or_default().push(src);
        } else {
            by_role.insert(role, src);
        }
    }
    Ok(Sources { by_role, many })
}

/// The last accepted version read, by its index's content name (which names its content: the agent
/// plans with it every pass of its loop).
static READ: std::sync::Mutex<Option<(String, Sources)>> = std::sync::Mutex::new(None);

/// The accepted sources the records `manifest` name; None before any version is accepted.
pub fn open(root: &store::nasfs::NasRoot, manifest: &BTreeMap<String, String>) -> Result<Option<Sources>> {
    let Some(name) = manifest.get(&super::logical(UNIT)) else { return Ok(None) };
    if let Some((n, s)) = READ.lock().unwrap().as_ref() {
        if n == name {
            return Ok(Some(s.clone()));
        }
    }
    let Some(a) = super::open(root, manifest, UNIT)? else { return Ok(None) };
    let s = of(&a)?;
    *READ.lock().unwrap() = Some((name.clone(), s.clone()));
    Ok(Some(s))
}

/// The accepted sources as the build's records name them now (crate::out::read_manifest, under the
/// NAS project folder `root`).
pub fn open_now(root: &store::nasfs::NasRoot) -> Result<Option<Sources>> {
    let m = crate::out::read_manifest(root)?;
    open(root, &m)
}

/// The accepted sources, or why the step can't read them (none accepted yet: it waits for them).
pub fn read(root: &store::nasfs::NasRoot, manifest: &BTreeMap<String, String>) -> Result<Sources> {
    open(root, manifest)?
        .context("no version of the source declarations is accepted (inputs/sources/, docs/inputs.md §6.9)")
}

/// What the units' keys name of the declared sources they read (the land cover, the canopy): each
/// one's id and `keyed` digest where it isn't as staged (`Source::key_extra` against
/// agent::rekey::declared_before), together; None while all are, so the keys are as they were.
pub fn unit_extra(s: &Sources) -> Result<Option<String>> {
    use crate::agent::rekey::declared_before as b;
    extras(s, &[(LAND_COVER, b::LAND_COVER), (CANOPY, b::CANOPY)])
}

/// The same of tree cover's (the canopy, the leaf types).
pub fn trees_extra(s: &Sources) -> Result<Option<String>> {
    use crate::agent::rekey::declared_before as b;
    extras(s, &[(CANOPY, b::CANOPY), (LEAF_EUROPE, b::LEAF_EUROPE), (LEAF_NORTH_AMERICA, b::LEAF_NORTH_AMERICA)])
}

fn extras(s: &Sources, roles: &[(&str, &str)]) -> Result<Option<String>> {
    let mut v = Vec::new();
    for (role, before) in roles {
        v.extend(s.role(role)?.key_extra(before));
    }
    Ok((!v.is_empty()).then(|| v.join(" ")))
}

/// Whether the records name an accepted version of the declarations: the steps that read them wait
/// until they do.
pub fn accepted(manifest: &BTreeMap<String, String>) -> bool {
    manifest.contains_key(&super::logical(UNIT))
}

/// The staged declarations (producers/sources/), as the inputs folder takes them: path and bytes.
#[cfg(test)]
pub const STAGED: [(&str, &[u8]); 17] = [
    ("overture-buildings.toml", include_bytes!("../../../../producers/sources/overture-buildings.toml")),
    ("ghsl-built-h.toml", include_bytes!("../../../../producers/sources/ghsl-built-h.toml")),
    ("ghsl-tiles.geojson", include_bytes!("../../../../producers/sources/ghsl-tiles.geojson")),
    ("canvec-lakes.toml", include_bytes!("../../../../producers/sources/canvec-lakes.toml")),
    ("wikidata-qlever.toml", include_bytes!("../../../../producers/sources/wikidata-qlever.toml")),
    ("wikipedia-api.toml", include_bytes!("../../../../producers/sources/wikipedia-api.toml")),
    ("wikipedia-pageviews.toml", include_bytes!("../../../../producers/sources/wikipedia-pageviews.toml")),
    ("esa-worldcover.toml", include_bytes!("../../../../producers/sources/esa-worldcover.toml")),
    ("meta-canopy.toml", include_bytes!("../../../../producers/sources/meta-canopy.toml")),
    ("canopy-squares.geojson", include_bytes!("../../../../producers/sources/canopy-squares.geojson")),
    ("eea-leaf-type.toml", include_bytes!("../../../../producers/sources/eea-leaf-type.toml")),
    ("nalcms.toml", include_bytes!("../../../../producers/sources/nalcms.toml")),
    ("marineregions-eez.toml", include_bytes!("../../../../producers/sources/marineregions-eez.toml")),
    (
        "marineregions-internal-waters.toml",
        include_bytes!("../../../../producers/sources/marineregions-internal-waters.toml"),
    ),
    (
        "marineregions-territorial-seas.toml",
        include_bytes!("../../../../producers/sources/marineregions-territorial-seas.toml"),
    ),
    ("ne-lakes.toml", include_bytes!("../../../../producers/sources/ne-lakes.toml")),
    ("osm-land-polygons.toml", include_bytes!("../../../../producers/sources/osm-land-polygons.toml")),
];

/// An accepted version of the staged declarations (`STAGED`), its index named in records `m`: the
/// steps' tests read them as the build does.
#[cfg(test)]
pub fn accept_fixture(root: &store::nasfs::NasRoot, m: &mut BTreeMap<String, String>) {
    let mut files = BTreeMap::new();
    root.join(super::store_dir(UNIT)).create_dir_all().unwrap();
    for (path, b) in STAGED {
        let c = SourcesCheck::default().file(path, b);
        assert!(c.findings.is_empty(), "{path}: {:?}", c.findings);
        let (stem, ext) = path.rsplit_once('.').unwrap();
        let file = format!("{}/{stem}.{}.{ext}", super::store_dir(UNIT), store::naming::hash16(b));
        root.join(&file).write(b).unwrap();
        files.insert(
            path.to_string(),
            super::FileEntry {
                file,
                size: b.len() as u64,
                keyed: c.keyed.unwrap_or_else(|| store::naming::hash16(b)),
                facts: c.facts,
            },
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
    root.join(&name).write(body).unwrap();
    m.insert(super::logical(UNIT), name);
}

/// The staged declarations, accepted (`accept_fixture`), read.
#[cfg(test)]
pub fn staged() -> Sources {
    let d = tempfile::tempdir().unwrap();
    let mut m = BTreeMap::new();
    accept_fixture(&store::nasfs::NasRoot::local_for_test(d.path()), &mut m);
    read(&store::nasfs::NasRoot::local_for_test(d.path()), &m).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(data: &str) -> String {
        format!("name = \"N\"\ncredit = \"C\"\nlicence = \"CC-BY-4.0\"\nterritory = \"world\"\n[data]\n{data}")
    }

    #[test]
    fn a_declarations_shape() {
        let ok = decl("role = \"pageviews\"\nformat = \"pageviews-dump\"\nkind = \"data\"\nurl = \"https://d.org/{y}/{y}-{m}.bz2\"\n");
        let c = SourcesCheck::default().file("pv.toml", ok.as_bytes());
        assert!(c.findings.is_empty() && c.keyed.is_some(), "{:?}", c.findings);
        let bad = |data: &str| {
            let c = SourcesCheck::default().file("x.toml", decl(data).as_bytes());
            assert!(c.findings.iter().any(|f| f.check() == DECL), "{data}: {:?}", c.findings);
        };
        bad("role = \"pageviews\"\nformat = \"pageviews-dump\"\nurl = \"https://d.org/{y}\"\n");
        bad("role = \"pageviews\"\nformat = \"pageviews-dump\"\nkind = \"page\"\nurl = \"https://d.org/{y}\"\n");
        bad("role = \"pageviews\"\nformat = \"pageviews-dump\"\nkind = \"crawl\"\nurl = \"https://d.org/{y}\"\n");
        bad("role = \"weather\"\nformat = \"geotiff\"\nkind = \"data\"\nurl = \"https://a.org/x.tif\"\n");
        bad("role = \"pageviews\"\nformat = \"geotiff\"\nkind = \"data\"\nurl = \"https://a.org/{y}\"\n");
        bad("role = \"pageviews\"\nformat = \"pageviews-dump\"\nkind = \"data\"\nurl = \"https://a.org/{z}\"\n");
        bad("role = \"canopy\"\nformat = \"geotiff\"\nkind = \"data\"\nurl = \"https://a.org/x\"\nindex = \"i.geojson\"\n");
        bad("role = \"canopy\"\nformat = \"geotiff\"\nkind = \"data\"\n");
        bad("role = \"canopy\"\nformat = \"geotiff\"\nkind = \"data\"\nindex = \"i.geojson\"\nrefresh = \"daily\"\n");
        bad("role = \"canopy\"\nformat = \"geotiff\"\nkind = \"data\"\nindex = \"i.geojson\"\nmirror = \"x\"\n");
        // (The description's own shape: its credit.)
        let c = SourcesCheck::default()
            .file("x.toml", b"[data]\nrole = \"canopy\"\nformat = \"geotiff\"\nindex = \"i.geojson\"\n");
        assert!(c.findings.iter().any(|f| f.check() == DESC), "{:?}", c.findings);
        assert!(SourcesCheck::default().file("a/b.toml", b"").findings.iter().any(|f| f.check() == SHAPE));
    }

    #[test]
    fn an_index_and_the_version_whole() {
        let ix = br#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"url":"https://a.org/1.tif","kind":"p95"},"geometry":{"type":"Polygon","coordinates":[[[0,0],[10,0],[10,10],[0,10],[0,0]]]}}]}"#;
        let f = parse_index(ix).unwrap();
        assert_eq!(f[0].bbox, [0.0, 0.0, 10.0, 10.0]);
        assert_eq!(f[0].props["kind"], "p95");
        assert!(parse_index(br#"{"type":"FeatureCollection","features":[{"properties":{},"geometry":null}]}"#).is_err());
        let a = decl("role = \"canopy\"\nformat = \"geotiff\"\nkind = \"data\"\nindex = \"sq.geojson\"\n");
        let b = decl("role = \"canopy\"\nformat = \"geotiff\"\nkind = \"data\"\nindex = \"other.geojson\"\n");
        let files: BTreeMap<&str, Vec<u8>> = BTreeMap::from([
            ("a.toml", a.into_bytes()),
            ("b.toml", b.into_bytes()),
            ("sq.geojson", ix.to_vec()),
            ("stray.geojson", ix.to_vec()),
        ]);
        let read = |p: &str| Ok(std::sync::Arc::new(files[p].clone()));
        let v = Version { paths: files.keys().map(|s| s.to_string()).collect(), read: &read };
        let checks: Vec<String> =
            SourcesCheck::default().whole(&v).unwrap().iter().map(|f| f.check().to_string()).collect();
        assert!(
            checks.contains(&MISSING.to_string())
                && checks.contains(&ROLE.to_string())
                && checks.contains(&ORPHAN.to_string()),
            "{checks:?}"
        );
    }

    /// The declarations staged for the inputs folder (producers/sources/) pass the input check, file by
    /// file and whole, one a role.
    #[test]
    fn the_staged_declarations_pass_the_check() {
        let files: BTreeMap<&str, &[u8]> = STAGED.iter().copied().collect();
        for (p, b) in &files {
            let c = SourcesCheck::default().file(p, b);
            assert!(c.findings.is_empty(), "{p}: {:?}", c.findings);
        }
        let read = |p: &str| Ok(std::sync::Arc::new(files[p].to_vec()));
        let v = Version { paths: files.keys().map(|s| s.to_string()).collect(), read: &read };
        assert_eq!(SourcesCheck::default().whole(&v).unwrap(), Vec::new());
        let s = staged();
        assert_eq!(s.all().count(), files.keys().filter(|p| p.ends_with(".toml")).count());
    }

    /// The accepted sources by role: the URLs the steps had in code before they were declared.
    #[test]
    fn the_steps_sources() {
        let d = tempfile::tempdir().unwrap();
        let mut m = BTreeMap::new();
        accept_fixture(&store::nasfs::NasRoot::local_for_test(d.path()), &mut m);
        let s = read(&store::nasfs::NasRoot::local_for_test(d.path()), &m).unwrap();
        // (crate::heritagewd's query service and its Wikipedia API, crate::pageviews's dumps.)
        assert_eq!(s.role(WIKIDATA).unwrap().url().unwrap(), "https://qlever.dev/api/wikidata");
        assert_eq!(s.role(WIKIPEDIA).unwrap().url().unwrap(), "https://en.wikipedia.org/w/api.php");
        assert_eq!(
            s.role(PAGEVIEWS).unwrap().url().unwrap(),
            "https://dumps.wikimedia.org/other/pageview_complete/monthly/{y}/{y}-{m}/pageviews-{y}{m}-user.bz2"
        );
    }

    /// The units' and tree cover's keys name the land-cover, canopy and leaf-type declarations only
    /// once they aren't as staged: nothing as they are, the changed one's id and `keyed` otherwise.
    #[test]
    fn the_units_and_trees_keys_name_their_sources_once_changed() {
        use crate::agent::rekey::declared_before as b;
        let s = staged();
        for (role, before) in [
            (LAND_COVER, b::LAND_COVER),
            (CANOPY, b::CANOPY),
            (LEAF_EUROPE, b::LEAF_EUROPE),
            (LEAF_NORTH_AMERICA, b::LEAF_NORTH_AMERICA),
        ] {
            assert_eq!(s.role(role).unwrap().digest(), before, "{role}");
        }
        assert_eq!((unit_extra(&s).unwrap(), trees_extra(&s).unwrap()), (None, None));
        let mut changed = s.clone();
        let lc = changed.by_role.get_mut(LAND_COVER).unwrap();
        lc.data.params.insert("legend".into(), serde_json::json!({ "trees": [10] }));
        let want = format!("source esa-worldcover {}", lc.keyed);
        assert_eq!((unit_extra(&changed).unwrap(), trees_extra(&changed).unwrap()), (Some(want), None));
        let leaf = changed.by_role.get_mut(LEAF_EUROPE).unwrap();
        leaf.data.url = Some("https://example.org/other".into());
        let want = format!("source eea-leaf-type {}", leaf.keyed);
        assert_eq!(trees_extra(&changed).unwrap(), Some(want));
    }

    /// The depth's declarations staged in producers/depth/declarations/ pass the file check, one code
    /// each; a depth index's grids need a file and a zoom.
    #[test]
    fn the_geoid_declarations_pass_the_check_and_reach_egm2008() {
        let staged: [(&str, &[u8]); 6] = [
            ("cgg2013a.toml", include_bytes!("../../../../producers/geoid/declarations/cgg2013a.toml")),
            ("egm2008.toml", include_bytes!("../../../../producers/geoid/declarations/egm2008.toml")),
            ("geoid12b-alaska.toml", include_bytes!("../../../../producers/geoid/declarations/geoid12b-alaska.toml")),
            ("geoid18-conus.toml", include_bytes!("../../../../producers/geoid/declarations/geoid18-conus.toml")),
            ("gsigeo2011.toml", include_bytes!("../../../../producers/geoid/declarations/gsigeo2011.toml")),
            ("nad83-itrf2008.toml", include_bytes!("../../../../producers/geoid/declarations/nad83-itrf2008.toml")),
        ];
        let mut all = Vec::new();
        for (p, b) in staged {
            let c = SourcesCheck::default().file(p, b);
            assert!(c.findings.is_empty(), "{p}: {:?}", c.findings);
            let id = p.trim_end_matches(".toml").to_string();
            all.push(Source { id, data: parse(b).unwrap().data, keyed: store::naming::hash16(b), index: Vec::new() });
        }
        // Every reference the live roads' declarations state reaches EGM2008 (§6.8).
        for (from, hops) in [("NAVD88", 3), ("CGVD2013", 3), ("JGD2011", 2)] {
            let c = crate::dem::geoid::conversion(from, &all).unwrap().unwrap();
            assert_eq!(c.hops.len(), hops, "{from}: {:?}", c.hops);
        }
        assert!(crate::dem::geoid::conversion("EGM2008", &all).unwrap().is_none());
    }

    #[test]
    fn the_depth_declarations_pass_the_check() {
        let staged: [(&str, &[u8]); 15] = [
            ("bluetopo.toml", include_bytes!("../../../../producers/depth/declarations/bluetopo.toml")),
            ("emodnet-dtm.toml", include_bytes!("../../../../producers/depth/declarations/emodnet-dtm.toml")),
            ("gblq.toml", include_bytes!("../../../../producers/depth/declarations/gblq.toml")),
            ("gebco-2026.toml", include_bytes!("../../../../producers/depth/declarations/gebco-2026.toml")),
            ("globathy.toml", include_bytes!("../../../../producers/depth/declarations/globathy.toml")),
            ("gttm.toml", include_bytes!("../../../../producers/depth/declarations/gttm.toml")),
            (
                "masswildlife-ponds.toml",
                include_bytes!("../../../../producers/depth/declarations/masswildlife-ponds.toml"),
            ),
            ("mn-dnr-lakes.toml", include_bytes!("../../../../producers/depth/declarations/mn-dnr-lakes.toml")),
            ("ncei-crm.toml", include_bytes!("../../../../producers/depth/declarations/ncei-crm.toml")),
            ("ncei-greatlakes.toml", include_bytes!("../../../../producers/depth/declarations/ncei-greatlakes.toml")),
            ("nonna-10.toml", include_bytes!("../../../../producers/depth/declarations/nonna-10.toml")),
            ("nonna-100.toml", include_bytes!("../../../../producers/depth/declarations/nonna-100.toml")),
            ("shom-homonim-100.toml", include_bytes!("../../../../producers/depth/declarations/shom-homonim-100.toml")),
            ("shom-homonim-20.toml", include_bytes!("../../../../producers/depth/declarations/shom-homonim-20.toml")),
            ("smalllakes.toml", include_bytes!("../../../../producers/depth/declarations/smalllakes.toml")),
        ];
        let mut codes = BTreeSet::new();
        for (p, b) in staged {
            let c = SourcesCheck::default().file(p, b);
            assert!(c.findings.is_empty(), "{p}: {:?}", c.findings);
            let d = parse(b).unwrap();
            assert!(codes.insert(d.data.params["code"].as_u64().unwrap()), "{p}");
        }
        let ok = br#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"url":"https://a.org/x","file":"a.tif","zoom":8},"geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}}]}"#;
        assert!(depth_index_problems(&parse_index(ok).unwrap()).is_empty());
        let bad = br#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"url":"https://a.org/x","file":"../a.tif"},"geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}}]}"#;
        assert_eq!(depth_index_problems(&parse_index(bad).unwrap()).len(), 2);
        // (Packed, task #452: a range for each file it names, and none for one it doesn't.)
        let packed = |props: &str| {
            let j = format!(
                r#"{{"type":"FeatureCollection","features":[{{"type":"Feature","properties":{{"url":"https://a.org/x","file":"a.tif","zoom":8,{props}}},"geometry":{{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}}}}]}}"#
            );
            depth_index_problems(&parse_index(j.as_bytes()).unwrap())
        };
        assert!(packed(r#""pack":"6-1-2.0123456789abcdef.tar","at":{"file":[512,100]}"#).is_empty());
        assert!(packed(r#""pack":"p.tar","at":{"file":[512,100,"0123456789abcdef"]}"#).is_empty());
        assert_eq!(packed(r#""pack":"p.tar","at":{"file":[512,100,7]}"#).len(), 1);
        assert!(
            packed(r#""low_file":"a.low.tif","pack":"p.tar","at":{"file":[512,100],"low_file":[1024,9]}"#).is_empty()
        );
        assert_eq!(packed(r#""low_file":"a.low.tif","pack":"p.tar","at":{"file":[512,100]}"#).len(), 1);
        assert_eq!(packed(r#""pack":"p.tar","at":{"file":[512,100],"low_file":[0,1]}"#).len(), 1);
        assert_eq!(packed(r#""pack":"../p.tar","at":{"file":[512,0]}"#).len(), 2);
        assert_eq!(packed(r#""at":{"file":[512,100]}"#).len(), 1);
        let wrong = decl("role = \"depth\"\nformat = \"depth-grid\"\nkind = \"data\"\nindex = \"i.geojson\"\n[data.params]\ncode = 16\nwater = \"river\"\n");
        assert!(SourcesCheck::default().file("w.toml", wrong.as_bytes()).findings.iter().any(|f| f.check() == DECL));
        let flags = decl("role = \"depth\"\nformat = \"depth-grid\"\nkind = \"data\"\nindex = \"i.geojson\"\n[data.params]\ncode = 15\nwater = \"lake\"\nlabel = \"L\"\ndatum = \"D\"\nrivers = \"yes\"\nestimated = true\n");
        let f = SourcesCheck::default().file("f.toml", flags.as_bytes()).findings;
        assert!(f.len() == 1 && f[0].message.contains("params.rivers"), "{f:?}");
    }

    #[test]
    fn a_key_part_is_the_codes_while_the_declaration_names_the_same() {
        let s = Source {
            id: "x".into(),
            data: parse(
                decl("role = \"pageviews\"\nformat = \"pageviews-dump\"\nkind = \"data\"\nurl = \"https://d.org/{y}-{m}\"\n").as_bytes(),
            )
            .unwrap()
            .data,
            keyed: "k".into(),
            index: Vec::new(),
        };
        assert_eq!(s.key_part(("old", &s.digest())), "old");
        assert_eq!(s.key_part(("old", "another")), "source x k");
    }
}
