//! The build page's Catalogues map (docs/pool.md §10, task #629; docs/maplog.md §6.6): the map log's
//! batches (store::map::Batch, `seq` the catalogue's number) as the map tiles each published, for
//! `/work/catalogs`.
//!
//! A batch publishes a tile when it sets or deletes a key of that tile in one of the map's tile
//! sections (`s/base/…`, `s/roads/…`, `s/hidata/…`, `s/markdata/…`, `s/ovdata/…`, a layer's
//! `l/<layer>/lo|hi|deep/…`), or sets or deletes the file (`f/<name>`) of a name such a key gives
//! (a pack built again keeps its name; only its file changes: #833 review). A z6 tile by its base
//! pack, road values, hi data, landmark points, its layers' hi packs and the deep packs under it
//! (z10 keys, their z6 tile's); a z10 tile by its deep packs alone; a z3 tile by its layers' lo
//! packs and area overlays' details. Its entry: its number, time (the batch's `at`), and the z3, z6
//! and z10 tiles it published (`y * 2^z + x`), and those no key names any more. A tile's age on the
//! page is the newest entry naming it.
//!
//! It follows the log (`Source`): on the lead, the log's feed in process; on a member, the lead's
//! `GET /map/changes` (and `/map/snapshot` to start). What it keeps, in the agent's folder
//! (`KEPT`), saved once a refresh applied a batch: its entries, the `seq` they reach, each tile
//! section's key with its name (hashed), and every `f/` name with its file (both hashed: a file
//! set again unchanged, its size alone, publishes nothing; the names are what the map's finished
//! tiles are published by, crate::agent::builtview, task #866). Never a catalogue. So it goes on from where it was after a restart, as deep as it has
//! followed the log. A view with nothing kept starts at the map's `seq` with one entry, `first`,
//! publishing every tile the map names (what was there before isn't known); one told to reset (the
//! log no longer holds the batches after its `seq`: a new lead's log) takes the map again and adds
//! no entry for the batches it missed (said in `error` until the next batch).
//!
//! Read while the agent runs (`wake`, each loop and each page's ask): a thread of its own asks for
//! the batches after the last applied, no more often than `EVERY`; the answer is what's applied so
//! far, never waiting on it.

use super::bus::Snap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::map::feed::Answer;
use store::map::{Batch, Map};

/// The log is asked again this often at most.
pub const EVERY: Duration = Duration::from_secs(20);
/// What the view keeps, in the agent's folder.
pub const KEPT: &str = "catalogues-view.json.zst";

/// One batch as the map shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub n: u64,
    /// Its `at`, unix seconds (0: not read).
    pub t: u64,
    /// The view's start: every tile the map named then counts as published by it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub first: bool,
    /// The tiles it published, `y * 2^z + x`, sorted.
    pub z3: Vec<u32>,
    pub z6: Vec<u32>,
    /// The tiles it left unnamed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub z3_gone: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub z6_gone: Vec<u32>,
    /// The deep (z10) tiles it published and dropped (left out when none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub z10: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub z10_gone: Vec<u32>,
}

/// A tile as kept: its zoom (3, 6 or 10) and index.
type Tile = (u8, u32);

/// The tiles a tile key ("z/x/y") belongs to: z3 and z6 as they are; a deeper key its z6 tile's,
/// a z10 key its own too; none for another zoom.
fn tiles_of(key: &str) -> Vec<Tile> {
    let mut it = key.split('/').map(|s| s.parse::<u32>().ok());
    let (Some(Some(z)), Some(Some(x)), Some(Some(y)), None) = (it.next(), it.next(), it.next(), it.next()) else {
        return vec![];
    };
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

/// The tile key of a map entry in a tile section (module doc), if it's one.
fn tile_key(entry: &str) -> Option<&str> {
    if let Some(rest) = entry.strip_prefix("s/") {
        let (sec, key) = rest.split_once('/')?;
        return ["base", "roads", "hidata", "markdata", "ovdata"].contains(&sec).then_some(key);
    }
    let (_layer, rest) = entry.strip_prefix("l/")?.split_once('/')?;
    let (pack, key) = rest.split_once('/')?;
    ["lo", "hi", "deep"].contains(&pack).then_some(key)
}

/// A name as `names` holds it.
pub fn hash(s: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(s.as_bytes())
}

/// RFC 3339 UTC ("2026-10-23T11:07:54Z") as unix seconds.
pub(crate) fn unix_of(s: &str) -> Option<u64> {
    let day = super::backup::parse_day(s.get(..10)?)?;
    let hms = s.get(11..19)?;
    let p = |r: std::ops::Range<usize>| hms.get(r)?.parse::<u64>().ok();
    Some(day + p(0..2)? * 3600 + p(3..5)? * 60 + p(6..8)?)
}

/// The view's state, as kept (`KEPT`).
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    /// The last batch applied (None: not started).
    seq: Option<u64>,
    entries: Vec<Entry>,
    /// Each tile section's entry key, its name hashed.
    keys: HashMap<String, u64>,
    /// Every `f/` name and its file, hashed.
    files: HashMap<u64, u64>,
}

/// The tiles each name is given for, and how many keys name each tile: `Kept::keys` turned about.
#[derive(Default)]
struct Index {
    by_name: HashMap<u64, Vec<Tile>>,
    count: HashMap<Tile, u32>,
}

/// What one batch changed, as it's applied.
#[derive(Default)]
struct Change {
    set: BTreeSet<Tile>,
    dropped: BTreeSet<Tile>,
}

/// The view's state with its index.
#[derive(Default)]
struct State {
    kept: Kept,
    index: Index,
}

impl State {
    fn of(kept: Kept) -> State {
        let mut index = Index::default();
        for (k, nh) in &kept.keys {
            let ts = tile_key(k).map(tiles_of).unwrap_or_default();
            for t in &ts {
                *index.count.entry(*t).or_default() += 1;
            }
            index.by_name.entry(*nh).or_default().extend(ts);
        }
        State { kept, index }
    }

    /// Entry key `k` now names `name` (its tiles published).
    fn link(&mut self, k: &str, name: &str, ch: &mut Change) {
        let Some(key) = tile_key(k) else { return };
        let ts = tiles_of(key);
        let nh = hash(name);
        match self.kept.keys.insert(k.to_string(), nh) {
            Some(old) => self.unname(old, &ts),
            None => {
                for t in &ts {
                    *self.index.count.entry(*t).or_default() += 1;
                }
            }
        }
        self.index.by_name.entry(nh).or_default().extend(ts.iter().copied());
        ch.set.extend(ts);
    }

    /// Entry key `k` gone (its tiles dropped once no key names them).
    fn unlink(&mut self, k: &str, ch: &mut Change) {
        let Some(nh) = self.kept.keys.remove(k) else { return };
        let ts = tile_key(k).map(tiles_of).unwrap_or_default();
        self.unname(nh, &ts);
        for t in ts {
            let c = self.index.count.entry(t).or_default();
            *c = c.saturating_sub(1);
            if *c == 0 {
                self.index.count.remove(&t);
                ch.dropped.insert(t);
            }
        }
    }

    /// Name `nh` no longer given for tiles `ts` (once each).
    fn unname(&mut self, nh: u64, ts: &[Tile]) {
        let Some(v) = self.index.by_name.get_mut(&nh) else { return };
        for t in ts {
            if let Some(i) = v.iter().position(|u| u == t) {
                v.swap_remove(i);
            }
        }
        if v.is_empty() {
            self.index.by_name.remove(&nh);
        }
    }

    /// The map whole: what's kept made again from it (`first`: its entry added).
    fn take_map(&mut self, m: &Map, first: bool) {
        let entries = std::mem::take(&mut self.kept.entries);
        *self = State::default();
        let mut ch = Change::default();
        for (k, v) in m.entries() {
            if let Some(name) = k.strip_prefix("f/") {
                self.kept.files.insert(hash(name), hash(file_of(v)));
            } else if let Some(name) = v.as_str() {
                self.link(k, name, &mut ch);
            }
        }
        self.kept.entries = entries;
        self.kept.seq = Some(m.seq);
        if first {
            ch.dropped.clear();
            self.kept.entries.push(entry(m.seq, unix_of(&m.at).unwrap_or(0), true, ch));
        }
    }

    /// Batch `b` applied: its entry added.
    fn apply(&mut self, b: &Batch) {
        let mut ch = Change::default();
        let mut refiled = BTreeSet::new();
        for k in &b.del {
            match k.strip_prefix("f/") {
                Some(name) => {
                    self.kept.files.remove(&hash(name));
                    refiled.insert(hash(name));
                }
                None => self.unlink(k, &mut ch),
            }
        }
        for (k, v) in &b.set {
            match k.strip_prefix("f/") {
                Some(name) => {
                    let (nh, fh) = (hash(name), hash(file_of(v)));
                    if self.kept.files.insert(nh, fh) != Some(fh) {
                        refiled.insert(nh);
                    }
                }
                None => match v.as_str() {
                    Some(name) => self.link(k, name, &mut ch),
                    None => self.unlink(k, &mut ch),
                },
            }
        }
        // (A name's file changed: the tiles given it published again.)
        for nh in refiled {
            ch.set.extend(self.index.by_name.get(&nh).into_iter().flatten().copied());
        }
        self.kept.seq = Some(b.seq);
        self.kept.entries.push(entry(b.seq, unix_of(&b.at).unwrap_or(0), false, ch));
    }
}

/// An `f/` entry's file (its content name).
fn file_of(v: &Value) -> &str {
    v.get("file").and_then(Value::as_str).unwrap_or_default()
}

fn entry(n: u64, t: u64, first: bool, ch: Change) -> Entry {
    let at = |s: &BTreeSet<Tile>, z: u8| s.iter().filter(|t| t.0 == z).map(|t| t.1).collect::<Vec<u32>>();
    // (A tile dropped and another file of it set in one batch: published, not dropped.)
    let dropped: BTreeSet<Tile> = ch.dropped.difference(&ch.set).copied().collect();
    Entry {
        n,
        t,
        first,
        z3: at(&ch.set, 3),
        z6: at(&ch.set, 6),
        z3_gone: at(&dropped, 3),
        z6_gone: at(&dropped, 6),
        z10: at(&ch.set, 10),
        z10_gone: at(&dropped, 10),
    }
}

/// Where the log is read: the lead's feed in process, the lead's routes from a member, a test's.
pub trait Source: Send + Sync {
    /// The batches after `since` (`GET /map/changes`' answer), or a reset.
    fn changes(&self, since: u64) -> anyhow::Result<Answer>;
    /// The whole map now (`GET /map/snapshot`).
    fn map(&self) -> anyhow::Result<Arc<Map>>;
}

impl Source for store::map::feed::Feed {
    fn changes(&self, since: u64) -> anyhow::Result<Answer> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // (No cursor: a page reads names, never files: docs/maplog.md §5.4.)
        match store::map::feed::Feed::changes(self, since, None, now) {
            store::map::feed::Changes::NotReady => anyhow::bail!("the map's log isn't open yet"),
            store::map::feed::Changes::Reset(head) => Ok(Answer { reset: Some(head), ..Default::default() }),
            store::map::feed::Changes::Batches { body, .. } => Ok(serde_json::from_slice(&body)?),
        }
    }
    fn map(&self) -> anyhow::Result<Arc<Map>> {
        store::map::feed::Feed::map(self).ok_or_else(|| anyhow::anyhow!("the map's log isn't open yet"))
    }
}

/// The lead's `/map/` routes, asked from a member (its contact on the NAS, crate::coord::client).
pub struct Lead {
    root: store::nasfs::NasRoot,
    worker: String,
    client: Arc<Snap<Option<Arc<crate::coord::client::Client>>>>,
}

impl Lead {
    pub fn new(root: &store::nasfs::NasRoot, host: &str) -> Lead {
        Lead { root: root.short_retry(), worker: format!("page {host}"), client: Snap::new(None) }
    }

    fn get(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let c = match &*self.client.get() {
            Some(c) => c.clone(),
            None => {
                let made = crate::coord::client::Client::from_nas(&self.root, &self.worker)?;
                let c = Arc::new(made.ok_or_else(|| anyhow::anyhow!("no lead's coordinator"))?);
                self.client.set(Some(c.clone()));
                c
            }
        };
        c.get_bytes(path)
    }
}

impl Source for Lead {
    fn changes(&self, since: u64) -> anyhow::Result<Answer> {
        Ok(serde_json::from_slice(&self.get(&format!("/map/changes?since={since}"))?)?)
    }
    fn map(&self) -> anyhow::Result<Arc<Map>> {
        Ok(Arc::new(store::map::Snapshot::decode(&self.get("/map/snapshot")?)?.0))
    }
}

/// What the view shows (`answer`, `names`), as its reader last made it.
#[derive(Default)]
struct Shown {
    entries: Vec<Entry>,
    latest: Option<u64>,
    error: Option<String>,
    /// The `f/` names at `latest` (`names`).
    names: Arc<HashSet<u64>>,
}

/// The view: its source, what it shows, and its reader with its state (that thread's alone while it
/// runs, kept between its runs).
pub struct CataloguesView {
    src: Arc<Snap<Arc<dyn Source>>>,
    /// Where it's kept (None: not kept).
    keep: Option<PathBuf>,
    shown: Arc<Snap<Shown>>,
    state: Mutex<Option<State>>,
    /// When the log was last asked (ms since `born`; 0: never).
    asked: AtomicU64,
    born: Instant,
    reading: AtomicBool,
}

impl std::fmt::Debug for CataloguesView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CataloguesView").field("latest", &self.shown.get().latest).finish()
    }
}

/// What's kept at `path`; none when it isn't there or doesn't read (logged).
fn read_kept(path: &std::path::Path) -> Kept {
    let read = std::fs::read(path).map_err(anyhow::Error::from).and_then(|b| {
        let r = std::io::BufReader::new(zstd::stream::Decoder::new(&b[..])?);
        Ok(serde_json::from_reader::<_, Kept>(r)?)
    });
    match read {
        Ok(k) => k,
        Err(e) => {
            if path.exists() {
                eprintln!("agent: the Catalogues map's {}: {e:#}; started again", path.display());
            }
            Kept::default()
        }
    }
}

fn write_kept(path: &std::path::Path, k: &Kept) -> anyhow::Result<()> {
    let mut enc = zstd::stream::Encoder::new(Vec::new(), 3)?;
    serde_json::to_writer(std::io::BufWriter::new(&mut enc), k)?;
    store::whole::write(path, &enc.finish()?)
}

/// `/work/catalogs`' answer from entries `entries` after `after` (all with none).
fn answer_of(entries: &[Entry], latest: Option<u64>, reading: bool, error: Option<&str>, after: Option<u64>) -> Value {
    let list: Vec<&Entry> = entries.iter().filter(|e| after.is_none_or(|a| e.n > a)).collect();
    serde_json::json!({ "latest": latest, "reading": reading, "error": error, "catalogs": list })
}

/// What a reset says until the next batch.
const MISSED: &str = "the log no longer holds the batches after";

impl CataloguesView {
    /// A view of the log `src`, kept at `keep`.
    pub fn new(src: Arc<dyn Source>, keep: Option<PathBuf>) -> Arc<Self> {
        Arc::new(CataloguesView {
            src: Snap::new(src),
            keep,
            shown: Snap::new(Shown::default()),
            state: Mutex::new(None),
            asked: AtomicU64::new(0),
            born: Instant::now(),
            reading: AtomicBool::new(false),
        })
    }

    /// The log read from `src` from now on (this Mac took the lead, or let it go).
    pub fn set_source(&self, src: Arc<dyn Source>) {
        self.src.set(src);
    }

    /// A page's ask: the reader woken (`wake`); the batches after `after` as applied so far.
    pub fn ask(self: &Arc<Self>, after: Option<u64>) -> Value {
        self.wake();
        self.answer(after)
    }

    /// The reader started when it isn't running and the log wasn't asked in the last `EVERY`.
    pub fn wake(self: &Arc<Self>) {
        let asked = self.asked.load(Ordering::SeqCst);
        let now = self.born.elapsed().as_millis() as u64 + 1;
        let due = asked == 0 || now.saturating_sub(asked) >= EVERY.as_millis() as u64;
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
    }

    /// The last batch applied and every `f/` name of the map then (`hash`); None before the first.
    pub fn names(&self) -> Option<(u64, Arc<HashSet<u64>>)> {
        let s = self.shown.get();
        s.latest.map(|n| (n, s.names.clone()))
    }

    /// The answer now: `catalogs` after `after` (all with none), the last applied (`latest`),
    /// whether the reader is running (`reading`), its last error.
    pub fn answer(&self, after: Option<u64>) -> Value {
        let s = self.shown.get();
        answer_of(&s.entries, s.latest, self.reading.load(Ordering::SeqCst), s.error.as_deref(), after)
    }

    /// What's kept at `path` as `/work/catalogs` answers it (`scenic catalogs`: this Mac's agent's
    /// view, read without it).
    pub fn answer_kept(path: &std::path::Path, after: Option<u64>) -> Value {
        let k = read_kept(path);
        answer_of(&k.entries, k.seq, false, None, after)
    }

    /// Asks the log for the batches after the last applied (the map whole to start, or when told to
    /// reset) and applies each in turn; what's kept written, and what's shown made again, when it
    /// moved.
    pub fn refresh(&self) {
        self.asked.store(self.born.elapsed().as_millis() as u64 + 1, Ordering::SeqCst);
        let src = self.src.get();
        // (The state is this thread's alone while it runs: an answer reads what's shown, never
        // waiting on the log.)
        let taken = self.state.lock().unwrap_or_else(|e| e.into_inner()).take();
        let mut st = taken.unwrap_or_else(|| State::of(self.keep.as_deref().map(read_kept).unwrap_or_default()));
        let was = st.kept.seq;
        let mut error = None;
        loop {
            let got = match st.kept.seq {
                None => src.map().map(|m| {
                    st.take_map(&m, true);
                    true
                }),
                Some(since) => src.changes(since).and_then(|a| match a.reset {
                    Some(head) => {
                        let m = src.map()?;
                        error = Some(format!("{MISSED} {since} (now at {head}): those to {} aren't shown", m.seq));
                        st.take_map(&m, false);
                        Ok(true)
                    }
                    None => {
                        for b in a.batches.iter().filter(|b| b.seq > since) {
                            st.apply(b);
                            error = None;
                        }
                        Ok(a.more)
                    }
                }),
            };
            match got {
                Ok(true) => continue,
                Ok(false) => break,
                Err(e) => {
                    error = Some(format!("{e:#}"));
                    break;
                }
            }
        }
        let shown = self.shown.get();
        // (A reset's word kept until a batch comes.)
        let error = error.or_else(|| shown.error.clone().filter(|e| e.starts_with(MISSED) && st.kept.seq == was));
        if st.kept.seq != was {
            if let Some(p) = &self.keep {
                if let Err(e) = write_kept(p, &st.kept) {
                    eprintln!("agent: keeping the Catalogues map: {e:#}");
                }
            }
        }
        if st.kept.seq != shown.latest || error != shown.error {
            let names = if st.kept.seq == shown.latest {
                shown.names.clone()
            } else {
                Arc::new(st.kept.files.keys().copied().collect())
            };
            self.shown.set(Shown { entries: st.kept.entries.clone(), latest: st.kept.seq, error, names });
        }
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

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
        assert_eq!(tile_key("s/base/6/1/2"), Some("6/1/2"));
        assert_eq!(tile_key("l/terrain/deep/10/1/2"), Some("10/1/2"));
        assert_eq!((tile_key("s/global/roaden/6-1-2"), tile_key("l/terrain"), tile_key("u/6/1/2")), (None, None, None));
        assert_eq!(tile_key("l/slope/native_zoom/6/1/2"), None);
    }

    /// The lead's feed as a log writer keeps it (crate::agent::publish), its catalogues given in turn.
    struct Log {
        d: tempfile::TempDir,
        w: super::super::publish::Writer,
        here: Arc<super::super::publish::Here>,
    }

    impl Log {
        fn new() -> Log {
            let d = tempfile::tempdir().unwrap();
            let here = Arc::new(super::super::publish::Here::default());
            let w = super::super::publish::Writer::new(d.path().join("maplog"), here.clone());
            Log { d, w, here }
        }
        fn publish(&mut self, c: &store::catalogue::Catalogue) {
            let root = store::nasfs::NasRoot::local_for_test(self.d.path().join("nas"));
            self.w.publish(&root, Arc::new(c.clone()), false, std::time::SystemTime::now());
            assert_eq!(self.here.feed.head(), Some(c.n), "catalogue {} in the log", c.n);
        }
        fn source(&self) -> Arc<dyn Source> {
            self.here.feed.clone()
        }
    }

    fn cat(n: u64, created: &str, v: Value) -> store::catalogue::Catalogue {
        let mut v = v;
        v["n"] = n.into();
        v["fmt"] = 2.into();
        v["created"] = created.into();
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_view_follows_the_log_and_goes_on_from_what_it_kept() {
        let mut lead = Log::new();
        let f = |n: &str| json!({"file": format!("{n}.a.pack"), "size": 1, "fmt": 1});
        let c1 = cat(
            1,
            "2026-10-23T00:00:00Z",
            json!({"base": {"6/1/1": "base/6-1-1", "6/2/2": "base/6-2-2"}, "units": ["6/1/1"],
            "layers": {"terrain": {"lo": {"3/0/0": "t/lo/a"}, "hi": {"6/1/1": "t/hi/a"}, "deep": {"10/320/320": "t/f/a"}}},
            "files": {"base/6-1-1": f("base/6-1-1"), "base/6-2-2": f("base/6-2-2")}}),
        );
        lead.publish(&c1);
        let keep = lead.d.path().join(KEPT);
        let v = CataloguesView::new(lead.source(), Some(keep.clone()));
        v.refresh();
        let a = v.answer(None);
        assert_eq!(a["latest"], 1, "{a}");
        let cs = a["catalogs"].as_array().unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0]["first"], true);
        assert_eq!(cs[0]["z6"], json!([65, 130, 1300]));
        assert_eq!(cs[0]["z3"], json!([0]));
        // 2: one base pack drawn again, a deep terrain pack added (its z6 tile's), one base gone.
        let mut c2 = c1.clone();
        c2.n = 2;
        c2.created = "2026-10-23T01:00:00Z".into();
        c2.base.insert("6/1/1".into(), "base/6-1-1b".into());
        c2.base.remove("6/2/2");
        c2.layers.get_mut("terrain").unwrap().deep.insert("10/48/48".into(), "t/f/b".into());
        lead.publish(&c2);
        v.refresh();
        let a = v.answer(Some(1));
        assert_eq!(
            a["catalogs"],
            json!([{"n": 2, "t": 1_792_717_200, "z3": [], "z6": [65, 195], "z6_gone": [130], "z10": [48 * 1024 + 48]}])
        );
        // Kept: a view made again (an agent restarted) goes on from it, and reads 3 alone.
        let mut c3 = c2.clone();
        c3.n = 3;
        c3.created = "2026-10-23T02:00:00Z".into();
        c3.ovdata.insert("3/1/1".into(), "ov/a".into());
        lead.publish(&c3);
        let again = CataloguesView::new(lead.source(), Some(keep.clone()));
        again.refresh();
        let a = again.answer(None);
        let ns: Vec<u64> = a["catalogs"].as_array().unwrap().iter().map(|e| e["n"].as_u64().unwrap()).collect();
        assert_eq!(ns, [1, 2, 3]);
        assert_eq!(a["catalogs"][2]["z3"], json!([9]));
        assert_eq!(
            CataloguesView::answer_kept(&keep, Some(2))["catalogs"],
            json!(a["catalogs"].as_array().unwrap()[2..])
        );
        // A new lead's log (seeded later): reset, the map taken again, no entry for what was missed.
        let mut other = Log::new();
        let mut c5 = c3.clone();
        c5.n = 5;
        c5.created = "2026-10-23T04:00:00Z".into();
        other.publish(&c5);
        again.set_source(other.source());
        again.refresh();
        let a = again.answer(None);
        assert_eq!(a["latest"], 5);
        assert_eq!(a["catalogs"].as_array().unwrap().len(), 3);
        assert!(a["error"].as_str().is_some_and(|e| e.contains("after 3")), "{a}");
        // (And goes on with its batches.)
        let mut c6 = c5.clone();
        c6.n = 6;
        c6.created = "2026-10-23T05:00:00Z".into();
        c6.base.insert("6/2/2".into(), "base/6-2-2".into());
        other.publish(&c6);
        again.refresh();
        let a = again.answer(Some(5));
        assert_eq!(a["catalogs"][0]["z6"], json!([130]));
        assert!(a["error"].is_null());
    }

    /// A pack built again keeps its name in its tile section; only its `f/` entry (its file)
    /// changes: the tile is published again by that batch (#833 review: Ontario's deep packs
    /// rebuilt on 27–29 Oct had kept catalogue 514's time). A tile's age (its newest entry) is the
    /// last batch touching it, by its key or its name's file.
    #[test]
    fn published_age_is_the_last_batch_touching_the_tile() {
        let mut lead = Log::new();
        let deep = "layers/terrain/deep/10-259-349";
        let other = "layers/terrain/deep/10-271-349";
        let f = |n: &str, h: &str| json!({"file": format!("{n}.{h}.pack"), "size": 1, "fmt": 1});
        let c1 = cat(
            1,
            "2026-10-23T00:00:00Z",
            json!({"base": {"6/16/21": "base/6-16-21"},
            "layers": {"terrain": {"deep": {"10/259/349": deep, "10/271/349": other}}},
            "files": {deep: f(deep, "a"), other: f(other, "b"), "base/6-16-21": f("base/6-16-21", "c")}}),
        );
        lead.publish(&c1);
        let v = CataloguesView::new(lead.source(), None);
        v.refresh();
        // 2: 10/259/349 built again (its file alone). 3: 10/271/349's size alone changing, no new
        // file: nothing. 4: 10/271/349 built again, and the base pack's name (a key) changes.
        let mut c = c1.clone();
        let steps: Vec<Box<dyn Fn(&mut store::catalogue::Catalogue)>> = vec![
            Box::new(|c| c.files.get_mut(deep).unwrap().file = format!("{deep}.d.pack")),
            Box::new(|c| c.files.get_mut(other).unwrap().size = 9),
            Box::new(|c| {
                c.files.get_mut(other).unwrap().file = format!("{other}.e.pack");
                c.base.insert("6/16/21".into(), "base/6-16-21x".into());
            }),
        ];
        for (i, s) in steps.iter().enumerate() {
            c.n = i as u64 + 2;
            c.created = format!("2026-10-23T0{}:00:00Z", i + 1);
            s(&mut c);
            lead.publish(&c);
        }
        v.refresh();
        let a = v.answer(None);
        let cs: Vec<Entry> = serde_json::from_value(a["catalogs"].clone()).unwrap();
        assert_eq!(cs.len(), 4, "{a}");
        let (t259, t271, z6) = (349 * 1024 + 259, 349 * 1024 + 271, 21 * 64 + 16);
        assert_eq!((cs[1].z10.clone(), cs[1].z6.clone()), (vec![t259], vec![z6]));
        assert_eq!((cs[2].z10.clone(), cs[2].z6.clone()), (vec![], vec![]));
        assert_eq!((cs[3].z10.clone(), cs[3].z6.clone()), (vec![t271], vec![z6]));
        // Each tile's age: its newest entry.
        let age = |z: u8, i: u32| -> u64 {
            cs.iter()
                .rev()
                .find(|e| match z {
                    6 => e.z6.contains(&i),
                    _ => e.z10.contains(&i),
                })
                .map(|e| e.t)
                .unwrap()
        };
        let hour = |h: u64| unix_of("2026-10-23T00:00:00Z").unwrap() + h * 3600;
        assert_eq!((age(10, t259), age(10, t271), age(6, z6)), (hour(1), hour(3), hour(3)));
    }

    /// docs/maplog.md §8, step 4: on the recorded run of real catalogues (store's
    /// `src/map/testdata`, 798–809), the view from the log has the same entries as the catalogues
    /// give, each pair compared by their tile sections' names and those names' files.
    #[test]
    fn catalogues_view_from_the_log_matches_from_catalogues() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../store/src/map/testdata");
        let mut ns = store::catalogue::list(dir.as_path()).unwrap();
        ns.reverse();
        let chain: Vec<store::catalogue::Catalogue> =
            ns.into_iter().map(|n| store::catalogue::read_n(dir.as_path(), n).unwrap()).collect();
        // (Every tile section's key → (its tiles, its name, that name's file).)
        type Named = BTreeMap<String, (Vec<Tile>, String, Option<String>)>;
        let named = |c: &store::catalogue::Catalogue| -> Named {
            let mut out = Named::new();
            let mut put = |sec: String, m: &BTreeMap<String, String>| {
                for (k, name) in m {
                    let file = c.files.get(name).map(|f| f.file.clone());
                    out.insert(format!("{sec}/{k}"), (tiles_of(k), name.clone(), file));
                }
            };
            for (s, m) in [
                ("base", &c.base),
                ("roads", &c.roads),
                ("hidata", &c.hidata),
                ("markdata", &c.markdata),
                ("ovdata", &c.ovdata),
            ] {
                put(s.into(), m);
            }
            for (l, layer) in &c.layers {
                for (p, m) in [("lo", &layer.lo), ("hi", &layer.hi), ("deep", &layer.deep)] {
                    put(format!("{l}.{p}"), m);
                }
            }
            out
        };
        let tiles = |n: &Named| -> BTreeSet<Tile> { n.values().flat_map(|v| v.0.iter().copied()).collect() };
        let from_catalogues = |a: &store::catalogue::Catalogue, b: &store::catalogue::Catalogue| -> Entry {
            let (na, nb) = (named(a), named(b));
            let mut ch = Change::default();
            for (k, v) in &nb {
                if na.get(k).is_none_or(|o| o.1 != v.1 || o.2 != v.2) {
                    ch.set.extend(v.0.iter().copied());
                }
            }
            ch.dropped = tiles(&na).difference(&tiles(&nb)).copied().collect();
            entry(b.n, unix_of(&b.created).unwrap(), false, ch)
        };
        let mut lead = Log::new();
        lead.publish(&chain[0]);
        let v = CataloguesView::new(lead.source(), None);
        v.refresh();
        for c in &chain[1..] {
            lead.publish(c);
        }
        v.refresh();
        let got: Vec<Entry> = serde_json::from_value(v.answer(None)["catalogs"].clone()).unwrap();
        let first = Change { set: tiles(&named(&chain[0])), ..Default::default() };
        let mut want = vec![entry(chain[0].n, unix_of(&chain[0].created).unwrap(), true, first)];
        want.extend(chain.windows(2).map(|w| from_catalogues(&w[0], &w[1])));
        assert_eq!(got.len(), chain.len());
        assert!(got.iter().skip(1).any(|e| !e.z6.is_empty()), "the run publishes tiles");
        for (g, w) in got.iter().zip(&want) {
            assert_eq!(g, w, "catalogue {}", w.n);
        }
    }
}
