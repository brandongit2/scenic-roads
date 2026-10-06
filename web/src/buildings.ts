// 3D buildings (docs/buildings3d.md §4): every building of the coverage extruded to its height on
// the 3D terrain by MapLibre's fill-extrusion, from the z12–14 tiles of /tiles/buildings (layer `b`:
// h top and m base in dm, s where the height comes from, f floors, c kind, k 1 a part / 2 an outline
// with parts, o 1 a copy for the flat footprints; crates/pipeline/src/bld). A building is whole in
// its centroid's tile, and the z14 tiles stay whole at every zoom above (keepWhole). MapLibre
// stands each building on the terrain at its centroid, a base of 0 sunk 10 m so it doesn't float
// on a slope.
//
// A source and three layers: `buildings` (fill-extrusion: the parts, and the buildings without
// parts), `buildings-flat` (fill, draped: footprints, the flat mode and 2D maps) and
// `buildings-hover` (the hovered building, a little larger, lit). Settings → Buildings: 3D or flat,
// colour (plain, by height, by where the height comes from), opacity, height scale, skyline only.

import type { ExpressionSpecification, FilterSpecification, GeoJSONSource, Map as MLMap, MapGeoJSONFeature } from 'maplibre-gl';
import { paletteRgb } from './palettes';
import { hostFor } from './hosts';
import { ver } from './api';
import type { FeatureSummary } from './overlays';

export type BuildingColour = 'plain' | 'height' | 'source';

export interface BuildingState {
  on: boolean;
  /** Footprints only (also what a map without 3D terrain shows). */
  flat: boolean;
  colour: BuildingColour;
  opacity: number;
  /** Heights × this (1–3), or 0: × the terrain's exaggeration. */
  scale: number;
  /** Only the skyline: buildings 40 m tall or more. */
  skyline: boolean;
}

export const SOURCE = 'bld';
export const LAYER = 'buildings';
export const FLAT = 'buildings-flat';
export const HOVER = 'buildings-hover';
/** The extruded buildings' footprints, transparent (a fill at opacity 0 isn't drawn): what the
 * hover finds candidates in (MapLibre's own query of extrusions ignores the terrain, and finds
 * nothing on the globe). */
export const PICK = 'buildings-pick';
/** The skyline (the z12 tiles' threshold), dm. */
export const SKYLINE_DM = 400;
/** The tallest a building can be (the pipeline's bound on heights taken), dm. */
const TALLEST_DM = 7000;

/** Where a height comes from (`s`, docs/buildings3d.md §2.3): the hover's words and the source
 * colouring's colours. */
export const SOURCES: [string, string, string][] = [
  ['measured', '#5ec2a8', 'Measured: lidar, OpenStreetMap, Esri or a city'],
  ['from floors', '#6aa8ff', 'From its number of floors, times the country’s storey height'],
  ['estimated by Microsoft', '#b48cf2', 'Microsoft’s machine-learning estimate (they grow about 1.2 m a floor)'],
  ['estimated from neighbours', '#e8b04a', 'The median of the measured heights around it, of similar footprints'],
  ['estimated, GHSL', '#ef7a5a', 'GHSL’s average building height of its 90 m cell, in high-rise cores'],
  ['estimated from its size', '#8b95a3', 'The measured median of its kind and size in its country'],
];
/** `c`, the kind (from Overture's subtype). */
export const KINDS = ['', 'residential', 'outbuilding', 'commercial', 'industrial', 'religious', 'civic', 'agricultural', 'transportation', 'other'];

const PLAIN = '#566173';
const HEIGHT_PALETTE = 'viridis';
/** By height: the colour ramp's top (m). */
export const HEIGHT_TOP_M = 150;

/** The tiles' URL (versioned: the catalog's `buildings.tiles`). */
export const buildingTiles = (): string => `${hostFor('buildings')}/tiles/buildings/{z}/{x}/{y}${ver('buildings.tiles')}`;

/** The colour expression of a mode. */
function colourOf(mode: BuildingColour): ExpressionSpecification | string {
  if (mode === 'source') return ['match', ['get', 's'], ...SOURCES.flatMap(([, c], i) => [i, c]), PLAIN] as unknown as ExpressionSpecification;
  if (mode === 'height') {
    const stops: (number | string)[] = [];
    for (let i = 0; i <= 8; i++) {
      const t = i / 8;
      // (A square-root ramp: most buildings are low.)
      stops.push(Math.round(HEIGHT_TOP_M * 10 * t * t), paletteRgb(HEIGHT_PALETTE, 0.08 + 0.92 * t));
    }
    return ['interpolate', ['linear'], ['get', 'h'], ...stops] as unknown as ExpressionSpecification;
  }
  return PLAIN;
}

/** The height colouring's legend stops (m, colour). */
export function heightLegend(): [number, string][] {
  return Array.from({ length: 9 }, (_, i) => {
    const t = i / 8;
    return [HEIGHT_TOP_M * t * t, paletteRgb(HEIGHT_PALETTE, 0.08 + 0.92 * t)] as [number, string];
  });
}

/** Heights × the scale (`s`: the state's, or the terrain's exaggeration). */
const metres = (prop: 'h' | 'm', k: number): ExpressionSpecification => ['*', ['/', ['coalesce', ['get', prop], 0], 10], k];

/** The extruded layer's filter: parts and buildings without parts (an outline with parts is drawn
 * by them), not the copies (`o`: a building reaching into a tile next to its own is copied there,
 * whole, for the flat footprints, which are cut at their tile's edge), and only the skyline when
 * asked. The pick layer's takes the copies too: a building is found from the tile it reaches into. */
function extrudedFilter(skyline: boolean, copies = false): FilterSpecification {
  const f: unknown[] = ['all', ['!=', ['coalesce', ['get', 'k'], 0], 2]];
  if (!copies) f.push(['!', ['has', 'o']]);
  if (skyline) f.push(['>=', ['get', 'h'], SKYLINE_DM]);
  return f as FilterSpecification;
}

/** The flat layer's filter: footprints (buildings and outlines, not parts; copies too). */
function flatFilter(skyline: boolean): FilterSpecification {
  const f: unknown[] = ['all', ['!=', ['coalesce', ['get', 'k'], 0], 1]];
  if (skyline) f.push(['>=', ['get', 'h'], SKYLINE_DM]);
  return f as FilterSpecification;
}

/** The buildings' tiles kept out of view, at most: about a view's worth of z14 tiles. MapLibre's
 * own is five zooms' worth of the tiles in view (~60), which, with one whole z14 tile for every
 * zoom above, only keeps places panned away from: after panning around Tokyo it held 437 MB of
 * buffers against 63 MB in view; 8 hold 54 (docs/buildings3d.md §4.6). Further tiles come back
 * from the browser's cache. */
const CACHE_TILES = 8;

/**
 * The z14 tiles whole at every zoom above 14, one tile for all of them. MapLibre slices a vector
 * source's deepest tiles into z15–16 pieces above (its `zoomLevelsToOverscale`), each clipped at
 * its edges and standing on the terrain at its own centroid: roofs stepped on slopes where a
 * building crosses a slice's edge, its overhang past the z14 tile cut off, a hovered building lit in
 * part; and parses each again for every zoom. For this source only: slicing stays for the others
 * (the map-wide option made their re-parsed copies cost more than the buildings saved). Also its
 * cache bounded ([`CACHE_TILES`]).
 */
function keepWhole(map: MLMap) {
  type TM = { update: (tr: unknown, terrain?: unknown) => void; map?: { _zoomLevelsToOverscale?: number }; _maxTileCacheSize?: number | null };
  const src = map.getSource(SOURCE) as unknown as { reparseOverscaled?: boolean } | undefined;
  const tm = (map as unknown as { style?: { tileManagers?: Record<string, TM> } }).style?.tileManagers?.[SOURCE];
  if (!src || !tm || typeof tm.update !== 'function') return;
  // (Not parsed again for each zoom: an extrusion doesn't change with it.)
  src.reparseOverscaled = false;
  tm._maxTileCacheSize = CACHE_TILES;
  const update = tm.update;
  tm.update = function (this: TM, tr: unknown, terrain?: unknown) {
    const m = this.map;
    const z = m?._zoomLevelsToOverscale;
    if (m) m._zoomLevelsToOverscale = undefined;
    try {
      update.call(this, tr, terrain);
    } finally {
      if (m) m._zoomLevelsToOverscale = z;
    }
  };
}

/** Adds the source and layers (when the catalog has buildings): the extrusions after the road and
 * rail layers (`before`: the first symbol layer), the footprints among the draped layers
 * (`flatBefore`: the first boundary layer). */
export function addBuildings(map: MLMap, before: string, flatBefore: string) {
  if (map.getSource(SOURCE)) return;
  map.addSource(SOURCE, { type: 'vector', tiles: [buildingTiles()], minzoom: 12, maxzoom: 14 });
  keepWhole(map);
  map.addLayer({
    id: FLAT, type: 'fill', source: SOURCE, 'source-layer': 'b', minzoom: 12, filter: flatFilter(false),
    layout: { visibility: 'none' },
    paint: { 'fill-color': PLAIN, 'fill-opacity': 0.55, 'fill-outline-color': 'rgba(20,24,30,0.6)' },
  }, flatBefore);
  map.addLayer({
    id: PICK, type: 'fill', source: SOURCE, 'source-layer': 'b', minzoom: 12, filter: extrudedFilter(false, true),
    paint: { 'fill-color': '#000000', 'fill-opacity': 0 },
  }, flatBefore);
  map.addLayer({
    id: LAYER, type: 'fill-extrusion', source: SOURCE, 'source-layer': 'b', minzoom: 12, filter: extrudedFilter(false),
    paint: { 'fill-extrusion-color': PLAIN, 'fill-extrusion-height': metres('h', 1), 'fill-extrusion-base': metres('m', 1), 'fill-extrusion-vertical-gradient': true },
  }, before);
  map.addSource('bld-hover', { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
  map.addLayer({
    id: HOVER, type: 'fill-extrusion', source: 'bld-hover',
    paint: { 'fill-extrusion-color': '#ffd27a', 'fill-extrusion-opacity': 0.7, 'fill-extrusion-height': ['get', 'top'], 'fill-extrusion-base': ['get', 'base'], 'fill-extrusion-vertical-gradient': false },
  }, before);
}

/** The settings applied: visibility, colour, opacity, scale, filters, light. `exaggeration`: the
 * terrain's (0: no 3D terrain, the footprints then drawn flat); `light`: the hill-shading's
 * azimuth (degrees). */
export function applyBuildings(map: MLMap, b: BuildingState, exaggeration: number, light: number) {
  if (!map.getLayer(LAYER)) return;
  const flat = b.flat || exaggeration <= 0;
  const k = b.scale > 0 ? b.scale : Math.max(1, exaggeration);
  map.setLayoutProperty(LAYER, 'visibility', b.on && !flat ? 'visible' : 'none');
  map.setLayoutProperty(FLAT, 'visibility', b.on && flat ? 'visible' : 'none');
  map.setLayoutProperty(HOVER, 'visibility', b.on && !flat ? 'visible' : 'none');
  map.setLayoutProperty(PICK, 'visibility', b.on && !flat ? 'visible' : 'none');
  const c = colourOf(b.colour);
  map.setPaintProperty(LAYER, 'fill-extrusion-color', c);
  map.setPaintProperty(FLAT, 'fill-color', c);
  map.setPaintProperty(LAYER, 'fill-extrusion-opacity', b.opacity);
  map.setPaintProperty(FLAT, 'fill-opacity', 0.65 * b.opacity);
  map.setPaintProperty(LAYER, 'fill-extrusion-height', metres('h', k));
  map.setPaintProperty(LAYER, 'fill-extrusion-base', metres('m', k));
  map.setFilter(LAYER, extrudedFilter(b.skyline));
  map.setFilter(PICK, extrudedFilter(b.skyline, true));
  map.setFilter(FLAT, flatFilter(b.skyline));
  // Lit from the hill-shading's light, low, so the roofs are a little brighter than the walls.
  map.setLight({ anchor: 'map', position: [1.5, ((light % 360) + 360) % 360, 40], intensity: 0.35, color: '#ffffff' });
}

/** A new catalog's tiles. */
export function switchBuildings(map: MLMap) {
  (map.getSource(SOURCE) as { setTiles?: (t: string[]) => void } | undefined)?.setTiles?.([buildingTiles()]);
}

/** Where a screen point's view ray is at a height (camera3d.rayAt: null at or above the camera),
 * and the camera's altitude. */
export interface Ray {
  at: (px: number, py: number, elev: number) => { lng: number; lat: number } | null;
  camera: number;
}

/** The vertices of a polygon's rings, (lng × cos lat, lat): a local plane for crossing tests. */
type Ring = [number, number][];

/** Whether point q is inside the rings (even–odd). */
function inside(rings: Ring[], q: [number, number]): boolean {
  let c = false;
  for (const r of rings) {
    for (let i = 0, j = r.length - 1; i < r.length; j = i++) {
      if ((r[i][1] > q[1]) !== (r[j][1] > q[1]) && q[0] < ((r[j][0] - r[i][0]) * (q[1] - r[i][1])) / (r[j][1] - r[i][1]) + r[i][0]) c = !c;
    }
  }
  return c;
}

/** The first point (as a fraction from a) where segment a–b crosses a ring, if it does. */
function firstCrossing(rings: Ring[], a: [number, number], b: [number, number]): number | null {
  let best: number | null = null;
  const dx = b[0] - a[0], dy = b[1] - a[1];
  for (const r of rings) {
    for (let i = 0, j = r.length - 1; i < r.length; j = i++) {
      const ex = r[i][0] - r[j][0], ey = r[i][1] - r[j][1];
      const den = dx * ey - dy * ex;
      if (den === 0) continue;
      const fx = r[j][0] - a[0], fy = r[j][1] - a[1];
      const t = (fx * ey - fy * ex) / den, u = (fx * dy - fy * dx) / den;
      if (t >= 0 && t <= 1 && u >= 0 && u <= 1 && (best === null || t < best)) best = t;
    }
  }
  return best;
}

/** A query box on the screen and the least height (dm) a footprint in it needs to reach the ray. */
type TrackBox = { box: [[number, number], [number, number]]; minDm: number };

/** The ray's ground track on the screen in boxes, each with the height a building in it must reach
 * to meet the ray there: points every 1/48 of the height from the ground `g0` to `top` (the ray at
 * that height, on the ground below it), cut where they leave the screen, grouped into boxes ~24 px
 * across and at most 160 px long (a query's own cost, its corners found on the terrain, is most of
 * it), 3 px around. A box's height is the ray's at its near end, less the ground there and some. */
function trackBoxes(map: MLMap, p: { x: number; y: number }, ray: Ray, g0: number, top: number, k: number, slack: number): TrackBox[] {
  const W = map.getCanvas().clientWidth, H = map.getCanvas().clientHeight;
  const pts: { x: number; y: number; a: number; g: number }[] = [{ x: p.x, y: p.y, a: g0, g: g0 }];
  for (let i = 1; i <= 48; i++) {
    const a = g0 + ((top - g0) * i) / 48;
    const ll = ray.at(p.x, p.y, a);
    if (!ll) break;
    const s = map.project([ll.lng, ll.lat]);
    if (!Number.isFinite(s.x) || !Number.isFinite(s.y)) break;
    const last = pts[pts.length - 1];
    if (s.x < 0 || s.x > W || s.y < 0 || s.y > H) {
      // (Off the screen: the track to the screen's edge.)
      const c = clipTo(last, s, W, H);
      if (Math.hypot(c.x - last.x, c.y - last.y) > 0.5) pts.push({ ...c, a, g: map.queryTerrainElevation(ll) ?? last.g });
      break;
    }
    pts.push({ x: s.x, y: s.y, a, g: map.queryTerrainElevation(ll) ?? last.g });
  }
  const out: TrackBox[] = [];
  let j = 0;
  while (j < pts.length - 1) {
    let m = j + 1;
    const span = (to: number) => {
      const xs = pts.slice(j, to + 1).map((q) => q.x), ys = pts.slice(j, to + 1).map((q) => q.y);
      const w = Math.max(...xs) - Math.min(...xs), h = Math.max(...ys) - Math.min(...ys);
      return Math.min(w, h) <= 24 && Math.max(w, h) <= 160;
    };
    while (m + 1 < pts.length && span(m + 1)) m++;
    const seg = pts.slice(j, m + 1);
    const xs = seg.map((q) => q.x), ys = seg.map((q) => q.y);
    const g = Math.max(...seg.map((q) => q.g));
    const minDm = Math.floor(((pts[j].a - g - slack) / k) * 10);
    out.push({ box: [[Math.min(...xs) - 3, Math.min(...ys) - 3], [Math.max(...xs) + 3, Math.max(...ys) + 3]], minDm });
    j = m;
  }
  return out;
}

/** Segment p–q cut at the box 0–w × 0–h (p inside it): q, or where the segment leaves the box. */
function clipTo(p: { x: number; y: number }, q: { x: number; y: number }, w: number, h: number): { x: number; y: number } {
  let t = 1;
  const dx = q.x - p.x, dy = q.y - p.y;
  if (dx > 0) t = Math.min(t, (w - p.x) / dx);
  if (dx < 0) t = Math.min(t, -p.x / dx);
  if (dy > 0) t = Math.min(t, (h - p.y) / dy);
  if (dy < 0) t = Math.min(t, -p.y / dy);
  t = Math.max(0, t);
  return { x: p.x + dx * t, y: p.y + dy * t };
}

/** A feature's key for telling queries' answers apart: its tile and its place in the tile's data
 * (MapLibre's fields; else its geometry). */
function featureKey(f: MapGeoJSONFeature): string {
  const g = f as unknown as { _x?: number; _y?: number; _z?: number; _vectorTileFeature?: { _geometry?: number } };
  const at = g._vectorTileFeature?._geometry;
  return at !== undefined && g._z !== undefined ? `${g._z}/${g._x}/${g._y}/${at}` : JSON.stringify(f.geometry);
}

/** The building the cursor's view ray meets first, if any (in the flat mode, the footprint under
 * it). Candidates: the footprints under the ray's ground track, from the point it meets the ground
 * back toward the camera as far as the tallest building could reach: on the screen, from the
 * cursor toward the point under the ray at that height (toward the camera's nadir, below the
 * screen's middle), in boxes along it, each taking only footprints tall enough to reach the ray
 * there (`trackBoxes`); each tested against the ray between its roof and its base, as MapLibre
 * draws it (on the terrain at its centroid, a base of 0 sunk 10 m). The one met highest wins. */
export function buildingAt(map: MLMap, p: { x: number; y: number }, b: BuildingState, exaggeration: number, ray: Ray): { f: MapGeoJSONFeature; alt: number } | null {
  if (!b.on || !map.getLayer(PICK)) return null;
  if (map.getLayoutProperty(FLAT, 'visibility') === 'visible') {
    const f = map.queryRenderedFeatures([p.x, p.y], { layers: [FLAT] })[0];
    return f ? { f, alt: -Infinity } : null;
  }
  if (map.getLayoutProperty(PICK, 'visibility') !== 'visible') return null;
  const k = b.scale > 0 ? b.scale : Math.max(1, exaggeration);
  // (Heights a little under the camera's: the ray is only below it.)
  const cap = (e: number) => Math.min(e, ray.camera - 0.5);
  const ground = ray.at(p.x, p.y, cap(0));
  if (!ground) return null;
  const g0 = map.queryTerrainElevation(ground) ?? 0;
  // The track from the cursor to where the ray is at the tallest top (700 m), each box's footprints
  // those tall enough to reach the ray over it (the terrain and its slopes allowed for).
  const slack = 20 * Math.max(1, exaggeration);
  const cands = trackBoxes(map, p, ray, g0, cap(g0 + (TALLEST_DM / 10) * k), k, slack).flatMap(({ box, minDm }) =>
    minDm > TALLEST_DM ? [] : map.queryRenderedFeatures(box, { layers: [PICK], ...(minDm > 0 ? { filter: ['>=', ['get', 'h'], minDm] as FilterSpecification } : {}) }),
  );
  let best: MapGeoJSONFeature | null = null, bestAlt = -Infinity;
  const seen = new Set<string>();
  for (const f of cands) {
    const key = featureKey(f);
    if (seen.has(key)) continue;
    seen.add(key);
    if (f.geometry.type !== 'Polygon' && f.geometry.type !== 'MultiPolygon') continue;
    const polys = f.geometry.type === 'Polygon' ? [f.geometry.coordinates] : f.geometry.coordinates;
    const pr = f.properties as Record<string, number>;
    const top0 = ((pr.h ?? 0) / 10) * k, base0 = ((pr.m ?? 0) / 10) * k;
    for (const poly of polys) {
      // MapLibre's centroid of a polygon: its rings' vertices' mean (the closing ones left out).
      let cx = 0, cy = 0, n = 0;
      for (const ring of poly) {
        const m = ring.length > 1 && ring[0][0] === ring[ring.length - 1][0] && ring[0][1] === ring[ring.length - 1][1] ? ring.length - 1 : ring.length;
        for (let i = 0; i < m; i++) {
          cx += ring[i][0];
          cy += ring[i][1];
          n++;
        }
      }
      if (!n) continue;
      const g = map.queryTerrainElevation([cx / n, cy / n]) ?? 0;
      const top = cap(g + top0), base = cap(g + (base0 > 0 ? base0 : -10));
      if (top <= base) continue;
      const a = ray.at(p.x, p.y, top), z = ray.at(p.x, p.y, base);
      if (!a || !z) continue;
      const cos = Math.cos(((cy / n) * Math.PI) / 180);
      const rings: Ring[] = poly.map((r) => r.map(([x, y]) => [x * cos, y] as [number, number]));
      const A: [number, number] = [a.lng * cos, a.lat], Z: [number, number] = [z.lng * cos, z.lat];
      const t = inside(rings, A) ? 0 : firstCrossing(rings, A, Z);
      if (t === null) continue;
      const alt = top - t * (top - base);
      if (alt > bestAlt) {
        bestAlt = alt;
        best = f;
      }
    }
  }
  return best ? { f: best, alt: bestAlt } : null;
}

/** Whether a point of a road or rail line under the cursor is hidden by a building: the view ray
 * to it meets one above it (a line drawn on the terrain; bridges and elevated rail aside). */
export function hiddenByBuilding(map: MLMap, ll: [number, number], b: BuildingState, exaggeration: number, ray: Ray): boolean {
  const q = map.project(ll);
  const hit = buildingAt(map, q, b, exaggeration, ray);
  return !!hit && hit.alt > (map.queryTerrainElevation(ll) ?? 0) + 2;
}

/** What the bottom bar says of a building: its height, where the height comes from ("from 6
 * floors", "estimated by Microsoft"…), its base when it has one, its kind. */
export function summarise(f: MapGeoJSONFeature): FeatureSummary {
  const p = f.properties as Record<string, number>;
  const h = (p.h ?? 0) / 10, m = (p.m ?? 0) / 10, s = p.s ?? 5;
  const [how, colour] = SOURCES[s] ?? SOURCES[5];
  const facts: string[] = [];
  if (m > 0) facts.push(`from ${fmtM(m)} up`);
  const kind = KINDS[p.c ?? 0];
  if (kind) facts.push(kind);
  return {
    title: `${p.k === 1 ? 'Building part' : 'Building'} · ${fmtM(h)}`,
    kind: s === 1 && p.f ? `from ${p.f} floor${p.f === 1 ? '' : 's'}` : how,
    colour,
    facts,
    source: 'Overture Maps (OpenStreetMap, Microsoft, Esri, USGS and others)',
    area: false,
  };
}

const fmtM = (v: number) => `${v < 10 ? v.toFixed(1) : Math.round(v)} m`;

// ---- the hovered building ------------------------------------------------------------------

/** A ring moved outward (`d` metres), or inward for a hole, so the highlight encloses the building's
 * walls: each vertex along its corner's bisector (the mitre capped). */
function offsetRing(ring: number[][], d: number, grow: boolean): number[][] {
  const n = ring.length - (ring.length > 1 && ring[0][0] === ring[ring.length - 1][0] && ring[0][1] === ring[ring.length - 1][1] ? 1 : 0);
  if (n < 3) return ring;
  const lat0 = ring[0][1];
  const kx = 111320 * Math.cos((lat0 * Math.PI) / 180), ky = 110574;
  const pts = ring.slice(0, n).map(([x, y]) => [(x - ring[0][0]) * kx, (y - lat0) * ky]);
  let a = 0;
  for (let i = 0; i < n; i++) {
    const [x1, y1] = pts[i], [x2, y2] = pts[(i + 1) % n];
    a += x1 * y2 - x2 * y1;
  }
  // Outward (away from the ring's inside) is the right of each edge for a counter-clockwise ring.
  const out = (a > 0 ? 1 : -1) * (grow ? 1 : -1);
  const norm = (i: number) => {
    const [x1, y1] = pts[i], [x2, y2] = pts[(i + 1) % n];
    const l = Math.hypot(x2 - x1, y2 - y1) || 1;
    return [(out * (y2 - y1)) / l, (-out * (x2 - x1)) / l];
  };
  const res = pts.map((_, i) => {
    const n1 = norm((i + n - 1) % n), n2 = norm(i);
    let bx = n1[0] + n2[0], by = n1[1] + n2[1];
    const bl = Math.hypot(bx, by) || 1;
    bx /= bl;
    by /= bl;
    const k = d / Math.max(0.4, bx * n2[0] + by * n2[1]);
    return [ring[0][0] + (pts[i][0] + bx * k) / kx, lat0 + (pts[i][1] + by * k) / ky];
  });
  res.push(res[0]);
  return res;
}

/** How much larger and taller the hovered building's highlight is than the building (m): enough
 * to show over it (each draws on the terrain at its own centroid). A tower's other parts and nearer
 * buildings still hide it where they're in front. */
const HOVER_GROW_M = 1;

/** The hovered building highlighted (null: none): its footprint a little larger and taller. */
export function setHovered(map: MLMap, f: MapGeoJSONFeature | null, b: BuildingState, exaggeration: number) {
  const src = map.getSource<GeoJSONSource>('bld-hover');
  if (!src) return;
  if (!f || (f.geometry.type !== 'Polygon' && f.geometry.type !== 'MultiPolygon')) {
    src.setData({ type: 'FeatureCollection', features: [] });
    return;
  }
  const k = b.scale > 0 ? b.scale : Math.max(1, exaggeration);
  const p = f.properties as Record<string, number>;
  const polys = f.geometry.type === 'Polygon' ? [f.geometry.coordinates] : f.geometry.coordinates;
  const grown = polys.map((poly) => poly.map((ring, i) => offsetRing(ring, HOVER_GROW_M, i === 0)));
  src.setData({
    type: 'Feature',
    properties: { top: ((p.h ?? 0) / 10) * k + HOVER_GROW_M, base: ((p.m ?? 0) / 10) * k },
    geometry: { type: 'MultiPolygon', coordinates: grown },
  });
}
