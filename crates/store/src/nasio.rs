//! This process's NAS traffic, counted where it happens (the store's readers, a build's `Out`, the
//! metadata walks noted by pipeline::costin::meta): the bytes read and written, the files opened
//! for reading, the files made, the folder entries listed, the files statted and deleted.
//!
//! A job's processes count into files of their own on the agent's disk, so the agent sees a job's
//! traffic while it runs with no channel to it (docs/pool.md §10, Live): when `ENV` names a folder
//! (the agent's, per job slot, on its own disk: never the NAS, whose small writes are slow), the
//! first count makes `<folder>/<pid>`, `SLOTS` little-endian u64s mapped shared, and every count
//! after is an atomic add in that mapping (no write call). The agent sums the folder's files
//! (`read_dir`); a process that ended leaves its file, so a job's totals only grow. Without `ENV`
//! (or its file not made) the counts stay in this process's memory alone.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// The environment variable naming the folder a job's processes keep their counts in.
pub const ENV: &str = "SCENIC_NAS_COUNTS";

/// What is counted, by its place in a counts file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    ReadBytes = 0,
    /// Files opened for reading.
    Files = 1,
    WroteBytes = 2,
    /// Files made.
    Creates = 3,
    /// Folder entries listed.
    Listed = 4,
    Stats = 5,
    Deletes = 6,
}

/// The counts in a file (`Count`'s, then room for more).
pub const SLOTS: usize = 8;

/// A process's counts: their names in `Count`'s order.
pub type Counts = [u64; SLOTS];

type Cells = [AtomicU64; SLOTS];

static OWN: Cells = [const { AtomicU64::new(0) }; SLOTS];

fn cells() -> &'static Cells {
    static CELLS: OnceLock<&'static Cells> = OnceLock::new();
    CELLS.get_or_init(|| std::env::var_os(ENV).and_then(|d| mapped(Path::new(&d))).unwrap_or(&OWN))
}

/// This process's counts file in folder `dir`, made and mapped shared (kept for the process's life);
/// None when it can't be (the counts kept in memory then).
fn mapped(dir: &Path) -> Option<&'static Cells> {
    let p = dir.join(std::process::id().to_string());
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&p).ok()?;
    f.set_len((SLOTS * 8) as u64).ok()?;
    // SAFETY: the file is this process's alone to write (named by its pid, made afresh), the agent
    // only reads it; the mapping is kept for the process's life (leaked).
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

/// A job's counts: the sums of the files in folder `dir` (`ENV`); None when the folder isn't there
/// (a job its agent gave none: no measure).
pub fn sum_dir(dir: &Path) -> Option<Counts> {
    let mut out = [0u64; SLOTS];
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(b) = std::fs::read(e.path()) else { continue };
        for (o, c) in out.iter_mut().zip(b.chunks_exact(8)) {
            *o += u64::from_le_bytes(c.try_into().unwrap_or_default());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mapped file's counts are seen by a reader of the folder as they're added.
    #[test]
    fn a_folders_files_are_summed() {
        let d = tempfile::tempdir().unwrap();
        let a = mapped(d.path()).unwrap();
        a[Count::ReadBytes as usize].fetch_add(100, Ordering::Relaxed);
        a[Count::Creates as usize].fetch_add(2, Ordering::Relaxed);
        // (Another process's, ended.)
        let mut other = vec![0u8; SLOTS * 8];
        other[..8].copy_from_slice(&5u64.to_le_bytes());
        std::fs::write(d.path().join("1"), other).unwrap();
        let s = sum_dir(d.path()).unwrap();
        assert_eq!((s[Count::ReadBytes as usize], s[Count::Creates as usize]), (105, 2));
        assert!(sum_dir(&d.path().join("missing")).is_none());
        let before = own()[Count::Listed as usize];
        add(Count::Listed, 3);
        assert!(own()[Count::Listed as usize] >= before + 3);
    }
}
