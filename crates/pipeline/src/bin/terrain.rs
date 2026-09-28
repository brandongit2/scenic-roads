//! Terrain tiles for 3D terrain / hillshade, and the z11 terrain analysis grid.
//!
//! usage: terrain <build_dir>
//!
//! Downloads Terrarium-encoded DEM tiles (AWS Terrain Tiles: USGS 3DEP in the US, NRCan
//! CDEM in Canada, ~27 m at z12) for z0–8 over the region and z9–12 within one tile of a
//! road. Below-sea-level values (ocean bathymetry) are clamped to 0 so the sea stays flat
//! in 3D. Tiles already in a previous archive are reused. Writes:
//!   terrain.tiles       tile archive of Terrarium PNGs (served for MapLibre raster-dem)
//!   grid.idx            z11 tiles within ~14 km of a road (shared by all analysis layers)
//!   grid.terrain.i16    their elevations in metres

use anyhow::Result;
use pipeline::count_bar;
use rayon::prelude::*;
use roadcore::archive::{Archive, ArchiveWriter};
use roadcore::grid::{decode_terrain_png, encode_terrain_png, GridIndex, CELLS};
use roadcore::{Ways, E7};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const URL: &str = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium";

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

fn fetch(agent: &ureq::Agent, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let url = format!("{URL}/{z}/{x}/{y}.png");
    for attempt in 0..5 {
        match agent.get(&url).call() {
            Ok(mut r) => {
                if let Ok(b) = r.body_mut().with_config().limit(20_000_000).read_to_vec() {
                    return Some(b);
                }
            }
            Err(ureq::Error::StatusCode(404 | 403)) => return None,
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(300 << attempt));
    }
    None
}

/// Clamp bathymetry to sea level; returns the original bytes when nothing changes.
fn process(png: Vec<u8>) -> Vec<u8> {
    let Ok(mut e) = decode_terrain_png(&png) else { return png };
    if e.iter().all(|&v| v >= 0.0) {
        return png;
    }
    for v in e.iter_mut() {
        *v = v.max(0.0);
    }
    encode_terrain_png(&e, 256, 256).unwrap_or(png)
}

fn main() -> Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "data/build".into()));
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
        .build()
        .into();
    let pb = count_bar(want.len() as u64, "terrain tiles");
    let missing = std::sync::atomic::AtomicUsize::new(0);
    let reused = std::sync::atomic::AtomicUsize::new(0);
    rayon::ThreadPoolBuilder::new().num_threads(48).build()?.install(|| {
        want.par_iter().for_each(|&(z, x, y)| {
            let blob = if let Some(b) = old.as_ref().and_then(|a| a.get(z, x, y)) {
                reused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some(b.to_vec())
            } else {
                fetch(&agent, z, x, y).map(process)
            };
            match blob {
                Some(b) => aw.lock().unwrap().add(z, x, y, &b, b.len()).unwrap(),
                None => {
                    missing.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            pb.inc(1);
        });
    });
    pb.finish_and_clear();
    let size = aw.into_inner().unwrap().finish()?;
    drop(old);
    roadcore::commit(&dir, &["terrain.tiles"])?;
    eprintln!(
        "terrain.tiles: {:.2} GB, {} reused, {} missing ({:.0?})",
        size as f64 / 1e9,
        reused.into_inner(),
        missing.into_inner(),
        t0.elapsed()
    );

    // z11 analysis grid.
    build_grid(&dir, grid_tiles)?;
    eprintln!("done ({:.0?})", t0.elapsed());
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
