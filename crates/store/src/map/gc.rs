//! GC's queue on the lead (docs/maplog.md §7.2 to §7.5), the map log's own store beside it: the
//! content names no longer held, each with the map's `seq` from which no map names it and when it
//! was freed; the holders' index (the map's names, the records', and the names a hand-off being
//! merged brings); the names being removed (told, not yet gone) and the names deleted, each with
//! the newest lease granted then. Told of each batch (`batch`) by the log's writer and of each
//! records save (`records`) by the lead's agent; the `gc` job takes its chunks (`chunk`) and says
//! what went (`gone`) through the lead's coordinator; the merge asks it of each hand-off
//! (`bringing`). Its users and the rules in full: pipeline's `agent::gcqueue`.
//!
//! One file of JSON lines in the log's folder (`FILE`), appended and `fsync`ed once per change,
//! rewritten whole once it has grown to four times what it holds; a last line cut short is dropped.
//! The holders' index is memory alone, made again at the open.

use super::{Batch, Cursor, Map};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    Queued {
        q: String,
        seq: u64,
        at: u64,
    },
    Taken {
        taken: String,
    },
    /// Told and gone, by a `gc` job of before step 8b (each removed or kept as its run decided):
    /// off the queue, marked.
    Deleted {
        deleted: Vec<String>,
        at: u64,
        lease: Lease,
    },
    /// Told: marked (the merge refuses them from then on), being removed; still queued until gone.
    Removing {
        removing: Vec<String>,
        at: u64,
        lease: Lease,
    },
    /// Removed: off the queue, the mark's lease the newest granted when the removal ended.
    Gone {
        gone: Vec<String>,
        at: u64,
        lease: Lease,
    },
    /// Told and not removed (the job yielded first): queued again as they were, unmarked.
    Released {
        released: Vec<String>,
    },
    /// Told again (the job killed, or its gone not heard) and held now: no longer being removed,
    /// but still marked, since the job before may have removed them (the merge goes on refusing
    /// a hand-off of an older lease bringing one).
    Kept {
        kept: Vec<String>,
    },
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

/// Why the merge refuses a hand-off (`GcQueue::bringing`).
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The name was deleted (told) at this time after the hand-off's lease was granted.
    Deleted(String, u64),
    /// The name is being removed now (told, not yet gone).
    Removing(String),
}

#[derive(Default)]
struct Inner {
    /// The file and its lines (None: not opened).
    path: Option<PathBuf>,
    file: Option<File>,
    lines: usize,
    queued: HashMap<String, Queued>,
    /// Told and not yet gone: the chunk in progress, in its order.
    removing: BTreeSet<String>,
    /// Those of them told more than once (a chunk told again: the job before killed, or its gone
    /// not heard), which it may have removed: never released unmarked, only kept.
    retold: HashSet<String>,
    /// Deleted (told) names: when, and the newest lease granted then (or when gone).
    deleted: HashMap<String, (u64, Lease)>,
    /// The map's content names and its `seq`.
    map: HashSet<String>,
    map_seq: u64,
    /// The records' (term, seq) and content names, once seen.
    records: Option<((u64, u64), HashSet<String>)>,
    /// Names a hand-off being merged brings, and when: held until the records after it are seen
    /// (then queued again from that time unless they name them).
    brought: HashMap<String, u64>,
    /// Lines not yet written.
    out: Vec<u8>,
}

/// The queue, shared by the map log's writer (batches), the agent's loop (records saves, the
/// merge's refusals) and the lead's coordinator (the `gc` job's chunks).
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
        self.map.contains(n)
            || self.records.as_ref().is_some_and(|(_, r)| r.contains(n))
            || self.brought.contains_key(n)
    }

    fn line(&mut self, l: &Line) {
        if let Ok(mut b) = serde_json::to_vec(l) {
            b.push(b'\n');
            self.out.extend(b);
        }
    }

    fn queue(&mut self, n: &str, seq: u64, at: u64) {
        if self.held(n) || self.queued.contains_key(n) || self.removing.contains(n) {
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

    /// `l` written (once flushed) and applied.
    fn put(&mut self, l: Line) {
        self.line(&l);
        self.apply(l);
    }

    fn apply(&mut self, l: Line) {
        match l {
            Line::Queued { q, seq, at } => {
                self.queued.insert(q, Queued { seq, at });
            }
            Line::Taken { taken } => {
                self.queued.remove(&taken);
            }
            Line::Deleted { deleted: names, at, lease } | Line::Gone { gone: names, at, lease } => {
                for n in names {
                    self.queued.remove(&n);
                    self.removing.remove(&n);
                    self.retold.remove(&n);
                    self.deleted.insert(n, (at, lease));
                }
            }
            Line::Removing { removing, at, lease } => {
                for n in removing {
                    self.deleted.insert(n.clone(), (at, lease));
                    if !self.removing.insert(n.clone()) {
                        self.retold.insert(n);
                    }
                }
            }
            Line::Released { released } => {
                for n in released {
                    // (One told again may have been removed by the job before: kept, still marked.)
                    if self.removing.remove(&n) && !self.retold.remove(&n) {
                        self.deleted.remove(&n);
                    }
                }
            }
            Line::Kept { kept } => {
                for n in kept {
                    self.removing.remove(&n);
                    self.retold.remove(&n);
                }
            }
        }
    }

    /// The lines made written and `fsync`ed (once per change); the file rewritten whole once it
    /// has grown to four times what it holds, deletions older than `DELETIONS_KEPT_S` let go (but
    /// those being removed). Whether they're on the disk.
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
            let removing = &self.removing;
            self.deleted.retain(|n, (at, _)| removing.contains(n) || now.saturating_sub(*at) < DELETIONS_KEPT_S);
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

    /// Whether `n` is held now, as a tell is answered: a holder names it, or it's queued and
    /// hasn't outlasted its grace, a cursor or a live lease.
    fn holds(&self, n: &str, floor: Option<u64>, h: &Holders) -> bool {
        self.held(n) || self.queued.get(n).is_some_and(|q| !Self::outlasted(q, floor, h))
    }

    /// The names due by the queue's rules, by `take`, sorted (not those being removed).
    fn due_by(&self, h: &Holders, take: &dyn Fn(&str) -> bool) -> Vec<String> {
        if self.records.is_none() {
            return Vec::new();
        }
        let floor = Self::floor(h);
        let mut v: Vec<String> = self
            .queued
            .iter()
            .filter(|(n, q)| !self.removing.contains(*n) && Self::outlasted(q, floor, h) && !self.held(n) && take(n))
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }

    fn rewrite(&mut self, path: &Path) -> Result<()> {
        let mut b = Vec::new();
        let mut q: Vec<_> = self.queued.iter().collect();
        q.sort_by(|a, b| a.0.cmp(b.0));
        for (n, x) in q {
            serde_json::to_writer(&mut b, &Line::Queued { q: n.clone(), seq: x.seq, at: x.at })?;
            b.push(b'\n');
        }
        // (Each mark as the lines that make it: gone, a `Deleted`; being removed, a `Removing`, twice
        // when told again; kept and still queued, a `Removing` and a `Kept`.)
        #[derive(PartialEq, Eq, PartialOrd, Ord)]
        enum Is {
            Gone,
            Removing,
            Retold,
            Kept,
        }
        let mut by: BTreeMap<(Is, u64, Lease), Vec<String>> = BTreeMap::new();
        for (n, (at, lease)) in &self.deleted {
            let is = if self.retold.contains(n) {
                Is::Retold
            } else if self.removing.contains(n) {
                Is::Removing
            } else if self.queued.contains_key(n) {
                Is::Kept
            } else {
                Is::Gone
            };
            by.entry((is, *at, *lease)).or_default().push(n.clone());
        }
        for ((is, at, lease), mut names) in by {
            names.sort();
            let lines = match is {
                Is::Gone => vec![Line::Deleted { deleted: names, at, lease }],
                Is::Removing => vec![Line::Removing { removing: names, at, lease }],
                Is::Retold => vec![
                    Line::Removing { removing: names.clone(), at, lease },
                    Line::Removing { removing: names, at, lease },
                ],
                Is::Kept => vec![Line::Removing { removing: names.clone(), at, lease }, Line::Kept { kept: names }],
            };
            for l in lines {
                serde_json::to_writer(&mut b, &l)?;
                b.push(b'\n');
            }
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
    /// names them, those they name now taken; the names hand-offs brought before them (`bringing`)
    /// that they don't name queued again from when they were brought. The first seen only start
    /// the index (what was dropped before them is the orphan sweep's, §7.5). The same `id` again
    /// changes nothing.
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
        for (n, at) in std::mem::take(&mut g.brought) {
            g.queue(&n, seq, at);
        }
        g.flush(now);
    }

    /// The names due now (sorted): none while the records haven't been seen (a holder unknown),
    /// nor those being removed.
    pub fn due(&self, h: &Holders) -> Vec<String> {
        self.lock().due_by(h, &|_| true)
    }

    /// Those due now that `take` lets a chunk take (the queue's own: crate::agent::gc::by_the_queue
    /// in pipeline), sorted.
    pub fn due_by(&self, h: &Holders, take: &dyn Fn(&str) -> bool) -> Vec<String> {
        self.lock().due_by(h, take)
    }

    /// The `gc` job's next chunk, up to `max` names (§7.3): first those of a chunk told and not
    /// gone (the job killed, or its gone not heard), told again, the rest of the chunk the names
    /// due that `take` lets it take. Each is checked against its holders as of now (the map, the
    /// records, a hand-off being merged, or queued and not outlasted `h`): held, a name of a chunk
    /// before is kept (no longer being removed, taken off the queue when the map or the records
    /// name it, but still marked: the job before may have removed it, so a hand-off of an older
    /// lease bringing it is still refused); the others are marked deleted, `lease` the newest
    /// granted now, on the disk before this answers. An error, nothing marked, while the records
    /// haven't been seen (a holder unknown) or the marks can't be written.
    pub fn chunk(&self, h: &Holders, lease: Lease, max: usize, take: &dyn Fn(&str) -> bool) -> Result<Vec<String>> {
        let mut g = self.lock();
        anyhow::ensure!(g.records.is_some(), "the lead hasn't seen the records yet");
        let floor = Inner::floor(h);
        let (mut out, mut freed) = (Vec::new(), Vec::new());
        for n in g.removing.iter() {
            if out.len() >= max {
                break;
            }
            if g.holds(n, floor, h) {
                freed.push(n.clone());
            } else {
                out.push(n.clone());
            }
        }
        if !freed.is_empty() {
            // (Not released: the job before may have removed them, so their marks stay.)
            g.put(Line::Kept { kept: freed.clone() });
            for n in &freed {
                if g.held(n) {
                    g.take(n);
                }
            }
        }
        let left = max.saturating_sub(out.len());
        out.extend(g.due_by(h, take).into_iter().take(left));
        if !out.is_empty() {
            g.put(Line::Removing { removing: out.clone(), at: h.now, lease });
        }
        anyhow::ensure!(g.flush(h.now), "the marks aren't on the lead's disk");
        Ok(out)
    }

    /// A sweep's `names` about to be removed (by its own rules: the gate's copies): those
    /// something holds as of now answered (the map, the records or a hand-off name them, or
    /// they're queued and haven't outlasted `h`: the sweep's roots are as old as its run), the rest
    /// marked as `chunk` marks them. Errors as `chunk`'s.
    pub fn removing(&self, names: &[String], h: &Holders, lease: Lease) -> Result<Vec<String>> {
        let mut g = self.lock();
        anyhow::ensure!(g.records.is_some(), "the lead hasn't seen the records yet");
        let floor = Inner::floor(h);
        let (held, free): (Vec<String>, Vec<String>) = names.iter().cloned().partition(|n| g.holds(n, floor, h));
        if !free.is_empty() {
            g.put(Line::Removing { removing: free, at: h.now, lease });
        }
        anyhow::ensure!(g.flush(h.now), "the marks aren't on the lead's disk");
        Ok(held)
    }

    /// The job's `gone`, removed (or not there), at `at`: off the queue, each mark's lease
    /// `lease`, the newest granted now (a lease granted while it went refused too); and its
    /// `released`, told and not removed (it yielded first): queued again, unmarked, but those told
    /// more than once, which the job before may have removed, kept marked. An error when
    /// that can't be on the disk (the names then stay being removed: the next chunk takes them
    /// again).
    pub fn gone(&self, gone: &[String], released: &[String], at: u64, lease: Lease) -> Result<()> {
        let mut g = self.lock();
        if !gone.is_empty() {
            g.put(Line::Gone { gone: gone.to_vec(), at, lease });
        }
        let released: Vec<String> = released.iter().filter(|n| g.removing.contains(*n)).cloned().collect();
        if !released.is_empty() {
            g.put(Line::Released { released });
        }
        anyhow::ensure!(g.flush(at), "what went isn't on the lead's disk");
        Ok(())
    }

    /// The orphan sweep's `names` (§7.5: content-named files on the NAS old enough that no job
    /// still holds them by its upload): those nothing holds and not queued queued, freed `now` at
    /// the map's `seq` (so a lease granted before they were found holds them, and every cursor
    /// must reach the map they were found at). How many. An error while the records haven't been
    /// seen.
    pub fn orphans(&self, names: &[String], now: u64) -> Result<usize> {
        let mut g = self.lock();
        anyhow::ensure!(g.records.is_some(), "the lead hasn't seen the records yet");
        let (seq, before) = (g.map_seq, g.queued.len());
        for n in names {
            g.queue(n, seq, now);
        }
        let added = g.queued.len() - before;
        g.flush(now);
        Ok(added)
    }

    /// Hand-off names `names` as the merge takes it (its lease `lease`, at `now`): why it's
    /// refused (a name being removed, or deleted after its lease was granted: its job may have
    /// found the file on the NAS before it went, §7.4); else they're held (off the queue) until
    /// the records after it are seen (`records`), so no chunk takes one meanwhile.
    pub fn bringing<'a>(
        &self,
        names: impl Iterator<Item = &'a String> + Clone,
        lease: Lease,
        now: u64,
    ) -> Option<Refused> {
        let mut g = self.lock();
        for n in names.clone() {
            if g.removing.contains(n) {
                return Some(Refused::Removing(n.clone()));
            }
            if let Some((at, l)) = g.deleted.get(n) {
                if *l >= lease {
                    return Some(Refused::Deleted(n.clone(), *at));
                }
            }
        }
        for n in names {
            g.brought.insert(n.clone(), now);
            g.take(n);
        }
        g.flush(now);
        None
    }

    /// The first of `names` deleted after lease `lease` was granted (the deletion's newest lease is
    /// it or after it), and when.
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

    /// The top folders of what the map and the records name (the orphan sweep's, §7.5).
    pub fn tops(&self) -> BTreeSet<String> {
        let g = self.lock();
        let records = g.records.iter().flat_map(|(_, r)| r.iter());
        g.map.iter().chain(records).filter_map(|n| n.split_once('/').map(|(t, _)| t.to_string())).collect()
    }

    /// The names queued (those being removed among them).
    pub fn len(&self) -> usize {
        self.lock().queued.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The names told and not yet gone.
    pub fn removing_now(&self) -> Vec<String> {
        self.lock().removing.iter().cloned().collect()
    }

    /// When the oldest queued name was freed.
    pub fn oldest(&self) -> Option<u64> {
        self.lock().queued.values().map(|q| q.at).min()
    }

    /// Whether `n` is queued.
    pub fn queued(&self, n: &str) -> Option<Queued> {
        self.lock().queued.get(n).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file rewritten whole keeps every mark as it was: gone, being removed, told again (never
    /// released unmarked), and kept while still queued.
    #[test]
    fn a_rewrite_keeps_every_mark() {
        let d = tempfile::tempdir().unwrap();
        let m = Map::new(1, "t1".into(), Default::default());
        let q = GcQueue::default();
        q.open(d.path(), &m, 100).unwrap();
        q.records((1, 1), std::iter::empty(), 100);
        let names: Vec<String> = ["a", "b", "c", "d", "e"].map(|n| format!("{n}.0000000000000000.pack")).to_vec();
        q.orphans(&names, 100).unwrap();
        let none = BTreeMap::new();
        let h = Holders { cursors: &none, oldest_lease: None, now: 100 + GRACE_S };
        let (l1, l2) = (Lease { term: 1, n: 1 }, Lease { term: 1, n: 2 });
        assert_eq!(q.chunk(&h, l1, 4, &|_| true).unwrap(), names[..4]);
        // (a gone; b, c, d told again, e first told; then c let go (kept: told twice) and d kept.)
        q.gone(&names[..1], &[], 100 + GRACE_S, l1).unwrap();
        assert_eq!(q.chunk(&h, l2, 4, &|_| true).unwrap(), names[1..]);
        q.gone(&[], &names[2..3], 100 + GRACE_S, l2).unwrap();
        {
            let mut g = q.lock();
            g.put(Line::Kept { kept: vec![names[3].clone()] });
            let path = g.path.clone().unwrap();
            g.rewrite(&path).unwrap();
        }
        let back = GcQueue::default();
        back.open(d.path(), &m, 100 + GRACE_S).unwrap();
        let (a, b) = (q.lock(), back.lock());
        assert_eq!(a.removing, b.removing);
        assert_eq!(a.retold, b.retold);
        assert_eq!(a.deleted, b.deleted);
        assert_eq!(a.queued, b.queued);
        assert_eq!(b.removing.iter().cloned().collect::<Vec<_>>(), [names[1].clone(), names[4].clone()]);
        assert!(b.retold.contains(&names[1]) && !b.retold.contains(&names[4]));
        assert!(b.deleted.contains_key(&names[2]) && b.queued.contains_key(&names[2]), "kept, still queued");
    }
}
