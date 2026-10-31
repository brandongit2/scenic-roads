//! GC's queue on the lead (docs/maplog.md §7.2, §7.4; §8 step 7: in shadow). The lead queues each
//! content name as it stops being held: a batch of the map's log frees it (`Batch::freed`), or a
//! save of the records drops it (the manifest's names and the pending uploads'), while the other
//! holder doesn't name it. A name named again (a batch sets it, a records save names it) leaves the
//! queue. A queued name is **due** (`due`) when every client's cursor heard within `CURSOR_TTL_S` is
//! at or past the `seq` it was freed at, `GRACE_S` has passed since, every live lease was granted
//! after it, and neither holder names it now.
//!
//! The queue lives beside the map's log, in the lead's `maplog/` (`FILE`, JSON lines appended and
//! `fsync`ed once per change: one batch, one records save; rewritten whole when it has grown to
//! several times what it holds), so it survives a lead restart. The holders' index (the map's and
//! the records' content names) is memory alone, made again at the open from the log's map and the
//! first records seen. A lead disk restore loses the file: what it held is found again by the
//! orphan sweep (§7.5), step 8's.
//!
//! What the queue has been told was deleted (`deleted`, with the newest lease granted then) is what
//! the merge refuses (`refusal`): a hand-off of a lease granted before the deletion that brings a
//! deleted name may have found the file on the NAS and reused it; its targets are done again.
//!
//! Shadow (step 7): nothing deletes by the queue. The `gc` job, which still deletes by its own
//! rules, is given the queue's due set (`SHADOW_DUE`, written as its job is planned) and checks it
//! against what it removes that day (crate::agent::gc::Report::shadow): the shadow's due set should
//! be a part of today's deletions. Its check's result (`SHADOW_CHECK`) is read back by the next
//! plan, and what today's GC removed or found gone is marked deleted here.

use crate::pool::journal::{Entry, LeaseId};
use crate::pool::records::Records;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use store::map::{Batch, Cursor, Map};

/// The queue's file in the map log's folder.
pub const FILE: &str = "gc.jsonl";
/// The due set handed to the `gc` job (`--shadow-due`), in the map log's folder.
pub const SHADOW_DUE: &str = "gc-shadow.json";
/// The `gc` job's check of it, beside it.
pub const SHADOW_CHECK: &str = "gc-shadow-check.json";
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
    Deleted { deleted: Vec<String>, at: u64, lease: LeaseId },
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

/// The due set handed to the `gc` job, and its check.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowDue {
    /// When it was found (unix s).
    pub at: u64,
    /// The names queued then, and of them the due.
    pub queued: usize,
    pub names: Vec<String>,
}

/// The `gc` job's check of the shadow's due set against what it removed (crate::agent::gc).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowCheck {
    /// The due set's size and when it was found.
    pub due: usize,
    pub due_at: u64,
    /// Due and removed by today's GC (a dry run: would be).
    pub removed: Vec<String>,
    /// Due and not on the NAS (gone before).
    pub gone: Vec<String>,
    /// Due and kept by today's GC, by its reason: `records` (the manifest, the records or a hand-off
    /// not merged name it: the shadow missed a holder), `catalogue` (a catalogue of its window
    /// names it), `young` (younger than its window), `not swept` (in no folder it sweeps).
    pub kept: BTreeMap<String, Vec<String>>,
}

impl ShadowCheck {
    /// Whether the due set is a part of what today's GC removed (or found gone).
    pub fn holds(&self) -> bool {
        self.kept.values().all(Vec::is_empty)
    }

    /// In words, for the log and GC's note.
    pub fn said(&self) -> String {
        let kept: Vec<String> =
            self.kept.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{} {k}", v.len())).collect();
        format!(
            "GC's queue (shadow): {} due, {} removed by today's GC, {} gone already{}",
            self.due,
            self.removed.len(),
            self.gone.len(),
            if kept.is_empty() { String::new() } else { format!(", kept by it: {}", kept.join(", ")) }
        )
    }
}

#[derive(Default)]
struct Inner {
    /// The file and its lines (None: not opened).
    path: Option<PathBuf>,
    file: Option<File>,
    lines: usize,
    queued: HashMap<String, Queued>,
    /// Deleted names: when, and the newest lease granted then.
    deleted: HashMap<String, (u64, LeaseId)>,
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
            .filter_map(|(_, v)| store::map::content_of(v))
            .map(str::to_string)
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
    fn flush(&mut self, now: u64) {
        if self.out.is_empty() {
            return;
        }
        let out = std::mem::take(&mut self.out);
        let Some(path) = self.path.clone() else { return };
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
        if let Err(e) = r {
            eprintln!("gcqueue: can't write {}: {e:#} (held in memory)", path.display());
            self.file = None;
        }
        if self.lines > 4 * (self.queued.len() + self.deleted.len()) + 10_000 {
            self.deleted.retain(|_, (at, _)| now.saturating_sub(*at) < DELETIONS_KEPT_S);
            if let Err(e) = self.rewrite(&path) {
                eprintln!("gcqueue: can't rewrite {}: {e:#}", path.display());
            }
        }
    }

    fn rewrite(&mut self, path: &Path) -> Result<()> {
        let mut b = Vec::new();
        let mut q: Vec<_> = self.queued.iter().collect();
        q.sort_by(|a, b| a.0.cmp(b.0));
        for (n, x) in q {
            serde_json::to_writer(&mut b, &Line::Queued { q: n.clone(), seq: x.seq, at: x.at })?;
            b.push(b'\n');
        }
        let mut by: BTreeMap<(u64, LeaseId), Vec<String>> = BTreeMap::new();
        for (n, k) in &self.deleted {
            by.entry(*k).or_default().push(n.clone());
        }
        for ((at, lease), mut deleted) in by {
            deleted.sort();
            serde_json::to_writer(&mut b, &Line::Deleted { deleted, at, lease })?;
            b.push(b'\n');
        }
        self.file = None;
        store::seglog::write_durable(path, &b)?;
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
                    eprintln!("gcqueue: a torn last line of {} dropped", path.display());
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
            if let Some(n) = store::map::content_of(v).filter(|_| k.starts_with("f/")) {
                g.map.insert(n.to_string());
                g.take(n);
            }
        }
        for n in &b.freed {
            g.map.remove(n);
            g.queue(n, b.seq, now);
        }
        g.flush(now);
    }

    /// The records as saved now (`r`; at `now`): the names they've dropped since the last seen are
    /// queued unless the map names them, those they name now taken. The first seen only start the
    /// index (what was dropped before them is the orphan sweep's, §7.5).
    pub fn records(&self, r: &Records, now: u64) {
        let mut g = self.lock();
        let id = (r.term, r.seq);
        if g.records.as_ref().is_some_and(|(k, _)| *k == id) {
            return;
        }
        let names: HashSet<String> = r.manifest.values().chain(r.pending.keys()).cloned().collect();
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
        let floor = h.cursors.values().filter(|c| h.now.saturating_sub(c.heard) < CURSOR_TTL_S).map(|c| c.seq).min();
        let mut v: Vec<String> = g
            .queued
            .iter()
            .filter(|(n, q)| {
                h.now.saturating_sub(q.at) >= GRACE_S
                    && floor.is_none_or(|s| s >= q.seq)
                    && h.oldest_lease.is_none_or(|t| t > q.at)
                    && !g.held(n)
            })
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }

    /// Names `names` deleted from the NAS at `at`, `lease` the newest granted then.
    pub fn deleted(&self, names: &[String], at: u64, lease: LeaseId) {
        if names.is_empty() {
            return;
        }
        let mut g = self.lock();
        let l = Line::Deleted { deleted: names.to_vec(), at, lease };
        g.line(&l);
        g.apply(l);
        g.flush(at);
    }

    /// Why the merge refuses journal entry `e`: a content name it brings was deleted after its lease
    /// was granted (the deletion's newest lease is it or after it). None: none was.
    pub fn refusal(&self, e: &Entry) -> Option<String> {
        let g = self.lock();
        let h = &e.handoff;
        let n = h
            .changes
            .values()
            .flatten()
            .chain(h.pending.keys())
            .find(|n| g.deleted.get(*n).is_some_and(|(_, l)| *l >= e.lease))?;
        Some(format!(
            "{n} was deleted from the NAS by GC after its lease {} was granted (its job may have found it there): its targets are done again",
            e.lease
        ))
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

#[cfg(test)]
mod tests;
