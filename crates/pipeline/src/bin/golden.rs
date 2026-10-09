//! Golden comparison of the legacy server and the new one (docs/plan.md §10, phase 1): the same
//! data served two ways should agree.
//!
//!   golden --legacy-build data/build --legacy http://127.0.0.1:8080 --new http://127.0.0.1:18081
//!          --bbox w,s,e,n [--n 400] [--tiles 300]
//!
//! - way info: sampled ways in the box, by legacy index there and by OSM id plus a point here; every
//!   shared field must match;
//! - profiles: the elevation at every vertex both profiles hold (the whole road differs by design:
//!   the new chaining);
//! - tiles: terrain, slope and tree tiles byte for byte; road and rail tiles present in both;
//! - layers: feature counts; details: records value for value.
//!
//! Prints a summary and exits non-zero when anything that must match doesn't.

use det::Det;
use anyhow::{Context, Result};
use pipeline::legacy::Legacy;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

struct Http(ureq::Agent);

impl Http {
    fn get(&self, url: &str) -> Result<(u16, Vec<u8>)> {
        pipeline::fetch::online(url)?;
        let mut r = self.0.get(url).header("Accept-Encoding", "identity").call().with_context(|| format!("GET {url}"))?;
        let st = r.status().as_u16();
        let b = r.body_mut().with_config().limit(200 << 20).read_to_vec()?;
        Ok((st, b))
    }

    fn json(&self, url: &str) -> Result<Option<Value>> {
        let (st, b) = self.get(url)?;
        if st != 200 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&b).with_context(|| format!("{url}: not JSON"))?))
    }
}

/// Field-by-field differences between two way infos (the fields both have, minus the ones that
/// differ by design).
fn way_diffs(a: &Value, b: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else { return vec!["not objects".into()] };
    for (k, va) in a {
        if matches!(k.as_str(), "idx" | "name_en") {
            continue;
        }
        let Some(vb) = b.get(k) else {
            out.push(format!("{k}: missing"));
            continue;
        };
        let same = match (va.as_f64(), vb.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= 1e-3 * x.abs().max(1.0),
            _ => va == vb,
        };
        if !same {
            out.push(format!("{k}: {va} vs {vb}"));
        }
    }
    out
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let build = PathBuf::from(opt(&args, "--legacy-build").context("--legacy-build <dir>")?);
    let old = opt(&args, "--legacy").context("--legacy <url>")?;
    let new = opt(&args, "--new").context("--new <url>")?;
    let bb: Vec<f64> = opt(&args, "--bbox").context("--bbox w,s,e,n")?.split(',').map(|x| x.parse()).collect::<Result<_, _>>()?;
    let n: usize = opt(&args, "--n").map(|x| x.parse()).transpose()?.unwrap_or(400);
    let ntiles: usize = opt(&args, "--tiles").map(|x| x.parse()).transpose()?.unwrap_or(300);
    let http = Http(ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(120))).http_status_as_error(false).build().into());
    let mut fails = 0usize;

    // ---- ways ---------------------------------------------------------------------------------
    let lg = Legacy::open(&build)?;
    let e7 = |v: f64| (v * 1e7).round() as i32;
    let (w, s, e, nn) = (e7(bb[0]), e7(bb[1]), e7(bb[2]), e7(bb[3]));
    let ways = lg.ways.ways();
    let verts = lg.ways.verts();
    let inside: Vec<usize> = (0..ways.len())
        .filter(|&i| {
            let p = verts[ways[i].vstart as usize];
            ways[i].vcount >= 2 && p[0] >= w && p[0] < e && p[1] >= s && p[1] < nn
        })
        .collect();
    let step = (inside.len() / n.max(1)).max(1);
    let sample: Vec<usize> = inside.iter().step_by(step).take(n).copied().collect();
    eprintln!("ways: {} in the box, comparing {}", inside.len(), sample.len());
    let (mut way_ok, mut way_bad, mut way_missing) = (0, 0, 0);
    let mut field_diffs: HashMap<String, usize> = HashMap::new();
    let (mut prof_pts, mut prof_close, mut prof_max, mut prof_n) = (0usize, 0usize, 0f64, 0usize);
    let mut len_ratio: Vec<f64> = Vec::new();
    for (k, &i) in sample.iter().enumerate() {
        let wr = &ways[i];
        let r = Legacy::range(wr);
        let mid = verts[r.start + (r.len() / 2)];
        let at = format!("{:.6},{:.6}", mid[0] as f64 * 1e-7, mid[1] as f64 * 1e-7);
        let a = http.json(&format!("{old}/api/way/{i}"))?;
        let b = http.json(&format!("{new}/api/way/{}?at={at}", wr.id))?;
        match (a, b) {
            (Some(a), Some(b)) => {
                let d = way_diffs(&a, &b);
                if d.is_empty() {
                    way_ok += 1;
                } else {
                    way_bad += 1;
                    for x in &d {
                        *field_diffs.entry(x.split(':').next().unwrap_or("").to_string()).or_default() += 1;
                    }
                    if way_bad <= 5 {
                        eprintln!("  way {} (legacy {i}): {}", wr.id, d.join("; "));
                    }
                }
            }
            (Some(_), None) => {
                way_missing += 1;
                if way_missing <= 5 {
                    eprintln!("  way {} (legacy {i}) at {at}: not found by the new server", wr.id);
                }
            }
            _ => {}
        }
        // Profiles, for a quarter of the sample.
        if k % 4 == 0 {
            let pa = http.json(&format!("{old}/api/profile/{i}"))?;
            let pb = http.json(&format!("{new}/api/profile/{}?at={at}", wr.id))?;
            if let (Some(pa), Some(pb)) = (pa, pb) {
                prof_n += 1;
                let key = |c: &Value| -> Option<(i64, i64)> { Some(((c.get(0)?.as_f64()? * 1e6).round() as i64, (c.get(1)?.as_f64()? * 1e6).round() as i64)) };
                let elev_of = |p: &Value| -> HashMap<(i64, i64), f64> {
                    let cs = p["coords"].as_array().cloned().unwrap_or_default();
                    let es = p["elev"].as_array().cloned().unwrap_or_default();
                    cs.iter().zip(es.iter()).filter_map(|(c, e)| Some((key(c)?, e.as_f64()?))).collect()
                };
                let (ea, eb) = (elev_of(&pa), elev_of(&pb));
                for (k2, va) in &ea {
                    if let Some(vb) = eb.get(k2) {
                        prof_pts += 1;
                        let d = (va - vb).abs();
                        prof_max = prof_max.max(d);
                        if d <= 0.5 {
                            prof_close += 1;
                        }
                    }
                }
                if let (Some(la), Some(lb)) = (pa["length_m"].as_f64(), pb["length_m"].as_f64()) {
                    if la > 0.0 {
                        len_ratio.push(lb / la);
                    }
                }
            }
        }
    }
    println!("way info: {way_ok} equal, {way_bad} differ, {way_missing} not found (of {})", sample.len());
    if !field_diffs.is_empty() {
        let mut v: Vec<_> = field_diffs.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        println!("  fields that differ: {}", v.iter().map(|(k, c)| format!("{k} ×{c}")).collect::<Vec<_>>().join(", "));
    }
    fails += way_bad + way_missing;
    len_ratio.sort_by(f64::total_cmp);
    let q = |p: f64| len_ratio.get(((len_ratio.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f64::NAN);
    println!(
        "profiles: {prof_n} compared; {prof_pts} shared vertices, {:.2} % within 0.5 m (max {prof_max:.1} m); whole-road length new/legacy: p10 {:.2}, median {:.2}, p90 {:.2}",
        100.0 * prof_close as f64 / prof_pts.max(1) as f64,
        q(0.1),
        q(0.5),
        q(0.9)
    );
    if prof_pts > 0 && (prof_close as f64) < 0.98 * prof_pts as f64 {
        fails += 1;
    }

    // ---- tiles --------------------------------------------------------------------------------
    let mut tile_stats: HashMap<&str, (usize, usize, usize, usize)> = HashMap::new(); // (equal, differ, only legacy, only new)
    let lon2x = |lon: f64, z: u8| ((lon + 180.0) / 360.0 * (1u64 << z) as f64).floor() as u32;
    let lat2y = |lat: f64, z: u8| {
        let r = lat.to_radians();
        ((1.0 - (r.dtan() + 1.0 / r.dcos()).dln() / std::f64::consts::PI) / 2.0 * (1u64 << z) as f64).floor() as u32
    };
    let mut tiles: Vec<(u8, u32, u32)> = Vec::new();
    for z in 4u8..=14 {
        let (x0, x1, y0, y1) = (lon2x(bb[0], z), lon2x(bb[2], z), lat2y(bb[3], z), lat2y(bb[1], z));
        let all: Vec<(u8, u32, u32)> = (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (z, x, y))).collect();
        let per = (ntiles / 11).max(1);
        let st = (all.len() / per).max(1);
        tiles.extend(all.into_iter().step_by(st).take(per));
    }
    for &(z, x, y) in &tiles {
        for (layer, path) in [("terrain", format!("terrain/{z}/{x}/{y}")), ("slope", format!("slope/{z}/{x}/{y}")), ("trees-cover", format!("trees/cover/{z}/{x}/{y}")), ("roads", format!("roads/{z}/{x}/{y}")), ("rails", format!("rails/{z}/{x}/{y}"))] {
            if layer == "slope" && z > 12 {
                continue;
            }
            let (sa, ba) = http.get(&format!("{old}/tiles/{path}"))?;
            let (sb, bb2) = http.get(&format!("{new}/tiles/{path}"))?;
            let ent = tile_stats.entry(layer).or_default();
            let (ha, hb) = (sa == 200 && !ba.is_empty(), sb == 200 && !bb2.is_empty());
            match (ha, hb) {
                (true, true) if matches!(layer, "roads" | "rails") || ba == bb2 => ent.0 += 1,
                (true, true) => ent.1 += 1,
                (true, false) => ent.2 += 1,
                (false, true) => ent.3 += 1,
                (false, false) => {}
            }
        }
    }
    let mut names: Vec<&&str> = tile_stats.keys().collect();
    names.sort();
    for l in names {
        let (eq, d, oa, ob) = tile_stats[*l];
        println!("tiles {l}: {eq} {}, {d} differ, {oa} only legacy, {ob} only new", if matches!(*l, "roads" | "rails") { "in both" } else { "equal" });
        if matches!(*l, "terrain" | "slope" | "trees-cover") {
            fails += d + oa;
        } else {
            fails += oa;
        }
    }

    // ---- layers and details -------------------------------------------------------------------
    for l in ["pois", "heritage", "special", "indigenous", "heritage-areas", "ferries", "stations", "whs-shapes"] {
        let a = http.json(&format!("{old}/api/layer/{l}"))?;
        let b = http.json(&format!("{new}/api/layer/{l}"))?;
        let count = |v: &Option<Value>| v.as_ref().and_then(|v| v["features"].as_array().map(|f| f.len()));
        let (ca, cb) = (count(&a), count(&b));
        println!("layer {l}: {ca:?} vs {cb:?} features");
        if ca != cb {
            fails += 1;
        }
    }
    let (mut det_ok, mut det_bad) = (0, 0);
    for layer in ["poi", "heritage", "harea", "special", "indigenous"] {
        for i in (0..2000u32).step_by(97) {
            let (sa, ba) = http.get(&format!("{old}/api/detail/{layer}/{i}"))?;
            let (sb, bb2) = http.get(&format!("{new}/api/detail/{layer}/{i}"))?;
            // Value for value (a record with a description laid over is re-serialised).
            let same = ba == bb2 || matches!((serde_json::from_slice::<Value>(&ba), serde_json::from_slice::<Value>(&bb2)), (Ok(x), Ok(y)) if x == y);
            if sa != sb || !same {
                det_bad += 1;
                if det_bad <= 3 {
                    eprintln!("  detail {layer}/{i}: {sa} vs {sb}");
                }
            } else {
                det_ok += 1;
            }
        }
    }
    println!("details: {det_ok} equal, {det_bad} differ");
    fails += det_bad;
    println!("{}", if fails == 0 { "GOLDEN: all equal" } else { "GOLDEN: differences found" });
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
