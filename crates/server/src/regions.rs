//! Regions and the areas they're made of (docs/surface.md, the Regions panel; docs/regions.md).
//!
//!   GET    /api/regions             the region files in inputs/regions on the NAS, in every folder,
//!                                   each with its path and its state in the input check (accepted,
//!                                   checking, held with the findings), the files that don't parse,
//!                                   and the folders' tree (docs/inputs.md §6.1.9); from the listing
//!                                   kept on this Mac (`Listing`), brought up to date every minute
//!   GET    /api/regions/at          the regions at a point (`lng`, `lat`), from the region layer's
//!                                   tiles (codecs::regionlayer::hit): what the footer tells hovered
//!   GET    /api/regions/symbols/<hash16>.<ext>  a region's or folder's symbol (its flag, coat of
//!                                   arms or seal: docs/inputs.md §6.1.3), by its content name, from
//!                                   this Mac's copy of the accepted version: cached for good
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
use buildstore::coverage::DrawnRegion;
use buildstore::outlines::{OutlineRec, Ring};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use store::iopool::{IoError, IoPool};

/// The outlines of the current catalogue: records and strings in memory, rings read on demand.
pub struct OutlineIndex {
    sect: Arc<crate::views::SectView>,
    pub recs: Vec<OutlineRec>,
    strings: Vec<String>,
}

impl OutlineIndex {
    fn open(data: &crate::data::Data) -> Result<Option<OutlineIndex>> {
        let cat = data.catalogue();
        let Some(logical) = cat.global.get("outlines") else { return Ok(None) };
        let Some(sect) = data.sect(logical)? else { return Ok(None) };
        let recs: Vec<OutlineRec> = bytemuck::pod_collect_to_vec(sect.get("recs")?.bytes());
        let strings: Vec<String> =
            String::from_utf8_lossy(sect.get("strings")?.bytes()).split('\n').map(str::to_string).collect();
        Ok(Some(OutlineIndex { sect, recs, strings }))
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
    let areas = buildstore::outlines::spoken_areas(table, &ix.recs, |i| ix.string(i).to_owned(), polygons)?;
    Ok(Some(names::Spoken::build(table, areas)))
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
    /// Its land and water (km²), from its accepted geometry (`Sized`); none until this Mac has
    /// worked them out, or for a region the accepted version doesn't have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub land_km2: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_km2: Option<f64>,
    /// Its box and the point its name is drawn at (`Sized`), with its sizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<[f64; 2]>,
    /// Its symbol as accepted (`symbol_json`: its URL here and its credit); none when it has none
    /// (the list shows the placeholder). Not kept in the listing: read from the accepted copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<Value>,
}

impl Listed {
    fn of(r: &inputs::inputs::regions::file::Region, path: String) -> Listed {
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
            bbox: None,
            label: None,
            symbol: None,
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
            if inputs::inputs::ignored(&it.name) || (rel.is_empty() && (it.name == "how" || it.name == "todo")) {
                continue;
            }
            let path = if rel.is_empty() { it.name.clone() } else { format!("{rel}/{}", it.name) };
            if it.is_dir {
                stack.push((d.join(&it.name), path));
                continue;
            }
            if !it.name.ends_with(".toml") || inputs::inputs::regions::resolve::special(&path).is_some() {
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
                inputs::inputs::regions::file::parse(&it.name, &t)
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
/// and time), then the regions' accepted copy (its index: the geometry the sizes are read from),
/// brought up to date; and in a
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
        let accepted = inputs::inputs::served::kept(&accepted_dir(&s2));
        let images = accepted.as_ref().map(|k| Symbols::of(&k.index)).unwrap_or_default();
        for r in &mut ok {
            let facts = accepted.as_ref().and_then(|k| k.index.files.get(&r.path)).map(|e| &e.facts);
            r.symbol = facts.and_then(|f| symbol_json(&images, f.get("symbol")?));
        }
        let mut t = serde_json::to_value(tree(&s2, &ok)).unwrap_or_default();
        folder_symbols(&images, &mut t);
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

/// A region's land and water (km², on the authalic sphere: `geom::area_km2`), its box
/// (w, s, e, n in degrees: the panel frames the view on it) and a point inside its largest land
/// polygon (a region of water alone: water) for its name on the map (`label_point`), from its
/// stored geometry (inputs::inputs::regions::stored: water shapes are those marked `water`, the rest
/// land).
#[derive(serde::Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Sized {
    pub land: f64,
    pub water: f64,
    /// (None in what an older app kept: sized again.)
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
    #[serde(default)]
    pub label: Option<[f64; 2]>,
}

pub fn sized(g: &inputs::inputs::regions::stored::Geometry) -> Sized {
    use inputs::inputs::regions::geom::{area_km2, Area};
    let (mut land, mut water) = (0.0, 0.0);
    // (The name's point: in the largest polygon by its outer ring of its land shapes, unless its land
    // is under a thousandth of it (a region of waters with a few islets of land: Mexico's, 14 km² of
    // 3.2 M); a shape's own polygon, never a union of them, which takes minutes for a region of land
    // and water. Land and water alike, a region's waters won, their outer ring holding the land too,
    // and the name sat in the sea: Iceland's, Alaska's, peninsular Malaysia's.)
    let mut polys: Vec<(bool, f64, Vec<Vec<[f64; 2]>>)> = Vec::new();
    for sh in &g.shapes {
        let rings: Vec<Vec<[i64; 2]>> =
            sh.rings.iter().map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect()).collect();
        let area = Area::from_rings(&rings);
        let a = area_km2(&area).abs();
        if sh.water {
            water += a;
        } else {
            land += a;
        }
        for p in &area.polys {
            let poly: Vec<Vec<[f64; 2]>> =
                p.iter().map(|r| r.iter().map(|q| [q[0] as f64 * 1e-7, q[1] as f64 * 1e-7]).collect()).collect();
            polys.push((!sh.water, poly.first().map_or(0.0, |r| ring_area(r).abs()), poly));
        }
    }
    let on_land = land >= (land + water) / 1000.0;
    let largest =
        polys.into_iter().map(|(l, a, p)| ((l == on_land, a), p)).reduce(|best, x| if x.0 > best.0 { x } else { best });
    let b = g.bbox();
    let deg = |v: i32| f64::from(v) * 1e-7;
    let bbox = (b != [0; 4]).then(|| [deg(b[0]), deg(b[1]), deg(b[2]), deg(b[3])]);
    let label = largest.and_then(|(_, p)| label_point(&p));
    Sized { land: land.round(), water: water.round(), bbox, label }
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
    let kept = inputs::inputs::served::kept(&accepted_dir(s));
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
            r.bbox = z.bbox;
            r.label = z.label;
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
        if sizes.get(g).is_some_and(|z| z.bbox.is_some()) || !pool.is_online() {
            continue;
        }
        let read = store::PooledFile::new(root.join(g), pool.clone()).read_all();
        match read.map_err(anyhow::Error::from).and_then(|b| inputs::inputs::regions::stored::Geometry::decode(&b)) {
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
) -> Option<(Option<inputs::inputs::Index>, Option<inputs::inputs::Report>)> {
    let root = s.data.nas_root()?;
    let m = inputs::inputs::served::manifest(&|p: &str| Ok(pool.read_all(&root.join(p))?)).ok()?;
    let read = |p: &str| pool.read_all(&root.join(p)).ok();
    let unit = inputs::inputs::regions::UNIT;
    let index =
        m.get(&inputs::inputs::logical(unit)).and_then(|n| read(n)).and_then(|b| serde_json::from_slice(&b).ok());
    let held =
        m.get(&inputs::inputs::held_logical(unit)).and_then(|n| read(n)).and_then(|b| serde_json::from_slice(&b).ok());
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
/// from their `_group.toml`, and their regions (inputs::inputs::regions::read::tree), from the
/// copy kept on this Mac; with none yet, the folders of the files listed, by their own names.
fn tree(s: &crate::AppState, listed: &[Listed]) -> inputs::inputs::regions::read::Folder {
    let kept = inputs::inputs::served::kept(&accepted_dir(s));
    let index = kept.map(|k| k.index).unwrap_or_else(|| {
        let mut i = inputs::inputs::Index::default();
        for l in listed {
            let mut facts = serde_json::Map::new();
            facts.insert("id".into(), l.id.clone().into());
            i.files.insert(
                l.path.clone(),
                inputs::inputs::FileEntry { file: String::new(), size: 0, keyed: String::new(), facts },
            );
        }
        i
    });
    inputs::inputs::regions::read::tree(&index)
}

/// Where this Mac keeps the regions' accepted version (its index).
fn accepted_dir(s: &crate::AppState) -> std::path::PathBuf {
    crate::accepted::local_dir(&s.data.home, inputs::inputs::regions::UNIT)
}

/// The regions' accepted version kept on this Mac in step with the records (its index alone: the
/// files' facts); nothing while the NAS is away.
pub fn sync_accepted(s: &crate::AppState) {
    let (Some(root), Some(pool)) = (s.data.nas_root(), s.data.pool()) else { return };
    if !pool.is_online() {
        return;
    }
    let read =
        |p: &str| -> anyhow::Result<Vec<u8>> { Ok(store::PooledFile::new(root.join(p), pool.clone()).read_all()?) };
    if let Err(e) = inputs::inputs::served::sync(&read, inputs::inputs::regions::UNIT, &accepted_dir(s)) {
        eprintln!("regions: the accepted copy wasn't brought up to date: {e:#}");
    }
}

/// Whether region `id` is disabled (`disabled = true`): by the accepted copy kept on this Mac, else
/// the listing of the inputs folder.
pub fn is_disabled(s: &crate::AppState, id: &str) -> bool {
    let dir = accepted_dir(s);
    let kept = inputs::inputs::served::kept(&dir);
    if let Some(k) = kept {
        return k.index.files.values().any(|e| {
            e.facts.get("id").and_then(Value::as_str) == Some(id)
                && e.facts.get("disabled").and_then(Value::as_bool) == Some(true)
        });
    }
    read_listing(s).is_some_and(|l| l.files.values().any(|f| f.read.as_ref().is_ok_and(|l| l.id == id && l.disabled)))
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

// ---- the symbols ---------------------------------------------------------------------------------

use inputs::inputs::regions::symbols::{self, Images as Symbols};

/// A `[symbol]` fact as the list gives it: its URL here (by its image's content name) and its
/// credit; None when the accepted version hasn't its image (the placeholder).
fn symbol_json(images: &Symbols, fact: &Value) -> Option<Value> {
    let s: symbols::Symbol = serde_json::from_value(fact.clone()).ok()?;
    let content = images.by_name.get(&s.image)?;
    Some(json!({
        "url": format!("/api/regions/symbols/{content}"),
        "kind": s.kind, "source": s.source, "author": s.author, "licence": s.licence,
    }))
}

/// The tree's folders' symbols (their `_group.toml`'s) as the list gives them.
fn folder_symbols(images: &Symbols, f: &mut Value) {
    if let Some(o) = f.as_object_mut() {
        if let Some(s) = o.remove("symbol").and_then(|s| symbol_json(images, &s)) {
            o.insert("symbol".into(), s);
        }
        for g in o.get_mut("folders").and_then(Value::as_array_mut).into_iter().flatten() {
            folder_symbols(images, g);
        }
    }
}

/// The images of the accepted version kept here, by content name (`Symbols`), and the bundle read
/// last (its copy's name and its images): read again only when the copy's index changes.
type SymbolsKept = Option<((std::path::PathBuf, u64, Option<std::time::SystemTime>), Arc<Symbols>)>;
static SYMBOLS: Mutex<SymbolsKept> = Mutex::new(None);
static BUNDLE: Mutex<Option<(String, Arc<std::collections::BTreeMap<String, Vec<u8>>>)>> = Mutex::new(None);

/// The bytes of the image named `name` (`<hash16>.<ext>`) in the accepted version kept in `dir`,
/// checked against its name; None when it has none of that name.
fn symbol_bytes(dir: &std::path::Path, name: &str) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(dir.join("@index.json")).ok()?;
    let stamp = (dir.to_path_buf(), meta.len(), meta.modified().ok());
    let images = {
        let mut g = SYMBOLS.lock().unwrap_or_else(|e| e.into_inner());
        match g.as_ref().filter(|(k, _)| *k == stamp) {
            Some((_, i)) => i.clone(),
            None => {
                let i = Arc::new(Symbols::of(&inputs::inputs::served::kept(dir)?.index));
                *g = Some((stamp, i.clone()));
                i
            }
        }
    };
    let (file, member) = images.by_content.get(name)?;
    let bytes = match member {
        None => std::fs::read(dir.join(file)).ok()?,
        Some(m) => {
            let mut g = BUNDLE.lock().unwrap_or_else(|e| e.into_inner());
            if g.as_ref().is_none_or(|(f, _)| f != file) {
                let b = symbols::bundle(&std::fs::read(dir.join(file)).ok()?).ok()?;
                *g = Some((file.clone(), Arc::new(b)));
            }
            g.as_ref()?.1.get(m)?.clone()
        }
    };
    (name.split('.').next() == Some(store::naming::hash16(&bytes).as_str())).then_some(bytes)
}

/// A symbol by its content name: cached for good (its name is its content), served as an image
/// alone. The check refused any SVG with a script, an event handler or a reference outside itself
/// (inputs::inputs::regions::symbols::svg); behind it, the headers: its type from its name (never
/// sniffed), and a policy that loads nothing and runs nothing, so even opened on its own, the
/// image can't reach anything.
pub async fn symbol(State(s): State<S>, axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    // (A content name, `<hash16>.<ext>`, or nothing: never a path.)
    let hex = name
        .split_once('.')
        .is_some_and(|(h, _)| h.len() == 16 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    let Some(kind) = symbols::Kind::of(&name).filter(|_| hex) else { return StatusCode::NOT_FOUND.into_response() };
    let dir = accepted_dir(&s);
    let found = tokio::task::spawn_blocking(move || symbol_bytes(&dir, &name)).await.ok().flatten();
    let Some(bytes) = found else { return StatusCode::NOT_FOUND.into_response() };
    (
        [
            (axum::http::header::CONTENT_TYPE, kind.mime()),
            (axum::http::header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (axum::http::header::CONTENT_SECURITY_POLICY, SYMBOL_CSP),
        ],
        bytes,
    )
        .into_response()
}

/// What a symbol may do when opened on its own: load nothing, run nothing; its own styles only.
pub const SYMBOL_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/// What `/api/regions/at` is asked: a point.
#[derive(Deserialize)]
pub struct AtQuery {
    lng: f64,
    lat: f64,
}

/// The regions at a point (`{"regions": [id, …]}`), from the region layer's tiles
/// (codecs::regionlayer::hit): none where no region is, or before the map has the layer.
pub async fn at(State(s): State<S>, axum::extract::Query(q): axum::extract::Query<AtQuery>) -> Response {
    tokio::task::spawn_blocking(move || -> Result<Value> {
        let mut get = |z: u8, x: u32, y: u32| -> Result<Option<Vec<u8>>> {
            match s.data.tile(codecs::regionlayer::LAYER, z, x, y)? {
                Some((b, _)) => Ok(Some(names::mvt::gunzip_if_gzip(b.bytes())?.into_owned())),
                None => Ok(None),
            }
        };
        Ok(json!({ "regions": codecs::regionlayer::hit::regions_at(&mut get, q.lng, q.lat)? }))
    })
    .await
    .map(|r| match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")),
    })
    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// The regions a catalogue records (its `coverage`), each with its small outlines (@drawn: what
/// downloads, credits and the names' checks read; the map draws the region layer); none for a catalogue made before they were recorded (or one whose coverage this app can't read).
pub fn recorded(cat: &store::catalogue::Catalogue) -> Vec<DrawnRegion> {
    cat.coverage.regions().unwrap_or_default()
}

/// The regions a catalogue records, without their outlines (id, name and outline entries), read straight from its coverage: /api/catalog asks
/// for them every minute.
pub fn recorded_list(cat: &store::catalogue::Catalogue) -> Vec<Value> {
    // (Each object's three members, null where it has none, read from its text.)
    #[derive(serde::Deserialize)]
    struct Head {
        #[serde(default)]
        id: Value,
        #[serde(default)]
        name: Value,
        #[serde(default)]
        outline: Value,
    }
    cat.coverage
        .region_texts()
        .iter()
        .filter(|r| r.starts_with('{'))
        .filter_map(|r| serde_json::from_str::<Head>(r).ok())
        .map(|h| json!({ "id": h.id, "name": h.name, "outline": h.outline }))
        .collect()
}

/// `recorded_list` of the catalogue served, kept for it: /api/catalog asks every minute, the
/// downloads' state (/api/downloads) as the Regions panel shows it, and each region's text is read
/// through (~0.15 s for the world's on the M4).
pub fn recorded_list_kept(cat: &Arc<store::catalogue::Catalogue>) -> Arc<Vec<Value>> {
    type Listed = Option<(std::sync::Weak<store::catalogue::Catalogue>, Arc<Vec<Value>>)>;
    static LISTED: Mutex<Listed> = Mutex::new(None);
    // (Its Weak keeps the catalogue's allocation, so a pointer equal to it is that catalogue.)
    if let Some((w, l)) = LISTED.lock().unwrap().as_ref() {
        if std::ptr::eq(w.as_ptr(), Arc::as_ptr(cat)) {
            return l.clone();
        }
    }
    let l = Arc::new(recorded_list(cat));
    *LISTED.lock().unwrap() = Some((Arc::downgrade(cat), l.clone()));
    l
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
        let e = rx.recv_timeout(store::testkit::WATCHDOG).unwrap_or_else(|_| {
            panic!("the listing waited on the hung read: no answer in {:?}", store::testkit::WATCHDOG)
        });
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

    #[tokio::test]
    async fn the_symbols_are_listed_and_served_from_the_accepted_copy() {
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = store::nasfs::NasRoot::local_for_test(nas.path());
        let table = |image: &str| {
            format!("[symbol]\nimage = \"{image}\"\nkind = \"flag\"\nsource = \"https://example.org/{image}\"\nlicence = \"CC0\"\n")
        };
        let a = format!("{}{}", region("a", "A"), table("Q1.svg"));
        let b = format!("{}{}", region("b", "B"), table("Q9.svg"));
        let group = format!("name = \"Europe\"\n{}", table("Q2.png"));
        std::fs::create_dir_all(nas.path().join("inputs/regions/eu")).unwrap();
        std::fs::write(nas.path().join("inputs/regions/eu/a.toml"), &a).unwrap();
        std::fs::write(nas.path().join("inputs/regions/b.toml"), &b).unwrap();
        let svg = "<svg xmlns='http://www.w3.org/2000/svg'/>";
        let png = {
            let mut p = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            p.extend(2u32.to_be_bytes());
            p.extend(1u32.to_be_bytes());
            p
        };
        // (A zip of one stored member, as a tool may write it; its CRC isn't read.)
        let zip = {
            let n = b"Q2.png";
            let head = |sig: u32, central: bool| {
                let mut h = sig.to_le_bytes().to_vec();
                if central {
                    h.extend(20u16.to_le_bytes());
                }
                h.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                h.extend((png.len() as u32).to_le_bytes());
                h.extend((png.len() as u32).to_le_bytes());
                h.extend((n.len() as u16).to_le_bytes());
                h.extend(0u16.to_le_bytes());
                if central {
                    h.extend([0u8; 14]);
                }
                h.extend(n);
                h
            };
            let mut z = head(0x0403_4b50, false);
            z.extend(&png);
            let (at, dir) = (z.len() as u32, head(0x0201_4b50, true));
            z.extend(&dir);
            z.extend(0x0605_4b50u32.to_le_bytes());
            z.extend([0, 0, 0, 0, 1, 0, 1, 0]);
            z.extend((dir.len() as u32).to_le_bytes());
            z.extend(at.to_le_bytes());
            z.extend([0, 0]);
            z
        };
        // (The accepted version as the check makes it: each file's facts.)
        let name = crate::accepted::put(
            &root,
            "regions",
            &[("eu/a.toml", a.as_str()), ("b.toml", b.as_str()), ("eu/Q1.svg", svg)],
        );
        let mut index: inputs::inputs::Index = serde_json::from_slice(&root.join(&name).read().unwrap()).unwrap();
        let fact = |t: &str| serde_json::to_value(toml::from_str::<toml::Value>(t).unwrap()["symbol"].clone()).unwrap();
        for (p, f, id) in [("eu/a.toml", fact(&a), "a"), ("b.toml", fact(&b), "b")] {
            let e = index.files.get_mut(p).unwrap();
            e.facts.insert("symbol".into(), f);
            e.facts.insert("id".into(), id.into());
        }
        let zname = inputs::inputs::copy_name("regions", "symbols.zip", &store::naming::hash16(&zip)).unwrap();
        root.join(&zname).write(&zip).unwrap();
        let mut zfacts = serde_json::Map::new();
        zfacts.insert("images".into(), json!({"Q2.png": store::naming::hash16(&png)}));
        index.files.insert(
            "symbols.zip".into(),
            inputs::inputs::FileEntry { file: zname, size: zip.len() as u64, keyed: "-".into(), facts: zfacts },
        );
        let gname =
            inputs::inputs::copy_name("regions", "eu/_group.toml", &store::naming::hash16(group.as_bytes())).unwrap();
        root.join(&gname).write(group.as_bytes()).unwrap();
        let mut gfacts = serde_json::Map::new();
        gfacts.insert("group".into(), json!({"name": "Europe"}));
        gfacts.insert("symbol".into(), fact(&group));
        index.files.insert(
            "eu/_group.toml".into(),
            inputs::inputs::FileEntry { file: gname, size: group.len() as u64, keyed: "-".into(), facts: gfacts },
        );
        let ib = serde_json::to_vec(&index).unwrap();
        let iname = format!("sources/inputs/regions/@index.{}.json", store::naming::hash16(&ib));
        root.join(&iname).write(&ib).unwrap();
        root.join(inputs::inputs::served::RECORDS)
            .write(&serde_json::to_vec(&json!({ inputs::inputs::logical("regions"): iname })).unwrap())
            .unwrap();
        let s = crate::test_state(home.path(), &root);
        sync_accepted(&s);
        let v = json_of(list(State(s.clone())).await).await;
        let by = |id: &str| v["regions"].as_array().unwrap().iter().find(|r| r["id"] == id).unwrap().clone();
        let a_sym = &by("a")["symbol"];
        assert_eq!(a_sym["url"], format!("/api/regions/symbols/{}.svg", store::naming::hash16(svg.as_bytes())));
        assert_eq!((a_sym["kind"].as_str(), a_sym["licence"].as_str()), (Some("flag"), Some("CC0")));
        // An image the version hasn't: no symbol (the placeholder).
        assert!(by("b").get("symbol").is_none());
        let folder = &v["tree"]["folders"][0];
        assert_eq!(folder["symbol"]["url"], format!("/api/regions/symbols/{}.png", store::naming::hash16(&png)));
        // Served: a loose one, a bundle's member; by content name only, as an image alone.
        for (url, bytes, mime) in [
            (a_sym["url"].as_str().unwrap(), svg.as_bytes(), "image/svg+xml; charset=utf-8"),
            (folder["symbol"]["url"].as_str().unwrap(), png.as_slice(), "image/png"),
        ] {
            let name = url.rsplit('/').next().unwrap().to_string();
            let r = symbol(State(s.clone()), axum::extract::Path(name)).await;
            assert_eq!(r.status(), StatusCode::OK);
            let h = |k: axum::http::header::HeaderName| r.headers()[k].to_str().unwrap().to_string();
            assert_eq!(h(axum::http::header::CONTENT_TYPE), mime);
            assert_eq!(h(axum::http::header::CACHE_CONTROL), "public, max-age=31536000, immutable");
            assert_eq!(h(axum::http::header::X_CONTENT_TYPE_OPTIONS), "nosniff");
            assert_eq!(h(axum::http::header::CONTENT_SECURITY_POLICY), SYMBOL_CSP);
            assert_eq!(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()[..], bytes);
        }
        let q1 = a_sym["url"].as_str().unwrap().rsplit('/').next().unwrap().to_string();
        // (Not a content name it has, or not a content name: an upper-case one, one with more after
        // it, a path.)
        let names = [
            "0000000000000000.svg".into(),
            "x.txt".into(),
            "../@index.json".into(),
            q1.to_uppercase(),
            format!("{q1}.svg"),
            format!("../{q1}"),
        ];
        for name in names {
            assert_eq!(
                symbol(State(s.clone()), axum::extract::Path(name.into())).await.status(),
                StatusCode::NOT_FOUND
            );
        }
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
    }

    fn catalogue(coverage: Value) -> store::catalogue::Catalogue {
        let mut c = store::catalogue::Catalogue::new(3);
        c.coverage = coverage.into();
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
        // (An object's members it lacks are null, as a `Value`'s index gave; one that isn't left out.)
        let odd = catalogue(json!({"regions": [{"id": "x"}, 5, {"id": "y", "name": "Y"}]}));
        assert_eq!(
            recorded_list(&odd),
            [json!({"id": "x", "name": null, "outline": null}), json!({"id": "y", "name": "Y", "outline": null})]
        );
        // Kept for the catalogue it's of, made again for another.
        let (c, odd) = (Arc::new(c), Arc::new(odd));
        let kept = recorded_list_kept(&c);
        assert_eq!(*kept, recorded_list(&c));
        assert!(Arc::ptr_eq(&kept, &recorded_list_kept(&c)));
        assert_eq!(*recorded_list_kept(&odd), recorded_list(&odd));
        assert_eq!(*recorded_list_kept(&c), *kept);
    }

    #[test]
    fn a_regions_land_and_water() {
        use inputs::inputs::regions::stored::{Geometry, Shape};
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
        // Its name on its land: not in its waters round it, whose outer ring holds the land.
        let land = sq(0, 0, e);
        let waters = vec![sq(-e, -e, 3 * e), sq(0, 0, e)];
        let g = Geometry { shapes: vec![shape(waters, true, ""), shape(vec![land], false, "MT")], ..g };
        let l = sized(&g).label.unwrap();
        assert!(l[0] > 0.0 && l[0] < 1.0 && l[1] > 0.0 && l[1] < 1.0, "{l:?}");
        // A region of water alone, or of waters with an islet of land under a thousandth of it: in its
        // water.
        let water = || shape(vec![sq(-e, -e, 3 * e), sq(0, 0, e)], true, "");
        for shapes in [vec![water()], vec![water(), shape(vec![sq(5 * e, 5 * e, e / 100)], false, "")]] {
            let l = sized(&Geometry { shapes, ..g.clone() }).label.unwrap();
            assert!(!(l[0] > 0.0 && l[0] < 1.0 && l[1] > 0.0 && l[1] < 1.0) && l[0] > -1.0 && l[0] < 2.0, "{l:?}");
        }
    }

    #[test]
    fn a_name_inside_its_largest_polygon() {
        // A ring round a bay: the parallel through its centroid crosses the bay, the box's middle doesn't.
        let ring =
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 6.0], [8.0, 6.0], [8.0, 4.0], [0.0, 4.0]];
        let p = label_point(&[ring.clone()]).unwrap();
        assert!(p[1] < 4.0 || p[1] > 6.0 || p[0] > 8.0, "{p:?}");
        assert!(label_point(&[]).is_none());
    }
}
