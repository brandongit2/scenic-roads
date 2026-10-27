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
        // (No group, 0 or less: never this process's own, which killpg(0) would be.)
        if pgid <= 0 {
            return false;
        }
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
        let n = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size)
        };
        (n == size).then_some(info.pbi_start_tvsec)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

/// A process as `process` reads it: its parent, when it started (microseconds since the epoch) and
/// its executable (its full path, and its file name).
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub start_us: u64,
    pub path: String,
    pub exe: String,
}

/// A process, when it exists and is readable (this user's).
pub fn process(pid: i32) -> Option<Proc> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: as in `process_start`.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size)
        };
        if n != size {
            return None;
        }
        let mut buf = vec![0u8; 4096];
        // SAFETY: proc_pidpath writes at most the buffer's size.
        let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        buf.truncate(len.max(0) as usize);
        let path = String::from_utf8_lossy(&buf);
        Some(Proc {
            pid,
            ppid: info.pbi_ppid as i32,
            start_us: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            exe: path.rsplit('/').next().unwrap_or("").to_string(),
            path: path.into_owned(),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

/// The processes (other than this one) with a file at `path` open: who holds a lock on it.
pub fn openers(path: &std::path::Path) -> Vec<i32> {
    #[cfg(target_os = "macos")]
    {
        // libproc's, not in the libc crate.
        unsafe extern "C" {
            fn proc_listpidspath(
                kind: u32,
                info: u32,
                path: *const libc::c_char,
                flags: u32,
                buf: *mut libc::c_void,
                size: libc::c_int,
            ) -> libc::c_int;
        }
        const PROC_ALL_PIDS: u32 = 1;
        let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return Vec::new() };
        let mut buf = vec![0i32; 1024];
        // SAFETY: the buffer holds `len` pids and its size in bytes is passed; the path is a C string.
        let n = unsafe {
            proc_listpidspath(PROC_ALL_PIDS, 0, c.as_ptr(), 0, buf.as_mut_ptr().cast(), (buf.len() * 4) as libc::c_int)
        };
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize / 4);
        let me = std::process::id() as i32;
        buf.retain(|&p| p > 0 && p != me);
        buf
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Vec::new()
    }
}

/// SIGTERM to one process; whether it was delivered.
pub fn terminate(pid: i32) -> bool {
    #[cfg(unix)]
    {
        // (Never 0 or less: a group, or every process.)
        // SAFETY: a plain syscall on an exact pid.
        pid > 0 && unsafe { libc::kill(pid, libc::SIGTERM) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// The processes of a process group.
pub fn group_members(pgid: i32) -> Vec<i32> {
    // (No group: none.)
    if pgid <= 0 {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    {
        const PROC_PGRP_ONLY: u32 = 2;
        let mut buf = vec![0i32; 1024];
        // SAFETY: the buffer holds `len` pids and its size in bytes is passed.
        let n = unsafe {
            libc::proc_listpids(PROC_PGRP_ONLY, pgid as u32, buf.as_mut_ptr().cast(), (buf.len() * 4) as libc::c_int)
        };
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

/// A process's children.
pub fn children(pid: i32) -> Vec<i32> {
    #[cfg(target_os = "macos")]
    {
        const PROC_PPID_ONLY: u32 = 6;
        let mut buf = vec![0i32; 1024];
        // SAFETY: the buffer holds `len` pids and its size in bytes is passed.
        let n = unsafe {
            libc::proc_listpids(PROC_PPID_ONLY, pid as u32, buf.as_mut_ptr().cast(), (buf.len() * 4) as libc::c_int)
        };
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize / 4);
        buf.retain(|&p| p > 0);
        buf
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Vec::new()
    }
}

/// Kills a process and every process it started, theirs too, in their process group: each stopped
/// before its children are listed (so none starts another, and none's pid is freed for another
/// process before the kill), then all killed. For a child that starts its own (`uv run` starting
/// Python) inside a job's group, which stays the agent's to pause: a group of their own would run
/// on through a pause.
pub fn kill_tree(pid: i32) {
    #[cfg(unix)]
    {
        let mut all = vec![pid];
        let mut i = 0;
        while i < all.len() {
            // SAFETY: a plain syscall on a process this one started, or one of theirs.
            unsafe { libc::kill(all[i], libc::SIGSTOP) };
            for c in children(all[i]) {
                if !all.contains(&c) {
                    all.push(c);
                }
            }
            i += 1;
        }
        for p in all {
            // SAFETY: as above.
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
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

/// An exclusive lock on the file at a path (`lock`), the file made when it isn't there; let go when
/// dropped, its file removed first while it's still held. So a copy of its descriptor that a child
/// process holds for a moment, between its fork and its exec (a process another thread is starting),
/// holds nothing anyone wants: the next taker makes the file again, and its lock is its own at
/// once, however loaded the Mac. A taker checks, once it has the lock, that the path still names
/// the file it locked (one removed meanwhile by a holder letting go: it takes the path's new file).
#[derive(Debug)]
pub struct PathLock {
    file: File,
    path: std::path::PathBuf,
}

impl PathLock {
    /// The lock of `path` (waiting for it when `wait`); None when another holds it.
    pub fn take(path: &std::path::Path, wait: bool) -> io::Result<Option<PathLock>> {
        // (Each turn a holder let go between our open and our lock: not time, but files.)
        for _ in 0..100 {
            let f = File::options().create(true).truncate(false).write(true).open(path)?;
            if !lock(&f, wait)? {
                return Ok(None);
            }
            if same_file(path, &f) {
                return Ok(Some(PathLock { file: f, path: path.to_path_buf() }));
            }
        }
        Err(io::Error::other(format!("{}: removed each time it was locked", path.display())))
    }

    /// Whether its path still names the file it holds.
    pub fn holds(&self) -> bool {
        same_file(&self.path, &self.file)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for PathLock {
    fn drop(&mut self) {
        // (Removed only while it names this file, held: another's file, made since, stays.)
        if self.holds() {
            std::fs::remove_file(&self.path).ok();
        }
    }
}

/// Whether `path` names the file `f` is open on (its device and inode).
fn same_file(path: &std::path::Path, f: &File) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(path), f.metadata()) {
            (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = f;
        path.exists()
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

/// This process's open-files limit (its soft limit), where there is one.
pub fn open_files_limit() -> Option<u64> {
    #[cfg(unix)]
    // SAFETY: plain getrlimit on this process.
    unsafe {
        let mut l: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) == 0 {
            return Some(l.rlim_cur as u64);
        }
    }
    None
}

/// A symbolic link `dst` to `src` where there are links; elsewhere a copy.
pub fn symlink(src: &std::path::Path, dst: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(src, dst);
    #[cfg(not(unix))]
    std::fs::copy(src, dst).map(|_| ())
}

/// The calling thread, as `thread_cpu_ms` names it (macOS: its Mach port, no reference taken;
/// elsewhere 0, which names none).
pub fn this_thread() -> u32 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: both only read the calling thread's own handles.
        unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

/// The CPU time thread `t` (`this_thread`) has used so far, user and system (ms); None when it
/// can't be read (it ended, or not macOS).
pub fn thread_cpu_ms(t: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        if t == 0 {
            return None;
        }
        // SAFETY: the call fills the struct we own, of the flavour and count asked for.
        let mut info: libc::thread_basic_info = unsafe { std::mem::zeroed() };
        let mut n = libc::THREAD_BASIC_INFO_COUNT;
        let r = unsafe {
            libc::thread_info(t, libc::THREAD_BASIC_INFO as u32, &mut info as *mut _ as libc::thread_info_t, &mut n)
        };
        let ms = |v: libc::time_value_t| v.seconds.max(0) as u64 * 1000 + v.microseconds.max(0) as u64 / 1000;
        (r == libc::KERN_SUCCESS).then(|| ms(info.user_time) + ms(info.system_time))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = t;
        None
    }
}

/// This process's physical footprint now (bytes): what the system charges it, its compressed and
/// swapped memory included (macOS: `proc_pid_rusage`'s `ri_phys_footprint`); elsewhere, its
/// resident peak. None when it can't be read.
pub fn footprint() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: the call fills the struct we own, of the flavour asked for.
        let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V4,
                &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
            )
        };
        if ok == 0 {
            return Some(info.ri_phys_footprint);
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(peak_rss()).filter(|v| *v > 0)
    }
}

/// A watch on this process's footprint (`footprint`), every 50 ms: past `max_mb` MB it says so on
/// stderr, naming `what`, and exits with code 3. Once per process; later calls do nothing.
pub fn footprint_guard(what: &'static str, max_mb: u64) {
    static ON: std::sync::Once = std::sync::Once::new();
    ON.call_once(|| {
        std::thread::Builder::new()
            .name("footprint-guard".into())
            .spawn(move || loop {
                if let Some(b) = footprint() {
                    let mb = b / 1_000_000;
                    if mb > max_mb {
                        eprintln!("{what}: {mb} MB of memory (its footprint, compressed and swapped included), past its limit of {max_mb} MB: stopped");
                        std::process::exit(3);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            })
            .ok();
    });
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
            if cfg!(target_os = "macos") {
                v
            } else {
                v * 1024
            }
        };
        max(libc::RUSAGE_SELF).max(max(libc::RUSAGE_CHILDREN))
    }
    #[cfg(not(unix))]
    0
}

/// A sysctl's value by name, into `T` (plain old data); None where it can't be read.
#[cfg(target_os = "macos")]
fn sysctl<T: Copy>(name: &str) -> Option<T> {
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: T is plain old data the call fills, its size given; the name is a C string.
    let mut v: T = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<T>();
    let r = unsafe { libc::sysctlbyname(name.as_ptr(), (&mut v as *mut T).cast(), &mut len, std::ptr::null_mut(), 0) };
    (r == 0 && len == std::mem::size_of::<T>()).then_some(v)
}

/// This Mac's memory (bytes); None where it can't be read (never kept: a failed read is tried
/// again).
pub fn memsize() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        sysctl::<u64>("hw.memsize").filter(|&b| b > 0)
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The kernel's memory pressure now: 1 normal, 2 warning, 4 critical; None where it can't be read.
pub fn memory_pressure() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        sysctl::<u32>("kern.memorystatus_vm_pressure_level")
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The memory available now (%: the kernel's `kern.memorystatus_level`, what `memory_pressure`
/// prints as its free percentage); None where it can't be read.
pub fn memory_free_pct() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        sysctl::<u32>("kern.memorystatus_level").filter(|&p| p <= 100)
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The swap in use now (bytes); None where it can't be read.
pub fn swap_used() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        sysctl::<libc::xsw_usage>("vm.swapusage").map(|u| u.xsu_used)
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The memory macOS holds compressed now (bytes: `host_statistics64`'s compressor pages, what
/// Activity Monitor calls Compressed); None where it can't be read.
pub fn memory_compressed() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: the call fills `v`, `count` its size in ints.
        let mut v: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
        let mut count = libc::HOST_VM_INFO64_COUNT;
        #[allow(deprecated)]
        let r = unsafe {
            libc::host_statistics64(
                libc::mach_host_self(),
                libc::HOST_VM_INFO64,
                (&mut v as *mut libc::vm_statistics64).cast(),
                &mut count,
            )
        };
        // SAFETY: sysconf has no failure that matters here (-1 is caught).
        let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
        (r == libc::KERN_SUCCESS).then(|| u64::from(v.compressor_page_count) * page)
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// macOS's thermal state now, ProcessInfo.thermalState's four levels: 0 nominal, 1 fair, 2
/// serious, 3 critical. Read from the public notification its value comes from
/// (OSThermalNotification.h's `com.apple.system.thermalpressurelevel`: nominal, moderate, heavy,
/// trapping, sleeping; the last two critical), a shared-memory read once registered; None where it
/// can't be read.
pub fn thermal_state() -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn notify_register_check(name: *const libc::c_char, token: *mut libc::c_int) -> u32;
            fn notify_get_state(token: libc::c_int, state: *mut u64) -> u32;
        }
        static TOKEN: std::sync::OnceLock<Option<libc::c_int>> = std::sync::OnceLock::new();
        let token = (*TOKEN.get_or_init(|| {
            let mut t = 0;
            // SAFETY: a C string; the call fills `t`.
            (unsafe { notify_register_check(c"com.apple.system.thermalpressurelevel".as_ptr(), &mut t) } == 0)
                .then_some(t)
        }))?;
        let mut s = 0u64;
        // SAFETY: a registered token; the call fills `s`.
        (unsafe { notify_get_state(token, &mut s) } == 0).then(|| s.min(3) as u32)
    }
    #[cfg(not(target_os = "macos"))]
    None
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
    usage_of_group(pgid).map(|u| u.footprint)
}

/// The CPU time (user and system, ns) the live processes of group `pgid` have used, summed; None
/// where it can't be read. (A process that ended takes its time with it: the sum may fall.)
pub fn cpu_of_group(pgid: i32) -> Option<u64> {
    usage_of_group(pgid).map(|u| u.cpu_ns)
}

/// What a process group's live processes have used, summed (`usage_of_group`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// CPU time, user and system (ns).
    pub cpu_ns: u64,
    /// Physical footprints now (bytes).
    pub footprint: u64,
    /// Bytes written to disks (`ri_diskio_byteswritten`).
    pub written: u64,
}

/// What the live processes of group `pgid` have used (one `proc_pid_rusage` each); None where none
/// can be read. (A process that ended takes its counts with it: the sums may fall.)
#[allow(deprecated)]
pub fn usage_of_group(pgid: i32) -> Option<Usage> {
    #[cfg(target_os = "macos")]
    {
        // (rusage_info's times are in mach absolute time units: ns on Intel, 125/3 ns on Apple
        // silicon.)
        let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
        let ok = unsafe { libc::mach_timebase_info(&mut tb) } == 0 && tb.denom != 0;
        let (numer, denom) = if ok { (tb.numer as u128, tb.denom as u128) } else { (1, 1) };
        let mut total: Option<Usage> = None;
        for pid in group_members(pgid) {
            // SAFETY: rusage_info_v2 is plain old data; proc_pid_rusage fills it, the flavour asked.
            let mut ri: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
            if unsafe {
                libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&mut ri as *mut libc::rusage_info_v2).cast())
            } == 0
            {
                let u = total.get_or_insert_with(Usage::default);
                u.cpu_ns += ((ri.ri_user_time as u128 + ri.ri_system_time as u128) * numer / denom) as u64;
                u.footprint += ri.ri_phys_footprint;
                u.written += ri.ri_diskio_byteswritten;
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

/// Each core's CPU ticks since the Mac started, (busy, all), in the processors' order (Apple
/// silicon: the efficiency cores first); None where they can't be read (`host_processor_info`).
pub fn core_ticks() -> Option<Vec<(u64, u64)>> {
    #[cfg(target_os = "macos")]
    {
        let mut n: libc::natural_t = 0;
        let mut info: libc::processor_info_array_t = std::ptr::null_mut();
        let mut count: libc::mach_msg_type_number_t = 0;
        // SAFETY: the call allocates `info` (count ints) in this task, freed below.
        #[allow(deprecated)]
        let r = unsafe {
            libc::host_processor_info(
                libc::mach_host_self(),
                libc::PROCESSOR_CPU_LOAD_INFO,
                &mut n,
                &mut info,
                &mut count,
            )
        };
        if r != libc::KERN_SUCCESS || info.is_null() {
            return None;
        }
        // (Per core: user, system, idle, nice.)
        let ticks = unsafe { std::slice::from_raw_parts(info as *const u32, count as usize) };
        let out = ticks
            .chunks_exact(libc::CPU_STATE_MAX as usize)
            .take(n as usize)
            .map(|c| {
                let busy = u64::from(c[0]) + u64::from(c[1]) + u64::from(c[3]);
                (busy, busy + u64::from(c[2]))
            })
            .collect();
        #[allow(deprecated)]
        unsafe {
            libc::vm_deallocate(libc::mach_task_self(), info as libc::vm_address_t, count as usize * 4);
        }
        Some(out)
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The efficiency cores' count (the last perf level's `logicalcpu`, when it's named Efficiency;
/// else 0).
pub fn efficiency_cores() -> u32 {
    #[cfg(target_os = "macos")]
    {
        // (The last level is the slowest: perflevel0 the fastest, as Super, Performance,
        // Efficiency on a three-level chip; its name checked.)
        let n = sysctl::<u32>("hw.nperflevels").unwrap_or(1);
        if n < 2 {
            return 0;
        }
        let name = std::ffi::CString::new(format!("hw.perflevel{}.name", n - 1)).unwrap_or_default();
        let mut buf = [0u8; 64];
        let mut len = buf.len();
        // SAFETY: the call writes at most `len` bytes of the buffer; the name is a C string.
        let r =
            unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
        if r != 0 || !buf[..len.min(buf.len())].starts_with(b"Efficiency") {
            return 0;
        }
        sysctl::<u32>(&format!("hw.perflevel{}.logicalcpu", n - 1)).unwrap_or(0)
    }
    #[cfg(not(target_os = "macos"))]
    0
}

/// The bytes written to this Mac's disks since it started, summed over its block storage drivers'
/// statistics (IOKit's `IOBlockStorageDriver`, as `iostat` reads them); None where they can't be
/// read.
pub fn disk_written() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::c_void;
        #[link(name = "IOKit", kind = "framework")]
        unsafe extern "C" {
            fn IOServiceMatching(name: *const libc::c_char) -> *mut c_void;
            fn IOServiceGetMatchingServices(main: u32, matching: *mut c_void, it: *mut u32) -> i32;
            fn IOIteratorNext(it: u32) -> u32;
            fn IORegistryEntryCreateCFProperty(
                e: u32,
                key: *const c_void,
                alloc: *const c_void,
                o: u32,
            ) -> *const c_void;
            fn IOObjectRelease(o: u32) -> i32;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFStringCreateWithCString(alloc: *const c_void, s: *const libc::c_char, enc: u32) -> *const c_void;
            fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
            fn CFNumberGetValue(n: *const c_void, kind: isize, out: *mut c_void) -> bool;
            fn CFGetTypeID(p: *const c_void) -> usize;
            fn CFDictionaryGetTypeID() -> usize;
            fn CFNumberGetTypeID() -> usize;
            fn CFRelease(p: *const c_void);
        }
        const UTF8: u32 = 0x0800_0100;
        const SINT64: isize = 4;
        // SAFETY: CoreFoundation and IOKit calls on objects made here, each released once; the
        // matching dictionary is consumed by IOServiceGetMatchingServices.
        unsafe {
            let stats = CFStringCreateWithCString(std::ptr::null(), c"Statistics".as_ptr(), UTF8);
            let key = CFStringCreateWithCString(std::ptr::null(), c"Bytes (Write)".as_ptr(), UTF8);
            let mut it = 0u32;
            let mut total = None;
            // (Made only when it will be consumed: IOServiceGetMatchingServices releases it.)
            let m = if stats.is_null() || key.is_null() {
                std::ptr::null_mut()
            } else {
                IOServiceMatching(c"IOBlockStorageDriver".as_ptr())
            };
            if !m.is_null() && IOServiceGetMatchingServices(0, m, &mut it) == 0 {
                loop {
                    let e = IOIteratorNext(it);
                    if e == 0 {
                        break;
                    }
                    let d = IORegistryEntryCreateCFProperty(e, stats, std::ptr::null(), 0);
                    if !d.is_null() {
                        if CFGetTypeID(d) == CFDictionaryGetTypeID() {
                            let v = CFDictionaryGetValue(d, key);
                            let mut b: i64 = 0;
                            if !v.is_null()
                                && CFGetTypeID(v) == CFNumberGetTypeID()
                                && CFNumberGetValue(v, SINT64, (&mut b as *mut i64).cast())
                            {
                                total = Some(total.unwrap_or(0) + b.max(0) as u64);
                            }
                        }
                        CFRelease(d);
                    }
                    IOObjectRelease(e);
                }
                IOObjectRelease(it);
            }
            for s in [stats, key] {
                if !s.is_null() {
                    CFRelease(s);
                }
            }
            total
        }
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The CPU time the calling thread has used (ns).
pub fn thread_cpu_ns() -> u64 {
    #[cfg(unix)]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: fills `t`.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) } != 0 {
            return 0;
        }
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }
    #[cfg(not(unix))]
    0
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

    #[cfg(target_os = "macos")]
    #[test]
    fn a_threads_cpu_time_grows_as_it_works() {
        let t = this_thread();
        let a = thread_cpu_ms(t).unwrap();
        let s = std::time::Instant::now();
        let mut x = 0u64;
        while s.elapsed() < std::time::Duration::from_millis(50) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        assert!(thread_cpu_ms(t).unwrap() > a);
        assert_eq!(thread_cpu_ms(0), None);
    }

    #[test]
    fn the_macs_memory_pressure_and_swap_read() {
        // (What the memory guard reads: crate::agent::memguard.)
        let mem = memsize().expect("hw.memsize");
        assert!(mem >= 4 << 30, "{mem}");
        assert!(memory_pressure().is_some_and(|p| [1, 2, 4].contains(&p)), "{:?}", memory_pressure());
        assert!(swap_used().is_some());
        // (Task #609: the compressed memory, under the Mac's; the thermal state, one of four.)
        assert!(memory_compressed().is_some_and(|c| c < mem), "{:?}", memory_compressed());
        assert!(thermal_state().is_some_and(|t| t <= 3), "{:?}", thermal_state());
    }

    #[test]
    fn a_groups_memory_is_its_processes_together() {
        // Run again in a process and a group of its own, this test alone in it: in the runner's,
        // the other tests' memory comes and goes meanwhile, by more than the check's margin.
        const ALONE: &str = "SCENIC_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let mut c = store::spawn::command(std::env::current_exe().unwrap());
            c.args(["--exact", "sys::tests::a_groups_memory_is_its_processes_together"]).env(ALONE, "1");
            own_group(&mut c);
            let out = store::spawn::output(&mut c).unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && said.contains(" 1 passed"),
                "{said}{}",
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        // This test's process group: the test binary, running this test alone.
        let me = group_footprint().unwrap();
        assert!(me > 1 << 20, "{me}");
        // A child holding 200 MB of its own (written, so it's resident) counts while it runs. (It
        // holds them until its input closes, after they're counted: not for a time a loaded Mac
        // might take longer than to count them.)
        let mut c = store::spawn::spawn(store::spawn::command("/usr/bin/python3").args(["-c", "import sys; b = bytearray(200 << 20); b[::4096] = b'x' * len(b[::4096]); print(1, flush=True); sys.stdin.read()"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())).unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(c.stdout.as_mut().unwrap()), &mut line).unwrap();
        reset_group_peak();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let with = group_peak();
        drop(c.stdin.take());
        c.wait().unwrap();
        assert!(with >= me + (190 << 20), "{with} vs {me}");
    }

    #[test]
    fn a_tree_killed_whole() {
        // A shell with two sleeps (as uv with Python): the shell killed, its children too.
        let mut c = store::spawn::spawn(store::spawn::command("/bin/sh").args(["-c", "sleep 3600 & sleep 3600; wait"]))
            .unwrap();
        let pid = c.id() as i32;
        let kids = store::testkit::wait_for("its two sleeps started", || Some(children(pid)).filter(|k| k.len() == 2));
        kill_tree(pid);
        assert!(!c.wait().unwrap().success());
        // (Gone once launchd has reaped them.)
        store::testkit::eventually("its sleeps gone", || kids.iter().all(|&k| unsafe { libc::kill(k, 0) } != 0));
    }
}
