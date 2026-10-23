//! NALCMS's land cover resampled onto a leaf-type square as rasterio 1.5.1's `reproject` did it
//! (nearest neighbour, source nodata 0, destination nodata 255, the destination filled with 255
//! first): GDAL 3.12.4's warper and PROJ 9.8.1's Lambert azimuthal equal-area, step for step, so
//! each pixel takes the source pixel GDAL took.
//!
//! - The warp's chunks (`GDALWarpOperation::CollectChunkList`): the square halved along its longer
//!   side until a chunk's working memory (9 bits a source pixel, 9 a destination one) is within
//!   64 MiB and its source window is at least half filled, each chunk's source window from 21
//!   points along each of its edges (`ComputeSourceWindow`).
//! - Each destination row of a chunk through the approximating transformer (error 0.125 source
//!   pixels: `GDALApproxTransform`, its row halved until a straight line between transformed points
//!   is within it), pixel centres; a point just off the window transformed alone, exactly.
//! - The exact transform (`GDALGenImgProjTransform`): pixel to degrees by the square's geotransform,
//!   PROJ's pipeline run backwards (degrees to radians as its unit conversion's inverse, then the
//!   projection's forward), metres to source pixels by the inverse geotransform.
//! - The pixel taken (`GWKNearestThread`): the one the point falls in (+1e-10); a 0 (nodata) not
//!   applied, a 255 written as 254 (GDAL keeps a value off the destination's nodata).
//!
//! GDAL and PROJ are built by clang, which fuses a multiply and an add in one expression into a
//! multiply-add (the left operand's product if it's one, else the right's): fused here the same
//! (`mul_add`). The transcendental functions are this Mac's libm's (`f64::sin` and the rest are
//! Apple's), as GDAL's and PROJ's were: the squares are made on the Macs only.
//!
//! The cases a NALCMS square never meets fail rather than guess: a destination corner or edge
//! point that doesn't transform (GDAL would sample a grid), a pole inside the square (likewise), a
//! source window GDAL would call bogus.

use anyhow::{bail, ensure, Result};

/// PROJ's degree.
const DEG_TO_RAD: f64 = 0.017453292519943296;
/// GDAL's default warp memory (`dfWarpMemoryLimit`).
const MEMORY: f64 = 64.0 * 1024.0 * 1024.0;
/// rasterio's `tolerance`, the approximating transformer's error in source pixels.
const MAX_ERROR: f64 = 0.125;
/// `ComputeSourceWindow`'s points along an edge.
const STEPS: usize = 21;

/// PROJ's `laea` on WGS 84 (ellipsoidal, oblique), its forward as the pipeline runs it.
#[derive(Clone, Debug)]
pub struct Laea {
    a: f64,
    lam0: f64,
    /// Geographic to authalic latitude coefficients (PROJ's `APA + Lmax`).
    apa: [f64; 6],
    sinb1: f64,
    cosb1: f64,
    xmf: f64,
    ymf: f64,
    /// The pipeline's unit conversion (radians to degrees: 1 / DEG_TO_RAD), run backwards.
    xy_factor: f64,
}

/// PROJ's `pj_polyval`: Horner's.
fn polyval(x: f64, p: &[f64], n: usize) -> f64 {
    let mut y = p[n];
    let mut k = n;
    while k > 0 {
        k -= 1;
        y = y.mul_add(x, p[k]);
    }
    y
}

/// PROJ's `pj_clenshaw`.
fn clenshaw(s: f64, c: f64, f: &[f64; 6]) -> f64 {
    let x = 2.0 * (c - s) * (c + s);
    let (mut u0, mut u1) = (0.0f64, 0.0f64);
    for k in (0..6).rev() {
        let t = x.mul_add(u0, -u1) + f[k];
        u1 = u0;
        u0 = t;
    }
    2.0 * s * c * u0
}

impl Laea {
    /// `+proj=laea +lat_0=lat0 +lon_0=lon0 +x_0=0 +y_0=0 +ellps=WGS84`, as PROJ sets it up.
    pub fn new(lat0: f64, lon0: f64) -> Result<Laea> {
        let (a, rf) = (6378137.0f64, 298.257223563f64);
        let f = 1.0 / rf;
        let es = 2.0f64.mul_add(f, -(f * f));
        let e = es.sqrt();
        let alpha = e.asin();
        let t = (alpha / 2.0).tan();
        let n = t * t;
        ensure!(n.abs() < 0.01, "PROJ's authalic series apply only below n = 0.01");
        let one_es = 1.0 - es;
        // C[xi,phi], PROJ's pj_auxlat_coeffs (geographic to authalic).
        const C: [f64; 21] = [
            -4.0 / 3.0,
            -4.0 / 45.0,
            88.0 / 315.0,
            538.0 / 4725.0,
            20824.0 / 467775.0,
            -44732.0 / 2837835.0,
            34.0 / 45.0,
            8.0 / 105.0,
            -2482.0 / 14175.0,
            -37192.0 / 467775.0,
            -12467764.0 / 212837625.0,
            -1532.0 / 2835.0,
            -898.0 / 14175.0,
            54968.0 / 467775.0,
            100320856.0 / 1915538625.0,
            6007.0 / 14175.0,
            24496.0 / 467775.0,
            -5884124.0 / 70945875.0,
            -23356.0 / 66825.0,
            -839792.0 / 19348875.0,
            570284222.0 / 1915538625.0,
        ];
        let mut apa = [0.0; 6];
        let (mut d, mut o) = (n, 0usize);
        for (l, v) in apa.iter_mut().enumerate() {
            let m = 6 - l - 1;
            *v = d * polyval(n, &C[o..], m);
            o += m + 1;
            d *= n;
        }
        // qp: pj_authalic_lat_q(1).
        let e1 = e * 1.0;
        let qp = one_es * (1.0 / (-e1).mul_add(e1, 1.0) + e1.atanh() / e);
        let phi0 = lat0 * DEG_TO_RAD;
        ensure!(phi0.abs() > 1e-10 && (phi0.abs() - std::f64::consts::FRAC_PI_2).abs() >= 1e-10, "an oblique aspect");
        let rq = (0.5 * qp).sqrt();
        let (sinphi, cosphi) = (phi0.sin(), phi0.cos());
        let b1 = phi0 + clenshaw(sinphi, cosphi, &apa);
        let (sinb1, cosb1) = (b1.sin(), b1.cos());
        let dd = phi0.cos() / ((-(es * sinphi)).mul_add(sinphi, 1.0).sqrt() * rq * cosb1);
        let ymf = rq / dd;
        let xmf = rq * dd;
        Ok(Laea { a, lam0: lon0 * DEG_TO_RAD, apa, sinb1, cosb1, xmf, ymf, xy_factor: 1.0 / DEG_TO_RAD })
    }

    /// PROJ's `adjlon`.
    fn adjlon(l: f64) -> f64 {
        use std::f64::consts::PI;
        if l.abs() < PI + 1e-12 {
            return l;
        }
        let mut l = l + PI;
        l -= 2.0 * PI * (l / (2.0 * PI)).floor();
        l - PI
    }

    /// Metres of `lon`, `lat` (degrees), None where PROJ fails.
    pub fn forward(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        use std::f64::consts::FRAC_PI_2;
        // The unit conversion's inverse.
        let mut lam = lon / self.xy_factor;
        let mut phi = lat / self.xy_factor;
        // pj_fwd4d's fwd_prepare.
        if phi.abs() - FRAC_PI_2 > 1e-12 || !(-10.0..=10.0).contains(&lam) {
            return None;
        }
        phi = phi.clamp(-FRAC_PI_2, FRAC_PI_2);
        lam = Self::adjlon(lam);
        lam = Self::adjlon((lam - 0.0) - self.lam0);
        // laea_e_forward, oblique.
        let (coslam, sinlam) = (lam.cos(), lam.sin());
        let (sinphi, cosphi) = (phi.sin(), phi.cos());
        let xi = phi + clenshaw(sinphi, cosphi, &self.apa);
        let (sinb, cosb) = (xi.sin(), xi.cos());
        let b = (self.cosb1 * cosb).mul_add(coslam, self.sinb1.mul_add(sinb, 1.0));
        if b.abs() < 1e-10 {
            return None;
        }
        let b = (2.0 / b).sqrt();
        let y = self.ymf * b * self.cosb1.mul_add(sinb, -(self.sinb1 * cosb * coslam));
        let x = self.xmf * b * cosb * sinlam;
        // fwd_finalize: classic units, no false origin.
        Some(((x * self.a + 0.0) * 1.0, (y * self.a + 0.0) * 1.0))
    }
}

/// GDAL's transformer from a square's pixels to the source's (`GDALApproxTransform` over
/// `GDALGenImgProjTransform`).
pub struct Transform {
    laea: Laea,
    /// The square's geotransform, and the source's inverse one.
    dst: [f64; 6],
    src_inv: [f64; 6],
}

/// `GDALInvGeoTransform`, for a north-up one.
fn inv_geotransform(g: [f64; 6]) -> Result<[f64; 6]> {
    ensure!(g[2] == 0.0 && g[4] == 0.0 && g[1] != 0.0 && g[5] != 0.0, "a rotated geotransform");
    Ok([-g[0] / g[1], 1.0 / g[1], 0.0, -g[3] / g[5], 0.0, 1.0 / g[5]])
}

impl Transform {
    pub fn new(laea: Laea, dst: [f64; 6], src: [f64; 6]) -> Result<Transform> {
        ensure!(dst[2] == 0.0 && dst[4] == 0.0, "a rotated geotransform");
        Ok(Transform { laea, dst, src_inv: inv_geotransform(src)? })
    }

    /// The exact transform of each point, in place; `ok` false where it fails.
    fn exact(&self, x: &mut [f64], y: &mut [f64], ok: &mut [bool]) -> bool {
        let (g, s) = (self.dst, self.src_inv);
        let mut all = true;
        for i in 0..x.len() {
            ok[i] = x[i] != f64::INFINITY && y[i] != f64::INFINITY;
            if !ok[i] {
                all = false;
                continue;
            }
            let lon = y[i].mul_add(g[2], x[i].mul_add(g[1], g[0]));
            let lat = y[i].mul_add(g[5], x[i].mul_add(g[4], g[3]));
            match self.laea.forward(lon, lat) {
                Some((mx, my)) => {
                    x[i] = my.mul_add(s[2], mx.mul_add(s[1], s[0]));
                    y[i] = my.mul_add(s[5], mx.mul_add(s[4], s[3]));
                }
                None => {
                    x[i] = f64::INFINITY;
                    y[i] = f64::INFINITY;
                    ok[i] = false;
                    all = false;
                }
            }
        }
        all
    }

    /// `GDALApproxTransform`, destination to source.
    pub fn approx(&self, x: &mut [f64], y: &mut [f64], ok: &mut [bool]) -> bool {
        let n = x.len();
        if n == 0 {
            return true;
        }
        let mid = (n - 1) / 2;
        if y[0] != y[n - 1] || y[0] != y[mid] || x[0] == x[n - 1] || x[0] == x[mid] || n <= 5 {
            return self.exact(x, y, ok);
        }
        let mut x2 = [x[0], x[mid], x[n - 1]];
        let mut y2 = [y[0], y[mid], y[n - 1]];
        let mut ok2 = [false; 3];
        if !self.exact(&mut x2, &mut y2, &mut ok2) || !ok2.iter().all(|&o| o) {
            return self.exact(x, y, ok);
        }
        self.internal(x, y, ok, x2, y2)
    }

    /// `GDALApproxTransformInternal`: `xs`, `ys` the start, middle and end transformed.
    fn internal(&self, x: &mut [f64], y: &mut [f64], ok: &mut [bool], xs: [f64; 3], ys: [f64; 3]) -> bool {
        let n = x.len();
        let mid = (n - 1) / 2;
        let dx = (xs[2] - xs[0]) / (x[n - 1] - x[0]);
        let dy = (ys[2] - ys[0]) / (x[n - 1] - x[0]);
        let err = (dx.mul_add(x[mid] - x[0], xs[0]) - xs[1]).abs() + (dy.mul_add(x[mid] - x[0], ys[0]) - ys[1]).abs();
        if err > MAX_ERROR {
            let im = [(mid - 1) / 2, mid - 1, mid + (n - mid - 1) / 2];
            let mut xm = [x[im[0]], x[im[1]], x[im[2]]];
            let mut ym = [y[im[0]], y[im[1]], y[im[2]]];
            let half1 = mid <= 5 || y[0] != y[mid - 1] || y[0] != y[im[0]] || x[0] == x[mid - 1] || x[0] == x[im[0]];
            let half2 =
                n - mid <= 5 || y[mid] != y[n - 1] || y[mid] != y[im[2]] || x[mid] == x[n - 1] || x[mid] == x[im[2]];
            let mut ok2 = [false; 3];
            let mut good = false;
            if !half1 && !half2 {
                good = self.exact(&mut xm, &mut ym, &mut ok2);
            } else if !half1 {
                good = self.exact(&mut xm[..2], &mut ym[..2], &mut ok2[..2]);
                ok2[2] = true;
            } else if !half2 {
                good = self.exact(&mut xm[2..], &mut ym[2..], &mut ok2[2..]);
                ok2[0] = true;
                ok2[1] = true;
            }
            if !good || !ok2.iter().all(|&o| o) {
                let mut g = self.exact(&mut x[1..mid], &mut y[1..mid], &mut ok[1..mid]);
                g &= self.exact(&mut x[mid + 1..n - 1], &mut y[mid + 1..n - 1], &mut ok[mid + 1..n - 1]);
                (x[0], y[0], ok[0]) = (xs[0], ys[0], true);
                (x[mid], y[mid], ok[mid]) = (xs[1], ys[1], true);
                (x[n - 1], y[n - 1], ok[n - 1]) = (xs[2], ys[2], true);
                return g;
            }
            let g = if !half1 {
                self.internal(
                    &mut x[..mid],
                    &mut y[..mid],
                    &mut ok[..mid],
                    [xs[0], xm[0], xm[1]],
                    [ys[0], ym[0], ym[1]],
                )
            } else {
                let g = self.exact(&mut x[1..mid], &mut y[1..mid], &mut ok[1..mid]);
                (x[0], y[0], ok[0]) = (xs[0], ys[0], true);
                g
            };
            if !g {
                return false;
            }
            let g = if !half2 {
                self.internal(
                    &mut x[mid..],
                    &mut y[mid..],
                    &mut ok[mid..],
                    [xs[1], xm[2], xs[2]],
                    [ys[1], ym[2], ys[2]],
                )
            } else {
                let g = self.exact(&mut x[mid + 1..n - 1], &mut y[mid + 1..n - 1], &mut ok[mid + 1..n - 1]);
                (x[mid], y[mid], ok[mid]) = (xs[1], ys[1], true);
                (x[n - 1], y[n - 1], ok[n - 1]) = (xs[2], ys[2], true);
                g
            };
            return g;
        }
        for i in (0..n).rev() {
            let dist = x[i] - x[0];
            x[i] = dx.mul_add(dist, xs[0]);
            y[i] = dy.mul_add(dist, ys[0]);
            ok[i] = true;
        }
        true
    }
}

/// A chunk of the square: its destination window (x, y, width, height) and its source window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chunk {
    pub dst: (i32, i32, i32, i32),
    pub src: (i32, i32, i32, i32),
}

/// The source's size and the transform: what the chunks are computed from.
pub struct Warp {
    pub t: Transform,
    pub src_w: i32,
    pub src_h: i32,
}

impl Warp {
    /// `ComputeSourceWindow` for destination window (`dx`, `dy`, `dw`, `dh`): the source window
    /// and its fill ratio.
    fn source_window(&self, dx: i32, dy: i32, dw: i32, dh: i32) -> Result<((i32, i32, i32, i32), f64)> {
        let (rw, rh) = (self.src_w, self.src_h);
        // The corners, which decide between edge and grid sampling.
        let mut cx = [dx as f64, (dx + dw) as f64, dx as f64, (dx + dw) as f64];
        let mut cy = [dy as f64, dy as f64, (dy + dh) as f64, (dy + dh) as f64];
        let mut cok = [false; 4];
        if !self.t.approx(&mut cx, &mut cy, &mut cok) || !cok.iter().all(|&o| o) {
            bail!("a corner of window {dx},{dy},{dw},{dh} doesn't transform (GDAL samples every edge pixel)");
        }
        // The edges, STEPS points along each.
        let step = 1.0 / (STEPS - 1) as f64;
        let (mut xs, mut ys) = (Vec::with_capacity(4 * STEPS), Vec::with_capacity(4 * STEPS));
        let mut r = 0.0f64;
        while r <= 1.0 + step * 0.5 {
            xs.extend([r.mul_add(dw as f64, dx as f64), r.mul_add(dw as f64, dx as f64), dx as f64, (dw + dx) as f64]);
            ys.extend([dy as f64, (dy + dh) as f64, r.mul_add(dh as f64, dy as f64), r.mul_add(dh as f64, dy as f64)]);
            r += step;
        }
        ensure!(xs.len() == 4 * STEPS, "{} edge points, not {}", xs.len(), 4 * STEPS);
        let mut ok = vec![false; xs.len()];
        self.t.approx(&mut xs, &mut ys, &mut ok);
        let (mut minx, mut miny, mut maxx, mut maxy) =
            (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut failed = 0;
        for i in 0..xs.len() {
            if !ok[i] || xs[i].is_nan() || ys[i].is_nan() {
                failed += 1;
                continue;
            }
            minx = minx.min(xs[i]);
            miny = miny.min(ys[i]);
            maxx = maxx.max(xs[i]);
            maxy = maxy.max(ys[i]);
        }
        ensure!(
            failed == 0,
            "{failed} edge points of window {dx},{dy},{dw},{dh} don't transform (GDAL samples a grid)"
        );
        ensure!(
            !(minx < -1e6 || miny < -1e6 || maxx > rw as f64 + 1e6 || maxy > rh as f64 + 1e6),
            "a bogus source window for {dx},{dy},{dw},{dh} (GDAL tries again checking with the inverse)"
        );
        if minx > rw as f64 || maxx < 0.0 || miny > rh as f64 || maxy < 0.0 {
            return Ok(((0, 0, 0, 0), 0.0));
        }
        let close = |v: f64| {
            let r = v.round();
            if (r - v).abs() < 1e-6 {
                r
            } else {
                v
            }
        };
        let (minx, miny, maxx, maxy) = (close(minx), close(miny), close(maxx), close(maxy));
        // (Nearest neighbour: no kernel radius, no extra source pixels.)
        let min_xc = minx.max(0.0) as i32;
        let min_yc = miny.max(0.0) as i32;
        let max_xc = maxx.ceil().min(rw as f64) as i32;
        let max_yc = maxy.ceil().min(rh as f64) as i32;
        let (sx, sw) = if (max_xc - min_xc) as f64 > 0.9 * rw as f64 {
            (0, rw)
        } else {
            let o = min_xc.min(rw).max(0);
            (o, (rw - o).min(max_xc - o).max(0))
        };
        let (sy, sh) = if (max_yc - min_yc) as f64 > 0.9 * rh as f64 {
            (0, rh)
        } else {
            let o = min_yc.min(rh).max(0);
            (o, (rh - o).min(max_yc - o).max(0))
        };
        let fill = sw as f64 * sh as f64 / 1.0f64.max((maxx - minx) * (maxy - miny));
        Ok(((sx, sy, sw, sh), fill))
    }

    /// `CollectChunkList` over the whole square (`w` by `h`), sorted top to bottom, then left to
    /// right.
    pub fn chunks(&self, w: i32, h: i32) -> Result<Vec<Chunk>> {
        let mut out = Vec::new();
        self.collect(0, 0, w, h, &mut out)?;
        out.sort_by_key(|c| (c.dst.1, c.dst.0));
        Ok(out)
    }

    fn collect(&self, dx: i32, dy: i32, dw: i32, dh: i32, out: &mut Vec<Chunk>) -> Result<()> {
        let (src, fill) = self.source_window(dx, dy, dw, dh)?;
        // Working memory: 8 bits a pixel and a validity bit, source and destination alike.
        let memory = (9.0 * src.2 as f64 * src.3 as f64 + 9.0 * dw as f64 * dh as f64) / 8.0;
        if (memory > MEMORY && (dw > 2 || dh > 2)) || (fill > 0.0 && fill < 0.5 && (dw > 100 || dh > 100)) {
            // (The destination's blocks are its rows, a square wide: never "optimised for size".)
            if dw > dh {
                let c1 = dw / 2;
                self.collect(dx, dy, c1, dh, out)?;
                self.collect(dx + c1, dy, dw - c1, dh, out)?;
            } else {
                let c1 = dh / 2;
                self.collect(dx, dy, dw, c1, out)?;
                self.collect(dx, dy + c1, dw, dh - c1, out)?;
            }
            return Ok(());
        }
        out.push(Chunk { dst: (dx, dy, dw, dh), src });
        Ok(())
    }

    /// Chunk `c` warped into `out` (its destination window, row by row, filled with 255), from its
    /// source window's pixels `src` (row by row): `GWKNearestThread`, row by row.
    pub fn warp_chunk(&self, c: &Chunk, src: &[u8], out: &mut [u8]) {
        let (dx, dy, dw, dh) = c.dst;
        if c.src.2 == 0 || c.src.3 == 0 {
            return;
        }
        let n = dw as usize;
        let mut xs = vec![0.0f64; n];
        let mut ys = vec![0.0f64; n];
        let mut ok = vec![false; n];
        for row in 0..dh {
            for (i, x) in xs.iter_mut().enumerate() {
                *x = i as f64 + 0.5 + dx as f64;
            }
            let fy = row as f64 + 0.5 + dy as f64;
            ys.fill(fy);
            self.t.approx(&mut xs, &mut ys, &mut ok);
            let line = &mut out[row as usize * n..][..n];
            for i in 0..n {
                let Some(at) = self.source_offset(c, &mut xs, &mut ys, &mut ok, i, row) else { continue };
                let v = src[at];
                // (Nodata in the source: not applied.)
                if v == 0 {
                    continue;
                }
                line[i] = if v == 255 { 254 } else { v };
            }
        }
    }

    /// `GWKCheckAndComputeSrcOffsets`: point `i` of row `row`'s offset in the source window, if it
    /// falls in it (a point just off it transformed alone, exactly, first).
    fn source_offset(
        &self,
        c: &Chunk,
        xs: &mut [f64],
        ys: &mut [f64],
        ok: &mut [bool],
        i: usize,
        row: i32,
    ) -> Option<usize> {
        let (dx, dy, _, _) = c.dst;
        let (sx, sy, sw, sh) = c.src;
        let (sxf, syf) = (sx as f64, sy as f64);
        let mut tries = 0;
        loop {
            if tries == 1 {
                xs[i] = i as f64 + 0.5 + dx as f64;
                ys[i] = row as f64 + 0.5 + dy as f64;
                self.t.approx(&mut xs[i..=i], &mut ys[i..=i], &mut ok[i..=i]);
            }
            if !ok[i] || xs[i].is_nan() || ys[i].is_nan() {
                return None;
            }
            let (x, y) = (xs[i], ys[i]);
            let retry = if x < sxf {
                x > sxf - 1.0
            } else if y < syf {
                y > syf - 1.0
            } else if x + 1e-10 > (sw + sx) as f64 {
                x < (sw + sx) as f64 + 1.0
            } else if y + 1e-10 > (sh + sy) as f64 {
                y < (sh + sy) as f64 + 1.0
            } else {
                break;
            };
            if tries == 0 && retry {
                tries = 1;
                continue;
            }
            return None;
        }
        let mut ix = (xs[i] + 1e-10) as i32 - sx;
        let mut iy = (ys[i] + 1e-10) as i32 - sy;
        if ix == sw {
            ix -= 1;
        }
        if iy == sh {
            iy -= 1;
        }
        Some(ix as usize + iy as usize * sw as usize)
    }

    /// Whether GDAL's special points (lon 0 at ±89.9999°, in the square's pixels) fall in the
    /// square (`w` by `h`), where GDAL would sample a grid instead of the edges.
    pub fn pole_inside(&self, w: i32, h: i32) -> bool {
        let g = self.t.dst;
        [-89.9999f64, 89.9999].iter().any(|&lat| {
            let px = (0.0 - g[0]) / g[1];
            let py = (lat - g[3]) / g[5];
            (0.0..=w as f64).contains(&px) && (0.0..=h as f64).contains(&py)
        })
    }
}
