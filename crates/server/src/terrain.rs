//! Terrarium DEM tiles for MapLibre's raster-dem sources (3D terrain, hillshade, tint), from the
//! terrain layer's packs. Tiles absent from the layer are synthesised from their nearest ancestor
//! so the terrain surface never has holes; outside the data, a flat sea-level tile is returned.
//!
//! Slope tiles carry terrain slope in percent, Terrarium-encoded in place of elevation, so a
//! MapLibre colour-relief layer can colour the terrain by slope with the same machinery. The slope
//! layer stores z11 and coarser (four quarter slopes a pixel); z12, and any tile the layer lacks,
//! is made here from the terrain (Horn's method with the neighbouring tiles), as the slope step
//! makes its finest level: to z14, or as deep as the terrain's fine tiles go (from them where
//! they're stored, so the slope there is the fine data's).

use crate::tiles::{etag_match, not_modified, versioned};
use crate::S;
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use roadcore::grid::{encode_terrain_png, tile_with_fallback_by};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static SYNTH: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();
static FLAT: OnceLock<Vec<u8>> = OnceLock::new();
static FLAT_SLOPE: OnceLock<Vec<u8>> = OnceLock::new();
static SLOPE: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();
static FLOOR: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();

/// A tile's response: a PNG, or a JPEG XL codestream (the fine terrain's stored tiles,
/// pipeline::terrain_fine).
fn png(b: Vec<u8>, versioned: bool, etag: Option<String>) -> Response {
    let kind = if b.starts_with(&[0xff, 0x0a]) { "image/jxl" } else { "image/png" };
    let mut r = (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(kind)),
            (header::CACHE_CONTROL, crate::cache::cache_control(versioned, "no-cache")),
        ],
        b,
    )
        .into_response();
    if let Some(e) = etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
        r.headers_mut().insert(header::ETAG, e);
    }
    r
}

fn flat() -> Vec<u8> {
    FLAT.get_or_init(|| encode_terrain_png(&vec![0.0; 65536], 256, 256).unwrap()).clone()
}

/// The catalog generation, so synthesised tiles from an older catalog aren't served.
fn gen(s: &crate::AppState) -> u64 {
    s.data.generation.load(std::sync::atomic::Ordering::Relaxed)
}

/// A terrain tile's bytes as stored (PNG; JPEG XL past the base's zooms).
pub fn terrain_png(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    Ok(s.data.tile("terrain", z, x, y)?.map(|(b, _)| b.bytes().to_vec()))
}

/// The fine terrain's zooms (pipeline::terrain_fine's fine packs, past the base's finest): no Mac
/// mirrors them, so away from the NAS a tile there that can't be read is made from its ancestor in
/// the hi packs.
fn fine_zoom(z: u8, y: u32) -> bool {
    z > pipeline::terrain_pack::max_zoom_at(pipeline::terrain_pack::tile_lat(z, y))
}

/// Terrain heights (m) of a tile, from its nearest stored ancestor when it isn't stored, or is a
/// fine tile that can't be read now (`fine_zoom`): None outside the data, an error when a tile of
/// z12 or coarser couldn't be read (the NAS away and no mirror of it).
pub fn terrain_f32(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<f32>>> {
    let failed = std::cell::RefCell::new(None);
    let got = tile_with_fallback_by(
        &|z, x, y| match terrain_png(s, z, x, y) {
            Ok(b) => b,
            Err(_) if fine_zoom(z, y) => None,
            Err(e) => {
                failed.borrow_mut().get_or_insert(e);
                None
            }
        },
        z,
        x,
        y,
    );
    match failed.into_inner() {
        Some(e) => Err(e),
        None => Ok(got),
    }
}

/// What a tile request comes to.
pub(crate) enum Got {
    /// As stored, with its ETag.
    Stored(Vec<u8>, String),
    /// The browser has it (its ETag).
    Same(String),
    /// Made here (from ancestors or neighbours), or flat.
    Made(Vec<u8>),
    /// A read failed: 503, so nothing is cached.
    Failed,
}

pub(crate) fn respond_png(got: Got, v: bool) -> Response {
    respond(got, v)
}

fn respond(got: Got, v: bool) -> Response {
    match got {
        Got::Stored(b, e) => png(b, v, Some(e)),
        Got::Same(e) => not_modified(&e, v),
        // Made from other tiles (an ancestor, neighbours, possibly in other packs than the one whose
        // version the URL carries): never cached for good (cache::NotForGood).
        Got::Made(b) => {
            let mut r = png(b, false, None);
            r.extensions_mut().insert(crate::cache::NotForGood);
            r
        }
        Got::Failed => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// The stored tile of `layer`, or the browser's copy when current; None when the layer lacks it.
pub(crate) fn stored(s: &crate::AppState, layer: &str, z: u8, x: u32, y: u32, headers: &HeaderMap) -> Option<Got> {
    let h = match s.data.tile_hash(layer, z, x, y) {
        Ok(Some(h)) => h,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("{layer} {z}/{x}/{y}: {e:#}");
            return Some(Got::Failed);
        }
    };
    let etag = format!("\"{h:016x}\"");
    if etag_match(headers, &etag) {
        return Some(Got::Same(etag));
    }
    match s.data.tile(layer, z, x, y) {
        Ok(Some((b, _))) => Some(Got::Stored(b.bytes().to_vec(), etag)),
        Ok(None) => None,
        Err(e) => {
            eprintln!("{layer} {z}/{x}/{y}: {e:#}");
            Some(Got::Failed)
        }
    }
}

/// A tile made here, cached per catalog generation; failures aren't kept.
fn made(
    cache: &'static OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>>,
    g: u64,
    z: u8,
    x: u32,
    y: u32,
    make: impl FnOnce() -> anyhow::Result<Vec<u8>>,
) -> Got {
    let cache = cache.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(b) = cache.lock().unwrap().get(&(z, x, y, g)) {
        return Got::Made(b.clone());
    }
    match make() {
        Ok(b) => {
            let mut c = cache.lock().unwrap();
            if c.len() > 4000 {
                c.clear();
            }
            c.insert((z, x, y, g), b.clone());
            Got::Made(b)
        }
        Err(e) => {
            eprintln!("tile {z}/{x}/{y}: {e:#}");
            Got::Failed
        }
    }
}

/// A terrain tile; `?floor=1`: with the floor under the water (crate::depth::terrain_floor_png),
/// where the catalog has depth (else the terrain as it is).
pub async fn terrain_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    let v = versioned(q.as_deref());
    let floor = q.as_deref().is_some_and(|q| q.split('&').any(|kv| kv == "floor=1"));
    let got = tokio::task::spawn_blocking(move || {
        if floor && s.data.catalog().layers.contains_key(pipeline::depth::LAYER) {
            return made(&FLOOR, gen(&s), z, x, y, || {
                Ok(crate::depth::terrain_floor_png(&s, z, x, y)?.unwrap_or_else(flat))
            });
        }
        match stored(&s, "terrain", z, x, y, &headers) {
            // (A fine tile that can't be read now: made from its ancestor, `terrain_f32`.)
            Some(Got::Failed) if fine_zoom(z, y) => {}
            Some(g) => return g,
            None => {}
        }
        made(&SYNTH, gen(&s), z, x, y, || {
            Ok(terrain_f32(&s, z, x, y)?.and_then(|e| encode_terrain_png(&e, 256, 256).ok()).unwrap_or_else(flat))
        })
    })
    .await
    .unwrap_or(Got::Failed);
    respond(got, v)
}

/// Slope (percent, Horn's method with the edge neighbours) of a terrain tile, as the slope step
/// makes z12: four equal quarters a pixel.
/// None outside the data.
pub fn slope_png(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    let Some(e) = terrain_f32(s, z, x, y)? else { return Ok(None) };
    let n = 1u32 << z;
    let nb = |dx: i32, dy: i32| -> anyhow::Result<Option<Vec<f32>>> {
        let (nx, ny) = ((x as i64 + dx as i64).rem_euclid(n as i64) as u32, y as i64 + dy as i64);
        if ny < 0 || ny >= n as i64 {
            return Ok(None);
        }
        terrain_f32(s, z, nx, ny as u32)
    };
    let (west, east, north, south) = (nb(-1, 0)?, nb(1, 0)?, nb(0, -1)?, nb(0, 1)?);
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
            let dzdx = ((c + 2.0 * f + k) - (a + 2.0 * dd + g)) / (8.0 * d);
            let dzdy = ((g + 2.0 * h + k) - (a + 2.0 * b + c)) / (8.0 * d);
            out[(j * 256 + i) as usize] = ((dzdx * dzdx + dzdy * dzdy).sqrt() * 100.0).min(500.0);
        }
    }
    let q: Vec<[f32; 4]> = out.iter().map(|&v| [v; 4]).collect();
    Ok(roadcore::slope::encode_slope4(&q, 256, 256).ok())
}

/// The finest slope made: z14, or as deep as the terrain is stored anywhere (its fine tiles' zooms,
/// pipeline::terrain_fine); finer, flat.
fn slope_depth(cat: &store::catalog::Catalog) -> u8 {
    cat.layers.get("terrain").map_or(14, |l| l.maxzoom.max(14))
}

pub async fn slope_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    let v = versioned(q.as_deref());
    let flat =
        || FLAT_SLOPE.get_or_init(|| roadcore::slope::encode_slope4(&vec![[0.0; 4]; 65536], 256, 256).unwrap()).clone();
    let got = tokio::task::spawn_blocking(move || {
        if let Some(g) = stored(&s, "slope", z, x, y, &headers) {
            return g;
        }
        if z > slope_depth(&s.data.catalog()) {
            return Got::Made(flat());
        }
        made(&SLOPE, gen(&s), z, x, y, || Ok(slope_png(&s, z, x, y)?.unwrap_or_else(flat)))
    })
    .await
    .unwrap_or(Got::Failed);
    respond(got, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pack of terrain tiles at `path`, each `(z, x, y, metres)`: PNG, or as the fine terrain
    /// stores them (JPEG XL, `fine`).
    fn pack(path: &std::path::Path, tiles: &[(u8, u32, u32, f32)], fine: bool) -> u64 {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = store::pack::PackWriter::create(path, serde_json::json!({"layer": "terrain"}), false).unwrap();
        for &(z, x, y, h) in tiles {
            let e = vec![h; 65536];
            let b = if fine {
                pipeline::terrain_fine::encode(&e).unwrap()
            } else {
                encode_terrain_png(&e, 256, 256).unwrap()
            };
            w.add(z, x, y, &b, b.len() as u32).unwrap();
        }
        w.finish().unwrap();
        std::fs::metadata(path).unwrap().len()
    }

    /// A NAS with a terrain hi pack (z12 at 50 m) and a fine pack (z13 at 100 m) under z10 tile
    /// (296, 360), and a catalog naming them; the fine pack's path.
    fn nas(root: &std::path::Path) -> std::path::PathBuf {
        let (hi, fine) = ("layers/terrain/hi/6-18-22", "layers/terrain/fine/10-296-360");
        let (hc, fc) = (format!("{hi}.0000000000000001.pack"), format!("{fine}.0000000000000002.pack"));
        let hs = pack(&root.join(&hc), &[(12, 296 * 4, 360 * 4, 50.0)], false);
        let fs = pack(&root.join(&fc), &[(13, 296 * 8, 360 * 8, 100.0)], true);
        let mut cat = store::catalog::Catalog::new(1);
        let mut l = store::catalog::Layer { encoding: "terrarium-png".into(), maxzoom: 16, ..Default::default() };
        l.hi.insert("6/18/22".into(), hi.into());
        l.fine.insert("10/296/360".into(), fine.into());
        cat.layers.insert("terrain".into(), l);
        for (lg, c, size) in [(hi, &hc, hs), (fine, &fc, fs)] {
            cat.files.insert(lg.into(), store::catalog::FileRef { file: c.clone(), size, ..Default::default() });
        }
        store::catalog::write_copy(&root.join("catalog"), &cat).unwrap();
        root.join(fc)
    }

    #[test]
    fn fine_tiles_come_from_the_fine_packs_and_from_their_ancestor_when_those_cant_be_read() {
        let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        nas(root.path());
        let s = crate::test_state(home.path(), root.path());
        let at = |z: u8, x: u32, y: u32| terrain_f32(&s, z, x, y).unwrap().map(|e| e[0]);
        // z12 from the hi pack, z13 from the fine pack, z14 made from the fine z13.
        assert_eq!(at(12, 296 * 4, 360 * 4), Some(50.0));
        assert_eq!(at(13, 296 * 8, 360 * 8), Some(100.0));
        assert_eq!(at(14, 296 * 16, 360 * 16), Some(100.0));
        // (Served as stored, a JPEG XL tile, so named.)
        let b = terrain_png(&s, 13, 296 * 8, 360 * 8).unwrap().unwrap();
        assert_eq!(png(b, false, None).headers()[header::CONTENT_TYPE], "image/jxl");
        // The fine pack unreadable (the NAS away, no mirror of it): made from the hi pack's z12.
        let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::remove_file(nas(root.path())).unwrap();
        let s = crate::test_state(home.path(), root.path());
        let at = |z: u8, x: u32, y: u32| terrain_f32(&s, z, x, y).unwrap().map(|e| e[0]);
        assert_eq!(at(13, 296 * 8, 360 * 8), Some(50.0));
        assert_eq!(at(15, 296 * 32, 360 * 32), Some(50.0));
        // (Read as stored, it is an error: the terrain made it from its ancestor instead.)
        assert!(s.data.tile("terrain", 13, 296 * 8, 360 * 8).is_err());
    }

    #[test]
    fn slope_is_made_as_deep_as_the_terrain_goes() {
        let mut cat = store::catalog::Catalog::new(1);
        assert_eq!(slope_depth(&cat), 14);
        cat.layers.insert("terrain".into(), store::catalog::Layer { maxzoom: 12, ..Default::default() });
        assert_eq!(slope_depth(&cat), 14);
        cat.layers.get_mut("terrain").unwrap().maxzoom = 16;
        assert_eq!(slope_depth(&cat), 16);
    }
}
