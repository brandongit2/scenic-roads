//! The terrain's long targets resumed where they stopped (task #650; docs/pool.md §7.2, Resuming),
//! their logs crate::resume_log's (this Mac's disk, at the top of the step's scratch folder, only
//! with resuming on):
//!
//! - **terrain-hi, a z6 tile, and terrain-deep, a z10 tile** (`HiLog`; dense ones run 30 minutes
//!   and more, up to 5,440 tiles): a level's blocks (crate::terrain_pack's `blocks`: at z12 the
//!   tiles of one z8 tile, at z11 of one z7 tile, …, up to 64 a level), each kept as it's finished:
//!   its tiles finished (each its bytes, whether it changed, its 2×2 means for the level above), the
//!   levels of the lakes it alone has (worked out from it), and which of its tiles wait for the
//!   level's end (a lake they share with another block); then the level's end: the waiting tiles
//!   finished and the lakes' levels worked out there. A resumed run takes the blocks kept, prepares
//!   again only their waiting tiles (their shared lakes' samples, which the end reads), and makes
//!   the rest: a tile's finishing reads only its own lakes' levels, a lake's level its samples in
//!   any order (crate::terrain_water::level), so the same bytes. The identity (`hi_id`): the
//!   terrain's versions and codec, the tile, the levels' tiles this run makes (the coverage's, less
//!   the subtrees out to other workers), the accepted elevation datasets and sources (the pool and
//!   its geoids), the water's pin, and which of its tiles are at their place's finest zoom.
//!
//! The steps not kept, measured from the history (21–25 Oct): slope-hi and slope-deep (the slope's
//! z6 tiles took 37 s at the median and 2 minutes at the 90th percentile with their deep levels
//! in them, before #639 split those out a z10 tile a target); a stop costs them little.

use crate::resume_log::{Get, Log, Put};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// A tile finished (crate::terrain_pack's `finish`): its column and row, its bytes (None: its raw
/// tile didn't decode), whether it changed, its 2×2 means.
pub type Finished = (u32, u32, Option<Vec<u8>>, bool, Option<Vec<f32>>);

/// A block kept, or a level's end.
#[derive(Default)]
pub struct Kept {
    pub done: Vec<Finished>,
    /// The lakes' levels it worked out.
    pub lakes: Vec<(u64, f32)>,
    /// Its tiles waiting for the level's end (none for the end).
    pub waiting: Vec<(u32, u32)>,
}

/// A level's blocks kept (by the tile up their tiles are under: crate::terrain_pack's `blocks`),
/// and its end once kept.
#[derive(Default)]
pub struct LevelKept {
    pub blocks: HashMap<(u32, u32), Kept>,
    pub end: Option<Kept>,
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
            let k = parse(p)?;
            let lv = levels.entry(b.0).or_default();
            if b.1 == END {
                anyhow::ensure!(b.2 == END && lv.end.is_none(), "a level's end twice");
                lv.end = Some(k);
                if let Some(finer) = levels.get_mut(&(b.0 + 1)) {
                    for f in finer
                        .blocks
                        .values_mut()
                        .flat_map(|k| &mut k.done)
                        .chain(finer.end.iter_mut().flat_map(|k| &mut k.done))
                    {
                        f.4 = None;
                    }
                }
            } else {
                anyhow::ensure!(lv.end.is_none() && !lv.blocks.contains_key(&(b.1, b.2)), "a block twice");
                lv.blocks.insert((b.1, b.2), k);
            }
            Ok(())
        })?;
        Some(HiLog { log, levels })
    }

    /// Level `z`'s blocks kept, and its end: taken.
    pub fn level(&mut self, z: u8) -> LevelKept {
        self.levels.remove(&z).unwrap_or_default()
    }

    /// Level `z`'s block under tile `b` (`up` levels up) finished: kept.
    pub fn block(&mut self, z: u8, b: (u32, u32), k: &Kept) -> Result<()> {
        self.log.keep((z, b.0, b.1), &payload(k))
    }

    /// Level `z`'s end: kept.
    pub fn end(&mut self, z: u8, k: &Kept) -> Result<()> {
        self.log.keep((z, END, END), &payload(k))
    }
}

/// A block's (or a level's end's) record: its tiles finished, its lakes' levels, its tiles waiting.
fn payload(k: &Kept) -> Vec<u8> {
    let mut p = Put::default();
    p.u64(k.done.len() as u64);
    for (x, y, b, changed, q) in &k.done {
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
    p.u64(k.lakes.len() as u64);
    for (id, l) in &k.lakes {
        p.u64(*id);
        p.u32(l.to_bits());
    }
    p.u64(k.waiting.len() as u64);
    for (x, y) in &k.waiting {
        p.u32(*x);
        p.u32(*y);
    }
    p.0
}

fn parse(p: &[u8]) -> Result<Kept> {
    let mut g = Get(p);
    let mut k = Kept::default();
    for _ in 0..g.count(9)? {
        let (x, y, f) = (g.u32()?, g.u32()?, g.u8()?);
        let b = if f & 1 != 0 { Some(g.bytes()?) } else { None };
        let q = if f & 4 != 0 { Some(g.f32s()?) } else { None };
        k.done.push((x, y, b, f & 2 != 0, q));
    }
    for _ in 0..g.count(12)? {
        k.lakes.push((g.u64()?, f32::from_bits(g.u32()?)));
    }
    for _ in 0..g.count(8)? {
        k.waiting.push((g.u32()?, g.u32()?));
    }
    g.end()?;
    Ok(k)
}

/// What a terrain run reads besides its own tiles, as `manifest` names it: the accepted elevation
/// datasets (the pool, the fine datasets, the Terrarium pyramid) and sources (the geoids).
pub(crate) fn inputs(manifest: &BTreeMap<String, String>) -> String {
    [crate::inputs::elevation::UNIT, crate::inputs::sources::UNIT]
        .iter()
        .map(|u| {
            let l = crate::inputs::logical(u);
            format!("{l}={}", manifest.get(&l).map(String::as_str).unwrap_or("-"))
        })
        .collect::<Vec<_>>()
        .join("\n")
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
/// blocks `up` levels up, from what `manifest` names (`inputs`), sources `src` and the lakes' levels
/// it's given (`lakes`: a deep target's, crate::terrain_lakes).
pub fn hi_id(
    manifest: &BTreeMap<String, String>,
    src: &crate::terrain_pack::Sources,
    t: (u8, u32, u32),
    levels: &[(u8, Vec<(u32, u32)>)],
    up: u8,
    lakes: &BTreeMap<u64, f32>,
) -> String {
    let mut h = blake3::Hasher::new();
    for (id, l) in lakes {
        h.update(&id.to_le_bytes());
        h.update(&l.to_bits().to_le_bytes());
    }
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
        &inputs(manifest),
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
        let k = |xs: &[u32]| Kept {
            done: xs.iter().map(|&x| fin(x, true)).collect(),
            lakes: vec![(9, 1.25)],
            waiting: vec![(3, 4)],
        };
        let mut l = HiLog::open(&scratch, (6, 1, 2), "a").unwrap();
        l.block(12, (0, 0), &k(&[1, 2])).unwrap();
        l.block(12, (1, 0), &Kept { done: vec![(5, 5, None, false, None)], ..Kept::default() }).unwrap();
        l.end(12, &k(&[3])).unwrap();
        l.block(11, (0, 0), &k(&[4])).unwrap();
        drop(l);
        let mut l = HiLog::open(&scratch, (6, 1, 2), "a").unwrap();
        let z12 = l.level(12);
        assert_eq!(z12.blocks.len(), 2);
        let b = &z12.blocks[&(0, 0)];
        assert_eq!((b.done.len(), b.lakes.clone(), b.waiting.clone()), (2, vec![(9, 1.25)], vec![(3, 4)]));
        assert_eq!(b.done[1].2, Some(vec![2; 3]));
        assert!(b.done[1].3 && !b.done[0].3);
        assert!(b.done[0].4.as_ref().is_some_and(|q| q[0] == 1.0 && q[1].is_nan()));
        assert_eq!(z12.blocks[&(1, 0)].done, vec![(5, 5, None, false, None)]);
        assert_eq!(z12.end.as_ref().unwrap().done.len(), 1);
        assert!(l.level(11).end.is_none());
        // (Z11 ended: z12's means let go.)
        l.end(11, &Kept::default()).unwrap();
        drop(l);
        let mut l = HiLog::open(&scratch, (6, 1, 2), "a").unwrap();
        let z12 = l.level(12);
        assert!(z12.blocks.values().flat_map(|k| &k.done).chain(&z12.end.unwrap().done).all(|f| f.4.is_none()));
        assert!(l.level(11).blocks[&(0, 0)].done[0].4.is_some());
        // Another identity: nothing.
        assert!(HiLog::open(&scratch, (6, 1, 2), "b").unwrap().level(12).blocks.is_empty());
    }
}
