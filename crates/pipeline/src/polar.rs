//! The polar caps' terrain and shading (docs/polar.md, planned; task #601): tiles in polar
//! stereographic (roadcore::polar: EPSG:3413 north, EPSG:3031 south) past the Web Mercator tiles'
//! 85.05°, made from the elevation pool as the Mercator terrain is (crate::terrain_base: the same
//! datasets in the same order, blended alike, each pixel the mean of `K` × `K` samples), so the
//! roads and both terrains read one dataset at every point. Stored as the Mercator tiles are:
//! terrain as Terrarium in lossless WebP (crate::terrain_codec), shading as `shade-webp`
//! (crate::shade_codec) from Horn's method at the finest level and plain means of four above it
//! (crate::slope_pack's rule), its directions on the grid (east = +x, south = −y; the map turns
//! them to the ground's: roadcore::polar::east_north).
//!
//! Only a tool's today (`scenic-polar`): it writes a folder of tiles, never the NAS. Not a step yet.

use crate::dem::declared::{Plan, Source};
use crate::dem::generic::{Opts, Sampler};
use anyhow::Result;
use roadcore::polar::{self, Pole, TS};
use roadcore::shade::Px;
use std::collections::HashMap;

/// Samples a side over a pixel (crate::terrain_base's).
pub const K: usize = crate::terrain_base::K;

/// The finest zoom: 16 m pixels on the grid (~16.5 m on the ground), GLO-30's 30 m cell's half
/// rounded up, the Mercator terrain's rule for its native zoom (docs/steps.md §2, Terrain).
pub const MAX_Z: u8 = 9;

/// The plan read at polar zoom `z`: its GeoTIFFs at their overview nearest half a pixel, as the
/// Mercator base reads them (crate::terrain_base::Pool::at_zoom); the tile pyramids left out, since
/// a Web Mercator pyramid has nothing past 85.05° (the cap's ring between 85.0° and the seam is read
/// from the GeoTIFFs alone; past the seam it is never drawn).
pub fn plan_at(plan: &Plan, z: u8) -> Plan {
    let half = polar::pixel_m(z) / polar::scale(Pole::South, 90.0) / 2.0;
    let mut p = plan.clone();
    p.datasets.retain(|d| matches!(d.source, Source::Geotiff { .. }));
    for d in &mut p.datasets {
        if let Source::Geotiff { read_resolution, .. } = &mut d.source {
            *read_resolution = Some(half);
            d.resolution = d.resolution.max(half);
        }
    }
    p
}

/// Tiles `ts` of zoom `z` sampled from `s`: each its 256 × 256 heights (m, EGM2008), each the
/// mean of `K` × `K` samples; sea level where no dataset has data, and below zero to zero (the
/// Mercator terrain's rule after its water).
pub fn sample(s: &Sampler, pole: Pole, z: u8, ts: &[(u32, u32)]) -> Result<Vec<Vec<f32>>> {
    let per = TS * TS * K * K;
    let mut lon = Vec::with_capacity(per * ts.len());
    let mut lat = Vec::with_capacity(per * ts.len());
    for &(x, y) in ts {
        for j in 0..TS * K {
            for i in 0..TS * K {
                let f = |v: usize| (v as f64 + 0.5) / K as f64 - 0.5;
                let (gx, gy) = polar::pixel_xy(z, x, y, f(i), f(j));
                let (lo, la) = polar::inverse(pole, gx, gy);
                lon.push(lo);
                lat.push(la);
            }
        }
    }
    let got = s.sample(&lon, &lat, &|_| {})?;
    Ok(ts
        .iter()
        .enumerate()
        .map(|(t, _)| {
            let base = t * per;
            let mut e = vec![0f32; TS * TS];
            for (p, v) in e.iter_mut().enumerate() {
                let (pj, pi) = (p / TS, p % TS);
                let mut sum = 0f32;
                for k in 0..K * K {
                    let (si, sj) = (pi * K + k % K, pj * K + k / K);
                    let h = got.elev[base + sj * TS * K + si];
                    sum += if h.is_nan() { 0.0 } else { h };
                }
                *v = (sum / (K * K) as f32).max(0.0);
            }
            e
        })
        .collect())
}

/// The shading of tile (z, x, y) by Horn's method from the heights of its zoom's tiles `tile`
/// (past a missing neighbour, its own edge), each pixel's gradient on the grid (east = +x, south =
/// −y) per ground metre: a grid metre is 1 ÷ the scale factor at the pixel's latitude.
pub fn horn(
    tile: &dyn Fn(u32, u32) -> Option<std::sync::Arc<Vec<f32>>>,
    pole: Pole,
    z: u8,
    x: u32,
    y: u32,
) -> Option<Vec<Px>> {
    let e = tile(x, y)?;
    let n = 1u32 << z;
    let nb = |dx: i64, dy: i64| {
        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
        if nx < 0 || ny < 0 || nx >= n as i64 || ny >= n as i64 {
            return None;
        }
        tile(nx as u32, ny as u32)
    };
    let around: Vec<Vec<Option<std::sync::Arc<Vec<f32>>>>> =
        (-1..=1).map(|dy| (-1..=1).map(|dx| if dx == 0 && dy == 0 { None } else { nb(dx, dy) }).collect()).collect();
    let at = |i: i32, j: i32| -> f32 {
        let (tx, ty) = (i.div_euclid(TS as i32), j.div_euclid(TS as i32));
        let (ii, jj) = (i.rem_euclid(TS as i32) as usize, j.rem_euclid(TS as i32) as usize);
        if tx == 0 && ty == 0 {
            return e[jj * TS + ii];
        }
        match &around[(ty + 1) as usize][(tx + 1) as usize] {
            Some(t) => t[jj * TS + ii],
            None => e[(j.clamp(0, TS as i32 - 1) as usize) * TS + i.clamp(0, TS as i32 - 1) as usize],
        }
    };
    let pm = polar::pixel_m(z);
    let mut out = vec![Px::default(); TS * TS];
    for j in 0..TS as i32 {
        for i in 0..TS as i32 {
            let (gx, gy) = polar::pixel_xy(z, x, y, i as f64, j as f64);
            let (_, lat) = polar::inverse(pole, gx, gy);
            let d = (pm / polar::scale(pole, lat)) as f32;
            let (a, b, c) = (at(i - 1, j - 1), at(i, j - 1), at(i + 1, j - 1));
            let (dd, f) = (at(i - 1, j), at(i + 1, j));
            let (g, h, k) = (at(i - 1, j + 1), at(i, j + 1), at(i + 1, j + 1));
            let dzdx = ((c + 2.0 * f + k) - (a + 2.0 * dd + g)) / (8.0 * d);
            let dzdy = ((g + 2.0 * h + k) - (a + 2.0 * b + c)) / (8.0 * d);
            out[j as usize * TS + i as usize] = Px::of_gradient(dzdx, dzdy);
        }
    }
    Some(out)
}

/// A level's tiles: heights and shading by (x, y).
pub type Level = HashMap<(u32, u32), (std::sync::Arc<Vec<f32>>, Vec<Px>)>;

/// The parent level of `kids` (zoom z + 1) at zoom `z`: each tile of `tiles` the 2 × 2 means of
/// its children's heights and shading where they are, and where they aren't (past the cap) its own
/// level's: sampled by `own` and shaded by Horn's method among its level's own samples.
pub fn parents(
    pole: Pole,
    z: u8,
    tiles: &[(u32, u32)],
    kids: &Level,
    own: &dyn Fn(&[(u32, u32)]) -> Result<Vec<Vec<f32>>>,
) -> Result<Level> {
    let missing: Vec<(u32, u32)> = tiles
        .iter()
        .copied()
        .filter(|&(x, y)| (0..4).any(|k| !kids.contains_key(&(2 * x + (k & 1), 2 * y + (k >> 1)))))
        .collect();
    let sampled: HashMap<(u32, u32), std::sync::Arc<Vec<f32>>> =
        missing.iter().copied().zip(own(&missing)?.into_iter().map(std::sync::Arc::new)).collect();
    let shaded: HashMap<(u32, u32), Vec<Px>> = missing
        .iter()
        .filter_map(|&(x, y)| Some(((x, y), horn(&|a, b| sampled.get(&(a, b)).cloned(), pole, z, x, y)?)))
        .collect();
    let mut out = Level::new();
    for &(x, y) in tiles {
        let mut e = vec![0f32; TS * TS];
        let mut s = vec![Px::default(); TS * TS];
        for k in 0..4u32 {
            let (cx, cy) = (2 * x + (k & 1), 2 * y + (k >> 1));
            let (ox, oy) = ((k & 1) as usize * TS / 2, (k >> 1) as usize * TS / 2);
            match kids.get(&(cx, cy)) {
                Some((ke, ks)) => {
                    for j in 0..TS / 2 {
                        for i in 0..TS / 2 {
                            let p = |di: usize, dj: usize| (2 * j + dj) * TS + 2 * i + di;
                            let o = (oy + j) * TS + ox + i;
                            e[o] = (ke[p(0, 0)] + ke[p(1, 0)] + ke[p(0, 1)] + ke[p(1, 1)]) * 0.25;
                            s[o] = roadcore::shade::mean4(ks[p(0, 0)], ks[p(1, 0)], ks[p(0, 1)], ks[p(1, 1)]);
                        }
                    }
                }
                None => {
                    let (oe, os) = (&sampled[&(x, y)], &shaded[&(x, y)]);
                    for j in 0..TS / 2 {
                        for i in 0..TS / 2 {
                            let o = (oy + j) * TS + ox + i;
                            e[o] = oe[o];
                            s[o] = os[o];
                        }
                    }
                }
            }
        }
        out.insert((x, y), (std::sync::Arc::new(e), s));
    }
    Ok(out)
}

/// What a build made: tiles and bytes by zoom.
#[derive(Default, Debug, serde::Serialize)]
pub struct Made {
    /// (zoom, tiles, terrain bytes, shading bytes).
    pub levels: Vec<(u8, usize, u64, u64)>,
}

/// A pole's cap, zooms `max_z` to 0, made from `plan` (read through `o`) and handed to `put` as
/// (z, x, y, terrain WebP, shading WebP). Made by z`block` tiles (each one's subtree down to
/// `max_z`, with a ring of tiles at `max_z` for Horn's border), then the levels above `block` from
/// those: the memory is a block's, not the cap's. With `only`, those blocks alone, and no level
/// above them (a sample, to measure).
#[allow(clippy::too_many_arguments)]
pub fn build(
    plan: &Plan,
    o: &Opts,
    pole: Pole,
    max_z: u8,
    block: u8,
    only: Option<&[(u32, u32)]>,
    put: &(dyn Fn(u8, u32, u32, &[u8], &[u8]) -> Result<()> + Sync),
    said: &dyn Fn(&str),
) -> Result<Made> {
    let block = block.min(max_z);
    let plans: Vec<Plan> = (0..=max_z).map(|z| plan_at(plan, z)).collect();
    let first = Sampler::new(&plans[max_z as usize], o)?;
    let samplers: Vec<Sampler> =
        (0..=max_z).map(|z| Sampler::sharing(&plans[z as usize], o, &first)).collect::<Result<_>>()?;
    let mut made: HashMap<u8, (usize, u64, u64)> = HashMap::new();
    let mut store = |z: u8, lv: &Level| -> Result<()> {
        let mut keys: Vec<&(u32, u32)> = lv.keys().collect();
        keys.sort();
        for &(x, y) in keys {
            let (e, s) = &lv[&(x, y)];
            let mut h = e.to_vec();
            let q = if z == MAX_Z { crate::terrain_codec::NATIVE_M } else { crate::terrain_codec::COARSER_M };
            let t = crate::terrain_codec::encode(&mut h, q)?;
            let sh = crate::shade_codec::encode(s)?;
            put(z, x, y, &t, &sh)?;
            let m = made.entry(z).or_default();
            m.0 += 1;
            m.1 += t.len() as u64;
            m.2 += sh.len() as u64;
        }
        Ok(())
    };
    let cap_at = |z: u8| -> std::collections::BTreeSet<(u32, u32)> { polar::cap_tiles(pole, z).into_iter().collect() };
    let mut roots = Level::new();
    let blocks: Vec<(u32, u32)> =
        polar::cap_tiles(pole, block).into_iter().filter(|b| only.is_none_or(|o| o.contains(b))).collect();
    for (bi, &(bx, by)) in blocks.iter().enumerate() {
        said(&format!("block {}/{} ({block}/{bx}/{by})", bi + 1, blocks.len()));
        let d = max_z - block;
        let inside = |z: u8, x: u32, y: u32| (x >> (z - block)) == bx && (y >> (z - block)) == by;
        // The finest level: the block's cap tiles and, for Horn's border, their neighbours in the cap.
        let cap = cap_at(max_z);
        let own: Vec<(u32, u32)> = cap.iter().copied().filter(|&(x, y)| inside(max_z, x, y)).collect();
        let ring: Vec<(u32, u32)> = {
            let mut r = std::collections::BTreeSet::new();
            for &(x, y) in &own {
                for (dx, dy) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx >= 0
                        && ny >= 0
                        && cap.contains(&(nx as u32, ny as u32))
                        && !inside(max_z, nx as u32, ny as u32)
                    {
                        r.insert((nx as u32, ny as u32));
                    }
                }
            }
            r.into_iter().collect()
        };
        let all: Vec<(u32, u32)> = own.iter().chain(&ring).copied().collect();
        let mut heights: HashMap<(u32, u32), std::sync::Arc<Vec<f32>>> = HashMap::new();
        for chunk in all.chunks(16) {
            for (t, e) in chunk.iter().zip(sample(&samplers[max_z as usize], pole, max_z, chunk)?) {
                heights.insert(*t, std::sync::Arc::new(e));
            }
        }
        let mut lv = Level::new();
        for &(x, y) in &own {
            let s = horn(&|a, b| heights.get(&(a, b)).cloned(), pole, max_z, x, y).expect("its own heights");
            lv.insert((x, y), (heights[&(x, y)].clone(), s));
        }
        drop(heights);
        store(max_z, &lv)?;
        for k in 1..=d {
            let z = max_z - k;
            let ts: Vec<(u32, u32)> = cap_at(z).into_iter().filter(|&(x, y)| inside(z, x, y)).collect();
            let s = &samplers[z as usize];
            lv = parents(pole, z, &ts, &lv, &|t| sample(s, pole, z, t))?;
            store(z, &lv)?;
        }
        roots.extend(lv);
    }
    let mut lv = roots;
    for z in (0..if only.is_some() { 0 } else { block }).rev() {
        let ts = polar::cap_tiles(pole, z);
        let s = &samplers[z as usize];
        lv = parents(pole, z, &ts, &lv, &|t| sample(s, pole, z, t))?;
        store(z, &lv)?;
    }
    let mut levels: Vec<(u8, usize, u64, u64)> = made.into_iter().map(|(z, (n, t, s))| (z, n, t, s)).collect();
    levels.sort();
    Ok(Made { levels })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Horn's method on the grid gives a plane's own slope, east and south on the grid, per ground
    /// metre.
    #[test]
    fn horn_gives_a_planes_gradient() {
        let (pole, z, x, y) = (Pole::South, 6u8, 30u32, 33u32);
        // Heights rising 1 m per grid metre eastward (+x) and 0.5 m per grid metre southward (−y).
        let plane = |tx: u32, ty: u32| {
            let mut v = vec![0f32; TS * TS];
            for j in 0..TS {
                for i in 0..TS {
                    let (gx, gy) = polar::pixel_xy(z, tx, ty, i as f64, j as f64);
                    v[j * TS + i] = (gx * 1e-3 - gy * 0.5e-3) as f32 + 1000.0;
                }
            }
            std::sync::Arc::new(v)
        };
        let s = horn(&|a, b| Some(plane(a, b)), pole, z, x, y).unwrap();
        let (gx, gy) = polar::pixel_xy(z, x, y, 128.0, 128.0);
        let (_, lat) = polar::inverse(pole, gx, gy);
        let k = polar::scale(pole, lat) as f32;
        let p = s[128 * TS + 128];
        let g = (1e-3f32.hypot(0.5e-3)) * k * 100.0;
        assert!((p.s - g).abs() < 1e-3 * g, "{} vs {g}", p.s);
        let n = 1e-3f32.hypot(0.5e-3);
        assert!((p.u[0] - 1e-3 / n).abs() < 1e-3 && (p.u[1] - 0.5e-3 / n).abs() < 1e-3, "{:?}", p.u);
    }

    /// A parent is its children's means where they are, its own level's where they aren't.
    #[test]
    fn parents_mean_their_children_and_fill_the_rest() {
        let pole = Pole::North;
        let mut kids = Level::new();
        kids.insert((0, 0), (std::sync::Arc::new(vec![4.0; TS * TS]), vec![Px { s: 10.0, u: [1.0, 0.0] }; TS * TS]));
        let lv = parents(pole, 0, &[(0, 0)], &kids, &|t| Ok(t.iter().map(|_| vec![7.0; TS * TS]).collect())).unwrap();
        let (e, s) = &lv[&(0, 0)];
        assert_eq!(e[0], 4.0);
        assert_eq!(s[0].s, 10.0);
        assert_eq!(e[TS * TS - 1], 7.0);
        assert_eq!(s[TS * TS - 1].s, 0.0);
    }
}
