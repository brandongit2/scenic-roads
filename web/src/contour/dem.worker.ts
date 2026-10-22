// The contour lines' vector tiles, made off the main thread from the terrain's DEM tiles: our copy
// of maplibre-contour's (BSD-3-Clause, ./LICENSE; its HeightTile, isolines and vector-tile encoder
// are ./height-tile.ts, ./isolines.ts and ./vtpbf.ts as it has them), cut to what the map uses, and
// reading the DEM by area: where the catalog has the terrain finer than its base (the meta's
// terrainAreas, our MapLibre's `scenic:zoom-by-area`), a contour tile reads it to that zoom, so the
// lines follow the fine terrain there; elsewhere to the DEM's maxzoom (z12).
//
// A contour tile of zoom z reads the DEM a zoom up (z − 1, at most its area's zoom), the part of it
// that the tile covers, and its eight neighbours' (one zoom for all nine, the tile's own), so lines
// meet across tiles; subsampled to at least 100 pixels a side, smoothed to the grid, its isolines
// at the tile's levels, a line's `level` the largest index whose interval divides its elevation.
import { HeightTile, type DemTile } from './height-tile';
import generateIsolines from './isolines';
import encodeVectorTile, { GeomType } from './vtpbf';

export type ContourMsg =
  /** The DEM (a URL with {z}, {x} and {y}), its maxzoom and its areas: anew, the cache emptied. */
  | { type: 'init'; url: string; maxzoom: number; areas?: { zoom: number; tiles: Record<string, number> } }
  /** A contour tile, its contour intervals [minor, major] (none: an empty tile). */
  | { type: 'tile'; id: number; z: number; x: number; y: number; levels: number[] }
  | { type: 'cancel'; id: number };
export interface ContourReply {
  id: number;
  data?: ArrayBuffer;
  error?: string;
}

/** DEM tiles decoded, kept (the least recently used go first). */
const CACHE = 200;
/** A contour tile's DEM subsampled to at least this many pixels a side (smoother lines overzoomed). */
const SUBSAMPLE_BELOW = 100;
const EXTENT = 4096;
const BUFFER = 1;

let url = '';
let maxzoom = 12;
let areaZoom = 0;
let areaMax = new Map<string, number>();
let cache = new Map<string, Promise<DemTile>>();
const inflight = new Set<number>();
const cancelled = new Set<number>();

/** The finest DEM zoom of tile z/x/y's area, else the maxzoom. */
function areaMaxZoom(z: number, x: number, y: number): number {
  if (!areaMax.size || z < areaZoom) return maxzoom;
  const d = z - areaZoom;
  return areaMax.get(`${Math.floor(x / 2 ** d)}/${Math.floor(y / 2 ** d)}`) ?? maxzoom;
}

/** A DEM tile, decoded (Terrarium; JPEG XL or PNG, decoded by the browser with no colour change). */
function demTile(z: number, x: number, y: number): Promise<DemTile> {
  const u = url.replace('{z}', `${z}`).replace('{x}', `${x}`).replace('{y}', `${y}`);
  const hit = cache.get(u);
  if (hit) {
    cache.delete(u);
    cache.set(u, hit);
    return hit;
  }
  const p = (async () => {
    const r = await fetch(u);
    if (!r.ok) throw new Error(`${r.status} for ${u}`);
    const img = await createImageBitmap(await r.blob(), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
    const c = new OffscreenCanvas(img.width, img.height);
    const g = c.getContext('2d', { willReadFrequently: true })!;
    g.drawImage(img, 0, 0);
    const rgba = g.getImageData(0, 0, img.width, img.height).data;
    img.close();
    const data = new Float32Array(c.width * c.height);
    for (let i = 0; i < data.length; i++) data[i] = rgba[4 * i] * 256 + rgba[4 * i + 1] + rgba[4 * i + 2] / 256 - 32768;
    return { width: c.width, height: c.height, data };
  })();
  p.catch(() => cache.delete(u));
  cache.set(u, p);
  while (cache.size > CACHE) cache.delete(cache.keys().next().value!);
  return p;
}

/** Tile z/x/y's part of the DEM tile of zoom `dz` holding it. */
async function demPart(z: number, x: number, y: number, dz: number): Promise<HeightTile> {
  const sub = z - dz;
  const div = 1 << sub;
  const t = await demTile(dz, Math.floor(x / div), Math.floor(y / div));
  return HeightTile.fromRawDem(t).split(sub, x % div, y % div);
}

async function contourTile(z: number, x: number, y: number, levels: number[], id: number): Promise<ArrayBuffer> {
  if (!levels.length) return new ArrayBuffer(0);
  const dz = Math.max(0, Math.min(z - 1, areaMaxZoom(z, x, y)));
  const n = 1 << z;
  const parts = await Promise.all(
    [-1, 0, 1].flatMap((oy) =>
      [-1, 0, 1].map((ox) => {
        const ty = y + oy;
        if (ty < 0 || ty >= n) return undefined;
        const p = demPart(z, (x + ox + n) % n, ty, dz);
        // (A neighbour that can't be read leaves its side open; the tile itself, an error.)
        return ox || oy ? p.catch(() => undefined) : p;
      }),
    ),
  );
  let t = HeightTile.combineNeighbors(parts);
  if (!t || cancelled.has(id)) return new ArrayBuffer(0);
  if (t.width >= SUBSAMPLE_BELOW) t = t.materialize(2);
  else while (t.width < SUBSAMPLE_BELOW) t = t.subsamplePixelCenters(2).materialize(2);
  t = t.averagePixelCentersToGrid().materialize(1);
  const lines = generateIsolines(levels[0], t, EXTENT, BUFFER);
  const out = encodeVectorTile({
    extent: EXTENT,
    layers: {
      contours: {
        features: Object.entries(lines).map(([e, geometry]) => {
          const ele = Number(e);
          return {
            type: GeomType.LINESTRING,
            geometry,
            properties: { ele, level: Math.max(...levels.map((l, i) => (ele % l === 0 ? i : 0))) },
          };
        }),
      },
    },
  });
  return out.slice().buffer;
}

self.onmessage = async (ev: MessageEvent<ContourMsg>) => {
  const m = ev.data;
  if (m.type === 'init') {
    url = m.url;
    maxzoom = m.maxzoom;
    areaZoom = m.areas?.zoom ?? 0;
    areaMax = new Map(Object.entries(m.areas?.tiles ?? {}));
    cache = new Map();
  } else if (m.type === 'cancel') {
    if (inflight.has(m.id)) cancelled.add(m.id);
  } else {
    inflight.add(m.id);
    let reply: ContourReply;
    try {
      const data = await contourTile(m.z, m.x, m.y, m.levels, m.id);
      reply = { id: m.id, data };
    } catch (e) {
      reply = { id: m.id, error: String(e) };
    }
    inflight.delete(m.id);
    cancelled.delete(m.id);
    (self as unknown as Worker).postMessage(reply, reply.data ? [reply.data] : []);
  }
};
