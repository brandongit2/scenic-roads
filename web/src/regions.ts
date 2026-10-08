// Regions and the areas they're made of (docs/plan.md §1, §5): the server's recipes (/api/regions),
// the administrative areas to make them from (/api/areas), and the coverage the map's catalog was
// built for, drawn on the map (/api/coverage). The panel is ui/regions.ts.
import type { ExpressionSpecification, GeoJSONSource, Map as MLMap } from 'maplibre-gl';
import { fmt } from './ui/dom';

/** An administrative or ISO 3166 area from the OSM pass's outlines. */
export interface Area {
  /** Its OSM relation. */
  id: number;
  name: string;
  /** English name ('' when none). */
  en: string;
  /** admin_level (0: none, an ISO 3166 code only). */
  level: number;
  iso: string;
  /** An ISO 3166-1 country (Hong Kong is one, at admin_level 3). */
  country: boolean;
  /** The country it lies in: its ISO 3166-1 code and name ('' for a country, or outlines made
   * before this was recorded). */
  in?: string;
  in_name?: string;
  km2: number;
  bbox: [number, number, number, number];
}

/** A region's recipe: its outline is the union of the entries ("osm:<relation>", "geofabrik:<id>",
 * "poly:<file>", "place:<lon>,<lat>,<km>"). */
export interface Region {
  id: string;
  name: string;
  outline: string[];
}

const NAS_AWAY = 'The NAS isn’t reachable: try again at home';

/** A request the server refused, with its reason in plain words. */
export class RegionsError extends Error {
  constructor(readonly status: number, message: string) {
    super(message);
  }
}

async function call<T>(url: string, init?: RequestInit): Promise<T> {
  let r: Response;
  try {
    r = await fetch(url, { cache: 'no-store', ...init });
  } catch (e) {
    if ((e as Error).name === 'AbortError') throw e;
    throw new RegionsError(0, 'The map server isn’t answering');
  }
  if (r.status === 503) throw new RegionsError(503, NAS_AWAY);
  const body = (await r.json().catch(() => null)) as { error?: string } | null;
  if (!r.ok) throw new RegionsError(r.status, cap(body?.error ?? `HTTP ${r.status}`));
  return body as T;
}
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
const json = (method: string, body: unknown): RequestInit => ({ method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });

/** The regions; away from home, the last list read with the edits still waiting to go (`pending`). */
export const listRegions = () => call<{ regions: Region[]; bad: [string, string][]; pending?: number; offline?: boolean }>('/api/regions');
/** An edit: done on the NAS, or (away from home) kept on this Mac until it's reachable: `queued`. */
export type EditResult = { done?: boolean; queued?: boolean };
export const addRegion = (r: Region) => call<EditResult>('/api/regions', json('POST', r));
export const editRegion = (id: string, patch: { name?: string; outline?: string[] }) => call<EditResult>(`/api/regions/${encodeURIComponent(id)}`, json('PUT', patch));
export const removeRegion = (id: string) => call<EditResult>(`/api/regions/${encodeURIComponent(id)}`, { method: 'DELETE' });
/** What to say after an edit kept on this Mac. */
export const QUEUED = 'Saved on this Mac: it goes to the NAS when you’re home';
/** The areas containing a point, smallest first. */
export const areasAt = (lng: number, lat: number) => call<{ areas: Area[] }>(`/api/areas?at=${lng.toFixed(5)},${lat.toFixed(5)}`).then((d) => d.areas);
/** Areas by the start of their name or English name, largest first. */
export const searchAreas = (q: string, signal?: AbortSignal) => call<{ areas: Area[] }>(`/api/areas/search?${new URLSearchParams({ q })}`, { signal }).then((d) => d.areas);
/**
 * The coverage the map's catalog (`catalog`, its number) was built for: each region's outline
 * entries, simplified for drawing (properties: region, region_name, entry, and an osm: entry's area
 * fields), and the regions themselves. A catalog made before catalogs recorded their coverage has
 * no `regions`: its outlines are then the recipes', and which regions it holds isn't known.
 */
export type Coverage = GeoJSON.FeatureCollection<GeoJSON.MultiPolygon> & { regions?: Region[]; catalog?: number };
export const getCoverage = () => call<Coverage>('/api/coverage').then((fc) => ({ ...fc, features: fc.features.map(nested) }));

/** An area's outline (simplified), kept once loaded. */
const outlines = new Map<number, Promise<GeoJSON.Feature<GeoJSON.MultiPolygon>>>();
export function areaOutline(id: number): Promise<GeoJSON.Feature<GeoJSON.MultiPolygon>> {
  let p = outlines.get(id);
  if (!p) {
    p = call<GeoJSON.Feature<GeoJSON.MultiPolygon>>(`/api/areas/${id}`).then(nested);
    outlines.set(id, p);
    p.catch(() => outlines.get(id) === p && outlines.delete(id));
  }
  return p;
}

type Ring = GeoJSON.Position[];
const ringBox = (r: Ring) => {
  let w = Infinity, s = Infinity, e = -Infinity, n = -Infinity;
  for (const [x, y] of r) {
    w = Math.min(w, x);
    s = Math.min(s, y);
    e = Math.max(e, x);
    n = Math.max(n, y);
  }
  return [w, s, e, n];
};
const inRing = (r: Ring, [x, y]: GeoJSON.Position) => {
  let inside = false;
  for (let i = 0, j = r.length - 1; i < r.length; j = i++) {
    if (r[i][1] > y !== r[j][1] > y && x < ((r[j][0] - r[i][0]) * (y - r[i][1])) / (r[j][1] - r[i][1]) + r[i][0]) inside = !inside;
  }
  return inside;
};

/**
 * An outline's rings as polygons with their holes. Outlines from passes before ring kinds were
 * recorded come with every ring as a polygon of its own, read even–odd (a ring inside another is a
 * hole in it, an island in that hole a ring again), which MapLibre would fill solid, an enclave over
 * its surroundings; newer ones come nested. Either way the rings are nested again by containment,
 * each hole with the smallest ring around it.
 */
function nested(f: GeoJSON.Feature<GeoJSON.MultiPolygon>): GeoJSON.Feature<GeoJSON.MultiPolygon> {
  const rings = f.geometry.coordinates.flat(1);
  if (rings.length < 2) return f;
  const box = rings.map(ringBox);
  const around = rings.map((r, i) =>
    rings.flatMap((_, j) => (j !== i && box[j][0] <= box[i][0] && box[j][1] <= box[i][1] && box[j][2] >= box[i][2] && box[j][3] >= box[i][3] && inRing(rings[j], r[0]) ? [j] : [])),
  );
  const depth = around.map((a) => a.length);
  const size = box.map((b) => (b[2] - b[0]) * (b[3] - b[1]));
  const polys = new Map<number, Ring[]>();
  rings.forEach((r, i) => depth[i] % 2 === 0 && polys.set(i, [r]));
  rings.forEach((r, i) => {
    if (depth[i] % 2 === 0) return;
    const outer = around[i].filter((j) => depth[j] === depth[i] - 1).sort((a, b) => size[a] - size[b])[0];
    if (outer === undefined) polys.set(i, [r]);
    else polys.get(outer)!.push(r);
  });
  return { ...f, geometry: { type: 'MultiPolygon', coordinates: [...polys.values()] } };
}

const LEVELS: Record<number, string> = { 2: 'country', 3: 'region', 4: 'state/province', 5: 'region', 6: 'county', 7: 'district', 8: 'municipality' };
/** What kind of area it is, by its admin level (its name varies by country: a level 6 is a county
 * in England, a département in France). */
export const levelName = (a: Pick<Area, 'level' | 'country' | 'iso'>): string =>
  a.country ? 'country' : LEVELS[a.level] ?? ((a.iso ?? '').includes('-') ? 'subdivision' : 'area');

/** An area's name to show: English when it has one. */
export const areaName = (a: Pick<Area, 'name' | 'en'>): string => a.en || a.name;

export const km2 = (v: number) => (v < 1 ? '<1 km²' : `${fmt.n(v)} km²`);

/** An outline entry in words: "Northumberland (county)", "Kanto (Geofabrik)", "40 km around …".
 * `area`: an osm: entry's area, when known (the coverage's features have none when the server
 * can't read the pass's outlines: away from the NAS, not on this Mac). */
export function entryLabel(entry: string, area?: Pick<Area, 'name' | 'en' | 'level' | 'country' | 'iso'>): string {
  const [kind, v = ''] = entry.split(/:(.*)/s);
  if (kind === 'osm') return area && typeof area.level === 'number' ? `${areaName(area)} (${levelName(area)})` : `OSM relation ${v}`;
  if (kind === 'geofabrik') {
    const last = v.split('/').pop() ?? v;
    return `${last.split('-').map(cap).join(' ')} (Geofabrik)`;
  }
  if (kind === 'poly') return `${v} (drawn)`;
  if (kind === 'place') {
    const [lon, lat, km] = v.split(',').map(Number);
    return `${fmt.km(km)} around ${fmt.coord(lat, lon).replace(/(\d+\.\d{2})\d+/g, '$1')}`;
  }
  return entry;
}

/** A region id from a name: lower-case letters, digits and dashes. */
export const slug = (s: string): string =>
  s.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+/, '').slice(0, 64).replace(/-+$/, '');
/** The server's rule for ids (pipeline::agent::recipes::valid_id). */
export const validId = (id: string) => /^[a-z0-9][a-z0-9-]{0,63}$/.test(id);

// The coverage in the app's accent; the areas being looked at in a pale white no overlay uses.
const COVERAGE = '#6fd3a6';
const PREVIEW = '#dce7f5';

/**
 * The regions on the map: the coverage (every region's outline, a thin line and a faint fill) and
 * the areas being looked at in the panel (the one hovered, the ones chosen for a new region).
 * Draped on the terrain under the roads and every label; nothing here takes the pointer.
 */
export class RegionLayers {
  private ready = false;
  private coverageOn = false;
  private coverage: GeoJSON.Feature[] = [];

  constructor(private map: MLMap) {}

  /** The sources and layers, once the style is there: above the draped layers (terrain, water,
   * areas), below the roads' and contours' own layers. */
  private ensure(): boolean {
    const map = this.map;
    if (this.ready) return true;
    if (!(map as unknown as { style?: { _loaded?: boolean } }).style?._loaded) return false;
    // (Under the first layer that isn't draped: one between draped layers would split the draping
    // in two.)
    const before = ['contours-3d', 'roads'].find((id) => map.getLayer(id));
    const width = (w: number): ExpressionSpecification => ['interpolate', ['linear'], ['zoom'], 3, w * 0.8, 10, w * 1.2, 15, w * 1.8];
    const hidden = { visibility: 'none' as const };
    for (const src of ['coverage', 'region-draft', 'region-hover']) map.addSource(src, { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
    map.addLayer({ id: 'coverage-fill', type: 'fill', source: 'coverage', layout: hidden, paint: { 'fill-color': COVERAGE, 'fill-opacity': 0.05 } }, before);
    map.addLayer({ id: 'coverage-line', type: 'line', source: 'coverage', layout: { ...hidden, 'line-join': 'round' }, paint: { 'line-color': COVERAGE, 'line-width': width(1), 'line-opacity': 0.65 } }, before);
    map.addLayer({ id: 'region-draft-fill', type: 'fill', source: 'region-draft', layout: hidden, paint: { 'fill-color': PREVIEW, 'fill-opacity': 0.08 } }, before);
    map.addLayer({ id: 'region-draft-line', type: 'line', source: 'region-draft', layout: { ...hidden, 'line-join': 'round' }, paint: { 'line-color': PREVIEW, 'line-width': width(1.4), 'line-opacity': 0.9 } }, before);
    map.addLayer({ id: 'region-hover-fill', type: 'fill', source: 'region-hover', layout: hidden, paint: { 'fill-color': PREVIEW, 'fill-opacity': 0.05 } }, before);
    map.addLayer({ id: 'region-hover-line', type: 'line', source: 'region-hover', layout: { ...hidden, 'line-join': 'round' }, paint: { 'line-color': PREVIEW, 'line-width': width(1.2), 'line-dasharray': [2, 1.5], 'line-opacity': 0.9 } }, before);
    this.ready = true;
    return true;
  }

  /** A source's features, and its layers shown while it has some (and `on`). Hidden rather than
   * emptied: with 3D terrain an emptied source leaves its last outlines in the draped textures,
   * which a change of the layers' visibility redraws. */
  private put(src: string, features: GeoJSON.Feature[], on = true) {
    if (!this.ready && !(on && features.length)) return;
    if (!this.ensure()) return;
    const map = this.map;
    const vis = on && features.length ? 'visible' : 'none';
    if (features.length) map.getSource<GeoJSONSource>(src)?.setData({ type: 'FeatureCollection', features });
    for (const id of [`${src}-fill`, `${src}-line`]) if (map.getLayoutProperty(id, 'visibility') !== vis) map.setLayoutProperty(id, 'visibility', vis);
  }

  /** The coverage shown or not, and its outlines (kept for when it's shown). */
  setCoverage(on: boolean, data?: GeoJSON.FeatureCollection) {
    this.coverageOn = on;
    if (data) this.coverage = data.features;
    this.put('coverage', this.coverage, this.coverageOn);
  }

  /** The areas chosen for a new region (their union). */
  setDraft(features: GeoJSON.Feature[]) {
    this.put('region-draft', features);
  }

  /** The area or region hovered in the panel (none: null). */
  setHover(features: GeoJSON.Feature[] | null) {
    this.put('region-hover', features ?? []);
  }
}
