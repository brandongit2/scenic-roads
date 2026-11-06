//! The terrain's long targets resumed where they stopped (task #650; docs/pool.md §7.2, Resuming),
//! their logs crate::resume_log's (this Mac's disk, at the top of the step's scratch folder, only
//! with resuming on):
//!
//! - **terrain-hi, a z6 tile, and terrain-deep, a z10 tile** (`HiLog`; dense ones run 30 minutes
//!   and more, up to 5,440 tiles): a level's blocks (crate::terrain_pack's `blocks`: at z12 the
//!   tiles of one z8 tile, at z11 of one z7 tile, …, up to 64 a level), each kept as it's made: its
//!   tiles (each its bytes, whether it changed, its 2×2 means for the level above); then the
//!   level's end. A resumed run takes the blocks kept and makes the rest: a tile reads only its own
//!   raw tile, its children's means and its water, so the same bytes. The identity (`hi_id`): the
//!   terrain's versions and codec, the tile, the levels' tiles this run makes (the coverage's, less
//!   the subtrees out to other workers), the accepted elevation datasets and the geoid sources (the
//!   pool and its geoids: `inputs`), the water's pin, and which of its tiles are at their place's finest zoom.
//!
//! The steps not kept, measured from the history (21–25 Oct): slope-hi and slope-deep (the slope's
//! z6 tiles took 37 s at the median and 2 minutes at the 90th percentile with their deep levels
//! in them, before #639 split those out a z10 tile a target); a stop costs them little.

use crate::resume_log::{Get, Log, Put};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// A tile made (crate::terrain_pack::process): its column and row, its bytes (None: its raw tile
/// didn't decode), whether it changed, its 2×2 means.
pub type Finished = (u32, u32, Option<Vec<u8>>, bool, Option<Vec<f32>>);

/// A level's blocks kept, each its tiles made (by the tile up their tiles are under:
/// crate::terrain_pack's `blocks`), and whether its end is.
#[derive(Default)]
pub struct LevelKept {
    pub blocks: HashMap<(u32, u32), Vec<Finished>>,
    pub ended: bool,
}

/// The block id of a level's end.
const END: u32 = u32::MAX;

/// A terrain-hi z6 tile's log, and what it held, by level (each taken by its level as it begins).
pub struct HiLog {
    log: Log,
    levels: BTreeMap<u8, LevelKept>,
}

impl HiLog {
    /// Tile `t`'s log in `scratch` (a z6 tile's terrain-hi run, a z10 tile's terrain-deep), of
    /// identity `id` (`hi_id`). The 2×2 means of a level whose next level has ended are let go as
    /// it's read: what's held is what the run it resumes held.
    pub fn open(scratch: &Path, t: (u8, u32, u32), id: &str) -> Option<HiLog> {
        let mut levels: BTreeMap<u8, LevelKept> = BTreeMap::new();
        let step = if t.0 == crate::terrain_deep::PACK_Z { "terrain-deep" } else { "terrain-hi" };
        let log = Log::open(scratch, t, step, id, &mut |b, p| {
            let lv = levels.entry(b.0).or_default();
            if b.1 == END {
                anyhow::ensure!(b.2 == END && !lv.ended && p.is_empty(), "a level's end twice");
                lv.ended = true;
                if let Some(finer) = levels.get_mut(&(b.0 + 1)) {
                    for f in finer.blocks.values_mut().flatten() {
                        f.4 = None;
                    }
                }
            } else {
                anyhow::ensure!(!lv.ended && !lv.blocks.contains_key(&(b.1, b.2)), "a block twice");
                lv.blocks.insert((b.1, b.2), parse(p)?);
            }
            Ok(())
        })?;
        Some(HiLog { log, levels })
    }

    /// Level `z`'s blocks kept, and whether it ended: taken.
    pub fn level(&mut self, z: u8) -> LevelKept {
        self.levels.remove(&z).unwrap_or_default()
    }

    /// Level `z`'s block under tile `b` (`up` levels up) made: kept.
    pub fn block(&mut self, z: u8, b: (u32, u32), done: &[Finished]) -> Result<()> {
        self.log.keep((z, b.0, b.1), &payload(done))
    }

    /// Level `z`'s end: kept (no payload).
    pub fn end(&mut self, z: u8) -> Result<()> {
        self.log.keep((z, END, END), &[])
    }
}

/// A block's record: its tiles made.
fn payload(done: &[Finished]) -> Vec<u8> {
    let mut p = Put::default();
    p.u64(done.len() as u64);
    for (x, y, b, changed, q) in done {
        p.u32(*x);
        p.u32(*y);
        p.u8(u8::from(b.is_some()) | u8::from(*changed) << 1 | u8::from(q.is_some()) << 2);
        if let Some(b) = b {
            p.bytes(b);
        }
        if let Some(q) = q {
            p.f32s(q);
        }
    }
    p.0
}

fn parse(p: &[u8]) -> Result<Vec<Finished>> {
    let mut g = Get(p);
    let mut done = Vec::new();
    for _ in 0..g.count(9)? {
        let (x, y, f) = (g.u32()?, g.u32()?, g.u8()?);
        let b = if f & 1 != 0 { Some(g.bytes()?) } else { None };
        let q = if f & 4 != 0 { Some(g.f32s()?) } else { None };
        done.push((x, y, b, f & 2 != 0, q));
    }
    g.end()?;
    Ok(done)
}

/// What a terrain run reads besides its own tiles, a line each: the accepted elevation datasets
/// (the pool, the fine datasets, the Terrarium pyramid) by the index `manifest` names, and the
/// geoid sources converting their heights (`geoid`: crate::inputs::sources::geoid, what the pool's
/// readers take of the declared sources), each by its own declaration (`Source::named`). Another
/// source's declaration changed leaves it as it was (task #968).
pub(crate) fn inputs(manifest: &BTreeMap<String, String>, geoid: &[crate::inputs::sources::Source]) -> Vec<String> {
    let l = crate::inputs::logical(crate::inputs::elevation::UNIT);
    std::iter::once(format!("{l}={}", manifest.get(&l).map(String::as_str).unwrap_or("-")))
        .chain(geoid.iter().map(crate::inputs::sources::Source::named))
        .collect()
}

/// `inputs` of the records `manifest` names under `root`, as a run's resume log's identity names
/// them (one string).
pub(crate) fn inputs_read(root: &store::nasfs::NasRoot, manifest: &BTreeMap<String, String>) -> Result<String> {
    Ok(inputs(manifest, &crate::inputs::sources::geoid(root, manifest)?).join("\n"))
}

/// The tiles of `levels` at their place's finest zoom (`Sources::native`), as an identity names them.
fn natives(src: &crate::terrain_pack::Sources, levels: &[(u8, Vec<(u32, u32)>)]) -> String {
    let mut h = blake3::Hasher::new();
    for (z, ts) in levels {
        for &(x, y) in ts {
            if src.native.is_some_and(|n| n(*z, x, y)) {
                h.update(&[*z]);
                h.update(&x.to_le_bytes());
                h.update(&y.to_le_bytes());
            }
        }
    }
    h.finalize().to_hex().to_string()
}

/// A terrain-hi run's identity: z6 tile `t`, the levels it makes here `levels` (their tiles), in
/// blocks `up` levels up, from `inputs` (`inputs_read`) and sources `src`.
pub fn hi_id(
    inputs: &str,
    src: &crate::terrain_pack::Sources,
    t: (u8, u32, u32),
    levels: &[(u8, Vec<(u32, u32)>)],
    up: u8,
) -> String {
    let mut h = blake3::Hasher::new();
    for (z, ts) in levels {
        h.update(&[*z]);
        h.update(&(ts.len() as u64).to_le_bytes());
        for (x, y) in ts {
            h.update(&x.to_le_bytes());
            h.update(&y.to_le_bytes());
        }
    }
    crate::resume_log::id(&[
        &format!("terrain-hi {} blocks {up}", crate::agent::build::TERRAIN_V),
        &crate::terrain_codec::version(),
        &format!("{}/{}/{}", t.0, t.1, t.2),
        &h.finalize().to_hex(),
        inputs,
        &src.water.map(|w| w.pin()).unwrap_or_else(|| "-".into()),
        &natives(src, levels),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hi_log_reads_back_and_lets_go_of_the_means_no_level_reads() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/terrain-hi");
        let fin = |x: u32, q: bool| (x, 7, Some(vec![x as u8; 3]), x % 2 == 0, q.then(|| vec![x as f32, f32::NAN]));
        let k = |xs: &[u32]| -> Vec<Finished> { xs.iter().map(|&x| fin(x, true)).collect() };
        let mut l = HiLog::open(&scratch, (6, 1, 2), "a").unwrap();
        l.block(12, (0, 0), &k(&[1, 2])).unwrap();
        l.block(12, (1, 0), &[(5, 5, None, false, None)]).unwrap();
        l.end(12).unwrap();
        l.block(11, (0, 0), &k(&[4])).unwrap();
        drop(l);
        let mut l = HiLog::open(&scratch, (6, 1, 2), "a").unwrap();
        let z12 = l.level(12);
        assert_eq!(z12.blocks.len(), 2);
        let b = &z12.blocks[&(0, 0)];
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].2, Some(vec![2; 3]));
        assert!(b[1].3 && !b[0].3);
        assert!(b[0].4.as_ref().is_some_and(|q| q[0] == 1.0 && q[1].is_nan()));
        assert_eq!(z12.blocks[&(1, 0)], vec![(5, 5, None, false, None)]);
        assert!(z12.ended);
        assert!(!l.level(11).ended);
        // (Z11 ended: z12's means let go.)
        l.end(11).unwrap();
        drop(l);
        let mut l = HiLog::open(&scratch, (6, 1, 2), "a").unwrap();
        let z12 = l.level(12);
        assert!(z12.blocks.values().flatten().all(|f| f.4.is_none()));
        assert!(l.level(11).blocks[&(0, 0)][0].4.is_some());
        // Another identity: nothing.
        assert!(HiLog::open(&scratch, (6, 1, 2), "b").unwrap().level(12).blocks.is_empty());
    }
}
