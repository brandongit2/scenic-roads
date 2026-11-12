//! The server's NAS I/O pool (docs/macs.md, Server; task #478): a circuit breaker over
//! crate::deadline. Every NAS filesystem operation (open, stat, list, read, write, rename) is a
//! deadline call, at most `calls` of them at once, so a stalled SMB mount can never hang a request.
//!
//! An operation that overruns its timeout is left to the kernel (crate::deadline: its process's
//! calls held there are capped, `deadline::cap`); if the share then fails a quick probe too, the
//! NAS is marked offline and from then on calls fail at once with [`IoError::Offline`] without
//! touching the share. A share that answers the probe is busy, not gone (another copy saturating
//! the link): only the slow call fails. One prober thread checks a probe directory every few
//! seconds (a stat, plus a lookup the SMB client can't answer from its caches), under its own
//! timeout and never two at a time, and closes the breaker when the share answers. Network errors
//! from a soft mount, and a file missing because the whole share is gone, trip the breaker the
//! same way.
//!
//! The API is blocking, for use from plain threads or `spawn_blocking`.

use crate::nasfs::NasPath;
use crate::sys::PosIo;
use std::fmt::Display;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::SeqCst};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// Why a pool call failed.
#[derive(Debug)]
pub enum IoError {
    /// The NAS is marked offline; the call was refused without touching the share.
    Offline,
    /// The operation overran its timeout (the NAS is then marked offline, unless it still answers a
    /// probe: busy, not gone), or it couldn't start in time (the pool's calls all busy, or the
    /// process's calls held in the kernel at their cap).
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
    /// Operations under way at once (a call waits its timeout at most for its turn).
    pub calls: usize,
    /// How long a caller waits for an operation, and for its turn to start it.
    pub op_timeout: Duration,
    /// A directory on the share (e.g. the project folder) that the prober checks.
    pub probe: PathBuf,
    /// Time between probes while offline.
    pub probe_interval: Duration,
    /// How long one probe may take before it counts as failed.
    pub probe_timeout: Duration,
    /// How long the share has to answer a probe after an operation overran, to count as busy
    /// rather than gone.
    pub alive_timeout: Duration,
}

impl PoolConfig {
    /// `calls` at once and `op_timeout`; a probe every 3 s, under the same timeout; 3 s for the
    /// share to answer after an overrun.
    pub fn new(calls: usize, op_timeout: Duration, probe: PathBuf) -> Self {
        Self {
            calls: calls.max(1),
            op_timeout,
            probe,
            probe_interval: Duration::from_secs(3),
            probe_timeout: op_timeout,
            alive_timeout: Duration::from_secs(3),
        }
    }
}

/// A snapshot of the pool's state, for status displays.
#[derive(Clone, Debug)]
pub struct PoolStatus {
    pub online: bool,
    /// When the NAS was last marked offline, while it still is.
    pub offline_since: Option<SystemTime>,
    /// Its operations that overran their timeout and are still in the kernel.
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

/// How often a caller waiting for its turn checks whether the NAS went offline meanwhile.
const POLL: Duration = Duration::from_millis(50);

// An operation's life: RUNNING → DONE, or RUNNING → ABANDONED (its caller gave up on it at its
// timeout: counted stuck until it ends).
const RUNNING: u8 = 0;
const DONE: u8 = 1;
const ABANDONED: u8 = 2;

/// See `Inner::alive`.
#[derive(Default)]
struct Alive {
    started: Option<Instant>,
    answer: Option<(Instant, bool)>,
}

#[derive(Default)]
struct Counts {
    /// Operations under way (`PoolConfig::calls` at most).
    busy: usize,
    stuck: usize,
    timeouts: u64,
    offline_since: Option<SystemTime>,
}

struct Inner {
    cfg: PoolConfig,
    online: AtomicBool,
    closed: AtomicBool,
    prober_running: AtomicBool,
    /// The prober's next probe is made at once (`probe_first`), not after `probe_interval`.
    probe_now: AtomicBool,
    /// The liveness probe after an overrun: when the one in flight started, and the last answer
    /// (when it came, and whether the share answered). Overruns arriving meanwhile wait for it.
    alive: Mutex<Alive>,
    alive_cv: Condvar,
    counts: Mutex<Counts>,
    /// Signalled as an operation's turn is given back.
    turn_cv: Condvar,
    listener: Mutex<Option<Arc<Listener>>>,
}

/// The NAS I/O pool with its circuit breaker (see the module docs).
pub struct IoPool {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic elsewhere never leaves these structures half-updated in a way that matters here.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// An operation's turn (`Inner::turn`), given back as it's dropped.
struct Turn<'a>(&'a Inner);

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        lock(&self.0.counts).busy -= 1;
        self.0.turn_cv.notify_one();
    }
}

impl IoPool {
    /// A pool of `calls` operations at once; operations time out after `op_timeout`; `probe` is a
    /// path on the share that the prober stats while the NAS is offline.
    pub fn new(calls: usize, op_timeout: Duration, probe: PathBuf) -> Arc<IoPool> {
        Self::with_config(PoolConfig::new(calls, op_timeout, probe))
    }

    /// A pool over the NAS project folder `root`, which its prober stats while the NAS is offline.
    pub fn for_root(root: &crate::nasfs::NasRoot, calls: usize, op_timeout: Duration) -> Arc<IoPool> {
        Self::new(calls, op_timeout, root.unchecked_path())
    }

    /// A pool with every setting given.
    pub fn with_config(mut cfg: PoolConfig) -> Arc<IoPool> {
        cfg.calls = cfg.calls.max(1);
        let inner = Arc::new(Inner {
            cfg,
            online: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            prober_running: AtomicBool::new(false),
            probe_now: AtomicBool::new(false),
            alive: Mutex::new(Alive::default()),
            alive_cv: Condvar::new(),
            counts: Mutex::new(Counts::default()),
            turn_cv: Condvar::new(),
            listener: Mutex::new(None),
        });
        Arc::new(IoPool { inner })
    }

    pub fn config(&self) -> &PoolConfig {
        &self.inner.cfg
    }

    /// Whether the breaker is closed (calls go to the share).
    pub fn is_online(&self) -> bool {
        self.inner.online.load(SeqCst)
    }

    /// The breaker's state and the stuck operations' counts.
    pub fn status(&self) -> PoolStatus {
        let c = lock(&self.inner.counts);
        PoolStatus { online: self.is_online(), offline_since: c.offline_since, stuck: c.stuck, timeouts: c.timeouts }
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

    /// For a new pool over a share that may not answer (a mount left from home): calls are refused
    /// (`Offline`) until the share answers a probe, made at once, so none waits on it first.
    pub fn probe_first(&self) {
        if self.inner.online.swap(false, SeqCst) {
            lock(&self.inner.counts).offline_since = Some(SystemTime::now());
        }
        self.inner.probe_now.store(true, SeqCst);
        self.inner.start_prober();
    }

    /// Runs `f` as a deadline call. The caller waits up to the pool's timeout for its turn
    /// (`Timeout` if none comes, without marking the NAS offline: busy isn't dead), then up to the
    /// timeout again for it to finish (`Timeout`, and the NAS is marked offline unless it answers a
    /// probe). While the NAS is offline, `Offline` at once.
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
        self.run(&self.inner.cfg.probe.display(), timeout, f)
    }

    /// `call_timeout`, the operation on `on` (its errors and the deadline's log name it).
    fn run<T, F>(&self, on: &dyn Display, timeout: Duration, f: F) -> Result<T, IoError>
    where
        F: FnOnce() -> io::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        if !self.is_online() {
            return Err(IoError::Offline);
        }
        let _w = crate::waits::nas();
        let turn = self.inner.turn(timeout)?;
        let (probe, state) = (self.inner.cfg.probe.clone(), Arc::new(AtomicU8::new(RUNNING)));
        let (me, st) = (self.inner.clone(), state.clone());
        let r = crate::deadline::call(on, &"a NAS operation", timeout, move || {
            let r = panic::catch_unwind(AssertUnwindSafe(f))
                .unwrap_or_else(|_| Err(io::Error::other("NAS operation panicked")));
            // A file missing because the whole share is gone means the NAS is offline.
            let gone = matches!(&r, Err(e) if e.kind() == io::ErrorKind::NotFound) && fs::metadata(&probe).is_err();
            if st.compare_exchange(RUNNING, DONE, SeqCst, SeqCst).is_err() {
                // (Its caller gave up on it: it was counted stuck.)
                lock(&me.counts).stuck -= 1;
            }
            Ok((r, gone))
        });
        drop(turn);
        match r {
            Ok((r, gone)) => self.inner.finish(r, gone),
            Err(e) if crate::deadline::past_deadline(&e) => {
                {
                    let mut c = lock(&self.inner.counts);
                    c.timeouts += 1;
                    if state.compare_exchange(RUNNING, ABANDONED, SeqCst, SeqCst).is_ok() {
                        c.stuck += 1;
                    }
                }
                if !self.inner.alive_within(self.inner.cfg.alive_timeout) {
                    self.inner.trip(&format!("an operation took longer than {timeout:?}"));
                }
                Err(IoError::Timeout)
            }
            // (Refused: the process's calls held at their cap. Busy, not gone.)
            Err(e) if crate::deadline::refused(&e) => Err(IoError::Timeout),
            Err(e) => Err(IoError::Io(e)),
        }
    }

    /// Opens a file for reading.
    pub fn open(&self, path: &NasPath) -> Result<Arc<File>, IoError> {
        Ok(self.open_len(path)?.0)
    }

    /// Opens a file for reading and gets its length, in one trip (the open retried on a transient
    /// error, on the root's schedule: the server's short one). Counted as a file opened.
    pub fn open_len(&self, path: &NasPath) -> Result<(Arc<File>, u64), IoError> {
        let p = path.unchecked_path();
        let t = Instant::now();
        let r = self.retried(path, "open", || {
            let p = p.clone();
            self.run(&path.display(), self.inner.cfg.op_timeout, move || {
                let f = File::open(&p)?;
                let len = f.metadata()?.len();
                Ok((Arc::new(f), len))
            })
        })?;
        crate::nasio::opened(&p, r.1, t.elapsed().as_millis() as u64);
        Ok(r)
    }

    /// `f()`, tried again on the root's schedule while it fails with a transient I/O error (never
    /// a timeout or the breaker's `Offline`: those are the pool's own).
    fn retried<T>(&self, path: &NasPath, what: &str, mut f: impl FnMut() -> Result<T, IoError>) -> Result<T, IoError> {
        let mut other = None;
        let r = path.retry(what, || match f() {
            Ok(t) => Ok(t),
            Err(IoError::Io(e)) => Err(e),
            Err(e) => {
                other = Some(e);
                Err(io::Error::other("(pool)"))
            }
        });
        match (r, other) {
            (Ok(t), _) => Ok(t),
            (Err(_), Some(e)) => Err(e),
            (Err(e), None) => Err(IoError::Io(e)),
        }
    }

    /// Reads exactly `len` bytes at `off` (pread; an error if the file is shorter).
    pub fn read_at(&self, file: &Arc<File>, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        self.read_at_timeout(file, off, len, self.inner.cfg.op_timeout)
    }

    /// `read_at` with its own timeout.
    pub fn read_at_timeout(
        &self,
        file: &Arc<File>,
        off: u64,
        len: usize,
        timeout: Duration,
    ) -> Result<Vec<u8>, IoError> {
        let f = file.clone();
        self.call_timeout(timeout, move || {
            let mut buf = vec![0u8; len];
            f.read_exact_at(&mut buf, off)?;
            Ok(buf)
        })
    }

    /// Reads a whole (small) file. Counted as a file read; retried as `open_len`.
    pub fn read_all(&self, path: &NasPath) -> Result<Vec<u8>, IoError> {
        let p = path.unchecked_path();
        let t = Instant::now();
        let b = self.retried(path, "read", || {
            let p = p.clone();
            self.run(&path.display(), self.inner.cfg.op_timeout, move || fs::read(&p))
        })?;
        crate::nasio::opened(&p, b.len() as u64, t.elapsed().as_millis() as u64);
        crate::nasio::read(b.len() as u64);
        Ok(b)
    }

    /// `fs::metadata` (following symlinks). One `Stats`; retried as `open_len`.
    pub fn stat(&self, path: &NasPath) -> Result<Metadata, IoError> {
        let p = path.unchecked_path();
        crate::nasio::add(crate::nasio::Count::Stats, 1);
        self.retried(path, "stat", || {
            let p = p.clone();
            self.run(&path.display(), self.inner.cfg.op_timeout, move || fs::metadata(&p))
        })
    }

    /// Whether `path` exists. A missing path while the whole share is gone is `Offline`, not false.
    /// One `Stats`; retried as `open_len`.
    pub fn exists(&self, path: &NasPath) -> Result<bool, IoError> {
        let p = path.unchecked_path();
        crate::nasio::add(crate::nasio::Count::Stats, 1);
        self.retried(path, "stat", || {
            let (p, probe) = (p.clone(), self.inner.cfg.probe.clone());
            self.run(&path.display(), self.inner.cfg.op_timeout, move || match fs::metadata(&p) {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == io::ErrorKind::NotFound && fs::metadata(&probe).is_ok() => Ok(false),
                Err(e) => Err(e),
            })
        })
    }

    /// The entries of a directory with their type, size and modification time, sorted by name.
    /// Its entries counted as `Listed`; retried as `open_len`.
    pub fn list(&self, dir: &NasPath) -> Result<Vec<DirItem>, IoError> {
        let d = dir.unchecked_path();
        let v = self.retried(dir, "list", || {
            let d = d.clone();
            self.run(&dir.display(), self.inner.cfg.op_timeout, move || {
                let mut out = Vec::new();
                for e in fs::read_dir(&d)? {
                    let e = e?;
                    // Gone between the listing and the stat: skip it.
                    let Ok(m) = e.metadata() else { continue };
                    out.push(DirItem {
                        name: e.file_name().to_string_lossy().into_owned(),
                        is_dir: m.is_dir(),
                        len: m.len(),
                        modified: m.modified().ok(),
                    });
                }
                out.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(out)
            })
        })?;
        crate::nasio::add(crate::nasio::Count::Listed, v.len() as u64);
        Ok(v)
    }

    /// `fs::create_dir_all`.
    pub fn create_dir_all(&self, dir: &NasPath) -> Result<(), IoError> {
        let d = dir.unchecked_path();
        self.run(&dir.display(), self.inner.cfg.op_timeout, move || fs::create_dir_all(&d))
    }

    /// `fs::rename`, replacing `to` if it exists (both on the share).
    pub fn rename(&self, from: &NasPath, to: &NasPath) -> Result<(), IoError> {
        let (a, b) = (from.unchecked_path(), to.unchecked_path());
        self.run(&from.display(), self.inner.cfg.op_timeout, move || fs::rename(&a, &b))
    }

    /// Creates `path` exclusively (`O_EXCL`: `AlreadyExists` if it's there) and writes `bytes`,
    /// closed checked (crate::sys::close). Counted as a file made. Never retried.
    pub fn write_new(&self, path: &NasPath, bytes: Vec<u8>) -> Result<(), IoError> {
        let p = path.unchecked_path();
        let (n, t, q) = (bytes.len() as u64, Instant::now(), p.clone());
        self.run(&path.display(), self.inner.cfg.op_timeout, move || {
            let mut f = OpenOptions::new().write(true).create_new(true).open(&p)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            crate::sys::close(f, &p)
        })?;
        crate::nasio::wrote(&q, n, t.elapsed().as_millis() as u64);
        Ok(())
    }

    /// Creates or truncates `path` and writes `bytes`, synced and closed checked (crate::sys::close).
    /// (Callers write a temporary file this way and rename it over the real one, so no reader sees
    /// half of it; one left by a write that never finished is simply written again.)
    /// Counted as a file made. Never retried.
    pub fn write(&self, path: &NasPath, bytes: Vec<u8>) -> Result<(), IoError> {
        let p = path.unchecked_path();
        let (n, t, q) = (bytes.len() as u64, Instant::now(), p.clone());
        self.run(&path.display(), self.inner.cfg.op_timeout, move || {
            let mut f = File::create(&p)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            crate::sys::close(f, &p)
        })?;
        crate::nasio::wrote(&q, n, t.elapsed().as_millis() as u64);
        Ok(())
    }
}

impl Drop for IoPool {
    fn drop(&mut self) {
        // (The prober notices this flag.)
        self.inner.closed.store(true, SeqCst);
    }
}

impl Inner {
    /// An operation's turn (`PoolConfig::calls` under way at most), waited for `timeout` at most:
    /// `Timeout` past it, `Offline` as the breaker opens meanwhile.
    fn turn(&self, timeout: Duration) -> Result<Turn<'_>, IoError> {
        let end = Instant::now() + timeout;
        let mut c = lock(&self.counts);
        while c.busy >= self.cfg.calls {
            if !self.online.load(SeqCst) {
                return Err(IoError::Offline);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(IoError::Timeout);
            }
            c = self.turn_cv.wait_timeout(c, left.min(POLL)).unwrap_or_else(|e| e.into_inner()).0;
        }
        c.busy += 1;
        Ok(Turn(self))
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
        // First wait, so the client's attribute cache from before the stall has a chance to expire
        // (none for a new pool's first probe: `probe_first`).
        if !inner.probe_now.swap(false, SeqCst) && !inner.sleep(inner.cfg.probe_interval) {
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
    let unique = dir.join(format!(".scenic-probe-{}-{}", crate::sys::pid(), N.fetch_add(1, SeqCst)));
    match fs::symlink_metadata(unique) {
        Ok(_) => true,
        Err(e) => matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory),
    }
}

/// Errors that mean the share itself is unreachable (what a soft mount returns once it gives up).
fn is_disconnect(e: &io::Error) -> bool {
    crate::sys::is_disconnect(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    // (The operations here that overrun are held by a gate until the test lets them go, so an
    // overrun never depends on how fast a busy Mac runs a thread; the waits in the pool are short,
    // for speed, and a call that didn't get its turn in time is told apart and made again; the
    // share's answer after an overrun is waited for as long as it takes (`alive_timeout`); and the
    // ordinary operations, whose time isn't what's tested, have minutes.)

    /// A minute, for what isn't timed.
    const LONG: Duration = Duration::from_secs(60);
    use crate::testkit::eventually;

    fn cfg(probe: PathBuf, threads: usize, timeout_ms: u64) -> PoolConfig {
        PoolConfig {
            probe_interval: Duration::from_millis(20),
            alive_timeout: Duration::from_secs(600),
            ..PoolConfig::new(threads, Duration::from_millis(timeout_ms), probe)
        }
    }

    /// Operations held in their call until the gate opens.
    #[derive(Clone, Default)]
    struct Gate {
        open: Arc<(Mutex<bool>, Condvar)>,
        started: Arc<AtomicUsize>,
    }

    impl Gate {
        fn op(&self) -> impl FnOnce() -> io::Result<()> + Send + 'static {
            let g = self.clone();
            move || {
                g.started.fetch_add(1, SeqCst);
                let (m, cv) = &*g.open;
                let mut open = lock(m);
                while !*open {
                    open = cv.wait(open).unwrap_or_else(|e| e.into_inner());
                }
                Ok(())
            }
        }

        fn release(&self) {
            *lock(&self.open.0) = true;
            self.open.1.notify_all();
        }

        fn started(&self) -> usize {
            self.started.load(SeqCst)
        }
    }

    /// A gated operation called until one overruns (a call that didn't get its turn in time is
    /// dropped unrun, and made again): its error (an overrun's is `Timeout`). One
    /// caller at a time (the pool's count of overruns tells them apart); `together` for many.
    fn overrun(pool: &IoPool, gate: &Gate) -> IoError {
        for _ in 0..10_000 {
            let before = pool.status().timeouts;
            match pool.call(gate.op()) {
                Err(IoError::Timeout) if pool.status().timeouts == before => continue,
                Err(e) => return e,
                Ok(()) => panic!("a gated operation ended"),
            }
        }
        panic!("no call had its turn in 10,000 tries");
    }

    /// `n` gated operations called at once, again for those that didn't get their turn in time, until
    /// each overran or was refused: their errors.
    fn together(pool: &Arc<IoPool>, gate: &Gate, n: usize) -> Vec<IoError> {
        let mut out = Vec::new();
        while out.len() < n {
            let calls: Vec<_> = (out.len()..n)
                .map(|_| {
                    let (p, g) = (pool.clone(), gate.clone());
                    thread::spawn(move || {
                        let before = g.started();
                        let r = p.call(g.op());
                        (before, r)
                    })
                })
                .collect();
            let results: Vec<_> = calls.into_iter().map(|h| h.join().unwrap().1).collect();
            // (Every operation that overran is running, held by the gate; one dropped unrun never
            // runs: once the gate has seen as many as overran, the rest were dropped.)
            eventually("each overrun's operation running", || gate.started() as u64 == pool.status().timeouts);
            let overran = gate.started() - out.iter().filter(|e| matches!(e, IoError::Timeout)).count();
            let mut timeouts = 0;
            for r in results {
                match r {
                    Err(IoError::Timeout) if timeouts < overran => {
                        timeouts += 1;
                        out.push(IoError::Timeout);
                    }
                    Err(IoError::Timeout) => {}
                    Err(e) => out.push(e),
                    Ok(()) => panic!("a gated operation ended"),
                }
            }
        }
        out
    }

    #[test]
    fn operations() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(PoolConfig::new(2, LONG, dir.path().to_owned()));
        assert_eq!(pool.call(|| Ok(41 + 1)).unwrap(), 42);
        let nas = crate::nasfs::NasRoot::local_for_test(dir.path());
        let p = nas.join("a.bin");
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
        assert!(!pool.exists(&nas.join("nope")).unwrap());
        fs::create_dir(dir.path().join("sub")).unwrap();
        let q = nas.join("b.bin");
        pool.rename(&p, &q).unwrap();
        let names: Vec<_> = pool.list(&nas.path()).unwrap().into_iter().map(|e| (e.name, e.is_dir, e.len)).collect();
        assert_eq!(names, [("b.bin".to_string(), false, 10), ("sub".to_string(), true, names[1].2)]);
        // A file written through a temporary one, over a stale temporary file, in a new folder.
        let deeper = nas.join("sub/deeper");
        pool.create_dir_all(&deeper).unwrap();
        pool.create_dir_all(&deeper).unwrap();
        let tmp = deeper.join("c.bin.tmp");
        pool.write(&tmp, b"stale, and longer".to_vec()).unwrap();
        pool.write(&tmp, b"new".to_vec()).unwrap();
        pool.rename(&tmp, &deeper.join("c.bin")).unwrap();
        assert_eq!(deeper.join("c.bin").read().unwrap(), b"new");
        assert!(!tmp.exists());
        // An ordinary error leaves the NAS online.
        assert!(
            matches!(pool.read_all(&nas.join("missing")), Err(IoError::Io(e)) if e.kind() == io::ErrorKind::NotFound)
        );
        assert!(pool.is_online());
        // A panicking operation is an error, and the pool goes on.
        assert!(matches!(pool.call(|| -> io::Result<()> { panic!("boom") }), Err(IoError::Io(_))));
        assert_eq!(pool.call(|| Ok(1)).unwrap(), 1);
        assert_eq!(pool.status().stuck, 0);
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

        // The share "disappears" and an operation stalls: the caller gets its timeout while the
        // operation is still in the kernel.
        fs::remove_dir(&probe).unwrap();
        let gate = Gate::default();
        assert!(matches!(overrun(&pool, &gate), IoError::Timeout));
        assert!(!pool.is_online());
        let st = pool.status();
        assert_eq!((st.stuck, st.timeouts), (1, 1));
        assert!(st.offline_since.is_some());

        // Offline: refused at once, without waiting (a call that waited for its turn would wait
        // its whole timeout, here ten minutes).
        let t0 = Instant::now();
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        assert!(matches!(
            pool.call_timeout(Duration::from_secs(600), move || Ok(r2.store(true, SeqCst))),
            Err(IoError::Offline)
        ));
        assert!(matches!(
            pool.read_all(&crate::nasfs::NasRoot::local_for_test(dir.path()).join("x")),
            Err(IoError::Offline)
        ));
        assert!(t0.elapsed() < Duration::from_secs(600) && !ran.load(SeqCst));

        // The probe keeps failing while the share is away.
        thread::sleep(Duration::from_millis(200)); // (absence)
        assert!(!pool.is_online());

        // It comes back: the prober closes the breaker.
        fs::create_dir(&probe).unwrap();
        eventually("the breaker closed", || pool.is_online());
        assert_eq!(pool.call_timeout(LONG, || Ok(7)).unwrap(), 7);
        assert_eq!(*events.lock().unwrap(), [false, true]);
        assert!(pool.status().offline_since.is_none());

        // The stuck operation returns.
        gate.release();
        eventually("the stuck operation back", || pool.status().stuck == 0);
    }

    #[test]
    fn slow_share_that_answers_isnt_offline() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 2, 100));
        // An operation overruns while the share still answers probes: busy, not gone.
        let gate = Gate::default();
        let r = overrun(&pool, &gate);
        assert!(matches!(r, IoError::Timeout), "{r:?}");
        assert!(pool.is_online());
        let st = pool.status();
        assert_eq!((st.stuck, st.timeouts), (1, 1));
        assert!(st.offline_since.is_none());
        // Other calls go on meanwhile, and the slow operation ends.
        assert_eq!(pool.call_timeout(LONG, || Ok(5)).unwrap(), 5);
        gate.release();
        eventually("the slow thread back", || pool.status().stuck == 0);
    }

    #[test]
    fn many_overruns_on_a_busy_share_make_one_probe_and_no_trip() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 8, 100));
        // Eight reads started together all overrun; the share answers probes.
        let gate = Gate::default();
        for e in together(&pool, &gate, 8) {
            assert!(matches!(e, IoError::Timeout), "{e:?}");
        }
        assert!(pool.is_online(), "a busy share isn't offline");
        assert_eq!(pool.status().timeouts, 8);
        gate.release();
        eventually("the slow threads back", || pool.status().stuck == 0);
    }

    #[test]
    fn overruns_on_a_gone_share_trip() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join("share");
        fs::create_dir(&probe).unwrap();
        let pool =
            IoPool::with_config(PoolConfig { probe_interval: Duration::from_secs(60), ..cfg(probe.clone(), 4, 100) });
        fs::remove_dir(&probe).unwrap();
        // Reads overrun: the first to find the share gone trips the breaker; any call after that is
        // refused.
        let gate = Gate::default();
        for e in together(&pool, &gate, 4) {
            assert!(matches!(e, IoError::Timeout | IoError::Offline), "{e:?}");
        }
        assert!(pool.status().timeouts >= 1);
        assert!(!pool.is_online());
        gate.release();
    }

    /// A pool probed first (a share that may be away) refuses calls until its share answers the
    /// probe, made at once: the interval here is one no test waits out, so only that probe can
    /// bring it online. A share that doesn't answer leaves it refusing every call at once.
    #[test]
    fn a_pool_probed_first_is_read_once_its_share_answers() {
        let dir = tempfile::tempdir().unwrap();
        let never = Duration::from_secs(24 * 3600);
        let pool = IoPool::with_config(PoolConfig { probe_interval: never, ..cfg(dir.path().to_owned(), 1, 60_000) });
        pool.probe_first();
        eventually("the first probe answered", || pool.is_online());
        assert!(pool.call(|| Ok(())).is_ok());
        let away = IoPool::with_config(PoolConfig { probe_interval: never, ..cfg(dir.path().join("away"), 1, 60_000) });
        away.probe_first();
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        let r = away.call(move || {
            r2.store(true, SeqCst);
            Ok(())
        });
        assert!(matches!(r, Err(IoError::Offline)), "{r:?}");
        assert!(!ran.load(SeqCst) && !away.is_online());
    }

    /// Operations stuck in the kernel are bounded by the process's cap (crate::deadline): at it, a
    /// call is refused at once, unrun, as `Timeout`, and no overrun is counted; once they return,
    /// work resumes.
    #[test]
    fn stuck_operations_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 2, 50));
        let _cap = crate::deadline::cap_for_test(2);
        let gate = Gate::default();
        while pool.status().stuck < 2 {
            let r = overrun(&pool, &gate);
            assert!(matches!(r, IoError::Timeout), "{r:?}");
        }
        assert!(pool.is_online());
        let ran = Arc::new(AtomicBool::new(false));
        for _ in 0..3 {
            let r2 = ran.clone();
            let t = Instant::now();
            let r = pool.call_timeout(LONG, move || {
                r2.store(true, SeqCst);
                Ok(())
            });
            assert!(matches!(r, Err(IoError::Timeout)), "{r:?}");
            crate::testkit::ended_before(t, LONG, "refused at once");
        }
        let s = pool.status();
        assert_eq!((s.stuck, s.timeouts), (2, 2), "{s:?}");
        gate.release();
        eventually("the stuck operations back", || pool.status().stuck == 0);
        assert!(pool.is_online());
        assert_eq!(pool.call_timeout(LONG, || Ok(3)).unwrap(), 3);
        assert!(!ran.load(SeqCst), "a refused operation ran");
    }

    #[test]
    fn busy_pool_times_out_without_tripping() {
        let dir = tempfile::tempdir().unwrap();
        let pool = IoPool::with_config(cfg(dir.path().to_owned(), 1, 1000));
        // Its one turn taken (by a call with a long timeout of its own)...
        let gate = Gate::default();
        let (p2, g2) = (pool.clone(), gate.clone());
        let long = thread::spawn(move || p2.call_timeout(LONG, g2.op()));
        eventually("the long operation started", || gate.started() == 1);
        // ...a call waits for it in vain, and times out unrun.
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        let r = pool.call_timeout(Duration::from_millis(100), move || {
            r2.store(true, SeqCst);
            Ok(())
        });
        assert!(matches!(r, Err(IoError::Timeout)), "{r:?}");
        assert!(pool.is_online(), "a queue wait must not mark the NAS offline");
        gate.release();
        long.join().unwrap().unwrap();
        thread::sleep(Duration::from_millis(50)); // (absence)
        assert!(!ran.load(SeqCst));
        assert_eq!(pool.status().timeouts, 0);
    }

    #[test]
    fn network_errors_and_vanished_share_trip() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join("share");
        fs::create_dir(&probe).unwrap();
        let cfg = || PoolConfig { probe_interval: Duration::from_secs(60), ..PoolConfig::new(1, LONG, probe.clone()) };
        let pool = IoPool::with_config(cfg());
        let r = pool.call(|| -> io::Result<()> { Err(io::Error::from_raw_os_error(libc::ETIMEDOUT)) });
        assert!(matches!(r, Err(IoError::Io(_))));
        assert!(!pool.is_online());

        let pool = IoPool::with_config(cfg());
        fs::remove_dir(&probe).unwrap();
        assert!(matches!(
            pool.stat(&crate::nasfs::NasRoot::local_for_test(&probe).join("file")),
            Err(IoError::Offline)
        ));
        assert!(!pool.is_online());

        let pool = IoPool::with_config(cfg());
        assert!(matches!(
            pool.exists(&crate::nasfs::NasRoot::local_for_test(&probe).join("file")),
            Err(IoError::Offline)
        ));
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
