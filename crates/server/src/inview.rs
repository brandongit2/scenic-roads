//! The ground in view, from the outline the client sends (web/src/main.ts groundOutline: lon, lat
//! points around the screen), as the "in view" lists (drives, rides, rail lines) and landmarks test
//! it; web/src/inview.ts is the same in the app. In longitude and latitude the outline of a view
//! around a pole or across the antimeridian isn't a plain polygon (its longitudes wrap, or circle
//! the pole), so the test is made on the stereographic plane centred in the outline, where it is
//! one; the far hemisphere, behind the planet, is outside (#826). Without an outline, the bounds
//! (west > east: across the antimeridian).

use roadcore::E7;

/// The part of the map in view.
pub struct Region {
    /// The outline on the plane (empty: none, the bounds only).
    pts: Vec<[f64; 2]>,
    /// The plane's centre: longitude (radians), sine and cosine of latitude.
    centre: (f64, f64, f64),
    /// The outline's box on the plane.
    pbox: [f64; 4],
    /// Boxes (west ≤ east, degrees) holding the area: two across the antimeridian, every
    /// longitude round a pole.
    boxes: Vec<[f64; 4]>,
}

/// Longitude into −180…180.
fn wrap(x: f64) -> f64 {
    (x + 180.0).rem_euclid(360.0) - 180.0
}

/// A box (west > east: across the antimeridian) as boxes with west ≤ east.
fn split([w, s, e, n]: [f64; 4]) -> Vec<[f64; 4]> {
    if w <= e {
        vec![[w, s, e, n]]
    } else {
        vec![[w, s, 180.0, n], [-180.0, s, e, n]]
    }
}

impl Region {
    /// `poly`: the outline (degrees), used with at least 3 points; else `bounds` (west, south,
    /// east, north).
    pub fn new(poly: Vec<[f64; 2]>, bounds: [f64; 4]) -> Region {
        if poly.len() < 3 {
            let [w, s, e, n] = bounds;
            let b = if e - w >= 360.0 { [-180.0, s, 180.0, n] } else { [wrap(w), s, wrap(e), n] };
            return Region { pts: Vec::new(), centre: (0.0, 0.0, 1.0), pbox: [0.0; 4], boxes: split(b) };
        }
        let (mut cx, mut cy, mut cz) = (0.0, 0.0, 0.0);
        for &[x, y] in &poly {
            let (x, y) = (x.to_radians(), y.to_radians());
            cx += y.cos() * x.cos();
            cy += y.cos() * x.sin();
            cz += y.sin();
        }
        let s0 = cz / (cx * cx + cy * cy + cz * cz).sqrt();
        let mut r = Region {
            pts: Vec::new(),
            centre: (cy.atan2(cx), s0, (1.0 - s0 * s0).max(0.0).sqrt()),
            pbox: [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
            boxes: Vec::new(),
        };
        r.pts = poly.iter().filter_map(|&[x, y]| r.plane(x, y)).collect();
        for &[x, y] in &r.pts {
            r.pbox = [r.pbox[0].min(x), r.pbox[1].min(y), r.pbox[2].max(x), r.pbox[3].max(y)];
        }
        // The box: the longitudes unwrapped along the outline (each within 180° of the one before).
        let (mut s, mut n, mut u) = (f64::INFINITY, f64::NEG_INFINITY, poly[0][0]);
        let (mut w, mut e) = (u, u);
        for (i, &[x, y]) in poly.iter().enumerate() {
            if i > 0 {
                u += wrap(x - poly[i - 1][0]);
            }
            (w, e, s, n) = (w.min(u), e.max(u), s.min(y), n.max(y));
        }
        u += wrap(poly[0][0] - poly[poly.len() - 1][0]);
        let turns = ((u - poly[0][0]) / 360.0).round() as i32;
        let b = if turns != 0 && r.test(0.0, 90.0) {
            [-180.0, s, 180.0, 90.0]
        } else if turns != 0 && r.test(0.0, -90.0) {
            [-180.0, -90.0, 180.0, n]
        } else if turns != 0 || e - w >= 360.0 {
            [-180.0, s, 180.0, n]
        } else {
            [wrap(w), s, wrap(e), n]
        };
        r.boxes = split(b);
        r
    }

    /// `bbox`: west,south,east,north; `poly`: lon,lat,lon,lat,… (degrees), at least 3 points.
    pub fn parse(bbox: &str, poly: Option<&str>) -> Option<Region> {
        let pts: Vec<[f64; 2]> = poly
            .map(|p| {
                let v: Vec<f64> = p.split(',').filter_map(|x| x.parse().ok()).collect();
                v.chunks_exact(2).map(|c| [c[0], c[1]]).collect()
            })
            .unwrap_or_default();
        let b: Vec<f64> = bbox.split(',').filter_map(|x| x.parse().ok()).collect();
        if pts.len() < 3 && b.len() != 4 {
            return None;
        }
        let bounds = if b.len() == 4 { [b[0], b[1], b[2], b[3]] } else { [-180.0, -90.0, 180.0, 90.0] };
        Some(Region::new(pts, bounds))
    }

    /// A point (degrees) on the plane; None on the far side's centre.
    fn plane(&self, lon: f64, lat: f64) -> Option<[f64; 2]> {
        let (l0, s0, c0) = self.centre;
        let dl = lon.to_radians() - l0;
        let (s, c, cd) = (lat.to_radians().sin(), lat.to_radians().cos(), dl.cos());
        let q = 1.0 + s0 * s + c0 * c * cd;
        if q < 1e-9 {
            return None;
        }
        let k = 2.0 / q;
        Some([k * c * dl.sin(), k * (c0 * s - s0 * c * cd)])
    }

    /// Point (degrees) in view.
    pub fn test(&self, lon: f64, lat: f64) -> bool {
        if self.pts.len() < 3 {
            return self.boxes.iter().any(|&[w, s, e, n]| lon >= w && lon <= e && lat >= s && lat <= n);
        }
        let Some([x, y]) = self.plane(lon, lat) else { return false };
        let [w, s, e, n] = self.pbox;
        if x < w || x > e || y < s || y > n {
            return false;
        }
        let p = &self.pts;
        let mut inside = false;
        let mut j = p.len() - 1;
        for i in 0..p.len() {
            let ([xi, yi], [xj, yj]) = (p[i], p[j]);
            if (yi > y) != (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    /// Point (E7 degrees) in view.
    pub fn contains(&self, lon: i32, lat: i32) -> bool {
        self.test(lon as f64 * E7, lat as f64 * E7)
    }

    /// Boxes (west ≤ east, degrees) holding the area.
    pub fn boxes(&self) -> &[[f64; 4]] {
        &self.boxes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ring of `k` points at `r` degrees (great-circle) round (lon, lat).
    fn ring(lon: f64, lat: f64, r: f64, k: usize) -> Vec<[f64; 2]> {
        let (l0, p0, d) = (lon.to_radians(), lat.to_radians(), r.to_radians());
        (0..k)
            .map(|i| {
                let b = i as f64 / k as f64 * std::f64::consts::TAU;
                let p = (p0.sin() * d.cos() + p0.cos() * d.sin() * b.cos()).asin();
                let l = l0 + (b.sin() * d.sin() * p0.cos()).atan2(d.cos() - p0.sin() * p.sin());
                [wrap(l.to_degrees()), p.to_degrees()]
            })
            .collect()
    }

    #[test]
    fn an_ordinary_view() {
        let r = Region::new(vec![[-75.0, 45.0], [-70.0, 45.0], [-70.0, 48.0], [-75.0, 48.0]], [0.0; 4]);
        assert!(r.test(-72.0, 46.0));
        assert!(!r.test(-76.0, 46.0) && !r.test(-72.0, 49.0) && !r.test(108.0, -46.0));
        assert!(r.contains(-720_000_000, 460_000_000));
        let b = r.boxes();
        assert_eq!(b.len(), 1);
        assert!((b[0][0] + 75.0).abs() < 1e-9 && (b[0][2] + 70.0).abs() < 1e-9);
    }

    #[test]
    fn a_view_round_the_south_pole() {
        let r = Region::new(ring(30.0, -88.0, 5.0, 40), [0.0; 4]);
        assert!(r.test(0.0, -90.0) && r.test(-150.0, -88.0) && r.test(30.0, -84.0));
        assert!(!r.test(30.0, -82.0) && !r.test(-150.0, -84.0) && !r.test(0.0, 90.0));
        assert_eq!(r.boxes(), &[[-180.0, -90.0, 180.0, r.boxes()[0][3]]]);
        assert!((r.boxes()[0][3] + 83.0).abs() < 0.01);
        let r = Region::new(ring(0.0, 89.0, 3.0, 40), [0.0; 4]);
        assert!(r.test(120.0, 89.5) && !r.test(180.0, 85.0) && !r.test(0.0, -90.0));
        assert_eq!(r.boxes()[0][3], 90.0);
    }

    #[test]
    fn a_view_across_the_antimeridian() {
        let r = Region::new(ring(180.0, -40.0, 3.0, 40), [0.0; 4]);
        assert!(r.test(179.0, -40.0) && r.test(-179.0, -40.0) && r.test(180.0, -41.0));
        assert!(!r.test(0.0, -40.0) && !r.test(170.0, -40.0) && !r.test(-170.0, -40.0));
        let b = r.boxes();
        assert_eq!(b.len(), 2);
        assert!(b[0][0] > 175.0 && b[0][2] == 180.0 && b[1][0] == -180.0 && b[1][2] < -175.0);
    }

    #[test]
    fn without_an_outline_the_bounds() {
        let r = Region::new(Vec::new(), [170.0, -10.0, -170.0, 10.0]);
        assert!(r.test(175.0, 0.0) && r.test(-175.0, 0.0) && !r.test(0.0, 0.0));
        assert_eq!(r.boxes().len(), 2);
        assert!(Region::parse("1,2,3,4", None).is_some_and(|r| r.test(2.0, 3.0)));
        assert!(Region::parse("x", None).is_none());
    }
}
