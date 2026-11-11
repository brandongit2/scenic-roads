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
use std::time::Duration;

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

    /// Whether bytes `off..off + len` are in memory already, a read of them no I/O (nothing to read
    /// ahead: buildstore::geotiff::Tiff::fetch).
    fn in_memory(&self, _off: u64, _len: usize) -> bool {
        false
    }
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
pub(crate) fn network_fs(f: &File) -> io::Result<Option<String>> {
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
/// the NAS directly. A NAS read runs on its calling thread's helper (`bounded`), waited for
/// `read_deadline` at most (task #1139: SMB reads never answered held z6 tiles' staging in `pread`
/// for 40 minutes and more, their jobs at no CPU and nothing ending them): past it the read fails
/// (`TimedOut`, naming the file, not tried again), its descriptor is let go (the next read opens the
/// file again) and the helper is left to the kernel, holding the descriptor and the read's buffer
/// until the read returns, its result then dropped. Its opens (`open`, and again by its path) are
/// waited for `READ_DEADLINE` the same way.
/// Its reads are counted as this Mac's own disk's (`LOCAL`) when it's on one. A read that fails as
/// a gone handle does (crate::retry::handle_gone: a dropped SMB session's handle stays dead, every
/// read of it failing, task #1090) is tried again on the file opened again by its path, each try
/// opening it again, within crate::retry's bound (once, inside another retry's try: that retry
/// waits and tries again); the descriptor that read then is the one kept.
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
    /// (Tests: the next read's thread waits on this before it reads, as a read never answered; and
    /// a deadline of their own.)
    #[cfg(test)]
    stall: std::sync::Mutex<(Option<std::sync::mpsc::Receiver<()>>, Option<Duration>)>,
}

/// The longest a NAS read through a `PlainFile` is waited for: this, and a second more for each MB
/// it reads (`read_deadline`: a 32 MB span on a share giving it 1 MB/s still ends). The NAS
/// answers a read in milliseconds, a span of tens of MB in seconds; a read waits as long for its
/// turn (crate::aimd::TAKE_WAIT).
pub const READ_DEADLINE: Duration = Duration::from_secs(120);

/// A NAS read of `len` bytes' deadline (`READ_DEADLINE`).
pub fn read_deadline(len: usize) -> Duration {
    READ_DEADLINE + Duration::from_secs((len >> 20) as u64)
}

/// How often a bounded call's caller looks (`bounded`). A look that comes back over `LATE` times
/// later than asked counts as one look: the process was stopped (the agent's memory and disk guards
/// freeze a job, SIGSTOP), and that time isn't the call's. (The Mac asleep isn't counted either:
/// `Instant` is the uptime clock on macOS.) The rule only ever counts less than the time passed: a
/// late look never ends a call sooner.
const LOOK: Duration = Duration::from_secs(1);
const LATE: u32 = 4;

/// What a look that took `took`, asked to take `look`, counts toward a deadline (`LATE`).
fn counted(took: Duration, look: Duration) -> Duration {
    if took > look * LATE {
        look
    } else {
        took
    }
}

/// A call `bounded` runs on its caller's helper.
type Call = Box<dyn FnOnce() + Send>;

thread_local! {
    /// This thread's helper for `bounded` (its calls run there, one at a time): made at its first
    /// call, kept while its calls come back in time, given up with one that doesn't.
    static HELPER: std::cell::RefCell<Option<std::sync::mpsc::Sender<Call>>> =
        const { std::cell::RefCell::new(None) };
}

/// A new helper thread: it runs the calls sent it, and ends once its sender is dropped and the
/// call it's in (if any) has come back.
fn helper() -> io::Result<std::sync::mpsc::Sender<Call>> {
    let (tx, rx) = std::sync::mpsc::channel::<Call>();
    std::thread::Builder::new().name("nas call".into()).spawn(move || {
        for c in rx {
            c();
        }
    })?;
    Ok(tx)
}

/// `f` on this thread's helper (`HELPER`), its result if it's back within `deadline` of the time
/// this process ran (`LOOK`); else `TimedOut` with no OS code (`past_deadline`), the helper left to
/// its call: its result, when it comes, goes nowhere (its channel's receiver is gone), and the next
/// call here gets a new helper. (A helper a thread keeps, not a thread a call: a spawn a read cost
/// 3 to 6 times a pread from the cache with 8 to 16 threads reading at once, on the M4.)
fn bounded<T: Send + 'static>(
    deadline: Duration,
    look: Duration,
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    use std::sync::mpsc::RecvTimeoutError;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let mut call: Call = Box::new(move || {
        let _ = tx.send(f());
    });
    let h = loop {
        let h = match HELPER.with(|h| h.borrow_mut().take()) {
            Some(h) => h,
            None => helper()?,
        };
        match h.send(call) {
            Ok(()) => break h,
            // (Its helper ended, a call of its panicking: a new one.)
            Err(std::sync::mpsc::SendError(c)) => call = c,
        }
    };
    let mut waited = Duration::ZERO;
    let r = loop {
        let t = std::time::Instant::now();
        match rx.recv_timeout(look.min(deadline.saturating_sub(waited))) {
            Ok(r) => break r,
            Err(RecvTimeoutError::Disconnected) => return Err(io::Error::other("its call panicked")),
            Err(RecvTimeoutError::Timeout) => {}
        }
        waited += counted(t.elapsed(), look);
        if waited >= deadline {
            // (Answered as the deadline came: taken.)
            match rx.try_recv() {
                Ok(r) => break r,
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("no answer in {} s", deadline.as_secs_f64().round()),
                    ))
                }
            }
        }
    };
    HELPER.with(|c| *c.borrow_mut() = Some(h));
    r
}

/// Whether `e` is `bounded`'s deadline passing (not an OS error's `TimedOut`).
fn past_deadline(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::TimedOut && e.raw_os_error().is_none()
}

/// `e`, past its deadline (`past_deadline`), said with what it was (`what`, naming the file) and
/// logged; any other error as it is.
fn said_past_deadline(e: io::Error, what: impl FnOnce() -> String) -> io::Error {
    if !past_deadline(&e) {
        return e;
    }
    let said = format!("{}: {e}", what());
    crate::retry::log(format_args!("{said}"));
    io::Error::new(io::ErrorKind::TimedOut, said)
}

/// `path` opened, its length, and whether it's on this Mac's own disk, within `READ_DEADLINE`
/// (`bounded`: an open the share never answers would hold its job as a read would).
fn open_bounded(path: &Path) -> io::Result<(File, u64, bool)> {
    let p = path.to_path_buf();
    bounded(READ_DEADLINE, LOOK, move || {
        let file = File::open(&p)?;
        let len = file.metadata()?.len();
        let local = network_fs(&file)?.is_none();
        Ok((file, len, local))
    })
    .map_err(|e| said_past_deadline(e, || format!("{}: open", path.display())))
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
        let (file, len, local) = open_bounded(path)?;
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
            #[cfg(test)]
            stall: std::sync::Mutex::new((None, None)),
        })
    }

    /// One positioned read of `len` bytes at `off` of `f`: on the NAS, on a thread of its own within
    /// `read_deadline` (the struct's doc), its descriptor let go past it.
    fn read_once(&self, f: Arc<File>, off: u64, len: usize) -> io::Result<Vec<u8>> {
        use crate::sys::PosIo;
        #[cfg(test)]
        {
            let mut g = self.faults.lock().unwrap();
            if g.0 > 0 {
                g.0 -= 1;
                return Err(io::Error::from_raw_os_error(g.1));
            }
        }
        #[cfg(test)]
        let (stall, deadline) = {
            let mut g = self.stall.lock().unwrap();
            (g.0.take(), g.1.unwrap_or_else(|| read_deadline(len)))
        };
        #[cfg(not(test))]
        let deadline = read_deadline(len);
        let read = move |f: &File| {
            let mut b = vec![0u8; len];
            f.read_exact_at(&mut b, off).map(|_| b)
        };
        if self.local {
            return read(&f);
        }
        let held = f.clone();
        bounded(deadline, LOOK, move || {
            #[cfg(test)]
            if let Some(s) = stall {
                s.recv().ok();
            }
            read(&held)
        })
        .map_err(|e| {
            if past_deadline(&e) {
                self.let_go(&f);
            }
            said_past_deadline(e, || format!("{}: read {len} bytes at {off}; its handle let go", self.path.display()))
        })
    }

    /// Descriptor `f` closed for its reads to come, if it's still the one held (a read that never
    /// came back: the next read opens the file again). Its stuck read keeps its own until it ends.
    fn let_go(&self, f: &Arc<File>) {
        let mut open = lock(&OPEN);
        let mut g = lock(&self.slot.file);
        if g.as_ref().is_some_and(|h| Arc::ptr_eq(h, f)) {
            *g = None;
            open.retain(|w| w.upgrade().is_some_and(|o| !Arc::ptr_eq(&o, &self.slot)));
        }
    }

    /// Test builds: its descriptor made dead in place (a write-only one put in its stead), so its
    /// next read fails with EBADF, as a handle a dropped SMB session left does.
    #[cfg(any(test, feature = "testkit"))]
    pub fn kill_handle(&self) {
        use std::os::fd::AsRawFd;
        let g = lock(&self.slot.file);
        let Some(f) = g.as_ref() else { return };
        let w = std::fs::OpenOptions::new().write(true).open("/dev/null").expect("/dev/null");
        // SAFETY: both descriptors are open; dup2 puts w's file in f's descriptor in place.
        assert!(unsafe { libc::dup2(w.as_raw_fd(), f.as_raw_fd()) } >= 0, "dup2");
    }

    /// Its descriptor, opened again by its path when it was closed (`plain_open_cap`).
    fn handle(&self) -> std::io::Result<Arc<File>> {
        self.slot.used.store(tick(), std::sync::atomic::Ordering::Relaxed);
        if let Some(f) = lock(&self.slot.file).as_ref() {
            return Ok(f.clone());
        }
        self.reopen()
    }

    /// The file opened again by its path (`open_bounded`; its length unchanged: else `InvalidData`,
    /// never tried again), for the reads after it; the descriptor it replaces closes once no read
    /// holds it.
    fn reopen(&self) -> std::io::Result<Arc<File>> {
        let (f, len, _) = open_bounded(&self.path)?;
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
        // (A NAS read takes its turn of the process's NAS limit, crate::aimd: its latency or its
        // failure tells the limit.)
        let mut turn = (!self.local).then(|| crate::aimd::NAS.take());
        // (A read or open never answered (`bounded`'s `TimedOut`) is no gone handle: not tried
        // again.)
        let r = match self.handle().and_then(|f| self.read_once(f, off, len)) {
            Err(e) if crate::retry::handle_gone(&e) => {
                let what = format!("{}: read {len} bytes at {off}", self.path.display());
                crate::retry::log(format_args!("{what}: {e}; opened again"));
                // (Opened again each try: a "not found" then is the share's moment, the file was open.)
                crate::retry::io(
                    &what,
                    Some(&self.path),
                    &crate::retry::waits(),
                    crate::retry::transient_or_missing,
                    || self.read_once(self.reopen()?, off, len),
                )
            }
            r => r,
        };
        let b = r.map_err(|e| {
            if crate::aimd::nas_congested(&e) {
                turn.as_mut().map(crate::aimd::Slot::congested);
            }
            IoError::Io(e)
        })?;
        if let Some(t) = turn.as_mut() {
            t.read(len as u64);
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

    fn in_memory(&self, off: u64, len: usize) -> bool {
        (**self).in_memory(off, len)
    }
}

impl<T: RangeRead + ?Sized> RangeRead for Arc<T> {
    fn len(&self) -> Result<u64, IoError> {
        (**self).len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        (**self).read_at(off, len)
    }

    fn in_memory(&self, off: u64, len: usize) -> bool {
        (**self).in_memory(off, len)
    }
}

impl<T: RangeRead + ?Sized> RangeRead for Box<T> {
    fn len(&self) -> Result<u64, IoError> {
        (**self).len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        (**self).read_at(off, len)
    }

    fn in_memory(&self, off: u64, len: usize) -> bool {
        (**self).in_memory(off, len)
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

    /// Task #1090: a handle that's dead (every read of it failing) is let go: the read that meets
    /// it opens the file again and reads, and the reads after use the new one. A file that can't
    /// be opened again still fails, past the bound, not for ever.
    #[test]
    fn a_dead_handle_is_opened_again() {
        crate::retry::fast();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        let f = PlainFile::open(&p).unwrap();
        f.kill_handle();
        assert_eq!(f.read_at(2, 5).unwrap(), b"23456");
        assert_eq!(f.read_at(0, 3).unwrap(), b"012", "the new handle kept");
        // Inside another retry's try: opened again once, at once.
        f.kill_handle();
        {
            let _a = crate::retry::attempt();
            assert_eq!(f.read_at(7, 3).unwrap(), b"789");
        }
        // Gone for good: its error, past the bound, marked so an outer retry doesn't start over.
        f.kill_handle();
        std::fs::remove_file(&p).unwrap();
        let e = match f.read_at(0, 3) {
            Err(IoError::Io(e)) => e,
            r => panic!("{r:?}"),
        };
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(!crate::retry::transient_or_missing(&e));
    }

    /// Task #1139: a NAS read that's never answered (a z6 tile's staging sat in `pread` for 40
    /// minutes and more, the rayon worker running its span stuck and the job's main thread waiting
    /// on it) fails within its deadline, naming the file, and isn't tried again; its descriptor is
    /// let go, and the next read opens the file again and reads. The I/O limit is halved.
    #[test]
    fn a_nas_read_never_answered_fails_at_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        let mut f = PlainFile::open(&p).unwrap();
        f.local = false;
        assert_eq!(f.read_at(2, 5).unwrap(), b"23456", "a NAS read answered: as before");
        let (release, stall) = std::sync::mpsc::channel::<()>();
        *f.stall.lock().unwrap() = (Some(stall), Some(Duration::from_millis(300)));
        let held = lock(&f.slot.file).clone().unwrap();
        let t = std::time::Instant::now();
        let e = match f.read_at(0, 3) {
            Err(IoError::Io(e)) => e,
            r => panic!("{r:?}"),
        };
        // (Never answered, it would not end at all.) (bound ok: its own 300 ms deadline, not 120 s)
        assert!(t.elapsed() < READ_DEADLINE / 2, "{:?}", t.elapsed());
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        let said = e.to_string();
        assert!(said.contains(&p.display().to_string()) && said.contains("no answer"), "{said}");
        assert!(!crate::retry::handle_gone(&e) && crate::aimd::nas_congested(&e));
        assert!(lock(&f.slot.file).is_none(), "its descriptor let go");
        assert!(Arc::strong_count(&held) >= 2, "the stuck read keeps its own");
        // The next read: the file opened again, and read.
        f.stall.lock().unwrap().1 = None;
        assert_eq!(f.read_at(7, 3).unwrap(), b"789");
        assert!(lock(&f.slot.file).as_ref().is_some_and(|h| !Arc::ptr_eq(h, &held)));
        let open = lock(&OPEN);
        assert_eq!(open.iter().filter(|w| w.upgrade().is_some_and(|o| Arc::ptr_eq(&o, &f.slot))).count(), 1);
        drop(open);
        drop(release);
    }

    /// A bounded call: its answer when it comes in time; past its deadline `TimedOut`; and a look
    /// that came back far later than asked (the process stopped, the Mac asleep) counts as one.
    #[test]
    fn a_bounded_call_counts_the_time_the_process_ran() {
        let ms = Duration::from_millis;
        assert_eq!(bounded(ms(500), ms(10), || Ok(7)).unwrap(), 7);
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let e = bounded(ms(50), ms(10), move || rx.recv().map_err(io::Error::other)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(past_deadline(&e) && !past_deadline(&io::Error::from_raw_os_error(libc::ETIMEDOUT)));
        drop(tx);
        // A thread's calls run on one helper, kept; the call past its deadline took it, and the
        // next is a new one's. A call that panics loses its helper too, and the next goes on.
        let here = || bounded(ms(500), ms(10), || Ok(std::thread::current().id())).unwrap();
        let (a, b) = (here(), here());
        assert!(a == b && a != std::thread::current().id());
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        bounded(ms(20), ms(10), move || rx.recv().map_err(io::Error::other)).unwrap_err();
        let c = here();
        assert!(c != a && here() == c);
        drop(tx);
        assert!(bounded::<()>(ms(500), ms(10), || panic!("(a test's panic)")).is_err());
        assert!(here() != c);
        // (A look as asked, or a little late, counts as it took; one back after the process was
        // stopped (the guards' freeze, 30 minutes) counts as one look.)
        assert_eq!(counted(LOOK, LOOK), LOOK);
        assert_eq!(counted(ms(1300), LOOK), ms(1300));
        assert_eq!(counted(Duration::from_secs(1800), LOOK), LOOK);
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
