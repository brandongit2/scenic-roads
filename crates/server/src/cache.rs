//! Browser caching, and the overlay layer files compressed once.
//!
//! The app puts each data file's build time in its URLs (`?v=…`, from meta "versions"), so a
//! versioned response can be cached for good ("immutable": not even revalidated on reload); a
//! rebuilt file gets new URLs. Unversioned ones keep a short lifetime.
//!
//! The overlay layers are tens of MB of GeoJSON each. Compressing them on every request (the
//! router's compression layer) took ~2.5 s per file and held a connection all that time; here each
//! is gzipped once (again only when the file changes), kept in memory, and served with an ETag.

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use flate2::{write::GzEncoder, Compression};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;

/// A request for a versioned URL (`v=` in its query).
pub fn versioned(query: Option<&str>) -> bool {
    query.is_some_and(|q| q.split('&').any(|kv| kv.starts_with("v=")))
}

/// Cache-Control for a data response: for good when versioned, else `fallback`.
pub fn cache_control(versioned: bool, fallback: &'static str) -> HeaderValue {
    HeaderValue::from_static(if versioned { "public, max-age=31536000, immutable" } else { fallback })
}

/// A file gzipped in memory.
pub struct Packed {
    pub gz: bytes::Bytes,
    pub etag: HeaderValue,
}

type Cell = Arc<OnceCell<Option<Arc<Packed>>>>;

/// Gzipped files by path and modification time.
#[derive(Default)]
pub struct Packs {
    cells: Mutex<HashMap<(PathBuf, u64), Cell>>,
}

fn mtime(path: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((t.as_secs() * 1000 + t.subsec_millis() as u64, m.len()))
}

fn pack(path: &Path) -> Option<Arc<Packed>> {
    let raw = std::fs::read(path).ok()?;
    let mut enc = GzEncoder::new(Vec::with_capacity(raw.len() / 6), Compression::new(6));
    enc.write_all(&raw).ok()?;
    let gz = bytes::Bytes::from(enc.finish().ok()?);
    let (t, len) = mtime(path)?;
    let etag = HeaderValue::from_str(&format!("\"{t:x}-{len:x}\"")).ok()?;
    Some(Arc::new(Packed { gz, etag }))
}

impl Packs {
    fn cell(&self, path: &Path) -> Option<Cell> {
        let (t, _) = mtime(path)?;
        let mut cells = self.cells.lock().unwrap();
        // A rebuilt file: drop its older versions.
        cells.retain(|(p, v), _| p != path || *v == t);
        Some(cells.entry((path.to_path_buf(), t)).or_default().clone())
    }

    /// The file gzipped (compressed on first use, off the async threads).
    pub async fn get(&self, path: &Path) -> Option<Arc<Packed>> {
        let cell = self.cell(path)?;
        let p = path.to_path_buf();
        cell.get_or_init(|| async move { tokio::task::spawn_blocking(move || pack(&p)).await.ok().flatten() }).await.clone()
    }

    /// Compress these files now, in the background, so the first page load finds them ready.
    pub fn warm(self: &Arc<Self>, paths: Vec<PathBuf>) {
        for p in paths.into_iter().filter(|p| p.exists()) {
            let me = self.clone();
            tokio::spawn(async move {
                me.get(&p).await;
            });
        }
    }
}

/// Serves a gzipped file: 304 when the browser's copy is current, gzip when it accepts it (every
/// browser does), else the file as is.
pub async fn respond(packs: &Packs, path: &Path, content_type: &'static str, headers: &HeaderMap, versioned: bool) -> Option<Response> {
    let p = packs.get(path).await?;
    let cache = cache_control(versioned, "no-cache");
    if headers.get(header::IF_NONE_MATCH).is_some_and(|v| v == p.etag) {
        return Some((StatusCode::NOT_MODIFIED, [(header::ETAG, p.etag.clone()), (header::CACHE_CONTROL, cache)]).into_response());
    }
    let gzip = headers.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("gzip"));
    let common = [
        (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
        (header::ETAG, p.etag.clone()),
        (header::CACHE_CONTROL, cache),
        (header::VARY, HeaderValue::from_static("Accept-Encoding")),
    ];
    if gzip {
        Some((common, [(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"))], p.gz.clone()).into_response())
    } else {
        let raw = tokio::fs::read(path).await.ok()?;
        Some((common, raw).into_response())
    }
}
