// The landmark dots' GPU layout (dots.ts), built in the landmarks worker from a source's points.
//
// Draw order: the points grouped by zoom-4 tile ("chunk"; chunks in Morton order, so the chunks in
// view make few runs), in index (fame) order within one, the best known drawn last, on top.
// Height order: the same points in Morton order at zoom 16, so the points of any map tile down to
// zoom 16 are one run, for the pass that reads their ground heights from the terrain (dots.ts).
//
// Zoomed out, most dots are specks (below the prominence range: all alike), piled up by the
// thousand where landmarks are dense, and a GPU pays for every point it rasterises there, however
// small. So specks are drawn once per cell of about two device pixels, by the least known visible
// dot of the cell (the one least likely to be prominent itself), with the opacity of all the
// cell's visible dots over one another. The cells are the Morton order's tiles at zooms 9–16, one
// run each; the flags say which dots stand for their cell at each zoom and for how many
// (visWords, for the filters of the moment).

export const CHUNK_Z = 4;
/** Per point, in draw order: position in its chunk (2 × f32, 8192 units a side), fame and interest
 * isolation (2 × f32), its texel in the heights texture (u32: its place in height order), chunk x
 * and y and class (3 × u8), padding. */
export const DRAW_STRIDE = 24;
/** Per point, in height order: position in its chunk (2 × f32), chunk x and y (2 × u8), padding. */
export const HPOS_STRIDE = 12;
/** Width of the heights texture (texels); a point's texel is (k mod W, k / W). */
export const HEIGHT_W = 2048;
/** Zoom of the height order's Morton codes. */
export const MORTON_Z = 16;
/** Zooms of the speck cells: LOD_Z0 … LOD_Z0 + LOD_LEVELS - 1. */
export const LOD_Z0 = 9;
export const LOD_LEVELS = 8;
/** Specks are drawn once per cell of at most this many device pixels. */
export const LOD_PX = 2;
/** The speck cells' zoom at the view centre (dots.ts u_lodZ); none from LOD_Z0 + LOD_LEVELS. */
export const lodZoom = (zoom: number, dpr: number) => zoom + Math.log2((512 * dpr) / LOD_PX);
/** Per point, in draw order, the filter flags (visWords): 3 words. */
export const VIS_WORDS = 3;

/** A source's points laid out for drawing (transferred from the worker). */
export interface DotData {
  n: number;
  /** DRAW_STRIDE bytes per point, draw order. */
  draw: ArrayBuffer;
  /** HPOS_STRIDE bytes per point, height order. */
  hpos: ArrayBuffer;
  /** Zoom-16 Morton code of each point, height order (ascending). */
  morton: Uint32Array;
  /** The chunks in draw order, 4 numbers each: x, y, first point, count. */
  chunks: Uint32Array;
}

/** The bits of v (16 of them) spread to the even bits. */
function spread(v: number): number {
  v &= 0xffff;
  v = (v | (v << 8)) & 0x00ff00ff;
  v = (v | (v << 4)) & 0x0f0f0f0f;
  v = (v | (v << 2)) & 0x33333333;
  v = (v | (v << 1)) & 0x55555555;
  return v;
}

/** The even bits of v gathered (the inverse of spread). */
function compact(v: number): number {
  v &= 0x55555555;
  v = (v | (v >>> 1)) & 0x33333333;
  v = (v | (v >>> 2)) & 0x0f0f0f0f;
  v = (v | (v >>> 4)) & 0x00ff00ff;
  v = (v | (v >>> 8)) & 0x0000ffff;
  return v;
}

/** Morton code of (x, y), 16 bits each: x in the even bits, y in the odd ones. */
export const morton = (x: number, y: number): number => (spread(x) | (spread(y) << 1)) >>> 0;

/** Height-order run [first, end) of the points in map tile z/x/y: a binary search of the codes. */
export function tileRun(codes: Uint32Array, z: number, x: number, y: number): [number, number] {
  // Below zoom 16, the tile's zoom-16 ancestor (its points are all in it).
  if (z > MORTON_Z) {
    x >>= z - MORTON_Z;
    y >>= z - MORTON_Z;
    z = MORTON_Z;
  }
  const s = MORTON_Z - z;
  const m0 = morton(x << s, y << s), m1 = m0 + 2 ** (2 * s);
  return [lowerBound(codes, m0), lowerBound(codes, m1)];
}

function lowerBound(a: Uint32Array, v: number): number {
  let lo = 0, hi = a.length;
  while (lo < hi) {
    const m = (lo + hi) >> 1;
    if (a[m] < v) lo = m + 1;
    else hi = m;
  }
  return lo;
}

/** Indices 0..n-1 sorted by their codes (LSD radix, two 16-bit passes). */
function sortByCode(codes: Uint32Array): Uint32Array {
  const n = codes.length;
  let a = new Uint32Array(n), b = new Uint32Array(n);
  for (let i = 0; i < n; i++) a[i] = i;
  const cnt = new Uint32Array(65537);
  for (const shift of [0, 16]) {
    cnt.fill(0);
    for (let i = 0; i < n; i++) cnt[((codes[a[i]] >>> shift) & 0xffff) + 1]++;
    for (let k = 0; k < 65536; k++) cnt[k + 1] += cnt[k];
    for (let i = 0; i < n; i++) {
      const j = a[i];
      b[cnt[(codes[j] >>> shift) & 0xffff]++] = j;
    }
    [a, b] = [b, a];
  }
  return a;
}

/** What the worker keeps of a layout to send the filter flags: draw position → index, height order
 * → draw position, the Morton codes (height order), fame and how many points each stands for
 * (draw order; null: one each). */
export interface DotAux {
  order: Uint32Array;
  byCode: Uint32Array;
  codes: Uint32Array;
  fa: Float32Array;
  weight: Uint32Array | null;
}

/**
 * The layout of a source's points (index order in, as the landmarks worker indexes them), and what
 * the worker keeps to send the filters in draw order (visWords).
 */
export function layoutDots(lon: Float64Array, lat: Float64Array, fa: Float32Array, ia: Float32Array, cls: Uint8Array, weight: Uint32Array | null = null): { data: DotData; aux: DotAux } {
  const n = lon.length, K = 1 << CHUNK_Z, Q = 1 << MORTON_Z;
  const mx = new Float64Array(n), my = new Float64Array(n);
  const chunk = new Uint16Array(n);
  const counts = new Uint32Array(K * K + 1);
  for (let i = 0; i < n; i++) {
    const s = Math.sin((lat[i] * Math.PI) / 180);
    const x = Math.min(1 - 1e-9, Math.max(0, (lon[i] + 180) / 360));
    const y = Math.min(1 - 1e-9, Math.max(0, 0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)));
    mx[i] = x;
    my[i] = y;
    const c = morton(Math.floor(x * K), Math.floor(y * K));
    chunk[i] = c;
    counts[c + 1]++;
  }
  for (let c = 0; c < K * K; c++) counts[c + 1] += counts[c];
  // Draw order: a stable counting sort by chunk (index order kept within one).
  const order = new Uint32Array(n);
  const next = counts.slice(0, K * K);
  for (let i = 0; i < n; i++) order[next[chunk[i]]++] = i;
  // Height order: by zoom-16 Morton code (its first bits are the chunk's).
  const code = new Uint32Array(n);
  for (let j = 0; j < n; j++) {
    const i = order[j];
    code[j] = morton(Math.floor(mx[i] * Q), Math.floor(my[i] * Q));
  }
  const byCode = sortByCode(code);
  const hidx = new Uint32Array(n);
  for (let k = 0; k < n; k++) hidx[byCode[k]] = k;

  const draw = new ArrayBuffer(n * DRAW_STRIDE);
  const f32 = new Float32Array(draw), u32 = new Uint32Array(draw), u8 = new Uint8Array(draw);
  const W = DRAW_STRIDE / 4;
  for (let j = 0; j < n; j++) {
    const i = order[j];
    const cx = Math.floor(mx[i] * K), cy = Math.floor(my[i] * K);
    f32[j * W] = (mx[i] * K - cx) * 8192;
    f32[j * W + 1] = (my[i] * K - cy) * 8192;
    f32[j * W + 2] = fa[i];
    f32[j * W + 3] = ia[i];
    u32[j * W + 4] = hidx[j];
    u8[j * DRAW_STRIDE + 20] = cx;
    u8[j * DRAW_STRIDE + 21] = cy;
    u8[j * DRAW_STRIDE + 22] = cls[i];
  }
  const hpos = new ArrayBuffer(n * HPOS_STRIDE);
  const hf = new Float32Array(hpos), hb = new Uint8Array(hpos);
  const HW = HPOS_STRIDE / 4;
  const codes = new Uint32Array(n);
  for (let k = 0; k < n; k++) {
    const j = byCode[k];
    hf[k * HW] = f32[j * W];
    hf[k * HW + 1] = f32[j * W + 1];
    hb[k * HPOS_STRIDE + 8] = u8[j * DRAW_STRIDE + 20];
    hb[k * HPOS_STRIDE + 9] = u8[j * DRAW_STRIDE + 21];
    codes[k] = code[j];
  }
  const chunks: number[] = [];
  for (let c = 0; c < K * K; c++) {
    const count = counts[c + 1] - counts[c];
    if (count) chunks.push(compact(c), compact(c >>> 1), counts[c], count);
  }
  const faDraw = new Float32Array(n);
  for (let j = 0; j < n; j++) faDraw[j] = fa[order[j]];
  let wDraw: Uint32Array | null = null;
  if (weight) {
    wDraw = new Uint32Array(n);
    for (let j = 0; j < n; j++) wDraw[j] = weight[order[j]];
  }
  return { data: { n, draw, hpos, morton: codes, chunks: Uint32Array.from(chunks) }, aux: { order, byCode, codes: codes.slice(), fa: faDraw, weight: wDraw } };
}

/**
 * The filter flags of a source's points, draw order, VIS_WORDS words each: bit 0 of the first,
 * visible; bits 8–15, the speck cell zooms (LOD_Z0 + bit) at which the point stands for its cell;
 * then a byte per zoom (4 a word): how many visible points it stands for there (at most 255).
 * `vis`: 1 per visible point, draw order. A speck cell from a thinned tile (a pseudo-point) counts as
 * the points it stands for (aux.weight).
 */
export function visWords(vis: Uint8Array, aux: DotAux): Uint32Array {
  const n = vis.length;
  const out = new Uint32Array(n * VIS_WORDS);
  for (let j = 0; j < n; j++) out[j * VIS_WORDS] = vis[j];
  const { byCode, codes, fa, weight } = aux;
  for (let l = 0; l < LOD_LEVELS; l++) {
    const shift = 2 * (MORTON_Z - LOD_Z0 - l);
    for (let k = 0; k < n; ) {
      const cell = codes[k] >>> shift;
      let best = -1, bestFa = Infinity, count = 0;
      for (; k < n && codes[k] >>> shift === cell; k++) {
        const j = byCode[k];
        if (!vis[j]) continue;
        count += weight ? weight[j] : 1;
        if (fa[j] < bestFa) {
          bestFa = fa[j];
          best = j;
        }
      }
      if (best < 0) continue;
      out[best * VIS_WORDS] |= 1 << (8 + l);
      out[best * VIS_WORDS + 1 + (l >> 2)] |= Math.min(255, count) << (8 * (l & 3));
    }
  }
  return out;
}
