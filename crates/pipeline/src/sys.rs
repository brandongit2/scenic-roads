//! The operating system's processes, signals and locks, for the agent and the steps' runner: macOS
//! has them; WASI (the WebAssembly programs workers run, docs/workers.md) has none, and there each
//! of these fails (or finds nothing), which only the agent would notice: it never runs there.
//! (I/O that differs by platform is `store::sys`.)

use std::fs::File;
use std::io;
use std::process::{Child, Command, ExitStatus};

/// A signal to a process group.
#[derive(Clone, Copy, Debug)]
pub enum Signal {
    Stop,
    Cont,
    Term,
    Kill,
    /// None: whether the group still has members.
    Probe,
}

/// Sends `sig` to process group `pgid`; whether it was delivered.
pub fn signal_group(pgid: i32, sig: Signal) -> bool {
    #[cfg(unix)]
    {
        let s = match sig {
            Signal::Stop => libc::SIGSTOP,
            Signal::Cont => libc::SIGCONT,
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
            Signal::Probe => 0,
        };
        // SAFETY: a plain syscall on a group we started.
        unsafe { libc::killpg(pgid, s) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = (pgid, sig);
        false
    }
}

/// Makes `c` start in a process group of its own, so signals to it reach every process it starts.
pub fn own_group(c: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = c;
}

/// A process's start time (seconds since the epoch), when it exists.
pub fn process_start(pid: i32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: proc_bsdinfo is plain old data; proc_pidinfo fills at most `size` bytes of it.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
        (n == size).then_some(info.pbi_start_tvsec)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

/// The processes of a process group.
pub fn group_members(pgid: i32) -> Vec<i32> {
    #[cfg(target_os = "macos")]
    {
        const PROC_PGRP_ONLY: u32 = 2;
        let mut buf = vec![0i32; 1024];
        // SAFETY: the buffer holds `len` pids and its size in bytes is passed.
        let n = unsafe { libc::proc_listpids(PROC_PGRP_ONLY, pgid as u32, buf.as_mut_ptr().cast(), (buf.len() * 4) as libc::c_int) };
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize / 4);
        buf.retain(|&p| p > 0);
        buf
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pgid;
        Vec::new()
    }
}

/// Takes an exclusive lock on `f`, waiting for it if `wait`; Ok(false) when it's held elsewhere and
/// not waited for.
pub fn lock(f: &File, wait: bool) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let op = if wait { libc::LOCK_EX } else { libc::LOCK_EX | libc::LOCK_NB };
        // SAFETY: flock on a descriptor we own.
        if unsafe { libc::flock(f.as_raw_fd(), op) } == 0 {
            return Ok(true);
        }
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::WouldBlock {
            return Ok(false);
        }
        Err(e)
    }
    #[cfg(not(unix))]
    {
        let _ = (f, wait);
        Err(io::Error::new(io::ErrorKind::Unsupported, "no file locks on this platform"))
    }
}

/// Calls `handler` on SIGTERM and SIGINT (it may only store to atomics).
pub fn on_terminate(handler: extern "C" fn(i32)) {
    #[cfg(unix)]
    // SAFETY: the handler only stores to an atomic (the caller's promise).
    unsafe {
        libc::signal(libc::SIGTERM, handler as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, handler as *const () as libc::sighandler_t);
    }
    #[cfg(not(unix))]
    let _ = handler;
}

/// Waits for `child` to end; its exit status and the most memory (resident, bytes) it or any
/// program it started took.
pub fn wait_with_peak(child: Child) -> io::Result<(ExitStatus, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let pid = child.id() as libc::pid_t;
        let (mut status, mut ru): (libc::c_int, libc::rusage) = (0, unsafe { std::mem::zeroed() });
        loop {
            // SAFETY: wait4 on our own child, into values we own.
            if unsafe { libc::wait4(pid, &mut status, 0, &mut ru) } == pid {
                break;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        // (Bytes on macOS.)
        Ok((ExitStatus::from_raw(status), ru.ru_maxrss as u64))
    }
    #[cfg(not(unix))]
    {
        let mut child = child;
        Ok((child.wait()?, 0))
    }
}

/// A symbolic link `dst` to `src` where there are links; elsewhere a copy.
pub fn symlink(src: &std::path::Path, dst: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(src, dst);
    #[cfg(not(unix))]
    std::fs::copy(src, dst).map(|_| ())
}
