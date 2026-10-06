//! The 3D buildings (docs/buildings3d.md): every building of the coverage from the pinned Overture
//! release, extruded to its height (measured, from its floors, or estimated: §2.3), as vector tiles
//! of zooms 12–14 per z6 tile.
//!
//! Two steps per z6 tile T (§3.1):
//! - **`bldprep T`** ([`prep`]): `dem/bldprep.py` reads the downloaded Overture row groups meeting T
//!   and the GHSL windows under T, and hands their columns over a pipe; here each building's and
//!   part's WKB is read, its centroid, footprint area and z14 tile computed, GHSL sampled at the
//!   centroid, and those whose centroid is in T are written, sorted by (z14 tile, id), as T's
//!   normalized file `work/bld/6-x-y` ([`work`], docs/formats.md). Every number in it is computed
//!   here, in Rust (§3.7): Python only decodes.
//! - **`bldtiles T`** ([`job`]): the buildings of T that touch the coverage, their heights filled
//!   ([`fill`]: the neighbours' rule reads the buildings within 300 m beyond T's edges from its 8
//!   neighbours' files), the z12–14 tiles encoded ([`tiles`]), written as the hi pack
//!   `layers/buildings/hi/6-x-y`. A z8 area at a time. Pure: its output is a function of its inputs'
//!   bytes (the work files and the coverage over T).
//!
//! (The design's `buildings T` step is `bldtiles` here: `buildings` is the roadside buildings' job,
//! crate::buildtiles, in scenic-build and the agent.)

pub mod fill;
pub mod job;
pub mod prep;
pub mod tiles;
pub mod work;

use det::Det;

/// Bumped when the normalized files change (every `bldprep` target runs again).
pub const BLDPREP_V: u32 = 2;
/// Bumped when the fill (its rules, fits and defaults: [`fill`]) or the tiles change (every
/// `bldtiles` target runs again, nothing else).
pub const BUILDINGS_V: u32 = 1;
/// The served layer (`/tiles/buildings/…`, the catalog's `buildings`).
pub const LAYER: &str = "buildings";
/// The zooms the tiles are made at.
pub const MINZOOM: u8 = 12;
pub const MAXZOOM: u8 = 14;

/// A Web Mercator world unit at the equator, metres (WGS84's equator).
pub const EQ: f64 = 40_075_016.685_578_49;
/// A degree on the mean sphere, metres (footprint areas, as B0 measured them).
pub const DEG_M: f64 = 6_371_008.8 * std::f64::consts::PI / 180.0;
/// Web Mercator's latitude limit.
const MAX_LAT: f64 = 85.051_128_78;

/// Degrees to E7, as every derived coordinate is rounded.
pub fn e7(v: f64) -> i32 {
    (v * 1e7).round() as i32
}

/// Web Mercator world coordinates (0–1, y down) of degrees.
pub fn world(lon: f64, lat: f64) -> (f64, f64) {
    let s = lat.clamp(-MAX_LAT, MAX_LAT).to_radians().dsin();
    ((lon + 180.0) / 360.0, 0.5 - ((1.0 + s) / (1.0 - s)).dln() / (4.0 * std::f64::consts::PI))
}

/// Web Mercator world coordinates of an E7 point.
pub fn world7(p: [i32; 2]) -> (f64, f64) {
    world(p[0] as f64 * 1e-7, p[1] as f64 * 1e-7)
}

/// The zoom-`z` tile holding a point in world coordinates.
pub fn tile_of(w: (f64, f64), z: u8) -> (u32, u32) {
    let n = (1u64 << z) as f64;
    let c = |v: f64| ((v * n).floor().max(0.0) as u64).min((1u64 << z) - 1) as u32;
    (c(w.0), c(w.1))
}

/// The z14 tile key (roadcore::archive::tile_key) of an E7 point: the block a building's record
/// is in.
pub fn z14_key(p: [i32; 2]) -> u64 {
    let (x, y) = tile_of(world7(p), 14);
    roadcore::archive::tile_key(14, x, y)
}

/// A tile key's zoom, x and y.
pub fn key_zxy(k: u64) -> (u8, u32, u32) {
    ((k >> 58) as u8, ((k >> 29) & ((1 << 29) - 1)) as u32, (k & ((1 << 29) - 1)) as u32)
}

/// A tile's box in degrees (w, s, e, n).
pub fn tile_box_deg(z: u8, x: u32, y: u32) -> [f64; 4] {
    let n = (1u64 << z) as f64;
    let lon = |x: f64| x / n * 360.0 - 180.0;
    let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).dsinh().datan().to_degrees();
    [lon(x as f64), lat(y as f64 + 1.0), lon(x as f64 + 1.0), lat(y as f64)]
}

/// The logical name of z6 tile T's normalized file.
pub fn work_logical(x: u32, y: u32) -> String {
    format!("work/bld/6-{x}-{y}")
}

/// The logical name of z6 tile T's hi pack.
pub fn pack_logical(x: u32, y: u32) -> String {
    format!("layers/{LAYER}/hi/6-{x}-{y}")
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
        let b = tile_box_deg(14, x, y);
        assert!(b[0] <= 139.7006 && 139.7006 < b[2] && b[1] < 35.6896 && 35.6896 <= b[3]);
        // World coordinates round-trip the tile box.
        let (wx, wy) = world(b[0], b[3]);
        assert!((wx * 16384.0 - x as f64).abs() < 1e-6 && (wy * 16384.0 - y as f64).abs() < 1e-6);
    }
}
