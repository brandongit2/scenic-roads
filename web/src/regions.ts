// Regions (docs/surface.md, docs/regions.md), read only: the server's list of the region files
// (/api/regions) and their outlines simplified for a zoom (/api/regions/outlines), drawn on the map
// while "Show regions on map" is on. The panel is ui/regions.ts.
import type { ExpressionSpecification, GeoJSONSource, Map as MLMap } from 'maplibre-gl';

/** An administrative or ISO 3166 area's fields a region's flag comes from (its first osm: part's). */
export interface Area {
  iso: string;
  /** An ISO 3166-1 country (Hong Kong is one, at admin_level 3). */
  country: boolean;
  /** The country it lies in: its ISO 3166-1 code ('' for a country). */
  in?: string;
}

/** A region as the server lists it (docs/inputs.md §6.1): a checked input in the inputs folder's
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
  /** `disabled = true` in its file: checked like any region (others may name it), never built or
   * downloaded. */
  disabled?: boolean;
  /** Its land and water (km²) and its country's ISO 3166-1 code, from its accepted geometry
   * (crates/server/src/regions.rs, `sized`): absent until this Mac's server has worked them out. */
  land_km2?: number;
  water_km2?: number;
  country?: string;
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
  // (Aborted while the body comes: aborted, not an empty answer.)
  const body = (await r.json().catch((e) => {
    if ((e as Error).name === 'AbortError') throw e;
    return null;
  })) as { error?: string } | null;
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

type Bbox = [number, number, number, number];
/** A region's outline as /api/regions/outlines sends it: every shape in one MultiPolygon,
 * simplified for the zoom asked. */
export type RegionOutline = GeoJSON.Feature<GeoJSON.MultiPolygon, {
  region: string;
  name: string;
  disabled: boolean;
  bbox: Bbox;
  /** A point inside it for its name (none: an empty outline). */
  label: [number, number] | null;
  /** Its first osm: part's area (its flag), when the pass has it. */
  area: Area | null;
}>;

/** The deepest zoom the server simplifies for (regions.rs OUTLINE_MAX_Z). */
export const OUTLINE_MAX_Z = 12;
/** The outlines asked for at a map zoom: floored, 0–12. */
export const outlineZoom = (zoom: number) => Math.max(0, Math.min(OUTLINE_MAX_Z, Math.floor(zoom)));
/** Every region's outline for zoom `z` (outlineZoom); with `bbox`, those meeting it, clipped to it
 * (`clip`: the box, whose edges the cuts follow), with `ids` those named; with `union`, those named
 * as one outline (their union: no line between regions that touch), a folder's hovered in the panel. */
export function getOutlines(z: number, opts: { bbox?: Bbox; ids?: string[]; union?: boolean; signal?: AbortSignal } = {}) {
  const q = new URLSearchParams({ z: String(z) });
  if (opts.bbox) q.set('bbox', opts.bbox.map((v) => v.toFixed(3)).join(','));
  if (opts.ids) q.set('ids', opts.ids.join(','));
  if (opts.union) q.set('union', '1');
  return call<{ features: RegionOutline[]; z: number; clip: Bbox | null }>(`/api/regions/outlines?${q}`, { signal: opts.signal });
}

/** An outline's rings as lines, leaving out the stretches along the edges of the box it was clipped
 * to (`clip`): those are cuts, not its border. */
export function outlineLines(f: Pick<RegionOutline, 'geometry' | 'properties'>, clip: Bbox | null): GeoJSON.Feature<GeoJSON.MultiLineString> {
  const lines: GeoJSON.Position[][] = [];
  const onEdge = (p: GeoJSON.Position, q: GeoJSON.Position) => !!clip &&
    ((p[0] === clip[0] && q[0] === clip[0]) || (p[0] === clip[2] && q[0] === clip[2]) || (p[1] === clip[1] && q[1] === clip[1]) || (p[1] === clip[3] && q[1] === clip[3]));
  for (const poly of f.geometry.coordinates) {
    for (const ring of poly) {
      let cur: GeoJSON.Position[] = [];
      for (let i = 1; i < ring.length; i++) {
        if (onEdge(ring[i - 1], ring[i])) {
          if (cur.length > 1) lines.push(cur);
          cur = [];
          continue;
        }
        if (!cur.length) cur.push(ring[i - 1]);
        cur.push(ring[i]);
      }
      if (cur.length > 1) lines.push(cur);
    }
  }
  return { type: 'Feature', geometry: { type: 'MultiLineString', coordinates: lines }, properties: { disabled: f.properties.disabled } };
}

/** Whether `p` ([lng, lat]) lies in a MultiPolygon (even-odd over every ring). */
export function inside(coords: GeoJSON.Position[][][], p: [number, number]): boolean {
  const [x, y] = p;
  let isIn = false;
  for (const poly of coords) {
    for (const ring of poly) {
      for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
        const [xi, yi] = ring[i], [xj, yj] = ring[j];
        if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) isIn = !isIn;
      }
    }
  }
  return isIn;
}

/** The region at `p` among `fs`: the smallest (by its box) holding it, so a province wins over the
 * waters around its country. */
export function regionAt(fs: RegionOutline[], p: [number, number]): RegionOutline | null {
  let best: RegionOutline | null = null, bestA = Infinity;
  for (const f of fs) {
    const [w, s, e, n] = f.properties.bbox;
    if (p[0] < w || p[0] > e || p[1] < s || p[1] > n) continue;
    const a = (e - w) * (n - s);
    if (a < bestA && inside(f.geometry.coordinates, p)) {
      best = f;
      bestA = a;
    }
  }
  return best;
}

/** The outlines' colours: green, disabled regions grey; the names in the same. */
const GREEN = '#5fcf8f';
const GREY = '#8a919c';
const COLOUR: ExpressionSpecification = ['case', ['to-boolean', ['get', 'disabled']], GREY, GREEN];
/** A region row hovered in the panel: its outline in a pale white no other line uses. */
const PREVIEW = '#dce7f5';
/** Past this zoom the outlines asked are those meeting the view (and around it), not the world's. */
const VIEW_FROM_Z = 3;

/**
 * "Show regions on map": every declared region's outline (green, disabled ones grey) and its name,
 * as a country's name (labellook.ts's 'country' type), simplified for the zoom by the server and
 * asked again as the zoom changes. The lines' width and opacity follow the boundaries' (basemap.ts
 * LINE_WIDTHS, applyBoundaryOpacity), whatever the boundaries' own switch. Draped on the terrain
 * under the roads; nothing here takes the pointer (the footer tells the region hovered: regionAt).
 * A row hovered in the panel is outlined on its own at the map's zoom (setHover): a region's, or a
 * folder's regions' as one.
 */
export class RegionLayers {
  private ready = false;
  private on = false;
  private features: RegionOutline[] = [];
  /** What was asked last: its zoom and the box it covers (null: the world). */
  private asked: { z: number; box: Bbox | null } | null = null;
  private abort: AbortController | null = null;
  /** The regions a panel row hovered outlines (null: none), the ask for them, and the last answers
   * (by their ask: hovering a row again draws at once). */
  private hoverIds: string[] | null = null;
  private hoverAbort: AbortController | null = null;
  private hoverSeen = new Map<string, GeoJSON.Feature<GeoJSON.MultiLineString>[]>();
  /** Set by the app: the layers just added (their widths, opacity and label look to apply). */
  onAdded: () => void = () => {};
  /** Set by the app: whether the region names are shown (their label type's look). */
  labelsShown: () => boolean = () => true;
  /** Set by the app: new outlines drawn (the footer's region may change). */
  onData: () => void = () => {};

  constructor(private map: MLMap) {
    map.on('moveend', () => {
      if (this.on) this.load();
      if (this.hoverIds) this.loadHover();
    });
  }

  /** The outlines drawn now (for the footer's hover). */
  get shown(): RegionOutline[] {
    return this.on ? this.features : [];
  }

  /** The source and layers, once the style is there: above the draped layers (terrain, water,
   * areas), below the roads' and contours' own layers. */
  private ensure(): boolean {
    const map = this.map;
    if (this.ready) return true;
    if (!(map as unknown as { style?: { _loaded?: boolean } }).style?._loaded) return false;
    // (Under the first layer that isn't draped: one between draped layers would split the draping
    // in two.)
    const before = ['contours-3d', 'roads'].find((id) => map.getLayer(id));
    const empty = { type: 'FeatureCollection' as const, features: [] };
    map.addSource('regions', { type: 'geojson', data: empty });
    map.addSource('regions-labels', { type: 'geojson', data: empty });
    map.addSource('region-hover', { type: 'geojson', data: empty });
    map.addLayer({
      id: 'regions-line', type: 'line', source: 'regions', layout: { visibility: 'none', 'line-join': 'round' },
      paint: { 'line-color': COLOUR, 'line-width': ['interpolate', ['linear'], ['zoom'], 3, 0.8, 10, 1.6], 'line-opacity': 0.85 },
    }, before);
    map.addLayer({
      id: 'region-hover-line', type: 'line', source: 'region-hover', layout: { visibility: 'none', 'line-join': 'round' },
      paint: { 'line-color': PREVIEW, 'line-width': ['interpolate', ['linear'], ['zoom'], 3, 1, 10, 1.8], 'line-dasharray': [2, 1.5], 'line-opacity': 0.9 },
    }, before);
    // The names on top: a symbol layer isn't draped, so it can't split the draping.
    map.addLayer({
      id: 'regions-label', type: 'symbol', source: 'regions-labels', layout: {
        visibility: 'none', 'text-field': ['get', 'name'], 'text-font': ['Noto Sans Medium'], 'text-transform': 'uppercase',
        'text-size': ['interpolate', ['linear'], ['zoom'], 2, 10, 6, 13, 10, 15], 'text-letter-spacing': 0.15, 'text-max-width': 9, 'text-padding': 6,
      },
      paint: { 'text-color': COLOUR, 'text-halo-color': '#0b0e13', 'text-halo-width': 1.4, 'text-halo-blur': 0.4 },
    });
    this.ready = true;
    this.onAdded();
    return true;
  }

  /** The layers shown or hidden: hidden rather than emptied (with 3D terrain an emptied source
   * leaves its last outlines in the draped textures, which a change of visibility redraws). */
  private apply() {
    if (!this.ready && !this.on) return;
    if (!this.ensure()) return;
    const vis = (id: string, v: boolean) => {
      const want = v ? 'visible' : 'none';
      if (this.map.getLayoutProperty(id, 'visibility') !== want) this.map.setLayoutProperty(id, 'visibility', want);
    };
    vis('regions-line', this.on);
    vis('regions-label', this.on && this.labelsShown());
  }

  /** Shown or not. */
  set(on: boolean) {
    this.on = on;
    if (!on) {
      this.abort?.abort();
      this.abort = null;
    }
    this.apply();
    if (on) this.load();
  }

  /** The row hovered in the panel outlined: a region (one id) or a folder's regions as one (null:
   * none). */
  setHover(ids: string[] | null) {
    this.hoverIds = ids?.length ? ids : null;
    this.hoverAbort?.abort();
    this.hoverAbort = null;
    if (!this.hoverIds) {
      if (this.ready) this.showHover(null);
      return;
    }
    this.loadHover();
  }

  /** The hovered regions' outline at the map's zoom, as the regions' own are asked (the view's box
   * and around it past VIEW_FROM_Z), drawn when it comes (at once when asked before). */
  private loadHover() {
    const ids = this.hoverIds;
    if (!ids || !this.ensure()) return;
    const z = outlineZoom(this.map.getZoom());
    let box: Bbox | undefined;
    if (z >= VIEW_FROM_Z) {
      const b = this.map.getBounds();
      const [w, s, e, n] = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
      const dx = (e - w) / 2, dy = (n - s) / 2;
      box = [Math.max(-180, w - dx), Math.max(-90, s - dy), Math.min(180, e + dx), Math.min(90, n + dy)];
    }
    const key = `${z} ${box?.map((v) => v.toFixed(3)).join(',') ?? ''} ${ids.join(',')}`;
    const seen = this.hoverSeen.get(key);
    if (seen) return this.showHover(seen);
    // (One ask at a time for a hover: a move meanwhile asks again once it's answered, the outline
    // asked first drawn meanwhile; only another row's hover gives it up, setHover.)
    if (this.hoverAbort) return;
    const ac = (this.hoverAbort = new AbortController());
    getOutlines(z, { ids, bbox: box, union: ids.length > 1, signal: ac.signal }).then(({ features, clip }) => {
      const lines = features.map((f) => outlineLines(f, clip));
      // (A few answers kept: rows hovered one after another.)
      if (this.hoverSeen.size >= 24) this.hoverSeen.delete(this.hoverSeen.keys().next().value!);
      this.hoverSeen.set(key, lines);
      if (ac.signal.aborted || this.hoverIds !== ids) return;
      this.hoverAbort = null;
      this.showHover(lines);
      this.loadHover();
    }, (e) => {
      if ((e as Error).name === 'AbortError') return;
      console.warn('regions: hover outline', e);
      if (this.hoverAbort === ac) this.hoverAbort = null;
    });
  }

  private showHover(lines: GeoJSON.Feature<GeoJSON.MultiLineString>[] | null) {
    if (lines?.length) this.map.getSource<GeoJSONSource>('region-hover')?.setData({ type: 'FeatureCollection', features: lines });
    const want = lines?.length ? 'visible' : 'none';
    if (this.map.getLayoutProperty('region-hover-line', 'visibility') !== want) this.map.setLayoutProperty('region-hover-line', 'visibility', want);
  }

  /** The names' look changed (shown or not). */
  restyle() {
    this.apply();
  }

  /** The outlines for the view's zoom, unless those drawn are for it already. */
  private load() {
    if (!this.ensure()) {
      this.map.once('styledata', () => this.on && this.load());
      return;
    }
    const z = outlineZoom(this.map.getZoom());
    let box: Bbox | null = null;
    if (z >= VIEW_FROM_Z) {
      const b = this.map.getBounds();
      const [w, s, e, n] = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
      const inAsked = this.asked?.z === z && this.asked.box && w >= this.asked.box[0] && s >= this.asked.box[1] && e <= this.asked.box[2] && n <= this.asked.box[3];
      if (inAsked) return;
      // (A view and as much again around it: a pan needn't ask again.)
      const dx = (e - w) / 2, dy = (n - s) / 2;
      box = [Math.max(-180, w - dx), Math.max(-90, s - dy), Math.min(180, e + dx), Math.min(90, n + dy)];
    } else if (this.asked?.z === z && !this.asked.box) return;
    this.abort?.abort();
    const ac = (this.abort = new AbortController());
    this.asked = { z, box };
    getOutlines(z, { bbox: box ?? undefined, signal: ac.signal }).then(({ features: fs, clip }) => {
      if (ac.signal.aborted) return;
      this.features = fs;
      this.map.getSource<GeoJSONSource>('regions')?.setData({ type: 'FeatureCollection', features: fs.map((f) => outlineLines(f, clip)) });
      const labels = fs.flatMap((f) => f.properties.label ? [{
        type: 'Feature' as const, geometry: { type: 'Point' as const, coordinates: f.properties.label },
        properties: { name: f.properties.name, disabled: f.properties.disabled },
      }] : []);
      this.map.getSource<GeoJSONSource>('regions-labels')?.setData({ type: 'FeatureCollection', features: labels });
      this.onData();
    }, (e) => {
      if ((e as Error).name !== 'AbortError') console.warn('regions: outlines', e);
      if (this.asked?.z === z && this.asked.box === box) this.asked = null;
    });
  }
}
