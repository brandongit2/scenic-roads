//! Terrain slope tiles (percent, Terrarium-encoded as if it were elevation) for the slope tint.
//!
//! usage: slope <build_dir> [--seed] [--coarse]
//!
//! Slope depends on scale: computed directly from a coarse DEM, the peaks and valleys merge into a
//! gentle surface and low zooms look flat (the White Mountains' high ground: 25 % at z12, 15 % at
//! z8, 1.4 % at z4). So slope is computed once at the finest level (z12, Horn's method with
//! neighbouring tiles), and each coarser pixel takes the mean of the slopes beneath it: the average
//! steepness of the ground it covers, which every level keeps (there: 25 % down to z8, 19 % at z4).
//! What fades far out is the steepest class, cliffs narrower than a pixel blending with the gentler
//! ground beside them (≥ 45 %: 9.7 % of that ground at z12, 6.5 % at z8, none at z4). Until
//! 2026-09-30 each coarser pixel took one of the slopes beneath it at random, a halftone that kept
//! the steepest class at every zoom but read as noise zoomed out. Pixels with no finer data (far
//! from roads, where the terrain archive stops at z8) fall back to the slope of that level's own
//! DEM. Writes slope.tiles (served at
//! /tiles/slope/{z}/{x}/{y}). --coarse recomputes every level below z12 from the z12 tiles (after
//! a change to how they are made).
//!
//! Incremental: the terrain tiles of the last run are listed in data/cache/steps/slope.keys, and
//! only slope tiles whose terrain (the tile, its neighbours or their ancestors) is new are
//! recomputed, with their parents; the rest are copied from the previous slope.tiles.

use anyhow::Result;
use pipeline::count_bar;
use rayon::prelude::*;
use roadcore::archive::{Archive, ArchiveWriter};
use roadcore::grid::tile_with_fallback;
use std::collections::HashMap;
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

/// Each parent pixel's slope (the mean of its 2×2 children), and whether a finer tile supplied it.
struct Acc {
    val: Vec<f32>,
    has: Vec<bool>,
}

/// Terrarium PNG of slopes in 1/16 % steps: R is constant, B takes 16 levels, which compresses
/// far better than the elevation encoder's full precision. (Whole percents were too coarse: the
/// map interpolates between pixels, and around a colour threshold integer steps left lens-shaped
/// blotches centred on single pixels instead of smooth edges.)
fn encode(v: &[f32]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(v.len() * 3);
    for &s in v {
        let q = (s.clamp(0.0, 500.0) * 16.0).round() as u32; // 1/16 %
        let e = (q >> 4) + 32768;
        rgb.extend_from_slice(&[(e >> 8) as u8, e as u8, ((q & 15) << 4) as u8]);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, TS as u32, TS as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Balanced);
        enc.set_filter(png::Filter::Adaptive);
        let mut w = enc.write_header().expect("png");
        w.write_image_data(&rgb).expect("png");
    }
    out
}

fn decode(png_bytes: &[u8]) -> Option<Vec<f32>> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let ch = info.color_type.samples();
    Some((0..TS * TS).map(|i| {
        let p = &buf[i * ch..];
        let q = ((((p[0] as u32) << 8 | p[1] as u32).saturating_sub(32768)) << 4) | (p[2] as u32 >> 4);
        q as f32 / 16.0
    }).collect())
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
    let old = Archive::open(&dir.join("slope.tiles")).ok().filter(|_| prev_keys.is_some());
    let new_terrain: std::collections::HashSet<u64> = match &prev_keys {
        Some(p) => arc.entries().iter().map(|e| e.key).filter(|k| !p.contains(k)).collect(),
        None => std::collections::HashSet::new(),
    };
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
    let mut by_z: Vec<Vec<(u32, u32)>> = vec![Vec::new(); MAXZ as usize + 1];
    for e in arc.entries() {
        let z = (e.key >> 58) as u8;
        if z <= MAXZ {
            by_z[z as usize].push((((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32));
        }
    }
    let mut aw = ArchiveWriter::create(&roadcore::tmp(&dir, "slope.tiles"), r#"{"format":"png","encoding":"terrarium","value":"slope percent"}"#)?;
    let in_old = |z: u8, x: u32, y: u32| old.as_ref().is_some_and(|o| o.get(z, x, y).is_some());
    // --coarse: every level below z12 again (z12 copied).
    let coarse = std::env::args().any(|a| a == "--coarse");
    // Accumulators for the level being built, filled from the level above; and the parents that
    // exist because of finer tiles.
    let mut acc: HashMap<(u32, u32), Acc> = HashMap::new();
    let mut present: std::collections::HashSet<(u32, u32)> = Default::default();
    let mut dirty_below: std::collections::HashSet<(u32, u32)> = Default::default();
    let (mut written, mut copied) = (0usize, 0usize);
    for z in (0..=MAXZ).rev() {
        // Tiles at this level: those in the terrain archive plus parents of finer slope tiles.
        let mut tiles: Vec<(u32, u32)> = by_z[z as usize].clone();
        tiles.extend(present.iter().copied().filter(|k| arc.get(z, k.0, k.1).is_none()));
        tiles.sort_unstable();
        tiles.dedup();
        // Recomputed: new terrain here (or nearby, or above), a recomputed child, or not there
        // last time. Everything else is copied.
        let parents_of_dirty: std::collections::HashSet<(u32, u32)> = dirty_below.iter().map(|&(x, y)| (x / 2, y / 2)).collect();
        let dirty: std::collections::HashSet<(u32, u32)> = tiles
            .par_iter()
            .filter(|&&(x, y)| old.is_none() || (coarse && z < MAXZ) || parents_of_dirty.contains(&(x, y)) || !in_old(z, x, y) || terrain_changed(z, x, y))
            .copied()
            .collect();
        let dirty_parents: std::collections::HashSet<(u32, u32)> = dirty.iter().map(|&(x, y)| (x / 2, y / 2)).collect();
        let parent_dirty = |x: u32, y: u32| -> bool {
            z > 0 && (coarse || {
                let (px, py) = (x / 2, y / 2);
                dirty_parents.contains(&(px, py)) || !in_old(z - 1, px, py) || terrain_changed(z - 1, px, py)
            })
        };
        let pb = count_bar(tiles.len() as u64, &format!("slope z{z}"));
        let mut next: HashMap<(u32, u32), Acc> = HashMap::new();
        let mut next_present: std::collections::HashSet<(u32, u32)> = Default::default();
        for chunk in tiles.chunks(1024) {
            // (tile, encoded tile, values when the parent needs them)
            let done: Vec<((u32, u32), Vec<u8>, Option<Vec<f32>>)> = chunk
                .par_iter()
                .filter_map(|&(x, y)| {
                    pb.inc(1);
                    if !dirty.contains(&(x, y)) {
                        let blob = old.as_ref()?.get(z, x, y)?.to_vec();
                        let vals = if parent_dirty(x, y) { decode(&blob) } else { None };
                        return Some(((x, y), blob, vals));
                    }
                    let a = acc.get(&(x, y));
                    let full = a.is_some_and(|a| a.has.iter().all(|&h| h));
                    // Direct slope only where the finer levels don't cover the tile.
                    let direct = if full { None } else { slope_tile(&arc, z, x, y) };
                    if a.is_none() && direct.is_none() {
                        return None;
                    }
                    let mut v = vec![0f32; TS * TS];
                    for p in 0..TS * TS {
                        v[p] = match a {
                            Some(a) if a.has[p] => a.val[p],
                            _ => direct.as_ref().map_or(0.0, |d| d[p]),
                        };
                    }
                    Some(((x, y), encode(&v), Some(v)))
                })
                .collect();
            for ((x, y), blob, vals) in &done {
                aw.add(z, *x, *y, blob, TS * TS * 3)?;
                if dirty.contains(&(*x, *y)) {
                    written += 1;
                } else {
                    copied += 1;
                }
                if z == 0 {
                    continue;
                }
                let (px, py) = (x / 2, y / 2);
                next_present.insert((px, py));
                // Each 2×2 block's mean into the parent's quadrant (when it is recomputed).
                let Some(v) = vals else { continue };
                let (ox, oy) = ((x % 2) as usize * 128, (y % 2) as usize * 128);
                let p = next.entry((px, py)).or_insert_with(|| Acc { val: vec![0.0; TS * TS], has: vec![false; TS * TS] });
                for j in 0..128 {
                    for i in 0..128 {
                        let k = (oy + j) * TS + ox + i;
                        let (a, b) = ((2 * j) * TS + 2 * i, (2 * j + 1) * TS + 2 * i);
                        p.val[k] = (v[a] + v[a + 1] + v[b] + v[b + 1]) * 0.25;
                        p.has[k] = true;
                    }
                }
            }
        }
        pb.finish_and_clear();
        eprintln!("slope z{z}: {} tiles ({} recomputed)", tiles.len(), dirty.len());
        acc = next;
        present = next_present;
        dirty_below = dirty;
    }
    aw.finish()?;
    drop(old);
    roadcore::commit(&dir, &["slope.tiles"])?;
    let keys: Vec<u64> = arc.entries().iter().map(|e| e.key).collect();
    std::fs::write(&keys_path, bytemuck::cast_slice(&keys))?;
    eprintln!("slope: {written} tiles computed, {copied} copied, in {:.0?}", t0.elapsed());
    Ok(())
}
