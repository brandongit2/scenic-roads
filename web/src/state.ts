import { GROUPS, NGROUP, NRAIL } from './config';
import { RAIL_DEFAULT_WEIGHTS, RAIL_METRICS, RNCOMP, railMetricDef, type RailColour, type RailMetric } from './rail';
import { MAP_SCHEMES } from './mapschemes';
import { FERRY_METRICS, NFERRY, ferryMetricDef, type FerryColour, type FerryMetric } from './ferry';
import { baseKey } from './palettes';
import { STOP_FILTERS, filtersFromHash, filtersToHash, type StopFilter } from './stopfilters';
import { TREE_PALETTES, type TreeState, type TreeStyle, type TreeVar } from './trees';
import { BUILTIN, DEFAULT_PRESET, DEFAULT_WEIGHTS, RAIL_DEFAULT_PRESET, presets, railPresets, sameWeights } from './presets';
import { MODES, migrateWeights, modeDef, type Mode } from './scenic';

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

/** Kinds of map labels, each toggled under "Place labels" (layer ids: basemap.ts LABEL_LAYERS). */
export const LABEL_KINDS = [
  ['city', 'Cities', 'Cities (and provinces’ and states’ largest towns)'],
  ['town', 'Towns', 'Towns'],
  ['village', 'Villages', 'Villages'],
  ['minor', 'Hamlets & neighbourhoods', 'Hamlets, suburbs, quarters and neighbourhoods (zoom 11.5 and closer)'],
  ['state', 'Provinces & states', 'Province, state and region names, zoomed out'],
  ['water', 'Water', 'Seas, bays, lakes and rivers'],
  ['parks', 'Parks & protected areas', 'Names of the parks and protected areas shown (Stops & sights)'],
  ['heritage', 'Heritage sites', 'Names of the heritage sites shown'],
  ['areas', 'Biospheres, geoparks & Indigenous lands', 'Names of those areas, where shown'],
  ['stops', 'Stops & sights', 'Names of the viewpoints, peaks, waterfalls, lighthouses and other stops shown'],
  ['ferries', 'Ferries & terminals', 'Ferry line and terminal names'],
] as const;
export type LabelKind = (typeof LABEL_KINDS)[number][0];

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
}

export type ThresholdDir = 'above' | 'below' | 'low';
export interface Stretch {
  kind: 'climb' | 'drive';
  /** Start and end (lng, lat). */
  a: [number, number];
  b: [number, number];
  label: string;
}
const THR_CODE: Record<ThresholdDir, string> = { above: 'a', below: 'b', low: 'l' };
const THR_OF: Record<string, ThresholdDir> = { a: 'above', b: 'below', l: 'low' };

/** fit lo, fit hi, equalise, fade span, highlight on, direction, value (hash fields). */
function parseScaleTail(v: string[], d: ScaleFields): Pick<ScaleFields, 'fit' | 'equalize' | 'lowSpan' | 'threshold'> {
  const n = (x: string, dv: number) => (x !== '' && Number.isFinite(Number(x)) ? Number(x) : dv);
  const lo = Math.min(99.5, Math.max(0, n(v[0], d.fit[0]))), hi = Math.min(100, Math.max(lo + 0.5, n(v[1], d.fit[1])));
  return {
    fit: [lo, hi],
    equalize: v[2] === '1',
    lowSpan: Math.min(1, Math.max(0.1, n(v[3], d.lowSpan))),
    threshold: { on: v[4] === '1', dir: THR_OF[v[5]] ?? d.threshold.dir, value: n(v[6], d.threshold.value) },
  };
}

/** The display types; each keeps its own colour settings (the scenic metrics share one set). */
export type ModeGroup = 'elev' | 'grade' | 'relief' | 'scenic' | 'map';
const MODE_GROUPS: ModeGroup[] = ['elev', 'grade', 'relief', 'scenic', 'map'];
export const modeGroup = (m: Mode): ModeGroup => (m === 'elev' || m === 'grade' || m === 'relief' || m === 'map' ? m : 'scenic');

/** A metric colour scale's settings (rail and ferries), as kept per metric. */
export interface MetricLook {
  palette: string;
  fit: [number, number];
  equalize: boolean;
  lowFade: number;
  lowSpan: number;
  thrOn: boolean;
  thrDir: ThresholdDir;
  auto: boolean;
  range: [number, number];
  thrValue: number;
}

/** The colour-scale fields shared by roads, rail and ferries (see ui/scale.ts). */
interface ScaleFields {
  palette: string;
  auto: boolean;
  range: [number, number];
  fit: [number, number];
  equalize: boolean;
  lowFade: number;
  lowSpan: number;
  threshold: { on: boolean; dir: ThresholdDir; value: number };
}

export function lookOfScale(x: ScaleFields): MetricLook {
  return {
    palette: x.palette, fit: [...x.fit], equalize: x.equalize, lowFade: x.lowFade, lowSpan: x.lowSpan,
    thrOn: x.threshold.on, thrDir: x.threshold.dir, auto: x.auto, range: [...x.range], thrValue: x.threshold.value,
  };
}

export function scaleOfLook(l: MetricLook): ScaleFields {
  return {
    palette: l.palette, fit: [...l.fit], equalize: l.equalize, lowFade: l.lowFade, lowSpan: l.lowSpan,
    auto: l.auto, range: [...l.range], threshold: { on: l.thrOn, dir: l.thrDir, value: l.thrValue },
  };
}

/** A metric's look the first time it is picked. */
export function freshLook(range: [number, number], lowFade: number): MetricLook {
  return {
    palette: 'viridis', fit: [2, 98], equalize: false, lowFade, lowSpan: 0.6, thrOn: false, thrDir: 'above',
    auto: true, range: [...range], thrValue: +((range[0] + range[1]) / 2).toPrecision(3),
  };
}

function validLook(v: unknown): MetricLook | null {
  const x = v as Partial<MetricLook> | null;
  const num = (n: unknown) => typeof n === 'number' && Number.isFinite(n);
  const pair = (a: unknown) => Array.isArray(a) && a.length === 2 && a.every(num);
  if (!x || typeof x !== 'object' || typeof x.palette !== 'string' || !pair(x.fit) || typeof x.equalize !== 'boolean' || !num(x.lowFade)
      || !num(x.lowSpan) || typeof x.thrOn !== 'boolean' || !['above', 'below', 'low'].includes(x.thrDir as string) || typeof x.auto !== 'boolean'
      || !pair(x.range) || !num(x.thrValue)) return null;
  return x as MetricLook;
}

/** Passenger rail layer and its styling (top-left panel, "Rail" section). */
export interface RailState extends ScaleFields {
  on: boolean;
  /** Service groups shown: trams, metro, commuter, intercity, heritage & mountain. */
  groups: boolean[];
  colour: RailColour;
  metric: RailMetric;
  /** The other metrics' colour settings (the active one's are the scale fields). */
  looks: Partial<Record<RailMetric, MetricLook>>;
  weights: number[];
  /** Ride-factor preset name ('' = custom). */
  preset: string;
  /** Line weight multiplier. */
  weight: number;
  /** Railway symbol (thin line with cross-ties) instead of a solid line. */
  ties: boolean;
  casing: boolean;
  /** Colour for 'single'. */
  single: string;
  /** Service-frequency filter (Layers): trains a day each way, 0 = no limit; keep unknown lines. */
  freqOn: boolean;
  freqMin: number;
  freqMax: number;
  freqUnknown: boolean;
}
const RAIL_COLOURS: RailColour[] = ['line', 'group', 'metric', 'single'];
/** Passenger ferries and their styling (top-left panel, "Ferries" section). */
export interface FerryState extends ScaleFields {
  on: boolean;
  /** Service groups shown: urban & commuter, short crossings, long-distance & overnight, cable & chain. */
  groups: boolean[];
  colour: FerryColour;
  /** What the metric colouring shows (sailings a day, season length). */
  metric: FerryMetric;
  looks: Partial<Record<FerryMetric, MetricLook>>;
  weight: number;
  /** Line opacity 0..1. */
  opacity: number;
  dashed: boolean;
  single: string;
  /** Sailings-a-day filter (Layers): 0 = no limit; keep lines without a timetable. */
  freqOn: boolean;
  freqMin: number;
  freqMax: number;
  freqUnknown: boolean;
}
const FERRY_COLOURS: FerryColour[] = ['service', 'freq', 'season', 'operator', 'single'];
/** A display type's colour settings (free of units). */
export interface Look {
  palette: string;
  fit: [number, number];
  equalize: boolean;
  lowFade: number;
  lowSpan: number;
  thrOn: boolean;
  thrDir: ThresholdDir;
}
/** Settings in a metric's own units, kept per mode. */
export interface Scale {
  auto: boolean;
  range: [number, number];
  thrValue: number;
}

export interface AppState {
  mode: Mode;
  /** Saved settings of the other display types and modes (the active ones are the fields below). */
  looks: Partial<Record<ModeGroup, Look>>;
  scales: Partial<Record<Mode, Scale>>;
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
  /** Per group: unnamed roads (no name, no route number) shown. */
  unnamed: boolean[];
  /** Whole-road length filter [min, max], km; 0 = no limit. */
  roadLen: [number, number];
  /** The road-length filter applies (off: the limits are kept but ignored). */
  roadLenOn: boolean;
  /** Street-map colouring (mapschemes.ts), for the "Map" display type. */
  mapScheme: string;
  rail: RailState;
  ferry: FerryState;
  /** Tree cover layer (Layers → Trees). */
  trees: TreeState;
  surface: { paved: boolean; unpaved: boolean };
  /** Toll-free and toll roads shown (OSM toll=yes). */
  toll: { free: boolean; toll: boolean };
  /** Line weight multiplier. */
  weight: number;
  routeGlow: boolean;
  /** Transparency at the low end of the colour scale (0..1) and the share of the scale it spans. */
  lowFade: number;
  lowSpan: number;
  /** Opacity of all map labels. */
  labelOpacity: number;
  /** Opacity of the Stops & sights layers (dots, areas and, with labelOpacity, their labels). */
  poiOpacity: number;
  /** How strongly stops & sights and heritage dots are sized and faded by prominence: 0 all alike,
   * 1 the least prominent tiny and faint. */
  poiEmphasis: number;
  /** Landmark prominence: a scale over each dot's score (0–1: fame and rarity mixed by `balance`,
   * 0 fame only … 1 rarity only), like the road colour scales: range (auto-fitted to percentiles
   * of the landmarks in view, locked or full), equalisation, low-end fade and highlight. The
   * palette only draws the legend. */
  landmarks: ScaleFields & { balance: number };
  /** Globe projection (flattens to Web Mercator as you zoom in). */
  globe: boolean;
  /** With 3D terrain: hide roads behind it rather than drawing them faint. */
  occlude: boolean;
  layers: { roads: boolean; water: boolean; boundaries: boolean; places: boolean };
  /** Boundary levels shown (with Boundaries on): countries, provinces & states, counties & regions. */
  boundaryLevels: [boolean, boolean, boolean];
  /** Label kinds shown (with Place labels on). */
  labelKinds: Record<LabelKind, boolean>;
  overlays: Record<OverlayKey, boolean>;
  /** Kinds of heritage designation hidden (basemap.ts HERITAGE_TIERS keys). */
  heritageOff: string[];
  /** Stops & sights filters by key (stopfilters.ts), and per overlay whether features without the
   * filtered value stay (default yes). */
  stopFilters: Record<string, StopFilter>;
  stopUnknown: Partial<Record<OverlayKey, boolean>>;
  terrain: Terrain;
  /** 'low': above the colour scale's low end (its left handle, auto-fitted or not), whatever `value`. */
  threshold: { on: boolean; dir: ThresholdDir; value: number };
  selected: number | null;
  /** Highlighted stretch of the selected road: a climb or scenic drive picked from a list. */
  stretch: Stretch | null;
  /** elev: camera pivot height (m, exaggerated) when the camera doesn't follow the terrain. */
  view: { zoom: number; lat: number; lng: number; bearing: number; pitch: number; elev: number } | null;
}

export const WEIGHT_MIN = 0.1;
export const WEIGHT_MAX = 1;

export const defaults: AppState = {
  mode: 'elev',
  looks: {},
  scales: {},
  palette: 'viridis',
  auto: true,
  range: [0, 600],
  fit: [1, 99],
  equalize: false,
  weights: [...DEFAULT_WEIGHTS],
  preset: DEFAULT_PRESET,
  groups: new Array(NGROUP).fill(true),
  unnamed: new Array(NGROUP).fill(true),
  roadLen: [0, 0],
  roadLenOn: true,
  mapScheme: 'carto',
  rail: {
    on: true, groups: new Array(NRAIL).fill(true), colour: 'line', metric: 'rscore', looks: {},
    ...scaleOfLook(freshLook([0, 100], 0.4)),
    weights: [...RAIL_DEFAULT_WEIGHTS], preset: RAIL_DEFAULT_PRESET, weight: 1, ties: true, casing: true, single: '#e8ecf2',
    freqOn: false, freqMin: 0, freqMax: 0, freqUnknown: true,
  },
  trees: {
    on: false, variable: 'cover', style: 'ramp', opacity: 0.55, palette: 'greens',
    cutCover: 20, cutHeight: 5, maskCover: 50, maskHeight: 10, maskColour: '#3f8f4f',
  },
  ferry: { on: true, groups: new Array(NFERRY).fill(true), colour: 'service', metric: 'freq', looks: {},
    ...scaleOfLook({ ...freshLook(FERRY_METRICS[0].range, 0), fit: [0, 100] }),
    weight: 1, opacity: 0.9, dashed: true, single: '#8fc8ff',
    freqOn: false, freqMin: 0, freqMax: 0, freqUnknown: true },
  surface: { paved: true, unpaved: true },
  toll: { free: true, toll: true },
  weight: 0.5,
  routeGlow: false,
  lowFade: 0.7,
  lowSpan: 0.6,
  labelOpacity: 0.8,
  poiOpacity: 1,
  poiEmphasis: 0.85,
  landmarks: { palette: 'oslo', auto: true, range: [0, 1], fit: [50, 99.9], equalize: false, lowFade: 0.8, lowSpan: 0.6, threshold: { on: false, dir: 'above', value: 0.5 }, balance: 0.5 },
  globe: true,
  occlude: false,
  layers: { roads: true, water: true, boundaries: true, places: true },
  boundaryLevels: [true, true, true],
  labelKinds: Object.fromEntries(LABEL_KINDS.map(([k]) => [k, true])) as Record<LabelKind, boolean>,
  overlays: Object.fromEntries(OVERLAYS.map(([k]) => [k, false])) as Record<OverlayKey, boolean>,
  heritageOff: [],
  stopFilters: {},
  stopUnknown: {},
  terrain: {
    on: true, exaggeration: 3, hillshade: true, method: 'combined', light: 315, shade: 0.55,
    tint: false, tintVar: 'elev', tintOpacity: 0.45, tintPalette: 'atlas', tintRange: 'region', tintMin: 0, tintMax: 1900, tintBands: 0, tintCurve: 1,
    tintFade: { elev: 0, slope: 1 }, tintFadeSpan: { elev: 0.5, slope: 0.35 },
    contours: false, sky: true,
  },
  threshold: { on: false, dir: 'above', value: 500 },
  selected: null,
  stretch: null,
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
  /**
   * Switch colour mode. The current display type's settings are put aside and the new one's
   * restored (defaults the first time), so each of Elevation, Grade, Relief and Scenic keeps its
   * own palette, fades, fit, equalisation and highlight; ranges and threshold values, which are
   * in the metric's units, are kept per mode.
   */
  setMode(mode: Mode) {
    const s = this.s;
    if (mode === s.mode) return;
    const looks = { ...s.looks, [modeGroup(s.mode)]: lookOf(s) };
    const scales = { ...s.scales, [s.mode]: { auto: s.auto, range: [...s.range], thrValue: s.threshold.value } as Scale };
    const d = modeDef(mode);
    const lk = looks[modeGroup(mode)] ?? lookOf(defaults);
    const sc = scales[mode] ?? { auto: d.auto, range: [...d.range] as [number, number], thrValue: d.thrDefault };
    this.set({
      mode, looks, scales,
      palette: lk.palette, fit: [...lk.fit], equalize: lk.equalize, lowFade: lk.lowFade, lowSpan: lk.lowSpan,
      auto: sc.auto, range: [...sc.range], threshold: { on: lk.thrOn, dir: lk.thrDir, value: sc.thrValue },
    });
  }
}

function lookOf(s: AppState): Look {
  return {
    palette: s.palette, fit: [...s.fit], equalize: s.equalize, lowFade: s.lowFade, lowSpan: s.lowSpan,
    thrOn: s.threshold.on, thrDir: s.threshold.dir,
  };
}

export function classMask(s: AppState): number {
  let m = 0;
  GROUPS.forEach((g, i) => {
    if (s.groups[i]) for (const c of g.classes) m |= 1 << c;
  });
  return m;
}

/** Heritage kinds of the old five levels (links and saved settings from before the kinds). */
const LEVEL_TIERS = [['w.c', 'w.n'], ['n.top', 'n.hist', 'n.fed'], ['n.second', 'n.mon', 'n.land'], ['n.lower', 'p.des', 'p.reg', 'p.area'],
  ['m.des', 'm.reg', 'm.area', 'm.agr']];
function offFromLevels(levels: boolean[]): string[] {
  return levels.flatMap((on, i) => (on ? [] : LEVEL_TIERS[i]));
}

export function surfaceMask(s: AppState): number {
  return (s.surface.paved ? 1 : 0) | (s.surface.unpaved ? 2 : 0);
}

/** Bit 0 toll-free roads shown, bit 1 toll roads. */
export function tollMask(s: AppState): number {
  return (s.toll.free ? 1 : 0) | (s.toll.toll ? 2 : 0);
}

/** The road-length filter in effect, km (0 = no limit): nothing while it is switched off. */
export function roadLenKm(s: AppState): [number, number] {
  return s.roadLenOn ? s.roadLen : [0, 0];
}

/** The road-length filter in metres: [min, max], max = Infinity when unbounded. */
export function roadLenM(s: AppState): [number, number] {
  const [lo, hi] = roadLenKm(s);
  return [lo * 1000, hi > 0 ? hi * 1000 : Infinity];
}

/** Whether labels of a kind are shown: Place labels on, and that kind. */
export function labelShown(s: AppState, kind: LabelKind): boolean {
  return s.layers.places && s.labelKinds[kind] !== false;
}

/** Rail service groups shown (bits). */
export function railMask(s: AppState): number {
  return s.rail.groups.reduce((m, on, i) => (on ? m | (1 << i) : m), 0);
}

/** Groups (bits) whose unnamed roads are hidden. */
export function unnamedHideGroups(s: AppState): number {
  return s.unnamed.reduce((m, on, i) => (on ? m : m | (1 << i)), 0);
}

/** Classes (bits) whose unnamed roads are hidden. */
export function unnamedHideClasses(s: AppState): number {
  let m = 0;
  GROUPS.forEach((g, i) => {
    if (!s.unnamed[i]) for (const c of g.classes) m |= 1 << c;
  });
  return m;
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
  // Presets are per browser, so a link carries the weights themselves whenever they aren't the default.
  if (s.preset !== DEFAULT_PRESET) p.set('pr', s.preset || 'custom');
  if (s.preset !== DEFAULT_PRESET || !sameWeights(s.weights, DEFAULT_WEIGHTS)) p.set('wt', s.weights.map((w) => +w.toFixed(2)).join(','));
  if (s.groups.some((g) => !g)) p.set('g', s.groups.map((g) => (g ? 1 : 0)).join(''));
  if (s.unnamed.some((g) => !g)) p.set('un', s.unnamed.map((g) => (g ? 1 : 0)).join(''));
  if (s.roadLen[0] || s.roadLen[1]) p.set('rl', s.roadLen.map((v) => (v ? +v.toFixed(3) : '')).join(','));
  if (!s.roadLenOn) p.set('rlo', '0');
  if (s.mapScheme !== defaults.mapScheme) p.set('ms', s.mapScheme);
  const r = s.rail, dr = defaults.rail;
  const rs = [
    r.on ? 1 : 0, r.groups.map((g) => (g ? 1 : 0)).join(''), r.colour, r.metric, r.palette, r.auto ? 1 : 0,
    +r.range[0].toFixed(2), +r.range[1].toFixed(2), +r.weight.toFixed(2), r.ties ? 1 : 0, r.casing ? 1 : 0, r.single.replace('#', ''), +r.lowFade.toFixed(2),
    r.freqOn ? 1 : 0, +r.freqMin.toFixed(2), +r.freqMax.toFixed(2), r.freqUnknown ? 1 : 0,
    r.fit[0], r.fit[1], r.equalize ? 1 : 0, +r.lowSpan.toFixed(2), r.threshold.on ? 1 : 0, THR_CODE[r.threshold.dir], +r.threshold.value.toFixed(3),
  ].join(',');
  const rsd = [
    dr.on ? 1 : 0, dr.groups.map((g) => (g ? 1 : 0)).join(''), dr.colour, dr.metric, dr.palette, dr.auto ? 1 : 0,
    dr.range[0], dr.range[1], dr.weight, dr.ties ? 1 : 0, dr.casing ? 1 : 0, dr.single.replace('#', ''), dr.lowFade,
    dr.freqOn ? 1 : 0, dr.freqMin, dr.freqMax, dr.freqUnknown ? 1 : 0,
    dr.fit[0], dr.fit[1], dr.equalize ? 1 : 0, dr.lowSpan, dr.threshold.on ? 1 : 0, THR_CODE[dr.threshold.dir], dr.threshold.value,
  ].join(',');
  if (rs !== rsd) p.set('rs', rs);
  if (r.weights.some((w, i) => w !== dr.weights[i])) p.set('rw', r.weights.map((w) => +w.toFixed(2)).join(','));
  if (r.preset !== dr.preset) p.set('rp', r.preset || 'custom');
  const fy = (f: FerryState) => [f.on ? 1 : 0, f.groups.map((g) => (g ? 1 : 0)).join(''), f.colour, f.palette, +f.weight.toFixed(2), f.dashed ? 1 : 0, f.single.replace('#', ''), +f.opacity.toFixed(2),
    f.freqOn ? 1 : 0, +f.freqMin.toFixed(2), +f.freqMax.toFixed(2), f.freqUnknown ? 1 : 0,
    f.metric, f.auto ? 1 : 0, +f.range[0].toFixed(3), +f.range[1].toFixed(3), f.fit[0], f.fit[1], f.equalize ? 1 : 0, +f.lowFade.toFixed(2), +f.lowSpan.toFixed(2),
    f.threshold.on ? 1 : 0, THR_CODE[f.threshold.dir], +f.threshold.value.toFixed(3)].join(',');
  if (fy(s.ferry) !== fy(defaults.ferry)) p.set('fy', fy(s.ferry));
  const tc = (t: TreeState) => [t.on ? 1 : 0, t.variable, t.style, +t.opacity.toFixed(2), t.palette, t.cutCover, t.cutHeight, t.maskCover, t.maskHeight, t.maskColour.replace('#', '')].join(',');
  if (tc(s.trees) !== tc(defaults.trees)) p.set('tc', tc(s.trees));
  if (!(s.surface.paved && s.surface.unpaved)) p.set('sf', `${s.surface.paved ? 'p' : ''}${s.surface.unpaved ? 'u' : ''}`);
  if (!(s.toll.free && s.toll.toll)) p.set('tl', `${s.toll.free ? 'f' : ''}${s.toll.toll ? 't' : ''}`);
  if (s.weight !== defaults.weight) p.set('w', s.weight.toFixed(2));
  if (s.routeGlow) p.set('rg', '1');
  const l = s.layers;
  if (!(l.roads && l.water && l.boundaries && l.places)) p.set('l', `${l.roads ? 'r' : ''}${l.water ? 'w' : ''}${l.boundaries ? 'b' : ''}${l.places ? 'p' : ''}`);
  const bits = OVERLAYS.map(([k]) => (s.overlays[k] ? '1' : '0')).join('');
  if (bits !== OVERLAYS.map(([k]) => (defaults.overlays[k] ? '1' : '0')).join('')) p.set('o', bits);
  if (s.heritageOff.length) p.set('ht', s.heritageOff.join(','));
  if (s.boundaryLevels.some((x) => !x)) p.set('bl', s.boundaryLevels.map((x) => (x ? 1 : 0)).join(''));
  if (LABEL_KINDS.some(([k]) => !s.labelKinds[k])) p.set('lk', LABEL_KINDS.map(([k]) => (s.labelKinds[k] ? 1 : 0)).join(''));
  const sq = filtersToHash(s.stopFilters);
  if (sq) p.set('sq', sq);
  const sqx = OVERLAYS.filter(([k]) => s.stopUnknown[k] === false).map(([k]) => k).join(',');
  if (sqx) p.set('sqx', sqx);
  const t = s.terrain, dt = defaults.terrain;
  const tt = [
    t.on ? 1 : 0, +t.exaggeration.toFixed(2), t.hillshade ? 1 : 0, HM.indexOf(t.method), Math.round(t.light),
    +t.shade.toFixed(2), t.tint ? 1 : 0, t.contours ? 1 : 0, t.sky ? 1 : 0,
  ].join(',');
  const td = [dt.on ? 1 : 0, dt.exaggeration, dt.hillshade ? 1 : 0, HM.indexOf(dt.method), dt.light, dt.shade, dt.tint ? 1 : 0, dt.contours ? 1 : 0, dt.sky ? 1 : 0].join(',');
  if (tt !== td) p.set('t3', tt);
  const tn = [t.tintPalette, t.tintRange, Math.round(t.tintMin), Math.round(t.tintMax), t.tintBands, +t.tintCurve.toFixed(2), +t.tintOpacity.toFixed(2), t.tintVar].join(',');
  const tnd = [dt.tintPalette, dt.tintRange, dt.tintMin, dt.tintMax, dt.tintBands, dt.tintCurve, dt.tintOpacity, dt.tintVar].join(',');
  if (tn !== tnd) p.set('tn', tn);
  const tf = [t.tintFade.elev, t.tintFadeSpan.elev, t.tintFade.slope, t.tintFadeSpan.slope].map((v) => +v.toFixed(2)).join(',');
  if (tf !== [dt.tintFade.elev, dt.tintFadeSpan.elev, dt.tintFade.slope, dt.tintFadeSpan.slope].join(',')) p.set('tf', tf);
  if (s.lowFade !== defaults.lowFade || s.lowSpan !== defaults.lowSpan) p.set('lf', `${+s.lowFade.toFixed(2)},${+s.lowSpan.toFixed(2)}`);
  if (s.labelOpacity !== defaults.labelOpacity) p.set('lo', s.labelOpacity.toFixed(2));
  if (s.poiOpacity !== defaults.poiOpacity) p.set('po', s.poiOpacity.toFixed(2));
  if (s.poiEmphasis !== defaults.poiEmphasis) p.set('pe', s.poiEmphasis.toFixed(2));
  const lm = (l: AppState['landmarks']) => [+l.balance.toFixed(2), l.auto ? 1 : 0, +l.range[0].toFixed(3), +l.range[1].toFixed(3), +l.lowFade.toFixed(2),
    l.fit[0], l.fit[1], l.equalize ? 1 : 0, +l.lowSpan.toFixed(2), l.threshold.on ? 1 : 0, THR_CODE[l.threshold.dir], +l.threshold.value.toFixed(3)].join(',');
  if (lm(s.landmarks) !== lm(defaults.landmarks)) p.set('lm', lm(s.landmarks));
  if (!s.globe) p.set('gb', '0');
  if (s.occlude) p.set('oc', '1');
  if (s.threshold.on) p.set('t', `${THR_CODE[s.threshold.dir]}${+s.threshold.value.toFixed(3)}`);
  if (s.selected !== null) p.set('s', String(s.selected));
  if (s.selected !== null && s.stretch) {
    const { kind, a, b, label } = s.stretch;
    p.set('st', `${kind[0]},${[...a, ...b].map((v) => v.toFixed(5)).join(',')},${label}`);
  }
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
  const wt = migrateWeights(p.get('wt')?.split(',').map(Number));
  if (pr !== null) s.preset = pr === 'custom' ? '' : pr;
  if (wt) s.weights = wt;
  else if (pr) s.weights = [...(presets.get(pr)?.w ?? BUILTIN.find((b) => b.id === pr)?.w ?? DEFAULT_WEIGHTS)]; // older links
  const g = p.get('g');
  if (g && g.length === NGROUP) s.groups = [...g].map((c) => c === '1');
  const un = p.get('un');
  if (un && un.length === NGROUP) s.unnamed = [...un].map((c) => c === '1');
  const rl = p.get('rl')?.split(',').map((v) => Math.max(0, Number(v) || 0));
  if (rl && rl.length === 2) s.roadLen = [rl[0], rl[1]];
  s.roadLenOn = p.get('rlo') !== '0';
  const ms = p.get('ms');
  if (ms && MAP_SCHEMES.some((m) => m.key === ms)) s.mapScheme = ms;
  const rsv = p.get('rs')?.split(',');
  if (rsv && rsv.length >= 13) {
    const rs = rsv;
    const num = (v: string, d: number) => (Number.isFinite(Number(v)) && v !== '' ? Number(v) : d);
    const r = s.rail;
    s.rail = {
      ...r,
      on: rs[0] === '1',
      groups: rs[1].length === NRAIL ? [...rs[1]].map((c) => c === '1') : r.groups,
      colour: RAIL_COLOURS.includes(rs[2] as RailColour) ? (rs[2] as RailColour) : r.colour,
      metric: RAIL_METRICS.some((m) => m.key === rs[3]) ? (rs[3] as RailMetric) : r.metric,
      palette: rs[4] || r.palette,
      auto: rs[5] === '1',
      range: [num(rs[6], r.range[0]), num(rs[7], r.range[1])],
      weight: Math.min(3, Math.max(0.25, num(rs[8], r.weight))),
      ties: rs[9] === '1',
      casing: rs[10] === '1',
      single: /^[0-9a-f]{6}$/i.test(rs[11]) ? `#${rs[11]}` : r.single,
      lowFade: Math.min(1, Math.max(0, num(rs[12], r.lowFade))),
      freqOn: rs[13] === '1',
      freqMin: Math.max(0, num(rs[14] ?? '', r.freqMin)),
      freqMax: Math.max(0, num(rs[15] ?? '', r.freqMax)),
      freqUnknown: rs[16] === undefined ? r.freqUnknown : rs[16] === '1',
      ...(rs.length >= 24 ? parseScaleTail(rs.slice(17), r) : {}),
    };
  }
  const fy = p.get('fy')?.split(',');
  if (fy && fy.length >= 7) {
    const f = s.ferry;
    const w = Number(fy[4]);
    const op = Number(fy[7]);
    s.ferry = {
      on: fy[0] === '1',
      groups: fy[1].length === NFERRY ? [...fy[1]].map((c) => c === '1') : f.groups,
      colour: FERRY_COLOURS.includes(fy[2] as FerryColour) ? (fy[2] as FerryColour) : f.colour,
      palette: fy[3] || f.palette,
      weight: Number.isFinite(w) && fy[4] !== '' ? Math.min(3, Math.max(0.25, w)) : f.weight,
      dashed: fy[5] === '1',
      single: /^[0-9a-f]{6}$/i.test(fy[6]) ? `#${fy[6]}` : f.single,
      opacity: fy[7] !== undefined && fy[7] !== '' && Number.isFinite(op) ? Math.min(1, Math.max(0.05, op)) : f.opacity,
      freqOn: fy[8] === '1',
      freqMin: Math.max(0, Number(fy[9]) || 0),
      freqMax: Math.max(0, Number(fy[10]) || 0),
      freqUnknown: fy[11] === undefined ? f.freqUnknown : fy[11] === '1',
      metric: FERRY_METRICS.some((m) => m.key === fy[12]) ? (fy[12] as FerryMetric) : f.metric,
      looks: f.looks,
      auto: fy[13] === undefined ? f.auto : fy[13] === '1',
      range: fy.length >= 16 && fy[14] !== '' && fy[15] !== '' && Number(fy[15]) > Number(fy[14]) ? [Number(fy[14]), Number(fy[15])] : f.range,
      lowFade: fy[19] !== undefined && Number.isFinite(Number(fy[19])) && fy[19] !== '' ? Math.min(1, Math.max(0, Number(fy[19]))) : f.lowFade,
      ...(fy.length >= 24 ? parseScaleTail([fy[16], fy[17], fy[18], fy[20], fy[21], fy[22], fy[23]], f) : {
        fit: f.fit, equalize: f.equalize, lowSpan: f.lowSpan, threshold: f.threshold, palette: fy[3] || f.palette }),
    };
  }
  const tcv = p.get('tc')?.split(',');
  if (tcv && tcv.length >= 10) {
    const t = s.trees;
    const n = (v: string, d: number, lo: number, hi: number) => (v !== '' && Number.isFinite(Number(v)) ? Math.min(hi, Math.max(lo, Number(v))) : d);
    s.trees = {
      on: tcv[0] === '1',
      variable: (['cover', 'height', 'leaf'] as TreeVar[]).includes(tcv[1] as TreeVar) ? (tcv[1] as TreeVar) : t.variable,
      style: (['ramp', 'mask'] as TreeStyle[]).includes(tcv[2] as TreeStyle) ? (tcv[2] as TreeStyle) : t.style,
      opacity: n(tcv[3], t.opacity, 0.05, 1),
      palette: TREE_PALETTES.some((pp) => pp.key === baseKey(tcv[4])) ? tcv[4] : t.palette,
      cutCover: n(tcv[5], t.cutCover, 0, 95),
      cutHeight: n(tcv[6], t.cutHeight, 0, 39),
      maskCover: n(tcv[7], t.maskCover, 1, 100),
      maskHeight: n(tcv[8], t.maskHeight, 1, 40),
      maskColour: /^[0-9a-f]{6}$/i.test(tcv[9]) ? `#${tcv[9]}` : t.maskColour,
    };
  }
  const rw = p.get('rw')?.split(',').map(Number);
  // Links from before the service-frequency factor carry one weight fewer.
  if (rw && rw.length === RNCOMP - 1) rw.push(RAIL_DEFAULT_WEIGHTS[RNCOMP - 1]);
  if (rw && rw.length === RNCOMP && rw.every(Number.isFinite)) s.rail = { ...s.rail, weights: rw };
  // Rail preset: a link to one this browser has (else custom, keeping the weights), or the default.
  const rp = p.get('rp');
  if (rp !== null) s.rail = { ...s.rail, preset: rp !== 'custom' && railPresets.get(rp) ? rp : '' };
  else if (rw) s.rail = { ...s.rail, preset: sameWeights(s.rail.weights, railPresets.get(RAIL_DEFAULT_PRESET)?.w ?? []) ? RAIL_DEFAULT_PRESET : '' };
  const sf = p.get('sf');
  if (sf !== null) s.surface = { paved: sf.includes('p'), unpaved: sf.includes('u') };
  const tl = p.get('tl');
  if (tl !== null) s.toll = { free: tl.includes('f'), toll: tl.includes('t') };
  const w = Number(p.get('w'));
  if (p.get('w') && w >= WEIGHT_MIN && w <= WEIGHT_MAX) s.weight = w;
  s.routeGlow = p.get('rg') === '1';
  const l = p.get('l');
  if (l !== null) s.layers = { roads: l.includes('r'), water: l.includes('w'), boundaries: l.includes('b'), places: l.includes('p') };
  const o = p.get('o');
  if (o && o.length === OVERLAYS.length) OVERLAYS.forEach(([k], i) => (s.overlays[k] = o[i] === '1'));
  const hl = p.get('hl');
  if (hl && hl.length === 5) s.heritageOff = offFromLevels([...hl].map((c) => c === '1'));
  const ht = p.get('ht');
  if (ht !== null) s.heritageOff = ht.split(',').filter((k) => /^[wnpm]\.[a-z]+$/.test(k));
  const bl = p.get('bl');
  if (bl && bl.length === 3) s.boundaryLevels = [...bl].map((c) => c === '1') as [boolean, boolean, boolean];
  if (p.get('sq')) s.stopFilters = filtersFromHash(p.get('sq')!);
  const sqx = p.get('sqx');
  if (sqx) s.stopUnknown = Object.fromEntries(sqx.split(',').filter((k) => OVERLAYS.some((o) => o[0] === k)).map((k) => [k, false]));
  const lk = p.get('lk');
  if (lk && lk.length === LABEL_KINDS.length) s.labelKinds = Object.fromEntries(LABEL_KINDS.map(([k], i) => [k, lk[i] === '1'])) as Record<LabelKind, boolean>;
  const t3 = p.get('t3')?.split(',').map(Number);
  if (t3 && t3.length >= 9 && t3.every(Number.isFinite)) {
    s.terrain = {
      ...s.terrain,
      on: t3[0] === 1, exaggeration: Math.min(6, Math.max(1, t3[1])), hillshade: t3[2] === 1, method: HM[t3[3]] ?? 'combined',
      light: t3[4], shade: Math.min(1, Math.max(0, t3[5])), tint: t3[6] === 1, contours: t3[7] === 1, sky: t3[8] === 1,
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
  s.occlude = p.get('oc') === '1';
  const lo = Number(p.get('lo'));
  if (p.get('lo') && lo >= 0 && lo <= 1) s.labelOpacity = lo;
  const po = Number(p.get('po'));
  if (p.get('po') && po >= 0 && po <= 1) s.poiOpacity = po;
  const pe = Number(p.get('pe'));
  if (p.get('pe') && pe >= 0 && pe <= 1) s.poiEmphasis = pe;
  const lmv = p.get('lm')?.split(',');
  if (lmv && lmv.length === 12) {
    const d = defaults.landmarks;
    const num = (x: string, dv: number, lo: number, hi: number) => (x !== '' && Number.isFinite(Number(x)) ? Math.min(hi, Math.max(lo, Number(x))) : dv);
    const r0 = num(lmv[2], d.range[0], 0, 1), r1 = num(lmv[3], d.range[1], 0, 1);
    s.landmarks = {
      ...d, balance: num(lmv[0], d.balance, 0, 1), auto: lmv[1] !== '0', range: r1 > r0 ? [r0, r1] : d.range,
      lowFade: num(lmv[4], d.lowFade, 0, 1), ...parseScaleTail(lmv.slice(5), d),
    };
  }
  const t = p.get('t');
  if (t && /^[abl]-?[\d.]+$/.test(t)) s.threshold = { on: true, dir: t[0] === 'a' ? 'above' : t[0] === 'b' ? 'below' : 'low', value: Number(t.slice(1)) };
  const sel = Number(p.get('s'));
  if (p.get('s') && Number.isInteger(sel)) s.selected = sel;
  const sm = p.get('st')?.match(/^([cd]),(-?[\d.]+),(-?[\d.]+),(-?[\d.]+),(-?[\d.]+),(.*)$/);
  if (sm && s.selected !== null) {
    const n = sm.slice(2, 6).map(Number);
    if (n.every(Number.isFinite)) s.stretch = { kind: sm[1] === 'c' ? 'climb' : 'drive', a: [n[0], n[1]], b: [n[2], n[3]], label: sm[6] };
  }
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
  const { view, selected: _sel, stretch: _st, looks, scales, ...rest } = src;
  const w = migrateWeights(rest.weights);
  if (w) rest.weights = w;
  // Rail weights saved before the service-frequency factor: one fewer.
  const rs = rest.rail as { weights?: unknown } | undefined;
  if (rs && Array.isArray(rs.weights) && rs.weights.length === RNCOMP - 1) rs.weights = [...rs.weights, RAIL_DEFAULT_WEIGHTS[RNCOMP - 1]];
  merge(s as unknown as Record<string, unknown>, rest);
  if (Array.isArray(rest.heritageOff)) s.heritageOff = rest.heritageOff.filter((k): k is string => typeof k === 'string');
  else if (Array.isArray(rest.heritageLevels) && rest.heritageLevels.length === 5) s.heritageOff = offFromLevels(rest.heritageLevels.map(Boolean));
  // Saved settings of the other display types and modes.
  const num = (v: unknown) => typeof v === 'number' && Number.isFinite(v);
  const pair = (v: unknown): v is [number, number] => Array.isArray(v) && v.length === 2 && v.every(num);
  const lk = (looks ?? {}) as Record<string, Partial<Look>>;
  for (const g of MODE_GROUPS) {
    const v = lk[g];
    if (v && typeof v.palette === 'string' && pair(v.fit) && typeof v.equalize === 'boolean' && num(v.lowFade) && num(v.lowSpan)
        && typeof v.thrOn === 'boolean' && (v.thrDir === 'above' || v.thrDir === 'below' || v.thrDir === 'low')) {
      s.looks[g] = { palette: v.palette, fit: v.fit, equalize: v.equalize, lowFade: v.lowFade!, lowSpan: v.lowSpan!, thrOn: v.thrOn, thrDir: v.thrDir };
    }
  }
  const sc = (scales ?? {}) as Record<string, Partial<Scale>>;
  for (const m of MODES) {
    const v = sc[m.key];
    if (v && typeof v.auto === 'boolean' && pair(v.range) && num(v.thrValue)) s.scales[m.key] = { auto: v.auto, range: v.range, thrValue: v.thrValue! };
  }
  if (!MODES.some((m) => m.key === s.mode)) s.mode = defaults.mode;
  if (!MAP_SCHEMES.some((m) => m.key === s.mapScheme)) s.mapScheme = defaults.mapScheme;
  if (!RAIL_COLOURS.includes(s.rail.colour)) s.rail.colour = defaults.rail.colour;
  if (!RAIL_METRICS.some((m) => m.key === s.rail.metric)) s.rail.metric = defaults.rail.metric;
  if (!FERRY_COLOURS.includes(s.ferry.colour)) s.ferry.colour = defaults.ferry.colour;
  if (!FERRY_METRICS.some((m) => m.key === s.ferry.metric)) s.ferry.metric = defaults.ferry.metric;
  // Stops & sights filters (merge() only keeps keys the defaults have).
  const sfv = (rest.stopFilters ?? {}) as Record<string, Partial<StopFilter>>;
  s.stopFilters = {};
  for (const d of STOP_FILTERS) {
    const v = sfv[d.key];
    if (v && typeof v.on === 'boolean' && Number.isFinite(v.min) && Number.isFinite(v.max)) s.stopFilters[d.key] = { on: v.on, min: v.min!, max: v.max! };
  }
  const suv = (rest.stopUnknown ?? {}) as Record<string, unknown>;
  s.stopUnknown = Object.fromEntries(OVERLAYS.filter(([k]) => typeof suv[k] === 'boolean').map(([k]) => [k, suv[k] as boolean]));
  // Per-metric colour settings of rail and ferries (merge() only keeps keys the defaults have).
  const rl = ((rest.rail as { looks?: Record<string, unknown> } | undefined)?.looks ?? {}) as Record<string, unknown>;
  s.rail.looks = {};
  for (const m of RAIL_METRICS) {
    const v = validLook(rl[m.key]);
    if (v) s.rail.looks[m.key] = v;
  }
  const fl = ((rest.ferry as { looks?: Record<string, unknown> } | undefined)?.looks ?? {}) as Record<string, unknown>;
  s.ferry.looks = {};
  for (const m of FERRY_METRICS) {
    const v = validLook(fl[m.key]);
    if (v) s.ferry.looks[m.key] = v;
  }
  // Saved before the ferry metric scale existed: its range in the metric's units.
  if (!(s.ferry.range[1] > s.ferry.range[0])) s.ferry.range = [...ferryMetricDef(s.ferry.metric).range];
  if (!(s.rail.range[1] > s.rail.range[0])) s.rail.range = [...railMetricDef(s.rail.metric).range];
  const v = view as AppState['view'];
  if (v && [v.zoom, v.lat, v.lng].every(Number.isFinite)) {
    s.view = { zoom: v.zoom, lat: v.lat, lng: v.lng, bearing: Number(v.bearing) || 0, pitch: Number(v.pitch) || 0, elev: Number(v.elev) || 0 };
  }
  return s;
}
