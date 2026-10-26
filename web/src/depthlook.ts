// The depth layers' look (depth.ts; Layers → Depth, ui/depth.ts; `dp=` in links): their settings,
// defaults and colour ramps, apart from the layers so the app's state can name them.
import { PALETTE_ITEMS, baseKey, isRev, paletteFn } from './palettes';

type RGB = [number, number, number];

/** The depth layers' settings (Layers → Depth; `dp=` in links). */
export interface DepthState {
  /** Depth colour and floor shading. */
  on: boolean;
  palette: string;
  scale: 'linear' | 'log' | 'fit';
  /** The depth at the ramp's end (m). */
  max: number;
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

export const DEPTH_DEFAULTS: DepthState = {
  on: true, palette: 'nautical', scale: 'log', max: 300, opacity: 0.85, shade: 0.45, light: 315, dry: true, dryColour: '#8fae7d',
  contours: false, interval: 'zoom', fixed: 10, index: 5, labels: true, colour: '#9ec3dc', weight: 1, lineOpacity: 0.55,
  floor: false,
};

/** Depth ramps made for the dark map: shallow light, deep sinking into the water's own colour. */
const RAMPS: Record<string, { label: string; stops: [number, string][] }> = {
  nautical: { label: 'Nautical blues', stops: [[0, '#cfeef5'], [0.12, '#8fd0e6'], [0.3, '#4fa3d1'], [0.5, '#2f73b0'], [0.72, '#1e4b85'], [0.88, '#152f5a'], [1, '#0e1d36']] },
  quiet: { label: 'Quiet blues', stops: [[0, '#4a6b87'], [0.35, '#304d66'], [0.7, '#1c3043'], [1, '#101d2b']] },
};

/** The depth's colour ramps: its own, then the app's. */
export const DEPTH_PALETTES: { key: string; label: string; group: string }[] = [
  ...Object.entries(RAMPS).map(([key, r]) => ({ key, label: r.label, group: 'Depth' })),
  ...PALETTE_ITEMS,
];

/** Position t (0 shallow … 1 deep) on ramp `key` (reversed with its "_r"). */
export function depthColour(key: string, t: number): RGB {
  t = Math.max(0, Math.min(1, isRev(key) ? 1 - t : t));
  const r = RAMPS[baseKey(key)];
  if (!r) return paletteFn(baseKey(key))(t);
  const cs = r.stops.map(([s, c]) => [s, parseInt(c.slice(1, 3), 16) / 255, parseInt(c.slice(3, 5), 16) / 255, parseInt(c.slice(5, 7), 16) / 255]);
  for (let i = 1; i < cs.length; i++) {
    if (t <= cs[i][0]) {
      const u = (t - cs[i - 1][0]) / (cs[i][0] - cs[i - 1][0] || 1);
      return [1, 2, 3].map((k) => cs[i - 1][k] + (cs[i][k] - cs[i - 1][k]) * u) as RGB;
    }
  }
  const l = cs[cs.length - 1];
  return [l[1], l[2], l[3]];
}

export const rgba = (c: RGB, a = 1) => `rgba(${c.map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255)).join(',')},${a})`;
export const hexRgb = (s: string): RGB => [1, 3, 5].map((i) => parseInt(s.slice(i, i + 2), 16) / 255) as RGB;

/** Depth (m, positive) → position on the ramp, and back. */
export function scaleFns(kind: 'linear' | 'log', lo: number, hi: number): [(d: number) => number, (t: number) => number] {
  if (kind === 'linear') return [(d) => (d - lo) / (hi - lo || 1), (t) => lo + t * (hi - lo)];
  const L = (d: number) => Math.log1p(Math.max(0, d));
  const a = L(lo), b = L(hi);
  return [(d) => (L(d) - a) / (b - a || 1), (t) => Math.expm1(a + t * (b - a))];
}

/** Histogram bins: depth (m) on a log scale from 0.1 to 12,000 m. */
export const HBINS = 120;
export const hbin = (d: number) => Math.max(0, Math.min(HBINS - 1, Math.floor(((Math.log10(Math.max(0.1, d)) + 1) / 5.08) * HBINS)));
export const hdepth = (b: number) => 10 ** ((b / HBINS) * 5.08 - 1);
