//! Peak prominence and isolation from terrain tiles (docs/phase5.md "peaks"), a z6 tile's peaks at a
//! time (`unit`).
//!
//! Summit: the highest DEM pixel within 150 m of the OSM point. A pixel claimed by several peaks
//! (needles next to a main summit) goes to the one tagged highest; the others start from their own
//! point. The DEM (~30 m at z12) cuts sharp summits down (Snowdon: 1,040 m for 1,085 m, below the
//! broad Carneddau), so the summit height is the tagged `ele` where it is plausible (from 30 m
//! under to 200 m over the DEM), else the DEM's; every summit pixel is raised to its peak's height
//! for all floods and searches, so a neighbouring summit counts at its real height too (Garnedd
//! Ugain, 740 m from Snowdon, would otherwise see nothing higher nearby).
//!
//! Prominence: a priority flood from the summit, always expanding the highest unvisited pixel,
//! until it reaches ground higher than the summit; the lowest pixel it had to pass is the key col,
//! and prominence = summit − col. Once the flood is down to sea level the answer is the summit
//! height whatever lies beyond. It runs at z12 (the terrain's hi packs) for up to 600k pixels,
//! then, for the bigger peaks, again at z8 (the lo packs: ~430 m pixels, which can miss narrow cols
//! and summits, so those values are rougher). Missing tiles are walls: a flood that ends without
//! higher ground or sea gives a lower bound.
//!
//! The terrain tiles have single-pixel spikes and pits (a 2,781 m pixel among ~700 m ones in a z8
//! tile over the Laurentides), which would be false summits and cols: a pixel more than
//! max(150 m, 1.2 × the pixel size) above or below all eight neighbours is clamped to them.
//!
//! Isolation: the distance to the nearest ground higher than the summit, searching tiles nearest
//! first (z12 within 25 km, then z8 over the region); none found = a lower bound (region edge).
//!
//! Results as peaks.json has them: {i (the peak's index), e (DEM summit m), p (prominence m), pl
//! (lower bound), c ([lon, lat] of the col), ce (col m), iso (km), il (lower bound), hi ([lon, lat]
//! of the nearest higher ground)}.

pub mod unit;
pub mod z8;

use std::collections::HashMap;

const FINE_MAX: usize = 600_000;
const COARSE_MAX: usize = 40_000_000;
const ISO_FINE_KM: f64 = 25.0;
const TS: i64 = 256;

/// Summit heights stamped over the DEM: by pixel, and by tile for the tile scans.
#[derive(Default)]
struct Overlay {
    px: HashMap<(i64, i64), f32>,
    tiles: HashMap<(i64, i64), Vec<(i64, i64, f32)>>,
}

impl Overlay {
    fn add(&mut self, x: i64, y: i64, e: f32) {
        let v = self.px.entry((x, y)).or_insert(f32::MIN);
        *v = v.max(e);
    }
    fn index(&mut self) {
        for (&(x, y), &e) in &self.px {
            self.tiles.entry((x.div_euclid(TS), y.div_euclid(TS))).or_default().push((x, y, e));
        }
        // In pixel order, so that of two summits at the same distance the same one is the nearest
        // higher ground every run.
        for v in self.tiles.values_mut() {
            v.sort_unstable_by_key(|p| (p.1, p.0));
        }
    }
}

/// Heights below 0 m to 0 m (the peaks' sea, docs/phase5.md), then single-pixel spikes and pits
/// (interior pixels) clamped to their neighbours. Clamped first: the terrain keeps the source's
/// bathymetry at sea (task #975), and an islet's pixel 80 m over −100 m water would read as a spike.
pub(crate) fn despike(t: &mut [f32], z: u8, lat: f64) {
    for v in t.iter_mut() {
        *v = v.max(0.0);
    }
    let px_m = roadcore::geo::merc_m(lat) / ((1u64 << z) as f64 * TS as f64);
    let thr = (1.2 * px_m).max(150.0) as f32;
    let src = t.to_vec();
    let w = TS as usize;
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
            } else if v < lo - thr {
                t[y * w + x] = lo;
            }
        }
    }
}

/// Latitude of a tile's centre.
pub(crate) fn tile_lat(z: u8, ty: u32) -> f64 {
    let y = (ty as f64 + 0.5) / (1u64 << z) as f64;
    roadcore::geo::merc_lat(y)
}

/// A peak's result.
#[derive(Clone, Debug, PartialEq)]
pub struct Out {
    pub i: usize,
    pub e: f32,
    pub p: f32,
    pub pl: bool,
    pub c: [f64; 2],
    pub ce: f32,
    pub iso: f64,
    pub il: bool,
    pub hi: Option<[f64; 2]>,
}

impl Out {
    /// As peaks.json has it.
    pub fn json(&self) -> serde_json::Value {
        let mut v =
            serde_json::json!({ "i": self.i, "e": self.e, "p": self.p, "c": self.c, "ce": self.ce, "iso": self.iso });
        if self.pl {
            v["pl"] = true.into();
        }
        if self.il {
            v["il"] = true.into();
        }
        if let Some(h) = self.hi {
            v["hi"] = serde_json::json!(h);
        }
        v
    }
}

fn round5(ll: [f64; 2]) -> [f64; 2] {
    [(ll[0] * 1e5).round() / 1e5, (ll[1] * 1e5).round() / 1e5]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An islet's pixel 80 m over water the terrain has at −100 m (the source's bathymetry, task
    /// #975) is no spike: the sea is 0 m to the peaks, so it stands 80 m over it and stays; a
    /// spike 400 m over the sea is still cut, and every height below 0 m is 0 m.
    #[test]
    fn the_sea_is_0_m_before_despiking() {
        let w = TS as usize;
        let mut t = vec![-100.0f32; w * w];
        t[100 * w + 100] = 80.0;
        t[50 * w + 50] = 400.0;
        despike(&mut t, 12, 35.0);
        assert_eq!(t[100 * w + 100], 80.0, "the islet kept");
        assert_eq!(t[50 * w + 50], 0.0, "the spike cut to its neighbours");
        assert!(t.iter().all(|&v| v >= 0.0));
    }
}
