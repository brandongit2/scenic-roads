/**
 * Scenic Roads: whether the globe closes its poles. Upstream's globe draws a cap past the Web
 * Mercator tiles' last row (85.05°) at that row's colour and height, and fills (fill and
 * fill-extrusion buckets' subdivision) close the poles likewise; a map that draws the polar caps
 * itself (the app's polar layer, docs/polar.md) turns them off, so its own cap shows. Read when a
 * tile's mesh is made (meshes are cached by whether they reach a pole, so either value keeps its
 * own) and when a vector tile is parsed (the projection's subdivision setting carries it to the
 * worker). Set through `Map#setPoleCaps`.
 */
export const poleCaps = {north: true, south: true};

/** How near a pole the globe's camera may look where the map draws that pole itself. */
const POLAR_LATITUDE = 89.5;

/**
 * Scenic Roads: the globe camera's centre `lat` held to the latitudes the map draws: ±85.05° (the
 * Web Mercator tiles' end, upstream's limit), or nearer a pole whose cap upstream's globe doesn't
 * close (the map draws it: `poleCaps`).
 */
export function clampGlobeLatitude(lat: number, mercatorMax: number): number {
    const south = poleCaps.south ? -mercatorMax : -POLAR_LATITUDE;
    const north = poleCaps.north ? mercatorMax : POLAR_LATITUDE;
    return Math.min(Math.max(lat, south), north);
}

/**
 * Scenic Roads: the ground's height (m, before the terrain's exaggeration) at a point past the Web
 * Mercator tiles (|lat| > 85.05°), where MapLibre's terrain has no tiles and reads 0; null where the
 * provider has none (then 0, as upstream). Given by the map that draws the poles (the app's polar
 * layer, from its loaded tiles) through `Map#setPoleGround`; read by `Terrain`'s elevation lookups,
 * so the camera's centre, its minimum height and the frustum's ground follow the polar caps.
 */
export type PoleGround = (lng: number, lat: number) => number | null;
export const poleGround: {fn: PoleGround | null} = {fn: null};
