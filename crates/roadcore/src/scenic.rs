//! Scenic analysis: road sample points and the per-vertex / per-sample channels.
//!
//!   samples.bin        `Sample` records (every ~100 m along each road)
//!   near.i8            32 near-field horizon angles per sample (0.5° units, −128 = none)
//!   roadside.u8        [roadside tree height ×8 m, forest cover within 150 m ×255] per sample
//!   samples.ch.u8      `ch::N` channels per sample
//!   scenic.u8          `ch::N` channels per vertex
//!   vterrain.i16       drape height per vertex (metres) for 3D rendering

use bytemuck::{Pod, Zeroable};

pub const SAMPLE_SPACING_M: f64 = 100.0;
pub const NEAR_AZ: usize = 32;
pub const NEAR_MAX_M: f64 = 300.0;
pub const FAR_MAX_M: f64 = 15_000.0;
pub const EYE_M: f32 = 1.5;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Sample {
    pub way: u32,
    /// Distance along the way, metres.
    pub dist: f32,
    pub lon: i32,
    pub lat: i32,
    /// Observer eye elevation, metres.
    pub eye: f32,
    /// `sflag::*`
    pub flags: u8,
    pub _pad: [u8; 3],
}

const _: () = assert!(std::mem::size_of::<Sample>() == 24);

pub mod sflag {
    pub const TUNNEL: u8 = 1;
    pub const BRIDGE: u8 = 2;
}

/// Channel indices (u8 each).
pub mod ch {
    /// Visible area within 15 km (log scale), terrain- and tree-aware.
    pub const VIEW: usize = 0;
    /// Visible water area (log scale).
    pub const WATER: usize = 1;
    /// Terrain relief within 3 km, 3 m units.
    pub const RELIEF: usize = 2;
    /// Height above (+) / below (−) surrounding terrain within 1.5 km: 128 + m/2.
    pub const TPI: usize = 3;
    /// Curviness, 4 °/km units.
    pub const CURVY: usize = 4;
    /// Share of directions blocked within 300 m (trees or terrain), ×255.
    pub const ENCLOSURE: usize = 5;
    /// Built-up share within 500 m, ×255.
    pub const BUILT: usize = 6;
    /// `flag::*` bits.
    pub const FLAGS: usize = 7;
    /// Farthest visible distance, 1/17 km units (15 km → 255).
    pub const VISTA: usize = 8;
    /// Open land (fields, meadows, bare) within 1 km, ×255.
    pub const OPEN: usize = 9;
    /// Forest cover within 150 m, ×255.
    pub const COVER: usize = 10;
    /// Roadside tree height, 1/8 m units.
    pub const TREEH: usize = 11;
    pub const N: usize = 12;
}

/// Bits of `ch::FLAGS`.
pub mod flag {
    pub const SCENIC_ROUTE: u8 = 1 << 0;
    pub const PARK: u8 = 1 << 1;
    pub const VIEWPOINT: u8 = 1 << 2;
    pub const WATERFRONT: u8 = 1 << 3;
    pub const HERITAGE: u8 = 1 << 4;
    pub const COVERED_BRIDGE: u8 = 1 << 5;
    /// UNESCO biosphere reserve / geopark or dark-sky preserve.
    pub const SPECIAL_AREA: u8 = 1 << 6;
    pub const INDIGENOUS: u8 = 1 << 7;
}

/// Log mapping of an area (km²) to 0..255 (0.05 km² .. 700 km²).
pub fn area_u8(km2: f64) -> u8 {
    let v = (1.0 + km2 / 0.05).ln() / (1.0 + 700.0f64 / 0.05).ln();
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

pub fn u8_area(v: u8) -> f64 {
    ((v as f64 / 255.0) * (1.0 + 700.0 / 0.05f64).ln()).exp_m1() * 0.05
}
