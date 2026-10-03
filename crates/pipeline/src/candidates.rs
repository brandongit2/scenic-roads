//! A unit's landmark candidates (docs/phase5.md "pois"): extract's points of interest from the
//! unit's piece (`extract --candidates`, with the pass's hiking-route ends), clipped to the points
//! the unit owns that are in the coverage, with what dem/poidetails.py added, written sorted by
//! key as `work/pois/<u>` (zstd JSON lines).

use crate::coverage::Coverage;
use crate::legacy::Unit;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::Path;

/// One candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cand {
    /// Its OSM object ("n123", "w5") or hiking-route end ("trail:<relation>:<node>"), then ":" and
    /// its kind (a way can be a point of interest and a covered bridge).
    pub key: String,
    pub kind: String,
    /// E7.
    pub lon: i32,
    pub lat: i32,
    pub name: String,
    /// Its own English name: name:en, else (Japan) name:ja-Latn or name:ja_rm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub en: Option<String>,
    /// Metres, as extract rounds it (nodes only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ele: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub osm: Option<String>,
    /// The `wikidata` tag as tagged (one QID or several).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qid: Option<String>,
    /// The tags its details show.
    pub tags: BTreeMap<String, String>,
    /// A covered bridge's length (dem/poidetails.py).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_m: Option<u32>,
    /// A peak tagged as a viewpoint too (its details say so).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub viewpoint: bool,
}

/// The candidates in extract's `pois.json` (made with `--candidates`) that unit `u` owns and the
/// coverage holds: a point by its position, a way when one of its nodes is inside.
pub fn from_pois(pois_json: &[u8], u: Unit, cov: &Coverage) -> Result<Vec<Cand>> {
    let v: serde_json::Value = serde_json::from_slice(pois_json).context("pois.json")?;
    let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
    let mut out = Vec::new();
    for f in v["features"].as_array().context("pois.json: no features")? {
        let c = &f["geometry"]["coordinates"];
        let e7 = |x: &serde_json::Value| (x.as_f64().unwrap_or(0.0) * 1e7).round() as i32;
        let (lon, lat) = (e7(&c[0]), e7(&c[1]));
        if !crate::unit::owns(tb, [lon, lat]) {
            continue;
        }
        let p = &f["properties"];
        let nodes: Vec<[i32; 2]> = p["nodes"].as_array().map(|a| a.iter().filter_map(|n| Some([n[0].as_i64()? as i32, n[1].as_i64()? as i32])).collect()).unwrap_or_default();
        let inside = if nodes.is_empty() { cov.contains([lon, lat]) } else { nodes.iter().any(|&n| cov.contains(n)) };
        if !inside {
            continue;
        }
        let kind = p["kind"].as_str().unwrap_or("").to_string();
        let osm = p["osm"].as_str().map(str::to_string);
        let id = osm.clone().or_else(|| p["key"].as_str().map(str::to_string)).context("a point without an OSM id or key")?;
        let tags: BTreeMap<String, String> = p["tags"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
        let tag = |k: &str| tags.get(k).filter(|v| !v.is_empty()).cloned();
        out.push(Cand {
            key: format!("{id}:{kind}"),
            en: tag("name:en").or_else(|| tag("name:ja-Latn")).or_else(|| tag("name:ja_rm")),
            ele: p["ele"].as_f64().map(|e| e as f32),
            qid: tag("wikidata"),
            length_m: p["length_m"].as_u64().map(|m| m as u32),
            viewpoint: kind == "peak" && tags.get("tourism").map(String::as_str) == Some("viewpoint"),
            name: p["name"].as_str().unwrap_or("").to_string(),
            kind,
            lon,
            lat,
            osm,
            tags,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

pub fn write(path: &Path, cands: &[Cand]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut w = zstd::Encoder::new(std::fs::File::create(&tmp)?, 9)?.auto_finish();
        for c in cands {
            serde_json::to_writer(&mut w, c)?;
            w.write_all(b"\n")?;
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read(path: &Path) -> Result<Vec<Cand>> {
    let r = std::io::BufReader::new(zstd::Decoder::new(std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?)?);
    r.lines().map(|l| Ok(serde_json::from_str(&l?)?)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_to_the_unit_and_the_coverage() {
        let d = tempfile::tempdir().unwrap();
        // Iceland around Reykjavik: unit 6/28/17 holds -21.9, 64.13.
        let cov = Coverage::from_recipes(&[crate::agent::recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:-21.9,64.13,20".into()] }], None, d.path()).unwrap();
        let u = Unit { z: 6, x: 28, y: 17 };
        let tb = crate::hipack::tile_bounds(u.z, u.x, u.y);
        assert!(crate::unit::owns(tb, [-219_000_000, 641_300_000]), "{tb:?}");
        let pois = br#"{"type":"FeatureCollection","features":[
          {"geometry":{"coordinates":[-21.9,64.13]},"properties":{"kind":"peak","name":"A","ele":512,"osm":"n2","tags":{"name":"A","tourism":"viewpoint","name:ja-Latn":"a","wikidata":"Q5"}}},
          {"geometry":{"coordinates":[-21.0,64.13]},"properties":{"kind":"peak","name":"Far","osm":"n3","tags":{}}},
          {"geometry":{"coordinates":[-21.6,64.13]},"properties":{"kind":"covered_bridge","name":"B","osm":"w4","tags":{},"length_m":61,"nodes":[[-219100000,641300000],[-210000000,641300000]]}},
          {"geometry":{"coordinates":[-21.9,64.14]},"properties":{"kind":"trailhead","name":"T","key":"trail:9:8","tags":{}}}]}"#;
        let c = from_pois(pois, u, &cov).unwrap();
        let keys: Vec<&str> = c.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["n2:peak", "trail:9:8:trailhead", "w4:covered_bridge"]);
        assert_eq!((c[0].en.as_deref(), c[0].qid.as_deref(), c[0].viewpoint, c[0].ele), (Some("a"), Some("Q5"), true, Some(512.0)));
        assert_eq!(c[2].length_m, Some(61));
        let p = d.path().join("c.jsonl.zst");
        write(&p, &c).unwrap();
        assert_eq!(read(&p).unwrap(), c);
    }
}
