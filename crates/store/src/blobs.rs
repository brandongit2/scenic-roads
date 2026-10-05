//! Local copies of the store's content-named files, on this Mac's disk (docs/workers.md §4: the
//! SSD tier). A file is copied from the NAS once, whole, in one sequential read, and read locally
//! after that: a unit's staging made thousands of small reads of the NAS's packs, one per tile, at
//! milliseconds each under load. Content names never change meaning, so a copy is never stale, and
//! since the records name only files the NAS has, a copy can be deleted without asking it.
//!
//! Layout: `<dir>/<content name>` (the name's folders kept), each copy's modification time its last
//! use, so the least recently used go first (`evict`). A copy is written under a temporary name
//! and renamed once its length matches, so a reader never sees half of one; two processes copying
//! the same file both finish with the same bytes under the one name.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug)]
pub struct Blobs {
    dir: PathBuf,
}

impl Blobs {
    pub fn new(dir: impl Into<PathBuf>) -> Blobs {
        Blobs { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The local path a content name's copy has (whether or not it's there).
    pub fn path_of(&self, content: &str) -> PathBuf {
        self.dir.join(content)
    }

    /// A whole local copy of `content` (under the NAS root `root`), copied first if it isn't here;
    /// marked used.
    pub fn get(&self, root: &Path, content: &str) -> io::Result<PathBuf> {
        let local = self.path_of(content);
        let src = root.join(content);
        if let Ok(m) = fs::metadata(&local) {
            // (A copy's length is its source's: content-named files are written whole, once.)
            if m.is_file() {
                touch(&local);
                return Ok(local);
            }
        }
        let parent = local.parent().ok_or_else(|| io::Error::other("a content name without a folder"))?;
        fs::create_dir_all(parent)?;
        let tmp = crate::naming::tmp_path(&local);
        let copied = (|| -> io::Result<()> {
            let n = crate::sys::copy_data(&src, &tmp)?;
            let want = fs::metadata(&src)?.len();
            if n != want {
                return Err(io::Error::other(format!("{}: copied {n} of {want} bytes", src.display())));
            }
            fs::rename(&tmp, &local)
        })();
        if let Err(e) = copied {
            fs::remove_file(&tmp).ok();
            return Err(e);
        }
        Ok(local)
    }

    /// The copy of `content` if it's here (marked used), else None: nothing fetched.
    pub fn have(&self, content: &str) -> Option<PathBuf> {
        let local = self.path_of(content);
        fs::metadata(&local).ok().filter(|m| m.is_file()).map(|_| {
            touch(&local);
            local
        })
    }

    /// Every copy here: (path, length, last use).
    pub fn list(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut out = Vec::new();
        walk(&self.dir, &mut out);
        out
    }

    /// Bytes the copies hold.
    pub fn bytes(&self) -> u64 {
        self.list().iter().map(|f| f.1).sum()
    }

    /// Deletes the least recently used copies, not used in the last `keep`, until at least
    /// `want` bytes are freed or none is left to delete; the bytes freed. Temporary files left by a
    /// stopped copy go too (they're never used).
    pub fn evict(&self, want: u64, keep: Duration) -> u64 {
        let now = SystemTime::now();
        let mut files = self.list();
        files.sort_by_key(|f| f.2);
        let mut freed = 0;
        for (p, len, used) in files {
            if freed >= want {
                break;
            }
            let tmp = p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".tmp"));
            if !tmp && now.duration_since(used).unwrap_or_default() < keep {
                continue;
            }
            if fs::remove_file(&p).is_ok() {
                freed += len;
            }
        }
        freed
    }
}

/// Marks a copy used now (its modification time; failure only costs eviction order).
fn touch(p: &Path) {
    if let Ok(f) = fs::File::options().write(true).open(p) {
        f.set_modified(SystemTime::now()).ok();
    }
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue };
        if m.is_dir() {
            walk(&e.path(), out);
        } else {
            out.push((e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_once_and_evicts_the_least_recently_used() {
        let d = tempfile::tempdir().unwrap();
        let (nas, local) = (d.path().join("nas"), d.path().join("local"));
        for (n, len) in [("layers/a/6-1-2.0000000000000001.pack", 100), ("layers/b/6-1-3.0000000000000002.pack", 200)] {
            fs::create_dir_all(nas.join(n).parent().unwrap()).unwrap();
            fs::write(nas.join(n), vec![7u8; len]).unwrap();
        }
        let b = Blobs::new(&local);
        let a = b.get(&nas, "layers/a/6-1-2.0000000000000001.pack").unwrap();
        assert_eq!(fs::read(&a).unwrap(), vec![7u8; 100]);
        // Copied once: a second ask is served here even with the NAS gone.
        fs::remove_dir_all(&nas).unwrap();
        assert_eq!(b.get(&nas, "layers/a/6-1-2.0000000000000001.pack").unwrap(), a);
        assert!(b.get(&nas, "layers/b/6-1-3.0000000000000002.pack").is_err());
        assert!(b.have("layers/b/6-1-3.0000000000000002.pack").is_none());
        assert_eq!(b.bytes(), 100);
        // Used just now: kept within `keep`, deleted past it.
        assert_eq!(b.evict(1, Duration::from_secs(3600)), 0);
        assert_eq!(b.evict(1, Duration::ZERO), 100);
        assert_eq!(b.bytes(), 0);
    }
}
