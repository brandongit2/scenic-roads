/**
 * Scenic Roads: whether the globe closes its poles. Upstream's globe draws a cap past the Web
 * Mercator tiles' last row (85.05°) at that row's colour and height; a map that draws the polar caps
 * itself (the app's polar layer, docs/polar.md) turns them off, so its own cap shows. Read when a
 * tile's mesh is made (meshes are cached by whether they reach a pole, so either value keeps its
 * own). Set through `Map#setPoleCaps`.
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
