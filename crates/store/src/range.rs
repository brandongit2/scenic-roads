//! Range reads over local and NAS files. A local file is mmapped. A NAS file never is: when the
//! share drops, a page fault on a mapped file is a SIGBUS that kills the server. NAS files are read
//! with pread on an ordinary descriptor, through the I/O pool, so a stall becomes a timeout.

use crate::iopool::{IoError, IoPool};
use anyhow::{Context, Result};
use memmap2::Mmap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Counts `n` bytes read from this Mac's own disk (crate::nasio's `Local`: a mapped file's whole
/// length as it is mapped, a plain local file's reads, `cachefile::read`'s).
pub(crate) fn count_local(n: u64) {
    crate::nasio::add(crate::nasio::Count::Local, n);
}

/// Random access to an immutable file's bytes.
pub trait RangeRead: Send + Sync {
    /// The length in bytes: an error when it can't be learnt (a NAS file whose share is away).
    fn len(&self) -> Result<u64, IoError>;

    /// Exactly `len` bytes at `off`; an error if that runs past the end.
    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError>;
}

/// A range source whose bytes are all in memory already (an mmap), so they can be borrowed.
pub trait Mapped {
    fn bytes(&self) -> &[u8];
}

/// An error unless `off..off + len` lies within `file_len` bytes.
pub(crate) fn check_range(file_len: u64, off: u64, len: usize) -> Result<(), IoError> {
    if off.checked_add(len as u64).is_some_and(|end| end <= file_len) {
        Ok(())
    } else {
        Err(IoError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("bytes {off}..+{len} are past the end ({file_len} bytes)"),
        )))
    }
}

fn slice_at(b: &[u8], off: u64, len: usize) -> Result<Vec<u8>, IoError> {
    check_range(b.len() as u64, off, len)?;
    Ok(b[off as usize..off as usize + len].to_vec())
}

/// A local file, mmapped.
pub struct MmapFile {
    map: Mmap,
    path: PathBuf,
}

impl MmapFile {
    /// Maps a local file; refuses a file on a network filesystem (see the module docs).
    pub fn open(path: &Path) -> Result<Self> {
        let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        ensure_local(&f).with_context(|| format!("map {}", path.display()))?;
        // SAFETY: the files mapped here are content-named and never modified once written, and
        // the mirror evicts by unlinking (never truncating), so the mapping stays valid.
        let map = unsafe { Mmap::map(&f) }.with_context(|| format!("map {}", path.display()))?;
        count_local(map.len() as u64);
        Ok(Self { map, path: path.to_owned() })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.map
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for MmapFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MmapFile({}, {} bytes)", self.path.display(), self.map.len())
    }
}

/// An error when `f` lives on a filesystem the kernel doesn't call local (an SMB share).
fn ensure_local(f: &File) -> Result<()> {
    if let Some(fstype) = network_fs(f)? {
        anyhow::bail!("the file is on a network filesystem ({fstype}); NAS files are never mmapped");
    }
    Ok(())
}

/// The network filesystem `f` lives on (an SMB share: "smbfs"); None when the kernel calls it local.
fn network_fs(f: &File) -> io::Result<Option<String>> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `statfs` is plain old data, filled in by the call.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: a valid descriptor and a valid out-pointer.
        if unsafe { libc::fstatfs(f.as_raw_fd(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if st.f_flags & libc::MNT_LOCAL as u32 == 0 {
            return Ok(Some(st.f_fstypename.iter().take_while(|&&c| c != 0).map(|&c| c as u8 as char).collect()));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = f;
    Ok(None)
}

impl RangeRead for MmapFile {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.map.len() as u64)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        slice_at(&self.map, off, len)
    }
}

impl Mapped for MmapFile {
    fn bytes(&self) -> &[u8] {
        &self.map
    }
}

/// A NAS file, read with pread through the I/O pool (never mmapped). Its handle and length
/// (content-named files never change) are taken at its first read and kept; a read that fails with
/// an I/O error (a handle gone stale after sleep or a reconnect) opens it again and tries once more,
/// and a handle whose read hung or was refused isn't used again. Reads go in pieces of at most
/// `PIECE`, each its own pool operation, so a big read on a busy NAS doesn't overrun the pool's
/// timeout (which would mark the NAS offline).
pub struct PooledFile {
    path: crate::nasfs::NasPath,
    pool: Arc<IoPool>,
    /// Unique per instance: a key for what's kept of its bytes.
    id: u64,
    file: std::sync::Mutex<Option<(Arc<File>, u64)>>,
    /// (For its errors and Debug.)
    shown: PathBuf,
}

/// The most a PooledFile reads in one pool operation.
pub const PIECE: usize = 1 << 20;

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl PooledFile {
    /// `path`, opened at its first read.
    pub fn new(path: crate::nasfs::NasPath, pool: Arc<IoPool>) -> Self {
        let shown = path.unchecked_path();
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { path, pool, id, file: std::sync::Mutex::new(None), shown }
    }

    /// Opens `path` through the pool now (open and fstat in one trip; counted there).
    pub fn open(pool: &Arc<IoPool>, path: &crate::nasfs::NasPath) -> Result<Self, IoError> {
        let f = Self::new(path.clone(), pool.clone());
        f.handle(false)?;
        Ok(f)
    }

    pub fn path(&self) -> &Path {
        &self.shown
    }

    /// Unique to this instance (for caches of its bytes).
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn pool(&self) -> &Arc<IoPool> {
        &self.pool
    }

    fn handle(&self, fresh: bool) -> Result<(Arc<File>, u64), IoError> {
        let mut g = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if fresh {
            *g = None;
        }
        if let Some(h) = g.as_ref() {
            return Ok(h.clone());
        }
        let h = self.pool.open_len(&self.path)?;
        *g = Some(h.clone());
        Ok(h)
    }

    fn read_piece(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        let (f, flen) = self.handle(false)?;
        check_range(flen, off, len)?;
        let b = match self.pool.read_at(&f, off, len) {
            Err(IoError::Io(_)) => {
                let (f, _) = self.handle(true)?;
                self.pool.read_at(&f, off, len)
            }
            Err(e) => {
                // A handle that hung (sleep, a reconnect) isn't used again: the next read reopens.
                *self.file.lock().unwrap_or_else(|e| e.into_inner()) = None;
                Err(e)
            }
            r => r,
        }?;
        crate::nasio::read(len as u64);
        Ok(b)
    }

    /// Fills `buf` from `off`, piece by piece (no second copy of the whole).
    pub fn read_into(&self, off: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let mut at = 0usize;
        while at < buf.len() {
            let n = (buf.len() - at).min(PIECE);
            buf[at..at + n].copy_from_slice(&self.read_piece(off + at as u64, n)?);
            at += n;
        }
        Ok(())
    }

    /// The whole file.
    pub fn read_all(&self) -> Result<Vec<u8>, IoError> {
        let n = self.len()? as usize;
        self.read_at(0, n)
    }
}

impl std::fmt::Debug for PooledFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PooledFile({})", self.shown.display())
    }
}

impl RangeRead for PooledFile {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.handle(false)?.1)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        if len <= PIECE {
            return self.read_piece(off, len);
        }
        let mut v = vec![0u8; len];
        self.read_into(off, &mut v)?;
        Ok(v)
    }
}

/// A file read with plain positioned reads (no pool, never mapped): for build steps, which may read
/// the NAS directly (they run alone, and a stalled share just stalls the step).
/// Its reads are counted as this Mac's own disk's (`LOCAL`) when it's on one. A read the share fails
/// for a moment (crate::retry::transient) is tried again, the file opened again by its path (a
/// dropped SMB session's handle stays dead), within crate::retry's bound.
/// Its descriptor is held only while it's among the `plain_open_cap` files read last: past that the
/// one read longest ago is closed, and opened again by its path at its next read (its length
/// unchanged: else `InvalidData`). A run that keeps thousands open (a pool's GLO-30 tiles and their
/// masks over a wide area) holds a bounded number of descriptors.
pub struct PlainFile {
    slot: Arc<Slot>,
    path: std::path::PathBuf,
    len: u64,
    local: bool,
    /// (Tests: this many reads fail with this OS error first.)
    #[cfg(test)]
    faults: std::sync::Mutex<(u32, i32)>,
}

/// A PlainFile's descriptor (None: closed, `plain_open_cap`'s) and when it was read last.
struct Slot {
    file: std::sync::Mutex<Option<Arc<File>>>,
    used: std::sync::atomic::AtomicU64,
}

/// The PlainFiles with a descriptor open, the order of their reads by `CLOCK`.
static OPEN: std::sync::Mutex<Vec<std::sync::Weak<Slot>>> = std::sync::Mutex::new(Vec::new());
static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// (Tests: a cap of their own; 0: `plain_open_cap`'s derived one.)
static CAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The PlainFiles' descriptors held open at most (beyond those mid-read): a quarter of this
/// process's open-file limit (crate::cachefile holds up to another quarter), at least 16.
pub fn plain_open_cap() -> usize {
    match CAP.load(std::sync::atomic::Ordering::Relaxed) {
        0 => (crate::cachefile::open_files_limit() / 4).max(16) as usize,
        n => n,
    }
}

/// Test builds: `plain_open_cap` set to `n` until the guard is dropped (a panic included), for a
/// test that opens more files than it. Process-wide: one test at a time holds it (the others
/// wait); in one `cargo test` process, tests that don't take it run under it meanwhile (a lower cap
/// only closes idle files sooner).
#[cfg(any(test, feature = "testkit"))]
pub fn cap_plain_opens(n: usize) -> PlainOpenCap {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let turn = lock(&TURN);
    CAP.store(n, std::sync::atomic::Ordering::Relaxed);
    PlainOpenCap { _turn: turn }
}

#[cfg(any(test, feature = "testkit"))]
pub struct PlainOpenCap {
    _turn: std::sync::MutexGuard<'static, ()>,
}

#[cfg(any(test, feature = "testkit"))]
impl Drop for PlainOpenCap {
    fn drop(&mut self) {
        CAP.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The PlainFiles' descriptors open now (those closed but mid-read aside).
pub fn plain_open_count() -> usize {
    let open = OPEN.lock().unwrap_or_else(|p| p.into_inner());
    open.iter().filter(|w| w.strong_count() > 0).count()
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// `s` among the open, the ones read longest ago closed past the cap. (No slot's lock is held
/// while `OPEN`'s is taken but here, under it.)
fn register(s: &Arc<Slot>) {
    let mut open = lock(&OPEN);
    open.push(Arc::downgrade(s));
    let cap = plain_open_cap();
    if open.len() <= cap {
        return;
    }
    open.retain(|w| w.strong_count() > 0);
    while open.len() > cap {
        // (The one read longest ago, never `s`.)
        let Some((i, _)) = open
            .iter()
            .enumerate()
            .filter_map(|(i, w)| w.upgrade().map(|o| (i, o)))
            .filter(|(_, o)| !Arc::ptr_eq(o, s))
            .min_by_key(|(_, o)| o.used.load(std::sync::atomic::Ordering::Relaxed))
        else {
            break;
        };
        if let Some(o) = open.swap_remove(i).upgrade() {
            // (A read under way keeps its own Arc: the descriptor closes when it's done.)
            *lock(&o.file) = None;
        }
    }
}

fn tick() -> u64 {
    CLOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl PlainFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let t = std::time::Instant::now();
        // (Taken as the NAS's: which it is isn't known till it's open.)
        let w = crate::waits::nas();
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        let local = network_fs(&file)?.is_none();
        drop(w);
        if !local {
            crate::nasio::opened(path, len, t.elapsed().as_millis() as u64);
        }
        let slot = Arc::new(Slot {
            file: std::sync::Mutex::new(Some(Arc::new(file))),
            used: std::sync::atomic::AtomicU64::new(tick()),
        });
        register(&slot);
        Ok(Self {
            slot,
            path: path.to_path_buf(),
            len,
            local,
            #[cfg(test)]
            faults: std::sync::Mutex::new((0, 0)),
        })
    }

    /// One positioned read of `f`.
    fn read_once(&self, f: &File, b: &mut [u8], off: u64) -> std::io::Result<()> {
        use crate::sys::PosIo;
        #[cfg(test)]
        {
            let mut g = self.faults.lock().unwrap();
            if g.0 > 0 {
                g.0 -= 1;
                return Err(std::io::Error::from_raw_os_error(g.1));
            }
        }
        f.read_exact_at(b, off)
    }

    /// Its descriptor, opened again by its path when it was closed (`plain_open_cap`).
    fn handle(&self) -> std::io::Result<Arc<File>> {
        self.slot.used.store(tick(), std::sync::atomic::Ordering::Relaxed);
        if let Some(f) = lock(&self.slot.file).as_ref() {
            return Ok(f.clone());
        }
        self.reopen()
    }

    /// The file opened again by its path (its length unchanged: else `InvalidData`, never tried
    /// again), for the reads after it; the descriptor it replaces closes once no read holds it.
    fn reopen(&self) -> std::io::Result<Arc<File>> {
        let f = File::open(&self.path)?;
        let len = f.metadata()?.len();
        if len != self.len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is {len} bytes now, {} when opened", self.path.display(), self.len),
            ));
        }
        let f = Arc::new(f);
        let was = lock(&self.slot.file).replace(f.clone());
        if was.is_none() {
            register(&self.slot);
        }
        Ok(f)
    }
}

impl RangeRead for PlainFile {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.len)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        check_range(self.len, off, len)?;
        let _w = if self.local { crate::waits::disk() } else { crate::waits::nas() };
        let mut b = vec![0u8; len];
        if let Err(e) = self.handle().and_then(|f| self.read_once(&f, &mut b, off)) {
            if !crate::retry::transient(&e) {
                return Err(IoError::Io(e));
            }
            let what = format!("{}: read {len} bytes at {off}", self.path.display());
            eprintln!("{what}: {e}");
            // (Opened again each try: a "not found" then is the share's moment, the file was open.)
            crate::retry::io(
                &what,
                Some(&self.path),
                &crate::retry::waits(),
                crate::retry::transient_or_missing,
                || {
                    let f = self.reopen()?;
                    self.read_once(&f, &mut b, off)
                },
            )
            .map_err(IoError::Io)?;
        }
        if self.local {
            count_local(len as u64);
        } else {
            crate::nasio::read(len as u64);
        }
        Ok(b)
    }
}

/// Bytes in memory (tests, small files read whole).
impl RangeRead for Vec<u8> {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.as_slice().len() as u64)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        slice_at(self, off, len)
    }
}

impl Mapped for Vec<u8> {
    fn bytes(&self) -> &[u8] {
        self
    }
}

impl<T: RangeRead + ?Sized> RangeRead for &T {
    fn len(&self) -> Result<u64, IoError> {
        (**self).len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        (**self).read_at(off, len)
    }
}

impl<T: RangeRead + ?Sized> RangeRead for Arc<T> {
    fn len(&self) -> Result<u64, IoError> {
        (**self).len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        (**self).read_at(off, len)
    }
}

impl<T: RangeRead + ?Sized> RangeRead for Box<T> {
    fn len(&self) -> Result<u64, IoError> {
        (**self).len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        (**self).read_at(off, len)
    }
}

/// The bytes `off..off + len` of another source, as a source of their own (a member stored in a
/// zip, read in place).
pub struct Slice<R> {
    inner: R,
    off: u64,
    len: u64,
}

impl<R: RangeRead> Slice<R> {
    /// An error unless the window lies within `inner`.
    pub fn new(inner: R, off: u64, len: u64) -> Result<Self, IoError> {
        check_range(inner.len()?, off, usize::try_from(len).unwrap_or(usize::MAX))?;
        Ok(Self { inner, off, len })
    }
}

impl<R: RangeRead> RangeRead for Slice<R> {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.len)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        check_range(self.len, off, len)?;
        self.inner.read_at(self.off + off, len)
    }
}

impl<T: Mapped + ?Sized> Mapped for &T {
    fn bytes(&self) -> &[u8] {
        (**self).bytes()
    }
}

impl<T: Mapped + ?Sized> Mapped for Arc<T> {
    fn bytes(&self) -> &[u8] {
        (**self).bytes()
    }
}

impl<T: Mapped + ?Sized> Mapped for Box<T> {
    fn bytes(&self) -> &[u8] {
        (**self).bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn check(r: &dyn RangeRead) {
        assert_eq!(r.len().unwrap(), 10);
        assert_eq!(r.read_at(0, 3).unwrap(), b"012");
        assert_eq!(r.read_at(7, 3).unwrap(), b"789");
        assert_eq!(r.read_at(10, 0).unwrap(), b"");
        assert!(matches!(r.read_at(8, 3), Err(IoError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof));
        assert!(r.read_at(u64::MAX, 2).is_err());
    }

    /// A plain file's read the share fails for a moment, with each errno, is tried again (the file
    /// opened again) and reads the same bytes; one failing past the bound fails with its error.
    #[test]
    fn plain_reads_ride_out_a_short_outage() {
        crate::retry::fast();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        let f = PlainFile::open(&p).unwrap();
        for code in [libc::EIO, libc::ENXIO, libc::ETIMEDOUT, libc::ENOTCONN, libc::EAGAIN, libc::ESTALE] {
            *f.faults.lock().unwrap() = (4, code);
            assert_eq!(f.read_at(2, 5).unwrap(), b"23456", "errno {code}");
        }
        *f.faults.lock().unwrap() = (u32::MAX, libc::ENXIO);
        let e = f.read_at(0, 3).unwrap_err().to_string();
        assert_eq!(e, io::Error::from_raw_os_error(libc::ENXIO).to_string());
        // Not the share's moment: at once.
        *f.faults.lock().unwrap() = (1, libc::EISDIR);
        assert!(f.read_at(0, 3).is_err());
        assert_eq!(f.read_at(0, 3).unwrap(), b"012");
        // The file changed under it: an error, not its new bytes.
        *f.faults.lock().unwrap() = (1, libc::EIO);
        std::fs::write(&p, b"01234").unwrap();
        assert!(f.read_at(0, 3).unwrap_err().to_string().contains("5 bytes now"));
    }

    #[test]
    fn sources() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        let m = MmapFile::open(&p).unwrap();
        check(&m);
        assert_eq!(m.bytes(), b"0123456789");
        check(&b"0123456789".to_vec());
        let pool = IoPool::new(1, Duration::from_secs(60), dir.path().to_owned());
        let np = crate::nasfs::NasRoot::local_for_test(dir.path()).join("f");
        let f = PooledFile::open(&pool, &np).unwrap();
        check(&f);
        let shared: Arc<dyn RangeRead> = Arc::new(f);
        check(&shared);
        // Offline: reads fail fast with the typed error.
        pool.mark_offline("test");
        assert!(matches!(shared.read_at(0, 1), Err(IoError::Offline)));
        assert!(matches!(PooledFile::open(&pool, &np), Err(IoError::Offline)));
    }

    #[test]
    fn slices() {
        let s = Slice::new(b"xx0123456789yy".to_vec(), 2, 10).unwrap();
        check(&s);
        assert!(Slice::new(b"0123".to_vec(), 2, 3).is_err(), "past the end");
    }

    #[test]
    fn empty_file_maps() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("empty");
        std::fs::write(&p, b"").unwrap();
        let m = MmapFile::open(&p).unwrap();
        assert_eq!(m.len().unwrap(), 0);
        assert_eq!(m.read_at(0, 0).unwrap(), b"");
        assert!(MmapFile::open(&dir.path().join("missing")).is_err());
    }
}
