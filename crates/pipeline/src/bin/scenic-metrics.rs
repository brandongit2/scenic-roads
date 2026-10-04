//! Scenic analysis of the road network.
//!
//! usage: scenic-metrics <build_dir> prep|canopy|view|buildings [dir]|flags|all  (flags: cheap rerun after POI/heritage/area changes)
//!
//! prep    road sample points (~100 m apart) with observer eye heights; per-vertex drape
//!         heights on the Terrarium surface (what MapLibre renders in 3D)
//! canopy  Meta/WRI 1.2 m canopy heights, streamed per z9 tile:
//!           · z11 grid layers: canopy height (p75 of 5 m max-pooled cells) and cover
//!           · near-field horizons: 32 rays per sample through terrain + trees to 300 m
//!           · roadside tree height and forest cover per sample
//!         Cached (pipeline::scache): grid tiles and samples done before are copied, and only
//!         10° canopy tiles with something new are read.
//! view    far-field viewshed and landscape metrics → per-sample and per-vertex channels
//! buildings  roadside buildings per sample from Overture footprints (data/buildings, `dem/buildings.py`)
//! seed-cache  fill the canopy and view caches (data/cache/scenic) from this build's outputs, so
//!         the next run reuses them (for builds made before the caches existed)

use anyhow::{bail, Context, Result};
use pipeline::count_bar;
use pipeline::scache::{self, GridChange};
use pipeline::terr::TerrainCache;
use pipeline::view::eye_height;
use rayon::prelude::*;
use roadcore::archive::Archive;
use roadcore::grid::{GridIndex, CELLS};
use roadcore::scenic::{sflag, Sample, EYE_M, NEAR_AZ, NEAR_MAX_M, RAIL_EYE_M, SAMPLE_SPACING_M};
use roadcore::{class, dist_m, flag, merc, Array, Ways, E7};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI8, AtomicU8, Ordering::Relaxed};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("data/build"));
    let step = args.get(2).map(String::as_str).unwrap_or("all");
    let t0 = std::time::Instant::now();
    match step {
        "prep" => prep(&dir)?,
        "canopy" => canopy(&dir)?,
        "view" => pipeline::view::run(&dir)?,
        "flags" => pipeline::view::flags(&dir)?,
        "buildings" => pipeline::buildings::run(&dir, &PathBuf::from(args.get(3).map(String::as_str).unwrap_or("data/buildings")))?,
        "seed-cache" => seed_cache(&dir)?,
        "all" => {
            prep(&dir)?;
            canopy(&dir)?;
            pipeline::view::run(&dir)?;
        }
        s => bail!("unknown step {s}"),
    }
    eprintln!("scenic {step}: done in {:.0?}", t0.elapsed());
    Ok(())
}

// ---- caches ------------------------------------------------------------------------------

fn seed_cache(dir: &Path) -> Result<()> {
    use roadcore::scenic::ch;
    let grid = GridIndex::load(dir)?;
    let samples_a = Array::<Sample>::open(&dir.join("samples.bin"))?;
    let samples = samples_a.get();
    let near_a = Array::<i8>::open(&dir.join("near.i8"))?;
    let near = near_a.get();
    let road_a = Array::<u8>::open(&dir.join("roadside.u8"))?;
    let roadside = road_a.get();
    let met_a = Array::<u8>::open(&dir.join("samples.metrics.u8"))?;
    let met = met_a.get();
    anyhow::ensure!(near.len() == samples.len() * NEAR_AZ && roadside.len() == samples.len() * 2 && met.len() == samples.len() * ch::NBASE, "build outputs out of step");
    let cdir = scache::dir(dir);
    let keys: Vec<u64> = samples.par_iter().map(scache::sample_key).collect();
    scache::Prev::save(&cdir, "canopy", &keys)?;
    GridChange::save(&cdir, "canopy", &grid.tiles)?;
    anyhow::ensure!(std::fs::metadata(dir.join("grid.canopy.u8"))?.len() as usize == grid.tiles.len() * CELLS, "grid layers out of step");
    let vkeys: Vec<u64> = samples
        .par_iter()
        .enumerate()
        .map(|(i, s)| scache::mix(scache::mix(scache::sample_key(s), bytemuck::cast_slice(&near[i * NEAR_AZ..(i + 1) * NEAR_AZ])), &roadside[i * 2..i * 2 + 2]))
        .collect();
    scache::Prev::save(&cdir, "view", &vkeys)?;
    GridChange::save(&cdir, "view", &grid.tiles)?;
    eprintln!("seed-cache: {} samples, {} grid tiles", samples.len(), grid.tiles.len());
    Ok(())
}

// ---- prep --------------------------------------------------------------------------------

fn prep(dir: &Path) -> Result<()> {
    let wv = Ways::open(dir)?;
    let ways = wv.ways();
    let verts = wv.verts();
    let fin = roadcore::elev::Stored::open(dir)?;
    let fin = fin.get();
    let arc = Archive::open(&dir.join("terrain.tiles"))?;
    let pb = count_bar(ways.len() as u64, "samples + drape");
    let parts: Vec<(Vec<Sample>, Vec<i16>)> = ways
        .par_chunks(2048)
        .enumerate()
        .map(|(ci, chunk)| {
            let mut tc = TerrainCache::new(&arc);
            let mut samples = Vec::new();
            let mut drape = Vec::new();
            for (k, w) in chunk.iter().enumerate() {
                let wi = (ci * 2048 + k) as u32;
                let s = w.vstart as usize;
                let n = w.vcount as usize;
                let v = &verts[s..s + n];
                let tunnel = w.flags & flag::TUNNEL != 0;
                let bridge = w.flags & flag::BRIDGE != 0 && !tunnel;
                // Drape heights: the rendered terrain surface; tunnels keep their true (buried)
                // level. Bridges store the ground beneath: the renderer lifts the deck to
                // max(ground, elevation), and the difference is the viaduct's height.
                for (j, p) in v.iter().enumerate() {
                    let (mx, my) = merc(p[0] as f64 * E7, p[1] as f64 * E7);
                    let t = tc.at(mx, my);
                    let e = fin.m(s + j);
                    let h = if tunnel { e } else { t };
                    drape.push(h.round().clamp(-500.0, 9000.0) as i16);
                }
                if w.class == class::FERRY {
                    pb.inc(1);
                    continue;
                }
                let mut cd = Vec::with_capacity(n);
                let mut acc = 0.0;
                cd.push(0.0);
                for j in 1..n {
                    acc += dist_m(v[j - 1][0] as f64 * E7, v[j - 1][1] as f64 * E7, v[j][0] as f64 * E7, v[j][1] as f64 * E7);
                    cd.push(acc);
                }
                let m = ((acc / SAMPLE_SPACING_M).round() as usize).max(1);
                for q in 0..m {
                    let d = (q as f64 + 0.5) * acc / m as f64;
                    let j = cd.partition_point(|&x| x <= d).clamp(1, n - 1);
                    let t = if cd[j] > cd[j - 1] { (d - cd[j - 1]) / (cd[j] - cd[j - 1]) } else { 0.0 };
                    let lon = v[j - 1][0] as f64 + (v[j][0] - v[j - 1][0]) as f64 * t;
                    let lat = v[j - 1][1] as f64 + (v[j][1] - v[j - 1][1]) as f64 * t;
                    let (e0, e1) = (fin.dm(s + j - 1), fin.dm(s + j));
                    let e = (e0 as f64 + (e1 - e0) as f64 * t) as f32 / 10.0;
                    let (mx, my) = merc(lon * E7, lat * E7);
                    let terrain = tc.at(mx, my);
                    let ground = if tunnel { e } else { terrain.max(e) };
                    samples.push(Sample {
                        way: wi,
                        dist: d as f32,
                        lon: lon.round() as i32,
                        lat: lat.round() as i32,
                        // Rail: a carriage window is higher than a car's.
                        eye: ground + if class::is_rail(w.class) { RAIL_EYE_M } else { EYE_M },
                        flags: if tunnel { sflag::TUNNEL } else if bridge { sflag::BRIDGE } else { 0 },
                        _pad: [0; 3],
                    });
                }
                pb.inc(1);
            }
            (samples, drape)
        })
        .collect();
    pb.finish_and_clear();
    let mut samples = Vec::new();
    let mut drape = Vec::with_capacity(verts.len());
    for (s, d) in parts {
        samples.extend(s);
        drape.extend(d);
    }
    std::fs::write(roadcore::tmp(dir, "samples.bin"), bytemuck::cast_slice(&samples))?;
    std::fs::write(roadcore::tmp(dir, "vterrain.i16"), bytemuck::cast_slice(&drape))?;
    roadcore::commit(dir, &["samples.bin", "vterrain.i16"])?;
    eprintln!("prep: {} samples, {} drape heights", samples.len(), drape.len());
    Ok(())
}

// ---- canopy ------------------------------------------------------------------------------
//
// Meta/WRI canopy height statistics on a 0.00025° grid (≈28 m × 20 m at 45° N), aggregated
// from their 1.2 m canopy height map: per cell the median and 95th-percentile tree height
// and the share of 1 m pixels taller than 5 m. (The 1.2 m tiles themselves total ~550 GB
// for this region.) Median height is used for occlusion, so scattered trees in a field
// don't wall off a view; p95 gives roadside tree height.

const CHM10_URL: &str = "https://dataforgood-fb-data.s3.amazonaws.com/forests/v1/alsgedi_global_v6_float_epsg4326_v3_10deg";
const C10: usize = 40_000;
const C10_RES: f64 = 0.00025;

struct Chm10 {
    left: f64,
    top: f64,
    median: Vec<u8>, // metres
    p95: Vec<u8>,    // metres
    cover: Vec<u8>,  // share > 5 m, ×255
}

impl Chm10 {
    #[inline]
    fn idx(&self, lon: f64, lat: f64) -> Option<usize> {
        let c = ((lon - self.left) / C10_RES).floor();
        let r = ((self.top - lat) / C10_RES).floor();
        (c >= 0.0 && r >= 0.0 && c < C10 as f64 && r < C10 as f64).then(|| r as usize * C10 + c as usize)
    }
}

fn fetch_file(agent: &ureq::Agent, url: &str, path: &Path) -> Result<Option<Vec<u8>>> {
    if let Ok(b) = std::fs::read(path) {
        // Used now: the build agent's room-making deletes the least recently used squares first.
        if let Ok(f) = std::fs::File::options().append(true).open(path) {
            f.set_modified(std::time::SystemTime::now()).ok();
        }
        return Ok((!b.is_empty()).then_some(b));
    }
    for attempt in 0..6 {
        match agent.get(url).call() {
            Ok(mut r) => {
                if let Ok(b) = r.body_mut().with_config().limit(3_000_000_000).read_to_vec() {
                    let tmp = path.with_extension("part");
                    std::fs::write(&tmp, &b)?;
                    std::fs::rename(&tmp, path)?;
                    return Ok(Some(b));
                }
            }
            Err(ureq::Error::StatusCode(404 | 403)) => {
                std::fs::write(path, b"")?;
                return Ok(None);
            }
            Err(_) => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(1000 << attempt));
    }
    bail!("download failed: {url}")
}

/// Minimal little-endian TIFF strip index.
struct Strips {
    width: usize,
    height: usize,
    rows_per_strip: usize,
    compression: u64,
    strips: Vec<(usize, usize)>,
}

fn parse_tiff(b: &[u8]) -> Result<Strips> {
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    if &b[0..2] != b"II" {
        bail!("not little-endian TIFF");
    }
    let big = match u16_at(2) {
        42 => false,
        43 => true,
        v => bail!("TIFF version {v}"),
    };
    let ifd = if big { u64_at(8) as usize } else { u32_at(4) as usize };
    let (count, entry0, esz) = if big { (u64_at(ifd) as usize, ifd + 8, 20) } else { (u16_at(ifd) as usize, ifd + 2, 12) };
    let mut tags: HashMap<u16, Vec<u64>> = HashMap::new();
    for i in 0..count {
        let e = entry0 + i * esz;
        let (tag, typ) = (u16_at(e), u16_at(e + 2));
        let n = if big { u64_at(e + 4) as usize } else { u32_at(e + 4) as usize };
        let sz = match typ {
            3 => 2,
            4 => 4,
            16 => 8,
            _ => continue,
        };
        let inline = if big { 8 } else { 4 };
        let base = if n * sz <= inline { e + if big { 12 } else { 8 } } else if big { u64_at(e + 12) as usize } else { u32_at(e + 8) as usize };
        let vals = (0..n)
            .map(|k| match sz {
                2 => u16_at(base + k * 2) as u64,
                4 => u32_at(base + k * 4) as u64,
                _ => u64_at(base + k * 8),
            })
            .collect();
        tags.insert(tag, vals);
    }
    let get = |t: u16| tags.get(&t).and_then(|v| v.first().copied()).context(format!("TIFF tag {t}"));
    let offs = tags.get(&273).context("StripOffsets")?;
    let lens = tags.get(&279).context("StripByteCounts")?;
    Ok(Strips {
        width: get(256)? as usize,
        height: get(257)? as usize,
        rows_per_strip: get(278).unwrap_or(1) as usize,
        compression: get(259)?,
        strips: offs.iter().zip(lens).map(|(&o, &l)| (o as usize, l as usize)).collect(),
    })
}

/// Decode a 40000² uint16 LZW TIFF, mapping each value through `f` to u8 (65535 = no data → 0).
fn decode_u16(b: &[u8], f: impl Fn(u16) -> u8 + Sync) -> Result<Vec<u8>> {
    let st = parse_tiff(b)?;
    if st.width != C10 || st.height != C10 || st.compression != 5 {
        bail!("unexpected TIFF {}×{} compression {}", st.width, st.height, st.compression);
    }
    let mut out = vec![0u8; C10 * C10];
    out.par_chunks_mut(C10 * st.rows_per_strip).zip(&st.strips).for_each(|(dst, &(off, len))| {
        let mut dec = weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8);
        if let Ok(raw) = dec.decode(&b[off..off + len]) {
            for (o, v) in dst.iter_mut().zip(raw.chunks_exact(2)) {
                let x = u16::from_le_bytes([v[0], v[1]]);
                *o = if x == 65535 { 0 } else { f(x) };
            }
        }
    });
    Ok(out)
}

fn canopy(dir: &Path) -> Result<()> {
    let grid = GridIndex::load(dir)?;
    let terr_l = roadcore::grid::Layer::<i16>::open(&dir.join("grid.terrain.i16"))?;
    let terr = terr_l.data();
    let samples_a = Array::<Sample>::open(&dir.join("samples.bin"))?;
    let samples = samples_a.get();
    // The canopy files: data/cache/chm10 next to the build, or under SCENIC_CACHE (shared by units).
    let cache = match std::env::var_os("SCENIC_CACHE") {
        Some(c) => PathBuf::from(c).join("chm10"),
        None => dir.parent().unwrap().join("cache/chm10"),
    };
    std::fs::create_dir_all(&cache)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(1800))).build().into();

    // 10° tiles needed: (top latitude, left longitude).
    let mut need: BTreeSet<(i32, i32)> = BTreeSet::new();
    for t in &grid.tiles {
        for (dx, dy) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
            let (x, y) = ((t[0] + dx) as f64 * 256.0, (t[1] + dy) as f64 * 256.0);
            let lon = x / roadcore::grid::WORLD * 360.0 - 180.0;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / roadcore::grid::WORLD)).sinh().atan().to_degrees();
            need.insert(((lat / 10.0).ceil() as i32 * 10, (lon / 10.0).floor() as i32 * 10));
        }
    }
    // Previous results: samples and grid tiles done in the last run (its outputs are still here),
    // unless new terrain is within reach.
    let cdir = scache::dir(dir);
    let change = GridChange::load(&cdir, "canopy", &grid.tiles);
    let prev = scache::Prev::load(&cdir, "canopy");
    let old_near = Array::<i8>::open(&dir.join("near.i8")).ok().filter(|a| a.get().len() == prev.len() * NEAR_AZ && !prev.is_empty());
    let old_road = Array::<u8>::open(&dir.join("roadside.u8")).ok().filter(|a| a.get().len() == prev.len() * 2);
    let near: Vec<AtomicI8> = (0..samples.len() * NEAR_AZ).map(|_| AtomicI8::new(i8::MIN)).collect();
    let roadside: Vec<AtomicU8> = (0..samples.len() * 2).map(|_| AtomicU8::new(0)).collect();
    let todo_s: Vec<bool> = samples
        .par_iter()
        .enumerate()
        .map(|(si, s)| {
            let (Some(on), Some(or)) = (&old_near, &old_road) else { return true };
            if change.near(s.lon as f64 * E7, s.lat as f64 * E7, 1) {
                return true;
            }
            match prev.row(scache::sample_key(s)) {
                Some(r) => {
                    let (on, or) = (on.get(), or.get());
                    for a in 0..NEAR_AZ {
                        near[si * NEAR_AZ + a].store(on[r * NEAR_AZ + a], Relaxed);
                    }
                    roadside[si * 2].store(or[r * 2], Relaxed);
                    roadside[si * 2 + 1].store(or[r * 2 + 1], Relaxed);
                    false
                }
                None => true,
            }
        })
        .collect();
    let mut canopy_out = vec![0u8; grid.tiles.len() * CELLS];
    let mut cover_out = vec![0u8; grid.tiles.len() * CELLS];
    // Grid tiles: copied from the last run's layers where the tile was there.
    let mut todo_t = vec![true; grid.tiles.len()];
    let old_can = Array::<u8>::open(&dir.join("grid.canopy.u8")).ok().filter(|a| a.get().len() == change.prev_len() * CELLS && change.prev_len() > 0);
    let old_cov = Array::<u8>::open(&dir.join("grid.cover.u8")).ok().filter(|a| a.get().len() == change.prev_len() * CELLS);
    if let (Some(oc), Some(ov)) = (&old_can, &old_cov) {
        let slot = change.prev_slots();
        for (i, t) in grid.tiles.iter().enumerate() {
            if let Some(&j) = slot.get(t) {
                canopy_out[i * CELLS..(i + 1) * CELLS].copy_from_slice(&oc.get()[j * CELLS..(j + 1) * CELLS]);
                cover_out[i * CELLS..(i + 1) * CELLS].copy_from_slice(&ov.get()[j * CELLS..(j + 1) * CELLS]);
                todo_t[i] = false;
            }
        }
    }
    let n_s = todo_s.iter().filter(|&&t| t).count();
    let n_t = todo_t.iter().filter(|&&t| t).count();
    // 10° tiles with something to do (a sample or grid tile, with the near-field margin).
    let margin = NEAR_MAX_M / 111_000.0 * 2.0;
    let touches = |top: i32, left: i32, lon: f64, lat: f64| {
        lon >= left as f64 - margin && lon <= left as f64 + 10.0 + margin && lat <= top as f64 + margin && lat >= top as f64 - 10.0 - margin
    };
    need.retain(|&(top, left)| {
        samples.iter().zip(&todo_s).any(|(s, &t)| t && touches(top, left, s.lon as f64 * E7, s.lat as f64 * E7))
            || grid.tiles.iter().zip(&todo_t).any(|(g, &t)| {
                if !t {
                    return false;
                }
                let lon = (g[0] as f64 + 0.5) * 256.0 / roadcore::grid::WORLD * 360.0 - 180.0;
                let y = (g[1] as f64 + 0.5) * 256.0 / roadcore::grid::WORLD;
                let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
                touches(top, left, lon, lat) || (lon - (left as f64 + 5.0)).abs() < 6.0 && (lat - (top as f64 - 5.0)).abs() < 6.0
            })
    });
    eprintln!(
        "canopy: {} samples ({} to do, {} cached), {} grid tiles ({} to do), {} ten-degree tiles to read",
        samples.len(),
        n_s,
        samples.len() - n_s,
        grid.tiles.len(),
        n_t,
        need.len()
    );
    let pb = count_bar(need.len() as u64, "canopy 10° tiles");
    for &(top, left) in &need {
        let name = |st: &str| format!("meta_chm_lat={top}.0_lon={left}.0_{st}.tif");
        let files: Vec<Option<Vec<u8>>> = ["median", "p95", "cover5m"]
            .par_iter()
            .map(|st| fetch_file(&agent, &format!("{CHM10_URL}/{}", name(st)), &cache.join(name(st))))
            .collect::<Result<_>>()?;
        let [Some(med), Some(p95), Some(cov)] = [&files[0], &files[1], &files[2]] else {
            pb.println(format!("canopy {top},{left}: no data"));
            pb.inc(1);
            continue;
        };
        let t = Chm10 {
            left: left as f64,
            top: top as f64,
            median: decode_u16(med, |v| ((v as u32 + 50) / 100).min(254) as u8)?,
            p95: decode_u16(p95, |v| ((v as u32 + 50) / 100).min(254) as u8)?,
            cover: decode_u16(cov, |v| ((v as u32).min(1000) * 255 / 1000) as u8)?,
        };
        drop(files);

        // Grid layers: cells whose centre lies in this tile (4 sub-samples per cell), for tiles to do.
        canopy_out.par_chunks_mut(CELLS).zip(cover_out.par_chunks_mut(CELLS)).zip(grid.tiles.par_iter().zip(&todo_t)).for_each(|((can, cov), (tile, &todo))| {
            if !todo {
                return;
            }
            for cy in 0..256usize {
                for cx in 0..256usize {
                    let (gx, gy) = (tile[0] as f64 * 256.0 + cx as f64, tile[1] as f64 * 256.0 + cy as f64);
                    let (mut sh, mut sc, mut n) = (0u32, 0u32, 0u32);
                    for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                        let lon = (gx + ox) / roadcore::grid::WORLD * 360.0 - 180.0;
                        let lat = (std::f64::consts::PI * (1.0 - 2.0 * (gy + oy) / roadcore::grid::WORLD)).sinh().atan().to_degrees();
                        if let Some(i) = t.idx(lon, lat) {
                            sh += t.median[i] as u32;
                            sc += t.cover[i] as u32;
                            n += 1;
                        }
                    }
                    if n > 0 {
                        can[cy * 256 + cx] = (sh / n) as u8;
                        cov[cy * 256 + cx] = (sc / n) as u8;
                    }
                }
            }
        });

        // Near field for samples to do in (or within 300 m of) this tile.
        samples.par_iter().enumerate().for_each(|(si, s)| {
            if !todo_s[si] {
                return;
            }
            let (lon, lat) = (s.lon as f64 * E7, s.lat as f64 * E7);
            if lon < t.left - margin || lon > t.left + 10.0 + margin || lat > t.top + margin || lat < t.top - 10.0 - margin {
                return;
            }
            let owned = lon >= t.left && lon < t.left + 10.0 && lat <= t.top && lat > t.top - 10.0;
            near_field(s, si, owned, &t, &grid, terr, &near, &roadside);
        });
        pb.inc(1);
    }
    pb.finish_and_clear();
    let near_bytes: Vec<i8> = near.into_iter().map(|a| a.into_inner()).collect();
    let road_bytes: Vec<u8> = roadside.into_iter().map(|a| a.into_inner()).collect();
    std::fs::write(roadcore::tmp(dir, "grid.canopy.u8"), &canopy_out)?;
    std::fs::write(roadcore::tmp(dir, "grid.cover.u8"), &cover_out)?;
    std::fs::write(roadcore::tmp(dir, "near.i8"), bytemuck::cast_slice(&near_bytes))?;
    std::fs::write(roadcore::tmp(dir, "roadside.u8"), &road_bytes)?;
    roadcore::commit(dir, &["grid.canopy.u8", "grid.cover.u8", "near.i8", "roadside.u8"])?;
    // What each row is, for the next run.
    let keys: Vec<u64> = samples.par_iter().map(scache::sample_key).collect();
    scache::Prev::save(&cdir, "canopy", &keys)?;
    GridChange::save(&cdir, "canopy", &grid.tiles)?;
    Ok(())
}

/// Near-field horizon per direction (0.5° units; merged by max across tiles) through
/// terrain + median canopy height to 300 m, plus roadside p95 tree height and forest cover
/// within 150 m for the owning tile.
#[allow(clippy::too_many_arguments)]
fn near_field(s: &Sample, si: usize, owned: bool, t: &Chm10, grid: &GridIndex, terr: &[i16], near: &[AtomicI8], roadside: &[AtomicU8]) {
    let (lon, lat) = (s.lon as f64 * E7, s.lat as f64 * E7);
    let m_lat = 111_320.0;
    let m_lon = 111_320.0 * lat.to_radians().cos();
    let (gx, gy) = roadcore::grid::cell_of(lon, lat);
    let cm = roadcore::grid::cell_m(lat);
    let eye = eye_height(s, grid, terr);
    let tunnel = s.flags & sflag::TUNNEL != 0;
    for a in 0..NEAR_AZ {
        let th = a as f64 * std::f64::consts::TAU / NEAR_AZ as f64;
        let (sx, sy) = (th.sin(), th.cos()); // east, north
        let mut best = f64::MIN;
        let mut d = 8.0;
        while d <= NEAR_MAX_M {
            let (plon, plat) = (lon + sx * d / m_lon, lat + sy * d / m_lat);
            if let Some(i) = t.idx(plon, plat) {
                let ground = roadcore::grid::terrain_bilinear(grid, terr, gx + sx * d / cm, gy - sy * d / cm).unwrap_or(eye - EYE_M);
                let h = ground + t.median[i] as f32;
                best = best.max((h - eye) as f64 / d);
            }
            d += 14.0;
        }
        if tunnel {
            best = best.max(10.0);
        }
        if best > f64::MIN {
            let v = (best.atan().to_degrees() * 2.0).round().clamp(-127.0, 127.0) as i8;
            near[si * NEAR_AZ + a].fetch_max(v, Relaxed);
        }
    }
    if owned {
        let (mut sum, mut n, mut cov, mut m) = (0f32, 0f32, 0f32, 0f32);
        for a in 0..16 {
            let th = a as f64 * std::f64::consts::TAU / 16.0;
            let (sx, sy) = (th.sin(), th.cos());
            for d in [10.0, 20.0, 30.0] {
                if let Some(i) = t.idx(lon + sx * d / m_lon, lat + sy * d / m_lat) {
                    sum += t.p95[i] as f32;
                    n += 1.0;
                }
            }
            for k in 1..=10 {
                let d = 15.0 * k as f64;
                if let Some(i) = t.idx(lon + sx * d / m_lon, lat + sy * d / m_lat) {
                    cov += t.cover[i] as f32;
                    m += 1.0;
                }
            }
        }
        if n > 0.0 {
            roadside[si * 2].store(((sum / n) * 8.0).round().min(255.0) as u8, Relaxed);
        }
        if m > 0.0 {
            roadside[si * 2 + 1].store((cov / m).round().min(255.0) as u8, Relaxed);
        }
    }
}
