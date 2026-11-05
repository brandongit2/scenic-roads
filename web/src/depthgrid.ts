// The depth's zooms (depthZooms: which the server stores, which the map asks for), the worker's
// 256-px grids above the least stored (coarsen) and the values its relief and contours draw
// (reliefValues, contourValues; depth.worker.ts). Kept apart from depth.ts and the worker so the
// tests can run them (web/test/depthgrid.test.ts).

/** Depth at or over this (m) is none. */
export const NODATA_MIN = 9000;
/** The depth a grid holds where it has none. */
export const SENTINEL = 12000;

/** The catalogue's depth layers' zooms; the server answers 404 outside them. */
export interface DepthZooms {
  /** The least stored (0 once depth-root has built the root packs, else 3): the worker makes the coarser. */
  least: number;
  /** The map's deepest relief and contour tile: one before the greatest stored, a map tile z being
   * made from the server's z + 1 (#429: a hard-coded 16 asked for z17, every one a 404, and the
   * depth vanished from map zoom 15.5). MapLibre scales it up past that. */
  tiles: number;
  /** The floor's shading's greatest (its 256-px tiles straight from the server). */
  shade: number;
}

/** The zooms of the catalogue's `layers` (null: no depth). */
export function depthZooms(layers: Record<string, unknown> | undefined): DepthZooms | null {
  if (!layers || !('depth' in layers) || !('depth-class' in layers)) return null;
  type Z = { minzoom?: number; maxzoom?: number } | undefined;
  const zs = (['depth', 'depth-class'] as const).map((k) => layers[k] as Z);
  const shade = (layers['depth-shade'] as Z)?.maxzoom ?? 16;
  return { least: Math.max(...zs.map((l) => l?.minzoom ?? 0)), tiles: Math.min(...zs.map((l) => l?.maxzoom ?? 16)) - 1, shade };
}

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

/** The least relief value of water that dries at low tide: the colour ramp's drying colours start
 * here (depth.ts), and no other water reaches them. */
export const DRY_MIN = 0.05;
/** How far (px) a depth reaches past its water's data for the colour: across the shore's pixels with
 * none (a coarse tile's water rule, a lake under a pixel) to the water the map draws. */
export const REACH = 8;

/** A square of the worker's composite: depth (m; ≥ NODATA_MIN none), class bytes (bits 0–2 the
 * class, 0 none: land; bit 3 dries) and the basemap's water (1). */
export interface Square {
  d: Float32Array;
  k: Uint8Array;
  w: Uint8Array;
}

/** A pixel of the water's own data: a depth with a class (a value classed none isn't the water's). */
const own = (c: Square, i: number) => c.d[i] < NODATA_MIN && (c.k[i] & 7) !== 0;

/**
 * Each pixel's nearest pixel of the water's own data (`own`; with `water`, of the basemap's water
 * alone), by two-pass chamfer propagation (-1: none in the square). A depth crosses the land only
 * to reach the shore: once on the basemap's land it never comes back into its water, so water
 * without data across a dike or a spit (another water body) stays without.
 */
export function nearest(c: Square, water: boolean): { src: Int32Array; dist: Float32Array } {
  const N = Math.round(Math.sqrt(c.d.length));
  const src = new Int32Array(N * N).fill(-1);
  const dist = new Float32Array(N * N).fill(1e9);
  for (let i = 0; i < N * N; i++) {
    if (own(c, i) && (!water || c.w[i])) {
      src[i] = i;
      dist[i] = 0;
    }
  }
  const relax = (i: number, j: number, cost: number) => {
    if (src[j] >= 0 && dist[j] + cost < dist[i] && (src[j] === j || !c.w[i] || c.w[j])) {
      dist[i] = dist[j] + cost;
      src[i] = src[j];
    }
  };
  for (let y = 0; y < N; y++) for (let x = 0; x < N; x++) {
    const i = y * N + x;
    if (x > 0) relax(i, i - 1, 1);
    if (y > 0) {
      relax(i, i - N, 1);
      if (x > 0) relax(i, i - N - 1, 1.414);
      if (x < N - 1) relax(i, i - N + 1, 1.414);
    }
  }
  for (let y = N - 1; y >= 0; y--) for (let x = N - 1; x >= 0; x--) {
    const i = y * N + x;
    if (x < N - 1) relax(i, i + 1, 1);
    if (y < N - 1) {
      relax(i, i + N, 1);
      if (x < N - 1) relax(i, i + N + 1, 1.414);
      if (x > 0) relax(i, i + N - 1, 1.414);
    }
  }
  return { src, dist };
}

/** The contours' values: water with data as it is, land 0, water without data its nearest depth in
 * the water. */
export function contourValues(c: Square): Float32Array {
  const { src } = nearest(c, true);
  const v = new Float32Array(c.d.length);
  for (let i = 0; i < v.length; i++) v[i] = c.w[i] && src[i] >= 0 ? c.d[src[i]] : 0;
  return v;
}

/** Relief values: a pixel's water's depth (its nearest within REACH where it has none), SENTINEL
 * beyond; `masked`, on land too (the map draws it only in its water: depth.ts), else only in the
 * basemap's water. Only water marked as drying at low water takes the ramp's drying colours (at
 * least DRY_MIN); any other is at most 0, the ramp's shallowest blue (the depth itself stays as it
 * is everywhere else: footer, shade). */
export function reliefValues(c: Square, masked: boolean): Float32Array {
  const { src, dist } = nearest(c, false);
  const v = new Float32Array(c.d.length).fill(SENTINEL);
  for (let i = 0; i < v.length; i++) {
    const j = src[i];
    if (j < 0 || dist[i] > REACH || (!masked && !c.w[i])) continue;
    v[i] = c.k[j] & 8 ? Math.max(c.d[j], DRY_MIN) : Math.min(c.d[j], 0);
  }
  return v;
}
