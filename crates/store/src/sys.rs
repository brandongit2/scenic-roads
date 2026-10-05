//! What differs between the platforms the build's code runs on: macOS (the Macs) and WASI (the
//! WebAssembly programs workers run, docs/workers.md). Unix gets the real calls; elsewhere a
//! portable fallback, or `Unsupported`.

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

/// This process's id, or 0 where there are no processes (WASI, where `std::process::id` panics).
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
    use std::io::{Read, Write};
    let src = src.as_ref();
    // (A file alone, as std::fs::copy: a clone would take a folder whole.)
    if !std::fs::metadata(src)?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} isn't a file", src.display())));
    }
    #[cfg(target_os = "macos")]
    if let Some(n) = clone(src, dst.as_ref()) {
        return Ok(n);
    }
    let mut r = File::open(src)?;
    let mut w = File::create(dst)?;
    // (Big reads and writes: over SMB each is a round trip.)
    let mut buf = vec![0u8; 1 << 20];
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
    Ok(n)
}

/// An APFS clone of `src` at `dst`, which isn't there yet; None when the volume makes none (another
/// volume, the NAS, any error), and nothing is left at `dst` then.
#[cfg(target_os = "macos")]
fn clone(src: &Path, dst: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    if dst.symlink_metadata().is_ok() {
        return None;
    }
    let (s, d) = (std::ffi::CString::new(src.as_os_str().as_bytes()).ok()?, std::ffi::CString::new(dst.as_os_str().as_bytes()).ok()?);
    // SAFETY: clonefile with two NUL-terminated paths we own.
    if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } != 0 {
        std::fs::remove_file(dst).ok();
        return None;
    }
    std::fs::metadata(dst).ok().map(|m| m.len())
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
    matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::NotConnected | io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset)
}

#[cfg(test)]
mod tests {
    use super::*;

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
