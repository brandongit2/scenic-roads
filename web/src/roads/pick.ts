// CPU picking: a lazily-built uniform grid of segments per tile.
import { GPU_EOL } from '../config';
import { STRIDE } from './types';

const S2 = STRIDE / 2, S4 = STRIDE / 4;

const G = 32;

export interface PickHit {
  line: number;
  seg: number; // index of the segment's first vertex
  t: number; // 0..1 along the segment
  dist: number; // tile units from the centre line
}

export class PickGrid {
  private start: Uint32Array;
  private items: Uint32Array;
  private cell: number;
  private i16: Int16Array;
  private u32: Uint32Array;

  constructor(verts: ArrayBuffer, nverts: number, extent: number) {
    const i16 = (this.i16 = new Int16Array(verts));
    const u8 = new Uint8Array(verts);
    this.u32 = new Uint32Array(verts);
    const cell = (this.cell = extent / G);
    const c = (v: number) => Math.max(0, Math.min(G - 1, Math.floor(v / cell)));
    // Each segment's cells (x0, y0, x1, y1; 255: a line's end), for both passes. Built on the first
    // hover over a tile, a hundred thousand segments and more: no allocation per segment.
    const n = Math.max(0, nverts - 1);
    const box = new Uint8Array(n * 4);
    const counts = new Uint32Array(G * G + 1);
    for (let i = 0; i < n; i++) {
      const o = i * 4;
      if (u8[i * STRIDE + 9] & GPU_EOL) {
        box[o] = 255;
        continue;
      }
      const x0 = i16[i * S2], y0 = i16[i * S2 + 1], x1 = i16[(i + 1) * S2], y1 = i16[(i + 1) * S2 + 1];
      const ax = c(Math.min(x0, x1)), ay = c(Math.min(y0, y1)), bx = c(Math.max(x0, x1)), by = c(Math.max(y0, y1));
      box[o] = ax;
      box[o + 1] = ay;
      box[o + 2] = bx;
      box[o + 3] = by;
      for (let cy = ay; cy <= by; cy++) for (let cx = ax; cx <= bx; cx++) counts[cy * G + cx + 1]++;
    }
    for (let k = 1; k <= G * G; k++) counts[k] += counts[k - 1];
    this.start = counts.slice();
    this.items = new Uint32Array(counts[G * G]);
    const fill = counts;
    for (let i = 0; i < n; i++) {
      const o = i * 4;
      if (box[o] === 255) continue;
      for (let cy = box[o + 1]; cy <= box[o + 3]; cy++) for (let cx = box[o]; cx <= box[o + 2]; cx++) this.items[fill[cy * G + cx]++] = i;
    }
  }

  /** Segments with a bounding cell within `radius` tile units of (x, y), ascending (= draw priority). */
  candidates(x: number, y: number, radius: number, visible: (seg: number) => boolean): number[] {
    const c = (v: number) => Math.max(0, Math.min(G - 1, Math.floor(v / this.cell)));
    const out = new Set<number>();
    for (let cy = c(y - radius); cy <= c(y + radius); cy++) {
      for (let cx = c(x - radius); cx <= c(x + radius); cx++) {
        const k = cy * G + cx;
        for (let j = this.start[k]; j < this.start[k + 1]; j++) {
          const i = this.items[j];
          if (out.has(i) || !visible(i)) continue;
          // Cheap ground-distance prefilter.
          const { i16 } = this;
          const x0 = i16[i * S2], y0 = i16[i * S2 + 1], x1 = i16[(i + 1) * S2], y1 = i16[(i + 1) * S2 + 1];
          const dx = x1 - x0, dy = y1 - y0;
          const l2 = dx * dx + dy * dy;
          const t = l2 > 0 ? Math.max(0, Math.min(1, ((x - x0) * dx + (y - y0) * dy) / l2)) : 0;
          if (Math.hypot(x0 + t * dx - x, y0 + t * dy - y) <= radius) out.add(i);
        }
      }
    }
    return [...out].sort((a, b) => a - b);
  }

  /** Nearest segment to (x, y) within `radius` tile units, scored by distance minus half-width. */
  query(x: number, y: number, radius: number, halfWidth: (seg: number) => number, visible: (seg: number) => boolean): PickHit | null {
    const { i16 } = this;
    const c = (v: number) => Math.max(0, Math.min(G - 1, Math.floor(v / this.cell)));
    let best: PickHit | null = null;
    let bestScore = Infinity;
    const seen = new Set<number>();
    for (let cy = c(y - radius); cy <= c(y + radius); cy++) {
      for (let cx = c(x - radius); cx <= c(x + radius); cx++) {
        const k = cy * G + cx;
        for (let j = this.start[k]; j < this.start[k + 1]; j++) {
          const i = this.items[j];
          if (seen.has(i)) continue;
          seen.add(i);
          if (!visible(i)) continue;
          const x0 = i16[i * S2], y0 = i16[i * S2 + 1], x1 = i16[(i + 1) * S2], y1 = i16[(i + 1) * S2 + 1];
          const dx = x1 - x0, dy = y1 - y0;
          const l2 = dx * dx + dy * dy;
          const t = l2 > 0 ? Math.max(0, Math.min(1, ((x - x0) * dx + (y - y0) * dy) / l2)) : 0;
          const px = x0 + t * dx - x, py = y0 + t * dy - y;
          const dist = Math.sqrt(px * px + py * py);
          const score = dist - halfWidth(i);
          // Ties (overlapping lines): prefer later segments, which are drawn on top.
          if (score < radius && score <= bestScore) {
            bestScore = score;
            best = { line: this.u32[i * S4 + 4], seg: i, t, dist };
          }
        }
      }
    }
    return best;
  }
}
