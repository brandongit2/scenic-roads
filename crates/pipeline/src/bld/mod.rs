//! The 3D buildings (docs/buildings3d.md): every building of the coverage from the pinned Overture
//! release, extruded to its height (measured, from its floors, or estimated: §2.3), as vector tiles
//! of zooms 12–14 per z6 tile.
//!
//! Their sources come onto the NAS whole by `bld-fetch` ([`fetch`]), once per release and coverage.
//! Then two steps per z6 tile T (§3.1):
//! - **`bldprep T`** ([`prep`]): the downloaded Overture row groups meeting T ([`overture`]) and
//!   the GHSL windows under T ([`ghsl`]) are read as columns; each building's and part's WKB is
//!   read, its centroid, footprint area and z14 tile computed, GHSL sampled at the centroid, and
//!   those whose centroid is in T are written, sorted by (z14 tile, id), as T's normalized file
//!   `work/bld/6-x-y` ([`work`], docs/formats.md).
//! - **`bldtiles T`** ([`job`]): the buildings of T that touch the coverage, their heights filled
//!   ([`fill`]: the neighbours' rule reads the buildings within 300 m beyond T's edges from its 8
//!   neighbours' files), the z14 tiles encoded ([`tiles`]), written as the hi pack
//!   `layers/buildings/hi/6-x-y`. A z8 tile at a time. Pure: its output is a function of its inputs'
//!   bytes (the work files and the coverage over T).
//!
//! (The design's `buildings T` step is `bldtiles` here: `buildings` is the roadside buildings' job,
//! crate::buildtiles, in scenic-build and the agent.)

pub mod fetch;
pub mod field;
pub mod fill;
pub mod ghsl;
pub mod job;
pub mod overture;
pub mod prep;
pub mod sources;
pub mod task;
pub mod tiles;
pub mod work;

/// Bumped when the normalized files change (every `bldprep` target runs again).
pub const BLDPREP_V: u32 = 2;
/// Bumped when the fill (its rules, fits and defaults: [`fill`]) or the tiles change (every
/// `bldtiles` target runs again, nothing else). (5, task #862: a heritage footprint's buildings by
/// roadcore::geo::inside.)
pub const BUILDINGS_V: u32 = 5;
/// The served layer (`/tiles/buildings/…`, the catalogue's `buildings`).
pub const LAYER: &str = "buildings";
/// The zooms the tiles are made at.
pub const ZOOM: u8 = 14;

/// Degrees to E7, as every derived coordinate is rounded.
pub fn e7(v: f64) -> i32 {
    (v * 1e7).round() as i32
}

/// Web Mercator world coordinates of an E7 point (roadcore::geo::merc_clamped).
pub fn world7(p: [i32; 2]) -> (f64, f64) {
    roadcore::geo::merc_clamped(p[0] as f64 * 1e-7, p[1] as f64 * 1e-7)
}

/// The z14 tile key (roadcore::archive::tile_key) of an E7 point: the block a building's record
/// is in.
pub fn z14_key(p: [i32; 2]) -> u64 {
    let (x, y) = roadcore::geo::tile_of(world7(p), 14);
    roadcore::archive::tile_key(14, x, y)
}

/// A tile key's zoom, x and y.
pub fn key_zxy(k: u64) -> (u8, u32, u32) {
    ((k >> 58) as u8, ((k >> 29) & ((1 << 29) - 1)) as u32, (k & ((1 << 29) - 1)) as u32)
}

/// The logical name of z6 tile T's normalized file.
pub fn work_logical(x: u32, y: u32) -> String {
    format!("work/bld/6-{x}-{y}")
}

/// The logical name of z6 tile T's hi pack.
pub fn pack_logical(x: u32, y: u32) -> String {
    crate::layers::hi_logical(LAYER, x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_and_keys() {
        // Shinjuku station: z14 tile 14/14549/6451, in z6 tile 6/56/25.
        let p = [e7(139.7006), e7(35.6896)];
        let (z, x, y) = key_zxy(z14_key(p));
        assert_eq!((z, x, y), (14, 14549, 6451));
        assert_eq!((x >> 8, y >> 8), (56, 25));
        let b = roadcore::geo::tile_box(14, x, y);
        assert!(b[0] <= 139.7006 && 139.7006 < b[2] && b[1] < 35.6896 && 35.6896 <= b[3]);
        // World coordinates round-trip the tile box.
        let (wx, wy) = roadcore::geo::merc_clamped(b[0], b[3]);
        assert!((wx * 16384.0 - x as f64).abs() < 1e-6 && (wy * 16384.0 - y as f64).abs() < 1e-6);
    }
}
