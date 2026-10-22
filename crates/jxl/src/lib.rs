//! Lossless JPEG XL encoding of 8-bit RGB images through libjxl (jpegxl-src's copy, built by
//! CMake, which `mise install` provides; linked statically). Every Mac makes the same bytes: the golden
//! hashes of tests/encode.rs hold on the M1, M4 and M6 alike (#543), so no SIMD target is pinned.
//! For the fine terrain's Terrarium tiles (pipeline::terrain_fine), which Chrome and Safari decode
//! natively with every value exact.

use std::os::raw::{c_int, c_uchar, c_uint, c_void};

extern "C" {
    fn scenic_jxl_lossless_rgb8(
        rgb: *const c_uchar,
        w: c_uint,
        h: c_uint,
        effort: c_int,
        out: *mut *mut c_uchar,
        len: *mut usize,
    ) -> c_int;
    fn free(p: *mut c_void);
    fn JxlEncoderVersion() -> u32;
}

/// The linked libjxl's version, as it says it ("0.12.0"): what a key naming the encoder's bytes names.
pub fn version() -> String {
    // SAFETY: no arguments, no state.
    let v = unsafe { JxlEncoderVersion() };
    format!("{}.{}.{}", v / 1_000_000, v / 1000 % 1000, v % 1000)
}

/// RGB image `rgb` (`w` × `h`, 3 bytes a pixel, row by row) as a lossless JPEG XL codestream, at
/// libjxl's `effort` (1–10; 7 its default).
pub fn lossless_rgb8(rgb: &[u8], w: u32, h: u32, effort: u8) -> Result<Vec<u8>, String> {
    if rgb.len() != w as usize * h as usize * 3 {
        return Err(format!("{} bytes for {w} × {h} RGB", rgb.len()));
    }
    let (mut p, mut n) = (std::ptr::null_mut(), 0usize);
    // SAFETY: `rgb` holds w × h × 3 bytes; on success the shim hands back a malloc'd `p` of `n` bytes.
    let rc = unsafe { scenic_jxl_lossless_rgb8(rgb.as_ptr(), w, h, effort.into(), &mut p, &mut n) };
    if rc != 0 {
        return Err(format!("libjxl failed ({rc})"));
    }
    // SAFETY: as above; copied, then freed once.
    let out = unsafe { std::slice::from_raw_parts(p, n) }.to_vec();
    unsafe { free(p.cast()) };
    Ok(out)
}
