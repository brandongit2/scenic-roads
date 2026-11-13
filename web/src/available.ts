// Data available (docs/steps.md §14): the layers colouring each place by the best data any declared
// source has there, per theme (`terrain`) and set (`best`). The server paints the tiles and answers
// the click; this module is everything the app does with them that isn't drawing: the themes the
// catalogue lists, the server's description of a theme read into its layers and legends, the
// tiles' and the click's URLs, the legend's rows and the click's rows. Nothing here knows the map
// engine or the page (availablemap.ts draws, ui/available.ts shows), and nothing is written per
// metric: labels, units, legend kinds and colours are the server's (`/api/available/<theme>`), so
// a new metric or a retuned palette changes no code here.

/** The availability switch: only data that downloads freely (`open`), or every piece. */
export type Avail = 'open' | 'all';

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
}

/** A legend's stop: a class (`upto` its upper edge in the metric's unit, null for the last; the
 * score's with `shaded`, its colour at the worst tie-break), a word, or a place on a ramp. */
export interface ClassStop {
  label: string;
  upto: number | null;
  colour: string;
  shaded?: string;
}
export interface ValueStop {
  value: number;
  label: string;
  colour: string;
}
export type Legend =
  | { kind: 'classes'; stops: ClassStop[]; shadedBy: string[] }
  | { kind: 'values'; stops: ValueStop[] }
  | { kind: 'ramp'; ramp: [number, string][]; stops: ValueStop[] };

export interface AvLayer {
  id: string;
  label: string;
  unit: string | null;
  legend: Legend;
}
export interface AvMetric {
  id: string;
  label: string;
  unit: string | null;
  primary: boolean;
}
/** A theme as the server describes it: its sets, metrics and layers (the score's first). */
export interface AvTheme {
  theme: string;
  sets: string[];
  metrics: AvMetric[];
  layers: AvLayer[];
}

/** A piece at a point, as the click answers it (the server's `pieces`, best first). */
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
}

/** The score layer's id (the server's `SCORE_LAYER`). */
export const SCORE = 'score';

const HEX = /^#[0-9a-f]{6}$/i;
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

/** A layer's legend from the server's layer, null when it isn't one the app can draw. */
function legendOf(l: Record<string, unknown>): Legend | null {
  const stops = arr(l.stops).map(obj).filter((s): s is Record<string, unknown> => !!s);
  switch (l.legend) {
    case 'classes': {
      const st = stops.map((s) => ({ label: str(s.label) ?? '', upto: num(s.upto), colour: str(s.colour) ?? '', ...(HEX.test(str(s.shaded) ?? '') ? { shaded: s.shaded as string } : {}) }));
      if (!st.length || st.some((s) => !HEX.test(s.colour))) return null;
      return { kind: 'classes', stops: st, shadedBy: arr(l.shaded_by).filter((x): x is string => typeof x === 'string') };
    }
    case 'values':
    case 'ramp': {
      const st = stops.map((s) => ({ value: num(s.value) ?? NaN, label: str(s.label) ?? '', colour: str(s.colour) ?? '' }));
      if (!st.length || st.some((s) => !Number.isFinite(s.value) || !HEX.test(s.colour))) return null;
      if (l.legend === 'values') return { kind: 'values', stops: st };
      const ramp = arr(l.ramp).filter((r): r is [number, string] => Array.isArray(r) && num(r[0]) !== null && HEX.test(str(r[1]) ?? ''));
      return ramp.length >= 2 ? { kind: 'ramp', ramp, stops: st } : null;
    }
    default:
      return null;
  }
}

/** `/api/available/<theme>`'s answer read: null when it isn't one (or holds no layer: a theme with
 * no score). A layer whose legend can't be drawn is left out. */
export function parseTheme(j: unknown): AvTheme | null {
  const o = obj(j);
  if (!o || typeof o.theme !== 'string') return null;
  const metrics = arr(o.metrics).map(obj).filter((m): m is Record<string, unknown> => !!m && typeof m.id === 'string')
    .map((m) => ({ id: m.id as string, label: str(m.label) ?? (m.id as string), unit: str(m.unit), primary: m.primary === true }));
  const layers: AvLayer[] = [];
  for (const l of arr(o.layers).map(obj)) {
    if (!l || typeof l.id !== 'string') continue;
    const legend = legendOf(l);
    if (legend) layers.push({ id: l.id, label: str(l.label) ?? l.id, unit: str(l.unit), legend });
  }
  if (!layers.length) return null;
  return { theme: o.theme, sets: arr(o.sets).filter((x): x is string => typeof x === 'string'), metrics, layers };
}

/** The layer and set shown of a theme: the state's where the theme lists them, else its first. */
export function shown(s: AvailableState, t: AvTheme): { layer: AvLayer; set: string } {
  const layer = t.layers.find((l) => l.id === s.layer) ?? t.layers[0];
  return { layer, set: t.sets.includes(s.set) ? s.set : t.sets[0] ?? 'best' };
}

/** A set's name in the panel (the server lists its ids). */
const SET_LABELS: Record<string, string> = { best: 'Best available anywhere', built: 'Built into the map' };
export const setLabel = (id: string) => SET_LABELS[id] ?? id;

/** The Mercator tiles' URL template (`{z}/{x}/{y}`), from `origin` (the data host), at version `v`
 * (meta's `available-<theme>`). */
export const tileUrl = (origin: string, theme: string, set: string, layer: string, avail: Avail, v: string) =>
  `${origin}/tiles/available/${theme}/${set}/${layer}/{z}/{x}/{y}?avail=${avail}${v ? `&v=${v}` : ''}`;

/** A polar tile's URL (z0–3 of the pole's grid), at version `v` (meta's `available-<theme>-<pole>`). */
export const polarTileUrl = (origin: string, pole: 'n' | 's', theme: string, set: string, layer: string, avail: Avail, v: string, z: number, x: number, y: number) =>
  `${origin}/tiles/polar-available/${pole}/${theme}/${set}/${layer}/${z}/${x}/${y}?avail=${avail}${v ? `&v=${v}` : ''}`;

/** The click's URL: the pieces at a point. */
export const atUrl = (theme: string, set: string, lng: number, lat: number, avail: Avail) =>
  `/api/available/${theme}/${set}?lng=${lng.toFixed(6)}&lat=${lat.toFixed(6)}&avail=${avail}`;

/** The legend as drawn: swatches with labels (a class's or a word's; the score's classes each
 * from its own colour to its shaded one, `caption` saying what shades it), or a ramp with ticks. */
export type LegendModel =
  | { kind: 'swatches'; rows: { colour: string; to?: string; label: string }[]; caption: string }
  | { kind: 'ramp'; gradient: string; ticks: string[] };

/** A value in a metric's unit ("0.5 m"; none: as it is). */
const withUnit = (v: string, unit: string | null) => (unit ? `${v} ${unit}` : v);

/** "a, b and c". */
const listed = (xs: string[]) => (xs.length < 2 ? xs.join('') : `${xs.slice(0, -1).join(', ')} and ${xs[xs.length - 1]}`);

export function legendModel(l: AvLayer, t: AvTheme): LegendModel {
  const g = l.legend;
  if (g.kind === 'ramp') {
    return {
      kind: 'ramp',
      gradient: `linear-gradient(90deg, ${g.ramp.map(([at, c]) => `${c} ${+(at * 100).toFixed(2)}%`).join(', ')})`,
      ticks: g.stops.map((s) => withUnit(s.label, l.id === SCORE ? null : l.unit)),
    };
  }
  if (g.kind === 'values') return { kind: 'swatches', rows: g.stops.map((s) => ({ colour: s.colour, label: s.label })), caption: '' };
  const shaded = g.stops.some((s) => s.shaded);
  const by = g.shadedBy.map((id) => t.metrics.find((m) => m.id === id)?.label ?? id);
  return {
    kind: 'swatches',
    rows: g.stops.map((s) => ({ colour: s.colour, ...(s.shaded ? { to: s.shaded } : {}), label: s.label })),
    caption: shaded && by.length ? `Each class from its best (left) to its worst (right) ${listed(by.map((x) => x.charAt(0).toLowerCase() + x.slice(1)))}` : '',
  };
}

/** The index of the piece a layer paints among those at a point (the server's Theme::best): the
 * best score, or the best 0–1 value of the metric (a piece not stating it takes no part; of equal
 * values, the first id); -1 where none does. */
export function paintedIndex(pieces: AvPiece[], layer: string): number {
  let best = -1;
  for (let i = 0; i < pieces.length; i++) {
    const v = layer === SCORE ? pieces[i].score : pieces[i].values[layer];
    if (typeof v !== 'number') continue;
    const b = best < 0 ? null : layer === SCORE ? pieces[best].score! : pieces[best].values[layer]!;
    if (b === null || v > b || (v === b && pieces[i].id < pieces[best].id)) best = i;
  }
  return best;
}

/** A piece's availability in plain words (docs/inputs.md §6.11). */
const AVAILABILITY: Record<string, string> = {
  open: 'downloads freely',
  login: 'free after a login or a request',
  paid: 'paid',
  closed: 'exists, not to be had',
};
/** How a piece's extent was found, in plain words (docs/inputs.md §6.11's extent classes). */
const EXTENT: Record<string, string> = {
  mask: 'where the map’s own copy has data',
  files: 'from the files’ own headers',
  index: 'from the publisher’s tile index',
  service: 'from the publisher’s footprint service',
  claimed: 'claimed area, not checked against the data',
  unknown: 'not known',
};

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
  /** What the layer shown paints here. */
  painted: boolean;
  score: string;
  /** The metrics it states, in the theme's order: label and value (with its unit, and the
   * publisher's class where that's what was published). */
  metrics: [string, string][];
  availability: string;
  open: boolean;
  extent: string;
  /** What the extent was read from, how fine and when ("wfs · to 50 m · 2026-11-13"). */
  read: string;
  licence: { text: string; url: string | null } | null;
  link: string | null;
}

export function pieceRows(pieces: AvPiece[], layer: string, t: AvTheme): PieceRow[] {
  const painted = paintedIndex(pieces, layer);
  return pieces.map((p, i) => {
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
      painted: i === painted,
      score: p.score === null ? '' : p.score.toFixed(2),
      metrics,
      availability: p.availability ? AVAILABILITY[p.availability] ?? p.availability : '',
      open: p.availability === 'open',
      extent: p.extent ? EXTENT[p.extent] ?? p.extent : '',
      read,
      licence: p.licence ? { text: p.licence, url: p.licence_url } : null,
      link: p.link,
    };
  });
}

/** The click's answer read (its pieces), null when it isn't one. */
export function parsePieces(j: unknown): AvPiece[] | null {
  const o = obj(j);
  if (!o || !Array.isArray(o.pieces)) return null;
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
  }));
}

/** The settings as a link carries them (`av=`): on, theme, layer, set, switch, opacity. */
export const toField = (s: AvailableState) => [s.on ? 1 : 0, s.theme, s.layer, s.set, s.avail, +s.opacity.toFixed(2)].join(',');

/** A link's `av=` over the settings it changes (`base`), base where it isn't one. */
export function fromField(v: string | null | undefined, base: AvailableState): AvailableState {
  const f = v?.split(',');
  if (!f || f.length !== 6) return base;
  const id = (x: string, d: string) => (/^[a-z0-9_]+$/.test(x) ? x : d);
  const op = Number(f[5]);
  return {
    on: f[0] === '1',
    theme: id(f[1], base.theme),
    layer: id(f[2], base.layer),
    set: id(f[3], base.set),
    avail: f[4] === 'all' || f[4] === 'open' ? f[4] : base.avail,
    opacity: f[5] !== '' && Number.isFinite(op) ? clampOpacity(op) : base.opacity,
  };
}

/** The overlay's opacity range. */
export const OPACITY_RANGE: [number, number] = [0.1, 1];
const clampOpacity = (v: number) => Math.min(OPACITY_RANGE[1], Math.max(OPACITY_RANGE[0], v));

/** Settings read from a save or a profile, each one valid (else `base`'s). */
export function cleanState(s: AvailableState, base: AvailableState): AvailableState {
  const id = (x: unknown, d: string) => (typeof x === 'string' && /^[a-z0-9_]+$/.test(x) ? x : d);
  return {
    on: typeof s.on === 'boolean' ? s.on : base.on,
    theme: id(s.theme, base.theme),
    layer: id(s.layer, base.layer),
    set: id(s.set, base.set),
    avail: s.avail === 'open' || s.avail === 'all' ? s.avail : base.avail,
    opacity: Number.isFinite(s.opacity) ? clampOpacity(s.opacity) : base.opacity,
  };
}
