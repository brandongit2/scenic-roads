// Data available (docs/steps.md §14): the layers colouring each place by the best data any declared
// source has there, per theme (`terrain`) and set (`best`). The server picks the piece drawn at each
// pixel and sends its value (a tile of values) and answers the click; the app colours the values
// through the sidebar's colour-map control (ui/scale.ts), as the terrain tint's. This module is
// everything the app does with them that isn't drawing: the themes the catalogue lists, the
// server's description of a theme read into its layers, each layer's axis in its tiles' values,
// the colour ramp's stops, the tiles' and the click's URLs, the click's rows and the settings.
// Nothing here knows the map engine or the page (availablemap.ts draws, ui/available.ts shows),
// and nothing is written per metric: labels, units and scales are the server's
// (`/api/available/<theme>`), so a new metric changes no code here.
import type { ScaleFields, ThresholdDir } from './state';

/** The availability switch: only the data that costs nothing (`free`: it downloads freely, or
 * after a login or a request), or every piece. */
export type Avail = 'free' | 'all';

/** A switch's value from a save or a link: `free` or `all`, else `d`; `open`, as saves and links
 * made before free data took in the login pieces say, is `free`. */
const availOf = (v: unknown, d: Avail): Avail => (v === 'all' ? 'all' : v === 'free' || v === 'open' ? 'free' : d);

/** The group's settings (a profile's, as every layer's: web/default-profile.json has them). */
export interface AvailableState {
  on: boolean;
  /** The theme and its layer shown (one at a time): a layer the theme doesn't list shows its first. */
  theme: string;
  layer: string;
  /** The set: `best`, the best data anywhere (a set the theme doesn't list: its first). */
  set: string;
  avail: Avail;
  opacity: number;
  /** A layer's colour map the first time it's shown (`scaleOf`; its palette reversed where the
   * layer's better end is its low one, so the best data takes the same end of the ramp). */
  colours: ScaleFields;
  /** Each layer's own colour map, by layer id, once changed. */
  scales: Record<string, ScaleFields>;
}

/** What a layer's tiles carry (the server's `code`): log10 of the value on a `log` scale, the value
 * on a `linear` one, a word's value on a `values` one. */
export type ScaleKind = 'log' | 'linear' | 'values';

export interface AvLayer {
  id: string;
  label: string;
  unit: string | null;
  scale: ScaleKind;
  /** The better end: the low values (a finer spacing, a better accuracy) or the high ones. */
  best: 'low' | 'high';
  /** The least and greatest value present, in its unit (null: none). */
  range: [number, number] | null;
  /** A `values` layer's words, by value. */
  words: { value: number; label: string }[];
}
export interface AvMetric {
  id: string;
  label: string;
  unit: string | null;
}
/** A theme as the server describes it: its sets, its metrics (the click's rows) and its layers
 * (the score's first: the best piece at its own grid spacing; then the other metrics'), and the
 * words it shows for the sets' ids, the pieces' availability and extent classes (`labels`; an id
 * the server gives no word for is shown as it is). */
export interface AvTheme {
  theme: string;
  sets: string[];
  metrics: AvMetric[];
  layers: AvLayer[];
  labels: { sets: Record<string, string>; availability: Record<string, string>; extent: Record<string, string> };
}

/** A piece at a point, as the click answers it (the server's `pieces`, best first; `painted`, the
 * layers it is the piece drawn of there: the server's own pick, the app ranks nothing). */
export interface AvPiece {
  id: string;
  source: string;
  name: string | null;
  source_name: string | null;
  licence: string | null;
  licence_url: string | null;
  link: string | null;
  availability: string | null;
  extent: string | null;
  read: Record<string, unknown> | null;
  metrics: Record<string, unknown>;
  stated: Record<string, unknown> | null;
  values: Record<string, number | null>;
  score: number | null;
  painted: string[];
}

const str = (v: unknown): string | null => (typeof v === 'string' ? v : null);
const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);
const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/** A theme the catalogue lists: its tiles' finest zoom, and each pole's where it has one. */
export interface ThemeLayers {
  theme: string;
  maxzoom: number;
  poles: Partial<Record<'n' | 's', number>>;
}

/** The themes the catalogue's layers list (`available-<theme>`), in its order; with each pole's
 * layer (`available-<theme>-<n|s>`) and its finest zoom where it has one. */
export function themesOf(layers: Record<string, unknown> | undefined): ThemeLayers[] {
  const names = Object.keys(layers ?? {});
  return names
    .map((n) => /^available-([a-z]+)$/.exec(n)?.[1])
    .filter((t): t is string => !!t)
    .map((theme) => {
      const poles: Partial<Record<'n' | 's', number>> = {};
      for (const p of ['n', 's'] as const) {
        const l = obj(layers?.[`available-${theme}-${p}`]);
        if (l) poles[p] = num(l.maxzoom) ?? 0;
      }
      return { theme, maxzoom: num(obj(layers?.[`available-${theme}`])?.maxzoom) ?? 12, poles };
    });
}

/** A layer from the server's, null when it isn't one the app can draw (a scale it doesn't know, a
 * log range not above 0, a `values` one without words). */
function layerOf(l: Record<string, unknown>): AvLayer | null {
  if (typeof l.id !== 'string' || (l.scale !== 'log' && l.scale !== 'linear' && l.scale !== 'values')) return null;
  const r = arr(l.range).map(num);
  const range = r.length === 2 && r[0] !== null && r[1] !== null && r[1] >= r[0] ? ([r[0], r[1]] as [number, number]) : null;
  if (range && l.scale === 'log' && range[0] <= 0) return null;
  const words = arr(l.words).map(obj).filter((w): w is Record<string, unknown> => !!w && num(w.value) !== null && typeof w.label === 'string')
    .map((w) => ({ value: w.value as number, label: w.label as string })).sort((a, b) => a.value - b.value);
  if (l.scale === 'values' && words.length < 2) return null;
  return { id: l.id, label: str(l.label) ?? l.id, unit: str(l.unit), scale: l.scale, best: l.best === 'low' ? 'low' : 'high', range, words };
}

/** `/api/available/<theme>`'s answer read: null when it isn't one (or holds no layer: a theme with
 * no score, or a server of another format). A layer the app can't draw is left out. */
export function parseTheme(j: unknown): AvTheme | null {
  const o = obj(j);
  if (!o || typeof o.theme !== 'string') return null;
  const metrics = arr(o.metrics).map(obj).filter((m): m is Record<string, unknown> => !!m && typeof m.id === 'string')
    .map((m) => ({ id: m.id as string, label: str(m.label) ?? (m.id as string), unit: str(m.unit) }));
  const layers = arr(o.layers).map(obj).map((l) => (l ? layerOf(l) : null)).filter((l): l is AvLayer => !!l);
  if (!layers.length) return null;
  const lb = obj(o.labels);
  const words = (v: unknown) => Object.fromEntries(Object.entries(obj(v) ?? {}).filter((e): e is [string, string] => typeof e[1] === 'string'));
  return {
    theme: o.theme, sets: arr(o.sets).filter((x): x is string => typeof x === 'string'), metrics, layers,
    labels: { sets: words(lb?.sets), availability: words(lb?.availability), extent: words(lb?.extent) },
  };
}

/** The layer and set shown of a theme: the state's where the theme lists them, else its first. */
export function shown(s: AvailableState, t: AvTheme): { layer: AvLayer; set: string } {
  const layer = t.layers.find((l) => l.id === s.layer) ?? t.layers[0];
  return { layer, set: t.sets.includes(s.set) ? s.set : t.sets[0] ?? s.set };
}

/** An id's word in the server's `labels` (none: the id as it is). */
export const word = (words: Record<string, string>, id: string) => (Object.hasOwn(words, id) ? words[id] : id);

// ---- a layer's axis: the colour-map control's metric, in the tiles' values --------------------

/** A layer's axis in its tiles' values, as the colour-map control takes it (ui/scale.ts
 * ScaleMetric): its whole domain (the values present), the step of its handles and highlight, the
 * labels in the layer's own unit, and the ticks where they're round in that unit. A log layer's
 * values are log10 of its unit's, so the control's even axis is a log one (0.5 m, 1 m, 2 m, 5 m
 * …); a `values` layer's run between its words, its range fixed to them (`fixed`): the control has
 * no categories, so the two words of the terrain's kind are the ramp's two ends. */
export interface Axis {
  domain: [number, number];
  step: number;
  fmt: (v: number) => string;
  ticks?: (lo: number, hi: number) => [number, number][];
  fixed: boolean;
}

/** A number to two significant digits ("0.61", "10", "1300"). */
const short = (v: number) => String(+v.toPrecision(2));
const inUnit = (v: string, unit: string | null) => (unit ? `${v} ${unit}` : v);

/** Round steps of a log axis between `lo` and `hi` (log10): 1, 5 and 2 times the powers of ten, in
 * that order of rank (scaleaxis.ts axisTicks labels the lower ranks first). */
export function logTicks(lo: number, hi: number): [number, number][] {
  const out: [number, number][] = [];
  for (let k = Math.floor(lo) - 1; k <= Math.ceil(hi); k++) {
    for (const [m, rank] of [[1, 0], [5, 1], [2, 2]]) {
      const v = Math.log10(m) + k;
      if (v >= lo - 1e-9 && v <= hi + 1e-9) out.push([v, rank]);
    }
  }
  return out;
}

/** A step a little under a hundredth of a span, rounded (1, 2 or 5 times a power of ten). */
const stepOf = (span: number) => {
  const raw = span / 100, p = 10 ** Math.floor(Math.log10(raw)), r = raw / p;
  return (r < 2 ? 1 : r < 5 ? 2 : 5) * p;
};

export function axisOf(l: AvLayer): Axis {
  if (l.scale === 'values') {
    const lo = l.words[0].value, hi = l.words[l.words.length - 1].value;
    const nearest = (v: number) => l.words.reduce((a, b) => (Math.abs(b.value - v) < Math.abs(a.value - v) ? b : a));
    // (The step from word to word where they're evenly spaced, so the highlight's slider stops on
    // the words alone: bare earth or surface.)
    const gap = Math.min(...l.words.slice(1).map((w, i) => w.value - l.words[i].value));
    const even = gap > 0 && l.words.every((w) => Math.abs((w.value - lo) / gap - Math.round((w.value - lo) / gap)) < 1e-9);
    return {
      domain: [lo, hi], step: even ? gap : stepOf(hi - lo), fixed: true,
      fmt: (v) => (Math.abs(nearest(v).value - v) < (hi - lo) / 50 ? nearest(v).label : `${+v.toFixed(2)}`),
      ticks: () => l.words.map((w) => [w.value, 0] as [number, number]),
    };
  }
  if (l.scale === 'log') {
    const [a, b] = l.range ? l.range.map(Math.log10) : [-1, 3];
    // (At least a factor of two wide, about the middle, and a 1/256th past the ends: the tiles'
    // values are rounded to it.)
    const m = (a + b) / 2, half = Math.max((b - a) / 2, 0.15) + Q;
    return { domain: [m - half, m + half], step: 0.01, fixed: false, fmt: (v) => inUnit(short(10 ** v), l.unit), ticks: logTicks };
  }
  const [a, b] = l.range ?? [0, 1];
  const [lo, hi] = b > a ? [a, b] : [a - 1, b + 1];
  const whole = Number.isInteger(lo) && Number.isInteger(hi);
  const step = whole ? Math.max(1, stepOf(hi - lo)) : stepOf(hi - lo);
  const dec = Math.max(0, -Math.floor(Math.log10(step)));
  return { domain: [lo, hi], step, fixed: false, fmt: (v) => inUnit(v.toFixed(dec), l.unit) };
}

/** A layer's colour map: its own once changed, else the profile's first one, its palette reversed
 * where the layer's better end is its low one (`colours`). The highlight's value is on the layer's
 * axis: one off it (the profile's, shared by layers of other units: a year's 0) is the axis's
 * middle. */
export function scaleOf(s: AvailableState, l: AvLayer): ScaleFields {
  const own = s.scales[l.id];
  const p = s.colours.palette, rev = p.endsWith('_r');
  const sc = own ?? { ...s.colours, palette: l.best === 'low' ? (rev ? p.slice(0, -2) : `${p}_r`) : p };
  const { domain: [lo, hi], step } = axisOf(l), t = sc.threshold;
  if (t.value >= lo && t.value <= hi) return sc;
  return { ...sc, threshold: { ...t, value: lo + Math.round((hi - lo) / 2 / step) * step } };
}

// ---- the colour ramp on the map ----------------------------------------------------------------

/** A pixel with no value (all 0: Terrarium's −32,768), and the tiles' precision (1/256). */
export const NONE = -32768;
const Q = 1 / 256;
const snap = (v: number) => Math.round(v / Q) * Q;

/** The colour ramp's stops over the tiles' values, [value, colour], ascending: none transparent,
 * then `colourAt` (an rgba() colour) at even steps over the range (more when equalized: the lookup
 * bends the ramp), clamped outside it; with the highlight on, the values it leaves out
 * transparent, its edge snapped to the tiles' precision and a step of it wide (values are whole
 * 1/256ths, so none falls inside the edge). For a colour-relief layer's `interpolate`
 * (availablemap.ts). */
export function rampStops(range: [number, number], equalized: boolean, colourAt: (v: number) => string,
  threshold: { on: boolean; dir: ThresholdDir; value: number }): [number, string][] {
  const clear = 'rgba(0,0,0,0)';
  const [lo, hi] = range;
  const e = snap(threshold.dir === 'low' ? lo : threshold.value);
  const shows = (v: number) => !threshold.on || (threshold.dir === 'below' ? v <= e + Q / 2 : v >= e - Q / 2);
  const at = (v: number) => (shows(v) ? colourAt(v) : clear);
  const n = equalized ? 64 : 32;
  // (Each at a whole 1/256th: the map packs the stops as it does the tiles, so two closer would be
  // one.)
  const vs = Array.from({ length: n + 1 }, (_, i) => snap(lo + ((hi - lo) * i) / n));
  // (Below: the edge's own value shows; above: the one under it doesn't.)
  if (threshold.on) vs.push(...(threshold.dir === 'below' ? [e, e + Q] : [e - Q, e]));
  const sorted = [...new Set(vs.map((v) => +v.toFixed(8)))].sort((a, b) => a - b);
  return [[NONE, clear], [NONE + 1, at(sorted[0])], ...sorted.filter((v) => v > NONE + 1).map((v) => [v, at(v)] as [number, string])];
}

// ---- URLs ------------------------------------------------------------------------------------------

/** Ids as a URL's path segments (the server's ids, escaped). */
const path = (...ids: string[]) => ids.map(encodeURIComponent).join('/');

/** The Mercator tiles' URL template (`{z}/{x}/{y}`), from `origin` (the data host), at version `v`
 * (meta's `available-<theme>`). */
export const tileUrl = (origin: string, theme: string, set: string, layer: string, avail: Avail, v: string) =>
  `${origin}/tiles/available/${path(theme, set, layer)}/{z}/{x}/{y}?avail=${avail}${v ? `&v=${encodeURIComponent(v)}` : ''}`;

/** A polar tile's URL (z0–3 of the pole's grid), at version `v` (meta's `available-<theme>-<pole>`). */
export const polarTileUrl = (origin: string, pole: 'n' | 's', theme: string, set: string, layer: string, avail: Avail, v: string, z: number, x: number, y: number) =>
  `${origin}/tiles/polar-available/${pole}/${path(theme, set, layer)}/${z}/${x}/${y}?avail=${avail}${v ? `&v=${encodeURIComponent(v)}` : ''}`;

/** The click's URL: the pieces at a point. */
export const atUrl = (theme: string, set: string, lng: number, lat: number, avail: Avail) =>
  `/api/available/${path(theme, set)}?lng=${lng.toFixed(6)}&lat=${lat.toFixed(6)}&avail=${avail}`;

// ---- the click -----------------------------------------------------------------------------------

/** A value in a metric's unit ("0.5 m"; none: as it is). */
const withUnit = (v: string, unit: string | null) => (unit ? `${v} ${unit}` : v);

/** A value as the click shows it: a number to at most three decimals, else as it is. */
function shownValue(v: unknown): string | null {
  if (typeof v === 'number' && Number.isFinite(v)) return String(+v.toFixed(3));
  if (typeof v === 'string' && v) return v;
  return null;
}

/** A length in metres, short ("50 m", "111 km"). */
const metres = (m: number) => (m < 1000 ? `${+m.toPrecision(3)} m` : `${+(m / 1000).toPrecision(3)} km`);

/** A piece's row in the click's panel. */
export interface PieceRow {
  title: string;
  /** Its own name where the source has several (a project's). */
  sub: string;
  /** What the layer shown draws here. */
  painted: boolean;
  score: string;
  /** The metrics it states, in the theme's order: label and value (with its unit, and the
   * publisher's class where that's what was published). */
  metrics: [string, string][];
  availability: string;
  extent: string;
  /** What the extent was read from, how fine and when ("wfs · to 50 m · 2026-11-13"). */
  read: string;
  licence: { text: string; url: string | null } | null;
  link: string | null;
}

export function pieceRows(pieces: AvPiece[], layer: string, t: AvTheme): PieceRow[] {
  return pieces.map((p) => {
    const metrics: [string, string][] = [];
    for (const m of t.metrics) {
      const v = shownValue(p.metrics[m.id]);
      if (v === null) continue;
      const st = shownValue(p.stated?.[m.id]);
      metrics.push([m.label, `${withUnit(v, typeof p.metrics[m.id] === 'number' ? m.unit : null)}${st && st !== v ? ` (published as ${st})` : ''}`]);
    }
    const r = p.read ?? {};
    const grain = num(r.grain_m), tol = num(r.tolerance_m);
    const read = [str(r.method), grain ? `cells of ${metres(grain)}` : tol ? `to ${metres(tol)}` : null, str(r.date)].filter(Boolean).join(' · ');
    const title = p.source_name ?? p.source;
    return {
      title,
      sub: p.name && p.name !== title ? p.name : '',
      painted: p.painted.includes(layer),
      score: p.score === null ? '' : p.score.toFixed(2),
      metrics,
      availability: p.availability ? word(t.labels.availability, p.availability) : '',
      extent: p.extent ? word(t.labels.extent, p.extent) : '',
      read,
      licence: p.licence ? { text: p.licence, url: p.licence_url } : null,
      link: p.link,
    };
  });
}

/** The click's answer read (its pieces, each with the layers it is drawn of), null when it isn't
 * one. */
export function parsePieces(j: unknown): AvPiece[] | null {
  const o = obj(j);
  if (!o || !Array.isArray(o.pieces)) return null;
  const painted = Object.entries(obj(o.painted) ?? {});
  return o.pieces.map(obj).filter((p): p is Record<string, unknown> => !!p && typeof p.id === 'string').map((p) => ({
    id: p.id as string,
    source: str(p.source) ?? '',
    name: str(p.name),
    source_name: str(p.source_name),
    licence: str(p.licence),
    licence_url: str(p.licence_url),
    link: str(p.link),
    availability: str(p.availability),
    extent: str(p.extent),
    read: obj(p.read),
    metrics: obj(p.metrics) ?? {},
    stated: obj(p.stated),
    values: Object.fromEntries(Object.entries(obj(p.values) ?? {}).map(([k, v]) => [k, num(v)])),
    score: num(p.score),
    painted: painted.filter(([, id]) => id === p.id).map(([l]) => l),
  }));
}

// ---- the settings ---------------------------------------------------------------------------------

/** The settings a link carries (`av=`): on, theme, layer, set, switch, opacity (the colour maps are
 * a profile's). */
export const toField = (s: AvailableState) => [s.on ? 1 : 0, s.theme, s.layer, s.set, s.avail, +s.opacity.toFixed(2)].join(',');

/** A link's `av=` over the settings it changes (`base`), base where it isn't one. */
export function fromField(v: string | null | undefined, base: AvailableState): AvailableState {
  const f = v?.split(',');
  if (!f || f.length !== 6) return base;
  const id = (x: string, d: string) => (/^[a-z0-9_]+$/.test(x) ? x : d);
  const op = Number(f[5]);
  return {
    ...base,
    on: f[0] === '1',
    theme: id(f[1], base.theme),
    layer: id(f[2], base.layer),
    set: id(f[3], base.set),
    avail: availOf(f[4], base.avail),
    opacity: f[5] !== '' && Number.isFinite(op) ? clampOpacity(op) : base.opacity,
  };
}

/** The overlay's opacity range. */
export const OPACITY_RANGE: [number, number] = [0.1, 1];
const clampOpacity = (v: number) => Math.min(OPACITY_RANGE[1], Math.max(OPACITY_RANGE[0], v));

const DIRS: ThresholdDir[] = ['above', 'below', 'low'];
const pairOf = (v: unknown): [number, number] | null => {
  const p = arr(v).map(num);
  return p.length === 2 && p[0] !== null && p[1] !== null ? [p[0], p[1]] : null;
};

/** A colour map read from a save, each field valid (else `base`'s). */
export function cleanScale(x: unknown, base: ScaleFields): ScaleFields {
  const o = obj(x) ?? {};
  const range = pairOf(o.range), fit = pairOf(o.fit), t = obj(o.threshold) ?? {};
  const clamp = (v: unknown, lo: number, hi: number, d: number) => Math.min(hi, Math.max(lo, num(v) ?? d));
  return {
    palette: typeof o.palette === 'string' && o.palette ? o.palette : base.palette,
    auto: typeof o.auto === 'boolean' ? o.auto : base.auto,
    range: range && range[1] > range[0] ? range : [...base.range],
    fit: fit && fit[0] >= 0 && fit[1] <= 100 && fit[1] > fit[0] ? fit : [...base.fit],
    equalize: typeof o.equalize === 'boolean' ? o.equalize : base.equalize,
    lowFade: clamp(o.lowFade, 0, 1, base.lowFade),
    lowSpan: clamp(o.lowSpan, 0.1, 1, base.lowSpan),
    threshold: {
      on: typeof t.on === 'boolean' ? t.on : base.threshold.on,
      dir: DIRS.includes(t.dir as ThresholdDir) ? (t.dir as ThresholdDir) : base.threshold.dir,
      value: num(t.value) ?? base.threshold.value,
    },
  };
}

/** Settings read from a save or a profile, each one valid (else `base`'s). */
export function cleanState(s: AvailableState, base: AvailableState): AvailableState {
  const id = (x: unknown, d: string) => (typeof x === 'string' && /^[a-z0-9_]+$/.test(x) ? x : d);
  const colours = cleanScale(s.colours, base.colours);
  const scales = Object.fromEntries(Object.entries(obj(s.scales) ?? {}).filter(([k]) => /^[a-z0-9_]+$/.test(k)).map(([k, v]) => [k, cleanScale(v, colours)]));
  return {
    on: typeof s.on === 'boolean' ? s.on : base.on,
    theme: id(s.theme, base.theme),
    layer: id(s.layer, base.layer),
    set: id(s.set, base.set),
    avail: availOf(s.avail, base.avail),
    opacity: Number.isFinite(s.opacity) ? clampOpacity(s.opacity) : base.opacity,
    colours,
    scales,
  };
}
