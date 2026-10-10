// Auto-fitting a colour range to the lines in view (roads, rail, ferries): by screen widths of
// line (a fixed amount of line at the view centre's scale) or by percentiles of line length (a
// share of what's in view). Pure; main.ts feeds it the distributions and the view's scale.

/** The unit a colour range auto-fits in: screen widths of line, or percentiles of its length. */
export type FitUnit = 'widths' | 'pct';

/** Each metric's unit, by its key (roads' scenic modes, rail's and ferries' ranked metrics);
 * a metric not in it fits in screen widths. Only percentiles are kept. */
export type FitUnits<K extends string = string> = Partial<Record<K, 'pct'>>;

/** A metric's unit. */
export const unitOf = <K extends string>(u: FitUnits<K>, key: K): FitUnit => (u[key] === 'pct' ? 'pct' : 'widths');

/** The units with one metric's set. */
export function withUnit<K extends string>(u: FitUnits<K>, key: K, unit: FitUnit): FitUnits<K> {
  const out = { ...u };
  if (unit === 'pct') out[key] = 'pct';
  else delete out[key];
  return out;
}

/** Every one of `keys` in percentiles (an older link's or saved setting's unit for the layer). */
export const allPct = <K extends string>(keys: readonly K[]): FitUnits<K> => Object.fromEntries(keys.map((k) => [k, 'pct'])) as FitUnits<K>;

/** A layer's units in a link, `keys` being its metrics that can fit either way: '' all screen
 * widths; 'p' all percentiles (as the per-layer unit's links had it); else the keys in
 * percentiles, in `keys`' order, joined by '.' ("score.view"), or, when that is shorter, '-' and
 * the keys in screen widths ("-drama": all but terrain drama). */
export function unitsField<K extends string>(u: FitUnits<K>, keys: readonly K[]): string {
  const on = keys.filter((k) => u[k] === 'pct'), off = keys.filter((k) => u[k] !== 'pct');
  if (!on.length) return '';
  if (!off.length) return 'p';
  const a = on.join('.'), b = `-${off.join('.')}`;
  return b.length < a.length ? b : a;
}

/** A layer's units from a link's field (unitsField; keys not among `keys` are dropped, and '-'
 * with nothing after it reads as no field). A metric the link's app didn't have (added since)
 * fits by % under 'p' and '-…' (every metric but those named) and by screen widths under a list
 * of names. */
export function unitsOfField<K extends string>(v: string | null | undefined, keys: readonly K[]): FitUnits<K> {
  if (!v) return {};
  if (v === 'p') return allPct(keys);
  const but = v.startsWith('-');
  if (but && v.length === 1) return {}; // '-' alone: malformed
  const named = new Set((but ? v.slice(1) : v).split('.'));
  return Object.fromEntries(keys.filter((k) => named.has(k) !== but).map((k) => [k, 'pct'])) as FitUnits<K>;
}

/** A layer's units from saved settings: its `fitUnits` (keys among `keys`, values 'pct'), else
 * the per-layer `fitUnit` they replaced ('pct': every metric). */
export function unitsOfSaved<K extends string>(o: { fitUnits?: unknown; fitUnit?: unknown } | null | undefined, keys: readonly K[]): FitUnits<K> {
  const u = o?.fitUnits;
  if (u && typeof u === 'object' && !Array.isArray(u)) {
    const r = u as Record<string, unknown>;
    return Object.fromEntries(keys.filter((k) => r[k] === 'pct').map((k) => [k, 'pct'])) as FitUnits<K>;
  }
  return o?.fitUnit === 'pct' ? allPct(keys) : {};
}

/** A pair of numbers kept per metric: a screen-widths fit (the best so many to the best so few)
 * or a percentile fit (low, high). */
export type Pair = [number, number];
/** Pairs by metric key; a metric not in it has the layer's default. */
export type PerMetric<K extends string = string> = Partial<Record<K, Pair>>;

/** A screen-widths fit's default: the best 15 screen widths of line in view to the best 1. */
export const FIT_LEN_DEFAULT: Pair = [15, 1];
/** A valid screen-widths fit: more, then fewer, both above 0. */
export const validFitLen = (p: Pair) => p.every(Number.isFinite) && p[1] > 0 && p[0] > p[1];
/** Valid percentiles: 0 ≤ low < high ≤ 100. */
export const validFit = (p: Pair) => p.every(Number.isFinite) && p[0] >= 0 && p[1] <= 100 && p[1] > p[0];

export const samePair = (a: Pair, b: Pair) => a[0] === b[0] && a[1] === b[1];

/** A metric's pair (a copy), `d` if it has none. */
export const pairOf = <K extends string>(r: PerMetric<K>, key: K, d: Pair): Pair => {
  const v = r[key];
  return v ? [v[0], v[1]] : [d[0], d[1]];
};

/** The pairs with one metric's set (the default is not kept). */
export function withPair<K extends string>(r: PerMetric<K>, key: K, v: Pair, d: Pair): PerMetric<K> {
  const out = { ...r };
  if (samePair(v, d)) delete out[key];
  else out[key] = [v[0], v[1]];
  return out;
}

/** Each of `keys`' pair from `at`, kept where it isn't `d`. */
export function perMetric<K extends string>(keys: readonly K[], at: (k: K) => Pair, d: Pair): PerMetric<K> {
  const out: PerMetric<K> = {};
  for (const k of keys) {
    const v = at(k);
    if (!samePair(v, d)) out[k] = [v[0], v[1]];
  }
  return out;
}

/** A layer's per-metric pairs in a link: a base (the most common pair, `d` when it ties for
 * that, else the first metric's; or `base` if given), which the link's layer-wide field carries
 * as before, and the metrics with another as `key_lo_hi` joined by '/' ("score_10_1/view_20_2"). */
export function pairsField<K extends string>(at: (k: K) => Pair, keys: readonly K[], d: Pair, base?: Pair): { base: Pair; rest: string } {
  let b = base;
  if (!b) {
    const n = new Map<string, number>();
    for (const k of keys) n.set(at(k).join('_'), (n.get(at(k).join('_')) ?? 0) + 1);
    const top = Math.max(0, ...n.values());
    b = (n.get(d.join('_')) ?? 0) === top ? d : keys.map(at).find((v) => n.get(v.join('_')) === top) ?? d;
  }
  const bb = b;
  const rest = keys.filter((k) => !samePair(at(k), bb)).map((k) => `${k}_${+at(k)[0]}_${+at(k)[1]}`).join('/');
  return { base: [bb[0], bb[1]], rest };
}

/** The pairs a link's field lists (pairsField's `rest`): metrics among `keys`, pairs that are `ok`. */
export function pairsOfField<K extends string>(v: string | null | undefined, keys: readonly K[], ok: (p: Pair) => boolean): PerMetric<K> {
  const out: PerMetric<K> = {};
  for (const e of v ? v.split('/') : []) {
    const parts = e.split('_');
    if (parts.length !== 3) continue;
    const [k, a, b] = parts;
    const p: Pair = [Number(a), Number(b)];
    if (a !== '' && b !== '' && keys.includes(k as K) && ok(p)) out[k as K] = p;
  }
  return out;
}

/** A layer's screen-widths fits from saved settings: its `fitLens` (keys among `keys`, valid
 * pairs), else the per-layer `fitLen` they replaced, for each of its metrics. */
export function fitLensOfSaved<K extends string>(o: { fitLens?: unknown; fitLen?: unknown } | null | undefined, keys: readonly K[]): PerMetric<K> {
  const pair = (v: unknown): v is Pair => Array.isArray(v) && v.length === 2 && v.every((x) => typeof x === 'number') && validFitLen(v as Pair);
  const r = o?.fitLens;
  if (r && typeof r === 'object' && !Array.isArray(r)) {
    const m = r as Record<string, unknown>;
    return perMetric(keys, (k) => (pair(m[k]) ? (m[k] as Pair) : FIT_LEN_DEFAULT), FIT_LEN_DEFAULT);
  }
  const old = o?.fitLen;
  return pair(old) ? perMetric(keys, () => old, FIT_LEN_DEFAULT) : {};
}

/** A distribution of line length over a metric (roads/stats.ts Dist). */
export interface LengthDist {
  /** Total length (any unit). */
  total: number;
  /** The metric's value at cumulative length share p, 0..1. */
  quantile(p: number): number;
}

/** [lo, hi], widened about its middle to at least `min`. */
export function spread(lo: number, hi: number, min: number): [number, number] {
  if (hi - lo < min) {
    const m = (lo + hi) / 2;
    return [m - min / 2, m + min / 2];
  }
  return [lo, hi];
}

/** Km of ground a view `px` CSS pixels wide spans at latitude `lat` and zoom `zoom` (512-px tiles). */
export const screenWidthKm = (lat: number, zoom: number, px: number) =>
  ((40075.016686 * Math.cos((lat * Math.PI) / 180)) / (512 * 2 ** zoom)) * px;

/** The best `widths[0]` screen widths of line in view to the best `widths[1]`: the low end where
 * the top-ranked lines reach that length, full colour from the second. `km`: the length in view;
 * `widthKm`: one screen width. */
export function fitByWidths(dist: LengthDist, km: number, widthKm: number, widths: [number, number], step: number): [number, number] {
  const at = (w: number) => dist.quantile(Math.max(0, 1 - (w * widthKm) / km));
  return spread(at(widths[0]), at(widths[1]), step * 4);
}

/** Percentiles of line length in view (`pct`: low, high, 0–100): the low end at the length-weighted
 * pct[0]-th percentile, full colour from the pct[1]-th. */
export function fitByPct(dist: LengthDist, pct: [number, number], step: number): [number, number] {
  return spread(dist.quantile(pct[0] / 100), dist.quantile(pct[1] / 100), step * 4);
}

/** Percentiles as "the best n %" (and back): the best 20 % begins at the 80th percentile. */
export const bestOfPct = (p: number) => +(100 - p).toFixed(2);
export const pctOfBest = (b: number) => +(100 - b).toFixed(2);

/** "The best [lo] … full colour from the best [hi]" in percent, edited one at a time: the low end
 * 0.5–100, the top 0–99.5, at least 0.5 apart (as the percentiles' own limits, state.ts), to 0.1;
 * the edited one keeps its value and pushes the other along. Returns the percentiles (low, high). */
export function setBestPct(pct: [number, number], i: 0 | 1, best: number): [number, number] {
  const b: [number, number] = [bestOfPct(pct[0]), bestOfPct(pct[1])];
  const v = Math.round(Math.min(i ? 99.5 : 100, Math.max(i ? 0 : 0.5, Number.isFinite(best) ? best : 0)) * 10) / 10;
  b[i] = v;
  if (b[0] - b[1] < 0.5) b[1 - i] = +(i ? v + 0.5 : v - 0.5).toFixed(1);
  return [pctOfBest(b[0]), pctOfBest(b[1])];
}
