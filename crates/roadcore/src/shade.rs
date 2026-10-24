//! The shading tiles' values (docs/formats.md, Pack: `shade-webp`; .claude/notes/SHADING-PYRAMID.md,
//! option E): per pixel the mean steepness of the ground beneath it, E|g|, and the mean of its
//! downhill-facing unit directions, E[g / |g|] (g the height's gradient, east and south). One
//! pyramid serves both the slope tint (the steepness) and the hill-shading (the map shades the
//! steepness along the mean direction, flattened by its length, the coherence c: 1 on a plain face,
//! about 0 where the faces beneath cancel). A parent pixel is the plain mean of its four children's
//! steepness and direction, so a low zoom keeps the finest level's steepness everywhere and its
//! direction where it agrees.
//!
//! Stored as RGB, 8 bits a channel:
//! - R: 255 × √(steepness ÷ `SLOPE_MAX`) (finer steps on the gentle slopes most ground has); steeper
//!   ground counts as `SLOPE_MAX`.
//! - G, B: the direction's east and south parts, 128 + `DIR_SCALE` × u rounded to steps of `DIR_STEP`
//!   (5 bits: 31 levels, 8 to 248); (128, 128) where the coherence is under `COHERENT`, or the ground
//!   is flat.

use anyhow::{Context, Result};

/// Steepness (percent) at R's top.
pub const SLOPE_MAX: f32 = 400.0;
/// Below this coherence the direction is stored as none (128, 128): its speckle costs bytes and
/// shows as noise (SHADING-PYRAMID.md, S1b §3).
pub const COHERENT: f32 = 0.25;
/// G and B: 128 + DIR_SCALE × u, rounded to multiples of DIR_STEP (±1 → 8 and 248).
pub const DIR_SCALE: f32 = 120.0;
pub const DIR_STEP: f32 = 8.0;

/// A pixel's values: the mean steepness (percent) and the mean unit direction (east, south).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Px {
    pub s: f32,
    pub u: [f32; 2],
}

impl Px {
    /// The pixel of a gradient (east, south; m per m): its steepness and its direction (none on flat
    /// ground).
    pub fn of_gradient(gx: f32, gy: f32) -> Px {
        let g = (gx * gx + gy * gy).sqrt();
        if g > 0.0 {
            Px { s: g * 100.0, u: [gx / g, gy / g] }
        } else {
            Px::default()
        }
    }

    /// The coherence: the mean direction's length.
    pub fn coherence(&self) -> f32 {
        (self.u[0] * self.u[0] + self.u[1] * self.u[1]).sqrt()
    }
}

/// The mean of four pixels (a parent's of its 2 × 2 children).
#[inline]
pub fn mean4(a: Px, b: Px, c: Px, d: Px) -> Px {
    Px {
        s: (a.s + b.s + c.s + d.s) * 0.25,
        u: [(a.u[0] + b.u[0] + c.u[0] + d.u[0]) * 0.25, (a.u[1] + b.u[1] + c.u[1] + d.u[1]) * 0.25],
    }
}

/// A pixel's stored bytes. (The coherence is that of the direction as rounded, so a pixel's codes
/// read back and coded again are the same codes.)
pub fn code(p: Px) -> [u8; 3] {
    let r = (255.0 * (p.s.clamp(0.0, SLOPE_MAX) / SLOPE_MAX).sqrt()).round() as u8;
    let step = |u: f32| (DIR_SCALE * u.clamp(-1.0, 1.0) / DIR_STEP).round();
    let (a, b) = (step(p.u[0]), step(p.u[1]));
    let min = COHERENT * DIR_SCALE / DIR_STEP;
    if a * a + b * b < min * min {
        return [r, 128, 128];
    }
    [r, (128.0 + DIR_STEP * a) as u8, (128.0 + DIR_STEP * b) as u8]
}

/// A pixel from its stored bytes.
pub fn value(c: [u8; 3]) -> Px {
    let r = c[0] as f32 / 255.0;
    Px { s: r * r * SLOPE_MAX, u: [(c[1] as f32 - 128.0) / DIR_SCALE, (c[2] as f32 - 128.0) / DIR_SCALE] }
}

/// A tile's pixels as stored bytes (RGB, row by row).
pub fn rgb(px: &[Px]) -> Vec<u8> {
    px.iter().flat_map(|&p| code(p)).collect()
}

/// The pixels of stored bytes (`rgb`'s, or a decoded tile's: RGB, or RGBA with `ch` 4).
pub fn pixels(b: &[u8], ch: usize) -> Vec<Px> {
    b.chunks_exact(ch).map(|p| value([p[0], p[1], p[2]])).collect()
}

/// A stored tile's pixels (lossless WebP: pipeline::shade_codec).
pub fn decode(bytes: &[u8]) -> Result<Vec<Px>> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("WebP: {e}"))?;
    let (w, h) = d.dimensions();
    let ch = if d.has_alpha() { 4 } else { 3 };
    let mut buf = vec![0u8; d.output_buffer_size().context("WebP: too large")?];
    d.read_image(&mut buf).map_err(|e| anyhow::anyhow!("WebP: {e}"))?;
    Ok(pixels(&buf[..(w * h) as usize * ch], ch))
}

/// A 256 × 256 tile's pixels merged 2 × 2: its parent's quadrant (128 × 128).
pub fn quadrant(v: &[Px]) -> Vec<Px> {
    let mut q = Vec::with_capacity(128 * 128);
    for j in 0..128 {
        for i in 0..128 {
            let (a, b) = ((2 * j) * 256 + 2 * i, (2 * j + 1) * 256 + 2 * i);
            q.push(mean4(v[a], v[a + 1], v[b], v[b + 1]));
        }
    }
    q
}

/// Tile (x, y)'s pixels from those of its ancestor `dz` levels up (`p`), each value bilinear as
/// crate::grid::upsampled makes the terrain: steepness and direction alike.
pub fn upsampled(p: &[Px], dz: u8, x: u32, y: u32) -> Vec<Px> {
    let part = |f: &dyn Fn(&Px) -> f32| crate::grid::upsampled(&p.iter().map(f).collect::<Vec<_>>(), dz, x, y);
    let (s, ux, uy) = (part(&|p| p.s), part(&|p| p.u[0]), part(&|p| p.u[1]));
    (0..s.len()).map(|i| Px { s: s[i], u: [ux[i], uy[i]] }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_comes_back_as_its_codes_say() {
        // Steepness: within half a step of the square-root scale.
        for s in [0.0f32, 0.3, 4.0, 37.0, 150.0, 399.0] {
            let b = value(code(Px { s, u: [1.0, 0.0] })).s;
            let step = 2.0 * (s.max(0.01) * SLOPE_MAX).sqrt() / 255.0;
            assert!((b - s).abs() <= step * 0.51 + 1e-3, "{s} came back {b}");
        }
        assert_eq!(value(code(Px { s: 900.0, u: [0.0, 0.0] })).s, SLOPE_MAX);
        // Direction: 31 levels, ±1 and 0 exact; under the coherence threshold, none.
        assert_eq!(code(Px { s: 10.0, u: [1.0, -1.0] }), [40, 248, 8]);
        assert_eq!(value(code(Px { s: 10.0, u: [1.0, -1.0] })).u, [1.0, -1.0]);
        assert_eq!(code(Px { s: 10.0, u: [0.2, 0.1] })[1..], [128, 128]);
        assert_eq!(value(code(Px { s: 10.0, u: [0.27, 0.0] })).u, [32.0 / DIR_SCALE, 0.0]);
        // Coded, read back and coded again: the same codes (the threshold on the rounded direction:
        // (0.23, 0.101) is over it, rounded under it).
        assert_eq!(code(Px { s: 10.0, u: [0.23, 0.101] })[1..], [128, 128]);
        for i in 0..20_000u32 {
            let (a, b) = ((i as f32 * 0.6180339).fract() * 2.0 - 1.0, (i as f32 * 0.4142135).fract() * 2.0 - 1.0);
            let c = code(Px { s: i as f32 * 0.02, u: [a * 0.7, b * 0.7] });
            assert_eq!(code(value(c)), c, "{a} {b}");
        }
        for u in [-1.0f32, -0.61, -0.3, 0.29, 0.5, 0.97] {
            let c = code(Px { s: 1.0, u: [u, 0.0] })[1];
            assert_eq!(c % 8, 0);
            assert!((value([0, c, 128]).u[0] - u).abs() <= DIR_STEP / DIR_SCALE / 2.0 + 1e-6);
        }
    }

    #[test]
    fn a_gradient_is_its_steepness_and_direction_and_parents_their_means() {
        let p = Px::of_gradient(0.3, -0.4);
        assert!((p.s - 50.0).abs() < 1e-4 && (p.u[0] - 0.6).abs() < 1e-6 && (p.u[1] + 0.8).abs() < 1e-6);
        assert_eq!(Px::of_gradient(0.0, 0.0), Px::default());
        // A ridge: two faces east and west cancel to no direction, their steepness kept.
        let (e, w) = (Px::of_gradient(0.5, 0.0), Px::of_gradient(-0.5, 0.0));
        let m = mean4(e, w, e, w);
        assert_eq!((m.s, m.coherence()), (50.0, 0.0));
        let v: Vec<Px> = (0..65536).map(|i| if i % 2 == 0 { e } else { w }).collect();
        assert!(quadrant(&v).iter().all(|q| *q == m));
    }

    #[test]
    fn a_tile_round_trips_through_its_bytes_and_upsamples_flat_as_flat() {
        let px: Vec<Px> =
            (0..65536).map(|i| Px::of_gradient((i % 256) as f32 / 100.0, -((i / 256) as f32) / 300.0)).collect();
        let b = rgb(&px);
        let back = pixels(&b, 3);
        assert_eq!(rgb(&back), b, "decoding then encoding is the identity");
        let flat = vec![Px { s: 12.0, u: [0.5, 0.0] }; 65536];
        assert!(upsampled(&flat, 3, 17, 9).iter().all(|p| *p == flat[0]));
    }
}
