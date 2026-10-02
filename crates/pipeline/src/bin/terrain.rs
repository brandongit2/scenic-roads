//! Terrain tiles for 3D terrain / hillshade, and the z11 terrain analysis grid.
//!
//! usage: terrain <build_dir>
//!
//! Downloads Terrarium-encoded DEM tiles (AWS Terrain Tiles: USGS 3DEP in the US, NRCan
//! CDEM in Canada, ~27 m at z12) for z0–8 over the region and z9–12 within one tile of a
//! road. Below-sea-level values (ocean bathymetry) are clamped to 0 so the sea stays flat
//! in 3D. Voids (AWS fills some with 32767 m: a z9 pixel on the Toyama shore, clusters of
//! hundreds of z12 pixels along the US–Canada border) and single-pixel spikes and pits are
//! repaired (roadcore::grid::repair_terrain), finest zoom first, and every pixel above a repaired
//! one is made again from its four below (the coarse tiles averaged the voids in: 18 km at z8 above
//! Toyama, kilometres more up to z5). From z8 down, every quarter of a tile whose child tile exists
//! is made again from it (process, REBUILD_Z): AWS's coarse levels come from coarser sources, which
//! lost peaks as you zoomed out. Tiles already in a previous archive are reused (and repaired the
//! same way). Writes:
//!   terrain.tiles       tile archive of Terrarium PNGs (served for MapLibre raster-dem)
//!   grid.idx            z11 tiles within ~14 km of a road (shared by all analysis layers)
//!   grid.terrain.i16    their elevations in metres

use anyhow::Result;
use pipeline::count_bar;
use pipeline::terrain_pack::{fetch, process, tile_lat, Repaired};
use rayon::prelude::*;
use roadcore::archive::{Archive, ArchiveWriter};
use roadcore::grid::{decode_terrain_png, repair_terrain, GridIndex, CELLS};
use roadcore::{Ways, E7};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;


fn tile_of(lon: f64, lat: f64, z: u8) -> (i64, i64) {
    let n = (1u64 << z) as f64;
    let (x, y) = roadcore::merc(lon, lat);
    ((x * n).floor() as i64, (y * n).floor() as i64)
}

/// Tiles containing road vertices at zoom z, dilated by `ring` tiles.
pub fn near_roads(verts: &[[i32; 2]], z: u8, ring: i64) -> Vec<[u32; 2]> {
    let n = 1i64 << z;
    let base: HashSet<(i64, i64)> = verts
        .par_iter()
        .step_by(8)
        .map(|v| tile_of(v[0] as f64 * E7, v[1] as f64 * E7, z))
        .collect::<Vec<_>>()
        .into_iter()
        .collect();
    let mut out: HashSet<(i64, i64)> = HashSet::new();
    for (x, y) in base {
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                let (a, b) = (x + dx, y + dy);
                if a >= 0 && b >= 0 && a < n && b < n {
                    out.insert((a, b));
                }
            }
        }
    }
    out.into_iter().map(|(x, y)| [x as u32, y as u32]).collect()
}

fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "data/build".into()));
    if std::env::args().any(|a| a == "--scan") {
        return scan(&dir);
    }
    let t0 = std::time::Instant::now();
    let wv = Ways::open(&dir)?;
    let verts = wv.verts();

    // Tile set.
    let (mut w, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for v in verts.iter().step_by(101) {
        let (lon, lat) = (v[0] as f64 * E7, v[1] as f64 * E7);
        w = w.min(lon);
        e = e.max(lon);
        s = s.min(lat);
        n = n.max(lat);
    }
    let mut want: Vec<(u8, u32, u32)> = Vec::new();
    for z in 0..=8u8 {
        let (x0, y0) = tile_of(w - 1.0, n + 1.0, z);
        let (x1, y1) = tile_of(e + 1.0, s - 1.0, z);
        for y in y0.max(0)..=y1.min((1 << z) - 1) {
            for x in x0.max(0)..=x1.min((1 << z) - 1) {
                want.push((z, x as u32, y as u32));
            }
        }
    }
    let grid_tiles = near_roads(verts, 11, 1);
    for z in 9..=12u8 {
        let set = if z == 11 { grid_tiles.clone() } else { near_roads(verts, z, 1) };
        want.extend(set.into_iter().map(|t| (z, t[0], t[1])));
    }
    eprintln!("terrain: {} tiles wanted ({} for the z11 grid)", want.len(), grid_tiles.len());

    // Download (reusing a previous archive).
    let archive_path = dir.join("terrain.tiles");
    let old = Archive::open(&archive_path).ok();
    let aw = Mutex::new(ArchiveWriter::create(&roadcore::tmp(&dir, "terrain.tiles"), r#"{"format":"png","encoding":"terrarium"}"#)?);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent("road-elevations/0.1 (personal offline map)")
        .build()
        .into();
    let pb = count_bar(want.len() as u64, "terrain tiles");
    let missing = std::sync::atomic::AtomicUsize::new(0);
    let reused = std::sync::atomic::AtomicUsize::new(0);
    let repaired = std::sync::atomic::AtomicUsize::new(0);
    let repaired_keys: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    // Finest zoom first: a level's repairs are made again in the level above, and from z9 down every
    // tile's 2×2 means (quads) make the level above (process).
    let mut below: HashMap<(u32, u32), Repaired> = HashMap::new();
    let mut quads: HashMap<(u32, u32), Vec<f32>> = HashMap::new();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(48).build()?;
    for zl in (0..=12u8).rev() {
        let level: Vec<(u32, u32)> = want.iter().filter(|t| t.0 == zl).map(|t| (t.1, t.2)).collect();
        let now: Mutex<HashMap<(u32, u32), Repaired>> = Mutex::new(HashMap::new());
        let now_quads: Mutex<HashMap<(u32, u32), Vec<f32>>> = Mutex::new(HashMap::new());
        pool.install(|| {
            level.par_iter().for_each(|&(x, y)| {
                let blob = if let Some(b) = old.as_ref().and_then(|a| a.get(zl, x, y)) {
                    reused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Some(b.to_vec())
                } else {
                    fetch(&agent, zl, x, y)
                };
                match blob {
                    Some(b) => {
                        let (b, r, q) = process(b, zl, x, y, &below, &quads);
                        if let Some(q) = q {
                            now_quads.lock().unwrap().insert((x, y), q);
                        }
                        if let Some(r) = r {
                            repaired.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            repaired_keys.lock().unwrap().push(roadcore::archive::tile_key(zl, x, y));
                            now.lock().unwrap().insert((x, y), r);
                        }
                        aw.lock().unwrap().add(zl, x, y, &b, b.len()).unwrap()
                    }
                    None => {
                        missing.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                pb.inc(1);
            });
        });
        below = now.into_inner().unwrap();
        quads = now_quads.into_inner().unwrap();
    }
    pb.finish_and_clear();
    let size = aw.into_inner().unwrap().finish()?;
    drop(old);
    roadcore::commit(&dir, &["terrain.tiles"])?;
    // The tiles whose content changed under the same key (repaired), for the steps that recompute
    // only where terrain is new (slope, scenic canopy and view: TERRAIN_REPAIRED).
    let rk = repaired_keys.into_inner().unwrap();
    if !rk.is_empty() {
        let cdir = dir.parent().unwrap_or(Path::new(".")).join("cache/steps");
        std::fs::create_dir_all(&cdir)?;
        std::fs::write(cdir.join(roadcore::archive::TERRAIN_REPAIRED), bytemuck::cast_slice(&rk))?;
    }
    eprintln!(
        "terrain.tiles: {:.2} GB, {} reused, {} missing, {} repaired ({:.0?})",
        size as f64 / 1e9,
        reused.into_inner(),
        missing.into_inner(),
        repaired.into_inner(),
        t0.elapsed()
    );

    // z11 analysis grid.
    build_grid(&dir, grid_tiles)?;
    eprintln!("done ({:.0?})", t0.elapsed());
    Ok(())
}

/// `--scan`: what repair_terrain would change, per zoom, with the worst tiles (read only).
fn scan(dir: &Path) -> Result<()> {
    let arc = Archive::open(&dir.join("terrain.tiles"))?;
    let entries = arc.entries();
    let pb = count_bar(entries.len() as u64, "scan");
    // zoom, x, y, filled, clamped, largest change (m) and where (pixel)
    let found: Mutex<Vec<(u8, u32, u32, usize, usize, f32, usize)>> = Mutex::new(Vec::new());
    entries.par_iter().for_each(|e| {
        let (z, x, y) = ((e.key >> 58) as u8, ((e.key >> 29) & 0x1fff_ffff) as u32, (e.key & 0x1fff_ffff) as u32);
        if let Some(png) = arc.get(z, x, y) {
            if let Ok(v) = decode_terrain_png(png) {
                let mut r = v.clone();
                let (f, c) = repair_terrain(&mut r, z, tile_lat(z, y));
                if f + c > 0 {
                    let (mut big, mut at) = (0f32, 0usize);
                    for (i, (a, b)) in v.iter().zip(&r).enumerate() {
                        let d = if a.is_finite() { (a - b).abs() } else { f32::MAX };
                        if d > big {
                            big = d;
                            at = i;
                        }
                    }
                    found.lock().unwrap().push((z, x, y, f, c, big, at));
                }
            }
        }
        pb.inc(1);
    });
    pb.finish_and_clear();
    let mut f = found.into_inner().unwrap();
    f.sort_by(|a, b| b.5.total_cmp(&a.5));
    // Every change over 50 m, for checking what the rules take (z, lon, lat, before, after).
    if let Some(path) = std::env::args().skip_while(|a| a != "--dump").nth(1) {
        let mut out = String::from("z,lon,lat,before,after\n");
        for &(z, x, y, ..) in &f {
            let Some(png) = arc.get(z, x, y) else { continue };
            let Ok(v) = decode_terrain_png(png) else { continue };
            let mut r = v.clone();
            repair_terrain(&mut r, z, tile_lat(z, y));
            let n2 = (1u64 << z) as f64;
            for (i, (a, b)) in v.iter().zip(&r).enumerate() {
                if !((a - b).abs() <= 50.0) {
                    let (px, py) = ((i % 256) as f64 + 0.5, (i / 256) as f64 + 0.5);
                    let lon = (x as f64 + px / 256.0) / n2 * 360.0 - 180.0;
                    let lat = (std::f64::consts::PI * (1.0 - 2.0 * (y as f64 + py / 256.0) / n2)).sinh().atan().to_degrees();
                    out.push_str(&format!("{z},{lon:.5},{lat:.5},{a:.0},{b:.0}\n"));
                }
            }
        }
        std::fs::write(&path, out)?;
    }
    eprintln!("{} tiles, {} to repair", entries.len(), f.len());
    for z in 0..=12u8 {
        let at_z: Vec<_> = f.iter().filter(|t| t.0 == z).collect();
        if at_z.is_empty() {
            continue;
        }
        eprintln!("z{z}: {} tiles, {} pixels filled, {} clamped", at_z.len(), at_z.iter().map(|t| t.3).sum::<usize>(), at_z.iter().map(|t| t.4).sum::<usize>());
        for &&(z, x, y, fl, cl, big, at) in at_z.iter().take(6) {
            let n2 = (1u64 << z) as f64;
            let (px, py) = ((at % 256) as f64 + 0.5, (at / 256) as f64 + 0.5);
            let lon = (x as f64 + px / 256.0) / n2 * 360.0 - 180.0;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * (y as f64 + py / 256.0) / n2)).sinh().atan().to_degrees();
            eprintln!("  {z}/{x}/{y}: {fl} filled, {cl} clamped, largest change {big:.0} m at {lon:.5}, {lat:.5}");
        }
    }
    Ok(())
}

fn build_grid(dir: &Path, tiles: Vec<[u32; 2]>) -> Result<()> {
    let idx = GridIndex::new(tiles);
    let arc = Archive::open(&dir.join("terrain.tiles"))?;
    let mut data = vec![0i16; idx.tiles.len() * CELLS];
    let pb = count_bar(idx.tiles.len() as u64, "z11 terrain grid");
    data.par_chunks_mut(CELLS).zip(&idx.tiles).for_each(|(chunk, t)| {
        if let Some(png) = arc.get(11, t[0], t[1]) {
            if let Ok(e) = decode_terrain_png(png) {
                for (o, v) in chunk.iter_mut().zip(e) {
                    *o = v.round().clamp(-500.0, 9000.0) as i16;
                }
            }
        }
        pb.inc(1);
    });
    pb.finish_and_clear();
    idx.save(&roadcore::tmp(dir, "grid.idx"))?;
    std::fs::write(roadcore::tmp(dir, "grid.terrain.i16"), bytemuck::cast_slice(&data))?;
    roadcore::commit(dir, &["grid.idx", "grid.terrain.i16"])?;
    eprintln!("grid: {} z11 tiles, {:.2} GB", idx.tiles.len(), (data.len() * 2) as f64 / 1e9);
    Ok(())
}
