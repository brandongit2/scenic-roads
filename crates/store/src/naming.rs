//! Content naming (docs/formats.md, Names and hashes). Every file on the NAS is named
//! `<logical>.<hash16>.<ext>` after the BLAKE3 hash of its bytes and is never rewritten: a rebuild
//! that produces the same bytes keeps the same name and uploads nothing, and a reader holding a name
//! knows exactly which bytes it will get.

use anyhow::{bail, ensure, Context, Result};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// Hex digits of the BLAKE3 hash kept in content names.
pub const HASH_LEN: usize = 16;

/// Chunk size for streaming file copies and hashes.
const CHUNK: usize = 4 << 20;

/// BLAKE3 of `bytes`, first 16 hex digits.
pub fn hash16(bytes: &[u8]) -> String {
    hex16(&blake3::hash(bytes))
}

/// `hash16` of a file's contents, streamed.
pub fn hash16_file(path: &Path) -> Result<String> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    hash16_reader(f).with_context(|| format!("read {}", path.display()))
}

/// The first 16 hex digits of a finished BLAKE3 hash.
pub fn hex16(h: &blake3::Hash) -> String {
    h.to_hex()[..HASH_LEN].to_string()
}

fn hash16_reader(mut r: impl Read) -> io::Result<String> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                h.update(&buf[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(hex16(&h.finalize()))
}

/// XXH3-64 of `bytes`: the blob and section checksum (and tile ETag) of packs and sectioned files.
pub fn xxh3(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytes)
}

/// `<logical>.<hash16>.<ext>`.
pub fn content_name(logical: &str, hash16: &str, ext: &str) -> String {
    debug_assert!(valid_logical(logical) && is_hash16(hash16) && valid_ext(ext), "{logical}.{hash16}.{ext}");
    format!("{logical}.{hash16}.{ext}")
}

/// A content name split into its parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentName<'a> {
    /// The logical name, e.g. `layers/roads/hi/6-32-21` (never contains a dot).
    pub logical: &'a str,
    /// 16 lowercase hex digits of the file's BLAKE3 hash.
    pub hash16: &'a str,
    /// The extension, e.g. `pack` (may itself contain dots, as in `json.zst`).
    pub ext: &'a str,
}

/// Splits `<logical>.<hash16>.<ext>`, or None when `name` isn't a well-formed content name. A
/// well-formed name is also a safe relative path: no empty, `.` or `..` components, nothing absolute.
pub fn parse_content_name(name: &str) -> Option<ContentName<'_>> {
    let (logical, rest) = name.split_once('.')?;
    let (hash16, ext) = rest.split_once('.')?;
    (valid_logical(logical) && is_hash16(hash16) && valid_ext(ext)).then_some(ContentName { logical, hash16, ext })
}

/// A logical name: `/`-separated non-empty components, no dot, no NUL, no backslash.
pub fn valid_logical(s: &str) -> bool {
    !s.is_empty() && !s.contains(['.', '\0', '\\']) && s.split('/').all(|c| !c.is_empty())
}

/// An extension: non-empty dot-separated parts, no `/`, NUL or backslash.
fn valid_ext(s: &str) -> bool {
    !s.is_empty() && !s.contains(['/', '\0', '\\']) && s.split('.').all(|c| !c.is_empty())
}

fn is_hash16(s: &str) -> bool {
    s.len() == HASH_LEN && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Where `write_atomic` takes a file's bytes from.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    Bytes(&'a [u8]),
    /// A local file, streamed (it may be larger than memory).
    File(&'a Path),
}

/// Writes `src` under `root` as `<logical>.<hash16>.<ext>` and returns that name (relative to
/// `root`). The bytes go to `<name>.tmp` first, are read back and checked against the hash, and only
/// then renamed into place, so a reader never sees a partial file under a content name. When the
/// name already exists with the right size nothing is written; an existing content-named file is
/// never overwritten (one with the wrong size is an error to resolve by hand, not a file to replace).
///
/// On the NAS this blocks on the share: callers that must stay responsive run it in the I/O pool.
pub fn write_atomic(root: &Path, logical: &str, ext: &str, src: Source<'_>) -> Result<String> {
    ensure!(valid_logical(logical), "bad logical name {logical:?}");
    ensure!(valid_ext(ext), "bad extension {ext:?}");
    let (hash, size) = match src {
        Source::Bytes(b) => (hash16(b), b.len() as u64),
        Source::File(p) => {
            let size = fs::metadata(p).with_context(|| format!("stat {}", p.display()))?.len();
            (hash16_file(p)?, size)
        }
    };
    let name = content_name(logical, &hash, ext);
    let dest = root.join(&name);
    if present(&dest, size)? {
        return Ok(name);
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = TmpFile(tmp_path(&dest));
    write_tmp(&tmp.0, src).with_context(|| format!("write {}", tmp.0.display()))?;
    let (got, got_size) = reread(&tmp.0).with_context(|| format!("read back {}", tmp.0.display()))?;
    ensure!(
        got == hash && got_size == size,
        "{}: read back {got} ({got_size} bytes), expected {hash} ({size} bytes); the source changed or the write was corrupted",
        tmp.0.display()
    );
    match rename_no_replace(&tmp.0, &dest) {
        Ok(()) => {
            tmp.keep();
            Ok(name)
        }
        // Someone else put the same content there meanwhile.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && present(&dest, size)? => Ok(name),
        Err(e) => Err(e).with_context(|| format!("rename into {}", dest.display())),
    }
}

/// Whether `dest` exists with `size` bytes; an error when it exists with another size.
fn present(dest: &Path, size: u64) -> Result<bool> {
    match fs::metadata(dest) {
        Ok(m) if m.is_file() && m.len() == size => Ok(true),
        Ok(m) if !m.is_file() => bail!("{} exists and isn't a file", dest.display()),
        Ok(m) => bail!(
            "{} exists with {} bytes instead of {size}; content-named files are never rewritten, so it must be removed by hand",
            dest.display(),
            m.len()
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("stat {}", dest.display())),
    }
}

fn write_tmp(tmp: &Path, src: Source<'_>) -> io::Result<()> {
    let mut out = OpenOptions::new().write(true).create(true).truncate(true).open(tmp)?;
    match src {
        Source::Bytes(b) => out.write_all(b)?,
        Source::File(p) => {
            // A plain copy: fs::copy would also carry extended attributes over to the share.
            let mut inp = File::open(p)?;
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = match inp.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                };
                out.write_all(&buf[..n])?;
            }
        }
    }
    out.sync_all()
}

/// Hash and size of a just-written file, read back past the client's cache where the OS allows it.
fn reread(path: &Path) -> io::Result<(String, u64)> {
    let f = File::open(path)?;
    no_cache(&f);
    let size = f.metadata()?.len();
    Ok((hash16_reader(f)?, size))
}

/// Asks the OS not to serve this descriptor's reads from its cache (best effort; macOS only).
fn no_cache(f: &File) {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: plain fcntl on a descriptor we own; failure only means reads may be cached.
        unsafe {
            libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1);
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = f;
}

/// `<path>.tmp`.
pub(crate) fn tmp_path(path: &Path) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(".tmp");
    PathBuf::from(s)
}

/// Renames `from` to `to` unless `to` exists (then `AlreadyExists`). Atomic where the filesystem
/// supports exclusive renames (APFS); elsewhere (some SMB servers) it checks first, which only a
/// concurrent writer of the same name could race — and there is one writer (plan §3).
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let f = CString::new(from.as_os_str().as_bytes())?;
        let t = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both arguments are NUL-terminated paths that outlive the call.
        if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::AlreadyExists {
            return Err(e);
        }
        // Any other error may only mean the filesystem can't rename exclusively; a real failure
        // shows again below.
    }
    match fs::symlink_metadata(to) {
        Ok(_) => Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists", to.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::rename(from, to),
        Err(e) => Err(e),
    }
}

/// Removes a temporary file when dropped, unless kept.
struct TmpFile(PathBuf);

impl TmpFile {
    fn keep(self) {
        std::mem::forget(self);
    }
}

impl Drop for TmpFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Writes `bytes` to `path` through a temporary file and a rename, replacing what was there (for
/// small local state files, never for content-named ones), with permission bits `mode` if given.
/// The temporary name is unique to the call (`<path>.<pid>-<n>.tmp`), so concurrent writers of
/// the same file can't mix their bytes: the last rename wins whole.
pub(crate) fn replace_file(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = OsString::from(path.as_os_str());
    name.push(format!(".{}-{}.tmp", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let tmp = TmpFile(PathBuf::from(name));
    let mut f = File::create(&tmp.0)?;
    f.write_all(bytes)?;
    if let Some(mode) = mode {
        f.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    f.sync_all()?;
    fs::rename(&tmp.0, path)?;
    tmp.keep();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes() {
        // BLAKE3 test vector for the empty input.
        assert_eq!(hash16(b""), "af1349b9f5f9a1a6");
        assert_eq!(hash16(b"abc"), &blake3::hash(b"abc").to_hex()[..16]);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let big: Vec<u8> = (0..(CHUNK * 2 + 123)).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(&p, &big).unwrap();
        assert_eq!(hash16_file(&p).unwrap(), hash16(&big));
        assert_eq!(xxh3(b"abc"), xxhash_rust::xxh3::xxh3_64(b"abc"));
    }

    #[test]
    fn names() {
        let n = content_name("layers/roads/hi/6-32-21", "0a1b2c3d4e5f6071", "pack");
        assert_eq!(n, "layers/roads/hi/6-32-21.0a1b2c3d4e5f6071.pack");
        let c = parse_content_name(&n).unwrap();
        assert_eq!((c.logical, c.hash16, c.ext), ("layers/roads/hi/6-32-21", "0a1b2c3d4e5f6071", "pack"));
        let c = parse_content_name("global/pois.0123456789abcdef.json.zst").unwrap();
        assert_eq!((c.logical, c.ext), ("global/pois", "json.zst"));
        for bad in [
            "",
            "a",
            "a.0123456789abcdef",
            "a.0123456789abcdef.",
            "a.0123456789ABCDEF.pack",
            "a.0123456789abcde.pack",
            "/a.0123456789abcdef.pack",
            "a//b.0123456789abcdef.pack",
            "../a.0123456789abcdef.pack",
            "a/.0123456789abcdef.pack",
            "a.0123456789abcdef.x/y",
            "a.0123456789abcdef..y",
        ] {
            assert!(parse_content_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn atomic_writes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let name = write_atomic(root, "layers/roads/root", "pack", Source::Bytes(b"hello")).unwrap();
        assert_eq!(name, format!("layers/roads/root.{}.pack", hash16(b"hello")));
        assert_eq!(fs::read(root.join(&name)).unwrap(), b"hello");
        assert!(!tmp_path(&root.join(&name)).exists());

        // Same bytes again: same name, nothing rewritten.
        let mtime = fs::metadata(root.join(&name)).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(write_atomic(root, "layers/roads/root", "pack", Source::Bytes(b"hello")).unwrap(), name);
        assert_eq!(fs::metadata(root.join(&name)).unwrap().modified().unwrap(), mtime);

        // From a file, larger than one copy chunk.
        let src = root.join("src.bin");
        let big: Vec<u8> = (0..(CHUNK + 999)).map(|i| (i % 253) as u8).collect();
        fs::write(&src, &big).unwrap();
        let name2 = write_atomic(root, "base/6-1-2", "sect", Source::File(&src)).unwrap();
        assert_eq!(fs::read(root.join(&name2)).unwrap(), big);

        // A content-named file with the wrong size is never replaced.
        let bogus = root.join(content_name("x", &hash16(b"right"), "bin"));
        fs::write(&bogus, b"wrong size").unwrap();
        let err = write_atomic(root, "x", "bin", Source::Bytes(b"right")).unwrap_err();
        assert!(err.to_string().contains("never rewritten"), "{err:#}");
        assert_eq!(fs::read(&bogus).unwrap(), b"wrong size");

        // Bad logical names.
        assert!(write_atomic(root, "a.b", "bin", Source::Bytes(b"x")).is_err());
        assert!(write_atomic(root, "../a", "bin", Source::Bytes(b"x")).is_err());
        assert!(write_atomic(root, "/abs", "bin", Source::Bytes(b"x")).is_err());
    }

    #[test]
    fn rename_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let e = rename_no_replace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&b).unwrap(), b"b");
        fs::remove_file(&b).unwrap();
        rename_no_replace(&a, &b).unwrap();
        assert_eq!(fs::read(&b).unwrap(), b"a");
        assert!(!a.exists());
    }
}
