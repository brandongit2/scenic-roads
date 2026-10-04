//! The NAS I/O pool (plan §4, Server; §11, Hard SMB mounts). Every NAS filesystem operation
//! (open, stat, list, read, write, rename) runs on one of a few worker threads while the caller
//! waits with a timeout, so a stalled SMB mount can never hang a request.
//!
//! The pool is a circuit breaker. An operation that overruns its timeout has its thread abandoned
//! (counted, and replaced only up to a small fixed number, so threads stuck in the kernel can't
//! pile up); if the share then fails a quick probe too, the NAS is marked offline and from then on
//! calls fail at once with [`IoError::Offline`] without touching the share. A share that answers
//! the probe is busy, not gone (another copy saturating the link): only the slow call fails. One prober thread checks a probe directory
//! every few seconds (a stat, plus a lookup the SMB client can't answer from its caches), under
//! its own timeout and never two at a time, and closes the breaker when the share answers.
//! Network errors from a soft mount, and a file missing because the whole share is gone, trip the
//! breaker the same way.
//!
//! The API is blocking (std threads and channels), for use from plain threads or
//! `spawn_blocking`.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::SeqCst};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// Why a pool call failed.
#[derive(Debug)]
pub enum IoError {
    /// The NAS is marked offline; the call was refused without touching the share.
    Offline,
    /// The operation overran its timeout (the NAS is then marked offline, unless it still answers a
    /// probe: busy, not gone), or no worker was free to start it in time.
    Timeout,
    /// The operation itself failed.
    Io(io::Error),
}

impl IoError {
    /// Whether this means the NAS can't be reached (as opposed to an ordinary I/O error).
    pub fn is_unreachable(&self) -> bool {
        matches!(self, IoError::Offline | IoError::Timeout)
    }

    /// The `IoError` behind an `anyhow` error, if any (through any added context).
    pub fn find(e: &anyhow::Error) -> Option<&IoError> {
        e.chain().find_map(|c| c.downcast_ref::<IoError>())
    }
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::Offline => f.write_str("the NAS is offline"),
            IoError::Timeout => f.write_str("the NAS didn't answer in time"),
            IoError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for IoError {}

impl From<io::Error> for IoError {
    fn from(e: io::Error) -> Self {
        IoError::Io(e)
    }
}

/// Pool settings. [`PoolConfig::new`] fills in the defaults.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Worker threads.
    pub threads: usize,
    /// How long a caller waits for an operation, and for a worker to start it.
    pub op_timeout: Duration,
    /// A directory on the share (e.g. the project folder) that the prober checks.
    pub probe: PathBuf,
    /// Time between probes while offline.
    pub probe_interval: Duration,
    /// How long one probe may take before it counts as failed.
    pub probe_timeout: Duration,
    /// Extra threads allowed to replace stuck ones: at most `threads + spare` ever exist.
    pub spare: usize,
}

impl PoolConfig {
    /// `threads` workers and `op_timeout`; a probe every 3 s, under the same timeout; 2 spare
    /// threads.
    pub fn new(threads: usize, op_timeout: Duration, probe: PathBuf) -> Self {
        Self { threads: threads.max(1), op_timeout, probe, probe_interval: Duration::from_secs(3), probe_timeout: op_timeout, spare: 2 }
    }
}

/// A snapshot of the pool's state, for status displays.
#[derive(Clone, Debug)]
pub struct PoolStatus {
    pub online: bool,
    /// When the NAS was last marked offline, while it still is.
    pub offline_since: Option<SystemTime>,
    /// Worker threads alive, stuck ones included.
    pub workers: usize,
    /// Workers abandoned in an operation that overran its timeout.
    pub stuck: usize,
    /// Operations that overran their timeout since the pool started.
    pub timeouts: u64,
}

/// One entry of [`IoPool::list`].
#[derive(Clone, Debug)]
pub struct DirItem {
    pub name: String,
    pub is_dir: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

type Listener = dyn Fn(bool) + Send + Sync;

// A queued operation's life: QUEUED → RUNNING → DONE, or QUEUED → CANCELLED (its caller gave up
// before any worker took it), or RUNNING → ABANDONED (its caller gave up while it ran).
const QUEUED: u8 = 0;
const RUNNING: u8 = 1;
const DONE: u8 = 2;
const ABANDONED: u8 = 3;
const CANCELLED: u8 = 4;

/// How often a caller waiting for a worker checks whether the NAS went offline meanwhile.
const POLL: Duration = Duration::from_millis(50);

/// How long the share has to answer a probe after an operation overran, to count as busy rather
/// than gone.
const ALIVE_TIMEOUT: Duration = Duration::from_secs(3);

struct Item {
    state: Arc<AtomicU8>,
    run: Box<dyn FnOnce() + Send>,
}

enum Msg<T> {
    Started,
    /// The result, and whether a NotFound came from the whole share being gone.
    Done(io::Result<T>, bool),
}

/// See `Inner::alive`.
#[derive(Default)]
struct Alive {
    started: Option<Instant>,
    answer: Option<(Instant, bool)>,
}

#[derive(Default)]
struct Counts {
    workers: usize,
    stuck: usize,
    timeouts: u64,
    offline_since: Option<SystemTime>,
    next_id: usize,
}

struct Inner {
    cfg: PoolConfig,
    jobs: Mutex<Receiver<Item>>,
    online: AtomicBool,
    closed: AtomicBool,
    prober_running: AtomicBool,
    /// The liveness probe after an overrun: when the one in flight started, and the last answer
    /// (when it came, and whether the share answered). Overruns arriving meanwhile wait for it.
    alive: Mutex<Alive>,
    alive_cv: std::sync::Condvar,
    counts: Mutex<Counts>,
    listener: Mutex<Option<Arc<Listener>>>,
}

/// The bounded NAS I/O pool with its circuit breaker (see the module docs).
pub struct IoPool {
    inner: Arc<Inner>,
    jobs: Sender<Item>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere never leaves these structures half-updated in a way that matters here.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl IoPool {
    /// A pool of `threads` workers; operations time out after `op_timeout`; `probe` is a path on
    /// the share that the prober stats while the NAS is offline.
    pub fn new(threads: usize, op_timeout: Duration, probe: PathBuf) -> Arc<IoPool> {
        Self::with_config(PoolConfig::new(threads, op_timeout, probe))
    }

    /// A pool with every setting given.
    pub fn with_config(mut cfg: PoolConfig) -> Arc<IoPool> {
        cfg.threads = cfg.threads.max(1);
        let (tx, rx) = mpsc::channel();
        let inner = Arc::new(Inner {
            cfg,
            jobs: Mutex::new(rx),
            online: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            prober_running: AtomicBool::new(false),
            alive: Mutex::new(Alive::default()),
            alive_cv: std::sync::Condvar::new(),
            counts: Mutex::new(Counts::default()),
            listener: Mutex::new(None),
        });
        inner.ensure_workers();
        Arc::new(IoPool { inner, jobs: tx })
    }

    pub fn config(&self) -> &PoolConfig {
        &self.inner.cfg
    }

    /// Whether the breaker is closed (calls go to the share).
    pub fn is_online(&self) -> bool {
        self.inner.online.load(SeqCst)
    }

    /// The breaker's state and the threads' counts.
    pub fn status(&self) -> PoolStatus {
        let c = lock(&self.inner.counts);
        PoolStatus { online: self.is_online(), offline_since: c.offline_since, workers: c.workers, stuck: c.stuck, timeouts: c.timeouts }
    }

    /// Calls `f(online)` on every change of state (from whichever thread caused it; keep it short).
    pub fn set_listener(&self, f: impl Fn(bool) + Send + Sync + 'static) {
        *lock(&self.inner.listener) = Some(Arc::new(f));
    }

    /// Opens the breaker now (e.g. the share was found unmounted, or the Mac just woke); the
    /// prober closes it again once the probe path answers.
    pub fn mark_offline(&self, why: &str) {
        self.inner.trip(why);
    }

    /// Runs `f` on a worker. The caller waits up to the pool's timeout for a worker to take it
    /// (`Timeout` if none is free, without marking the NAS offline: busy isn't dead), then up to
    /// the timeout again for it to finish (`Timeout`, and the NAS is marked offline). While the NAS
    /// is offline, `Offline` at once.
    pub fn call<T, F>(&self, f: F) -> Result<T, IoError>
    where
        F: FnOnce() -> io::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.call_timeout(self.inner.cfg.op_timeout, f)
    }

    /// `call` with its own timeout (for operations known to take long, like big sequential reads).
    pub fn call_timeout<T, F>(&self, timeout: Duration, f: F) -> Result<T, IoError>
    where
        F: FnOnce() -> io::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        if !self.is_online() {
            return Err(IoError::Offline);
        }
        self.inner.ensure_workers();
        let (tx, rx) = mpsc::sync_channel::<Msg<T>>(2);
        let state = Arc::new(AtomicU8::new(QUEUED));
        let probe = self.inner.cfg.probe.clone();
        let run = Box::new(move || {
            let _ = tx.send(Msg::Started);
            let r = panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(io::Error::other("NAS operation panicked")));
            // A file missing because the whole share is gone means the NAS is offline.
            let gone = matches!(&r, Err(e) if e.kind() == io::ErrorKind::NotFound) && fs::metadata(&probe).is_err();
            let _ = tx.send(Msg::Done(r, gone));
        });
        if self.jobs.send(Item { state: state.clone(), run }).is_err() {
            return Err(IoError::Io(io::Error::other("the NAS I/O pool has shut down")));
        }

        // Wait for a worker to take it; give up when none is free in time or the NAS goes offline
        // meanwhile (an operation that never started is simply dropped).
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left.min(POLL)) {
                Ok(Msg::Started) => break,
                Ok(Msg::Done(r, gone)) => return self.inner.finish(r, gone),
                Err(RecvTimeoutError::Disconnected) => return Err(IoError::Io(io::Error::other("the NAS I/O pool has shut down"))),
                Err(RecvTimeoutError::Timeout) => {
                    let offline = !self.is_online();
                    if !offline && Instant::now() < deadline {
                        continue;
                    }
                    if state.compare_exchange(QUEUED, CANCELLED, SeqCst, SeqCst).is_ok() {
                        return Err(if offline { IoError::Offline } else { IoError::Timeout });
                    }
                    // A worker took it at the last moment: wait for it like any other.
                    match rx.recv() {
                        Ok(Msg::Started) => break,
                        Ok(Msg::Done(r, gone)) => return self.inner.finish(r, gone),
                        Err(_) => return Err(IoError::Io(io::Error::other("NAS operation lost"))),
                    }
                }
            }
        }

        match rx.recv_timeout(timeout) {
            Ok(Msg::Done(r, gone)) => self.inner.finish(r, gone),
            Ok(Msg::Started) | Err(RecvTimeoutError::Disconnected) => Err(IoError::Io(io::Error::other("NAS operation lost"))),
            Err(RecvTimeoutError::Timeout) => {
                if state.compare_exchange(RUNNING, ABANDONED, SeqCst, SeqCst).is_err() {
                    // It finished just as the time ran out; the result is already in the channel.
                    return match rx.recv() {
                        Ok(Msg::Done(r, gone)) => self.inner.finish(r, gone),
                        _ => Err(IoError::Io(io::Error::other("NAS operation lost"))),
                    };
                }
                {
                    let mut c = lock(&self.inner.counts);
                    c.stuck += 1;
                    c.timeouts += 1;
                }
                if let Ok(Msg::Done(r, gone)) = rx.try_recv() {
                    // Done at the very deadline: no stall after all (the worker un-counts itself).
                    return self.inner.finish(r, gone);
                }
                if !self.inner.alive_within(ALIVE_TIMEOUT) {
                    self.inner.trip(&format!("an operation took longer than {timeout:?}"));
                }
                Err(IoError::Timeout)
            }
        }
    }

    /// Opens a file for reading.
    pub fn open(&self, path: &Path) -> Result<Arc<File>, IoError> {
        Ok(self.open_len(path)?.0)
    }

    /// Opens a file for reading and gets its length, in one trip.
    pub fn open_len(&self, path: &Path) -> Result<(Arc<File>, u64), IoError> {
        let p = path.to_owned();
        self.call(move || {
            let f = File::open(&p)?;
            let len = f.metadata()?.len();
            Ok((Arc::new(f), len))
        })
    }

    /// Reads exactly `len` bytes at `off` (pread; an error if the file is shorter).
    pub fn read_at(&self, file: &Arc<File>, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        let f = file.clone();
        self.call(move || {
            let mut buf = vec![0u8; len];
            f.read_exact_at(&mut buf, off)?;
            Ok(buf)
        })
    }

    /// `read_at` with its own timeout.
    pub fn read_at_timeout(&self, file: &Arc<File>, off: u64, len: usize, timeout: Duration) -> Result<Vec<u8>, IoError> {
        let f = file.clone();
        self.call_timeout(timeout, move || {
            let mut buf = vec![0u8; len];
            f.read_exact_at(&mut buf, off)?;
            Ok(buf)
        })
    }

    /// Reads a whole (small) file.
    pub fn read_all(&self, path: &Path) -> Result<Vec<u8>, IoError> {
        let p = path.to_owned();
        self.call(move || fs::read(&p))
    }

    /// `fs::metadata` (following symlinks).
    pub fn stat(&self, path: &Path) -> Result<Metadata, IoError> {
        let p = path.to_owned();
        self.call(move || fs::metadata(&p))
    }

    /// Whether `path` exists. A missing path while the whole share is gone is `Offline`, not false.
    pub fn exists(&self, path: &Path) -> Result<bool, IoError> {
        let (p, probe) = (path.to_owned(), self.inner.cfg.probe.clone());
        self.call(move || match fs::metadata(&p) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound && fs::metadata(&probe).is_ok() => Ok(false),
            Err(e) => Err(e),
        })
    }

    /// The entries of a directory with their type, size and modification time, sorted by name.
    pub fn list(&self, dir: &Path) -> Result<Vec<DirItem>, IoError> {
        let d = dir.to_owned();
        self.call(move || {
            let mut out = Vec::new();
            for e in fs::read_dir(&d)? {
                let e = e?;
                // Gone between the listing and the stat: skip it.
                let Ok(m) = e.metadata() else { continue };
                out.push(DirItem { name: e.file_name().to_string_lossy().into_owned(), is_dir: m.is_dir(), len: m.len(), modified: m.modified().ok() });
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(out)
        })
    }

    /// `fs::create_dir_all`.
    pub fn create_dir_all(&self, dir: &Path) -> Result<(), IoError> {
        let d = dir.to_owned();
        self.call(move || fs::create_dir_all(&d))
    }

    /// `fs::rename`, replacing `to` if it exists (both on the share).
    pub fn rename(&self, from: &Path, to: &Path) -> Result<(), IoError> {
        let (a, b) = (from.to_owned(), to.to_owned());
        self.call(move || fs::rename(&a, &b))
    }

    /// Creates `path` exclusively (`O_EXCL`: `AlreadyExists` if it's there) and writes `bytes`.
    pub fn write_new(&self, path: &Path, bytes: Vec<u8>) -> Result<(), IoError> {
        let p = path.to_owned();
        self.call(move || {
            let mut f = OpenOptions::new().write(true).create_new(true).open(&p)?;
            f.write_all(&bytes)?;
            f.sync_all()
        })
    }

    /// Creates or truncates `path` and writes `bytes`, synced. (Callers write a temporary file this
    /// way and rename it over the real one, so no reader sees half of it; one left by a write that
    /// never finished is simply written again.)
    pub fn write(&self, path: &Path, bytes: Vec<u8>) -> Result<(), IoError> {
        let p = path.to_owned();
        self.call(move || {
            let mut f = File::create(&p)?;
            f.write_all(&bytes)?;
            f.sync_all()
        })
    }
}

impl Drop for IoPool {
    fn drop(&mut self) {
        // Workers leave when the job channel closes (stuck ones when they return); the prober
        // notices this flag.
        self.inner.closed.store(true, SeqCst);
    }
}

impl Inner {
    /// Starts workers until `threads` are free to work, within the `threads + spare` bound.
    fn ensure_workers(self: &Arc<Self>) {
        let mut c = lock(&self.counts);
        while c.workers - c.stuck < self.cfg.threads && c.workers < self.cfg.threads + self.cfg.spare {
            let id = c.next_id;
            let me = self.clone();
            match thread::Builder::new().name(format!("nas-io-{id}")).spawn(move || worker(me)) {
                Ok(_) => {
                    c.workers += 1;
                    c.next_id += 1;
                }
                Err(e) => {
                    eprintln!("nas: can't start an I/O thread: {e}");
                    break;
                }
            }
        }
    }

    fn finish<T>(self: &Arc<Self>, r: io::Result<T>, gone: bool) -> Result<T, IoError> {
        match r {
            Ok(v) => Ok(v),
            Err(_) if gone => {
                self.trip(&format!("{} is gone", self.cfg.probe.display()));
                Err(IoError::Offline)
            }
            Err(e) => {
                if is_disconnect(&e) {
                    self.trip(&e.to_string());
                }
                Err(IoError::Io(e))
            }
        }
    }

    /// Whether the share answers a probe within `t` of the probe's start, on a helper thread. One
    /// probe at a time: an overrun while one is in flight waits for its answer (many reads
    /// overrunning together on a busy link make one probe, not a trip each); a probe still
    /// unanswered after `t` means the share doesn't answer.
    fn alive_within(self: &Arc<Self>, t: Duration) -> bool {
        let mut g = lock(&self.alive);
        let start = match g.started {
            Some(s) => s,
            None => {
                let now = Instant::now();
                g.started = Some(now);
                let (me, path) = (self.clone(), self.cfg.probe.clone());
                let spawned = thread::Builder::new().name("nas-alive".into()).spawn(move || {
                    let ok = probe_ok(&path);
                    let mut g = lock(&me.alive);
                    g.started = None;
                    g.answer = Some((Instant::now(), ok));
                    me.alive_cv.notify_all();
                });
                if spawned.is_err() {
                    g.started = None;
                    return false;
                }
                now
            }
        };
        let deadline = start + t;
        loop {
            if let Some((at, ok)) = g.answer {
                if at >= start {
                    return ok;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            g = self.alive_cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Opens the breaker (if closed) and makes sure a prober is running.
    fn trip(self: &Arc<Self>, why: &str) {
        if self.online.swap(false, SeqCst) {
            eprintln!("nas: offline: {why}");
            lock(&self.counts).offline_since = Some(SystemTime::now());
            self.notify(false);
        }
        self.start_prober();
    }

    fn set_online(&self) {
        if !self.online.swap(true, SeqCst) {
            let since = lock(&self.counts).offline_since.take();
            let away = since.and_then(|t| t.elapsed().ok()).map(|d| format!(" after {:.0?}", d)).unwrap_or_default();
            eprintln!("nas: back online{away}");
            self.notify(true);
        }
    }

    fn notify(&self, online: bool) {
        let l = lock(&self.listener).clone();
        if let Some(l) = l {
            l(online);
        }
    }

    fn start_prober(self: &Arc<Self>) {
        if self.closed.load(SeqCst) || self.prober_running.swap(true, SeqCst) {
            return;
        }
        let me = self.clone();
        if let Err(e) = thread::Builder::new().name("nas-prober".into()).spawn(move || prober(me)) {
            eprintln!("nas: can't start the prober: {e}");
            self.prober_running.store(false, SeqCst);
        }
    }

    /// Sleeps up to `d`, returning early (false) when the pool closes.
    fn sleep(&self, d: Duration) -> bool {
        let end = Instant::now() + d;
        while !self.closed.load(SeqCst) {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            thread::sleep(left.min(POLL));
        }
        false
    }
}

fn worker(inner: Arc<Inner>) {
    loop {
        let item = {
            let rx = lock(&inner.jobs);
            match rx.recv() {
                Ok(item) => item,
                Err(_) => break,
            }
        };
        if item.state.compare_exchange(QUEUED, RUNNING, SeqCst, SeqCst).is_err() {
            continue; // its caller gave up before it started
        }
        (item.run)();
        if item.state.compare_exchange(RUNNING, DONE, SeqCst, SeqCst).is_err() {
            // The caller gave up on this operation and counted this thread as stuck. Rejoin, or
            // leave if a replacement has taken this thread's place meanwhile.
            let mut c = lock(&inner.counts);
            c.stuck -= 1;
            if c.workers - c.stuck > inner.cfg.threads {
                c.workers -= 1;
                return;
            }
        }
    }
    lock(&inner.counts).workers -= 1;
}

/// While offline: probe the share every `probe_interval` on a helper thread, one probe in flight
/// at a time (a stuck one is waited for, never duplicated), until one succeeds.
fn prober(inner: Arc<Inner>) {
    let (req_tx, req_rx) = mpsc::channel::<()>();
    let (res_tx, res_rx) = mpsc::channel::<bool>();
    let path = inner.cfg.probe.clone();
    let spawned = thread::Builder::new().name("nas-probe".into()).spawn(move || {
        for () in req_rx {
            if res_tx.send(probe_ok(&path)).is_err() {
                break;
            }
        }
    });
    if let Err(e) = spawned {
        eprintln!("nas: can't start the probe thread: {e}");
        inner.prober_running.store(false, SeqCst);
        return;
    }
    let mut in_flight = false;
    loop {
        // First wait, so the client's attribute cache from before the stall has a chance to expire.
        if !inner.sleep(inner.cfg.probe_interval) {
            break;
        }
        if !in_flight {
            if req_tx.send(()).is_err() {
                break;
            }
            in_flight = true;
        }
        match res_rx.recv_timeout(inner.cfg.probe_timeout) {
            Ok(true) => {
                inner.set_online();
                inner.prober_running.store(false, SeqCst);
                // A timeout may have tripped the breaker again right after; keep probing unless
                // another prober has taken over.
                if inner.online.load(SeqCst) || inner.prober_running.swap(true, SeqCst) {
                    return;
                }
                in_flight = false;
            }
            Ok(false) => in_flight = false,
            Err(RecvTimeoutError::Timeout) => {} // the probe is stuck: keep waiting for it
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    inner.prober_running.store(false, SeqCst);
}

/// Whether the share answers: the probe directory exists, and a lookup of a name never asked
/// before (so the answer can't come from the SMB client's caches) gets a reply, found or not.
fn probe_ok(dir: &Path) -> bool {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if fs::metadata(dir).is_err() {
        return false;
    }
    let unique = dir.join(format!(".scenic-probe-{}-{}", std::process::id(), N.fetch_add(1, SeqCst)));
    match fs::symlink_metadata(unique) {
        Ok(_) => true,
        Err(e) => matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory),
    }
}

/// Errors that mean the share itself is unreachable (what a soft mount returns once it gives up).
fn is_disconnect(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(
            libc::ETIMEDOUT
                | libc::ENOTCONN
                | libc::ENETDOWN
                | libc::ENETUNREACH
                | libc::ENETRESET
                | libc::ECONNABORTED
                | libc::ECONNRESET
                | libc::EHOSTDOWN
                | libc::EHOSTUNREACH
                | libc::ESHUTDOWN
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn cfg(probe: PathBuf, threads: usize, timeout_ms: u64) -> PoolConfig {
        PoolConfig { probe_interval: Duration::from_millis(20), ..PoolConfig::new(threads, Duration::from_millis(timeout_ms), probe) }
    }

    /// Polls `cond` for up to `secs` seconds.
    fn eventually(secs: u64, cond: impl Fn() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if cond() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        cond()
    }

    #[test]
    fn operations() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 2, 2000));
        assert_eq!(pool.call(|| Ok(41 + 1)).unwrap(), 42);
        let p = dir.path().join("a.bin");
        pool.write_new(&p, b"0123456789".to_vec()).unwrap();
        match pool.write_new(&p, b"x".to_vec()) {
            Err(IoError::Io(e)) => assert_eq!(e.kind(), io::ErrorKind::AlreadyExists),
            other => panic!("{other:?}"),
        }
        let (f, len) = pool.open_len(&p).unwrap();
        assert_eq!(len, 10);
        assert_eq!(pool.read_at(&f, 3, 4).unwrap(), b"3456");
        assert!(matches!(pool.read_at(&f, 8, 4), Err(IoError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof));
        assert_eq!(pool.read_all(&p).unwrap(), b"0123456789");
        assert_eq!(pool.stat(&p).unwrap().len(), 10);
        assert!(pool.exists(&p).unwrap());
        assert!(!pool.exists(&dir.path().join("nope")).unwrap());
        fs::create_dir(dir.path().join("sub")).unwrap();
        let q = dir.path().join("b.bin");
        pool.rename(&p, &q).unwrap();
        let names: Vec<_> = pool.list(dir.path()).unwrap().into_iter().map(|e| (e.name, e.is_dir, e.len)).collect();
        assert_eq!(names, [("b.bin".to_string(), false, 10), ("sub".to_string(), true, names[1].2)]);
        // A file written through a temporary one, over a stale temporary file, in a new folder.
        let deeper = dir.path().join("sub/deeper");
        pool.create_dir_all(&deeper).unwrap();
        pool.create_dir_all(&deeper).unwrap();
        let tmp = deeper.join("c.bin.tmp");
        pool.write(&tmp, b"stale, and longer".to_vec()).unwrap();
        pool.write(&tmp, b"new".to_vec()).unwrap();
        pool.rename(&tmp, &deeper.join("c.bin")).unwrap();
        assert_eq!(fs::read(deeper.join("c.bin")).unwrap(), b"new");
        assert!(!tmp.exists());
        // An ordinary error leaves the NAS online.
        assert!(matches!(pool.read_all(&dir.path().join("missing")), Err(IoError::Io(e)) if e.kind() == io::ErrorKind::NotFound));
        assert!(pool.is_online());
        // A panicking operation is an error, and its worker lives on.
        assert!(matches!(pool.call(|| -> io::Result<()> { panic!("boom") }), Err(IoError::Io(_))));
        assert_eq!(pool.call(|| Ok(1)).unwrap(), 1);
        assert_eq!(pool.status().workers, 2);
    }

    #[test]
    fn timeout_opens_breaker_and_prober_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join("share");
        fs::create_dir(&probe).unwrap();
        let pool = IoPool::with_config(cfg(probe.clone(), 2, 200));
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        pool.set_listener(move |online| ev.lock().unwrap().push(online));

        // The share "disappears" and an operation stalls.
        fs::remove_dir(&probe).unwrap();
        let t0 = Instant::now();
        assert!(matches!(pool.call(|| { thread::sleep(Duration::from_millis(1500)); Ok(()) }), Err(IoError::Timeout)));
        assert!(t0.elapsed() < Duration::from_millis(1000));
        assert!(!pool.is_online());
        let st = pool.status();
        assert_eq!((st.stuck, st.timeouts), (1, 1));
        assert!(st.offline_since.is_some());

        // Offline: refused at once, without queueing.
        let t0 = Instant::now();
        assert!(matches!(pool.call(|| Ok(())), Err(IoError::Offline)));
        assert!(matches!(pool.read_all(&dir.path().join("x")), Err(IoError::Offline)));
        assert!(t0.elapsed() < Duration::from_millis(50));

        // The probe keeps failing while the share is away.
        thread::sleep(Duration::from_millis(200));
        assert!(!pool.is_online());

        // It comes back: the prober closes the breaker.
        fs::create_dir(&probe).unwrap();
        assert!(eventually(3, || pool.is_online()));
        assert_eq!(pool.call(|| Ok(7)).unwrap(), 7);
        assert_eq!(*events.lock().unwrap(), [false, true]);
        assert!(pool.status().offline_since.is_none());

        // The stuck thread returns and the pool settles back to its size.
        assert!(eventually(5, || {
            let s = pool.status();
            s.stuck == 0 && s.workers == 2
        }));
    }

    #[test]
    fn slow_share_that_answers_isnt_offline() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 2, 100));
        // An operation overruns while the share still answers probes: busy, not gone.
        let r = pool.call(|| {
            thread::sleep(Duration::from_millis(600));
            Ok(())
        });
        assert!(matches!(r, Err(IoError::Timeout)), "{r:?}");
        assert!(pool.is_online());
        let st = pool.status();
        assert_eq!((st.stuck, st.timeouts), (1, 1));
        assert!(st.offline_since.is_none());
        // Other calls go on meanwhile, and the slow thread rejoins.
        assert_eq!(pool.call(|| Ok(5)).unwrap(), 5);
        assert!(eventually(3, || pool.status().stuck == 0));
    }

    #[test]
    fn many_overruns_on_a_busy_share_make_one_probe_and_no_trip() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 8, 100));
        // Eight reads started together all overrun; the share answers probes.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let p = pool.clone();
                thread::spawn(move || p.call(|| {
                    thread::sleep(Duration::from_millis(500));
                    Ok(())
                }))
            })
            .collect();
        for h in handles {
            assert!(matches!(h.join().unwrap(), Err(IoError::Timeout)));
        }
        assert!(pool.is_online(), "a busy share isn't offline");
        assert_eq!(pool.status().timeouts, 8);
        assert!(eventually(3, || pool.status().stuck == 0));
    }

    #[test]
    fn overruns_on_a_gone_share_trip() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join("share");
        fs::create_dir(&probe).unwrap();
        let pool = IoPool::with_config(PoolConfig { probe_interval: Duration::from_secs(60), ..cfg(probe.clone(), 4, 100) });
        fs::remove_dir(&probe).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let p = pool.clone();
                thread::spawn(move || p.call(|| {
                    thread::sleep(Duration::from_millis(400));
                    Ok(())
                }))
            })
            .collect();
        for h in handles {
            assert!(matches!(h.join().unwrap(), Err(IoError::Timeout)));
        }
        assert!(!pool.is_online());
    }

    #[test]
    fn stuck_threads_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 2, 50));
        let peak = Arc::new(AtomicUsize::new(0));
        for _ in 0..6 {
            assert!(eventually(3, || pool.is_online()));
            let r = pool.call(|| {
                thread::sleep(Duration::from_millis(1500));
                Ok(())
            });
            assert!(matches!(r, Err(IoError::Timeout)), "{r:?}");
            let s = pool.status();
            peak.fetch_max(s.workers, SeqCst);
            assert!(s.workers <= 4, "{s:?}");
        }
        let s = pool.status();
        assert_eq!(s.workers, 4);
        assert!(s.stuck >= 4, "{s:?}");
        // Every thread is stuck: a call can't start, times out, and is dropped unrun.
        assert!(eventually(3, || pool.is_online()));
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        let r = pool.call(move || {
            r2.store(true, SeqCst);
            Ok(())
        });
        assert!(matches!(r, Err(IoError::Timeout)), "{r:?}");
        // Then they all come back, the extras leave, and work resumes.
        assert!(eventually(5, || {
            let s = pool.status();
            s.stuck == 0 && s.workers == 2
        }));
        assert!(eventually(3, || pool.is_online()));
        assert_eq!(pool.call(|| Ok(3)).unwrap(), 3);
        thread::sleep(Duration::from_millis(50));
        assert!(!ran.load(SeqCst), "a cancelled operation ran");
        assert!(peak.load(SeqCst) <= 4);
    }

    #[test]
    fn busy_pool_times_out_without_tripping() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 1, 1000));
        let p2 = pool.clone();
        let long = thread::spawn(move || p2.call(|| {
            thread::sleep(Duration::from_millis(400));
            Ok(1)
        }));
        thread::sleep(Duration::from_millis(50));
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        let r = pool.call_timeout(Duration::from_millis(100), move || {
            r2.store(true, SeqCst);
            Ok(())
        });
        assert!(matches!(r, Err(IoError::Timeout)), "{r:?}");
        assert!(pool.is_online(), "a queue wait must not mark the NAS offline");
        assert_eq!(long.join().unwrap().unwrap(), 1);
        thread::sleep(Duration::from_millis(50));
        assert!(!ran.load(SeqCst));
        assert_eq!(pool.status().timeouts, 0);
    }

    #[test]
    fn network_errors_and_vanished_share_trip() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join("share");
        fs::create_dir(&probe).unwrap();
        let pool = IoPool::with_config(PoolConfig { probe_interval: Duration::from_secs(60), ..cfg(probe.clone(), 1, 1000) });
        let r = pool.call(|| -> io::Result<()> { Err(io::Error::from_raw_os_error(libc::ETIMEDOUT)) });
        assert!(matches!(r, Err(IoError::Io(_))));
        assert!(!pool.is_online());

        let pool = IoPool::with_config(PoolConfig { probe_interval: Duration::from_secs(60), ..cfg(probe.clone(), 1, 1000) });
        fs::remove_dir(&probe).unwrap();
        assert!(matches!(pool.stat(&probe.join("file")), Err(IoError::Offline)));
        assert!(!pool.is_online());

        let pool = IoPool::with_config(PoolConfig { probe_interval: Duration::from_secs(60), ..cfg(probe.clone(), 1, 1000) });
        assert!(matches!(pool.exists(&probe.join("file")), Err(IoError::Offline)));
        pool.mark_offline("test");
        assert!(matches!(pool.call(|| Ok(())), Err(IoError::Offline)));
    }

    #[test]
    fn error_lookup_through_context() {
        let e = anyhow::Error::from(IoError::Offline).context("reading a pack");
        assert!(IoError::find(&e).is_some_and(IoError::is_unreachable));
        let e = anyhow::anyhow!("plain");
        assert!(IoError::find(&e).is_none());
    }
}
