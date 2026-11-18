//! The operating system's processes, signals and locks, for the agent and the steps' runner (macOS's), and the
//! app version a program runs from.
//! (I/O that differs by platform is `store::sys`.)

use std::fs::File;
use std::io;
use std::process::{Child, Command, ExitStatus};

/// The app version of the programs in `bin` (`…/app/<version>/`), or "development".
pub fn app_version(bin: &std::path::Path) -> String {
    let canon = bin.canonicalize().unwrap_or_else(|_| bin.to_path_buf());
    match (canon.parent().and_then(|p| p.file_name()), canon.file_name()) {
        (Some(app), Some(v)) if app == "app" => v.to_string_lossy().into_owned(),
        _ => "development".into(),
    }
}

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

/// Whether process `pid` is stopped (state T: a SIGSTOP, a guard's freeze); false when it doesn't
/// exist or can't be read.
pub fn process_stopped(pid: i32) -> bool {
    #[cfg(target_os = "macos")]
    {
        // (proc_bsdinfo's pbi_status: SSTOP, <sys/proc.h>.)
        const SSTOP: u32 = 4;
        // SAFETY: as in `process_start`.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size)
        };
        n == size && info.pbi_status == SSTOP
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        false
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

    /// Writes `text` as its file's whole content: who holds it, for one finding it held to read
    /// (the agent's lock, agent::stray). A taker's own file, made anew when the last let go, so
    /// never an earlier holder's.
    pub fn note(&self, text: &str) -> io::Result<()> {
        use std::io::{Seek, Write};
        let mut f = &self.file;
        f.set_len(0)?;
        f.seek(io::SeekFrom::Start(0))?;
        f.write_all(text.as_bytes())
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

/// This process's physical footprint now (bytes): what the system charges it, its compressed and
/// swapped memory included (macOS: `proc_pid_rusage`'s `ri_phys_footprint`); elsewhere, its
/// resident peak. None when it can't be read.
pub fn footprint() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        rusage(std::process::id() as i32).map(|r| r.footprint)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(peak_rss()).filter(|v| *v > 0)
    }
}

/// What a process has used and retired so far, as macOS counts it (`proc_pid_rusage`,
/// `RUSAGE_INFO_V6`; task #1419): its CPU time, the instructions and cycles its threads retired,
/// each of those on the performance cores apart, its memory now and at most, and its disk writes.
/// Every thread's of the process, never its children's (a child's counts end with it once it's
/// reaped: only its times reach its parent, `RUSAGE_CHILDREN`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rusage {
    /// CPU time, user and system (ns), and of it on the performance cores.
    pub cpu_ns: u64,
    pub p_cpu_ns: u64,
    /// Instructions and cycles retired, and of them on the performance cores.
    pub insn: u64,
    pub cycles: u64,
    pub p_insn: u64,
    pub p_cycles: u64,
    /// Physical footprint now, and the most it has been (bytes).
    pub footprint: u64,
    pub peak: u64,
    /// Bytes written to disks (`ri_diskio_byteswritten`).
    pub written: u64,
    /// When it started (mach absolute time): with its pid, which process it is (a pid is used again).
    pub start: u64,
}

/// Process `pid`'s use (`Rusage`); None where it can't be read (gone and reaped, another user's, or
/// elsewhere than macOS). A macOS without `RUSAGE_INFO_V6` is read at V4: no performance-core split.
pub fn rusage(pid: i32) -> Option<Rusage> {
    #[cfg(target_os = "macos")]
    {
        // (The flavours' layouts, in u64 words after the 16-byte uuid's two: the SDK's
        // sys/resource.h. The buffer is larger than any flavour: the kernel writes its own size.)
        const USER: usize = 2;
        const SYSTEM: usize = 3;
        const FOOTPRINT: usize = 9;
        const START: usize = 10;
        const WRITTEN: usize = 19;
        const PEAK: usize = 30;
        const INSN: usize = 31;
        const CYCLES: usize = 32;
        const USER_P: usize = 38;
        const SYSTEM_P: usize = 39;
        const INSN_P: usize = 40;
        const CYCLES_P: usize = 41;
        const RUSAGE_INFO_V6: i32 = 6;
        let mut w = [0u64; 96];
        // SAFETY: the buffer (768 bytes) holds any flavour's struct (V6's is 464), which the call
        // fills; it's plain words.
        let read = |flavour: i32, w: &mut [u64; 96]| unsafe { libc::proc_pid_rusage(pid, flavour, w.as_mut_ptr().cast()) } == 0;
        let v6 = read(RUSAGE_INFO_V6, &mut w);
        if !v6 && !read(libc::RUSAGE_INFO_V4, &mut w) {
            return None;
        }
        let ns = mach_ns();
        let p = |i: usize| if v6 { w[i] } else { 0 };
        Some(Rusage {
            cpu_ns: ns(w[USER] + w[SYSTEM]),
            p_cpu_ns: ns(p(USER_P) + p(SYSTEM_P)),
            insn: w[INSN],
            cycles: w[CYCLES],
            p_insn: p(INSN_P),
            p_cycles: p(CYCLES_P),
            footprint: w[FOOTPRINT],
            peak: w[PEAK],
            written: w[WRITTEN],
            start: w[START],
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

/// Mach absolute time units to ns (rusage_info's times: 1 ns on Intel, 125/3 ns on Apple silicon).
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn mach_ns() -> impl Fn(u64) -> u64 {
    static TB: std::sync::OnceLock<(u128, u128)> = std::sync::OnceLock::new();
    let &(numer, denom) = TB.get_or_init(|| {
        let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: the call fills the struct we own.
        let ok = unsafe { libc::mach_timebase_info(&mut tb) } == 0 && tb.denom != 0;
        if ok {
            (tb.numer as u128, tb.denom as u128)
        } else {
            (1, 1)
        }
    });
    move |t| (t as u128 * numer / denom) as u64
}

/// This Mac's performance cores' top clock (Hz): the highest of the frequencies its power manager
/// may set them to (`table_clock`), else, where the table can't be read, measured (`spun_clock`).
/// What a job's cycles over its CPU time can't pass (crate::timings::implausible). Read once a
/// process, on the first call (an `ioreg` run, or a thread's spin): a caller that mustn't wait
/// starts it on a thread of its own (`top_clock_soon`) and reads `top_clock_now`.
pub fn top_clock() -> Option<f64> {
    *TOP_CLOCK.get_or_init(|| table_clock().or_else(spun_clock))
}

static TOP_CLOCK: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();

/// `top_clock` read on a thread of its own, from now (the agent's start).
pub fn top_clock_soon() {
    std::thread::Builder::new().name("top-clock".into()).spawn(top_clock).ok();
}

/// `top_clock` if it has been read; None while it's being read (or never was).
pub fn top_clock_now() -> Option<f64> {
    TOP_CLOCK.get().copied().flatten()
}

/// The performance cores' top clock from IOKit's power manager (`ioreg -r -n pmgr`: its
/// `voltage-states5-sram`, the performance cluster's states; `parse_top_clock`); None where there's
/// no such table (another platform, a Mac without it).
fn table_clock() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        let mut c = store::spawn::command("/usr/sbin/ioreg");
        c.args(["-r", "-n", "pmgr", "-d", "1"]);
        let o = store::spawn::output(&mut c).ok()?;
        parse_top_clock(&String::from_utf8_lossy(&o.stdout))
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The highest frequency in `ioreg`'s `voltage-states5-sram` (`<hex>`: pairs of little-endian u32s,
/// each a frequency and its voltage), in Hz. The M1 generation says Hz (the M1 Pro's last, 3228
/// MHz), later ones kHz (the M4 Pro's last, 4512000; the M6's, 4788000): a value under 10^8, no
/// core's clock in Hz, is read as kHz.
pub fn parse_top_clock(ioreg: &str) -> Option<f64> {
    let (_, rest) = ioreg.split_once("\"voltage-states5-sram\" = <")?;
    let hex = &rest[..rest.find('>')?];
    let b: Vec<u8> =
        (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect();
    let top = b.chunks_exact(8).map(|e| u32::from_le_bytes([e[0], e[1], e[2], e[3]])).max().filter(|&f| f > 0)?;
    Some(if top < 100_000_000 { f64::from(top) * 1e3 } else { f64::from(top) })
}

/// The performance cores' clock measured (`Rusage`'s performance-core cycles over their time while a
/// thread of its own spins at the user-interactive QoS, which macOS runs on them, till 0.2 s of
/// their CPU or 2 s by the clock): the cluster's clock as it is then, which other work on its cores
/// may lower. None where it can't be measured (no split, another platform).
fn spun_clock() -> Option<f64> {
    std::thread::Builder::new()
        .name("top-clock-spin".into())
        .spawn(|| {
            interactive();
            let me = std::process::id() as i32;
            let a = rusage(me)?;
            let t = std::time::Instant::now();
            let mut x = 0u64;
            let mut b = a;
            while b.p_cpu_ns.saturating_sub(a.p_cpu_ns) < 200_000_000 && t.elapsed() < std::time::Duration::from_secs(2)
            {
                for i in 0..100_000u64 {
                    x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(i));
                }
                b = rusage(me)?;
            }
            let (cyc, ns) = (b.p_cycles.saturating_sub(a.p_cycles), b.p_cpu_ns.saturating_sub(a.p_cpu_ns));
            (ns >= 50_000_000).then(|| cyc as f64 / (ns as f64 / 1e9))
        })
        .ok()
        .and_then(|h| h.join().ok())
        .flatten()
}

/// This thread at the user-interactive QoS (macOS runs it on a performance core).
fn interactive() {
    #[cfg(target_os = "macos")]
    // SAFETY: sets this thread's own QoS class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
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
                        crate::control::exit(3);
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

/// The kernel's memory-free percentage (`kern.memorystatus_level`, what `memory_pressure` prints):
/// the pages neither wired nor compressed, so apps' pages in use count as free; the live store's
/// pressure row shows it, the memory floor reads `memory_available`. None where it can't be read.
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

/// The kernel's page counts now (`host_statistics64`'s HOST_VM_INFO64) and its page size (bytes);
/// None where they can't be read.
#[cfg(target_os = "macos")]
fn vm_stats() -> Option<(libc::vm_statistics64, u64)> {
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
    (r == libc::KERN_SUCCESS).then_some((v, page))
}

/// The memory macOS holds compressed now (bytes: `host_statistics64`'s compressor pages, what
/// Activity Monitor calls Compressed); None where it can't be read.
pub fn memory_compressed() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        vm_stats().map(|(v, page)| u64::from(v.compressor_page_count) * page)
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The compressor's share of the memory not wired (the kernel's AVAILABLE_MEMORY: active, inactive,
/// free and compressor pages) under which macOS's memory pressure is normal: two fifths. The
/// kernel goes to warning once the compressor holds half of it and back to normal once it holds
/// under two fifths (its warning-to-normal line, 1.2 × the normal-to-warning one on the pages not
/// compressed). Measured on the three Macs' live series (the 10 s ring, 2–4 Nov, every bucket the
/// pressure held one level, the compressor's share as `kern.memorystatus_level` and the
/// compressor's pages give it): into warning most often at 48–50 % (6 of 174 entries under 40 %,
/// as low as 31 %), back to normal most often at 39–41 %, never in warning a whole bucket under
/// 39.8 % (docs/pool.md §7.1). Compression the available counts
/// (`Pages::available`) stops at this line, so memory taken by compressing leaves the pressure
/// normal, and a Mac past it counts none.
const COMPRESSOR_NORMAL: (u64, u64) = (2, 5);

/// The kernel's page counts the available memory is reckoned from (`Pages::available`; pages):
/// `host_statistics64`'s HOST_VM_INFO64 and `vm.page_reusable_count`, a test's Mac as it sets them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pages {
    /// Free (the kernel's count, a read-ahead's speculative pages among them), speculative,
    /// file-backed (`external_page_count`, the speculative among them), purgeable.
    pub free: u64,
    pub speculative: u64,
    pub file: u64,
    pub purgeable: u64,
    /// The reusable pages (`vm.page_reusable_count`): anonymous memory a process freed and its
    /// allocator told the kernel it no longer needs (malloc's `MADV_FREE_REUSABLE`). They stay in
    /// the process's resident size, out of its footprint, marked clean, on whichever queue they
    /// were (xnu moves them nowhere: most are active, just used); the kernel's scan frees one with
    /// nothing compressed when it reaches it on the inactive queue, in turn with the other
    /// anonymous pages there, not before them.
    pub reusable: u64,
    /// The active and inactive queues (file-backed and anonymous both).
    pub active: u64,
    pub inactive: u64,
    /// The pages the compressor occupies, and the pages it holds compressed in them.
    pub compressor: u64,
    pub stored: u64,
}

/// The memory available (`Pages::available`; pages or bytes, as reckoned): what macOS hands out
/// without a cost (`cheap`), and what it frees by compressing other processes' idle memory with
/// the pressure left normal (`compressible`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Available {
    pub cheap: u64,
    pub compressible: u64,
}

impl Available {
    /// Both together.
    pub fn total(&self) -> u64 {
        self.cheap + self.compressible
    }
}

impl Pages {
    /// The memory available (pages).
    ///
    /// - **Cheap:** the free pages, the file-backed ones (the file cache, dropped at once), the
    ///   purgeable ones, and the reusable ones as far as the inactive anonymous pages hold them
    ///   (freed by their process: the scan frees those it reaches there with nothing compressed;
    ///   one on the active queue it reaches only after compressing others, as measured on the M4
    ///   on 12 Nov: 1.4 of 5.6 GB handed out before macOS compressed the memory in use). A
    ///   read-ahead's speculative pages are counted once: the kernel's free count includes them
    ///   and so does the file-backed (file-backed + anonymous = active + inactive + speculative, to
    ///   the page; `vm_stat` prints free less speculative), so they come off the free.
    /// - **Compressible:** what macOS frees by compressing other processes' idle memory before its
    ///   pressure leaves normal. Its inactive anonymous pages (the inactive queue less every
    ///   file-backed page not speculative and the purgeable: the least they can be, as the kernel
    ///   doesn't split the queue), less the reusable counted above, compressed at the ratio its
    ///   compressor holds now (`stored` over `compressor`) free 1 − 1/ratio of themselves; the
    ///   compressor may grow only to `COMPRESSOR_NORMAL` of the memory not wired, each page more
    ///   of it freeing ratio − 1. The less of the two. A compressor holding nothing has no ratio
    ///   measured: none counted.
    pub fn available(&self) -> Available {
        let idle =
            self.inactive.saturating_sub(self.file.saturating_sub(self.speculative)).saturating_sub(self.purgeable);
        let reusable = self.reusable.min(idle);
        let cheap = self.free.saturating_sub(self.speculative) + self.file + self.purgeable + reusable;
        if self.compressor == 0 || self.stored <= self.compressor {
            return Available { cheap, compressible: 0 };
        }
        let idle_anon = idle - reusable;
        // (Compressing x pages at ratio r = stored / compressor frees x (1 − 1/r) = x (stored −
        // compressor) / stored.)
        let by_idle = idle_anon as u128 * (self.stored - self.compressor) as u128 / self.stored as u128;
        let unwired = self.active + self.inactive + self.free + self.compressor;
        let (n, d) = COMPRESSOR_NORMAL;
        let room = (unwired * n / d).saturating_sub(self.compressor);
        // (Each page the compressor grows by holds r: frees r − 1 = (stored − compressor) / compressor.)
        let by_room = room as u128 * (self.stored - self.compressor) as u128 / self.compressor as u128;
        Available { cheap, compressible: by_idle.min(by_room) as u64 }
    }
}

/// The kernel's page counts now (`Pages`) and its page size (bytes); None where they can't be read.
/// (The reusable pages are no field of `host_statistics64`'s: a sysctl's, none counted where it
/// can't be read.)
fn memory_pages() -> Option<(Pages, u64)> {
    #[cfg(target_os = "macos")]
    {
        vm_stats().map(|(v, page)| {
            let p = Pages {
                free: u64::from(v.free_count),
                speculative: u64::from(v.speculative_count),
                file: u64::from(v.external_page_count),
                purgeable: u64::from(v.purgeable_count),
                reusable: sysctl::<u32>("vm.page_reusable_count").map_or(0, u64::from),
                active: u64::from(v.active_count),
                inactive: u64::from(v.inactive_count),
                compressor: u64::from(v.compressor_page_count),
                stored: v.total_uncompressed_pages_in_compressor,
            };
            (p, page)
        })
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// The memory available now (bytes, `Pages::available`): what macOS hands a process at no cost,
/// and what it frees by compressing other processes' idle memory with its pressure left normal.
/// Not `kern.memorystatus_level` (`memory_free_pct`): that counts every page neither wired nor
/// compressed, apps' own pages in use too, and read 60 % on the M4 on 4 Nov with 21 GB swapped
/// (docs/pool.md §7.1). None where it can't be read.
pub fn memory_available() -> Option<Available> {
    let (p, page) = memory_pages()?;
    let a = p.available();
    Some(Available { cheap: a.cheap * page, compressible: a.compressible * page })
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

/// This process's group: a job's (the agent starts each in a group of its own: scenic-build and
/// every program it runs, a pool's workers too). 0 where there are none.
pub fn this_group() -> i32 {
    #[cfg(unix)]
    {
        // SAFETY: getpgrp has no failure.
        unsafe { libc::getpgrp() }
    }
    #[cfg(not(unix))]
    0
}

/// The memory this process's group holds now (bytes), as `footprint_of_group`; None where it
/// can't be read.
pub fn group_footprint() -> Option<u64> {
    footprint_of_group(this_group())
}

/// The memory process group `pgid` holds now (bytes), its processes' physical footprints (each
/// one's memory as Activity Monitor shows it) summed: a running job's, for the agent's status.
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
pub fn usage_of_group(pgid: i32) -> Option<Usage> {
    let all = group_rusage(pgid);
    (!all.is_empty()).then(|| Usage {
        cpu_ns: all.iter().map(|(_, r)| r.cpu_ns).sum(),
        footprint: all.iter().map(|(_, r)| r.footprint).sum(),
        written: all.iter().map(|(_, r)| r.written).sum(),
    })
}

/// Each live process of group `pgid` and its use (`rusage`; one call each), those that can be read.
pub fn group_rusage(pgid: i32) -> Vec<(i32, Rusage)> {
    group_members(pgid).into_iter().filter_map(|p| Some((p, rusage(p)?))).collect()
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

/// The osmium program, found in Homebrew's folder first (task #916: from inputs::osmpass, which
/// re-exports it), its pool held to the threads the job's work runs on (crate::control::cap_threads:
/// `OSMIUM_POOL_THREADS`; it takes every core without it). Every osmium a job runs is started here.
pub fn osmium() -> Command {
    let path = format!("/opt/homebrew/bin:{}", std::env::var("PATH").unwrap_or_default());
    // (Looked up by the PATH it's given: store::spawn.)
    let mut c = store::spawn::command_in("osmium", std::ffi::OsStr::new(&path));
    c.env("PATH", path);
    crate::control::cap_threads(&mut c);
    c
}

/// The programs folded into `scenic-build` (task #917): run as `scenic-build tool <name> …`, each
/// with its own command line (agentcore::cli::tool); jobs and tasks name them as programs
/// (`program`), so a task's spec (`unit::Run::prog`) reads the same to an app from before the fold,
/// which has them as binaries. (Task #916 step 6d: from agentcore::cli, which re-exports both.)
pub const TOOLS: [&str; 12] = [
    "scenic-metrics",
    "extract",
    "elev",
    "tile",
    "areaflags",
    "landcover",
    "railfreq",
    "trees",
    "bldtile",
    "terrainsub",
    "lakefill",
    "scenic-polar",
];

/// The command running program `prog` from the programs' folder `bin` (an app version's): a tool
/// (`TOOLS`) through `bin`'s scenic-build, any other by its own file there. Its arguments follow.
pub fn program(bin: &std::path::Path, prog: &str) -> std::process::Command {
    if TOOLS.contains(&prog) {
        let mut c = store::spawn::command(bin.join("scenic-build"));
        c.args(["tool", prog]);
        c
    } else {
        store::spawn::command(bin.join(prog))
    }
}

/// The available memory's reckoning, on the three Macs' pages as read at 14:39 on 4 Nov (task #945;
/// `vm_stat`, the pressure normal on each) and the Mac mini's at 19:01 on 12 Nov (task #1215, its
/// reusable pages read too), whatever Mac the test runs on.
#[cfg(test)]
mod available {
    use super::*;

    const GB: f64 = (1u64 << 30) as f64;

    /// `vm_stat`'s counts (its free less speculative, as it prints it) and `vm.page_reusable_count`,
    /// in 16 KB pages: the available's two parts (MB).
    fn mb(
        free: u64,
        active: u64,
        inactive: u64,
        spec: u64,
        purgeable: u64,
        file: u64,
        stored: u64,
        occupied: u64,
        reusable: u64,
    ) -> (u64, u64) {
        let p = Pages {
            free: free + spec,
            speculative: spec,
            file,
            purgeable,
            reusable,
            active,
            inactive,
            compressor: occupied,
            stored,
        };
        let a = p.available();
        ((a.cheap * 16384) >> 20, (a.compressible * 16384) >> 20)
    }

    #[test]
    fn memory_a_process_freed_counts_as_far_as_the_idle_memory_holds_it() {
        // The Mac mini (32 GB) with no job running, 7.4 GB wired and its compressor holding 8.5 GB
        // in 4.5: 0.1 GB free, 7.6 of file cache, 0.3 purgeable, and 5.5 GB of reusable pages (its
        // agent's resident size 6.1 GB, its footprint 1.6: memory it freed). The inactive queue's
        // 9.7 GB less the file cache and the purgeable leaves 1.8 GB of idle anonymous memory: of
        // the reusable pages, those 1.8 GB count at no cost (the rest, on the active queue, the
        // scan reaches only after compressing others), and nothing is left idle to compress.
        let mini = |reusable| mb(9110, 630781, 634068, 2463, 18885, 500470, 560163, 295424, reusable);
        let idle = 634068 - (500470 - 2463) - 18885;
        assert_eq!(mini(359193), (10088, 0));
        assert_eq!(mini(359193), mini(idle));
        assert_eq!(mini(u64::MAX).0, ((9110 + 500470 + 18885 + idle) * 16384) >> 20);
        // (Left out, the 1.8 GB read as idle memory to compress, worth 0.8: 8.9 GB in all, a GB
        // less.)
        assert_eq!(mini(0), (8257, 865));
        // (1 GB of them: a GB more at no cost, and of the 1.8 GB read as idle, 0.8 left, worth 0.4.)
        assert_eq!(mini(65536), (9281, 381));
        // The M4 on 12 Nov (MB as read, in pages), a process holding 4.5 GB it had just marked
        // reusable, 5.6 GB of them in all, its inactive queue (11.4 GB) all file cache: none
        // counted. Taking memory then, macOS handed out the free pages, the file cache down to 5.6
        // GB, and 1.4 GB of the reusable ones before it compressed 17 GB of the taker's own (25.1
        // GB taken; 28.7 GB counted available, 34.4 had all of the reusable pages counted).
        let m4 =
            |reusable| mb(15922 * 64, 13676 * 64, 11423 * 64, 0, 149 * 64, 12675 * 64, 6440 * 64, 2894 * 64, reusable);
        assert_eq!(m4(5626 * 64), m4(0));
        assert_eq!(m4(0), (28746, 0));
    }

    #[test]
    fn the_three_macs_count_their_idle_memory_compressed() {
        // (4 Nov's readings, taken without the reusable pages: none counted.)
        // The M4 (48 GB): 6.8 GB cheap; 8.0 GB of idle anonymous pages at the compressor's 1.79
        // would free 3.5 GB, and its 12.2 GB compressor is 4.4 GB short of two fifths of the 41.4
        // GB not wired, which frees as much: 3.5 GB more (6.8 → 10.3 GB).
        assert_eq!(mb(17421, 951169, 949068, 1136, 10128, 418053, 1427478, 796577, 0), (6962, 3583));
        // The Mac mini (32 GB): 6.1 GB cheap; 5.1 GB idle at 3.0 frees 3.4 (the compressor's room
        // would free 14.6): 9.6 GB.
        assert_eq!(mb(13048, 683239, 725414, 518, 4896, 384266, 464897, 154627, 0), (6284, 3511));
        // The M1 (16 GB): 3.0 GB cheap; 2.1 GB idle at 2.43 frees 1.3 (its room 3.8): 4.3 GB.
        assert_eq!(mb(9393, 326665, 325945, 417, 6514, 180159, 367013, 151175, 0), (3063, 1283));
    }

    #[test]
    fn compression_stops_at_the_pressures_normal_line() {
        // (The M4 at 01:10 on 4 Nov: the compressor 23 GB of the 39.7 GB not wired, past two
        // fifths: nothing more counted, whatever is idle.)
        let page = |g: f64| (g * GB / 16384.0) as u64;
        let night = Pages {
            free: page(0.2),
            file: page(1.0),
            active: page(9.0),
            inactive: page(6.5),
            compressor: page(23.0),
            stored: page(41.0),
            ..Default::default()
        };
        assert_eq!(night.available().compressible, 0);
        // (Just under the line, what's left of the room limits it; an empty compressor, no ratio
        // measured, counts none; nothing idle, none.)
        let under = Pages { compressor: page(10.0), stored: page(18.0), ..night };
        let a = under.available();
        let unwired = under.active + under.inactive + under.free + under.compressor;
        let room = (unwired * 2 / 5 - under.compressor) as u128 * (under.stored - under.compressor) as u128
            / under.compressor as u128;
        assert!(a.compressible > 0 && a.compressible as u128 == room, "{a:?} {room}");
        assert_eq!(Pages { compressor: 0, stored: 0, ..under }.available().compressible, 0);
        assert_eq!(Pages { inactive: page(1.0), ..under }.available().compressible, 0, "the idle all file-backed");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_macs_memory_pressure_and_swap_read() {
        // (What the memory guard reads: agentcore::agent::memguard.)
        let mem = memsize().expect("hw.memsize");
        assert!(mem >= 4 << 30, "{mem}");
        assert!(memory_pressure().is_some_and(|p| [1, 2, 4].contains(&p)), "{:?}", memory_pressure());
        assert!(swap_used().is_some());
        // (Task #609: the compressed memory, under the Mac's; the thermal state, one of four.)
        assert!(memory_compressed().is_some_and(|c| c < mem), "{:?}", memory_compressed());
        assert!(thermal_state().is_some_and(|t| t <= 3), "{:?}", thermal_state());
        // (Task #932 review: the speculative pages are file-backed (file-backed + anonymous =
        // active + inactive + speculative, within the pages that move between the counts' reads)
        // and in the kernel's free count too, so the available counts them once, and stays within
        // the Mac less its wired and compressor pages.)
        let (v, page) = vm_stats().expect("host_statistics64");
        let queued = u64::from(v.active_count) + u64::from(v.inactive_count) + u64::from(v.speculative_count);
        let typed = u64::from(v.external_page_count) + u64::from(v.internal_page_count);
        assert!(queued.abs_diff(typed) <= queued / 100, "{queued} {typed}");
        assert!(v.free_count >= v.speculative_count, "{} {}", v.free_count, v.speculative_count);
        // (Task #1215: the reusable pages' count is read, anonymous pages among the queues'.)
        let reusable = sysctl::<u32>("vm.page_reusable_count").expect("vm.page_reusable_count");
        assert!(u64::from(reusable) <= u64::from(v.internal_page_count) + queued / 100, "{reusable} {v:?}");
        let avail = memory_available().expect("available");
        let used = (u64::from(v.wire_count) + u64::from(v.compressor_page_count)) * page;
        assert!(avail.cheap <= mem.saturating_sub(used) + (64 << 20), "{avail:?} {mem} {used}");
        // (Task #945: what compressing frees comes out of the apps' memory in use, never more than
        // the anonymous pages.)
        assert!(avail.compressible <= u64::from(v.internal_page_count) * page, "{avail:?}");
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
        // (group_peak counts the group as it is now beside the peak sampled: no wait for a sample.)
        reset_group_peak();
        let with = group_peak();
        drop(c.stdin.take());
        c.wait().unwrap();
        assert!(with >= me + (190 << 20), "{with} vs {me}");
    }

    /// Task #1419: the rusage reader agrees with what the system says otherwise of this process:
    /// its CPU with getrusage's (both read after a spin, the same counters), its peak at least its
    /// footprint, its start with its start time, and some instructions and cycles retired.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_rusage_reader_agrees_with_getrusage() {
        let mut x = 0u64;
        for i in 0..50_000_000u64 {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(i));
        }
        let me = std::process::id() as i32;
        let r = rusage(me).unwrap();
        // SAFETY: getrusage fills the struct it's given.
        let mut g: libc::rusage = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut g) }, 0);
        let tv = |t: libc::timeval| t.tv_sec as u64 * 1_000_000_000 + t.tv_usec as u64 * 1000;
        let cpu = tv(g.ru_utime) + tv(g.ru_stime);
        // (getrusage read after: as much or a little more, every thread's here.)
        assert!(r.cpu_ns <= cpu + 1_000_000 && cpu - r.cpu_ns.min(cpu) < 50_000_000, "{} vs {cpu}", r.cpu_ns);
        assert!(r.cpu_ns > 10_000_000 && r.p_cpu_ns <= r.cpu_ns, "{r:?}");
        assert!(r.insn > 50_000_000 && r.cycles > 0 && r.p_insn <= r.insn && r.p_cycles <= r.cycles, "{r:?}");
        assert!(r.footprint > 0 && r.peak >= r.footprint, "{r:?}");
        // (Its start, mach absolute time, as seconds since the epoch: now less the time since.)
        #[allow(deprecated)]
        let since = mach_ns()(unsafe { libc::mach_absolute_time() }.saturating_sub(r.start));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let start = now.saturating_sub(since / 1_000_000_000);
        assert!(start.abs_diff(process_start(me).unwrap()) <= 2, "{start} vs {:?}", process_start(me));
        // (The top clock, from the power manager's table where it's there.)
        let ioreg = r#"| |   "voltage-states5-sram" = <0046c32300030000005f67c0ef030000>"#;
        assert_eq!(parse_top_clock(ioreg), Some(3_228_000_000.0));
        let khz = r#""voltage-states5-sram" = <e0391300160300004017460096030000>"#;
        assert_eq!(parse_top_clock(khz), Some(4_593_472_000.0));
        assert_eq!(parse_top_clock("no table"), None);
    }
}
