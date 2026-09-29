/// <reference lib="webworker" />
// Fetches and decodes "RT" road tiles into GPU-ready vertex buffers plus per-cell
// statistics (length-weighted elevation / grade quantile sketches).
//
// GPU vertex layout, 32 bytes (STRIDE):
//   0  i16 x, i16 y          tile units
//   4  i16 elevation         decimetres
//   6  i16 drape height      metres (terrain surface, for 3D)
//   8  u8  |grade|           0.5 % units
//   9  u8  style             class | UNPAVED | BRIDGE | TUNNEL | EOL (last vertex of a line)
//   10 u8  line flags        LF_UNNAMED
//   12 f32 distance          tile units from the start of the line (dash phase);
//                            for dots (zero-length lines): road-length density weight 0..1
//   16 u32 line index        within the tile
//   20 u8 × 12               scenic channels (roadcore::scenic::ch order)

import { CELLS, CLASS_GROUP, EQ, GQ, GPU_EOL, LF_TOLL, LF_UNNAMED, MINOR_MAX_CLASS, NCLASS, NSG, ST_BRIDGE, ST_LINK, ST_TUNNEL, ST_UNPAVED, FERRY } from '../config';
import { NCH, STRIDE, chOff, type DecodedTile, type WorkerRequest, type WorkerResponse } from './types';

const inflight = new Map<number, AbortController>();

self.onmessage = async (ev: MessageEvent<WorkerRequest>) => {
  const msg = ev.data;
  if (msg.type === 'abort') {
    inflight.get(msg.id)?.abort();
    inflight.delete(msg.id);
    return;
  }
  const ctrl = new AbortController();
  inflight.set(msg.id, ctrl);
  try {
    const res = await fetch(msg.url, { signal: ctrl.signal });
    if (res.status === 204) return post({ type: 'tile', id: msg.id, tile: null });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const buf = new Uint8Array(await res.arrayBuffer());
    const t0 = performance.now();
    const tile = decode(buf, msg.z, msg.y);
    tile.decodeMs = performance.now() - t0;
    tile.bytes = buf.byteLength;
    post({ type: 'tile', id: msg.id, tile }, [
      tile.verts, tile.lineStart.buffer, tile.lineWay.buffer, tile.lineStyle.buffer, tile.lineFlags.buffer, tile.lineRoadLen.buffer, tile.lineAttr.buffer, tile.lineColour.buffer,
      tile.rlStart.buffer, tile.rlRoad.buffer, tile.rlCum.buffer, tile.eq.buffer, tile.gq.buffer, tile.glen.buffer, tile.clen.buffer, tile.ext.buffer,
    ]);
  } catch (e) {
    if ((e as Error).name !== 'AbortError') post({ type: 'error', id: msg.id, message: String(e) });
  } finally {
    inflight.delete(msg.id);
  }
};

function post(m: WorkerResponse, transfer: Transferable[] = []) {
  (self as unknown as DedicatedWorkerGlobalScope).postMessage(m, transfer);
}

function decode(b: Uint8Array, z: number, ty: number): DecodedTile {
  let pos = 4;
  const version = b[2];
  if (b[0] !== 0x52 || b[1] !== 0x54 || version < 4 || version > 6) throw new Error('bad tile header (expected RT v4–v6)');
  const extent = 1 << b[3];
  const rv = (): number => {
    let r = 0, s = 1, c: number;
    do {
      c = b[pos++];
      r += (c & 0x7f) * s;
      s *= 128;
    } while (c & 0x80);
    return r;
  };
  const zz = (v: number) => (v % 2 === 0 ? v / 2 : -(v + 1) / 2);

  const nlines = rv();
  const nverts = rv();
  const lineStyle = b.slice(pos, pos + nlines);
  pos += nlines;
  const lineFlags = b.slice(pos, pos + nlines);
  pos += nlines;
  const lineWay = new Uint32Array(nlines);
  let w = 0;
  for (let i = 0; i < nlines; i++) lineWay[i] = w += zz(rv());
  const lineStart = new Uint32Array(nlines + 1);
  for (let i = 0; i < nlines; i++) lineStart[i + 1] = lineStart[i] + rv();
  const trueLen = new Float32Array(nlines);
  for (let i = 0; i < nlines; i++) trueLen[i] = rv() / 10;
  const roadLen = new Float32Array(nlines);
  for (let i = 0; i < nlines; i++) roadLen[i] = rv();
  // v5: per-line attributes (network, maxspeed ÷ 2, lanes, surface) and line colour (0xRRGGBB + 1).
  let lineAttr = new Uint8Array(nlines * 4);
  const lineColour = new Uint32Array(nlines);
  if (version >= 5) {
    lineAttr = b.slice(pos, pos + nlines * 4);
    pos += nlines * 4;
    for (let i = 0; i < nlines; i++) lineColour[i] = rv();
  }

  const verts = new ArrayBuffer(nverts * STRIDE);
  const i16 = new Int16Array(verts);
  const u8 = new Uint8Array(verts);
  const f32 = new Float32Array(verts);
  const u32 = new Uint32Array(verts);
  const S2 = STRIDE / 2, S4 = STRIDE / 4;
  let x = 0, y = 0;
  for (let i = 0; i < nverts; i++) {
    x += zz(rv());
    y += zz(rv());
    i16[i * S2] = x;
    i16[i * S2 + 1] = y;
  }
  let e = 0;
  for (let i = 0; i < nverts; i++) {
    e += zz(rv());
    i16[i * S2 + 2] = e;
  }
  for (let i = 0; i < nverts; i++) u8[i * STRIDE + 8] = b[pos++];
  let h = 0;
  for (let i = 0; i < nverts; i++) {
    h += zz(rv());
    i16[i * S2 + 3] = h;
  }
  // Scenic channels: 12 (v4, v5) or 13 (v6: roadside buildings, kept in the vertex's spare byte).
  for (let c = 0; c < (version >= 6 ? NCH : 12); c++) {
    let v = 0;
    for (let i = 0; i < nverts; i++) {
      v += zz(rv());
      u8[i * STRIDE + chOff(c)] = v;
    }
  }

  // Metres per tile unit at this tile's latitude.
  const n = Math.PI - (2 * Math.PI * (ty + 0.5)) / (1 << z);
  const lat = Math.atan(Math.sinh(n));
  const mpu = (2 * Math.PI * 6371008.8 * Math.cos(lat)) / ((1 << z) * extent);
  const halfPxM = (mpu * extent) / 512;

  // Per-line fills: style/EOL, distance, line index; and collect statistics samples.
  const nsamp = nverts; // upper bound: segments + dots
  const eKeys = new Float64Array(nsamp);
  const gKeys = new Float64Array(nsamp);
  let ns = 0;
  const NCG = CELLS * CELLS * NSG;
  const NK = CELLS * CELLS * NCLASS * 8;
  const clen = new Float32Array(NK); // (cell, class, unpaved, unnamed, toll)
  // The same lengths keyed by (clen index, whole-road length in m), merged along each line.
  const rl = new Map<number, number>();
  let rlKey = -1, rlAcc = 0;
  const addLen = (k: number, road: number, len: number) => {
    clen[k] += len;
    const key = k * 16777216 + Math.min(16777215, Math.round(road));
    if (key !== rlKey) {
      if (rlKey >= 0) rl.set(rlKey, (rl.get(rlKey) ?? 0) + rlAcc);
      rlKey = key;
      rlAcc = 0;
    }
    rlAcc += len;
  };
  const ext = new Float32Array(NCG * 8);
  for (let k = 0; k < NCG; k++) {
    ext[k * 8] = -Infinity;
    ext[k * 8 + 4] = Infinity;
  }
  const cellOf = (px: number, py: number) => {
    const cx = Math.min(CELLS - 1, Math.max(0, Math.floor((px / extent) * CELLS)));
    const cy = Math.min(CELLS - 1, Math.max(0, Math.floor((py / extent) * CELLS)));
    return cy * CELLS + cx;
  };
  const sample = (cg: number, elevDm: number, grade2: number, lenM: number) => {
    const lq = Math.min(1048575, Math.round(lenM * 4));
    if (lq <= 0) return;
    const eq = Math.max(0, Math.min(65535, Math.round(elevDm) + 32768));
    const gq = Math.max(0, Math.min(1023, Math.round(grade2 * 2))); // 0.5 % units
    eKeys[ns] = (cg * 65536 + eq) * 1048576 + lq;
    gKeys[ns] = (cg * 1024 + gq) * 1048576 + lq;
    ns++;
  };

  let bridgeStart = nverts;
  let firstBridgeLine = nlines;
  for (let l = 0; l < nlines; l++) {
    const st = lineStyle[l];
    const cls = st & 15;
    const unp = (st & ST_UNPAVED) !== 0 ? 1 : 0;
    const un = (lineFlags[l] & LF_UNNAMED) !== 0 ? 1 : 0;
    // Roads: toll (rail tiles use the bit for a service group; their stats count both).
    const toll = (lineFlags[l] & LF_TOLL) !== 0 ? 1 : 0;
    const group = (CLASS_GROUP[cls] * 2 + unp) * 2 + un;
    const isBridge = (st & ST_BRIDGE) !== 0 && cls !== FERRY && (st & ST_TUNNEL) === 0;
    if (isBridge && bridgeStart === nverts) {
      bridgeStart = lineStart[l];
      firstBridgeLine = l;
    }
    const a = lineStart[l], bEnd = lineStart[l + 1];
    const gpuStyle = st & ~ST_LINK & 0x7f;
    let d = 0;
    let simp = 0;
    for (let i = a; i < bEnd; i++) {
      u8[i * STRIDE + 9] = gpuStyle | (i === bEnd - 1 ? GPU_EOL : 0);
      u8[i * STRIDE + 10] = lineFlags[l];
      if (i > a) {
        const dx = i16[i * S2] - i16[(i - 1) * S2];
        const dy = i16[i * S2 + 1] - i16[(i - 1) * S2 + 1];
        const sl = Math.sqrt(dx * dx + dy * dy);
        d += sl;
        simp += sl;
      }
      f32[i * S4 + 3] = d;
      u32[i * S4 + 4] = l;
      // Extremes per cell/group (vertex-exact).
      const cg = cellOf(i16[i * S2], i16[i * S2 + 1]) * NSG + group;
      const ev = i16[i * S2 + 2] / 10;
      if (ev > ext[cg * 8]) {
        ext[cg * 8] = ev; ext[cg * 8 + 1] = i16[i * S2]; ext[cg * 8 + 2] = i16[i * S2 + 1]; ext[cg * 8 + 3] = l;
      }
      if (ev < ext[cg * 8 + 4]) {
        ext[cg * 8 + 4] = ev; ext[cg * 8 + 5] = i16[i * S2]; ext[cg * 8 + 6] = i16[i * S2 + 1]; ext[cg * 8 + 7] = l;
      }
    }
    // Length-weighted samples; scale simplified lengths up to the true length.
    const tl = trueLen[l];
    if (simp <= 0) {
      const c = cellOf(i16[a * S2], i16[a * S2 + 1]);
      sample(c * NSG + group, i16[a * S2 + 2], u8[a * STRIDE + 8] / 2, tl);
      addLen((((c * NCLASS + cls) * 2 + unp) * 2 + un) * 2 + toll, roadLen[l], tl);
      // Density weight for the renderer: road length per half-pixel cell, saturating at 2 cells' width.
      const w = Math.min(1, tl / (2 * halfPxM));
      for (let i = a; i < bEnd; i++) f32[i * S4 + 3] = w;
      continue;
    }
    const f = tl > 0 ? tl / simp : mpu;
    for (let i = a + 1; i < bEnd; i++) {
      const x0 = i16[(i - 1) * S2], y0 = i16[(i - 1) * S2 + 1], x1 = i16[i * S2], y1 = i16[i * S2 + 1];
      const sl = Math.sqrt((x1 - x0) ** 2 + (y1 - y0) ** 2);
      if (sl === 0) continue;
      const c = cellOf((x0 + x1) / 2, (y0 + y1) / 2);
      const lenM = sl * f;
      sample(c * NSG + group, (i16[(i - 1) * S2 + 2] + i16[i * S2 + 2]) / 2, (u8[(i - 1) * STRIDE + 8] + u8[i * STRIDE + 8]) / 4, lenM);
      addLen((((c * NCLASS + cls) * 2 + unp) * 2 + un) * 2 + toll, roadLen[l], lenM);
    }
  }
  if (rlKey >= 0) rl.set(rlKey, (rl.get(rlKey) ?? 0) + rlAcc);
  const rlKeys = Float64Array.from(rl.keys()).sort();
  const rlStart = new Uint32Array(NK + 1);
  const rlRoad = new Float32Array(rlKeys.length);
  const rlCum = new Float64Array(rlKeys.length + 1);
  for (let i = 0; i < rlKeys.length; i++) {
    const k = Math.floor(rlKeys[i] / 16777216);
    rlStart[k + 1]++;
    rlRoad[i] = rlKeys[i] - k * 16777216;
    rlCum[i + 1] = rlCum[i] + rl.get(rlKeys[i])!;
  }
  for (let k = 0; k < NK; k++) rlStart[k + 1] += rlStart[k];

  const eSorted = eKeys.subarray(0, ns).sort();
  const gSorted = gKeys.subarray(0, ns).sort();
  const glen = new Float32Array(NCG);
  const eq = sketch(eSorted, 65536, EQ, (v) => (v - 32768) / 10, glen);
  const gq = sketch(gSorted, 1024, GQ, (v) => v / 2, null);
  for (let k = 0; k < NCG; k++) {
    if (!isFinite(ext[k * 8])) ext[k * 8] = NaN;
    if (!isFinite(ext[k * 8 + 4])) ext[k * 8 + 4] = NaN;
  }

  // GPU draw order. The renderer lets the first road drawn at a pixel win (so overlapping
  // translucent roads don't compound), while tiles store lines bottom → top: ferries & tunnels,
  // roads minor → major, bridges minor → major. Reverse each group and put bridges first:
  // [bridges major → minor][roads major → minor, tunnels & ferries last].
  const order: number[] = [];
  for (let l = nlines - 1; l >= firstBridgeLine; l--) order.push(l);
  for (let l = firstBridgeLine - 1; l >= 0; l--) order.push(l);
  const newIndex = new Uint32Array(nlines);
  const out = new ArrayBuffer(nverts * STRIDE);
  const o8 = new Uint8Array(out);
  const o32 = new Uint32Array(out);
  const nStart = new Uint32Array(nlines + 1);
  const nWay = new Uint32Array(nlines);
  const nStyle = new Uint8Array(nlines);
  const nFlags = new Uint8Array(nlines);
  const nRoad = new Float32Array(nlines);
  const nAttr = new Uint8Array(nlines * 4);
  const nColour = new Uint32Array(nlines);
  let at = 0;
  order.forEach((l, k) => {
    newIndex[l] = k;
    const a = lineStart[l], bEnd = lineStart[l + 1];
    o8.set(u8.subarray(a * STRIDE, bEnd * STRIDE), at * STRIDE);
    for (let i = 0; i < bEnd - a; i++) o32[(at + i) * S4 + 4] = k;
    nStart[k] = at;
    nWay[k] = lineWay[l];
    nStyle[k] = lineStyle[l];
    nFlags[k] = lineFlags[l];
    nRoad[k] = roadLen[l];
    nAttr.set(lineAttr.subarray(l * 4, l * 4 + 4), k * 4);
    nColour[k] = lineColour[l];
    at += bEnd - a;
  });
  nStart[nlines] = at;
  const bridgeEnd = nverts - bridgeStart;
  // The minor classes among the roads: after the majors, before tunnels & ferries (a contiguous run,
  // as the roads go major → minor).
  const nBridgeLines = nlines - firstBridgeLine;
  const lastGroup = (st: number) => (st & 15) === FERRY || (st & ST_TUNNEL) !== 0;
  let minorStart = nverts, minorEnd = nverts;
  for (let k = nBridgeLines; k < nlines; k++) {
    const st = nStyle[k];
    if (lastGroup(st)) {
      minorEnd = nStart[k];
      if (minorStart === nverts) minorStart = minorEnd;
      break;
    }
    if (minorStart === nverts && (st & 15) <= MINOR_MAX_CLASS) minorStart = nStart[k];
  }
  for (let k = 0; k < NCG; k++) {
    if (!Number.isNaN(ext[k * 8])) ext[k * 8 + 3] = newIndex[ext[k * 8 + 3]];
    if (!Number.isNaN(ext[k * 8 + 4])) ext[k * 8 + 7] = newIndex[ext[k * 8 + 7]];
  }

  return {
    extent, nverts, nlines, verts: out, lineStart: nStart, lineWay: nWay, lineStyle: nStyle, lineFlags: nFlags, lineRoadLen: nRoad, lineAttr: nAttr, lineColour: nColour, bridgeEnd,
    minorStart: Math.max(minorStart, bridgeEnd), minorEnd: Math.max(minorEnd, bridgeEnd),
    eq, gq, glen, clen, rlStart, rlRoad, rlCum, ext, mpu, bytes: 0, decodeMs: 0,
  };
}

/** Weighted quantiles per (cell, group) run of composite-sorted keys. */
function sketch(keys: Float64Array, span: number, nq: number, unq: (v: number) => number, totals: Float32Array | null): Float32Array {
  const NCG = CELLS * CELLS * NSG;
  const out = new Float32Array(NCG * nq).fill(NaN);
  let i = 0;
  const M = 1048576;
  while (i < keys.length) {
    const hi = Math.floor(keys[i] / M);
    const cg = Math.floor(hi / span);
    let j = i;
    let total = 0;
    while (j < keys.length && Math.floor(Math.floor(keys[j] / M) / span) === cg) {
      total += keys[j] % M;
      j++;
    }
    if (totals) totals[cg] = total / 4;
    // Walk the run once, emitting each quantile when the cumulative weight passes it.
    let acc = 0;
    let q = 0;
    for (let k = i; k < j && q < nq; k++) {
      const wgt = keys[k] % M;
      const val = unq(Math.floor(keys[k] / M) % span);
      const next = acc + wgt;
      while (q < nq && (q === 0 ? true : next >= (q / (nq - 1)) * total)) {
        out[cg * nq + q] = val;
        q++;
        if (q === nq) break;
      }
      acc = next;
    }
    // Maximum = last value in the run.
    out[cg * nq + nq - 1] = unq(Math.floor(keys[j - 1] / M) % span);
    i = j;
  }
  return out;
}
