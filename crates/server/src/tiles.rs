//! Tiles of every layer, from the catalogue's packs (and the basemap's PMTiles archives), answered
//! through crate::reply's responder.
//!
//! Caching: a tile's ETag is its content hash (the basemap's: its archives' content names and its
//! position), plus, for tiles with names (`has_names`), the version of the translations it uses. A
//! 304 is answered from the pack index (the basemap's from the catalogue) alone, never the NAS. URLs
//! the app versions (`?v=`, the layer's version from the catalogue) are cached for good.

use crate::reply::{self, blocking, none, not_modified, respond, serve_made, Lru};
use crate::S;
use anyhow::Context;
use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::{Arc, LazyLock, Mutex};

/// Whether a layer's tiles are served with display names attached (so their ETags and versions
/// carry the translations' version).
pub fn has_names(layer: &str) -> bool {
    matches!(layer, "labels" | "stations" | "ferries") || layer.starts_with("ov-")
}

/// A layer's stored tile's content hash, read from its pack's index alone: None where the layer
/// has none.
async fn hash(s: &S, layer: &str, z: u8, x: u32, y: u32) -> Result<Option<u64>, Response> {
    let (s2, l2) = (s.clone(), layer.to_string());
    blocking(format!("{layer} {z}/{x}/{y}"), move || s2.data.tile_hash(&l2, z, x, y)).await
}

/// A plain tile of one of our layers, served as stored (gzip'd as its pack says).
async fn plain(
    s: S,
    layer: String,
    z: u8,
    x: u32,
    y: u32,
    q: Option<String>,
    headers: HeaderMap,
    content_type: &'static str,
) -> Response {
    let v = reply::versioned(q.as_deref());
    let h = match hash(&s, &layer, z, x, y).await {
        Ok(Some(h)) => h,
        Ok(None) => return none(),
        Err(r) => return r,
    };
    let etag = format!("\"{h:016x}\"");
    if reply::etag_match(&headers, &etag) {
        return not_modified(&etag, v);
    }
    match blocking(format!("{layer} {z}/{x}/{y}"), move || s.data.tile(&layer, z, x, y)).await {
        Ok(Some((b, gzip))) => respond(b.bytes().to_vec(), content_type, gzip, Some(&etag), v),
        Ok(None) => none(),
        Err(r) => r,
    }
}

pub async fn road_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    plain(s, "roads".into(), z, x, y, q, headers, "application/octet-stream").await
}

pub async fn rail_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    plain(s, "rails".into(), z, x, y, q, headers, "application/octet-stream").await
}

/// The 3D buildings' field (bld::bld::field): `field5-webp` tiles, levels 4–14, as stored.
pub async fn building_field_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    plain(s, "buildings-field".into(), z, x, y, q, headers, "application/octet-stream").await
}

/// The 3D buildings (bld::bld): gzip'd MVT, layer `b`, served as stored (no names).
pub async fn building_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    plain(s, "buildings".into(), z, x, y, q, headers, "application/x-protobuf").await
}

/// The region layer (codecs::regionlayer): gzip'd MVT, layer `borders`, served as stored (no names).
pub async fn region_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    plain(s, codecs::regionlayer::LAYER.into(), z, x, y, q, headers, "application/x-protobuf").await
}

pub async fn tree_tile(
    State(s): State<S>,
    Path((var, z, x, y)): Path<(String, u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !["cover", "height", "leaf"].contains(&var.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    plain(s, format!("trees-{var}"), z, x, y, q, headers, "image/webp").await
}

// ---- tiles with names --------------------------------------------------------------------------

/// Tiles made here (names attached, gzip'd), by (kind: 0 a named MVT layer's, 1 the basemap's, 2 a
/// ferry block; z, x, y, content hash, names version), under a byte budget.
type Key = (u8, u8, u32, u32, u64, u64);
static MADE: LazyLock<Mutex<Lru<Key, bytes::Bytes>>> = LazyLock::new(|| Mutex::new(Lru::new(192 << 20)));

/// Labels by importance (landmarks::labels): gzip'd MVT, one layer "l", name n, OSM's English en.
pub async fn label_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    named_mvt_tile(s, "labels".into(), crate::names_live::Rules::Labels, z, x, y, q, headers).await
}

/// Ferries by view (landmarks::ovconv): a block of gzip'd GeoJSON (the ways touching the tile, the
/// terminals near it, its lines' records), names attached.
pub async fn ferry_block(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    let v = reply::versioned(q.as_deref());
    let nv = s.names.version_all();
    let h = match hash(&s, "ferries", z, x, y).await {
        Ok(Some(h)) => h,
        Ok(None) => return none(),
        Err(r) => return r,
    };
    let etag = format!("\"{h:016x}-{nv:x}\"");
    let what = format!("ferries {z}/{x}/{y}");
    serve_made(&MADE, (2, z, x, y, h, nv), &etag, &headers, v, "application/geo+json", what, move || {
        let Some((b, _)) = s.data.tile("ferries", z, x, y)? else { return Ok(None) };
        let raw = names::mvt::gunzip_if_gzip(b.bytes())?;
        Ok(Some(names::mvt::gzip(&reply::with_names(&s, &raw))?))
    })
    .await
}

/// Rail stops by view (landmarks::ovconv): gzip'd MVT, layer "s", name n.
pub async fn station_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    named_mvt_tile(s, "stations".into(), crate::names_live::Rules::Stations, z, x, y, q, headers).await
}

/// A layer's gzip'd MVT tile with display names attached (`rules`: which layer and properties).
#[allow(clippy::too_many_arguments)]
pub async fn named_mvt_tile(
    s: S,
    layer: String,
    rules: crate::names_live::Rules,
    z: u8,
    x: u32,
    y: u32,
    q: Option<String>,
    headers: HeaderMap,
) -> Response {
    let v = reply::versioned(q.as_deref());
    let h = match hash(&s, &layer, z, x, y).await {
        Ok(Some(h)) => h,
        Ok(None) => return none(),
        Err(r) => return r,
    };
    let nv = s.names.version_for_tile(z, x, y, 0.0);
    let etag = format!("\"{h:016x}-{nv:x}\"");
    // (Tiles of different layers have different contents, so the content hash keys them apart.)
    let what = format!("{layer} {z}/{x}/{y}");
    serve_made(&MADE, (0, z, x, y, h, nv), &etag, &headers, v, "application/x-protobuf", what, move || {
        let Some((b, _)) = s.data.tile(&layer, z, x, y)? else { return Ok(None) };
        Ok(Some(s.names.attach_gz(b.bytes(), z, x, y, rules)))
    })
    .await
}

/// A basemap tile's identity, known from the catalogue without reading the tile: its archives'
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
/// merged, with names attached. Its ETag comes from the catalogue, so the browser's copy is confirmed
/// (304) without reading the archives, which are on the NAS until the mirror has them.
pub async fn base_tile(
    State(s): State<S>,
    Path((z, x, y)): Path<(u8, u32, u32)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    let v = reply::versioned(q.as_deref());
    let archives = s.data.basemap_names();
    if archives.is_empty() {
        return none();
    }
    // (Away from the NAS, a tile no archive of which is read here is none, before the ETag and
    // the tiles made from the NAS's are looked at: crate::data.)
    if !s.data.online() && archives.iter().all(|a| s.data.basemap_key(a, z, x, y).is_none()) {
        return none();
    }
    let nv = s.names.version_for_tile(z, x, y, 1.0);
    // (Away from the NAS, a tile only partly where the downloads reach is cut to them: told apart by
    // where they reach.)
    let cut = s.data.cut(None, z, x, y);
    let h = match &cut {
        Some(r) => xxhash_rust::xxh3::xxh3_64_with_seed(r.token().as_bytes(), base_hash(&archives, z, x, y)),
        None => base_hash(&archives, z, x, y),
    };
    let etag = format!("\"{h:016x}-{nv:x}\"");
    let what = format!("basemap {z}/{x}/{y}");
    serve_made(&MADE, (1, z, x, y, h, nv), &etag, &headers, v, "application/x-protobuf", what, move || {
        // From the archives the ETag names, even if a new catalogue came meanwhile.
        let tiles = s.basemap.tiles(&s.data, &archives, z, x, y)?;
        let raw: Vec<Vec<u8>> =
            tiles.iter().filter_map(|b| names::mvt::gunzip_if_gzip(b).ok().map(|c| c.into_owned())).collect();
        let merged = match raw.len() {
            0 => return Ok(None),
            1 => raw.into_iter().next().unwrap(),
            _ => {
                let refs: Vec<&[u8]> = raw.iter().map(Vec::as_slice).collect();
                names::mvt::merge(&refs)?
            }
        };
        let merged = match &cut {
            Some(r) => crate::clip::basemap(&merged, z, x, y, r.grid(None))?,
            None => merged,
        };
        let out = s.names.attach_raw(&merged, z, x, y, crate::names_live::Rules::Basemap);
        Ok(Some(names::mvt::gzip(&out)?))
    })
    .await
}

/// The basemap's archives (PMTiles) and the pieces of them this Mac has downloaded
/// (store::pieces), opened as they're read.
pub struct Basemap {
    /// The mirror's generation they were opened for, and the opened ones by their key
    /// (`Data::basemap_key`: a piece's path in the mirror, or an archive's content name).
    open: Mutex<(u64, Lru<String, Arc<store::pmtiles::PmTiles>>)>,
}

/// Opened pieces and archives kept, at most.
const OPEN: usize = 256;

impl Default for Basemap {
    fn default() -> Self {
        Basemap { open: Mutex::new((0, Lru::entries(OPEN))) }
    }
}

impl Basemap {
    fn open(&self, data: &crate::data::Data, key: &str) -> anyhow::Result<Arc<store::pmtiles::PmTiles>> {
        // (Opened again once the mirror has copied or deleted something: a piece may be here now.)
        let gen = data.mirror_gen.load(std::sync::atomic::Ordering::Relaxed);
        {
            let mut g = self.open.lock().unwrap();
            if g.0 != gen {
                *g = (gen, Lru::entries(OPEN));
            }
            if let Some(a) = g.1.get(key) {
                return Ok(a.clone());
            }
        }
        let pm = match data.src(key)? {
            crate::views::Src::Local(m) => store::pmtiles::PmTiles::open(Box::new(m)),
            crate::views::Src::Remote(r) => store::pmtiles::PmTiles::open(Box::new(r)),
        };
        let pm = Arc::new(pm.with_context(|| format!("basemap {key}"))?);
        let mut g = self.open.lock().unwrap();
        g.1.put(key.to_string(), pm.clone(), 0);
        Ok(pm)
    }

    /// What tile z/x/y of `archive` (a content name) is read from: the piece of it this Mac has, else
    /// the archive (from the NAS while it's read: `Data::basemap_key`). Its key (a piece's path in
    /// the mirror, else the archive's name) and the archive or piece opened; None away from the NAS
    /// where nothing of it is here.
    pub fn source(
        &self,
        data: &crate::data::Data,
        archive: &str,
        z: u8,
        x: u32,
        y: u32,
    ) -> anyhow::Result<Option<(String, Arc<store::pmtiles::PmTiles>)>> {
        let Some(key) = data.basemap_key(archive, z, x, y) else { return Ok(None) };
        match self.open(data, &key) {
            Ok(pm) => Ok(Some((key, pm))),
            // (A piece removed just now: the archive, while it's read.)
            Err(_) if key != archive && data.online() => Ok(Some((archive.to_string(), self.open(data, archive)?))),
            Err(_) if key != archive => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Lets go of what's opened of `names` (what the mirror has just deleted): its map would hold
    /// the disk's room.
    pub fn forget(&self, names: &[String]) {
        let mut g = self.open.lock().unwrap();
        g.1.retain(|k, _| !names.contains(k));
    }

    /// The tile from each of the archives `contents` (content names) that has it, as stored.
    pub fn tiles(
        &self,
        data: &crate::data::Data,
        contents: &[String],
        z: u8,
        x: u32,
        y: u32,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for a in contents {
            let Some((_, pm)) = self.source(data, a, z, x, y)? else { continue };
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
    use axum::http::{header, HeaderValue};

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

    /// A NAS folder whose catalogue's basemap is the one archive `name` (on the NAS when given), and
    /// its content name.
    fn nas_with_basemap(name: &str, archive: Option<&[u8]>) -> (tempfile::TempDir, String) {
        let nas = tempfile::tempdir().unwrap();
        let logical = format!("layers/basemap/{name}");
        let content = format!("{logical}.0123456789abcdef.pmtiles");
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.basemap = vec![logical.clone()];
        cat.files.insert(
            logical,
            store::catalogue::FileRef {
                file: content.clone(),
                size: archive.map_or(0, |a| a.len() as u64),
                ..Default::default()
            },
        );
        store::testkit::map_root(&nas.path(), &cat);
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
        for other in [
            base_hash(&ab, 5, 10, 12),
            base_hash(&[], 5, 10, 12),
            base_hash(&a, 6, 10, 12),
            base_hash(&a, 5, 11, 12),
            base_hash(&a, 5, 10, 13),
            base_hash(&a, 5, 12, 10),
        ] {
            assert_ne!(h, other);
        }
    }

    #[tokio::test]
    async fn basemap_304_reads_nothing() {
        // The catalogue names an archive the NAS doesn't have, so any read of it fails.
        let (nas, content) = nas_with_basemap("world-gone", None);
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
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
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        let r = get(&s, (0, 0, 0), None).await;
        assert_eq!(r.status(), StatusCode::OK);
        let etag = etag_of(&r).unwrap();
        assert_eq!(
            etag,
            format!("\"{:016x}-{:x}\"", base_hash(&[content], 0, 0, 0), s.names.version_for_tile(0, 0, 0, 1.0))
        );
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

    #[tokio::test]
    async fn a_downloaded_piece_serves_its_tiles_without_the_nas() {
        // Tiles at z12 under the z6 tiles 31/21 and 32/21 (gzipped: as Planetiler stores them).
        let t = |b: u8| names::mvt::gzip(&[0x1a, 0x05, 0x0a, 0x01, b, 0x78, 0x02]).unwrap();
        let archive = store::pieces::archive(
            &[(12, 31 << 6, 21 << 6, t(b'a')), (12, 32 << 6, 21 << 6, t(b'b'))],
            store::pmtiles::Compression::Gzip,
        );
        let logical = "layers/basemap/world-piece";
        let nas = tempfile::tempdir().unwrap();
        let content = store::naming::write_atomic(
            &store::nasfs::NasRoot::local_for_test(nas.path()),
            logical,
            "pmtiles",
            store::naming::WriteFrom::Bytes(&archive),
        )
        .unwrap();
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.basemap = vec![logical.into()];
        cat.files.insert(
            logical.into(),
            store::catalogue::FileRef { file: content.clone(), size: archive.len() as u64, ..Default::default() },
        );
        store::testkit::map_root(&nas.path(), &cat);
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state_with(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()), true);
        // 31/21's piece downloaded; then the NAS's archive gone.
        let m = s.data.mirror.clone().unwrap();
        let w = store::mirror::Wanted {
            items: vec![store::mirror::Item::Piece {
                archive: content.clone(),
                piece: store::pieces::Piece::Tile(31, 21),
            }],
        };
        let st = m
            .sync(
                &cat,
                &w,
                &store::nasfs::NasRoot::local_for_test(nas.path()),
                &s.data.pool().unwrap(),
                &store::mirror::Control::FREE,
            )
            .unwrap();
        assert_eq!(st.copied, 1);
        s.data.forget_remote();
        std::fs::remove_file(nas.path().join(&content)).unwrap();
        let r = get(&s, (12, 31 << 6, 21 << 6), None).await;
        assert_eq!(r.status(), StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            names::mvt::gunzip_if_gzip(&body).unwrap().as_ref(),
            &[0x1a, 0x05, 0x0a, 0x01, b'a', 0x78, 0x02][..]
        );
        // The other z6 tile's: not here, and the NAS hasn't it.
        assert_eq!(get(&s, (12, 32 << 6, 21 << 6), None).await.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// A NAS whose catalogue has a roads lo pack (gzip'd blobs) and a buildings-field one (not),
    /// each holding tile 3/0/0, and the rail frequencies' file.
    fn nas_with_packs(root: &std::path::Path) {
        let mut cat = store::catalogue::Catalogue::new(1);
        for (layer, gzip, n) in [("roads", true, 1), ("buildings-field", false, 2)] {
            let logical = format!("layers/{layer}/lo/3-0-0");
            let content = format!("{logical}.{n:016x}.pack");
            let p = root.join(&content);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            let mut w = store::pack::PackWriter::create(&p, serde_json::json!({"layer": layer}), gzip).unwrap();
            let b = if gzip { names::mvt::gzip(layer.as_bytes()).unwrap() } else { layer.as_bytes().to_vec() };
            w.add(3, 0, 0, &b, layer.len() as u32).unwrap();
            w.finish().unwrap();
            let size = std::fs::metadata(&p).unwrap().len();
            let mut l = store::catalogue::Layer { maxzoom: 8, ..Default::default() };
            l.lo.insert("3/0/0".into(), logical.clone());
            cat.layers.insert(layer.into(), l);
            cat.files.insert(logical, store::catalogue::FileRef { file: content, size, ..Default::default() });
        }
        let content = "global/railfreq.00000000000000aa.bin";
        std::fs::create_dir_all(root.join("global")).unwrap();
        std::fs::write(root.join(content), b"freq").unwrap();
        cat.files.insert(
            "global/railfreq".into(),
            store::catalogue::FileRef { file: content.into(), size: 4, ..Default::default() },
        );
        store::testkit::map_root(&root, &cat);
    }

    fn headers(etags: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(e) = etags {
            h.insert(header::IF_NONE_MATCH, HeaderValue::from_str(e).unwrap());
        }
        h
    }

    fn header_of(r: &Response, name: header::HeaderName) -> Option<&str> {
        r.headers().get(name).map(|v| v.to_str().unwrap())
    }

    /// Every endpoint answers alike (crate::reply): a body with its type, gzip by the pack's own
    /// flag, its ETag and Cache-Control (for good when versioned); a 304 for any ETag of an
    /// `If-None-Match` list; 204 where a layer has no tile (also the depth's and the polar caps',
    /// which said 404).
    #[tokio::test]
    async fn endpoints_answer_alike() {
        let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        nas_with_packs(nas.path());
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        let q = |v: bool| v.then(|| "v=1".to_string());
        let roads = |h, v| road_tile(State(s.clone()), Path((3, 0, 0)), RawQuery(q(v)), h);
        let r = roads(headers(None), false).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(header_of(&r, header::CONTENT_ENCODING), Some("gzip"));
        assert_eq!(header_of(&r, header::CONTENT_TYPE), Some("application/octet-stream"));
        assert_eq!(header_of(&r, header::CACHE_CONTROL), Some("no-cache"));
        let etag = header_of(&r, header::ETAG).unwrap().to_string();
        let r = roads(headers(None), true).await;
        assert_eq!(header_of(&r, header::CACHE_CONTROL), Some("public, max-age=31536000, immutable"));
        let r = roads(headers(Some(&format!("\"other\", {etag}"))), true).await;
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header_of(&r, header::ETAG), Some(etag.as_str()));
        assert_eq!(header_of(&r, header::CACHE_CONTROL), Some("public, max-age=31536000, immutable"));
        // Not gzip'd where its pack's blobs aren't.
        let r = building_field_tile(State(s.clone()), Path((3, 0, 0)), RawQuery(None), headers(None)).await;
        assert_eq!((r.status(), header_of(&r, header::CONTENT_ENCODING)), (StatusCode::OK, None));
        // No tile there: 204, every layer alike; a URL naming nothing: 404.
        assert_eq!(
            road_tile(State(s.clone()), Path((3, 1, 0)), RawQuery(None), headers(None)).await.status(),
            StatusCode::NO_CONTENT
        );
        let depth = crate::depth::depth_tile(State(s.clone()), Path((5, 0, 0)), RawQuery(None), headers(None)).await;
        assert_eq!(depth.status(), StatusCode::NO_CONTENT);
        let polar = |pole: &str| {
            crate::terrain::polar_terrain_tile(
                State(s.clone()),
                Path((pole.into(), 4, 0, 0)),
                RawQuery(None),
                headers(None),
            )
        };
        assert_eq!(polar("n").await.status(), StatusCode::NO_CONTENT);
        assert_eq!(polar("x").await.status(), StatusCode::NOT_FOUND);
        // The rail frequencies: an ETag from the catalogue, and its 304.
        let rf = |h| crate::rail_freq_h(State(s.clone()), RawQuery(None), h);
        let r = rf(headers(None)).await;
        assert_eq!((r.status(), header_of(&r, header::ETAG)), (StatusCode::OK, Some("\"00000000000000aa\"")));
        assert_eq!(rf(headers(Some("\"00000000000000aa\""))).await.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn named_layers() {
        for l in ["labels", "stations", "ferries", "ov-whs"] {
            assert!(has_names(l), "{l}");
        }
        for l in ["roads", "rails", "buildings", "terrain", "marks-peak"] {
            assert!(!has_names(l), "{l}");
        }
    }
}
