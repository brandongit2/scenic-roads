//! The build page's Catalogues map (docs/pool.md §10, task #629): every catalog on the NAS
//! (`catalog/<n>.json.zst` whole, `<n>.delta.json.zst` as what changed over the one before,
//! store::catalog) as the map tiles it published, for `/work/catalogs`.
//!
//! A catalog publishes a tile when a file it names for that tile is new or other than the last
//! catalog's: a z6 tile by its base pack, road values, hi data, landmark points, its layers' hi
//! packs and the terrain's deep packs under it (z10 keys, their z6 tile's); a z3 tile by its layers' lo
//! packs and area overlays' details. Its entry: its number, time (`created`), whether it was written
//! whole, and the z3 and z6 tiles it published (`y * 2^z + x`), and those it dropped. The oldest
//! catalog kept (GC keeps 14 days') publishes every tile it names: `first`, what was there before it
//! isn't known.
//!
//! Read only while a page asks (`ask`): a thread of its own lists the folder (no more often than
//! `EVERY`) and reads the catalogs after the last it read, oldest first, each through the agent's
//! NAS reads (crate::agent::nasread); the answer is what's read so far, never waiting on it. What it
//! keeps: the entries (a few hundred, tens of tiles each) and, to tell a whole catalog's tiles from
//! the last's, each tile file's name by its key, hashed (some 18 000, about 2 MB): never a catalog.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The folder is listed again this often at most while a page asks.
pub const EVERY: Duration = Duration::from_secs(20);

/// One catalog as the map shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Entry {
    pub n: u64,
    /// Its `created`, unix seconds (0: not read).
    pub t: u64,
    /// Written whole (else as a delta).
    pub whole: bool,
    /// The oldest catalog read: every tile it names counts as published by it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub first: bool,
    /// The tiles it published, `y * 2^z + x`, sorted.
    pub z3: Vec<u32>,
    pub z6: Vec<u32>,
    /// The tiles it no longer names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub z3_gone: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub z6_gone: Vec<u32>,
}

/// A tile as kept: its zoom (3 or 6) and index.
type Tile = (u8, u32);

/// The tile a file key ("z/x/y") belongs to: z3 and z6 as they are, a deeper key its z6 tile's;
/// none for another zoom.
fn tile_of(key: &str) -> Option<Tile> {
    let mut it = key.split('/').map(|s| s.parse::<u32>().ok());
    let (z, x, y) = (it.next()??, it.next()??, it.next()??);
    match z {
        3 | 6 if x < 1 << z && y < 1 << z => Some((z as u8, (y << z) + x)),
        7..=24 => {
            let (x, y) = (x >> (z - 6), y >> (z - 6));
            (x < 64 && y < 64).then_some((6, (y << 6) + x))
        }
        _ => None,
    }
}

fn hash(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

/// RFC 3339 UTC ("2026-10-23T11:07:54Z") as unix seconds.
fn unix_of(s: &str) -> Option<u64> {
    let day = super::backup::parse_day(s.get(..10)?)?;
    let hms = s.get(11..19)?;
    let p = |r: std::ops::Range<usize>| hms.get(r)?.parse::<u64>().ok();
    Some(day + p(0..2)? * 3600 + p(3..5)? * 60 + p(6..8)?)
}

/// A layer's packs keyed by tile.
const PACKS: [&str; 3] = ["lo", "hi", "deep"];

/// A whole catalog, only what's read of it (the rest skipped as it parses).
#[derive(Default, Deserialize)]
struct Slim {
    #[serde(default)]
    created: String,
    #[serde(default)]
    base: BTreeMap<String, String>,
    #[serde(default)]
    roads: BTreeMap<String, String>,
    #[serde(default)]
    hidata: BTreeMap<String, String>,
    #[serde(default)]
    markdata: BTreeMap<String, String>,
    #[serde(default)]
    ovdata: BTreeMap<String, String>,
    #[serde(default)]
    layers: BTreeMap<String, SlimLayer>,
}

#[derive(Default, Deserialize)]
struct SlimLayer {
    #[serde(default)]
    lo: BTreeMap<String, String>,
    #[serde(default)]
    hi: BTreeMap<String, String>,
    #[serde(default)]
    deep: BTreeMap<String, String>,
}

/// A delta catalog's header and patch (store::catalog::Delta), only what's read of it: the
/// patch's tile sections (the coverage's, megabytes as JSON values, skipped as it parses).
#[derive(Deserialize)]
struct SlimDelta {
    of: u64,
    patch: SlimPatch,
}

/// A catalog's patch: `{"=": catalog}` or `{"o": {section: patch}}` (store::catalog::delta).
#[derive(Default, Deserialize)]
struct SlimPatch {
    #[serde(rename = "=")]
    whole: Option<Slim>,
    o: Option<SlimSections>,
}

#[derive(Default, Deserialize)]
struct SlimSections {
    created: Option<Value>,
    base: Option<Value>,
    roads: Option<Value>,
    hidata: Option<Value>,
    markdata: Option<Value>,
    ovdata: Option<Value>,
    layers: Option<Value>,
}

/// Every tile file the last catalog read names: by section ("base", "<layer>.hi"), its key → its
/// tile and its name hashed; and how many files each tile has.
#[derive(Default)]
struct Files {
    by: HashMap<String, HashMap<String, (Tile, u64)>>,
    count: HashMap<Tile, u32>,
}

/// What one catalog changed, as it's applied.
#[derive(Default)]
struct Change {
    set: BTreeSet<Tile>,
    dropped: BTreeSet<Tile>,
}

impl Files {
    fn put(&mut self, sec: &str, key: &str, name: &str, ch: &mut Change) {
        let Some(t) = tile_of(key) else { return };
        let h = hash(name);
        let m = self.by.entry(sec.to_string()).or_default();
        match m.insert(key.to_string(), (t, h)) {
            Some((_, old)) if old == h => {}
            Some(_) => {
                ch.set.insert(t);
            }
            None => {
                *self.count.entry(t).or_default() += 1;
                ch.set.insert(t);
            }
        }
    }

    fn drop_key(&mut self, sec: &str, key: &str, ch: &mut Change) {
        let Some((t, _)) = self.by.get_mut(sec).and_then(|m| m.remove(key)) else { return };
        let c = self.count.entry(t).or_default();
        *c = c.saturating_sub(1);
        if *c == 0 {
            self.count.remove(&t);
            ch.dropped.insert(t);
        }
    }

    /// Section `sec` as `all` names it, whole.
    fn set_all<'a>(&mut self, sec: &str, all: impl Iterator<Item = (&'a str, &'a str)>, ch: &mut Change) {
        let mut keep = BTreeSet::new();
        for (k, v) in all {
            self.put(sec, k, v, ch);
            keep.insert(k.to_string());
        }
        let gone: Vec<String> =
            self.by.get(sec).map_or(vec![], |m| m.keys().filter(|k| !keep.contains(*k)).cloned().collect());
        for k in gone {
            self.drop_key(sec, &k, ch);
        }
    }

    fn clear(&mut self, sec: &str, ch: &mut Change) {
        self.set_all(sec, std::iter::empty(), ch);
    }

    /// The sections a layer's packs are kept in, those there now.
    fn layer_secs(&self, layer: &str) -> Vec<String> {
        PACKS.iter().map(|p| format!("{layer}.{p}")).filter(|s| self.by.contains_key(s)).collect()
    }

    fn layers(&self) -> BTreeSet<String> {
        self.by.keys().filter_map(|s| s.rsplit_once('.').map(|(l, _)| l.to_string())).collect()
    }

    fn whole(&mut self, c: &Slim, ch: &mut Change) {
        for (sec, m) in [
            ("base", &c.base),
            ("roads", &c.roads),
            ("hidata", &c.hidata),
            ("markdata", &c.markdata),
            ("ovdata", &c.ovdata),
        ] {
            self.set_all(sec, m.iter().map(|(k, v)| (k.as_str(), v.as_str())), ch);
        }
        for l in self.layers() {
            if !c.layers.contains_key(&l) {
                for s in self.layer_secs(&l) {
                    self.clear(&s, ch);
                }
            }
        }
        for (name, l) in &c.layers {
            self.whole_layer(name, l, ch);
        }
    }

    fn whole_layer(&mut self, name: &str, l: &SlimLayer, ch: &mut Change) {
        for (p, m) in [("lo", &l.lo), ("hi", &l.hi), ("deep", &l.deep)] {
            self.set_all(&format!("{name}.{p}"), m.iter().map(|(k, v)| (k.as_str(), v.as_str())), ch);
        }
    }

    /// A delta's patch applied; the catalog's `created` if it names one.
    fn patch(&mut self, p: &SlimPatch, ch: &mut Change) -> anyhow::Result<Option<String>> {
        if let Some(c) = &p.whole {
            self.whole(c, ch);
            return Ok(Some(c.created.clone()));
        }
        let Some(o) = &p.o else { anyhow::bail!("not a patch") };
        for (k, sub) in [
            ("base", &o.base),
            ("roads", &o.roads),
            ("hidata", &o.hidata),
            ("markdata", &o.markdata),
            ("ovdata", &o.ovdata),
        ] {
            if let Some(sub) = sub {
                self.map(k, sub, ch)?;
            }
        }
        if let Some(l) = &o.layers {
            self.layers_patch(l, ch)?;
        }
        Ok(o.created.as_ref().and_then(|c| op(c).ok()).and_then(|(_, v)| v.as_str().map(str::to_string)))
    }

    fn map(&mut self, sec: &str, p: &Value, ch: &mut Change) -> anyhow::Result<()> {
        if p.is_null() {
            self.clear(sec, ch);
            return Ok(());
        }
        match op(p)? {
            ("=", Value::Object(m)) => {
                self.set_all(sec, m.iter().filter_map(|(k, v)| Some((k.as_str(), v.as_str()?))), ch);
            }
            ("=", _) => self.clear(sec, ch),
            ("o", Value::Object(m)) => {
                for (k, v) in m {
                    if v.is_null() {
                        self.drop_key(sec, k, ch);
                    } else if let ("=", Value::String(name)) = op(v)? {
                        self.put(sec, k, name, ch);
                    }
                }
            }
            _ => anyhow::bail!("{sec}: not a patch of a map"),
        }
        Ok(())
    }

    fn layers_patch(&mut self, p: &Value, ch: &mut Change) -> anyhow::Result<()> {
        match op(p)? {
            ("=", v) => {
                let all: BTreeMap<String, SlimLayer> = serde_json::from_value(v.clone())?;
                for l in self.layers() {
                    if !all.contains_key(&l) {
                        for s in self.layer_secs(&l) {
                            self.clear(&s, ch);
                        }
                    }
                }
                for (name, l) in &all {
                    self.whole_layer(name, l, ch);
                }
            }
            ("o", Value::Object(o)) => {
                for (name, lp) in o {
                    if lp.is_null() {
                        for s in self.layer_secs(name) {
                            self.clear(&s, ch);
                        }
                        continue;
                    }
                    match op(lp)? {
                        ("=", v) => self.whole_layer(name, &serde_json::from_value(v.clone())?, ch),
                        ("o", Value::Object(packs)) => {
                            for (pk, sub) in packs {
                                if PACKS.contains(&pk.as_str()) {
                                    self.map(&format!("{name}.{pk}"), sub, ch)?;
                                }
                            }
                        }
                        _ => anyhow::bail!("layer {name}: not a patch"),
                    }
                }
            }
            _ => anyhow::bail!("layers: not a patch"),
        }
        Ok(())
    }
}

/// A patch's one operation and its value.
fn op(p: &Value) -> anyhow::Result<(&str, &Value)> {
    match p.as_object().filter(|o| o.len() == 1).and_then(|o| o.iter().next()) {
        Some((k, v)) => Ok((k.as_str(), v)),
        None => anyhow::bail!("not a patch"),
    }
}

/// Where the catalogs are read: the NAS's folder, or a test's.
pub trait Source: Send + Sync {
    /// The folder's file names; None when it can't be listed.
    fn list(&self) -> Option<Vec<String>>;
    /// A file's bytes; None when it can't be read.
    fn read(&self, name: &str) -> Option<Vec<u8>>;
}

/// The NAS's `catalog/`, read through the agent's NAS reads under their deadline.
pub struct NasCatalogs(pub PathBuf);

impl Source for NasCatalogs {
    fn list(&self) -> Option<Vec<String>> {
        let p = self.0.clone();
        super::nasread::with(&self.0, move || {
            let mut out = vec![];
            for e in std::fs::read_dir(&p)? {
                if let Some(n) = e?.file_name().to_str() {
                    out.push(n.to_string());
                }
            }
            Ok(out)
        })
        .ok()
    }
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        super::nasread::read(&self.0.join(name)).ok()
    }
}

/// What's read so far.
#[derive(Default)]
struct Read {
    files: Files,
    /// The last catalog applied.
    at: Option<u64>,
    entries: Vec<Entry>,
    listed: Option<Instant>,
    error: Option<String>,
}

/// The view: what's read, and its reader.
pub struct CatalogsView {
    src: Arc<dyn Source>,
    read: Mutex<Read>,
    reading: AtomicBool,
}

impl std::fmt::Debug for CatalogsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogsView").field("latest", &self.read.lock().unwrap().at).finish()
    }
}

fn number(name: &str) -> Option<(u64, bool)> {
    if let Some(n) = name.strip_suffix(".delta.json.zst") {
        return n.parse().ok().map(|n| (n, false));
    }
    name.strip_suffix(".json.zst")?.parse().ok().map(|n| (n, true))
}

/// A catalog file's JSON parsed as it's decompressed (a whole catalog is some 40 MB of JSON: never
/// held whole).
fn parse<T: serde::de::DeserializeOwned>(b: &[u8]) -> anyhow::Result<T> {
    let r = std::io::BufReader::new(zstd::stream::Decoder::new(b)?);
    Ok(serde_json::from_reader(r)?)
}

impl CatalogsView {
    pub fn new(src: Arc<dyn Source>) -> Arc<Self> {
        Arc::new(CatalogsView { src, read: Mutex::new(Read::default()), reading: AtomicBool::new(false) })
    }

    /// A page's ask: the reader started when it isn't running and the folder wasn't listed in the
    /// last `EVERY`; the catalogs after `after` as read so far.
    pub fn ask(self: &Arc<Self>, after: Option<u64>) -> Value {
        let due = self.read.lock().unwrap().listed.is_none_or(|t| t.elapsed() >= EVERY);
        if due && !self.reading.swap(true, Ordering::SeqCst) {
            let me = self.clone();
            let started = std::thread::Builder::new().name("catalogs-view".into()).spawn(move || {
                me.refresh();
                me.reading.store(false, Ordering::SeqCst);
            });
            if started.is_err() {
                self.reading.store(false, Ordering::SeqCst);
            }
        }
        self.answer(after)
    }

    /// The answer now: `catalogs` after `after` (all with none), the last read (`latest`), whether
    /// the reader is running (`reading`), its last error.
    pub fn answer(&self, after: Option<u64>) -> Value {
        let r = self.read.lock().unwrap();
        let list: Vec<&Entry> = r.entries.iter().filter(|e| after.is_none_or(|a| e.n > a)).collect();
        serde_json::json!({
            "latest": r.at,
            "reading": self.reading.load(Ordering::SeqCst),
            "error": r.error,
            "catalogs": list,
        })
    }

    /// Lists the folder and reads every catalog after the last read, oldest first, adding each
    /// one's entry as it's read.
    pub fn refresh(&self) {
        let Some(names) = self.src.list() else {
            let mut r = self.read.lock().unwrap();
            r.error = Some("the catalogs' folder can't be listed".into());
            r.listed = Some(Instant::now());
            return;
        };
        let mut whole = BTreeSet::new();
        let mut delta = BTreeSet::new();
        for n in names.iter().filter_map(|n| number(n)) {
            if n.1 {
                whole.insert(n.0)
            } else {
                delta.insert(n.0)
            };
        }
        // (The reading is done outside the lock: an answer never waits on the NAS. The files are
        // this thread's alone while it runs.)
        let (mut files, mut at) = {
            let mut r = self.read.lock().unwrap();
            r.listed = Some(Instant::now());
            (std::mem::take(&mut r.files), r.at)
        };
        let todo: Vec<u64> = whole.union(&delta).copied().filter(|n| at.is_none_or(|a| *n > a)).collect();
        let mut error = None;
        for n in todo {
            let mut ch = Change::default();
            let first = at.is_none();
            let got = self.one(n, &whole, &delta, at, &mut files, &mut ch);
            match got {
                Ok(Some((t, was_whole))) => {
                    at = Some(n);
                    let e = entry(n, t, was_whole, first, ch);
                    let mut r = self.read.lock().unwrap();
                    r.entries.push(e);
                    r.at = at;
                }
                // (Neither readable as the next: one written whole later starts again from it.)
                Ok(None) => {}
                Err(e) => {
                    error = Some(format!("catalog {n}: {e:#}"));
                    break;
                }
            }
        }
        let mut r = self.read.lock().unwrap();
        r.files = files;
        r.error = error;
    }

    /// Catalog `n` applied to `files` (as of catalog `at`): its delta when it changes `at`, else
    /// its whole file; its time and whether it was whole. None: neither there to read as the next.
    fn one(
        &self,
        n: u64,
        whole: &BTreeSet<u64>,
        delta: &BTreeSet<u64>,
        at: Option<u64>,
        files: &mut Files,
        ch: &mut Change,
    ) -> anyhow::Result<Option<(u64, bool)>> {
        if at.is_some() && delta.contains(&n) {
            let b =
                self.src.read(&store::catalog::delta_file_name(n)).ok_or_else(|| anyhow::anyhow!("can't be read"))?;
            let d: SlimDelta = parse(&b)?;
            if Some(d.of) == at {
                let created = files.patch(&d.patch, ch)?;
                return Ok(Some((created.as_deref().and_then(unix_of).unwrap_or(0), false)));
            }
        }
        if whole.contains(&n) {
            let b = self.src.read(&store::catalog::file_name(n)).ok_or_else(|| anyhow::anyhow!("can't be read"))?;
            let c: Slim = parse(&b)?;
            files.whole(&c, ch);
            return Ok(Some((unix_of(&c.created).unwrap_or(0), true)));
        }
        Ok(None)
    }
}

fn entry(n: u64, t: u64, whole: bool, first: bool, ch: Change) -> Entry {
    let at = |s: &BTreeSet<Tile>, z: u8| s.iter().filter(|t| t.0 == z).map(|t| t.1).collect::<Vec<u32>>();
    // (A tile dropped and another file of it set in one catalog: published, not dropped.)
    let dropped: BTreeSet<Tile> = ch.dropped.difference(&ch.set).copied().collect();
    Entry {
        n,
        t,
        whole,
        first,
        z3: at(&ch.set, 3),
        z6: at(&ch.set, 6),
        z3_gone: at(&dropped, 3),
        z6_gone: at(&dropped, 6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn zst(v: &Value) -> Vec<u8> {
        let mut e = zstd::stream::Encoder::new(vec![], 3).unwrap();
        e.write_all(&serde_json::to_vec(v).unwrap()).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn tiles_and_times() {
        assert_eq!(tile_of("6/30/50"), Some((6, 50 * 64 + 30)));
        assert_eq!(tile_of("3/1/2"), Some((3, 17)));
        assert_eq!(tile_of("10/337/16"), Some((6, 64 + 21)));
        assert_eq!(tile_of("4/1/1"), None);
        assert_eq!(tile_of("6/64/0"), None);
        assert_eq!(unix_of("2026-10-23T11:07:54Z"), Some(1_792_753_674));
    }

    #[test]
    fn whole_then_deltas() {
        let mem = Arc::new(Mem::default());
        let c1 = serde_json::json!({"fmt": 2, "n": 1, "created": "2026-10-23T00:00:00Z",
            "base": {"6/1/1": "base/a", "6/2/2": "base/b"}, "units": ["6/1/1"], "coverage": {"regions": []},
            "layers": {"terrain": {"lo": {"3/0/0": "t/lo/a"}, "hi": {"6/1/1": "t/hi/a"}, "deep": {"10/320/320": "t/f/a"}}}});
        // 2: one base pack drawn again, a deep terrain pack added (its z6 tile's), one base gone.
        let p2 = serde_json::json!({"o": {"n": {"=": 2}, "created": {"=": "2026-10-23T01:00:00Z"},
            "base": {"o": {"6/1/1": {"=": "base/a2"}, "6/2/2": null}},
            "layers": {"o": {"terrain": {"o": {"deep": {"o": {"10/48/48": {"=": "t/f/b"}}}}}}}}});
        // 3: written whole: the lo pack drawn again, nothing else.
        let mut c3 = c1.clone();
        c3["n"] = 3.into();
        c3["created"] = "2026-10-23T02:00:00Z".into();
        c3["base"] = serde_json::json!({"6/1/1": "base/a2"});
        c3["layers"]["terrain"]["deep"]["10/48/48"] = "t/f/b".into();
        c3["layers"]["terrain"]["lo"]["3/0/0"] = "t/lo/b".into();
        {
            let mut m = mem.0.lock().unwrap();
            m.insert("1.json.zst".into(), zst(&c1));
            m.insert(
                "2.delta.json.zst".into(),
                zst(&serde_json::json!({"fmt": 2, "n": 2, "of": 1, "of_digest": "", "digest": "", "patch": p2})),
            );
            m.insert("3.json.zst".into(), zst(&c3));
            m.insert("junk.txt".into(), vec![]);
        }
        let v = CatalogsView::new(mem.clone());
        v.refresh();
        let a = v.answer(None);
        assert_eq!(a["latest"], 3, "{a}");
        let cs = a["catalogs"].as_array().unwrap();
        assert_eq!(cs.len(), 3);
        assert_eq!(cs[0]["first"], true);
        assert_eq!(cs[0]["z6"], serde_json::json!([65, 130, 1300]));
        assert_eq!(cs[0]["z3"], serde_json::json!([0]));
        assert_eq!(cs[1]["t"], 1_792_717_200);
        assert_eq!(cs[1]["whole"], false);
        assert_eq!(cs[1]["z6"], serde_json::json!([65, 195]));
        assert_eq!(cs[1]["z6_gone"], serde_json::json!([130]));
        assert!(cs[1].get("first").is_none());
        assert_eq!(cs[2]["whole"], true);
        assert_eq!(cs[2]["z3"], serde_json::json!([0]));
        assert_eq!(cs[2]["z6"], serde_json::json!([]));
        assert_eq!(v.answer(Some(2))["catalogs"].as_array().unwrap().len(), 1);

        // A new delta comes: read on the next refresh alone.
        let p4 =
            serde_json::json!({"o": {"created": {"=": "2026-10-23T03:00:00Z"}, "ovdata": {"=": {"3/1/1": "ov/a"}}}});
        mem.0.lock().unwrap().insert(
            "4.delta.json.zst".into(),
            zst(&serde_json::json!({"fmt": 2, "n": 4, "of": 3, "of_digest": "", "digest": "", "patch": p4})),
        );
        v.refresh();
        let a = v.answer(Some(3));
        assert_eq!(
            a["catalogs"],
            serde_json::json!([{"n": 4, "t": 1_792_724_400, "whole": false, "z3": [9], "z6": []}])
        );
    }

    #[test]
    fn a_delta_of_another_base_waits_for_a_whole_one() {
        let mem = Arc::new(Mem::default());
        let c =
            |n: u64| serde_json::json!({"n": n, "created": "2026-10-23T00:00:00Z", "base": {"6/0/0": format!("b{n}")}});
        {
            let mut m = mem.0.lock().unwrap();
            m.insert("5.json.zst".into(), zst(&c(5)));
            m.insert("6.delta.json.zst".into(), zst(&serde_json::json!({"n": 6, "of": 4, "patch": {"o": {}}})));
            m.insert("7.json.zst".into(), zst(&c(7)));
        }
        let v = CatalogsView::new(mem);
        v.refresh();
        let a = v.answer(None);
        let ns: Vec<u64> = a["catalogs"].as_array().unwrap().iter().map(|e| e["n"].as_u64().unwrap()).collect();
        assert_eq!(ns, [5, 7]);
        assert_eq!(a["catalogs"][1]["z6"], serde_json::json!([0]));
    }
}
