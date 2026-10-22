//! Whole local copies of the NAS's content-named files a job reads by many small ranges (task #551):
//! the basemap (`layers/basemap/world-<date>.<hash>.pmtiles`, the depth's, the terrain's, the lakes'
//! and the water's water) and the depth sources' packs (`sources/fetched/<id>/<z6 tile>.<hash>.tar`).
//! A random 64 KB read of the NAS waits ~40 ms on its disks' seeks, the Mac's own SSD's 0.2 ms: a
//! file read in thousands of ranges is copied once, whole, in one sequential read, and read here.
//!
//! The copies are this Mac's `blobs/` (store::blobs, in the agent's cache, `SCENIC_CACHE`):
//! content-named, so never stale; kept between jobs, so the targets after share them; held while a
//! job reads them, and otherwise freed by room-making, the least recently used first
//! (crate::agent::room::toward). A copy is made only when the disk has room for it past the
//! owner's room target; short of it (a laptop), or with no cache named (a job run by
//! hand, a test), the NAS's file is read in place, as before. The copies' reads count as the local
//! disk's (store::range::local_bytes, crate::costin's `local`), so the NAS's weight of a run that
//! reads them is what it copied (crate::needs::nas_rate); the copies are one shared cache a Mac
//! fills once (crate::needs::CACHES: `CACHE`), reserved only on a Mac with room for it
//! (crate::needs::ROOM_ONLY). One process copies a file at a time (`making`, and across processes a
//! lock file in the cache's `copying/`): another wanting it waits, then reads that copy.
//!
//! (A copy room-making frees between jobs is copied again by the next job that reads it: with the
//! disk near its target, the basemap's 28.6 GB may go and come back job after job. Its reads are
//! the NAS's then, as before, plus the copy's.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The shared cache's name (crate::needs::CACHES).
pub const CACHE: &str = "whole copies";

/// The folders of `blobs/` the shared cache is (what a Mac holds of it: crate::agent's
/// `caches_here`).
pub const FOLDERS: [(&str, &str); 2] = [("layers", "basemap"), ("sources", "fetched")];

/// This Mac's copies of the NAS's files under `root`.
pub struct Local {
    root: PathBuf,
    blobs: store::blobs::Blobs,
    /// Where the copies' lock files are (`<cache>/copying/`).
    locks: PathBuf,
    /// The owner's room target (bytes), kept free.
    target: u64,
    /// The copies being made by this process, one at a time each (the readers' threads wanting the
    /// same pack wait for its copy, not make it twice).
    making: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Local {
    /// The agent's cache (`SCENIC_CACHE`) and room target (crate::agent::room::job_target) for the
    /// NAS's files under `root`; None without a cache (a job run by hand, a test): the NAS read.
    pub fn from_env(root: &Path) -> Option<Local> {
        let cache = std::env::var_os(crate::coverage::CACHE_ENV)?;
        Some(Local::new(root, Path::new(&cache), crate::agent::room::job_target()))
    }

    pub fn new(root: &Path, cache: &Path, target: u64) -> Local {
        Local {
            root: root.to_path_buf(),
            blobs: store::blobs::Blobs::new(cache.join("blobs")),
            locks: cache.join("copying"),
            target,
            making: Mutex::new(HashMap::new()),
        }
    }

    /// Where NAS file `p` (a content-named file under the root) is read: its whole local copy,
    /// held by this process, when it's here or there's room to make it (made now, counted as the
    /// NAS's bytes read); else `p` itself (a copy that fails says so and reads the NAS).
    pub fn path(&self, p: &Path) -> PathBuf {
        let Some(content) = p.strip_prefix(&self.root).ok().and_then(|r| r.to_str()).map(str::to_string) else {
            return p.to_path_buf();
        };
        if let Some(l) = self.blobs.have(&content) {
            return l;
        }
        let one = self.making.lock().unwrap_or_else(|e| e.into_inner()).entry(content.clone()).or_default().clone();
        let _one = one.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = self.blobs.have(&content) {
            return l;
        }
        // (Another process copying it: waited for, its copy read.)
        let _lock = self.lock(&content);
        if let Some(l) = self.blobs.have(&content) {
            return l;
        }
        let Ok(len) = std::fs::metadata(p).map(|m| m.len()) else { return p.to_path_buf() };
        if std::fs::create_dir_all(self.blobs.dir()).is_err()
            || crate::agent::room::free_past(self.blobs.dir(), self.target) < len
        {
            return p.to_path_buf();
        }
        let _p = crate::timings::phase("whole local copies made", crate::timings::Class::NasRead);
        let t = std::time::Instant::now();
        match self.blobs.get(&self.root, &content) {
            Ok(l) => {
                store::range::count_read(len);
                store::nasio::read(len);
                store::nasio::opened();
                crate::timings::count(len, 1);
                eprintln!("{content}: copied here ({} MB in {:.1} s)", len >> 20, t.elapsed().as_secs_f64());
                l
            }
            Err(e) => {
                eprintln!("{content}: not copied here ({e}): read from the NAS");
                p.to_path_buf()
            }
        }
    }
}

impl Local {
    /// The lock on copying `content` across processes (an exclusive `flock` on its lock file, held
    /// until it's dropped); None when it can't be taken (copied unlocked, as before).
    fn lock(&self, content: &str) -> Option<std::fs::File> {
        std::fs::create_dir_all(&self.locks).ok()?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.locks.join(format!("{}.lock", store::naming::hash16(content.as_bytes()))))
            .ok()?;
        // (std's `File::lock`: flock on macOS; the lock goes with the file.)
        f.lock().ok().map(|()| f)
    }
}

/// `p` as `local` reads it (`Local::path`), or `p` without one.
pub fn path(local: Option<&Local>, p: &Path) -> PathBuf {
    local.map_or_else(|| p.to_path_buf(), |l| l.path(p))
}

/// The bytes this Mac's cache folder `cache` holds of the shared cache (`FOLDERS`).
pub fn held(cache: &Path) -> u64 {
    FOLDERS.iter().map(|(d, prefix)| store::cachefile::bytes_of(&cache.join("blobs").join(d), prefix)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file copied once when there's room, read from the copy after (the NAS's gone), and read
    /// in place when there's none or it's outside the root.
    #[test]
    fn copied_when_theres_room_else_read_in_place() {
        let d = tempfile::tempdir().unwrap();
        let (nas, cache) = (d.path().join("nas"), d.path().join("cache"));
        let name = "sources/fetched/s/6-1-2.0123456789abcdef.tar";
        std::fs::create_dir_all(nas.join(name).parent().unwrap()).unwrap();
        std::fs::write(nas.join(name), vec![5u8; 4096]).unwrap();
        // (No room: a target past any disk.)
        let full = Local::new(&nas, &cache, crate::agent::room::MAX_TARGET);
        assert_eq!(full.path(&nas.join(name)), nas.join(name));
        assert_eq!(held(&cache), 0);
        let l = Local::new(&nas, &cache, 0);
        let outside = d.path().join("elsewhere.tar");
        assert_eq!(l.path(&outside), outside);
        let read = store::range::read_bytes();
        let got = l.path(&nas.join(name));
        assert_eq!(got, cache.join("blobs").join(name));
        assert!(store::range::read_bytes() - read >= 4096, "the copy counted as read");
        assert_eq!(std::fs::read(&got).unwrap(), vec![5u8; 4096]);
        assert_eq!(held(&cache), 4096);
        std::fs::remove_dir_all(&nas).unwrap();
        // (Here already: no room asked, the NAS not read.)
        assert_eq!(full.path(&nas.join(name)), got);
        // Its reads are the local disk's.
        let local = store::range::local_bytes();
        let f = store::range::PlainFile::open(&got).unwrap();
        store::range::RangeRead::read_at(&f, 0, 100).unwrap();
        assert!(store::range::local_bytes() - local >= 100);
        store::cachefile::release(&got);
    }
}
