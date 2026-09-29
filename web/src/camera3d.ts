// Cursor-anchored 3D camera moves, on the globe (earth-centred coordinates) and on the flat map
// (mercator) that MapLibre switches to when zoomed in. MapLibre anchors zoom on a surface at the
// camera pivot's height, but with 3D terrain (exaggerated, and a pivot that doesn't follow the
// ground) the point under the cursor can be kilometres above or below that plane, so the anchor
// slides. Here the anchor is a point on the cursor's view ray (the terrain it hits, when that is
// reliable), and the camera moves rigidly:
//   · dolly: along the line from the camera to the anchor (zoom),
//   · orbit: rotation about the anchor (bearing about the vertical, pitch about the camera's
//     horizontal right axis).
// A point on the cursor ray stays under the cursor through a dolly, and a rigid rotation about
// a point keeps it at the same pixel. The new camera position and orientation are then turned
// back into MapLibre's centre / zoom / pivot (applyCamera on the flat map, placeGlobe on the globe).
import { LngLat, MercatorCoordinate, Point, type Map as MLMap } from 'maplibre-gl';

export interface Anchor {
  ll: LngLat;
  /** Rendered height of the point (terrain × exaggeration), metres. */
  elev: number;
  /** True when the anchor is ground the cursor is actually on (not sky / a capped distance). */
  ground: boolean;
}

type Tr = {
  getCameraLngLat: () => LngLat;
  getCameraAltitude: () => number;
  cameraToCenterDistance: number;
  centerPoint: { x: number; y: number };
  fov: number;
  tileSize?: number;
  isGlobeRendering?: boolean;
};
const transform = (map: MLMap) => (map as unknown as { _camera?: { transform?: Tr } })._camera?.transform;

const DEG = Math.PI / 180;

// Earth-centred coordinates on MapLibre's sphere (radius R, altitude along the normal).
type V3 = [number, number, number];
const R = 6371008.8;
const ecef = (ll: { lng: number; lat: number }, alt: number): V3 => {
  const la = ll.lat * DEG, lo = ll.lng * DEG, r = R + alt;
  return [r * Math.cos(la) * Math.sin(lo), r * Math.sin(la), r * Math.cos(la) * Math.cos(lo)];
};
const toLngLat = (p: V3) => {
  const r = Math.hypot(p[0], p[1], p[2]);
  return new LngLat(Math.atan2(p[0], p[2]) / DEG, Math.asin(p[1] / r) / DEG);
};
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
/** Local east / north / up unit vectors at a point on the sphere. */
const enu = (ll: { lng: number; lat: number }) => {
  const la = ll.lat * DEG, lo = ll.lng * DEG;
  const sa = Math.sin(la), ca = Math.cos(la), so = Math.sin(lo), co = Math.cos(lo);
  return { E: [co, 0, -so] as V3, N: [-sa * so, ca, -sa * co] as V3, U: [ca * so, sa, ca * co] as V3 };
};

/**
 * Centre and zoom that put MapLibre's globe camera at C (earth-centred) with the given pitch and
 * bearing. The globe camera sits at distance D from a sea-level centre S, tilted by the pitch
 * from S's vertical toward azimuth bearing + 180°. So |C|² = (R + D cos p)² + (D sin p)² gives
 * D, the angle between S and C is γ = atan(D sin p / (R + D cos p)), and S lies γ from the point
 * below the camera along the great circle that arrives at S heading `bearing`.
 */
function placeGlobe(tr: Tr, C: V3, pitch: number, bearing: number): { center: LngLat; zoom: number } | null {
  const sp = Math.sin(pitch * DEG), cp = Math.cos(pitch * DEG);
  const rc = Math.hypot(C[0], C[1], C[2]);
  const D = -R * cp + Math.sqrt(rc * rc - R * R * sp * sp);
  if (!(D > 0)) return null;
  const g = Math.atan2(D * sp, R + D * cp);
  const sub = toLngLat(C);
  let lat = sub.lat * DEG, lng = sub.lng * DEG;
  if (g > 1e-12) {
    const la1 = lat, lo1 = lng, b = bearing * DEG;
    const sg = Math.sin(g), cg = Math.cos(g);
    let th = b; // initial heading from below the camera toward the centre
    for (let i = 0; i < 12; i++) {
      lat = Math.asin(Math.sin(la1) * cg + Math.cos(la1) * sg * Math.cos(th));
      lng = lo1 + Math.atan2(Math.sin(th) * sg * Math.cos(la1), cg - Math.sin(la1) * Math.sin(lat));
      // Heading on arrival = the bearing from the centre back to the camera's foot + 180°.
      const dl = lo1 - lng;
      const back = Math.atan2(Math.sin(dl) * Math.cos(la1), Math.cos(lat) * Math.sin(la1) - Math.sin(lat) * Math.cos(la1) * Math.cos(dl));
      const err = ((((b - back - Math.PI) % (2 * Math.PI)) + 3 * Math.PI) % (2 * Math.PI)) - Math.PI;
      if (Math.abs(err) < 1e-13) break;
      th += err;
    }
  }
  const center = new LngLat(((((lng / DEG) % 360) + 540) % 360) - 180, lat / DEG);
  // Globe radius in pixels is worldSize / 2π / cos(lat); the camera sits cameraToCenterDistance px out.
  const zoom = Math.log2((2 * Math.PI * Math.cos(lat) * tr.cameraToCenterDistance * R) / (D * (tr.tileSize ?? 512)));
  return Number.isFinite(zoom) ? { center, zoom } : null;
}

/**
 * Rotate a camera offset (east, north, up metres from the pivot) by dBearing clockwise about the
 * vertical, then by dPitch about the right axis for the new bearing b1 (Rodrigues).
 */
function orbitOffset(e: number, n: number, u: number, dBearing: number, b1: number, dPitch: number): V3 {
  const tb = dBearing * DEG;
  [e, n] = [e * Math.cos(tb) + n * Math.sin(tb), -e * Math.sin(tb) + n * Math.cos(tb)];
  const th = dPitch * DEG;
  const rx = Math.cos(b1 * DEG), ry = -Math.sin(b1 * DEG);
  const d = rx * e + ry * n;
  const cx = ry * u, cy = -rx * u, cz = rx * n - ry * e; // r × v
  const c = Math.cos(th), s = Math.sin(th);
  return [e * c + cx * s + rx * d * (1 - c), n * c + cy * s + ry * d * (1 - c), u * c + cz * s];
}

/** True while MapLibre renders the globe (its camera then lives on a sphere). */
export const isGlobe = (map: MLMap) => transform(map)?.isGlobeRendering === true;

// On the globe the pivot stays at sea level: the globe camera ignores it, but the flat map
// blended in during the hand-off uses it, and jumpTo would otherwise reset it to the terrain.

/**
 * Globe moves. The camera moves rigidly in earth-centred space (toward the anchor, or about it)
 * and is placed there with the new pitch and bearing measured in the new centre's frame, as
 * MapLibre defines them (placeGlobe). What is left (terrain detail differing slightly from what
 * the anchor was picked on; the frame turning over long moves) is removed by shifting the camera
 * until the anchor projects exactly onto the cursor: across the view, or for pans (`level`)
 * horizontally, so that repeated pans keep the height. Refuses moves that bring the camera too
 * close to the terrain or past a limit, before touching the map.
 */
function globeMove(map: MLMap, tr: Tr, a: Anchor, px: number, py: number, C: V3, pitch: number, bearing: number, level = false): boolean | null {
  const clear = (P: V3) => Math.hypot(P[0], P[1], P[2]) - R >= (map.queryTerrainElevation(toLngLat(P)) ?? 0) + 30;
  if (!clear(C)) return false;
  let pl = placeGlobe(tr, C, pitch, bearing);
  if (!pl) return null; // no globe camera has this orientation here: caller falls back
  if (!inZoomRange(map, pl.zoom)) return false;
  const before = { center: map.getCenter(), zoom: map.getZoom(), pitch: map.getPitch(), bearing: map.getBearing(), elevation: map.getCenterElevation() };
  const A = ecef(a.ll, a.elev);
  let err = Infinity;
  for (let i = 0; i < 4; i++) {
    if (!pl || !inZoomRange(map, pl.zoom)) break;
    map.jumpTo({ center: pl.center, zoom: pl.zoom, pitch, bearing, elevation: 0 });
    const q = map.project(a.ll);
    err = Math.hypot(q.x - px, q.y - py);
    if (!(err > 0.25)) break;
    const { E, N, U } = enu(pl.center);
    if (level) {
      // Shift the camera horizontally (same height) until the anchor is seen at the cursor.
      const alt = Math.hypot(C[0], C[1], C[2]) - R;
      const mv = panMove([A[0] - C[0], A[1] - C[1], A[2] - C[2]], E, N, U, pitch, bearing, tr.cameraToCenterDistance, px - tr.centerPoint.x, py - tr.centerPoint.y);
      if (!mv) break;
      C = ecef(toLngLat([C[0] + mv[0], C[1] + mv[1], C[2] + mv[2]]), alt);
    } else {
      // Shift the camera across the view (a point moves by −shift × focal length ÷ depth).
      const sp = Math.sin(pitch * DEG), cp = Math.cos(pitch * DEG), sb = Math.sin(bearing * DEG), cb = Math.cos(bearing * DEG);
      const f = [0, 1, 2].map((k) => sp * (sb * E[k] + cb * N[k]) - cp * U[k]) as V3;
      const r = [0, 1, 2].map((k) => cb * E[k] - sb * N[k]) as V3;
      const up = cross(r, f);
      const k = dot([A[0] - C[0], A[1] - C[1], A[2] - C[2]], f) / tr.cameraToCenterDistance;
      const ex = q.x - px, ey = q.y - py;
      C = [0, 1, 2].map((j) => C[j] + k * (ex * r[j] - ey * up[j])) as V3;
    }
    pl = placeGlobe(tr, C, pitch, bearing);
  }
  if (!(err <= 3) || !clear(ecef(tr.getCameraLngLat(), tr.getCameraAltitude()))) {
    map.jumpTo(before);
    return false;
  }
  return true;
}

/**
 * Fallback for orientations no globe camera can have at the target position (its view centre
 * would miss the planet): MapLibre does the move, then pans (Newton, with a probed Jacobian)
 * until the anchor is back under the cursor.
 */
function globeMoveMapLibre(map: MLMap, tr: Tr, a: Anchor, px: number, py: number, move: () => void): boolean {
  const before = { center: map.getCenter(), zoom: map.getZoom(), pitch: map.getPitch(), bearing: map.getBearing(), elevation: map.getCenterElevation() };
  move();
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
      map.jumpTo({ center: c0, elevation: 0 });
      map.panBy([0, h], { animate: false });
      const qy = map.project(a.ll);
      map.jumpTo({ center: c0, elevation: 0 });
      J = [(qx.x - q.x) / h, (qy.x - q.x) / h, (qx.y - q.y) / h, (qy.y - q.y) / h];
    }
    const det = J[0] * J[3] - J[1] * J[2];
    if (!Number.isFinite(det) || Math.abs(det) < 1e-6) break;
    // Solve J·pan = −e.
    map.panBy([(-ex * J[3] + ey * J[1]) / det, (-ey * J[0] + ex * J[2]) / det], { animate: false });
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

const inZoomRange = (map: MLMap, z: number) => z <= map.getMaxZoom() + 1e-6 && z >= map.getMinZoom() - 1e-6;

/**
 * Beyond this camera → anchor distance the planet fills a large part of the view and each move
 * turns the centre's frame a lot; MapLibre's own moves (then a pan) are the more accurate there,
 * and relief is irrelevant at that distance.
 */
const PLANET_SCALE = 1000e3;

function dollyGlobe(map: MLMap, tr: Tr, a: Anchor, dz: number, px: number, py: number): boolean {
  // The camera's distance to the anchor scales by 2^−dz. (MapLibre's zoom scales the distance to
  // sea level, which near high ground is several times the distance to the ground.)
  const C = ecef(tr.getCameraLngLat(), tr.getCameraAltitude()), A = ecef(a.ll, a.elev), k = 2 ** -dz;
  if (Math.hypot(C[0] - A[0], C[1] - A[1], C[2] - A[2]) < PLANET_SCALE) {
    const C2: V3 = [A[0] + (C[0] - A[0]) * k, A[1] + (C[1] - A[1]) * k, A[2] + (C[2] - A[2]) * k];
    const done = globeMove(map, tr, a, px, py, C2, map.getPitch(), map.getBearing());
    if (done !== null) return done;
  }
  const z = map.getZoom() + dz;
  if (!inZoomRange(map, z)) return false;
  const around = seaLevelAt(map, tr, px, py);
  return globeMoveMapLibre(map, tr, a, px, py, () => map.zoomTo(z, around ? { around, duration: 0 } : { duration: 0 }));
}

function orbitGlobe(map: MLMap, tr: Tr, a: Anchor, dBearing: number, dPitch: number, px: number, py: number): boolean {
  const p0 = map.getPitch(), b1 = map.getBearing() + dBearing;
  const p1 = Math.max(0, Math.min(map.getMaxPitch(), p0 + dPitch));
  // Rotate the camera about the anchor in the anchor's local frame.
  const { E, N, U } = enu(a.ll);
  const C = ecef(tr.getCameraLngLat(), tr.getCameraAltitude()), A = ecef(a.ll, a.elev);
  const v: V3 = [C[0] - A[0], C[1] - A[1], C[2] - A[2]];
  if (Math.hypot(v[0], v[1], v[2]) < PLANET_SCALE) {
    const [e, n, u] = orbitOffset(dot(v, E), dot(v, N), dot(v, U), dBearing, b1, p1 - p0);
    if (u <= 1) return false; // camera would drop to the anchor's height
    const C2 = [0, 1, 2].map((i) => A[i] + e * E[i] + n * N[i] + u * U[i]) as V3;
    const done = globeMove(map, tr, a, px, py, C2, p1, b1);
    if (done !== null) return done;
  }
  return globeMoveMapLibre(map, tr, a, px, py, () => map.jumpTo({ bearing: b1, pitch: p1, elevation: 0 }));
}

/** Highest rendered ground to expect: Mt Washington (1917 m) is the tallest summit covered. */
const MAX_RELIEF = 2000;

/**
 * The terrain point on the cursor ray, marched on the globe. MapLibre's own terrain ray-cast
 * gives up while the globe is mostly blended into the flat map (the last ~20 % of the hand-off)
 * and returns the sea-level point instead, which is off by tens of pixels up close.
 */
function marchGlobe(map: MLMap, tr: Tr, px: number, py: number): Anchor | null {
  const sea = seaLevelAt(map, tr, px, py);
  if (!sea) return null;
  const s = (tr as unknown as { locationToScreenPoint: (ll: LngLat) => { x: number; y: number } }).locationToScreenPoint(sea);
  if (Math.hypot(s.x - px, s.y - py) > 3) return null; // the ray misses the planet (horizon point)
  const C = ecef(tr.getCameraLngLat(), tr.getCameraAltitude()), S = ecef(sea, 0);
  const d: V3 = [S[0] - C[0], S[1] - C[1], S[2] - C[2]];
  const tSea = Math.hypot(d[0], d[1], d[2]);
  d[0] /= tSea; d[1] /= tSea; d[2] /= tSea;
  // From where the ray drops below the highest terrain, to sea level.
  const top = R + MAX_RELIEF * (map.getTerrain()?.exaggeration ?? 1);
  const b = dot(C, d), disc = b * b - (dot(C, C) - top * top);
  const t0 = disc > 0 ? Math.max(0, -b - Math.sqrt(disc)) : 0;
  const at = (t: number) => {
    const P: V3 = [C[0] + d[0] * t, C[1] + d[1] * t, C[2] + d[2] * t];
    const ll = toLngLat(P);
    const g = map.queryTerrainElevation(ll) ?? 0;
    return { ll, g, below: Math.hypot(P[0], P[1], P[2]) - R <= g };
  };
  const n = 96;
  let lo = t0;
  for (let i = 1; i <= n; i++) {
    let hi = t0 + ((tSea - t0) * i) / n;
    if (!at(hi).below) {
      lo = hi;
      continue;
    }
    for (let k = 0; k < 30 && hi - lo > 0.05; k++) {
      const mid = (lo + hi) / 2;
      if (at(mid).below) hi = mid;
      else lo = mid;
    }
    const h = at(hi);
    return { ll: h.ll, elev: h.g, ground: true };
  }
  return { ll: sea, elev: 0, ground: true };
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
    if (Math.hypot(q.x - px, q.y - py) <= 3) return { ll, elev: map.queryTerrainElevation(ll) ?? 0, ground: true };
    return marchGlobe(map, tr, px, py);
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

/** Globe mode is on (it may be rendering flat right now, zoomed in past the hand-off). */
const globeOn = (map: MLMap) => {
  const t = map.getProjection()?.type;
  return t !== undefined && t !== 'mercator';
};

/**
 * The pivot (MapLibre's centre elevation) sits at sea level while zoomed out. From this zoom up
 * it is lifted to the ground at the view centre, so zoom-based detail follows the real distance
 * to the ground. With the globe on, MapLibre's globe camera ignores the pivot, so it must stay at
 * sea level until past the globe → flat hand-off (zoom 15.5–16.5) or the view would jump there.
 */
const groundPivotZoom = (map: MLMap) => (globeOn(map) ? 17 : 12.5);

/**
 * Camera options showing `target` (rendered height `elev`) at the view centre from the distance
 * `zoom` gives over flat ground at sea level. Zoom levels are measured from the pivot, which is
 * sea level on the globe, so over high ground a plain flyTo({ zoom }) would put the camera much
 * closer to it, or inside it.
 */
export function frame(map: MLMap, target: LngLat, elev: number, zoom: number, pitch = map.getPitch(), bearing = map.getBearing()) {
  const tr = transform(map);
  if (tr && globeOn(map)) {
    const D = (tr.cameraToCenterDistance * 2 * Math.PI * R * Math.cos(target.lat * DEG)) / ((tr.tileSize ?? 512) * 2 ** zoom);
    const { E, N, U } = enu(target);
    const sp = Math.sin(pitch * DEG), cp = Math.cos(pitch * DEG), sb = Math.sin(bearing * DEG), cb = Math.cos(bearing * DEG);
    const P = ecef(target, elev);
    const C = [0, 1, 2].map((k) => P[k] - D * (sp * (sb * E[k] + cb * N[k]) - cp * U[k])) as V3;
    const pl = placeGlobe(tr, C, pitch, bearing);
    if (pl) return { center: pl.center, zoom: pl.zoom, pitch, bearing, elevation: 0 };
  }
  return { center: target, zoom, pitch, bearing, elevation: elev };
}

/** Mercator zoom for a camera at `alt` looking at pitch p (radians) with the pivot at `E`. */
function zoomFor(map: MLMap, tr: Tr, alt: number, E: number, p: number): number {
  const d = (alt - E) / Math.cos(p);
  const mu = MercatorCoordinate.fromLngLat(map.getCenter()).meterInMercatorCoordinateUnits();
  return Math.log2(map.getCanvas().clientHeight / 2 / Math.tan((tr.fov * DEG) / 2) / (d * mu) / 512);
}

/**
 * Ground points of screen samples for the tilted tile cover. MapLibre's flat unprojection uses the
 * camera pivot's level, which on the globe is sea level: over high (exaggerated) terrain that lands
 * far beyond the real ground, and the tiles near the camera never load. Instead each sample's view
 * ray is intersected with levels spanning the terrain heights in view (probed on a 5 × 3 grid), so
 * the union covers wherever the ray meets the ground; each point carries its ground resolution
 * (m per CSS px) from its distance. Terrain lookups are few (the probes), the rest is arithmetic.
 */
export function coverSamples(map: MLMap, pts: { x: number; y: number }[]): { lng: number; lat: number; mpp: number }[] | null {
  const tr = transform(map);
  if (!tr || !map.getTerrain()) return null;
  const canvas = map.getCanvas();
  const W = canvas.clientWidth, H = canvas.clientHeight;
  const camLL = tr.getCameraLngLat(), camAlt = tr.getCameraAltitude();
  const fov = ((tr as unknown as { fov?: number }).fov ?? 36.87) * DEG;
  const radPerPx = (2 * Math.tan(fov / 2)) / H;
  const globe = globeOn(map);
  // Terrain heights in view.
  let lo = Infinity, hi = -Infinity;
  for (const fy of [0.35, 0.65, 0.95]) {
    for (const fx of [0.05, 0.275, 0.5, 0.725, 0.95]) {
      const g = probeGround(map, tr, fx * W, fy * H);
      if (g === null) continue;
      lo = Math.min(lo, g);
      hi = Math.max(hi, g);
    }
  }
  if (!Number.isFinite(lo)) return null;
  const levels = hi - lo < 20 ? [lo] : [lo, (lo + hi) / 2, hi];
  const out: { lng: number; lat: number; mpp: number }[] = [];
  if (globe) {
    const C = ecef(camLL, camAlt);
    const CC = dot(C, C);
    for (const p of pts) {
      const sea = seaLevelAt(map, tr, p.x, p.y);
      if (!sea) continue;
      const S = ecef(sea, 0);
      const d: V3 = [S[0] - C[0], S[1] - C[1], S[2] - C[2]];
      const L = Math.hypot(d[0], d[1], d[2]);
      d[0] /= L; d[1] /= L; d[2] /= L;
      const b = dot(C, d);
      for (const e of levels) {
        const r = R + e, disc = b * b - (CC - r * r);
        if (disc <= 0) continue;
        const t = -b - Math.sqrt(disc);
        if (t <= 0) continue;
        const ll = toLngLat([C[0] + d[0] * t, C[1] + d[1] * t, C[2] + d[2] * t]);
        out.push({ lng: ll.lng, lat: ll.lat, mpp: Math.max(0.01, t * radPerPx) });
      }
    }
  } else {
    const t = tr as unknown as { screenPointToLocationAtElevation?: (p: Point, e: number) => LngLat | undefined };
    if (!t.screenPointToLocationAtElevation) return null;
    const cosLat = Math.cos(camLL.lat * DEG);
    for (const p of pts) {
      for (const e of levels) {
        const ll = t.screenPointToLocationAtElevation(new Point(p.x, p.y), e);
        if (!ll || !Number.isFinite(ll.lng) || !Number.isFinite(ll.lat)) continue;
        const dx = (ll.lng - camLL.lng) * DEG * R * cosLat, dy = (ll.lat - camLL.lat) * DEG * R;
        out.push({ lng: ll.lng, lat: ll.lat, mpp: Math.max(0.01, Math.hypot(dx, dy, camAlt - e) * radPerPx) });
      }
    }
  }
  return out;
}

/** Terrain height (m, exaggerated) where a screen point's ray meets the ground (refined twice). */
function probeGround(map: MLMap, tr: Tr, px: number, py: number): number | null {
  let e = 0;
  let ll = pointAt(map, tr, px, py, e);
  if (!ll) return null;
  for (let i = 0; i < 3; i++) {
    e = map.queryTerrainElevation(ll) ?? e;
    const next = pointAt(map, tr, px, py, e);
    if (!next) break;
    ll = next;
  }
  return e;
}

function pointAt(map: MLMap, tr: Tr, px: number, py: number, elev: number): LngLat | null {
  if (globeOn(map)) {
    const sea = seaLevelAt(map, tr, px, py);
    if (!sea) return null;
    const C = ecef(tr.getCameraLngLat(), tr.getCameraAltitude()), S = ecef(sea, 0);
    const d: V3 = [S[0] - C[0], S[1] - C[1], S[2] - C[2]];
    const L = Math.hypot(d[0], d[1], d[2]);
    d[0] /= L; d[1] /= L; d[2] /= L;
    const r = R + elev, b = dot(C, d), disc = b * b - (dot(C, C) - r * r);
    if (disc <= 0) return sea;
    const t = -b - Math.sqrt(disc);
    return t > 0 ? toLngLat([C[0] + d[0] * t, C[1] + d[1] * t, C[2] + d[2] * t]) : sea;
  }
  const t = tr as unknown as { screenPointToLocationAtElevation?: (p: Point, e: number) => LngLat | undefined };
  const ll = t.screenPointToLocationAtElevation?.(new Point(px, py), elev);
  return ll && Number.isFinite(ll.lng) && Number.isFinite(ll.lat) ? ll : null;
}

/**
 * Depth precision on the globe. MapLibre's globe view uses a 0.5 px near plane (its 2D layers
 * compute their own depth, so it didn't matter to them), but the terrain mesh, its depth texture
 * and 3D custom layers use the perspective depth, where everything then lives in the last pixel
 * of sums of ~2.6 million: float32 leaves it good to only ~20 % of the distance, so roads and
 * terrain z-fight (roads hatched with the terrain's triangles, flickering as you move) and the
 * terrain fights itself. While the globe renders, use the flat map's near plane (viewport height
 * ÷ 50), at most half the camera's height above the ground so nothing close is clipped: 28× the
 * precision. (Circles and symbols test themselves against the terrain depth texture with the same
 * perspective depth: see maplibreTerrainVisibility in vite.config.ts.) Call on every camera change;
 * the flat map keeps MapLibre's own planes.
 */
let depthTuned = false;
export function tuneDepth(map: MLMap): void {
  const tr = transform(map) as unknown as {
    isGlobeRendering?: boolean; overrideNearFarZ?: (n: number, f: number) => void; clearNearFarZOverride?: () => void;
    nearZ: number; farZ: number; height: number; worldSize: number; cameraToCenterDistance: number; pixelsPerMeter: number;
    getCameraAltitude: () => number; getCameraLngLat: () => LngLat;
  } | undefined;
  if (!tr?.overrideNearFarZ || !tr.clearNearFarZOverride) return;
  if (!tr.isGlobeRendering) {
    if (depthTuned) {
      depthTuned = false;
      tr.clearNearFarZOverride();
    }
    return;
  }
  const ground = map.getTerrain() ? map.queryTerrainElevation(tr.getCameraLngLat()) ?? 0 : 0;
  const clearance = Math.max(1, tr.getCameraAltitude() - ground) * tr.pixelsPerMeter;
  const near = Math.max(0.5, Math.min(tr.height / 50, clearance * 0.5));
  const lat = map.getCenter().lat;
  const far = tr.cameraToCenterDistance + (2 * tr.worldSize) / (2 * Math.PI) / Math.cos((lat * Math.PI) / 180);
  if (depthTuned && Math.abs(tr.nearZ - near) < near * 0.02 && Math.abs(tr.farZ - far) < far * 0.02) return;
  depthTuned = true;
  tr.overrideNearFarZ(near, far);
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
  if (zoomFor(map, tr, alt, 0, p) >= groundPivotZoom(map)) {
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
  // Below groundPivotZoom the pivot stays at sea level (see there).
  if (E !== 0 && zoomFor(map, tr, alt, 0, p) < groundPivotZoom(map)) E = 0;
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
  const b0 = map.getBearing(), p0 = map.getPitch();
  const p1 = Math.max(0, Math.min(map.getMaxPitch(), p0 + dPitch));
  const b1 = b0 + dBearing;
  const C = MercatorCoordinate.fromLngLat(tr.getCameraLngLat(), tr.getCameraAltitude());
  const P = MercatorCoordinate.fromLngLat(a.ll, a.elev);
  const mu = P.meterInMercatorCoordinateUnits();
  // Camera relative to the anchor, local east/north/up metres.
  const [e2, n2, u2] = orbitOffset((C.x - P.x) / mu, -(C.y - P.y) / mu, tr.getCameraAltitude() - a.elev, dBearing, b1, p1 - p0);
  if (u2 <= 1) return false; // camera would drop to the anchor's height
  const N = MercatorCoordinate.fromLngLat(
    new MercatorCoordinate(P.x + e2 * mu, P.y - n2 * mu, 0).toLngLat(),
    a.elev + u2,
  );
  return applyCamera(map, N, b1, p1, a.elev);
}

/** Screen point the view centre is drawn at (the canvas centre, shifted by any padding). */
export function centrePoint(map: MLMap): { x: number; y: number } {
  const tr = transform(map);
  const c = map.getCanvas();
  return tr ? { x: tr.centerPoint.x, y: tr.centerPoint.y } : { x: c.clientWidth / 2, y: c.clientHeight / 2 };
}

/**
 * Horizontal camera move (in the basis E, N, U) after which the point at `rel` from the camera
 * is seen (sx, sy) px from the view centre's screen point, for a camera with this pitch and
 * bearing and focal length F px: solve rel = α·right + β·ahead + t·ray. Null if that pixel's
 * ray cannot reach the point (it would have to look up at it).
 */
function panMove(rel: V3, E: V3, N: V3, U: V3, pitch: number, bearing: number, F: number, sx: number, sy: number): V3 | null {
  const sp = Math.sin(pitch * DEG), cp = Math.cos(pitch * DEG), sb = Math.sin(bearing * DEG), cb = Math.cos(bearing * DEG);
  const f = [0, 1, 2].map((k) => sp * (sb * E[k] + cb * N[k]) - cp * U[k]) as V3;
  const r = [0, 1, 2].map((k) => cb * E[k] - sb * N[k]) as V3;
  const h = [0, 1, 2].map((k) => sb * E[k] + cb * N[k]) as V3;
  const up = cross(r, f);
  const v = [0, 1, 2].map((k) => F * f[k] + sx * r[k] - sy * up[k]) as V3;
  const det = dot(r, cross(h, v));
  if (Math.abs(det) < 1e-9) return null;
  const al = dot(rel, cross(h, v)) / det, be = dot(r, cross(rel, v)) / det, t = dot(r, cross(h, rel)) / det;
  if (!(t > 0)) return null;
  return [0, 1, 2].map((k) => al * r[k] + be * h[k]) as V3;
}

/**
 * MapLibre's own grab: put `ll` (on the pivot plane) at screen point (x, y), as its drag pan
 * does. Right where the ground is close to the pivot plane (see ownPan).
 */
export function setLocationAt(map: MLMap, ll: LngLat, x: number, y: number): void {
  const tr = transform(map) as unknown as { clone?: () => { setLocationAtPoint: (l: LngLat, p: { x: number; y: number }) => void; center: LngLat; zoom: number } };
  const t2 = tr?.clone?.();
  if (!t2) return;
  t2.setLocationAtPoint(ll, { x, y } as never);
  map.jumpTo({ center: t2.center, zoom: t2.zoom, elevation: map.getCenterElevation() });
}

/**
 * Whether to pan with panTo rather than MapLibre's pan, which moves the pivot plane (sea level on
 * the globe): on the globe, when the point sits far enough off that plane that MapLibre's pan
 * would move it at a visibly wrong speed (> 3 %); on the flat map (where panTo is exact) unless
 * zoomed far out, as panTo works in local metres there. Zoomed out on the globe, relief is
 * negligible next to the camera distance and MapLibre's pan is the natural one.
 */
export function ownPan(map: MLMap, a: Anchor): boolean {
  const tr = transform(map);
  if (!tr) return false;
  const C = ecef(tr.getCameraLngLat(), tr.getCameraAltitude()), A = ecef(a.ll, a.elev);
  const d = Math.hypot(C[0] - A[0], C[1] - A[1], C[2] - A[2]);
  if (!tr.isGlobeRendering) return d < 100e3;
  return Math.abs(a.elev - map.getCenterElevation()) > 0.03 * d;
}

/**
 * Move the camera horizontally (keeping its height, but rising over ground in the way) so that
 * anchor `a` is seen at (tx, ty). Pans use this so the ground moves with the fingers at any
 * height: MapLibre's own pan moves the pivot plane, which on the globe is sea level, far below
 * high ground, so near a summit it would move the view many times too fast.
 */
export function panTo(map: MLMap, a: Anchor, tx: number, ty: number): boolean {
  const tr = transform(map);
  if (!tr) return false;
  const pitch = map.getPitch(), bearing = map.getBearing();
  const sx = tx - tr.centerPoint.x, sy = ty - tr.centerPoint.y;
  const camLL = tr.getCameraLngLat(), alt = tr.getCameraAltitude();
  if (tr.isGlobeRendering) {
    // Globe: rotate the camera about the earth's centre, taking along the point T that is now
    // seen at the target pixel (at the anchor's radius) onto the anchor; the anchor is then seen
    // there. The camera keeps its altitude; pitch and bearing keep their values.
    const C = ecef(camLL, alt), A = ecef(a.ll, a.elev);
    const { E, N, U } = enu(map.getCenter());
    const sp = Math.sin(pitch * DEG), cp = Math.cos(pitch * DEG), sb = Math.sin(bearing * DEG), cb = Math.cos(bearing * DEG);
    const f = [0, 1, 2].map((k) => sp * (sb * E[k] + cb * N[k]) - cp * U[k]) as V3;
    const r = [0, 1, 2].map((k) => cb * E[k] - sb * N[k]) as V3;
    const up = cross(r, f);
    const v = [0, 1, 2].map((k) => tr.cameraToCenterDistance * f[k] + sx * r[k] - sy * up[k]) as V3;
    const vl = Math.hypot(v[0], v[1], v[2]);
    const u: V3 = [v[0] / vl, v[1] / vl, v[2] / vl];
    const rA = Math.hypot(A[0], A[1], A[2]);
    const b = dot(C, u), disc = b * b - (dot(C, C) - rA * rA);
    if (disc < 0) return false; // that pixel sees nothing at the anchor's height
    const t = -b - Math.sqrt(disc);
    if (!(t > 0)) return false;
    const T: V3 = [C[0] + u[0] * t, C[1] + u[1] * t, C[2] + u[2] * t];
    const ax = cross(T, A), s = Math.hypot(ax[0], ax[1], ax[2]) / (rA * rA), co = dot(T, A) / (rA * rA);
    let C2 = C;
    if (s > 1e-15) {
      const k: V3 = [ax[0] / (s * rA * rA), ax[1] / (s * rA * rA), ax[2] / (s * rA * rA)];
      const kc = cross(k, C), kd = dot(k, C);
      C2 = [0, 1, 2].map((i) => C[i] * co + kc[i] * s + k[i] * kd * (1 - co)) as V3; // Rodrigues
    }
    const ll2 = toLngLat(C2);
    const g = (map.queryTerrainElevation(ll2) ?? 0) + 31;
    if (alt < g) C2 = ecef(ll2, g); // rise over ground in the way
    return globeMove(map, tr, a, tx, ty, C2, pitch, bearing, true) === true;
  }
  // Flat map: east / north / up metres from the camera.
  const Cm = MercatorCoordinate.fromLngLat(camLL, alt), Am = MercatorCoordinate.fromLngLat(a.ll, a.elev);
  const mu = Cm.meterInMercatorCoordinateUnits();
  const rel: V3 = [(Am.x - Cm.x) / mu, -(Am.y - Cm.y) / mu, a.elev - alt];
  const mv = panMove(rel, [1, 0, 0], [0, 1, 0], [0, 0, 1], pitch, bearing, tr.cameraToCenterDistance, sx, sy);
  if (!mv) return false;
  const ll2 = new MercatorCoordinate(Cm.x + mv[0] * mu, Cm.y - mv[1] * mu, 0).toLngLat();
  const alt2 = Math.max(alt + mv[2], (map.queryTerrainElevation(ll2) ?? 0) + 31);
  return applyCamera(map, MercatorCoordinate.fromLngLat(ll2, alt2), bearing, pitch, map.getCenterElevation());
}
