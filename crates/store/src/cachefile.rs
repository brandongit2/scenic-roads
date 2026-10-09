//! The build agent's caches, read and written through one accessor (docs/plan.md §8, Room on the
//! disk), so room-making may delete any file of them no job uses, at any moment, mid-job too.
//!
//! - A job holds each cache file it uses (`hold`): opened, with a shared advisory lock (`flock`
//!   LOCK_SH) on it, kept until the process ends or lets it go (`release_before`). Children that open
//!   it by name (osmium, the Python steps, scenic-metrics) are covered: the lock is on the file
//!   itself, whoever holds it. A file that isn't there is filled first (`refill`: from the NAS, or
//!   wherever that cache fills from), so a file deleted wrongly costs a fetch, never a failure.
//! - Room-making deletes a file only with an exclusive lock taken without waiting (`try_remove`:
//!   LOCK_EX|LOCK_NB), its name checked to still be that file, and unlinked while it's held; a file
//!   in use is skipped.
//! - Nothing in the caches is renamed over another file: a file is made by a temporary name, held
//!   from the start, and given its name only if the name is free (`create`), so a name, once there,
//!   names that file until it's deleted under the exclusive lock.
//!
//! The race (a job's "open, then lock" against a deleter's "lock exclusively, then unlink"): a job
//! that opened a file a deleter then unlinked takes its shared lock on a file with no name left
//! (`nlink` 0), lets it go and looks again (`hold`'s loop): it fills it again. A job never ends up
//! holding a name that's gone. (Deleting an open file is safe on macOS anyway: its bytes stay until
//! it's closed, and free nothing until then.)
//!
//! (A child process being started has copies of its parent's open files, their locks with them,
//! until it starts its program: a file looks in use for that moment, and is passed over.)
//!
//! A cache file read whole at once (a raw terrain tile) is locked only while it's read (`read`).
//! Folders go file by file, then empty folders (`remove_tree`). dem/cachefile.py is the same for
//! the Python steps. On WebAssembly (no locks, no deleter) it only reads and fills.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// What `try_remove` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removed {
    /// Deleted: its bytes (freed once no one has it open).
    Freed(u64),
    /// A job uses it (or it's a link, or it can't be opened to lock): it stays.
    InUse,
    /// It isn't there (any more).
    Gone,
}

/// A process's held files: by (device, inode), the open file (its lock with it), when it was held
/// (`mark`'s count) and its name.
struct Held {
    files: HashMap<(u64, u64), (File, u64, PathBuf)>,
    warned: bool,
}

fn held() -> &'static Mutex<Held> {
    static H: OnceLock<Mutex<Held>> = OnceLock::new();
    H.get_or_init(|| {
        raise_limit();
        Mutex::new(Held { files: HashMap::new(), warned: false })
    })
}

static GEN: AtomicU64 = AtomicU64::new(1);
static TMP: AtomicU64 = AtomicU64::new(0);

/// The open files this process allows itself (RLIMIT_NOFILE, raised once to what the system allows).
static LIMIT: AtomicU64 = AtomicU64::new(256);

/// Raises this process's open-file limit to its hard limit (macOS: at most OPEN_MAX, 10240).
fn raise_limit() {
    #[cfg(unix)]
    unsafe {
        let mut r = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) == 0 {
            let want = if cfg!(target_os = "macos") { r.rlim_max.min(10240) } else { r.rlim_max };
            if want > r.rlim_cur {
                let n = libc::rlimit { rlim_cur: want, rlim_max: r.rlim_max };
                if libc::setrlimit(libc::RLIMIT_NOFILE, &n) == 0 {
                    r.rlim_cur = want;
                }
            }
            LIMIT.store(r.rlim_cur as u64, Ordering::Relaxed);
        }
    }
}

/// The files a process holds at most: a quarter of its open-file limit. Past it the earliest held
/// go (said once): a job that holds that many should hold them in turns (`mark`, `release_before`).
pub fn cap() -> usize {
    held();
    (LIMIT.load(Ordering::Relaxed) / 4).max(32) as usize
}

/// The files this process holds now.
pub fn held_count() -> usize {
    held().lock().unwrap().files.len()
}

/// A point in this process's holding: files held from now on are after it (`release_before`).
pub fn mark() -> u64 {
    GEN.fetch_add(1, Ordering::Relaxed)
}

/// Lets go of the files held before `mark` (and not held again since): a job done with them (a
/// unit's, once the next is under way). Room-making may delete them now.
pub fn release_before(mark: u64) {
    held().lock().unwrap().files.retain(|_, f| f.1 >= mark);
}

/// Lets go of the file at `path`, when this process holds it.
pub fn release(path: &Path) {
    if let Ok(m) = std::fs::metadata(path) {
        held().lock().unwrap().files.remove(&id(&m));
    }
}

/// EINVAL's number.
fn libc_einval() -> i32 {
    #[cfg(unix)]
    return libc::EINVAL;
    #[cfg(not(unix))]
    22
}

/// `e`, saying what was done to which file.
fn with(e: io::Error, what: &str, p: &Path) -> io::Error {
    io::Error::new(e.kind(), format!("{what} {}: {e}", p.display()))
}

#[cfg(unix)]
fn id(m: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (m.dev(), m.ino())
}

#[cfg(not(unix))]
fn id(_: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

#[cfg(unix)]
fn links(m: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.nlink()
}

#[cfg(not(unix))]
fn links(_: &std::fs::Metadata) -> u64 {
    1
}

#[cfg(unix)]
fn lock(f: &File, how: libc::c_int) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    loop {
        // SAFETY: flock on a descriptor we own.
        if unsafe { libc::flock(f.as_raw_fd(), how) } == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Opens `path` with a shared lock: the file, when its name still names it once locked; None when
/// it isn't there, or was deleted meanwhile (look again).
fn open_shared(path: &Path) -> io::Result<Option<File>> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(with(e, "open", path)),
    };
    #[cfg(unix)]
    lock(&f, libc::LOCK_SH).map_err(|e| with(e, "lock", path))?;
    let m = f.metadata()?;
    // (Deleted while it was being locked: no name left, or its name another file's now.)
    if links(&m) == 0 || std::fs::metadata(path).map(|n| id(&n)).ok() != Some(id(&m)) {
        return Ok(None);
    }
    Ok(Some(f))
}

/// Keeps `f` (`path`'s) held by this process.
fn keep(f: File, path: &Path) -> io::Result<()> {
    let m = f.metadata()?;
    let g = GEN.fetch_add(1, Ordering::Relaxed);
    let cap = cap();
    let mut h = held().lock().unwrap();
    h.files.insert(id(&m), (f, g, path.to_path_buf()));
    if h.files.len() > cap {
        // (The earliest held let go: a job past its cap. Said once.)
        let mut gens: Vec<u64> = h.files.values().map(|f| f.1).collect();
        gens.sort_unstable();
        let cut = gens[h.files.len() - cap * 3 / 4];
        if !h.warned {
            eprintln!("cache: this process holds {} cache files, past its cap of {cap}: the earliest let go (room-making may delete them; they're filled again if read through the accessor)", h.files.len());
            h.warned = true;
        }
        h.files.retain(|_, f| f.1 >= cut);
    } else if h.files.len() > cap * 4 / 5 && !h.warned {
        eprintln!("cache: this process holds {} cache files, near its cap of {cap}", h.files.len());
        h.warned = true;
    }
    Ok(())
}

/// Marks a cache file used now (its modification time: room-making deletes the least recently
/// used first). Failure costs only that order.
pub fn touch(path: &Path) {
    if let Ok(f) = File::options().append(true).open(path) {
        f.set_modified(std::time::SystemTime::now()).ok();
    }
}

/// Holds the cache file at `path` (shared lock, until the process ends or `release_before`),
/// filled first by `refill` when it isn't there (`create`); marked used. Its name, as given.
pub fn hold(path: &Path, refill: &mut dyn FnMut(&Path) -> io::Result<()>) -> io::Result<PathBuf> {
    for _ in 0..100 {
        if let Some(f) = open_shared(path)? {
            keep(f, path)?;
            touch(path);
            return Ok(path.to_path_buf());
        }
        if !path.exists() {
            create(path, refill)?;
        }
    }
    Err(io::Error::other(format!("{}: deleted each time it was opened", path.display())))
}

/// Holds the cache file at `path` when it's there (as `hold`, nothing filled): None when it isn't.
pub fn hold_existing(path: &Path) -> io::Result<Option<PathBuf>> {
    for _ in 0..100 {
        match open_shared(path)? {
            Some(f) => {
                keep(f, path)?;
                touch(path);
                return Ok(Some(path.to_path_buf()));
            }
            None if !path.exists() => return Ok(None),
            None => {}
        }
    }
    Err(io::Error::other(format!("{}: deleted each time it was opened", path.display())))
}

/// The bytes of the cache file at `path`, read under a shared lock (let go once read: a file read
/// whole at once, as a raw tile); None when it isn't there.
pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    for _ in 0..100 {
        match open_shared(path)? {
            Some(mut f) => {
                let mut b = Vec::new();
                f.read_to_end(&mut b)?;
                return Ok(Some(b));
            }
            None if !path.exists() => return Ok(None),
            None => {}
        }
    }
    Err(io::Error::other(format!("{}: deleted each time it was opened", path.display())))
}

/// A temporary name beside `path`, this process's own: `<name>.<pid>.<n>.tmp`.
pub fn tmp_of(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{name}.{}.{}.tmp", crate::sys::pid(), TMP.fetch_add(1, Ordering::Relaxed)))
}

/// Makes folder `d` of the caches (and those above it), as `create_dir_all`, but made again when
/// room-making takes one, empty, as it's made (which `create_dir_all` gives up on: EEXIST, ENOENT,
/// or as it goes on macOS, EINVAL).
pub fn make_dir(d: &Path) -> io::Result<()> {
    let mut last = None;
    for _ in 0..100 {
        match std::fs::create_dir_all(d) {
            Ok(()) => return Ok(()),
            Err(_) if d.is_dir() => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(with(last.unwrap_or_else(|| io::Error::other("no folder")), "make", d))
}

/// A temporary file beside cache file `path` (`tmp_of`), made and locked (shared) at once, so
/// room-making never takes it while it's written: its name and the file (read and write). The
/// folder is made (again, if room-making took it meanwhile). Named with `publish`, or deleted.
pub fn scratch(path: &Path) -> io::Result<(PathBuf, File)> {
    let dir = path.parent().unwrap_or(Path::new("."));
    for _ in 0..100 {
        // (A folder room-making takes as it's made, empty: made again.)
        make_dir(dir)?;
        let tmp = tmp_of(path);
        let f = match File::options().read(true).write(true).create_new(true).open(&tmp) {
            Ok(f) => f,
            // (Its folder taken meanwhile: gone, or, as it goes, EINVAL on macOS.)
            Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(libc_einval()) => continue,
            Err(e) => return Err(with(e, "make", &tmp)),
        };
        #[cfg(unix)]
        lock(&f, libc::LOCK_SH)?;
        // (Taken between its making and its lock: another made.)
        if links(&f.metadata()?) == 0 {
            continue;
        }
        return Ok((tmp, f));
    }
    Err(io::Error::other(format!("{}: no temporary file could be kept beside it", path.display())))
}

/// Gives scratch file `tmp` (its open file `f`, from `scratch`) the name `path` if that's free
/// (another process's copy made meanwhile wins: this one goes), held by this process (as `hold`);
/// the temporary name goes either way. (A writer that put another file at `tmp`: that one.)
pub fn publish(f: File, tmp: &Path, path: &Path) -> io::Result<()> {
    let r = (|| -> io::Result<()> {
        let f = match std::fs::metadata(tmp) {
            Ok(m) if id(&m) == id(&f.metadata()?) => f,
            Ok(_) => open_shared(tmp)?.ok_or_else(|| io::Error::other(format!("{}: gone as it was written", tmp.display())))?,
            Err(e) => return Err(e),
        };
        match std::fs::hard_link(tmp, path) {
            Ok(()) => keep(f, path),
            // (Another copy made meanwhile: that one's used.)
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(with(e, "name", path)),
        }
    })();
    std::fs::remove_file(tmp).ok();
    r
}

/// Makes the cache file at `path` when it isn't there: a scratch file beside it (`scratch`), written
/// by `fill` in place (it may truncate and rewrite it, never rename another file onto it: a writer
/// that does is followed to its file), then named (`publish`).
pub fn create(path: &Path, fill: &mut dyn FnMut(&Path) -> io::Result<()>) -> io::Result<()> {
    let (tmp, f) = scratch(path)?;
    if let Err(e) = fill(&tmp) {
        std::fs::remove_file(&tmp).ok();
        return Err(with(e, "fill", &tmp));
    }
    publish(f, &tmp, path)
}

/// Locks open cache file `f` shared, as `scratch` does: one a job writes under a name of its own
/// (a spool), which room-making then leaves.
pub fn lock_shared(f: &File) -> io::Result<()> {
    #[cfg(unix)]
    lock(f, libc::LOCK_SH)?;
    #[cfg(not(unix))]
    let _ = f;
    Ok(())
}

/// Makes the cache file at `path` with bytes `b` when it isn't there, not held after (a small file
/// read whole when it's read: a raw tile, a "none there" marker).
pub fn put(path: &Path, b: &[u8]) -> io::Result<()> {
    create_bytes(path, b)?;
    release(path);
    Ok(())
}

/// Makes the cache file at `path` with bytes `b` when it isn't there (`create`).
pub fn create_bytes(path: &Path, b: &[u8]) -> io::Result<()> {
    create(path, &mut |t| {
        use std::io::Write;
        let mut f = File::options().write(true).truncate(true).open(t)?;
        f.write_all(b)
    })
}

/// Deletes the cache file at `path` unless a job holds it (an exclusive lock, not waited for; the
/// name checked to still be that file; unlinked while locked). Never through a link.
pub fn try_remove(path: &Path) -> Removed {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let f = match File::options().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Removed::Gone,
            // (A link, or not to be opened: left.)
            Err(_) => return Removed::InUse,
        };
        let Ok(m) = f.metadata() else { return Removed::InUse };
        if !m.is_file() {
            return Removed::InUse;
        }
        if lock(&f, libc::LOCK_EX | libc::LOCK_NB).is_err() {
            return Removed::InUse;
        }
        match std::fs::symlink_metadata(path) {
            Ok(n) if id(&n) == id(&m) => {}
            Ok(_) => return Removed::InUse,
            Err(_) => return Removed::Gone,
        }
        match std::fs::remove_file(path) {
            Ok(()) => Removed::Freed(m.len()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Removed::Gone,
            Err(_) => Removed::InUse,
        }
    }
    #[cfg(not(unix))]
    match std::fs::metadata(path) {
        Ok(m) => std::fs::remove_file(path).map_or(Removed::InUse, |_| Removed::Freed(m.len())),
        Err(_) => Removed::Gone,
    }
}

/// Deletes the cache files at `paths` together, or none: each locked exclusively first (not
/// waited for), all unlinked only when every one could be (a set whose files mean something only
/// together: the DEM seed). Their bytes when deleted; None when one is in use.
pub fn try_remove_all(paths: &[PathBuf]) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut locked = Vec::new();
        for p in paths {
            let f = match File::options().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(p) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return None,
            };
            let m = f.metadata().ok()?;
            if !m.is_file() || lock(&f, libc::LOCK_EX | libc::LOCK_NB).is_err() {
                return None;
            }
            if std::fs::symlink_metadata(p).ok().map(|n| id(&n)) != Some(id(&m)) {
                return None;
            }
            locked.push((f, m.len(), p));
        }
        let mut n = 0;
        for (_, len, p) in &locked {
            if std::fs::remove_file(p).is_ok() {
                n += len;
            }
        }
        Some(n)
    }
    #[cfg(not(unix))]
    Some(paths.iter().map(|p| match try_remove(p) {
        Removed::Freed(n) => n,
        _ => 0,
    }).sum())
}

/// Deletes a damaged cache file a job found (cut short: no use to anyone), so it's filled again:
/// with the exclusive lock when it can be had within seconds (a deleter's is momentary), else
/// anyway (another job holding it holds bytes no one can use: its own open file stays readable).
/// This process's own hold on it goes first. Only while its name still names the file looked at.
pub fn discard(path: &Path) {
    release(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let Ok(f) = File::options().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path) else { return };
        let Ok(m) = f.metadata() else { return };
        let t = std::time::Instant::now();
        while lock(&f, libc::LOCK_EX | libc::LOCK_NB).is_err() && t.elapsed() < std::time::Duration::from_secs(3) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if std::fs::symlink_metadata(path).ok().map(|n| id(&n)) == Some(id(&m)) {
            std::fs::remove_file(path).ok();
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::remove_file(path).ok();
    }
}

/// What `remove_tree` did: the bytes freed, and those left (in use, or not to be deleted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub freed: u64,
    pub left: u64,
}

/// Deletes the files under folder `dir` that no job holds (`try_remove`; nothing through a link),
/// then the folders left empty; a file, alone.
pub fn remove_tree(dir: &Path) -> Tree {
    let mut t = Tree::default();
    let Ok(m) = std::fs::symlink_metadata(dir) else { return t };
    if m.file_type().is_symlink() {
        return t;
    }
    if !m.is_dir() {
        match try_remove(dir) {
            Removed::Freed(n) => t.freed += n,
            Removed::InUse => t.left += m.len(),
            Removed::Gone => {}
        }
        return t;
    }
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let s = remove_tree(&e.path());
        t.freed += s.freed;
        t.left += s.left;
    }
    std::fs::remove_dir(dir).ok();
    t
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `try_remove`, tried for a moment: a child process another test starts holds copies of this
    /// process's open files (their locks too) until it's started its program.
    fn remove(p: &Path) -> Removed {
        for _ in 0..100 {
            match try_remove(p) {
                Removed::InUse => std::thread::sleep(std::time::Duration::from_millis(10)),
                r => return r,
            }
        }
        Removed::InUse
    }

    fn remove_all(ps: &[PathBuf]) -> Option<u64> {
        (0..100).find_map(|_| try_remove_all(ps).or_else(|| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            None
        }))
    }

    #[test]
    fn a_held_file_stays_and_one_let_go_goes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a/b.bin");
        let mut fills = 0;
        let mut fill = |t: &Path| {
            fills += 1;
            std::fs::write(t, b"bytes")
        };
        hold(&p, &mut fill).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"bytes");
        assert_eq!(try_remove(&p), Removed::InUse);
        assert!(p.exists());
        release(&p);
        assert_eq!(remove(&p), Removed::Freed(5));
        assert_eq!(try_remove(&p), Removed::Gone);
        // Wrongly deleted: filled again when next held.
        hold(&p, &mut fill).unwrap();
        assert_eq!(fills, 2);
        release(&p);
        // (No temporary files left.)
        assert_eq!(std::fs::read_dir(d.path().join("a")).unwrap().count(), 1);
    }

    #[test]
    fn a_file_being_made_is_never_deleted_and_a_name_is_never_replaced() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x");
        let seen = std::sync::Mutex::new(Vec::new());
        create(&p, &mut |t| {
            // (Mid-write: room-making can't take the temporary file.)
            seen.lock().unwrap().push(try_remove(t));
            std::fs::write(t, b"first")
        })
        .unwrap();
        assert_eq!(seen.lock().unwrap()[0], Removed::InUse);
        // A second maker's copy loses: the name keeps the first's file.
        create(&p, &mut |t| std::fs::write(t, b"second")).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"first");
        release(&p);
    }

    #[test]
    fn a_writer_that_renames_onto_the_temporary_name_is_followed() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("y");
        create(&p, &mut |t| {
            let other = t.with_extension("other");
            std::fs::write(&other, b"renamed")?;
            std::fs::rename(&other, t)
        })
        .unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"renamed");
        assert_eq!(try_remove(&p), Removed::InUse);
        release(&p);
        assert_eq!(remove(&p), Removed::Freed(7));
    }

    #[test]
    fn a_reader_racing_a_deleter_never_keeps_a_name_that_is_gone() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("r");
        std::fs::write(&p, b"0123456789").unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    try_remove(&p);
                }
            });
            for _ in 0..2000 {
                hold(&p, &mut |t| std::fs::write(t, b"0123456789")).unwrap();
                // Held: its name is there and is the file, until it's let go.
                assert_eq!(std::fs::read(&p).unwrap(), b"0123456789");
                release(&p);
            }
            stop.store(true, Ordering::Relaxed);
        });
    }

    #[test]
    fn a_child_process_reads_by_name_what_its_parent_holds() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c");
        hold(&p, &mut |t| std::fs::write(t, b"child")).unwrap();
        assert_eq!(try_remove(&p), Removed::InUse);
        // A child (cat) opens it by name, while a deleter tries in between.
        let out = std::process::Command::new("cat").arg(&p).output().unwrap();
        assert_eq!(out.stdout, b"child");
        // And the lock is the file's, not the name's: a child's own lock on it, shared, is no conflict.
        assert_eq!(try_remove(&p), Removed::InUse);
        release(&p);
    }

    #[test]
    fn a_set_goes_together_or_not_at_all() {
        let d = tempfile::tempdir().unwrap();
        let ps: Vec<PathBuf> = ["k", "e", "s"].iter().map(|n| d.path().join(n)).collect();
        for p in &ps {
            std::fs::write(p, b"12").unwrap();
        }
        hold_existing(&ps[1]).unwrap();
        assert_eq!(try_remove_all(&ps), None);
        assert!(ps.iter().all(|p| p.exists()));
        release(&ps[1]);
        assert_eq!(remove_all(&ps), Some(6));
        assert!(ps.iter().all(|p| !p.exists()));
    }

    #[test]
    fn a_tree_loses_what_no_job_holds() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("t");
        for n in ["a/1", "a/2", "b/3"] {
            std::fs::create_dir_all(t.join(n).parent().unwrap()).unwrap();
            std::fs::write(t.join(n), b"xyz").unwrap();
        }
        hold_existing(&t.join("a/2")).unwrap();
        let mut r = remove_tree(&t);
        for _ in 0..100 {
            if r.left == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            r.freed += remove_tree(&t).freed;
            r.left = remove_tree(&t).left;
        }
        assert_eq!(r, Tree { freed: 6, left: 3 });
        assert!(t.join("a/2").exists() && !t.join("b").exists());
        release(&t.join("a/2"));
    }
}
