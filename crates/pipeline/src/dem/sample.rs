//! A raster sampled at points, to the bit as the DEM cache's samples were made: points grouped by 512-pixel block (whatever the file's own tiling), each block read whole,
//! nodata marked, and a bilinear sample in 32-bit floats in numpy's order of operations, edges
//! clamping to the block's, falling back to the nearest pixel where a neighbour has no data.

use crate::geotiff::Tiff;
use anyhow::Result;
use rayon::prelude::*;

/// Values below this are nodata (all the sources use large negative nodata values).
pub const NODATA_BELOW: f32 = -1000.0;

/// The blocks points are grouped by, and read in.
pub const BLOCK: i64 = 512;

/// Block `a` (`w` by `h`) sampled at pixel-centre coordinates `c`, `r`: bilinear, the edges
/// clamping; the nearest pixel when any of the four neighbours is nodata (or the sum isn't
/// finite); NaN when that one is nodata too.
pub fn bilinear(a: &[f32], w: usize, h: usize, c: f64, r: f64) -> f32 {
    let c0 = c.floor() as i32;
    let r0 = r.floor() as i32;
    let fx = (c - c0 as f64) as f32;
    let fy = (r - r0 as f64) as f32;
    let clip = |v: i32, n: usize| v.clamp(0, n as i32 - 1) as usize;
    let (c0c, c1) = (clip(c0, w), clip(c0 + 1, w));
    let (r0c, r1) = (clip(r0, h), clip(r0 + 1, h));
    let (v00, v01, v10, v11) = (a[r0c * w + c0c], a[r0c * w + c1], a[r1 * w + c0c], a[r1 * w + c1]);
    let val = (v00 * (1.0 - fx) + v01 * fx) * (1.0 - fy) + (v10 * (1.0 - fx) + v11 * fx) * fy;
    let bad = v00 < NODATA_BELOW || v01 < NODATA_BELOW || v10 < NODATA_BELOW || v11 < NODATA_BELOW || !val.is_finite();
    if !bad {
        return val;
    }
    let rn = clip(r.round_ties_even() as i32, h);
    let cn = clip(c.round_ties_even() as i32, w);
    let vn = a[rn * w + cn];
    if vn < NODATA_BELOW || !vn.is_finite() {
        f32::NAN
    } else {
        vn
    }
}

/// Runs `f` in `pool` (its threads for the parallel work), or where it's called.
pub fn within<R: Send>(pool: Option<&rayon::ThreadPool>, f: impl FnOnce() -> R + Send) -> R {
    match pool {
        Some(p) => p.install(f),
        None => f(),
    }
}

/// Image `level` of `t` sampled at points (`px`, `py`, in the raster's CRS): each point's value,
/// NaN where it's outside the raster or there's no data.
pub fn sample_raster(t: &Tiff, level: usize, px: &[f64], py: &[f64], pool: Option<&rayon::ThreadPool>) -> Result<Vec<f32>> {
    let img = t.level(level)?;
    let gt = t.transform(level)?;
    let (w, h) = (img.width as i64, img.height as i64);
    let nbx = (w + BLOCK - 1) / BLOCK;
    // (block key, point, column, row), the points inside the raster.
    let mut pts: Vec<(i64, u32, f64, f64)> = Vec::new();
    for (i, (&x, &y)) in px.iter().zip(py).enumerate() {
        let cf = (x - gt[0]) / gt[1] - 0.5;
        let rf = (y - gt[3]) / gt[5] - 0.5;
        if !(cf > -0.5 && rf > -0.5 && cf < w as f64 - 0.5 && rf < h as f64 - 0.5) {
            continue;
        }
        let bx = (cf.floor() as i64).clamp(0, w - 1) / BLOCK;
        let by = (rf.floor() as i64).clamp(0, h - 1) / BLOCK;
        pts.push((by * nbx + bx, i as u32, cf, rf));
    }
    pts.sort_by_key(|p| p.0);
    let mut groups: Vec<&[(i64, u32, f64, f64)]> = Vec::new();
    let mut s = 0;
    for e in 1..=pts.len() {
        if e == pts.len() || pts[e].0 != pts[s].0 {
            groups.push(&pts[s..e]);
            s = e;
        }
    }
    let nodata = t.nodata().filter(|&v| v >= NODATA_BELOW as f64).map(|v| v as f32);
    let sampled: Vec<Vec<(u32, f32)>> = within(pool, || {
        groups
            .par_iter()
            .map(|g| -> Result<Vec<(u32, f32)>> {
                let k = g[0].0;
                let (x0, y0) = ((k % nbx) * BLOCK, (k / nbx) * BLOCK);
                let (bw, bh) = (BLOCK.min(w - x0), BLOCK.min(h - y0));
                let mut a = t.read_window(level, x0 as u32, y0 as u32, bw as u32, bh as u32)?;
                if let Some(nd) = nodata {
                    for v in a.iter_mut().filter(|v| **v == nd) {
                        *v = -1e9;
                    }
                }
                Ok(g.iter().map(|&(_, i, cf, rf)| (i, bilinear(&a, bw as usize, bh as usize, cf - x0 as f64, rf - y0 as f64))).collect())
            })
            .collect::<Result<_>>()
    })?;
    let mut out = vec![f32::NAN; px.len()];
    for (i, v) in sampled.into_iter().flatten() {
        out[i as usize] = v;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_as_numpy() {
        // 3 x 2 block.
        let a = [1.0f32, 2.0, 4.0, 10.0, 20.0, 40.0];
        assert_eq!(bilinear(&a, 3, 2, 0.0, 0.0), 1.0);
        assert_eq!(bilinear(&a, 3, 2, 0.5, 0.5), (1.0 * 0.5 + 2.0 * 0.5) * 0.5 + (10.0 * 0.5 + 20.0 * 0.5) * 0.5);
        // Edges clamp.
        assert_eq!(bilinear(&a, 3, 2, -0.5, 1.5), 10.0);
        assert_eq!(bilinear(&a, 3, 2, 2.5, -0.5), 4.0);
        // A nodata neighbour: the nearest pixel (halves to even), or NaN.
        let b = [1.0f32, -1e9, 4.0, 10.0, 20.0, 40.0];
        assert_eq!(bilinear(&b, 3, 2, 0.4, 0.4), 1.0);
        assert_eq!(bilinear(&b, 3, 2, 1.5, 0.5), 4.0, "row 0, column 2");
        assert!(bilinear(&b, 3, 2, 1.2, 0.2).is_nan());
    }
}
