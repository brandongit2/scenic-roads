// The field's roofs at their buildings' centroids (field5-webp's second block a class: each texel's
// offset to its buildings' centroid), made into what the march reads: each texel's roof ground, `dg`,
// how much higher the ground at its centroid is than at the texel's centre (the terrain as drawn,
// the tile's elevation grid), worked out off the main thread once the elevations are known.
//
// On the GPU (R16F, a layer a class held) dg is premultiplied by the texel's share, so filtered and
// mipmapped reads are sums of share × dg, and the march divides them by the share it reads from the
// class's texture at the same place and level: the share-weighted mean, as the class's own top is.
// (Filtered as is, an empty or other buildings' texel would dilute it toward 0.) A mean is no more
// than the largest it mixes, but a read at level L mixes texels up to 2^(L+1) texels away, so no cell
// bound short of the whole tile holds every level; instead the march clamps a roof to its cell's
// start: each cell's runs start at its tallest top plus its largest dg (its texels' and the texel
// around them: what a level-0 read mixes), and a roof is never higher than that. So no run starts
// below a roof, at any level (tools/bldcheck/offmip.mjs), and a coarse read near a building with more
// lift lowers a roof a little at a distance instead. (A max-filtered chain would bound coarse reads
// too, but needs its own texture and mipmaps built by hand, and then the roof would be the max, not
// the mean.) Pure functions (the worker's, and the check's).

/**
 * The march's ground, as the shader's: bilinear in the elevation grid (GE × GE vertices, `M` cells
 * beyond the tile, a cell 16 texels), the point (level-0 texels from the tile's corner) clamped to it.
 */
export function groundAt(ee: Float32Array, GE: number, M: number, px: number, py: number): number {
  const gx = Math.min(Math.max(px / 16 + M, 0), GE - 1 - 0.001), gy = Math.min(Math.max(py / 16 + M, 0), GE - 1 - 0.001);
  const i = Math.floor(gx), j = Math.floor(gy), fx = gx - i, fy = gy - j;
  const a = ee[j * GE + i], b = ee[j * GE + i + 1], c = ee[(j + 1) * GE + i], d = ee[(j + 1) * GE + i + 1];
  return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy;
}

/** A share as the class texture holds it: (R / 255)², rounded to R11F's 5-bit mantissa. */
export function shareOf(r: number): number {
  const f = (r / 255) ** 2;
  if (!(f > 0)) return 0;
  const q = 2 ** (Math.floor(Math.log2(f)) - 5);
  return Math.round(f / q) * q;
}

/**
 * Each texel's roof ground, premultiplied by its share (a layer a class held, layer-major), and each
 * 16 × 16-texel cell's largest dg (≥ 0) over its texels and the texel around them (a 32 × 32 grid for
 * T = 512), where a share is held.
 * @param texels each layer's texels, 3 bytes each: the share's code (√f × 255), the offset + 128 (x, y)
 */
export function roofGround(texels: Uint8Array, n: number, T: number, ee: Float32Array, GE: number, M: number): { dg: Float32Array; dgc: Float32Array } {
  const N = T * T, C = T / 16;
  const dg = new Float32Array(n * N);
  const big = new Float32Array(N); // (each texel's largest dg over the classes, ≥ 0)
  for (let l = 0; l < n; l++)
    for (let i = 0; i < N; i++) {
      const k = (l * N + i) * 3, f = shareOf(texels[k]);
      if (!f) continue;
      const x = (i % T) + 0.5, y = ((i / T) | 0) + 0.5;
      const d = groundAt(ee, GE, M, x + texels[k + 1] - 128, y + texels[k + 2] - 128) - groundAt(ee, GE, M, x, y);
      dg[l * N + i] = f * d;
      if (d > big[i]) big[i] = d;
    }
  const dgc = new Float32Array(C * C);
  for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) {
    const d = big[y * T + x];
    if (!d) continue;
    for (let cy = Math.max(0, (y - 1) >> 4); cy <= Math.min(C - 1, (y + 1) >> 4); cy++)
      for (let cx = Math.max(0, (x - 1) >> 4); cx <= Math.min(C - 1, (x + 1) >> 4); cx++) if (d > dgc[cy * C + cx]) dgc[cy * C + cx] = d;
  }
  return { dg, dgc };
}

/** A level's box mean, as generateMipmap makes one (for the check): `comps` floats a texel. */
export function boxMip(a: Float32Array, size: number, comps: number): Float32Array {
  const h = size >> 1, o = new Float32Array(h * h * comps);
  for (let y = 0; y < h; y++) for (let x = 0; x < h; x++) for (let c = 0; c < comps; c++) {
    const at = (yy: number, xx: number) => a[(yy * size + xx) * comps + c];
    o[(y * h + x) * comps + c] = 0.25 * (at(2 * y, 2 * x) + at(2 * y, 2 * x + 1) + at(2 * y + 1, 2 * x) + at(2 * y + 1, 2 * x + 1));
  }
  return o;
}
