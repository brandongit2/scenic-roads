//! The depth steps (docs/plan.md §6, Depth): `depth-lo`, a z3 tile's zoomed-out levels, then
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
use super::{CLASS_ENCODING, CLASS_LAYER, ENCODING, LAYER, NODATA, NODATA_MIN};
use crate::out::Out;
use crate::terrain_water::{water_tile, Poly, WaterSource, WaterTile};
use anyhow::{Context, Result};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// The depth piece's version: bumped with any change to the bytes `build_hi` writes.
pub const DEPTH_V: u32 = 1;
/// The depth assembly's (`build_lo`).
pub const DEPTH_LO_V: u32 = 1;
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

/// A tile's water polygons: the basemap's at z6–14, a z14 tile's scaled at z15–16.
pub fn polys_at(src: &dyn WaterSource, z: u8, x: u32, y: u32) -> Result<Vec<Poly>> {
    if z <= WATER_Z {
        return src.polys(z, x, y);
    }
    let d = z - WATER_Z;
    let n = f64::from(1u32 << d);
    let (ox, oy) = (f64::from(x & ((1 << d) - 1)), f64::from(y & ((1 << d) - 1)));
    let mut ps = src.polys(WATER_Z, x >> d, y >> d)?;
    for p in &mut ps {
        for r in &mut p.rings {
            for pt in r.iter_mut() {
                *pt = [(pt[0] - ox * 256.0 / n) * n, (pt[1] - oy * 256.0 / n) * n];
            }
        }
    }
    Ok(ps)
}

/// The water of the tiles around blocks, kept while a level is made (neighbouring blocks share
/// their rings).
#[derive(Default)]
struct WaterCache {
    tiles: HashMap<(u8, u32, u32), Option<Arc<WaterTile>>>,
}

impl WaterCache {
    fn tile(&mut self, src: &dyn WaterSource, z: u8, x: u32, y: u32) -> Result<Option<Arc<WaterTile>>> {
        if let Some(t) = self.tiles.get(&(z, x, y)) {
            return Ok(t.clone());
        }
        let ps = polys_at(src, z, x, y)?;
        let t = if ps.is_empty() { None } else { Some(Arc::new(water_tile(&ps))).filter(|t| !t.is_empty()) };
        if self.tiles.len() > 4096 {
            self.tiles.clear();
        }
        self.tiles.insert((z, x, y), t.clone());
        Ok(t)
    }

    /// Block `b`'s water: a pixel is water when the sea and lakes cover half of it or more, of a
    /// lake when lakes cover more of it than the sea; lakes numbered by their key, block-wide.
    fn block(&mut self, src: &dyn WaterSource, b: Block) -> Result<Water> {
        let (w, h) = (b.w(), b.h());
        let mut out = Water { water: vec![false; w * h], lake: vec![0; w * h] };
        let mut ids: HashMap<u64, u32> = HashMap::new();
        let n = 1i64 << b.z;
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
                        let (sea, lake) = (t.sea[s], t.lake[s]);
                        if u16::from(sea) + u16::from(lake) < 8 {
                            continue;
                        }
                        let o = y as usize * w + x as usize;
                        out.water[o] = true;
                        if lake > sea && t.lake_of[s] != u16::MAX {
                            let key = t.ids[t.lake_of[s] as usize];
                            let next = ids.len() as u32 + 1;
                            out.lake[o] = *ids.entry(key).or_insert(next);
                        }
                    }
                }
            }
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
        Tiles { out, made: BTreeMap::new(), lo: RefCell::new(HashMap::new()), reads_lo, decoded: RefCell::new(HashMap::new()) }
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
                    let f = store::range::PlainFile::open(&self.out.path(c)).with_context(|| format!("the depth lo pack {c}"))?;
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

/// Composes zoom `z`'s tiles `x0..x0 + n` by `y0..y0 + n` into `t`, block by block.
fn level(t: &mut Tiles, sources: &[Source], rd: &Reader, water: &dyn WaterSource, wc: &mut WaterCache, z: u8, x0: u32, y0: u32, n: u32, zmin: u8) -> Result<usize> {
    let mut kept = 0;
    for by in (y0..y0 + n).step_by(BLOCK as usize) {
        for bx in (x0..x0 + n).step_by(BLOCK as usize) {
            let b = Block { z, tx0: bx, ty0: by, nx: BLOCK.min(x0 + n - bx), ny: BLOCK.min(y0 + n - by) };
            let (c0, r0) = b.origin();
            let bbox = super::grids::window_box(z, c0, r0, b.w(), b.h());
            let need = if z > super::compose::ALL_UPTO { z } else { zmin };
            if !sources.iter().any(|s| s.max_zoom_in(bbox).is_some_and(|m| m >= need)) {
                continue;
            }
            let wt = wc.block(water, b)?;
            let made = {
                let tt: &Tiles = t;
                let mut up = |zv: u8| tt.up(b, zv);
                compose(b, sources, rd, &wt, &mut up, zmin)?
            };
            let Some(c) = made else { continue };
            for (x, y, d, k) in tiles(b, &c, &wt) {
                t.put(z, x, y, &d, k)?;
                kept += 1;
            }
        }
    }
    Ok(kept)
}

/// Writes the layers' packs of `scope` under `root` from the tiles made, replacing or dropping
/// what the manifest named; how many tiles.
fn put_packs(out: &mut Out, made: &BTreeMap<(u8, u32, u32), (Vec<u8>, Vec<u8>)>, scope: &str, root: (u8, u32, u32)) -> Result<usize> {
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
pub fn build_lo(out: &mut Out, sources: &[Source], water: &dyn WaterSource, q: (u32, u32)) -> Result<usize> {
    let rd = Reader::default();
    let mut wc = WaterCache::default();
    let made = {
        let mut t = Tiles::new(out, false);
        for z in LO_COMPOSED.0..=LO_COMPOSED.1 {
            let n = 1u32 << (z - 3);
            let k = level(&mut t, sources, &rd, water, &mut wc, z, q.0 * n, q.1 * n, n, 3)?;
            crate::agent::jobs::report(u64::from(z - LO_COMPOSED.0 + 1), u64::from(LO_COMPOSED.1 - LO_COMPOSED.0 + 1), "zooms");
            eprintln!("depth-lo 3/{}/{}: z{z}, {k} tiles", q.0, q.1);
        }
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
        std::mem::take(&mut t.made)
    };
    put_packs(out, &made, "lo", (3, q.0, q.1))
}

/// The piece of z6 tile `t`: its hi packs (z9–16); how many tiles.
pub fn build_hi(out: &mut Out, sources: &[Source], water: &dyn WaterSource, t: (u32, u32)) -> Result<usize> {
    let rd = Reader::default();
    let made = {
        let mut tl = Tiles::new(out, true);
        for z in HI.0..=HI.1 {
            let mut wc = WaterCache::default();
            let n = 1u32 << (z - 6);
            let k = level(&mut tl, sources, &rd, water, &mut wc, z, t.0 * n, t.1 * n, n, 3)?;
            crate::agent::jobs::report(u64::from(z - HI.0 + 1), u64::from(HI.1 - HI.0 + 1), "zooms");
            eprintln!("depth 6/{}/{}: z{z}, {k} tiles", t.0, t.1);
        }
        std::mem::take(&mut tl.made)
    };
    put_packs(out, &made, "hi", (6, t.0, t.1))
}

/// The sea and lakes `polys` of a test: squares in a tile's pixels.
#[cfg(test)]
pub fn square(kind: crate::terrain_water::Kind, id: u64, x0: f64, y0: f64, x1: f64, y1: f64) -> Poly {
    Poly { kind, id, rings: vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]] }
}

