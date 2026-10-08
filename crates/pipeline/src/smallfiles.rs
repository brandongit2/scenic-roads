//! Small input files read again and again by the long-running processes (the agent's plans, the
//! map server's regions): kept in memory and read again only when their size or time changes.
//!
//! Over SMB the NAS answers a file's open and read slowly under load (docs/plan.md §3: the region
//! recipes, ~100 KB in 90 files, took 1–9 s; the ferries' 64 timetable files 1–11 s, 8 Oct 2026),
//! while its size and time come with the folder's listing, in a fraction of that. A plan read both
//! once or twice, so each plan cost the NAS tens of seconds of small reads. Here a file is read once,
//! and after that only stat'ed; what's read is the same bytes, so nothing built from it changes.
//!
//! A file changed in the last few seconds is always read (a time's granularity over SMB can hide a
//! second write in the same second), and nothing larger than `MAX` is kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// The largest file kept.
pub const MAX: u64 = 4 << 20;
/// A file changed this recently is read every time.
const SETTLE: Duration = Duration::from_secs(5);

type Kept = HashMap<PathBuf, (u64, SystemTime, Arc<Vec<u8>>)>;

fn kept() -> &'static Mutex<Kept> {
    static K: OnceLock<Mutex<Kept>> = OnceLock::new();
    K.get_or_init(Default::default)
}

/// `path`'s bytes: the copy kept while its size and time are the same, else read (and kept, when
/// small and settled).
pub fn read(path: &Path) -> std::io::Result<Arc<Vec<u8>>> {
    let md = std::fs::metadata(path)?;
    let (len, time) = (md.len(), md.modified()?);
    if let Some((l, t, bytes)) = kept().lock().unwrap().get(path) {
        if *l == len && *t == time {
            return Ok(bytes.clone());
        }
    }
    let bytes = Arc::new(std::fs::read(path)?);
    let settled = SystemTime::now().duration_since(time).is_ok_and(|age| age >= SETTLE);
    // (Kept as read, under the size and time it had before the read: a write during the read
    // changes them, so the next read sees it.)
    if settled && len <= MAX && bytes.len() as u64 == len {
        kept().lock().unwrap().insert(path.to_path_buf(), (len, time, bytes.clone()));
    } else {
        kept().lock().unwrap().remove(path);
    }
    Ok(bytes)
}

/// `path` as text (UTF-8), as `read` keeps it.
pub fn read_to_string(path: &Path) -> std::io::Result<String> {
    let b = read(path)?;
    String::from_utf8(b.to_vec()).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(p: &Path, secs: u64) {
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }

    #[test]
    fn a_file_is_read_again_only_when_its_size_or_time_changes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.toml");
        std::fs::write(&p, b"one").unwrap();
        age(&p, 60);
        assert_eq!(*read(&p).unwrap(), b"one");
        // Kept: the same bytes even when the file's are (by another means) different but its size
        // and time aren't.
        let kept_time = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::fs::write(&p, b"two").unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(kept_time).unwrap();
        assert_eq!(*read(&p).unwrap(), b"one", "same size and time: the kept copy");
        // A new time: read again.
        age(&p, 30);
        assert_eq!(*read(&p).unwrap(), b"two");
        // A new size: read again.
        std::fs::write(&p, b"three").unwrap();
        age(&p, 20);
        assert_eq!(read_to_string(&p).unwrap(), "three");
    }

    #[test]
    fn a_file_just_changed_is_read_every_time() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("b.json");
        std::fs::write(&p, b"x1").unwrap();
        assert_eq!(*read(&p).unwrap(), b"x1");
        // Changed within the same second, same size: still seen, as it wasn't kept.
        let t = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::fs::write(&p, b"x2").unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        assert_eq!(*read(&p).unwrap(), b"x2");
        // Gone: an error, as reading it would be.
        std::fs::remove_file(&p).unwrap();
        assert!(read(&p).is_err());
    }
}
