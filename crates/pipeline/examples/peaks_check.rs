//! The unit peaks against today's (docs/phase5.md "Step 4's comparison", 5): today's peaks in a box
//! (w,s,e,n) through pipeline::peaks::unit, every one of today's peaks a summit (ids from their
//! index, so near-ties may break otherwise than by OSM id), z12 from the packs else AWS's raw
//! tiles (cached under <scratch>/aws), z8 from the NAS's artifact; writes <scratch>/cmp.jsonl
//! (key, new, old). "halves": the two halves of the box, 10 km past the middle each, must agree
//! with the whole run on every peak they share.
//!   cargo run --release -p pipeline --example peaks_check -- <w,s,e,n> <scratch> [halves]
use pipeline::peaks::unit;
use std::path::Path;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let b: Vec<f64> = a[1].split(',').map(|v| v.parse().unwrap()).collect();
    let scratch = Path::new(&a[2]);
    let root = Path::new("/Volumes/personal/projects/scenic-roads");
    let out = pipeline::out::Out::open(root, &scratch.join("out"))?;
    let pois: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(out.get("global/legacy/pois").unwrap()))?)?;
    let feats = pois["features"].as_array().unwrap();
    let mut summits: Vec<pipeline::summits::Summit> = Vec::new();
    let mut peaks: Vec<unit::UnitPeak> = Vec::new();
    for f in feats {
        let p = &f["properties"];
        if p["kind"] != "peak" { continue; }
        let c = &f["geometry"]["coordinates"];
        let (lon, lat) = (c[0].as_f64().unwrap(), c[1].as_f64().unwrap());
        let i = p["i"].as_u64().unwrap();
        let (le, la) = ((lon * 1e7).round() as i32, (lat * 1e7).round() as i32);
        let ele = p["ele"].as_f64().map(|v| v as f32);
        summits.push(pipeline::summits::Summit { id: format!("n{i}"), kind: "peak".into(), lon: le, lat: la, ele, z8: None });
        if lon >= b[0] && lat >= b[1] && lon <= b[2] && lat <= b[3] {
            peaks.push(unit::UnitPeak { key: i.to_string(), id: format!("n{i}"), lon: le, lat: la, ele });
        }
    }
    summits.sort_by_key(pipeline::summits::Summit::order);
    let pack = root.join(out.get(&pipeline::terrain_z8::logical()).unwrap());
    let maxes = root.join(out.get(&pipeline::terrain_z8::max_logical()).unwrap());
    let z8 = pipeline::terrain_z8::Z8::open(&pack, &maxes, 4096)?;
    let raised = pipeline::summits::add_z8(&mut summits, &z8)?;
    eprintln!("{} summits ({} with a z8 height), {} peaks in the box", summits.len(), raised, peaks.len());
    let base8 = unit::Z8Base::new(&summits);
    let want = unit::tiles_wanted(&peaks.iter().map(|p| (p.lon as f64 * 1e-7, p.lat as f64 * 1e-7)).collect::<Vec<_>>(), 29.5);
    let raw = pipeline::terrain_pack::RawTiles::new(&scratch.join("aws"));
    let z12 = unit::UnitZ12::load(&out, &raw, &want)?;
    eprintln!("z12: {} tiles: {} packs, {} AWS, {} sea", want.len(), z12.from.0, z12.from.1, z12.from.2);
    let t = std::time::Instant::now();
    let res = unit::run(&peaks, &summits, &base8, &z12, &z8, 4)?;
    eprintln!("ran in {:.1?}", t.elapsed());
    // Determinism: the halves of the box, each with 10 km past the split, must agree with the whole
    // run on every peak they share.
    if a.get(3).map(String::as_str) == Some("halves") {
        let mid = (b[0] + b[2]) / 2.0;
        let pad = 10.0 / 111.0 / (b[1].to_radians().cos());
        for (lo, hi) in [(b[0], mid + pad), (mid - pad, b[2])] {
            let part: Vec<unit::UnitPeak> = peaks.iter().filter(|p| { let x = p.lon as f64 * 1e-7; x >= lo && x <= hi }).cloned().collect();
            let r = unit::run(&part, &summits, &base8, &z12, &z8, 4)?;
            let whole: std::collections::HashMap<&str, &pipeline::peaks::Out> = res.iter().map(|(k, o)| (k.as_str(), o)).collect();
            let differ: Vec<&String> = r.iter().filter(|(k, o)| { let w = whole[k.as_str()]; (w.e, w.p, w.pl, w.c, w.ce, w.iso, w.il, w.hi) != (o.e, o.p, o.pl, o.c, o.ce, o.iso, o.il, o.hi) }).map(|(k, _)| k).collect();
            eprintln!("half {lo:.3}..{hi:.3}: {} peaks, {} differ from the whole run {:?}", r.len(), differ.len(), &differ[..differ.len().min(5)]);
        }
    }
    let leg: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(root.join(out.get("global/legacy/peaks").unwrap()))?)?;
    let by_i: std::collections::HashMap<u64, &serde_json::Value> = leg.iter().map(|r| (r["i"].as_u64().unwrap(), r)).collect();
    let mut w = std::fs::File::create(scratch.join("cmp.jsonl"))?;
    use std::io::Write;
    for (k, o) in &res {
        let l = by_i.get(&k.parse::<u64>().unwrap());
        writeln!(w, "{}", serde_json::json!({"key": k, "new": o.json(), "old": l}))?;
    }
    Ok(())
}
