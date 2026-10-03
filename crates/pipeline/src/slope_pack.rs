//! Slope tiles per pack (docs/plan.md §6, global-source layers): slope in percent from the
//! terrain's z12 (Horn's method with neighbouring tiles), not stored; z11 and coarser stored, each
//! pixel the four quarters of the slopes beneath it (`roadcore::slope::merge4`), or where no finer
//! level covers it, the slope of its own level's terrain. Today's `slope` step does the same over a
//! region's archive; this makes one z3 pack's z6 tiles at a time from the build's terrain packs.

use crate::out::Out;
use crate::terrain_pack::ManifestTiles;
use anyhow::Result;
use rayon::prelude::*;
use roadcore::grid::{decode_terrain_png, tile_with_fallback_by};
use roadcore::slope::{decode_slope4, encode_slope4, merge4, Quarters};
use std::collections::{HashMap, HashSet};

const TS: usize = 256;
pub const MAXZ: u8 = 12;

/// Slope in percent (Horn's method) of tile (z, x, y), using its edge neighbours; terrain from
/// `terrain` (a missing tile from its nearest ancestor).
pub fn slope_tile(terrain: &(dyn Fn(u8, u32, u32) -> Option<Vec<u8>> + Sync), z: u8, x: u32, y: u32) -> Option<Vec<f32>> {
    let tf = |z: u8, x: u32, y: u32| tile_with_fallback_by(terrain, z, x, y);
    let e = tf(z, x, y)?;
    let n = 1u32 << z;
    let nb = |dx: i64, dy: i64| -> Option<Vec<f32>> {
        let nx = (x as i64 + dx).rem_euclid(n as i64) as u32;
        let ny = y as i64 + dy;
        if ny < 0 || ny >= n as i64 {
            return None;
        }
        tf(z, nx, ny as u32)
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
    let world = 40_075_016.686f64;
    let mut out = vec![0f32; TS * TS];
    for j in 0..256i32 {
        let yy = (y as f64 + (j as f64 + 0.5) / 256.0) / n as f64;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * yy)).sinh().atan();
        let d = (world * lat.cos() / (256.0 * n as f64)) as f32;
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
fn compose(terrain: &(dyn Fn(u8, u32, u32) -> Option<Vec<u8>> + Sync), z: u8, x: u32, y: u32, kids: &[((u32, u32), Vec<[u16; 4]>)]) -> Option<Vec<Quarters>> {
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

/// Builds tile (z, x, y) after its children among `tiles` (depth first); stores z ≤ 11 in `out`.
fn build(terrain: &(dyn Fn(u8, u32, u32) -> Option<Vec<u8>> + Sync), tiles: &HashSet<(u8, u32, u32)>, z: u8, x: u32, y: u32, out: &std::sync::Mutex<Vec<(u8, u32, u32, Vec<u8>)>>) -> Option<Vec<[u16; 4]>> {
    let kids: Vec<((u32, u32), Vec<[u16; 4]>)> = if z < MAXZ {
        (0..4u32)
            .map(|k| (2 * x + (k & 1), 2 * y + (k >> 1)))
            .filter(|&(cx, cy)| tiles.contains(&(z + 1, cx, cy)))
            .collect::<Vec<_>>()
            .into_par_iter()
            .filter_map(|(cx, cy)| build(terrain, tiles, z + 1, cx, cy, out).map(|q| ((cx, cy), q)))
            .collect()
    } else {
        Vec::new()
    };
    let v = compose(terrain, z, x, y, &kids)?;
    if z < MAXZ {
        let (blob, v) = stored(&v);
        out.lock().unwrap().push((z, x, y, blob));
        return (z > 0).then(|| quadrant(&v));
    }
    (z > 0).then(|| quadrant(&v))
}

/// A tile as stored, and its quarters as read back: parents are made from the stored values, so a
/// tile made now and one read from its pack later give the same parents.
fn stored(v: &[Quarters]) -> (Vec<u8>, Vec<Quarters>) {
    let blob = encode_slope4(v, TS as u32, TS as u32).expect("png");
    let back = decode_slope4(&blob).expect("a slope tile just encoded decodes");
    (blob, back)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub hi_tiles: usize,
    pub lo_tiles: usize,
}

/// The slope of the z6 tiles `ts` (in z3 tile `q`) from the build's terrain packs: each one's hi pack
/// (z9–11) and `q`'s lo pack (z3–8), the other z6 tiles of `q` taken from the slope lo pack as it is.
pub fn build_q(out: &mut Out, q: (u32, u32), ts: &[(u32, u32)]) -> Result<Report> {
    let mut rep = Report::default();
    let terr = ManifestTiles::new(out, "terrain");
    let slope_now = ManifestTiles::new(out, "slope");
    let get = |z: u8, x: u32, y: u32| -> Option<Vec<u8>> { terr.get(z, x, y).ok().flatten() };
    // Each z6 tile: the terrain tiles it has (z9–12) and their ancestors down to z6.
    let made = std::sync::Mutex::new(Vec::new());
    let mut z6q: HashMap<(u32, u32), Vec<[u16; 4]>> = HashMap::new();
    for &(tx, ty) in ts {
        let mut tiles: HashSet<(u8, u32, u32)> = HashSet::new();
        for z in 9..=12u8 {
            let s = 1u32 << (z - 6);
            for x in tx * s..(tx + 1) * s {
                for y in ty * s..(ty + 1) * s {
                    if get(z, x, y).is_some() {
                        for dz in 0..=(z - 6) {
                            tiles.insert((z - dz, x >> dz, y >> dz));
                        }
                    }
                }
            }
        }
        tiles.insert((6, tx, ty));
        if let Some(qd) = build(&get, &tiles, 6, tx, ty, &made) {
            z6q.insert((tx, ty), qd);
        }
    }
    // The other z6 tiles of q: their stored slope's quadrant, else their terrain's own slope.
    let mut made = made.into_inner().unwrap();
    for x in q.0 * 8..(q.0 + 1) * 8 {
        for y in q.1 * 8..(q.1 + 1) * 8 {
            if z6q.contains_key(&(x, y)) {
                continue;
            }
            let kept = slope_now.get(6, x, y)?;
            let v = match kept.as_ref().and_then(|b| decode_slope4(b)) {
                Some(v) => {
                    // Kept as stored (byte for byte), its quadrant read from it.
                    made.push((6, x, y, kept.unwrap()));
                    v
                }
                None => match compose(&get, 6, x, y, &[]) {
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
                if let Some(v) = compose(&get, z, x, y, &kids) {
                    let (blob, v) = stored(&v);
                    made.push((z, x, y, blob));
                    next.insert((x, y), quadrant(&v));
                }
            }
        }
        below = next;
    }
    drop((terr, slope_now));
    // Packs: each z6 tile's z9–11, and q's z3–8 (this run's z6 tiles' z6–8 with the rest of q's).
    made.sort_by_key(|t| (t.0, t.1, t.2));
    made.dedup_by_key(|t| (t.0, t.1, t.2));
    for &(tx, ty) in ts {
        let mut it = made.iter().filter(|t| t.0 >= 9 && (t.1 >> (t.0 - 6), t.2 >> (t.0 - 6)) == (tx, ty)).map(|t| (t.0, t.1, t.2, t.3.clone(), (TS * TS * 4) as u32));
        rep.hi_tiles += made.iter().filter(|t| t.0 >= 9 && (t.1 >> (t.0 - 6), t.2 >> (t.0 - 6)) == (tx, ty)).count();
        crate::layers::write_pack(out, "slope", "slope4-png", false, "hi", (6, tx, ty), &mut it)?;
    }
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
    Ok(rep)
}

/// Whether a terrain PNG decodes (for checks).
pub fn decodes(b: &[u8]) -> bool {
    decode_terrain_png(b).is_ok()
}
