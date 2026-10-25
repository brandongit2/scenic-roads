//! The terrain's long targets resumed where they stopped (task #650; docs/pool.md §7.2, Resuming),
//! their logs crate::resume_log's (this Mac's disk, at the top of the step's scratch folder, only
//! with resuming on):
//!
//! - **terrain-hi, a z6 tile** (`HiLog`; dense ones run 30 minutes and more, up to 5,440 tiles): a
//!   level's blocks (crate::terrain_pack's `blocks`: at z12 the tiles of one z8 tile, at z11 of
//!   one z7 tile, …, up to 64 a level), each kept as it's finished: its tiles finished (each its
//!   bytes, whether it changed, its 2×2 means for the level above), the levels of the lakes it
//!   alone has (worked out from it), and which of its tiles wait for the level's end (a lake they
//!   share with another block); then the level's end: the waiting tiles finished and the lakes'
//!   levels worked out there. A resumed run takes the blocks kept, prepares again only their
//!   waiting tiles (their shared lakes' samples, which the end reads), and makes the rest: a tile's
//!   finishing reads only its own lakes' levels, a lake's level its samples in any order
//!   (crate::terrain_water::level), so the same bytes. The identity (`hi_id`): the terrain's
//!   versions and codec, the z6 tile, the levels' tiles this run makes (the coverage's, less the
//!   subtrees out to other workers), the accepted elevation datasets and sources (the pool and its
//!   geoids), the water's pin, and its z10 tiles past the base's zoom.
//! - **terrain-deep, a z10 tile** (`DeepLog`; up to 22 minutes): its base tiles, each kept as its
//!   deep tiles (z13–16, a z12 tile's subtree) are made. A resumed run neither samples the
//!   presence of the base tiles kept nor reads their fine data, and makes the base only near the
//!   others. The identity (`deep_id`): the deep levels' versions, the terrain's, the z10 tile,
//!   what it reads of the fine datasets (crate::terrain_deep::target_key_lines), the accepted
//!   elevation datasets and sources, the water's pin, and the z10 tiles past the base's zoom of the
//!   z6 tiles its base is of.
//!
//! The steps not kept, measured from the history (21–25 Oct): slope-hi and slope-deep (the slope's
//! z6 tiles took 37 s at the median and 2 minutes at the 90th percentile with their deep levels
//! in them, before #639 split those out a z10 tile a target); a stop costs them little.

use crate::resume_log::{BlockId, Get, Log, Put};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    /// Z6 tile `t`'s log in `scratch`, of identity `id` (`hi_id`). The 2×2 means of a level whose
    /// next level has ended are let go as it's read: what's held is what the run it resumes held.
    pub fn open(scratch: &Path, t: (u32, u32), id: &str) -> Option<HiLog> {
        let mut levels: BTreeMap<u8, LevelKept> = BTreeMap::new();
        let log = Log::open(scratch, (6, t.0, t.1), "terrain-hi", id, &mut |b, p| {
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
fn inputs(manifest: &BTreeMap<String, String>) -> String {
    [crate::inputs::elevation::UNIT, crate::inputs::sources::UNIT]
        .iter()
        .map(|u| {
            let l = crate::inputs::logical(u);
            format!("{l}={}", manifest.get(&l).map(String::as_str).unwrap_or("-"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The z10 tiles of `deeper` under z6 tiles `pieces`, as a line.
fn deeper_under(deeper: Option<&BTreeSet<(u32, u32)>>, pieces: &BTreeSet<(u32, u32)>) -> String {
    deeper
        .into_iter()
        .flatten()
        .filter(|&&(x, y)| pieces.contains(&(x >> 4, y >> 4)))
        .map(|(x, y)| format!("{x},{y}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A terrain-hi run's identity: z6 tile `t`, the levels it makes here `levels` (their tiles), in
/// blocks `up` levels up, from what `manifest` names (`inputs`) and sources `src`.
pub fn hi_id(
    manifest: &BTreeMap<String, String>,
    src: &crate::terrain_pack::Sources,
    t: (u32, u32),
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
        &format!("6/{}/{}", t.0, t.1),
        &h.finalize().to_hex(),
        &inputs(manifest),
        &src.water.map(|w| w.pin()).unwrap_or_else(|| "-".into()),
        &deeper_under(src.deeper, &[t].into()),
    ])
}

/// A terrain-deep target's log: its base tiles kept, each with its deep tiles.
pub struct DeepLog {
    log: Log,
    /// The base tiles kept, each its deep tiles (z, x, y, bytes).
    pub kept: BTreeMap<(u8, u32, u32), Vec<(u8, u32, u32, Vec<u8>)>>,
}

impl DeepLog {
    /// Z10 tile `t`'s log in `scratch`, of identity `id` (`deep_id`).
    pub fn open(scratch: &Path, t: (u32, u32), id: &str) -> Option<DeepLog> {
        let mut kept = BTreeMap::new();
        let log = Log::open(scratch, (crate::terrain_deep::PACK_Z, t.0, t.1), "terrain-deep", id, &mut |b, p| {
            let mut g = Get(p);
            let mut tiles = Vec::new();
            for _ in 0..g.count(17)? {
                tiles.push((g.u8()?, g.u32()?, g.u32()?, g.bytes()?));
            }
            g.end()?;
            anyhow::ensure!(kept.insert(b, tiles).is_none(), "a base tile twice");
            Ok(())
        })?;
        Some(DeepLog { log, kept })
    }

    /// Base tile `b`'s deep tiles `tiles` made: kept.
    pub fn keep(&mut self, b: BlockId, tiles: &[(u8, u32, u32, Vec<u8>)]) -> Result<()> {
        let mut p = Put::default();
        p.u64(tiles.len() as u64);
        for (z, x, y, b) in tiles {
            p.u8(*z);
            p.u32(*x);
            p.u32(*y);
            p.bytes(b);
        }
        self.log.keep(b, &p.0)
    }
}

/// A terrain-deep run's identity: z10 tile `t` of the fine datasets `fine` and water `water`, its
/// base of the z6 tiles `pieces` from what `manifest` names (`inputs`) and sources `src`.
pub fn deep_id(
    manifest: &BTreeMap<String, String>,
    src: &crate::terrain_pack::Sources,
    water: Option<&dyn crate::terrain_water::WaterSource>,
    fine: &crate::terrain_deep::Fine,
    t: (u32, u32),
    pieces: &BTreeSet<(u32, u32)>,
) -> String {
    crate::resume_log::id(&[
        &format!(
            "terrain-deep {} {} terrain-hi {}",
            crate::agent::build::TERRAIN_DEEP_V,
            crate::terrain_deep::versions(),
            crate::agent::build::TERRAIN_V
        ),
        &crate::terrain_codec::version(),
        &format!("{}/{}/{}", crate::terrain_deep::PACK_Z, t.0, t.1),
        &crate::terrain_deep::target_key_lines(&fine.sets, t),
        &inputs(manifest),
        &[src.water, water].map(|w| w.map(|w| w.pin()).unwrap_or_else(|| "-".into())).join(" "),
        &deeper_under(src.deeper, pieces),
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
        let mut l = HiLog::open(&scratch, (1, 2), "a").unwrap();
        l.block(12, (0, 0), &k(&[1, 2])).unwrap();
        l.block(12, (1, 0), &Kept { done: vec![(5, 5, None, false, None)], ..Kept::default() }).unwrap();
        l.end(12, &k(&[3])).unwrap();
        l.block(11, (0, 0), &k(&[4])).unwrap();
        drop(l);
        let mut l = HiLog::open(&scratch, (1, 2), "a").unwrap();
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
        let mut l = HiLog::open(&scratch, (1, 2), "a").unwrap();
        let z12 = l.level(12);
        assert!(z12.blocks.values().flat_map(|k| &k.done).chain(&z12.end.unwrap().done).all(|f| f.4.is_none()));
        assert!(l.level(11).blocks[&(0, 0)].done[0].4.is_some());
        // Another identity: nothing.
        assert!(HiLog::open(&scratch, (1, 2), "b").unwrap().level(12).blocks.is_empty());
    }

    #[test]
    fn a_deep_log_reads_back() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/terrain-deep");
        let mut l = DeepLog::open(&scratch, (300, 380), "a").unwrap();
        l.keep((12, 1200, 1520), &[(13, 2400, 3040, vec![1, 2]), (14, 4800, 6080, vec![])]).unwrap();
        l.keep((12, 1201, 1520), &[]).unwrap();
        drop(l);
        let l = DeepLog::open(&scratch, (300, 380), "a").unwrap();
        assert_eq!(l.kept.len(), 2);
        assert_eq!(l.kept[&(12, 1200, 1520)], vec![(13, 2400, 3040, vec![1, 2]), (14, 4800, 6080, vec![])]);
    }
}
