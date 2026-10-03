//! Tiles of every layer, from the catalog's packs (and the basemap's PMTiles archives).
//!
//! Caching: a tile's ETag is its content hash (plus, for tiles with names, the version of the
//! translations it uses), answered with a 304 from the pack index alone, never the NAS. URLs the
//! app versions (`?v=`, the layer's version from the catalog) are cached for good.

use crate::S;
use axum::{
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use anyhow::Context;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A request for a versioned URL (`v=` in its query).
pub fn versioned(query: Option<&str>) -> bool {
    crate::cache::versioned(query)
}

pub fn etag_match(headers: &HeaderMap, etag: &str) -> bool {
    headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
}

pub fn not_modified(etag: &str, versioned: bool) -> Response {
    let mut r = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(e) = HeaderValue::from_str(etag) {
        r.headers_mut().insert(header::ETAG, e);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, crate::cache::cache_control(versioned, "no-cache"));
    r
}

fn respond(body: Vec<u8>, content_type: &'static str, gzip: bool, etag: &str, versioned: bool) -> Response {
    let mut r = body.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if gzip {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    if let Ok(e) = HeaderValue::from_str(etag) {
        h.insert(header::ETAG, e);
    }
    h.insert(header::CACHE_CONTROL, crate::cache::cache_control(versioned, "no-cache"));
    r
}

/// A plain tile of one of our layers, served as stored.
async fn plain(s: S, layer: String, z: u8, x: u32, y: u32, q: Option<String>, headers: HeaderMap, content_type: &'static str, gzip: bool) -> Response {
    let v = versioned(q.as_deref());
    let s2 = s.clone();
    let l2 = layer.clone();
    let h = match tokio::task::spawn_blocking(move || s2.data.tile_hash(&l2, z, x, y)).await {
        Ok(Ok(Some(h))) => h,
        Ok(Ok(None)) => return StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("{layer} {z}/{x}/{y}: {e:#}");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let etag = format!("\"{h:016x}\"");
    if etag_match(&headers, &etag) {
        return not_modified(&etag, v);
    }
    let s2 = s.clone();
    match tokio::task::spawn_blocking(move || s2.data.tile(&layer, z, x, y)).await {
        Ok(Ok(Some((b, _)))) => respond(b.bytes().to_vec(), content_type, gzip, &etag, v),
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("tile {z}/{x}/{y}: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn road_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    plain(s, "roads".into(), z, x, y, q, headers, "application/octet-stream", true).await
}

pub async fn rail_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    plain(s, "rails".into(), z, x, y, q, headers, "application/octet-stream", true).await
}

pub async fn tree_tile(State(s): State<S>, Path((var, z, x, y)): Path<(String, u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    if !["cover", "height", "leaf"].contains(&var.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    plain(s, format!("trees-{var}"), z, x, y, q, headers, "image/webp", false).await
}

// ---- tiles with names --------------------------------------------------------------------------

/// Rewritten tiles (names attached), by (layer, z, x, y, content hash, names version).
type Rewritten = Mutex<HashMap<(u8, u8, u32, u32, u64, u64), Arc<Vec<u8>>>>;
static REWRITTEN: OnceLock<Rewritten> = OnceLock::new();

fn remember(key: (u8, u8, u32, u32, u64, u64), v: Arc<Vec<u8>>) {
    let c = REWRITTEN.get_or_init(|| Mutex::new(HashMap::new()));
    let mut c = c.lock().unwrap();
    if c.len() > 6000 {
        c.clear();
    }
    c.insert(key, v);
}

fn recall(key: &(u8, u8, u32, u32, u64, u64)) -> Option<Arc<Vec<u8>>> {
    REWRITTEN.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap().get(key).cloned()
}

/// Labels by importance (dem/labels.py): gzip'd MVT, one layer "l", name n, OSM's English en.
pub async fn label_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    named_mvt_tile(s, "labels".into(), crate::names_live::Rules::Labels, z, x, y, q, headers).await
}

/// Ferries by view (pipeline::ovconv): a block of gzip'd GeoJSON (the ways touching the tile, the
/// terminals near it, its lines' records), names attached.
pub async fn ferry_block(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let v = versioned(q.as_deref());
    let nv = s.names.version_all();
    let s2 = s.clone();
    let got = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(u64, Vec<u8>)>> {
        let Some(h) = s2.data.tile_hash("ferries", z, x, y)? else { return Ok(None) };
        let key = (2u8, z, x, y, h, nv);
        if let Some(b) = recall(&key) {
            return Ok(Some((h, b.to_vec())));
        }
        let Some((b, _)) = s2.data.tile("ferries", z, x, y)? else { return Ok(None) };
        let raw = names::mvt::gunzip_if_gzip(b.bytes())?;
        let named = crate::cache::with_names(&s2, &raw);
        let gz = names::mvt::gzip(&named)?;
        remember(key, Arc::new(gz.clone()));
        Ok(Some((h, gz)))
    })
    .await;
    match got {
        Ok(Ok(Some((h, body)))) => {
            let etag = format!("\"{h:016x}-{nv:x}\"");
            if etag_match(&headers, &etag) {
                return not_modified(&etag, v);
            }
            respond(body, "application/geo+json", true, &etag, v)
        }
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("ferries {z}/{x}/{y}: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Rail stops by view (pipeline::ovconv): gzip'd MVT, layer "s", name n.
pub async fn station_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    named_mvt_tile(s, "stations".into(), crate::names_live::Rules::Stations, z, x, y, q, headers).await
}

/// A layer's gzip'd MVT tile with display names attached (`rules`: which layer and properties).
#[allow(clippy::too_many_arguments)]
pub async fn named_mvt_tile(s: S, layer: String, rules: crate::names_live::Rules, z: u8, x: u32, y: u32, q: Option<String>, headers: HeaderMap) -> Response {
    let v = versioned(q.as_deref());
    let s2 = s.clone();
    let l2 = layer.clone();
    let h = match tokio::task::spawn_blocking(move || s2.data.tile_hash(&l2, z, x, y)).await {
        Ok(Ok(Some(h))) => h,
        Ok(Ok(None)) => return StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("{layer} {z}/{x}/{y}: {e:#}");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let nv = s.names.version_for_tile(z, x, y, 0.0);
    let etag = format!("\"{h:016x}-{nv:x}\"");
    if etag_match(&headers, &etag) {
        return not_modified(&etag, v);
    }
    // (Tiles of different layers have different contents, so the content hash keys them apart.)
    let key = (0u8, z, x, y, h, nv);
    if let Some(b) = recall(&key) {
        return respond(b.to_vec(), "application/x-protobuf", true, &etag, v);
    }
    let s2 = s.clone();
    let l2 = layer.clone();
    let made = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<u8>>> {
        let Some((b, _)) = s2.data.tile(&l2, z, x, y)? else { return Ok(None) };
        Ok(Some(s2.names.attach_gz(b.bytes(), z, x, y, rules)))
    })
    .await;
    match made {
        Ok(Ok(Some(b))) => {
            let b = Arc::new(b);
            remember(key, b.clone());
            respond(b.to_vec(), "application/x-protobuf", true, &etag, v)
        }
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("{layer} {z}/{x}/{y}: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The basemap: the tile from every basemap archive that has it (today's basemap and its parts),
/// merged, with names attached.
pub async fn base_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let v = versioned(q.as_deref());
    let nv = s.names.version_for_tile(z, x, y, 1.0);
    let s2 = s.clone();
    let got = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(u64, Vec<u8>)>> {
        let tiles = s2.basemap.tiles(&s2.data, z, x, y)?;
        if tiles.is_empty() {
            return Ok(None);
        }
        // The tiles' identity: their archives' content names (immutable) and position.
        let mut hh = blake3::Hasher::new();
        for (c, _) in &tiles {
            hh.update(c.as_bytes());
        }
        let h = u64::from_le_bytes(hh.finalize().as_bytes()[..8].try_into().unwrap());
        let key = (1u8, z, x, y, h, nv);
        if let Some(b) = recall(&key) {
            return Ok(Some((h, b.to_vec())));
        }
        let raw: Vec<Vec<u8>> = tiles.iter().filter_map(|(_, b)| names::mvt::gunzip_if_gzip(b).ok().map(|c| c.into_owned())).collect();
        let merged = match raw.len() {
            0 => return Ok(None),
            1 => raw.into_iter().next().unwrap(),
            _ => {
                let refs: Vec<&[u8]> = raw.iter().map(Vec::as_slice).collect();
                names::mvt::merge(&refs)?
            }
        };
        let out = s2.names.attach_raw(&merged, z, x, y, crate::names_live::Rules::Basemap);
        let gz = names::mvt::gzip(&out)?;
        remember(key, Arc::new(gz.clone()));
        Ok(Some((h, gz)))
    })
    .await;
    match got {
        Ok(Ok(Some((h, body)))) => {
            let etag = format!("\"{h:016x}-{nv:x}\"");
            if etag_match(&headers, &etag) {
                return not_modified(&etag, v);
            }
            respond(body, "application/x-protobuf", true, &etag, v)
        }
        Ok(Ok(None)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            eprintln!("basemap {z}/{x}/{y}: {e:#}");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// The basemap archives (PMTiles) of the current catalog, opened.
#[derive(Default)]
pub struct Basemap {
    /// The archives opened, for (their content names, the mirror's generation then).
    open: Mutex<Option<((Vec<String>, u64), Vec<(String, Arc<store::pmtiles::PmTiles>)>)>>,
}

impl Basemap {
    /// Every archive opened, or the first failure (kept only when all open).
    fn archives(&self, data: &crate::data::Data) -> anyhow::Result<Vec<(String, Arc<store::pmtiles::PmTiles>)>> {
        let srcs = data.basemaps()?;
        // Opened again once the mirror has copied files (an archive read from the NAS until then).
        let names: (Vec<String>, u64) = (srcs.iter().map(|(c, _)| c.clone()).collect(), data.mirror_gen.load(std::sync::atomic::Ordering::Relaxed));
        if let Some((n, a)) = self.open.lock().unwrap().as_ref() {
            if *n == names {
                return Ok(a.clone());
            }
        }
        let mut out = Vec::new();
        for (c, src) in srcs {
            let pm = match src {
                crate::views::Src::Local(m) => store::pmtiles::PmTiles::open(Box::new(crate::views::MapRange(m))),
                crate::views::Src::Remote(r) => store::pmtiles::PmTiles::open(Box::new(r)),
            };
            out.push((c.clone(), Arc::new(pm.with_context(|| format!("basemap {c}"))?)));
        }
        *self.open.lock().unwrap() = Some((names, out.clone()));
        Ok(out)
    }

    /// The tile from each archive that has it: (archive content name, bytes as stored).
    pub fn tiles(&self, data: &crate::data::Data, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let mut out = Vec::new();
        for (c, pm) in self.archives(data)? {
            if let Some(b) = pm.get(z, x, y)? {
                out.push((c, b));
            }
        }
        Ok(out)
    }
}
