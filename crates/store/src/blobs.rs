//! Local copies of the store's content-named files, on this Mac's disk (docs/workers.md §4: the
//! SSD tier). A file is copied from the NAS once, whole, in one sequential read, and read locally
//! after that: a unit's staging made thousands of small reads of the NAS's packs, one per tile, at
//! milliseconds each under load. Content names never change meaning, so a copy is never stale, and
//! since the records name only files the NAS has, a copy can be deleted without asking it.
//!
//! Layout: `<dir>/<content name>` (the name's folders kept), each copy's modification time its last
//! use, so the least recently used go first (`evict`). A copy is made and held through
//! crate::cachefile: under a temporary name, named once its length matches, so a reader never sees
//! half of one (two processes copying the same file: the first named wins); held by the job that
//! asked for it, so room-making leaves it while that job runs.

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
    /// held by this process (crate::cachefile: room-making leaves it until the job lets go) and
    /// marked used.
    pub fn get(&self, root: &Path, content: &str) -> io::Result<PathBuf> {
        let local = self.path_of(content);
        let src = root.join(content);
        crate::cachefile::hold(&local, &mut |tmp| {
            let n = crate::sys::copy_data(&src, tmp)?;
            let want = fs::metadata(&src)?.len();
            if n != want {
                return Err(io::Error::other(format!("{}: copied {n} of {want} bytes", src.display())));
            }
            Ok(())
        })
    }

    /// The copy of `content` if it's here (held, and marked used, as `get`), else None: nothing
    /// fetched.
    pub fn have(&self, content: &str) -> Option<PathBuf> {
        crate::cachefile::hold_existing(&self.path_of(content)).ok().flatten()
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
            // (Not one a job holds: crate::cachefile.)
            if let crate::cachefile::Removed::Freed(_) = crate::cachefile::try_remove(&p) {
                freed += len;
            }
        }
        freed
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

    /// Room-making mid-job (crate::cachefile): copies deleted as fast as they can be, each between
    /// its uses, are copied again; each read gives the NAS's bytes.
    #[test]
    fn copies_deleted_under_a_job_are_copied_again() {
        let d = tempfile::tempdir().unwrap();
        let (nas, local) = (d.path().join("nas"), d.path().join("local"));
        let names: Vec<String> = (0..20).map(|i| format!("layers/a/6-1-{i}.{i:016x}.pack")).collect();
        for (i, n) in names.iter().enumerate() {
            fs::create_dir_all(nas.join(n).parent().unwrap()).unwrap();
            fs::write(nas.join(n), vec![i as u8; 1000 + i]).unwrap();
        }
        let b = Blobs::new(&local);
        let stop = std::sync::atomic::AtomicBool::new(false);
        let freed = std::thread::scope(|s| {
            let deleter = s.spawn(|| {
                let mut n = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    n += crate::cachefile::remove_tree(&local).freed;
                }
                n
            });
            let mut wrong = Vec::new();
            for _ in 0..30 {
                for (i, n) in names.iter().enumerate() {
                    match b.get(&nas, n).and_then(|p| fs::read(&p).map(|b| (p, b))) {
                        Ok((p, got)) => {
                            if got != vec![i as u8; 1000 + i] {
                                wrong.push(format!("{n}: {} bytes", got.len()));
                            }
                            crate::cachefile::release(&p);
                        }
                        Err(e) => wrong.push(format!("{n}: {e}")),
                    }
                }
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            assert!(wrong.is_empty(), "{wrong:?}");
            deleter.join().unwrap()
        });
        assert!(freed > 0);
    }

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
        // (Held: room-making leaves it.)
        assert_eq!(crate::cachefile::try_remove(&a), crate::cachefile::Removed::InUse);
        crate::cachefile::release(&a);
        // Copied once: a second ask is served here even with the NAS gone.
        fs::remove_dir_all(&nas).unwrap();
        assert_eq!(b.get(&nas, "layers/a/6-1-2.0000000000000001.pack").unwrap(), a);
        assert!(b.get(&nas, "layers/b/6-1-3.0000000000000002.pack").is_err());
        assert!(b.have("layers/b/6-1-3.0000000000000002.pack").is_none());
        assert_eq!(b.bytes(), 100);
        // Used just now: kept within `keep`, deleted past it.
        assert_eq!(b.evict(1, Duration::from_secs(3600)), 0);
        crate::cachefile::release(&a);
        // (A sibling test's child may hold a copy of the hold's descriptor a moment, between its
        // fork and its exec: evicted again until it goes, five minutes a watchdog.)
        let end = std::time::Instant::now() + Duration::from_secs(300);
        let mut freed = b.evict(1, Duration::ZERO);
        while freed == 0 && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
            freed = b.evict(1, Duration::ZERO);
        }
        assert_eq!(freed, 100);
        assert_eq!(b.bytes(), 0);
    }
}
