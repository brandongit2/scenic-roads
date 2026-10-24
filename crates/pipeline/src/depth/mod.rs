//! Depth under the water (docs/steps.md §3; docs/inputs.md §6.10): the depth layers, split by
//! tile like the terrain, from the declared depth sources' grids.
//!
//! - **`depth`** (Terrarium PNG, 256 px): depth below the water's surface in metres, negative (the
//!   sea's mean level; a lake's mean surface), drying heights positive, `NODATA` where there's none;
//!   quantised to 0.1 m in the first 100 m and 1 m deeper (`quantise`). It lies on the terrain's own
//!   water (crate::terrain_water), so the terrain minus the depth is the floor: the server serves it
//!   so (`?floor=1` on the terrain's tiles) with no second terrain stored.
//! - **`depth-class`** (8-bit grey PNG): a byte a pixel: the class (bits 0–2: `C_NONE`, measured,
//!   interpolated, estimated, filled to the shore), dries at low tide (bit 3), the source's code
//!   (bits 4–7, its declaration's `code`).
//! - **`depth-shade`** (crate::shade_codec, as the terrain's shading: crate::slope_pack): the
//!   floor's steepness and direction, which the map's depth shading reads, made with the depth
//!   tiles (`build`'s `shading`).
//!
//! Stored only where a pixel has detail of the tile's zoom (every water tile up to z8): a tile the
//! layer lacks is made from its nearest stored ancestor (`fall`) by the server, the same way the
//! build scales a coarser level up (`compose`). Two steps make them (`build`): **depth-lo**, a z3
//! tile's zooms 3–8 (z6–8 composed, z3–5 from their children), then **depth**, a z6 tile's zooms
//! 9–16 where a source has data finer than z8.

pub mod build;
pub mod compose;
pub mod edt;
pub mod fall;
pub mod grids;
pub mod resume;

/// The depth layer's name (`layers/depth/…`).
pub const LAYER: &str = "depth";
/// The class layer's.
pub const CLASS_LAYER: &str = "depth-class";
/// The floor's shading's (`depth-shade`; encoding crate::shade_codec::ENCODING).
pub const SHADE_LAYER: &str = "depth-shade";
/// Their encodings in the packs' meta and the catalog.
pub const ENCODING: &str = "terrarium-png";
pub const CLASS_ENCODING: &str = "class-png";
/// The zooms each layer holds.
pub const ZOOMS: (u8, u8) = (3, 16);

/// Depth tiles' "no data" (positive: interpolation toward it passes through dry land).
pub const NODATA: f32 = 12000.0;
/// Values at or above this are "no data".
pub const NODATA_MIN: f32 = 9000.0;

/// The classes (bits 0–2 of a class byte).
pub const C_NONE: u8 = 0;
pub const C_MEASURED: u8 = 1;
pub const C_INTERP: u8 = 2;
pub const C_ESTIMATED: u8 = 3;
pub const C_FILLED: u8 = 4;
/// Dries at the chart's low water (bit 3).
pub const DRIES: u8 = 8;

/// 0.1 m in the first 100 m, 1 m deeper; no data left alone.
pub fn quantise(v: f32) -> f32 {
    if v >= NODATA_MIN {
        v
    } else if v > -100.0 {
        (v * 10.0).round() / 10.0
    } else {
        v.round()
    }
}

#[cfg(test)]
mod tests;
