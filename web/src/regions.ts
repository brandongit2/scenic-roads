// Regions (docs/plan.md §1, §5), read only: the server's list of the region files (/api/regions),
// the outlines of the administrative areas they're made of (/api/areas/{id}), and the coverage the
// map's catalog was built for, drawn on the map (/api/coverage). The panel is ui/regions.ts.
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

/** A region as the server lists it (docs/inputs.md §6.1): a checked input in the drop box's
 * inputs/regions/, changed there only. Its outline is its parts as texts to read ("osm:<relation>",
 * "shape:<name>.geojson", or more: minus, intersect, a buffer, a cut). */
export interface Region {
  id: string;
  name: string;
  outline: string[];
  /** Its file under inputs/regions/ ("europe/portugal/azores.toml"); its folder groups the list. */
  path?: string;
  /** accepted: checked and in use; checking: being checked; held: a finding holds it back (its
   * last accepted outline stays in use). */
  state?: 'accepted' | 'checking' | 'held';
  held?: { id: string; message: string }[];
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

/** The regions; away from home, the last list read (`offline`). */
export const listRegions = () => call<{ regions: Region[]; bad: [string, string][]; offline?: boolean; tree?: Folder }>('/api/regions');
/** A folder under inputs/regions/ holding regions at some depth: they group the list. `name`: its
 * _group.toml's name, else the folder's; `folders` come sorted (by order, then name). */
export interface Folder {
  path: string;
  name: string;
  order?: number;
  regions: string[];
  folders: Folder[];
}
/**
 * The coverage the map's catalog (`catalog`, its number) was built for: each region's outline
 * entries, simplified for drawing (properties: region, region_name, entry, and an osm: entry's area
 * fields), and the regions themselves. A catalog made before catalogs recorded their coverage has
 * no `regions`: its outlines are then the recipes', and which regions it holds isn't known.
 */
export type Coverage = GeoJSON.FeatureCollection<GeoJSON.MultiPolygon> & { regions?: Region[]; catalog?: number };
/** Every region's accepted outline for drawing, a feature per stored shape (properties: region,
 * name, from, colour, held). `source` 'catalog': the catalog's coverage stands in until this Mac
 * has the accepted copy. */
export type RegionsLayer = GeoJSON.FeatureCollection<GeoJSON.MultiPolygon> & { source?: 'accepted' | 'catalog' };
export const getRegionsLayer = () => call<RegionsLayer>('/api/regions/layer');
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
  if (kind === 'shape') return `${v.replace(/\.geojson$/i, '')} (drawn)`;
  if (kind === 'poly') return `${v} (drawn)`;
  if (kind === 'place') {
    const [lon, lat, km] = v.split(',').map(Number);
    return `${fmt.km(km)} around ${fmt.coord(lat, lon).replace(/(\d+\.\d{2})\d+/g, '$1')}`;
  }
  return entry;
}


// The coverage in the app's accent; the area being looked at in a pale white no overlay uses.
const COVERAGE = '#6fd3a6';
const PREVIEW = '#dce7f5';

/**
 * The regions on the map: the coverage (every region's outline, a thin line and a faint fill), each
 * region in its own colour, and the region or view hovered in the panel.
 * Draped on the terrain under the roads and every label; nothing here takes the pointer.
 */
export class RegionLayers {
  private ready = false;
  private coverageOn = false;
  private coverage: GeoJSON.Feature[] = [];
  private regionsOn = false;
  private regions: GeoJSON.Feature[] = [];

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
    for (const src of ['coverage', 'region-hover']) map.addSource(src, { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
    map.addLayer({ id: 'coverage-fill', type: 'fill', source: 'coverage', layout: hidden, paint: { 'fill-color': COVERAGE, 'fill-opacity': 0.05 } }, before);
    map.addLayer({ id: 'coverage-line', type: 'line', source: 'coverage', layout: { ...hidden, 'line-join': 'round' }, paint: { 'line-color': COVERAGE, 'line-width': width(1), 'line-opacity': 0.65 } }, before);
    // Every region in its own colour: a light fill, so overlaps mix and gaps show, and a thin line,
    // dashed while a finding holds the region back (its last accepted outline is what's drawn).
    map.addSource('regions', { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
    const colour: ExpressionSpecification = ['coalesce', ['get', 'colour'], COVERAGE];
    const held: ExpressionSpecification = ['to-boolean', ['get', 'held']];
    map.addLayer({ id: 'regions-fill', type: 'fill', source: 'regions', layout: hidden, paint: { 'fill-color': colour, 'fill-opacity': 0.07 } }, before);
    map.addLayer({ id: 'regions-line', type: 'line', source: 'regions', filter: ['!', held], layout: { ...hidden, 'line-join': 'round' }, paint: { 'line-color': colour, 'line-width': width(1), 'line-opacity': 0.75 } }, before);
    map.addLayer({ id: 'regions-held', type: 'line', source: 'regions', filter: held, layout: { ...hidden, 'line-join': 'round' }, paint: { 'line-color': colour, 'line-width': width(1), 'line-opacity': 0.75, 'line-dasharray': [3, 2] } }, before);
    // The names on top: a symbol layer isn't draped, so it can't split the draping.
    map.addLayer({
      id: 'regions-label', type: 'symbol', source: 'regions',
      layout: { ...hidden, 'text-field': ['get', 'name'], 'text-font': ['Noto Sans Regular'], 'text-size': 11, 'text-padding': 8 },
      paint: { 'text-color': colour, 'text-halo-color': '#0b0e13', 'text-halo-width': 1.2, 'text-opacity': 0.85 },
    });
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
    const ids = src === 'regions' ? ['regions-fill', 'regions-line', 'regions-held', 'regions-label'] : [`${src}-fill`, `${src}-line`];
    for (const id of ids) if (map.getLayoutProperty(id, 'visibility') !== vis) map.setLayoutProperty(id, 'visibility', vis);
  }

  /** The coverage shown or not, and its outlines (kept for when it's shown). */
  setCoverage(on: boolean, data?: GeoJSON.FeatureCollection) {
    this.coverageOn = on;
    if (data) this.coverage = data.features;
    this.put('coverage', this.coverage, this.coverageOn);
  }

  /** The regions layer shown or not, and its features (kept for when it's shown). */
  setRegions(on: boolean, data?: GeoJSON.FeatureCollection) {
    this.regionsOn = on;
    if (data) this.regions = data.features;
    this.put('regions', this.regions, this.regionsOn);
  }

  /** The area or region hovered in the panel (none: null). */
  setHover(features: GeoJSON.Feature[] | null) {
    this.put('region-hover', features ?? []);
  }
}
