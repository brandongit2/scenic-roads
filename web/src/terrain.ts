// Terrain presentation: 3D mesh + exaggeration, hillshade, hypsometric tint, contours, sky.
import type { Map as MLMap } from 'maplibre-gl';
import * as maplibregl from 'maplibre-gl';
import mlcontour from 'maplibre-contour';
import { HYPSO } from './basemap';
import { PALETTES, baseKey, isRev, paletteFn } from './palettes';
import type { ScaleFields, Terrain } from './state';
import { ver } from './api';
import { CONTOUR_MINZOOM } from './contours';

type RGB = [number, number, number];

function ramp(stops: [number, string][]): (t: number) => RGB {
  const cs = stops.map(([t, h]) => [t, parseInt(h.slice(1, 3), 16) / 255, parseInt(h.slice(3, 5), 16) / 255, parseInt(h.slice(5, 7), 16) / 255]);
  return (t) => {
    for (let i = 1; i < cs.length; i++) {
      if (t <= cs[i][0]) {
        const u = (t - cs[i - 1][0]) / (cs[i][0] - cs[i - 1][0]);
        return [1, 2, 3].map((k) => cs[i - 1][k] + (cs[i][k] - cs[i - 1][k]) * u) as RGB;
      }
    }
    const l = cs[cs.length - 1];
    return [l[1], l[2], l[3]];
  };
}

const atlas = ramp(HYPSO.slice(1).map(([e, c]) => [e / 1900, c] as [number, string]));
/** Tint colour ramps: terrain ramps ('roads' follows the current road palette), then every shared
 * ramp. Keys ending in "_r" are reversed. */
export const TINT_PALETTES: { key: string; label: string; group: string; fn?: (t: number) => RGB }[] = [
  { key: 'atlas', label: 'Atlas (green → brown → white)', group: 'Terrain', fn: atlas },
  {
    key: 'steep',
    label: 'Slope classes (green → red → purple)',
    group: 'Terrain',
    fn: ramp([[0, '#20352a'], [0.1, '#2f5a3a'], [0.2, '#7a9a3e'], [0.3, '#d2c14a'], [0.45, '#e58a3a'], [0.6, '#d6453b'], [0.8, '#a3337f'], [1, '#5b2a8f']]),
  },
  { key: 'roads', label: 'Same as roads', group: 'Terrain' },
  { key: 'earth', label: 'Earth tones', group: 'Terrain', fn: ramp([[0, '#23321f'], [0.3, '#4b5a2e'], [0.55, '#7d6a3e'], [0.8, '#8f5e44'], [1, '#d9c9b4']]) },
  { key: 'glacier', label: 'Glacier (blue → white)', group: 'Terrain', fn: ramp([[0, '#0f2440'], [0.4, '#2e5f8a'], [0.75, '#7fb2d4'], [1, '#eef6ff']]) },
  { key: 'grey', label: 'Greyscale', group: 'Terrain', fn: ramp([[0, '#101318'], [1, '#d8dde5']]) },
  ...PALETTES.map((p) => ({ key: p.key, label: p.label, group: p.group, fn: p.fn })),
];

/** Per tint variable: the scale's whole domain, its step, the unit, and the band heights offered. */
export const TINT_VARS = {
  elev: { domain: [-100, 4900] as [number, number], step: 10, unit: 'm', bands: [25, 50, 100, 200, 500] },
  slope: { domain: [0, 200] as [number, number], step: 1, unit: '%', bands: [2, 5, 10, 15, 20, 25] },
};

/** Colour along the ramp (0..1) for a palette key, with the emphasis curve ('roads': the road
 * palette). */
export function tintColourFn(palette: string, curve: number, roadPalette: string): (u: number) => RGB {
  const base = baseKey(palette);
  const f0 = base === 'roads' ? paletteFn(roadPalette) : (TINT_PALETTES.find((p) => p.key === base)?.fn ?? atlas);
  const fn = isRev(palette) ? (u: number) => f0(1 - u) : f0;
  return (u) => fn(Math.pow(Math.max(0, Math.min(1, u)), curve));
}

/** Opacity along the ramp (same curve as the road low-end fade). */
const fadeFn = (sc: ScaleFields) => {
  const sp = Math.max(0.05, sc.lowSpan);
  return (u: number) => 1 - sc.lowFade * Math.pow(1 - Math.max(0, Math.min(1, u / sp)), 1.5);
};

const rgba = (c: RGB, a: number) =>
  `rgba(${c.map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255)).join(',')},${Math.max(0, Math.min(1, a)).toFixed(3)})`;

/** The tint's colour and opacity at a value (metres or percent), on the range in use: banded,
 * equalised (cdf over the range), faded at the low end; outside the highlight, transparent. */
export function tintColourAt(t: Terrain, sc: ScaleFields, range: [number, number], cdf: Uint8Array | null, roadPalette: string): (v: number) => [RGB, number] {
  const col = tintColourFn(sc.palette, t.tintCurve, roadPalette);
  const al = fadeFn(sc);
  const [lo, hi] = range;
  const B = t.tintBands;
  return (v) => {
    const thr = sc.threshold;
    if (thr.on && !(thr.dir === 'low' ? v >= lo : thr.dir === 'below' ? v <= thr.value : v >= thr.value)) return [[0, 0, 0], 0];
    const e = B > 0 ? (Math.floor(v / B) + 0.5) * B : v;
    let u = Math.max(0, Math.min(1, (e - lo) / (hi - lo || 1e-9)));
    if (cdf) u = cdf[Math.min(255, Math.floor(u * 255 + 0.5))] / 255;
    return [col(u), al(u)];
  };
}

/** CSS gradient of the tint over a range (the palette list's swatches). */
export function tintCss(t: Terrain, sc: ScaleFields, range: [number, number], roadPalette: string): string {
  const at = tintColourAt(t, { ...sc, threshold: { ...sc.threshold, on: false } }, range, null, roadPalette);
  const parts: string[] = [];
  const n = 24;
  for (let i = 0; i <= n; i++) {
    const [c, a] = at(range[0] + (i / n) * (range[1] - range[0]));
    parts.push(`${rgba(c, Math.max(0.12, a))} ${((i / n) * 100).toFixed(1)}%`);
  }
  return `linear-gradient(90deg, ${parts.join(', ')})`;
}

let tintSig = '';

/** Update the colour-relief layer for the range in use (eased by the caller, main.ts); cheap to
 * call every frame: a no-op when nothing changed, else the ramp without style validation. */
export function applyTint(map: MLMap, t: Terrain, sc: ScaleFields, range: [number, number], cdf: Uint8Array | null, roadPalette: string, cdfKey = '') {
  if (!map.getLayer('tint') || !map.getLayer('tint-slope')) return;
  const id = t.tintVar === 'slope' ? 'tint-slope' : 'tint';
  const vis = (l: string) => (t.tint && id === l ? 'visible' : 'none');
  if (map.getLayoutProperty('tint', 'visibility') !== vis('tint')) map.setLayoutProperty('tint', 'visibility', vis('tint'));
  if (map.getLayoutProperty('tint-slope', 'visibility') !== vis('tint-slope')) map.setLayoutProperty('tint-slope', 'visibility', vis('tint-slope'));
  if (!t.tint) return;
  const [lo, hi] = range;
  const span = hi - lo;
  const sig = [id, JSON.stringify(sc), baseKey(sc.palette) === 'roads' ? roadPalette : '', t.tintBands, t.tintCurve, t.tintOpacity, cdf ? cdfKey : '', lo.toPrecision(5), hi.toPrecision(5)].join('|');
  if (sig === tintSig) return;
  tintSig = sig;
  const at = tintColourAt(t, sc, range, cdf, roadPalette);
  const col = (v: number) => {
    const [c, a] = at(v);
    return rgba(c, a);
  };
  // Values to place stops at: band edges (each a pair of stops 1 cm apart: colour-relief only
  // takes `interpolate`), else even steps (more when equalised: the lookup bends the ramp), and
  // the highlight's edge.
  const vs: number[] = [];
  const thr = sc.threshold;
  if (t.tintBands > 0) {
    let B = t.tintBands;
    while (span / B > 90) B *= 2;
    const first = Math.floor(lo / B) * B, eps = Math.min(0.01, B / 100);
    vs.push(first);
    for (let e = first + B; e < hi; e += B) vs.push(e - eps, e);
    vs.push(Math.max(hi, first + B) + 1);
  } else {
    const n = cdf ? 64 : 32;
    for (let i = 0; i <= n; i++) vs.push(lo + (i / n) * span);
  }
  if (thr.on && thr.dir !== 'low') vs.push(thr.value - span * 1e-4, thr.value);
  else if (thr.on) vs.push(lo - span * 1e-4, lo);
  const stops = [...new Set(vs)].sort((a, b) => a - b);
  const expr: unknown[] = ['interpolate', ['linear'], ['elevation']];
  for (const v of stops) expr.push(v, col(v));
  map.setPaintProperty(id, 'color-relief-color', expr as never, { validate: false });
  map.setPaintProperty(id, 'color-relief-opacity', t.tintOpacity, { validate: false });
}

/** Opacity of every label layer (the profile / marker labels stay opaque). */
export function applyLabelOpacity(map: MLMap, v: number, scale: (id: string) => number = () => 1) {
  for (const l of map.getStyle().layers) {
    if (l.type !== 'symbol' || l.id === 'marks-label') continue;
    const k = scale(l.id);
    if (Number.isNaN(k)) continue; // set elsewhere (landmark names follow their dots)
    map.setPaintProperty(l.id, 'text-opacity', (l.id === 'contour-label' ? v * 0.85 : v) * k);
  }
}

/** Contour intervals [minor, major] (m) by tile zoom, from that zoom up; the density shifts
 * the zooms (Layers → Terrain → Interval). No finer than 5 m: the terrain tiles' cells are tens of metres. */
const INTERVALS: [number, [number, number]][] = [[7, [500, 2500]], [8, [200, 1000]], [10, [100, 500]], [11, [50, 250]], [12, [20, 100]], [14, [10, 50]], [16, [5, 25]]];
/** The intervals contour tiles of zoom `z` get at a density. */
export function contourInterval(z: number, density = 0): [number, number] {
  let r = INTERVALS[0][1];
  for (const [k, v] of INTERVALS) if (z + density >= k) r = v;
  return r;
}

let demSource: InstanceType<typeof mlcontour.DemSource> | null = null;
let contourDensity = 0;
const contourUrl = (density: number) => {
  const thresholds: Record<number, [number, number]> = {};
  for (let z = 8; z <= 16; z++) thresholds[z] = contourInterval(z, density);
  return demSource!.contourProtocolUrl({ thresholds, elevationKey: 'ele', levelKey: 'level', contourLayer: 'contours', overzoom: 1 });
};

/**
 * The contours source and its layers, the first time they are shown. The lines themselves are
 * drawn by ContourLayer (contours.ts) from the source's tiles: 'contour-line' only keeps them
 * loaded (it matches no line), with the labels.
 */
function setupContours(map: MLMap, origin: string, density: number) {
  if (map.getSource('contours')) return;
  demSource ??= new mlcontour.DemSource({
    url: `${origin}/tiles/terrain/{z}/{x}/{y}${ver('terrain.tiles')}`,
    encoding: 'terrarium',
    maxzoom: 12,
    worker: true,
    cacheSize: 200,
  });
  demSource.setupMaplibre(maplibregl);
  contourDensity = density;
  map.addSource('contours', { type: 'vector', tiles: [contourUrl(density)], maxzoom: 16 });
  map.addLayer(
    {
      id: 'contour-line',
      type: 'line',
      source: 'contours',
      'source-layer': 'contours',
      minzoom: CONTOUR_MINZOOM,
      filter: ['==', ['get', 'ele'], -1e9],
      paint: { 'line-opacity': 0 },
    },
    'boundary-county',
  );
  map.addLayer(
    {
      id: 'contour-label',
      type: 'symbol',
      source: 'contours',
      'source-layer': 'contours',
      minzoom: 11,
      // (no 0 m line: contours.worker.ts)
      filter: ['all', ['>', ['get', 'level'], 0], ['>', ['get', 'ele'], 0]],
      layout: {
        'symbol-placement': 'line',
        // Facing the viewer, along the line as it shows on screen: lying on the ground, tilted
        // views foreshortened them past reading. MapLibre sizes screen-facing text by half the
        // perspective (0.5 + 0.5 × the view centre's distance ÷ the label's, at most 4×).
        'text-pitch-alignment': 'viewport',
        'text-rotation-alignment': 'map',
        'text-field': ['concat', ['number-format', ['get', 'ele'], {}], ' m'],
        'text-font': ['Noto Sans Regular'],
        'text-size': 9.5,
        'text-padding': 12,
      },
      paint: { 'text-color': '#a9b6c8', 'text-halo-color': '#0b0e13', 'text-halo-width': 1.2, 'text-opacity': 0.7 },
    },
    'park-label',
  );
}

export function applyTerrain(map: MLMap, t: Terrain, origin: string) {
  map.setTerrain(t.on ? { source: 'dem', exaggeration: t.exaggeration } : null);
  fastTerrainCoords(map);
  // Hillshade.
  if (map.getLayer('hillshade')) {
    map.setLayoutProperty('hillshade', 'visibility', t.hillshade ? 'visible' : 'none');
    const multi = t.method === 'multidirectional';
    map.setPaintProperty('hillshade', 'hillshade-method', t.method);
    map.setPaintProperty(
      'hillshade',
      'hillshade-illumination-direction',
      multi ? [t.light - 45, t.light, t.light + 45, t.light + 90].map((a) => ((a % 360) + 360) % 360) : t.light,
    );
    if (multi) {
      map.setPaintProperty('hillshade', 'hillshade-illumination-altitude', [30, 35, 30, 45]);
      map.setPaintProperty('hillshade', 'hillshade-highlight-color', ['rgba(170,190,215,0.18)', 'rgba(170,190,215,0.18)', 'rgba(170,190,215,0.12)', 'rgba(170,190,215,0.1)']);
      map.setPaintProperty('hillshade', 'hillshade-shadow-color', ['rgba(0,0,0,0.55)', 'rgba(0,0,0,0.55)', 'rgba(0,0,0,0.4)', 'rgba(0,0,0,0.3)']);
    } else {
      map.setPaintProperty('hillshade', 'hillshade-illumination-altitude', 40);
      map.setPaintProperty('hillshade', 'hillshade-highlight-color', 'rgba(170,190,215,0.30)');
      map.setPaintProperty('hillshade', 'hillshade-shadow-color', 'rgba(0,0,0,0.85)');
    }
    map.setPaintProperty('hillshade', 'hillshade-exaggeration', t.shade);
  }
  // Contours (the lines: ContourLayer).
  const cl = t.contour;
  if (t.contours) setupContours(map, origin, cl.density);
  const src = map.getSource('contours') as maplibregl.VectorTileSource | undefined;
  if (src) {
    if (cl.density !== contourDensity) {
      contourDensity = cl.density;
      src.setTiles([contourUrl(cl.density)]);
    }
    map.setLayoutProperty('contour-line', 'visibility', t.contours ? 'visible' : 'none');
    map.setLayoutProperty('contour-label', 'visibility', t.contours && cl.labels ? 'visible' : 'none');
    map.setPaintProperty('contour-label', 'text-color', cl.colour);
  }
  // Sky / horizon fog (only visible when pitched).
  map.setSky(
    t.sky
      ? {
          'sky-color': '#0d121a',
          'horizon-color': '#1b2432',
          'fog-color': '#0b0e13',
          'sky-horizon-blend': 0.6,
          'horizon-fog-blend': 0.6,
          'fog-ground-blend': 0.75,
          // No atmosphere: with this light it drew nothing visible (not one pixel changed, even at
          // full strength), yet its scattering shader ran for every pixel below zoom 7 (1.6 ms of
          // GPU a frame).
          'atmosphere-blend': 0,
        }
      : {
          'sky-color': '#0b0e13',
          'horizon-color': '#0b0e13',
          'fog-color': '#0b0e13',
          'fog-ground-blend': 1,
          'horizon-fog-blend': 0,
          'sky-horizon-blend': 0,
          'atmosphere-blend': 0,
        },
  );
}

interface CanonicalID {
  x: number;
  y: number;
  z: number;
  equals(o: CanonicalID): boolean;
  isChildOf(o: CanonicalID): boolean;
}
interface TileID {
  canonical: CanonicalID;
  clone(): TileID & { terrainRttPosMatrix32f?: Float32Array };
}
interface TerrainTiles {
  _renderableTilesKeys: string[];
  _tiles: Record<string, { tileID: TileID }>;
  _getTerrainCoordsForRegularTile?: (tileID: TileID) => Record<string, TileID>;
}

/**
 * MapLibre's terrain draping asks, every frame, for each tile of each draped source, which terrain
 * tiles it overlaps (TerrainTileManager._getTerrainCoordsForRegularTile). It copies the tile id and
 * allocates a matrix for every terrain tile before checking whether the two are related at all:
 * tilted, with a hundred terrain tiles in view, that was tens of thousands of allocations a frame and
 * the largest share of the main thread's time. The same result here, the matrices built only for the
 * related tiles (ortho, translate and scale as in gl-matrix).
 */
function fastTerrainCoords(map: MLMap) {
  const tm = (map as unknown as { terrain?: { tileManager?: TerrainTiles } }).terrain?.tileManager;
  const proto = tm && (Object.getPrototypeOf(tm) as TerrainTiles & { __fastCoords?: boolean });
  if (!proto || proto.__fastCoords || typeof proto._getTerrainCoordsForRegularTile !== 'function') return;
  if (!Array.isArray(tm._renderableTilesKeys) || typeof tm._tiles !== 'object') return;
  const EXTENT = 8192;
  const ortho = (r: number) => {
    const m = new Float64Array(16);
    m[0] = 2 / r;
    m[5] = -2 / r;
    m[10] = -2;
    m[12] = -1;
    m[13] = 1;
    m[14] = -1;
    m[15] = 1;
    return m;
  };
  const translate = (m: Float64Array, x: number, y: number) => {
    for (let i = 0; i < 4; i++) m[12 + i] += m[i] * x + m[4 + i] * y;
  };
  proto._getTerrainCoordsForRegularTile = function (this: TerrainTiles, tileID: TileID) {
    const coords: Record<string, TileID> = {};
    const c = tileID.canonical;
    for (const key of this._renderableTilesKeys) {
      const t = this._tiles[key].tileID.canonical;
      let mat: Float64Array;
      if (t.equals(c)) {
        mat = ortho(EXTENT);
      } else if (t.z > c.z && t.isChildOf(c)) {
        const dz = t.z - c.z;
        const dx = t.x - ((t.x >> dz) << dz), dy = t.y - ((t.y >> dz) << dz);
        const size = EXTENT >> dz;
        mat = ortho(size);
        translate(mat, -dx * size, -dy * size);
      } else if (c.z > t.z && c.isChildOf(t)) {
        const dz = c.z - t.z;
        const dx = c.x - ((c.x >> dz) << dz), dy = c.y - ((c.y >> dz) << dz);
        const size = EXTENT >> dz;
        mat = ortho(EXTENT);
        translate(mat, dx * size, dy * size);
        const k = 1 / 2 ** dz;
        for (let i = 0; i < 4; i++) {
          mat[i] *= k;
          mat[4 + i] *= k;
          mat[8 + i] = 0;
        }
      } else {
        continue;
      }
      const coord = tileID.clone();
      coord.terrainRttPosMatrix32f = new Float32Array(mat);
      coords[key] = coord;
    }
    return coords;
  };
  proto.__fastCoords = true;
}

/**
 * With 3D terrain, MapLibre turns a screen point into a map point by marching a ray through the
 * terrain (screenTerrainPointToMercatorCoordinate), a tenth of a millisecond and more a point. A
 * feature query does it for its box's corners in every source it looks in, and a hover here asks a
 * dozen sources (markers, areas, ferries, roads): some 140 marches, 13 ms a hover. The results are
 * kept per point until the camera moves or the map draws again (new terrain tiles, a new frame), so
 * each distinct point is marched once.
 */
export function cacheTerrainRays(map: MLMap) {
  type Ray = (this: RayHost, p: { x: number; y: number }, terrain: unknown) => unknown;
  type RayHost = { screenTerrainPointToMercatorCoordinate: Ray; __rays?: { gen: number; terrain: unknown; m: Map<string, unknown> } };
  let gen = 0;
  const own = (o: object, k: string) => Object.prototype.hasOwnProperty.call(o, k);
  const patch = (t: unknown) => {
    // The class that defines the method (the transforms' classes may inherit from one another).
    let proto = t ? (Object.getPrototypeOf(t) as (RayHost & { __rayCache?: boolean }) | null) : null;
    while (proto && !own(proto, 'screenTerrainPointToMercatorCoordinate')) proto = Object.getPrototypeOf(proto);
    if (!proto || own(proto, '__rayCache') || typeof proto.screenTerrainPointToMercatorCoordinate !== 'function') return;
    const orig = proto.screenTerrainPointToMercatorCoordinate;
    proto.__rayCache = true;
    proto.screenTerrainPointToMercatorCoordinate = function (p, terrain) {
      const c = (this.__rays ??= { gen: -1, terrain: null, m: new Map() });
      if (c.gen !== gen || c.terrain !== terrain) {
        c.gen = gen;
        c.terrain = terrain;
        c.m.clear();
      }
      const key = `${p.x},${p.y}`;
      if (c.m.has(key)) return c.m.get(key);
      const r = orig.call(this, p, terrain);
      c.m.set(key, r);
      return r;
    };
  };
  // The transform changes with the projection (the globe's comes with the style): its classes are
  // patched as they appear.
  let seen: unknown = null;
  const ensure = () => {
    const tr = (map as unknown as { _camera?: { transform?: Record<string, unknown> } })._camera?.transform;
    if (!tr || tr === seen) return;
    seen = tr;
    for (const t of [tr, tr._mercatorTransform, tr._verticalPerspectiveTransform]) patch(t);
  };
  for (const ev of ['move', 'render', 'resize'] as const) map.on(ev, () => {
    gen++;
    ensure();
  });
  ensure();
}
