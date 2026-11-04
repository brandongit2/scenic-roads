//! Each place's finest zoom (docs/steps.md §2, Terrain: the rule), derived from the declared
//! elevation datasets (docs/inputs.md §6.8), never typed: no dataset, zoom or latitude is named
//! here. A dataset's finest zoom at a latitude is the finest whose pixels are no finer than its
//! cell there (`CELL_FACTOR` of it: the owner's, 4 Nov), its cell its band's (`Measured::bands`,
//! measured per file), else its resolution; a tile pyramid's, its declared zoom. A place's sources
//! are the declared datasets meeting it in their order (rank, then resolution), up to the first that
//! holds it whole: past that one no later dataset is read there but to fill voids
//! (crate::dem::generic). A tile's finest zoom is the finest of its sources' at its poleward edge,
//! at most `DEEPEST`.
//!
//! The same rule sets the polar caps' finest zoom (crate::polar), on their grid's pixel.

use crate::dem::declared::{self, Accepted, Cover, Data};
use anyhow::{Context, Result};

/// A dataset's pixels are no finer than this share of its cell (the owner's: 1, 4 Nov).
pub const CELL_FACTOR: f64 = 1.0;

/// The deepest zoom the map stores (its deep packs' last: docs/formats.md, Pack; the owner's, so
/// 1 m data stops there too).
pub const DEEPEST: u8 = 16;

/// The finest zoom at latitude `lat` whose pixels are no finer than `CELL_FACTOR` × `cell` metres
/// (Web Mercator's), at most `DEEPEST`.
pub fn zoom_for(cell: f64, lat: f64) -> u8 {
    let mut z = 0u8;
    while z < DEEPEST && crate::dem::generic::tile_resolution(z as u32 + 1, lat) >= cell * CELL_FACTOR - 1e-9 {
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
    /// A raster: its cell (m) by latitude band (`declared::Band`), else `all`.
    Raster { bands: Vec<declared::Band>, all: f64 },
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

/// A declared dataset as the terrain's keys and runs read it.
#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    pub id: String,
    /// As the keys name it (crate::dem::declared::line: its `keyed` and its heights' conversion).
    pub line: String,
    pub cover: Cover,
    pub detail: Detail,
    /// Where its `regions` cut its coverage: what was measured (`Measured::uncut`, for
    /// declared::cut_note); None: not cut.
    pub cut: Option<declared::Measured>,
    /// A pyramid published at every zoom to its declared one (the Terrarium encoding's, as
    /// crate::terrain_base reads it): what the coarse relief outside every region is read from
    /// (`relief`), one tile a tile.
    pub every_zoom: bool,
}

/// Datasets `ds` (in their order: declared::accepted), as `Set`s, their regions' outlines read
/// (`read`: a content name's bytes; declared::OutlineCut).
pub fn sets(ds: &[Accepted], read: &dyn Fn(&str) -> Result<Vec<u8>>) -> Result<Vec<Set>> {
    ds.iter()
        .map(|d| -> Result<Set> {
            let outline = match &d.measured.outline {
                Some(o) => Some(std::sync::Arc::new(
                    declared::OutlineCut::read(o, read).with_context(|| format!("{}: its regions' outlines", d.id))?,
                )),
                None => None,
            };
            Ok(Set {
                id: d.id.clone(),
                line: declared::line(d),
                cover: Cover { outline, ..Cover::of(d.measured.cover.as_deref()) },
                detail: Detail::of(d),
                cut: d.measured.uncut.is_some().then(|| d.measured.clone()),
                every_zoom: matches!(&d.declaration.data, Data::Tiles { encoding, .. } if encoding == "terrarium"),
            })
        })
        .collect()
}

/// The poleward latitude of a box (where a raster's pixels, in Web Mercator, are finest).
pub fn poleward(b: [f64; 4]) -> f64 {
    b[1].abs().max(b[3].abs())
}

/// Of `sets`, those a place in box `b` (w, s, e, n) may read, in their order: each meeting it, up
/// to the first holding it whole (`Cover::contains_box`).
pub fn sources(sets: &[Set], b: [f64; 4]) -> Vec<&Set> {
    let mut out = Vec::new();
    for s in sets {
        if !s.cover.meets(b) {
            continue;
        }
        out.push(s);
        if s.cover.contains_box(b) == Some(true) {
            break;
        }
    }
    out
}

/// Tile `z/x/y`'s finest zoom: its sources' finest at its poleward edge (`sources`), at most
/// `DEEPEST`; 0 with none.
pub fn finest(sets: &[Set], z: u8, x: u32, y: u32) -> u8 {
    let b = crate::stage::tile_box_grown(z, x, y, 0.0);
    let lat = poleward(b);
    sources(sets, b).iter().map(|s| s.detail.zoom_at(lat)).max().unwrap_or(0).min(DEEPEST)
}

/// What tile `z/x/y`'s key names of its sources: each one's line, where its regions cut it
/// inside the tile, and the zoom it reaches there (so a dataset measured again moves the key).
pub fn key_lines(sets: &[Set], z: u8, x: u32, y: u32) -> Vec<String> {
    let b = crate::stage::tile_box_grown(z, x, y, 0.0);
    let lat = poleward(b);
    sources(sets, b)
        .into_iter()
        .map(|s| {
            let cut = s.cut.as_ref().and_then(|m| declared::cut_note(m, &s.cover, b));
            match cut {
                Some(n) => format!("{} {n} zoom {}", s.line, s.detail.zoom_at(lat)),
                None => format!("{} zoom {}", s.line, s.detail.zoom_at(lat)),
            }
        })
        .collect()
}

/// Of `sets`, those published at every zoom (`Set::every_zoom`): the relief outside every region.
pub fn relief(sets: &[Set]) -> Vec<&Set> {
    sets.iter().filter(|s| s.every_zoom).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(cell: f64) -> Detail {
        Detail::Raster { bands: Vec::new(), all: cell }
    }

    #[test]
    fn a_datasets_pixels_are_no_finer_than_its_cell() {
        for (cell, lat) in
            [(30.0, 0.0), (30.0, 50.0), (30.0, 70.0), (10.0, 40.0), (1.0, 45.0), (2.0, 60.0), (100.0, 10.0)]
        {
            let z = raster(cell).zoom_at(lat);
            let px = |z: u8| crate::dem::generic::tile_resolution(z as u32, lat);
            assert!(px(z) >= cell * CELL_FACTOR - 1e-9 || z == 0, "{cell} m at {lat}°: z{z}'s {} m", px(z));
            assert!(z == DEEPEST || px(z + 1) < cell * CELL_FACTOR, "{cell} m at {lat}°: z{} would do", z + 1);
        }
        assert_eq!(raster(0.01).zoom_at(0.0), DEEPEST, "never past the deepest zoom stored");
        assert_eq!(Detail::Pyramid(14).zoom_at(70.0), 14);
    }

    #[test]
    fn a_bands_cell_is_its_own_and_the_coarsest_where_bands_meet() {
        let d = Detail::Raster {
            bands: vec![
                declared::Band { south: 0.0, north: 50.0, cell: 20.0 },
                declared::Band { south: 50.0, north: 60.0, cell: 40.0 },
            ],
            all: 30.0,
        };
        assert_eq!(d.cell_at(10.0), Some(20.0));
        assert_eq!(d.cell_at(50.0), Some(40.0));
        assert_eq!(d.cell_at(70.0), Some(30.0), "past its bands, its resolution");
        assert_eq!(d.zoom_at(10.0), raster(20.0).zoom_at(10.0));
    }

    #[test]
    fn the_polar_grid_takes_the_same_rule() {
        let px = |z: u8| 2.0 * roadcore::polar::HALF / (256.0 * (1u64 << z) as f64);
        for cell in [8.0, 16.0, 30.0, 32.0, 100.0] {
            let z = polar_zoom_for(cell);
            assert!(px(z) >= cell * CELL_FACTOR - 1e-9 && px(z + 1) < cell * CELL_FACTOR, "{cell} m: z{z}");
        }
    }

    fn set(id: &str, cover: Option<Vec<[f64; 4]>>, detail: Detail) -> Set {
        Set { id: id.into(), line: id.into(), cover: Cover::of(cover.as_deref()), detail, cut: None, every_zoom: false }
    }

    #[test]
    fn a_places_sources_end_at_the_first_holding_it_whole() {
        let fine = set("a", Some(vec![[-75.0, 45.0, -74.0, 46.0]]), raster(1.0));
        let world = set("b", None, raster(30.0));
        let after = set("c", None, Detail::Pyramid(12));
        let sets = vec![fine, world, after];
        // (A tile inside the fine one's cell, and one past it.)
        let (x, y) = crate::bld::tile_of((-74.5, 45.5), 12);
        assert_eq!(sources(&sets, crate::stage::tile_box_grown(12, x, y, 0.0)).len(), 2);
        assert_eq!(finest(&sets, 12, x, y), DEEPEST, "1 m data, to the deepest zoom stored");
        let (x, y) = crate::bld::tile_of((10.0, 0.5), 12);
        let b = crate::stage::tile_box_grown(12, x, y, 0.0);
        let s = sources(&sets, b);
        assert_eq!(s.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b"], "the pyramid after it is hidden");
        assert_eq!(finest(&sets, 12, x, y), raster(30.0).zoom_at(poleward(b)));
    }
}
