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

/// Raises this process's open-files limit (macOS starts a LaunchAgent at 256) to its hard limit,
/// at most 10,240: the coordinator holds a socket per connection.
pub fn raise_open_files() {
    #[cfg(unix)]
    // SAFETY: plain getrlimit/setrlimit on this process.
    unsafe {
        let mut l: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) == 0 {
            let want = l.rlim_max.min(10_240);
            if l.rlim_cur < want {
                l.rlim_cur = want;
                libc::setrlimit(libc::RLIMIT_NOFILE, &l);
            }
        }
    }
}

/// A symbolic link `dst` to `src` where there are links; elsewhere a copy.
pub fn symlink(src: &std::path::Path, dst: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(src, dst);
    #[cfg(not(unix))]
    std::fs::copy(src, dst).map(|_| ())
}

/// The most memory this process and its finished children have held at once (bytes): getrusage's
/// maximum resident sizes, the larger (macOS gives them in bytes); 0 where there's no such call.
/// (A child's own, not theirs together: programs running at once, a pool's workers, are summed by
/// `group_peak` alone.)
pub fn peak_rss() -> u64 {
    #[cfg(unix)]
    {
        let max = |who: libc::c_int| -> u64 {
            // SAFETY: getrusage fills the struct we own.
            let mut u: libc::rusage = unsafe { std::mem::zeroed() };
            if unsafe { libc::getrusage(who, &mut u) } != 0 {
                return 0;
            }
            let v = u.ru_maxrss.max(0) as u64;
            if cfg!(target_os = "macos") { v } else { v * 1024 }
        };
        max(libc::RUSAGE_SELF).max(max(libc::RUSAGE_CHILDREN))
    }
    #[cfg(not(unix))]
    0
}

/// The memory this process's group holds now (bytes): a job's (the agent starts each in a group of
/// its own: scenic-build and every program it runs, a pool's workers too), its processes' physical
/// footprints (each one's memory as Activity Monitor shows it) summed; None where it can't be read.
pub fn group_footprint() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: getpgrp has no failure.
        footprint_of_group(unsafe { libc::getpgrp() })
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The memory process group `pgid` holds now (bytes), as `group_footprint`: a running job's, for
/// the agent's status.
pub fn footprint_of_group(pgid: i32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut total = None;
        for pid in group_members(pgid) {
            // SAFETY: rusage_info_v2 is plain old data; proc_pid_rusage fills it, the flavour asked.
            let mut ri: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
            if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&mut ri as *mut libc::rusage_info_v2).cast()) } == 0 {
                total = Some(total.unwrap_or(0) + ri.ri_phys_footprint);
            }
        }
        total
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pgid;
        None
    }
}

/// The most `group_footprint` since the last `reset_group_peak`, as a thread samples it four times
/// a second (started by the first reset), under the lock; a sample taken before a reset is dropped
/// (`GROUP_EPOCH`), so an earlier target's peak isn't counted in the next.
static GROUP_PEAK: std::sync::Mutex<(u64, u64)> = std::sync::Mutex::new((0, 0));
static SAMPLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Starts the job's peak memory again (a target's start: `group_peak` is then that target's).
pub fn reset_group_peak() {
    let sampled = *SAMPLED.get_or_init(|| {
        group_footprint().is_some()
            && std::thread::Builder::new()
                .name("group-peak".into())
                .spawn(|| loop {
                    let epoch = GROUP_PEAK.lock().map_or(0, |g| g.0);
                    if let Some(v) = group_footprint() {
                        if let Ok(mut g) = GROUP_PEAK.lock() {
                            if g.0 == epoch {
                                g.1 = g.1.max(v);
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                })
                .is_ok()
    });
    if sampled {
        let now = group_footprint().unwrap_or(0);
        if let Ok(mut g) = GROUP_PEAK.lock() {
            *g = (g.0 + 1, now);
        }
    }
}

/// The most memory the job's processes held together since `reset_group_peak` (bytes): sampled, so
/// a program shorter than a quarter of a second may go unseen; `peak_rss` where the group can't be
/// sampled (or before the first reset).
pub fn group_peak() -> u64 {
    if SAMPLED.get() != Some(&true) {
        return peak_rss();
    }
    let now = group_footprint().unwrap_or(0);
    GROUP_PEAK.lock().map_or(now, |g| g.1.max(now))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn a_groups_memory_is_its_processes_together() {
        // Run again in a process and a group of its own, this test alone in it: in the runner's,
        // the other tests' memory comes and goes meanwhile, by more than the check's margin.
        const ALONE: &str = "SCENIC_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let mut c = Command::new(std::env::current_exe().unwrap());
            c.args(["--exact", "sys::tests::a_groups_memory_is_its_processes_together"]).env(ALONE, "1");
            own_group(&mut c);
            let out = c.output().unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success() && said.contains(" 1 passed"), "{said}{}", String::from_utf8_lossy(&out.stderr));
            return;
        }
        // This test's process group: the test binary, running this test alone.
        let me = group_footprint().unwrap();
        assert!(me > 1 << 20, "{me}");
        // A child holding 200 MB of its own (written, so it's resident) counts while it runs. (It
        // holds them until its input closes, after they're counted: not for a time a loaded Mac
        // might take longer than to count them.)
        let mut c = Command::new("/usr/bin/python3").args(["-c", "import sys; b = bytearray(200 << 20); b[::4096] = b'x' * len(b[::4096]); print(1, flush=True); sys.stdin.read()"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(c.stdout.as_mut().unwrap()), &mut line).unwrap();
        reset_group_peak();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let with = group_peak();
        drop(c.stdin.take());
        c.wait().unwrap();
        assert!(with >= me + (190 << 20), "{with} vs {me}");
    }
}
