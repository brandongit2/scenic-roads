//! Range reads over local and NAS files. A local file is mmapped. A NAS file never is: when the
//! share drops, a page fault on a mapped file is a SIGBUS that kills the server. NAS files are read
//! with pread on an ordinary descriptor, through the I/O pool, so a stall becomes a timeout.

use crate::iopool::{IoError, IoPool};
use anyhow::{Context, Result};
use memmap2::Mmap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Random access to an immutable file's bytes.
pub trait RangeRead: Send + Sync {
    /// The length in bytes: an error when it can't be learnt (a NAS file whose share is away).
    fn len(&self) -> Result<u64, IoError>;

    /// Exactly `len` bytes at `off`; an error if that runs past the end.
    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError>;
}

/// A range source whose bytes are all in memory already (an mmap), so they can be borrowed.
pub trait Mapped {
    fn bytes(&self) -> &[u8];
}

/// An error unless `off..off + len` lies within `file_len` bytes.
pub(crate) fn check_range(file_len: u64, off: u64, len: usize) -> Result<(), IoError> {
    if off.checked_add(len as u64).is_some_and(|end| end <= file_len) {
        Ok(())
    } else {
        Err(IoError::Io(io::Error::new(io::ErrorKind::UnexpectedEof, format!("bytes {off}..+{len} are past the end ({file_len} bytes)"))))
    }
}

fn slice_at(b: &[u8], off: u64, len: usize) -> Result<Vec<u8>, IoError> {
    check_range(b.len() as u64, off, len)?;
    Ok(b[off as usize..off as usize + len].to_vec())
}

/// A local file, mmapped.
pub struct MmapFile {
    map: Mmap,
    path: PathBuf,
}

impl MmapFile {
    /// Maps a local file; refuses a file on a network filesystem (see the module docs).
    pub fn open(path: &Path) -> Result<Self> {
        let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        ensure_local(&f).with_context(|| format!("map {}", path.display()))?;
        // SAFETY: the files mapped here are content-named and never modified once written, and
        // the mirror evicts by unlinking (never truncating), so the mapping stays valid.
        let map = unsafe { Mmap::map(&f) }.with_context(|| format!("map {}", path.display()))?;
        Ok(Self { map, path: path.to_owned() })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.map
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for MmapFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MmapFile({}, {} bytes)", self.path.display(), self.map.len())
    }
}

/// An error when `f` lives on a filesystem the kernel doesn't call local (an SMB share).
fn ensure_local(f: &File) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `statfs` is plain old data, filled in by the call.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: a valid descriptor and a valid out-pointer.
        if unsafe { libc::fstatfs(f.as_raw_fd(), &mut st) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        if st.f_flags & libc::MNT_LOCAL as u32 == 0 {
            let fstype: String = st.f_fstypename.iter().take_while(|&&c| c != 0).map(|&c| c as u8 as char).collect();
            anyhow::bail!("the file is on a network filesystem ({fstype}); NAS files are never mmapped");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = f;
    Ok(())
}

impl RangeRead for MmapFile {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.map.len() as u64)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        slice_at(&self.map, off, len)
    }
}

impl Mapped for MmapFile {
    fn bytes(&self) -> &[u8] {
        &self.map
    }
}

/// A NAS file, read with pread through the I/O pool (never mmapped). Its length is taken once at
/// open: content-named files never change.
pub struct PooledFile {
    file: Arc<File>,
    len: u64,
    pool: Arc<IoPool>,
    path: PathBuf,
}

impl PooledFile {
    /// Opens `path` through the pool (open and fstat in one trip).
    pub fn open(pool: &Arc<IoPool>, path: &Path) -> Result<Self, IoError> {
        let (file, len) = pool.open_len(path)?;
        Ok(Self { file, len, pool: pool.clone(), path: path.to_owned() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for PooledFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PooledFile({}, {} bytes)", self.path.display(), self.len)
    }
}

impl RangeRead for PooledFile {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.len)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        check_range(self.len, off, len)?;
        self.pool.read_at(&self.file, off, len)
    }
}

/// A file read with plain positioned reads (no pool, never mapped): for build steps, which may read
/// the NAS directly (they run alone, and a stalled share just stalls the step).
pub struct PlainFile {
    file: File,
    len: u64,
}

impl PlainFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl RangeRead for PlainFile {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.len)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        use crate::sys::PosIo;
        check_range(self.len, off, len)?;
        let mut b = vec![0u8; len];
        self.file.read_exact_at(&mut b, off).map_err(IoError::Io)?;
        Ok(b)
    }
}

/// Bytes in memory (tests, small files read whole).
impl RangeRead for Vec<u8> {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.as_slice().len() as u64)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        slice_at(self, off, len)
    }
}

impl Mapped for Vec<u8> {
    fn bytes(&self) -> &[u8] {
        self
    }
}

impl<T: RangeRead + ?Sized> RangeRead for &T {
    fn len(&self) -> Result<u64, IoError> {
        (**self).len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        (**self).read_at(off, len)
    }
}

impl<T: RangeRead + ?Sized> RangeRead for Arc<T> {
    fn len(&self) -> Result<u64, IoError> {
        (**self).len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        (**self).read_at(off, len)
    }
}

impl<T: RangeRead + ?Sized> RangeRead for Box<T> {
    fn len(&self) -> Result<u64, IoError> {
        (**self).len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        (**self).read_at(off, len)
    }
}

/// The bytes `off..off + len` of another source, as a source of their own (a member stored in a
/// zip, read in place).
pub struct Slice<R> {
    inner: R,
    off: u64,
    len: u64,
}

impl<R: RangeRead> Slice<R> {
    /// An error unless the window lies within `inner`.
    pub fn new(inner: R, off: u64, len: u64) -> Result<Self, IoError> {
        check_range(inner.len()?, off, usize::try_from(len).unwrap_or(usize::MAX))?;
        Ok(Self { inner, off, len })
    }
}

impl<R: RangeRead> RangeRead for Slice<R> {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.len)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        check_range(self.len, off, len)?;
        self.inner.read_at(self.off + off, len)
    }
}

impl<T: Mapped + ?Sized> Mapped for &T {
    fn bytes(&self) -> &[u8] {
        (**self).bytes()
    }
}

impl<T: Mapped + ?Sized> Mapped for Arc<T> {
    fn bytes(&self) -> &[u8] {
        (**self).bytes()
    }
}

impl<T: Mapped + ?Sized> Mapped for Box<T> {
    fn bytes(&self) -> &[u8] {
        (**self).bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn check(r: &dyn RangeRead) {
        assert_eq!(r.len().unwrap(), 10);
        assert_eq!(r.read_at(0, 3).unwrap(), b"012");
        assert_eq!(r.read_at(7, 3).unwrap(), b"789");
        assert_eq!(r.read_at(10, 0).unwrap(), b"");
        assert!(matches!(r.read_at(8, 3), Err(IoError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof));
        assert!(r.read_at(u64::MAX, 2).is_err());
    }

    #[test]
    fn sources() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        let m = MmapFile::open(&p).unwrap();
        check(&m);
        assert_eq!(m.bytes(), b"0123456789");
        check(&b"0123456789".to_vec());
        let pool = IoPool::new(1, Duration::from_secs(2), dir.path().to_owned());
        let f = PooledFile::open(&pool, &p).unwrap();
        check(&f);
        let shared: Arc<dyn RangeRead> = Arc::new(f);
        check(&shared);
        // Offline: reads fail fast with the typed error.
        pool.mark_offline("test");
        assert!(matches!(shared.read_at(0, 1), Err(IoError::Offline)));
        assert!(matches!(PooledFile::open(&pool, &p), Err(IoError::Offline)));
    }

    #[test]
    fn slices() {
        let s = Slice::new(b"xx0123456789yy".to_vec(), 2, 10).unwrap();
        check(&s);
        assert!(Slice::new(b"0123".to_vec(), 2, 3).is_err(), "past the end");
    }

    #[test]
    fn empty_file_maps() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("empty");
        std::fs::write(&p, b"").unwrap();
        let m = MmapFile::open(&p).unwrap();
        assert_eq!(m.len().unwrap(), 0);
        assert_eq!(m.read_at(0, 0).unwrap(), b"");
        assert!(MmapFile::open(&dir.path().join("missing")).is_err());
    }
}
