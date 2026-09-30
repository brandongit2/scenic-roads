// Terrain presentation: 3D mesh + exaggeration, hillshade, hypsometric tint, contours, sky.
import type { Map as MLMap } from 'maplibre-gl';
import * as maplibregl from 'maplibre-gl';
import mlcontour from 'maplibre-contour';
import { HYPSO } from './basemap';
import { PALETTES, baseKey, isRev, paletteFn } from './palettes';
import type { Dist } from './roads/stats';
import type { Terrain } from './state';
import { ver } from './api';

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

export interface TintContext {
  /** Current road palette key. */
  roadPalette: string;
  /** Current (animated) road colour range and whether it is an elevation range. */
  roadRange: [number, number];
  roadIsElevation: boolean;
  /** Road colours show grade (%), comparable with terrain slope. */
  roadIsGrade: boolean;
  /** Elevation distribution of roads in view. */
  elev: Dist | null;
}

let tintSig = '';

/** Default spans and custom-range limits per tint variable (m or %). */
export const TINT_VARS = {
  elev: { full: [0, 1900] as [number, number], custom: [0, 1900] as [number, number], limits: [-50, 1950, 10] as const, unit: 'm', bands: [25, 50, 100, 200, 500] },
  slope: { full: [0, 100] as [number, number], custom: [0, 60] as [number, number], limits: [0, 200, 1] as const, unit: '%', bands: [2, 5, 10, 15, 20, 25] },
};

/** Span of the tint ramp for the current settings (metres, or percent slope). */
export function tintRange(t: Terrain, ctx: TintContext): [number, number] {
  if (t.tintVar === 'slope') {
    const full = TINT_VARS.slope.full;
    switch (t.tintRange) {
      case 'custom':
        return [t.tintMin, Math.max(t.tintMin + 1, t.tintMax)];
      case 'roads':
        return ctx.roadIsGrade ? ctx.roadRange : full;
      default:
        return full; // 'view' can't be fitted for slope (no terrain statistics): full scale
    }
  }
  const fit = (): [number, number] | null => {
    const e = ctx.elev;
    if (!e || e.total <= 0) return null;
    const lo = e.quantile(0.005), hi = e.quantile(1);
    // Terrain rises above the roads: leave headroom for summits.
    return [Math.max(-20, lo - 20), Math.max(lo + 80, hi + (hi - lo) * 0.4 + 40)];
  };
  switch (t.tintRange) {
    case 'custom':
      return [t.tintMin, Math.max(t.tintMin + 10, t.tintMax)];
    case 'view':
      return fit() ?? [0, 1900];
    case 'roads':
      return ctx.roadIsElevation ? ctx.roadRange : (fit() ?? [0, 1900]);
    default:
      return [0, 1900];
  }
}

function tintFn(t: Terrain, ctx: TintContext): (u: number) => RGB {
  const base = baseKey(t.tintPalette);
  const f0 = base === 'roads' ? paletteFn(ctx.roadPalette) : (TINT_PALETTES.find((p) => p.key === base)?.fn ?? atlas);
  const fn = isRev(t.tintPalette) ? (u: number) => f0(1 - u) : f0;
  const g = t.tintCurve;
  return (u) => fn(Math.pow(Math.max(0, Math.min(1, u)), g));
}

/** Tint opacity along the ramp (same curve as the road low-end fade). */
function tintAlpha(t: Terrain): (u: number) => number {
  const f = t.tintFade[t.tintVar], sp = Math.max(0.05, t.tintFadeSpan[t.tintVar]);
  return (u) => 1 - f * Math.pow(1 - Math.max(0, Math.min(1, u / sp)), 1.5);
}

const rgba = (c: RGB, a: number) =>
  `rgba(${c.map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255)).join(',')},${Math.max(0, Math.min(1, a)).toFixed(3)})`;

/** CSS gradient of the current tint ramp (for the legend). */
export function tintCss(t: Terrain, ctx: TintContext): string {
  const f = tintFn(t, ctx);
  const al = tintAlpha(t);
  const [lo, hi] = tintRange(t, ctx);
  const parts: string[] = [];
  const B = t.tintBands;
  const n = 24;
  for (let i = 0; i <= n; i++) {
    let u = i / n;
    if (B > 0) {
      const e = lo + u * (hi - lo);
      u = ((Math.floor(e / B) + 0.5) * B - lo) / (hi - lo);
    }
    parts.push(`${rgba(f(u), al(u))} ${((i / n) * 100).toFixed(1)}%`);
  }
  return `linear-gradient(90deg, ${parts.join(', ')})`;
}

/** Update the colour-relief layer; cheap to call often (no-op when nothing changed). */
export function applyTint(map: MLMap, t: Terrain, ctx: TintContext) {
  if (!map.getLayer('tint') || !map.getLayer('tint-slope')) return;
  const id = t.tintVar === 'slope' ? 'tint-slope' : 'tint';
  map.setLayoutProperty('tint', 'visibility', t.tint && id === 'tint' ? 'visible' : 'none');
  map.setLayoutProperty('tint-slope', 'visibility', t.tint && id === 'tint-slope' ? 'visible' : 'none');
  if (!t.tint) return;
  const [lo, hi] = tintRange(t, ctx);
  const span = hi - lo;
  const sig = [id + t.tintPalette + t.tintFade[t.tintVar] + ':' + t.tintFadeSpan[t.tintVar], baseKey(t.tintPalette) === 'roads' ? ctx.roadPalette : '', t.tintBands, t.tintCurve, t.tintOpacity, lo.toFixed(1), hi.toFixed(1)].join('|');
  if (sig === tintSig) return;
  // Ignore sub-2 % drifts of a fitted range (avoids re-uploading the ramp while panning).
  const prev = tintSig.split('|');
  if (prev.length === 7 && prev.slice(0, 5).join('|') === sig.split('|').slice(0, 5).join('|')) {
    const [plo, phi] = [Number(prev[5]), Number(prev[6])];
    if (Math.abs(plo - lo) < span * 0.02 && Math.abs(phi - hi) < span * 0.02) return;
  }
  tintSig = sig;
  const f = tintFn(t, ctx);
  const al = tintAlpha(t);
  const col = (u: number) => rgba(f(u), al(u));
  let expr: unknown[];
  if (t.tintBands > 0) {
    // Bands: one colour per band, sampled at its middle. (colour-relief only accepts
    // `interpolate`, so each band edge is a pair of stops 1 cm apart.)
    let B = t.tintBands;
    while (span / B > 90) B *= 2;
    const first = Math.floor(lo / B) * B;
    const band = (e: number) => col((Math.floor(e / B) * B + B / 2 - lo) / span);
    const eps = Math.min(0.01, B / 100);
    expr = ['interpolate', ['linear'], ['elevation'], first, band(first)];
    for (let e = first + B; e < hi; e += B) expr.push(e - eps, band(e - B), e, band(e));
    expr.push(Math.max(hi, first + B) + 1, band(hi));
  } else {
    expr = ['interpolate', ['linear'], ['elevation']];
    const n = 32;
    for (let i = 0; i <= n; i++) expr.push(lo + (i / n) * span, col(i / n));
  }
  map.setPaintProperty(id, 'color-relief-color', expr as never);
  map.setPaintProperty(id, 'color-relief-opacity', t.tintOpacity);
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

let contoursReady = false;

function setupContours(map: MLMap, origin: string) {
  if (contoursReady) return;
  contoursReady = true;
  const demSource = new mlcontour.DemSource({
    url: `${origin}/tiles/terrain/{z}/{x}/{y}${ver('terrain.tiles')}`,
    encoding: 'terrarium',
    maxzoom: 12,
    worker: true,
    cacheSize: 200,
  });
  demSource.setupMaplibre(maplibregl);
  map.addSource('contours', {
    type: 'vector',
    tiles: [
      demSource.contourProtocolUrl({
        // [minor, major] interval (m) per zoom
        thresholds: { 8: [200, 1000], 10: [100, 500], 11: [50, 250], 12: [20, 100], 14: [10, 50], 16: [5, 25] },
        elevationKey: 'ele',
        levelKey: 'level',
        contourLayer: 'contours',
        overzoom: 1,
      }),
    ],
    maxzoom: 16,
  });
  map.addLayer(
    {
      id: 'contour-line',
      type: 'line',
      source: 'contours',
      'source-layer': 'contours',
      minzoom: 8,
      paint: {
        'line-color': '#a9b6c8',
        'line-opacity': ['match', ['get', 'level'], 1, 0.34, 0.16],
        'line-width': ['match', ['get', 'level'], 1, 0.9, 0.5],
      },
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
      filter: ['>', ['get', 'level'], 0],
      layout: {
        'symbol-placement': 'line',
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
  // Contours.
  if (t.contours) setupContours(map, origin);
  for (const id of ['contour-line', 'contour-label'])
    if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', t.contours ? 'visible' : 'none');
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
