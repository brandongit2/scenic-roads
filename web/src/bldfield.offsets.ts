// The field's offsets to the buildings' centroids (field5-webp's second block a class) made ready
// for the GPU: premultiplied by each texel's share, so filtered reads (bilinear taps, the mipmaps'
// box means) are sums of share × offset, and the march divides them by the share it reads there from
// the class's texture (same place, same level): the share-weighted mean offset. Filtered as stored
// (not premultiplied), a texel's offset was diluted by its empty or other buildings' neighbours: a
// small building filling 1 of 4 texels kept a quarter of its offset at mip 1. What stays: the mean
// is of offsets from each texel's own centre, taken from the read's point, so a read at level L can
// put a centroid up to half its texel (2^(L−1) level-0 texels) from where the builder's level-L
// texel would (that level's offsets are from its own texels' centres); within the pixel's own
// footprint. (A position instead of an offset would avoid that, but its ratio to a share read from
// another texture, R11F's 5-bit mantissa, errs by up to 3 % of the position: texels, not a few
// hundredths of one; and a share of its own beside it doubles the texture.) And each 16 × 16-texel
// cell's reach: how many cells away a read in it can put a roof's centroid. Pure functions (the
// worker's, and tools/bldcheck/offmip.mjs's check).

/** Cells beyond a cell's own and its neighbours' largest offset that a read in it can reach: a read
 * at level L mixes texels up to 2^(L+1) − 1 texels away (its bilinear taps' footprints), so 2 cells
 * (32 texels) cover the levels to 4. Coarser reads (a 32-texel cell is under 1/16 of a 512-texel
 * tile, so a few pixels across) can reach further; the march clamps a roof's ground to the cell's
 * highest ground within its reach, so such a read keeps a roof no higher than the cell's runs start. */
export const REACH_PAD = 2;

/**
 * Share × offset, RG a texel (level-0 texels), a layer a class held (layer-major).
 * @param offsets the stored offsets, RG + 128 a texel, layer-major (`n` layers of T × T)
 * @param shares each layer's shares (T × T), as the class textures hold them
 */
export function premultiplied(offsets: Uint8Array, shares: Float32Array[], T: number): Float32Array {
  const N = T * T;
  const out = new Float32Array(offsets.length);
  shares.forEach((f, l) => {
    for (let i = 0; i < N; i++) {
      const s = f[i];
      if (!s) continue;
      const k = (l * N + i) * 2;
      out[k] = s * (offsets[k] - 128);
      out[k + 1] = s * (offsets[k + 1] - 128);
    }
  });
  return out;
}

/** Each 16 × 16-texel cell's reach in cells (a 32 × 32 grid for T = 512): its own and its
 * neighbours' largest offset, rounded up to cells, + REACH_PAD; 0 where none of them holds a roof. */
export function reachOf(offsets: Uint8Array, n: number, T: number): Uint8Array {
  const N = T * T, C = T / 16;
  const own = new Uint8Array(C * C);
  const has = new Uint8Array(C * C);
  for (let l = 0; l < n; l++)
    for (let i = 0; i < N; i++) {
      const a = offsets[(l * N + i) * 2], b = offsets[(l * N + i) * 2 + 1];
      const k = (((i / T) | 0) >> 4) * C + ((i % T) >> 4);
      if (a !== 128 || b !== 128) has[k] = 1;
      const d = Math.max(Math.abs(a - 128), Math.abs(b - 128));
      if (d > own[k]) own[k] = d;
    }
  const reach = new Uint8Array(C * C);
  for (let y = 0; y < C; y++) for (let x = 0; x < C; x++) {
    let d = 0, any = 0;
    for (let yy = Math.max(0, y - 1); yy <= Math.min(C - 1, y + 1); yy++)
      for (let xx = Math.max(0, x - 1); xx <= Math.min(C - 1, x + 1); xx++) {
        d = Math.max(d, own[yy * C + xx]);
        any |= has[yy * C + xx];
      }
    reach[y * C + x] = any ? Math.ceil(d / 16) + REACH_PAD : 0;
  }
  return reach;
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
