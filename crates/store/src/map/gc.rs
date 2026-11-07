//! GC's queue on the lead (docs/maplog.md §7.2, §7.4), the map log's own store beside it: the
//! content names no longer held, each with the map's `seq` from which no map names it and when it
//! was freed; the holders' index (the map's names, the records'); and the names deleted, each with
//! the newest lease granted then. Told of each batch (`batch`) by the log's writer and of each
//! records save (`records`) by the lead's agent, which reads its due set (`due`) and the deletions
//! a hand-off meets (`deleted_under`). Its users and the rules in full: pipeline's
//! `agent::gcqueue`.
//!
//! One file of JSON lines in the log's folder (`FILE`), appended and `fsync`ed once per change,
//! rewritten whole once it has grown to four times what it holds; a last line cut short is dropped.
//! The holders' index is memory alone, made again at the open.

use super::{Batch, Cursor, Map};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The queue's file in the map log's folder.
pub const FILE: &str = "gc.jsonl";
/// How long a freed name waits once no client holds it: a margin for clients outside the cursor
/// set, and for the owner to look at a bad publish before its files go (PM's decision 2, 30 Oct).
pub const GRACE_S: u64 = 2 * 86400;
/// A client unheard this long no longer holds anything: a laptop away a month doesn't hold GC; on
/// its return it resets from the snapshot (PM's decision 3, 30 Oct; docs/maplog.md §5.4).
pub const CURSOR_TTL_S: u64 = 7 * 86400;
/// How long a deletion is kept for the merge's refusal: as long as a client is kept, the same
/// bound on a member's absence (a hand-off told later than that, of a lease granted before the
/// deletion, isn't refused; its files' absence is the pending uploads' verification's to find).
pub const DELETIONS_KEPT_S: u64 = CURSOR_TTL_S;

/// A lease's id as the pool numbers them (pipeline's `pool::journal::LeaseId`): its term, then its
/// number there; later grants are greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Lease {
    pub term: u64,
    pub n: u64,
}

/// A queued name: the map's `seq` from which no map names it, and when it was freed (unix s).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    pub seq: u64,
    pub at: u64,
}

/// A line of the file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Line {
    Queued { q: String, seq: u64, at: u64 },
    Taken { taken: String },
    Deleted { deleted: Vec<String>, at: u64, lease: Lease },
}

/// What holds a name besides the map and the records, as `due` is asked.
pub struct Holders<'a> {
    /// The clients' cursors (store::map::feed::Feed::cursors).
    pub cursors: &'a BTreeMap<String, Cursor>,
    /// When the oldest live lease was granted (None: none live).
    pub oldest_lease: Option<u64>,
    /// Now, unix seconds.
    pub now: u64,
}

#[derive(Default)]
struct Inner {
    /// The file and its lines (None: not opened).
    path: Option<PathBuf>,
    file: Option<File>,
    lines: usize,
    queued: HashMap<String, Queued>,
    /// Deleted names: when, and the newest lease granted then.
    deleted: HashMap<String, (u64, Lease)>,
    /// The map's content names and its `seq`.
    map: HashSet<String>,
    map_seq: u64,
    /// The records' (term, seq) and content names, once seen.
    records: Option<((u64, u64), HashSet<String>)>,
    /// Lines not yet written.
    out: Vec<u8>,
}

/// The queue, shared by the map log's writer (batches) and the agent's loop (records saves, the
/// plan's due set, the merge's refusals).
#[derive(Default)]
pub struct GcQueue {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for GcQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GcQueue({} queued)", self.len())
    }
}

impl Inner {
    fn start(&mut self, map: &Map) {
        self.map = map
            .entries()
            .iter()
            .filter(|(k, _)| k.starts_with("f/"))
            .filter_map(|(_, v)| super::content_of(v))
            .collect();
        self.map_seq = map.seq;
    }

    fn held(&self, n: &str) -> bool {
        self.map.contains(n) || self.records.as_ref().is_some_and(|(_, r)| r.contains(n))
    }

    fn line(&mut self, l: &Line) {
        if let Ok(mut b) = serde_json::to_vec(l) {
            b.push(b'\n');
            self.out.extend(b);
        }
    }

    fn queue(&mut self, n: &str, seq: u64, at: u64) {
        if self.held(n) || self.queued.contains_key(n) {
            return;
        }
        self.queued.insert(n.to_string(), Queued { seq, at });
        self.line(&Line::Queued { q: n.to_string(), seq, at });
    }

    fn take(&mut self, n: &str) {
        if self.queued.remove(n).is_some() {
            self.line(&Line::Taken { taken: n.to_string() });
        }
    }

    fn apply(&mut self, l: Line) {
        match l {
            Line::Queued { q, seq, at } => {
                self.queued.insert(q, Queued { seq, at });
            }
            Line::Taken { taken } => {
                self.queued.remove(&taken);
            }
            Line::Deleted { deleted, at, lease } => {
                for n in deleted {
                    self.queued.remove(&n);
                    self.deleted.insert(n, (at, lease));
                }
            }
        }
    }

    /// The lines made written and `fsync`ed (once per change); the file rewritten whole once it
    /// has grown to four times what it holds, deletions older than `DELETIONS_KEPT_S` let go.
    /// Whether they're on the disk.
    fn flush(&mut self, now: u64) -> bool {
        if self.out.is_empty() {
            return true;
        }
        let out = std::mem::take(&mut self.out);
        let Some(path) = self.path.clone() else { return false };
        self.lines += out.iter().filter(|&&c| c == b'\n').count();
        let r = (|| -> Result<()> {
            let f = match self.file.as_mut() {
                Some(f) => f,
                None => self.file.insert(OpenOptions::new().create(true).append(true).open(&path)?),
            };
            f.write_all(&out)?;
            f.sync_data()?;
            Ok(())
        })();
        let written = r.is_ok();
        if let Err(e) = r {
            eprintln!("maplog: GC's queue: can't write {}: {e:#} (held in memory)", path.display());
            self.file = None;
        }
        if self.lines > 4 * (self.queued.len() + self.deleted.len()) + 10_000 {
            self.deleted.retain(|_, (at, _)| now.saturating_sub(*at) < DELETIONS_KEPT_S);
            if let Err(e) = self.rewrite(&path) {
                eprintln!("maplog: GC's queue: can't rewrite {}: {e:#}", path.display());
            }
        }
        written
    }

    /// The oldest `seq` a client heard within `CURSOR_TTL_S` is at.
    fn floor(h: &Holders) -> Option<u64> {
        h.cursors.values().filter(|c| h.now.saturating_sub(c.heard) < CURSOR_TTL_S).map(|c| c.seq).min()
    }

    /// Whether queued `q` has outlasted its holders: `GRACE_S`, every cursor (`floor`), every live
    /// lease.
    fn outlasted(q: &Queued, floor: Option<u64>, h: &Holders) -> bool {
        h.now.saturating_sub(q.at) >= GRACE_S
            && floor.is_none_or(|s| s >= q.seq)
            && h.oldest_lease.is_none_or(|t| t > q.at)
    }

    fn rewrite(&mut self, path: &Path) -> Result<()> {
        let mut b = Vec::new();
        let mut q: Vec<_> = self.queued.iter().collect();
        q.sort_by(|a, b| a.0.cmp(b.0));
        for (n, x) in q {
            serde_json::to_writer(&mut b, &Line::Queued { q: n.clone(), seq: x.seq, at: x.at })?;
            b.push(b'\n');
        }
        let mut by: BTreeMap<(u64, Lease), Vec<String>> = BTreeMap::new();
        for (n, k) in &self.deleted {
            by.entry(*k).or_default().push(n.clone());
        }
        for ((at, lease), mut deleted) in by {
            deleted.sort();
            serde_json::to_writer(&mut b, &Line::Deleted { deleted, at, lease })?;
            b.push(b'\n');
        }
        self.file = None;
        crate::seglog::write_durable(path, &b)?;
        self.lines = b.iter().filter(|&&c| c == b'\n').count();
        Ok(())
    }
}

impl GcQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens the queue in `dir` (the map log's folder), its holders' index started from `map`.
    /// What was queued in memory before (a records save seen first) is kept and written. A last
    /// line cut short (a write stopped midway) is dropped; another that doesn't read is an error.
    pub fn open(&self, dir: &Path, map: &Map, now: u64) -> Result<()> {
        let path = dir.join(FILE);
        let text = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        let mut lines = Vec::new();
        let parts: Vec<&[u8]> = text.split(|&c| c == b'\n').collect();
        let last = parts.len().saturating_sub(1);
        let mut good = 0;
        for (i, l) in parts.iter().enumerate() {
            if i == last {
                // (A last line with no newline: a write cut short.)
                if !l.is_empty() {
                    eprintln!("maplog: GC's queue: a torn last line of {} dropped", path.display());
                }
                break;
            }
            if !l.is_empty() {
                lines.push(
                    serde_json::from_slice::<Line>(l).with_context(|| format!("{}: line {}", path.display(), i + 1))?,
                );
            }
            good += l.len() + 1;
        }
        if good < text.len() {
            OpenOptions::new().write(true).open(&path)?.set_len(good as u64)?;
        }
        let mut g = self.lock();
        let before = std::mem::take(&mut g.queued);
        g.lines = lines.len();
        for l in lines {
            g.apply(l);
        }
        g.start(map);
        g.path = Some(path);
        g.file = None;
        for (n, q) in before {
            g.queue(&n, q.seq, q.at);
        }
        // (A name the map or the records name now leaves the queue: taken while the lead was down.)
        let held: Vec<String> = g.queued.keys().filter(|n| g.held(n)).cloned().collect();
        for n in held {
            g.take(&n);
        }
        g.flush(now);
        Ok(())
    }

    /// The map's log started at `map` (seeded): its names the map's holders.
    pub fn start(&self, map: &Map) {
        self.lock().start(map);
    }

    /// Batch `b` appended to the map's log (at `now`): the names it sets taken, those it frees
    /// queued unless the records name them.
    pub fn batch(&self, b: &Batch, now: u64) {
        let mut g = self.lock();
        g.map_seq = b.seq;
        for (k, v) in &b.set {
            if let Some(n) = k.starts_with("f/").then(|| super::content_of(v)).flatten() {
                g.take(&n);
                g.map.insert(n);
            }
        }
        for n in &b.freed {
            g.map.remove(n);
            g.queue(n, b.seq, now);
        }
        g.flush(now);
    }

    /// The records as saved now (`id`: their term and number; `names`: the content names they
    /// hold; at `now`): the names they've dropped since the last seen are queued unless the map
    /// names them, those they name now taken. The first seen only start the index (what was dropped
    /// before them is the orphan sweep's, §7.5). The same `id` again changes nothing.
    pub fn records<'a>(&self, id: (u64, u64), names: impl Iterator<Item = &'a String>, now: u64) {
        let mut g = self.lock();
        if g.records.as_ref().is_some_and(|(k, _)| *k == id) {
            return;
        }
        let names: HashSet<String> = names.cloned().collect();
        let was = g.records.replace((id, names)).map(|(_, w)| w);
        let (added, dropped): (Vec<String>, Vec<String>) = {
            let now_names = &g.records.as_ref().expect("set above").1;
            match &was {
                Some(w) => (
                    now_names.iter().filter(|n| !w.contains(*n)).cloned().collect(),
                    w.iter().filter(|n| !now_names.contains(*n)).cloned().collect(),
                ),
                None => (g.queued.keys().filter(|n| now_names.contains(*n)).cloned().collect(), Vec::new()),
            }
        };
        for n in &added {
            g.take(n);
        }
        let seq = g.map_seq;
        for n in &dropped {
            g.queue(n, seq, now);
        }
        g.flush(now);
    }

    /// The names due now (sorted): none while the records haven't been seen (a holder unknown).
    pub fn due(&self, h: &Holders) -> Vec<String> {
        let g = self.lock();
        if g.records.is_none() {
            return Vec::new();
        }
        let floor = Inner::floor(h);
        let mut v: Vec<String> = g
            .queued
            .iter()
            .filter(|(n, q)| Inner::outlasted(q, floor, h) && !g.held(n))
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }

    /// Names `names` deleted from the NAS at `at`, `lease` the newest granted then.
    pub fn deleted(&self, names: &[String], at: u64, lease: Lease) {
        if names.is_empty() {
            return;
        }
        let mut g = self.lock();
        let l = Line::Deleted { deleted: names.to_vec(), at, lease };
        g.line(&l);
        g.apply(l);
        g.flush(at);
    }

    /// The `gc` job's `names`, about to be removed: those something holds as of now answered (the
    /// map or the records name them, or they're queued and haven't outlasted `h`: the job's roots
    /// are as old as its run, hours on a slow share, and the holders are here), the rest marked
    /// deleted, `lease` the newest granted now, on the disk before this answers. An error, the job
    /// removing nothing more, while the records haven't been seen (a holder unknown) or the marks
    /// can't be written.
    pub fn removing(&self, names: &[String], h: &Holders, lease: Lease) -> Result<Vec<String>> {
        let mut g = self.lock();
        anyhow::ensure!(g.records.is_some(), "the lead hasn't seen the records yet");
        let floor = Inner::floor(h);
        let (held, free): (Vec<String>, Vec<String>) = names
            .iter()
            .cloned()
            .partition(|n| g.held(n) || g.queued.get(n).is_some_and(|q| !Inner::outlasted(q, floor, h)));
        if !free.is_empty() {
            let l = Line::Deleted { deleted: free, at: h.now, lease };
            g.line(&l);
            g.apply(l);
            anyhow::ensure!(g.flush(h.now), "the marks aren't on the lead's disk");
        }
        Ok(held)
    }

    /// The first of `names` deleted after lease `lease` was granted (the deletion's newest lease is
    /// it or after it), and when: what the merge refuses a hand-off for (§7.4).
    pub fn deleted_under<'a>(&self, names: impl Iterator<Item = &'a String>, lease: Lease) -> Option<(String, u64)> {
        let g = self.lock();
        for n in names {
            if let Some((at, l)) = g.deleted.get(n) {
                if *l >= lease {
                    return Some((n.clone(), *at));
                }
            }
        }
        None
    }

    /// The map's content names, as the queue holds them (sorted).
    pub fn map_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.lock().map.iter().cloned().collect();
        v.sort();
        v
    }

    /// The names queued, each with when it was freed (sorted by name).
    pub fn freed(&self) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = self.lock().queued.iter().map(|(n, q)| (n.clone(), q.at)).collect();
        v.sort();
        v
    }

    /// The names queued, and the oldest one's time freed.
    pub fn len(&self) -> usize {
        self.lock().queued.len()
    }

    pub fn oldest(&self) -> Option<u64> {
        self.lock().queued.values().map(|q| q.at).min()
    }

    /// Whether `n` is queued.
    pub fn queued(&self, n: &str) -> Option<Queued> {
        self.lock().queued.get(n).copied()
    }
}
