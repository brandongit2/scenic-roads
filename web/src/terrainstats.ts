// The terrain in view, for the elevation tint's histogram and auto-fit: elevation (or slope)
// sampled from the DEM tiles MapLibre is drawing, over the ground in view (the outline the in-view
// lists use), each sample weighted by the ground area it stands for. Tiles at several zooms (a
// tilted view) each count by area, so a far coarse tile counts as much ground as it covers.
// Water is left out: the sea (0 m) and lakes are perfectly flat in the elevation tiles, where
// ground practically never is, so a sample equal to its four neighbours (or with no slope in any of
// its four quarters) is water. With it, an island zoomed out to a speck in the sea had the sea's
// 0 m take over the percentiles and the equalisation.
import type { Map as MLMap } from 'maplibre-gl';
import { Dist } from './roads/stats';

type Dem = { dim: number; stride: number; data: Uint32Array; get(x: number, y: number): number };
type TileLike = { tileID: { canonical: { z: number; x: number; y: number } }; dem?: Dem | null };
type TileManagerLike = { getRenderableIds(): string[]; getTileByID(id: string): TileLike | undefined };

/** Samples per tile side. */
const SIDE = 24;
const BINS = 1024;

const y2lat = (y: number) => (Math.atan(Math.sinh(Math.PI * (1 - 2 * y))) * 180) / Math.PI;

/** Point-in-polygon test with the polygon's bounding box first. */
function inside(poly: [number, number][]): (lng: number, lat: number) => boolean {
  if (poly.length < 3) return () => true;
  let w = Infinity, e = -Infinity, s = Infinity, n = -Infinity;
  for (const [x, y] of poly) {
    w = Math.min(w, x); e = Math.max(e, x); s = Math.min(s, y); n = Math.max(n, y);
  }
  return (x, y) => {
    if (x < w || x > e || y < s || y > n) return false;
    let c = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
      const [xi, yi] = poly[i], [xj, yj] = poly[j];
      if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) c = !c;
    }
    return c;
  };
}

/** The distribution of a raster-dem source's values over the ground in view, in `domain`; null
 * while no tile in view is loaded. `quarters`: the slope source's four slopes a pixel (roadcore::
 * slope, each 255 × √(slope ÷ max) on its own channel), each a quarter of the pixel's ground. */
export function terrainDist(map: MLMap, source: string, domain: [number, number], outline: [number, number][], quarters: { max: number } | null = null): Dist | null {
  const tm = (map as unknown as { style?: { tileManagers?: Record<string, TileManagerLike> } }).style?.tileManagers?.[source];
  if (!tm) return null;
  const inView = inside(outline);
  const bins = new Float64Array(BINS);
  const [lo, hi] = domain;
  const k = BINS / (hi - lo);
  let total = 0;
  for (const id of tm.getRenderableIds()) {
    const t = tm.getTileByID(id);
    const dem = t?.dem;
    if (!t || !dem) continue;
    const { z, x, y } = t.tileID.canonical;
    const n = 2 ** z;
    const step = dem.dim / SIDE;
    const bytes = quarters ? new Uint8Array(dem.data.buffer, dem.data.byteOffset, dem.data.byteLength) : null;
    for (let j = 0; j < SIDE; j++) {
      const py = (j + 0.5) * step;
      const lat = y2lat((y + py / dem.dim) / n);
      // Ground area of a sample: the tile's side shrinks with the cosine of the latitude.
      const c = Math.cos((lat * Math.PI) / 180) / n;
      const w = c * c;
      for (let i = 0; i < SIDE; i++) {
        const px = (i + 0.5) * step;
        const lng = ((x + px / dem.dim) / n) * 360 - 180;
        if (!inView(lng, lat)) continue;
        if (bytes) {
          const at = ((Math.floor(py) + 2) * dem.stride + Math.floor(px) + 2) * 4;
          if ((bytes[at] | bytes[at + 1] | bytes[at + 2] | bytes[at + 3]) === 0) continue; // water
          for (let ch = 0; ch < 4; ch++) {
            const u = bytes[at + ch] / 255;
            bins[Math.max(0, Math.min(BINS - 1, Math.floor((u * u * quarters!.max - lo) * k)))] += w / 4;
          }
          total += w;
          continue;
        }
        const fx = Math.floor(px), fy = Math.floor(py);
        const v = dem.get(fx, fy);
        if (!Number.isFinite(v)) continue;
        if (dem.get(fx - 1, fy) === v && dem.get(fx + 1, fy) === v && dem.get(fx, fy - 1) === v && dem.get(fx, fy + 1) === v) continue; // water
        bins[Math.max(0, Math.min(BINS - 1, Math.floor((v - lo) * k)))] += w;
        total += w;
      }
    }
  }
  return total > 0 ? new Dist(lo, hi, bins, total) : null;
}
