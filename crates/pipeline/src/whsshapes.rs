//! World Heritage Sites as their lines and areas, from OpenStreetMap, and one dot per site, in the
//! heritage chain's stand-in root.
//!
//! Outlines: a canal, a wall, a site boundary drawn through the view, where UNESCO's data (the
//! unesco-whc register) has only one point per component (the Rideau Canal's dots sit at its locks
//! and forts; the canal between them is not marked). A site's OSM objects are those tagged with the
//! Wikidata item of the site or of one of its components (P757, the World Heritage id, "430" or
//! "430-001"), or of an item that is part of one (P361: Hadrian's Wall, part of the Frontiers of the
//! Roman Empire), or with its World Heritage reference (ref:whc) or heritage:operator=whc; route,
//! waterway and site relations bring their member ways (a canal's waterway relation). Only lines
//! and areas (the points are the dots already), a closed way as an area unless it is a line by its
//! tags (a wall, a canal). Not the boundaries of settlements, administrative areas, natural regions
//! or regional and landscape parks that share an item (Wikidata puts some components on their town:
//! Sahagún on the Camino de Santiago), nor, matched by item alone, an area over three times the
//! site's inscribed area (the whole island of Ibiza for its Dalt Vila and seagrass meadows): these
//! aren't the site's extent, unless tagged as World Heritage themselves (heritage=1, ref:whc).
//!
//! One dot per site: a site UNESCO lists in several components here (Hadrian's Wall in 193, the
//! Rideau Canal in 6) gets one dot at its centre, on the site: of the site's points (on its lines,
//! inside its areas clear of their edges, and its components with no outline near), the one with
//! the least total distance to all of the site (the cells it touches at 1/256 of its extent: lines,
//! areas and components alike). So a winding canal's dot is on the canal, and a site in scattered
//! pieces has it on the piece nearest the middle. The components are drawn small from zoom 10
//! (crate::heritage_layers). A site with one component keeps UNESCO's point.
//!
//! Input: data/build/heritage.json (the sites in our regions: a World Heritage register's entries
//! carry their site's id, `site`, its inscribed area, `site_area_km2`, and a component's UNESCO
//! reference, `ref`: docs/inputs.md §6.5), data/osm/merged.osm.pbf, Wikidata (cached in
//! data/heritage/whs/, asked of the declared `wikidata` source, QLever, when not: crate::wikiask).
//! Writes data/build/whs-shapes.json: lines and polygons with id (World Heritage id), n (the site's
//! name as the dots show it), c (Cultural, Natural or Mixed), a (1 for areas), simplified to ~15 m;
//! and data/build/whs-sites.json, per site id: q, its Wikidata items (its own and those of its
//! components here), and for a site with several components here, n (their number), dot
//! [lon, lat] and lead [lon, lat] (the component nearest the dot, whose record the dot shows).
//!
//! The bytes are those dem/whsshapes.py made (shapely 2.1.2 on GEOS 3.13.1, numpy 2.5.3): the
//! simplification is crate::tpsimplify's, the geometry GEOS's own (linked statically: the `geos`
//! crate), numpy's sampling and float32 sums crate::pynumpy's, every float operation in the
//! script's order.

use crate::pyjson::{dumps, loads, Py};
use crate::pynumpy;
use crate::pytext::{decimal, strip};
use crate::timings::{phase, Class};
use crate::wikiask::Asker;
use anyhow::{bail, Context, Result};
use geos::{CapStyle, CoordSeq, Geom, Geometry, GeometryTypes, JoinStyle};
use std::collections::{HashMap, HashSet};
use std::path::Path;

type P = [f64; 2];

/// Closed ways that are lines, not areas, by their tags (a city wall around the old town).
const LINEAR_KEYS: [&str; 5] = ["barrier", "highway", "railway", "power", "aerialway"];
const LINEAR_MAN_MADE: [&str; 6] = ["embankment", "dyke", "breakwater", "groyne", "cutting", "pipeline"];
const AREA_WATERWAYS: [&str; 3] = ["riverbank", "dock", "boatyard"];
/// Kinds of place that are settlements or administrative areas (not a square or an island).
const SETTLEMENT: [&str; 19] = [
    "city",
    "town",
    "village",
    "hamlet",
    "isolated_dwelling",
    "farm",
    "municipality",
    "borough",
    "suburb",
    "quarter",
    "neighbourhood",
    "city_block",
    "civil_parish",
    "county",
    "region",
    "state",
    "province",
    "district",
    "country",
];
/// An area matched by item alone, over this many times the site's inscribed area.
const TOO_LARGE: f64 = 3.0;
/// The footprint's resolution for a site's centre: cells across its extent.
const CELLS: f64 = 256.0;
/// Cells to measure distances to (a sample, for large areas).
const MASS_MAX: usize = 4000;
/// The outlines' simplification, degrees.
const TOL: f64 = 0.00015;

/// Python's `round(x, n)`.
fn py_round(x: f64, n: usize) -> f64 {
    crate::interest::py_round(x, n)
}

/// Tags, read from OSM's OPL (strings as OPL escapes them) or a GeoJSON export's properties.
trait Tags {
    fn get(&self, k: &str) -> Option<&str>;
    fn has(&self, k: &str) -> bool;
    fn is(&self, k: &str, v: &str) -> bool {
        self.get(k) == Some(v)
    }
    /// Python's truthiness of the tag.
    fn set(&self, k: &str) -> bool {
        self.get(k).is_some_and(|v| !v.is_empty())
    }
}

impl Tags for HashMap<String, String> {
    fn get(&self, k: &str) -> Option<&str> {
        HashMap::get(self, k).map(String::as_str)
    }
    fn has(&self, k: &str) -> bool {
        self.contains_key(k)
    }
}

impl Tags for Py {
    fn get(&self, k: &str) -> Option<&str> {
        Py::get(self, k).and_then(Py::as_str)
    }
    fn has(&self, k: &str) -> bool {
        Py::get(self, k).is_some()
    }
}

/// Tagged as World Heritage itself (not only by a shared item).
fn tagged_whs(t: &impl Tags) -> bool {
    t.is("heritage", "1") || t.set("ref:whc") || t.is("heritage:operator", "whc")
}

/// A settlement, administrative or natural-region boundary, or a regional or landscape park (IUCN
/// V–VI), not tagged as World Heritage itself: sharing a site's item, but not its extent.
fn not_extent(t: &impl Tags) -> bool {
    let among = |k: &str, vs: &[&str]| t.get(k).is_some_and(|v| vs.contains(&v));
    !tagged_whs(t)
        && (among("boundary", &["administrative", "place", "political", "natural", "census", "statistical"])
            || among("place", &SETTLEMENT)
            || among("protect_class", &["5", "6"]))
}

/// A closed way that is a line (a wall, a canal loop), not an area.
fn linear(t: &impl Tags) -> bool {
    if t.is("area", "yes") {
        return false;
    }
    let w = t.get("waterway").filter(|w| !w.is_empty());
    LINEAR_KEYS.iter().any(|k| t.has(k))
        || w.is_some_and(|w| !AREA_WATERWAYS.contains(&w))
        || t.get("historic").is_some_and(|h| h == "citywalls" || h == "aqueduct")
        || t.get("man_made").is_some_and(|m| LINEAR_MAN_MADE.contains(&m))
}

/// Python's `re.match(r"\d+", s)`: the leading digits, if any.
fn leading_digits(s: &str) -> Option<&str> {
    let end = s.char_indices().find(|(_, c)| decimal(*c).is_none()).map_or(s.len(), |(i, _)| i);
    (end > 0).then(|| &s[..end])
}

/// A UNESCO reference without its revision suffixes ("430ter-038" → "430-038").
fn plain(r: &str) -> String {
    const SUFFIXES: [&str; 5] = ["bis", "ter", "quater", "quinquies", "rev"];
    let s = strip(r);
    let mut out = String::new();
    let mut prev_digit = false;
    let mut i = 0;
    'scan: while i < s.len() {
        if prev_digit {
            for x in SUFFIXES {
                if s[i..].starts_with(x) {
                    // (The look-behind reads the text as it was: the suffix's last letter next.)
                    i += x.len();
                    prev_digit = false;
                    continue 'scan;
                }
            }
        }
        let c = s[i..].chars().next().unwrap();
        out.push(c);
        prev_digit = decimal(c).is_some();
        i += c.len_utf8();
    }
    out
}

/// The last part of a URL (`rsplit("/", 1)[-1]`).
fn last(u: &str) -> &str {
    u.rsplit('/').next().unwrap_or(u)
}

/// Our World Heritage Sites (heritage.json), in their order.
struct Sites {
    ids: Vec<String>,
    /// The name the dots show, category, components' places (rounded: their JSON numbers and their
    /// values), components' UNESCO references, inscribed area (km², where it's known).
    names: HashMap<String, Py>,
    cats: HashMap<String, Py>,
    comps: HashMap<String, Vec<([Py; 2], P)>>,
    refs: HashMap<String, Vec<String>>,
    km2: HashMap<String, f64>,
}

fn our_sites(b: &Path) -> Result<Sites> {
    let fc = loads(&std::fs::read_to_string(b.join("heritage.json"))?).context("parse heritage.json")?;
    let mut s = Sites {
        ids: vec![],
        names: HashMap::new(),
        cats: HashMap::new(),
        comps: HashMap::new(),
        refs: HashMap::new(),
        km2: HashMap::new(),
    };
    for f in fc.at("features").list().context("heritage.json: no features")? {
        let p = f.at("properties");
        let Some(sid) = crate::heritage_layers::whs_id(p) else { continue };
        if !s.names.contains_key(&sid) {
            s.ids.push(sid.clone());
            let name = [p.at("name"), p.at("name_en")].into_iter().find(|v| v.truthy()).cloned();
            s.names.insert(sid.clone(), name.unwrap_or(Py::Str(String::new())));
            let cat = Some(p.at("category")).filter(|v| v.truthy()).cloned();
            s.cats.insert(sid.clone(), cat.unwrap_or(Py::Str("Cultural".into())));
        }
        let c = f.at("geometry").at("coordinates").list().context("a site without coordinates")?;
        let round = |v: &Py| -> Result<(Py, f64)> {
            Ok(match v {
                Py::Float(x) => (Py::Float(py_round(*x, 6)), py_round(*x, 6)),
                v => (v.clone(), v.number().context("a coordinate not a number")?),
            })
        };
        let (x, y) = (round(&c[0])?, round(c.get(1).context("a site without y")?)?);
        s.comps.entry(sid.clone()).or_default().push(([x.0, y.0], [x.1, y.1]));
        if p.at("ref").truthy() {
            let r = p.at("ref").as_str().context("a component's ref not a string")?;
            s.refs.entry(sid.clone()).or_default().push(r.to_string());
        }
        if p.at("site_area_km2").truthy() {
            s.km2.insert(sid.clone(), p.at("site_area_km2").number().context("site_area_km2 not a number")?);
        }
    }
    Ok(s)
}

/// A Wikidata query's answer (its bindings' values), from its cache (asked once, when it isn't
/// there, of the declared `wikidata` source: QLever's endpoint).
fn wd_sparql(ask: &Asker, wikidata: &str, query: &str, cache: &Path) -> Result<Vec<HashMap<String, String>>> {
    if std::fs::metadata(cache).map_or(true, |m| m.len() == 0) {
        let _p = phase("downloading sources", Class::Net);
        let (body, _) = ask.sparql_answer(wikidata, query)?;
        std::fs::create_dir_all(cache.parent().unwrap())?;
        let tmp = cache.with_extension("json.part");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, cache)?;
    }
    let d = loads(&std::fs::read_to_string(cache)?).with_context(|| format!("parse {}", cache.display()))?;
    let mut out = vec![];
    for b in d.at("results").at("bindings").list().context("no bindings")? {
        let Py::Dict(kv) = b else { bail!("a binding not an object") };
        let mut m = HashMap::new();
        for (k, v) in kv {
            m.insert(k.clone(), v.at("value").as_str().context("a value not a string")?.to_string());
        }
        out.push(m);
    }
    Ok(out)
}

const P757: &str = "SELECT ?item ?id WHERE { ?item wdt:P757 ?id }";

/// Per site, its Wikidata items: its own (P757 "1221", "320bis") and its components' here (P757
/// "1221-001", by the components' UNESCO references).
fn site_items(sites: &Sites, p757: &[HashMap<String, String>]) -> HashMap<String, Vec<String>> {
    let ours: HashMap<&str, HashSet<String>> =
        sites.refs.iter().map(|(sid, rs)| (sid.as_str(), rs.iter().map(|r| plain(r)).collect())).collect();
    let mut items: HashMap<String, Vec<String>> = HashMap::new();
    for r in p757 {
        let v = plain(&r["id"]);
        let sid = v.split('-').next().unwrap();
        let q = last(&r["item"]);
        if sites.names.contains_key(sid) && (!v.contains('-') || ours.get(sid).is_some_and(|o| o.contains(&v))) {
            let l = items.entry(sid.to_string()).or_default();
            if !l.iter().any(|x| x == q) {
                l.push(q.to_string());
            }
        }
    }
    // (By number: the same list whatever order the service answered in.)
    for l in items.values_mut() {
        l.sort_by(|a, b| by_number(a, b));
    }
    items
}

/// Ids ordered by their number ("Q9" before "Q10", "85" before "120"), then as text.
fn by_number(a: &str, b: &str) -> std::cmp::Ordering {
    let digits = |s: &str| s.trim_start_matches(|c: char| !c.is_ascii_digit()).len();
    digits(a).cmp(&digits(b)).then_with(|| a.cmp(b))
}

/// A query's rows sorted by the values of `keys` in turn: the same order whatever order the
/// service answered in.
fn sorted(mut rows: Vec<HashMap<String, String>>, keys: [&str; 2]) -> Vec<HashMap<String, String>> {
    let at = |r: &HashMap<String, String>, k: &str| r.get(k).cloned().unwrap_or_default();
    rows.sort_by(|a, b| at(a, keys[0]).cmp(&at(b, keys[0])).then_with(|| at(a, keys[1]).cmp(&at(b, keys[1]))));
    rows
}

/// A simplified outline: GeoJSON's type and coordinates.
struct Outline {
    kind: &'static str,
    coords: Py,
}

impl Outline {
    fn area(&self) -> bool {
        matches!(self.kind, "Polygon" | "MultiPolygon")
    }
}

fn pts_of(l: &Py) -> Result<Vec<P>> {
    l.list()
        .context("a line")?
        .iter()
        .map(|p| match p.list().map(|a| a.as_slice()) {
            Some([Py::Float(x), Py::Float(y)]) => Ok([*x, *y]),
            _ => bail!("not a 2D position"),
        })
        .collect()
}

fn seq(pts: &[P]) -> Result<CoordSeq> {
    Ok(CoordSeq::new_from_vec(pts)?)
}

fn polygon(rings: &[Vec<P>]) -> Result<Geometry> {
    let shell = Geometry::create_linear_ring(seq(&rings[0])?)?;
    let holes = rings[1..].iter().map(|r| Ok(Geometry::create_linear_ring(seq(r)?)?)).collect::<Result<_>>()?;
    Ok(Geometry::create_polygon(shell, holes)?)
}

/// An outline as a GEOS geometry, its coordinates mapped by `f`.
fn geometry(o: &Outline, f: impl Fn(P) -> P) -> Result<Geometry> {
    let line = |l: &Py| -> Result<Vec<P>> { Ok(pts_of(l)?.into_iter().map(&f).collect()) };
    let poly = |p: &Py| -> Result<Geometry> {
        polygon(&p.list().context("a polygon")?.iter().map(line).collect::<Result<Vec<_>>>()?)
    };
    let parts = || o.coords.list().context("multi coordinates");
    Ok(match o.kind {
        "LineString" => Geometry::create_line_string(seq(&line(&o.coords)?)?)?,
        "MultiLineString" => Geometry::create_multiline_string(
            parts()?.iter().map(|l| Ok(Geometry::create_line_string(seq(&line(l)?)?)?)).collect::<Result<_>>()?,
        )?,
        "Polygon" => poly(&o.coords)?,
        _ => Geometry::create_multipolygon(parts()?.iter().map(poly).collect::<Result<_>>()?)?,
    })
}

/// Each site's OSM lines and areas (simplified), by site id, in the order sites were first found.
fn outlines(
    sites: &Sites,
    site_of: &HashMap<String, String>,
    w: &Path,
    osm: &Path,
) -> Result<(Vec<String>, HashMap<String, Vec<Outline>>)> {
    // Their OSM objects: everything tagged wikidata or ref:whc (tags only), picked by value, then
    // those objects with their geometry (a filter on thousands of values at once is slow).
    let tagged = w.join("tagged.osm.pbf");
    let p = phase("filtering the tagged objects (osmium)", Class::Compute);
    let mut c = crate::osmpass::osmium();
    c.arg("tags-filter").arg(osm).args(["nwr/wikidata", "nwr/ref:whc", "nwr/heritage:operator=whc", "-R", "-o"]);
    c.arg(&tagged).arg("--overwrite");
    osmium_run(c, "osmium tags-filter", &[0])?;
    drop(p);
    let p = phase("picking the sites' objects", Class::Compute);
    let opl = w.join("tagged.opl");
    let mut c = crate::osmpass::osmium();
    c.arg("cat").arg(&tagged).args(["-f", "opl", "--overwrite", "-o"]).arg(&opl);
    osmium_run(c, "osmium cat", &[0])?;
    let text = std::fs::read_to_string(&opl)?;
    std::fs::remove_file(&opl)?;
    let mut want: Vec<&str> = vec![];
    for line in text.lines() {
        if site_of.contains_key(wikidata_in_opl(line).unwrap_or(""))
            || line.contains("ref:whc=")
            || line.contains("heritage:operator=whc")
        {
            want.push(line.split(' ').next().unwrap());
        }
    }
    let ids = w.join("ids.txt");
    std::fs::write(&ids, want.join("\n") + "\n")?;
    eprintln!("{} OSM objects", want.len());
    drop(text);
    drop(p);
    let pbf = w.join("whs.osm.pbf");
    // (exit 1: some objects weren't found, e.g. a relation member outside our extracts)
    let p = phase("extracting the sites' objects (osmium)", Class::Compute);
    let mut c = crate::osmpass::osmium();
    c.args(["getid", "-r"]).arg(osm).arg("-i").arg(&ids).arg("-o").arg(&pbf).arg("--overwrite");
    osmium_run(c, "osmium getid", &[0, 1])?;
    drop(p);
    let p = phase("exporting the sites' objects (osmium)", Class::Compute);
    let mut c = crate::osmpass::osmium();
    c.arg("export").arg(&pbf).args(["-f", "geojsonseq", "-a", "type,id", "--geometry-types=linestring,polygon", "-o"]);
    c.arg(w.join("whs.geojsonseq")).arg("--overwrite");
    osmium_run(c, "osmium export", &[0])?;
    drop(p);
    let p = phase("listing the relations (osmium)", Class::Compute);
    let mut c = crate::osmpass::osmium();
    c.arg("cat").arg(&pbf).args(["-t", "relation", "-f", "opl", "-o"]).arg(w.join("relations.opl")).arg("--overwrite");
    osmium_run(c, "osmium cat (relations)", &[0])?;
    drop(p);

    let site = |t: &dyn Fn(&str) -> Option<String>| -> Option<String> {
        let r = t("ref:whc").filter(|v| !v.is_empty()).or_else(|| t("whc:ref").filter(|v| !v.is_empty()));
        if let Some(d) = r.as_deref().and_then(leading_digits) {
            if sites.names.contains_key(d) {
                return Some(d.to_string());
            }
        }
        site_of.get(&t("wikidata").unwrap_or_default()).cloned()
    };

    // Route, waterway and site relations of a site pass it on to their member ways (not areas'
    // relations: their ways are the area's edges).
    let p = phase("reading the relations' members", Class::Compute);
    let mut member_site: HashMap<String, String> = HashMap::new();
    for line in std::fs::read_to_string(w.join("relations.opl"))?.lines() {
        let (Some(tm), Some(mm)) = (field(line, " T"), field(line, " M")) else { continue };
        let mut tags: HashMap<String, String> = HashMap::new();
        for kv in tm.split(',') {
            if let Some((k, v)) = kv.split_once('=') {
                tags.insert(k.to_string(), v.to_string());
            }
        }
        let Some(s) = site(&|k| Tags::get(&tags, k).map(str::to_string)) else { continue };
        if tags.is("type", "multipolygon") || tags.is("type", "boundary") || not_extent(&tags) {
            continue;
        }
        for m in mm.split(',') {
            let k = m.split('@').next().unwrap();
            if k.starts_with('w') {
                member_site.entry(k.to_string()).or_insert_with(|| s.clone());
            }
        }
    }
    drop(p);

    // A closed way comes out twice, as a line and as an area: one of them, by its tags.
    let p = phase("picking and simplifying the outlines", Class::Compute);
    let mut keys: Vec<String> = vec![];
    let mut found: HashMap<String, [Option<Py>; 2]> = HashMap::new();
    let text = std::fs::read_to_string(w.join("whs.geojsonseq"))?;
    for line in text.lines() {
        let f = loads(line.trim_start_matches('\x1e')).context("a line of whs.geojsonseq")?;
        let p = f.at("properties");
        let ty = p.at("@type").as_str().context("no @type")?;
        let key = format!("{}{}", ty.chars().next().context("an empty @type")?, p.at("@id").py_str());
        let poly = matches!(f.at("geometry").at("type").as_str(), Some("Polygon" | "MultiPolygon"));
        let by = found.entry(key.clone()).or_insert_with(|| {
            keys.push(key);
            [None, None]
        });
        by[poly as usize] = Some(f);
    }
    drop(text);
    let mut order: Vec<String> = vec![];
    let mut out: HashMap<String, Vec<Outline>> = HashMap::new();
    let mut skipped: Vec<(String, usize)> = vec![];
    for key in keys {
        let by = &found[&key];
        let f = match by {
            [Some(l), None] => l,
            [Some(l), Some(a)] => {
                if linear(l.at("properties")) {
                    l
                } else {
                    a
                }
            }
            [None, Some(a)] => a,
            [None, None] => unreachable!(),
        };
        let p = f.at("properties");
        let s = site(&|k| Tags::get(p, k).map(str::to_string)).or_else(|| member_site.get(&key).cloned());
        let Some(s) = s else { continue };
        let g = f.at("geometry");
        let Some((kind, coords)) = crate::tpsimplify::simplify_geojson(g.at("type"), g.at("coordinates"), TOL)? else {
            continue;
        };
        let o = Outline { kind, coords };
        let too_large = || -> Result<bool> {
            let Some(km2) = sites.km2.get(&s) else { return Ok(false) };
            let gg = geometry(&o, |p| p)?;
            let y = gg.get_centroid()?.get_y()?;
            Ok(gg.area()? * (111.32 * 111.32) * y.to_radians().cos() > TOO_LARGE * km2)
        };
        if not_extent(p) || (o.area() && !tagged_whs(p) && too_large()?) {
            let n = sites.names[&s].py_str();
            match skipped.iter_mut().find(|(k, _)| *k == n) {
                Some(e) => e.1 += 1,
                None => skipped.push((n, 1)),
            }
            continue;
        }
        if !out.contains_key(&s) {
            order.push(s.clone());
        }
        out.entry(s).or_default().push(o);
    }
    drop(p);
    if !skipped.is_empty() {
        // (Python's sort: stable, by count down.)
        skipped.sort_by(|a, b| b.1.cmp(&a.1));
        let l: Vec<String> = skipped.iter().take(12).map(|(k, n)| format!("{k} {n}")).collect();
        eprintln!(
            "not the sites' extent (settlements, administrative areas, regions, regional parks, too large): {}",
            l.join(", ")
        );
    }
    Ok((order, out))
}

/// Python's `re.search(r"(?:^| T)(?:.*,)?wikidata=(Q\d+)", line)`: the last `,wikidata=Q…` of the
/// line, else a tag list starting with it.
fn wikidata_in_opl(line: &str) -> Option<&str> {
    let qid = |rest: &str| -> Option<usize> {
        let r = rest.strip_prefix('Q')?;
        let n = leading_digits(r)?.len();
        Some(1 + n)
    };
    for (i, _) in line.rmatch_indices(",wikidata=") {
        let at = i + ",wikidata=".len();
        if let Some(n) = qid(&line[at..]) {
            return Some(&line[at..at + n]);
        }
    }
    for (i, _) in line.match_indices(" Twikidata=") {
        let at = i + " Twikidata=".len();
        if let Some(n) = qid(&line[at..]) {
            return Some(&line[at..at + n]);
        }
    }
    None
}

/// Python's `re.search(r" T(\S*)", line).group(1)` (and " M").
fn field<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let at = line.find(tag)? + tag.len();
    let rest = &line[at..];
    let end = rest.find(|c: char| c.is_whitespace() || crate::pytext::space(c)).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn osmium_run(mut c: std::process::Command, what: &str, ok: &[i32]) -> Result<()> {
    let st = store::spawn::status(&mut c).with_context(|| format!("run {what}"))?;
    match st.code() {
        Some(k) if ok.contains(&k) => Ok(()),
        _ => bail!("{what} failed ({st})"),
    }
}

/// A geometry's parts, two levels down (shapely's `get_parts(get_parts(g))`).
fn parts2(g: &Geometry) -> Result<Vec<Geometry>> {
    let multi = |g: &Geometry| -> Result<bool> {
        Ok(matches!(
            g.geometry_type()?,
            GeometryTypes::MultiPoint
                | GeometryTypes::MultiLineString
                | GeometryTypes::MultiPolygon
                | GeometryTypes::GeometryCollection
        ))
    };
    let mut one = vec![];
    if multi(g)? {
        for i in 0..g.get_num_geometries()? {
            one.push(Geom::clone(&g.get_geometry_n(i)?)?);
        }
    } else {
        one.push(Geom::clone(g)?);
    }
    let mut out = vec![];
    for p in one {
        if multi(&p)? {
            for i in 0..p.get_num_geometries()? {
                out.push(Geom::clone(&p.get_geometry_n(i)?)?);
            }
        } else {
            out.push(p);
        }
    }
    Ok(out)
}

fn coords(g: &impl Geom) -> Result<Vec<P>> {
    let s = g.get_coord_seq()?;
    (0..s.size()?).map(|i| Ok([s.get_x(i)?, s.get_y(i)?])).collect()
}

fn xy(g: &impl Geom) -> Result<P> {
    Ok([g.get_x()?, g.get_y()?])
}

/// Points spaced at most half a cell along a line (a ring: not its end again).
fn along(line: &Geometry, h: f64, closed: bool) -> Result<Vec<P>> {
    let len = line.length()?;
    let n = 2usize.max((len / (h / 2.0)).ceil() as usize + 1);
    pynumpy::linspace(len, n, !closed).into_iter().map(|d| xy(&line.interpolate(d)?)).collect()
}

/// A site's dot: of its points (on its lines, inside its areas clear of the edges, its components
/// with no outline near), the one with the least total distance to the cells it touches at
/// 1/CELLS of its extent (see above).
fn centre(geoms: &[Outline], comps: &[P]) -> Result<P> {
    let lon0 = pynumpy::mean(&comps.iter().map(|c| c[0]).collect::<Vec<_>>());
    let lat0 = pynumpy::mean(&comps.iter().map(|c| c[1]).collect::<Vec<_>>());
    let k = [111.32 * lat0.to_radians().cos(), 110.57];
    let o = [lon0, lat0];
    let km = |p: P| [(p[0] - o[0]) * k[0], (p[1] - o[1]) * k[1]];
    let mut lines: Vec<Geometry> = vec![];
    let mut polys: Vec<Geometry> = vec![];
    for g in geoms {
        let mut gg = geometry(g, km)?;
        if g.area() {
            gg = gg.make_valid()?;
        }
        for part in parts2(&gg)? {
            if part.is_empty()? {
                continue;
            }
            match part.geometry_type()? {
                GeometryTypes::Polygon if part.area()? > 0.0 => polys.push(part),
                GeometryTypes::LineString | GeometryTypes::LinearRing if part.length()? > 0.0 => {
                    lines.push(Geometry::create_line_string(seq(&coords(&part)?)?)?)
                }
                _ => {}
            }
        }
    }
    let pts: Vec<P> = comps.iter().map(|&c| km(c)).collect();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for g in lines.iter().chain(&polys) {
        x0 = x0.min(g.get_x_min()?);
        y0 = y0.min(g.get_y_min()?);
        x1 = x1.max(g.get_x_max()?);
        y1 = y1.max(g.get_y_max()?);
    }
    for p in &pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let ext = (x1 - x0).max(y1 - y0);
    let h = (ext / CELLS).max(0.01);

    let mut mass: Vec<P> = pts.clone();
    let mut cand: Vec<P> = vec![];
    for ln in &lines {
        let s = along(ln, h, false)?;
        mass.extend(&s);
        cand.extend(&s);
    }
    for pg in &polys {
        let (bx0, by0, bx1, by1) = (pg.get_x_min()?, pg.get_y_min()?, pg.get_x_max()?, pg.get_y_max()?);
        let gx = pynumpy::arange(bx0 + h / 2.0, bx1, h);
        let gy = pynumpy::arange(by0 + h / 2.0, by1, h);
        let prep = pg.to_prepared_geom()?;
        let mut inner: Vec<P> = vec![];
        for &y in &gy {
            for &x in &gx {
                if prep.contains_xy(x, y)? {
                    inner.push([x, y]);
                }
            }
        }
        mass.extend(&inner);
        mass.extend(along(&Geom::clone(&pg.get_exterior_ring()?)?, h, true)?);
        // Inside, clear of the edges: half its inradius, at most a cell; the pole of
        // inaccessibility always.
        let pole = match pg.maximum_inscribed_circle((h / 20.0).max(1e-4)) {
            Ok(l) => xy(&l.get_point_n(0)?)?,
            // (A degenerate ring.)
            Err(_) => xy(&pg.point_on_surface()?)?,
        };
        let pole_g = Geometry::create_point(seq(&[pole])?)?;
        let d = 0.5 * pg.boundary()?.distance(&pole_g)?;
        let margin = if d < h { d } else { h };
        if !inner.is_empty() {
            let core = pg.buffer_with_style(-margin, 16, CapStyle::Round, JoinStyle::Round, 5.0)?;
            if !core.is_empty()? {
                let prep = core.to_prepared_geom()?;
                for p in &inner {
                    if prep.contains_xy(p[0], p[1])? {
                        cand.push(*p);
                    }
                }
            }
        }
        cand.push(pole);
    }
    if !lines.is_empty() || !polys.is_empty() {
        // Components with no outline near: points of the site as they are.
        let all: Vec<&Geometry> = lines.iter().chain(&polys).collect();
        for p in &pts {
            let pg = Geometry::create_point(seq(&[*p])?)?;
            let mut near = f64::INFINITY;
            for g in &all {
                near = near.min(g.distance(&pg)?);
            }
            if near > 2.0 * h {
                cand.push(*p);
            }
        }
    } else {
        cand.extend(&pts);
    }
    // (np.floor(v / h) as a key: -0 as 0, as numpy's comparisons have it.)
    let cell = |p: &P| [(p[0] / h).floor() + 0.0, (p[1] / h).floor() + 0.0];
    let mut cells: Vec<P> = mass.iter().map(cell).collect();
    cells.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    cells.dedup();
    let mut m: Vec<[f32; 2]> = cells.iter().map(|c| [((c[0] + 0.5) * h) as f32, ((c[1] + 0.5) * h) as f32]).collect();
    if m.len() > MASS_MAX {
        let pick = pynumpy::Rng::new(0).choice(m.len(), MASS_MAX);
        m = pick.into_iter().map(|i| m[i]).collect();
    }
    let mut seen: HashSet<[u64; 2]> = HashSet::new();
    let c: Vec<[f32; 2]> = cand
        .iter()
        .filter(|p| {
            let k = cell(p);
            seen.insert([k[0].to_bits(), k[1].to_bits()])
        })
        .map(|p| [p[0] as f32, p[1] as f32])
        .collect();
    let mut row = vec![0f32; m.len()];
    let mut best = (f32::INFINITY, 0);
    for (i, p) in c.iter().enumerate() {
        for (r, q) in row.iter_mut().zip(&m) {
            let (dx, dy) = (p[0] - q[0], p[1] - q[1]);
            *r = (dx * dx + dy * dy).sqrt();
        }
        let t = pynumpy::sum_f32(&row);
        // (np.argmin: the first of the least.)
        if t < best.0 {
            best = (t, i);
        }
    }
    let b = c[best.1];
    Ok([py_round(b[0] as f64 / k[0] + o[0], 6), py_round(b[1] as f64 / k[1] + o[1], 6)])
}

/// Writes whs-shapes.json and whs-sites.json in `root`'s data/build, asking Wikidata at `wikidata`
/// (the declared `wikidata` source) for what its cache lacks.
pub fn run(root: &Path, wikidata: &str) -> Result<()> {
    let b = root.join("data/build");
    let w = root.join("data/heritage/whs");
    let osm = root.join("data/osm/merged.osm.pbf");
    std::fs::create_dir_all(&w)?;
    let p = phase("reading the sites", Class::Disk);
    let sites = our_sites(&b)?;
    drop(p);
    eprintln!("{} World Heritage Sites in our regions", sites.ids.len());
    let ask = Asker::new();

    // Its Wikidata items: the site and its components (P757), and items part of one (P361).
    let p = phase("reading the Wikidata items", Class::Disk);
    // (An item of several of our sites, or part of several, is the lowest-numbered site's; an
    // item's own P757 before what it is part of.)
    let p757 = sorted(wd_sparql(&ask, wikidata, P757, &w.join("wd-p757.json"))?, ["item", "id"]);
    let mut site_of: HashMap<String, String> = HashMap::new();
    let lower = |site_of: &mut HashMap<String, String>, item: &str, s: &str| match site_of.get_mut(item) {
        Some(cur) if by_number(s, cur).is_lt() => *cur = s.to_string(),
        Some(_) => {}
        None => {
            site_of.insert(item.to_string(), s.to_string());
        }
    };
    for r in &p757 {
        if let Some(base) = leading_digits(&r["id"]).filter(|b| sites.names.contains_key(*b)) {
            lower(&mut site_of, last(&r["item"]), base);
        }
    }
    let own: HashSet<String> = site_of.keys().cloned().collect();
    let parts = sorted(
        wd_sparql(
            &ask,
            wikidata,
            "SELECT ?part ?whole WHERE { ?whole wdt:P757 ?id . ?part wdt:P361 ?whole }",
            &w.join("wd-parts.json"),
        )?,
        ["whole", "part"],
    );
    for r in &parts {
        let (whole, part) = (last(&r["whole"]), last(&r["part"]));
        if let Some(s) = site_of.get(whole).cloned() {
            if !own.contains(part) {
                lower(&mut site_of, part, &s);
            }
        }
    }
    drop(p);
    eprintln!("{} Wikidata items", site_of.len());
    let (order, shapes) = outlines(&sites, &site_of, &w, &osm)?;

    let p = phase("making the outlines' features", Class::Compute);
    let mut feats = vec![];
    let mut per: Vec<(String, usize)> = vec![];
    for s in &order {
        for g in &shapes[s] {
            let mut props = vec![
                ("id".to_string(), Py::Int(s.parse::<u64>().with_context(|| format!("a site id {s:?}"))?.to_string())),
                ("n".into(), sites.names[s].clone()),
                ("c".into(), sites.cats[s].clone()),
            ];
            if g.area() {
                props.push(("a".into(), Py::Int("1".into())));
            }
            feats.push(Py::Dict(vec![
                ("type".into(), Py::Str("Feature".into())),
                (
                    "geometry".into(),
                    Py::Dict(vec![("type".into(), Py::Str(g.kind.into())), ("coordinates".into(), round5(&g.coords))]),
                ),
                ("properties".into(), Py::Dict(props)),
            ]));
        }
        per.push((s.clone(), shapes[s].len()));
    }
    drop(p);
    let p = phase("writing the outlines", Class::Disk);
    let fc = Py::Dict(vec![("type".into(), Py::Str("FeatureCollection".into())), ("features".into(), Py::List(feats))]);
    std::fs::write(b.join("whs-shapes.json"), dumps(&fc))?;
    drop(p);
    eprintln!(
        "{} lines and areas for {} of {} sites",
        per.iter().map(|x| x.1).sum::<usize>(),
        per.len(),
        sites.ids.len()
    );

    let p = phase("finding the sites' items", Class::Compute);
    let items = site_items(&sites, &p757);
    drop(p);
    let p = phase("placing the sites' dots", Class::Compute);
    let mut out = vec![];
    for s in &sites.ids {
        let q = items.get(s).map_or(vec![], |l| l.iter().map(|q| Py::Str(q.clone())).collect());
        let mut rec = vec![("q".to_string(), Py::List(q))];
        let all = &sites.comps[s];
        // (sorted(set(…)): the first of equal places, in order.)
        let mut cs: Vec<&([Py; 2], P)> = vec![];
        for c in all {
            if !cs.iter().any(|d| d.1 == c.1) {
                cs.push(c);
            }
        }
        cs.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]).then(a.1[1].total_cmp(&b.1[1])));
        if cs.len() > 1 {
            let places: Vec<P> = cs.iter().map(|c| c.1).collect();
            let dot = centre(shapes.get(s).map_or(&[][..], |v| &v[..]), &places)?;
            let kx = dot[1].to_radians().cos();
            let dist = |c: &P| {
                let (a, b) = ((c[0] - dot[0]) * kx, c[1] - dot[1]);
                a * a + b * b
            };
            let mut lead = cs[0];
            for c in &cs[1..] {
                if dist(&c.1) < dist(&lead.1) {
                    lead = c;
                }
            }
            rec.push(("n".into(), Py::Int(all.len().to_string())));
            rec.push(("dot".into(), Py::List(vec![Py::Float(dot[0]), Py::Float(dot[1])])));
            rec.push(("lead".into(), Py::List(lead.0.to_vec())));
            eprintln!(
                "  one dot: {:52} {:4} components, {} outlines; dot {:.4}, {:.4}",
                sites.names[s].py_str().chars().take(52).collect::<String>(),
                all.len(),
                shapes.get(s).map_or(0, Vec::len),
                dot[1],
                dot[0]
            );
        }
        out.push((s.clone(), Py::Dict(rec)));
    }
    drop(p);
    let p = phase("writing the sites", Class::Disk);
    let n = out.iter().filter(|(_, r)| r.get("dot").is_some()).count();
    std::fs::write(b.join("whs-sites.json"), dumps(&Py::Dict(out)))?;
    drop(p);
    eprintln!("{n} sites in several components get one dot");
    Ok(())
}

/// Coordinates to 5 decimals (the Python's `parse_float`).
fn round5(c: &Py) -> Py {
    match c {
        Py::Float(x) => Py::Float(py_round(*x, 5)),
        Py::List(l) => Py::List(l.iter().map(round5).collect()),
        v => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_as_python() {
        assert_eq!(plain(" 430ter-038 "), "430-038");
        assert_eq!(plain("320bis"), "320");
        assert_eq!(plain("12bisbis"), "12bis");
        assert_eq!(plain("ter12"), "ter12");
        assert_eq!(leading_digits("1221-001"), Some("1221"));
        assert_eq!(leading_digits("x1"), None);
        let mut q = vec!["Q10", "Q9", "Q100", "Q11"];
        q.sort_by(|a, b| by_number(a, b));
        assert_eq!(q, ["Q9", "Q10", "Q11", "Q100"]);
        assert!(by_number("85", "120").is_lt());
    }

    #[test]
    fn opl_as_python() {
        let l = "r1 v1 dV c1 t2020-01-01T00:00:00Z i1 utest Tname=A,wikidata=Q42,x=y Mw5@outer,w6@";
        assert_eq!(wikidata_in_opl(l), Some("Q42"));
        assert_eq!(wikidata_in_opl("w1 Twikidata=Q7,a=b"), Some("Q7"));
        assert_eq!(wikidata_in_opl("w1 Tsubject:wikidata=Q7"), None);
        assert_eq!(wikidata_in_opl("w1 Ta=b,wikidata=Q1,c=d,wikidata=x"), Some("Q1"));
        assert_eq!(field(l, " T"), Some("name=A,wikidata=Q42,x=y"));
        assert_eq!(field(l, " M"), Some("w5@outer,w6@"));
    }

    #[test]
    fn tags_as_python() {
        let t: HashMap<String, String> = [("barrier", "wall"), ("boundary", "administrative")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        assert!(linear(&t) && not_extent(&t));
        let p = loads(r#"{"area":"yes","barrier":"wall","ref:whc":"430"}"#).unwrap();
        assert!(!linear(&p) && tagged_whs(&p) && !not_extent(&p));
    }

    #[test]
    fn geos_as_shapely() {
        // (shapely 2.1.2 on GEOS 3.13.1: polylabel, buffer, distance, centroid, interpolate.)
        let ring = [[0.0, 0.0], [4.0, 0.3], [5.0, 3.0], [2.2, 4.1], [0.5, 2.9], [1.4, 1.7], [0.0, 0.0]];
        let g = polygon(&[ring.to_vec()]).unwrap();
        let pole = xy(&g.maximum_inscribed_circle(0.013).unwrap().get_point_n(0).unwrap()).unwrap();
        assert_eq!(pole, [2.9541015625, 2.0841796875]);
        let sq = polygon(&[vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]]]).unwrap();
        assert_eq!(xy(&sq.maximum_inscribed_circle(1e-4).unwrap().get_point_n(0).unwrap()).unwrap(), [0.5, 0.5]);
        let core = g.buffer_with_style(-0.37, 16, CapStyle::Round, JoinStyle::Round, 5.0).unwrap();
        assert_eq!(core.area().unwrap(), 7.624105524043545);
        let p = Geometry::create_point(seq(&[pole]).unwrap()).unwrap();
        assert_eq!(g.boundary().unwrap().distance(&p).unwrap(), 1.6004611824460866);
        assert_eq!(g.get_centroid().unwrap().get_y().unwrap(), 1.9446708463949844);
        let ext = Geom::clone(&g.get_exterior_ring().unwrap()).unwrap();
        assert_eq!(xy(&ext.interpolate(7.77).unwrap()).unwrap()[0], 4.181376356587751);
        let bow = polygon(&[vec![[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0], [0.0, 0.0]]]).unwrap();
        let v = bow.make_valid().unwrap();
        assert_eq!(v.to_wkt_precision(0).unwrap(), "MULTIPOLYGON (((0 2, 1 1, 0 0, 0 2)), ((2 0, 1 1, 2 2, 2 0)))");
    }
}
