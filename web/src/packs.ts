// A tile's pack, as the server finds it (crates/server Data::pack_of), and the terrain's finest zoom
// by area: pure, for the main thread and the contours' worker (contour/dem.worker.ts).

/** A tiled layer's packs' versions (meta.packs): the root's (z0–2), the lo packs' (z3–8) by z3
 * tile, the hi packs' (z9–14) by z6 tile and the terrain's deep packs (past z12, where it has
 * any: pipeline::terrain_deep) by z10 tile, "z/x/y". */
export interface LayerPacks {
  root?: string;
  lo?: Record<string, string>;
  hi?: Record<string, string>;
  deep?: Record<string, string>;
}

/** The deep packs' tile zoom (pipeline::terrain_deep::PACK_Z); they hold the zooms past it + 2. */
const DEEP_Z = 10;

/** The version token of tile z/x/y of a layer with packs `l`: its pack's, or '' when no pack holds it. */
export function packToken(l: LayerPacks, z: number, x: number, y: number): string {
  if (z <= 2) return l.root ?? '';
  // ('' past z12 outside the deep packs: the server makes those tiles from their ancestor and
  // serves them uncached for good (no current token), so no stale copy outlives a catalog.)
  if (z > DEEP_Z + 2 && l.deep) return l.deep[`${DEEP_Z}/${x >> (z - DEEP_Z)}/${y >> (z - DEEP_Z)}`] ?? '';
  const pz = z <= 8 ? 3 : 6;
  return (z <= 8 ? l.lo : l.hi)?.[`${pz}/${x >> (z - pz)}/${y >> (z - pz)}`] ?? '';
}

/** A raster source's finest zoom by area (meta.terrainAreas; our MapLibre's `scenic:zoom-by-area`):
 * its tiles of zoom `zoom` that go past its maxzoom, "x/y", and the native zoom of each (the
 * terrain's: pipeline::terrain_deep::native_zoom). */
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
