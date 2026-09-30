//! Terrarium DEM tiles for MapLibre's raster-dem sources (3D terrain, hillshade, tint).
//! Tiles absent from the archive are synthesised from their nearest ancestor so the terrain
//! surface never has holes; outside the data, a flat sea-level tile is returned.
//!
//! Slope tiles carry terrain slope in percent, Terrarium-encoded in place of elevation, so a
//! MapLibre colour-relief layer can colour the terrain by slope with the same machinery.

use crate::S;
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
};
use roadcore::grid::{encode_terrain_png, tile_with_fallback};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static SYNTH: OnceLock<Mutex<HashMap<(u8, u32, u32), Vec<u8>>>> = OnceLock::new();
static FLAT: OnceLock<Vec<u8>> = OnceLock::new();

fn png(b: Vec<u8>, versioned: bool) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
            (header::CACHE_CONTROL, crate::cache::cache_control(versioned, "public, max-age=86400")),
        ],
        b,
    )
        .into_response()
}

pub async fn terrain_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery) -> Response {
    let v = crate::cache::versioned(q.as_deref());
    let png = |b: Vec<u8>| png(b, v);
    let Some(arc) = s.terrain.as_ref() else {
        return png(FLAT.get_or_init(|| encode_terrain_png(&vec![0.0; 65536], 256, 256).unwrap()).clone());
    };
    if let Some(b) = arc.get(z, x, y) {
        return png(b.to_vec());
    }
    let cache = SYNTH.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(b) = cache.lock().unwrap().get(&(z, x, y)) {
        return png(b.clone());
    }
    let s2 = s.clone();
    let made = tokio::task::spawn_blocking(move || {
        let arc = s2.terrain.as_ref()?;
        let e = tile_with_fallback(arc, z, x, y)?;
        encode_terrain_png(&e, 256, 256).ok()
    })
    .await
    .ok()
    .flatten();
    let b = made.unwrap_or_else(|| FLAT.get_or_init(|| encode_terrain_png(&vec![0.0; 65536], 256, 256).unwrap()).clone());
    let mut c = cache.lock().unwrap();
    if c.len() > 4000 {
        c.clear();
    }
    c.insert((z, x, y), b.clone());
    png(b)
}

static SLOPE: OnceLock<Mutex<HashMap<(u8, u32, u32), Vec<u8>>>> = OnceLock::new();

pub async fn slope_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery) -> Response {
    let v = crate::cache::versioned(q.as_deref());
    let png = |b: Vec<u8>| png(b, v);
    let flat = || FLAT.get_or_init(|| encode_terrain_png(&vec![0.0; 65536], 256, 256).unwrap()).clone();
    if let Some(b) = s.slope.as_ref().and_then(|a| a.get(z, x, y)) {
        return png(b.to_vec());
    }
    if s.terrain.is_none() || z > 14 {
        return png(flat());
    }
    let cache = SLOPE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(b) = cache.lock().unwrap().get(&(z, x, y)) {
        return png(b.clone());
    }
    let s2 = s.clone();
    let made = tokio::task::spawn_blocking(move || {
        let arc = s2.terrain.as_ref()?;
        let e = tile_with_fallback(arc, z, x, y)?;
        let n = 1u32 << z;
        // Edge neighbours so the 3×3 kernel doesn't seam at tile borders.
        let nb = |dx: i32, dy: i32| -> Option<Vec<f32>> {
            let (nx, ny) = ((x as i64 + dx as i64).rem_euclid(n as i64) as u32, y as i64 + dy as i64);
            if ny < 0 || ny >= n as i64 {
                return None;
            }
            tile_with_fallback(arc, z, nx, ny as u32)
        };
        let (west, east, north, south) = (nb(-1, 0), nb(1, 0), nb(0, -1), nb(0, 1));
        let at = |i: i32, j: i32| -> f32 {
            let (ci, cj) = (i.clamp(0, 255), j.clamp(0, 255));
            let pick = |t: &Option<Vec<f32>>, ii: i32, jj: i32| t.as_ref().map(|v| v[(jj * 256 + ii) as usize]);
            let v = if i < 0 {
                pick(&west, 255, cj)
            } else if i > 255 {
                pick(&east, 0, cj)
            } else if j < 0 {
                pick(&north, ci, 255)
            } else if j > 255 {
                pick(&south, ci, 0)
            } else {
                None
            };
            v.unwrap_or(e[(cj * 256 + ci) as usize])
        };
        let mut out = vec![0f32; 256 * 256];
        let world = 40_075_016.686f64;
        for j in 0..256i32 {
            let yy = (y as f64 + (j as f64 + 0.5) / 256.0) / n as f64;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * yy)).sinh().atan();
            let d = (world * lat.cos() / (256.0 * n as f64)) as f32; // metres per pixel
            for i in 0..256i32 {
                let (a, b, c) = (at(i - 1, j - 1), at(i, j - 1), at(i + 1, j - 1));
                let (dd, f) = (at(i - 1, j), at(i + 1, j));
                let (g, h, k) = (at(i - 1, j + 1), at(i, j + 1), at(i + 1, j + 1));
                // Horn's method.
                let dzdx = ((c + 2.0 * f + k) - (a + 2.0 * dd + g)) / (8.0 * d);
                let dzdy = ((g + 2.0 * h + k) - (a + 2.0 * b + c)) / (8.0 * d);
                out[(j * 256 + i) as usize] = ((dzdx * dzdx + dzdy * dzdy).sqrt() * 100.0).min(500.0);
            }
        }
        encode_terrain_png(&out, 256, 256).ok()
    })
    .await
    .ok()
    .flatten();
    let b = made.unwrap_or_else(flat);
    let mut c = cache.lock().unwrap();
    if c.len() > 4000 {
        c.clear();
    }
    c.insert((z, x, y), b.clone());
    png(b)
}
