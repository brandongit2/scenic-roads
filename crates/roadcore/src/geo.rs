//! The Earth and the Web Mercator, once for the workspace (docs/consistency-report.md F25, F29–F32):
//! the Earth's one radius, the projection's sphere, the projection and its inverse, a tile's box,
//! the box-grower, the even–odd point-in-polygon and the nonzero winding number. Every transcendental is `det`'s, so a value is
//! the same bits on every Mac and in wasm.

use det::Det;
use std::cmp::Ordering;
use std::f64::consts::PI;

/// The Earth's mean radius (m, IUGG): the one Earth every ground distance is measured on.
pub const EARTH_R: f64 = 6_371_008.8;
/// A degree of latitude on that sphere, and of longitude at the equator (m).
pub const M_PER_DEG: f64 = EARTH_R * PI / 180.0;
/// The same in km.
pub const KM_PER_DEG: f64 = M_PER_DEG / 1000.0;

/// WGS 84's semi-major axis (m) and inverse flattening: the ellipsoid of the reprojections.
pub const WGS84_A: f64 = 6_378_137.0;
pub const WGS84_RF: f64 = 298.257_223_563;

/// The Web Mercator's equator (m): the projection's sphere is WGS 84's semi-major axis (EPSG:3857),
/// so a world unit is this many of the projection's metres. Not a ground distance: that is
/// `EARTH_R`'s.
pub const MERC_C: f64 = 2.0 * PI * WGS84_A;

/// A degree along the equator in the projection's metres: a geographic file's pixel is measured
/// in these, as a tile pyramid's is (`merc_m`), so the two compare.
pub const MERC_M_PER_DEG: f64 = MERC_C / 360.0;

/// Where the Web Mercator tiles end (degrees from the equator): y = 0 and y = 1.
pub const MERCATOR_LAT: f64 = 85.051_128_779_806_59;

/// How far "near the coverage" reaches (km): a tile within it of the coverage is made (terrain,
/// slope, buildings), and a terrain target's coverage key reads the coverage that far round it.
pub const NEAR_COVERAGE_KM: f64 = 20.0;

/// Web Mercator world coordinates of a point (x east, y down; 0–1 between ±`MERCATOR_LAT`).
/// Past `MERCATOR_LAT` y runs on beyond 0 and 1, to its limit at ±89.2° (sin ±0.9999), so a point
/// near a pole is still finite; `merc_clamped` keeps y within the tiles.
#[inline]
pub fn merc(lon: f64, lat: f64) -> (f64, f64) {
    (merc_x(lon), merc_y(lat))
}

/// `merc` with the latitude clamped to ±`MERCATOR_LAT`: y within 0–1.
#[inline]
pub fn merc_clamped(lon: f64, lat: f64) -> (f64, f64) {
    merc(lon, lat.clamp(-MERCATOR_LAT, MERCATOR_LAT))
}

/// A longitude's world x (0–1 between ±180°).
#[inline]
pub fn merc_x(lon: f64) -> f64 {
    (lon + 180.0) / 360.0
}

/// A latitude's world y (as `merc`).
#[inline]
pub fn merc_y(lat: f64) -> f64 {
    let s = lat.to_radians().dsin().clamp(-0.9999, 0.9999);
    0.5 - ((1.0 + s) / (1.0 - s)).dln() / (4.0 * PI)
}

/// A world x's longitude.
#[inline]
pub fn merc_lon(x: f64) -> f64 {
    x * 360.0 - 180.0
}

/// A world y's latitude (degrees).
#[inline]
pub fn merc_lat(y: f64) -> f64 {
    merc_phi(y).to_degrees()
}

/// A world y's latitude in radians.
#[inline]
pub fn merc_phi(y: f64) -> f64 {
    (PI * (1.0 - 2.0 * y)).dsinh().datan()
}

/// A point in the projection's metres (EPSG:3857: x east and y north of lon 0 on the equator; as
/// `merc_clamped`).
#[inline]
pub fn epsg3857(lon: f64, lat: f64) -> [f64; 2] {
    let (x, y) = merc_clamped(lon, lat);
    [(x - 0.5) * MERC_C, (0.5 - y) * MERC_C]
}

/// `epsg3857`'s inverse: the longitude and latitude of a point in the projection's metres.
#[inline]
pub fn epsg3857_inv(x: f64, y: f64) -> [f64; 2] {
    [merc_lon(x / MERC_C + 0.5), merc_lat(0.5 - y / MERC_C)]
}

/// Metres of the projection a world unit spans along the parallel at `lat`: a tile's or a pixel's
/// width there, divided by the tiles or pixels across the world.
#[inline]
pub fn merc_m(lat: f64) -> f64 {
    MERC_C * lat.to_radians().dcos()
}

/// Tile `z`/`x`/`y`'s box in degrees: [west, south, east, north].
pub fn tile_box(z: u8, x: u32, y: u32) -> [f64; 4] {
    let n = (1u64 << z) as f64;
    let (x, y) = (x as f64, y as f64);
    [merc_lon(x / n), merc_lat((y + 1.0) / n), merc_lon((x + 1.0) / n), merc_lat(y / n)]
}

/// The zoom-`z` tile holding a point in world coordinates.
pub fn tile_of(w: (f64, f64), z: u8) -> (u32, u32) {
    let n = (1u64 << z) as f64;
    let c = |v: f64| ((v * n).floor().max(0.0) as u64).min((1u64 << z) - 1) as u32;
    (c(w.0), c(w.1))
}

/// The tiles at `z` meeting the box (w, s, e, n in degrees).
pub fn tiles_in(z: u8, b: [f64; 4]) -> Vec<(u32, u32)> {
    let [x0, x1, y0, y1] = tile_range(z, b);
    (x0..=x1).flat_map(|tx| (y0..=y1).map(move |ty| (tx, ty))).collect()
}

/// `tiles_in` as ranges: [x0, x1, y0, y1], both ends in.
pub fn tile_range(z: u8, b: [f64; 4]) -> [u32; 4] {
    let n = 1u32 << z;
    // (A billionth of a tile up before the floor: a box edge on a tile boundary, from
    // tile_box_grown's round trip through degrees, can come back a hair below it
    // and would take in the tile before.)
    let tile = |t: f64| ((t * n as f64 + 1e-9).floor().max(0.0) as u32).min(n - 1);
    let x = |lon: f64| tile(merc_x(lon));
    let y = |lat: f64| tile(merc_clamped(0.0, lat).1);
    [x(b[0]), x(b[2]), y(b[3]), y(b[1])]
}

/// A box ([west, south, east, north], degrees) grown by `km` on every side: `km` of latitude each
/// way (stopping at the poles), and of longitude at the grown box's latitude farthest from the
/// equator (`lon_span`), taken as `GROW_LAT` past it: a box reaching a pole would otherwise take in
/// every longitude (a point within `km` of it past `GROW_LAT` off its longitudes is left out).
/// Longitudes may run past ±180: the caller wraps or clamps them.
pub fn grow(b: [f64; 4], km: f64) -> [f64; 4] {
    let dlat = km / KM_PER_DEG;
    let (s, n) = ((b[1] - dlat).max(-90.0), (b[3] + dlat).min(90.0));
    let dlon = lon_span(km, s.abs().max(n.abs()).min(GROW_LAT));
    [b[0] - dlon, s, b[2] + dlon, n]
}

/// The latitude `grow` takes a box's longitudes grown at, at most.
const GROW_LAT: f64 = 89.0;

/// The degrees of longitude `km` spans along the parallel at `lat`: half a turn at most (a pole's
/// circle shorter than that: every longitude).
pub fn lon_span(km: f64, lat: f64) -> f64 {
    let c = lat.abs().min(90.0).to_radians().dcos();
    if c * 180.0 * KM_PER_DEG <= km {
        180.0
    } else {
        km / (KM_PER_DEG * c)
    }
}

/// Tile `z`/`x`/`y`'s box grown by `km` (`grow`), its latitudes kept within the tiles
/// (±`MERCATOR_LAT`).
pub fn tile_box_grown(z: u8, x: u32, y: u32, km: f64) -> [f64; 4] {
    within_tiles(grow(tile_box(z, x, y), km))
}

/// Tile `z`/`x`/`y`'s box grown by `km` (`tile_box_grown`), E7.
pub fn grown_e7(z: u8, x: u32, y: u32, km: f64) -> [i32; 4] {
    e7_box(tile_box_grown(z, x, y, km))
}

/// A box's latitudes kept within the tiles (±`MERCATOR_LAT`).
pub fn within_tiles(b: [f64; 4]) -> [f64; 4] {
    [b[0], b[1].max(-MERCATOR_LAT), b[2], b[3].min(MERCATOR_LAT)]
}

/// A box in degrees as E7 (rounded), its longitudes clamped to ±180.
pub fn e7_box(b: [f64; 4]) -> [i32; 4] {
    let lon = |v: f64| (v.clamp(-180.0, 180.0) * 1e7).round() as i32;
    let lat = |v: f64| (v * 1e7).round() as i32;
    [lon(b[0]), lat(b[1]), lon(b[2]), lat(b[3])]
}

/// A coordinate the even–odd test takes: the sign of the cross product
/// (p − a) × (b − a), exact for integers.
pub trait Coord: Copy + PartialOrd {
    fn side(a: [Self; 2], b: [Self; 2], p: [Self; 2]) -> Ordering;
}

impl Coord for i32 {
    fn side(a: [i32; 2], b: [i32; 2], p: [i32; 2]) -> Ordering {
        let w = |v: i32| v as i128;
        ((w(p[0]) - w(a[0])) * (w(b[1]) - w(a[1]))).cmp(&((w(p[1]) - w(a[1])) * (w(b[0]) - w(a[0]))))
    }
}

impl Coord for i64 {
    fn side(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> Ordering {
        let w = |v: i64| v as i128;
        ((w(p[0]) - w(a[0])) * (w(b[1]) - w(a[1]))).cmp(&((w(p[1]) - w(a[1])) * (w(b[0]) - w(a[0]))))
    }
}

impl Coord for f64 {
    fn side(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> Ordering {
        ((p[0] - a[0]) * (b[1] - a[1])).total_cmp(&((p[1] - a[1]) * (b[0] - a[0])))
    }
}

/// Whether `p` is inside `rings` read even–odd (holes, islands in lakes and multipolygons alike;
/// rings closed or open, in either orientation). A point on an edge may fall either way, the same
/// way for every caller: a ring edge counts where it crosses p's row strictly east of p.
pub fn inside<T: Coord, R: AsRef<[[T; 2]]>>(rings: &[R], p: [T; 2]) -> bool {
    let mut ins = false;
    for r in rings {
        let r = r.as_ref();
        let n = r.len();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let east = if b[1] > a[1] { Ordering::Less } else { Ordering::Greater };
                if T::side(a, b, p) == east {
                    ins = !ins;
                }
            }
        }
    }
    ins
}

/// The nonzero winding number of `rings` at `p` (each edge counted as `inside` counts it, +1
/// upward, −1 downward): nonzero inside the union of rings wound alike, holes wound against their
/// outer ring. For rings that may overlap, wound by a rule (a basemap's polygons, each tile's with
/// its buffer, as the lake fill reads them), where even–odd would call their overlap outside.
pub fn winding<T: Coord, R: AsRef<[[T; 2]]>>(rings: &[R], p: [T; 2]) -> i32 {
    let mut w = 0;
    for r in rings {
        let r = r.as_ref();
        let n = r.len();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let up = b[1] > a[1];
                if T::side(a, b, p) == if up { Ordering::Less } else { Ordering::Greater } {
                    w += if up { 1 } else { -1 };
                }
            }
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ranges() {
        // The z6 tile of Newcastle (6/31/19) grown by 30 km meets its neighbours.
        let b = tile_box_grown(6, 31, 19, 30.0);
        let t = tiles_in(6, b);
        assert!(t.contains(&(31, 19)) && t.contains(&(30, 19)) && t.contains(&(32, 20)), "{t:?}");
        assert_eq!(tiles_in(11, tile_box(6, 31, 19)).len(), 33 * 33);
    }

    #[test]
    fn mercator_round_trips_and_ends_at_the_tiles() {
        for lat in [-85.0, -45.5, 0.0, 12.25, 70.5, 85.0] {
            assert!((merc_lat(merc_y(lat)) - lat).abs() < 1e-9, "{lat}");
        }
        assert!(merc_y(MERCATOR_LAT).abs() < 1e-12);
        assert!((merc_y(-MERCATOR_LAT) - 1.0).abs() < 1e-12);
        assert_eq!(merc_clamped(0.0, 89.0).1, merc_y(MERCATOR_LAT));
        assert!(merc(0.0, 89.0).1 < 0.0 && merc(0.0, 90.0).1.is_finite());
        assert_eq!(merc_x(-180.0), 0.0);
        assert_eq!(merc_lon(merc_x(33.0)), 33.0);
        let b = tile_box(6, 33, 21);
        assert_eq!(b[0], 5.625);
        assert!((merc_y(b[3]) * 64.0 - 21.0).abs() < 1e-9 && (merc_y(b[1]) * 64.0 - 22.0).abs() < 1e-9);
    }

    #[test]
    fn a_box_grows_by_its_km_at_its_farthest_latitude() {
        let g = grow([10.0, 45.0, 11.0, 46.0], NEAR_COVERAGE_KM);
        let dlat = NEAR_COVERAGE_KM / KM_PER_DEG;
        assert_eq!([g[1], g[3]], [45.0 - dlat, 46.0 + dlat]);
        assert!((g[2] - 11.0 - dlat / (46.0 + dlat).to_radians().dcos()).abs() < 1e-12);
        let p = grow([0.0, 88.0, 1.0, 89.9], 50.0);
        assert_eq!(p[3], 90.0);
        assert_eq!(p[2] - 1.0, lon_span(50.0, GROW_LAT), "a box reaching the pole grown at 89°");
        assert_eq!(lon_span(KM_PER_DEG, 0.0), 1.0);
        assert_eq!(lon_span(1.0, 90.0), 180.0);
        let t = tile_box_grown(6, 0, 0, NEAR_COVERAGE_KM);
        assert_eq!(t[3], MERCATOR_LAT);
        assert_eq!(e7_box([-190.0, -1.0, 190.0, 1.0]), [-1_800_000_000, -10_000_000, 1_800_000_000, 10_000_000]);
    }

    #[test]
    fn even_odd_holes_and_edges_alike_for_every_type() {
        let outer = [[0, 0], [10, 0], [10, 10], [0, 10]];
        let hole = [[3, 3], [3, 7], [7, 7], [7, 3], [3, 3]];
        let rings: Vec<&[[i32; 2]]> = vec![&outer, &hole];
        assert!(inside(&rings, [1, 1]));
        assert!(!inside(&rings, [5, 5]));
        assert!(!inside(&rings, [11, 5]));
        let f: Vec<Vec<[f64; 2]>> =
            rings.iter().map(|r| r.iter().map(|p| [p[0] as f64, p[1] as f64]).collect()).collect();
        let l: Vec<Vec<[i64; 2]>> =
            rings.iter().map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect()).collect();
        // Every point of a fine grid, edges and vertices included, the same answer in each type.
        for x in -2..=24 {
            for y in -2..=24 {
                let (fx, fy) = (x as f64 / 2.0, y as f64 / 2.0);
                let want = inside(&f, [fx, fy]);
                if x % 2 == 0 && y % 2 == 0 {
                    assert_eq!(inside(&rings, [x / 2, y / 2]), want, "{x} {y}");
                    assert_eq!(inside(&l, [x as i64 / 2, y as i64 / 2]), want, "{x} {y}");
                }
            }
        }
        // Huge E7 coordinates stay exact.
        let big = [[-1_800_000_000, -900_000_000], [1_800_000_000, -900_000_000], [0, 900_000_000]];
        assert!(inside(&[&big[..]], [0, 0]));
        assert!(!inside(&[&big[..]], [1_799_999_999, 899_999_999]));
    }

    #[test]
    fn winding_counts_overlaps_and_holes() {
        // Two squares wound alike overlapping (a tile's polygon and its neighbour's buffered copy),
        // a hole wound against the first.
        let a = [[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]];
        let b = [[8.0, 0.0], [8.0, 10.0], [18.0, 10.0], [18.0, 0.0]];
        let hole = [[2.0, 2.0], [4.0, 2.0], [4.0, 4.0], [2.0, 4.0]];
        let rings: Vec<&[[f64; 2]]> = vec![&a, &b, &hole];
        assert_eq!(winding(&rings, [9.0, 5.0]).abs(), 2);
        assert!(!inside(&rings, [9.0, 5.0]), "even–odd: the overlap outside");
        assert_eq!(winding(&rings, [3.0, 3.0]), 0);
        assert_eq!(winding(&rings, [20.0, 5.0]), 0);
        assert_eq!(winding(&rings, [1.0, 5.0]).abs(), 1);
        // Wherever rings don't overlap, the two rules agree.
        let one: Vec<&[[f64; 2]]> = vec![&a, &hole];
        for x in -1..=11 {
            for y in -1..=11 {
                let p = [x as f64 + 0.5, y as f64 + 0.5];
                assert_eq!(winding(&one, p) != 0, inside(&one, p), "{p:?}");
            }
        }
    }
}
