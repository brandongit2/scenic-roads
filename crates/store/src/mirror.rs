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
//! **Room first.** The disk keeps a reserve of free space. When it's short of it, files go until
//! it's back, in this order: those the current catalog doesn't list (an older catalog's), least
//! recently used first; then the current catalog's, least recently used first (among those never
//! used, the ones copied last first); never the essentials (`essentials`), nor the files the caller
//! keeps (a Mac's kept areas'). Nothing is copied while the disk is under the reserve.
//!
//! **Copy order**, one file at a time in large sequential reads through the I/O pool: the
//! essentials, then the kept files, then the rest; within each, small worldwide files, root and lo
//! packs and the basemap, hi data and road values (and the essentials' per-tile records), base
//! packs, hi packs, and everything else; the most recently used first within each group. The budget
//! is the free space less the reserve. An essential or kept file that doesn't fit takes the room of
//! the files that may go, in the order above; any other file only that of files the current
//! catalog doesn't list, so the mirror never trades one of the catalog's files for another.

use crate::catalog::{self, Catalog};
use crate::iopool::{IoError, IoPool};
use crate::naming::{hex16, parse_content_name, replace_file};
use crate::pack::PackIndex;
use anyhow::{Context, Result};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Bytes per NAS read while copying.
const CHUNK: u64 = 4 << 20;
/// Shortest timeout for one chunk: long enough that a slow link isn't taken for a dead one.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
/// Catalogs kept in `catalog/`.
const KEEP_CATALOGS: usize = 3;
/// Age past which a temporary file in `idx/` is left over from an interrupted write.
const STALE_TMP: Duration = Duration::from_secs(3600);
const PARTIAL: &str = ".partial";
const USES: &str = ".uses";
/// The group of files no other group claims (the basemap's parts; anything newer).
const LAST_GROUP: u8 = 5;

type FreeSpace = dyn Fn(&Path) -> io::Result<u64> + Send + Sync;
/// Told the content names of the complete files a sync evicted, as soon as they're gone: the
/// server drops its maps of them, so the disk gets their room back.
type OnEvict = dyn Fn(&[String]) + Send + Sync;

/// What a `sync` (or `keep_reserve`) did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Files copied and verified.
    pub copied: u32,
    pub copied_bytes: u64,
    /// Files left out because they don't fit the budget.
    pub skipped: u32,
    pub skipped_bytes: u64,
    /// Of those, the files to keep (essentials and kept files), and the bytes they still lack.
    pub skipped_kept: u32,
    pub skipped_kept_bytes: u64,
    /// Files whose copy failed (logged; retried next time).
    pub failed: u32,
    /// Local files deleted to make room (copies in progress among them).
    pub evicted: u32,
    pub evicted_bytes: u64,
    /// Catalog files still not local when the sync ended.
    pub pending: u32,
    /// How far under the reserve the disk stayed once everything that may go had gone (0: the
    /// reserve held). Nothing was copied then.
    pub short: u64,
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

/// The copy under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copying {
    /// Its content name.
    pub name: String,
    pub size: u64,
    /// The bytes here so far.
    pub have: u64,
    /// One of the files to keep (an essential or a kept file).
    pub kept: bool,
}

/// The room for the files to keep (`Mirror::room`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Room {
    /// Free bytes on the disk now.
    pub free: u64,
    pub reserve: u64,
    /// The bytes the files to keep (the essentials and the kept files) still lack here, copies in
    /// progress counted.
    pub missing: u64,
    /// The bytes of the mirror's other files: what may go to make room for them.
    pub evictable: u64,
    /// How much more room they need than freeing all that would make: 0 when they fit.
    pub more: u64,
}

enum Copy {
    Done,
    Paused,
    Offline,
    Failed(String),
}

/// What a file is to this Mac: the order files are copied in, and what may take whose room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    /// An essential: every Mac keeps it.
    Essential,
    /// A kept file (the caller's `keep`: a Mac's kept areas').
    Kept,
    /// Anything else the catalog lists.
    Rest,
}

/// A file the catalog references that isn't local yet.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Want {
    name: String,
    size: u64,
    tier: Tier,
    group: u8,
    used: u64,
}

/// A local file that may go to make room.
struct Victim {
    /// 0: the current catalog doesn't list it; 1: it does.
    class: u8,
    used: u64,
    group: u8,
    size: u64,
    name: String,
    /// A copy in progress.
    partial: bool,
}

/// What a sync knows of the catalog's files: their copy groups, and which never go.
struct Ctx<'a> {
    /// The current catalog's files: content name → copy group.
    current: HashMap<&'a str, u8>,
    /// The essentials' logical names.
    essential: HashSet<&'a str>,
    /// Never evicted: the essentials and the kept files (content names).
    never: HashSet<&'a str>,
}

impl<'a> Ctx<'a> {
    fn new(cat: &'a Catalog, keep: &'a HashSet<String>) -> Ctx<'a> {
        let groups = groups(cat);
        let essential = essential_logicals(cat);
        let mut current = HashMap::with_capacity(cat.files.len());
        let mut never: HashSet<&str> = keep.iter().map(String::as_str).collect();
        for (logical, f) in &cat.files {
            current.insert(f.file.as_str(), groups.get(logical.as_str()).copied().unwrap_or(LAST_GROUP));
            if essential.contains(logical.as_str()) {
                never.insert(f.file.as_str());
            }
        }
        Ctx { current, essential, never }
    }
}

struct State {
    /// Complete local copies: content name → size.
    files: HashMap<String, u64>,
    /// Logical name → last use (milliseconds since 1970, strictly increasing per touch).
    uses: HashMap<String, u64>,
    last_stamp: u64,
    dirty: bool,
    /// The copy under way.
    copying: Option<Copying>,
    /// What the last sync (or reserve check) did, and when.
    last: Option<(SyncStats, SystemTime)>,
}

/// The local copy of the NAS's current files on one Mac.
pub struct Mirror {
    root: PathBuf,
    reserve: u64,
    state: Mutex<State>,
    free_space: Box<FreeSpace>,
    on_evict: Mutex<Option<Arc<OnEvict>>>,
}

/// Clears the copy under way when its copy returns, however it returns.
struct CopyingGuard<'a>(&'a Mirror);

impl Drop for CopyingGuard<'_> {
    fn drop(&mut self) {
        self.0.state().copying = None;
    }
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
            state: Mutex::new(State { files, uses, last_stamp, dirty: false, copying: None, last: None }),
            free_space: Box::new(disk_free),
            on_evict: Mutex::new(None),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The free space the disk keeps (bytes).
    pub fn reserve(&self) -> u64 {
        self.reserve
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Free bytes on the mirror's disk now.
    pub fn free(&self) -> Result<u64> {
        (self.free_space)(&self.root).with_context(|| format!("free space at {}", self.root.display()))
    }

    /// Has `f` told the content names of complete files as soon as a sync evicts them (the server
    /// drops what it mapped of them).
    pub fn on_evict(&self, f: impl Fn(&[String]) + Send + Sync + 'static) {
        *self.on_evict.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
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

    /// Whether the mirror holds a complete copy of `content_name`, as far as it knows (no look at
    /// the disk: for tallies; `local` is for reading it).
    pub fn has(&self, content_name: &str) -> bool {
        self.state().files.contains_key(content_name)
    }

    /// Complete local copies and their total size.
    pub fn usage(&self) -> (usize, u64) {
        let st = self.state();
        (st.files.len(), st.files.values().sum())
    }

    /// The copy under way, if any.
    pub fn copying(&self) -> Option<Copying> {
        self.state().copying.clone()
    }

    /// What the last sync (or reserve check) did, and when.
    pub fn last(&self) -> Option<(SyncStats, SystemTime)> {
        self.state().last
    }

    /// Records a use of `content_name` (its logical name, so a newer version of the same file
    /// inherits it): local or read from the NAS, it orders copies and evictions. Cheap, in memory:
    /// `flush` writes the use times out (a sync does at its end; the server's mirror thread every
    /// minute).
    pub fn touch(&self, content_name: &str) {
        let Some(c) = parse_content_name(content_name) else { return };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let mut st = self.state();
        let stamp = now.max(st.last_stamp + 1);
        st.last_stamp = stamp;
        match st.uses.get_mut(c.logical) {
            Some(u) => *u = stamp,
            None => {
                st.uses.insert(c.logical.to_string(), stamp);
            }
        }
        st.dirty = true;
    }

    /// Writes the use times out if they changed.
    pub fn flush(&self) -> Result<()> {
        let snapshot: BTreeMap<String, u64> = {
            let mut st = self.state();
            if !st.dirty {
                return Ok(());
            }
            st.dirty = false;
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

    /// Keeps a copy of `cat` in `catalog/` (for starting offline), and the last few before it.
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

    /// The content names of `cat` and of the newest saved catalog before it: their copies in
    /// progress and pack indexes are kept through a catalog switch (`tidy`). (Their files aren't:
    /// what the current catalog doesn't list goes first when room is short.)
    fn recent(&self, cat: &Catalog) -> Result<HashSet<String>> {
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

    /// The files of `cat` not here yet, in copy order: the essentials, the kept files, the rest;
    /// by group within each, the most recently used first.
    fn plan(&self, cat: &Catalog, ctx: &Ctx) -> Vec<Want> {
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
            let tier = if ctx.essential.contains(logical.as_str()) {
                Tier::Essential
            } else if ctx.never.contains(f.file.as_str()) {
                Tier::Kept
            } else {
                Tier::Rest
            };
            let group = ctx.current.get(f.file.as_str()).copied().unwrap_or(LAST_GROUP);
            v.push(Want { name: f.file.clone(), size: f.size, tier, group, used: st.uses.get(logical).copied().unwrap_or(0) });
        }
        v.sort_by(|a, b| a.tier.cmp(&b.tier).then(a.group.cmp(&b.group)).then(b.used.cmp(&a.used)).then_with(|| a.name.cmp(&b.name)));
        v
    }

    /// Copies what `cat` references and isn't here yet from `nas_root`, through `pool`, until done,
    /// `pause()` says stop, or the NAS goes offline: room first (the reserve before any copy), then
    /// the essentials, the files in `keep` (content names: a Mac's kept areas'), and the rest
    /// (module doc). Run it on a background thread; it never holds the mirror's lock while it waits
    /// on the NAS.
    pub fn sync(&self, cat: &Catalog, keep: &HashSet<String>, nas_root: &Path, pool: &IoPool, pause: &dyn Fn() -> bool) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        let recent = self.recent(cat)?;
        self.drop_mismatched(cat);
        let ctx = Ctx::new(cat, keep);
        // Room first: the user may have filled the disk since last time. Nothing is copied while
        // it's under the reserve.
        stats.short = self.make_room(0, &ctx, true, None, &mut stats)?;
        let plan = self.plan(cat, &ctx);
        let left = plan.len() as u32;
        if stats.short > 0 {
            for w in &plan {
                skip(&mut stats, w, 0);
            }
            return self.finish(cat, &recent, stats, left);
        }
        // Once a tier's make_room has failed, nothing more may go for it in this sync (copies
        // only add files the earlier tiers may not take): its files then fit what's free or not.
        let (mut keep_spent, mut rest_spent) = (false, false);
        let mut left = left;
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
            let need = w.size - have;
            let kept = w.tier != Tier::Rest;
            let spent = if kept { keep_spent } else { rest_spent };
            let fits = if spent {
                self.free()? >= self.reserve.saturating_add(need)
            } else if self.make_room(need, &ctx, kept, Some(&w.name), &mut stats)? == 0 {
                true
            } else {
                // Everything that may go for it has gone (for a kept file, all a later file may
                // take too).
                rest_spent = true;
                keep_spent |= kept;
                false
            };
            if !fits {
                skip(&mut stats, w, have);
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
        self.finish(cat, &recent, stats, left)
    }

    /// Room first without the NAS (away from home): files go until the reserve is back, as `sync`
    /// makes it; nothing is copied. What it did.
    pub fn keep_reserve(&self, cat: &Catalog, keep: &HashSet<String>) -> Result<SyncStats> {
        let ctx = Ctx::new(cat, keep);
        let mut stats = SyncStats { end: SyncEnd::Offline, ..Default::default() };
        stats.short = self.make_room(0, &ctx, true, None, &mut stats)?;
        self.state().last = Some((stats, SystemTime::now()));
        Ok(stats)
    }

    /// A sync's end: what's left, the tidying, the use times written out, the outcome noted.
    fn finish(&self, cat: &Catalog, recent: &HashSet<String>, mut stats: SyncStats, left: u32) -> Result<SyncStats> {
        stats.pending = left;
        self.tidy(cat, recent);
        self.flush()?;
        self.state().last = Some((stats, SystemTime::now()));
        Ok(stats)
    }

    /// The room for the files to keep, the essentials and `keep` (content names): what they still
    /// lack, what may go for them, and how much more room they need than that makes (`Room`).
    pub fn room(&self, cat: &Catalog, keep: &HashSet<String>) -> Result<Room> {
        let ctx = Ctx::new(cat, keep);
        let free = self.free()?;
        let mut partials = HashMap::new();
        if let Err(e) = scan(&self.root.join("mirror").join(PARTIAL), "", &mut partials) {
            eprintln!("mirror: copies in progress: {e}");
        }
        let st = self.state();
        let missing: u64 = cat
            .files
            .values()
            .filter(|f| ctx.never.contains(f.file.as_str()) && !st.files.contains_key(&f.file))
            .map(|f| f.size - partials.get(&f.file).copied().unwrap_or(0).min(f.size))
            .sum();
        let evictable: u64 = st.files.iter().chain(partials.iter()).filter(|(n, _)| !ctx.never.contains(n.as_str())).map(|(_, &s)| s).sum();
        let can = free as i128 + evictable as i128 - self.reserve as i128;
        let more = (missing as i128 - can).max(0) as u64;
        Ok(Room { free, reserve: self.reserve, missing, evictable, more })
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

    /// Frees space until `need` more bytes fit within the budget: files the current catalog doesn't
    /// list go first, least recently used first; then, with `take_current`, the current catalog's,
    /// least recently used first (among those never used, the last groups first, then the biggest);
    /// never the essentials or the kept files (`ctx.never`), nor `sparing`'s copy in progress. A
    /// copy in progress goes as its file would. The bytes still short (0: they fit).
    fn make_room(&self, need: u64, ctx: &Ctx, take_current: bool, sparing: Option<&str>, stats: &mut SyncStats) -> Result<u64> {
        let mut free = self.free()?;
        let target = self.reserve.saturating_add(need);
        if free >= target {
            return Ok(0);
        }
        let mut partials = HashMap::new();
        if let Err(e) = scan(&self.root.join("mirror").join(PARTIAL), "", &mut partials) {
            eprintln!("mirror: copies in progress: {e}");
        }
        let mut victims: Vec<Victim> = {
            let st = self.state();
            let complete = st.files.iter().map(|(n, &s)| (n, s, false));
            let started = partials.iter().map(|(n, &s)| (n, s, true));
            complete
                .chain(started)
                .filter(|(n, _, partial)| !ctx.never.contains(n.as_str()) && !(*partial && Some(n.as_str()) == sparing))
                .filter_map(|(n, size, partial)| {
                    let (class, group) = match ctx.current.get(n.as_str()) {
                        Some(&g) if take_current => (1, g),
                        Some(_) => return None,
                        None => (0, LAST_GROUP + 1),
                    };
                    let used = parse_content_name(n).and_then(|c| st.uses.get(c.logical).copied()).unwrap_or(0);
                    Some(Victim { class, used, group, size, name: n.clone(), partial })
                })
                .collect()
        };
        victims.sort_by(|a, b| (a.class, a.used, Reverse(a.group), Reverse(a.size), &a.name).cmp(&(b.class, b.used, Reverse(b.group), Reverse(b.size), &b.name)));
        let mut gone = Vec::new();
        for v in victims {
            if free >= target {
                break;
            }
            let p = if v.partial { self.partial_path(&v.name) } else { self.path(&v.name) };
            match fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    eprintln!("mirror: can't evict {}: {e}", p.display());
                    continue;
                }
            }
            self.remove_empty_dirs(&p);
            if !v.partial {
                self.state().files.remove(&v.name);
                gone.push(v.name);
            }
            free += v.size;
            stats.evicted += 1;
            stats.evicted_bytes += v.size;
        }
        if !gone.is_empty() {
            let hook = self.on_evict.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(f) = hook {
                f(&gone);
            }
        }
        Ok(target.saturating_sub(free))
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
        self.state().copying = Some(Copying { name: w.name.clone(), size: w.size, have, kept: w.tier != Tier::Rest });
        let _clear = CopyingGuard(self);
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
            if let Some(cur) = self.state().copying.as_mut() {
                cur.have = have;
            }
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

    /// After a sync: drops copies in progress and pack indexes no recent catalog needs (and
    /// temporary files an interrupted write left in `idx/`), and use records of logical names the
    /// catalog no longer has.
    fn tidy(&self, cat: &Catalog, recent: &HashSet<String>) {
        let partial = self.root.join("mirror").join(PARTIAL);
        let mut partials = HashMap::new();
        if scan(&partial, "", &mut partials).is_ok() {
            for name in partials.keys().filter(|n| !recent.contains(*n)) {
                let p = partial.join(name);
                if fs::remove_file(&p).is_ok() {
                    self.remove_empty_dirs(&p);
                }
            }
        }
        let hashes: HashSet<&str> = recent.iter().filter_map(|n| parse_content_name(n)).map(|c| c.hash16).collect();
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

/// A file left out of a sync for want of room (`have`: the bytes of its copy in progress).
fn skip(stats: &mut SyncStats, w: &Want, have: u64) {
    stats.skipped += 1;
    stats.skipped_bytes += w.size;
    if w.tier != Tier::Rest {
        stats.skipped_kept += 1;
        stats.skipped_kept_bytes += w.size - have;
    }
}

/// The logical names every Mac keeps, whatever its room (plan §4, "Mirror, per Mac"): what the map
/// needs to start and to draw anywhere zoomed out, and the small per-tile records any view's lists
/// read. The worldwide files the build makes (`global/…`: rail frequencies, the road → units index,
/// landmark totals, heritage summaries, today's converted layer files and details, roads' English
/// names), every layer's root and lo packs (zooms 0–8), and the landmark points and area details
/// (markdata, ovdata). Not the pass's area outlines (a worldwide file under `sources/`, read only to
/// make new regions), nor the basemap (big: kept with the kept areas).
fn essential_logicals(cat: &Catalog) -> HashSet<&str> {
    let mut e: HashSet<&str> = cat.global.values().map(String::as_str).filter(|l| l.starts_with("global/")).collect();
    for l in cat.layers.values() {
        e.extend(l.root.iter().map(String::as_str));
        e.extend(l.lo.values().map(String::as_str));
    }
    e.extend(cat.markdata.values().map(String::as_str));
    e.extend(cat.ovdata.values().map(String::as_str));
    e
}

/// The content names of `cat`'s essentials (`essential_logicals`), sorted.
pub fn essentials(cat: &Catalog) -> Vec<String> {
    let mut v: Vec<String> = essential_logicals(cat).into_iter().filter_map(|l| cat.content(l).map(str::to_string)).collect();
    v.sort();
    v
}

/// Copy order: 0 small worldwide files, 1 root and lo packs and the basemap (drawn on every view),
/// 2 hi data and road values (and the essentials' landmark points and area details), 3 base packs,
/// 4 hi packs; `LAST_GROUP` for the rest.
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
    for v in cat.hidata.values().chain(cat.roads.values()).chain(cat.markdata.values()).chain(cat.ovdata.values()) {
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
    crate::sys::disk_free(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{FileRef, Layer};
    use crate::naming::{write_atomic, Source};
    use crate::pack::PackWriter;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};
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

    /// A mirror on a fake disk whose capacity can change (the user filling or emptying it).
    fn mirror_on(root: &Path, capacity: &Arc<AtomicU64>, reserve: u64) -> Mirror {
        let (m, cap) = (root.join("mirror"), capacity.clone());
        Mirror::open(root.to_owned(), reserve).unwrap().with_free_space(move |_| Ok(cap.load(SeqCst).saturating_sub(used(&m))))
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

    /// Two areas' files, A (the z6 tile 6/32/21) and B (6/33/21), and the worldwide ones: 34 kB of
    /// essentials, a 100 kB basemap, 30 kB of outlines, 145 kB an area. `seed` changes their
    /// contents; `b_base` B's base pack's alone (a catalog where only it changed).
    fn two_areas(nas: &Nas, n: u64, seed: u8, b_base: u8) -> Catalog {
        let mut c = Catalog::new(n);
        let s = |k: u8| seed.wrapping_add(k);
        let mut lay = Layer { encoding: "rt7".into(), minzoom: 4, maxzoom: 14, ..Default::default() };
        nas.put(&mut c, "global/railfreq", "bin", &bytes(s(0), 1_000));
        nas.put(&mut c, "sources/osm/2026-09-28/outlines", "sect", &bytes(s(1), 30_000));
        nas.put(&mut c, "layers/roads/root", "pack", &bytes(s(2), 10_000));
        nas.put(&mut c, "layers/roads/lo/3-4-2", "pack", &bytes(s(3), 20_000));
        nas.put(&mut c, "markdata/6-32-21", "sect", &bytes(s(4), 2_000));
        nas.put(&mut c, "ovdata/3-4-2", "sect", &bytes(s(5), 1_000));
        nas.put(&mut c, "layers/basemap/world", "pmtiles", &bytes(s(6), 100_000));
        for (t, k) in [("6-32-21", 10u8), ("6-33-21", 20)] {
            let key = t.replace('-', "/");
            nas.put(&mut c, &format!("layers/roads/hi/{t}"), "pack", &bytes(s(k), 50_000));
            let b = if t == "6-33-21" { b_base } else { s(k + 1) };
            nas.put(&mut c, &format!("base/{t}"), "sect", &bytes(b, 80_000));
            nas.put(&mut c, &format!("global/roads/{t}"), "sect", &bytes(s(k + 2), 5_000));
            nas.put(&mut c, &format!("hidata/{t}"), "sect", &bytes(s(k + 3), 10_000));
            lay.hi.insert(key.clone(), format!("layers/roads/hi/{t}"));
            c.base.insert(key.clone(), format!("base/{t}"));
            c.roads.insert(key.clone(), format!("global/roads/{t}"));
            c.hidata.insert(key, format!("hidata/{t}"));
        }
        lay.root = Some("layers/roads/root".into());
        lay.lo.insert("3/4/2".into(), "layers/roads/lo/3-4-2".into());
        c.layers.insert("roads".into(), lay);
        c.basemap = vec!["layers/basemap/world".into()];
        c.markdata.insert("6/32/21".into(), "markdata/6-32-21".into());
        c.ovdata.insert("3/4/2".into(), "ovdata/3-4-2".into());
        c.global.insert("railfreq".into(), "global/railfreq".into());
        c.global.insert("outlines".into(), "sources/osm/2026-09-28/outlines".into());
        c.validate().unwrap();
        c
    }

    /// The content names of an area's files (its hi pack, base pack, road values and hi data) and,
    /// with `basemap`, the basemap's: what a Mac keeping it passes `sync`.
    fn area(cat: &Catalog, tile: &str, basemap: bool) -> HashSet<String> {
        let mut v: Vec<String> = ["layers/roads/hi", "base", "global/roads", "hidata"].iter().map(|d| cat.content(&format!("{d}/{tile}")).unwrap().to_string()).collect();
        if basemap {
            v.push(cat.content("layers/basemap/world").unwrap().to_string());
        }
        v.into_iter().collect()
    }

    /// Of `cat`'s files, the logical names of those here, sorted.
    fn here(m: &Mirror, cat: &Catalog) -> Vec<String> {
        cat.files.iter().filter(|(_, f)| m.has(&f.file)).map(|(l, _)| l.clone()).collect()
    }

    fn logicals(m: &Mirror, cat: &Catalog, keep: &HashSet<String>) -> Vec<String> {
        let by_name: HashMap<&str, &str> = cat.files.iter().map(|(l, f)| (f.file.as_str(), l.as_str())).collect();
        m.plan(cat, &Ctx::new(cat, keep)).into_iter().map(|w| by_name[w.name.as_str()].to_string()).collect()
    }

    fn none() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn copies_in_order_and_verifies() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let cat = catalog(&nas, 1, 0);
        assert_eq!(
            logicals(&m, &cat, &none()),
            [
                // The essentials: worldwide files, root and lo packs.
                "global/marks/summary",
                "global/pois",
                "layers/roads/lo/3-4-2",
                "layers/roads/root",
                // The rest.
                "layers/basemap/basemap",
                "global/roads/6-32-21",
                "hidata/6-32-21",
                "base/6-32-21",
                "layers/roads/hi/6-32-21",
            ]
        );
        // Most recently used first within a group.
        m.touch(cat.content("layers/roads/root").unwrap());
        assert_eq!(logicals(&m, &cat, &none())[2..4], ["layers/roads/root", "layers/roads/lo/3-4-2"]);

        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.failed, s.skipped, s.pending, s.end), (9, 0, 0, 0, SyncEnd::Done));
        for f in cat.files.values() {
            let p = m.local(&f.file).unwrap();
            assert_eq!(fs::read(&p).unwrap(), fs::read(nas.root().join(&f.file)).unwrap());
        }
        assert!(!home.path().join("mirror").join(PARTIAL).exists());
        assert_eq!(m.usage(), (9, cat.files.values().map(|f| f.size).sum()));
        assert_eq!(m.last().map(|(s, _)| s), Some(s));

        // Nothing left to do.
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
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
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.failed, s.pending), (2, 0, 0), "the cut file and the deleted one");
        assert_eq!(fs::read(m.local(hi).unwrap()).unwrap(), body);
    }

    #[test]
    fn essentials_are_the_worldwide_files_the_zoomed_out_packs_and_the_small_records() {
        let nas = nas();
        let cat = two_areas(&nas, 1, 0, 1);
        let names = essentials(&cat);
        let logical: Vec<&str> = names.iter().map(|c| parse_content_name(c).unwrap().logical).collect();
        let mut want = vec!["global/railfreq", "layers/roads/lo/3-4-2", "layers/roads/root", "markdata/6-32-21", "ovdata/3-4-2"];
        want.sort_by_key(|l| cat.content(l).unwrap().to_string());
        // Not the pass's outlines (a worldwide file only the Regions panel reads), the basemap,
        // nor anything of an area: its hi packs, base packs, road values and hi data.
        assert_eq!(logical, want);
    }

    #[test]
    fn kept_files_come_first_and_take_the_room_of_files_not_kept() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 100_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat = two_areas(&nas, 1, 0, 1);
        let keep_a = area(&cat, "6-32-21", true);
        assert_eq!(
            logicals(&m, &cat, &keep_a),
            [
                // The essentials: worldwide files (group 0), root and lo packs (1), the small
                // per-tile records (2).
                "global/railfreq",
                "layers/roads/lo/3-4-2",
                "layers/roads/root",
                "markdata/6-32-21",
                "ovdata/3-4-2",
                // A, kept: the basemap (1), road values and hi data (2), the base pack (3), the hi
                // pack (4).
                "layers/basemap/world",
                "global/roads/6-32-21",
                "hidata/6-32-21",
                "base/6-32-21",
                "layers/roads/hi/6-32-21",
                // The rest, in the same groups.
                "sources/osm/2026-09-28/outlines",
                "global/roads/6-33-21",
                "hidata/6-33-21",
                "base/6-33-21",
                "layers/roads/hi/6-33-21",
            ]
        );

        // Room for the essentials (34 kB), A and the basemap (245 kB), and 40 kB more: of the
        // rest, the outlines and B's road values fit, then B's hi data doesn't, nor its packs.
        cap.store(reserve + 34_000 + 245_000 + 40_000, SeqCst);
        let s = m.sync(&cat, &keep_a, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped, s.skipped_kept, s.evicted, s.pending), (12, 3, 0, 0, 3));
        assert!(keep_a.iter().all(|c| m.has(c)) && essentials(&cat).iter().all(|c| m.has(c)));
        assert_eq!(m.room(&cat, &keep_a).unwrap(), Room { free: reserve + 5_000, reserve, missing: 0, evictable: 35_000, more: 0 });

        // B kept too: its files take the room of those no area keeps (the outlines; B's road values
        // are its own now), then wait for more. The room says how much more they need.
        let keep_ab: HashSet<String> = keep_a.union(&area(&cat, "6-33-21", false)).cloned().collect();
        let s = m.sync(&cat, &keep_ab, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.evicted, s.evicted_bytes), (1, 1, 30_000), "the outlines went for B's hi data");
        assert_eq!((s.skipped, s.skipped_kept, s.skipped_kept_bytes), (2, 2, 130_000), "B's packs wait");
        assert!(!m.has(cat.content("sources/osm/2026-09-28/outlines").unwrap()));
        assert_eq!(m.room(&cat, &keep_ab).unwrap(), Room { free: reserve + 25_000, reserve, missing: 130_000, evictable: 0, more: 105_000 });
        // Room made on the disk: they come.
        cap.fetch_add(105_000, SeqCst);
        let s = m.sync(&cat, &keep_ab, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped_kept, s.evicted), (2, 0, 0));
        assert_eq!(m.room(&cat, &keep_ab).unwrap().more, 0);
        assert!(keep_ab.iter().all(|c| m.has(c)));
    }

    #[test]
    fn room_first_an_old_catalogs_files_go_first_then_the_current_ones_least_recently_used() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 200_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat1 = two_areas(&nas, 1, 0, 1);
        m.sync(&cat1, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(m.usage().0, 15);
        // Used, in this order: A's base pack, B's hi pack, the basemap.
        for l in ["base/6-32-21", "layers/roads/hi/6-33-21", "layers/basemap/world"] {
            m.touch(cat1.content(l).unwrap());
        }
        let evicted: Arc<Mutex<Vec<String>>> = Default::default();
        let e2 = evicted.clone();
        m.on_evict(move |names| e2.lock().unwrap().extend(names.iter().cloned()));

        // A new catalog where only B's base pack changed, and the disk 150 kB short of the reserve
        // (the user filled it): the old base pack goes first; then the current catalog's files
        // least recently used first, those never used of the last groups first (A's hi pack, then
        // the hi data, the biggest first), until the reserve is back. Nothing is copied: there's
        // no room for B's new base pack, and nothing else may go for it.
        let cat2 = two_areas(&nas, 2, 0, 99);
        let old_b = cat1.content("base/6-33-21").unwrap().to_string();
        cap.store(used(&home.path().join("mirror")) + reserve - 150_000, SeqCst);
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.evicted, s.evicted_bytes, s.short, s.copied), (4, 150_000, 0, 0));
        let gone = [old_b.as_str(), cat2.content("layers/roads/hi/6-32-21").unwrap(), cat2.content("hidata/6-32-21").unwrap(), cat2.content("hidata/6-33-21").unwrap()];
        assert_eq!(*evicted.lock().unwrap(), gone, "told as they went, in that order");
        assert!(gone.iter().all(|c| !m.has(c)));
        // Those used stay, and the essentials.
        for l in ["base/6-32-21", "layers/roads/hi/6-33-21", "layers/basemap/world", "global/railfreq", "layers/roads/root", "markdata/6-32-21"] {
            assert!(m.has(cat2.content(l).unwrap()), "{l}");
        }
        assert_eq!(s.skipped, 4, "B's new base pack, A's hi pack and both hi data wait for room");

        // Fuller still, past what may go (the disk full, and a reserve of 1 MB): everything but the
        // essentials goes (270 kB), and the disk stays short of the reserve, which the sync says.
        // Nothing is copied.
        cap.store(used(&home.path().join("mirror")), SeqCst);
        drop(m);
        let m = mirror_on(home.path(), &cap, 1_000_000);
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(here(&m, &cat2), ["global/railfreq", "layers/roads/lo/3-4-2", "layers/roads/root", "markdata/6-32-21", "ovdata/3-4-2"]);
        assert_eq!((s.evicted, s.evicted_bytes, s.short), (6, 270_000, 730_000));
        assert_eq!((s.copied, s.skipped, s.skipped_kept), (0, 10, 0));
        assert_eq!(m.last().unwrap().0.short, s.short);
    }

    #[test]
    fn a_file_not_kept_takes_only_the_room_of_files_no_longer_listed() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 100_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat1 = two_areas(&nas, 1, 0, 1);
        m.sync(&cat1, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        // The disk just at the reserve.
        cap.store(used(&home.path().join("mirror")) + reserve, SeqCst);
        // B's base pack changed: the old one makes room for the new.
        let cat2 = two_areas(&nas, 2, 0, 99);
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.evicted, s.skipped), (1, 1, 0));
        assert_eq!(here(&m, &cat2).len(), 15);
        // A new tile with nothing old left to take the room of: it waits, and none of the
        // catalog's files goes for it (the mirror never trades one of them for another).
        let mut cat3 = cat2.clone();
        cat3.n = 3;
        nas.put(&mut cat3, "layers/roads/hi/6-34-21", "pack", &bytes(7, 50_000));
        cat3.layers.get_mut("roads").unwrap().hi.insert("6/34/21".into(), "layers/roads/hi/6-34-21".into());
        let s = m.sync(&cat3, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.evicted, s.skipped, s.pending), (0, 0, 1, 1));
        // Kept, it takes the room of the least recently used of the rest.
        let keep: HashSet<String> = [cat3.content("layers/roads/hi/6-34-21").unwrap().to_string()].into();
        let s = m.sync(&cat3, &keep, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.evicted, s.evicted_bytes), (1, 1, 50_000), "a never-used hi pack went");
    }

    #[test]
    fn away_from_home_the_reserve_comes_first_too() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 100_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat = two_areas(&nas, 1, 0, 1);
        let keep_a = area(&cat, "6-32-21", true);
        m.sync(&cat, &keep_a, nas.root(), &nas.pool, &|| false).unwrap();
        nas.pool.mark_offline("test");
        cap.store(used(&home.path().join("mirror")) + reserve - 60_000, SeqCst);
        let s = m.keep_reserve(&cat, &keep_a).unwrap();
        // B's hi pack (50 kB), then its base pack (80 kB): never used, the last groups first.
        assert_eq!((s.evicted, s.evicted_bytes, s.short, s.end), (2, 130_000, 0, SyncEnd::Offline));
        assert!(!m.has(cat.content("layers/roads/hi/6-33-21").unwrap()) && !m.has(cat.content("base/6-33-21").unwrap()));
        assert!(keep_a.iter().all(|c| m.has(c)), "a kept area's files never go");
        assert_eq!(m.last().unwrap().0, s);
    }

    #[test]
    fn the_copy_under_way_is_told() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let cat = catalog(&nas, 1, 0);
        let base = cat.content("base/6-32-21").unwrap().to_string();
        let seen: Mutex<Vec<Copying>> = Default::default();
        let pause = || {
            if let Some(c) = m.copying().filter(|c| c.name == base) {
                seen.lock().unwrap().push(c);
            }
            false
        };
        m.sync(&cat, &none(), nas.root(), &nas.pool, &pause).unwrap();
        let seen = seen.into_inner().unwrap();
        let have: Vec<u64> = seen.iter().map(|c| c.have).collect();
        assert_eq!(have, [0, CHUNK, 2 * CHUNK], "before each chunk");
        assert!(seen.iter().all(|c| c.size == 9 << 20 && !c.kept));
        assert_eq!(m.copying(), None, "none once done");
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

        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
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
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &pause).unwrap();
        assert_eq!(s.end, SyncEnd::Paused);
        assert_eq!(fs::metadata(&part).unwrap().len(), CHUNK);
        assert!(m.local(&base).is_none());
        assert_eq!(s.copied, 7, "everything before the base pack");

        // Resumed: the rest is appended, the whole hash checks out.
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.end, s.pending), (2, SyncEnd::Done, 0));
        assert_eq!(fs::read(m.local(&base).unwrap()).unwrap(), fs::read(nas.root().join(&base)).unwrap());
        assert!(!part.exists());

        // A partial copy whose bytes went bad is caught at the end and dropped.
        let cat2 = catalog(&nas, 2, 50);
        let base2 = cat2.content("base/6-32-21").unwrap().to_string();
        let part2 = m.partial_path(&base2);
        let pause = || fs::metadata(&part2).is_ok_and(|md| md.len() >= CHUNK);
        m.sync(&cat2, &none(), nas.root(), &nas.pool, &pause).unwrap();
        let f = OpenOptions::new().write(true).open(&part2).unwrap();
        std::os::unix::fs::FileExt::write_all_at(&f, b"XXXX", 10).unwrap();
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(s.failed, 1);
        assert!(!part2.exists() && m.local(&base2).is_none());
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.failed), (1, 0));
    }

    #[test]
    fn offline_stops_the_sync() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let cat = catalog(&nas, 1, 0);
        nas.pool.mark_offline("test");
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
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
        let s = m.sync(&cat1, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped), (9, 0));
        // Use some of them, in this order (the root pack last: most recent).
        for l in ["base/6-32-21", "layers/roads/hi/6-32-21", "layers/roads/root"] {
            m.touch(cat1.content(l).unwrap());
        }

        // A new catalog where everything changed. When the base pack needs room, the old files go
        // least recently used first: the never-used ones (the biggest first), then the old base
        // pack, which is enough; the two most recently used old files stay.
        let cat2 = catalog(&nas, 2, 100);
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped, s.evicted, s.end), (9, 0, 7, SyncEnd::Done));
        let kept: Vec<&str> = cat1.files.iter().filter(|(_, f)| m.local(&f.file).is_some()).map(|(l, _)| l.as_str()).collect();
        assert_eq!(kept, ["layers/roads/hi/6-32-21", "layers/roads/root"]);

        // The previous catalog's files aren't spared any more: with catalog 2 saved, catalog 3
        // takes the room of catalog 1's leftovers and catalog 2's alike, least recently used
        // first, and is copied whole; what stays of the old ones is what was used last.
        m.save_catalog(&cat1).unwrap();
        m.save_catalog(&cat2).unwrap();
        let cat3 = catalog(&nas, 3, 200);
        let s = m.sync(&cat3, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped, s.evicted, s.pending), (9, 0, 7, 0));
        let old: Vec<&str> = cat1.files.iter().chain(&cat2.files).filter(|(_, f)| m.local(&f.file).is_some()).map(|(l, _)| l.as_str()).collect();
        assert_eq!(old, ["layers/roads/hi/6-32-21", "layers/roads/root", "layers/roads/hi/6-32-21", "layers/roads/root"]);
        // Within the budget throughout.
        assert!(used(&home.path().join("mirror")) <= total1 + 400_000);
    }

    #[test]
    fn the_reserve_comes_first_even_with_nothing_to_copy() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let cat1 = catalog(&nas, 1, 0);
        let total1: u64 = cat1.files.values().map(|f| f.size).sum();
        let m = mirror(home.path(), 1 << 40, 0);
        m.sync(&cat1, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        drop(m);
        // The disk filled up behind our back, to 4 MB short of a 5 MB reserve: the current
        // catalog's files go too (the essentials aside), least recently used first, the last
        // groups first among the never used: the hi pack, then the base pack, which is enough. The
        // hi pack fits again (its room is spare now); the base pack waits.
        let reserve = 5_000_000;
        let m = mirror(home.path(), total1 + 1_000_000, reserve);
        let cat2 = Catalog { n: 2, ..cat1.clone() };
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.evicted, s.short, s.copied, s.skipped), (2, 0, 1, 1));
        assert!(m.local(cat2.content("base/6-32-21").unwrap()).is_none());
        assert!(essentials(&cat2).iter().all(|c| m.local(c).is_some()));
        // A catalog that lists none of them: they all go, and the reserve still isn't met (the
        // disk is that full), so its one file, an essential, isn't copied.
        drop(m);
        let m = mirror(home.path(), 1_000_000, reserve);
        let mut cat3 = Catalog::new(3);
        nas.put(&mut cat3, "global/pois", "json", b"{}");
        cat3.global.insert("pois.json".into(), "global/pois".into());
        let s = m.sync(&cat3, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.evicted, s.copied, s.skipped, s.skipped_kept), (8, 0, 1, 1));
        assert_eq!(s.short, 4_000_000);
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
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert!(idx.exists());
        m.sync(&Catalog::new(2), &none(), nas.root(), &nas.pool, &|| false).unwrap();
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
