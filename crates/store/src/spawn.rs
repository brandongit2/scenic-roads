//! Starting children, and making and closing sockets, so that no child ever holds a socket of
//! this process (docs/pool.md §7.5).
//!
//! On macOS a socket is made in one call and marked close-on-exec in a second (std's `bind` and
//! `accept`, mio's `accept`: Darwin has no `SOCK_CLOEXEC`). A child started between the two keeps
//! the socket for as long as it lives: a coordinator's listener, or a connection it took, outlived
//! its `stop`, and the next coordinator's listen on the port was refused (EADDRINUSE). A child
//! started by fork and exec (std's way when the child's PATH differs from this process's) holds
//! every socket of this process, close-on-exec or not, until its exec: a socket closed then stays
//! open until it does.
//!
//! A child's pipes have the same gap: std makes them (`Stdio::piped`, and the fork path's pipe
//! for a failed exec's error) with `pipe`, then marks each end close-on-exec (Darwin has no
//! `pipe2`). A child started by another thread between the two keeps both ends: two
//! `caffeinate -w` children held a `scutil` child's output pipe for their whole lives, so its
//! reader never saw the end and a test hung for 80 minutes (task #900).
//!
//! So a child is started (`spawn`, `status`, `output`, `detached`) only while no other child is
//! being started and no listener or connection is being made or closed (`sockets`): one lock,
//! held by each start and by the sockets' few moments, one at a time. Every start in the workspace
//! goes through here (clippy.toml's disallowed-methods, checked by tools/app/publish.sh).
//!
//! A child that outlives what started it, and needs nothing of it (caffeinate), is started by
//! `detached`: posix_spawn with every descriptor but its three closed
//! (`POSIX_SPAWN_CLOEXEC_DEFAULT`), those three /dev/null, so it holds nothing of this process's
//! whatever was open, or half-marked, at its start. (std's `Child` can't be had from one: a
//! `Detached` is its own.)
//!
//! No start leaves a child holding this process's files once it returns (store::cachefile's
//! deletes count on it: `no_child_starting`). Only std's posix_spawn of a program by its path is
//! such a start (measured: a lock let go of, then taken exclusively, while other threads start
//! children). A posix_spawn whose exec fails returns while the half-made child, with copies of
//! every descriptor and the `flock`s on them, is still being torn down; posix_spawnp, std's way
//! for a program named without a folder, makes one such failed exec for each PATH folder before
//! the program's own; and std's fork and exec (a PATH of the child's own, `pre_exec`) returns once
//! the exec has closed the descriptor it waits on, the child's others maybe not yet. So a program
//! is named by its path (`command`, `command_in`: looked up here, in PATH), and `spawn` refuses
//! one named without a folder, or by a path that isn't there, without a start.
//!
//! Sockets that hold no port of a listener here, an outgoing connection's (ureq's), aren't made
//! under the lock: a child holding one keeps a connection open, never a port from being listened
//! on. Files are opened close-on-exec in the one call (`O_CLOEXEC`): no gap.
#![allow(clippy::disallowed_methods)]

use std::ffi::OsStr;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Mutex, MutexGuard};

static GAP: Mutex<()> = Mutex::new(());

fn gap() -> MutexGuard<'static, ()> {
    GAP.lock().unwrap_or_else(|e| e.into_inner())
}

/// `cmd.spawn()`, while no other child is being started and no socket is being made or closed.
pub fn spawn(cmd: &mut Command) -> io::Result<Child> {
    settle(cmd)?;
    let _g = gap();
    cmd.spawn()
}

/// A child started by `detached`: no pipes, no descriptor of this process's. Waited for (reaped)
/// by `wait`, `kill` or a `try_wait` that finds it ended.
#[derive(Debug)]
#[cfg_attr(not(unix), allow(dead_code))]
pub struct Detached {
    pid: i32,
    done: bool,
}

/// Starts `program` (a path, as `command` finds it) with `args`, its standard streams /dev/null and
/// every other descriptor closed (the module's note), its environment this process's with
/// `NO_LARGE_CACHE`.
#[cfg(unix)]
pub fn detached(program: impl AsRef<OsStr>, args: &[&str]) -> io::Result<Detached> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let cmd = command(program);
    settle(&cmd)?;
    let cs = |b: &[u8]| CString::new(b).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e));
    let prog = cs(cmd.get_program().as_bytes())?;
    let argv: Vec<CString> =
        std::iter::once(Ok(prog.clone())).chain(args.iter().map(|a| cs(a.as_bytes()))).collect::<io::Result<_>>()?;
    let envv: Vec<CString> = std::env::vars_os()
        .filter(|(k, _)| k != NO_LARGE_CACHE.0)
        .map(|(k, v)| [k.as_bytes(), b"=", v.as_bytes()].concat())
        .chain(std::iter::once(format!("{}={}", NO_LARGE_CACHE.0, NO_LARGE_CACHE.1).into_bytes()))
        .map(|b| cs(&b))
        .collect::<io::Result<_>>()?;
    let ptrs = |v: &[CString]| {
        v.iter().map(|c| c.as_ptr() as *mut libc::c_char).chain([std::ptr::null_mut()]).collect::<Vec<_>>()
    };
    let (argp, envp) = (ptrs(&argv), ptrs(&envv));
    let null = c"/dev/null";
    let check = |r: libc::c_int| if r == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(r)) };
    // SAFETY: attr and actions initialised before use and destroyed after; argv and envp are
    // NUL-terminated arrays of NUL-terminated strings that outlive the call.
    unsafe {
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        let mut acts: libc::posix_spawn_file_actions_t = std::mem::zeroed();
        check(libc::posix_spawnattr_init(&mut attr))?;
        check(libc::posix_spawn_file_actions_init(&mut acts))?;
        let r = (|| {
            #[cfg(target_os = "macos")]
            check(libc::posix_spawnattr_setflags(&mut attr, libc::POSIX_SPAWN_CLOEXEC_DEFAULT as libc::c_short))?;
            check(libc::posix_spawn_file_actions_addopen(&mut acts, 0, null.as_ptr(), libc::O_RDONLY, 0))?;
            check(libc::posix_spawn_file_actions_addopen(&mut acts, 1, null.as_ptr(), libc::O_WRONLY, 0))?;
            check(libc::posix_spawn_file_actions_addopen(&mut acts, 2, null.as_ptr(), libc::O_WRONLY, 0))?;
            let mut pid = 0;
            let _g = gap();
            check(libc::posix_spawn(&mut pid, prog.as_ptr(), &acts, &attr, argp.as_ptr(), envp.as_ptr()))?;
            Ok(Detached { pid, done: false })
        })();
        libc::posix_spawn_file_actions_destroy(&mut acts);
        libc::posix_spawnattr_destroy(&mut attr);
        r
    }
}

/// No children where there are no processes (WASI).
#[cfg(not(unix))]
pub fn detached(_program: impl AsRef<OsStr>, _args: &[&str]) -> io::Result<Detached> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(not(unix))]
impl Detached {
    pub fn try_wait(&mut self) -> io::Result<bool> {
        Ok(true)
    }

    pub fn kill(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
impl Detached {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    fn reap(&mut self, flags: libc::c_int) -> io::Result<bool> {
        if self.done {
            return Ok(true);
        }
        let mut st = 0;
        // SAFETY: waitpid on our own child.
        let r = unsafe { libc::waitpid(self.pid, &mut st, flags) };
        if r < 0 {
            return Err(io::Error::last_os_error());
        }
        self.done = r == self.pid;
        Ok(self.done)
    }

    /// Whether it has ended (reaped if so), as `Child::try_wait`'s `Some`.
    pub fn try_wait(&mut self) -> io::Result<bool> {
        self.reap(libc::WNOHANG)
    }

    /// Waits for its end.
    pub fn wait(&mut self) -> io::Result<()> {
        self.reap(0).map(drop)
    }

    /// SIGKILL, then waited for.
    pub fn kill(&mut self) -> io::Result<()> {
        if !self.done {
            // SAFETY: a signal to our own unreaped child (its pid can't be reused yet).
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
        }
        self.wait()
    }
}

/// A command for `program`, named by its path: a name without a folder looked up in this
/// process's PATH (the module's note). One not found keeps its name, and `spawn` refuses it. The
/// lookup is done here, once: a PATH the child is given later (`.env("PATH", ..)`) doesn't change
/// which program it is; a child given a PATH of its own is made by `command_in` with that PATH.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    command_in(program, &std::env::var_os("PATH").unwrap_or_default())
}

/// `command`, the name looked up in `path` (a PATH): for a child given that PATH. Every child is
/// started through here is started without macOS malloc's cache of freed large blocks
/// (`NO_LARGE_CACHE`).
pub fn command_in(program: impl AsRef<OsStr>, path: &OsStr) -> Command {
    let mut c = Command::new(find(program.as_ref(), path));
    c.env(NO_LARGE_CACHE.0, NO_LARGE_CACHE.1);
    c
}

/// The environment setting (libmalloc's, undocumented: `man malloc` doesn't name it) that turns off
/// macOS malloc's cache of freed large blocks: with it on, a
/// process's memory never comes down from its most (freed blocks stay in its footprint, which the
/// memory guard, the jobs' measured peaks and macOS's own pressure count), so a unit job's
/// scenic-build, after its DEM cache slice and staging, held 6–7 GB of freed blocks while its
/// programs ran (measured on unit 6/17/24 and four other dense units). Its children inherit it.
pub const NO_LARGE_CACHE: (&str, &str) = ("MallocLargeCache", "0");

/// `program`'s path: a name without a folder, the first executable file of that name in `path`'s
/// absolute folders; else as given. (An empty or relative entry is passed over, never taken as
/// the current folder: a program is started by an absolute path.)
fn find(program: &OsStr, path: &OsStr) -> PathBuf {
    let p = Path::new(program);
    if p.components().count() == 1 && !p.is_absolute() {
        for d in std::env::split_paths(path).filter(|d| d.is_absolute()) {
            let c = d.join(p);
            if executable(&c) {
                return c;
            }
        }
    }
    p.to_path_buf()
}

/// Whether `p` is a file this process may run (through links; `access`'s X_OK for its user).
#[cfg(unix)]
fn executable(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
    // SAFETY: access on a NUL-terminated path.
    p.is_file() && unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0
}

#[cfg(not(unix))]
fn executable(p: &Path) -> bool {
    p.is_file()
}

/// Refuses, without a start, a command whose exec would fail or be looked up (the module's note):
/// a program not named by an absolute path (a name `command` didn't find, or a relative path), or
/// named by one that isn't an executable file.
fn settle(cmd: &Command) -> io::Result<()> {
    if cfg!(unix) {
        let prog = Path::new(cmd.get_program());
        if !prog.is_absolute() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("{}: not found in PATH, or not an absolute path (a program is started by its path: store::spawn::command)", prog.display())));
        }
        if !executable(prog) {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("{}: no such program", prog.display())));
        }
    }
    Ok(())
}

/// `cmd.status()`: started by `spawn` (its standard streams this process's unless set), waited for.
pub fn status(cmd: &mut Command) -> io::Result<ExitStatus> {
    spawn(cmd)?.wait()
}

/// `cmd.output()`: started by `spawn`, its standard input nothing and its output and errors read
/// (std's own, which a caller's settings of them would override: these do).
pub fn output(cmd: &mut Command) -> io::Result<Output> {
    spawn(cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()))?.wait_with_output()
}

/// Runs `f`, which makes or closes sockets, while no child is being started. Brief: it waits
/// for nothing but the calls (no blocking accept, no connect, no lookup by name).
pub fn sockets<T>(f: impl FnOnce() -> T) -> T {
    let _g = gap();
    f()
}

/// Runs `f` once no child of this process is part way started, and starts none until it's done:
/// a child started by fork and exec holds copies of this process's open files, their `flock`s
/// with them, until its exec, so a lock this process let go of would still look taken for that
/// moment (store::cachefile's `try_remove`, which would pass over a file no one uses).
/// `f` must be brief (it holds back every start, and `sockets`, which share the lock) and must
/// not call `spawn`, `status`, `output`, `detached`, `sockets` or this again: the lock isn't re-entrant.
pub fn no_child_starting<T>(f: impl FnOnce() -> T) -> T {
    let _g = gap();
    f()
}

/// A listener on `addr` (an address, not a name to look up), made by `sockets`.
pub fn listen(addr: impl ToSocketAddrs) -> io::Result<TcpListener> {
    sockets(|| TcpListener::bind(addr))
}

/// A connection `listener` has waiting, taken by `sockets`; `listener` doesn't block (an accept
/// that waited would hold every start back): one that would is refused.
pub fn accept(listener: &TcpListener) -> io::Result<(TcpStream, SocketAddr)> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: fcntl F_GETFL on a descriptor `listener` owns.
        let flags = unsafe { libc::fcntl(listener.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if flags & libc::O_NONBLOCK == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "accept on a blocking listener would hold every child's start back",
            ));
        }
    }
    sockets(|| listener.accept())
}

/// Whether a child could be started now (none held back by `sockets`). For tests.
#[doc(hidden)]
pub fn could_start() -> bool {
    GAP.try_lock().is_ok()
}

#[cfg(all(test, unix))]
mod tests {

    #[test]
    fn programs_are_found_by_absolute_executable_paths_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (bin, other) = (d.path().join("bin"), d.path().join("other"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let mk = |p: &Path, mode: u32| {
            std::fs::write(p, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        mk(&bin.join("run"), 0o755);
        mk(&other.join("run"), 0o755);
        mk(&other.join("plain"), 0o644);
        std::os::unix::fs::symlink(bin.join("run"), other.join("link")).unwrap();
        std::os::unix::fs::symlink(bin.join("gone"), other.join("broken")).unwrap();
        let path = |ds: &[&str]| std::env::join_paths(ds).unwrap();
        let (b, o) = (bin.to_str().unwrap(), other.to_str().unwrap());
        // The first absolute folder that has it.
        assert_eq!(find("run".as_ref(), &path(&[b, o])), bin.join("run"));
        // An empty entry and a relative one are passed over, not taken as the current folder.
        assert_eq!(find("run".as_ref(), &path(&["", "bin", o])), other.join("run"));
        assert_eq!(find("nothing".as_ref(), &path(&["", "."])), PathBuf::from("nothing"));
        // A file that can't be run is passed over; a link to one that can is found, a broken one isn't.
        assert_eq!(find("plain".as_ref(), &path(&[o])), PathBuf::from("plain"));
        assert_eq!(find("link".as_ref(), &path(&[o])), other.join("link"));
        assert_eq!(find("broken".as_ref(), &path(&[o])), PathBuf::from("broken"));
        // A path is kept as given.
        assert_eq!(find("./run".as_ref(), &path(&[b])), PathBuf::from("./run"));
        // Refused without a start: a bare name, a relative path, a missing or unrunnable absolute one.
        for p in [
            PathBuf::from("run"),
            PathBuf::from("bin/run"),
            bin.join("gone"),
            other.join("plain"),
            other.join("broken"),
        ] {
            assert_eq!(settle(&Command::new(&p)).unwrap_err().kind(), io::ErrorKind::NotFound, "{}", p.display());
        }
        settle(&Command::new(bin.join("run"))).unwrap();
        settle(&Command::new(other.join("link"))).unwrap();
    }
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    /// A child started from `command` runs without malloc's large cache, and a program it starts
    /// inherits that.
    #[test]
    fn children_run_without_the_large_cache() {
        let o = output(command("/bin/sh").args(["-c", "/usr/bin/printenv MallocLargeCache"])).unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), NO_LARGE_CACHE.1);
    }

    /// Whether a child started by `spawn` holds descriptor `fd` (its `/dev/fd`).
    fn child_holds(fd: i32) -> bool {
        status(Command::new("/bin/test").args(["-e", &format!("/dev/fd/{fd}")])).unwrap().success()
    }

    /// A socket made the way std makes one on macOS: the socket, then (`mark`) close-on-exec.
    fn raw_socket() -> OwnedFd {
        // SAFETY: a new socket, owned from here.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(fd >= 0, "{}", io::Error::last_os_error());
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    fn mark(fd: &OwnedFd) {
        // SAFETY: fcntl on a descriptor `fd` owns.
        assert_eq!(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) }, 0);
    }

    /// The gap itself: a child started between a socket's making and its mark holds it (what
    /// refused the next coordinator its port) …
    #[test]
    fn a_child_started_in_the_gap_holds_the_socket() {
        let s = raw_socket();
        assert!(child_holds(s.as_raw_fd()));
        mark(&s);
        assert!(!child_holds(s.as_raw_fd()));
    }

    /// … and no child can be started in it: while `sockets` runs, a start waits.
    #[test]
    fn no_child_starts_while_a_socket_is_made() {
        let fd = sockets(|| {
            let s = raw_socket();
            assert!(!could_start());
            mark(&s);
            s
        });
        assert!(!child_holds(fd.as_raw_fd()));
    }

    /// A listener `listen` makes, and a connection `accept` takes, are no child's.
    #[test]
    fn listen_and_accept_leave_children_nothing() {
        let l = listen(("127.0.0.1", 0)).unwrap();
        let addr = l.local_addr().unwrap();
        assert!(!child_holds(l.as_raw_fd()));
        assert_eq!(accept(&l).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        l.set_nonblocking(true).unwrap();
        let _c = TcpStream::connect(addr).unwrap();
        // (Connected isn't yet on the listener's queue, under load: waited for, with no time limit,
        // outside the lock, until the listener is readable, which then holds until taken.)
        let mut ready = libc::pollfd { fd: l.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, on a descriptor `l` owns.
        assert_eq!(unsafe { libc::poll(&mut ready, 1, -1) }, 1, "{}", io::Error::last_os_error());
        let (s, _) = accept(&l).unwrap();
        assert!(!child_holds(s.as_raw_fd()));
    }

    /// A detached child holds no descriptor of this process's, close-on-exec or not: a pipe whose
    /// write end is left open across exec (the worst case) still reads to its end once ours closes.
    #[test]
    fn a_detached_child_holds_nothing() {
        let mut fds = [0; 2];
        // SAFETY: a new pipe, owned from here, neither end close-on-exec.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let mut c = detached("/bin/sleep", &["30"]).unwrap();
        drop(w);
        let mut b = [0u8; 1];
        let mut p = libc::pollfd { fd: r.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd on a descriptor `r` owns; then a read into our buffer.
        assert_eq!(unsafe { libc::poll(&mut p, 1, 5000) }, 1, "no end seen: the child holds the pipe");
        assert_eq!(unsafe { libc::read(r.as_raw_fd(), b.as_mut_ptr().cast(), 1) }, 0);
        assert!(!c.try_wait().unwrap());
        c.kill().unwrap();
        assert!(c.try_wait().unwrap());
    }

    /// Task #900: children that outlive a read (here 0.5 s, as caffeinate outlives one) started by
    /// other threads while one reads children's output (std's pipes, marked close-on-exec a call
    /// after they're made) never hold that output's pipe: every read ends at once, never when a
    /// holder ends. Both kinds: `spawn` with its streams null, and `detached`. (With the lock taken
    /// out of `spawn`, a read waits out a holder within seconds.)
    #[test]
    fn long_lived_children_never_hold_another_childs_pipe() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let starters: Vec<_> = (0..6)
            .map(|i| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let (mut kids, mut held) = (Vec::new(), Vec::new());
                    while !stop.load(Ordering::Relaxed) {
                        if i % 3 == 0 {
                            kids.push(detached("/bin/sleep", &["0.5"]).unwrap());
                        } else {
                            let mut c = command("/bin/sleep");
                            c.arg("0.5").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                            held.push(spawn(&mut c).unwrap());
                        }
                        kids.retain_mut(|k| !k.try_wait().unwrap());
                        held.retain_mut(|h| h.try_wait().unwrap().is_none());
                        if kids.len() + held.len() >= 150 {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                    kids.iter_mut().for_each(|k| drop(k.kill()));
                    held.iter_mut().for_each(|h| drop((h.kill(), h.wait())));
                })
            })
            .collect();
        let start = Instant::now();
        let mut slowest = Duration::ZERO;
        while start.elapsed() < Duration::from_secs(6) {
            let t = Instant::now();
            let o = output(command("/bin/echo").arg("x")).unwrap();
            assert_eq!(o.stdout, b"x\n");
            slowest = slowest.max(t.elapsed());
        }
        stop.store(true, Ordering::Relaxed);
        starters.into_iter().for_each(|t| t.join().unwrap());
        assert!(slowest < Duration::from_millis(400), "a read took {slowest:?}: a long-lived child held its pipe");
    }
}
