//! Riding out a short NAS outage (task #656, docs/pool.md §3): a build job's NAS read that fails
//! with an error the share gives for a moment (its SMB session dropped and reconnecting, the NAS
//! restarting) is tried again after each of `WAITS`, ~3.5 min in all, each retry logged, instead of
//! failing the job and the hour of work behind it. Before the last try the share's mount is looked
//! at (the kernel's table, then a stat under a deadline); a share that's gone is waited for a
//! little (the agent's self-heal, crate::agent::heal, remounts it; a job never does). Past the
//! bound the job fails with the original error, marked `Exhausted` so an outer retry doesn't start
//! over.
//!
//! No retry storms (docs/macs.md, the NAS's small-file rate): each operation backs off on its own
//! schedule, and a process's retries run one at a time (`LANE`): while the NAS is away, a job with
//! many reads in flight tries one of them at a time.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// The waits between tries, in seconds (211 s in all).
pub const WAITS: [u64; 11] = [1, 2, 4, 8, 16, 30, 30, 30, 30, 30, 30];

/// How long a share found gone before the last try is waited for, at most (seconds; looked at
/// every 10 s).
const SHARE_WAIT: u64 = 60;

/// Milliseconds a second of `WAITS` lasts (tests make it 1: `fast`).
static UNIT_MS: AtomicU64 = AtomicU64::new(1000);

/// A process's retries, one at a time.
static LANE: Mutex<()> = Mutex::new(());

thread_local! {
    /// Whether this thread runs a try of an outer retry (`Attempt`).
    static IN_ATTEMPT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A try of a retry under way on this thread (or on a worker it waits on, which takes one of its
/// own, `attempt`): a retry inside it tries once and leaves the waits to the outer one. Nested,
/// the inner one would wait on `LANE`, which the outer try holds: forever on the same thread (a
/// std mutex isn't reentrant), and past the outer's deadline on a worker (review of #663: a store
/// tile's retry around `NasPath::open`, a pack open's `nas_call`, bldprep's footers).
pub struct Attempt(bool);

/// Marks this thread as running an outer retry's try until dropped (a worker thread an outer try
/// waits on takes one when `in_attempt()` was true where it was spawned).
pub fn attempt() -> Attempt {
    Attempt(IN_ATTEMPT.with(|a| a.replace(true)))
}

impl Drop for Attempt {
    fn drop(&mut self) {
        IN_ATTEMPT.with(|a| a.set(self.0));
    }
}

/// Whether this thread runs an outer retry's try.
pub fn in_attempt() -> bool {
    IN_ATTEMPT.with(|a| a.get())
}

/// The waits in this process (`WAITS`, scaled by `fast`).
pub fn waits() -> Vec<Duration> {
    let u = UNIT_MS.load(Ordering::Relaxed);
    WAITS.iter().map(|s| Duration::from_millis(s * u)).collect()
}

/// The server's waits between tries, in seconds (5 s in all, decision D3 of #663: an HTTP worker
/// never stalls for minutes).
pub const SHORT_WAITS: [u64; 4] = [1, 1, 1, 2];

/// `SHORT_WAITS` in this process (scaled by `fast`).
pub fn short_waits() -> Vec<Duration> {
    let u = UNIT_MS.load(Ordering::Relaxed);
    SHORT_WAITS.iter().map(|s| Duration::from_millis(s * u)).collect()
}

/// Makes this process's waits milliseconds (tests).
pub fn fast() {
    UNIT_MS.store(1, Ordering::Relaxed);
}

/// The last error of an operation whose retries ran out (its message the original's): never
/// transient, so a retry around it doesn't start over.
#[derive(Debug)]
pub struct Exhausted {
    pub msg: String,
    pub code: Option<i32>,
}

impl std::fmt::Display for Exhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Exhausted {}

fn exhausted(e: io::Error) -> io::Error {
    let msg = e.to_string();
    io::Error::new(e.kind(), Exhausted { msg, code: e.raw_os_error() })
}

/// Whether an error is one the share gives for a moment: EIO, ENXIO ("Device not configured", a
/// dropped SMB session), ETIMEDOUT and the other disconnects (crate::sys::is_disconnect), EAGAIN,
/// ESTALE, EINTR, a reset or broken connection. Not a "not found" (`transient_or_missing`), not
/// a deadline a caller set (a `TimedOut` with no OS code), not `Exhausted`.
pub fn transient(e: &io::Error) -> bool {
    use io::ErrorKind as K;
    if e.get_ref().is_some_and(|r| r.is::<Exhausted>()) {
        return false;
    }
    crate::sys::is_disconnect(e)
        || matches!(e.raw_os_error(), Some(libc::EIO | libc::ENXIO | libc::ESTALE | libc::EAGAIN | libc::EINTR))
        || matches!(
            e.kind(),
            K::Interrupted
                | K::WouldBlock
                | K::ConnectionReset
                | K::ConnectionAborted
                | K::NotConnected
                | K::BrokenPipe
        )
}

/// `transient`, or a "not found" (for a file known to be there: SMB's moment of ENOENT on a file
/// that's there, a file open a moment ago).
pub fn transient_or_missing(e: &io::Error) -> bool {
    transient(e) || (e.kind() == io::ErrorKind::NotFound && !e.get_ref().is_some_and(|r| r.is::<Exhausted>()))
}

/// The I/O error an `anyhow` error holds (through its context, a pool's `IoError::Io`, or a
/// parquet or other error that names its OS code, "(os error N)"), as one of its code and kind;
/// `Exhausted` kept. None: no I/O error in it.
pub fn io_of(e: &anyhow::Error) -> Option<io::Error> {
    for c in e.chain() {
        let i = c.downcast_ref::<io::Error>().or_else(|| match c.downcast_ref::<crate::IoError>() {
            Some(crate::IoError::Io(i)) => Some(i),
            _ => None,
        });
        if let Some(i) = i {
            if let Some(x) = i.get_ref().and_then(|r| r.downcast_ref::<Exhausted>()) {
                return Some(io::Error::new(i.kind(), Exhausted { msg: x.msg.clone(), code: x.code }));
            }
            return Some(match i.raw_os_error() {
                Some(code) => io::Error::from_raw_os_error(code),
                None => io::Error::new(i.kind(), format!("{e:#}")),
            });
        }
        if let Some(x) = c.downcast_ref::<Exhausted>() {
            return Some(io::Error::other(Exhausted { msg: x.msg.clone(), code: x.code }));
        }
    }
    let s = format!("{e:#}");
    let at = s.rfind("(os error ")?;
    let code: i32 = s[at + 10..].split(')').next()?.parse().ok()?;
    Some(io::Error::from_raw_os_error(code))
}

/// An error as an I/O error of its kind (`io_of`; none in it, `InvalidData`: never transient).
pub fn as_io(e: anyhow::Error) -> io::Error {
    io_of(&e).unwrap_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")))
}

/// `f()`, tried again after each of `waits` while it fails as `again` says (`transient`,
/// `transient_or_missing`), each retry logged with `what`; the share holding `path` looked at
/// before the last try. Past the waits, the last error, `Exhausted`.
pub fn io<T>(
    what: &str,
    path: Option<&Path>,
    waits: &[Duration],
    again: fn(&io::Error) -> bool,
    mut f: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    if in_attempt() {
        return f();
    }
    let mut tries = 0;
    let mut r = {
        let _a = attempt();
        f()
    };
    loop {
        let e = match r {
            Ok(t) => return Ok(t),
            Err(e) if again(&e) => e,
            Err(e) => return Err(e),
        };
        let Some(&w) = waits.get(tries) else { return Err(exhausted(e)) };
        tries += 1;
        let last = tries == waits.len();
        eprintln!("{what}: {e}; try {} of {} in {w:?}", tries + 1, waits.len() + 1);
        std::thread::sleep(w);
        if last {
            if let Some(p) = path {
                share_check(what, p, w);
            }
        }
        let _lane = LANE.lock().unwrap_or_else(|p| p.into_inner());
        let _a = attempt();
        r = f();
    }
}

/// `io` for an operation whose errors are `anyhow`'s (classified by `io_of`); past the waits, the
/// last error with an `Exhausted` in its chain.
pub fn any<T>(
    what: &str,
    path: Option<&Path>,
    waits: &[Duration],
    again: fn(&io::Error) -> bool,
    f: impl FnMut() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    any_until(what, path, waits, again, || false, f)
}

/// `any`, given up the moment `stop()` says so (looked at before each try and through each wait,
/// at least every `STOP_EVERY`): its last error then, not `Exhausted` (a job pausing stops its
/// retries at once, task #903: the caller says what the stop means).
pub fn any_until<T>(
    what: &str,
    path: Option<&Path>,
    waits: &[Duration],
    again: fn(&io::Error) -> bool,
    stop: fn() -> bool,
    mut f: impl FnMut() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    if in_attempt() {
        return f();
    }
    let mut tries = 0;
    let mut r = {
        let _a = attempt();
        f()
    };
    loop {
        let e = match r {
            Ok(t) => return Ok(t),
            Err(e) if io_of(&e).is_some_and(|i| again(&i)) => e,
            Err(e) => return Err(e),
        };
        let Some(&w) = waits.get(tries) else {
            let i = io_of(&e).unwrap_or_else(|| io::Error::other(""));
            let x = Exhausted { msg: format!("{e:#}"), code: i.raw_os_error() };
            return Err(io::Error::new(i.kind(), x).into());
        };
        tries += 1;
        let last = tries == waits.len();
        if stop() {
            return Err(e);
        }
        eprintln!("{what}: {e:#}; try {} of {} in {w:?}", tries + 1, waits.len() + 1);
        let until = std::time::Instant::now() + w;
        while let Some(left) = until.checked_duration_since(std::time::Instant::now()).filter(|d| !d.is_zero()) {
            std::thread::sleep(left.min(STOP_EVERY));
            if stop() {
                return Err(e);
            }
        }
        if last {
            if let Some(p) = path {
                share_check(what, p, w);
            }
        }
        let _lane = LANE.lock().unwrap_or_else(|p| p.into_inner());
        let _a = attempt();
        r = f();
    }
}

/// How often `any_until` looks at its stop while it waits.
const STOP_EVERY: Duration = Duration::from_millis(500);

/// Before an operation's last try on `p`: whether the NAS share is mounted (the kernel's table,
/// which never blocks) and answers (a stat of its mount point under a deadline). Gone, it's waited
/// for, `SHARE_WAIT` at most; a path not on the share isn't looked at.
fn share_check(what: &str, p: &Path, every: Duration) {
    let Some(m) = crate::nas::find_mount(crate::nas::HOST, crate::nas::SHARE) else {
        if !p.starts_with("/Volumes") {
            return;
        }
        eprintln!("{what}: the NAS share isn't mounted; waiting for the agent to mount it again");
        return wait_share(what, every);
    };
    if !p.starts_with(&m.point) {
        return;
    }
    if !answers(&m.point, every.max(Duration::from_millis(10))) {
        eprintln!("{what}: the NAS share ({}) doesn't answer; waiting for it", m.point.display());
        wait_share(what, every);
    }
}

fn wait_share(what: &str, every: Duration) {
    let every = every.min(Duration::from_secs(10));
    for _ in 0..SHARE_WAIT / 10 {
        std::thread::sleep(every);
        if let Some(m) = crate::nas::find_mount(crate::nas::HOST, crate::nas::SHARE) {
            if answers(&m.point, every) {
                eprintln!("{what}: the NAS share is back");
                return;
            }
        }
    }
    eprintln!("{what}: the NAS share isn't back; the last try anyway");
}

/// Whether a stat of `dir` answers within `deadline` (its thread left behind when it doesn't).
fn answers(dir: &Path, deadline: Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let d = dir.to_path_buf();
    let spawned = std::thread::Builder::new().name("share-check".into()).spawn(move || {
        let _ = tx.send(std::fs::metadata(&d).is_ok());
    });
    spawned.is_ok() && rx.recv_timeout(deadline).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn ms(n: u64) -> Vec<Duration> {
        vec![Duration::from_millis(1); n as usize]
    }

    #[test]
    fn each_transient_errno_is_ridden_out() {
        for code in [libc::EIO, libc::ENXIO, libc::ETIMEDOUT, libc::ENOTCONN, libc::EAGAIN, libc::ESTALE] {
            let n = Cell::new(0);
            let r = io("t", None, &ms(4), transient, || {
                n.set(n.get() + 1);
                if n.get() <= 3 {
                    Err(io::Error::from_raw_os_error(code))
                } else {
                    Ok(7)
                }
            });
            assert_eq!(r.unwrap(), 7, "errno {code}");
            assert_eq!(n.get(), 4);
        }
    }

    #[test]
    fn past_the_bound_the_original_error() {
        let n = Cell::new(0);
        let e = io("t", None, &ms(3), transient, || -> io::Result<()> {
            n.set(n.get() + 1);
            Err(io::Error::from_raw_os_error(libc::ENXIO))
        })
        .unwrap_err();
        assert_eq!(n.get(), 4);
        assert_eq!(e.to_string(), io::Error::from_raw_os_error(libc::ENXIO).to_string());
        assert!(!transient(&e) && !transient_or_missing(&e), "an outer retry doesn't start over");
    }

    #[test]
    fn missing_and_other_errors_fail_at_once() {
        for e in [
            io::Error::from_raw_os_error(libc::ENOENT),
            io::Error::new(io::ErrorKind::InvalidData, "not a TIFF"),
            io::Error::new(io::ErrorKind::TimedOut, "over 120 s"),
        ] {
            let n = Cell::new(0);
            let msg = e.to_string();
            let mut e = Some(e);
            let r: io::Result<()> = io("t", None, &ms(3), transient, || {
                n.set(n.get() + 1);
                Err(e.take().unwrap())
            });
            assert_eq!(r.unwrap_err().to_string(), msg);
            assert_eq!(n.get(), 1);
        }
        assert!(transient_or_missing(&io::Error::from_raw_os_error(libc::ENOENT)));
    }

    /// A retry inside another's try tries once: no wait on the lane the outer try holds (a hang
    /// on the same thread), no second schedule (on a worker the outer try waits on, past its
    /// deadline).
    #[test]
    fn a_retry_inside_a_retry_tries_once() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (outer, inner) = (Cell::new(0), Cell::new(0));
            let r = io("outer", None, &ms(3), transient_or_missing, || {
                outer.set(outer.get() + 1);
                // (The first try missing, the rest failing as the share does for a moment.)
                if outer.get() == 1 {
                    return Err(io::Error::from_raw_os_error(libc::ENOENT));
                }
                io("inner", None, &ms(3), transient, || {
                    inner.set(inner.get() + 1);
                    if inner.get() < 3 {
                        Err(io::Error::from_raw_os_error(libc::EIO))
                    } else {
                        Ok(5)
                    }
                })
            });
            let worker = io("outer", None, &ms(2), transient, || {
                let nested = in_attempt();
                std::thread::spawn(move || {
                    let _a = nested.then(attempt);
                    io("inner", None, &ms(5), transient, || -> io::Result<()> {
                        Err(io::Error::from_raw_os_error(libc::EIO))
                    })
                })
                .join()
                .unwrap()
            });
            let _ = tx.send((r.map_err(|e| e.to_string()), outer.get(), inner.get(), worker.is_err()));
        });
        let got = rx.recv_timeout(crate::testkit::WATCHDOG).expect("a nested retry hangs");
        assert_eq!(got, (Ok(5), 4, 3, true));
        assert!(!in_attempt());
    }

    #[test]
    fn anyhow_errors_by_their_os_code() {
        let n = Cell::new(0);
        let r = any("t", None, &ms(3), transient, || {
            n.set(n.get() + 1);
            if n.get() < 3 {
                Err(anyhow::anyhow!("External: Device not configured (os error 6)").context("read row group"))
            } else {
                Ok(1)
            }
        });
        assert_eq!(r.unwrap(), 1);
        let e = any("t", None, &ms(2), transient, || -> anyhow::Result<()> {
            Err(anyhow::Error::from(io::Error::from_raw_os_error(libc::EIO)).context("TIFF block 3,2"))
        })
        .unwrap_err();
        assert!(format!("{e:#}").contains("TIFF block 3,2"));
        assert!(!io_of(&e).is_some_and(|i| transient(&i)), "marked exhausted");
        let e = any("t", None, &ms(2), transient, || -> anyhow::Result<()> { anyhow::bail!("cut short") }).unwrap_err();
        assert_eq!(e.to_string(), "cut short");
    }

    /// Task #903: a stop ends the retries at once, mid-wait, with the last error (not Exhausted).
    #[test]
    fn a_stop_ends_the_waits_at_once() {
        static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let tries = Cell::new(0);
        let t = std::time::Instant::now();
        let e = any_until(
            "t",
            None,
            &[Duration::from_secs(60)],
            transient,
            || STOP.load(Ordering::Relaxed),
            || -> anyhow::Result<()> {
                tries.set(tries.get() + 1);
                STOP.store(true, Ordering::Relaxed);
                Err(io::Error::from_raw_os_error(libc::ENXIO).into())
            },
        )
        .unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(30) && tries.get() == 1); // (bound ok: a 60 s wait cut short)
        assert!(io_of(&e).is_some_and(|i| transient(&i)), "not marked exhausted");
    }
}
