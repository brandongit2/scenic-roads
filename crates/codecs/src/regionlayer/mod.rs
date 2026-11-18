//! The region layer's tiles (docs/steps.md §15): the regions' borders as vector tiles, made from
//! their full geometry with each shared border simplified once per zoom.
//!
//! - [`topo`]: the regions' rings as a topology: every edge two regions share found once, edges
//!   joined into arcs between nodes, each arc with the regions on its two sides.
//! - [`simplify`]: each arc simplified once per zoom (Douglas–Peucker, its ends fixed), arcs that
//!   would cross or touch after it given back points until none does, rings under a pixel across
//!   left out where no region needs them.
//! - [`tiles`]: the arcs drawn as lines, clipped to each tile and its buffer: MVT, layer
//!   [`SOURCE_LAYER`], a feature an arc with the regions left and right of it and the level it
//!   separates.
//! - [`hit`]: the regions at a point, from the tiles alone.
//!
//! Nothing here reads or writes a file: the step (terrain::regionlayer) reads the geometry and
//! writes the packs.

pub mod hit;
pub mod simplify;
pub mod tiles;
pub mod topo;

/// The layer's name in the catalogue (`layers/regions/…`).
pub const LAYER: &str = "regions";
/// Its tiles' encoding: gzip'd MVT.
pub const ENCODING: &str = "mvt";
/// The MVT layer holding the borders.
pub const SOURCE_LAYER: &str = "borders";
/// The finest zoom built: every point of the geometry is in its tiles (past it, the map draws them
/// overzoomed).
pub const MAXZ: u8 = 12;
/// A tile's extent (MVT units across), and the finest zoom's (twice it: 1.2 m at the equator).
pub const EXTENT: u32 = 4096;
pub const EXTENT_MAXZ: u32 = 8192;
/// The buffer round each tile, as a share of the tile (64 units of 4096): lines reaching into it are
/// in the tile too, so their joins and caps across a tile edge are drawn whole.
pub const BUFFER: f64 = 1.0 / 64.0;
/// The simplification at each zoom, in pixels of a 512-pixel tile: no point of a simplified arc
/// strays further from the arc than this.
pub const TOL_PX: f64 = 0.5;
/// A ring no other arc meets (an island, a lake, an enclave's border) smaller than this across, in
/// pixels of a 512-pixel tile, is left out at the zoom (both sides at once: the region round it fills
/// it), unless it is the largest of a region made only of such rings.
pub const DROP_PX: f64 = 1.0;
/// The antimeridian and the poles (E7).
pub const HALF: i32 = 1_800_000_000;
pub const POLE: i32 = 900_000_000;

/// A region as the layer reads it: its id, the folders it is in (for the level a border
/// separates), and its rings (E7, unclosed) as one area, each ring with the region on its left
/// (outer rings counter-clockwise, holes clockwise; crate users make them so).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    pub id: String,
    pub folders: Vec<String>,
    pub rings: Vec<Vec<[i32; 2]>>,
}

/// The tolerance at zoom `z` in Web Mercator world units (0–1 across the world).
pub fn tolerance(z: u8) -> f64 {
    TOL_PX / (512.0 * f64::from(1u32 << z))
}

/// The extent of zoom `z`'s tiles.
pub fn extent(z: u8) -> u32 {
    if z == MAXZ {
        EXTENT_MAXZ
    } else {
        EXTENT
    }
}

#[cfg(test)]
mod tests;
