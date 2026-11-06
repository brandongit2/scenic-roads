//! The terrain's zoom rule (docs/steps.md §2, Terrain: the rule; crate::terrain_zoom applies it
//! to the declared datasets): a dataset's finest zoom at a latitude is the finest whose pixels are
//! no finer than its cell there (`CELL_FACTOR` of it), at most `DEEPEST`; and the deep packs' zoom
//! (`PACK_Z`), by whose tiles a dataset's presence is measured. Here, with the datasets, since the
//! input check measures by them (task #916 step 6d: from crate::terrain_zoom and
//! crate::terrain_deep, which re-export them).

use super::declared::{Accepted, Band, Data};

/// The zoom of a deep pack's tile (a terrain-deep job's target: crate::terrain_deep).
pub const PACK_Z: u8 = 10;

/// A dataset's pixels are no finer than this share of its cell (the owner's: 1, 4 Nov).
pub const CELL_FACTOR: f64 = 1.0;

/// The deepest zoom the map stores (its deep packs' last: docs/formats.md, Pack; the owner's, so
/// 1 m data stops there too).
pub const DEEPEST: u8 = 16;

/// The finest zoom at latitude `lat` whose pixels are no finer than `CELL_FACTOR` × `cell` metres
/// (Web Mercator's), at most `DEEPEST`.
pub fn zoom_for(cell: f64, lat: f64) -> u8 {
    let mut z = 0u8;
    while z < DEEPEST && super::generic::tile_resolution(z as u32 + 1, lat) >= cell * CELL_FACTOR - 1e-9 {
        z += 1;
    }
    z
}

/// The finest zoom of a pole's grid (roadcore::polar: true scale at its standard parallel) whose
/// pixels are no finer than `CELL_FACTOR` × `cell` metres.
pub fn polar_zoom_for(cell: f64) -> u8 {
    let px = |z: u8| 2.0 * roadcore::polar::HALF / (roadcore::polar::TS as f64 * (1u64 << z) as f64);
    let mut z = 0u8;
    while z < DEEPEST && px(z + 1) >= cell * CELL_FACTOR - 1e-9 {
        z += 1;
    }
    z
}

/// How a dataset's finest zoom is told.
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    /// A tile pyramid: its declared zoom.
    Pyramid(u8),
    /// A raster: its cell (m) by latitude band (`Band`), else `all`.
    Raster { bands: Vec<Band>, all: f64 },
}

impl Detail {
    /// Of dataset `d`, as measured.
    pub fn of(d: &Accepted) -> Detail {
        match &d.declaration.data {
            Data::Tiles { zoom, .. } => Detail::Pyramid((*zoom).min(DEEPEST as u32) as u8),
            Data::Geotiff { .. } => {
                Detail::Raster { bands: d.measured.bands.clone().unwrap_or_default(), all: d.measured.resolution }
            }
        }
    }

    /// Its cell (m) at latitude `lat`: the coarsest band's holding it, else its resolution.
    pub fn cell_at(&self, lat: f64) -> Option<f64> {
        match self {
            Detail::Pyramid(_) => None,
            Detail::Raster { bands, all } => Some(
                bands
                    .iter()
                    .filter(|b| b.south <= lat && lat <= b.north)
                    .map(|b| b.cell)
                    .reduce(f64::max)
                    .unwrap_or(*all),
            ),
        }
    }

    /// Its finest zoom at latitude `lat`.
    pub fn zoom_at(&self, lat: f64) -> u8 {
        match self {
            Detail::Pyramid(z) => *z,
            Detail::Raster { .. } => zoom_for(self.cell_at(lat).unwrap_or(f64::INFINITY), lat),
        }
    }

    /// Its finest zoom on a pole's grid (its cell at the pole's latitudes: `lat`).
    pub fn polar_zoom(&self, lat: f64) -> u8 {
        match self {
            // (A Web Mercator pyramid has nothing past its edge.)
            Detail::Pyramid(_) => 0,
            Detail::Raster { .. } => polar_zoom_for(self.cell_at(lat).unwrap_or(f64::INFINITY)),
        }
    }
}
