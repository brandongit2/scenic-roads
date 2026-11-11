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
pub const CHUNK: usize = 4 << 20;

/// Writes of one file `write_atomic` makes before it gives up on a share whose read-backs keep
/// differing from a source that hasn't changed.
const WRITE_TRIES: u32 = 3;

/// The bytes the streamed hashes and copies here have read and written, all told: a long upload
/// says how far it is by them (pipeline's `Out::put_file_with`).
static MOVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The bytes moved so far (`MOVED`).
pub fn moved() -> u64 {
    MOVED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Counts `n` bytes moved (`MOVED`): the callers' own streamed reads too (an upload's checksum).
/// Also into this thread's own count, if it has one (`count_here_into`).
pub fn count_moved(n: usize) {
    MOVED.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
    HERE.with(|h| {
        if let Some(c) = &*h.borrow() {
            c.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

thread_local! {
    /// This thread's own count of bytes moved (`count_here_into`).
    static HERE: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicU64>>> =
        const { std::cell::RefCell::new(None) };
}

/// From now on, the bytes this thread moves (`count_moved`) are counted into `to` as well: what a
/// thread waiting on its uploads tells moving from stuck by (pipeline's `Out::settle`); `MOVED`
/// counts every other thread's too.
pub fn count_here_into(to: std::sync::Arc<std::sync::atomic::AtomicU64>) {
    HERE.with(|h| *h.borrow_mut() = Some(to));
}

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

/// The first 16 hex digits of the BLAKE3 hash of what `r` reads.
pub fn hash16_reader(mut r: impl Read) -> io::Result<String> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                h.update(&buf[..n]);
                count_moved(n);
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

/// An extension: non-empty dot-separated parts, no `/`, NUL or backslash, and not a tombstone of the
/// GC before docs/maplog.md §8 step 8b (`TOMB`, which no sweep takes for a content-named file).
fn valid_ext(s: &str) -> bool {
    !s.is_empty() && !s.contains(['/', '\0', '\\']) && s.split('.').all(|c| !c.is_empty()) && !s.ends_with(TOMB)
}

/// What the GC before step 8b renamed a content-named file to before it removed it (removed unless
/// a job had touched it meanwhile): one left by a run stopped between, its bytes perhaps a reused
/// file's, stays on the share, never taken for a content-named file, so no sweep removes it.
pub const TOMB: &str = ".gc-tomb";

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
/// `root`). The bytes go to `<name>.tmp` first, are read back past the client's cache and checked
/// against the hash, and only then renamed into place, so a reader never sees a partial or damaged
/// file under a content name. A read-back that differs, its source unchanged, is written again,
/// `WRITE_TRIES` writes at most (task #1140: on the NAS, a write in a session reconnect of the
/// Mac's SMB client read back other bytes of the same length); a source changed since it was
/// hashed fails at once. Each write again is a line on stderr. When the
/// name already exists with the right size nothing is written; an existing content-named file is
/// never overwritten (one with the wrong size is an error to resolve by hand, not a file to replace).
///
/// On the NAS this blocks on the share: callers that must stay responsive run it in the I/O pool.
pub fn write_atomic(root: &crate::nasfs::NasRoot, logical: &str, ext: &str, src: Source<'_>) -> Result<String> {
    write_atomic_as(root, logical, ext, src, None)
}

/// `write_atomic` of a local file whose `hash16` and length the caller has just worked out (an
/// upload hashing it once for its name and its checksum together: pipeline's `Out::put_file`), so
/// it isn't read for them again. The read-back is checked against them as against its own: a wrong
/// hash, or a file changed since, fails (the source changed since it was hashed), and leaves nothing.
pub fn write_atomic_hashed(
    root: &crate::nasfs::NasRoot,
    logical: &str,
    ext: &str,
    file: &Path,
    hash: &str,
    size: u64,
) -> Result<String> {
    ensure!(is_hash16(hash), "bad hash {hash:?}");
    write_atomic_as(root, logical, ext, Source::File(file), Some((hash.to_string(), size)))
}

fn write_atomic_as(
    root: &crate::nasfs::NasRoot,
    logical: &str,
    ext: &str,
    src: Source<'_>,
    known: Option<(String, u64)>,
) -> Result<String> {
    let _w = crate::waits::nas();
    ensure!(valid_logical(logical), "bad logical name {logical:?}");
    ensure!(valid_ext(ext), "bad extension {ext:?}");
    let (hash, size) = match (known, src) {
        (Some(k), _) => k,
        (None, Source::Bytes(b)) => (hash16(b), b.len() as u64),
        (None, Source::File(p)) => {
            let size = fs::metadata(p).with_context(|| format!("stat {}", p.display()))?.len();
            (hash16_file(p)?, size)
        }
    };
    let name = content_name(logical, &hash, ext);
    // (Raw: its own temporary name, an exclusive rename, a read-back past the cache; its bytes
    // counted as they're written (`write_tmp`), the file by its caller as it lands:
    // tests/nas_access.rs.)
    let dest = root.join(&name).unchecked_path();
    if present(&dest, size)? {
        return Ok(name);
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = TmpFile(tmp_path(&dest));
    let mut writes = 0;
    loop {
        write_tmp(&tmp.0, src).with_context(|| format!("write {}", tmp.0.display()))?;
        writes += 1;
        let (got, got_size) = reread(&tmp.0).with_context(|| format!("read back {}", tmp.0.display()))?;
        if got == hash && got_size == size {
            break;
        }
        // Its source unchanged, written again, whichever went wrong, the write or its read-back
        // (task #1140: on the NAS, in a session reconnect of the Mac's SMB client): a file is
        // taken only on a read-back that matches the write just made.
        let what = format!("{}: read back {got} ({got_size} bytes), expected {hash} ({size} bytes)", tmp.0.display());
        if let Source::File(p) = src {
            // (Bytes are borrowed for the call: they can't change.)
            let now = fs::metadata(p).with_context(|| format!("stat {}", p.display()))?.len();
            let now = (hash16_file(p)?, now);
            ensure!(
                now == (hash.clone(), size),
                "{what}; the source changed since it was hashed (now {} with {} bytes)",
                now.0,
                now.1
            );
        }
        ensure!(writes < WRITE_TRIES, "{what}, the source unchanged; {writes} writes read back wrong running");
        eprintln!("{what}, the source unchanged: written again");
    }
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
    // (Past the client's cache: a write returns once its bytes went to the share, so the bytes
    // counted moved are the share's and the closing sync has no backlog to flush unseen: an
    // upload's progress is judged by them, pipeline's `Out::settle`.)
    no_cache(&out)?;
    // (Each write's bytes counted the NAS's as it goes, so a long upload's job is seen moving by
    // its beats, task #990; the file made is its caller's to count, as it lands.)
    match src {
        Source::Bytes(b) => {
            for c in b.chunks(CHUNK) {
                out.write_all(c)?;
                crate::nasio::written(c.len() as u64);
            }
        }
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
                crate::nasio::written(n as u64);
                count_moved(n);
            }
        }
    }
    out.sync_all()?;
    #[cfg(test)]
    if faults::take() {
        faults::damage(tmp)?;
    }
    Ok(())
}

/// Hash and size of a just-written file, read back past the client's cache: on a fresh descriptor
/// with `F_NOCACHE`, which the Mac's SMB client reads from the server (the cached range dropped
/// first), never from its pages.
fn reread(path: &Path) -> io::Result<(String, u64)> {
    let f = File::open(path)?;
    no_cache(&f)?;
    let size = f.metadata()?.len();
    Ok((hash16_reader(f)?, size))
}

/// Tests' fault in `write_atomic`'s writes: this thread's next writes left damaged.
#[cfg(test)]
mod faults {
    thread_local! {
        static DAMAGED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    /// This thread's next `n` writes are left damaged.
    pub fn damage_next(n: u32) {
        DAMAGED.with(|d| d.set(n));
    }

    /// Whether this write is to be damaged (then one fewer is).
    pub fn take() -> bool {
        DAMAGED.with(|d| {
            let n = d.get();
            d.set(n.saturating_sub(1));
            n > 0
        })
    }

    /// Flips the byte in the middle of `path`.
    pub fn damage(path: &std::path::Path) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;
        let f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let at = f.metadata()?.len() / 2;
        let mut b = [0u8];
        f.read_exact_at(&mut b, at)?;
        f.write_all_at(&[b[0] ^ 0xff], at)
    }
}

/// Asks the OS not to serve this descriptor's reads and writes through its cache (`F_NOCACHE`,
/// macOS only; elsewhere nothing): an error when it refuses, as a read-back through the cache
/// checks nothing.
fn no_cache(f: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: plain fcntl on a descriptor we own.
        if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = f;
    Ok(())
}

/// `<path>.<host>-<pid>.tmp`: this Mac's and this process's, so two writers of the same name (the
/// two Macs building the same unit) never share one. GC sweeps those abandoned.
pub(crate) fn tmp_path(path: &Path) -> PathBuf {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let host = HOST
        .get_or_init(|| crate::sys::hostname().map(|h| h.replace(['.', '/'], "-")).unwrap_or_else(|| "host".into()));
    let mut s = OsString::from(path.as_os_str());
    s.push(format!(".{host}-{}.tmp", crate::sys::pid()));
    PathBuf::from(s)
}

/// Renames `from` to `to` unless `to` exists (then `AlreadyExists`). Atomic where the filesystem
/// supports exclusive renames (APFS); elsewhere (some SMB servers) it checks first, which only a
/// concurrent writer of the same name could race: then the rename finds the name taken, and the
/// copy there (same name, same content) is checked for its size instead.
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
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
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = OsString::from(path.as_os_str());
    name.push(format!(".{}-{}.tmp", crate::sys::pid(), N.fetch_add(1, Ordering::Relaxed)));
    let tmp = TmpFile(PathBuf::from(name));
    let mut f = File::create(&tmp.0)?;
    f.write_all(bytes)?;
    if let Some(mode) = mode {
        crate::sys::set_mode(&f, mode)?;
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
        let nas = crate::nasfs::NasRoot::local_for_test(dir.path());
        let (root, nr) = (dir.path(), &nas);
        let name = write_atomic(nr, "layers/roads/root", "pack", Source::Bytes(b"hello")).unwrap();
        assert_eq!(name, format!("layers/roads/root.{}.pack", hash16(b"hello")));
        assert_eq!(fs::read(root.join(&name)).unwrap(), b"hello");
        assert!(!tmp_path(&root.join(&name)).exists());

        // Same bytes again: same name, nothing rewritten.
        let mtime = fs::metadata(root.join(&name)).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20)); // (absence)
        assert_eq!(write_atomic(nr, "layers/roads/root", "pack", Source::Bytes(b"hello")).unwrap(), name);
        assert_eq!(fs::metadata(root.join(&name)).unwrap().modified().unwrap(), mtime);

        // From a file, larger than one copy chunk.
        let src = root.join("src.bin");
        let big: Vec<u8> = (0..(CHUNK + 999)).map(|i| (i % 253) as u8).collect();
        fs::write(&src, &big).unwrap();
        let name2 = write_atomic(nr, "base/6-1-2", "sect", Source::File(&src)).unwrap();
        assert_eq!(fs::read(root.join(&name2)).unwrap(), big);

        // A content-named file with the wrong size is never replaced.
        let bogus = root.join(content_name("x", &hash16(b"right"), "bin"));
        fs::write(&bogus, b"wrong size").unwrap();
        let err = write_atomic(nr, "x", "bin", Source::Bytes(b"right")).unwrap_err();
        assert!(err.to_string().contains("never rewritten"), "{err:#}");
        assert_eq!(fs::read(&bogus).unwrap(), b"wrong size");

        // Bad logical names.
        assert!(write_atomic(nr, "a.b", "bin", Source::Bytes(b"x")).is_err());
        assert!(write_atomic(nr, "../a", "bin", Source::Bytes(b"x")).is_err());
        assert!(write_atomic(nr, "/abs", "bin", Source::Bytes(b"x")).is_err());
    }

    #[test]
    fn a_hash_given_is_checked_as_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        let nr = &crate::nasfs::NasRoot::local_for_test(&root);
        let src = dir.path().join("src.bin");
        fs::write(&src, b"the bytes").unwrap();
        // A wrong hash (or the file changed since it was hashed): the read-back doesn't match it,
        // and nothing is left, under its name or a temporary one.
        let e = write_atomic_hashed(nr, "h", "bin", &src, &hash16(b"other bytes"), 9).unwrap_err();
        assert!(format!("{e:#}").contains("the source changed since it was hashed"), "{e:#}");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        // A wrong length likewise.
        assert!(write_atomic_hashed(nr, "h", "bin", &src, &hash16(b"the bytes"), 8).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        // The right ones: the name write_atomic gives.
        let name = write_atomic_hashed(nr, "h", "bin", &src, &hash16(b"the bytes"), 9).unwrap();
        assert_eq!(name, write_atomic(nr, "h", "bin", Source::File(&src)).unwrap());
        assert_eq!(fs::read(root.join(&name)).unwrap(), b"the bytes");
    }

    #[test]
    fn a_write_read_back_wrong_is_written_again() {
        use faults::damage_next;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        let nr = &crate::nasfs::NasRoot::local_for_test(&root);
        let src = dir.path().join("src.bin");
        let big: Vec<u8> = (0..(CHUNK + 999)).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &big).unwrap();
        let landed = |name: &str| fs::read(root.join(name)).unwrap() == big;
        let left = || fs::read_dir(&root).unwrap().count();

        // A write read back wrong, its source unchanged: written again, and the next lands whole;
        // all but the last of WRITE_TRIES read back wrong, the last lands. From bytes as from a file.
        damage_next(1);
        let name = write_atomic(nr, "a", "bin", Source::File(&src)).unwrap();
        assert!(landed(&name));
        fs::remove_file(root.join(&name)).unwrap();
        damage_next(WRITE_TRIES - 1);
        assert_eq!(write_atomic(nr, "a", "bin", Source::Bytes(&big)).unwrap(), name);
        assert!(landed(&name));
        fs::remove_file(root.join(&name)).unwrap();

        // Every write read back wrong: it gives up after WRITE_TRIES of them, leaving nothing.
        damage_next(WRITE_TRIES);
        let e = write_atomic(nr, "a", "bin", Source::File(&src)).unwrap_err();
        assert!(format!("{e:#}").contains(&format!("{WRITE_TRIES} writes read back wrong running")), "{e:#}");
        assert_eq!(left(), 0);
        // (Its faults all taken: the next write lands at once.)
        assert!(landed(&write_atomic(nr, "a", "bin", Source::File(&src)).unwrap()));

        // A source changed since it was hashed is never written again (the hash given is stale).
        fs::remove_dir_all(&root).unwrap();
        fs::create_dir(&root).unwrap();
        let stale = hash16(&big);
        fs::write(&src, big.iter().map(|b| b ^ 1).collect::<Vec<u8>>()).unwrap();
        let e = write_atomic_hashed(nr, "a", "bin", &src, &stale, big.len() as u64).unwrap_err();
        assert!(format!("{e:#}").contains("the source changed since it was hashed"), "{e:#}");
        assert_eq!(left(), 0);
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
