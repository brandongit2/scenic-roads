//! The coordinate conversions sample.py took from PROJ 9.8 (through pyproj), computed as PROJ
//! computes them, with the deterministic maths (`det`): WGS 84 to Canada Atlas Lambert (EPSG:3979,
//! NRCan's HRDEM and MRDEM) through NAD83(CSRS), by EPSG's 7-parameter "NAD83(CSRS) to WGS 84 (2)"
//! (the transformation pyproj picks, everywhere: it needs no grid); and WGS 84 to a transverse
//! Mercator (Taiwan's TM2 zones, for the MOI DTM; TWD97 taken as WGS 84, as PROJ does), by
//! Poder–Engsager's series (PROJ's default, its proj.ini's `tmerc_default_algo`).
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
    /// The third flattening.
    n: f64,
}

impl Ellipsoid {
    pub fn new(a: f64, rf: f64) -> Ellipsoid {
        let f = 1.0 / rf;
        let es = 2.0f64.mul_add(f, -(f * f));
        let e = es.sqrt();
        let alpha = e.dasin();
        let e2 = alpha.dtan();
        Ellipsoid { a, f, es, e, e2s: e2 * e2, ra: 1.0 / a, n: (alpha / 2.0).dtan().dpowf(2.0) }
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

/// Horner's evaluation of the polynomial with coefficients `p` (constant first), PROJ's
/// `pj_polyval`.
fn polyval(x: f64, p: &[f64]) -> f64 {
    let mut y = p[p.len() - 1];
    for &c in p[..p.len() - 1].iter().rev() {
        y = y.mul_add(x, c);
    }
    y
}

/// PROJ's `pj_auxlat_coeffs` for one of the conversions to or from conformal latitude: six
/// Fourier coefficients, each a polynomial in the third flattening `n` (`c` holds the six
/// polynomials' coefficients, 6, 5, … 1 of them).
fn auxlat_coeffs(n: f64, c: &[f64; 21]) -> [f64; 6] {
    let mut f = [0.0; 6];
    let (mut d, mut o) = (n, 0);
    for (l, v) in f.iter_mut().enumerate() {
        let m = 6 - l;
        *v = d * polyval(n, &c[o..o + m]);
        o += m;
        d *= n;
    }
    f
}

/// Geographic to conformal latitude, C[chi, phi].
const C_CHI_PHI: [f64; 21] = [
    -2.0, 2.0 / 3.0, 4.0 / 3.0, -82.0 / 45.0, 32.0 / 45.0, 4642.0 / 4725.0, 5.0 / 3.0, -16.0 / 15.0, -13.0 / 9.0, 904.0 / 315.0, -1522.0 / 945.0, -26.0 / 15.0, 34.0 / 21.0, 8.0 / 5.0, -12686.0 / 2835.0, 1237.0 / 630.0, -12.0 / 5.0,
    -24832.0 / 14175.0, -734.0 / 315.0, 109598.0 / 31185.0, 444337.0 / 155925.0,
];
/// Conformal to rectifying latitude, C[mu, chi].
const C_MU_CHI: [f64; 21] = [
    1.0 / 2.0, -2.0 / 3.0, 5.0 / 16.0, 41.0 / 180.0, -127.0 / 288.0, 7891.0 / 37800.0, 13.0 / 48.0, -3.0 / 5.0, 557.0 / 1440.0, 281.0 / 630.0, -1983433.0 / 1935360.0, 61.0 / 240.0, -103.0 / 140.0, 15061.0 / 26880.0,
    167603.0 / 181440.0, 49561.0 / 161280.0, -179.0 / 168.0, 6601661.0 / 7257600.0, 34729.0 / 80640.0, -3418889.0 / 1995840.0, 212378941.0 / 319334400.0,
];

/// PROJ's `pj_clenshaw`: the sum of `f[k] sin((2k + 2) zeta)`.
fn clenshaw(szeta: f64, czeta: f64, f: &[f64; 6]) -> f64 {
    let x = 2.0 * (czeta - szeta) * (czeta + szeta);
    let (mut u0, mut u1) = (0.0f64, 0.0f64);
    for k in (0..6).rev() {
        let t = x.mul_add(u0, -u1) + f[k];
        u1 = u0;
        u0 = t;
    }
    2.0 * szeta * czeta * u0
}

/// PROJ's `pj_auxlat_convert`: latitude `zeta` converted by coefficients `f`.
fn auxlat_convert(zeta: f64, f: &[f64; 6]) -> f64 {
    zeta + clenshaw(zeta.dsin(), zeta.dcos(), f)
}

/// PROJ's complex Clenshaw summation (`clenS`): the real part, and the imaginary in `im`.
fn clens(a: &[f64; 6], sin_r: f64, cos_r: f64, sinh_i: f64, cosh_i: f64) -> (f64, f64) {
    let r = 2.0 * cos_r * cosh_i;
    let i = -2.0 * sin_r * sinh_i;
    let (mut hi1, mut hr1, mut hi) = (0.0f64, 0.0f64, 0.0f64);
    let mut hr = a[5];
    for k in (0..5).rev() {
        let (hr2, hi2) = (hr1, hi1);
        hr1 = hr;
        hi1 = hi;
        hr = (-i).mul_add(hi1, r.mul_add(hr1, -hr2)) + a[k];
        hi = r.mul_add(hi1, i.mul_add(hr1, -hi2));
    }
    let r = sin_r * cosh_i;
    let i = cos_r * sinh_i;
    (r.mul_add(hr, -(i * hi)), r.mul_add(hi, i * hr))
}

/// WGS 84 longitude, latitude (degrees) to a transverse Mercator on an ellipsoid (no datum shift),
/// by Poder–Engsager's series, as PROJ's `tmerc` computes it.
#[derive(Clone, Debug)]
pub struct TransverseMercator {
    a: f64,
    lam0: f64,
    x0: f64,
    y0: f64,
    /// Geographic to Gaussian (conformal) latitude, and conformal sphere to the ellipsoid's
    /// rectifying coordinates.
    cbg: [f64; 6],
    gtu: [f64; 6],
    qn: f64,
    zb: f64,
}

impl TransverseMercator {
    /// Central meridian `lon0` and origin latitude `lat0` (degrees), scale `k0`, false easting and
    /// northing (m), on the ellipsoid of axis `a` and inverse flattening `rf`.
    pub fn new(lon0: f64, lat0: f64, k0: f64, x0: f64, y0: f64, a: f64, rf: f64) -> TransverseMercator {
        let ell = Ellipsoid::new(a, rf);
        let n = ell.n;
        let cbg = auxlat_coeffs(n, &C_CHI_PHI);
        let gtu = auxlat_coeffs(n, &C_MU_CHI);
        let qn = k0 * (polyval(n * n, &[1.0, 1.0 / 4.0, 1.0 / 64.0, 1.0 / 256.0]) / (1.0 + n));
        let z = auxlat_convert(lat0 * DEG_TO_RAD, &cbg);
        let zb = -qn * auxlat_convert(z, &gtu);
        TransverseMercator { a, lam0: lon0 * DEG_TO_RAD, x0, y0, cbg, gtu, qn, zb }
    }

    /// Taiwan's TM2 zone with central meridian `lon0` (TWD97: EPSG:3826 for 121° E, 3825 for 119°).
    pub fn tm2(lon0: f64) -> TransverseMercator {
        TransverseMercator::new(lon0, 0.0, 0.9999, 250000.0, 0.0, Ellipsoid::GRS80.0, Ellipsoid::GRS80.1)
    }

    /// x, y (m); None outside the projection's domain (150° from the central meridian).
    pub fn project(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        let lam = adjlon(adjlon(lon * DEG_TO_RAD) - self.lam0);
        let cn = auxlat_convert(lat * DEG_TO_RAD, &self.cbg);
        let (sin_cn, cos_cn) = (cn.dsin(), cn.dcos());
        let (sin_ce, cos_ce) = (lam.dsin(), lam.dcos());
        let cos_cn_cos_ce = cos_cn * cos_ce;
        let cn = sin_cn.datan2(cos_cn_cos_ce);
        let inv_denom_tan_ce = 1.0 / sin_cn.dhypot(cos_cn_cos_ce);
        let tan_ce = sin_ce * cos_cn * inv_denom_tan_ce;
        let ce = tan_ce.dasinh();
        let two_inv_denom_tan_ce = 2.0 * inv_denom_tan_ce;
        let two_inv_denom_tan_ce_square = two_inv_denom_tan_ce * inv_denom_tan_ce;
        let tmp_r = cos_cn_cos_ce * two_inv_denom_tan_ce_square;
        let sin_arg_r = sin_cn * tmp_r;
        let cos_arg_r = cos_cn_cos_ce.mul_add(tmp_r, -1.0);
        let sinh_arg_i = tan_ce * two_inv_denom_tan_ce;
        let cosh_arg_i = two_inv_denom_tan_ce_square - 1.0;
        let (dcn, dce) = clens(&self.gtu, sin_arg_r, cos_arg_r, sinh_arg_i, cosh_arg_i);
        let (cn, ce) = (cn + dcn, ce + dce);
        if ce.abs() > 2.623395162778 {
            return None;
        }
        let y = self.qn.mul_add(cn, self.zb);
        let x = self.qn * ce;
        Some((x * self.a + self.x0, y * self.a + self.y0))
    }
}

/// Web Mercator tile coordinates (fractional tiles at zoom `z`) of `lon`, `lat` (degrees), as
/// sample.py computed them with numpy.
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
        // EPSG:3826 and 3825 (TWD97 / TM2 zones 121 and 119).
        let (x, y) = TransverseMercator::tm2(121.0).project(121.5, 24.0).unwrap();
        assert!((x - 300871.2349519656).abs() < 1e-8 && (y - 2655113.4088765015).abs() < 1e-8, "{x} {y}");
        let (x, y) = TransverseMercator::tm2(119.0).project(121.5, 24.0).unwrap();
        assert!((x - 504408.39106802724).abs() < 1e-8 && (y - 2657281.6159110046).abs() < 1e-8, "{x} {y}");
    }

    #[test]
    fn mercator_tiles() {
        // As Python's math gives them.
        let (fx, fy) = mercator_tile(139.7, 35.7, 15);
        assert!((fx - 29099.804444444442).abs() < 1e-9 && (fy - 12901.214839206761).abs() < 1e-9, "{fx} {fy}");
        assert_eq!(mercator_tile(0.0, 0.0, 1), (1.0, 1.0));
    }
}
