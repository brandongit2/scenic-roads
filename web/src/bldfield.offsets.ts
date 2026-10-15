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
// cell's centroid boxes: where a read in it can put a roof's centroid, by the read's level, so the
// march starts a cell's runs only as high as the ground there can lift a roof. Pure functions (the
// worker's, and tools/bldcheck/offmip.mjs's check).

/** The levels whose reads the near box covers (0–2: a read mixes texels under 2^(L+1) = 8 texels
 * away) and the far box (to 4: under 32); a read at a coarser level is clamped to the far box's
 * highest ground by the march (a 16-texel cell is then under a texel of the read: a few pixels). */
export const NEAR_LEVEL = 2, FAR_LEVEL = 4;

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

/**
 * Each 16 × 16-texel cell's boxes of where a read in it can put a roof's centroid (level-0 texels
 * from the tile's corner, may run past its edges): `near` for reads at levels to NEAR_LEVEL, `far`
 * to FAR_LEVEL; 4 floats a cell (x0, y0, x1, y1), x0 > x1 where no roof can be read. A read's
 * centroid is p + Σ f o ÷ Σ f over the texels it mixes, p its point: Σ f (c + (p − t)) ÷ Σ f, c a
 * texel's centroid and t its centre, so it lies in the box of those texels' centroids grown by how
 * far p is from them (under 2^(L+1) texels), and by the divide's error (the share read from the class
 * texture, R11F's 5-bit mantissa, mipmapped apart: 4 % of the largest offset, + a texel).
 */
export function centroidBoxes(offsets: Uint8Array, shares: Float32Array[], T: number): { near: Float32Array; far: Float32Array } {
  const N = T * T, C = T / 16;
  const own = new Float32Array(C * C * 5).fill(NaN); // x0 y0 x1 y1, largest |offset|
  for (let k = 0; k < C * C; k++) { own[k * 5] = Infinity; own[k * 5 + 1] = Infinity; own[k * 5 + 2] = -Infinity; own[k * 5 + 3] = -Infinity; own[k * 5 + 4] = 0; }
  shares.forEach((f, l) => {
    for (let i = 0; i < N; i++) {
      if (!f[i]) continue;
      const x = i % T, y = (i / T) | 0, j = (l * N + i) * 2;
      const ox = offsets[j] - 128, oy = offsets[j + 1] - 128;
      const cx = x + 0.5 + ox, cy = y + 0.5 + oy, k = ((y >> 4) * C + (x >> 4)) * 5;
      if (cx < own[k]) own[k] = cx;
      if (cy < own[k + 1]) own[k + 1] = cy;
      if (cx > own[k + 2]) own[k + 2] = cx;
      if (cy > own[k + 3]) own[k + 3] = cy;
      own[k + 4] = Math.max(own[k + 4], Math.abs(ox), Math.abs(oy));
    }
  });
  const grow = (cells: number, reach: number) => {
    const out = new Float32Array(C * C * 4);
    for (let y = 0; y < C; y++) for (let x = 0; x < C; x++) {
      let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity, om = 0;
      for (let yy = Math.max(0, y - cells); yy <= Math.min(C - 1, y + cells); yy++)
        for (let xx = Math.max(0, x - cells); xx <= Math.min(C - 1, x + cells); xx++) {
          const k = (yy * C + xx) * 5;
          if (!(own[k] <= own[k + 2])) continue;
          x0 = Math.min(x0, own[k]); y0 = Math.min(y0, own[k + 1]); x1 = Math.max(x1, own[k + 2]); y1 = Math.max(y1, own[k + 3]);
          om = Math.max(om, own[k + 4]);
        }
      const g = reach + 0.04 * om + 1, k = (y * C + x) * 4;
      if (x0 <= x1) { out[k] = x0 - g; out[k + 1] = y0 - g; out[k + 2] = x1 + g; out[k + 3] = y1 + g; }
      else { out[k] = 1; out[k + 1] = 1; out[k + 2] = 0; out[k + 3] = 0; }
    }
    return out;
  };
  // (a read at level L mixes texels under 2^(L+1) texels from its point: within 1 cell to level 2,
  // 2 cells to level 4)
  return { near: grow(1, 2 ** (NEAR_LEVEL + 1)), far: grow(2, 2 ** (FAR_LEVEL + 1)) };
}

/**
 * The highest ground a cell's box can put a roof's centroid on: the largest of the elevation grid's
 * vertices (GE × GE, `M` cells beyond the tile, a cell 16 texels) around the box's grid squares (the
 * march's ground is bilinear between them, its point clamped to the grid as these indexes are), and
 * `own` (the cell's own corners'); `own` alone where the box is empty.
 */
export function boxGroundMax(box: Float32Array, k: number, ee: Float32Array, GE: number, M: number, own: number): number {
  if (!(box[k * 4] <= box[k * 4 + 2])) return own;
  const vi = (v: number) => Math.min(GE - 1, Math.max(0, v));
  const i0 = vi(Math.floor(box[k * 4] / 16 + M)), i1 = vi(Math.floor(box[k * 4 + 2] / 16 + M) + 1);
  const j0 = vi(Math.floor(box[k * 4 + 1] / 16 + M)), j1 = vi(Math.floor(box[k * 4 + 3] / 16 + M) + 1);
  let m = own;
  for (let j = j0; j <= j1; j++) for (let i = i0; i <= i1; i++) m = Math.max(m, ee[j * GE + i]);
  return m;
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
