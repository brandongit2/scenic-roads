//! Terrain slope tiles for the slope tint: per pixel four slopes (percent), the means of the four
//! quarters of the z12 slopes beneath it (roadcore::slope: the map colours each and averages the
//! colours, so a pixel shows the mix of colours its ground would).
//!
//! usage: slope <build_dir> [--seed] [--coarse]
//!
//! Slope depends on scale: computed directly from a coarse DEM, the peaks and valleys merge into a
//! gentle surface and low zooms look flat (the White Mountains' high ground: 25 % at z12, 15 % at
//! z8, 1.4 % at z4). So slope is computed once at the finest level (z12, Horn's method with
//! neighbouring tiles), and each coarser pixel takes the mean of the slopes beneath it: the average
//! steepness of the ground it covers, which every level keeps (there: 25 % down to z8, 19 % at z4).
//! The mean alone faded the steepest class far out, cliffs narrower than a pixel blending with the
//! gentler ground beside them (≥ 45 %: 9.7 % of that ground at z12, 6.5 % at z8, none at z4), so
//! each pixel keeps the quarters of the slopes beneath it: a parent's from its four children's 16,
//! sorted (roadcore::slope::merge4). Until 2026-09-30 each coarser pixel took one of the slopes
//! beneath it at random, a halftone that kept the steepest class at every zoom but read as noise. Pixels with no finer data (far
//! from roads, where the terrain archive stops at z8) fall back to the slope of that level's own
//! DEM. Writes slope.tiles (served at
//! /tiles/slope/{z}/{x}/{y}). --coarse recomputes every level below z12 from the z12 tiles (after
//! a change to how they are made).
//!
//! Incremental: the terrain tiles of the last run are listed in data/cache/steps/slope.keys, and
//! only slope tiles whose terrain (the tile, its neighbours or their ancestors) is new are
//! recomputed, with their parents; the rest are copied from the previous slope.tiles.
//!
//! Built depth first (build), so memory stays at a few hundred megabytes: level by level, every
//! tile's quarters waited for the level above (some 28 GB at z11, swapped out until the disk filled).

use anyhow::Result;
use pipeline::count_bar;
use rayon::prelude::*;
use roadcore::archive::{Archive, ArchiveWriter};
use roadcore::grid::tile_with_fallback;
use roadcore::slope::{decode_slope4, encode_slope4, merge4, Quarters};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::path::PathBuf;

const TS: usize = 256;
const MAXZ: u8 = 12;

/// Slope in percent (Horn's method) of tile (z, x, y), using its edge neighbours.
fn slope_tile(arc: &Archive, z: u8, x: u32, y: u32) -> Option<Vec<f32>> {
    let e = tile_with_fallback(arc, z, x, y)?;
    let n = 1u32 << z;
    let nb = |dx: i64, dy: i64| -> Option<Vec<f32>> {
        let nx = (x as i64 + dx).rem_euclid(n as i64) as u32;
        let ny = y as i64 + dy;
        if ny < 0 || ny >= n as i64 {
            return None;
        }
        tile_with_fallback(arc, z, nx, ny as u32)
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

/// A tile's own quarters merged 2×2 for its parent's quadrant (128 × 128, slope × 100).
fn quadrant(v: &[Quarters]) -> Vec<[u16; 4]> {
    let mut q = vec![[0u16; 4]; 128 * 128];
    for j in 0..128 {
        for i in 0..128 {
            let (a, b) = ((2 * j) * TS + 2 * i, (2 * j + 1) * TS + 2 * i);
            q[j * 128 + i] = merge4([&v[a], &v[a + 1], &v[b], &v[b + 1]]).map(|s| (s * 100.0).round().clamp(0.0, 65535.0) as u16);
        }
    }
    q
}

/// What a run builds from: the terrain, the previous slope tiles, every tile to make, which ones
/// have new terrain; the output and its counts (recomputed, copied) per zoom.
struct Build<'a> {
    arc: &'a Archive,
    old: Option<&'a Archive>,
    tiles: &'a HashSet<u64>,
    coarse: bool,
    terrain_changed: &'a (dyn Fn(u8, u32, u32) -> bool + Sync),
    aw: &'a std::sync::Mutex<ArchiveWriter>,
    pb: indicatif::ProgressBar,
    per_z: Vec<(AtomicUsize, AtomicUsize)>,
}

/// A tile built (build): whether it was recomputed, and if so its quadrant for its parent.
struct Built {
    dirty: bool,
    quad: Option<Vec<[u16; 4]>>,
}

/// Tile (z, x, y) after its children, depth first (each subtree in parallel): only the tiles on the
/// way down are held, a few megabytes, where building a whole level at a time held every tile's
/// quarters for the level above (some 28 GB at z11, swapped out until the disk filled). Recomputed
/// when its terrain (itself, a neighbour, or an ancestor of those) is new, a child was, or it wasn't
/// there last time; else copied. A recomputed tile takes each pixel's quarters from the finer tile
/// beneath (a copied child decoded again), or where none covers it, the slope of its own terrain.
fn build(b: &Build, z: u8, x: u32, y: u32) -> Result<Built> {
    let kids: Vec<((u32, u32), Built)> = if z < MAXZ {
        (0..4u32)
            .map(|k| (2 * x + (k & 1), 2 * y + (k >> 1)))
            .filter(|&(cx, cy)| b.tiles.contains(&roadcore::archive::tile_key(z + 1, cx, cy)))
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(cx, cy)| build(b, z + 1, cx, cy).map(|r| ((cx, cy), r)))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    b.pb.inc(1);
    let prev = b.old.and_then(|o| o.get(z, x, y));
    let dirty = b.old.is_none() || (b.coarse && z < MAXZ) || kids.iter().any(|k| k.1.dirty) || prev.is_none() || (b.terrain_changed)(z, x, y);
    if !dirty {
        let blob = prev.unwrap();
        b.aw.lock().unwrap().add(z, x, y, blob, TS * TS * 4)?;
        b.per_z[z as usize].1.fetch_add(1, Ordering::Relaxed);
        return Ok(Built { dirty: false, quad: None });
    }
    // The children's quadrants (a copied child's decoded again).
    let mut val: Option<(Vec<[u16; 4]>, Vec<bool>)> = None;
    for ((cx, cy), k) in kids {
        let q = match k.quad {
            Some(q) => Some(q),
            None if !k.dirty => b.old.and_then(|o| o.get(z + 1, cx, cy)).and_then(decode_slope4).map(|v| quadrant(&v)),
            None => None,
        };
        let Some(q) = q else { continue };
        let (val, has) = val.get_or_insert_with(|| (vec![[0; 4]; TS * TS], vec![false; TS * TS]));
        let (ox, oy) = ((cx % 2) as usize * 128, (cy % 2) as usize * 128);
        for j in 0..128 {
            let row = (oy + j) * TS + ox;
            val[row..row + 128].copy_from_slice(&q[j * 128..(j + 1) * 128]);
            has[row..row + 128].iter_mut().for_each(|h| *h = true);
        }
    }
    let full = val.as_ref().is_some_and(|(_, has)| has.iter().all(|&h| h));
    // Direct slope only where the finer levels don't cover the tile.
    let direct = if full { None } else { slope_tile(b.arc, z, x, y) };
    if val.is_none() && direct.is_none() {
        return Ok(Built { dirty: true, quad: None });
    }
    let mut v = vec![[0f32; 4]; TS * TS];
    for p in 0..TS * TS {
        v[p] = match &val {
            Some((val, has)) if has[p] => val[p].map(|q| q as f32 / 100.0),
            _ => [direct.as_ref().map_or(0.0, |d| d[p]); 4],
        };
    }
    let blob = encode_slope4(&v, TS as u32, TS as u32).expect("png");
    b.aw.lock().unwrap().add(z, x, y, &blob, TS * TS * 4)?;
    b.per_z[z as usize].0.fetch_add(1, Ordering::Relaxed);
    Ok(Built { dirty: true, quad: (z > 0).then(|| quadrant(&v)) })
}

fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "data/build".into()));
    let t0 = std::time::Instant::now();
    let arc = Archive::open(&dir.join("terrain.tiles"))?;
    // Last run: its terrain keys and slope tiles (still in place until the commit).
    let cdir = dir.parent().unwrap_or(std::path::Path::new(".")).join("cache/steps");
    std::fs::create_dir_all(&cdir)?;
    let keys_path = cdir.join("slope.keys");
    // --seed: record the current terrain keys (for a slope.tiles built before this cache existed).
    if std::env::args().any(|a| a == "--seed") {
        let keys: Vec<u64> = arc.entries().iter().map(|e| e.key).collect();
        std::fs::write(&keys_path, bytemuck::cast_slice(&keys))?;
        eprintln!("slope: seeded {} terrain keys", keys.len());
        return Ok(());
    }
    let prev_keys: Option<std::collections::HashSet<u64>> = std::fs::read(&keys_path)
        .ok()
        .filter(|b| b.len() % 8 == 0 && !b.is_empty())
        .map(|b| bytemuck::cast_slice::<u8, u64>(&b).iter().copied().collect());
    // (Tiles of another encoding than the quarters are all made again.)
    let old = Archive::open(&dir.join("slope.tiles")).ok().filter(|a| prev_keys.is_some() && a.meta_json.contains("slope4"));
    let mut new_terrain: std::collections::HashSet<u64> = match &prev_keys {
        Some(p) => arc.entries().iter().map(|e| e.key).filter(|k| !p.contains(k)).collect(),
        None => std::collections::HashSet::new(),
    };
    // Tiles repaired in place since the last run (terrain.rs) count as new.
    let repaired = roadcore::archive::terrain_repaired_since(&cdir, &keys_path);
    if !repaired.is_empty() {
        eprintln!("slope: {} terrain tiles repaired since the last run", repaired.len());
    }
    new_terrain.extend(repaired);
    let key = |z: u8, x: i64, y: i64| roadcore::archive::tile_key(z, x.rem_euclid(1 << z) as u32, y.clamp(0, (1 << z) - 1) as u32);
    // A tile whose terrain (itself, a neighbour, or an ancestor of those) is new.
    let terrain_changed = |z: u8, x: u32, y: u32| -> bool {
        if old.is_none() {
            return true;
        }
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                for dz in 0..=z {
                    if new_terrain.contains(&key(z - dz, (x as i64 + dx) >> dz, (y as i64 + dy) >> dz)) {
                        return true;
                    }
                }
            }
        }
        false
    };
    let aw = std::sync::Mutex::new(ArchiveWriter::create(&roadcore::tmp(&dir, "slope.tiles"), r#"{"format":"png","encoding":"slope4","value":"slope percent, quarter means"}"#)?);
    // --coarse: every level below z12 again (z12 copied).
    let coarse = std::env::args().any(|a| a == "--coarse");
    // Every slope tile: the terrain's and their ancestors (a parent of finer tiles exists even where
    // the terrain archive has none).
    let mut tiles: HashSet<u64> = HashSet::new();
    for e in arc.entries() {
        let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32);
        if z > MAXZ {
            continue;
        }
        for dz in 0..=z {
            if !tiles.insert(roadcore::archive::tile_key(z - dz, x >> dz, y >> dz)) && dz > 0 {
                break; // (its ancestors are in already)
            }
        }
    }
    let b = Build {
        arc: &arc,
        old: old.as_ref(),
        tiles: &tiles,
        coarse,
        terrain_changed: &terrain_changed,
        aw: &aw,
        pb: count_bar(tiles.len() as u64, "slope tiles"),
        per_z: (0..=MAXZ).map(|_| (AtomicUsize::new(0), AtomicUsize::new(0))).collect(),
    };
    if tiles.contains(&roadcore::archive::tile_key(0, 0, 0)) {
        build(&b, 0, 0, 0)?;
    }
    b.pb.finish_and_clear();
    let (mut written, mut copied) = (0usize, 0usize);
    for (z, (w, c)) in b.per_z.iter().enumerate().rev() {
        let (w, c) = (w.load(Ordering::Relaxed), c.load(Ordering::Relaxed));
        eprintln!("slope z{z}: {} tiles ({w} recomputed)", w + c);
        written += w;
        copied += c;
    }
    let aw = aw.into_inner().unwrap();
    aw.finish()?;
    drop(old);
    roadcore::commit(&dir, &["slope.tiles"])?;
    let keys: Vec<u64> = arc.entries().iter().map(|e| e.key).collect();
    std::fs::write(&keys_path, bytemuck::cast_slice(&keys))?;
    eprintln!("slope: {written} tiles computed, {copied} copied, in {:.0?}", t0.elapsed());
    Ok(())
}
