// Cheaper tile covers. While the camera moves, MapLibre works out every frame, for each source whose
// layers show, which tiles cover the view: 30-odd sources here (the basemap and its regional parts,
// terrain, hill-shading, slope, trees, each kind of stop and sight, heritage, the GeoJSON overlays),
// a quarter of the main thread in tilted views. The same tiles for less work:
//  - Tile zoom. MapLibre's default (a source's calculateTileZoom) evaluates two numerical integrals,
//    40-odd cos and pow, for every tile it visits of every source, though they depend only on the
//    camera: here they're computed once per camera.
//  - Frustum tests. On the globe a tile's bounding volume is shared by the sources in a frame
//    (MapLibre caches it); its tests against the frame's frustum and horizon plane are kept with it.
//    The volumes are cached by the height they reach, which near the horizon grows with the pitch:
//    rounded up to ELEV_STEP, they last through a tilt instead of being made again every frame.
//  - Sources that can't be in view skip the cover: one whose bounds lie wholly beyond the horizon seen
//    from the camera (with room for mountains beyond it), such as the regional basemaps, and an empty
//    GeoJSON source (a selection or highlight not in use).
import type { Map as MLMap } from 'maplibre-gl';

const DEG = Math.PI / 180;
const R = 6371008.8;
/** Terrain seen beyond the horizon: up to this high (m, before exaggeration). */
const MAX_RELIEF = 9000;
/** Bounding volume heights rounded up to this (m). */
const ELEV_STEP = 64;
// MapLibre's defaults (geo/projection/covering_tiles.ts, mercator_utils.ts).
const MAX_ZOOM_LEVELS_ON_SCREEN = 9.314;
const TILE_COUNT_MAX_MIN_RATIO = 3.0;
const MAX_MERCATOR_HORIZON_ANGLE = 89.25;

const scaleZoom = (s: number) => Math.log(s) / Math.LN2;

function integralOfCosXByP(p: number, x1: number, x2: number): number {
  const n = 10;
  let sum = 0;
  const dx = (x2 - x1) / n;
  for (let i = 0; i < n; i++) sum += dx * Math.pow(Math.cos(x1 + ((i + 0.5) / n) * (x2 - x1)), p);
  return sum;
}

type TileZoomFn = (centerZoom: number, toTile2d: number, toTileZ: number, toCenter3d: number, fov: number) => number;

/** MapLibre's createCalculateTileZoomFunction(maxZoomLevelsOnScreen, tileCountMaxMinRatio), with the
 * terms that depend only on the camera computed once per camera (cos(atan(a / b)) = b / hypot(a, b)). */
function tileZoomFunction(maxLevels: number, ratio: number): TileZoomFn {
  let kZ = NaN, kC = NaN, kF = NaN;
  let behaviour = 0, cosHalf = 1, countAdj = 0;
  return (centerZoom, toTile2d, toTileZ, toCenter3d, fov) => {
    if (toTileZ !== kZ || toCenter3d !== kC || fov !== kF) {
      kZ = toTileZ;
      kC = toCenter3d;
      kF = fov;
      behaviour = 2 * ((maxLevels - 1) / scaleZoom(Math.cos((MAX_MERCATOR_HORIZON_ANGLE - fov) * DEG) / Math.cos(MAX_MERCATOR_HORIZON_ANGLE * DEG)) - 1);
      const centerPitch = Math.acos(toTileZ / toCenter3d);
      const count0 = 2 * integralOfCosXByP(behaviour - 1, 0, (fov / 2) * DEG);
      const highest = Math.min(MAX_MERCATOR_HORIZON_ANGLE * DEG, centerPitch + (fov / 2) * DEG);
      const lowest = Math.min(highest, centerPitch - (fov / 2) * DEG);
      const count = integralOfCosXByP(behaviour - 1, lowest, highest);
      countAdj = scaleZoom(Math.max(1, count / count0 / ratio)) / 2;
      cosHalf = Math.max(0.5, Math.cos((fov / 2) * DEG));
    }
    const toTile3d = Math.hypot(toTile2d, toTileZ);
    return centerZoom + scaleZoom(toCenter3d / toTile3d / cosHalf) + (behaviour * scaleZoom(toTileZ / toTile3d)) / 2 - countAdj;
  };
}

const defaultTileZoom = tileZoomFunction(MAX_ZOOM_LEVELS_ON_SCREEN, TILE_COUNT_MAX_MIN_RATIO);

interface Bounds {
  getWest(): number;
  getEast(): number;
  getSouth(): number;
  getNorth(): number;
}
interface Source {
  type: string;
  calculateTileZoom?: TileZoomFn;
  tileBounds?: { bounds: Bounds };
  _data?: { geojson?: { type?: string; features?: unknown[] }; updateable?: Map<unknown, unknown>; url?: string };
}
interface Transform {
  zoom: number;
  pitch: number;
  bearing: number;
  elevation: number;
  center: { lng: number; lat: number };
  getCameraLngLat(): { lng: number; lat: number };
  getCameraAltitude(): number;
}
interface TileManager {
  used: boolean;
  usedForTerrain: boolean;
  _source?: Source;
  map?: { terrain?: { exaggeration?: number } | null };
  update(tr: Transform, terrain?: unknown): void;
}

/** The part of the globe the camera can see: a cap (degrees) around the point below it. */
let capFor: Transform | null = null;
let capKey = [NaN, NaN, NaN, NaN, NaN, NaN, NaN];
const cap = { lng: 0, lat: 0, r: 180 };
function viewCap(tr: Transform, exaggeration: number) {
  const k = [tr.zoom, tr.pitch, tr.bearing, tr.elevation, tr.center.lng, tr.center.lat, exaggeration];
  if (capFor === tr && k.every((v, i) => v === capKey[i])) return cap;
  capFor = tr;
  capKey = k;
  const ll = tr.getCameraLngLat();
  const alt = Math.max(0, tr.getCameraAltitude());
  const r = (Math.acos(R / (R + alt)) + Math.acos(R / (R + MAX_RELIEF * Math.max(1, exaggeration)))) / DEG;
  cap.lng = ll.lng;
  cap.lat = ll.lat;
  cap.r = Number.isFinite(r) ? r : 180;
  return cap;
}

/** Whether a cap meets a longitude/latitude box. */
function capMeets(c: typeof cap, b: Bounds): boolean {
  if (c.r >= 90) return true;
  const s = c.lat - c.r, n = c.lat + c.r;
  if (n < b.getSouth() || s > b.getNorth()) return false;
  if (n >= 90 || s <= -90) return true;
  const sinDl = Math.sin(c.r * DEG) / Math.cos(c.lat * DEG);
  if (sinDl >= 1) return true;
  const dl = Math.asin(sinDl) / DEG;
  const w = b.getWest(), e = b.getEast();
  if (e - w >= 360) return true;
  for (const k of [-360, 0, 360]) if (c.lng - dl + k <= e && c.lng + dl + k >= w) return true;
  return false;
}

function emptyGeoJSON(src: Source): boolean {
  const d = src._data;
  if (!d || d.url) return false;
  if (d.updateable) return d.updateable.size === 0;
  return d.geojson?.type === 'FeatureCollection' && (d.geojson.features?.length ?? 1) === 0;
}

const own = (o: object, k: string) => Object.prototype.hasOwnProperty.call(o, k);

function patchTileManager(proto: TileManager & { __covers?: boolean }) {
  if (own(proto, '__covers') || typeof proto.update !== 'function') return;
  proto.__covers = true;
  const update = proto.update;
  proto.update = function (this: TileManager, tr: Transform, terrain?: unknown) {
    const src = this._source;
    if (src && !src.calculateTileZoom) src.calculateTileZoom = defaultTileZoom;
    if (src && this.used && !this.usedForTerrain) {
      const b = src.tileBounds?.bounds;
      const out = src.type === 'geojson' ? emptyGeoJSON(src) : !!b && !capMeets(viewCap(tr, this.map?.terrain?.exaggeration ?? 1), b);
      if (out) {
        // As if its layers were hidden for this update: no cover, and no tiles kept.
        this.used = false;
        try {
          update.call(this, tr, terrain);
        } finally {
          this.used = true;
        }
        return;
      }
    }
    update.call(this, tr, terrain);
  };
}

type Volume = {
  intersectsFrustum(f: unknown): number;
  intersectsPlane(p: unknown): number;
  __f?: unknown;
  __fr?: number;
  __p?: unknown;
  __pr?: number;
};

function patchVolume(proto: Volume & { __covers?: boolean }) {
  if (own(proto, '__covers') || typeof proto.intersectsFrustum !== 'function') return;
  proto.__covers = true;
  const frustum = proto.intersectsFrustum, plane = proto.intersectsPlane;
  // (The frustum and the plane are new objects whenever the camera changes.)
  proto.intersectsFrustum = function (this: Volume, f: unknown) {
    if (this.__f !== f) {
      this.__fr = frustum.call(this, f);
      this.__f = f;
    }
    return this.__fr!;
  };
  proto.intersectsPlane = function (this: Volume, p: unknown) {
    if (this.__p !== p) {
      this.__pr = plane.call(this, p);
      this.__p = p;
    }
    return this.__pr!;
  };
}

type Provider = { getTileBoundingVolume(t: object, wrap: number, elevation: number, o: object): Volume; __covers?: boolean };

function patchProvider(proto: Provider) {
  if (own(proto, '__covers') || typeof proto.getTileBoundingVolume !== 'function') return;
  proto.__covers = true;
  const get = proto.getTileBoundingVolume;
  proto.getTileBoundingVolume = function (this: Provider, t: object, wrap: number, elevation: number, o: object) {
    return get.call(this, t, wrap, Math.ceil(elevation / ELEV_STEP) * ELEV_STEP, o);
  };
}

/** Installs the above on the map's classes as they appear (the globe's transform comes with the style). */
export function cheaperCovers(map: MLMap) {
  let volumes = false;
  const ensure = () => {
    const style = (map as unknown as { style?: { tileManagers?: Record<string, TileManager> } }).style;
    const tms = style?.tileManagers;
    if (tms) {
      for (const id in tms) {
        patchTileManager(Object.getPrototypeOf(tms[id]) as TileManager);
        break;
      }
    }
    if (!volumes) {
      const tr = (map as unknown as { _camera?: { transform?: { _verticalPerspectiveTransform?: { getCoveringTilesDetailsProvider?: () => Provider } } } })._camera?.transform;
      const provider = tr?._verticalPerspectiveTransform?.getCoveringTilesDetailsProvider?.();
      if (provider) {
        volumes = true;
        patchVolume(Object.getPrototypeOf(provider.getTileBoundingVolume({ x: 0, y: 0, z: 0 }, 0, 0, {})) as Volume);
        patchProvider(Object.getPrototypeOf(provider) as Provider);
      }
    }
  };
  for (const ev of ['styledata', 'sourcedata', 'render'] as const) map.on(ev, ensure);
  ensure();
}
