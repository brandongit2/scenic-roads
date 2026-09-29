// Colour modes and scenic score components (weight presets: presets.ts).
// Component normalisation must match crates/server/src/drives.rs.

export type Mode =
  | 'elev' | 'grade' | 'relief'
  | 'score' | 'view' | 'water' | 'vista' | 'drama' | 'ridge' | 'curvy'
  | 'openness' | 'trees' | 'forest' | 'fields' | 'built' | 'bldg'
  | 'map';

export interface ModeDef {
  key: Mode;
  label: string;
  short: string;
  /** Shader mode id (see layer.ts). */
  id: number;
  unit: string;
  /** Default range in display units, used when not auto-fitting. */
  range: [number, number];
  /** Auto-fit the range to the roads in view by default. */
  auto: boolean;
  /** Full plausible span (threshold slider, "Full" range). */
  domain: [number, number];
  thrDefault: number;
  /** Slider step in display units. */
  step: number;
  diverging?: boolean;
  help: string;
  fmt: (v: number) => string;
}

const n0 = (v: number) => Math.round(v).toLocaleString('en-CA');
const km2 = (v: number) => (v < 1 ? v.toFixed(2) : v < 10 ? v.toFixed(1) : n0(v)) + ' km²';

/** Visible area from its 0..255 log encoding (roadcore::scenic::area_u8). */
export const u8Area = (v: number) => Math.expm1((v / 255) * Math.log(1 + 700 / 0.05)) * 0.05;

export const MODES: ModeDef[] = [
  { key: 'elev', label: 'Elevation', short: 'Elevation', id: 0, unit: 'm', range: [0, 600], auto: true, domain: [-50, 1950], thrDefault: 500, step: 10, help: 'Metres above sea level.', fmt: (v) => `${n0(v)} m` },
  { key: 'grade', label: 'Grade', short: 'Grade', id: 1, unit: '%', range: [0, 15], auto: false, domain: [0, 30], thrDefault: 8, step: 0.5, help: 'Steepness over ~50 m.', fmt: (v) => `${v.toFixed(0)} %` },
  { key: 'relief', label: 'Local relief', short: 'Relief', id: 2, unit: 'm', range: [0, 600], auto: true, domain: [-50, 1950], thrDefault: 500, step: 10, help: 'Lowest → highest road in view.', fmt: (v) => `${n0(v)} m` },
  { key: 'score', label: 'Scenic score', short: 'Score', id: 3, unit: '', range: [0, 100], auto: true, domain: [0, 100], thrDefault: 60, step: 1, help: 'Weighted blend of the scenic factors below (weights adjustable).', fmt: (v) => v.toFixed(0) },
  { key: 'view', label: 'Views', short: 'Views', id: 4, unit: 'km²', range: [0, 1], auto: true, domain: [0, 1], thrDefault: 0.5, step: 0.01, help: 'Area visible within 15 km — terrain and tree heights block the view.', fmt: (v) => km2(u8Area(v * 255)) },
  { key: 'water', label: 'Water views', short: 'Water', id: 5, unit: 'km²', range: [0, 1], auto: true, domain: [0, 1], thrDefault: 0.3, step: 0.01, help: 'Lake, river and sea area visible within 15 km.', fmt: (v) => km2(u8Area(v * 255)) },
  { key: 'vista', label: 'Vista distance', short: 'Vista', id: 6, unit: 'km', range: [0, 12], auto: true, domain: [0, 15], thrDefault: 3, step: 0.1, help: 'Average farthest visible distance over all directions.', fmt: (v) => `${v.toFixed(1)} km` },
  { key: 'drama', label: 'Terrain drama', short: 'Drama', id: 7, unit: 'm', range: [0, 500], auto: true, domain: [0, 765], thrDefault: 200, step: 5, help: 'Relief of the terrain within 3 km (highest − lowest).', fmt: (v) => `${n0(v)} m` },
  { key: 'ridge', label: 'Ridge ↔ valley', short: 'Ridge', id: 8, unit: 'm', range: [-60, 60], auto: false, domain: [-256, 254], thrDefault: 20, step: 2, diverging: true, help: 'Height above (+) or below (−) the surrounding terrain within 1.5 km.', fmt: (v) => `${v > 0 ? '+' : ''}${n0(v)} m` },
  { key: 'curvy', label: 'Curviness', short: 'Curvy', id: 9, unit: '°/km', range: [0, 400], auto: true, domain: [0, 1020], thrDefault: 150, step: 5, help: 'Turning per kilometre over ±250 m.', fmt: (v) => `${n0(v)} °/km` },
  { key: 'openness', label: 'Unblocked views', short: 'Open', id: 10, unit: '%', range: [0, 100], auto: false, domain: [0, 100], thrDefault: 50, step: 1, help: 'Share of directions not blocked by trees or terrain within 300 m.', fmt: (v) => `${n0(v)} %` },
  { key: 'trees', label: 'Roadside trees', short: 'Trees', id: 11, unit: 'm', range: [0, 25], auto: false, domain: [0, 32], thrDefault: 10, step: 0.5, help: 'Typical (p95) tree height within 30 m of the road.', fmt: (v) => `${v.toFixed(0)} m` },
  { key: 'forest', label: 'Forest cover', short: 'Forest', id: 12, unit: '%', range: [0, 100], auto: false, domain: [0, 100], thrDefault: 50, step: 1, help: 'Share of land within 150 m under trees taller than 5 m.', fmt: (v) => `${n0(v)} %` },
  { key: 'fields', label: 'Open land', short: 'Fields', id: 13, unit: '%', range: [0, 100], auto: false, domain: [0, 100], thrDefault: 30, step: 1, help: 'Fields, meadows and bare land within 1 km (pastoral views).', fmt: (v) => `${n0(v)} %` },
  { key: 'built', label: 'Built-up', short: 'Built', id: 14, unit: '%', range: [0, 100], auto: false, domain: [0, 100], thrDefault: 20, step: 1, help: 'Built-up land within 500 m.', fmt: (v) => `${n0(v)} %` },
  { key: 'bldg', label: 'Roadside buildings', short: 'Buildings', id: 15, unit: '%', range: [0, 100], auto: false, domain: [0, 100], thrDefault: 25, step: 1, help: 'Share of the road frontage (both sides, ±50 m) lined with buildings within 30 m, fading out at 80 m. Heights are not counted.', fmt: (v) => `${n0(v)} %` },
  { key: 'map', label: 'Street map', short: 'Map', id: 20, unit: '', range: [0, 1], auto: false, domain: [0, 1], thrDefault: 0, step: 1, help: 'Roads as on a street map: fixed colours by road size, route network or attribute.', fmt: () => '' },
];
/** Scenic metrics (the "Scenic" display type). */
export const isScenic = (m: ModeDef) => m.id >= 3 && m.id < 20;

export const modeDef = (k: Mode) => MODES.find((m) => m.key === k) ?? MODES[0];

// ---- scenic score components --------------------------------------------------------------

export interface Component {
  key: string;
  label: string;
  help: string;
  /** Weight for presets saved before this factor existed (else 0). */
  def?: number;
  /** Measured factors (not yes/no flags): short label, unit and value (a number) for the hover bars. */
  bar?: { short: string; unit: string; text: (ch: ArrayLike<number>) => string };
}

const signed = (v: number) => `${v > 0 ? '+' : v < 0 ? '−' : ''}${n0(Math.abs(v))}`;
const area = (v: number) => (v < 1 ? v.toFixed(2) : v < 10 ? v.toFixed(1) : n0(v));

export const COMPONENTS: Component[] = [
  { key: 'views', label: 'Views', help: 'Visible area (trees & terrain block the view)', bar: { short: 'Views', unit: 'km²', text: (c) => area(u8Area(c[0])) } },
  { key: 'water', label: 'Water in view', help: 'Visible lake / river / sea area', bar: { short: 'Water', unit: 'km²', text: (c) => area(u8Area(c[1])) } },
  { key: 'vista', label: 'Long vistas', help: 'Farthest visible distance (full at 15 km)', bar: { short: 'Vista', unit: 'km', text: (c) => (c[8] / 17).toFixed(1) } },
  { key: 'relief', label: 'Mountains', help: 'Terrain relief within 3 km (full at 600 m)', bar: { short: 'Mountains', unit: 'm', text: (c) => n0(c[2] * 3) } },
  { key: 'ridge', label: 'Ridge roads', help: 'Road above its surroundings (full at +60 m)', bar: { short: 'Ridge', unit: 'm', text: (c) => signed((c[3] - 128) * 2) } },
  { key: 'curvy', label: 'Twisty', help: 'Curvature (full at 400 °/km)', bar: { short: 'Twisty', unit: '°/km', text: (c) => n0(c[4] * 4) } },
  { key: 'unblocked', label: 'No tree walls', help: 'Not enclosed by roadside trees / cuts', bar: { short: 'Open', unit: '%', text: (c) => n0(100 - c[5] / 2.55) } },
  { key: 'forest', label: 'Forest', help: 'Forest cover (foliage); negative = prefer open country', bar: { short: 'Forest', unit: '%', text: (c) => n0(c[10] / 2.55) } },
  { key: 'built', label: 'Built-up', help: 'Towns nearby (negative = away from towns)', bar: { short: 'Built-up', unit: '%', text: (c) => n0(c[6] / 2.55) } },
  { key: 'bldg', label: 'Roadside buildings', def: -1, help: 'Buildings lining the road, the closer together the more (negative = away from villages and strip development)', bar: { short: 'Buildings', unit: '%', text: (c) => n0(c[12] / 2.55) } },
  { key: 'route', label: 'Scenic route', help: 'Designated byway / route touristique' },
  { key: 'viewpoint', label: 'Viewpoints', help: 'Mapped viewpoint within 1 km' },
];
export const NCOMP = COMPONENTS.length;

/** Components (0..1, COMPONENTS order) from the 13 channel bytes (see roadcore::scenic::ch). */
export function components(ch: ArrayLike<number>): number[] {
  const f = ch[7];
  const b = (m: number) => ((f & m) !== 0 ? 1 : 0);
  return [
    ch[0] / 255,
    ch[1] / 255,
    ch[8] / 255,
    Math.min(1, (ch[2] * 3) / 600),
    Math.max(0, Math.min(1, ((ch[3] - 128) * 2) / 60)),
    Math.min(1, (ch[4] * 4) / 400),
    1 - ch[5] / 255,
    ch[10] / 255,
    ch[6] / 255,
    (ch[12] ?? 0) / 255,
    b(1), b(4),
  ];
}

/**
 * Older weight lists → the current 12: 16 values (before waterfront, parks, heritage,
 * UNESCO/dark-sky and farmland were dropped from the score), 11 (before roadside buildings, which
 * then weigh −1).
 */
export function migrateWeights(w: unknown): number[] | null {
  if (!Array.isArray(w) || !w.every((v) => typeof v === 'number' && Number.isFinite(v))) return null;
  if (w.length === NCOMP) return w as number[];
  const bldg = COMPONENTS.findIndex((c) => c.key === 'bldg');
  const add = (v: number[]) => [...v.slice(0, bldg), COMPONENTS[bldg].def ?? 0, ...v.slice(bldg)];
  if (w.length === 11) return add(w as number[]);
  if (w.length === 16) return add([0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 11].map((i) => w[i] as number));
  return null;
}

export function scoreOf(ch: ArrayLike<number>, w: number[]): number {
  const c = components(ch);
  const pos = w.reduce((a, v) => a + Math.max(v, 0), 0) || 1e-6;
  const s = c.reduce((a, v, i) => a + v * w[i], 0);
  return Math.max(0, Math.min(1, s / pos)) * 100;
}

/** Metric value (display units) of a vertex for a mode. */
export function metricOf(mode: Mode, elevM: number, gradePct: number, ch: ArrayLike<number>, w: number[]): number {
  switch (mode) {
    case 'elev':
    case 'relief':
      return elevM;
    case 'grade':
      return gradePct;
    case 'score':
      return scoreOf(ch, w);
    case 'view':
      return ch[0] / 255;
    case 'water':
      return ch[1] / 255;
    case 'vista':
      return ch[8] / 17;
    case 'drama':
      return ch[2] * 3;
    case 'ridge':
      return (ch[3] - 128) * 2;
    case 'curvy':
      return ch[4] * 4;
    case 'openness':
      return 100 - ch[5] / 2.55;
    case 'trees':
      return ch[11] / 8;
    case 'forest':
      return ch[10] / 2.55;
    case 'fields':
      return ch[9] / 2.55;
    case 'built':
      return ch[6] / 2.55;
    case 'bldg':
      return (ch[12] ?? 0) / 2.55;
    case 'map':
      return 0;
  }
}

export const FLAG_LABELS: [number, string][] = [
  [1, 'scenic route'],
  [2, 'park'],
  [4, 'viewpoint nearby'],
  [8, 'waterfront'],
  [16, 'heritage site nearby'],
  [32, 'covered bridge'],
  [64, 'UNESCO / dark-sky area'],
  [128, 'Indigenous land'],
];
