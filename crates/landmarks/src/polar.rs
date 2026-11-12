//! What the polar caps draw past the Web Mercator tiles' 85.05° (docs/polar.md §10): the labels,
//! the landmarks and the roads there, a GeoJSON file per pole (`polar-points`, task #668). The
//! Mercator layers can't place a point past their tiles' edge, so these come from the pass's
//! filtered planet cut to the pole (`EXTRACT_LAT`, the step's `osmium extract`) by the same rules
//! as the layers they stand for: the labels by `crate::labels`' (its rows), the landmarks the
//! landmark candidates' (the extract tool's `--candidates`), the roads the road layer's (its ways).
//!
//! Each point is ranked by its isolation as the labels and the landmarks are (crate::interest):
//! the distance to the nearest point of its group that matters more, here on the sphere over the
//! whole extract (so the points just short of 85.05° count), and each kind's other rules turned to
//! the ground (`r`: the label shows once that many metres span the label spacing, as `mz` says on
//! the Mercator side; `w`: an area's √area, its name shown once that spans crate::labels' SIZE_PX
//! at the default spacing, as `ms` says). Metres, not zooms: a zoom's ground scale goes to nothing
//! at the pole.

use crate::interest::IA_BEST;
use crate::labels::{Row, BIG_PX, DEFAULT_PX};
use anyhow::{Context, Result};
use det::Det;
use roadcore::polar::Pole;
use serde_json::{json, Map, Value};
use std::path::Path;

/// The file's version: its rules and its fields.
pub const POINTS_V: u32 = 1;

/// The poles with a file: both, whatever the coverage (the labels are the world's, crate::labels).
pub const POLES: [Pole; 2] = [Pole::North, Pole::South];

/// Whether `l` is a pole's file (`Pole::points_logical`, served at `/api/layer/polar-<n|s>` with
/// display names).
pub fn is_logical(l: &str) -> bool {
    POLES.iter().any(|&p| p.points_logical() == l)
}

/// How far from its pole a cap's extract reaches (degrees of latitude): the points past 85.05° are
/// drawn, the others only ranked beside them. A point past 85.05° whose nearest more important one
/// is further than this reaches (10° of latitude, 1,100 km and more) shows at every zoom anyway.
pub const EXTRACT_LAT: f64 = 75.0;

/// The ground's metres per pixel at zoom 0 on the equator (MapLibre's 512-pixel world), per km:
/// crate::interest::min_zoom's 78.2715.
const KM_PER_PX_Z0: f64 = 78.2715;

/// A landmark candidate (the extract tool's `pois.json`).
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// The extractor's kind (rest_area and picnic_site are the map's "rest": marksjob::group_of).
    pub kind: String,
    pub name: String,
    pub en: Option<String>,
    pub ele: Option<f64>,
    pub lon: f64,
    pub lat: f64,
    /// The OSM object ("n123") or the candidate's reference.
    pub osm: String,
}

/// A road (the extract tool's ways: what the road layer draws).
#[derive(Clone, Debug, PartialEq)]
pub struct Road {
    pub id: i64,
    pub class: u8,
    pub name: String,
    pub coords: Vec<[f64; 2]>,
}

/// The pole's points and roads from a cap's extract (`src`, OSM PBF) and the extract tool's output
/// over it (`cands`: its `pois.json`, `ways.bin`, `verts.bin`, `strings.txt`), `work` for the
/// labels' osmium files: the file as `file` makes it.
pub fn make(south: bool, src: &Path, cands: &Path, work: &Path) -> Result<Value> {
    let rows = crate::labels::read_rows(src, work)?;
    let pois = std::fs::read(cands.join("pois.json")).context("the extract's pois.json")?;
    let candidates = candidates(&pois)?;
    let roads = roads(cands)?;
    Ok(file(south, &rows, &candidates, &roads))
}

/// The landmark candidates of an extract tool's `pois.json`.
pub fn candidates(pois: &[u8]) -> Result<Vec<Candidate>> {
    let v: Value = serde_json::from_slice(pois).context("pois.json")?;
    let feats = v.get("features").and_then(Value::as_array).context("pois.json: no features")?;
    let mut out = Vec::new();
    for f in feats {
        let c = f.pointer("/geometry/coordinates").and_then(Value::as_array).context("a candidate's point")?;
        let p = f.get("properties").context("a candidate's properties")?;
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let osm = Some(s("osm")).filter(|o| !o.is_empty()).unwrap_or_else(|| s("key"));
        out.push(Candidate {
            kind: s("kind"),
            name: s("name"),
            en: p
                .pointer("/tags/name:en")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .map(str::to_string),
            ele: p.get("ele").and_then(Value::as_f64),
            lon: c.first().and_then(Value::as_f64).context("lon")?,
            lat: c.get(1).and_then(Value::as_f64).context("lat")?,
            osm,
        });
    }
    Ok(out)
}

/// The roads among an extract tool's ways (classes up to the motorway; not the ferries nor rail).
pub fn roads(dir: &Path) -> Result<Vec<Road>> {
    let ways = roadcore::Ways::open(dir)?;
    let strings = roadcore::read_strings(dir)?;
    let verts = ways.verts();
    Ok(ways
        .ways()
        .iter()
        .filter(|w| w.class <= roadcore::class::MOTORWAY)
        .map(|w| Road {
            id: w.id,
            class: w.class,
            name: strings.get(w.name as usize).cloned().unwrap_or_default(),
            coords: verts[w.vstart as usize..w.vstart as usize + w.vcount as usize]
                .iter()
                .map(|v| [v[0] as f64 * roadcore::E7, v[1] as f64 * roadcore::E7])
                .collect(),
        })
        .collect())
}

/// Whether a latitude is past the Web Mercator tiles, toward the pole `south` says.
fn past(south: bool, lat: f64) -> bool {
    if south {
        lat < -roadcore::geo::MERCATOR_LAT
    } else {
        lat > roadcore::geo::MERCATOR_LAT
    }
}

/// The great-circle distance (km) between two points.
fn km(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (p1, p2) = (a[1].to_radians(), b[1].to_radians());
    let dl = (b[0] - a[0]).to_radians();
    let (a, b) = (((p2 - p1) / 2.0).dsin(), (dl / 2.0).dsin());
    let h = a * a + p1.dcos() * p2.dcos() * b * b;
    2.0 * roadcore::geo::EARTH_R / 1000.0 * h.sqrt().min(1.0).dasin()
}

/// Each point's distance (km) to the nearest with a higher score, ties to the earlier (as
/// crate::interest::isolation orders them); `IA_BEST` for the best. Every pair: a cap's group is
/// some thousands of points at most.
fn isolation(pts: &[[f64; 2]], score: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..pts.len()).collect();
    order.sort_by(|&a, &b| score[b].total_cmp(&score[a]).then(a.cmp(&b)));
    let mut out = vec![IA_BEST; pts.len()];
    for (r, &i) in order.iter().enumerate().skip(1) {
        out[i] = order[..r].iter().map(|&j| km(pts[i], pts[j])).fold(IA_BEST, f64::min);
    }
    out
}

/// Each index of `items` with its isolation in its group (`group`).
fn grouped<T>(
    items: &[T],
    group: impl Fn(&T) -> String,
    at: impl Fn(&T) -> [f64; 2],
    score: impl Fn(&T) -> f64,
) -> Vec<f64> {
    let mut groups: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
    for (i, t) in items.iter().enumerate() {
        groups.entry(group(t)).or_default().push(i);
    }
    let mut out = vec![IA_BEST; items.len()];
    for idx in groups.values() {
        let pts: Vec<[f64; 2]> = idx.iter().map(|&i| at(&items[i])).collect();
        let sc: Vec<f64> = idx.iter().map(|&i| score(&items[i])).collect();
        for (&i, d) in idx.iter().zip(isolation(&pts, &sc)) {
            out[i] = d;
        }
    }
    out
}

/// Metres rounded to the metre (the file's distances).
fn m(km: f64) -> f64 {
    (km * 1000.0).round_ties_even()
}

/// A label's `r` (km): its isolation, or sooner by its size or population as crate::labels' `absz`
/// has it (an area from when √area spans BIG_PX at the default spacing; a place by population from
/// POP_ZOOM's zoom at its latitude), whichever reaches further.
fn label_reach(r: &Row, ia: f64) -> f64 {
    let size = r.km2.filter(|&a| a != 0.0).map_or(0.0, |a| a.sqrt() * DEFAULT_PX / BIG_PX);
    let pop = r.popz.map_or(0.0, |z| KM_PER_PX_Z0 * r.lat.to_radians().dcos() * DEFAULT_PX * (-z).dexp2());
    ia.max(size).max(pop)
}

/// A landmark's score: named ones over unnamed, then by height (a peak's), as a cap has no fame.
fn landmark_score(c: &Candidate) -> f64 {
    (if c.name.is_empty() { 0.0 } else { 1e5 }) + c.ele.unwrap_or(0.0)
}

/// The pole's file: a GeoJSON FeatureCollection of the labels (`k` place, state, water or park, `c`
/// their class), the landmarks (`k` poi, `c` the map's kind) and the roads (`k` road, `c` its class)
/// past 85.05°, each point with `n` (its name, the landmarks' maybe ""), `o` (its OSM object), `s`
/// (its score: the most important placed first), `r` (m: it shows once `r` spans the label spacing
/// on screen), and as it has them `en`, `kana` and `l` (crate::labels'), `w` (an area: √area, m) and
/// `ele` (m). In a fixed order: labels by score, landmarks by score, roads by id.
pub(crate) fn file(south: bool, rows: &[Row], cands: &[Candidate], roads: &[Road]) -> Value {
    let mut feats: Vec<(u8, f64, String, Value)> = Vec::new();
    let point = |lon: f64, lat: f64| json!({"type": "Point", "coordinates": [lon, lat]});
    let ia = grouped(rows, |r| r.kind.to_string(), |r| [r.lon, r.lat], |r| r.score);
    for (r, ia) in rows.iter().zip(ia) {
        if !past(south, r.lat) {
            continue;
        }
        let mut p = Map::new();
        p.insert("n".into(), json!(r.name));
        let en = r.en.as_deref().filter(|e| pyport::pytext::differs(&r.name, Some(e))).map(pyport::pytext::strip);
        if let Some(e) = en {
            p.insert("en".into(), json!(e));
        }
        if let Some(k) = r.kana.as_deref().filter(|_| en.is_none()) {
            p.insert("kana".into(), json!(k));
        }
        if let Some(l) = &r.langs {
            p.insert("l".into(), json!(l));
        }
        p.insert("o".into(), json!(r.oid));
        p.insert("k".into(), json!(r.kind));
        p.insert("c".into(), json!(r.class));
        p.insert("s".into(), json!(crate::interest::py_round(r.score, 2)));
        p.insert("r".into(), json!(m(label_reach(r, ia))));
        if let Some(a) = r.km2.filter(|&a| a != 0.0) {
            p.insert("w".into(), json!(m(a.sqrt())));
        }
        feats.push((
            0,
            r.score,
            r.oid.clone(),
            json!({"type": "Feature", "geometry": point(r.lon, r.lat), "properties": p}),
        ));
    }
    let kinds: Vec<&str> = cands.iter().map(|c| crate::marksjob::group_of(&c.kind)).collect();
    let ia = grouped(
        &(0..cands.len()).collect::<Vec<_>>(),
        |&i| kinds[i].to_string(),
        |&i| [cands[i].lon, cands[i].lat],
        |&i| landmark_score(&cands[i]),
    );
    for ((c, kind), ia) in cands.iter().zip(&kinds).zip(ia) {
        if !past(south, c.lat) || !crate::markconv::POI_KINDS.contains(kind) {
            continue;
        }
        let mut p = Map::new();
        p.insert("n".into(), json!(c.name));
        if let Some(e) = c.en.as_deref().filter(|e| pyport::pytext::differs(&c.name, Some(e))) {
            p.insert("en".into(), json!(e));
        }
        p.insert("o".into(), json!(c.osm));
        p.insert("k".into(), json!("poi"));
        p.insert("c".into(), json!(kind));
        let s = landmark_score(c);
        p.insert("s".into(), json!(s));
        p.insert("r".into(), json!(m(ia)));
        if let Some(e) = c.ele {
            p.insert("ele".into(), json!(e.round()));
        }
        feats.push((1, s, c.osm.clone(), json!({"type": "Feature", "geometry": point(c.lon, c.lat), "properties": p})));
    }
    for r in roads.iter().filter(|r| r.coords.iter().any(|c| past(south, c[1]))) {
        let mut p = Map::new();
        if !r.name.is_empty() {
            p.insert("n".into(), json!(r.name));
        }
        p.insert("o".into(), json!(format!("w{}", r.id)));
        p.insert("k".into(), json!("road"));
        p.insert("c".into(), json!(roadcore::class::NAMES[r.class as usize]));
        let g = json!({"type": "LineString", "coordinates": r.coords});
        feats.push((2, r.id as f64, String::new(), json!({"type": "Feature", "geometry": g, "properties": p})));
    }
    // (Most important first within labels and landmarks, ties by OSM object; roads by id.)
    feats.sort_by(|a, b| {
        a.0.cmp(&b.0).then(if a.0 == 2 { a.1.total_cmp(&b.1) } else { b.1.total_cmp(&a.1) }).then_with(|| a.2.cmp(&b.2))
    });
    json!({"type": "FeatureCollection", "features": feats.into_iter().map(|f| f.3).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: &'static str, class: &str, name: &str, lon: f64, lat: f64, score: f64) -> Row {
        Row {
            kind,
            class: class.into(),
            name: name.into(),
            lon,
            lat,
            score,
            en: None,
            km2: None,
            popz: None,
            kana: None,
            langs: None,
            oid: format!("n{}", name.len()),
        }
    }

    fn cand(kind: &str, name: &str, lon: f64, lat: f64, ele: Option<f64>) -> Candidate {
        Candidate { kind: kind.into(), name: name.into(), en: None, ele, lon, lat, osm: format!("n{lon}") }
    }

    #[test]
    fn distances_on_the_sphere_across_the_pole() {
        // (Two points 1° from the South Pole on opposite meridians: 2° of a meridian apart.)
        let d = km([0.0, -89.0], [180.0, -89.0]);
        assert!((d - 2.0 * std::f64::consts::PI / 180.0 * roadcore::geo::EARTH_R / 1000.0).abs() < 1e-6, "{d}");
        assert_eq!(isolation(&[[0.0, -89.0], [180.0, -89.0], [90.0, -89.0]], &[1.0, 2.0, 2.0]), {
            let q = km([90.0, -89.0], [180.0, -89.0]);
            vec![q.min(km([0.0, -89.0], [90.0, -89.0])), IA_BEST, q]
        });
    }

    /// Only what's past 85.05° on the pole's side is in the file; its neighbours just short of it
    /// rank it; the labels' reach is their isolation or their population's, whichever is further.
    #[test]
    fn the_file_holds_what_lies_past_the_tiles() {
        let mut pole = row("place", "locality", "South Pole", 0.0, -90.0, 11.0);
        pole.oid = "n1".into();
        let mut station = row("place", "isolated_dwelling", "Amundsen-Scott", 139.27, -89.9975, 3.0);
        station.popz = Some(14.0);
        station.oid = "n2".into();
        let near = row("place", "locality", "Short of it", 0.0, -85.0, 20.0);
        let north = row("place", "locality", "North Pole", 0.0, 90.0, 11.0);
        let peaks = [
            cand("peak", "Mount A", 10.0, -86.0, Some(4000.0)),
            cand("peak", "", 10.1, -86.0, Some(4100.0)),
            cand("viewpoint", "", 0.0, -60.0, None),
        ];
        let roads = [
            Road {
                id: 7,
                class: roadcore::class::SERVICE,
                name: "Skiway".into(),
                coords: vec![[0.0, -89.99], [1.0, -89.98]],
            },
            Road {
                id: 8,
                class: roadcore::class::SERVICE,
                name: String::new(),
                coords: vec![[0.0, -80.0], [1.0, -80.0]],
            },
        ];
        let f = file(true, &[pole, station.clone(), near.clone(), north], &peaks, &roads);
        let feats = f["features"].as_array().unwrap();
        let names: Vec<&str> = feats.iter().map(|f| f["properties"]["n"].as_str().unwrap_or("-")).collect();
        assert_eq!(names, ["South Pole", "Amundsen-Scott", "Mount A", "", "Skiway"]);
        // (The pole's nearest more important label: the one short of 85.05°.)
        let r = |i: usize| feats[i]["properties"]["r"].as_f64().unwrap();
        assert_eq!(r(0), m(km([0.0, -90.0], [0.0, -85.0])));
        // (The station: its isolation from the pole, 280 m, under its population's reach.)
        let pop = KM_PER_PX_Z0 * (-89.9975f64).to_radians().dcos() * DEFAULT_PX * (-14.0f64).dexp2();
        assert_eq!(r(1), m(km([139.27, -89.9975], [0.0, -90.0]).max(pop)));
        // (A named peak over a higher unnamed one; the unnamed one 3.7 km from it.)
        assert_eq!(r(2), m(IA_BEST));
        assert_eq!(r(3), m(km([10.0, -86.0], [10.1, -86.0])));
        assert_eq!(feats[3]["properties"]["ele"], json!(4100.0));
        assert_eq!(feats[4]["geometry"]["type"], "LineString");
        assert_eq!(feats[4]["properties"]["c"], "service");
        // (The north's file: the North Pole alone.)
        let f = file(false, &[station, near, row("place", "locality", "North Pole", 0.0, 90.0, 11.0)], &peaks, &roads);
        assert_eq!(f["features"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn candidates_from_the_extract() {
        let pois = br#"{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"Point","coordinates":[1.5,-86.25]},"properties":{"kind":"peak","name":"Mount B","ele":3500.0,"osm":"n9","key":null,"tags":{"name:en":" Mt B "}}},{"type":"Feature","geometry":{"type":"Point","coordinates":[2.0,-87.0]},"properties":{"kind":"trailhead","name":"","ele":null,"osm":null,"key":"trail:1:2","tags":{}}}]}"#;
        let c = candidates(pois).unwrap();
        assert_eq!(
            c,
            [
                Candidate {
                    kind: "peak".into(),
                    name: "Mount B".into(),
                    en: Some("Mt B".into()),
                    ele: Some(3500.0),
                    lon: 1.5,
                    lat: -86.25,
                    osm: "n9".into()
                },
                Candidate {
                    kind: "trailhead".into(),
                    name: String::new(),
                    en: None,
                    ele: None,
                    lon: 2.0,
                    lat: -87.0,
                    osm: "trail:1:2".into()
                },
            ]
        );
    }

    /// A run on a real cut: `SCENIC_POLAR_POINTS_RUN="<n|s> <cut> <extract dir> <work dir> <out>"`
    /// makes the pole's file as the step does from a cut and the extract tool's output over it, all
    /// local. Unset, nothing.
    #[test]
    fn make_on_a_real_cut() {
        let Ok(m) = std::env::var("SCENIC_POLAR_POINTS_RUN") else { return };
        let a: Vec<&str> = m.split_whitespace().collect();
        let f = make(a[0] == "s", Path::new(a[1]), Path::new(a[2]), Path::new(a[3])).unwrap();
        std::fs::write(a[4], serde_json::to_vec(&f).unwrap()).unwrap();
    }
}
