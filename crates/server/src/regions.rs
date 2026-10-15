//! Regions and the areas they're made of (docs/plan.md §1, the Regions panel; §5).
//!
//!   GET    /api/regions             the region files in inputs/regions on the NAS, in every folder,
//!                                   each with its path, whether it's plain (only osm:/shape: parts,
//!                                   nothing else) and its state in the input check (accepted,
//!                                   checking, held with the findings), and the files that don't
//!                                   parse (docs/inputs.md §6.1.9)
//!   GET    /api/regions/layer       every region's accepted outline for drawing (its drawn files,
//!                                   kept on this Mac), a colour from its id, held or not; the
//!                                   catalog's regions until this Mac has the accepted copy
//!   POST   /api/regions             {id, name, outline}: a new region at the folder's top (created
//!                                   exclusively: 409 when the id is taken); osm: and shape: parts
//!   PUT    /api/regions/{id}        {name, outline}: rename or redraw a plain region, found by id in
//!                                   whichever folder it lies (another is "edit the file")
//!   DELETE /api/regions/{id}        removed (its recipe kept as .removed)
//!   GET    /api/areas?at=lon,lat    the outlines containing a point, smallest first
//!   GET    /api/areas/search?q=     outlines by name, largest first
//!   GET    /api/areas/{id}          one outline, simplified (a GeoJSON feature)
//!   GET    /api/coverage            the coverage the catalog was built for: its regions' outlines,
//!                                   simplified (GeoJSON), and the regions
//!
//! Outlines come from the latest OSM pass (the catalog's `global.outlines`); records and names are
//! read once per catalog, rings when an outline is drawn or tested.
//!
//! The recipes, and the `.poly` files the coverage draws, are read and written through the NAS I/O
//! pool like every other access to the share: a mount that hangs fails the request in the pool's
//! time (a 503, as for any read the NAS can't answer) instead of holding it.

use crate::S;
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use pipeline::agent::recipes::{self, Recipe};
use pipeline::coverage::DrawnRegion;
use pipeline::outlines::{flag, inside, OutlineRec, Ring};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use store::iopool::{IoError, IoPool};

/// The outlines of the current catalog: records and names in memory, rings read on demand.
pub struct OutlineIndex {
    content: String,
    sect: Arc<crate::views::SectView>,
    pub recs: Vec<OutlineRec>,
    strings: Vec<String>,
    /// Folded names (lower case, no accents) with record indexes, sorted, for search.
    names: Vec<(String, u32)>,
}

fn fold(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfkd().filter(|c| !unicode_normalization::char::is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

impl OutlineIndex {
    fn open(data: &crate::data::Data) -> Result<Option<OutlineIndex>> {
        let cat = data.catalog();
        let Some(logical) = cat.global.get("outlines") else { return Ok(None) };
        let Some(sect) = data.sect(logical)? else { return Ok(None) };
        let content = data.content(logical).unwrap_or_default();
        let recs: Vec<OutlineRec> = bytemuck::pod_collect_to_vec(sect.get("recs")?.bytes());
        let strings: Vec<String> = String::from_utf8_lossy(sect.get("strings")?.bytes()).split('\n').map(str::to_string).collect();
        let mut names: Vec<(String, u32)> = Vec::with_capacity(recs.len() * 2);
        for (i, r) in recs.iter().enumerate() {
            for k in [r.name, r.name_en] {
                if let Some(n) = strings.get(k as usize).filter(|n| !n.is_empty()) {
                    names.push((fold(n), i as u32));
                }
            }
        }
        names.sort();
        names.dedup();
        Ok(Some(OutlineIndex { content, sect, recs, strings, names }))
    }

    pub fn string(&self, i: u32) -> &str {
        self.strings.get(i as usize).map(String::as_str).unwrap_or("")
    }

    /// An outline's rings (`simple`: the simplified ones), read from the file, with their kinds
    /// (0 outer, 1 inner).
    fn rings_kinds(&self, o: &OutlineRec, simple: bool) -> Result<Vec<(u32, Vec<[i32; 2]>)>> {
        let (rs, ps, first, n) = if simple { ("srings", "spoints", o.srings, o.nsrings) } else { ("rings", "points", o.rings, o.nrings) };
        let rb = self.sect.get_part(rs, first as u64 * 16, n as usize * 16)?;
        let rings: Vec<Ring> = bytemuck::pod_collect_to_vec(&rb);
        let (Some(a), Some(b)) = (rings.first(), rings.last()) else { return Ok(Vec::new()) };
        let (start, end) = (a.start, b.start + b.count as u64);
        let pb = self.sect.get_part(ps, start * 8, ((end - start) * 8) as usize)?;
        let pts: Vec<[i32; 2]> = bytemuck::pod_collect_to_vec(&pb);
        Ok(rings.iter().map(|r| (r.kind, pts[(r.start - start) as usize..(r.start - start) as usize + r.count as usize].to_vec())).collect())
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

/// The languages spoken where (`names::spoken`), from the catalog's outlines (their simplified
/// rings, read from the mirror or the NAS) and `table` (None when the catalog has no outlines).
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

/// The outline index of the current catalog, opened once per catalog.
#[derive(Default)]
pub struct Areas {
    cur: Mutex<Option<Arc<OutlineIndex>>>,
}

impl Areas {
    pub fn get(&self, s: &crate::AppState) -> Result<Option<Arc<OutlineIndex>>> {
        let cat = s.data.catalog();
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

/// The recipes folder on the NAS, and the I/O pool every access to it goes through (None while the
/// NAS is away).
fn nas(s: &crate::AppState) -> Option<(std::path::PathBuf, Arc<IoPool>)> {
    let (root, pool) = (s.data.nas_root()?, s.data.pool()?);
    pool.is_online().then(|| (root.join("inputs/regions"), pool))
}

// ---- the recipes on the NAS ---------------------------------------------------------------------
//
// Read and written as `recipes` does for the agent and `scenic add` and `scenic remove`, but every
// access through the pool.

/// Whether a failure means the NAS couldn't be reached: the pool gave up waiting or refused (the
/// NAS offline), or the error took the NAS offline (a soft mount's network error). Nothing is wrong
/// with what was asked then, and it can be asked again.
fn nas_unreachable(e: &anyhow::Error, pool: &IoPool) -> bool {
    match IoError::find(e) {
        Some(IoError::Io(_)) => !pool.is_online(),
        Some(_) => true,
        None => false,
    }
}

/// The recipes, sorted by id, and the files that don't parse (file name, problem).
type Recipes = (Vec<Recipe>, Vec<(String, String)>);

/// Every recipe in the folder, and the files that don't parse, as `recipes::load` reads them. The
/// NAS failing midway is an error, never a shorter list.
fn load(pool: &IoPool, dir: &std::path::Path) -> Result<Recipes> {
    let items = match pool.list(dir) {
        Ok(items) => items,
        // No recipes yet.
        Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
    };
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for it in items.into_iter().filter(|it| it.name.ends_with(".toml")) {
        let p = dir.join(&it.name);
        let text = match pool.read_all(&p) {
            Ok(b) => String::from_utf8(b).map_err(anyhow::Error::from),
            // Removed or renamed since the listing.
            Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // A file that can't be read is listed with the reason, unless the error took the NAS
            // offline.
            Err(IoError::Io(e)) if pool.is_online() => Err(e.into()),
            Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
        };
        match text.and_then(|t| recipes::parse(&it.name, &t)) {
            Ok(r) => ok.push(r),
            Err(e) => bad.push((it.name, format!("{e:#}"))),
        }
    }
    ok.sort_by(|a, b| a.id.cmp(&b.id));
    bad.sort();
    Ok((ok, bad))
}

/// A region file as the panel lists it (docs/inputs.md §6.1.9).
#[derive(serde::Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub outline: Vec<String>,
    /// Its path in the drop box (`europe/portugal/azores.toml`).
    pub path: String,
    /// Only `osm:` and `shape:` parts, nothing else: what the panel edits.
    pub plain: bool,
    /// accepted, checking, held or pending ("" when the check's records can't be read now).
    #[serde(default)]
    pub state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<Value>,
}

impl Listed {
    fn of(r: &pipeline::inputs::regions::file::Region, path: String) -> Listed {
        let plain = r.is_plain() && r.lines.is_empty() && r.pins.is_empty();
        Listed { id: r.id.clone(), name: r.name.clone(), outline: r.outline_text(), path, plain, state: String::new(), held: Vec::new() }
    }
}

/// The region files and the files that don't parse (path, problem), as the panel lists them.
type ListedAll = (Vec<Listed>, Vec<(String, String)>);

/// Every region file in the folder and its subfolders (`how/`, `todo/` and the names every drop box
/// passes over aside, and `pins.toml`), by id; the files that don't parse. The NAS failing midway is
/// an error, never a shorter list.
fn load_all(pool: &IoPool, dir: &std::path::Path) -> Result<ListedAll> {
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    let mut stack = vec![(dir.to_path_buf(), String::new())];
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
            if !it.name.ends_with(".toml") || path == "pins.toml" {
                continue;
            }
            let p = d.join(&it.name);
            let text = match pool.read_all(&p) {
                Ok(b) => String::from_utf8(b).map_err(anyhow::Error::from),
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(IoError::Io(e)) if pool.is_online() => Err(e.into()),
                Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
            };
            match text.and_then(|t| pipeline::inputs::regions::file::parse(&it.name, &t).map_err(|fs| anyhow::anyhow!(fs.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; ")))) {
                Ok(r) => ok.push(Listed::of(&r, path)),
                Err(e) => bad.push((path, format!("{e:#}"))),
            }
        }
    }
    ok.sort_by(|a, b| a.id.cmp(&b.id));
    bad.sort();
    Ok((ok, bad))
}

/// The drop-box path of region `id`'s file, in whichever folder it lies.
fn find(pool: &IoPool, dir: &std::path::Path, id: &str) -> Result<String> {
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    let want = format!("{id}.toml");
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
            } else if it.name == want {
                return Ok(path);
            }
        }
    }
    bail!("no region {id}")
}

/// Whether the records say the build's readers read the regions' accepted version
/// (`sources/inputs/regions/@switched`): before, they read the drop box's recipes, which know only
/// `osm:` parts. `dir` is the regions' folder; false when the records can't be read.
fn switched_on(pool: &IoPool, dir: &std::path::Path) -> bool {
    let Some(root) = dir.parent().and_then(|p| p.parent()) else { return false };
    let Ok(b) = pool.read_all(&root.join(pipeline::inputs::served::RECORDS)) else { return false };
    serde_json::from_slice::<std::collections::BTreeMap<String, String>>(&b).is_ok_and(|m| pipeline::inputs::regions::read::switched(&m))
}

/// A recipe's text checked as a region file is (its parts `osm:` or `shape:` alone: the panel's;
/// before the readers' switch, `osm:` alone, the only kind the drop box's readers share with the
/// check).
fn region_text(r: &Recipe, switched: bool) -> Result<String> {
    for o in &r.outline {
        ensure!(o.starts_with("osm:") || o.starts_with("shape:"), "{o:?}: the panel takes osm:<relation> and shape:<name>.geojson parts (edit the file for others)");
        ensure!(switched || o.starts_with("osm:"), "{o:?}: shape: parts are available after the switch to the checked regions (docs/inputs.md §6.1.10)");
    }
    let text = toml::to_string(r)?;
    pipeline::inputs::regions::file::parse(&format!("{}.toml", r.id), &text).map_err(|fs| anyhow::anyhow!(fs.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; ")))?;
    Ok(text)
}

/// `old` (a plain region file) with its `name` and `outline` lines replaced by `r`'s, the rest
/// (comments, blank lines) as it was; None when either isn't on one line of its own, or the result
/// doesn't read back as `r`.
fn keep_layout(old: &str, r: &Recipe) -> Option<String> {
    let v = toml::Value::try_from(r).ok()?;
    let line = |k: &str| Some(format!("{k} = {}", v.get(k)?));
    let (mut name, mut outline) = (0, 0);
    let mut out: Vec<String> = Vec::new();
    for l in old.lines() {
        let key = l.split('=').next().unwrap_or("").trim();
        if key == "name" {
            name += 1;
            out.push(line("name")?);
        } else if key == "outline" {
            outline += 1;
            out.push(line("outline")?);
        } else {
            out.push(l.to_string());
        }
    }
    let text = out.join("\n") + "\n";
    let back = pipeline::inputs::regions::file::parse(&format!("{}.toml", r.id), &text).ok()?;
    (name == 1 && outline == 1 && back.name == r.name && back.outline_text() == r.outline).then_some(text)
}

/// Applies one edit to the folder. A new region file is created exclusively at the folder's top
/// (`O_EXCL`, so two Macs can't both create it), a changed one is written where it lies to a
/// temporary file renamed over it (no reader sees half of it), and a removed one is renamed to
/// `.removed`, which keeps it for undoing.
fn apply(pool: &IoPool, dir: &std::path::Path, q: &Queued) -> Result<()> {
    match q {
        Queued::Add { recipe } => {
            let text = region_text(recipe, switched_on(pool, dir))?;
            if find(pool, dir, &recipe.id).is_ok() {
                bail!("region {} exists already", recipe.id);
            }
            pool.create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
            let p = dir.join(format!("{}.toml", recipe.id));
            match pool.write_new(&p, text.into_bytes()) {
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => bail!("region {} exists already", recipe.id),
                r => r.with_context(|| format!("create {}", p.display())),
            }
        }
        Queued::Remove { id } => {
            ensure!(recipes::valid_id(id), "{id:?} isn't a region id");
            let p = dir.join(find(pool, dir, id)?);
            match pool.rename(&p, &p.with_extension("toml.removed")) {
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => bail!("no region {id}"),
                r => r.with_context(|| format!("rename {}", p.display())),
            }
        }
        Queued::Edit { id, name, outline } => {
            // (The id makes a path: nothing but a region id may.)
            ensure!(recipes::valid_id(id), "{id:?} isn't a region id");
            let file = format!("{id}.toml");
            let path = find(pool, dir, id)?;
            let p = dir.join(&path);
            let text = match pool.read_all(&p) {
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => bail!("no region {id}"),
                r => r.with_context(|| format!("read {}", p.display()))?,
            };
            let old = String::from_utf8(text)?;
            let region = pipeline::inputs::regions::file::parse(&file, &old).map_err(|fs| anyhow::anyhow!(fs.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; ")))?;
            ensure!(Listed::of(&region, path.clone()).plain, "{id} has more than osm: and shape: parts: edit the file (inputs/regions/{path})");
            let mut r = Recipe { id: region.id, name: region.name, outline: region.outline.iter().map(|x| x.to_string()).collect() };
            if let Some(n) = name {
                r.name = n.clone();
            }
            if let Some(o) = outline {
                r.outline = o.clone();
            }
            let text = region_text(&r, switched_on(pool, dir))?;
            // (The owner's comments and layout kept: only the name's and the outline's lines
            // rewritten, when each is one line; else the file written anew.)
            let text = keep_layout(&old, &r).unwrap_or(text);
            // (Beside the file, an ignored name: `.` starts it, so no listing takes it half-written.)
            let tmp = p.with_file_name(format!(".{file}.tmp"));
            pool.write(&tmp, text.into_bytes()).with_context(|| format!("write {}", tmp.display()))?;
            pool.rename(&tmp, &p).with_context(|| format!("rename {}", tmp.display()))?;
            Ok(())
        }
    }
}

/// A failed edit's status: 503 when the NAS couldn't be reached (as for any read it can't answer;
/// the panel says to try again), 409 when the region exists already, 404 when there's no such
/// region, else 400.
fn status(e: &anyhow::Error, pool: &IoPool) -> StatusCode {
    let msg = format!("{e:#}");
    if nas_unreachable(e, pool) {
        StatusCode::SERVICE_UNAVAILABLE
    } else if msg.contains("exists") {
        StatusCode::CONFLICT
    } else if msg.contains("no region") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_REQUEST
    }
}

// ---- edits made away from home ------------------------------------------------------------------
//
// Away from home (the NAS unreachable), an edit is kept on this Mac (`regions-queue/`, one file per
// edit, in order) and the list shows it as pending; it goes to the NAS once it's back (`flush`).
// The last list read from the NAS is kept too (`regions.json`), so the panel works offline.

/// An edit made while the NAS was away.
#[derive(serde::Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Queued {
    Add { recipe: Recipe },
    Edit { id: String, name: Option<String>, outline: Option<Vec<String>> },
    Remove { id: String },
}

fn queue_dir(s: &crate::AppState) -> std::path::PathBuf {
    s.data.home.join("regions-queue")
}

/// The edits waiting in `queue` (this Mac's folder of them), oldest first.
fn queued(queue: &std::path::Path) -> Vec<(std::path::PathBuf, Queued)> {
    let mut v: Vec<(std::path::PathBuf, Queued)> = std::fs::read_dir(queue)
        .map(|rd| rd.flatten().filter_map(|e| {
            let p = e.path();
            let q: Queued = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((p, q))
        }).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn enqueue(queue: &std::path::Path, q: &Queued) -> Result<()> {
    std::fs::create_dir_all(queue)?;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let p = queue.join(format!("{t:024}.json"));
    let tmp = queue.join(format!("{t:024}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(q)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// The list as these edits leave it (offline: the pending ones).
fn with_queued(mut list: Vec<Listed>, q: &[(std::path::PathBuf, Queued)]) -> Vec<Listed> {
    for (_, e) in q {
        match e {
            Queued::Add { recipe } => {
                list.retain(|r| r.id != recipe.id);
                list.push(Listed { id: recipe.id.clone(), name: recipe.name.clone(), outline: recipe.outline.clone(), path: format!("{}.toml", recipe.id), plain: true, state: "pending".into(), held: Vec::new() });
            }
            Queued::Remove { id } => list.retain(|r| &r.id != id),
            Queued::Edit { id, name, outline } => {
                if let Some(r) = list.iter_mut().find(|r| &r.id == id) {
                    if let Some(n) = name {
                        r.name = n.clone();
                    }
                    if let Some(o) = outline {
                        r.outline = o.clone();
                    }
                    r.state = "pending".into();
                }
            }
        }
    }
    list.sort_by(|a, b| a.id.cmp(&b.id));
    list
}

/// Sends the edits made away from home to the NAS, in order, once it's reachable. One that can't
/// apply any more (its region removed or taken meanwhile) is dropped, with a note in the log.
pub fn flush(s: &crate::AppState) {
    let Some((dir, pool)) = nas(s) else { return };
    flush_to(&pool, &dir, &queue_dir(s));
}

/// `flush`, from the folder `queue` to the recipes folder `dir`. An edit the NAS doesn't answer
/// waits, with those after it, for the next time: it's still wanted, and the order is kept.
fn flush_to(pool: &IoPool, dir: &std::path::Path, queue: &std::path::Path) {
    for (p, q) in queued(queue) {
        match apply(pool, dir, &q) {
            Ok(()) => eprintln!("regions: sent an edit made away from home ({q:?})"),
            Err(e) if nas_unreachable(&e, pool) => {
                eprintln!("regions: the edits made away from home wait, the NAS didn't take them: {e:#}");
                return;
            }
            Err(e) => eprintln!("regions: an edit made away from home no longer applies, dropped: {e:#} ({q:?})"),
        }
        std::fs::remove_file(&p).ok();
    }
}

/// Every minute: send queued edits when the NAS is back.
pub fn spawn_flusher(s: S) {
    std::thread::Builder::new()
        .name("regions".into())
        .spawn(move || loop {
            if !queued(&queue_dir(&s)).is_empty() {
                flush(&s);
            }
            // (The regions' accepted copy, for the layer: brought up to date once a minute.)
            sync_accepted(&s);
            std::thread::sleep(std::time::Duration::from_secs(60));
        })
        .ok();
}

pub async fn list(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let q = queued(&queue_dir(&s2));
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
                let pending: Vec<String> = q.iter().map(|(_, e)| format!("{e:?}")).collect();
                let t = tree(&s2, &ok);
                Json(json!({ "regions": with_queued(ok, &q), "bad": bad, "pending": pending.len(), "tree": t })).into_response()
            }
            None => {
                // Away: the last list read, with the edits waiting to go.
                let last: Vec<Listed> = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
                let t = tree(&s2, &last);
                Json(json!({ "regions": with_queued(last, &q), "bad": [], "pending": q.len(), "offline": true, "tree": t })).into_response()
            }
        }
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// An edit: applied on the NAS, or kept on this Mac until it's reachable (202). A NAS that doesn't
/// answer in time is a 503, as for any request it can't answer, and the edit isn't kept: whether
/// the share still made it is only known once the list is read again.
fn edit_or_queue(s: &crate::AppState, q: Queued) -> Response {
    match nas(s) {
        Some((dir, pool)) => match apply(&pool, &dir, &q) {
            Ok(()) => {
                // (Checked within seconds: this Mac's agent asked to list the drop boxes now.)
                let ask = pipeline::inputs::Ask { unit: pipeline::inputs::regions::UNIT.into(), check: true, by: format!("the Regions panel on {}", pipeline::agent::cond::host_name()), at: pipeline::agent::jobs::now_s(), ..Default::default() };
                if let Err(e) = pipeline::inputs::ask(&s.home.join("agent"), &ask) {
                    eprintln!("regions: the check wasn't asked for now ({e:#}): the next listing takes the edit");
                }
                Json(json!({ "done": true })).into_response()
            }
            Err(e) => {
                let code = status(&e, &pool);
                if code == StatusCode::SERVICE_UNAVAILABLE {
                    eprintln!("regions: {e:#}");
                }
                err(code, format!("{e:#}"))
            }
        },
        None => match enqueue(&queue_dir(s), &q) {
            Ok(()) => (StatusCode::ACCEPTED, Json(json!({ "queued": true }))).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        },
    }
}

pub async fn add(State(s): State<S>, Json(r): Json<Recipe>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = region_text(&r, true) {
            return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
        }
        edit_or_queue(&s2, Queued::Add { recipe: r })
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Deserialize)]
pub struct Edit {
    name: Option<String>,
    outline: Option<Vec<String>>,
}

pub async fn edit(State(s): State<S>, Path(id): Path<String>, Json(e): Json<Edit>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(o) = &e.outline {
            if let Err(e) = region_text(&Recipe { id: id.clone(), name: "-".into(), outline: o.clone() }, true) {
                return err(StatusCode::BAD_REQUEST, format!("{e:#}"));
            }
        }
        edit_or_queue(&s2, Queued::Edit { id, name: e.name, outline: e.outline })
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn remove(State(s): State<S>, Path(id): Path<String>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || edit_or_queue(&s2, Queued::Remove { id }))
        .await
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ---- the input check's view of the regions ------------------------------------------------------

/// The regions' records on the NAS: the accepted index and the held report, each when named and
/// read (through the pool).
fn records(s: &crate::AppState, pool: &IoPool) -> Option<(Option<pipeline::inputs::Index>, Option<pipeline::inputs::Report>)> {
    let root = s.data.nas_root()?;
    let read = |p: &str| pool.read_all(&root.join(p)).ok();
    let m: std::collections::BTreeMap<String, String> = serde_json::from_slice(&read(pipeline::inputs::served::RECORDS)?).ok()?;
    let unit = pipeline::inputs::regions::UNIT;
    let index = m.get(&pipeline::inputs::logical(unit)).and_then(|n| read(n)).and_then(|b| serde_json::from_slice(&b).ok());
    let held = m.get(&pipeline::inputs::held_logical(unit)).and_then(|n| read(n)).and_then(|b| serde_json::from_slice(&b).ok());
    Some((index, held))
}

/// Each region's state in the input check: held (with the findings holding it), accepted (its
/// name and parts as the accepted version has them), or checking; none when the records can't be
/// read now.
fn states(s: &crate::AppState, pool: &IoPool, list: &mut [Listed]) {
    let Some((index, held)) = records(s, pool) else { return };
    for r in list.iter_mut() {
        let findings: Vec<Value> = held.iter().flat_map(|h| h.findings.iter()).filter(|f| f.files.contains(&r.path)).map(|f| json!({ "id": f.id, "message": f.message })).collect();
        if held.as_ref().is_some_and(|h| h.held.contains(&r.path)) {
            r.state = "held".into();
            r.held = findings;
            continue;
        }
        let same = index.as_ref().and_then(|i| i.files.get(&r.path)).is_some_and(|e| e.facts.get("name").and_then(Value::as_str) == Some(r.name.as_str()) && e.facts.get("outline") == Some(&json!(r.outline)));
        r.state = if same { "accepted" } else { "checking" }.into();
    }
}

/// The Regions list's tree (docs/inputs.md §6.1.2): the accepted version's folders, their names
/// from their `_group.toml`, and their regions (pipeline::inputs::regions::read::tree), from the
/// copy kept on this Mac; with none yet, the folders of the files listed, by their own names.
fn tree(s: &crate::AppState, listed: &[Listed]) -> pipeline::inputs::regions::read::Folder {
    sync_accepted(s);
    let kept: Option<pipeline::inputs::served::Kept> = std::fs::read(accepted_dir(s).join("@index.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let index = kept.map(|k| k.index).unwrap_or_else(|| {
        let mut i = pipeline::inputs::Index::default();
        for l in listed {
            let mut facts = serde_json::Map::new();
            facts.insert("id".into(), l.id.clone().into());
            facts.insert("geometry".into(), "-".into());
            i.files.insert(l.path.clone(), pipeline::inputs::FileEntry { file: String::new(), size: 0, keyed: String::new(), facts });
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
    let read = |p: &str| -> anyhow::Result<Vec<u8>> { Ok(crate::views::RemoteFile::new(root.join(p), pool.clone()).read_all()?) };
    if let Err(e) = pipeline::inputs::served::sync_with(&read, pipeline::inputs::regions::UNIT, &accepted_dir(s), &["drawn"]) {
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

/// The region layer (docs/inputs.md §6.1.9): every region's accepted outline, from the drawn files
/// kept on this Mac, a feature per stored shape with the region's id, name, colour and whether it's
/// held now; the catalog's regions (as built) until this Mac has the accepted copy.
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
                let Some(drawn) = e.facts.get("drawn").and_then(Value::as_str) else { continue };
                let Some(d) = std::fs::read(dir.join(drawn)).ok().and_then(|b| serde_json::from_slice::<pipeline::inputs::regions::stored::Drawn>(&b).ok()) else { continue };
                for (from, coords) in &d.shapes {
                    feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": { "region": d.id, "name": d.name, "from": from, "colour": colour(&d.id), "held": held.contains(&path) } }));
                }
            }
            "accepted"
        } else {
            for r in recorded(&s2.data.catalog()) {
                for (from, coords) in &r.shapes {
                    feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": { "region": r.id, "name": r.name, "from": from, "colour": colour(&r.id), "held": false } }));
                }
            }
            "catalog"
        };
        Ok(Json(json!({ "type": "FeatureCollection", "features": feats, "source": source })).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Deserialize)]
pub struct AtQ {
    at: String,
}

pub async fn at(State(s): State<S>, Query(q): Query<AtQ>) -> Response {
    let v: Vec<f64> = q.at.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if v.len() != 2 {
        return err(StatusCode::BAD_REQUEST, "at=lon,lat");
    }
    let p = [(v[0] * 1e7).round() as i32, (v[1] * 1e7).round() as i32];
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let Some(ix) = s2.areas.get(&s2)? else { return Ok(err(StatusCode::NOT_FOUND, "no outlines yet (the OSM pass makes them)")) };
        let mut found: Vec<&OutlineRec> = Vec::new();
        for o in ix.recs.iter().filter(|o| p[0] >= o.bbox[0] && p[0] <= o.bbox[2] && p[1] >= o.bbox[1] && p[1] <= o.bbox[3]) {
            let rings = ix.rings(o, false)?;
            let r: Vec<&[[i32; 2]]> = rings.iter().map(Vec::as_slice).collect();
            if inside(&r, p) {
                found.push(o);
            }
        }
        found.sort_by(|a, b| a.area_km2.total_cmp(&b.area_km2));
        Ok(Json(json!({ "areas": found.iter().map(|o| ix.summary(o)).collect::<Vec<_>>() })).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Deserialize)]
pub struct SearchQ {
    q: String,
}

pub async fn search(State(s): State<S>, Query(q): Query<SearchQ>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let Some(ix) = s2.areas.get(&s2)? else { return Ok(err(StatusCode::NOT_FOUND, "no outlines yet")) };
        let k = fold(q.q.trim());
        if k.is_empty() {
            return Ok(Json(json!({ "areas": [] })).into_response());
        }
        let from = ix.names.partition_point(|(n, _)| n.as_str() < k.as_str());
        let mut hits: Vec<u32> = ix.names[from..].iter().take_while(|(n, _)| n.starts_with(&k)).map(|(_, i)| *i).collect();
        hits.sort_unstable();
        hits.dedup();
        let mut recs: Vec<&OutlineRec> = hits.iter().map(|&i| &ix.recs[i as usize]).collect();
        recs.sort_by(|a, b| b.area_km2.total_cmp(&a.area_km2));
        recs.truncate(30);
        Ok(Json(json!({ "areas": recs.iter().map(|o| ix.summary(o)).collect::<Vec<_>>() })).into_response())
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

/// The regions a catalog records (its `coverage`), each with its outlines simplified for drawing;
/// none for a catalog made before they were recorded (or one whose coverage this app can't read).
pub fn recorded(cat: &store::catalog::Catalog) -> Vec<DrawnRegion> {
    cat.coverage.get("regions").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// The regions a catalog records, without their outlines (id, name and outline entries), read
/// straight from its coverage: /api/catalog asks for them every minute.
pub fn recorded_list(cat: &store::catalog::Catalog) -> Vec<Value> {
    let regions = cat.coverage.get("regions").and_then(Value::as_array);
    regions.map(|rs| rs.iter().filter(|r| r.is_object()).map(|r| json!({ "id": r["id"], "name": r["name"], "outline": r["outline"] })).collect()).unwrap_or_default()
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
            let area = match recipes::parse_outline(entry) {
                Ok(recipes::Outline::Osm(id)) => ix.and_then(|ix| ix.by_id(id).map(|o| ix.summary(o))),
                _ => None,
            };
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

/// The coverage for drawing, as the current catalog recorded it, with the regions it was built for
/// (`regions`), so the panel can tell which recipes aren't on the map yet (added or redrawn since)
/// and works away from home. A catalog made before catalogs recorded their coverage has none (and
/// no `recorded` mark: one built for no region yet has it): then it's built from the recipes on the
/// NAS, as it was, without `regions`.
pub async fn coverage(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let cat = s2.data.catalog();
        let regions = recorded(&cat);
        if regions.is_empty() && cat.coverage.get("recorded").and_then(serde_json::Value::as_bool) != Some(true) {
            return coverage_from_recipes(&s2, cat.n);
        }
        // The areas' fields are a nicety: without the pass's outlines (the NAS away, not mirrored
        // yet) the outlines are drawn all the same.
        let ix = s2.areas.get(&s2).unwrap_or_else(|e| {
            eprintln!("coverage: the pass's outlines: {e:#}");
            None
        });
        let feats = recorded_features(&regions, ix.as_deref());
        Ok(Json(json!({ "type": "FeatureCollection", "features": feats, "regions": recorded_list(&cat), "catalog": cat.n })).into_response())
    })
    .await
    .map(|r| r.unwrap_or_else(|e| err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"))))
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Every region's outlines for drawing, from the recipes on the NAS (a catalog that records none):
/// osm: entries simplified from the outlines, the others as written (poly files, place circles).
fn coverage_from_recipes(s: &crate::AppState, n: u64) -> Result<Response> {
    let Some((dir, pool)) = nas(s) else { return Ok(err(StatusCode::SERVICE_UNAVAILABLE, "the NAS isn't reachable")) };
    let (regions, _) = load(&pool, &dir)?;
    let ix = s.areas.get(s)?;
    let mut feats = Vec::new();
    for r in &regions {
        for entry in &r.outline {
            let extra = json!({ "region": r.id, "region_name": r.name, "entry": entry });
            match recipes::parse_outline(entry)? {
                recipes::Outline::Osm(id) => {
                    if let Some((ix, o)) = ix.as_ref().and_then(|ix| ix.by_id(id).map(|o| (ix, o))) {
                        feats.push(feature(ix, o, extra)?);
                    }
                }
                other => {
                    let rings = match other {
                        recipes::Outline::Place { lon, lat, km } => vec![(0..65)
                            .map(|i| {
                                let t = i as f64 / 64.0 * std::f64::consts::TAU;
                                [lon + km / (111.32 * lat.to_radians().cos().max(0.01)) * t.cos(), lat + km / 110.574 * t.sin()]
                            })
                            .collect::<Vec<_>>()],
                        recipes::Outline::Poly(f) => poly_rings(&pool, &dir.parent().unwrap_or(&dir).join("outlines").join(f))?,
                        recipes::Outline::Geofabrik(g) => poly_rings(&pool, &dir.parent().unwrap_or(&dir).join("outlines/geofabrik").join(format!("{}.poly", g.replace('/', "-"))))?,
                        recipes::Outline::Osm(_) => unreachable!(),
                    };
                    let coords: Vec<Vec<Vec<[f64; 2]>>> = rings.into_iter().map(|r| vec![r]).collect();
                    feats.push(json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": coords }, "properties": extra }));
                }
            }
        }
    }
    Ok(Json(json!({ "type": "FeatureCollection", "features": feats, "catalog": n })).into_response())
}

/// A `.poly` file's rings, read whole through the pool.
fn poly_rings(pool: &IoPool, p: &std::path::Path) -> Result<Vec<Vec<[f64; 2]>>> {
    let text = String::from_utf8(pool.read_all(p).with_context(|| format!("{}", p.display()))?).with_context(|| format!("{}", p.display()))?;
    Ok(pipeline::coverage::read_poly(&text)?.into_iter().map(|r| r.into_iter().map(|q| [q[0] as f64 * 1e-7, q[1] as f64 * 1e-7]).collect()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A pool over the share `root` with a minute for each operation: none here is timed.
    fn pool(root: &std::path::Path) -> Arc<IoPool> {
        IoPool::new(2, Duration::from_secs(60), root.to_owned())
    }

    /// A pool over the share `root` that gives up on an operation after 300 ms (`hang`'s never
    /// end), and waits as long as it takes for the share to answer a probe after one: busy, not
    /// away, however slowly a busy Mac answers it.
    fn hung_pool(root: &std::path::Path) -> Arc<IoPool> {
        let cfg = store::iopool::PoolConfig::new(2, Duration::from_millis(300), root.to_owned());
        IoPool::with_config(store::iopool::PoolConfig { probe_interval: Duration::from_millis(50), alive_timeout: Duration::from_secs(600), ..cfg })
    }

    fn recipe(id: &str, name: &str) -> Recipe {
        Recipe { id: id.into(), name: name.into(), outline: vec!["osm:1877178".into()] }
    }

    fn rename(id: &str, name: &str) -> Queued {
        Queued::Edit { id: id.into(), name: Some(name.into()), outline: None }
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
        let end = Instant::now() + Duration::from_secs(300);
        // (A writer that doesn't wait opens once a reader waits, and closes at once.)
        while let Err(e) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(p) {
            assert!(Instant::now() < end, "nothing reads {}: {e}", p.display());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn edits_through_the_pool() {
        let share = tempfile::tempdir().unwrap();
        let dir = share.path().join("inputs/regions");
        let pool = pool(share.path());
        // No folder yet: no recipes, and the first one added makes it.
        assert_eq!(load(&pool, &dir).unwrap(), (vec![], vec![]));
        apply(&pool, &dir, &Queued::Add { recipe: recipe("borders", "Scottish Borders") }).unwrap();
        // Created exclusively: a taken id is a 409.
        let e = apply(&pool, &dir, &Queued::Add { recipe: recipe("borders", "Other") }).unwrap_err();
        assert_eq!(status(&e, &pool), StatusCode::CONFLICT, "{e:#}");
        // Listed as `recipes::load` lists them: other files left out, broken ones reported.
        std::fs::write(dir.join("bad.toml"), "id = \"other\"\nname = \"x\"\noutline = [\"osm:1\"]").unwrap();
        std::fs::write(dir.join("old.toml.removed"), "").unwrap();
        let (ok, bad) = load(&pool, &dir).unwrap();
        assert_eq!((ok.clone(), bad.clone()), recipes::load(&dir));
        assert_eq!((ok, bad.len()), (vec![recipe("borders", "Scottish Borders")], 1));
        // Changed through a temporary file, also over one an unfinished edit left.
        apply(&pool, &dir, &rename("borders", "The Borders")).unwrap();
        std::fs::write(dir.join(".borders.toml.tmp"), "half a recipe, and then some").unwrap();
        apply(&pool, &dir, &Queued::Edit { id: "borders".into(), name: None, outline: Some(vec!["osm:1".into()]) }).unwrap();
        assert_eq!(load(&pool, &dir).unwrap().0, [Recipe { outline: vec!["osm:1".into()], ..recipe("borders", "The Borders") }]);
        assert!(!dir.join(".borders.toml.tmp").exists());
        // In every folder: a region found by id where it lies, a plain one edited, another refused
        // ("edit the file"); old kinds of part refused.
        std::fs::create_dir_all(dir.join("eu/pt")).unwrap();
        std::fs::write(dir.join("eu/pt/azores.toml"), "id = \"azores\"\nname = \"Azores\"\noutline = [{ buffer = \"osm:1629145\", by = \"12 nmi\" }]\n").unwrap();
        std::fs::write(dir.join("eu/pt/madeira.toml"), "id = \"madeira\"\nname = \"Madeira\"\noutline = [\"osm:1\"]\n").unwrap();
        std::fs::write(dir.join("pins.toml"), "osm = \"2026-09-28\"\n").unwrap();
        let (all, _) = load_all(&pool, &dir).unwrap();
        let by: std::collections::BTreeMap<&str, &Listed> = all.iter().map(|l| (l.id.as_str(), l)).collect();
        assert_eq!((by["azores"].path.as_str(), by["azores"].plain), ("eu/pt/azores.toml", false));
        assert_eq!((by["madeira"].path.as_str(), by["madeira"].plain), ("eu/pt/madeira.toml", true));
        std::fs::write(dir.join("eu/pt/madeira.toml"), "# Madeira and Porto Santo\nid = \"madeira\"\nname = \"Madeira\"   \noutline = [\"osm:1\"]  # the islands\n").unwrap();
        apply(&pool, &dir, &rename("madeira", "Madeira Islands")).unwrap();
        let t = std::fs::read_to_string(dir.join("eu/pt/madeira.toml")).unwrap();
        assert!(t.contains("Madeira Islands") && t.starts_with("# Madeira and Porto Santo\n"), "the owner's comment kept: {t}");
        let e = apply(&pool, &dir, &rename("azores", "X")).unwrap_err();
        assert!(format!("{e:#}").contains("edit the file"), "{e:#}");
        assert!(apply(&pool, &dir, &Queued::Add { recipe: Recipe { id: "p".into(), name: "P".into(), outline: vec!["place:1,2,3".into()] } }).is_err());
        // Before the readers' switch (no records here), only osm: parts: the drop box's readers know no other.
        let e = apply(&pool, &dir, &Queued::Add { recipe: Recipe { id: "s".into(), name: "S".into(), outline: vec!["shape:s.geojson".into()] } }).unwrap_err();
        assert!(format!("{e:#}").contains("after the switch"), "{e:#}");
        assert!(apply(&pool, &dir, &Queued::Add { recipe: recipe("madeira", "again") }).is_err(), "an id in a subfolder is taken");
        apply(&pool, &dir, &Queued::Remove { id: "madeira".into() }).unwrap();
        assert!(dir.join("eu/pt/madeira.toml.removed").exists());
        // Removed: renamed to .removed, kept for undoing.
        apply(&pool, &dir, &Queued::Remove { id: "borders".into() }).unwrap();
        assert!(load(&pool, &dir).unwrap().0.is_empty());
        assert!(recipes::parse("borders.toml", &std::fs::read_to_string(dir.join("borders.toml.removed")).unwrap()).is_ok());
        // No such region: 404. An id that isn't one never makes a path: 400.
        for q in [Queued::Remove { id: "borders".into() }, rename("borders", "X")] {
            assert_eq!(status(&apply(&pool, &dir, &q).unwrap_err(), &pool), StatusCode::NOT_FOUND);
        }
        let outside = share.path().join("inputs/x.toml");
        std::fs::write(&outside, toml::to_string(&recipe("x", "X")).unwrap()).unwrap();
        for q in [Queued::Remove { id: "../x".into() }, rename("../x", "Y")] {
            assert_eq!(status(&apply(&pool, &dir, &q).unwrap_err(), &pool), StatusCode::BAD_REQUEST);
        }
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), toml::to_string(&recipe("x", "X")).unwrap());
        assert!(pool.is_online());
    }

    #[test]
    fn a_hung_share_fails_promptly() {
        let share = tempfile::tempdir().unwrap();
        let dir = share.path().join("inputs/regions");
        let (pool, hung) = (pool(share.path()), hung_pool(share.path()));
        apply(&pool, &dir, &Queued::Add { recipe: recipe("borders", "Scottish Borders") }).unwrap();
        // A recipe the share never gives: reading it fails (in the pool's time, rather than never:
        // the read itself never ends).
        let stuck = dir.join("stuck.toml");
        hang(&stuck);
        // (A watchdog: one that waited on the read would never return.)
        let promptly = |what: &str, f: Box<dyn FnOnce() -> anyhow::Error + Send>| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || tx.send(f()).ok());
            rx.recv_timeout(Duration::from_secs(300)).unwrap_or_else(|_| panic!("{what} waited on the hung read: no answer in 5 min"))
        };
        let (h, d) = (hung.clone(), dir.clone());
        let e = promptly("the listing", Box::new(move || load(&h, &d).unwrap_err()));
        assert!(nas_unreachable(&e, &hung), "{e:#}");
        let (h, d) = (hung.clone(), dir.clone());
        let e = promptly("the edit", Box::new(move || apply(&h, &d, &rename("stuck", "Stuck")).unwrap_err()));
        assert_eq!(status(&e, &hung), StatusCode::SERVICE_UNAVAILABLE, "{e:#}");
        release(&stuck);
        // The share still answers, so it's busy rather than away: the rest goes on.
        assert!(hung.is_online());
        apply(&pool, &dir, &rename("borders", "The Borders")).unwrap();
    }

    #[test]
    fn edits_made_away_wait_until_the_nas_takes_them() {
        let (share, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (dir, queue) = (share.path().join("inputs/regions"), home.path().join("regions-queue"));
        std::fs::create_dir_all(&dir).unwrap();
        let (pool, hung) = (pool(share.path()), hung_pool(share.path()));
        enqueue(&queue, &rename("borders", "The Borders")).unwrap();
        enqueue(&queue, &Queued::Add { recipe: recipe("kanto", "Kanto") }).unwrap();
        enqueue(&queue, &Queued::Add { recipe: recipe("kanto", "Kanto again") }).unwrap();
        // The share doesn't give the first edit's recipe: nothing is sent or dropped, out of order.
        let borders = dir.join("borders.toml");
        hang(&borders);
        flush_to(&hung, &dir, &queue);
        release(&borders);
        assert_eq!(queued(&queue).len(), 3);
        assert!(!dir.join("kanto.toml").exists());
        // It answers: they go in order, and the one that no longer applies (its id taken) is dropped.
        std::fs::remove_file(&borders).unwrap();
        std::fs::write(&borders, toml::to_string(&recipe("borders", "Scottish Borders")).unwrap()).unwrap();
        flush_to(&pool, &dir, &queue);
        assert!(queued(&queue).is_empty());
        assert_eq!(load(&pool, &dir).unwrap().0, [recipe("borders", "The Borders"), recipe("kanto", "Kanto")]);
    }

    #[test]
    fn a_network_error_is_the_nas_away() {
        let share = tempfile::tempdir().unwrap();
        let cfg = store::iopool::PoolConfig::new(1, Duration::from_secs(60), share.path().to_owned());
        let pool = IoPool::with_config(store::iopool::PoolConfig { probe_interval: Duration::from_secs(60), ..cfg });
        // The edit's own errors, the NAS there.
        let denied = anyhow::Error::from(IoError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        assert!(!nas_unreachable(&denied, &pool));
        assert_eq!(status(&anyhow::anyhow!("no region x"), &pool), StatusCode::NOT_FOUND);
        // A soft mount's network error takes the NAS offline: the NAS away, nothing wrong with the edit.
        let e = anyhow::Error::from(pool.call(|| -> std::io::Result<()> { Err(std::io::Error::from_raw_os_error(libc::ENOTCONN)) }).unwrap_err());
        assert!(matches!(IoError::find(&e), Some(IoError::Io(_))));
        assert_eq!(status(&e, &pool), StatusCode::SERVICE_UNAVAILABLE);
    }

    async fn json_of(r: Response) -> Value {
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    fn ids(v: &Value) -> Vec<String> {
        v["regions"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
    }

    #[tokio::test]
    async fn away_edits_wait_on_this_mac() {
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, gone) = (nas.path().join("project"), nas.path().join("gone"));
        std::fs::create_dir_all(root.join("inputs/regions")).unwrap();
        std::fs::write(root.join("inputs/regions/borders.toml"), toml::to_string(&recipe("borders", "Scottish Borders")).unwrap()).unwrap();
        let s = crate::test_state(home.path(), &root);
        let v = json_of(list(State(s.clone())).await).await;
        assert_eq!((ids(&v), v.get("offline")), (vec!["borders".to_string()], None));
        // The share goes away: edits wait on this Mac, and the list is the last one read with them.
        std::fs::rename(&root, &gone).unwrap();
        s.data.pool().unwrap().mark_offline("test");
        assert_eq!(add(State(s.clone()), Json(recipe("kanto", "Kanto"))).await.status(), StatusCode::ACCEPTED);
        assert_eq!(remove(State(s.clone()), Path("borders".into())).await.status(), StatusCode::ACCEPTED);
        let v = json_of(list(State(s.clone())).await).await;
        assert_eq!((ids(&v), v["offline"].as_bool(), v["pending"].as_u64()), (vec!["kanto".to_string()], Some(true), Some(2)));
        assert_eq!(coverage(State(s.clone())).await.status(), StatusCode::SERVICE_UNAVAILABLE);
        // Back (the pool's prober sees it): they go to the NAS.
        std::fs::rename(&gone, &root).unwrap();
        let t0 = Instant::now();
        while !s.data.online() {
            assert!(t0.elapsed() < Duration::from_secs(300), "the NAS never came back (a watchdog: the prober looks every 3 s)");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        flush(&s);
        let v = json_of(list(State(s.clone())).await).await;
        assert_eq!((ids(&v), v["pending"].as_u64()), (vec!["kanto".to_string()], Some(0)));
        assert!(root.join("inputs/regions/borders.toml.removed").exists());
    }

    fn catalog(coverage: Value) -> store::catalog::Catalog {
        let mut c = store::catalog::Catalog::new(3);
        c.coverage = coverage;
        c
    }

    #[test]
    fn coverage_as_the_catalog_recorded_it() {
        let ring = json!([[1.0, 50.0], [2.0, 50.0], [2.0, 51.0], [1.0, 50.0]]);
        let c = catalog(json!({"regions": [
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
        assert_eq!(recorded_list(&c), [json!({"id": "b", "name": "B", "outline": ["poly:b.poly", "osm:7"]}), json!({"id": "a", "name": "A", "outline": ["place:1,2,3"]})]);
        // Catalogs made before catalogs recorded their coverage: none (then it's the recipes').
        for old in [json!({"regions": []}), Value::Null, json!({"regions": ["northumberland"]})] {
            assert!(recorded(&catalog(old.clone())).is_empty() && recorded_list(&catalog(old)).is_empty());
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
