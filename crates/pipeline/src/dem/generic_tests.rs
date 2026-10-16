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
