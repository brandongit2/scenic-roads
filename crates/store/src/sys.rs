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
}
