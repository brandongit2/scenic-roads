//! The slope layer (docs/steps.md §2, Slope): the terrain's shading pyramid (roadcore::shade, stored
//! by crate::shade_codec), which the map's slope tint and hill-shading both read. At each place's
//! finest terrain level, every pixel's steepness and direction by Horn's method (with the
//! neighbouring tiles); each coarser level the mean of the four pixels beneath it, or where no finer
//! level covers a pixel, its own level's terrain's. Every level is stored: z0–2 in the root
//! (`build_root`), z3–8 by z3 tile (lo: `build_lo`, from the z6 tiles' mids), z9–12 by z6 tile (hi:
//! the slope-hi step, `build_piece`) and the deep levels (z13–16, where the terrain has them:
//! crate::terrain_deep) by z10 tile (deep: `deep_logical`, the slope-deep step, `build_deep`), from
//! the build's terrain packs. A slope-deep z10 tile makes its z13 tiles' subtrees from its terrain
//! deep tiles alone and hands their quadrants on at full precision in its deep mid (`DeepMid`); its
//! z6 tile's slope-hi run makes z10–12 from them and its hi terrain, taking each in as made: the
//! same bytes as one run of the whole.

use crate::out::Out;
use crate::terrain_deep::{FIRST_Z, MAX_Z, PACK_Z};
use crate::terrain_pack::ManifestTiles;
use anyhow::Result;
use det::Det;
use rayon::prelude::*;
use roadcore::grid::{decode_terrain, upsampled};
use roadcore::shade::{quadrant, Px};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

const TS: usize = 256;
/// The hi packs' last zoom (their band's: the deep packs hold z13 on, crate::terrain_deep).
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

    /// Tile (z, x, y), or a missing one from its nearest ancestor down to `fallback_floor`'s zoom;
    /// None when there's none, or the tile there doesn't decode.
    pub fn tile(&self, z: u8, x: u32, y: u32) -> Tile {
        if (self.has)(z, x, y) {
            if let Some(t) = self.slot((z, x, y, 0), || self.decode(z, x, y)) {
                return t;
            }
        }
        for dz in 1..=z - fallback_floor(z) {
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

/// The coarsest zoom a missing terrain tile at zoom `z` is made from (`Terrain::tile`, and the
/// keys' agent::tiles::Reader::resolve): its nearest ancestor up to eight levels up, but a deep
/// tile's (crate::terrain_deep: z13 on) only within the deep levels, so the deep slope reads the
/// terrain deep packs alone. Past the deep terrain's end, no tile: Horn's method takes the tile's
/// own edge pixels again there (`horn`), a strip of one pixel along the end of the fine coverage.
pub fn fallback_floor(z: u8) -> u8 {
    if z >= FIRST_Z {
        FIRST_Z
    } else {
        z.saturating_sub(8)
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
/// its children among `tiles` (depth first), its pixels their quadrants where they cover it
/// (`compose`), else its own level's (`own`), stored in `out`; its quadrant for its parent. A tile
/// `done` gives a quadrant for (Some: none or one) isn't made: its quadrant as made before, asked
/// for once.
///
/// Its parallelism, which its memory bound counts (`working_bytes`): the children of the tiles
/// within `PAR_LEVELS` of the call's tile are made in parallel (on `pool`'s threads), each tile
/// below those levels' subtree on one thread alone, depth first, with no parallel call inside it,
/// so a thread works on one subtree's chain at a time. Its values don't depend on the threads: a
/// pixel is `mean4` of its four children's in a fixed order, each child's quadrant placed by its
/// column and row (`compose`), and nothing is summed across tiles.
pub struct Pyramid<'a> {
    pub own: &'a (dyn Fn(u8, u32, u32) -> Option<Vec<Px>> + Sync),
    pub tiles: &'a HashSet<(u8, u32, u32)>,
    pub out: &'a Mutex<Vec<Made>>,
    pub tick: &'a (dyn Fn() + Sync),
    pub done: &'a (dyn Fn(u8, u32, u32) -> Option<Option<Vec<Px>>> + Sync),
}

/// A pyramid's `done` with no tile made before.
pub fn none_done(_: u8, _: u32, _: u32) -> Option<Option<Vec<Px>>> {
    None
}

/// The levels under a pyramid's tile whose children are made in parallel: 64 subtrees below them.
pub const PAR_LEVELS: u8 = 3;

/// The most threads a pyramid's build uses (`pool`): the job's (RAYON_NUM_THREADS) up to this.
pub const THREADS: usize = 16;

/// The pool a pyramid's build runs on: the job's threads, at most `THREADS`.
fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(rayon::current_num_threads().clamp(1, THREADS))
            .build()
            .expect("a pyramid's thread pool")
    })
}

impl Pyramid<'_> {
    /// Tile (z, x, y) and those under it among `tiles` (none past `MAX_Z`); its quadrant.
    pub fn tile(&self, z: u8, x: u32, y: u32) -> Option<Vec<Px>> {
        pool().install(|| self.node(z, x, y, z + PAR_LEVELS))
    }

    /// The tiles `tops` at zoom `z`, each with those under it among `tiles` (`tile`), the tops in
    /// parallel and each one's subtree on one thread; their quadrants, in turn.
    pub fn tops(&self, z: u8, tops: &[(u32, u32)]) -> Vec<Option<Vec<Px>>> {
        // (Each top a task of its own: split into a chunk a thread, the chunks of uneven tops left
        // threads idle at the end.)
        pool().install(|| tops.par_iter().with_max_len(1).map(|&(x, y)| self.node(z, x, y, z)).collect())
    }

    /// `tile`: its children in parallel above zoom `par_until`, one after another from it.
    fn node(&self, z: u8, x: u32, y: u32, par_until: u8) -> Option<Vec<Px>> {
        if let Some(q) = (self.done)(z, x, y) {
            return q;
        }
        let kids: Vec<(u32, u32)> = if z < MAX_Z {
            (0..4u32)
                .map(|k| (2 * x + (k & 1), 2 * y + (k >> 1)))
                .filter(|&(cx, cy)| self.tiles.contains(&(z + 1, cx, cy)))
                .collect()
        } else {
            Vec::new()
        };
        let one = |&(cx, cy): &(u32, u32)| self.node(z + 1, cx, cy, par_until).map(|q| ((cx, cy), q));
        let kids: Vec<((u32, u32), Vec<Px>)> = if z < par_until {
            kids.par_iter().filter_map(one).collect()
        } else {
            kids.iter().filter_map(one).collect()
        };
        (self.tick)();
        let v = compose(&kids, || (self.own)(z, x, y))?;
        drop(kids);
        let (blob, q) = stored(&v);
        self.out.lock().unwrap().push((z, x, y, blob));
        (z > 0).then_some(q)
    }
}

/// The most memory (bytes) a pyramid's build holds working, beyond the tiles it stores (`out`) and
/// what `own` keeps, from its bounds: a call's tile `top`, its tiles to `bottom`. Each thread of
/// `pool` (at most `THREADS`; the calling thread waits) works on one chain at a time below the parallel
/// levels (`PAR_LEVELS`), each tile of it holding its children's quadrants (4), and one tile's
/// making (`MAKING`); the tiles of the parallel levels are each made once, each holding its
/// children's quadrants while it waits.
pub const fn working_bytes(top: u8, bottom: u8) -> u64 {
    chains_bytes(top + PAR_LEVELS, bottom) + tiles_below(0, PAR_LEVELS - 1) * 4 * QUADRANT
}

/// The most memory (bytes) `pool`'s threads hold working on subtrees from zoom `from` to
/// `bottom`, one each, one after another (`working_bytes` below its parallel levels, `tops`):
/// each tile of a thread's chain its children's quadrants (4), and one tile's making (`MAKING`).
const fn chains_bytes(from: u8, bottom: u8) -> u64 {
    let chain = if bottom + 1 > from { (bottom + 1 - from) as u64 } else { 0 };
    THREADS as u64 * (chain * 4 * QUADRANT + MAKING)
}

/// A tile's making at most (bytes): `compose`'s values, coverage and own level's pixels and the
/// pixels it returns; `own`'s Horn border (`horn`) and the five terrain tiles it reads (its own
/// and its edge neighbours', in metres); the codes `encode` takes. (libwebp's own working memory
/// isn't counted.)
const MAKING: u64 = 3 * PX_TILE + (TS * TS) as u64 + ((TS + 2) * (TS + 2) * 4) as u64 + 5 * (TS * TS * 4) as u64 + RAW;

/// A tile's pixels (256 × 256), in bytes.
const PX_TILE: u64 = (TS * TS * std::mem::size_of::<Px>()) as u64;

/// A tile as stored and its quadrant for its parent, from its pixels at full precision: inside a
/// job a parent is the mean of its children's values, coded once, as itself; only where a job hands
/// its tiles on (a z6 tile's mid to its assembly, the z3 tiles to the root) is a parent made from
/// its children as stored. (Coded at every level, the direction drifted a step of its 5 bits by
/// z8 and up to 9 % of pixels changed between none and some; the steepness crept up: task #632's
/// review, `measure_quantisation_drift`.)
pub fn stored(v: &[Px]) -> (Vec<u8>, Vec<Px>) {
    (crate::shade_codec::encode(v).expect("a tile's WebP"), quadrant(v))
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
    crate::layers::hi_logical("slope", x, y)
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

/// Drops z6 tile `t`'s slope hi pack and mid from the manifest (a z6 tile the coverage has left:
/// docs/regions.md, Shrinking), saving it; how many it had. (Its deep packs and deep mids are its
/// z10 tiles' slope-deep targets': `drop_deep`.)
pub fn drop_piece(out: &mut Out, t: (u32, u32)) -> Result<usize> {
    let gone = vec![hi_logical(t.0, t.1), mid_logical(t.0, t.1)];
    let n = crate::layers::drop(out, &gone);
    out.save()?;
    Ok(n)
}

/// Drops z3 tile `q`'s slope lo pack from the manifest (a "none" assembly: the coverage has left
/// the z3 tile), saving it; how many it had.
pub fn drop_lo(out: &mut Out, q: (u32, u32)) -> Result<usize> {
    let n = crate::layers::drop(out, &[crate::layers::lo_logical("slope", q.0, q.1)]);
    out.save()?;
    Ok(n)
}

/// Z6 tile (`x`, `y`)'s slope mid in the manifest: its z6–8 tiles as made (`build_piece`), which its
/// z3 tile's assembly reads (`build_lo`).
pub fn mid_logical(x: u32, y: u32) -> String {
    crate::layers::mid_logical("slope", x, y)
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
pub fn read_mid<'a>(path: impl Into<store::nasfs::Loc<'a>>) -> Result<((u32, u32), Vec<Made>)> {
    use anyhow::Context;
    let path = path.into();
    let what = || path.display();
    // (Read whole, one read under a deadline, tried again while it fails transiently: its sections
    // read one by one from the share were a round trip each, some 21 a mid.)
    let bytes = read_whole(path).with_context(what)?;
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
fn mid_waits() -> Vec<std::time::Duration> {
    if cfg!(test) {
        vec![std::time::Duration::from_millis(1)]
    } else {
        store::retry::waits()
    }
}

/// How many mids an assembly reads at once (`read_mids`): each a round trip or two to the share.
const MIDS_AT_ONCE: usize = 8;

fn read_file(p: &std::path::Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(p)
}

fn read_nas(p: &store::nasfs::NasPath) -> std::io::Result<Vec<u8>> {
    p.read()
}

/// A mid's bytes, read whole under `MID_DEADLINE`, tried again while it fails transiently.
fn read_whole(path: store::nasfs::Loc<'_>) -> std::io::Result<Vec<u8>> {
    use crate::depth::grids::{open_retrying, within};
    match path {
        store::nasfs::Loc::Local(p) => open_retrying(p, &|p| within(p, MID_DEADLINE, read_file), &mid_waits()),
        store::nasfs::Loc::Nas(p) => open_retrying(p, &|p| within(p, MID_DEADLINE, read_nas), &mid_waits()),
    }
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

/// A quadrant's values kept for its parent (128 × 128 pixels).
const QUADRANT: u64 = (128 * 128 * std::mem::size_of::<Px>()) as u64;

/// A deep mid's z13 quadrants at most (a z10 tile's z13 tiles).
const MID_QUADRANTS: u64 = 1 << (2 * (FIRST_Z - PACK_Z));

/// The most memory (MB) a slope-hi z6 tile's run takes, from its bounds: its hi tiles held until
/// written (z9–12), each at most its raw codes; its z6–8 tiles; the deep mids open
/// (`DeepQuadrants`: one a thread of the pyramid's, each its quadrants and, as it's read, its
/// bytes); the pyramid's working memory (`working_bytes`); the decoded terrain kept; the z6 tile
/// before's uploads in flight beside it (crate::out::UPLOAD_HELD).
pub const fn peak_mb() -> u64 {
    let held = tiles_below(9 - 6, HI_MAX - 6) + tiles_below(0, 2);
    let mids = THREADS as u64 * 2 * MID_QUADRANTS * QUADRANT;
    (held * RAW + mids + working_bytes(6, HI_MAX) + KEPT + crate::out::UPLOAD_HELD) >> 20
}

/// The most memory (MB) a slope-deep z10 tile's run takes, from its bounds: its deep pack's tiles
/// (z13–16) held until written, each at most its raw codes; its z13 tiles' quadrants; the
/// pyramid's working memory (`chains_bytes`: a z13 subtree a thread); the decoded terrain kept;
/// beside it, the target before's uploads in flight (crate::out::UPLOAD_HELD) and the next one's
/// pack being copied ahead (`DeepBatch`: a copy's buffer, store::sys::COPY_BUF).
pub const fn deep_peak_mb() -> u64 {
    let held = tiles_below(FIRST_Z - PACK_Z, MAX_Z - PACK_Z);
    let beside = crate::out::UPLOAD_HELD + store::sys::COPY_BUF as u64;
    (held * RAW + MID_QUADRANTS * QUADRANT + chains_bytes(FIRST_Z, MAX_Z) + KEPT + beside) >> 20
}

/// The most memory (MB) an assembly takes, from its bounds: its z6 tiles' mids (64 of z6–8) and
/// the lo tiles it makes (z3–8), each at most its raw codes; the decoded terrain kept.
pub const fn lo_peak_mb() -> u64 {
    ((64 * tiles_below(0, 2) + tiles_below(0, 5)) * RAW + KEPT) >> 20
}

/// The terrain's deep tiles under z10 tile `t` (its deep pack: crate::terrain_deep).
fn deep_terrain(terr: &ManifestTiles, t: (u32, u32)) -> Result<Vec<(u8, u32, u32)>> {
    terr.tiles_in(&crate::terrain_deep::pack_logical(t.0, t.1))
}

/// Tiles `tiles` and their ancestors up to zoom `top`.
fn with_ancestors(tiles: impl IntoIterator<Item = (u8, u32, u32)>, top: u8) -> HashSet<(u8, u32, u32)> {
    let mut out = HashSet::new();
    for (z, x, y) in tiles {
        for dz in 0..=(z - top) {
            out.insert((z - dz, x >> dz, y >> dz));
        }
    }
    out
}

/// The tiles a z6 tile `t`'s pyramid works out from its hi terrain: the terrain tiles under it it
/// has (z9–12, `has`) and their ancestors up to it, and itself.
fn tiles_under(has: &(dyn Fn(u8, u32, u32) -> bool + Sync), t: (u32, u32)) -> HashSet<(u8, u32, u32)> {
    let mut hi = vec![(6, t.0, t.1)];
    for z in 9..=HI_MAX {
        let s = 1u32 << (z - 6);
        for x in t.0 * s..(t.0 + 1) * s {
            for y in t.1 * s..(t.1 + 1) * s {
                if has(z, x, y) {
                    hi.push((z, x, y));
                }
            }
        }
    }
    with_ancestors(hi, 6)
}

/// The z13 tiles among a slope-deep run's tiles (`deep_tiles`), sorted: its tops, whose quadrants
/// its deep mid hands on.
fn deep_tops(tiles: &HashSet<(u8, u32, u32)>) -> Vec<(u32, u32)> {
    let mut tops: Vec<(u32, u32)> = tiles.iter().filter(|t| t.0 == FIRST_Z).map(|&(_, x, y)| (x, y)).collect();
    tops.sort_unstable();
    tops
}

/// The tiles z10 tile `t`'s slope-deep run works out: its deep pack's tiles `deep` (z13–16) and
/// their ancestors up to z13 (its tops: `deep_tops`).
fn deep_tiles(deep: &[(u8, u32, u32)], t: (u32, u32)) -> HashSet<(u8, u32, u32)> {
    let under =
        |&&(z, x, y): &&(u8, u32, u32)| (FIRST_Z..=MAX_Z).contains(&z) && (x >> (z - PACK_Z), y >> (z - PACK_Z)) == t;
    with_ancestors(deep.iter().filter(under).copied(), FIRST_Z)
}

/// A z10 tile's part of its z6 tile's pyramid, as a slope-deep run hands it on (`write_deep_mid`):
/// each of its z13 tiles' quadrant at full precision (`deep_tops`; none where it shades nothing),
/// which the z6 tile's z12 tiles are made from (`stored`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeepMid {
    pub quadrants: BTreeMap<(u32, u32), Option<Vec<Px>>>,
}

/// The tiles z6 tile `t`'s pyramid works out: its hi terrain's (`tiles_under`), and the z13 tiles of
/// its z10 tiles with a deep mid (`tops`, by z10 tile: their quadrants made by their slope-deep
/// runs, not made here) with their ancestors.
fn piece_tiles(
    has: &(dyn Fn(u8, u32, u32) -> bool + Sync),
    tops: &BTreeMap<(u32, u32), Vec<(u32, u32)>>,
    t: (u32, u32),
) -> HashSet<(u8, u32, u32)> {
    let mut tiles = tiles_under(has, t);
    tiles.extend(with_ancestors(tops.values().flatten().map(|&(x, y)| (FIRST_Z, x, y)), 6));
    tiles
}

/// A z6 tile's z10 tiles' deep mids as its pyramid takes them in (`Pyramid::done`): each z13
/// quadrant asked for once, its mid read (`read`) when the first of them is, and let go once all are
/// taken. Below the pyramid's parallel levels a thread makes a z9 tile's subtree alone, a z10 tile
/// after another, so it holds one mid at a time (`peak_mb`).
struct DeepQuadrants<'a> {
    read: &'a (dyn Fn((u32, u32)) -> Result<DeepMid> + Sync),
    /// The z10 tiles with a deep mid.
    of: BTreeSet<(u32, u32)>,
    open: Mutex<HashMap<(u32, u32), Arc<Mutex<Option<BTreeMap<(u32, u32), Option<Vec<Px>>>>>>>>,
    /// The first mid that couldn't be read; its quadrants none from then on.
    failed: Mutex<Option<anyhow::Error>>,
}

impl DeepQuadrants<'_> {
    /// Z13 tile (x, y)'s quadrant as its slope-deep run made it (Some: none or one); None where its
    /// z10 tile has no deep mid.
    fn take(&self, x: u32, y: u32) -> Option<Option<Vec<Px>>> {
        let k = FIRST_Z - PACK_Z;
        let f = (x >> k, y >> k);
        if !self.of.contains(&f) {
            return None;
        }
        if self.failed.lock().unwrap().is_some() {
            return Some(None);
        }
        let slot = self.open.lock().unwrap().entry(f).or_default().clone();
        let mut s = slot.lock().unwrap();
        if s.is_none() {
            match (self.read)(f) {
                Ok(m) => *s = Some(m.quadrants),
                Err(e) => {
                    self.failed.lock().unwrap().get_or_insert(e);
                    return Some(None);
                }
            }
        }
        let m = s.as_mut().unwrap();
        let q = m.remove(&(x, y)).flatten();
        if m.is_empty() {
            self.open.lock().unwrap().remove(&f);
        }
        Some(q)
    }
}

/// A z6 tile's slope (z6 tile `t`, its pyramid's tiles `tiles`), its z10 tiles' z13 quadrants from
/// their deep mids (`done`: made by their slope-deep runs, not here): its hi tiles (z9–12), sorted,
/// and its z6–8 tiles (its mid's).
fn piece(
    terrain: &Terrain,
    tiles: &HashSet<(u8, u32, u32)>,
    done: &(dyn Fn(u8, u32, u32) -> Option<Option<Vec<Px>>> + Sync),
    t: (u32, u32),
    count: &Count,
) -> (Vec<Made>, Vec<Made>) {
    let own = |z: u8, x: u32, y: u32| shade_tile(terrain, z, x, y);
    let tick = || count.one();
    let mine = Mutex::new(Vec::new());
    Pyramid { own: &own, tiles, out: &mine, tick: &tick, done }.tile(6, t.0, t.1);
    let (mut hi, mut lo): (Vec<Made>, Vec<Made>) = mine.into_inner().unwrap().into_iter().partition(|m| m.0 >= 9);
    hi.sort_by_key(|m| (m.0, m.1, m.2));
    lo.sort_by_key(|m| (m.0, m.1, m.2));
    (hi, lo)
}

/// A z10 tile's part of the pyramid (its tiles `tiles`: `deep_tiles`), as its z6 tile's run would
/// make it: its deep tiles (z13–16, for its deep pack) and its mid
/// (`DeepMid`: its z13 tiles' quadrants).
fn deep_part(terrain: &Terrain, tiles: &HashSet<(u8, u32, u32)>, count: &Count) -> (Vec<Made>, DeepMid) {
    let own = |z: u8, x: u32, y: u32| shade_tile(terrain, z, x, y);
    let tick = || count.one();
    let mine = Mutex::new(Vec::new());
    let tops = deep_tops(tiles);
    let qs = Pyramid { own: &own, tiles, out: &mine, tick: &tick, done: &none_done }.tops(FIRST_Z, &tops);
    let mut deep = mine.into_inner().unwrap();
    deep.sort_by_key(|m| (m.0, m.1, m.2));
    (deep, DeepMid { quadrants: tops.into_iter().zip(qs).collect() })
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
    pieces: &mut crate::resume_log::Pieces,
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
    // The other z6 tiles of q: their terrain's own shading, a row of them a piece kept (each tile its
    // bytes and quadrant, none for one with no terrain).
    for y in q.1 * 8..(q.1 + 1) * 8 {
        let row: Vec<u32> = (q.0 * 8..(q.0 + 1) * 8).filter(|&x| !z6q.contains_key(&(x, y))).collect();
        if row.is_empty() {
            continue;
        }
        let p = pieces.piece((6, q.0 * 8, y), || {
            let mut p = crate::resume_log::Put::default();
            for &x in &row {
                count.one();
                match shade_tile(terrain, 6, x, y) {
                    Some(v) => {
                        let (blob, qd) = stored(&v);
                        p.u8(1);
                        p.bytes(&blob);
                        p.f32s(&qd.iter().flat_map(|c| [c.s, c.u[0], c.u[1]]).collect::<Vec<_>>());
                    }
                    None => p.u8(0),
                }
            }
            Ok(p.0)
        })?;
        let mut g = crate::resume_log::Get(&p);
        for &x in &row {
            if g.u8()? == 0 {
                continue;
            }
            let blob = g.bytes()?;
            let qd = g.f32s()?.chunks_exact(3).map(|c| Px { s: c[0], u: [c[1], c[2]] }).collect();
            made.push((6, x, y, blob));
            z6q.insert((x, y), qd);
        }
        g.end()?;
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

/// Makes z6 tile `t`'s slope (a slope-hi z6 tile) from the build's terrain packs and its z10 tiles'
/// deep mids (their slope-deep runs': `build_deep`), and uploads it: its hi pack (z9–12; none when it
/// made no tile, its terrain having no hi tile: the manifest's then dropped) and its mid
/// (`mid_logical`: its z6–8 tiles), which its z3 tile's assembly reads. `expect_same`: a z6 tile
/// made again as it is (its mid made), whose hi pack must come out as the manifest has it (none
/// where it has none), else an error and nothing uploaded. A z6 tile the coverage has left isn't
/// made: its hi pack and mid are dropped (`drop_piece`).
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
    use crate::timings::phase;
    let mut rep = Report::default();
    let p = phase("reading the terrain packs' indexes");
    let terr = ManifestTiles::with_copies(out, "terrain", out.scratch_file("terrain-copies"));
    let (get, has) = terrain_reads(&terr);
    let terrain = Terrain::new(&get, &has);
    // (Its z10 tiles with a deep mid, and the z13 tiles their slope-deep runs made: those of their
    // terrain deep packs.)
    let mids: BTreeMap<(u32, u32), String> = deep_mids_under(&out.manifest, t).into_iter().collect();
    let mut tops = BTreeMap::new();
    for &f in mids.keys() {
        tops.insert(f, deep_tops(&deep_tiles(&deep_terrain(&terr, f)?, f)));
    }
    let tiles = piece_tiles(&has, &tops, t);
    let count = Count::new(tiles.len() as u64 + 1, on);
    drop(p);
    // Its own z6 tile's terrain pack, which it reads thousands of tiles of (each a round trip to
    // the share), copied here whole, one sequential read; its neighbours' few edge tiles read where
    // they are.
    let p = phase("copying its terrain pack here");
    if let Some(n) = terr.copy_here(9, t.0 << 3, t.1 << 3) {
        p.count(n, 1);
    }
    drop(p);
    on("slope tiles worked out", 0, count.total);
    let p = phase("working out the slope tiles");
    let read = |f: (u32, u32)| -> Result<DeepMid> {
        let c = &mids[&f];
        let (of, mid) = read_deep_mid(&out.path(c))?;
        anyhow::ensure!(of == f, "{c} is 10/{}/{}'s deep mid", of.0, of.1);
        anyhow::ensure!(
            mid.quadrants.keys().eq(tops[&f].iter()),
            "{c}: its z13 tiles aren't those of 10/{}/{}'s terrain deep pack",
            f.0,
            f.1
        );
        Ok(mid)
    };
    let deep = DeepQuadrants {
        read: &read,
        of: mids.keys().copied().collect(),
        open: Default::default(),
        failed: Default::default(),
    };
    let done = |z: u8, x: u32, y: u32| if z == FIRST_Z { deep.take(x, y) } else { None };
    let (hi, lo) = piece(&terrain, &tiles, &done, t, &count);
    if let Some(e) = deep.failed.into_inner().unwrap() {
        return Err(e.context(format!("piece 6/{}/{}: a deep mid", t.0, t.1)));
    }
    drop(terrain);
    drop(p);
    let p = phase("writing its hi pack and mid");
    rep.hi_tiles = hi.len();
    rep.lo_tiles = lo.len();
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
    if expect_same {
        let made = pack.as_ref().map(|p| p.content_name()).transpose()?;
        if out.get(&hl) != made.as_deref() {
            for p in pack.iter() {
                std::fs::remove_file(&p.local).ok();
            }
            std::fs::remove_file(&mid).ok();
            anyhow::bail!(
                "piece 6/{}/{} was expected the same as the manifest has it, and isn't (nothing uploaded): {hl}: made {}, the manifest has {}",
                t.0,
                t.1,
                made.as_deref().unwrap_or("none"),
                out.get(&hl).unwrap_or("none")
            );
        }
    }
    drop(p);
    match pack {
        Some(p) => out.put_file_later(&p.logical, "pack", &p.local)?,
        None => {
            crate::layers::drop(out, &[hl]);
        }
    }
    out.put_file_later(&ml, "sect", &mid)?;
    count.one();
    Ok(rep)
}

/// Makes z10 tile `t`'s slope deep levels (a slope-deep target: its part of its z6 tile's pyramid,
/// `deep_part`, from its own and its edge neighbours' terrain deep packs alone: `fallback_floor`)
/// and uploads them: its deep pack (z13–16; none when it made no deep tile: the manifest's then
/// dropped) and its deep mid (`deep_mid_logical`: its z13 tiles' quadrants), which its z6 tile's
/// slope-hi run takes in. Saved here (a batch's targets: `DeepBatch::build`, their uploads put off).
pub fn build_deep(out: &mut Out, t: (u32, u32), on: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<Report> {
    let rep = DeepBatch::new(out, &[t]).build(out, t, on)?;
    out.save()?;
    out.claim_into_costin();
    Ok(rep)
}

/// A tile of the terrain pack slope-deep target `t` reads thousands of tiles of, copied here whole
/// before it's made: its terrain deep pack. Its edge neighbours' few edge tiles are read where they
/// are.
fn deep_copied(t: (u32, u32)) -> (u8, u32, u32) {
    (FIRST_Z, t.0 << 3, t.1 << 3)
}

/// A slope-deep job's batch of z10 tiles (`targets`, in the order it makes them), its terrain read
/// through one reader for the whole job. The next target's terrain deep pack is copied on a thread
/// beside the target being made (its compute, its uploads), so it starts with it here; once a
/// target is made, every copy and open pack but that one is let go. The same bytes as each made
/// alone: a pack is content-named, read here or on the NAS alike.
pub struct DeepBatch {
    terr: Arc<ManifestTiles>,
    targets: Vec<(u32, u32)>,
    /// The thread copying the next target's pack ahead.
    ahead: Option<std::thread::JoinHandle<()>>,
}

impl DeepBatch {
    pub fn new(out: &Out, targets: &[(u32, u32)]) -> Self {
        let terr = ManifestTiles::with_copies(out, "terrain", out.scratch_file("terrain-copies"));
        DeepBatch { terr: Arc::new(terr), targets: targets.to_vec(), ahead: None }
    }

    /// Makes z10 tile `t` (`build_deep`), its uploads put off (crate::out::Out::put_file_later):
    /// they go on beside what the job does next, and the target is saved (and noted done) by a later
    /// save (`Out::save_upto` the mark after it). Its pack copied first, or waited for where it's
    /// being copied ahead; then the next target's copying ahead begins.
    pub fn build(&mut self, out: &mut Out, t: (u32, u32), on: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<Report> {
        use crate::timings::phase;
        let at = self.targets.iter().position(|&u| u == t);
        let p = phase("copying its terrain pack here");
        if let Some(h) = self.ahead.take() {
            // (Its pack copied ahead: done before its reads begin.)
            h.join().ok();
        }
        let (z, x, y) = deep_copied(t);
        if let Some(n) = self.terr.copy_here(z, x, y) {
            p.count(n, 1);
        }
        drop(p);
        let next = at.and_then(|k| self.targets.get(k + 1)).copied();
        if let Some(next) = next {
            let terr = self.terr.clone();
            let h = std::thread::Builder::new().name("copies ahead".into()).spawn(move || {
                let p = phase("copying the next target's terrain pack ahead");
                let (z, x, y) = deep_copied(next);
                if let Some(n) = terr.copy_here(z, x, y) {
                    p.count(n, 1);
                }
            });
            // (No thread: the next copies its own.)
            self.ahead = h.ok();
        }
        let rep = build_deep_with(out, &self.terr, t, on);
        // (All but the next target's pack let go: the copying ahead's.)
        let ahead = next.map(|u| {
            let (z, x, y) = deep_copied(u);
            ManifestTiles::logical("terrain", z, x, y)
        });
        self.terr.keep_only(&|l| ahead.as_deref() == Some(l));
        rep
    }

    /// The copies made here deleted now, the copying ahead's once it ends (waited for: by a safe
    /// point it has mostly long ended): the job stopping at a safe point (its exit runs no
    /// destructor).
    pub fn let_go(&mut self) {
        if let Some(h) = self.ahead.take() {
            h.join().ok();
        }
        self.terr.keep_only(&|_| false);
    }
}

/// `DeepBatch::build`'s making of z10 tile `t` from the terrain as `terr` reads it.
fn build_deep_with(
    out: &mut Out,
    terr: &ManifestTiles,
    t: (u32, u32),
    on: &(dyn Fn(&str, u64, u64) + Sync),
) -> Result<Report> {
    use crate::timings::phase;
    let mut rep = Report::default();
    let p = phase("reading the terrain packs' indexes");
    let (get, has) = terrain_reads(terr);
    let terrain = Terrain::new(&get, &has);
    let tiles = deep_tiles(&deep_terrain(terr, t)?, t);
    let count = Count::new(tiles.len() as u64 + 1, on);
    drop(p);
    on("slope tiles worked out", 0, count.total);
    let p = phase("working out the slope tiles");
    let (made, mid) = deep_part(&terrain, &tiles, &count);
    drop(terrain);
    drop(p);
    let p = phase("writing its deep pack and deep mid");
    rep.deep_tiles = made.len();
    let mut it = made.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let enc = crate::shade_codec::ENCODING;
    let pack = crate::layers::write_pack_local(out, "slope", enc, false, "deep", (PACK_Z, t.0, t.1), &mut it)?;
    let ml = deep_mid_logical(t.0, t.1);
    let path = out.scratch_file(&format!("{ml}.sect"));
    write_deep_mid(&path, t, &mid)?;
    drop(p);
    match pack {
        Some(p) => {
            rep.deep_packs = 1;
            out.put_file_later(&p.logical, "pack", &p.local)?;
        }
        None => rep.deep_dropped = crate::layers::drop(out, &[deep_logical(t.0, t.1)]),
    }
    out.put_file_later(&ml, "sect", &path)?;
    count.one();
    Ok(rep)
}

/// Z10 tile `t` no slope-deep target any more: its deep pack and deep mid dropped (not saved); how
/// many.
pub fn drop_deep(out: &mut Out, t: (u32, u32)) -> usize {
    crate::layers::drop(out, &[deep_logical(t.0, t.1), deep_mid_logical(t.0, t.1)])
}

/// Z10 tile (`x`, `y`)'s slope deep mid in the manifest (`DeepMid`): what its z6 tile's slope-hi run
/// takes in.
pub fn deep_mid_logical(x: u32, y: u32) -> String {
    format!("{DEEP_MID_PREFIX}{PACK_Z}-{x}-{y}")
}

/// The deep mids' logical names' prefix.
pub const DEEP_MID_PREFIX: &str = "work/slope-deep-mid/";

/// The z10 tile of a deep mid, from its logical name.
pub fn deep_mid_tile(l: &str) -> Option<(u32, u32)> {
    let u = crate::legacy::TileId::parse(l.strip_prefix(DEEP_MID_PREFIX)?).filter(|u| u.z == PACK_Z)?;
    (deep_mid_logical(u.x, u.y) == l).then_some((u.x, u.y))
}

/// The deep mids the manifest names under z6 tile `t`, by z10 tile.
pub fn deep_mids_under(m: &BTreeMap<String, String>, t: (u32, u32)) -> Vec<((u32, u32), String)> {
    let k = PACK_Z - 6;
    m.range(DEEP_MID_PREFIX.to_string()..)
        .take_while(|(l, _)| l.starts_with(DEEP_MID_PREFIX))
        .filter_map(|(l, c)| deep_mid_tile(l).filter(|f| (f.0 >> k, f.1 >> k) == t).map(|f| (f, c.clone())))
        .collect()
}

/// The deep mids' format (1: a z10 tile's quadrant and its z10–12 tiles, before task #952).
const DEEP_MID_FMT: u64 = 2;

/// A quadrant's bytes in a deep mid: 128 × 128 pixels, each its steepness and direction's two
/// values, f32 little-endian, row by row.
const QUADRANT_BYTES: usize = 128 * 128 * 12;

/// Writes z10 tile `t`'s deep mid to `path`: a sectioned file (store::sect), meta `{"fmt": 2,
/// "step": "slope-deep", "tile": "10/x/y", "v": SLOPE_V}` and a section `13-<x>-<y>` for each of
/// its z13 tiles, in (x, y) order: its quadrant (`QUADRANT_BYTES`), empty where it shades nothing.
pub fn write_deep_mid(path: &std::path::Path, t: (u32, u32), mid: &DeepMid) -> Result<()> {
    let meta = serde_json::json!({ "fmt": DEEP_MID_FMT, "step": "slope-deep", "tile": format!("{PACK_Z}/{}/{}", t.0, t.1), "v": crate::agent::build::SLOPE_V });
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut w = store::sect::SectWriter::create(path, meta)?;
    let k = FIRST_Z - PACK_Z;
    for (&(x, y), q) in &mid.quadrants {
        anyhow::ensure!((x >> k, y >> k) == t, "13/{x}/{y} isn't one of 10/{}/{}'s z13 tiles", t.0, t.1);
        let mut b = Vec::with_capacity(QUADRANT_BYTES);
        if let Some(q) = q {
            anyhow::ensure!(q.len() == 128 * 128, "a quadrant of {} pixels", q.len());
            for p in q {
                for v in [p.s, p.u[0], p.u[1]] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        w.add(&format!("{FIRST_Z}-{x}-{y}"), &b)?;
    }
    w.finish()?;
    Ok(())
}

/// A deep mid (`write_deep_mid`), read whole and checked (its format, step and version, every
/// section whole): its z10 tile and its part.
pub fn read_deep_mid<'a>(path: impl Into<store::nasfs::Loc<'a>>) -> Result<((u32, u32), DeepMid)> {
    use anyhow::Context;
    let path = path.into();
    let what = || path.display();
    let bytes = read_whole(path).with_context(what)?;
    let r = store::sect::SectReader::open(bytes).with_context(what)?;
    let m = r.meta();
    anyhow::ensure!(
        m["fmt"].as_u64() == Some(DEEP_MID_FMT) && m["step"] == "slope-deep",
        "{}: not a slope deep mid of format {DEEP_MID_FMT}",
        what()
    );
    anyhow::ensure!(
        m["v"].as_u64() == Some(crate::agent::build::SLOPE_V as u64),
        "{}: a deep mid of slope version {}, not {}",
        what(),
        m["v"],
        crate::agent::build::SLOPE_V
    );
    let t = m["tile"]
        .as_str()
        .and_then(crate::legacy::TileId::parse)
        .filter(|u| u.z == PACK_Z)
        .with_context(|| format!("{}: no z10 tile", what()))?;
    let k = FIRST_Z - PACK_Z;
    let mut mid = DeepMid::default();
    for s in r.sections() {
        let u = crate::legacy::TileId::parse(&s.name.replace('-', "/"))
            .filter(|u| u.z == FIRST_Z && (u.x >> k, u.y >> k) == (t.x, t.y))
            .with_context(|| format!("{}: a section {:?}", what(), s.name))?;
        let b = r.read(&s.name).with_context(what)?;
        let q = match b.len() {
            0 => None,
            QUADRANT_BYTES => {
                let f = |i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
                Some((0..128 * 128).map(|k| Px { s: f(3 * k), u: [f(3 * k + 1), f(3 * k + 2)] }).collect())
            }
            n => anyhow::bail!("{}: {}'s quadrant of {n} bytes", what(), s.name),
        };
        mid.quadrants.insert((u.x, u.y), q);
    }
    Ok(((t.x, t.y), mid))
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
    let got = crate::layers::read_each(todo.len(), MIDS_AT_ONCE, |i| {
        let (t, c) = &todo[i];
        let (of, tiles) = read_mid(&out.path(c))?;
        anyhow::ensure!(of == *t, "{c} is 6/{}/{}'s mid", of.0, of.1);
        Ok((*t, tiles))
    })?;
    Ok(got.into_iter().collect())
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
    build_lo_keeping(out, q, ts, on, crate::resume_log::on())
}

/// What z3 tile `q`'s zoomed-out slope reads besides its z6 tiles' mids `ts`: the terrain's lo
/// packs of it and around it and its root, and its slope lo pack as it is (a `job_id`'s inputs).
fn lo_inputs(out: &Out, q: (u32, u32), ts: &[(u32, u32)]) -> String {
    let mut ls = vec![crate::layers::root_logical("terrain"), crate::layers::lo_logical("slope", q.0, q.1)];
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (x, y) = (q.0 as i64 + dx, q.1 as i64 + dy);
            if (0..8).contains(&x) && (0..8).contains(&y) {
                ls.push(crate::layers::lo_logical("terrain", x as u32, y as u32));
            }
        }
    }
    ls.extend(ts.iter().map(|t| mid_logical(t.0, t.1)));
    crate::resume_log::names(out, ls)
}

/// `build_lo`, its z6 tiles shaded from their terrain kept a row at a time (task #889:
/// crate::resume_log::Pieces) with `keep`.
pub fn build_lo_keeping(
    out: &mut Out,
    q: (u32, u32),
    ts: &[(u32, u32)],
    on: &(dyn Fn(&str, u64, u64) + Sync),
    keep: bool,
) -> Result<Report> {
    use crate::timings::phase;
    let target = format!("3/{}/{}", q.0, q.1);
    let id = crate::resume_log::job_id(
        "slope-lo",
        &format!(
            "slope-lo {} slope {} {}",
            crate::agent::build::SLOPE_LO_V,
            crate::agent::build::SLOPE_V,
            crate::shade_codec::version()
        ),
        &target,
        &[&lo_inputs(out, q, ts)],
    );
    let mut pieces = crate::resume_log::Pieces::open(&out.scratch, (3, q.0, q.1), "slope-lo", &id, keep);
    let mut rep = Report::default();
    let p = phase("reading the pieces' mids and the terrain packs' indexes");
    let mids = read_mids(out, q, ts)?;
    p.count(0, mids.len() as u64);
    let terr = ManifestTiles::new(out, "terrain");
    let (get, has) = terrain_reads(&terr);
    let terrain = Terrain::new(&get, &has);
    let count = Count::new(64 + 16 + 4 + 1, on);
    drop(p);
    let p = phase("assembling the z3 tile's slope");
    let lo = assemble(out, &terrain, q, ts, &mids, &count, &mut pieces)?;
    drop(terrain);
    drop(p);
    let up = phase("uploading the lo pack");
    on("writing packs", 0, 1);
    rep.lo_tiles = lo.len();
    let mut it = lo.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    let enc = crate::shade_codec::ENCODING;
    if crate::layers::write_pack(out, "slope", enc, false, "lo", (3, q.0, q.1), &mut it)?.is_none() {
        crate::layers::drop(out, &[crate::layers::lo_logical("slope", q.0, q.1)]);
    }
    drop(up);
    out.save()?;
    pieces.finish();
    on("writing packs", 1, 1);
    Ok(rep)
}

/// The root pack (z0–2) made from the 64 z3 slope tiles as stored (their quadrants), where a z3
/// tile is missing from its own terrain's shading.
pub fn build_root(out: &mut Out) -> Result<usize> {
    use crate::timings::phase;
    let p = phase("reading the z3 slope tiles and the terrain packs' indexes");
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
    let p = phase("working out the root's slope");
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
    let up = phase("uploading the root pack");
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

    /// Task #952: a missing deep tile (z13 on) is made from its nearest ancestor within the deep
    /// levels alone, never a hi one; past them, none (Horn's method repeats the tile's own edge
    /// there), so the deep slope reads the deep packs alone. A hi tile falls back as before.
    #[test]
    fn a_deep_tile_falls_back_within_the_deep_levels_alone() {
        let mut tiles: HashMap<(u8, u32, u32), Vec<u8>> = HashMap::new();
        for t in [(11, 896, 512), (12, 1792, 1024), (13, 3584, 2048), (14, 7168, 4096), (14, 7169, 4096)] {
            tiles.insert(t, hill(t.0, t.1, t.2));
        }
        let get = |z: u8, x: u32, y: u32| tiles.get(&(z, x, y)).cloned();
        let has = |z: u8, x: u32, y: u32| tiles.contains_key(&(z, x, y));
        let terrain = Terrain::new(&get, &has);
        assert_eq!(fallback_floor(16), 13);
        assert_eq!(fallback_floor(13), 13);
        assert_eq!(fallback_floor(12), 4);
        let up = |z: u8, x: u32, y: u32| terrain.tile(z, x, y).map(|t| (*t).clone());
        assert_eq!(up(14, 7168, 4097), tile_with_fallback_by(&get, 14, 7168, 4097), "from its z13 tile");
        assert!(up(14, 7170, 4096).is_none() && up(13, 3585, 2048).is_none(), "never from a z12 tile");
        assert_eq!(up(12, 1793, 1025), tile_with_fallback_by(&get, 12, 1793, 1025), "a hi tile as before");
        assert!(up(12, 1793, 1025).is_some());
        // (The deep tiles' shading: as if no hi tile were there.)
        let deep_only = |z: u8, x: u32, y: u32| if z >= 13 { get(z, x, y) } else { None };
        for (z, x, y) in [(14, 7169, 4096), (13, 3584, 2048), (14, 7168, 4097)] {
            assert_eq!(shade_tile(&terrain, z, x, y), shade_direct(&deep_only, z, x, y), "{z}/{x}/{y}");
        }
        assert_ne!(
            shade_tile(&terrain, 14, 7169, 4096),
            shade_direct(&get, 14, 7169, 4096),
            "its east edge not from the z12 tile"
        );
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
    /// pack by z10 tile (its slope-deep run's, with its deep mid), z9–12 in the hi pack, z6–8 in the
    /// mid; each pixel of a parent the mean of the four stored beneath it where its children cover it
    /// (the deep detail reaching z6), else its own terrain's shading; the same bytes as one run of the
    /// whole pyramid. Made again, the same packs; the deep terrain gone, its deep pack dropped.
    #[test]
    fn a_z6_tile_stores_every_level_down_to_its_deep_terrain() {
        let d = tempfile::tempdir().unwrap();
        let mut out =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.path().join("root")), &d.path().join("scratch"))
                .unwrap();
        terrain(&mut out, true);
        let r = build_deep(&mut out, (448, 256), &|_, _, _| {}).unwrap();
        assert_eq!((r.deep_packs, r.deep_tiles, r.deep_dropped), (1, 6, 0), "{r:?}");
        out.save().unwrap();
        assert!(out.get(&deep_mid_logical(448, 256)).is_some());
        build_piece(&mut out, (28, 16), false, &|_, _, _| {}).unwrap();
        assert!(out.get(&deep_logical(448, 256)).is_some() && out.get(&hi_logical(28, 16)).is_some());
        let s = ManifestTiles::new(&out, "slope");
        let at = |z: u8, x: u32, y: u32| s.get(z, x, y).unwrap().map(|b| roadcore::shade::decode(&b).unwrap());
        for (z, x, y) in [(14, 7168, 4096), (13, 3585, 2049), (12, 1792, 1024), (12, 1799, 1031), (9, 224, 128)] {
            assert!(at(z, x, y).is_some(), "{z}/{x}/{y} stored");
        }
        assert!(at(14, 7170, 4096).is_none() && at(13, 3586, 2048).is_none(), "none past the terrain's");
        // The same bytes as the whole pyramid made in one run (the z6 tile's before task #639).
        {
            let terr = ManifestTiles::new(&out, "terrain");
            let (get, has) = terrain_reads(&terr);
            let terrain = Terrain::new(&get, &has);
            let deep = deep_terrain(&terr, (448, 256)).unwrap();
            let mut tiles = tiles_under(&has, (28, 16));
            tiles.extend(with_ancestors(deep, 6));
            let own = |z: u8, x: u32, y: u32| shade_tile(&terrain, z, x, y);
            let mine = Mutex::new(Vec::new());
            Pyramid { own: &own, tiles: &tiles, out: &mine, tick: &|| {}, done: &none_done }.tile(6, 28, 16);
            let whole = mine.into_inner().unwrap();
            assert!(whole.len() > 70);
            for (z, x, y, b) in whole.iter().filter(|m| m.0 >= 9) {
                assert_eq!(s.get(*z, *x, *y).unwrap().as_ref(), Some(b), "{z}/{x}/{y} as one run makes it");
            }
            let mid = read_mid(&out.path(out.get(&mid_logical(28, 16)).unwrap())).unwrap().1;
            let lo: Vec<&Made> = whole.iter().filter(|m| m.0 < 9).collect();
            assert_eq!(mid.iter().collect::<Vec<_>>().len(), lo.len());
            assert!(mid.iter().all(|m| lo.contains(&m)), "its mid as one run makes it");
        }
        // z14 → z13 → z12: each parent's pixels the mean of the four beneath at full precision (its
        // children's values as made, not as stored), coded once; its own terrain's beside them.
        let get = |z: u8, x: u32, y: u32| {
            let deep = [
                (13, 3584, 2048),
                (13, 3585, 2048),
                (13, 3584, 2049),
                (13, 3585, 2049),
                (14, 7168, 4096),
                (14, 7169, 4096),
            ];
            ((z == 12 && (1792..1800).contains(&x) && (1024..1032).contains(&y)) || deep.contains(&(z, x, y)))
                .then(|| hill(z, x, y))
        };
        // (A deep tile's border from the deep tiles alone: `fallback_floor`.)
        let deep_get = |z: u8, x: u32, y: u32| if z >= FIRST_Z { get(z, x, y) } else { None };
        let leaf = |z: u8, x: u32, y: u32| shade_direct(&deep_get, z, x, y).unwrap();
        let z13 = |x: u32, y: u32| {
            let kids: Vec<((u32, u32), Vec<Px>)> = if (x, y) == (3584, 2048) {
                [(7168, 4096), (7169, 4096)].iter().map(|&(cx, cy)| ((cx, cy), quadrant(&leaf(14, cx, cy)))).collect()
            } else {
                Vec::new()
            };
            compose(&kids, || Some(leaf(13, x, y))).unwrap()
        };
        let z12: Vec<((u32, u32), Vec<Px>)> = [(3584, 2048), (3585, 2048), (3584, 2049), (3585, 2049)]
            .iter()
            .map(|&(x, y)| ((x, y), quadrant(&z13(x, y))))
            .collect();
        let want12 = compose(&z12, || None).unwrap();
        assert_eq!(
            roadcore::shade::rgb(&at(12, 1792, 1024).unwrap()),
            roadcore::shade::rgb(&want12),
            "z12 from its z13 tiles"
        );
        assert_eq!(
            roadcore::shade::rgb(&at(13, 3584, 2048).unwrap()),
            roadcore::shade::rgb(&z13(3584, 2048)),
            "z13 from its z14 tiles and its own"
        );
        // (A tile's own terrain's shading, inside it: its neighbours don't matter there.)
        let own =
            |z, x, y| shade_tile(&Terrain::new(&|z, x, y| Some(hill(z, x, y)), &|_, _, _| true), z, x, y).unwrap();
        let rgb = |v: &[Px], j: usize, i: usize| roadcore::shade::code(v[j * TS + i]);
        // (The deep detail reaches the z12 level everywhere the deep tiles are: unlike z12's own.)
        assert_ne!(roadcore::shade::rgb(&at(12, 1792, 1024).unwrap()), roadcore::shade::rgb(&own(12, 1792, 1024)));
        drop(s);
        // A deep mid as written reads back bit for bit.
        let (tm, mid) = read_deep_mid(&out.path(out.get(&deep_mid_logical(448, 256)).unwrap())).unwrap();
        let p = d.path().join("again.sect");
        write_deep_mid(&p, tm, &mid).unwrap();
        assert_eq!(read_deep_mid(&p).unwrap(), (tm, mid.clone()));
        let tops: Vec<(u32, u32)> = mid.quadrants.keys().copied().collect();
        assert_eq!(tops, [(3584, 2048), (3584, 2049), (3585, 2048), (3585, 2049)], "its z13 tiles' quadrants");
        assert!(mid.quadrants.values().all(Option::is_some));
        // A deep mid of other terrain (a z13 tile its terrain deep pack doesn't have): refused.
        let mut other = mid.clone();
        other.quadrants.remove(&(3585, 2049));
        let c = out.get(&deep_mid_logical(448, 256)).unwrap().to_string();
        write_deep_mid(&p, tm, &other).unwrap();
        out.put_file(&deep_mid_logical(448, 256), "sect", &p).unwrap();
        let e = format!("{:#}", build_piece(&mut out, (28, 16), false, &|_, _, _| {}).unwrap_err());
        assert!(e.contains("aren't those of 10/448/256's terrain deep pack"), "{e}");
        out.set(&deep_mid_logical(448, 256), Some(c));
        // Made again expecting the same: nothing changes.
        let before = out.manifest.clone();
        out.remove(&mid_logical(28, 16));
        build_piece(&mut out, (28, 16), true, &|_, _, _| {}).unwrap();
        assert_eq!(out.manifest, before);
        // The deep terrain gone: its slope-deep run drops its deep pack; its z6 tile, expected the
        // same, is refused (its z12 isn't); made, its z12 tile its own terrain's again.
        out.remove(&crate::terrain_deep::pack_logical(448, 256));
        out.save().unwrap();
        let r = build_deep(&mut out, (448, 256), &|_, _, _| {}).unwrap();
        assert_eq!((r.deep_packs, r.deep_dropped), (0, 1));
        out.save().unwrap();
        assert!(out.get(&deep_logical(448, 256)).is_none());
        let e = build_piece(&mut out, (28, 16), true, &|_, _, _| {}).unwrap_err().to_string();
        assert!(e.contains(&hi_logical(28, 16)) && e.contains("nothing uploaded"), "{e}");
        build_piece(&mut out, (28, 16), false, &|_, _, _| {}).unwrap();
        let s = ManifestTiles::new(&out, "slope");
        let z12 = roadcore::shade::decode(&s.get(12, 1792, 1024).unwrap().unwrap()).unwrap();
        assert_eq!(rgb(&z12, 100, 100), rgb(&own(12, 1792, 1024), 100, 100));
        // No slope-deep target any more: its deep mid goes too.
        assert_eq!(drop_deep(&mut out, (448, 256)), 1);
        assert!(out.get(&deep_mid_logical(448, 256)).is_none());
    }

    /// Task #928: a batch's targets (`DeepBatch`) come out as each made alone (`build_deep`); the
    /// next target's terrain deep pack is copied ahead while one is made, read there by that target
    /// (copied once), and kept for it alone: let go once it's made, though a neighbour after reads its
    /// edge tiles (then on the NAS's); every copy let go after the last.
    #[test]
    fn a_deep_batch_is_its_targets_made_alone() {
        use std::os::unix::fs::MetadataExt;
        // (Targets `ts` as a batch, or each made alone.)
        let made = |ts: &[(u32, u32)], batch: bool| {
            let d = tempfile::tempdir().unwrap();
            let mut out =
                Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.path().join("root")), &d.path().join("scratch"))
                    .unwrap();
            terrain(&mut out, true);
            match batch {
                true => {
                    let copies = out.scratch_file("terrain-copies");
                    let name = |l: &str| out.path(out.get(l).unwrap()).file_name().unwrap().to_owned();
                    let deep = copies.join(name(&crate::terrain_deep::pack_logical(448, 256)));
                    let mut b = DeepBatch::new(&out, ts);
                    let seen = Mutex::new(HashSet::new());
                    for (k, &t) in ts.iter().enumerate() {
                        // (As it's made, the copy it reads: the one made ahead, the same file.)
                        b.build(&mut out, t, &|_, _, _| {
                            seen.lock().unwrap().extend(deep.metadata().ok().map(|m| m.ino()));
                        })
                        .unwrap();
                        // (The copying ahead ended.)
                        if let Some(h) = b.ahead.take() {
                            h.join().unwrap();
                        }
                        out.save().unwrap();
                        let next = ts.get(k + 1) == Some(&(448, 256));
                        assert_eq!(deep.exists(), next, "10/448/256's deep pack kept for the next target alone ({k})");
                    }
                    assert_eq!(seen.into_inner().unwrap().len(), 1, "10/448/256's deep pack copied once");
                    assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 0, "every copy let go");
                }
                false => {
                    for &t in ts {
                        build_deep(&mut out, t, &|_, _, _| {}).unwrap();
                    }
                }
            }
            out.manifest
                .into_iter()
                .filter(|(l, _)| l.starts_with(DEEP_PREFIX) || l.starts_with(DEEP_MID_PREFIX))
                .collect::<Vec<_>>()
        };
        // (10/448/256 between its edge neighbours: copied ahead for it, read by the one before as
        // it's copied and by the one after on the NAS.)
        let ts = [(449, 256), (448, 256), (448, 257)];
        let alone = made(&ts, false);
        assert_eq!(alone.len(), 4, "a deep pack and three deep mids: {alone:?}");
        assert_eq!(made(&ts, true), alone);
    }

    /// A pyramid's tiles don't depend on its threads' order: made again and again, the same bytes,
    /// and each parent the mean of its children as made, coded once.
    #[test]
    fn a_pyramid_is_the_same_whatever_its_threads_do() {
        let own = |z: u8, x: u32, y: u32| -> Option<Vec<Px>> {
            (z >= 4 || (x + y) % 3 != 0).then(|| {
                (0..TS * TS)
                    .map(|i| {
                        let t = (i as f32 * 0.37 + x as f32 * 1.3 + y as f32 * 0.7 + z as f32).sin();
                        Px::of_gradient(t * 0.9, (t * 2.1).cos() * 0.6)
                    })
                    .collect()
            })
        };
        let mut tiles = HashSet::new();
        for z in 0..=5u8 {
            for x in 0..1u32 << z {
                for y in 0..1u32 << z {
                    if z < 4 || (x + 2 * y) % 5 != 0 {
                        tiles.insert((z, x, y));
                    }
                }
            }
        }
        let run = || {
            let out = Mutex::new(Vec::new());
            Pyramid { own: &own, tiles: &tiles, out: &out, tick: &|| {}, done: &none_done }.tile(0, 0, 0);
            let mut v = out.into_inner().unwrap();
            v.sort_by_key(|m| (m.0, m.1, m.2));
            v
        };
        let first = run();
        assert!(first.len() > 500);
        for _ in 0..2 {
            assert!(run() == first);
        }
        // Its z2 tiles made as tops, each subtree on one thread (a slope-deep run's z13 tiles): the
        // same tiles from z2 down, their quadrants those the z1 tiles are made from.
        let tops: Vec<(u32, u32)> = (0..4u32).flat_map(|x| (0..4u32).map(move |y| (x, y))).collect();
        let out = Mutex::new(Vec::new());
        let qs = Pyramid { own: &own, tiles: &tiles, out: &out, tick: &|| {}, done: &none_done }.tops(2, &tops);
        let mut v = out.into_inner().unwrap();
        v.sort_by_key(|m| (m.0, m.1, m.2));
        assert!(v == first.iter().filter(|m| m.0 >= 2).cloned().collect::<Vec<_>>());
        // (And handed in as made: the same pyramid.)
        let by: HashMap<(u32, u32), Option<Vec<Px>>> = tops.into_iter().zip(qs).collect();
        let done = |z: u8, x: u32, y: u32| (z == 2).then(|| by[&(x, y)].clone());
        let out = Mutex::new(Vec::new());
        Pyramid { own: &own, tiles: &tiles, out: &out, tick: &|| {}, done: &done }.tile(0, 0, 0);
        let mut above = out.into_inner().unwrap();
        above.sort_by_key(|m| (m.0, m.1, m.2));
        assert!(above == first.iter().filter(|m| m.0 < 2).cloned().collect::<Vec<_>>());
    }

    /// The memory bounds are those of the tiles a run holds.
    #[test]
    fn the_memory_bounds_count_the_tiles_held() {
        assert_eq!(tiles_below(3, 6), 64 + 256 + 1024 + 4096);
        // (The pyramid's working memory: 16 threads, each a chain of four tiles' children's
        // quadrants and one tile's making (3 tiles' pixels, the coverage, Horn's border, five terrain
        // tiles, the codes); the 21 tiles of the parallel levels' children's quadrants.)
        let making = 3 * 786_432 + 65_536 + 258 * 258 * 4 + 5 * 262_144 + 196_608;
        let chains = 16 * (4 * 4 * 196_608 + making);
        let working = chains + 21 * 4 * 196_608;
        assert_eq!(working_bytes(6, HI_MAX), working);
        assert_eq!(chains_bytes(FIRST_Z, MAX_Z), chains, "a z13 subtree a thread");
        // (Beside a target: the one before's uploads in flight, an 8 MB read at a time; slope-deep's
        // next target's copy ahead, 1 MB at a time. Slope-hi: a deep mid open a thread, its 64
        // quadrants and its bytes as read.)
        let (upload, copy) = (8 << 20, 1 << 20);
        assert_eq!(peak_mb(), ((5440 + 21) * 196_608 + 16 * 2 * 64 * 196_608 + working + 512 * 262_144 + upload) >> 20);
        assert_eq!(deep_peak_mb(), (5440 * 196_608 + 64 * 196_608 + chains + 512 * 262_144 + upload + copy) >> 20);
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
        let mut fresh = Out::unbound(
            &store::nasfs::NasRoot::local_for_test(&d.path().join("fresh")),
            &d.path().join("fresh-scratch"),
        )
        .unwrap();
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
        let mut again = Out::unbound(
            &store::nasfs::NasRoot::local_for_test(&d.path().join("again")),
            &d.path().join("again-scratch"),
        )
        .unwrap();
        terrain(&mut again);
        let old = vec![(6, 24, 23, z6.clone(), z6.len() as u32), (7, 48, 46, z7.clone(), z7.len() as u32)];
        let enc = crate::shade_codec::ENCODING;
        crate::layers::write_pack(&mut again, "slope", enc, false, "lo", (3, 3, 2), &mut old.into_iter()).unwrap();
        again.save().unwrap();
        build_piece(&mut again, (28, 16), false, &|_, _, _| {}).unwrap();
        build_lo(&mut again, (3, 2), &[(28, 16)], &|_, _, _| {}).unwrap();
        assert_eq!(again.get(lo), fresh.get(lo), "the same lo pack as a fresh build's");
        // Task #889: paused at every one of its safe points (each row of z6 tiles shaded kept),
        // resumed each time: the same lo pack, nothing of it put before its end.
        let mut paused = Out::unbound(
            &store::nasfs::NasRoot::local_for_test(&d.path().join("paused")),
            &d.path().join("paused-scratch"),
        )
        .unwrap();
        terrain(&mut paused);
        build_piece(&mut paused, (28, 16), false, &|_, _, _| {}).unwrap();
        let mut stops = 0;
        loop {
            crate::resume_log::STOP_AFTER.set(Some(1));
            let r = build_lo_keeping(&mut paused, (3, 2), &[(28, 16)], &|_, _, _| {}, true);
            crate::resume_log::STOP_AFTER.set(None);
            match r {
                Err(e) => {
                    assert!(crate::control::is_paused(&e), "{e:#}");
                    assert!(paused.get(lo).is_none(), "nothing of it put");
                    stops += 1;
                }
                Ok(_) => break,
            }
        }
        assert!(stops > 2, "several safe points: {stops}");
        assert_eq!(paused.get(lo), fresh.get(lo), "paused at each of its {stops} safe points");
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
    /// left drops its hi pack and mid (`drop_piece`), its z10 tiles' slope-deep targets their deep
    /// packs (`drop_deep`); a "none" assembly its lo pack.
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
        let mut out =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.path().join("root")), &d.path().join("scratch"))
                .unwrap();
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
        assert_eq!(drop_piece(&mut out, (24, 23)).unwrap(), 2);
        assert!(out.get(&lhi).is_none() && out.get(&lmid).is_none() && out.get(&ldeep).is_some());
        assert_eq!(drop_deep(&mut out, (24 * 16 + 3, 23 * 16 + 15)), 1);
        assert!(out.get(&ldeep).is_none());
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
        let mut out =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.path().join("root")), &d.path().join("scratch"))
                .unwrap();
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
        out.path(&theirs).remove_file().unwrap();
        assert!(read_mids(&out, q, &ts[1..2]).is_err());
        // A z6 tile outside q: refused before anything is read.
        assert!(read_mids(&out, (0, 0), &ts).unwrap_err().to_string().contains("isn't in 3/0/0"));
    }

    /// A measure (run by hand, read-only: nothing written to the root): a real slope z6 tile (`piece
    /// 6/x/y`: its terrain packs copied here, its tiles worked out), slope-deep z10 tile (`deep
    /// 10/x/y`: as its run makes it, nothing uploaded), assembly (`lo 3/x/y`: its z6
    /// tiles' mids and the terrain read, its tiles worked out) or z3 tile whole (`z3 3/x/y`: the z6
    /// tiles given, else each with a terrain hi pack, then the assembly from their tiles as made),
    /// timed, with the CPU it took, the bytes of the tiles by zoom and a digest of them (the same
    /// before and after a change). With a folder, its tiles are written there as
    /// `<z>/<x>/<y>.webp` too.
    /// SCENIC_SLOPE_MEASURE="<root> piece|deep|lo|z3 <x>/<y> [<folder>] [<z6 tiles: x/y,…>]"; unset,
    /// nothing.
    #[test]
    fn measure_a_real_target() {
        let Ok(m) = std::env::var("SCENIC_SLOPE_MEASURE") else { return };
        jobrt::sys::footprint_guard("the slope measure", 4500);
        let a: Vec<&str> = m.split_whitespace().collect();
        let root = std::path::Path::new(a[0]);
        let xy = |s: &str| {
            let (x, y) = s.split_once('/').unwrap();
            (x.parse::<u32>().unwrap(), y.parse::<u32>().unwrap())
        };
        let t = xy(a[2]);
        let scratch = tempfile::tempdir().unwrap();
        let out = Out::unbound(&store::nasfs::NasRoot::local_for_test(root), scratch.path()).unwrap();
        let cpu = || {
            let mut u: libc::rusage = unsafe { std::mem::zeroed() };
            unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
            let s = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
            s(u.ru_utime) + s(u.ru_stime)
        };
        crate::timings::start(if a[1] == "lo" { "slope-lo" } else { "slope" });
        let (t0, c0) = (std::time::Instant::now(), cpu());
        let none = |_: &str, _: u64, _: u64| {};
        let mut h = <sha2::Sha256 as sha2::Digest>::new();
        let mut by_z: BTreeMap<u8, (usize, usize)> = BTreeMap::new();
        let mut n = 0;
        let mut put = |made: &[Made]| {
            for (z, x, y, b) in made {
                sha2::Digest::update(&mut h, format!("{z}/{x}/{y} {}\n", b.len()));
                sha2::Digest::update(&mut h, b);
                let e = by_z.entry(*z).or_default();
                *e = (e.0 + 1, e.1 + b.len());
                n += 1;
                if let Some(dir) = a.get(3) {
                    let p = std::path::Path::new(dir).join(format!("{z}/{x}"));
                    std::fs::create_dir_all(&p).unwrap();
                    std::fs::write(p.join(format!("{y}.webp")), b).unwrap();
                }
            }
        };
        let terr = ManifestTiles::with_copies(&out, "terrain", out.scratch_file("terrain-copies"));
        let (get, has) = terrain_reads(&terr);
        let terrain = Terrain::new(&get, &has);
        // A z6 tile's tiles (its z10 tiles' deep parts first, as their slope-deep runs make them)
        // put, its z6–8 tiles kept for the assembly.
        // (Its z10 tiles' deep mids written here, and read back as its slope-hi run reads them.)
        let mid_path = |x: u32, y: u32| scratch.path().join(format!("deep-mid-{x}-{y}.sect"));
        let piece_of = |t: (u32, u32), put: &mut dyn FnMut(&[Made])| -> Vec<Made> {
            let k = PACK_Z - 6;
            let mut tops = BTreeMap::new();
            let p = crate::timings::phase("its z10 tiles' slope-deep runs");
            for x in t.0 << k..(t.0 + 1) << k {
                for y in t.1 << k..(t.1 + 1) << k {
                    let deep = deep_terrain(&terr, (x, y)).unwrap();
                    if deep.is_empty() {
                        continue;
                    }
                    terr.copy_here(FIRST_Z, x << 3, y << 3);
                    let tiles = deep_tiles(&deep, (x, y));
                    let (d, mid) = deep_part(&terrain, &tiles, &Count::new(1, &none));
                    put(&d);
                    write_deep_mid(&mid_path(x, y), (x, y), &mid).unwrap();
                    tops.insert((x, y), deep_tops(&tiles));
                }
            }
            drop(p);
            let p = crate::timings::phase("its slope-hi run");
            terr.copy_here(9, t.0 << 3, t.1 << 3);
            let read = |f: (u32, u32)| Ok(read_deep_mid(&mid_path(f.0, f.1))?.1);
            let deep = DeepQuadrants {
                read: &read,
                of: tops.keys().copied().collect(),
                open: Default::default(),
                failed: Default::default(),
            };
            let done = |z: u8, x: u32, y: u32| if z == FIRST_Z { deep.take(x, y) } else { None };
            let tiles = piece_tiles(&has, &tops, t);
            let (hi, lo) = piece(&terrain, &tiles, &done, t, &Count::new(1, &none));
            assert!(deep.failed.into_inner().unwrap().is_none());
            drop(p);
            put(&hi);
            put(&lo);
            lo
        };
        match a[1] {
            "lo" => {
                let ts: Vec<(u32, u32)> = (0..64)
                    .map(|k| (t.0 * 8 + k % 8, t.1 * 8 + k / 8))
                    .filter(|&(x, y)| out.get(&hi_logical(x, y)).is_some() || out.get(&mid_logical(x, y)).is_some())
                    .collect();
                let mids = read_mids(&out, t, &ts).unwrap();
                put(&assemble(
                    &out,
                    &terrain,
                    t,
                    &ts,
                    &mids,
                    &Count::new(85, &none),
                    &mut crate::resume_log::Pieces::none(),
                )
                .unwrap());
            }
            "z3" => {
                let ts: Vec<(u32, u32)> = match a.get(4) {
                    Some(l) => l.split(',').map(xy).collect(),
                    None => (0..64)
                        .map(|k| (t.0 * 8 + k % 8, t.1 * 8 + k / 8))
                        .filter(|&(x, y)| out.get(&crate::layers::hi_logical("terrain", x, y)).is_some())
                        .collect(),
                };
                let mut mids = BTreeMap::new();
                for &p in &ts {
                    mids.insert(p, piece_of(p, &mut put));
                    eprintln!("6/{}/{} made ({:.0} s)", p.0, p.1, t0.elapsed().as_secs_f64());
                }
                let lo = assemble(
                    &out,
                    &terrain,
                    t,
                    &ts,
                    &mids,
                    &Count::new(85, &none),
                    &mut crate::resume_log::Pieces::none(),
                )
                .unwrap();
                put(&lo
                    .into_iter()
                    .filter(|m| m.0 < 6 || !ts.contains(&(m.1 >> (m.0 - 6), m.2 >> (m.0 - 6))))
                    .collect::<Vec<_>>());
            }
            "deep" => {
                // As its slope-deep run makes it (`build_deep`), nothing uploaded: the terrain packs
                // it copies, the tiles it reads from the NAS besides, its deep pack's and deep mid's
                // bytes.
                let p = crate::timings::phase("copying its terrain packs here");
                let copied = terr.copy_here(FIRST_Z, t.0 << 3, t.1 << 3).unwrap_or(0);
                drop(p);
                let local = |z: u8, x: u32, y: u32| z >= FIRST_Z && (x >> (z - PACK_Z), y >> (z - PACK_Z)) == t;
                let nas = std::sync::atomic::AtomicU64::new(0);
                let counted = |z: u8, x: u32, y: u32| {
                    let b = get(z, x, y);
                    if !local(z, x, y) {
                        nas.fetch_add(b.as_ref().map_or(0, |b| b.len() as u64), std::sync::atomic::Ordering::Relaxed);
                    }
                    b
                };
                let terrain = Terrain::new(&counted, &has);
                let p = crate::timings::phase("working out the slope tiles");
                let tiles = deep_tiles(&deep_terrain(&terr, t).unwrap(), t);
                let (d, mid) = deep_part(&terrain, &tiles, &Count::new(1, &none));
                drop(p);
                let path = scratch.path().join("deep-mid.sect");
                write_deep_mid(&path, t, &mid).unwrap();
                let pack: u64 = d.iter().map(|m| m.3.len() as u64).sum();
                put(&d);
                println!(
                    "deep {}: copied {:.1} MB, read from the NAS besides {:.1} MB; deep pack's tiles {:.1} MB, deep mid {:.1} MB",
                    a[2],
                    copied as f64 / 1e6,
                    nas.into_inner() as f64 / 1e6,
                    pack as f64 / 1e6,
                    std::fs::metadata(&path).unwrap().len() as f64 / 1e6
                );
            }
            _ => {
                piece_of(t, &mut put);
            }
        }
        let (wall, cpu) = (t0.elapsed().as_secs_f64(), cpu() - c0);
        let r = crate::timings::snapshot(true).unwrap();
        // (The process's peak physical footprint, compressed and swapped memory included.)
        let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        unsafe {
            libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V4,
                &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
            )
        };
        println!(
            "{} {}: {n} tiles in {wall:.1} s, CPU {cpu:.1} s ({:.2} cores), peak footprint {} MB; digest {:x}",
            a[1],
            a[2],
            cpu / wall,
            info.ri_lifetime_max_phys_footprint >> 20,
            sha2::Digest::finalize(h)
        );
        for (z, (n, b)) in by_z {
            println!("  z{z}: {n} tiles, {:.1} KB a tile", b as f64 / n as f64 / 1024.0);
        }
        for p in &r.phases {
            println!("  {:<34} {:>8.1} s  CPU {:>6.1} s  n={}", p.name, p.wall_s, p.cpu_s.unwrap_or(0.0), p.n);
        }
    }

    /// A measure (run by hand; the root only read): real slope-deep z10 tiles (`<x/y,…>`) made as
    /// the step makes a batch of them (`DeepBatch`, each one's uploads landing beside the next one's
    /// work), or with `alone` as it made each before task #928 (its own reader, its packs copied
    /// then, its uploads waited for). Their uploads go to `<into>` (a local folder of its own: never
    /// the root, nor the NAS). Each target's time and its outputs' content names (the same bytes either way), then
    /// the batch's wall, CPU, the share of its threads kept busy and its peak footprint (sampled
    /// every 50 ms; past 4 GB it stops).
    /// SCENIC_SLOPE_DEEP_MEASURE="<root> <into> <x/y,…> [alone]"; unset, nothing.
    #[test]
    fn measure_a_slope_deep_batch() {
        let Ok(m) = std::env::var("SCENIC_SLOPE_DEEP_MEASURE") else { return };
        let a: Vec<&str> = m.split_whitespace().collect();
        let (root, into) = (std::path::Path::new(a[0]), std::path::Path::new(a[1]));
        assert!(!into.starts_with(root), "{} is inside the root", into.display());
        let ts: Vec<(u32, u32)> = a[2]
            .split(',')
            .map(|s| {
                let (x, y) = s.split_once('/').unwrap();
                (x.parse().unwrap(), y.parse().unwrap())
            })
            .collect();
        let alone = a.get(3) == Some(&"alone");
        crate::sys::footprint_guard("measure_a_slope_deep_batch", 4000);
        let peak = Arc::new(std::sync::atomic::AtomicU64::new(0));
        {
            let peak = peak.clone();
            std::thread::spawn(move || loop {
                peak.fetch_max(crate::sys::footprint().unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(50));
            });
        }
        let cpu = || {
            let mut u: libc::rusage = unsafe { std::mem::zeroed() };
            unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
            let s = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
            s(u.ru_utime) + s(u.ru_stime)
        };
        let scratch = tempfile::tempdir().unwrap();
        // (The root only read: its records and terrain packs, nothing written there.)
        let read = Out::unbound(&store::nasfs::NasRoot::from_arg(root), scratch.path()).unwrap();
        std::fs::create_dir_all(into).unwrap();
        let mut w = Out::unbound(&store::nasfs::NasRoot::local_for_test(into), scratch.path()).unwrap();
        crate::timings::start("slope-deep");
        let none = |_: &str, _: u64, _: u64| {};
        let threads = rayon::current_num_threads();
        let (t0, c0) = (std::time::Instant::now(), cpu());
        let mut batch = (!alone).then(|| DeepBatch::new(&read, &ts));
        let mut before: Option<crate::out::Mark> = None;
        for &t in &ts {
            let (s, c) = (std::time::Instant::now(), cpu());
            let r = match &mut batch {
                Some(b) => {
                    let r = b.build(&mut w, t, &none).unwrap();
                    if let Some(m) = before.take() {
                        w.save_upto(m).unwrap();
                    }
                    before = Some(w.mark());
                    r
                }
                None => {
                    let r = DeepBatch::new(&read, &[t]).build(&mut w, t, &none).unwrap();
                    w.save().unwrap();
                    r
                }
            };
            let (wall, cpu) = (s.elapsed().as_secs_f64(), cpu() - c);
            println!(
                "10/{}/{}: {} tiles, {wall:.1} s, CPU {cpu:.1} s ({:.0} % of {threads} threads)",
                t.0,
                t.1,
                r.deep_tiles,
                100.0 * cpu / wall / threads as f64
            );
        }
        w.save().unwrap();
        let (wall, cpu) = (t0.elapsed().as_secs_f64(), cpu() - c0);
        println!(
            "{} targets{}: {wall:.1} s ({:.1} s a target), CPU {cpu:.1} s, {:.0} % of {threads} threads busy, peak {} MB",
            ts.len(),
            if alone { " alone" } else { " as a batch" },
            wall / ts.len() as f64,
            100.0 * cpu / wall / threads as f64,
            peak.load(std::sync::atomic::Ordering::Relaxed) >> 20
        );
        for &t in &ts {
            println!(
                "  10/{}/{}: {:?} {:?}",
                t.0,
                t.1,
                w.get(&deep_logical(t.0, t.1)),
                w.get(&deep_mid_logical(t.0, t.1))
            );
        }
        let r = crate::timings::snapshot(true).unwrap();
        for p in &r.phases {
            let bg = if p.background { " (beside)" } else { "" };
            println!(
                "  {:<48} {:>7.1} s  CPU {:>6.1} s  n={} {} MB{bg}",
                p.name,
                p.wall_s,
                p.cpu_s.unwrap_or(0.0),
                p.n,
                p.bytes >> 20
            );
        }
    }

    /// A measure (run by hand, read-only): how far the pyramid as built (each level the mean of its
    /// children as stored, so coded again at every level) drifts from the same pyramid made from
    /// full-precision means coded once at each level, over z6 tiles' z12 terrain down to z3; and a
    /// hybrid, full precision inside a job and coded only where a job hands its tiles on (z6 to its
    /// assembly, z3 to the root). Per level: R's codes (and the steepness's mean bias), the
    /// direction's (G, B), pixels neutral in one and not the other, and the hill-shading's brightness
    /// (GDAL's basic method, light from 315° at 45°, as the map's shader shades a pixel), in 255ths.
    /// SCENIC_SHADE_DRIFT="<root> <x/y>[,<x/y>…]"; unset, nothing.
    #[test]
    fn measure_quantisation_drift() {
        use roadcore::shade::{code, mean4, value};
        let Ok(m) = std::env::var("SCENIC_SHADE_DRIFT") else { return };
        let a: Vec<&str> = m.split_whitespace().collect();
        let root = std::path::Path::new(a[0]);
        let scratch = tempfile::tempdir().unwrap();
        let out = Out::unbound(&store::nasfs::NasRoot::local_for_test(root), scratch.path()).unwrap();
        let q = |p: Px| value(code(p));
        let shade = |p: Px| -> f32 {
            let (alt, az) = (45f32.to_radians(), 315f32.to_radians() + std::f32::consts::PI);
            let basic = |gx: f32, gy: f32| {
                let c = (alt.sin() - (gy * az.cos() * alt.cos() - gx * az.sin() * alt.cos()))
                    / (1.0 + gx * gx + gy * gy).sqrt();
                c.clamp(0.0, 1.0)
            };
            let s = p.s / 100.0;
            let c = p.coherence().min(1.0);
            let dir = if c > 0.0 { basic(s * p.u[0] / c, s * p.u[1] / c) } else { 0.0 };
            let rough = (0..16)
                .map(|k| {
                    let t = (k as f32 + 0.5) * std::f32::consts::TAU / 16.0;
                    basic(s * t.cos(), s * t.sin())
                })
                .sum::<f32>()
                / 16.0;
            c * dir + (1.0 - c) * rough
        };
        // Per level (z12 − k): [n, Σ|ΔR|, max ΔR, ΔR≠0, Σ Δsteepness, Σ|ΔG|+|ΔB|, max, neutral mismatch, Σ|Δshade|, max]
        type St = [f64; 10];
        let add = |st: &mut St, got: Px, want_full: Px| {
            let (g, w) = (code(got), code(want_full));
            let dr = (g[0] as f64 - w[0] as f64).abs();
            let dd = (g[1] as f64 - w[1] as f64).abs() + (g[2] as f64 - w[2] as f64).abs();
            let ng = g[1] == 128 && g[2] == 128;
            let nw = w[1] == 128 && w[2] == 128;
            let ds = (shade(value(g)) - shade(value(w))).abs() as f64 * 255.0;
            st[0] += 1.0;
            st[1] += dr;
            st[2] = st[2].max(dr);
            st[3] += (dr > 0.0) as u8 as f64;
            st[4] += (value(g).s - value(w).s) as f64;
            st[5] += dd;
            st[6] = st[6].max(dd);
            st[7] += (ng != nw) as u8 as f64;
            st[8] += ds;
            st[9] = st[9].max(ds);
        };
        let merge = |a: &mut St, b: &St| {
            for i in 0..10 {
                if i == 2 || i == 6 || i == 9 {
                    a[i] = a[i].max(b[i])
                } else {
                    a[i] += b[i]
                }
            }
        };
        let down = |v: &[Px], w: usize| -> Vec<Px> {
            let h = w / 2;
            (0..h * h)
                .map(|p| {
                    let (j, i) = (p / h, p % h);
                    let (a, b) = (2 * j * w + 2 * i, (2 * j + 1) * w + 2 * i);
                    mean4(v[a], v[a + 1], v[b], v[b + 1])
                })
                .collect()
        };
        for t in a[1].split(',') {
            let (tx, ty) = t.split_once('/').unwrap();
            let t: (u32, u32) = (tx.parse().unwrap(), ty.parse().unwrap());
            let terr = ManifestTiles::with_copies(&out, "terrain", out.scratch_file("terrain-copies"));
            terr.copy_here(9, t.0 << 3, t.1 << 3);
            let (get, has) = terrain_reads(&terr);
            let terrain = Terrain::new(&get, &has);
            let t0 = std::time::Instant::now();
            // Each z12 tile: chains Q (coded every level), F (full) and H (full, coded at z6) to z4.
            let per: Vec<((u32, u32), [Px; 3], Vec<(St, St)>)> = (0..64 * 64u32)
                .into_par_iter()
                .map(|k| {
                    let (x, y) = (t.0 * 64 + k % 64, t.1 * 64 + k / 64);
                    let f0 = shade_tile(&terrain, 12, x, y).unwrap_or_else(|| vec![Px::default(); TS * TS]);
                    let mut qv: Vec<Px> = f0.iter().map(|&p| q(p)).collect();
                    let mut fv = f0;
                    let mut hv = fv.clone();
                    let mut st = Vec::new();
                    let mut w = TS;
                    loop {
                        let (mut sq, mut sh) = ([0f64; 10], [0f64; 10]);
                        for i in 0..w * w {
                            add(&mut sq, qv[i], fv[i]);
                            add(&mut sh, hv[i], fv[i]);
                        }
                        st.push((sq, sh));
                        if w == 1 {
                            break;
                        }
                        qv = down(&qv, w).into_iter().map(q).collect();
                        fv = down(&fv, w);
                        hv = down(&hv, w);
                        w /= 2;
                        if w == 4 {
                            hv = hv.into_iter().map(q).collect();
                        } // z6: handed to the assembly
                    }
                    ((x, y), [qv[0], fv[0], hv[0]], st)
                })
                .collect();
            let mut levels: Vec<(St, St)> = vec![([0.0; 10], [0.0; 10]); 10];
            for (_, _, st) in &per {
                for (k, (a, b)) in st.iter().enumerate() {
                    merge(&mut levels[k].0, a);
                    merge(&mut levels[k].1, b);
                }
            }
            // z4 → z3 over the 64 × 64 tiles' pixels.
            let grid = |c: usize| -> Vec<Px> {
                let mut g = vec![Px::default(); 64 * 64];
                for ((x, y), v, _) in &per {
                    g[((y - t.1 * 64) * 64 + (x - t.0 * 64)) as usize] = v[c];
                }
                g
            };
            let (qg, fg, hg) = (grid(0), grid(1), grid(2));
            let (q3, f3, h3): (Vec<Px>, Vec<Px>, Vec<Px>) =
                (down(&qg, 64).into_iter().map(q).collect(), down(&fg, 64), down(&hg, 64));
            let (mut sq, mut sh) = ([0f64; 10], [0f64; 10]);
            for i in 0..32 * 32 {
                add(&mut sq, q3[i], f3[i]);
                add(&mut sh, h3[i], f3[i]);
            }
            levels[9] = (sq, sh);
            println!(
                "6/{}/{} ({:.0} s): per level, as built (Q) and hybrid (H) against full precision coded once",
                t.0,
                t.1,
                t0.elapsed().as_secs_f64()
            );
            println!("  z   | Q: mean|dR| max dR  dR!=0  bias s%   mean|dGB| max  neutral≠  shade mean max (1/255) | H: mean|dR| max  dR!=0  mean|dGB| max neutral≠ shade mean max");
            for (k, (a, b)) in levels.iter().enumerate() {
                let z = 12 - k.min(8) as i32 - if k == 9 { 1 } else { 0 };
                let r = |s: &St| {
                    format!(
                        "{:>6.3} {:>3} {:>6.2}% {:>+8.4} {:>7.3} {:>3} {:>6.2}% {:>6.3} {:>5.1}",
                        s[1] / s[0],
                        s[2],
                        100.0 * s[3] / s[0],
                        s[4] / s[0],
                        s[5] / s[0],
                        s[6],
                        100.0 * s[7] / s[0],
                        s[8] / s[0],
                        s[9]
                    )
                };
                println!("  z{z:<3}| {} | {}", r(a), r(b));
            }
        }
    }
}
