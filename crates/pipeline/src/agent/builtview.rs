//! The build page's map's finished tiles (task #674): every tile target the build has recorded as
//! built, by step, from its keys (`state/build/jobs.json`, crate::agent::build::Keys), for
//! `/work/built`. The history's job ends (what the map read before) reach back only as far as the
//! page has read them and the history keeps; the keys hold every target built, whenever, and every
//! Mac reads the same file, so each Mac's page draws the same tiles.
//!
//! With each step's tiles, those that made nothing to publish (task #839): a target recorded built
//! whose published files (crate::agent::steps::output_named) the manifest doesn't name, e.g. a deep
//! tile with no fine data there (crate::terrain_deep, `PRESENCE`), sent as `empty: {step: [i]}`,
//! indexes into the step's list (6,276 of terrain-deep's 7,243 in term 599's brief, 29 Oct: ~50 KB on 247 KB). The keys and the
//! manifest come from one read of the records' brief.
//!
//! And which are published (task #866): a target is published only while the latest catalogue
//! (crate::agent::cataloguesview `names`) still names one of its published files
//! (crate::agent::steps::output_names), sent as `published: {step: [i]}` for every step whose files
//! are told by name (an empty list: none published) with that catalogue's number (`catalogue`), both
//! left out before the catalogues are read. A removal (a drop's null in a delta) ends a tile's
//! published life whatever catalogue named it before: Alaska's 4,363 deep packs dropped by
//! catalogue 756 (29 Oct) had stayed drawn as published, their drop runs' job ends taken as
//! published once a later catalogue came.
//!
//! Read only while a page asks (`ask`): a thread of its own reads the file again when it's older
//! than `EVERY` (about 600 KB over SMB); the answer is the last read, never waiting on it.

use super::build::Keys;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The keys are read again this often at most while a page asks.
pub const EVERY: Duration = Duration::from_secs(60);

/// Each step's tile targets ("z/x/y") recorded in `keys`: a per-target step's under its id (a
/// record's name with `-` for `_`), a `-root` step's (kept with the lo keys under its own name) as
/// the root tile, "0/0/0". Other records' targets (a worldwide step's) aren't tiles, records of
/// steps this app doesn't know (`Keys::other`: a retired step's, terrain_fine's) aren't drawn, and
/// terrain-hi's, slope-hi's and tree cover's z3 records (whole runs from before their z6 tiles,
/// `Keys`' docs; 18 each of terrain's and slope's on 26 Oct) aren't targets now: left out.
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
        let z3_old = matches!(name.as_str(), "terrain_hi" | "slope_hi" | "trees");
        let step = name.replace('_', "-");
        for (t, k) in &targets {
            // (A drop's record, its "none" key (build::is_none_key): the tile left coverage, its
            // packs dropped by design: not a tile built, left out (#839 review; 436 of terrain-hi's
            // 520 with no pack, 29 Oct).)
            if k.as_str().is_some_and(|k| super::build::is_none_key(&step, t, k)) {
                continue;
            }
            if tile(t) && !(z3_old && t.starts_with("3/")) {
                out.entry(step.clone()).or_default().push(t.clone());
            } else if name == "lo" && t.ends_with("-root") {
                out.entry(t.clone()).or_default().push("0/0/0".into());
            }
        }
    }
    out
}

/// Each step's tile targets whose record in `keys` is a drop's (its "none" key, build::is_none_key:
/// the tile left coverage, its packs dropped), left out of `tiles_of`: the page leaves out a job end
/// for one of them (the drop run's own), task #866 review: an exact signal, not a time guess.
pub fn dropped_of(keys: &Keys) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let known = Keys { other: BTreeMap::new(), ..keys.clone() };
    let Ok(Value::Object(m)) = serde_json::to_value(&known) else { return out };
    for (name, v) in m {
        let Value::Object(targets) = v else { continue };
        let step = name.replace('_', "-");
        for (t, k) in &targets {
            if k.as_str().is_some_and(|k| super::build::is_none_key(&step, t, k)) {
                out.entry(step.clone()).or_default().push(t.clone());
            }
        }
    }
    out
}

/// Each step's tiles in `steps` (`tiles_of`) that made nothing to publish: indexes into its list of
/// the targets whose published files manifest `m` doesn't name (crate::agent::steps::output_named);
/// a step whose files aren't told by name, or with none such, left out.
pub fn empty_of(steps: &BTreeMap<String, Vec<String>>, m: &BTreeMap<String, String>) -> BTreeMap<String, Vec<usize>> {
    let mut out = BTreeMap::new();
    for (step, tiles) in steps {
        let none: Vec<usize> = tiles
            .iter()
            .enumerate()
            .filter(|(_, t)| super::steps::output_named(step, t, m) == Some(false))
            .map(|(i, _)| i)
            .collect();
        if !none.is_empty() {
            out.insert(step.clone(), none);
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
    empty: Arc<BTreeMap<String, Vec<usize>>>,
    dropped: Arc<BTreeMap<String, Vec<String>>>,
    error: Option<String>,
}

/// Each told step's tiles in `steps` (`tiles_of`) the latest catalogue publishes: indexes into its
/// list of the targets one of whose published files (crate::agent::steps::output_names) `has` names.
pub fn published_of(steps: &BTreeMap<String, Vec<String>>, has: impl Fn(&str) -> bool) -> BTreeMap<String, Vec<usize>> {
    let mut out = BTreeMap::new();
    for (step, tiles) in steps {
        let mut told = false;
        let mut on = vec![];
        for (i, t) in tiles.iter().enumerate() {
            let Some(names) = super::steps::output_names(step, t) else { continue };
            told = true;
            if names.iter().any(|n| has(n)) {
                on.push(i);
            }
        }
        if told {
            out.insert(step.clone(), on);
        }
    }
    out
}

pub struct BuiltView {
    root: store::nasfs::NasRoot,
    /// The catalogues read (the latest's names publish a tile); None: not told.
    catalogues: Option<Arc<super::cataloguesview::CataloguesView>>,
    read: Mutex<Read>,
    reading: AtomicBool,
}

impl std::fmt::Debug for BuiltView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltView").field("at", &self.read.lock().unwrap().at).finish()
    }
}

impl BuiltView {
    /// `root` read on the server's short schedule (store::nasfs::NasRoot::short_retry): a page's
    /// answer, read again a minute on; a blip ridden out, an outage shown as the error at once.
    pub fn new(
        root: &store::nasfs::NasRoot,
        catalogues: Option<Arc<super::cataloguesview::CataloguesView>>,
    ) -> Arc<Self> {
        Arc::new(BuiltView {
            root: root.short_retry(),
            catalogues,
            read: Mutex::new(Read::default()),
            reading: AtomicBool::new(false),
        })
    }

    /// A page's ask: the reader started when it isn't running and the keys weren't read in the last
    /// `EVERY` (and the catalogues' reader, when due); the last read: `{at, steps: {step: [z/x/y]},
    /// empty: {step: [i]}, dropped: {step: [z/x/y]}, error}`, with `published: {step: [i]}` and `catalogue` once the
    /// catalogues are read.
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
        let names = self.catalogues.as_ref().and_then(|cv| {
            cv.wake();
            cv.names()
        });
        let (at, steps, empty, dropped, error) = {
            let r = self.read.lock().unwrap();
            (r.at, r.steps.clone(), r.empty.clone(), r.dropped.clone(), r.error.clone())
        };
        let mut a =
            serde_json::json!({ "at": at, "steps": *steps, "empty": *empty, "dropped": *dropped, "error": error });
        if let Some((n, names)) = names {
            let has = |s: &str| names.contains(&super::cataloguesview::hash(s));
            a["published"] = serde_json::json!(published_of(&steps, has));
            a["catalogue"] = n.into();
        }
        a
    }

    /// Reads the keys and the manifest (the last read kept when they can't be read now).
    pub fn refresh(&self) {
        let got = crate::out::brief_now(&self.root).map(|b| {
            let steps = tiles_of(&b.keys);
            let empty = empty_of(&steps, &b.manifest);
            (steps, empty, dropped_of(&b.keys))
        });
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mut r = self.read.lock().unwrap();
        r.read = Some(Instant::now());
        match got {
            Ok((s, e, d)) => {
                r.steps = Arc::new(s);
                r.empty = Arc::new(e);
                r.dropped = Arc::new(d);
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
        k.record("terrain-hi", &[("6/16/21".into(), "c".into()), ("3/2/2".into(), "old".into())]);
        k.record("trees", &[("3/2/2".into(), "old".into())]);
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
        let v = BuiltView::new(&store::nasfs::NasRoot::local_for_test(d.path()), None);
        v.refresh();
        assert_eq!(v.ask()["steps"], serde_json::json!({}), "no keys yet: none built");
        let mut k = Keys::default();
        k.record("terrain-deep", &[("10/1/2".into(), "a".into())]);
        k.save(&store::nasfs::NasRoot::local_for_test(d.path())).unwrap();
        v.refresh();
        let a = v.ask();
        assert_eq!(a["steps"]["terrain-deep"], serde_json::json!(["10/1/2"]));
        assert!(a["error"].is_null() && a["at"].as_u64().unwrap() > 0);
        assert_eq!(a["empty"], serde_json::json!({ "terrain-deep": [0] }), "no pack named: nothing to publish");
        assert!(a.get("published").is_none(), "no catalogues told: not said");
        let mut k = Keys::default();
        k.record("terrain-deep", &[("10/1/2".into(), "k1".into())]);
        let none = crate::agent::build::none_deep_key("terrain-deep", "10/3/4");
        k.record("terrain-deep", &[("10/3/4".into(), none)]);
        assert_eq!(dropped_of(&k), BTreeMap::from([("terrain-deep".to_string(), vec!["10/3/4".to_string()])]));
        assert_eq!(tiles_of(&k)["terrain-deep"], ["10/1/2"], "a drop's record: no tile");
    }

    #[test]
    fn the_targets_that_made_nothing_to_publish() {
        let mut k = Keys::default();
        k.record(
            "terrain-deep",
            &[("10/1/2".into(), "a".into()), ("10/3/4".into(), "b".into()), ("10/5/6".into(), "c".into())],
        );
        k.record("slope-deep", &[("10/1/2".into(), "a".into())]);
        k.record("terrain-hi", &[("6/1/2".into(), "a".into())]);
        k.record("terrain-root", &[("terrain-root".into(), "e".into())]);
        let m: BTreeMap<String, String> = [
            ("layers/terrain/deep/10-3-4".to_string(), "x".to_string()),
            ("layers/slope/deep/10-1-2".into(), "y".into()),
            ("layers/terrain/hi/6-1-2".into(), "z".into()),
        ]
        .into();
        let steps = tiles_of(&k);
        let e = empty_of(&steps, &m);
        assert_eq!(e["terrain-deep"], [0, 2], "10/1/2 and 10/5/6 made no pack");
        assert_eq!(e.len(), 1, "slope-deep's and terrain-hi's are named; the root's not told: {e:?}");
    }

    #[test]
    fn a_drops_record_is_no_tile_built() {
        use super::super::build::{none_deep_key, none_piece_key, piece_key_made};
        let mut k = Keys::default();
        let no_hi = piece_key_made((21, 18), "fp", "src", "w", false);
        k.record("terrain-hi", &[("6/1/2".into(), none_piece_key("terrain-hi", "6/1/2")), ("6/21/18".into(), no_hi)]);
        k.record("terrain-deep", &[("10/1/2".into(), none_deep_key("terrain-deep", "10/1/2"))]);
        let steps = tiles_of(&k);
        assert_eq!(steps["terrain-hi"], ["6/21/18"], "the drop's left out: {steps:?}");
        assert!(!steps.contains_key("terrain-deep"), "a deep drop too");
        let e = empty_of(&steps, &BTreeMap::new());
        assert_eq!(e["terrain-hi"], [0], "no hi tiles made: listed, nothing to publish");
    }

    /// Task #866: a tile is published only while the latest catalogue names its output; a delta's
    /// null (a drop) ends it, though an older catalogue kept named it.
    #[test]
    fn published_only_while_the_latest_catalogue_names_it() {
        use super::super::cataloguesview::{hash, CataloguesView, Source};
        use std::io::Write;
        #[derive(Default)]
        struct Mem(Mutex<BTreeMap<String, Vec<u8>>>);
        impl Source for Mem {
            fn list(&self) -> Option<Vec<String>> {
                Some(self.0.lock().unwrap().keys().cloned().collect())
            }
            fn read(&self, name: &str) -> Option<Vec<u8>> {
                self.0.lock().unwrap().get(name).cloned()
            }
        }
        let zst = |v: &Value| {
            let mut e = zstd::stream::Encoder::new(vec![], 3).unwrap();
            e.write_all(&serde_json::to_vec(v).unwrap()).unwrap();
            e.finish().unwrap()
        };
        let f = |n: &str| serde_json::json!({"file": format!("{n}.aaaa.pack"), "size": 1, "fmt": 1});
        let (deep, fine, hi) = ("layers/terrain/deep/10-1-2", "layers/terrain/fine/10-3-4", "layers/terrain/hi/6-1-2");
        let c1 = serde_json::json!({"n": 1, "created": "2026-10-29T00:00:00Z",
            "layers": {"terrain": {"deep": {"10/1/2": deep}, "hi": {"6/1/2": hi}}},
            "files": {deep: f(deep), fine: f(fine), hi: f(hi)}});
        // 2: the retired-name pack of 10/3/4 dropped (as catalogue 756 dropped Alaska's), and the
        // hi pack with its tile.
        let p2 = serde_json::json!({"o": {"created": {"=": "2026-10-29T01:00:00Z"},
            "layers": {"o": {"terrain": {"o": {"hi": {"o": {"6/1/2": null}}}}}},
            "files": {"o": {fine: null, hi: null}}}});
        let mem = Arc::new(Mem::default());
        mem.0.lock().unwrap().insert("1.json.zst".into(), zst(&c1));
        let cv = CataloguesView::new(mem.clone());
        assert!(cv.names().is_none(), "nothing read yet");
        cv.refresh();
        let steps: BTreeMap<String, Vec<String>> = [
            ("terrain-deep".to_string(), vec!["10/1/2".to_string(), "10/3/4".into(), "10/5/6".into()]),
            ("terrain-hi".into(), vec!["6/1/2".into()]),
            ("pack".into(), vec!["6/1/2".into()]),
        ]
        .into();
        let (n, names) = cv.names().unwrap();
        assert_eq!(n, 1);
        let p = published_of(&steps, |s| names.contains(&hash(s)));
        assert_eq!(p["terrain-deep"], [0, 1], "10/5/6 never named");
        assert_eq!(p["terrain-hi"], [0]);
        assert!(!p.contains_key("pack"), "a work step's files aren't told");
        mem.0
            .lock()
            .unwrap()
            .insert("2.delta.json.zst".into(), zst(&serde_json::json!({"n": 2, "of": 1, "patch": p2})));
        cv.refresh();
        let (n, names) = cv.names().unwrap();
        assert_eq!(n, 2);
        let p = published_of(&steps, |s| names.contains(&hash(s)));
        assert_eq!(p["terrain-deep"], [0], "the dropped pack's tile is published no more");
        assert_eq!(p["terrain-hi"], Vec::<usize>::new(), "told, none published");
    }
}
