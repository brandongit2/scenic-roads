//! Regions and the areas they're made of (docs/surface.md, the Regions panel; docs/regions.md).
//!
//!   GET    /api/regions             the region files in inputs/regions on the NAS, in every folder,
//!                                   each with its path and its state in the input check (accepted,
//!                                   checking, held with the findings), the files that don't parse,
//!                                   and the folders' tree (docs/inputs.md §6.1.9); from the listing
//!                                   kept on this Mac (`Listing`), brought up to date every minute
//!   GET    /api/regions/outlines    every region's outline simplified for a zoom (`z`; `bbox`, `ids`
//!                                   narrow it), a feature per region, made once per zoom and version
//!                                   and kept on this Mac (made for every zoom when the version
//!                                   changes); from the accepted copy kept on this Mac, else the catalogue's
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
    extract::State,
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
    /// Its land and water (km²) and its country, from its accepted geometry (`Sized`); none until
    /// this Mac has worked them out, or for a region the accepted version doesn't have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land_km2: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_km2: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub country: String,
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
            land_km2: None,
            water_km2: None,
            country: String::new(),
        }
    }
}

/// The region files and the files that don't parse (path, problem), as the panel lists them.
type ListedAll = (Vec<Listed>, Vec<(String, String)>);

/// The inputs folder's region files as this Mac last listed them (`accepted/regions/@listed.json`,
/// beside the accepted copy; docs/formats.md): each file by path with the size and modification time
/// it had when read, and what it read as (with its state in the input check). /api/regions answers
/// from it; the regions thread brings it up to date every minute, reading only the files whose size
/// or time changed (SMB reads ~1,000 small files in about 30 s).
#[derive(serde::Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Listing {
    files: std::collections::BTreeMap<String, Seen>,
}

#[derive(serde::Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Seen {
    len: u64,
    /// Nanoseconds since the epoch (None: the share gave none, so the file is read each time).
    modified: Option<u64>,
    read: Result<Listed, String>,
}

impl Listing {
    /// The region files by id, and the files that don't parse (path, problem).
    fn split(self) -> ListedAll {
        let (mut ok, mut bad) = (Vec::new(), Vec::new());
        for (path, f) in self.files {
            match f.read {
                Ok(l) => ok.push(l),
                Err(e) => bad.push((path, e)),
            }
        }
        ok.sort_by(|a, b| a.id.cmp(&b.id));
        (ok, bad)
    }
}

fn listing_path(s: &crate::AppState) -> std::path::PathBuf {
    accepted_dir(s).join("@listed.json")
}

fn read_listing(s: &crate::AppState) -> Option<Listing> {
    serde_json::from_slice(&std::fs::read(listing_path(s)).ok()?).ok()
}

/// One listing made at a time (the regions thread, a first /api/regions on a fresh home): the
/// other waits and takes it.
static LISTING: Mutex<()> = Mutex::new(());

/// The listing brought up to date from the NAS and kept (written only when it changed); None while
/// the NAS is away. An error (the NAS failing midway) leaves the kept one as it was. `afresh`: every
/// file read again, whatever its size and time (a file rewritten with both kept, as `cp -p` or
/// `rsync -t` do, is otherwise missed; the regions thread does it hourly).
fn refresh_listing(s: &crate::AppState, afresh: bool) -> Result<Option<Listing>> {
    let Some((dir, pool)) = nas(s) else { return Ok(None) };
    let _one = LISTING.lock().unwrap_or_else(|e| e.into_inner());
    let prev = if afresh { Listing::default() } else { read_listing(s).unwrap_or_default() };
    let mut l = load_all(&pool, &dir, &prev)?;
    states(s, &pool, &mut l);
    if l != prev {
        std::fs::create_dir_all(accepted_dir(s))?;
        store::whole::write(&listing_path(s), &serde_json::to_vec(&l)?)?;
    }
    Ok(Some(l))
}

/// Every region file in the folder and its subfolders (`how/`, `todo/` and the names every inputs folder
/// passes over aside, and the special files), by path; those `prev` saw with the same size and time
/// taken from it, the rest read. The NAS failing midway is an error, never a shorter list.
fn load_all(pool: &IoPool, dir: &store::nasfs::NasPath, prev: &Listing) -> Result<Listing> {
    let mut out = Listing::default();
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
            let modified =
                it.modified.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64);
            if let Some(seen) =
                prev.files.get(&path).filter(|f| f.len == it.len && f.modified.is_some() && f.modified == modified)
            {
                out.files.insert(path, seen.clone());
                continue;
            }
            let p = d.join(&it.name);
            let text = match pool.read_all(&p) {
                Ok(b) => String::from_utf8(b).map_err(anyhow::Error::from),
                Err(IoError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(IoError::Io(e)) if pool.is_online() => Err(e.into()),
                Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
            };
            let read = match text.and_then(|t| {
                pipeline::inputs::regions::file::parse(&it.name, &t)
                    .map_err(|fs| anyhow::anyhow!(fs.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; ")))
            }) {
                Ok(r) => Ok(Listed::of(&r, path.clone())),
                Err(e) => Err(format!("{e:#}")),
            };
            out.files.insert(path, Seen { len: it.len, modified, read });
        }
    }
    Ok(out)
}

/// Every minute: the listing of the region files (each file read again hourly, whatever its size
/// and time), then the regions' accepted copy (for the layer),
/// brought up to date, and the outlines made for every zoom when their version changed; and in a
/// thread of its own (a fresh home's first pass reads every geometry, ~20 min on the M1, which would
/// hold the rest back that long), the land and water of the regions not sized yet.
pub fn spawn_syncer(s: S) {
    let s2 = s.clone();
    std::thread::Builder::new()
        .name("regions".into())
        .spawn(move || {
            for pass in 0u64.. {
                if let Err(e) = refresh_listing(&s, pass % 60 == 59) {
                    eprintln!("regions: the listing wasn't brought up to date: {e:#}");
                }
                sync_accepted(&s);
                s.outlines.make_all(&s);
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        })
        .ok();
    std::thread::Builder::new()
        .name("region-areas".into())
        .spawn(move || loop {
            size_regions(&s2);
            std::thread::sleep(std::time::Duration::from_secs(60));
        })
        .ok();
}

/// The region files from the listing kept on this Mac (`offline` while the NAS is away: the last
/// listing). With none kept yet (a fresh home), the listing is made now; the NAS not answering then
/// is a 503, as for any read it can't answer.
pub async fn list(State(s): State<S>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || {
        let mut kept = read_listing(&s2);
        if kept.is_none() {
            kept = match refresh_listing(&s2, false) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("regions: {e:#}");
                    return err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}"));
                }
            };
        }
        let (mut ok, bad) = kept.unwrap_or_default().split();
        sizes_into(&s2, &mut ok);
        let t = tree(&s2, &ok);
        let mut v = json!({ "regions": ok, "bad": bad, "tree": t });
        if nas(&s2).is_none() {
            v["offline"] = true.into();
        }
        Json(v).into_response()
    })
    .await
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ---- each region's land and water ---------------------------------------------------------------

/// A region's land and water (km², on the authalic sphere: `geom::area_km2`) and its country (its
/// first land shape's ISO 3166-1 code, else its first shape's; "" when none is known), from its
/// stored geometry (pipeline::inputs::regions::stored: water shapes are those marked `water`, the
/// rest land).
#[derive(serde::Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Sized {
    pub land: f64,
    pub water: f64,
    #[serde(default)]
    pub country: String,
}

pub fn sized(g: &pipeline::inputs::regions::stored::Geometry) -> Sized {
    use pipeline::inputs::regions::geom::{area_km2, Area};
    let (mut land, mut water) = (0.0, 0.0);
    for sh in &g.shapes {
        let rings: Vec<Vec<[i64; 2]>> =
            sh.rings.iter().map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect()).collect();
        let a = area_km2(&Area::from_rings(&rings)).abs();
        if sh.water {
            water += a;
        } else {
            land += a;
        }
    }
    let country = g
        .shapes
        .iter()
        .filter(|s| !s.water)
        .chain(g.shapes.iter())
        .map(|s| s.country.clone())
        .find(|c| !c.is_empty())
        .unwrap_or_default();
    Sized { land: land.round(), water: water.round(), country }
}

/// Where this Mac keeps the regions' sizes, by geometry content name (a name is its content: a
/// size never goes stale, only unused).
fn sizes_path(s: &crate::AppState) -> std::path::PathBuf {
    s.data.home.join("region-areas.json")
}

fn read_sizes(s: &crate::AppState) -> std::collections::BTreeMap<String, Sized> {
    std::fs::read(sizes_path(s)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The accepted version kept on this Mac: each region file's path and its geometry's content name.
fn geometry_names(s: &crate::AppState) -> std::collections::BTreeMap<String, String> {
    let kept = pipeline::inputs::served::kept(&accepted_dir(s));
    kept.map(|k| k.index.files)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(p, e)| Some((p, e.facts.get("geometry")?.as_str()?.to_string())))
        .collect()
}

/// The listed regions' sizes, where this Mac has worked them out.
fn sizes_into(s: &crate::AppState, list: &mut [Listed]) {
    let (names, sizes) = (geometry_names(s), read_sizes(s));
    for r in list.iter_mut() {
        if let Some(z) = names.get(&r.path).and_then(|g| sizes.get(g)) {
            r.land_km2 = Some(z.land);
            r.water_km2 = Some(z.water);
            r.country = z.country.clone();
        }
    }
}

/// The sizes of the accepted regions this Mac hasn't sized yet, each geometry read once from the NAS
/// through the pool (the region-areas thread); those no
/// region names any more dropped. Written out every 50, so a restart keeps what was done.
fn size_regions(s: &crate::AppState) {
    let (Some(root), Some(pool)) = (s.data.nas_root(), s.data.pool()) else { return };
    let names = geometry_names(s);
    let mut sizes = read_sizes(s);
    let used: std::collections::BTreeSet<&String> = names.values().collect();
    let before = sizes.len();
    sizes.retain(|g, _| used.contains(g));
    let mut dirty = sizes.len() != before;
    let write = |sizes: &std::collections::BTreeMap<String, Sized>| {
        // (Through a temporary name unique to the call: two servers may share a home.)
        if let Ok(b) = serde_json::to_vec(sizes) {
            store::whole::write(&sizes_path(s), &b).ok();
        }
    };
    let mut n = 0;
    for g in used {
        if sizes.contains_key(g) || !pool.is_online() {
            continue;
        }
        let read = store::PooledFile::new(root.join(g), pool.clone()).read_all();
        match read.map_err(anyhow::Error::from).and_then(|b| pipeline::inputs::regions::stored::Geometry::decode(&b)) {
            Ok(geo) => {
                sizes.insert(g.clone(), sized(&geo));
                dirty = true;
                n += 1;
                if n % 50 == 0 {
                    write(&sizes);
                }
            }
            Err(e) => eprintln!("regions: {g} not sized: {e:#}"),
        }
    }
    if dirty {
        write(&sizes);
    }
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
/// name and parts as the accepted version has them), or checking; as they were when the records
/// can't be read now.
fn states(s: &crate::AppState, pool: &IoPool, list: &mut Listing) {
    let Some((index, held)) = records(s, pool) else { return };
    for r in list.files.values_mut().filter_map(|f| f.read.as_mut().ok()) {
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
    let kept = pipeline::inputs::served::kept(&accepted_dir(s));
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
    let read =
        |p: &str| -> anyhow::Result<Vec<u8>> { Ok(store::PooledFile::new(root.join(p), pool.clone()).read_all()?) };
    if let Err(e) =
        pipeline::inputs::served::sync_with(&read, pipeline::inputs::regions::UNIT, &accepted_dir(s), &["drawn"])
    {
        eprintln!("regions: the accepted copy wasn't brought up to date: {e:#}");
    }
}

/// Whether region `id` is disabled (`disabled = true`): by the accepted copy kept on this Mac, else
/// the listing of the inputs folder.
pub fn is_disabled(s: &crate::AppState, id: &str) -> bool {
    let dir = accepted_dir(s);
    let kept = pipeline::inputs::served::kept(&dir);
    if let Some(k) = kept {
        return k.index.files.values().any(|e| {
            e.facts.get("id").and_then(Value::as_str) == Some(id)
                && e.facts.get("disabled").and_then(Value::as_bool) == Some(true)
        });
    }
    read_listing(s).is_some_and(|l| l.files.values().any(|f| f.read.as_ref().is_ok_and(|l| l.id == id && l.disabled)))
}

/// The deepest zoom the outlines are simplified for: past it, z12's (about 10 m) are drawn.
pub const OUTLINE_MAX_Z: u8 = 12;

/// The simplification at zoom `z` (metres): about half a pixel at the equator (MapLibre's 512 px
/// tiles: 40 075 km over 512 × 2^z px).
fn tolerance_m(z: u8) -> f64 {
    0.5 * 40_075_016.7 / (512.0 * f64::from(1u32 << z.min(OUTLINE_MAX_Z)))
}

/// One region's outline at one zoom, ready to send: its id and box, its polygons and properties
/// (to clip to a box), and its GeoJSON feature as text.
struct Simplified {
    id: String,
    bbox: [f64; 4],
    polys: Vec<Vec<Vec<[f64; 2]>>>,
    props: Value,
    json: String,
}

/// The regions' outlines simplified per zoom, made once per version of their source (the accepted
/// version kept on this Mac, else the catalogue's coverage) and of the pass's outlines (the flags),
/// and kept: in memory for the zooms asked, and on this Mac's disk for every zoom
/// (`accepted/regions/@outlines/z<z>.jsonl`), made by the regions thread when the version changes, so
/// a first ask after a restart reads the zoom's file: `/api/regions/outlines`.
#[derive(Default)]
pub struct Outlines {
    cur: Mutex<(String, std::collections::HashMap<u8, Arc<Vec<Simplified>>>)>,
    /// Held while one zoom is made: asks arriving meanwhile wait and take it, rather than each
    /// reading every drawn file and simplifying the world again.
    making: Mutex<()>,
    /// Folders' outlines made (`union_of`), by the outlines' key, the zoom and the regions' ids:
    /// the last few, a folder hovered again drawn at once.
    unions: Mutex<Vec<(String, Arc<Simplified>)>>,
}

/// The deepest zoom a folder's outline is made for (`Outlines::union`): about 1.2 km, a
/// folder being an overview; deeper zooms draw it. Canada's, its waters round the Arctic
/// islands, takes about 1 s here, 4 s at z6 and 20 s at z8.
const UNION_MAX_Z: u8 = 5;

/// How many folders' outlines are kept (`Outlines::unions`).
const UNIONS_KEPT: usize = 32;

/// A region's outline as read from its source, before simplifying: every shape's polygons.
struct Source {
    id: String,
    name: String,
    disabled: bool,
    /// Its first osm: entry's relation (its area gives the panel's flag).
    first_osm: Option<u64>,
    polygons: Vec<Vec<Vec<[f64; 2]>>>,
}

/// A region's first osm: entry's relation.
fn first_osm(outline: &[String]) -> Option<u64> {
    outline.iter().find_map(|e| e.strip_prefix("osm:").and_then(|i| i.parse().ok()))
}

/// The sources, and whether every one was read (a sync that removed a drawn file the index read
/// named leaves them short: those aren't kept on disk under the key).
type MakeSources<'a> = Box<dyn FnOnce() -> (Vec<Source>, bool) + 'a>;

/// What the outlines are made from (the accepted copy kept on this Mac, else the catalogue's
/// coverage), and a key that changes with it: the key is the version of the very index the sources
/// are read from (one read), so a sync between the two never puts one version's outlines under
/// another's key.
fn sources(s: &crate::AppState) -> (String, MakeSources<'_>) {
    let dir = accepted_dir(s);
    let kept = pipeline::inputs::served::kept(&dir);
    if let Some(kept) = kept {
        let key = format!("accepted {}", kept.version);
        let make = move || {
            let mut out = Vec::new();
            let mut whole = true;
            for e in kept.index.files.into_values() {
                let Some(drawn) = e.facts.get("drawn").and_then(Value::as_str) else { continue };
                let disabled = e.facts.get("disabled").and_then(Value::as_bool).unwrap_or(false);
                let Some(d) = std::fs::read(dir.join(drawn))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<pipeline::inputs::regions::stored::Drawn>(&b).ok())
                else {
                    whole = false;
                    continue;
                };
                out.push(Source {
                    first_osm: first_osm(&d.outline),
                    polygons: d.shapes.into_values().flatten().collect(),
                    id: d.id,
                    name: d.name,
                    disabled,
                });
            }
            (out, whole)
        };
        return (key, Box::new(make));
    }
    let cat = s.data.catalogue();
    let key = format!("catalogue {}", cat.n);
    let make = move || {
        let v = recorded(&cat)
            .into_iter()
            .map(|r| Source {
                first_osm: first_osm(&r.outline),
                polygons: r.shapes.into_values().flatten().collect(),
                id: r.id,
                name: r.name,
                disabled: false,
            })
            .collect::<Vec<_>>();
        (v, true)
    };
    (key, Box::new(make))
}

/// A ring's area in square degrees (shoelace; its sign the ring's turn).
fn ring_area(r: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..r.len() {
        let (p, q) = (r[i], r[(i + 1) % r.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a / 2.0
}

/// A point inside a polygon (its outer ring, then its holes) to put its name on: the middle of the
/// widest span inside it along the parallel through its outer ring's centroid, or through the
/// middle of its box, whichever is wider.
fn label_point(poly: &[Vec<[f64; 2]>]) -> Option<[f64; 2]> {
    let outer = poly.first()?;
    let (mut s, mut n) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in outer {
        s = s.min(p[1]);
        n = n.max(p[1]);
    }
    let a = ring_area(outer);
    let cy = if a.abs() > 0.0 {
        let mut c = 0.0;
        for i in 0..outer.len() {
            let (p, q) = (outer[i], outer[(i + 1) % outer.len()]);
            c += (p[1] + q[1]) * (p[0] * q[1] - q[0] * p[1]);
        }
        c / (6.0 * a)
    } else {
        (s + n) / 2.0
    };
    let widest = |y: f64| -> Option<([f64; 2], f64)> {
        let mut xs: Vec<f64> = Vec::new();
        for r in poly {
            for i in 0..r.len() {
                let (p, q) = (r[i], r[(i + 1) % r.len()]);
                if (p[1] > y) != (q[1] > y) {
                    xs.push(p[0] + (y - p[1]) / (q[1] - p[1]) * (q[0] - p[0]));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        xs.chunks_exact(2).map(|c| ([(c[0] + c[1]) / 2.0, y], c[1] - c[0])).max_by(|a, b| a.1.total_cmp(&b.1))
    };
    [cy, (s + n) / 2.0].into_iter().filter_map(widest).max_by(|a, b| a.1.total_cmp(&b.1)).map(|w| w.0)
}

/// The rounding of degrees simplified to `tol_m` metres: to a tenth of it (fewer digits to send),
/// as 10^digits.
fn rounding(tol_m: f64) -> f64 {
    10f64.powi(((111_320.0 / tol_m).log10() + 1.0).ceil().clamp(1.0, 7.0) as i32)
}

/// A ring simplified to `tol_m` metres (pipeline::outlines::simplify, on its 1e-7° grid), its
/// degrees rounded to a tenth of that (fewer digits to send).
fn simplify_ring(r: &[[f64; 2]], tol_m: f64) -> Vec<[f64; 2]> {
    let grid: Vec<[i32; 2]> = r.iter().map(|p| [(p[0] * 1e7).round() as i32, (p[1] * 1e7).round() as i32]).collect();
    let k = rounding(tol_m);
    let round = |v: i32| (f64::from(v) * 1e-7 * k).round() / k;
    pipeline::outlines::simplify(&grid, tol_m).into_iter().map(|p| [round(p[0]), round(p[1])]).collect()
}

/// A ring clipped to a box (Sutherland–Hodgman): what of it lies inside, closed, its cuts along the
/// box's edges (none when nothing is left).
fn clip_ring(r: &[[f64; 2]], b: [f64; 4]) -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = r.to_vec();
    if pts.first() == pts.last() {
        pts.pop();
    }
    // Each edge of the box: inside, and where a segment crosses it.
    for edge in 0..4 {
        if pts.is_empty() {
            break;
        }
        let ins = |p: &[f64; 2]| match edge {
            0 => p[0] >= b[0],
            1 => p[0] <= b[2],
            2 => p[1] >= b[1],
            _ => p[1] <= b[3],
        };
        let cut = |p: [f64; 2], q: [f64; 2]| -> [f64; 2] {
            let (i, v) = match edge {
                0 => (0, b[0]),
                1 => (0, b[2]),
                2 => (1, b[1]),
                _ => (1, b[3]),
            };
            let t = (v - p[i]) / (q[i] - p[i]);
            let mut o = [p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])];
            o[i] = v;
            o
        };
        let mut out = Vec::with_capacity(pts.len());
        for k in 0..pts.len() {
            let (p, q) = (pts[(k + pts.len() - 1) % pts.len()], pts[k]);
            match (ins(&p), ins(&q)) {
                (true, true) => out.push(q),
                (true, false) => out.push(cut(p, q)),
                (false, true) => {
                    out.push(cut(p, q));
                    out.push(q);
                }
                (false, false) => {}
            }
        }
        pts = out;
    }
    if pts.len() < 3 {
        return Vec::new();
    }
    pts.push(pts[0]);
    pts
}

/// One region's outline at a tolerance: rings smaller than about the tolerance across left out
/// (but its largest polygon), its name on its largest polygon.
fn simplified(src: &Source, tol_m: f64, ix: Option<&OutlineIndex>) -> Simplified {
    // (Square degrees of a square the tolerance across, at the equator: smaller elsewhere.)
    let tiny = (tol_m / 111_320.0).powi(2);
    let outer_area = |p: &Vec<Vec<[f64; 2]>>| p.first().map_or(0.0, |r| ring_area(r).abs());
    let largest =
        src.polygons.iter().enumerate().max_by(|a, b| outer_area(a.1).total_cmp(&outer_area(b.1))).map(|l| l.0);
    let label = largest.and_then(|k| label_point(&src.polygons[k]));
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut polys: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
    for (k, p) in src.polygons.iter().enumerate() {
        for q in p.first().into_iter().flatten() {
            bbox = [bbox[0].min(q[0]), bbox[1].min(q[1]), bbox[2].max(q[0]), bbox[3].max(q[1])];
        }
        if Some(k) != largest && outer_area(p) < tiny {
            continue;
        }
        let rings = p
            .iter()
            .enumerate()
            .filter(|(i, r)| *i == 0 || ring_area(r).abs() >= tiny)
            .map(|(_, r)| simplify_ring(r, tol_m))
            .collect();
        polys.push(rings);
    }
    let area = src
        .first_osm
        .and_then(|i| ix.and_then(|ix| ix.by_id(i).map(|o| ix.summary(o))))
        .map(|a| json!({ "iso": a["iso"], "country": a["country"], "in": a["in"] }));
    let props = json!({ "region": src.id, "name": src.name, "disabled": src.disabled, "bbox": bbox, "label": label, "area": area });
    let json =
        json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": polys }, "properties": props })
            .to_string();
    Simplified { id: src.id.clone(), bbox, polys, props, json }
}

/// What the kept outlines are made with: changed when `simplified` makes them otherwise.
const OUTLINES_FMT: u32 = 1;

/// The outlines' key: the format, the source's key, and the pass's outlines that give the flags.
fn outlines_key(source: &str, ix: Option<&OutlineIndex>) -> String {
    format!("{OUTLINES_FMT} {source} {}", ix.map_or("-", |i| i.content.as_str()))
}

fn zoom_path(s: &crate::AppState, z: u8) -> std::path::PathBuf {
    accepted_dir(s).join("@outlines").join(format!("z{z}.jsonl"))
}

/// A zoom's outlines kept on disk, when kept for `key`: its key on the first line, then a feature a line.
fn read_zoom(path: &std::path::Path, key: &str) -> Option<Vec<Simplified>> {
    use std::io::BufRead;
    let mut lines = std::io::BufReader::new(std::fs::File::open(path).ok()?).lines();
    if lines.next()?.ok()? != key {
        return None;
    }
    let mut out = Vec::new();
    for l in lines {
        let json = l.ok()?;
        let mut f: Value = serde_json::from_str(&json).ok()?;
        let props = f["properties"].take();
        let polys = serde_json::from_value(f["geometry"]["coordinates"].take()).ok()?;
        let bbox = serde_json::from_value(props["bbox"].clone()).ok()?;
        out.push(Simplified { id: props["region"].as_str()?.to_string(), bbox, polys, props, json });
    }
    Some(out)
}

/// Whether `path` keeps a zoom for `key` (its first line only).
fn zoom_kept(path: &std::path::Path, key: &str) -> bool {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(path) else { return false };
    let mut first = String::new();
    std::io::BufReader::new(f).read_line(&mut first).is_ok() && first.trim_end_matches('\n') == key
}

/// Keeps a zoom's outlines on disk for `key` (one local write; failures said and passed over).
fn write_zoom(path: &std::path::Path, key: &str, v: &[Simplified]) {
    let mut b = String::with_capacity(v.iter().map(|r| r.json.len() + 1).sum::<usize>() + key.len() + 1);
    b.push_str(key);
    for r in v {
        b.push('\n');
        b.push_str(&r.json);
    }
    let w = path.parent().map_or(Ok(()), std::fs::create_dir_all).map_err(anyhow::Error::from);
    if let Err(e) = w.and_then(|_| store::whole::write(path, b.as_bytes())) {
        eprintln!("regions: the outlines weren't kept: {e:#}");
    }
}

impl Outlines {
    /// Every region's outline at zoom `z`: kept in memory, else read from disk, else made (and kept).
    fn at(&self, s: &crate::AppState, z: u8) -> Arc<Vec<Simplified>> {
        let (source, make) = sources(s);
        let ix = s.areas.get(s).ok().flatten();
        let key = outlines_key(&source, ix.as_deref());
        let cached = || {
            let g = self.cur.lock().unwrap();
            if g.0 == key {
                g.1.get(&z).cloned()
            } else {
                None
            }
        };
        if let Some(v) = cached() {
            return v;
        }
        let _making = self.making.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = cached() {
            return v;
        }
        let path = zoom_path(s, z);
        let v = Arc::new(read_zoom(&path, &key).unwrap_or_else(|| {
            let (srcs, whole) = make();
            let v: Vec<Simplified> = srcs.iter().map(|r| simplified(r, tolerance_m(z), ix.as_deref())).collect();
            if whole {
                write_zoom(&path, &key, &v);
            }
            v
        }));
        let mut g = self.cur.lock().unwrap();
        if g.0 != key {
            *g = (key, Default::default());
        }
        g.1.insert(z, v.clone());
        v
    }

    /// The outline of the regions `ids` as one at zoom `z` (UNION_MAX_Z at most; a folder's:
    /// `union_of`), made once and kept for the version (the last UNIONS_KEPT); None when none of
    /// them is drawn.
    fn union(
        &self,
        s: &crate::AppState,
        z: u8,
        ids: &std::collections::HashSet<&str>,
    ) -> Result<Option<Arc<Simplified>>> {
        let z = z.min(UNION_MAX_Z);
        let all = self.at(s, z);
        let mut sorted: Vec<&str> = ids.iter().copied().collect();
        sorted.sort_unstable();
        let key = format!("{} {z} {}", self.cur.lock().unwrap().0, sorted.join(","));
        if let Some((_, u)) = self.unions.lock().unwrap().iter().find(|u| u.0 == key) {
            return Ok(Some(u.clone()));
        }
        let parts: Vec<&Simplified> = all.iter().filter(|r| ids.contains(r.id.as_str())).collect();
        if parts.is_empty() {
            return Ok(None);
        }
        let u = Arc::new(union_of(&parts, tolerance_m(z))?);
        let mut kept = self.unions.lock().unwrap();
        if kept.len() >= UNIONS_KEPT {
            kept.remove(0);
        }
        kept.push((key, u.clone()));
        Ok(Some(u))
    }

    /// Every zoom's outlines kept on disk for the current version: those not kept yet made, the
    /// sources read once (the regions thread, after a sync).
    fn make_all(&self, s: &crate::AppState) {
        let (source, make) = sources(s);
        let ix = s.areas.get(s).ok().flatten();
        let key = outlines_key(&source, ix.as_deref());
        let mut make = Some(make);
        let (mut srcs, mut whole) = (Vec::new(), true);
        for z in 0..=OUTLINE_MAX_Z {
            let _making = self.making.lock().unwrap_or_else(|e| e.into_inner());
            let path = zoom_path(s, z);
            if zoom_kept(&path, &key) {
                continue;
            }
            if let Some(m) = make.take() {
                (srcs, whole) = m();
            }
            if !whole {
                return;
            }
            let v: Vec<Simplified> = srcs.iter().map(|r| simplified(r, tolerance_m(z), ix.as_deref())).collect();
            write_zoom(&path, &key, &v);
        }
    }
}

/// Some regions' outlines at a tolerance as one (a folder's, hovered in the panel): their union,
/// with no line between regions that touch. Their outer rings are unioned, and a hole of one is
/// kept only where none of the others covers it (a country's waters hold its provinces as holes:
/// filled; a folder round an enclave keeps it): the waters' rings round every coast never meet
/// the overlay, which made Canada's take minutes. Each region was simplified on its own, so
/// neighbours' shared borders no longer quite meet: the union is snapped to a grid of half the
/// tolerance and closed by the tolerance (grown by it, then shrunk: slivers up to about twice it
/// wide filled), then simplified to it again.
fn union_of(parts: &[&Simplified], tol_m: f64) -> Result<Simplified> {
    use geos::{CapStyle, CoordSeq, Geom, Geometry, JoinStyle, Precision};
    let ring = |r: &[[f64; 2]]| -> Result<Option<Geometry>> {
        let mut v: Vec<Vec<f64>> = r.iter().map(|p| vec![p[0], p[1]]).collect();
        if v.first() != v.last() {
            v.push(v[0].clone());
        }
        if v.len() < 4 {
            return Ok(None);
        }
        Ok(Some(Geometry::create_polygon(Geometry::create_linear_ring(CoordSeq::new_from_vec(&v)?)?, vec![])?))
    };
    let deg = tol_m / 111_320.0;
    let grid = deg / 2.0;
    // Each region's outer rings as polygons (made valid: a ring simplified may touch itself),
    // and its holes.
    let mut shells: Vec<(usize, Geometry)> = Vec::new();
    let mut holes: Vec<(usize, Geometry)> = Vec::new();
    for (k, r) in parts.iter().enumerate() {
        for p in &r.polys {
            for (i, rr) in p.iter().enumerate() {
                if let Some(g) = ring(rr)? {
                    let g = g.buffer(0.0, 2)?;
                    if i == 0 {
                        shells.push((k, g))
                    } else {
                        holes.push((k, g))
                    }
                }
            }
        }
    }
    // A hole stays when no other region's outer rings hold a point inside it.
    let mut kept = Vec::new();
    for (k, h) in holes {
        if h.is_empty()? {
            continue;
        }
        let pt = h.point_on_surface()?;
        if !shells.iter().any(|(j, s)| *j != k && s.contains(&pt).unwrap_or(false)) {
            kept.push(h);
        }
    }
    let snap = |gs: Vec<Geometry>| -> Result<Geometry> {
        let gs =
            gs.into_iter().map(|g| g.set_precision(grid, Precision::ValidOutput)).collect::<Result<Vec<_>, _>>()?;
        Ok(Geometry::create_geometry_collection(gs)?.unary_union_prec(grid)?)
    };
    let mut whole = snap(shells.into_iter().map(|s| s.1).collect())?;
    if !kept.is_empty() {
        whole = whole.difference_prec(&snap(kept)?, grid)?;
    }
    let closed = whole
        .buffer_with_style(deg, 2, CapStyle::Round, JoinStyle::Mitre, 2.0)?
        .buffer_with_style(-deg, 2, CapStyle::Round, JoinStyle::Mitre, 2.0)?
        .topology_preserve_simplify(deg)?;
    // (Degrees rounded as simplify_ring's are.)
    let k = rounding(tol_m);
    let round = |v: f64| (v * k).round() / k;
    let coords = |g: &geos::ConstGeometry| -> Result<Vec<[f64; 2]>> {
        let s = g.get_coord_seq()?;
        // (A ring cut at the antimeridian moved off it by the closing: put back on it, where the map
        // knows it for a cut, not a border: outlineLines.)
        let x = |v: f64| if v.abs() >= 180.0 - 2.0 * deg { 180f64.copysign(v) } else { round(v) };
        (0..s.size()?).map(|i| Ok([x(s.get_x(i)?), round(s.get_y(i)?)])).collect()
    };
    let mut out: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
    for i in 0..closed.get_num_geometries()? {
        let p = closed.get_geometry_n(i)?;
        if p.geometry_type()? != geos::GeometryTypes::Polygon || p.is_empty()? {
            continue;
        }
        let mut rings = vec![coords(&p.get_exterior_ring()?)?];
        for k in 0..p.get_num_interior_rings()? {
            rings.push(coords(&p.get_interior_ring_n(k)?)?);
        }
        out.push(rings);
    }
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for r in parts {
        bbox = [bbox[0].min(r.bbox[0]), bbox[1].min(r.bbox[1]), bbox[2].max(r.bbox[2]), bbox[3].max(r.bbox[3])];
    }
    let ids: Vec<&str> = parts.iter().map(|r| r.id.as_str()).collect();
    let props =
        json!({ "region": ids.join(","), "name": "", "disabled": false, "bbox": bbox, "label": null, "area": null });
    let json =
        json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": out }, "properties": props })
            .to_string();
    Ok(Simplified { id: ids.join(","), bbox, polys: out, props, json })
}

/// What `/api/regions/outlines` is asked: `z` (the map's zoom; 0–12, deeper as 12), optionally
/// `bbox=w,s,e,n` (only the regions whose box meets it), `ids=a,b` (only those regions) and
/// `union=1` (those regions as one outline: `union_of`).
#[derive(Deserialize)]
pub struct OutlinesQuery {
    z: Option<f64>,
    bbox: Option<String>,
    ids: Option<String>,
    union: Option<u8>,
}

/// The features of `all` that `bbox` and `ids` keep, as a GeoJSON FeatureCollection's text; with a
/// box, each outline reaching out of it clipped to it (`clip`: the cuts lie along its edges, which
/// the map leaves undrawn).
fn collection(
    all: &[Simplified],
    z: u8,
    bbox: Option<[f64; 4]>,
    ids: Option<&std::collections::HashSet<&str>>,
) -> String {
    let mut body = String::from("{\"type\":\"FeatureCollection\",\"features\":[");
    let mut first = true;
    for r in all {
        if ids.is_some_and(|i| !i.contains(r.id.as_str())) {
            continue;
        }
        if bbox.is_some_and(|b| r.bbox[0] > b[2] || r.bbox[2] < b[0] || r.bbox[1] > b[3] || r.bbox[3] < b[1]) {
            continue;
        }
        if !first {
            body.push(',');
        }
        first = false;
        match bbox {
            Some(b) if r.bbox[0] < b[0] || r.bbox[1] < b[1] || r.bbox[2] > b[2] || r.bbox[3] > b[3] => {
                let polys: Vec<Vec<Vec<[f64; 2]>>> = r
                    .polys
                    .iter()
                    .filter_map(|p| {
                        let rings: Vec<Vec<[f64; 2]>> =
                            p.iter().map(|ring| clip_ring(ring, b)).filter(|c| !c.is_empty()).collect();
                        // (A hole left without its outer ring isn't one.)
                        (!rings.is_empty() && !clip_ring(&p[0], b).is_empty()).then_some(rings)
                    })
                    .collect();
                let f = json!({ "type": "Feature", "geometry": { "type": "MultiPolygon", "coordinates": polys }, "properties": r.props });
                body.push_str(&f.to_string());
            }
            _ => body.push_str(&r.json),
        }
    }
    let clip = bbox.map_or("null".to_string(), |b| json!(b).to_string());
    body.push_str(&format!("],\"z\":{z},\"clip\":{clip}}}"));
    body
}

/// The regions' outlines for drawing at a zoom (docs/formats.md): a feature per region, its every
/// shape in one MultiPolygon simplified to about half a pixel there (rings smaller than that left
/// out), with properties region, name, disabled, bbox, label (a point inside it for its name) and
/// area (its first osm: entry's iso, country and in, for its flag). From the accepted copy kept on
/// this Mac, else the catalogue's coverage; made once per zoom and version, and kept. With
/// `union=1`, the regions `ids` names as one feature (`union_of`; its region their ids, no name).
pub async fn outlines(State(s): State<S>, axum::extract::Query(q): axum::extract::Query<OutlinesQuery>) -> Response {
    let s2 = s.clone();
    tokio::task::spawn_blocking(move || -> Result<Response> {
        let z = q.z.unwrap_or(0.0).clamp(0.0, f64::from(OUTLINE_MAX_Z)) as u8;
        let bbox: Option<[f64; 4]> = q.bbox.as_deref().and_then(|b| {
            let v: Vec<f64> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            <[f64; 4]>::try_from(v).ok()
        });
        let ids: Option<std::collections::HashSet<&str>> = q.ids.as_deref().map(|i| i.split(',').collect());
        let body = match (&ids, q.union) {
            (Some(ids), Some(1)) => {
                let u = s2.outlines.union(&s2, z, ids)?;
                collection(u.as_deref().map(std::slice::from_ref).unwrap_or(&[]), z, bbox, None)
            }
            _ => collection(&s2.outlines.at(&s2, z), z, bbox, ids.as_ref()),
        };
        Ok(([(axum::http::header::CONTENT_TYPE, "application/json")], body).into_response())
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

/// The regions a catalogue records, without their outlines (id, name and outline entries), read straight from its coverage: /api/catalog asks
/// for them every minute.
pub fn recorded_list(cat: &store::catalogue::Catalogue) -> Vec<Value> {
    let regions = cat.coverage.get("regions").and_then(Value::as_array);
    regions
        .map(|rs| {
            rs.iter()
                .filter(|r| r.is_object())
                .map(|r| json!({ "id": r["id"], "name": r["name"], "outline": r["outline"] }))
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
        IoPool::for_root(root, 2, Duration::from_secs(60))
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

    /// Every region file, read afresh, as the panel lists them.
    fn listed(pool: &IoPool, dir: &store::nasfs::NasPath) -> Result<ListedAll> {
        load_all(pool, dir, &Listing::default()).map(Listing::split)
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
        assert_eq!(listed(&pool, &store::nasfs::NasPath::local_for_test(&dir)).unwrap(), (vec![], vec![]));
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
        let (ok, bad) = listed(&pool, &store::nasfs::NasPath::local_for_test(&dir)).unwrap();
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
        let (ok, bad) = listed(
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
        std::thread::spawn(move || tx.send(listed(&h, &store::nasfs::NasPath::local_for_test(&d)).unwrap_err()).ok());
        let e = rx
            .recv_timeout(Duration::from_secs(300))
            .unwrap_or_else(|_| panic!("the listing waited on the hung read: no answer in 5 min"));
        assert!(IoError::find(&e).is_some(), "{e:#}");
        release(&stuck);
        // The share still answers, so it's busy rather than away: the rest goes on.
        assert!(hung.is_online());
        std::fs::remove_file(&stuck).unwrap();
        assert_eq!(listed(&pool, &store::nasfs::NasPath::local_for_test(&dir)).unwrap().0.len(), 1);
    }

    async fn json_of(r: Response) -> Value {
        serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    fn ids(v: &Value) -> Vec<String> {
        v["regions"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
    }

    /// The listing reads only the files whose size or time changed since the one before.
    #[test]
    fn only_changed_files_are_read_again() {
        let share = tempfile::tempdir().unwrap();
        let dir = share.path().join("inputs/regions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.toml"), region("a", "A")).unwrap();
        std::fs::write(dir.join("b.toml"), region("b", "B")).unwrap();
        let (pool, nas) =
            (pool(&store::nasfs::NasRoot::local_for_test(share.path())), store::nasfs::NasPath::local_for_test(&dir));
        let mut first = load_all(&pool, &nas, &Listing::default()).unwrap();
        // (What the listing before read for a.toml, told apart from the file's text.)
        if let Ok(l) = first.files.get_mut("a.toml").unwrap().read.as_mut() {
            l.name = "as listed before".into();
        }
        // b.toml changes (its size with it), c.toml comes: both read; a.toml taken as listed.
        std::fs::write(dir.join("b.toml"), region("b", "B, renamed")).unwrap();
        std::fs::write(dir.join("c.toml"), region("c", "C")).unwrap();
        let (ok, _) = load_all(&pool, &nas, &first).unwrap().split();
        assert_eq!(
            ok.iter().map(|l| (l.id.as_str(), l.name.as_str())).collect::<Vec<_>>(),
            [("a", "as listed before"), ("b", "B, renamed"), ("c", "C")]
        );
        // A file gone is gone from it.
        std::fs::remove_file(dir.join("c.toml")).unwrap();
        assert_eq!(load_all(&pool, &nas, &first).unwrap().files.len(), 2);
    }

    /// A zoom's outlines kept on disk read back as made, and only for their key.
    #[test]
    fn kept_outlines_read_back_for_their_key() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("@outlines/z3.jsonl");
        let src = Source {
            id: "c".into(),
            name: "C".into(),
            disabled: true,
            first_osm: None,
            polygons: vec![vec![circle(10.0, 20.0, 1.0, 64)]],
        };
        let made = vec![simplified(&src, tolerance_m(3), None)];
        write_zoom(&path, "1 accepted v1 -", &made);
        assert!(zoom_kept(&path, "1 accepted v1 -") && !zoom_kept(&path, "1 accepted v2 -"));
        assert!(read_zoom(&path, "1 accepted v2 -").is_none());
        let back = read_zoom(&path, "1 accepted v1 -").unwrap();
        assert_eq!(back.len(), 1);
        let (a, b) = (&back[0], &made[0]);
        assert_eq!((&a.id, a.bbox, &a.polys, &a.props, &a.json), (&b.id, b.bbox, &b.polys, &b.props, &b.json));
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
    fn a_regions_land_and_water() {
        use pipeline::inputs::regions::stored::{Geometry, Shape};
        // A 1° square at the equator (~12,364 km²) with a 0.5° hole, as land; a 1° square as water.
        let sq = |x: i32, y: i32, d: i32| vec![[x, y], [x + d, y], [x + d, y + d], [x, y + d]];
        let e = 10_000_000;
        let shape = |rings, water, country: &str| Shape {
            from: "x".into(),
            country: country.into(),
            tolerance_m: 0,
            rings,
            water,
        };
        let g = Geometry {
            id: "r".into(),
            osm: String::new(),
            shapes: vec![
                shape(vec![sq(0, 0, e), sq(e / 4, e / 4, e / 2)], false, "PT"),
                shape(vec![sq(2 * e, 0, e)], true, ""),
            ],
            relations: vec![],
            ways: vec![],
        };
        let z = sized(&g);
        assert!((z.land - 12_364.0 * 0.75).abs() < 60.0, "{z:?}");
        assert!((z.water - 12_364.0).abs() < 60.0, "{z:?}");
        assert_eq!(z.country, "PT");
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

    /// A circle of `n` points around (`x`, `y`), `r` degrees across, as a closed ring.
    fn circle(x: f64, y: f64, r: f64, n: usize) -> Vec<[f64; 2]> {
        let mut v: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                [x + r * a.cos(), y + r * a.sin()]
            })
            .collect();
        v.push(v[0]);
        v
    }

    #[test]
    fn outlines_are_simplified_for_the_zoom_with_a_name_inside() {
        // A big round region with a hole, and a speck of an island far off.
        let src = Source {
            id: "q".into(),
            name: "Q".into(),
            disabled: true,
            first_osm: None,
            polygons: vec![
                vec![circle(-72.0, 52.0, 5.0, 20_000), circle(-72.0, 52.0, 1.0, 4_000)],
                vec![circle(-60.0, 45.0, 0.001, 50)],
            ],
        };
        let points = |v: &Value| -> usize {
            v["geometry"]["coordinates"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|p| p.as_array().unwrap())
                .map(|r| r.as_array().unwrap().len())
                .sum()
        };
        let at = |z: u8| {
            let s = simplified(&src, tolerance_m(z), None);
            (serde_json::from_str::<Value>(&s.json).unwrap(), s)
        };
        let (z0, s0) = at(0);
        let (z12, _) = at(OUTLINE_MAX_Z);
        // Zoomed out: few points, the speck left out (but the hole, about a pixel at z0, kept or not).
        assert!(points(&z0) < 200, "{} points at z0", points(&z0));
        assert_eq!(z0["geometry"]["coordinates"].as_array().unwrap().len(), 1, "the speck is under a pixel at z0");
        // Zoomed in: the shape's detail, and the speck.
        assert!(points(&z12) > 10 * points(&z0), "{} points at z12", points(&z12));
        assert_eq!(z12["geometry"]["coordinates"].as_array().unwrap().len(), 2);
        // The box is the whole region's, the speck too, whatever was left out.
        assert!((s0.bbox[0] - -77.0).abs() < 1e-6 && (s0.bbox[2] - -59.999).abs() < 1e-3, "{:?}", s0.bbox);
        // The name inside the ring, out of the hole.
        let l = z0["properties"]["label"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
        let d = ((l[0] + 72.0).powi(2) + (l[1] - 52.0).powi(2)).sqrt();
        assert!(d > 1.0 && d < 5.0, "label {l:?}");
        assert_eq!(z0["properties"]["disabled"], true);
        assert_eq!(z0["properties"]["region"], "q");
    }

    /// A folder's outline: two regions side by side (their shared border simplified a little apart)
    /// and the waters round them, which hold them as a hole, make one ring round all; an enclave
    /// no region covers stays a hole.
    #[test]
    fn a_folders_outline_is_its_regions_as_one() {
        let sq = |id: &str, polys: Vec<Vec<Vec<[f64; 2]>>>| Simplified {
            id: id.into(),
            bbox: [0.0; 4],
            polys,
            props: json!({}),
            json: String::new(),
        };
        let r = |w: f64, s: f64, e: f64, n: f64| vec![[w, s], [e, s], [e, n], [w, n], [w, s]];
        let west = sq("west", vec![vec![r(0.0, 0.0, 1.0, 1.0)]]);
        // (Its border 1e-4° off the west's: simplified apart.)
        let east = sq("east", vec![vec![r(1.0001, 0.0, 2.0, 1.0), r(1.4, 0.4, 1.6, 0.6)]]);
        let waters = sq("waters", vec![vec![r(-1.0, -1.0, 3.0, 2.0), r(0.0, 0.0, 2.0, 1.0)]]);
        let u = union_of(&[&west, &east, &waters], 100.0).unwrap();
        assert_eq!(u.id, "west,east,waters");
        assert_eq!(u.polys.len(), 1, "one polygon");
        assert_eq!(u.polys[0].len(), 2, "its outer ring and the enclave: {:?}", u.polys);
        let xs: Vec<f64> = u.polys[0][0].iter().map(|p| p[0]).collect();
        assert!(
            xs.iter().all(|x| (*x + 1.0).abs() < 1e-3 || (*x - 3.0).abs() < 1e-3),
            "the waters' outer ring: {xs:?}"
        );
        // Two regions apart: two polygons.
        let far = sq("far", vec![vec![r(10.0, 0.0, 11.0, 1.0)]]);
        assert_eq!(union_of(&[&west, &far], 100.0).unwrap().polys.len(), 2);
    }

    #[test]
    fn outlines_are_narrowed_by_box_and_ids() {
        let one = |id: &str, bbox: [f64; 4]| Simplified {
            id: id.into(),
            bbox,
            polys: vec![],
            props: json!({}),
            json: format!("{{\"id\":\"{id}\"}}"),
        };
        let all = [one("a", [0.0, 0.0, 1.0, 1.0]), one("b", [10.0, 10.0, 11.0, 11.0])];
        let ids = |b: &str| {
            serde_json::from_str::<Value>(b).unwrap()["features"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["id"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&collection(&all, 3, None, None)), ["a", "b"]);
        assert_eq!(ids(&collection(&all, 3, Some([-0.5, -0.5, 2.0, 2.0]), None)), ["a"]);
        let only_b: std::collections::HashSet<&str> = ["b"].into();
        assert_eq!(ids(&collection(&all, 3, None, Some(&only_b))), ["b"]);
        assert_eq!(serde_json::from_str::<Value>(&collection(&all, 7, None, None)).unwrap()["z"], 7);
    }

    #[test]
    fn an_outline_reaching_out_of_the_box_is_clipped_to_it() {
        let sq = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0], [0.0, 0.0]];
        let c = clip_ring(&sq, [1.0, -1.0, 2.0, 5.0]);
        assert_eq!(c.first(), c.last());
        assert!(c.iter().all(|p| p[0] >= 1.0 && p[0] <= 2.0), "{c:?}");
        assert!((ring_area(&c).abs() - 4.0).abs() < 1e-9, "{c:?}");
        assert!(clip_ring(&sq, [5.0, 5.0, 6.0, 6.0]).is_empty());
        let r = Simplified {
            id: "a".into(),
            bbox: [0.0, 0.0, 4.0, 4.0],
            polys: vec![vec![sq.clone()]],
            props: json!({"region": "a"}),
            json: String::new(),
        };
        let v: Value = serde_json::from_str(&collection(&[r], 9, Some([1.0, -1.0, 2.0, 5.0]), None)).unwrap();
        assert_eq!(v["clip"], json!([1.0, -1.0, 2.0, 5.0]));
        let ring = &v["features"][0]["geometry"]["coordinates"][0][0];
        assert!(ring.as_array().unwrap().iter().all(|p| p[0].as_f64().unwrap() >= 1.0), "{ring}");
        assert_eq!(v["features"][0]["properties"]["region"], "a");
    }
}
