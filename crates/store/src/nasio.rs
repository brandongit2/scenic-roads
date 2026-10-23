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
//!
//! After the counts, the same file says what the process is doing now (`set_phase`: the phase
//! pipeline::timings has open on a job's main thread, and since when), written under a sequence
//! number (odd while it's written) so a reader never takes half of one; never summed or folded.
//! The agent reads it from the newest running process's file at the folder's top (`phase_in`).

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

/// The phase's text's room (bytes; longer is cut at a character's end).
pub const PHASE_MAX: usize = 48;

/// A counts file's layout: the counts, then the phase (`set_phase`): its sequence number, when it
/// was set (unix ms), its text (zero-padded).
#[repr(C)]
struct File {
    counts: Cells,
    seq: AtomicU64,
    since_ms: AtomicU64,
    text: [AtomicU64; PHASE_MAX / 8],
}

/// A counts file's length.
const FILE_LEN: usize = std::mem::size_of::<File>();

static OWN: File = File {
    counts: [const { AtomicU64::new(0) }; SLOTS],
    seq: AtomicU64::new(0),
    since_ms: AtomicU64::new(0),
    text: [const { AtomicU64::new(0) }; PHASE_MAX / 8],
};

fn file() -> &'static File {
    static FILE: OnceLock<&'static File> = OnceLock::new();
    FILE.get_or_init(|| std::env::var_os(ENV).and_then(|d| mapped(Path::new(&d))).unwrap_or(&OWN))
}

fn cells() -> &'static Cells {
    &file().counts
}

/// Says what this process is doing now (`""`: nothing named), from now: one sequence of relaxed
/// stores in its counts file (no write call), cut to `PHASE_MAX` bytes.
pub fn set_phase(text: &str) {
    let f = file();
    let b = cut(text);
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    f.seq.fetch_add(1, Ordering::AcqRel);
    for (cell, w) in f.text.iter().zip(b.chunks_exact(8)) {
        cell.store(u64::from_le_bytes(w.try_into().unwrap_or_default()), Ordering::Relaxed);
    }
    f.since_ms.store(ms, Ordering::Relaxed);
    f.seq.fetch_add(1, Ordering::Release);
}

/// `text` cut to `PHASE_MAX` bytes at a character's end, zero-padded.
fn cut(text: &str) -> [u8; PHASE_MAX] {
    let mut end = text.len().min(PHASE_MAX);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut b = [0u8; PHASE_MAX];
    b[..end].copy_from_slice(&text.as_bytes()[..end]);
    b
}

/// A counts file's phase (`set_phase`): its sequence number, its text and when it was set (unix
/// ms); None when none was ever set, or it was being written (odd sequence) or the file is an older
/// one's (counts alone). (One copy alone may hold a text torn by a write begun after its sequence
/// was copied: `settled`.)
fn phase_of(b: &[u8]) -> Option<(u64, String, u64)> {
    if b.len() < FILE_LEN {
        return None;
    }
    let word = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap_or_default());
    let seq = word(SLOTS);
    if seq == 0 || seq % 2 == 1 {
        return None;
    }
    let text = &b[(SLOTS + 2) * 8..FILE_LEN];
    let text = String::from_utf8_lossy(&text[..text.iter().position(|&c| c == 0).unwrap_or(text.len())]).into_owned();
    Some((seq, text, word(SLOTS + 1)))
}

/// A file's phase from `read` (its bytes, read whole), taken only when the next read's sequence is
/// the same even one (no write began after the first copy's sequence, so its text is whole); tried
/// three times.
fn settled(read: impl Fn() -> Option<Vec<u8>>) -> Option<(String, u64)> {
    for _ in 0..3 {
        let Some((seq, text, ms)) = read().and_then(|b| phase_of(&b)) else { continue };
        if read().and_then(|b| phase_of(&b)).is_some_and(|again| again.0 == seq) {
            return Some((text, ms));
        }
    }
    None
}

/// What folder `dir`'s job is doing now: the phase of the newest set among its running processes'
/// files at its top (a job's own processes; its programs' folders not), with when it was set (unix
/// ms). Each read twice (`settled`), so a text caught mid-write is never taken. None: none says.
pub fn phase_in(dir: &Path) -> Option<(String, u64)> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(String, u64)> = None;
    for e in rd.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        let Some(pid) = name.split_once('-').and_then(|(p, _)| p.parse::<i32>().ok()) else { continue };
        if !running(pid) {
            continue;
        }
        if let Some(p) = settled(|| std::fs::read(e.path()).ok()) {
            if best.as_ref().is_none_or(|b| p.1 > b.1) {
                best = Some(p);
            }
        }
    }
    best
}

/// This process's counts file in folder `dir`, made new and mapped shared (kept for the process's
/// life); None when it can't be (the counts kept in memory then).
fn mapped(dir: &Path) -> Option<&'static File> {
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
    f.set_len(FILE_LEN as u64).ok()?;
    // SAFETY: the file is this process's alone to write (made new), the agent only reads it (and
    // removes it once the process ended); the mapping is kept for the process's life (leaked).
    let m = unsafe { memmap2::MmapMut::map_mut(&f) }.ok()?;
    let m: &'static mut memmap2::MmapMut = Box::leak(Box::new(m));
    // SAFETY: the mapping is page-aligned, FILE_LEN bytes long, zeroed by set_len, and `File` is
    // repr(C) of u64 atomics, which have u64's layout.
    Some(unsafe { &*(m.as_mut_ptr() as *const File) })
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
    // (The counts alone: the phase after them is never summed.)
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
        a.counts[Count::ReadBytes as usize].fetch_add(100, Ordering::Relaxed);
        a.counts[Count::Creates as usize].fetch_add(2, Ordering::Relaxed);
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
        first.counts[0].fetch_add(7, Ordering::Relaxed);
        let again = mapped(d.path()).unwrap();
        again.counts[0].fetch_add(1, Ordering::Relaxed);
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 2);
        assert_eq!(sum_dir(d.path()).unwrap()[0], 8);
    }

    /// A process's phase is read from its file whole, the newest of the folder's running
    /// processes', cut at a character's end; an ended process's, an older file's (counts alone) and
    /// one mid-write aren't taken; the counts' sums never include it.
    #[test]
    fn the_newest_running_processs_phase_is_read() {
        let d = tempfile::tempdir().unwrap();
        let f = mapped(d.path()).unwrap();
        let set = |f: &File, text: &[u8; PHASE_MAX], seq: u64, ms: u64| {
            f.seq.store(seq, Ordering::Relaxed);
            f.since_ms.store(ms, Ordering::Relaxed);
            for (c, w) in f.text.iter().zip(text.chunks_exact(8)) {
                c.store(u64::from_le_bytes(w.try_into().unwrap()), Ordering::Relaxed);
            }
        };
        let mut t = [0u8; PHASE_MAX];
        t[..12].copy_from_slice(b"z6 tiles\tcpu");
        set(f, &t, 2, 1000);
        assert_eq!(phase_in(d.path()), Some(("z6 tiles\tcpu".into(), 1000)));
        // (An ended process's newer one isn't taken; nor an older file's.)
        let mut ended = vec![0u8; FILE_LEN];
        ended[SLOTS * 8..SLOTS * 8 + 16].copy_from_slice(&[2, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 0, 0, 0, 0]);
        ended[(SLOTS + 2) * 8] = b'x';
        std::fs::write(d.path().join("2147483646-1"), &ended).unwrap();
        std::fs::write(d.path().join("2147483646-2"), [0u8; SLOTS * 8]).unwrap();
        assert_eq!(phase_in(d.path()).unwrap().1, 1000);
        assert_eq!(phase_of(&ended), Some((2, "x".into(), 0xffff)));
        // (Mid-write: not taken.)
        set(f, &t, 3, 2000);
        assert_eq!(phase_in(d.path()), None);
        assert_eq!(sum_dir(d.path()).unwrap(), [0; SLOTS]);
        // (Cut at a character's end.)
        set(f, &cut(&"é".repeat(30)), 4, 3000);
        assert_eq!(phase_in(d.path()), Some(("é".repeat(24), 3000)));
    }

    /// A copy whose sequence the next read doesn't repeat (a write begun after it was copied: its
    /// text may be torn) isn't taken; one the next read repeats is.
    #[test]
    fn a_phase_is_taken_only_when_two_reads_agree() {
        let file = |seq: u64, text: &[u8]| {
            let mut b = vec![0u8; FILE_LEN];
            b[SLOTS * 8..SLOTS * 8 + 8].copy_from_slice(&seq.to_le_bytes());
            b[(SLOTS + 2) * 8..(SLOTS + 2) * 8 + text.len()].copy_from_slice(text);
            b
        };
        let reads = |v: Vec<Vec<u8>>| {
            let v = std::cell::RefCell::new(v.into_iter());
            settled(move || v.borrow_mut().next())
        };
        // (Torn at seq 2, the writer then at 4 each later read: the torn copy never taken, the
        // whole one is.)
        assert_eq!(
            reads(vec![file(2, b"torn\0\0"), file(4, b"whole"), file(4, b"whole"), file(4, b"whole")]),
            Some(("whole".into(), 0))
        );
        assert_eq!(reads(vec![file(2, b"a"), file(3, b"b"), file(4, b"c"), file(6, b"d")]), None);
        assert_eq!(reads(vec![file(2, b"same"), file(2, b"same")]), Some(("same".into(), 0)));
    }

    /// What `set_phase` costs (the timings' every top-level phase opened and ended calls it).
    #[test]
    fn setting_the_phase_is_cheap() {
        let t = std::time::Instant::now();
        for i in 0..100_000 {
            set_phase(if i % 2 == 0 { "records saved\tnas-write" } else { "" });
        }
        let ns = t.elapsed().as_nanos() / 100_000;
        eprintln!("set_phase: {ns} ns a call");
        assert!(ns < 20_000, "{ns} ns a call");
    }
}
