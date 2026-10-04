//! Tiles of every layer, from the catalog's packs (and the basemap's PMTiles archives).
//!
//! Caching: a tile's ETag is its content hash (the basemap's: its archives' content names and its
//! position), plus, for tiles with names, the version of the translations it uses. A 304 is answered
//! from the pack index (the basemap's from the catalog) alone, never the NAS. URLs the app versions
//! (`?v=`, the layer's version from the catalog) are cached for good.

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

/// A basemap tile's identity, known from the catalog without reading the tile: its archives'
/// content names (immutable, so the same names hold the same tiles) and its position. With the
/// version of the translations it uses, that's everything the served tile is made from.
fn base_hash(archives: &[String], z: u8, x: u32, y: u32) -> u64 {
    let mut h = blake3::Hasher::new();
    for c in archives {
        h.update(c.as_bytes());
        h.update(b"\n");
    }
    h.update(&[z]);
    h.update(&x.to_le_bytes());
    h.update(&y.to_le_bytes());
    u64::from_le_bytes(h.finalize().as_bytes()[..8].try_into().unwrap())
}

/// The basemap: the tile from every basemap archive that has it (today's basemap and its parts),
/// merged, with names attached. Its ETag comes from the catalog, so the browser's copy is confirmed
/// (304) without reading the archives, which are on the NAS until the mirror has them.
pub async fn base_tile(State(s): State<S>, Path((z, x, y)): Path<(u8, u32, u32)>, RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let v = versioned(q.as_deref());
    let archives = s.data.basemap_names();
    if archives.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let nv = s.names.version_for_tile(z, x, y, 1.0);
    let h = base_hash(&archives, z, x, y);
    let etag = format!("\"{h:016x}-{nv:x}\"");
    if etag_match(&headers, &etag) {
        return not_modified(&etag, v);
    }
    let key = (1u8, z, x, y, h, nv);
    if let Some(b) = recall(&key) {
        return respond(b.to_vec(), "application/x-protobuf", true, &etag, v);
    }
    let s2 = s.clone();
    let got = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Vec<u8>>> {
        // From the archives the ETag names, even if a new catalog came meanwhile.
        let tiles = s2.basemap.tiles(&s2.data, &archives, z, x, y)?;
        let raw: Vec<Vec<u8>> = tiles.iter().filter_map(|b| names::mvt::gunzip_if_gzip(b).ok().map(|c| c.into_owned())).collect();
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
        Ok(Some(gz))
    })
    .await;
    match got {
        Ok(Ok(Some(body))) => respond(body, "application/x-protobuf", true, &etag, v),
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
    open: Mutex<Option<((Vec<String>, u64), Vec<Arc<store::pmtiles::PmTiles>>)>>,
}

impl Basemap {
    /// The archives `contents` (content names) opened, or the first failure (kept only when all
    /// open).
    fn archives(&self, data: &crate::data::Data, contents: &[String]) -> anyhow::Result<Vec<Arc<store::pmtiles::PmTiles>>> {
        // Opened again once the mirror has copied files (an archive read from the NAS until then).
        let key: (Vec<String>, u64) = (contents.to_vec(), data.mirror_gen.load(std::sync::atomic::Ordering::Relaxed));
        if let Some((k, a)) = self.open.lock().unwrap().as_ref() {
            if *k == key {
                return Ok(a.clone());
            }
        }
        let mut out = Vec::new();
        for (c, src) in data.basemaps(contents)? {
            let pm = match src {
                crate::views::Src::Local(m) => store::pmtiles::PmTiles::open(Box::new(crate::views::MapRange(m))),
                crate::views::Src::Remote(r) => store::pmtiles::PmTiles::open(Box::new(r)),
            };
            out.push(Arc::new(pm.with_context(|| format!("basemap {c}"))?));
        }
        *self.open.lock().unwrap() = Some((key, out.clone()));
        Ok(out)
    }

    /// The tile from each of the archives `contents` that has it, as stored.
    pub fn tiles(&self, data: &crate::data::Data, contents: &[String], z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for pm in self.archives(data, contents)? {
            if let Some(b) = pm.get(z, x, y)? {
                out.push(b);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PMTiles archive holding one tile, 0/0/0: the 127-byte header, a root directory of one
    /// entry (uncompressed), the tile.
    fn archive(tile: &[u8]) -> Vec<u8> {
        // One entry: tile id 0, a run of 1, its length, offset 0 (stored plus one).
        let dir = [1, 0, 1, tile.len() as u8, 1];
        let (root_at, data_at) = (127u64, 127 + dir.len() as u64);
        let mut b = b"PMTiles\x03".to_vec();
        // The root, metadata, leaves and data (offset, length); tiles addressed, entries, contents.
        for v in [root_at, dir.len() as u64, data_at, 0, data_at, 0, data_at, tile.len() as u64, 1, 1, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        // Clustered, nothing compressed, MVT, z0–0; then bounds and centre, unused.
        b.extend_from_slice(&[1, 1, 1, 1, 0, 0]);
        b.extend_from_slice(&[0; 25]);
        assert_eq!(b.len(), 127);
        b.extend_from_slice(&dir);
        b.extend_from_slice(tile);
        b
    }

    /// A NAS folder whose catalog's basemap is the one archive `name` (on the NAS when given), and
    /// its content name.
    fn nas_with_basemap(name: &str, archive: Option<&[u8]>) -> (tempfile::TempDir, String) {
        let nas = tempfile::tempdir().unwrap();
        let logical = format!("layers/basemap/{name}");
        let content = format!("{logical}.0123456789abcdef.pmtiles");
        let mut cat = store::catalog::Catalog::new(1);
        cat.basemap = vec![logical.clone()];
        cat.files.insert(logical, store::catalog::FileRef { file: content.clone(), size: archive.map_or(0, |a| a.len() as u64), ..Default::default() });
        store::catalog::write_copy(&nas.path().join("catalog"), &cat).unwrap();
        if let Some(a) = archive {
            let p = nas.path().join(&content);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, a).unwrap();
        }
        (nas, content)
    }

    async fn get(s: &S, (z, x, y): (u8, u32, u32), etag: Option<&str>) -> Response {
        let mut h = HeaderMap::new();
        if let Some(e) = etag {
            h.insert(header::IF_NONE_MATCH, HeaderValue::from_str(e).unwrap());
        }
        base_tile(State(s.clone()), Path((z, x, y)), RawQuery(None), h).await
    }

    fn etag_of(r: &Response) -> Option<String> {
        r.headers().get(header::ETAG).map(|e| e.to_str().unwrap().to_string())
    }

    #[test]
    fn basemap_etag_changes_with_what_the_tile_is_made_from() {
        let a = vec!["layers/basemap/world.0123456789abcdef.pmtiles".to_string()];
        let ab = vec![a[0].clone(), "layers/basemap/part.0123456789abcdef.pmtiles".to_string()];
        let h = base_hash(&a, 5, 10, 12);
        assert_eq!(h, base_hash(&a, 5, 10, 12));
        for other in [base_hash(&ab, 5, 10, 12), base_hash(&[], 5, 10, 12), base_hash(&a, 6, 10, 12), base_hash(&a, 5, 11, 12), base_hash(&a, 5, 10, 13), base_hash(&a, 5, 12, 10)] {
            assert_ne!(h, other);
        }
    }

    #[tokio::test]
    async fn basemap_304_reads_nothing() {
        // The catalog names an archive the NAS doesn't have, so any read of it fails.
        let (nas, content) = nas_with_basemap("world-gone", None);
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state(home.path(), nas.path());
        let etag = format!("\"{:016x}-{:x}\"", base_hash(&[content], 3, 4, 2), s.names.version_for_tile(3, 4, 2, 1.0));
        let r = get(&s, (3, 4, 2), Some(&etag)).await;
        assert_eq!((r.status(), etag_of(&r)), (StatusCode::NOT_MODIFIED, Some(etag.clone())));
        // Without the browser's copy, or with another tile's, it has to read, and can't.
        assert_eq!(get(&s, (3, 4, 2), None).await.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(get(&s, (3, 4, 3), Some(&etag)).await.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn basemap_200_carries_the_etag_its_304_answers() {
        // An MVT tile of one empty layer, "t".
        let tile = [0x1a, 0x05, 0x0a, 0x01, b't', 0x78, 0x02];
        let (nas, content) = nas_with_basemap("world-here", Some(&archive(&tile)));
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state(home.path(), nas.path());
        let r = get(&s, (0, 0, 0), None).await;
        assert_eq!(r.status(), StatusCode::OK);
        let etag = etag_of(&r).unwrap();
        assert_eq!(etag, format!("\"{:016x}-{:x}\"", base_hash(&[content], 0, 0, 0), s.names.version_for_tile(0, 0, 0, 1.0)));
        let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        assert_eq!(names::mvt::gunzip_if_gzip(&body).unwrap().as_ref(), &tile[..]);
        assert_eq!(get(&s, (0, 0, 0), Some(&etag)).await.status(), StatusCode::NOT_MODIFIED);
        // Again (from memory now): the same ETag.
        let r = get(&s, (0, 0, 0), None).await;
        assert_eq!((r.status(), etag_of(&r)), (StatusCode::OK, Some(etag)));
        // A tile no archive has: 204, without an ETag.
        let r = get(&s, (1, 0, 0), None).await;
        assert_eq!((r.status(), etag_of(&r)), (StatusCode::NO_CONTENT, None));
    }
}
