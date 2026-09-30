// Passenger ferries: service groups, colourings and how a line's sailings are described. The data
// (dem/ferries.py) has one feature per OSM way: g = primary group, gs = every group on the way
// (digits), f = sailings a day each way over all lines on the way (-1 unknown), fp = some lines on
// the way unknown, s = season (0 unknown, 1 year-round daily, 2 year-round some days, 3 seasonal),
// m = months a year (-1 unknown), col = official line colour, op = operator, n = name, lines =
// line ids (ferry-lines.json).

import type { ExpressionSpecification } from 'maplibre-gl';
import { paletteRgb } from './palettes';
import type { ThresholdDir } from './state';
import { fadeAlpha, passes, scaleU } from './ui/scale';

/** 'freq' is the metric colouring (its key predates the other metrics). */
export type FerryColour = 'service' | 'freq' | 'season' | 'operator' | 'single';
export type FerryMetric = 'freq' | 'months';

type Expr = ExpressionSpecification;

export interface FerryMetricDef {
  key: FerryMetric;
  label: string;
  help: string;
  /** Default range and the whole scale, in the metric's units. */
  range: [number, number];
  domain: [number, number];
  step: number;
  fmt: (v: number) => string;
  /** The value of a feature (NaN: unknown). */
  value: (p: Record<string, any>) => number;
  /** The same as a MapLibre expression, and whether it is known. */
  expr: Expr;
  known: Expr;
}

export const FERRY_METRICS: FerryMetricDef[] = [
  {
    key: 'freq', label: 'Sailings a day', help: 'Sailings a day each way, every line on the stretch added up (log scale). Grey: no timetable found; light grey: a headway is published but not the hours.',
    range: [Math.log10(1 / 7), 2], domain: [Math.log10(1 / 30), Math.log10(300)], step: 0.02,
    fmt: (v) => fmtPerDay(10 ** v).replace(' a day', '/day').replace(' a week', '/wk'),
    value: (p) => (Number(p.f) > 0 ? Math.log10(Number(p.f)) : NaN),
    expr: ['log10', ['max', ['get', 'f'], 0.001]],
    known: ['>', ['get', 'f'], 0],
  },
  {
    key: 'months', label: 'Season length', help: 'Months a year the line runs (the longest-running line on the stretch): 12 for year-round lines, else from the published season. Grey: unknown.',
    range: [1, 12], domain: [0, 12], step: 0.5,
    fmt: (v) => `${+v.toFixed(1)} mo`,
    value: (p) => (Number(p.m) >= 0 ? Number(p.m) : NaN),
    expr: ['coalesce', ['get', 'm'], -1],
    known: ['>=', ['coalesce', ['get', 'm'], -1], 0],
  },
];
export const ferryMetricDef = (k: FerryMetric) => FERRY_METRICS.find((m) => m.key === k) ?? FERRY_METRICS[0];

/** What the metric colouring needs from the ferry settings. */
export interface FerryScale {
  metric: FerryMetric;
  palette: string;
  equalize: boolean;
  lowFade: number;
  lowSpan: number;
  threshold: { on: boolean; dir: ThresholdDir; value: number };
  single: string;
  colour: FerryColour;
}
/** Lines failing the highlight threshold (as the roads' dimmed colour). */
export const DIMMED = '#3a414c';

export const FERRY_GROUPS = [
  { key: 'urban', label: 'Urban & commuter', one: 'Urban & commuter ferry', help: 'City water buses and commuter boats (Staten Island, Star Ferry, Thames Clippers, Lisbon …)' },
  { key: 'crossing', label: 'Short crossings', one: 'Short crossing', help: 'River, lake, strait and island crossings under 2 h 30' },
  { key: 'long', label: 'Long-distance & overnight', one: 'Long-distance ferry', help: 'Crossings of 2 h 30 or more (Channel, Bay of Biscay, Irish Sea, Newfoundland …)' },
  { key: 'cable', label: 'Cable & chain ferries', one: 'Cable ferry', help: 'Ferries pulled along a cable or chain' },
] as const;
export const NFERRY = FERRY_GROUPS.length;
export const FERRY_GROUP_COLOURS = ['#4fd1c5', '#6aa8ff', '#b48cff', '#f6ad55'];

export const SEASONS: [string, string][] = [
  ['#5b6573', 'Unknown'],
  ['#5cc97a', 'Year-round, daily'],
  ['#3fb6c9', 'Year-round, some days'],
  ['#f0a64a', 'Seasonal'],
];
export const UNKNOWN = '#5b6573';
/** Frequency colouring: a headway is published but not the hours (so no daily count). */
export const HEADWAY_ONLY = '#9aa8bd';

/** Frequency scale: sailings a day each way, log scale from one a week to a hundred a day. */
export const FREQ_LO = Math.log10(1 / 7);
export const FREQ_HI = Math.log10(100);
export const FREQ_TICKS: [number, string][] = [[1 / 7, '1/wk'], [1, '1/day'], [10, '10'], [100, '100/day']];
export const freqT = (perDay: number) => Math.max(0, Math.min(1, (Math.log10(Math.max(perDay, 1e-3)) - FREQ_LO) / (FREQ_HI - FREQ_LO)));

/** Distinct colours for operators without an official line colour. */
const OPERATOR_PALETTE = ['#6aa8ff', '#4fd1c5', '#f6ad55', '#f472b6', '#a3e635', '#b48cff', '#fb7185', '#facc15', '#38bdf8', '#34d399', '#fdba74', '#c4b5fd'];
export function operatorColour(op: string): string {
  if (!op) return UNKNOWN;
  let h = 2166136261;
  for (let i = 0; i < op.length; i++) h = Math.imul(h ^ op.charCodeAt(i), 16777619);
  return OPERATOR_PALETTE[(h >>> 0) % OPERATOR_PALETTE.length];
}

/** Scale position 0..1 as an expression: the range, then the equalisation lookup if any. */
function uExpr(st: FerryScale, range: [number, number], cdf: Uint8Array | null): Expr {
  const d = ferryMetricDef(st.metric);
  const raw: Expr = ['/', ['-', d.expr, range[0]], Math.max(1e-9, range[1] - range[0])];
  if (!cdf || !st.equalize) return raw;
  const stops: number[] = [];
  for (let i = 0; i <= 32; i++) stops.push(i / 32, cdf[Math.round((i / 32) * 255)] / 255);
  return ['interpolate', ['linear'], raw, ...stops] as unknown as Expr;
}

function passExpr(st: FerryScale, range: [number, number]): Expr | null {
  const t = st.threshold;
  if (!t.on) return null;
  const v = ferryMetricDef(st.metric).expr;
  if (t.dir === 'low') return ['>=', v, range[0]];
  return t.dir === 'below' ? ['<=', v, t.value] : ['>=', v, t.value];
}

/** Line colour for the current ferry colouring (range and lookup: the metric scale's). */
export function ferryColourExpr(st: FerryScale, range: [number, number], cdf: Uint8Array | null): Expr | string {
  const { colour, single } = st;
  switch (colour) {
    case 'service':
      return ['match', ['get', 'g'], 0, FERRY_GROUP_COLOURS[0], 1, FERRY_GROUP_COLOURS[1], 2, FERRY_GROUP_COLOURS[2], FERRY_GROUP_COLOURS[3]];
    case 'season':
      return ['match', ['get', 's'], 1, SEASONS[1][0], 2, SEASONS[2][0], 3, SEASONS[3][0], SEASONS[0][0]];
    case 'operator':
      return ['get', 'oc'];
    case 'single':
      return single;
    case 'freq': {
      const d = ferryMetricDef(st.metric);
      const stops: (number | string)[] = [];
      for (let i = 0; i <= 16; i++) stops.push(i / 16, paletteRgb(st.palette, i / 16));
      const ramp = ['interpolate', ['linear'], uExpr(st, range, cdf), ...stops] as unknown as Expr;
      const pass = passExpr(st, range);
      const unknown: Expr | string = st.metric === 'freq' ? ['case', ['>', ['coalesce', ['get', 'hw'], 0], 0], HEADWAY_ONLY, UNKNOWN] : UNKNOWN;
      return ['case', ['!', d.known], unknown, ...(pass ? [['!', pass], DIMMED] : []), ramp] as unknown as Expr;
    }
  }
}

/** Line opacity: the low-end fade over the metric scale, dimmed and unknown lines fainter. */
export function ferryOpacityExpr(st: FerryScale, range: [number, number], cdf: Uint8Array | null, o: number): Expr | number {
  if (st.colour === 'season') return ['case', ['==', ['get', 's'], 0], o * 0.6, o];
  if (st.colour !== 'freq') return o;
  const d = ferryMetricDef(st.metric);
  const stops: number[] = [];
  for (let i = 0; i <= 12; i++) stops.push(i / 12, o * fadeAlpha(i / 12, st.lowFade, st.lowSpan));
  const faded = st.lowFade > 0 ? (['interpolate', ['linear'], uExpr(st, range, cdf), ...stops] as unknown as Expr) : o;
  const pass = passExpr(st, range);
  return ['case', ['!', d.known], o * 0.6, ...(pass ? [['!', pass], o * 0.45] : []), faded] as unknown as Expr;
}

/** The colour a feature is drawn with (same as ferryColourExpr), for hover swatches. */
export function ferryColourOf(p: Record<string, any>, st: FerryScale, range: [number, number], cdf: Uint8Array | null): string {
  switch (st.colour) {
    case 'service': return FERRY_GROUP_COLOURS[p.g] ?? UNKNOWN;
    case 'season': return SEASONS[p.s]?.[0] ?? UNKNOWN;
    case 'operator': return p.oc || UNKNOWN;
    case 'single': return st.single;
    case 'freq': {
      const v = ferryMetricDef(st.metric).value(p);
      if (Number.isNaN(v)) return st.metric === 'freq' && Number(p.hw) > 0 ? HEADWAY_ONLY : UNKNOWN;
      if (!passes(v, st.threshold, range)) return DIMMED;
      return paletteRgb(st.palette, scaleU(v, range, st.equalize ? cdf : null));
    }
  }
}

/** A line's details (ferry-lines.json). */
export interface FerryLine {
  name: string;
  /** English name, where the name isn't English (dem/names.py). */
  en?: string;
  ref: string;
  operator: string;
  network: string;
  from: string;
  to: string;
  via: string;
  group: number;
  km: number;
  duration: number | null;
  vehicles: boolean;
  bicycle: string;
  roundtrip: boolean;
  colour: string;
  website: string;
  wikidata: string;
  osm: string[];
  season: number;
  seasonText: string;
  freq?: {
    per_day?: number;
    per_day_low?: number;
    headway?: number;
    days?: string;
    months?: string;
    overnight?: boolean;
    source?: string;
    url?: string;
    checked?: string;
  };
}

export const fmtDuration = (min: number) => (min < 60 ? `${Math.round(min)} min` : `${Math.floor(min / 60)} h${min % 60 ? ` ${String(Math.round(min % 60)).padStart(2, '0')}` : ''}`);

export function fmtPerDay(v: number): string {
  if (v >= 1) return `${v >= 10 ? Math.round(v) : +v.toFixed(1)} a day`;
  const wk = v * 7;
  return `${wk >= 1.5 ? Math.round(wk) : +wk.toFixed(1)} a week`;
}

/** "24 sailings a day each way · every 20 min" etc. */
export function freqText(l: FerryLine): string | null {
  const f = l.freq;
  if (!f) return null;
  const parts: string[] = [];
  if (f.per_day !== undefined) {
    let s = `${fmtPerDay(f.per_day)} each way`;
    if (f.per_day_low !== undefined && f.per_day_low !== f.per_day) s += ` (${fmtPerDay(f.per_day_low)} off-season)`;
    parts.push(s);
  }
  if (f.headway) parts.push(`every ${fmtDuration(f.headway)}`);
  if (f.days) parts.push(f.days);
  return parts.join(' · ') || null;
}

export function lineTitle(l: FerryLine): string {
  if (l.name) return l.name;
  if (l.from && l.to) return `${l.from} – ${l.to}`;
  return l.vehicles ? 'Car ferry' : 'Ferry';
}
