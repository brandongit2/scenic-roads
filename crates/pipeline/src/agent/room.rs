//! Room on the build Mac's disk (docs/plan.md §8): before a job starts, while the disk's free
//! space is under what the job needs (`RESERVE`, or the OSM pass's own), the local copies of what
//! the NAS keeps lose their least recently used files: Meta's canopy squares (`chm10/`, ~2 GB a 10°
//! square; scenic-metrics marks a square used when it reads it) and AWS's raw terrain tiles
//! (`aws-terrarium/`, read once per terrain run, so oldest first). They fill again from the NAS
//! (`sources/canopy/`, `sources/aws-terrarium/`), never from the internet: a file the NAS lacks,
//! or has at another size (downloaded before it kept them, or a copy cut short), is copied there
//! first (whole: crate::whole), and kept here when that fails; one that isn't whole itself (cut
//! short, or a temporary file) is deleted without being kept anywhere. Each NAS folder is listed
//! once per run, not asked about file by file. Then,
//! last, the units' kept scenic results (`scenic-units/`, a unit's whole folder, least recently
//! kept first: losing one costs that unit's next run its reuse). The DEM samples are never deleted
//! here.

use anyhow::Result;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The free space a job starts with, at least, when the caches can make it.
pub const RESERVE: u64 = 60 << 30;

/// The caches' folders whose files may be deleted, under the agent's cache, each with the NAS's
/// store of them, under its `sources/`.
const CHEAP: [(&str, &str); 2] = [("chm10", "canopy"), ("aws-terrarium", "aws-terrarium")];
/// Kept results, deleted a unit's folder at a time, after the cheap caches.
const KEPT: &str = "scenic-units";

/// Bytes the cheap caches hold (what `make_room` can free before the kept results).
pub fn cheap_bytes(cache: &Path) -> u64 {
    let mut files = Vec::new();
    for (d, _) in CHEAP {
        walk(&cache.join(d), &mut files);
    }
    files.iter().map(|f| f.1).sum()
}

/// Deletes from the caches at `cache` until the disk has `reserve` free (or they're empty), each
/// cheap file only once the NAS's `sources` has it; the bytes deleted.
pub fn make_room(cache: &Path, sources: &Path, reserve: u64) -> Result<u64> {
    make_room_with(cache, sources, reserve, &disk_free)
}

/// The NAS folders' files and their sizes, each folder listed once (a folder not there: none).
type Listed = HashMap<PathBuf, HashMap<OsString, u64>>;

/// Whether local cache file `p` (under `cache/<dir>`) may go: the NAS's store has it at the same
/// size, or does once it's copied there (when `p` is whole); or `p` isn't whole (cut short, or a
/// temporary file), so it's no use anywhere. False when it can't be copied.
fn may_go(cache: &Path, sources: &Path, p: &Path, listed: &mut Listed) -> bool {
    let Some((dir, store)) = CHEAP.iter().find(|(d, _)| p.starts_with(cache.join(d))) else { return false };
    let Ok(rel) = p.strip_prefix(cache.join(dir)) else { return false };
    let dest = sources.join(store).join(rel);
    let (Some(folder), Some(name)) = (dest.parent(), dest.file_name()) else { return false };
    let Ok(len) = std::fs::metadata(p).map(|m| m.len()) else { return false };
    let names = listed.entry(folder.to_path_buf()).or_insert_with(|| {
        std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Some((e.file_name(), e.metadata().ok().filter(|m| m.is_file())?.len())))
            .collect()
    });
    if names.get(name) == Some(&len) {
        return true;
    }
    if crate::whole::is_tmp(p) || !crate::whole::file_whole(p) {
        eprintln!("room: {} isn't whole: deleted, not kept", p.display());
        return true;
    }
    let ok = std::fs::create_dir_all(folder).is_ok() && crate::whole::copy(p, &dest).is_ok();
    if ok {
        names.insert(name.to_os_string(), len);
    }
    ok
}

fn make_room_with(cache: &Path, sources: &Path, reserve: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>) -> Result<u64> {
    let free = free_space(cache)?;
    if free >= reserve {
        return Ok(0);
    }
    let mut short = reserve - free;
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for (d, _) in CHEAP {
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
    let mut listed = Listed::new();
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
        let ok = if whole { std::fs::remove_dir_all(&p).is_ok() } else { may_go(cache, sources, &p, &mut listed) && std::fs::remove_file(&p).is_ok() };
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
        bytes(p, &vec![0u8; len], age_s);
    }

    fn bytes(p: &Path, b: &[u8], age_s: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
        let f = std::fs::File::options().append(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
    }

    /// A whole file of `p`'s kind (crate::whole), `age_s` old; its length.
    fn whole(p: &Path, age_s: u64) -> u64 {
        let b = if p.extension().is_some_and(|x| x == "png") { crate::whole::testfiles::png() } else { crate::whole::testfiles::tiff(false) };
        bytes(p, &b, age_s);
        b.len() as u64
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
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas");
        let old = whole(&c.join("chm10/old.tif"), 3000);
        file(&c.join("chm10/none.tif"), 0, 4000);
        let png = whole(&c.join("aws-terrarium/12/1/2.png"), 2000);
        let new = whole(&c.join("chm10/new.tif"), 10);
        file(&c.join("scenic-units/6-1-2/basis.json"), 100, 9000);
        file(&c.join("dem-units/6-1-2.dem"), 100, 9000);
        assert_eq!(cheap_bytes(c), old + png + new);
        // A disk with 850 free plus what's deleted.
        let all = used(c);
        let disk = |base: u64| move |p: &Path| Ok(base + all - used(p));
        // Short of all but a byte of the two oldest cheap files: they go; the marker, the kept
        // results and the DEM samples stay.
        let freed = make_room_with(c, nas, 850 + old + png - 1, &disk(850)).unwrap();
        assert_eq!(freed, old + png);
        assert!(!c.join("chm10/old.tif").exists() && !c.join("aws-terrarium/12/1/2.png").exists());
        assert!(c.join("chm10/new.tif").exists() && c.join("chm10/none.tif").exists() && c.join("scenic-units/6-1-2").exists());
        // What went is on the NAS (copied there first: it wasn't).
        assert!(nas.join("canopy/old.tif").exists() && nas.join("aws-terrarium/12/1/2.png").exists());
        // Room enough: nothing goes.
        assert_eq!(make_room_with(c, nas, 1000, &|_| Ok(1 << 20)).unwrap(), 0);
        // Far short: every cheap file, then the kept results; never the DEM samples.
        make_room_with(c, nas, 1 << 40, &disk(0)).unwrap();
        assert!(!c.join("chm10/new.tif").exists() && !c.join("scenic-units/6-1-2").exists() && c.join("dem-units/6-1-2.dem").exists());
        assert!(disk_free(c).unwrap() > 0);
    }

    #[test]
    fn it_stops_once_the_disk_has_room() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas");
        for i in 0..4 {
            file(&c.join(format!("chm10/{i}.tif")), 1 << 20, 100 - i);
        }
        // 2 MB short: two files, the free space measured again, and it stops.
        let calls = Cell::new(0);
        let all = used(c);
        let freed = make_room_with(c, nas, 10 << 20, &|p| {
            calls.set(calls.get() + 1);
            Ok((8 << 20) + all - used(p))
        })
        .unwrap();
        assert_eq!(freed, 2 << 20);
        assert_eq!(calls.get(), 2);
        assert!(!c.join("chm10/0.tif").exists() && !c.join("chm10/1.tif").exists() && c.join("chm10/3.tif").exists());
    }
}

#[cfg(test)]
mod nas_tests {
    use super::*;

    #[test]
    fn a_file_the_nas_cant_take_stays() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("cache");
        std::fs::create_dir_all(c.join("chm10")).unwrap();
        std::fs::write(c.join("chm10/a.tif"), crate::whole::testfiles::tiff(false)).unwrap();
        // The NAS's store is a file, not a folder: nothing can be copied there.
        let nas = d.path().join("nas");
        std::fs::write(&nas, b"").unwrap();
        assert_eq!(make_room_with(&c, &nas, 1000, &|_| Ok(0)).unwrap(), 0);
        assert!(c.join("chm10/a.tif").exists());
    }

    #[test]
    fn only_whole_files_are_kept_on_the_nas() {
        let d = tempfile::tempdir().unwrap();
        let (c, nas) = (d.path().join("cache"), d.path().join("nas"));
        let png = crate::whole::testfiles::png();
        let put = |p: &Path, b: &[u8]| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b).unwrap();
        };
        // Cut short, or a temporary file: deleted here, not kept there.
        put(&c.join("aws-terrarium/9/1/1.png"), &png[..png.len() - 5]);
        put(&c.join("chm10/a.tif.m4.12.tmp"), &crate::whole::testfiles::tiff(false));
        // The NAS has it at the same size: deleted here, the NAS's copy left as it is.
        put(&c.join("aws-terrarium/9/1/2.png"), &png);
        put(&nas.join("aws-terrarium/9/1/2.png"), &vec![7u8; png.len()]);
        // The NAS has it cut short: its copy replaced with this whole one.
        put(&c.join("aws-terrarium/9/1/3.png"), &png);
        put(&nas.join("aws-terrarium/9/1/3.png"), &png[..10]);
        make_room_with(&c, &nas, 1 << 40, &|_| Ok(0)).unwrap();
        for f in ["aws-terrarium/9/1/1.png", "chm10/a.tif.m4.12.tmp", "aws-terrarium/9/1/2.png", "aws-terrarium/9/1/3.png"] {
            assert!(!c.join(f).exists(), "{f} deleted");
        }
        assert!(!nas.join("aws-terrarium/9/1/1.png").exists() && !nas.join("canopy/a.tif.m4.12.tmp").exists());
        assert_eq!(std::fs::read(nas.join("aws-terrarium/9/1/2.png")).unwrap(), vec![7u8; png.len()]);
        assert_eq!(std::fs::read(nas.join("aws-terrarium/9/1/3.png")).unwrap(), png);
    }
}
