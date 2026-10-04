//! Far-field viewshed and landscape metrics per road sample, then per-vertex channels.
//!
//! Visibility: 32 rays per sample from 300 m to 15 km over the z11 grid (terrain + p75
//! canopy height, earth curvature with standard refraction). Each ray starts from that
//! direction's near-field horizon (terrain + 5 m tree canopy within 300 m), so roadside
//! trees block distant views. Visible area is accumulated per ray sector.
//!
//! Cached (crate::scache): a sample keeps its last metrics while its position, eye, near field
//! and roadside values are the same and no analysis-grid tile within the far field's reach (2
//! tiles, more above ~67° where they're narrower) is new.

use crate::count_bar;
use anyhow::Result;
use rayon::prelude::*;
use roadcore::grid::{cell_m, cell_of, class as lc, GridIndex, Layer};
use roadcore::scenic::{area_u8, ch, flag as sf, sflag, Sample, FAR_MAX_M, NEAR_AZ, NEAR_MAX_M};
use roadcore::{class, dist_m, flag, merc, Array, Ways, E7};
use std::collections::HashMap;
use std::path::Path;

const R_EFF: f64 = 6_371_000.0 / 0.87; // refraction-adjusted earth radius

struct Grids {
    idx: GridIndex,
    terrain: Layer<i16>,
    canopy: Option<Layer<u8>>,
    class: Option<Layer<u8>>,
}

/// Points of interest bucketed on a ~1 km Web-Mercator grid.
struct PointHash {
    cells: HashMap<(i64, i64), Vec<(f64, f64)>>,
    scale: f64,
}

impl PointHash {
    fn new(pts: &[(f64, f64)]) -> Self {
        let scale = (1u64 << 15) as f64; // ~1.2 km cells at the equator
        let mut cells: HashMap<(i64, i64), Vec<(f64, f64)>> = HashMap::new();
        for &(lon, lat) in pts {
            let (x, y) = merc(lon, lat);
            cells.entry(((x * scale) as i64, (y * scale) as i64)).or_default().push((lon, lat));
        }
        Self { cells, scale }
    }
    fn len(&self) -> usize {
        self.cells.values().map(Vec::len).sum()
    }
    fn within(&self, lon: f64, lat: f64, r_m: f64) -> bool {
        let (x, y) = merc(lon, lat);
        let (cx, cy) = ((x * self.scale) as i64, (y * self.scale) as i64);
        for dy in -1..=1 {
            for dx in -1..=1 {
                if let Some(v) = self.cells.get(&(cx + dx, cy + dy)) {
                    if v.iter().any(|&(a, b)| dist_m(lon, lat, a, b) <= r_m) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn load_points(path: &Path, kinds: &[&str]) -> Vec<(f64, f64)> {
    let Ok(txt) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { return Vec::new() };
    v["features"]
        .as_array()
        .map(|fs| {
            fs.iter()
                .filter(|f| kinds.is_empty() || f["properties"]["kind"].as_str().is_some_and(|k| kinds.contains(&k)))
                .filter_map(|f| {
                    let c = f["geometry"]["coordinates"].as_array()?;
                    Some((c[0].as_f64()?, c[1].as_f64()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A z11 grid tile's width at the equator (m); it narrows with the cosine of the latitude.
const TILE_M_EQUATOR: f64 = 40_075_016.7 / 2048.0;

/// Heavy pass: viewsheds and landscape metrics per sample (→ samples.ch.u8), then flags and
/// per-vertex channels.
pub fn run(dir: &Path) -> Result<()> {
    let open_opt = |n: &str| Layer::<u8>::open(&dir.join(n)).ok();
    let g = Grids {
        idx: GridIndex::load(dir)?,
        terrain: Layer::open(&dir.join("grid.terrain.i16"))?,
        canopy: open_opt("grid.canopy.u8"),
        class: open_opt("grid.class.u8"),
    };
    let samples_a = Array::<Sample>::open(&dir.join("samples.bin"))?;
    let samples = samples_a.get();
    let near_a = Array::<i8>::open(&dir.join("near.i8"))?;
    let near = near_a.get();
    let road_a = Array::<u8>::open(&dir.join("roadside.u8"))?;
    let roadside = road_a.get();
    eprintln!(
        "view: {} samples; canopy {} · land cover {}",
        samples.len(),
        g.canopy.is_some(),
        g.class.is_some(),
    );

    // Previous metrics (still in the build) by sample key, unless new grid tiles are within reach.
    let cdir = crate::scache::dir(dir);
    let change = crate::scache::GridChange::load(&cdir, "view", &g.idx.tiles);
    let prev = crate::scache::Prev::load(&cdir, "view");
    let old = Array::<u8>::open(&dir.join("samples.metrics.u8")).ok().filter(|a| a.get().len() == prev.len() * ch::NBASE && !prev.is_empty());
    let keys: Vec<u64> = samples
        .par_iter()
        .enumerate()
        .map(|(i, s)| {
            let k = crate::scache::mix(crate::scache::sample_key(s), bytemuck::cast_slice(&near[i * NEAR_AZ..(i + 1) * NEAR_AZ]));
            crate::scache::mix(k, &roadside[i * 2..i * 2 + 2])
        })
        .collect();
    let pb = count_bar(samples.len() as u64, "viewsheds");
    let reused = std::sync::atomic::AtomicUsize::new(0);
    let chans: Vec<[u8; ch::NBASE]> = samples
        .par_iter()
        .enumerate()
        .map(|(i, s)| {
            if i % 4096 == 0 {
                pb.inc(4096);
            }
            if let Some(o) = &old {
                // New grid tiles within the far field's 15 km: 2 z11 tiles, more where they're
                // narrower (above ~67°).
                let lat = s.lat as f64 * E7;
                let ring = ((FAR_MAX_M / (TILE_M_EQUATOR * lat.to_radians().cos().max(0.01))).ceil() as i64).max(2);
                if !change.near(s.lon as f64 * E7, lat, ring) {
                    if let Some(r) = prev.row(keys[i]) {
                        reused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return o.get()[r * ch::NBASE..(r + 1) * ch::NBASE].try_into().unwrap();
                    }
                }
            }
            sample_metrics(&g, s, &near[i * NEAR_AZ..(i + 1) * NEAR_AZ], &roadside[i * 2..i * 2 + 2])
        })
        .collect();
    pb.finish_and_clear();
    eprintln!("view: {} of {} samples reused from the last run", reused.into_inner(), samples.len());
    drop(old);
    std::fs::write(roadcore::tmp(dir, "samples.metrics.u8"), bytemuck::cast_slice(&chans))?;
    roadcore::commit(dir, &["samples.metrics.u8"])?;
    crate::scache::Prev::save(&cdir, "view", &keys)?;
    crate::scache::GridChange::save(&cdir, "view", &g.idx.tiles)?;
    let mean = |c: usize| chans.iter().map(|x| x[c] as f64).sum::<f64>() / chans.len().max(1) as f64;
    eprintln!(
        "view: mean view {:.0}, water {:.0}, vista {:.1} km, enclosure {:.0} %",
        mean(ch::VIEW),
        mean(ch::WATER),
        mean(ch::VISTA) / 17.0,
        mean(ch::ENCLOSURE) / 2.55
    );
    drop(chans);
    flags(dir)
}

/// Cheap pass, rerun whenever POIs, heritage or designated areas change: sample flags from
/// viewpoints (1 km), heritage sites (500 m) and the rasterised areas (parks, heritage
/// districts, special areas, Indigenous lands), then per-vertex
/// channels (→ samples.ch.u8, scenic.u8).
pub fn flags(dir: &Path) -> Result<()> {
    let idx = GridIndex::load(dir)?;
    // Designation areas on the grid; one from before the grid changed (the network grew) is
    // ignored until `heritage.py` rebuilds it.
    let terrain_cells = Layer::<i16>::open(&dir.join("grid.terrain.i16"))?.data().len();
    let areas = Layer::<u8>::open(&dir.join("grid.areas.u8")).ok().filter(|a| a.data().len() == terrain_cells);
    let samples_a = Array::<Sample>::open(&dir.join("samples.bin"))?;
    let samples = samples_a.get();
    let base = Array::<[u8; ch::NBASE]>::open(&dir.join("samples.metrics.u8"))?;
    anyhow::ensure!(base.get().len() == samples.len(), "samples.metrics.u8 does not match samples.bin; rerun `scenic view`");
    // Roadside buildings (`scenic buildings`); none until it has run for these samples.
    let bld_a = Array::<u8>::open(&dir.join("samples.bld.u8")).ok().filter(|a| a.get().len() == samples.len());
    let bld = bld_a.as_ref().map(|a| a.get());
    let viewpoints = PointHash::new(&load_points(&dir.join("pois.json"), &["viewpoint"]));
    let heritage = PointHash::new(&load_points(&dir.join("heritage.json"), &[]));
    eprintln!(
        "flags: {} viewpoints, {} heritage sites, areas {}, roadside buildings {}",
        viewpoints.len(),
        heritage.len(),
        areas.is_some(),
        bld.is_some()
    );
    let pb = count_bar(samples.len() as u64, "sample flags");
    let chans: Vec<[u8; ch::N]> = samples
        .par_iter()
        .zip(base.get().par_iter())
        .enumerate()
        .map(|(i, (s, b))| {
            let mut c = [0u8; ch::N];
            c[..ch::NBASE].copy_from_slice(b);
            c[ch::BLDG] = bld.map_or(0, |x| x[i]);
            let (lon, lat) = (s.lon as f64 * E7, s.lat as f64 * E7);
            let mut f = c[ch::FLAGS] & sf::WATERFRONT;
            if viewpoints.within(lon, lat, 1000.0) {
                f |= sf::VIEWPOINT;
            }
            if heritage.within(lon, lat, 500.0) {
                f |= sf::HERITAGE;
            }
            if let Some(ar) = areas.as_ref() {
                let (gx, gy) = cell_of(lon, lat);
                if let Some(k) = idx.cell(gx as i64, gy as i64) {
                    f |= ar.data()[k] & (sf::PARK | sf::HERITAGE | sf::SPECIAL_AREA | sf::INDIGENOUS);
                }
            }
            c[ch::FLAGS] = f;
            if i % 65536 == 0 {
                pb.inc(65536);
            }
            c
        })
        .collect();
    pb.finish_and_clear();
    std::fs::write(roadcore::tmp(dir, "samples.ch.u8"), bytemuck::cast_slice(&chans))?;

    // Per-vertex channels.
    let wv = Ways::open(dir)?;
    let ways = wv.ways();
    let verts = wv.verts();
    let mut range = vec![(0u32, 0u32); ways.len()];
    let mut k = 0usize;
    while k < samples.len() {
        let w = samples[k].way as usize;
        let s0 = k;
        while k < samples.len() && samples[k].way as usize == w {
            k += 1;
        }
        range[w] = (s0 as u32, k as u32);
    }
    let pb = count_bar(ways.len() as u64, "per-vertex channels");
    let parts: Vec<Vec<[u8; ch::N]>> = ways
        .par_chunks(4096)
        .enumerate()
        .map(|(ci, chunk)| {
            let mut out = Vec::new();
            for (j, w) in chunk.iter().enumerate() {
                let wi = ci * 4096 + j;
                let v = &verts[w.vstart as usize..(w.vstart + w.vcount as u64) as usize];
                let (a, b) = range[wi];
                out.extend(vertex_channels(w, v, &samples[a as usize..b as usize], &chans[a as usize..b as usize]));
                pb.inc(1);
            }
            out
        })
        .collect();
    pb.finish_and_clear();
    let per_vertex: Vec<[u8; ch::N]> = parts.into_iter().flatten().collect();
    std::fs::write(roadcore::tmp(dir, "scenic.u8"), bytemuck::cast_slice(&per_vertex))?;
    roadcore::commit(dir, &["samples.ch.u8", "scenic.u8"])?;
    let share = |m: u8| chans.iter().filter(|c| c[ch::FLAGS] & m != 0).count() as f64 / chans.len().max(1) as f64 * 100.0;
    eprintln!(
        "flags: waterfront {:.1} %, viewpoint {:.1} %, heritage {:.1} %, park {:.1} %, special {:.1} %, indigenous {:.1} %",
        share(sf::WATERFRONT),
        share(sf::VIEWPOINT),
        share(sf::HERITAGE),
        share(sf::PARK),
        share(sf::SPECIAL_AREA),
        share(sf::INDIGENOUS)
    );
    Ok(())
}

pub fn eye_height(s: &Sample, grid: &GridIndex, terr: &[i16]) -> f32 {
    if s.flags & sflag::TUNNEL != 0 {
        return s.eye;
    }
    let (gx, gy) = cell_of(s.lon as f64 * E7, s.lat as f64 * E7);
    roadcore::grid::terrain_bilinear(grid, terr, gx, gy).map_or(s.eye, |t| s.eye.max(t + 1.5))
}

fn sample_metrics(g: &Grids, s: &Sample, near: &[i8], roadside: &[u8]) -> [u8; ch::NBASE] {
    let mut c = [0u8; ch::NBASE];
    let (lon, lat) = (s.lon as f64 * E7, s.lat as f64 * E7);
    let (gx, gy) = cell_of(lon, lat);
    let cm = cell_m(lat);
    let terr = g.terrain.data();
    let eye = eye_height(s, &g.idx, terr) as f64;
    let ground = eye - 1.5;
    let tunnel = s.flags & sflag::TUNNEL != 0;
    let can = g.canopy.as_ref().map(|l| l.data());
    let cls = g.class.as_ref().map(|l| l.data());
    let at = |d: f64, ux: f64, uy: f64| g.idx.cell((gx + ux * d / cm) as i64, (gy + uy * d / cm) as i64);
    let is_water = |i: usize| cls.is_some_and(|k| k[i] == lc::WATER) || (cls.is_some_and(|k| k[i] == lc::NONE) && terr[i] <= 1);

    let dth = std::f64::consts::TAU / NEAR_AZ as f64;
    let (mut area, mut water, mut vista_sum) = (0f64, 0f64, 0f64);
    let (mut tpi_sum, mut tpi_w) = (0f64, 0f64);
    let (mut rmin, mut rmax) = (ground, ground);
    let (mut open_n, mut open_d) = (0f64, 0f64);
    let (mut built_n, mut built_d) = (0f64, 0f64);
    let mut water_dist = f64::MAX;
    let mut blocked = 0;
    for a in 0..NEAR_AZ {
        let th = a as f64 * dth;
        let (ux, uy) = (th.sin(), -th.cos());
        // Landscape sweep (terrain only), 25 m – 3 km.
        let mut d = 25.0;
        while d <= 3000.0 {
            if let Some(i) = at(d, ux, uy) {
                let t = terr[i] as f64;
                rmin = rmin.min(t);
                rmax = rmax.max(t);
                if d <= 1500.0 {
                    tpi_sum += t * d;
                    tpi_w += d;
                }
                if let Some(k) = cls {
                    if d <= 1000.0 {
                        open_d += d;
                        if k[i] == lc::OPEN || k[i] == lc::SNOW {
                            open_n += d;
                        }
                    }
                    if d <= 500.0 {
                        built_d += d;
                        if k[i] == lc::BUILT {
                            built_n += d;
                        }
                    }
                }
                if d < water_dist && is_water(i) {
                    water_dist = d;
                }
            }
            d += if d < 100.0 { 25.0 } else { 100.0 };
        }
        // Visibility.
        let na = near[a];
        if na >= 10 {
            blocked += 1;
        }
        if tunnel {
            continue;
        }
        let mut smax = if na == i8::MIN { f64::MIN } else { (na as f64 * 0.5).to_radians().tan() };
        let mut far = 0f64;
        let mut d = NEAR_MAX_M;
        while d <= FAR_MAX_M {
            let step = cm.max(d * 0.03);
            let Some(i) = at(d, ux, uy) else { break };
            let h = terr[i] as f64 + can.map_or(0.0, |cv| cv[i] as f64);
            let sl = (h - eye - d * d / (2.0 * R_EFF)) / d;
            if sl >= smax {
                let da = step * d * dth;
                area += da;
                if is_water(i) {
                    water += da;
                }
                far = d;
                smax = sl;
            }
            d += step;
        }
        vista_sum += far;
    }
    if !tunnel {
        c[ch::VIEW] = area_u8(area / 1e6);
        c[ch::WATER] = area_u8(water / 1e6);
        c[ch::VISTA] = ((vista_sum / NEAR_AZ as f64) / 1000.0 * 17.0).round().min(255.0) as u8;
    }
    c[ch::RELIEF] = ((rmax - rmin) / 3.0).round().min(255.0) as u8;
    if tpi_w > 0.0 {
        c[ch::TPI] = (128.0 + (ground - tpi_sum / tpi_w) / 2.0).round().clamp(0.0, 255.0) as u8;
    } else {
        c[ch::TPI] = 128;
    }
    c[ch::ENCLOSURE] = ((blocked as f64 / NEAR_AZ as f64) * 255.0).round() as u8;
    if built_d > 0.0 {
        c[ch::BUILT] = (built_n / built_d * 255.0).round() as u8;
    }
    if open_d > 0.0 {
        c[ch::OPEN] = (open_n / open_d * 255.0).round() as u8;
    }
    c[ch::TREEH] = roadside[0];
    c[ch::COVER] = roadside[1];
    let mut f = 0u8;
    if water_dist <= 100.0 {
        f |= sf::WATERFRONT;
    }
    c[ch::FLAGS] = f;
    c
}

/// Interpolate sample channels onto a way's vertices (flags: nearest sample), add curviness
/// and way-level flags.
fn vertex_channels(w: &roadcore::WayRec, v: &[[i32; 2]], ss: &[Sample], cs: &[[u8; ch::N]]) -> Vec<[u8; ch::N]> {
    let n = v.len();
    let mut d = vec![0f64; n];
    let mut head = vec![0f64; n];
    for j in 1..n {
        let (x0, y0, x1, y1) = (v[j - 1][0] as f64 * E7, v[j - 1][1] as f64 * E7, v[j][0] as f64 * E7, v[j][1] as f64 * E7);
        d[j] = d[j - 1] + dist_m(x0, y0, x1, y1);
        head[j] = (y1 - y0).atan2((x1 - x0) * y0.to_radians().cos()).to_degrees();
    }
    // Turning per vertex, then windowed sum (±250 m) → degrees per km.
    let mut turn = vec![0f64; n];
    for j in 2..n {
        if d[j] - d[j - 1] < 0.5 || d[j - 1] - d[j - 2] < 0.5 {
            continue;
        }
        let mut t = (head[j] - head[j - 1]).abs();
        if t > 180.0 {
            t = 360.0 - t;
        }
        turn[j - 1] = t;
    }
    let mut pre = vec![0f64; n + 1];
    for j in 0..n {
        pre[j + 1] = pre[j] + turn[j];
    }
    let way_flags = (if w.flags & flag::SCENIC != 0 { sf::SCENIC_ROUTE } else { 0 })
        | (if w.flags & flag::COVERED != 0 { sf::COVERED_BRIDGE } else { 0 });
    let mut out = Vec::with_capacity(n);
    let (mut lo, mut hi) = (0usize, 0usize);
    for j in 0..n {
        let mut c = [0u8; ch::N];
        if !ss.is_empty() {
            let k = ss.partition_point(|s| (s.dist as f64) < d[j]);
            let (a, b) = if k == 0 { (0, 0) } else if k >= ss.len() { (ss.len() - 1, ss.len() - 1) } else { (k - 1, k) };
            let t = if a == b { 0.0 } else { ((d[j] - ss[a].dist as f64) / (ss[b].dist - ss[a].dist) as f64).clamp(0.0, 1.0) };
            for q in 0..ch::N {
                c[q] = (cs[a][q] as f64 * (1.0 - t) + cs[b][q] as f64 * t).round() as u8;
            }
            c[ch::FLAGS] = if t < 0.5 { cs[a][ch::FLAGS] } else { cs[b][ch::FLAGS] };
        } else {
            c[ch::TPI] = 128;
        }
        while d[j] - d[lo] > 250.0 {
            lo += 1;
        }
        while hi + 1 < n && d[hi + 1] - d[j] <= 250.0 {
            hi += 1;
        }
        let span = (d[hi] - d[lo]).max(100.0) / 1000.0;
        let deg_km = (pre[hi + 1] - pre[lo]) / span;
        c[ch::CURVY] = if w.class == class::FERRY { 0 } else { (deg_km / 4.0).round().min(255.0) as u8 };
        c[ch::FLAGS] |= way_flags;
        out.push(c);
    }
    out
}
