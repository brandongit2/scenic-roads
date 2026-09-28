//! Terrain slope tiles (percent, Terrarium-encoded as if it were elevation) for the slope tint.
//!
//! usage: slope <build_dir>
//!
//! Slope depends on scale: computed directly from a coarse DEM, steep ground averages out and
//! low zooms look flatter. So slope is computed once at the finest level (z12, Horn's method with
//! neighbouring tiles) and every coarser tile is the *mean of the slopes* beneath it. Pixels with
//! no finer data (far from roads, where the terrain archive stops at z8) fall back to the slope of
//! that level's own DEM. Writes slope.tiles (served at /tiles/slope/{z}/{x}/{y}).

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

/// Sum and count of child slopes per parent pixel.
struct Acc {
    sum: Vec<f32>,
    cnt: Vec<u16>,
}

/// Terrarium PNG of whole-percent slopes: R and B are then constant and only G varies, which
/// compresses several times better than the elevation encoder's fast settings.
fn encode(v: &[f32]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(v.len() * 3);
    for &s in v {
        let e = s.round().clamp(0.0, 500.0) as u32 + 32768;
        rgb.extend_from_slice(&[(e >> 8) as u8, e as u8, 0]);
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

fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "data/build".into()));
    let t0 = std::time::Instant::now();
    let arc = Archive::open(&dir.join("terrain.tiles"))?;
    let mut by_z: Vec<Vec<(u32, u32)>> = vec![Vec::new(); MAXZ as usize + 1];
    for e in arc.entries() {
        let z = (e.key >> 58) as u8;
        if z <= MAXZ {
            by_z[z as usize].push((((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32));
        }
    }
    let mut aw = ArchiveWriter::create(&roadcore::tmp(&dir, "slope.tiles"), r#"{"format":"png","encoding":"terrarium","value":"slope percent"}"#)?;
    // Accumulators for the level being built, filled from the level above.
    let mut acc: HashMap<(u32, u32), Acc> = HashMap::new();
    let mut written = 0usize;
    for z in (0..=MAXZ).rev() {
        // Tiles at this level: those in the terrain archive plus parents of finer slope tiles.
        let mut tiles: Vec<(u32, u32)> = by_z[z as usize].clone();
        for k in acc.keys() {
            if arc.get(z, k.0, k.1).is_none() {
                tiles.push(*k);
            }
        }
        tiles.sort_unstable();
        tiles.dedup();
        let pb = count_bar(tiles.len() as u64, &format!("slope z{z}"));
        let mut next: HashMap<(u32, u32), Acc> = HashMap::new();
        for chunk in tiles.chunks(1024) {
            let done: Vec<((u32, u32), Vec<f32>)> = chunk
                .par_iter()
                .filter_map(|&(x, y)| {
                    let a = acc.get(&(x, y));
                    let full = a.is_some_and(|a| a.cnt.iter().all(|&c| c > 0));
                    // Direct slope only where the finer levels don't cover the tile.
                    let direct = if full { None } else { slope_tile(&arc, z, x, y) };
                    let mut v = vec![0f32; TS * TS];
                    for p in 0..TS * TS {
                        v[p] = match a {
                            Some(a) if a.cnt[p] > 0 => a.sum[p] / a.cnt[p] as f32,
                            _ => direct.as_ref().map_or(0.0, |d| d[p]),
                        };
                    }
                    if a.is_none() && direct.is_none() {
                        return None;
                    }
                    pb.inc(1);
                    Some(((x, y), v))
                })
                .collect();
            for ((x, y), v) in &done {
                aw.add(z, *x, *y, &encode(v), TS * TS * 3)?;
                written += 1;
                if z == 0 {
                    continue;
                }
                // 2×2 mean into the parent's quadrant.
                let (px, py) = (x / 2, y / 2);
                let (ox, oy) = ((x % 2) as usize * 128, (y % 2) as usize * 128);
                let p = next.entry((px, py)).or_insert_with(|| Acc { sum: vec![0.0; TS * TS], cnt: vec![0; TS * TS] });
                for j in 0..128 {
                    for i in 0..128 {
                        let s = v[(2 * j) * TS + 2 * i] + v[(2 * j) * TS + 2 * i + 1] + v[(2 * j + 1) * TS + 2 * i] + v[(2 * j + 1) * TS + 2 * i + 1];
                        let k = (oy + j) * TS + ox + i;
                        p.sum[k] += s / 4.0;
                        p.cnt[k] += 1;
                    }
                }
            }
        }
        pb.finish_and_clear();
        eprintln!("slope z{z}: {} tiles", tiles.len());
        acc = next;
    }
    aw.finish()?;
    roadcore::commit(&dir, &["slope.tiles"])?;
    eprintln!("slope: {written} tiles in {:.0?}", t0.elapsed());
    Ok(())
}
