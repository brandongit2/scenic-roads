//! Browser caching, the one responder every data endpoint answers through, the one cache type the
//! server keeps things in, and the layer files compressed once.
//!
//! The app puts each data file's version in its URLs (`?v=…`, from meta "versions"), so a versioned
//! response can be cached for good ("immutable": not even revalidated on reload); a new version gets
//! new URLs. Unversioned ones revalidate with their ETag.
//!
//! Every endpoint answers alike (`respond`, `not_modified`, `etag_match`, `none`, `blocking`):
//! - a body carries its type, `Content-Encoding: gzip` when it's gzip'd (a pack's tiles: by the
//!   pack's own flag, `store::pack::PackIndex::gzip`), its ETag when it has one, and Cache-Control
//!   (for good when versioned, else `no-cache`; `versioned_caching` in main.rs keeps "for good" to
//!   current versions);
//! - `If-None-Match` is read as a list (`"a", "b"`); a match is a 304 with the ETag and the same
//!   Cache-Control;
//! - a tile the layer hasn't, at an address that can hold one, is 204 (no body); 404 is for a URL
//!   that names nothing (an unknown kind, a tile outside the grid or the layer's zooms);
//! - away from the NAS, a tile not downloaded is 204 too (crate::data), never cached for good
//!   (`versioned_caching` in main.rs), and every version names where the downloads reach then
//!   (`Data::reach_token`), so nothing answered away is reused at home or for another reach;
//! - a read that failed is 503, so nothing is cached; a task that died, 500.
//!
//! The layer files are tens of MB of GeoJSON each. Each is read, given display names (`main`, `sub`
//! on every named feature), gzipped once per content and translations version, kept in memory, and
//! served with an ETag.

use crate::AppState;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use flate2::{write::GzEncoder, Compression};
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::OnceCell;

/// The tiles of one view, the unit the per-tile caches below are sized by: a 4K screen at one
/// pixel a CSS pixel (3840×2160, the largest the app is used on) in 256 px tiles, 15×9, with a ring
/// of one around it that panning reaches first: 17×11.
pub const VIEW_TILES: u64 = (3840 / 256 + 2) * (2160_u64.div_ceil(256) + 2);

/// A request for a versioned URL (`v=` in its query).
pub fn versioned(query: Option<&str>) -> bool {
    query.is_some_and(|q| q.split('&').any(|kv| kv.starts_with("v=")))
}

/// Cache-Control for a data response: for good when versioned, else `fallback`.
pub fn cache_control(versioned: bool, fallback: &'static str) -> HeaderValue {
    HeaderValue::from_static(if versioned { "public, max-age=31536000, immutable" } else { fallback })
}

/// On a response: not cached for good even under a current version (`versioned_caching`): what
/// the server made from other tiles, which a catalogue changing only those can leave stale.
#[derive(Clone, Copy)]
pub struct NotForGood;

/// Whether the request's `If-None-Match` (a list) names `etag`.
pub fn etag_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
}

/// The browser's copy is current.
pub fn not_modified(etag: &str, versioned: bool) -> Response {
    let mut r = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(e) = HeaderValue::from_str(etag) {
        r.headers_mut().insert(header::ETAG, e);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, cache_control(versioned, "no-cache"));
    r
}

/// A body: its type, gzip'd or not, its ETag when it has one.
pub fn respond(
    body: impl Into<axum::body::Body>,
    content_type: &'static str,
    gzip: bool,
    etag: Option<&str>,
    versioned: bool,
) -> Response {
    let mut r = Response::new(body.into());
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if gzip {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    if let Some(e) = etag.and_then(|e| HeaderValue::from_str(e).ok()) {
        h.insert(header::ETAG, e);
    }
    h.insert(header::CACHE_CONTROL, cache_control(versioned, "no-cache"));
    r
}

/// No such tile here (the layer hasn't one at this address): 204.
pub fn none() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// `f` on a blocking thread: its error said (`what`) and answered 503 (a read failed: try again
/// later), a task that died 500.
pub async fn blocking<T: Send + 'static>(
    what: impl std::fmt::Display,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, Response> {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(t)) => Ok(t),
        Ok(Err(e)) => {
            eprintln!("{what}: {e:#}");
            Err(StatusCode::SERVICE_UNAVAILABLE.into_response())
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR.into_response()),
    }
}

/// Serves something made here and gzip'd (names attached, tiles merged): a 304 for its ETag, else
/// from `kept`, else made now (`make`, on a blocking thread: None when there's nothing, 204) and kept
/// under `key`.
#[allow(clippy::too_many_arguments)]
pub async fn serve_made<K: Hash + Eq + Clone + Send + 'static>(
    kept: &'static Mutex<Lru<K, bytes::Bytes>>,
    key: K,
    etag: &str,
    headers: &HeaderMap,
    versioned: bool,
    content_type: &'static str,
    what: impl std::fmt::Display,
    make: impl FnOnce() -> anyhow::Result<Option<Vec<u8>>> + Send + 'static,
) -> Response {
    if etag_match(headers, etag) {
        return not_modified(etag, versioned);
    }
    if let Some(b) = kept.lock().unwrap().get(&key) {
        return respond(b, content_type, true, Some(etag), versioned);
    }
    match blocking(what, make).await {
        Ok(Some(b)) => {
            let b = bytes::Bytes::from(b);
            kept.lock().unwrap().put(key, b.clone(), b.len() as u64);
            respond(b, content_type, true, Some(etag), versioned)
        }
        Ok(None) => none(),
        Err(r) => r,
    }
}

/// The server's one cache:a map that drops its least recently used entries past a total weight
/// (bytes; nothing for what costs no memory of ours, such as a mapped file) or a number of entries.
pub struct Lru<K, V> {
    /// Value, weight, last use.
    map: HashMap<K, (V, u64, u64)>,
    by_use: BTreeMap<u64, K>,
    tick: u64,
    bytes: u64,
    cap: u64,
    entries: usize,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    /// At most `cap` bytes.
    pub fn new(cap: u64) -> Self {
        Self::bounded(usize::MAX, cap)
    }

    /// At most `entries` entries.
    pub fn entries(entries: usize) -> Self {
        Self::bounded(entries, u64::MAX)
    }

    /// At most `entries` entries and `cap` bytes.
    pub fn bounded(entries: usize, cap: u64) -> Self {
        Lru { map: HashMap::new(), by_use: BTreeMap::new(), tick: 0, bytes: 0, cap, entries }
    }

    pub fn get<Q: Hash + Eq + ?Sized>(&mut self, k: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        self.tick += 1;
        let t = self.tick;
        let e = self.map.get_mut(k)?;
        let k = self.by_use.remove(&e.2)?;
        e.2 = t;
        self.by_use.insert(t, k);
        Some(e.0.clone())
    }

    pub fn contains<Q: Hash + Eq + ?Sized>(&self, k: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.map.contains_key(k)
    }

    /// Adds `v`; past the budget the least recently used go (never the newest).
    pub fn put(&mut self, k: K, v: V, weight: u64) {
        self.tick += 1;
        let t = self.tick;
        if let Some((_, w, used)) = self.map.insert(k.clone(), (v, weight, t)) {
            self.by_use.remove(&used);
            self.bytes -= w;
        }
        self.by_use.insert(t, k);
        self.bytes += weight;
        while (self.bytes > self.cap || self.map.len() > self.entries) && self.map.len() > 1 {
            let Some((_, old)) = self.by_use.pop_first() else { break };
            if let Some((_, w, _)) = self.map.remove(&old) {
                self.bytes -= w;
            }
        }
    }

    /// Keeps only the entries `keep` says.
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        let gone: Vec<K> = self.map.iter().filter(|(k, (v, ..))| !keep(k, v)).map(|(k, _)| k.clone()).collect();
        for k in gone {
            if let Some((_, w, used)) = self.map.remove(&k) {
                self.bytes -= w;
                self.by_use.remove(&used);
            }
        }
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// A layer file prepared for serving.
pub struct Packed {
    pub gz: bytes::Bytes,
    pub etag: String,
}

type Cell = Arc<OnceCell<Arc<Packed>>>;

/// Prepared layer files by (content name, translations version).
#[derive(Default)]
pub struct Packs {
    cells: Mutex<HashMap<(String, u64), Cell>>,
}

impl Packs {
    /// Drops what was prepared for files no longer in the catalogue.
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
pub(crate) fn put_names(
    s: &AppState,
    o: &mut serde_json::Map<String, serde_json::Value>,
    name_key: &str,
    own: Option<&str>,
    at: [f64; 2],
    prefix: &str,
) -> bool {
    let Some(name) = o.get(name_key).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_owned) else {
        return false;
    };
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
/// - a ferry block's lines (`lines`: `{id: {name, ends: [[lon, lat], …], …}}`): on each line.
///
/// A file with nothing to name is served as it is (re-encoding would sort its keys).
pub(crate) fn with_names(s: &AppState, raw: &[u8]) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(raw) else { return raw.to_vec() };
    let mut changed = false;
    if let Some(feats) = v.get_mut("features").and_then(|f| f.as_array_mut()) {
        for f in feats {
            let Some(at) = f.get("geometry").and_then(first_point) else { continue };
            let Some(props) = f.get_mut("properties").and_then(|p| p.as_object_mut()) else { continue };
            let Some(key) =
                NAME_KEYS.iter().find(|k| props.get(**k).and_then(|x| x.as_str()).is_some_and(|x| !x.is_empty()))
            else {
                continue;
            };
            let own = EN_KEYS
                .iter()
                .find_map(|k| props.get(*k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()))
                .map(str::to_owned);
            changed |= put_names(s, props, key, own.as_deref(), at, "");
            changed |= put_names(s, props, "cn", None, at, "c");
        }
        // A ferry block's lines (pipeline::ovconv) too.
        if let Some(lines) = v.get_mut("lines").and_then(|l| l.as_object_mut()) {
            for l in lines.values_mut() {
                let Some(o) = l.as_object_mut() else { continue };
                let Some(at) =
                    o.get("ends").and_then(|e| e.get(0)).and_then(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?]))
                else {
                    continue;
                };
                changed |= put_names(s, o, "name", None, at, "");
            }
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
    let etag = format!("\"{h}-{names:x}\"");
    Some(Arc::new(Packed { gz, etag }))
}

/// A layer file prepared (once per content and translations version), with its content name.
async fn packed(s: &Arc<AppState>, logical: &str) -> Option<(String, Arc<Packed>)> {
    let content = s.data.content(logical)?;
    let names = s.names.version_all();
    let cell = s.packs.cell(&content, names);
    let (s2, l2, c2) = (s.clone(), logical.to_string(), content.clone());
    let p = cell
        .get_or_try_init(|| async move {
            tokio::task::spawn_blocking(move || prepare(&s2, &l2, &c2, names)).await.ok().flatten().ok_or(())
        })
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
    if etag_match(headers, &p.etag) {
        return Some(not_modified(&p.etag, versioned));
    }
    let ctype = if content.ends_with(".json") { "application/geo+json" } else { "application/octet-stream" };
    Some(respond(p.gz.clone(), ctype, true, Some(&p.etag), versioned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_drops_the_least_recently_used() {
        let mut l: Lru<u32, u32> = Lru::new(30);
        l.put(1, 10, 10);
        l.put(2, 20, 10);
        l.put(3, 30, 10);
        assert_eq!(l.get(&1), Some(10)); // 1 is now the most recent
        l.put(4, 40, 10); // over budget: 2 goes
        assert!(l.contains(&1) && !l.contains(&2) && l.contains(&3) && l.contains(&4));
        assert_eq!(l.bytes(), 30);
        // Replacing keeps the count right; something heavier than the budget stays alone.
        l.put(3, 31, 5);
        assert_eq!(l.bytes(), 25);
        l.put(9, 90, 100);
        assert!(l.contains(&9) && l.map.len() == 1 && l.bytes() == 100);
        // By entries: the least recently used past the count; a weightless entry counts too.
        let mut l: Lru<String, u32> = Lru::entries(2);
        l.put("a".into(), 1, 0);
        l.put("b".into(), 2, 0);
        assert_eq!(l.get("a"), Some(1));
        l.put("c".into(), 3, 0);
        assert!(l.contains("a") && !l.contains("b") && l.contains("c"));
        l.retain(|k, _| k != "a");
        assert!(!l.contains("a") && l.contains("c"));
    }

    #[test]
    fn if_none_match_is_a_list() {
        let h = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::IF_NONE_MATCH, HeaderValue::from_str(v).unwrap());
            h
        };
        assert!(etag_match(&h("\"a\""), "\"a\""));
        assert!(etag_match(&h("\"b\", \"a\""), "\"a\""));
        assert!(!etag_match(&h("\"b\""), "\"a\""));
        assert!(!etag_match(&HeaderMap::new(), "\"a\""));
    }
}
