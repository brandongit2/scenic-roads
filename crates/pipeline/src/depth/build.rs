//! The depth steps (docs/steps.md §3): `depth`, a z6 tile's hi packs (z9–16), then `depth-lo`, a
//! z3 tile's lo packs (z3–8), each written as packs of the three layers (`depth`, `depth-class`,
//! `depth-shade`). Heights and shading are built as the terrain's (§2, Coarser zooms from the
//! finer), with no zoom special:
//!
//! - **Composed at the finest data's zoom:** a block of zoom z is composed from the sources
//!   (crate::depth::compose) only where a grid of zoom z meets it (`compose::composes_at`), a tile
//!   kept only where a pixel is a source's own data of its zoom: GEBCO's z8, a chart's z16, a new
//!   source's whatever zoom. No tile is stored finer than the data; finer views scale up.
//! - **Every coarser level the mean of the finer** (`means`): each pixel the mean of the four
//!   beneath it that have data, where they do; elsewhere its own level composed (the same rule
//!   there).
//! - **The shading** (`shading`): crate::slope_pack's pyramid over those tiles, each place's at its
//!   finest stored level by Horn's method, each coarser level the mean of the finer.
//!
//! Where the two jobs meet is where their packs do (z8 | z9, as the terrain's), not part of the rule:
//! - **depth** (a z6 tile T, where a source has a grid finer than the lo packs' `LO`): its blocks
//!   composed z6 to z16, coarse to fine (a capped pixel is the level of its source's zoom scaled up
//!   as the map's fallback does: z6–8 composed over T and the ring of tiles around it for that
//!   alone, `coarse_areas`), then z15 down to z9 each the mean of the finer.
//! - **depth-lo** (a z3 tile Q, after its z6 tiles' depth): its blocks composed z3 to z8, then z8
//!   the mean of the z9 tiles its z6 tiles' hi packs store, z7 down to z3 each the mean of the
//!   finer; its shading over them, z8 taking the z9 shading the hi packs store where it covers.
//!
//! Deterministic: the same sources, water and z9 tiles make the same packs, byte for byte.

use super::compose::{compose, tiles, Block, Water, BLOCK, PAD};
use super::fall::{self, Grid, Kind, Stored};
use super::grids::{Reader, Source};

/// This Mac's whole copies of the depth sources' packs a reader reads (crate::localread), if any.
type Local = Option<std::sync::Arc<crate::localread::Local>>;
use super::{CLASS_ENCODING, CLASS_LAYER, ENCODING, LAYER, NODATA, NODATA_MIN};
use crate::out::Out;
use crate::terrain_water::{water_tile, Poly, WaterSource, WaterTile};
use anyhow::{Context, Result};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The depth piece's version: bumped with any change to the bytes `build_hi` writes (7: composed
/// only at a grid's own zoom, the coarser levels the mean of the finer, task #804; 6: its
/// coarse levels its own, task #532; 5: lossless WebP, task #671; 4: the
/// floor's shading, `shading`, task #632; 3: a grid lined up with no overview sampled from one
/// askew, `grids::level_for`, task #433). Its key names crate::shade_codec::version too.
pub const DEPTH_V: u32 = 7;
/// The depth assembly's (`build_lo`; 8: composed only at a grid's own zoom, z3–8 the means of the
/// finer, its shading made here, task #804; 7: z3–8 from the z9 tiles, task #532; 6: lossless
/// WebP, task #671).
pub const DEPTH_LO_V: u32 = 8;
/// How far from the coverage depth is built (km): 200 nautical miles, a region's EEZ's reach.
pub const REACH_KM: f64 = 370.4;
/// The zooms a z6 tile's hi packs hold.
pub const HI: (u8, u8) = (9, 16);
/// The zooms a z3 tile's lo packs hold.
pub const LO: (u8, u8) = (3, 8);
/// A piece's zoom: the coarse levels it composes for its capped pixels are this to `LO.1`.
pub const PIECE_Z: u8 = 6;
/// The deepest zoom the basemap has water at.
pub const WATER_Z: u8 = 14;

pub fn hi_logical(layer: &str, x: u32, y: u32) -> String {
    format!("layers/{layer}/hi/6-{x}-{y}")
}

pub fn lo_logical(layer: &str, x: u32, y: u32) -> String {
    format!("layers/{layer}/lo/3-{x}-{y}")
}

/// A tile's water polygons as the depth reads them (`WaterSource::depth_polys`, the rivers too): the
/// basemap's at z6–14, a z14 tile's scaled at z15–16.
pub fn polys_at(src: &dyn WaterSource, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
    if z <= WATER_Z {
        return src.depth_polys(z, x, y);
    }
    let d = z - WATER_Z;
    Ok(scaled(&src.depth_polys(WATER_Z, x >> d, y >> d)?, d, x, y))
}

/// Z14 tile (x >> d, y >> d)'s polygons `ps` scaled onto its descendant x/y, `d` zooms finer.
fn scaled(ps: &[Poly], d: u8, x: u32, y: u32) -> Vec<Poly> {
    let n = f64::from(1u32 << d);
    let (ox, oy) = (f64::from(x & ((1 << d) - 1)), f64::from(y & ((1 << d) - 1)));
    ps.iter()
        .map(|p| Poly {
            kind: p.kind,
            id: p.id,
            rings: p
                .rings
                .iter()
                .map(|r| r.iter().map(|pt| [(pt[0] - ox * 256.0 / n) * n, (pt[1] - oy * 256.0 / n) * n]).collect())
                .collect(),
        })
        .collect()
}

/// A tile's water from its polygons (`water_tile`), None where there's none.
fn made(ps: Vec<Poly>) -> Option<Arc<WaterTile>> {
    if ps.is_empty() {
        None
    } else {
        Some(Arc::new(water_tile(&ps))).filter(|t| !t.is_empty())
    }
}

/// The bytes of polygons `ps` hold (their rings' points, and each ring's and polygon's own).
fn polys_bytes(ps: &[Poly]) -> usize {
    ps.iter().map(|p| 48 + p.rings.iter().map(|r| 24 + r.len() * 16).sum::<usize>()).sum()
}

/// The most bytes of z14 polygons a `WaterCache` keeps (`polys_bytes`): past it, the rows farthest
/// up go first (a level's blocks go row by row down the tile, each row's blocks sharing the z14
/// tiles of the row before it: a row of a z6 tile's 256 z14 tiles holds a few MB on a coast).
pub const POLYS_BYTES: usize = 48 << 20;

/// The water of the tiles around blocks, kept while a level is made (neighbouring blocks share
/// their rings), and the z14 polygons the finer levels are drawn from, kept across them: each
/// z14 tile's are read from the basemap and decoded once for its 4 z15 and 16 z16 children. A
/// block's tiles it lacks are read together (`prefetch`: the basemap reads them in runs) and
/// rasterised on the job's threads.
pub struct WaterCache {
    tiles: HashMap<(u8, u32, u32), Option<Arc<WaterTile>>>,
    /// By (y, x): the z14 tile's polygons, shared, and their bytes.
    polys: BTreeMap<(u32, u32), (Arc<Vec<Poly>>, usize)>,
    polys_bytes: usize,
    bound: usize,
}

impl Default for WaterCache {
    fn default() -> Self {
        Self::with_bound(POLYS_BYTES)
    }
}

impl WaterCache {
    /// A cache keeping at most `bound` bytes of z14 polygons (`POLYS_BYTES` by default).
    pub fn with_bound(bound: usize) -> Self {
        WaterCache { tiles: HashMap::new(), polys: BTreeMap::new(), polys_bytes: 0, bound }
    }

    /// The bytes of z14 polygons it holds.
    pub fn polys_bytes(&self) -> usize {
        self.polys_bytes
    }

    /// A level done: its tiles let go (the z14 polygons kept for the next).
    pub fn next_level(&mut self) {
        self.tiles.clear();
    }

    /// `polys_at`, a z14 tile's polygons read once for its z15–16 descendants and shared among
    /// them (each scaled from the one copy).
    fn polys(&mut self, src: &dyn WaterSource, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
        if z <= WATER_Z {
            return src.depth_polys(z, x, y);
        }
        let d = z - WATER_Z;
        let key = (y >> d, x >> d);
        let base = match self.polys.get(&key) {
            Some((p, _)) => p.clone(),
            None => {
                let p = Arc::new(src.depth_polys(WATER_Z, key.1, key.0)?);
                let bytes = polys_bytes(&p);
                self.polys.insert(key, (p.clone(), bytes));
                self.polys_bytes += bytes;
                // (Past the bound: the others go, the rows farthest up first, until it fits; this
                // tile's own stays whatever the bound.)
                while self.polys_bytes > self.bound {
                    let Some(k) = self.polys.keys().find(|k| **k != key).copied() else { break };
                    if let Some((_, b)) = self.polys.remove(&k) {
                        self.polys_bytes -= b;
                    }
                }
                p
            }
        };
        Ok(scaled(&base, d, x, y))
    }

    fn tile(&mut self, src: &dyn WaterSource, z: u8, x: u32, y: u32) -> Result<Option<Arc<WaterTile>>> {
        if let Some(t) = self.tiles.get(&(z, x, y)) {
            return Ok(t.clone());
        }
        let ps = self.polys(src, z, x, y)?;
        let t = made(ps);
        if self.tiles.len() > 4096 {
            self.tiles.clear();
        }
        self.tiles.insert((z, x, y), t.clone());
        Ok(t)
    }

    /// The water tiles of `keys` (zoom `z`'s x, y) this cache lacks, made together: their
    /// polygons read in one ask (`WaterSource::depth_polys_many`: the basemap reads a block's
    /// tiles in runs, task #492; at z15–16 the z14 tiles they're drawn from, those not kept) and
    /// rasterised on the job's threads, each tile's as `tile` makes it one by one.
    fn prefetch(&mut self, src: &dyn WaterSource, z: u8, keys: &[(u32, u32)]) -> Result<()> {
        use rayon::prelude::*;
        let todo: Vec<(u32, u32)> =
            keys.iter().copied().filter(|&(x, y)| !self.tiles.contains_key(&(z, x, y))).collect();
        if todo.is_empty() {
            return Ok(());
        }
        let polys: Vec<Vec<Poly>> = if z <= WATER_Z {
            src.depth_polys_many(&todo.iter().map(|&(x, y)| (z, x, y)).collect::<Vec<_>>())?
        } else {
            let d = z - WATER_Z;
            let mut parents: Vec<(u32, u32)> = todo.iter().map(|&(x, y)| (y >> d, x >> d)).collect();
            parents.sort_unstable();
            parents.dedup();
            let missing: Vec<(u32, u32)> = parents.iter().copied().filter(|k| !self.polys.contains_key(k)).collect();
            let mut last = None;
            if !missing.is_empty() {
                let read = src.depth_polys_many(&missing.iter().map(|&(y, x)| (WATER_Z, x, y)).collect::<Vec<_>>())?;
                for (k, p) in missing.iter().zip(read) {
                    let bytes = polys_bytes(&p);
                    self.polys.insert(*k, (Arc::new(p), bytes));
                    self.polys_bytes += bytes;
                    last = Some(*k);
                }
            }
            let kept = &self.polys;
            let scaled: Vec<Vec<Poly>> =
                todo.par_iter().map(|&(x, y)| scaled(&kept[&(y >> d, x >> d)].0, d, x, y)).collect();
            // (Past the bound: the others go, the rows farthest up first, until it fits; the last
            // read stays whatever the bound.)
            if let Some(last) = last {
                while self.polys_bytes > self.bound {
                    let Some(k) = self.polys.keys().find(|k| **k != last).copied() else { break };
                    if let Some((_, b)) = self.polys.remove(&k) {
                        self.polys_bytes -= b;
                    }
                }
            }
            scaled
        };
        let tiles: Vec<Option<Arc<WaterTile>>> = polys.into_par_iter().map(made).collect();
        if self.tiles.len() + tiles.len() > 4096 {
            self.tiles.clear();
        }
        for ((x, y), t) in todo.into_iter().zip(tiles) {
            self.tiles.insert((z, x, y), t);
        }
        Ok(())
    }

    /// Block `b`'s water (`Water`): a pixel is water when the sea, lakes and rivers cover half of
    /// it or more, or it's narrow water; of a lake when lakes cover more of it than the sea, of a
    /// river when rivers cover more of it than either and the sea and lakes under half (`Water`);
    /// lakes numbered by their key, block-wide.
    pub fn block(&mut self, src: &dyn WaterSource, b: Block) -> Result<Water> {
        let (w, h) = (b.w(), b.h());
        let mut out = Water::land(w * h);
        // (Each pixel's share of water, for the narrow water.)
        let mut share = vec![0u8; w * h];
        let mut river = vec![false; w * h];
        // (The sea's and lakes' share alone: what made a pixel water before rivers counted.)
        let mut old = vec![0u8; w * h];
        let mut ids: HashMap<u64, u32> = HashMap::new();
        let n = 1i64 << b.z;
        let around: Vec<(u32, u32)> = (i64::from(b.ty0) - 1..=i64::from(b.ty0 + b.ny))
            .flat_map(|ty| (i64::from(b.tx0) - 1..=i64::from(b.tx0 + b.nx)).map(move |tx| (tx, ty)))
            .filter(|&(tx, ty)| (0..n).contains(&ty) && (0..n).contains(&tx))
            .map(|(tx, ty)| (tx as u32, ty as u32))
            .collect();
        self.prefetch(src, b.z, &around)?;
        for ty in i64::from(b.ty0) - 1..=i64::from(b.ty0 + b.ny) {
            for tx in i64::from(b.tx0) - 1..=i64::from(b.tx0 + b.nx) {
                if !(0..n).contains(&ty) || !(0..n).contains(&tx) {
                    continue;
                }
                let Some(t) = self.tile(src, b.z, tx as u32, ty as u32)? else { continue };
                let (bx, by) = ((tx - i64::from(b.tx0)) * 256 + PAD as i64, (ty - i64::from(b.ty0)) * 256 + PAD as i64);
                for py in 0..256i64 {
                    let y = by + py;
                    if !(0..h as i64).contains(&y) {
                        continue;
                    }
                    for px in 0..256i64 {
                        let x = bx + px;
                        if !(0..w as i64).contains(&x) {
                            continue;
                        }
                        let s = (py * 256 + px) as usize;
                        let (sea, lake, riv) = (t.sea[s], t.lake[s], t.river_at(s));
                        let o = y as usize * w + x as usize;
                        share[o] = sea + lake + riv;
                        old[o] = sea + lake;
                        if share[o] == 0 {
                            continue;
                        }
                        river[o] = riv > sea && riv > lake && sea + lake < 8;
                        if lake > sea && lake >= riv && t.lake_of[s] != u16::MAX {
                            let key = t.ids[t.lake_of[s] as usize];
                            let next = ids.len() as u32 + 1;
                            out.lake[o] = *ids.entry(key).or_insert(next);
                        }
                    }
                }
            }
        }
        let half = |o: usize| share[o] >= 8;
        let mut if_data = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let o = y * w + x;
                if share[o] == 0 {
                    continue;
                }
                let narrow = !half(o)
                    && !(y.saturating_sub(1)..(y + 2).min(h))
                        .any(|yy| (x.saturating_sub(1)..(x + 2).min(w)).any(|xx| half(yy * w + xx)));
                if !(half(o) || narrow) {
                    out.lake[o] = 0;
                    continue;
                }
                out.water[o] = true;
                // (Water only by the rivers' share, or narrow: water only with a value.)
                if_data[o] = river[o] || narrow || (half(o) && old[o] < 8);
            }
        }
        if river.iter().any(|&r| r) {
            out.river = river.iter().zip(&out.water).map(|(r, w)| *r && *w).collect();
        }
        if if_data.iter().any(|&r| r) {
            out.if_data = if_data;
        }
        Ok(out)
    }
}

/// The depth tiles a job has made so far (as stored: fall::encode), which its finer levels' capped
/// pixels are scaled up from.
struct Tiles<'a> {
    out: &'a Out,
    made: BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>,
    decoded: RefCell<HashMap<(u8, u32, u32), Stored>>,
}

impl<'a> Tiles<'a> {
    fn new(out: &'a Out) -> Self {
        Tiles { out, made: BTreeMap::new(), decoded: RefCell::new(HashMap::new()) }
    }

    fn get(&self, z: u8, x: u32, y: u32) -> Result<Stored> {
        if let Some(g) = self.decoded.borrow().get(&(z, x, y)) {
            return Ok(g.clone());
        }
        let g = match self.made.get(&(z, x, y)) {
            Some((b, _)) => Some(Arc::new(fall::decode(Kind::Terrarium, b)?)),
            None => None,
        };
        let mut d = self.decoded.borrow_mut();
        if d.len() > 2048 {
            d.clear();
        }
        d.insert((z, x, y), g.clone());
        Ok(g)
    }

    fn put(&mut self, z: u8, x: u32, y: u32, depth: &[f32], class: Vec<u8>) -> Result<()> {
        let dc = encoded(depth, class)?;
        self.decoded.borrow_mut().remove(&(z, x, y));
        self.made.insert((z, x, y), dc);
        Ok(())
    }

    /// Level `zv` scaled up onto block `b` as the map's fallback makes each of its tiles: from the
    /// nearest stored ancestor at `zv` or coarser (NODATA where there's none).
    fn up(&self, b: Block, zv: u8) -> Result<Vec<f32>> {
        let (w, h) = (b.w(), b.h());
        let mut out = vec![NODATA; w * h];
        let n = 1i64 << b.z;
        for ty in i64::from(b.ty0) - 1..=i64::from(b.ty0 + b.ny) {
            for tx in i64::from(b.tx0) - 1..=i64::from(b.tx0 + b.nx) {
                if !(0..n).contains(&ty) || !(0..n).contains(&tx) {
                    continue;
                }
                let (x, y) = (tx as u32, ty as u32);
                let mut made = None;
                for a in (0..=zv.min(b.z - 1)).rev() {
                    let k = b.z - a;
                    let Some(anc) = self.get(a, x >> k, y >> k)? else { continue };
                    let get = |z: u8, x: u32, y: u32| self.get(z, x, y);
                    // (The ancestor's neighbours, as fall::served takes them.)
                    let mut nbs: Vec<Stored> = Vec::with_capacity(8);
                    let na = 1i64 << a;
                    for (dx, dy) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                        let (nx, ny) = ((i64::from(x >> k) + dx).rem_euclid(na), i64::from(y >> k) + dy);
                        nbs.push(if (0..na).contains(&ny) { get(a, nx as u32, ny as u32)? } else { None });
                    }
                    let nb: [Option<&Grid>; 8] = std::array::from_fn(|i| nbs[i].as_deref());
                    made = Some(fall::upsample(k, x, y, &anc, nb));
                    break;
                }
                let Some(Grid::F(v)) = made else { continue };
                let (bx, by) = ((tx - i64::from(b.tx0)) * 256 + PAD as i64, (ty - i64::from(b.ty0)) * 256 + PAD as i64);
                for py in 0..256i64 {
                    let yy = by + py;
                    if !(0..h as i64).contains(&yy) {
                        continue;
                    }
                    for px in 0..256i64 {
                        let xx = bx + px;
                        if (0..w as i64).contains(&xx) {
                            out[yy as usize * w + xx as usize] = v[(py * 256 + px) as usize];
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

/// How often a level says how far it is.
const SAY_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// Composes zoom `z`'s tiles `x0..x0 + n` by `y0..y0 + n` into `t`, block by block, telling
/// `done` the share of the level's blocks done after each (a level's blocks can take an hour on the
/// NAS: the job's progress moves within it).
fn level(
    t: &mut Tiles,
    sources: &[Source],
    rd: &Reader,
    water: &dyn WaterSource,
    wc: &mut WaterCache,
    z: u8,
    area: Area,
    zmin: u8,
    log: Option<&mut crate::resume_log::Log>,
    done: &(dyn Fn(f64) + Sync),
) -> Result<usize> {
    use std::sync::atomic::{AtomicU32, Ordering};
    let all = area.nx.div_ceil(BLOCK) * area.ny.div_ceil(BLOCK);
    let blocks = AtomicU32::new(0);
    // (The reporter stops however the blocks end, a panic too, when `stop` drops: else the scope
    // waits on it for good. The drop wakes it at once: a level never waits out its 200 ms.)
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let counted = &blocks;
    // (Said every few seconds by a thread beside the work: a block's sources can take minutes on the
    // NAS, so a block's share counts its grids sampled so far, crate::depth::grids::sampled.)
    std::thread::scope(|s| {
        s.spawn(move || {
            let mut last = std::time::Instant::now();
            let tick = std::time::Duration::from_millis(200);
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(tick) {
                if last.elapsed() >= SAY_EVERY {
                    let k = f64::from(counted.load(Ordering::Relaxed));
                    done(((k + super::grids::sampled()) / f64::from(all)).min(1.0));
                    last = std::time::Instant::now();
                }
            }
        });
        let _stop = stop;
        level_blocks(t, sources, rd, water, wc, z, area, zmin, log, &blocks)
    })
}

/// A level's tiles a job composes: columns `x0..x0 + nx`, rows `y0..y0 + ny`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x0: u32,
    pub y0: u32,
    pub nx: u32,
    pub ny: u32,
}

impl Area {
    /// Tile (z, x, y)'s tiles at zoom `at` (as fine or finer).
    fn under(z: u8, x: u32, y: u32, at: u8) -> Area {
        let n = 1u32 << (at - z);
        Area { x0: x * n, y0: y * n, nx: n, ny: n }
    }
}

/// The coarse level `z` (`PIECE_Z` to `LO.1`) z6 tile `t` composes for its capped pixels
/// (`compose_hi`): its tiles and the ring of one around them, rows within the world's, columns
/// across the date line too (the map's fallback takes an ancestor's neighbours east–west so:
/// `Tiles::up`); the wrapped column an area of its own.
pub fn coarse_areas(t: (u32, u32), z: u8) -> Vec<Area> {
    let a = Area::under(6, t.0, t.1, z);
    let n = 1u32 << z;
    let (y0, y1) = (a.y0.saturating_sub(1), (a.y0 + a.ny + 1).min(n));
    let (x0, x1) = (a.x0.saturating_sub(1), (a.x0 + a.nx + 1).min(n));
    let mut v = vec![Area { x0, y0, nx: x1 - x0, ny: y1 - y0 }];
    if a.x0 == 0 {
        v.push(Area { x0: n - 1, y0, nx: 1, ny: y1 - y0 });
    }
    if a.x0 + a.nx == n {
        v.push(Area { x0: 0, y0, nx: 1, ny: y1 - y0 });
    }
    v
}

/// `level`'s blocks, in turn, counting those done in `blocks`; with `log`, those it holds skipped
/// (their tiles laid in as it opened) and each other one kept as it ends (crate::depth::resume).
fn level_blocks(
    t: &mut Tiles,
    sources: &[Source],
    rd: &Reader,
    water: &dyn WaterSource,
    wc: &mut WaterCache,
    z: u8,
    area: Area,
    zmin: u8,
    mut log: Option<&mut crate::resume_log::Log>,
    blocks: &std::sync::atomic::AtomicU32,
) -> Result<usize> {
    let Area { x0, y0, nx, ny } = area;
    let mut kept = 0;
    for by in (y0..y0 + ny).step_by(BLOCK as usize) {
        for bx in (x0..x0 + nx).step_by(BLOCK as usize) {
            super::grids::sampled_reset();
            // (Counted done however it ends: skipped, dry or composed.)
            struct Done<'a>(&'a std::sync::atomic::AtomicU32);
            impl Drop for Done<'_> {
                fn drop(&mut self) {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    super::grids::sampled_reset();
                }
            }
            let _done = Done(blocks);
            if log.as_ref().is_some_and(|l| l.done.contains(&(z, bx, by))) {
                continue;
            }
            let b = Block { z, tx0: bx, ty0: by, nx: BLOCK.min(x0 + nx - bx), ny: BLOCK.min(y0 + ny - by) };
            let put = block(t, sources, rd, water, wc, b, zmin)?;
            kept += put.len();
            if let Some(l) = log.as_deref_mut() {
                let tiles: Vec<_> = put.iter().filter_map(|k| t.made.get_key_value(k)).collect();
                super::resume::keep(l, (z, bx, by), &tiles)?;
            }
        }
    }
    Ok(kept)
}

/// Block `b` of `level_blocks`: its tiles made and put into `t`, which those are.
fn block(
    t: &mut Tiles,
    sources: &[Source],
    rd: &Reader,
    water: &dyn WaterSource,
    wc: &mut WaterCache,
    b: Block,
    zmin: u8,
) -> Result<Vec<(u8, u32, u32)>> {
    use crate::timings::{phase, Class};
    let z = b.z;
    let mut put = Vec::new();
    {
        {
            let (c0, r0) = b.origin();
            let bbox = super::grids::window_box(z, c0, r0, b.w(), b.h());
            let _p = phase("the block's sources found", Class::Compute);
            let any = super::compose::composes_at(sources, bbox, z);
            drop(_p);
            if !any {
                return Ok(put);
            }
            let wt = {
                let _p = phase("the basemap's water read", Class::NasRead);
                wc.block(water, b)?
            };
            let made = {
                let _p = phase("composing", Class::Compute);
                let tt: &Tiles = t;
                let mut up = |zv: u8| tt.up(b, zv);
                compose(b, sources, rd, &wt, &mut up, zmin)?
            };
            let Some(c) = made else { return Ok(put) };
            let _p = phase("tiles encoded", Class::Compute);
            for (x, y, d, k) in tiles(b, &c) {
                t.put(z, x, y, &d, k)?;
                put.push((z, x, y));
            }
        }
    }
    Ok(put)
}

/// Writes the layers' packs of `scope` under `root` from the tiles made and, with `shade`, their
/// shading (`shading`), replacing or dropping what the manifest named; how many tiles.
fn put_packs(
    out: &mut Out,
    made: &Made,
    shade: Option<&BTreeMap<(u8, u32, u32), Vec<u8>>>,
    scope: &str,
    root: (u8, u32, u32),
) -> Result<usize> {
    let _p = crate::timings::phase("packs written", crate::timings::Class::NasWrite);
    for (layer, enc, which) in [(LAYER, ENCODING, 0usize), (CLASS_LAYER, CLASS_ENCODING, 1)] {
        let mut it = made.iter().map(move |(&(z, x, y), p)| {
            let b = if which == 0 { &p.0 } else { &p.1 };
            (z, x, y, b.clone(), 0u32)
        });
        put_pack(out, layer, enc, scope, root, &mut it)?;
    }
    if let Some(shade) = shade {
        let mut it = shade.iter().map(|(&(z, x, y), b)| (z, x, y, b.clone(), 0u32));
        put_pack(out, super::SHADE_LAYER, crate::shade_codec::ENCODING, scope, root, &mut it)?;
    }
    out.save()?;
    Ok(made.len())
}

/// Writes one layer's pack of `scope` under `root`, replacing or dropping what the manifest named.
fn put_pack(
    out: &mut Out,
    layer: &str,
    enc: &str,
    scope: &str,
    root: (u8, u32, u32),
    it: &mut dyn Iterator<Item = (u8, u32, u32, Vec<u8>, u32)>,
) -> Result<()> {
    let logical = format!("layers/{layer}/{scope}/{}-{}-{}", root.0, root.1, root.2);
    match crate::layers::write_pack_local(out, layer, enc, false, scope, root, it)? {
        Some(p) => {
            out.put_file(&p.logical, "pack", &p.local)?;
            std::fs::remove_file(&p.local).ok();
        }
        None if out.get(&logical).is_some() => out.remove(&logical),
        None => {}
    }
    Ok(())
}

/// A depth tile and its class tile, decoded.
type Pair = (Vec<f32>, Vec<u8>);

/// Depth tile `d` and class tile `c` as stored, decoded.
fn decoded(d: &[u8], c: &[u8]) -> Result<Pair> {
    let Grid::F(d) = fall::decode(Kind::Terrarium, d)? else { unreachable!() };
    let Grid::C(c) = fall::decode(Kind::Class, c)? else { unreachable!() };
    Ok((d, c))
}

/// A tile from its four children `kids` (west to east, north to south) and, where they have no
/// data, `base` (its own level's, composed): each pixel the mean of the 2 × 2 beneath it that have
/// data, its class the first of them with one; a pixel without data under it takes `base`'s (with
/// none, its class the children's). None when no pixel has data.
fn from_kids(kids: &[Option<Pair>; 4], base: Option<&Pair>) -> Option<Pair> {
    let (mut d, mut c) = match base {
        Some((d, c)) => (d.clone(), c.clone()),
        None => (vec![NODATA; 65536], vec![0u8; 65536]),
    };
    for py in 0..256usize {
        for px in 0..256usize {
            let Some((kd, kc)) = &kids[(py / 128) * 2 + px / 128] else { continue };
            let (cx, cy) = ((px % 128) * 2, (py % 128) * 2);
            let (mut s, mut m) = (0f32, 0);
            let mut cl = 0u8;
            for (ox, oy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (cy + oy) * 256 + cx + ox;
                if kd[i] < NODATA_MIN {
                    s += kd[i];
                    m += 1;
                    if cl == 0 {
                        cl = kc[i];
                    }
                }
            }
            let o = py * 256 + px;
            if m > 0 {
                d[o] = s / m as f32;
                c[o] = cl;
            } else if base.is_none() {
                c[o] = (0..4).map(|k| kc[(cy + k / 2) * 256 + cx + k % 2]).find(|&v| v != 0).unwrap_or(0);
            }
        }
    }
    d.iter().any(|&v| v < NODATA_MIN).then_some((d, c))
}

/// The tiles a job makes, as stored (depth, class), by (z, x, y).
type Made = BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>;

/// Tiles `means` made: each one's key and what it now is (None: no data).
type Means = Vec<((u8, u32, u32), Option<(Vec<u8>, Vec<u8>)>)>;

/// Zoom `z`'s tiles from the finer (as the terrain's, §2): each tile with a child of zoom z + 1 in
/// `kids`, its pixels the mean of the four beneath them that have data (`from_kids`), elsewhere its
/// own in `own` (zoom z, composed) or, where its zoom wasn't composed, the nearest coarser level
/// `own` holds scaled up (`scaled_up`: what the map would show there), made on the job's threads.
fn means(kids: &Made, own: &Made, z: u8) -> Result<Means> {
    use rayon::prelude::*;
    let mut parents: Vec<(u32, u32)> =
        kids.range((z + 1, 0, 0)..(z + 2, 0, 0)).map(|(&(_, x, y), _)| (x / 2, y / 2)).collect();
    parents.sort_unstable();
    parents.dedup();
    parents
        .into_par_iter()
        .map(|(x, y)| {
            let mut ks: [Option<Pair>; 4] = Default::default();
            for (k, kid) in ks.iter_mut().enumerate() {
                if let Some((d, c)) = kids.get(&(z + 1, 2 * x + k as u32 % 2, 2 * y + k as u32 / 2)) {
                    *kid = Some(decoded(d, c)?);
                }
            }
            let base = match own.get(&(z, x, y)) {
                Some((d, c)) => Some(decoded(d, c)?),
                None => scaled_up(own, z, x, y)?,
            };
            let t = match from_kids(&ks, base.as_ref()) {
                Some((d, c)) => Some(encoded(&d, c)?),
                None => None,
            };
            Ok(((z, x, y), t))
        })
        .collect()
}

/// Tile (z, x, y) as the map's fallback makes it from the nearest coarser tile in `made` (with that
/// tile's neighbours' edges: crate::depth::fall::upsample), its classes those of a coarser grid
/// scaled up (`C_NATIVE` taken as `C_SCALED`); None with none.
fn scaled_up(made: &Made, z: u8, x: u32, y: u32) -> Result<Option<Pair>> {
    for a in (0..z).rev() {
        let k = z - a;
        let (ax, ay) = (x >> k, y >> k);
        let Some((d, c)) = made.get(&(a, ax, ay)) else { continue };
        let (anc, cls) = (fall::decode(Kind::Terrarium, d)?, fall::decode(Kind::Class, c)?);
        let na = 1i64 << a;
        let mut nbs: Vec<Option<Grid>> = Vec::with_capacity(8);
        for (dx, dy) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
            let (nx, ny) = ((i64::from(ax) + dx).rem_euclid(na), i64::from(ay) + dy);
            let t = if (0..na).contains(&ny) { made.get(&(a, nx as u32, ny as u32)) } else { None };
            nbs.push(match t {
                Some((d, _)) => Some(fall::decode(Kind::Terrarium, d)?),
                None => None,
            });
        }
        let nb: [Option<&Grid>; 8] = std::array::from_fn(|i| nbs[i].as_ref());
        let Grid::F(dv) = fall::upsample(k, x, y, &anc, nb) else { unreachable!() };
        let Grid::C(mut cv) = fall::upsample(k, x, y, &cls, [None; 8]) else { unreachable!() };
        for v in &mut cv {
            if *v & 7 == super::C_NATIVE {
                *v = (*v & !7) | super::C_SCALED;
            }
        }
        return Ok(Some((dv, cv)));
    }
    Ok(None)
}

/// `means`' tiles laid into `made`.
fn put_means(made: &mut Made, got: Means) {
    for (k, t) in got {
        match t {
            Some(t) => made.insert(k, t),
            None => made.remove(&k),
        };
    }
}

/// Zooms `top - 1` down to `bottom` of `made`, each the mean of the finer over its own (`means`).
fn means_down(made: &mut Made, top: u8, bottom: u8) -> Result<()> {
    let _p = crate::timings::phase("the coarser levels from the finer", crate::timings::Class::Compute);
    for z in (bottom..top).rev() {
        let got = means(made, made, z)?;
        put_means(made, got);
    }
    Ok(())
}

/// A tile's depth (quantised: `quantise`) and class, encoded as stored.
fn encoded(depth: &[f32], class: Vec<u8>) -> Result<(Vec<u8>, Vec<u8>)> {
    let q: Vec<f32> = depth.iter().map(|&v| super::quantise(v)).collect();
    Ok((fall::encode(&Grid::F(q))?, fall::encode(&Grid::C(class))?))
}

/// The assembly of z3 tile `q`: its lo packs (z3–8: the depth, its classes and the floor's
/// shading); how many depth tiles.
/// (`local`: this Mac's whole copies of the sources' packs, crate::localread; None, the NAS's read.)
pub fn build_lo(
    out: &mut Out,
    sources: &[Source],
    water: &dyn WaterSource,
    local: Local,
    q: (u32, u32),
) -> Result<usize> {
    let made = compose_lo(out, sources, water, local, q, LO.1, crate::resume_log::on())?;
    let shade = shading_lo(out, &made, q)?;
    let n = put_packs(out, &made, Some(&shade), "lo", (3, q.0, q.1))?;
    std::fs::remove_file(crate::resume_log::path(&out.scratch, (3, q.0, q.1))).ok();
    Ok(n)
}

/// The depth tiles in metres the floor's shading keeps decoded (`shading_over`), at most.
const SHADE_CACHED: usize = 2048;

/// The most memory (MB) a z3 tile's assembly (`build_lo`) takes: its composing's first guess (2 GB:
/// a block's arrays and distance fields, the sources' blocks read, its water; let go before its
/// shading) or its floor's shading's (`shade_lo_peak_mb`), the more.
pub const fn lo_peak_mb() -> u64 {
    const COMPOSING_MB: u64 = 2000;
    let s = shade_lo_peak_mb();
    if s > COMPOSING_MB {
        s
    } else {
        COMPOSING_MB
    }
}

/// The most memory (MB) the floor's shading of a z3 tile's assembly takes (`shading_lo`), from
/// its bounds: its z6 tiles' z9 quadrants (64 × 64); its depth lo tiles (z3–8) as stored, each
/// taken at its raw RGBA and a margin for the container's headers (lossless WebP of a 256-px tile
/// is smaller); its depth
/// tiles kept decoded (`SHADE_CACHED`, and one more); the shading's tiles made, each at most its raw
/// codes; the pyramid's working memory (crate::slope_pack::working_bytes, z3 to z8).
const fn shade_lo_peak_mb() -> u64 {
    const TS: u64 = 256;
    let px = std::mem::size_of::<roadcore::shade::Px>() as u64;
    let quadrant = 128 * 128 * px;
    let lo_tiles = 1 + 4 + 16 + 64 + 256 + 1024;
    let stored = TS * (TS * 4 + 1) + (TS * TS * 4 / 16_384 + 1) * 5 + 1024;
    let decoded = (SHADE_CACHED as u64 + 1) * TS * TS * 4;
    let made = lo_tiles * TS * TS * 3;
    let bytes = 64 * 64 * quadrant + lo_tiles * stored + decoded + made + crate::slope_pack::working_bytes(LO.0, LO.1);
    bytes >> 20
}

/// The floor's shading of z3 tile `q`'s assembly (its `depth-shade` lo pack, z3–8): the pyramid of
/// `shading` over its depth tiles `made`, each z8 pixel under a z6 tile's z9 shading the mean of
/// those four as the hi packs store them (the fine sources' detail), else its own depth's.
pub(super) fn shading_lo(out: &Out, made: &Made, q: (u32, u32)) -> Result<BTreeMap<(u8, u32, u32), Vec<u8>>> {
    let p = crate::timings::phase("the z9 shading read", crate::timings::Class::NasRead);
    let shade = crate::terrain_pack::ManifestTiles::new(out, super::SHADE_LAYER);
    let mut below = HashMap::new();
    for k in 0..64u32 {
        let t = (q.0 * 8 + k % 8, q.1 * 8 + k / 8);
        let fine: Vec<(u8, u32, u32)> =
            shade.tiles_in(&hi_logical(super::SHADE_LAYER, t.0, t.1))?.into_iter().filter(|e| e.0 == HI.0).collect();
        for (&(z, x, y), b) in fine.iter().zip(shade.get_many(&fine)?) {
            let b = b.with_context(|| format!("depth-shade {z}/{x}/{y} of its pack"))?;
            let v = roadcore::shade::decode(&b).with_context(|| format!("depth-shade {z}/{x}/{y}"))?;
            below.insert((z, x, y), roadcore::shade::quadrant(&v));
        }
    }
    p.count(0, below.len() as u64);
    drop(p);
    shading_over(made, LO.0, below)
}

/// The assembly of z3 tile `q`'s tiles, made, unwritten: its blocks composed z3 to `upto` (at most
/// z8; below it, the composing alone, a measure); then, with `upto` z8, z8 from the z9 tiles q's z6
/// tiles' hi packs store and z7 down to z3 each from the finer (`means`), each over its own level
/// composed where the finer has no data. With `keep`, its blocks kept as made and those an earlier
/// run kept taken (`resume_log`).
pub fn compose_lo(
    out: &Out,
    sources: &[Source],
    water: &dyn WaterSource,
    local: Local,
    q: (u32, u32),
    upto: u8,
    keep: bool,
) -> Result<Made> {
    let rd = Reader::default().with_local(local);
    let mut wc = WaterCache::default();
    let upto = upto.min(LO.1);
    let mut t = Tiles::new(out);
    let mut log = resume_log(keep, &mut t, sources, water, (3, q.0, q.1), upto);
    let zooms = u64::from(upto.saturating_sub(LO.0) + 1);
    for z in LO.0..=upto {
        wc.next_level();
        let at = f64::from(z - LO.0);
        let done = |f: f64| crate::agent::jobs::report_f(at + f, zooms, "zooms");
        let area = Area::under(3, q.0, q.1, z);
        let k = level(&mut t, sources, &rd, water, &mut wc, z, area, LO.0, log.as_mut(), &done)?;
        crate::agent::jobs::report(u64::from(z - LO.0 + 1), zooms, "zooms");
        eprintln!("depth-lo 3/{}/{}: z{z}, {k} tiles", q.0, q.1);
    }
    let mut made = std::mem::take(&mut t.made);
    drop(t);
    close(rd, wc);
    if upto < LO.1 {
        return Ok(made);
    }
    from_hi(out, q, &mut made)?;
    means_down(&mut made, LO.1, LO.0)?;
    Ok(made)
}

/// Z3 tile `q`'s z8 tiles in `made` from the z9 tiles its z6 tiles' hi packs store (`means`, a z6
/// tile at a time: its z9 tiles held only while its z8 is made).
fn from_hi(out: &Out, q: (u32, u32), made: &mut Made) -> Result<()> {
    let _p = crate::timings::phase("z8 from the z9 tiles", crate::timings::Class::NasRead);
    let depth = crate::terrain_pack::ManifestTiles::new(out, LAYER);
    let class = crate::terrain_pack::ManifestTiles::new(out, CLASS_LAYER);
    for k in 0..64u32 {
        let t = (q.0 * 8 + k % 8, q.1 * 8 + k / 8);
        let fine: Vec<(u8, u32, u32)> =
            depth.tiles_in(&hi_logical(LAYER, t.0, t.1))?.into_iter().filter(|e| e.0 == HI.0).collect();
        if fine.is_empty() {
            continue;
        }
        let (ds, cs) = (depth.get_many(&fine)?, class.get_many(&fine)?);
        let mut kids = Made::new();
        for ((&(z, x, y), d), c) in fine.iter().zip(ds).zip(cs) {
            let d = d.with_context(|| format!("depth {z}/{x}/{y} of its pack"))?;
            let c = c.with_context(|| format!("depth-class {z}/{x}/{y}: the depth's tile has none"))?;
            kids.insert((z, x, y), (d, c));
        }
        let got = means(&kids, made, LO.1)?;
        put_means(made, got);
    }
    Ok(())
}

/// With `keep`, target `t`'s log of blocks kept (crate::depth::resume), its tiles laid into `tl`,
/// for a run to zoom `upto` whose identity is everything its composing reads: the depth's
/// versions, the sources' pins and the water's (an assembly's z8 from the z9 tiles is made after,
/// not logged).
fn resume_log(
    keep: bool,
    tl: &mut Tiles,
    sources: &[Source],
    water: &dyn WaterSource,
    t: (u8, u32, u32),
    upto: u8,
) -> Option<crate::resume_log::Log> {
    if !keep {
        return None;
    }
    let pins: Vec<&str> = sources.iter().map(|s| s.pin.as_str()).collect();
    let id = crate::resume_log::id(&[
        &format!("depth {DEPTH_V} {DEPTH_LO_V} {LAYER} {CLASS_LAYER} {}", fall::version()),
        &format!("{}/{}/{} to z{upto}", t.0, t.1, t.2),
        &pins.join("\n"),
        &water.pin(),
    ]);
    super::resume::open(&tl.out.scratch, t, &id, &mut tl.made)
}

/// The grids a reader holds open closed and the water's polygons let go, as a phase: on the NAS
/// a close is a round trip, and a reader keeps up to 2,000 open (grids::open_grids_limit).
fn close(rd: Reader, wc: WaterCache) {
    let _p = crate::timings::phase("the grids closed and the caches let go", crate::timings::Class::NasRead);
    drop(rd);
    drop(wc);
}

/// The piece of z6 tile `t`: its hi packs (z9–16); how many tiles.
/// (`local`: as `build_lo`'s.)
pub fn build_hi(
    out: &mut Out,
    sources: &[Source],
    water: &dyn WaterSource,
    local: Local,
    t: (u32, u32),
) -> Result<usize> {
    build_hi_keeping(out, sources, water, local, t, crate::resume_log::on())
}

/// `build_hi`, its blocks kept and taken (crate::depth::resume) with `keep`.
pub(super) fn build_hi_keeping(
    out: &mut Out,
    sources: &[Source],
    water: &dyn WaterSource,
    local: Local,
    t: (u32, u32),
    keep: bool,
) -> Result<usize> {
    let made = compose_hi(out, sources, water, local, t, HI.1, keep)?;
    // (The shading from the tiles made alone: a resumed run's the same bytes.)
    let shade = shading(&made, HI.0)?;
    let n = put_packs(out, &made, Some(&shade), "hi", (6, t.0, t.1))?;
    std::fs::remove_file(crate::resume_log::path(&out.scratch, (6, t.0, t.1))).ok();
    Ok(n)
}

/// The floor's shading (the `depth-shade` layer: crate::slope_pack's pyramid, as the terrain's)
/// of a job's depth tiles `made`, from zoom `top` down: at each place's finest stored level each
/// pixel's steepness and direction by Horn's method on the depth, a pixel without data taking its
/// nearest's in the tile (`filled`: no cliff at the shore, which the map draws only in the water)
/// and a neighbour tile the job didn't make read as the tile's own edge; each coarser level the mean
/// of the four pixels beneath it where its children cover it, else its own depth's.
pub fn shading(made: &Made, top: u8) -> Result<BTreeMap<(u8, u32, u32), Vec<u8>>> {
    shading_over(made, top, HashMap::new())
}

/// `shading`, over the quadrants of finer tiles another job made (`below`, by tile: as their packs
/// store them), which their parents take where they cover them (not made again).
fn shading_over(
    made: &Made,
    top: u8,
    below: HashMap<(u8, u32, u32), Vec<roadcore::shade::Px>>,
) -> Result<BTreeMap<(u8, u32, u32), Vec<u8>>> {
    use std::collections::HashSet;
    use std::sync::Mutex;
    let _p = crate::timings::phase("the floor's shading", crate::timings::Class::Compute);
    let mut tiles: HashSet<(u8, u32, u32)> = HashSet::new();
    for &(z, x, y) in made.keys().chain(below.keys()).filter(|k| k.0 >= top) {
        for dz in 0..=(z - top) {
            tiles.insert((z - dz, x >> dz, y >> dz));
        }
    }
    let cache: Mutex<HashMap<(u8, u32, u32), Option<Arc<Vec<f32>>>>> = Mutex::new(HashMap::new());
    let at = |z: u8, x: u32, y: u32| -> Option<Arc<Vec<f32>>> {
        if let Some(v) = cache.lock().unwrap().get(&(z, x, y)) {
            return v.clone();
        }
        let v = made.get(&(z, x, y)).and_then(|(d, _)| match fall::decode(Kind::Terrarium, d) {
            Ok(Grid::F(v)) => filled(v).map(Arc::new),
            _ => None,
        });
        let mut c = cache.lock().unwrap();
        if c.len() >= SHADE_CACHED {
            c.clear();
        }
        c.insert((z, x, y), v.clone());
        v
    };
    let own = |z: u8, x: u32, y: u32| crate::slope_pack::horn(&|x, y| at(z, x, y), z, x, y);
    let out = Mutex::new(Vec::new());
    let done: HashMap<(u8, u32, u32), Option<Vec<roadcore::shade::Px>>> =
        below.into_iter().map(|(k, q)| (k, Some(q))).collect();
    let mut roots: Vec<(u8, u32, u32)> = tiles.iter().copied().filter(|t| t.0 == top).collect();
    roots.sort_unstable();
    for (z, x, y) in roots {
        crate::slope_pack::Pyramid { own: &own, tiles: &tiles, out: &out, tick: &|| {}, done: &done }.tile(z, x, y);
    }
    Ok(out.into_inner().unwrap().into_iter().map(|(z, x, y, b)| ((z, x, y), b)).collect())
}

/// A depth tile's values with each pixel without data (`NODATA_MIN` and over) given its nearest's
/// with data (two passes of a chamfer distance, 1 and √2); None with none.
fn filled(mut v: Vec<f32>) -> Option<Vec<f32>> {
    const N: usize = 256;
    let mut src: Vec<i32> = (0..N * N).map(|i| if v[i] < NODATA_MIN { i as i32 } else { -1 }).collect();
    if src.iter().all(|&s| s < 0) {
        return None;
    }
    let mut dist: Vec<f32> = src.iter().map(|&s| if s >= 0 { 0.0 } else { f32::MAX }).collect();
    let mut relax = |i: usize, j: usize, cost: f32, src: &mut Vec<i32>| {
        if src[j] >= 0 && dist[j] + cost < dist[i] {
            dist[i] = dist[j] + cost;
            src[i] = src[j];
        }
    };
    for y in 0..N {
        for x in 0..N {
            let i = y * N + x;
            if x > 0 {
                relax(i, i - 1, 1.0, &mut src);
            }
            if y > 0 {
                relax(i, i - N, 1.0, &mut src);
                if x > 0 {
                    relax(i, i - N - 1, std::f32::consts::SQRT_2, &mut src);
                }
                if x < N - 1 {
                    relax(i, i - N + 1, std::f32::consts::SQRT_2, &mut src);
                }
            }
        }
    }
    for y in (0..N).rev() {
        for x in (0..N).rev() {
            let i = y * N + x;
            if x < N - 1 {
                relax(i, i + 1, 1.0, &mut src);
            }
            if y < N - 1 {
                relax(i, i + N, 1.0, &mut src);
                if x < N - 1 {
                    relax(i, i + N + 1, std::f32::consts::SQRT_2, &mut src);
                }
                if x > 0 {
                    relax(i, i + N - 1, std::f32::consts::SQRT_2, &mut src);
                }
            }
        }
    }
    for i in 0..N * N {
        if v[i] >= NODATA_MIN {
            v[i] = v[src[i] as usize];
        }
    }
    Some(v)
}

/// The piece of z6 tile `t`'s tiles, made, unwritten: z9 to `upto` (at most z16) composed (`upto`
/// below z16: a measure of part of it). Its capped pixels are scaled up from its own coarser
/// tiles: z6–8 composed first over t and the ring of tiles around it (`coarse_areas`, as an
/// assembly composes them), which serve only that and aren't stored. With `keep`, its blocks (the
/// coarse levels' too) kept as made and those an earlier run kept taken (`resume_log`).
pub fn compose_hi(
    out: &Out,
    sources: &[Source],
    water: &dyn WaterSource,
    local: Local,
    t: (u32, u32),
    upto: u8,
    keep: bool,
) -> Result<Made> {
    let rd = Reader::default().with_local(local);
    let upto = upto.min(HI.1);
    let mut tl = Tiles::new(out);
    let mut log = resume_log(keep, &mut tl, sources, water, (6, t.0, t.1), upto);
    let mut wc = WaterCache::default();
    for z in PIECE_Z..=LO.1 {
        wc.next_level();
        for area in coarse_areas(t, z) {
            level(&mut tl, sources, &rd, water, &mut wc, z, area, LO.0, log.as_mut(), &|_| {})?;
        }
    }
    let zooms = u64::from(upto.saturating_sub(HI.0) + 1);
    for z in HI.0..=upto {
        wc.next_level();
        let at = f64::from(z - HI.0);
        let done = |f: f64| crate::agent::jobs::report_f(at + f, zooms, "zooms");
        let area = Area::under(6, t.0, t.1, z);
        let k = level(&mut tl, sources, &rd, water, &mut wc, z, area, LO.0, log.as_mut(), &done)?;
        crate::agent::jobs::report(u64::from(z - HI.0 + 1), zooms, "zooms");
        eprintln!("depth 6/{}/{}: z{z}, {k} tiles", t.0, t.1);
    }
    // (Its coarse levels kept through the means: a level not composed takes them scaled up where
    // the finer has no data.)
    let mut made: Made = std::mem::take(&mut tl.made);
    drop(tl);
    close(rd, wc);
    means_down(&mut made, upto, HI.0)?;
    Ok(made.into_iter().filter(|(k, _)| k.0 >= HI.0).collect())
}

/// The sea and lakes `polys` of a test: squares in a tile's pixels.
#[cfg(test)]
pub fn square(kind: crate::terrain_water::Kind, id: u64, x0: f64, y0: f64, x1: f64, y1: f64) -> Poly {
    Poly { kind, id, rings: vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]] }
}
