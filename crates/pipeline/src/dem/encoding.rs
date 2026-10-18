//! Elevation tile encodings (docs/inputs.md §6.8, `[data] encoding`): formats, decoded by generic
//! code. Each tile is a PNG; a pixel's height in metres from its red, green and blue:
//! - `gsi-png`: x = R·2¹⁶ + G·2⁸ + B, h = 0.01·x below 2²³, 0.01·(x − 2²⁴) above, 2²³ no data;
//! - `terrarium`: h = R·256 + G + B/256 − 32768;
//! - `terrain-rgb`: h = −10000 + 0.1·(R·2¹⁶ + G·2⁸ + B).

use anyhow::{bail, Context, Result};

/// A tile's heights (metres; -1e9 where there's none), and its width and height.
pub fn decode(encoding: &str, png: &[u8]) -> Result<(usize, usize, Vec<f32>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info()?;
    let mut buf = vec![0u8; r.output_buffer_size().context("PNG size")?];
    let info = r.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = info.color_type.samples();
    let mut out = Vec::with_capacity(w * h);
    for p in buf[..w * h * ch].chunks_exact(ch) {
        // As PIL's convert("RGB"): grey repeated, alpha dropped.
        let (r, g, b) = if ch >= 3 { (p[0], p[1], p[2]) } else { (p[0], p[0], p[0]) };
        let v = match encoding {
            "gsi-png" => {
                let x = (r as i64) << 16 | (g as i64) << 8 | b as i64;
                if x == 1 << 23 {
                    -1e9
                } else {
                    (if x < 1 << 23 { x } else { x - (1 << 24) }) as f32 * 0.01f32
                }
            }
            "terrarium" => (r as f32 * 256.0 + g as f32 + b as f32 / 256.0) - 32768.0,
            "terrain-rgb" => -10000.0 + (((r as u32) << 16 | (g as u32) << 8 | b as u32) as f32) * 0.1,
            e => bail!("the tile encoding {e} isn't one the app knows"),
        };
        out.push(v);
    }
    Ok((w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(rgb: &[[u8; 3]]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, rgb.len() as u32, 1);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&rgb.concat()).unwrap();
        out
    }

    #[test]
    fn the_encodings() {
        // 12.34 m and -5.5 m in GSI's, and its no data.
        let neg = (1i64 << 24) - 550;
        let g = decode("gsi-png", &png(&[[0, 4, 210], [(neg >> 16) as u8, (neg >> 8) as u8, neg as u8], [128, 0, 0]]))
            .unwrap();
        assert_eq!(g.2, vec![1234.0f32 * 0.01, -550.0f32 * 0.01, -1e9]);
        assert_eq!(decode("terrarium", &png(&[[128, 100, 128]])).unwrap().2, vec![100.5]);
        assert_eq!(decode("terrain-rgb", &png(&[[1, 134, 160]])).unwrap().2, vec![-10000.0 + 100000.0 * 0.1]);
        assert!(decode("lerc", &png(&[[0, 0, 0]])).is_err());
    }
}
