//! The polar caps' projection and tiling (docs/polar.md, task #601): polar stereographic on the
//! WGS 84 ellipsoid, EPSG:3413 in the north (standard parallel 70° N, central meridian 45° W) and
//! EPSG:3031 in the south (71° S, 0°), with no false easting or northing (the pole at 0, 0). Snyder's
//! formulas (USGS PP 1395, §21; the EPSG guidance note 7-2's "variant B").
//!
//! The tiles are a quadtree over the square ±`HALF` metres around the pole, 256 pixels a side: tile
//! (z, x, y) is column x from the west (−x) and row y from the top (+y), as the Web Mercator tiles
//! count; at zoom z a pixel is `pixel_m(z)` metres of the grid (8192 m at z0, 16 m at z9). A pole's
//! tiles are those meeting its cap, the disc out to `CAP_LAT` (`cap_tiles`).

use det::Det;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

/// WGS 84's semi-major axis (m) and flattening.
pub const A: f64 = 6_378_137.0;
pub const F: f64 = 1.0 / 298.257_223_563;
/// Half the side of the tiled square (m): z0 is one tile of 2 × 2^20 m, reaching 80.6° S and 80.9° N.
pub const HALF: f64 = 1_048_576.0;
/// A tile's side in pixels.
pub const TS: usize = 256;
/// The caps' tiles reach this latitude (degrees from the equator, either pole): the Web Mercator
/// tiles stop at `MERCATOR_LAT`, so a ring of about 5 km past it gives the shading's border and
/// keeps the seam inside the tiles.
pub const CAP_LAT: f64 = 85.0;
/// Where the Web Mercator tiles end (degrees): the seam.
pub const MERCATOR_LAT: f64 = 85.051_128_779_806_59;

/// A pole and its projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pole {
    /// EPSG:3413.
    North,
    /// EPSG:3031.
    South,
}

impl Pole {
    /// Its letter in names and paths.
    pub fn letter(self) -> char {
        match self {
            Pole::North => 'n',
            Pole::South => 's',
        }
    }

    /// The pole a letter names.
    pub fn of_letter(c: &str) -> Option<Pole> {
        match c {
            "n" => Some(Pole::North),
            "s" => Some(Pole::South),
            _ => None,
        }
    }

    /// The projection's EPSG code.
    pub fn epsg(self) -> u16 {
        match self {
            Pole::North => 3413,
            Pole::South => 3031,
        }
    }

    /// The standard parallel's latitude (degrees, positive) and the central meridian (degrees).
    fn params(self) -> (f64, f64) {
        match self {
            Pole::North => (70.0, -45.0),
            Pole::South => (71.0, 0.0),
        }
    }

    /// +1 north, −1 south.
    fn sign(self) -> f64 {
        match self {
            Pole::North => 1.0,
            Pole::South => -1.0,
        }
    }
}

fn e() -> f64 {
    (F * (2.0 - F)).sqrt()
}

/// Snyder's t at latitude `phi` (radians, the pole's hemisphere taken as north).
fn t_of(phi: f64) -> f64 {
    let e = e();
    let es = e * phi.dsin();
    (FRAC_PI_4 - phi / 2.0).dtan() / ((1.0 - es) / (1.0 + es)).dpowf(e / 2.0)
}

/// Snyder's m at latitude `phi` (radians).
fn m_of(phi: f64) -> f64 {
    let s = phi.dsin();
    phi.dcos() / (1.0 - e() * e() * s * s).sqrt()
}

/// a × m_c / t_c: ρ = this × t.
fn rho_scale(pole: Pole) -> f64 {
    let pc = pole.params().0.to_radians();
    A * m_of(pc) / t_of(pc)
}

/// The distance from the pole on the grid (m) of latitude `lat` (degrees, either sign: its
/// distance from the pole is what counts).
pub fn rho(pole: Pole, lat: f64) -> f64 {
    rho_scale(pole) * t_of(lat.abs().to_radians())
}

/// Grid coordinates (x, y, m) of `lon`, `lat` (degrees).
pub fn forward(pole: Pole, lon: f64, lat: f64) -> (f64, f64) {
    let s = pole.sign();
    let r = rho_scale(pole) * t_of((s * lat).to_radians());
    let d = (lon - pole.params().1).to_radians();
    // North: x = ρ sin Δ, y = −ρ cos Δ; south: y = +ρ cos Δ.
    (r * d.dsin(), -s * r * d.dcos())
}

/// Longitude and latitude (degrees) of grid point `x`, `y` (m). The pole itself is at longitude
/// the central meridian.
pub fn inverse(pole: Pole, x: f64, y: f64) -> (f64, f64) {
    let s = pole.sign();
    let r = x.dhypot(y);
    let (_, lon0) = pole.params();
    if r == 0.0 {
        return (lon0, s * 90.0);
    }
    let t = r / rho_scale(pole);
    let e = e();
    let mut phi = FRAC_PI_2 - 2.0 * t.datan();
    for _ in 0..16 {
        let es = e * phi.dsin();
        let next = FRAC_PI_2 - 2.0 * (t * ((1.0 - es) / (1.0 + es)).dpowf(e / 2.0)).datan();
        let done = (next - phi).abs() < 1e-14;
        phi = next;
        if done {
            break;
        }
    }
    // Δ: x = ρ sin Δ, −s y = ρ cos Δ.
    let d = x.datan2(-s * y);
    let mut lon = lon0 + d.to_degrees();
    if lon > 180.0 {
        lon -= 360.0;
    } else if lon <= -180.0 {
        lon += 360.0;
    }
    (lon, s * phi.to_degrees())
}

/// The scale factor at latitude `lat` (degrees): grid metres per ground metre (1 at the standard
/// parallel, 0.97 at the poles).
pub fn scale(pole: Pole, lat: f64) -> f64 {
    let phi = lat.abs().to_radians();
    if (FRAC_PI_2 - phi).abs() < 1e-12 {
        // At the pole: ρ / (a m) → m_c / t_c × ((1 + e)^(1 + e) (1 − e)^(1 − e))^½ / 2.
        let e = e();
        let pc = pole.params().0.to_radians();
        return m_of(pc) / t_of(pc) * ((1.0 + e).dpowf(1.0 + e) * (1.0 - e).dpowf(1.0 - e)).sqrt() / 2.0;
    }
    rho_scale(pole) * t_of(phi) / (A * m_of(phi))
}

/// A pixel's side at zoom `z` (grid metres).
pub fn pixel_m(z: u8) -> f64 {
    2.0 * HALF / (TS as f64 * (1u64 << z) as f64)
}

/// The grid coordinates (m) of pixel (`i`, `j`)'s centre in tile (z, x, y), `i`, `j` from 0 to 255
/// (or past, for a neighbour's).
pub fn pixel_xy(z: u8, x: u32, y: u32, i: f64, j: f64) -> (f64, f64) {
    let p = pixel_m(z);
    (-HALF + (x as f64 * TS as f64 + i + 0.5) * p, HALF - (y as f64 * TS as f64 + j + 0.5) * p)
}

/// The box of tile (z, x, y) in grid metres: (west, south, east, north).
pub fn tile_box(z: u8, x: u32, y: u32) -> [f64; 4] {
    let s = 2.0 * HALF / (1u64 << z) as f64;
    [-HALF + x as f64 * s, HALF - (y + 1) as f64 * s, -HALF + (x + 1) as f64 * s, HALF - y as f64 * s]
}

/// Whether tile (z, x, y) meets the disc of radius `r` (m) around the pole.
pub fn meets_disc(z: u8, x: u32, y: u32, r: f64) -> bool {
    let b = tile_box(z, x, y);
    let dx = if b[0] > 0.0 {
        b[0]
    } else if b[2] < 0.0 {
        -b[2]
    } else {
        0.0
    };
    let dy = if b[1] > 0.0 {
        b[1]
    } else if b[3] < 0.0 {
        -b[3]
    } else {
        0.0
    };
    dx * dx + dy * dy <= r * r
}

/// The tiles of zoom `z` meeting the pole's cap (out to `CAP_LAT`), by row then column. A tile's
/// parent meets the cap whenever it does, so each zoom's tiles are the parents of the next's.
pub fn cap_tiles(pole: Pole, z: u8) -> Vec<(u32, u32)> {
    let r = rho(pole, CAP_LAT);
    let n = 1u32 << z;
    let mut v = Vec::new();
    for y in 0..n {
        for x in 0..n {
            if meets_disc(z, x, y, r) {
                v.push((x, y));
            }
        }
    }
    v
}

/// The directions of geographic east and north at longitude `lon` on the grid, as (x, y) unit
/// vectors (the grid's y up): the shading's directions are stored on the grid and turned to the
/// ground's by these (docs/polar.md, Shading).
pub fn east_north(pole: Pole, lon: f64) -> ([f64; 2], [f64; 2]) {
    let d = (lon - pole.params().1).to_radians();
    let (s, c) = (d.dsin(), d.dcos());
    match pole {
        Pole::North => ([c, s], [-s, c]),
        Pole::South => ([c, -s], [s, c]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The EPSG guidance note 7-2's example of polar stereographic variant B (WGS 84, standard
    /// parallel 71° S, origin longitude 70° E, false easting and northing 6,000,000 m): 75° S,
    /// 120° E is at 7,255,380.79 E, 7,053,389.56 N. Our south pole is the same projection with
    /// origin longitude 0°, so the point 50° east of the meridian matches it.
    #[test]
    fn matches_the_epsg_example() {
        let (x, y) = forward(Pole::South, 50.0, -75.0);
        assert!((x + 6_000_000.0 - 7_255_380.79).abs() < 0.01, "{x}");
        assert!((y + 6_000_000.0 - 7_053_389.56).abs() < 0.01, "{y}");
        // The north is its mirror at its own parameters: at the standard parallel the scale is 1.
        assert!((scale(Pole::North, 70.0) - 1.0).abs() < 1e-12);
        assert!((scale(Pole::South, -71.0) - 1.0).abs() < 1e-12);
        // And the scale at the pole is the limit of the scale near it.
        for p in [Pole::North, Pole::South] {
            assert!((scale(p, 90.0) - scale(p, 89.99999)).abs() < 1e-9);
        }
    }

    #[test]
    fn inverse_undoes_forward() {
        for p in [Pole::North, Pole::South] {
            let s = if p == Pole::North { 1.0 } else { -1.0 };
            for lat in [84.0, 85.0511, 87.3, 89.9999] {
                for lon in [-179.9, -45.0, 0.0, 12.5, 135.0, 180.0] {
                    let (x, y) = forward(p, lon, s * lat);
                    let (lo, la) = inverse(p, x, y);
                    assert!((la - s * lat).abs() < 1e-9, "{p:?} {lon} {lat}: {la}");
                    let dl = ((lo - lon + 540.0) % 360.0 - 180.0).abs();
                    assert!(dl < 1e-7, "{p:?} {lon} {lat}: {lo}");
                }
            }
        }
    }

    /// Geographic north and east on the grid are the forward projection's own directions.
    #[test]
    fn east_and_north_follow_the_projection() {
        for p in [Pole::North, Pole::South] {
            let s = if p == Pole::North { 1.0 } else { -1.0 };
            for lon in [-170.0, -45.0, 0.0, 60.0, 179.0] {
                let lat = s * 86.0;
                let (x0, y0) = forward(p, lon, lat);
                let (xe, ye) = forward(p, lon + 1e-6, lat);
                let (xn, yn) = forward(p, lon, lat + 1e-6);
                let (e, n) = east_north(p, lon);
                let (le, ln) = ((xe - x0).hypot(ye - y0), (xn - x0).hypot(yn - y0));
                assert!(
                    ((xe - x0) / le - e[0]).abs() < 1e-6 && ((ye - y0) / le - e[1]).abs() < 1e-6,
                    "{p:?} east at {lon}"
                );
                assert!(
                    ((xn - x0) / ln - n[0]).abs() < 1e-6 && ((yn - y0) / ln - n[1]).abs() < 1e-6,
                    "{p:?} north at {lon}"
                );
            }
        }
    }

    /// The caps' tiles: the pole's tile at every zoom, each zoom's the parents of the next's, and
    /// every pixel past the seam inside some tile.
    #[test]
    fn cap_tiles_nest_and_cover_the_seam() {
        for p in [Pole::North, Pole::South] {
            let mut prev = cap_tiles(p, 0);
            assert_eq!(prev, vec![(0, 0)]);
            for z in 1..=7u8 {
                let t = cap_tiles(p, z);
                let parents: std::collections::BTreeSet<(u32, u32)> = t.iter().map(|&(x, y)| (x / 2, y / 2)).collect();
                assert_eq!(parents, prev.iter().copied().collect(), "{p:?} z{z}");
                prev = t;
            }
            let r = rho(p, MERCATOR_LAT);
            assert!(r < rho(p, CAP_LAT) && rho(p, CAP_LAT) < HALF);
        }
    }

    #[test]
    fn pixels_lie_in_their_tiles() {
        let b = tile_box(3, 2, 5);
        let (x, y) = pixel_xy(3, 2, 5, 0.0, 0.0);
        assert!(x > b[0] && x < b[0] + pixel_m(3) && y < b[3] && y > b[3] - pixel_m(3));
        assert_eq!(pixel_m(9), 16.0);
    }
}
