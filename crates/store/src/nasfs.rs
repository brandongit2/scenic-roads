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
//!
//! Every call on the share is a crate::deadline call within its root's deadline (`LONG`, or
//! `SHORT` for the server's and the agent loop's roots), each try of a retry its own, made with
//! the path and bytes owned (the call may outlive its caller's wait). A read, listing or copy says
//! it moves as it goes (its deadline counted from its last move); a call that changes the share
//! names the paths it changes, so a later change of this process to one of them is refused while
//! it is held (why a late finish is harmless, class by class: docs/nas-access.md §6). A `NasFile`'s
//! reads, writes and close are calls too; a `File` taken out of one (`into_file`, `Loc::open`) is
//! a library's to read, unbounded.

use crate::deadline::{self, Progress};
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

    /// The deadline of each of its NAS calls (crate::deadline): `LONG` on the build's schedule (jobs,
    /// the CLI, the agent's own threads), `SHORT` on the server's and the agent loop's.
    fn deadline(&self) -> Duration {
        match self.0.waits {
            Waits::Long => deadline::LONG,
            Waits::Short | Waits::Never => deadline::SHORT,
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

    /// Whether it is a file, by the listing (no stat). A link made from a Mac lists as a file on
    /// the NAS's SMB share (it is stored as an XSym file); only `symlink_metadata` tells.
    pub fn is_file(&self) -> bool {
        self.file_type.is_some_and(|t| t.is_file())
    }

    /// Its type by the listing (a stat when the listing didn't say). On the SMB share a link made
    /// from a Mac lists as a file (`is_file`).
    pub fn file_type(&self) -> io::Result<std::fs::FileType> {
        match self.file_type {
            Some(t) => Ok(t),
            None => self.path.symlink_metadata().map(|m| m.file_type()),
        }
    }

    /// A stat of it that doesn't follow a link: a link is a link (`is_symlink`), never the folder
    /// or file it leads to, so a walk by it never goes through one (task #1004: a link to its own
    /// folder, `inputs/inputs`, took the backup's walk down it until "Too many levels of symbolic
    /// links").
    pub fn symlink_metadata(&self) -> io::Result<std::fs::Metadata> {
        self.path.symlink_metadata()
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

/// A whole file's read under a turn of the process's NAS limit (crate::aimd): its bytes and time
/// a sample, a timeout of the share's a halving. A turn a try (`NasPath::retry` outside it): a
/// retry's wait holds no turn, and each try's failure is its own signal.
fn taking_turn(read: impl FnOnce() -> io::Result<Vec<u8>>) -> io::Result<Vec<u8>> {
    let mut turn = crate::aimd::NAS.take();
    let r = read();
    match &r {
        Ok(b) => turn.read(b.len() as u64),
        Err(e) if crate::aimd::nas_congested(e) => turn.congested(),
        Err(_) => {}
    }
    r
}

/// A whole copy here under a turn of the process's NAS limit (crate::aimd): its time no sample (a
/// copy's length is its file's), a failure as the share's a halving (`congested`).
pub(crate) fn copying_turn<T, E>(copy: impl FnOnce() -> Result<T, E>, congested: impl Fn(&E) -> bool) -> Result<T, E> {
    let mut turn = crate::aimd::NAS.take();
    turn.unmeasured();
    let r = copy();
    if let Err(e) = &r {
        if congested(e) {
            turn.congested();
        }
    }
    r
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

    // ---- the deadline: every call on the share ----

    /// `f` (one NAS call on this path, `op` naming it) within its root's deadline (crate::deadline),
    /// given the path, owned: past the deadline the call is left to the kernel.
    fn call<T: Send + 'static>(
        &self,
        op: &'static str,
        f: impl FnOnce(&Path) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let p = self.full();
        let on = p.clone();
        deadline::call(&on.display(), &op, self.root.deadline(), move || f(&p))
    }

    /// `call`, for one that moves as it goes (`Progress::add`): its deadline counted from its last
    /// move. One that changes the share (`changes`: the paths it changes, this one among them or not)
    /// is refused at once while an earlier change of this process to one of them is held.
    fn moving<T: Send + 'static>(
        &self,
        op: &'static str,
        changes: Vec<PathBuf>,
        f: impl FnOnce(&Path, &Progress) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let p = self.full();
        let on = p.clone();
        deadline::watched(&on.display(), &op, self.root.deadline(), changes, move |m| f(&p, m))
    }

    /// A call changing the share at this path (and `also`'s), on a copy of this path (`me`): its
    /// paths held while a late call would land (`moving`).
    fn change<T: Send + 'static>(
        &self,
        op: &'static str,
        also: &[&NasPath],
        f: impl FnOnce(&NasPath) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let me = self.clone();
        let keys = std::iter::once(self.full()).chain(also.iter().map(|q| q.full())).collect();
        self.moving(op, keys, move |_, _| f(&me))
    }

    // ---- reads: counted, and retried on a transient error ----

    /// Its bytes (crate::whole::read: a busy file waited out). Counted as a file read.
    pub fn read(&self) -> io::Result<Vec<u8>> {
        let _w = crate::waits::nas();
        let t = Instant::now();
        let b = self.retry("read", || {
            taking_turn(|| self.moving("read", Vec::new(), |p, m| crate::whole::retried(p, &|q| read_told(q, m))))
        })?;
        counted_read(&self.full(), b.len() as u64, t);
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
        let t = Instant::now();
        let b = self.retry("read", || {
            taking_turn(|| {
                self.moving("read", Vec::new(), |p, m| crate::whole::read_replaced_by(p, &|q| read_told(q, m)))
            })
        })?;
        counted_read(&self.full(), b.len() as u64, t);
        Ok(b)
    }

    /// Its bytes past this Mac's cache (the file opened with `F_NOCACHE` on macOS: from the
    /// server, not the unified buffer cache; an SMB lease may still answer).
    pub fn read_fresh(&self) -> io::Result<Vec<u8>> {
        let _w = crate::waits::nas();
        let t = Instant::now();
        let b = self.retry("read", || {
            taking_turn(|| {
                self.moving("read", Vec::new(), |p, m| {
                    let f = crate::whole::retried(p, &|q| std::fs::File::open(q))?;
                    #[cfg(target_os = "macos")]
                    {
                        use std::os::fd::AsRawFd;
                        // SAFETY: fcntl on a descriptor this function owns, with an int argument.
                        if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    read_all_told(f, m)
                })
            })
        })?;
        counted_read(&self.full(), b.len() as u64, t);
        Ok(b)
    }

    /// The file opened for reading (`std::fs::File::open`; the open retried): its reads counted,
    /// each a call within its root's deadline (`NasFile`).
    pub fn open(&self) -> io::Result<NasFile> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let (f, len) = self.retry("open", || {
            self.call("open", |p| {
                let f = std::fs::File::open(p)?;
                let len = f.metadata().map_or(0, |m| m.len());
                Ok((f, len))
            })
        })?;
        nasio::opened(&p, len, ms_since(t));
        Ok(NasFile::new(f, p, None, t, self.root.deadline()))
    }

    /// The file opened for positioned reads (crate::range::PlainFile: counted there, its open and
    /// reads within their deadlines there; the open retried here).
    pub fn open_plain(&self) -> io::Result<crate::range::PlainFile> {
        let _w = crate::waits::nas();
        let p = self.full();
        self.retry("open", || crate::range::PlainFile::open(&p))
    }

    /// A stat (`std::fs::metadata`). One `Stats`.
    pub fn metadata(&self) -> io::Result<std::fs::Metadata> {
        let _w = crate::waits::nas();
        nasio::add(Count::Stats, 1);
        self.retry("stat", || self.call("stat", |p| std::fs::metadata(p)))
    }

    /// A stat not following a link (`std::fs::symlink_metadata`). One `Stats`.
    fn symlink_metadata(&self) -> io::Result<std::fs::Metadata> {
        let _w = crate::waits::nas();
        nasio::add(Count::Stats, 1);
        self.retry("stat", || self.call("stat", |p| std::fs::symlink_metadata(p)))
    }

    /// Whether it exists (`Path::try_exists`). One `Stats`.
    pub fn try_exists(&self) -> io::Result<bool> {
        let _w = crate::waits::nas();
        nasio::add(Count::Stats, 1);
        self.retry("stat", || self.call("stat", |p| p.try_exists()))
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
    /// in the listing's order (moving as its entries come: a long listing that goes on never ends
    /// for its length). Its entries counted as `Listed`.
    pub fn read_dir(&self) -> io::Result<Vec<NasEntry>> {
        let _w = crate::waits::nas();
        let v = self.retry("list", || self.moving("list", Vec::new(), |p, m| listed(p, m)))?;
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
    /// bytes copied, counted as they're read (`copy_counter`: what this Mac's cache held, not the
    /// NAS's), and the file counted opened. Moving as it copies; `dst` held while a late copy would
    /// land (a later copy to it refused meanwhile).
    pub fn copy_to_local(&self, dst: &Path) -> Result<u64> {
        let _w = crate::waits::nas();
        let t = Instant::now();
        let d = dst.to_path_buf();
        let (n, _) = copying_turn(
            || {
                self.moving("copy", vec![d.clone()], move |p, m| {
                    let mut count = copy_counter();
                    Ok(crate::whole::copy_held(p, &d, &mut |k, h| {
                        count(k, h);
                        m.add(k);
                    }))
                })?
            },
            |e| e.downcast_ref::<io::Error>().is_some_and(crate::aimd::nas_congested),
        )?;
        nasio::opened(&self.full(), n, ms_since(t));
        Ok(n)
    }

    /// Copies it to local file `dst` (crate::sys::copy_data: its bytes and mode, no temporary name).
    /// Counted as it's read (`copy_counter`: what this Mac's cache held, not the NAS's), and the
    /// file counted opened. Moving as it copies; `dst` held as `copy_to_local`'s is.
    pub fn copy_data_to_local(&self, dst: &Path) -> io::Result<u64> {
        let _w = crate::waits::nas();
        let t = Instant::now();
        let d = dst.to_path_buf();
        let (n, _) = copying_turn(
            || {
                self.moving("copy", vec![d.clone()], move |p, m| {
                    let mut count = copy_counter();
                    crate::sys::copy_data_held(p, &d, &mut |k, h| {
                        count(k, h);
                        m.add(k);
                    })
                })
            },
            crate::aimd::nas_congested,
        )?;
        nasio::opened(&self.full(), n, ms_since(t));
        Ok(n)
    }

    /// Counts this file opened, read whole (`n` bytes in `ms`) by a reader of its own that counted
    /// its bytes as it read them (crate::blobs' copy, `copy_counter`).
    pub fn count_opened(&self, n: u64, ms: u64) {
        nasio::opened(&self.full(), n, ms);
    }

    /// Counts a file of `n` bytes made here in `ms` by a writer of its own that counted its bytes as
    /// it wrote them (crate::naming's content-named upload, `nasio::written`): the file, as it lands.
    pub fn count_made(&self, n: u64, ms: u64) {
        nasio::made(&self.full(), n, ms);
    }

    // ---- writes: counted, never retried; each holds the paths it changes while it's held ----

    /// Writes `b` as it (`std::fs::write`: in place, no temporary name). Counted as a file made.
    pub fn write(&self, b: impl AsRef<[u8]>) -> io::Result<()> {
        let _w = crate::waits::nas();
        let b = b.as_ref().to_vec();
        let (n, t) = (b.len() as u64, Instant::now());
        self.change("write", &[], move |q| std::fs::write(q.full(), &b))?;
        nasio::wrote(&self.full(), n, ms_since(t));
        Ok(())
    }

    /// Writes `b` as it, whole (crate::whole::write: a temporary name renamed over it, the old one
    /// renamed aside when that stays busy). Counted as a file made.
    pub fn write_whole(&self, b: impl AsRef<[u8]>) -> Result<()> {
        let _w = crate::waits::nas();
        let b = b.as_ref().to_vec();
        let (n, t) = (b.len() as u64, Instant::now());
        self.change("write", &[], move |q| Ok(crate::whole::write(&q.full(), &b)))??;
        nasio::wrote(&self.full(), n, ms_since(t));
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
    /// Past its deadline, an error: whether it made the file is unknown, as with a lost answer.
    pub fn create_new(&self, b: &[u8]) -> Result<Created> {
        let _w = crate::waits::nas();
        let b = b.to_vec();
        self.change("create", &[], move |q| Ok(q.create_new_now(&b)))?
    }

    fn create_new_now(&self, b: &[u8]) -> Result<Created> {
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

    /// The file made for writing (`std::fs::File::create`): the bytes written through it counted as
    /// they go, the file counted made when it is dropped.
    pub fn create(&self) -> io::Result<NasFile> {
        self.open_with(std::fs::OpenOptions::new().write(true).create(true).truncate(true))
    }

    /// The file opened with `o` (append, create-new, write in place). A file it may make: the bytes
    /// written through it counted as they go, the file counted made when it is dropped; one opened
    /// only for reading as a file read. Never retried. Its open a change of the share (a create, a
    /// truncate), its writes each another.
    pub fn open_with(&self, o: &std::fs::OpenOptions) -> io::Result<NasFile> {
        let _w = crate::waits::nas();
        let p = self.full();
        let t = Instant::now();
        let o = o.clone();
        let (f, writable, len) = self.change("open", &[], move |q| {
            let f = o.open(q.full())?;
            // (OpenOptions can't be asked what it was set to: a file opened writable is one written.)
            #[cfg(unix)]
            let writable = {
                use std::os::fd::AsRawFd;
                // SAFETY: F_GETFL on a descriptor this function owns.
                let fl = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETFL) };
                fl != -1 && fl & libc::O_ACCMODE != libc::O_RDONLY
            };
            #[cfg(not(unix))]
            let writable = true;
            let len = if writable { 0 } else { f.metadata().map_or(0, |m| m.len()) };
            Ok((f, writable, len))
        })?;
        if writable {
            Ok(NasFile::new(f, p, Some(0), t, self.root.deadline()))
        } else {
            nasio::opened(&p, len, ms_since(t));
            Ok(NasFile::new(f, p, None, t, self.root.deadline()))
        }
    }

    /// Copies local file `src` to it, whole (crate::whole::copy_told). Its bytes counted as they're
    /// written (`nasio::written`: a job's beats see a long upload move, task #990), the file counted
    /// made once copied. Moving as it copies.
    pub fn copy_from_local(&self, src: &Path) -> Result<u64> {
        let _w = crate::waits::nas();
        let t = Instant::now();
        let s = src.to_path_buf();
        let n = self.moving("copy", vec![self.full()], move |p, m| {
            Ok(crate::whole::copy_told(&s, p, &mut |k| {
                nasio::written(k);
                m.add(k);
            }))
        })??;
        nasio::made(&self.full(), n, ms_since(t));
        Ok(n)
    }

    /// Copies it to `to` on the NAS (crate::sys::copy_data). Its bytes counted as they're read
    /// (`copy_counter`) and written (`nasio::written`), the file counted opened and made once copied.
    /// Moving as it copies.
    pub fn copy_data_to(&self, to: &NasPath) -> io::Result<u64> {
        let _w = crate::waits::nas();
        let q = to.full();
        let t = Instant::now();
        let (n, _) = self.moving("copy", vec![q.clone()], move |p, m| {
            let mut read = copy_counter();
            crate::sys::copy_data_held(p, &q, &mut |k, h| {
                read(k, h);
                nasio::written(k);
                m.add(k);
            })
        })?;
        nasio::opened(&self.full(), n, ms_since(t));
        nasio::made(&to.full(), n, ms_since(t));
        Ok(n)
    }

    /// Renames it to `to` (`std::fs::rename`).
    pub fn rename(&self, to: &NasPath) -> io::Result<()> {
        let _w = crate::waits::nas();
        let q = to.full();
        self.change("rename", &[to], move |p| std::fs::rename(p.full(), &q))
    }

    /// Renames it over `to`, busy waited out (crate::whole::rename_over).
    pub fn rename_over(&self, to: &NasPath) -> io::Result<()> {
        let _w = crate::waits::nas();
        let q = to.full();
        self.change("rename", &[to], move |p| crate::whole::rename_over(&p.full(), &q))
    }

    /// Renames it to `to` unless `to` exists (then `AlreadyExists`): crate::naming::rename_no_replace.
    pub fn rename_no_replace(&self, to: &NasPath) -> io::Result<()> {
        let _w = crate::waits::nas();
        let q = to.full();
        self.change("rename", &[to], move |p| crate::naming::rename_no_replace(&p.full(), &q))
    }

    /// Removes the file (`std::fs::remove_file`). One `Deletes`.
    pub fn remove_file(&self) -> io::Result<()> {
        let _w = crate::waits::nas();
        self.change("remove", &[], |p| std::fs::remove_file(p.full()))?;
        nasio::add(Count::Deletes, 1);
        Ok(())
    }

    /// Removes the empty folder (`std::fs::remove_dir`). One `Deletes`.
    pub fn remove_dir(&self) -> io::Result<()> {
        let _w = crate::waits::nas();
        self.change("remove", &[], |p| std::fs::remove_dir(p.full()))?;
        nasio::add(Count::Deletes, 1);
        Ok(())
    }

    /// Removes the folder and all in it (`std::fs::remove_dir_all`'s walk: a link removed, never
    /// followed), moving as each name goes. One `Deletes`.
    pub fn remove_dir_all(&self) -> io::Result<()> {
        let _w = crate::waits::nas();
        self.moving("remove", vec![self.full()], |p, m| remove_all(p, m))?;
        nasio::add(Count::Deletes, 1);
        Ok(())
    }

    /// Makes the folder and those above it (`std::fs::create_dir_all`).
    pub fn create_dir_all(&self) -> io::Result<()> {
        let _w = crate::waits::nas();
        self.change("make", &[], |p| std::fs::create_dir_all(p.full()))
    }

    /// Its folder made, once a process (the `made` set). On SMB `create_dir_all` may fail with
    /// EEXIST when its follow-up stat of a folder another Mac just made is stale: taken as made
    /// (the write that follows fails if the folder truly is missing), but not remembered.
    fn make_parent(&self) -> Result<()> {
        let _w = crate::waits::nas();
        let Some(d) = self.parent() else { return Ok(()) };
        let dir = d.full();
        if self.root.0.made.lock().unwrap().contains(&dir) {
            return Ok(());
        }
        match d.create_dir_all() {
            Ok(()) => {
                self.root.0.made.lock().unwrap().insert(dir);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e).with_context(|| format!("make {}", dir.display())),
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
        self.change("touch", &[], move |p| std::fs::File::options().write(true).open(p.full())?.set_modified(t))
    }
}

/// The file at `p` read whole, moving as its bytes come (`read_all_told`).
fn read_told(p: &Path, m: &Progress) -> io::Result<Vec<u8>> {
    read_all_told(std::fs::File::open(p)?, m)
}

/// `f` read to its end in pieces of crate::sys::COPY_BUF at most, each told to `m` as it comes.
fn read_all_told(mut f: std::fs::File, m: &Progress) -> io::Result<Vec<u8>> {
    let len = f.metadata().map_or(0, |m| m.len()) as usize;
    let mut b = Vec::with_capacity(len.saturating_add(1));
    loop {
        let k = (&mut f).take(crate::sys::COPY_BUF as u64).read_to_end(&mut b)?;
        if k == 0 {
            return Ok(b);
        }
        m.add(k as u64);
    }
}

/// Folder `p`'s entries (names and types as the listing gave them), each told to `m` as it comes.
fn listed(p: &Path, m: &Progress) -> io::Result<Vec<(String, Option<std::fs::FileType>)>> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(p)? {
        let e = e?;
        v.push((e.file_name().to_string_lossy().into_owned(), e.file_type().ok()));
        m.add(1);
    }
    Ok(v)
}

/// Folder `p` and everything in it removed (a link removed as a file, never followed), each name
/// told to `m` as it goes.
fn remove_all(p: &Path, m: &Progress) -> io::Result<()> {
    if !std::fs::symlink_metadata(p)?.is_dir() {
        return std::fs::remove_file(p);
    }
    for e in std::fs::read_dir(p)? {
        let e = e?;
        let q = e.path();
        if e.file_type()?.is_dir() {
            remove_all(&q, m)?;
        } else {
            std::fs::remove_file(&q)?;
        }
        m.add(1);
    }
    std::fs::remove_dir(p)
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

    /// Opened for reading, as a `File` (a NAS file's reads not counted, nor waited on, nor tried
    /// again past its open: for a library that wants a `File`, read through at once; a file kept
    /// open and read by range is `open_plain`'s, task #1090).
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
/// file: buildstore::localread). Readers take it as a `Loc` (`&place`).
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

/// Counts a NAS file's copy's bytes as they come (crate::sys::copy_data_held's `each`: a read of
/// `k` bytes of a file of which this Mac's page cache held `held` as the copy began, asked of the
/// copy's own descriptor; a file of no more than one copy read isn't asked): the first `held`
/// read here, not from the NAS (nasio's `Local`, so not in a job's NAS weight, task #939: a
/// slope-deep job copied its z6 tile's 200 MB terrain pack once a target, every copy after the
/// first answered by the cache in a second or less, and each target weighed past the whole NAS),
/// the rest the NAS's, each as it's read: a job's beats see a long copy move (task #986: its
/// bytes at its end alone left a job copying a big file from a slow NAS still all the while).
/// Reads into memory (`read` and the like) aren't asked: most are of small files, where asking
/// would be a mapping, a round trip of its own. The file is counted opened (`nasio::opened`) once
/// copied.
pub(crate) fn copy_counter() -> impl FnMut(u64, u64) {
    let mut done = 0u64;
    move |k, held| {
        let here = held.saturating_sub(done).min(k);
        nasio::read(k - here);
        nasio::add(nasio::Count::Local, here);
        done += k;
    }
}

/// An open NAS file: reads and writes through it are counted as they go; one written is counted as
/// made when it is dropped. Each read, write, seek to its end and its close is a crate::deadline
/// call within its root's deadline: its reads fill a buffer of crate::sys::COPY_BUF at a time (one
/// call a MB, not one a reader's few kB), its writes hold its path (a later change to it refused
/// while one is held). Derefs to the `File` for its metadata and `set_len`, made inline.
pub struct NasFile {
    /// (None only once `into_file` took it, or its close began.)
    f: Option<Arc<std::fs::File>>,
    path: PathBuf,
    /// Some(bytes written so far) when opened writable.
    wrote: Option<u64>,
    t: Instant,
    deadline: Duration,
    /// Bytes read ahead and not yet given (`buf[at..]`): the file's own position is past them.
    buf: Vec<u8>,
    at: usize,
}

impl NasFile {
    fn new(f: std::fs::File, path: PathBuf, wrote: Option<u64>, t: Instant, deadline: Duration) -> NasFile {
        NasFile { f: Some(Arc::new(f)), path, wrote, t, deadline, buf: Vec::new(), at: 0 }
    }

    fn file(&self) -> &Arc<std::fs::File> {
        self.f.as_ref().expect("a NasFile's file")
    }

    /// `f` on its file, a call within its deadline (`op` naming it); one that changes it holds its
    /// path while it's held.
    fn on<T: Send + 'static>(
        &self,
        op: &'static str,
        changes: bool,
        f: impl FnOnce(&std::fs::File) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let file = self.file().clone();
        let keys = if changes { vec![self.path.clone()] } else { Vec::new() };
        deadline::writing(&self.path.display(), &op, self.deadline, keys, move || f(&file))
    }

    /// Bytes read ahead and not given back to the file's position (a write or a seek past them is
    /// made where the reader is).
    fn unread(&mut self) -> io::Result<()> {
        let ahead = (self.buf.len() - self.at) as i64;
        self.buf.clear();
        self.at = 0;
        if ahead > 0 {
            // (A move of the file's own position: no call on the share.)
            (&**self.file()).seek(io::SeekFrom::Current(-ahead))?;
        }
        Ok(())
    }

    /// The `File` itself, at the reader's position (its reads no longer counted, nor under a
    /// deadline: a library's own; a written one counted as made now).
    pub fn into_file(mut self) -> std::fs::File {
        if let Err(e) = self.unread() {
            crate::retry::log(format_args!("{}: its reader's position: {e}", self.path.display()));
        }
        let f = self.f.take().expect("a NasFile's file");
        // (A call left to the kernel may hold it still: a second descriptor of the same open file.)
        Arc::try_unwrap(f).unwrap_or_else(|f| f.try_clone().expect("a NasFile's descriptor"))
    }
}

impl std::ops::Deref for NasFile {
    type Target = std::fs::File;
    fn deref(&self) -> &std::fs::File {
        self.file()
    }
}

impl Read for NasFile {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if self.at == self.buf.len() {
            let _w = crate::waits::nas();
            let want = b.len().max(crate::sys::COPY_BUF);
            let got = self.on("read", false, move |mut f| {
                let mut v = Vec::with_capacity(want);
                (&mut f).take(want as u64).read_to_end(&mut v)?;
                Ok(v)
            })?;
            nasio::read(got.len() as u64);
            (self.buf, self.at) = (got, 0);
        }
        let n = b.len().min(self.buf.len() - self.at);
        b[..n].copy_from_slice(&self.buf[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

impl Write for NasFile {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.unread()?;
        let n = {
            let _w = crate::waits::nas();
            let owned = b.to_vec();
            self.on("write", true, move |mut f| f.write(&owned))?
        };
        if let Some(w) = &mut self.wrote {
            *w += n as u64;
            nasio::written(n as u64);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        // (A `File` keeps nothing back to flush.)
        Ok(())
    }
}

impl Seek for NasFile {
    fn seek(&mut self, to: io::SeekFrom) -> io::Result<u64> {
        // (Within what was read ahead: no move of the file's own.)
        if let io::SeekFrom::Current(d) = to {
            let at = self.at as i64 + d;
            if (0..=self.buf.len() as i64).contains(&at) {
                let ahead = (self.buf.len() - at as usize) as u64;
                self.at = at as usize;
                return Ok((&**self.file()).stream_position()? - ahead);
            }
        }
        self.unread()?;
        match to {
            // (Its end asks the share for its length: a call.)
            io::SeekFrom::End(_) => {
                let _w = crate::waits::nas();
                self.on("seek", false, move |mut f| f.seek(to))
            }
            _ => (&**self.file()).seek(to),
        }
    }
}

impl Drop for NasFile {
    fn drop(&mut self) {
        // (Its close: where SMB sends what's left of a write. Past its deadline it's left to the
        // kernel, as any call.)
        if let Some(f) = self.f.take() {
            let _w = crate::waits::nas();
            let keys = if self.wrote.is_some() { vec![self.path.clone()] } else { Vec::new() };
            if let Err(e) = deadline::writing(&self.path.display(), &"close", self.deadline, keys, move || {
                drop(f);
                Ok(())
            }) {
                crate::retry::log(format_args!("{}: close: {e}", self.path.display()));
            }
        }
        if let Some(n) = self.wrote {
            nasio::made(&self.path, n, ms_since(self.t));
        }
    }
}

#[cfg(test)]
mod tests;
