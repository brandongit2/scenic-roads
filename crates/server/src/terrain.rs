//! Terrarium DEM tiles for MapLibre's raster-dem sources (3D terrain, hillshade, tint), from the
//! terrain layer's packs: lossless WebP as stored (pipeline::terrain_codec), down to each place's
//! native zoom (its deep packs past z12: pipeline::terrain_deep). Tiles absent from the layer are
//! made from their nearest ancestor, encoded the same way, so the terrain surface never has holes;
//! outside the data, a flat sea-level tile is returned.
//!
//!
//! Shading tiles (the slope layer's, the terrain's shading, and `depth-shade`, the depth's:
//! pipeline::shade_codec; each pixel's steepness and mean direction, which the map's slope tint and
//! hill-shading read) are served as stored, every level down to each place's finest; a tile the
//! layer lacks (past its finest there, or a deep tile that can't be read now) is made from its
//! nearest stored ancestor, scaled up, as the terrain's are; outside the layer, a flat one.

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
static FLAT_SHADE: OnceLock<Vec<u8>> = OnceLock::new();
static SHADE: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();
static DEPTH_SHADE: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();
static FLOOR: OnceLock<Mutex<HashMap<(u8, u32, u32, u64), Vec<u8>>>> = OnceLock::new();

/// A tile's response: a terrain or shading tile's WebP (pipeline::terrain_codec,
/// pipeline::shade_codec), else a PNG (depth).
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

/// The terrain's deep zooms (pipeline::terrain_deep's deep packs, z13 on; a deep z12, north of 67°,
/// is in the hi packs): read from this Mac where a region's terrain detail is downloaded
/// (crate::downloads), else from the NAS; away from it, a tile there that can't be read is made from
/// its ancestor in the hi packs.
fn deep_zoom(z: u8) -> bool {
    z >= pipeline::terrain_deep::FIRST_Z
}

/// Terrain heights (m) of a tile, from its nearest stored ancestor when it isn't stored, or is a
/// deep tile that can't be read now (`deep_zoom`): None outside the data, an error when a tile of
/// z12 or coarser couldn't be read (the NAS away and no mirror of it).
pub fn terrain_f32(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<f32>>> {
    let failed = std::cell::RefCell::new(None);
    let got = tile_with_fallback_by(
        &|z, x, y| match terrain_stored(s, z, x, y) {
            Ok(b) => b,
            Err(_) if deep_zoom(z) => None,
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
            Some(Got::Failed) if deep_zoom(z) => {}
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

/// A shading tile's pixels made from its nearest stored ancestor in `layer` (up to eight levels
/// up: the terrain's reach, roadcore::grid::tile_with_fallback_by), scaled up
/// (roadcore::shade::upsampled), encoded as stored; None with none. A deep tile's ancestor that
/// can't be read is passed over, as the terrain's.
pub fn shade_from_ancestor(s: &crate::AppState, layer: &str, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    for dz in 1..=z.min(8) {
        let (pz, px, py) = (z - dz, x >> dz, y >> dz);
        let b = match s.data.tile(layer, pz, px, py) {
            Ok(Some((b, _))) => b,
            Ok(None) => continue,
            Err(_) if deep_zoom(pz) => continue,
            Err(e) => return Err(e),
        };
        let p = roadcore::shade::decode(b.bytes())?;
        return Ok(Some(pipeline::shade_codec::encode(&roadcore::shade::upsampled(&p, dz, x, y))?));
    }
    Ok(None)
}

/// A shading tile of `layer` (the slope's or the depth's): as stored (a tile not in the shading's
/// encoding read as none: a pack from before it), else made from its nearest stored ancestor, else
/// flat. (A deep tile that can't be read now: from its ancestor too.)
pub async fn shade_tile(
    s: S,
    layer: &'static str,
    z: u8,
    x: u32,
    y: u32,
    q: Option<String>,
    headers: HeaderMap,
) -> Response {
    let v = versioned(q.as_deref());
    let flat =
        || FLAT_SHADE.get_or_init(|| pipeline::shade_codec::encode(&vec![Default::default(); 65536]).unwrap()).clone();
    let cache = if layer == "slope" { &SHADE } else { &DEPTH_SHADE };
    let got = tokio::task::spawn_blocking(move || {
        match stored(&s, layer, z, x, y, &headers) {
            Some(Got::Stored(b, _)) if !b.starts_with(b"RIFF") => {}
            Some(Got::Failed) if deep_zoom(z) => {}
            Some(g) => return g,
            None => {}
        }
        made(cache, gen(&s), z, x, y, || Ok(shade_from_ancestor(&s, layer, z, x, y)?.unwrap_or_else(flat)))
    })
    .await
    .unwrap_or(Got::Failed);
    respond(got, v)
}

pub async fn slope_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    shade_tile(s, "slope", z, x, y, q, headers).await
}

pub async fn depth_shade_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    shade_tile(s, pipeline::depth::SHADE_LAYER, z, x, y, q, headers).await
}

/// A polar cap's tile (`/tiles/polar-terrain/{pole}/{z}/{x}/{y}`, `/tiles/polar-slope/…`: the
/// catalog's `polar-<kind>-<pole>`, pipeline::polar): as stored, else 404 (past the cap, or not
/// built; the map draws the tile's loaded ancestor). Nothing is made here: a renderer of any
/// projection reads the grid's tiles as they are.
#[allow(clippy::too_many_arguments)]
async fn polar_tile(
    s: S,
    kind: &str,
    pole: &str,
    z: u8,
    x: u32,
    y: u32,
    q: Option<String>,
    headers: HeaderMap,
) -> Response {
    let Some(pole) = roadcore::polar::Pole::of_letter(pole) else { return StatusCode::NOT_FOUND.into_response() };
    let v = versioned(q.as_deref());
    let layer = pipeline::polar::layer(kind, pole);
    let got =
        tokio::task::spawn_blocking(move || stored(&s, &layer, z, x, y, &headers)).await.unwrap_or(Some(Got::Failed));
    match got {
        Some(g) => respond(g, v),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn polar_terrain_tile(
    State(s): State<S>,
    Path((pole, z, x, y)): Path<(String, u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    polar_tile(s, "terrain", &pole, z, x, y, q, headers).await
}

pub async fn polar_slope_tile(
    State(s): State<S>,
    Path((pole, z, x, y)): Path<(String, u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    polar_tile(s, "slope", &pole, z, x, y, q, headers).await
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
    fn nas(root: &store::nasfs::NasRoot) -> std::path::PathBuf {
        let (hi, fine) = ("layers/terrain/hi/6-18-22", "layers/terrain/deep/10-296-360");
        let (hc, fc) = (format!("{hi}.0000000000000001.pack"), format!("{fine}.0000000000000002.pack"));
        let hs = pack(&root.unchecked_path().join(&hc), &[(12, 296 * 4, 360 * 4, 50.0)]);
        let fs = pack(&root.unchecked_path().join(&fc), &[(13, 296 * 8, 360 * 8, 100.0)]);
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
        store::catalog::write_copy(&root.unchecked_path().join("catalog"), &cat).unwrap();
        root.unchecked_path().join(fc)
    }

    #[test]
    fn deep_tiles_come_from_the_deep_packs_and_from_their_ancestor_when_those_cant_be_read() {
        let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        nas(&store::nasfs::NasRoot::local_for_test(root.path()));
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(root.path()));
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
        std::fs::remove_file(nas(&store::nasfs::NasRoot::local_for_test(root.path()))).unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(root.path()));
        let at = |z: u8, x: u32, y: u32| terrain_f32(&s, z, x, y).unwrap().map(|e| e[0]);
        assert_eq!(at(13, 296 * 8, 360 * 8), Some(50.0));
        assert_eq!(at(15, 296 * 32, 360 * 32), Some(50.0));
        // (Read as stored, it is an error: the terrain made it from its ancestor instead.)
        assert!(s.data.tile("terrain", 13, 296 * 8, 360 * 8).is_err());
    }

    /// A shading pack of `layer` at `path`, each tile `(z, x, y, steepness %)` facing east.
    fn shade_pack(path: &std::path::Path, layer: &str, tiles: &[(u8, u32, u32, f32)]) -> u64 {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = store::pack::PackWriter::create(path, serde_json::json!({"layer": layer}), false).unwrap();
        for &(z, x, y, sl) in tiles {
            let b = pipeline::shade_codec::encode(&vec![roadcore::shade::Px { s: sl, u: [1.0, 0.0] }; 65536]).unwrap();
            w.add(z, x, y, &b, b.len() as u32).unwrap();
        }
        w.finish().unwrap();
        std::fs::metadata(path).unwrap().len()
    }

    /// Shading tiles as stored, every level; a tile the layer lacks from its nearest stored
    /// ancestor, scaled up (a deep one whose pack can't be read too); outside the layer, flat.
    #[test]
    fn shading_is_served_as_stored_else_from_its_ancestor() {
        let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (hi, deep) = ("layers/slope/hi/6-18-22", "layers/slope/deep/10-296-360");
        let (hc, dc) = (format!("{hi}.0000000000000001.pack"), format!("{deep}.0000000000000002.pack"));
        let hs = shade_pack(&root.path().join(&hc), "slope", &[(12, 296 * 4, 360 * 4, 30.0)]);
        let ds = shade_pack(&root.path().join(&dc), "slope", &[(13, 296 * 8, 360 * 8, 80.0)]);
        let mut cat = store::catalog::Catalog::new(1);
        let mut l = store::catalog::Layer {
            encoding: pipeline::shade_codec::ENCODING.into(),
            maxzoom: 16,
            ..Default::default()
        };
        l.hi.insert("6/18/22".into(), hi.into());
        l.deep.insert("10/296/360".into(), deep.into());
        cat.layers.insert("slope".into(), l);
        for (lg, c, size) in [(hi, &hc, hs), (deep, &dc, ds)] {
            cat.files.insert(lg.into(), store::catalog::FileRef { file: c.clone(), size, ..Default::default() });
        }
        store::catalog::write_copy(&root.path().join("catalog"), &cat).unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(root.path()));
        let steep = |z: u8, x: u32, y: u32| {
            let b = match s.data.tile("slope", z, x, y) {
                Ok(Some((b, _))) => b.bytes().to_vec(),
                _ => shade_from_ancestor(&s, "slope", z, x, y).unwrap().unwrap_or_default(),
            };
            (!b.is_empty()).then(|| roadcore::shade::code(roadcore::shade::decode(&b).unwrap()[1000]))
        };
        let code = |sl: f32| Some(roadcore::shade::code(roadcore::shade::Px { s: sl, u: [1.0, 0.0] }));
        assert_eq!(steep(12, 296 * 4, 360 * 4), code(30.0));
        assert_eq!(steep(13, 296 * 8, 360 * 8), code(80.0));
        assert_eq!(steep(15, 296 * 32 + 1, 360 * 32), code(80.0), "z15 from the deep z13");
        assert_eq!(steep(13, 296 * 8 + 1, 360 * 8), code(30.0), "a z13 the deep pack lacks, from z12");
        assert_eq!(steep(12, 0, 0), None, "outside it: none (served flat)");
        // The deep pack unreadable (the NAS away, no mirror of it): from the hi pack's z12.
        std::fs::remove_file(root.path().join(&dc)).unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(root.path()));
        assert!(s.data.tile("slope", 13, 296 * 8, 360 * 8).is_err());
        assert_eq!(
            shade_from_ancestor(&s, "slope", 14, 296 * 16, 360 * 16)
                .unwrap()
                .map(|b| roadcore::shade::code(roadcore::shade::decode(&b).unwrap()[0])),
            code(30.0)
        );
    }
}
