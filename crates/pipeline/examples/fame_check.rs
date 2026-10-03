//! The marks job's legacy-input regression for the stops & sights (docs/phase5.md "Today's
//! regions", step 5): from today's files (the layers' properties and order, details-poi, the
//! pageview table) recompute fame, pageviews, isolation and the label zoom as dem/interest.py did,
//! and compare with the layers' own values.
//!   cargo run --release -p pipeline --example fame_check -- <mirror global/legacy dir> <items.json>
use pipeline::interest::{fame, isolation, min_zoom, percentile, poi_base, py_round, view_item};
use serde_json::Value;
use std::collections::HashMap;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[1]);
    let views: HashMap<String, f64> = serde_json::from_slice(&std::fs::read(&a[2])?)?;
    let find = |stem: &str| -> anyhow::Result<std::path::PathBuf> {
        std::fs::read_dir(dir)?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{stem}."))).ok_or_else(|| anyhow::anyhow!("no {stem}"))
    };
    // Today's points as interest.py had them (pois.json: unrounded coordinates, its values), and as
    // the map has them (the layers, after layers.py's repeated names).
    let fc: Value = serde_json::from_slice(&std::fs::read(find("pois")?)?)?;
    let pts: Vec<(usize, Value)> = fc["features"].as_array().unwrap().iter().map(|f| (f["properties"]["i"].as_u64().unwrap() as usize, f.clone())).collect();
    anyhow::ensure!(pts.iter().enumerate().all(|(k, p)| p.0 == k), "pois.json not in index order");
    let mut layer_mz: HashMap<usize, Value> = HashMap::new();
    for k in ["viewpoint", "peak", "waterfall", "lighthouse", "covered_bridge", "rest", "trailhead"] {
        let fc: Value = serde_json::from_slice(&std::fs::read(find(&format!("layer-pois-{k}"))?)?)?;
        for f in fc["features"].as_array().unwrap() {
            layer_mz.insert(f["properties"]["i"].as_u64().unwrap() as usize, f["properties"]["mz"].clone());
        }
    }
    let mut det: HashMap<u64, Value> = HashMap::new();
    for line in std::fs::read_to_string(find("details-poi")?)?.lines() {
        let v: Value = serde_json::from_str(line)?;
        det.insert(v["i"].as_u64().unwrap(), v);
    }
    let n = pts.len();
    let prop = |i: usize, k: &str| pts[i].1["properties"].get(k).cloned().unwrap_or(Value::Null);
    let num = |v: &Value| v.as_f64();
    let kind = |i: usize| prop(i, "kind").as_str().unwrap_or("").to_string();
    let group = |i: usize| { let k = kind(i); if k == "rest_area" || k == "picnic_site" { "rest".to_string() } else { k } };
    // Python truthiness for the lighthouse's `fh or h or rg`.
    let truthy = |v: &Value| !(v.is_null() || v.as_f64() == Some(0.0) || v.as_str() == Some("") || v == &Value::Bool(false));
    let size = |i: usize| -> Option<f64> {
        match kind(i).as_str() {
            "peak" if !prop(i, "pr").is_null() => num(&prop(i, "pr")),
            "peak" | "viewpoint" => num(&prop(i, "ele")),
            "waterfall" => num(&prop(i, "h")),
            "lighthouse" => {
                let (fh, h, rg) = (prop(i, "fh"), prop(i, "h"), prop(i, "rg"));
                num(if truthy(&fh) { &fh } else if truthy(&h) { &h } else { &rg })
            }
            "covered_bridge" => num(&prop(i, "len")),
            _ => None,
        }
    };
    let mut base = vec![0f64; n];
    let mut pv_ = vec![None; n];
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for i in 0..n {
        groups.entry(group(i)).or_default().push(i);
    }
    for (g, idx) in &groups {
        let sp = percentile(&idx.iter().map(|&i| size(i)).collect::<Vec<_>>());
        for (m, &i) in idx.iter().enumerate() {
            let d = det.get(&(pts[i].0 as u64)).cloned().unwrap_or(Value::Null);
            let mut qid = d.get("wikidata").and_then(Value::as_str).unwrap_or("").split(';').next().unwrap_or("").trim().to_string();
            if g == "viewpoint" && !view_item(d["wd"].get("d_en").and_then(Value::as_str).unwrap_or("")) {
                qid.clear();
            }
            let sl = if qid.is_empty() { 0 } else { d["wd"].get("sl").and_then(Value::as_u64).unwrap_or(0) };
            let (f, pv) = fame(if qid.is_empty() { None } else { views.get(&qid).copied() }, sl);
            let rich = ["wikidata", "wikipedia", "description", "website", "image"].iter().filter(|t| d.get(**t).is_some_and(|v| truthy(v))).count();
            let named = prop(i, "name").as_str().is_some_and(|s| !s.is_empty());
            base[i] = poi_base(f, named, rich, sp[m]);
            pv_[i] = pv;
        }
    }
    let (mut ia, mut lon, mut lat) = (vec![0f64; n], vec![0f64; n], vec![0f64; n]);
    for i in 0..n {
        let c = &pts[i].1["geometry"]["coordinates"];
        lon[i] = c[0].as_f64().unwrap();
        lat[i] = c[1].as_f64().unwrap();
    }
    for idx in groups.values() {
        let r = isolation(&idx.iter().map(|&i| lon[i]).collect::<Vec<_>>(), &idx.iter().map(|&i| lat[i]).collect::<Vec<_>>(), &idx.iter().map(|&i| base[i]).collect::<Vec<_>>());
        for (m, &i) in idx.iter().enumerate() {
            ia[i] = r[m];
        }
    }
    let (mut bad_fa, mut bad_pv, mut bad_ia, mut bad_mz) = (0, 0, 0, 0);
    for i in 0..n {
        let fa = py_round(base[i], 3);
        let pv = pv_[i].map(|v| py_round(v, 0));
        let (ia1, mz) = (py_round(ia[i], 1), min_zoom(lat[i], ia[i]));
        let show = |what: &str, a: String, b: String| eprintln!("{what} i {}: {a} vs {b} ({})", pts[i].0, kind(i));
        if Some(fa) != num(&prop(i, "fa")) {
            bad_fa += 1;
            if bad_fa <= 3 { show("fa", fa.to_string(), prop(i, "fa").to_string()); }
        }
        if pv != num(&prop(i, "pv")) {
            bad_pv += 1;
            if bad_pv <= 3 { show("pv", format!("{pv:?}"), prop(i, "pv").to_string()); }
        }
        if Some(ia1) != num(&prop(i, "ia")) {
            bad_ia += 1;
            if bad_ia <= 3 { show("ia", ia1.to_string(), prop(i, "ia").to_string()); }
        }
        if Some(mz) != num(&prop(i, "mz")) {
            bad_mz += 1;
            if bad_mz <= 3 { show("mz", mz.to_string(), prop(i, "mz").to_string()); }
        }
    }
    println!("{n} stops & sights, interest.py's values: fa {bad_fa}, pv {bad_pv}, ia {bad_ia}, mz {bad_mz} different");
    // layers.py: repeated names wait (same_names), in the layers' fame order.
    let fa: Vec<f64> = base.iter().map(|b| py_round(*b, 3)).collect();
    let ia1: Vec<f64> = ia.iter().map(|v| py_round(*v, 1)).collect();
    let mut mz: Vec<f64> = (0..n).map(|i| min_zoom(lat[i], ia[i])).collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| fa[a].partial_cmp(&fa[b]).unwrap());
    let waits = pipeline::interest::same_names(
        &order,
        &|i| { let k = kind(i); (if k == "rest_area" || k == "picnic_site" { "rest".to_string() } else { k }, prop(i, "name").as_str().unwrap_or("").to_string()) },
        &lon, &lat, &fa, &ia1, &mut mz,
    );
    let bad = (0..n).filter(|&i| Some(mz[i]) != layer_mz.get(&pts[i].0).and_then(Value::as_f64)).count();
    println!("layers.py: {waits} names wait; the layers' mz: {bad} different");
    Ok(())
}
