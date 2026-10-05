//! Today's area overlays, parks, stations and ferries as tiles and data by view (docs/phase5.md
//! "Areas", "Stations", "Ferries"): the `convert-legacy-overlays` step.
//!
//! - **Areas** (`layers/ov-{heritage-areas,indigenous,special,whs}`): vector tiles (layer `a`), the
//!   lean files' properties with each feature's id (docs/phase5.md "Ids") and `own`, the z3 tile
//!   whose `ovdata` holds its details. World Heritage outlines carry their site dot's id and
//!   position (`px`, `py`), whose popup they open.
//! - **`ovdata/3-x-y`**: the areas' details by id, and the parks' records (looked up by name near a
//!   point), owned by the z3 tile of their box's centre.
//! - **Stations** (`layers/stations`): vector tiles (layer `s`: `n, en, g, m, sp, mz` and an id);
//!   a tile at zoom z holds the stops that show at zooms up to z + 1 (a stop's dot from `mz` +
//!   log2(12 px), stations.ts), zoom 12 every stop.
//! - **Ferries** (`layers/ferries`, gzip'd GeoJSON blocks): the world at zoom 0 (ways simplified
//!   to 5 km), each z3 tile (300 m) and z6 tile (whole): the ways touching it, the terminals near
//!   it, its ways' lines' records; each way with its whole length (`km`) and an id, so the app
//!   merges blocks by id and measures what's in view on the geometry it has.

use det::Det;
use crate::hipack::{grow, meets, tile_bounds};
use crate::layers::{pack_of, write_pack};
use crate::legacy::Unit;
use crate::markconv::{self, legacy_bytes, src_bytes};
use crate::marks::{self, IdSource};
use crate::out::Out;
use crate::vtgen::{self, Feature, Geom};
use anyhow::{bail, Context, Result};
use names::mvt::Value as MvtValue;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// The areas' vector-tile layer.
pub const LAYER: &str = "a";
pub const MAXZ: u8 = 12;
/// Hi tiles (z9–12) only where the coverage is, and this far around it.
const HALO_KM: f64 = 20.0;

/// An area overlay: its catalog layer, today's lean file, its details (file, and the key its
/// records go under in ovdata and the detail route).
struct Area {
    layer: &'static str,
    file: &'static str,
    details: Option<(&'static str, &'static str)>,
}

const AREAS: [Area; 4] = [
    Area { layer: "ov-heritage-areas", file: "layer-heritage-areas", details: Some(("details-harea", "harea")) },
    Area { layer: "ov-indigenous", file: "layer-indigenous", details: Some(("details-indigenous", "indigenous")) },
    Area { layer: "ov-special", file: "layer-special", details: Some(("details-special", "special")) },
    Area { layer: "ov-whs", file: "layer-whs-shapes", details: None },
];

pub struct Converted {
    pub areas: usize,
    pub tiles: usize,
    pub ovdata: usize,
    pub parks: usize,
    pub stations: usize,
    pub ferries: usize,
}

/// The stations' vector-tile layer.
pub const STATION_LAYER: &str = "s";
/// A stop's dot shows from its `mz` plus this (log2 of stations.ts STOP_PX).
const STOP_DZ: f64 = 3.584_962_500_721_156;

/// A GeoJSON geometry as vtgen's, and its bounding box (None: not a line or area).
fn geom_of(g: &Value) -> Option<(Geom, [f64; 4])> {
    let pt = |c: &Value| -> Option<[f64; 2]> { Some([c[0].as_f64()?, c[1].as_f64()?]) };
    let line = |c: &Value| -> Option<Vec<[f64; 2]>> { c.as_array()?.iter().map(pt).collect() };
    let poly = |c: &Value| -> Option<Vec<Vec<[f64; 2]>>> { c.as_array()?.iter().map(line).collect() };
    let c = &g["coordinates"];
    let geom = match g["type"].as_str()? {
        "Polygon" => Geom::Polygons(vec![poly(c)?]),
        "MultiPolygon" => Geom::Polygons(c.as_array()?.iter().map(poly).collect::<Option<_>>()?),
        "LineString" => Geom::Lines(vec![line(c)?]),
        "MultiLineString" => Geom::Lines(c.as_array()?.iter().map(line).collect::<Option<_>>()?),
        _ => return None,
    };
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut add = |p: &[f64; 2]| {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    };
    match &geom {
        Geom::Polygons(ps) => ps.iter().flatten().flatten().for_each(&mut add),
        Geom::Lines(ls) => ls.iter().flatten().for_each(&mut add),
        Geom::Points(ps) => ps.iter().for_each(&mut add),
    }
    b[0].is_finite().then_some((geom, b))
}

/// A JSON property as a vector-tile value (None: null, or not a scalar).
fn mvt_value(v: &Value) -> Option<MvtValue> {
    Some(match v {
        Value::String(s) => MvtValue::String(s.clone()),
        Value::Bool(b) => MvtValue::Bool(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => MvtValue::Sint(i),
            (None, Some(f)) => MvtValue::Double(f),
            _ => return None,
        },
        _ => return None,
    })
}

/// The z3 tile ("3/x/y") holding a point.
fn z3_of(lon: f64, lat: f64) -> String {
    Unit::of_point(3, [marks::e7(lon), marks::e7(lat)]).slash()
}

/// The z6 tiles meeting the coverage (the base packs' units) grown by `HALO_KM`: where hi tiles go.
fn hi_cover(out: &Out) -> HashSet<(u32, u32)> {
    let grown: Vec<_> = out.manifest.keys().filter_map(|k| k.strip_prefix("base/")).filter_map(Unit::parse).map(|u| grow(tile_bounds(u.z, u.x, u.y), HALO_KM)).collect();
    let mut s = HashSet::new();
    for x in 0..64u32 {
        for y in 0..64u32 {
            let b = tile_bounds(6, x, y);
            if grown.iter().any(|g| meets(*g, b)) {
                s.insert((x, y));
            }
        }
    }
    s
}

/// Each heritage record's (`i`) dot: its id and place, as `convert-legacy-marks` made them.
fn heritage_dots(out: &Out) -> Result<HashMap<u64, (u64, f64, f64)>> {
    let (pts, ids) = markconv::points_with_ids(out)?;
    let k = marks::kind_index("heritage").unwrap();
    let mut m = HashMap::new();
    for (p, id) in pts.iter().zip(ids) {
        if p.kind != k || p.pt.flags & marks::flag::COMPONENT != 0 {
            continue;
        }
        let Some(i) = p.info.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()).and_then(|v| v["i"].as_u64()) else { continue };
        m.insert(i, (id, p.lon, p.lat));
    }
    Ok(m)
}

/// The heritage dots the last marks job wrote (markconv::HERITAGE_DOTS).
pub fn marks_dots(out: &Out) -> Result<HashMap<u64, (u64, f64, f64)>> {
    let c = out.get(markconv::HERITAGE_DOTS).context("no heritage dots (the marks step writes them)")?;
    let m: BTreeMap<u64, (u64, f64, f64)> = serde_json::from_slice(&std::fs::read(out.path(c))?)?;
    Ok(m.into_iter().collect())
}

/// Details records by their index (`i`).
fn details_by_i(out: &Out, src: &str, file: &str) -> Result<HashMap<u64, String>> {
    let b = src_bytes(out, src, file)?;
    let mut m = HashMap::new();
    for line in b.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        let v: Value = serde_json::from_slice(line).with_context(|| format!("{file}: a record"))?;
        if let Some(i) = v["i"].as_u64() {
            m.insert(i, String::from_utf8_lossy(line).into_owned());
        }
    }
    Ok(m)
}

/// Records (JSON) under ids in ovdata: sorted ids, offsets (n + 1), the records end to end.
#[derive(Default)]
struct Recs {
    rows: Vec<(u64, String)>,
}

impl Recs {
    fn sections(mut self) -> (Vec<u64>, Vec<u32>, Vec<u8>) {
        self.rows.sort_by_key(|r| r.0);
        let mut offs = vec![0u32];
        let mut bytes = Vec::new();
        let ids = self.rows.iter().map(|r| r.0).collect();
        for (_, s) in &self.rows {
            bytes.extend_from_slice(s.as_bytes());
            offs.push(bytes.len() as u32);
        }
        (ids, offs, bytes)
    }
}

pub fn convert(out: &mut Out) -> Result<Converted> {
    let t0 = std::time::Instant::now();
    let cover = hi_cover(out);
    let want = |z: u8, x: u32, y: u32| z < 9 || cover.contains(&(x >> (z - 6), y >> (z - 6)));
    let dots = heritage_dots(out)?;
    eprintln!("overlays: {} heritage dots, {} z6 tiles for hi tiles ({:.1?})", dots.len(), cover.len(), t0.elapsed());
    let mut ntiles = 0;
    let (areas, n_ov, parks, _) = areas_and_parks(out, markconv::LEGACY, &dots, &want, &mut ntiles)?;
    let stations = stations(out, &want, &mut ntiles)?;
    let ferries = ferries(out, &mut ntiles)?;
    eprintln!("overlays: {areas} areas, {stations} stations, {ferries} ferry blocks, {ntiles} tiles, {n_ov} ovdata, {parks} parks ({:.1?})", t0.elapsed());
    Ok(Converted { areas, tiles: ntiles, ovdata: n_ov, parks, stations, ferries })
}

/// The overlays job (docs/phase5.md "Heritage and area flags"): the area overlays and the parks
/// from a pass's heritage outputs (`src`), the World Heritage outlines with the dots' ids the marks
/// job gave them (`dots`); the overlays' packs and ovdata this run didn't write dropped; the
/// summary and the sources list the map loads whole, as `global/heritage/…`.
pub fn overlays(out: &mut Out, src: &str, dots: &HashMap<u64, (u64, f64, f64)>) -> Result<(usize, usize, usize, usize)> {
    let t0 = std::time::Instant::now();
    let cover = hi_cover(out);
    let want = |z: u8, x: u32, y: u32| z < 9 || cover.contains(&(x >> (z - 6), y >> (z - 6)));
    let mut ntiles = 0;
    let (areas, n_ov, parks, mut wrote) = areas_and_parks(out, src, dots, &want, &mut ntiles)?;
    for stem in ["layer-summary", "heritage-sources"] {
        let l = format!("global/heritage/{stem}");
        out.put_bytes(&l, "json", &src_bytes(out, src, stem)?)?;
        wrote.insert(l);
    }
    let stale: Vec<String> = out
        .manifest
        .keys()
        .filter(|k| AREAS.iter().any(|a| k.starts_with(&format!("layers/{}/", a.layer))) || k.starts_with("ovdata/") || k.starts_with("global/heritage/"))
        .filter(|k| !wrote.contains(*k))
        .cloned()
        .collect();
    for k in &stale {
        out.remove(k);
    }
    eprintln!("overlays: {areas} areas, {ntiles} tiles, {n_ov} ovdata, {parks} parks from {src}; {} stale dropped ({:.1?})", stale.len(), t0.elapsed());
    Ok((areas, ntiles, n_ov, parks))
}

/// The area overlays as tiles and the areas' and parks' details as ovdata, from `src`: counts of
/// areas, ovdata tiles and parks, and the logical names written.
fn areas_and_parks(
    out: &mut Out,
    src: &str,
    dots: &HashMap<u64, (u64, f64, f64)>,
    want: &(dyn Fn(u8, u32, u32) -> bool + Sync),
    ntiles: &mut usize,
) -> Result<(usize, usize, usize, std::collections::BTreeSet<String>)> {
    let t0 = std::time::Instant::now();
    let mut wrote: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // ovdata per z3 owner: per details key, records by id; and parks.
    let mut owned: BTreeMap<String, BTreeMap<&'static str, Recs>> = BTreeMap::new();
    let mut areas = 0usize;
    for a in &AREAS {
        let fc: Value = serde_json::from_slice(&src_bytes(out, src, a.file)?)?;
        let feats = fc["features"].as_array().with_context(|| format!("{}: no features", a.file))?;
        let details = match a.details {
            Some((f, _)) => details_by_i(out, src, f)?,
            None => HashMap::new(),
        };
        let mut src: Vec<IdSource> = Vec::new();
        // Per feature: it, its id (outlines: their dot's), its details record, its owner.
        let mut made: Vec<(Feature, Option<u64>, String, String)> = Vec::new();
        let mut empty = 0;
        for (n, f) in feats.iter().enumerate() {
            let Some((geom, bb)) = geom_of(&f["geometry"]) else {
                // (Nothing to draw or click: a multipolygon without polygons.)
                if f["geometry"]["coordinates"].as_array().is_some_and(|c| c.is_empty()) {
                    empty += 1;
                    continue;
                }
                bail!("{} #{n}: geometry {}", a.file, f["geometry"]["type"])
            };
            let mut props = f["properties"].as_object().cloned().unwrap_or_default();
            let i = props.remove("i").and_then(|v| v.as_u64());
            let (cx, cy) = ((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0);
            let rec = i.and_then(|i| details.get(&i));
            let mut id = None;
            if a.details.is_none() {
                // A World Heritage outline: its site's dot (id, place).
                let (dot, lon, lat) = *i.and_then(|i| dots.get(&i)).with_context(|| format!("{} #{n}: no heritage dot for record {i:?}", a.file))?;
                id = Some(dot);
                props.insert("px".into(), serde_json::json!(lon));
                props.insert("py".into(), serde_json::json!(lat));
            } else {
                let osm = rec.and_then(|r| serde_json::from_str::<Value>(r).ok()).and_then(|v| v["osm"].as_str().and_then(marks::osm_id));
                let name = props.get("name").and_then(Value::as_str).unwrap_or("");
                let reference = format!("legacy:{}|{name}|{},{}", a.layer, marks::e7(cx), marks::e7(cy));
                src.push(IdSource { osm, reference, canon: Value::Object(props.clone()).to_string() });
            }
            let own = z3_of(cx, cy);
            if a.details.is_some() {
                props.insert("own".into(), serde_json::json!(own));
            }
            let mvt: Vec<(String, MvtValue)> = props.iter().filter_map(|(k, v)| mvt_value(v).map(|m| (k.clone(), m))).collect();
            made.push((Feature { id: 0, geom, props: mvt, minzoom: 0 }, id, rec.cloned().unwrap_or_default(), own));
        }
        // Ids for the overlays with details (their own group); outlines have their dots'.
        if a.details.is_some() {
            let ids = marks::assign_ids(&src)?;
            for ((f, id, _, _), new) in made.iter_mut().zip(ids) {
                f.id = new;
                *id = Some(new);
            }
        } else {
            for (f, id, _, _) in made.iter_mut() {
                f.id = id.unwrap();
            }
        }
        if let Some((_, key)) = a.details {
            for (_, id, rec, own) in &made {
                if !rec.is_empty() {
                    owned.entry(own.clone()).or_default().entry(key).or_default().rows.push((id.unwrap(), rec.clone()));
                }
            }
        }
        areas += made.len();
        let feats: Vec<Feature> = made.into_iter().map(|m| m.0).collect();
        wrote.extend(write_tiles(out, a.layer, LAYER, &feats, want, ntiles)?);
        eprintln!("overlays: {} {} features ({empty} without geometry), {ntiles} tiles so far ({:.1?})", a.layer, feats.len(), t0.elapsed());
    }
    // Parks: owned by their box's centre.
    let pb = src_bytes(out, src, "details-park")?;
    let mut parks = 0;
    for line in pb.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        let v: Value = serde_json::from_slice(line).context("details-park: a record")?;
        let b = &v["bbox"];
        let (Some(w), Some(s), Some(e), Some(n)) = (b[0].as_f64(), b[1].as_f64(), b[2].as_f64(), b[3].as_f64()) else { continue };
        let own = z3_of((w + e) / 2.0, (s + n) / 2.0);
        let r = owned.entry(own).or_default().entry("parks").or_default();
        let k = r.rows.len() as u64;
        r.rows.push((k, String::from_utf8_lossy(line).into_owned()));
        parks += 1;
    }
    // ovdata, per z3 tile.
    let n_ov = owned.len();
    for (tile, keys) in owned {
        let u = Unit::parse(&tile).unwrap();
        let logical = format!("ovdata/{}", u.dash());
        let local = out.scratch_file(&format!("{logical}.sect"));
        let counts: BTreeMap<&str, usize> = keys.iter().map(|(k, r)| (*k, r.rows.len())).collect();
        let mut w = store::sect::SectWriter::create(&local, serde_json::json!({"fmt": 1, "tile": tile, "records": counts}))?;
        for (key, recs) in keys {
            let (ids, offs, bytes) = recs.sections();
            w.add_pod(&format!("{key}.ids"), &ids)?;
            w.add_pod(&format!("{key}.offs"), &offs)?;
            w.add(&format!("{key}.recs"), &bytes)?;
        }
        w.finish()?;
        out.put_file(&logical, "sect", &local)?;
        wrote.insert(logical);
    }
    Ok((areas, n_ov, parks, wrote))
}

/// How far from a block its terminals and ways reach (ferries.ts NEAR_M: a terminal takes the
/// colour of a line this near).
const FERRY_NEAR_KM: f64 = 30.0;

/// A line's length (km), as ferries.ts measures it.
fn length_km(c: &[[f64; 2]]) -> f64 {
    c.windows(2)
        .map(|w| {
            let k = (((w[0][1] + w[1][1]) / 2.0).to_radians()).dcos();
            ((w[1][0] - w[0][0]) * k).dhypot(w[1][1] - w[0][1]) * 111.195
        })
        .sum()
}

/// A line simplified (Douglas–Peucker) to `tol_m` metres.
fn simplify_m(c: &[[f64; 2]], tol_m: f64) -> Vec<[f64; 2]> {
    if c.len() <= 2 || tol_m <= 0.0 {
        return c.to_vec();
    }
    let k = (c.iter().map(|p| p[1]).sum::<f64>() / c.len() as f64).to_radians().dcos() * 111_195.0;
    let xy: Vec<(f64, f64)> = c.iter().map(|p| (p[0] * k, p[1] * 111_195.0)).collect();
    let mut keep = vec![false; c.len()];
    keep[0] = true;
    keep[c.len() - 1] = true;
    let mut stack = vec![(0usize, c.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay, bx, by) = (xy[a].0, xy[a].1, xy[b].0, xy[b].1);
        let (dx, dy) = (bx - ax, by - ay);
        let l2 = dx * dx + dy * dy;
        let (mut best, mut at) = (-1.0, a);
        for (i, &(px, py)) in xy.iter().enumerate().take(b).skip(a + 1) {
            let t = if l2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let d = (px - ax - t * dx).dhypot(py - ay - t * dy);
            if d > best {
                (best, at) = (d, i);
            }
        }
        if best > tol_m {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    c.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// Today's ferries as blocks (see the module's docs).
fn ferries(out: &mut Out, ntiles: &mut usize) -> Result<usize> {
    let fc: Value = serde_json::from_slice(&legacy_bytes(out, "ferries")?)?;
    let lines: serde_json::Map<String, Value> = serde_json::from_slice(&legacy_bytes(out, "ferry-lines")?)?;
    ferry_blocks(out, &fc, &lines, ntiles)
}

/// The `ferries` job (docs/phase5.md "Build"): the pass's `ferries` set exported as ferries.py
/// reads it (the Makefile's osmium steps), ferries.py with the timetables (`inputs/ferries/freq`:
/// GTFS-derived sailings and the ones looked up by hand), and the blocks from what it writes.
pub fn ferries_job(out: &mut Out, date: &str, dem: &std::path::Path) -> Result<usize> {
    use std::process::Command;
    let logical = crate::osmpass::set_name(date, "ferries");
    let set = out.path(out.get(&logical).with_context(|| format!("{logical} isn't in the build manifest"))?);
    let work = out.scratch.join("ferries-work");
    std::fs::remove_dir_all(&work).ok();
    std::fs::create_dir_all(work.join("freq"))?;
    let run = |mut c: Command, what: &str| -> Result<()> {
        let o = c.output().with_context(|| format!("run {what}"))?;
        anyhow::ensure!(o.status.success(), "{what}: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""));
        Ok(())
    };
    let osmium = |args: &[&str]| {
        let mut c = Command::new("osmium");
        c.current_dir(&work).args(args);
        c
    };
    let set_s = set.to_string_lossy().into_owned();
    run(osmium(&["tags-filter", &set_s, "w/route=ferry", "r/route=ferry", "-o", "ferries.osm.pbf", "--overwrite"]), "osmium (ferry routes)")?;
    run(osmium(&["export", "ferries.osm.pbf", "-f", "geojsonseq", "--geometry-types=linestring", "-a", "type,id", "-o", "ways.geojsonseq", "--overwrite"]), "osmium export (ways)")?;
    run(osmium(&["cat", "ferries.osm.pbf", "-t", "relation", "-f", "opl", "-o", "relations.opl", "--overwrite"]), "osmium cat (relations)")?;
    run(osmium(&["tags-filter", &set_s, "nw/amenity=ferry_terminal", "-o", "terminals.osm.pbf", "--overwrite"]), "osmium (terminals)")?;
    run(osmium(&["export", "terminals.osm.pbf", "-f", "geojsonseq", "-a", "type,id", "-o", "terminals.geojsonseq", "--overwrite"]), "osmium export (terminals)")?;
    let freq = out.root().join("inputs/ferries/freq");
    let mut n_freq = 0;
    for e in std::fs::read_dir(&freq).with_context(|| format!("{} (the timetables)", freq.display()))?.flatten() {
        if e.path().extension().is_some_and(|x| x == "json") {
            store::sys::copy_data(e.path(), work.join("freq").join(e.file_name()))?;
            n_freq += 1;
        }
    }
    let mut py = Command::new("uv");
    py.current_dir(dem).args(["run", "python", "ferries.py", "--src"]).arg(&work).arg("--out").arg(&work);
    run(py, "ferries.py")?;
    let fc: Value = serde_json::from_slice(&std::fs::read(work.join("ferries.json"))?)?;
    let lines: serde_json::Map<String, Value> = serde_json::from_slice(&std::fs::read(work.join("ferry-lines.json"))?)?;
    eprintln!("ferries: {} features, {} lines ({n_freq} timetable files)", fc["features"].as_array().map_or(0, Vec::len), lines.len());
    let mut ntiles = 0;
    ferry_blocks(out, &fc, &lines, &mut ntiles)
}

/// Ferries as blocks (see the module's docs), from ferries.json and ferry-lines.json as written.
fn ferry_blocks(out: &mut Out, fc: &Value, lines: &serde_json::Map<String, Value>, ntiles: &mut usize) -> Result<usize> {
    let feats = fc["features"].as_array().context("ferries: no features")?;
    // Per feature: its geometry (a way's line, a terminal's point), properties, id.
    struct F {
        line: Option<Vec<[f64; 2]>>,
        pt: [f64; 2],
        bbox: [f64; 4],
        props: serde_json::Map<String, Value>,
        id: u64,
    }
    let mut fs = Vec::with_capacity(feats.len());
    let mut src = Vec::new();
    let mut terminals = Vec::new();
    for (n, f) in feats.iter().enumerate() {
        let g = &f["geometry"];
        let pt = |c: &Value| -> Option<[f64; 2]> { Some([c[0].as_f64()?, c[1].as_f64()?]) };
        let mut props = f["properties"].as_object().cloned().unwrap_or_default();
        match g["type"].as_str() {
            Some("LineString") => {
                let c: Vec<[f64; 2]> = g["coordinates"].as_array().context("a ferry way")?.iter().filter_map(pt).collect();
                let way = f["id"].as_u64().with_context(|| format!("ferries #{n}: a way without its id"))?;
                let bbox = c.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
                props.insert("km".into(), serde_json::json!((length_km(&c) * 1000.0).round() / 1000.0));
                fs.push(F { pt: c[0], line: Some(c), bbox, props, id: way * 4 + 1 });
            }
            Some("Point") => {
                let p = pt(&g["coordinates"]).with_context(|| format!("ferries #{n}: a terminal without its place"))?;
                let name = props.get("n").and_then(Value::as_str).unwrap_or("");
                src.push(IdSource { osm: None, reference: format!("legacy:terminal|{name}|{},{}", marks::e7(p[0]), marks::e7(p[1])), canon: Value::Object(props.clone()).to_string() });
                terminals.push(fs.len());
                fs.push(F { line: None, pt: p, bbox: [p[0], p[1], p[0], p[1]], props, id: 0 });
            }
            t => bail!("ferries #{n}: geometry {t:?}"),
        }
    }
    for (&k, id) in terminals.iter().zip(marks::assign_ids(&src)?) {
        fs[k].id = id;
    }
    // Blocks: (zoom, simplification); which features each holds.
    let deg = |b: [i32; 4]| [b[0] as f64 * roadcore::E7, b[1] as f64 * roadcore::E7, b[2] as f64 * roadcore::E7, b[3] as f64 * roadcore::E7];
    let meets_f = |a: [f64; 4], b: [f64; 4]| a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3];
    let mut packs: BTreeMap<(&'static str, u8, u32, u32), Vec<(u8, u32, u32, Vec<u8>, u32)>> = BTreeMap::new();
    let mut blocks = 0;
    for (z, tol) in [(0u8, 5000.0), (3, 300.0), (6, 0.0)] {
        let n = 1u32 << z;
        for x in 0..n {
            for y in 0..n {
                let b = deg(grow(tile_bounds(z, x, y), FERRY_NEAR_KM));
                let mut feats = Vec::new();
                let mut ids: Vec<String> = Vec::new();
                for f in &fs {
                    // (Terminals appear from zoom 4: none in the world's block.)
                    if !meets_f(f.bbox, b) || (z == 0 && f.line.is_none()) {
                        continue;
                    }
                    let geom = match &f.line {
                        Some(c) => serde_json::json!({"type": "LineString", "coordinates": simplify_m(c, tol)}),
                        None => serde_json::json!({"type": "Point", "coordinates": f.pt}),
                    };
                    if f.line.is_some() {
                        ids.extend(f.props.get("lines").and_then(Value::as_str).unwrap_or("").split(',').filter(|s| !s.is_empty()).map(str::to_string));
                    }
                    feats.push(serde_json::json!({"type": "Feature", "id": f.id, "geometry": geom, "properties": f.props}));
                }
                if feats.is_empty() {
                    continue;
                }
                ids.sort();
                ids.dedup();
                let recs: serde_json::Map<String, Value> = ids.iter().filter_map(|i| lines.get(i).map(|l| (i.clone(), l.clone()))).collect();
                let raw = serde_json::to_vec(&serde_json::json!({"type": "FeatureCollection", "features": feats, "lines": recs}))?;
                let gz = names::mvt::gzip(&raw)?;
                packs.entry(pack_of(z, x, y)).or_default().push((z, x, y, gz, raw.len() as u32));
                blocks += 1;
            }
        }
    }
    let mut wrote = Vec::new();
    for ((scope, rz, rx, ry), mut tiles) in packs {
        tiles.sort_by_key(|t| (t.0, t.1, t.2));
        *ntiles += tiles.len();
        if let Some((l, _)) = write_pack(out, "ferries", "geojson-gz", true, scope, (rz, rx, ry), &mut tiles.into_iter())? {
            wrote.push(l);
        }
    }
    drop_stale(out, "ferries", &wrote);
    Ok(blocks)
}

/// Drops from the manifest every pack of `layer` that a job making the whole layer didn't write this
/// time: a tile it has nothing for any more (a pass without that ferry, a region removed). How many.
fn drop_stale(out: &mut Out, layer: &str, wrote: &[String]) -> usize {
    let prefix = format!("layers/{layer}/");
    let stale: Vec<String> = out.manifest.keys().filter(|k| k.starts_with(&prefix) && !wrote.contains(*k)).cloned().collect();
    for k in &stale {
        out.remove(k);
    }
    if !stale.is_empty() {
        eprintln!("{layer}: {} stale packs dropped", stale.len());
    }
    stale.len()
}

/// Writes a layer's tiles (gzipped, in packs by scope).
/// The features as vector tiles in the layer's packs; the packs' logical names.
fn write_tiles(out: &mut Out, layer: &str, mvt_layer: &str, feats: &[Feature], want: &(dyn Fn(u8, u32, u32) -> bool + Sync), ntiles: &mut usize) -> Result<Vec<String>> {
    let mut packs: BTreeMap<(&'static str, u8, u32, u32), Vec<(u8, u32, u32, Vec<u8>, u32)>> = BTreeMap::new();
    let mut err = None;
    vtgen::tiles(mvt_layer, feats, 0, MAXZ, want, &mut |z, x, y, raw| match names::mvt::gzip(&raw) {
        Ok(gz) => packs.entry(pack_of(z, x, y)).or_default().push((z, x, y, gz, raw.len() as u32)),
        Err(e) => {
            err.get_or_insert(e);
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    let mut wrote = Vec::new();
    for ((scope, rz, rx, ry), mut tiles) in packs {
        tiles.sort_by_key(|t| (t.0, t.1, t.2));
        *ntiles += tiles.len();
        if let Some((l, _)) = write_pack(out, layer, "mvt", true, scope, (rz, rx, ry), &mut tiles.into_iter())? {
            wrote.push(l);
        }
    }
    Ok(wrote)
}

/// Today's rail stops (stations.json) as vector tiles: each stop in the tiles of the zooms it shows
/// at, every stop at zoom 12. Ids: today's stops don't keep their OSM members, so references
/// (`legacy:station|name|place`).
fn stations(out: &mut Out, want: &(dyn Fn(u8, u32, u32) -> bool + Sync), ntiles: &mut usize) -> Result<usize> {
    let fc: Value = serde_json::from_slice(&legacy_bytes(out, "stations")?)?;
    let feats = fc["features"].as_array().context("stations: no features")?;
    let mut src = Vec::with_capacity(feats.len());
    let mut made = Vec::with_capacity(feats.len());
    for (n, f) in feats.iter().enumerate() {
        let c = &f["geometry"]["coordinates"];
        let (Some(lon), Some(lat)) = (c[0].as_f64(), c[1].as_f64()) else { bail!("stations #{n}: not a point") };
        let props = f["properties"].as_object().cloned().unwrap_or_default();
        let name = props.get("n").and_then(Value::as_str).unwrap_or("");
        src.push(IdSource { osm: None, reference: format!("legacy:station|{name}|{},{}", marks::e7(lon), marks::e7(lat)), canon: Value::Object(props.clone()).to_string() });
        made.push(station_feature(lon, lat, &props));
    }
    for (f, id) in made.iter_mut().zip(marks::assign_ids(&src)?) {
        f.id = id;
    }
    write_tiles(out, "stations", STATION_LAYER, &made, want, ntiles)?;
    Ok(made.len())
}

/// A stop's tile feature: its properties, shown from the first zoom whose tiles serve a view where
/// its dot shows (z + 1 ≥ mz + STOP_DZ).
fn station_feature(lon: f64, lat: f64, props: &serde_json::Map<String, Value>) -> Feature {
    let mz = props.get("mz").and_then(Value::as_f64).unwrap_or(0.0);
    let minzoom = (mz + STOP_DZ - 1.0).ceil().clamp(0.0, MAXZ as f64) as u8;
    let mvt: Vec<(String, MvtValue)> = props.iter().filter_map(|(k, v)| mvt_value(v).map(|m| (k.clone(), m))).collect();
    Feature { id: 0, geom: Geom::Points(vec![[lon, lat]]), props: mvt, minzoom }
}

/// A stop's properties as the map reads them (stations.py's: n, g, m, sp rounded, mz to 2 places,
/// en where OSM's English differs).
pub fn station_props(s: &crate::stations::Stop) -> serde_json::Map<String, Value> {
    let mut p = serde_json::Map::new();
    p.insert("n".into(), serde_json::json!(s.name));
    p.insert("g".into(), serde_json::json!(s.group));
    p.insert("m".into(), serde_json::json!(s.mask));
    p.insert("sp".into(), serde_json::json!(crate::interest::py_round(s.spacing, 0) as i64));
    p.insert("mz".into(), serde_json::json!(crate::interest::py_round(s.mz(), 2)));
    if let Some(en) = &s.en {
        p.insert("en".into(), serde_json::json!(en));
    }
    p
}

/// The `stations` job (docs/phase5.md "Build"): the pass's `rail` set's stops within the coverage
/// (+ 20 km, the hi tiles' reach) as the stations' tiles; each stop's id its lowest member's
/// (docs/phase5.md "Ids").
pub fn stations_job(out: &mut Out, date: &str, geojson: Option<&std::path::Path>) -> Result<(usize, usize)> {
    let logical = crate::osmpass::set_name(date, "rail");
    let set = out.path(out.get(&logical).with_context(|| format!("{logical} isn't in the build manifest"))?);
    let t0 = std::time::Instant::now();
    let all = crate::stations::stops(&set)?;
    let cover = hi_cover(out);
    let kept: Vec<&crate::stations::Stop> = all
        .iter()
        .filter(|s| {
            let u = Unit::of_point(6, [marks::e7(s.lon), marks::e7(s.lat)]);
            cover.contains(&(u.x, u.y))
        })
        .collect();
    eprintln!("stations: {} stops worldwide, {} within the coverage ({:.1?})", all.len(), kept.len(), t0.elapsed());
    if let Some(p) = geojson {
        let feats: Vec<Value> = kept
            .iter()
            .map(|s| serde_json::json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": [crate::interest::py_round(s.lon, 6), crate::interest::py_round(s.lat, 6)]}, "properties": station_props(s)}))
            .collect();
        std::fs::write(p, serde_json::to_vec(&serde_json::json!({"type": "FeatureCollection", "features": feats}))?)?;
    }
    let mut made: Vec<Feature> = kept
        .iter()
        .map(|s| {
            let mut f = station_feature(s.lon, s.lat, &station_props(s));
            let (ty, id) = s.members[0];
            f.id = id as u64 * 4 + ty as u64;
            f
        })
        .collect();
    made.sort_by_key(|f| f.id);
    let want = |z: u8, x: u32, y: u32| z < 9 || cover.contains(&(x >> (z - 6), y >> (z - 6)));
    let mut ntiles = 0;
    let wrote = write_tiles(out, "stations", STATION_LAYER, &made, &want, &mut ntiles)?;
    drop_stale(out, "stations", &wrote);
    Ok((kept.len(), ntiles))
}
