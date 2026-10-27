//! Regions and the areas they're made of (docs/surface.md, the Regions panel; docs/regions.md).
//!
//!   GET    /api/regions             the region files in inputs/regions on the NAS, in every folder,
//!                                   each with its path and its state in the input check (accepted,
//!                                   checking, held with the findings), the files that don't parse,
//!                                   and the folders' tree (docs/inputs.md §6.1.9); away from the
//!                                   NAS, the last list read
//!   GET    /api/regions/layer       every region's accepted outline for drawing (its drawn files,
//!                                   kept on this Mac), a colour from its id, held or not, and the
//!                                   12-nautical-mile line (`kind: "ts12"`); the catalogue's
//!                                   regions until this Mac has the accepted copy
//!   GET    /api/areas/{id}          one outline, simplified (a GeoJSON feature)
//!   GET    /api/coverage            the coverage the catalogue was built for: its regions' outlines,
//!                                   simplified (GeoJSON), and the regions
//!
//! Outlines come from the latest OSM pass (the catalogue's `global.outlines`); records and strings are
//! read once per catalogue, rings when an outline is drawn or tested.
//!
//! Nothing here writes a region: regions change only as files in the inputs folder, which the input
//! check takes in (docs/inputs.md §6.1). The region files are read through the NAS I/O pool like
//! every other access to the share: a mount that hangs fails the request in the pool's time (a
//! 503, as for any read the NAS can't answer) instead of holding it.

use crate::S;
use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use pipeline::coverage::DrawnRegion;
use pipeline::outlines::{flag, OutlineRec, Ring};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use store::iopool::{IoError, IoPool};

/// The outlines of the current catalogue: records and strings in memory, rings read on demand.
pub struct OutlineIndex {
    content: String,
    sect: Arc<crate::views::SectView>,
    pub recs: Vec<OutlineRec>,
    strings: Vec<String>,
}

impl OutlineIndex {
    fn open(data: &crate::data::Data) -> Result<Option<OutlineIndex>> {
        let cat = data.catalogue();
        let Some(logical) = cat.global.get("outlines") else { return Ok(None) };
        let Some(sect) = data.sect(logical)? else { return Ok(None) };
        let content = data.content(logical).unwrap_or_default();
        let recs: Vec<OutlineRec> = bytemuck::pod_collect_to_vec(sect.get("recs")?.bytes());
        let strings: Vec<String> =
            String::from_utf8_lossy(sect.get("strings")?.bytes()).split('\n').map(str::to_string).collect();
        Ok(Some(OutlineIndex { content, sect, recs, strings }))
    }

    pub fn string(&self, i: u32) -> &str {
        self.strings.get(i as usize).map(String::as_str).unwrap_or("")
    }

    /// An outline's rings (`simple`: the simplified ones), read from the file, with their kinds
    /// (0 outer, 1 inner).
    fn rings_kinds(&self, o: &OutlineRec, simple: bool) -> Result<Vec<(u32, Vec<[i32; 2]>)>> {
        let (rs, ps, first, n) =
            if simple { ("srings", "spoints", o.srings, o.nsrings) } else { ("rings", "points", o.rings, o.nrings) };
        let rb = self.sect.get_part(rs, first as u64 * 16, n as usize * 16)?;
        let rings: Vec<Ring> = bytemuck::pod_collect_to_vec(&rb);
        let (Some(a), Some(b)) = (rings.first(), rings.last()) else { return Ok(Vec::new()) };
        let (start, end) = (a.start, b.start + b.count as u64);
        let pb = self.sect.get_part(ps, start * 8, ((end - start) * 8) as usize)?;
        let pts: Vec<[i32; 2]> = bytemuck::pod_collect_to_vec(&pb);
        Ok(rings
            .iter()
            .map(|r| (r.kind, pts[(r.start - start) as usize..(r.start - start) as usize + r.count as usize].to_vec()))
            .collect())
    }

    /// An outline's rings (`simple`: the simplified ones).
    pub fn rings(&self, o: &OutlineRec, simple: bool) -> Result<Vec<Vec<[i32; 2]>>> {
        Ok(self.rings_kinds(o, simple)?.into_iter().map(|(_, r)| r).collect())
    }

    /// The simplified rings as GeoJSON polygons: an outer ring, then its holes. (Outlines from passes
    /// before ring kinds were recorded have every ring outer: each its own polygon.)
    pub fn polygons(&self, o: &OutlineRec) -> Result<Vec<Vec<Vec<[f64; 2]>>>> {
        let mut out: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
        for (kind, r) in self.rings_kinds(o, true)? {
            let ring: Vec<[f64; 2]> = r.iter().map(|p| [p[0] as f64 * 1e-7, p[1] as f64 * 1e-7]).collect();
            match out.last_mut() {
                Some(p) if kind == 1 => p.push(ring),
                _ => out.push(vec![ring]),
            }
        }
        Ok(out)
    }

    /// The country or territory an outline lies in, as its summary's `in` and `in_name`.
    fn lies_in(&self, o: &OutlineRec) -> (String, String) {
        lies_in(&self.recs, |i| self.string(i), o)
    }

    pub fn by_id(&self, id: u64) -> Option<&OutlineRec> {
        self.recs.binary_search_by_key(&id, |r| r.id).ok().map(|k| &self.recs[k])
    }

    fn summary(&self, o: &OutlineRec) -> Value {
        let (in_code, in_name) = self.lies_in(o);
        json!({
            "id": o.id,
            "name": self.string(o.name),
            "en": self.string(o.name_en),
            "level": o.level,
            "iso": self.string(o.iso),
            "country": o.flags & flag::ISO1 != 0,
            // The country it lies in (its ISO 3166-1 code, and the country's English or own name):
            // none for a country or territory itself.
            "in": in_code,
            "in_name": in_name,
            "km2": o.area_km2.round(),
            "bbox": o.bbox.map(|v| v as f64 * 1e-7),
        })
    }
}

/// The country or territory `o` lies in: its ISO 3166-1 code and its English (else own) name,
/// the name that of the outline whose own code it is (`pipeline::outlines::own_code_of`: GP's
/// outline is coded FR-971 but is Guadeloupe's), the lowest level first. Both "" for a country or
/// territory itself, and for an outline in none.
fn lies_in<'a>(recs: &[OutlineRec], string: impl Fn(u32) -> &'a str, o: &OutlineRec) -> (String, String) {
    use pipeline::outlines::{country_of_string, own_code_of};
    if o.flags & flag::ISO1 != 0 {
        return (String::new(), String::new());
    }
    let Some(code) = country_of_string(string(o.country)) else { return (String::new(), String::new()) };
    let name = recs
        .iter()
        .filter(|r| own_code_of(r.flags, string(r.iso), string(r.country)) == Some(code))
        .min_by_key(|r| (r.level, r.id))
        .map(|r| if string(r.name_en).is_empty() { string(r.name) } else { string(r.name_en) })
        .unwrap_or("");
    (code.to_string(), name.to_string())
}

/// The languages spoken where (`names::spoken`), from the catalogue's outlines (their simplified
/// rings, read from the mirror or the NAS) and `table` (None when the catalogue has no outlines).
pub fn spoken_from_outlines(data: &crate::data::Data, table: &names::Table) -> Result<Option<names::Spoken>> {
    let Some(ix) = OutlineIndex::open(data)? else { return Ok(None) };
    let polygons = |o: &OutlineRec| -> Result<Vec<Vec<Vec<[i32; 2]>>>> {
        let mut out: Vec<Vec<Vec<[i32; 2]>>> = Vec::new();
        for (kind, r) in ix.rings_kinds(o, true)? {
            match out.last_mut() {
                Some(p) if kind == 1 => p.push(r),
                _ => out.push(vec![r]),
            }
        }
        Ok(out)
    };
    let areas = pipeline::outlines::spoken_areas(table, &ix.recs, |i| ix.string(i).to_owned(), polygons)?;
    Ok(Some(names::Spoken::build(table, areas)))
}

/// The outline index of the current catalogue, opened once per catalogue.
#[derive(Default)]
pub struct Areas {
    cur: Mutex<Option<Arc<OutlineIndex>>>,
}

impl Areas {
    pub fn get(&self, s: &crate::AppState) -> Result<Option<Arc<OutlineIndex>>> {
        let cat = s.data.catalogue();
        let want = cat.global.get("outlines").and_then(|l| s.data.content(l));
        let mut g = self.cur.lock().unwrap();
        if let Some(cur) = g.as_ref() {
            if Some(&cur.content) == want.as_ref() {
                return Ok(Some(cur.clone()));
            }
        }
        let opened = OutlineIndex::open(&s.data)?.map(Arc::new);
        *g = opened.clone();
        Ok(opened)
    }

    /// Lets go of the outlines when the mirror has just evicted their file (`names`): its map would
    /// hold the disk's room. Opened again, from the NAS, when next asked for.
    pub fn forget(&self, names: &[String]) {
        let mut g = self.cur.lock().unwrap();
        if g.as_ref().is_some_and(|cur| names.contains(&cur.content)) {
            *g = None;
        }
    }
}

fn err(code: StatusCode, msg: impl std::fmt::Display) -> Response {
    (code, Json(json!({ "error": msg.to_string() }))).into_response()
}

/// The regions' inputs folder on the NAS, and the I/O pool every access to it goes through (None while the
/// NAS is away).
fn nas(s: &crate::AppState) -> Option<(store::nasfs::NasPath, Arc<IoPool>)> {
    let (root, pool) = (s.data.nas_root()?, s.data.pool()?);
    pool.is_online().then(|| (root.join("inputs/regions"), pool))
}

// ---- the region files on the NAS ----------------------------------------------------------------

/// A region file as the panel lists it (docs/inputs.md §6.1.9).
#[derive(serde::Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub outline: Vec<String>,
    /// Its path in the inputs folder (`europe/portugal/azores.toml`).
    pub path: String,
    /// accepted, checking or held ("" when the check's records can't be read now).
    #[serde(default)]
    pub state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<Value>,
    /// `disabled = true` in its file: checked like any region, never built or downloaded (the panel
    /// shows it greyed, with no Download button).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

impl Listed {
    fn of(r: &pipeline::inputs::regions::file::Region, path: String) -> Listed {
        Listed {
            id: r.id.clone(),
            name: r.name.clone(),
            outline: r.outline_text(),
            path,
            state: String::new(),
            held: Vec::new(),
            disabled: r.disabled,
        }
    }
}

/// The region files and the files that don't parse (path, problem), as the panel lists them.
type ListedAll = (Vec<Listed>, Vec<(String, String)>);

/// Every region file in the folder and its subfolders (`how/`, `todo/` and the names every inputs folder
/// passes over aside, and the special files), by id; the files that don't parse. The NAS failing midway is
/// an error, never a shorter list.
fn load_all(pool: &IoPool, dir: &store::nasfs::NasPath) -> Result<ListedAll> {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    let mut stack = vec![(dir.clone(), String::new())];
    while let Some((d, rel)) = stack.pop() {
        let items = match pool.list(&d) {
            Ok(items) => items,
            Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("list {}", d.display())),
        };
        for it in items {
            if pipeline::inputs::ignored(&it.name) || (rel.is_empty() && (it.name == "how" || it.name == "todo")) {
                continue;
            }
            let path = if rel.is_empty() { it.name.clone() } else { format!("{rel}/{}", it.name) };
            if it.is_dir {
                stack.push((d.join(&it.name), path));
                continue;
            }
            if !it.name.ends_with(".toml") || pipeline::inputs::regions::resolve::special(&path).is_some() {
                continue;
            }
            let p = d.join(&it.name);
            let text = match pool.read_all(&p) {
                Ok(b) => String::from_utf8(b).map_err(anyhow::Error::from),
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(IoError::Io(e)) if pool.is_online() => Err(e.into()),
                Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
            };
            match text.and_then(|t| {
                pipeline::inputs::regions::file::parse(&it.name, &t)
                    .map_err(|fs| anyhow::anyhow!(fs.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; ")))
            }) {
                Ok(r) => ok.push(Listed::of(&r, path)),
                Err(e) => bad.push((path, format!("{e:#}"))),
            }
        }
    }
    ok.sort_by(|a, b| a.id.cmp(&b.id));
    bad.sort();
    Ok((ok, bad))
}

/// Every minute: the regions' accepted copy (for the layer) brought up to date.
pub fn spawn_syncer(s: S) {
    std::thread::Builder::new()
        .name("regions".into())
        .spawn(move || loop {
            sync_accepted(&s);
            std::thread::sleep(std::time::Duration::from_secs(60));
        })
        .ok();
}

pub async fn list(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let cache = s2.data.home.join("regions.json");
        match nas(&s2) {
            Some((dir, pool)) => {
                // The NAS not answering is a 503, as for any read it can't answer; the list kept
                // for going away stays as it was.
                let (mut ok, bad) = match load_all(&pool, &dir) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("regions: {e:#}");
                        return err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"));
                    }
                };
                states(&s2, &pool, &mut ok);
                if let Ok(b) = serde_json::to_vec(&ok) {
                    let tmp = cache.with_extension("json.tmp");
                    if std::fs::write(&tmp, b).is_ok() {
                        std::fs::rename(&tmp, &cache).ok();
                    }
                }
                let t = tree(&s2, &ok);
                Json(json!({ "regions": ok, "bad": bad, "tree": t })).into_response()
            }
            None => {
                // Away: the last list read.
                let last: Vec<Listed> =
                    std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                let t = tree(&s2, &last);
                Json(json!({ "regions": last, "bad": [], "offline": true, "tree": t })).into_response()
            }
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ---- the input check's view of the regions ------------------------------------------------------

/// The regions' records on the NAS: the accepted index and the held report, each when named and
/// read (through the pool).
fn records(
    s: &crate::AppState,
    pool: &IoPool,
) -> Option<(Option<pipeline::inputs::Index>, Option<pipeline::inputs::Report>)> {
    let root = s.data.nas_root()?;
    let m = pipeline::inputs::served::manifest(&|p: &str| Ok(pool.read_all(&root.join(p))?)).ok()?;
    let read = |p: &str| pool.read_all(&root.join(p)).ok();
    let unit = pipeline::inputs::regions::UNIT;
    let index =
        m.get(&pipeline::inputs::logical(unit)).and_then(|n| read(n)).and_then(|b| serde_json::from_slice(&b).ok());
    let held = m
        .get(&pipeline::inputs::held_logical(unit))
        .and_then(|n| read(n))
        .and_then(|b| serde_json::from_slice(&b).ok());
    Some((index, held))
}

/// Each region's state in the input check: held (with the findings holding it), accepted (its
/// name and parts as the accepted version has them), or checking; none when the records can't be
/// read now.
fn states(s: &crate::AppState, pool: &IoPool, list: &mut [Listed]) {
    let Some((index, held)) = records(s, pool) else { return };
    for r in list.iter_mut() {
        let findings: Vec<Value> = held
            .iter()
            .flat_map(|h| h.findings.iter())
            .filter(|f| f.files.contains(&r.path))
            .map(|f| json!({ "id": f.id, "message": f.message }))
            .collect();
        if held.as_ref().is_some_and(|h| h.held.contains(&r.path)) {
            r.state = "held".into();
            r.held = findings;
            continue;
        }
        let same = index.as_ref().and_then(|i| i.files.get(&r.path)).is_some_and(|e| {
            e.facts.get("name").and_then(Value::as_str) == Some(r.name.as_str())
                && e.facts.get("outline") == Some(&json!(r.outline))
                && e.facts.get("disabled").and_then(Value::as_bool).unwrap_or(false) == r.disabled
        });
        r.state = if same { "accepted" } else { "checking" }.into();
    }
}

/// The Regions list's tree (docs/inputs.md §6.1.2): the accepted version's folders, their names
/// from their `_group.toml`, and their regions (pipeline::inputs::regions::read::tree), from the
/// copy kept on this Mac; with none yet, the folders of the files listed, by their own names.
fn tree(s: &crate::AppState, listed: &[Listed]) -> pipeline::inputs::regions::read::Folder {
    sync_accepted(s);
    let kept: Option<pipeline::inputs::served::Kept> =
        std::fs::read(accepted_dir(s).join("@index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let index = kept.map(|k| k.index).unwrap_or_else(|| {
        let mut i = pipeline::inputs::Index::default();
        for l in listed {
            let mut facts = serde_json::Map::new();
            facts.insert("id".into(), l.id.clone().into());
            i.files.insert(
                l.path.clone(),
                pipeline::inputs::FileEntry { file: String::new(), size: 0, keyed: String::new(), facts },
            );
        }
        i
    });
    pipeline::inputs::regions::read::tree(&index)
}

/// Where this Mac keeps the regions' accepted version (its drawn files with it).
fn accepted_dir(s: &crate::AppState) -> std::path::PathBuf {
    crate::accepted::local_dir(&s.data.home, pipeline::inputs::regions::UNIT)
}

/// The regions' accepted version kept on this Mac in step with the records, with each region's
/// drawn file (what the layer draws); nothing while the NAS is away.
pub fn sync_accepted(s: &crate::AppState) {
    let (Some(root), Some(pool)) = (s.data.nas_root(), s.data.pool()) else { return };
    if !pool.is_online() {
        return;
    }
    let read = |p: &str| -> anyhow::Result<Vec<u8>> {
        Ok(crate::views::RemoteFile::new(root.join(p), pool.clone()).read_all()?)
    };
    if let Err(e) = pipeline::inputs::served::sync_with(
        &read,
        pipeline::inputs::regions::UNIT,
        &accepted_dir(s),
        &["drawn", "ts12"],
    ) {
        eprintln!("regions: the accepted copy wasn't brought up to date: {e:#}");
    }
}

/// A region's colour, from its id's hash (stable: a region keeps its colour whatever is added).
pub fn colour(id: &str) -> String {
    let h = store::naming::hash16(id.as_bytes());
    let v = u64::from_str_radix(&h[..8], 16).unwrap_or(0);
    // A hue from the hash, at a fixed saturation and lightness, for the dark map.
    let hue = (v % 360) as f64;
    let (s, l) = (0.65, 0.6);
    let c = (1.0 - (2.0 * l - 1.0f64).abs()) * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    format!("#{:02x}{:02x}{:02x}", to(r), to(g), to(b))
}

/// Whether region `id` is disabled (`disabled = true`): by the accepted copy kept on this Mac, else
/// the last list read from the inputs folder.
pub fn is_disabled(s: &crate::AppState, id: &str) -> bool {
    let dir = accepted_dir(s);
    let kept: Option<pipeline::inputs::served::Kept> =
        std::fs::read(dir.join("@index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(k) = kept {
        return k.index.files.values().any(|e| {
            e.facts.get("id").and_then(Value::as_str) == Some(id)
                && e.facts.get("disabled").and_then(Value::as_bool) == Some(true)
        });
    }
    let last: Vec<Listed> = std::fs::read(s.data.home.join("regions.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    last.iter().any(|l| l.id == id && l.disabled)
}

/// The region layer (docs/inputs.md §6.1.9): every region's accepted outline, from the drawn files
/// kept on this Mac, a feature per stored shape with the region's id, name, colour and whether it's
/// held now; the catalogue's regions (as built) until this Mac has the accepted copy.
pub async fn layer(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        sync_accepted(&s2);
        let held: std::collections::BTreeSet<String> = s2.data.pool().filter(|p| p.is_online()).and_then(|p| records(&s2, &p)).and_then(|(_, h)| h).map(|h| h.held.into_iter().collect()).unwrap_or_default();
        let dir = accepted_dir(&s2);
        let kept = pipeline::inputs::served::files(&dir);
        let mut feats = Vec::new();
        let source = if kept.is_some() {
            let index: Option<pipeline::inputs::served::Kept> = std::fs::read(dir.join("@index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
            for (path, e) in index.map(|k| k.index.files).unwrap_or_default() {
                // (The 12-nautical-mile line, with the pins: one feature of lines.)
                if let Some(lines) = e.facts.get("ts12").and_then(Value::as_str).and_then(|n| std::fs::read(dir.join(n)).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                    feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiLineString", "coordinates": lines["lines"] }, "properties": { "kind": "ts12" } }));
                }
                let Some(drawn) = e.facts.get("drawn").and_then(Value::as_str) else { continue };
                let disabled = e.facts.get("disabled").and_then(Value::as_bool).unwrap_or(false);
                let Some(d) = std::fs::read(dir.join(drawn)).ok().and_then(|b| serde_json::from_slice::<pipeline::inputs::regions::stored::Drawn>(&b).ok()) else { continue };
                for (from, coords) in &d.shapes {
                    feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": { "region": d.id, "name": d.name, "from": from, "colour": colour(&d.id), "held": held.contains(&path), "disabled": disabled } }));
                }
            }
            "accepted"
        } else {
            for r in recorded(&s2.data.catalogue()) {
                for (from, coords) in &r.shapes {
                    feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": { "region": r.id, "name": r.name, "from": from, "colour": colour(&r.id), "held": false } }));
                }
            }
            store::catalogue::NAME
        };
        Ok(Json(json!({ "type": "FeatureCollection", "features": feats, "source": source })).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn feature(ix: &OutlineIndex, o: &OutlineRec, extra: Value) -> Result<Value> {
    let coords = ix.polygons(o)?;
    let mut props = ix.summary(o);
    if let (Some(p), Some(x)) = (props.as_object_mut(), extra.as_object()) {
        p.extend(x.clone());
    }
    Ok(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": props }))
}

pub async fn one(State(s): State<S>, Path(id): Path<u64>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let Some(ix) = s2.areas.get(&s2)? else { return Ok(err(StatusCode::NOT_FOUND, "no outlines yet")) };
        let Some(o) = ix.by_id(id) else { return Ok(err(StatusCode::NOT_FOUND, format!("no outline {id}"))) };
        Ok(Json(feature(&ix, o, json!({}))?).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// The regions a catalogue records (its `coverage`), each with its outlines simplified for drawing;
/// none for a catalogue made before they were recorded (or one whose coverage this app can't read).
pub fn recorded(cat: &store::catalogue::Catalogue) -> Vec<DrawnRegion> {
    cat.coverage.get("regions").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// The regions a catalogue records, without their outlines (id, name and outline entries, and a region
/// still building's `tiles`: built of total), read straight from its coverage: /api/catalog asks
/// for them every minute.
pub fn recorded_list(cat: &store::catalogue::Catalogue) -> Vec<Value> {
    let regions = cat.coverage.get("regions").and_then(Value::as_array);
    regions
        .map(|rs| {
            rs.iter()
                .filter(|r| r.is_object())
                .map(|r| {
                    let mut v = json!({ "id": r["id"], "name": r["name"], "outline": r["outline"] });
                    if r.get("tiles").is_some_and(Value::is_object) {
                        v["tiles"] = r["tiles"].clone();
                    }
                    v
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The recorded regions' outlines as GeoJSON features, one per entry in each recipe's order, with
/// the properties the panel reads: region, region_name, entry, and an osm: entry's area fields when
/// the pass's outlines can be read (`ix`).
fn recorded_features(regions: &[DrawnRegion], ix: Option<&OutlineIndex>) -> Vec<Value> {
    let mut feats = Vec::new();
    for r in regions {
        for entry in &r.outline {
            let Some(coords) = r.shapes.get(entry) else { continue };
            let mut props = json!({ "region": r.id, "region_name": r.name, "entry": entry });
            let area = entry
                .strip_prefix("osm:")
                .and_then(|id| id.parse::<u64>().ok())
                .and_then(|id| ix.and_then(|ix| ix.by_id(id).map(|o| ix.summary(o))));
            if let (Some(Value::Object(a)), Some(p)) = (area, props.as_object_mut()) {
                for (k, v) in a {
                    p.entry(k).or_insert(v);
                }
            }
            feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": props }));
        }
    }
    feats
}

/// The coverage for drawing, as the current catalogue recorded it, with the regions it was built for
/// (`regions`), so the panel can tell which regions aren't on the map yet (added or redrawn since)
/// and works away from home.
pub async fn coverage(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let cat = s2.data.catalogue();
        let regions = recorded(&cat);
        // The areas' fields are a nicety: without the pass's outlines (the NAS away, not mirrored
        // yet) the outlines are drawn all the same.
        let ix = s2.areas.get(&s2).unwrap_or_else(|e| {
            eprintln!("coverage: the pass's outlines: {e:#}");
            None
        });
        let feats = recorded_features(&regions, ix.as_deref());
        Ok(Json(
            json!({ "type": "FeatureCollection", "features": feats, "regions": recorded_list(&cat), "catalog": cat.n }),
        )
        .into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A pool over the share `root` with a minute for each operation: none here is timed.
    fn pool(root: &store::nasfs::NasRoot) -> Arc<IoPool> {
        IoPool::new(2, Duration::from_secs(60), root.unchecked_path())
    }

    /// A pool over the share `root` that gives up on an operation after 300 ms (`hang`'s never
    /// end), and waits as long as it takes for the share to answer a probe after one: busy, not
    /// away, however slowly a busy Mac answers it.
    fn hung_pool(root: &store::nasfs::NasRoot) -> Arc<IoPool> {
        let cfg = store::iopool::PoolConfig::new(2, Duration::from_millis(300), root.unchecked_path());
        IoPool::with_config(store::iopool::PoolConfig {
            probe_interval: Duration::from_millis(50),
            alive_timeout: Duration::from_secs(600),
            ..cfg
        })
    }

    /// A region file's text.
    fn region(id: &str, name: &str) -> String {
        format!("id = \"{id}\"\nname = \"{name}\"\noutline = [\"osm:1877178\"]\n")
    }

    /// Makes `p` a named pipe: reading it waits for a writer that never comes, as a read waits on a
    /// hung mount.
    fn hang(p: &std::path::Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo {}", p.display());
    }

    /// Lets the reads waiting on `hang`'s pipe finish (with nothing), so no pool thread stays stuck.
    fn release(p: &std::path::Path) {
        use std::os::unix::fs::OpenOptionsExt;
        // (A watchdog: the reader is a stuck pool thread, already waiting.)
        // (A writer that doesn't wait opens once a reader waits, and closes at once.)
        store::testkit::eventually(&format!("a reader of {}", p.display()), || {
            std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(p).is_ok()
        });
    }

    #[test]
    fn lists_every_folder() {
        let share = tempfile::tempdir().unwrap();
        let dir = share.path().join("inputs/regions");
        let pool = pool(&store::nasfs::NasRoot::local_for_test(share.path()));
        // No folder yet: no regions.
        assert_eq!(load_all(&pool, &store::nasfs::NasPath::local_for_test(&dir)).unwrap(), (vec![], vec![]));
        std::fs::create_dir_all(dir.join("eu/pt")).unwrap();
        std::fs::write(dir.join("borders.toml"), region("borders", "Scottish Borders")).unwrap();
        std::fs::write(
            dir.join("eu/pt/azores.toml"),
            "id = \"azores\"\nname = \"Azores\"\noutline = [{ buffer = \"osm:1629145\", by = \"12 nmi\" }]\n",
        )
        .unwrap();
        // Broken ones reported; other files (a `.toml.removed` among them), ignored names, the special files (`_`
        // names) and how/ left out.
        std::fs::write(dir.join("bad.toml"), "id = \"other\"\nname = \"x\"\noutline = [\"osm:1\"]").unwrap();
        std::fs::write(dir.join("old.toml.removed"), region("old", "Old")).unwrap();
        std::fs::write(dir.join(".half.toml.tmp"), "half").unwrap();
        for (n, t) in [("_pins.toml", "osm = \"2026-09-28\"\n"), ("_rules.toml", "cover = \"earth\"\n")] {
            std::fs::write(dir.join(n), t).unwrap();
        }
        std::fs::create_dir_all(dir.join("how")).unwrap();
        std::fs::write(dir.join("how/example.toml"), region("example", "Example")).unwrap();
        let (ok, bad) = load_all(&pool, &store::nasfs::NasPath::local_for_test(&dir)).unwrap();
        assert_eq!(
            ok.iter().map(|l| (l.id.as_str(), l.path.as_str())).collect::<Vec<_>>(),
            [("azores", "eu/pt/azores.toml"), ("borders", "borders.toml")]
        );
        assert_eq!(bad.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["bad.toml"]);
        assert!(pool.is_online());
    }

    #[test]
    fn a_disabled_region_is_listed_as_disabled() {
        let share = tempfile::tempdir().unwrap();
        let dir = share.path().join("inputs/regions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("on.toml"), region("on", "On")).unwrap();
        std::fs::write(dir.join("off.toml"), format!("{}disabled = true\n", region("off", "Off"))).unwrap();
        let (ok, bad) = load_all(
            &pool(&store::nasfs::NasRoot::local_for_test(share.path())),
            &store::nasfs::NasPath::local_for_test(&dir),
        )
        .unwrap();
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(ok.iter().map(|l| (l.id.as_str(), l.disabled)).collect::<Vec<_>>(), [("off", true), ("on", false)]);
        // The panel's field: there only when set.
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v[0]["disabled"], true);
        assert!(v[1].get("disabled").is_none());
        let back: Vec<Listed> = serde_json::from_value(v).unwrap();
        assert_eq!(back, ok);
    }

    #[test]
    fn a_hung_share_fails_promptly() {
        let share = tempfile::tempdir().unwrap();
        let dir = share.path().join("inputs/regions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("borders.toml"), region("borders", "Scottish Borders")).unwrap();
        let (pool, hung) = (
            pool(&store::nasfs::NasRoot::local_for_test(share.path())),
            hung_pool(&store::nasfs::NasRoot::local_for_test(share.path())),
        );
        // A region file the share never gives: reading it fails (in the pool's time, rather than
        // never: the read itself never ends).
        let stuck = dir.join("stuck.toml");
        hang(&stuck);
        // (A watchdog: a listing that waited on the read would never return.)
        let (tx, rx) = std::sync::mpsc::channel();
        let (h, d) = (hung.clone(), dir.clone());
        std::thread::spawn(move || tx.send(load_all(&h, &store::nasfs::NasPath::local_for_test(&d)).unwrap_err()).ok());
        let e = rx
            .recv_timeout(Duration::from_secs(300))
            .unwrap_or_else(|_| panic!("the listing waited on the hung read: no answer in 5 min"));
        assert!(IoError::find(&e).is_some(), "{e:#}");
        release(&stuck);
        // The share still answers, so it's busy rather than away: the rest goes on.
        assert!(hung.is_online());
        std::fs::remove_file(&stuck).unwrap();
        assert_eq!(load_all(&pool, &store::nasfs::NasPath::local_for_test(&dir)).unwrap().0.len(), 1);
    }

    async fn json_of(r: Response) -> Value {
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    fn ids(v: &Value) -> Vec<String> {
        v["regions"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
    }

    #[tokio::test]
    async fn away_the_last_list_read() {
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, gone) = (nas.path().join("project"), nas.path().join("gone"));
        std::fs::create_dir_all(root.join("inputs/regions")).unwrap();
        std::fs::write(root.join("inputs/regions/borders.toml"), region("borders", "Scottish Borders")).unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(&root));
        let v = json_of(list(State(s.clone())).await).await;
        assert_eq!((ids(&v), v.get("offline")), (vec!["borders".to_string()], None));
        // The share goes away: the list is the last one read.
        std::fs::rename(&root, &gone).unwrap();
        s.data.pool().unwrap().mark_offline("test");
        let v = json_of(list(State(s.clone())).await).await;
        assert_eq!((ids(&v), v["offline"].as_bool()), (vec!["borders".to_string()], Some(true)));
        assert_eq!(coverage(State(s.clone())).await.status(), StatusCode::OK, "drawn from the catalogue, away too");
    }

    fn catalogue(coverage: Value) -> store::catalogue::Catalogue {
        let mut c = store::catalogue::Catalogue::new(3);
        c.coverage = coverage;
        c
    }

    #[test]
    fn coverage_as_the_catalogue_recorded_it() {
        let ring = json!([[1.0, 50.0], [2.0, 50.0], [2.0, 51.0], [1.0, 50.0]]);
        let c = catalogue(json!({"regions": [
            {"id": "b", "name": "B", "outline": ["poly:b.poly", "osm:7"], "shapes": {"poly:b.poly": [[ring]]}},
            {"id": "a", "name": "A", "outline": ["place:1,2,3"], "shapes": {}},
        ]}));
        let r = recorded(&c);
        assert_eq!(r.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        // A feature per entry with an outline, with what the panel reads (an osm: entry's area
        // fields come from the pass's outlines, none here).
        let f = recorded_features(&r, None);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0]["properties"], json!({"region": "b", "region_name": "B", "entry": "poly:b.poly"}));
        assert_eq!(f[0]["geometry"], json!({"type": "MultiPolygon", "coordinates": [[ring]]}));
        // The regions, outlines left out; every region recorded, drawn or not.
        assert_eq!(
            recorded_list(&c),
            [
                json!({"id": "b", "name": "B", "outline": ["poly:b.poly", "osm:7"]}),
                json!({"id": "a", "name": "A", "outline": ["place:1,2,3"]})
            ]
        );
        // Catalogues made before catalogues recorded their coverage: none (then it's the recipes').
        for old in [json!({"regions": []}), Value::Null, json!({"regions": ["northumberland"]})] {
            assert!(recorded(&catalogue(old.clone())).is_empty() && recorded_list(&catalogue(old)).is_empty());
        }
    }

    #[test]
    fn what_an_outline_lies_in() {
        // France holding Guadeloupe (GP, coded FR-971) holding a town; a district of France.
        let sq = |id: u64, lvl: u8, tags: &str, x0: f64, x1: f64| {
            format!(
                r#"{{"type":"Feature","geometry":{{"type":"Polygon","coordinates":[[[{x0},{x0}],[{x1},{x0}],[{x1},{x1}],[{x0},{x1}],[{x0},{x0}]]]}},"properties":{{"@type":"relation","@id":{id},"boundary":"administrative","admin_level":"{lvl}",{tags}}}}}"#
            )
        };
        let d = tempfile::tempdir().unwrap();
        let geo = d.path().join("o.geojsonseq");
        let lines = [
            sq(1, 2, r#""ISO3166-1":"FR","name":"France""#, 0.0, 10.0),
            sq(2, 3, r#""ISO3166-1":"GP","ISO3166-2":"FR-971","name":"Guadeloupe""#, 1.0, 2.0),
            sq(3, 8, r#""name":"Town""#, 1.2, 1.4),
            sq(4, 6, r#""name":"District""#, 5.0, 6.0),
        ];
        std::fs::write(&geo, lines.iter().map(|l| format!("\u{1e}{l}\n")).collect::<String>()).unwrap();
        let out = d.path().join("o.sect");
        pipeline::outlines::assemble_geojsonseq(&geo, &out).unwrap();
        let o = pipeline::outlines::Outlines::open(&out).unwrap();
        let at = |id| lies_in(&o.recs, |i| o.string(i), o.by_id(id).unwrap());
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(at(1), s("", ""), "a country lies in none");
        assert_eq!(at(2), s("", ""), "nor a territory");
        assert_eq!(at(3), s("GP", "Guadeloupe"));
        assert_eq!(at(4), s("FR", "France"));
    }
}
