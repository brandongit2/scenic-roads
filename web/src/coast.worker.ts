// Distance from the shore, off the main thread, for the coastal shading (coast.ts): per map tile,
// a raster-DEM tile (coastdist.ts's encoding) whose "elevation" is the signed distance to the coast
// in metres: positive over water, negative over land. Measured exactly within the window (MARGIN
// pixels); beyond it, land is "deep" (the ramp needs less than a pixel of it) and water comes from
// a coarser level where the window holds too few metres (toward the poles: farLevels), else "far".
//
// From the water's shares (the water layer's tiles, the tile and its eight neighbours): every pixel
// holding any land is a shore, placed within it by its share (coastdist.ts), so zoomed out an
// island smaller than a pixel keeps its shore and its glow, as full detail would.
//
// Without the water layer, the water is the basemap's (its vector tiles, as the map's basemap
// source reads them): the tile's polygons and its eight neighbours', painted into a canvas MARGIN
// pixels wider than the tile on every side (a coast just across a tile edge still counts), then an
// exact Euclidean distance transform (Felzenszwalb & Huttenlocher) each way. Either way, metres per
// pixel follow each row's latitude, so neighbouring tiles agree along their edges, and tiles of
// different zooms (the globe takes coarser ones toward the poles) agree in metres.
import { readPolygons } from './mvt';
import { coastCode, farLevels, signedDistance, withFarField } from './coastdist';

export type CoastMessage =
  /** An input by name (`key`; coast.ts): `tiles`, the basemap's tile URL ({z}, {x}, {y});
   * `maxzoom`, its deepest tiles; `cov`, the water's shares instead; `margin`, how far the distance
   * is measured exactly (pixels of the tile); `far`, at least this many levels coarser beyond it
   * (else only toward the poles: farLevels). Sent again when they change. */
  | { type: 'init'; key: string; tiles: string; maxzoom: number; cov?: string; margin?: number; far?: number }
  | { type: 'tile'; id: number; key: string; z: number; x: number; y: number; lakes: boolean }
  | { type: 'cancel'; id: number };
export interface CoastResponse {
  id: number;
  data?: ArrayBuffer;
  error?: string;
}

const SIZE = 512;
/** Pixels of the neighbouring tiles taken in around the tile by default: the farthest distance
 * measured exactly. */
const MARGIN = 192;
const EARTH = 40075016.686;

/** An input (init): the basemap's tile URL and its deepest zoom, or instead the water's shares
 * (PNG, 512 px, red the sea's share, green the inland water's: the server's
 * `/tiles/water/…?raw=1`, or the shoreline check's reference). */
type Input = { tiles: string; maxzoom: number; cov: string; margin: number; far: number };
const inputs = new Map<string, Input>();
const cancelled = new Set<number>();
let queue = Promise.resolve();

self.onmessage = (ev: MessageEvent<CoastMessage>) => {
  const m = ev.data;
  if (m.type === 'init') {
    // (Again for new tiles: their water is read anew.)
    inputs.set(m.key, { tiles: m.tiles, maxzoom: m.maxzoom, cov: m.cov ?? '', margin: Math.min(SIZE, m.margin ?? MARGIN), far: m.far ?? 0 });
    waterCache.clear();
    covCache.clear();
    coarseCache.clear();
  } else if (m.type === 'cancel') {
    cancelled.add(m.id);
  } else {
    // One tile at a time (each is a burst of CPU; MapLibre asks for many at once).
    queue = queue.then(async () => {
      if (cancelled.delete(m.id)) return;
      try {
        const input = inputs.get(m.key);
        if (!input) throw new Error(`coast: no input ${m.key}`);
        const data = await coastTile(input, m.z, m.x, m.y, m.lakes);
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

/** A vector tile's water polygons (the tunnels' left out; the sea only, unless lakes). A tile that
 * failed to load isn't kept: asked for again, it loads again. */
function water(tiles: string, z: number, x: number, y: number, lakes: boolean): Promise<Water> {
  const key = `${tiles}|${z}/${x}/${y}/${lakes ? 1 : 0}`;
  const hit = waterCache.get(key);
  if (hit) return hit;
  const p = loadWater(tiles, z, x, y, lakes);
  waterCache.set(key, p);
  p.catch(() => waterCache.get(key) === p && waterCache.delete(key));
  if (waterCache.size > 400) waterCache.delete(waterCache.keys().next().value!);
  return p;
}

async function loadWater(tiles: string, z: number, x: number, y: number, lakes: boolean): Promise<Water> {
  const out: Water = { extent: 4096, rings: [] };
  // The tile as the map's basemap source gets it (the browser undoes its gzip); no content: no
  // basemap there.
  const r = await fetch(tiles.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y)));
  if (r.status === 204 || r.status === 404) return out;
  if (!r.ok) throw new Error(`basemap tile ${z}/${x}/${y}: HTTP ${r.status}`);
  const layer = readPolygons(await r.arrayBuffer(), 'water');
  if (!layer) return out;
  // (Rings are scaled to 4096 when the tile's extent differs.)
  const k = 4096 / layer.extent;
  for (const f of layer.lines) {
    if (f.props.brunnel === 'tunnel' || (!lakes && f.props.class !== 'ocean')) continue;
    out.rings.push(k === 1 ? f.runs : f.runs.map((run) => run.map((v) => v * k)));
  }
  return out;
}

// ---- the water's shares ------------------------------------------------------------------------

/** Share tiles read (RGBA), the most recent kept: a tile's eight neighbours are its neighbours'
 * too. */
const covCache = new Map<string, Promise<Uint8ClampedArray | null>>();
const COV_KEPT = 48;

function shares(url: string): Promise<Uint8ClampedArray | null> {
  const hit = covCache.get(url);
  if (hit) {
    covCache.delete(url);
    covCache.set(url, hit);
    return hit;
  }
  const p = (async () => {
    const r = await fetch(url);
    if (!r.ok) return null;
    const img = await createImageBitmap(await r.blob());
    const c = new OffscreenCanvas(SIZE, SIZE), g = c.getContext('2d', { willReadFrequently: true })!;
    g.drawImage(img, 0, 0);
    return g.getImageData(0, 0, SIZE, SIZE).data;
  })();
  covCache.set(url, p);
  p.then((d) => d || covCache.delete(url), () => covCache.delete(url));
  if (covCache.size > COV_KEPT) covCache.delete(covCache.keys().next().value!);
  return p;
}

/** The 3 × 3 share tiles around z/x/y: each pixel's land share (for the sea's shore alone, all but
 * the sea), n × n with the margin. Past the poles (no tile north of the top row, south of the
 * bottom one) the edge row goes on unchanged: Mercator stops at 85.05°, and taken as water it put a
 * shore along that whole circle, a glow ringing each pole that matched no coast (the owner saw it
 * around Antarctica, 9 Oct). A neighbour that failed counts as water with no land: its shore, just
 * past the tile's edge, is missed rather than the whole tile failing. */
async function landShares(input: Input, z: number, x: number, y: number, lakes: boolean): Promise<Float32Array> {
  const M = input.margin, n = SIZE + 2 * M;
  const frac = new Float32Array(n * n);
  const tiles = 2 ** z;
  const jobs: Promise<void>[] = [];
  for (let dy = -1; dy <= 1; dy++) {
    for (let dx = -1; dx <= 1; dx++) {
      const ty = y + dy;
      if (ty < 0 || ty >= tiles) continue;
      const tx = (((x + dx) % tiles) + tiles) % tiles;
      jobs.push((async () => {
        const d = await shares(input.cov.replace('{z}', String(z)).replace('{x}', String(tx)).replace('{y}', String(ty)));
        if (!d) {
          if (dx === 0 && dy === 0) throw new Error(`coverage tile ${z}/${tx}/${ty}`);
          return;
        }
        // The part of this tile within the margin.
        const ox = M + dx * SIZE, oy = M + dy * SIZE;
        const c0 = Math.max(0, -ox), c1 = Math.min(SIZE, n - ox), r0 = Math.max(0, -oy), r1 = Math.min(SIZE, n - oy);
        for (let r = r0; r < r1; r++) {
          for (let c = c0; c < c1; c++) {
            const j = (r * SIZE + c) * 4;
            const w = d[j] + (lakes ? d[j + 1] : 0);
            frac[(r + oy) * n + c + ox] = w >= 255 ? 0 : 1 - w / 255;
          }
        }
      })());
    }
  }
  const failed = (await Promise.allSettled(jobs)).find((r): r is PromiseRejectedResult => r.status === 'rejected');
  if (failed) throw failed.reason;
  pastThePoles(frac, n, M, y === 0, y === tiles - 1);
  return frac;
}

/** Rows of an n × n grid (margin M) past the poles: the tile's own edge row, repeated (`top`: the
 * tile is the top row of tiles; `bottom`: the bottom). */
export function pastThePoles<T extends Float32Array | Uint8ClampedArray>(g: T, n: number, M: number, top: boolean, bottom: boolean, stride = 1) {
  const row = n * stride;
  if (top) for (let r = 0; r < M; r++) g.copyWithin(r * row, M * row, (M + 1) * row);
  if (bottom) for (let r = n - M; r < n; r++) g.copyWithin(r * row, (n - M - 1) * row, (n - M) * row);
}

// ---- the tile --------------------------------------------------------------------------------

/** Canvases n × n (by n: the margin can differ between inputs). */
const canvases = new Map<number, OffscreenCanvasRenderingContext2D>();
function canvasOf(n: number) {
  let c = canvases.get(n);
  if (!c) canvases.set(n, (c = new OffscreenCanvas(n, n).getContext('2d', { willReadFrequently: true })!));
  return c;
}
const outCanvas = new OffscreenCanvas(SIZE, SIZE);
const outCtx = outCanvas.getContext('2d')!;

async function coastTile(input: Input, z: number, x: number, y: number, lakes: boolean): Promise<ArrayBuffer> {
  const M = input.margin;
  const sd = await distances(input, z, x, y, lakes);
  // Toward the poles, the water beyond the window from a coarser level (none within 60°). Should
  // that level fail to load, the water beyond is far: the tile itself is still good.
  const k = Math.min(z, Math.max(input.far, farLevels(z, y)));
  if (k > 0 && sd.some((d) => d >= 0.75 * M)) {
    const coarse = await coarseDistances(input, z - k, x >> k, y >> k, lakes).catch(() => null);
    const part = SIZE / 2 ** k;
    if (coarse) withFarField(sd, SIZE, M, coarse, k, (x % 2 ** k) * part, (y % 2 ** k) * part);
  }
  return encode(sd, z, y);
}

/** Coarse levels' distances, the most recent kept: a coarse tile serves all 4^k of its
 * descendants in its level's band of latitude, and the tiles in view are mostly siblings. 1 MB
 * each. */
const coarseCache = new Map<string, Promise<Float32Array>>();
const COARSE_KEPT = 12;

function coarseDistances(input: Input, z: number, x: number, y: number, lakes: boolean): Promise<Float32Array> {
  const key = `${input.cov}|${input.tiles}|${input.margin}|${z}/${x}/${y}/${lakes ? 1 : 0}`;
  const hit = coarseCache.get(key);
  if (hit) {
    coarseCache.delete(key);
    coarseCache.set(key, hit);
    return hit;
  }
  const p = distances(input, z, x, y, lakes);
  coarseCache.set(key, p);
  p.catch(() => coarseCache.get(key) === p && coarseCache.delete(key));
  if (coarseCache.size > COARSE_KEPT) coarseCache.delete(coarseCache.keys().next().value!);
  return p;
}

/** The tile's signed distance in pixels (coastdist.ts signedDistance: ±Infinity beyond the
 * window). */
async function distances(input: Input, z: number, x: number, y: number, lakes: boolean): Promise<Float32Array> {
  const M = input.margin, n = SIZE + 2 * M;
  if (input.cov) return signedDistance(await landShares(input, z, x, y, lakes), n, M);
  const ctx = canvasOf(n);
  // Past the basemap's zoom: the deepest tile's water, cut to this one.
  const zv = Math.min(z, input.maxzoom), dz = z - zv;
  const tiles = 2 ** zv;
  const vx = x >> dz, vy = y >> dz;
  // This tile's place within the vector tile (pixels at its own scale).
  const scale = 2 ** dz, ox = (x - (vx << dz)) * SIZE, oy = (y - (vy << dz)) * SIZE;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, n, n);
  ctx.fillStyle = '#fff';
  const jobs: Promise<void>[] = [];
  for (let dy = -1; dy <= 1; dy++) {
    for (let dx = -1; dx <= 1; dx++) {
      const ty = vy + dy;
      if (ty < 0 || ty >= tiles) continue;
      const tx = (((vx + dx) % tiles) + tiles) % tiles;
      jobs.push(water(input.tiles, zv, tx, ty, lakes).then((w) => {
        const k = (SIZE * scale) / w.extent;
        ctx.setTransform(k, 0, 0, k, M - ox + dx * SIZE * scale, M - oy + dy * SIZE * scale);
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
  // (Painted as each arrives: fills are opaque, so the order doesn't matter.) All of them painted
  // before the canvas is read, or the next tile clears it; a neighbour that didn't load fails the
  // tile (its coast would be drawn where its water is missing).
  const failed = (await Promise.allSettled(jobs)).find((r): r is PromiseRejectedResult => r.status === 'rejected');
  if (failed) throw failed.reason;
  // (Past the poles, the edge row goes on, as with the shares: nothing painted there read as land.)
  const top = y === 0, bottom = y === 2 ** z - 1;
  if (top || bottom) {
    const img = ctx.getImageData(0, 0, n, n);
    pastThePoles(img.data, n, M, top, bottom, 4);
    ctx.putImageData(img, 0, 0);
  }
  return thresholded(ctx, M);
}

/** The canvas's water (opaque pixels) as the tile's signed distance in pixels: water + (to the
 * nearest land), land − (to the nearest water), ±Infinity beyond the window. */
function thresholded(ctx: OffscreenCanvasRenderingContext2D, M: number): Float32Array {
  const N = SIZE + 2 * M;
  const px = ctx.getImageData(0, 0, N, N).data;
  const wet = new Uint8Array(N * N);
  let nWet = 0;
  for (let i = 0; i < N * N; i++) if (px[i * 4 + 3] >= 128) (wet[i] = 1), nWet++;
  const sd = new Float32Array(SIZE * SIZE);
  if (nWet === 0) sd.fill(-Infinity);
  else if (nWet === N * N) sd.fill(Infinity);
  else {
    const toLand = edt(wet, 0, N), toWater = edt(wet, 1, N);
    for (let r = 0; r < SIZE; r++) {
      for (let c = 0; c < SIZE; c++) {
        const i = (r + M) * N + c + M;
        const d = wet[i] ? Math.sqrt(toLand[i]) - 0.5 : -(Math.sqrt(toWater[i]) - 0.5);
        sd[r * SIZE + c] = Math.abs(d) > M ? d * Infinity : d;
      }
    }
  }
  return sd;
}

/** A signed distance (pixels) as the tile's raster-DEM, in metres (coastdist.ts coastCode). */
async function encode(sd: Float32Array, z: number, y: number): Promise<ArrayBuffer> {
  // Each row's metres per pixel.
  const img = outCtx.createImageData(SIZE, SIZE), o = img.data;
  const world = SIZE * 2 ** z;
  for (let r = 0; r < SIZE; r++) {
    const latR = Math.atan(Math.sinh(Math.PI * (1 - (2 * (y * SIZE + r + 0.5)) / world)));
    const mpp = (EARTH * Math.cos(latR)) / world;
    for (let c = 0; c < SIZE; c++) {
      const code = coastCode(sd[r * SIZE + c] * mpp);
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
function edt(grid: Uint8Array, target: number, N: number): Float32Array {
  const d = new Float32Array(N * N);
  for (let i = 0; i < N * N; i++) d[i] = grid[i] === target ? 0 : INF;
  const t = { f: new Float64Array(N), v: new Int32Array(N), zz: new Float64Array(N + 1) };
  for (let c = 0; c < N; c++) edt1d(d, c, N, N, t);
  for (let r = 0; r < N; r++) edt1d(d, r * N, 1, N, t);
  return d;
}

const INF = 1e20;
/** The 1-D transform of N values of `d` from `off`, `stride` apart, in place. */
function edt1d(d: Float32Array, off: number, stride: number, N: number, t: { f: Float64Array; v: Int32Array; zz: Float64Array }) {
  const { f, v, zz } = t;
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
