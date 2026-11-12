//! Settings profiles (task #819): named sets of every sidebar setting, shared by every user of the
//! map. One file, `<home>/profiles.json` (`{"v": 1, "default": id, "profiles": [{"id", "name",
//! "settings"}]}`), on the map server's Mac, in the lead's daily backup (agentcore::agent::backup,
//! `mac/profiles.json`). A laptop's server on demand (its offline map) keeps a list of its own, in
//! its own home. `settings` is the web app's own (its state without the camera: state.ts
//! `profileSettings`); the server only checks that it is an object. Before any profile is saved the
//! list is one, `base` ("Default"), whose settings are `{}`: the app's defaults.
//!
//!   GET    /api/profiles               the list, the default's id, and whether it can be changed now
//!   POST   /api/profiles               {name, settings, id?}: a new profile (`id`: one deleted, back
//!                                      by Undo); answers the list and the new id
//!   PUT    /api/profiles/{id}          {name?, settings?}: renamed, or its settings replaced
//!   DELETE /api/profiles/{id}          removed (never the default)
//!   POST   /api/profiles/{id}/default  made the default
//!
//! Every change reads the file again, changes it and writes it back whole, under one lock.

use crate::S;
use anyhow::{bail, ensure, Context, Result};
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The first profile: the app's defaults.
pub const BASE_ID: &str = "base";
const BASE_NAME: &str = "Default";
const MAX_NAME: usize = 60;
/// A profile's settings, serialised, at most (the app's are ~10 KB).
const MAX_SETTINGS: usize = 256 * 1024;
const MAX_PROFILES: usize = 200;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub settings: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct List {
    pub v: u32,
    pub default: String,
    pub profiles: Vec<Profile>,
}

impl Default for List {
    fn default() -> List {
        List {
            v: 1,
            default: BASE_ID.into(),
            profiles: vec![Profile { id: BASE_ID.into(), name: BASE_NAME.into(), settings: Map::new() }],
        }
    }
}

/// A change to the list.
#[derive(Debug, Clone)]
pub enum Op {
    Add { id: Option<String>, name: String, settings: Map<String, Value> },
    Update { id: String, name: Option<String>, settings: Option<Map<String, Value>> },
    Delete { id: String },
    SetDefault { id: String },
}

fn valid_id(id: &str) -> bool {
    id.len() == 4 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn clean_name(name: &str) -> Result<String> {
    let n = name.trim();
    ensure!(!n.is_empty(), "a profile needs a name");
    ensure!(n.chars().count() <= MAX_NAME, "a profile's name is at most {MAX_NAME} characters");
    ensure!(!n.chars().any(char::is_control), "a profile's name has no control characters");
    Ok(n.to_string())
}

fn check_settings(s: &Map<String, Value>) -> Result<()> {
    ensure!(serde_json::to_vec(s)?.len() <= MAX_SETTINGS, "a profile's settings are at most {MAX_SETTINGS} bytes");
    Ok(())
}

/// A new id: 4 characters of [a-z0-9], none of `taken`'s.
fn new_id(taken: &List) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    const ABC: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    loop {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let seed = format!("{t}.{}.{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
        let h = blake3::hash(seed.as_bytes());
        let id: String = h.as_bytes()[..4].iter().map(|b| ABC[*b as usize % ABC.len()] as char).collect();
        if !taken.profiles.iter().any(|p| p.id == id) {
            return id;
        }
    }
}

impl List {
    fn get(&self, id: &str) -> Result<usize> {
        self.profiles.iter().position(|p| p.id == id).with_context(|| format!("no profile {id}"))
    }

    fn name_free(&self, name: &str, except: Option<&str>) -> Result<()> {
        let taken =
            self.profiles.iter().any(|p| Some(p.id.as_str()) != except && p.name.to_lowercase() == name.to_lowercase());
        ensure!(!taken, "a profile is already named {name:?}");
        Ok(())
    }

    /// The list with `op` applied, and the profile it made or changed.
    pub fn apply(&self, op: Op) -> Result<(List, String)> {
        let mut l = self.clone();
        let id = match op {
            Op::Add { id, name, settings } => {
                let name = clean_name(&name)?;
                l.name_free(&name, None)?;
                check_settings(&settings)?;
                ensure!(l.profiles.len() < MAX_PROFILES, "at most {MAX_PROFILES} profiles");
                let id = match id {
                    Some(id) => {
                        ensure!(valid_id(&id), "a profile's id is 4 characters of a-z and 0-9");
                        ensure!(l.get(&id).is_err(), "a profile {id} already exists");
                        id
                    }
                    None => new_id(&l),
                };
                l.profiles.push(Profile { id: id.clone(), name, settings });
                id
            }
            Op::Update { id, name, settings } => {
                let i = l.get(&id)?;
                ensure!(name.is_some() || settings.is_some(), "nothing to change");
                if let Some(n) = name {
                    let n = clean_name(&n)?;
                    l.name_free(&n, Some(&id))?;
                    l.profiles[i].name = n;
                }
                if let Some(s) = settings {
                    check_settings(&s)?;
                    l.profiles[i].settings = s;
                }
                id
            }
            Op::Delete { id } => {
                let i = l.get(&id)?;
                if id == l.default {
                    bail!("the default profile can't be deleted: make another the default first");
                }
                l.profiles.remove(i);
                id
            }
            Op::SetDefault { id } => {
                l.get(&id)?;
                l.default = id.clone();
                id
            }
        };
        Ok((l, id))
    }

    /// A list read from a file: None when it doesn't read or doesn't hold together (a default that
    /// isn't in it, ids twice or malformed).
    fn parse(b: &[u8]) -> Option<List> {
        let l: List = serde_json::from_slice(b).ok()?;
        let mut ids: Vec<&str> = l.profiles.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        let ok = l.v == 1
            && l.profiles.iter().all(|p| valid_id(&p.id))
            && ids.windows(2).all(|w| w[0] != w[1])
            && ids.binary_search(&l.default.as_str()).is_ok();
        ok.then_some(l)
    }
}

/// Where the list is kept: `<home>/profiles.json`.
pub struct Profiles {
    file: PathBuf,
    /// Held across a change's read, change and write.
    lock: Mutex<()>,
}

/// Why a change can't be made now: the file is there but doesn't read.
#[derive(Debug)]
pub struct Unreadable(String);

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the profiles' file doesn't read ({}): nothing changed", self.0)
    }
}
impl std::error::Error for Unreadable {}

impl Profiles {
    pub fn new(home: &Path) -> Profiles {
        Profiles { file: home.join(jobrt::PROFILES), lock: Mutex::new(()) }
    }

    /// The list: Ok(None) when there is no file yet, Err when it's there but doesn't read or
    /// doesn't hold together (a hand edit): never taken as none, which a change would then write
    /// over every profile.
    fn read(&self) -> Result<Option<List>, Unreadable> {
        match std::fs::read(&self.file) {
            Ok(b) => List::parse(&b).map(Some).ok_or_else(|| Unreadable("not a list".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Unreadable(e.to_string())),
        }
    }

    /// The list and whether it can be changed now (not while its file doesn't read: the app's
    /// defaults served meanwhile).
    pub fn list(&self) -> (List, bool) {
        match self.read() {
            Ok(l) => (l.unwrap_or_default(), true),
            Err(e) => {
                eprintln!("profiles: {}: {}", self.file.display(), e.0);
                (List::default(), false)
            }
        }
    }

    /// Applies `op` to the list, read again under the lock and written back whole
    /// (store::whole::write): the new list and the profile's id.
    pub fn change(&self, op: Op) -> Result<(List, String)> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let (next, id) = self.read()?.unwrap_or_default().apply(op)?;
        if let Some(d) = self.file.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("make {}", d.display()))?;
        }
        store::whole::write(&self.file, &serde_json::to_vec_pretty(&next)?)
            .with_context(|| format!("write {}", self.file.display()))?;
        Ok((next, id))
    }
}

fn answer(l: &List, writable: bool, id: Option<&str>) -> Value {
    let mut v = json!({ "default": l.default, "profiles": l.profiles, "writable": writable });
    if let Some(id) = id {
        v["id"] = json!(id);
    }
    v
}

fn no_store(v: Value) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], Json(v)).into_response()
}

async fn changed(s: S, op: Op) -> Response {
    match tokio::task::spawn_blocking(move || s.profiles.change(op)).await {
        Ok(Ok((l, id))) => no_store(answer(&l, true, Some(&id))),
        Ok(Err(e)) if e.is::<Unreadable>() => (StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn get_list(State(s): State<S>) -> Response {
    match tokio::task::spawn_blocking(move || s.profiles.list()).await {
        Ok((l, w)) => no_store(answer(&l, w, None)),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
pub struct New {
    name: String,
    settings: Map<String, Value>,
    id: Option<String>,
}

pub async fn post(State(s): State<S>, Json(n): Json<New>) -> Response {
    changed(s, Op::Add { id: n.id, name: n.name, settings: n.settings }).await
}

#[derive(Deserialize)]
pub struct Edit {
    name: Option<String>,
    settings: Option<Map<String, Value>>,
}

pub async fn put(State(s): State<S>, UrlPath(id): UrlPath<String>, Json(e): Json<Edit>) -> Response {
    changed(s, Op::Update { id, name: e.name, settings: e.settings }).await
}

pub async fn delete(State(s): State<S>, UrlPath(id): UrlPath<String>) -> Response {
    changed(s, Op::Delete { id }).await
}

pub async fn set_default(State(s): State<S>, UrlPath(id): UrlPath<String>) -> Response {
    changed(s, Op::SetDefault { id }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post as post_r, put as put_r};
    use axum::Router;
    use tower::ServiceExt;

    fn app(s: &S) -> Router {
        Router::new()
            .route("/api/profiles", get(get_list).post(post))
            .route("/api/profiles/{id}", put_r(put).delete(delete))
            .route("/api/profiles/{id}/default", post_r(set_default))
            .with_state(s.clone())
    }

    async fn call(s: &S, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut r = Request::builder().method(method).uri(uri);
        if body.is_some() {
            r = r.header(header::CONTENT_TYPE, "application/json");
        }
        let req = r.body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap();
        let res = app(s).oneshot(req).await.unwrap();
        let st = res.status();
        let b = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (st, serde_json::from_slice(&b).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&b).into())))
    }

    fn names(v: &Value) -> Vec<String> {
        v["profiles"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect()
    }

    #[tokio::test]
    async fn the_api_lists_adds_renames_replaces_sets_the_default_and_deletes_in_the_home() {
        let nas = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        // Nothing saved yet: one profile, the app's defaults, the default.
        let (st, v) = call(&s, "GET", "/api/profiles", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            (v["default"].as_str(), names(&v), v["writable"].as_bool()),
            (Some(BASE_ID), vec!["Default".to_string()], Some(true))
        );
        assert_eq!(v["profiles"][0]["settings"], json!({}));
        // Add.
        let (st, v) =
            call(&s, "POST", "/api/profiles", Some(json!({"name": " Scenic night ", "settings": {"mode": "score"}})))
                .await;
        assert_eq!(st, StatusCode::OK);
        let id = v["id"].as_str().unwrap().to_string();
        assert!(valid_id(&id), "{id}");
        assert_eq!(names(&v), ["Default", "Scenic night"]);
        // In the home; nothing on the NAS (task #1119).
        let kept = List::parse(&std::fs::read(home.path().join(jobrt::PROFILES)).unwrap()).unwrap();
        assert_eq!(kept.profiles[1].settings, json!({"mode": "score"}).as_object().unwrap().clone());
        assert_eq!(std::fs::read_dir(nas.path()).unwrap().count(), 0, "nothing written on the NAS");
        // A name taken (in any case), or empty: refused.
        let (st, _) = call(&s, "POST", "/api/profiles", Some(json!({"name": "scenic NIGHT", "settings": {}}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _) = call(&s, "POST", "/api/profiles", Some(json!({"name": "  ", "settings": {}}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // Rename, replace.
        let (st, v) = call(&s, "PUT", &format!("/api/profiles/{id}"), Some(json!({"name": "Night"}))).await;
        assert_eq!((st, names(&v)), (StatusCode::OK, vec!["Default".to_string(), "Night".into()]));
        let (_, v) =
            call(&s, "PUT", &format!("/api/profiles/{id}"), Some(json!({"settings": {"mode": "curvy"}}))).await;
        assert_eq!(v["profiles"][1]["settings"], json!({"mode": "curvy"}));
        assert_eq!(v["profiles"][1]["name"], "Night");
        // The default can't be deleted; once another is the default, it can.
        let (st, _) = call(&s, "DELETE", &format!("/api/profiles/{BASE_ID}"), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (_, v) = call(&s, "POST", &format!("/api/profiles/{id}/default"), None).await;
        assert_eq!(v["default"].as_str(), Some(id.as_str()));
        let (st, v) = call(&s, "DELETE", &format!("/api/profiles/{BASE_ID}"), None).await;
        assert_eq!((st, names(&v)), (StatusCode::OK, vec!["Night".to_string()]));
        // Undo of a delete: the same id back.
        let (st, v) =
            call(&s, "POST", "/api/profiles", Some(json!({"name": "Default", "settings": {}, "id": BASE_ID}))).await;
        assert_eq!((st, v["id"].as_str()), (StatusCode::OK, Some(BASE_ID)));
        let (st, _) =
            call(&s, "POST", "/api/profiles", Some(json!({"name": "Other", "settings": {}, "id": BASE_ID}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // Unknown ids.
        let (st, _) = call(&s, "PUT", "/api/profiles/zzzz", Some(json!({"name": "x"}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        // A server started again over the same home (its file copied in by hand among them) serves
        // the same list.
        let s2 = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        let (_, v2) = call(&s2, "GET", "/api/profiles", None).await;
        assert_eq!(
            (names(&v2), v2["default"].as_str()),
            (vec!["Night".to_string(), "Default".into()], Some(id.as_str()))
        );
        // Another home over the same NAS (a laptop's server on demand) keeps a list of its own.
        let home2 = tempfile::tempdir().unwrap();
        let s3 = crate::test_state(home2.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        let (_, v3) = call(&s3, "GET", "/api/profiles", None).await;
        assert_eq!((names(&v3), v3["writable"].as_bool()), (vec!["Default".to_string()], Some(true)));
        assert_eq!(std::fs::read_dir(nas.path()).unwrap().count(), 0, "nothing written on the NAS");
    }

    #[tokio::test]
    async fn a_file_that_doesnt_read_is_never_written_over() {
        let nas = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let s = crate::test_state(home.path(), &store::nasfs::NasRoot::local_for_test(nas.path()));
        let (_, v) = call(&s, "POST", "/api/profiles", Some(json!({"name": "Night", "settings": {}}))).await;
        assert_eq!(names(&v), ["Default", "Night"]);
        let file = home.path().join(jobrt::PROFILES);
        std::fs::write(&file, b"{\"v\":1,").unwrap();
        // The app's defaults are served, read-only; a change is refused and the file left as it is.
        let (_, v) = call(&s, "GET", "/api/profiles", None).await;
        assert_eq!((names(&v), v["writable"].as_bool()), (vec!["Default".to_string()], Some(false)));
        let (st, _) = call(&s, "POST", "/api/profiles", Some(json!({"name": "Day", "settings": {}}))).await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(std::fs::read(&file).unwrap(), b"{\"v\":1,");
        let left: Vec<_> = std::fs::read_dir(home.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("profiles"))
            .collect();
        assert_eq!(left, [jobrt::PROFILES], "no temporary file left");
    }

    #[test]
    fn a_broken_file_or_one_whose_default_is_missing_reads_as_none() {
        assert!(List::parse(b"{").is_none());
        assert!(
            List::parse(br#"{"v":1,"default":"abcd","profiles":[{"id":"base","name":"D","settings":{}}]}"#).is_none()
        );
        assert!(List::parse(br#"{"v":1,"default":"base","profiles":[{"id":"base","name":"D","settings":{}},{"id":"base","name":"E","settings":{}}]}"#).is_none());
        assert_eq!(List::parse(&serde_json::to_vec(&List::default()).unwrap()), Some(List::default()));
    }
}
