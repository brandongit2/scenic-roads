// Distance from the shore, off the main thread, for the coastal shading (coast.ts): per map tile,
// a raster-DEM tile (Mapbox Terrain-RGB encoding) whose "elevation" is the signed distance to the
// coast in metres: positive over water, negative over land (only a few pixels' worth: enough for
// the shoreline to fall between pixels where the colour ramp crosses zero).
//
// The water is the basemap's (base.pmtiles and the parts' archives, read here with pmtiles): the
// tile's polygons and its eight neighbours', painted into a canvas MARGIN pixels wider than the
// tile on every side (a coast just across a tile edge still counts), then an exact Euclidean
// distance transform (Felzenszwalb & Huttenlocher) each way. Metres per pixel follow each row's
// latitude, so neighbouring tiles agree along their edges.
import { PMTiles } from 'pmtiles';
import { readPolygons } from './mvt';

export type CoastMessage =
  | { type: 'init'; archives: string[] }
  | { type: 'tile'; id: number; z: number; x: number; y: number; lakes: boolean }
  | { type: 'cancel'; id: number };
export interface CoastResponse {
  id: number;
  data?: ArrayBuffer;
  error?: string;
}

const SIZE = 512;
/** Pixels of the neighbouring tiles taken in around the tile: the farthest distance measured. */
const MARGIN = 192;
const N = SIZE + 2 * MARGIN;
/** Land pixels: their distance to the water, at most this many pixels. */
const LAND_PX = 2;
/** The vector tiles' zoom at most (the basemap archives'). */
const VECTOR_MAXZOOM = 14;
const EARTH = 40075016.686;

let archives: { pm: PMTiles; header: Promise<{ minZoom: number; maxZoom: number; minLon: number; minLat: number; maxLon: number; maxLat: number } | null> }[] = [];
const cancelled = new Set<number>();
let queue = Promise.resolve();

self.onmessage = (ev: MessageEvent<CoastMessage>) => {
  const m = ev.data;
  if (m.type === 'init') {
    archives = m.archives.map((url) => {
      const pm = new PMTiles(url);
      return { pm, header: pm.getHeader().catch(() => null) };
    });
  } else if (m.type === 'cancel') {
    cancelled.add(m.id);
  } else {
    // One tile at a time (each is a burst of CPU; MapLibre asks for many at once).
    queue = queue.then(async () => {
      if (cancelled.delete(m.id)) return;
      try {
        const data = await coastTile(m.z, m.x, m.y, m.lakes);
        (self as unknown as Worker).postMessage({ id: m.id, data } satisfies CoastResponse, [data]);
      } catch (e) {
        (self as unknown as Worker).postMessage({ id: m.id, error: String(e) } satisfies CoastResponse);
      }
    });
  }
};

// ---- the basemap's water -------------------------------------------------------------------

type Water = { extent: number; rings: number[][][] }; // per feature: its rings
const waterCache = new Map<string, Promise<Water>>();

/** A vector tile's water polygons, from every archive covering it (the tunnels' left out; the sea
 * only, unless lakes). */
function water(z: number, x: number, y: number, lakes: boolean): Promise<Water> {
  const key = `${z}/${x}/${y}/${lakes ? 1 : 0}`;
  let p = waterCache.get(key);
  if (!p) {
    p = loadWater(z, x, y, lakes);
    waterCache.set(key, p);
    if (waterCache.size > 400) waterCache.delete(waterCache.keys().next().value!);
  }
  return p;
}

async function loadWater(z: number, x: number, y: number, lakes: boolean): Promise<Water> {
  const n = 2 ** z;
  const lon0 = (x / n) * 360 - 180, lon1 = ((x + 1) / n) * 360 - 180;
  const lat = (t: number) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * t) / n))) * 180) / Math.PI;
  const lat0 = lat(y + 1), lat1 = lat(y);
  const out: Water = { extent: 4096, rings: [] };
  await Promise.all(archives.map(async (a) => {
    const h = await a.header;
    if (!h || z < h.minZoom || z > h.maxZoom || lon1 < h.minLon || lon0 > h.maxLon || lat1 < h.minLat || lat0 > h.maxLat) return;
    const r = await a.pm.getZxy(z, x, y).catch(() => undefined);
    if (!r?.data) return;
    const layer = readPolygons(r.data, 'water');
    if (!layer) return;
    // (The archives share one extent in practice; rings are scaled to 4096 if not.)
    const k = 4096 / layer.extent;
    for (const f of layer.lines) {
      if (f.props.brunnel === 'tunnel' || (!lakes && f.props.class !== 'ocean')) continue;
      out.rings.push(k === 1 ? f.runs : f.runs.map((run) => run.map((v) => v * k)));
    }
  }));
  return out;
}

// ---- the tile --------------------------------------------------------------------------------

const canvas = new OffscreenCanvas(N, N);
const ctx = canvas.getContext('2d', { willReadFrequently: true })!;
const outCanvas = new OffscreenCanvas(SIZE, SIZE);
const outCtx = outCanvas.getContext('2d')!;

async function coastTile(z: number, x: number, y: number, lakes: boolean): Promise<ArrayBuffer> {
  // Past the archives' zoom: the deepest tile's water, cut to this one.
  const zv = Math.min(z, VECTOR_MAXZOOM), dz = z - zv;
  const n = 2 ** zv;
  const vx = x >> dz, vy = y >> dz;
  // This tile's place within the vector tile (pixels at its own scale).
  const scale = 2 ** dz, ox = (x - (vx << dz)) * SIZE, oy = (y - (vy << dz)) * SIZE;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, N, N);
  ctx.fillStyle = '#fff';
  const jobs: Promise<void>[] = [];
  for (let dy = -1; dy <= 1; dy++) {
    for (let dx = -1; dx <= 1; dx++) {
      const ty = vy + dy;
      if (ty < 0 || ty >= n) continue;
      const tx = (((vx + dx) % n) + n) % n;
      jobs.push(water(zv, tx, ty, lakes).then((w) => {
        const k = (SIZE * scale) / w.extent;
        ctx.setTransform(k, 0, 0, k, MARGIN - ox + dx * SIZE * scale, MARGIN - oy + dy * SIZE * scale);
        for (const rings of w.rings) {
          const path = new Path2D();
          for (const r of rings) {
            path.moveTo(r[0], r[1]);
            for (let i = 2; i + 1 < r.length; i += 2) path.lineTo(r[i], r[i + 1]);
            path.closePath();
          }
          ctx.fill(path, 'evenodd');
        }
      }));
    }
  }
  // (Painted as each arrives: fills are opaque, so the order doesn't matter.)
  await Promise.all(jobs);
  const px = ctx.getImageData(0, 0, N, N).data;
  const wet = new Uint8Array(N * N);
  let nWet = 0;
  for (let i = 0; i < N * N; i++) if (px[i * 4 + 3] >= 128) (wet[i] = 1), nWet++;

  // Signed distance in pixels: water + (to the nearest land), land − (to the nearest water).
  const sd = new Float32Array(SIZE * SIZE);
  if (nWet === 0) sd.fill(-LAND_PX);
  else if (nWet === N * N) sd.fill(MARGIN);
  else {
    const toLand = edt(wet, 0), toWater = edt(wet, 1);
    for (let r = 0; r < SIZE; r++) {
      for (let c = 0; c < SIZE; c++) {
        const i = (r + MARGIN) * N + c + MARGIN;
        sd[r * SIZE + c] = wet[i] ? Math.min(MARGIN, Math.sqrt(toLand[i]) - 0.5) : -Math.min(LAND_PX, Math.sqrt(toWater[i]) - 0.5);
      }
    }
  }

  // Terrain-RGB: metres = -10000 + (R·65536 + G·256 + B) / 10. Each row's metres per pixel.
  const img = outCtx.createImageData(SIZE, SIZE), o = img.data;
  const world = SIZE * 2 ** z;
  for (let r = 0; r < SIZE; r++) {
    const latR = Math.atan(Math.sinh(Math.PI * (1 - (2 * (y * SIZE + r + 0.5)) / world)));
    const mpp = (EARTH * Math.cos(latR)) / world;
    for (let c = 0; c < SIZE; c++) {
      const v = sd[r * SIZE + c] * mpp;
      const code = Math.max(0, Math.min(16777215, Math.round((v + 10000) * 10)));
      const j = (r * SIZE + c) * 4;
      o[j] = code >> 16;
      o[j + 1] = (code >> 8) & 255;
      o[j + 2] = code & 255;
      o[j + 3] = 255;
    }
  }
  outCtx.putImageData(img, 0, 0);
  const blob = await outCanvas.convertToBlob({ type: 'image/png' });
  return blob.arrayBuffer();
}

/** Squared Euclidean distance from each pixel to the nearest pixel whose `grid` value is `target`
 * (N × N), Felzenszwalb & Huttenlocher: columns, then rows. */
function edt(grid: Uint8Array, target: number): Float32Array {
  const d = new Float32Array(N * N);
  for (let i = 0; i < N * N; i++) d[i] = grid[i] === target ? 0 : INF;
  for (let c = 0; c < N; c++) edt1d(d, c, N);
  for (let r = 0; r < N; r++) edt1d(d, r * N, 1);
  return d;
}

const INF = 1e20;
const f = new Float64Array(N), v = new Int32Array(N), zz = new Float64Array(N + 1);
/** The 1-D transform of N values of `d` from `off`, `stride` apart, in place. */
function edt1d(d: Float32Array, off: number, stride: number) {
  let any = false;
  for (let q = 0; q < N; q++) {
    f[q] = d[off + q * stride];
    if (f[q] < INF) any = true;
  }
  if (!any) return;
  let k = 0;
  v[0] = 0;
  zz[0] = -INF;
  zz[1] = INF;
  for (let q = 1; q < N; q++) {
    let s = (f[q] + q * q - (f[v[k]] + v[k] * v[k])) / (2 * q - 2 * v[k]);
    while (s <= zz[k]) {
      k--;
      s = (f[q] + q * q - (f[v[k]] + v[k] * v[k])) / (2 * q - 2 * v[k]);
    }
    k++;
    v[k] = q;
    zz[k] = s;
    zz[k + 1] = INF;
  }
  k = 0;
  for (let q = 0; q < N; q++) {
    while (zz[k + 1] < q) k++;
    const r = q - v[k];
    d[off + q * stride] = r * r + f[v[k]];
  }
}
