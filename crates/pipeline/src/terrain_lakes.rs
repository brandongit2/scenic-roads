//! The levels of the lakes across the deep targets' edges, decided once a lake (docs/steps.md §2,
//! Terrain): the `terrain-lakes` step, a z6 tile a job, before its terrain-deep targets. A deep target
//! levels a lake from its own tiles (crate::terrain_pack), so a lake two targets share would sit a
//! little apart in each (task #942: a few centimetres to metres); its level is decided here instead,
//! from the whole lake, and handed to every target it reaches.
//!
//! - **A lake across:** an OSM lake (a lake of a tile's own, crate::terrain_water::own_key, is never
//!   another tile's) whose tiles at `LEVEL_Z` lie in two z10 tiles or more. Its tiles (`Lakes::lake`):
//!   every `LEVEL_Z` tile of the z10 tiles it reaches holding it, a z10 tile reached where one of its
//!   tiles touches (8 neighbours) one of the lake's in a z10 tile already reached; so the same from
//!   whichever of its targets it's found, and one lake a z10 tile.
//! - **Its level:** crate::terrain_water::level over its tiles at `LEVEL_Z` with a shore (those it
//!   doesn't wholly cover), each sampled from the elevation pool as it has it at that zoom
//!   (crate::terrain_base: the same bytes in every job), before any water is flattened.
//! - **Handed on:** a file a deep target (`logical`, only where it has such a lake): its lakes' levels,
//!   which its run takes as known (crate::terrain_deep::build_target), so every target's tiles of a
//!   lake, and the z6 tiles' z9–12 from their deep mids, flatten it to one level. Its key names that
//!   file by content: a lake changed re-keys only the targets it reaches.
//!
//! What it reads: the water (the latest basemap and the lake fill) and the elevation pool, past its
//! z6 tile where a lake goes on; its key names them whole (agent::steps).

use crate::terrain_pack::{RawSource, Sources};
use crate::terrain_water::{Kind, WaterSource};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// Bumped with any change to the levels decided from the same inputs.
pub const V: u32 = 1;
/// The zoom a lake's level is decided at: the basemap's water's finest (crate::terrain_water).
pub const LEVEL_Z: u8 = 14;
/// A z10 tile's `LEVEL_Z` tiles a side.
const SIDE: u32 = 1 << (LEVEL_Z - crate::terrain_deep::PACK_Z);

/// The levels' logical names' prefix.
pub const PREFIX: &str = "work/terrain-lakes/";

/// Deep target `t`'s lakes' levels (a z10 tile's).
pub fn logical(t: (u32, u32)) -> String {
    format!("{PREFIX}{}-{}-{}", crate::terrain_deep::PACK_Z, t.0, t.1)
}

/// A levels file's z10 tile, from its logical name.
pub fn tile_of(l: &str) -> Option<(u32, u32)> {
    let u = crate::legacy::TileId::parse(l.strip_prefix(PREFIX)?).filter(|u| u.z == crate::terrain_deep::PACK_Z)?;
    (logical((u.x, u.y)) == l).then_some((u.x, u.y))
}

/// The levels files the manifest names under z6 tile `t`.
pub fn named_under(m: &BTreeMap<String, String>, t: (u32, u32)) -> Vec<String> {
    let k = crate::terrain_deep::PACK_Z - 6;
    m.range(PREFIX.to_string()..)
        .take_while(|(l, _)| l.starts_with(PREFIX))
        .filter(|(l, _)| tile_of(l).is_some_and(|f| (f.0 >> k, f.1 >> k) == t))
        .map(|(l, _)| l.clone())
        .collect()
}

/// The levels file's format.
const FMT: u64 = 1;

/// A levels file: JSON, `{"fmt": 1, "step": "terrain-lakes", "tile": "10/x/y", "v": V, "lakes":
/// {"<OSM id>": <level, m>}}`.
#[derive(serde::Serialize, serde::Deserialize)]
struct File {
    fmt: u64,
    step: String,
    tile: String,
    v: u32,
    lakes: BTreeMap<u64, f32>,
}

/// Target `t`'s levels as written (`read`).
pub fn to_bytes(t: (u32, u32), lakes: &BTreeMap<u64, f32>) -> Vec<u8> {
    let f = File {
        fmt: FMT,
        step: "terrain-lakes".into(),
        tile: format!("{}/{}/{}", crate::terrain_deep::PACK_Z, t.0, t.1),
        v: V,
        lakes: lakes.clone(),
    };
    serde_json::to_vec(&f).expect("levels as JSON")
}

/// Target `t`'s levels from its file's bytes, checked: its format, version and tile.
pub fn from_bytes(t: (u32, u32), b: &[u8]) -> Result<BTreeMap<u64, f32>> {
    let f: File = serde_json::from_slice(b)?;
    let tile = format!("{}/{}/{}", crate::terrain_deep::PACK_Z, t.0, t.1);
    anyhow::ensure!(
        f.fmt == FMT && f.step == "terrain-lakes" && f.v == V && f.tile == tile,
        "not {tile}'s lake levels of version {V}"
    );
    Ok(f.lakes)
}

/// Each `LEVEL_Z` tile's OSM lakes, and whether it lies wholly in each, by z10 tile, read once.
pub struct Lakes<'a> {
    water: &'a dyn WaterSource,
    read: Mutex<HashMap<(u32, u32), Arc<Vec<Vec<(u64, bool)>>>>>,
}

/// The lakes across its z10 tiles' edges that a z6 tile's targets reach (`Lakes::across`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Across {
    /// Each lake: its id and its z10 tiles (two or more).
    pub lakes: Vec<(u64, BTreeSet<(u32, u32)>)>,
    /// Each target's lakes (indexes into `lakes`), only those reaching one.
    pub of: BTreeMap<(u32, u32), Vec<usize>>,
}

impl<'a> Lakes<'a> {
    pub fn new(water: &'a dyn WaterSource) -> Self {
        Lakes { water, read: Mutex::new(HashMap::new()) }
    }

    /// Z10 tile `v`'s `LEVEL_Z` tiles (row by row): each one's OSM lakes, and whether it's wholly
    /// that lake's.
    fn at(&self, v: (u32, u32)) -> Result<Arc<Vec<Vec<(u64, bool)>>>> {
        if let Some(a) = self.read.lock().unwrap().get(&v) {
            return Ok(a.clone());
        }
        let tiles: Vec<(u8, u32, u32)> =
            (0..SIDE * SIDE).map(|k| (LEVEL_Z, v.0 * SIDE + k % SIDE, v.1 * SIDE + k / SIDE)).collect();
        // (Its rivers left out: a lake's pixels are as the terrain's water has them.)
        let got: Vec<Vec<(u64, bool)>> = self
            .water
            .depth_polys_many(&tiles)?
            .into_iter()
            .map(|mut polys| {
                polys.retain(|p| matches!(p.kind, Kind::Sea | Kind::Lake));
                if !polys.iter().any(|p| p.kind == Kind::Lake && p.id >> 63 == 0) {
                    return Vec::new();
                }
                let wt = crate::terrain_water::water_tile(&polys);
                wt.ids
                    .iter()
                    .enumerate()
                    .filter(|(_, id)| **id >> 63 == 0)
                    .map(|(k, &id)| {
                        let whole = (0..wt.lake_of.len())
                            .all(|p| wt.lake_of[p] as usize == k && wt.lake[p] == crate::terrain_water::FULL);
                        (id, whole)
                    })
                    .collect()
            })
            .collect();
        let a = Arc::new(got);
        self.read.lock().unwrap().insert(v, a.clone());
        Ok(a)
    }

    /// Lake `id`'s z10 tiles from `seed` (one holding it): those reached from it where one of the
    /// lake's `LEVEL_Z` tiles touches one of a neighbour's, the same set from any of them.
    pub fn lake(&self, id: u64, seed: (u32, u32)) -> Result<BTreeSet<(u32, u32)>> {
        let n = 1i64 << LEVEL_Z;
        let s = SIDE as i64;
        let mut reached = BTreeSet::from([seed]);
        let mut todo = vec![seed];
        while let Some(v) = todo.pop() {
            let here = self.at(v)?;
            for (k, ids) in here.iter().enumerate() {
                let (i, j) = ((k as i64) % s, (k as i64) / s);
                if !ids.iter().any(|l| l.0 == id) || (0 < i && i < s - 1 && 0 < j && j < s - 1) {
                    continue;
                }
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (tx, ty) = (v.0 as i64 * s + i + dx, v.1 as i64 * s + j + dy);
                    if !(0..n).contains(&tx) || !(0..n).contains(&ty) {
                        continue;
                    }
                    let w = ((tx / s) as u32, (ty / s) as u32);
                    if reached.contains(&w) {
                        continue;
                    }
                    if self.at(w)?[((ty % s) * s + tx % s) as usize].iter().any(|l| l.0 == id) {
                        reached.insert(w);
                        todo.push(w);
                    }
                }
            }
        }
        Ok(reached)
    }

    /// The lakes across z10 tiles' edges that the targets `targets` reach (`Across`), each found
    /// once from the first target holding it on its edge.
    pub fn across(&self, targets: &[(u32, u32)]) -> Result<Across> {
        let s = SIDE as usize;
        let mut out = Across::default();
        let mut found: HashMap<(u64, (u32, u32)), usize> = HashMap::new();
        let mut alone: BTreeSet<(u64, (u32, u32))> = BTreeSet::new();
        for &t in targets {
            let here = self.at(t)?;
            let edge: BTreeSet<u64> = here
                .iter()
                .enumerate()
                .filter(|(k, _)| k % s == 0 || k % s == s - 1 || k / s == 0 || k / s == s - 1)
                .flat_map(|(_, ids)| ids.iter().map(|l| l.0))
                .collect();
            let mut mine = Vec::new();
            for id in edge {
                let n = match found.get(&(id, t)) {
                    Some(&n) => n,
                    None if alone.contains(&(id, t)) => continue,
                    None => {
                        let tiles = self.lake(id, t)?;
                        if tiles.len() < 2 {
                            alone.insert((id, t));
                            continue;
                        }
                        let n = out.lakes.len();
                        found.extend(tiles.iter().map(|&v| ((id, v), n)));
                        out.lakes.push((id, tiles));
                        n
                    }
                };
                mine.push(n);
            }
            if !mine.is_empty() {
                out.of.insert(t, mine);
            }
        }
        Ok(out)
    }

    /// Lake `n` of `across`'s `LEVEL_Z` tiles with a shore: those holding it that it doesn't wholly
    /// cover.
    pub fn shore(&self, across: &Across, n: usize) -> Result<Vec<(u32, u32)>> {
        let (id, tiles) = &across.lakes[n];
        let mut out = Vec::new();
        for &v in tiles {
            for (k, ids) in self.at(v)?.iter().enumerate() {
                if ids.iter().any(|l| l.0 == *id && !l.1) {
                    out.push((v.0 * SIDE + k as u32 % SIDE, v.1 * SIDE + k as u32 / SIDE));
                }
            }
        }
        Ok(out)
    }

    /// Every lake's shore tiles (`shore`), each with the lakes it's a shore of: what `decide` samples.
    pub fn to_sample(&self, across: &Across) -> Result<BTreeMap<(u32, u32), Vec<usize>>> {
        let mut want: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
        for n in 0..across.lakes.len() {
            for t in self.shore(across, n)? {
                want.entry(t).or_default().push(n);
            }
        }
        Ok(want)
    }
}

/// Tiles sampled at once (`decide`): their heights held together.
const CHUNK: usize = 256;

/// The most memory (MB) a z6 tile's run takes, from its bounds: a chunk of tiles sampled and
/// prepared (`CHUNK`, ~800 KB each: the sample, its heights and water), the pool's chunks being
/// sampled (crate::terrain_base::sampling_mb) and the samplers' caches
/// (crate::dem::generic::samplers_mb).
pub const PEAK_MB: u64 =
    CHUNK as u64 * 800 / 1024 + crate::terrain_base::sampling_mb() + crate::dem::generic::samplers_mb();

/// Each lake of `across`'s level (None: no shore and no flat inside), from its shore tiles `want`
/// (`Lakes::to_sample`) sampled from `pool` at `LEVEL_Z` (the elevation pool's own sample: no
/// coarser tile's means), their water from `src`; `progress` told each chunk sampled.
pub fn decide(
    pool: &dyn RawSource,
    src: &Sources,
    across: &Across,
    want: &BTreeMap<(u32, u32), Vec<usize>>,
    progress: &dyn Fn(u64, u64),
) -> Result<Vec<Option<f32>>> {
    use rayon::prelude::*;
    let tiles: Vec<(u32, u32)> = want.keys().copied().collect();
    let none = HashMap::new();
    let mut samples: Vec<crate::terrain_water::LakeSamples> = vec![Default::default(); across.lakes.len()];
    let done = std::sync::atomic::AtomicU64::new(0);
    for (k, chunk) in tiles.chunks(CHUNK).enumerate() {
        pool.prefetch_counted(LEVEL_Z, chunk, rayon::current_num_threads(), &done)?;
        let got: Vec<Result<Vec<(usize, crate::terrain_water::LakeSamples)>>> = chunk
            .par_iter()
            .map(|&(x, y)| {
                let (raw, _) = pool.get(LEVEL_Z, x, y)?;
                let mut s = crate::terrain_pack::prepare(raw, LEVEL_Z, x, y, &none, src)?.lake_samples();
                Ok(want[&(x, y)].iter().filter_map(|&n| s.remove(&across.lakes[n].0).map(|v| (n, v))).collect())
            })
            .collect();
        for r in got {
            for (n, s) in r? {
                samples[n].shore.extend(s.shore);
                samples[n].inside.extend(s.inside);
            }
        }
        progress(((k + 1) * CHUNK).min(tiles.len()) as u64, tiles.len() as u64);
    }
    Ok(samples.iter().map(crate::terrain_water::level).collect())
}

/// Each target's lakes' levels (`Across::of`, those with one), as its file holds them.
pub fn by_target(across: &Across, levels: &[Option<f32>]) -> BTreeMap<(u32, u32), BTreeMap<u64, f32>> {
    let mut out = BTreeMap::new();
    for (t, ns) in &across.of {
        let l: BTreeMap<u64, f32> = ns.iter().filter_map(|&n| levels[n].map(|l| (across.lakes[n].0, l))).collect();
        if !l.is_empty() {
            out.insert(*t, l);
        }
    }
    out
}

/// The box (west, south, east, north) of the tiles `want` samples, for the pool to read
/// (crate::terrain_base::Pool::read); None for none.
pub fn sample_box(want: &BTreeMap<(u32, u32), Vec<usize>>) -> Option<[f64; 4]> {
    let ts: Vec<(u32, u32)> = want.keys().copied().collect();
    (!ts.is_empty()).then(|| crate::terrain_base::tiles_box(LEVEL_Z, &ts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_water::tests::{square, Fixed};

    /// A z10 tile's first `LEVEL_Z` column.
    const X0: u32 = 300 * SIDE;
    const Y0: u32 = 360 * SIDE;

    /// Lake 7 across the edge between z10 tiles 300/360 and 301/360 (its `LEVEL_Z` columns 14–17,
    /// row 5); lake 8 in 300/360 alone; lake 9 a part in each of 300/361 and 301/361, not touching.
    fn polys(z: u8, x: u32, y: u32) -> Vec<crate::terrain_water::Poly> {
        if z != LEVEL_Z {
            return Vec::new();
        }
        let (c, r) = (x - X0, y - Y0);
        let mut v = Vec::new();
        if (14..=17).contains(&c) && r == 5 {
            // (Wholly the lake's in columns 15 and 16, half in 14 and 17.)
            let x0 = if c == 14 { 128.0 } else { 0.0 };
            let x1 = if c == 17 { 128.0 } else { 256.0 };
            v.push(square(Kind::Lake, 7, x0, 0.0, x1, 256.0));
        }
        if c == 3 && r == 3 {
            v.push(square(Kind::Lake, 8, 10.0, 10.0, 200.0, 200.0));
        }
        if (c == 14 || c == 17) && r == 20 {
            v.push(square(Kind::Lake, 9, 10.0, 10.0, 200.0, 200.0));
        }
        v
    }

    #[test]
    fn a_lake_is_found_whole_from_either_side_and_only_across() {
        let water = Fixed(polys);
        let lakes = Lakes::new(&water);
        let both = BTreeSet::from([(300, 360), (301, 360)]);
        assert_eq!(lakes.lake(7, (300, 360)).unwrap(), both);
        assert_eq!(lakes.lake(7, (301, 360)).unwrap(), both);
        assert_eq!(lakes.lake(9, (300, 361)).unwrap(), BTreeSet::from([(300, 361)]), "parts that don't touch");
        let a = Lakes::new(&water).across(&[(300, 360), (301, 360), (300, 361), (301, 361)]).unwrap();
        assert_eq!(a.lakes, vec![(7, both)], "lake 8 within one z10 tile, lake 9's parts apart");
        assert_eq!(a.of, BTreeMap::from([((300, 360), vec![0]), ((301, 360), vec![0])]));
        // Its shore: the tiles it doesn't wholly cover.
        assert_eq!(lakes.shore(&a, 0).unwrap(), vec![(X0 + 14, Y0 + 5), (X0 + 17, Y0 + 5)]);
    }

    /// A pool whose heights rise to the east, so each side of an edge has its own shore.
    struct Ramp;
    impl RawSource for Ramp {
        fn get(&self, _z: u8, x: u32, _y: u32) -> Result<(Option<Vec<u8>>, bool)> {
            let e: Vec<f32> = (0..65536).map(|p| 100.0 + (x - X0) as f32 * 10.0 + (p % 256) as f32 / 256.0).collect();
            Ok((Some(roadcore::grid::encode_terrain_png(&e, 256, 256)?), false))
        }
        fn prefetch_counted(
            &self,
            _z: u8,
            _t: &[(u32, u32)],
            _n: usize,
            _d: &std::sync::atomic::AtomicU64,
        ) -> Result<usize> {
            Ok(0)
        }
    }

    /// Its level is decided from its whole shore, the same for every target it reaches, handed on
    /// in each target's file and read back.
    #[test]
    fn one_level_a_lake_for_every_target_it_reaches() {
        let water = Fixed(polys);
        let lakes = Lakes::new(&water);
        let a = lakes.across(&[(300, 360), (301, 360)]).unwrap();
        let want = lakes.to_sample(&a).unwrap();
        let src = Sources { water: Some(&water), ..Default::default() };
        let lv = decide(&Ramp, &src, &a, &want, &|_, _| {}).unwrap();
        let l = lv[0].unwrap();
        // (The west shore's land at ~240 m, the east's at ~270 m: the whole shore's tenth percentile
        // is the west's.)
        assert!((240.0..241.0).contains(&l), "{l}");
        let by = by_target(&a, &lv);
        assert_eq!(by[&(300, 360)], by[&(301, 360)]);
        let b = to_bytes((300, 360), &by[&(300, 360)]);
        assert_eq!(from_bytes((300, 360), &b).unwrap(), by[&(300, 360)]);
        assert!(from_bytes((301, 360), &b).is_err(), "another target's");
        assert_eq!(tile_of(&logical((300, 360))), Some((300, 360)));
        assert!(sample_box(&want).is_some_and(|b| b[0] < b[2] && b[1] < b[3]));
    }

    /// Each target's run (crate::terrain_pack::make_piece_logged over its tiles of the lake) levels
    /// it from its own shore, a step at their edge; given the level decided once, both flatten it
    /// to that level, its pixels the same on either side.
    #[test]
    fn two_targets_given_the_level_meet_without_a_step() {
        let water = Fixed(polys);
        let src = Sources { water: Some(&water), ..Default::default() };
        let lakes = Lakes::new(&water);
        let a = lakes.across(&[(300, 360), (301, 360)]).unwrap();
        let decided = decide(&Ramp, &src, &a, &lakes.to_sample(&a).unwrap(), &|_, _| {}).unwrap();
        let by = by_target(&a, &decided);
        let run = |x: u32, given: BTreeMap<u64, f32>| {
            let (made, mid) = crate::terrain_pack::make_piece_logged(
                &Ramp,
                &src,
                vec![(LEVEL_Z, vec![(X0 + x, Y0 + 5)])],
                None,
                given,
            )
            .unwrap();
            let e = roadcore::grid::decode_terrain(&made[0].3).unwrap();
            // (A pixel wholly the lake's: its row's middle, inside the lake's half of the tile.)
            let px = if x == 14 { 200 } else { 50 };
            (mid.levels[&7], e[128 * 256 + px])
        };
        let (west, east) = (run(14, BTreeMap::new()), run(17, BTreeMap::new()));
        assert!(east.0 - west.0 > 20.0, "each its own shore: {west:?} {east:?}");
        let (west, east) = (run(14, by[&(300, 360)].clone()), run(17, by[&(301, 360)].clone()));
        assert_eq!(west, east);
        assert_eq!(west.0, decided[0].unwrap());
    }
}
