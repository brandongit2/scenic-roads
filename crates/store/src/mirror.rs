//! The per-Mac mirror (plan §4, "Mirror, per Mac"; docs/formats.md, "On each Mac"): the map's
//! files this Mac has downloaded, copied from the NAS under the same content names, so the map
//! reads them mmapped and works away from home where they reach. Nothing is copied unless the
//! owner downloaded it (the Regions panel: the World, zoomed out; a region; a view), and nothing
//! downloaded goes by itself.
//!
//! Under the app's folder (`~/Library/Application Support/scenic/`):
//!
//! ```text
//! mirror/<content name>             complete copies, each moved here only after its hash checked out
//! mirror/.partial/<content name>    copies in progress, resumed where they stopped
//! mirror/.basemap/<hash16>/<piece>.pmtiles        the basemap's pieces (crate::pieces) of the
//!                                                 archive whose content hash that is: lo, 6-x-y
//! mirror/.basemap/<hash16>/<piece>.pmtiles.part   a piece being made, resumed where it stopped
//! mirror/.basemap/<hash16>/sizes.json             each piece's size, worked out from the archive
//! idx/<hash16>.idx                  pack indexes (PackIndex::to_bytes), cached as they're read
//! catalog/<n>.json.zst              the last catalogs adopted
//! ```
//!
//! **What's wanted** (`Wanted`, from the caller: the server's downloads) is a list of files of the
//! current catalog and pieces of its basemap, in the order they're copied. A sync first lets go of
//! everything else here (an older catalog's files, replaced ones, what a removed download had), then
//! copies what's wanted and missing, one at a time, in large sequential reads through the I/O pool.
//! A file that doesn't fit (the disk's free space, less the reserve) waits, and nothing goes to make
//! room for it. While the caller says the build is running (`Control::slow`), copies keep to 20 MB/s
//! so the build's own traffic has the NAS first; while its own agent runs a job on this Mac
//! (`Control::hold`), nothing is deleted (its pack and lo jobs may read this mirror's base packs).

use crate::catalog::{self, Catalog};
use crate::iopool::{IoError, IoPool};
use crate::naming::{hex16, parse_content_name, replace_file};
use crate::pack::PackIndex;
use crate::pieces::{self, Piece, Wrote};
use crate::pmtiles::PmTiles;
use crate::range::RangeRead;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// Bytes per NAS read while copying.
const CHUNK: u64 = 4 << 20;
/// Shortest timeout for one read: long enough that a slow link isn't taken for a dead one.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
/// Catalogs kept in `catalog/`.
const KEEP_CATALOGS: usize = 3;
/// Age past which a temporary file in `idx/` is left over from an interrupted write.
const STALE_TMP: Duration = Duration::from_secs(3600);
const PARTIAL: &str = ".partial";
const BASEMAP: &str = ".basemap";
const PART: &str = ".part";
const SIZES: &str = "sizes.json";
/// The use times the mirror kept before downloads (no longer: deleted when found).
const OLD_USES: &str = ".uses";
/// Bytes a second the copies keep to while the build runs.
pub const SLOW_RATE: u64 = 20_000_000;

type FreeSpace = dyn Fn(&Path) -> io::Result<u64> + Send + Sync;
/// Told what a sync deleted (content names, and pieces as `.basemap/<hash16>/<piece>.pmtiles`), as
/// each goes: the server drops its maps of them, so the disk gets their room back.
type OnRemove = dyn Fn(&[String]) + Send + Sync;

/// Something to copy: a file of the catalog, or a piece of one of its basemap archives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    File { name: String, size: u64 },
    Piece { archive: String, piece: Piece },
}

/// What this Mac's downloads want here, in copy order (each once).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    pub items: Vec<Item>,
}

impl Wanted {
    /// Adds `item` unless it's there already.
    pub fn push(&mut self, item: Item, seen: &mut HashSet<Item>) {
        if seen.insert(item.clone()) {
            self.items.push(item);
        }
    }

    fn files(&self) -> HashSet<&str> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::File { name, .. } => Some(name.as_str()),
                Item::Piece { .. } => None,
            })
            .collect()
    }

    /// The wanted pieces, by archive hash and piece name.
    fn pieces(&self) -> HashSet<(String, String)> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Piece { archive, piece } => Some((hash_of(archive)?.to_string(), piece.name())),
                Item::File { .. } => None,
            })
            .collect()
    }
}

/// An archive's content hash, from its content name.
fn hash_of(archive: &str) -> Option<&str> {
    parse_content_name(archive).map(|c| c.hash16)
}

/// What the caller tells a sync as it goes.
pub struct Control<'a> {
    /// Stop now (the downloads changed, a new catalog): a copy under way resumes next time.
    pub stop: &'a dyn Fn() -> bool,
    /// The build is running: copies keep to `SLOW_RATE`.
    pub slow: &'a dyn Fn() -> bool,
    /// Delete nothing for now (this Mac's own agent runs a job).
    pub hold: &'a dyn Fn() -> bool,
}

impl Control<'_> {
    /// Nothing to stop, slow or hold for.
    pub const FREE: Control<'static> = Control { stop: &|| false, slow: &|| false, hold: &|| false };
}

/// What a `sync` (or `let_go`) did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Files and pieces copied and verified.
    pub copied: u32,
    pub copied_bytes: u64,
    /// Those waiting for room (they don't fit the free space less the reserve), and the bytes
    /// they still lack.
    pub waiting: u32,
    pub waiting_bytes: u64,
    /// Those whose copy failed (logged; retried next time).
    pub failed: u32,
    /// Local files and pieces deleted: wanted no more.
    pub removed: u32,
    pub removed_bytes: u64,
    /// Wanted, and still not here when the sync ended.
    pub pending: u32,
    pub end: SyncEnd,
}

/// How a `sync` ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncEnd {
    /// Went through everything wanted.
    #[default]
    Done,
    /// `Control::stop` asked it to stop; a copy in progress resumes next time.
    Paused,
    /// The NAS went (or was) offline.
    Offline,
}

/// The copy under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copying {
    /// Its content name, or for a piece `basemap <hash16> <piece>` (`copy_name`).
    pub name: String,
    pub size: u64,
    /// The bytes here so far.
    pub have: u64,
    /// Kept to `SLOW_RATE` (the build is running).
    pub slow: bool,
}

/// The room for what's wanted (`Mirror::room`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Room {
    /// Free bytes on the disk now.
    pub free: u64,
    pub reserve: u64,
    /// The bytes what's wanted still lacks here, copies in progress counted.
    pub missing: u64,
    /// How much more room that needs than the free space above the reserve: 0 when it fits.
    pub more: u64,
    /// Wanted pieces whose size isn't known yet (not counted).
    pub unknown: u32,
}

enum Copy {
    Done(u64),
    Paused,
    Offline,
    Failed(String),
}

struct State {
    /// Complete local copies: content name → size.
    files: HashMap<String, u64>,
    /// Complete pieces: (archive hash, piece name) → size.
    pieces: HashMap<(String, String), u64>,
    /// Pieces' sizes as planned, by archive hash (`sizes.json`).
    sizes: HashMap<String, BTreeMap<String, u64>>,
    /// The copy under way.
    copying: Option<Copying>,
    /// What the last sync did, and when.
    last: Option<(SyncStats, SystemTime)>,
}

/// The local copy of what this Mac has downloaded.
pub struct Mirror {
    root: PathBuf,
    reserve: u64,
    state: Mutex<State>,
    free_space: Box<FreeSpace>,
    on_remove: Mutex<Option<Arc<OnRemove>>>,
}

/// Clears the copy under way when its copy returns, however it returns.
struct CopyingGuard<'a>(&'a Mirror);

impl Drop for CopyingGuard<'_> {
    fn drop(&mut self) {
        self.0.state().copying = None;
    }
}

/// Keeps copies to `SLOW_RATE` while the build runs.
struct Pace<'a> {
    slow: &'a dyn Fn() -> bool,
    since: Instant,
    bytes: u64,
}

impl Pace<'_> {
    /// After `n` bytes were read: waits as long as keeps the rate, while the build runs.
    fn after(&mut self, n: u64, stop: &dyn Fn() -> bool) {
        if !(self.slow)() {
            self.since = Instant::now();
            self.bytes = 0;
            return;
        }
        self.bytes += n;
        let due = Duration::from_secs_f64(self.bytes as f64 / SLOW_RATE as f64);
        while self.since.elapsed() < due && !stop() {
            std::thread::sleep((due - self.since.elapsed()).min(Duration::from_millis(250)));
        }
    }
}

/// A NAS file read through the pool with the copies' own timeout.
struct NasFile {
    file: Arc<File>,
    len: u64,
    pool: Arc<IoPool>,
    timeout: Duration,
}

impl RangeRead for NasFile {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.len)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        crate::range::check_range(self.len, off, len)?;
        self.pool.read_at_timeout(&self.file, off, len, self.timeout)
    }
}

/// Whether an error means the NAS can't be reached.
fn unreachable(e: &anyhow::Error) -> bool {
    IoError::find(e).is_some_and(IoError::is_unreachable)
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
        // (The use times of the mirror before downloads: nothing reads them now.)
        let _ = fs::remove_file(root.join("mirror").join(OLD_USES));
        let (pieces, sizes) = scan_pieces(&root.join("mirror").join(BASEMAP));
        Ok(Mirror {
            root,
            reserve: reserve_bytes,
            state: Mutex::new(State { files, pieces, sizes, copying: None, last: None }),
            free_space: Box::new(disk_free),
            on_remove: Mutex::new(None),
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

    /// Has `f` told what a sync deletes, as it goes (the server drops what it mapped of it).
    pub fn on_remove(&self, f: impl Fn(&[String]) + Send + Sync + 'static) {
        *self.on_remove.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    /// Where a local copy lives (whether or not it's there).
    pub fn path(&self, content_name: &str) -> PathBuf {
        self.root.join("mirror").join(content_name)
    }

    fn partial_path(&self, content_name: &str) -> PathBuf {
        self.root.join("mirror").join(PARTIAL).join(content_name)
    }

    fn pieces_dir(&self, hash: &str) -> PathBuf {
        self.root.join("mirror").join(BASEMAP).join(hash)
    }

    /// Where a piece of `archive` (a content name) lives, whether or not it's there.
    pub fn piece_path(&self, archive: &str, piece: Piece) -> PathBuf {
        self.pieces_dir(hash_of(archive).unwrap_or("none")).join(format!("{}.pmtiles", piece.name()))
    }

    fn piece_part(&self, archive: &str, piece: Piece) -> PathBuf {
        self.pieces_dir(hash_of(archive).unwrap_or("none")).join(format!("{}.pmtiles{PART}", piece.name()))
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

    /// The piece `piece` of `archive`, when it's here and complete.
    pub fn piece_local(&self, archive: &str, piece: Piece) -> Option<PathBuf> {
        let key = (hash_of(archive)?.to_string(), piece.name());
        if !self.state().pieces.contains_key(&key) {
            return None;
        }
        let p = self.piece_path(archive, piece);
        if p.is_file() {
            return Some(p);
        }
        self.state().pieces.remove(&key);
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

    /// A piece's size: its file's when it's here, else as planned (None: not worked out yet).
    pub fn piece_size(&self, archive: &str, piece: Piece) -> Option<u64> {
        let h = hash_of(archive)?;
        let name = piece.name();
        let st = self.state();
        st.pieces.get(&(h.to_string(), name.clone())).copied().or_else(|| st.sizes.get(h).and_then(|m| m.get(&name)).copied())
    }

    /// Whether a piece is here whole.
    pub fn has_piece(&self, archive: &str, piece: Piece) -> bool {
        hash_of(archive).is_some_and(|h| self.state().pieces.contains_key(&(h.to_string(), piece.name())))
    }

    /// The bytes of `item` here: all of it when complete, what's copied of it when in progress.
    pub fn item_here(&self, item: &Item) -> u64 {
        match item {
            Item::File { name, size } => {
                if self.has(name) {
                    *size
                } else {
                    fs::metadata(self.partial_path(name)).map_or(0, |m| m.len()).min(*size)
                }
            }
            Item::Piece { archive, piece } => match self.has_piece(archive, *piece) {
                true => self.piece_size(archive, *piece).unwrap_or(0),
                false => fs::metadata(self.piece_part(archive, *piece)).map_or(0, |m| m.len()).min(self.piece_size(archive, *piece).unwrap_or(0)),
            },
        }
    }

    /// `item`'s size, None for a piece not sized yet.
    pub fn item_size(&self, item: &Item) -> Option<u64> {
        match item {
            Item::File { size, .. } => Some(*size),
            Item::Piece { archive, piece } => self.piece_size(archive, *piece),
        }
    }

    /// Complete local copies and pieces, and their total size.
    pub fn usage(&self) -> (usize, u64) {
        let st = self.state();
        (st.files.len() + st.pieces.len(), st.files.values().sum::<u64>() + st.pieces.values().sum::<u64>())
    }

    /// The copy under way, if any.
    pub fn copying(&self) -> Option<Copying> {
        self.state().copying.clone()
    }

    /// What the last sync did, and when.
    pub fn last(&self) -> Option<(SyncStats, SystemTime)> {
        self.state().last
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

    /// Works out the sizes of `wanted` pieces of `archive` (a content name) not known yet, from
    /// `pm` (the archive, opened: only its directory is read), and keeps them (`sizes.json`),
    /// until `stop()`. How many it worked out.
    pub fn size_pieces(&self, archive: &str, wanted: &[Piece], pm: &PmTiles, stop: &dyn Fn() -> bool) -> Result<u32> {
        let h = hash_of(archive).with_context(|| format!("{archive:?} isn't a content name"))?.to_string();
        let mut n = 0;
        let mut saved = Instant::now();
        for &p in wanted {
            if self.piece_size(archive, p).is_some() {
                continue;
            }
            if stop() {
                break;
            }
            let size = pieces::plan(pm, p)?.size;
            self.state().sizes.entry(h.clone()).or_default().insert(p.name(), size);
            n += 1;
            if saved.elapsed() > Duration::from_secs(10) {
                self.save_sizes(&h);
                saved = Instant::now();
            }
        }
        if n > 0 {
            self.save_sizes(&h);
        }
        Ok(n)
    }

    fn save_sizes(&self, hash: &str) {
        let body = match self.state().sizes.get(hash) {
            Some(m) => serde_json::to_vec(m).unwrap_or_default(),
            None => return,
        };
        let dir = self.pieces_dir(hash);
        if let Err(e) = fs::create_dir_all(&dir).and_then(|_| replace_file(&dir.join(SIZES), &body, None).map_err(io::Error::other)) {
            eprintln!("mirror: can't keep the pieces' sizes in {}: {e:#}", dir.display());
        }
    }

    /// The room for `wanted`: what it still lacks here, and how much more room that needs than the
    /// free space above the reserve (`Room`).
    pub fn room(&self, wanted: &Wanted) -> Result<Room> {
        let free = self.free()?;
        let mut missing = 0;
        let mut unknown = 0;
        for i in &wanted.items {
            match self.item_size(i) {
                Some(s) => missing += s - self.item_here(i).min(s),
                None => unknown += 1,
            }
        }
        let more = missing.saturating_sub(free.saturating_sub(self.reserve));
        Ok(Room { free, reserve: self.reserve, missing, more, unknown })
    }

    /// Lets go of what `wanted` doesn't name: the files and pieces here, copies in progress, the
    /// pieces of archives `cat` no longer has (their sizes too). Away from home as at home; not
    /// while `hold()`. What it did.
    pub fn let_go(&self, cat: &Catalog, wanted: &Wanted, hold: &dyn Fn() -> bool) -> Result<SyncStats> {
        let mut stats = SyncStats::default();
        if hold() {
            return Ok(stats);
        }
        let files = wanted.files();
        let gone: Vec<(String, u64)> = self.state().files.iter().filter(|(n, _)| !files.contains(n.as_str())).map(|(n, &s)| (n.clone(), s)).collect();
        for (name, size) in gone {
            let p = self.path(&name);
            match fs::remove_file(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    eprintln!("mirror: can't delete {}: {e}", p.display());
                    continue;
                }
            }
            self.remove_empty_dirs(&p);
            self.state().files.remove(&name);
            self.told(&name);
            stats.removed += 1;
            stats.removed_bytes += size;
        }
        let mut partials = HashMap::new();
        let _ = scan(&self.root.join("mirror").join(PARTIAL), "", &mut partials);
        for (name, size) in partials {
            if !files.contains(name.as_str()) {
                let p = self.partial_path(&name);
                if fs::remove_file(&p).is_ok() {
                    self.remove_empty_dirs(&p);
                    stats.removed += 1;
                    stats.removed_bytes += size;
                }
            }
        }
        // Pieces: those of archives the catalog no longer has go with their folder.
        let pieces = wanted.pieces();
        let current: HashSet<String> = cat.basemap.iter().filter_map(|l| cat.content(l)).filter_map(hash_of).map(str::to_string).collect();
        let base = self.root.join("mirror").join(BASEMAP);
        for e in fs::read_dir(&base).into_iter().flatten().flatten() {
            let Ok(hash) = e.file_name().into_string() else { continue };
            let dir = e.path();
            for f in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let Ok(fname) = f.file_name().into_string() else { continue };
                if fname == SIZES && current.contains(&hash) {
                    continue;
                }
                let stem = fname.strip_suffix(PART).unwrap_or(&fname).strip_suffix(".pmtiles");
                if stem.is_some_and(|s| pieces.contains(&(hash.clone(), s.to_string()))) {
                    continue;
                }
                let size = f.metadata().map_or(0, |m| m.len());
                if fs::remove_file(f.path()).is_ok() {
                    if let Some(s) = stem.filter(|_| !fname.ends_with(PART)) {
                        self.state().pieces.remove(&(hash.clone(), s.to_string()));
                        self.told(&format!("{BASEMAP}/{hash}/{fname}"));
                    }
                    if fname != SIZES {
                        stats.removed += 1;
                        stats.removed_bytes += size;
                    }
                }
            }
            if !current.contains(&hash) {
                self.state().sizes.remove(&hash);
            }
            let _ = fs::remove_dir(&dir);
        }
        Ok(stats)
    }

    fn told(&self, name: &str) {
        let hook = self.on_remove.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(f) = hook {
            f(&[name.to_string()]);
        }
    }

    /// Lets go of what isn't wanted (unless `ctl.hold()`), then copies what `wanted` names and
    /// isn't here yet from `nas_root`, through `pool`, in its order, until done, `ctl.stop()` says
    /// stop, or the NAS goes offline. What doesn't fit waits (module doc). Run it on a background
    /// thread; it never holds the mirror's lock while it waits on the NAS.
    pub fn sync(&self, cat: &Catalog, wanted: &Wanted, nas_root: &Path, pool: &Arc<IoPool>, ctl: &Control) -> Result<SyncStats> {
        self.drop_mismatched(cat);
        let mut stats = self.let_go(cat, wanted, ctl.hold)?;
        let mut pace = Pace { slow: ctl.slow, since: Instant::now(), bytes: 0 };
        let mut archives: HashMap<String, Arc<PmTiles>> = HashMap::new();
        let mut left = wanted.items.iter().filter(|i| !self.is_here(i)).count() as u32;
        for item in &wanted.items {
            if self.is_here(item) {
                continue;
            }
            if (ctl.stop)() {
                stats.end = SyncEnd::Paused;
                break;
            }
            if !pool.is_online() {
                stats.end = SyncEnd::Offline;
                break;
            }
            let got = match item {
                Item::File { name, size } => self.copy_file(name, *size, nas_root, pool, ctl, &mut pace, &mut stats)?,
                Item::Piece { archive, piece } => self.copy_piece(archive, *piece, nas_root, pool, ctl, &mut pace, &mut archives, &mut stats)?,
            };
            match got {
                Some(Copy::Done(bytes)) => {
                    stats.copied += 1;
                    stats.copied_bytes += bytes;
                    left -= 1;
                }
                // (Waiting for room: counted.)
                None => {}
                Some(Copy::Paused) => {
                    stats.end = SyncEnd::Paused;
                    break;
                }
                Some(Copy::Offline) => {
                    stats.end = SyncEnd::Offline;
                    break;
                }
                Some(Copy::Failed(why)) => {
                    eprintln!("mirror: {}: {why}", label(item));
                    stats.failed += 1;
                }
            }
        }
        stats.pending = left;
        self.tidy(cat);
        self.state().last = Some((stats, SystemTime::now()));
        Ok(stats)
    }

    /// Away from the NAS: lets go of what isn't wanted, copies nothing. What it did.
    pub fn sync_away(&self, cat: &Catalog, wanted: &Wanted, hold: &dyn Fn() -> bool) -> Result<SyncStats> {
        let mut stats = self.let_go(cat, wanted, hold)?;
        stats.end = SyncEnd::Offline;
        stats.pending = wanted.items.iter().filter(|i| !self.is_here(i)).count() as u32;
        self.state().last = Some((stats, SystemTime::now()));
        Ok(stats)
    }

    fn is_here(&self, item: &Item) -> bool {
        match item {
            Item::File { name, .. } => self.has(name),
            Item::Piece { archive, piece } => self.has_piece(archive, *piece),
        }
    }

    /// Whether `need` more bytes fit above the reserve; when not, counted as waiting.
    fn fits(&self, need: u64, stats: &mut SyncStats) -> Result<bool> {
        let room = self.free()?.saturating_sub(self.reserve);
        if need <= room {
            return Ok(true);
        }
        stats.waiting += 1;
        stats.waiting_bytes += need;
        Ok(false)
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

    /// Copies a file of the catalog (None: it waits for room).
    #[allow(clippy::too_many_arguments)]
    fn copy_file(&self, name: &str, size: u64, nas_root: &Path, pool: &Arc<IoPool>, ctl: &Control, pace: &mut Pace, stats: &mut SyncStats) -> Result<Option<Copy>> {
        let Some(c) = parse_content_name(name) else { return Ok(Some(Copy::Failed("not a content name".into()))) };
        let part = self.partial_path(name);
        let have = fs::metadata(&part).map_or(0, |m| m.len()).min(size);
        if !self.fits(size - have, stats)? {
            return Ok(None);
        }
        let (file, len) = match pool.open_len(&nas_root.join(name)) {
            Ok(v) => v,
            Err(e) if e.is_unreachable() => return Ok(Some(Copy::Offline)),
            Err(e) => return Ok(Some(Copy::Failed(format!("open on the NAS: {e}")))),
        };
        if len != size {
            return Ok(Some(Copy::Failed(format!("{len} bytes on the NAS, but the catalog says {size}"))));
        }
        if let Some(dir) = part.parent() {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let mut out = OpenOptions::new().create(true).append(true).open(&part).with_context(|| format!("open {}", part.display()))?;
        let mut have = out.metadata()?.len();
        let mut hasher = blake3::Hasher::new();
        if have > size {
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
        self.state().copying = Some(Copying { name: name.to_string(), size, have, slow: (ctl.slow)() });
        let _clear = CopyingGuard(self);
        let timeout = pool.config().op_timeout.max(CHUNK_TIMEOUT);
        while have < size {
            if (ctl.stop)() {
                out.sync_all()?;
                return Ok(Some(Copy::Paused));
            }
            let n = (size - have).min(CHUNK) as usize;
            let buf = match pool.read_at_timeout(&file, have, n, timeout) {
                Ok(b) => b,
                Err(IoError::Offline | IoError::Timeout) => {
                    out.sync_all()?;
                    return Ok(Some(Copy::Offline));
                }
                Err(e) => return Ok(Some(Copy::Failed(format!("read on the NAS: {e}")))),
            };
            hasher.update(&buf);
            out.write_all(&buf).with_context(|| format!("write {}", part.display()))?;
            have += n as u64;
            let slow = (ctl.slow)();
            if let Some(cur) = self.state().copying.as_mut() {
                cur.have = have;
                cur.slow = slow;
            }
            pace.after(n as u64, ctl.stop);
        }
        out.sync_all()?;
        drop(out);
        let got = hex16(&hasher.finalize());
        if got != c.hash16 {
            let _ = fs::remove_file(&part);
            return Ok(Some(Copy::Failed(format!("the copy hashes to {got}; the NAS file is damaged"))));
        }
        let dest = self.path(name);
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        fs::rename(&part, &dest).with_context(|| format!("move {} into the mirror", part.display()))?;
        self.remove_empty_dirs(&part);
        self.state().files.insert(name.to_string(), size);
        Ok(Some(Copy::Done(size)))
    }

    /// The archive `name` on the NAS, opened (once a sync).
    fn archive(&self, name: &str, nas_root: &Path, pool: &Arc<IoPool>, open: &mut HashMap<String, Arc<PmTiles>>) -> Result<Arc<PmTiles>> {
        if let Some(a) = open.get(name) {
            return Ok(a.clone());
        }
        let (file, len) = pool.open_len(&nas_root.join(name))?;
        let timeout = pool.config().op_timeout.max(CHUNK_TIMEOUT);
        let a = Arc::new(PmTiles::with_leaf_cache(Box::new(NasFile { file, len, pool: pool.clone(), timeout }), 512)?);
        open.insert(name.to_string(), a.clone());
        Ok(a)
    }

    /// Makes a piece of a basemap archive (None: it waits for room).
    #[allow(clippy::too_many_arguments)]
    fn copy_piece(
        &self,
        archive: &str,
        piece: Piece,
        nas_root: &Path,
        pool: &Arc<IoPool>,
        ctl: &Control,
        pace: &mut Pace,
        open: &mut HashMap<String, Arc<PmTiles>>,
        stats: &mut SyncStats,
    ) -> Result<Option<Copy>> {
        let Some(hash) = hash_of(archive).map(str::to_string) else { return Ok(Some(Copy::Failed("not a content name".into()))) };
        let fail = |e: anyhow::Error| if unreachable(&e) { Copy::Offline } else { Copy::Failed(format!("{e:#}")) };
        let pm = match self.archive(archive, nas_root, pool, open) {
            Ok(a) => a,
            Err(e) => return Ok(Some(fail(e))),
        };
        let plan = match pieces::plan(&pm, piece) {
            Ok(p) => p,
            Err(e) => return Ok(Some(fail(e))),
        };
        if self.piece_size(archive, piece) != Some(plan.size) {
            self.state().sizes.entry(hash.clone()).or_default().insert(piece.name(), plan.size);
            self.save_sizes(&hash);
        }
        let part = self.piece_part(archive, piece);
        let have = fs::metadata(&part).map_or(0, |m| m.len()).min(plan.size);
        if !self.fits(plan.size - have, stats)? {
            return Ok(None);
        }
        fs::create_dir_all(self.pieces_dir(&hash))?;
        let name = copy_name(&Item::Piece { archive: archive.to_string(), piece });
        self.state().copying = Some(Copying { name, size: plan.size, have, slow: (ctl.slow)() });
        let _clear = CopyingGuard(self);
        let pace = std::cell::RefCell::new(pace);
        let read = |off: u64, len: usize| {
            let b = pm.source().read_at(off, len)?;
            pace.borrow_mut().after(len as u64, ctl.stop);
            Ok(b)
        };
        let stop = || (ctl.stop)() || !pool.is_online();
        let mut progress = |h: u64| {
            let slow = (ctl.slow)();
            if let Some(cur) = self.state().copying.as_mut() {
                cur.have = h;
                cur.slow = slow;
            }
        };
        match pieces::write(&plan, &part, &read, &stop, &mut progress) {
            Ok(Wrote::Done) => {}
            Ok(Wrote::Stopped) if !pool.is_online() => return Ok(Some(Copy::Offline)),
            Ok(Wrote::Stopped) => return Ok(Some(Copy::Paused)),
            Err(e) if unreachable(&e) => return Ok(Some(Copy::Offline)),
            Err(e) => {
                let _ = fs::remove_file(&part);
                return Ok(Some(Copy::Failed(format!("{e:#}"))));
            }
        }
        fs::rename(&part, self.piece_path(archive, piece)).with_context(|| format!("move {} into place", part.display()))?;
        self.state().pieces.insert((hash, piece.name()), plan.size);
        Ok(Some(Copy::Done(plan.size)))
    }

    /// After a sync: drops pack indexes no recent catalog needs (and temporary files an
    /// interrupted write left in `idx/`).
    fn tidy(&self, cat: &Catalog) {
        let mut recent: HashSet<String> = cat.files.values().filter_map(|f| parse_content_name(&f.file).map(|c| c.hash16.to_string())).collect();
        let dir = self.root.join("catalog");
        if let Ok(ns) = catalog::list(&dir) {
            if let Some(prev) = ns.into_iter().filter(|&n| n < cat.n).find_map(|n| catalog::read(&dir.join(catalog::file_name(n))).ok()) {
                recent.extend(prev.files.values().filter_map(|f| parse_content_name(&f.file).map(|c| c.hash16.to_string())));
            }
        }
        if let Ok(rd) = fs::read_dir(self.root.join("idx")) {
            for e in rd.flatten() {
                let name = e.file_name();
                let Some(name) = name.to_str() else { continue };
                let stale = match name.strip_suffix(".idx") {
                    Some(h) => !recent.contains(h),
                    // A temporary file an hour old belongs to no write in progress.
                    None => name.ends_with(".tmp") && e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|a| a > STALE_TMP)),
                };
                if stale {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }

    #[cfg(test)]
    fn with_free_space(mut self, f: impl Fn(&Path) -> io::Result<u64> + Send + Sync + 'static) -> Self {
        self.free_space = Box::new(f);
        self
    }
}

/// The name the copy under way goes by (`Copying::name`) while it's `item`'s.
pub fn copy_name(item: &Item) -> String {
    match item {
        Item::File { name, .. } => name.clone(),
        Item::Piece { archive, piece } => format!("basemap {} {}", hash_of(archive).unwrap_or("none"), piece.name()),
    }
}

/// An item, for the log.
fn label(i: &Item) -> String {
    match i {
        Item::File { name, .. } => name.clone(),
        Item::Piece { archive, piece } => format!("{archive} {}", piece.name()),
    }
}

/// The logical names of the World download's files (plan §4, "Mirror, per Mac"): what the map
/// needs to start and to draw anywhere zoomed out, and the small per-tile records any view's lists
/// read. The worldwide files the build makes (`global/…`: rail frequencies, the road → units index,
/// landmark totals, heritage summaries, today's converted layer files and details, roads' English
/// names), every layer's root and lo packs (zooms 0–8), and the landmark points and area details
/// (markdata, ovdata). Not the pass's area outlines (a worldwide file under `sources/`, read only to
/// make new regions), nor the basemap (its pieces: crate::pieces).
fn essential_logicals(cat: &Catalog) -> std::collections::HashSet<&str> {
    let mut e: HashSet<&str> = cat.global.values().map(String::as_str).filter(|l| l.starts_with("global/")).collect();
    for l in cat.layers.values() {
        e.extend(l.root.iter().map(String::as_str));
        e.extend(l.lo.values().map(String::as_str));
    }
    e.extend(cat.markdata.values().map(String::as_str));
    e.extend(cat.ovdata.values().map(String::as_str));
    e
}

/// The content names of `cat`'s essentials (`essential_logicals`), in copy order (`copy_order`).
pub fn essentials(cat: &Catalog) -> Vec<String> {
    let mut v: Vec<&str> = essential_logicals(cat).into_iter().collect();
    copy_order(cat, &mut v);
    v.into_iter().filter_map(|l| cat.content(l).map(str::to_string)).collect()
}

/// Sorts logical names in copy order: small worldwide files, root and lo packs, hi data and road
/// values (and the per-tile records), base packs, hi packs, the 3D buildings' hi packs (a city's z6
/// tile is hundreds of MB: the roads and terrain first), and anything else; by name within each.
pub fn copy_order(cat: &Catalog, logicals: &mut [&str]) {
    let g = groups(cat);
    logicals.sort_by(|a, b| g.get(a).copied().unwrap_or(LAST_GROUP).cmp(&g.get(b).copied().unwrap_or(LAST_GROUP)).then(a.cmp(b)));
}

/// The 3D buildings' layer in the catalog (pipeline::bld::LAYER).
const BUILDINGS_LAYER: &str = "buildings";
const LAST_GROUP: u8 = 6;

fn groups(cat: &Catalog) -> HashMap<&str, u8> {
    fn set<'a>(g: &mut HashMap<&'a str, u8>, logical: &'a str, k: u8) {
        let e = g.entry(logical).or_insert(k);
        *e = (*e).min(k);
    }
    let mut g = HashMap::new();
    for v in cat.global.values() {
        set(&mut g, v, 0);
    }
    for (name, l) in &cat.layers {
        if let Some(r) = &l.root {
            set(&mut g, r, 1);
        }
        for v in l.lo.values() {
            set(&mut g, v, 1);
        }
        let hi = if name == BUILDINGS_LAYER { 5 } else { 4 };
        for v in l.hi.values() {
            set(&mut g, v, hi);
        }
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
/// relative to the top, with its size.
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

type PieceSizes = (HashMap<(String, String), u64>, HashMap<String, BTreeMap<String, u64>>);

/// The complete pieces under `dir` (`.basemap/`), and the sizes kept for each archive.
fn scan_pieces(dir: &Path) -> PieceSizes {
    let (mut pieces, mut sizes) = (HashMap::new(), HashMap::new());
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(hash) = e.file_name().into_string() else { continue };
        if let Ok(b) = fs::read(e.path().join(SIZES)) {
            if let Ok(m) = serde_json::from_slice::<BTreeMap<String, u64>>(&b) {
                sizes.insert(hash.clone(), m);
            }
        }
        for f in fs::read_dir(e.path()).into_iter().flatten().flatten() {
            let Ok(name) = f.file_name().into_string() else { continue };
            if let Some(stem) = name.strip_suffix(".pmtiles").filter(|s| Piece::parse(s).is_some()) {
                pieces.insert((hash.clone(), stem.to_string()), f.metadata().map_or(0, |m| m.len()));
            }
        }
    }
    (pieces, sizes)
}

/// Free bytes on the (local) filesystem holding `path`, for an unprivileged user.
fn disk_free(path: &Path) -> io::Result<u64> {
    crate::sys::disk_free(path)
}

#[cfg(test)]
mod tests;
