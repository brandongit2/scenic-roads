//! The marks job against today's map (docs/phase5.md "Today's regions", step 5): today's
//! stops & sights as candidates (pois.json's points, details-poi, peaks.json, the pageview table,
//! the layers' English names) through `marksjob::poi_points`, compared point by point with the
//! layers: order, properties, the filters' values, records and positions.
//!   cargo run --release -p pipeline --example marks_regression -- <legacy dir> <items.json>
use pipeline::interest::py_round;
use pipeline::marksjob::{group_of, poi_points, Candidate};
use serde_json::Value;
use std::collections::HashMap;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&a[1]);
    let views: HashMap<String, f64> = serde_json::from_slice(&std::fs::read(&a[2])?)?;
    let find = |stem: &str| -> std::path::PathBuf {
        std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{stem}."))).unwrap()
    };
    let mut det: HashMap<u64, Value> = HashMap::new();
    for line in std::fs::read_to_string(find("details-poi"))?.lines() {
        let v: Value = serde_json::from_str(line)?;
        det.insert(v["i"].as_u64().unwrap(), v);
    }
    let mut peaks: HashMap<u64, Value> = HashMap::new();
    for mut p in serde_json::from_slice::<Vec<Value>>(&std::fs::read(find("peaks"))?)? {
        let i = p["i"].as_u64().unwrap();
        p.as_object_mut().unwrap().remove("i");
        peaks.insert(i, p);
    }
    // Today's layers: per kind, in order, and their English names by index.
    let mut layers: HashMap<String, Vec<Value>> = HashMap::new();
    let mut en: HashMap<u64, String> = HashMap::new();
    for k in ["viewpoint", "peak", "waterfall", "lighthouse", "covered_bridge", "rest", "trailhead"] {
        let fc: Value = serde_json::from_slice(&std::fs::read(find(&format!("layer-pois-{k}")))?)?;
        let fs = fc["features"].as_array().unwrap().clone();
        for f in &fs {
            if let Some(e) = f["properties"]["en"].as_str() {
                en.insert(f["properties"]["i"].as_u64().unwrap(), e.to_string());
            }
        }
        layers.insert(k.to_string(), fs);
    }
    let fc: Value = serde_json::from_slice(&std::fs::read(find("pois"))?)?;
    let round6 = |v: f64| py_round(v, 6);
    let cands: Vec<Candidate> = fc["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let p = &f["properties"];
            let i = p["i"].as_u64().unwrap();
            let (lon, lat) = (f["geometry"]["coordinates"][0].as_f64().unwrap(), f["geometry"]["coordinates"][1].as_f64().unwrap());
            let d = det.get(&i).cloned().unwrap_or(Value::Null);
            let kind = p["kind"].as_str().unwrap().to_string();
            let name = p["name"].as_str().unwrap_or("").to_string();
            Candidate {
                reference: format!("legacy:poi|{kind}|{},{}|{name}", pipeline::marks::e7(round6(lon)), pipeline::marks::e7(round6(lat))),
                osm: d.get("osm").and_then(Value::as_str).and_then(pipeline::marks::osm_id),
                kind,
                lon,
                lat,
                name,
                ele: p["ele"].as_f64(),
                en: en.get(&i).cloned(),
                details: d,
                peak: peaks.get(&i).cloned(),
            }
        })
        .collect();
    let t = std::time::Instant::now();
    let pts = poi_points(&cands, &views);
    eprintln!("{} points in {:.1?}", pts.len(), t.elapsed());
    // Compared with the layers, kind by kind in order.
    let mut at: HashMap<String, usize> = HashMap::new();
    let (mut bad, mut n) = (0usize, 0usize);
    for p in &pts {
        let g = group_of(p.props["kind"].as_str().unwrap()).to_string();
        let k = at.entry(g.clone()).or_default();
        let f = &layers[&g][*k];
        *k += 1;
        n += 1;
        let mut want = f["properties"].as_object().unwrap().clone();
        let i = want.remove("i").and_then(|v| v.as_u64()).unwrap();
        let mut why = Vec::new();
        if Value::Object(p.props.clone()).to_string() != Value::Object(want.clone()).to_string() {
            why.push(format!("props {} vs {}", Value::Object(p.props.clone()), Value::Object(want)));
        }
        let c = &f["geometry"]["coordinates"];
        if (pipeline::marks::e7(round6(p.lon)), pipeline::marks::e7(round6(p.lat))) != (pipeline::marks::e7(c[0].as_f64().unwrap()), pipeline::marks::e7(c[1].as_f64().unwrap())) {
            why.push("position".into());
        }
        let mut info = det.get(&i).cloned().unwrap_or(Value::Null);
        if let (Some(pk), Value::Object(o)) = (peaks.get(&i), &mut info) {
            o.insert("peak".into(), pk.clone());
        }
        if p.info.as_deref() != (!info.is_null()).then(|| info.to_string()).as_deref() {
            why.push("record".into());
        }
        if !why.is_empty() {
            bad += 1;
            if bad <= 5 {
                println!("{g} #{}: {} | mine {:.7},{:.7} {} | layer {} {}", *k - 1, why.join("; ").chars().take(300).collect::<String>(), p.lon, p.lat, Value::Object(p.props.clone()), c, Value::Object(f["properties"].as_object().unwrap().clone()));
            }
        }
    }
    println!("{n} stops & sights: {bad} different from today's layers");
    Ok(())
}
