//! A depth target's run resumed where it stopped (docs/pool.md §7.2, Resuming; task #643): a z6
//! tile's (z9–16) or an assembly's (z6–8) composing goes block by block (crate::depth::build's
//! `level`), up to two hours a run; a run stopped (sleep, an update, the memory guard, a pause)
//! starts again at the block after the last it finished, not at z9.
//!
//! - **On this Mac's disk, never the NAS:** one folder per target in the job's scratch
//!   (`<scratch>/<z>_<x>_<y>.resume/`, `dir`): `id` (the run's identity) and `blocks`, a log
//!   appended once a block (its block and the tiles it made, each record with its length and
//!   BLAKE3, so a record cut short by a stop is dropped with what follows it).
//! - **Taken only when it's what a fresh run would make:** the identity (`id`) is a digest of
//!   everything the run reads: the depth's versions, the target, each source's pin, the water's
//!   pin and the lo packs it reads (by content name). Another identity: the folder is emptied and
//!   the run starts afresh. A block's tiles depend only on those and on the blocks before it
//!   (`Tiles::up` reads coarser tiles made earlier), which the log restores in order: the
//!   outputs are the same bytes whether a run resumed or not.
//! - **Where it's found:** the job's scratch folder, or the same step's folder of another of the
//!   agent's slots (`scratch-2/`, …), moved here.
//! - **Gone:** once the target's packs are saved (crate::depth::build's `build_hi`, `build_lo`), when its job is cancelled (the agent
//!   clears the target's scratch, `<z>_<x>_<y>.*`), and with the slot's scratch when room-making
//!   deletes a given-back job's.
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

/// Target `t` (z, x, y)'s folder in scratch folder `scratch`.
pub fn dir(scratch: &Path, t: (u8, u32, u32)) -> PathBuf {
    scratch.join(format!("{}_{}_{}.resume", t.0, t.1, t.2))
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

#[cfg(test)]
thread_local! {
    /// Tests: the run stops (an error) once this many blocks more are kept.
    pub static STOP_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// A target's blocks kept so far, and its log open to append.
pub struct Log {
    dir: PathBuf,
    file: Option<std::fs::File>,
    /// The blocks the log holds.
    pub done: BTreeSet<BlockId>,
}

impl Log {
    /// Target `t`'s log in `scratch` (another slot's moved here), for a run of identity `id`: its
    /// blocks and their tiles (into `made`, in the order made), or none when it has another
    /// identity or none. None when its folder can't be made (nothing kept then).
    pub fn open(
        scratch: &Path,
        t: (u8, u32, u32),
        id: &str,
        made: &mut BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>,
    ) -> Option<Log> {
        let d = dir(scratch, t);
        if !d.exists() {
            if let Some(other) = elsewhere(scratch, t) {
                if let Err(e) = std::fs::rename(&other, &d) {
                    eprintln!("resume: {} not moved here: {e}", other.display());
                }
            }
        }
        let same = std::fs::read_to_string(d.join("id")).is_ok_and(|s| s == id);
        let r = (|| -> Result<Log> {
            if !same {
                std::fs::remove_dir_all(&d).ok();
                std::fs::create_dir_all(&d)?;
                crate::whole::write(&d.join("id"), id.as_bytes())?;
            }
            let p = d.join("blocks");
            let (done, good) = if same { read(&p, made) } else { (BTreeSet::new(), 0) };
            // (Whatever follows the last whole record, cut.)
            let f = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&p)?;
            f.set_len(good)?;
            let mut f = f;
            std::io::Seek::seek(&mut f, std::io::SeekFrom::End(0))?;
            Ok(Log { dir: d.clone(), file: Some(f), done })
        })();
        match r {
            Ok(l) => {
                if !l.done.is_empty() {
                    eprintln!("resume: {}/{}/{}: {} blocks kept by an earlier run", t.0, t.1, t.2, l.done.len());
                }
                Some(l)
            }
            Err(e) => {
                eprintln!("resume: {}: {e:#}; nothing kept", d.display());
                None
            }
        }
    }

    /// Block `b` finished, its tiles `tiles`: appended. A failure to keep it is said and the run
    /// goes on keeping nothing more.
    pub fn keep(&mut self, b: BlockId, tiles: &[(&(u8, u32, u32), &(Vec<u8>, Vec<u8>))]) -> Result<()> {
        if let Some(f) = self.file.as_mut() {
            if let Err(e) = f.write_all(&record(b, tiles)) {
                eprintln!("resume: {} not kept further: {e}", self.dir.display());
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

/// The log at `p`: its blocks, their tiles laid into `made`, and the length of its whole records.
fn read(p: &Path, made: &mut BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>) -> (BTreeSet<BlockId>, u64) {
    let mut done = BTreeSet::new();
    let Ok(b) = std::fs::read(p) else { return (done, 0) };
    let mut at = 0usize;
    while let Some(len) = b.get(at..at + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()) as usize) {
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
    (done, at as u64)
}

fn parse(p: &[u8]) -> Result<(BlockId, Vec<Made>)> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let s = p.get(at..at + n).context("cut short")?;
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
            let len = u64::from_le_bytes(take(8)?.try_into()?) as usize;
            *e = take(len)?.to_vec();
        }
        let [d, c] = v;
        tiles.push((k, (d, c)));
    }
    Ok((blk, tiles))
}

/// Target `t`'s folder in the same step's folder of another of this agent's slots (scratch folder
/// `<home>/scratch[-n]/<step>`), if one has it.
fn elsewhere(scratch: &Path, t: (u8, u32, u32)) -> Option<PathBuf> {
    let step = scratch.file_name()?;
    let slot = scratch.parent()?;
    let home = slot.parent()?;
    if !slot.file_name()?.to_string_lossy().starts_with("scratch") {
        return None;
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(home)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("scratch") && e.path() != slot)
        .map(|e| dir(&e.path().join(step), t))
        .filter(|d| d.join("id").exists())
        .collect();
    found.sort();
    found.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_keeps_whole_records_and_drops_a_tail_cut_short() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/depth");
        let t = (6, 19, 23);
        let mut made = BTreeMap::new();
        let mut l = Log::open(&scratch, t, "a", &mut made).unwrap();
        let (k1, v1) = ((9u8, 152u32, 184u32), (vec![1u8, 2], vec![3u8]));
        let (k2, v2) = ((9u8, 153u32, 184u32), (vec![4u8], Vec::new()));
        l.keep((9, 152, 184), &[(&k1, &v1)]).unwrap();
        l.keep((9, 160, 184), &[]).unwrap();
        l.keep((10, 304, 368), &[(&k2, &v2)]).unwrap();
        drop(l);
        // (The last record cut short, as a stop mid-write.)
        let p = dir(&scratch, t).join("blocks");
        let n = std::fs::metadata(&p).unwrap().len();
        std::fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(n - 3).unwrap();
        let mut made = BTreeMap::new();
        let l = Log::open(&scratch, t, "a", &mut made).unwrap();
        assert_eq!(l.done, [(9, 152, 184), (9, 160, 184)].into());
        assert_eq!(made, [(k1, v1.clone())].into());
        drop(l);
        // Another identity: afresh.
        let mut made = BTreeMap::new();
        assert!(Log::open(&scratch, t, "b", &mut made).unwrap().done.is_empty() && made.is_empty());
        // Another slot's, moved here.
        let other = d.path().join("home/scratch-2/depth");
        let mut l = Log::open(&other, (6, 1, 1), "c", &mut made).unwrap();
        l.keep((9, 8, 8), &[(&k1, &v1)]).unwrap();
        drop(l);
        let mut made = BTreeMap::new();
        let l = Log::open(&scratch, (6, 1, 1), "c", &mut made).unwrap();
        assert_eq!((l.done.len(), made.len()), (1, 1));
        assert!(!dir(&other, (6, 1, 1)).exists());
    }
}
