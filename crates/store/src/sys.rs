//! What differs between platforms: the build runs on macOS (the Macs). Unix gets the real calls;
//! elsewhere a portable fallback, or `Unsupported`.

use std::fs::File;
use std::io;
use std::path::Path;

/// Reads and writes at an offset. Unix's `FileExt`; elsewhere a seek then a read or write (the
/// WebAssembly programs are single-threaded, so nothing else moves the position in between).
pub trait PosIo {
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()>;
    fn write_all_at(&self, buf: &[u8], off: u64) -> io::Result<()>;
}

#[cfg(unix)]
impl PosIo for File {
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(self, buf, off)
    }
    fn write_all_at(&self, buf: &[u8], off: u64) -> io::Result<()> {
        std::os::unix::fs::FileExt::write_all_at(self, buf, off)
    }
}

#[cfg(not(unix))]
impl PosIo for File {
    fn read_exact_at(&self, buf: &mut [u8], off: u64) -> io::Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self;
        f.seek(SeekFrom::Start(off))?;
        f.read_exact(buf)
    }
    fn write_all_at(&self, buf: &[u8], off: u64) -> io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = self;
        f.seek(SeekFrom::Start(off))?;
        f.write_all(buf)
    }
}

/// This machine's name, when it has one.
pub fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        let mut b = [0u8; 256];
        // SAFETY: gethostname writes at most `b.len()` bytes into the buffer we own.
        if unsafe { libc::gethostname(b.as_mut_ptr().cast(), b.len()) } != 0 {
            return None;
        }
        let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Some(String::from_utf8_lossy(&b[..n]).into_owned()).filter(|h| !h.is_empty())
    }
    #[cfg(not(unix))]
    None
}

/// This Mac's name (its LocalHostName: the heartbeat's, the temporary files' names), read once.
pub fn host() -> &'static str {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(host_name)
}

/// This Mac's name, read now (`host`): its LocalHostName, from SystemConfiguration (what
/// `scutil --get LocalHostName` prints), no child started.
pub fn host_name() -> String {
    local_host_name().filter(|s| !s.is_empty()).unwrap_or_else(|| "unknown".into())
}

#[cfg(target_os = "macos")]
fn local_host_name() -> Option<String> {
    use std::ffi::c_void;
    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyLocalHostName(store: *const c_void) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(s: *const c_void, buf: *mut libc::c_char, len: isize, encoding: u32) -> u8;
        fn CFRelease(cf: *const c_void);
    }
    // kCFStringEncodingUTF8
    const UTF8: u32 = 0x0800_0100;
    // SAFETY: SCDynamicStoreCopyLocalHostName takes a null store (a session of its own for the
    // call, so safe from any thread) and returns a CFString we own (the Copy rule), or null when
    // no LocalHostName is set; read into our buffer (a name is at most 63 bytes), then released.
    unsafe {
        let s = SCDynamicStoreCopyLocalHostName(std::ptr::null());
        if s.is_null() {
            return None;
        }
        let mut b = [0 as libc::c_char; 256];
        let ok = CFStringGetCString(s, b.as_mut_ptr(), b.len() as isize, UTF8) != 0;
        CFRelease(s);
        ok.then(|| std::ffi::CStr::from_ptr(b.as_ptr()).to_string_lossy().into_owned())
    }
}

#[cfg(not(target_os = "macos"))]
fn local_host_name() -> Option<String> {
    hostname().map(|h| h.split('.').next().unwrap_or_default().to_string())
}

/// This process's id (0 off Unix).
pub fn pid() -> u32 {
    #[cfg(unix)]
    return std::process::id();
    #[cfg(not(unix))]
    0
}

/// Free bytes for an unprivileged user on the local filesystem holding `path`.
pub fn disk_free(path: &Path) -> io::Result<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes())?;
        #[cfg(target_os = "macos")]
        {
            // SAFETY: `statfs` is plain old data; the path is NUL-terminated. Local disks only
            // (statfs can hang on a NAS path).
            let mut st: libc::statfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(st.f_bavail.saturating_mul(u64::from(st.f_bsize)))
        }
        #[cfg(not(target_os = "macos"))]
        {
            // SAFETY: as above, with statvfs.
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(io::Error::new(io::ErrorKind::Unsupported, "no free-space query on this platform"))
    }
}

/// The filesystem holding `path` (macOS's statfs): what it mounts (`//user@host/share` for a
/// network share), its kind (`smbfs`, `nfs`, `apfs`, ...) and the bytes free for an unprivileged
/// user. Can hang on a NAS path: off any loop that must keep time.
pub fn mount_of(path: &Path) -> io::Result<(String, String, u64)> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::{CStr, CString};
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: as disk_free; the names are NUL-terminated within their arrays.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let text = |a: &[libc::c_char]| unsafe { CStr::from_ptr(a.as_ptr()) }.to_string_lossy().into_owned();
        Ok((text(&st.f_mntfromname), text(&st.f_fstypename), st.f_bavail.saturating_mul(u64::from(st.f_bsize))))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err(io::Error::new(io::ErrorKind::Unsupported, "no mount query on this platform"))
    }
}

/// The size of the (local) filesystem holding `path`, in bytes.
pub fn disk_size(path: &Path) -> io::Result<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes())?;
        #[cfg(target_os = "macos")]
        {
            // SAFETY: as disk_free.
            let mut st: libc::statfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(st.f_blocks.saturating_mul(u64::from(st.f_bsize)))
        }
        #[cfg(not(target_os = "macos"))]
        {
            // SAFETY: as disk_free.
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((st.f_blocks as u64).saturating_mul(st.f_frsize as u64))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(io::Error::new(io::ErrorKind::Unsupported, "no disk-size query on this platform"))
    }
}

/// A file's permission bits, where files have them.
pub fn mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Sets a file's permission bits, where files have them (elsewhere nothing).
pub fn set_mode(f: &File, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (f, mode);
        Ok(())
    }
}

/// Copies `src`'s bytes to `dst` (made, or emptied first) and its permission bits, and nothing else:
/// what every copy in the build wants. (`std::fs::copy` on macOS also brings the extended
/// attributes, and the NAS refuses one, `com.apple.provenance`, which macOS puts on whatever an app
/// writes, when it differs from the folder's: that fails the whole copy.) Within one APFS volume, to
/// a `dst` not there yet, it's a clone instead (instant, the blocks shared, attributes and all).
pub fn copy_data(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<u64> {
    copy_data_as(src.as_ref(), dst.as_ref(), false).map(|c| c.0)
}

/// `copy_data`, with how many of `src`'s bytes this Mac's page cache held as it was opened
/// (`resident`): a NAS file's copy counts those as read here (crate::nasfs's copies).
pub fn copy_data_held(src: &Path, dst: &Path) -> io::Result<(u64, u64)> {
    copy_data_as(src, dst, true)
}

/// Each copy's reads and writes (over SMB each is a round trip, so big ones).
pub const COPY_BUF: usize = 1 << 20;

fn copy_data_as(src: &Path, dst: &Path, held: bool) -> io::Result<(u64, u64)> {
    use std::io::{Read, Write};
    // (A file alone, as std::fs::copy: a clone would take a folder whole.)
    let m = std::fs::metadata(src)?;
    if !m.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} isn't a file", src.display())));
    }
    // (Asked of the descriptor the copy reads: no other open of it.)
    let (r, h) = if held {
        let f = File::open(src)?;
        let h = resident(&f, m.len());
        (Some(f), h)
    } else {
        (None, 0)
    };
    #[cfg(target_os = "macos")]
    if let Some(n) = clone(src, dst) {
        return Ok((n, h));
    }
    let mut r = match r {
        Some(f) => f,
        None => File::open(src)?,
    };
    let mut w = File::create(dst)?;
    let mut buf = vec![0u8; COPY_BUF];
    let mut n = 0u64;
    loop {
        let k = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        w.write_all(&buf[..k])?;
        n += k as u64;
    }
    if let Some(m) = mode(src) {
        set_mode(&w, m).ok();
    }
    Ok((n, h))
}

/// An APFS clone of `src` at `dst`, which isn't there yet; None when the volume makes none (another
/// volume, the NAS, any error), and nothing is left at `dst` then.
#[cfg(target_os = "macos")]
fn clone(src: &Path, dst: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    if dst.symlink_metadata().is_ok() {
        return None;
    }
    let (s, d) = (
        std::ffi::CString::new(src.as_os_str().as_bytes()).ok()?,
        std::ffi::CString::new(dst.as_os_str().as_bytes()).ok()?,
    );
    // SAFETY: clonefile with two NUL-terminated paths we own.
    if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } != 0 {
        std::fs::remove_file(dst).ok();
        return None;
    }
    std::fs::metadata(dst).ok().map(|m| m.len())
}

/// How many of the first `len` bytes of open file `f` (its length as opened) this Mac's page cache
/// holds now (`mincore` over a mapping of it): a read of them is answered here, never by the file's
/// server (a NAS file read again while its pages are kept: on 4 Nov a second copy of a 233 MB
/// terrain pack took 0.05 s, and en0 received 5 MB). 0 when it can't be told (any error, an empty
/// file, another platform). The mapping is only asked about, never read: no page is brought in, so
/// a share that drops meanwhile can't fault (crate::range: a NAS file's mapped pages are never
/// touched). A file of no more than one copy read (`COPY_BUF`) isn't asked, 0: mapping a NAS file
/// is a round trip of its own (15-19 ms each on the M4 over 150 small files, 4 Nov, twice an
/// open), as long as reading it, so asking could save no more than it costs.
pub fn resident(f: &File, len: u64) -> u64 {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if len <= COPY_BUF as u64 {
            return 0;
        }
        let Ok(n) = usize::try_from(len) else { return 0 };
        // SAFETY: sysconf with a valid name.
        let page = match unsafe { libc::sysconf(libc::_SC_PAGESIZE) } {
            p if p > 0 => p as usize,
            _ => return 0,
        };
        // SAFETY: a read-only shared mapping of `n` bytes of a descriptor the caller holds open,
        // unmapped below and never dereferenced.
        let a = unsafe { libc::mmap(std::ptr::null_mut(), n, libc::PROT_READ, libc::MAP_SHARED, f.as_raw_fd(), 0) };
        if a == libc::MAP_FAILED {
            return 0;
        }
        let mut v = vec![0u8; n.div_ceil(page)];
        // SAFETY: `a` maps `n` bytes; `v` has a byte for each of its pages.
        let ok = unsafe { libc::mincore(a, n, v.as_mut_ptr().cast()) } == 0;
        // SAFETY: the mapping made above, `n` bytes long.
        unsafe { libc::munmap(a, n) };
        if !ok {
            return 0;
        }
        // (The last page counts its bytes in the file alone.)
        let pages = v.iter().filter(|&&b| b & 1 != 0).count() as u64;
        let last = v.last().is_some_and(|&b| b & 1 != 0);
        let tail = (page as u64 - len % page as u64) % page as u64;
        (pages * page as u64).saturating_sub(if last { tail } else { 0 }).min(len)
    }
    #[cfg(not(unix))]
    {
        let _ = (f, len);
        0
    }
}

/// Whether an error means the network or the share itself is gone (what a soft mount returns once
/// it gives up), as opposed to a problem with one file.
pub fn is_disconnect(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
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
    #[cfg(not(unix))]
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut
            | io::ErrorKind::NotConnected
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name read without a child is the one `scutil --get LocalHostName` prints (task #900).
    #[cfg(target_os = "macos")]
    #[test]
    fn host_name_is_scutils_local_host_name() {
        let o =
            crate::spawn::output(crate::spawn::command("/usr/sbin/scutil").args(["--get", "LocalHostName"])).unwrap();
        assert_eq!(host_name(), String::from_utf8_lossy(&o.stdout).trim());
    }

    #[test]
    fn positional_reads_and_writes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        let f = File::options().read(true).write(true).open(&p).unwrap();
        f.write_all_at(b"ab", 3).unwrap();
        let mut b = [0u8; 4];
        f.read_exact_at(&mut b, 2).unwrap();
        assert_eq!(&b, b"2ab5");
        assert!(disk_free(d.path()).unwrap() > 0);
        assert!(hostname().is_some());
    }

    /// A file just written and read is in the page cache whole (its last page counting the
    /// file's bytes alone); one of no more than a copy read isn't asked, so counts none; and a
    /// copy says what its source's cache held.
    #[cfg(unix)]
    #[test]
    fn resident_counts_the_bytes_the_page_cache_holds() {
        let d = tempfile::tempdir().unwrap();
        let (a, small) = (d.path().join("a"), d.path().join("small"));
        let n = COPY_BUF + 3 * 16384 + 5;
        std::fs::write(&a, vec![7u8; n]).unwrap();
        std::fs::read(&a).unwrap();
        assert_eq!(resident(&File::open(&a).unwrap(), n as u64), n as u64);
        std::fs::write(&small, vec![7u8; COPY_BUF]).unwrap();
        std::fs::read(&small).unwrap();
        assert_eq!(resident(&File::open(&small).unwrap(), COPY_BUF as u64), 0);
        assert_eq!(copy_data_held(&a, &d.path().join("b")).unwrap(), (n as u64, n as u64));
        assert_eq!(copy_data(&a, d.path().join("c")).unwrap(), n as u64);
    }

    #[test]
    fn a_copy_is_the_bytes_and_the_mode_alone() {
        let d = tempfile::tempdir().unwrap();
        let (a, b, c) = (d.path().join("a"), d.path().join("b"), d.path().join("c"));
        let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7) as u8).collect();
        std::fs::write(&a, &bytes).unwrap();
        set_mode(&File::open(&a).unwrap(), 0o640).unwrap();
        std::fs::write(&b, b"longer than nothing, and replaced").unwrap();
        #[cfg(target_os = "macos")]
        {
            // An attribute on the source doesn't come along.
            let (p, n) = (std::ffi::CString::new(a.to_str().unwrap()).unwrap(), c"org.scenic.test");
            // SAFETY: setxattr with pointers to buffers we own, their lengths given.
            assert_eq!(unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), b"1".as_ptr().cast(), 1, 0, 0) }, 0);
        }
        // Onto a file there already: the bytes copied.
        assert_eq!(copy_data(&a, &b).unwrap(), bytes.len() as u64);
        assert_eq!(std::fs::read(&b).unwrap(), bytes);
        assert_eq!(mode(&b), Some(0o640));
        // A new one: on APFS, a clone; the same bytes either way.
        assert_eq!(copy_data(&a, &c).unwrap(), bytes.len() as u64);
        assert_eq!(std::fs::read(&c).unwrap(), bytes);
        assert_eq!(mode(&c), Some(0o640));
        #[cfg(target_os = "macos")]
        {
            let (p, n) = (std::ffi::CString::new(b.to_str().unwrap()).unwrap(), c"org.scenic.test");
            // SAFETY: getxattr asking only for the size (no buffer).
            assert_eq!(unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), std::ptr::null_mut(), 0, 0, 0) }, -1);
        }
    }
}
