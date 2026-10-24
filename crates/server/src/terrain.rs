//! Terrarium DEM tiles for MapLibre's raster-dem sources (3D terrain, hillshade, tint), from the
//! terrain layer's packs: lossless WebP as stored (pipeline::terrain_codec), down to each place's
//! native zoom (its deep packs past z12: pipeline::terrain_deep). Tiles absent from the layer are
//! made from their nearest ancestor, encoded the same way, so the terrain surface never has holes;
//! outside the data, a flat sea-level tile is returned.
//!
//! Slope tiles carry terrain slope in percent, Terrarium-encoded in place of elevation, so a
//! MapLibre colour-relief layer can colour the terrain by slope with the same machinery. The slope
//! layer stores z11 and coarser (four quarter slopes a pixel); z12, and any tile the layer lacks,
//! is made here from the terrain (pipeline::slope_pack::horn with the neighbouring tiles), as the
//! slope step makes its finest level: to z12, and where the terrain has deep tiles to their zoom
//! there, from them (so the slope there is the deep data's; none stored).

use crate::tiles::{etag_match, not_modified, versioned};
use crate::S;
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use roadcore::grid::tile_with_fallback_by;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static SYNTH: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();
static FLAT: OnceLock<Vec<u8>> = OnceLock::new();
static FLAT_SLOPE: OnceLock<Vec<u8>> = OnceLock::new();
static SLOPE: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();
static FLOOR: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();

/// A tile's response: a terrain tile's WebP (pipeline::terrain_codec), else a PNG (slope, depth).
fn image(b: Vec<u8>, versioned: bool, etag: Option<String>) -> Response {
    let kind = if b.starts_with(b"RIFF") { "image/webp" } else { "image/png" };
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

/// A terrain tile made here (from an ancestor, with the floor under the water, flat) as the stored
/// ones are: lossless WebP, to the centimetre (pipeline::terrain_codec).
pub fn encode(mut e: Vec<f32>) -> anyhow::Result<Vec<u8>> {
    pipeline::terrain_codec::encode(&mut e, pipeline::terrain_codec::NATIVE_M)
}

fn flat() -> Vec<u8> {
    FLAT.get_or_init(|| encode(vec![0.0; 65536]).unwrap()).clone()
}

/// The catalog generation, so synthesised tiles from an older catalog aren't served.
fn gen(s: &crate::AppState) -> u64 {
    s.data.generation.load(std::sync::atomic::Ordering::Relaxed)
}

/// A terrain tile's bytes as stored.
pub fn terrain_stored(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    Ok(s.data.tile("terrain", z, x, y)?.map(|(b, _)| b.bytes().to_vec()))
}

/// The terrain's deep zooms (pipeline::terrain_deep's deep packs, past the base's finest): read from
/// this Mac where a region's terrain detail is downloaded (crate::downloads), else from the NAS;
/// away from it, a tile there that can't be read is made from its ancestor in the hi packs.
fn deep_zoom(z: u8, y: u32) -> bool {
    z > pipeline::terrain_pack::max_zoom_at(pipeline::terrain_pack::tile_lat(z, y))
}

/// Terrain heights (m) of a tile, from its nearest stored ancestor when it isn't stored, or is a
/// deep tile that can't be read now (`deep_zoom`): None outside the data, an error when a tile of
/// z12 or coarser couldn't be read (the NAS away and no mirror of it).
pub fn terrain_f32(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<f32>>> {
    let failed = std::cell::RefCell::new(None);
    let got = tile_with_fallback_by(
        &|z, x, y| match terrain_stored(s, z, x, y) {
            Ok(b) => b,
            Err(_) if deep_zoom(z, y) => None,
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

pub(crate) fn respond(got: Got, v: bool) -> Response {
    match got {
        Got::Stored(b, e) => image(b, v, Some(e)),
        Got::Same(e) => not_modified(&e, v),
        // Made from other tiles (an ancestor, neighbours, possibly in other packs than the one whose
        // version the URL carries): never cached for good (cache::NotForGood).
        Got::Made(b) => {
            let mut r = image(b, false, None);
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

/// A terrain tile; `?floor=1`: with the floor under the water (crate::depth::terrain_floor),
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
                Ok(crate::depth::terrain_floor(&s, z, x, y)?.unwrap_or_else(flat))
            });
        }
        match stored(&s, "terrain", z, x, y, &headers) {
            // (A deep tile that can't be read now: made from its ancestor, `terrain_f32`.)
            Some(Got::Failed) if deep_zoom(z, y) => {}
            Some(g) => return g,
            None => {}
        }
        made(&SYNTH, gen(&s), z, x, y, || {
            Ok(terrain_f32(&s, z, x, y)?.and_then(|e| encode(e).ok()).unwrap_or_else(flat))
        })
    })
    .await
    .unwrap_or(Got::Failed);
    respond(got, v)
}

/// Slope (percent) of a terrain tile, as the slope step makes z12 (pipeline::slope_pack::horn, with
/// the edge neighbours): four equal quarters a pixel. None outside the data.
pub fn slope_png(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    let failed = std::cell::RefCell::new(None);
    let tile = |x: u32, y: u32| match terrain_f32(s, z, x, y) {
        Ok(e) => e.map(std::sync::Arc::new),
        Err(e) => {
            failed.borrow_mut().get_or_insert(e);
            None
        }
    };
    let out = pipeline::slope_pack::horn(&tile, z, x, y);
    if let Some(e) = failed.into_inner() {
        return Err(e);
    }
    let Some(out) = out else { return Ok(None) };
    let q: Vec<[f32; 4]> = out.iter().map(|&v| [v; 4]).collect();
    Ok(roadcore::slope::encode_slope4(&q, 256, 256).ok())
}

/// The finest slope made at tile (z, x, y): z12 (the stored terrain's finest), or where the terrain
/// has deep tiles (pipeline::terrain_deep), their zoom there (the catalog's `native_zoom` by z10
/// tile, the map's zoom by area); finer, flat.
fn slope_depth(cat: &store::catalog::Catalog, z: u8, x: u32, y: u32) -> u8 {
    let native = (z > 10)
        .then(|| {
            cat.layers.get("terrain")?.native_zoom.get(&format!("10/{}/{}", x >> (z - 10), y >> (z - 10))).copied()
        })
        .flatten();
    native.unwrap_or(0).max(pipeline::slope_pack::MAXZ)
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
        if z > slope_depth(&s.data.catalog(), z, x, y) {
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

    /// A pack of terrain tiles at `path`, each `(z, x, y, metres)`, as the terrain stores them.
    fn pack(path: &std::path::Path, tiles: &[(u8, u32, u32, f32)]) -> u64 {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = store::pack::PackWriter::create(path, serde_json::json!({"layer": "terrain"}), false).unwrap();
        for &(z, x, y, h) in tiles {
            let b = encode(vec![h; 65536]).unwrap();
            w.add(z, x, y, &b, b.len() as u32).unwrap();
        }
        w.finish().unwrap();
        std::fs::metadata(path).unwrap().len()
    }

    /// A NAS with a terrain hi pack (z12 at 50 m) and a deep pack (z13 at 100 m) under z10 tile
    /// (296, 360), and a catalog naming them; the deep pack's path.
    fn nas(root: &std::path::Path) -> std::path::PathBuf {
        let (hi, fine) = ("layers/terrain/hi/6-18-22", "layers/terrain/deep/10-296-360");
        let (hc, fc) = (format!("{hi}.0000000000000001.pack"), format!("{fine}.0000000000000002.pack"));
        let hs = pack(&root.join(&hc), &[(12, 296 * 4, 360 * 4, 50.0)]);
        let fs = pack(&root.join(&fc), &[(13, 296 * 8, 360 * 8, 100.0)]);
        let mut cat = store::catalog::Catalog::new(1);
        let mut l = store::catalog::Layer {
            encoding: pipeline::terrain_codec::ENCODING.into(),
            maxzoom: 16,
            ..Default::default()
        };
        l.hi.insert("6/18/22".into(), hi.into());
        l.deep.insert("10/296/360".into(), fine.into());
        cat.layers.insert("terrain".into(), l);
        for (lg, c, size) in [(hi, &hc, hs), (fine, &fc, fs)] {
            cat.files.insert(lg.into(), store::catalog::FileRef { file: c.clone(), size, ..Default::default() });
        }
        store::catalog::write_copy(&root.join("catalog"), &cat).unwrap();
        root.join(fc)
    }

    #[test]
    fn deep_tiles_come_from_the_deep_packs_and_from_their_ancestor_when_those_cant_be_read() {
        let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        nas(root.path());
        let s = crate::test_state(home.path(), root.path());
        let at = |z: u8, x: u32, y: u32| terrain_f32(&s, z, x, y).unwrap().map(|e| e[0]);
        // z12 from the hi pack, z13 from the deep pack, z14 made from the deep z13.
        assert_eq!(at(12, 296 * 4, 360 * 4), Some(50.0));
        assert_eq!(at(13, 296 * 8, 360 * 8), Some(100.0));
        assert_eq!(at(14, 296 * 16, 360 * 16), Some(100.0));
        // (Served as stored, a WebP tile, so named; one made here the same.)
        let b = terrain_stored(&s, 13, 296 * 8, 360 * 8).unwrap().unwrap();
        assert_eq!(image(b, false, None).headers()[header::CONTENT_TYPE], "image/webp");
        assert_eq!(image(flat(), false, None).headers()[header::CONTENT_TYPE], "image/webp");
        // The deep pack unreadable (the NAS away, no mirror of it): made from the hi pack's z12.
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
    fn slope_is_made_as_deep_as_the_terrain_goes_there() {
        let mut cat = store::catalog::Catalog::new(1);
        assert_eq!(slope_depth(&cat, 13, 296 * 8, 360 * 8), 12);
        let mut l = store::catalog::Layer { maxzoom: 16, ..Default::default() };
        l.native_zoom.insert("10/296/360".into(), 16);
        cat.layers.insert("terrain".into(), l);
        // Under the deep pack's z10 tile, to its zoom; beside it, z12.
        assert_eq!(slope_depth(&cat, 16, 296 * 64 + 63, 360 * 64), 16);
        assert_eq!(slope_depth(&cat, 13, 297 * 8, 360 * 8), 12);
        assert_eq!(slope_depth(&cat, 9, 148, 180), 12);
    }

    #[test]
    fn deep_slope_is_the_deep_terrains_and_meets_the_coarse_slope_without_a_seam() {
        let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        nas(root.path());
        let s = crate::test_state(home.path(), root.path());
        let q =
            |z: u8, x: u32, y: u32| roadcore::slope::decode_slope4(&slope_png(&s, z, x, y).unwrap().unwrap()).unwrap();
        // The fixture's tiles are level: 50 m at z12, 100 m in its one deep z13 tile, so the deep
        // tile's slope is its own (flat inside) and its east edge meets the 50 m overzoomed beside
        // it, which the tile beside sees too.
        let fine = q(13, 296 * 8, 360 * 8);
        assert_eq!(fine[128 * 256 + 128][0], 0.0);
        assert!(fine[128 * 256 + 255][0] > 0.0, "the step to the 50 m beside it");
        let beside = q(13, 296 * 8 + 1, 360 * 8);
        assert!(beside[128 * 256][0] > 0.0, "the same step, seen from beside it");
        assert_eq!(beside[128 * 256 + 128][0], 0.0);
    }
}
