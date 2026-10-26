//! The coordinate conversions the DEMs need, computed as PROJ 9.8
//! computes them, with the deterministic maths (`det`): WGS 84 to Canada Atlas Lambert (EPSG:3979,
//! NRCan's HRDEM) through NAD83(CSRS), by EPSG's 7-parameter "NAD83(CSRS) to WGS 84 (2)"
//! (the transformation PROJ picks, everywhere: it needs no grid); and to NAD83 / UTM (EPSG:26901–
//! 26923, USGS 3DEP's 1 m tiles) through the same Helmert, by PROJ's Poder/Engsager transverse
//! Mercator (`Utm`).
//!
//! PROJ's build fuses a multiply and an add in one expression into a multiply-add (clang's
//! contraction: the left operand's product if it's one, else the right's), so those are fused here
//! too (`mul_add`, exact on every target). What's left is the last bits of the transcendental
//! functions (Apple's libm against libm's): within 6 nm of pyproj's.

use det::Det;
use std::f64::consts::{FRAC_PI_2, PI};

/// PROJ's degree, as a constant.
const DEG_TO_RAD: f64 = 0.017453292519943296;

/// An ellipsoid's constants, as PROJ derives them from its axis and inverse flattening.
#[derive(Clone, Copy, Debug)]
pub struct Ellipsoid {
    a: f64,
    f: f64,
    es: f64,
    e: f64,
    e2s: f64,
    ra: f64,
}

impl Ellipsoid {
    pub fn new(a: f64, rf: f64) -> Ellipsoid {
        let f = 1.0 / rf;
        let es = 2.0f64.mul_add(f, -(f * f));
        let e = es.sqrt();
        let alpha = e.dasin();
        let e2 = alpha.dtan();
        Ellipsoid { a, f, es, e, e2s: e2 * e2, ra: 1.0 / a }
    }

    pub const WGS84: (f64, f64) = (6378137.0, 298.257223563);
    pub const GRS80: (f64, f64) = (6378137.0, 298.257222101);

    /// Geodetic (radians, height 0) to geocentric.
    fn cartesian(&self, lam: f64, phi: f64) -> [f64; 3] {
        let (cosphi, sinphi) = (phi.dcos(), phi.dsin());
        let n = self.a / (-(self.es * sinphi)).mul_add(sinphi, 1.0).sqrt();
        [n * cosphi * lam.dcos(), n * cosphi * lam.dsin(), n.mul_add(1.0 - self.es, 0.0) * sinphi]
    }

    /// Geocentric to geodetic (radians), PROJ's `cart` inverse (Bowring).
    fn geodetic(&self, c: [f64; 3]) -> (f64, f64) {
        let (x, y, z) = (c[0] * self.ra, c[1] * self.ra, c[2] * self.ra);
        let p = x.mul_add(x, y * y).sqrt();
        let bda = 1.0 - self.f;
        let pb = p * bda;
        let norm = z.mul_add(z, pb * pb).sqrt();
        let (cc, s) = if norm != 0.0 {
            let inv = 1.0 / norm;
            (pb * inv, z * inv)
        } else {
            (1.0, 0.0)
        };
        let y_phi = (self.e2s * bda * s * s).mul_add(s, z);
        let x_phi = (-(self.es * cc * cc)).mul_add(cc, p);
        let phi = if x_phi <= 0.0 {
            if c[2] >= 0.0 {
                FRAC_PI_2
            } else {
                -FRAC_PI_2
            }
        } else {
            (y_phi / x_phi).datan()
        };
        (y.datan2(x), phi)
    }
}

/// PROJ's `pj_tsfn`.
fn tsfn(phi: f64, sinphi: f64, e: f64) -> f64 {
    let cosphi = phi.dcos();
    (e * (e * sinphi).datanh()).dexp() * if sinphi > 0.0 { cosphi / (1.0 + sinphi) } else { (1.0 - sinphi) / cosphi }
}

/// PROJ's `pj_msfn`.
fn msfn(sinphi: f64, cosphi: f64, es: f64) -> f64 {
    cosphi / (-(es * sinphi)).mul_add(sinphi, 1.0).sqrt()
}

/// PROJ's `adjlon`.
fn adjlon(l: f64) -> f64 {
    if l.abs() < PI + 1e-12 {
        return l;
    }
    let l = l + PI;
    let l = (-2.0 * PI).mul_add((l / (2.0 * PI)).floor(), l);
    l - PI
}

/// WGS 84 to NAD83(CSRS) by EPSG's "NAD83(CSRS) to WGS 84 (2)" (EPSG:1946), inverted: geocentric,
/// the Helmert, back to geodetic on GRS 80.
#[derive(Clone, Debug)]
struct Csrs {
    wgs: Ellipsoid,
    grs: Ellipsoid,
    /// The Helmert translation (m) and rotations (radians; coordinate frame, small angles).
    t: [f64; 3],
    r: [f64; 3],
}

impl Csrs {
    fn new() -> Csrs {
        let arc = DEG_TO_RAD / 3600.0;
        Csrs {
            wgs: Ellipsoid::new(Ellipsoid::WGS84.0, Ellipsoid::WGS84.1),
            grs: Ellipsoid::new(Ellipsoid::GRS80.0, Ellipsoid::GRS80.1),
            // EPSG:1946's parameters, as PROJ writes them.
            t: [-0.991, 1.9072, 0.5129],
            r: [-0.0257899075194932 * arc, -0.0096500989602704 * arc, -0.0116599432323421 * arc],
        }
    }

    /// NAD83(CSRS) longitude, latitude (radians) of WGS 84 `lon`, `lat` (degrees).
    fn of(&self, lon: f64, lat: f64) -> (f64, f64) {
        let c = self.wgs.cartesian(lon * DEG_TO_RAD, lat * DEG_TO_RAD);
        let (x, y, z) = (c[0] - self.t[0], c[1] - self.t[1], c[2] - self.t[2]);
        let [f, t, p] = self.r;
        // The rotation matrix transposed: R00 = R11 = R22 = 1, R01 = p, R02 = -t, R10 = -p,
        // R12 = f, R20 = t, R21 = -f.
        let local = [
            t.mul_add(z, 1.0f64.mul_add(x, -p * y)),
            (-f).mul_add(z, p.mul_add(x, 1.0 * y)),
            1.0f64.mul_add(z, (-t).mul_add(x, f * y)),
        ];
        self.grs.geodetic(local)
    }
}

/// WGS 84 longitude, latitude (degrees) to Canada Atlas Lambert (EPSG:3979) x, y (metres), as
/// pyproj's `Transformer.from_crs("EPSG:4326", "EPSG:3979", always_xy=True)` does.
#[derive(Clone, Debug)]
pub struct Atlas {
    csrs: Csrs,
    n: f64,
    c: f64,
    rho0: f64,
    lam0: f64,
}

impl Default for Atlas {
    fn default() -> Self {
        Self::new()
    }
}

impl Atlas {
    pub fn new() -> Atlas {
        let csrs = Csrs::new();
        let grs = csrs.grs;
        // Lambert conformal conic: standard parallels 49° and 77° N, origin 49° N 95° W, GRS 80.
        let (phi1, phi2, phi0, lam0) = (49.0 * DEG_TO_RAD, 77.0 * DEG_TO_RAD, 49.0 * DEG_TO_RAD, -(95.0 * DEG_TO_RAD));
        let (sinphi, cosphi) = (phi1.dsin(), phi1.dcos());
        let m1 = msfn(sinphi, cosphi, grs.es);
        let ml1 = tsfn(phi1, sinphi, grs.e);
        let s2 = phi2.dsin();
        let mut n = (m1 / msfn(s2, phi2.dcos(), grs.es)).dln();
        let ml2 = tsfn(phi2, s2, grs.e);
        n /= (ml1 / ml2).dln();
        let c = m1 * ml1.dpowf(-n) / n;
        let rho0 = c * tsfn(phi0, phi0.dsin(), grs.e).dpowf(n);
        Atlas { csrs, n, c, rho0, lam0 }
    }

    /// EPSG:3979 x, y (m) of WGS 84 `lon`, `lat` (degrees).
    pub fn project(&self, lon: f64, lat: f64) -> (f64, f64) {
        let (lam, phi) = self.csrs.of(lon, lat);
        let grs = &self.csrs.grs;
        // Lambert conformal conic.
        let lam = adjlon(adjlon(lam) - self.lam0);
        let rho = self.c * tsfn(phi, phi.dsin(), grs.e).dpowf(self.n);
        let l = lam * self.n;
        let x = rho * l.dsin();
        let y = (-rho).mul_add(l.dcos(), self.rho0);
        (x * grs.a, y * grs.a)
    }
}

/// The projected CRSs the DEMs are read in: Canada Atlas Lambert, a NAD83 / UTM zone, or a WGS 84
/// polar stereographic (EPSG:3031, Antarctic: REMA; EPSG:3413, the north: roadcore::polar, the
/// polar caps' own grids, docs/polar.md).
#[derive(Clone, Debug)]
pub enum Projection {
    Atlas(Box<Atlas>),
    Utm(Utm),
    Polar(roadcore::polar::Pole),
}

impl Projection {
    /// The projection of EPSG code `crs`; None for a geographic CRS (4326, 4269: lon, lat as given)
    /// or one the app doesn't know.
    pub fn of(crs: u16) -> Option<Projection> {
        match crs {
            3979 => Some(Projection::Atlas(Box::new(Atlas::new()))),
            26901..=26923 => Some(Projection::Utm(Utm::new(u32::from(crs - 26900)))),
            3031 => Some(Projection::Polar(roadcore::polar::Pole::South)),
            3413 => Some(Projection::Polar(roadcore::polar::Pole::North)),
            _ => None,
        }
    }

    /// x, y (m) of WGS 84 `lon`, `lat` (degrees).
    pub fn project(&self, lon: f64, lat: f64) -> (f64, f64) {
        match self {
            Projection::Atlas(a) => a.project(lon, lat),
            Projection::Utm(u) => u.project(lon, lat),
            Projection::Polar(p) => roadcore::polar::forward(*p, lon, lat),
        }
    }
}

/// Whether EPSG code `crs` is one the app projects to (metres), not a geographic one.
pub fn projected(crs: u16) -> bool {
    matches!(crs, 3979 | 3031 | 3413 | 26901..=26923)
}

/// PROJ's Clenshaw sum of `p` at `b` (`gatg`).
fn gatg(p: &[f64; 6], b: f64, cos_2b: f64, sin_2b: f64) -> f64 {
    let two_cos_2b = 2.0 * cos_2b;
    let (mut h, mut h2) = (0.0, 0.0);
    let mut h1 = p[5];
    for &c in p[..5].iter().rev() {
        h = -h2 + two_cos_2b * h1 + c;
        h2 = h1;
        h1 = h;
    }
    b + h * sin_2b
}

/// PROJ's complex Clenshaw sum (`clenS`): (real, imaginary).
fn clen_s(a: &[f64; 6], sin_r: f64, cos_r: f64, sinh_i: f64, cosh_i: f64) -> (f64, f64) {
    let r = 2.0 * cos_r * cosh_i;
    let i = -2.0 * sin_r * sinh_i;
    let (mut hi1, mut hr1, mut hi) = (0.0, 0.0, 0.0);
    let mut hr = a[5];
    for &c in a[..5].iter().rev() {
        let (hr2, hi2) = (hr1, hi1);
        hr1 = hr;
        hi1 = hi;
        hr = -hr2 + r * hr1 - i * hi1 + c;
        hi = -hi2 + i * hr1 + r * hi1;
    }
    let r = sin_r * cosh_i;
    let i = cos_r * sinh_i;
    (r * hr - i * hi, r * hi + i * hr)
}

/// PROJ's real Clenshaw sum (`clens`).
fn clens(a: &[f64; 6], arg: f64) -> f64 {
    let r = 2.0 * arg.dcos();
    let mut hr1 = 0.0;
    let mut hr = a[5];
    for &c in a[..5].iter().rev() {
        let hr2 = hr1;
        hr1 = hr;
        hr = -hr2 + r * hr1 + c;
    }
    arg.dsin() * hr
}

/// WGS 84 longitude, latitude (degrees) to NAD83 / UTM zone `zone` north (EPSG:269zz) x, y (metres),
/// NAD83 taken as NAD83(CSRS) (USGS's NAD83(2011) and it one frame, as crate::dem::geoid takes
/// them), so through the Atlas's Helmert: as pyproj's `Transformer.from_crs("EPSG:4326",
/// "EPSG:<NAD83(CSRS) / UTM zone>", always_xy=True)` does. PROJ's `tmerc` by Poder/Engsager (its
/// default algorithm), on GRS 80, k0 0.9996, false easting 500 km. (PROJ's own pick for EPSG:269zz,
/// "NAD83 to WGS 84 (1)", is a null transformation: about a metre off in the north-east, where
/// HRDEM, read through the Helmert, meets the 1 m tiles.)
#[derive(Clone, Debug)]
pub struct Utm {
    csrs: Csrs,
    lam0: f64,
    qn: f64,
    zb: f64,
    cbg: [f64; 6],
    gtu: [f64; 6],
}

impl Utm {
    pub fn new(zone: u32) -> Utm {
        let grs = Ellipsoid::new(Ellipsoid::GRS80.0, Ellipsoid::GRS80.1);
        let n = grs.f / (2.0 - grs.f);
        let mut np = n;
        let mut cbg = [0.0; 6];
        let mut gtu = [0.0; 6];
        cbg[0] = n
            * (-2.0
                + n * (2.0 / 3.0 + n * (4.0 / 3.0 + n * (-82.0 / 45.0 + n * (32.0 / 45.0 + n * (4642.0 / 4725.0))))));
        np *= n;
        cbg[1] =
            np * (5.0 / 3.0 + n * (-16.0 / 15.0 + n * (-13.0 / 9.0 + n * (904.0 / 315.0 + n * (-1522.0 / 945.0)))));
        np *= n;
        cbg[2] = np * (-26.0 / 15.0 + n * (34.0 / 21.0 + n * (8.0 / 5.0 + n * (-12686.0 / 2835.0))));
        np *= n;
        cbg[3] = np * (1237.0 / 630.0 + n * (-12.0 / 5.0 + n * (-24832.0 / 14175.0)));
        np *= n;
        cbg[4] = np * (-734.0 / 315.0 + n * (109598.0 / 31185.0));
        np *= n;
        cbg[5] = np * (444337.0 / 155925.0);
        let k0 = 0.9996;
        np = n * n;
        let qn = k0 / (1.0 + n) * (1.0 + np * (1.0 / 4.0 + np * (1.0 / 64.0 + np / 256.0)));
        gtu[0] = n
            * (0.5
                + n * (-2.0 / 3.0
                    + n * (5.0 / 16.0 + n * (41.0 / 180.0 + n * (-127.0 / 288.0 + n * (7891.0 / 37800.0))))));
        gtu[1] = np
            * (13.0 / 48.0
                + n * (-3.0 / 5.0 + n * (557.0 / 1440.0 + n * (281.0 / 630.0 + n * (-1983433.0 / 1935360.0)))));
        np *= n;
        gtu[2] = np * (61.0 / 240.0 + n * (-103.0 / 140.0 + n * (15061.0 / 26880.0 + n * (167603.0 / 181440.0))));
        np *= n;
        gtu[3] = np * (49561.0 / 161280.0 + n * (-179.0 / 168.0 + n * (6601661.0 / 7257600.0)));
        np *= n;
        gtu[4] = np * (34729.0 / 80640.0 + n * (-3418889.0 / 1995840.0));
        np *= n;
        gtu[5] = np * (212378941.0 / 319334400.0);
        // (The origin's latitude is 0: its Gaussian latitude 0, so Zb = -Qn·(0 + clens(gtu, 0)).)
        let zb = -qn * clens(&gtu, 0.0);
        let lam0 = (f64::from(zone - 1) + 0.5) * PI / 30.0 - PI;
        Utm { csrs: Csrs::new(), lam0, qn, zb, cbg, gtu }
    }

    /// NAD83 / UTM x, y (m) of WGS 84 `lon`, `lat` (degrees).
    pub fn project(&self, lon: f64, lat: f64) -> (f64, f64) {
        let (lam, phi) = self.csrs.of(lon, lat);
        let lam = adjlon(adjlon(lam) - self.lam0);
        let cn = gatg(&self.cbg, phi, (2.0 * phi).dcos(), (2.0 * phi).dsin());
        let (sin_cn, cos_cn) = (cn.dsin(), cn.dcos());
        let (sin_ce, cos_ce) = (lam.dsin(), lam.dcos());
        let cos_cn_cos_ce = cos_cn * cos_ce;
        let cn = sin_cn.datan2(cos_cn_cos_ce);
        let inv_denom = 1.0 / sin_cn.dhypot(cos_cn_cos_ce);
        let tan_ce = sin_ce * cos_cn * inv_denom;
        let ce = tan_ce.dasinh();
        let two_inv = 2.0 * inv_denom;
        let two_inv_sq = two_inv * inv_denom;
        let tmp_r = cos_cn_cos_ce * two_inv_sq;
        let sin_r = sin_cn * tmp_r;
        let cos_r = cos_cn_cos_ce * tmp_r - 1.0;
        let sinh_i = tan_ce * two_inv;
        let cosh_i = two_inv_sq - 1.0;
        let (dcn, dce) = clen_s(&self.gtu, sin_r, cos_r, sinh_i, cosh_i);
        let (cn, ce) = (cn + dcn, ce + dce);
        if ce.abs() > 2.623395162778 {
            // (Outside the projection's domain, as PROJ says.)
            return (f64::INFINITY, f64::INFINITY);
        }
        let a = Ellipsoid::GRS80.0;
        (a * (self.qn * ce) + 500000.0, a * (self.qn * cn + self.zb))
    }
}

/// Web Mercator tile coordinates (fractional tiles at zoom `z`) of `lon`, `lat` (degrees), in
/// numpy's order of operations (as the DEM cache's samples were made).
pub fn mercator_tile(lon: f64, lat: f64, z: u32) -> (f64, f64) {
    let n = (1u64 << z) as f64;
    let r = lat.to_radians();
    let fx = (lon + 180.0) / 360.0 * n;
    let fy = (1.0 - (r.dtan() + 1.0 / r.dcos()).dln() / PI) / 2.0 * n;
    (fx, fy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_pyproj_projects() {
        // pyproj 3.8 / PROJ 9.8.1: Transformer.from_crs("EPSG:4326", "EPSG:3979", always_xy=True).
        let (x, y) = Atlas::new().project(-66.5, 46.0);
        assert!((x - 2161705.2913344926).abs() < 1e-8 && (y - 156964.1917673155).abs() < 1e-8, "{x} {y}");
    }

    #[test]
    fn as_pyproj_projects_utm() {
        // pyproj 3.8 / PROJ 9.8.1: Transformer.from_crs("EPSG:4326", c, always_xy=True), c the
        // NAD83(CSRS) / UTM zone (17N: EPSG:2958, 18N: 2959, 19N: 2960), whose pick is EPSG:1946.
        for (crs, lon, lat, ex, ey) in [
            (26918, -73.5, 44.0, 620261.7942057655, 4872965.530432259),
            (26919, -68.9, 46.7, 507645.00434363197, 5171830.875142959),
            (26917, -79.9, 42.1, 590957.4701680548, 4661463.696590829),
            // (Far from their central meridians: 3.5°, 4.4° and 2°.)
            (26918, -78.5, 43.0, 214708.34357307118, 4766760.453064816),
            (26917, -76.6, 43.4, 856321.7143546145, 4814644.859389579),
            (26919, -67.0, 47.4, 650910.0966837071, 5251554.312166896),
        ] {
            let (x, y) = Projection::of(crs).unwrap().project(lon, lat);
            assert!((x - ex).abs() < 1e-6 && (y - ey).abs() < 1e-6, "{crs}: {x} {y}");
        }
        assert!(Projection::of(4326).is_none() && Projection::of(26924).is_none());
        assert!(projected(3979) && projected(26901) && projected(26923) && !projected(4269) && !projected(26900));
        assert!(projected(3031) && projected(3413));
        // EPSG:3031 is roadcore::polar's south grid (checked against EPSG guidance note 7-2 there):
        // McMurdo (166.67° E, 77.85° S) at Snyder 21-33, 21-34 worked by hand in f64.
        let (x, y) = Projection::of(3031).unwrap().project(166.67, -77.85);
        assert!((x - 305_467.162_732).abs() < 1e-3 && (y + 1_289_201.537_714).abs() < 1e-3, "{x} {y}");
    }

    #[test]
    fn mercator_tiles() {
        // As Python's math gives them.
        let (fx, fy) = mercator_tile(139.7, 35.7, 15);
        assert!((fx - 29099.804444444442).abs() < 1e-9 && (fy - 12901.214839206761).abs() < 1e-9, "{fx} {fy}");
        assert_eq!(mercator_tile(0.0, 0.0, 1), (1.0, 1.0));
    }
}
