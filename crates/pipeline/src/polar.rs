//! The polar caps' terrain and shading (docs/polar.md; task #601): tiles in polar stereographic
//! (roadcore::polar: EPSG:3413 north, EPSG:3031 south) past the Web Mercator tiles' 85.05°, made
//! from the elevation pool as the Mercator terrain is (crate::terrain_base: the same datasets in the
//! same order, blended alike, each pixel the mean of `K` × `K` samples), so the roads and both
//! terrains read one dataset at every point. Stored as the Mercator tiles are: terrain as Terrarium
//! in lossless WebP (crate::terrain_codec), shading as `shade-webp` (crate::shade_codec) from
//! Horn's method at the finest level and plain means of four above it (crate::slope_pack's rule),
//! its directions on the grid (east = +x, south = −y; a renderer turns them to the ground's:
//! roadcore::polar::east_north). Nothing here is the globe's: the tiles are the grid's, for any
//! projection that draws them.
//!
//! The steps (crate::agent::build's polar chain): `polar-terrain` makes a z`BLOCK_Z` block's
//! terrain, z4–`MAX_Z` (`terrain_block`); `polar-slope` its shading from the terrain packs of it
//! and its neighbours (`slope_block`); `polar-root` z0–3 of both from the blocks' z4 tiles
//! (`root`). `scenic-polar` runs the same into a folder.

use crate::dem::declared::{Plan, Source};
use crate::dem::generic::Sampler;
use anyhow::Result;
use roadcore::polar::{self, Pole, TS};
use roadcore::shade::Px;
use std::collections::HashMap;
use std::sync::Arc;

/// The finest zoom: 16 m pixels on the grid (~16.5 m on the ground), GLO-30's 30 m cell's half
/// rounded up, the Mercator terrain's rule for its native zoom (docs/steps.md §2, Terrain). The
/// owner's (25 Oct).
pub const MAX_Z: u8 = 9;

/// The zoom of a block: a `polar-terrain` and `polar-slope` target, its subtree z4–`MAX_Z` one
/// pack (131 km a side; 76 make the south cap).
pub const BLOCK_Z: u8 = 4;

/// The poles built: the south. The north cap has no land (sea level everywhere north of 85° N,
/// GLO-30 having nothing there), so it waits for the depth (docs/polar.md §2).
pub const POLES: [Pole; 1] = [Pole::South];

/// The steps' versions (in their keys).
pub const TERRAIN_V: u32 = 1;
pub const SLOPE_V: u32 = 1;
pub const ROOT_V: u32 = 1;

/// A polar layer's name: `polar-terrain-s`, `polar-slope-s` (`kind` "terrain" or "slope").
pub fn layer(kind: &str, pole: Pole) -> String {
    format!("polar-{kind}-{}", pole.letter())
}

/// A block's pack: `layers/polar-terrain-s/polar/4-x-y` (its key in the catalog's layer `polar`).
pub fn block_logical(kind: &str, pole: Pole, b: (u32, u32)) -> String {
    format!("layers/{}/polar/{BLOCK_Z}-{}-{}", layer(kind, pole), b.0, b.1)
}

/// A pole's root pack (z0–3).
pub fn root_logical(kind: &str, pole: Pole) -> String {
    format!("layers/{}/root/0-0-0", layer(kind, pole))
}

/// A block's target name: `s/4/x/y`.
pub fn target(pole: Pole, b: (u32, u32)) -> String {
    format!("{}/{BLOCK_Z}/{}/{}", pole.letter(), b.0, b.1)
}

/// A pole's root target name: `s`.
pub fn root_target(pole: Pole) -> String {
    pole.letter().to_string()
}

/// The block a target names.
pub fn parse_target(t: &str) -> Option<(Pole, (u32, u32))> {
    let mut it = t.split('/');
    let pole = Pole::of_letter(it.next()?)?;
    if it.next()? != BLOCK_Z.to_string() {
        return None;
    }
    let (x, y) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    let n = 1u32 << BLOCK_Z;
    (it.next().is_none() && x < n && y < n).then_some((pole, (x, y)))
}

/// A pole's blocks: the z`BLOCK_Z` tiles meeting its cap.
pub fn blocks(pole: Pole) -> Vec<(u32, u32)> {
    polar::cap_tiles(pole, BLOCK_Z)
}

/// A pole's blocks the coverage builds: those whose latitudes it meets (`roadcore::polar::lat_box`).
pub fn built(cov: &crate::coverage::Coverage, pole: Pole) -> Vec<(u32, u32)> {
    let e7 = |b: [f64; 4]| b.map(|v| (v * 1e7).round() as i32);
    blocks(pole).into_iter().filter(|&(x, y)| cov.meets_rect(e7(polar::lat_box(pole, BLOCK_Z, x, y)))).collect()
}

/// The block's eight neighbours of the cap (and itself first): what its shading reads.
pub fn around(pole: Pole, b: (u32, u32)) -> Vec<(u32, u32)> {
    let cap: std::collections::BTreeSet<(u32, u32)> = blocks(pole).into_iter().collect();
    let mut out = vec![b];
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (x, y) = (b.0 as i64 + dx, b.1 as i64 + dy);
            if (dx, dy) != (0, 0) && x >= 0 && y >= 0 && cap.contains(&(x as u32, y as u32)) {
                out.push((x as u32, y as u32));
            }
        }
    }
    out
}

/// A tile's terrain as stored (crate::terrain_codec): 1 cm at `MAX_Z`, 10 cm coarser.
pub fn encode_terrain(z: u8, e: &[f32]) -> Result<Vec<u8>> {
    let mut h = e.to_vec();
    let q = if z == MAX_Z { crate::terrain_codec::NATIVE_M } else { crate::terrain_codec::COARSER_M };
    crate::terrain_codec::encode(&mut h, q)
}

/// Samples a side over a pixel (crate::terrain_base's).
pub const K: usize = crate::terrain_base::K;

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

/// Where the pool leaves holes in the cap, by latitude band (the degree's floor): (band, samples
/// no dataset had data for, those of them filled from a fill mask's void (`Sampler::fill_voids`),
/// samples). GLO-30 has voids near the South Pole (its FLM's fills from other DEMs) and nothing lies
/// beneath it inside REMA's own hole round the pole (docs/polar.md §2).
#[derive(Default)]
pub struct Voids(std::sync::Mutex<std::collections::BTreeMap<i32, [u64; 3]>>);

impl Voids {
    pub fn bands(&self) -> Vec<(i32, u64, u64, u64)> {
        self.0.lock().unwrap().iter().map(|(&b, &[v, f, n])| (b, v, f, n)).collect()
    }

    /// (samples with no data, those filled from voids, samples) over every band.
    pub fn total(&self) -> (u64, u64, u64) {
        self.bands().iter().fold((0, 0, 0), |a, b| (a.0 + b.1, a.1 + b.2, a.2 + b.3))
    }
}

/// Tiles `ts` of zoom `z` sampled from `s`: each its 256 × 256 heights (m, EGM2008), each the
/// mean of `K` × `K` samples; where no dataset has data, a fill mask's own filled value
/// (`Sampler::fill_voids`), else sea level (both counted in `voids`); below zero to zero (the
/// Mercator terrain's rule after its water).
pub fn sample(s: &Sampler, pole: Pole, z: u8, ts: &[(u32, u32)], voids: &Voids) -> Result<Vec<Vec<f32>>> {
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
    let mut got = s.sample(&lon, &lat, &|_| {})?;
    let gaps: Vec<bool> = got.elev.iter().map(|h| h.is_nan()).collect();
    s.fill_voids(&lon, &lat, &mut got)?;
    {
        let mut v = voids.0.lock().unwrap();
        for ((h, la), gap) in got.elev.iter().zip(&lat).zip(gaps) {
            let b = v.entry(la.floor() as i32).or_default();
            b[0] += u64::from(gap);
            b[1] += u64::from(gap && !h.is_nan());
            b[2] += 1;
        }
    }
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

/// Tiles by (x, y) of one zoom.
pub type Heights = HashMap<(u32, u32), Arc<Vec<f32>>>;

/// The parent heights of `kids` (a zoom finer): each of `tiles` its children's 2 × 2
/// means where they are, and where they aren't (past the cap) its own level's, sampled by `own`.
pub fn parent_heights(
    tiles: &[(u32, u32)],
    kids: &Heights,
    own: &dyn Fn(&[(u32, u32)]) -> Result<Vec<Vec<f32>>>,
) -> Result<Heights> {
    let missing: Vec<(u32, u32)> =
        tiles.iter().copied().filter(|&t| !all_kids(t, &|c| kids.contains_key(&c))).collect();
    let sampled: Heights = missing.iter().copied().zip(own(&missing)?.into_iter().map(Arc::new)).collect();
    let mut out = Heights::new();
    for &(x, y) in tiles {
        let mut e = vec![0f32; TS * TS];
        for k in 0..4u32 {
            let (cx, cy) = (2 * x + (k & 1), 2 * y + (k >> 1));
            let (ox, oy) = ((k & 1) as usize * TS / 2, (k >> 1) as usize * TS / 2);
            for j in 0..TS / 2 {
                for i in 0..TS / 2 {
                    let o = (oy + j) * TS + ox + i;
                    e[o] = match kids.get(&(cx, cy)) {
                        Some(ke) => {
                            let p = |di: usize, dj: usize| (2 * j + dj) * TS + 2 * i + di;
                            (ke[p(0, 0)] + ke[p(1, 0)] + ke[p(0, 1)] + ke[p(1, 1)]) * 0.25
                        }
                        None => sampled[&(x, y)][o],
                    };
                }
            }
        }
        out.insert((x, y), Arc::new(e));
    }
    Ok(out)
}

fn all_kids(t: (u32, u32), has: &dyn Fn((u32, u32)) -> bool) -> bool {
    (0..4).all(|k| has((2 * t.0 + (k & 1), 2 * t.1 + (k >> 1))))
}

/// The parent shading at zoom `z` of `kids`: each of `tiles` its children's plain means where they
/// are, and where they aren't, Horn's method on its own level's terrain (`terrain`, with its
/// neighbours).
pub fn parent_shading(
    pole: Pole,
    z: u8,
    tiles: &[(u32, u32)],
    kids: &HashMap<(u32, u32), Vec<Px>>,
    terrain: &dyn Fn(u32, u32) -> Option<Arc<Vec<f32>>>,
) -> HashMap<(u32, u32), Vec<Px>> {
    let mut out = HashMap::new();
    for &(x, y) in tiles {
        let own = (!all_kids((x, y), &|c| kids.contains_key(&c))).then(|| horn(terrain, pole, z, x, y)).flatten();
        let mut s = vec![Px::default(); TS * TS];
        for k in 0..4u32 {
            let (cx, cy) = (2 * x + (k & 1), 2 * y + (k >> 1));
            let (ox, oy) = ((k & 1) as usize * TS / 2, (k >> 1) as usize * TS / 2);
            for j in 0..TS / 2 {
                for i in 0..TS / 2 {
                    let o = (oy + j) * TS + ox + i;
                    s[o] = match (kids.get(&(cx, cy)), &own) {
                        (Some(ks), _) => {
                            let p = |di: usize, dj: usize| (2 * j + dj) * TS + 2 * i + di;
                            roadcore::shade::mean4(ks[p(0, 0)], ks[p(1, 0)], ks[p(0, 1)], ks[p(1, 1)])
                        }
                        (None, Some(os)) => os[o],
                        (None, None) => Px::default(),
                    };
                }
            }
        }
        out.insert((x, y), s);
    }
    out
}

/// The plans a pole's levels read, z0–`max_z` (`plan_at` each), for `samplers`.
pub fn plans(plan: &Plan, max_z: u8) -> Vec<Plan> {
    (0..=max_z).map(|z| plan_at(plan, z)).collect()
}

/// The levels' samplers of `plans` (`plans`), sharing one cache.
pub fn samplers<'a>(plans: &'a [Plan], o: &'a crate::dem::generic::Opts<'a>) -> Result<Vec<Sampler<'a>>> {
    let first = Sampler::new(plans.last().expect("a level"), o)?;
    plans.iter().map(|p| Sampler::sharing(p, o, &first)).collect()
}

/// A block's terrain, z`BLOCK_Z`–`max_z`, handed to `put` as (z, x, y, Terrarium WebP) finest
/// first: its cap tiles at `max_z` sampled, each coarser level its children's means (sampled where
/// they aren't, past the cap). `samplers` from `samplers`.
pub fn terrain_block(
    samplers: &[Sampler],
    pole: Pole,
    b: (u32, u32),
    max_z: u8,
    voids: &Voids,
    put: &mut dyn FnMut(u8, u32, u32, Vec<u8>) -> Result<()>,
) -> Result<()> {
    let inside = |z: u8, x: u32, y: u32| (x >> (z - BLOCK_Z)) == b.0 && (y >> (z - BLOCK_Z)) == b.1;
    let level = |z: u8| -> Vec<(u32, u32)> {
        polar::cap_tiles(pole, z).into_iter().filter(|&(x, y)| inside(z, x, y)).collect()
    };
    let own = level(max_z);
    let mut lv = Heights::new();
    for chunk in own.chunks(16) {
        for (t, e) in chunk.iter().zip(sample(&samplers[max_z as usize], pole, max_z, chunk, voids)?) {
            lv.insert(*t, Arc::new(e));
        }
    }
    let mut z = max_z;
    loop {
        let mut keys: Vec<&(u32, u32)> = lv.keys().collect();
        keys.sort();
        for &(x, y) in keys {
            put(z, x, y, encode_terrain(z, &lv[&(x, y)])?)?;
        }
        if z == BLOCK_Z {
            return Ok(());
        }
        z -= 1;
        let s = &samplers[z as usize];
        lv = parent_heights(&level(z), &lv, &|t| sample(s, pole, z, t, voids))?;
    }
}

/// A block's shading, z`BLOCK_Z`–`max_z`, handed to `put` as (z, x, y, shade WebP): at `max_z`
/// Horn's method on the stored terrain (`terrain`: any tile of the cap, its neighbours' blocks'
/// too), each coarser level its children's means at full precision (Horn's on its own terrain
/// where they aren't).
pub fn slope_block(
    pole: Pole,
    b: (u32, u32),
    max_z: u8,
    terrain: &dyn Fn(u8, u32, u32) -> Result<Option<Arc<Vec<f32>>>>,
    put: &mut dyn FnMut(u8, u32, u32, Vec<u8>) -> Result<()>,
) -> Result<()> {
    let inside = |z: u8, x: u32, y: u32| (x >> (z - BLOCK_Z)) == b.0 && (y >> (z - BLOCK_Z)) == b.1;
    let level = |z: u8| -> Vec<(u32, u32)> {
        polar::cap_tiles(pole, z).into_iter().filter(|&(x, y)| inside(z, x, y)).collect()
    };
    // (A level's terrain: its tiles and their neighbours in the cap, read once.)
    let read = |z: u8, ts: &[(u32, u32)]| -> Result<Heights> {
        let cap: std::collections::BTreeSet<(u32, u32)> = polar::cap_tiles(pole, z).into_iter().collect();
        let mut want = std::collections::BTreeSet::new();
        for &(x, y) in ts {
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx >= 0 && ny >= 0 && cap.contains(&(nx as u32, ny as u32)) {
                        want.insert((nx as u32, ny as u32));
                    }
                }
            }
        }
        let mut out = Heights::new();
        for (x, y) in want {
            if let Some(t) = terrain(z, x, y)? {
                out.insert((x, y), t);
            }
        }
        Ok(out)
    };
    let own = level(max_z);
    let hs = read(max_z, &own)?;
    let mut kids: HashMap<(u32, u32), Vec<Px>> = own
        .iter()
        .filter_map(|&(x, y)| Some(((x, y), horn(&|a, c| hs.get(&(a, c)).cloned(), pole, max_z, x, y)?)))
        .collect();
    drop(hs);
    let mut z = max_z;
    loop {
        let mut keys: Vec<&(u32, u32)> = kids.keys().collect();
        keys.sort();
        for &(x, y) in keys {
            put(z, x, y, crate::shade_codec::encode(&kids[&(x, y)])?)?;
        }
        if z == BLOCK_Z {
            return Ok(());
        }
        z -= 1;
        let ts = level(z);
        // (Only the tiles some of whose children aren't there need their own terrain.)
        let lacking: Vec<(u32, u32)> =
            ts.iter().copied().filter(|&t| !all_kids(t, &|c| kids.contains_key(&c))).collect();
        let hs = read(z, &lacking)?;
        kids = parent_shading(pole, z, &ts, &kids, &|a, c| hs.get(&(a, c)).cloned());
    }
}

/// A pole's root, z0–`BLOCK_Z` − 1, of both layers, handed to `put` as (z, x, y, terrain WebP,
/// shade WebP): the terrain from the blocks' z`BLOCK_Z` terrain as stored (`terrain`), the
/// shading from their shading as stored (`shade`), each level its children's means, and where they
/// aren't (past the cap) the level's own: sampled, and shaded by Horn's method on it.
pub fn root(
    samplers: &[Sampler],
    pole: Pole,
    terrain: &dyn Fn(u32, u32) -> Result<Option<Vec<f32>>>,
    shade: &dyn Fn(u32, u32) -> Result<Option<Vec<Px>>>,
    voids: &Voids,
    put: &mut dyn FnMut(u8, u32, u32, Vec<u8>, Vec<u8>) -> Result<()>,
) -> Result<()> {
    let mut hs = Heights::new();
    let mut ss: HashMap<(u32, u32), Vec<Px>> = HashMap::new();
    for t in blocks(pole) {
        if let Some(e) = terrain(t.0, t.1)? {
            hs.insert(t, Arc::new(e));
        }
        if let Some(s) = shade(t.0, t.1)? {
            ss.insert(t, s);
        }
    }
    for z in (0..BLOCK_Z).rev() {
        let ts = polar::cap_tiles(pole, z);
        let s = &samplers[z as usize];
        hs = parent_heights(&ts, &hs, &|t| sample(s, pole, z, t, voids))?;
        ss = parent_shading(pole, z, &ts, &ss, &|x, y| hs.get(&(x, y)).cloned());
        for &(x, y) in &ts {
            put(z, x, y, encode_terrain(z, &hs[&(x, y)])?, crate::shade_codec::encode(&ss[&(x, y)])?)?;
        }
    }
    Ok(())
}

// ---- The steps (crate::agent::build's polar chain) ----------------------------------------------

/// The pool over latitudes `b` (crate::terrain_base::Pool), its levels' plans read at `max_z`.
fn pool_plans(out: &crate::out::Out, b: [f64; 4]) -> Result<(Vec<Plan>, std::path::PathBuf)> {
    let pool = crate::terrain_base::Pool::read(out.root(), &out.manifest, b)?;
    Ok((plans(&pool.plan, MAX_Z), pool.store.clone()))
}

/// A pack of the build's, opened: its index and its file (the NAS's; whole in memory with `whole`).
struct Pack {
    idx: store::pack::PackIndex,
    src: Box<dyn store::range::RangeRead>,
}

impl Pack {
    fn open(out: &crate::out::Out, logical: &str, whole: bool) -> Result<Option<Pack>> {
        use anyhow::Context;
        let Some(c) = out.get(logical) else { return Ok(None) };
        let p = out.path(c);
        let src: Box<dyn store::range::RangeRead> = if whole {
            Box::new(std::fs::read(&p).with_context(|| format!("read {}", p.display()))?)
        } else {
            Box::new(store::range::PlainFile::open(&p).with_context(|| format!("open {}", p.display()))?)
        };
        let idx = store::pack::PackIndex::read_from(&*src).with_context(|| format!("read {logical}'s index"))?;
        Ok(Some(Pack { idx, src }))
    }

    fn get(&self, z: u8, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
        Ok(self.idx.get(&*self.src, z, x, y)?.map(|(_, b)| b))
    }
}

/// Writes a polar layer's pack (crate::layers::write_pack: `layers/<layer>/<scope>/<z>-<x>-<y>`).
fn write(
    out: &mut crate::out::Out,
    kind: &str,
    pole: Pole,
    scope: &str,
    root: (u8, u32, u32),
    tiles: Vec<(u8, u32, u32, Vec<u8>)>,
) -> Result<()> {
    let enc = if kind == "terrain" { crate::terrain_codec::ENCODING } else { crate::shade_codec::ENCODING };
    let mut it = tiles.into_iter().map(|(z, x, y, b)| {
        let n = b.len() as u32;
        (z, x, y, b, n)
    });
    crate::layers::write_pack(out, &layer(kind, pole), enc, false, scope, root, &mut it)?;
    Ok(())
}

/// `polar-terrain`: each block's terrain pack (`terrain_block`), from the pool over its latitudes.
pub fn terrain_step(out: &mut crate::out::Out, targets: &[(Pole, (u32, u32))]) -> Result<()> {
    let net = crate::fetch::Fetcher::from_env();
    for (k, &(pole, b)) in targets.iter().enumerate() {
        let t = target(pole, b);
        crate::control::safe_point("polar-terrain");
        crate::agent::jobs::report(k as u64, targets.len() as u64, "polar blocks");
        let (plans, store) = pool_plans(out, polar::lat_box(pole, BLOCK_Z, b.0, b.1))?;
        let o = crate::dem::generic::Opts { fetch: &net, store: Some(store) };
        let ss = samplers(&plans, &o)?;
        let voids = Voids::default();
        let mut tiles = Vec::new();
        terrain_block(&ss, pole, b, MAX_Z, &voids, &mut |z, x, y, t| {
            tiles.push((z, x, y, t));
            Ok(())
        })?;
        let (v, f, n) = voids.total();
        eprintln!(
            "polar-terrain {t}: {} tiles; of {n} samples {v} with no data, {f} of them filled from voids",
            tiles.len()
        );
        write(out, "terrain", pole, "polar", (BLOCK_Z, b.0, b.1), tiles)?;
        crate::control::done("polar-terrain", &t);
    }
    crate::agent::jobs::report(targets.len() as u64, targets.len() as u64, "polar blocks");
    Ok(())
}

/// `polar-slope`: each block's shading pack (`slope_block`), from the terrain packs of it (read
/// whole) and its neighbours (their tiles read as wanted).
pub fn slope_step(out: &mut crate::out::Out, targets: &[(Pole, (u32, u32))]) -> Result<()> {
    for (k, &(pole, b)) in targets.iter().enumerate() {
        let t = target(pole, b);
        crate::control::safe_point("polar-slope");
        crate::agent::jobs::report(k as u64, targets.len() as u64, "polar blocks");
        let mut packs: HashMap<(u32, u32), Pack> = HashMap::new();
        for n in around(pole, b) {
            if let Some(p) = Pack::open(out, &block_logical("terrain", pole, n), n == b)? {
                packs.insert(n, p);
            }
        }
        anyhow::ensure!(packs.contains_key(&b), "{t}: no terrain pack ({})", block_logical("terrain", pole, b));
        let terrain = |z: u8, x: u32, y: u32| -> Result<Option<Arc<Vec<f32>>>> {
            let Some(p) = packs.get(&(x >> (z - BLOCK_Z), y >> (z - BLOCK_Z))) else { return Ok(None) };
            p.get(z, x, y)?.map(|b| roadcore::grid::decode_terrain(&b).map(Arc::new)).transpose()
        };
        let mut tiles = Vec::new();
        slope_block(pole, b, MAX_Z, &terrain, &mut |z, x, y, s| {
            tiles.push((z, x, y, s));
            Ok(())
        })?;
        eprintln!("polar-slope {t}: {} tiles", tiles.len());
        write(out, "slope", pole, "polar", (BLOCK_Z, b.0, b.1), tiles)?;
        crate::control::done("polar-slope", &t);
    }
    crate::agent::jobs::report(targets.len() as u64, targets.len() as u64, "polar blocks");
    Ok(())
}

/// `polar-root`: a pole's root packs of both layers (`root`) from its blocks' packs (`blocks`, the
/// targets), the pool over the root's latitudes sampled past the cap; the packs of blocks no
/// longer built removed.
pub fn root_step(out: &mut crate::out::Out, pole: Pole, built: &[(u32, u32)]) -> Result<()> {
    let net = crate::fetch::Fetcher::from_env();
    let (plans, store) = pool_plans(out, polar::lat_box(pole, 0, 0, 0))?;
    let o = crate::dem::generic::Opts { fetch: &net, store: Some(store) };
    let ss = samplers(&plans[..BLOCK_Z as usize], &o)?;
    let open = |kind: &str| -> Result<HashMap<(u32, u32), Pack>> {
        let mut m = HashMap::new();
        for &b in built {
            if let Some(p) = Pack::open(out, &block_logical(kind, pole, b), false)? {
                m.insert(b, p);
            }
        }
        Ok(m)
    };
    let (tp, sp) = (open("terrain")?, open("slope")?);
    let get = |m: &HashMap<(u32, u32), Pack>, x: u32, y: u32| -> Result<Option<Vec<u8>>> {
        m.get(&(x, y)).map_or(Ok(None), |p| p.get(BLOCK_Z, x, y))
    };
    let (mut ts, mut shs) = (Vec::new(), Vec::new());
    let voids = Voids::default();
    root(
        &ss,
        pole,
        &|x, y| get(&tp, x, y)?.map(|b| roadcore::grid::decode_terrain(&b)).transpose(),
        &|x, y| get(&sp, x, y)?.map(|b| roadcore::shade::decode(&b)).transpose(),
        &voids,
        &mut |z, x, y, t, s| {
            ts.push((z, x, y, t));
            shs.push((z, x, y, s));
            Ok(())
        },
    )?;
    drop((tp, sp));
    eprintln!("polar-root {}: {} tiles of each layer, from {} blocks", pole.letter(), ts.len(), built.len());
    write(out, "terrain", pole, "root", (0, 0, 0), ts)?;
    write(out, "slope", pole, "root", (0, 0, 0), shs)?;
    for kind in ["terrain", "slope"] {
        let pre = format!("layers/{}/polar/", layer(kind, pole));
        let gone: Vec<String> = out
            .manifest
            .range(pre.clone()..)
            .take_while(|(l, _)| l.starts_with(&pre))
            .map(|(l, _)| l.clone())
            .filter(|l| !built.iter().any(|&b| *l == block_logical(kind, pole, b)))
            .collect();
        for l in gone {
            out.remove(&l);
        }
    }
    crate::control::done("polar-root", &root_target(pole));
    Ok(())
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

    /// A parent is its children's means where they are, its own level's where they aren't: heights
    /// sampled, shading by Horn's method on them (a flat level: none).
    #[test]
    fn parents_mean_their_children_and_fill_the_rest() {
        let mut kids = Heights::new();
        kids.insert((0, 0), Arc::new(vec![4.0; TS * TS]));
        let e = parent_heights(&[(0, 0)], &kids, &|t| Ok(t.iter().map(|_| vec![7.0; TS * TS]).collect())).unwrap();
        assert_eq!(e[&(0, 0)][0], 4.0);
        assert_eq!(e[&(0, 0)][TS * TS - 1], 7.0);
        let mut ks = HashMap::new();
        ks.insert((0, 0), vec![Px { s: 10.0, u: [1.0, 0.0] }; TS * TS]);
        let s = parent_shading(Pole::North, 0, &[(0, 0)], &ks, &|x, y| e.get(&(x, y)).cloned());
        assert_eq!(s[&(0, 0)][0].s, 10.0);
        assert_eq!(s[&(0, 0)][TS * TS - 1].s, 0.0);
    }

    /// Where no dataset has data but a fill mask's void (GLO-30's filled pixels with nothing
    /// beneath), the cap takes the dataset's own filled value (`Sampler::fill_voids`); elsewhere the
    /// pool's as sampled.
    #[test]
    fn voids_with_nothing_beneath_take_their_own_filled_values() {
        use crate::dem::declared::{Dataset, Source};
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("fetched");
        std::fs::create_dir_all(store.join("glo")).unwrap();
        // (1/200° pixels over 10–11°E, 45–46°N: 100 m plus 1 m a pixel east; the mask 3 in its west
        // half.)
        let n = 200usize;
        let gt = [10.0, 0.005, 0.0, 46.0, 0.0, -0.005];
        let e: Vec<f32> = (0..n * n).map(|p| 100.0 + (p % n) as f32).collect();
        let m: Vec<f32> = (0..n * n).map(|p| if p % n < 100 { 3.0 } else { 1.0 }).collect();
        let tif = |v: &[f32]| crate::geotiff::write_f32(n as u32, n as u32, v, 256, gt, 4326, None).unwrap();
        std::fs::write(store.join("glo/T_DEM.tif"), tif(&e)).unwrap();
        std::fs::write(store.join("glo/T_FLM.tif"), tif(&m)).unwrap();
        let plan = Plan {
            fmt: 1,
            version: "test".into(),
            tag: "t".into(),
            datasets: vec![Dataset {
                id: "glo".into(),
                code: 3,
                resolution: 400.0,
                cover: Some(vec![[10.0, 45.0, 11.0, 46.0]]),
                source: Source::Geotiff {
                    crs: 4326,
                    read_resolution: None,
                    store: true,
                    files: vec![("https://e.org/T/T_DEM.tif".into(), Some([10.0, 45.0, 11.0, 46.0]))],
                    masks: vec![Some("https://e.org/T/T_FLM.tif".into())],
                },
                convert: None,
            }],
        };
        crate::fetch::go_offline();
        let net = crate::fetch::Fetcher::from_env();
        let o = crate::dem::generic::Opts { fetch: &net, store: Some(store) };
        let s = Sampler::new(&plan, &o).unwrap();
        let (lon, lat) = ([10.2525, 10.7525, 12.0], [45.5, 45.5, 45.5]);
        let mut got = s.sample(&lon, &lat, &|_| {}).unwrap();
        assert!(got.elev[0].is_nan() && (got.elev[1] - 250.0).abs() < 1.0, "{:?}", got.elev);
        assert_eq!(s.fill_voids(&lon, &lat, &mut got).unwrap(), 1);
        assert!((got.elev[0] - 150.0).abs() < 1.0 && got.src[0] == 3, "{:?}", got.elev);
        assert!(got.elev[2].is_nan(), "outside every dataset: none");
    }

    /// Targets name a block and its pole, and only that.
    #[test]
    fn targets_round_trip() {
        let b = blocks(Pole::South)[3];
        assert_eq!(parse_target(&target(Pole::South, b)), Some((Pole::South, b)));
        assert_eq!(parse_target("s/5/1/1"), None);
        assert_eq!(parse_target("s/4/16/1"), None);
        assert_eq!(block_logical("terrain", Pole::South, (7, 8)), "layers/polar-terrain-s/polar/4-7-8");
        assert_eq!(blocks(Pole::South).len(), 76);
        assert_eq!(around(Pole::South, (7, 8)).len(), 9);
    }
}
