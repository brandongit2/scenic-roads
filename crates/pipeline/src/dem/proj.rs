//! The coordinate conversions the DEMs need, computed as PROJ 9.8
//! computes them, with the deterministic maths (`det`): WGS 84 to Canada Atlas Lambert (EPSG:3979,
//! NRCan's HRDEM and MRDEM) through NAD83(CSRS), by EPSG's 7-parameter "NAD83(CSRS) to WGS 84 (2)"
//! (the transformation PROJ picks, everywhere: it needs no grid).
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
            if c[2] >= 0.0 { FRAC_PI_2 } else { -FRAC_PI_2 }
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

/// WGS 84 longitude, latitude (degrees) to Canada Atlas Lambert (EPSG:3979) x, y (metres), as
/// pyproj's `Transformer.from_crs("EPSG:4326", "EPSG:3979", always_xy=True)` does.
#[derive(Clone, Debug)]
pub struct Atlas {
    wgs: Ellipsoid,
    grs: Ellipsoid,
    /// The Helmert translation (m) and rotations (radians; coordinate frame, small angles).
    t: [f64; 3],
    r: [f64; 3],
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
        let wgs = Ellipsoid::new(Ellipsoid::WGS84.0, Ellipsoid::WGS84.1);
        let grs = Ellipsoid::new(Ellipsoid::GRS80.0, Ellipsoid::GRS80.1);
        let arc = DEG_TO_RAD / 3600.0;
        // EPSG:1946's parameters, as PROJ writes them.
        let t = [-0.991, 1.9072, 0.5129];
        let r = [-0.0257899075194932 * arc, -0.0096500989602704 * arc, -0.0116599432323421 * arc];
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
        Atlas { wgs, grs, t, r, n, c, rho0, lam0 }
    }

    /// EPSG:3979 x, y (m) of WGS 84 `lon`, `lat` (degrees).
    pub fn project(&self, lon: f64, lat: f64) -> (f64, f64) {
        // WGS 84 to NAD83(CSRS): geocentric, the inverse Helmert, back to geodetic on GRS 80.
        let c = self.wgs.cartesian(lon * DEG_TO_RAD, lat * DEG_TO_RAD);
        let (x, y, z) = (c[0] - self.t[0], c[1] - self.t[1], c[2] - self.t[2]);
        let [f, t, p] = self.r;
        // The rotation matrix transposed: R00 = R11 = R22 = 1, R01 = p, R02 = -t, R10 = -p,
        // R12 = f, R20 = t, R21 = -f.
        let local = [t.mul_add(z, 1.0f64.mul_add(x, -p * y)), (-f).mul_add(z, p.mul_add(x, 1.0 * y)), 1.0f64.mul_add(z, (-t).mul_add(x, f * y))];
        let (lam, phi) = self.grs.geodetic(local);
        // Lambert conformal conic.
        let lam = adjlon(adjlon(lam) - self.lam0);
        let rho = self.c * tsfn(phi, phi.dsin(), self.grs.e).dpowf(self.n);
        let l = lam * self.n;
        let x = rho * l.dsin();
        let y = (-rho).mul_add(l.dcos(), self.rho0);
        (x * self.grs.a, y * self.grs.a)
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
    fn mercator_tiles() {
        // As Python's math gives them.
        let (fx, fy) = mercator_tile(139.7, 35.7, 15);
        assert!((fx - 29099.804444444442).abs() < 1e-9 && (fy - 12901.214839206761).abs() < 1e-9, "{fx} {fy}");
        assert_eq!(mercator_tile(0.0, 0.0, 1), (1.0, 1.0));
    }
}
