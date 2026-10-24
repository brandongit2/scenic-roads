//! The shading tiles as stored (docs/formats.md, Pack: `shade-webp`; the owner's decisions of 24 Oct,
//! .claude/notes/SHADING-PYRAMID.md): each pixel's steepness and mean direction (roadcore::shade's
//! codes) as RGB in lossless WebP (`cwebp -lossless -z 6 -exact`, no metadata: crate webpenc), the
//! terrain's codec (crate::terrain_codec). Chrome and Safari decode them natively with every value
//! exact; roadcore::shade::decode reads them back. The slope layer's tiles (the terrain's shading)
//! and the depth's (crate::depth: `depth-shade`) alike.

use anyhow::Result;
use roadcore::shade::{rgb, Px, COHERENT, DIR_SCALE, DIR_STEP, SLOPE_MAX};

/// The encoding packs of shading tiles name (their header's `encoding`, the catalog's).
pub const ENCODING: &str = "shade-webp";
/// libwebp's lossless preset (cwebp's `-z`).
pub const LEVEL: u8 = 6;

/// What a key naming the tiles' bytes names of their making: the encoding, libwebp's version and
/// level, and the codes' constants.
pub fn version() -> String {
    format!(
        "{ENCODING} libwebp {} z{LEVEL} exact; steepness sqrt to {SLOPE_MAX} %, direction 128 + {DIR_SCALE} u by {DIR_STEP}, none under {COHERENT}",
        webpenc::version()
    )
}

/// A tile's pixels (256 × 256) as stored: lossless WebP of their codes.
pub fn encode(px: &[Px]) -> Result<Vec<u8>> {
    webpenc::lossless_rgb8(&rgb(px), 256, 256, LEVEL).map_err(|m| anyhow::anyhow!(m))
}

/// A tile as stored and its pixels as read back (what's stored: their codes' values), from which
/// its parent is made, so a tile made now and one read from its pack later give the same parent.
pub fn stored(px: &[Px]) -> Result<(Vec<u8>, Vec<Px>)> {
    let b = rgb(px);
    let blob = webpenc::lossless_rgb8(&b, 256, 256, LEVEL).map_err(|m| anyhow::anyhow!(m))?;
    Ok((blob, roadcore::shade::pixels(&b, 3)))
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
        let (b, back) = stored(&px).unwrap();
        assert_eq!(&b[8..16], b"WEBPVP8L");
        assert_eq!(roadcore::shade::decode(&b).unwrap(), back);
        assert_eq!(encode(&px).unwrap(), b);
    }
}
