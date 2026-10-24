//! The slope layer (docs/steps.md §2, Slope): the terrain's shading pyramid (roadcore::shade, stored
//! by crate::shade_codec), which the map's slope tint and hill-shading both read. At each place's
//! finest terrain level, every pixel's steepness and direction by Horn's method (with the
//! neighbouring tiles); each coarser level the mean of the four pixels beneath it, or where no finer
//! level covers a pixel, its own level's terrain's. Every level is stored: z0–2 in the root
//! (`build_root`), z3–8 by z3 tile (lo: `build_lo`, from the z6 tiles' mids), z9–12 by z6 tile (hi)
//! and the deep levels (z13–16, where the terrain has them: crate::terrain_deep) by z10 tile (deep:
//! `deep_logical`), the last two made by a z6 tile's run (`build_piece`) from the build's terrain
//! packs.

use crate::out::Out;
use crate::terrain_deep::{MAX_Z, PACK_Z};
use crate::terrain_pack::ManifestTiles;
use anyhow::Result;
use det::Det;
use rayon::prelude::*;
use roadcore::grid::{decode_terrain, upsampled};
use roadcore::shade::{quadrant, Px};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

const TS: usize = 256;
/// The hi packs' finest zoom (the base terrain's: crate::terrain_pack::max_zoom_at).
pub const HI_MAX: u8 = 12;
/// Decoded terrain tiles kept (256 KB each): a slope tile reads its own and its four neighbours',
/// which the depth-first build reaches close together.
const KEEP: usize = 512;
/// The deep packs' logical names' prefix.
pub const DEEP_PREFIX: &str = "layers/slope/deep/";

/// A tile kept: its z/x/y and how many levels up the tile it was made from is (0: its own).
type Key = (u8, u32, u32, u8);
/// A tile in metres; None when its bytes don't decode.
type Tile = Option<Arc<Vec<f32>>>;
/// A kept tile, made once by whichever thread asks first; None when its bytes couldn't be read.
type Slot = Arc<OnceLock<Option<Tile>>>;

/// A job's terrain tiles in metres, each decoded once while it's in use (the `KEEP` most recently
/// read are kept): a missing tile is its nearest ancestor's, upsampled as roadcore::grid's
/// tile_with_fallback_by makes it, and that ancestor is decoded once for all its descendants.
pub struct Terrain<'a> {
    get: &'a (dyn Fn(u8, u32, u32) -> Option<Vec<u8>> + Sync),
    has: &'a (dyn Fn(u8, u32, u32) -> bool + Sync),
    kept: Mutex<Kept>,
}

/// The tiles kept, with the time each was last read, and their keys by those times.
#[derive(Default)]
struct Kept {
    slots: HashMap<Key, (Slot, u64)>,
    by_time: BTreeMap<u64, Key>,
    now: u64,
}

impl<'a> Terrain<'a> {
    /// `get` reads a terrain tile, `has` says whether there is one.
    pub fn new(
        get: &'a (dyn Fn(u8, u32, u32) -> Option<Vec<u8>> + Sync),
        has: &'a (dyn Fn(u8, u32, u32) -> bool + Sync),
    ) -> Self {
        Terrain { get, has, kept: Mutex::new(Kept::default()) }
    }

    /// Tile (z, x, y), or a missing one from its nearest ancestor up to eight levels up; None when
    /// there's none, or the tile there doesn't decode.
    pub fn tile(&self, z: u8, x: u32, y: u32) -> Tile {
        if (self.has)(z, x, y) {
            if let Some(t) = self.slot((z, x, y, 0), || self.decode(z, x, y)) {
                return t;
            }
        }
        for dz in 1..=z.min(8) {
            let (pz, px, py) = (z - dz, x >> dz, y >> dz);
            if !(self.has)(pz, px, py) {
                continue;
            }
            let Some(p) = self.slot((pz, px, py, 0), || self.decode(pz, px, py)) else { continue };
            let p = p?;
            return self.slot((z, x, y, dz), || Some(Some(Arc::new(upsampled(&p, dz, x, y))))).flatten();
        }
        None
    }

    /// A tile's own: None when its bytes can't be read (a missing tile, to tile_with_fallback_by).
    fn decode(&self, z: u8, x: u32, y: u32) -> Option<Tile> {
        (self.get)(z, x, y).map(|b| decode_terrain(&b).ok().map(Arc::new))
    }

    /// `k`'s tile, made by `make` unless it's kept.
    fn slot(&self, k: Key, make: impl FnOnce() -> Option<Tile>) -> Option<Tile> {
        let s = {
            let mut kept = self.kept.lock().unwrap();
            let Kept { slots, by_time, now } = &mut *kept;
            *now += 1;
            let s = match slots.get_mut(&k) {
                Some((s, t)) => {
                    by_time.remove(t);
                    *t = *now;
                    s.clone()
                }
                None => {
                    let s = Slot::default();
                    slots.insert(k, (s.clone(), *now));
                    s
                }
            };
            by_time.insert(*now, k);
            while slots.len() > KEEP {
                let (_, old) = by_time.pop_first().unwrap();
                slots.remove(&old);
            }
            s
        };
        let v = s.get_or_init(make).clone();
        if v.is_none() {
            // Unreadable: forgotten, so it's read again next time.
            let mut kept = self.kept.lock().unwrap();
            if kept.slots.get(&k).is_some_and(|(o, _)| Arc::ptr_eq(o, &s)) {
                let (_, t) = kept.slots.remove(&k).unwrap();
                kept.by_time.remove(&t);
            }
        }
        v
    }
}

/// The shading (roadcore::shade's pixels) of terrain tile (z, x, y), by Horn's method with its
/// edge neighbours.
pub fn shade_tile(terrain: &Terrain, z: u8, x: u32, y: u32) -> Option<Vec<Px>> {
    horn(&|x, y| terrain.tile(z, x, y), z, x, y)
}

/// The shading of tile (z, x, y) by Horn's method from `tile`'s heights of the tiles at its zoom
/// (its own and its edge neighbours'; past a missing neighbour, its own edge): each pixel's
/// gradient (east, south) as its steepness and direction (roadcore::shade::Px::of_gradient). The
/// slope's finest level, and the depth's (crate::depth::build).
pub fn horn(tile: &dyn Fn(u32, u32) -> Option<Arc<Vec<f32>>>, z: u8, x: u32, y: u32) -> Option<Vec<Px>> {
    let e = tile(x, y)?;
    let n = 1u32 << z;
    let nb = |dx: i64, dy: i64| -> Option<Arc<Vec<f32>>> {
        let nx = (x as i64 + dx).rem_euclid(n as i64) as u32;
        let ny = y as i64 + dy;
        if ny < 0 || ny >= n as i64 {
            return None;
        }
        tile(nx, ny as u32)
    };
    let (west, east, north, south) = (nb(-1, 0), nb(1, 0), nb(0, -1), nb(0, 1));
    let (e, west, east, north, south) = (
        &e[..],
        west.as_deref().map(|v| &v[..]),
        east.as_deref().map(|v| &v[..]),
        north.as_deref().map(|v| &v[..]),
        south.as_deref().map(|v| &v[..]),
    );
    let at = |i: i32, j: i32| -> f32 {
        let (ci, cj) = (i.clamp(0, 255), j.clamp(0, 255));
        let pick = |t: Option<&[f32]>, ii: i32, jj: i32| t.map(|v| v[(jj * 256 + ii) as usize]);
        let v = if i < 0 {
            pick(west, 255, cj)
        } else if i > 255 {
            pick(east, 0, cj)
        } else if j < 0 {
            pick(north, ci, 255)
        } else if j > 255 {
            pick(south, ci, 0)
        } else {
            None
        };
        v.unwrap_or(e[(cj * 256 + ci) as usize])
    };
    // The tile with the one-pixel border Horn's method reads, each value taken once.
    const W: usize = TS + 2;
    let mut p = vec![0f32; W * W];
    for j in -1..=256i32 {
        for i in -1..=256i32 {
            p[(j + 1) as usize * W + (i + 1) as usize] = at(i, j);
        }
    }
    let world = 40_075_016.686f64;
    let mut out = vec![Px::default(); TS * TS];
    for j in 0..256usize {
        let yy = (y as f64 + (j as f64 + 0.5) / 256.0) / n as f64;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * yy)).dsinh().datan();
        let d = (world * lat.dcos() / (256.0 * n as f64)) as f32;
        let (r0, r1, r2) = (&p[j * W..(j + 1) * W], &p[(j + 1) * W..(j + 2) * W], &p[(j + 2) * W..(j + 3) * W]);
        for i in 0..256usize {
            let (a, b, c) = (r0[i], r0[i + 1], r0[i + 2]);
            let (dd, f) = (r1[i], r1[i + 2]);
            let (g, h, k) = (r2[i], r2[i + 1], r2[i + 2]);
            let dzdx = ((c + 2.0 * f + k) - (a + 2.0 * dd + g)) / (8.0 * d);
            let dzdy = ((g + 2.0 * h + k) - (a + 2.0 * b + c)) / (8.0 * d);
            out[j * 256 + i] = Px::of_gradient(dzdx, dzdy);
        }
    }
    Some(out)
}

/// A tile's pixels: its children's quadrants where they cover it (`kids`, by column and row), else
/// `own` (its own level's: called only when they don't cover it all). None with neither.
pub fn compose(kids: &[((u32, u32), Vec<Px>)], own: impl FnOnce() -> Option<Vec<Px>>) -> Option<Vec<Px>> {
    let mut val = vec![Px::default(); TS * TS];
    let mut has = vec![false; TS * TS];
    for ((cx, cy), q) in kids {
        let (ox, oy) = ((cx % 2) as usize * 128, (cy % 2) as usize * 128);
        for j in 0..128 {
            let row = (oy + j) * TS + ox;
            val[row..row + 128].copy_from_slice(&q[j * 128..(j + 1) * 128]);
            has[row..row + 128].iter_mut().for_each(|h| *h = true);
        }
    }
    let direct = if has.iter().all(|&h| h) { None } else { own() };
    if kids.is_empty() && direct.is_none() {
        return None;
    }
    Some(
        (0..TS * TS)
            .map(|p| if has[p] { val[p] } else { direct.as_ref().map_or_else(Px::default, |d| d[p]) })
            .collect(),
    )
}

/// How far a slope build is: its tiles worked out, said at most once a second.
struct Count<'a> {
    done: std::sync::atomic::AtomicU64,
    total: u64,
    said: std::sync::Mutex<std::time::Instant>,
    on: &'a (dyn Fn(&str, u64, u64) + Sync),
}

impl Count<'_> {
    fn new(total: u64, on: &(dyn Fn(&str, u64, u64) + Sync)) -> Count<'_> {
        Count { done: Default::default(), total, said: std::sync::Mutex::new(std::time::Instant::now()), on }
    }

    fn one(&self) {
        let d = self.done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if let Ok(mut t) = self.said.try_lock() {
            if t.elapsed() >= std::time::Duration::from_secs(1) || d == self.total {
                *t = std::time::Instant::now();
                (self.on)("slope tiles worked out", d.min(self.total), self.total);
            }
        }
    }
}

/// A tile as made: zoom, column, row, its bytes.
pub type Made = (u8, u32, u32, Vec<u8>);

/// A shading pyramid's build (the slope's and the depth's: crate::depth::build): each tile after
/// its children among `tiles` (depth first, the children in parallel), its pixels their quadrants
/// where they cover it (`compose`), else its own level's (`own`), stored in `out`; its quadrant
/// for its parent. A tile in `done` isn't made: its quadrant as made before.
pub struct Pyramid<'a> {
    pub own: &'a (dyn Fn(u8, u32, u32) -> Option<Vec<Px>> + Sync),
    pub tiles: &'a HashSet<(u8, u32, u32)>,
    pub out: &'a Mutex<Vec<Made>>,
    pub tick: &'a (dyn Fn() + Sync),
    pub done: &'a HashMap<(u8, u32, u32), Option<Vec<Px>>>,
}

impl Pyramid<'_> {
    /// Tile (z, x, y) and those under it among `tiles` (none past `MAX_Z`); its quadrant.
    pub fn tile(&self, z: u8, x: u32, y: u32) -> Option<Vec<Px>> {
        if let Some(q) = self.done.get(&(z, x, y)) {
            return q.clone();
        }
        let kids: Vec<((u32, u32), Vec<Px>)> = if z < MAX_Z {
            (0..4u32)
                .map(|k| (2 * x + (k & 1), 2 * y + (k >> 1)))
                .filter(|&(cx, cy)| self.tiles.contains(&(z + 1, cx, cy)))
                .collect::<Vec<_>>()
                .into_par_iter()
                .filter_map(|(cx, cy)| self.tile(z + 1, cx, cy).map(|q| ((cx, cy), q)))
                .collect()
        } else {
            Vec::new()
        };
        (self.tick)();
        let v = compose(&kids, || (self.own)(z, x, y))?;
        let (blob, q) = stored(&v);
        self.out.lock().unwrap().push((z, x, y, blob));
        (z > 0).then_some(q)
    }
}

/// A tile as stored and its quadrant for its parent, from its pixels as read back
/// (crate::shade_codec::stored).
pub fn stored(v: &[Px]) -> (Vec<u8>, Vec<Px>) {
    let (blob, v) = crate::shade_codec::stored(v).expect("a tile's WebP");
    (blob, quadrant(&v))
}

/// What a slope run made.
#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub hi_tiles: usize,
    pub lo_tiles: usize,
    pub deep_tiles: usize,
    pub deep_packs: usize,
    pub deep_dropped: usize,
}

/// Z6 tile (`x`, `y`)'s slope hi pack in the manifest (z9–12).
pub fn hi_logical(x: u32, y: u32) -> String {
    format!("layers/slope/hi/6-{x}-{y}")
}

/// Deep pack (`x`, `y`)'s logical name (a z10 tile: its z13–16 tiles).
pub fn deep_logical(x: u32, y: u32) -> String {
    format!("{DEEP_PREFIX}{PACK_Z}-{x}-{y}")
}

/// A deep pack's z10 tile, from its logical name.
pub fn deep_tile(l: &str) -> Option<(u32, u32)> {
    let u = crate::legacy::TileId::parse(l.strip_prefix(DEEP_PREFIX)?).filter(|u| u.z == PACK_Z)?;
    (deep_logical(u.x, u.y) == l).then_some((u.x, u.y))
}

/// The deep packs the manifest names under z6 tile `t`.
pub fn deep_under(m: &BTreeMap<String, String>, t: (u32, u32)) -> Vec<String> {
    let k = PACK_Z - 6;
    m.range(DEEP_PREFIX.to_string()..)
        .take_while(|(l, _)| l.starts_with(DEEP_PREFIX))
        .filter(|(l, _)| deep_tile(l).is_some_and(|f| (f.0 >> k, f.1 >> k) == t))
        .map(|(l, _)| l.clone())
        .collect()
}

/// Drops z6 tile `t`'s slope hi pack, deep packs and mid from the manifest (a z6 tile the coverage
/// has left: docs/regions.md, Shrinking), saving it; how many it had.
pub fn drop_piece(out: &mut Out, t: (u32, u32)) -> Result<usize> {
    let mut gone = vec![hi_logical(t.0, t.1), mid_logical(t.0, t.1)];
    gone.extend(deep_under(&out.manifest, t));
    let n = crate::terrain_pack::drop_named(out, &gone);
    out.save()?;
    Ok(n)
}

/// Drops z3 tile `q`'s slope lo pack from the manifest (a "none" assembly: the coverage has left
/// the z3 tile), saving it; how many it had.
pub fn drop_lo(out: &mut Out, q: (u32, u32)) -> Result<usize> {
    let n = crate::terrain_pack::drop_named(out, &[format!("layers/slope/lo/3-{}-{}", q.0, q.1)]);
    out.save()?;
    Ok(n)
}

/// Z6 tile (`x`, `y`)'s slope mid in the manifest: its z6–8 tiles as made (`build_piece`), which its
/// z3 tile's assembly reads (`build_lo`).
pub fn mid_logical(x: u32, y: u32) -> String {
    format!("work/slope-mid/6-{x}-{y}")
}

/// The mid's format.
const MID_FMT: u64 = 1;

/// Writes z6 tile `t`'s mid to `path`: a sectioned file (store::sect), meta `{"fmt": 1, "step":
/// "slope", "tile": "6/x/y", "v": SLOPE_V}`, a section `<z>-<x>-<y>` for each of its z6–8 tiles (its
/// bytes as stored), in (z, x, y) order.
pub fn write_mid(path: &std::path::Path, t: (u32, u32), tiles: &[Made]) -> Result<()> {
    let meta = serde_json::json!({ "fmt": MID_FMT, "step": "slope", "tile": format!("6/{}/{}", t.0, t.1), "v": crate::agent::build::SLOPE_V });
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut w = store::sect::SectWriter::create(path, meta)?;
    let mut tiles: Vec<&Made> = tiles.iter().collect();
    tiles.sort_by_key(|t| (t.0, t.1, t.2));
    for (z, x, y, b) in tiles {
        anyhow::ensure!(
            (6..=8).contains(z) && (x >> (z - 6), y >> (z - 6)) == t,
            "{z}/{x}/{y} isn't one of 6/{}/{}'s z6–8 tiles",
            t.0,
            t.1
        );
        w.add(&format!("{z}-{x}-{y}"), b)?;
    }
    w.finish()?;
    Ok(())
}

/// A mid (`write_mid`), read and checked (its format, step and version, every section whole): its
/// z6 tile and its tiles, in (z, x, y) order.
pub fn read_mid(path: &std::path::Path) -> Result<((u32, u32), Vec<Made>)> {
    use anyhow::Context;
    let what = || path.display().to_string();
    // (Read whole, one read under a deadline, tried again while it fails transiently: its sections
    // read one by one from the share were a round trip each, some 21 a mid.)
    let bytes = crate::depth::grids::open_retrying(
        path,
        &|p| crate::depth::grids::within(p, MID_DEADLINE, read_file),
        MID_WAITS,
    )
    .with_context(what)?;
    let r = store::sect::SectReader::open(bytes).with_context(what)?;
    let m = r.meta();
    anyhow::ensure!(m["fmt"].as_u64() == Some(MID_FMT) && m["step"] == "slope", "{}: not a slope mid", what());
    anyhow::ensure!(
        m["v"].as_u64() == Some(crate::agent::build::SLOPE_V as u64),
        "{}: a mid of slope version {}, not {}",
        what(),
        m["v"],
        crate::agent::build::SLOPE_V
    );
    let t = m["tile"]
        .as_str()
        .and_then(crate::legacy::TileId::parse)
        .filter(|u| u.z == 6)
        .with_context(|| format!("{}: no z6 tile", what()))?;
    let mut out = Vec::new();
    for s in r.sections() {
        let u = crate::legacy::TileId::parse(&s.name.replace('-', "/"))
            .filter(|u| (6..=8).contains(&u.z) && (u.x >> (u.z - 6), u.y >> (u.z - 6)) == (t.x, t.y))
            .with_context(|| format!("{}: a section {:?}", what(), s.name))?;
        out.push((u.z, u.x, u.y, r.read(&s.name).with_context(what)?));
    }
    out.sort_by_key(|t| (t.0, t.1, t.2));
    Ok(((t.x, t.y), out))
}

/// How long a mid's read may take before the job fails (an SMB read can stay stuck in the kernel
/// for many minutes; the job's own retry takes over). A mid is a few MB.
const MID_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// The waits before a mid's read is tried again (crate::depth::grids::open_retrying); a test's
/// short.
const MID_WAITS: &[std::time::Duration] =
    if cfg!(test) { &[std::time::Duration::from_millis(1)] } else { &crate::depth::grids::OPEN_WAITS };

/// How many mids an assembly reads at once (`read_mids`): each a round trip or two to the share.
const MIDS_AT_ONCE: usize = 8;

fn read_file(p: &std::path::Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(p)
}

/// The raw bytes of a tile's codes (RGB): the most its stored WebP is counted at.
const RAW: u64 = (TS * TS * 3) as u64;

/// How many tiles lie under one tile from `d0` to `d1` levels down.
const fn tiles_below(d0: u8, d1: u8) -> u64 {
    let (mut n, mut d) = (0u64, d0);
    while d <= d1 {
        n += 1 << (2 * d);
        d += 1;
    }
    n
}

/// The decoded terrain a job keeps (`KEEP` tiles of 256 × 256 f32), in bytes.
const KEPT: u64 = KEEP as u64 * (TS * TS * 4) as u64;

/// The most memory (MB) a z6 tile's run takes, from its bounds: its hi tiles held until written
/// (z9–12) and the tiles of one deep pack likewise (z13–16 under a z10 tile: the deep packs are
/// made one at a time), each at most its raw codes; its z6–8 tiles; the decoded terrain kept.
pub const fn peak_mb() -> u64 {
    let held = tiles_below(9 - 6, HI_MAX - 6) + tiles_below(3, MAX_Z - PACK_Z) + tiles_below(0, 2);
    (held * RAW + KEPT) >> 20
}

/// The most memory (MB) an assembly takes, from its bounds: its z6 tiles' mids (64 of z6–8) and
/// the lo tiles it makes (z3–8), each at most its raw codes; the decoded terrain kept.
pub const fn lo_peak_mb() -> u64 {
    ((64 * tiles_below(0, 2) + tiles_below(0, 5)) * RAW + KEPT) >> 20
}

/// The terrain's deep tiles under z6 tile `t`, by z10 tile (their packs: crate::terrain_deep).
fn deep_terrain(terr: &ManifestTiles, t: (u32, u32)) -> Result<BTreeMap<(u32, u32), Vec<(u8, u32, u32)>>> {
    let k = PACK_Z - 6;
    let mut by = BTreeMap::new();
    for x in t.0 << k..(t.0 + 1) << k {
        for y in t.1 << k..(t.1 + 1) << k {
            let ts = terr.tiles_in(&crate::terrain_deep::pack_logical(x, y))?;
            if !ts.is_empty() {
                by.insert((x, y), ts);
            }
        }
    }
    Ok(by)
}

/// The tiles z6 tile `t` works out: the terrain tiles it has (z9–12, `has`, and its deep tiles
/// `deep`) and their ancestors down to z6, and its z6 tile.
fn piece_set(
    has: &(dyn Fn(u8, u32, u32) -> bool + Sync),
    deep: &BTreeMap<(u32, u32), Vec<(u8, u32, u32)>>,
    t: (u32, u32),
) -> HashSet<(u8, u32, u32)> {
    let (tx, ty) = t;
    let mut tiles: HashSet<(u8, u32, u32)> = HashSet::new();
    let mut with = |z: u8, x: u32, y: u32| {
        for dz in 0..=(z - 6) {
            tiles.insert((z - dz, x >> dz, y >> dz));
        }
    };
    for z in 9..=HI_MAX {
        let s = 1u32 << (z - 6);
        for x in tx * s..(tx + 1) * s {
            for y in ty * s..(ty + 1) * s {
                if has(z, x, y) {
                    with(z, x, y);
                }
            }
        }
    }
    for &(z, x, y) in deep.values().flatten() {
        if z > HI_MAX && z <= MAX_Z {
            with(z, x, y);
        }
    }
    tiles.insert((6, tx, ty));
    tiles
}

/// A z6 tile's slope (z6 tile `t`): its deep packs (written here, not uploaded: each z10 tile with
/// deep terrain made whole, one at a time, its terrain pack copied here first), then its hi tiles
/// (z9–12), sorted, and its z6–8 tiles (its mid's).
fn piece(
    out: &Out,
    terr: &ManifestTiles,
    terrain: &Terrain,
    tiles: &HashSet<(u8, u32, u32)>,
    deep: &BTreeMap<(u32, u32), Vec<(u8, u32, u32)>>,
    t: (u32, u32),
    count: &Count,
) -> Result<(Vec<Made>, Vec<Made>, Vec<crate::layers::LocalPack>)> {
    let enc = crate::shade_codec::ENCODING;
    let mut hi: Vec<Made> = Vec::new();
    let mut packs = Vec::new();
    let mut done: HashMap<(u8, u32, u32), Option<Vec<Px>>> = HashMap::new();
    let none = HashMap::new();
    let own = |z: u8, x: u32, y: u32| shade_tile(terrain, z, x, y);
    let tick = || count.one();
    for &(x, y) in deep.keys() {
        let p = crate::timings::phase("a deep pack's terrain copied here", crate::timings::Class::NasRead);
        if let Some(n) = terr.copy_here(PACK_Z + 3, x << 3, y << 3) {
            p.count(n, 1);
        }
        drop(p);
        let mine = Mutex::new(Vec::new());
        let q = Pyramid { own: &own, tiles, out: &mine, tick: &tick, done: &none }.tile(PACK_Z, x, y);
        done.insert((PACK_Z, x, y), q);
        let (mut d, h): (Vec<Made>, Vec<Made>) = mine.into_inner().unwrap().into_iter().partition(|m| m.0 > HI_MAX);
        hi.extend(h);
        d.sort_by_key(|m| (m.0, m.1, m.2));
        let mut it = d.into_iter().map(|(z, x, y, b)| {
            let n = b.len() as u32;
            (z, x, y, b, n)
        });
        if let Some(p) = crate::layers::write_pack_local(out, "slope", enc, false, "deep", (PACK_Z, x, y), &mut it)? {
            packs.push(p);
        }
    }
    let mine = Mutex::new(Vec::new());
    Pyramid { own: &own, tiles, out: &mine, tick: &tick, done: &done }.tile(6, t.0, t.1);
    let (h, mut lo): (Vec<Made>, Vec<Made>) = mine.into_inner().unwrap().into_iter().partition(|m| m.0 >= 9);
    hi.extend(h);
    hi.sort_by_key(|m| (m.0, m.1, m.2));
    lo.sort_by_key(|m| (m.0, m.1, m.2));
    Ok((hi, lo, packs))
}

/// A z3 tile's assembly (z3 tile `q`, its z6 tiles `ts`): its lo tiles (z3–8), sorted. Each z6 tile's
/// z6–8 tiles from `mids` (its mid, or the slope lo pack's for one that has none: a z6 tile current
/// without one has its tiles there), its z6 tile's quadrant read from its stored z6 tile; each other
/// z6 tile of q its terrain's own shading (none of its z7–8 tiles: the server makes them from their
/// ancestor, as for any tile the layer lacks), never what an earlier run stored, so the pack is what
/// a fresh build makes; then z5–z3.
fn assemble(
    out: &Out,
    terrain: &Terrain,
    q: (u32, u32),
    ts: &[(u32, u32)],
    mids: &BTreeMap<(u32, u32), Vec<Made>>,
    count: &Count,
) -> Result<Vec<Made>> {
    let slope_now = ManifestTiles::new(out, "slope");
    let mut made: Vec<Made> = Vec::new();
    let mut z6q: HashMap<(u32, u32), Vec<Px>> = HashMap::new();
    for &(tx, ty) in ts {
        let tiles: Vec<Made> = match mids.get(&(tx, ty)) {
            Some(m) => m.clone(),
            None => {
                let mut v = Vec::new();
                for z in 6..=8u8 {
                    let s = 1u32 << (z - 6);
                    for x in tx * s..(tx + 1) * s {
                        for y in ty * s..(ty + 1) * s {
                            if let Some(b) = slope_now.get(z, x, y)? {
                                v.push((z, x, y, b));
                            }
                        }
                    }
                }
                v
            }
        };
        if let Some(v) = tiles.iter().find(|t| t.0 == 6).and_then(|t| roadcore::shade::decode(&t.3).ok()) {
            z6q.insert((tx, ty), quadrant(&v));
        }
        made.extend(tiles);
    }
    // The other z6 tiles of q: their terrain's own shading.
    for x in q.0 * 8..(q.0 + 1) * 8 {
        for y in q.1 * 8..(q.1 + 1) * 8 {
            if z6q.contains_key(&(x, y)) {
                continue;
            }
            count.one();
            let Some(v) = shade_tile(terrain, 6, x, y) else { continue };
            let (blob, qd) = stored(&v);
            made.push((6, x, y, blob));
            z6q.insert((x, y), qd);
        }
    }
    // z5 → z3 of q from the z6 quadrants.
    let mut below = z6q;
    for z in (3..=5u8).rev() {
        let s = 1u32 << (z - 3);
        let mut next = HashMap::new();
        for x in q.0 * s..(q.0 + 1) * s {
            for y in q.1 * s..(q.1 + 1) * s {
                count.one();
                if let Some(v) = compose(&kids_of(&below, x, y), || shade_tile(terrain, z, x, y)) {
                    let (blob, qd) = stored(&v);
                    made.push((z, x, y, blob));
                    next.insert((x, y), qd);
                }
            }
        }
        below = next;
    }
    made.sort_by_key(|t| (t.0, t.1, t.2));
    made.dedup_by_key(|t| (t.0, t.1, t.2));
    Ok(made.into_iter().filter(|t| t.0 <= 8).collect())
}

/// Tile (x, y)'s children's quadrants among `below` (the next zoom's, by column and row).
fn kids_of(below: &HashMap<(u32, u32), Vec<Px>>, x: u32, y: u32) -> Vec<((u32, u32), Vec<Px>)> {
    (0..4u32)
        .filter_map(|k| {
            let c = (2 * x + (k & 1), 2 * y + (k >> 1));
            below.get(&c).map(|q| (c, q.clone()))
        })
        .collect()
}

/// The build's terrain as a job reads it (`Terrain`), from the manifest's packs as it opened them.
fn terrain_reads(
    terr: &ManifestTiles,
) -> (impl Fn(u8, u32, u32) -> Option<Vec<u8>> + Sync + '_, impl Fn(u8, u32, u32) -> bool + Sync + '_) {
    (
        move |z: u8, x: u32, y: u32| terr.get(z, x, y).ok().flatten(),
        move |z: u8, x: u32, y: u32| terr.has(z, x, y).unwrap_or(false),
    )
}

/// Makes z6 tile `t`'s slope (a z6 tile) from the build's terrain packs and uploads it: its hi pack
/// (z9–12; none when it made no tile, its terrain having no hi tile: the manifest's then dropped),
/// its deep packs (z13–16 where the terrain has deep tiles; those the manifest names under it and
/// it no longer makes dropped) and its mid (`mid_logical`: its z6–8 tiles), which its z3 tile's
/// assembly reads. `expect_same`: a z6 tile made again as it is (its mid made), whose packs must
/// come out as the manifest has them (none where it has none), else an error and nothing uploaded.
/// A z6 tile the coverage has left isn't made: its packs and mid are dropped (`drop_piece`).
pub fn build_piece(
    out: &mut Out,
    t: (u32, u32),
    expect_same: bool,
    on: &(dyn Fn(&str, u64, u64) + Sync),
) -> Result<Report> {
    let rep = build_piece_later(out, t, expect_same, on)?;
    out.save()?;
    out.claim_into_costin();
    Ok(rep)
}

/// `build_piece`, its uploads put off (crate::out::Out::put_file_later): they go on beside what the
/// job does next, and the z6 tile is saved (and noted done) by a later save (`Out::save_upto` the
/// mark after it).
pub fn build_piece_later(
    out: &mut Out,
    t: (u32, u32),
    expect_same: bool,
    on: &(dyn Fn(&str, u64, u64) + Sync),
) -> Result<Report> {
    use crate::timings::{phase, Class};
    let mut rep = Report::default();
    let p = phase("the terrain packs' indexes read", Class::NasRead);
    let terr = ManifestTiles::with_copies(out, "terrain", out.scratch_file("terrain-copies"));
    let (get, has) = terrain_reads(&terr);
    let terrain = Terrain::new(&get, &has);
    let deep = deep_terrain(&terr, t)?;
    let tiles = piece_set(&has, &deep, t);
    let count = Count::new(tiles.len() as u64 + 1, on);
    drop(p);
    // Its own z6 tile's terrain pack, which it reads thousands of tiles of (each a round trip to
    // the share), copied here whole, one sequential read (its deep packs likewise as it comes to
    // them: `piece`); its neighbours' few edge tiles read where they are.
    let p = phase("its terrain pack copied here", Class::NasRead);
    if let Some(n) = terr.copy_here(9, t.0 << 3, t.1 << 3) {
        p.count(n, 1);
    }
    drop(p);
    on("slope tiles worked out", 0, count.total);
    let p = phase("slope tiles worked out (the terrain read as needed)", Class::Compute);
    let (hi, lo, deep_packs) = piece(out, &terr, &terrain, &tiles, &deep, t, &count)?;
    drop(terrain);
    drop(p);
    let p = phase("its hi pack and mid written", Class::Disk);
    rep.hi_tiles = hi.len();
    rep.lo_tiles = lo.len();
    rep.deep_packs = deep_packs.len();
    rep.deep_tiles = tiles.iter().filter(|u| u.0 > HI_MAX).count();
    let mut it = hi.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let enc = crate::shade_codec::ENCODING;
    let pack = crate::layers::write_pack_local(out, "slope", enc, false, "hi", (6, t.0, t.1), &mut it)?;
    let ml = mid_logical(t.0, t.1);
    let mid = out.scratch_file(&format!("{ml}.sect"));
    write_mid(&mid, t, &lo)?;
    let hl = hi_logical(t.0, t.1);
    let made: BTreeSet<&str> = deep_packs.iter().map(|p| p.logical.as_str()).collect();
    let gone: Vec<String> = deep_under(&out.manifest, t).into_iter().filter(|l| !made.contains(l.as_str())).collect();
    rep.deep_dropped = gone.len();
    if expect_same {
        let mut differ: Vec<String> = Vec::new();
        for (l, p) in std::iter::once((&hl, pack.as_ref())).chain(deep_packs.iter().map(|p| (&p.logical, Some(p)))) {
            let made = p.map(|p| p.content_name()).transpose()?;
            if out.get(l) != made.as_deref() {
                differ.push(format!(
                    "{l}: made {}, the manifest has {}",
                    made.as_deref().unwrap_or("none"),
                    out.get(l).unwrap_or("none")
                ));
            }
        }
        differ.extend(gone.iter().map(|l| format!("{l}: no longer made")));
        if !differ.is_empty() {
            for p in pack.iter().chain(&deep_packs) {
                std::fs::remove_file(&p.local).ok();
            }
            std::fs::remove_file(&mid).ok();
            anyhow::bail!(
                "piece 6/{}/{} was expected the same as the manifest has it, and isn't (nothing uploaded): {}",
                t.0,
                t.1,
                differ.join("; ")
            );
        }
    }
    drop(p);
    match pack {
        Some(p) => out.put_file_later(&p.logical, "pack", &p.local)?,
        None => {
            crate::terrain_pack::drop_named(out, &[hl]);
        }
    }
    for d in deep_packs {
        out.put_file_later(&d.logical, "pack", &d.local)?;
    }
    crate::terrain_pack::drop_named(out, &gone);
    out.put_file_later(&ml, "sect", &mid)?;
    count.one();
    Ok(rep)
}

/// The mids of z3 tile `q`'s z6 tiles `ts` that have one, each read whole and checked (`read_mid`),
/// `MIDS_AT_ONCE` at once (one after another, each section a read, a z3 tile's 60-odd mids took
/// most of an assembly's time: task #400). The first failure fails it.
fn read_mids(out: &Out, q: (u32, u32), ts: &[(u32, u32)]) -> Result<BTreeMap<(u32, u32), Vec<Made>>> {
    let mut todo: Vec<((u32, u32), String)> = Vec::new();
    for &t in ts {
        anyhow::ensure!((t.0 >> 3, t.1 >> 3) == q, "6/{}/{} isn't in 3/{}/{}", t.0, t.1, q.0, q.1);
        if let Some(c) = out.get(&mid_logical(t.0, t.1)) {
            todo.push((t, c.to_string()));
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let got: Mutex<BTreeMap<(u32, u32), Vec<Made>>> = Mutex::new(BTreeMap::new());
    let failed: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..MIDS_AT_ONCE.min(todo.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= todo.len() || failed.lock().unwrap().is_some() {
                    return;
                }
                let (t, c) = &todo[i];
                let r = read_mid(&out.path(c)).and_then(|(of, tiles)| {
                    anyhow::ensure!(of == *t, "{c} is 6/{}/{}'s mid", of.0, of.1);
                    Ok(tiles)
                });
                match r {
                    Ok(tiles) => {
                        got.lock().unwrap().insert(*t, tiles);
                    }
                    Err(e) => {
                        failed.lock().unwrap().get_or_insert(e);
                    }
                }
            });
        }
    });
    match failed.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(got.into_inner().unwrap()),
    }
}

/// Makes z3 tile `q`'s zoomed-out slope (an assembly: its lo pack, z3–8) from its z6 tiles' mids (`ts`:
/// its z6 tiles near the coverage; one current without a mid has its z6–8 tiles in the lo pack as it
/// is) and the lo pack as it is, and uploads it.
pub fn build_lo(
    out: &mut Out,
    q: (u32, u32),
    ts: &[(u32, u32)],
    on: &(dyn Fn(&str, u64, u64) + Sync),
) -> Result<Report> {
    use crate::timings::{phase, Class};
    let mut rep = Report::default();
    let p = phase("the pieces' mids and the terrain packs' indexes read", Class::NasRead);
    let mids = read_mids(out, q, ts)?;
    p.count(0, mids.len() as u64);
    let terr = ManifestTiles::new(out, "terrain");
    let (get, has) = terrain_reads(&terr);
    let terrain = Terrain::new(&get, &has);
    let count = Count::new(64 + 16 + 4 + 1, on);
    drop(p);
    let p = phase("the z3 tile's slope assembled", Class::Compute);
    let lo = assemble(out, &terrain, q, ts, &mids, &count)?;
    drop(terrain);
    drop(p);
    let up = phase("the lo pack uploaded", Class::NasWrite);
    on("packs written", 0, 1);
    rep.lo_tiles = lo.len();
    let mut it = lo.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let enc = crate::shade_codec::ENCODING;
    if crate::layers::write_pack(out, "slope", enc, false, "lo", (3, q.0, q.1), &mut it)?.is_none() {
        crate::terrain_pack::drop_named(out, &[format!("layers/slope/lo/3-{}-{}", q.0, q.1)]);
    }
    drop(up);
    out.save()?;
    on("packs written", 1, 1);
    Ok(rep)
}

/// The root pack (z0–2) made from the 64 z3 slope tiles as stored (their quadrants), where a z3
/// tile is missing from its own terrain's shading.
pub fn build_root(out: &mut Out) -> Result<usize> {
    use crate::timings::{phase, Class};
    let p = phase("the z3 slope tiles and the terrain packs' indexes read", Class::NasRead);
    let terr = ManifestTiles::new(out, "terrain");
    let have = ManifestTiles::new(out, "slope");
    let (get, has) = terrain_reads(&terr);
    let terrain = Terrain::new(&get, &has);
    let mut below: HashMap<(u32, u32), Vec<Px>> = HashMap::new();
    for x in 0..8u32 {
        for y in 0..8u32 {
            if let Some(v) = have.get(3, x, y)?.and_then(|b| roadcore::shade::decode(&b).ok()) {
                below.insert((x, y), quadrant(&v));
            }
        }
    }
    drop(p);
    let p = phase("the root's slope worked out", Class::Compute);
    let mut made: Vec<Made> = Vec::new();
    for z in (0..=2u8).rev() {
        let n = 1u32 << z;
        let mut next = HashMap::new();
        for x in 0..n {
            for y in 0..n {
                if let Some(v) = compose(&kids_of(&below, x, y), || shade_tile(&terrain, z, x, y)) {
                    let (blob, qd) = stored(&v);
                    made.push((z, x, y, blob));
                    next.insert((x, y), qd);
                }
            }
        }
        below = next;
    }
    drop(terrain);
    drop((get, has));
    drop((terr, have));
    made.sort_by_key(|t| (t.0, t.1, t.2));
    drop(p);
    let up = phase("the root pack uploaded", Class::NasWrite);
    let n = made.len();
    let mut it = made.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    crate::layers::write_pack(out, "slope", crate::shade_codec::ENCODING, false, "root", (0, 0, 0), &mut it)?;
    drop(up);
    out.save()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use roadcore::grid::{encode_terrain_png, tile_with_fallback_by};

    /// Horn's shading as computed straight from the tiles, each neighbour value looked up per pixel.
    fn shade_direct(get: &dyn Fn(u8, u32, u32) -> Option<Vec<u8>>, z: u8, x: u32, y: u32) -> Option<Vec<Px>> {
        let tf = |z: u8, x: u32, y: u32| tile_with_fallback_by(get, z, x, y);
        let e = tf(z, x, y)?;
        let n = 1u32 << z;
        let nb = |dx: i64, dy: i64| {
            let ny = y as i64 + dy;
            if ny < 0 || ny >= n as i64 {
                return None;
            }
            tf(z, (x as i64 + dx).rem_euclid(n as i64) as u32, ny as u32)
        };
        let (west, east, north, south) = (nb(-1, 0), nb(1, 0), nb(0, -1), nb(0, 1));
        let at = |i: i32, j: i32| -> f32 {
            let (ci, cj) = (i.clamp(0, 255), j.clamp(0, 255));
            let pick = |t: &Option<Vec<f32>>, ii: i32, jj: i32| t.as_ref().map(|v| v[(jj * 256 + ii) as usize]);
            let v = if i < 0 {
                pick(&west, 255, cj)
            } else if i > 255 {
                pick(&east, 0, cj)
            } else if j < 0 {
                pick(&north, ci, 255)
            } else if j > 255 {
                pick(&south, ci, 0)
            } else {
                None
            };
            v.unwrap_or(e[(cj * 256 + ci) as usize])
        };
        let mut out = vec![Px::default(); TS * TS];
        for j in 0..256i32 {
            let yy = (y as f64 + (j as f64 + 0.5) / 256.0) / n as f64;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * yy)).dsinh().datan();
            let d = (40_075_016.686f64 * lat.dcos() / (256.0 * n as f64)) as f32;
            for i in 0..256i32 {
                let (a, b, c) = (at(i - 1, j - 1), at(i, j - 1), at(i + 1, j - 1));
                let (dd, f) = (at(i - 1, j), at(i + 1, j));
                let (g, h, k) = (at(i - 1, j + 1), at(i, j + 1), at(i + 1, j + 1));
                let dzdx = ((c + 2.0 * f + k) - (a + 2.0 * dd + g)) / (8.0 * d);
                let dzdy = ((g + 2.0 * h + k) - (a + 2.0 * b + c)) / (8.0 * d);
                out[(j * 256 + i) as usize] = Px::of_gradient(dzdx, dzdy);
            }
        }
        Some(out)
    }

    /// Wavy terrain for tile z/x/y, as a Terrarium PNG (what decode_terrain reads besides WebP).
    fn hill(z: u8, x: u32, y: u32) -> Vec<u8> {
        let e: Vec<f32> = (0..TS * TS)
            .map(|p| {
                ((p % TS) as f32 * 0.37 + (p / TS) as f32 * 1.9 + x as f32 * 7.0 - y as f32 * 3.0 + z as f32).dsin()
                    * 40.0
                    + 300.0
            })
            .collect();
        encode_terrain_png(&e, TS as u32, TS as u32).unwrap()
    }

    #[test]
    fn terrain_and_shading_as_read_straight_from_the_tiles() {
        // A z8 tile and three of its z9 children (the fourth upsampled from it), a z9 tile that
        // doesn't decode, and a z10 tile under the missing child (its siblings from two levels up).
        let mut tiles: HashMap<(u8, u32, u32), Vec<u8>> = HashMap::new();
        tiles.insert((8, 100, 50), hill(8, 100, 50));
        for (x, y) in [(200, 100), (201, 100), (200, 101)] {
            tiles.insert((9, x, y), hill(9, x, y));
        }
        tiles.insert((9, 202, 100), b"not a png".to_vec());
        tiles.insert((10, 402, 202), hill(10, 402, 202));
        let get = |z: u8, x: u32, y: u32| tiles.get(&(z, x, y)).cloned();
        let has = |z: u8, x: u32, y: u32| tiles.contains_key(&(z, x, y));
        let terrain = Terrain::new(&get, &has);
        let asked = [
            (8, 100, 50),
            (9, 200, 100),
            (9, 201, 101),
            (9, 202, 100),
            (10, 402, 202),
            (10, 403, 203),
            (10, 404, 200),
            (9, 5, 5),
            (9, 199, 100),
        ];
        for _ in 0..2 {
            for &(z, x, y) in &asked {
                assert_eq!(
                    terrain.tile(z, x, y).map(|t| (*t).clone()),
                    tile_with_fallback_by(&get, z, x, y),
                    "{z}/{x}/{y}"
                );
                assert_eq!(shade_tile(&terrain, z, x, y), shade_direct(&get, z, x, y), "{z}/{x}/{y}");
            }
        }
    }

    /// Terrain packs for z6 tile 6/28/16: its z9 tile 9/224/128's z12 tiles (16 by 16 of them, a hi
    /// pack), and where `deep` the four z13 tiles under 12/1792/1024 and two z14 under one of them (a
    /// deep pack of z10 tile 10/448/256), each its own hill.
    fn terrain(out: &mut Out, deep: bool) {
        let tile = |z: u8, x: u32, y: u32| {
            let b = hill(z, x, y);
            let n = b.len() as u32;
            (z, x, y, b, n)
        };
        let hi: Vec<_> = (1792..1800).flat_map(|x| (1024..1032).map(move |y| tile(12, x, y))).collect();
        crate::layers::write_pack(out, "terrain", "terrarium-png", false, "hi", (6, 28, 16), &mut hi.into_iter())
            .unwrap();
        if deep {
            let mut d: Vec<_> = [(3584, 2048), (3585, 2048), (3584, 2049), (3585, 2049)]
                .into_iter()
                .map(|(x, y)| tile(13, x, y))
                .collect();
            d.extend([(7168, 4096), (7169, 4096)].into_iter().map(|(x, y)| tile(14, x, y)));
            crate::layers::write_pack(
                out,
                "terrain",
                "terrarium-png",
                false,
                "deep",
                (10, 448, 256),
                &mut d.into_iter(),
            )
            .unwrap();
        }
        out.save().unwrap();
    }

    /// Every level of a z6 tile is stored, the finest with deep terrain too: the deep tiles in a deep
    /// pack by z10 tile, z9–12 in the hi pack, z6–8 in the mid; each pixel of a parent the mean of the
    /// four stored beneath it where its children cover it (the deep detail reaching z6), else its own
    /// terrain's shading. Made again, the same packs; the deep terrain gone, its deep pack dropped.
    #[test]
    fn a_z6_tile_stores_every_level_down_to_its_deep_terrain() {
        let d = tempfile::tempdir().unwrap();
        let mut out = Out::unbound(&d.path().join("root"), &d.path().join("scratch")).unwrap();
        terrain(&mut out, true);
        let r = build_piece(&mut out, (28, 16), false, &|_, _, _| {}).unwrap();
        assert_eq!((r.deep_packs, r.deep_tiles, r.deep_dropped), (1, 6, 0), "{r:?}");
        assert!(out.get(&deep_logical(448, 256)).is_some() && out.get(&hi_logical(28, 16)).is_some());
        let s = ManifestTiles::new(&out, "slope");
        let at = |z: u8, x: u32, y: u32| s.get(z, x, y).unwrap().map(|b| roadcore::shade::decode(&b).unwrap());
        for (z, x, y) in [(14, 7168, 4096), (13, 3585, 2049), (12, 1792, 1024), (12, 1799, 1031), (9, 224, 128)] {
            assert!(at(z, x, y).is_some(), "{z}/{x}/{y} stored");
        }
        assert!(at(14, 7170, 4096).is_none() && at(13, 3586, 2048).is_none(), "none past the terrain's");
        // z13 → z12: the parent's every pixel the mean of the four stored beneath (all four z13 tiles
        // there), as stored; its z14 child likewise in its quadrant.
        let parent_of = |z: u8, x: u32, y: u32| {
            let p = at(z, x, y).unwrap();
            let mut want = vec![Px::default(); TS * TS];
            for k in 0..4u32 {
                let (cx, cy) = (2 * x + (k & 1), 2 * y + (k >> 1));
                let Some(c) = at(z + 1, cx, cy) else { continue };
                let q = quadrant(&c);
                for j in 0..128 {
                    for i in 0..128 {
                        want[((cy % 2) as usize * 128 + j) * TS + (cx % 2) as usize * 128 + i] = q[j * 128 + i];
                    }
                }
            }
            (p, want)
        };
        let (p, want) = parent_of(12, 1792, 1024);
        assert_eq!(roadcore::shade::rgb(&p), roadcore::shade::rgb(&want), "z12 from its z13 tiles");
        // (A tile's own terrain's shading, inside it: its neighbours don't matter there.)
        let own =
            |z, x, y| shade_tile(&Terrain::new(&|z, x, y| Some(hill(z, x, y)), &|_, _, _| true), z, x, y).unwrap();
        let (p, want) = parent_of(13, 3584, 2048);
        let rgb = |v: &[Px], j: usize, i: usize| roadcore::shade::code(v[j * TS + i]);
        assert_eq!(rgb(&p, 10, 10), rgb(&want, 10, 10), "z13's quadrant from its z14 tiles");
        assert_eq!(rgb(&p, 200, 10), rgb(&own(13, 3584, 2048), 200, 10), "and its own terrain's beside them");
        // (The deep detail reaches the z12 level everywhere the deep tiles are: unlike z12's own.)
        assert_ne!(roadcore::shade::rgb(&at(12, 1792, 1024).unwrap()), roadcore::shade::rgb(&own(12, 1792, 1024)));
        drop(s);
        // Made again expecting the same: nothing changes.
        let before = out.manifest.clone();
        out.remove(&mid_logical(28, 16));
        build_piece(&mut out, (28, 16), true, &|_, _, _| {}).unwrap();
        assert_eq!(out.manifest, before);
        // The deep terrain gone: refused when expected the same, else its deep pack dropped.
        out.remove(&crate::terrain_deep::pack_logical(448, 256));
        out.save().unwrap();
        let e = build_piece(&mut out, (28, 16), true, &|_, _, _| {}).unwrap_err().to_string();
        assert!(e.contains(&deep_logical(448, 256)) && e.contains("nothing uploaded"), "{e}");
        let r = build_piece(&mut out, (28, 16), false, &|_, _, _| {}).unwrap();
        assert_eq!((r.deep_packs, r.deep_dropped), (0, 1));
        assert!(out.get(&deep_logical(448, 256)).is_none());
        // And its z12 tile its own terrain's again.
        let s = ManifestTiles::new(&out, "slope");
        let z12 = roadcore::shade::decode(&s.get(12, 1792, 1024).unwrap().unwrap()).unwrap();
        assert_eq!(rgb(&z12, 100, 100), rgb(&own(12, 1792, 1024), 100, 100));
    }

    /// The memory bounds are those of the tiles a run holds.
    #[test]
    fn the_memory_bounds_count_the_tiles_held() {
        assert_eq!(tiles_below(3, 6), 64 + 256 + 1024 + 4096);
        assert_eq!(peak_mb(), ((5440 * 2 + 21) * 196_608 + 512 * 262_144) >> 20);
        assert_eq!(lo_peak_mb(), ((64 * 21 + 1365) * 196_608 + 512 * 262_144) >> 20);
    }

    /// With terrain: an assembly over a lo pack an earlier run stored, holding tiles of a z6 tile
    /// that isn't its z6 tile (6/24/23's z6 tile and a z7 tile of it, another tile's bytes), comes
    /// out as a fresh build's assembly, byte for byte: nothing of the earlier run kept.
    #[test]
    fn an_assembly_over_an_earlier_runs_lo_pack_is_a_fresh_builds() {
        let d = tempfile::tempdir().unwrap();
        // Terrain for 3/3/2: its z6–z3 tiles (a lo pack) and four z9 tiles of its z6 tile 6/28/16 (a
        // hi pack), each its own hill.
        let png = |z: u8, x: u32, y: u32| {
            let b = hill(z, x, y);
            let n = b.len() as u32;
            (z, x, y, b, n)
        };
        let terrain = |out: &mut Out| {
            let lo: Vec<_> = (3..=6u8)
                .flat_map(|z| {
                    let s = 1u32 << (z - 3);
                    (3 * s..4 * s).flat_map(move |x| (2 * s..3 * s).map(move |y| (z, x, y)))
                })
                .map(|(z, x, y)| png(z, x, y))
                .collect();
            crate::layers::write_pack(out, "terrain", "terrarium-png", false, "lo", (3, 3, 2), &mut lo.into_iter())
                .unwrap();
            let hi: Vec<_> =
                [(224, 128), (224, 129), (225, 128), (225, 129)].into_iter().map(|(x, y)| png(9, x, y)).collect();
            crate::layers::write_pack(out, "terrain", "terrarium-png", false, "hi", (6, 28, 16), &mut hi.into_iter())
                .unwrap();
            out.save().unwrap();
        };
        let lo = "layers/slope/lo/3-3-2";
        let mut fresh = Out::unbound(&d.path().join("fresh"), &d.path().join("fresh-scratch")).unwrap();
        terrain(&mut fresh);
        build_piece(&mut fresh, (28, 16), false, &|_, _, _| {}).unwrap();
        build_lo(&mut fresh, (3, 2), &[(28, 16)], &|_, _, _| {}).unwrap();
        assert!(fresh.get(lo).is_some());
        let made = ManifestTiles::new(&fresh, "slope");
        let (z6, z7) = (made.get(6, 25, 23).unwrap().unwrap(), made.get(7, 56, 32).unwrap().unwrap());
        assert!(
            made.get(7, 48, 46).unwrap().is_none(),
            "a fresh build stores no z7 tile of a z6 tile that isn't its piece"
        );
        assert_ne!(made.get(6, 24, 23).unwrap().as_ref(), Some(&z6), "the earlier run's tile isn't the fresh one");
        drop(made);
        let mut again = Out::unbound(&d.path().join("again"), &d.path().join("again-scratch")).unwrap();
        terrain(&mut again);
        let old = vec![(6, 24, 23, z6.clone(), z6.len() as u32), (7, 48, 46, z7.clone(), z7.len() as u32)];
        let enc = crate::shade_codec::ENCODING;
        crate::layers::write_pack(&mut again, "slope", enc, false, "lo", (3, 3, 2), &mut old.into_iter()).unwrap();
        again.save().unwrap();
        build_piece(&mut again, (28, 16), false, &|_, _, _| {}).unwrap();
        build_lo(&mut again, (3, 2), &[(28, 16)], &|_, _, _| {}).unwrap();
        assert_eq!(again.get(lo), fresh.get(lo), "the same lo pack as a fresh build's");
        // The root from the z3 tiles as stored (no terrain there but 3/3/2's: its ancestors alone),
        // 3/3/2's quadrant in its z2 tile's north-east quarter.
        assert_eq!(build_root(&mut again).unwrap(), 3);
        let s = ManifestTiles::new(&again, "slope");
        let z3 = roadcore::shade::decode(&s.get(3, 3, 2).unwrap().unwrap()).unwrap();
        let z2 = roadcore::shade::decode(&s.get(2, 1, 1).unwrap().unwrap()).unwrap();
        assert_eq!(roadcore::shade::code(z2[128 + 5]), roadcore::shade::code(quadrant(&z3)[5]));
    }

    /// A z6 tile whose terrain has no hi tile makes no hi tile, and drops the hi pack an earlier run
    /// left (expected the same, it isn't: refused, nothing uploaded); a z6 tile the coverage has
    /// left drops its hi pack, deep packs and mid (`drop_piece`); a "none" assembly its lo pack.
    #[test]
    fn a_piece_making_no_hi_tile_or_left_by_the_coverage_drops_its_packs() {
        let d = tempfile::tempdir().unwrap();
        // (Files an earlier run left: their bytes don't matter here.)
        let fake = |out: &mut Out, l: &str| {
            let ext = if l.starts_with("work/") { "sect" } else { "pack" };
            let c = out.put_bytes(l, ext, l.as_bytes()).unwrap();
            out.save().unwrap();
            c
        };
        let (hi, mid, other) = (hi_logical(28, 16), mid_logical(28, 16), hi_logical(29, 16));
        let (lhi, lmid, ldeep) = (hi_logical(24, 23), mid_logical(24, 23), deep_logical(24 * 16 + 3, 23 * 16 + 15));
        let mut out = Out::unbound(&d.path().join("root"), &d.path().join("scratch")).unwrap();
        fake(&mut out, &hi);
        let kept = fake(&mut out, &other);
        let e = build_piece(&mut out, (28, 16), true, &|_, _, _| {}).unwrap_err().to_string();
        assert!(e.contains(&hi) && e.contains("made none") && e.contains("nothing uploaded"), "{e}");
        assert!(out.get(&hi).is_some() && out.get(&mid).is_none());
        let r = build_piece(&mut out, (28, 16), false, &|_, _, _| {}).unwrap();
        assert_eq!(r.hi_tiles, 0);
        assert!(out.get(&hi).is_none() && out.get(&mid).is_some());
        let before = out.manifest.clone();
        out.remove(&mid);
        build_piece(&mut out, (28, 16), true, &|_, _, _| {}).unwrap();
        assert_eq!(out.manifest, before, "made again as it is: the same");
        for l in [&lhi, &lmid, &ldeep] {
            fake(&mut out, l);
        }
        assert_eq!(deep_under(&out.manifest, (24, 23)), [ldeep.clone()]);
        assert_eq!(drop_piece(&mut out, (24, 23)).unwrap(), 3);
        assert!(out.get(&lhi).is_none() && out.get(&lmid).is_none() && out.get(&ldeep).is_none());
        assert_eq!(out.get(&other), Some(kept.as_str()), "another tile's hi pack stays");
        // A z3 tile the coverage has left: its "none" assembly drops its lo pack.
        fake(&mut out, "layers/slope/lo/3-5-2");
        assert_eq!(drop_lo(&mut out, (5, 2)).unwrap(), 1);
        assert!(out.get("layers/slope/lo/3-5-2").is_none());
    }

    /// Task #400: a z3 tile's mids read whole, several at once, give each z6 tile's tiles as written, in
    /// order; a mid that can't be read, or is another z6 tile's, fails the read.
    #[test]
    fn an_areas_mids_read_together_are_each_pieces_tiles() {
        let d = tempfile::tempdir().unwrap();
        let mut out = Out::unbound(&d.path().join("root"), &d.path().join("scratch")).unwrap();
        let q = (2u32, 1u32);
        let ts: Vec<(u32, u32)> = (0..20u32).map(|k| (16 + k % 8, 8 + k / 8)).collect();
        let tiles_of = |t: (u32, u32)| -> Vec<Made> {
            let mut v = vec![(6, t.0, t.1, format!("6 {t:?}").into_bytes())];
            for k in 0..4u32 {
                v.push((7, 2 * t.0 + (k & 1), 2 * t.1 + (k >> 1), vec![k as u8; 100 + (t.0 * 7 + t.1) as usize]));
            }
            v.sort_by_key(|t| (t.0, t.1, t.2));
            v
        };
        // (Every z6 tile but the last has a mid: that one's tiles come from the lo pack.)
        for &t in &ts[..ts.len() - 1] {
            let p = out.scratch_file("mid.sect");
            write_mid(&p, t, &tiles_of(t)).unwrap();
            out.put_file(&mid_logical(t.0, t.1), "sect", &p).unwrap();
        }
        let mids = read_mids(&out, q, &ts).unwrap();
        assert_eq!(mids.len(), ts.len() - 1);
        for &t in &ts[..ts.len() - 1] {
            assert_eq!(mids[&t], tiles_of(t), "6/{}/{}", t.0, t.1);
        }
        // Another z6 tile's mid under one's name: an error, naming it.
        let (a, b) = (ts[0], ts[1]);
        let theirs = out.get(&mid_logical(b.0, b.1)).unwrap().to_string();
        out.set(&mid_logical(a.0, a.1), Some(theirs.clone()));
        let e = read_mids(&out, q, &ts).unwrap_err().to_string();
        assert!(e.contains(&theirs) && e.contains(&format!("6/{}/{}'s mid", b.0, b.1)), "{e}");
        // One gone: an error (NotFound is tried again first, as a share's moment of it is).
        std::fs::remove_file(out.path(&theirs)).unwrap();
        assert!(read_mids(&out, q, &ts[1..2]).is_err());
        // A z6 tile outside q: refused before anything is read.
        assert!(read_mids(&out, (0, 0), &ts).unwrap_err().to_string().contains("isn't in 3/0/0"));
    }

    /// A measure (run by hand, read-only: nothing written to the root): a real slope z6 tile (`piece
    /// 6/x/y`: its terrain packs copied here, its tiles worked out) or assembly (`lo 3/x/y`: its z6
    /// tiles' mids and the terrain read, its tiles worked out), timed, with the CPU it took, the bytes
    /// of the tiles by zoom and a digest of them (the same before and after a change). With a third
    /// word, a folder, its tiles are written there as `<z>/<x>/<y>.webp` too.
    /// SCENIC_SLOPE_MEASURE="<root> piece|lo <x>/<y> [<folder>]"; unset, nothing.
    #[test]
    fn measure_a_real_target() {
        let Ok(m) = std::env::var("SCENIC_SLOPE_MEASURE") else { return };
        let a: Vec<&str> = m.split_whitespace().collect();
        let root = std::path::Path::new(a[0]);
        let (x, y) = a[2].split_once('/').unwrap();
        let t: (u32, u32) = (x.parse().unwrap(), y.parse().unwrap());
        let scratch = tempfile::tempdir().unwrap();
        let out = Out::unbound(root, scratch.path()).unwrap();
        let cpu = || {
            let mut u: libc::rusage = unsafe { std::mem::zeroed() };
            unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
            let s = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
            s(u.ru_utime) + s(u.ru_stime)
        };
        crate::timings::start(if a[1] == "lo" { "slope-lo" } else { "slope" });
        let (t0, c0) = (std::time::Instant::now(), cpu());
        let none = |_: &str, _: u64, _: u64| {};
        let made: Vec<Made> = if a[1] == "lo" {
            let ts: Vec<(u32, u32)> = (0..64)
                .map(|k| (t.0 * 8 + k % 8, t.1 * 8 + k / 8))
                .filter(|&(x, y)| out.get(&hi_logical(x, y)).is_some() || out.get(&mid_logical(x, y)).is_some())
                .collect();
            let mids = read_mids(&out, t, &ts).unwrap();
            let terr = ManifestTiles::new(&out, "terrain");
            let (get, has) = terrain_reads(&terr);
            let terrain = Terrain::new(&get, &has);
            let _p = crate::timings::phase("assembled", crate::timings::Class::Compute);
            assemble(&out, &terrain, t, &ts, &mids, &Count::new(85, &none)).unwrap()
        } else {
            let terr = ManifestTiles::with_copies(&out, "terrain", out.scratch_file("terrain-copies"));
            let (get, has) = terrain_reads(&terr);
            terr.copy_here(9, t.0 << 3, t.1 << 3);
            let terrain = Terrain::new(&get, &has);
            let deep = deep_terrain(&terr, t).unwrap();
            let tiles = piece_set(&has, &deep, t);
            let _p = crate::timings::phase("worked out", crate::timings::Class::Compute);
            let (hi, lo, packs) = piece(&out, &terr, &terrain, &tiles, &deep, t, &Count::new(1, &none)).unwrap();
            let mut all: Vec<Made> = hi.into_iter().chain(lo).collect();
            for p in packs {
                let f = store::range::PlainFile::open(&p.local).unwrap();
                let idx = store::pack::PackIndex::read_from(&f).unwrap();
                for e in &idx.entries {
                    let (z, x, y) = e.zxy();
                    all.push((z, x, y, idx.read_blob(&f, e).unwrap()));
                }
            }
            all
        };
        let (wall, cpu) = (t0.elapsed().as_secs_f64(), cpu() - c0);
        let mut h = <sha2::Sha256 as sha2::Digest>::new();
        let mut by_z: BTreeMap<u8, (usize, usize)> = BTreeMap::new();
        for (z, x, y, b) in &made {
            sha2::Digest::update(&mut h, format!("{z}/{x}/{y} {}\n", b.len()));
            sha2::Digest::update(&mut h, b);
            let e = by_z.entry(*z).or_default();
            *e = (e.0 + 1, e.1 + b.len());
            if let Some(dir) = a.get(3) {
                let p = std::path::Path::new(dir).join(format!("{z}/{x}"));
                std::fs::create_dir_all(&p).unwrap();
                std::fs::write(p.join(format!("{y}.webp")), b).unwrap();
            }
        }
        let r = crate::timings::snapshot(true).unwrap();
        println!(
            "{} {}: {} tiles in {wall:.1} s, CPU {cpu:.1} s ({:.2} cores); digest {:x}",
            a[1],
            a[2],
            made.len(),
            cpu / wall,
            sha2::Digest::finalize(h)
        );
        for (z, (n, b)) in by_z {
            println!("  z{z}: {n} tiles, {:.1} KB a tile", b as f64 / n as f64 / 1024.0);
        }
        for p in &r.phases {
            println!("  {:<34} {:>8.1} s  CPU {:>6.1} s  n={}", p.name, p.wall_s, p.cpu_s.unwrap_or(0.0), p.n);
        }
    }
}
