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
//
// The DEM is the catalog's: a new catalog's (`dem`) changes the DEM tiles' URLs only where their
// packs changed (each URL carries its pack's version, as the map's terrain sources ask for it, so
// the two share the browser's cache); those cached under old URLs age out. A contour tile reads the
// DEM as it was when the tile was asked for (its `Dem`), so a new DEM mid-tile can't mix two
// catalogs' tiles in one; a cancelled tile's fetches that no other tile waits for are aborted.
import { HeightTile, type DemTile } from './height-tile';
import generateIsolines from './isolines';
import encodeVectorTile, { GeomType } from './vtpbf';
import { areaMaxZoom, packToken, type LayerPacks, type TerrainAreas } from '../packs';

export type ContourMsg =
  /** The DEM: `url` (with {z}, {x} and {y}; with its file's version, used where `packs` is none),
   * the terrain's packs, its maxzoom and its areas. */
  | ({ type: 'dem' } & Dem)
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

/** A catalog's DEM. */
export interface Dem {
  url: string;
  packs?: LayerPacks;
  maxzoom: number;
  areas?: TerrainAreas;
}
/** A DEM tile, being fetched or fetched, and the contour tiles waiting for it. */
interface Entry {
  p: Promise<DemTile>;
  done: boolean;
  waiting: Set<number>;
  abort: AbortController;
}

let dem: Dem = { url: '', maxzoom: 12 };
/** The DEM tiles by URL. */
const cache = new Map<string, Entry>();
/** The contour tiles being made: the DEM tiles each waits for. */
const jobs = new Map<number, Set<Entry>>();
const cancelled = new Set<number>();

/** DEM tile z/x/y's URL: with its pack's version where the packs are known (as api.ts packUrl). */
function demUrl(d: Dem, z: number, x: number, y: number): string {
  let u = d.url;
  if (d.packs) {
    const v = packToken(d.packs, z, x, y);
    u = u.replace(/\?.*$/, '') + (v ? `?v=${v}` : '');
  }
  return u.replace('{z}', `${z}`).replace('{x}', `${x}`).replace('{y}', `${y}`);
}

/** A DEM tile for contour tile `id`, decoded (Terrarium; JPEG XL or PNG, decoded by the browser
 * with no colour change). */
function demTile(d: Dem, id: number, z: number, x: number, y: number): Promise<DemTile> {
  const u = demUrl(d, z, x, y);
  const job = jobs.get(id);
  const hit = cache.get(u);
  if (hit) {
    cache.delete(u);
    cache.set(u, hit);
    hit.waiting.add(id);
    job?.add(hit);
    return hit.p;
  }
  const abort = new AbortController();
  const p = (async () => {
    const r = await fetch(u, { signal: abort.signal });
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
  const e: Entry = { p, done: false, waiting: new Set([id]), abort };
  p.then(
    () => (e.done = true),
    () => {
      e.done = true;
      if (cache.get(u) === e) cache.delete(u);
    },
  );
  job?.add(e);
  cache.set(u, e);
  while (cache.size > CACHE) cache.delete(cache.keys().next().value!);
  return p;
}

/** Tile z/x/y's part of the DEM tile of zoom `dz` holding it. */
async function demPart(d: Dem, id: number, z: number, x: number, y: number, dz: number): Promise<HeightTile> {
  const sub = z - dz;
  const div = 1 << sub;
  const t = await demTile(d, id, dz, Math.floor(x / div), Math.floor(y / div));
  return HeightTile.fromRawDem(t).split(sub, x % div, y % div);
}

async function contourTile(d: Dem, z: number, x: number, y: number, levels: number[], id: number): Promise<ArrayBuffer> {
  if (!levels.length) return new ArrayBuffer(0);
  const dz = Math.max(0, Math.min(z - 1, areaMaxZoom(d.areas, d.maxzoom, z, x, y)));
  const n = 1 << z;
  const parts = await Promise.all(
    [-1, 0, 1].flatMap((oy) =>
      [-1, 0, 1].map((ox) => {
        const ty = y + oy;
        if (ty < 0 || ty >= n) return undefined;
        const p = demPart(d, id, z, (x + ox + n) % n, ty, dz);
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

/** Contour tile `id` waits no more: cancelled, the DEM tiles still being fetched for it alone are
 * aborted. */
function release(id: number, cancel: boolean) {
  for (const e of jobs.get(id) ?? []) {
    e.waiting.delete(id);
    if (cancel && !e.done && !e.waiting.size) {
      e.abort.abort();
      for (const [u, c] of cache) if (c === e) cache.delete(u);
    }
  }
  jobs.delete(id);
}

self.onmessage = async (ev: MessageEvent<ContourMsg>) => {
  const m = ev.data;
  if (m.type === 'dem') {
    dem = { url: m.url, packs: m.packs, maxzoom: m.maxzoom, areas: m.areas };
  } else if (m.type === 'cancel') {
    if (jobs.has(m.id)) {
      cancelled.add(m.id);
      release(m.id, true);
    }
  } else {
    jobs.set(m.id, new Set());
    let reply: ContourReply;
    try {
      reply = { id: m.id, data: await contourTile(dem, m.z, m.x, m.y, m.levels, m.id) };
    } catch (e) {
      reply = { id: m.id, error: String(e) };
    }
    release(m.id, false);
    cancelled.delete(m.id);
    (self as unknown as Worker).postMessage(reply, reply.data ? [reply.data] : []);
  }
};
