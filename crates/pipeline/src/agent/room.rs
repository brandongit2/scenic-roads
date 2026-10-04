//! Room on the build Mac's disk (docs/plan.md §8): before a job starts, while the disk's free
//! space is under what the job needs (`RESERVE`, or the OSM pass's own), the caches that are cheap
//! to fill again lose their least recently used files: Meta's canopy squares (`chm10/`, ~2 GB a
//! 10° square, downloaded again in a minute or so; scenic-metrics marks a square used when it
//! reads it) and AWS's raw terrain tiles (`aws-terrarium/`, read once per terrain run, so oldest
//! downloaded first). Then, last, the units' kept scenic results (`scenic-units/`, a unit's whole
//! folder, least recently kept first: losing one costs that unit's next run its reuse). The DEM
//! samples are never deleted here.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The free space a job starts with, at least, when the caches can make it.
pub const RESERVE: u64 = 60 << 30;

/// The caches' folders whose files may be deleted, under the agent's cache.
const CHEAP: [&str; 2] = ["chm10", "aws-terrarium"];
/// Kept results, deleted a unit's folder at a time, after the cheap caches.
const KEPT: &str = "scenic-units";

/// Bytes the cheap caches hold (what `make_room` can free before the kept results).
pub fn cheap_bytes(cache: &Path) -> u64 {
    let mut files = Vec::new();
    for d in CHEAP {
        walk(&cache.join(d), &mut files);
    }
    files.iter().map(|f| f.1).sum()
}

/// Deletes from the caches at `cache` until the disk has `reserve` free (or they're empty); the
/// bytes deleted.
pub fn make_room(cache: &Path, reserve: u64) -> Result<u64> {
    make_room_with(cache, reserve, &disk_free)
}

fn make_room_with(cache: &Path, reserve: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>) -> Result<u64> {
    let free = free_space(cache)?;
    if free >= reserve {
        return Ok(0);
    }
    let mut short = reserve - free;
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for d in CHEAP {
        walk(&cache.join(d), &mut files);
    }
    // (Empty files are markers, "none there", that free nothing.)
    files.retain(|f| f.1 > 0);
    files.sort();
    // Then the kept results, a unit at a time, by when they were kept.
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = std::fs::read_dir(cache.join(KEPT))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let t = std::fs::metadata(e.path().join("basis.json")).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let mut fs = Vec::new();
            walk(&e.path(), &mut fs);
            (t, fs.iter().map(|f| f.1).sum(), e.path())
        })
        .collect();
    kept.sort();
    let mut freed = 0u64;
    let mut since = 0u64;
    let items = files.into_iter().map(|(_, len, p)| (len, p, false)).chain(kept.into_iter().map(|(_, len, p)| (len, p, true)));
    for (len, p, whole) in items {
        // Once what was short is deleted, or every 2 GB, the free space measured again: what a
        // file held isn't always what deleting it frees (snapshots keep it).
        if since >= short.min(2 << 30) {
            let free = free_space(cache)?;
            if free >= reserve {
                return Ok(freed);
            }
            (short, since) = (reserve - free, 0);
        }
        let ok = if whole { std::fs::remove_dir_all(&p).is_ok() } else { std::fs::remove_file(&p).is_ok() };
        if ok {
            freed += len;
            since += len;
        }
    }
    Ok(freed)
}

fn walk(dir: &Path, out: &mut Vec<(SystemTime, u64, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk(&e.path(), out);
        } else {
            out.push((md.modified().unwrap_or(SystemTime::UNIX_EPOCH), md.len(), e.path()));
        }
    }
}

/// The free space on the disk holding `path` (a local disk).
pub fn disk_free(path: &Path) -> std::io::Result<u64> {
    super::cond::free_bytes(path).ok_or_else(std::io::Error::last_os_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Duration;

    fn file(p: &Path, len: usize, age_s: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![0u8; len]).unwrap();
        let f = std::fs::File::options().append(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
    }

    /// The bytes under the caches' folders.
    fn used(c: &Path) -> u64 {
        let mut fs = Vec::new();
        for d in ["chm10", "aws-terrarium", "scenic-units", "dem-units"] {
            walk(&c.join(d), &mut fs);
        }
        fs.iter().map(|f| f.1).sum()
    }

    #[test]
    fn the_least_recently_used_cheap_files_go_first() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path();
        file(&c.join("chm10/old.tif"), 100, 3000);
        file(&c.join("chm10/none.tif"), 0, 4000);
        file(&c.join("aws-terrarium/12/1/2.png"), 100, 2000);
        file(&c.join("chm10/new.tif"), 100, 10);
        file(&c.join("scenic-units/6-1-2/basis.json"), 100, 9000);
        file(&c.join("dem-units/6-1-2.dem"), 100, 9000);
        assert_eq!(cheap_bytes(c), 300);
        // A disk with 850 free plus what's deleted.
        let all = used(c);
        let disk = |base: u64| move |p: &Path| Ok(base + all - used(p));
        // 150 bytes short: the two oldest cheap files go; the marker, the kept results and the DEM
        // samples stay.
        let freed = make_room_with(c, 1000, &disk(850)).unwrap();
        assert_eq!(freed, 200);
        assert!(!c.join("chm10/old.tif").exists() && !c.join("aws-terrarium/12/1/2.png").exists());
        assert!(c.join("chm10/new.tif").exists() && c.join("chm10/none.tif").exists() && c.join("scenic-units/6-1-2").exists());
        // Room enough: nothing goes.
        assert_eq!(make_room_with(c, 1000, &|_| Ok(5000)).unwrap(), 0);
        // Far short: every cheap file, then the kept results; never the DEM samples.
        make_room_with(c, 1 << 40, &disk(0)).unwrap();
        assert!(!c.join("chm10/new.tif").exists() && !c.join("scenic-units/6-1-2").exists() && c.join("dem-units/6-1-2.dem").exists());
        assert!(disk_free(c).unwrap() > 0);
    }

    #[test]
    fn it_stops_once_the_disk_has_room() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path();
        for i in 0..4 {
            file(&c.join(format!("chm10/{i}.tif")), 1 << 20, 100 - i);
        }
        // 2 MB short: two files, the free space measured again, and it stops.
        let calls = Cell::new(0);
        let all = used(c);
        let freed = make_room_with(c, 10 << 20, &|p| {
            calls.set(calls.get() + 1);
            Ok((8 << 20) + all - used(p))
        })
        .unwrap();
        assert_eq!(freed, 2 << 20);
        assert_eq!(calls.get(), 2);
        assert!(!c.join("chm10/0.tif").exists() && !c.join("chm10/1.tif").exists() && c.join("chm10/3.tif").exists());
    }
}
