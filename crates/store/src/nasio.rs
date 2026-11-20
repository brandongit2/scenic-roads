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
//! one (`kern.speculative_prefetch_max`) and what this Mac's links could have carried since the
//! run began: at their rate (`ifi_baudrate`, summed) since its first slow read began, or the bytes
//! they received (summed) since its first quick read, whichever is more (a Wi-Fi's stated rate was
//! below what it carried: 45-58 MB/s stated against 61-71 MB/s streamed). So a cached read held up
//! past the bound on a loaded Mac (1.4 ms for a MB against 0.77) counts its own bytes and not a
//! run after it, for a reader quicker than the links (a slower one, up to the read-ahead's
//! reach). A run's first quick read the stated rate doesn't cover is the cache's, the run
//! kept for one more (its received bytes are counted from that read on); a second ends it.
//! Against the truth (the M4 under load 12-120, 18 Nov): cold files of 98-129 MB read through
//! counted within 0.02 %; 75 warm reads of a 105 MB file, 7.9 GB, counted 1-2 MB the NAS's (a held-up
//! read, a MB, at most one a run) against 87 KB sent; a file warm at its start and cold after,
//! exact, or a MB short (that first quick read); one cold for 55 MB and warm for 38 MB after,
//! 64-85 MB counted against 58 MB sent (its warm part while the links' rate and other traffic left
//! room; 114 MB before the bound). Not
//! counted: the whole pages the wire carries for a small read (a 4 KB read moves about 20 KB, a 64
//! KB one about 80 KB). No call of its own: two clock reads and a lock a read; a run's first quick
//! read asks the links (a sysctl, about 0.1 ms), and again only when its bound falls short.

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
/// the kernel's read-ahead reach and what the links could have carried since (`Link`; the module's
/// doc, Which answered). Up to `RUNS` sequential runs of it are followed at once (readers
/// interleaving their runs on one file, as a shared PlainFile's threads do).
#[derive(Debug, Default)]
pub struct Reads(std::sync::Mutex<[Option<Run>; RUNS]>);

/// The NAS's runs a file's `Reads` follows at once (the one read longest ago gives way).
pub const RUNS: usize = 4;

/// A sequential run of reads the NAS answered: where its last read ended; when its first slow read
/// began; the links as its first quick read came (`Link`, asked then: a run of slow reads alone,
/// random reads', never asks) and the bytes counted the NAS's before it; the bytes counted the
/// NAS's since it began and since its last slow read; the bytes the links had received since its
/// links were asked, as last asked; when it was read last (which gives way); and whether a quick
/// read of it has fallen short of the links' bound (the next to fall short ends it).
#[derive(Debug, Clone, Copy)]
struct Run {
    end: u64,
    began: std::time::Instant,
    link: Option<Link>,
    before_link: u64,
    since_began: u64,
    since_slow: u64,
    received: u64,
    used: std::time::Instant,
    short: bool,
}

/// This Mac's links: their rate (bytes a second: their up interfaces' own,
/// `ifi_baudrate`, summed; the Wi-Fi's is its current rate, which a stream's received bytes were
/// measured past) and the bytes they had received (64-bit counters, summed); None where not told.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Link {
    pub rate: Option<u64>,
    pub received: Option<u64>,
}

impl Reads {
    /// A read of `n` bytes at `off` that took `took` (its call's own time: not a wait for a turn or
    /// a thread), ending now, counted.
    pub fn read(&self, off: u64, n: u64, took: std::time::Duration) {
        if n == 0 {
            return;
        }
        if self.nas(off, n, took, std::time::Instant::now(), &link) {
            read(n);
        } else {
            add(Count::Local, n);
            held(n);
        }
    }

    /// Whether that read, ended at `now`, was the NAS's; `link` this Mac's links now, asked at a
    /// run's first quick read and when what they could have carried falls short.
    fn nas(
        &self,
        off: u64,
        n: u64,
        took: std::time::Duration,
        now: std::time::Instant,
        link: &dyn Fn() -> Link,
    ) -> bool {
        let mut runs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let at = runs.iter().position(|r| r.is_some_and(|r| off.abs_diff(r.end) <= page()));
        if !cached(n, took) {
            match at.and_then(|k| runs[k].as_mut()) {
                Some(r) => (r.end, r.since_began, r.since_slow, r.used) = (off + n, r.since_began + n, 0, now),
                None => {
                    // (A new run, in an empty slot or the one read longest ago.)
                    let k = runs
                        .iter()
                        .position(Option::is_none)
                        .unwrap_or_else(|| (0..RUNS).min_by_key(|&k| runs[k].map(|r| r.used)).unwrap_or(0));
                    let began = now.checked_sub(took).unwrap_or(now);
                    runs[k] = Some(Run {
                        end: off + n,
                        began,
                        link: None,
                        before_link: 0,
                        since_began: n,
                        since_slow: 0,
                        received: 0,
                        used: now,
                        short: false,
                    });
                }
            }
            return true;
        }
        let Some(k) = at else { return false };
        let Some(mut r) = runs[k] else { return false };
        let l = *r.link.get_or_insert_with(|| {
            r.before_link = r.since_began;
            link()
        });
        let (total, after) = (r.since_began + n, r.since_began - r.before_link + n);
        // (What the links could have carried: at their rate since the run began, or as they
        // received since they were asked, whichever is more; with neither told, no bound.)
        let by_rate = l.rate.map(|b| (now.duration_since(r.began).as_secs_f64() * b as f64) as u64);
        let mut fits = match (by_rate, l.received) {
            (None, None) => true,
            (Some(c), _) if total <= c => true,
            _ => after <= r.received,
        };
        if !fits {
            if let (Some(at), Some(was)) = (link().received, l.received) {
                r.received = at.saturating_sub(was);
                fits = after <= r.received;
            }
        }
        if fits && r.since_slow + n <= read_ahead() {
            (r.end, r.since_began, r.since_slow, r.used) = (off + n, total, r.since_slow + n, now);
            runs[k] = Some(r);
            true
        } else if r.since_slow + n <= read_ahead() && !r.short {
            // (The cache's, the run kept for one more: the links' received bytes are counted from
            // its first quick read, so that read's own arrived before they were asked.)
            (r.end, r.since_slow, r.used, r.short) = (off + n, r.since_slow + n, now, true);
            runs[k] = Some(r);
            false
        } else {
            runs[k] = None;
            false
        }
    }
}

/// This Mac's links now (`Link`: its up, running interfaces' but loopback, from the routing
/// table's list, NET_RT_IFLIST2; no call off this Mac).
fn link() -> Link {
    #[cfg(target_os = "macos")]
    {
        let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
        let mut len = 0usize;
        // SAFETY: sysctl asking the list's length (no buffer, no new value).
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 6, std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) } != 0 {
            return Link::default();
        }
        // (Room for interfaces come meanwhile; 8-byte aligned for the messages' 64-bit counters.)
        let mut buf = vec![0u64; (len + len / 4).div_ceil(8)];
        len = buf.len() * 8;
        // SAFETY: sysctl writing at most `len` bytes into `buf`, which holds that many.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 6, buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } != 0
        {
            return Link::default();
        }
        let b = &buf_bytes(&buf)[..len.min(buf.len() * 8)];
        let (mut each, mut at) = (Vec::new(), 0usize);
        while at + std::mem::size_of::<libc::if_msghdr2>() <= b.len() {
            // SAFETY: a message header within `b` (read unaligned: messages are packed one after another).
            let m: libc::if_msghdr2 = unsafe { std::ptr::read_unaligned(b[at..].as_ptr().cast()) };
            if m.ifm_msglen == 0 {
                break;
            }
            if i32::from(m.ifm_type) == libc::RTM_IFINFO2 {
                each.push((m.ifm_flags, m.ifm_data.ifi_baudrate, m.ifm_data.ifi_ibytes));
            }
            at += usize::from(m.ifm_msglen);
        }
        links_of(&each)
    }
    #[cfg(not(target_os = "macos"))]
    Link::default()
}

/// The links of interfaces `each` (their flags, rate in bits a second, bytes received): the up,
/// running, physical ones' (a rate of their own: not a tunnel, Tailscale's utun, whose traffic its
/// physical link carries and counts again, nor AWDL's or a bridge's, rate 0), loopback left out.
fn links_of(each: &[(i32, u64, u64)]) -> Link {
    let want = libc::IFF_UP | libc::IFF_RUNNING;
    let (mut bits, mut received) = (0u64, 0u64);
    for &(flags, baud, ibytes) in each {
        if flags & want == want && flags & libc::IFF_LOOPBACK == 0 && baud > 0 {
            bits += baud;
            received += ibytes;
        }
    }
    Link { rate: (bits > 0).then_some(bits / 8), received: Some(received) }
}

/// `w`'s bytes.
#[cfg(target_os = "macos")]
fn buf_bytes(w: &[u64]) -> &[u8] {
    // SAFETY: u64s viewed as their bytes, the same memory and length.
    unsafe { std::slice::from_raw_parts(w.as_ptr().cast(), std::mem::size_of_val(w)) }
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

/// A reader of a NAS file that opens it by its path itself (a library's `File::open`), its reads
/// counted as `Reads` counts a file's: each of its calls timed and told the NAS's or this Mac's
/// cache's. It reads its source `crate::sys::COPY_BUF` at a time and gives those out, as
/// crate::nasfs::NasFile does (a reader taking a few kB a call is told by calls too quick to say
/// which answered).
pub struct Counting<R> {
    r: R,
    reads: Reads,
    at: u64,
    buf: Vec<u8>,
    given: usize,
}

impl<R: std::io::Read> Counting<R> {
    pub fn new(r: R) -> Self {
        Self { r, reads: Reads::default(), at: 0, buf: Vec::new(), given: 0 }
    }
}

impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.given == self.buf.len() {
            self.buf.resize(crate::sys::COPY_BUF, 0);
            let t = std::time::Instant::now();
            let k = self.r.read(&mut self.buf)?;
            self.reads.read(self.at, k as u64, t.elapsed());
            self.buf.truncate(k);
            (self.at, self.given) = (self.at + k as u64, 0);
        }
        let n = out.len().min(self.buf.len() - self.given);
        out[..n].copy_from_slice(&self.buf[self.given..self.given + n]);
        self.given += n;
        Ok(n)
    }
}

/// How a program reads a file it opens where it lies: how many times it reads all of it, and how
/// many times it opens it (osmium's smart extract: three reads, four opens).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Passes {
    pub reads: u32,
    pub opens: u32,
}

impl Passes {
    /// Opened once, read once.
    pub const ONCE: Passes = Passes { reads: 1, opens: 1 };
}

/// How often a running program's bytes are credited (`ProgramReads`).
pub const CREDIT_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// What a program is credited as it runs: `expected` bytes at most, as the links receive them.
struct Credit {
    expected: u64,
    /// The links' received bytes at the last look (None: they aren't told), and the bytes credited
    /// so far (changed under this lock alone).
    state: std::sync::Mutex<(Option<u64>, u64)>,
}

impl Credit {
    /// The bytes to credit now that the links have received `now` in all: what they received since
    /// the last look (counters that went back, a restarted interface's, none), no more than what is
    /// left of `expected`.
    fn step(&self, now: Option<u64>) -> u64 {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (Some(was), Some(now)) = (g.0, now) else {
            g.0 = now.or(g.0);
            return 0;
        };
        g.0 = Some(now);
        let c = now.saturating_sub(was).min(self.expected - g.1);
        g.1 += c;
        c
    }

    fn look(&self, now: Option<u64>) {
        read(self.step(now));
    }
}

/// A program's reads of NAS files it opens where they lie (osmium given a path on the mount): its
/// own reads can't be seen. Its `expected` bytes (the files' sizes times its reads of them) are
/// credited as the NAS's while it runs, a look a second (`CREDIT_EVERY`) at the bytes this Mac's
/// links received since the last, no more than `expected` in all, so the job's NAS rates move as
/// the program reads, not in one span when it ends; the rest, when it ended (`ended`), is this Mac's
/// cache's (`held`, and `Local`): a program reading a warm file reads little off the links. A known
/// limit: the links are this Mac's, not the job's, so other traffic over them while the program
/// runs is credited to it too, up to `expected`; and with the links not told (not macOS) all of
/// `expected` is credited as the NAS's, when it ended. Dropped without `ended` (a failed run), it
/// keeps what it credited: those bytes crossed the links.
pub struct ProgramReads {
    credit: std::sync::Arc<Credit>,
    told: bool,
    received: std::sync::Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    ticker: Option<(std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>, std::thread::JoinHandle<()>)>,
}

impl ProgramReads {
    pub fn begin(expected: u64) -> Self {
        Self::begin_with(expected, CREDIT_EVERY, std::sync::Arc::new(|| link().received))
    }

    /// `begin`, looking every `every` at what `received` says the links received (tests').
    fn begin_with(
        expected: u64,
        every: std::time::Duration,
        received: std::sync::Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    ) -> Self {
        let first = received();
        let credit = std::sync::Arc::new(Credit { expected, state: std::sync::Mutex::new((first, 0)) });
        let stop = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let ticker = (first.is_some() && expected > 0).then(|| {
            let (c, s, r) = (credit.clone(), stop.clone(), received.clone());
            let h = std::thread::spawn(move || loop {
                let (m, cv) = &*s;
                let g = m.lock().unwrap_or_else(|e| e.into_inner());
                let (g, _) = cv.wait_timeout_while(g, every, |stopped| !*stopped).unwrap_or_else(|e| e.into_inner());
                if *g {
                    return;
                }
                drop(g);
                c.look(r());
            });
            (stop, h)
        });
        Self { credit, told: first.is_some(), received, ticker }
    }

    /// The looks ended, the thread joined.
    fn stop(&mut self) {
        if let Some((stop, h)) = self.ticker.take() {
            *stop.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            stop.1.notify_all();
            h.join().ok();
        }
    }

    /// The program ended having read each of `files` (a path, the bytes read of it in all its
    /// reads), opening each `opens` times: each counted opened, a last look at the links, and what
    /// of `expected` wasn't credited the cache's (all the NAS's with the links not told).
    pub fn ended(mut self, files: &[(&Path, u64)], opens: u32) {
        self.stop();
        self.credit.look((self.received)());
        let left = self.credit.expected - self.credit.state.lock().unwrap_or_else(|e| e.into_inner()).1;
        for (path, bytes) in files {
            opened(path, *bytes, 0);
            add(Count::Files, u64::from(opens.saturating_sub(1)));
        }
        if self.told {
            add(Count::Local, left);
            held(left);
        } else {
            read(left);
        }
    }
}

impl Drop for ProgramReads {
    fn drop(&mut self) {
        self.stop();
    }
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
    /// bytes; a quick read following one the NAS answered is the NAS's (the kernel's read-ahead)
    /// while the link could have carried it since the run's first slow read began and within the
    /// read-ahead's reach; one elsewhere, or after the cache's, is the cache's; what each is told
    /// is counted so.
    #[test]
    fn reads_are_told_the_nas_or_the_caches_by_their_time_and_run() {
        use std::time::{Duration, Instant};
        let (us, ms) = (Duration::from_micros, Duration::from_millis);
        assert!(cached(4096, us(5)) && !cached(4096, us(600)), "a 4 KB read: µs the cache's, a round trip the NAS's");
        assert!(cached(1 << 20, us(360)) && !cached(1 << 20, ms(9)), "a MB");
        assert!(cached(4 << 20, us(940)) && !cached(4 << 20, ms(5)), "4 MB");
        let mb = 1u64 << 20;
        // (A file's reads on a clock of their own, over links of `rate` that receive `each` bytes a
        // read: each read ends `gap` after the last, taking `took` of it.)
        struct File {
            r: Reads,
            t: Instant,
            rate: Option<u64>,
            each: Option<u64>,
            received: std::cell::Cell<u64>,
        }
        impl File {
            fn new(rate: Option<u64>) -> File {
                File { r: Reads::default(), t: Instant::now(), rate, each: None, received: Default::default() }
            }
            fn read(&mut self, off: u64, n: u64, took: Duration, gap: Duration) -> bool {
                self.t += gap.max(took);
                if let Some(e) = self.each {
                    self.received.set(self.received.get() + e);
                }
                let at = Link { rate: self.rate, received: self.each.map(|_| self.received.get()) };
                self.r.nas(off, n, took, self.t, &|| at)
            }
        }
        let gbe = Some(125_000_000);
        // (Warm: every read quick, none the NAS's.)
        let mut warm = File::new(gbe);
        assert!((0..8).all(|k| !warm.read(k * mb, mb, us(200), us(200))));
        // (Cold: the first slow, the read-ahead's quick ones after it the NAS's too, the link
        // having carried them while the first was answered.)
        let mut cold = File::new(gbe);
        assert!(cold.read(0, mb, ms(200), ms(200)));
        assert!((1..20).all(|k| cold.read(k * mb, mb, us(200), us(200))));
        // (Warm, one read held up past the bound on a loaded Mac: that read the NAS's, and the
        // quick ones after it the cache's, the link not carrying a MB in a fraction of a ms.)
        let mut busy = File::new(gbe);
        assert!(!busy.read(0, mb, us(200), us(200)) && busy.read(mb, mb, us(1400), us(1400)));
        assert!((2..100).all(|k| !busy.read(k * mb, mb, us(200), us(200))));
        // (Cold at its start, read as fast as the link carries it, warm after: the warm part the
        // cache's within a read of where the cold one ends.)
        let mut half = File::new(gbe);
        let link_mb = Duration::from_nanos(1_000_000_000 * mb / 125_000_000);
        assert!((0..10).all(|k| half.read(k * mb, mb, link_mb, link_mb)));
        let tail = (10..30).filter(|&k| half.read(k * mb, mb, us(200), us(200))).count();
        assert!(tail <= 1, "{tail} MB of the warm tail counted the NAS's");
        // (A link whose rate says less than it carries (a Wi-Fi's, measured so): the quick reads the
        // NAS's while its received bytes keep up; a warm run past a false slow read on a link
        // receiving nothing else, the cache's.)
        let mut wifi = File::new(Some(5_000_000));
        wifi.each = Some(2 * mb);
        assert!(wifi.read(0, mb, ms(20), ms(20)));
        assert!(!wifi.read(mb, mb, us(200), ms(10)), "its received bytes counted from here");
        assert!((2..30).all(|k| wifi.read(k * mb, mb, us(200), ms(10))), "kept up with by its received bytes");
        let mut quiet = File::new(Some(5_000_000));
        quiet.each = Some(4096);
        assert!(quiet.read(0, mb, us(1400), us(1400)));
        assert!((1..100).all(|k| !quiet.read(k * mb, mb, us(200), us(200))));
        // (Warm at its start, cold after: the NAS's from its first slow read.)
        let mut part = File::new(gbe);
        assert!(!part.read(0, mb, us(200), us(200)) && !part.read(mb, mb, us(200), us(200)));
        assert!(part.read(2 * mb, mb, ms(30), ms(30)) && part.read(3 * mb, mb, us(200), us(200)));
        // (A quick read elsewhere is the cache's; within a page of where a run ended still follows it.)
        assert!(!part.read(40 * mb, mb, us(200), us(200)));
        assert!(part.read(4 * mb + page(), 4096, us(5), us(5)));
        // (Two readers' runs on one file, interleaved: each followed, both the NAS's; a fifth run
        // takes the slot of the one read longest ago.)
        let mut two = File::new(gbe);
        assert!(two.read(0, mb, ms(50), ms(50)) && two.read(500 * mb, mb, ms(50), ms(50)));
        assert!((1..10).all(|k| two.read(k * mb, mb, us(200), ms(5)) && two.read((500 + k) * mb, mb, us(200), ms(5))));
        for k in 1..=RUNS as u64 {
            assert!(two.read(k * 1000 * mb, mb, ms(50), ms(50)));
        }
        assert!(!two.read(10 * mb, mb, us(200), us(200)), "the first run gave way");
        assert!(two.read((4000 + 1) * mb, mb, us(200), us(200)), "the newest kept");
        // (No rate known: no bound but the read-ahead's reach.)
        let mut unknown = File::new(None);
        assert!(unknown.read(0, mb, us(1400), us(1400)) && unknown.read(mb, mb, us(200), us(200)));
        if read_ahead() < u64::MAX / 4 {
            let mut far = File::new(None);
            assert!(far.read(0, mb, ms(30), ms(30)) && far.read(mb, read_ahead() - mb, us(10), us(10)));
            assert!(!far.read(read_ahead(), 2 * mb, us(200), us(200)), "past the read-ahead's reach");
        }
        let here = link();
        assert!(here.rate.is_none_or(|b| b > 0) && here.received.is_some(), "{here:?}");
        // (The physical links alone: a tunnel's (utun, rate 0) bytes are its link's again, AWDL's
        // aren't the share's; loopback and a down one not at all.)
        let up = libc::IFF_UP | libc::IFF_RUNNING;
        let each = [
            (up, 1_000_000_000, 500),
            (up, 0, 300),
            (up | libc::IFF_LOOPBACK, 0, 9000),
            (up | libc::IFF_LOOPBACK, 1_000_000_000, 9000),
            (libc::IFF_UP, 800_000_000, 70),
            (up, 600_000_000, 200),
        ];
        assert_eq!(links_of(&each), Link { rate: Some(200_000_000), received: Some(700) });
        assert_eq!(links_of(&[(up, 0, 300)]), Link { rate: None, received: Some(0) });
        let (r, h, l) = (get(Count::ReadBytes), own_held(), get(Count::Local));
        let f = Reads::default();
        f.read(0, 3000, ms(10));
        f.read(9 << 20, 700, us(3));
        f.read(0, 0, ms(10));
        assert!(get(Count::ReadBytes) >= r + 3000 && own_held() >= h + 700 && get(Count::Local) >= l + 700);
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

    /// A program is credited what the links received, no more than its bytes, never by counters
    /// that went back; with the links not told, nothing as it runs.
    #[test]
    fn a_program_is_credited_as_the_links_receive() {
        let c = Credit { expected: 1000, state: std::sync::Mutex::new((Some(10), 0)) };
        assert_eq!(c.step(Some(310)), 300);
        assert_eq!(c.step(Some(310)), 0);
        // (A restarted interface's counter: none, and the next look from there.)
        assert_eq!(c.step(Some(5)), 0);
        assert_eq!(c.step(Some(205)), 200);
        assert_eq!(c.step(Some(100_000)), 500);
        assert_eq!(c.step(Some(200_000)), 0);
        let dark = Credit { expected: 1000, state: std::sync::Mutex::new((None, 0)) };
        assert_eq!(dark.step(None), 0);
    }

    /// A long program's bytes show across its run, not in one span as it ends: credited while it
    /// runs (seen before it ended), and `expected` exactly in all once it has, the rest held.
    #[test]
    fn a_running_programs_bytes_are_credited_as_it_runs() {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
        let links = std::sync::Arc::new(AtomicU64::new(0));
        let l = links.clone();
        let (r0, h0) = (get(Count::ReadBytes), own_held());
        let p = ProgramReads::begin_with(
            10_000,
            std::time::Duration::from_millis(2),
            std::sync::Arc::new(move || Some(l.fetch_add(100, Relaxed))),
        );
        // (Seen served while the program is still running: no `ended` yet.)
        crate::testkit::eventually("its bytes credited as it runs", || get(Count::ReadBytes) - r0 >= 500);
        assert_eq!(own_held(), h0, "none held before it ended");
        p.ended(&[(Path::new("/Volumes/x/a.osm.pbf"), 6_000), (Path::new("/Volumes/x/b"), 4_000)], 4);
        assert_eq!(get(Count::ReadBytes) - r0 + own_held() - h0, 10_000);
        assert_eq!(get(Count::Files), 8, "each opened four times");
    }

    /// A program that failed (dropped without `ended`) leaves what it was credited, and no thread.
    #[test]
    fn a_program_that_failed_keeps_only_what_was_credited() {
        let r0 = get(Count::ReadBytes);
        let p = ProgramReads::begin_with(100, std::time::Duration::from_secs(3600), std::sync::Arc::new(|| Some(0)));
        drop(p);
        assert_eq!((get(Count::ReadBytes) - r0, own_held()), (0, 0));
    }

    /// With the links not told the whole is the NAS's, counted as it ended; opened once.
    #[test]
    fn a_program_with_no_links_told_counts_its_bytes_served() {
        let r0 = get(Count::ReadBytes);
        ProgramReads::begin_with(700, CREDIT_EVERY, std::sync::Arc::new(|| None))
            .ended(&[(Path::new("/Volumes/x/a"), 700)], 1);
        assert_eq!((get(Count::ReadBytes) - r0, own_held(), get(Count::Files)), (700, 0, 1));
    }

    /// A reader counted as it reads: a file read through shows its bytes, whichever answered, and
    /// gives its bytes unchanged.
    #[test]
    fn a_counting_reader_counts_the_bytes_it_gives() {
        let asked = || get(Count::ReadBytes) + own_held();
        let data: Vec<u8> = (0..(2 << 20) + 17).map(|i| (i % 251) as u8).collect();
        let a = asked();
        let mut got = Vec::new();
        std::io::copy(&mut Counting::new(&data[..]), &mut got).unwrap();
        assert_eq!(got, data);
        assert_eq!(asked() - a, data.len() as u64);
    }
}
