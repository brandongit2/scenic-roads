//! The elevation datasets' drop box, `inputs/elevation/` (docs/inputs.md §6.8, #143): a declaration
//! per dataset (`<dataset>.toml`: a description, §5.1, and crate::dem::declared's fields), a tile
//! index beside it when it names one (`<name>.geojson`), and the files of a dataset dropped whole in
//! `<dataset>/`.
//!
//! Checked: each declaration's shape (its description's, and its own: surface, height reference,
//! code, data), each index's; then, over the version, what they name being there, and the data
//! itself read where it lies (`measure`: each dataset's GeoTIFF headers by range requests, a sample
//! of an index's and every one of the rest: their CRS and resolution; the coverage from an index's
//! footprints, else from where each file has valid data at its coarsest overview; a tile pyramid's
//! coverage from its territory's outlines, and one of its tiles read for its size). Errors: a shape broken, a height reference or encoding the app doesn't know, a CRS
//! the app can't project, a file named that isn't there, data that can't be read. Warnings: a
//! dataset's folder or an index no declaration names; two datasets of one rank and resolution
//! meeting (their order would fall to their ids).
//!
//! What's measured goes in the index with the declaration (`facts.measured`, `facts.extent`), so the
//! unit jobs read the order and coverage from the index alone (crate::dem::declared::plan). The
//! units are built only once the records say the readers switched (`@switched`, recorded by the
//! check as soon as a version is accepted, naming the published app that recorded it: docs/inputs.md §6.8).

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
    /// The app the switch records; None: this program's (`agent::app_version`).
    pub app: Option<String>,
    /// The stores' folder (`sources/fetched/`): a dataset declared `store` is measured from its
    /// stored files there (crate::dem::generic::stored), never fetched.
    pub store: Option<std::path::PathBuf>,
}

impl Elevation {
    pub fn new(fetch: Arc<dyn Fetch>, outlines: Option<Outlines>) -> Elevation {
        Elevation { fetch: Some(fetch), outlines, memo: Mutex::default(), app: None, store: None }
    }
}

#[cfg(test)]
thread_local! {
    /// The check job's reader and app in tests (`prepared`): no network, and a published app's version.
    pub static TEST_PREPARED: std::cell::RefCell<Option<(Arc<dyn Fetch>, String)>> = const { std::cell::RefCell::new(None) };
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

/// The digest of what a declaration's data is beyond the declaration itself: its index's content and
/// its dropped files' names and contents ("" for none). Part of its `keyed` (`derive`): adding a
/// tile to an index, or a file to its folder, reruns the units it meets.
fn data_digest(v: &Version, id: &str, d: &Declaration) -> String {
    let Data::Geotiff { index, files, .. } = &d.data else { return String::new() };
    let mut lines = Vec::new();
    if let Some(i) = index {
        lines.push(format!("index {}", (v.read)(i).map(|b| store::naming::hash16(&b)).unwrap_or_default()));
    }
    for p in &v.paths {
        if let Some(rest) = p.strip_prefix(&format!("{id}/")) {
            if files.iter().any(|g| declared::glob(g, rest)) {
                lines.push(format!(
                    "file {rest} {}",
                    (v.read)(p).map(|b| store::naming::hash16(&b)).unwrap_or_default()
                ));
            }
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    store::naming::hash16(lines.join("\n").as_bytes())
}

/// A declaration's `keyed` with its data's digest (`data_digest`): what the units' keys name.
fn keyed_with_data(declared: &str, data: &str) -> String {
    if data.is_empty() {
        declared.to_string()
    } else {
        store::naming::hash16(format!("{declared} {data}").as_bytes())
    }
}

/// Files whose valid data is read at their coarsest overview for the coverage: at most this many
/// pixels there (a bigger one's coverage is its bounds).
const MASK_MAX_PX: usize = 16 << 20;

/// The sample of an index's positions whose headers are read: the first, the last and seven
/// between. (Files without an index's footprint are each read: their coverage is their own.)
fn sample_of(n: usize) -> Vec<usize> {
    let mut v: BTreeSet<usize> = (0..=8).map(|k| (k * n.saturating_sub(1)) / 8).collect();
    v.retain(|&i| i < n);
    v.into_iter().collect()
}

impl Elevation {
    /// What dataset `id` declared as `d` has, read from its data (memoized by `memo_key`).
    fn measure(
        &self,
        id: &str,
        d: &Declaration,
        territory: &[String],
        v: &Version,
        memo_key: &str,
    ) -> std::result::Result<Measured, String> {
        if let Some(m) = self.memo.lock().unwrap().get(memo_key) {
            return m.clone();
        }
        let m = self.measure_now(id, d, territory, v);
        self.memo.lock().unwrap().insert(memo_key.to_string(), m.clone());
        m
    }

    fn measure_now(
        &self,
        id: &str,
        d: &Declaration,
        territory: &[String],
        v: &Version,
    ) -> std::result::Result<Measured, String> {
        let world = territory == ["world"];
        match &d.data {
            Data::Tiles { zoom, url, encoding } => {
                if world {
                    self.tile_size(url, *zoom, encoding, &[[-1.0, -1.0, 1.0, 1.0]])?;
                    return Ok(Measured {
                        crs: None,
                        resolution: crate::dem::generic::tile_resolution(*zoom, 0.0),
                        cover: None,
                    });
                }
                if territory.is_empty() {
                    return Err(
                        "a tile pyramid needs its `territory` (its coverage can't be read from a header)".into()
                    );
                }
                let o = self
                    .outlines
                    .as_ref()
                    .ok_or("the pass's outlines can't be read now, and a tile pyramid's coverage is its territory's")?;
                let boxes = territory_boxes(o, territory)?;
                self.tile_size(url, *zoom, encoding, &boxes)?;
                let cells = declared::geographic_cells(&boxes);
                let mid = cells.iter().map(|c| c.1 as f64 + 0.5).sum::<f64>() / cells.len() as f64;
                Ok(Measured {
                    crs: None,
                    resolution: crate::dem::generic::tile_resolution(*zoom, mid),
                    cover: Some(declared::cells_to_boxes(&cells)),
                })
            }
            Data::Geotiff { urls, index, files, read_resolution, store } => {
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
                // (The index's files sampled; every other file read, its coverage its own.)
                let mut picks: BTreeSet<usize> =
                    sample_of(list.len()).into_iter().filter(|&k| list[k].1.is_some()).collect();
                picks.extend((0..list.len()).filter(|&k| list[k].1.is_none()));
                let mut masks: Vec<Mask> = Vec::new();
                for k in picks {
                    let (w, footprint) = &list[k];
                    let src: Arc<dyn store::range::RangeRead> = if w.starts_with("http") && *store {
                        let dir = self
                            .store
                            .as_ref()
                            .ok_or("its files are read from the store, and no store is given here")?
                            .join(id);
                        crate::dem::generic::stored(id, &dir, w)
                            .map_err(|e| format!("{e:#}"))?
                            .ok_or_else(|| format!("{w}: the store notes the source has no such file"))?
                    } else if w.starts_with("http") {
                        crate::dem::generic::open_url(&**fetch, w)
                            .map_err(|e| format!("{w}: {e:#}"))?
                            .ok_or_else(|| format!("{w}: the server has no such file"))?
                    } else {
                        Arc::new((v.read)(w).map_err(|e| format!("{w}: {e:#}"))?.as_ref().clone())
                    };
                    let t = crate::geotiff::Tiff::open(src)
                        .map_err(|e| format!("{w}: its header can't be read ({e:#})"))?;
                    let keys = t.geo_keys();
                    let c = keys
                        .short(crate::geotiff::key::PROJECTED_CS_TYPE)
                        .or_else(|| keys.short(crate::geotiff::key::GEOGRAPHIC_TYPE))
                        .ok_or_else(|| format!("{w}: names no EPSG CRS"))?;
                    if !declared::CRSS.contains(&c) {
                        return Err(format!("{w}: EPSG:{c} isn't a CRS the app can project"));
                    }
                    if index_crs.is_some_and(|ic| ic != c && !(ic == 4326 && c == 4269)) {
                        return Err(format!(
                            "{w} is in EPSG:{c}, its index's footprints in EPSG:{}",
                            index_crs.unwrap()
                        ));
                    }
                    if crs.is_some_and(|x| x != c) {
                        return Err(format!("{w} is in EPSG:{c}, the dataset's other files in EPSG:{}", crs.unwrap()));
                    }
                    crs = Some(c);
                    let level =
                        crate::dem::generic::level_for(&t, c, *read_resolution).map_err(|e| format!("{w}: {e:#}"))?;
                    resolution = resolution
                        .max(crate::dem::generic::resolution(&t, c, level).map_err(|e| format!("{w}: {e:#}"))?);
                    if footprint.is_none() {
                        masks.push(valid_data(&t).map_err(|e| format!("{w}: {e:#}"))?);
                    }
                }
                let crs = crs.unwrap();
                // (Its footprints: the index's, and where the other files have valid data.)
                let boxes: Vec<[f64; 4]> = list.iter().filter_map(|(_, b)| *b).collect();
                let cover = if world {
                    None
                } else {
                    let cells = if crs == 3979 {
                        declared::projected_cells_where(crs, &|x, y| {
                            boxes.iter().any(|b| x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3])
                                || masks.iter().any(|m| m.holds(x, y))
                        })
                        .map_err(|e| format!("{e:#}"))?
                    } else {
                        let mut c = declared::geographic_cells(&boxes);
                        for m in &masks {
                            c.extend(declared::geographic_cells(&m.boxes()));
                        }
                        c
                    };
                    if cells.is_empty() {
                        return Err("has no valid data anywhere".into());
                    }
                    Some(declared::cells_to_boxes(&cells))
                };
                Ok(Measured { crs: Some(crs), resolution, cover })
            }
        }
    }

    /// Whether a tile pyramid's tiles are of the size the sampler reads (`TILE_PX`): the tile at the
    /// middle of the first of `boxes` (w, s, e, n) the server has (at most four asked for; none
    /// found passes: the sampler refuses one of another size too).
    fn tile_size(&self, url: &str, zoom: u32, encoding: &str, boxes: &[[f64; 4]]) -> std::result::Result<(), String> {
        let fetch = self.fetch.as_ref().ok_or("nothing reads the data here")?;
        for b in boxes.iter().take(4) {
            let (fx, fy) =
                crate::dem::proj::mercator_tile((b[0] + b[2]) / 2.0, ((b[1] + b[3]) / 2.0).clamp(-85.0, 85.0), zoom);
            let u = url
                .replace("{z}", &zoom.to_string())
                .replace("{x}", &(fx.floor() as i64).to_string())
                .replace("{y}", &(fy.floor() as i64).to_string());
            let Some(png) = fetch.get(&u).map_err(|e| format!("{u}: {e:#}"))? else { continue };
            let (w, h, _) = crate::dem::encoding::decode(encoding, &png)
                .map_err(|e| format!("{u}: not a PNG the app reads ({e:#})"))?;
            let px = crate::dem::generic::TILE_PX;
            if (w, h) != (px, px) {
                return Err(format!("{u} is a {w} × {h} tile: tile pyramids are read as {px} × {px}"));
            }
            return Ok(());
        }
        Ok(())
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

    fn memo_key(v: &Version, id: &str, dk: &str, d: &Declaration) -> String {
        format!("{id} {} {}", keyed(dk, d), data_digest(v, id, d))
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
                        out.push(Finding::new(
                            MISSING,
                            Level::Error,
                            &[path],
                            &[i],
                            vec![path.clone()],
                            format!("{path} names {i}, which isn't there"),
                        ));
                        continue;
                    }
                }
                for g in files {
                    if !v.paths.iter().any(|p| p.strip_prefix(&format!("{id}/")).is_some_and(|r| declared::glob(g, r)))
                    {
                        out.push(Finding::new(
                            MISSING,
                            Level::Error,
                            &[path],
                            &[g],
                            vec![path.clone()],
                            format!("{path}: no file in {id}/ is {g}"),
                        ));
                    }
                }
            }
            if self.fetch.is_none() {
                continue;
            }
            match self.measure(id, d, territory, v, &Self::memo_key(v, id, dk, d)) {
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
        // Datasets of one code labelled apart: the map's list would name them by whichever comes first.
        let mut labels: BTreeMap<u8, (String, String)> = BTreeMap::new();
        for p in &v.paths {
            let Kind::Declaration(_) = kind(p) else { continue };
            let Ok(b) = (v.read)(p) else { continue };
            let (Some(desc), _, _) = super::credits::parse_description(p, &b) else { continue };
            let Ok(d) = declared::parse(&b) else { continue };
            if d.code == 0 {
                continue;
            }
            let label = if desc.label.is_empty() { desc.name } else { desc.label };
            match labels.get(&d.code) {
                Some((q, l)) if *l != label => out.push(Finding::new(
                    DECL,
                    Level::Error,
                    &[p],
                    &["label"],
                    vec![q.clone(), p.clone()],
                    format!(
                        "{p} and {q} share code {} but are labelled apart (\"{label}\", \"{l}\"): one label per code",
                        d.code
                    ),
                )),
                Some(_) => {}
                None => {
                    labels.insert(d.code, (p.clone(), label));
                }
            }
        }
        for p in &v.paths {
            let orphan = match kind(p) {
                Kind::Data(id) => {
                    (!ids.contains(id)).then(|| format!("{p}: no declaration names its dataset ({id}.toml)"))
                }
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
                let same = da.rank.unwrap_or(0) == db.rank.unwrap_or(0)
                    && (ma.resolution - mb.resolution).abs() < 0.5
                    && da.surface == db.surface
                    && da.terrain == db.terrain;
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

    fn switch(&self, _root: &Path, index: &str) -> Result<Option<Vec<u8>>> {
        // (As soon as a version is accepted: the version and the app that recorded it, which bars
        // older apps from leading: regions::read::switch_app. Not by an app that isn't published
        // (a development build): it would bar none, for good; the next published app's check
        // records it.)
        let app = match &self.app {
            Some(a) => a.clone(),
            None => crate::agent::app_version(
                std::env::current_exe()?.parent().ok_or_else(|| anyhow::anyhow!("the program's folder"))?,
            ),
        };
        if !crate::pool::term::is_version(&app) {
            return Ok(None);
        }
        Ok(Some(serde_json::to_vec_pretty(&serde_json::json!({ "index": index, "app": app }))?))
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
            let Ok(m) = self.measure(&id, &decl, &territory, v, &Self::memo_key(v, &id, &dk, &decl)) else { continue };
            let mut facts = serde_json::Map::new();
            // (The credits' extent: where it has data.)
            let extent = match &m.cover {
                None => serde_json::json!("world"),
                Some(b) => serde_json::json!(b),
            };
            facts.insert("extent".into(), extent);
            facts.insert("measured".into(), serde_json::to_value(&m)?);
            // (Its `keyed` with its data's: an index's tiles or a folder's files added rerun units.)
            d.facts.insert(path, (facts, Some(keyed_with_data(&keyed(&dk, &decl), &data_digest(v, &id, &decl)))));
        }
        Ok(d)
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
        #[cfg(test)]
        if let Some((fetch, app)) = TEST_PREPARED.with(|t| t.borrow().clone()) {
            return Ok(Some(Box::new(Elevation {
                app: Some(app),
                store: Some(root.join("sources/fetched")),
                ..Elevation::new(fetch, outlines)
            })));
        }
        Ok(Some(Box::new(Elevation {
            store: Some(root.join("sources/fetched")),
            ..Elevation::new(Arc::new(crate::fetch::Fetcher::from_env()), outlines)
        })))
    }
}

/// The boxes of territory `territory`'s polygons (ISO codes) in the pass's outlines `o`: each
/// polygon's own box (an island group's outline's own box would hold its neighbours). An error when
/// the outlines have none of them.
pub fn territory_boxes(o: &Outlines, territory: &[String]) -> std::result::Result<Vec<[f64; 4]>, String> {
    let mut boxes = Vec::new();
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
    Ok(boxes)
}

/// Where a GeoTIFF has valid data: its coarsest overview's pixels that hold data (`valid` by row
/// and column), in its CRS by its transform.
struct Mask {
    gt: [f64; 6],
    w: usize,
    h: usize,
    valid: Vec<bool>,
}

impl Mask {
    /// Whether point x, y (in the file's CRS) is in a pixel with data.
    fn holds(&self, x: f64, y: f64) -> bool {
        let (c, r) = ((x - self.gt[0]) / self.gt[1], (y - self.gt[3]) / self.gt[5]);
        c >= 0.0
            && r >= 0.0
            && (c as usize) < self.w
            && (r as usize) < self.h
            && self.valid[r as usize * self.w + c as usize]
    }

    /// A box (w, s, e, n in its CRS) per run of pixels with data along a row.
    fn boxes(&self) -> Vec<[f64; 4]> {
        let gt = self.gt;
        let bounds = |x0: f64, y0: f64, x1: f64, y1: f64| -> [f64; 4] {
            let (ax, ay) = (gt[0] + x0 * gt[1], gt[3] + y0 * gt[5]);
            let (bx, by) = (gt[0] + x1 * gt[1], gt[3] + y1 * gt[5]);
            [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
        };
        let mut out = Vec::new();
        for r in 0..self.h {
            let mut c = 0;
            while c < self.w {
                if !self.valid[r * self.w + c] {
                    c += 1;
                    continue;
                }
                let c0 = c;
                while c < self.w && self.valid[r * self.w + c] {
                    c += 1;
                }
                out.push(bounds(c0 as f64, r as f64, c as f64, r as f64 + 1.0));
            }
        }
        out
    }
}

/// Where a GeoTIFF has valid data, read at its coarsest overview; one with no overview small enough
/// (`MASK_MAX_PX`) taken as all data within its bounds.
fn valid_data(t: &crate::geotiff::Tiff) -> Result<Mask> {
    let level = t.images().len() - 1;
    let img = t.level(level)?.clone();
    let gt = t.transform(level)?;
    let (w, h) = (img.width as usize, img.height as usize);
    if w * h > MASK_MAX_PX {
        let g0 = t.transform(0)?;
        let i0 = t.level(0)?;
        return Ok(Mask {
            gt: [g0[0], g0[1] * i0.width as f64, 0.0, g0[3], 0.0, g0[5] * i0.height as f64],
            w: 1,
            h: 1,
            valid: vec![true],
        });
    }
    let a = t.read_window(level, 0, 0, img.width, img.height)?;
    let nodata = t.nodata();
    Ok(Mask { gt, w, h, valid: a.iter().map(|&v| crate::dem::generic::valid(v, nodata)).collect() })
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

/// An accepted version of one dataset covering the world, its index named in records `m` and the
/// switch recorded (the units' tests: they're built only from the declared datasets).
#[cfg(test)]
pub fn accept_fixture(root: &Path, m: &mut BTreeMap<String, String>) {
    let d = Declaration {
        surface: "ground".into(),
        heights: "EGM2008".into(),
        rank: None,
        code: 4,
        terrain: false,
        data: Data::Tiles { url: "https://e.org/{z}/{x}/{y}.png".into(), zoom: 10, encoding: "terrarium".into() },
    };
    let measured = Measured { crs: None, resolution: 150.0, cover: None };
    let entry = |id: &str, d: &Declaration, measured: Measured, keyed: &str| {
        let mut facts = serde_json::Map::new();
        facts.insert("declaration".into(), serde_json::to_value(d).unwrap());
        facts.insert("measured".into(), serde_json::to_value(&measured).unwrap());
        (
            format!("{id}.toml"),
            super::FileEntry {
                file: format!("{}/{id}.0000000000000000.toml", super::store_dir(UNIT)),
                size: 0,
                keyed: keyed.into(),
                facts,
            },
        )
    };
    let mut files = BTreeMap::from([entry("world", &d, measured, "w")]);
    // (The terrain's: its base and its northern dataset, by an index of two 1° tiles, both where the
    // code fetched them before they were declared, so its records from then are re-keyed.)
    let base = Declaration {
        surface: "surface".into(),
        heights: "mixed".into(),
        rank: None,
        code: 0,
        terrain: true,
        data: Data::Tiles {
            url: "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png".into(),
            zoom: 12,
            encoding: "terrarium".into(),
        },
    };
    files.extend([entry("terrarium", &base, Measured { crs: None, resolution: 38.0, cover: None }, "t")]);
    let north = Declaration {
        surface: "surface".into(),
        heights: "EGM2008".into(),
        rank: None,
        code: 0,
        terrain: true,
        data: Data::Geotiff {
            urls: vec![],
            index: Some("glo30.geojson".into()),
            files: vec![],
            read_resolution: None,
            store: false,
        },
    };
    files.extend([entry("glo30", &north, Measured { crs: Some(4326), resolution: 30.0, cover: None }, "g")]);
    let feature = |la: i32, lo: i32| serde_json::json!({ "type": "Feature", "properties": { "url": format!("https://copernicus-dem-30m.s3.amazonaws.com/N{la}_W{}_DEM.tif", -lo), "mask": format!("https://copernicus-dem-30m.s3.amazonaws.com/N{la}_W{}_FLM.tif", -lo) }, "geometry": { "type": "Polygon", "coordinates": [[[lo, la], [lo + 1, la], [lo + 1, la + 1], [lo, la + 1], [lo, la]]] } });
    let index = serde_json::to_vec(
        &serde_json::json!({ "type": "FeatureCollection", "features": [feature(64, -148), feature(65, -148)] }),
    )
    .unwrap();
    let ifile = format!("{}/glo30.0000000000000000.geojson", super::store_dir(UNIT));
    std::fs::create_dir_all(root.join(super::store_dir(UNIT))).unwrap();
    std::fs::write(root.join(&ifile), &index).unwrap();
    files.insert(
        "glo30.geojson".into(),
        super::FileEntry { file: ifile, size: index.len() as u64, keyed: "i".into(), facts: Default::default() },
    );
    let index =
        super::Index { fmt: 1, unit: UNIT.into(), checks: format!("{UNIT} 1"), files, accepted: BTreeMap::new() };
    let body = serde_json::to_vec(&index).unwrap();
    let name = format!("{}/@index.{}.json", super::store_dir(UNIT), store::naming::hash16(&body));
    std::fs::create_dir_all(root.join(super::store_dir(UNIT))).unwrap();
    std::fs::write(root.join(&name), body).unwrap();
    m.insert(super::logical(UNIT), name);
    // (Its switch, recorded on an app every app passes.)
    let sw = format!("{}/@switched.1111111111111111.json", super::store_dir(UNIT));
    std::fs::write(root.join(&sw), br#"{"ok": true, "app": "development"}"#).unwrap();
    m.insert(super::switched_logical(UNIT), sw);
}

#[cfg(test)]
#[path = "elevation_tests.rs"]
mod tests;
