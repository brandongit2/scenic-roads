// 3D buildings (docs/buildings3d.md §4): every building of the coverage extruded to its height on
// the 3D terrain by MapLibre's fill-extrusion, from the z12–14 tiles of /tiles/buildings (layer `b`:
// h top and m base in dm, s where the height comes from, f floors, c kind, k 1 a part / 2 an outline
// with parts; crates/pipeline/src/bld). MapLibre stands each building on the terrain at its
// centroid, a base of 0 sunk 10 m so it doesn't float on a slope.
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
 * by them), and only the skyline when asked. */
function extrudedFilter(skyline: boolean): FilterSpecification {
  const f: unknown[] = ['all', ['!=', ['coalesce', ['get', 'k'], 0], 2]];
  if (skyline) f.push(['>=', ['get', 'h'], SKYLINE_DM]);
  return f as FilterSpecification;
}

/** The flat layer's filter: footprints (buildings and outlines, not parts). */
function flatFilter(skyline: boolean): FilterSpecification {
  const f: unknown[] = ['all', ['!=', ['coalesce', ['get', 'k'], 0], 1]];
  if (skyline) f.push(['>=', ['get', 'h'], SKYLINE_DM]);
  return f as FilterSpecification;
}

/** Adds the source and layers (when the catalog has buildings): the extrusions after the road and
 * rail layers (`before`: the first symbol layer), the footprints among the draped layers
 * (`flatBefore`: the first boundary layer). */
export function addBuildings(map: MLMap, before: string, flatBefore: string) {
  if (map.getSource(SOURCE)) return;
  map.addSource(SOURCE, { type: 'vector', tiles: [buildingTiles()], minzoom: 12, maxzoom: 14 });
  map.addLayer({
    id: FLAT, type: 'fill', source: SOURCE, 'source-layer': 'b', minzoom: 12, filter: flatFilter(false),
    layout: { visibility: 'none' },
    paint: { 'fill-color': PLAIN, 'fill-opacity': 0.55, 'fill-outline-color': 'rgba(20,24,30,0.6)' },
  }, flatBefore);
  map.addLayer({
    id: PICK, type: 'fill', source: SOURCE, 'source-layer': 'b', minzoom: 12, filter: extrudedFilter(false),
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
  map.setFilter(PICK, extrudedFilter(b.skyline));
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

/** The building the cursor's view ray meets first, if any (in the flat mode, the footprint under
 * it). Candidates: the footprints under the ray's ground track, from the point it meets the ground
 * back toward the camera as far as the tallest building could reach (a thin box on the screen,
 * down from the cursor); each tested against the ray between its roof and its base, as MapLibre
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
  // Where the ray is at the tallest top (700 m), on the ground: the box's bottom on the screen.
  const far = ray.at(p.x, p.y, cap(g0 + 700 * k));
  const H = map.getCanvas().clientHeight;
  const yFar = far ? map.project([far.lng, far.lat]).y : H;
  const bottom = Math.min(H, yFar >= p.y ? yFar : H);
  const cands = map.queryRenderedFeatures([[p.x - 2, p.y - 2], [p.x + 2, bottom + 2]], { layers: [PICK] });
  let best: MapGeoJSONFeature | null = null, bestAlt = -Infinity;
  const seen = new Set<string>();
  for (const f of cands) {
    if (f.geometry.type !== 'Polygon' && f.geometry.type !== 'MultiPolygon') continue;
    const key = JSON.stringify(f.geometry.coordinates).slice(0, 200);
    if (seen.has(key)) continue;
    seen.add(key);
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
