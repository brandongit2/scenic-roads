//! The pool's store on the lead's disk (docs/coordination.md §4; task #878): the coordinator's state
//! as keyed JSON documents, a snapshot written whole plus an append-only log after it, both
//! crate::store::seglog's (shared with the map's log).
//!
//! - **One writer thread** owns the files. A caller `submit`s a change (documents set and removed),
//!   applied at once to the documents in memory and numbered (`seq`); the writer appends every
//!   change waiting when it wakes with one `fsync` (a burst of grants is one), then says it's
//!   durable (`wait`). A handler answers only once its change is (§2, principle 5): past
//!   `DURABLE_WAIT`, 503.
//! - **Compaction** (§4.2): past `COMPACT_BYTES` of log or `COMPACT_AGE`, the writer writes the
//!   snapshot at the last change and starts a new segment; the segments it covers go only after
//!   the snapshot's rename and folder `fsync`. A crash between leaves a log overlapping the
//!   snapshot: replay skips what it holds. A failed append is followed by a compaction too, so a
//!   change in memory is never left out of the files.
//! - **The epoch** (§4.3): bumped and made durable as the store opens, before anything is answered.
//! - **Recovery** (§4.4): opening loads the snapshot and replays the log; a torn last line (a crash
//!   mid-append: never answered) is cut. `restore` rebuilds a lost store from the NAS backup, its
//!   epoch past the backup's by `RESTORE_EPOCHS`.
//! - **The backup** (`back_up`): the snapshot written whole to the NAS (`BACKUP_PATH`) each
//!   `BACKUP_EVERY` while it changed, off the writer's thread and every handler's.

use crate::pool::nas::Nas;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use store::seglog::{self, Appender};

/// The documents: key → value.
pub type Docs = BTreeMap<String, Value>;

pub const SNAPSHOT: &str = "snapshot.json";
/// A compaction past this much log, or this long since the last (§4.2).
pub const COMPACT_BYTES: u64 = 10 << 20;
pub const COMPACT_AGE: Duration = Duration::from_secs(3600);
/// A handler waits this long for its change to be durable; past it, 503 (§4.1).
pub const DURABLE_WAIT: Duration = Duration::from_secs(2);
/// The store's backup on the NAS (§3.2), and how often it's written while the store changes.
pub const BACKUP_PATH: &str = "state/lead/store.json";
pub const BACKUP_EVERY: Duration = if cfg!(test) { Duration::from_millis(200) } else { Duration::from_secs(600) };
/// A store restored from the backup starts this many epochs past it: no lease id repeats (§4.4).
pub const RESTORE_EPOCHS: u64 = 1000;

/// The whole state at a `seq` (the snapshot's file, and the backup's, which also names its writer
/// and when).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub epoch: u64,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub member: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub at: u64,
    #[serde(default)]
    pub docs: Docs,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// One change, a line of the log.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Line {
    seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Docs::is_empty")]
    set: Docs,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    del: Vec<String>,
}

impl Snapshot {
    fn apply(&mut self, l: &Line) {
        if l.seq <= self.seq {
            return;
        }
        self.seq = l.seq;
        if let Some(e) = l.epoch {
            self.epoch = e;
        }
        for k in &l.del {
            self.docs.remove(k);
        }
        self.docs.extend(l.set.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
}

#[derive(Debug)]
struct Inner {
    /// The state with every change submitted (`state.seq`: the last).
    state: Snapshot,
    /// Lines submitted and not yet written, and the last of them.
    pending: Vec<u8>,
    pending_n: u64,
    /// The last change on disk.
    durable: u64,
    /// The next write is a compaction (due, or an append failed).
    compact: bool,
    /// Why the last write failed (None: it landed).
    failed: Option<String>,
    stop: bool,
    /// Snapshots written (the tests' measure).
    compactions: u64,
}

/// The store: its state in memory and its writer.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    m: Mutex<Inner>,
    /// Wakes the writer.
    work: Condvar,
    /// Wakes those waiting for `durable`.
    done: Condvar,
    /// The writer's pace (a test's lower).
    compact_bytes: u64,
    compact_age: Duration,
}

impl Store {
    /// The store in `dir` (made, empty, if missing): its snapshot and log replayed, its epoch bumped
    /// and durable, its writer started.
    pub fn open(dir: &Path) -> Result<Arc<Store>> {
        Store::open_with(dir, COMPACT_BYTES, COMPACT_AGE)
    }

    pub fn open_with(dir: &Path, compact_bytes: u64, compact_age: Duration) -> Result<Arc<Store>> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let mut state: Snapshot = match std::fs::read(dir.join(SNAPSHOT)) {
            Ok(b) => serde_json::from_slice(&b).with_context(|| format!("{}", dir.join(SNAPSHOT).display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Snapshot::default(),
            Err(e) => return Err(e).with_context(|| format!("read {}", dir.join(SNAPSHOT).display())),
        };
        let open = seglog::read::<Line>(dir, "pool store", |l| {
            state.apply(&l);
            Ok(())
        })?;
        // (The epoch bumped, durable before anything is answered: a new segment when there's none.)
        let bump = Line { seq: state.seq + 1, epoch: Some(state.epoch + 1), ..Default::default() };
        let mut open = match open {
            Some(a) if !a.torn => a,
            _ => Appender::create(dir, bump.seq)?,
        };
        open.append(&line_bytes(&bump)?, 1)?;
        state.apply(&bump);
        let durable = state.seq;
        let s = Arc::new(Store {
            dir: dir.to_owned(),
            m: Mutex::new(Inner {
                state,
                pending: Vec::new(),
                pending_n: 0,
                durable,
                compact: false,
                failed: None,
                stop: false,
                compactions: 0,
            }),
            work: Condvar::new(),
            done: Condvar::new(),
            compact_bytes,
            compact_age,
        });
        let w = Arc::downgrade(&s);
        std::thread::Builder::new()
            .name("pool-store".into())
            .spawn(move || write_loop(w, open))
            .context("start the pool store's writer")?;
        Ok(s)
    }

    /// Rebuilds the store in `dir` from `backup` (the NAS's, `BACKUP_PATH`: `scenic lead restore`),
    /// in place of whatever is there, its epoch `RESTORE_EPOCHS` past the backup's; then opens it.
    pub fn restore(dir: &Path, backup: &Snapshot) -> Result<Arc<Store>> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let snap = Snapshot { epoch: backup.epoch + RESTORE_EPOCHS, member: String::new(), at: 0, ..backup.clone() };
        seglog::write_durable(&dir.join(SNAPSHOT), &serde_json::to_vec(&snap)?)?;
        for first in seglog::segments(dir)? {
            std::fs::remove_file(dir.join(seglog::segment_name(first)))?;
        }
        Store::open(dir)
    }

    /// Whether `dir` holds a store (a snapshot or a log).
    pub fn exists(dir: &Path) -> bool {
        dir.join(SNAPSHOT).exists() || seglog::segments(dir).is_ok_and(|s| !s.is_empty())
    }

    /// Sets `set` and removes `del`, at once in memory and on disk by the writer: its `seq` (0:
    /// nothing changed), for `wait`.
    pub fn submit(&self, set: Docs, del: Vec<String>) -> u64 {
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let del: Vec<String> = del.into_iter().filter(|k| m.state.docs.contains_key(k)).collect();
        let set: Docs = set.into_iter().filter(|(k, v)| m.state.docs.get(k) != Some(v)).collect();
        if set.is_empty() && del.is_empty() {
            return 0;
        }
        let l = Line { seq: m.state.seq + 1, epoch: None, set, del };
        match line_bytes(&l) {
            Ok(b) => m.pending.extend_from_slice(&b),
            // (Never: documents are JSON values. Written whole by the next compaction.)
            Err(_) => m.compact = true,
        }
        m.pending_n += 1;
        m.state.apply(&l);
        self.work.notify_all();
        l.seq
    }

    /// Waits until change `seq` is on disk, at most `within`: false past it (or stopped).
    pub fn wait(&self, seq: u64, within: Duration) -> bool {
        let until = Instant::now() + within;
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        while m.durable < seq {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || m.stop {
                return false;
            }
            m = self.done.wait_timeout(m, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        true
    }

    /// The last change submitted.
    pub fn last(&self) -> u64 {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).state.seq
    }

    pub fn epoch(&self) -> u64 {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).state.epoch
    }

    /// The state with every change submitted.
    pub fn snapshot(&self) -> Snapshot {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).state.clone()
    }

    /// Why the last write failed, while it did (the status's: a lead's disk that can't take its
    /// store is broken).
    pub fn failing(&self) -> Option<String> {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).failed.clone()
    }

    pub fn compactions(&self) -> u64 {
        self.m.lock().unwrap_or_else(|e| e.into_inner()).compactions
    }

    /// Writes what's pending and stops the writer (what's submitted after is in memory alone).
    pub fn stop(&self) {
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let until = Instant::now() + DURABLE_WAIT;
        while m.durable < m.state.seq && m.failed.is_none() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            m = self.done.wait_timeout(m, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        m.stop = true;
        self.work.notify_all();
        self.done.notify_all();
    }

    /// A test's kill: the writer stopped at once, what's pending never written (a crash before the
    /// `fsync`).
    #[cfg(test)]
    pub fn kill(&self) {
        let mut m = self.m.lock().unwrap_or_else(|e| e.into_inner());
        m.stop = true;
        (m.pending, m.pending_n) = (Vec::new(), 0);
        self.work.notify_all();
        self.done.notify_all();
    }
}

fn line_bytes(l: &Line) -> Result<Vec<u8>> {
    let mut b = serde_json::to_vec(l)?;
    b.push(b'\n');
    Ok(b)
}

fn write_loop(store: std::sync::Weak<Store>, mut open: Appender) {
    let mut since = Instant::now();
    loop {
        let Some(s) = store.upgrade() else { return };
        // What's pending, or the snapshot to write in its place.
        let (bytes, n, last, snap) = {
            let mut m = s.m.lock().unwrap_or_else(|e| e.into_inner());
            while !m.stop && m.pending_n == 0 && !m.compact {
                m = s.work.wait_timeout(m, Duration::from_secs(60)).unwrap_or_else(|e| e.into_inner()).0;
                if since.elapsed() >= s.compact_age && m.durable > open.first() {
                    m.compact = true;
                }
            }
            if m.stop {
                return;
            }
            let due = m.compact || open.bytes >= s.compact_bytes || since.elapsed() >= s.compact_age;
            let (bytes, n) = (std::mem::take(&mut m.pending), std::mem::take(&mut m.pending_n));
            (bytes, n, m.state.seq, due.then(|| m.state.clone()))
        };
        let compacted = snap.is_some();
        let wrote = match snap {
            // (The snapshot holds every change pending: their lines aren't written.)
            Some(snap) => compact(&s.dir, &snap).map(|a| {
                open = a;
                since = Instant::now();
            }),
            None => open.append(&bytes, n),
        };
        let mut m = s.m.lock().unwrap_or_else(|e| e.into_inner());
        match wrote {
            Ok(()) => {
                if compacted {
                    m.compactions += 1;
                }
                m.compact = false;
                m.failed = None;
                m.durable = last;
            }
            Err(e) => {
                if m.failed.is_none() {
                    eprintln!("pool store: {e:#} (written whole at the next try)");
                }
                m.failed = Some(format!("{e:#}"));
                m.compact = true;
            }
        }
        s.done.notify_all();
        let retry = m.failed.is_some();
        drop(m);
        drop(s);
        if retry {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Writes `snap` whole, starts the segment after it, and removes those it covers.
fn compact(dir: &Path, snap: &Snapshot) -> Result<Appender> {
    seglog::write_durable(&dir.join(SNAPSHOT), &serde_json::to_vec(snap)?)?;
    let a = match seglog::segments(dir)?.last() {
        Some(&f) if f == snap.seq + 1 => {
            // (A compaction with nothing since the last: the empty segment it started goes on.)
            let path = dir.join(seglog::segment_name(f));
            ensure!(std::fs::metadata(&path)?.len() == 0, "{} isn't empty", path.display());
            std::fs::remove_file(&path)?;
            Appender::create(dir, f)?
        }
        _ => Appender::create(dir, snap.seq + 1)?,
    };
    seglog::remove_covered(dir, snap.seq, Some(a.first()))?;
    Ok(a)
}

/// Writes `store`'s snapshot to the NAS (`BACKUP_PATH`) each `every` while it changed, as `member`,
/// on a thread of its own, until the store stops or is dropped (and once more as it stops).
pub fn back_up(store: &Arc<Store>, nas: Arc<dyn Nas + Send + Sync>, member: &str, every: Duration) {
    let (w, member) = (Arc::downgrade(store), member.to_string());
    std::thread::Builder::new()
        .name("pool-store-backup".into())
        .spawn(move || {
            let mut backed = 0;
            loop {
                let Some(s) = w.upgrade() else { return };
                let stopped = {
                    let m = s.m.lock().unwrap_or_else(|e| e.into_inner());
                    let m = s.work.wait_timeout_while(m, every, |m| !m.stop).unwrap_or_else(|e| e.into_inner()).0;
                    m.stop
                };
                let snap = Snapshot { member: member.clone(), at: crate::coord::wall_now(), ..s.snapshot() };
                drop(s);
                if snap.seq != backed {
                    match serde_json::to_vec(&snap)
                        .map_err(anyhow::Error::from)
                        .and_then(|b| nas.write_whole(BACKUP_PATH, &b))
                    {
                        Ok(()) => backed = snap.seq,
                        Err(e) => eprintln!("pool store: its backup on the NAS: {e:#}"),
                    }
                }
                if stopped {
                    return;
                }
            }
        })
        .map_err(|e| eprintln!("pool store: its backup: {e}"))
        .ok();
}

/// The backup on the NAS, if there is one.
pub fn read_backup(nas: &dyn Nas) -> Result<Option<Snapshot>> {
    let Some(b) = nas.read(BACKUP_PATH)? else { return Ok(None) };
    Ok(Some(serde_json::from_slice(&b).context(BACKUP_PATH)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn set(k: &str, v: Value) -> Docs {
        [(k.to_string(), v)].into()
    }

    fn put(s: &Store, k: &str, v: Value) -> u64 {
        let n = s.submit(set(k, v), Vec::new());
        assert!(s.wait(n, Duration::from_secs(10)));
        n
    }

    #[test]
    fn a_restart_replays_the_store_and_bumps_the_epoch() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path()).unwrap();
        assert_eq!(s.epoch(), 1);
        for i in 0..50 {
            s.submit(set(&format!("lease/{i}"), json!(i)), Vec::new());
        }
        let n = s.submit(Docs::new(), vec!["lease/3".into()]);
        assert!(s.wait(n, Duration::from_secs(10)));
        assert_eq!(s.submit(set("lease/4", json!(4)), Vec::new()), 0, "no change, no line");
        let before = s.snapshot().docs;
        s.stop();
        drop(s);
        let back = Store::open(d.path()).unwrap();
        assert_eq!(back.snapshot().docs, before);
        assert_eq!(back.epoch(), 2);
    }

    /// A crash mid-append leaves half a line: dropped (it was never answered), the rest kept, the
    /// store going on from there.
    #[test]
    fn store_recovers_after_a_torn_append() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path()).unwrap();
        put(&s, "a", json!(1));
        put(&s, "b", json!(2));
        s.kill();
        drop(s);
        let seg = *seglog::segments(d.path()).unwrap().last().unwrap();
        let path = d.path().join(seglog::segment_name(seg));
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(br#"{"seq":9,"set":{"c""#).unwrap();
        let back = Store::open(d.path()).unwrap();
        let docs = back.snapshot().docs;
        assert_eq!(docs, [("a".to_string(), json!(1)), ("b".to_string(), json!(2))].into());
        put(&back, "c", json!(3));
        back.stop();
        drop(back);
        assert_eq!(Store::open(d.path()).unwrap().snapshot().docs.get("c"), Some(&json!(3)));
    }

    /// A kill mid-write: a change answered (`wait` true) is there after the restart; one not
    /// answered may be or not (a crash after its `fsync`, before its answer), and nothing else
    /// is lost.
    #[test]
    fn a_kill_before_the_fsync_loses_only_the_unanswered() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path()).unwrap();
        put(&s, "a", json!(1));
        // (The writer held off: the change stays pending.)
        let held = s.m.lock().unwrap();
        let s2 = s.clone();
        let t = std::thread::spawn(move || s2.submit(set("b", json!(2)), Vec::new()));
        drop(held);
        let n = t.join().unwrap();
        s.kill();
        let answered = s.wait(n, Duration::from_millis(1));
        drop(s);
        let docs = Store::open(d.path()).unwrap().snapshot().docs;
        assert!(docs.contains_key("b") || !answered, "answered, so on disk");
        assert_eq!(docs.get("a"), Some(&json!(1)));
    }

    /// A crash after the compaction's snapshot and before its old segments went: the log overlaps
    /// the snapshot, and replay skips what it holds.
    #[test]
    fn store_compaction_crash_between_snapshot_and_log() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open_with(d.path(), 1 << 30, COMPACT_AGE).unwrap();
        for i in 0..20 {
            put(&s, &format!("k{i}"), json!(i));
        }
        put(&s, "k3", json!("changed"));
        s.submit(Docs::new(), vec!["k4".into()]);
        let want = s.snapshot();
        s.stop();
        drop(s);
        // (The compaction's snapshot written, its segments not yet removed, nor the next begun.)
        seglog::write_durable(&d.path().join(SNAPSHOT), &serde_json::to_vec(&want).unwrap()).unwrap();
        let back = Store::open(d.path()).unwrap();
        assert_eq!(back.snapshot().docs, want.docs);
        assert_eq!(back.epoch(), want.epoch + 1);
        put(&back, "k5", json!(5));
        back.stop();
        drop(back);
        let again = Store::open(d.path()).unwrap().snapshot();
        assert_eq!(again.docs.get("k5"), Some(&json!(5)));
        assert_eq!(again.docs.get("k3"), Some(&json!("changed")));
        assert!(!again.docs.contains_key("k4"));
    }

    /// Past its size the log is compacted: one snapshot, the old segments gone, nothing lost.
    #[test]
    fn the_log_compacts_past_its_size() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open_with(d.path(), 2000, COMPACT_AGE).unwrap();
        for i in 0..200 {
            put(&s, &format!("lease/{i}"), json!({ "worker": "m1", "n": i }));
        }
        assert!(s.compactions() >= 1);
        assert!(seglog::segments(d.path()).unwrap().len() <= 2);
        let want = s.snapshot().docs;
        s.stop();
        drop(s);
        assert_eq!(Store::open(d.path()).unwrap().snapshot().docs, want);
    }

    /// The backup on the NAS rebuilds a lost store, its epoch far past the backup's.
    #[test]
    fn store_restores_from_the_backup() {
        let d = tempfile::tempdir().unwrap();
        let nas_dir = d.path().join("nas");
        std::fs::create_dir_all(&nas_dir).unwrap();
        let nas: Arc<dyn Nas + Send + Sync> = Arc::new(store::nasfs::NasRoot::local_for_test(&nas_dir));
        let s = Store::open(&d.path().join("lead")).unwrap();
        put(&s, "settings/brokers", json!("on"));
        back_up(&s, nas.clone(), "mini", Duration::from_millis(20));
        store::testkit::eventually("the backup on the NAS", || {
            read_backup(nas.as_ref()).ok().flatten().is_some_and(|b| b.docs.contains_key("settings/brokers"))
        });
        let epoch = s.epoch();
        s.stop();
        drop(s);
        // (The lead's disk lost.)
        std::fs::remove_dir_all(d.path().join("lead")).unwrap();
        let b = read_backup(nas.as_ref()).unwrap().unwrap();
        assert_eq!(b.member, "mini");
        let back = Store::restore(&d.path().join("lead"), &b).unwrap();
        assert_eq!(back.snapshot().docs.get("settings/brokers"), Some(&json!("on")));
        assert!(back.epoch() > epoch + RESTORE_EPOCHS);
    }
}
