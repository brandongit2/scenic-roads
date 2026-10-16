//! The elevation datasets' drop box, `inputs/elevation/` (docs/inputs.md §6.8, #143): a declaration
//! per dataset (`<dataset>.toml`: a description, §5.1, and crate::dem::declared's fields), a tile
//! index beside it when it names one (`<name>.geojson`), and the files of a dataset dropped whole in
//! `<dataset>/`.
//!
//! Checked: each declaration's shape (its description's, and its own: surface, height reference,
//! code, data), each index's; then, over the version, what they name being there, and the data
//! itself read where it lies (`measure`: a sample of each dataset's GeoTIFF headers, by range
//! requests: their CRS, resolution and footprints; a tile pyramid's coverage from its territory's
//! outlines). Errors: a shape broken, a height reference or encoding the app doesn't know, a CRS
//! the app can't project, a file named that isn't there, data that can't be read. Warnings: a
//! dataset's folder or an index no declaration names; two datasets of one rank and resolution
//! meeting (their order would fall to their ids).
//!
//! What's measured goes in the index with the declaration (`facts.measured`, `facts.extent`), so the
//! unit jobs read the order and coverage from the index alone (crate::dem::declared::plan). The
//! build reads the datasets once the records say the readers switched (`@switched`: the migration's
//! proof, `scenic-build elevation-proof`, says every vertex that differs from today's lies in a
//! blend band, of exactly the version the records name: `switch`).

use super::{Checks, Derived, FileCheck, Finding, Level, Version};
use crate::dem::declared::{self, Data, Declaration, Measured};
use crate::fetch::Fetch;
use crate::outlines::Outlines;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The unit's name.
pub const UNIT: &str = "elevation";

/// The migration's proof (`scenic-build elevation-proof`): the version it proved, and whether every
/// vertex that differed lay in a blend band.
pub const PROOF: &str = "state/inputs/elevation-proof.json";

const SHAPE: &str = "el-shape";
const DESC: &str = "el-desc";
const DECL: &str = "el-decl";
const INDEX: &str = "el-index";
const MISSING: &str = "el-missing";
const DATA: &str = "el-data";
const ORPHAN: &str = "el-orphan";
const TIE: &str = "el-tie";

/// The checks: with the data's reader and the newest pass's outlines for the check job
/// (`prepared`); without, the shapes alone.
#[derive(Default)]
pub struct Elevation {
    pub fetch: Option<Arc<dyn Fetch>>,
    pub outlines: Option<Outlines>,
    /// What was measured, by what it was measured from (a declaration's `keyed` and its index's
    /// hash): a version checked whole twice reads the data once.
    memo: Mutex<HashMap<String, std::result::Result<Measured, String>>>,
}

impl Elevation {
    pub fn new(fetch: Arc<dyn Fetch>, outlines: Option<Outlines>) -> Elevation {
        Elevation { fetch: Some(fetch), outlines, memo: Mutex::default() }
    }
}

/// What kind of file a drop-box path is: a declaration (its dataset's id), an index, or a dataset's
/// file.
enum Kind<'a> {
    Declaration(&'a str),
    Index,
    Data(&'a str),
    Other,
}

fn kind(path: &str) -> Kind<'_> {
    match path.split_once('/') {
        Some((id, _)) if declared::valid_id(id) => Kind::Data(id),
        Some(_) => Kind::Other,
        None => match path.rsplit_once('.') {
            Some((id, "toml")) if declared::valid_id(id) => Kind::Declaration(id),
            Some((_, "geojson")) => Kind::Index,
            _ => Kind::Other,
        },
    }
}

/// The digest of a declaration's fields that enter builds (its description's `keyed`, then its own).
fn keyed(desc: &str, d: &Declaration) -> String {
    store::naming::hash16(format!("{desc}\n{}", serde_json::to_string(d).unwrap_or_default()).as_bytes())
}

/// The sample of a list's positions whose headers are read: the first, the last and seven between.
fn sample_of(n: usize) -> Vec<usize> {
    let mut v: BTreeSet<usize> = (0..=8).map(|k| (k * n.saturating_sub(1)) / 8).collect();
    v.retain(|&i| i < n);
    v.into_iter().collect()
}

impl Elevation {
    /// What dataset `id` declared as `d` has, read from its data (memoized by `memo_key`).
    fn measure(&self, id: &str, d: &Declaration, territory: &[String], v: &Version, memo_key: &str) -> std::result::Result<Measured, String> {
        if let Some(m) = self.memo.lock().unwrap().get(memo_key) {
            return m.clone();
        }
        let m = self.measure_now(id, d, territory, v);
        self.memo.lock().unwrap().insert(memo_key.to_string(), m.clone());
        m
    }

    fn measure_now(&self, id: &str, d: &Declaration, territory: &[String], v: &Version) -> std::result::Result<Measured, String> {
        let world = territory == ["world"];
        match &d.data {
            Data::Tiles { zoom, .. } => {
                if world {
                    return Ok(Measured { crs: None, resolution: crate::dem::generic::tile_resolution(*zoom, 0.0), cover: None });
                }
                if territory.is_empty() {
                    return Err("a tile pyramid needs its `territory` (its coverage can't be read from a header)".into());
                }
                let o = self.outlines.as_ref().ok_or("the pass's outlines can't be read now, and a tile pyramid's coverage is its territory's")?;
                let mut boxes = Vec::new();
                // (Each of its polygons' boxes: an island group's outline's own box would hold its
                // neighbours.)
                for r in o.recs.iter().filter(|r| o.code_of(r).is_some_and(|c| territory.contains(&c))) {
                    for poly in o.simple_polygons(r).map_err(|e| format!("the pass's outlines: {e:#}"))? {
                        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
                        for p in poly.iter().flatten() {
                            let (x, y) = (p[0] as f64 * 1e-7, p[1] as f64 * 1e-7);
                            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
                        }
                        if b[0] <= b[2] {
                            boxes.push(b);
                        }
                    }
                }
                if boxes.is_empty() {
                    return Err(format!("the pass's outlines have none of {}", territory.join(", ")));
                }
                let cells = declared::geographic_cells(&boxes);
                let mid = cells.iter().map(|c| c.1 as f64 + 0.5).sum::<f64>() / cells.len() as f64;
                Ok(Measured { crs: None, resolution: crate::dem::generic::tile_resolution(*zoom, mid), cover: Some(declared::cells_to_boxes(&cells)) })
            }
            Data::Geotiff { urls, index, files, read_resolution, .. } => {
                let fetch = self.fetch.as_ref().ok_or("nothing reads the data here")?;
                // (Where each file is, and its footprint when the index gives one.)
                let mut list: Vec<(String, Option<[f64; 4]>)> = urls.iter().map(|u| (u.clone(), None)).collect();
                let mut index_crs = None;
                if let Some(i) = index {
                    let b = (v.read)(i).map_err(|e| format!("{i} can't be read: {e:#}"))?;
                    let ix = declared::parse_index(&b).map_err(|e| format!("{i} {e}"))?;
                    index_crs = Some(ix.crs);
                    for (w, b) in ix.files {
                        list.push((
                            match w {
                                declared::Where::Url(u) => u,
                                declared::Where::File(f) => format!("{id}/{f}"),
                            },
                            Some(b),
                        ));
                    }
                }
                for p in &v.paths {
                    if let Some(rest) = p.strip_prefix(&format!("{id}/")) {
                        if files.iter().any(|g| declared::glob(g, rest)) {
                            list.push((p.clone(), None));
                        }
                    }
                }
                if list.is_empty() {
                    return Err("names no file there is".into());
                }
                let mut crs = None;
                let mut resolution: f64 = 0.0;
                let mut bounds: Vec<[f64; 4]> = Vec::new();
                for k in sample_of(list.len()) {
                    let (w, _) = &list[k];
                    let src: Arc<dyn store::range::RangeRead> = if w.starts_with("http") {
                        crate::dem::generic::open_url(&**fetch, w).map_err(|e| format!("{w}: {e:#}"))?.ok_or_else(|| format!("{w}: the server has no such file"))?
                    } else {
                        Arc::new((v.read)(w).map_err(|e| format!("{w}: {e:#}"))?.as_ref().clone())
                    };
                    let t = crate::geotiff::Tiff::open(src).map_err(|e| format!("{w}: its header can't be read ({e:#})"))?;
                    let keys = t.geo_keys();
                    let c = keys.short(crate::geotiff::key::PROJECTED_CS_TYPE).or_else(|| keys.short(crate::geotiff::key::GEOGRAPHIC_TYPE)).ok_or_else(|| format!("{w}: names no EPSG CRS"))?;
                    if !declared::CRSS.contains(&c) {
                        return Err(format!("{w}: EPSG:{c} isn't a CRS the app can project"));
                    }
                    if index_crs.is_some_and(|ic| ic != c && !(ic == 4326 && c == 4269)) {
                        return Err(format!("{w} is in EPSG:{c}, its index's footprints in EPSG:{}", index_crs.unwrap()));
                    }
                    if crs.is_some_and(|x| x != c) {
                        return Err(format!("{w} is in EPSG:{c}, the dataset's other files in EPSG:{}", crs.unwrap()));
                    }
                    crs = Some(c);
                    let level = crate::dem::generic::level_for(&t, c, *read_resolution).map_err(|e| format!("{w}: {e:#}"))?;
                    resolution = resolution.max(crate::dem::generic::resolution(&t, c, level).map_err(|e| format!("{w}: {e:#}"))?);
                    let gt = t.transform(0).map_err(|e| format!("{w}: {e:#}"))?;
                    let img = t.level(0).map_err(|e| format!("{w}: {e:#}"))?;
                    let (x1, y1) = (gt[0] + img.width as f64 * gt[1], gt[3] + img.height as f64 * gt[5]);
                    bounds.push([gt[0].min(x1), gt[3].min(y1), gt[0].max(x1), gt[3].max(y1)]);
                }
                let crs = crs.unwrap();
                // (Its footprints: the index's, or the files' own bounds when there's no index.)
                let boxes: Vec<[f64; 4]> = if list.iter().all(|(_, b)| b.is_some()) { list.iter().filter_map(|(_, b)| *b).collect() } else { bounds };
                let cover = if world {
                    None
                } else {
                    let cells = if crs == 3979 { declared::projected_cells(crs, &boxes).map_err(|e| format!("{e:#}"))? } else { declared::geographic_cells(&boxes) };
                    Some(declared::cells_to_boxes(&cells))
                };
                Ok(Measured { crs: Some(crs), resolution, cover })
            }
        }
    }

    /// The declarations of version `v`: (path, id, description's keyed, territory, declaration).
    fn declarations(v: &Version) -> Result<Vec<(String, String, String, Vec<String>, Declaration)>> {
        let mut out = Vec::new();
        for p in &v.paths {
            let Kind::Declaration(id) = kind(p) else { continue };
            let b = (v.read)(p)?;
            let (Some(desc), _, dk) = super::credits::parse_description(p, &b) else { continue };
            let Ok(d) = declared::parse(&b) else { continue };
            out.push((p.clone(), id.to_string(), dk, desc.territory, d));
        }
        Ok(out)
    }

    fn memo_key(v: &Version, dk: &str, d: &Declaration) -> String {
        let ix = match &d.data {
            Data::Geotiff { index: Some(i), .. } => (v.read)(i).map(|b| store::naming::hash16(&b)).unwrap_or_default(),
            _ => String::new(),
        };
        format!("{} {ix}", keyed(dk, d))
    }
}

impl Checks for Elevation {
    fn unit(&self) -> &'static str {
        UNIT
    }

    fn version(&self) -> u32 {
        1
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
        match kind(path) {
            Kind::Declaration(_) => {
                let (desc, findings, dk) = super::credits::parse_description(path, bytes);
                c.findings = findings.into_iter().map(|mut f| {
                    // (Its own check's name: the description's shape.)
                    f.id = f.id.replacen("description.", &format!("{DESC}."), 1);
                    f
                }).collect();
                match declared::parse(bytes) {
                    Ok(d) => {
                        if let Data::Geotiff { index: Some(i), .. } = &d.data {
                            c.refers.push(i.clone());
                        }
                        if let Some(desc) = desc {
                            c.facts.insert("description".into(), serde_json::to_value(&desc).unwrap_or_default());
                            c.facts.insert("declaration".into(), serde_json::to_value(&d).unwrap_or_default());
                            c.keyed = Some(keyed(&dk, &d));
                        }
                    }
                    Err(why) => c.findings.push(err(DECL, &why)),
                }
            }
            Kind::Index => match declared::parse_index(bytes) {
                Ok(ix) => {
                    c.facts.insert("files".into(), ix.files.len().into());
                    c.facts.insert("crs".into(), ix.crs.into());
                }
                Err(e) => c.findings.push(err(INDEX, &[e])),
            },
            Kind::Data(_) => {}
            Kind::Other => c.findings.push(Finding::new(SHAPE, Level::Error, &[path], &["not the drop box's shape"], vec![path.into()], format!("{path} isn't the drop box's shape: elevation takes <dataset>.toml, an index <name>.geojson beside it, and a dataset's files in <dataset>/ (docs/inputs.md §6.8); notes go in how/"))),
        }
        c
    }

    fn whole(&self, v: &Version) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        let decls = Self::declarations(v)?;
        let ids: BTreeSet<&str> = decls.iter().map(|d| d.1.as_str()).collect();
        let mut named: BTreeSet<String> = BTreeSet::new();
        let mut measured: Vec<(String, Declaration, Measured)> = Vec::new();
        for (path, id, dk, territory, d) in &decls {
            if let Data::Geotiff { index, files, .. } = &d.data {
                if let Some(i) = index {
                    named.insert(i.clone());
                    if !v.paths.contains(i) {
                        out.push(Finding::new(MISSING, Level::Error, &[path], &[i], vec![path.clone()], format!("{path} names {i}, which isn't there")));
                        continue;
                    }
                }
                for g in files {
                    if !v.paths.iter().any(|p| p.strip_prefix(&format!("{id}/")).is_some_and(|r| declared::glob(g, r))) {
                        out.push(Finding::new(MISSING, Level::Error, &[path], &[g], vec![path.clone()], format!("{path}: no file in {id}/ is {g}")));
                    }
                }
            }
            if self.fetch.is_none() {
                continue;
            }
            match self.measure(id, d, territory, v, &Self::memo_key(v, dk, d)) {
                Ok(m) => measured.push((id.clone(), d.clone(), m)),
                Err(why) => {
                    let mut files = vec![path.clone()];
                    if let Data::Geotiff { index: Some(i), .. } = &d.data {
                        files.push(i.clone());
                    }
                    out.push(Finding::new(DATA, Level::Error, &[path], &[&why], files, format!("{id}: {why}")));
                }
            }
        }
        for p in &v.paths {
            let orphan = match kind(p) {
                Kind::Data(id) => (!ids.contains(id)).then(|| format!("{p}: no declaration names its dataset ({id}.toml)")),
                Kind::Index => (!named.contains(p)).then(|| format!("{p}: no declaration names it as its index")),
                _ => None,
            };
            if let Some(m) = orphan {
                out.push(Finding::new(ORPHAN, Level::Warning, &[p], &[], vec![p.clone()], m));
            }
        }
        // Two datasets of one rank and resolution meeting: their order would fall to their ids.
        for (i, (a, da, ma)) in measured.iter().enumerate() {
            for (b, db, mb) in &measured[i + 1..] {
                let same = da.rank.unwrap_or(0) == db.rank.unwrap_or(0) && (ma.resolution - mb.resolution).abs() < 0.5 && da.surface == db.surface;
                let (ca, cb) = (declared::Cover::of(ma.cover.as_deref()), declared::Cover::of(mb.cover.as_deref()));
                let meet = ca.world || cb.world || ca.cells.iter().any(|c| cb.cells.contains(c));
                if same && meet {
                    let (pa, pb) = (format!("{a}.toml"), format!("{b}.toml"));
                    out.push(Finding::new(TIE, Level::Warning, &[&pa, &pb], &[&format!("{:.1}", ma.resolution)], vec![pa.clone(), pb.clone()], format!("{a} and {b} meet, of one rank and resolution ({:.1} m): {a} is read first, by its id; give one a `rank`", ma.resolution)));
                }
            }
        }
        Ok(out)
    }

    fn derive(&self, v: &Version, keep: &BTreeSet<String>) -> Result<Derived> {
        let mut d = Derived::default();
        if self.fetch.is_none() {
            return Ok(d);
        }
        for (path, id, dk, territory, decl) in Self::declarations(v)? {
            if keep.contains(&path) {
                continue;
            }
            let Ok(m) = self.measure(&id, &decl, &territory, v, &Self::memo_key(v, &dk, &decl)) else { continue };
            let mut facts = serde_json::Map::new();
            // (The credits' extent: where it has data.)
            let extent = match &m.cover {
                None => serde_json::json!("world"),
                Some(b) => serde_json::json!(b),
            };
            facts.insert("extent".into(), extent);
            facts.insert("measured".into(), serde_json::to_value(&m)?);
            d.facts.insert(path, (facts, None));
        }
        Ok(d)
    }

    fn switch(&self, root: &Path, index: &str) -> Result<Option<Vec<u8>>> {
        let p = root.join(PROOF);
        let b = match std::fs::read(&p) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(anyhow::Error::from(e).context(format!("read {PROOF}"))),
        };
        let v: serde_json::Value = serde_json::from_slice(&b).map_err(|e| anyhow::anyhow!("parse {PROOF}: {e}"))?;
        Ok((v["index"].as_str() == Some(index) && v["ok"] == true && v["app"].as_str().is_some_and(|a| !a.is_empty())).then_some(b))
    }

    fn context(&self, manifest: &BTreeMap<String, String>) -> Vec<String> {
        // (The newest pass's outlines: a tile pyramid's coverage is its territory's.)
        outlines_of(manifest).map(|o| format!("outlines {o}")).into_iter().collect()
    }

    fn prepared(&self, root: &Path, manifest: &BTreeMap<String, String>) -> Result<Option<Box<dyn Checks>>> {
        let outlines = match outlines_of(manifest) {
            Some(o) => match Outlines::open(&root.join(o)) {
                Ok(x) => Some(x),
                Err(e) => {
                    eprintln!("inputs: {UNIT}: the pass's outlines, {o}, can't be read ({e:#})");
                    None
                }
            },
            None => None,
        };
        Ok(Some(Box::new(Elevation::new(Arc::new(crate::fetch::Fetcher::from_env()), outlines))))
    }
}

/// The content name of the newest pass's outlines the records `manifest` name.
pub fn outlines_of(manifest: &BTreeMap<String, String>) -> Option<&String> {
    let date = crate::osmpass::latest_pass_in(manifest)?;
    manifest.get(&format!("sources/osm/{date}/outlines"))
}

/// Whether the records say the build reads the declared datasets (`@switched`).
pub fn switched(manifest: &BTreeMap<String, String>) -> bool {
    manifest.contains_key(&super::switched_logical(UNIT))
}

#[cfg(test)]
#[path = "elevation_tests.rs"]
mod tests;
