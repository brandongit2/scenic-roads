//! The NAS project folder as a type (docs/nas-access.md): every NAS read, write, listing and stat
//! goes through a `NasPath`, which counts it (crate::nasio) and retries the reads, stats and
//! listings the share fails for a moment (crate::retry). A `NasPath` is no `Path`: the one way to
//! a raw path is `unchecked_path`, which crates/pipeline/tests/nas_access.rs lists by site.
//!
//! What a method does is what `std::fs` (or crate::whole, for `write_whole`, `read`,
//! `read_replaced`, `copy_to_local`, `copy_from_local`) does on the same path: same bytes, same
//! names, same temporary names. On top, each counts as crate::nasio says (a file read: one `Files`
//! and its bytes, noted in the ring; a file made: its bytes and one `Creates`, noted; a stat one
//! `Stats`; a listing its entries; a remove one `Deletes`), and reads, stats and listings are
//! tried again on a transient error (`RETRY`). Writes, removes and renames are never tried again
//! here (a lost answer can't tell made from not made); nor are create-new and append.

use crate::nasio::{self, Count};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Whether reads, stats and listings are tried again on a transient error (decision D2, #663): one
/// constant, so turning the widening off needs no revert of the types.
pub const RETRY: bool = true;

/// Whether `p` is the real NAS's or the app's real home: a test never writes there.
pub fn is_live_path(p: &Path) -> bool {
    p.starts_with("/Volumes")
        || std::env::var_os("HOME")
            .is_some_and(|h| p.starts_with(Path::new(&h).join("Library/Application Support/scenic")))
}

/// How long a root's reads are retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Waits {
    /// crate::retry::waits: ~3.5 min (build jobs, the agent, the CLI).
    Long,
    /// crate::retry::short_waits: ~5 s (the server: an HTTP worker never stalls for minutes, D3).
    Short,
    /// None: the agent's loop, the pool's thread and the coordinator (their reads fail at once, as
    /// before #663: a stalled loop stops planning and the pool, and its next beat reads again).
    Never,
}

struct Inner {
    root: PathBuf,
    /// The folders this process has made (or found made) under it (`make_parent`): the writes'
    /// hot path makes no folder twice; one is dropped again when a write into it fails (`forget`).
    /// (Shared by the root's copies with other schedules.)
    made: Arc<Mutex<BTreeSet<PathBuf>>>,
    waits: Waits,
}

/// The NAS project folder (or, in tests, a local folder standing in for it). Cheap to clone.
#[derive(Clone)]
pub struct NasRoot(Arc<Inner>);

impl std::fmt::Debug for NasRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "nas-root:{}", self.0.root.display())
    }
}

impl PartialEq for NasRoot {
    fn eq(&self, o: &NasRoot) -> bool {
        self.0.root == o.0.root
    }
}

impl Eq for NasRoot {}

impl NasRoot {
    fn new(root: PathBuf) -> NasRoot {
        NasRoot(Arc::new(Inner { root, made: Arc::default(), waits: Waits::Long }))
    }

    /// The share's project folder where it is mounted; when it isn't and `mount` is set, mounted
    /// first (at home by the LAN name, away through Tailscale when the Keychain has the bare name's
    /// password; else nothing: a dialog would ask for it).
    pub fn find(mount: bool) -> Option<NasRoot> {
        use crate::nas::{find_mount, HOST, PROJECT, SHARE};
        if let Some(m) = find_mount(HOST, SHARE) {
            return Some(NasRoot::new(m.point.join(PROJECT)));
        }
        if mount {
            let url = if crate::nas::at_home() { Some(crate::nas::smb_url()) } else { crate::nas::tunnel_url() };
            match url {
                Some(url) => {
                    if let Err(e) = crate::nas::mount(url, Duration::from_secs(60)) {
                        eprintln!("agent: mounting the NAS: {e:#}");
                    }
                }
                None => eprintln!(
                    "agent: away from home, and the Keychain has no password for the NAS's Tailscale name ({HOST})"
                ),
            }
            return find_mount(HOST, SHARE).map(|m| NasRoot::new(m.point.join(PROJECT)));
        }
        None
    }

    /// The root a user (or a parent process's spec) named: `--root DIR`, the server's config.
    pub fn from_arg(dir: impl Into<PathBuf>) -> NasRoot {
        NasRoot::new(dir.into())
    }

    /// A local folder standing in for the NAS, in a test: never the real NAS or app home.
    #[cfg(any(test, feature = "testkit"))]
    #[track_caller]
    pub fn local_for_test(dir: impl Into<PathBuf>) -> NasRoot {
        let dir = dir.into();
        assert!(!is_live_path(&dir), "a test's root is the real NAS: {}", dir.display());
        NasRoot::new(dir)
    }

    /// This root with the server's short retries (`Waits::Short`, ~5 s).
    pub fn short_retry(&self) -> NasRoot {
        self.with_waits(Waits::Short)
    }

    /// This root with no retries (`Waits::Never`): the agent's loop, the pool, the coordinator.
    pub fn no_retry(&self) -> NasRoot {
        self.with_waits(Waits::Never)
    }

    /// This root with the build's retries (`Waits::Long`, ~3.5 min, the default): a thread the
    /// agent's loop starts and never waits on.
    pub fn long_retry(&self) -> NasRoot {
        self.with_waits(Waits::Long)
    }

    fn with_waits(&self, waits: Waits) -> NasRoot {
        NasRoot(Arc::new(Inner { root: self.0.root.clone(), made: self.0.made.clone(), waits }))
    }

    /// The root folder itself.
    pub fn path(&self) -> NasPath {
        NasPath { root: self.clone(), rel: String::new() }
    }

    /// `rel` under the root ('/'-separated; never `..` or absolute).
    #[track_caller]
    pub fn join(&self, rel: impl AsRef<str>) -> NasPath {
        self.path().join(rel)
    }

    /// The root as a raw path: an escape from the type, listed by site in tests/nas_access.rs.
    pub fn unchecked_path(&self) -> PathBuf {
        self.0.root.clone()
    }

    /// The root's full path for a message (the same text as `Path::display`'s).
    pub fn display(&self) -> std::path::Display<'_> {
        self.0.root.display()
    }

    fn waits(&self) -> Vec<Duration> {
        match self.0.waits {
            Waits::Long => crate::retry::waits(),
            Waits::Short => crate::retry::short_waits(),
            Waits::Never => Vec::new(),
        }
    }
}

/// A file or folder under a `NasRoot`. Two are equal when they name the same file (their full
/// paths), whichever root each was joined from.
#[derive(Clone)]
pub struct NasPath {
    root: NasRoot,
    /// '/'-joined parts, none empty, `.` or `..`; "" for the root.
    rel: String,
}

impl std::fmt::Debug for NasPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "nas:{}", self.rel)
    }
}

impl PartialEq for NasPath {
    fn eq(&self, o: &NasPath) -> bool {
        self.full() == o.full()
    }
}

impl Eq for NasPath {}

impl PartialOrd for NasPath {
    fn partial_cmp(&self, o: &NasPath) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for NasPath {
    fn cmp(&self, o: &NasPath) -> std::cmp::Ordering {
        self.full().cmp(&o.full())
    }
}

impl std::hash::Hash for NasPath {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.full().hash(h);
    }
}

/// A folder's entry (`NasPath::read_dir`).
#[derive(Clone, Debug)]
pub struct NasEntry {
    pub path: NasPath,
    pub name: String,
    file_type: Option<std::fs::FileType>,
}

impl NasEntry {
    /// Whether it is a folder, by the listing (no stat; false when the listing didn't say).
    pub fn is_dir(&self) -> bool {
        self.file_type.is_some_and(|t| t.is_dir())
    }

    /// Whether it is a file, by the listing (no stat).
    pub fn is_file(&self) -> bool {
        self.file_type.is_some_and(|t| t.is_file())
    }

    /// Its type by the listing (a stat when the listing didn't say).
    pub fn file_type(&self) -> io::Result<std::fs::FileType> {
        match self.file_type {
            Some(t) => Ok(t),
            None => self.path.symlink_metadata().map(|m| m.file_type()),
        }
    }

    /// A stat of it.
    pub fn metadata(&self) -> io::Result<std::fs::Metadata> {
        self.path.metadata()
    }
}

/// What a create-new did.
#[derive(Debug)]
pub enum Created {
    /// This call made the file, and its bytes landed.
    Made,
    /// Something had that name: nothing was done.
    There,
    /// This call made the file, but its bytes didn't all land (the share went away between the
    /// two): the file is this Mac's, empty or short, for it to finish with a whole write (no other
    /// Mac's create of that name can succeed), or to leave; why.
    Unwritten(anyhow::Error),
}

fn ms_since(t: Instant) -> u64 {
    t.elapsed().as_millis() as u64
}

/// Whether an error is a path's not being there.
fn not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<io::Error>().is_some_and(|io| io.kind() == io::ErrorKind::NotFound))
}

impl NasPath {
    /// A local file standing in for a NAS one, in a test (its folder the root): never on the real
    /// NAS or in the app's home.
    #[cfg(any(test, feature = "testkit"))]
    #[track_caller]
    pub fn local_for_test(p: impl Into<PathBuf>) -> NasPath {
        let p = p.into();
        let (dir, name) = (p.parent().unwrap_or(Path::new("/")), p.file_name().map(|n| n.to_string_lossy()));
        let root = NasRoot::local_for_test(dir);
        match name {
            Some(n) => root.join(n),
            None => root.path(),
        }
    }

    /// `part` under this path ('/'-separated parts; empty and `.` parts skipped). Panics on `..`
    /// or an absolute part: a NAS path never leaves its root.
    #[track_caller]
    pub fn join(&self, part: impl AsRef<str>) -> NasPath {
        let part = part.as_ref();
        assert!(!part.starts_with('/'), "a NAS path part is absolute: {part}");
        let mut rel = self.rel.clone();
        for p in part.split('/') {
            match p {
                "" | "." => {}
                ".." => panic!("a NAS path part leaves its folder: {part}"),
                p => {
                    if !rel.is_empty() {
                        rel.push('/');
                    }
                    rel.push_str(p);
                }
            }
        }
        NasPath { root: self.root.clone(), rel }
    }

    /// The path under the root ("" for the root): for logs, manifests, notes.
    pub fn rel(&self) -> &str {
        &self.rel
    }

    /// The root it is under.
    pub fn root(&self) -> &NasRoot {
        &self.root
    }

    /// Its folder; None for the root.
    pub fn parent(&self) -> Option<NasPath> {
        if self.rel.is_empty() {
            return None;
        }
        let rel = self.rel.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        Some(NasPath { root: self.root.clone(), rel })
    }

    /// Its last part; None for the root.
    pub fn file_name(&self) -> Option<&str> {
        (!self.rel.is_empty()).then(|| self.rel.rsplit('/').next().unwrap_or(&self.rel))
    }

    /// Its name's extension, as `Path::extension` says.
    pub fn extension(&self) -> Option<&str> {
        Path::new(self.file_name()?).extension().and_then(|x| x.to_str())
    }

    /// The same folder, named `name`.
    #[track_caller]
    pub fn with_file_name(&self, name: impl AsRef<str>) -> NasPath {
        match self.parent() {
            Some(d) => d.join(name),
            None => self.join(name),
        }
    }

    /// Its extension replaced, as `Path::with_extension` does.
    #[track_caller]
    pub fn with_extension(&self, ext: impl AsRef<str>) -> NasPath {
        let name = Path::new(self.file_name().unwrap_or_default()).with_extension(ext.as_ref());
        self.with_file_name(name.to_string_lossy())
    }

    /// It relative to `root` ('/'-joined), when it lies under it (by their full paths: a path
    /// joined from another root naming the same folder is under it too).
    pub fn rel_under(&self, root: &NasRoot) -> Option<String> {
        let p = self.full();
        let r = p.strip_prefix(&root.0.root).ok()?;
        Some(r.to_string_lossy().replace('\\', "/"))
    }

    /// A temporary name beside it (crate::whole::tmp_name).
    pub fn tmp_name(&self) -> NasPath {
        self.with_file_name(crate::whole::tmp_name(Path::new(self.file_name().unwrap_or_default())).to_string_lossy())
    }

    /// The raw path: an escape from the type, listed by site in tests/nas_access.rs.
    pub fn unchecked_path(&self) -> PathBuf {
        self.full()
    }

    /// Its full path for a message (the same text as `Path::display`'s: logs and errors stay as
    /// they were).
    pub fn display(&self) -> impl std::fmt::Display + '_ {
        Shown(self.full())
    }

    fn full(&self) -> PathBuf {
        if self.rel.is_empty() {
            self.root.0.root.clone()
        } else {
            self.root.0.root.join(&self.rel)
        }
    }

    /// `f()` (an operation on this path) tried again while it fails transiently, on its root's
    /// schedule (`RETRY`), its retries logged with `what`.
    pub fn retry<T>(&self, what: &str, f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
        if !RETRY || self.root.0.waits == Waits::Never {
            let mut f = f;
            return f();
        }
        let p = self.full();
        let what = format!("{what} {}", p.display());
        // (The share looked at, and waited for up to a minute, before the last try: the build's
        // schedule only, never the server's 5 s.)
        let share = (self.root.0.waits == Waits::Long).then_some(p.as_path());
        crate::retry::io(&what, share, &self.root.waits(), crate::retry::transient, f)
    }

    /// `f()` tried again after each of `waits` while it fails as `again` says (a caller's own
    /// schedule and rule: crate::retry::io on this path).
    pub fn retry_with<T>(
        &self,
        what: &str,
        waits: &[Duration],
        again: fn(&io::Error) -> bool,
        f: impl FnMut() -> io::Result<T>,
    ) -> io::Result<T> {
        crate::retry::io(what, Some(&self.full()), waits, again, f)
    }

    /// `f()` (anyhow's errors) tried again as `retry_with` does: crate::retry::any on this path.
    pub fn retry_any<T>(
        &self,
        what: &str,
        waits: &[Duration],
        again: fn(&io::Error) -> bool,
        f: impl FnMut() -> Result<T>,
    ) -> Result<T> {
        crate::retry::any(what, Some(&self.full()), waits, again, f)
    }

    // ---- reads: counted, and retried on a transient error ----

    /// Its bytes (crate::whole::read: a busy file waited out). Counted as a file read.
    pub fn read(&self) -> io::Result<Vec<u8>> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let b = self.retry("read", || crate::whole::read(&p))?;
        counted_read(&p, b.len() as u64, t);
        Ok(b)
    }

    /// Its bytes as UTF-8 (`std::fs::read_to_string`'s error on bytes that aren't).
    pub fn read_to_string(&self) -> io::Result<String> {
        String::from_utf8(self.read()?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Its bytes, a file `write_whole` replaces and nothing removes on purpose
    /// (crate::whole::read_replaced: read from its newest aside when a writer left it missing).
    pub fn read_replaced(&self) -> io::Result<Vec<u8>> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let b = self.retry("read", || crate::whole::read_replaced(&p))?;
        counted_read(&p, b.len() as u64, t);
        Ok(b)
    }

    /// Its bytes past this Mac's cache (the file opened with `F_NOCACHE` on macOS: from the
    /// server, not the unified buffer cache; an SMB lease may still answer).
    pub fn read_fresh(&self) -> io::Result<Vec<u8>> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let b = self.retry("read", || {
            let mut f = crate::whole::retried(&p, &|q| std::fs::File::open(q))?;
            #[cfg(target_os = "macos")]
            {
                use std::os::fd::AsRawFd;
                // SAFETY: fcntl on a descriptor this function owns, with an int argument.
                if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            let mut b = Vec::new();
            f.read_to_end(&mut b)?;
            Ok(b)
        })?;
        counted_read(&p, b.len() as u64, t);
        Ok(b)
    }

    /// The file opened for reading (`std::fs::File::open`; the open retried): its reads counted.
    pub fn open(&self) -> io::Result<NasFile> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let f = self.retry("open", || std::fs::File::open(&p))?;
        let len = f.metadata().map_or(0, |m| m.len());
        nasio::opened(&p, len, ms_since(t));
        Ok(NasFile { f: Some(f), path: p, wrote: None, t })
    }

    /// The file opened for positioned reads (crate::range::PlainFile: counted there; the open
    /// retried here, its reads there).
    pub fn open_plain(&self) -> io::Result<crate::range::PlainFile> {
        let _w = crate::waits::nas();
        let p = self.full();
        self.retry("open", || crate::range::PlainFile::open(&p))
    }

    /// A stat (`std::fs::metadata`). One `Stats`.
    pub fn metadata(&self) -> io::Result<std::fs::Metadata> {
        let _w = crate::waits::nas();
        let p = self.full();
        nasio::add(Count::Stats, 1);
        self.retry("stat", || std::fs::metadata(&p))
    }

    /// A stat not following a link (`std::fs::symlink_metadata`). One `Stats`.
    fn symlink_metadata(&self) -> io::Result<std::fs::Metadata> {
        let _w = crate::waits::nas();
        let p = self.full();
        nasio::add(Count::Stats, 1);
        self.retry("stat", || std::fs::symlink_metadata(&p))
    }

    /// Whether it exists (`Path::try_exists`). One `Stats`.
    pub fn try_exists(&self) -> io::Result<bool> {
        let _w = crate::waits::nas();
        let p = self.full();
        nasio::add(Count::Stats, 1);
        self.retry("stat", || p.try_exists())
    }

    /// Whether it exists (`Path::exists`: an error is "no"). One `Stats`.
    pub fn exists(&self) -> bool {
        self.metadata().is_ok()
    }

    /// Whether it is a folder (`Path::is_dir`). One `Stats`.
    pub fn is_dir(&self) -> bool {
        self.metadata().is_ok_and(|m| m.is_dir())
    }

    /// Whether it is a file (`Path::is_file`). One `Stats`.
    pub fn is_file(&self) -> bool {
        self.metadata().is_ok_and(|m| m.is_file())
    }

    /// The folder's entries (`std::fs::read_dir`, read to its end: an error midway is an error),
    /// in the listing's order. Its entries counted as `Listed`.
    pub fn read_dir(&self) -> io::Result<Vec<NasEntry>> {
        let _w = crate::waits::nas();
        let p = self.full();
        let v = self.retry("list", || {
            std::fs::read_dir(&p)?
                .map(|e| e.map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().ok())))
                .collect::<io::Result<Vec<_>>>()
        })?;
        nasio::add(Count::Listed, v.len() as u64);
        Ok(v.into_iter().map(|(name, file_type)| NasEntry { path: self.join(&name), name, file_type }).collect())
    }

    /// The names in the folder, sorted, temporary files (crate::whole::is_tmp) left out; none when
    /// it isn't there.
    pub fn list(&self) -> io::Result<Vec<String>> {
        let mut names: Vec<String> = match self.read_dir() {
            Ok(v) => v.into_iter().map(|e| e.name).filter(|n| !crate::whole::is_tmp(Path::new(n))).collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        names.sort();
        Ok(names)
    }

    /// Copies it to local file `dst`, whole (crate::whole::copy: a temporary name, renamed). The
    /// bytes copied, counted as a file read.
    pub fn copy_to_local(&self, dst: &Path) -> Result<u64> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let n = crate::whole::copy(&p, dst)?;
        counted_read(&p, n, t);
        Ok(n)
    }

    /// Copies it to local file `dst` (crate::sys::copy_data: its bytes and mode, no temporary name).
    /// Counted as a file read.
    pub fn copy_data_to_local(&self, dst: &Path) -> io::Result<u64> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let n = crate::sys::copy_data(&p, dst)?;
        counted_read(&p, n, t);
        Ok(n)
    }

    /// Counts this file read whole (`n` bytes in `ms`) by a reader of its own (crate::blobs' copy).
    pub fn count_read(&self, n: u64, ms: u64) {
        nasio::opened(&self.full(), n, ms);
        nasio::read(n);
    }

    /// Counts a file of `n` bytes made here in `ms` by a writer of its own (crate::naming's
    /// content-named upload, which counts once as the upload lands).
    pub fn count_wrote(&self, n: u64, ms: u64) {
        nasio::wrote(&self.full(), n, ms);
    }

    // ---- writes: counted, never retried ----

    /// Writes `b` as it (`std::fs::write`: in place, no temporary name). Counted as a file made.
    pub fn write(&self, b: impl AsRef<[u8]>) -> io::Result<()> {
        let _w = crate::waits::nas();
        let p = self.full();
        let b = b.as_ref();
        let t = Instant::now();
        std::fs::write(&p, b)?;
        nasio::wrote(&p, b.len() as u64, ms_since(t));
        Ok(())
    }

    /// Writes `b` as it, whole (crate::whole::write: a temporary name renamed over it, the old one
    /// renamed aside when that stays busy). Counted as a file made.
    pub fn write_whole(&self, b: impl AsRef<[u8]>) -> Result<()> {
        let _w = crate::waits::nas();
        let p = self.full();
        let b = b.as_ref();
        let t = Instant::now();
        crate::whole::write(&p, b)?;
        nasio::wrote(&p, b.len() as u64, ms_since(t));
        Ok(())
    }

    /// `write_whole`, its folder made first (once a process: `make_parent`), and again when the
    /// write finds it gone (an outside reset).
    pub fn write_whole_made(&self, b: &[u8]) -> Result<()> {
        self.make_parent()?;
        let mut r = self.write_whole(b);
        if r.as_ref().is_err_and(not_found) {
            self.forget();
            self.make_parent()?;
            r = self.write_whole(b);
        }
        if r.is_err() {
            self.forget();
        }
        r
    }

    /// Makes it holding `b` unless something has that name (the pool protocol's create-new: atomic
    /// on the server; the bytes follow the create), its folder made once a process. Never retried.
    pub fn create_new(&self, b: &[u8]) -> Result<Created> {
        let _w = crate::waits::nas();
        let p = self.full();
        self.make_parent()?;
        let open = || std::fs::OpenOptions::new().write(true).create_new(true).open(&p);
        let t = Instant::now();
        // (Its folder gone since it was made, an outside reset: made again, and tried once more.)
        let opened = match open() {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.forget();
                self.make_parent()?;
                open()
            }
            r => r,
        };
        let mut f = match opened {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(Created::There),
            Err(e) => {
                self.forget();
                return Err(e).with_context(|| format!("create {}", p.display()));
            }
        };
        let r = (|| -> Result<()> {
            f.write_all(b)?;
            f.sync_all()?;
            let n = f.metadata()?.len();
            anyhow::ensure!(n == b.len() as u64, "{n} of {} bytes written", b.len());
            Ok(())
        })();
        nasio::wrote(&p, b.len() as u64, ms_since(t));
        Ok(match r {
            Ok(()) => Created::Made,
            Err(e) => Created::Unwritten(e.context(format!("write {} (made, and left short)", p.display()))),
        })
    }

    /// The file made for writing (`std::fs::File::create`): counted as a file made, with the
    /// bytes written through it, when it is dropped.
    pub fn create(&self) -> io::Result<NasFile> {
        self.open_with(std::fs::OpenOptions::new().write(true).create(true).truncate(true))
    }

    /// The file opened with `o` (append, create-new, write in place, read and write). A file it
    /// may make is counted as made, with the bytes written through it, when it is dropped; one
    /// opened only for reading as a file read. Never retried.
    pub fn open_with(&self, o: &std::fs::OpenOptions) -> io::Result<NasFile> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let f = o.open(&p)?;
        // (OpenOptions can't be asked what it was set to: a file opened writable is one written.)
        let writable = f.metadata().is_ok_and(|_| {
            #[cfg(unix)]
            {
                use std::os::fd::AsRawFd;
                // SAFETY: F_GETFL on a descriptor this function owns.
                let fl = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETFL) };
                fl != -1 && fl & libc::O_ACCMODE != libc::O_RDONLY
            }
            #[cfg(not(unix))]
            true
        });
        if writable {
            Ok(NasFile { f: Some(f), path: p, wrote: Some(0), t })
        } else {
            let len = f.metadata().map_or(0, |m| m.len());
            nasio::opened(&p, len, ms_since(t));
            Ok(NasFile { f: Some(f), path: p, wrote: None, t })
        }
    }

    /// Copies local file `src` to it, whole (crate::whole::copy). The bytes, counted as made.
    pub fn copy_from_local(&self, src: &Path) -> Result<u64> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let n = crate::whole::copy(src, &p)?;
        nasio::wrote(&p, n, ms_since(t));
        Ok(n)
    }

    /// Copies it to `to` on the NAS (crate::sys::copy_data). Counted as a read and a file made.
    pub fn copy_data_to(&self, to: &NasPath) -> io::Result<u64> {
        let _w = crate::waits::nas();
        let (p, q) = (self.full(), to.full());
        let t = Instant::now();
        let n = crate::sys::copy_data(&p, &q)?;
        counted_read(&p, n, t);
        nasio::wrote(&q, n, ms_since(t));
        Ok(n)
    }

    /// Renames it to `to` (`std::fs::rename`).
    pub fn rename(&self, to: &NasPath) -> io::Result<()> {
        let _w = crate::waits::nas();
        std::fs::rename(self.full(), to.full())
    }

    /// Renames it over `to`, busy waited out (crate::whole::rename_over).
    pub fn rename_over(&self, to: &NasPath) -> io::Result<()> {
        let _w = crate::waits::nas();
        crate::whole::rename_over(&self.full(), &to.full())
    }

    /// Renames it to `to` unless `to` exists (then `AlreadyExists`): crate::naming::rename_no_replace.
    pub fn rename_no_replace(&self, to: &NasPath) -> io::Result<()> {
        let _w = crate::waits::nas();
        crate::naming::rename_no_replace(&self.full(), &to.full())
    }

    /// Removes the file (`std::fs::remove_file`). One `Deletes`.
    pub fn remove_file(&self) -> io::Result<()> {
        let _w = crate::waits::nas();
        std::fs::remove_file(self.full())?;
        nasio::add(Count::Deletes, 1);
        Ok(())
    }

    /// Removes the empty folder (`std::fs::remove_dir`). One `Deletes`.
    pub fn remove_dir(&self) -> io::Result<()> {
        let _w = crate::waits::nas();
        std::fs::remove_dir(self.full())?;
        nasio::add(Count::Deletes, 1);
        Ok(())
    }

    /// Removes the folder and all in it (`std::fs::remove_dir_all`). One `Deletes`.
    pub fn remove_dir_all(&self) -> io::Result<()> {
        let _w = crate::waits::nas();
        std::fs::remove_dir_all(self.full())?;
        nasio::add(Count::Deletes, 1);
        Ok(())
    }

    /// Makes the folder and those above it (`std::fs::create_dir_all`).
    pub fn create_dir_all(&self) -> io::Result<()> {
        let _w = crate::waits::nas();
        std::fs::create_dir_all(self.full())
    }

    /// Its folder made, once a process (the `made` set). On SMB `create_dir_all` may fail with
    /// EEXIST when its follow-up stat of a folder another Mac just made is stale: taken as made
    /// (the write that follows fails if the folder truly is missing), but not remembered.
    fn make_parent(&self) -> Result<()> {
        let _w = crate::waits::nas();
        let Some(d) = self.full().parent().map(Path::to_path_buf) else { return Ok(()) };
        if self.root.0.made.lock().unwrap().contains(&d) {
            return Ok(());
        }
        match std::fs::create_dir_all(&d) {
            Ok(()) => {
                self.root.0.made.lock().unwrap().insert(d);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e).with_context(|| format!("make {}", d.display())),
        }
    }

    /// A write into its folder failed: the folder is made again next time (it may have gone).
    fn forget(&self) {
        if let Some(d) = self.full().parent() {
            self.root.0.made.lock().unwrap().remove(d);
        }
    }

    /// Whether `make_parent` remembers its folder as made (tests).
    #[cfg(test)]
    fn parent_remembered(&self) -> bool {
        self.full().parent().is_some_and(|d| self.root.0.made.lock().unwrap().contains(d))
    }

    /// Sets its modification time (a reuse's touch: what GC reads as in use). Not counted.
    pub fn set_modified(&self, t: std::time::SystemTime) -> io::Result<()> {
        let _w = crate::waits::nas();
        std::fs::File::options().write(true).open(self.full())?.set_modified(t)
    }
}

/// A file either on this Mac's disk or on the NAS, for a reader that takes both (a split helper
/// would be the same code twice): its NAS side goes through `NasPath`, its local side through
/// `std::fs`. A caller passes a `&Path` or a `&NasPath`.
#[derive(Clone, Copy, Debug)]
pub enum Loc<'a> {
    Local(&'a Path),
    Nas(&'a NasPath),
}

impl<'a> From<&'a Path> for Loc<'a> {
    fn from(p: &'a Path) -> Loc<'a> {
        Loc::Local(p)
    }
}

impl<'a> From<&'a PathBuf> for Loc<'a> {
    fn from(p: &'a PathBuf) -> Loc<'a> {
        Loc::Local(p)
    }
}

impl<'a> From<&'a NasPath> for Loc<'a> {
    fn from(p: &'a NasPath) -> Loc<'a> {
        Loc::Nas(p)
    }
}

impl<'a> From<&'a Loc<'a>> for Loc<'a> {
    fn from(l: &'a Loc<'a>) -> Loc<'a> {
        *l
    }
}

impl Loc<'_> {
    /// Its bytes (`std::fs::read`; `NasPath::read`).
    pub fn read(&self) -> io::Result<Vec<u8>> {
        match self {
            Loc::Local(p) => {
                let _w = crate::waits::disk();
                std::fs::read(p)
            }
            Loc::Nas(p) => p.read(),
        }
    }

    /// Its text (`std::fs::read_to_string`; `NasPath::read_to_string`).
    pub fn read_to_string(&self) -> io::Result<String> {
        match self {
            Loc::Local(p) => {
                let _w = crate::waits::disk();
                std::fs::read_to_string(p)
            }
            Loc::Nas(p) => p.read_to_string(),
        }
    }

    /// What a call on it waits on (crate::waits): for the reads of a `File` it opened (`open`),
    /// which no guard here covers, `crate::waits::wait(loc.on())` held around each.
    pub fn on(&self) -> crate::waits::On {
        match self {
            Loc::Local(_) => crate::waits::On::Disk,
            Loc::Nas(_) => crate::waits::On::Nas,
        }
    }

    /// Opened for reading, as a `File` (a NAS file's reads not counted, nor waited on, past its
    /// open: for a library that wants a `File`; its caller's reads hold `crate::waits::wait(on())`).
    pub fn open(&self) -> io::Result<std::fs::File> {
        match self {
            Loc::Local(p) => {
                let _w = crate::waits::disk();
                std::fs::File::open(p)
            }
            Loc::Nas(p) => p.open().map(|f| f.into_file()),
        }
    }

    /// Opened for positioned reads (crate::range::PlainFile).
    pub fn open_plain(&self) -> io::Result<crate::range::PlainFile> {
        match self {
            Loc::Local(p) => crate::range::PlainFile::open(p),
            Loc::Nas(p) => p.open_plain(),
        }
    }

    /// A stat (`std::fs::metadata`; `NasPath::metadata`).
    pub fn metadata(&self) -> io::Result<std::fs::Metadata> {
        match self {
            Loc::Local(p) => {
                let _w = crate::waits::disk();
                std::fs::metadata(p)
            }
            Loc::Nas(p) => p.metadata(),
        }
    }

    /// Whether it exists.
    pub fn exists(&self) -> bool {
        match self {
            Loc::Local(p) => p.exists(),
            Loc::Nas(p) => p.exists(),
        }
    }

    /// Copies it to local file `dst` (crate::sys::copy_data; `NasPath::copy_data_to_local`).
    pub fn copy_data_to_local(&self, dst: &Path) -> io::Result<u64> {
        match self {
            Loc::Local(p) => {
                let _w = crate::waits::disk();
                crate::sys::copy_data(p, dst)
            }
            Loc::Nas(p) => p.copy_data_to_local(dst),
        }
    }

    /// Removes the file (`std::fs::remove_file`; `NasPath::remove_file`).
    pub fn remove_file(&self) -> io::Result<()> {
        match self {
            Loc::Local(p) => std::fs::remove_file(p),
            Loc::Nas(p) => p.remove_file(),
        }
    }

    /// `name` in this folder.
    #[track_caller]
    pub fn join(&self, name: &str) -> Place {
        match self {
            Loc::Local(p) => Place::Local(p.join(name)),
            Loc::Nas(p) => Place::Nas(p.join(name)),
        }
    }

    /// The names in this folder, in the listing's order (`std::fs::read_dir` read to its end; a
    /// name that isn't UTF-8 left out).
    pub fn list_names(&self) -> io::Result<Vec<String>> {
        match self {
            Loc::Local(p) => std::fs::read_dir(p)?
                .map(|e| e.map(|e| e.file_name().to_str().map(str::to_string)))
                .filter_map(|r| r.transpose())
                .collect(),
            Loc::Nas(p) => Ok(p.read_dir()?.into_iter().map(|e| e.name).collect()),
        }
    }

    /// Its last part.
    pub fn file_name(&self) -> Option<&str> {
        match self {
            Loc::Local(p) => p.file_name().and_then(|n| n.to_str()),
            Loc::Nas(p) => p.file_name(),
        }
    }

    /// Its extension.
    pub fn extension(&self) -> Option<&str> {
        match self {
            Loc::Local(p) => p.extension().and_then(|n| n.to_str()),
            Loc::Nas(p) => p.extension(),
        }
    }

    /// Its full path for a message.
    pub fn display(&self) -> String {
        match self {
            Loc::Local(p) => p.display().to_string(),
            Loc::Nas(p) => p.display().to_string(),
        }
    }

    /// The raw path, for a library that opens it itself: a NAS one is an escape from the type,
    /// listed by site in tests/nas_access.rs.
    pub fn unchecked_path(&self) -> PathBuf {
        match self {
            Loc::Local(p) => p.to_path_buf(),
            Loc::Nas(p) => p.unchecked_path(),
        }
    }
}

/// A file on this Mac's disk or on the NAS, owned (a reader's choice of a local copy or the NAS's
/// file: pipeline::localread). Readers take it as a `Loc` (`&place`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Local(PathBuf),
    Nas(NasPath),
}

impl Place {
    pub fn loc(&self) -> Loc<'_> {
        match self {
            Place::Local(p) => Loc::Local(p),
            Place::Nas(p) => Loc::Nas(p),
        }
    }
}

impl<'a> From<&'a Place> for Loc<'a> {
    fn from(p: &'a Place) -> Loc<'a> {
        p.loc()
    }
}

/// A path shown in a message.
struct Shown(PathBuf);

impl std::fmt::Display for Shown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.display().fmt(f)
    }
}

/// A file read (`len` bytes since `t`): one `Files`, its bytes, noted.
fn counted_read(p: &Path, len: u64, t: Instant) {
    nasio::opened(p, len, ms_since(t));
    nasio::read(len);
}

/// An open NAS file: reads through it are counted as they go; one written is counted as made, with
/// its bytes, when it is dropped. Derefs to the `File` for its metadata, `sync_all`, `set_len`.
pub struct NasFile {
    /// (None only once `into_file` took it.)
    f: Option<std::fs::File>,
    path: PathBuf,
    /// Some(bytes written so far) when opened writable.
    wrote: Option<u64>,
    t: Instant,
}

impl std::ops::Deref for NasFile {
    type Target = std::fs::File;
    fn deref(&self) -> &std::fs::File {
        self.f.as_ref().expect("a NasFile's file")
    }
}

impl std::ops::DerefMut for NasFile {
    fn deref_mut(&mut self) -> &mut std::fs::File {
        self.f.as_mut().expect("a NasFile's file")
    }
}

impl Read for NasFile {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = {
            let _w = crate::waits::nas();
            (**self).read(b)?
        };
        nasio::read(n as u64);
        Ok(n)
    }
}

impl Write for NasFile {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let n = {
            let _w = crate::waits::nas();
            (**self).write(b)?
        };
        if let Some(w) = &mut self.wrote {
            *w += n as u64;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let _w = crate::waits::nas();
        (**self).flush()
    }
}

impl NasFile {
    /// The `File` itself (its reads no longer counted; a written one counted as made now).
    pub fn into_file(mut self) -> std::fs::File {
        self.f.take().expect("a NasFile's file")
    }
}

impl Seek for NasFile {
    fn seek(&mut self, to: io::SeekFrom) -> io::Result<u64> {
        (**self).seek(to)
    }
}

impl Drop for NasFile {
    fn drop(&mut self) {
        // (Its close: where SMB sends what's left of a write.)
        if let Some(f) = self.f.take() {
            let _w = crate::waits::nas();
            drop(f);
        }
        if let Some(n) = self.wrote {
            nasio::wrote(&self.path, n, ms_since(self.t));
        }
    }
}

#[cfg(test)]
mod tests;
