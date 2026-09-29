//! Details of the stops & sights, heritage sites and highlighted areas, for hover and popups.
//!
//! The map layers stay light (names, kinds, a few fields); everything else is looked up here:
//!   GET /api/detail/{layer}/{i}   layer: poi, heritage, harea, special, indigenous; i: the
//!                                 feature's index (its `i` property)
//!   GET /api/park?name=&lon=&lat= the protected area of that name around the point (parks are
//!                                 drawn from the basemap's park layer, which has no ids)
//! Loaded at startup from details-{layer}.jsonl (dem/poidetails.py, heritagedetails.py,
//! areadetails.py), with peaks.json (the `peaks` step) merged into the POI details.

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path as FsPath;

use crate::S;

const LAYERS: [&str; 5] = ["poi", "heritage", "harea", "special", "indigenous"];

pub struct Details {
    by: HashMap<(u8, u32), Box<str>>,
    parks: Vec<Park>,
    park_names: HashMap<String, Vec<u32>>,
}

struct Park {
    bbox: [f64; 4],
    area: f64,
    json: Box<str>,
}

fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

impl Details {
    pub fn load(dir: &FsPath) -> Self {
        let mut by: HashMap<(u8, u32), Box<str>> = HashMap::new();
        // Peak prominence and isolation, merged into the POI records.
        let mut peaks: HashMap<u32, serde_json::Value> = HashMap::new();
        if let Ok(b) = std::fs::read(dir.join("peaks.json")) {
            if let Ok(serde_json::Value::Array(a)) = serde_json::from_slice::<serde_json::Value>(&b) {
                for mut p in a {
                    if let Some(i) = p.get("i").and_then(|v| v.as_u64()) {
                        p.as_object_mut().map(|o| o.remove("i"));
                        peaks.insert(i as u32, p);
                    }
                }
            }
        }
        for (li, layer) in LAYERS.iter().enumerate() {
            let Ok(text) = std::fs::read_to_string(dir.join(format!("details-{layer}.jsonl"))) else { continue };
            for line in text.lines() {
                let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                let Some(i) = v.get("i").and_then(|x| x.as_u64()) else { continue };
                if li == 0 {
                    if let Some(p) = peaks.remove(&(i as u32)) {
                        v["peak"] = p;
                    }
                }
                by.insert((li as u8, i as u32), v.to_string().into_boxed_str());
            }
        }
        // Peaks without other details.
        for (i, p) in peaks {
            by.entry((0, i)).or_insert_with(|| serde_json::json!({ "i": i, "peak": p }).to_string().into_boxed_str());
        }
        // Heritage sites' own properties that the map's layer leaves out (dem/layers.py: dates,
        // authority, source, links), as "props".
        if let Some(hi) = LAYERS.iter().position(|l| *l == "heritage") {
            if let Ok(text) = std::fs::read_to_string(dir.join("props-heritage.jsonl")) {
                for line in text.lines() {
                    let Ok(mut p) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                    let Some(i) = p.get("i").and_then(|x| x.as_u64()) else { continue };
                    p.as_object_mut().map(|o| o.remove("i"));
                    let key = (hi as u8, i as u32);
                    let mut v = by.get(&key).and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok()).unwrap_or_else(|| serde_json::json!({ "i": i }));
                    v["props"] = p;
                    by.insert(key, v.to_string().into_boxed_str());
                }
            }
        }
        let mut parks = Vec::new();
        let mut park_names: HashMap<String, Vec<u32>> = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(dir.join("details-park.jsonl")) {
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                let (Some(name), Some(b)) = (v["name"].as_str(), v["bbox"].as_array()) else { continue };
                let bb: Vec<f64> = b.iter().filter_map(|x| x.as_f64()).collect();
                if bb.len() != 4 {
                    continue;
                }
                park_names.entry(norm(name)).or_default().push(parks.len() as u32);
                parks.push(Park { bbox: [bb[0], bb[1], bb[2], bb[3]], area: v["area_km2"].as_f64().unwrap_or(0.0), json: line.into() });
            }
        }
        eprintln!("details: {} records, {} parks", by.len(), parks.len());
        Self { by, parks, park_names }
    }
}

fn json(body: &str) -> Response {
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=3600")], body.to_string()).into_response()
}

pub async fn detail(State(s): State<S>, Path((layer, i)): Path<(String, u32)>) -> Response {
    let Some(li) = LAYERS.iter().position(|l| *l == layer) else { return StatusCode::NOT_FOUND.into_response() };
    match s.details.by.get(&(li as u8, i)) {
        Some(b) => json(b),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

#[derive(Deserialize)]
pub struct ParkQ {
    name: String,
    lon: f64,
    lat: f64,
}

/// The park of that name whose bounding box holds the point (the smallest, for nested ones), else
/// the nearest of that name within ~5 km.
pub async fn park(State(s): State<S>, Query(q): Query<ParkQ>) -> Response {
    let d = &s.details;
    let Some(ids) = d.park_names.get(&norm(&q.name)) else { return StatusCode::NO_CONTENT.into_response() };
    let pad = 0.002;
    let inside = ids
        .iter()
        .map(|&k| &d.parks[k as usize])
        .filter(|p| q.lon >= p.bbox[0] - pad && q.lon <= p.bbox[2] + pad && q.lat >= p.bbox[1] - pad && q.lat <= p.bbox[3] + pad)
        .min_by(|a, b| a.area.total_cmp(&b.area));
    let pick = inside.or_else(|| {
        ids.iter()
            .map(|&k| &d.parks[k as usize])
            .map(|p| {
                let cx = q.lon.clamp(p.bbox[0], p.bbox[2]);
                let cy = q.lat.clamp(p.bbox[1], p.bbox[3]);
                (roadcore::dist_m(q.lon, q.lat, cx, cy), p)
            })
            .filter(|(m, _)| *m < 5000.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    });
    match pick {
        Some(p) => json(&p.json),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}
