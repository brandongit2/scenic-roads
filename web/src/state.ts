import { GROUPS, NGROUP, NRAIL } from './config';
import { RAIL_METRICS, RNCOMP, railMetricDef, type RailColour, type RailMetric } from './rail';
import { MAP_SCHEMES } from './mapschemes';
import { FERRY_METRICS, NFERRY, ferryMetricDef, type FerryColour, type FerryMetric } from './ferry';
import { baseKey } from './palettes';
import { STOP_FILTERS, filtersFromHash, filtersToHash, type StopFilter } from './stopfilters';
import { TREE_PALETTES, type TreeState, type TreeStyle, type TreeVar } from './trees';
import type { BuildingColour, BuildingState } from './buildings';
import { BUILTIN, DEFAULT_PRESET, DEFAULT_WEIGHTS, RAIL_DEFAULT_PRESET, RAIL_DEFAULT_WEIGHTS, presets, railPresets, sameWeights } from './presets';
import { MODES, migrateWeights, modeDef, type Mode } from './scenic';

export type { Mode };

export type HillshadeMethod = 'standard' | 'basic' | 'combined' | 'igor' | 'multidirectional';
export type TintVar = 'elev' | 'slope';

/** Overlay layers (designations, stops, context). */
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
  ['stations', 'Rail stations & stops', 'Names of the rail stops shown, once their line\'s stops are well apart on screen'],
] as const;
export type LabelKind = (typeof LABEL_KINDS)[number][0];

/** Label density groups (Layers → Map labels → Density): each a factor on its labels' density. */
export const DENSITY_KINDS = [
  ['places', 'Places', 'Cities, towns, villages, hamlets and neighbourhoods, provinces and states'],
  ['water', 'Water', 'Seas, bays and lakes (river names follow their rivers)'],
  ['parks', 'Parks', 'Parks and protected areas'],
  ['landmarks', 'Stops & sights', 'Stops & sights and heritage sites'],
  ['stations', 'Rail stations', 'Rail stops (by their line\'s stop spacing)'],
] as const;
export type DensityKind = (typeof DENSITY_KINDS)[number][0];
export interface LabelDensity {
  /** A label shows once the distance to the nearest label of its kind that matters more spans this
   * many pixels (dem/interest.py isolation; rail stops: their line's stop spacing). */
  px: number;
  /** Per kind, a factor on density: ×2 halves its spacing. */
  kinds: Record<DensityKind, number>;
  /** Thinning toward the horizon in a pitched view (horizon.ts): 0 none … 1 strong. */
  horizon: number;
}
export const DEFAULT_DENSITY: LabelDensity = { px: 90, kinds: { places: 1, water: 1, parks: 1, landmarks: 1, stations: 1 }, horizon: 0.5 };
/** The label spacing range (px); per kind, no closer than MIN_SPACING_PX (dem/labels.py MIN_PX:
 * the label tiles hold a label from the zoom it shows at that). */
export const SPACING_RANGE: [number, number] = [30, 300];
export const MIN_SPACING_PX = 16;
/** A kind's spacing (px): the density's, over the kind's factor; `base` for a kind spaced on its
 * own scale (rail stops: 70 at the default 90). */
export const kindSpacing = (d: LabelDensity, kind: DensityKind, base = DEFAULT_DENSITY.px): number =>
  Math.max(MIN_SPACING_PX, (d.px * base) / DEFAULT_DENSITY.px / Math.max(0.05, d.kinds[kind]));

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
  /** The tint's colour scale per variable (ui/scale.ts, as the roads'): palette (terrain.ts
   * TINT_PALETTES; 'roads' follows the road palette), range auto-fitted to the terrain in view
   * (percentiles `fit`), locked or full, equalisation, low-end fade and highlight. */
  tintScales: Record<TintVar, ScaleFields>;
  /** The range follows the road colours' while roads show elevation (or grade, for slope). */
  tintMatch: boolean;
  /** Band height in metres (or percent slope), 0 = smooth. */
  tintBands: number;
  /** Ramp exponent: < 1 spends more colour on lowlands, > 1 on highlands. */
  tintCurve: number;
  contours: boolean;
  /** How the contour lines look (contours.ts draws them). */
  contour: ContourLook;
  sky: boolean;
}

/** Contour lines' look (Layers → Terrain). */
export interface ContourLook {
  /** Width × (the global line weight on top). */
  weight: number;
  /** Opacity of the minor and the major (every fifth, labelled) lines. */
  minor: number;
  major: number;
  /** #rrggbb */
  colour: string;
  /** Interval: the intervals' zoom table shifted by this (1: each zoom's finer intervals a zoom sooner). */
  density: number;
  /** How much widths follow the distance, tilted: 0 the same everywhere … 1 as the ground. */
  perspective: number;
  labels: boolean;
  /** Closed rings smaller than this across (CSS px at their tile's zoom) left out: specks of flat
   * land a hair above an interval. */
  ring: number;
  /** Size of the contour labels (× Label size). */
  labelSize: number;
}

/** Water colour and the coastal shading (coast.ts). */
export interface WaterLook {
  /** #rrggbb, the sea's (lakes and rivers a shade lighter). */
  colour: string;
  /** Shading along the coasts: a band of colour over the water, fading out from the shore. */
  shade: boolean;
  /** Band width, CSS px at the view centre (a ground distance: narrower further off when tilted). */
  width: number;
  /** Opacity at the shore. */
  strength: number;
  /** Fade exponent across the band: 1 linear, higher hugs the shore. */
  falloff: number;
  /** #rrggbb */
  shadeColour: string;
  /** A thin line along the shore, its opacity. */
  shore: number;
  /** Lines along the coast inside the band, like an engraved map's (0 none), and their opacity. */
  ripples: number;
  rippleStrength: number;
  /** Lake and river shores too, not just the sea's. */
  lakes: boolean;
}

export type ThresholdDir = 'above' | 'below' | 'low';
/** The selected road or rail line: a way of it (OSM id) and a point on that way (lng, lat), which
 * the way APIs need with the id. */
export interface Selection {
  way: number;
  at: [number, number];
}
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
/** A screen-widths auto-fit from a link (more, then fewer; else `d`). */
function fitLenOf(a: string | undefined, b: string | undefined, d: [number, number]): [number, number] {
  const lo = Number(a), hi = Number(b);
  return a && b && Number.isFinite(lo) && Number.isFinite(hi) && hi > 0 && lo > hi ? [lo, hi] : d;
}

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

/** A colour scale's fields in a link: palette, auto, range, fit, equalise, fades, highlight. */
function scaleStr(x: ScaleFields): string {
  return [x.palette, x.auto ? 1 : 0, +x.range[0].toFixed(3), +x.range[1].toFixed(3), +x.lowFade.toFixed(2),
    x.fit[0], x.fit[1], x.equalize ? 1 : 0, +x.lowSpan.toFixed(2), x.threshold.on ? 1 : 0, THR_CODE[x.threshold.dir], +x.threshold.value.toFixed(3)].join(',');
}
function parseScale(v: string[], d: ScaleFields): ScaleFields {
  const n = (x: string | undefined, dv: number) => (x !== undefined && x !== '' && Number.isFinite(Number(x)) ? Number(x) : dv);
  const lo = n(v[2], d.range[0]), hi = n(v[3], d.range[1]);
  return {
    palette: v[0] || d.palette, auto: v[1] === '1', range: hi > lo ? [lo, hi] : [...d.range], lowFade: Math.min(1, Math.max(0, n(v[4], d.lowFade))),
    ...parseScaleTail(v.slice(5), d),
  };
}

/** The tint from settings saved or linked before its colour scale: the old palette, range mode
 * (whole region, fitted to the view, the roads', custom min–max) and fades, per variable. */
function migrateTint(t: Terrain, o: Record<string, unknown>): Terrain {
  const num = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
  const tv: TintVar = o.tintVar === 'elev' ? 'elev' : o.tintVar === 'slope' ? 'slope' : t.tintVar;
  const sc = { ...t.tintScales[tv] };
  if (typeof o.tintPalette === 'string' && o.tintPalette) sc.palette = o.tintPalette;
  const full: [number, number] = tv === 'slope' ? [0, 100] : [0, 1900];
  if (o.tintRange === 'view') sc.auto = true;
  else if (o.tintRange === 'region') Object.assign(sc, { auto: false, range: full });
  else if (o.tintRange === 'custom' && num(o.tintMin) && num(o.tintMax) && o.tintMax > o.tintMin) Object.assign(sc, { auto: false, range: [o.tintMin, o.tintMax] });
  const fade = o.tintFade as Record<string, unknown> | undefined, span = o.tintFadeSpan as Record<string, unknown> | undefined;
  const out: Terrain = { ...t, tintVar: tv, tintMatch: o.tintRange === 'roads', tintScales: { ...t.tintScales, [tv]: sc } };
  for (const k of ['elev', 'slope'] as const) {
    const f = fade?.[k], sp = span?.[k];
    if (num(f) || num(sp)) out.tintScales[k] = { ...out.tintScales[k], ...(num(f) ? { lowFade: Math.min(1, Math.max(0, f)) } : {}), ...(num(sp) ? { lowSpan: Math.min(1, Math.max(0.05, sp)) } : {}) };
  }
  if (num(o.tintBands)) out.tintBands = Math.max(0, o.tintBands);
  if (num(o.tintCurve)) out.tintCurve = Math.min(3, Math.max(0.3, o.tintCurve));
  if (num(o.tintOpacity)) out.tintOpacity = Math.min(1, Math.max(0, o.tintOpacity));
  return out;
}

/** Landmark fit ranks: whole numbers, the low end's rank below (after) the top end's. */
export const topRanks = (lo: number, hi: number): [number, number] => {
  const h = Math.max(1, Math.round(hi));
  return [Math.max(h + 1, Math.round(lo)), h];
};

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

/** The colour-scale fields shared by roads, rail, ferries and the terrain tint (see ui/scale.ts). */
export interface ScaleFields {
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

/** Passenger rail layer and its colouring (top-left panel, "Rail" section). */
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
  /** The rail layer's opacity, 0.1..1 (Layers). */
  opacity: number;
  /** Colour for 'single'. */
  single: string;
  /** Service-frequency filter (Layers): trains a day each way, 0 = no limit; keep unknown lines. */
  freqOn: boolean;
  freqMin: number;
  freqMax: number;
  freqUnknown: boolean;
  /** The ranked metrics' auto-fit (rail.ts byLen): the best this much rail in view, in screen
   * widths, to the best this much (as the roads' fitLen). */
  fitLen: [number, number];
}
const RAIL_COLOURS: RailColour[] = ['line', 'group', 'metric', 'single'];
/** Passenger ferries and their colouring (top-left panel, "Ferries" section). */
export interface FerryState extends ScaleFields {
  on: boolean;
  /** Service groups shown: urban & commuter, short crossings, long-distance & overnight, cable & chain. */
  groups: boolean[];
  colour: FerryColour;
  /** What the metric colouring shows (sailings a day, season length). */
  metric: FerryMetric;
  looks: Partial<Record<FerryMetric, MetricLook>>;
  /** Line opacity 0..1 and dashed lines (Layers). */
  opacity: number;
  dashed: boolean;
  single: string;
  /** Sailings-a-day filter (Layers): 0 = no limit; keep lines without a timetable. */
  freqOn: boolean;
  freqMin: number;
  freqMax: number;
  freqUnknown: boolean;
  /** Sailings a day's auto-fit (ferry.ts byLen): the busiest this much ferry line in view, in
   * screen widths, to the busiest this much (as the roads' fitLen). */
  fitLen: [number, number];
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
  /** Auto-fit percentiles of the roads in view (low, high), 0–100 (not the scenic metrics: fitLen). */
  fit: [number, number];
  /** The scenic metrics' auto-fit: the best this much road in view, in screen widths (road as long
   * as the view is wide at its centre): the scale's low end at the first, full colour from the
   * second. A fixed amount of road, not a share of it, so a view of mostly bland streets doesn't
   * pull the scale down to them (as the landmarks' top ranks). */
  fitLen: [number, number];
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
  /** 3D buildings (Layers → Buildings). */
  buildings: BuildingState;
  surface: { paved: boolean; unpaved: boolean };
  /** Toll-free and toll roads shown (OSM toll=yes). */
  toll: { free: boolean; toll: boolean };
  lineWeights: LineWeights;
  /** Opacity of the roads layer (every display type), 0.1..1. */
  roadOpacity: number;
  /** Opacity of the boundary lines (countries, provinces & states, counties), 0..1. */
  boundaryOpacity: number;
  routeGlow: boolean;
  /** Transparency at the low end of the colour scale (0..1) and the share of the scale it spans. */
  lowFade: number;
  lowSpan: number;
  /** Opacity of all map labels. */
  labelOpacity: number;
  /** Size of all map labels (× their own). */
  labelSize: number;
  /** Water colour and the shading along coasts (Layers → Map → Water). */
  water: WaterLook;
  /** How densely labels show (Layers → Map labels → Density). */
  labelDensity: LabelDensity;
  /** Opacity of the Stops & sights layers (dots, areas and, with labelOpacity, their labels). */
  poiOpacity: number;
  /** How strongly stops & sights and heritage dots are sized and faded by prominence: 0 all alike,
   * 1 the least prominent tiny and faint. */
  poiEmphasis: number;
  /** Landmark prominence: a scale over each dot's score (0–1: fame and rarity mixed by `balance`,
   * 0 fame only … 1 rarity only), like the road colour scales: range (auto-fitted to ranks of the
   * landmarks in view, locked or full), equalisation, low-end fade and highlight. The palette only
   * draws the legend. `top`: the auto-fitted range runs from the score of the top[0]-th best
   * landmark in view to that of the top[1]-th (counts, not percentiles: a share of the landmarks
   * in view would light up many times more of them where they are dense, Europe against Canada;
   * one bar for everything in view keeps regions comparable). `fit` is unused. */
  landmarks: ScaleFields & { balance: number; top: [number, number] };
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
  selected: Selection | null;
  /** Highlighted stretch of the selected road: a climb or scenic drive picked from a list. */
  stretch: Stretch | null;
  /** elev: camera pivot height (m, exaggerated) when the camera doesn't follow the terrain. */
  view: { zoom: number; lat: number; lng: number; bearing: number; pitch: number; elev: number } | null;
}

/** Kinds of line with their own weight (Layers → Map, under Global line weight): key, label, help;
 * in the panel's order and the link's. */
export const LINE_KINDS = [
  ['roads', 'Roads', 'Roads, and the highlights along them: selected road, scenic drives, climbs'],
  ['rail', 'Rail', 'Passenger rail lines and their stop dots'],
  ['ferries', 'Ferries', 'Ferry lines and their terminal dots'],
  ['borders', 'Borders', 'Borders of countries, provinces & states and counties'],
  ['rivers', 'Rivers & canals', 'Rivers, canals and streams'],
  ['outlines', 'Area outlines', 'Outlines of parks, heritage sites and districts, biospheres, geoparks, dark-sky places and Indigenous lands, and World Heritage lines such as canals and walls'],
] as const;
export type LineKind = (typeof LINE_KINDS)[number][0];
/** Line weights: `global` scales every line on the map (contour lines too); each kind's is
 * relative to it. */
export type LineWeights = { global: number } & Record<LineKind, number>;
/** Slider limits of the weights (global, and each kind's). */
export const WEIGHT_RANGE: [number, number] = [0.25, 3];
const clampWeight = (v: number) => Math.min(WEIGHT_RANGE[1], Math.max(WEIGHT_RANGE[0], v));
/** A kind of line's weight in effect: the global weight times its own. */
export const lineWeight = (s: AppState, kind: LineKind) => s.lineWeights.global * s.lineWeights[kind];
/** Road widths at weight 1 (roads/layer.ts WIDTHS × this). */
export const ROAD_WEIGHT = 0.5;

// The defaults (as set up in the app on 2026-09-30): scenic score in PuBuGn over the top fifth of
// the roads in view, with Big vistas; no unnamed service roads, no roads under 500 m; rail by ride
// score (My preset), ferries by sailings a day; the tree mask; the slope tint; landmarks and
// heritage on.
export const defaults: AppState = {
  mode: 'score',
  looks: {},
  scales: {},
  palette: 'pubugn',
  auto: true,
  range: [0, 100],
  fit: [80, 99.9],
  fitLen: [15, 1],
  equalize: false,
  weights: [...DEFAULT_WEIGHTS],
  preset: DEFAULT_PRESET,
  groups: new Array(NGROUP).fill(true),
  unnamed: GROUPS.map((g) => g.key !== 'service'),
  roadLen: [0.5, 0],
  roadLenOn: true,
  mapScheme: 'blueprint',
  rail: {
    on: true, groups: new Array(NRAIL).fill(true), colour: 'metric', metric: 'rscore', looks: {},
    ...scaleOfLook({ ...freshLook([0, 100], 0.6), palette: 'rocket', fit: [70, 99.8] }),
    weights: [...RAIL_DEFAULT_WEIGHTS], preset: RAIL_DEFAULT_PRESET, opacity: 1, single: '#e8ecf2',
    freqOn: false, freqMin: 0, freqMax: 0, freqUnknown: true, fitLen: [15, 1],
  },
  trees: {
    on: true, variable: 'cover', style: 'mask', opacity: 0.05, palette: 'greens',
    cutCover: 20, cutHeight: 5, maskCover: 20, maskHeight: 10, maskColour: '#03a300',
  },
  // (Opaque on a touch screen: one pass instead of two, docs/buildings3d.md §4.6.)
  buildings: { on: true, flat: false, colour: 'plain', opacity: typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches ? 1 : 0.85, scale: 1, skyline: false },
  ferry: { on: true, groups: new Array(NFERRY).fill(true), colour: 'freq', metric: 'freq', looks: {},
    ...scaleOfLook({ ...freshLook(FERRY_METRICS[0].range, 0.45), fit: [0, 100], palette: 'oslo', lowSpan: 0.5 }),
    opacity: 0.9, dashed: true, single: '#8fc8ff',
    freqOn: false, freqMin: 0, freqMax: 0, freqUnknown: true, fitLen: [15, 1] },
  surface: { paved: true, unpaved: true },
  toll: { free: true, toll: true },
  lineWeights: { global: 1, roads: 1.5, rail: 0.5, ferries: 0.5, borders: 1, rivers: 1, outlines: 1 },
  roadOpacity: 1,
  boundaryOpacity: 1,
  routeGlow: false,
  lowFade: 0.8,
  lowSpan: 0.6,
  labelOpacity: 0.5,
  labelSize: 1,
  water: {
    colour: '#0c1622', shade: true, width: 28, strength: 0.22, falloff: 1.8, shadeColour: '#4f7fa8',
    shore: 0.25, ripples: 0, rippleStrength: 0.35, lakes: false,
  },
  labelDensity: { ...DEFAULT_DENSITY, kinds: { ...DEFAULT_DENSITY.kinds } },
  poiOpacity: 0.7,
  poiEmphasis: 1,
  landmarks: { palette: 'oslo', auto: true, range: [0, 1], fit: [70, 99.9], equalize: false, lowFade: 0.8, lowSpan: 0.4, threshold: { on: false, dir: 'above', value: 0.5 }, balance: 0.3, top: [250, 10] },
  globe: true,
  occlude: true,
  layers: { roads: true, water: true, boundaries: true, places: true },
  boundaryLevels: [true, true, true],
  labelKinds: Object.fromEntries(LABEL_KINDS.map(([k]) => [k, true])) as Record<LabelKind, boolean>,
  // Every stop and sight, heritage sites and districts and Indigenous lands (not parks, biospheres & co.).
  overlays: Object.fromEntries(OVERLAYS.map(([k]) => [k, k !== 'parks' && k !== 'special'])) as Record<OverlayKey, boolean>,
  heritageOff: [],
  stopFilters: {},
  stopUnknown: { waterfall: false },
  terrain: {
    on: true, exaggeration: 3, hillshade: true, method: 'combined', light: 315, shade: 0.15,
    tint: true, tintVar: 'slope', tintOpacity: 0.1, tintBands: 0, tintCurve: 1, tintMatch: false,
    tintScales: {
      elev: { palette: 'atlas', auto: true, range: [0, 1900], fit: [1, 99], equalize: false, lowFade: 0, lowSpan: 0.5, threshold: { on: false, dir: 'above', value: 1000 } },
      slope: { palette: 'plasma_r', auto: false, range: [10, 70], fit: [2, 98], equalize: false, lowFade: 1, lowSpan: 0.1, threshold: { on: false, dir: 'above', value: 30 } },
    },
    contours: false,
    contour: { weight: 1, minor: 0.16, major: 0.34, colour: '#a9b6c8', density: 0, perspective: 1, labels: true, ring: 6, labelSize: 1 },
    sky: true,
  },
  threshold: { on: false, dir: 'above', value: 60 },
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
    const lk = looks[modeGroup(mode)] ?? (modeGroup(mode) === modeGroup(defaults.mode) ? lookOf(defaults) : FRESH_LOOK);
    const sc = scales[mode] ?? { auto: d.auto, range: [...d.range] as [number, number], thrValue: d.thrDefault };
    this.set({
      mode, looks, scales,
      palette: lk.palette, fit: [...lk.fit], equalize: lk.equalize, lowFade: lk.lowFade, lowSpan: lk.lowSpan,
      auto: sc.auto, range: [...sc.range], threshold: { on: lk.thrOn, dir: lk.thrDir, value: sc.thrValue },
    });
  }
}

/** A display type's colours the first time it is picked, unless it is the default one (whose are
 * the defaults'). */
const FRESH_LOOK: Look = { palette: 'viridis', fit: [1, 99], equalize: false, lowFade: 0.7, lowSpan: 0.6, thrOn: false, thrDir: 'above' };

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
// #map=zoom/lat/lng/bearing/pitch&m=score&p=viridis&r=lo,hi&eq=1&pr=vistas&wt=…&g=11111&sf=pu&lw=1,1,1,1,1,1,1
//  &l=rwbp&o=<overlay bits>&hl=<levels>&t3=…&t=a500&s=<way>,<lng>,<lat>

const ob = (k: OverlayKey) => OVERLAYS.findIndex((o) => o[0] === k);
/** The service-frequency weight given to rail weights from before the factor existed (then the
 * default preset's). */
const FREQ_WEIGHT_ADDED = 0.3;
const HM: HillshadeMethod[] = ['standard', 'basic', 'combined', 'igor', 'multidirectional'];

/** The state as a link (the address bar's hash). `buildings`: whether the catalog has the 3D
 * buildings; without them their settings stay out of links. */
export function toHash(s: AppState, buildings: boolean): string {
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
  if (s.fitLen[0] !== defaults.fitLen[0] || s.fitLen[1] !== defaults.fitLen[1]) p.set('fl', `${s.fitLen[0]},${s.fitLen[1]}`);
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
  // (Fields 8–10, empty: line weight, ties and casing in older links.)
  const rs = [
    r.on ? 1 : 0, r.groups.map((g) => (g ? 1 : 0)).join(''), r.colour, r.metric, r.palette, r.auto ? 1 : 0,
    +r.range[0].toFixed(2), +r.range[1].toFixed(2), '', '', '', r.single.replace('#', ''), +r.lowFade.toFixed(2),
    r.freqOn ? 1 : 0, +r.freqMin.toFixed(2), +r.freqMax.toFixed(2), r.freqUnknown ? 1 : 0,
    r.fit[0], r.fit[1], r.equalize ? 1 : 0, +r.lowSpan.toFixed(2), r.threshold.on ? 1 : 0, THR_CODE[r.threshold.dir], +r.threshold.value.toFixed(3),
    +r.opacity.toFixed(2), r.fitLen[0], r.fitLen[1],
  ].join(',');
  const rsd = [
    dr.on ? 1 : 0, dr.groups.map((g) => (g ? 1 : 0)).join(''), dr.colour, dr.metric, dr.palette, dr.auto ? 1 : 0,
    dr.range[0], dr.range[1], '', '', '', dr.single.replace('#', ''), dr.lowFade,
    dr.freqOn ? 1 : 0, dr.freqMin, dr.freqMax, dr.freqUnknown ? 1 : 0,
    dr.fit[0], dr.fit[1], dr.equalize ? 1 : 0, dr.lowSpan, dr.threshold.on ? 1 : 0, THR_CODE[dr.threshold.dir], dr.threshold.value,
    dr.opacity, dr.fitLen[0], dr.fitLen[1],
  ].join(',');
  if (rs !== rsd) p.set('rs', rs);
  if (r.weights.some((w, i) => w !== dr.weights[i])) p.set('rw', r.weights.map((w) => +w.toFixed(2)).join(','));
  if (r.preset !== dr.preset) p.set('rp', r.preset || 'custom');
  // (Field 4, empty: line weight in older links.)
  const fy = (f: FerryState) => [f.on ? 1 : 0, f.groups.map((g) => (g ? 1 : 0)).join(''), f.colour, f.palette, '', f.dashed ? 1 : 0, f.single.replace('#', ''), +f.opacity.toFixed(2),
    f.freqOn ? 1 : 0, +f.freqMin.toFixed(2), +f.freqMax.toFixed(2), f.freqUnknown ? 1 : 0,
    f.metric, f.auto ? 1 : 0, +f.range[0].toFixed(3), +f.range[1].toFixed(3), f.fit[0], f.fit[1], f.equalize ? 1 : 0, +f.lowFade.toFixed(2), +f.lowSpan.toFixed(2),
    f.threshold.on ? 1 : 0, THR_CODE[f.threshold.dir], +f.threshold.value.toFixed(3), f.fitLen[0], f.fitLen[1]].join(',');
  if (fy(s.ferry) !== fy(defaults.ferry)) p.set('fy', fy(s.ferry));
  const tc = (t: TreeState) => [t.on ? 1 : 0, t.variable, t.style, +t.opacity.toFixed(2), t.palette, t.cutCover, t.cutHeight, t.maskCover, t.maskHeight, t.maskColour.replace('#', '')].join(',');
  if (tc(s.trees) !== tc(defaults.trees)) p.set('tc', tc(s.trees));
  const bd = (b: BuildingState) => [b.on ? 1 : 0, b.flat ? 1 : 0, b.colour, +b.opacity.toFixed(2), +b.scale.toFixed(2), b.skyline ? 1 : 0].join(',');
  if (buildings && bd(s.buildings) !== bd(defaults.buildings)) p.set('bd', bd(s.buildings));
  if (!(s.surface.paved && s.surface.unpaved)) p.set('sf', `${s.surface.paved ? 'p' : ''}${s.surface.unpaved ? 'u' : ''}`);
  if (!(s.toll.free && s.toll.toll)) p.set('tl', `${s.toll.free ? 'f' : ''}${s.toll.toll ? 't' : ''}`);
  const lw = (l: LineWeights) => [l.global, ...LINE_KINDS.map(([k]) => l[k])].map((v) => +v.toFixed(2)).join(',');
  if (lw(s.lineWeights) !== lw(defaults.lineWeights)) p.set('lw', lw(s.lineWeights));
  if (s.roadOpacity !== defaults.roadOpacity) p.set('ro', s.roadOpacity.toFixed(2));
  if (s.boundaryOpacity !== defaults.boundaryOpacity) p.set('bo', s.boundaryOpacity.toFixed(2));
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
  const tv = (x: Terrain) => [x.tintVar, x.tintBands, +x.tintCurve.toFixed(2), +x.tintOpacity.toFixed(2), x.tintMatch ? 1 : 0].join(',');
  if (tv(t) !== tv(dt)) p.set('tv', tv(t));
  const cl = (c: ContourLook) => [+c.weight.toFixed(2), +c.minor.toFixed(2), +c.major.toFixed(2), c.colour.slice(1), c.density, +c.perspective.toFixed(2), c.labels ? 1 : 0, c.ring, +c.labelSize.toFixed(2)].join(',');
  if (cl(t.contour) !== cl(dt.contour)) p.set('cl', cl(t.contour));
  for (const [k, key] of [['elev', 'te'], ['slope', 'ts']] as const) {
    if (scaleStr(t.tintScales[k]) !== scaleStr(dt.tintScales[k])) p.set(key, scaleStr(t.tintScales[k]));
  }
  if (s.lowFade !== defaults.lowFade || s.lowSpan !== defaults.lowSpan) p.set('lf', `${+s.lowFade.toFixed(2)},${+s.lowSpan.toFixed(2)}`);
  if (s.labelOpacity !== defaults.labelOpacity) p.set('lo', s.labelOpacity.toFixed(2));
  if (s.labelSize !== defaults.labelSize) p.set('lz', s.labelSize.toFixed(2));
  const wa = (w: WaterLook) => [w.colour.slice(1), w.shade ? 1 : 0, w.width, +w.strength.toFixed(2), +w.falloff.toFixed(2), w.shadeColour.slice(1),
    +w.shore.toFixed(2), w.ripples, +w.rippleStrength.toFixed(2), w.lakes ? 1 : 0].join(',');
  if (wa(s.water) !== wa(defaults.water)) p.set('wa', wa(s.water));
  const ldv = (d: LabelDensity) => [d.px, ...DENSITY_KINDS.map(([k]) => +d.kinds[k].toFixed(3)), +d.horizon.toFixed(2)].join(',');
  if (ldv(s.labelDensity) !== ldv(defaults.labelDensity)) p.set('ld', ldv(s.labelDensity));
  if (s.poiOpacity !== defaults.poiOpacity) p.set('po', s.poiOpacity.toFixed(2));
  if (s.poiEmphasis !== defaults.poiEmphasis) p.set('pe', s.poiEmphasis.toFixed(2));
  const lm = (l: AppState['landmarks']) => [+l.balance.toFixed(2), l.auto ? 1 : 0, +l.range[0].toFixed(3), +l.range[1].toFixed(3), +l.lowFade.toFixed(2),
    l.fit[0], l.fit[1], l.equalize ? 1 : 0, +l.lowSpan.toFixed(2), l.threshold.on ? 1 : 0, THR_CODE[l.threshold.dir], +l.threshold.value.toFixed(3), l.top[0], l.top[1]].join(',');
  if (lm(s.landmarks) !== lm(defaults.landmarks)) p.set('lm', lm(s.landmarks));
  if (!s.globe) p.set('gb', '0');
  if (s.occlude) p.set('oc', '1');
  if (s.threshold.on) p.set('t', `${THR_CODE[s.threshold.dir]}${+s.threshold.value.toFixed(3)}`);
  // The selected way's OSM id and a point on it.
  if (s.selected !== null) p.set('s', `${s.selected.way},${s.selected.at.map((v) => v.toFixed(5)).join(',')}`);
  if (s.selected !== null && s.stretch) {
    const { kind, a, b, label } = s.stretch;
    p.set('st', `${kind[0]},${[...a, ...b].map((v) => v.toFixed(5)).join(',')},${label}`);
  }
  return '#' + p.toString().replace(/%2F/g, '/').replace(/%2C/g, ',');
}

/** The state a link gives (`buildings`: whether the catalog has the 3D buildings; without, their
 * `bd=` is left out, as `toHash` leaves it out). */
export function fromHash(hash: string, buildings = true): AppState {
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
  const fl = p.get('fl')?.split(',').map(Number);
  if (fl && fl.length === 2 && fl.every(Number.isFinite) && fl[1] > 0 && fl[0] > fl[1]) s.fitLen = [fl[0], fl[1]];
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
      single: /^[0-9a-f]{6}$/i.test(rs[11]) ? `#${rs[11]}` : r.single,
      lowFade: Math.min(1, Math.max(0, num(rs[12], r.lowFade))),
      freqOn: rs[13] === '1',
      freqMin: Math.max(0, num(rs[14] ?? '', r.freqMin)),
      freqMax: Math.max(0, num(rs[15] ?? '', r.freqMax)),
      freqUnknown: rs[16] === undefined ? r.freqUnknown : rs[16] === '1',
      ...(rs.length >= 24 ? parseScaleTail(rs.slice(17), r) : {}),
      opacity: rs[24] ? Math.min(1, Math.max(0.1, num(rs[24], r.opacity))) : r.opacity,
      fitLen: fitLenOf(rs[25], rs[26], r.fitLen),
    };
    // Older links: the rail card's line weight.
    if (rs[8]) s.lineWeights.rail = clampWeight(num(rs[8], 1));
  }
  const fy = p.get('fy')?.split(',');
  if (fy && fy.length >= 7) {
    const f = s.ferry;
    const op = Number(fy[7]);
    // Older links: the ferry card's line weight.
    if (fy[4] && Number.isFinite(Number(fy[4]))) s.lineWeights.ferries = clampWeight(Number(fy[4]));
    s.ferry = {
      on: fy[0] === '1',
      groups: fy[1].length === NFERRY ? [...fy[1]].map((c) => c === '1') : f.groups,
      colour: FERRY_COLOURS.includes(fy[2] as FerryColour) ? (fy[2] as FerryColour) : f.colour,
      palette: fy[3] || f.palette,
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
      fitLen: fitLenOf(fy[24], fy[25], f.fitLen),
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
  const bdv = buildings ? p.get('bd')?.split(',') : undefined;
  if (bdv && bdv.length >= 6) {
    const b = s.buildings;
    const n = (v: string, d: number, lo: number, hi: number) => (v !== '' && Number.isFinite(Number(v)) ? Math.min(hi, Math.max(lo, Number(v))) : d);
    // (The scale 0 means "× the terrain's exaggeration": a negative one is no scale, not that.)
    const scale = Number(bdv[4]);
    s.buildings = {
      on: bdv[0] === '1',
      flat: bdv[1] === '1',
      colour: (['plain', 'height', 'source'] as BuildingColour[]).includes(bdv[2] as BuildingColour) ? (bdv[2] as BuildingColour) : b.colour,
      opacity: n(bdv[3], b.opacity, 0.1, 1),
      scale: bdv[4] !== '' && scale >= 0 ? n(bdv[4], b.scale, 0, 3) : b.scale,
      skyline: bdv[5] === '1',
    };
  }
  const rw = p.get('rw')?.split(',').map(Number);
  // Links from before the service-frequency factor carry one weight fewer.
  if (rw && rw.length === RNCOMP - 1) rw.push(FREQ_WEIGHT_ADDED);
  if (rw && rw.length === RNCOMP && rw.every(Number.isFinite)) s.rail = { ...s.rail, weights: rw };
  // Rail preset: a link to one this browser has (else custom, keeping the weights), or the default.
  const rp = p.get('rp');
  if (rp !== null) s.rail = { ...s.rail, preset: rp !== 'custom' && railPresets.get(rp) ? rp : '' };
  else if (rw) s.rail = { ...s.rail, preset: sameWeights(s.rail.weights, railPresets.get(RAIL_DEFAULT_PRESET)?.w ?? []) ? RAIL_DEFAULT_PRESET : '' };
  const sf = p.get('sf');
  if (sf !== null) s.surface = { paved: sf.includes('p'), unpaved: sf.includes('u') };
  const tl = p.get('tl');
  if (tl !== null) s.toll = { free: tl.includes('f'), toll: tl.includes('t') };
  // Older links: the road line weight (0.5 by default), which scaled rail and ferries too.
  const w = Number(p.get('w'));
  if (p.get('w') && w > 0) s.lineWeights.global = clampWeight(w / ROAD_WEIGHT);
  const lw = p.get('lw')?.split(',').map(Number);
  if (lw && lw.length === LINE_KINDS.length + 1 && lw.every((v) => Number.isFinite(v) && v > 0)) {
    s.lineWeights = { global: clampWeight(lw[0]), ...(Object.fromEntries(LINE_KINDS.map(([k], i) => [k, clampWeight(lw[i + 1])])) as Record<LineKind, number>) };
  }
  const ro = Number(p.get('ro'));
  if (p.get('ro') && ro >= 0.1 && ro <= 1) s.roadOpacity = ro;
  const bo = Number(p.get('bo'));
  if (p.get('bo') && bo >= 0 && bo <= 1) s.boundaryOpacity = bo;
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
  // Kinds added since the link was made (beyond its length) stay on.
  if (lk && lk.length <= LABEL_KINDS.length) s.labelKinds = Object.fromEntries(LABEL_KINDS.map(([k], i) => [k, i >= lk.length || lk[i] === '1'])) as Record<LabelKind, boolean>;
  const t3 = p.get('t3')?.split(',').map(Number);
  if (t3 && t3.length >= 9 && t3.every(Number.isFinite)) {
    s.terrain = {
      ...s.terrain,
      on: t3[0] === 1, exaggeration: Math.min(6, Math.max(1, t3[1])), hillshade: t3[2] === 1, method: HM[t3[3]] ?? 'combined',
      light: t3[4], shade: Math.min(1, Math.max(0, t3[5])), tint: t3[6] === 1, contours: t3[7] === 1, sky: t3[8] === 1,
    };
  }
  // Links from before the tint's scale (palette, range mode, min, max, …; fades per variable).
  const tn = p.get('tn')?.split(','), tf = p.get('tf')?.split(',').map(Number);
  if (tn && tn.length >= 7) s.terrain = migrateTint(s.terrain, { tintPalette: tn[0], tintRange: tn[1], tintMin: Number(tn[2]), tintMax: Number(tn[3]),
    tintBands: Number(tn[4]), tintCurve: Number(tn[5]), tintOpacity: Number(tn[6]), tintVar: tn[7],
    ...(tf && tf.length === 4 && tf.every(Number.isFinite) ? { tintFade: { elev: tf[0], slope: tf[2] }, tintFadeSpan: { elev: tf[1], slope: tf[3] } } : {}) });
  const tvv = p.get('tv')?.split(',');
  if (tvv && tvv.length >= 5) {
    const n = (v: string, d: number) => (v !== '' && Number.isFinite(Number(v)) ? Number(v) : d);
    s.terrain = {
      ...s.terrain, tintVar: tvv[0] === 'elev' ? 'elev' : 'slope', tintBands: Math.max(0, n(tvv[1], 0)), tintCurve: Math.min(3, Math.max(0.3, n(tvv[2], 1))),
      tintOpacity: Math.min(1, Math.max(0, n(tvv[3], s.terrain.tintOpacity))), tintMatch: tvv[4] === '1',
    };
  }
  const cl = p.get('cl')?.split(',');
  if (cl && cl.length >= 8) {
    const n = (v: string, d: number, lo: number, hi: number) => (v !== '' && Number.isFinite(Number(v)) ? Math.min(hi, Math.max(lo, Number(v))) : d);
    const d = defaults.terrain.contour;
    s.terrain = {
      ...s.terrain,
      contour: {
        weight: n(cl[0], d.weight, 0.25, 3), minor: n(cl[1], d.minor, 0, 1), major: n(cl[2], d.major, 0, 1),
        colour: /^[0-9a-f]{6}$/i.test(cl[3]) ? `#${cl[3].toLowerCase()}` : d.colour, density: Math.round(n(cl[4], d.density, -2, 2)),
        perspective: n(cl[5], d.perspective, 0, 1), labels: cl[6] !== '0', ring: Math.round(n(cl[7], d.ring, 0, 24)),
        labelSize: n(cl[8] ?? '', d.labelSize, 0.5, 2.5),
      },
    };
  }
  for (const [k, key] of [['elev', 'te'], ['slope', 'ts']] as const) {
    const v = p.get(key)?.split(',');
    if (v) s.terrain = { ...s.terrain, tintScales: { ...s.terrain.tintScales, [k]: parseScale(v, s.terrain.tintScales[k]) } };
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
  const lz = Number(p.get('lz'));
  if (p.get('lz') && Number.isFinite(lz)) s.labelSize = Math.min(2, Math.max(0.5, lz));
  const wa = p.get('wa')?.split(',');
  if (wa && wa.length >= 10) {
    const n = (v: string, d: number, lo: number, hi: number) => (v !== '' && Number.isFinite(Number(v)) ? Math.min(hi, Math.max(lo, Number(v))) : d);
    const hex = (v: string, d: string) => (/^[0-9a-f]{6}$/i.test(v) ? `#${v.toLowerCase()}` : d);
    const d = defaults.water;
    s.water = {
      colour: hex(wa[0], d.colour), shade: wa[1] !== '0', width: Math.round(n(wa[2], d.width, 2, 80)), strength: n(wa[3], d.strength, 0, 1),
      falloff: n(wa[4], d.falloff, 0.5, 4), shadeColour: hex(wa[5], d.shadeColour), shore: n(wa[6], d.shore, 0, 1),
      ripples: Math.round(n(wa[7], d.ripples, 0, 8)), rippleStrength: n(wa[8], d.rippleStrength, 0, 1), lakes: wa[9] === '1',
    };
  }
  const ld = p.get('ld')?.split(',').map(Number);
  if (ld && ld.length === DENSITY_KINDS.length + 2 && ld.every(Number.isFinite)) {
    const c = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
    s.labelDensity = {
      px: c(ld[0], ...SPACING_RANGE),
      kinds: Object.fromEntries(DENSITY_KINDS.map(([k], i) => [k, c(ld[i + 1], 0.25, 4)])) as Record<DensityKind, number>,
      horizon: c(ld[ld.length - 1], 0, 1),
    };
  }
  const po = Number(p.get('po'));
  if (p.get('po') && po >= 0 && po <= 1) s.poiOpacity = po;
  const pe = Number(p.get('pe'));
  if (p.get('pe') && pe >= 0 && pe <= 1) s.poiEmphasis = pe;
  const lmv = p.get('lm')?.split(',');
  if (lmv && lmv.length >= 12) {
    const d = defaults.landmarks;
    const num = (x: string, dv: number, lo: number, hi: number) => (x !== '' && Number.isFinite(Number(x)) ? Math.min(hi, Math.max(lo, Number(x))) : dv);
    const r0 = num(lmv[2], d.range[0], 0, 1), r1 = num(lmv[3], d.range[1], 0, 1);
    s.landmarks = {
      ...d, balance: num(lmv[0], d.balance, 0, 1), auto: lmv[1] !== '0', range: r1 > r0 ? [r0, r1] : d.range,
      lowFade: num(lmv[4], d.lowFade, 0, 1), ...parseScaleTail(lmv.slice(5), d),
      top: lmv.length >= 14 ? topRanks(num(lmv[12], d.top[0], 1, 100000), num(lmv[13], d.top[1], 1, 100000)) : d.top,
    };
  }
  const t = p.get('t');
  if (t && /^[abl]-?[\d.]+$/.test(t)) s.threshold = { on: true, dir: t[0] === 'a' ? 'above' : t[0] === 'b' ? 'below' : 'low', value: Number(t.slice(1)) };
  // The selected way and a point on it (older links, a way index alone: no selection).
  const sel = p.get('s')?.match(/^(\d+),(-?[\d.]+),(-?[\d.]+)$/);
  if (sel) {
    const [way, lng, lat] = sel.slice(1).map(Number);
    if (Number.isSafeInteger(way) && way > 0 && Math.abs(lng) <= 180 && Math.abs(lat) <= 90) s.selected = { way, at: [lng, lat] };
  }
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
  if (rs && Array.isArray(rs.weights) && rs.weights.length === RNCOMP - 1) rs.weights = [...rs.weights, FREQ_WEIGHT_ADDED];
  merge(s as unknown as Record<string, unknown>, rest);
  const oldT = rest.terrain as Record<string, unknown> | undefined;
  if (oldT && 'tintPalette' in oldT && !('tintScales' in oldT)) s.terrain = migrateTint(s.terrain, oldT);
  if (!rest.lineWeights) {
    const num = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) && v > 0 ? v : null);
    const w = num(rest.weight), rw = num((rest.rail as { weight?: unknown } | undefined)?.weight), fw = num((rest.ferry as { weight?: unknown } | undefined)?.weight);
    if (w) s.lineWeights.global = clampWeight(w / ROAD_WEIGHT);
    if (rw) s.lineWeights.rail = clampWeight(rw);
    if (fw) s.lineWeights.ferries = clampWeight(fw);
  }
  for (const k of ['global', ...LINE_KINDS.map(([k]) => k)] as const) s.lineWeights[k] = clampWeight(s.lineWeights[k]);
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
  const lt = s.landmarks.top as unknown;
  s.landmarks.top = Array.isArray(lt) && lt.length === 2 && lt.every((x) => Number.isFinite(x)) ? topRanks(lt[0], lt[1]) : defaults.landmarks.top;
  if (!RAIL_COLOURS.includes(s.rail.colour)) s.rail.colour = defaults.rail.colour;
  if (!RAIL_METRICS.some((m) => m.key === s.rail.metric)) s.rail.metric = defaults.rail.metric;
  if (!FERRY_COLOURS.includes(s.ferry.colour)) s.ferry.colour = defaults.ferry.colour;
  if (!['plain', 'height', 'source'].includes(s.buildings.colour)) s.buildings.colour = defaults.buildings.colour;
  s.buildings.opacity = Math.min(1, Math.max(0.1, s.buildings.opacity));
  s.buildings.scale = Math.min(3, Math.max(0, s.buildings.scale));
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
