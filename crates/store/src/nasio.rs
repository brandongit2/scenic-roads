//! This process's I/O counts, kept where they happen: its NAS traffic (the store's readers, a
//! build's `Out` through pipeline::costin::wrote, the metadata walks costin::meta notes: bytes read
//! and written, files opened for reading, files made, folder entries listed, files statted and
//! deleted), and beside it the bytes read from this Mac's own disk (`Local`: crate::range's mapped
//! and local files, crate::cachefile's reads) and the GeoTIFFs opened. The one per-process counter
//! set: a build target's input measures (pipeline::costin) are its deltas.
//!
//! A job's processes count into files of their own on the agent's disk, so the agent sees a job's
//! traffic while it runs with no channel to it (docs/pool.md §10, Live): when `ENV` names a folder
//! (the agent's, per job slot, on its own disk: never the NAS, whose small writes are slow), the
//! first count makes `<folder>/<pid>-<nonce>` (a later process given an ended one's pid makes a file
//! of its own), `SLOTS` little-endian u64s mapped shared, and every count after is an atomic add in
//! that mapping (no write call). The agent sums the folder's files (`sum_dir`), folding the files of
//! processes that ended into one (`BASE`, `fold_ended`) so a long job's many programs keep the read
//! small; a job's totals only grow. Without `ENV` (or its file not made) the counts stay in this
//! process's memory alone. A program a step runs gets a folder of its own inside its runner's
//! (`program_dir`: summed one level down, so it shows as it runs), whose sums its runner takes into
//! its own as it ends (`take_program`; pipeline::unit's `run`).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// The environment variable naming the folder a process keeps its counts in.
pub const ENV: &str = "SCENIC_NAS_COUNTS";

/// The file in a counts folder holding the sums of processes that ended (`fold_ended`).
pub const BASE: &str = "base";

/// What is counted, by its place in a counts file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    /// Bytes read from the NAS.
    ReadBytes = 0,
    /// Files opened on the NAS for reading.
    Files = 1,
    WroteBytes = 2,
    /// Files made on the NAS.
    Creates = 3,
    /// Folder entries listed.
    Listed = 4,
    Stats = 5,
    Deletes = 6,
    /// Bytes read from this Mac's own disk.
    Local = 7,
    /// GeoTIFFs opened.
    Tiffs = 8,
}

/// The NAS's counts: the first `NAS` of `Count`'s.
pub const NAS: usize = 7;

/// The counts in a file (`Count`'s, then room for one more).
pub const SLOTS: usize = 10;

/// A process's counts, in `Count`'s order.
pub type Counts = [u64; SLOTS];

type Cells = [AtomicU64; SLOTS];

static OWN: Cells = [const { AtomicU64::new(0) }; SLOTS];

fn cells() -> &'static Cells {
    static CELLS: OnceLock<&'static Cells> = OnceLock::new();
    CELLS.get_or_init(|| std::env::var_os(ENV).and_then(|d| mapped(Path::new(&d))).unwrap_or(&OWN))
}

/// This process's counts file in folder `dir`, made new and mapped shared (kept for the process's
/// life); None when it can't be (the counts kept in memory then).
fn mapped(dir: &Path) -> Option<&'static Cells> {
    let pid = std::process::id();
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let f = (0..100u32).find_map(|i| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(dir.join(format!("{pid}-{}", nonce + u128::from(i))))
            .ok()
    })?;
    f.set_len((SLOTS * 8) as u64).ok()?;
    // SAFETY: the file is this process's alone to write (made new), the agent only reads it (and
    // removes it once the process ended); the mapping is kept for the process's life (leaked).
    let m = unsafe { memmap2::MmapMut::map_mut(&f) }.ok()?;
    let m: &'static mut memmap2::MmapMut = Box::leak(Box::new(m));
    // SAFETY: the mapping is page-aligned, SLOTS * 8 bytes long, zeroed by set_len, and u64's
    // atomics have u64's layout.
    Some(unsafe { &*(m.as_mut_ptr() as *const Cells) })
}

/// Adds `n` to count `c`.
pub fn add(c: Count, n: u64) {
    if n > 0 {
        cells()[c as usize].fetch_add(n, Ordering::Relaxed);
    }
}

/// Adds counts `c` (a program's this process ran, `sum_dir` of its folder) to this process's.
pub fn add_counts(c: &Counts) {
    for (cell, n) in cells().iter().zip(c) {
        if *n > 0 {
            cell.fetch_add(*n, Ordering::Relaxed);
        }
    }
}

/// A file opened on the NAS for reading.
pub fn opened() {
    add(Count::Files, 1);
}

/// `n` bytes read from the NAS.
pub fn read(n: u64) {
    add(Count::ReadBytes, n);
}

/// A file of `n` bytes made on the NAS.
pub fn wrote(n: u64) {
    add(Count::WroteBytes, n);
    add(Count::Creates, 1);
}

/// This process's counts so far.
pub fn own() -> Counts {
    std::array::from_fn(|i| cells()[i].load(Ordering::Relaxed))
}

/// Count `c` of this process's so far.
pub fn get(c: Count) -> u64 {
    cells()[c as usize].load(Ordering::Relaxed)
}

fn parse(b: &[u8]) -> Counts {
    let mut out = [0u64; SLOTS];
    for (o, c) in out.iter_mut().zip(b.chunks_exact(8)) {
        *o = u64::from_le_bytes(c.try_into().unwrap_or_default());
    }
    out
}

fn bytes(c: &Counts) -> Vec<u8> {
    c.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Folder `dir`'s counts: the sums of its files; None when the folder isn't there (a job its agent
/// gave none: no measure).
pub fn sum_dir(dir: &Path) -> Option<Counts> {
    let mut out = [0u64; SLOTS];
    add_dir(dir, &mut out, true).then_some(out)
}

/// Adds folder `dir`'s counts files (`BASE` and `<pid>-<nonce>`; nothing else, `base.tmp` not) to
/// `out`, and with `subs` its running programs' folders' (`SUB`*, one level); false when it can't be
/// read.
fn add_dir(dir: &Path, out: &mut Counts, subs: bool) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        if name.starts_with(SUB) {
            if subs {
                add_dir(&e.path(), out, false);
            }
        } else if name == BASE || name.split_once('-').is_some_and(|(p, _)| p.parse::<u32>().is_ok()) {
            let Ok(b) = std::fs::read(e.path()) else { continue };
            for (o, v) in out.iter_mut().zip(parse(&b)) {
                *o += v;
            }
        }
    }
    true
}

/// The prefix of a running program's counts folder inside its runner's (`program_dir`).
pub const SUB: &str = "sub-";

/// A folder for a program this process runs to count in (`ENV`), new: inside this process's own
/// counts folder when it has one (so the agent sees the program's traffic as it runs: `sum_dir`
/// goes one level into it), else `fallback`. Its runner, as it ends, calls `take_program`.
pub fn program_dir(fallback: &Path) -> std::path::PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let d = match std::env::var_os(ENV) {
        Some(own) => Path::new(&own).join(format!("{SUB}{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))),
        None => fallback.to_path_buf(),
    };
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(&d).ok();
    d
}

/// A program's folder (`program_dir`) as it ended: moved out of the summed folder first, then its
/// counts added to this process's (so nothing is summed twice; the agent's rates take a dip in the
/// meantime as none), and removed.
pub fn take_program(dir: &Path) {
    let gone = dir.with_file_name(format!(
        ".taken-{}",
        dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let at = if std::fs::rename(dir, &gone).is_ok() { gone } else { dir.to_path_buf() };
    if let Some(n) = sum_dir(&at) {
        add_counts(&n);
    }
    std::fs::remove_dir_all(&at).ok();
}

/// Whether process `pid` is running (a process of another user's counts as running).
fn running(pid: i32) -> bool {
    // SAFETY: signal 0 sends nothing; it only checks the process is there.
    #[cfg(unix)]
    {
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        alive || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    // (No processes to fold: the wasm build's.)
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Folds the files in folder `dir` of processes that ended into its `BASE` (written whole, then
/// renamed), each removed after. (A pid taken again by a running process keeps its ended file
/// unfolded a while: still summed, just not folded yet.)
pub fn fold_ended(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let ended: Vec<std::path::PathBuf> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let pid: i32 = name.split_once('-')?.0.parse().ok()?;
            (!running(pid)).then(|| e.path())
        })
        .collect();
    if ended.is_empty() {
        return;
    }
    let base = dir.join(BASE);
    let mut sum = std::fs::read(&base).map(|b| parse(&b)).unwrap_or_default();
    let mut taken = Vec::new();
    for p in ended {
        let Ok(b) = std::fs::read(&p) else { continue };
        for (o, v) in sum.iter_mut().zip(parse(&b)) {
            *o += v;
        }
        taken.push(p);
    }
    let tmp = dir.join(format!("{BASE}.tmp"));
    if std::fs::write(&tmp, bytes(&sum)).and_then(|_| std::fs::rename(&tmp, &base)).is_ok() {
        for p in taken {
            std::fs::remove_file(p).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mapped file's counts are seen by a reader of the folder as they're added; files of
    /// processes that ended fold into the base, the sums kept.
    #[test]
    fn a_folders_files_are_summed_and_folded() {
        let d = tempfile::tempdir().unwrap();
        let a = mapped(d.path()).unwrap();
        a[Count::ReadBytes as usize].fetch_add(100, Ordering::Relaxed);
        a[Count::Creates as usize].fetch_add(2, Ordering::Relaxed);
        // (Another process's, ended: no process has pid 2^31 - 2.)
        let mut other = [0u64; SLOTS];
        other[0] = 5;
        std::fs::write(d.path().join("2147483646-1"), bytes(&other)).unwrap();
        let s = sum_dir(d.path()).unwrap();
        assert_eq!((s[Count::ReadBytes as usize], s[Count::Creates as usize]), (105, 2));
        fold_ended(d.path());
        fold_ended(d.path());
        assert!(!d.path().join("2147483646-1").exists());
        assert_eq!(parse(&std::fs::read(d.path().join(BASE)).unwrap())[0], 5);
        assert_eq!(sum_dir(d.path()).unwrap(), s);
        assert!(sum_dir(&d.path().join("missing")).is_none());
        let before = get(Count::Listed);
        add(Count::Listed, 3);
        assert!(get(Count::Listed) >= before + 3);
    }

    /// A running program's folder inside its runner's is summed with it; taken as it ends, its
    /// counts move to the runner's own (none twice); `base.tmp` is never summed.
    #[test]
    fn a_running_programs_counts_show_in_the_slots_sum() {
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join(format!("{SUB}1-0"));
        std::fs::create_dir(&sub).unwrap();
        let mut c = [0u64; SLOTS];
        c[Count::ReadBytes as usize] = 40;
        std::fs::write(sub.join("2147483646-9"), bytes(&c)).unwrap();
        std::fs::write(d.path().join("base.tmp"), bytes(&c)).unwrap();
        assert_eq!(sum_dir(d.path()).unwrap()[0], 40);
        let before = get(Count::ReadBytes);
        take_program(&sub);
        assert!(get(Count::ReadBytes) >= before + 40);
        assert_eq!(sum_dir(d.path()).unwrap()[0], 0);
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }

    /// A process given the pid of one that ended makes a file of its own: the ended one's counts
    /// stay.
    #[test]
    fn a_pid_taken_again_keeps_the_ended_ones_counts() {
        let d = tempfile::tempdir().unwrap();
        let first = mapped(d.path()).unwrap();
        first[0].fetch_add(7, Ordering::Relaxed);
        let again = mapped(d.path()).unwrap();
        again[0].fetch_add(1, Ordering::Relaxed);
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 2);
        assert_eq!(sum_dir(d.path()).unwrap()[0], 8);
    }
}
