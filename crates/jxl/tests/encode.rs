#[test]
fn a_tile_encodes_to_the_same_jxl_codestream_each_time() {
    let rgb: Vec<u8> = (0..256 * 256 * 3).map(|i| (i * 7 % 251) as u8).collect();
    let a = jxl::lossless_rgb8(&rgb, 256, 256, 7).unwrap();
    assert_eq!(&a[..2], &[0xff, 0x0a], "a bare codestream");
    assert_eq!(a, jxl::lossless_rgb8(&rgb, 256, 256, 7).unwrap());
    assert!(jxl::lossless_rgb8(&rgb[1..], 256, 256, 7).is_err());
}
