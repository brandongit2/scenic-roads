// Cursor-anchored 3D camera moves, on the globe (earth-centred coordinates) and on the flat map
// (mercator) that MapLibre switches to when zoomed in. MapLibre anchors zoom on a surface at the camera pivot's
// height, but with 3D terrain (exaggerated, and a pivot that doesn't follow the ground) the
// point under the cursor can be kilometres above or below that plane, so the anchor slides.
// Here the anchor is a point on the cursor's view ray (the terrain it hits, when that is
// reliable), and the camera moves rigidly:
//   · dolly: along the line from the camera to the anchor (zoom),
//   · orbit: rotation about the anchor (bearing about the vertical, pitch about the camera's
//     horizontal right axis).
// A point on the cursor ray stays under the cursor through a dolly, and a rigid rotation about
// a point keeps it at the same pixel. The new camera position and orientation are turned back
// into centre/zoom by MapLibre, keeping the pivot height.
import { MercatorCoordinate, type LngLat, type Map as MLMap } from 'maplibre-gl';

export interface Anchor {
  ll: LngLat;
  /** Rendered height of the point (terrain × exaggeration), metres. */
  elev: number;
  /** True when the anchor is ground the cursor is actually on (not sky / a capped distance). */
  ground: boolean;
}

type Tr = { getCameraLngLat: () => LngLat; getCameraAltitude: () => number; cameraToCenterDistance: number; fov: number; isGlobeRendering?: boolean };
const transform = (map: MLMap) => (map as unknown as { _camera?: { transform?: Tr } })._camera?.transform;

const DEG = Math.PI / 180;

/** True while MapLibre renders the globe (its camera then lives on a sphere). */
export const isGlobe = (map: MLMap) => transform(map)?.isGlobeRendering === true;

/**
 * Globe moves. MapLibre defines pitch and bearing in the local frame at the view centre, and on
 * a sphere that frame turns as the camera moves, so a rigid 3D move would change pitch and add
 * roll. Instead: let MapLibre do the move (globe-correct, canonical orientation) anchored on the
 * sea-level point under the cursor, then pan by the few remaining pixels until the terrain point
 * under the cursor is back exactly under it. Refuses (and undoes) moves that bring the camera
 * too close to the terrain or hit a limit.
 */
function globeMove(map: MLMap, tr: Tr, a: Anchor, px: number, py: number, move: () => void): boolean {
  const before = { center: map.getCenter(), zoom: map.getZoom(), pitch: map.getPitch(), bearing: map.getBearing(), elevation: map.getCenterElevation() };
  move();
  // Newton: the anchor (above sea level, in perspective) moves by J·pan on screen when the map is
  // panned; J is measured once by probing, then solved for the pan that puts it on the cursor.
  let J: [number, number, number, number] | null = null;
  for (let i = 0; i < 5; i++) {
    const q = map.project(a.ll);
    const ex = q.x - px, ey = q.y - py;
    if (!Number.isFinite(ex) || Math.hypot(ex, ey) < 0.2) break;
    if (!J) {
      const c0 = map.getCenter();
      const h = 6;
      map.panBy([h, 0], { animate: false });
      const qx = map.project(a.ll);
      map.jumpTo({ center: c0 });
      map.panBy([0, h], { animate: false });
      const qy = map.project(a.ll);
      map.jumpTo({ center: c0 });
      J = [(qx.x - q.x) / h, (qy.x - q.x) / h, (qx.y - q.y) / h, (qy.y - q.y) / h];
    }
    const det = J[0] * J[3] - J[1] * J[2];
    if (!Number.isFinite(det) || Math.abs(det) < 1e-6) break;
    // Solve J·pan = −e.
    const panX = (-ex * J[3] + ey * J[1]) / det;
    const panY = (-ey * J[0] + ex * J[2]) / det;
    map.panBy([panX, panY], { animate: false });
  }
  const q = map.project(a.ll);
  const ground = map.queryTerrainElevation(tr.getCameraLngLat()) ?? 0;
  if (Math.hypot(q.x - px, q.y - py) > 3 || tr.getCameraAltitude() < ground + 30) {
    map.jumpTo(before);
    return false;
  }
  return true;
}

/** The sea-level point on the cursor ray (MapLibre's globe anchors zoom there). */
function seaLevelAt(map: MLMap, tr: Tr, px: number, py: number): LngLat | undefined {
  const t = tr as unknown as { screenPointToLocation?: (p: { x: number; y: number }) => LngLat | undefined };
  const ll = t.screenPointToLocation?.({ x: px, y: py });
  return ll && Number.isFinite(ll.lng) ? ll : undefined;
}

function dollyGlobe(map: MLMap, tr: Tr, a: Anchor, dz: number, px: number, py: number): boolean {
  const z = map.getZoom() + dz;
  if (z > map.getMaxZoom() || z < map.getMinZoom()) return false;
  const around = seaLevelAt(map, tr, px, py);
  return globeMove(map, tr, a, px, py, () => map.zoomTo(z, around ? { around, duration: 0 } : { duration: 0 }));
}

function orbitGlobe(map: MLMap, tr: Tr, a: Anchor, dBearing: number, dPitch: number, px: number, py: number): boolean {
  const pitch = Math.max(0, Math.min(map.getMaxPitch(), map.getPitch() + dPitch));
  return globeMove(map, tr, a, px, py, () => map.jumpTo({ bearing: map.getBearing() + dBearing, pitch }));
}

/**
 * The anchor for a screen position: a point on the cursor's view ray. It is the terrain the ray
 * hits when that hit reprojects onto the cursor (terrain loaded), else the pivot plane, and it
 * is never farther than a few camera→centre distances (no flights to the horizon, and the sky
 * gets a point straight ahead instead of one behind the camera).
 */
export function anchorAt(map: MLMap, px: number, py: number): Anchor | null {
  const tr = transform(map);
  if (!tr) return null;
  const c = map.getCanvas();
  const W = c.clientWidth, H = c.clientHeight;
  if (!W || !H) return null;
  if (tr.isGlobeRendering) {
    // Globe: the ground under the cursor if it reprojects onto the cursor; off the planet there
    // is no anchor (callers then zoom / rotate about the view centre).
    const ll = map.unproject([px, py]);
    if (!ll || !Number.isFinite(ll.lng)) return null;
    const q = map.project(ll);
    if (Math.hypot(q.x - px, q.y - py) > 3) return null;
    return { ll, elev: map.queryTerrainElevation(ll) ?? 0, ground: true };
  }
  const deg = Math.PI / 180;
  const b = map.getBearing() * deg, p = map.getPitch() * deg;
  // Camera frame in east/north/up.
  const f = [Math.sin(p) * Math.sin(b), Math.sin(p) * Math.cos(b), -Math.cos(p)];
  const r = [Math.cos(b), -Math.sin(b), 0];
  const u = [Math.cos(p) * Math.sin(b), Math.cos(p) * Math.cos(b), Math.sin(p)];
  const s = Math.tan((tr.fov * deg) / 2) / (H / 2);
  const d = [0, 1, 2].map((k) => f[k] + r[k] * (px - W / 2) * s + u[k] * (H / 2 - py) * s);
  const dl = Math.hypot(d[0], d[1], d[2]);
  const dir = d.map((v) => v / dl);
  const camLL = tr.getCameraLngLat(), camAlt = tr.getCameraAltitude();
  const C = MercatorCoordinate.fromLngLat(camLL, camAlt);
  const mu = C.meterInMercatorCoordinateUnits();
  // Camera → pivot distance in metres (MapLibre's cameraToCenterDistance is in pixels).
  const pivotDist = Math.max(50, (camAlt - map.getCenterElevation()) / Math.max(0.05, Math.cos(p)));
  const tMax = pivotDist * 2;
  const along = (t: number, v = dir): Anchor => {
    const m = new MercatorCoordinate(C.x + v[0] * t * mu, C.y - v[1] * t * mu, 0);
    return { ll: m.toLngLat(), elev: camAlt + v[2] * t, ground: false };
  };
  // 1. Terrain hit, if it is really on this ray and not too far.
  const ll = map.unproject([px, py]);
  if (ll && Number.isFinite(ll.lng)) {
    const e = map.queryTerrainElevation(ll) ?? 0;
    const q = map.project(ll);
    if (Math.hypot(q.x - px, q.y - py) <= 3) {
      const P = MercatorCoordinate.fromLngLat(ll, e);
      const v = [(P.x - C.x) / mu, -(P.y - C.y) / mu, e - camAlt];
      const t = v[0] * dir[0] + v[1] * dir[1] + v[2] * dir[2];
      if (t > 0 && t <= tMax) return { ll, elev: e, ground: true };
      if (t > tMax) return along(tMax);
    }
  }
  // 2. The pivot plane, when the ray reaches it ahead of the camera.
  if (dir[2] < -1e-6) {
    const t = (map.getCenterElevation() - camAlt) / dir[2];
    if (t > 0) return along(Math.min(t, tMax));
  }
  // 3. Sky: level flight toward the cursor's direction (never climbing into the sky).
  const lv = [dir[0], dir[1], 0];
  const ll2 = Math.hypot(lv[0], lv[1]) || 1;
  return along(pivotDist, [lv[0] / ll2, lv[1] / ll2, 0]);
}

/**
 * The pivot (MapLibre's centre elevation) sits at sea level while zoomed out: MapLibre's globe
 * camera ignores it, so a pivot anywhere else would make the view jump where the globe hands
 * over to the flat map (zoom 11–12). From this zoom up (flat map only) it is lifted to the
 * ground at the view centre, so zoom-based detail follows the real distance to the ground.
 */
export const GROUND_PIVOT_ZOOM = 12.5;

/** Mercator zoom for a camera at `alt` looking at pitch p (radians) with the pivot at `E`. */
function zoomFor(map: MLMap, tr: Tr, alt: number, E: number, p: number): number {
  const d = (alt - E) / Math.cos(p);
  const mu = MercatorCoordinate.fromLngLat(map.getCenter()).meterInMercatorCoordinateUnits();
  return Math.log2(map.getCanvas().clientHeight / 2 / Math.tan((tr.fov * DEG) / 2) / (d * mu) / 512);
}

/**
 * Re-pivot without moving the camera: to the ground at the view centre when zoomed in on the flat
 * map, else to sea level. Nothing moves on screen; the zoom number (tile detail, widths, labels)
 * follows. Call when a gesture has settled.
 */
export function relevel(map: MLMap): void {
  const tr = transform(map);
  if (!tr) return;
  if (tr.isGlobeRendering) {
    if (map.getCenterElevation() !== 0) map.jumpTo({ elevation: 0 }); // no camera change on the globe
    return;
  }
  const alt = tr.getCameraAltitude();
  const p = map.getPitch() * DEG;
  let target = 0;
  if (zoomFor(map, tr, alt, 0, p) >= GROUND_PIVOT_ZOOM) {
    const c = map.getCanvas();
    const cx = c.clientWidth / 2, cy = c.clientHeight / 2;
    const ll = map.unproject([cx, cy]);
    const ground = ll && map.queryTerrainElevation(ll);
    if (ground === null || ground === undefined) return;
    const q = map.project(ll);
    if (Math.hypot(q.x - cx, q.y - cy) > 2) return; // terrain there not loaded yet
    target = ground;
  }
  if (Math.abs(target - map.getCenterElevation()) < 2) return;
  applyCamera(map, MercatorCoordinate.fromLngLat(tr.getCameraLngLat(), alt), map.getBearing(), map.getPitch(), target);
}

/** Move the pivot to elevation E without moving the camera. */
export function repivot(map: MLMap, E: number): void {
  const tr = transform(map);
  if (!tr) return;
  if (tr.isGlobeRendering) {
    map.jumpTo({ elevation: E });
    return;
  }
  applyCamera(map, MercatorCoordinate.fromLngLat(tr.getCameraLngLat(), tr.getCameraAltitude()), map.getBearing(), map.getPitch(), E);
}

/**
 * Put the camera at `m` (mercator position + altitude) with the given orientation. The pivot
 * (MapLibre's centre elevation) is set to `pivot`, kept safely below the camera so the view axis
 * always meets the pivot plane ahead (otherwise MapLibre substitutes an arbitrary distance and
 * the view jumps). Refuses positions too close to the terrain (MapLibre would otherwise "rescue"
 * the camera by changing pitch and zoom) or outside the zoom limits.
 */
function applyCamera(map: MLMap, m: MercatorCoordinate, bearing: number, pitch: number, pivot: number): boolean {
  const tr = transform(map);
  if (!tr) return false;
  const deg = Math.PI / 180;
  const alt = m.toAltitude();
  const camLL = m.toLngLat();
  // Clearance above the ground under the camera.
  const ground = map.queryTerrainElevation(camLL) ?? 0;
  if (alt < ground + 30) return false;
  const p = Math.min(pitch, 85) * deg, b = bearing * deg;
  let E = Math.min(pivot, alt - Math.max(30, (alt - ground) * 0.25));
  // Below GROUND_PIVOT_ZOOM the pivot stays at sea level, like the globe's (see there).
  if (E !== 0 && zoomFor(map, tr, alt, 0, p) < GROUND_PIVOT_ZOOM) E = 0;
  const d = (alt - E) / Math.cos(p); // metres from the camera to the pivot along the view axis
  const hz = d * Math.sin(p);
  // Horizontal offset in mercator units. MapLibre converts camera↔centre distances with the
  // metre scale at the centre, so iterate on the centre (as it does) to land exactly on `m`.
  let mu = m.meterInMercatorCoordinateUnits();
  let cx = m.x, cy = m.y;
  for (let i = 0; i < 6; i++) {
    cx = m.x + Math.sin(b) * hz * mu;
    cy = m.y - Math.cos(b) * hz * mu;
    mu = new MercatorCoordinate(cx, cy, 0).meterInMercatorCoordinateUnits();
  }
  const centre = new MercatorCoordinate(cx, cy, 0);
  const dM = d * mu;
  const H = map.getCanvas().clientHeight;
  const zoom = Math.log2(H / 2 / Math.tan((tr.fov * deg) / 2) / dM / 512);
  if (!Number.isFinite(zoom) || zoom > map.getMaxZoom() + 1e-6 || zoom < map.getMinZoom() - 1e-6) return false;
  map.jumpTo({ center: centre.toLngLat(), zoom, pitch, bearing, elevation: E });
  return true;
}

/** Zoom by dz levels, moving the camera toward (dz > 0) or away from the anchor. */
export function dolly(map: MLMap, a: Anchor, dz: number, px: number, py: number): boolean {
  const tr = transform(map);
  if (!tr) return false;
  if (tr.isGlobeRendering) return dollyGlobe(map, tr, a, dz, px, py);
  const C = MercatorCoordinate.fromLngLat(tr.getCameraLngLat(), tr.getCameraAltitude());
  const P = MercatorCoordinate.fromLngLat(a.ll, a.elev);
  const f = 2 ** -dz;
  const N = new MercatorCoordinate(P.x + (C.x - P.x) * f, P.y + (C.y - P.y) * f, P.z + (C.z - P.z) * f);
  return applyCamera(map, N, map.getBearing(), map.getPitch(), a.elev);
}

/** Rotate the camera about the anchor: dBearing about the vertical, dPitch about the camera's right axis. */
export function orbit(map: MLMap, a: Anchor, dBearing: number, dPitch: number, px: number, py: number): boolean {
  const tr = transform(map);
  if (!tr) return false;
  if (tr.isGlobeRendering) return orbitGlobe(map, tr, a, dBearing, dPitch, px, py);
  const deg = Math.PI / 180;
  const b0 = map.getBearing(), p0 = map.getPitch();
  const p1 = Math.max(0, Math.min(map.getMaxPitch(), p0 + dPitch));
  const b1 = b0 + dBearing;
  const C = MercatorCoordinate.fromLngLat(tr.getCameraLngLat(), tr.getCameraAltitude());
  const P = MercatorCoordinate.fromLngLat(a.ll, a.elev);
  const mu = P.meterInMercatorCoordinateUnits();
  // Camera relative to the anchor, local east/north/up metres.
  let e = (C.x - P.x) / mu, n = -(C.y - P.y) / mu;
  const u = tr.getCameraAltitude() - a.elev;
  // Bearing: clockwise about the vertical axis.
  const tb = dBearing * deg;
  [e, n] = [e * Math.cos(tb) + n * Math.sin(tb), -e * Math.sin(tb) + n * Math.cos(tb)];
  // Pitch: Rodrigues rotation about the (new) right axis r = (cos b, −sin b, 0).
  const th = (p1 - p0) * deg;
  const rx = Math.cos(b1 * deg), ry = -Math.sin(b1 * deg);
  const dot = rx * e + ry * n;
  const cx = ry * u, cy = -rx * u, cz = rx * n - ry * e; // r × v
  const c = Math.cos(th), s = Math.sin(th);
  const e2 = e * c + cx * s + rx * dot * (1 - c);
  const n2 = n * c + cy * s + ry * dot * (1 - c);
  const u2 = u * c + cz * s;
  if (u2 <= 1) return false; // camera would drop to the anchor's height
  const N = MercatorCoordinate.fromLngLat(
    new MercatorCoordinate(P.x + e2 * mu, P.y - n2 * mu, 0).toLngLat(),
    a.elev + u2,
  );
  return applyCamera(map, N, b1, p1, a.elev);
}
