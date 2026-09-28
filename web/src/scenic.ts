// Colour modes, scenic score components, weights and presets.
// Component normalisation must match crates/server/src/drives.rs.

export type Mode =
  | 'elev' | 'grade' | 'relief'
  | 'score' | 'view' | 'water' | 'vista' | 'drama' | 'ridge' | 'curvy'
  | 'openness' | 'trees' | 'forest' | 'fields' | 'built';

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
];

export const modeDef = (k: Mode) => MODES.find((m) => m.key === k) ?? MODES[0];

// ---- scenic score components --------------------------------------------------------------

export interface Component {
  key: string;
  label: string;
  help: string;
}

export const COMPONENTS: Component[] = [
  { key: 'views', label: 'Views', help: 'Visible area (trees & terrain block the view)' },
  { key: 'water', label: 'Water in view', help: 'Visible lake / river / sea area' },
  { key: 'vista', label: 'Long vistas', help: 'Farthest visible distance' },
  { key: 'relief', label: 'Mountains', help: 'Terrain relief within 3 km' },
  { key: 'ridge', label: 'Ridge roads', help: 'Road above its surroundings' },
  { key: 'curvy', label: 'Twisty', help: 'Curvature' },
  { key: 'unblocked', label: 'No tree walls', help: 'Not enclosed by roadside trees / cuts' },
  { key: 'forest', label: 'Forest', help: 'Forest cover (foliage); negative = prefer open country' },
  { key: 'fields', label: 'Farmland', help: 'Open fields & meadows nearby' },
  { key: 'built', label: 'Built-up', help: 'Towns nearby (negative = away from towns)' },
  { key: 'route', label: 'Scenic route', help: 'Designated byway / route touristique' },
  { key: 'viewpoint', label: 'Viewpoints', help: 'Mapped viewpoint within 1 km' },
  { key: 'waterfront', label: 'Waterfront', help: 'Water within 100 m' },
  { key: 'park', label: 'Parks', help: 'Inside a park or protected area' },
  { key: 'heritage', label: 'Heritage', help: 'Designated heritage site within 500 m' },
  { key: 'special', label: 'UNESCO / dark sky', help: 'Biosphere reserve, geopark or dark-sky preserve' },
];
export const NCOMP = COMPONENTS.length;

export const PRESETS: Record<string, { label: string; w: number[] }> = {
  balanced: { label: 'Balanced', w: [1, 1, 0.5, 0.8, 0.3, 0.6, 0.6, 0, 0, 0, 0.5, 0.4, 0.4, 0, 0, 0] },
  vistas: { label: 'Big vistas', w: [1.5, 0.6, 1.2, 0.6, 0.8, 0.1, 1, -0.2, 0.2, -0.3, 0.3, 0.6, 0.2, 0, 0, 0] },
  water: { label: 'Lakes & coast', w: [0.5, 1.6, 0.4, 0.2, 0, 0.3, 0.6, 0, 0, -0.2, 0.3, 0.3, 1.2, 0, 0, 0] },
  mountains: { label: 'Mountains', w: [0.8, 0.3, 0.6, 1.6, 0.6, 0.6, 0.4, 0, 0, -0.3, 0.3, 0.4, 0.1, 0.2, 0, 0] },
  twisty: { label: 'Twisty', w: [0.4, 0.2, 0.2, 0.6, 0, 2, 0.2, 0, 0, -0.5, 0.2, 0, 0, 0, 0, 0] },
  foliage: { label: 'Foliage', w: [0.3, 0.3, 0.1, 0.6, 0, 0.5, -0.2, 1.4, 0.2, -0.6, 0.4, 0.2, 0.2, 0.3, 0, 0] },
  backroads: { label: 'Quiet backroads', w: [0.6, 0.5, 0.3, 0.5, 0.2, 0.5, 0.4, 0.2, 0.6, -1.2, 0.2, 0.2, 0.2, 0.3, 0, 0] },
  culture: { label: 'Heritage', w: [0.5, 0.5, 0.2, 0.3, 0, 0.2, 0.3, 0, 0.3, 0, 0.8, 0.4, 0.3, 0.3, 1.5, 0.5] },
};
export const DEFAULT_WEIGHTS = PRESETS.balanced.w;

/** Components from the 12 channel bytes (see roadcore::scenic::ch). */
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
    ch[9] / 255,
    ch[6] / 255,
    b(1), b(4), b(8), b(2), b(16), b(64),
  ];
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
