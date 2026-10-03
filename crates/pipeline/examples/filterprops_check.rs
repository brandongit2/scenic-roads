//! filterprops.py's values for today's stops & sights, recomputed from details-poi and peaks.json,
//! against the map's layers (pr, is, h, fh, rg, y, len, pan, tw, fac).
use serde_json::Value;
use std::collections::HashMap;

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("legacy dir"));
    let find = |stem: &str| -> std::path::PathBuf {
        std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(&format!("{stem}."))).unwrap()
    };
    let mut det: HashMap<u64, Value> = HashMap::new();
    for line in std::fs::read_to_string(find("details-poi"))?.lines() {
        let v: Value = serde_json::from_str(line)?;
        det.insert(v["i"].as_u64().unwrap(), v);
    }
    let mut peaks: HashMap<u64, Value> = HashMap::new();
    for p in serde_json::from_slice::<Vec<Value>>(&std::fs::read(find("peaks"))?)? {
        peaks.insert(p["i"].as_u64().unwrap(), p);
    }
    let (mut n, mut bad) = (0, 0);
    for k in ["viewpoint", "peak", "waterfall", "lighthouse", "covered_bridge", "rest", "trailhead"] {
        let fc: Value = serde_json::from_slice(&std::fs::read(find(&format!("layer-pois-{k}")))?)?;
        for f in fc["features"].as_array().unwrap() {
            let p = &f["properties"];
            let i = p["i"].as_u64().unwrap();
            let d = det.get(&i).cloned().unwrap_or(Value::Null);
            let got = pipeline::interest::poi_filter_props(p["kind"].as_str().unwrap(), &d, peaks.get(&i));
            n += 1;
            for key in ["pr", "is", "h", "fh", "rg", "y", "len", "pan", "tw", "fac"] {
                let (a, b) = (got.get(key).and_then(Value::as_f64), p.get(key).and_then(Value::as_f64));
                if a != b {
                    bad += 1;
                    if bad <= 8 {
                        println!("i {i} ({}): {key} {:?} vs {:?}", p["kind"], got.get(key), p.get(key));
                    }
                }
            }
        }
    }
    println!("{n} stops & sights: {bad} filter values different");
    Ok(())
}
