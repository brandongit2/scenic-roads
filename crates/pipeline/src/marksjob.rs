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
