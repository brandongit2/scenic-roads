//! A log of JSON lines on a local disk, in segments `<first seq>.jsonl`, beside a snapshot written
//! whole: what the map's log (`map::log`, docs/maplog.md §4.2) and the pool's store on the lead
//! (pipeline's `pool::store`, docs/coordination.md §4) are both made of.
//!
//! - Appends are `fsync`ed before they count; a failed one is cut off again, and failing that the
//!   appender refuses more (`Appender::torn`) until the log is read again, which cuts it.
//! - Reading cuts a torn last line of the last segment (a write cut short: never answered) off the
//!   file; any other line that doesn't parse is an error.
//! - A snapshot is written whole (temporary file, `fsync`, rename, `fsync` of the folder), so the
//!   segments it covers can go once it returns (`remove_covered`).

use anyhow::{ensure, Context, Result};
use serde::de::DeserializeOwned;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const SEGMENT: &str = ".jsonl";

/// `fsync` of a folder: the names made, renamed or removed in it durable.
pub fn sync_dir(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Writes `b` at `path` whole and durably: a crash leaves the old file or the new one, never half.
pub fn write_durable(path: &Path, b: &[u8]) -> Result<()> {
    let dir = path.parent().context("a file in a folder")?;
    let tmp = crate::naming::tmp_path(path);
    let mut f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    f.write_all(b)?;
    f.sync_all()?;
    fs::rename(&tmp, path).with_context(|| format!("rename into {}", path.display()))?;
    sync_dir(dir).with_context(|| format!("fsync {}", dir.display()))
}

pub fn segment_name(first: u64) -> String {
    format!("{first}{SEGMENT}")
}

/// The segments in `dir` by their first `seq`, lowest first.
pub fn segments(dir: &Path) -> Result<Vec<u64>> {
    let mut v: Vec<u64> = fs::read_dir(dir)
        .with_context(|| format!("list {}", dir.display()))?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(SEGMENT)?.parse().ok())
        .collect();
    v.sort_unstable();
    Ok(v)
}

/// Reads every segment of `dir` in order, each line given to `each` as a `T`. A last line of the
/// last segment that doesn't parse (or has no newline) is cut off the file, logged with `who`.
/// Returns the appender for the last segment (None: no segments), to go on from where it ends.
pub fn read<T: DeserializeOwned>(
    dir: &Path,
    who: &str,
    mut each: impl FnMut(T) -> Result<()>,
) -> Result<Option<Appender>> {
    let segs = segments(dir)?;
    let mut open = None;
    for (i, &first) in segs.iter().enumerate() {
        let path = dir.join(segment_name(first));
        let text = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let last_seg = i + 1 == segs.len();
        let (mut at, mut lines) = (0usize, 0u64);
        while at < text.len() {
            let end = text[at..].iter().position(|&c| c == b'\n').map(|p| at + p);
            let line = &text[at..end.unwrap_or(text.len())];
            let parsed = end.and_then(|_| serde_json::from_slice::<T>(line).ok());
            let Some(v) = parsed else {
                let rest_blank = end.is_none_or(|e| text[e + 1..].iter().all(u8::is_ascii_whitespace));
                ensure!(last_seg && rest_blank, "{}: a damaged line at byte {at}", path.display());
                eprintln!("{who}: {}: a torn last line ({} bytes) dropped", path.display(), text.len() - at);
                let f = OpenOptions::new().write(true).open(&path)?;
                f.set_len(at as u64)?;
                f.sync_all()?;
                break;
            };
            each(v).with_context(|| format!("{}", path.display()))?;
            lines += 1;
            at = end.expect("parsed lines end") + 1;
        }
        if last_seg {
            let file = OpenOptions::new().append(true).open(&path)?;
            open = Some(Appender { dir: dir.to_owned(), first, file, lines, bytes: at as u64, torn: false });
        }
    }
    Ok(open)
}

/// The open segment being appended to.
#[derive(Debug)]
pub struct Appender {
    dir: PathBuf,
    first: u64,
    file: File,
    /// Lines and bytes in it.
    pub lines: u64,
    pub bytes: u64,
    /// A failed append whose bytes couldn't be cut off again: no more appends until the log is read
    /// again (which cuts them as a torn last line), so none lands after them.
    pub torn: bool,
}

impl Appender {
    /// A new segment in `dir` whose first line is `first` (its name made durable).
    pub fn create(dir: &Path, first: u64) -> Result<Appender> {
        let path = dir.join(segment_name(first));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|file| sync_dir(dir).map(|()| file))
            .with_context(|| format!("open {}", path.display()))?;
        Ok(Appender { dir: dir.to_owned(), first, file, lines: 0, bytes: 0, torn: false })
    }

    /// The `seq` of its first line.
    pub fn first(&self) -> u64 {
        self.first
    }

    /// Appends `bytes` (`n` whole lines, each ending in a newline) with one `fsync`. On a failure
    /// nothing counts: what went down is cut off again, or else the appender is `torn`.
    pub fn append(&mut self, bytes: &[u8], n: u64) -> Result<()> {
        ensure!(!self.torn, "the log has a failed write it couldn't cut off: read it again");
        let wrote = self.file.write_all(bytes).and_then(|_| self.file.sync_data());
        if let Err(e) = wrote {
            self.torn = self.file.set_len(self.bytes).and_then(|_| self.file.sync_data()).is_err();
            return Err(e).with_context(|| format!("append to {}", self.dir.display()));
        }
        self.lines += n;
        self.bytes += bytes.len() as u64;
        Ok(())
    }
}

/// Removes each segment of `dir` all of whose lines are at or below `seq` (a snapshot at `seq`
/// holds them), never the one starting at `keep` (the open one).
pub fn remove_covered(dir: &Path, seq: u64, keep: Option<u64>) -> Result<()> {
    let segs = segments(dir)?;
    for w in segs.windows(2) {
        let (first, next) = (w[0], w[1]);
        if next <= seq + 1 && Some(first) != keep {
            let p = dir.join(segment_name(first));
            fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_torn_last_line_is_cut_and_the_log_goes_on() {
        let d = tempfile::tempdir().unwrap();
        let mut a = Appender::create(d.path(), 1).unwrap();
        a.append(b"1\n2\n", 2).unwrap();
        // (A crash mid-write: half a line, no newline.)
        OpenOptions::new().append(true).open(d.path().join(segment_name(1))).unwrap().write_all(b"[3,").unwrap();
        let mut seen = Vec::new();
        let mut a = read::<u64>(d.path(), "test", |v| {
            seen.push(v);
            Ok(())
        })
        .unwrap()
        .unwrap();
        assert_eq!(seen, [1, 2]);
        assert_eq!((a.lines, a.bytes), (2, 4));
        a.append(b"3\n", 1).unwrap();
        let mut seen = Vec::new();
        read::<u64>(d.path(), "test", |v| {
            seen.push(v);
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, [1, 2, 3]);
    }

    #[test]
    fn a_damaged_line_before_the_last_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(segment_name(1)), b"1\nx\n3\n").unwrap();
        assert!(read::<u64>(d.path(), "test", |_| Ok(())).is_err());
    }

    #[test]
    fn covered_segments_go_but_the_open_one() {
        let d = tempfile::tempdir().unwrap();
        for f in [1, 5, 9] {
            fs::write(d.path().join(segment_name(f)), b"").unwrap();
        }
        remove_covered(d.path(), 8, Some(9)).unwrap();
        assert_eq!(segments(d.path()).unwrap(), [9]);
        write_durable(&d.path().join("s.json"), b"{}").unwrap();
        assert_eq!(fs::read(d.path().join("s.json")).unwrap(), b"{}");
    }
}
