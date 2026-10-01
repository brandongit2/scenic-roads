// Road classes (must match roadcore::class, ordered minor → major), then the passenger rail
// groups (rails.tiles).
export const CLASS_KEYS = [
  'service', 'living_street', 'residential', 'unclassified', 'tertiary',
  'secondary', 'primary', 'trunk', 'motorway', 'ferry',
  'tram', 'metro', 'commuter', 'intercity', 'heritage',
] as const;
export const CLASS_LABELS = [
  'Service', 'Living street', 'Residential', 'Unclassified', 'Tertiary',
  'Secondary', 'Primary', 'Trunk', 'Motorway', 'Ferry',
  'Tram', 'Metro', 'Commuter rail', 'Intercity rail', 'Heritage railway',
];
export const NCLASS = 15;
/** Road classes (and ferries) come first. */
export const NROAD = 10;
export const FERRY = 9;
/** Minor road classes: service to unclassified (0–3). */
export const MINOR_MAX_CLASS = 3;
export const RAIL0 = 10;
/** Passenger rail groups: class RAIL0 + index; a track can serve several (line flags). */
export const RAIL_GROUPS = [
  { key: 'tram', label: 'Trams' },
  { key: 'metro', label: 'Metro · rapid transit' },
  { key: 'commuter', label: 'Commuter · regional' },
  { key: 'intercity', label: 'Intercity · sleepers' },
  { key: 'heritage', label: 'Heritage · mountain railways' },
] as const;
export const NRAIL = RAIL_GROUPS.length;

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
/** Statistics are kept per (group, surface, named): index = (group * 2 + unpaved) * 2 + unnamed. */
export const NSG = NGROUP * 4;
/** Per-line flags (tile v3+): the road has neither a name nor a route number; roads: one-way,
 * toll; rail: the service groups using the track (bits from LF_RAIL_SHIFT). */
export const LF_UNNAMED = 1;
export const LF_ONEWAY = 2;
export const LF_TOLL = 4;
export const LF_RAIL_SHIFT = 1;
/** Statistics group of each class: road groups for roads, the rail group for rail (each layer
 * keeps its own statistics, so the indices don't collide). */
export const CLASS_GROUP: number[] = (() => {
  const g = new Array(NCLASS).fill(0);
  GROUPS.forEach((gr, i) => gr.classes.forEach((c) => (g[c] = i)));
  for (let k = 0; k < NRAIL; k++) g[RAIL0 + k] = k;
  return g;
})();

// Tile pyramid served by the backend.
export const TILE_MINZOOM = 4;
export const TILE_MAXZOOM = 14;
/** Tiles up to this zoom have their pieces split to at most SPRITE_SEG_PX (of a 256 px tile), so
 * the renderer can draw them as point sprites (roads/layer.ts). */
export const SPRITE_MAXZ = 11;
export const SPRITE_SEG_PX = 8;
/** Largest sprite (CSS px) the renderer draws a piece as; tiles with longer pieces on screen are
 * drawn as quads. A large sprite is mostly empty pixels, but even so far cheaper than a quad. */
export const SPRITE_MAX_CSS = 128;
/** Cells (tile units) of the levels of detail (roads/lod.ts): the coarsest tiles get them all
 * (for views zoomed out beyond them), the others the first (drawn at one to two times their size,
 * a CSS pixel at most). */
export const LOD_CELLS = [8, 16, 32, 64];
/** Largest cell (CSS px on screen) a level of detail may use: a pixel. The pieces it leaves out
 * pass their area to the ones kept (lod.ts), which the renderer sums (roads/layer.ts), so a view
 * stays the same, only the area moved within the cell. */
export const LOD_CELL_PX = 1;

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
  [0.3, 0.45, 0.8, 1.4, 2.6, 4], // tram
  [0.5, 0.7, 1.1, 1.8, 3.2, 5], // metro
  [0.6, 0.8, 1.3, 2.0, 3.4, 5.5], // commuter
  [0.8, 1.0, 1.5, 2.2, 3.6, 6], // intercity
  [0.7, 0.9, 1.4, 2.1, 3.6, 6], // heritage
];
// Casing width (CSS px) at zooms CASING_Z (no casing below the first), and the scenic-route
// halo width at zooms GLOW_Z.
export const CASING_Z: [number, number, number] = [9.5, 12, 16];
export const CASING_W: [number, number, number] = [0.35, 0.9, 1.6];
export const GLOW_Z: [number, number, number, number] = [4, 8, 12, 16];
export const GLOW_W: [number, number, number, number] = [1.2, 1.8, 2.6, 4];
// Colour strength (mix with background) at zooms FADE_Z: minor roads a little fainter than major
// ones, the same from zoom 10 out: zoomed out, a road's share of a pixel already follows its area
// (roads/layer.ts), and fading its colour too dimmed whole cities of streets (Paris all but gone
// at zoom 4, where its streets drew at a third of their colour).
export const FADE_Z = [4, 7, 10, 13];
export const FADES: number[][] = [
  [0.75, 0.75, 0.75, 1],
  [0.9, 0.9, 0.9, 1],
  [0.9, 0.9, 0.9, 1],
  [0.9, 0.9, 0.9, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
  [0.8, 0.8, 0.8, 0.9],
  [0.9, 0.9, 0.9, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
  [1, 1, 1, 1],
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
