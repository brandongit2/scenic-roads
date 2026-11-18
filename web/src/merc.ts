// Web Mercator and the Earth's size, the one copy the app and its workers use (the Rust side's is
// roadcore::geo). x and y are the world's unit square: x from 0 at 180° W to 1 at 180° E, y from 0 at the top
// (MERCATOR_LAT north) to 1 at the bottom.

/** The Web Mercator tiles' last latitude: y is 0 here and 1 at its negative. */
export const MERCATOR_LAT = 85.05112877980659;

/** A latitude held to the Mercator tiles' (±MERCATOR_LAT). */
export const clampLat = (lat: number) => Math.max(-MERCATOR_LAT, Math.min(MERCATOR_LAT, lat));

export const mercX = (lon: number) => (lon + 180) / 360;

/** y of a latitude, unclamped: past ±MERCATOR_LAT it leaves [0, 1], and the poles are ∓Infinity. */
export const mercY = (lat: number) => {
  const s = Math.sin((lat * Math.PI) / 180);
  return 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI);
};

/** y of a latitude held to ±MERCATOR_LAT (so within [0, 1] up to rounding). */
export const mercYClamped = (lat: number) => mercY(clampLat(lat));

/** Just under 1: the In forms keep a point inside the square, so its tile at any zoom exists. */
const IN = 1 - 1e-9;

/** The Mercator world's width in metres (its equator: 2π × WGS 84's semi-major axis, MapLibre's):
 * a unit of x is this many metres times cos(latitude) on the ground. */
export const MERC_CIRC = 40075016.686;

/** The Earth's mean radius (m), for distances on the ground (roadcore::geo::EARTH_R) and MapLibre's
 * globe. */
export const EARTH_R = 6371008.8;

/** WGS 84's semi-major axis (m) and flattening (roadcore::geo::WGS84_A, 1 / WGS84_RF): the polar
 * caps' projection (polar.ts). */
export const WGS84_A = 6378137.0;
export const WGS84_F = 1 / 298.257223563;

/** A rough km per degree, of latitude and of longitude at the equator, where a box's size or a
 * search cell needs no more (inview.ts boxKm, the ferries' terminals and lengths); not EARTH_R's
 * (111.195 km). */
export const ROUGH_KM_PER_DEG = 111.32;
/** The same in metres. */
export const ROUGH_M_PER_DEG = 111320;
/** A rough km per degree of latitude (the ferries' lengths). */
export const ROUGH_KM_PER_DEG_LAT = 110.57;

/** x held to [0, 1). */
export const mercXIn = (lon: number) => Math.min(IN, Math.max(0, mercX(lon)));

/** y held to [0, 1): the poles' rows for points past ±MERCATOR_LAT. */
export const mercYIn = (lat: number) => Math.min(IN, Math.max(0, mercY(lat)));

export const mercLon = (x: number) => x * 360 - 180;

/** The latitude (radians) at y. */
export const mercPhi = (y: number) => Math.atan(Math.sinh(Math.PI * (1 - 2 * y)));

/** The latitude (degrees) at y. */
export const mercLat = (y: number) => (mercPhi(y) * 180) / Math.PI;

/** The column of tiles at zoom z holding a longitude, held to the world's (0 … 2^z - 1). */
export const tileX = (lon: number, z: number) => {
  const n = 2 ** z;
  return Math.min(n - 1, Math.max(0, Math.floor(mercX(lon) * n)));
};

/** The row of tiles at zoom z holding a latitude, held to the world's (0 … 2^z - 1). */
export const tileY = (lat: number, z: number) => {
  const n = 2 ** z;
  return Math.min(n - 1, Math.max(0, Math.floor(mercYClamped(lat) * n)));
};
