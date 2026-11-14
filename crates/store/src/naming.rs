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

/// The most memory a `write_atomic` of a local file holds at once (bytes): its copy's read buffer
/// and, beside it, the piece handed to the share's write call (`write_tmp`: each piece its own, the
/// call run on a watched thread of its own); its hash's read and its read-back hold one `CHUNK`
/// each, in turn.
pub const HELD: usize = 2 * CHUNK;

use crate::whole::WRITE_TRIES;

/// The bytes the streamed hashes and copies here have read and written, all told: a long upload
/// says how far it is by them (pipeline's `Out::put_file_with`).
static MOVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The bytes moved so far (`MOVED`).
pub fn moved() -> u64 {
    MOVED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Counts `n` bytes moved (`MOVED`), and into this thread's own count, if it has one
/// (`count_here_into`).
fn count_moved(n: usize) {
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

/// `xxh3` of the text `write` writes, hashed as it is written and never held whole.
pub fn xxh3_text(write: impl FnOnce(&mut dyn std::fmt::Write) -> std::fmt::Result) -> u64 {
    struct Hashed(xxhash_rust::xxh3::Xxh3);
    impl std::fmt::Write for Hashed {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.update(s.as_bytes());
            Ok(())
        }
    }
    let mut h = Hashed(xxhash_rust::xxh3::Xxh3::new());
    // (The hasher's `write_str` never fails.)
    write(&mut h).ok();
    h.0.digest()
}

/// Whether `text` is exactly what `args` prints, compared as it is printed (no text made).
pub fn prints_as(text: &str, args: std::fmt::Arguments) -> bool {
    struct Left<'a>(&'a str);
    impl std::fmt::Write for Left<'_> {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0 = self.0.strip_prefix(s).ok_or(std::fmt::Error)?;
            Ok(())
        }
    }
    let mut left = Left(text);
    std::fmt::write(&mut left, args).is_ok() && left.0.is_empty()
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
pub enum WriteFrom<'a> {
    Bytes(&'a [u8]),
    /// A local file, streamed (it may be larger than memory).
    File(&'a Path),
}

/// Writes `src` under `root` as `<logical>.<hash16>.<ext>` and returns that name (relative to
/// `root`). The bytes go to `<name>.tmp` first, are read back past the client's cache and checked
/// against the hash, and only then renamed into place, so a reader never sees a partial or damaged
/// file under a content name. The temporary file is closed with its close's result checked
/// (crate::sys::close). A read-back that differs, or a write lost to the share for a moment (an
/// ETIMEDOUT, EIO or dropped session at its open, a write, its sync or its close:
/// crate::whole::lost_write), its source unchanged, is written again, `WRITE_TRIES` writes at most
/// (task #1140: on the NAS, a write in a session reconnect of the Mac's SMB client read back other
/// bytes of the same length; task #1142: a soft mount's reconnect fails what's in flight with
/// ETIMEDOUT); a source changed since it was hashed fails at once, and so does a call past its
/// deadline (held, its temporary name with it). Each write again is a line on stderr. The
/// read-back's bytes are counted the NAS's reads (crate::nasio::read_back). When the
/// name already exists with the right size nothing is written; an existing content-named file is
/// never overwritten (one with the wrong size is an error to resolve by hand, not a file to replace).
///
/// On the NAS this blocks on the share: callers that must stay responsive run it in the I/O pool.
/// Each of its calls on the share (the stat, the folder made, the open, each `CHUNK` written, the
/// sync, the read-back, the rename, the temporary file's removal) is a crate::deadline call
/// (`LONG`, a read-back counted from its last byte), the temporary name held while one is held: a
/// late write lands only in this process's temporary name, which a later upload of the same name
/// can't write meanwhile, and which GC sweeps once abandoned; a late exclusive rename lands only
/// the same content-named bytes.
pub fn write_atomic(root: &crate::nasfs::NasRoot, logical: &str, ext: &str, src: WriteFrom<'_>) -> Result<String> {
    write_atomic_as(root, logical, ext, src, None)
}

/// `write_atomic` of a local file whose `hash16` and length the caller has just worked out (an
/// upload hashing it for its name: buildstore's `Out::put_file`), so it isn't read for them again.
/// The read-back is checked against them as against its own: a wrong hash, or a file changed since,
/// fails (the source changed since it was hashed), and leaves nothing.
pub fn write_atomic_hashed(
    root: &crate::nasfs::NasRoot,
    logical: &str,
    ext: &str,
    file: &Path,
    hash: &str,
    size: u64,
) -> Result<String> {
    ensure!(is_hash16(hash), "bad hash {hash:?}");
    write_atomic_as(root, logical, ext, WriteFrom::File(file), Some((hash.to_string(), size)))
}

fn write_atomic_as(
    root: &crate::nasfs::NasRoot,
    logical: &str,
    ext: &str,
    src: WriteFrom<'_>,
    known: Option<(String, u64)>,
) -> Result<String> {
    let _w = crate::waits::nas();
    ensure!(valid_logical(logical), "bad logical name {logical:?}");
    ensure!(valid_ext(ext), "bad extension {ext:?}");
    let (hash, size) = match (known, src) {
        (Some(k), _) => k,
        (None, WriteFrom::Bytes(b)) => (hash16(b), b.len() as u64),
        (None, WriteFrom::File(p)) => {
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
        let d = dir.to_path_buf();
        on_nas(dir, "make", vec![d.clone()], move |_| fs::create_dir_all(&d))
            .with_context(|| format!("create {}", dir.display()))?;
    }
    let tmp = TmpFile(tmp_path(&dest));
    let mut writes = 0;
    loop {
        writes += 1;
        let what = match write_tmp(&tmp.0, src) {
            Ok(()) => {
                let (got, got_size) = reread(&tmp.0).with_context(|| format!("read back {}", tmp.0.display()))?;
                if got == hash && got_size == size {
                    break;
                }
                format!("{}: read back {got} ({got_size} bytes), expected {hash} ({size} bytes)", tmp.0.display())
            }
            // (Lost to the share for a moment, its close's error too, task #1142: written again. A
            // call past its deadline isn't: it's held, the temporary name with it.)
            Err(Failed::Share(e)) if crate::whole::lost_write(&e) => format!("{}: {e}", tmp.0.display()),
            Err(Failed::Share(e) | Failed::Source(e)) => {
                return Err(e).with_context(|| format!("write {}", tmp.0.display()))
            }
        };
        // Its source unchanged, written again, whichever went wrong, the write (an error at its
        // write, sync or close) or its read-back (task #1140: on the NAS, in a session reconnect
        // of the Mac's SMB client): a file is taken only on a read-back that matches the write
        // just made.
        if let WriteFrom::File(p) = src {
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
        ensure!(writes < WRITE_TRIES, "{what}, the source unchanged; {writes} writes lost or read back wrong running");
        eprintln!("{what}, the source unchanged: written again");
    }
    let (from, to) = (tmp.0.clone(), dest.clone());
    match on_nas(&dest, "rename", vec![from.clone(), to.clone()], move |_| rename_no_replace(&from, &to)) {
        Ok(()) => {
            tmp.keep();
            Ok(name)
        }
        // Someone else put the same content there meanwhile.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists && present(&dest, size)? => Ok(name),
        Err(e) => Err(e).with_context(|| format!("rename into {}", dest.display())),
    }
}

/// `f`, a call on the share at `on` (`op` naming it), as a crate::deadline call (`LONG`): one that
/// changes the share names the paths it changes (`changes`).
fn on_nas<T: Send + 'static>(
    on: &Path,
    op: &'static str,
    changes: Vec<PathBuf>,
    f: impl FnOnce(&crate::deadline::Progress) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    crate::deadline::watched(&on.display(), &op, crate::deadline::LONG, changes, f)
}

/// Whether `dest` exists with `size` bytes; an error when it exists with another size.
fn present(dest: &Path, size: u64) -> Result<bool> {
    let d = dest.to_path_buf();
    match on_nas(dest, "stat", Vec::new(), move |_| fs::metadata(&d)) {
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

/// Where a temporary file's write failed: on the share (its open, a write, its sync, its close), or
/// reading its local source.
enum Failed {
    Share(io::Error),
    Source(io::Error),
}

/// Writes `src` to `tmp`, from its open to its close, the close's result checked (crate::sys::close:
/// where the Mac's SMB client gives a write's last error).
fn write_tmp(tmp: &Path, src: WriteFrom<'_>) -> std::result::Result<(), Failed> {
    let t = tmp.to_path_buf();
    let opened = on_nas(tmp, "open", vec![t.clone()], move |_| {
        let out = OpenOptions::new().write(true).create(true).truncate(true).open(&t)?;
        // (Past the client's cache: a write returns once its bytes went to the share, so the
        // bytes counted moved are the share's and the closing sync has no backlog to flush unseen:
        // an upload's progress is judged by them, pipeline's `Out::settle`.)
        no_cache(&out)?;
        Ok(out)
    });
    // (The file goes into each call and comes back with its answer; a call that fails keeps it,
    // closed unchecked as the write is failed anyway.)
    let mut out = Some(opened.map_err(Failed::Share)?);
    let mut on_file = |op: &'static str, f: Box<dyn FnOnce(&File) -> io::Result<()> + Send>| {
        let (file, t) = (out.take().expect("the temporary file"), tmp.to_path_buf());
        let file = on_nas(tmp, op, vec![t], move |_| f(&file).map(|()| file)).map_err(Failed::Share)?;
        out = Some(file);
        Ok(())
    };
    // (Each piece a call of its own, the piece's bytes its own: never the whole upload copied.
    // Each write's bytes counted the NAS's as it goes, so a long upload's job is seen moving by
    // its beats, task #990; the file made is its caller's to count, as it lands.)
    // (A file's piece copied beside its read buffer: `HELD`.)
    let mut put = |c: &[u8]| -> std::result::Result<(), Failed> {
        let owned = c.to_vec();
        on_file("write", Box::new(move |mut f| f.write_all(&owned)))?;
        crate::nasio::written(c.len() as u64);
        Ok(())
    };
    match src {
        WriteFrom::Bytes(b) => {
            for c in b.chunks(CHUNK) {
                put(c)?;
            }
        }
        WriteFrom::File(p) => {
            // A plain copy: fs::copy would also carry extended attributes over to the share.
            let mut inp = File::open(p).map_err(Failed::Source)?;
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = match inp.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(Failed::Source(e)),
                };
                put(&buf[..n])?;
                count_moved(n);
            }
        }
    }
    on_file("sync", Box::new(|f| f.sync_all()))?;
    let (file, t) = (out.take().expect("the temporary file"), tmp.to_path_buf());
    on_nas(tmp, "close", vec![t.clone()], move |_| crate::sys::close(file, &t)).map_err(Failed::Share)?;
    #[cfg(test)]
    if faults::take() {
        faults::damage(tmp).map_err(Failed::Share)?;
    }
    Ok(())
}

/// Hash and size of a just-written file, read back past the client's cache: on a fresh descriptor
/// with `F_NOCACHE`, which the Mac's SMB client reads from the server (the cached range dropped
/// first), never from its pages. Its bytes counted read from the NAS as they come
/// (crate::nasio::read_back: in the job's NAS reads, and its NAS weight, task #1142).
fn reread(path: &Path) -> io::Result<(String, u64)> {
    let p = path.to_path_buf();
    on_nas(path, "read back", Vec::new(), move |m| {
        let f = File::open(&p)?;
        no_cache(&f)?;
        let size = f.metadata()?.len();
        Ok((hash16_reader(Moving(f, m))?, size))
    })
}

/// A read-back's reader: each read's bytes said moved (crate::deadline: a long read-back that goes
/// on never ends for its length), and counted read back (crate::nasio::read_back).
struct Moving<'a, R>(R, &'a crate::deadline::Progress);

impl<R: Read> Read for Moving<'_, R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = self.0.read(b)?;
        self.1.add(n as u64);
        crate::nasio::read_back(n as u64);
        Ok(n)
    }
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
        // (A call within its deadline: one held, or refused, leaves the name for GC's sweep.)
        let p = self.0.clone();
        let _ = on_nas(&self.0, "remove", vec![p.clone()], move |_| fs::remove_file(&p));
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
    crate::sys::close(f, &tmp.0)?;
    fs::rename(&tmp.0, path)?;
    tmp.keep();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text hashed as it is written has the hash of the whole text; a text is what a format
    /// prints only when it is all of it, to the letter.
    #[test]
    fn a_text_is_hashed_and_compared_as_written() {
        let whole = "lake-a (3, 14) 0123\nlake-b (60, 2) 4567";
        let streamed = xxh3_text(|w| {
            write!(w, "lake-a {:?} 0123", (3, 14))?;
            write!(w, "\nlake-b {:?} {}", (60, 2), 4567)
        });
        assert_eq!(streamed, xxh3(whole.as_bytes()));
        assert_eq!(xxh3_text(|_| Ok(())), xxh3(b""));
        assert!(prints_as("6-3-14", format_args!("6-{}-{}", 3, 14)));
        for other in ["6-03-14", "6-3-140", "6-3-1", "6-3-14 ", ""] {
            assert!(!prints_as(other, format_args!("6-{}-{}", 3, 14)), "{other}");
        }
        assert!(prints_as("", format_args!("")));
    }

    /// Hashed as written is the whole text's hash for any text cut anywhere (lengths across xxh3's
    /// short, middle and streamed paths and its buffer's edges), and for what every kind of format
    /// writes (floats, widths, fills, escapes, debug forms) as `format!` writes it; a text is what a
    /// format prints exactly when `format!`'s text is it.
    #[test]
    fn written_is_formatted_for_any_text_and_format() {
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        const CHARS: [char; 8] = ['a', '/', ' ', '\n', '"', 'é', '\u{1F600}', '0'];
        for len in (0..=1100).chain([2047, 2048, 2049, 4096, 10_000]) {
            let text: String = (0..len).map(|_| CHARS[(next() % 8) as usize]).collect();
            let mut at: Vec<usize> = (0..next() % 7).map(|_| (next() as usize) % (text.len() + 1)).collect();
            at.retain(|&i| text.is_char_boundary(i));
            at.sort_unstable();
            let streamed = xxh3_text(|w| {
                let mut from = 0;
                for &i in at.iter().chain([&text.len()]) {
                    w.write_str(&text[from..i])?;
                    from = i;
                }
                Ok(())
            });
            assert_eq!(streamed, xxh3(text.as_bytes()), "{len} cut at {at:?}");
        }
        for _ in 0..2000 {
            let (f, i, u) = (f64::from_bits(next()), next() as i64, next() as u32);
            let t = (u, (next() % 100_000) as u32);
            let word: String = (0..next() % 12).map(|_| CHARS[(next() % 8) as usize]).collect();
            let w = (next() % 20) as usize;
            let whole = format!(
                "{f} {f:?} {f:.3} {f:e} {i:>w$} {i:+08} {u:x} {u:#010b} {t:?} {word:?} {word:^w$} {word:-<9} {:?}",
                (word.as_str(), f)
            );
            let streamed = xxh3_text(|o| {
                write!(
                    o,
                    "{f} {f:?} {f:.3} {f:e} {i:>w$} {i:+08} {u:x} {u:#010b} {t:?} {word:?} {word:^w$} {word:-<9} {:?}",
                    (word.as_str(), f)
                )
            });
            assert_eq!(streamed, xxh3(whole.as_bytes()), "{whole}");
            let printed = format!("6-{}-{} {word}", t.0, t.1);
            let mut others = vec![printed.clone(), String::new(), format!("{printed} "), format!("0{printed}")];
            for k in [0, printed.len() / 2, printed.len().saturating_sub(1)] {
                others.extend(printed.get(..k).map(str::to_string));
                others.extend(printed.get(k..).map(str::to_string));
            }
            others.push(printed.replacen('-', "--", 1));
            for o in others {
                assert_eq!(
                    prints_as(&o, format_args!("6-{}-{} {word}", t.0, t.1)),
                    o == printed,
                    "{o:?} against {printed:?}"
                );
            }
        }
    }

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
        let name = write_atomic(nr, "layers/roads/root", "pack", WriteFrom::Bytes(b"hello")).unwrap();
        assert_eq!(name, format!("layers/roads/root.{}.pack", hash16(b"hello")));
        assert_eq!(fs::read(root.join(&name)).unwrap(), b"hello");
        assert!(!tmp_path(&root.join(&name)).exists());

        // Same bytes again: same name, nothing rewritten.
        let mtime = fs::metadata(root.join(&name)).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20)); // (absence)
        assert_eq!(write_atomic(nr, "layers/roads/root", "pack", WriteFrom::Bytes(b"hello")).unwrap(), name);
        assert_eq!(fs::metadata(root.join(&name)).unwrap().modified().unwrap(), mtime);

        // From a file, larger than one copy chunk.
        let src = root.join("src.bin");
        let big: Vec<u8> = (0..(CHUNK + 999)).map(|i| (i % 253) as u8).collect();
        fs::write(&src, &big).unwrap();
        let name2 = write_atomic(nr, "base/6-1-2", "sect", WriteFrom::File(&src)).unwrap();
        assert_eq!(fs::read(root.join(&name2)).unwrap(), big);

        // A content-named file with the wrong size is never replaced.
        let bogus = root.join(content_name("x", &hash16(b"right"), "bin"));
        fs::write(&bogus, b"wrong size").unwrap();
        let err = write_atomic(nr, "x", "bin", WriteFrom::Bytes(b"right")).unwrap_err();
        assert!(err.to_string().contains("never rewritten"), "{err:#}");
        assert_eq!(fs::read(&bogus).unwrap(), b"wrong size");

        // Bad logical names.
        assert!(write_atomic(nr, "a.b", "bin", WriteFrom::Bytes(b"x")).is_err());
        assert!(write_atomic(nr, "../a", "bin", WriteFrom::Bytes(b"x")).is_err());
        assert!(write_atomic(nr, "/abs", "bin", WriteFrom::Bytes(b"x")).is_err());
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
        assert_eq!(name, write_atomic(nr, "h", "bin", WriteFrom::File(&src)).unwrap());
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
        let name = write_atomic(nr, "a", "bin", WriteFrom::File(&src)).unwrap();
        assert!(landed(&name));
        fs::remove_file(root.join(&name)).unwrap();
        damage_next(WRITE_TRIES - 1);
        assert_eq!(write_atomic(nr, "a", "bin", WriteFrom::Bytes(&big)).unwrap(), name);
        assert!(landed(&name));
        fs::remove_file(root.join(&name)).unwrap();

        // Every write read back wrong: it gives up after WRITE_TRIES of them, leaving nothing.
        damage_next(WRITE_TRIES);
        let e = write_atomic(nr, "a", "bin", WriteFrom::File(&src)).unwrap_err();
        assert!(format!("{e:#}").contains(&format!("{WRITE_TRIES} writes lost or read back wrong running")), "{e:#}");
        assert_eq!(left(), 0);
        // (Its faults all taken: the next write lands at once.)
        assert!(landed(&write_atomic(nr, "a", "bin", WriteFrom::File(&src)).unwrap()));

        // A source changed since it was hashed is never written again (the hash given is stale).
        fs::remove_dir_all(&root).unwrap();
        fs::create_dir(&root).unwrap();
        let stale = hash16(&big);
        fs::write(&src, big.iter().map(|b| b ^ 1).collect::<Vec<u8>>()).unwrap();
        let e = write_atomic_hashed(nr, "a", "bin", &src, &stale, big.len() as u64).unwrap_err();
        assert!(format!("{e:#}").contains("the source changed since it was hashed"), "{e:#}");
        assert_eq!(left(), 0);
    }

    /// Task #1142: a write whose close fails as the share's for a moment (ETIMEDOUT, EIO: a pageout
    /// lost to a session reconnect comes back at the close alone) is written again, its source
    /// unchanged, WRITE_TRIES writes at most, leaving nothing when all fail; another close error
    /// fails at once; a call past its deadline is held, not made again.
    #[test]
    fn a_write_whose_close_fails_is_written_again() {
        use crate::sys::close_fault::fail;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        let nr = &crate::nasfs::NasRoot::local_for_test(&root);
        let src = dir.path().join("src.bin");
        let big: Vec<u8> = (0..(CHUNK + 999)).map(|i| (i % 249) as u8).collect();
        fs::write(&src, &big).unwrap();
        let left = || fs::read_dir(&root).unwrap().count();
        let back0 = crate::nasio::get(crate::nasio::Count::ReadBack);

        // One close failed (its file whole: the close's error alone sends it again): written
        // again, and lands. From bytes too, all but the last of WRITE_TRIES failed, EIO.
        let f = fail(&root, libc::ETIMEDOUT, 1);
        let name = write_atomic(nr, "c", "bin", WriteFrom::File(&src)).unwrap();
        assert_eq!((f.failed(), fs::read(root.join(&name)).unwrap()), (1, big.clone()));
        drop(f);
        fs::remove_file(root.join(&name)).unwrap();
        let f = fail(&root, libc::EIO, WRITE_TRIES - 1);
        assert_eq!(write_atomic(nr, "c", "bin", WriteFrom::Bytes(&big)).unwrap(), name);
        assert_eq!((f.failed(), fs::read(root.join(&name)).unwrap()), (WRITE_TRIES - 1, big.clone()));
        drop(f);
        fs::remove_file(root.join(&name)).unwrap();
        // (Each landed write read back whole, counted the NAS's reads: two uploads' worth.)
        assert!(crate::nasio::get(crate::nasio::Count::ReadBack) - back0 >= 2 * big.len() as u64);

        // Every close failed: given up after WRITE_TRIES writes, the error said, nothing left.
        let f = fail(&root, libc::ETIMEDOUT, WRITE_TRIES + 5);
        let e = write_atomic(nr, "c", "bin", WriteFrom::File(&src)).unwrap_err();
        assert_eq!(f.failed(), WRITE_TRIES);
        let s = format!("{e:#}");
        assert!(s.contains(&format!("{WRITE_TRIES} writes lost or read back wrong running")), "{s}");
        assert!(s.contains(&io::Error::from_raw_os_error(libc::ETIMEDOUT).to_string()), "{s}");
        assert_eq!(left(), 0);
        drop(f);

        // A close error that isn't the share's moment (the disk full): failed at once, not again.
        let f = fail(&root, libc::ENOSPC, 3);
        let e = write_atomic(nr, "c", "bin", WriteFrom::File(&src)).unwrap_err();
        assert_eq!(f.failed(), 1, "{e:#}");
        assert_eq!(left(), 0);
        drop(f);

        // A call past its deadline (the temporary file's open, hung): held, the temporary name
        // with it, and never made again (no second call on the hung share).
        let _cap = crate::deadline::cap_for_test(crate::deadline::CAP_DEFAULT);
        let dest = root.join(content_name("c", &hash16(&big), "bin"));
        let hung = crate::deadline::fake::hang(&tmp_path(&dest)).deadline(std::time::Duration::from_millis(50));
        let e = write_atomic(nr, "c", "bin", WriteFrom::File(&src)).unwrap_err();
        let ie = e.chain().find_map(|c| c.downcast_ref::<io::Error>()).expect("an I/O error");
        assert!(crate::deadline::past_deadline(ie), "{e:#}");
        assert_eq!(hung.held(), 1, "made once: {e:#}");
        drop(hung);
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
