//! Browser caching, and the layer files compressed once.
//!
//! The app puts each data file's version in its URLs (`?v=…`, from meta "versions"), so a versioned
//! response can be cached for good ("immutable": not even revalidated on reload); a new version gets
//! new URLs. Unversioned ones revalidate with their ETag.
//!
//! The layer files are tens of MB of GeoJSON each. Each is read, given display names (`main`, `sub`
//! on every named feature), gzipped once per content and translations version, kept in memory, and
//! served with an ETag.

use crate::AppState;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use flate2::{write::GzEncoder, Compression};
use std::collections::HashMap;
use std::io::Write;
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

/// A layer file prepared for serving.
pub struct Packed {
    pub gz: bytes::Bytes,
    pub etag: HeaderValue,
}

type Cell = Arc<OnceCell<Arc<Packed>>>;

/// Prepared layer files by (content name, translations version).
#[derive(Default)]
pub struct Packs {
    cells: Mutex<HashMap<(String, u64), Cell>>,
}

impl Packs {
    /// Drops what was prepared for files no longer in the catalog.
    pub fn retain_contents(&self, keep: &std::collections::HashSet<String>) {
        self.cells.lock().unwrap().retain(|(c, _), _| keep.contains(c));
    }

    fn cell(&self, content: &str, names: u64) -> Cell {
        let mut cells = self.cells.lock().unwrap();
        // Older versions of the same file go.
        cells.retain(|(c, v), _| c != content || *v == names);
        cells.entry((content.to_string(), names)).or_default().clone()
    }
}

/// Name properties in our layer files, and their own-English properties.
pub(crate) const NAME_KEYS: [&str; 2] = ["name", "n"];
pub(crate) const EN_KEYS: [&str; 3] = ["en", "name_en", "name:en"];

/// The first coordinate of a GeoJSON geometry.
fn first_point(g: &serde_json::Value) -> Option<[f64; 2]> {
    let mut c = g.get("coordinates")?;
    loop {
        let a = c.as_array()?;
        if a.len() >= 2 && a[0].is_number() {
            return Some([a[0].as_f64()?, a[1].as_f64()?]);
        }
        c = a.first()?;
    }
}

/// Sets `<prefix>main` (when it differs from the name) and `<prefix>sub` (when there is one) on an
/// object, for the name in `name_key`. Whether it set either.
pub(crate) fn put_names(s: &AppState, o: &mut serde_json::Map<String, serde_json::Value>, name_key: &str, own: Option<&str>, at: [f64; 2], prefix: &str) -> bool {
    let Some(name) = o.get(name_key).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_owned) else { return false };
    let d = s.names.display(names::Kind::Other, &name, own, &[], at[0], at[1]);
    let mut set = false;
    if d.main != name {
        o.insert(format!("{prefix}main"), serde_json::Value::from(d.main));
        set = true;
    }
    if let Some(sub) = d.sub {
        o.insert(format!("{prefix}sub"), serde_json::Value::from(sub));
        set = true;
    }
    set
}

/// Gives every named thing in a layer file its display name: `main` (when it differs from the
/// name) and `sub` (when there is one). The files:
/// - GeoJSON: on each feature's properties, for `name` (or `n`), and `cmain`/`csub` for a World
///   Heritage component's own name (`cn`);
/// - the summits (`{"p": [[lon, lat, ele, name], …]}`): main and sub appended to each ("" for none);
/// - the ferry lines (`{id: {name, ends: [[lon, lat], …], …}}`): on each line; a ferry block's
///   features and its lines (`lines`), both.
///
/// A file with nothing to name is served as it is (re-encoding would sort its keys).
pub(crate) fn with_names(s: &AppState, raw: &[u8]) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(raw) else { return raw.to_vec() };
    let mut changed = false;
    if let Some(feats) = v.get_mut("features").and_then(|f| f.as_array_mut()) {
        for f in feats {
            let Some(at) = f.get("geometry").and_then(first_point) else { continue };
            let Some(props) = f.get_mut("properties").and_then(|p| p.as_object_mut()) else { continue };
            let Some(key) = NAME_KEYS.iter().find(|k| props.get(**k).and_then(|x| x.as_str()).is_some_and(|x| !x.is_empty())) else { continue };
            let own = EN_KEYS.iter().find_map(|k| props.get(*k).and_then(|x| x.as_str()).filter(|x| !x.is_empty())).map(str::to_owned);
            changed |= put_names(s, props, key, own.as_deref(), at, "");
            changed |= put_names(s, props, "cn", None, at, "c");
        }
        // A ferry block's lines (pipeline::ovconv) too.
        if let Some(lines) = v.get_mut("lines").and_then(|l| l.as_object_mut()) {
            for l in lines.values_mut() {
                let Some(o) = l.as_object_mut() else { continue };
                let Some(at) = o.get("ends").and_then(|e| e.get(0)).and_then(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])) else { continue };
                changed |= put_names(s, o, "name", None, at, "");
            }
        }
    } else if let Some(peaks) = v.get_mut("p").and_then(|p| p.as_array_mut()) {
        for e in peaks {
            let Some(a) = e.as_array_mut() else { continue };
            let (Some(lon), Some(lat), Some(name)) = (a.first().and_then(|x| x.as_f64()), a.get(1).and_then(|x| x.as_f64()), a.get(3).and_then(|x| x.as_str())) else { continue };
            let d = s.names.display(names::Kind::Other, name, None, &[], lon, lat);
            let main = if d.main != name { d.main } else { String::new() };
            a.truncate(4);
            a.push(main.into());
            a.push(d.sub.unwrap_or_default().into());
            changed = true;
        }
    } else if let Some(lines) = v.as_object_mut() {
        for l in lines.values_mut() {
            let Some(o) = l.as_object_mut() else { continue };
            let Some(at) = o.get("ends").and_then(|e| e.get(0)).and_then(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])) else { continue };
            changed |= put_names(s, o, "name", None, at, "");
        }
    }
    if !changed {
        return raw.to_vec();
    }
    serde_json::to_vec(&v).unwrap_or_else(|_| raw.to_vec())
}

/// A layer file read, named and gzipped; None when it couldn't be read (the NAS away), which isn't
/// kept: the next request tries again.
fn prepare(s: &AppState, logical: &str, content: &str, names: u64) -> Option<Arc<Packed>> {
    let raw = match s.data.global(logical) {
        Ok(b) => b?,
        Err(e) => {
            eprintln!("{logical}: {e:#}");
            return None;
        }
    };
    let body = if content.ends_with(".json") { with_names(s, &raw) } else { raw.to_vec() };
    let mut enc = GzEncoder::new(Vec::with_capacity(body.len() / 6), Compression::new(6));
    enc.write_all(&body).ok()?;
    let gz = bytes::Bytes::from(enc.finish().ok()?);
    let h = store::naming::parse_content_name(content).map(|c| c.hash16.to_string()).unwrap_or_default();
    let etag = HeaderValue::from_str(&format!("\"{h}-{names:x}\"")).ok()?;
    Some(Arc::new(Packed { gz, etag }))
}

/// A layer file prepared (once per content and translations version), with its content name.
async fn packed(s: &Arc<AppState>, logical: &str) -> Option<(String, Arc<Packed>)> {
    let content = s.data.content(logical)?;
    let names = s.names.version_all();
    let cell = s.packs.cell(&content, names);
    let (s2, l2, c2) = (s.clone(), logical.to_string(), content.clone());
    let p = cell
        .get_or_try_init(|| async move { tokio::task::spawn_blocking(move || prepare(&s2, &l2, &c2, names)).await.ok().flatten().ok_or(()) })
        .await
        .ok()?
        .clone();
    Some((content, p))
}

/// Prepares a layer file ahead of its first request. Whether it's ready.
pub async fn warm(s: &Arc<AppState>, logical: &str) -> bool {
    packed(s, logical).await.is_some()
}

/// Serves a layer file: 304 when the browser's copy is current, else gzip (every browser takes it).
pub async fn respond_layer(s: &Arc<AppState>, logical: &str, headers: &HeaderMap, versioned: bool) -> Option<Response> {
    let (content, p) = packed(s, logical).await?;
    let cache = cache_control(versioned, "no-cache");
    if headers.get(header::IF_NONE_MATCH).is_some_and(|v| v == p.etag) {
        return Some((StatusCode::NOT_MODIFIED, [(header::ETAG, p.etag.clone()), (header::CACHE_CONTROL, cache)]).into_response());
    }
    let ctype = if content.ends_with(".json") { "application/geo+json" } else { "application/octet-stream" };
    Some(
        (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(ctype)),
                (header::ETAG, p.etag.clone()),
                (header::CACHE_CONTROL, cache),
                (header::CONTENT_ENCODING, HeaderValue::from_static("gzip")),
                (header::VARY, HeaderValue::from_static("Accept-Encoding")),
            ],
            p.gz.clone(),
        )
            .into_response(),
    )
}
