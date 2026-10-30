//! A long target's run resumed where it stopped (docs/pool.md §7.2, Resuming): its work goes
//! block by block, and a run stopped (sleep, an update, the NAS restarting, the memory guard, a
//! pause) starts again at the block after the last it finished. The steps keeping one: depth and
//! depth-lo (crate::depth::resume), terrain-hi (crate::terrain_pack, a level's blocks), and
//! terrain-deep (crate::terrain_deep, its base tiles).
//!
//! - **On this Mac's disk, never the NAS:** one file per target at the top of the job's scratch
//!   folder (`<scratch>/<z>_<x>_<y>.resume`, `path`; each step's scratch folder is its own): a line
//!   naming the step's kind and the run's identity (`id`), then a log appended once a block (its
//!   block and what the step keeps of it, each record with its length and BLAKE3, so a record cut
//!   short or damaged is dropped with what follows it). At the top, the agent's sampler counts it in
//!   the job's disk (crate::agent::memguard's `top_bytes`), so the step's measured disk, and the room
//!   asked for it, hold it.
//! - **Read a record at a time:** a log is streamed as it opens (`Log::open`'s `take`), never read
//!   whole, so what a resumed run holds is what its step keeps of the blocks, as the run it resumes
//!   held them.
//! - **Taken only for the same inputs:** the identity is a digest of everything the run reads, its
//!   step's choosing; another identity, the log is emptied and the run starts afresh.
//! - **Where it's found:** the job's scratch folder, or the same step's folder of another of the
//!   agent's slots (`scratch-2/`, …), moved here.
//! - **Gone:** once its step's packs are written (each step deletes its own); when its job is
//!   cancelled (the agent clears the target's scratch, `<z>_<x>_<y>.*`); with the slot's step folder
//!   when room-making deletes a given-back job's; and, a target that never comes back here (made on
//!   another Mac), as the step's next run here opens its own log: any log in the step's folders
//!   untouched for crate::agent::resume::HOLD_S (`sweep`).
//! - **Only with resuming on:** the job's agent says so (crate::agent::resume::RESUME_ENV, with
//!   `state/pool/resume` on); a run without it keeps nothing.

use anyhow::Result;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A block: (z, and two numbers its step names it by: its first tile's column and row, …).
pub type BlockId = (u8, u32, u32);

/// Whether this job keeps its blocks: its agent turned resuming on.
pub fn on() -> bool {
    std::env::var_os(crate::agent::resume::RESUME_ENV).is_some()
}

/// Target `t` (z, x, y)'s log in scratch folder `scratch`.
pub fn path(scratch: &Path, t: (u8, u32, u32)) -> PathBuf {
    scratch.join(format!("{}_{}_{}{EXT}", t.0, t.1, t.2))
}

/// A log's name's ending.
const EXT: &str = ".resume";

/// A log's first line: `<kind>-resume 1 <id>` and a newline.
fn head(kind: &str, id: &str) -> String {
    format!("{kind}-resume 1 {id}\n")
}

/// A run's identity: a digest of `parts`.
pub fn id(parts: &[&str]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// A `Pieces` run's identity (task #889): `id` of the program running it (its executable's digest,
/// so a run of other code never takes pieces another made), its step's kind, its target, and
/// `parts` (what it reads, its step's choosing).
pub fn job_id(kind: &str, target: &str, parts: &[&str]) -> String {
    static EXE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let exe = EXE.get_or_init(|| {
        std::env::current_exe()
            .and_then(std::fs::read)
            .map_or_else(|_| "-".into(), |b| blake3::hash(&b).to_hex().to_string())
    });
    let mut v = vec![exe.as_str(), kind, target];
    v.extend_from_slice(parts);
    id(&v)
}

#[cfg(test)]
thread_local! {
    /// Tests: the run stops as a pause stops it (crate::control::Paused) once this many blocks more
    /// are kept.
    pub static STOP_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// A target's blocks kept so far, and its log open to append.
pub struct Log {
    path: PathBuf,
    file: Option<std::fs::File>,
    /// The blocks the log holds.
    pub done: BTreeSet<BlockId>,
}

impl Log {
    /// Target `t`'s log in `scratch` (another slot's moved here) for a run of step kind `kind` and
    /// identity `id`: each block it holds given to `take` in the order kept (with what was kept of
    /// it), none when it has another identity or none. A record `take` refuses ends the log there,
    /// as a damaged one. None when it can't be written (nothing kept then). Logs left long by other
    /// targets swept first.
    pub fn open(
        scratch: &Path,
        t: (u8, u32, u32),
        kind: &str,
        id: &str,
        take: &mut dyn FnMut(BlockId, &[u8]) -> Result<()>,
    ) -> Option<Log> {
        let p = path(scratch, t);
        sweep(scratch, &p, crate::agent::resume::HOLD_S);
        if !p.exists() {
            if let Some(other) = elsewhere(scratch, t) {
                if let Err(e) = std::fs::create_dir_all(scratch).and_then(|()| std::fs::rename(&other, &p)) {
                    eprintln!("resume: {} not moved here: {e}", other.display());
                }
            }
        }
        let head = head(kind, id);
        let r = (|| -> Result<Log> {
            std::fs::create_dir_all(scratch)?;
            let (done, good) = match std::fs::File::open(&p) {
                Ok(f) => {
                    let mut r = std::io::BufReader::with_capacity(1 << 20, f);
                    let mut first = vec![0u8; head.len()];
                    if r.read_exact(&mut first).is_ok() && first == head.as_bytes() {
                        let (done, n) = read(&mut r, take);
                        (done, (head.len() as u64) + n)
                    } else {
                        (BTreeSet::new(), 0)
                    }
                }
                Err(_) => (BTreeSet::new(), 0),
            };
            let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&p)?;
            // (Whatever follows the last whole record cut; another identity's, all of it.)
            f.set_len(good)?;
            std::io::Seek::seek(&mut f, std::io::SeekFrom::End(0))?;
            if good == 0 {
                f.write_all(head.as_bytes())?;
            }
            Ok(Log { path: p.clone(), file: Some(f), done })
        })();
        match r {
            Ok(l) => {
                if !l.done.is_empty() {
                    eprintln!("resume: {kind} {}/{}/{}: {} blocks kept by an earlier run", t.0, t.1, t.2, l.done.len());
                }
                Some(l)
            }
            Err(e) => {
                eprintln!("resume: {}: {e:#}; nothing kept", p.display());
                std::fs::remove_file(&p).ok();
                None
            }
        }
    }

    /// Block `b` finished, what its step keeps of it `payload`: appended. A failure to keep it is
    /// said and the run goes on keeping nothing more. Once it's kept, a safe point (task #885,
    /// crate::control::piece_saved): Err(crate::control::Paused) with the build pausing, the run
    /// resumed from here; a block not kept (the log failed) is no safe point, as its work would be
    /// lost.
    pub fn keep(&mut self, b: BlockId, payload: &[u8]) -> Result<()> {
        if let Some(f) = self.file.as_mut() {
            if let Err(e) = f.write_all(&record(b, payload)) {
                eprintln!("resume: {} not kept further: {e}", self.path.display());
                self.file = None;
            }
        }
        self.done.insert(b);
        #[cfg(test)]
        if let Some(n) = STOP_AFTER.get() {
            if n <= 1 {
                STOP_AFTER.set(None);
                // (As a pause stops it: the same error, at the same point.)
                return Err(anyhow::Error::new(crate::control::Paused)
                    .context(format!("stopped (a test's pause) after block {b:?}")));
            }
            STOP_AFTER.set(Some(n - 1));
        }
        if self.file.is_some() {
            crate::control::piece_saved()?;
        }
        Ok(())
    }
}

/// A target's work as pieces, each piece's result kept as it's made (task #889): the shared path of
/// the steps whose target is a run of independent pieces (a chunk of tiles sampled, a z6 tile's
/// part, a batch of a pack's tiles), each a pure function of the run's inputs (its identity) and
/// its own id. `piece` hands back a piece kept by an earlier run of the same identity, else makes
/// it and keeps it: a safe point once kept (`Log::keep`). A resumed run makes only the pieces its
/// last didn't keep, and as each is the same bytes, so is the target's output. Without a log
/// (resuming off, or the log unwritable) it just makes each.
pub struct Pieces {
    log: Option<Log>,
    kept: std::collections::HashMap<BlockId, Vec<u8>>,
}

impl Pieces {
    /// Target `t`'s pieces in `scratch` for a run of step kind `kind` and identity `id` (`id()`),
    /// kept only with `keep` (the job's agent turned resuming on, `on()`; tests say so).
    pub fn open(scratch: &Path, t: (u8, u32, u32), kind: &str, id: &str, keep: bool) -> Pieces {
        let mut kept = std::collections::HashMap::new();
        let log = if keep {
            Log::open(scratch, t, kind, id, &mut |b, p| {
                kept.insert(b, p.to_vec());
                Ok(())
            })
        } else {
            None
        };
        Pieces { log, kept }
    }

    /// Pieces never kept (each just made): a run by hand, a test's.
    pub fn none() -> Pieces {
        Pieces { log: None, kept: Default::default() }
    }

    /// Pieces of floats (`piece`, the floats by their bits).
    pub fn floats(&mut self, b: BlockId, make: impl FnOnce() -> Result<Vec<Vec<f32>>>) -> Result<Vec<Vec<f32>>> {
        let p = self.piece(b, || {
            let mut p = Put::default();
            let v = make()?;
            p.u64(v.len() as u64);
            for e in &v {
                p.f32s(e);
            }
            Ok(p.0)
        })?;
        let mut g = Get(&p);
        let v = (0..g.count(8)?).map(|_| g.f32s()).collect::<Result<_>>()?;
        g.end()?;
        Ok(v)
    }

    /// What an earlier run kept of piece `b`, if it did (not taken).
    pub fn kept(&self, b: BlockId) -> Option<&[u8]> {
        self.kept.get(&b).map(Vec::as_slice)
    }

    /// Piece `b`'s result: kept by an earlier run, else `make`'s, kept (a safe point:
    /// Err(crate::control::Paused) with the build pausing, once it's kept).
    pub fn piece(&mut self, b: BlockId, make: impl FnOnce() -> Result<Vec<u8>>) -> Result<Vec<u8>> {
        if let Some(p) = self.kept.remove(&b) {
            return Ok(p);
        }
        let p = make()?;
        // (Not kept, no safe point: a stop here would lose the target's pieces.)
        if let Some(l) = self.log.as_mut() {
            l.keep(b, &p)?;
        }
        Ok(p)
    }

    /// The target's output saved: its log deleted.
    pub fn finish(self) {
        if let Some(l) = self.log {
            std::fs::remove_file(&l.path).ok();
        }
    }
}

/// A record: its payload's length, its BLAKE3, the payload (the block's z, x and y, then what its
/// step keeps).
fn record(b: BlockId, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(9 + payload.len());
    p.push(b.0);
    p.extend(b.1.to_le_bytes());
    p.extend(b.2.to_le_bytes());
    p.extend_from_slice(payload);
    let mut r = Vec::with_capacity(40 + p.len());
    r.extend((p.len() as u64).to_le_bytes());
    r.extend(blake3::hash(&p).as_bytes());
    r.extend(p);
    r
}

/// The most a record may hold: a length past it is damage, not a record (a block's tiles are tens
/// of MB).
const MOST: u64 = 4 << 30;

/// A log's records (after its first line), read from `r` one at a time, each whole one given to
/// `take`: their blocks and the length of the whole records `take` took.
fn read(r: &mut impl Read, take: &mut dyn FnMut(BlockId, &[u8]) -> Result<()>) -> (BTreeSet<BlockId>, u64) {
    let mut done = BTreeSet::new();
    let mut at = 0u64;
    loop {
        let mut top = [0u8; 40];
        if r.read_exact(&mut top).is_err() {
            break;
        }
        let len = u64::from_le_bytes(top[..8].try_into().unwrap());
        if !(9..=MOST).contains(&len) {
            break;
        }
        let mut p = Vec::new();
        if r.take(len).read_to_end(&mut p).is_err() || p.len() as u64 != len {
            break;
        }
        if blake3::hash(&p).as_bytes() != &top[8..] {
            break;
        }
        let b =
            (p[0], u32::from_le_bytes(p[1..5].try_into().unwrap()), u32::from_le_bytes(p[5..9].try_into().unwrap()));
        if let Err(e) = take(b, &p[9..]) {
            eprintln!("resume: block {b:?} not taken: {e:#}; the log ends there");
            break;
        }
        done.insert(b);
        at += 40 + len;
    }
    (done, at)
}

/// The same step's folders of this agent's other slots (scratch folder `scratch` is
/// `<home>/scratch[-n]/<step>`), and `scratch`'s own first; just its own when it isn't one.
fn step_folders(scratch: &Path) -> Vec<PathBuf> {
    let mut v = vec![scratch.to_path_buf()];
    let (Some(step), Some(slot)) = (scratch.file_name(), scratch.parent()) else { return v };
    let Some(home) = slot.parent() else { return v };
    if !slot.file_name().is_some_and(|n| n.to_string_lossy().starts_with("scratch")) {
        return v;
    }
    let mut others: Vec<PathBuf> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("scratch") && e.path() != slot)
        .map(|e| e.path().join(step))
        .collect();
    others.sort();
    v.extend(others);
    v
}

/// Target `t`'s log in the same step's folder of another of this agent's slots, if one has it.
fn elsewhere(scratch: &Path, t: (u8, u32, u32)) -> Option<PathBuf> {
    step_folders(scratch).into_iter().skip(1).map(|d| path(&d, t)).find(|p| p.is_file())
}

/// The logs in the step's folders (`step_folders`) but `keep` not written for `age_s`, deleted: a
/// target made since on another Mac never comes back to them (a log being written is touched
/// each block, far more often).
fn sweep(scratch: &Path, keep: &Path, age_s: u64) {
    let now = std::time::SystemTime::now();
    for d in step_folders(scratch) {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p == keep || !e.file_name().to_string_lossy().ends_with(EXT) {
                continue;
            }
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|a| a.as_secs() >= age_s);
            if old && e.file_type().is_ok_and(|t| t.is_file()) && std::fs::remove_file(&p).is_ok() {
                eprintln!("resume: {} left {age_s} s, deleted", p.display());
            }
        }
    }
}

/// A payload written field by field, little-endian (what the steps keep of a block).
#[derive(Default)]
pub struct Put(pub Vec<u8>);

impl Put {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend(v.to_le_bytes());
    }
    /// Bytes, with their length.
    pub fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.0.extend_from_slice(v);
    }
    /// Floats by their bits, with their count.
    pub fn f32s(&mut self, v: &[f32]) {
        self.u64(v.len() as u64);
        for x in v {
            self.0.extend(x.to_bits().to_le_bytes());
        }
    }
}

/// A payload read back field by field (`Put`'s); an error where it's cut short.
pub struct Get<'a>(pub &'a [u8]);

impl<'a> Get<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        anyhow::ensure!(self.0.len() >= n, "cut short");
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    /// A count of items each at least `each` bytes: an error past what's left.
    pub fn count(&mut self, each: usize) -> Result<usize> {
        let n = usize::try_from(self.u64()?)?;
        anyhow::ensure!(n.saturating_mul(each.max(1)) <= self.0.len(), "a count past the end");
        Ok(n)
    }
    pub fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.count(1)?;
        Ok(self.take(n)?.to_vec())
    }
    pub fn f32s(&mut self) -> Result<Vec<f32>> {
        let n = self.count(4)?;
        Ok(self
            .take(n * 4)?
            .chunks_exact(4)
            .map(|c| f32::from_bits(u32::from_le_bytes(c.try_into().unwrap())))
            .collect())
    }
    /// Whether all was read.
    pub fn end(&self) -> Result<()> {
        anyhow::ensure!(self.0.is_empty(), "{} bytes past the end", self.0.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A log's blocks and payloads as opened.
    fn opened(scratch: &Path, t: (u8, u32, u32), id: &str) -> (Log, BTreeMap<BlockId, Vec<u8>>) {
        let mut m = BTreeMap::new();
        let l = Log::open(scratch, t, "test", id, &mut |b, p| {
            m.insert(b, p.to_vec());
            Ok(())
        })
        .unwrap();
        (l, m)
    }

    /// A log of three blocks for target `t` in `scratch`.
    fn three(scratch: &Path, t: (u8, u32, u32), id: &str) {
        let mut l = opened(scratch, t, id).0;
        l.keep((9, 152, 184), &[1, 2, 3]).unwrap();
        l.keep((9, 160, 184), &[]).unwrap();
        l.keep((10, 304, 368), &[4]).unwrap();
    }

    #[test]
    fn a_log_keeps_whole_records_and_drops_a_tail_cut_short() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/depth");
        let t = (6, 19, 23);
        three(&scratch, t, "a");
        let (l, m) = opened(&scratch, t, "a");
        assert_eq!(l.done.len(), 3);
        assert_eq!(m, [((9, 152, 184), vec![1, 2, 3]), ((9, 160, 184), vec![]), ((10, 304, 368), vec![4])].into());
        drop(l);
        // (The last record cut short, as a stop mid-write.)
        let p = path(&scratch, t);
        let n = std::fs::metadata(&p).unwrap().len();
        std::fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(n - 3).unwrap();
        let (mut l, m) = opened(&scratch, t, "a");
        assert_eq!(l.done, [(9, 152, 184), (9, 160, 184)].into());
        assert_eq!(m.len(), 2);
        // Appended after the cut: whole again.
        l.keep((9, 168, 184), &[]).unwrap();
        drop(l);
        assert_eq!(opened(&scratch, t, "a").0.done.len(), 3);
        // Another identity, or another step's kind: afresh, the log emptied.
        let (l, m) = opened(&scratch, t, "b");
        assert!(l.done.is_empty() && m.is_empty());
        drop(l);
        assert!(opened(&scratch, t, "a").0.done.is_empty());
        three(&scratch, t, "a");
        let l = Log::open(&scratch, t, "other", "a", &mut |_, _| Ok(())).unwrap();
        assert!(l.done.is_empty());
    }

    #[test]
    fn a_damaged_log_keeps_the_records_before_the_damage() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/depth");
        let t = (6, 19, 23);
        three(&scratch, t, "a");
        let p = path(&scratch, t);
        let b = std::fs::read(&p).unwrap();
        let first = head("test", "a").len() + record((9, 152, 184), &[1, 2, 3]).len();
        // A byte of the second record's payload flipped: the first kept, the rest dropped.
        let mut bad = b.clone();
        bad[first + 45] ^= 1;
        std::fs::write(&p, &bad).unwrap();
        let (l, m) = opened(&scratch, t, "a");
        assert_eq!(l.done, [(9, 152, 184)].into());
        assert_eq!(m.len(), 1);
        drop(l);
        assert_eq!(std::fs::metadata(&p).unwrap().len() as usize, first, "cut after the last whole record");
        // A record its step refuses: the log ends before it.
        std::fs::write(&p, &b).unwrap();
        let l = Log::open(&scratch, t, "test", "a", &mut |b, _| {
            anyhow::ensure!(b.0 == 9, "no");
            Ok(())
        })
        .unwrap();
        assert_eq!(l.done.len(), 2);
        drop(l);
        // A length past the file's end, or past the most, a first line damaged, an empty file:
        // nothing more taken.
        let h = head("test", "a").len();
        for bad in [
            [&b[..first], &u64::MAX.to_le_bytes()[..], &[0u8; 64][..]].concat(),
            [&b[..first], &100_000u64.to_le_bytes()[..], &[0u8; 64][..]].concat(),
            [b"test-resume 1 x\n".as_slice(), &b[h..]].concat(),
            Vec::new(),
        ] {
            std::fs::write(&p, &bad).unwrap();
            let (l, m) = opened(&scratch, t, "a");
            assert!(l.done.len() <= 1 && m.len() == l.done.len());
        }
    }

    #[test]
    fn a_log_moves_from_another_slot_and_logs_left_long_are_swept() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let (mine, other) = (home.join("scratch/depth"), home.join("scratch-2/depth"));
        // Another slot's log of the target, moved here (this slot's step folder not yet made).
        three(&other, (6, 1, 1), "c");
        assert_eq!(opened(&mine, (6, 1, 1), "c").0.done.len(), 3);
        assert!(!path(&other, (6, 1, 1)).exists() && path(&mine, (6, 1, 1)).exists());
        // Two slots at once, each its own target: neither touches the other's.
        let mut a = opened(&mine, (6, 2, 2), "c").0;
        let mut b = opened(&other, (6, 3, 3), "c").0;
        a.keep((9, 16, 16), &[]).unwrap();
        b.keep((9, 24, 24), &[]).unwrap();
        drop((a, b));
        assert_eq!(opened(&other, (6, 3, 3), "c").0.done.len(), 1);
        assert!(path(&mine, (6, 2, 2)).exists());
        // Swept: those older than the age, in every slot's folder, but the one opened.
        std::fs::write(other.join("notes.txt"), b"x").unwrap();
        sweep(&mine, &path(&mine, (6, 1, 1)), 0);
        assert!(path(&mine, (6, 1, 1)).exists());
        assert!(!path(&mine, (6, 2, 2)).exists() && !path(&other, (6, 3, 3)).exists());
        assert!(other.join("notes.txt").exists(), "only logs");
        sweep(&mine, &path(&mine, (6, 9, 9)), 3600);
        assert!(path(&mine, (6, 1, 1)).exists(), "a fresh one kept");
    }

    /// Task #889: pieces kept are handed back, not made again, to a run of the same identity; to
    /// another, made afresh; once finished, the log is gone; without keeping, each is made.
    #[test]
    fn pieces_kept_are_taken_not_made_again() {
        let d = tempfile::tempdir().unwrap();
        let t = (6, 1, 2);
        let made = std::cell::Cell::new(0);
        let make = |v: u8| {
            made.set(made.get() + 1);
            Ok(vec![v; 3])
        };
        let mut p = Pieces::open(d.path(), t, "k", "a", true);
        assert_eq!(p.piece((9, 0, 0), || make(1)).unwrap(), [1; 3]);
        assert_eq!(p.floats((9, 0, 1), || Ok(vec![vec![1.5, -0.0], vec![]])).unwrap(), [vec![1.5, -0.0], vec![]]);
        drop(p);
        let mut p = Pieces::open(d.path(), t, "k", "a", true);
        assert_eq!(p.piece((9, 0, 0), || make(2)).unwrap(), [1; 3], "kept");
        assert_eq!(p.floats((9, 0, 1), || anyhow::bail!("made again")).unwrap(), [vec![1.5, -0.0], vec![]]);
        assert_eq!(p.piece((9, 1, 0), || make(3)).unwrap(), [3; 3]);
        assert_eq!(made.get(), 2);
        p.finish();
        assert!(!path(d.path(), t).exists(), "gone once finished");
        let mut p = Pieces::open(d.path(), t, "k", "b", true);
        assert_eq!(p.piece((9, 0, 0), || make(4)).unwrap(), [4; 3], "another identity: afresh");
        drop(p);
        let mut p = Pieces::none();
        assert_eq!(p.piece((9, 0, 0), || make(5)).unwrap(), [5; 3]);
        assert!(
            !job_id("k", "6/1/2", &["x"]).is_empty() && job_id("k", "6/1/2", &["x"]) != job_id("k", "6/1/2", &["y"])
        );
    }

    #[test]
    fn a_payload_reads_back_and_a_cut_one_is_an_error() {
        let mut p = Put::default();
        p.u8(7);
        p.u32(9);
        p.bytes(&[1, 2]);
        p.f32s(&[1.5, f32::NAN, -0.0]);
        let mut g = Get(&p.0);
        assert_eq!((g.u8().unwrap(), g.u32().unwrap(), g.bytes().unwrap()), (7, 9, vec![1, 2]));
        let f = g.f32s().unwrap();
        assert_eq!(f.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), [1.5f32, f32::NAN, -0.0].map(f32::to_bits));
        g.end().unwrap();
        let mut g = Get(&p.0[..p.0.len() - 1]);
        assert!(g.u8().is_ok() && g.u32().is_ok() && g.bytes().is_ok() && g.f32s().is_err());
        assert!(Get(&u64::MAX.to_le_bytes()).bytes().is_err());
    }
}
