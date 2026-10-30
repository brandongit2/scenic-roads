//! The log store on the lead's disk (docs/maplog.md §4.2, §4.5): `snapshot.json.zst` (the map whole
//! at some `seq`, with the clients' cursors) and segments `<first seq>.jsonl` of batches, one JSON
//! line each, appended and `fsync`ed before a batch counts. Opening loads the snapshot and replays
//! the segments' batches past it; a torn last line (a write cut short: it was never answered) is
//! dropped. `compact` writes the snapshot again and removes the segments it covers.

use super::{Batch, Cursor, Map, Snapshot};
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const SNAPSHOT: &str = "snapshot.json.zst";
const SEGMENT: &str = ".jsonl";
/// A new segment each this many batches or bytes (§4.2).
const SEGMENT_BATCHES: u64 = 10_000;
const SEGMENT_BYTES: u64 = 64 << 20;

/// The open segment being appended to.
struct Segment {
    first: u64,
    file: File,
    batches: u64,
    bytes: u64,
}

/// The map and its log in a folder of the lead's disk.
pub struct Log {
    dir: PathBuf,
    map: Arc<Map>,
    cursors: BTreeMap<String, Cursor>,
    open: Option<Segment>,
}

fn segment_name(first: u64) -> String {
    format!("{first}{SEGMENT}")
}

/// The segments in `dir` by their first `seq`, lowest first.
fn segments(dir: &Path) -> Result<Vec<u64>> {
    let mut v: Vec<u64> = fs::read_dir(dir)
        .with_context(|| format!("list {}", dir.display()))?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(SEGMENT)?.parse().ok())
        .collect();
    v.sort_unstable();
    Ok(v)
}

impl Log {
    /// The log in `dir` (made if missing): its snapshot, the batches after it applied in turn. A last
    /// line of the last segment that doesn't parse is cut off the file (logged); any other line that
    /// doesn't, or a batch that doesn't apply (its digest), is an error.
    pub fn open(dir: &Path) -> Result<Log> {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let (mut map, cursors) = match fs::read(dir.join(SNAPSHOT)) {
            Ok(b) => Snapshot::decode(&b).with_context(|| format!("{}", dir.join(SNAPSHOT).display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Map::default(), BTreeMap::new()),
            Err(e) => return Err(e).with_context(|| format!("read {}", dir.join(SNAPSHOT).display())),
        };
        let segs = segments(dir)?;
        let mut open = None;
        for (i, &first) in segs.iter().enumerate() {
            let path = dir.join(segment_name(first));
            let text = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            let last_seg = i + 1 == segs.len();
            let (mut at, mut batches) = (0usize, 0u64);
            while at < text.len() {
                let end = text[at..].iter().position(|&c| c == b'\n').map(|p| at + p);
                let line = &text[at..end.unwrap_or(text.len())];
                let parsed = end.and_then(|_| serde_json::from_slice::<Batch>(line).ok());
                let Some(b) = parsed else {
                    let rest_blank = end.is_none_or(|e| text[e + 1..].iter().all(u8::is_ascii_whitespace));
                    ensure!(last_seg && rest_blank, "{}: a damaged line at byte {at}", path.display());
                    eprintln!("maplog: {}: a torn last line ({} bytes) dropped", path.display(), text.len() - at);
                    let f = OpenOptions::new().write(true).open(&path)?;
                    f.set_len(at as u64)?;
                    f.sync_all()?;
                    break;
                };
                if b.seq > map.seq {
                    map.apply_in(&b).with_context(|| format!("{}", path.display()))?;
                }
                batches += 1;
                at = end.expect("parsed lines end") + 1;
            }
            if last_seg {
                let file = OpenOptions::new().append(true).open(&path)?;
                open = Some(Segment { first, file, batches, bytes: at as u64 });
            }
        }
        Ok(Log { dir: dir.to_owned(), map: Arc::new(map), cursors, open })
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
        self.map = Arc::new(map);
        self.write_snapshot()
    }

    /// Appends `b`: applied to a copy of the map (checked by its digest), written and `fsync`ed, then
    /// the copy swapped in. Nothing changes when it doesn't apply or can't be written.
    pub fn append(&mut self, b: &Batch) -> Result<Arc<Map>> {
        let next = Arc::new(self.map.apply(b)?);
        let mut line = serde_json::to_vec(b)?;
        line.push(b'\n');
        let roll = self.open.as_ref().is_none_or(|s| s.batches >= SEGMENT_BATCHES || s.bytes >= SEGMENT_BYTES);
        if roll {
            let path = self.dir.join(segment_name(b.seq));
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .with_context(|| format!("open {}", path.display()))?;
            self.open = Some(Segment { first: b.seq, file, batches: 0, bytes: 0 });
        }
        let s = self.open.as_mut().expect("opened above");
        let wrote = s.file.write_all(&line).and_then(|_| s.file.sync_data());
        if let Err(e) = wrote {
            // (Whatever went down cut off again; failing that, it's a torn last line, cut at the next
            // open.)
            let _ = s.file.set_len(s.bytes);
            return Err(e).context("append to the map's log");
        }
        s.batches += 1;
        s.bytes += line.len() as u64;
        self.map = next;
        Ok(self.map.clone())
    }

    fn write_snapshot(&self) -> Result<()> {
        let b = Snapshot::of(&self.map, self.cursors.clone()).encode()?;
        let path = self.dir.join(SNAPSHOT);
        let tmp = crate::naming::tmp_path(&path);
        let mut f = File::create(&tmp)?;
        f.write_all(&b)?;
        f.sync_all()?;
        fs::rename(&tmp, &path).with_context(|| format!("rename into {}", path.display()))?;
        Ok(())
    }

    /// Writes the snapshot at the map's `seq` and removes each segment all of whose batches it holds
    /// (none is read by a client yet: docs/maplog.md §8, step 3). The open one stays.
    pub fn compact(&mut self) -> Result<()> {
        self.write_snapshot()?;
        let segs = segments(&self.dir)?;
        let open = self.open.as_ref().map(|s| s.first);
        for w in segs.windows(2) {
            let (first, next) = (w[0], w[1]);
            if next <= self.map.seq + 1 && Some(first) != open {
                let p = self.dir.join(segment_name(first));
                if let Err(e) = fs::remove_file(&p) {
                    bail!("remove {}: {e}", p.display());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
