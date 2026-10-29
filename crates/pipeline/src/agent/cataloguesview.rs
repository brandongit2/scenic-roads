//! The build page's Catalogues map (docs/pool.md §10, task #629): every catalogue on the NAS
//! (`catalog/<n>.json.zst` whole, `<n>.delta.json.zst` as what changed over the one before,
//! store::catalogue) as the map tiles it published, for `/work/catalogs`.
//!
//! A catalogue publishes a tile when a file it names for that tile is new or other than the last
//! catalogue's: a z6 tile by its base pack, road values, hi data, landmark points, its layers' hi
//! packs and the terrain's deep packs under it (z10 keys, their z6 tile's); a z10 tile by its deep
//! packs alone; a z3 tile by its layers' lo packs and area overlays' details. A tile section names a
//! file by its stable name ("layers/terrain/deep/10-259-349"), the same when it's built again; the
//! file itself is the `files` entry under that name (its content-hashed `file`): a tile is published
//! again when either changes (#833 review: a pack built again had kept the tile's older catalogue).
//! Its entry: its number, time (`created`), whether it was written whole, and the z3, z6 and z10
//! tiles it published (`y * 2^z + x`), and those it dropped. The oldest
//! catalogue kept (GC keeps 14 days') publishes every tile it names: `first`, what was there before it
//! isn't known.
//!
//! Read only while a page asks (`ask`): a thread of its own lists the folder (no more often than
//! `EVERY`) and reads the catalogues after the last it read, oldest first, each through the agent's
//! NAS reads (crate::agent::nasread); the answer is what's read so far, never waiting on it. What it
//! keeps: the entries (a few hundred, tens of tiles each) and, to tell a whole catalogue's tiles from
//! the last's, each tile file's name by its key, hashed (some 18 000, about 2 MB): never a catalogue.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The folder is listed again this often at most while a page asks.
pub const EVERY: Duration = Duration::from_secs(20);

/// One catalogue as the map shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Entry {
    pub n: u64,
    /// Its `created`, unix seconds (0: not read).
    pub t: u64,
    /// Written whole (else as a delta).
    pub whole: bool,
    /// The oldest catalogue read: every tile it names counts as published by it.
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
    /// The deep (z10) tiles it published and dropped (left out when none).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub z10: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub z10_gone: Vec<u32>,
}

/// A tile as kept: its zoom (3, 6 or 10) and index.
type Tile = (u8, u32);

/// The tiles a file key ("z/x/y") belongs to: z3 and z6 as they are; a deeper key its z6 tile's,
/// a z10 key its own too; none for another zoom.
fn tiles_of(key: &str) -> Vec<Tile> {
    let mut it = key.split('/').map(|s| s.parse::<u32>().ok());
    let (Some(Some(z)), Some(Some(x)), Some(Some(y))) = (it.next(), it.next(), it.next()) else { return vec![] };
    match z {
        3 | 6 if x < 1 << z && y < 1 << z => vec![(z as u8, (y << z) + x)],
        7..=24 if x < 1 << z && y < 1 << z => {
            let six = (6, ((y >> (z - 6)) << 6) + (x >> (z - 6)));
            if z == 10 {
                vec![six, (10, (y << 10) + x)]
            } else {
                vec![six]
            }
        }
        _ => vec![],
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

/// A whole catalogue, only what's read of it (the rest skipped as it parses).
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
    /// Each stable name's file (`files`: {name: {file, size, fmt}}), its `file` alone read.
    #[serde(default)]
    files: BTreeMap<String, SlimFile>,
}

#[derive(Default, Deserialize)]
struct SlimFile {
    #[serde(default)]
    file: String,
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

/// A delta catalogue's header and patch (store::catalogue::Delta), only what's read of it: the
/// patch's tile sections (the coverage's, megabytes as JSON values, skipped as it parses).
#[derive(Deserialize)]
struct SlimDelta {
    of: u64,
    patch: SlimPatch,
}

/// A catalogue's patch: `{"=": catalogue}` or `{"o": {section: patch}}` (store::catalogue::delta).
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
    files: Option<Value>,
}

/// Every tile file the last catalogue read names: by section ("base", "<layer>.hi"), its key → its
/// tiles, its stable name hashed, and that with its file's hashed (`with_file`); how many files each
/// tile has; and each stable name's file (`files`), both hashed.
#[derive(Default)]
struct Files {
    by: HashMap<String, HashMap<String, (Vec<Tile>, u64, u64)>>,
    count: HashMap<Tile, u32>,
    file: HashMap<u64, u64>,
}

/// A stable name's hash with its file's.
fn with_file(name: u64, file: Option<&u64>) -> u64 {
    name.rotate_left(17) ^ file.copied().unwrap_or(0)
}

/// What one catalogue changed, as it's applied.
#[derive(Default)]
struct Change {
    set: BTreeSet<Tile>,
    dropped: BTreeSet<Tile>,
}

impl Files {
    fn put(&mut self, sec: &str, key: &str, name: &str, ch: &mut Change) {
        let ts = tiles_of(key);
        if ts.is_empty() {
            return;
        }
        let nh = hash(name);
        let h = with_file(nh, self.file.get(&nh));
        let m = self.by.entry(sec.to_string()).or_default();
        match m.insert(key.to_string(), (ts.clone(), nh, h)) {
            Some((_, _, old)) if old == h => {}
            Some(_) => ch.set.extend(ts),
            None => {
                for t in &ts {
                    *self.count.entry(*t).or_default() += 1;
                }
                ch.set.extend(ts);
            }
        }
    }

    fn drop_key(&mut self, sec: &str, key: &str, ch: &mut Change) {
        let Some((ts, _, _)) = self.by.get_mut(sec).and_then(|m| m.remove(key)) else { return };
        for t in ts {
            let c = self.count.entry(t).or_default();
            *c = c.saturating_sub(1);
            if *c == 0 {
                self.count.remove(&t);
                ch.dropped.insert(t);
            }
        }
    }

    /// A stable name's file set (None: gone); its name hashed when that changed it.
    fn set_file(&mut self, name: &str, file: Option<&str>) -> Option<u64> {
        let nh = hash(name);
        let was = match file {
            Some(f) => self.file.insert(nh, hash(f)),
            None => self.file.remove(&nh),
        };
        (was != self.file.get(&nh).copied()).then_some(nh)
    }

    /// `files` as a whole catalogue names it; the names whose file changed.
    fn files_whole<'a>(&mut self, all: impl Iterator<Item = (&'a str, &'a str)>) -> BTreeSet<u64> {
        let mut changed = BTreeSet::new();
        let mut keep = BTreeSet::new();
        for (name, f) in all {
            keep.insert(hash(name));
            changed.extend(self.set_file(name, Some(f)));
        }
        let gone: Vec<u64> = self.file.keys().filter(|k| !keep.contains(*k)).copied().collect();
        for k in gone {
            self.file.remove(&k);
            changed.insert(k);
        }
        changed
    }

    /// A delta's `files` patch applied; the names whose file changed.
    fn files_patch(&mut self, p: &Value) -> anyhow::Result<BTreeSet<u64>> {
        let file_of = |v: &Value| v.get("file").and_then(Value::as_str).map(str::to_string);
        match op(p)? {
            ("=", Value::Object(m)) => {
                let all: Vec<(String, String)> = m.iter().filter_map(|(k, v)| Some((k.clone(), file_of(v)?))).collect();
                Ok(self.files_whole(all.iter().map(|(k, v)| (k.as_str(), v.as_str()))))
            }
            ("o", Value::Object(m)) => {
                let mut changed = BTreeSet::new();
                for (k, v) in m {
                    let f = if v.is_null() {
                        None
                    } else {
                        match op(v)? {
                            ("=", e) => file_of(e),
                            // (An entry patched field by field: its `file` when that's among them.)
                            ("o", e) => match e.get("file").map(op).transpose()? {
                                Some(("=", Value::String(f))) => Some(f.clone()),
                                _ => continue,
                            },
                            _ => anyhow::bail!("files.{k}: not a patch"),
                        }
                    };
                    changed.extend(self.set_file(k, f.as_deref()));
                }
                Ok(changed)
            }
            _ => anyhow::bail!("files: not a patch of a map"),
        }
    }

    /// The tiles naming a name in `changed` published again when their file is another now.
    fn refile(&mut self, changed: &BTreeSet<u64>, ch: &mut Change) {
        if changed.is_empty() {
            return;
        }
        for m in self.by.values_mut() {
            for (ts, nh, h) in m.values_mut() {
                if !changed.contains(nh) {
                    continue;
                }
                let now = with_file(*nh, self.file.get(nh));
                if now != *h {
                    *h = now;
                    ch.set.extend(ts.iter().copied());
                }
            }
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
        let changed = self.files_whole(c.files.iter().map(|(k, f)| (k.as_str(), f.file.as_str())));
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
        self.refile(&changed, ch);
    }

    fn whole_layer(&mut self, name: &str, l: &SlimLayer, ch: &mut Change) {
        for (p, m) in [("lo", &l.lo), ("hi", &l.hi), ("deep", &l.deep)] {
            self.set_all(&format!("{name}.{p}"), m.iter().map(|(k, v)| (k.as_str(), v.as_str())), ch);
        }
    }

    /// A delta's patch applied; the catalogue's `created` if it names one.
    fn patch(&mut self, p: &SlimPatch, ch: &mut Change) -> anyhow::Result<Option<String>> {
        if let Some(c) = &p.whole {
            self.whole(c, ch);
            return Ok(Some(c.created.clone()));
        }
        let Some(o) = &p.o else { anyhow::bail!("not a patch") };
        let changed = match &o.files {
            Some(f) => self.files_patch(f)?,
            None => BTreeSet::new(),
        };
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
        self.refile(&changed, ch);
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

/// Where the catalogues are read: the NAS's folder, or a test's.
pub trait Source: Send + Sync {
    /// The folder's file names; None when it can't be listed.
    fn list(&self) -> Option<Vec<String>>;
    /// A file's bytes; None when it can't be read.
    fn read(&self, name: &str) -> Option<Vec<u8>>;
}

/// The NAS's `catalog/`, read through the agent's NAS reads under their deadline.
pub struct NasCatalogues(pub store::nasfs::NasPath);

impl Source for NasCatalogues {
    fn list(&self) -> Option<Vec<String>> {
        let p = self.0.clone();
        super::nasread::with(&self.0, move || Ok(p.read_dir()?.into_iter().map(|e| e.name).collect())).ok()
    }
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        super::nasread::read(&self.0.join(name)).ok()
    }
}

/// What's read so far.
#[derive(Default)]
struct Read {
    files: Files,
    /// The last catalogue applied.
    at: Option<u64>,
    entries: Vec<Entry>,
    listed: Option<Instant>,
    error: Option<String>,
}

/// The view: what's read, and its reader.
pub struct CataloguesView {
    src: Arc<dyn Source>,
    read: Mutex<Read>,
    reading: AtomicBool,
}

impl std::fmt::Debug for CataloguesView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CataloguesView").field("latest", &self.read.lock().unwrap().at).finish()
    }
}

fn number(name: &str) -> Option<(u64, bool)> {
    if let Some(n) = name.strip_suffix(".delta.json.zst") {
        return n.parse().ok().map(|n| (n, false));
    }
    name.strip_suffix(".json.zst")?.parse().ok().map(|n| (n, true))
}

/// A catalogue file's JSON parsed as it's decompressed (a whole catalogue is some 40 MB of JSON: never
/// held whole).
fn parse<T: serde::de::DeserializeOwned>(b: &[u8]) -> anyhow::Result<T> {
    let r = std::io::BufReader::new(zstd::stream::Decoder::new(b)?);
    Ok(serde_json::from_reader(r)?)
}

impl CataloguesView {
    pub fn new(src: Arc<dyn Source>) -> Arc<Self> {
        Arc::new(CataloguesView { src, read: Mutex::new(Read::default()), reading: AtomicBool::new(false) })
    }

    /// A page's ask: the reader started when it isn't running and the folder wasn't listed in the
    /// last `EVERY`; the catalogues after `after` as read so far.
    pub fn ask(self: &Arc<Self>, after: Option<u64>) -> Value {
        let due = self.read.lock().unwrap().listed.is_none_or(|t| t.elapsed() >= EVERY);
        if due && !self.reading.swap(true, Ordering::SeqCst) {
            let me = self.clone();
            let started = std::thread::Builder::new().name("catalogues-view".into()).spawn(move || {
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

    /// Lists the folder and reads every catalogue after the last read, oldest first, adding each
    /// one's entry as it's read.
    pub fn refresh(&self) {
        let Some(names) = self.src.list() else {
            let mut r = self.read.lock().unwrap();
            r.error = Some("the catalogues' folder can't be listed".into());
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
                    error = Some(format!("catalogue {n}: {e:#}"));
                    break;
                }
            }
        }
        let mut r = self.read.lock().unwrap();
        r.files = files;
        r.error = error;
    }

    /// Catalogue `n` applied to `files` (as of catalogue `at`): its delta when it changes `at`, else
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
                self.src.read(&store::catalogue::delta_file_name(n)).ok_or_else(|| anyhow::anyhow!("can't be read"))?;
            let d: SlimDelta = parse(&b)?;
            if Some(d.of) == at {
                let created = files.patch(&d.patch, ch)?;
                return Ok(Some((created.as_deref().and_then(unix_of).unwrap_or(0), false)));
            }
        }
        if whole.contains(&n) {
            let b = self.src.read(&store::catalogue::file_name(n)).ok_or_else(|| anyhow::anyhow!("can't be read"))?;
            let c: Slim = parse(&b)?;
            files.whole(&c, ch);
            return Ok(Some((unix_of(&c.created).unwrap_or(0), true)));
        }
        Ok(None)
    }
}

fn entry(n: u64, t: u64, whole: bool, first: bool, ch: Change) -> Entry {
    let at = |s: &BTreeSet<Tile>, z: u8| s.iter().filter(|t| t.0 == z).map(|t| t.1).collect::<Vec<u32>>();
    // (A tile dropped and another file of it set in one catalogue: published, not dropped.)
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
        z10: at(&ch.set, 10),
        z10_gone: at(&dropped, 10),
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
        assert_eq!(tiles_of("6/30/50"), [(6, 50 * 64 + 30)]);
        assert_eq!(tiles_of("3/1/2"), [(3, 17)]);
        assert_eq!(tiles_of("10/337/16"), [(6, 64 + 21), (10, 16 * 1024 + 337)]);
        assert_eq!(tiles_of("12/1348/64"), [(6, 64 + 21)]);
        assert!(tiles_of("4/1/1").is_empty());
        assert!(tiles_of("6/64/0").is_empty());
        assert!(tiles_of("10/1024/0").is_empty());
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
        let v = CataloguesView::new(mem.clone());
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
        let v = CataloguesView::new(mem);
        v.refresh();
        let a = v.answer(None);
        let ns: Vec<u64> = a["catalogs"].as_array().unwrap().iter().map(|e| e["n"].as_u64().unwrap()).collect();
        assert_eq!(ns, [5, 7]);
        assert_eq!(a["catalogs"][1]["z6"], serde_json::json!([0]));
    }

    /// A pack built again keeps its stable name in its tile section; only its `files` entry (its
    /// hashed file) changes: the tile is published again by that catalogue, as the NAS's are (#833
    /// review: Ontario's deep packs rebuilt on 27–29 Oct had kept catalogue 514's time).
    #[test]
    fn a_pack_built_again_is_published_again() {
        let mem = Arc::new(Mem::default());
        let deep = "layers/terrain/deep/10-259-349";
        let c1 = serde_json::json!({"n": 1, "created": "2026-10-23T00:00:00Z",
            "base": {"6/16/21": "base/6-16-21"},
            "layers": {"terrain": {"deep": {"10/259/349": deep, "10/271/349": "layers/terrain/deep/10-271-349"}}},
            "files": {deep: {"file": format!("{deep}.aaaa.pack"), "size": 1, "fmt": 1},
                      "layers/terrain/deep/10-271-349": {"file": "layers/terrain/deep/10-271-349.bbbb.pack", "size": 1, "fmt": 1},
                      "base/6-16-21": {"file": "base/6-16-21.cccc.sect", "size": 1, "fmt": 1}}});
        // 2: 10/259/349 built again, its entry written whole (as the catalogue's deltas do).
        let p2 = serde_json::json!({"o": {"created": {"=": "2026-10-23T01:00:00Z"},
            "files": {"o": {deep: {"=": {"file": format!("{deep}.dddd.pack"), "size": 2, "fmt": 1}}}}}});
        // 3: its entry patched field by field; a size alone changing (no new file) publishes nothing.
        let p3 = serde_json::json!({"o": {"created": {"=": "2026-10-23T02:00:00Z"},
            "files": {"o": {deep: {"o": {"file": {"=": format!("{deep}.eeee.pack")}}},
                            "base/6-16-21": {"o": {"size": {"=": 9}}}}}}});
        // 4: whole again with the same files: nothing; 10/271/349's file other: it alone.
        let mut c4 = c1.clone();
        c4["n"] = 4.into();
        c4["created"] = "2026-10-23T03:00:00Z".into();
        c4["files"][deep]["file"] = format!("{deep}.eeee.pack").into();
        c4["files"]["layers/terrain/deep/10-271-349"]["file"] = "layers/terrain/deep/10-271-349.ffff.pack".into();
        {
            let mut m = mem.0.lock().unwrap();
            m.insert("1.json.zst".into(), zst(&c1));
            for (n, p) in [(2, p2), (3, p3)] {
                m.insert(format!("{n}.delta.json.zst"), zst(&serde_json::json!({"n": n, "of": n - 1, "patch": p})));
            }
            m.insert("4.json.zst".into(), zst(&c4));
        }
        let v = CataloguesView::new(mem);
        v.refresh();
        let a = v.answer(None);
        let cs = a["catalogs"].as_array().unwrap();
        assert_eq!(cs.len(), 4, "{a}");
        let (t259, t271, z6) = (349 * 1024 + 259, 349 * 1024 + 271, 21 * 64 + 16);
        assert_eq!(cs[0]["z10"], serde_json::json!([t259, t271]));
        assert_eq!(cs[0]["z6"], serde_json::json!([z6]));
        for c in &cs[1..3] {
            assert_eq!(c["z10"], serde_json::json!([t259]), "{c}");
            assert_eq!(c["z6"], serde_json::json!([z6]), "{c}");
        }
        assert_eq!(cs[3]["z10"], serde_json::json!([t271]));
        assert!(cs[3].get("z10_gone").is_none());
    }
}
