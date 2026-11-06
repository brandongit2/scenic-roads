//! A block of depth tiles composed from the depth sources (docs/steps.md §3): up to 8 × 8
//! tiles of one zoom with a margin of `PAD` pixels, on the basemap's own water
//! (codecs::terrain_water).
//!
//! 1. Each source is sampled onto the block (crate::depth::grids): depth, the zoom of the grid each
//!    pixel is from, its chart low water. A sea source is drawn on the sea (and in rivers and lakes
//!    where declared so: its depths there below their own surface), a lake source in lakes
//!    (`Source::drawn_in`); on an enclosed sea, its values less that sea's surface
//!    (crate::depth::surface: the Caspian's depths below its −28 m).
//! 2. Surveys are painted coarse to fine, then the estimates, which only fill what no survey has (a
//!    survey beats an estimate wherever both cover water). A pixel past its source's zoom shows the
//!    stored tiles of that zoom scaled up, as the map's fallback makes them (crate::depth::fall);
//!    inside a source, detail fades in over three of the capped zoom's pixels where its finer grids
//!    meet capped ones;
//!    where a source ends in the water it's feathered into what lies beneath over its band, where
//!    something does (never across a source whose datum isn't corrected to the others', which only
//!    fills gaps); with nothing beneath, its pixels are its own whole.
//! 3. The water decides the shore: positive values stay as drying heights only where an intertidal
//!    source gives them (else 0, and at most `DRY_MAX`); water with no data near data is filled,
//!    falling linearly to 0 at the shore, from data within `GAP_M` reached through the same water
//!    body (never across a dike or a spit); a lake with no data of its own stays empty. The land
//!    keeps a `BLEED_PX` band of the nearest water's own value, class `C_NONE`, so a finer level's
//!    shoreline scaled up from it (crate::depth::fall) never meets a gap; the map doesn't draw it
//!    (it reaches past the water's own data: web/src/depthgrid.ts), nor the floor.
//! 4. A pixel above its chart's low water (an intertidal source's, in the sea) dries at low tide.
//! 5. A tile is kept when one of its water pixels is a source's data of the tile's zoom (the
//!    source painted there, not one feathered in under half nor a gap filled from it), so no tile
//!    is stored finer than the data under it.
//!
//! A block is composed only at a zoom some place in it has as its finest data (`composes_at`: a
//! grid of that zoom meets it); every coarser level is the mean of the finer (crate::depth::build).

use super::edt::nearest;
use super::grids::{px, sample, Reader, Source};
use super::{C_ESTIMATED, C_FILLED, C_NATIVE, C_NONE, C_SCALED, DRIES, NODATA, NODATA_MIN};
use anyhow::Result;
use det::Det;

/// The block's margin (pixels).
pub const PAD: usize = 32;
/// A block's tiles a side.
pub const BLOCK: u32 = 8;
/// The land's band of the nearest water's value (pixels).
pub const BLEED_PX: f32 = 3.0;
/// Drying heights above this (m) are land.
pub const DRY_MAX: f32 = 20.0;
/// How far water without data is filled from data (ground metres).
pub const GAP_M: f32 = 600.0;

/// The block's water: whether each pixel is water, and its lake (0: the sea or a river; else the
/// block's own number for the lake covering most of it). A pixel is water when the sea, lakes and
/// rivers cover half of it or more, or when it's narrow water (some of it water, none of its eight
/// neighbours half so: a channel under a pixel wide). A river pixel (rivers cover more of it than
/// the sea or lakes, which cover less than half of it) takes only the sources drawn in rivers; a
/// pixel water only by the rivers' share (a river pixel, or the sea and lakes under half with the
/// rivers making it half), or narrow water (`if_data`), stays water only where a source gives it a
/// value, so where none does the block is what it was without them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Water {
    pub water: Vec<bool>,
    pub lake: Vec<u32>,
    /// A river's pixel (empty: none in the block).
    pub river: Vec<bool>,
    /// Water only where a source has a value (empty: none such).
    pub if_data: Vec<bool>,
    /// The surface each sea pixel's depth is measured below where it isn't sea level: an enclosed
    /// sea's, the terrain's height there (crate::depth::surface), 0 m elsewhere; empty: sea level
    /// throughout.
    pub surface: Vec<f32>,
}

impl Water {
    /// All land, `n` pixels.
    pub fn land(n: usize) -> Water {
        Water { water: vec![false; n], lake: vec![0; n], river: Vec::new(), if_data: Vec::new(), surface: Vec::new() }
    }
    pub fn is_river(&self, i: usize) -> bool {
        self.river.get(i).copied().unwrap_or(false)
    }
    pub fn is_if_data(&self, i: usize) -> bool {
        self.if_data.get(i).copied().unwrap_or(false)
    }
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

/// A block composed: depth (NODATA: none), class bytes, the zoom each pixel's data supports, and
/// its water as composed (`Water::if_data` pixels without a value no longer water).
pub struct Composed {
    pub depth: Vec<f32>,
    pub class: Vec<u8>,
    pub zoom: Vec<i8>,
    pub water: Vec<bool>,
}

/// Whether a block at zoom `z` over lon/lat box `bbox` is composed: a grid of zoom `z` meets it, so
/// some place in it may have its finest data at `z` (a grid's zoom taken within the layer's,
/// `ZOOMS`: one coarser counts at the coarsest, one finer at the finest). No zoom is special: a
/// new source's grids of any zoom are composed at theirs, and coarser levels over them are means.
pub fn composes_at(sources: &[Source], bbox: [f64; 4], z: u8) -> bool {
    let (lo, hi) = super::ZOOMS;
    sources.iter().any(|s| s.meeting(bbox).any(|g| g.zoom.clamp(lo, hi) == z))
}

/// Composes block `b` from `sources` over `water`; `up(zv)` is the stored level `zv` scaled up onto
/// the block (NODATA: none), `zmin` the layer's coarsest zoom. None when no grid of the block's
/// zoom meets it (`composes_at`) or it has no water.
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
    if !composes_at(sources, bbox, b.z) {
        return Ok(None);
    }
    let here: Vec<(&Source, u8)> = sources.iter().filter_map(|s| s.max_zoom_in(bbox).map(|z| (s, z))).collect();
    let inner = |i: usize| {
        let (x, y) = (i % w, i / w);
        (PAD..w - PAD).contains(&x) && (PAD..h - PAD).contains(&y)
    };
    if !(0..n).any(|i| water.water[i] && inner(i)) {
        return Ok(None);
    }
    let gpx = (px(b.z) * b.mid_lat().to_radians().dcos()) as f32;
    let mut c = vec![f32::NAN; n];
    let mut zz = vec![-1i8; n];
    let mut k = vec![0u8; n];
    let mut src: Vec<i16> = vec![-1; n];
    let mut inter = vec![false; n];
    let mut dok = vec![true; n];
    let mut low = vec![f32::NAN; n];
    let mut order = here.clone();
    order.sort_by_key(|(s, z)| (s.estimated, *z, s.code));
    let mut ups: std::collections::HashMap<u8, Vec<f32>> = std::collections::HashMap::new();
    let mut get_up = |zv: u8, ups: &mut std::collections::HashMap<u8, Vec<f32>>| -> Result<()> {
        if !ups.contains_key(&zv) {
            let _p = jobrt::timings::sub("scaling up stored levels");
            let v = if zv < zmin { vec![NODATA; n] } else { up(zv)? };
            ups.insert(zv, v);
        }
        Ok(())
    };
    for (s, _) in &order {
        let Some(mut smp) = sample(rd, s, b.z, c0, r0, w, h)? else { continue };
        for i in 0..n {
            if !s.drawn_in(water.is_river(i), water.lake[i] > 0) {
                smp.v[i] = f32::NAN;
            } else if !water.surface.is_empty() && water.lake[i] == 0 && !water.is_river(i) {
                // (On an enclosed sea: its depth below its own surface.)
                smp.v[i] -= water.surface[i];
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
        // (Detail fades in where pixels of the tile's zoom meet capped ones.)
        if let Some(zc) = median.filter(|_| native.iter().any(|&x| x) && b.z > zmin) {
            get_up(b.z - 1, &mut ups)?;
            let _p = jobrt::timings::sub("feathering");
            let band = (PAD as f32).min(3.0 * (1u32 << (b.z - zc).min(20)) as f32);
            let (d, _) = nearest(&capped, w, h);
            let prev = &ups[&(b.z - 1)];
            for i in 0..n {
                let wc = (d[i] / band).clamp(0.0, 1.0);
                if native[i] && wc < 1.0 && prev[i] < NODATA_MIN {
                    disp[i] = wc * smp.v[i] + (1.0 - wc) * prev[i];
                }
            }
        }
        let mut dv: Vec<bool> = disp.iter().map(|v| v.is_finite()).collect();
        let gaps_only = s.estimated || !s.datum_exact;
        if gaps_only {
            // (An estimate, or not on the others' datum: gaps only.)
            for i in 0..n {
                dv[i] &= !c[i].is_finite();
            }
        }
        let mut wgt = vec![1f32; n];
        let band = s.band_m / gpx;
        if !gaps_only && band >= 1.0 && (0..n).any(|i| dv[i] && c[i].is_finite()) {
            // (Its edge in the water; along a shore it doesn't count.)
            let _p = jobrt::timings::sub("feathering");
            let seed: Vec<bool> = (0..n).map(|i| !dv[i] && water.water[i] && !water.is_if_data(i)).collect();
            let (d, _) = nearest(&seed, w, h);
            let bw = band.min(PAD as f32);
            for i in 0..n {
                // (Nothing beneath: its own whole. Weighed down, its value showed but its class,
                // zoom and source weren't set: a pixel classed none, which the map doesn't draw.)
                wgt[i] = if dok[i] && c[i].is_finite() { (d[i] / bw).clamp(0.0, 1.0) } else { 1.0 };
            }
        }
        for i in 0..n {
            if !dv[i] {
                continue;
            }
            c[i] = if c[i].is_finite() { wgt[i] * disp[i] + (1.0 - wgt[i]) * c[i] } else { disp[i] };
            if wgt[i] >= 0.5 {
                // (The zoom of the source the pixel is: a source feathered in under half, or
                // filled from, gives the tile no detail of its own.)
                zz[i] = zs[i] as i8;
                k[i] = if s.estimated {
                    C_ESTIMATED
                } else if native[i] {
                    C_NATIVE
                } else {
                    C_SCALED
                };
                src[i] = i16::from(s.code);
                inter[i] = s.intertidal;
                dok[i] = !gaps_only;
                low[i] = smp.low[i];
            }
        }
    }
    // ---- the shore ----
    // (A river's pixel or narrow water without a value is land, as it was without them.)
    let owned;
    let water = if water.if_data.is_empty() {
        water
    } else {
        let mut w2 = water.clone();
        for i in 0..n {
            if w2.water[i] && water.is_if_data(i) && !c[i].is_finite() {
                w2.water[i] = false;
                w2.lake[i] = 0;
            }
        }
        owned = w2;
        &owned
    };
    for i in 0..n {
        if !water.water[i] || !c[i].is_finite() {
            continue;
        }
        if c[i] > 0.0 && (!inter[i] || water.lake[i] > 0 || water.is_river(i)) {
            c[i] = 0.0;
        }
        c[i] = c[i].min(DRY_MAX);
    }
    if (0..n).any(|i| water.water[i] && !c[i].is_finite()) {
        let _p = jobrt::timings::sub("filling gaps");
        fill(&mut c, &mut k, water, w, h, gpx);
    }
    let (dl, near) = nearest(&water.water, w, h);
    for i in 0..n {
        if water.water[i] {
            continue;
        }
        let j = near[i] as usize;
        c[i] = if dl[i] <= BLEED_PX && j < n && c[j].is_finite() { c[j] } else { f32::NAN };
        k[i] = C_NONE;
    }
    let mut out = Composed { depth: vec![NODATA; n], class: vec![C_NONE; n], zoom: zz, water: water.water.clone() };
    for i in 0..n {
        if c[i].is_finite() {
            out.depth[i] = c[i];
        }
        if !water.water[i] {
            continue;
        }
        let dries = water.lake[i] == 0
            && !water.is_river(i)
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

/// Water without data near data: filled from the nearest data reached through the same water body
/// (a gap across a dike, a spit or another body's shore from data is never filled from it: the
/// Bassin de La Prairie beside the Seaway, task #963), falling linearly to 0 at the shore, within
/// `GAP_M`; a lake with no data of its own stays empty (a river is filled from a river's data alone).
fn fill(c: &mut [f32], k: &mut [u8], water: &Water, w: usize, h: usize, gpx: f32) {
    use std::cmp::Reverse;
    let n = w * h;
    let g = (GAP_M / gpx).clamp(2.0, (4 * PAD) as f32);
    let land: Vec<bool> = water.water.iter().map(|x| !x).collect();
    let (dl, _) = nearest(&land, w, h);
    let gap: Vec<bool> = (0..n).map(|i| water.water[i] && !c[i].is_finite() && dl[i] <= g).collect();
    let same = |i: usize, j: usize| water.lake[j] == water.lake[i] && water.is_river(j) == water.is_river(i);
    // (Dijkstra over the gaps, 8-connected, from the data beside them: each gap's distance through
    // its body's water to its nearest data, and which it is; ties go to the lower index.)
    let mut dv = vec![f32::INFINITY; n];
    let mut iv = vec![u32::MAX; n];
    let mut heap = std::collections::BinaryHeap::new();
    let steps = |i: usize| {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        [
            (-1i64, 0i64, 1f32),
            (1, 0, 1.0),
            (0, -1, 1.0),
            (0, 1, 1.0),
            (-1, -1, std::f32::consts::SQRT_2),
            (1, -1, std::f32::consts::SQRT_2),
            (-1, 1, std::f32::consts::SQRT_2),
            (1, 1, std::f32::consts::SQRT_2),
        ]
        .into_iter()
        .filter_map(move |(dx, dy, cost)| {
            let (nx, ny) = (x + dx, y + dy);
            (nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h)
                .then(|| (ny as usize * w + nx as usize, cost))
        })
    };
    for i in 0..n {
        if water.water[i] && c[i].is_finite() && steps(i).any(|(j, _)| gap[j] && same(i, j)) {
            dv[i] = 0.0;
            iv[i] = i as u32;
            heap.push(Reverse((0u32, i as u32)));
        }
    }
    while let Some(Reverse((bits, i))) = heap.pop() {
        let (d, i) = (f32::from_bits(bits), i as usize);
        if d > dv[i] {
            continue;
        }
        for (j, cost) in steps(i) {
            let nd = d + cost;
            if nd <= g && nd < dv[j] && gap[j] && same(i, j) {
                dv[j] = nd;
                iv[j] = iv[i];
                // (Non-negative floats order as their bits.)
                heap.push(Reverse((nd.to_bits(), j as u32)));
            }
        }
    }
    for i in 0..n {
        if !gap[i] || iv[i] == u32::MAX {
            continue;
        }
        let j = iv[i] as usize;
        // (dl: to the nearest land pixel's centre, where the depth is 0.)
        let t = dl[i] / (dl[i] + dv[i]).max(1e-6);
        c[i] = c[j] * t;
        k[i] = C_FILLED;
    }
}

/// The block's tiles worth keeping: (x, y, depth, class), each 256 × 256.
pub fn tiles(b: Block, c: &Composed) -> Vec<(u32, u32, Vec<f32>, Vec<u8>)> {
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
                    if c.water[p] && c.depth[p] < NODATA_MIN {
                        wet = true;
                        own |= c.zoom[p] >= b.z as i8;
                    }
                }
            }
            if wet && own {
                out.push((b.tx0 + i as u32, b.ty0 + j as u32, d, k));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task #963: a gap is filled only from data reached through its own water body: the sea beyond
    /// a dike (a land column) and a lake beside the data's water stay empty, the sea's own gap
    /// between the data and the dike is filled, falling to the shore.
    #[test]
    fn a_gap_is_filled_through_its_own_water_alone() {
        // 12 × 5: x 0–1 data, 2–5 the sea without, 6 the dike, 7–9 the sea beyond, 10–11 a lake.
        let (w, h) = (12, 5);
        let mut water = Water::land(w * h);
        let mut c = vec![f32::NAN; w * h];
        let mut k = vec![C_NONE; w * h];
        for y in 1..h - 1 {
            for x in (0..w).filter(|&x| x != 6) {
                let i = y * w + x;
                water.water[i] = true;
                water.lake[i] = u32::from(x >= 10);
                if x < 2 {
                    c[i] = -10.0;
                    k[i] = C_NATIVE;
                }
            }
        }
        fill(&mut c, &mut k, &water, w, h, 1.0);
        let at = |x: usize| (c[2 * w + x], k[2 * w + x]);
        for x in 2..6 {
            let (v, cl) = at(x);
            assert!(cl == C_FILLED && v < 0.0 && v > -10.0, "the sea's own gap at {x}: {v} {cl}");
        }
        for x in (7..10).chain(10..12) {
            assert!(at(x).0.is_nan() && at(x).1 == C_NONE, "across the dike or in the lake at {x}: {:?}", at(x));
        }
    }
}
