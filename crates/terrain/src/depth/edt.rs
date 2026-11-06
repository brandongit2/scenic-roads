//! Exact Euclidean distance transforms on a pixel grid (Felzenszwalb and Huttenlocher's lower
//! envelope of parabolas, a row pass then a column pass): each pixel's distance to the nearest
//! seed pixel, and which one it is.

/// For every pixel of a `w` × `h` grid (row-major), the distance (pixels) to the nearest pixel where
/// `seed` holds, and that pixel's index; `f32::INFINITY` and `u32::MAX` when there's no seed.
pub fn nearest(seed: &[bool], w: usize, h: usize) -> (Vec<f32>, Vec<u32>) {
    debug_assert_eq!(seed.len(), w * h);
    const INF: f64 = 1e20;
    // Pass 1, along each row: squared distance to the nearest seed in the row, and its column.
    let mut d2 = vec![INF; w * h];
    let mut col = vec![u32::MAX; w * h];
    let mut f = vec![0f64; w.max(h)];
    let mut out_d = vec![0f64; w.max(h)];
    let mut out_i = vec![0usize; w.max(h)];
    let mut v = vec![0usize; w.max(h)];
    let mut z = vec![0f64; w.max(h) + 1];
    for r in 0..h {
        for c in 0..w {
            f[c] = if seed[r * w + c] { 0.0 } else { INF };
        }
        if envelope(&f[..w], &mut out_d[..w], &mut out_i[..w], &mut v, &mut z) {
            for c in 0..w {
                d2[r * w + c] = out_d[c];
                col[r * w + c] = out_i[c] as u32;
            }
        }
    }
    // Pass 2, along each column of the row results.
    let mut dist = vec![f32::INFINITY; w * h];
    let mut idx = vec![u32::MAX; w * h];
    for c in 0..w {
        for r in 0..h {
            f[r] = d2[r * w + c];
        }
        if envelope(&f[..h], &mut out_d[..h], &mut out_i[..h], &mut v, &mut z) {
            for r in 0..h {
                if out_d[r] < INF / 2.0 {
                    let rr = out_i[r];
                    dist[r * w + c] = (out_d[r] as f32).sqrt();
                    idx[r * w + c] = (rr * w) as u32 + col[rr * w + c];
                }
            }
        }
    }
    (dist, idx)
}

/// The lower envelope of the parabolas (q − p)² + f(p): each q's minimum and its p. False when
/// every f is infinite (nothing written).
fn envelope(f: &[f64], d: &mut [f64], at: &mut [usize], v: &mut [usize], z: &mut [f64]) -> bool {
    const INF: f64 = 1e20;
    let n = f.len();
    let Some(first) = (0..n).find(|&q| f[q] < INF / 2.0) else { return false };
    let mut k = 0usize;
    v[0] = first;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in first + 1..n {
        if f[q] >= INF / 2.0 {
            continue;
        }
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64));
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // k == 0: q replaces the first parabola.
                v[0] = q;
                z[0] = f64::NEG_INFINITY;
                z[1] = f64::INFINITY;
                break;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
            break;
        }
    }
    let mut k = 0usize;
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        d[q] = ((q as f64 - p as f64) * (q as f64 - p as f64)) + f[p];
        at[q] = p;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against brute force on a scatter of seeds, including empty rows and columns.
    #[test]
    fn the_distances_are_exact() {
        let (w, h) = (37, 23);
        let mut seed = vec![false; w * h];
        for (r, c) in [(0, 0), (5, 30), (22, 36), (11, 11), (12, 11), (3, 20)] {
            seed[r * w + c] = true;
        }
        let (d, i) = nearest(&seed, w, h);
        for r in 0..h {
            for c in 0..w {
                let best = (0..w * h)
                    .filter(|&k| seed[k])
                    .map(|k| (((k / w) as f64 - r as f64).powi(2) + ((k % w) as f64 - c as f64).powi(2)).sqrt())
                    .fold(f64::INFINITY, f64::min);
                assert!((d[r * w + c] as f64 - best).abs() < 1e-4, "{r},{c}: {} vs {best}", d[r * w + c]);
                let k = i[r * w + c] as usize;
                assert!(seed[k]);
                let got = (((k / w) as f64 - r as f64).powi(2) + ((k % w) as f64 - c as f64).powi(2)).sqrt();
                assert!((got - best).abs() < 1e-4);
            }
        }
        let (d, i) = nearest(&vec![false; 6], 3, 2);
        assert!(d.iter().all(|x| x.is_infinite()) && i.iter().all(|&x| x == u32::MAX));
    }
}
