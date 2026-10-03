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
//                            for dots (zero-length lines): the road length merged into the dot,
//                            tile units (the renderer draws its area, length × width)
//   16 u32 line index        within the tile
//   20 u8 × 12               scenic channels (roadcore::scenic::ch order)

import {
  CELLS, CLASS_GROUP, EQ, GQ, GPU_EOL, LF_TOLL, LF_UNNAMED, MINOR_MAX_CLASS, NCLASS, NSG, SPRITE_MAXZ, SPRITE_SEG_PX, ST_BRIDGE, ST_LINK, ST_TUNNEL, ST_UNPAVED, FERRY,
} from '../config';
import { levelZero, lodCells, lodSig, pieceLists, type LodFilter } from './lod';
import { legibleRgb } from '../linecolour';
import { DRAPE_OFF, ELEV_OFF, NCH, STRIDE, chOff, type DecodedTile, type WorkerRequest, type WorkerResponse } from './types';

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
    const tile = decode(buf, msg.z, msg.y, msg.lod);
    tile.decodeMs = performance.now() - t0;
    tile.bytes = buf.byteLength;
    post({ type: 'tile', id: msg.id, tile }, [
      tile.verts, tile.lineStart.buffer, tile.lineWay.buffer, tile.wayOrder.buffer, tile.lineStyle.buffer, tile.lineFlags.buffer, tile.lineRoadLen.buffer, tile.lineAttr.buffer, tile.lineColour.buffer,
      tile.rlStart.buffer, tile.rlRoad.buffer, tile.rlCum.buffer, tile.pieces.buffer, tile.eq.buffer, tile.gq.buffer, tile.glen.buffer, tile.clen.buffer, tile.ext.buffer,
      ...tile.levels.flatMap((l) => (l.mult ? [l.mult.buffer] : [])),
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

function decode(b: Uint8Array, z: number, ty: number, lod: LodFilter | null): DecodedTile {
  let pos = 4;
  // RT v7 only: its way column holds OSM way ids, lines sorted by draw class then id (older tiles
  // held the legacy build's way indices, which nothing can look up any more).
  if (b[0] !== 0x52 || b[1] !== 0x54 || b[2] !== 7) throw new Error(`bad tile header (expected RT v7, got v${b[2]})`);
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
  let nverts = rv();
  const lineStyle = b.slice(pos, pos + nlines);
  pos += nlines;
  const lineFlags = b.slice(pos, pos + nlines);
  pos += nlines;
  // Way of each line: its OSM id, delta-coded.
  const lineWay = new Uint32Array(nlines);
  let w = 0;
  for (let i = 0; i < nlines; i++) lineWay[i] = w += zz(rv());
  let lineStart = new Uint32Array(nlines + 1);
  for (let i = 0; i < nlines; i++) lineStart[i + 1] = lineStart[i] + rv();
  const trueLen = new Float32Array(nlines);
  for (let i = 0; i < nlines; i++) trueLen[i] = rv() / 10;
  const roadLen = new Float32Array(nlines);
  for (let i = 0; i < nlines; i++) roadLen[i] = rv();
  // Per-line attributes (network, maxspeed ÷ 2, lanes, surface) and line colour (0xRRGGBB + 1),
  // made legible on the dark map (linecolour.ts).
  const lineAttr = b.slice(pos, pos + nlines * 4);
  pos += nlines * 4;
  const lineColour = new Uint32Array(nlines);
  for (let i = 0; i < nlines; i++) {
    const c = rv();
    lineColour[i] = c ? legibleRgb(c - 1) + 1 : 0;
  }

  let verts = new ArrayBuffer(nverts * STRIDE);
  const S2 = STRIDE / 2, S4 = STRIDE / 4;
  {
    const i16 = new Int16Array(verts);
    const u8 = new Uint8Array(verts);
    let x = 0, y = 0;
    for (let i = 0; i < nverts; i++) {
      x += zz(rv());
      y += zz(rv());
      i16[i * S2] = x;
      i16[i * S2 + 1] = y;
    }
    const u16 = new Uint16Array(verts);
    let e = 0;
    for (let i = 0; i < nverts; i++) {
      e += zz(rv());
      u16[i * S2 + 2] = Math.min(65535, Math.max(0, e + ELEV_OFF));
    }
    for (let i = 0; i < nverts; i++) u8[i * STRIDE + 8] = b[pos++];
    let h = 0;
    for (let i = 0; i < nverts; i++) {
      h += zz(rv());
      u16[i * S2 + 3] = Math.min(65535, Math.max(0, h + DRAPE_OFF));
    }
    // Scenic channels: 13 (the last, roadside buildings, kept in the vertex's spare byte).
    for (let c = 0; c < NCH; c++) {
      let v = 0;
      for (let i = 0; i < nverts; i++) {
        v += zz(rv());
        u8[i * STRIDE + chOff(c)] = v;
      }
    }
  }

  // Zoomed-out tiles are drawn as one point sprite per piece (layer.ts), which needs every piece
  // short on screen: split the few long ones.
  if (z <= SPRITE_MAXZ) {
    const s = subdivide(verts, nverts, lineStart, (SPRITE_SEG_PX * extent) / 256);
    if (s) ({ verts, nverts, lineStart } = s);
  }
  const i16 = new Int16Array(verts);
  const u16 = new Uint16Array(verts);
  const u8 = new Uint8Array(verts);
  const f32 = new Float32Array(verts);
  const u32 = new Uint32Array(verts);

  // Metres per tile unit at this tile's latitude.
  const n = Math.PI - (2 * Math.PI * (ty + 0.5)) / (1 << z);
  const lat = Math.atan(Math.sinh(n));
  const mpu = (2 * Math.PI * 6371008.8 * Math.cos(lat)) / ((1 << z) * extent);

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
  let maxSeg = 0;
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
        if (sl > maxSeg) maxSeg = sl;
      }
      f32[i * S4 + 3] = d;
      u32[i * S4 + 4] = l;
      // Extremes per cell/group (vertex-exact).
      const cg = cellOf(i16[i * S2], i16[i * S2 + 1]) * NSG + group;
      const ev = (u16[i * S2 + 2] - ELEV_OFF) / 10;
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
      sample(c * NSG + group, u16[a * S2 + 2] - ELEV_OFF, u8[a * STRIDE + 8] / 2, tl);
      addLen((((c * NCLASS + cls) * 2 + unp) * 2 + un) * 2 + toll, roadLen[l], tl);
      // The road length the dot stands for, in tile units: the renderer draws its area.
      for (let i = a; i < bEnd; i++) f32[i * S4 + 3] = tl / mpu;
      continue;
    }
    const f = tl > 0 ? tl / simp : mpu;
    for (let i = a + 1; i < bEnd; i++) {
      const x0 = i16[(i - 1) * S2], y0 = i16[(i - 1) * S2 + 1], x1 = i16[i * S2], y1 = i16[i * S2 + 1];
      const sl = Math.sqrt((x1 - x0) ** 2 + (y1 - y0) ** 2);
      if (sl === 0) continue;
      const c = cellOf((x0 + x1) / 2, (y0 + y1) / 2);
      const lenM = sl * f;
      sample(c * NSG + group, (u16[(i - 1) * S2 + 2] + u16[i * S2 + 2]) / 2 - ELEV_OFF, (u8[(i - 1) * STRIDE + 8] + u8[i * STRIDE + 8]) / 4, lenM);
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
  // The lines in order of way id: (way, line) pairs as 64-bit integers (line in the low word, little
  // endian) in the typed array's native sort.
  const pairs = new Uint32Array(nlines * 2);
  for (let k = 0; k < nlines; k++) {
    pairs[2 * k] = k;
    pairs[2 * k + 1] = nWay[k];
  }
  new BigUint64Array(pairs.buffer).sort();
  const wayOrder = new Uint32Array(nlines);
  for (let i = 0; i < nlines; i++) wayOrder[i] = pairs[2 * i];
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

  minorStart = Math.max(minorStart, bridgeEnd);
  minorEnd = Math.max(minorEnd, bridgeEnd);
  const { pieces, levels } = pieceLists(out, levelZero(out, nverts), [bridgeEnd, minorStart, minorEnd], extent, lodCells(z, lod), lod, nRoad, z);

  return {
    extent, nverts, nlines, verts: out, lineStart: nStart, lineWay: nWay, wayOrder, lineStyle: nStyle, lineFlags: nFlags, lineRoadLen: nRoad, lineAttr: nAttr, lineColour: nColour, bridgeEnd,
    minorStart, minorEnd, maxSeg, pieces, levels, lodSig: lodCells(z, lod).length ? lodSig(lod) : '',
    eq, gq, glen, clen, rlStart, rlRoad, rlCum, ext, mpu, bytes: 0, decodeMs: 0,
  };
}

/**
 * Splits pieces longer than `maxLen` tile units into equal parts, interpolating the vertex fields
 * decoded so far (position, elevation, drape height, grade, scenic channels; the flags channel
 * steps at the middle, as in the tile builder). Returns null when no piece is that long.
 */
function subdivide(verts: ArrayBuffer, nverts: number, lineStart: Uint32Array, maxLen: number): { verts: ArrayBuffer; nverts: number; lineStart: Uint32Array<ArrayBuffer> } | null {
  const i16 = new Int16Array(verts);
  const S2 = STRIDE / 2;
  const nlines = lineStart.length - 1;
  const parts = (i: number) => {
    const dx = i16[(i + 1) * S2] - i16[i * S2], dy = i16[(i + 1) * S2 + 1] - i16[i * S2 + 1];
    return Math.ceil(Math.sqrt(dx * dx + dy * dy) / maxLen);
  };
  let extra = 0;
  for (let l = 0; l < nlines; l++) for (let i = lineStart[l]; i + 1 < lineStart[l + 1]; i++) extra += Math.max(1, parts(i)) - 1;
  if (!extra) return null;
  const out = new ArrayBuffer((nverts + extra) * STRIDE);
  const o8 = new Uint8Array(out), o16 = new Int16Array(out), ou16 = new Uint16Array(out);
  const u16 = new Uint16Array(verts);
  const u8 = new Uint8Array(verts);
  const starts = new Uint32Array(nlines + 1);
  const FLAGS_BYTE = chOff(7);
  let at = 0;
  for (let l = 0; l < nlines; l++) {
    starts[l] = at;
    const a = lineStart[l], b = lineStart[l + 1];
    for (let i = a; i < b; i++) {
      o8.set(u8.subarray(i * STRIDE, (i + 1) * STRIDE), at * STRIDE);
      at++;
      if (i + 1 >= b) break;
      const k = parts(i);
      for (let j = 1; j < k; j++) {
        const t = j / k;
        // (Position signed; elevation and drape height unsigned, offset: types.ts ELEV_OFF.)
        for (let f = 0; f < 2; f++) o16[at * S2 + f] = Math.round(i16[i * S2 + f] + (i16[(i + 1) * S2 + f] - i16[i * S2 + f]) * t);
        for (let f = 2; f < 4; f++) ou16[at * S2 + f] = Math.round(u16[i * S2 + f] + (u16[(i + 1) * S2 + f] - u16[i * S2 + f]) * t);
        for (const off of [8, 11, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31]) {
          o8[at * STRIDE + off] = off === FLAGS_BYTE ? u8[(t < 0.5 ? i : i + 1) * STRIDE + off] : Math.round(u8[i * STRIDE + off] + (u8[(i + 1) * STRIDE + off] - u8[i * STRIDE + off]) * t);
        }
        at++;
      }
    }
  }
  starts[nlines] = at;
  return { verts: out, nverts: at, lineStart: starts };
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
