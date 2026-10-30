//! Map labels by importance: places, provinces and states, water and parks as label points, each
//! with the zoom from which it may show (mz), for the label density control (web: Layers → Map).
//! The labels step (`scenic-build labels`) runs it on the OSM pass's labels set.
//!
//! A label shows from the zoom where its isolation, the distance to the nearest label of its kind
//! that matters more (crate::interest), spans the spacing set in the app: mz + log2(spacing in px),
//! as the stops & sights' names do. The most important for miles around shows first, and labels are
//! as dense at every zoom: a hamlet alone in the north shows zoomed out, one beside a town only
//! close in.
//!
//! What matters more, per kind (k; class c):
//!   place  city > town > village > suburb > hamlet, quarter > neighbourhood > locality, isolated
//!          dwelling; within a class, population (a capital first)
//!   state  provinces and states, by population
//!   water  oceans and seas, then by area: lakes, reservoirs, bays and straits alike (a bay or
//!          strait mapped as a point has none: as one of 1 km², 10 km² with a Wikipedia article;
//!          not by Wikidata item, which most Canadian bays have from the national names database)
//!   park   national parks, other protected areas and nature reserves by area, a national park as
//!          one ten times its size
//! Isolation alone would hide a big place beside a bigger one (Laval beside Montréal, Yokohama
//! beside Tokyo) until close in, so a place also shows from a zoom by its population (18 − 2 log10
//! population at the default spacing: a million people from zoom 6, 100,000 from 8), and an area
//! once it spans BIG_PX on screen (√area), whichever comes first: mz is the earlier, both moving
//! with the spacing; collisions decide between neighbours, the most important first. An area's
//! name also waits until the area spans SIZE_PX (ms: the zoom it does), however far the next one
//! is: no pond named zoomed out for want of a lake near. Water and parks of the same name within
//! DEDUPE_KM of a more important one are left out (a lake mapped twice). Rivers keep the basemap's
//! labels (along their lines).
//!
//! Output: a roadcore archive of gzip'd Mapbox vector tiles, layer "l", z0–12, z12 overzoomed
//! beyond: a label is in the tiles from the zoom where it could show at MIN_PX, and in every z12
//! tile. Properties: n (name), en (English: its own name:en only, as the server attaches the
//! translations when serving), kana, l (OSM's languages of the name), o (OSM object), k, c, mz,
//! ms (areas), s (importance, the placement order). A feature's id is the label's row, the same
//! in every tile it's in.
//!
//! The areas are osmium's (its export: libosmium's assembler), their area and the point inside
//! each GEOS's (shapely's `area` and `representative_point`, ported: [`area`], [`interior_point`]).
//! Every number is computed in a fixed order (numpy's, where the layer began in Python), so a pass's
//! tiles are the same bytes once unzipped, run after run; the gzip header carries no time.

use crate::agent::jobs;
use crate::interest::{isolation, min_zoom, py_round};
use crate::pyjson::{self, Py};
use crate::pytext;
use crate::timings::phase;
use anyhow::{ensure, Context, Result};
use det::Det;
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::Stdio;

pub const MAXZ: u32 = 12;
/// The smallest spacing the app allows (px): a label is in the tiles from the zoom it could show at it.
const MIN_PX: f64 = 16.0;
const EXTENT: f64 = 4096.0;
/// Seas and oceans above any water area (whose scores are log10 km² + 4: up to ~10).
const SEA_SCORE: f64 = 20.0;
/// An area's name shows once the area spans SIZE_PX (√area), whatever the spacing (the app scales
/// it by the square root of the kind's spacing over DEFAULT_PX), and from BIG_PX however near a
/// more important one is.
const SIZE_PX: f64 = 20.0;
const BIG_PX: f64 = 120.0;
/// The app's default label spacing (web/src/state.ts DEFAULT_DENSITY): mz is relative to it.
const DEFAULT_PX: f64 = 90.0;
/// A place with a population shows from zoom POP_ZOOM.0 − POP_ZOOM.1 log10(population) at the
/// default spacing (a capital half a zoom sooner) however near a more important one is.
const POP_ZOOM: (f64, f64) = (18.0, 2.0);
const DEDUPE_KM: f64 = 30.0;

fn place_rank(p: &str) -> Option<u32> {
    Some(match p {
        "city" => 7,
        "town" => 6,
        "village" => 5,
        "suburb" => 4,
        "hamlet" | "quarter" => 3,
        "neighbourhood" => 2,
        "locality" => 1,
        "isolated_dwelling" => 0,
        _ => return None,
    })
}

/// natural=water that isn't a lake: a river's or canal's water area (named along its waterway line).
const NOT_LAKE: [&str; 13] = [
    "river",
    "stream",
    "canal",
    "ditch",
    "drain",
    "wastewater",
    "riverbank",
    "moat",
    "fountain",
    "rapids",
    "fish_pass",
    "lock",
    "basin",
];

/// Label points (nodes alone).
const NODE_FILTERS: [&str; 2] = [
    "n/place=city,town,village,suburb,hamlet,quarter,neighbourhood,locality,isolated_dwelling,state,province,ocean,sea",
    "n/natural=bay,strait",
];
/// Areas (with their outlines' ways and nodes).
const AREA_FILTERS: [&str; 3] =
    ["wr/natural=water,bay,strait", "wr/boundary=national_park,protected_area", "wr/leisure=nature_reserve"];

/// An object's tags as a Python dict of them: in their order, a repeated key in its first place
/// with its last value.
#[derive(Default)]
struct Tags(Vec<(String, String)>);

impl Tags {
    fn push(&mut self, k: &str, v: &str) {
        match self.0.iter_mut().find(|(key, _)| key == k) {
            Some(e) => e.1 = v.to_string(),
            None => self.0.push((k.to_string(), v.to_string())),
        }
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
    }
}

/// One label, before it's ranked.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    kind: &'static str,
    class: String,
    name: String,
    lon: f64,
    lat: f64,
    score: f64,
    /// Its own English (stripped).
    en: Option<String>,
    /// Its area (km²): an area's, or a bay's or strait's point's stand-in.
    km2: Option<f64>,
    /// The zoom by population.
    popz: Option<f64>,
    kana: Option<String>,
    langs: Option<String>,
    /// The OSM object (`n123`, `w123`, `r123`).
    oid: String,
}

/// Python's `float(v.replace(",", "").replace(" ", "").split(";")[0])`, at least 0; 0 when that
/// fails or there's none.
fn population(v: Option<&str>) -> f64 {
    let Some(v) = v.filter(|v| !v.is_empty()) else { return 0.0 };
    let s = v.replace([',', ' '], "");
    match pytext::float(s.split(';').next().unwrap_or("")) {
        Some(x) if x > 0.0 => x,
        _ => 0.0,
    }
}

/// Script and region subtags after a language that still name the name's language (name:zh-Hant
/// equal to name is Chinese); any other (-Latn, _rm, _kana) is a transliteration.
const SAME_LANGUAGE: [&str; 7] = ["Hant", "Hans", "TW", "HK", "CN", "MO", "SG"];
const KANA_KEYS: [&str; 3] = ["name:ja-Hira", "name:ja_kana", "name:ja-Kana"];

/// How much a name:… key that romanises the name is preferred (lower first).
fn romanised(key: &str) -> Option<u8> {
    let rest = key.strip_prefix("name:")?;
    if rest.ends_with("_rm") {
        return Some(1);
    }
    let parts: Vec<&str> = rest.split('-').collect();
    match parts.len() {
        2 if parts[1] == "Latn" => Some(0),
        n if n > 2 && parts[1] == "Latn" => Some(2),
        _ => None,
    }
}

/// A thing's own English (name:en, else its romanised name), its kana reading (when it has no
/// English), and the languages OSM gives its name (comma-separated), from its tags.
fn own(t: &Tags, name: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut en = t.get("name:en").map(pytext::strip).filter(|s| !s.is_empty()).map(str::to_string);
    if en.is_none() {
        en =
            t.0.iter()
                .filter(|(_, v)| !pytext::strip(v).is_empty())
                .filter_map(|(k, v)| Some((romanised(k)?, v.as_str())))
                .min()
                .map(|(_, v)| pytext::strip(v).to_string());
    }
    let kana = match en {
        Some(_) => None,
        None => KANA_KEYS.iter().find_map(|k| t.get(k).filter(|v| !pytext::strip(v).is_empty())).map(str::to_string),
    };
    let mut langs: Vec<String> = Vec::new();
    for (k, v) in &t.0 {
        let Some(rest) = k.strip_prefix("name:") else { continue };
        if v != name {
            continue;
        }
        let mut parts = rest.split(['-', '_']);
        let base = parts.next().unwrap_or("").to_lowercase();
        if (2..=4).contains(&base.chars().count())
            && base.chars().all(|c| c.is_ascii_alphabetic())
            && parts.all(|p| SAME_LANGUAGE.contains(&p))
            && !langs.contains(&base)
        {
            langs.push(base);
        }
    }
    (en, kana, (!langs.is_empty()).then(|| langs.join(",")))
}

/// A label node's row, if it's one.
fn point(id: i64, x: i32, y: i32, t: &Tags) -> Option<Row> {
    let name = t.get("name").filter(|n| !n.is_empty())?;
    // (libosmium's Location::valid.)
    if !(-1_800_000_000..=1_800_000_000).contains(&x) || !(-900_000_000..=900_000_000).contains(&y) {
        return None;
    }
    let (lon, lat) = (x as f64 / 1e7, y as f64 / 1e7);
    let (en, kana, langs) = own(t, name);
    let p = t.get("place");
    let row = |kind, class: &str, score, km2, popz| Row {
        kind,
        class: class.to_string(),
        name: name.to_string(),
        lon,
        lat,
        score,
        en: en.clone(),
        km2,
        popz,
        kana: kana.clone(),
        langs: langs.clone(),
        oid: format!("n{id}"),
    };
    if let Some(rank) = p.and_then(place_rank) {
        let cap = if matches!(t.get("capital"), Some("yes" | "2" | "3" | "4")) { 0.95 } else { 0.0 };
        let l = (population(t.get("population")) + 1.0).dlog10();
        let pop = if l < 8.9 { l } else { 8.9 };
        let popz = (pop > 0.0).then(|| POP_ZOOM.0 - POP_ZOOM.1 * pop - if cap != 0.0 { 0.5 } else { 0.0 });
        Some(row("place", p.unwrap(), (rank * 10) as f64 + pop + cap, None, popz))
    } else if let Some(p @ ("state" | "province")) = p {
        Some(row("state", p, (population(t.get("population")) + 1.0).dlog10(), None, None))
    } else if let Some(p @ ("ocean" | "sea")) = p {
        Some(row("water", p, SEA_SCORE, None, None))
    } else if let Some(n @ ("bay" | "strait")) = t.get("natural") {
        let km2 = if t.get("wikipedia").is_some_and(|w| !w.is_empty()) { 10.0 } else { 1.0 };
        Some(row("water", n, km2.dlog10() + 4.0, Some(km2), None))
    } else {
        None
    }
}

type P = [f64; 2];

/// GEOS's `Area::ofRing`: the shoelace sum relative to the first point, unsigned.
fn ring_area(r: &[P]) -> f64 {
    let n = r.len();
    if n < 3 {
        return 0.0;
    }
    let x0 = r[0][0];
    let mut sum = 0.0;
    for i in 1..n - 1 {
        let x = r[i][0] - x0;
        let (y1, y2) = (r[i + 1][1], r[i - 1][1]);
        sum += x * (y2 - y1);
    }
    (sum / 2.0).abs()
}

/// GEOS's `MultiPolygon::getArea`: each polygon's shell less its holes, summed in order.
fn area(polys: &[Vec<Vec<P>>]) -> f64 {
    let mut a = 0.0;
    for p in polys {
        let mut pa = 0.0;
        for (i, r) in p.iter().enumerate() {
            if i == 0 {
                pa += ring_area(r);
            } else {
                pa -= ring_area(r);
            }
        }
        a += pa;
    }
    a
}

fn avg(a: f64, b: f64) -> f64 {
    (a + b) / 2.0
}

/// GEOS 3.13's InteriorPointArea (shapely's `representative_point`): per polygon, the midpoint of
/// the widest section of a horizontal line through it (at a height between the vertices nearest
/// its envelope's middle), the widest of all polygons; a polygon with no section (no area) has its
/// first point.
fn interior_point(polys: &[Vec<Vec<P>>]) -> Option<P> {
    let mut max_width = -1.0;
    let mut best = None;
    for p in polys {
        let Some(shell) = p.first().filter(|s| !s.is_empty()) else { continue };
        let (lo, hi) = shell.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), q| (l.min(q[1]), h.max(q[1])));
        // ScanLineYOrdinateFinder.
        let (mut lo_y, mut hi_y) = (lo, hi);
        let centre = avg(lo_y, hi_y);
        for q in p.iter().flatten() {
            let y = q[1];
            if y <= centre {
                if y > lo_y {
                    lo_y = y;
                }
            } else if y < hi_y {
                hi_y = y;
            }
        }
        let scan_y = avg(hi_y, lo_y);
        let mut crossings: Vec<f64> = Vec::new();
        for r in p {
            let (rlo, rhi) = r.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), q| (l.min(q[1]), h.max(q[1])));
            if !(scan_y >= rlo && scan_y <= rhi) {
                continue;
            }
            for w in r.windows(2) {
                let (p0, p1) = (w[0], w[1]);
                if (p0[1] > scan_y && p1[1] > scan_y) || (p0[1] < scan_y && p1[1] < scan_y) {
                    continue;
                }
                // Counted: not horizontal, and a vertex on the line counted once.
                if p0[1] == p1[1] || (p0[1] == scan_y && p1[1] < scan_y) || (p1[1] == scan_y && p0[1] < scan_y) {
                    continue;
                }
                crossings.push(if p0[0] == p1[0] {
                    p0[0]
                } else {
                    let m = (p1[1] - p0[1]) / (p1[0] - p0[0]);
                    p0[0] + ((scan_y - p0[1]) / m)
                });
            }
        }
        let mut point = shell[0];
        let mut width = 0.0;
        crossings.sort_by(f64::total_cmp);
        for c in crossings.chunks_exact(2) {
            if c[1] - c[0] > width {
                width = c[1] - c[0];
                point = [avg(c[0], c[1]), scan_y];
            }
        }
        if width > max_width {
            max_width = width;
            best = Some(point);
        }
    }
    best
}

/// An area's row from one feature of osmium's export, if it's a label.
fn area_row(line: &str) -> Result<Option<Row>> {
    let line = line.trim_matches(|c: char| c == '\u{1e}' || c.is_ascii_whitespace());
    if line.is_empty() {
        return Ok(None);
    }
    let f = pyjson::loads(line)?;
    let mut t = Tags::default();
    let (mut ty, mut id) = ("", String::new());
    for (k, v) in f.get("properties").and_then(dict).context("no properties")? {
        match (k.as_str(), v) {
            ("@type", Py::Str(s)) => ty = if s == "way" { "w" } else { "r" },
            ("@id", Py::Int(i)) => id = i.clone(),
            (_, Py::Str(s)) => t.push(k, s),
            _ => {}
        }
    }
    let Some(name) = t.get("name").filter(|n| !n.is_empty()) else { return Ok(None) };
    let nat = t.get("natural");
    let (kind, class) = if nat == Some("water") && !NOT_LAKE.contains(&t.get("water").unwrap_or("lake")) {
        ("water", "lake")
    } else if let Some(n @ ("bay" | "strait")) = nat {
        ("water", n)
    } else if t.get("boundary") == Some("national_park") {
        ("park", "national_park")
    } else if t.get("boundary") == Some("protected_area") || t.get("leisure") == Some("nature_reserve") {
        ("park", "protected_area")
    } else {
        return Ok(None);
    };
    let g = f.get("geometry").context("no geometry")?;
    let coords = g.get("coordinates").and_then(Py::list).context("no coordinates")?;
    let pt = |p: &Py| -> Option<P> {
        let p = p.list()?;
        Some([p.first()?.number()?, p.get(1)?.number()?])
    };
    let ring = |r: &Py| -> Result<Vec<P>> {
        r.list().context("not a ring")?.iter().map(|p| pt(p).context("not a point")).collect()
    };
    let poly = |p: &Py| -> Result<Vec<Vec<P>>> { p.list().context("not a polygon")?.iter().map(ring).collect() };
    let polys: Vec<Vec<Vec<P>>> = match g.get("type").and_then(Py::as_str) {
        Some("MultiPolygon") => coords.iter().map(poly).collect::<Result<_>>()?,
        Some("Polygon") => vec![poly(&Py::List(coords.clone()))?],
        _ => return Ok(None),
    };
    // (A ring of fewer than 4 points isn't one to GEOS: the area was left out.)
    if polys.iter().flatten().any(|r| r.len() < 4) || polys.iter().all(|p| p.is_empty()) {
        return Ok(None);
    }
    let Some(pt) = interior_point(&polys) else { return Ok(None) };
    let km2 = area(&polys) * 111.32 * 110.57 * pt[1].to_radians().dcos();
    let score = (km2 + 1e-4).dlog10() + 4.0 + if class == "national_park" { 1.0 } else { 0.0 };
    let (en, kana, langs) = own(&t, name);
    Ok(Some(Row {
        kind,
        class: class.to_string(),
        name: name.to_string(),
        lon: pt[0],
        lat: pt[1],
        score,
        en,
        km2: Some(km2),
        popz: None,
        kana,
        langs,
        oid: format!("{ty}{id}"),
    }))
}

fn dict(p: &Py) -> Option<&Vec<(String, Py)>> {
    match p {
        Py::Dict(d) => Some(d),
        _ => None,
    }
}

/// Which to keep: water and parks but those within DEDUPE_KM of one of the same kind and name that
/// matters more.
fn dedupe(rows: &[Row]) -> Vec<bool> {
    let mut keep = vec![true; rows.len()];
    let mut groups: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        if matches!(r.kind, "water" | "park") {
            groups.entry((r.kind, &r.name)).or_default().push(i);
        }
    }
    for mut idx in groups.into_values() {
        if idx.len() < 2 {
            continue;
        }
        idx.sort_by(|&a, &b| (-rows[a].score).total_cmp(&-rows[b].score));
        let mut kept: Vec<P> = Vec::new();
        for i in idx {
            let (x, y) = (rows[i].lon * 111.32 * rows[i].lat.to_radians().dcos(), rows[i].lat * 110.57);
            if kept.iter().any(|k| (k[0] - x).dhypot(k[1] - y) < DEDUPE_KM) {
                keep[i] = false;
                continue;
            }
            kept.push([x, y]);
        }
    }
    keep
}

// ---- Mapbox vector tiles (points, one layer): as web/src/mvt.ts --------------------------------

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v > 0x7F {
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn field(out: &mut Vec<u8>, num: u64, wire: u64) {
    varint(out, (num << 3) | wire);
}

fn ld(out: &mut Vec<u8>, num: u64, b: &[u8]) {
    field(out, num, 2);
    varint(out, b.len() as u64);
    out.extend_from_slice(b);
}

fn zz(n: i64) -> u64 {
    ((n << 1) ^ (n >> 63)) as u64
}

/// A property's value.
enum Val<'a> {
    Str(&'a str),
    F(f64),
}

/// One point feature: its position in the tile, id and properties (those that are there, in order).
struct Feature<'a> {
    x: i64,
    y: i64,
    id: u64,
    props: Vec<(&'static str, Val<'a>)>,
}

/// One layer "l" of point features, as a tile (each key and value once, in the order first used).
fn encode(points: &[Feature]) -> Vec<u8> {
    let mut keys: Vec<&str> = Vec::new();
    let mut vals: HashMap<(bool, &str, u64), usize> = HashMap::new();
    let mut vmsgs: Vec<Vec<u8>> = Vec::new();
    let mut layer = Vec::new();
    field(&mut layer, 15, 0);
    varint(&mut layer, 2);
    ld(&mut layer, 1, b"l");
    for f in points {
        let mut tags = Vec::new();
        for (k, v) in &f.props {
            let ki = keys.iter().position(|x| x == k).unwrap_or_else(|| {
                keys.push(k);
                keys.len() - 1
            });
            // (Floats as Python's dict keys: 0.0 and -0.0 are one.)
            let key = match v {
                Val::Str(s) => (true, *s, 0),
                Val::F(x) => (false, "", if *x == 0.0 { 0 } else { x.to_bits() }),
            };
            let vi = *vals.entry(key).or_insert_with(|| {
                let mut m = Vec::new();
                match v {
                    Val::Str(s) => ld(&mut m, 1, s.as_bytes()),
                    Val::F(x) => {
                        field(&mut m, 3, 1);
                        m.extend_from_slice(&x.to_le_bytes());
                    }
                }
                vmsgs.push(m);
                vmsgs.len() - 1
            });
            varint(&mut tags, ki as u64);
            varint(&mut tags, vi as u64);
        }
        let mut feat = Vec::new();
        field(&mut feat, 1, 0);
        varint(&mut feat, f.id);
        ld(&mut feat, 2, &tags);
        field(&mut feat, 3, 0);
        varint(&mut feat, 1);
        let mut geom = Vec::new();
        varint(&mut geom, 9);
        varint(&mut geom, zz(f.x));
        varint(&mut geom, zz(f.y));
        ld(&mut feat, 4, &geom);
        ld(&mut layer, 2, &feat);
    }
    for k in keys {
        ld(&mut layer, 3, k.as_bytes());
    }
    for m in vmsgs {
        ld(&mut layer, 4, &m);
    }
    field(&mut layer, 5, 0);
    varint(&mut layer, EXTENT as u64);
    let mut out = Vec::new();
    ld(&mut out, 3, &layer);
    out
}

fn gzip(b: &[u8]) -> Result<Vec<u8>> {
    let mut e = flate2::write::GzEncoder::new(Vec::with_capacity(b.len() / 2 + 64), flate2::Compression::new(6));
    e.write_all(b)?;
    Ok(e.finish()?)
}

/// Runs osmium with `args`, its progress (`--progress`) said as the stage's under way.
fn osmium(args: &[&std::ffi::OsStr], what: &str) -> Result<()> {
    let mut c = crate::osmpass::osmium();
    c.args(args).arg("--progress");
    let st = jobs::run_watched(&mut c, jobs::within).with_context(|| format!("run {what}"))?;
    ensure!(st.success(), "{what} failed: {st}");
    Ok(())
}

const STEPS: u64 = 5;

/// The labels of `src` (the pass's labels set, OSM PBF) as an archive at `out`, using `work` for
/// osmium's filtered files.
pub fn run(src: &Path, out: &Path, work: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    std::fs::create_dir_all(work)?;
    let (nodes, all_areas, areas) =
        (work.join("labels-nodes.osm.pbf"), work.join("labels-areas-all.osm.pbf"), work.join("labels-areas.osm.pbf"));
    let os = |s: &'static str| std::ffi::OsStr::new(s);
    jobs::stage(0, STEPS, "steps (the label points and areas filtered)");
    {
        let _p = phase("filtering the label points (osmium)");
        let mut a = vec![os("tags-filter"), os("--overwrite"), os("-R"), src.as_os_str()];
        a.extend(NODE_FILTERS.map(os));
        a.extend([os("-o"), nodes.as_os_str()]);
        osmium(&a, "osmium tags-filter (the label points)")?;
    }
    // Only named areas become labels: the unnamed (most lakes and ponds) go before their nodes are
    // read. Members of a named relation stay (as its references), named or not.
    jobs::stage(1, STEPS, "steps (the named areas filtered)");
    {
        let _p = phase("filtering the named areas (osmium)");
        let mut a = vec![os("tags-filter"), os("--overwrite"), src.as_os_str()];
        a.extend(AREA_FILTERS.map(os));
        a.extend([os("-o"), all_areas.as_os_str()]);
        osmium(&a, "osmium tags-filter (the areas)")?;
        let a =
            [os("tags-filter"), os("--overwrite"), all_areas.as_os_str(), os("wr/name"), os("-o"), areas.as_os_str()];
        osmium(&a, "osmium tags-filter (the named areas)")?;
        std::fs::remove_file(&all_areas).ok();
    }
    jobs::stage(2, STEPS, "steps (reading the points)");
    let mut rows: Vec<Row> = Vec::new();
    {
        let _p = phase("reading the label points");
        let r = osmpbf::ElementReader::from_path(&nodes).with_context(|| format!("open {}", nodes.display()))?;
        r.for_each(|e| {
            let mut t = Tags::default();
            let row = match e {
                osmpbf::Element::DenseNode(n) => {
                    n.tags().for_each(|(k, v)| t.push(k, v));
                    point(n.id(), n.decimicro_lon(), n.decimicro_lat(), &t)
                }
                osmpbf::Element::Node(n) => {
                    n.tags().for_each(|(k, v)| t.push(k, v));
                    point(n.id(), n.decimicro_lon(), n.decimicro_lat(), &t)
                }
                _ => None,
            };
            rows.extend(row);
        })?;
        std::fs::remove_file(&nodes).ok();
    }
    eprintln!("{} label points ({:.0?})", rows.len(), t0.elapsed());
    // The areas as osmium assembles them, read as they come (in its order). Node locations in a
    // sparse index on disk (16 bytes a node): a dense array is as big as the highest node id.
    jobs::stage(3, STEPS, "steps (reading the named areas)");
    {
        let _p = phase("reading the named areas");
        let idx = work.join("labels-nodes.idx");
        let mut c = crate::osmpass::osmium();
        c.args(["export", "-f", "geojsonseq", "--geometry-types=polygon", "-a", "type,id", "-o", "-", "-i"])
            .arg(format!("sparse_file_array,{}", idx.display()))
            .arg(&areas)
            .stdout(Stdio::piped());
        let mut child = store::spawn::spawn(&mut c).context("run osmium export (the named areas)")?;
        let mut lines = std::io::BufReader::with_capacity(8 << 20, child.stdout.take().context("no stdout")?).lines();
        loop {
            let chunk: Vec<String> = lines.by_ref().take(20_000).collect::<std::io::Result<_>>()?;
            if chunk.is_empty() {
                break;
            }
            let got: Vec<Option<Row>> = chunk.par_iter().map(|l| area_row(l)).collect::<Result<_>>()?;
            rows.extend(got.into_iter().flatten());
        }
        let st = child.wait()?;
        ensure!(st.success(), "osmium export (the named areas) failed: {st}");
        std::fs::remove_file(&idx).ok();
        std::fs::remove_file(&areas).ok();
    }
    eprintln!("{} labels read ({:.0?})", rows.len(), t0.elapsed());
    let n0 = rows.len();
    {
        let _p = phase("leaving out the duplicates");
        let keep = dedupe(&rows);
        let mut k = keep.into_iter();
        rows.retain(|_| k.next().unwrap());
    }
    eprintln!("{} duplicates left out ({:.0?})", n0 - rows.len(), t0.elapsed());
    jobs::stage(4, STEPS, "steps (each label's isolation and tiles)");
    let p = phase("measuring the isolation");
    let n = rows.len();
    // Areas: the zoom where √area spans one pixel; ms where it spans SIZE_PX.
    let zs: Vec<f64> =
        rows.iter().map(|r| r.km2.filter(|&a| a != 0.0).map_or(f64::NAN, |a| min_zoom(r.lat, a.sqrt()))).collect();
    let ms: Vec<f64> = zs.iter().map(|z| z + SIZE_PX.dlog2()).collect();
    // The zoom by size or population, relative to the default spacing as mz is.
    let absz: Vec<f64> =
        (0..n).map(|i| (zs[i] + BIG_PX.dlog2()).min(rows[i].popz.unwrap_or(f64::NAN)) - DEFAULT_PX.dlog2()).collect();
    let mut mz = vec![0.0; n];
    for k in ["place", "state", "water", "park"] {
        let idx: Vec<usize> = (0..n).filter(|&i| rows[i].kind == k).collect();
        if idx.is_empty() {
            continue;
        }
        let col = |f: fn(&Row) -> f64| idx.iter().map(|&i| f(&rows[i])).collect::<Vec<f64>>();
        let ia = isolation(&col(|r| r.lon), &col(|r| r.lat), &col(|r| r.score));
        for (&i, d) in idx.iter().zip(ia) {
            mz[i] = min_zoom(rows[i].lat, d).min(absz[i]);
        }
        eprintln!("  {k}: {}, isolation done ({:.0?})", idx.len(), t0.elapsed());
    }
    let tx: Vec<f64> = rows.iter().map(|r| (r.lon + 180.0) / 360.0).collect();
    let ty: Vec<f64> = rows
        .iter()
        .map(|r| {
            let lat = r.lat.clamp(-85.05, 85.05);
            0.5 - (std::f64::consts::PI / 4.0 + lat.to_radians() / 2.0).dtan().dln() / (2.0 * std::f64::consts::PI)
        })
        .collect();
    // (ms shifts by half the spacing's log2: up to this much sooner at MIN_PX.)
    let shift = 0.5 * (DEFAULT_PX / MIN_PX).dlog2();
    let first: Vec<u32> =
        (0..n).map(|i| (mz[i] + MIN_PX.dlog2()).max(ms[i] - shift).floor().clamp(0.0, MAXZ as f64) as u32).collect();
    drop(p);
    let p = phase("placing the labels in tiles");
    let mut tiles: HashMap<(u32, u32, u32), Vec<u32>> = HashMap::new();
    for i in 0..n {
        for z in first[i]..=MAXZ {
            let m = 1u32 << z;
            let cell = |t: f64| ((t * m as f64) as u32).min(m - 1);
            tiles.entry((z, cell(tx[i]), cell(ty[i]))).or_default().push(i as u32);
        }
    }
    drop(p);
    eprintln!("{} tiles ({:.0?})", tiles.len(), t0.elapsed());
    let english: Vec<Option<&str>> =
        rows.iter().map(|r| r.en.as_deref().filter(|e| pytext::differs(&r.name, Some(e))).map(pytext::strip)).collect();
    let _p = phase("making and writing the label tiles");
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let meta = pyjson::dumps_spaced(&Py::Dict(vec![
        ("format".into(), Py::Str("pbf".into())),
        ("layer".into(), Py::Str("l".into())),
        ("maxzoom".into(), Py::Int(MAXZ.to_string())),
    ]));
    let tmp = out.with_extension("tiles.tmp");
    let mut w = roadcore::archive::ArchiveWriter::create(&tmp, &meta)?;
    let mut keys: Vec<(u32, u32, u32)> = tiles.keys().copied().collect();
    keys.sort_unstable();
    let total = keys.len();
    let mut made = 0;
    // (In batches of a hundredth, said as each is written.)
    for batch in keys.chunks((total / 100).max(4096)) {
        let blobs: Vec<(Vec<u8>, usize)> = batch
            .par_iter()
            .map(|&(z, x, y)| {
                let m = (1u64 << z) as f64;
                let mut ids = tiles[&(z, x, y)].clone();
                ids.sort_by(|&a, &b| (-rows[a as usize].score).total_cmp(&-rows[b as usize].score));
                let feats: Vec<Feature> = ids
                    .iter()
                    .map(|&i| {
                        let i = i as usize;
                        let r = &rows[i];
                        let props = [
                            ("n", Some(Val::Str(&r.name))),
                            ("en", english[i].map(Val::Str)),
                            ("kana", r.kana.as_deref().filter(|_| english[i].is_none()).map(Val::Str)),
                            ("l", r.langs.as_deref().map(Val::Str)),
                            ("o", Some(Val::Str(&r.oid))),
                            ("k", Some(Val::Str(r.kind))),
                            ("c", Some(Val::Str(&r.class))),
                            ("mz", Some(Val::F(py_round(mz[i], 2)))),
                            ("ms", (!ms[i].is_nan()).then(|| Val::F(py_round(ms[i], 2)))),
                            ("s", Some(Val::F(py_round(r.score, 2)))),
                        ];
                        Feature {
                            x: ((tx[i] * m - x as f64) * EXTENT).round_ties_even() as i64,
                            y: ((ty[i] * m - y as f64) * EXTENT).round_ties_even() as i64,
                            id: i as u64,
                            props: props.into_iter().filter_map(|(k, v)| Some((k, v?))).collect(),
                        }
                    })
                    .collect();
                let raw = encode(&feats);
                Ok((gzip(&raw)?, raw.len()))
            })
            .collect::<Result<_>>()?;
        for (&(z, x, y), (gz, raw)) in batch.iter().zip(blobs) {
            w.add(z as u8, x, y, &gz, raw)?;
        }
        made += batch.len();
        jobs::stage(made as u64, total as u64, "label tiles written");
    }
    w.finish()?;
    std::fs::rename(&tmp, out)?;
    eprintln!(
        "labels.tiles: {} labels in {} tiles, {:.0} MB ({:.0?})",
        n,
        total,
        std::fs::metadata(out)?.len() as f64 / 1e6,
        t0.elapsed()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(p: &[(f64, f64)]) -> Vec<P> {
        p.iter().map(|&(x, y)| [x, y]).collect()
    }

    /// shapely 2.1.2's (GEOS 3.13.1's) `representative_point` and `area`.
    #[test]
    fn interior_point_and_area_as_geos() {
        let l = vec![ring(&[(0., 0.), (3., 0.), (3., 1.), (1., 1.), (1., 3.), (0., 3.), (0., 0.)])];
        let h = vec![
            ring(&[(0., 0.), (4., 0.), (4., 4.), (0., 4.), (0., 0.)]),
            ring(&[(1., 1.), (3., 1.), (3., 3.), (1., 3.), (1., 1.)]),
        ];
        let t = vec![ring(&[(10., 10.), (10.5, 10.1), (10.2, 10.7), (10., 10.)])];
        let one = |p: &Vec<Vec<P>>| (interior_point(std::slice::from_ref(p)), area(std::slice::from_ref(p)));
        assert_eq!(one(&l), (Some([0.5, 2.0]), 5.0));
        assert_eq!(one(&h), (Some([0.5, 2.0]), 12.0));
        assert_eq!(one(&t), (Some([10.232142857142858, 10.399999999999999]), 0.1649999999999999));
        let all = [l, h, t];
        assert_eq!((interior_point(&all), area(&all)), (Some([0.5, 2.0]), 17.165));
    }

    fn tags(t: &[(&str, &str)]) -> Tags {
        let mut out = Tags::default();
        t.iter().for_each(|(k, v)| out.push(k, v));
        out
    }

    #[test]
    fn own_english_kana_and_languages() {
        let t = tags(&[
            ("name", "東京"),
            ("name:ja", "東京"),
            ("name:ja_rm", "Tokyo"),
            ("name:ja-Latn", "Tōkyō "),
            ("name:zh-Hant", "東京"),
        ]);
        assert_eq!(own(&t, "東京"), (Some("Tōkyō".into()), None, Some("ja,zh".into())));
        let t = tags(&[("name", "x"), ("name:ja-Hira", " "), ("name:ja_kana", "えっくす")]);
        assert_eq!(own(&t, "x"), (None, Some("えっくす".into()), None));
    }

    #[test]
    fn points_by_kind() {
        let t = tags(&[("name", "A"), ("place", "town"), ("population", "1,000;2"), ("capital", "4")]);
        let r = point(7, 10_000_000, 20_000_000, &t).unwrap();
        let pop = 1001f64.dlog10();
        assert_eq!(
            (r.kind, r.score, r.popz, r.lon, r.oid.as_str()),
            ("place", 60.0 + pop + 0.95, Some(18.0 - 2.0 * pop - 0.5), 1.0, "n7")
        );
        assert_eq!(population(Some("lots")), 0.0);
        assert_eq!(population(Some("-5")), 0.0);
        let t = tags(&[("name", "B"), ("natural", "bay"), ("wikipedia", "en:B")]);
        assert_eq!(point(1, 0, 0, &t).unwrap().score, 5.0);
        assert!(point(1, 1_800_000_001, 0, &t).is_none());
    }

    #[test]
    fn areas_from_the_export() {
        let f = r#"{"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[[[[0,0],[0.1,0],[0.1,0.1],[0,0.1],[0,0]]]]},"properties":{"@type":"way","@id":5,"natural":"water","name":"Lac","name:en":"Lake"}}"#;
        let r = area_row(&format!("\u{1e}{f}")).unwrap().unwrap();
        assert_eq!((r.kind, r.class.as_str(), r.oid.as_str(), r.en.as_deref()), ("water", "lake", "w5", Some("Lake")));
        assert_eq!([r.lon, r.lat], [0.05, 0.05]);
        assert!(area_row(&f.replace(r#""natural":"water""#, r#""natural":"water","water":"river""#))
            .unwrap()
            .is_none());
    }

    #[test]
    fn duplicates_near_a_better_one_left_out() {
        let row = |name: &str, lon: f64, score: f64| Row {
            kind: "water",
            class: "lake".into(),
            name: name.into(),
            lon,
            lat: 0.0,
            score,
            en: None,
            km2: None,
            popz: None,
            kana: None,
            langs: None,
            oid: "w1".into(),
        };
        let rows = [row("L", 0.0, 1.0), row("L", 0.1, 2.0), row("L", 1.0, 0.5), row("M", 0.0, 1.0)];
        assert_eq!(dedupe(&rows), [false, true, true, true]);
    }

    /// The layer's bytes for these features (-0.0 and 0.0 one value, as they always were).
    #[test]
    fn tile_bytes() {
        let f = |x, y, id, n, s| Feature {
            x,
            y,
            id,
            props: vec![("n", Val::Str(n)), ("mz", Val::F(-0.0)), ("s", Val::F(s))],
        };
        let b = encode(&[f(1, -2, 300, "a", 0.0), f(4096, 0, 0, "b", 1.5)]);
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        assert_eq!(
            hex,
            "1a5a78020a016c121208ac0212060000010102011801220309020312120800120600020101020318012204098040001a016e1a026d7a1a017322030a0161220919000000000000008022030a0162220919000000000000f83f288020"
        );
    }

    /// A run on real data: `SCENIC_LABELS_RUN="<labels set> <out archive> <work dir>"` makes the
    /// archive as the step does, all local. Unset, nothing.
    #[test]
    fn run_on_a_real_set() {
        let Ok(m) = std::env::var("SCENIC_LABELS_RUN") else { return };
        let a: Vec<&str> = m.split_whitespace().collect();
        run(Path::new(a[0]), Path::new(a[1]), Path::new(a[2])).unwrap();
    }
}
