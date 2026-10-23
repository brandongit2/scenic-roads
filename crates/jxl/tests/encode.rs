use sha2::{Digest, Sha256};

/// A test pattern (not an image's statistics).
fn pattern() -> Vec<u8> {
    (0..256 * 256 * 3).map(|i| (i * 7 % 251) as u8).collect()
}

/// A terrain tile as the fine terrain makes them: heights in 1 cm steps (a slope, ridges and a
/// rough patch, from integers alone), as Terrarium RGB (roadcore::grid::terrarium_rgb's encoding).
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
fn a_tile_encodes_to_the_same_jxl_codestream_each_time() {
    let rgb = pattern();
    let a = jxl::lossless_rgb8(&rgb, 256, 256, 6).unwrap();
    assert_eq!(&a[..2], &[0xff, 0x0a], "a bare codestream");
    assert_eq!(a, jxl::lossless_rgb8(&rgb, 256, 256, 6).unwrap());
    assert!(jxl::lossless_rgb8(&rgb[1..], 256, 256, 6).is_err());
    assert_eq!(jxl::version(), "0.12.0");
}

/// Every Mac makes the same bytes (a fine tile's content name and so its pack's follow them):
/// golden hashes at the fine terrain's effort (6: terrain_fine::JXL_EFFORT), the same on the M1, the M4 and the Mac mini (#543, 22 Oct; #599, 23 Oct). A libjxl change, or a
/// Mac whose CPU picks another SIMD target, shows here.
#[test]
fn every_mac_makes_the_same_bytes() {
    let got = [
        sha(&jxl::lossless_rgb8(&pattern(), 256, 256, 6).unwrap()),
        sha(&jxl::lossless_rgb8(&terrain(), 256, 256, 6).unwrap()),
    ];
    assert_eq!(
        got,
        [
            "738f0d73550dbb717326014708646d092464a499995358ac3c6a6bacd06d0eba",
            "84543cafdb894469856c68cfdfc784d71ec0858d336ac974f38adb5edb79f706"
        ]
    );
}
