//! The shading tiles as stored (docs/formats.md, Pack: `shade-webp`; the owner's decisions of 24 Oct,
//! .claude/notes/SHADING-PYRAMID.md): each pixel's steepness and mean direction (roadcore::shade's
//! codes) as RGB in lossless WebP (`cwebp -lossless -z 6 -exact`, no metadata: crate webpenc), the
//! terrain's codec (crate::terrain_codec). Chrome and Safari decode them natively with every value
//! exact; roadcore::shade::decode reads them back. The slope layer's tiles (the terrain's shading)
//! and the depth's (crate::depth: `depth-shade`) alike.

use anyhow::Result;
use roadcore::shade::{rgb, Px, COHERENT, DIR_SCALE, DIR_STEP, SLOPE_MAX};

/// The encoding packs of shading tiles name (their header's `encoding`, the catalogue's).
pub const ENCODING: &str = "shade-webp";
/// libwebp's lossless preset (cwebp's `-z`).
pub const LEVEL: u8 = 6;

/// What a key naming the tiles' bytes names of their making: the encoding, libwebp's version and
/// level, the codes' constants, and how a parent is made (crate::slope_pack::stored).
/// Made once a process (its parts are constants and the linked library's).
pub fn version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| format!(
        "{ENCODING} libwebp {} z{LEVEL} exact; steepness sqrt to {SLOPE_MAX} %, direction 128 + {DIR_SCALE} u by {DIR_STEP}, none under {COHERENT}; parents from full precision within a job",
        webpenc::version()
    ))
    .clone()
}

/// A tile's pixels (256 × 256) as stored: lossless WebP of their codes.
pub fn encode(px: &[Px]) -> Result<Vec<u8>> {
    webpenc::lossless_rgb8(&rgb(px), 256, 256, LEVEL).map_err(|m| anyhow::anyhow!(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile comes back as its codes say, bit for bit, through the WebP decoder.
    #[test]
    fn a_tile_comes_back_as_stored() {
        let px: Vec<Px> = (0..65536)
            .map(|i| Px::of_gradient(((i % 256) as f32 * 0.02).sin(), ((i / 256) as f32 * 0.03).cos() * 0.4))
            .collect();
        let b = encode(&px).unwrap();
        assert_eq!(&b[8..16], b"WEBPVP8L");
        assert_eq!(roadcore::shade::decode(&b).unwrap(), roadcore::shade::pixels(&rgb(&px), 3));
    }
}
