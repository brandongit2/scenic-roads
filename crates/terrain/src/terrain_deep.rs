//! The terrain pyramid's z13–16 band (docs/steps.md §2, Terrain), a z10 tile a job: the `terrain-deep`
//! step. Its tiles are made by the one rule every zoom follows (crate::terrain_pack: `Fill`): a tile
//! at its place's finest zoom (crate::terrain_zoom::finest, from the declared datasets meeting it)
//! is sampled from the pool, each dataset read at most at its own finest detail and the next in the
//! declared order filling its voids (crate::terrain_base); each coarser tile is its children's 2×2
//! means, a quarter whose child is past its place's finest zoom sampled there. It keeps the
//! source's heights as every level does: at sea, over lakes and below 0 m.
//!
//! - **Its tiles:** under the z10 tile, each of z13 to the deepest stored zoom near the coverage
//!   (`roadcore::geo::NEAR_COVERAGE_KM`) no finer than its place's finest zoom (`levels`). A z10
//!   tile with none is no target.
//! - **Packs:** by z10 tile (`PACK_Z`): `layers/terrain/deep/10-x-y` (docs/formats.md, Pack), z13 to
//!   `MAX_Z`; optional in a region's download (inputs::areafiles: "Terrain detail").
//! - **Its mid** (`mid_logical`, `work/terrain-deep-mid/10-x-y`): its z13 tiles' 2×2 means before
//!   rounding, which its z6 tile's terrain-hi run takes in (its z12 tiles' quarters there:
//!   crate::terrain_pack::build_piece): so the z6 tile's run waits for its z10 tiles' targets
//!   (agent::steps' declared reads), and a z6 tile with none waits for nothing.
//!
//! The band's edges are where its packs are (the hi packs end at z12, `FIRST_Z` − 1), a storage
//! layout the server and the downloads read; no dataset or zoom of the data is named here.

use anyhow::Result;
use buildstore::out::Out;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

// The zoom of a deep pack's tile (a job's target): inputs::dem::zoom's, kept at this path.
pub use inputs::dem::zoom::PACK_Z;
/// The band's first zoom: the hi packs' last (z12) and one.
pub const FIRST_Z: u8 = 13;
// (Past the lo packs' zooms: the readers look for a deep pack before the split's, store::catalogue.)
const _: () = assert!(FIRST_Z > store::catalogue::HI_FIRST_Z);
/// The band's last: the deepest zoom the map stores.
pub const MAX_Z: u8 = crate::terrain_zoom::DEEPEST;
/// What a deep target's key names of its layout: the band, its packs' zoom and the zoom rule's
/// cell factor (its code's versions are its step's: agent::build::TERRAIN_DEEP_V, the codec's).
pub fn versions() -> String {
    format!("band {FIRST_Z}-{MAX_Z} pack {PACK_Z} cell factor {}", crate::terrain_zoom::CELL_FACTOR)
}

/// Z10 tile `t`'s native zoom: the finest any of its tiles' places can have
/// (crate::terrain_zoom::finest_under; a place's finest zoom rises equatorward and past a band's
/// edge). The catalogue's `native_zoom` (by deep pack) and the map's zoom by area (crate::layers).
pub fn native_zoom(sets: &[crate::terrain_zoom::Set], t: (u32, u32)) -> u8 {
    crate::terrain_zoom::finest_under(sets, PACK_Z, t.0, t.1)
}

/// Z10 tile `t`'s tiles of the band, finest first (`MAX_Z` → `FIRST_Z`, from its finest zoom
/// holding one): those near the coverage (`roadcore::geo::NEAR_COVERAGE_KM`) no finer than their
/// place's finest zoom, whether or not their parent is one (a level can be empty, its run then
/// making nothing there: so each level is made from the one finer). A tile's place's finest zoom
/// can be past its parent's (crate::terrain_zoom::finest_under), so the tiles under one are asked
/// wherever its `finest_under` reaches theirs.
pub fn levels(
    cov: &buildstore::coverage::Coverage,
    sets: &[crate::terrain_zoom::Set],
    t: (u32, u32),
) -> Vec<(u8, Vec<(u32, u32)>)> {
    use crate::terrain_zoom::DEEPEST;
    let rows = crate::terrain_zoom::deepest_rows(PACK_Z, t.1);
    // (A tile's own rows, its part of its z10 tile's.)
    let under = |z: u8, x: u32, y: u32| {
        let k = DEEPEST - z;
        let r = ((y - (t.1 << (z - PACK_Z))) << k) as usize;
        crate::terrain_zoom::finest_over(sets, z, x, y, &rows[r..r + (1 << k)])
    };
    if under(PACK_Z, t.0, t.1) < FIRST_Z || !buildstore::coverage::near_coverage(cov, PACK_Z, t.0, t.1) {
        return Vec::new();
    }
    let cov = &buildstore::coverage::near_part(cov, PACK_Z, t.0, t.1);
    let mut out: Vec<(u8, Vec<(u32, u32)>)> = Vec::new();
    let mut above: Vec<(u32, u32)> = vec![t];
    for z in PACK_Z + 1..=MAX_Z {
        let (mut here, mut next) = (Vec::new(), Vec::new());
        for (x, y) in above.iter().flat_map(|&(x, y)| (0..4u32).map(move |k| (2 * x + (k & 1), 2 * y + (k >> 1)))) {
            let made = z >= FIRST_Z && z <= crate::terrain_zoom::finest(sets, z, x, y);
            let deeper = z < MAX_Z && under(z, x, y) > z.max(FIRST_Z - 1);
            if (made || deeper) && buildstore::coverage::near_coverage(cov, z, x, y) {
                if made {
                    here.push((x, y));
                }
                if deeper {
                    next.push((x, y));
                }
            }
        }
        if z >= FIRST_Z {
            out.push((z, here));
        }
        if next.is_empty() {
            break;
        }
        above = next;
    }
    while out.last().is_some_and(|l| l.1.is_empty()) {
        out.pop();
    }
    out.reverse();
    out
}

/// Whether z10 tile `t` is a terrain-deep target: it has tiles of the band (`levels`).
pub fn is_target(sets: &[crate::terrain_zoom::Set], cov: &buildstore::coverage::Coverage, t: (u32, u32)) -> bool {
    !levels(cov, sets, t).is_empty()
}

/// The terrain-deep targets under z6 tile `t` (`is_target`).
pub fn targets_in(
    sets: &[crate::terrain_zoom::Set],
    cov: &buildstore::coverage::Coverage,
    t: (u32, u32),
) -> Vec<(u32, u32)> {
    targets_under(sets, t, |u| is_target(sets, cov, u))
}

/// `targets_in`, each z10 tile under z6 tile `t` told by `is_target` (`Deep`'s, kept); none where
/// no tile under it can reach the band (crate::terrain_zoom::finest_under).
pub fn targets_under(
    sets: &[crate::terrain_zoom::Set],
    t: (u32, u32),
    is_target: impl Fn((u32, u32)) -> bool,
) -> Vec<(u32, u32)> {
    let k = PACK_Z - 6;
    if crate::terrain_zoom::finest_under(sets, 6, t.0, t.1) < FIRST_Z {
        return Vec::new();
    }
    (t.0 << k..(t.0 + 1) << k)
        .flat_map(|x| (t.1 << k..(t.1 + 1) << k).map(move |y| (x, y)))
        .filter(|&u| is_target(u))
        .collect()
}

/// Levels' tiles (z, then each column and row), as a key names them: what a terrain-deep z10
/// tile's run reads of the coverage, the tiles of its `levels` (`Deep::coverage`); a terrain z6
/// tile's, its (crate::terrain_pack::piece_levels: `Deep::piece`).
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
    /// By z6 tile: what its terrain run reads of the coverage (`Deep::piece`).
    pieces: Mutex<HashMap<(u32, u32), Arc<str>>>,
}

/// The terrain-deep targets and the coverage their keys name, of a coverage and the declared sets:
/// each z10 tile's `levels` (a polygon test on each of its z11–16 tiles: ~85% of a plan's time,
/// worked out per target and twice a plan before task #965) worked out once for both, kept as
/// long as its `Memo` is (`Kept`: the agent's, agent::tiles::TerrainTiles::deep).
pub struct Deep<'a> {
    pub cov: &'a buildstore::coverage::Coverage,
    pub sets: Arc<Vec<crate::terrain_zoom::Set>>,
    memo: Arc<Memo>,
}

impl<'a> Deep<'a> {
    /// Kept for as long as it lives (a job's: no agent's `Kept`).
    pub fn new(cov: &'a buildstore::coverage::Coverage, sets: Arc<Vec<crate::terrain_zoom::Set>>) -> Deep<'a> {
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
        let v = (!lv.is_empty(), (!lv.is_empty()).then(|| levels_digest(&lv).into()));
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

    /// What terrain z6 tile `t`'s run reads of the coverage (task #832), as its keys name it: the
    /// tiles of its levels it makes (crate::terrain_pack::piece_levels, each z9–12 tile within 20
    /// km of it), digested; worked out once (task #1457: every terrain target's again each time
    /// a terrain or slope job ended). Not the coverage's rings near it (`Coverage::fingerprint`):
    /// a region's outline moved, an islet dropped or added, that leaves the same tiles near it
    /// changes nothing the run reads, so it rebuilds nothing.
    pub fn piece(&self, t: (u32, u32)) -> Arc<str> {
        if let Some(v) = self.memo.pieces.lock().unwrap_or_else(|e| e.into_inner()).get(&t) {
            return v.clone();
        }
        let v: Arc<str> = levels_digest(&crate::terrain_pack::piece_levels(self.cov, &self.sets, t)).into();
        self.memo.pieces.lock().unwrap_or_else(|e| e.into_inner()).insert(t, v.clone());
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

/// `Deep`'s answers kept by the coverage's version (buildstore::coverage::Coverage::version) and the
/// declared sets they were worked out of (held, so told apart by their pointer), the latest `KEPT`.
#[derive(Default)]
pub struct Kept(Mutex<Vec<(String, Arc<Vec<crate::terrain_zoom::Set>>, Arc<Memo>)>>);

impl Kept {
    /// The answers of coverage `cov` and the declared sets `sets`, kept: the same memo for every
    /// asker while neither changes.
    pub fn get<'a>(
        &self,
        cov: &'a buildstore::coverage::Coverage,
        sets: Arc<Vec<crate::terrain_zoom::Set>>,
    ) -> Deep<'a> {
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
    let u = codecs::tile::TileId::parse(&l[p.len()..]).filter(|u| u.z == PACK_Z)?;
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
    let u = codecs::tile::TileId::parse(l.strip_prefix(MID_PREFIX)?).filter(|u| u.z == PACK_Z)?;
    (mid_logical(u.x, u.y) == l).then_some((u.x, u.y))
}

/// The deep mids the manifest names under z6 tile `t`, by z10 tile.
pub fn mids_under(m: &BTreeMap<String, String>, t: (u32, u32)) -> Vec<((u32, u32), String)> {
    named_z10_under(m, t, mid_logical)
}

/// The z10 tiles under z6 tile `t` whose file `logical` names (`logical(x, y)`) the manifest
/// holds, with its content, in the manifest's order: each of the z6 tile's 256 looked up, never
/// every such name of the manifest read (task #1457: every z6 tile's key read them all).
pub fn named_z10_under(
    m: &BTreeMap<String, String>,
    t: (u32, u32),
    logical: impl Fn(u32, u32) -> String,
) -> Vec<((u32, u32), String)> {
    let k = PACK_Z - 6;
    let mut v: Vec<(String, (u32, u32), String)> = (t.0 << k..(t.0 + 1) << k)
        .flat_map(|x| (t.1 << k..(t.1 + 1) << k).map(move |y| (x, y)))
        .filter_map(|(x, y)| {
            let l = logical(x, y);
            let c = m.get(&l)?.clone();
            Some((l, (x, y), c))
        })
        .collect();
    v.sort_unstable();
    v.into_iter().map(|(_, f, c)| (f, c)).collect()
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
/// dropped, a retired one there too) and its mid (`mid_logical`), their uploads put off
/// (buildstore::out::Out::put_file_later: they go on beside what the job does next, the target
/// saved by a later save). Its levels' blocks kept as made and an earlier run's taken (`keep`:
/// crate::terrain_resume). Says how far it is to `progress` (its tiles, every few seconds).
pub fn build_target(
    out: &mut Out,
    t: (u32, u32),
    levels: Vec<(u8, Vec<(u32, u32)>)>,
    raw: &dyn crate::terrain_pack::RawSource,
    src: &crate::terrain_pack::Sources,
    keep: bool,
    progress: buildstore::rawpack::Progress,
) -> Result<Report> {
    let mut rep = Report::default();
    let p = jobrt::timings::phase("making its tiles, its finest zooms sampled, the coarser from them");
    let mut log = if keep {
        let inputs = crate::terrain_resume::inputs_read(out.root(), &out.manifest)?;
        let id = crate::terrain_resume::hi_id(&inputs, src, (PACK_Z, t.0, t.1), &levels, 4);
        crate::terrain_resume::HiLog::open(&out.scratch, (PACK_Z, t.0, t.1), &id)
    } else {
        None
    };
    let (made, mid) = crate::terrain_pack::make_piece_logged(raw, src, levels, log.as_mut(), progress)?;
    drop(p);
    let _p = jobrt::timings::phase("writing its deep pack and mid");
    rep.tiles = made.len();
    rep.bytes = made.iter().map(|m| m.3.len() as u64).sum();
    let (pack, mid_path) = write_local(out, t, made, &mid)?;
    let mut gone = named_under(&out.manifest, PACK_Z, t.0, t.1);
    let ml = mid_logical(t.0, t.1);
    if let Some(p) = pack {
        gone.retain(|l| *l != p.logical);
        rep.packs = 1;
        out.put_file_later(&p.logical, "pack", &p.local)?;
    }
    rep.dropped = crate::layers::drop(out, &gone);
    out.put_file_later(&ml, "sect", &mid_path)?;
    std::fs::remove_file(jobrt::resume_log::path(&out.scratch, (PACK_Z, t.0, t.1))).ok();
    Ok(rep)
}

/// Deep target `t`'s tiles `made` and mid `mid` written in the scratch folder, not put: its deep
/// pack (None: no tile) and its mid's file (`build_target`; `scenic-build terrain-same 10/x/y`).
pub fn write_local(
    out: &Out,
    t: (u32, u32),
    made: Vec<crate::terrain_pack::Made>,
    mid: &crate::terrain_pack::Mid,
) -> Result<(Option<crate::layers::LocalPack>, std::path::PathBuf)> {
    let mut it = made.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let pack = crate::layers::write_pack_local(
        out,
        "terrain",
        codecs::terrain_codec::ENCODING,
        false,
        "deep",
        (PACK_Z, t.0, t.1),
        &mut it,
    )?;
    let mid_path = out.scratch_file(&format!("{}.sect", mid_logical(t.0, t.1)));
    crate::terrain_pack::write_mid_of(&mid_path, (PACK_Z, t.0, t.1), mid)?;
    Ok((pack, mid_path))
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
            cover: inputs::dem::declared::Cover::of(cover.as_deref()),
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
        let cov = buildstore::coverage::Coverage::fixture(
            &[buildstore::recipes::Recipe {
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

    /// A coverage of one place: a circle of `km` about `lon`, `lat`.
    fn place(d: &std::path::Path, lon: f64, lat: f64, km: f64) -> buildstore::coverage::Coverage {
        let r = buildstore::recipes::Recipe {
            id: "r".into(),
            name: "R".into(),
            outline: vec![format!("place:{lon},{lat},{km}")],
        };
        buildstore::coverage::Coverage::fixture(&[r], None, d).unwrap()
    }

    /// A raster of cell `cell` (m) south of latitude `edge` (to the equator), of cell `all` elsewhere.
    fn banded(id: &str, cover: Option<Vec<[f64; 4]>>, edge: f64, cell: f64, all: f64) -> Set {
        let bands = vec![inputs::dem::declared::Band { south: 0.0, north: edge, cell }];
        Set { detail: Detail::Raster { bands, all }, ..set(id, cover, all) }
    }

    /// The band's tiles of z10 tile `t` by the definition alone, every one of them asked: each
    /// near the coverage no finer than its place's finest zoom, as sets by zoom, finest first.
    fn every(
        cov: &buildstore::coverage::Coverage,
        sets: &[Set],
        t: (u32, u32),
    ) -> Vec<(u8, std::collections::BTreeSet<(u32, u32)>)> {
        let mut out: Vec<(u8, std::collections::BTreeSet<(u32, u32)>)> = (FIRST_Z..=MAX_Z)
            .map(|z| {
                let k = z - PACK_Z;
                let ts = (t.0 << k..(t.0 + 1) << k)
                    .flat_map(|x| (t.1 << k..(t.1 + 1) << k).map(move |y| (x, y)))
                    .filter(|&(x, y)| {
                        buildstore::coverage::near_coverage(cov, z, x, y)
                            && z <= crate::terrain_zoom::finest(sets, z, x, y)
                    })
                    .collect();
                (z, ts)
            })
            .collect();
        while out.last().is_some_and(|l| l.1.is_empty()) {
            out.pop();
        }
        out.reverse();
        out
    }

    fn as_sets(lv: &[(u8, Vec<(u32, u32)>)]) -> Vec<(u8, std::collections::BTreeSet<(u32, u32)>)> {
        lv.iter().map(|(z, ts)| (*z, ts.iter().copied().collect())).collect()
    }

    /// Tile `z/x/y`'s centre (lon, lat).
    fn centre(z: u8, x: u32, y: u32) -> (f64, f64) {
        let b = roadcore::geo::tile_box(z, x, y);
        ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0)
    }

    /// Pixels' size at zoom `z` and latitude `lat` (m).
    fn px(z: u8, lat: f64) -> f64 {
        inputs::dem::generic::tile_resolution(z as u32, lat)
    }

    /// Task #1461: a place's finest zoom rises equatorward (taken at a tile's poleward edge), so a
    /// z10 tile whose poleward edge is past a raster's threshold latitude still has tiles of the
    /// band further south: they're its levels, it's a target, and its z6 tile has it.
    #[test]
    fn a_target_straddling_a_rasters_threshold_has_its_southern_tiles() {
        let d = tempfile::tempdir().unwrap();
        let t = crate::terrain_zoom::tile_at(10.5, 58.3, PACK_Z);
        let b = roadcore::geo::tile_box(PACK_Z, t.0, t.1);
        let cell = (px(FIRST_Z, b[3]) + px(FIRST_Z, b[1])) / 2.0;
        let sets = vec![set("fine", Some(vec![[9.0, 57.0, 12.0, 60.0]]), cell), set("world", None, 30.0)];
        assert!(sets[0].detail.zoom_at(b[3]) < FIRST_Z && sets[0].detail.zoom_at(b[1]) >= FIRST_Z);
        let c = centre(PACK_Z, t.0, t.1);
        let cov = place(d.path(), c.0, c.1, 30.0);
        let lv = levels(&cov, &sets, t);
        let want = every(&cov, &sets, t);
        assert!(want.iter().any(|l| l.0 == FIRST_Z && !l.1.is_empty()), "z13 tiles south of the threshold");
        assert_eq!(as_sets(&lv), want);
        assert!(is_target(&sets, &cov, t));
        let t6 = (t.0 >> (PACK_Z - 6), t.1 >> (PACK_Z - 6));
        assert!(targets_in(&sets, &cov, t6).contains(&t), "its z6 tile's target");
        assert!(native_zoom(&sets, t) >= FIRST_Z, "the map's zoom there reaches its tiles");
    }

    /// Task #1461: a band's edge inside a tile lifts its place's finest zoom by more than one
    /// zoom past it (a z14 tile across the edge is its coarse cell's z13, its z15 child south of it
    /// the fine band's z16): that child and those under it are levels, though their parent isn't.
    #[test]
    fn tiles_past_a_band_edge_are_levels_under_a_parent_that_isnt() {
        let d = tempfile::tempdir().unwrap();
        let t = crate::terrain_zoom::tile_at(10.5, 45.5, PACK_Z);
        let b = roadcore::geo::tile_box(PACK_Z, t.0, t.1);
        // (The edge a quarter of the way up a z14 tile's southern z15 child's northern one.)
        let y14 = (t.1 << 4) + 5;
        let n14 = roadcore::geo::tile_box(14, t.0 << 4, y14)[3];
        let mid = roadcore::geo::tile_box(15, t.0 << 5, 2 * y14 + 1)[3];
        let edge = mid + (n14 - mid) * 0.25;
        let sets = vec![
            banded("fine", Some(vec![[10.0, 45.0, 11.0, 46.0]]), edge, 1.0, px(FIRST_Z, b[3])),
            set("world", None, 30.0),
        ];
        assert_eq!(crate::terrain_zoom::finest(&sets, PACK_Z, t.0, t.1), FIRST_Z);
        assert_eq!(crate::terrain_zoom::finest(&sets, 14, t.0 << 4, y14), FIRST_Z);
        assert_eq!(crate::terrain_zoom::finest(&sets, 15, t.0 << 5, 2 * y14 + 1), MAX_Z);
        let c = centre(PACK_Z, t.0, t.1);
        let cov = place(d.path(), c.0, c.1, 40.0);
        let lv = levels(&cov, &sets, t);
        assert_eq!(as_sets(&lv), every(&cov, &sets, t));
        let z15 = lv.iter().find(|l| l.0 == 15).unwrap();
        assert!(z15.1.contains(&(t.0 << 5, 2 * y14 + 1)), "the child south of the edge");
        assert_eq!(native_zoom(&sets, t), MAX_Z, "the map's zoom there reaches its tiles");
    }

    /// Task #1461: a z10 tile whose z13 tiles all lie across a band's edge, its fine band only in
    /// its z14 tiles south of it: a target whose z13 level is empty, so its mid holds no z14 tile's
    /// means taken for a z13 tile's (the run makes its levels in turn, each from the one finer).
    #[test]
    fn a_target_with_no_z13_tile_makes_its_finer_tiles_and_an_empty_mid() {
        let d = tempfile::tempdir().unwrap();
        let t = crate::terrain_zoom::tile_at(10.5, 45.5, PACK_Z);
        let b = roadcore::geo::tile_box(PACK_Z, t.0, t.1);
        let y13 = (t.1 << 3) + 7;
        let n13 = roadcore::geo::tile_box(FIRST_Z, t.0 << 3, y13)[3];
        let mid = roadcore::geo::tile_box(14, t.0 << 4, 2 * y13 + 1)[3];
        let edge = mid + (n13 - mid) * 0.25;
        let sets = vec![
            banded("fine", Some(vec![[10.0, 45.0, 11.0, 46.0]]), edge, 1.0, px(12, b[3])),
            set("world", None, 30.0),
        ];
        assert!(crate::terrain_zoom::finest(&sets, PACK_Z, t.0, t.1) < FIRST_Z);
        let c = centre(PACK_Z, t.0, t.1);
        let cov = place(d.path(), c.0, c.1, 40.0);
        let lv = levels(&cov, &sets, t);
        assert_eq!(as_sets(&lv), every(&cov, &sets, t));
        assert_eq!(lv.iter().map(|l| l.0).collect::<Vec<_>>(), [16, 15, 14, 13]);
        assert!(lv[3].1.is_empty() && !lv[2].1.is_empty());
        assert!(is_target(&sets, &cov, t));
        struct Flat;
        impl crate::terrain_pack::RawSource for Flat {
            fn get(&self, _: u8, _: u32, _: u32) -> Result<(Option<Vec<u8>>, bool)> {
                Ok((Some(roadcore::grid::encode_terrain_png(&vec![5.0; 65536], 256, 256)?), false))
            }
            fn prefetch_each(
                &self,
                _: u8,
                tiles: &[(u32, u32)],
                _: &crate::terrain_pack::Turns,
                ready: crate::terrain_pack::Ready,
            ) -> Result<usize> {
                ready(tiles);
                Ok(0)
            }
        }
        let src = crate::terrain_pack::Sources { native: None, sets: &sets };
        let (made, mid) = crate::terrain_pack::make_piece(&Flat, &src, lv.clone()).unwrap();
        assert_eq!(made.len(), lv.iter().map(|l| l.1.len()).sum::<usize>());
        assert!(mid.quads.is_empty(), "no z13 tile: nothing for its z12 tiles' quarters");
    }

    /// Task #1461: a z6 tile whose z9 tiles' places all stop short of z9 at their poleward edges,
    /// with z10 tiles past a band's edge reaching z10, makes hi tiles (`makes_hi`, as its levels).
    #[test]
    fn a_z6_tile_makes_hi_tiles_where_only_finer_tiles_reach_their_zoom() {
        let d = tempfile::tempdir().unwrap();
        let t6 = crate::terrain_zoom::tile_at(10.5, 45.5, 6);
        let b = roadcore::geo::tile_box(6, t6.0, t6.1);
        let y9 = (t6.1 << 3) + 7;
        let n9 = roadcore::geo::tile_box(9, t6.0 << 3, y9)[3];
        let mid = roadcore::geo::tile_box(PACK_Z, t6.0 << 4, 2 * y9 + 1)[3];
        let edge = mid + (n9 - mid) * 0.25;
        let sets = vec![banded("only", None, edge, 30.0, px(8, b[3]))];
        let c = centre(PACK_Z, (t6.0 << 4) + 8, 2 * y9 + 1);
        let cov = place(d.path(), c.0, c.1, 10.0);
        assert!((t6.0 << 3..(t6.0 + 1) << 3).all(|x| crate::terrain_zoom::finest(&sets, 9, x, y9) < 9));
        let lv = crate::terrain_pack::piece_levels(&cov, &sets, t6);
        assert!(lv.iter().any(|(z, ts)| *z == PACK_Z && !ts.is_empty()), "{lv:?}");
        assert!(crate::terrain_pack::makes_hi(&cov, &sets, t6));
    }
}
