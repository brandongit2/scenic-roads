// Depth tiles made ready for MapLibre, off the main thread (depth.ts).
//
// A map tile z/x/y (512 px) is built from the server's depth and class tiles of zoom z + 1 (four
// 256-px tiles: the stored ones, or the server's from their nearest ancestor: crates/server/src/
// depth.rs) and the basemap's own water (its vector tile of zoom min(z, 14), as the map draws it),
// painted at 512 px. From that one composite come:
//   relief   depth, reaching REACH px past its data (?mask=1: on land too, the map drawing it
//            only in its water tiles' water; else in the basemap's water); elsewhere +12,000 m,
//            which the colour ramp leaves transparent
//   contour  a vector tile of depth contours (maplibre-contour's isolines, contour/, over the depth in
//            the water, 0 on land and water without data its nearest depth, of the tile and its
//            eight neighbours, so lines meet across tiles)
// (The floor's shading is the build's: depth.ts.)
// and, on request, the depth, class and source at a point (the readout), depth histograms of the
// tiles in view (the colour scale fitted to the view) and the sources in them (the attribution).
import { HeightTile } from './contour/height-tile';
import generateIsolines from './contour/isolines';
import { HBINS, hbin } from './depthlook';
import { encodeLines, readPolygons } from './mvt';

export type DepthMsg =
  | { type: 'init'; base: string; depth: string; cls: string }
  | { type: 'tile'; id: number; kind: 'relief' | 'contour'; z: number; x: number; y: number; q: Record<string, string> }
  | { type: 'probe'; id: number; z: number; x: number; y: number; px: number; py: number }
  | { type: 'hist'; id: number; tiles: [number, number, number][] }
  | { type: 'cancel'; id: number };
export interface Probe {
  /** Depth (m, negative below the surface; null: none). */
  d: number | null;
  cls: number;
  src: number;
  water: boolean;
  dries: boolean;
}
export interface DepthReply {
  id: number;
  data?: ArrayBuffer;
  probe?: Probe;
  hist?: number[];
  error?: string;
}

const N = 512;
const NODATA_MIN = 9000;
const SENTINEL = 12000;
/** The basemap's deepest tiles. */
const VMAX = 14;

let base = '';
let depthUrl = '';
let clsUrl = '';
const cancelled = new Set<number>();

self.onmessage = (ev: MessageEvent<DepthMsg>) => {
  const m = ev.data;
  if (m.type === 'init') {
    // (A new catalog: its URLs, and nothing kept from the old one.)
    if (m.depth !== depthUrl || m.base !== base) {
      fetched.clear();
      composites.clear();
      waterCache.clear();
    }
    base = m.base;
    depthUrl = m.depth;
    clsUrl = m.cls;
    return;
  }
  if (m.type === 'cancel') {
    cancelled.add(m.id);
    return;
  }
  void handle(m).catch((e) => post({ id: m.id, error: String(e) }));
};

const post = (r: DepthReply, transfer: Transferable[] = []) => (self as unknown as Worker).postMessage(r, transfer);

async function handle(m: Exclude<DepthMsg, { type: 'init' } | { type: 'cancel' }>) {
  if (m.type === 'tile') {
    if (cancelled.delete(m.id)) return;
    const data = m.kind === 'contour' ? await contourTile(m.z, m.x, m.y, m.q) : await reliefTile(m.z, m.x, m.y, m.q.mask === '1');
    post({ id: m.id, data }, [data]);
  } else if (m.type === 'probe') {
    const c = await composite(m.z, m.x, m.y);
    const i = Math.max(0, Math.min(N - 1, m.py)) * N + Math.max(0, Math.min(N - 1, m.px));
    const d = c.d[i];
    post({ id: m.id, probe: { d: d < NODATA_MIN ? d : null, cls: c.k[i] & 7, src: c.k[i] >> 4, water: c.w[i] === 1, dries: (c.k[i] & 8) !== 0 } });
  } else if (m.type === 'hist') {
    const h = new Array(HBINS).fill(0);
    for (const [z, x, y] of m.tiles) {
      const c = await composite(z, x, y);
      for (let b = 0; b < HBINS; b++) h[b] += c.h[b];
    }
    post({ id: m.id, hist: h });
  }
}

// ---- inputs: the server's tiles and the basemap's water --------------------------------------

type Grid = { d: Float32Array; k: Uint8Array } | null;
const fetched = new Map<string, Promise<Grid>>();
function remember<T>(map: Map<string, T>, key: string, v: T, cap: number) {
  map.set(key, v);
  if (map.size > cap) map.delete(map.keys().next().value!);
}

const canvas256 = new OffscreenCanvas(256, 256);
const ctx256 = canvas256.getContext('2d', { willReadFrequently: true })!;

const fill = (t: string, z: number, x: number, y: number) => t.replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y));

async function pixels(url: string): Promise<Uint8ClampedArray | null> {
  const r = await fetch(url);
  if (r.status === 204 || r.status === 404) return null;
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  const bmp = await createImageBitmap(await r.blob(), { premultiplyAlpha: 'none', colorSpaceConversion: 'none' });
  ctx256.clearRect(0, 0, 256, 256);
  ctx256.drawImage(bmp, 0, 0);
  bmp.close();
  return ctx256.getImageData(0, 0, 256, 256).data;
}

/** A 256-px server tile (depth and class), decoded; null where the layer has nothing. */
function serverTile(z: number, x: number, y: number): Promise<Grid> {
  const key = `${z}/${x}/${y}`;
  const hit = fetched.get(key);
  if (hit) return hit;
  const p = (async () => {
    const [dp, kp] = await Promise.all([pixels(fill(depthUrl, z, x, y)), pixels(fill(clsUrl, z, x, y))]);
    if (!dp) return null;
    const d = new Float32Array(256 * 256), k = new Uint8Array(256 * 256);
    for (let i = 0; i < 256 * 256; i++) {
      d[i] = dp[4 * i] * 256 + dp[4 * i + 1] + dp[4 * i + 2] / 256 - 32768;
      k[i] = kp ? kp[4 * i] : 0;
    }
    return { d, k };
  })();
  remember(fetched, key, p, 600);
  p.catch(() => fetched.get(key) === p && fetched.delete(key));
  return p;
}

const waterCache = new Map<string, Promise<number[][][]>>();
/** A basemap tile's water rings (lakes and rivers included; tunnels left out), each feature its
 * rings, in tile units scaled to 4096. */
function waterRings(z: number, x: number, y: number): Promise<number[][][]> {
  const key = `${z}/${x}/${y}`;
  const hit = waterCache.get(key);
  if (hit) return hit;
  const p = (async () => {
    const r = await fetch(fill(base, z, x, y));
    if (r.status === 204 || r.status === 404) return [];
    if (!r.ok) throw new Error(`basemap ${key}: HTTP ${r.status}`);
    const layer = readPolygons(await r.arrayBuffer(), 'water');
    if (!layer) return [];
    const k = 4096 / layer.extent;
    return layer.lines.filter((f) => f.props.brunnel !== 'tunnel').map((f) => (k === 1 ? f.runs : f.runs.map((run) => run.map((v) => v * k))));
  })();
  remember(waterCache, key, p, 300);
  p.catch(() => waterCache.get(key) === p && waterCache.delete(key));
  return p;
}

const maskCanvas = new OffscreenCanvas(N, N);
const maskCtx = maskCanvas.getContext('2d', { willReadFrequently: true })!;

/** The basemap's water over map tile z/x/y at N px: 1 water, 0 land. */
async function waterMask(z: number, x: number, y: number): Promise<Uint8Array> {
  const zv = Math.min(z, VMAX), dz = z - zv;
  const vx = x >> dz, vy = y >> dz;
  const scale = 2 ** dz, ox = (x - (vx << dz)) * N, oy = (y - (vy << dz)) * N;
  const rings = await waterRings(zv, vx, vy);
  maskCtx.setTransform(1, 0, 0, 1, 0, 0);
  maskCtx.clearRect(0, 0, N, N);
  maskCtx.fillStyle = '#fff';
  const k = (N * scale) / 4096;
  maskCtx.setTransform(k, 0, 0, k, -ox, -oy);
  for (const f of rings) {
    const path = new Path2D();
    for (const r of f) {
      path.moveTo(r[0], r[1]);
      for (let i = 2; i + 1 < r.length; i += 2) path.lineTo(r[i], r[i + 1]);
      path.closePath();
    }
    maskCtx.fill(path, 'evenodd');
  }
  const px = maskCtx.getImageData(0, 0, N, N).data;
  const w = new Uint8Array(N * N);
  for (let i = 0; i < N * N; i++) w[i] = px[4 * i + 3] >= 128 ? 1 : 0;
  return w;
}

// ---- the composite --------------------------------------------------------------------------

interface Composite {
  /** Depth (m; ≥ NODATA_MIN: none). */
  d: Float32Array;
  /** Class (bits 0–2), dries at low water (bit 3), source (bits 4–7), as the class tiles hold them. */
  k: Uint8Array;
  /** Basemap water. */
  w: Uint8Array;
  /** Depth histogram of the water with data (HBINS log bins). */
  h: number[];
  empty: boolean;
}
const composites = new Map<string, Promise<Composite>>();

function composite(z: number, x: number, y: number): Promise<Composite> {
  const key = `${z}/${x}/${y}`;
  const hit = composites.get(key);
  if (hit) return hit;
  const p = (async (): Promise<Composite> => {
    const z1 = z + 1;
    const [g00, g10, g01, g11, w] = await Promise.all([
      serverTile(z1, 2 * x, 2 * y), serverTile(z1, 2 * x + 1, 2 * y), serverTile(z1, 2 * x, 2 * y + 1), serverTile(z1, 2 * x + 1, 2 * y + 1), waterMask(z, x, y),
    ]);
    const d = new Float32Array(N * N).fill(SENTINEL), k = new Uint8Array(N * N);
    const h = new Array(HBINS).fill(0);
    let any = false;
    ([[g00, 0, 0], [g10, 256, 0], [g01, 0, 256], [g11, 256, 256]] as [Grid, number, number][]).forEach(([g, ox, oy]) => {
      if (!g) return;
      any = true;
      for (let r = 0; r < 256; r++) {
        d.set(g.d.subarray(r * 256, r * 256 + 256), (oy + r) * N + ox);
        k.set(g.k.subarray(r * 256, r * 256 + 256), (oy + r) * N + ox);
      }
    });
    for (let i = 0; i < N * N; i++) {
      if (!w[i] || !(d[i] < NODATA_MIN)) continue;
      if (d[i] < 0) h[hbin(-d[i])]++;
    }
    return { d, k, w, h, empty: !any };
  })();
  remember(composites, key, p, 72);
  p.catch(() => composites.get(key) === p && composites.delete(key));
  return p;
}

// ---- variants ---------------------------------------------------------------------------------

const outCanvas = new OffscreenCanvas(N, N);
const outCtx = outCanvas.getContext('2d')!;

function terrarium(v: Float32Array): ImageData {
  const img = outCtx.createImageData(N, N), o = img.data;
  for (let i = 0; i < N * N; i++) {
    const c = Math.max(0, Math.min(16777215, Math.round((v[i] + 32768) * 256)));
    o[4 * i] = c >> 16;
    o[4 * i + 1] = (c >> 8) & 255;
    o[4 * i + 2] = c & 255;
    o[4 * i + 3] = 255;
  }
  return img;
}

async function encode(img: ImageData): Promise<ArrayBuffer> {
  outCtx.putImageData(img, 0, 0);
  return (await outCanvas.convertToBlob({ type: 'image/png' })).arrayBuffer();
}

/** How far (px) a depth reaches past its data for the colour: across the shore's pixels with none
 * (a coarse tile's water rule, a lake under a pixel) to the water the map draws. */
const REACH = 8;

/** Each pixel's nearest pixel with data (two-pass chamfer propagation), its sources the water's
 * (`water`) or all with data (the server's land band too); -1: none in the tile. */
function nearest(c: Composite, water: boolean): { src: Int32Array; dist: Float32Array } {
  const src = new Int32Array(N * N).fill(-1);
  const dist = new Float32Array(N * N).fill(1e9);
  for (let i = 0; i < N * N; i++) {
    if (c.d[i] < NODATA_MIN && (!water || c.w[i])) {
      src[i] = i;
      dist[i] = 0;
    }
  }
  const relax = (i: number, j: number, cost: number) => {
    if (src[j] >= 0 && dist[j] + cost < dist[i]) {
      dist[i] = dist[j] + cost;
      src[i] = src[j];
    }
  };
  for (let y = 0; y < N; y++) for (let x = 0; x < N; x++) {
    const i = y * N + x;
    if (x > 0) relax(i, i - 1, 1);
    if (y > 0) {
      relax(i, i - N, 1);
      if (x > 0) relax(i, i - N - 1, 1.414);
      if (x < N - 1) relax(i, i - N + 1, 1.414);
    }
  }
  for (let y = N - 1; y >= 0; y--) for (let x = N - 1; x >= 0; x--) {
    const i = y * N + x;
    if (x < N - 1) relax(i, i + 1, 1);
    if (y < N - 1) {
      relax(i, i + N, 1);
      if (x < N - 1) relax(i, i + N + 1, 1.414);
      if (x > 0) relax(i, i + N - 1, 1.414);
    }
  }
  return { src, dist };
}

/** The contours' values: water with data as it is, land 0, water without data its nearest depth in
 * the water. */
function contourValues(c: Composite): Float32Array {
  const { src } = nearest(c, true);
  const v = new Float32Array(N * N);
  for (let i = 0; i < N * N; i++) v[i] = c.w[i] && src[i] >= 0 ? c.d[src[i]] : 0;
  return v;
}

/** Relief values: a pixel's depth (its nearest within REACH where it has none), +12,000 m beyond;
 * masked, on land too (the map draws it only in its water: depth.ts), else only in the basemap's
 * water. Water that dries at low water takes the ramp's drying colours (heights above 0): those
 * below mean sea level at its lowest; the depth itself stays below MSL everywhere else (readout,
 * shade). */
function reliefValues(c: Composite, masked: boolean): Float32Array {
  const { src, dist } = nearest(c, false);
  const v = new Float32Array(N * N).fill(SENTINEL);
  for (let i = 0; i < N * N; i++) {
    const j = src[i];
    if (j < 0 || dist[i] > REACH || (!masked && !c.w[i])) continue;
    v[i] = c.k[j] & 8 ? Math.max(c.d[j], 0.05) : c.d[j];
  }
  return v;
}

async function reliefTile(z: number, x: number, y: number, masked: boolean): Promise<ArrayBuffer> {
  return encode(terrarium(reliefValues(await composite(z, x, y), masked)));
}

// ---- contours ---------------------------------------------------------------------------------

/** Contour interval (m) at map zoom z: by zoom, or fixed. */
export function interval(z: number, mode: string, fixed: number): number {
  if (mode === 'fixed') return fixed;
  const table: [number, number][] = [[0, 1000], [6, 500], [7, 200], [8, 100], [9, 50], [10, 20], [11, 10], [12, 5], [14, 2], [16, 1]];
  let v = 1000;
  for (const [zz, iv] of table) if (z >= zz) v = iv;
  return v;
}

async function contourTile(z: number, x: number, y: number, q: Record<string, string>): Promise<ArrayBuffer> {
  const iv = interval(z, q.m ?? 'zoom', +(q.f ?? 10));
  const every = Math.max(1, +(q.k ?? 5));
  const n2 = 2 ** z;
  const tiles = await Promise.all(
    [-1, 0, 1].flatMap((dy) => [-1, 0, 1].map(async (dx) => {
      const ty = y + dy;
      if (ty < 0 || ty >= n2) return undefined;
      const c = await composite(z, (x + dx + n2) % n2, ty);
      if (c.empty) return undefined;
      return HeightTile.fromRawDem({ width: N, height: N, data: contourValues(c) });
    })),
  );
  if (!tiles[4]) return new ArrayBuffer(0);
  const t = HeightTile.combineNeighbors(tiles)!.averagePixelCentersToGrid().materialize(1);
  const lines = generateIsolines(iv, t, 4096, 1);
  const out: { runs: number[][]; props: Record<string, number> }[] = [];
  for (const [eleS, runs] of Object.entries(lines)) {
    const ele = Number(eleS);
    if (!(ele < 0)) continue;
    const depth = Math.round(-ele * 100) / 100;
    const idx = Math.abs(Math.round(depth / iv)) % every === 0 ? 1 : 0;
    // Marching squares draw pixel staircases: simplified, then rounded (Chaikin), the ends kept so
    // lines still meet across tiles. Labels need it (their angle limit) and lines look drawn.
    out.push({ runs: runs.map((r) => smooth(simplify(r, 5), 2)), props: { d: depth, idx } });
  }
  const b = encodeLines('c', out);
  return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer;
}

/** Douglas–Peucker over a run (x, y in turn), tolerance in tile units. */
function simplify(run: number[], tol: number): number[] {
  const n = run.length / 2;
  if (n <= 2) return run;
  const keep = new Uint8Array(n);
  keep[0] = keep[n - 1] = 1;
  const stack: [number, number][] = [[0, n - 1]];
  const t2 = tol * tol;
  while (stack.length) {
    const [a, b] = stack.pop()!;
    const ax = run[2 * a], ay = run[2 * a + 1], bx = run[2 * b], by = run[2 * b + 1];
    const dx = bx - ax, dy = by - ay, L = dx * dx + dy * dy;
    let best = -1, bd = t2;
    for (let i = a + 1; i < b; i++) {
      const px = run[2 * i] - ax, py = run[2 * i + 1] - ay;
      const t = L > 0 ? Math.max(0, Math.min(1, (px * dx + py * dy) / L)) : 0;
      const ex = px - t * dx, ey = py - t * dy, d = ex * ex + ey * ey;
      if (d > bd) {
        bd = d;
        best = i;
      }
    }
    if (best >= 0) {
      keep[best] = 1;
      stack.push([a, best], [best, b]);
    }
  }
  const out: number[] = [];
  for (let i = 0; i < n; i++) if (keep[i]) out.push(run[2 * i], run[2 * i + 1]);
  return out;
}

/** Chaikin's corner cutting, `iters` times; a closed run stays closed, an open one keeps its ends. */
function smooth(run: number[], iters: number): number[] {
  let r = run;
  for (let k = 0; k < iters; k++) {
    const n = r.length / 2;
    if (n < 3) return r;
    const closed = r[0] === r[2 * n - 2] && r[1] === r[2 * n - 1];
    const out: number[] = closed ? [] : [r[0], r[1]];
    for (let i = 0; i + 1 < n; i++) {
      const x0 = r[2 * i], y0 = r[2 * i + 1], x1 = r[2 * i + 2], y1 = r[2 * i + 3];
      out.push(0.75 * x0 + 0.25 * x1, 0.75 * y0 + 0.25 * y1, 0.25 * x0 + 0.75 * x1, 0.25 * y0 + 0.75 * y1);
    }
    if (closed) out.push(out[0], out[1]);
    else out.push(r[2 * n - 2], r[2 * n - 1]);
    r = out;
  }
  return r;
}
