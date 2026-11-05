// The depth's zooms (depthZooms: which the server stores, which the map asks for) and the worker's
// 256-px grids above the least stored (coarsen, depth.worker.ts). Kept apart from depth.ts and the
// worker so the tests can run them (web/test/depthgrid.test.ts).

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
