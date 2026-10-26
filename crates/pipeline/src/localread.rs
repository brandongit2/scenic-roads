//! Whole local copies of the NAS's content-named files a job reads by many small ranges (task #551):
//! the basemap (`layers/basemap/world-<date>.<hash>.pmtiles`, the depth's, the terrain's, the lakes'
//! and the water's water), the depth sources' packs (`sources/fetched/<id>/<z6 tile>.<hash>.tar`),
//! and the stored elevation datasets' GeoTIFFs (`sources/fetched/<id>/<name>.tif`, task #689:
//! crate::dem::generic::stored) a terrain job reads at a level holding a large share of the file.
//! A random 64 KB read of the NAS waits ~40 ms on its disks' seeks, the Mac's own SSD's 0.2 ms: a
//! file read in thousands of ranges is copied once, whole, in one sequential read, and read here.
//!
//! The copies are this Mac's `blobs/` (store::blobs, in the agent's cache, `SCENIC_CACHE`):
//! content-named, so never stale; a stored GeoTIFF is named by its source, not its content, yet
//! written once (store-fetch; one cut short is deleted and fetched again), so its copy is read only
//! while its length is the NAS file's (`path_sized`: one that isn't is deleted). Kept between jobs, so the targets after share them; held while a
//! job reads them, and otherwise freed by room-making, the least recently used first
//! (crate::agent::room::toward). A copy is made only when the disk has room for it past the
//! owner's room target; short of it (a laptop), or with no cache named (a job run by
//! hand, a test), the NAS's file is read in place, as before. The copies' reads count as the local
//! disk's (store::nasio's `Local`, crate::costin's `local`), so the NAS's weight of a run that
//! reads them is what it copied (crate::needs::nas_rate); the copies are one shared cache a Mac
//! fills once (crate::needs::CACHES: `CACHE`), reserved only on a Mac with room for it
//! (crate::coord's `missing`). One process copies a file at a time (`making`, and across processes a
//! lock file in the cache's `copying/`): another wanting it waits, then reads that copy.
//!
//! (A copy room-making frees between jobs is copied again by the next job that reads it: with the
//! disk near its target, the basemap's 28.6 GB may go and come back job after job. Its reads are
//! the NAS's then, as before, plus the copy's.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use store::nasfs::{NasPath, Place};

/// The shared cache's name (crate::needs::CACHES).
pub const CACHE: &str = "whole copies";

/// This Mac's copies of the NAS's files under `root`.
pub struct Local {
    root: store::nasfs::NasRoot,
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
    pub fn from_env(root: &store::nasfs::NasRoot) -> Option<Local> {
        let cache = std::env::var_os(crate::coverage::CACHE_ENV)?;
        Some(Local::new(root, Path::new(&cache), crate::agent::room::job_target()))
    }

    pub fn new(root: &store::nasfs::NasRoot, cache: &Path, target: u64) -> Local {
        Local {
            root: root.clone(),
            blobs: store::blobs::Blobs::new(cache.join("blobs")),
            locks: cache.join("copying"),
            target,
            making: Mutex::new(HashMap::new()),
        }
    }

    /// Where NAS file `p` (a content-named file under the root) is read: its whole local copy,
    /// held by this process, when it's here or there's room to make it (made now, counted as the
    /// NAS's bytes read); else `p` itself (a copy that fails says so and reads the NAS).
    pub fn path(&self, p: &NasPath) -> Place {
        self.place(p, None)
    }

    /// As `path`, for a file named by its source, not its content (a stored GeoTIFF), `len` bytes
    /// long on the NAS: a copy of another length is stale, deleted, and copied again.
    pub fn path_sized(&self, p: &NasPath, len: u64) -> Place {
        self.place(p, Some(len))
    }

    /// The copy of `p` if it's here already (held, as `path`'s), `len` bytes long; None, nothing
    /// copied, otherwise.
    pub fn here(&self, p: &NasPath, len: u64) -> Option<PathBuf> {
        self.have(&p.rel_under(&self.root)?, Some(len)).ok()
    }

    fn place(&self, p: &NasPath, len: Option<u64>) -> Place {
        let nas = || Place::Nas(p.clone());
        let Some(content) = p.rel_under(&self.root) else { return nas() };
        // (A stale copy that can't be deleted, one another job holds: the NAS read.)
        let have = || match self.have(&content, len) {
            Ok(l) => Some(Some(l)),
            Err(true) => None,
            Err(false) => Some(None),
        };
        match have() {
            None => return nas(),
            Some(Some(l)) => return Place::Local(l),
            Some(None) => {}
        }
        let one = self.making.lock().unwrap_or_else(|e| e.into_inner()).entry(content.clone()).or_default().clone();
        let _one = one.lock().unwrap_or_else(|e| e.into_inner());
        match have() {
            None => return nas(),
            Some(Some(l)) => return Place::Local(l),
            Some(None) => {}
        }
        // (Another process copying it: waited for, its copy read.)
        let _lock = self.lock(&content);
        match have() {
            None => return nas(),
            Some(Some(l)) => return Place::Local(l),
            Some(None) => {}
        }
        let Some(len) = len.or_else(|| p.metadata().ok().map(|m| m.len())) else { return nas() };
        if std::fs::create_dir_all(self.blobs.dir()).is_err()
            || crate::agent::room::free_past(self.blobs.dir(), self.target) < len
        {
            return nas();
        }
        let _p = crate::timings::phase("whole local copies made", crate::timings::Class::NasRead);
        let t = std::time::Instant::now();
        match self.blobs.get(&self.root, &content) {
            Ok(l) => {
                p.count_read(len, t.elapsed().as_millis() as u64);
                crate::timings::count(len, 1);
                eprintln!("{content}: copied here ({} MB in {:.1} s)", len >> 20, t.elapsed().as_secs_f64());
                Place::Local(l)
            }
            Err(e) => {
                eprintln!("{content}: not copied here ({e}): read from the NAS");
                nas()
            }
        }
    }

    /// The copy of `content` here, held (`len`: its length must be that); Err(true) for a copy of
    /// another length still there (one another job holds), Err(false) for none.
    fn have(&self, content: &str, len: Option<u64>) -> Result<PathBuf, bool> {
        let l = self.blobs.have(content).ok_or(false)?;
        let Some(want) = len else { return Ok(l) };
        if std::fs::metadata(&l).is_ok_and(|m| m.len() == want) {
            return Ok(l);
        }
        store::cachefile::release(&l);
        let gone = store::cachefile::try_remove(&l) != store::cachefile::Removed::InUse;
        eprintln!(
            "{content}: the copy here isn't the NAS's {want} bytes: {}",
            if gone { "deleted" } else { "held elsewhere, the NAS read" }
        );
        Err(!gone)
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
pub fn path(local: Option<&Local>, p: &NasPath) -> Place {
    local.map_or_else(|| Place::Nas(p.clone()), |l| l.path(p))
}

/// The bytes this Mac's cache folder `cache` holds of the shared cache (what a Mac holds of it:
/// crate::agent's `caches_here`): the basemaps' copies (`blobs/layers/basemap*`) and the depth
/// sources' packs' (`blobs/sources/fetched/<id>/*.tar`). The stored GeoTIFFs' copies (`.tif`, the
/// terrain's) are not the shared cache's: each terrain job's disk counts those it makes.
pub fn held(cache: &Path) -> u64 {
    let blobs = cache.join("blobs");
    let packs: u64 = std::fs::read_dir(blobs.join("sources/fetched"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().flatten())
        .filter(|f| f.file_name().to_string_lossy().ends_with(".tar"))
        .filter_map(|f| f.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum();
    store::cachefile::bytes_of(&blobs.join("layers"), "basemap") + packs
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
        let nr = store::nasfs::NasRoot::local_for_test(&nas);
        let at = |p: Place| match p {
            Place::Local(l) => l,
            Place::Nas(n) => n.unchecked_path(),
        };
        let full = Local::new(&nr, &cache, crate::agent::room::MAX_TARGET);
        assert_eq!(at(full.path(&nr.join(name))), nas.join(name));
        assert_eq!(held(&cache), 0);
        let l = Local::new(&store::nasfs::NasRoot::local_for_test(&nas), &cache, 0);
        let outside = d.path().join("elsewhere.tar");
        assert_eq!(at(l.path(&store::nasfs::NasPath::local_for_test(&outside))), outside);
        let read = store::nasio::get(store::nasio::Count::ReadBytes);
        let got = at(l.path(&nr.join(name)));
        assert_eq!(got, cache.join("blobs").join(name));
        assert!(store::nasio::get(store::nasio::Count::ReadBytes) - read >= 4096, "the copy counted as read");
        assert_eq!(std::fs::read(&got).unwrap(), vec![5u8; 4096]);
        assert_eq!(held(&cache), 4096);
        // (A stored GeoTIFF's copy is its terrain job's, not the shared cache's.)
        std::fs::create_dir_all(cache.join("blobs/sources/fetched/glo30")).unwrap();
        std::fs::write(cache.join("blobs/sources/fetched/glo30/x_DEM.tif"), [0u8; 10]).unwrap();
        assert_eq!(held(&cache), 4096);
        std::fs::remove_dir_all(&nas).unwrap();
        // (Here already: no room asked, the NAS not read.)
        assert_eq!(at(full.path(&nr.join(name))), got);
        // Its reads are the local disk's.
        let local = store::nasio::get(store::nasio::Count::Local);
        let f = store::range::PlainFile::open(&got).unwrap();
        store::range::RangeRead::read_at(&f, 0, 100).unwrap();
        assert!(store::nasio::get(store::nasio::Count::Local) - local >= 100);
        store::cachefile::release(&got);
    }
}
