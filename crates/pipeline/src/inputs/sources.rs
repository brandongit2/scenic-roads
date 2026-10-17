//! The source declarations' drop box, `inputs/sources/` (docs/inputs.md §6.9, #145): one
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
//! drop box (§7.1). Each step's keys name a source by its declaration only where they named it in
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
pub const ROLES: [(&str, &[&str], &[&str]); 11] = [
    // (Land cover on the analysis grid: crate::landcover. 3° tiles by their south-west corner.)
    (LAND_COVER, &["geotiff"], &["{tile}"]),
    // (Canopy height and cover: crate::trees, scenic-metrics' canopy step. By an index of 10° squares.)
    (CANOPY, &["geotiff"], &[]),
    // (Forest leaf type, Europe: dem/leaftype.py, through the trees program.)
    (LEAF_EUROPE, &["image-service"], &[]),
    // (Forest leaf type, North America: dem/leaftype.py, one GeoTIFF in a zip read by range.)
    (LEAF_NORTH_AMERICA, &["zip-geotiff"], &[]),
    // (Building footprints, a release: crate::buildtiles, crate::bld, dem/buildings.py, dem/bldfetch.py.)
    (BUILDINGS, &["geoparquet"], &[]),
    // (Building heights on a grid: crate::bld, dem/bldfetch.py, dem/bldprep.py, dem/bldmeasure.py.)
    (BUILDING_HEIGHTS, &["zip-geotiff"], &[]),
    // (Wikidata's facts: dem/heritagewd.py.)
    (WIKIDATA, &["sparql"], &[]),
    // (Wikidata as it is now, by the Query Service's built-in prefixes: dem/whsshapes.py.)
    (WIKIDATA_LIVE, &["sparql"], &[]),
    // (Wikipedia's short descriptions and links: dem/heritagewd.py.)
    (WIKIPEDIA, &["mediawiki-api"], &[]),
    // (Wikipedia's monthly pageviews: dem/pageviews.py; the months are the items job's.)
    (PAGEVIEWS, &["pageviews-dump"], &["{y}", "{m}"]),
    // (Lakes missing from OSM, Canada: crate::lakefill. Shapefile zips, its files kept by hand.)
    (LAKE_FILL, &["zip-shapefile"], &[]),
];

pub const LAND_COVER: &str = "land-cover";
pub const CANOPY: &str = "canopy";
pub const LEAF_EUROPE: &str = "leaf-type-europe";
pub const LEAF_NORTH_AMERICA: &str = "leaf-type-north-america";
pub const BUILDINGS: &str = "buildings";
pub const BUILDING_HEIGHTS: &str = "building-heights";
pub const WIKIDATA: &str = "wikidata";
pub const WIKIDATA_LIVE: &str = "wikidata-live";
pub const WIKIPEDIA: &str = "wikipedia";
pub const PAGEVIEWS: &str = "pageviews";
pub const LAKE_FILL: &str = "lake-fill";

/// A source's `[data]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    /// The role the app reads it in (`ROLES`).
    pub role: String,
    /// Its format (one of its role's).
    pub format: String,
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
    const DESCRIPTION: [&str; 10] = ["name", "what", "credit", "licence", "licence_url", "source", "retrieved", "territory", "redistribute", "notes"];
    let mut why: Vec<String> = v.keys().filter(|k| !DESCRIPTION.contains(&k.as_str()) && k.as_str() != "data").map(|k| format!("`{k}` isn't a field of a source declaration (docs/inputs.md §6.9)")).collect();
    let data: Data = match v.get("data").cloned().map(|d| d.try_into::<Data>()) {
        None => return Err([why, vec!["no `[data]` table".into()]].concat()),
        Some(Err(e)) => return Err([why, vec![format!("`[data]`: {}", e.message())]].concat()),
        Some(Ok(d)) => d,
    };
    match ROLES.iter().find(|r| r.0 == data.role) {
        None => why.push(format!("`role` is \"{}\": one of {}", data.role, ROLES.iter().map(|r| r.0).collect::<Vec<_>>().join(", "))),
        Some((_, formats, placeholders)) => {
            if !formats.contains(&data.format.as_str()) {
                why.push(format!("`format` is \"{}\": the {} role reads {}", data.format, data.role, formats.join(", ")));
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
                        why.push(format!("{u}: {p} isn't a placeholder the {} role fills ({})", data.role, if placeholders.is_empty() { "none".to_string() } else { placeholders.join(", ") }));
                    }
                    rest = &rest[end..];
                }
            }
            if let Some(i) = &data.index {
                if i.contains('/') || !i.ends_with(".geojson") {
                    why.push(format!("`index` {i} isn't a `<name>.geojson` beside the declaration"));
                }
            }
        }
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
        let url = props.get("url").and_then(|u| u.as_str()).filter(|u| u.starts_with("https://") || u.starts_with("http://")).ok_or_else(|| format!("feature {} names no `url` (http or https)", i + 1))?.to_string();
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

/// What kind of file a drop-box path is.
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
            Kind::Other => c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not the drop box's shape"], vec![path.into()], format!("{path} isn't the drop box's shape: sources takes <id>.toml and an index <name>.geojson beside it (docs/inputs.md §6.9); notes go in how/"))),
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
                    out.push(Finding::new(MISSING, Level::Error, &[path], &[i], vec![path.clone()], format!("{path} names {i}, which isn't there")));
                }
            }
        }
        for (role, paths) in roles.iter().filter(|(_, p)| p.len() > 1) {
            out.push(Finding::new(ROLE, Level::Error, paths, &[role], paths.iter().map(|p| p.to_string()).collect(), format!("{} each declare the {role} role: the app reads one source a role", paths.join(", "))));
        }
        for p in &v.paths {
            if matches!(kind(p), Kind::Index) && !named.contains(p) {
                out.push(Finding::new(ORPHAN, Level::Warning, &[p], &[], vec![p.clone()], format!("{p}: no declaration names it as its index")));
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
                self.outlines.as_ref().and_then(|o| super::elevation::territory_boxes(o, &desc.territory).ok()).map(|b| serde_json::json!(b))
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

    fn prepared(&self, root: &Path, manifest: &BTreeMap<String, String>) -> Result<Option<Box<dyn Checks>>> {
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
        self.data.version.as_deref().with_context(|| format!("the {} source, {}, names no `version`", self.data.role, self.id))
    }

    /// Its format's field `k`.
    pub fn param(&self, k: &str) -> Result<&serde_json::Value> {
        self.data.params.get(k).with_context(|| format!("the {} source, {}, has no `params.{k}`", self.data.role, self.id))
    }

    /// Its format's field `k`, a string.
    pub fn param_str(&self, k: &str) -> Result<&str> {
        self.param(k)?.as_str().with_context(|| format!("the {} source, {}: `params.{k}` isn't a string", self.data.role, self.id))
    }

    /// Its format's field `k`, an integer.
    pub fn param_u64(&self, k: &str) -> Result<u64> {
        self.param(k)?.as_u64().with_context(|| format!("the {} source, {}: `params.{k}` isn't a whole number", self.data.role, self.id))
    }

    /// Its format's field `k`, strings.
    pub fn param_strs(&self, k: &str) -> Result<Vec<String>> {
        let v = self.param(k)?.as_array().with_context(|| format!("the {} source, {}: `params.{k}` isn't a list", self.data.role, self.id))?;
        v.iter().map(|x| x.as_str().map(str::to_string).with_context(|| format!("the {} source, {}: `params.{k}` holds something not a string", self.data.role, self.id))).collect()
    }

    /// The digest of what it names (its `[data]` and its index's features): what `key_part`
    /// compares with what the code named before it was declared.
    pub fn digest(&self) -> String {
        let feats: Vec<String> = self.index.iter().map(|f| format!("{:?} {} {}", f.bbox, f.url, serde_json::Value::Object(f.props.clone()))).collect();
        store::naming::hash16(format!("{}\n{}", serde_json::to_string(&self.data).unwrap_or_default(), feats.join("\n")).as_bytes())
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
}

impl Sources {
    /// The source declared for role `role`, or why there's none.
    pub fn role(&self, role: &str) -> Result<&Source> {
        self.by_role.get(role).with_context(|| format!("no source is declared for the {role} role (inputs/sources/, docs/inputs.md §6.9)"))
    }

    /// Every source, by role.
    pub fn all(&self) -> impl Iterator<Item = &Source> {
        self.by_role.values()
    }
}

/// The sources in accepted version `a` (each index read).
pub fn of(a: &super::Accepted) -> Result<Sources> {
    let mut by_role = BTreeMap::new();
    for (path, f) in &a.index.files {
        let Some(id) = path.strip_suffix(".toml") else { continue };
        let d: Declaration = serde_json::from_value(f.facts.get("declaration").cloned().with_context(|| format!("{path}: no declaration in the index"))?).with_context(|| format!("{path}: its declaration"))?;
        let index = match &d.data.index {
            Some(i) => parse_index(&a.read(i)?).map_err(|e| anyhow::anyhow!("{i}: {e}"))?,
            None => Vec::new(),
        };
        let role = d.data.role.clone();
        by_role.insert(role, Source { id: id.to_string(), data: d.data, keyed: f.keyed.clone(), index });
    }
    Ok(Sources { by_role })
}

/// The last accepted version read, by its index's content name (which names its content: the agent
/// plans with it every pass of its loop).
static READ: std::sync::Mutex<Option<(String, Sources)>> = std::sync::Mutex::new(None);

/// The accepted sources the records `manifest` name; None before any version is accepted.
pub fn open(root: &Path, manifest: &BTreeMap<String, String>) -> Result<Option<Sources>> {
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

/// The accepted sources, or why the step can't read them (none accepted yet: it waits for them).
pub fn read(root: &Path, manifest: &BTreeMap<String, String>) -> Result<Sources> {
    open(root, manifest)?.context("no version of the source declarations is accepted (inputs/sources/, docs/inputs.md §6.9)")
}

/// The environment variable naming the file a Python step reads the sources from (dem/declared.py).
pub const PYTHON_ENV: &str = "SCENIC_SOURCES";

/// The accepted sources as a Python step reads them (dem/declared.py): each by its role, its `id`,
/// `url`, `urls`, `version` and `params`, written as `sources.json` in `dir`; the file's path, for
/// `PYTHON_ENV`.
pub fn write_for_python(root: &Path, manifest: &BTreeMap<String, String>, dir: &Path) -> Result<std::path::PathBuf> {
    let s = read(root, manifest)?;
    let v: BTreeMap<&str, serde_json::Value> = s.all().map(|x| (x.data.role.as_str(), serde_json::json!({ "id": x.id, "url": x.data.url, "urls": x.data.urls, "version": x.data.version, "params": x.data.params }))).collect();
    std::fs::create_dir_all(dir)?;
    let p = std::path::absolute(dir.join("sources.json"))?;
    std::fs::write(&p, serde_json::to_vec_pretty(&v)?)?;
    Ok(p)
}

/// Whether the records name an accepted version of the declarations: the steps that read them wait
/// until they do.
pub fn accepted(manifest: &BTreeMap<String, String>) -> bool {
    manifest.contains_key(&super::logical(UNIT))
}

/// The staged declarations (producers/sources/), as the drop box takes them: path and bytes.
#[cfg(test)]
pub const STAGED: [(&str, &[u8]); 10] = [
    ("canvec-lakes.toml", include_bytes!("../../../../producers/sources/canvec-lakes.toml")),
    ("wikidata-qlever.toml", include_bytes!("../../../../producers/sources/wikidata-qlever.toml")),
    ("wikidata-query-service.toml", include_bytes!("../../../../producers/sources/wikidata-query-service.toml")),
    ("wikipedia-api.toml", include_bytes!("../../../../producers/sources/wikipedia-api.toml")),
    ("wikipedia-pageviews.toml", include_bytes!("../../../../producers/sources/wikipedia-pageviews.toml")),
    ("esa-worldcover.toml", include_bytes!("../../../../producers/sources/esa-worldcover.toml")),
    ("meta-canopy.toml", include_bytes!("../../../../producers/sources/meta-canopy.toml")),
    ("canopy-squares.geojson", include_bytes!("../../../../producers/sources/canopy-squares.geojson")),
    ("eea-leaf-type.toml", include_bytes!("../../../../producers/sources/eea-leaf-type.toml")),
    ("nalcms.toml", include_bytes!("../../../../producers/sources/nalcms.toml")),
];

/// An accepted version of the staged declarations (`STAGED`), its index named in records `m`: the
/// steps' tests read them as the build does.
#[cfg(test)]
pub fn accept_fixture(root: &Path, m: &mut BTreeMap<String, String>) {
    let mut files = BTreeMap::new();
    std::fs::create_dir_all(root.join(super::store_dir(UNIT))).unwrap();
    for (path, b) in STAGED {
        let c = SourcesCheck::default().file(path, b);
        assert!(c.findings.is_empty(), "{path}: {:?}", c.findings);
        let (stem, ext) = path.rsplit_once('.').unwrap();
        let file = format!("{}/{stem}.{}.{ext}", super::store_dir(UNIT), store::naming::hash16(b));
        std::fs::write(root.join(&file), b).unwrap();
        files.insert(path.to_string(), super::FileEntry { file, size: b.len() as u64, keyed: c.keyed.unwrap_or_else(|| store::naming::hash16(b)), facts: c.facts });
    }
    let index = super::Index { fmt: 1, unit: UNIT.into(), checks: format!("{UNIT} 1"), files, accepted: BTreeMap::new() };
    let body = serde_json::to_vec(&index).unwrap();
    let name = format!("{}/@index.{}.json", super::store_dir(UNIT), store::naming::hash16(&body));
    std::fs::write(root.join(&name), body).unwrap();
    m.insert(super::logical(UNIT), name);
}

/// The staged declarations, accepted (`accept_fixture`), read.
#[cfg(test)]
pub fn staged() -> Sources {
    let d = tempfile::tempdir().unwrap();
    let mut m = BTreeMap::new();
    accept_fixture(d.path(), &mut m);
    read(d.path(), &m).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(data: &str) -> String {
        format!("name = \"N\"\ncredit = \"C\"\nlicence = \"CC-BY-4.0\"\nterritory = \"world\"\n[data]\n{data}")
    }

    #[test]
    fn a_declarations_shape() {
        let ok = decl("role = \"pageviews\"\nformat = \"pageviews-dump\"\nurl = \"https://d.org/{y}/{y}-{m}.bz2\"\n");
        let c = SourcesCheck::default().file("pv.toml", ok.as_bytes());
        assert!(c.findings.is_empty() && c.keyed.is_some(), "{:?}", c.findings);
        let bad = |data: &str| {
            let c = SourcesCheck::default().file("x.toml", decl(data).as_bytes());
            assert!(c.findings.iter().any(|f| f.check() == DECL), "{data}: {:?}", c.findings);
        };
        bad("role = \"weather\"\nformat = \"geotiff\"\nurl = \"https://a.org/x.tif\"\n");
        bad("role = \"pageviews\"\nformat = \"geotiff\"\nurl = \"https://a.org/{y}\"\n");
        bad("role = \"pageviews\"\nformat = \"pageviews-dump\"\nurl = \"https://a.org/{z}\"\n");
        bad("role = \"canopy\"\nformat = \"geotiff\"\nurl = \"https://a.org/x\"\nindex = \"i.geojson\"\n");
        bad("role = \"canopy\"\nformat = \"geotiff\"\n");
        bad("role = \"canopy\"\nformat = \"geotiff\"\nindex = \"i.geojson\"\nrefresh = \"daily\"\n");
        bad("role = \"canopy\"\nformat = \"geotiff\"\nindex = \"i.geojson\"\nmirror = \"x\"\n");
        // (The description's own shape: its credit.)
        let c = SourcesCheck::default().file("x.toml", b"[data]\nrole = \"canopy\"\nformat = \"geotiff\"\nindex = \"i.geojson\"\n");
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
        let a = decl("role = \"canopy\"\nformat = \"geotiff\"\nindex = \"sq.geojson\"\n");
        let b = decl("role = \"canopy\"\nformat = \"geotiff\"\nindex = \"other.geojson\"\n");
        let files: BTreeMap<&str, Vec<u8>> = BTreeMap::from([("a.toml", a.into_bytes()), ("b.toml", b.into_bytes()), ("sq.geojson", ix.to_vec()), ("stray.geojson", ix.to_vec())]);
        let read = |p: &str| Ok(std::sync::Arc::new(files[p].clone()));
        let v = Version { paths: files.keys().map(|s| s.to_string()).collect(), read: &read };
        let checks: Vec<String> = SourcesCheck::default().whole(&v).unwrap().iter().map(|f| f.check().to_string()).collect();
        assert!(checks.contains(&MISSING.to_string()) && checks.contains(&ROLE.to_string()) && checks.contains(&ORPHAN.to_string()), "{checks:?}");
    }

    /// The declarations staged for the drop box (producers/sources/) pass the input check, file by
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

    /// What a Python step reads (dem/declared.py): each source by its role, the URLs the scripts
    /// had in code before they were declared.
    #[test]
    fn the_python_steps_sources() {
        let d = tempfile::tempdir().unwrap();
        let mut m = BTreeMap::new();
        accept_fixture(d.path(), &mut m);
        let p = write_for_python(d.path(), &m, &d.path().join("job")).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        // (heritagewd.py's WDQS and its Wikipedia API, whsshapes.py's query service, pageviews.py's DUMP.)
        assert_eq!(v[WIKIDATA]["url"], "https://qlever.dev/api/wikidata");
        assert_eq!(v[WIKIPEDIA]["url"], "https://en.wikipedia.org/w/api.php");
        assert_eq!(v[WIKIDATA_LIVE]["url"], "https://query.wikidata.org/sparql");
        assert_eq!(v[PAGEVIEWS]["url"], "https://dumps.wikimedia.org/other/pageview_complete/monthly/{y}/{y}-{m}/pageviews-{y}{m}-user.bz2");
    }

    #[test]
    fn a_key_part_is_the_codes_while_the_declaration_names_the_same() {
        let s = Source { id: "x".into(), data: parse(decl("role = \"pageviews\"\nformat = \"pageviews-dump\"\nurl = \"https://d.org/{y}-{m}\"\n").as_bytes()).unwrap().data, keyed: "k".into(), index: Vec::new() };
        assert_eq!(s.key_part(("old", &s.digest())), "old");
        assert_eq!(s.key_part(("old", "another")), "source x k");
    }
}
