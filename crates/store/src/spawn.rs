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
//! So a child is started (`spawn`, `status`, `output`) only while no listener or connection is
//! being made or closed (`sockets`), and the other way round: one lock, read by the starts (any
//! number at once), written by the sockets' few moments. Every start in the workspace goes
//! through here (clippy.toml's disallowed-methods, checked by `pipeline`'s
//! `every_child_is_started_by_spawn`).
//!
//! Not a fork-free start with every descriptor but the three closed (posix_spawn's
//! `POSIX_SPAWN_CLOEXEC_DEFAULT`): std's `Child` can't be had from one, and std's own fork and
//! exec, which `pre_exec` would force, holds every socket until the exec, worse than the gap.
//!
//! Sockets that hold no port of a listener here, an outgoing connection's (ureq's), aren't made
//! under the lock: a child holding one keeps a connection open, never a port from being listened
//! on. Files are opened close-on-exec in the one call (`O_CLOEXEC`): no gap.
#![allow(clippy::disallowed_methods)]

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

static GAP: RwLock<()> = RwLock::new(());

fn starting() -> RwLockReadGuard<'static, ()> {
    GAP.read().unwrap_or_else(|e| e.into_inner())
}

fn making() -> RwLockWriteGuard<'static, ()> {
    GAP.write().unwrap_or_else(|e| e.into_inner())
}

/// `cmd.spawn()`, while no socket is being made or closed.
pub fn spawn(cmd: &mut Command) -> io::Result<Child> {
    let _g = starting();
    cmd.spawn()
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
    let _g = making();
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
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "accept on a blocking listener would hold every child's start back"));
        }
    }
    sockets(|| listener.accept())
}

/// Whether a child could be started now (none held back by `sockets`). For tests.
#[doc(hidden)]
pub fn could_start() -> bool {
    GAP.try_read().is_ok()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

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
        // (Connected: the connection is waiting on the listener's queue, whatever the time.)
        let (s, _) = accept(&l).unwrap();
        assert!(!child_holds(s.as_raw_fd()));
    }
}
