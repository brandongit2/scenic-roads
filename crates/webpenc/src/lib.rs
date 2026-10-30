//! Lossless WebP encoding of 8-bit RGB images through libwebp 1.6.0 (libwebp-sys's copy, compiled
//! by cc and linked statically, natively and in the WebAssembly build alike): the bytes `cwebp
//! -lossless -z <level> -exact -metadata none` makes. For the terrain's Terrarium tiles
//! (pipeline::terrain_codec), which Chrome and Safari decode natively with every value exact.

mod entropy;

use libwebp_sys as w;

/// The linked libwebp's encoder version, as it says it ("1.6.0"): what a key naming the encoder's
/// bytes names.
pub fn version() -> String {
    // SAFETY: no arguments, no state.
    let v = unsafe { w::WebPGetEncoderVersion() } as u32;
    format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff)
}

/// RGB image `rgb` (`width` × `height`, 3 bytes a pixel, row by row) as a lossless WebP file at
/// libwebp's lossless preset `level` (0–9, cwebp's `-z`): every pixel exact, no metadata, one
/// thread.
pub fn lossless_rgb8(rgb: &[u8], width: u32, height: u32, level: u8) -> Result<Vec<u8>, String> {
    if rgb.len() != width as usize * height as usize * 3 {
        return Err(format!("{} bytes for {width} × {height} RGB", rgb.len()));
    }
    if level > 9 || width == 0 || height == 0 || width > 16383 || height > 16383 {
        return Err(format!("level {level}, {width} × {height}: past libwebp's bounds"));
    }
    entropy::install();
    let mut config = w::WebPConfig::new().map_err(|_| "libwebp: config".to_string())?;
    // SAFETY: `config` is initialised; the preset sets its lossless fields.
    if unsafe { w::WebPConfigLosslessPreset(&mut config, level.into()) } == 0 {
        return Err("libwebp: lossless preset".into());
    }
    config.exact = 1;
    config.thread_level = 0;
    let mut pic = w::WebPPicture::new().map_err(|_| "libwebp: picture".to_string())?;
    pic.use_argb = 1;
    pic.width = width as i32;
    pic.height = height as i32;
    // SAFETY: a plain C struct, initialised by WebPMemoryWriterInit before use.
    let mut out: w::WebPMemoryWriter = unsafe { std::mem::zeroed() };
    // SAFETY: `pic` is initialised and sized; `rgb` holds width × height × 3 bytes in rows of
    // width × 3; the writer appends to `out`, cleared once below; the picture's buffers freed once.
    unsafe {
        w::WebPMemoryWriterInit(&mut out);
        if w::WebPPictureImportRGB(&mut pic, rgb.as_ptr(), (width * 3) as i32) == 0 {
            w::WebPPictureFree(&mut pic);
            return Err("libwebp: import".into());
        }
        pic.writer = Some(w::WebPMemoryWrite);
        pic.custom_ptr = (&mut out as *mut w::WebPMemoryWriter).cast();
        let ok = w::WebPEncode(&config, &mut pic);
        let code = pic.error_code;
        w::WebPPictureFree(&mut pic);
        let bytes = if ok != 0 {
            Ok(std::slice::from_raw_parts(out.mem, out.size).to_vec())
        } else {
            Err(format!("libwebp: encode ({code:?})"))
        };
        w::WebPMemoryWriterClear(&mut out);
        bytes
    }
}
