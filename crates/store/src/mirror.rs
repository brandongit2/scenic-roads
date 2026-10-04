//! The per-Mac mirror (plan §4, "Mirror, per Mac"; docs/formats.md, "On each Mac"). While the NAS
//! is reachable, each Mac copies the files the current catalog references to local disk, under the
//! same content names, so the map reads them mmapped at today's speed and keeps working away from
//! home.
//!
//! Under the app's folder (`~/Library/Application Support/scenic/`):
//!
//! ```text
//! mirror/<content name>            complete copies, each moved here only after its hash checked out
//! mirror/.partial/<content name>   copies in progress, resumed where they stopped
//! mirror/.uses                     when each logical file was last used (JSON)
//! idx/<hash16>.idx                 pack indexes (PackIndex::to_bytes), for offline start
//! catalog/<n>.json.zst             the last catalogs adopted
//! ```
//!
//! The budget is the disk's free space minus a reserve. Copies go one file at a time, in large
//! sequential reads through the I/O pool: small worldwide files first, then root and lo packs, hi
//! data and road values, base packs, hi packs, and everything else (the basemap); most recently
//! used first within each group. When space runs short, files no recent catalog references are
//! evicted, least recently used first; files of the last two catalogs never are.

use crate::catalog::{self, Catalog};
use crate::iopool::{IoError, IoPool};
use crate::naming::{hex16, parse_content_name, replace_file};
use crate::pack::PackIndex;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Bytes per NAS read while copying.
const CHUNK: u64 = 4 << 20;
/// Shortest timeout for one chunk: long enough that a slow link isn't taken for a dead one.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
/// How often `touch` writes the use times out.
const SAVE_EVERY: Duration = Duration::from_secs(60);
/// Catalogs kept in `catalog/`.
const KEEP_CATALOGS: usize = 3;
/// Age past which a temporary file in `idx/` is left over from an interrupted write.
const STALE_TMP: Duration = Duration::from_secs(3600);
const PARTIAL: &str = ".partial";
const USES: &str = ".uses";
/// The group of files no other group claims (the basemap; anything newer).
const LAST_GROUP: u8 = 5;

type FreeSpace = dyn Fn(&Path) -> io::Result<u64> + Send + Sync;

/// What a `sync` did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Files copied and verified.
    pub copied: u32,
    pub copied_bytes: u64,
    /// Files left out because they don't fit the budget.
    pub skipped: u32,
    pub skipped_bytes: u64,
    /// Files whose copy failed (logged; retried next time).
    pub failed: u32,
    /// Local files deleted to make room.
    pub evicted: u32,
    pub evicted_bytes: u64,
    /// Catalog files still not local when the sync ended.
    pub pending: u32,
    pub end: SyncEnd,
}

/// How a `sync` ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncEnd {
    /// Went through every file.
    #[default]
    Done,
    /// `pause` asked it to stop; a copy in progress resumes next time.
    Paused,
    /// The NAS went (or was) offline.
    Offline,
}

enum Copy {
    Done,
    Paused,
    Offline,
    Failed(String),
}

/// A file the catalog references that isn't local yet.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Want {
    name: String,
    size: u64,
    group: u8,
    used: u64,
}

struct State {
    /// Complete local copies: content name → size.
    files: HashMap<String, u64>,
    /// Logical name → last use (milliseconds since 1970, strictly increasing per touch).
    uses: HashMap<String, u64>,
    last_stamp: u64,
    dirty: bool,
    saved: Instant,
}

/// The local copy of the NAS's current files on one Mac.
pub struct Mirror {
    root: PathBuf,
    reserve: u64,
    state: Mutex<State>,
    free_space: Box<FreeSpace>,
}

impl Mirror {
    /// Opens (creating as needed) the mirror under `root`, the app's folder. `reserve_bytes` of
    /// the disk stay free (plan §4: 50 GB; 150 GB on the build Mac, so builds have room).
    pub fn open(root: PathBuf, reserve_bytes: u64) -> Result<Mirror> {
        for d in ["mirror", "idx", "catalog"] {
            let p = root.join(d);
            fs::create_dir_all(&p).with_context(|| format!("create {}", p.display()))?;
        }
        let mut files = HashMap::new();
        scan(&root.join("mirror"), "", &mut files).context("list the mirror")?;
        let uses_path = root.join("mirror").join(USES);
        let uses: HashMap<String, u64> = match fs::read(&uses_path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                eprintln!("mirror: ignoring unreadable {}: {e}", uses_path.display());
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        };
        let last_stamp = uses.values().copied().max().unwrap_or(0);
        Ok(Mirror {
            root,
            reserve: reserve_bytes,
            state: Mutex::new(State { files, uses, last_stamp, dirty: false, saved: Instant::now() }),
            free_space: Box::new(disk_free),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Where a local copy lives (whether or not it's there).
    pub fn path(&self, content_name: &str) -> PathBuf {
        self.root.join("mirror").join(content_name)
    }

    fn partial_path(&self, content_name: &str) -> PathBuf {
        self.root.join("mirror").join(PARTIAL).join(content_name)
    }

    /// The local copy of `content_name`, when it's here and complete.
    pub fn local(&self, content_name: &str) -> Option<PathBuf> {
        if !self.state().files.contains_key(content_name) {
            return None;
        }
        let p = self.path(content_name);
        if p.is_file() {
            return Some(p);
        }
        // Deleted behind our back.
        self.state().files.remove(content_name);
        None
    }

    /// Complete local copies and their total size.
    pub fn usage(&self) -> (usize, u64) {
        let st = self.state();
        (st.files.len(), st.files.values().sum())
    }

    /// Records a use of `content_name` (its logical name, so a newer version of the same file
    /// inherits it). Cheap: written out at most once a minute, and by `flush`.
    pub fn touch(&self, content_name: &str) {
        let Some(c) = parse_content_name(content_name) else { return };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let mut st = self.state();
        let stamp = now.max(st.last_stamp + 1);
        st.last_stamp = stamp;
        st.uses.insert(c.logical.to_string(), stamp);
        st.dirty = true;
        if st.saved.elapsed() >= SAVE_EVERY {
            drop(st);
            if let Err(e) = self.flush() {
                eprintln!("mirror: {e:#}");
            }
        }
    }

    /// Writes the use times out if they changed.
    pub fn flush(&self) -> Result<()> {
        let snapshot: BTreeMap<String, u64> = {
            let mut st = self.state();
            if !st.dirty {
                return Ok(());
            }
            st.dirty = false;
            st.saved = Instant::now();
            st.uses.iter().map(|(k, v)| (k.clone(), *v)).collect()
        };
        let p = self.root.join("mirror").join(USES);
        replace_file(&p, &serde_json::to_vec(&snapshot)?, None).with_context(|| format!("write {}", p.display()))
    }

    /// Whether pack `content_name`'s index is cached on this Mac.
    pub fn has_index(&self, content_name: &str) -> bool {
        parse_content_name(content_name).is_some_and(|c| self.root.join("idx").join(format!("{}.idx", c.hash16)).exists())
    }

    /// Pack `content_name`'s index from the local cache, else from `loader` (which reads the
    /// pack, locally or through the pool), cached for next time.
    pub fn index(&self, content_name: &str, loader: impl FnOnce() -> Result<PackIndex>) -> Result<PackIndex> {
        let c = parse_content_name(content_name).with_context(|| format!("{content_name:?} isn't a content name"))?;
        let p = self.root.join("idx").join(format!("{}.idx", c.hash16));
        match fs::read(&p) {
            Ok(b) => match PackIndex::from_bytes(&b) {
                Ok(ix) => return Ok(ix),
                Err(e) => eprintln!("mirror: dropping {}: {e:#}", p.display()),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("mirror: can't read {}: {e}", p.display()),
        }
        let ix = loader()?;
        if let Err(e) = replace_file(&p, &ix.to_bytes(), None) {
            eprintln!("mirror: can't keep {}: {e}", p.display());
        }
        Ok(ix)
    }

    /// Keeps a copy of `cat` in `catalog/` (for starting offline, and so its files stay protected
    /// while it's one of the last two), and the last few before it.
    pub fn save_catalog(&self, cat: &Catalog) -> Result<()> {
        let dir = self.root.join("catalog");
        catalog::write_copy(&dir, cat)?;
        catalog::prune(&dir, KEEP_CATALOGS)?;
        Ok(())
    }

    /// The newest catalog kept here.
    pub fn saved_catalog(&self) -> Result<Option<Catalog>> {
        catalog::latest(&self.root.join("catalog"))
    }

    /// Content names never evicted: those of `cat` and of the newest saved catalog before it.
    fn protected(&self, cat: &Catalog) -> Result<HashSet<String>> {
        let mut keep: HashSet<String> = cat.files.values().map(|f| f.file.clone()).collect();
        let dir = self.root.join("catalog");
        for n in catalog::list(&dir)?.into_iter().filter(|&n| n < cat.n) {
            match catalog::read(&dir.join(catalog::file_name(n))) {
                Ok(prev) => {
                    keep.extend(prev.files.into_values().map(|f| f.file));
                    break;
                }
                Err(e) => eprintln!("mirror: {e:#}"),
            }
        }
        Ok(keep)
    }

    /// The files of `cat` not here yet, in copy order.
    fn plan(&self, cat: &Catalog) -> Vec<Want> {
        let groups = groups(cat);
        let st = self.state();
        let mut v: Vec<Want> = Vec::new();
        for (logical, f) in &cat.files {
            if st.files.contains_key(&f.file) {
                continue;
            }
            if parse_content_name(&f.file).is_none() {
                eprintln!("mirror: catalog {} lists {:?} for {logical}, which isn't a content name", cat.n, f.file);
                continue;
            }
            let group = groups.get(logical.as_str()).copied().unwrap_or(LAST_GROUP);
            v.push(Want { name: f.file.clone(), size: f.size, group, used: st.uses.get(logical).copied().unwrap_or(0) });
        }
        v.sort_by(|a, b| a.group.cmp(&b.group).then(b.used.cmp(&a.used)).then_with(|| a.name.cmp(&b.name)));
        v
    }

    /// Copies what `cat` references and isn't here yet from `nas_root`, through `pool`, until done,
    /// `pause()` says stop, or the NAS goes offline. Run it on a background thread; it never
    /// holds the mirror's lock while it waits on the NAS.
    pub fn sync(&self, cat: &Catalog, nas_root: &Path, pool: &IoPool, pause: &dyn Fn() -> bool) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        let protected = self.protected(cat)?;
        self.drop_mismatched(cat);
        // Honour the reserve first: the user may have filled the disk since last time.
        self.make_room(0, &protected, &mut stats)?;
        let plan = self.plan(cat);
        let mut left = plan.len() as u32;
        for w in &plan {
            if pause() {
                stats.end = SyncEnd::Paused;
                break;
            }
            if !pool.is_online() {
                stats.end = SyncEnd::Offline;
                break;
            }
            let have = fs::metadata(self.partial_path(&w.name)).map_or(0, |m| m.len()).min(w.size);
            if !self.make_room(w.size - have, &protected, &mut stats)? {
                stats.skipped += 1;
                stats.skipped_bytes += w.size;
                continue;
            }
            match self.copy(w, nas_root, pool, pause)? {
                Copy::Done => {
                    stats.copied += 1;
                    stats.copied_bytes += w.size;
                    left -= 1;
                }
                Copy::Paused => {
                    stats.end = SyncEnd::Paused;
                    break;
                }
                Copy::Offline => {
                    stats.end = SyncEnd::Offline;
                    break;
                }
                Copy::Failed(why) => {
                    eprintln!("mirror: {}: {why}", w.name);
                    stats.failed += 1;
                }
            }
        }
        stats.pending = left;
        self.tidy(cat, &protected);
        self.flush()?;
        Ok(stats)
    }

    /// Deletes local copies whose size isn't the catalog's (damaged, or cut short while the app
    /// wasn't running), so they're copied again.
    fn drop_mismatched(&self, cat: &Catalog) {
        let bad: Vec<(String, u64, u64)> = {
            let st = self.state();
            cat.files.values().filter_map(|f| st.files.get(&f.file).filter(|&&s| s != f.size).map(|&s| (f.file.clone(), s, f.size))).collect()
        };
        for (name, have, want) in bad {
            eprintln!("mirror: {name} is {have} bytes here but {want} in the catalog; copying it again");
            let p = self.path(&name);
            if fs::remove_file(&p).is_ok() || !p.exists() {
                self.state().files.remove(&name);
            }
        }
    }

    /// Frees space until `need` bytes fit within the budget, evicting unprotected files least
    /// recently used first. Whether they fit.
    fn make_room(&self, need: u64, protected: &HashSet<String>, stats: &mut SyncStats) -> Result<bool> {
        let mut free = (self.free_space)(&self.root).with_context(|| format!("free space at {}", self.root.display()))?;
        let target = self.reserve.saturating_add(need);
        if free >= target {
            return Ok(true);
        }
        let mut victims: Vec<(u64, String, u64)> = {
            let st = self.state();
            st.files
                .iter()
                .filter(|(n, _)| !protected.contains(*n))
                .map(|(n, &size)| {
                    let used = parse_content_name(n).and_then(|c| st.uses.get(c.logical).copied()).unwrap_or(0);
                    (used, n.clone(), size)
                })
                .collect()
        };
        victims.sort();
        for (_, name, size) in victims {
            if free >= target {
                break;
            }
            let p = self.path(&name);
            match fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    eprintln!("mirror: can't evict {}: {e}", p.display());
                    continue;
                }
            }
            self.remove_empty_dirs(&p);
            self.state().files.remove(&name);
            free += size;
            stats.evicted += 1;
            stats.evicted_bytes += size;
        }
        Ok(free >= target)
    }

    /// Removes the now-empty folders above a deleted file, up to `mirror/`.
    fn remove_empty_dirs(&self, file: &Path) {
        let top = self.root.join("mirror");
        let mut d = file.parent();
        while let Some(dir) = d {
            if dir == top || !dir.starts_with(&top) || fs::remove_dir(dir).is_err() {
                break;
            }
            d = dir.parent();
        }
    }

    fn copy(&self, w: &Want, nas_root: &Path, pool: &IoPool, pause: &dyn Fn() -> bool) -> Result<Copy> {
        let Some(c) = parse_content_name(&w.name) else { return Ok(Copy::Failed("not a content name".into())) };
        let (file, len) = match pool.open_len(&nas_root.join(&w.name)) {
            Ok(v) => v,
            Err(e) if e.is_unreachable() => return Ok(Copy::Offline),
            Err(e) => return Ok(Copy::Failed(format!("open on the NAS: {e}"))),
        };
        if len != w.size {
            return Ok(Copy::Failed(format!("{len} bytes on the NAS, but the catalog says {}", w.size)));
        }
        let part = self.partial_path(&w.name);
        if let Some(dir) = part.parent() {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let mut out = OpenOptions::new().create(true).append(true).open(&part).with_context(|| format!("open {}", part.display()))?;
        let mut have = out.metadata()?.len();
        let mut hasher = blake3::Hasher::new();
        if have > w.size {
            out.set_len(0)?;
            have = 0;
        } else if have > 0 {
            // Resuming: the hash covers the bytes already here.
            let mut f = File::open(&part)?;
            let mut buf = vec![0u8; CHUNK as usize];
            let mut left = have;
            while left > 0 {
                let n = f.read(&mut buf[..left.min(CHUNK) as usize])?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                left -= n as u64;
            }
        }
        let timeout = pool.config().op_timeout.max(CHUNK_TIMEOUT);
        while have < w.size {
            if pause() {
                out.sync_all()?;
                return Ok(Copy::Paused);
            }
            let n = (w.size - have).min(CHUNK) as usize;
            let buf = match pool.read_at_timeout(&file, have, n, timeout) {
                Ok(b) => b,
                Err(IoError::Offline | IoError::Timeout) => {
                    out.sync_all()?;
                    return Ok(Copy::Offline);
                }
                Err(e) => return Ok(Copy::Failed(format!("read on the NAS: {e}"))),
            };
            hasher.update(&buf);
            out.write_all(&buf).with_context(|| format!("write {}", part.display()))?;
            have += n as u64;
        }
        out.sync_all()?;
        drop(out);
        let got = hex16(&hasher.finalize());
        if got != c.hash16 {
            let _ = fs::remove_file(&part);
            return Ok(Copy::Failed(format!("the copy hashes to {got}; the NAS file is damaged")));
        }
        let dest = self.path(&w.name);
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        fs::rename(&part, &dest).with_context(|| format!("move {} into the mirror", part.display()))?;
        self.remove_empty_dirs(&part);
        self.state().files.insert(w.name.clone(), w.size);
        Ok(Copy::Done)
    }

    /// After a sync: drops partial copies and pack indexes no recent catalog needs (and temporary
    /// files an interrupted write left in `idx/`), and use records of logical names the catalog no
    /// longer has.
    fn tidy(&self, cat: &Catalog, protected: &HashSet<String>) {
        let partial = self.root.join("mirror").join(PARTIAL);
        let mut partials = HashMap::new();
        if scan(&partial, "", &mut partials).is_ok() {
            for name in partials.keys().filter(|n| !protected.contains(*n)) {
                let p = partial.join(name);
                if fs::remove_file(&p).is_ok() {
                    self.remove_empty_dirs(&p);
                }
            }
        }
        let hashes: HashSet<&str> = protected.iter().filter_map(|n| parse_content_name(n)).map(|c| c.hash16).collect();
        if let Ok(rd) = fs::read_dir(self.root.join("idx")) {
            for e in rd.flatten() {
                let name = e.file_name();
                let Some(name) = name.to_str() else { continue };
                let stale = match name.strip_suffix(".idx") {
                    Some(h) => !hashes.contains(h),
                    // A temporary file an hour old belongs to no write in progress.
                    None => name.ends_with(".tmp") && e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|a| a > STALE_TMP)),
                };
                if stale {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
        let mut st = self.state();
        let before = st.uses.len();
        st.uses.retain(|logical, _| cat.files.contains_key(logical));
        if st.uses.len() != before {
            st.dirty = true;
        }
    }

    #[cfg(test)]
    fn with_free_space(mut self, f: impl Fn(&Path) -> io::Result<u64> + Send + Sync + 'static) -> Self {
        self.free_space = Box::new(f);
        self
    }
}

impl Drop for Mirror {
    fn drop(&mut self) {
        if let Err(e) = self.flush() {
            eprintln!("mirror: {e:#}");
        }
    }
}

/// Copy order: 0 small worldwide files, 1 root and lo packs and the basemap (drawn on every view),
/// 2 hi data and road values, 3 base packs, 4 hi packs; `LAST_GROUP` for the rest.
fn groups(cat: &Catalog) -> HashMap<&str, u8> {
    fn set<'a>(g: &mut HashMap<&'a str, u8>, logical: &'a str, k: u8) {
        let e = g.entry(logical).or_insert(k);
        *e = (*e).min(k);
    }
    let mut g = HashMap::new();
    for v in cat.global.values() {
        set(&mut g, v, 0);
    }
    for l in cat.layers.values() {
        if let Some(r) = &l.root {
            set(&mut g, r, 1);
        }
        for v in l.lo.values() {
            set(&mut g, v, 1);
        }
        for v in l.hi.values() {
            set(&mut g, v, 4);
        }
    }
    for v in &cat.basemap {
        set(&mut g, v, 1);
    }
    for v in cat.hidata.values().chain(cat.roads.values()) {
        set(&mut g, v, 2);
    }
    for v in cat.base.values() {
        set(&mut g, v, 3);
    }
    g
}

/// Adds every content-named file under `dir` (skipping dot entries) to `out`, by its name
/// relative to the top.
fn scan(dir: &Path, prefix: &str, out: &mut HashMap<String, u64>) -> io::Result<()> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for e in rd {
        let e = e?;
        let Ok(name) = e.file_name().into_string() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let ft = e.file_type()?;
        if ft.is_dir() {
            scan(&e.path(), &rel, out)?;
        } else if ft.is_file() && parse_content_name(&rel).is_some() {
            out.insert(rel, e.metadata()?.len());
        }
    }
    Ok(())
}

/// Free bytes on the (local) filesystem holding `path`, for an unprivileged user.
fn disk_free(path: &Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `statfs` is plain old data; the path is NUL-terminated. This is the local disk
        // (statfs is avoided only for NAS paths, where it can hang).
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(st.f_bavail.saturating_mul(u64::from(st.f_bsize)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        // SAFETY: as above, with statvfs.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{FileRef, Layer};
    use crate::naming::{write_atomic, Source};
    use crate::pack::PackWriter;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::sync::Arc;

    /// Bytes of every file under `dir`, partial copies included: what the fake disk holds.
    fn used(dir: &Path) -> u64 {
        fs::read_dir(dir).map_or(0, |rd| {
            rd.flatten()
                .map(|e| {
                    let ft = e.file_type().unwrap();
                    if ft.is_dir() {
                        used(&e.path())
                    } else {
                        e.metadata().unwrap().len()
                    }
                })
                .sum()
        })
    }

    /// A mirror on a fake disk of `capacity` bytes holding only the mirror's files.
    fn mirror(root: &Path, capacity: u64, reserve: u64) -> Mirror {
        let m = root.join("mirror");
        Mirror::open(root.to_owned(), reserve).unwrap().with_free_space(move |_| Ok(capacity.saturating_sub(used(&m))))
    }

    fn bytes(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    struct Nas {
        dir: tempfile::TempDir,
        pool: Arc<IoPool>,
    }

    fn nas() -> Nas {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::new(2, Duration::from_secs(5), dir.path().to_owned());
        Nas { dir, pool }
    }

    impl Nas {
        fn root(&self) -> &Path {
            self.dir.path()
        }

        /// Puts a file on the "NAS" and lists it in `cat`.
        fn put(&self, cat: &mut Catalog, logical: &str, ext: &str, body: &[u8]) -> String {
            let name = write_atomic(self.root(), logical, ext, Source::Bytes(body)).unwrap();
            cat.files.insert(logical.into(), FileRef { file: name.clone(), size: body.len() as u64, fmt: 1, extra: Default::default() });
            name
        }
    }

    /// A catalog of one of each kind of file, `seed` changing their contents.
    fn catalog(nas: &Nas, n: u64, seed: u8) -> Catalog {
        let mut c = Catalog::new(n);
        let mut lay = Layer { encoding: "rt7".into(), minzoom: 4, maxzoom: 14, ..Default::default() };
        nas.put(&mut c, "layers/basemap/basemap", "pmtiles", &bytes(seed, 300_000));
        nas.put(&mut c, "layers/roads/hi/6-32-21", "pack", &bytes(seed + 1, 50_000));
        nas.put(&mut c, "base/6-32-21", "sect", &bytes(seed + 2, 9 << 20)); // three chunks
        nas.put(&mut c, "global/roads/6-32-21", "sect", &bytes(seed + 3, 20_000));
        nas.put(&mut c, "hidata/6-32-21", "sect", &bytes(seed + 4, 30_000));
        nas.put(&mut c, "layers/roads/lo/3-4-2", "pack", &bytes(seed + 5, 40_000));
        nas.put(&mut c, "layers/roads/root", "pack", &bytes(seed + 6, 10_000));
        nas.put(&mut c, "global/pois", "json", &bytes(seed + 7, 1_000));
        nas.put(&mut c, "global/marks/summary", "json", &bytes(seed + 8, 2_000));
        lay.root = Some("layers/roads/root".into());
        lay.lo.insert("3/4/2".into(), "layers/roads/lo/3-4-2".into());
        lay.hi.insert("6/32/21".into(), "layers/roads/hi/6-32-21".into());
        c.layers.insert("roads".into(), lay);
        c.basemap = vec!["layers/basemap/basemap".into()];
        c.base.insert("6/32/21".into(), "base/6-32-21".into());
        c.roads.insert("6/32/21".into(), "global/roads/6-32-21".into());
        c.hidata.insert("6/32/21".into(), "hidata/6-32-21".into());
        c.global.insert("pois.json".into(), "global/pois".into());
        c.global.insert("marks/summary".into(), "global/marks/summary".into());
        c.validate().unwrap();
        c
    }

    fn logicals(m: &Mirror, cat: &Catalog) -> Vec<String> {
        let by_name: HashMap<&str, &str> = cat.files.iter().map(|(l, f)| (f.file.as_str(), l.as_str())).collect();
        m.plan(cat).into_iter().map(|w| by_name[w.name.as_str()].to_string()).collect()
    }

    #[test]
    fn copies_in_order_and_verifies() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let cat = catalog(&nas, 1, 0);
        assert_eq!(
            logicals(&m, &cat),
            [
                "global/marks/summary",
                "global/pois",
                "layers/basemap/basemap",
                "layers/roads/lo/3-4-2",
                "layers/roads/root",
                "global/roads/6-32-21",
                "hidata/6-32-21",
                "base/6-32-21",
                "layers/roads/hi/6-32-21",
            ]
        );
        // Most recently used first within a group.
        m.touch(cat.content("layers/roads/root").unwrap());
        assert_eq!(logicals(&m, &cat)[2..5], ["layers/roads/root", "layers/basemap/basemap", "layers/roads/lo/3-4-2"]);

        let s = m.sync(&cat, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.failed, s.skipped, s.pending, s.end), (9, 0, 0, 0, SyncEnd::Done));
        for f in cat.files.values() {
            let p = m.local(&f.file).unwrap();
            assert_eq!(fs::read(&p).unwrap(), fs::read(nas.root().join(&f.file)).unwrap());
        }
        assert!(!home.path().join("mirror").join(PARTIAL).exists());
        assert_eq!(m.usage(), (9, cat.files.values().map(|f| f.size).sum()));

        // Nothing left to do.
        let s = m.sync(&cat, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.pending), (0, 0));

        // A reopened mirror finds its files, and its use times.
        let used_before = m.state().uses.clone();
        drop(m);
        let m = mirror(home.path(), 1 << 40, 0);
        assert!(cat.files.values().all(|f| m.local(&f.file).is_some()));
        assert_eq!(m.state().uses, used_before);

        // Deleted behind its back: not local any more.
        let pois = cat.content("global/pois").unwrap();
        fs::remove_file(m.path(pois)).unwrap();
        assert!(m.local(pois).is_none());
        assert!(m.local("../../etc/passwd").is_none());

        // Cut short while the app wasn't running: found at open, copied again.
        drop(m);
        let hi = cat.content("layers/roads/hi/6-32-21").unwrap();
        let p = home.path().join("mirror").join(hi);
        let body = fs::read(&p).unwrap();
        fs::write(&p, &body[..100]).unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let s = m.sync(&cat, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.failed, s.pending), (2, 0, 0), "the cut file and the deleted one");
        assert_eq!(fs::read(m.local(hi).unwrap()).unwrap(), body);
    }

    #[test]
    fn damaged_nas_files_are_not_kept() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let cat = catalog(&nas, 1, 0);
        // Same size, different bytes: the hash check catches it.
        let hi = cat.content("layers/roads/hi/6-32-21").unwrap().to_string();
        let mut body = fs::read(nas.root().join(&hi)).unwrap();
        body[100] ^= 1;
        fs::write(nas.root().join(&hi), &body).unwrap();
        // Wrong size: caught before copying.
        let lo = cat.content("layers/roads/lo/3-4-2").unwrap().to_string();
        fs::write(nas.root().join(&lo), b"short").unwrap();
        // Missing.
        let pois = cat.content("global/pois").unwrap().to_string();
        fs::remove_file(nas.root().join(&pois)).unwrap();

        let s = m.sync(&cat, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.failed, s.pending, s.end), (6, 3, 3, SyncEnd::Done));
        for bad in [&hi, &lo, &pois] {
            assert!(m.local(bad).is_none());
            assert!(!m.partial_path(bad).exists());
        }
        assert!(nas.pool.is_online());
    }

    #[test]
    fn pause_and_resume() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let cat = catalog(&nas, 1, 0);
        let base = cat.content("base/6-32-21").unwrap().to_string();
        let part = m.partial_path(&base);
        // Stop once the big file has its first chunk.
        let pause = || fs::metadata(&part).is_ok_and(|md| md.len() >= CHUNK);
        let s = m.sync(&cat, nas.root(), &nas.pool, &pause).unwrap();
        assert_eq!(s.end, SyncEnd::Paused);
        assert_eq!(fs::metadata(&part).unwrap().len(), CHUNK);
        assert!(m.local(&base).is_none());
        assert_eq!(s.copied, 7, "everything before the base pack");

        // Resumed: the rest is appended, the whole hash checks out.
        let s = m.sync(&cat, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.end, s.pending), (2, SyncEnd::Done, 0));
        assert_eq!(fs::read(m.local(&base).unwrap()).unwrap(), fs::read(nas.root().join(&base)).unwrap());
        assert!(!part.exists());

        // A partial copy whose bytes went bad is caught at the end and dropped.
        let cat2 = catalog(&nas, 2, 50);
        let base2 = cat2.content("base/6-32-21").unwrap().to_string();
        let part2 = m.partial_path(&base2);
        let pause = || fs::metadata(&part2).is_ok_and(|md| md.len() >= CHUNK);
        m.sync(&cat2, nas.root(), &nas.pool, &pause).unwrap();
        let f = OpenOptions::new().write(true).open(&part2).unwrap();
        std::os::unix::fs::FileExt::write_all_at(&f, b"XXXX", 10).unwrap();
        let s = m.sync(&cat2, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(s.failed, 1);
        assert!(!part2.exists() && m.local(&base2).is_none());
        let s = m.sync(&cat2, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.failed), (1, 0));
    }

    #[test]
    fn offline_stops_the_sync() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let cat = catalog(&nas, 1, 0);
        nas.pool.mark_offline("test");
        let s = m.sync(&cat, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.end, s.pending), (0, SyncEnd::Offline, 9));
    }

    #[test]
    fn budget_and_eviction() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let cat1 = catalog(&nas, 1, 0);
        let total1: u64 = cat1.files.values().map(|f| f.size).sum();
        // Room for one catalog's files and a bit, with a reserve.
        let reserve = 100_000;
        let m = mirror(home.path(), total1 + 400_000 + reserve, reserve);
        let s = m.sync(&cat1, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped), (9, 0));
        // Use some of them, in this order (the root pack last: most recent).
        for l in ["base/6-32-21", "layers/roads/hi/6-32-21", "layers/roads/root"] {
            m.touch(cat1.content(l).unwrap());
        }

        // A new catalog where everything changed. When the base pack needs room, the old files go
        // least recently used first: the never-used ones, then the old base pack, which is enough;
        // the two most recently used old files stay.
        let cat2 = catalog(&nas, 2, 100);
        let s = m.sync(&cat2, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped, s.evicted, s.end), (9, 0, 7, SyncEnd::Done));
        let kept: Vec<&str> = cat1.files.iter().filter(|(_, f)| m.local(&f.file).is_some()).map(|(l, _)| l.as_str()).collect();
        assert_eq!(kept, ["layers/roads/hi/6-32-21", "layers/roads/root"]);

        // The last two catalogs are protected: with catalog 2 saved, catalog 3 evicts only what's
        // left of catalog 1, and what still doesn't fit (the base pack, the basemap) waits.
        m.save_catalog(&cat1).unwrap();
        m.save_catalog(&cat2).unwrap();
        let cat3 = catalog(&nas, 3, 200);
        let s = m.sync(&cat3, nas.root(), &nas.pool, &|| false).unwrap();
        assert!(cat2.files.values().all(|f| m.local(&f.file).is_some()), "catalog 2's files stay");
        let waiting: Vec<&str> = cat3.files.iter().filter(|(_, f)| m.local(&f.file).is_none()).map(|(l, _)| l.as_str()).collect();
        eprintln!("cat3: {s:?}, waiting {waiting:?}");
        assert_eq!((s.copied, s.skipped, s.evicted, s.pending), (6, 3, 2, 3));
        // The basemap, drawn on every view, is copied early; the base pack is what waits.
        assert!(m.local(cat3.content("layers/basemap/basemap").unwrap()).is_some());
        assert!(m.local(cat3.content("base/6-32-21").unwrap()).is_none());
        // Within the budget throughout.
        assert!(used(&home.path().join("mirror")) <= total1 + 400_000);
    }

    #[test]
    fn reserve_is_honoured_even_with_nothing_to_copy() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let cat1 = catalog(&nas, 1, 0);
        let m = mirror(home.path(), 1 << 40, 0);
        m.sync(&cat1, nas.root(), &nas.pool, &|| false).unwrap();
        drop(m);
        // The disk filled up behind our back: the old files go to restore the reserve.
        let free = Arc::new(AtomicUsize::new(1_000_000));
        let f2 = free.clone();
        let m = Mirror::open(home.path().to_owned(), 5_000_000).unwrap().with_free_space(move |_| Ok(f2.load(SeqCst) as u64));
        let cat2 = Catalog { n: 2, ..cat1.clone() };
        let s = m.sync(&cat2, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(s.evicted, 0, "files of the current catalog are never evicted");
        let mut cat3 = Catalog::new(3);
        nas.put(&mut cat3, "global/pois", "json", b"{}");
        let s = m.sync(&cat3, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(s.evicted, 9, "nothing protects the old files");
        assert_eq!(s.skipped, 1, "and the reserve still isn't met (the fake disk doesn't change)");
    }

    #[test]
    fn index_cache() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let p = nas.root().join("x.pack");
        let mut w = PackWriter::create(&p, json!({"layer": "roads"}), false).unwrap();
        w.add(3, 1, 1, b"tile", 4).unwrap();
        w.finish().unwrap();
        let body = fs::read(&p).unwrap();
        let mut cat = Catalog::new(1);
        let name = nas.put(&mut cat, "layers/roads/lo/3-1-1", "pack", &body);

        let loads = AtomicUsize::new(0);
        let load = || {
            loads.fetch_add(1, SeqCst);
            PackIndex::read_from(&body)
        };
        let a = m.index(&name, load).unwrap();
        let b = m.index(&name, || panic!("must come from the cache")).unwrap();
        assert_eq!(a, b);
        assert_eq!(loads.load(SeqCst), 1);
        let idx = home.path().join("idx").join(format!("{}.idx", parse_content_name(&name).unwrap().hash16));
        // A damaged cache file is replaced.
        fs::write(&idx, b"junk").unwrap();
        assert_eq!(m.index(&name, || PackIndex::read_from(&body)).unwrap(), a);
        assert!(PackIndex::from_bytes(&fs::read(&idx).unwrap()).is_ok());
        assert!(m.index("not-a-content-name", || PackIndex::read_from(&body)).is_err());

        // Pack indexes no recent catalog uses are dropped after a sync.
        m.sync(&cat, nas.root(), &nas.pool, &|| false).unwrap();
        assert!(idx.exists());
        m.sync(&Catalog::new(2), nas.root(), &nas.pool, &|| false).unwrap();
        assert!(!idx.exists());
    }

    #[test]
    fn saved_catalogs() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        assert!(m.saved_catalog().unwrap().is_none());
        for n in 1..=5 {
            m.save_catalog(&catalog(&nas, n, n as u8)).unwrap();
        }
        m.save_catalog(&catalog(&nas, 5, 5)).unwrap();
        assert_eq!(m.saved_catalog().unwrap().unwrap().n, 5);
        assert_eq!(catalog::list(&home.path().join("catalog")).unwrap(), [5, 4, 3]);
    }
}
