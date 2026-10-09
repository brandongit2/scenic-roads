//! Japan: the Geospatial Information Authority's elevation tiles (256 × 256 PNGs on Web Mercator
//! tiles: x = R·2¹⁶ + G·2⁸ + B, h = 0.01·x below 2²³, 0.01·(x − 2²⁴) above, 2²³ no data), sampled
//! at pixel centres, a point near a tile's edge clamping to it.

use super::proj::mercator_tile;
use super::sample::{bilinear, within};
use crate::fetch::Fetch;
use anyhow::{bail, Context, Result};
use rayon::prelude::*;

pub const HOST: &str = "https://cyberjapandata.gsi.go.jp";

/// The layers in GSI's own order: (layer, zoom, source code). Lidar (1A, averaged by GSI to z15,
/// then 5A), photogrammetry (5B, 5C), then the 10 m DEM.
pub const LAYERS: [(&str, u32, u8); 5] = [("dem1a_png", 15, 5), ("dem5a_png", 15, 5), ("dem5b_png", 15, 6), ("dem5c_png", 15, 6), ("dem_png", 14, 7)];

/// Concurrent tile requests (S3 behind CloudFront: latency-bound).
pub const WORKERS: usize = 16;

/// A tile's elevations (metres; -1e9 where there's none), and its width and height.
pub fn decode(png: &[u8]) -> Result<(usize, usize, Vec<f32>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info()?;
    let mut buf = vec![0u8; r.output_buffer_size().context("PNG size")?];
    let info = r.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = info.color_type.samples();
    let mut out = Vec::with_capacity(w * h);
    for p in buf[..w * h * ch].chunks_exact(ch) {
        // As PIL's convert("RGB"): grey repeated, alpha dropped.
        let (r, g, b) = if ch >= 3 { (p[0], p[1], p[2]) } else { (p[0], p[0], p[0]) };
        let v = (r as i64) << 16 | (g as i64) << 8 | b as i64;
        out.push(if v == 1 << 23 { -1e9 } else { (if v < 1 << 23 { v } else { v - (1 << 24) }) as f32 * 0.01f32 });
    }
    Ok((w, h, out))
}

/// The tile's URL.
pub fn url(layer: &str, z: u32, x: i64, y: i64) -> String {
    format!("{HOST}/xyz/{layer}/{z}/{x}/{y}.png")
}

/// Layer `layer` at zoom `z` sampled at points `lon`, `lat`: each point's value, NaN where the
/// layer has none. `on` hears how many points have been sampled so far, about once a second (a
/// pass fetches thousands of tiles).
pub fn pass(fetch: &dyn Fetch, layer: &str, z: u32, lon: &[f64], lat: &[f64], pool: Option<&rayon::ThreadPool>, on: &(dyn Fn(usize) + Sync)) -> Result<Vec<f32>> {
    let n = 1i64 << z;
    // (tile key, point, fx, fy)
    let mut pts: Vec<(i64, u32, f64, f64)> = lon
        .iter()
        .zip(lat)
        .enumerate()
        .map(|(i, (&lo, &la))| {
            let (fx, fy) = mercator_tile(lo, la, z);
            (fx.floor() as i64 * n + fy.floor() as i64, i as u32, fx, fy)
        })
        .collect();
    pts.sort_by_key(|p| p.0);
    let mut groups: Vec<&[(i64, u32, f64, f64)]> = Vec::new();
    let mut s = 0;
    for e in 1..=pts.len() {
        if e == pts.len() || pts[e].0 != pts[s].0 {
            groups.push(&pts[s..e]);
            s = e;
        }
    }
    let (points, said) = (std::sync::atomic::AtomicUsize::new(0), std::sync::Mutex::new(std::time::Instant::now()));
    let sampled: Vec<Vec<(u32, f32)>> = within(pool, || {
        groups
            .par_iter()
            .map(|g| -> Result<Vec<(u32, f32)>> {
                let p = points.fetch_add(g.len(), std::sync::atomic::Ordering::Relaxed) + g.len();
                if let Ok(mut t) = said.try_lock() {
                    if t.elapsed() >= std::time::Duration::from_secs(1) {
                        *t = std::time::Instant::now();
                        on(p);
                    }
                }
                let k = g[0].0;
                let (tx, ty) = (k.div_euclid(n), k.rem_euclid(n));
                let u = url(layer, z, tx, ty);
                let Some(png) = fetch.get(&u)? else { return Ok(Vec::new()) };
                let (w, h, a) = decode(&png).with_context(|| format!("GSI {layer}/{z}/{tx}/{ty}"))?;
                if w * h == 0 {
                    bail!("GSI {layer}/{z}/{tx}/{ty}: an empty tile");
                }
                Ok(g.iter().map(|&(_, i, fx, fy)| (i, bilinear(&a, w, h, (fx - tx as f64) * 256.0 - 0.5, (fy - ty as f64) * 256.0 - 0.5))).collect())
            })
            .collect::<Result<_>>()
    })?;
    let mut out = vec![f32::NAN; lon.len()];
    for (i, v) in sampled.into_iter().flatten() {
        out[i as usize] = v;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile PNG of GSI's encoding from metres (NaN: no data).
    pub fn tile(h: &[f32]) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(h.len() * 3);
        for &v in h {
            let x: i64 = if v.is_nan() { 1 << 23 } else { ((v as f64) * 100.0).round() as i64 };
            let x = if x < 0 { x + (1 << 24) } else { x };
            rgb.extend_from_slice(&[(x >> 16) as u8, (x >> 8) as u8, x as u8]);
        }
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 256, 256);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&rgb).unwrap();
        out
    }

    #[test]
    fn tiles_decode_as_gsi_encodes_them() {
        let mut h = vec![12.34f32; 256 * 256];
        h[1] = -5.5;
        h[2] = f32::NAN;
        let (w, hh, a) = decode(&tile(&h)).unwrap();
        assert_eq!((w, hh), (256, 256));
        assert_eq!(a[0], 1234.0f32 * 0.01);
        assert_eq!(a[1], -550.0f32 * 0.01);
        assert_eq!(a[2], -1e9);
    }
}
