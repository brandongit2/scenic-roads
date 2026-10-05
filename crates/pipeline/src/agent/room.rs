//! Room on the build Mac's disk (docs/plan.md §8): before a job starts, when the disk's free space
//! is under what the job needs (`RESERVE`; a terrain run more; the OSM pass, its own), the local
//! copies of what the NAS
//! keeps lose files until it has that and a margin (`margin`: a sixth more, none for the OSM pass),
//! so the next jobs start without deleting again: Meta's canopy squares (`chm10/`, ~2 GB a 10°
//! square; scenic-metrics marks a square used when it reads it), AWS's raw terrain tiles
//! (`aws-terrarium/`, read once per terrain run), and the copies of the records' files staging
//! reads (`blobs/`, store::blobs). They fill again from the NAS (`sources/canopy/`,
//! `sources/aws-terrarium/`, the store), never from the internet.
//! - Canopy squares and copies of the records' files not read in the last hour go first, each by its
//!   own use, the least recently used first. A copy of a recorded file goes without asking the NAS
//!   (the records name only files it has). One listing of the NAS's canopy folder answers for every square's files (hundreds of MB
//!   each), while each raw tile folder takes a listing of its own for ~14 MB: seconds each when the
//!   NAS is busy, hours for tens of GB.
//! - Then raw tiles a folder at a time and the squares read since, together, the least recently
//!   used first (a folder by its newest tile, and in it the oldest first, so a folder's tiles go
//!   together): the squares of the area being built, which the next jobs read again, outlast idle
//!   tiles.
//! - A file goes once the NAS's folder, listed once (sixteen at a time: a listing mostly waits on
//!   the NAS; a folder that can't be listed now, or whose listing is cut short, keeps its raw tiles
//!   here this run, and has each canopy square asked about alone), has it at the same size (asked
//!   about once more when the listing lacks it). A canopy square the NAS lacks, or has at another
//!   size (downloaded before it kept them, or a copy cut short), is copied there first (whole and
//!   flushed: crate::whole: one large file), and kept here when that fails; a raw tile it lacks
//!   stays here until it reaches the NAS in bulk (copied a tile at a time, with a flush each, small
//!   files stall the NAS: ~23 a second, and both Macs' processes wait on it meanwhile). One that
//!   isn't whole itself (cut short, or a temporary file) is deleted without being kept anywhere.
//! - Raw tiles the NAS lacks are packed onto it first (crate::rawpack: an archive of their own for
//!   each area, one large write each, none kept here), then go; the copies of its archives here
//!   (`aws-terrarium/packs/`) go as the copies of the records' files do, each by its own use (a
//!   job marks one used when it opens it), without asking it.
//!
//! - A file the job reads at once (`spare`: a terrain run's own area's archive copies) stays.
//!
//! It ends early when the agent is asked to stop. Nothing else of the cache is deleted here.

use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The free space a job starts with, at least, when the caches can make it.
pub const RESERVE: u64 = 30 << 30;

/// What's freed past a job's `need` once the disk is short of it: a sixth more (5 GB past the
/// build Mac's 30), so the jobs after it start without deleting again. (Not for the OSM pass, whose
/// need is what its conditions admitted it with.)
pub fn margin(need: u64) -> u64 {
    need / 6
}

/// NAS folders listed at once.
const LIST_AHEAD: usize = 16;

/// Canopy squares read this lately (the area being built's, which the next jobs read again) wait
/// with the raw tiles, in least recently used order; the others go before any tile.
const RECENT: std::time::Duration = std::time::Duration::from_secs(3600);

/// The caches' folders whose files may be deleted, under the agent's cache, each with the NAS's
/// store of them, under its `sources/`; and `blobs/`, this Mac's copies of files the records name
/// (store::blobs), which the NAS has by construction: they go without asking it.
const CHEAP: [(&str, &str); 3] = [("chm10", "canopy"), ("aws-terrarium", "aws-terrarium"), ("blobs", "")];
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
pub fn make_room(cache: &Path, sources: &Path, need: u64, margin: u64, spare: &dyn Fn(&Path) -> bool) -> Result<u64> {
    make_room_with(cache, sources, need, need + margin, &disk_free, spare)
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
    // (Copies of the records' files, and of the raw tiles' archives, have no NAS folder to list:
    // they go as they are.)
    if p.starts_with(cache.join("aws-terrarium/packs")) {
        return None;
    }
    let (dir, store) = CHEAP.iter().find(|(d, s)| !s.is_empty() && p.starts_with(cache.join(d)))?;
    Some(sources.join(store).join(p.strip_prefix(cache.join(dir)).ok()?))
}

/// A NAS folder's files and their sizes (none when it isn't there; None when it can't be read now,
/// or its listing was cut short: a busy NAS's timeout midway, which would make the files it didn't
/// get to look missing there).
fn list(folder: &Path) -> Option<HashMap<OsString, u64>> {
    let rd = match std::fs::read_dir(folder) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(HashMap::new()),
        Err(_) => return None,
    };
    let mut names = HashMap::new();
    for e in rd {
        let e = e.ok()?;
        match e.metadata() {
            Ok(m) if m.is_file() => {
                names.insert(e.file_name(), m.len());
            }
            Ok(_) => {}
            // (Gone since it was listed: a temporary file renamed.)
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    Some(names)
}

/// What becomes of local cache file `p` (under `cache/<dir>`), its NAS folder listed once.
fn fate(cache: &Path, sources: &Path, p: &Path, listed: &mut Listed) -> Fate {
    // (Copies of files the NAS has by construction: the records', and the raw tiles' archives.)
    if p.starts_with(cache.join("blobs")) || p.starts_with(cache.join("aws-terrarium/packs")) {
        return Fate::Go;
    }
    let Some(dest) = nas_path(cache, sources, p) else { return Fate::Stay };
    let (Some(folder), Some(name)) = (dest.parent(), dest.file_name()) else { return Fate::Stay };
    let Ok(len) = std::fs::metadata(p).map(|m| m.len()) else { return Fate::Stay };
    let there = || std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == len);
    let Some(names) = listed.entry(folder.to_path_buf()).or_insert_with(|| list(folder)) else {
        // (Its folder can't be listed now: a canopy square, hundreds of MB, is asked about alone;
        // a raw tile stays this run, a question each costing about what the listing would.)
        return if !p.starts_with(cache.join("aws-terrarium")) && there() { Fate::Go } else { Fate::Stay };
    };
    // Missing from the listing: asked about once more before it's copied (a short listing, a
    // busy NAS's, without an error).
    if names.get(name) == Some(&len) || there() {
        return Fate::Go;
    }
    if crate::whole::is_tmp(p) || !crate::whole::file_whole(p) {
        eprintln!("room: {} isn't whole: deleted, not kept", p.display());
        return Fate::Go;
    }
    // (A raw tile the NAS lacks waits here to reach it in bulk.)
    if p.starts_with(cache.join("aws-terrarium")) {
        return Fate::Stay;
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
fn make_room_with(cache: &Path, sources: &Path, need: u64, target: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>, spare: &dyn Fn(&Path) -> bool) -> Result<u64> {
    let free = free_space(cache)?;
    if free >= need {
        return Ok(0);
    }
    // Raw tiles the NAS lacks, packed onto it first (an archive an area: large writes).
    if let Some(root) = sources.parent() {
        if let Err(e) = crate::rawpack::pack_local(&cache.join("aws-terrarium"), &sources.join("aws-terrarium"), root, false) {
            eprintln!("room: raw tiles not packed now ({e:#}); they stay");
        }
    }
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for (d, _) in CHEAP {
        walk(&cache.join(d), &mut files);
    }
    // (Empty files are markers, "none there", that free nothing; what the job reads at once stays.)
    files.retain(|f| f.1 > 0 && !spare(&f.2));
    // Raw tiles by folder, canopy squares each alone. The squares not read lately first, then the
    // tiles and the squares read since, together: each the least recently used group (by its
    // newest file) first, in each the oldest first.
    let tiles = cache.join("aws-terrarium");
    let packs = tiles.join("packs");
    let lately = SystemTime::now().checked_sub(RECENT).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut groups: BTreeMap<PathBuf, Vec<(SystemTime, u64, PathBuf)>> = BTreeMap::new();
    for f in files {
        // (An area's archive, each by its own use, as a canopy square.)
        let key = if f.2.starts_with(&tiles) && !f.2.starts_with(&packs) { f.2.parent().map(Path::to_path_buf).unwrap_or_default() } else { f.2.clone() };
        groups.entry(key).or_default().push(f);
    }
    let mut groups: Vec<Vec<(SystemTime, u64, PathBuf)>> = groups.into_values().collect();
    for g in &mut groups {
        g.sort();
    }
    groups.sort_by_key(|g| {
        let newest = g.last().map(|f| f.0);
        let idle_square = !g.first().is_some_and(|f| f.2.starts_with(&tiles) && !f.2.starts_with(&packs)) && newest.is_some_and(|t| t < lately);
        (!idle_square, newest)
    });
    let mut room = Room { cache, free_space, target, short: target.saturating_sub(free), since: 0, freed: 0 };
    let (mut listed, mut made) = (Listed::new(), HashSet::new());
    for ahead in groups.chunks(LIST_AHEAD) {
        // (Asked to stop: the room made so far does.)
        if room.enough()? || super::stopping() {
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
            if room.enough()? || super::stopping() {
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

    fn make_room_spared(cache: &Path, sources: &Path, need: u64, target: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>) -> Result<u64> {
        make_room_with(cache, sources, need, target, free_space, &|_| false)
    }
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
        for d in ["chm10", "aws-terrarium", "blobs"] {
            walk(&c.join(d), &mut fs);
        }
        fs.iter().map(|f| f.1).sum()
    }

    #[test]
    fn what_the_job_reads_at_once_stays() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas");
        // The job's own area's archive, idle longest, and another area's.
        let own = whole(&c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles"), 9000);
        let other = whole(&c.join("aws-terrarium/packs/6-9-9.0000000000000002.tiles"), 7200);
        let all = used(c);
        let disk = |base: u64| move |p: &Path| Ok(base + all - used(p));
        let spare = |p: &Path| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("6-1-1."));
        // Short of one archive: the other area's goes, though the job's own was idle longer.
        assert_eq!(make_room_with(c, nas, 1000 + own, 1000 + own, &disk(1000), &spare).unwrap(), other);
        assert!(c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles").exists() && !c.join("aws-terrarium/packs/6-9-9.0000000000000002.tiles").exists());
        // Far short: still not the job's own.
        make_room_with(c, nas, 1 << 40, 1 << 40, &disk(0), &spare).unwrap();
        assert!(c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles").exists());
    }

    #[test]
    fn idle_copies_go_first_then_those_read_lately() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas");
        // An area's archive idle longest, a canopy square idle two hours, another read a minute ago.
        let arch = whole(&c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles"), 9000);
        let idle = whole(&c.join("chm10/idle.tif"), 7200);
        file(&c.join("chm10/none.tif"), 0, 8000);
        let read = whole(&c.join("chm10/read.tif"), 60);
        file(&c.join("dem-cache.keys.u64"), 100, 9000);
        assert_eq!(cheap_bytes(c), arch + idle + read);
        // A disk with 850 free plus what's deleted.
        let all = used(c);
        let disk = |base: u64| move |p: &Path| Ok(base + all - used(p));
        // Short of the archive and all but a byte of the idle square: both go, the archive (idle
        // longer, and the NAS has it: nothing asked) first; the marker and the DEM seed stay.
        assert_eq!(make_room_spared(c, nas, 850 + arch + idle - 1, 850 + arch + idle - 1, &disk(850)).unwrap(), arch + idle);
        assert!(!c.join("chm10/idle.tif").exists() && !c.join("aws-terrarium/packs/6-1-1.0000000000000001.tiles").exists());
        assert!(c.join("chm10/read.tif").exists() && c.join("chm10/none.tif").exists());
        // What went is on the NAS (the square copied there first: it wasn't).
        assert!(nas.join("canopy/idle.tif").exists());
        // Room enough: nothing goes.
        assert_eq!(make_room_spared(c, nas, 1000, 1000, &|_| Ok(1 << 20)).unwrap(), 0);
        // Far short: every cheap file; never the DEM seed.
        make_room_spared(c, nas, 1 << 40, 1 << 40, &disk(0)).unwrap();
        assert!(!c.join("chm10/read.tif").exists() && c.join("dem-cache.keys.u64").exists());
        assert!(disk_free(c).unwrap() > 0);
    }

    #[test]
    fn raw_tiles_are_packed_onto_the_nas_before_they_go() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let root = &d.path().join("nas");
        let nas = &root.join("sources");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // Tiles AWS gave, the NAS without them; one too new to pack.
        let a = whole(&c.join("aws-terrarium/12/2048/1365.png"), 9000);
        whole(&c.join("aws-terrarium/12/2048/1366.png"), 9000);
        whole(&c.join("aws-terrarium/12/2049/1365.png"), 1);
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        make_room_spared(c, nas, a, a, &disk).unwrap();
        // Packed onto the NAS (an archive for their area, named in the index), gone here.
        let index = crate::rawpack::Index::load(&nas.join("aws-terrarium")).unwrap();
        let name = &index.of("6-32-21")[0].name;
        assert!(nas.join("aws-terrarium/packs").join(name).exists());
        assert!(!c.join("aws-terrarium/packs").join(name).exists(), "the disk is short: not kept here");
        assert!(!c.join("aws-terrarium/12/2048/1365.png").exists() && !c.join("aws-terrarium/12/2048/1366.png").exists());
        // The newest waits here: the NAS hasn't it (packed later).
        assert!(c.join("aws-terrarium/12/2049/1365.png").exists());
        assert!(!nas.join("aws-terrarium/12/2049/1365.png").exists(), "never copied alone");
    }

    #[test]
    fn copies_of_the_records_files_go_without_asking_the_nas() {
        let d = tempfile::tempdir().unwrap();
        let c = &d.path().join("cache");
        let nas = &d.path().join("nas");
        // An idle copy of a recorded pack, and a square read since: no NAS folder to ask exists.
        file(&c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack"), 1000, 7200);
        let square = whole(&c.join("chm10/read.tif"), 60);
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        assert_eq!(make_room_spared(c, nas, 1000, 1000, &disk).unwrap(), 1000);
        assert!(!c.join("blobs/layers/terrain/hi/6-1-2.0000000000000001.pack").exists() && c.join("chm10/read.tif").exists());
        assert!(!nas.exists(), "nothing was listed or copied there");
        assert_eq!(used(c), square);
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
        let freed = make_room_spared(c, nas, 10 << 20, 10 << 20, &|p| {
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
        // (The NAS takes no archive here: tiles it lacks stay, and those it has loose follow the
        // rules for loose files.)
        std::fs::create_dir_all(nas.join("aws-terrarium")).unwrap();
        std::fs::write(nas.join("aws-terrarium/packs"), b"").unwrap();
        // Column 1 holds the oldest tile, but was used since; column 2's are all older than that.
        // (The NAS has them all.)
        for (t, age) in [("1/1", 5000), ("1/2", 100), ("2/1", 4000), ("2/2", 3000), ("3/1", 9500), ("3/2", 9000)] {
            whole(&nas.join(format!("aws-terrarium/12/{t}.png")), 0);
            if !t.starts_with('3') {
                whole(&c.join(format!("aws-terrarium/12/{t}.png")), age);
            }
        }
        let (a, b) = (std::fs::metadata(c.join("aws-terrarium/12/2/1.png")).unwrap().len(), std::fs::metadata(c.join("aws-terrarium/12/2/2.png")).unwrap().len());
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        // Short of column 2: it goes, column 1 stays.
        assert_eq!(make_room_spared(c, nas, a + b, a + b, &disk).unwrap(), a + b);
        assert!(c.join("aws-terrarium/12/1/1.png").exists() && c.join("aws-terrarium/12/1/2.png").exists());
        assert!(!c.join("aws-terrarium/12/2/1.png").exists() && !c.join("aws-terrarium/12/2/2.png").exists());
        // Canopy squares idle an hour go before any tile, each by its own use, not its folder's: of
        // two in chm10/, the one read longer ago goes, before column 3, idle longer still; the
        // other stays.
        whole(&c.join("aws-terrarium/12/3/1.png"), 9500);
        whole(&c.join("aws-terrarium/12/3/2.png"), 9000);
        let square = whole(&c.join("chm10/old.tif"), 4500);
        whole(&c.join("chm10/new.tif"), 4000);
        let all = used(c);
        let disk = move |p: &Path| Ok(all - used(p));
        assert_eq!(make_room_spared(c, nas, square, square, &disk).unwrap(), square);
        assert!(!c.join("chm10/old.tif").exists() && c.join("chm10/new.tif").exists());
        assert!(c.join("aws-terrarium/12/3/1.png").exists() && c.join("aws-terrarium/12/1/1.png").exists());
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
        assert_eq!(make_room_spared(c, nas, 1 << 20, 3 << 20, &disk).unwrap(), 0);
        // Short of 2 MB: freed to 4 MB (three files), not just to 2 MB.
        assert_eq!(make_room_spared(c, nas, 2 << 20, 4 << 20, &disk).unwrap(), 3 << 20);
        assert_eq!(margin(30 << 30), 5 << 30);
    }
}

#[cfg(test)]
mod nas_tests {
    use super::*;

    #[test]
    fn a_file_a_short_listing_lacks_is_asked_about_before_a_copy() {
        let d = tempfile::tempdir().unwrap();
        let (c, nas) = (d.path().join("cache"), d.path().join("nas"));
        let png = crate::whole::testfiles::png();
        for p in [c.join("aws-terrarium/9/1/2.png"), nas.join("aws-terrarium/9/1/2.png")] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, &png).unwrap();
        }
        // The folder's listing came back without it (cut short): it's on the NAS, so it may go.
        let mut listed = Listed::new();
        listed.insert(nas.join("aws-terrarium/9/1"), Some(HashMap::new()));
        assert_eq!(fate(&c, &nas, &c.join("aws-terrarium/9/1/2.png"), &mut listed), Fate::Go);
        // A folder that couldn't be listed keeps its raw tiles.
        listed.insert(nas.join("aws-terrarium/9/1"), None);
        assert_eq!(fate(&c, &nas, &c.join("aws-terrarium/9/1/2.png"), &mut listed), Fate::Stay);
        // Its canopy squares are asked about alone: one the NAS has at its size may go, one it
        // lacks stays.
        let tif = crate::whole::testfiles::tiff(false);
        for p in [c.join("chm10/a.tif"), nas.join("canopy/a.tif"), c.join("chm10/b.tif")] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, &tif).unwrap();
        }
        listed.insert(nas.join("canopy"), None);
        assert_eq!(fate(&c, &nas, &c.join("chm10/a.tif"), &mut listed), Fate::Go);
        assert_eq!(fate(&c, &nas, &c.join("chm10/b.tif"), &mut listed), Fate::Stay);
    }

    #[test]
    fn a_file_the_nas_cant_take_stays() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("cache");
        std::fs::create_dir_all(c.join("chm10")).unwrap();
        std::fs::write(c.join("chm10/a.tif"), crate::whole::testfiles::tiff(false)).unwrap();
        // The NAS's store is a file, not a folder: nothing can be copied there.
        let nas = d.path().join("nas");
        std::fs::write(&nas, b"").unwrap();
        assert_eq!(make_room_with(&c, &nas, 1000, 1000, &|_| Ok(0), &|_| false).unwrap(), 0);
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
        // The NAS has it cut short: a canopy square's copy replaced with this whole one; a raw tile
        // kept here, to reach it in bulk.
        let tif = crate::whole::testfiles::tiff(false);
        put(&c.join("chm10/b.tif"), &tif);
        put(&nas.join("canopy/b.tif"), &tif[..10]);
        put(&c.join("aws-terrarium/9/1/3.png"), &png);
        put(&nas.join("aws-terrarium/9/1/3.png"), &png[..10]);
        make_room_with(&c, &nas, 1 << 40, 1 << 40, &|_| Ok(0), &|_| false).unwrap();
        for f in ["aws-terrarium/9/1/1.png", "chm10/a.tif.m4.12.tmp", "aws-terrarium/9/1/2.png", "chm10/b.tif"] {
            assert!(!c.join(f).exists(), "{f} deleted");
        }
        assert!(!nas.join("aws-terrarium/9/1/1.png").exists() && !nas.join("canopy/a.tif.m4.12.tmp").exists());
        assert_eq!(std::fs::read(nas.join("aws-terrarium/9/1/2.png")).unwrap(), vec![7u8; png.len()]);
        assert_eq!(std::fs::read(nas.join("canopy/b.tif")).unwrap(), tif);
        assert!(c.join("aws-terrarium/9/1/3.png").exists());
        assert_eq!(std::fs::read(nas.join("aws-terrarium/9/1/3.png")).unwrap(), png[..10].to_vec());
    }
}
