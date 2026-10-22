// A tile's pack, as the server finds it (crates/server Data::pack_of), and the terrain's finest zoom
// by area: pure, for the main thread and the contours' worker (contour/dem.worker.ts).

/** A tiled layer's packs' versions (meta.packs): the root's (z0–2), the lo packs' (z3–8) by z3
 * tile, the hi packs' (z9–14) by z6 tile and the terrain's fine packs (past z12, where it has
 * any: pipeline::terrain_fine) by z10 tile, "z/x/y". */
export interface LayerPacks {
  root?: string;
  lo?: Record<string, string>;
  hi?: Record<string, string>;
  fine?: Record<string, string>;
}

/** The fine packs' tile zoom (pipeline::terrain_fine::PACK_Z); they hold the zooms past it + 2. */
const FINE_Z = 10;

/** The version token of tile z/x/y of a layer with packs `l`: its pack's, or '' when no pack holds it. */
export function packToken(l: LayerPacks, z: number, x: number, y: number): string {
  if (z <= 2) return l.root ?? '';
  // ('' past z12 outside the fine packs: the server makes those tiles from their ancestor and
  // serves them uncached for good (no current token), so no stale copy outlives a catalog.)
  if (z > FINE_Z + 2 && l.fine) return l.fine[`${FINE_Z}/${x >> (z - FINE_Z)}/${y >> (z - FINE_Z)}`] ?? '';
  const pz = z <= 8 ? 3 : 6;
  return (z <= 8 ? l.lo : l.hi)?.[`${pz}/${x >> (z - pz)}/${y >> (z - pz)}`] ?? '';
}

/** A raster source's finest zoom by area (meta.terrainAreas; our MapLibre's `scenic:zoom-by-area`):
 * its tiles of zoom `zoom` that go finer, "x/y", and the finest zoom of each. */
export interface TerrainAreas {
  zoom: number;
  tiles: Record<string, number>;
}

/** The finest zoom of tile z/x/y's area (`areas`), else `maxzoom`. */
export function areaMaxZoom(areas: TerrainAreas | undefined, maxzoom: number, z: number, x: number, y: number): number {
  if (!areas || z < areas.zoom) return maxzoom;
  const k = z - areas.zoom;
  return areas.tiles[`${Math.floor(x / 2 ** k)}/${Math.floor(y / 2 ** k)}`] ?? maxzoom;
}
