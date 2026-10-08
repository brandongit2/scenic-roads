//! Terrain tiles for 3D terrain / hillshade, and the z11 terrain analysis grid.
//!
//! usage: terrain <build_dir>
//!
//! Downloads Terrarium-encoded DEM tiles (AWS Terrain Tiles: USGS 3DEP in the US, NRCan
//! CDEM in Canada, ~27 m at z12) for z0–8 over the region and z9–12 within one tile of a
//! road. Voids, towers and pits are repaired (roadcore::grid::repair_terrain: README "Terrain
//! repair"), then below-sea-level values (ocean bathymetry) clamped to 0 so the sea stays flat in
//! 3D, finest zoom first, and every pixel above a repaired one is made again from its four below
//! (the coarse tiles averaged the voids in: 18 km at z8 above Toyama, kilometres more up to z5).
//! From z8 down, every quarter of a tile whose child tile exists is made again from it (process,
//! REBUILD_Z): AWS's coarse levels come from coarser sources, which lost peaks as you zoomed out.
//! Tiles already in a previous archive are reused (and repaired the same way). Writes:
//!   terrain.tiles       tile archive of Terrarium PNGs (served for MapLibre raster-dem)
//!   grid.idx            z11 tiles within ~14 km of a road (shared by all analysis layers)
//!   grid.terrain.i16    their elevations in metres
//!
//! `terrain --scan`: the repair over the coverage's tiles, from AWS's raw tiles (see `scan`).

use det::Det;
use anyhow::{Context, Result};
use pipeline::count_bar;
use pipeline::terrain_pack::{fetch, max_zoom_at, near_coverage, process, process_with, tile_lat, Repaired};
use rayon::prelude::*;
use roadcore::archive::{Archive, ArchiveWriter};
use roadcore::grid::{decode_terrain_png, repair_terrain_with, steepest, weigh_top, Blob, GridIndex, Repair, BLOB_MAX, BLOB_RISE, CELLS};
use roadcore::{Ways, E7};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
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
        return scan();
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
                        let (b, r, q) = process(b, zl, x, y, &below, &quads, &Default::default());
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

/// `--scan [--root <NAS project folder>] [--cache <dir>] [--out <dir>] [--only <3/x/y,…>]
/// [--views <file>] [--world-z8]`: the repair over the coverage's tiles, every level, made from
/// AWS's raw tiles as the terrain job makes them (terrain_pack::build_q: finest first, the pixels
/// above a repaired one made again, z8 and coarser from their children), the new repair beside the
/// first one (1 October, `v1`), and each repaired tile repaired again (it must change nothing).
/// Reads the NAS (the coverage, the raw archives copied into `--cache` a z3 pack at a time) and
/// writes only under `--out`:
///   tiles.tsv    every tile something changed in: what each repair moved, and its second pass
///   changes.csv  every pixel either repair moved by more than 50 m (lon, lat, before, after)
///   blobs.tsv    the new repair's blobs, as it weighed them (its second pass's, rise negated)
///   peaks.tsv    OSM's summits with a height (the pass's `work/summits`): each one's pixel before
///                and after the new repair, and how close its rules came to taking it
///   holes.tsv    areas at or below 1 m inside raised ground in the raw tiles (Hans Island's kind)
///   lefts.tsv    towers left over low ground by either repair (`towers_left`)
///   views/       `z-x-y.{raw,v1,v2,fixed}.f32` for the tiles listed in `--views` (256 × 256,
///                metres; v2 the new repair alone, fixed the terrain made: GLO-30, the water)
///   summary.txt  the counts per zoom
/// The new repair's terrain is made as the terrain job makes it (terrain_pack::prepare and
/// finish: GLO-30 north of 60°N from the NAS's `sources/copernicus-dem/`, the latest pass's
/// basemap's water, AWS's z9 tiles for the walled patches; `--bare`: AWS's tiles alone), and its
/// second pass is over that terrain as stored. `--fixed-out <dir>` writes that terrain there too,
/// as the terrain job's packs (a project folder of its own: its manifest, `layers/terrain/…`).
/// `--world-z8` does the same for the worldwide z8 (terrain_z8: each tile alone).
fn scan() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let root = PathBuf::from(opt("--root").unwrap_or_else(|| "/Volumes/personal/projects/scenic-roads".into()));
    let cache = PathBuf::from(opt("--cache").unwrap_or_else(|| "data/scan-cache".into()));
    let outd = PathBuf::from(opt("--out").unwrap_or_else(|| "data/scan".into()));
    std::fs::create_dir_all(outd.join("views"))?;
    let views: HashSet<(u8, u32, u32)> = match opt("--views") {
        Some(f) => std::fs::read_to_string(f)?.lines().filter_map(|l| {
            let v: Vec<u32> = l.trim().split('/').filter_map(|s| s.parse().ok()).collect();
            (v.len() == 3).then(|| (v[0] as u8, v[1], v[2]))
        }).collect(),
        None => HashSet::new(),
    };
    let only: Option<HashSet<(u32, u32)>> = opt("--only").map(|s| s.split(',').filter_map(|q| {
        let v: Vec<u32> = q.split('/').filter_map(|s| s.parse().ok()).collect();
        (v.len() == 3 && v[0] == 3).then(|| (v[1], v[2]))
    }).collect());
    // (The sources, as the terrain job opens them, from the NAS read only: nothing fetched.)
    let opened = if args.iter().any(|a| a == "--bare") {
        None
    } else {
        let o = pipeline::out::Out::open(&root, &cache.join("scratch"))?;
        Some(pipeline::terrain_pack::SourceFiles::open(&o, false)?)
    };
    if let Some(o) = &opened {
        eprintln!("scan: sources {}", o.sources(None).pin());
    }
    let mut fixed = match opt("--fixed-out") {
        Some(d) => Some(pipeline::out::Out::open(Path::new(&d), &cache.join("fixed-scratch"))?),
        None => None,
    };
    let sc = Scan {
        opened,
        tiles: Mutex::new(std::io::BufWriter::new(std::fs::File::create(outd.join("tiles.tsv"))?)),
        changes: Mutex::new(std::io::BufWriter::new(std::fs::File::create(outd.join("changes.csv"))?)),
        holes: Mutex::new(std::io::BufWriter::new(std::fs::File::create(outd.join("holes.tsv"))?)),
        lefts: Mutex::new(std::io::BufWriter::new(std::fs::File::create(outd.join("lefts.tsv"))?)),
        blobs: Mutex::new(std::io::BufWriter::new(std::fs::File::create(outd.join("blobs.tsv"))?)),
        peaks: Mutex::new(std::io::BufWriter::new(std::fs::File::create(outd.join("peaks.tsv"))?)),
        summits: summits_by_z6(&root),
        views,
        outd: outd.clone(),
        sums: Mutex::new(BTreeMap::new()),
    };
    writeln!(sc.tiles.lock().unwrap(), "z\tx\ty\tlon\tlat\tv1_px\tv1_max\tv1b_px\tv1b_max\tv1c_px\tv1c_max\tvoids\tblobs\tv2_px\tv2_max\tv2b_any\tv2b_voids\tv2b_blobs\tv2b_px\tv2b_max\tdiff_px\tdiff_max\tstages\tunseen\tleft1\tleft1_max\tleft2\tleft2_max\tseam\tpatches\tfixed_px\tfixed_max")?;
    writeln!(sc.changes.lock().unwrap(), "which,z,x,y,px,py,lon,lat,before,after")?;
    writeln!(sc.peaks.lock().unwrap(), "z\tx\ty\tid\tele\traw\tv2\tratio\tarea\trise\tlevel\tsteep\tsteep_flat\tspike\tspike_wall\tspike_flat\tfixed")?;
    writeln!(sc.blobs.lock().unwrap(), "z\tx\ty\tpx\tpy\tlon\tlat\tpit\tpixels\trise\tlevel\treach\tedge\tpx_m\tislope\twall\tring_iqr\ttop\tkind\trough\tground\tstage")?;
    writeln!(sc.holes.lock().unwrap(), "z\tx\ty\tlon\tlat\tarea\tfloor_min\tfloor_med\tfloor_max\tones\tneg\trim_min\trim_med\trim_max\twall_med")?;
    writeln!(sc.lefts.lock().unwrap(), "which\tz\tx\ty\tpx\tpy\tlon\tlat\tv\tground\tupper")?;
    let store = root.join("sources/aws-terrarium");
    let t0 = std::time::Instant::now();
    if let Some(f) = opt("--tiles") {
        // (Tiles alone, each from its raw tile only, with their views: for a look.)
        let raw = pipeline::terrain_pack::RawTiles::with_store(&cache, &store);
        let list: Vec<(u8, u32, u32)> = std::fs::read_to_string(f)?.lines().filter_map(|l| {
            let v: Vec<u32> = l.trim().split('/').filter_map(|s| s.parse().ok()).collect();
            (v.len() == 3).then(|| (v[0] as u8, v[1], v[2]))
        }).collect();
        let sc = Scan { views: list.iter().copied().collect(), ..sc };
        list.par_iter().try_for_each(|&(z, x, y)| -> Result<()> {
            if let (Some(b), _) = raw.get(z, x, y)? {
                sc.tile(b, z, x, y, &raw, &Default::default());
            }
            Ok(())
        })?;
        sc.flush()?;
        drop(raw);
        std::fs::remove_dir_all(cache.join("packs")).ok();
        return Ok(());
    }
    if args.iter().any(|a| a == "--world-z8") {
        let raw = pipeline::terrain_pack::RawTiles::with_store(&cache, &store);
        let n = 1u32 << 8;
        let tiles: Vec<(u32, u32)> = (0..n).flat_map(|x| (0..n).map(move |y| (x, y))).collect();
        let pb = count_bar(tiles.len() as u64, "world z8");
        tiles.par_iter().try_for_each(|&(x, y)| -> Result<()> {
            if let (Some(b), _) = raw.get(8, x, y)? {
                sc.tile(b, 8, x, y, &raw, &Default::default());
            }
            pb.inc(1);
            Ok(())
        })?;
        pb.finish_and_clear();
        drop(raw);
        std::fs::remove_dir_all(cache.join("packs")).ok();
    } else {
        // The coverage, as the agent has it (the regions' recipes, the pass's outlines).
        let out = pipeline::out::Out::open(&root, &cache.join("scratch"))?;
        let date = pipeline::osmpass::latest_pass(&root).context("no OSM pass")?;
        let outlines = out.get(&format!("sources/osm/{date}/outlines")).map(|c| out.path(c));
        let outlines = outlines.as_deref().map(pipeline::outlines::Outlines::open).transpose()?;
        let (recipes, _) = pipeline::agent::recipes::load(&root.join("inputs/regions"));
        let cov = pipeline::coverage::Coverage::from_recipes(&recipes, outlines.as_ref(), &root.join("inputs/outlines"))?;
        let by_q = pipeline::agent::build::coverage_tiles(&cov);
        eprintln!("scan: {} regions, {} z3 packs, {} z6 tiles", recipes.len(), by_q.len(), by_q.values().map(Vec::len).sum::<usize>());
        let mut z3: HashMap<(u32, u32), (Vec<f32>, Vec<f32>)> = HashMap::new();
        for (qi, (&q, ts)) in by_q.iter().enumerate() {
            if only.as_ref().is_some_and(|o| !o.contains(&q)) {
                continue;
            }
            let tq = std::time::Instant::now();
            let mut n = 0usize;
            // (A reader of the raw tiles per z6 tile, and one for q's levels: each area's archives
            // are copied here when read, and deleted once it's done, so the cache holds one area.)
            let fresh = || pipeline::terrain_pack::RawTiles::with_store(&cache, &store);
            let done_with = |raw: pipeline::terrain_pack::RawTiles| {
                drop(raw);
                std::fs::remove_dir_all(cache.join("packs")).ok();
            };
            let mut nine = Levels::default();
            let mut lakes_q: HashMap<u64, f32> = HashMap::new();
            for &(tx, ty) in ts {
                let raw = fresh();
                let mut b = Levels::default();
                let mut lakes = HashMap::new();
                let mut hi: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
                for z in (9..=12u8).rev() {
                    let s = 1u32 << (z - 6);
                    let tiles: Vec<(u32, u32)> = (tx * s..(tx + 1) * s).flat_map(|x| (ty * s..(ty + 1) * s).map(move |y| (x, y))).filter(|&(x, y)| z <= max_zoom_at(tile_lat(z, y)) && near_coverage(&cov, z, x, y, 20.0)).collect();
                    n += tiles.len();
                    b = sc.level(&raw, z, &tiles, &b, &mut lakes)?;
                    hi.append(&mut b.4);
                }
                done_with(raw);
                for (k, v) in lakes {
                    lakes_q.entry(k).or_insert(v);
                }
                if let Some(out) = fixed.as_mut() {
                    hi.sort_by_key(|t| (t.0, t.1, t.2));
                    let mut it = hi.into_iter().map(|(z, x, y, b)| {
                        let n = b.len() as u32;
                        (z, x, y, b, n)
                    });
                    pipeline::layers::write_pack(out, "terrain", "terrarium-png", false, "hi", (6, tx, ty), &mut it)?;
                }
                nine.0.extend(b.0);
                nine.1.extend(b.1);
                nine.2.extend(b.2);
                nine.3.extend(b.3);
            }
            let raw = fresh();
            let mut b = nine;
            let mut lo: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
            for z in (3..=8u8).rev() {
                let s = 1u32 << (z - 3);
                let tiles: Vec<(u32, u32)> = (q.0 * s..(q.0 + 1) * s).flat_map(|x| (q.1 * s..(q.1 + 1) * s).map(move |y| (x, y))).collect();
                n += tiles.len();
                b = sc.level(&raw, z, &tiles, &b, &mut lakes_q)?;
                lo.append(&mut b.4);
            }
            done_with(raw);
            if let Some(out) = fixed.as_mut() {
                lo.sort_by_key(|t| (t.0, t.1, t.2));
                let mut it = lo.into_iter().map(|(z, x, y, b)| {
                    let n = b.len() as u32;
                    (z, x, y, b, n)
                });
                pipeline::layers::write_pack(out, "terrain", "terrarium-png", false, "lo", (3, q.0, q.1), &mut it)?;
                out.save()?;
            }
            // (q's z3 tile as each repair left it, for the root.)
            if let (Some(a), Some(c)) = (b.1.remove(&q), b.3.remove(&q)) {
                z3.insert(q, (a, c));
            }
            eprintln!("scan: 3/{}/{} ({}/{}): {} tiles in {:.0?} ({:.0?} so far)", q.0, q.1, qi + 1, by_q.len(), n, tq.elapsed(), t0.elapsed());
            sc.flush()?;
        }
        // The root (z0–2), from the z3 tiles made here, else AWS's.
        if only.is_none() {
            let raw = pipeline::terrain_pack::RawTiles::with_store(&cache, &store);
            let mut b = Levels::default();
            for x in 0..8u32 {
                for y in 0..8u32 {
                    let (q1, q2) = match z3.remove(&(x, y)) {
                        Some(qs) => qs,
                        None => {
                            let Some(png) = raw.get(3, x, y)?.0 else { continue };
                            let ((_, q1, _, q2), _) = sc.tile(png, 3, x, y, &raw, &Default::default());
                            match (q1, q2) {
                                (Some(a), Some(c)) => (a, c),
                                _ => continue,
                            }
                        }
                    };
                    b.1.insert((x, y), q1);
                    b.3.insert((x, y), q2);
                }
            }
            let mut root_tiles: Vec<(u8, u32, u32, Vec<u8>, u32)> = Vec::new();
            for z in (0..=2u8).rev() {
                let n = 1u32 << z;
                let mut next = Levels::default();
                for x in 0..n {
                    for y in 0..n {
                        let Some(png) = raw.get(z, x, y)?.0 else { continue };
                        let below = Levels(HashMap::new(), b.1.clone(), HashMap::new(), b.3.clone(), Vec::new());
                        let ((_, q1, _, q2), png2) = sc.tile(png, z, x, y, &raw, &below);
                        let l = png2.len() as u32;
                        root_tiles.push((z, x, y, png2, l));
                        if let (Some(a), Some(c)) = (q1, q2) {
                            next.1.insert((x, y), a);
                            next.3.insert((x, y), c);
                        }
                    }
                }
                b = next;
            }
            if let Some(out) = fixed.as_mut() {
                root_tiles.sort_by_key(|t| (t.0, t.1, t.2));
                let mut it = root_tiles.into_iter();
                pipeline::layers::write_pack(out, "terrain", "terrarium-png", false, "root", (0, 0, 0), &mut it)?;
                out.save()?;
            }
        }
    }
    sc.flush()?;
    // The counts per zoom.
    let sums = sc.sums.into_inner().unwrap();
    let mut txt = String::new();
    txt += "zoom: tiles | v1 changed, its 2nd pass, 3rd pass | v2 changed (voids, blobs), its 2nd pass (any change) | outputs differ\n";
    let mut all = Sum::default();
    for (z, s) in &sums {
        txt += &format!("z{z}: {} | {} ({} px, max {:.0} m), {} ({} px, max {:.0} m), {} ({} px, max {:.0} m) | {} ({} voids, {} blobs, {} px, max {:.0} m), {} ({} px, max {:.2} m) | {} (max {:.0} m)\n",
            s.tiles, s.v1.0, s.v1.1, s.v1.2, s.v1b.0, s.v1b.1, s.v1b.2, s.v1c.0, s.v1c.1, s.v1c.2, s.v2.0, s.voids, s.blobs, s.v2.1, s.v2.2, s.v2b.0, s.v2b.1, s.v2b.2, s.diff.0, s.diff.2);
        all.add(s);
    }
    let s = &all;
    txt += &format!("all: {} | {} ({} px, max {:.0} m), {} ({} px, max {:.0} m), {} ({} px, max {:.0} m) | {} ({} voids, {} blobs, {} px, max {:.0} m), {} ({} px, max {:.2} m) | {} (max {:.0} m)\n",
        s.tiles, s.v1.0, s.v1.1, s.v1.2, s.v1b.0, s.v1b.1, s.v1b.2, s.v1c.0, s.v1c.1, s.v1c.2, s.v2.0, s.voids, s.blobs, s.v2.1, s.v2.2, s.v2b.0, s.v2b.1, s.v2b.2, s.diff.0, s.diff.2);
    txt += &format!("tiles by the new repair's stages (0: nothing to weigh, 1..8): {:?}\n", s.stages);
    txt += "seam spikes' and walled patches' pixels changed, and changed by either on the second pass (over the terrain made), per zoom:\n";
    for (z, s) in &sums {
        txt += &format!("  z{z}: {} {} | {}\n", s.seam, s.patches, s.seam_b);
    }
    txt += "lone towers left over low ground (> 100 m above a 7 x 7 median of 30 m or less, 3 or fewer of the 49 in their upper half), per zoom: tiles, towers, most above the median (first repair | new):\n";
    for (z, s) in &sums {
        txt += &format!("  z{z}: {} {} {:.0} | {} {} {:.0}\n", s.left1.0, s.left1.1, s.left1.2, s.left2.0, s.left2.1, s.left2.2);
    }
    txt += &format!("({:.0?})\n", t0.elapsed());
    eprint!("{txt}");
    std::fs::write(outd.join("summary.txt"), txt)?;
    Ok(())
}

/// A level's repairs and quarters, each repair's (terrain_pack::process): v1's, then v2's.
#[derive(Default)]
struct Levels(HashMap<(u32, u32), Repaired>, HashMap<(u32, u32), Vec<f32>>, HashMap<(u32, u32), Repaired>, HashMap<(u32, u32), Vec<f32>>, Vec<(u8, u32, u32, Vec<u8>)>);
type Made = (Option<Repaired>, Option<Vec<f32>>, Option<Repaired>, Option<Vec<f32>>);

/// Tiles changed, pixels moved (> 0.5 m), the largest move (m).
#[derive(Clone, Copy, Default)]
struct Count(u64, u64, f32);

impl Count {
    fn add(&mut self, px: u32, max: f32) {
        if px > 0 {
            self.0 += 1;
            self.1 += px as u64;
            self.2 = self.2.max(max);
        }
    }
    fn sum(&mut self, o: &Count) {
        self.0 += o.0;
        self.1 += o.1;
        self.2 = self.2.max(o.2);
    }
}

#[derive(Clone, Copy, Default)]
struct Sum {
    tiles: u64,
    v1: Count,
    v1b: Count,
    v1c: Count,
    v2: Count,
    voids: u64,
    blobs: u64,
    v2b: Count,
    diff: Count,
    /// Tiles by the stages the new repair took (index: stages, 1 to 8).
    stages: [u64; 9],
    /// Towers left over low ground (`towers_left`), after each repair.
    left1: Count,
    left2: Count,
    /// Pixels the seam spikes' and walled patches' rules changed, and those either changed on the
    /// second pass (over the terrain made).
    seam: u64,
    patches: u64,
    seam_b: u64,
}

impl Sum {
    fn add(&mut self, o: &Sum) {
        self.tiles += o.tiles;
        self.v1.sum(&o.v1);
        self.v1b.sum(&o.v1b);
        self.v1c.sum(&o.v1c);
        self.v2.sum(&o.v2);
        self.voids += o.voids;
        self.blobs += o.blobs;
        self.v2b.sum(&o.v2b);
        self.diff.sum(&o.diff);
        for (a, b) in self.stages.iter_mut().zip(&o.stages) {
            *a += b;
        }
        self.left1.sum(&o.left1);
        self.left2.sum(&o.left2);
        self.seam += o.seam;
        self.patches += o.patches;
        self.seam_b += o.seam_b;
    }
}

struct Scan {
    opened: Option<pipeline::terrain_pack::SourceFiles>,
    tiles: Mutex<std::io::BufWriter<std::fs::File>>,
    changes: Mutex<std::io::BufWriter<std::fs::File>>,
    holes: Mutex<std::io::BufWriter<std::fs::File>>,
    lefts: Mutex<std::io::BufWriter<std::fs::File>>,
    blobs: Mutex<std::io::BufWriter<std::fs::File>>,
    peaks: Mutex<std::io::BufWriter<std::fs::File>>,
    /// OSM's summits with a height, by z6 tile: (lon, lat, ele, id).
    summits: HashMap<(u32, u32), Vec<(f64, f64, f32, String)>>,
    views: HashSet<(u8, u32, u32)>,
    outd: PathBuf,
    sums: Mutex<BTreeMap<u8, Sum>>,
}

/// Pixels that differ by more than 0.5 m as the map shows them (at or above sea level), and the
/// largest difference.
fn moved(a: &[f32], b: &[f32]) -> (u32, f32) {
    let (mut n, mut big) = (0u32, 0f32);
    for (x, y) in a.iter().zip(b) {
        let d = if x.is_finite() && y.is_finite() { (x.max(0.0) - y.max(0.0)).abs() } else { f32::MAX };
        if d > 0.5 {
            n += 1;
            big = big.max(d);
        }
    }
    (n, big)
}

/// A tile's first half (`Scan::tile_a`): the first repair whole, the new one but for its water.
struct Half {
    v1: (Vec<u8>, Option<Repaired>, Option<Vec<f32>>),
    seen1: Option<(Vec<f32>, Vec<f32>)>,
    v2: pipeline::terrain_pack::Prepared,
    seen2: Option<(Vec<f32>, Vec<f32>, Repair)>,
    found: Vec<Blob>,
}

impl Scan {
    /// The sources the new repair's terrain is made with (crate::terrain_pack::Sources), AWS's z9
    /// tiles from `coarse`.
    fn sources<'a>(&'a self, coarse: &'a pipeline::terrain_pack::Coarse<'a>) -> pipeline::terrain_pack::Sources<'a> {
        match &self.opened {
            Some(o) => o.sources(Some(coarse)),
            None => pipeline::terrain_pack::Sources { coarse: Some(coarse), ..Default::default() },
        }
    }

    /// A level's tiles, each made from its raw tile with what the level below made (`b`), the
    /// lakes' levels from all of them (those known, `lakes`, kept).
    fn level(&self, raw: &pipeline::terrain_pack::RawTiles, z: u8, tiles: &[(u32, u32)], b: &Levels, lakes: &mut HashMap<u64, f32>) -> Result<Levels> {
        raw.prefetch(z, tiles, 64)?;
        let coarse = pipeline::terrain_pack::Coarse::new(raw);
        let src = self.sources(&coarse);
        let halves: Vec<Result<Option<((u32, u32), Half)>>> = tiles
            .par_iter()
            .map(|&(x, y)| {
                let Some(png) = raw.get(z, x, y)?.0 else { return Ok(None) };
                Ok(Some(((x, y), self.tile_a(png, z, x, y, b, &src))))
            })
            .collect();
        let mut halves: Vec<((u32, u32), Half)> = halves.into_iter().filter_map(|r| r.transpose()).collect::<Result<_>>()?;
        let mut all = HashMap::new();
        for (_, h) in &halves {
            pipeline::terrain_water::gather(&mut all, h.v2.lake_samples());
        }
        pipeline::terrain_water::add_levels(lakes, &all);
        let lakes: &HashMap<u64, f32> = lakes;
        let made: Vec<((u32, u32), Made, Vec<u8>)> = halves.par_drain(..).map(|((x, y), h)| {
            let (m, png2) = self.tile_b(h, z, x, y, lakes, &coarse);
            ((x, y), m, png2)
        }).collect();
        let mut l = Levels::default();
        for ((x, y), (r1, q1, r2, q2), png2) in made {
            if let Some(r) = r1 {
                l.0.insert((x, y), r);
            }
            if let Some(q) = q1 {
                l.1.insert((x, y), q);
            }
            if let Some(r) = r2 {
                l.2.insert((x, y), r);
            }
            if let Some(q) = q2 {
                l.3.insert((x, y), q);
            }
            l.4.push((z, x, y, png2));
        }
        Ok(l)
    }

    /// One tile alone (its lakes' levels from it alone).
    fn tile(&self, png: Vec<u8>, z: u8, x: u32, y: u32, raw: &pipeline::terrain_pack::RawTiles, b: &Levels) -> (Made, Vec<u8>) {
        let coarse = pipeline::terrain_pack::Coarse::new(raw);
        let src = self.sources(&coarse);
        let h = self.tile_a(png, z, x, y, b, &src);
        let mut lakes = HashMap::new();
        pipeline::terrain_water::add_levels(&mut lakes, &h.v2.lake_samples());
        self.tile_b(h, z, x, y, &lakes, &coarse)
    }

    /// One raw tile through both repairs (`b.0`/`b.1`: the first's level below, `b.2`/`b.3`: the
    /// new one's), the new one's terrain but for its water.
    fn tile_a(&self, png: Vec<u8>, z: u8, x: u32, y: u32, b: &Levels, src: &pipeline::terrain_pack::Sources) -> Half {
        if let Ok(e) = decode_terrain_png(&png) {
            self.holes(&e, z, x, y);
        }
        let seen1: std::cell::RefCell<Option<(Vec<f32>, Vec<f32>)>> = Default::default();
        let v1 = process_with(png.clone(), z, x, y, &b.0, &b.1, &Default::default(), &HashMap::new(), &|e, z, lat, _| {
            // (The first repair took bathymetry to sea level before it.)
            for v in e.iter_mut() {
                if *v < 0.0 {
                    *v = 0.0;
                }
            }
            let b = e.to_vec();
            v1::repair(e, z, lat);
            *seen1.borrow_mut() = Some((b, e.to_vec()));
        });
        let seen2: std::cell::RefCell<Option<(Vec<f32>, Vec<f32>, Repair)>> = Default::default();
        let found: std::cell::RefCell<Vec<Blob>> = Default::default();
        let v2 = pipeline::terrain_pack::prepare(png.clone(), z, x, y, &b.2, &b.3, src, &|e, z, lat, c| {
            let b = e.to_vec();
            let (r, bl) = repair_terrain_with(e, z, lat, c);
            *seen2.borrow_mut() = Some((b, e.to_vec(), r));
            *found.borrow_mut() = bl;
        });
        Half { v1, seen1: seen1.into_inner(), v2, seen2: seen2.into_inner(), found: found.into_inner() }
    }

    /// A tile's second half: the new terrain's water, and every count; its output (the new
    /// terrain's PNG) too.
    fn tile_b(&self, h: Half, z: u8, x: u32, y: u32, lakes: &HashMap<u64, f32>, coarse: &pipeline::terrain_pack::Coarse) -> (Made, Vec<u8>) {
        let lat = tile_lat(z, y);
        let Half { v1: (png1, r1, qd1), seen1, v2, seen2, found } = h;
        let (png2, r2, qd2) = pipeline::terrain_pack::finish(v2, lakes);
        let found = std::cell::RefCell::new(found);
        let mut sum = Sum { tiles: 1, ..Default::default() };
        let (Some((b1v, a1v)), Some((b2v, a2v, rep2))) = (seen1, seen2) else {
            self.sums.lock().unwrap().entry(z).or_default().add(&sum);
            return ((r1, qd1, r2, qd2), png2);
        };
        let m1 = moved(&b1v, &a1v);
        let m2 = moved(&b2v, &a2v);
        // The first repair's second and third passes, on its output as stored.
        let o1 = decode_terrain_png(&png1).unwrap_or_default();
        let mut p2 = o1.clone();
        v1::repair(&mut p2, z, lat);
        let m1b = moved(&o1, &p2);
        let mut p3 = p2.clone();
        v1::repair(&mut p3, z, lat);
        let m1c = moved(&p2, &p3);
        // The new one's second pass, on its output as stored (the terrain made: GLO-30 and the water
        // too): any change at all counts.
        let o2 = decode_terrain_png(&png2).unwrap_or_default();
        let mut again = o2.clone();
        let cz = coarse.over(z, x, y);
        let (rb, bb) = repair_terrain_with(&mut again, z, lat, cz.as_deref());
        if !bb.is_empty() {
            // (Its second pass's blobs too, marked: rows with pixels negated.)
            let bb: Vec<Blob> = bb.into_iter().map(|b| Blob { pixels: b.pixels, rise: -b.rise, ..b }).collect();
            self.blob_rows(&o2, &bb, z, x, y);
        }
        let any = again.iter().zip(&o2).filter(|(a, b)| a.to_bits() != b.to_bits()).count() as u32;
        let m2b = moved(&o2, &again);
        let diff = moved(&o1, &o2);
        let n2 = (1u64 << z) as f64;
        let lonlat = |i: usize| {
            let (px, py) = ((i % 256) as f64 + 0.5, (i / 256) as f64 + 0.5);
            ((x as f64 + px / 256.0) / n2 * 360.0 - 180.0, (std::f64::consts::PI * (1.0 - 2.0 * (y as f64 + py / 256.0) / n2)).dsinh().datan().to_degrees())
        };
        // (Lone towers left: three or fewer of the 49 pixels around in their upper half.)
        let (l1, l2) = (towers_left(&o1), towers_left(&o2));
        let lone = |l: &[(u32, f32, f32, u32)]| l.iter().filter(|t| t.3 <= 3).fold((0u32, 0f32), |(n, big), t| (n + 1, big.max(t.1 - t.2)));
        let (left1, left2) = (lone(&l1), lone(&l2));
        if !l1.is_empty() || !l2.is_empty() {
            let mut rows = String::new();
            for (which, l) in [("v1", &l1), ("v2", &l2)] {
                for &(i, v, m, upper) in l.iter() {
                    let (lon, lat) = lonlat(i as usize);
                    rows += &format!("{which}\t{z}\t{x}\t{y}\t{}\t{}\t{lon:.5}\t{lat:.5}\t{v:.0}\t{m:.0}\t{upper}\n", i % 256, i / 256);
                }
            }
            self.lefts.lock().unwrap().write_all(rows.as_bytes()).ok();
        }
        sum.v1.add(m1.0, m1.1);
        sum.v1b.add(m1b.0, m1b.1);
        sum.v1c.add(m1c.0, m1c.1);
        sum.v2.add(m2.0, m2.1);
        sum.voids += rep2.voids as u64;
        sum.blobs += rep2.blobs as u64;
        sum.seam += rep2.seam as u64;
        sum.patches += rep2.patches as u64;
        sum.seam_b += (rb.seam + rb.patches) as u64;
        sum.v2b.add(any.max(m2b.0), m2b.1);
        sum.diff.add(diff.0, diff.1);
        sum.stages[rep2.stages.min(8)] += 1;
        sum.left1.add(left1.0, left1.1);
        sum.left2.add(left2.0, left2.1);
        self.sums.lock().unwrap().entry(z).or_default().add(&sum);
        let fx = moved(&b2v, &o2);
        if m1.0 + m1b.0 + m1c.0 + m2.0 + any + diff.0 + left1.0 + left2.0 + fx.0 > 0 || rep2.voids > 0 {
            let (lon, lat) = lonlat(128 * 256 + 128);
            writeln!(self.tiles.lock().unwrap(), "{z}\t{x}\t{y}\t{lon:.5}\t{lat:.5}\t{}\t{:.0}\t{}\t{:.0}\t{}\t{:.0}\t{}\t{}\t{}\t{:.0}\t{any}\t{}\t{}\t{}\t{:.2}\t{}\t{:.0}\t{}\t{}\t{}\t{:.0}\t{}\t{:.0}\t{}\t{}\t{}\t{:.0}", m1.0, m1.1, m1b.0, m1b.1, m1c.0, m1c.1, rep2.voids, rep2.blobs, m2.0, m2.1, rb.voids, rb.blobs, m2b.0, m2b.1, diff.0, diff.1, rep2.stages, rep2.unseen, left1.0, left1.1, left2.0, left2.1, rep2.seam, rep2.patches, fx.0, fx.1).ok();
        }
        self.blob_rows(&b2v, &found.into_inner(), z, x, y);
        self.peak_rows(&b2v, &a2v, &o2, z, x, y);
        let mut ch = String::new();
        for (which, b, a) in [("v1", &b1v, &a1v), ("v2", &b2v, &a2v)] {
            for (i, (bv, av)) in b.iter().zip(a.iter()).enumerate() {
                let (bv, av) = (bv.max(0.0), av.max(0.0));
                if !((bv - av).abs() <= 50.0) {
                    let (lon, lat) = lonlat(i);
                    ch += &format!("{which},{z},{x},{y},{},{},{lon:.5},{lat:.5},{bv:.0},{av:.0}\n", i % 256, i / 256);
                }
            }
        }
        if !ch.is_empty() {
            self.changes.lock().unwrap().write_all(ch.as_bytes()).ok();
        }
        if self.views.contains(&(z, x, y)) {
            for (name, v) in [("raw", &b2v), ("v1", &o1), ("v2", &a2v.iter().map(|v| v.max(0.0)).collect()), ("fixed", &o2)] {
                std::fs::write(self.outd.join(format!("views/{z}-{x}-{y}.{name}.f32")), bytemuck::cast_slice(v)).ok();
            }
        }
        ((r1, qd1, r2, qd2), png2)
    }

    fn flush(&self) -> Result<()> {
        self.tiles.lock().unwrap().flush()?;
        self.changes.lock().unwrap().flush()?;
        self.holes.lock().unwrap().flush()?;
        self.lefts.lock().unwrap().flush()?;
        self.blobs.lock().unwrap().flush()?;
        self.peaks.lock().unwrap().flush()?;
        Ok(())
    }

    /// The new repair's blobs, each with how it looks in the tile it came from (`e`, before it):
    /// its steepest step inside (islope, m per m), the median step down its edge (wall, m per m)
    /// and the spread of the ground around it (ring_iqr, m).
    fn blob_rows(&self, e: &[f32], blobs: &[Blob], z: u8, x: u32, y: u32) {
        if blobs.is_empty() {
            return;
        }
        let w = 256i32;
        let px = 40_075_016.7 * tile_lat(z, y).to_radians().dcos() / ((1u64 << z) as f64 * 256.0);
        let n2 = (1u64 << z) as f64;
        let mut rows = String::new();
        for b in blobs {
            let sg = if b.pit { -1.0f32 } else { 1.0 };
            let v = |i: usize| sg * e[i];
            let lvl = sg * b.level;
            // (Its pixels: above its level, connected to its top.)
            let mut inb = vec![false; e.len()];
            let mut stack = vec![b.top as usize];
            inb[b.top as usize] = true;
            let mut px_list = Vec::new();
            while let Some(p) = stack.pop() {
                px_list.push(p);
                let (xx, yy) = ((p % 256) as i32, (p / 256) as i32);
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (a, c) = (xx + dx, yy + dy);
                    if a < 0 || c < 0 || a >= w || c >= w {
                        continue;
                    }
                    let q = (c * w + a) as usize;
                    if !inb[q] && v(q) > lvl && v(q).is_finite() {
                        inb[q] = true;
                        stack.push(q);
                    }
                }
            }
            let (mut islope, mut walls, mut ring) = (0f64, Vec::new(), Vec::new());
            let mut onring = HashSet::new();
            for &p in &px_list {
                let (xx, yy) = ((p % 256) as i32, (p / 256) as i32);
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (a, c) = (xx + dx, yy + dy);
                    if a < 0 || c < 0 || a >= w || c >= w {
                        continue;
                    }
                    let q = (c * w + a) as usize;
                    let dist = if dx != 0 && dy != 0 { std::f64::consts::SQRT_2 } else { 1.0 };
                    let step = (v(p) - v(q)) as f64 / (dist * px);
                    if inb[q] {
                        islope = islope.max(step.abs());
                    } else {
                        walls.push(step);
                        if onring.insert(q) {
                            ring.push(e[q]);
                        }
                    }
                }
            }
            walls.sort_by(|a, b| a.total_cmp(b));
            ring.sort_by(|a, b| a.total_cmp(b));
            let wall = walls.get(walls.len() / 2).copied().unwrap_or(0.0);
            let iqr = if ring.is_empty() { 0.0 } else { ring[ring.len() * 3 / 4] - ring[ring.len() / 4] };
            let (tx, ty) = ((b.top % 256) as f64, (b.top / 256) as f64);
            let lon = (x as f64 + (tx + 0.5) / 256.0) / n2 * 360.0 - 180.0;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * (y as f64 + (ty + 0.5) / 256.0) / n2)).dsinh().datan().to_degrees();
            rows += &format!("{z}\t{x}\t{y}\t{}\t{}\t{lon:.5}\t{lat:.5}\t{}\t{}\t{:.0}\t{:.0}\t{:.0}\t{}\t{px:.1}\t{islope:.2}\t{wall:.2}\t{iqr:.1}\t{:.0}\t{:?}\t{:.1}\t{:.0}\t{}\n", b.top % 256, b.top / 256, b.pit as u8, b.pixels, b.rise, b.level, b.reach, b.edge as u8, e[b.top as usize], b.kind, b.rough, b.ground, b.stage);
        }
        self.blobs.lock().unwrap().write_all(rows.as_bytes()).ok();
    }

    /// OSM's summits in the tile (z6 and finer), each with how close the repair came to taking it
    /// (`ratio`: its most, over the levels below it, of its rise over what the repair allows; 1 is
    /// broken), and the height the new repair left it (`e`: before the repair, `after`: after).
    fn peak_rows(&self, e: &[f32], after: &[f32], fixed: &[f32], z: u8, x: u32, y: u32) {
        if z < 6 {
            return;
        }
        let Some(list) = self.summits.get(&(x >> (z - 6), y >> (z - 6))) else { return };
        let n2 = (1u64 << z) as f64;
        let px = 40_075_016.7 * tile_lat(z, y).to_radians().dcos() / (n2 * 256.0);
        let mut rows = String::new();
        for (lon, lat, ele, id) in list {
            let fx = (lon + 180.0) / 360.0 * n2 - x as f64;
            let fy = (1.0 - lat.to_radians().dtan().dasinh() / std::f64::consts::PI) / 2.0 * n2 - y as f64;
            if !(0.0..1.0).contains(&fx) || !(0.0..1.0).contains(&fy) {
                continue;
            }
            // (Its pixel: the highest within one of where OSM has it.)
            let (cx, cy) = ((fx * 256.0) as i64, (fy * 256.0) as i64);
            let mut best = (f32::MIN, 0usize);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (a, c) = (cx + dx, cy + dy);
                    if (0..256).contains(&a) && (0..256).contains(&c) && e[(c * 256 + a) as usize] > best.0 {
                        best = (e[(c * 256 + a) as usize], (c * 256 + a) as usize);
                    }
                }
            }
            let (ratio, area, rise, level) = summit_ratio(e, best.1, px);
            let wt = weigh_top(e, best.1 as u32, z, tile_lat(z, y));
            rows += &format!("{z}\t{x}\t{y}\t{id}\t{ele:.0}\t{:.0}\t{:.0}\t{ratio:.3}\t{area}\t{rise:.0}\t{level:.0}\t{:.3}\t{:.1}\t{}\t{:.2}\t{:.1}\t{:.0}\n", best.0, after[best.1], wt.steep, wt.steep_flat, wt.spike as u8, wt.spike_wall, wt.spike_flat, fixed.get(best.1).copied().unwrap_or(f32::NAN));
        }
        if !rows.is_empty() {
            self.peaks.lock().unwrap().write_all(rows.as_bytes()).ok();
        }
    }

    /// The survey of Hans Island's kind: areas of the raw tile at or below 1 m, inside it (not at
    /// its edge), 16 pixels or more, ringed by ground 10 m up or more (the ring's median).
    fn holes(&self, e: &[f32], z: u8, x: u32, y: u32) {
        let w = 256i32;
        let mut lab = vec![u32::MAX; e.len()];
        let mut rows = String::new();
        let n2 = (1u64 << z) as f64;
        for s0 in 0..e.len() {
            if lab[s0] != u32::MAX || !(e[s0] <= 1.0) {
                continue;
            }
            lab[s0] = s0 as u32;
            let (mut stack, mut px) = (vec![s0], Vec::new());
            let mut edge = false;
            while let Some(p) = stack.pop() {
                px.push(p);
                let (px_, py_) = ((p % 256) as i32, (p / 256) as i32);
                if px_ == 0 || py_ == 0 || px_ == w - 1 || py_ == w - 1 {
                    edge = true;
                }
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (xx, yy) = (px_ + dx, py_ + dy);
                    if xx < 0 || yy < 0 || xx >= w || yy >= w {
                        continue;
                    }
                    let q = (yy * w + xx) as usize;
                    if lab[q] == u32::MAX && e[q] <= 1.0 {
                        lab[q] = s0 as u32;
                        stack.push(q);
                    }
                }
            }
            if edge || px.len() < 16 {
                continue;
            }
            let (mut rim, mut wall) = (Vec::new(), Vec::new());
            let mut onrim = std::collections::HashSet::new();
            for &p in &px {
                let (px_, py_) = ((p % 256) as i32, (p / 256) as i32);
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let q = ((py_ + dy) * w + px_ + dx) as usize;
                    if lab[q] != s0 as u32 && onrim.insert(q) {
                        rim.push(e[q]);
                        wall.push(e[q] - e[p]);
                    }
                }
            }
            let med = |v: &mut Vec<f32>| {
                v.sort_by(|a, b| a.total_cmp(b));
                (v[0], v[v.len() / 2], v[v.len() - 1])
            };
            let (rmin, rmed, rmax) = med(&mut rim);
            if !(rmed >= 10.0) {
                continue;
            }
            let (_, wmed, _) = med(&mut wall);
            let mut fl: Vec<f32> = px.iter().map(|&p| e[p]).collect();
            let ones = fl.iter().filter(|&&v| v == 1.0).count() as f64 / fl.len() as f64;
            let neg = fl.iter().filter(|&&v| v < 0.0).count() as f64 / fl.len() as f64;
            let (fmin, fmed, fmax) = med(&mut fl);
            let (cx, cy) = (px.iter().map(|&p| (p % 256) as f64).sum::<f64>() / px.len() as f64, px.iter().map(|&p| (p / 256) as f64).sum::<f64>() / px.len() as f64);
            let lon = (x as f64 + (cx + 0.5) / 256.0) / n2 * 360.0 - 180.0;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * (y as f64 + (cy + 0.5) / 256.0) / n2)).dsinh().datan().to_degrees();
            rows += &format!("{z}\t{x}\t{y}\t{lon:.5}\t{lat:.5}\t{}\t{fmin:.1}\t{fmed:.1}\t{fmax:.1}\t{ones:.2}\t{neg:.2}\t{rmin:.1}\t{rmed:.1}\t{rmax:.1}\t{wmed:.1}\n", px.len());
        }
        if !rows.is_empty() {
            self.holes.lock().unwrap().write_all(rows.as_bytes()).ok();
        }
    }
}

/// Towers left over low ground in a tile as the map shows it, a check apart from the repair's own
/// rules: pixels higher than their eight neighbours and more than 100 m above the median of the
/// 7 × 7 pixels around them where that median is 30 m or less (water, lowland), each with how many
/// of those 49 pixels are in its upper half (a lone tower has a few; a coastal hill, many).
fn towers_left(e: &[f32]) -> Vec<(u32, f32, f32, u32)> {
    let mut out = Vec::new();
    let mut win = Vec::with_capacity(49);
    let at = |x: i32, y: i32| e[(y * 256 + x) as usize].max(0.0);
    for y in 0..256i32 {
        for x in 0..256i32 {
            let v = at(x, y);
            if v < 100.0 {
                continue;
            }
            let mut top = true;
            win.clear();
            for dy in -3..=3 {
                for dx in -3..=3 {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx >= 0 && yy >= 0 && xx < 256 && yy < 256 {
                        let w = at(xx, yy);
                        win.push(w);
                        top &= (dx, dy) == (0, 0) || dx.abs() > 1 || dy.abs() > 1 || w < v;
                    }
                }
            }
            if !top {
                continue;
            }
            let k = win.len() / 2;
            let m = *win.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1;
            if m <= 30.0 && v - m > 100.0 {
                let half = m + (v - m) / 2.0;
                let upper = win.iter().filter(|&&w| w >= half).count() as u32;
                out.push(((y * 256 + x) as u32, v, m, upper));
            }
        }
    }
    out
}

/// OSM's summits with a height (the pass's `work/summits`), by z6 tile; none when there are none.
fn summits_by_z6(root: &Path) -> HashMap<(u32, u32), Vec<(f64, f64, f32, String)>> {
    let mut by: HashMap<(u32, u32), Vec<(f64, f64, f32, String)>> = HashMap::new();
    let Ok(out) = pipeline::out::Out::open(root, &std::env::temp_dir().join("scan-summits")) else { return by };
    let Some(date) = pipeline::osmpass::latest_pass(root) else { return by };
    let Some(c) = out.get(&format!("work/summits/{date}")) else { return by };
    let Ok(all) = pipeline::summits::read(&out.path(c)) else { return by };
    for s in all {
        let Some(ele) = s.ele else { continue };
        let (lon, lat) = (s.lon as f64 * 1e-7, s.lat as f64 * 1e-7);
        let (tx, ty) = tile_of(lon, lat, 6);
        if (0..64).contains(&tx) && (0..64).contains(&ty) {
            by.entry((tx as u32, ty as u32)).or_default().push((lon, lat, ele, s.id));
        }
    }
    eprintln!("scan: {} summits with a height", by.values().map(Vec::len).sum::<usize>());
    by
}

/// How close the new repair comes to taking the top at `p` (its pixels joined highest first, as
/// the repair's tree does, until one higher than it: there its chain ends): the most, over the
/// levels below it, of its rise as the map shows it over what the repair allows (steepest(l) × l,
/// from the area as the repair counts it), with the area, rise and level there. Only rises above
/// BLOB_RISE count.
fn summit_ratio(e: &[f32], p: usize, px: f64) -> (f64, u32, f32, f32) {
    #[derive(PartialEq)]
    struct H(f32, usize);
    impl Eq for H {}
    impl PartialOrd for H {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for H {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&o.0).then(o.1.cmp(&self.1))
        }
    }
    let top = e[p];
    let mut seen = vec![false; e.len()];
    let mut heap = std::collections::BinaryHeap::new();
    heap.push(H(top, p));
    seen[p] = true;
    let (mut area, mut sides) = (0u32, 0u8);
    let mut best = (0f64, 0u32, 0f32, 0f32);
    while let Some(H(v, q)) = heap.pop() {
        if v > top || !v.is_finite() {
            break;
        }
        if area > 0 {
            // (As the map shows it, as the repair weighs it.)
            let rise = (top.max(0.0) - v.max(0.0)) as f64;
            if rise > BLOB_RISE {
                let a = area as f64 * (1u32 << (sides.count_ones())) as f64;
                let l = ((a / std::f64::consts::PI).sqrt() + 0.5) * px;
                let r = rise / (steepest(l) * l);
                if r > best.0 {
                    best = (r, area, rise as f32, v);
                }
            }
        }
        if area >= BLOB_MAX {
            break;
        }
        area += 1;
        let (x, y) = ((q % 256) as i64, (q / 256) as i64);
        sides |= (x == 0) as u8 | ((x == 255) as u8) << 1 | ((y == 0) as u8) << 2 | ((y == 255) as u8) << 3;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (a, c) = (x + dx, y + dy);
                if (0..256).contains(&a) && (0..256).contains(&c) && !seen[(c * 256 + a) as usize] {
                    seen[(c * 256 + a) as usize] = true;
                    heap.push(H(e[(c * 256 + a) as usize], (c * 256 + a) as usize));
                }
            }
        }
    }
    best
}

mod v1 {
    //! The first repair (1 October), as it was, for the scan to compare the new one with: voids
    //! filled ring by ring, towers judged against a ring two to three pixels out, lone spikes and
    //! pits clamped to their neighbours; a cluster's inner pixels hide behind its outer ones.
    use det::Det;
    use roadcore::grid::{MAX_ELEV, TS};
    const SPIKE_PX: f64 = 3.0;

    pub fn repair(t: &mut [f32], z: u8, lat: f64) -> (usize, usize) {
        let w = TS;
        let bad = |v: f32| !(v <= MAX_ELEV);
        let mut filled = 0;
        if t.iter().any(|&v| bad(v)) {
            if t.iter().all(|&v| bad(v)) {
                filled = t.len();
                t.fill(0.0);
            } else {
                loop {
                    let src = t.to_vec();
                    let mut left = 0;
                    for y in 0..w {
                        for x in 0..w {
                            if !bad(src[y * w + x]) {
                                continue;
                            }
                            let (mut sum, mut n) = (0f32, 0);
                            for dy in -1i32..=1 {
                                for dx in -1i32..=1 {
                                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                                    if (dx, dy) == (0, 0) || xx < 0 || yy < 0 || xx >= w as i32 || yy >= w as i32 {
                                        continue;
                                    }
                                    let v = src[yy as usize * w + xx as usize];
                                    if !bad(v) {
                                        sum += v;
                                        n += 1;
                                    }
                                }
                            }
                            if n > 0 {
                                t[y * w + x] = sum / n as f32;
                                filled += 1;
                            } else {
                                left += 1;
                            }
                        }
                    }
                    if left == 0 {
                        break;
                    }
                }
            }
        }
        let px_m = 40_075_016.7 * lat.to_radians().dcos() / ((1u64 << z) as f64 * w as f64);
        let mut clamped = 0;
        let rise = (0.5 * px_m).max(100.0) as f32;
        let src = t.to_vec();
        let mut ring: Vec<f32> = Vec::with_capacity(40);
        for y in 0..w {
            for x in 0..w {
                let v = src[y * w + x];
                let mut low = f32::MAX;
                for (dx, dy) in [(-1i32, -1i32), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if xx >= 0 && yy >= 0 && xx < w as i32 && yy < w as i32 {
                        low = low.min(src[yy as usize * w + xx as usize]);
                    }
                }
                if !(v > low + rise) {
                    continue;
                }
                ring.clear();
                for dy in -3i32..=3 {
                    for dx in -3i32..=3 {
                        if dx.abs().max(dy.abs()) < 2 {
                            continue;
                        }
                        let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                        if xx >= 0 && yy >= 0 && xx < w as i32 && yy < w as i32 {
                            ring.push(src[yy as usize * w + xx as usize]);
                        }
                    }
                }
                if ring.len() < 12 {
                    continue;
                }
                ring.sort_unstable_by(|a, b| a.total_cmp(b));
                let n = ring.len();
                let (min, max, q1, q3) = (ring[0], ring[n - 1], ring[n / 4], ring[n * 3 / 4]);
                if v > max + rise.max(2.0 * (max - min)) || (q3 - q1 <= 0.25 * rise && v > q3 + rise.max(2.0 * px_m as f32)) {
                    t[y * w + x] = ring[n / 2];
                    clamped += 1;
                }
            }
        }
        let thr = (SPIKE_PX * px_m).max(150.0) as f32;
        let src = t.to_vec();
        for y in 1..w - 1 {
            for x in 1..w - 1 {
                let v = src[y * w + x];
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                for (dx, dy) in [(-1i32, -1i32), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let n = src[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize];
                    lo = lo.min(n);
                    hi = hi.max(n);
                }
                if v > hi + thr {
                    t[y * w + x] = hi;
                    clamped += 1;
                } else if v < lo - thr {
                    t[y * w + x] = lo;
                    clamped += 1;
                }
            }
        }
        (filled, clamped)
    }
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
