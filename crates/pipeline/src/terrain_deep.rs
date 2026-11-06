//! The terrain pyramid's z13–16 band (docs/steps.md §2, Terrain), a z10 tile a job: the `terrain-deep`
//! step. Its tiles are made by the one rule every zoom follows (crate::terrain_pack: `Fill`): a tile
//! at its place's finest zoom (crate::terrain_zoom::finest, from the declared datasets meeting it)
//! is sampled from the pool, each dataset read at most at its own finest detail and the next in the
//! declared order filling its voids (crate::terrain_base); each coarser tile is its children's 2×2
//! means, a quarter whose child is past its place's finest zoom sampled there. It keeps the
//! source's heights as every level does: at sea, over lakes and below 0 m.
//!
//! - **Its tiles:** under the z10 tile, each of z13 to the deepest stored zoom near the coverage
//!   (20 km) no finer than its place's finest zoom (`levels`). A z10 tile with none is no target.
//! - **Packs:** by z10 tile (`PACK_Z`): `layers/terrain/deep/10-x-y` (docs/formats.md, Pack), z13 to
//!   `MAX_Z`; optional in a region's download (crate::areafiles: "Terrain detail").
//! - **Its mid** (`mid_logical`, `work/terrain-deep-mid/10-x-y`): its z13 tiles' 2×2 means before
//!   rounding, which its z6 tile's terrain-hi run takes in (its z12 tiles' quarters there:
//!   crate::terrain_pack::build_piece): so the z6 tile's run waits for its z10 tiles' targets
//!   (agent::steps' declared reads), and a z6 tile with none waits for nothing.
//!
//! The band's edges are where its packs are (the hi packs end at z12, `FIRST_Z` − 1), a storage
//! layout the server and the downloads read; no dataset or zoom of the data is named here.

use crate::out::Out;
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

/// The zoom of a deep pack's tile (a job's target).
pub const PACK_Z: u8 = 10;
/// The band's first zoom: the hi packs' last (z12) and one.
pub const FIRST_Z: u8 = 13;
/// The band's last: the deepest zoom the map stores.
pub const MAX_Z: u8 = crate::terrain_zoom::DEEPEST;
/// What a deep target's key names of its layout: the band, its packs' zoom and the zoom rule's
/// cell factor (its code's versions are its step's: agent::build::TERRAIN_DEEP_V, the codec's).
pub fn versions() -> String {
    format!("band {FIRST_Z}-{MAX_Z} pack {PACK_Z} cell factor {}", crate::terrain_zoom::CELL_FACTOR)
}

/// Z10 tile `t`'s native zoom: the finest of its places' (crate::terrain_zoom::finest). The
/// catalogue's `native_zoom` (by deep pack) and the map's zoom by area (crate::layers).
pub fn native_zoom(sets: &[crate::terrain_zoom::Set], t: (u32, u32)) -> u8 {
    crate::terrain_zoom::finest(sets, PACK_Z, t.0, t.1)
}

/// Z10 tile `t`'s tiles of the band, finest first (`MAX_Z` → `FIRST_Z`): those near the coverage
/// (20 km) no finer than their place's finest zoom.
pub fn levels(
    cov: &crate::coverage::Coverage,
    sets: &[crate::terrain_zoom::Set],
    t: (u32, u32),
) -> Vec<(u8, Vec<(u32, u32)>)> {
    if native_zoom(sets, t) < FIRST_Z || !crate::terrain_pack::near_coverage(cov, PACK_Z, t.0, t.1) {
        return Vec::new();
    }
    // (A tile's finest zoom is never past its parent's: its sources are among its parent's, and its
    // poleward edge no further; so a level's tiles are under the coarser level's.)
    let mut out: Vec<(u8, Vec<(u32, u32)>)> = Vec::new();
    let mut above: Vec<(u32, u32)> = vec![t];
    for z in PACK_Z + 1..=MAX_Z {
        let here: Vec<(u32, u32)> = above
            .iter()
            .flat_map(|&(x, y)| (0..4u32).map(move |k| (2 * x + (k & 1), 2 * y + (k >> 1))))
            .filter(|&(x, y)| {
                crate::terrain_pack::near_coverage(cov, z, x, y) && z <= crate::terrain_zoom::finest(sets, z, x, y)
            })
            .collect();
        if here.is_empty() {
            break;
        }
        if z >= FIRST_Z {
            out.push((z, here.clone()));
        }
        above = here;
    }
    out.reverse();
    out
}

/// Whether z10 tile `t` is a terrain-deep target: it has tiles of the band (`levels`).
pub fn is_target(sets: &[crate::terrain_zoom::Set], cov: &crate::coverage::Coverage, t: (u32, u32)) -> bool {
    is_target_with(sets, t, &levels(cov, sets, t))
}

/// `is_target`, given z10 tile `t`'s `levels`.
pub fn is_target_with(sets: &[crate::terrain_zoom::Set], t: (u32, u32), levels: &[(u8, Vec<(u32, u32)>)]) -> bool {
    native_zoom(sets, t) >= FIRST_Z && levels.iter().any(|(_, ts)| !ts.is_empty())
}

/// The terrain-deep targets under z6 tile `t` (`is_target`).
pub fn targets_in(
    sets: &[crate::terrain_zoom::Set],
    cov: &crate::coverage::Coverage,
    t: (u32, u32),
) -> Vec<(u32, u32)> {
    targets_under(sets, t, |u| is_target(sets, cov, u))
}

/// `targets_in`, each z10 tile under z6 tile `t` told by `is_target` (`Deep`'s, kept).
pub fn targets_under(
    sets: &[crate::terrain_zoom::Set],
    t: (u32, u32),
    is_target: impl Fn((u32, u32)) -> bool,
) -> Vec<(u32, u32)> {
    let k = PACK_Z - 6;
    if crate::terrain_zoom::finest(sets, 6, t.0, t.1) < FIRST_Z {
        return Vec::new();
    }
    (t.0 << k..(t.0 + 1) << k)
        .flat_map(|x| (t.1 << k..(t.1 + 1) << k).map(move |y| (x, y)))
        .filter(|&u| is_target(u))
        .collect()
}

/// Levels' tiles (z, then each column and row), as a key names them: what a terrain-deep z10
/// tile's run reads of the coverage, the tiles of its `levels` (`Deep::coverage`); a terrain z6
/// tile's, its (crate::terrain_pack::piece_levels: agent::build::piece_coverage).
pub fn levels_digest(levels: &[(u8, Vec<(u32, u32)>)]) -> String {
    let lines: Vec<String> = levels
        .iter()
        .map(|(z, ts)| format!("{z}:{}", ts.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(" ")))
        .collect();
    format!("tiles {}", store::naming::hash16(lines.join("\n").as_bytes()))
}

/// The terrain-deep answers worked out of one coverage version and the declared sets (`Deep`).
#[derive(Default)]
pub struct Memo {
    /// By z10 tile: whether it's a target (`is_target`) and its levels' digest (`Deep::coverage`;
    /// None: it has no levels).
    tiles: Mutex<HashMap<(u32, u32), (bool, Option<Arc<str>>)>>,
    /// By z6 tile: its targets (`targets_in`).
    targets: Mutex<HashMap<(u32, u32), Arc<[(u32, u32)]>>>,
}

/// The terrain-deep targets and the coverage their keys name, of a coverage and the declared sets:
/// each z10 tile's `levels` (a polygon test on each of its z11–16 tiles: ~85% of a plan's time,
/// worked out per target and twice a plan before task #965) worked out once for both, kept as
/// long as its `Memo` is (`Kept`: the agent's, agent::tiles::TerrainTiles::deep).
pub struct Deep<'a> {
    pub cov: &'a crate::coverage::Coverage,
    pub sets: Arc<Vec<crate::terrain_zoom::Set>>,
    memo: Arc<Memo>,
}

impl<'a> Deep<'a> {
    /// Kept for as long as it lives (a job's: no agent's `Kept`).
    pub fn new(cov: &'a crate::coverage::Coverage, sets: Arc<Vec<crate::terrain_zoom::Set>>) -> Deep<'a> {
        Deep { cov, sets, memo: Default::default() }
    }

    /// Its answers' memo (tests: whether two share it).
    pub fn memo(&self) -> &Arc<Memo> {
        &self.memo
    }

    /// Z10 tile `t`'s answers, worked out once.
    fn tile(&self, t: (u32, u32)) -> (bool, Option<Arc<str>>) {
        if let Some(v) = self.memo.tiles.lock().unwrap_or_else(|e| e.into_inner()).get(&t) {
            return v.clone();
        }
        let lv = levels(self.cov, &self.sets, t);
        let v = (is_target_with(&self.sets, t, &lv), (!lv.is_empty()).then(|| levels_digest(&lv).into()));
        self.memo.tiles.lock().unwrap_or_else(|e| e.into_inner()).insert(t, v.clone());
        v
    }

    /// The terrain-deep targets under z6 tile `t` (`targets_in`).
    pub fn targets_in(&self, t: (u32, u32)) -> Arc<[(u32, u32)]> {
        if let Some(v) = self.memo.targets.lock().unwrap_or_else(|e| e.into_inner()).get(&t) {
            return v.clone();
        }
        let v: Arc<[(u32, u32)]> = targets_under(&self.sets, t, |u| self.tile(u).0).into();
        self.memo.targets.lock().unwrap_or_else(|e| e.into_inner()).insert(t, v.clone());
        v
    }

    /// What z10 tile `t`'s run reads of the coverage, as its keys name it: its levels' tiles,
    /// digested (`levels_digest`).
    pub fn coverage(&self, t: (u32, u32)) -> String {
        match self.tile(t).1 {
            Some(d) => d.to_string(),
            None => levels_digest(&[]),
        }
    }
}

/// The coverage versions `Kept` keeps answers of: the whole coverage's, and a few regions' (their
/// progress asks each its own). A version's answers are a few MB at most: an entry per z10 tile
/// asked (a digest for a target), one per z6 tile.
const KEPT: usize = 16;

/// `Deep`'s answers kept by the coverage's version (crate::coverage::Coverage::version) and the
/// declared sets they were worked out of (held, so told apart by their pointer), the latest `KEPT`.
#[derive(Default)]
pub struct Kept(Mutex<Vec<(String, Arc<Vec<crate::terrain_zoom::Set>>, Arc<Memo>)>>);

impl Kept {
    /// The answers of coverage `cov` and the declared sets `sets`, kept: the same memo for every
    /// asker while neither changes.
    pub fn get<'a>(&self, cov: &'a crate::coverage::Coverage, sets: Arc<Vec<crate::terrain_zoom::Set>>) -> Deep<'a> {
        let v = cov.version();
        let mut all = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let memo = match all.iter().position(|(c, s, _)| *c == v && Arc::ptr_eq(s, &sets)) {
            Some(i) => {
                let e = all.remove(i);
                let memo = e.2.clone();
                all.push(e);
                memo
            }
            None => {
                if all.len() >= KEPT {
                    all.remove(0);
                }
                let memo = Arc::new(Memo::default());
                all.push((v, sets.clone(), memo.clone()));
                memo
            }
        };
        Deep { cov, sets, memo }
    }
}

/// What deep target `t`'s key names of its sources (crate::terrain_zoom::key_lines over it) and
/// its code (`versions`).
pub fn target_key_lines(sets: &[crate::terrain_zoom::Set], t: (u32, u32)) -> String {
    let mut out = vec![versions()];
    out.extend(crate::terrain_zoom::key_lines(sets, PACK_Z, t.0, t.1));
    out.join("\n")
}

/// The most memory (MB) a target's run takes, from its bounds: its tiles as a z6 tile's run holds
/// its levels (crate::terrain_pack::piece_peak_mb's parts: its tiles held until written, z13 to
/// `MAX_Z` under a z10 tile, ~270 KB each made; a level's quarters; the pool's chunks being
/// sampled, crate::terrain_base::sampling_mb, and the samplers' caches,
/// crate::dem::generic::samplers_mb).
pub const fn target_peak_mb() -> u64 {
    let mut tiles = 0u64;
    let mut z = FIRST_Z;
    while z <= MAX_Z {
        tiles += 1 << (2 * (z - PACK_Z));
        z += 1;
    }
    tiles * 270 / 1024
        + (1 << (2 * (MAX_Z - PACK_Z))) / 4
        + crate::terrain_base::sampling_mb()
        + crate::dem::generic::samplers_mb()
}

/// Deep pack `x`, `y`'s logical name (a z10 tile).
pub fn pack_logical(x: u32, y: u32) -> String {
    format!("{PREFIX}{PACK_Z}-{x}-{y}")
}

/// The deep packs' logical names' prefix.
pub const PREFIX: &str = "layers/terrain/deep/";

/// The terrain's packs of this band from before one terrain (task #633: JPEG XL, by z10 tile
/// alike), never made now: their z10 tile's next run drops them (`named_under`).
pub const RETIRED: &str = "layers/terrain/fine/";

/// A deep pack's z10 tile, from its logical name (or a retired pack's, `RETIRED`).
pub fn pack_tile(l: &str) -> Option<(u32, u32)> {
    let p = [PREFIX, RETIRED].into_iter().find(|p| l.starts_with(p))?;
    let u = crate::legacy::TileId::parse(&l[p.len()..]).filter(|u| u.z == PACK_Z)?;
    (format!("{p}{PACK_Z}-{}-{}", u.x, u.y) == l).then_some((u.x, u.y))
}

/// The deep packs the manifest names under tile `z/x/y` (z at most `PACK_Z`), and the retired
/// ones there (`RETIRED`).
pub fn named_under(m: &BTreeMap<String, String>, z: u8, x: u32, y: u32) -> Vec<String> {
    [PREFIX, RETIRED]
        .into_iter()
        .flat_map(|p| m.range(p.to_string()..).take_while(move |(l, _)| l.starts_with(p)))
        .filter(|(l, _)| pack_tile(l).is_some_and(|f| (f.0 >> (PACK_Z - z), f.1 >> (PACK_Z - z)) == (x, y)))
        .map(|(l, _)| l.clone())
        .collect()
}

/// The deep mids' logical names' prefix.
pub const MID_PREFIX: &str = "work/terrain-deep-mid/";

/// Z10 tile (`x`, `y`)'s deep mid: what its z6 tile's terrain-hi run takes in.
pub fn mid_logical(x: u32, y: u32) -> String {
    format!("{MID_PREFIX}{PACK_Z}-{x}-{y}")
}

/// A deep mid's z10 tile, from its logical name.
pub fn mid_tile(l: &str) -> Option<(u32, u32)> {
    let u = crate::legacy::TileId::parse(l.strip_prefix(MID_PREFIX)?).filter(|u| u.z == PACK_Z)?;
    (mid_logical(u.x, u.y) == l).then_some((u.x, u.y))
}

/// The deep mids the manifest names under z6 tile `t`, by z10 tile.
pub fn mids_under(m: &BTreeMap<String, String>, t: (u32, u32)) -> Vec<((u32, u32), String)> {
    let k = PACK_Z - 6;
    m.range(MID_PREFIX.to_string()..)
        .take_while(|(l, _)| l.starts_with(MID_PREFIX))
        .filter_map(|(l, c)| mid_tile(l).filter(|f| (f.0 >> k, f.1 >> k) == t).map(|f| (f, c.clone())))
        .collect()
}

/// What z6 tile `t`'s terrain-hi run takes in of its z10 tiles' targets (their deep mids, each read
/// and checked: crate::terrain_pack::read_mid_of): their z13 tiles' means (its z12 tiles' quarters
/// there).
pub fn read_mids(out: &Out, t: (u32, u32)) -> Result<crate::terrain_pack::Mid> {
    let mut mid = crate::terrain_pack::Mid::default();
    for (f, c) in mids_under(&out.manifest, t) {
        let (of, m) = crate::terrain_pack::read_mid_of(&out.path(&c), PACK_Z)?;
        anyhow::ensure!((of.1, of.2) == f, "{c} is {}/{}/{}'s deep mid", of.0, of.1, of.2);
        mid.quads.extend(m.quads);
    }
    Ok(mid)
}

/// What a deep target's run came to.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Report {
    pub tiles: usize,
    pub packs: usize,
    pub dropped: usize,
    pub bytes: u64,
}

/// Deep target `t` made, its levels `levels` (`levels`) from the tiles `raw` gives
/// (crate::terrain_pack::Fills over the pool) and sources `src`, and put: its deep pack (none where
/// it made no tile: the manifest's
/// dropped, a retired one there too) and its mid (`mid_logical`). Its levels' blocks kept as made
/// and an earlier run's taken (`keep`: crate::terrain_resume).
pub fn build_target(
    out: &mut Out,
    t: (u32, u32),
    levels: Vec<(u8, Vec<(u32, u32)>)>,
    raw: &dyn crate::terrain_pack::RawSource,
    src: &crate::terrain_pack::Sources,
    keep: bool,
) -> Result<Report> {
    let mut rep = Report::default();
    let p = crate::timings::phase("making its tiles, its finest zooms sampled, the coarser from them");
    let mut log = if keep {
        let inputs = crate::terrain_resume::inputs_read(out.root(), &out.manifest)?;
        let id = crate::terrain_resume::hi_id(&inputs, src, (PACK_Z, t.0, t.1), &levels, 4);
        crate::terrain_resume::HiLog::open(&out.scratch, (PACK_Z, t.0, t.1), &id)
    } else {
        None
    };
    let (made, mid) = crate::terrain_pack::make_piece_logged(raw, src, levels, log.as_mut())?;
    drop(p);
    let _p = crate::timings::phase("writing and uploading its deep pack and mid");
    rep.tiles = made.len();
    rep.bytes = made.iter().map(|m| m.3.len() as u64).sum();
    let mut it = made.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let pack = crate::layers::write_pack_local(
        out,
        "terrain",
        crate::terrain_codec::ENCODING,
        false,
        "deep",
        (PACK_Z, t.0, t.1),
        &mut it,
    )?;
    let mut gone = named_under(&out.manifest, PACK_Z, t.0, t.1);
    let ml = mid_logical(t.0, t.1);
    let mid_path = out.scratch_file(&format!("{ml}.sect"));
    crate::terrain_pack::write_mid_of(&mid_path, (PACK_Z, t.0, t.1), &mid)?;
    if let Some(p) = pack {
        gone.retain(|l| *l != p.logical);
        rep.packs = 1;
        out.put_file(&p.logical, "pack", &p.local)?;
    }
    rep.dropped = crate::layers::drop(out, &gone);
    out.put_file(&ml, "sect", &mid_path)?;
    std::fs::remove_file(crate::resume_log::path(&out.scratch, (PACK_Z, t.0, t.1))).ok();
    Ok(rep)
}

/// Z10 tile `t` no deep target any more (no place there past z12, or the coverage gone): its deep
/// pack, mid and a retired pack there dropped (not saved); how many.
pub fn drop_target(out: &mut Out, t: (u32, u32)) -> usize {
    let mut gone = named_under(&out.manifest, PACK_Z, t.0, t.1);
    gone.push(mid_logical(t.0, t.1));
    crate::layers::drop(out, &gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain_zoom::{Detail, Set};

    fn set(id: &str, cover: Option<Vec<[f64; 4]>>, cell: f64) -> Set {
        Set {
            id: id.into(),
            line: id.into(),
            cover: crate::dem::declared::Cover::of(cover.as_deref()),
            detail: Detail::Raster { bands: Vec::new(), all: cell },
            cut: None,
            every_zoom: false,
            presence: None,
        }
    }

    /// A z10 tile's band holds the tiles near the coverage no finer than their place's finest zoom,
    /// each under the coarser level's; a z10 tile no finer dataset meets has none and is no target.
    #[test]
    fn a_targets_levels_follow_each_places_finest_zoom() {
        let d = tempfile::tempdir().unwrap();
        let cov = crate::coverage::Coverage::fixture(
            &[crate::agent::recipes::Recipe {
                id: "r".into(),
                name: "R".into(),
                outline: vec!["place:10.5,45.5,5".into()],
            }],
            None,
            d.path(),
        )
        .unwrap();
        let fine = [10.0, 45.0, 11.0, 46.0];
        let sets = vec![set("fine", Some(vec![fine]), 2.0), set("world", None, 30.0)];
        let (x, y) = crate::terrain_zoom::tile_at(10.5, 45.5, PACK_Z);
        let lv = levels(&cov, &sets, (x, y));
        assert!(
            !lv.is_empty() && lv[0].0 > lv[lv.len() - 1].0,
            "finest first: {:?}",
            lv.iter().map(|l| l.0).collect::<Vec<_>>()
        );
        for w in lv.windows(2) {
            let (fz, fine_ts) = (&w[0].0, &w[0].1);
            for &(a, c) in fine_ts {
                assert!(w[1].1.contains(&(a >> 1, c >> 1)), "{fz}/{a}/{c} under a tile of the coarser level");
            }
        }
        assert!(is_target(&sets, &cov, (x, y)));
        let world = vec![set("world", None, 30.0)];
        assert!(levels(&cov, &world, (x, y)).is_empty() && !is_target(&world, &cov, (x, y)));
    }
}
