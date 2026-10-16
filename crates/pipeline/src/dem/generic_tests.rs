//! The sampler's own rules: the blend, and footprints.

use super::*;

#[test]
fn a_blend_is_continuous_and_exact_past_its_band() {
    assert_eq!(blend(10.0, 20.0, 0.0, 40.0), 20.0);
    assert_eq!(blend(10.0, 20.0, 40.0, 40.0), 10.0);
    assert_eq!(blend(10.0, 20.0, 1e9, 40.0), 10.0);
    assert_eq!(blend(10.0, 20.0, 20.0, 40.0), 15.0);
    let a = blend(10.0, 20.0, 39.999, 40.0);
    assert!((a - 10.0).abs() < 1e-3);
    assert_eq!(smoothstep(-1.0), 0.0);
    assert_eq!(smoothstep(2.0), 1.0);
}

#[test]
fn footprints_are_half_open() {
    let b = [0.0, 0.0, 1.0, 1.0];
    assert!(in_box(&b, 0.0, 1.0) && !in_box(&b, 1.0, 0.5) && !in_box(&b, 0.5, 0.0));
    assert_eq!(stored_name("https://e.org/z.zip#N48E002_X.tif"), "N48E002_X.tif");
    assert_eq!(stored_name("https://e.org/a/b.tif"), "b.tif");
}

#[test]
fn a_pyramids_tile_not_256_pixels_is_refused() {
    let png = |n: u32| {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, n, n);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&[128u8, 0, 0].repeat((n * n) as usize)).unwrap();
        out
    };
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://e.org/3/1/1.png".into(), Some(png(256)));
    f.0.insert("https://e.org/3/2/1.png".into(), Some(png(512)));
    let o = Opts { fetch: &f, store: None, read_only: true, earlier: None, pool: None };
    let t = TileSet { url: "https://e.org/{z}/{x}/{y}.png", zoom: 3, encoding: "terrarium", o: &o };
    assert_eq!(t.tile(1, 1).unwrap().unwrap().0, 256);
    assert!(t.tile(2, 1).unwrap_err().to_string().contains("512 × 512"));
}
