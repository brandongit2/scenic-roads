//! The build page's map's finished tiles (task #674): every tile target the build has recorded as
//! built, by step, from its keys (`state/build/jobs.json`, crate::agent::build::Keys), for
//! `/work/built`. The history's job ends (what the map read before) reach back only as far as the
//! page has read them and the history keeps; the keys hold every target built, whenever, and every
//! Mac reads the same file, so each Mac's page draws the same tiles.
//!
//! Read only while a page asks (`ask`): a thread of its own reads the file again when it's older
//! than `EVERY` (about 600 KB over SMB); the answer is the last read, never waiting on it.

use super::build::Keys;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The keys are read again this often at most while a page asks.
pub const EVERY: Duration = Duration::from_secs(60);

/// Each step's tile targets ("z/x/y") recorded in `keys`: a per-target step's under its id (a
/// record's name with `-` for `_`), a `-root` step's (kept with the lo keys under its own name) as
/// the root tile, "0/0/0". Other records' targets (a worldwide step's) aren't tiles, and records of
/// steps this app doesn't know (`Keys::other`) aren't drawn: left out.
pub fn tiles_of(keys: &Keys) -> BTreeMap<String, Vec<String>> {
    let tile = |t: &str| {
        let p: Vec<&str> = t.split('/').collect();
        p.len() == 3 && p.iter().all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    };
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let known = Keys { other: BTreeMap::new(), ..keys.clone() };
    let Ok(Value::Object(m)) = serde_json::to_value(&known) else { return out };
    for (name, v) in m {
        let Value::Object(targets) = v else { continue };
        for t in targets.keys() {
            if tile(t) {
                out.entry(name.replace('_', "-")).or_default().push(t.clone());
            } else if name == "lo" && t.ends_with("-root") {
                out.entry(t.clone()).or_default().push("0/0/0".into());
            }
        }
    }
    out
}

#[derive(Default)]
struct Read {
    /// When the keys were last read (unix s; 0: never), and when, here.
    at: u64,
    read: Option<Instant>,
    steps: Arc<BTreeMap<String, Vec<String>>>,
    error: Option<String>,
}

pub struct BuiltView {
    root: PathBuf,
    read: Mutex<Read>,
    reading: AtomicBool,
}

impl std::fmt::Debug for BuiltView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltView").field("at", &self.read.lock().unwrap().at).finish()
    }
}

impl BuiltView {
    pub fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(BuiltView { root, read: Mutex::new(Read::default()), reading: AtomicBool::new(false) })
    }

    /// A page's ask: the reader started when it isn't running and the keys weren't read in the last
    /// `EVERY`; the last read: `{at, steps: {step: [z/x/y]}, error}`.
    pub fn ask(self: &Arc<Self>) -> Value {
        let due = self.read.lock().unwrap().read.is_none_or(|t| t.elapsed() >= EVERY);
        if due && !self.reading.swap(true, Ordering::SeqCst) {
            let me = self.clone();
            let started = std::thread::Builder::new().name("built-view".into()).spawn(move || {
                me.refresh();
                me.reading.store(false, Ordering::SeqCst);
            });
            if started.is_err() {
                self.reading.store(false, Ordering::SeqCst);
            }
        }
        let r = self.read.lock().unwrap();
        serde_json::json!({ "at": r.at, "steps": *r.steps, "error": r.error })
    }

    /// Reads the keys (the last read kept when they can't be read now).
    pub fn refresh(&self) {
        let got = Keys::load_strict(&self.root).map(|k| tiles_of(&k));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mut r = self.read.lock().unwrap();
        r.read = Some(Instant::now());
        match got {
            Ok(s) => {
                r.steps = Arc::new(s);
                r.at = now;
                r.error = None;
            }
            Err(e) => r.error = Some(format!("{e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_recorded_tile_by_its_step() {
        let mut k = Keys::default();
        k.record("terrain-deep", &[("10/257/342".into(), "a".into()), ("10/300/400".into(), "b".into())]);
        k.record("terrain-hi", &[("6/16/21".into(), "c".into())]);
        k.record("bldfield-lo", &[("3/2/2".into(), "d".into())]);
        k.record("terrain-root", &[("terrain-root".into(), "e".into())]);
        k.record("outlines", &[("outlines".into(), "f".into())]);
        k.record("catalog", &[("catalog".into(), "g".into())]);
        k.other.insert("terrain_fine".into(), serde_json::json!({ "6/1/1": "h" }));
        let t = tiles_of(&k);
        assert_eq!(t["terrain-deep"], ["10/257/342", "10/300/400"]);
        assert_eq!(t["terrain-hi"], ["6/16/21"]);
        assert_eq!(t["bldfield-lo"], ["3/2/2"]);
        assert_eq!(t["terrain-root"], ["0/0/0"]);
        assert_eq!(t.len(), 4, "{t:?}");
    }

    #[test]
    fn read_from_the_keys_file_while_asked() {
        let d = tempfile::tempdir().unwrap();
        let v = BuiltView::new(d.path().to_path_buf());
        v.refresh();
        assert_eq!(v.ask()["steps"], serde_json::json!({}), "no keys yet: none built");
        let mut k = Keys::default();
        k.record("terrain-deep", &[("10/1/2".into(), "a".into())]);
        k.save(d.path()).unwrap();
        v.refresh();
        let a = v.ask();
        assert_eq!(a["steps"]["terrain-deep"], serde_json::json!(["10/1/2"]));
        assert!(a["error"].is_null() && a["at"].as_u64().unwrap() > 0);
    }
}
