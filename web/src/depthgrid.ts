// The depth worker's 256-px grids (depth.worker.ts) for a zoom the server doesn't store: above the
// layer's least zoom, from the tiles of that zoom under it (coarsen); past its greatest, from its
// ancestor of that zoom (refine). Kept apart from the worker so the tests can run them
// (web/test/depthgrid.test.ts).

/** Depth at or over this (m) is none. */
export const NODATA_MIN = 9000;
/** The depth a grid holds where it has none. */
export const SENTINEL = 12000;

/** A server tile's depth (m) and class bytes, 256 × 256; null where the layer has nothing. */
export type Grid = { d: Float32Array; k: Uint8Array } | null;

/**
 * The f × f grids under a tile (`parts`, row by row) as one: each pixel the mean depth of their
 * f × f pixels under it that have data, its class that of the first of them.
 */
export function coarsen(parts: Grid[], f: number): Grid {
  if (parts.every((g) => !g)) return null;
  const s = 256 / f;
  const d = new Float32Array(256 * 256).fill(SENTINEL), k = new Uint8Array(256 * 256);
  parts.forEach((g, i) => {
    if (!g) return;
    const ox = (i % f) * s, oy = Math.floor(i / f) * s;
    for (let r = 0; r < s; r++)
      for (let c = 0; c < s; c++) {
        let sum = 0, n = 0, cls = -1;
        for (let v = 0; v < f; v++)
          for (let u = 0; u < f; u++) {
            const j = (r * f + v) * 256 + c * f + u, dv = g.d[j];
            if (!(dv < NODATA_MIN)) continue;
            sum += dv;
            n++;
            if (cls < 0) cls = g.k[j];
          }
        if (!n) continue;
        const o = (oy + r) * 256 + ox + c;
        d[o] = sum / n;
        k[o] = cls;
      }
  });
  return { d, k };
}

/**
 * Part (`qx`, `qy`) of an ancestor `g` cut into f × f, scaled up f times: depth bilinear between
 * the ancestor's pixel centres where all four have data, else the nearest pixel's (so a lake's
 * depth never blends into none at its shore); classes the nearest pixel's (as the server makes a
 * tile past its zooms from its ancestor, docs/formats.md, Depth).
 */
export function refine(g: Grid, f: number, qx: number, qy: number): Grid {
  if (!g) return null;
  const s = 256 / f;
  const d = new Float32Array(256 * 256), k = new Uint8Array(256 * 256);
  const at = (p: number) => {
    // The ancestor's coordinate of output pixel p's centre, clamped to its pixel centres.
    const u = Math.min(255, Math.max(0, p));
    const a = Math.min(254, Math.floor(u));
    return [a, u - a] as const;
  };
  for (let r = 0; r < 256; r++) {
    const vy = qy * s + (r + 0.5) / f - 0.5;
    const [y0, ty] = at(vy);
    const ny = Math.min(255, Math.max(0, Math.floor(qy * s + (r + 0.5) / f)));
    for (let c = 0; c < 256; c++) {
      const vx = qx * s + (c + 0.5) / f - 0.5;
      const [x0, tx] = at(vx);
      const nx = Math.min(255, Math.max(0, Math.floor(qx * s + (c + 0.5) / f)));
      const near = ny * 256 + nx, o = r * 256 + c;
      k[o] = g.k[near];
      const i00 = y0 * 256 + x0, a = g.d[i00], b = g.d[i00 + 1], e = g.d[i00 + 256], h = g.d[i00 + 257];
      d[o] = a < NODATA_MIN && b < NODATA_MIN && e < NODATA_MIN && h < NODATA_MIN
        ? (a * (1 - tx) + b * tx) * (1 - ty) + (e * (1 - tx) + h * tx) * ty
        : g.d[near];
    }
  }
  return { d, k };
}
