//! A block of depth tiles composed from the depth sources (docs/plan.md §6, Depth): up to 8 × 8
//! tiles of one zoom with a margin of `PAD` pixels, on the basemap's own water as the terrain
//! flattens it (crate::terrain_water).
//!
//! 1. Each source is sampled onto the block (crate::depth::grids): depth, the zoom each pixel's data
//!    supports, its chart low water. Sea sources stay off the lakes, lake sources in them.
//! 2. Sources are painted coarse to fine. A pixel past its source's zoom shows the stored tiles of
//!    that zoom scaled up, as the map's fallback makes them (crate::depth::fall); inside a source,
//!    detail fades in over three of the capped zoom's pixels where measured cells meet capped ones;
//!    where a source ends in the water it's feathered into what lies beneath over its band (never
//!    across a source whose datum isn't corrected to the others', which only fills gaps).
//! 3. The water decides the shore: positive values stay as drying heights only where an intertidal
//!    source gives them (else 0, and at most `DRY_MAX`); water with no data near data is filled,
//!    falling linearly to 0 at the shore, from data of the same water body within `GAP_M`; the land
//!    keeps a `BLEED_PX` band of the nearest water's value (≥ 0) so the map's finer shoreline never
//!    meets a gap; a lake with no data of its own stays empty.
//! 4. A pixel above its chart's low water (an intertidal source's, in the sea) dries at low tide.
//! 5. A tile is kept when one of its water pixels has data of its own zoom (at `ALL_UPTO` and
//!    coarser: any data).

use super::edt::nearest;
use super::grids::{px, sample, Reader, Source};
use super::{C_FILLED, C_INTERP, C_NONE, DRIES, NODATA, NODATA_MIN};
use anyhow::Result;

/// The block's margin (pixels).
pub const PAD: usize = 32;
/// A block's tiles a side.
pub const BLOCK: u32 = 8;
/// At and below this zoom every tile with data in its water is kept.
pub const ALL_UPTO: u8 = 8;
/// The land's band of the nearest water's value (pixels).
pub const BLEED_PX: f32 = 3.0;
/// Drying heights above this (m) are land.
pub const DRY_MAX: f32 = 20.0;
/// How far water without data is filled from data (ground metres).
pub const GAP_M: f32 = 600.0;

/// The block's water: whether each pixel is water (at least half of it), and its lake (0: the sea;
/// else the block's own number for the lake covering most of it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Water {
    pub water: Vec<bool>,
    pub lake: Vec<u32>,
}

/// A block's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub z: u8,
    pub tx0: u32,
    pub ty0: u32,
    pub nx: u32,
    pub ny: u32,
}

impl Block {
    pub fn w(&self) -> usize {
        self.nx as usize * 256 + 2 * PAD
    }
    pub fn h(&self) -> usize {
        self.ny as usize * 256 + 2 * PAD
    }
    /// Its first pixel (the margin's) on the zoom's global grid.
    pub fn origin(&self) -> (i64, i64) {
        (i64::from(self.tx0) * 256 - PAD as i64, i64::from(self.ty0) * 256 - PAD as i64)
    }
    /// The latitude of its middle.
    pub fn mid_lat(&self) -> f64 {
        let (c, r) = self.origin();
        super::grids::lonlat(self.z, (c + self.w() as i64 / 2) as f64, (r + self.h() as i64 / 2) as f64).1
    }
}

/// A block composed: depth (NODATA: none), class bytes, and the zoom each pixel's data supports.
pub struct Composed {
    pub depth: Vec<f32>,
    pub class: Vec<u8>,
    pub zoom: Vec<i8>,
}

/// Composes block `b` from `sources` over `water`; `up(zv)` is the stored level `zv` scaled up onto
/// the block (NODATA: none), `zmin` the layer's coarsest zoom. None when no source has data of the
/// block's zoom there (at `ALL_UPTO` and coarser: any data) or it has no water.
pub fn compose(
    b: Block,
    sources: &[Source],
    rd: &Reader,
    water: &Water,
    up: &mut dyn FnMut(u8) -> Result<Vec<f32>>,
    zmin: u8,
) -> Result<Option<Composed>> {
    let (w, h) = (b.w(), b.h());
    let n = w * h;
    let (c0, r0) = b.origin();
    let bbox = super::grids::window_box(b.z, c0, r0, w, h);
    let need = if b.z > ALL_UPTO { b.z } else { zmin };
    let here: Vec<(&Source, u8)> = sources.iter().filter_map(|s| s.max_zoom_in(bbox).map(|z| (s, z))).collect();
    if !here.iter().any(|(_, z)| *z >= need) {
        return Ok(None);
    }
    let inner = |i: usize| {
        let (x, y) = (i % w, i / w);
        (PAD..w - PAD).contains(&x) && (PAD..h - PAD).contains(&y)
    };
    if !(0..n).any(|i| water.water[i] && inner(i)) {
        return Ok(None);
    }
    let gpx = (px(b.z) * b.mid_lat().to_radians().cos()) as f32;
    let mut c = vec![f32::NAN; n];
    let mut zz = vec![-1i8; n];
    let mut k = vec![0u8; n];
    let mut src: Vec<i16> = vec![-1; n];
    let mut inter = vec![false; n];
    let mut dok = vec![true; n];
    let mut low = vec![f32::NAN; n];
    let mut order = here.clone();
    order.sort_by_key(|(s, z)| (*z, s.code));
    let mut ups: std::collections::HashMap<u8, Vec<f32>> = std::collections::HashMap::new();
    let mut get_up = |zv: u8, ups: &mut std::collections::HashMap<u8, Vec<f32>>| -> Result<()> {
        if !ups.contains_key(&zv) {
            let v = if zv < zmin { vec![NODATA; n] } else { up(zv)? };
            ups.insert(zv, v);
        }
        Ok(())
    };
    for (s, _) in &order {
        let Some(mut smp) = sample(rd, s, b.z, c0, r0, w, h)? else { continue };
        for i in 0..n {
            if (water.lake[i] > 0) != s.lake {
                smp.v[i] = f32::NAN;
            }
        }
        let valid: Vec<bool> = smp.v.iter().map(|v| v.is_finite()).collect();
        if !valid.iter().any(|&v| v) {
            continue;
        }
        let zs: Vec<u8> = smp.zoom.iter().map(|&z| z.max(zmin)).collect();
        let native: Vec<bool> = (0..n).map(|i| valid[i] && zs[i] >= b.z).collect();
        let capped: Vec<bool> = (0..n).map(|i| valid[i] && zs[i] < b.z).collect();
        let mut disp: Vec<f32> = (0..n).map(|i| if native[i] { smp.v[i] } else { f32::NAN }).collect();
        let mut capped_zooms: Vec<u8> = (0..n).filter(|&i| capped[i]).map(|i| zs[i]).collect();
        capped_zooms.sort_unstable();
        let median = capped_zooms.get(capped_zooms.len() / 2).copied();
        capped_zooms.dedup();
        for &zv in &capped_zooms {
            get_up(zv, &mut ups)?;
            let u = &ups[&zv];
            for i in 0..n {
                if capped[i] && zs[i] == zv {
                    disp[i] = if u[i] < NODATA_MIN { u[i] } else { f32::NAN };
                }
            }
        }
        // (Detail fades in where measured cells meet capped ones.)
        if let Some(zc) = median.filter(|_| native.iter().any(|&x| x) && b.z > zmin) {
            let band = (PAD as f32).min(3.0 * (1u32 << (b.z - zc).min(20)) as f32);
            let (d, _) = nearest(&capped, w, h);
            get_up(b.z - 1, &mut ups)?;
            let prev = &ups[&(b.z - 1)];
            for i in 0..n {
                let wc = (d[i] / band).clamp(0.0, 1.0);
                if native[i] && wc < 1.0 && prev[i] < NODATA_MIN {
                    disp[i] = wc * smp.v[i] + (1.0 - wc) * prev[i];
                }
            }
        }
        let mut dv: Vec<bool> = disp.iter().map(|v| v.is_finite()).collect();
        if !s.datum_exact {
            // (Not on the others' datum: gaps only.)
            for i in 0..n {
                dv[i] &= !c[i].is_finite();
            }
        }
        let mut wgt = vec![1f32; n];
        let band = s.band_m / gpx;
        if s.datum_exact && band >= 1.0 && (0..n).any(|i| dv[i] && c[i].is_finite()) {
            // (Its edge in the water; along a shore it doesn't count.)
            let seed: Vec<bool> = (0..n).map(|i| !dv[i] && water.water[i]).collect();
            let (d, _) = nearest(&seed, w, h);
            let bw = band.min(PAD as f32);
            for i in 0..n {
                wgt[i] = if dok[i] { (d[i] / bw).clamp(0.0, 1.0) } else { 1.0 };
            }
        }
        for i in 0..n {
            if !dv[i] {
                continue;
            }
            c[i] = if c[i].is_finite() { wgt[i] * disp[i] + (1.0 - wgt[i]) * c[i] } else { disp[i] };
            if wgt[i] > 0.0 {
                zz[i] = zz[i].max(zs[i] as i8);
            }
            if wgt[i] >= 0.5 {
                k[i] = if native[i] { smp.class[i] } else { C_INTERP };
                src[i] = i16::from(s.code);
                inter[i] = s.intertidal;
                dok[i] = s.datum_exact;
                low[i] = smp.low[i];
            }
        }
    }
    // ---- the shore ----
    for i in 0..n {
        if !water.water[i] || !c[i].is_finite() {
            continue;
        }
        if c[i] > 0.0 && (!inter[i] || water.lake[i] > 0) {
            c[i] = 0.0;
        }
        c[i] = c[i].min(DRY_MAX);
    }
    if (0..n).any(|i| water.water[i] && !c[i].is_finite()) {
        fill(&mut c, &mut k, &mut zz, water, w, h, gpx);
    }
    let (dl, near) = nearest(&water.water, w, h);
    for i in 0..n {
        if water.water[i] {
            continue;
        }
        let j = near[i] as usize;
        c[i] = if dl[i] <= BLEED_PX && j < n && c[j].is_finite() { c[j].max(0.0) } else { f32::NAN };
        k[i] = C_NONE;
    }
    let mut out = Composed { depth: vec![NODATA; n], class: vec![C_NONE; n], zoom: zz };
    for i in 0..n {
        if c[i].is_finite() {
            out.depth[i] = c[i];
        }
        if !water.water[i] {
            continue;
        }
        let dries = water.lake[i] == 0
            && inter[i]
            && k[i] != C_FILLED
            && c[i].is_finite()
            && low[i].is_finite()
            && c[i] > -low[i];
        let code = if src[i] > 0 { (src[i] as u8) << 4 } else { 0 };
        out.class[i] = k[i] | if dries { DRIES } else { 0 } | code;
    }
    Ok(Some(out))
}

/// Water without data near data: filled from the nearest data of the same water body, falling
/// linearly to 0 at the shore, within `GAP_M`; a lake with no data of its own stays empty.
fn fill(c: &mut [f32], k: &mut [u8], zz: &mut [i8], water: &Water, w: usize, h: usize, gpx: f32) {
    let n = w * h;
    let g = (GAP_M / gpx).clamp(2.0, (4 * PAD) as f32);
    let valid: Vec<bool> = (0..n).map(|i| c[i].is_finite() && water.water[i]).collect();
    let (dv, iv) = nearest(&valid, w, h);
    let land: Vec<bool> = water.water.iter().map(|x| !x).collect();
    let (dl, _) = nearest(&land, w, h);
    let lakes_with: std::collections::HashSet<u32> =
        (0..n).filter(|&i| valid[i] && water.lake[i] > 0).map(|i| water.lake[i]).collect();
    for i in 0..n {
        if !water.water[i] || c[i].is_finite() || dv[i] > g || dl[i] > g {
            continue;
        }
        let j = iv[i] as usize;
        if j >= n || water.lake[j] != water.lake[i] || (water.lake[i] > 0 && !lakes_with.contains(&water.lake[i])) {
            continue;
        }
        // (dl: to the nearest land pixel's centre, where the depth is 0.)
        let t = dl[i] / (dl[i] + dv[i]).max(1e-6);
        c[i] = c[j] * t;
        k[i] = C_FILLED;
        zz[i] = zz[j];
    }
}

/// The block's tiles worth keeping: (x, y, depth, class), each 256 × 256.
pub fn tiles(b: Block, c: &Composed, water: &Water) -> Vec<(u32, u32, Vec<f32>, Vec<u8>)> {
    let w = b.w();
    let mut out = Vec::new();
    for j in 0..b.ny as usize {
        for i in 0..b.nx as usize {
            let mut d = vec![NODATA; 65536];
            let mut k = vec![C_NONE; 65536];
            let (mut wet, mut own) = (false, false);
            for y in 0..256 {
                let row = (PAD + j * 256 + y) * w + PAD + i * 256;
                for x in 0..256 {
                    let p = row + x;
                    d[y * 256 + x] = c.depth[p];
                    k[y * 256 + x] = c.class[p];
                    if water.water[p] && c.depth[p] < NODATA_MIN {
                        wet = true;
                        own |= c.zoom[p] >= b.z as i8;
                    }
                }
            }
            if wet && (b.z <= ALL_UPTO || own) {
                out.push((b.tx0 + i as u32, b.ty0 + j as u32, d, k));
            }
        }
    }
    out
}
