// Floats packed into the GPU's small float formats off the main thread (the far field's worker), so
// the page uploads them as they are stored: a texture given as 32-bit floats is converted by the
// browser's GPU process while the page's upload waits, a quarter of a second for a city's tiles
// (task #222). Each value comes out as the GPU process's own conversion makes it (tools/bench/
// packcheck.mjs compares the two in Chrome, for every value the field's class textures can hold).

const f32 = new Float32Array(1);
const u32 = new Uint32Array(f32.buffer);

/**
 * `x` as an unsigned float of `e` exponent bits and `m` mantissa bits (bias 2^(e−1) − 1), rounded to
 * the nearest, ties to even: R11F_G11F_B10F's channels (e 5, m 6 or 5). Negative values and NaN give
 * 0, values past the largest finite one infinity.
 */
export function toUfloat(x: number, e: number, m: number): number {
  f32[0] = x;
  const b = u32[0];
  if (b & 0x80000000 || !(x === x)) return 0;
  const inf = ((1 << e) - 1) << m;
  if (x === Infinity) return inf;
  const bias = (1 << (e - 1)) - 1;
  // (x as float32: its exponent and its 24-bit significand, exact)
  const ex = (b >>> 23) & 0xff, sig = ex ? (b & 0x7fffff) | 0x800000 : b & 0x7fffff;
  const e2 = (ex ? ex : 1) - 127 - 23; // x = sig × 2^e2
  if (sig === 0) return 0;
  // The target's exponent: the float's own, or the least normal one for a subnormal result.
  const top = 31 - Math.clz32(sig) + e2; // ⌊log2 x⌋
  const te = Math.max(top, 1 - bias);
  // The significand at the target's precision: sig × 2^(e2 − (te − m)), rounded half to even.
  const sh = te - m - e2;
  let n: number;
  if (sh <= 0) n = sig * 2 ** -sh;
  else if (sh > 25) n = 0;
  else {
    const q = 2 ** sh, fl = Math.floor(sig / q), r = sig - fl * q;
    n = r > q / 2 || (r === q / 2 && fl & 1) ? fl + 1 : fl;
  }
  // (n is the encoding's low bits with the implicit one: a carry into the next binade, or a subnormal
  // rounding up to the least normal, lands right as an integer)
  const enc = (te - (1 - bias)) * (1 << m) + n;
  return enc >= inf ? inf : enc;
}

/**
 * `x` as a half float (HALF_FLOAT's bits) as Chrome's GPU process converts a float for an R16F texture
 * (ANGLE's float32ToFloat16): to the nearest, ties to even, except that a subnormal result drops the
 * bits below 2^−24's half before rounding, and values from 65,520 to 131,040 come out NaN, as there.
 */
export function toHalf(x: number): number {
  f32[0] = x;
  const b = u32[0];
  const sign = (b & 0x80000000) >>> 16;
  let abs = b & 0x7fffffff;
  if (abs > 0x7f800000) return 0x7fff;
  if (abs > 0x47ffefff) return sign | 0x7c00;
  if (abs < 0x38800000) {
    const mant = (abs & 0x007fffff) | 0x00800000, e = 113 - (abs >>> 23);
    abs = e < 24 ? mant >>> e : 0;
    return (sign | ((abs + 0x00000fff + ((abs >>> 13) & 1)) >>> 13)) & 0xffff;
  }
  return (sign | (((abs + 0xc8000000 + 0x00000fff + ((abs >>> 13) & 1)) >>> 0) >>> 13)) & 0xffff;
}

/** Three floats a texel (R, G, B) as R11F_G11F_B10F texels (UNSIGNED_INT_10F_11F_11F_REV). */
export function packR11G11B10(rgb: Float32Array): Uint32Array {
  const n = rgb.length / 3, out = new Uint32Array(n);
  for (let i = 0; i < n; i++) {
    const r = rgb[i * 3], g = rgb[i * 3 + 1], b = rgb[i * 3 + 2];
    if (!r && !g && !b) continue;
    out[i] = (toUfloat(r, 5, 6) | (toUfloat(g, 5, 6) << 11) | (toUfloat(b, 5, 5) << 22)) >>> 0;
  }
  return out;
}

/** Floats as half floats (HALF_FLOAT). */
export function packHalf(a: Float32Array): Uint16Array {
  const out = new Uint16Array(a.length);
  for (let i = 0; i < a.length; i++) out[i] = toHalf(a[i]);
  return out;
}
