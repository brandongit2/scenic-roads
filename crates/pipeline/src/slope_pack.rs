//! Slope tiles per pack (docs/plan.md §6, global-source layers): slope in percent from the
//! terrain's z12 (Horn's method with neighbouring tiles), not stored; z11 and coarser stored, each
//! pixel the four quarters of the slopes beneath it (`roadcore::slope::merge4`), or where no finer
//! level covers it, the slope of its own level's terrain. Today's `slope` step does the same over a
//! region's archive; this makes one z3 pack's z6 tiles at a time from the build's terrain packs.

use det::Det;
use crate::out::Out;
use crate::terrain_pack::ManifestTiles;
use anyhow::Result;
use rayon::prelude::*;
use roadcore::grid::{decode_terrain_png, upsampled};
use roadcore::slope::{channels, decode_slope4, merge4, png_rgba, quarters, Quarters};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

const TS: usize = 256;
pub const MAXZ: u8 = 12;
/// Decoded terrain tiles kept (256 KB each): a slope tile reads its own and its four neighbours',
/// which the depth-first build reaches close together.
const KEEP: usize = 512;

/// A tile kept: its z/x/y and how many levels up the tile it was made from is (0: its own).
type Key = (u8, u32, u32, u8);
/// A tile in metres; None when its PNG doesn't decode.
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
    /// `get` reads a Terrarium PNG tile, `has` says whether there is one.
    pub fn new(get: &'a (dyn Fn(u8, u32, u32) -> Option<Vec<u8>> + Sync), has: &'a (dyn Fn(u8, u32, u32) -> bool + Sync)) -> Self {
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
        (self.get)(z, x, y).map(|b| decode_terrain_png(&b).ok().map(Arc::new))
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

/// Slope in percent (Horn's method) of tile (z, x, y), using its edge neighbours.
pub fn slope_tile(terrain: &Terrain, z: u8, x: u32, y: u32) -> Option<Vec<f32>> {
    let e = terrain.tile(z, x, y)?;
    let n = 1u32 << z;
    let nb = |dx: i64, dy: i64| -> Option<Arc<Vec<f32>>> {
        let nx = (x as i64 + dx).rem_euclid(n as i64) as u32;
        let ny = y as i64 + dy;
        if ny < 0 || ny >= n as i64 {
            return None;
        }
        terrain.tile(z, nx, ny as u32)
    };
    let (west, east, north, south) = (nb(-1, 0), nb(1, 0), nb(0, -1), nb(0, 1));
    let (e, west, east, north, south) = (&e[..], west.as_deref().map(|v| &v[..]), east.as_deref().map(|v| &v[..]), north.as_deref().map(|v| &v[..]), south.as_deref().map(|v| &v[..]));
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
    let mut out = vec![0f32; TS * TS];
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
            out[j * 256 + i] = ((dzdx * dzdx + dzdy * dzdy).sqrt() * 100.0).min(500.0);
        }
    }
    Some(out)
}

/// A tile's quarters merged 2×2 for its parent's quadrant (128 × 128, slope × 100).
pub fn quadrant(v: &[Quarters]) -> Vec<[u16; 4]> {
    let mut q = vec![[0u16; 4]; 128 * 128];
    for j in 0..128 {
        for i in 0..128 {
            let (a, b) = ((2 * j) * TS + 2 * i, (2 * j + 1) * TS + 2 * i);
            q[j * 128 + i] = merge4([&v[a], &v[a + 1], &v[b], &v[b + 1]]).map(|s| (s * 100.0).round().clamp(0.0, 65535.0) as u16);
        }
    }
    q
}

/// A tile's quarters: its children's quadrants where they cover it, else its own terrain's slope.
fn compose(terrain: &Terrain, z: u8, x: u32, y: u32, kids: &[((u32, u32), Vec<[u16; 4]>)]) -> Option<Vec<Quarters>> {
    let mut val = vec![[0u16; 4]; TS * TS];
    let mut has = vec![false; TS * TS];
    for ((cx, cy), q) in kids {
        let (ox, oy) = ((cx % 2) as usize * 128, (cy % 2) as usize * 128);
        for j in 0..128 {
            let row = (oy + j) * TS + ox;
            val[row..row + 128].copy_from_slice(&q[j * 128..(j + 1) * 128]);
            has[row..row + 128].iter_mut().for_each(|h| *h = true);
        }
    }
    let full = has.iter().all(|&h| h);
    let direct = if full { None } else { slope_tile(terrain, z, x, y) };
    if kids.is_empty() && direct.is_none() {
        return None;
    }
    Some((0..TS * TS).map(|p| if has[p] { val[p].map(|q| q as f32 / 100.0) } else { [direct.as_ref().map_or(0.0, |d| d[p]); 4] }).collect())
}

/// How far a slope build is (`build_q_with`): its tiles worked out, said at most once a second.
struct Count<'a> {
    done: std::sync::atomic::AtomicU64,
    total: u64,
    said: std::sync::Mutex<std::time::Instant>,
    on: &'a (dyn Fn(&str, u64, u64) + Sync),
}

impl Count<'_> {
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

/// Builds tile (z, x, y) after its children among `tiles` (depth first); stores z ≤ 11 in `out`.
fn build(terrain: &Terrain, tiles: &HashSet<(u8, u32, u32)>, z: u8, x: u32, y: u32, out: &std::sync::Mutex<Vec<(u8, u32, u32, Vec<u8>)>>, count: &Count) -> Option<Vec<[u16; 4]>> {
    let kids: Vec<((u32, u32), Vec<[u16; 4]>)> = if z < MAXZ {
        (0..4u32)
            .map(|k| (2 * x + (k & 1), 2 * y + (k >> 1)))
            .filter(|&(cx, cy)| tiles.contains(&(z + 1, cx, cy)))
            .collect::<Vec<_>>()
            .into_par_iter()
            .filter_map(|(cx, cy)| build(terrain, tiles, z + 1, cx, cy, out, count).map(|q| ((cx, cy), q)))
            .collect()
    } else {
        Vec::new()
    };
    count.one();
    let v = compose(terrain, z, x, y, &kids)?;
    if z < MAXZ {
        let (blob, v) = stored(&v);
        out.lock().unwrap().push((z, x, y, blob));
        return (z > 0).then(|| quadrant(&v));
    }
    (z > 0).then(|| quadrant(&v))
}

/// A tile as stored, and its quarters as read back (from the bytes encoded, which the PNG keeps):
/// parents are made from the stored values, so a tile made now and one read from its pack later
/// give the same parents.
fn stored(v: &[Quarters]) -> (Vec<u8>, Vec<Quarters>) {
    let rgba = channels(v);
    let blob = png_rgba(&rgba, TS as u32, TS as u32).expect("png");
    (blob, quarters(&rgba))
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub hi_tiles: usize,
    pub lo_tiles: usize,
}

/// The slope of the z6 tiles `ts` (in z3 tile `q`) from the build's terrain packs: each one's hi pack
/// (z9–11), written as soon as it's worked out (an area holds a z6 tile's tiles at a time, under a
/// GB, where all of them took up to 20 GB), and `q`'s lo pack (z3–8), the other z6 tiles of `q`
/// taken from the slope lo pack as it is.
pub fn build_q(out: &mut Out, q: (u32, u32), ts: &[(u32, u32)]) -> Result<Report> {
    build_q_with(out, q, ts, &|_, _, _| {})
}

/// `build_q`, saying how far it is to `on` as (what, done, total): the slope tiles worked out (each z6
/// tile's hi pack written with them), then the lo pack written.
pub fn build_q_with(out: &mut Out, q: (u32, u32), ts: &[(u32, u32)], on: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<Report> {
    let mut rep = Report::default();
    let terr = ManifestTiles::new(out, "terrain");
    let slope_now = ManifestTiles::new(out, "slope");
    let get = |z: u8, x: u32, y: u32| -> Option<Vec<u8>> { terr.get(z, x, y).ok().flatten() };
    let has = |z: u8, x: u32, y: u32| terr.has(z, x, y).unwrap_or(false);
    let terrain = Terrain::new(&get, &has);
    // Each z6 tile: the terrain tiles it has (z9–12) and their ancestors down to z6.
    let mut z6q: HashMap<(u32, u32), Vec<[u16; 4]>> = HashMap::new();
    let mut sets: Vec<((u32, u32), HashSet<(u8, u32, u32)>)> = Vec::new();
    for &(tx, ty) in ts {
        let mut tiles: HashSet<(u8, u32, u32)> = HashSet::new();
        for z in 9..=12u8 {
            let s = 1u32 << (z - 6);
            for x in tx * s..(tx + 1) * s {
                for y in ty * s..(ty + 1) * s {
                    if has(z, x, y) {
                        for dz in 0..=(z - 6) {
                            tiles.insert((z - dz, x >> dz, y >> dz));
                        }
                    }
                }
            }
        }
        tiles.insert((6, tx, ty));
        sets.push(((tx, ty), tiles));
    }
    // (Every tile worked out counts, the rest of q's z6 tiles and its z5–z3 too.)
    let total = sets.iter().map(|s| s.1.len() as u64).sum::<u64>() + 64 - ts.len().min(64) as u64 + 16 + 4 + 1;
    let count = Count { done: Default::default(), total, said: std::sync::Mutex::new(std::time::Instant::now()), on };
    on("slope tiles worked out", 0, total);
    // Each z6 tile's hi pack (z9–11) written once it's worked out; its z6–8 tiles kept for the lo
    // pack. (In each pack its tiles in order, as when all were written at the end: the same bytes.)
    let mut made: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
    for ((tx, ty), tiles) in &sets {
        let mine = std::sync::Mutex::new(Vec::new());
        if let Some(qd) = build(&terrain, tiles, 6, *tx, *ty, &mine, &count) {
            z6q.insert((*tx, *ty), qd);
        }
        let (mut hi, lo): (Vec<_>, Vec<_>) = mine.into_inner().unwrap().into_iter().partition(|t| t.0 >= 9);
        made.extend(lo);
        hi.sort_by_key(|t| (t.0, t.1, t.2));
        hi.dedup_by_key(|t| (t.0, t.1, t.2));
        rep.hi_tiles += hi.len();
        let mut it = hi.into_iter().map(|(z, x, y, b)| (z, x, y, b, (TS * TS * 4) as u32));
        crate::layers::write_pack(out, "slope", "slope4-png", false, "hi", (6, *tx, *ty), &mut it)?;
    }
    // The other z6 tiles of q: their stored slope's quadrant, else their terrain's own slope.
    for x in q.0 * 8..(q.0 + 1) * 8 {
        for y in q.1 * 8..(q.1 + 1) * 8 {
            if z6q.contains_key(&(x, y)) {
                continue;
            }
            count.one();
            let kept = slope_now.get(6, x, y)?;
            let v = match kept.as_ref().and_then(|b| decode_slope4(b)) {
                Some(v) => {
                    // Kept as stored (byte for byte), its quadrant read from it.
                    made.push((6, x, y, kept.unwrap()));
                    v
                }
                None => match compose(&terrain, 6, x, y, &[]) {
                    Some(v) => {
                        let (blob, v) = stored(&v);
                        made.push((6, x, y, blob));
                        v
                    }
                    None => continue,
                },
            };
            z6q.insert((x, y), quadrant(&v));
        }
    }
    // z5 → z3 of q from the z6 quadrants.
    let mut below = z6q;
    for z in (3..=5u8).rev() {
        let s = 1u32 << (z - 3);
        let mut next = HashMap::new();
        for x in q.0 * s..(q.0 + 1) * s {
            for y in q.1 * s..(q.1 + 1) * s {
                let kids: Vec<((u32, u32), Vec<[u16; 4]>)> = (0..4u32).filter_map(|k| {
                    let c = (2 * x + (k & 1), 2 * y + (k >> 1));
                    below.get(&c).map(|q| (c, q.clone()))
                }).collect();
                count.one();
                if let Some(v) = compose(&terrain, z, x, y, &kids) {
                    let (blob, v) = stored(&v);
                    made.push((z, x, y, blob));
                    next.insert((x, y), quadrant(&v));
                }
            }
        }
        below = next;
    }
    drop(terrain);
    drop((terr, slope_now));
    // The lo pack: q's z3–8 (this run's z6 tiles' z6–8 with the rest of q's).
    made.sort_by_key(|t| (t.0, t.1, t.2));
    made.dedup_by_key(|t| (t.0, t.1, t.2));
    on("packs written", 0, 1);
    // The lo pack keeps the z7–8 tiles of the other z6 tiles of q as they are.
    let lo_old = ManifestTiles::new(out, "slope");
    let ours: HashSet<(u32, u32)> = ts.iter().copied().collect();
    let mut lo: Vec<(u8, u32, u32, Vec<u8>)> = made.into_iter().filter(|t| t.0 <= 8).collect();
    for z in 7..=8u8 {
        let s = 1u32 << (z - 3);
        for x in q.0 * s..(q.0 + 1) * s {
            for y in q.1 * s..(q.1 + 1) * s {
                if ours.contains(&(x >> (z - 6), y >> (z - 6))) {
                    continue;
                }
                if let Some(b) = lo_old.get(z, x, y)? {
                    lo.push((z, x, y, b));
                }
            }
        }
    }
    drop(lo_old);
    lo.sort_by_key(|t| (t.0, t.1, t.2));
    rep.lo_tiles = lo.len();
    let mut it = lo.into_iter().map(|(z, x, y, b)| (z, x, y, b, (TS * TS * 4) as u32));
    crate::layers::write_pack(out, "slope", "slope4-png", false, "lo", (3, q.0, q.1), &mut it)?;
    out.save()?;
    on("packs written", 1, 1);
    Ok(rep)
}

/// Whether a terrain PNG decodes (for checks).
pub fn decodes(b: &[u8]) -> bool {
    decode_terrain_png(b).is_ok()
}

/// The root pack (z0–2) made from the 64 z3 slope tiles as stored (their quadrants), where a z3
/// tile is missing from its own terrain's slope.
pub fn build_root(out: &mut Out) -> Result<usize> {
    let terr = ManifestTiles::new(out, "terrain");
    let have = ManifestTiles::new(out, "slope");
    let get = |z: u8, x: u32, y: u32| -> Option<Vec<u8>> { terr.get(z, x, y).ok().flatten() };
    let has = |z: u8, x: u32, y: u32| terr.has(z, x, y).unwrap_or(false);
    let terrain = Terrain::new(&get, &has);
    let mut below: HashMap<(u32, u32), Vec<[u16; 4]>> = HashMap::new();
    for x in 0..8u32 {
        for y in 0..8u32 {
            if let Some(v) = have.get(3, x, y)?.and_then(|b| decode_slope4(&b)) {
                below.insert((x, y), quadrant(&v));
            }
        }
    }
    let mut made: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
    for z in (0..=2u8).rev() {
        let n = 1u32 << z;
        let mut next = HashMap::new();
        for x in 0..n {
            for y in 0..n {
                let kids: Vec<((u32, u32), Vec<[u16; 4]>)> = (0..4u32).filter_map(|k| {
                    let c = (2 * x + (k & 1), 2 * y + (k >> 1));
                    below.get(&c).map(|q| (c, q.clone()))
                }).collect();
                if let Some(v) = compose(&terrain, z, x, y, &kids) {
                    let (blob, v) = stored(&v);
                    made.push((z, x, y, blob));
                    if z > 0 {
                        next.insert((x, y), quadrant(&v));
                    }
                }
            }
        }
        below = next;
    }
    drop(terrain);
    drop((terr, have));
    made.sort_by_key(|t| (t.0, t.1, t.2));
    let n = made.len();
    let mut it = made.into_iter().map(|(z, x, y, b)| (z, x, y, b, (TS * TS * 4) as u32));
    crate::layers::write_pack(out, "slope", "slope4-png", false, "root", (0, 0, 0), &mut it)?;
    out.save()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use roadcore::grid::{encode_terrain_png, tile_with_fallback_by};

    /// Horn's slope as computed straight from the PNGs, each neighbour value looked up per pixel.
    fn slope_direct(get: &dyn Fn(u8, u32, u32) -> Option<Vec<u8>>, z: u8, x: u32, y: u32) -> Option<Vec<f32>> {
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
        let mut out = vec![0f32; TS * TS];
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
                out[(j * 256 + i) as usize] = ((dzdx * dzdx + dzdy * dzdy).sqrt() * 100.0).min(500.0);
            }
        }
        Some(out)
    }

    #[test]
    fn terrain_and_slopes_as_read_straight_from_the_tiles() {
        // A z8 tile and three of its z9 children (the fourth upsampled from it), a z9 tile that
        // doesn't decode, and a z10 tile under the missing child (its siblings from two levels up).
        let hill = |z: u8, x: u32, y: u32| -> Vec<u8> {
            let e: Vec<f32> = (0..TS * TS).map(|p| ((p % TS) as f32 * 0.37 + (p / TS) as f32 * 1.9 + x as f32 * 7.0 - y as f32 * 3.0 + z as f32).dsin() * 40.0 + 300.0).collect();
            encode_terrain_png(&e, TS as u32, TS as u32).unwrap()
        };
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
        let asked = [(8, 100, 50), (9, 200, 100), (9, 201, 101), (9, 202, 100), (10, 402, 202), (10, 403, 203), (10, 404, 200), (9, 5, 5), (9, 199, 100)];
        for _ in 0..2 {
            for &(z, x, y) in &asked {
                assert_eq!(terrain.tile(z, x, y).map(|t| (*t).clone()), tile_with_fallback_by(&get, z, x, y), "{z}/{x}/{y}");
                let bits = |v: Option<Vec<f32>>| v.map(|v| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>());
                assert_eq!(bits(slope_tile(&terrain, z, x, y)), bits(slope_direct(&get, z, x, y)), "{z}/{x}/{y}");
            }
        }
    }

    /// An area's slope made as it was before each z6 tile's pack was written as it was worked out:
    /// every tile held until the end (the reference for the test below).
    fn whole_area(out: &mut Out, q: (u32, u32), ts: &[(u32, u32)], on: &(dyn Fn(&str, u64, u64) + Sync)) -> Result<Report> {
        let mut rep = Report::default();
        let terr = ManifestTiles::new(out, "terrain");
        let slope_now = ManifestTiles::new(out, "slope");
        let get = |z: u8, x: u32, y: u32| -> Option<Vec<u8>> { terr.get(z, x, y).ok().flatten() };
        let has = |z: u8, x: u32, y: u32| terr.has(z, x, y).unwrap_or(false);
        let terrain = Terrain::new(&get, &has);
        // Each z6 tile: the terrain tiles it has (z9–12) and their ancestors down to z6.
        let made = std::sync::Mutex::new(Vec::new());
        let mut z6q: HashMap<(u32, u32), Vec<[u16; 4]>> = HashMap::new();
        let mut sets: Vec<((u32, u32), HashSet<(u8, u32, u32)>)> = Vec::new();
        for &(tx, ty) in ts {
            let mut tiles: HashSet<(u8, u32, u32)> = HashSet::new();
            for z in 9..=12u8 {
                let s = 1u32 << (z - 6);
                for x in tx * s..(tx + 1) * s {
                    for y in ty * s..(ty + 1) * s {
                        if has(z, x, y) {
                            for dz in 0..=(z - 6) {
                                tiles.insert((z - dz, x >> dz, y >> dz));
                            }
                        }
                    }
                }
            }
            tiles.insert((6, tx, ty));
            sets.push(((tx, ty), tiles));
        }
        // (Every tile worked out counts, the rest of q's z6 tiles and its z5–z3 too.)
        let total = sets.iter().map(|s| s.1.len() as u64).sum::<u64>() + 64 - ts.len().min(64) as u64 + 16 + 4 + 1;
        let count = Count { done: Default::default(), total, said: std::sync::Mutex::new(std::time::Instant::now()), on };
        on("slope tiles worked out", 0, total);
        for ((tx, ty), tiles) in &sets {
            if let Some(qd) = build(&terrain, tiles, 6, *tx, *ty, &made, &count) {
                z6q.insert((*tx, *ty), qd);
            }
        }
        // The other z6 tiles of q: their stored slope's quadrant, else their terrain's own slope.
        let mut made = made.into_inner().unwrap();
        for x in q.0 * 8..(q.0 + 1) * 8 {
            for y in q.1 * 8..(q.1 + 1) * 8 {
                if z6q.contains_key(&(x, y)) {
                    continue;
                }
                count.one();
                let kept = slope_now.get(6, x, y)?;
                let v = match kept.as_ref().and_then(|b| decode_slope4(b)) {
                    Some(v) => {
                        // Kept as stored (byte for byte), its quadrant read from it.
                        made.push((6, x, y, kept.unwrap()));
                        v
                    }
                    None => match compose(&terrain, 6, x, y, &[]) {
                        Some(v) => {
                            let (blob, v) = stored(&v);
                            made.push((6, x, y, blob));
                            v
                        }
                        None => continue,
                    },
                };
                z6q.insert((x, y), quadrant(&v));
            }
        }
        // z5 → z3 of q from the z6 quadrants.
        let mut below = z6q;
        for z in (3..=5u8).rev() {
            let s = 1u32 << (z - 3);
            let mut next = HashMap::new();
            for x in q.0 * s..(q.0 + 1) * s {
                for y in q.1 * s..(q.1 + 1) * s {
                    let kids: Vec<((u32, u32), Vec<[u16; 4]>)> = (0..4u32).filter_map(|k| {
                        let c = (2 * x + (k & 1), 2 * y + (k >> 1));
                        below.get(&c).map(|q| (c, q.clone()))
                    }).collect();
                    count.one();
                    if let Some(v) = compose(&terrain, z, x, y, &kids) {
                        let (blob, v) = stored(&v);
                        made.push((z, x, y, blob));
                        next.insert((x, y), quadrant(&v));
                    }
                }
            }
            below = next;
        }
        drop(terrain);
        drop((terr, slope_now));
        // Packs: each z6 tile's z9–11, and q's z3–8 (this run's z6 tiles' z6–8 with the rest of q's).
        made.sort_by_key(|t| (t.0, t.1, t.2));
        made.dedup_by_key(|t| (t.0, t.1, t.2));
        let packs = ts.len() as u64 + 1;
        for (k, &(tx, ty)) in ts.iter().enumerate() {
            on("packs written", k as u64, packs);
            let mut it = made.iter().filter(|t| t.0 >= 9 && (t.1 >> (t.0 - 6), t.2 >> (t.0 - 6)) == (tx, ty)).map(|t| (t.0, t.1, t.2, t.3.clone(), (TS * TS * 4) as u32));
            rep.hi_tiles += made.iter().filter(|t| t.0 >= 9 && (t.1 >> (t.0 - 6), t.2 >> (t.0 - 6)) == (tx, ty)).count();
            crate::layers::write_pack(out, "slope", "slope4-png", false, "hi", (6, tx, ty), &mut it)?;
        }
        on("packs written", packs - 1, packs);
        // The lo pack keeps the z7–8 tiles of the other z6 tiles of q as they are.
        let lo_old = ManifestTiles::new(out, "slope");
        let ours: HashSet<(u32, u32)> = ts.iter().copied().collect();
        let mut lo: Vec<(u8, u32, u32, Vec<u8>)> = made.iter().filter(|t| t.0 <= 8).cloned().collect();
        for z in 7..=8u8 {
            let s = 1u32 << (z - 3);
            for x in q.0 * s..(q.0 + 1) * s {
                for y in q.1 * s..(q.1 + 1) * s {
                    if ours.contains(&(x >> (z - 6), y >> (z - 6))) {
                        continue;
                    }
                    if let Some(b) = lo_old.get(z, x, y)? {
                        lo.push((z, x, y, b));
                    }
                }
            }
        }
        drop(lo_old);
        lo.sort_by_key(|t| (t.0, t.1, t.2));
        rep.lo_tiles = lo.len();
        let mut it = lo.into_iter().map(|(z, x, y, b)| (z, x, y, b, (TS * TS * 4) as u32));
        crate::layers::write_pack(out, "slope", "slope4-png", false, "lo", (3, q.0, q.1), &mut it)?;
        out.save()?;
        on("packs written", packs, packs);
        Ok(rep)
    }

    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let (p, q) = (e.path(), to.join(e.file_name()));
            if p.is_dir() {
                copy_dir(&p, &q);
            } else {
                std::fs::copy(&p, &q).unwrap();
            }
        }
    }

    #[test]
    fn a_z6_tile_at_a_time_makes_what_the_whole_area_did() {
        use crate::terrain_pack::{near_coverage, RawTiles};
        let d = tempfile::tempdir().unwrap();
        let local = d.path().join("local");
        // A small coverage in the far north (z11 the finest there: fewer tiles) on two z6 tiles'
        // edge, and its area's z6 tiles near it; terrain made for it from raw tiles (smooth slopes,
        // every third with a spike, every seventh missing), every level's.
        let cov = crate::coverage::Coverage::from_recipes(&[crate::agent::recipes::Recipe { id: "r".into(), name: "R".into(), outline: vec!["place:11.25,70.5,2".into()] }], None, d.path()).unwrap();
        let by_q = crate::agent::build::coverage_tiles(&cov);
        let (&q, ts) = by_q.iter().next().unwrap();
        assert!(ts.len() >= 2, "{ts:?}");
        let mut want: Vec<(u8, u32, u32)> = Vec::new();
        for z in 9..=12u8 {
            let s = 1u32 << (z - 6);
            for &(tx, ty) in ts {
                for x in tx * s..(tx + 1) * s {
                    for y in ty * s..(ty + 1) * s {
                        if near_coverage(&cov, z, x, y, 20.0) {
                            want.push((z, x, y));
                        }
                    }
                }
            }
        }
        for z in 3..=8u8 {
            let s = 1u32 << (z - 3);
            for x in q.0 * s..(q.0 + 1) * s {
                for y in q.1 * s..(q.1 + 1) * s {
                    want.push((z, x, y));
                }
            }
        }
        for &(z, x, y) in &want {
            std::fs::create_dir_all(local.join(format!("{z}/{x}"))).unwrap();
            let k = z as u32 + x + y;
            if k % 7 == 0 {
                std::fs::write(local.join(format!("{z}/{x}/{y}.none")), b"").unwrap();
                continue;
            }
            let mut e: Vec<f32> = (0..256 * 256).map(|i| 400.0 + (i % 256) as f32 * 0.7 + (i / 256) as f32 * 0.4 + (k % 11) as f32 * 30.0).collect();
            if k % 3 == 0 {
                e[128 * 256 + 128] += 900.0;
                e[64 * 256 + 200] -= 700.0;
            }
            std::fs::write(local.join(format!("{z}/{x}/{y}.png")), encode_terrain_png(&e, 256, 256).unwrap()).unwrap();
        }
        let raw = RawTiles::with_store(&local, &d.path().join("store"));
        let terrain = d.path().join("terrain");
        crate::terrain_pack::build_q(&mut Out::open(&terrain, &d.path().join("terrain-scratch")).unwrap(), &raw, q, ts, &cov).unwrap();
        // Its slope, both ways, each over a copy of the terrain's root.
        let made = |name: &str, way: &dyn Fn(&mut Out)| {
            let root = d.path().join(name);
            copy_dir(&terrain, &root);
            way(&mut Out::open(&root, &d.path().join(format!("{name}-scratch"))).unwrap());
            let out = Out::open(&root, &d.path().join(format!("{name}-scratch"))).unwrap();
            out.manifest.into_iter().filter(|(l, _)| l.starts_with("layers/slope/")).collect::<Vec<_>>()
        };
        let now = made("now", &|out| {
            let r = build_q(out, q, ts).unwrap();
            assert!(r.hi_tiles > 0 && r.lo_tiles > 0, "{r:?}");
        });
        let before = made("before", &|out| {
            whole_area(out, q, ts, &|_, _, _| {}).unwrap();
        });
        assert_eq!(now.len(), ts.len() + 1, "{now:?}");
        assert_eq!(now, before, "the same packs, byte for byte (their content names)");
    }
}
