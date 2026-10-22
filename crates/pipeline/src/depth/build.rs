//! The depth steps (docs/steps.md §3): `depth-lo`, a z3 tile's zoomed-out levels, then
//! `depth`, a z6 tile's fine levels, each composed block by block (crate::depth::compose) and
//! written as packs of the two layers (`depth`, `depth-class`), like the terrain's.
//!
//! - **depth-lo** (a z3 tile Q): z6–8 composed from the sources on the basemap's water (the zooms
//!   the basemap draws OSM's water at); z3–5 each the mean of its children's pixels (their class
//!   the first of the four that has one), as the terrain's assembly makes its coarse levels.
//! - **depth** (a z6 tile T): z9–16, a block only where a source has a grid of that zoom or finer
//!   (`Source::max_zoom_in`), its water the basemap's (z15–16: its z14 tile's polygons, finer). A
//!   capped pixel takes the stored level of its zoom scaled up as the map's fallback does: from this
//!   job's own tiles, and at z8 and coarser from the lo packs of T's z3 tile and its neighbours.
//!   Its neighbours' fine tiles aren't read (they're another job's), so along T's edge a capped
//!   pixel within the block's margin is scaled up from the lo levels instead.
//!
//! Deterministic: the same sources, water and lo packs make the same packs, byte for byte.

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

/// The depth piece's version: bumped with any change to the bytes `build_hi` writes (3: a grid
/// lined up with no overview sampled from one askew, `grids::level_for`, task #433).
pub const DEPTH_V: u32 = 3;
/// The depth assembly's (`build_lo`).
pub const DEPTH_LO_V: u32 = 3;
/// How far from the coverage depth is built (km): 200 nautical miles, a region's EEZ's reach.
pub const REACH_KM: f64 = 370.4;
/// The zooms a piece makes.
pub const HI: (u8, u8) = (9, 16);
/// The zooms an assembly composes (coarser ones from children).
pub const LO_COMPOSED: (u8, u8) = (6, 8);
/// The deepest zoom the basemap has water at.
const WATER_Z: u8 = 14;

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

/// The depth tiles a job has: its own, made so far (as stored: PNG), and the lo packs it reads.
struct Tiles<'a> {
    out: &'a Out,
    made: BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>,
    /// The lo packs read, by z3 tile (None: none there).
    lo: RefCell<HashMap<(u32, u32), Option<(store::pack::PackIndex, store::range::PlainFile)>>>,
    /// Whether the lo packs are read (a piece's job), else only its own (an assembly's).
    reads_lo: bool,
    decoded: RefCell<HashMap<(u8, u32, u32), Stored>>,
}

impl<'a> Tiles<'a> {
    fn new(out: &'a Out, reads_lo: bool) -> Self {
        Tiles {
            out,
            made: BTreeMap::new(),
            lo: RefCell::new(HashMap::new()),
            reads_lo,
            decoded: RefCell::new(HashMap::new()),
        }
    }

    fn png(&self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        if let Some((d, _)) = self.made.get(&(z, x, y)) {
            return Ok(Some(d.clone()));
        }
        if !(self.reads_lo && (3..=8).contains(&z)) {
            return Ok(None);
        }
        let q = (x >> (z - 3), y >> (z - 3));
        let mut lo = self.lo.borrow_mut();
        if !lo.contains_key(&q) {
            let p = match self.out.get(&lo_logical(LAYER, q.0, q.1)) {
                Some(c) => {
                    let f = store::range::PlainFile::open(&self.out.path(c))
                        .with_context(|| format!("the depth lo pack {c}"))?;
                    Some((store::pack::PackIndex::read_from(&f)?, f))
                }
                None => None,
            };
            lo.insert(q, p);
        }
        Ok(match &lo[&q] {
            Some((idx, f)) => idx.get(f, z, x, y)?.map(|(_, b)| b),
            None => None,
        })
    }

    fn get(&self, z: u8, x: u32, y: u32) -> Result<Stored> {
        if let Some(g) = self.decoded.borrow().get(&(z, x, y)) {
            return Ok(g.clone());
        }
        let g = match self.png(z, x, y)? {
            Some(b) => Some(Arc::new(fall::decode(Kind::Terrarium, &b)?)),
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
        let q: Vec<f32> = depth.iter().map(|&v| super::quantise(v)).collect();
        let d = fall::encode(&Grid::F(q))?;
        let c = fall::encode(&Grid::C(class))?;
        self.decoded.borrow_mut().remove(&(z, x, y));
        self.made.insert((z, x, y), (d, c));
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
    x0: u32,
    y0: u32,
    n: u32,
    zmin: u8,
    done: &(dyn Fn(f64) + Sync),
) -> Result<usize> {
    use std::sync::atomic::{AtomicU32, Ordering};
    let side = n.div_ceil(BLOCK);
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
                    done(((k + super::grids::sampled()) / f64::from(side * side)).min(1.0));
                    last = std::time::Instant::now();
                }
            }
        });
        let _stop = stop;
        level_blocks(t, sources, rd, water, wc, z, x0, y0, n, zmin, &blocks)
    })
}

/// `level`'s blocks, in turn, counting those done in `blocks`.
fn level_blocks(
    t: &mut Tiles,
    sources: &[Source],
    rd: &Reader,
    water: &dyn WaterSource,
    wc: &mut WaterCache,
    z: u8,
    x0: u32,
    y0: u32,
    n: u32,
    zmin: u8,
    blocks: &std::sync::atomic::AtomicU32,
) -> Result<usize> {
    use crate::timings::{phase, Class};
    let mut kept = 0;
    for by in (y0..y0 + n).step_by(BLOCK as usize) {
        for bx in (x0..x0 + n).step_by(BLOCK as usize) {
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
            let b = Block { z, tx0: bx, ty0: by, nx: BLOCK.min(x0 + n - bx), ny: BLOCK.min(y0 + n - by) };
            let (c0, r0) = b.origin();
            let bbox = super::grids::window_box(z, c0, r0, b.w(), b.h());
            let need = if z > super::compose::ALL_UPTO { z } else { zmin };
            let _p = phase("the block's sources found", Class::Compute);
            let any = sources.iter().any(|s| s.max_zoom_in(bbox).is_some_and(|m| m >= need));
            drop(_p);
            if !any {
                continue;
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
            let Some(c) = made else { continue };
            let _p = phase("tiles encoded", Class::Compute);
            for (x, y, d, k) in tiles(b, &c) {
                t.put(z, x, y, &d, k)?;
                kept += 1;
            }
        }
    }
    Ok(kept)
}

/// Writes the layers' packs of `scope` under `root` from the tiles made, replacing or dropping
/// what the manifest named; how many tiles.
fn put_packs(
    out: &mut Out,
    made: &BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>,
    scope: &str,
    root: (u8, u32, u32),
) -> Result<usize> {
    let _p = crate::timings::phase("packs written", crate::timings::Class::NasWrite);
    for (layer, enc, which) in [(LAYER, ENCODING, 0usize), (CLASS_LAYER, CLASS_ENCODING, 1)] {
        let mut it = made.iter().map(|(&(z, x, y), p)| {
            let b = if which == 0 { &p.0 } else { &p.1 };
            (z, x, y, b.clone(), 0u32)
        });
        let logical = format!("layers/{layer}/{scope}/{}-{}-{}", root.0, root.1, root.2);
        match crate::layers::write_pack_local(out, layer, enc, false, scope, root, &mut it)? {
            Some(p) => {
                out.put_file(&p.logical, "pack", &p.local)?;
                std::fs::remove_file(&p.local).ok();
            }
            None if out.get(&logical).is_some() => out.remove(&logical),
            None => {}
        }
    }
    out.save()?;
    Ok(made.len())
}

/// The mean of a tile's four children (each pixel the mean of the 2 × 2 it covers that have data;
/// its class the first of them with one): None when no child has data.
fn from_children(t: &Tiles, z: u8, x: u32, y: u32) -> Result<Option<(Vec<f32>, Vec<u8>)>> {
    let mut kids: Vec<Option<(Vec<f32>, Vec<u8>)>> = Vec::with_capacity(4);
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        kids.push(match t.made.get(&(z + 1, 2 * x + dx, 2 * y + dy)) {
            Some((d, c)) => {
                let Grid::F(d) = fall::decode(Kind::Terrarium, d)? else { unreachable!() };
                let Grid::C(c) = fall::decode(Kind::Class, c)? else { unreachable!() };
                Some((d, c))
            }
            None => None,
        });
    }
    if kids.iter().all(Option::is_none) {
        return Ok(None);
    }
    let mut d = vec![NODATA; 65536];
    let mut c = vec![0u8; 65536];
    let mut any = false;
    for py in 0..256usize {
        for px in 0..256usize {
            let k = &kids[(py / 128) * 2 + px / 128];
            let Some((kd, kc)) = k else { continue };
            let (cx, cy) = ((px % 128) * 2, (py % 128) * 2);
            let (mut s, mut m) = (0f32, 0);
            let mut cl = 0u8;
            for (ox, oy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (cy + oy) * 256 + cx + ox;
                if kd[i] < NODATA_MIN {
                    s += kd[i];
                    m += 1;
                }
                if cl == 0 {
                    cl = kc[i];
                }
            }
            if m > 0 {
                d[py * 256 + px] = s / m as f32;
                any = true;
            }
            c[py * 256 + px] = cl;
        }
    }
    Ok(any.then_some((d, c)))
}

/// The assembly of z3 tile `q`: its lo packs (z3–8); how many tiles.
/// (`local`: this Mac's whole copies of the sources' packs, crate::localread; None, the NAS's read.)
pub fn build_lo(
    out: &mut Out,
    sources: &[Source],
    water: &dyn WaterSource,
    local: Local,
    q: (u32, u32),
) -> Result<usize> {
    let made = compose_lo(out, sources, water, local, q, LO_COMPOSED.1)?;
    put_packs(out, &made, "lo", (3, q.0, q.1))
}

/// The assembly of z3 tile `q`'s tiles, made, unwritten: z6 to `upto` (at most z8) composed, the
/// coarser ones from their children (`upto` below z8: a measure of the composing alone).
pub fn compose_lo(
    out: &Out,
    sources: &[Source],
    water: &dyn WaterSource,
    local: Local,
    q: (u32, u32),
    upto: u8,
) -> Result<BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>> {
    let rd = Reader::default().with_local(local);
    let mut wc = WaterCache::default();
    let upto = upto.min(LO_COMPOSED.1);
    Ok({
        let mut t = Tiles::new(out, false);
        let zooms = u64::from(upto.saturating_sub(LO_COMPOSED.0) + 1);
        for z in LO_COMPOSED.0..=upto {
            let n = 1u32 << (z - 3);
            let at = f64::from(z - LO_COMPOSED.0);
            let done = |f: f64| crate::agent::jobs::report_f(at + f, zooms, "zooms");
            let k = level(&mut t, sources, &rd, water, &mut wc, z, q.0 * n, q.1 * n, n, 3, &done)?;
            crate::agent::jobs::report(u64::from(z - LO_COMPOSED.0 + 1), zooms, "zooms");
            eprintln!("depth-lo 3/{}/{}: z{z}, {k} tiles", q.0, q.1);
        }
        let _p = crate::timings::phase("z3–5 from their children", crate::timings::Class::Compute);
        for z in (3..LO_COMPOSED.0).rev() {
            let n = 1u32 << (z - 3);
            for y in q.1 * n..(q.1 + 1) * n {
                for x in q.0 * n..(q.0 + 1) * n {
                    if let Some((d, c)) = from_children(&t, z, x, y)? {
                        t.put(z, x, y, &d, c)?;
                    }
                }
            }
        }
        drop(_p);
        let made = std::mem::take(&mut t.made);
        drop(t);
        close(rd, wc);
        made
    })
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
    let made = compose_hi(out, sources, water, local, t, HI.1)?;
    put_packs(out, &made, "hi", (6, t.0, t.1))
}

/// The piece of z6 tile `t`'s tiles, made, unwritten: z9 to `upto` (at most z16) composed (`upto`
/// below z16: a measure of part of it).
pub fn compose_hi(
    out: &Out,
    sources: &[Source],
    water: &dyn WaterSource,
    local: Local,
    t: (u32, u32),
    upto: u8,
) -> Result<BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>> {
    let rd = Reader::default().with_local(local);
    let upto = upto.min(HI.1);
    let mut tl = Tiles::new(out, true);
    let mut wc = WaterCache::default();
    let zooms = u64::from(upto.saturating_sub(HI.0) + 1);
    for z in HI.0..=upto {
        wc.next_level();
        let n = 1u32 << (z - 6);
        let at = f64::from(z - HI.0);
        let done = |f: f64| crate::agent::jobs::report_f(at + f, zooms, "zooms");
        let k = level(&mut tl, sources, &rd, water, &mut wc, z, t.0 * n, t.1 * n, n, 3, &done)?;
        crate::agent::jobs::report(u64::from(z - HI.0 + 1), zooms, "zooms");
        eprintln!("depth 6/{}/{}: z{z}, {k} tiles", t.0, t.1);
    }
    let made = std::mem::take(&mut tl.made);
    drop(tl);
    close(rd, wc);
    Ok(made)
}

/// The sea and lakes `polys` of a test: squares in a tile's pixels.
#[cfg(test)]
pub fn square(kind: crate::terrain_water::Kind, id: u64, x0: f64, y0: f64, x1: f64, y1: f64) -> Poly {
    Poly { kind, id, rings: vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]] }
}
