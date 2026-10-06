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
//! it's back, in this order: those the current catalog doesn't list (an older catalog's); then the
//! current catalog's, the basemap last (it's drawn at every zoom); never the essentials
//! (`essentials`), nor the files the caller keeps (a Mac's kept areas'). Within each, the files
//! never used go first, the first copied first, then the used ones, least recently used first (a
//! file copied after its last use, as a new catalog's are, is a used one all the same). Of the
//! files in that order, the shortest run from the front that covers the deficit goes, less the
//! biggest of them it can spare, so a round doesn't go far past the deficit; the free space is
//! measured again as each goes. Nothing is copied while the disk is under the reserve, nor does
//! anything go while the caller says to wait (`sync_with`: on the build Mac, while its own agent
//! runs a job, whose pack and lo jobs read this mirror's base packs), unless the disk is below half
//! the reserve.
//!
//! **Copy order**, one file at a time in large sequential reads through the I/O pool: the
//! essentials, then the kept files, then the rest; within each, small worldwide files, root and lo
//! packs and the basemap, hi data and road values (and the essentials' per-tile records), base
//! packs, hi packs, and everything else; the most recently used first within each group. The budget
//! is the free space less the reserve. An essential or kept file that doesn't fit takes the room of
//! the files that may go, in room first's order, but only when that makes enough; while one waits
//! for room, no other file is copied. Any other file takes only the room of files the current
//! catalog doesn't list, and, when it's been used, of the current catalog's never used (the
//! basemap aside), so the mirror comes round to what's used without ever trading a used file for
//! another. Those other files are copied only while a margin (a twentieth of the reserve) stays
//! free above the reserve, and a file let go for room only once it's been used again: so the
//! disk's comings and goings around the reserve don't have the same files copied and let go over
//! and over.

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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Bytes per NAS read while copying.
const CHUNK: u64 = 4 << 20;
/// Shortest timeout for one chunk: long enough that a slow link isn't taken for a dead one.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
/// How often the use times are written out during a sync, at most (`flush_due`).
const SAVE_EVERY: Duration = Duration::from_secs(60);
/// Catalogs kept in `catalog/`.
const KEEP_CATALOGS: usize = 3;
/// Age past which a temporary file in `idx/` is left over from an interrupted write.
const STALE_TMP: Duration = Duration::from_secs(3600);
const PARTIAL: &str = ".partial";
const USES: &str = ".uses";
/// The group of files no other group claims (the basemap's parts; anything newer).
const LAST_GROUP: u8 = 5;
/// Files neither essential nor kept are copied only while this share of the reserve stays free
/// above it (module doc).
const SLACK: u64 = 20;

type FreeSpace = dyn Fn(&Path) -> io::Result<u64> + Send + Sync;
/// Told the content names of the complete files a sync evicted, as each goes: the server drops its
/// maps of them, so the disk gets their room back.
type OnEvict = dyn Fn(&[String]) + Send + Sync;

/// What a `sync` (or `keep_reserve`) did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Files copied and verified.
    pub copied: u32,
    pub copied_bytes: u64,
    /// Files left out because they don't fit the budget (or a kept file waited for room).
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
    /// `pause` asked it to stop (nothing went, the disk above half the reserve); a copy in
    /// progress resumes next time.
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
    /// The bytes of the mirror's other files: what may go to make room for them (none when the
    /// mirror can't let files go: `Mirror::evicts_here`).
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

/// What may go to make room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Take {
    /// Only files the current catalog doesn't list (for a file not kept and never used).
    Old,
    /// Those, and the current catalog's files never used, the basemap aside (for a file not kept
    /// that's been used: the mirror comes round to what's used).
    Unused,
    /// Any but the essentials and the kept files (room first, and for an essential or kept file).
    All,
}

/// A file the catalog references that isn't local yet.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Want {
    name: String,
    size: u64,
    tier: Tier,
    group: u8,
    /// Its last use (0: none).
    used: u64,
}

/// A local file that may go to make room.
#[derive(Clone, Debug)]
struct Victim {
    /// 0: the current catalog doesn't list it; 1: it does; 2: it's the current basemap.
    class: u8,
    /// Whether the map has used it (its logical name): within a class, those never used go first.
    used: bool,
    /// Its last use; for one never used, when it was copied.
    at: u64,
    size: u64,
    name: String,
    /// A copy in progress.
    partial: bool,
}

/// What a sync knows of the catalog's files: their copy groups, which never go, the basemap.
struct Ctx<'a> {
    /// The current catalog's files: content name → copy group.
    current: HashMap<&'a str, u8>,
    /// The essentials' logical names.
    essential: HashSet<&'a str>,
    /// Never evicted: the essentials and the kept files (content names).
    never: HashSet<&'a str>,
    /// The basemap's archives (content names).
    basemap: HashSet<&'a str>,
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
        let basemap = cat.basemap.iter().filter_map(|l| cat.content(l)).collect();
        Ctx { current, essential, never, basemap }
    }
}

struct State {
    /// Complete local copies: content name → size.
    files: HashMap<String, u64>,
    /// When each complete copy was made (content name → milliseconds since 1970: the file's
    /// modification time when the mirror was opened, the stamp of its copy since). Of the files
    /// never used, the first copied go first (room first's order).
    copied: HashMap<String, u64>,
    /// Logical name → last use (milliseconds since 1970, strictly increasing per touch).
    uses: HashMap<String, u64>,
    last_stamp: u64,
    dirty: bool,
    /// When the use times were last written out.
    saved: Instant,
    /// The copy under way.
    copying: Option<Copying>,
    /// What the last sync (or reserve check) did, and when.
    last: Option<(SyncStats, SystemTime)>,
    /// The catalog's files let go to make room (content name → `last_stamp` then): not copied
    /// again, unless kept, until they're used again.
    let_go: HashMap<String, u64>,
}

impl State {
    /// A new stamp: now, in milliseconds since 1970, and after every stamp before.
    fn stamp(&mut self) -> u64 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.last_stamp = now.max(self.last_stamp + 1);
        self.last_stamp
    }
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

/// A sync's files that may go for its wants, by what `Take` allows: worked out once, then again
/// after files have come or gone.
#[derive(Default)]
struct Pool {
    /// Bumped as files come or go.
    epoch: u64,
    lists: [Option<(u64, Vec<Victim>)>; 3],
}

impl Mirror {
    /// Opens (creating as needed) the mirror under `root`, the app's folder. `reserve_bytes` of
    /// the disk stay free (plan §4: 50 GB; 150 GB on the build Mac, so builds have room).
    pub fn open(root: PathBuf, reserve_bytes: u64) -> Result<Mirror> {
        for d in ["mirror", "idx", "catalog"] {
            let p = root.join(d);
            fs::create_dir_all(&p).with_context(|| format!("create {}", p.display()))?;
        }
        let mut found = HashMap::new();
        scan_meta(&root.join("mirror"), "", &mut found).context("list the mirror")?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let files: HashMap<String, u64> = found.iter().map(|(n, &(size, _))| (n.clone(), size)).collect();
        let copied: HashMap<String, u64> = found.into_iter().map(|(n, (_, at))| (n, at.min(now))).collect();
        let uses_path = root.join("mirror").join(USES);
        let uses: HashMap<String, u64> = match fs::read(&uses_path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                eprintln!("mirror: ignoring unreadable {}: {e}", uses_path.display());
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        };
        let last_stamp = uses.values().chain(copied.values()).copied().max().unwrap_or(0);
        Ok(Mirror {
            root,
            reserve: reserve_bytes,
            state: Mutex::new(State { files, copied, uses, last_stamp, dirty: false, saved: Instant::now(), copying: None, last: None, let_go: HashMap::new() }),
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

    /// What this Mac can hold of the map: the free space and the mirror's files, less the
    /// reserve.
    pub fn hold(&self) -> Result<u64> {
        let free = self.free()?;
        let mut partials = HashMap::new();
        let _ = scan(&self.root.join("mirror").join(PARTIAL), "", &mut partials);
        let here: u64 = self.state().files.values().sum::<u64>() + partials.values().sum::<u64>();
        Ok((free + here).saturating_sub(self.reserve))
    }

    /// Whether files deleted from `mirror/` give their room back on the disk the free space is
    /// measured on: `mirror/` a folder of its own, not a link, on the app folder's disk. When it
    /// isn't, nothing is let go (all of it might, with nothing to show for it).
    pub fn evicts_here(&self) -> bool {
        let m = self.root.join("mirror");
        let (Ok(mm), Ok(rm)) = (fs::symlink_metadata(&m), fs::metadata(&self.root)) else { return false };
        if mm.file_type().is_symlink() || !mm.is_dir() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            mm.dev() == rm.dev()
        }
        #[cfg(not(unix))]
        {
            let _ = rm;
            true
        }
    }

    /// Has `f` told the content names of complete files as a sync evicts them (the server drops
    /// what it mapped of them).
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
        let mut st = self.state();
        st.files.remove(content_name);
        st.copied.remove(content_name);
        None
    }

    /// Whether the mirror holds a complete copy of `content_name`, as far as it knows (no look at
    /// the disk: for tallies; `local` is for reading it).
    pub fn has(&self, content_name: &str) -> bool {
        self.state().files.contains_key(content_name)
    }

    /// The bytes of these files (content name, size) the mirror holds complete copies of.
    pub fn bytes_here<'a>(&self, files: impl IntoIterator<Item = (&'a str, u64)>) -> u64 {
        let st = self.state();
        files.into_iter().filter(|(n, _)| st.files.contains_key(*n)).map(|(_, s)| s).sum()
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
    /// written out by `flush`, which a sync calls at most once a minute while it goes, and at its
    /// end (the server's mirror thread every minute, and before it exits).
    pub fn touch(&self, content_name: &str) {
        let Some(c) = parse_content_name(content_name) else { return };
        let mut st = self.state();
        let stamp = st.stamp();
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
            st.saved = Instant::now();
            st.uses.iter().map(|(k, v)| (k.clone(), *v)).collect()
        };
        let p = self.root.join("mirror").join(USES);
        replace_file(&p, &serde_json::to_vec(&snapshot)?, None).with_context(|| format!("write {}", p.display()))
    }

    /// `flush`, if a minute has passed since the use times were last written (in a sync's loops).
    fn flush_due(&self) {
        if self.state().saved.elapsed() < SAVE_EVERY {
            return;
        }
        if let Err(e) = self.flush() {
            eprintln!("mirror: {e:#}");
        }
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
    /// by group within each, the most recently used first. Of the rest, those let go to make room
    /// and not used since are left out: how many is the second value.
    fn plan(&self, cat: &Catalog, ctx: &Ctx) -> (Vec<Want>, u32) {
        let st = self.state();
        let mut v: Vec<Want> = Vec::new();
        let mut let_go = 0;
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
            let used = st.uses.get(logical).copied().unwrap_or(0);
            if tier == Tier::Rest && st.let_go.get(&f.file).is_some_and(|&at| used <= at) {
                let_go += 1;
                continue;
            }
            v.push(Want { name: f.file.clone(), size: f.size, tier, group, used });
        }
        v.sort_by(|a, b| a.tier.cmp(&b.tier).then(a.group.cmp(&b.group)).then(b.used.cmp(&a.used)).then_with(|| a.name.cmp(&b.name)));
        (v, let_go)
    }

    /// The local files `take` lets go to make room, sparing the essentials, the kept files and
    /// `fresh` (copied this sync), in the order they go: files the current catalog doesn't list,
    /// then the current catalog's, then its basemap; within each, those never used, the first
    /// copied first, then the used ones, the least recently used first (whenever they were copied:
    /// a new catalog copies again the files used before it). Copies in progress go as their files
    /// would (not copied yet: the first of those never used).
    fn victims(&self, ctx: &Ctx, take: Take, fresh: &HashSet<String>) -> Vec<Victim> {
        let mut partials = HashMap::new();
        if let Err(e) = scan(&self.root.join("mirror").join(PARTIAL), "", &mut partials) {
            eprintln!("mirror: copies in progress: {e}");
        }
        let st = self.state();
        let complete = st.files.iter().map(|(n, &s)| (n, s, false));
        let started = partials.iter().map(|(n, &s)| (n, s, true));
        let mut v: Vec<Victim> = complete
            .chain(started)
            .filter(|(n, _, _)| !ctx.never.contains(n.as_str()) && !fresh.contains(*n))
            .filter_map(|(n, size, partial)| {
                let used = parse_content_name(n).and_then(|c| st.uses.get(c.logical).copied());
                let class = match ctx.current.get(n.as_str()) {
                    None => 0,
                    Some(_) if ctx.basemap.contains(n.as_str()) => 2,
                    Some(_) => 1,
                };
                let may = match take {
                    Take::All => true,
                    Take::Old => class == 0,
                    Take::Unused => class == 0 || (class == 1 && used.is_none()),
                };
                let at = used.unwrap_or_else(|| st.copied.get(n).copied().unwrap_or(0));
                may.then(|| Victim { class, used: used.is_some(), at, size, name: n.clone(), partial })
            })
            .collect();
        v.sort_by(|a, b| (a.class, a.used, a.at, &a.name).cmp(&(b.class, b.used, b.at, &b.name)));
        v
    }

    /// Frees space until `need` more bytes fit within the budget, from `victims` (in the order they
    /// go: `victims`), sparing `sparing`'s copy in progress; with `whole`, nothing goes unless
    /// that's enough. Each round of it takes the shortest run of victims from the front that covers
    /// what's short, less the biggest of them it can spare (`choose`), and measures the free space
    /// again as each goes (the server lets go of its maps of it at once). The bytes still short (0:
    /// they fit), and whether any file went.
    fn free_for(&self, need: u64, victims: &[Victim], sparing: Option<&str>, whole: bool, evicts: bool, stats: &mut SyncStats) -> Result<(u64, bool)> {
        let target = self.reserve.saturating_add(need);
        let mut free = self.free()?;
        if free >= target {
            return Ok((0, false));
        }
        if !evicts {
            return Ok((target - free, false));
        }
        let mut left: Vec<&Victim> = victims.iter().filter(|v| !(v.partial && Some(v.name.as_str()) == sparing)).collect();
        if whole && free.saturating_add(left.iter().map(|v| v.size).sum()) < target {
            return Ok((target - free, false));
        }
        let mut went = false;
        while free < target && !left.is_empty() {
            let chosen = choose(&left, target - free);
            let mut done = HashSet::new();
            for i in chosen {
                done.insert(i);
                went |= self.evict(left[i], stats);
                free = self.free()?;
                if free >= target {
                    break;
                }
            }
            // (Short still: maps not let go yet, or the disk filled meanwhile.)
            left = left.into_iter().enumerate().filter(|(i, _)| !done.contains(i)).map(|(_, v)| v).collect();
        }
        Ok((target.saturating_sub(free), went))
    }

    /// Deletes a victim; a complete file of the catalog is noted as let go, and the server told.
    /// Whether it went.
    fn evict(&self, v: &Victim, stats: &mut SyncStats) -> bool {
        let p = if v.partial { self.partial_path(&v.name) } else { self.path(&v.name) };
        match fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!("mirror: can't evict {}: {e}", p.display());
                return false;
            }
        }
        self.remove_empty_dirs(&p);
        if !v.partial {
            {
                let mut st = self.state();
                st.files.remove(&v.name);
                st.copied.remove(&v.name);
                if v.class != 0 {
                    let at = st.last_stamp;
                    st.let_go.insert(v.name.clone(), at);
                }
            }
            let hook = self.on_evict.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(f) = hook {
                f(std::slice::from_ref(&v.name));
            }
        }
        stats.evicted += 1;
        stats.evicted_bytes += v.size;
        true
    }

    /// Room first: files go until the disk is back over the reserve (or all that may go has
    /// gone). The bytes still short.
    fn room_first(&self, ctx: &Ctx, evicts: bool, stats: &mut SyncStats) -> Result<u64> {
        if self.free()? >= self.reserve {
            return Ok(0);
        }
        let victims = self.victims(ctx, Take::All, &HashSet::new());
        Ok(self.free_for(0, &victims, None, false, evicts, stats)?.0)
    }

    /// Copies what `cat` references and isn't here yet from `nas_root`, through `pool`, until done,
    /// `pause()` says stop, or the NAS goes offline: room first (the reserve before any copy), then
    /// the essentials, the files in `keep` (content names: a Mac's kept areas'), and the rest
    /// (module doc). `pause()` holds room first back too, as on the build Mac (`sync_with`). Run it
    /// on a background thread; it never holds the mirror's lock while it waits on the NAS.
    pub fn sync(&self, cat: &Catalog, keep: &HashSet<String>, nas_root: &Path, pool: &IoPool, pause: &dyn Fn() -> bool) -> Result<SyncStats> {
        self.sync_with(cat, keep, nas_root, pool, pause, pause)
    }

    /// `sync`, with room first held back by `room_waits()` rather than `pause()`: while it says wait
    /// at the start, nothing goes and nothing is copied, unless the disk is below half the reserve;
    /// once it says so later, the sync stops as for `pause()`. For the build Mac's server: its own
    /// agent's jobs (its pack and lo jobs read this mirror's base packs). Another Mac's room
    /// doesn't wait for the build Mac's jobs, only its copies do (`pause()`: the build's uploads
    /// have the NAS first).
    pub fn sync_with(
        &self,
        cat: &Catalog,
        keep: &HashSet<String>,
        nas_root: &Path,
        pool: &IoPool,
        pause: &dyn Fn() -> bool,
        room_waits: &dyn Fn() -> bool,
    ) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        let recent = self.recent(cat)?;
        self.drop_mismatched(cat);
        let ctx = Ctx::new(cat, keep);
        let evicts = self.evicts_here();
        if !evicts {
            eprintln!("mirror: {} is a link, or on another disk than {}: nothing is let go", self.root.join("mirror").display(), self.root.display());
        }
        if room_waits() && self.free()? >= self.reserve / 2 {
            stats.end = SyncEnd::Paused;
            let (plan, let_go) = self.plan(cat, &ctx);
            return self.finish(cat, &recent, stats, plan.len() as u32 + let_go);
        }
        // Room first: the user may have filled the disk since last time. Nothing is copied while
        // it's under the reserve.
        stats.short = self.room_first(&ctx, evicts, &mut stats)?;
        let (plan, let_go) = self.plan(cat, &ctx);
        let mut left = plan.len() as u32 + let_go;
        if stats.short > 0 {
            for w in &plan {
                skip(&mut stats, w, 0);
            }
            return self.finish(cat, &recent, stats, left);
        }
        // Copied this sync (none of them goes for a later want); a kept file waiting for room
        // (nothing else is copied then: what it'd take back).
        let mut fresh: HashSet<String> = HashSet::new();
        let mut waiting = false;
        let mut pool_of = Pool::default();
        for w in &plan {
            if pause() || room_waits() {
                stats.end = SyncEnd::Paused;
                break;
            }
            if !pool.is_online() {
                stats.end = SyncEnd::Offline;
                break;
            }
            self.flush_due();
            let have = fs::metadata(self.partial_path(&w.name)).map_or(0, |m| m.len()).min(w.size);
            let kept = w.tier != Tier::Rest;
            if waiting && !kept {
                skip(&mut stats, w, have);
                continue;
            }
            // (The rest keeps a margin free above the reserve.)
            let need = if kept { w.size - have } else { (w.size - have).saturating_add(self.reserve / SLACK) };
            let take = if kept {
                Take::All
            } else if w.used > 0 {
                Take::Unused
            } else {
                Take::Old
            };
            let k = take as usize;
            if pool_of.lists[k].as_ref().is_none_or(|(e, _)| *e != pool_of.epoch) {
                pool_of.lists[k] = Some((pool_of.epoch, self.victims(&ctx, take, &fresh)));
            }
            let victims = &pool_of.lists[k].as_ref().expect("just made").1;
            let (short, went) = self.free_for(need, victims, Some(&w.name), true, evicts, &mut stats)?;
            if went {
                pool_of.epoch += 1;
            }
            if short > 0 {
                waiting |= kept;
                skip(&mut stats, w, have);
                continue;
            }
            match self.copy(w, nas_root, pool, pause)? {
                Copy::Done => {
                    stats.copied += 1;
                    stats.copied_bytes += w.size;
                    left -= 1;
                    fresh.insert(w.name.clone());
                    pool_of.epoch += 1;
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
        let evicts = self.evicts_here();
        stats.short = self.room_first(&ctx, evicts, &mut stats)?;
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
        let evicts = self.evicts_here();
        let st = self.state();
        let missing: u64 = cat
            .files
            .values()
            .filter(|f| ctx.never.contains(f.file.as_str()) && !st.files.contains_key(&f.file))
            .map(|f| f.size - partials.get(&f.file).copied().unwrap_or(0).min(f.size))
            .sum();
        let evictable: u64 = if evicts { st.files.iter().chain(partials.iter()).filter(|(n, _)| !ctx.never.contains(n.as_str())).map(|(_, &s)| s).sum() } else { 0 };
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
                let mut st = self.state();
                st.files.remove(&name);
                st.copied.remove(&name);
            }
        }
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
            self.flush_due();
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
        let mut st = self.state();
        st.files.insert(w.name.clone(), w.size);
        let at = st.stamp();
        st.copied.insert(w.name.clone(), at);
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
        st.let_go.retain(|n, _| recent.contains(n));
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

/// Of `victims` (in the order they go), those to let go for `deficit` bytes: the shortest run
/// from the front that covers it, less the biggest of the run it can spare, so a round doesn't go
/// past the deficit when smaller files of the run cover it (and nothing later in the order goes
/// instead of something earlier). All of them when they don't cover it.
fn choose(victims: &[&Victim], deficit: u64) -> Vec<usize> {
    let (mut sum, mut k) = (0u64, 0);
    while k < victims.len() && sum < deficit {
        sum += victims[k].size;
        k += 1;
    }
    let mut chosen: Vec<usize> = (0..k).collect();
    if sum > deficit {
        let mut over = sum - deficit;
        let mut by_size = chosen.clone();
        by_size.sort_by_key(|&i| Reverse(victims[i].size));
        let mut spared = HashSet::new();
        for i in by_size {
            if victims[i].size <= over {
                over -= victims[i].size;
                spared.insert(i);
            }
        }
        chosen.retain(|i| !spared.contains(i));
    }
    chosen
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
    let mut found = HashMap::new();
    scan_meta(dir, prefix, &mut found)?;
    out.extend(found.into_iter().map(|(n, (size, _))| (n, size)));
    Ok(())
}

/// `scan`, with each file's size and modification time (milliseconds since 1970).
fn scan_meta(dir: &Path, prefix: &str, out: &mut HashMap<String, (u64, u64)>) -> io::Result<()> {
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
            scan_meta(&e.path(), &rel, out)?;
        } else if ft.is_file() && parse_content_name(&rel).is_some() {
            let md = e.metadata()?;
            let at = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
            out.insert(rel, (md.len(), at));
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
        m.plan(cat, &Ctx::new(cat, keep)).0.into_iter().map(|w| by_name[w.name.as_str()].to_string()).collect()
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
        // (the user filled it): the old base pack goes first; then the current catalog's files,
        // those never used first, the first copied first (the outlines, the road values, the hi
        // data, A's hi pack), until the run covers the deficit (190 kB); of
        // that run, the outlines and A's hi data are spared, the run less them covering it still.
        // Nothing is copied: there's no room for B's new base pack, and nothing else may go for it.
        let cat2 = two_areas(&nas, 2, 0, 99);
        let old_b = cat1.content("base/6-33-21").unwrap().to_string();
        cap.store(used(&home.path().join("mirror")) + reserve - 150_000, SeqCst);
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.evicted, s.evicted_bytes, s.short, s.copied), (5, 150_000, 0, 0));
        let gone = [old_b.as_str(), cat2.content("global/roads/6-32-21").unwrap(), cat2.content("global/roads/6-33-21").unwrap(), cat2.content("hidata/6-33-21").unwrap(), cat2.content("layers/roads/hi/6-32-21").unwrap()];
        assert_eq!(*evicted.lock().unwrap(), gone, "told as they went, in that order");
        assert!(gone.iter().all(|c| !m.has(c)));
        // Those used stay, and the essentials.
        for l in ["base/6-32-21", "layers/roads/hi/6-33-21", "layers/basemap/world", "global/railfreq", "layers/roads/root", "markdata/6-32-21"] {
            assert!(m.has(cat2.content(l).unwrap()), "{l}");
        }
        assert_eq!((s.skipped, s.pending), (1, 5), "B's new base pack waits for room; what was let go, to be used again");

        // Fuller still, past what may go (the disk full, and a reserve of 1 MB): everything but the
        // essentials goes (270 kB: the outlines, A's hi data and base pack, B's hi pack, the
        // basemap), and the disk stays short of the reserve, which the sync says. Nothing is copied.
        cap.store(used(&home.path().join("mirror")), SeqCst);
        drop(m);
        let m = mirror_on(home.path(), &cap, 1_000_000);
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(here(&m, &cat2), ["global/railfreq", "layers/roads/lo/3-4-2", "layers/roads/root", "markdata/6-32-21", "ovdata/3-4-2"]);
        assert_eq!((s.evicted, s.evicted_bytes, s.short), (5, 270_000, 730_000));
        assert_eq!((s.copied, s.skipped, s.skipped_kept, s.pending), (0, 5, 0, 10));
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
        // The disk just above the reserve and its margin (5 kB) for files not kept.
        cap.store(used(&home.path().join("mirror")) + reserve + 10_000, SeqCst);
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
        // Kept, it takes the room of the least recently used of the rest: none used, the first
        // copied (the outlines and both road values: 40 kB).
        let keep: HashSet<String> = [cat3.content("layers/roads/hi/6-34-21").unwrap().to_string()].into();
        let s = m.sync(&cat3, &keep, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.evicted, s.evicted_bytes), (1, 3, 40_000));
        assert!(!m.has(cat3.content("sources/osm/2026-09-28/outlines").unwrap()));
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
        // Of the files not kept, none used, the first copied go until they cover the deficit (the
        // outlines, B's road values and hi data, then its base pack); the run less what it can
        // spare is B's base pack alone (80 kB).
        assert_eq!((s.evicted, s.evicted_bytes, s.short, s.end), (1, 80_000, 0, SyncEnd::Offline));
        assert!(!m.has(cat.content("base/6-33-21").unwrap()) && m.has(cat.content("layers/roads/hi/6-33-21").unwrap()));
        assert!(keep_a.iter().all(|c| m.has(c)), "a kept area's files never go");
        assert_eq!(m.last().unwrap().0, s);
    }

    #[test]
    fn a_file_let_go_for_room_comes_back_once_used_again() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 100_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat = two_areas(&nas, 1, 0, 1);
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        // 50 kB short: the first copied go (none used): the outlines, both road values and A's hi
        // data, 50 kB.
        cap.store(used(&home.path().join("mirror")) + reserve - 50_000, SeqCst);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        let a_hidata = cat.content("hidata/6-32-21").unwrap();
        assert_eq!((s.evicted, s.evicted_bytes, s.copied, s.pending), (4, 50_000, 0, 4));
        assert!(!m.has(a_hidata));
        // Room again (the user freed some): they aren't copied back, not having been used since.
        cap.fetch_add(500_000, SeqCst);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.pending), (0, 4));
        // Used again: it comes back.
        m.touch(a_hidata);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.pending), (1, 3));
        assert!(m.has(a_hidata));
    }

    // ---- review scenarios (s5) -------------------------------------------------------------

    /// The M1 at the upgrade, in small: a mirror holding the basemap with no use time (the old code
    /// recorded a use only when a file was first mapped from the mirror, and the M1's basemap was
    /// copied at 02:29 with the map unused since: its `.uses` has no entry for it), a few
    /// never-used packs, and files used since. The disk is short of the reserve by a little more
    /// than the never-used packs. Room first takes the never-used packs, then the basemap (drawn
    /// on every view: 28.6 GB of catalog 14), though the used files would cover the rest of the
    /// deficit. On the real M1 (54.5 GB mirror, 40.5 GB free, a reserve of 50 GB): 35.0 GB go,
    /// the basemap among them, for a 9.5 GB deficit; and it never comes back while the room
    /// above the reserve is short of 28.6 GB plus the margin (only an older catalog's files may
    /// go for a file not kept).
    #[test]
    fn review_room_first_doesnt_let_the_basemap_go_for_a_deficit_smaller_files_cover() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        // (A reserve bigger than the deficit: the fake disk's free space can't go below 0.)
        let reserve = 300_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat = two_areas(&nas, 1, 0, 1);
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(m.usage().0, 15);
        // Used since (the outlines first, longest ago): 130 kB.
        for l in ["sources/osm/2026-09-28/outlines", "base/6-32-21", "hidata/6-32-21", "hidata/6-33-21"] {
            m.touch(cat.content(l).unwrap());
        }
        // Never used: both hi packs (100 kB), B's base pack (80 kB), the road values (10 kB),
        // the basemap (100 kB). 195 kB short: 190 kB of never-used packs, then 5 kB more.
        cap.store(used(&home.path().join("mirror")) + reserve - 195_000, SeqCst);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        let basemap = cat.content("layers/basemap/world").unwrap();
        assert!(m.has(basemap), "the basemap went for the last 5 kB of a 195 kB deficit: {s:?}");
        assert!(s.evicted_bytes < 195_000 + 50_000, "overshot the deficit by {} bytes", s.evicted_bytes - 195_000);
    }

    /// A kept file that can't fit even once everything that may go has gone (here the basemap,
    /// kept with any area: 28.6 GB, one file) has every other file evicted for it each round, and
    /// the rest copied into the room it then can't use, to be evicted again next round: the same
    /// bytes come from the NAS and go, round after round (each minute), until every file of the
    /// catalog has been let go once; and a file used again comes back to go again.
    #[test]
    fn review_a_kept_file_waiting_for_room_doesnt_churn_the_rest() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 100_000; // a margin of 5 kB
        let cat = two_areas(&nas, 1, 0, 1);
        // Room for the essentials (34 kB), A (145 kB) and 90 kB more.
        let m = mirror(home.path(), reserve + 34_000 + 145_000 + 90_000, reserve);
        // A kept without the basemap first: the essentials, A, and of the rest the outlines and
        // B's road values and hi data (the basemap doesn't fit with its margin).
        m.sync(&cat, &area(&cat, "6-32-21", false), nas.root(), &nas.pool, &|| false).unwrap();
        let a_and_basemap = area(&cat, "6-32-21", true);
        let mut before: HashSet<String> = cat.files.values().filter(|f| m.has(&f.file)).map(|f| f.file.clone()).collect();
        let mut churned = Vec::new();
        for round in 1..=3 {
            // The basemap kept too (as keep.rs keeps it with any area): it needs 100 kB, and at
            // most 90 kB can be made.
            let s = m.sync(&cat, &a_and_basemap, nas.root(), &nas.pool, &|| false).unwrap();
            assert!(!m.has(cat.content("layers/basemap/world").unwrap()));
            let now: HashSet<String> = cat.files.values().filter(|f| m.has(&f.file)).map(|f| f.file.clone()).collect();
            let copied: Vec<&String> = now.difference(&before).collect();
            let gone: Vec<&String> = before.difference(&now).collect();
            churned.push(format!("round {round}: copied {copied:?}, let go {gone:?} ({s:?})"));
            before = now;
        }
        // Every round after the first let go what the round before had copied.
        let copied_then_gone = churned.iter().filter(|r| r.contains("copied [\"")).count();
        assert_eq!(copied_then_gone, 0, "the rest was copied into room a waiting kept file took back:\n{}", churned.join("\n"));
    }

    #[test]
    fn a_used_file_takes_the_room_of_files_never_used_not_of_used_ones() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 100_000; // a margin of 5 kB
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat = two_areas(&nas, 1, 0, 1);
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        for l in ["base/6-32-21", "hidata/6-33-21", "layers/basemap/world"] {
            m.touch(cat.content(l).unwrap());
        }
        cap.store(used(&home.path().join("mirror")) + reserve + 5_000, SeqCst);
        // A new tile the map has read from the NAS (used): it takes the room of files never used,
        // the first copied first (the outlines, both road values, A's hi data: 50 kB), never of
        // the used ones nor the basemap.
        let mut cat2 = cat.clone();
        cat2.n = 2;
        let new = nas.put(&mut cat2, "layers/roads/hi/6-34-21", "pack", &bytes(7, 50_000));
        cat2.layers.get_mut("roads").unwrap().hi.insert("6/34/21".into(), "layers/roads/hi/6-34-21".into());
        m.touch(&new);
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.evicted, s.evicted_bytes), (1, 4, 50_000));
        assert!(m.has(&new));
        for l in ["base/6-32-21", "hidata/6-33-21", "layers/basemap/world"] {
            assert!(m.has(cat2.content(l).unwrap()), "{l} is used");
        }
        // One that needs more than all the files never used (B's base pack and both hi packs,
        // 180 kB): it waits, and nothing goes for it.
        let mut cat3 = cat2.clone();
        cat3.n = 3;
        let big = nas.put(&mut cat3, "layers/roads/hi/6-35-21", "pack", &bytes(8, 200_000));
        cat3.layers.get_mut("roads").unwrap().hi.insert("6/35/21".into(), "layers/roads/hi/6-35-21".into());
        m.touch(&big);
        let s = m.sync(&cat3, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.evicted, s.skipped), (0, 0, 1));
    }

    #[test]
    fn files_never_used_go_in_the_order_they_were_copied() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let cat = two_areas(&nas, 1, 0, 1);
        let m = mirror(home.path(), 1 << 40, 0);
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        drop(m);
        // No use records (the old server noted a use only when a file was first mapped): when each
        // was copied (its modification time) orders them. B's hi pack copied three hours ago, A's
        // an hour ago, the rest now.
        let hours = |h: u64| SystemTime::now() - Duration::from_secs(h * 3600);
        let set = |l: &str, t: SystemTime| OpenOptions::new().write(true).open(home.path().join("mirror").join(cat.content(l).unwrap())).unwrap().set_modified(t).unwrap();
        set("layers/roads/hi/6-33-21", hours(3));
        set("layers/roads/hi/6-32-21", hours(1));
        let reserve = 100_000;
        let total: u64 = cat.files.values().map(|f| f.size).sum();
        let m = mirror(home.path(), total + reserve - 50_000, reserve);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.evicted, s.evicted_bytes), (1, 50_000));
        assert!(!m.has(cat.content("layers/roads/hi/6-33-21").unwrap()));
        assert!(m.has(cat.content("layers/roads/hi/6-32-21").unwrap()));
    }

    #[test]
    fn room_first_waits_while_the_build_mac_works_unless_the_disk_is_below_half_the_reserve() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 200_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat = two_areas(&nas, 1, 0, 1);
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        // 50 kB short of the reserve, the build Mac at work, on the build Mac (its jobs read this
        // mirror): nothing goes.
        cap.store(used(&home.path().join("mirror")) + reserve - 50_000, SeqCst);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| true).unwrap();
        assert_eq!((s.evicted, s.end), (0, SyncEnd::Paused));
        // Below half the reserve: room first all the same (both base packs: of the run that covers
        // it, what's left once the rest is spared), and nothing copied.
        cap.store(used(&home.path().join("mirror")) + reserve - 150_000, SeqCst);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| true).unwrap();
        assert_eq!((s.evicted, s.evicted_bytes, s.short, s.copied), (2, 160_000, 0, 0));
    }

    #[test]
    fn on_another_mac_room_first_doesnt_wait_for_the_build_macs_job() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let reserve = 200_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat1 = two_areas(&nas, 1, 0, 1);
        m.sync(&cat1, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        // A new catalog (B's base pack changed), the disk 50 kB short of the reserve, the build Mac
        // at work. On the build Mac (its own jobs read this mirror): nothing goes.
        let cat2 = two_areas(&nas, 2, 0, 99);
        cap.store(used(&home.path().join("mirror")) + reserve - 50_000, SeqCst);
        let s = m.sync_with(&cat2, &none(), nas.root(), &nas.pool, &|| true, &|| true).unwrap();
        assert_eq!((s.evicted, s.copied, s.end), (0, 0, SyncEnd::Paused));
        // On another Mac: room first all the same (the old base pack, 80 kB), and the copies wait
        // for the build Mac.
        let s = m.sync_with(&cat2, &none(), nas.root(), &nas.pool, &|| true, &|| false).unwrap();
        assert_eq!((s.evicted, s.evicted_bytes, s.short), (1, 80_000, 0));
        assert_eq!((s.copied, s.end, s.pending), (0, SyncEnd::Paused, 1));
        assert!(!m.has(cat1.content("base/6-33-21").unwrap()));
    }

    #[test]
    fn nothing_goes_when_the_mirror_is_a_link() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join("mirror")).unwrap();
        let reserve = 100_000;
        let cap = Arc::new(AtomicU64::new(1 << 40));
        let m = mirror_on(home.path(), &cap, reserve);
        let cat = two_areas(&nas, 1, 0, 1);
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert!(!m.evicts_here());
        // Short of the reserve: nothing goes (deleting there might not free this disk at all).
        cap.store(used(&home.path().join("mirror")) + reserve - 50_000, SeqCst);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.evicted, s.short), (0, 50_000));
        assert_eq!(m.usage().0, 15);
        assert_eq!(m.room(&cat, &none()).unwrap().evictable, 0);
    }

    #[test]
    fn use_times_are_written_out_once_a_minute_while_a_sync_goes() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), 1 << 40, 0);
        let cat = two_areas(&nas, 1, 0, 1);
        let uses = home.path().join("mirror").join(USES);
        m.touch(cat.content("base/6-32-21").unwrap());
        m.flush_due();
        assert!(!uses.exists(), "not a minute yet");
        m.state().saved = Instant::now() - SAVE_EVERY;
        m.flush_due();
        assert!(fs::read_to_string(&uses).unwrap().contains("base/6-32-21"));
    }

    #[test]
    fn what_this_mac_can_hold() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let cat = two_areas(&nas, 1, 0, 1);
        let total: u64 = cat.files.values().map(|f| f.size).sum();
        let m = mirror(home.path(), total + 1_000_000, 300_000);
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        // The free space and the mirror's files, less the reserve.
        assert_eq!(m.hold().unwrap(), total + 1_000_000 - 300_000);
    }

    #[test]
    fn files_not_kept_leave_a_margin_above_the_reserve() {
        let nas = nas();
        // A reserve of 100 kB: a margin of 5 kB.
        let reserve = 100_000;
        let cat = two_areas(&nas, 1, 0, 1);
        let outlines = cat.content("sources/osm/2026-09-28/outlines").unwrap().to_string();
        // Room for the essentials (34 kB) and the outlines (30 kB) with the margin: they come, and
        // nothing after them (each would leave less than the margin).
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), reserve + 34_000 + 30_000 + 5_000, reserve);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(s.copied, 6);
        assert!(m.has(&outlines));
        // A byte less: the outlines wait; both road values (5 kB) and A's hi data (10 kB) come,
        // each with its margin.
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), reserve + 34_000 + 30_000 + 4_999, reserve);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(s.copied, 8);
        assert!(!m.has(&outlines) && m.has(cat.content("hidata/6-32-21").unwrap()));
        // Kept, they need no margin.
        let keep: HashSet<String> = [outlines.clone()].into();
        let home = tempfile::tempdir().unwrap();
        let m = mirror(home.path(), reserve + 34_000 + 30_000, reserve);
        let s = m.sync(&cat, &keep, nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!(s.copied, 6);
        assert!(m.has(&outlines));
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

        // A new catalog where everything changed. As each new file needs room, old files go, those
        // never used first (the first copied first), then the used ones, least recently used first,
        // no more of them than it needs: the old lo pack for the hi data, the old base pack for the
        // base pack (used, but none of the others together make room for it), the old basemap for
        // the hi pack.
        let cat2 = catalog(&nas, 2, 100);
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped, s.evicted, s.end), (9, 0, 3, SyncEnd::Done));
        let kept: Vec<&str> = cat1.files.iter().filter(|(_, f)| m.local(&f.file).is_some()).map(|(l, _)| l.as_str()).collect();
        assert_eq!(kept, ["global/marks/summary", "global/pois", "global/roads/6-32-21", "hidata/6-32-21", "layers/roads/hi/6-32-21", "layers/roads/root"]);

        // The previous catalog's files aren't spared any more: with catalog 2 saved, catalog 3
        // takes the room of catalog 1's leftovers and catalog 2's alike, and is copied whole. Those
        // never used go first, the first copied first (catalog 1's, then catalog 2's), the used
        // ones last: both catalogs' hi and root packs, and catalog 2's base pack (copied after
        // their last use, they're used ones all the same).
        m.save_catalog(&cat1).unwrap();
        m.save_catalog(&cat2).unwrap();
        let cat3 = catalog(&nas, 3, 200);
        let s = m.sync(&cat3, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.copied, s.skipped, s.evicted, s.pending), (9, 0, 7, 0));
        let old: Vec<&str> = cat1.files.iter().chain(&cat2.files).filter(|(_, f)| m.local(&f.file).is_some()).map(|(l, _)| l.as_str()).collect();
        // Gone: for the new basemap's room (71 kB, and the use times file on this fake disk),
        // catalog 1's places file and hi data and catalog 2's places file and lo pack (the run's
        // spare, catalog 1's road values and both summaries, 24 kB, can't take a places file too:
        // the use times file has the last kB); for the road values, catalog 1's; for the hi data,
        // catalog 2's basemap; for the base pack, catalog 2's, the one used file to go (nothing
        // else makes room for it). Of catalog 1's, its summary and its used hi and root packs stay.
        assert_eq!(old, ["global/marks/summary", "layers/roads/hi/6-32-21", "layers/roads/root", "global/marks/summary", "global/roads/6-32-21", "hidata/6-32-21", "layers/roads/hi/6-32-21", "layers/roads/root"]);
        // None of the current catalog's files went.
        assert!(cat3.files.values().all(|f| m.local(&f.file).is_some()));
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
        // catalog's files go too (the essentials aside), least recently used first (none used: the
        // first copied, the road values, the hi data, then the base pack, which covers it); the run
        // less what it can spare is the base pack alone. It isn't copied back into the room left
        // over: not until used again.
        let reserve = 5_000_000;
        let m = mirror(home.path(), total1 + 1_000_000, reserve);
        let cat2 = Catalog { n: 2, ..cat1.clone() };
        let s = m.sync(&cat2, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        assert_eq!((s.evicted, s.short, s.copied, s.skipped, s.pending), (1, 0, 0, 0, 1));
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
        assert_eq!((s.evicted, s.copied, s.skipped, s.skipped_kept), (8, 0, 1, 1), "the eight left");
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

    /// The M1 after catalog 14: every file here was copied after its last use (the new catalog's
    /// files were copied at 02:33–02:42, the owner's uses are from Oct 3), and the copy order puts
    /// the most recently used first, so they have the oldest copy times. Counting a used file as
    /// used when it was copied (if later) then has room first let go of the used files before the
    /// never-used ones copied after them: on the M1, 8.25 GB of the 9.54 GB it lets go were used
    /// (all of its hi data and road values go in the first round), while 5.17 GB never used stay.
    #[test]
    fn review_a_used_file_isnt_let_go_before_never_used_ones_copied_after_its_use() {
        let nas = nas();
        let home = tempfile::tempdir().unwrap();
        let cat = two_areas(&nas, 1, 0, 1);
        let m = mirror(home.path(), 1 << 40, 0);
        m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        drop(m);
        let ago = |h: u64| SystemTime::now() - Duration::from_secs(h * 3600);
        let ms = |t: SystemTime| t.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        // A's base pack and hi data used five hours ago, copied again three hours ago (a new
        // catalog), first (the most recently used first); both hi packs, never used, copied after
        // them, an hour ago; the rest just now.
        let uses: BTreeMap<&str, u64> = [("base/6-32-21", ms(ago(5))), ("hidata/6-32-21", ms(ago(5)))].into();
        fs::write(home.path().join("mirror").join(USES), serde_json::to_vec(&uses).unwrap()).unwrap();
        let set = |l: &str, t: SystemTime| OpenOptions::new().write(true).open(home.path().join("mirror").join(cat.content(l).unwrap())).unwrap().set_modified(t).unwrap();
        set("base/6-32-21", ago(3));
        set("hidata/6-32-21", ago(3));
        set("layers/roads/hi/6-32-21", ago(1));
        set("layers/roads/hi/6-33-21", ago(1));
        // 100 kB short: both never-used hi packs cover it.
        let reserve = 300_000;
        let m = mirror(home.path(), used(&home.path().join("mirror")) + reserve - 100_000, reserve);
        let s = m.sync(&cat, &none(), nas.root(), &nas.pool, &|| false).unwrap();
        let base_a = cat.content("base/6-32-21").unwrap();
        assert!(m.has(base_a), "A's base pack, used, went before never-used hi packs copied after its use: {s:?}, here {:?}", here(&m, &cat));
        assert!(!m.has(cat.content("layers/roads/hi/6-32-21").unwrap()) && !m.has(cat.content("layers/roads/hi/6-33-21").unwrap()));
    }
}
