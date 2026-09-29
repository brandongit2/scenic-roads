// Tree cover layer: tree cover (share of ground under trees over 5 m), canopy height and forest leaf
// type, from Terrarium-encoded tiles (dem/trees.py) coloured on the GPU by MapLibre color-relief.
// Two styles for cover and height: a shaded ramp (with a low-end cutoff) or a flat forest mask
// above a threshold; leaf type is categorical.

import type { ExpressionSpecification, Map as MLMap } from 'maplibre-gl';
import { PALETTE_ITEMS, baseKey, isRev, paletteRgb } from './palettes';

export type TreeVar = 'cover' | 'height' | 'leaf';
export type TreeStyle = 'ramp' | 'mask';

export interface TreeVarDef {
  key: TreeVar;
  label: string;
  unit: string;
  /** Ramp span. */
  max: number;
  /** Cutoff / threshold slider range and step. */
  range: [number, number];
  step: number;
  help: string;
}

export const TREE_VARS: TreeVarDef[] = [
  { key: 'cover', label: 'Cover', unit: '%', max: 100, range: [0, 95], step: 1, help: 'Share of the ground under trees taller than 5 m (Meta / WRI canopy height, from 1.2 m imagery; ~25 m pixels).' },
  { key: 'height', label: 'Height', unit: 'm', max: 40, range: [0, 40], step: 1, help: 'Canopy height (95th percentile of the trees in each ~25 m pixel), where cover is at least 5 %.' },
  { key: 'leaf', label: 'Leaf type', unit: '', max: 3, range: [0, 0], step: 1, help: 'Dominant leaf type of forests: Copernicus HRL 2018 (Europe, broadleaf / coniferous), NALCMS 2020 (North America, with mixed forest). None for Hong Kong.' },
];
export const treeVarDef = (k: TreeVar) => TREE_VARS.find((v) => v.key === k) ?? TREE_VARS[0];

export const LEAF_CLASSES: [number, string, string, string][] = [
  [1, 'Broadleaf', '#e0a33e', 'Broadleaf (deciduous in North America; in the Mediterranean some are evergreen)'],
  [2, 'Conifer', '#2e8b57', 'Coniferous / needleleaf'],
  [3, 'Mixed', '#9bbf4a', 'Mixed forest (North America only)'],
];

/** Tree palettes: a green ramp made for the dark map, then every shared ramp ("_r": reversed). */
export const TREE_PALETTES: { key: string; label: string; group: string }[] = [{ key: 'greens', label: 'Forest greens', group: 'Trees' }, ...PALETTE_ITEMS];
const GREENS = ['#15291c', '#1b3d26', '#215231', '#28673b', '#317c44', '#3d914c', '#4fa453', '#67b45b'];

export function treeColour(palette: string, t: number): string {
  if (baseKey(palette) !== 'greens') return paletteRgb(palette, t);
  if (isRev(palette)) t = 1 - t;
  const x = Math.max(0, Math.min(1, t)) * (GREENS.length - 1);
  const i = Math.min(GREENS.length - 2, Math.floor(x));
  const f = x - i;
  const a = hex(GREENS[i]), b = hex(GREENS[i + 1]);
  return `rgb(${a.map((v, k) => Math.round(v + (b[k] - v) * f)).join(',')})`;
}
const hex = (c: string) => [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16));
const CLEAR = 'rgba(0,0,0,0)';

export interface TreeState {
  on: boolean;
  variable: TreeVar;
  style: TreeStyle;
  opacity: number;
  palette: string;
  /** Ramp: values below are hidden (cover %, height m). */
  cutCover: number;
  cutHeight: number;
  /** Mask: shown at or above (cover %, height m). */
  maskCover: number;
  maskHeight: number;
  /** Mask colour. */
  maskColour: string;
}

export const TREE_LAYERS: Record<TreeVar, string> = { cover: 'trees-cover', height: 'trees-height', leaf: 'trees-leaf' };

/** color-relief colour ramp (piecewise linear between stops; steps are stop pairs a hair apart). */
export function treeRamp(t: TreeState): ExpressionSpecification {
  const d = treeVarDef(t.variable);
  const stops: (number | string)[] = [];
  const add = (v: number, c: string) => stops.push(v, c);
  const eps = 0.01;
  if (t.variable === 'leaf') {
    add(0, CLEAR);
    add(0.5 - eps, CLEAR);
    for (const [v, , c] of LEAF_CLASSES) {
      add(v - 0.5, c);
      add(v + 0.5 - eps, c);
    }
  } else if (t.style === 'mask') {
    const th = Math.max(eps * 2, t.variable === 'cover' ? t.maskCover : t.maskHeight);
    add(0, CLEAR);
    add(th - eps, CLEAR);
    add(th, t.maskColour);
    add(Math.max(d.max, th + 1), t.maskColour);
  } else {
    const cut = t.variable === 'cover' ? t.cutCover : t.cutHeight;
    add(0, CLEAR);
    const lo = Math.min(d.max - 1, Math.max(eps * 2, cut));
    add(lo - eps, CLEAR);
    const n = 8;
    for (let i = 0; i <= n; i++) add(lo + ((d.max - lo) * i) / n, treeColour(t.palette, i / n));
  }
  return ['interpolate', ['linear'], ['elevation'], ...stops] as unknown as ExpressionSpecification;
}

export function applyTrees(map: MLMap, t: TreeState) {
  for (const v of Object.keys(TREE_LAYERS) as TreeVar[]) {
    const id = TREE_LAYERS[v];
    if (!map.getLayer(id)) continue;
    const on = t.on && t.variable === v;
    map.setLayoutProperty(id, 'visibility', on ? 'visible' : 'none');
    if (!on) continue;
    map.setPaintProperty(id, 'color-relief-color', treeRamp(t));
    map.setPaintProperty(id, 'color-relief-opacity', t.opacity);
  }
}
