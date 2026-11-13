// Regions (docs/surface.md, docs/regions.md), read only: the server's list of the region files
// (/api/regions) and, while "Show regions on map" is on, their borders on the map from the region
// layer's vector tiles (/tiles/regions: docs/steps.md §15) and their names. The panel is
// ui/regions.ts.
import type { ExpressionSpecification, GeoJSONSource, Map as MLMap } from 'maplibre-gl';
import { onVersions, packChanged, version } from './api';
import { hostFor } from './hosts';
import { GREEN, GREY, LAYER, MAXZ, SOURCE_LAYER, borderOf, colourOf, labelPoints, pick } from './regionlayer';

/** An administrative or ISO 3166 area's fields a region's flag comes from (its first osm: part's). */
export interface Area {
  iso: string;
  /** An ISO 3166-1 country (Hong Kong is one, at admin_level 3). */
  country: boolean;
  /** The country it lies in: its ISO 3166-1 code ('' for a country). */
  in?: string;
}

type Bbox = [number, number, number, number];

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
  /** Its land and water (km²), its country's ISO 3166-1 code, its box (w, s, e, n) and the point its
   * name is drawn at, from its accepted geometry (crates/server/src/regions.rs, `sized`): absent until
   * this Mac's server has worked them out. */
  land_km2?: number;
  water_km2?: number;
  country?: string;
  bbox?: Bbox;
  label?: [number, number];
  /** Its first osm: part's area in the pass's outlines (its flag), when the pass has it. */
  area?: Area;
}

const NAS_AWAY = 'The NAS isn’t reachable: try again at home';

/** A request the server refused, with its reason in plain words. */
export class RegionsError extends Error {
  constructor(readonly status: number, message: string) {
    super(message);
  }
}

/** A JSON request to this Mac's server, for the region and download panels: its answer, else a
 * RegionsError in words (the server not answering; 503, the NAS away; its own `error`, capitalized).
 * An abort, before or during the answer, is thrown as the AbortError it is. */
export async function call<T>(url: string, init?: RequestInit): Promise<T> {
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

/** The regions at a point, as the server reads them from the layer's tiles (none where no region
 * is, or before the map has the layer). */
export const regionsAt = (lng: number, lat: number, signal?: AbortSignal) =>
  call<{ regions: string[] }>(`/api/regions/at?lng=${lng.toFixed(6)}&lat=${lat.toFixed(6)}`, { signal }).then((r) => r.regions);

/** The names' colour: a disabled region's grey. */
const NAME_COLOUR: ExpressionSpecification = ['case', ['to-boolean', ['get', 'disabled']], GREY, GREEN];

/** A region row hovered in the panel: its border in a pale white no other line uses. */
const PREVIEW = '#dce7f5';

/**
 * "Show regions on map": the regions' borders (green, those between disabled regions grey) from the
 * region layer's tiles, lines drawn from the borders themselves (never a ring's outline: no tile
 * edge, cut at ±180° or the tiles' edge at ±85.05° is drawn), and their names, as a country's name
 * (labellook.ts's 'country' type). The lines' width and opacity follow the boundaries' (basemap.ts
 * LINE_WIDTHS, applyBoundaryOpacity), whatever the boundaries' own switch. Draped on the terrain
 * under the roads; nothing here takes the pointer (the footer asks for the region hovered: `at`).
 * A row hovered in the panel draws its region's or folder's border on its own (setHover), a filter
 * of the same tiles. Until the map has the layer (a map from before it), only the names are drawn.
 */
export class RegionLayers {
  private ready = false;
  private on = false;
  private regions: Region[] = [];
  private byId = new Map<string, Region>();
  private hoverIds: string[] | null = null;
  /** The last point asked about and its answer, and the ask out (one at a time). */
  private lastAt: { key: string; id: string | null } | null = null;
  /** Set by the app: the layers just added (their widths, opacity and label look to apply). */
  onAdded: () => void = () => {};
  /** Set by the app: whether the region names are shown (their label type's look). */
  labelsShown: () => boolean = () => true;

  constructor(private map: MLMap) {
    // (The layer's packs moved, or the map got it: its tiles asked again, or its source added.)
    onVersions([LAYER], () => {
      if (!this.ready) return;
      if (map.getSource(LAYER)) map.refreshTilesWhere(LAYER, (z, x, y) => packChanged(LAYER, z, x, y));
      else this.addLines();
      this.apply();
    });
  }

  /** Whether the borders are on (the footer asks for the region hovered only then). */
  get shown(): boolean {
    return this.on;
  }

  /** The line layers, once the map has the layer: above the draped layers (terrain, water, areas),
   * below the roads' and contours' own layers. */
  private addLines() {
    const map = this.map;
    if (map.getSource(LAYER) || !version(LAYER)) return;
    // (Under the first layer that isn't draped: one between draped layers would split the draping
    // in two.)
    const before = ['contours-3d', 'roads'].find((id) => map.getLayer(id));
    map.addSource(LAYER, { type: 'vector', tiles: [`${hostFor('layers')}/tiles/${LAYER}/{z}/{x}/{y}`], minzoom: 0, maxzoom: MAXZ });
    map.addLayer({
      id: 'regions-line', type: 'line', source: LAYER, 'source-layer': SOURCE_LAYER, layout: { visibility: 'none', 'line-join': 'round' },
      paint: { 'line-color': colourOf(this.enabled()), 'line-width': ['interpolate', ['linear'], ['zoom'], 3, 0.8, 10, 1.6], 'line-opacity': 0.85 },
    }, before);
    map.addLayer({
      id: 'region-hover-line', type: 'line', source: LAYER, 'source-layer': SOURCE_LAYER, filter: borderOf(this.hoverIds ?? []),
      layout: { visibility: 'none', 'line-join': 'round' },
      paint: { 'line-color': PREVIEW, 'line-width': ['interpolate', ['linear'], ['zoom'], 3, 1, 10, 1.8], 'line-dasharray': [2, 1.5], 'line-opacity': 0.9 },
    }, before);
    this.onAdded();
  }

  /** The sources and layers, once the style is there. */
  private ensure(): boolean {
    const map = this.map;
    if (this.ready) return true;
    if (!(map as unknown as { style?: { _loaded?: boolean } }).style?._loaded) return false;
    map.addSource('regions-labels', { type: 'geojson', data: labelPoints(this.regions) });
    // The names on top: a symbol layer isn't draped, so it can't split the draping.
    map.addLayer({
      id: 'regions-label', type: 'symbol', source: 'regions-labels', layout: {
        visibility: 'none', 'text-field': ['get', 'name'], 'text-font': ['Noto Sans Medium'], 'text-transform': 'uppercase',
        'text-size': ['interpolate', ['linear'], ['zoom'], 2, 10, 6, 13, 10, 15], 'text-letter-spacing': 0.15, 'text-max-width': 9, 'text-padding': 6,
      },
      paint: { 'text-color': NAME_COLOUR, 'text-halo-color': '#0b0e13', 'text-halo-width': 1.4, 'text-halo-blur': 0.4 },
    });
    this.ready = true;
    this.addLines();
    this.onAdded();
    return true;
  }

  private enabled(): string[] {
    return this.regions.filter((r) => !r.disabled).map((r) => r.id);
  }

  /** The layers shown or hidden: hidden rather than emptied (with 3D terrain an emptied source
   * leaves its last lines in the draped textures, which a change of visibility redraws). */
  private apply() {
    if (!this.ready && !this.on) return;
    if (!this.ensure()) {
      this.map.once('styledata', () => this.apply());
      return;
    }
    const vis = (id: string, v: boolean) => {
      if (!this.map.getLayer(id)) return;
      const want = v ? 'visible' : 'none';
      if (this.map.getLayoutProperty(id, 'visibility') !== want) this.map.setLayoutProperty(id, 'visibility', want);
    };
    vis('regions-line', this.on);
    vis('regions-label', this.on && this.labelsShown());
    vis('region-hover-line', !!this.hoverIds);
  }

  /** Shown or not. Shown before the panel has listed the regions: the list read here (their names
   * and which are disabled). */
  set(on: boolean) {
    this.on = on;
    this.apply();
    if (on && !this.regions.length && !this.listing) {
      this.listing = true;
      listRegions().then((l) => {
        if (!this.regions.length) this.setRegions(l.regions);
      }, (e) => console.warn('regions: list', e)).finally(() => (this.listing = false));
    }
  }
  private listing = false;

  /** The regions as listed (their names' points, which are disabled): the names and colours again. */
  setRegions(rs: Region[]) {
    this.regions = rs;
    this.byId = new Map(rs.map((r) => [r.id, r]));
    this.lastAt = null;
    if (!this.ready) return;
    this.map.getSource<GeoJSONSource>('regions-labels')?.setData(labelPoints(rs));
    if (this.map.getLayer('regions-line')) this.map.setPaintProperty('regions-line', 'line-color', colourOf(this.enabled()));
  }

  /** The row hovered in the panel outlined: a region (one id) or a folder's regions as one (null:
   * none). */
  setHover(ids: string[] | null) {
    this.hoverIds = ids?.length ? ids : null;
    if (!this.ready) {
      if (this.hoverIds) this.apply();
      return;
    }
    if (this.hoverIds && this.map.getLayer('region-hover-line')) this.map.setFilter('region-hover-line', borderOf(this.hoverIds));
    this.apply();
  }

  /** The names' look changed (shown or not). */
  restyle() {
    this.apply();
  }

  /** The region the footer tells at a point (`pick` of the regions there), null for none; the same
   * point asked again answered at once. */
  async at(ll: { lng: number; lat: number }, signal?: AbortSignal): Promise<string | null> {
    const key = `${ll.lng.toFixed(5)},${ll.lat.toFixed(5)}`;
    if (this.lastAt?.key === key) return this.lastAt.id;
    const id = pick(await regionsAt(ll.lng, ll.lat, signal), this.byId);
    this.lastAt = { key, id };
    return id;
  }

  /** A region by its id, as listed. */
  region(id: string): Region | undefined {
    return this.byId.get(id);
  }
}
