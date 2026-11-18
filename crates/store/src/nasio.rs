//! This process's I/O counts, kept where they happen: its NAS traffic (the store's readers, a
//! build's `Out` through buildstore::costin::wrote, the metadata walks costin::meta notes: bytes read
//! (an upload's read-backs among them, and apart too: `ReadBack`) and written, files opened for
//! reading, files made, folder entries listed, files statted and deleted), and beside it the bytes
//! read from this Mac's own disk (`Local`: crate::range's mapped and local files, crate::cachefile's
//! reads, and the NAS reads this Mac's page cache answered, `Reads`) and the GeoTIFFs opened. The one per-process counter set: a build target's input measures
//! (buildstore::costin) are its deltas.
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
//! its own as it ends (`take_program`; roads::unit's `run`).
//!
//! After the counts, the same file says what the process is doing now (`set_phase`: the phase
//! jobrt::timings has open on a job's main thread, and since when), written under a sequence
//! number (odd while it's written) so a reader never takes half of one; never summed or folded.
//! The agent reads it from the newest running process's file at the folder's top (`phase_in`).
//!
//! After the phase, the last `RING` NAS files this process opened for reading or made (`opened`,
//! `wrote`: each one's path, its bytes and how long its open or its copy took), in a ring of slots
//! each written under a sequence number of its own: stores into the mapping, no write call, nothing
//! on the NAS. The agent reads the running processes' rings once a second (`notes_in`) for its live
//! file stream (docs/pool.md §10, Live). A ring's slot overwritten before the agent read it is
//! missed: the stream shows the recent files, not every one.
//!
//! Last, the wall time this process has waited on the NAS and the network so far (ns, those of
//! store::waits' kinds, by the wall clock: `waited`), which jobrt::timings
//! adds to as it samples (`add_waited`, once a second); summed as the counts are, its ended
//! processes' folded into `BASE` after the counts and its programs' taken in (`waited_in`). The
//! agent reads it to tell a job's computing from its waiting (crate's caller, agentcore::agent::
//! threads: its live thread control). An older file (shorter) says none.
//!
//! After it, the bytes of NAS files this process read that this Mac's page cache held, so the NAS
//! served none of them (`held`, counted as `Local` too; every NAS read is told one or the other by
//! its time, `Reads`, task #1450): with `ReadBytes`, what the job asked of the NAS whichever
//! answered (task #1419). Summed, folded and taken as the wall time waited is (`held_in`); an older
//! file says none.
//!
//! **Which answered** (`Reads`, task #1450, measured 18 Nov against the SMB session's own received
//! bytes, `smbutil multichannel`): a read this Mac's cache answers takes microseconds (a 4 KB read
//! 1-5 µs, 64 KB up to 0.13 ms, 1 MB up to 0.36 ms, 4 MB up to 0.94 ms on the M4 and the mini), the
//! NAS's never under a round trip (0.6 ms pinged from the wired mini, 0.8 ms its quickest 4 KB read;
//! 4-7 ms on the MacBooks' Wi-Fi), so a read quicker than `cached`'s bound is the cache's. Its time
//! alone misses the kernel's read-ahead: past a sequential read the kernel fetches on ahead, and
//! the reads after it come from the cache though the NAS served them for this very reader (a cold
//! 53 MB file read 1 MB at a time: 44 of its 51 reads under 0.5 ms). So a quick read following one
//! the NAS answered (where it ended, within a page: as the kernel's own read-ahead tells a
//! sequential read) is the NAS's too, while within the kernel's read-ahead reach of the last slow
//! one (`kern.speculative_prefetch_max`). Against the truth: a cold or a warm file read through,
//! one warm at its start and cold after, random reads of a file part warm: within 0.1 % of the
//! bytes asked (the wire carries whole pages, so a 4 KB read moves about 20 KB and a 64 KB one
//! about 80 KB: not counted); a file cold at its start and warm after, read through in one run, is
//! counted the NAS's from its first slow read to the read-ahead's reach (a 46 MB file half warm:
//! all of it counted, the NAS sending half), never less. No call of its own: two clock reads and a
//! lock a read.

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
    /// Bytes the NAS served (`Reads`: not those this Mac's cache answered).
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
    /// Bytes read from this Mac's own disk, or of a NAS file from its page cache (`Reads`).
    Local = 7,
    /// GeoTIFFs opened.
    Tiffs = 8,
    /// Of `ReadBytes`, those an upload read back to check what it wrote (crate::naming's
    /// `write_atomic`, `read_back`): a job's NAS traffic, not its step's inputs (buildstore::costin
    /// keeps them apart, `read_back`).
    ReadBack = 9,
}

/// The counts in a file (`Count`'s).
pub const SLOTS: usize = 10;

/// A process's counts, in `Count`'s order.
pub type Counts = [u64; SLOTS];

type Cells = [AtomicU64; SLOTS];

/// The phase's text's room (bytes; longer is cut at a character's end).
pub const PHASE_MAX: usize = 48;

/// The files a process's ring keeps (`note`).
pub const RING: usize = 32;

/// A noted path's room (bytes; a longer one keeps its end, cut at a character's start).
pub const PATH_MAX: usize = 112;

/// A ring's slot: its sequence number (odd while written; `2 n + 2` once file `n` is in it), when
/// the file was noted (unix ms), its kind (`Kind`), bytes, ms, path (zero-padded).
#[repr(C)]
struct Slot {
    seq: AtomicU64,
    t_ms: AtomicU64,
    kind: AtomicU64,
    bytes: AtomicU64,
    ms: AtomicU64,
    path: [AtomicU64; PATH_MAX / 8],
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            seq: AtomicU64::new(0),
            t_ms: AtomicU64::new(0),
            kind: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            ms: AtomicU64::new(0),
            path: [const { AtomicU64::new(0) }; PATH_MAX / 8],
        }
    }
}

/// A slot's length (u64 words).
const SLOT_WORDS: usize = std::mem::size_of::<Slot>() / 8;

/// A counts file's layout: the counts, then the phase (`set_phase`): its sequence number, when it
/// was set (unix ms), its text (zero-padded); then the files noted (`note`): how many so far, and
/// the ring; then the wall time waited (`add_waited`), then the NAS bytes held here (`held`).
#[repr(C)]
struct File {
    counts: Cells,
    seq: AtomicU64,
    since_ms: AtomicU64,
    text: [AtomicU64; PHASE_MAX / 8],
    noted: AtomicU64,
    ring: [Slot; RING],
    waited: AtomicU64,
    held: AtomicU64,
}

/// Where the phase ends in a counts file (an older one's ends there: no ring).
const PHASE_END: usize = (SLOTS + 2) * 8 + PHASE_MAX;

/// Where the ring begins.
const RING_AT: usize = PHASE_END + 8;

/// A counts file's length.
const FILE_LEN: usize = std::mem::size_of::<File>();

/// The words after a counts file's ring (and after `BASE`'s counts), in order.
#[derive(Clone, Copy)]
enum Tail {
    /// The wall time waited (`add_waited`).
    Waited = 0,
    /// The NAS bytes held here (`held`).
    Held = 1,
}

/// Where the tail's words begin in a counts file, and in `BASE` (after the counts).
const TAIL_AT: usize = std::mem::offset_of!(File, waited);
const BASE_TAIL_AT: usize = SLOTS * 8;

static OWN: File = File {
    counts: [const { AtomicU64::new(0) }; SLOTS],
    seq: AtomicU64::new(0),
    since_ms: AtomicU64::new(0),
    text: [const { AtomicU64::new(0) }; PHASE_MAX / 8],
    noted: AtomicU64::new(0),
    ring: [const { Slot::new() }; RING],
    waited: AtomicU64::new(0),
    held: AtomicU64::new(0),
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
    if b.len() < PHASE_END {
        return None;
    }
    let word = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap_or_default());
    let seq = word(SLOTS);
    if seq == 0 || seq % 2 == 1 {
        return None;
    }
    let text = &b[(SLOTS + 2) * 8..PHASE_END];
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

/// Adds `ns` to the wall time this process has waited (jobrt::timings, as it samples store::waits).
pub fn add_waited(ns: u64) {
    if ns > 0 {
        file().waited.fetch_add(ns, Ordering::Relaxed);
    }
}

/// The wall time this process has waited so far (ns: `add_waited`'s, its programs' taken in).
pub fn own_waited() -> u64 {
    file().waited.load(Ordering::Relaxed)
}

/// `n` bytes of a NAS file read here that this Mac's page cache held (no NAS read: `held`; `Reads`
/// counts them `Local` too).
pub fn held(n: u64) {
    if n > 0 {
        file().held.fetch_add(n, Ordering::Relaxed);
    }
}

/// The NAS bytes held here this process read so far (`held`'s, its programs' taken in).
pub fn own_held() -> u64 {
    file().held.load(Ordering::Relaxed)
}

/// File `path` opened on the NAS for reading (`bytes` long; its open took `ms`): counted and noted.
pub fn opened(path: &Path, bytes: u64, ms: u64) {
    add(Count::Files, 1);
    note(Kind::Read, path, bytes, ms);
}

/// `n` bytes the NAS served (a reader's are told by `Reads`).
pub fn read(n: u64) {
    add(Count::ReadBytes, n);
}

/// The quickest a NAS read of no bytes is answered, and the rate past it: a read quicker than
/// `CACHED_US` µs plus its bytes at `CACHED_RATE` bytes a second was this Mac's cache's (`cached`;
/// the module's doc: the cache's 1 MB reads take up to 0.36 ms, the bound 0.77 ms, the NAS's at
/// least 9 ms on 1 GbE).
pub const CACHED_US: u64 = 250;
pub const CACHED_RATE: u64 = 2_000_000_000;

/// Whether a NAS read of `n` bytes taking `took` (its call's own time) was answered by this Mac's
/// page cache (the module's doc, Which answered).
pub fn cached(n: u64, took: std::time::Duration) -> bool {
    took.as_nanos() < u128::from(CACHED_US) * 1000 + u128::from(n) * 1_000_000_000 / u128::from(CACHED_RATE)
}

/// One NAS file's reads, in the order they're made (one per open file, copy or whole read; its
/// readers' threads may share it): each told the NAS's (`ReadBytes`) or this Mac's cache's (`held`
/// and `Local`) by its time, a quick read following one the NAS answered the NAS's too while within
/// the kernel's read-ahead reach (the module's doc, Which answered).
#[derive(Debug, Default)]
pub struct Reads(std::sync::Mutex<Run>);

/// Where a file's last read ended, and, while its run is the NAS's, the bytes read since its last
/// read the NAS answered.
#[derive(Debug, Default)]
struct Run {
    end: Option<u64>,
    nas_since: Option<u64>,
}

impl Reads {
    /// A read of `n` bytes at `off` that took `took` (its call's own time: not a wait for a turn or
    /// a thread), counted.
    pub fn read(&self, off: u64, n: u64, took: std::time::Duration) {
        if n == 0 {
            return;
        }
        if self.nas(off, n, took) {
            read(n);
        } else {
            add(Count::Local, n);
            held(n);
        }
    }

    /// Whether that read was the NAS's.
    fn nas(&self, off: u64, n: u64, took: std::time::Duration) -> bool {
        let mut r = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let follows = r.end.is_some_and(|e| off.abs_diff(e) <= page());
        r.end = Some(off + n);
        r.nas_since = match r.nas_since {
            _ if !cached(n, took) => Some(0),
            Some(s) if follows && s + n <= read_ahead() => Some(s + n),
            _ => None,
        };
        r.nas_since.is_some()
    }
}

/// This Mac's page size (the kernel's read-ahead tells a sequential read within one).
fn page() -> u64 {
    static P: OnceLock<u64> = OnceLock::new();
    // SAFETY: sysconf with a valid name.
    *P.get_or_init(|| match unsafe { libc::sysconf(libc::_SC_PAGESIZE) } {
        p if p > 0 => p as u64,
        _ => 16384,
    })
}

/// The furthest the kernel reads ahead of a sequential reader (`kern.speculative_prefetch_max`:
/// 192 MB on the M4); unbounded where it can't be told, so a run is never counted short.
fn read_ahead() -> u64 {
    static R: OnceLock<u64> = OnceLock::new();
    *R.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            let (mut v, mut len) = (0u64, std::mem::size_of::<u64>());
            // SAFETY: sysctlbyname with a NUL-terminated name, an out buffer of `len` bytes we own,
            // and no new value.
            let ok = unsafe {
                libc::sysctlbyname(
                    c"kern.speculative_prefetch_max".as_ptr(),
                    (&mut v as *mut u64).cast(),
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            } == 0;
            if ok && v > 0 {
                // (An int on some versions: the low bytes.)
                return if len == 4 { v & 0xffff_ffff } else { v };
            }
        }
        u64::MAX
    })
}

/// `n` bytes an upload read back from the NAS (crate::naming's read-back, past the cache): NAS reads
/// like any (`read`), and counted apart too (`ReadBack`).
pub fn read_back(n: u64) {
    read(n);
    add(Count::ReadBack, n);
}

/// File `path` of `n` bytes made on the NAS in one write (it took `ms`): its bytes and the file
/// counted, and noted.
pub fn wrote(path: &Path, n: u64, ms: u64) {
    written(n);
    made(path, n, ms);
}

/// `n` bytes written to the NAS: an upload's as each write goes (crate::naming's, crate::nasfs's
/// copies and files), so its job's beats see a long upload move (task #990).
pub fn written(n: u64) {
    add(Count::WroteBytes, n);
}

/// File `path` made on the NAS, its `n` bytes counted as they were written (`written`; its copy
/// took `ms`): counted as a file made, and noted.
pub fn made(path: &Path, n: u64, ms: u64) {
    add(Count::Creates, 1);
    note(Kind::Wrote, path, n, ms);
}

/// What a noted file was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Opened for reading.
    Read = 1,
    /// Made.
    Wrote = 2,
}

/// A file noted in a process's ring (`notes_in`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    /// Its place among its process's notes (0 the first).
    pub n: u64,
    /// Unix ms.
    pub t_ms: u64,
    pub kind: Kind,
    pub bytes: u64,
    pub ms: u64,
    pub path: String,
}

/// Notes a file in this process's ring (module doc): a few relaxed stores, no lock, no call.
fn note(kind: Kind, path: &Path, bytes: u64, ms: u64) {
    let f = file();
    let n = f.noted.fetch_add(1, Ordering::Relaxed);
    let s = &f.ring[(n % RING as u64) as usize];
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    s.seq.store(2 * n + 1, Ordering::Relaxed);
    std::sync::atomic::fence(Ordering::Release);
    s.t_ms.store(t, Ordering::Relaxed);
    s.kind.store(kind as u64, Ordering::Relaxed);
    s.bytes.store(bytes, Ordering::Relaxed);
    s.ms.store(ms, Ordering::Relaxed);
    for (cell, w) in s.path.iter().zip(tail(&path.to_string_lossy()).chunks_exact(8)) {
        cell.store(u64::from_le_bytes(w.try_into().unwrap_or_default()), Ordering::Relaxed);
    }
    s.seq.store(2 * n + 2, Ordering::Release);
}

/// `path`'s last `PATH_MAX` bytes, from a character's start, zero-padded.
fn tail(path: &str) -> [u8; PATH_MAX] {
    let mut from = path.len().saturating_sub(PATH_MAX);
    while !path.is_char_boundary(from) {
        from += 1;
    }
    let mut b = [0u8; PATH_MAX];
    b[..path.len() - from].copy_from_slice(&path.as_bytes()[from..]);
    b
}

/// A counts file's notes (its bytes, read whole): its ring's slots written whole (an even sequence
/// past none); an older file's (no ring), none.
fn notes_of(b: &[u8]) -> Vec<Note> {
    if b.len() < FILE_LEN {
        return Vec::new();
    }
    let word = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap_or_default());
    (0..RING)
        .filter_map(|k| {
            let at = RING_AT + k * SLOT_WORDS * 8;
            let seq = word(at);
            if seq == 0 || seq % 2 == 1 {
                return None;
            }
            let kind = match word(at + 16) {
                1 => Kind::Read,
                2 => Kind::Wrote,
                _ => return None,
            };
            let p = &b[at + 40..at + 40 + PATH_MAX];
            let path = String::from_utf8_lossy(&p[..p.iter().position(|&c| c == 0).unwrap_or(PATH_MAX)]).into_owned();
            Some(Note { n: seq / 2 - 1, t_ms: word(at + 8), kind, bytes: word(at + 24), ms: word(at + 32), path })
        })
        .collect()
}

/// The files folder `dir`'s running processes noted (its top's and its programs' folders', one
/// level), each by its file's name: each file read twice, a note taken only when both reads have it
/// alike (a slot rewritten between is left for the next look).
pub fn notes_in(dir: &Path) -> Vec<(String, Note)> {
    let mut out = Vec::new();
    let look = |d: &Path, out: &mut Vec<(String, Note)>| {
        let Ok(rd) = std::fs::read_dir(d) else { return Vec::new() };
        let mut subs = Vec::new();
        for e in rd.flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            if name.starts_with(SUB) {
                subs.push(e.path());
                continue;
            }
            let Some(pid) = name.split_once('-').and_then(|(p, _)| p.parse::<i32>().ok()) else { continue };
            if !running(pid) {
                continue;
            }
            let (Ok(a), Ok(b)) = (std::fs::read(e.path()), std::fs::read(e.path())) else { continue };
            let again = notes_of(&b);
            for n in notes_of(&a) {
                if again.contains(&n) {
                    out.push((name.clone(), n));
                }
            }
        }
        subs
    };
    for s in look(dir, &mut out) {
        look(&s, &mut out);
    }
    out
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

/// A counts file's tail word `t` (its bytes, read whole; `name` its file's): a process's file's
/// after its ring, `BASE`'s after its counts; none in an older (shorter) one.
fn tail_of(name: &str, b: &[u8], t: Tail) -> u64 {
    let at = if name == BASE { BASE_TAIL_AT } else { TAIL_AT } + t as usize * 8;
    b.get(at..at + 8).map_or(0, |w| u64::from_le_bytes(w.try_into().unwrap_or_default()))
}

/// Folder `dir`'s wall time waited (ns, `add_waited`'s): its files' summed (`BASE` and
/// `<pid>-<nonce>`, its running programs' folders one level down), as `sum_dir` sums the counts;
/// None when the folder isn't there.
pub fn waited_in(dir: &Path) -> Option<u64> {
    tail_in(dir, Tail::Waited)
}

/// Folder `dir`'s NAS bytes held here (`held`'s), summed as `waited_in` sums the time waited.
pub fn held_in(dir: &Path) -> Option<u64> {
    tail_in(dir, Tail::Held)
}

fn tail_in(dir: &Path, t: Tail) -> Option<u64> {
    fn add(dir: &Path, subs: bool, t: Tail) -> Option<u64> {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut n = 0;
        for e in rd.flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            if name.starts_with(SUB) {
                if subs {
                    n += add(&e.path(), false, t).unwrap_or(0);
                }
            } else if name == BASE || name.split_once('-').is_some_and(|(p, _)| p.parse::<u32>().is_ok()) {
                n += std::fs::read(e.path()).map_or(0, |b| tail_of(&name, &b, t));
            }
        }
        Some(n)
    }
    add(dir, true, t)
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
    add_waited(waited_in(&at).unwrap_or(0));
    held(held_in(&at).unwrap_or(0));
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
    // (No processes to fold.)
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
    let was = std::fs::read(&base).ok();
    let mut sum = was.as_deref().map(parse).unwrap_or_default();
    let tails = [Tail::Waited, Tail::Held];
    let mut tail = tails.map(|t| was.as_deref().map_or(0, |b| tail_of(BASE, b, t)));
    let mut taken = Vec::new();
    for p in ended {
        let Ok(b) = std::fs::read(&p) else { continue };
        for (o, v) in sum.iter_mut().zip(parse(&b)) {
            *o += v;
        }
        for (s, t) in tail.iter_mut().zip(tails) {
            *s += tail_of("", &b, t);
        }
        taken.push(p);
    }
    let tmp = dir.join(format!("{BASE}.tmp"));
    let mut out = bytes(&sum);
    out.extend(tail.iter().flat_map(|w| w.to_le_bytes()));
    if std::fs::write(&tmp, out).and_then(|_| std::fs::rename(&tmp, &base)).is_ok() {
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

    /// The wall time waited and the NAS bytes held here are summed over a folder's files as the
    /// counts are: a running process's, a program's folder's, an ended one's folded into `BASE`
    /// (kept through a second fold), a program's taken into its runner's own; an older file's (no
    /// word for either, or for the bytes held alone: one before task #1419, its time waited its last
    /// word) says none of what it lacks; and the counts' sums never include them.
    #[test]
    fn the_tail_words_are_summed_folded_and_taken() {
        let d = tempfile::tempdir().unwrap();
        let a = mapped(d.path()).unwrap();
        a.waited.fetch_add(300, Ordering::Relaxed);
        a.held.fetch_add(3000, Ordering::Relaxed);
        let file = |waited: u64, held: u64, len: usize| {
            let mut b = vec![0u8; FILE_LEN];
            b[TAIL_AT..TAIL_AT + 8].copy_from_slice(&waited.to_le_bytes());
            b[TAIL_AT + 8..TAIL_AT + 16].copy_from_slice(&held.to_le_bytes());
            b.truncate(len);
            b
        };
        // (An ended process's, one from before the bytes held, and an older one's with the counts
        // alone.)
        std::fs::write(d.path().join("2147483646-1"), file(40, 400, FILE_LEN)).unwrap();
        std::fs::write(d.path().join("2147483646-4"), file(2, 99, FILE_LEN - 8)).unwrap();
        std::fs::write(d.path().join("2147483646-2"), [7u8; SLOTS * 8]).unwrap();
        let sub = d.path().join(format!("{SUB}1-0"));
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("2147483646-3"), file(5, 50, FILE_LEN)).unwrap();
        assert_eq!((waited_in(d.path()), held_in(d.path())), (Some(347), Some(3450)));
        let counts = sum_dir(d.path()).unwrap();
        fold_ended(d.path());
        fold_ended(d.path());
        assert!(!d.path().join("2147483646-1").exists());
        assert_eq!((waited_in(d.path()), held_in(d.path())), (Some(347), Some(3450)), "folded into the base");
        assert_eq!(sum_dir(d.path()).unwrap(), counts, "the counts as they were");
        let (w, h) = (own_waited(), own_held());
        take_program(&sub);
        assert!(own_waited() >= w + 5 && own_held() >= h + 50);
        assert!(waited_in(&d.path().join("missing")).is_none());
    }

    /// A read is the cache's when quicker than a NAS answer could be, the bound growing with its
    /// bytes; a quick read following one the NAS answered is the NAS's (the kernel's read-ahead),
    /// one elsewhere or after the cache's is the cache's, and past the read-ahead's reach the run
    /// ends; what each is told is counted so.
    #[test]
    fn reads_are_told_the_nas_or_the_caches_by_their_time_and_run() {
        let (us, ms) = (std::time::Duration::from_micros, std::time::Duration::from_millis);
        assert!(cached(4096, us(5)) && !cached(4096, us(600)), "a 4 KB read: µs the cache's, a round trip the NAS's");
        assert!(cached(1 << 20, us(360)) && !cached(1 << 20, ms(9)), "a MB");
        assert!(cached(4 << 20, us(940)) && !cached(4 << 20, ms(5)), "4 MB");
        let mb = 1u64 << 20;
        // (Warm: every read quick, none the NAS's.)
        let warm = Reads::default();
        assert!((0..8).all(|k| !warm.nas(k * mb, mb, us(200))));
        // (Cold: the first slow, the read-ahead's quick ones after it the NAS's too.)
        let cold = Reads::default();
        assert!(cold.nas(0, mb, ms(200)));
        assert!((1..8).all(|k| cold.nas(k * mb, mb, us(200))));
        // (Warm at its start, cold after: the NAS's from its first slow read; a quick read
        // elsewhere, after it, the cache's, and the reads following that one too.)
        let part = Reads::default();
        assert!(!part.nas(0, mb, us(200)) && !part.nas(mb, mb, us(200)));
        assert!(part.nas(2 * mb, mb, ms(30)) && part.nas(3 * mb, mb, us(200)));
        assert!(!part.nas(40 * mb, mb, us(200)) && !part.nas(41 * mb, mb, us(200)));
        // (Within a page of where the last ended still follows it.)
        assert!(part.nas(60 * mb, 4096, ms(5)) && part.nas(60 * mb + 4096 + page(), 4096, us(5)));
        // (Past the read-ahead's reach the run is the cache's again.)
        if read_ahead() < u64::MAX / 4 {
            let far = Reads::default();
            assert!(far.nas(0, mb, ms(30)) && far.nas(mb, read_ahead() - mb, us(10)));
            assert!(!far.nas(read_ahead(), 2 * mb, us(200)));
        }
        let (r, h, l) = (get(Count::ReadBytes), own_held(), get(Count::Local));
        let f = Reads::default();
        f.read(0, 3000, ms(10));
        f.read(3000, 5000, us(3));
        f.read(9 << 20, 700, us(3));
        f.read(0, 0, ms(10));
        assert!(get(Count::ReadBytes) >= r + 8000 && own_held() >= h + 700 && get(Count::Local) >= l + 700);
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
        let t = crate::testkit::thread_cpu_ns();
        for i in 0..100_000 {
            set_phase(if i % 2 == 0 { "records saved\tnas-write" } else { "" });
        }
        let ns = (crate::testkit::thread_cpu_ns() - t) / 100_000;
        eprintln!("set_phase: {ns} ns a call");
        assert!(ns < 20_000, "{ns} ns of CPU a call"); // (bound ok: CPU time, not wall)
    }

    /// A file's notes are read back from its counts file, the newest `RING` kept, a long path's end
    /// kept; one mid-write and an older file's (no ring) aren't taken; the counts' sums don't change.
    #[test]
    fn the_files_noted_are_read_from_the_ring() {
        let d = tempfile::tempdir().unwrap();
        let f = mapped(d.path()).unwrap();
        let put = |n: u64, kind: Kind, path: &str| {
            let s = &f.ring[(n % RING as u64) as usize];
            s.seq.store(2 * n + 1, Ordering::Relaxed);
            s.kind.store(kind as u64, Ordering::Relaxed);
            s.bytes.store(n * 10, Ordering::Relaxed);
            s.ms.store(n, Ordering::Relaxed);
            s.t_ms.store(1000 + n, Ordering::Relaxed);
            for (c, w) in s.path.iter().zip(tail(path).chunks_exact(8)) {
                c.store(u64::from_le_bytes(w.try_into().unwrap()), Ordering::Relaxed);
            }
            s.seq.store(2 * n + 2, Ordering::Relaxed);
        };
        for n in 0..(RING as u64 + 3) {
            put(n, if n % 2 == 0 { Kind::Read } else { Kind::Wrote }, &format!("terrain/6-{n}.z"));
        }
        let mut got: Vec<Note> = notes_in(d.path()).into_iter().map(|(_, n)| n).collect();
        got.sort_by_key(|n| n.n);
        assert_eq!(got.len(), RING);
        assert_eq!(got[0].n, 3);
        let last = &got[RING - 1];
        assert_eq!(
            (last.kind, last.bytes, last.ms, last.t_ms, last.path.as_str()),
            (Kind::Read, 340, 34, 1034, "terrain/6-34.z")
        );
        // (Mid-write: not taken.)
        f.ring[(34 % RING as u64) as usize].seq.store(2 * 66 + 1, Ordering::Relaxed);
        assert_eq!(notes_in(d.path()).len(), RING - 1);
        // (A long path's end, from a character's start.)
        let long = format!("/Volumes/x/{}é.z", "a".repeat(PATH_MAX));
        assert_eq!(String::from_utf8_lossy(&tail(&long)), format!("{}é.z", "a".repeat(PATH_MAX - 4)));
        let cut = format!("{}é", "a".repeat(PATH_MAX - 1));
        assert_eq!(
            String::from_utf8_lossy(&tail(&cut)).trim_end_matches('\0'),
            "a".repeat(PATH_MAX - 1).get(1..).unwrap().to_owned() + "é"
        );
        // (An older file's, counts and phase alone: none.)
        assert!(notes_of(&[0u8; PHASE_END]).is_empty());
        assert_eq!(sum_dir(d.path()).unwrap(), [0; SLOTS]);
    }

    /// What noting a file costs (every NAS file opened or made calls it).
    #[test]
    fn noting_a_file_is_cheap() {
        let p = Path::new("/Volumes/personal/projects/scenic-roads/terrain/6-4-36.z");
        let t = crate::testkit::thread_cpu_ns();
        for i in 0..100_000 {
            note(if i % 2 == 0 { Kind::Read } else { Kind::Wrote }, p, i, 3);
        }
        let ns = (crate::testkit::thread_cpu_ns() - t) / 100_000;
        eprintln!("note: {ns} ns a call");
        assert!(ns < 20_000, "{ns} ns of CPU a call"); // (bound ok: CPU time, not wall)
    }
}
