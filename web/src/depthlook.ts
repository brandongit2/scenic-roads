// The depth layers' look (depth.ts; Layers → Depth, ui/depth.ts; `dp=` in links): their settings,
// defaults and colour ramps, apart from the layers so the app's state can name them.
import { PALETTE_ITEMS, baseKey, isRev, paletteFn, stopsFn } from './palettes';
import type { Spacing } from './scaleaxis';
import type { ScaleFields } from './state';

type RGB = [number, number, number];

/** The depth layers' settings (Layers → Depth; `dp=` in links). */
export interface DepthState {
  /** Depth colour and floor shading. */
  on: boolean;
  /** The ramp's spacing over its range: in proportion to depth, or more colours in the shallows. */
  scale: Spacing;
  /** The colour scale (ui/scale.ts ScaleControls): the palette, the range (m of depth; `auto`:
   * fitted to the depths in view, between the `fit` percentiles) and equalization. The low-end
   * fade and the highlight aren't offered for depth. */
  colours: ScaleFields;
  opacity: number;
  /** Floor shading's strength (0 off). */
  shade: number;
  /** Where the light comes from (degrees from map north). */
  light: number;
  /** Water that dries at low tide in its own colour. */
  dry: boolean;
  dryColour: string;
  /** Depth contours. */
  contours: boolean;
  /** Interval by zoom, or fixed (`fixed` m). */
  interval: 'zoom' | 'fixed';
  fixed: number;
  /** Every n-th line bolder and labelled. */
  index: number;
  labels: boolean;
  colour: string;
  weight: number;
  lineOpacity: number;
  /** The 3D terrain from the sea and lake floor, the water's surface drawn over it (depth.ts). */
  floor: boolean;
}

/** Depth ramps made for the dark map: shallow light, deep sinking into the water's own colour. */
const RAMPS: Record<string, { label: string; stops: [number, string][] }> = {
  nautical: { label: 'Nautical blues', stops: [[0, '#cfeef5'], [0.12, '#8fd0e6'], [0.3, '#4fa3d1'], [0.5, '#2f73b0'], [0.72, '#1e4b85'], [0.88, '#152f5a'], [1, '#0e1d36']] },
  quiet: { label: 'Quiet blues', stops: [[0, '#4a6b87'], [0.35, '#304d66'], [0.7, '#1c3043'], [1, '#101d2b']] },
};
const RAMP_FNS = Object.fromEntries(Object.entries(RAMPS).map(([k, r]) => [k, stopsFn(r.stops)]));

/** The depth's colour ramps: its own, then the app's. */
export const DEPTH_PALETTES: { key: string; label: string; group: string }[] = [
  ...Object.entries(RAMPS).map(([key, r]) => ({ key, label: r.label, group: 'Depth' })),
  ...PALETTE_ITEMS,
];

/** Position t (0 shallow … 1 deep) on ramp `key` (reversed with its "_r"). */
export function depthColour(key: string, t: number): RGB {
  t = Math.max(0, Math.min(1, isRev(key) ? 1 - t : t));
  return (RAMP_FNS[baseKey(key)] ?? paletteFn(baseKey(key)))(t);
}

export const rgba = (c: RGB, a = 1) => `rgba(${c.map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255)).join(',')},${a})`;

/** The depth's colour scale's whole range (m): the ramp's end goes no deeper. */
export const DEPTH_DOMAIN: [number, number] = [0, 6000];
/** Histogram bins: depth (m) on a log scale from 0.1 to 12,000 m. */
export const HBINS = 120;
/** The depth histograms' cells a tile side (depth.worker.ts composite): the view's ground counts
 * a cell whose centre it holds, or the cell holding its centre (depth.ts viewTiles). */
export const HCELLS = 8;
export const hbin = (d: number) => Math.max(0, Math.min(HBINS - 1, Math.floor(((Math.log10(Math.max(0.1, d)) + 1) / 5.08) * HBINS)));
export const hdepth = (b: number) => 10 ** ((b / HBINS) * 5.08 - 1);

/** The workers' histogram (HBINS log bins) as linear bins over DEPTH_DOMAIN, 2 m each, for the
 * colour scale's histogram and equalization: each log bin's count spread evenly over its depths
 * (deeper than the domain: its last bin). */
export function depthBins(hist: number[]): Float64Array {
  const [lo, hi] = DEPTH_DOMAIN, n = (hi - lo) / 2, w = (hi - lo) / n;
  const out = new Float64Array(n);
  for (let b = 0; b < hist.length; b++) {
    if (!hist[b]) continue;
    const a = Math.max(lo, hdepth(b)), e = Math.max(a, hdepth(b + 1));
    if (a >= hi) {
      out[n - 1] += hist[b];
      continue;
    }
    const i0 = Math.floor((a - lo) / w), i1 = Math.min(n - 1, Math.floor((Math.min(e, hi) - lo) / w));
    if (i0 === i1 || e <= a) {
      out[i0] += hist[b];
      continue;
    }
    // Over the bins it covers, by how much of it each holds (what lies past the domain: the last).
    for (let i = i0; i <= i1; i++) out[i] += (hist[b] * (Math.min(e, lo + (i + 1) * w) - Math.max(a, lo + i * w))) / (e - a);
    if (e > hi) out[n - 1] += (hist[b] * (e - hi)) / (e - a);
  }
  return out;
}
