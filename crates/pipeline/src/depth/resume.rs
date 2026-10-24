//! A depth target's run resumed where it stopped (docs/pool.md §7.2, Resuming; task #643): a z6
//! tile's (z9–16) or an assembly's (z6–8) composing goes block by block (crate::depth::build's
//! `level`), up to two hours a run; a run stopped (sleep, an update, the memory guard, a pause)
//! starts again at the block after the last it finished, not at z9.
//!
//! - **On this Mac's disk, never the NAS:** one file per target at the top of the job's scratch
//!   folder (`<scratch>/<z>_<x>_<y>.resume`, `path`): a line naming the run's identity (`id`), then
//!   a log appended once a block (its block and the tiles it made, each record with its length and
//!   BLAKE3, so a record cut short or damaged is dropped with what follows it). At the top, the
//!   agent's sampler counts it in the job's disk (crate::agent::memguard's `top_bytes`), so the
//!   step's measured disk, and the room asked for it, hold it: at most the target's tiles once
//!   more, as its packs hold them.
//! - **Taken only when it's what a fresh run would make:** the identity (`id`) is a digest of
//!   everything the run reads: the depth's versions and encodings, the target, each source's pin,
//!   the water's pin and the depth lo packs it may read (by content name: the z3 tile over the
//!   target's and its neighbours'). Another identity: the log is emptied and the run starts afresh.
//!   A block's tiles depend only on those and on the blocks before it (`Tiles::up` reads coarser
//!   tiles made earlier), which the log restores: the outputs are the same bytes whether a run
//!   resumed or not. The floor's shading (`depth-shade`) isn't logged: it's made from the tiles
//!   once all are (crate::depth::build's `shading`), the same bytes again.
//! - **Where it's found:** the job's scratch folder, or the same step's folder of another of the
//!   agent's slots (`scratch-2/`, …), moved here.
//! - **Gone:** once the target's packs are saved (crate::depth::build's `build_hi`, `build_lo`);
//!   when its job is cancelled (the agent clears the target's scratch, `<z>_<x>_<y>.*`); with the
//!   slot's step folder when room-making deletes a given-back job's; and, a target that never comes
//!   back here (made on another Mac), as the step's next run here opens its own log: any log in
//!   the step's folders untouched for crate::agent::resume::HOLD_S (`sweep`), when the pool no
//!   longer offers it back to this Mac either.
//! - **Only with resuming on:** the job's agent says so (crate::agent::resume::RESUME_ENV, with
//!   `state/pool/resume` on); a run without it keeps nothing.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A block: (z, its first tile's column, row).
pub type BlockId = (u8, u32, u32);
/// A tile made: its depth's and class's encodings.
pub type Made = ((u8, u32, u32), (Vec<u8>, Vec<u8>));

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

/// A log's first line: this, its run's identity, a newline.
const HEAD: &str = "depth-resume 1 ";

/// A run's identity: a digest of `parts`.
pub fn id(parts: &[&str]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize().to_hex().to_string()
}

#[cfg(test)]
thread_local! {
    /// Tests: the run stops (an error) once this many blocks more are kept.
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
    /// Target `t`'s log in `scratch` (another slot's moved here), for a run of identity `id`: its
    /// blocks and their tiles (into `made`), or none when it has another identity or none. None
    /// when it can't be written (nothing kept then). Logs left long by other targets swept first.
    pub fn open(
        scratch: &Path,
        t: (u8, u32, u32),
        id: &str,
        made: &mut BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>,
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
        let head = format!("{HEAD}{id}\n");
        let r = (|| -> Result<Log> {
            std::fs::create_dir_all(scratch)?;
            let b = std::fs::read(&p).unwrap_or_default();
            let (done, good) = match b.strip_prefix(head.as_bytes()) {
                Some(rest) => {
                    let (done, n) = read(rest, made);
                    (done, (head.len() + n) as u64)
                }
                None => (BTreeSet::new(), 0),
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
                    eprintln!("resume: {}/{}/{}: {} blocks kept by an earlier run", t.0, t.1, t.2, l.done.len());
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

    /// Block `b` finished, its tiles `tiles`: appended. A failure to keep it is said and the run
    /// goes on keeping nothing more.
    pub fn keep(&mut self, b: BlockId, tiles: &[(&(u8, u32, u32), &(Vec<u8>, Vec<u8>))]) -> Result<()> {
        if let Some(f) = self.file.as_mut() {
            if let Err(e) = f.write_all(&record(b, tiles)) {
                eprintln!("resume: {} not kept further: {e}", self.path.display());
                self.file = None;
            }
        }
        self.done.insert(b);
        #[cfg(test)]
        if let Some(n) = STOP_AFTER.get() {
            if n <= 1 {
                STOP_AFTER.set(None);
                anyhow::bail!("stopped (a test's interruption) after block {b:?}");
            }
            STOP_AFTER.set(Some(n - 1));
        }
        Ok(())
    }
}

/// A record: its payload's length, its BLAKE3, the payload (the block, its tiles' count, each
/// tile's z, x, y and its two encodings, each with its length).
fn record(b: BlockId, tiles: &[(&(u8, u32, u32), &(Vec<u8>, Vec<u8>))]) -> Vec<u8> {
    let mut p = Vec::new();
    p.push(b.0);
    p.extend(b.1.to_le_bytes());
    p.extend(b.2.to_le_bytes());
    p.extend((tiles.len() as u64).to_le_bytes());
    for (&(z, x, y), (d, c)) in tiles {
        p.push(z);
        p.extend(x.to_le_bytes());
        p.extend(y.to_le_bytes());
        for v in [d, c] {
            p.extend((v.len() as u64).to_le_bytes());
            p.extend(v.iter());
        }
    }
    let mut r = Vec::with_capacity(40 + p.len());
    r.extend((p.len() as u64).to_le_bytes());
    r.extend(blake3::hash(&p).as_bytes());
    r.extend(p);
    r
}

/// A log's records `b` (after its first line): its blocks, their tiles laid into `made`, and the
/// length of its whole records.
fn read(b: &[u8], made: &mut BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>) -> (BTreeSet<BlockId>, usize) {
    let mut done = BTreeSet::new();
    let mut at = 0usize;
    while let Some(len) = b.get(at..at + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())) {
        let Ok(len) = usize::try_from(len) else { break };
        let Some(sum) = b.get(at + 8..at + 40) else { break };
        let Some(payload) = b.get(at + 40..(at + 40).saturating_add(len)) else { break };
        if blake3::hash(payload).as_bytes() != sum {
            break;
        }
        let Ok((blk, tiles)) = parse(payload) else { break };
        made.extend(tiles);
        done.insert(blk);
        at += 40 + len;
    }
    (done, at)
}

fn parse(p: &[u8]) -> Result<(BlockId, Vec<Made>)> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let s = p.get(at..at.checked_add(n).context("cut short")?).context("cut short")?;
        at += n;
        Ok(s)
    };
    let u32_ = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap());
    let blk = (take(1)?[0], u32_(take(4)?), u32_(take(4)?));
    let n = u64::from_le_bytes(take(8)?.try_into()?);
    let mut tiles = Vec::new();
    for _ in 0..n {
        let k = (take(1)?[0], u32_(take(4)?), u32_(take(4)?));
        let mut v = [Vec::new(), Vec::new()];
        for e in &mut v {
            let len = usize::try_from(u64::from_le_bytes(take(8)?.try_into()?))?;
            *e = take(len)?.to_vec();
        }
        let [d, c] = v;
        tiles.push((k, (d, c)));
    }
    Ok((blk, tiles))
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

#[cfg(test)]
mod tests {
    use super::*;

    type Tile = ((u8, u32, u32), (Vec<u8>, Vec<u8>));

    fn tiles() -> (Tile, Tile) {
        (((9, 152, 184), (vec![1, 2], vec![3])), ((9, 153, 184), (vec![4], Vec::new())))
    }

    /// A log of three blocks (the first and third with a tile each) for target `t` in `scratch`.
    fn three(scratch: &Path, t: (u8, u32, u32), id: &str) {
        let ((k1, v1), (k2, v2)) = tiles();
        let mut l = Log::open(scratch, t, id, &mut BTreeMap::new()).unwrap();
        l.keep((9, 152, 184), &[(&k1, &v1)]).unwrap();
        l.keep((9, 160, 184), &[]).unwrap();
        l.keep((10, 304, 368), &[(&k2, &v2)]).unwrap();
    }

    #[test]
    fn a_log_keeps_whole_records_and_drops_a_tail_cut_short() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/depth");
        let t = (6, 19, 23);
        let ((k1, v1), (k2, v2)) = tiles();
        three(&scratch, t, "a");
        let mut made = BTreeMap::new();
        assert_eq!(Log::open(&scratch, t, "a", &mut made).unwrap().done.len(), 3);
        assert_eq!(made, [(k1, v1.clone()), (k2, v2)].into());
        // (The last record cut short, as a stop mid-write.)
        let p = path(&scratch, t);
        let n = std::fs::metadata(&p).unwrap().len();
        std::fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(n - 3).unwrap();
        let mut made = BTreeMap::new();
        let mut l = Log::open(&scratch, t, "a", &mut made).unwrap();
        assert_eq!(l.done, [(9, 152, 184), (9, 160, 184)].into());
        assert_eq!(made, [(k1, v1.clone())].into());
        // Appended after the cut: whole again.
        l.keep((9, 168, 184), &[]).unwrap();
        drop(l);
        assert_eq!(Log::open(&scratch, t, "a", &mut BTreeMap::new()).unwrap().done.len(), 3);
        // Another identity: afresh, the log emptied.
        let mut made = BTreeMap::new();
        assert!(Log::open(&scratch, t, "b", &mut made).unwrap().done.is_empty() && made.is_empty());
        assert!(Log::open(&scratch, t, "a", &mut made).unwrap().done.is_empty());
    }

    #[test]
    fn a_damaged_log_keeps_the_records_before_the_damage() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/depth");
        let t = (6, 19, 23);
        let ((k1, v1), _) = tiles();
        three(&scratch, t, "a");
        let p = path(&scratch, t);
        let b = std::fs::read(&p).unwrap();
        let first = HEAD.len() + "a\n".len() + record((9, 152, 184), &[(&k1, &v1)]).len();
        // A byte of the second record's payload flipped: the first kept, the rest dropped.
        let mut bad = b.clone();
        bad[first + 45] ^= 1;
        std::fs::write(&p, &bad).unwrap();
        let mut made = BTreeMap::new();
        assert_eq!(Log::open(&scratch, t, "a", &mut made).unwrap().done, [(9, 152, 184)].into());
        assert_eq!(made, [(k1, v1)].into());
        assert_eq!(std::fs::metadata(&p).unwrap().len() as usize, first, "cut after the last whole record");
        // A length past the file's end, a first line damaged, an empty file: nothing taken.
        for bad in [
            [&b[..first], &u64::MAX.to_le_bytes()[..], &[0u8; 64][..]].concat(),
            [b"depth-resume 1 x\n".as_slice(), &b[HEAD.len() + 2..]].concat(),
            Vec::new(),
        ] {
            std::fs::write(&p, &bad).unwrap();
            let mut made = BTreeMap::new();
            let l = Log::open(&scratch, t, "a", &mut made).unwrap();
            assert!(l.done.len() <= 1 && made.len() == l.done.len());
        }
    }

    #[test]
    fn a_log_moves_from_another_slot_and_logs_left_long_are_swept() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let (mine, other) = (home.join("scratch/depth"), home.join("scratch-2/depth"));
        // Another slot's log of the target, moved here (this slot's step folder not yet made).
        three(&other, (6, 1, 1), "c");
        let mut made = BTreeMap::new();
        assert_eq!(Log::open(&mine, (6, 1, 1), "c", &mut made).unwrap().done.len(), 3);
        assert!(!path(&other, (6, 1, 1)).exists() && path(&mine, (6, 1, 1)).exists());
        // Two slots at once, each its own target: neither touches the other's.
        let mut a = Log::open(&mine, (6, 2, 2), "c", &mut BTreeMap::new()).unwrap();
        let mut b = Log::open(&other, (6, 3, 3), "c", &mut BTreeMap::new()).unwrap();
        a.keep((9, 16, 16), &[]).unwrap();
        b.keep((9, 24, 24), &[]).unwrap();
        drop((a, b));
        assert_eq!(Log::open(&other, (6, 3, 3), "c", &mut BTreeMap::new()).unwrap().done.len(), 1);
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
}
