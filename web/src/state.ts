import { GROUPS, NGROUP } from './config';
import { DEFAULT_WEIGHTS, MODES, NCOMP, PRESETS, modeDef, type Mode } from './scenic';

export type { Mode };

export type HillshadeMethod = 'standard' | 'basic' | 'combined' | 'igor' | 'multidirectional';
export type TintRange = 'region' | 'view' | 'roads' | 'custom';
export type TintVar = 'elev' | 'slope';

/** Overlay layers (designations, stops, context) — all off by default except parks. */
export const OVERLAYS = [
  // key, label, group
  ['parks', 'Parks & protected areas', 'map'],
  ['heritage', 'Heritage sites', 'designations'],
  ['heritageAreas', 'Heritage districts', 'designations'],
  ['special', 'Biospheres · geoparks · dark sky', 'designations'],
  ['indigenous', 'Indigenous lands', 'designations'],
  ['viewpoint', 'Viewpoints', 'stops'],
  ['peak', 'Peaks', 'stops'],
  ['waterfall', 'Waterfalls', 'stops'],
  ['lighthouse', 'Lighthouses', 'stops'],
  ['covered_bridge', 'Covered bridges', 'stops'],
  ['rest', 'Rest areas & picnic sites', 'stops'],
  ['trailhead', 'Trailheads', 'stops'],
] as const;
export type OverlayKey = (typeof OVERLAYS)[number][0];

export interface Terrain {
  /** 3D terrain mesh. */
  on: boolean;
  exaggeration: number;
  hillshade: boolean;
  method: HillshadeMethod;
  /** Light azimuth, degrees clockwise from north. */
  light: number;
  /** Hillshade strength 0..1. */
  shade: number;
  /** Hypsometric terrain tint (colour-relief). */
  tint: boolean;
  /** What the tint colours: elevation, or terrain slope (%). */
  tintVar: TintVar;
  tintOpacity: number;
  /** Tint colour ramp (see terrain.ts TINT_PALETTES; 'roads' follows the road palette). */
  tintPalette: string;
  /** Elevation span of the ramp: whole region, fitted to the view, the road colour range, or custom. */
  tintRange: TintRange;
  tintMin: number;
  tintMax: number;
  /** Band height in metres, 0 = smooth. */
  tintBands: number;
  /** Ramp exponent: < 1 spends more colour on lowlands, > 1 on highlands. */
  tintCurve: number;
  /** Transparency at the low end of the tint ramp (1 = fully transparent at the bottom), per variable. */
  tintFade: { elev: number; slope: number };
  /** Share of the ramp the fade spans, per variable. */
  tintFadeSpan: { elev: number; slope: number };
  contours: boolean;
  sky: boolean;
  /** Camera pivot height follows the terrain under the view centre (MapLibre's default). Off:
   *  the camera never moves with the terrain; the pivot stays at a fixed height. */
  cameraFollow: boolean;
}

export interface AppState {
  mode: Mode;
  palette: string;
  /** Follow the view (true) or keep `range` fixed. */
  auto: boolean;
  range: [number, number];
  /** Auto-fit percentiles of the roads in view (low, high), 0–100. */
  fit: [number, number];
  /** Histogram-equalised colours. */
  equalize: boolean;
  /** Scenic-score weights (see scenic.ts COMPONENTS). */
  weights: number[];
  preset: string;
  groups: boolean[];
  surface: { paved: boolean; unpaved: boolean };
  /** Line weight multiplier. */
  weight: number;
  routeGlow: boolean;
  /** Transparency at the low end of the colour scale (0..1) and the share of the scale it spans. */
  lowFade: number;
  lowSpan: number;
  /** Opacity of all map labels. */
  labelOpacity: number;
  /** Globe projection (flattens to Web Mercator as you zoom in). */
  globe: boolean;
  /** Road widths shrink with distance in tilted views. */
  perspective: boolean;
  /** Blend overlapping translucent roads (junctions and dense areas get brighter). */
  blendOverlaps: boolean;
  layers: { roads: boolean; water: boolean; boundaries: boolean; places: boolean };
  overlays: Record<OverlayKey, boolean>;
  /** Heritage levels shown (1 World Heritage … 5 municipal). */
  heritageLevels: boolean[];
  terrain: Terrain;
  threshold: { on: boolean; dir: 'above' | 'below'; value: number };
  selected: number | null;
  /** elev: camera pivot height (m, exaggerated) when the camera doesn't follow the terrain. */
  view: { zoom: number; lat: number; lng: number; bearing: number; pitch: number; elev: number } | null;
}

export const WEIGHT_MIN = 0.1;
export const WEIGHT_MAX = 1;

export const defaults: AppState = {
  mode: 'elev',
  palette: 'viridis',
  auto: true,
  range: [0, 600],
  fit: [1, 99],
  equalize: false,
  weights: [...DEFAULT_WEIGHTS],
  preset: 'balanced',
  groups: new Array(NGROUP).fill(true),
  surface: { paved: true, unpaved: true },
  weight: 0.5,
  routeGlow: false,
  lowFade: 0.7,
  lowSpan: 0.6,
  labelOpacity: 0.8,
  globe: true,
  perspective: true,
  blendOverlaps: false,
  layers: { roads: true, water: true, boundaries: true, places: true },
  overlays: Object.fromEntries(OVERLAYS.map(([k]) => [k, false])) as Record<OverlayKey, boolean>,
  heritageLevels: [true, true, true, true, true],
  terrain: {
    on: true, exaggeration: 3, hillshade: true, method: 'combined', light: 315, shade: 0.55,
    tint: false, tintVar: 'elev', tintOpacity: 0.45, tintPalette: 'atlas', tintRange: 'region', tintMin: 0, tintMax: 1900, tintBands: 0, tintCurve: 1,
    tintFade: { elev: 0, slope: 1 }, tintFadeSpan: { elev: 0.5, slope: 0.35 },
    contours: false, sky: true, cameraFollow: false,
  },
  threshold: { on: false, dir: 'above', value: 500 },
  selected: null,
  view: null,
};

type Listener = (s: AppState, changed: Set<keyof AppState>) => void;

export class Store {
  s: AppState;
  private ls: Listener[] = [];
  constructor(init: AppState) {
    this.s = init;
  }
  on(fn: Listener) {
    this.ls.push(fn);
  }
  set(patch: Partial<AppState>) {
    const changed = new Set(Object.keys(patch) as (keyof AppState)[]);
    this.s = { ...this.s, ...patch };
    for (const l of this.ls) l(this.s, changed);
  }
  terrain(patch: Partial<Terrain>) {
    this.set({ terrain: { ...this.s.terrain, ...patch } });
  }
  overlay(k: OverlayKey, on: boolean) {
    this.set({ overlays: { ...this.s.overlays, [k]: on } });
  }
  /** Switch colour mode, resetting the range and threshold to the mode's defaults. */
  setMode(mode: Mode) {
    const d = modeDef(mode);
    const thr = this.s.threshold;
    const [lo, hi] = d.domain;
    this.set({
      mode,
      auto: d.auto,
      range: [...d.range] as [number, number],
      threshold: { ...thr, value: thr.value >= lo && thr.value <= hi && mode === this.s.mode ? thr.value : d.thrDefault },
    });
  }
}

export function classMask(s: AppState): number {
  let m = 0;
  GROUPS.forEach((g, i) => {
    if (s.groups[i]) for (const c of g.classes) m |= 1 << c;
  });
  return m;
}

export function surfaceMask(s: AppState): number {
  return (s.surface.paved ? 1 : 0) | (s.surface.unpaved ? 2 : 0);
}

export function groupMask(s: AppState): number {
  return s.groups.reduce((m, on, i) => (on ? m | (1 << i) : m), 0);
}

// ---- URL hash ------------------------------------------------------------------------------
// #map=zoom/lat/lng/bearing/pitch&m=score&p=viridis&r=lo,hi&eq=1&pr=vistas&wt=…&g=11111&sf=pu&w=0.5
//  &l=rwbp&o=<overlay bits>&hl=<levels>&t3=…&t=a500&s=123

const ob = (k: OverlayKey) => OVERLAYS.findIndex((o) => o[0] === k);
const HM: HillshadeMethod[] = ['standard', 'basic', 'combined', 'igor', 'multidirectional'];

export function toHash(s: AppState): string {
  const p = new URLSearchParams();
  if (s.view) {
    const v = s.view;
    const extra = v.bearing || v.pitch || v.elev ? `/${v.bearing.toFixed(1)}/${v.pitch.toFixed(1)}${v.elev ? `/${Math.round(v.elev)}` : ''}` : '';
    p.set('map', `${v.zoom.toFixed(2)}/${v.lat.toFixed(5)}/${v.lng.toFixed(5)}${extra}`);
  }
  if (s.mode !== defaults.mode) p.set('m', s.mode);
  if (s.palette !== defaults.palette) p.set('p', s.palette);
  const d = modeDef(s.mode);
  if (s.auto !== d.auto || (!s.auto && (s.range[0] !== d.range[0] || s.range[1] !== d.range[1])))
    p.set('r', s.auto ? 'auto' : `${+s.range[0].toFixed(2)},${+s.range[1].toFixed(2)}`);
  if (s.fit[0] !== defaults.fit[0] || s.fit[1] !== defaults.fit[1]) p.set('fp', `${s.fit[0]},${s.fit[1]}`);
  if (s.equalize) p.set('eq', '1');
  if (s.preset !== 'balanced') p.set('pr', s.preset);
  if (s.preset === 'custom') p.set('wt', s.weights.map((w) => +w.toFixed(2)).join(','));
  if (s.groups.some((g) => !g)) p.set('g', s.groups.map((g) => (g ? 1 : 0)).join(''));
  if (!(s.surface.paved && s.surface.unpaved)) p.set('sf', `${s.surface.paved ? 'p' : ''}${s.surface.unpaved ? 'u' : ''}`);
  if (s.weight !== defaults.weight) p.set('w', s.weight.toFixed(2));
  if (s.routeGlow) p.set('rg', '1');
  const l = s.layers;
  if (!(l.roads && l.water && l.boundaries && l.places)) p.set('l', `${l.roads ? 'r' : ''}${l.water ? 'w' : ''}${l.boundaries ? 'b' : ''}${l.places ? 'p' : ''}`);
  const bits = OVERLAYS.map(([k]) => (s.overlays[k] ? '1' : '0')).join('');
  if (bits !== OVERLAYS.map(([k]) => (defaults.overlays[k] ? '1' : '0')).join('')) p.set('o', bits);
  if (s.heritageLevels.some((x) => !x)) p.set('hl', s.heritageLevels.map((x) => (x ? 1 : 0)).join(''));
  const t = s.terrain, dt = defaults.terrain;
  const tt = [
    t.on ? 1 : 0, +t.exaggeration.toFixed(2), t.hillshade ? 1 : 0, HM.indexOf(t.method), Math.round(t.light),
    +t.shade.toFixed(2), t.tint ? 1 : 0, t.contours ? 1 : 0, t.sky ? 1 : 0, t.cameraFollow ? 1 : 0,
  ].join(',');
  const td = [dt.on ? 1 : 0, dt.exaggeration, dt.hillshade ? 1 : 0, HM.indexOf(dt.method), dt.light, dt.shade, dt.tint ? 1 : 0, dt.contours ? 1 : 0, dt.sky ? 1 : 0, dt.cameraFollow ? 1 : 0].join(',');
  if (tt !== td) p.set('t3', tt);
  const tn = [t.tintPalette, t.tintRange, Math.round(t.tintMin), Math.round(t.tintMax), t.tintBands, +t.tintCurve.toFixed(2), +t.tintOpacity.toFixed(2), t.tintVar].join(',');
  const tnd = [dt.tintPalette, dt.tintRange, dt.tintMin, dt.tintMax, dt.tintBands, dt.tintCurve, dt.tintOpacity, dt.tintVar].join(',');
  if (tn !== tnd) p.set('tn', tn);
  const tf = [t.tintFade.elev, t.tintFadeSpan.elev, t.tintFade.slope, t.tintFadeSpan.slope].map((v) => +v.toFixed(2)).join(',');
  if (tf !== [dt.tintFade.elev, dt.tintFadeSpan.elev, dt.tintFade.slope, dt.tintFadeSpan.slope].join(',')) p.set('tf', tf);
  if (s.lowFade !== defaults.lowFade || s.lowSpan !== defaults.lowSpan) p.set('lf', `${+s.lowFade.toFixed(2)},${+s.lowSpan.toFixed(2)}`);
  if (s.labelOpacity !== defaults.labelOpacity) p.set('lo', s.labelOpacity.toFixed(2));
  if (!s.globe) p.set('gb', '0');
  if (!s.perspective) p.set('pw', '0');
  if (s.blendOverlaps) p.set('bo', '1');
  if (s.threshold.on) p.set('t', `${s.threshold.dir === 'above' ? 'a' : 'b'}${+s.threshold.value.toFixed(3)}`);
  if (s.selected !== null) p.set('s', String(s.selected));
  return '#' + p.toString().replace(/%2F/g, '/').replace(/%2C/g, ',');
}

export function fromHash(hash: string): AppState {
  const s: AppState = structuredClone(defaults);
  const p = new URLSearchParams(hash.replace(/^#/, ''));
  const map = p.get('map')?.split('/').map(Number);
  if (map && map.length >= 3 && map.every(Number.isFinite))
    s.view = { zoom: map[0], lat: map[1], lng: map[2], bearing: map[3] ?? 0, pitch: map[4] ?? 0, elev: map[5] ?? 0 };
  const m = p.get('m');
  if (m && MODES.some((d) => d.key === m)) {
    const d = modeDef(m as Mode);
    s.mode = d.key;
    s.auto = d.auto;
    s.range = [...d.range] as [number, number];
    s.threshold.value = d.thrDefault;
  }
  if (p.get('p')) s.palette = p.get('p')!;
  const rs = p.get('r');
  if (rs === 'auto') s.auto = true;
  else {
    const r = rs?.split(',').map(Number);
    if (r && r.length === 2 && r.every(Number.isFinite) && r[1] > r[0]) {
      s.auto = false;
      s.range = [r[0], r[1]];
    }
  }
  const fp = p.get('fp')?.split(',').map(Number);
  if (fp && fp.length === 2 && fp.every(Number.isFinite) && fp[0] >= 0 && fp[1] <= 100 && fp[1] > fp[0]) s.fit = [fp[0], fp[1]];
  s.equalize = p.get('eq') === '1';
  const pr = p.get('pr');
  if (pr && PRESETS[pr]) {
    s.preset = pr;
    s.weights = [...PRESETS[pr].w];
  } else if (pr === 'custom') {
    const wt = p.get('wt')?.split(',').map(Number);
    if (wt && wt.length === NCOMP && wt.every(Number.isFinite)) {
      s.preset = 'custom';
      s.weights = wt;
    }
  }
  const g = p.get('g');
  if (g && g.length === NGROUP) s.groups = [...g].map((c) => c === '1');
  const sf = p.get('sf');
  if (sf !== null) s.surface = { paved: sf.includes('p'), unpaved: sf.includes('u') };
  const w = Number(p.get('w'));
  if (p.get('w') && w >= WEIGHT_MIN && w <= WEIGHT_MAX) s.weight = w;
  s.routeGlow = p.get('rg') === '1';
  const l = p.get('l');
  if (l !== null) s.layers = { roads: l.includes('r'), water: l.includes('w'), boundaries: l.includes('b'), places: l.includes('p') };
  const o = p.get('o');
  if (o && o.length === OVERLAYS.length) OVERLAYS.forEach(([k], i) => (s.overlays[k] = o[i] === '1'));
  const hl = p.get('hl');
  if (hl && hl.length === 5) s.heritageLevels = [...hl].map((c) => c === '1');
  const t3 = p.get('t3')?.split(',').map(Number);
  if (t3 && t3.length >= 9 && t3.every(Number.isFinite)) {
    s.terrain = {
      ...s.terrain,
      on: t3[0] === 1, exaggeration: Math.min(6, Math.max(1, t3[1])), hillshade: t3[2] === 1, method: HM[t3[3]] ?? 'combined',
      light: t3[4], shade: Math.min(1, Math.max(0, t3[5])), tint: t3[6] === 1, contours: t3[7] === 1, sky: t3[8] === 1,
      cameraFollow: t3[9] === 1,
    };
  }
  const tn = p.get('tn')?.split(',');
  if (tn && tn.length >= 7) {
    const n = tn.slice(2, 7).map(Number);
    if (n.every(Number.isFinite) && n[1] > n[0]) {
      s.terrain = {
        ...s.terrain, tintPalette: tn[0], tintRange: (['region', 'view', 'roads', 'custom'] as const).find((r) => r === tn[1]) ?? 'region',
        tintMin: n[0], tintMax: n[1], tintBands: Math.max(0, n[2]), tintCurve: Math.min(3, Math.max(0.3, n[3])), tintOpacity: Math.min(1, Math.max(0, n[4])),
        tintVar: tn[7] === 'slope' ? 'slope' : 'elev',
      };
    }
  }
  const tf = p.get('tf')?.split(',').map(Number);
  if (tf && tf.length === 4 && tf.every(Number.isFinite)) {
    const c = (v: number, lo: number) => Math.min(1, Math.max(lo, v));
    s.terrain = { ...s.terrain, tintFade: { elev: c(tf[0], 0), slope: c(tf[2], 0) }, tintFadeSpan: { elev: c(tf[1], 0.05), slope: c(tf[3], 0.05) } };
  }
  const lf = p.get('lf')?.split(',').map(Number);
  if (lf && lf.length === 2 && lf.every(Number.isFinite)) {
    s.lowFade = Math.min(1, Math.max(0, lf[0]));
    s.lowSpan = Math.min(1, Math.max(0.1, lf[1]));
  }
  s.globe = p.get('gb') !== '0';
  s.perspective = p.get('pw') !== '0';
  s.blendOverlaps = p.get('bo') === '1';
  const lo = Number(p.get('lo'));
  if (p.get('lo') && lo >= 0 && lo <= 1) s.labelOpacity = lo;
  const t = p.get('t');
  if (t && /^[ab]-?[\d.]+$/.test(t)) s.threshold = { on: true, dir: t[0] === 'a' ? 'above' : 'below', value: Number(t.slice(1)) };
  const sel = Number(p.get('s'));
  if (p.get('s') && Number.isInteger(sel)) s.selected = sel;
  return s;
}

export { ob as overlayIndex };

/**
 * State saved in localStorage, merged onto the defaults key by key (unknown or mistyped
 * values are dropped, so older saves keep working after new settings are added).
 */
export function fromSaved(o: unknown): AppState {
  const s: AppState = structuredClone(defaults);
  if (!o || typeof o !== 'object') return s;
  const src = o as Record<string, unknown>;
  const merge = (dst: Record<string, unknown>, from: Record<string, unknown>) => {
    for (const k of Object.keys(dst)) {
      if (!(k in from)) continue;
      const d = dst[k], v = from[k];
      if (Array.isArray(d)) {
        if (Array.isArray(v) && v.length === d.length && v.every((x, i) => typeof x === typeof d[i])) dst[k] = v;
      } else if (d !== null && typeof d === 'object') {
        if (v && typeof v === 'object') merge(d as Record<string, unknown>, v as Record<string, unknown>);
      } else if (typeof v === typeof d && (typeof v !== 'number' || Number.isFinite(v))) {
        dst[k] = v;
      }
    }
  };
  const { view, selected: _sel, ...rest } = src;
  merge(s as unknown as Record<string, unknown>, rest);
  if (!MODES.some((m) => m.key === s.mode)) s.mode = defaults.mode;
  const v = view as AppState['view'];
  if (v && [v.zoom, v.lat, v.lng].every(Number.isFinite)) {
    s.view = { zoom: v.zoom, lat: v.lat, lng: v.lng, bearing: Number(v.bearing) || 0, pitch: Number(v.pitch) || 0, elev: Number(v.elev) || 0 };
  }
  return s;
}
