//! "What can I see from here?" — on-demand viewshed over the z11 analysis grid (terrain from the
//! terrain layer's z11 tiles, tree canopy from the grid layer; earth curvature with refraction).

use crate::S;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use roadcore::grid::{cell_m, cell_of, class as lc, WORLD};
use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Q {
    lng: f64,
    lat: f64,
    /// Radius in km (default 15, max 25).
    r: Option<f64>,
    /// Eye height above ground in metres (default 1.5).
    eye: Option<f64>,
}

#[derive(Serialize)]
pub struct Out {
    /// [[lng, lat] × 4]: top-left, top-right, bottom-right, bottom-left
    corners: [[f64; 2]; 4],
    image: String,
    ground_m: f32,
    visible_km2: f64,
    water_km2: f64,
    farthest_km: f64,
    /// Share of the circle's area that is visible.
    visible_share: f64,
}

fn lnglat(gx: f64, gy: f64) -> [f64; 2] {
    let lon = gx / WORLD * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * gy / WORLD)).sinh().atan().to_degrees();
    [lon, lat]
}

pub async fn viewshed(State(s): State<S>, Query(q): Query<Q>) -> Response {
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || compute(&s2, q)).await {
        Ok(Some(o)) => Json(o).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The z11 analysis grid read from tiles: terrain from the terrain layer (its z11 tiles, or their
/// nearest ancestor), canopy height and land cover from the grid layers. Tiles are decoded once per
/// request.
struct Grid<'a> {
    s: &'a crate::AppState,
    terr: HashMap<(i64, i64), Option<Arc<Vec<f32>>>>,
    can: HashMap<(i64, i64), Option<Arc<Vec<u8>>>>,
    cls: HashMap<(i64, i64), Option<Arc<Vec<u8>>>>,
}

impl<'a> Grid<'a> {
    fn new(s: &'a crate::AppState) -> Self {
        Grid { s, terr: HashMap::new(), can: HashMap::new(), cls: HashMap::new() }
    }

    fn terr_tile(&mut self, tx: i64, ty: i64) -> Option<Arc<Vec<f32>>> {
        let s = self.s;
        self.terr
            .entry((tx, ty))
            .or_insert_with(|| {
                if tx < 0 || ty < 0 || tx >= 2048 || ty >= 2048 {
                    return None;
                }
                crate::terrain::terrain_f32(s, 11, tx as u32, ty as u32).ok().flatten().map(Arc::new)
            })
            .clone()
    }

    fn u8_tile(s: &crate::AppState, map: &mut HashMap<(i64, i64), Option<Arc<Vec<u8>>>>, layer: &str, tx: i64, ty: i64) -> Option<Arc<Vec<u8>>> {
        map.entry((tx, ty))
            .or_insert_with(|| {
                if tx < 0 || ty < 0 || tx >= 2048 || ty >= 2048 {
                    return None;
                }
                let (b, _) = s.data.tile(layer, 11, tx as u32, ty as u32).ok()??;
                zstd::decode_all(b.bytes()).ok().filter(|v| v.len() == 65536).map(Arc::new)
            })
            .clone()
    }

    /// Terrain at a cell.
    fn terrain(&mut self, gx: i64, gy: i64) -> Option<f32> {
        let t = self.terr_tile(gx >> 8, gy >> 8)?;
        Some(t[((gy & 255) * 256 + (gx & 255)) as usize])
    }

    fn canopy(&mut self, gx: i64, gy: i64) -> u8 {
        let s = self.s;
        Self::u8_tile(s, &mut self.can, "grid-canopy", gx >> 8, gy >> 8).map_or(0, |t| t[((gy & 255) * 256 + (gx & 255)) as usize])
    }

    fn class(&mut self, gx: i64, gy: i64) -> Option<u8> {
        let s = self.s;
        Self::u8_tile(s, &mut self.cls, "grid-class", gx >> 8, gy >> 8).map(|t| t[((gy & 255) * 256 + (gx & 255)) as usize])
    }

    /// Bilinear terrain at fractional cell coordinates (cell centres at +0.5).
    fn bilinear(&mut self, gx: f64, gy: f64) -> Option<f32> {
        let (x, y) = (gx - 0.5, gy - 0.5);
        let (x0, y0) = (x.floor() as i64, y.floor() as i64);
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let a = self.terrain(x0, y0)?;
        let b = self.terrain(x0 + 1, y0).unwrap_or(a);
        let c = self.terrain(x0, y0 + 1).unwrap_or(a);
        let d = self.terrain(x0 + 1, y0 + 1).unwrap_or(a);
        Some(a * (1.0 - fx) * (1.0 - fy) + b * fx * (1.0 - fy) + c * (1.0 - fx) * fy + d * fx * fy)
    }
}

fn compute(s: &crate::AppState, q: Q) -> Option<Out> {
    let mut g = Grid::new(s);
    let radius = q.r.unwrap_or(15.0).clamp(1.0, 25.0) * 1000.0;
    let (gx, gy) = cell_of(q.lng, q.lat);
    let cm = cell_m(q.lat);
    let ground = g.bilinear(gx, gy)?;
    let eye = ground as f64 + q.eye.unwrap_or(1.5);
    let rc = (radius / cm).ceil() as i64;
    let n = (2 * rc + 1) as usize;
    let (ox, oy) = (gx.floor() as i64 - rc, gy.floor() as i64 - rc);
    // 0 = outside / not reached, 1 = hidden, 2 = visible land, 3 = visible water
    let mut vis = vec![0u8; n * n];
    let rays = 2048;
    let r_eff = 6_371_000.0 / 0.87;
    for a in 0..rays {
        let th = a as f64 * std::f64::consts::TAU / rays as f64;
        let (ux, uy) = (th.sin(), -th.cos());
        let mut smax = f64::MIN;
        let mut d = cm * 0.5;
        while d <= radius {
            let (cx, cy) = ((gx + ux * d / cm).floor() as i64, (gy + uy * d / cm).floor() as i64);
            let Some(tc) = g.terrain(cx, cy) else { break };
            // Near the observer use interpolated terrain (a coarse cell can sit above eye level
            // on a summit), and no canopy: the observer is standing at a clearing or lookout.
            let t = if d < 3.0 * cm { g.bilinear(gx + ux * d / cm, gy + uy * d / cm).map_or(tc as f64, |v| v as f64) } else { tc as f64 };
            let h = t + if d < 60.0 { 0.0 } else { g.canopy(cx, cy) as f64 };
            let drop = d * d / (2.0 * r_eff);
            let sl = (h - eye - drop) / d;
            let k = ((cy - oy) as usize) * n + (cx - ox) as usize;
            if sl >= smax {
                let c = g.class(cx, cy);
                let water = c.is_some_and(|c| c == lc::WATER) || (c.is_none_or(|c| c == lc::NONE) && t <= 1.0);
                vis[k] = vis[k].max(if water { 3 } else { 2 });
                smax = sl;
            } else if vis[k] == 0 {
                vis[k] = 1;
            }
            d += cm * 0.7;
        }
    }
    let (mut area, mut water, mut far, mut total) = (0f64, 0f64, 0f64, 0f64);
    let cell_area = cm * cm;
    let mut rgba = vec![0u8; n * n * 4];
    for j in 0..n {
        for i in 0..n {
            let k = j * n + i;
            let (dx, dy) = ((i as f64 + 0.5 - (gx - ox as f64)) * cm, (j as f64 + 0.5 - (gy - oy as f64)) * cm);
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > radius {
                vis[k] = 0;
                continue;
            }
            if vis[k] > 0 {
                total += cell_area;
            }
            let px = &mut rgba[k * 4..k * 4 + 4];
            match vis[k] {
                1 => px.copy_from_slice(&[6, 8, 12, 120]),
                2 => {
                    area += cell_area;
                    far = far.max(dist);
                    px.copy_from_slice(&[255, 196, 92, 105]);
                }
                3 => {
                    area += cell_area;
                    water += cell_area;
                    far = far.max(dist);
                    px.copy_from_slice(&[96, 214, 255, 150]);
                }
                _ => {}
            }
        }
    }
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, n as u32, n as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&rgba).ok()?;
    }
    let (x0, y0, x1, y1) = (ox as f64, oy as f64, (ox + n as i64) as f64, (oy + n as i64) as f64);
    Some(Out {
        corners: [lnglat(x0, y0), lnglat(x1, y0), lnglat(x1, y1), lnglat(x0, y1)],
        image: format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&buf)),
        ground_m: ground,
        visible_km2: area / 1e6,
        water_km2: water / 1e6,
        farthest_km: far / 1000.0,
        visible_share: if total > 0.0 { area / total } else { 0.0 },
    })
}
