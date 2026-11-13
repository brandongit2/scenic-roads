//! Details for the highlighted areas, in the heritage chain's stand-in root: land area for every
//! polygon, and for parks, protected areas and Indigenous lands their OSM tags and Wikidata facts.
//!
//!   heritage-areas.json, special.json, indigenous.json (data/build): each feature gains its index
//!       as `i` (by which the overlays job joins them), and details-{harea,special,indigenous}.jsonl
//!       get its area (km², spherical); Indigenous lands also the tags of the OSM boundary with
//!       the same name (data/areas/areas.geojsonseq).
//!   parks (drawn from the basemap's park layer, which has no ids): details-park.jsonl, one record
//!       per named protected area in data/areas/areas.geojsonseq: name, bounding box, area,
//!       protection title / class, operator, owner, access, start date, website, description, and
//!       Wikidata (inception, short description, English article, area, visitors) where tagged. The
//!       server finds the one a hovered park is by name and position.
//!
//! The park facts are the pass's answers (data/areas/wikidata.json); those it lacks are asked of
//! Wikidata (crate::wikiask) and added to it after each batch. The files' bytes are as Python's
//! `json` writes them (pyport::pyjson), every float operation in the order the Python's was.

use crate::heritagewd::write_atomic;
use crate::interest::py_round;
use crate::wikiask::{by_item, earliest, fact_union, labels, largest, least, val, Asker};
use anyhow::{bail, Context, Result};
use det::Det;
use jobrt::timings::phase;
use pyport::pyjson::{dumps, dumps_spaced, loads, Py};
use std::collections::HashMap;
use std::path::Path;

const PARK_TAGS: [&str; 17] = [
    "protection_title",
    "protect_class",
    "designation",
    "operator",
    "owner",
    "ownership",
    "access",
    "start_date",
    "website",
    "description",
    "wikipedia",
    "wikidata",
    "leisure",
    "boundary",
    "iucn_level",
    "opening_hours",
    "fee",
];

/// Items asked a query.
const BATCH: usize = 250;

fn num(v: &Py) -> Result<f64> {
    v.number().context("a coordinate not a number")
}

/// Spherical area of a lon/lat ring, m² (Chamberlain & Duquette).
fn ring_area(ring: &[Py]) -> Result<f64> {
    let pts: Vec<(f64, f64)> = ring
        .iter()
        .map(|c| {
            let c = c.list().context("a position not a list")?;
            Ok((num(c.first().context("an empty position")?)?, num(c.get(1).context("a position without y")?)?))
        })
        .collect::<Result<_>>()?;
    let mut s = 0.0;
    for (k, &(x0, y0)) in pts.iter().enumerate() {
        let (x1, y1) = pts[(k + 1) % pts.len()];
        s += (x1 - x0).to_radians() * (2.0 + y0.to_radians().dsin() + y1.to_radians().dsin());
    }
    Ok(s.abs() * roadcore::geo::EARTH_R * roadcore::geo::EARTH_R / 2.0)
}

/// Area in km² to 3 decimals, of a Polygon or MultiPolygon (else 0).
fn area_km2(g: &Py) -> Result<f64> {
    if !g.truthy() {
        return Ok(0.0);
    }
    let c = g.at("coordinates");
    let polys: Vec<&Py> = match g.at("type").as_str() {
        Some("Polygon") => vec![c],
        Some("MultiPolygon") => c.list().context("MultiPolygon coordinates not a list")?.iter().collect(),
        _ => vec![],
    };
    let mut a = 0.0;
    for p in polys {
        let rings = p.list().context("a polygon not a list")?;
        let Some(outer) = rings.first() else { continue };
        let mut holes = 0.0;
        for h in &rings[1..] {
            holes += ring_area(h.list().context("a ring not a list")?)?;
        }
        a += ring_area(outer.list().context("a ring not a list")?)? - holes;
    }
    // (Python's max(a, 0): 0 when negative.)
    Ok(py_round(if a < 0.0 { 0.0 } else { a } / 1e6, 3))
}

/// The geometry's bounds to 5 decimals: [min x, min y, max x, max y], integers kept as they are.
fn bbox(g: &Py) -> Result<Py> {
    fn walk<'a>(c: &'a Py, xs: &mut Vec<&'a Py>, ys: &mut Vec<&'a Py>) -> Result<()> {
        let l = c.list().context("coordinates not a list")?;
        match l.first() {
            Some(Py::Int(_) | Py::Float(_) | Py::Bool(_)) => {
                xs.push(&l[0]);
                ys.push(l.get(1).context("a position without y")?);
            }
            Some(_) => {
                for x in l {
                    walk(x, xs, ys)?;
                }
            }
            None => bail!("empty coordinates"),
        }
        Ok(())
    }
    let (mut xs, mut ys) = (vec![], vec![]);
    walk(g.at("coordinates"), &mut xs, &mut ys)?;
    // (Python's min and max: the first of equals.)
    let pick = |v: &[&Py], max: bool| -> Result<Py> {
        let mut best: Option<(&Py, f64)> = None;
        for &x in v {
            let n = num(x)?;
            if best.is_none_or(|(_, b)| if max { n > b } else { n < b }) {
                best = Some((x, n));
            }
        }
        Ok(match best.context("no coordinates")?.0 {
            Py::Float(f) => Py::Float(py_round(*f, 5)),
            other => other.clone(),
        })
    };
    Ok(Py::List(vec![pick(&xs, false)?, pick(&ys, false)?, pick(&xs, true)?, pick(&ys, true)?]))
}

/// Lower-cased, letters a–z and digits only.
fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

/// Python's `re.fullmatch(r"Q\d+", s)`.
fn is_qid(s: &str) -> bool {
    s.strip_prefix('Q').is_some_and(|d| !d.is_empty() && d.chars().all(|c| pyport::pytext::decimal(c).is_some()))
}

/// A park's facts as `park_record` names them.
const KEPT: [&str; 6] = ["d_en", "w_en", "op", "inception", "area_km2", "visitors"];

/// A park's record in the pass's answers, `KEPT`'s facts alone: records an older query made hold
/// others, which nothing reads.
fn kept(rec: &Py) -> Py {
    match rec {
        Py::Dict(d) => Py::Dict(d.iter().filter(|(f, _)| KEPT.contains(&f.as_str())).cloned().collect()),
        _ => Py::Null,
    }
}

/// Each fact asked: its name in the answer (`?k`), and the pattern binding its `?v` to `?item`'s.
const FACTS: [(&str, &str); 6] = [
    ("d", r#"?item schema:description ?v FILTER(STR(LANG(?v)) = "en")"#),
    ("w", "?a schema:about ?item ; schema:isPartOf <https://en.wikipedia.org/> ; schema:name ?v"),
    ("inc", "?item wdt:P571 ?v"),
    ("area", "?item p:P2046 ?st . ?st psn:P2046 ?nv . ?nv wikibase:quantityAmount ?v"),
    ("visitors", "?item wdt:P1174 ?v"),
    ("op", r#"?item wdt:P137 ?o . ?o rdfs:label ?v FILTER(STR(LANG(?v)) = "en")"#),
];

/// The park query of `chunk`'s items: each fact's values as rows (`?item ?k ?v`), a UNION of one
/// branch a fact holding the items (crate::wikiask::fact_union, task #1144), put together by
/// `park_record`.
fn parks_query(chunk: &[String]) -> String {
    let values = chunk.iter().map(|q| format!("wd:{q}")).collect::<Vec<_>>().join(" ");
    fact_union("?item ?k ?v", &format!("VALUES ?item {{ {values} }}"), FACTS)
}

/// A park's facts from its rows (`?k ?v`, `qid`'s), each picked the same way whatever order they
/// come in: its English description and article (the least, where the service has two: QLever
/// answered two English descriptions of Q1 on 12 Nov); its earliest inception; its largest area
/// (normalized to m²) and visitors; its operators' English labels, distinct and in order. (What
/// the map reads of them: web/src/details.ts's `enrichArea`.) An unknown value (crate::wikiask::val) is none; a value that isn't a number is left out,
/// logged (crate::wikiask::number); an inception or visitors that isn't a literal is none.
fn park_record(rows: &[&Py], qid: &str) -> Py {
    let mut rec: Vec<(String, Py)> = vec![];
    for (key, k) in [("d_en", "d"), ("w_en", "w")] {
        if let Some(v) = least(rows, k).and_then(|b| val(b, "v")) {
            rec.push((key.into(), Py::Str(v.into())));
        }
    }
    if let Some(ops) = labels(rows, "op") {
        rec.push(("op".into(), Py::Str(ops)));
    }
    if let Some(v) = earliest(rows, "inc").and_then(|b| val(b, "v")) {
        rec.push(("inception".into(), Py::Str(v.chars().take(10).collect())));
    }
    if let Some(a) = largest(rows, "area", qid) {
        rec.push(("area_km2".into(), Py::Float(py_round(a / 1e6, 2))));
    }
    if let Some(f) = largest(rows, "visitors", qid).map(f64::trunc) {
        rec.push(("visitors".into(), Py::Int(if f == 0.0 { "0".into() } else { format!("{f:.0}") })));
    }
    Py::Dict(rec)
}

/// Facts of these items, `BATCH` a query (crate::wikiask::Asker::sparql_batches: a batch too
/// costly for the service asked in halves, an item too costly alone left out), by item, every item
/// of a batch answered given a record (`{}` when Wikidata has none of its facts: not asked again);
/// `save` given what's in so far after each batch, a safe point after it (what came is in the
/// pass's answers, which a resumed run doesn't ask again).
fn wd_parks(
    ask: &Asker,
    endpoint: &str,
    qids: &[String],
    mut save: impl FnMut(&[(String, Py)]) -> Result<()>,
) -> Result<Vec<(String, Py)>> {
    let mut out: Vec<(String, Py)> = vec![];
    let mut done = 0;
    ask.sparql_batches(endpoint, qids, BATCH, &parks_query, &mut |chunk, rows| {
        let by = by_item(&rows)?;
        for qid in chunk {
            out.push((qid.clone(), park_record(by.get(qid.as_str()).map_or(&[][..], |r| r), qid)));
        }
        done += chunk.len();
        eprintln!("  wikidata {done}/{}", qids.len());
        // (As the chain's stage under way, task #1145: its batches took 11 min, said by no progress.)
        jobrt::jobs::within_doing(
            done as f64 / qids.len().max(1) as f64,
            &format!("park facts from Wikidata, {done} of {}", qids.len()),
        );
        save(&out)?;
        jobrt::control::piece_saved()
    })?;
    Ok(out)
}

/// `{**have, **part}` as Python merges dicts (the part's items being those `have` lacks).
fn merged(have: &[(String, Py)], part: &[(String, Py)]) -> Py {
    Py::Dict(have.iter().chain(part).cloned().collect())
}

/// Stamps each feature of data/build/`name` with its index `i` and writes details-`layer`.jsonl:
/// its area, and what `extra` adds.
fn index_layer(b: &Path, name: &str, layer: &str, extra: impl Fn(&Py) -> Option<Vec<(String, Py)>>) -> Result<()> {
    let path = b.join(name);
    let p = phase("reading the layers");
    let mut fc = loads(&std::fs::read_to_string(&path).with_context(|| format!("read {name}"))?)
        .with_context(|| format!("parse {name}"))?;
    drop(p);
    let p = phase("making and writing the area details");
    let mut out = String::new();
    let feats = match fc.get_mut("features") {
        Some(Py::List(l)) => l,
        _ => bail!("{name}: no features"),
    };
    let n = feats.len();
    for (i, f) in feats.iter_mut().enumerate() {
        let props = f.get_mut("properties").filter(|p| matches!(p, Py::Dict(_)));
        props.with_context(|| format!("{name}: a feature without properties"))?.set("i", Py::Int(i.to_string()));
        let mut rec = Py::Dict(vec![
            ("i".into(), Py::Int(i.to_string())),
            ("area_km2".into(), Py::Float(area_km2(f.at("geometry"))?)),
        ]);
        for (k, v) in extra(f).unwrap_or_default() {
            rec.set(&k, v);
        }
        out.push_str(&dumps_spaced(&rec));
        out.push('\n');
    }
    std::fs::write(b.join(format!("details-{layer}.jsonl")), out)?;
    drop(p);
    let _p = phase("writing the layers with their indexes");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, dumps(&fc))?;
    std::fs::rename(&tmp, &path)?;
    eprintln!("{name}: {n} areas");
    Ok(())
}

/// Writes the areas' details in `root`'s data/build (see above), asking Wikidata at `wikidata`
/// for the park facts the pass's answers lack.
pub fn run(root: &Path, wikidata: &str) -> Result<()> {
    let b = root.join("data/build");
    let areas = root.join("data/areas/areas.geojsonseq");
    // Protected areas and Indigenous lands from OSM.
    let p = phase("reading and measuring the protected areas");
    let mut parks: Vec<Py> = vec![];
    let mut aboriginal: Vec<(String, Vec<Py>)> = vec![];
    let mut ab_at: HashMap<String, usize> = HashMap::new();
    let mut qids = std::collections::BTreeSet::new();
    // (Line by line: the export is gigabytes.)
    let file = std::fs::File::open(&areas).with_context(|| format!("read {}", areas.display()))?;
    for line in std::io::BufRead::lines(std::io::BufReader::new(file)) {
        let line = line?;
        let d = loads(line.trim_start_matches('\x1e')).context("a line of areas.geojsonseq")?;
        let t = d.at("properties");
        let name = [t.at("name"), t.at("name:en")].into_iter().find(|v| v.truthy()).and_then(Py::as_str).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let ty = t.get("@type").map_or_else(|| "way".to_string(), Py::py_str);
        let osm = format!("{}{}", ty.chars().next().context("an empty @type")?, t.at("@id").py_str());
        let mut rec = Py::Dict(vec![
            ("name".into(), Py::Str(name.into())),
            ("osm".into(), Py::Str(osm)),
            ("bbox".into(), bbox(d.at("geometry"))?),
            ("area_km2".into(), Py::Float(area_km2(d.at("geometry"))?)),
        ]);
        for k in PARK_TAGS {
            if t.at(k).truthy() {
                rec.set(k, t.at(k).clone());
            }
        }
        if let Some(q) = t.at("wikidata").as_str().filter(|q| is_qid(q)) {
            qids.insert(q.to_string());
        }
        if t.at("boundary").as_str() == Some("aboriginal_lands") {
            let k = norm(name);
            let at = *ab_at.entry(k.clone()).or_insert_with(|| {
                aboriginal.push((k, vec![]));
                aboriginal.len() - 1
            });
            aboriginal[at].1.push(rec);
        } else {
            parks.push(rec);
        }
    }
    drop(p);
    let p = phase("reading the Wikidata cache");
    let cache = root.join("data/areas/wikidata.json");
    let mut have: Vec<(String, Py)> = if cache.exists() {
        match loads(&std::fs::read_to_string(&cache)?).context("parse the Wikidata cache")? {
            Py::Dict(d) => d,
            _ => bail!("the Wikidata cache not an object"),
        }
    } else {
        vec![]
    };
    let known: std::collections::HashSet<&str> = have.iter().map(|(k, _)| k.as_str()).collect();
    // (A BTreeSet's order: Python's sorted.)
    let todo: Vec<String> = qids.into_iter().filter(|q| !known.contains(q.as_str())).collect();
    drop(known);
    drop(p);
    if !todo.is_empty() {
        // Saved after every batch, so a rerun picks up where a failed one stopped.
        let p = phase("fetching park facts from Wikidata");
        let ask = Asker::new();
        // (Written whole by a rename: a run killed as it writes keeps the last batch's.)
        let got = wd_parks(&ask, wikidata, &todo, |part| write_atomic(&cache, &dumps_spaced(&merged(&have, part))))?;
        drop(p);
        let _p = phase("writing the Wikidata cache");
        let Py::Dict(d) = merged(&have, &got) else { unreachable!() };
        have = d;
        write_atomic(&cache, &dumps_spaced(&Py::Dict(have.clone())))?;
    }
    let p = phase("writing the park details");
    let facts: HashMap<&str, Py> = have.iter().map(|(k, v)| (k.as_str(), kept(v))).collect();
    let add_wd = |rec: &mut Py| {
        let q = rec.at("wikidata").as_str().unwrap_or("").to_string();
        if let Some(w) = facts.get(q.as_str()).filter(|w| w.truthy()) {
            rec.set("wd", w.clone());
        }
    };
    for rec in parks.iter_mut().chain(aboriginal.iter_mut().flat_map(|(_, rs)| rs.iter_mut())) {
        add_wd(rec);
    }
    let mut out = String::new();
    for rec in &parks {
        out.push_str(&dumps_spaced(rec));
        out.push('\n');
    }
    std::fs::write(b.join("details-park.jsonl"), out)?;
    drop(p);
    let with_wd = parks.iter().filter(|r| r.get("wd").is_some()).count();
    eprintln!("details-park.jsonl: {} named protected areas ({with_wd} with Wikidata)", parks.len());

    index_layer(&b, "heritage-areas.json", "harea", |_| None)?;
    index_layer(&b, "special.json", "special", |_| None)?;
    index_layer(&b, "indigenous.json", "indigenous", |f| {
        let name = f.at("properties").at("name");
        let rs = &aboriginal[*ab_at.get(&norm(name.as_str().unwrap_or("")))?].1;
        // (Python's max: the first of the largest.)
        let mut best = &rs[0];
        for r in &rs[1..] {
            if r.at("area_km2").number() > best.at("area_km2").number() {
                best = r;
            }
        }
        let Py::Dict(d) = best else { return None };
        Some(d.iter().filter(|(k, _)| !["name", "bbox", "area_km2"].contains(&k.as_str())).cloned().collect())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_as_python() {
        // (Python: area_km2 of a 1° square at the equator, and with a hole.)
        let sq = loads(r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,1],[0,0]]]}"#).unwrap();
        assert_eq!(area_km2(&sq).unwrap(), 12363.718);
        assert_eq!(area_km2(&Py::Null).unwrap(), 0.0);
        let b = loads(r#"{"type":"Polygon","coordinates":[[[1,0.123456],[1.5,-2],[0.5,3]]]}"#).unwrap();
        assert_eq!(dumps(&bbox(&b).unwrap()), "[0.5,-2,1.5,3]");
    }

    #[test]
    fn names_and_items_as_python() {
        assert_eq!(norm("Kahnawà:ke 14"), "kahnawke14");
        assert!(is_qid("Q42") && is_qid("Q١٢") && !is_qid("Q") && !is_qid("q42") && !is_qid("Q4 "));
    }

    /// A row of the park query's answer: `q`'s fact `k`, its value the binding `v`.
    fn row(q: &str, k: &str, v: &str) -> String {
        format!(
            r#"{{"item": {{"type": "uri", "value": "http://www.wikidata.org/entity/{q}"}},
            "k": {{"type": "literal", "value": "{k}"}}, "v": {v}}}"#
        )
    }

    fn lit(s: &str) -> String {
        format!(r#"{{"type": "literal", "value": "{s}"}}"#)
    }

    fn lang(s: &str, l: &str) -> String {
        format!(r#"{{"type": "literal", "value": "{s}", "xml:lang": "{l}"}}"#)
    }

    /// 7 Nov's two failures, from recorded answers: a park whose visitors are an unknown value
    /// (QLever's genid IRI) has none, and a batch past QLever's time limit is asked in halves, each
    /// batch saved as it comes. An item Wikidata has nothing of is answered `{}` (task #1144: the
    /// rows name only items with facts), so it isn't asked again.
    #[test]
    fn unknown_values_and_costly_batches() {
        use crate::wikiask::tests::{bindings, noting, replay, QLEVER_TIMED_OUT};
        let genid =
            r#"{"type": "uri", "value": "http://www.wikidata.org/.well-known/genid/9e0e785fb54b8e85182f3338680c3feb"}"#;
        let park = |q: &str, visitors: &str| [row(q, "visitors", visitors), row(q, "area", &lit("2500000"))].join(",");
        let (url, asked) = replay(vec![
            (429, None, QLEVER_TIMED_OUT.into()),
            (200, None, bindings(&park("Q1", genid))),
            (200, None, bindings(&park("Q2", &lit("1234.7")))),
            (200, None, bindings("")),
        ]);
        let (ask, waits) = noting();
        let mut saves = vec![];
        let qids = vec!["Q1".to_string(), "Q2".to_string(), "Q3".to_string()];
        let got = wd_parks(&ask, &url, &qids[..2], |s| {
            saves.push(s.len());
            Ok(())
        })
        .unwrap();
        let got: Vec<String> = got.iter().map(|(q, r)| format!("{q} {}", dumps_spaced(r))).collect();
        assert_eq!(got, [r#"Q1 {"area_km2": 2.5}"#, r#"Q2 {"area_km2": 2.5, "visitors": 1234}"#]);
        assert_eq!(saves, [1, 2]);
        assert_eq!(asked.lock().unwrap().len(), 3);
        assert!(waits.lock().unwrap().is_empty());
        let got = wd_parks(&ask, &url, &qids[2..], |_| Ok(())).unwrap();
        assert_eq!(dumps_spaced(&got[0].1), "{}");
    }

    /// Task #1144: each fact is picked the same way whatever order the service's rows come in (the
    /// old query's SAMPLE picked differently for 80 of 173 parks asked on 11 Nov and again on 12
    /// Nov); every branch of the query holds the items, so QLever joins them before it scans.
    #[test]
    fn facts_picked_the_same_way() {
        let rows = vec![
            row("Q9", "d", &lang("national park", "en")),
            row("Q9", "d", &lang("a national park", "en")),
            row("Q9", "w", &lit("Some Park")),
            row("Q9", "inc", &lit("1972-01-01T00:00:00Z")),
            row("Q9", "inc", &lit("-0500-01-01T00:00:00Z")),
            row("Q9", "inc", &lit("1952-06-09T00:00:00Z")),
            row("Q9", "area", &lit("2500000")),
            row("Q9", "area", &lit("12500000")),
            row("Q9", "op", &lang("Parks Board", "en")),
            row("Q9", "op", &lang("Forest Service", "en")),
            row("Q9", "op", &lang("Parks Board", "en")),
            row("Q9", "visitors", &lit("lots")),
        ];
        let want = r#"{"d_en": "a national park", "w_en": "Some Park", "op": "Forest Service|Parks Board", "inception": "-0500-01-0", "area_km2": 12.5}"#;
        let parsed: Vec<Py> = rows.iter().map(|r| loads(r).unwrap()).collect();
        let mut refs: Vec<&Py> = parsed.iter().collect();
        assert_eq!(dumps_spaced(&park_record(&refs, "Q9")), want);
        refs.reverse();
        assert_eq!(dumps_spaced(&park_record(&refs, "Q9")), want, "the same in any order");
        let q = parks_query(&["Q1".into(), "Q2".into()]);
        assert_eq!(q.matches("VALUES ?item { wd:Q1 wd:Q2 }").count(), FACTS.len());
        assert!(!q.contains("OPTIONAL") && !q.contains(" LANG("), "{q}");
        // An older query's record: its facts no one reads left out.
        let old = loads(r#"{"d_en": "x", "d_loc": "y", "inst": "z", "visitors": 3}"#).unwrap();
        assert_eq!(dumps_spaced(&kept(&old)), r#"{"d_en": "x", "visitors": 3}"#);
        assert!(!kept(&loads(r#"{"d_loc": "y"}"#).unwrap()).truthy());
    }
}
