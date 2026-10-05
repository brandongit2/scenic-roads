//! Slope tiles for the slope tint: per pixel four slopes (percent), the means of the four quarters
//! of the finest (z12) slopes beneath it, low to high. The map colours each of the four and averages
//! the colours in linear light (web/vite.config.ts), so zoomed out a pixel shows the mix of colours
//! its ground would, not the colour of its mean slope: the mean of a white cliff band and yellow
//! slopes beside it was a slightly brighter yellow, where the eye sees mostly white.
//!
//! RGBA PNG, each channel 255 × √(slope ÷ SLOPE_MAX) (finer steps on the gentle slopes most ground
//! has), the rounding carried from one channel to the next so that their mean keeps an eighth of a
//! step: at z12, where the four are one slope, two bits more of it. (Whole percents were too coarse
//! at z12: the map interpolates between pixels, and around a colour threshold integer steps left
//! lens-shaped blotches centred on single pixels.)
use anyhow::Result;

/// Slope (percent) at a channel's top; steeper ground counts as this.
pub const SLOPE_MAX: f32 = 400.0;
/// A pixel's four quarter means, low to high.
pub type Quarters = [f32; 4];

/// The PNG of a tile's quarters (row by row, `w` × `h`).
pub fn encode_slope4(q: &[Quarters], w: u32, h: u32) -> Result<Vec<u8>> {
    png_rgba(&channels(q), w, h)
}

/// A tile's quarters as the PNG's RGBA bytes.
pub fn channels(q: &[Quarters]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(q.len() * 4);
    for v in q {
        let mut carry = 0f32;
        for s in v {
            let e = 255.0 * (s.clamp(0.0, SLOPE_MAX) / SLOPE_MAX).sqrt() + carry;
            let r = e.round().clamp(0.0, 255.0);
            carry = e - r;
            rgba.push(r as u8);
        }
    }
    rgba
}

/// The PNG of RGBA bytes (`channels`, `w` × `h`).
pub fn png_rgba(rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Balanced);
        enc.set_filter(png::Filter::Adaptive);
        let mut wr = enc.write_header()?;
        wr.write_image_data(rgba)?;
    }
    Ok(out)
}

/// A channel's slope (percent).
pub fn slope_of(v: u8) -> f32 {
    let u = v as f32 / 255.0;
    u * u * SLOPE_MAX
}

/// A tile's quarters from its PNG.
pub fn decode_slope4(png_bytes: &[u8]) -> Option<Vec<Quarters>> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    if info.color_type.samples() != 4 {
        return None;
    }
    let n = (info.width * info.height) as usize;
    Some(quarters(&buf[..n * 4]))
}

/// The quarters of RGBA bytes (a decoded tile, or `channels` before it's encoded: the same values).
pub fn quarters(rgba: &[u8]) -> Vec<Quarters> {
    rgba.chunks_exact(4)
        .map(|p| {
            let mut q = [slope_of(p[0]), slope_of(p[1]), slope_of(p[2]), slope_of(p[3])];
            q.sort_by(|a, b| a.total_cmp(b));
            q
        })
        .collect()
}

/// A parent pixel's quarters from its 2×2 children's: their 16 values sorted, the mean of each
/// quarter (a quantile sketch: each child's quarters stand for equal shares of its ground).
#[inline]
pub fn merge4(c: [&Quarters; 4]) -> Quarters {
    // Sorted as f32::total_cmp orders them, by its integer keys (the key map is its own inverse),
    // through a comparator network: each child's four, then Batcher's odd-even merges, 4 + 4 twice
    // and 8 + 8. Branch-free, and the same order as sorting.
    const SORT4: [(usize, usize); 5] = [(0, 1), (2, 3), (0, 2), (1, 3), (1, 2)];
    const MERGE44: [(usize, usize); 9] = [(0, 4), (1, 5), (2, 6), (3, 7), (2, 4), (3, 5), (1, 2), (3, 4), (5, 6)];
    const MERGE88: [(usize, usize); 25] = [
        (0, 8), (1, 9), (2, 10), (3, 11), (4, 12), (5, 13), (6, 14), (7, 15),
        (4, 8), (5, 9), (6, 10), (7, 11),
        (2, 4), (3, 5), (6, 8), (7, 9), (10, 12), (11, 13),
        (1, 2), (3, 4), (5, 6), (7, 8), (9, 10), (11, 12), (13, 14),
    ];
    let key = |b: i32| b ^ (((b >> 31) as u32) >> 1) as i32;
    let mut v = [0i32; 16];
    for (k, q) in c.iter().enumerate() {
        for (i, s) in q.iter().enumerate() {
            v[k * 4 + i] = key(s.to_bits() as i32);
        }
    }
    let mut cs = |a: usize, b: usize| {
        let (x, y) = (v[a], v[b]);
        v[a] = x.min(y);
        v[b] = x.max(y);
    };
    for g in [0, 4, 8, 12] {
        SORT4.iter().for_each(|&(a, b)| cs(g + a, g + b));
    }
    for g in [0, 8] {
        MERGE44.iter().for_each(|&(a, b)| cs(g + a, g + b));
    }
    MERGE88.iter().for_each(|&(a, b)| cs(a, b));
    let f = |i: usize| f32::from_bits(key(v[i]) as u32);
    let m = |a: usize| (f(a) + f(a + 1) + f(a + 2) + f(a + 3)) * 0.25;
    [m(0), m(4), m(8), m(12)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_the_mean() {
        let tiles: Vec<Quarters> = (0..256).map(|i| { let s = i as f32 * 0.37; [s, s, s, s] }).collect();
        let png = encode_slope4(&tiles, 16, 16).unwrap();
        let back = decode_slope4(&png).unwrap();
        for (a, b) in tiles.iter().zip(&back) {
            let ma = a.iter().sum::<f32>() / 4.0;
            let mb = b.iter().sum::<f32>() / 4.0;
            // An eighth of a step of the square-root scale, or so.
            let step = 2.0 * (ma.max(0.01) * SLOPE_MAX).sqrt() / 255.0;
            assert!((ma - mb).abs() <= step * 0.2 + 1e-3, "{ma} vs {mb} (step {step})");
        }
    }

    #[test]
    fn merge_sorts_quarters() {
        let a = [0.0, 0.0, 0.0, 0.0];
        let b = [100.0, 100.0, 100.0, 100.0];
        let q = merge4([&a, &a, &a, &b]);
        assert_eq!(q, [0.0, 0.0, 0.0, 100.0]);
    }

    /// merge4 as a sort and the means of its quarters.
    fn merge_by_sorting(c: [&Quarters; 4]) -> Quarters {
        let mut v: Vec<f32> = c.iter().flat_map(|q| q.iter().copied()).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        let m = |a: usize| (v[a] + v[a + 1] + v[a + 2] + v[a + 3]) * 0.25;
        [m(0), m(4), m(8), m(12)]
    }

    #[test]
    fn merge_network_sorts_everything() {
        // Every pattern of 0s and 1s, so every input (the 0-1 principle), in any order within each
        // child.
        for bits in 0..1u32 << 16 {
            let v: Vec<f32> = (0..16).map(|i| ((bits >> i) & 1) as f32).collect();
            let c: Vec<Quarters> = v.chunks(4).map(|q| [q[0], q[1], q[2], q[3]]).collect();
            let c = [&c[0], &c[1], &c[2], &c[3]];
            assert_eq!(merge4(c), merge_by_sorting(c), "{bits:016b}");
        }
        // The bits of awkward values: zeros of either sign, NaNs, subnormals, infinities.
        let odd = [[3.0f32, -0.0, 0.0, 1.0], [f32::NAN, 2.0, -1.0, 0.5], [7.0, 7.0, f32::INFINITY, -0.0], [1e-40, -1e-40, -f32::NAN, 3.0]];
        let c = [&odd[0], &odd[1], &odd[2], &odd[3]];
        assert_eq!(merge4(c).map(f32::to_bits), merge_by_sorting(c).map(f32::to_bits));
        let mut s = 0x9e3779b97f4a7c15u64;
        for _ in 0..100_000 {
            let q: Vec<Quarters> = (0..4)
                .map(|_| {
                    let mut q = [0f32; 4];
                    for v in &mut q {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        *v = slope_of((s >> 56) as u8);
                    }
                    q
                })
                .collect();
            let c = [&q[0], &q[1], &q[2], &q[3]];
            assert_eq!(merge4(c).map(f32::to_bits), merge_by_sorting(c).map(f32::to_bits));
        }
    }

    #[test]
    fn quarters_of_the_encoded_bytes_are_the_decoded_ones() {
        let q: Vec<Quarters> = (0..256).map(|i| { let s = i as f32 * 1.7; [s, s * 0.5, s * 2.0, 400.0 - s] }).collect();
        let rgba = channels(&q);
        assert_eq!(quarters(&rgba), decode_slope4(&png_rgba(&rgba, 16, 16).unwrap()).unwrap());
    }
}
