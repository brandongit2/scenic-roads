//! Terrarium DEM tiles for MapLibre's raster-dem sources (3D terrain, hillshade, tint), from the
//! terrain layer's packs: lossless WebP as stored (codecs::terrain_codec), down to each place's
//! native zoom (its deep packs past z12: terrain::terrain_deep). Tiles absent from the layer are
//! made from their nearest ancestor, encoded the same way, so the terrain surface never has holes;
//! outside the data, a flat sea-level tile is returned. `?floor=1` and `?surface=1`: the depth's
//! floor under the water and the water's surface over it (crate::depth).
//!
//!
//! Shading tiles (the slope layer's, the terrain's shading, and `depth-shade`, the depth's:
//! codecs::shade_codec; each pixel's steepness and mean direction, which the map's slope tint and
//! hill-shading read) are served as stored, every level down to each place's finest; a tile the
//! layer lacks (past its finest there, or a deep tile that can't be read now) is made from its
//! nearest stored ancestor, scaled up, as the terrain's are; outside the layer, a flat one.

use crate::depth::Surface;
use crate::served::{self, not_modified, Lru};
use crate::S;
use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use roadcore::grid::tile_with_fallback_by;
use std::sync::{LazyLock, Mutex, OnceLock};

static FLAT: OnceLock<Vec<u8>> = OnceLock::new();
static FLAT_SHADE: OnceLock<Vec<u8>> = OnceLock::new();

/// What a made tile is (`Made`'s keys): the terrain's from its ancestors, with the floor, as the
/// water's surface, or a shading or depth layer's (its layer name).
pub(crate) const TERRAIN: &str = "terrain";
pub(crate) const FLOOR: &str = "terrain-floor";
pub(crate) const SURFACE: &str = "terrain-surface";
/// The water's surface over a terrain tile made from its ancestor.
pub(crate) const SURFACE_UP: &str = "terrain-surface-up";

/// Tiles made here (from ancestors, neighbours, or flat), by (what, z, x, y, catalogue generation),
/// under a byte budget: the terrain's, the shading's and the depth's.
type Made = Lru<(&'static str, u8, u32, u32, u64), bytes::Bytes>;
pub(crate) static MADE: LazyLock<Mutex<Made>> = LazyLock::new(|| Mutex::new(Lru::new(256 << 20)));

/// A tile's response: a terrain, shading or depth tile's WebP (codecs::terrain_codec,
/// codecs::shade_codec, terrain::depth::fall), else a PNG (a depth pack from before task #671).
fn image(b: impl Into<bytes::Bytes>, versioned: bool, etag: Option<&str>) -> Response {
    let b = b.into();
    let kind = if b.starts_with(b"RIFF") { "image/webp" } else { "image/png" };
    served::respond(b, kind, false, etag, versioned)
}

/// A terrain tile made here (from an ancestor, with the floor under the water, flat) as the stored
/// ones are: lossless WebP, to the centimetre (codecs::terrain_codec).
pub fn encode(mut e: Vec<f32>) -> anyhow::Result<Vec<u8>> {
    codecs::terrain_codec::encode(&mut e, codecs::terrain_codec::NATIVE_M)
}

fn flat() -> Vec<u8> {
    FLAT.get_or_init(|| encode(vec![0.0; 65536]).unwrap()).clone()
}

/// The catalogue generation, so synthesized tiles from an older catalogue aren't served.
fn gen(s: &crate::AppState) -> u64 {
    s.data.generation.load(std::sync::atomic::Ordering::Relaxed)
}

/// A terrain tile's bytes as stored.
pub fn terrain_stored(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    Ok(s.data.tile("terrain", z, x, y)?.map(|(b, _)| b.bytes().to_vec()))
}

/// The terrain's deep zooms (terrain::terrain_deep's deep packs, z13 on; a deep z12, north of 67°,
/// is in the hi packs): read from this Mac where a region's terrain detail is downloaded
/// (crate::downloads), else from the NAS; away from it, or where a read fails, a tile there is made
/// from its ancestor in the hi packs.
fn deep_zoom(z: u8) -> bool {
    z >= ::terrain::terrain_deep::FIRST_Z
}

/// Terrain heights (m) of a tile, from its nearest stored ancestor when it isn't stored, or is a
/// deep tile that can't be read now (`deep_zoom`): None outside the data (away from the NAS,
/// where the downloads don't reach, Data::reaches: never made from a coarse ancestor that does), an
/// error when a tile of z12 or coarser couldn't be read (a read of the NAS that failed).
pub fn terrain_f32(s: &crate::AppState, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<f32>>> {
    if !s.data.reaches(z, x, y) {
        return Ok(None);
    }
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
    Made(bytes::Bytes),
    /// A read failed: 503, so nothing is cached.
    Failed,
}

/// The response to what a tile request came to (None: the layer has none there, 204).
pub(crate) fn respond(got: Option<Got>, v: bool) -> Response {
    match got {
        None => served::none(),
        Some(Got::Stored(b, e)) => image(b, v, Some(&e)),
        Some(Got::Same(e)) => not_modified(&e, v),
        // Made from other tiles (an ancestor, neighbours, possibly in other packs than the one whose
        // version the URL carries): never cached for good (served::NotForGood).
        Some(Got::Made(b)) => {
            let mut r = image(b, false, None);
            r.extensions_mut().insert(served::NotForGood);
            r
        }
        Some(Got::Failed) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// The stored tile of `layer`, or the browser's copy when current; None when the layer lacks it.
/// `tag` ends its ETag (the shading layers': their encoding, so a browser's copy of a tile from
/// before it, as the pack still holds it, isn't answered 304).
pub(crate) fn stored(
    s: &crate::AppState,
    layer: &str,
    z: u8,
    x: u32,
    y: u32,
    headers: &HeaderMap,
    tag: &str,
) -> Option<Got> {
    let h = match s.data.tile_hash(layer, z, x, y) {
        Ok(Some(h)) => h,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("{layer} {z}/{x}/{y}: {e:#}");
            return Some(Got::Failed);
        }
    };
    let etag = format!("\"{h:016x}{tag}\"");
    if served::etag_match(headers, &etag) {
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

/// A tile made here (`what`: `TERRAIN`, `FLOOR` or a layer's name), kept per catalogue generation
/// in `MADE`; None when `make` has none; failures aren't kept.
pub(crate) fn made(
    what: &'static str,
    g: u64,
    z: u8,
    x: u32,
    y: u32,
    make: impl FnOnce() -> anyhow::Result<Option<Vec<u8>>>,
) -> Option<Got> {
    made_keep(what, g, z, x, y, || Ok(make()?.map(|b| (b, true))))
}

/// As `made`, `make` also saying whether what it made may be kept: not what's made from a
/// stand-in (the water's shares when the basemap is unreadable, crate::water::Sea::exact), made
/// again when asked for again.
pub(crate) fn made_keep(
    what: &'static str,
    g: u64,
    z: u8,
    x: u32,
    y: u32,
    make: impl FnOnce() -> anyhow::Result<Option<(Vec<u8>, bool)>>,
) -> Option<Got> {
    let key = (what, z, x, y, g);
    if let Some(b) = MADE.lock().unwrap().get(&key) {
        return Some(Got::Made(b));
    }
    match make() {
        Ok(Some((b, keep))) => {
            let b = bytes::Bytes::from(b);
            if keep {
                MADE.lock().unwrap().put(key, b.clone(), b.len() as u64);
            }
            Some(Got::Made(b))
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("{what} {z}/{x}/{y}: {e:#}");
            Some(Got::Failed)
        }
    }
}

/// A surface tile over the sea (crate::depth::Surface::Sea, StandIn). From a stored terrain tile
/// and the water tiles' own shares it is those two's alone: an ETag of its own (the terrain's
/// hash and the water's version), cached as a stored tile is (for good under current versions,
/// else a 304 while unchanged). Else made from the terrain's ancestor (not stored there, or a deep
/// tile that can't be read now) or from the water's stand-in, as made tiles are.
fn sea_surface(s: &S, z: u8, x: u32, y: u32, headers: &HeaderMap, exact: bool) -> Option<Got> {
    let hash = match s.data.tile_hash("terrain", z, x, y) {
        Ok(h) => h.filter(|_| exact),
        Err(_) if deep_zoom(z) => None,
        Err(e) => {
            eprintln!("terrain {z}/{x}/{y}: {e:#}");
            return Some(Got::Failed);
        }
    };
    let Some(h) = hash else {
        return made_keep(SURFACE_UP, gen(s), z, x, y, || {
            let Some(mut e) = terrain_f32(s, z, x, y)? else { return Ok(Some((flat(), true))) };
            let exact = crate::depth::surface_over(s, z, x, y, &mut e)?;
            Ok(Some((encode(e)?, exact)))
        });
    };
    let etag = format!("\"{h:016x}w{}\"", crate::water::version(s));
    if served::etag_match(headers, &etag) {
        return Some(Got::Same(etag));
    }
    let got = made(SURFACE, gen(s), z, x, y, || {
        let b = terrain_stored(s, z, x, y)?.ok_or_else(|| anyhow::anyhow!("no longer stored"))?;
        let mut e = roadcore::grid::decode_terrain(&b)?;
        if !crate::depth::surface_over(s, z, x, y, &mut e)? {
            anyhow::bail!("the water can't be read now");
        }
        Ok(Some(encode(e)?))
    });
    match got {
        Some(Got::Made(b)) => Some(Got::Stored(b.to_vec(), etag)),
        g => g,
    }
}

/// A terrain tile; `?floor=1`: with the floor under the water (crate::depth::terrain_floor),
/// where the catalogue has depth (else the terrain as it is); `?surface=1`: as the water's surface
/// (crate::depth::surface_over), sea level at sea. A surface tile with no pixel at sea (by
/// exact shares: not an ancestor's stand-in) is the terrain tile itself, served as it is (a stored
/// one with its ETag); one with sea, made from a stored terrain tile, has an ETag of its own
/// (`sea_surface`). Either is cached for good while the URL's water version `w=` is the current
/// one: most tiles of the map's 3D terrain, its tint and its contours, which all read the surface.
pub async fn terrain_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    let v = served::versioned(q.as_deref());
    let has = |k: &str| q.as_deref().is_some_and(|q| q.split('&').any(|kv| kv == k));
    let (floor, surface) = (has("floor=1"), has("surface=1"));
    let s2 = s.clone();
    let got = tokio::task::spawn_blocking(move || {
        if floor && s.data.catalogue().layers.contains_key(::terrain::depth::LAYER) {
            return made_keep(FLOOR, gen(&s), z, x, y, || {
                Ok(Some(crate::depth::terrain_floor(&s, z, x, y)?.unwrap_or_else(|| (flat(), true))))
            });
        }
        if surface {
            match crate::depth::surface_of(&s, z, x, y) {
                Ok(Surface::Terrain) => {}
                Ok(k) => return sea_surface(&s, z, x, y, &headers, k == Surface::Sea),
                Err(e) => {
                    eprintln!("{SURFACE} {z}/{x}/{y}: {e:#}");
                    return Some(Got::Failed);
                }
            }
        }
        match stored(&s, "terrain", z, x, y, &headers, "") {
            // (A deep tile that can't be read now: made from its ancestor, `terrain_f32`.)
            Some(Got::Failed) if deep_zoom(z) => {}
            Some(g) => return Some(g),
            None => {}
        }
        made(TERRAIN, gen(&s), z, x, y, || {
            Ok(Some(terrain_f32(&s, z, x, y)?.and_then(|e| encode(e).ok()).unwrap_or_else(flat)))
        })
    })
    .await
    .unwrap_or(Some(Got::Failed));
    // A surface tile under another water version than the current one is not cached for good: it
    // is the current water's (that one may say sea elsewhere).
    if surface && !floor && !has(&format!("w={}", crate::water::version(&s2))) {
        let mut r = respond(got, false);
        r.extensions_mut().insert(served::NotForGood);
        return r;
    }
    respond(got, v)
}

/// Whether a stored shading tile is in the shading's encoding (codecs::shade_codec: WebP). A tile
/// of a pack from before it (`slope4-png`, until its step is rebuilt) is read as none, as a parent
/// and as itself: its PNG isn't decodable as shading.
fn is_shade(b: &[u8]) -> bool {
    b.starts_with(b"RIFF")
}

/// Says once per pack (and server run) that `layer`'s tile z/x/y is read as none: `why`, a pack
/// from before the shading's encoding or a tile that won't decode. Every tile below it asks again,
/// so per request it would be tens of thousands of lines.
fn none_once(s: &crate::AppState, layer: &str, z: u8, x: u32, y: u32, why: &dyn std::fmt::Display) {
    static SAID: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let pack = s.data.pack_of(layer, z, x, y).unwrap_or_default();
    if SAID.get_or_init(Default::default).lock().unwrap().insert(pack.clone()) {
        eprintln!(
            "{layer} {z}/{x}/{y}: read as none, flat or from an ancestor ({why}); the same in {pack} not said again"
        );
    }
}

/// Why a stored shading tile isn't shading: its first bytes.
fn not_shade(b: &[u8]) -> String {
    let kind = if b.starts_with(b"\x89PNG") { "a PNG" } else { "not WebP" };
    format!("{kind}, not {}: a pack from before it, until its step is rebuilt", codecs::shade_codec::ENCODING)
}

/// A shading tile's pixels made from its nearest stored ancestor in `layer` (up to eight levels
/// up: the terrain's reach, roadcore::grid::tile_with_fallback_by), scaled up
/// (roadcore::shade::upsampled), encoded as stored; None with none, or where the downloads don't
/// reach away from the NAS (as the terrain's). A deep tile's ancestor that can't be read is passed
/// over, as the terrain's.
pub fn shade_from_ancestor(s: &crate::AppState, layer: &str, z: u8, x: u32, y: u32) -> anyhow::Result<Option<Vec<u8>>> {
    if !s.data.reaches(z, x, y) {
        return Ok(None);
    }
    for dz in 1..=z.min(8) {
        let (pz, px, py) = (z - dz, x >> dz, y >> dz);
        let b = match s.data.tile(layer, pz, px, py) {
            Ok(Some((b, _))) if is_shade(b.bytes()) => b,
            // A pack from before the shading's encoding (slope4-png): none.
            Ok(Some((b, _))) => {
                none_once(s, layer, pz, px, py, &not_shade(b.bytes()));
                continue;
            }
            Ok(None) => continue,
            Err(_) if deep_zoom(pz) => continue,
            Err(e) => return Err(e),
        };
        // (One that won't decode: said once, and none, never a 503 for every tile under it.)
        let p = match roadcore::shade::decode(b.bytes()) {
            Ok(p) => p,
            Err(e) => {
                none_once(s, layer, pz, px, py, &format!("undecodable: {e:#}"));
                continue;
            }
        };
        return Ok(Some(codecs::shade_codec::encode(&roadcore::shade::upsampled(&p, dz, x, y))?));
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
    let v = served::versioned(q.as_deref());
    let flat =
        || FLAT_SHADE.get_or_init(|| codecs::shade_codec::encode(&vec![Default::default(); 65536]).unwrap()).clone();
    let got = tokio::task::spawn_blocking(move || {
        match stored(&s, layer, z, x, y, &headers, &format!("-{}", codecs::shade_codec::ENCODING)) {
            Some(Got::Stored(b, _)) if !is_shade(&b) => none_once(&s, layer, z, x, y, &not_shade(&b)),
            Some(Got::Failed) if deep_zoom(z) => {}
            Some(g) => return Some(g),
            None => {}
        }
        made(layer, gen(&s), z, x, y, || Ok(Some(shade_from_ancestor(&s, layer, z, x, y)?.unwrap_or_else(flat))))
    })
    .await
    .unwrap_or(Some(Got::Failed));
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
    shade_tile(s, ::terrain::depth::SHADE_LAYER, z, x, y, q, headers).await
}

/// A polar cap's tile (`/tiles/polar-terrain/{pole}/{z}/{x}/{y}`, `/tiles/polar-slope/…`: the
/// catalogue's `polar-<kind>-<pole>`, terrain::polar): as stored, else none (204: past the cap, or not
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
    let v = served::versioned(q.as_deref());
    let layer = ::terrain::polar::layer(kind, pole);
    let got = tokio::task::spawn_blocking(move || stored(&s, &layer, z, x, y, &headers, ""))
        .await
        .unwrap_or(Some(Got::Failed));
    respond(got, v)
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
    use axum::http::header;

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
    /// (296, 360), and a catalogue naming them; the deep pack's path.
    fn nas(root: &store::nasfs::NasRoot) -> std::path::PathBuf {
        let (hi, fine) = ("layers/terrain/hi/6-18-22", "layers/terrain/deep/10-296-360");
        let (hc, fc) = (format!("{hi}.0000000000000001.pack"), format!("{fine}.0000000000000002.pack"));
        let hs = pack(&root.unchecked_path().join(&hc), &[(12, 296 * 4, 360 * 4, 50.0)]);
        let fs = pack(&root.unchecked_path().join(&fc), &[(13, 296 * 8, 360 * 8, 100.0)]);
        let mut cat = store::catalogue::Catalogue::new(1);
        let mut l = store::catalogue::Layer {
            encoding: codecs::terrain_codec::ENCODING.into(),
            maxzoom: 16,
            ..Default::default()
        };
        l.hi.insert("6/18/22".into(), hi.into());
        l.deep.insert("10/296/360".into(), fine.into());
        cat.layers.insert("terrain".into(), l);
        for (lg, c, size) in [(hi, &hc, hs), (fine, &fc, fs)] {
            cat.files.insert(lg.into(), store::catalogue::FileRef { file: c.clone(), size, ..Default::default() });
        }
        store::testkit::map_root(&root.unchecked_path(), &cat);
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
            let b = codecs::shade_codec::encode(&vec![roadcore::shade::Px { s: sl, u: [1.0, 0.0] }; 65536]).unwrap();
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
        let mut cat = store::catalogue::Catalogue::new(1);
        let mut l = store::catalogue::Layer {
            encoding: codecs::shade_codec::ENCODING.into(),
            maxzoom: 16,
            ..Default::default()
        };
        l.hi.insert("6/18/22".into(), hi.into());
        l.deep.insert("10/296/360".into(), deep.into());
        cat.layers.insert("slope".into(), l);
        for (lg, c, size) in [(hi, &hc, hs), (deep, &dc, ds)] {
            cat.files.insert(lg.into(), store::catalogue::FileRef { file: c.clone(), size, ..Default::default() });
        }
        store::testkit::map_root(&root.path(), &cat);
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

    /// Through the route: a slope tile as stored (shade-webp) is served, 200; a pack from before the
    /// shading's encoding (slope4-png, the live NAS's until its steps are rebuilt) is read as none,
    /// itself and as an ancestor, so its tiles and those below come back flat, never 503 (every
    /// slope tile 503'd, its PNG decoded as WebP).
    #[tokio::test]
    async fn a_slope_tile_is_served_whatever_its_pack_holds() {
        let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut cat = store::catalogue::Catalogue::new(1);
        let mut l = store::catalogue::Layer {
            encoding: codecs::shade_codec::ENCODING.into(),
            maxzoom: 16,
            ..Default::default()
        };
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x01\0\0\0\x01\0\x08\x06\0\0\0".to_vec();
        let webp = codecs::shade_codec::encode(&vec![roadcore::shade::Px { s: 30.0, u: [1.0, 0.0] }; 65536]).unwrap();
        let bad = b"RIFF\x10\0\0\0WEBPVP8L\x04\0\0\0junk".to_vec();
        for (i, (y6, b)) in [(22u32, &webp), (23, &png), (24, &bad)].into_iter().enumerate() {
            let key = format!("6/18/{y6}");
            let logical = format!("layers/slope/hi/{}", key.replace('/', "-"));
            let file = format!("{logical}.000000000000000{i}.pack");
            let path = root.path().join(&file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut w = store::pack::PackWriter::create(&path, serde_json::json!({"layer": "slope"}), false).unwrap();
            w.add(12, 18 << 6, y6 << 6, b, b.len() as u32).unwrap();
            w.finish().unwrap();
            l.hi.insert(key, logical.clone());
            let size = std::fs::metadata(&path).unwrap().len();
            cat.files.insert(logical, store::catalogue::FileRef { file, size, ..Default::default() });
        }
        cat.layers.insert("slope".into(), l);
        store::testkit::map_root(&root.path(), &cat);
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(root.path()));
        let get = |z: u8, x: u32, y: u32| {
            let s = s.clone();
            async move {
                let r = slope_tile(State(s), Path((z, x, y)), RawQuery(None), HeaderMap::new()).await;
                assert_eq!(r.status(), StatusCode::OK, "{z}/{x}/{y}");
                assert_eq!(r.headers()[header::CONTENT_TYPE], "image/webp");
                let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
                roadcore::shade::code(roadcore::shade::decode(&b).unwrap()[1000])
            }
        };
        let code = |sl: f32| roadcore::shade::code(roadcore::shade::Px { s: sl, u: [1.0, 0.0] });
        let flat = roadcore::shade::code(Default::default());
        assert_eq!(get(12, 18 << 6, 22 << 6).await, code(30.0), "as stored");
        assert_eq!(get(13, 18 << 7, 22 << 7).await, code(30.0), "from its stored ancestor");
        assert_eq!(get(12, 18 << 6, 23 << 6).await, flat, "a slope4-png tile: none");
        assert_eq!(get(13, 18 << 7, 23 << 7).await, flat, "below one: none");
        assert_eq!(get(13, 18 << 7, 24 << 7).await, flat, "below an undecodable one: none, not 503");
        // (Its ETag names the encoding: a browser's copy from a slope4-png pack, under the hash the
        // pack still holds, isn't answered 304.)
        let r = slope_tile(State(s.clone()), Path((12, 18 << 6, 22 << 6)), RawQuery(None), HeaderMap::new()).await;
        let etag = r.headers()[header::ETAG].to_str().unwrap().to_string();
        assert!(etag.ends_with("-shade-webp\""), "{etag}");
        let mut h = HeaderMap::new();
        let old = format!("{}\"", etag.trim_end_matches("-shade-webp\""));
        h.insert(header::IF_NONE_MATCH, old.parse().unwrap());
        let r = slope_tile(State(s.clone()), Path((12, 18 << 6, 22 << 6)), RawQuery(None), h).await;
        assert_eq!(r.status(), StatusCode::OK, "an untagged ETag: the tile again");
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        let r = slope_tile(State(s.clone()), Path((12, 18 << 6, 22 << 6)), RawQuery(None), h).await;
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
    }
}
