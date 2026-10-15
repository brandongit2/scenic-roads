// The buildings field's tiles fetched and decoded off the main thread (bldfield.ts; the format:
// docs/formats.md `field5-webp`, crates/pipeline/src/bld/field.rs). A tile is a class mask byte, a
// lossless WebP 512 wide and 2 × 512 × (the classes held) tall (a block a class: R √f, G log2(1 + w)
// ÷ log2(4097), B the class's mean top in its class's range, × 255; then a block a class: R and G the
// offset to its buildings' centroid + 128), and, on a level-8 tile, its heritage list after it.
// Decoded here to linear values (before the GPU's mipmaps: the coding's square root and log would
// bias them), with what the march reads beside them: each 1/32 and 1/128 of the tile's tallest mean
// top (with the texel around), the class mask pyramid, and each class texel's share code and offset
// to its buildings' centroid; and, asked again with the tile's elevation grid, each texel's roof
// ground (bldfield.offsets.ts).

import { roofGround } from './bldfield.offsets';

export type FieldIn =
  | { type: 'load'; key: string; url: string }
  | { type: 'cancel'; key: string }
  /** Each texel's roof ground for the elevations `ver` (the grid GE × GE, M cells beyond the tile). */
  | { type: 'dg'; key: string; ver: string; texels: Uint8Array; n: number; ee: Float32Array; GE: number; M: number };

export type FieldOut =
  | { type: 'done'; key: string; empty: true }
  | { type: 'failed'; key: string; error: string }
  /** Share × each texel's roof ground (R16F's, a layer a class held), each cell's largest (32 × 32);
   * the texels handed back. */
  | { type: 'dg'; key: string; ver: string; dg: Float32Array; dgc: Float32Array; texels: Uint8Array }
  | {
      type: 'done';
      key: string;
      empty: false;
      /** Each class present: v, w, f (Float32, 512 × 512 × 3); null where absent. */
      classes: (Float32Array | null)[];
      mask: number;
      /** The tallest mean top (m). */
      top: number;
      top32: Float32Array;
      top128: Float32Array;
      /** The class mask pyramid: levels 512 … 1, one after the other. */
      cmask: Uint8Array;
      bytes: number;
      heritage: Uint8Array | null;
      /** Each class texel held (a layer each, layer-major): its share's code (√f × 255) and its offset
       * to its buildings' centroid + 128 (x, y), 3 bytes: for its roof ground, once the elevations are
       * known. */
      texels: Uint8Array;
    };

const T = 512;
const N = T * T;
const LO = [0, 8, 20, 40, 100];
const HI = [8, 20, 40, 100, 1000];
const W_LOG = Math.log2(4097);

const inflight = new Map<string, AbortController>();

/** The tile's parts: mask, WebP, what follows. */
function split(b: Uint8Array): { mask: number; webp: Uint8Array; tail: Uint8Array } {
  if (b.length < 13 || String.fromCharCode(b[1], b[2], b[3], b[4]) !== 'RIFF') throw new Error('not a field5-webp tile');
  const riff = b[5] | (b[6] << 8) | (b[7] << 16) | (b[8] << 24);
  const end = 9 + riff;
  if (end > b.length) throw new Error('a field5-webp tile cut short');
  return { mask: b[0], webp: b.subarray(1, end), tail: b.subarray(end) };
}

async function load(key: string, url: string): Promise<FieldOut> {
  const ac = new AbortController();
  inflight.set(key, ac);
  try {
    const r = await fetch(url, { signal: ac.signal });
    if (r.status === 204 || r.status === 404) return { type: 'done', key, empty: true };
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    const b = new Uint8Array(await r.arrayBuffer());
    const { mask, webp, tail } = split(b);
    const n = popcount(mask);
    const bmp = await createImageBitmap(new Blob([webp as BlobPart], { type: 'image/webp' }), { premultiplyAlpha: 'none', colorSpaceConversion: 'none' });
    if (bmp.width !== T || bmp.height !== 2 * T * n) throw new Error(`a field tile ${bmp.width} × ${bmp.height}`);
    const cv = new OffscreenCanvas(T, 2 * T * n);
    const cx = cv.getContext('2d', { willReadFrequently: true })!;
    cx.drawImage(bmp, 0, 0);
    bmp.close();
    const px = cx.getImageData(0, 0, T, 2 * T * n).data;
    // (each class texel's share code, then its offset from the offsets' blocks after the classes')
    const texels = new Uint8Array(n * N * 3);
    for (let l = 0; l < n; l++) {
      const base = l * N * 4, ob = (n + l) * N * 4;
      for (let i = 0; i < N; i++) {
        texels[(l * N + i) * 3] = px[base + i * 4];
        texels[(l * N + i) * 3 + 1] = px[ob + i * 4];
        texels[(l * N + i) * 3 + 2] = px[ob + i * 4 + 1];
      }
    }
    const classes: (Float32Array | null)[] = [];
    const hmax = new Float32Array(N);
    const m = new Uint8Array(N);
    let top = 0;
    let row = 0;
    for (let c = 0; c < 5; c++) {
      if (!(mask & (1 << c))) {
        classes.push(null);
        continue;
      }
      const o = new Float32Array(N * 3);
      const base = row * N * 4;
      for (let i = 0; i < N; i++) {
        const R = px[base + i * 4], G = px[base + i * 4 + 1], B = px[base + i * 4 + 2];
        if (!R && !G) continue;
        const f = (R / 255) ** 2;
        const w = 2 ** ((G / 255) * W_LOG) - 1;
        const h = LO[c] + (B / 255) * (HI[c] - LO[c]);
        // (v, w, f: R11F_G11F_B10F's channels, 6, 6 and 5 bits of mantissa, rounded here to the
        // nearest, not left to the driver.)
        o[i * 3] = toMantissa(f * h, 6);
        o[i * 3 + 1] = toMantissa(w, 6);
        o[i * 3 + 2] = toMantissa(f, 5);
        m[i] |= 1 << c;
        if (f > 0) {
          if (h > hmax[i]) hmax[i] = h;
          if (h > top) top = h;
        }
      }
      classes.push(o);
      row++;
    }
    const pool = (cells: number) => {
      const out = new Float32Array(cells * cells);
      const sh = Math.log2(T / cells);
      for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) {
        const h = hmax[y * T + x];
        if (!h) continue;
        for (let cy = Math.max(0, (y - 1) >> sh); cy <= Math.min(cells - 1, (y + 1) >> sh); cy++)
          for (let cx2 = Math.max(0, (x - 1) >> sh); cx2 <= Math.min(cells - 1, (x + 1) >> sh); cx2++) if (h > out[cy * cells + cx2]) out[cy * cells + cx2] = h;
      }
      return out;
    };
    // The classes each texel and its neighbours hold, as a pyramid (each coarser level the four's,
    // dilated by a texel again).
    const dil = (a: Uint8Array, sz: number) => {
      const o = new Uint8Array(sz * sz);
      for (let y = 0; y < sz; y++) for (let x = 0; x < sz; x++) {
        let v = 0;
        for (let yy = Math.max(0, y - 1); yy <= Math.min(sz - 1, y + 1); yy++) for (let xx = Math.max(0, x - 1); xx <= Math.min(sz - 1, x + 1); xx++) v |= a[yy * sz + xx];
        o[y * sz + x] = v;
      }
      return o;
    };
    const levels: Uint8Array[] = [];
    let cur = dil(m, T);
    for (let sz = T; ; sz >>= 1) {
      levels.push(cur);
      if (sz === 1) break;
      const h = sz >> 1, o = new Uint8Array(h * h);
      for (let y = 0; y < h; y++) for (let x = 0; x < h; x++) o[y * h + x] = cur[2 * y * sz + 2 * x] | cur[2 * y * sz + 2 * x + 1] | cur[(2 * y + 1) * sz + 2 * x] | cur[(2 * y + 1) * sz + 2 * x + 1];
      cur = dil(o, h);
    }
    const cmask = new Uint8Array(levels.reduce((s, l) => s + l.length, 0));
    let at = 0;
    for (const l of levels) {
      cmask.set(l, at);
      at += l.length;
    }
    return { type: 'done', key, empty: false, classes, mask, top, top32: pool(32), top128: pool(128), cmask, bytes: b.length, heritage: tail.length ? tail.slice() : null, texels };
  } finally {
    inflight.delete(key);
  }
}

/** `x` (≥ 0) rounded to the nearest number with `bits` bits of mantissa after the leading one. */
function toMantissa(x: number, bits: number): number {
  if (!(x > 0)) return 0;
  const e = Math.floor(Math.log2(x));
  const q = 2 ** (e - bits);
  return Math.round(x / q) * q;
}

function popcount(m: number) {
  let n = 0;
  for (let k = 0; k < 5; k++) if (m & (1 << k)) n++;
  return n;
}

self.onmessage = (e: MessageEvent<FieldIn>) => {
  const msg = e.data;
  if (msg.type === 'cancel') {
    inflight.get(msg.key)?.abort();
    return;
  }
  if (msg.type === 'dg') {
    const { dg, dgc } = roofGround(msg.texels, msg.n, T, msg.ee, msg.GE, msg.M);
    (self as unknown as Worker).postMessage({ type: 'dg', key: msg.key, ver: msg.ver, dg, dgc, texels: msg.texels } satisfies FieldOut, [dg.buffer, dgc.buffer, msg.texels.buffer]);
    return;
  }
  load(msg.key, msg.url).then(
    (out) => {
      if (out.type === 'done' && !out.empty) {
        const tr: Transferable[] = [out.top32.buffer, out.top128.buffer, out.cmask.buffer, out.texels.buffer];
        for (const c of out.classes) if (c) tr.push(c.buffer);
        (self as unknown as Worker).postMessage(out, tr);
      } else (self as unknown as Worker).postMessage(out);
    },
    (err: unknown) => (self as unknown as Worker).postMessage({ type: 'failed', key: msg.key, error: err instanceof Error && err.name === 'AbortError' ? 'cancelled' : String(err) } satisfies FieldOut),
  );
};
