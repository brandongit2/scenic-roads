//! The water's exact coverage of a raster's pixels (`Raster`), and the world units it is measured in.
//!
//! **Coverage** is exact: each pixel's share of water, the area of the water inside its square
//! over the square's (signed-area accumulation, as font renderers do: each edge adds its area and
//! its crossing to the pixels it passes, and a sum along the row gives the winding inside each),
//! the winding clamped to 0–1. Water areas that abut (the sea's cells, a river mouth on the
//! coast) sum exactly; where two overlap the clamp takes one, and an edge pixel of the overlap may
//! count it twice.

/// Longitude and latitude in world units (Web Mercator, 0–1, y down).
pub fn world(lon: f64, lat: f64) -> [f64; 2] {
    let (x, y) = roadcore::geo::merc_clamped(lon, lat);
    [x, y]
}

/// Twice a ring's signed area (world units, y down: positive clockwise on the map). A closing point
/// repeating the first is fine.
pub fn area2(r: &[[f64; 2]]) -> f64 {
    let Some(&o) = r.first() else { return 0.0 };
    let mut a = 0.0;
    for i in 0..r.len() {
        let (p, q) = (r[i], r[(i + 1) % r.len()]);
        a += (p[0] - o[0]) * (q[1] - o[1]) - (q[0] - o[0]) * (p[1] - o[1]);
    }
    a
}

// ---- coverage -------------------------------------------------------------------------------------

/// Accumulates edges into a `w` × `h` raster's signed areas (a row `w + 2` wide: the edges clamped
/// to the raster's right side land in the two cells past it).
pub struct Raster {
    w: usize,
    h: usize,
    acc: Vec<f32>,
}

impl Raster {
    pub fn new(w: usize, h: usize) -> Raster {
        Raster { w, h, acc: vec![0.0; (w + 2) * h] }
    }

    /// An edge from `a` to `b` (pixels; anywhere: what's left of the raster counts as crossing its
    /// left side, what's right of it or above or below adds nothing).
    pub fn edge(&mut self, a: [f64; 2], b: [f64; 2]) {
        if a[1] == b[1] {
            return;
        }
        let (top, bot) = if a[1] < b[1] { (a[1], b[1]) } else { (b[1], a[1]) };
        if bot <= 0.0 || top >= self.h as f64 {
            return;
        }
        // Split where it crosses x = 0 and x = w, each piece clamped: left of the raster, a
        // vertical edge on its left side.
        let w = self.w as f64;
        let mut ts = [0.0, 1.0, 1.0, 1.0];
        let mut n = 1;
        for xc in [0.0, w] {
            if (a[0] < xc) != (b[0] < xc) {
                let t = (xc - a[0]) / (b[0] - a[0]);
                if t > 0.0 && t < 1.0 {
                    ts[n] = t;
                    n += 1;
                }
            }
        }
        let ts = &mut ts[..n];
        ts.sort_by(f64::total_cmp);
        let at = |t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let mut p = a;
        for q in ts[1..].iter().map(|&t| at(t)).chain(std::iter::once(b)) {
            let mid = (p[0] + q[0]) / 2.0;
            if mid < w {
                let c = |v: f64| v.clamp(0.0, w);
                self.line([c(p[0]), p[1]], [c(q[0]), q[1]]);
            }
            p = q;
        }
    }

    /// An edge within 0 ≤ x ≤ w (font-rs's accumulation).
    fn line(&mut self, p0: [f64; 2], p1: [f64; 2]) {
        if p0[1] == p1[1] {
            return;
        }
        // (Downward edges subtract: an outer ring, clockwise on the map, winds +1 inside.)
        let (dir, p0, p1) = if p0[1] < p1[1] { (-1.0, p0, p1) } else { (1.0, p1, p0) };
        let wf = self.w as f64;
        let stride = self.w + 2;
        let dxdy = (p1[0] - p0[0]) / (p1[1] - p0[1]);
        let mut x = p0[0];
        let ystart = p0[1].max(0.0);
        if p0[1] < 0.0 {
            x = (x - p0[1] * dxdy).clamp(0.0, wf);
        }
        let yend = (p1[1].ceil() as usize).min(self.h);
        for y in (ystart as usize)..yend {
            let row = y * stride;
            let dy = ((y + 1) as f64).min(p1[1]) - (y as f64).max(p0[1]);
            let xnext = (x + dxdy * dy).clamp(0.0, wf);
            let d = (dy * dir) as f32;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0floor = x0.floor();
            let x0i = x0floor as usize;
            let x1ceil = x1.ceil();
            let x1i = x1ceil as usize;
            let acc = &mut self.acc;
            if x1i <= x0i + 1 {
                let xmf = (0.5 * (x + xnext) - x0floor) as f32;
                acc[row + x0i] += d - d * xmf;
                acc[row + x0i + 1] += d * xmf;
            } else {
                let s = (1.0 / (x1 - x0)) as f32;
                let x0f = (x0 - x0floor) as f32;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = (x1 - x1ceil + 1.0) as f32;
                let am = 0.5 * s * x1f * x1f;
                acc[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    acc[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    acc[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        acc[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    acc[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                acc[row + x1i] += d * am;
            }
            x = xnext;
        }
    }

    /// Each pixel's coverage, 0–1, row by row.
    pub fn coverage(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.w * self.h);
        for row in self.acc.chunks_exact(self.w + 2) {
            let mut s = 0.0f32;
            for v in &row[..self.w] {
                s += v;
                out.push(s.clamp(0.0, 1.0));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use det::Det;

    /// Tile z/x/y's coverage, `size` × `size` pixels, by rings in world units (each closed by its
    /// first point; an outer ring clockwise on the map, a hole anticlockwise).
    fn tile_coverage(rings: &[Vec<[f64; 2]>], z: u8, x: u32, y: u32, size: usize) -> Vec<f32> {
        let k = size as f64 * f64::from(1u32 << z);
        let (ox, oy) = (f64::from(x) * size as f64, f64::from(y) * size as f64);
        let mut r = Raster::new(size, size);
        for ring in rings {
            let px = |p: [f64; 2]| [p[0] * k - ox, p[1] * k - oy];
            let mut prev = px(ring[ring.len() - 1]);
            for &p in ring {
                let q = px(p);
                r.edge(prev, q);
                prev = q;
            }
        }
        r.coverage()
    }

    /// A square in world units (clockwise on the map).
    fn square(x: f64, y: f64, s: f64) -> Vec<[f64; 2]> {
        vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]]
    }

    #[test]
    fn a_pixel_aligned_square_covers_its_pixels() {
        // Tile 10/500/300, 256 px: a square of pixels 10–20 × 30–50.
        let px = 1.0 / (256.0 * 1024.0);
        let (ox, oy) = (500.0 / 1024.0, 300.0 / 1024.0);
        let ring = vec![
            [ox + 10.0 * px, oy + 30.0 * px],
            [ox + 20.0 * px, oy + 30.0 * px],
            [ox + 20.0 * px, oy + 50.0 * px],
            [ox + 10.0 * px, oy + 50.0 * px],
        ];
        let c = tile_coverage(&[ring], 10, 500, 300, 256);
        for y in 0..256 {
            for x in 0..256 {
                let want = if (10..20).contains(&x) && (30..50).contains(&y) { 1.0 } else { 0.0 };
                assert!((c[y * 256 + x] - want).abs() < 1e-3, "{x},{y}: {}", c[y * 256 + x]);
            }
        }
    }

    #[test]
    fn coverage_is_the_area_inside_each_pixel() {
        // A square off the grid and its hole: the sum over pixels is its area, and a pixel cut by
        // an edge has its share.
        let px = 1.0 / (256.0 * 1024.0);
        let (ox, oy) = (500.0 / 1024.0, 300.0 / 1024.0);
        let at = |x: f64, y: f64| [ox + x * px, oy + y * px];
        let outer = vec![at(10.25, 30.5), at(20.75, 30.5), at(20.75, 50.25), at(10.25, 50.25)];
        let mut hole = vec![at(12.0, 32.0), at(14.5, 32.0), at(14.5, 34.0), at(12.0, 34.0)];
        hole.reverse();
        let c = tile_coverage(&[outer, hole], 10, 500, 300, 256);
        let sum: f64 = c.iter().map(|&v| f64::from(v)).sum();
        let want = 10.5 * 19.75 - 2.5 * 2.0;
        assert!((sum - want).abs() < 1e-2, "{sum} {want}");
        assert!((c[30 * 256 + 10] - 0.75 * 0.5).abs() < 1e-4, "{}", c[30 * 256 + 10]);
        assert!((c[40 * 256 + 20] - 0.75).abs() < 1e-4);
        assert!((c[32 * 256 + 14] - 0.5).abs() < 1e-4);
        assert!(c[32 * 256 + 12].abs() < 1e-4);
    }

    #[test]
    fn rings_past_the_tile_and_tiny_ones() {
        // A square much bigger than the tile covers all of it; one wholly left of it, nothing; a
        // pond a tenth of a pixel across, a hundredth of its pixel.
        let px = 1.0 / (256.0 * 1024.0);
        let (ox, oy) = (500.0 / 1024.0, 300.0 / 1024.0);
        let all = tile_coverage(&[square(ox - 0.01, oy - 0.01, 0.03)], 10, 500, 300, 64);
        assert!(all.iter().all(|&v| (v - 1.0).abs() < 1e-5));
        let none = tile_coverage(&[square(ox - 0.01, oy - 0.001, 0.005)], 10, 500, 300, 64);
        assert!(none.iter().all(|&v| v.abs() < 1e-5));
        let c = tile_coverage(&[square(ox + 100.45 * px, oy + 7.45 * px, 0.1 * px)], 10, 500, 300, 256);
        assert!((c[7 * 256 + 100] - 0.01).abs() < 1e-5, "{}", c[7 * 256 + 100]);
        assert!((c.iter().sum::<f32>() - 0.01).abs() < 1e-5);
    }

    #[test]
    fn a_circle_s_coverage_sums_to_its_area_at_any_zoom() {
        // A circle of radius 37.3 px at z12 (256 px): every lower zoom sees its area quartered.
        let n = 2000;
        let (cx, cy) = ((1000.0 + 0.3) / 4096.0, (2000.0 + 0.6) / 4096.0);
        let r = 37.3 / (256.0 * 4096.0);
        let ring: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                [cx + r * a.dcos(), cy + r * a.dsin()]
            })
            .collect();
        assert!(area2(&ring) > 0.0, "clockwise on the map");
        let area_px12 = 0.5 * area2(&ring) * (256.0 * 4096.0f64).powi(2);
        for z in [12u8, 10, 8, 5] {
            let s = 1u32 << (12 - z);
            let c = tile_coverage(std::slice::from_ref(&ring), z, 1000 / s, 2000 / s, 256);
            let sum: f64 = c.iter().map(|&v| f64::from(v)).sum();
            let want = area_px12 / f64::from(s * s);
            assert!((sum - want).abs() < 1e-3 * want.max(1.0), "z{z}: {sum} {want}");
        }
    }
}
