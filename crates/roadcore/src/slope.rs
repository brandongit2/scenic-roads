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
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Balanced);
        enc.set_filter(png::Filter::Adaptive);
        let mut wr = enc.write_header()?;
        wr.write_image_data(&rgba)?;
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
    Some((0..n).map(|i| {
        let mut q = [slope_of(buf[i * 4]), slope_of(buf[i * 4 + 1]), slope_of(buf[i * 4 + 2]), slope_of(buf[i * 4 + 3])];
        q.sort_by(|a, b| a.total_cmp(b));
        q
    }).collect())
}

/// A parent pixel's quarters from its 2×2 children's: their 16 values sorted, the mean of each
/// quarter (a quantile sketch: each child's quarters stand for equal shares of its ground).
pub fn merge4(c: [&Quarters; 4]) -> Quarters {
    let mut v = [0f32; 16];
    for (k, q) in c.iter().enumerate() {
        v[k * 4..k * 4 + 4].copy_from_slice(&q[..]);
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let m = |a: usize| (v[a] + v[a + 1] + v[a + 2] + v[a + 3]) * 0.25;
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
}
