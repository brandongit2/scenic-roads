use sha2::{Digest, Sha256};

/// A test pattern (not an image's statistics).
fn pattern() -> Vec<u8> {
    (0..256 * 256 * 3).map(|i| (i * 7 % 251) as u8).collect()
}

/// A terrain tile as the terrain makes them: heights in 1 cm steps (a slope, ridges and a rough
/// patch, from integers alone), as Terrarium RGB (roadcore::grid::terrarium_rgb's encoding).
fn terrain() -> Vec<u8> {
    let mut rgb = Vec::with_capacity(256 * 256 * 3);
    for j in 0..256i64 {
        for i in 0..256i64 {
            let ridge = ((i * 3 + j * 2) % 128 - 64).abs() * 40;
            let rough = if i > 160 && j > 160 { (i * 7919 + j * 104_729) % 37 } else { 0 };
            let cm = 120_000 + 31 * i + 57 * j + ridge + rough;
            let v = (32768 << 8) + (cm * 256 + 50) / 100;
            rgb.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
        }
    }
    rgb
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn a_tile_encodes_to_the_same_lossless_webp_each_time() {
    let rgb = terrain();
    let a = webpenc::lossless_rgb8(&rgb, 256, 256, 6).unwrap();
    assert_eq!(&a[..4], b"RIFF");
    assert_eq!(&a[8..16], b"WEBPVP8L", "lossless, no extended header (no metadata)");
    assert_eq!(a, webpenc::lossless_rgb8(&rgb, 256, 256, 6).unwrap());
    assert!(webpenc::lossless_rgb8(&rgb[1..], 256, 256, 6).is_err());
    assert_eq!(webpenc::version(), "1.6.0");
}

/// Every Mac makes the same bytes (a tile's content name and so its pack's follow them), and the
/// WebAssembly build (pipeline's terrainsub) the same as natively: golden hashes at the terrain's
/// level (6: pipeline::terrain_codec::LEVEL). A libwebp change, or a build taking another SIMD
/// path to other bytes, shows here.
#[test]
fn every_mac_makes_the_same_bytes() {
    let got = [
        sha(&webpenc::lossless_rgb8(&pattern(), 256, 256, 6).unwrap()),
        sha(&webpenc::lossless_rgb8(&terrain(), 256, 256, 6).unwrap()),
    ];
    assert_eq!(
        got,
        [
            "583c6ee3f10d5d7e0363bae9874d5de8e36a7bcbbb8ffa330556701e1c16c426",
            "e16a646ed38d2ad3c17f58b6a869ab00a442212d6b3c30e6eab33c9e8815f3e9"
        ]
    );
}
