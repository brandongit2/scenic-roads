// Filters for Stops & sights (Layers panel): min–max ranges and must-have flags per overlay, on the
// numbers dem/filterprops.py stamps onto the features. Applied as MapLibre filters (with each
// layer's own filter) and to the in-view counts.
import type { ExpressionSpecification } from 'maplibre-gl';
import type { OverlayKey } from './state';

type Expr = ExpressionSpecification;

export interface StopFilterDef {
  key: string;
  ov: OverlayKey;
  label: string;
  /** 'range': min–max on a number; 'flag': must be set (a bit of `prop` when `bit` is given). */
  type: 'range' | 'flag';
  prop: string;
  unit?: string;
  bit?: number;
  help?: string;
  /** Range filters: the histogram's axis (values beyond it count at its ends) and its scale. */
  domain?: [number, number];
  axis?: Axis;
}

/** A range filter's scale: even in the value ('lin'), its logarithm ('log'), or the logarithm of
 * its age ('age', years: the last centuries as wide as the millennia before). */
export type Axis = 'lin' | 'log' | 'age';
/** The year ages count from. */
const AGE_REF = 2030;

/** A value's position on an axis (the histograms' bins are even in it), and back. */
export function axisPos(axis: Axis, v: number): number {
  if (axis === 'log') return Math.log10(Math.max(v, 1e-9));
  if (axis === 'age') return -Math.log10(Math.max(1, AGE_REF - v));
  return v;
}
export function axisValue(axis: Axis, t: number): number {
  if (axis === 'log') return 10 ** t;
  if (axis === 'age') return AGE_REF - 10 ** -t;
  return t;
}

export const STOP_FILTERS: StopFilterDef[] = [
  { key: 'peak.ele', ov: 'peak', label: 'Elevation', type: 'range', prop: 'ele', unit: 'm', domain: [0, 6000] },
  { key: 'peak.pr', ov: 'peak', label: 'Prominence', type: 'range', prop: 'pr', unit: 'm', domain: [1, 6000], axis: 'log', help: 'Height above the lowest col to higher ground (tagged, else computed from the terrain)' },
  { key: 'peak.is', ov: 'peak', label: 'Isolation', type: 'range', prop: 'is', unit: 'km', domain: [0.01, 3000], axis: 'log', help: 'Distance to the nearest higher ground' },
  { key: 'waterfall.h', ov: 'waterfall', label: 'Height', type: 'range', prop: 'h', unit: 'm', domain: [0.5, 1000], axis: 'log', help: 'Tagged in OSM or Wikidata for about 1,600 waterfalls' },
  { key: 'lighthouse.h', ov: 'lighthouse', label: 'Tower height', type: 'range', prop: 'h', unit: 'm', domain: [2, 400], axis: 'log' },
  { key: 'lighthouse.fh', ov: 'lighthouse', label: 'Focal height', type: 'range', prop: 'fh', unit: 'm', domain: [1, 400], axis: 'log', help: 'Height of the light above sea level' },
  { key: 'lighthouse.rg', ov: 'lighthouse', label: 'Light range', type: 'range', prop: 'rg', unit: 'nmi', domain: [0, 50], help: 'Nominal range in nautical miles' },
  { key: 'lighthouse.y', ov: 'lighthouse', label: 'First lit', type: 'range', prop: 'y', unit: 'year', domain: [1600, 2025] },
  { key: 'viewpoint.ele', ov: 'viewpoint', label: 'Elevation', type: 'range', prop: 'ele', unit: 'm', domain: [0, 4000] },
  { key: 'viewpoint.pan', ov: 'viewpoint', label: 'Panoramic', type: 'flag', prop: 'pan', help: 'Tagged as looking all round (direction 0–360 or at least 300°)' },
  { key: 'viewpoint.tw', ov: 'viewpoint', label: 'Observation towers', type: 'flag', prop: 'tw' },
  { key: 'covered_bridge.len', ov: 'covered_bridge', label: 'Length', type: 'range', prop: 'len', unit: 'm', domain: [2, 500], axis: 'log' },
  { key: 'covered_bridge.y', ov: 'covered_bridge', label: 'Built', type: 'range', prop: 'y', unit: 'year', domain: [1800, 2025] },
  { key: 'rest.toilets', ov: 'rest', label: 'Toilets', type: 'flag', prop: 'fac', bit: 1 },
  { key: 'rest.water', ov: 'rest', label: 'Drinking water', type: 'flag', prop: 'fac', bit: 2 },
  { key: 'rest.shelter', ov: 'rest', label: 'Shelter', type: 'flag', prop: 'fac', bit: 4 },
  { key: 'rest.tables', ov: 'rest', label: 'Tables or benches', type: 'flag', prop: 'fac', bit: 8 },
  { key: 'rest.bbq', ov: 'rest', label: 'Barbecue', type: 'flag', prop: 'fac', bit: 16 },
  { key: 'trailhead.toilets', ov: 'trailhead', label: 'Toilets', type: 'flag', prop: 'fac', bit: 1 },
  { key: 'trailhead.water', ov: 'trailhead', label: 'Drinking water', type: 'flag', prop: 'fac', bit: 2 },
  { key: 'heritage.by', ov: 'heritage', label: 'Built', type: 'range', prop: 'by', unit: 'year', domain: [-3000, 2020], axis: 'age', help: 'Year built or founded (Wikidata inception)' },
  { key: 'heritage.dy', ov: 'heritage', label: 'Designated', type: 'range', prop: 'dy', unit: 'year', domain: [1900, 2026] },
  { key: 'heritage.wp', ov: 'heritage', label: 'With a Wikipedia article', type: 'flag', prop: 'wp' },
  { key: 'heritageAreas.a', ov: 'heritageAreas', label: 'Area', type: 'range', prop: 'a', unit: 'km²', domain: [0.001, 1000], axis: 'log' },
  { key: 'special.a', ov: 'special', label: 'Area', type: 'range', prop: 'a', unit: 'km²', domain: [1, 30000], axis: 'log' },
  { key: 'indigenous.a', ov: 'indigenous', label: 'Area', type: 'range', prop: 'a', unit: 'km²', domain: [0.001, 100000], axis: 'log' },
];

/** One filter's setting (min / max: 0 or empty = no limit on that side). */
export interface StopFilter {
  on: boolean;
  min: number;
  max: number;
}

export const filtersOf = (ov: OverlayKey) => STOP_FILTERS.filter((f) => f.ov === ov);

/** Bins of a range filter's histogram (over its axis' span). */
export const FILTER_BINS = 128;

/** Histograms of an overlay's range filters over the features given (each one's value, where it has
 * one): each filter's over the features the overlay's other filters keep, so that its own limits
 * show what they leave out. Bins over the axis positions of the filter's domain. */
export function filterHists(ov: OverlayKey, set: Record<string, StopFilter>, keepUnknown: boolean): { add: (p: Record<string, any>) => void; done: () => Record<string, { bins: Float64Array; n: number }> } {
  const defs = filtersOf(ov).filter((d) => d.type === 'range' && d.domain);
  const others = defs.map((d) => stopFilterPass(ov, Object.fromEntries(Object.entries(set).filter(([k]) => k !== d.key)), keepUnknown));
  const hs = defs.map((d) => {
    const ax = d.axis ?? 'lin';
    const a = axisPos(ax, d.domain![0]), b = axisPos(ax, d.domain![1]);
    return { d, ax, a, k: FILTER_BINS / (b - a), bins: new Float64Array(FILTER_BINS), n: 0 };
  });
  return {
    add: (p) => {
      for (let j = 0; j < hs.length; j++) {
        const x = hs[j];
        const v = p[x.d.prop];
        if (typeof v !== 'number' || (others[j] && !others[j]!(p))) continue;
        x.bins[Math.max(0, Math.min(FILTER_BINS - 1, Math.floor((axisPos(x.ax, v) - x.a) * x.k)))]++;
        x.n++;
      }
    },
    done: () => Object.fromEntries(hs.map((x) => [x.d.key, { bins: x.bins, n: x.n }])),
  };
}

/** The active filters of an overlay. */
function active(ov: OverlayKey, set: Record<string, StopFilter>) {
  return filtersOf(ov).filter((d) => set[d.key]?.on && (d.type === 'flag' || set[d.key].min || set[d.key].max));
}

/** MapLibre filter for an overlay (null: none). Features without a value pass range filters when
 * `keepUnknown`; flags must be set. */
export function stopFilterExpr(ov: OverlayKey, set: Record<string, StopFilter>, keepUnknown: boolean): Expr | null {
  const parts: Expr[] = [];
  for (const d of active(ov, set)) {
    const v: Expr = ['get', d.prop];
    if (d.type === 'flag') {
      parts.push(d.bit ? ['==', ['%', ['floor', ['/', ['coalesce', v, 0], d.bit]], 2], 1] : ['==', ['coalesce', v, 0], 1]);
      continue;
    }
    const f = set[d.key];
    const cond: Expr[] = [];
    if (f.min) cond.push(['>=', v, f.min]);
    if (f.max) cond.push(['<=', v, f.max]);
    parts.push(['case', ['==', ['typeof', v], 'number'], ['all', ...cond], keepUnknown]);
  }
  return parts.length ? (['all', ...parts] as Expr) : null;
}

/** The same test in JS (for counts). */
export function stopFilterPass(ov: OverlayKey, set: Record<string, StopFilter>, keepUnknown: boolean): ((p: Record<string, any>) => boolean) | null {
  const act = active(ov, set);
  if (!act.length) return null;
  return (p) => act.every((d) => {
    const v = p[d.prop];
    if (d.type === 'flag') return d.bit ? Math.floor((Number(v) || 0) / d.bit) % 2 === 1 : Number(v) === 1;
    if (typeof v !== 'number') return keepUnknown;
    const f = set[d.key];
    return (!f.min || v >= f.min) && (!f.max || v <= f.max);
  });
}

/** Hash form: "key:min:max" per active filter, joined by "~". */
export function filtersToHash(set: Record<string, StopFilter>): string {
  return STOP_FILTERS.filter((d) => set[d.key]?.on).map((d) => `${d.key}:${set[d.key].min || ''}:${set[d.key].max || ''}`).join('~');
}

export function filtersFromHash(v: string): Record<string, StopFilter> {
  const out: Record<string, StopFilter> = {};
  for (const part of v.split('~')) {
    const [k, a, b] = part.split(':');
    if (STOP_FILTERS.some((d) => d.key === k)) out[k] = { on: true, min: Number(a) || 0, max: Number(b) || 0 };
  }
  return out;
}
