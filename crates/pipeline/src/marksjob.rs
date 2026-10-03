//! The `marks` job (docs/phase5.md "Build"): landmark points from their candidates, as today's
//! scripts made them (dem/filterprops.py, dem/interest.py, dem/layers.py), for markdata and the
//! thinned tiles (`markconv::write`). This part: the stops & sights.

use crate::interest::{fame, isolation, min_zoom, percentile, poi_base, poi_filter_props, py_round, same_names, view_item};
use crate::markconv::Point;
use crate::marks::{self, flag, MarkPt};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// A stop & sight before ranking: what extraction and the details steps know of it.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// The extractor's kind (rest_area and picnic_site are the map's "rest").
    pub kind: String,
    pub lon: f64,
    pub lat: f64,
    pub name: String,
    pub ele: Option<f64>,
    /// Its English name (OSM's name:en, else what the names step found).
    pub en: Option<String>,
    /// The details record: the OSM tags kept, `osm`, `wd` (Wikidata facts), `length_m` …
    pub details: Value,
    /// The peak computation (prominence `p`, isolation `iso`, …), for peaks.
    pub peak: Option<Value>,
    /// What its id is made from.
    pub osm: Option<u64>,
    pub reference: String,
}

/// The map's kind of a stop & sight (its file, its filters): rest areas and picnic sites are one.
pub fn group_of(kind: &str) -> &str {
    if kind == "rest_area" || kind == "picnic_site" {
        "rest"
    } else {
        kind
    }
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// The stops & sights as points: their filter values, fame, isolation and label zoom (with repeated
/// names held back), lean properties, popup records, in the map's order (fame ascending). `views`:
/// monthly pageviews per Wikidata item.
pub fn poi_points(cands: &[Candidate], views: &HashMap<String, f64>) -> Vec<Point> {
    let n = cands.len();
    // The filters' values, as properties.
    let mut props: Vec<Map<String, Value>> = cands
        .iter()
        .map(|c| {
            let mut p = Map::new();
            p.insert("ele".into(), c.ele.map_or(Value::Null, |e| json!(e)));
            p.insert("kind".into(), json!(c.kind));
            p.insert("name".into(), json!(c.name));
            p.extend(poi_filter_props(&c.kind, &c.details, c.peak.as_ref()));
            p
        })
        .collect();
    // Fame, per kind: pageviews (else sitelinks), with ties broken by name, how much OSM says and
    // size within the kind.
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        groups.entry(group_of(&c.kind)).or_default().push(i);
    }
    let num = |p: &Map<String, Value>, k: &str| p.get(k).and_then(Value::as_f64);
    let size = |i: usize, p: &Map<String, Value>| -> Option<f64> {
        match cands[i].kind.as_str() {
            "peak" if p.get("pr").is_some_and(|v| !v.is_null()) => num(p, "pr"),
            "peak" | "viewpoint" => num(p, "ele"),
            "waterfall" => num(p, "h"),
            "lighthouse" => {
                let null = Value::Null;
                let (fh, h, rg) = (p.get("fh").unwrap_or(&null), p.get("h").unwrap_or(&null), p.get("rg").unwrap_or(&null));
                (if truthy(fh) { fh } else if truthy(h) { h } else { rg }).as_f64()
            }
            "covered_bridge" => num(p, "len"),
            _ => None,
        }
    };
    let mut base = vec![0f64; n];
    let mut pv: Vec<Option<f64>> = vec![None; n];
    for (g, idx) in &groups {
        let sp = percentile(&idx.iter().map(|&i| size(i, &props[i])).collect::<Vec<_>>());
        for (m, &i) in idx.iter().enumerate() {
            let d = &cands[i].details;
            let mut qid = d.get("wikidata").and_then(Value::as_str).unwrap_or("").split(';').next().unwrap_or("").trim().to_string();
            if *g == "viewpoint" && !view_item(d["wd"].get("d_en").and_then(Value::as_str).unwrap_or("")) {
                qid.clear();
            }
            let sl = if qid.is_empty() { 0 } else { d["wd"].get("sl").and_then(Value::as_u64).unwrap_or(0) };
            let (f, v) = fame(if qid.is_empty() { None } else { views.get(&qid).copied() }, sl);
            let rich = ["wikidata", "wikipedia", "description", "website", "image"].iter().filter(|t| d.get(**t).is_some_and(truthy)).count();
            base[i] = poi_base(f, !cands[i].name.is_empty(), rich, sp[m]);
            pv[i] = v;
        }
    }
    // Isolation per kind, in the candidates' order (ties: the earlier one).
    let mut ia = vec![0f64; n];
    for idx in groups.values() {
        let r = isolation(&idx.iter().map(|&i| cands[i].lon).collect::<Vec<_>>(), &idx.iter().map(|&i| cands[i].lat).collect::<Vec<_>>(), &idx.iter().map(|&i| base[i]).collect::<Vec<_>>());
        for (m, &i) in idx.iter().enumerate() {
            ia[i] = r[m];
        }
    }
    let fa: Vec<f64> = base.iter().map(|b| py_round(*b, 3)).collect();
    let ia1: Vec<f64> = ia.iter().map(|v| py_round(*v, 1)).collect();
    let mut mz: Vec<f64> = (0..n).map(|i| min_zoom(cands[i].lat, ia[i])).collect();
    // The map's order: by fame (stable), and repeated names wait in it.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| fa[a].partial_cmp(&fa[b]).unwrap());
    let lon: Vec<f64> = cands.iter().map(|c| c.lon).collect();
    let lat: Vec<f64> = cands.iter().map(|c| c.lat).collect();
    same_names(&order, &|i| (group_of(&cands[i].kind).to_string(), cands[i].name.clone()), &lon, &lat, &fa, &ia1, &mut mz);
    let mut out = Vec::with_capacity(n);
    let mut rank_in: HashMap<&str, u32> = HashMap::new();
    for &i in &order {
        let c = &cands[i];
        let g = group_of(&c.kind);
        let k = marks::kind_index(g).expect("a stops & sights kind");
        let p = &mut props[i];
        p.insert("fa".into(), json!(fa[i]));
        if let Some(v) = pv[i] {
            p.insert("pv".into(), json!(py_round(v, 0) as i64));
        }
        p.insert("ia".into(), json!(ia1[i]));
        p.insert("mz".into(), json!(mz[i]));
        if let Some(en) = &c.en {
            p.insert("en".into(), json!(en));
        }
        let rank = rank_in.entry(g).or_insert(0);
        let pt = MarkPt {
            lon: marks::e7(c.lon),
            lat: marks::e7(c.lat),
            fa: fa[i] as f32,
            ia: ia1[i] as f32,
            mz: mz[i] as f32,
            rank: *rank,
            kz: marks::KZ_NONE,
            class: 0,
            tier: 0,
            flags: if c.name.is_empty() { 0 } else { flag::NAMED } | if c.kind == "picnic_site" { flag::PICNIC } else { 0 },
        };
        *rank += 1;
        let fvals = marks::fields(g).iter().map(|f| marks::num(p, f)).collect();
        // The popup record: the details, with the peak computation (as the server merged them).
        let mut info = c.details.clone();
        if let (Some(pk), Value::Object(o)) = (&c.peak, &mut info) {
            o.insert("peak".into(), pk.clone());
        }
        out.push(Point {
            kind: k,
            lon: c.lon,
            lat: c.lat,
            pt,
            fvals,
            props: p.clone(),
            info: (!info.is_null()).then(|| info.to_string()),
            osm: c.osm,
            reference: c.reference.clone(),
        });
    }
    out
}

/// The tags today's popups keep, per kind (dem/poidetails.py COMMON and KEEP).
pub fn detail_keys(kind: &str) -> Vec<&'static str> {
    let common = ["description", "website", "wikipedia", "wikidata", "operator", "access", "fee", "opening_hours", "start_date", "heritage", "alt_name", "name:en"];
    let own: &[&str] = match kind {
        "peak" => &["prominence", "isolation", "munro", "corbett", "graham", "donald", "marilyn", "hewitt", "wainwright", "nuttall", "communication:amateur_radio:sota", "summit:cross", "summit:register", "volcano:status", "volcano:type", "natural"],
        "waterfall" => &["height", "width", "intermittent", "seasonal"],
        "lighthouse" => &[
            "height", "seamark:light:character", "seamark:light:colour", "seamark:light:period", "seamark:light:range", "seamark:light:height", "seamark:light:sequence", "seamark:light:reference", "seamark:name",
            "building:colour", "tower:type", "historic", "heritage:operator", "seamark:light:1:character", "seamark:light:1:colour", "seamark:light:1:period", "seamark:light:1:range", "seamark:light:1:height",
        ],
        "viewpoint" => &["direction", "tower:type", "height", "ele", "man_made"],
        "picnic_site" => &["toilets", "drinking_water", "shelter", "bench", "picnic_table", "fireplace", "bbq", "covered", "capacity"],
        "rest_area" => &["toilets", "drinking_water", "shelter", "picnic_table", "bench", "fuel", "restaurant", "shop", "wheelchair", "capacity"],
        "trailhead" => &["toilets", "drinking_water", "parking", "capacity", "route_ref", "hiking", "shelter"],
        "covered_bridge" => &["bridge:structure", "bridge:name", "material", "historic", "bridge:ref", "layer"],
        _ => &[],
    };
    common.iter().chain(own).copied().collect()
}

/// A unit's candidate (crate::candidates) as the job's: its details record as dem/poidetails.py
/// made it (the kept tags of its kind's lists, `osm`, `length_m`, `viewpoint`, and the facts of a
/// single-QID tag as `wd`), its peak result (a `work/peaks` record), its key as the reference.
pub fn from_unit(c: &crate::candidates::Cand, peak: Option<Value>, facts: &HashMap<String, Value>) -> Candidate {
    let mut d = Map::new();
    if let Some(o) = &c.osm {
        d.insert("osm".into(), json!(o));
    }
    for k in detail_keys(&c.kind) {
        if let Some(v) = c.tags.get(k).filter(|v| !v.is_empty()) {
            d.insert(k.into(), json!(v));
        }
    }
    if let Some(m) = c.length_m {
        d.insert("length_m".into(), json!(m));
    }
    if c.viewpoint {
        d.insert("viewpoint".into(), json!("yes"));
    }
    // As today: the facts only for a tag that is one QID.
    if let Some(q) = c.qid.as_deref().filter(|q| q.len() > 1 && q.starts_with('Q') && q[1..].bytes().all(|b| b.is_ascii_digit())) {
        if let Some(f) = facts.get(q) {
            d.insert("wd".into(), f.clone());
        }
    }
    Candidate {
        kind: c.kind.clone(),
        lon: c.lon as f64 * 1e-7,
        lat: c.lat as f64 * 1e-7,
        name: c.name.clone(),
        ele: c.ele.map(f64::from),
        en: c.en.clone(),
        details: Value::Object(d),
        peak,
        osm: c.osm.as_deref().and_then(marks::osm_id),
        reference: c.key.clone(),
    }
}

/// The named peaks with a height, by height (stable over the map's order), for the highest in
/// view (dem/layers.py's summits list: positions to 5 decimals, heights whole).
pub fn summits_list(pts: &[Point]) -> Vec<(marks::SummitRec, String)> {
    let k = marks::kind_index("peak").unwrap();
    let mut named: Vec<(f64, f64, f64, String)> = pts
        .iter()
        .filter(|p| p.kind == k)
        .filter_map(|p| {
            let name = p.props.get("name").and_then(Value::as_str).filter(|s| !s.is_empty())?;
            let ele = p.props.get("ele").and_then(Value::as_f64)?;
            Some((ele, p.lon, p.lat, name.to_string()))
        })
        .collect();
    named.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    named
        .into_iter()
        .enumerate()
        // (Today's layers.py rounded the points to 6 decimals, then the summits list to 5.)
        .map(|(rank, (e, x, y, n))| (marks::SummitRec { rank: rank as u32, lon: marks::e7(py_round(py_round(x, 6), 5)), lat: marks::e7(py_round(py_round(y, 6), 5)), pad: 0, ele: py_round(e, 0) }, n))
        .collect()
}
