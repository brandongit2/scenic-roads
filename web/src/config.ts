// Road classes (must match roadcore::class, ordered minor → major).
export const CLASS_KEYS = [
  'service', 'living_street', 'residential', 'unclassified', 'tertiary',
  'secondary', 'primary', 'trunk', 'motorway', 'ferry',
] as const;
export const CLASS_LABELS = [
  'Service', 'Living street', 'Residential', 'Unclassified', 'Tertiary',
  'Secondary', 'Primary', 'Trunk', 'Motorway', 'Ferry',
];
export const NCLASS = 10;
export const FERRY = 9;

// Tile style byte bits (roadcore::tile::style), and the extra GPU end-of-line bit.
export const ST_UNPAVED = 1 << 4;
export const ST_BRIDGE = 1 << 5;
export const ST_TUNNEL = 1 << 6;
export const ST_LINK = 1 << 7;
export const GPU_EOL = 1 << 7;

// Class groups: what the layer toggles and the statistics are split by.
export const GROUPS = [
  { key: 'major', label: 'Motorway · trunk · primary', classes: [6, 7, 8] },
  { key: 'mid', label: 'Secondary · tertiary', classes: [4, 5] },
  { key: 'local', label: 'Local streets', classes: [1, 2, 3] },
  { key: 'service', label: 'Service roads', classes: [0] },
  { key: 'ferry', label: 'Car ferries', classes: [9] },
] as const;
export const NGROUP = GROUPS.length;
/** Statistics are kept per (group, surface): index = group * 2 + (unpaved ? 1 : 0). */
export const NSG = NGROUP * 2;
export const CLASS_GROUP: number[] = (() => {
  const g = new Array(NCLASS).fill(0);
  GROUPS.forEach((gr, i) => gr.classes.forEach((c) => (g[c] = i)));
  return g;
})();

// Tile pyramid served by the backend.
export const TILE_MINZOOM = 4;
export const TILE_MAXZOOM = 14;

// Line widths in CSS px at MapLibre zooms WIDTH_Z, per class.
export const WIDTH_Z = [4, 7, 10, 13, 16, 19];
export const WIDTHS: number[][] = [
  [0.2, 0.35, 0.6, 1.2, 3.0, 6], // service
  [0.25, 0.45, 0.8, 1.6, 4.0, 8], // living street
  [0.3, 0.5, 0.9, 1.8, 4.5, 9], // residential
  [0.3, 0.55, 1.0, 1.9, 4.5, 9], // unclassified
  [0.45, 0.8, 1.4, 2.4, 5.5, 11], // tertiary
  [0.6, 1.0, 1.7, 2.8, 6.0, 12], // secondary
  [0.8, 1.2, 2.0, 3.2, 6.5, 13], // primary
  [1.0, 1.4, 2.2, 3.6, 7.0, 14], // trunk
  [1.1, 1.6, 2.4, 4.0, 8.0, 16], // motorway
  [0.5, 0.7, 1.0, 1.4, 2.0, 3], // ferry
];
// Colour strength (mix with background) at zooms FADE_Z: minor roads fade when zoomed out.
export const FADE_Z = [4, 7, 10, 13];
export const FADES: number[][] = [
  [0.3, 0.45, 0.75, 1],
  [0.45, 0.6, 0.9, 1],
  [0.45, 0.6, 0.9, 1],
  [0.45, 0.6, 0.9, 1],
  [0.7, 0.85, 1, 1],
  [0.85, 0.95, 1, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
  [0.55, 0.65, 0.8, 0.9],
];

export function interp(zs: number[], vs: number[], z: number): number {
  if (z <= zs[0]) return vs[0];
  for (let i = 1; i < zs.length; i++) {
    if (z <= zs[i]) {
      const t = (z - zs[i - 1]) / (zs[i] - zs[i - 1]);
      return vs[i - 1] + (vs[i] - vs[i - 1]) * t;
    }
  }
  return vs[vs.length - 1];
}

// Statistics grid inside each tile, and quantile sketch sizes.
export const CELLS = 8;
export const EQ = 33; // elevation quantiles per (cell, group): q = 0, 1/32 … 1
export const GQ = 17; // grade quantiles per (cell, group)

export const BG = [0x0b / 255, 0x0e / 255, 0x13 / 255];
export const DIM_GREY = [0.2, 0.22, 0.26];

// Classes that get a dark casing when zoomed in (tertiary and up; bridges always).
export const CASING_CLASSES_MASK = 0b111110000;
