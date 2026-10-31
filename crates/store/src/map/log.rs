//! The log store on the lead's disk (docs/maplog.md §4.2, §4.5): `snapshot.json.zst` (the map whole
//! at some `seq`, with the clients' cursors) and segments `<first seq>.jsonl` of batches, one JSON
//! line each, appended and `fsync`ed before a batch counts. Opening loads the snapshot and replays
//! the segments' batches past it; a torn last line (a write cut short: it was never answered) is
//! dropped. `compact` writes the snapshot again and removes the segments it covers. The segments,
//! their torn lines and the snapshot's whole write are crate::seglog's, shared with the pool's store.
//!
//! It keeps, for each watch its opener names (a key prefix, or a part of a batch's `why`:
//! super::marks), the last batch that marked it (docs/maplog.md §6.8), saved with the snapshot and
//! kept up as batches are replayed and appended: what tells the agent the names' to-do lists' inputs
//! changed, or when the low zooms last went out, whatever the log still holds.

use super::{Batch, Cursor, Map, Mark, Snapshot};
use crate::seglog::{self, Appender};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const SNAPSHOT: &str = "snapshot.json.zst";
/// A new segment each this many batches or bytes (§4.2).
const SEGMENT_BATCHES: u64 = 10_000;
const SEGMENT_BYTES: u64 = 64 << 20;

/// The map and its log in a folder of the lead's disk (its segments: crate::seglog).
pub struct Log {
    dir: PathBuf,
    map: Arc<Map>,
    cursors: BTreeMap<String, Cursor>,
    open: Option<Appender>,
    /// The batches replayed at the open, after the snapshot's `seq` (`base`): what its clients are
    /// answered from until it appends more (`replayed`, taken once).
    base: u64,
    replayed: Vec<Batch>,
    /// What's watched, and each watch's last mark (`marks`).
    watch: Vec<String>,
    marks: BTreeMap<String, Mark>,
}

impl Log {
    /// The log in `dir` (made if missing): its snapshot, the batches after it applied in turn. A last
    /// line of the last segment that doesn't parse is cut off the file (logged); any other line that
    /// doesn't, or a batch that doesn't apply (its digest), is an error.
    pub fn open(dir: &Path) -> Result<Log> {
        Log::open_watching(dir, &[])
    }

    /// `open`, its marks kept for `watch` (super::marks): those its snapshot has, else (a prefix
    /// newly watched) the snapshot's own `seq`, then each batch replayed.
    pub fn open_watching(dir: &Path, watch: &[&str]) -> Result<Log> {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let (mut map, cursors, mut marks) = match fs::read(dir.join(SNAPSHOT)) {
            Ok(b) => Snapshot::decode_marked(&b).with_context(|| format!("{}", dir.join(SNAPSHOT).display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Map::default(), BTreeMap::new(), BTreeMap::new()),
            Err(e) => return Err(e).with_context(|| format!("read {}", dir.join(SNAPSHOT).display())),
        };
        let watch: Vec<String> = watch.iter().map(|w| w.to_string()).collect();
        marks.retain(|w, _| watch.contains(w));
        if !map.is_empty() {
            for w in watch.iter().filter(|w| !w.starts_with(super::WHY)) {
                marks.entry(w.clone()).or_insert_with(|| Mark { seq: map.seq, at: map.at.clone() });
            }
        }
        let (base, mut replayed) = (map.seq, Vec::new());
        let open = seglog::read::<Batch>(dir, "maplog", |b| {
            if b.seq > map.seq {
                drop(map.apply_in(&b)?);
                mark(&watch, &mut marks, &b);
                replayed.push(b);
            }
            Ok(())
        })?;
        Ok(Log { dir: dir.to_owned(), map: Arc::new(map), cursors, open, base, replayed, watch, marks })
    }

    /// Each watch's last mark (none for a `why` part no batch since its watch began has said).
    pub fn marks(&self) -> &BTreeMap<String, Mark> {
        &self.marks
    }

    /// The batches replayed at the open and the `seq` they follow (taken: once).
    pub fn replayed(&mut self) -> (u64, Vec<Batch>) {
        (self.base, std::mem::take(&mut self.replayed))
    }

    /// The clients' cursors as its snapshot kept them.
    pub fn cursors(&self) -> &BTreeMap<String, Cursor> {
        &self.cursors
    }

    /// The clients' cursors as heard now, saved with the next snapshot (`compact`).
    pub fn set_cursors(&mut self, cursors: BTreeMap<String, Cursor>) {
        self.cursors = cursors;
    }

    /// The map now: swapped whole at each batch.
    pub fn map(&self) -> Arc<Map> {
        self.map.clone()
    }

    /// When the snapshot was last written, if it was.
    pub fn snapshot_at(&self) -> Option<std::time::SystemTime> {
        fs::metadata(self.dir.join(SNAPSHOT)).and_then(|m| m.modified()).ok()
    }

    /// Starts an empty log at `map` (the first catalogue it's told of): its snapshot.
    pub fn seed(&mut self, map: Map) -> Result<()> {
        ensure!(self.map.is_empty() && self.open.is_none(), "the log holds a map already");
        for w in self.watch.iter().filter(|w| !w.starts_with(super::WHY)) {
            self.marks.insert(w.clone(), Mark { seq: map.seq, at: map.at.clone() });
        }
        self.map = Arc::new(map);
        self.write_snapshot()
    }

    /// Appends `b`: applied to the map (checked by its digest), written and `fsync`ed. Nothing
    /// changes when it doesn't apply or can't be written. A reader holding the map (`map`) keeps the
    /// one it has: the batch is then applied to a copy, swapped in; with none, in place, never a
    /// second whole map.
    pub fn append(&mut self, b: &Batch) -> Result<Arc<Map>> {
        ensure!(
            !self.open.as_ref().is_some_and(|s| s.torn),
            "the map's log has a failed write it couldn't cut off: reopen it"
        );
        let mut line = serde_json::to_vec(b)?;
        line.push(b'\n');
        let map = Arc::make_mut(&mut self.map);
        let undo = map.apply_in(b)?;
        let roll = self.open.as_ref().is_none_or(|s| s.lines >= SEGMENT_BATCHES || s.bytes >= SEGMENT_BYTES);
        if roll {
            match Appender::create(&self.dir, b.seq) {
                Ok(a) => self.open = Some(a),
                Err(e) => {
                    map.undo(undo);
                    return Err(e);
                }
            }
        }
        let s = self.open.as_mut().expect("opened above");
        if let Err(e) = s.append(&line, 1) {
            map.undo(undo);
            return Err(e).context("append to the map's log");
        }
        mark(&self.watch, &mut self.marks, b);
        Ok(self.map.clone())
    }

    fn write_snapshot(&self) -> Result<()> {
        let b = Snapshot { marks: self.marks.clone(), ..Snapshot::of(&self.map, &self.cursors) }.encode()?;
        seglog::write_durable(&self.dir.join(SNAPSHOT), &b)
    }

    /// Writes the snapshot at the map's `seq` and removes each segment all of whose batches it holds
    /// (its clients are answered from memory: store::map::feed). The open one stays.
    pub fn compact(&mut self) -> Result<()> {
        self.write_snapshot()?;
        seglog::remove_covered(&self.dir, self.map.seq, self.open.as_ref().map(|s| s.first()))
    }
}

/// Batch `b`'s marks of the watches `watch`.
fn mark(watch: &[String], marks: &mut BTreeMap<String, Mark>, b: &Batch) {
    for w in watch.iter().filter(|w| super::marks(w, b)) {
        marks.insert(w.clone(), Mark { seq: b.seq, at: b.at.clone() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seglog::{segment_name, segments};
    use serde_json::json;
    use std::fs::OpenOptions;
    use std::io::Write;

    fn map_at(seq: u64, n: usize) -> Map {
        let e = (0..n).map(|i| (format!("u/6/{i}/0"), json!(true))).collect();
        Map::new(seq, format!("t{seq}"), e)
    }

    /// Batches 2..=k, each adding one z6 tile to `start`'s map.
    fn batches(start: &Map, k: u64) -> Vec<Batch> {
        let mut m = start.clone();
        let mut out = Vec::new();
        for s in start.seq + 1..=k {
            let mut next = m.entries().clone();
            next.insert(format!("u/7/{s}/0"), json!(true));
            let b = m.batch_to(&next, s, &format!("t{s}"), "", "");
            m = m.apply(&b).unwrap();
            out.push(b);
        }
        out
    }

    #[test]
    fn a_log_reopens_at_its_last_batch() {
        let d = tempfile::tempdir().unwrap();
        let mut log = Log::open(d.path()).unwrap();
        log.seed(map_at(1, 3)).unwrap();
        for b in batches(&map_at(1, 3), 5) {
            log.append(&b).unwrap();
        }
        let back = Log::open(d.path()).unwrap();
        assert_eq!(*back.map(), *log.map());
        assert_eq!(back.map().seq, 5);
        assert!(back.map().digest_holds());
    }

    #[test]
    fn a_torn_last_line_is_dropped() {
        let d = tempfile::tempdir().unwrap();
        let mut log = Log::open(d.path()).unwrap();
        log.seed(map_at(1, 3)).unwrap();
        let bs = batches(&map_at(1, 3), 4);
        for b in &bs[..2] {
            log.append(b).unwrap();
        }
        let at3 = log.map();
        // (Batch 4's line cut short, as a crash mid-write leaves it.)
        let seg = d.path().join(segment_name(2));
        let mut line = serde_json::to_vec(&bs[2]).unwrap();
        line.truncate(line.len() / 2);
        let len = fs::metadata(&seg).unwrap().len();
        OpenOptions::new().append(true).open(&seg).unwrap().write_all(&line).unwrap();
        let mut back = Log::open(d.path()).unwrap();
        assert_eq!(*back.map(), *at3);
        assert_eq!(fs::metadata(&seg).unwrap().len(), len, "the torn line is cut off the file");
        // (And the log goes on from there.)
        back.append(&bs[2]).unwrap();
        assert_eq!(Log::open(d.path()).unwrap().map().seq, 4);
        // A damaged line before the last isn't a torn write: an error.
        let mut text = fs::read(&seg).unwrap();
        text[3] = b'#';
        fs::write(&seg, text).unwrap();
        assert!(Log::open(d.path()).is_err());
    }

    #[test]
    fn replay_after_snapshot_skips_what_it_holds() {
        let d = tempfile::tempdir().unwrap();
        let mut log = Log::open(d.path()).unwrap();
        log.seed(map_at(1, 2)).unwrap();
        let bs = batches(&map_at(1, 2), 6);
        for b in &bs[..3] {
            log.append(b).unwrap();
        }
        // The snapshot at 4; the segment holding 2..4 is the open one and stays, and 5, 6 follow it.
        log.compact().unwrap();
        for b in &bs[3..] {
            log.append(b).unwrap();
        }
        let back = Log::open(d.path()).unwrap();
        assert_eq!(*back.map(), *log.map());
        assert_eq!(back.map().seq, 6);
    }

    #[test]
    fn compacting_removes_the_segments_the_snapshot_holds() {
        let d = tempfile::tempdir().unwrap();
        let mut log = Log::open(d.path()).unwrap();
        log.seed(map_at(1, 2)).unwrap();
        let bs = batches(&map_at(1, 2), 5);
        log.append(&bs[0]).unwrap();
        // (A segment ends when the agent restarts with none open: here, by hand.)
        log.open = None;
        for b in &bs[1..] {
            log.append(b).unwrap();
        }
        assert_eq!(segments(d.path()).unwrap(), vec![2, 3]);
        log.compact().unwrap();
        assert_eq!(segments(d.path()).unwrap(), vec![3]);
        assert_eq!(*Log::open(d.path()).unwrap().map(), *log.map());
    }

    #[test]
    fn a_batch_that_doesnt_apply_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let mut log = Log::open(d.path()).unwrap();
        log.seed(map_at(1, 2)).unwrap();
        let mut b = batches(&map_at(1, 2), 2).remove(0);
        b.digest = super::super::hex(1);
        assert!(log.append(&b).is_err());
        assert_eq!(log.map().seq, 1);
        assert!(segments(d.path()).unwrap().is_empty());
    }
}
