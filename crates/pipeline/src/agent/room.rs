//! Room on the build Mac's disk (docs/plan.md §8): before a job starts, when the disk's free space
//! is under what the job needs (`RESERVE`, or the OSM pass's own), the local copies of what the NAS
//! keeps lose their least recently used files until it has that and a margin (`margin`: a sixth
//! more, none for the OSM pass), so the next jobs start without deleting again: Meta's canopy squares
//! (`chm10/`, ~2 GB a 10° square; scenic-metrics marks a square used when it reads it) and AWS's raw
//! terrain tiles (`aws-terrarium/`, read once per terrain run). They fill again from the NAS
//! (`sources/canopy/`, `sources/aws-terrarium/`), never from the internet.
//! - Raw tiles go a folder at a time, the least recently used folder (by its newest tile) first, and
//!   in it the oldest first, so a folder's tiles go together; canopy squares each by their own use.
//! - A file goes once the NAS's folder, listed once (sixteen at a time: a listing mostly waits on
//!   the NAS; a folder that can't be listed now keeps its files here this run), has it at the same
//!   size. One the NAS lacks, or has at another size (downloaded before it kept them, or a copy cut
//!   short), is copied there first (whole and flushed: crate::whole), and kept here when that
//!   fails; one that isn't whole itself (cut short, or a temporary file) is deleted without being
//!   kept anywhere.
//!
//! Nothing else of the cache is deleted here.

use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The free space a job starts with, at least, when the caches can make it.
pub const RESERVE: u64 = 60 << 30;

/// What's freed past a job's `need` once the disk is short of it: a sixth more (10 GB past the
/// build Mac's 60), so the jobs after it start without deleting again. (Not for the OSM pass, whose
/// need is what its conditions admitted it with.)
pub fn margin(need: u64) -> u64 {
    need / 6
}

/// NAS folders listed at once.
const LIST_AHEAD: usize = 16;

/// The caches' folders whose files may be deleted, under the agent's cache, each with the NAS's
/// store of them, under its `sources/`.
const CHEAP: [(&str, &str); 2] = [("chm10", "canopy"), ("aws-terrarium", "aws-terrarium")];
/// Bytes the cheap caches hold (what `make_room` can free).
pub fn cheap_bytes(cache: &Path) -> u64 {
    let mut files = Vec::new();
    for (d, _) in CHEAP {
        walk(&cache.join(d), &mut files);
    }
    files.iter().map(|f| f.1).sum()
}

/// When the disk has less than `need` free, deletes from the caches at `cache` until it has `need`
/// and `margin` more (or they're empty), each cheap file only once the NAS's `sources` has it; the
/// bytes deleted.
pub fn make_room(cache: &Path, sources: &Path, need: u64, margin: u64) -> Result<u64> {
    make_room_with(cache, sources, need, need + margin, &disk_free)
}

/// The NAS folders' files and their sizes, each folder listed once (a folder not there: none; one
/// that can't be listed now: None, and its files stay this run).
type Listed = HashMap<PathBuf, Option<HashMap<OsString, u64>>>;

/// What becomes of a local cache file.
#[derive(Debug, PartialEq)]
enum Fate {
    /// It may go: the NAS's store has it at the same size, or it isn't whole (cut short, or a
    /// temporary file), so it's no use anywhere.
    Go,
    /// It may go once it's copied to `.0` in the NAS's store.
    Copy(PathBuf),
    /// It stays (not a cheap file, or gone already).
    Stay,
}

/// Where local cache file `p` (under `cache/<dir>`) is kept in the NAS's store.
fn nas_path(cache: &Path, sources: &Path, p: &Path) -> Option<PathBuf> {
    let (dir, store) = CHEAP.iter().find(|(d, _)| p.starts_with(cache.join(d)))?;
    Some(sources.join(store).join(p.strip_prefix(cache.join(dir)).ok()?))
}

/// A NAS folder's files and their sizes (none when it isn't there; None when it can't be read now).
fn list(folder: &Path) -> Option<HashMap<OsString, u64>> {
    match std::fs::read_dir(folder) {
        Ok(rd) => Some(rd.flatten().filter_map(|e| Some((e.file_name(), e.metadata().ok().filter(|m| m.is_file())?.len()))).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(HashMap::new()),
        Err(_) => None,
    }
}

/// What becomes of local cache file `p` (under `cache/<dir>`), its NAS folder listed once.
fn fate(cache: &Path, sources: &Path, p: &Path, listed: &mut Listed) -> Fate {
    let Some(dest) = nas_path(cache, sources, p) else { return Fate::Stay };
    let (Some(folder), Some(name)) = (dest.parent(), dest.file_name()) else { return Fate::Stay };
    let Ok(len) = std::fs::metadata(p).map(|m| m.len()) else { return Fate::Stay };
    let Some(names) = listed.entry(folder.to_path_buf()).or_insert_with(|| list(folder)) else { return Fate::Stay };
    if names.get(name) == Some(&len) {
        return Fate::Go;
    }
    if crate::whole::is_tmp(p) || !crate::whole::file_whole(p) {
        eprintln!("room: {} isn't whole: deleted, not kept", p.display());
        return Fate::Go;
    }
    Fate::Copy(dest)
}

/// Copies local cache file `p` to `dest` in the NAS's store, whole and flushed (the local copy goes
/// next: unflushed, a power cut on the NAS could lose both); whether it's there now. Each NAS folder
/// is made once a run (`made`).
fn copy_there(p: &Path, dest: &Path, made: &mut HashSet<PathBuf>) -> bool {
    let Some(folder) = dest.parent() else { return false };
    if !made.contains(folder) {
        if std::fs::create_dir_all(folder).is_err() {
            return false;
        }
        made.insert(folder.to_path_buf());
    }
    crate::whole::copy(p, dest).is_ok()
}

/// `make_room` with the disk's free space from `free_space`: nothing when it has `need`, else
/// deleting until it has `target`.
fn make_room_with(cache: &Path, sources: &Path, need: u64, target: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>) -> Result<u64> {
    let free = free_space(cache)?;
    if free >= need {
        return Ok(0);
    }
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for (d, _) in CHEAP {
        walk(&cache.join(d), &mut files);
    }
    // (Empty files are markers, "none there", that free nothing.)
    files.retain(|f| f.1 > 0);
    // Raw tiles by folder, canopy squares each alone; the least recently used group (by its newest
    // file) first, in each the oldest first.
    let tiles = cache.join("aws-terrarium");
    let mut groups: BTreeMap<PathBuf, Vec<(SystemTime, u64, PathBuf)>> = BTreeMap::new();
    for f in files {
        let key = if f.2.starts_with(&tiles) { f.2.parent().map(Path::to_path_buf).unwrap_or_default() } else { f.2.clone() };
        groups.entry(key).or_default().push(f);
    }
    let mut groups: Vec<Vec<(SystemTime, u64, PathBuf)>> = groups.into_values().collect();
    for g in &mut groups {
        g.sort();
    }
    groups.sort_by_key(|g| g.last().map(|f| f.0));
    let mut room = Room { cache, free_space, target, short: target.saturating_sub(free), since: 0, freed: 0 };
    let (mut listed, mut made) = (Listed::new(), HashSet::new());
    for ahead in groups.chunks(LIST_AHEAD) {
        if room.enough()? {
            break;
        }
        // Their NAS folders listed at once.
        let folders: BTreeSet<PathBuf> = ahead.iter().filter_map(|g| nas_path(cache, sources, &g.first()?.2)?.parent().map(Path::to_path_buf)).filter(|f| !listed.contains_key(f)).collect();
        // (A thread that can't start, or fails, leaves its folder to be listed when it's reached.)
        std::thread::scope(|s| {
            let lists: Vec<_> = folders.iter().filter_map(|f| std::thread::Builder::new().spawn_scoped(s, move || (f.clone(), list(f))).ok()).collect();
            listed.extend(lists.into_iter().filter_map(|h| h.join().ok()));
        });
        for (_, len, p) in ahead.iter().flatten() {
            if room.enough()? {
                return Ok(room.freed);
            }
            match fate(cache, sources, p, &mut listed) {
                Fate::Go => room.delete(p, *len),
                Fate::Copy(dest) if copy_there(p, &dest, &mut made) => room.delete(p, *len),
                Fate::Copy(_) | Fate::Stay => {}
            }
        }
    }
    Ok(room.freed)
}

/// make_room_with's count of what it has deleted, against the free space it's after.
struct Room<'a> {
    cache: &'a Path,
    free_space: &'a dyn Fn(&Path) -> std::io::Result<u64>,
    target: u64,
    /// What was short of `target` when the free space was last measured, and the bytes deleted
    /// since.
    short: u64,
    since: u64,
    freed: u64,
}

impl Room<'_> {
    /// Whether the disk has `target` free: measured again once what was short is deleted, or
    /// every 2 GB (what a file held isn't always what deleting it frees: snapshots keep it).
    fn enough(&mut self) -> Result<bool> {
        if self.since < self.short.min(2 << 30) {
            return Ok(false);
        }
        let free = (self.free_space)(self.cache)?;
        if free >= self.target {
            return Ok(true);
        }
        (self.short, self.since) = (self.target - free, 0);
        Ok(false)
    }

    fn delete(&mut self, p: &Path, len: u64) {
        if std::fs::remove_file(p).is_ok() {
            self.freed += len;
            self.since += len;
        }
    }
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
        for d in ["chm10", "aws-terrarium"] {
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
        file(&c.join("dem-cache.keys.u64"), 100, 9000);
        assert_eq!(cheap_bytes(c), old + png + new);
        // A disk with 850 free plus what's deleted.
        let all = used(c);
        let disk = |base: u64| move |p: &Path| Ok(base + all - used(p));
        // Short of all but a byte of the two oldest cheap files: they go; the marker and the DEM
        // seed stay.
        let freed = make_room_with(c, nas, 850 + old + png - 1, 850 + old + png - 1, &disk(850)).unwrap();
        assert_eq!(freed, old + png);
        assert!(!c.join("chm10/old.tif").exists() && !c.join("aws-terrarium/12/1/2.png").exists());
        assert!(c.join("chm10/new.tif").exists() && c.join("chm10/none.tif").exists());
        // What went is on the NAS (copied there first: it wasn't).
        assert!(nas.join("canopy/old.tif").exists() && nas.join("aws-terrarium/12/1/2.png").exists());
        // Room enough: nothing goes.
        assert_eq!(make_room_with(c, nas, 1000, 1000, &|_| Ok(1 << 20)).unwrap(), 0);
        // Far short: every cheap file; never the DEM seed.
        make_room_with(c, nas, 1 << 40, 1 << 40, &disk(0)).unwrap();
        assert!(!c.join("chm10/new.tif").exists() && c.join("dem-cache.keys.u64").exists());
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
        let freed = make_room_with(c, nas, 10 << 20, 10 << 20, &|p| {
            calls.set(calls.get() + 1);
            Ok((8 << 20) + all - used(p))
        })
        .unwrap();
        assert_eq!(freed, 2 << 20);
        assert_eq!(calls.get(), 2);
        assert!(!c.join("chm10/0.tif").exists() && !c.join("chm10/1.tif").exists() && c.join("chm10/3.tif").exists());
    }

    #[test]
    fn a_folder_goes_whole_the_least_recently_used_first() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas");
        // Column 1 holds the oldest tile, but was used since; column 2's are all older than that.
        whole(&c.join("aws-terrarium/12/1/1.png"), 5000);
        whole(&c.join("aws-terrarium/12/1/2.png"), 100);
        let a = whole(&c.join("aws-terrarium/12/2/1.png"), 4000);
        let b = whole(&c.join("aws-terrarium/12/2/2.png"), 3000);
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        // Short of column 2: it goes, column 1 stays.
        assert_eq!(make_room_with(c, nas, a + b, a + b, &disk).unwrap(), a + b);
        assert!(c.join("aws-terrarium/12/1/1.png").exists() && c.join("aws-terrarium/12/1/2.png").exists());
        assert!(nas.join("aws-terrarium/12/2/1.png").exists() && nas.join("aws-terrarium/12/2/2.png").exists());
        // A canopy square goes by its own use, not its folder's: one read long ago goes before the
        // tiles of column 1, read since.
        let square = whole(&c.join("chm10/old.tif"), 4500);
        whole(&c.join("chm10/new.tif"), 50);
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        assert_eq!(make_room_with(c, nas, square, square, &disk).unwrap(), square);
        assert!(!c.join("chm10/old.tif").exists() && c.join("chm10/new.tif").exists() && c.join("aws-terrarium/12/1/1.png").exists());
    }

    #[test]
    fn once_short_it_frees_the_margin_too() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas");
        for i in 0..6 {
            file(&c.join(format!("chm10/{i}.tif")), 1 << 20, 100 - i);
        }
        let all = used(c);
        let disk = move |p: &Path| Ok((1 << 20) + all - used(p));
        // 1 MB free: not short of 1 MB, so nothing goes, though it's under 3 MB.
        assert_eq!(make_room_with(c, nas, 1 << 20, 3 << 20, &disk).unwrap(), 0);
        // Short of 2 MB: freed to 4 MB (three files), not just to 2 MB.
        assert_eq!(make_room_with(c, nas, 2 << 20, 4 << 20, &disk).unwrap(), 3 << 20);
        assert_eq!(margin(60 << 30), 10 << 30);
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
        assert_eq!(make_room_with(&c, &nas, 1000, 1000, &|_| Ok(0)).unwrap(), 0);
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
        make_room_with(&c, &nas, 1 << 40, 1 << 40, &|_| Ok(0)).unwrap();
        for f in ["aws-terrarium/9/1/1.png", "chm10/a.tif.m4.12.tmp", "aws-terrarium/9/1/2.png", "aws-terrarium/9/1/3.png"] {
            assert!(!c.join(f).exists(), "{f} deleted");
        }
        assert!(!nas.join("aws-terrarium/9/1/1.png").exists() && !nas.join("canopy/a.tif.m4.12.tmp").exists());
        assert_eq!(std::fs::read(nas.join("aws-terrarium/9/1/2.png")).unwrap(), vec![7u8; png.len()]);
        assert_eq!(std::fs::read(nas.join("aws-terrarium/9/1/3.png")).unwrap(), png);
    }
}
