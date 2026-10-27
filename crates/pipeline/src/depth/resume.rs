//! A depth target's run resumed where it stopped (docs/pool.md §7.2, Resuming; task #643): a z6
//! tile's (z9–16) or an assembly's (z6–8) composing goes block by block (crate::depth::build's
//! `level`), up to two hours a run; a run stopped starts again at the block after the last it
//! finished, not at z9. The log is crate::resume_log's (on this Mac's disk, at the top of the step's
//! scratch folder, moved from another slot's, swept after a day, only with resuming on); what it
//! keeps of a block is its tiles, each with its depth's and class's encodings.
//!
//! - **Taken only when it's what a fresh run would make:** the identity is a digest of everything
//!   the run composes from: the depth's versions and encodings, the target, each source's pin and
//!   the water's pin (crate::depth::build's `resume_log`). A block's tiles depend only on those and
//!   on the blocks before it (`Tiles::up` reads coarser tiles made earlier: a z6 tile's own coarse
//!   levels under it among them), which the log restores: the outputs are the same bytes whether a
//!   run resumed or not. What's made from the composed tiles once all
//!   are isn't logged either, the same bytes again: the floor's shading (crate::depth::build's
//!   `shading`) and an assembly's z3–8 from the z9 tiles (`compose_lo`).
//! - **Gone** once the target's packs are saved (crate::depth::build's `build_hi`, `build_lo`).

use crate::resume_log::{BlockId, Get, Log, Put};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// A tile made: its depth's and class's encodings.
pub type Made = ((u8, u32, u32), (Vec<u8>, Vec<u8>));

/// Target `t`'s log in `scratch` for a run of identity `id`: its blocks, their tiles laid into
/// `made` (crate::resume_log::Log::open).
pub fn open(
    scratch: &Path,
    t: (u8, u32, u32),
    id: &str,
    made: &mut BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>,
) -> Option<Log> {
    Log::open(scratch, t, "depth", id, &mut |_, p| {
        made.extend(parse(p)?);
        Ok(())
    })
}

/// Block `b` finished, its tiles `tiles`: kept.
pub fn keep(log: &mut Log, b: BlockId, tiles: &[(&(u8, u32, u32), &(Vec<u8>, Vec<u8>))]) -> Result<()> {
    log.keep(b, &payload(tiles))
}

/// What a block keeps: its tiles' count, each tile's z, x, y and its two encodings.
fn payload(tiles: &[(&(u8, u32, u32), &(Vec<u8>, Vec<u8>))]) -> Vec<u8> {
    let mut p = Put::default();
    p.u64(tiles.len() as u64);
    for (&(z, x, y), (d, c)) in tiles {
        p.u8(z);
        p.u32(x);
        p.u32(y);
        p.bytes(d);
        p.bytes(c);
    }
    p.0
}

fn parse(p: &[u8]) -> Result<Vec<Made>> {
    let mut g = Get(p);
    let n = g.count(25)?;
    let mut tiles = Vec::with_capacity(n);
    for _ in 0..n {
        let k = (g.u8()?, g.u32()?, g.u32()?);
        tiles.push((k, (g.bytes()?, g.bytes()?)));
    }
    g.end()?;
    Ok(tiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocks_tiles_read_back() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/depth");
        let t = (6, 19, 23);
        let (k1, v1) = ((9, 152, 184), (vec![1, 2], vec![3]));
        let (k2, v2) = ((9, 153, 184), (vec![4], Vec::new()));
        let mut l = open(&scratch, t, "a", &mut BTreeMap::new()).unwrap();
        keep(&mut l, (9, 152, 184), &[(&k1, &v1)]).unwrap();
        keep(&mut l, (9, 160, 184), &[]).unwrap();
        keep(&mut l, (10, 304, 368), &[(&k2, &v2)]).unwrap();
        drop(l);
        let mut made = BTreeMap::new();
        assert_eq!(open(&scratch, t, "a", &mut made).unwrap().done.len(), 3);
        assert_eq!(made, [(k1, v1), (k2, v2)].into());
    }
}
