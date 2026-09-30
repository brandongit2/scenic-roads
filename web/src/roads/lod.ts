// A tile's piece lists for the sprite draw (layer.ts), built in the tile workers and rebuilt on the
// main thread when the road filters change.
//
// Level 0 is every piece (its first vertex), in draw order, without the line ends, which the
// vertex-pair layout would otherwise run the shaders for. Coarser levels (for tiles drawn small:
// zoomed out, far off in tilted views, or zoomed out beyond the coarsest tiles) keep, in draw
// order, only the shown pieces that reach a cell of `cell` tile units no piece kept before them
// reaches (the cells along each piece). At a pixel the first road drawn wins (layer.ts), so once
// cells are about a pixel, a piece whose every cell an earlier one already covers adds next to
// nothing: in a dense network (a city's streets, zoomed out) most pieces, long ones too, and the
// many tiny roads the tile's grid put on the very same point. Pieces the filters hide are left out
// of the coarser levels: kept, one could stand in for the shown roads of its cells and take them
// out of view. Level 0 is always the whole tile, so it stays right whatever the filters; the
// coarser ones are for the filters in `sig`.

import { GPU_EOL, LF_RAIL_SHIFT, LF_TOLL, LF_UNNAMED, LOD_CELLS, SPRITE_MAXZ, TILE_MINZOOM } from '../config';
import { STRIDE, type PieceLevel } from './types';

/** The road filters that hide pieces (as the vertex shader applies them). */
export interface LodFilter {
  classMask: number;
  surfaceMask: number;
  /** Bit 0 toll-free, bit 1 toll. */
  tollMask: number;
  /** Classes (bits) whose unnamed roads are hidden. */
  unnamedHide: number;
  /** Whole-road length range, metres. */
  lenMin: number;
  lenMax: number;
  /** Rail: the service groups shown (bits; pieces carry the groups using the track). */
  railMask?: number;
  /** Rail: the frequency filter's settings ('' off), and the lines it hides (on the main thread:
   * the tile workers don't have the timetables, so lists for it are made there). */
  freqSig?: string;
  lineHidden?: (line: number) => boolean;
}

export const lodSig = (f: LodFilter | null): string =>
  f ? `${f.classMask}|${f.surfaceMask}|${f.tollMask}|${f.unnamedHide}|${f.lenMin}|${f.lenMax}|${f.railMask ?? ''}|${f.freqSig ?? ''}` : '';

/** Cells of a tile's coarser levels: the coarsest tiles get them all (for views zoomed out beyond
 * them), the others those up to 8; none for tiles never drawn as sprites, nor with no filter. */
export function lodCells(z: number, filter: LodFilter | null): number[] {
  if (!filter || z > SPRITE_MAXZ) return [];
  return z === TILE_MINZOOM ? LOD_CELLS : LOD_CELLS.filter((c) => c <= 8);
}

/** Level 0: every piece's first vertex, without the line ends. */
export function levelZero(verts: ArrayBuffer, nverts: number): Uint32Array {
  const u8 = new Uint8Array(verts);
  const all = new Uint32Array(Math.max(0, nverts - 1));
  let n = 0;
  for (let i = 0; i + 1 < nverts; i++) if (!(u8[i * STRIDE + 9] & GPU_EOL)) all[n++] = i;
  return all.subarray(0, n);
}

/** Scratch bit grid (kept between tiles, cleared after use). */
let seenBits = new Uint8Array(0);

/**
 * The lists: level 0 (`all`) and the coarser levels for `cells` and `filter`, in one index array.
 * `bounds`: the first vertex of the roads, of the minor classes and of the tunnels & ferries (the
 * draw groups, see DecodedTile). `lineRoadLen`: per line, the whole road's length (m).
 */
export function pieceLists(verts: ArrayBuffer, all: Uint32Array, bounds: number[], extent: number, cells: number[], filter: LodFilter | null, lineRoadLen: Float32Array): { pieces: Uint32Array<ArrayBuffer>; levels: PieceLevel[] } {
  const u8 = new Uint8Array(verts), i16 = new Int16Array(verts), u32 = new Uint32Array(verts);
  const S2 = STRIDE / 2, S4 = STRIDE / 4;
  const counts = (list: Uint32Array, len: number) => {
    const c = bounds.map(() => 0);
    for (let k = 0; k < bounds.length; k++) {
      let lo = 0, hi = len;
      while (lo < hi) {
        const m = (lo + hi) >> 1;
        if (list[m] < bounds[k]) lo = m + 1;
        else hi = m;
      }
      c[k] = lo;
    }
    return [...c, len] as [number, number, number, number];
  };
  const levels: PieceLevel[] = [{ cell: 0, off: 0, n: counts(all, all.length) }];
  const lists: Uint32Array[] = [all];
  if (cells.length && filter) {
    const f = filter;
    const shown = (i: number) => {
      const st = u8[i * STRIDE + 9], fl = u8[i * STRIDE + 10], cls = st & 15;
      if (!((f.classMask >> cls) & 1) || !((f.surfaceMask >> (st & 16 ? 1 : 0)) & 1)) return false;
      if (f.railMask !== undefined) {
        if (!((fl >> LF_RAIL_SHIFT) & f.railMask)) return false;
        return !f.lineHidden?.(u32[i * S4 + 4]);
      }
      if (!((f.tollMask >> (fl & LF_TOLL ? 1 : 0)) & 1)) return false;
      if (fl & LF_UNNAMED && (f.unnamedHide >> cls) & 1) return false;
      const r = lineRoadLen[u32[i * S4 + 4]];
      return !(r < f.lenMin || r > f.lenMax);
    };
    let prev = all, first = true;
    for (const cell of cells) {
      const side = Math.ceil(extent / cell) + 1;
      const bytes = (side * side + 7) >> 3;
      if (seenBits.length < bytes) seenBits = new Uint8Array(bytes);
      const seen = seenBits;
      const touched: number[] = [];
      const keep = new Uint32Array(prev.length);
      const cellOf = (x: number, y: number) =>
        Math.max(0, Math.min(side - 1, Math.floor(y / cell))) * side + Math.max(0, Math.min(side - 1, Math.floor(x / cell)));
      const path: number[] = [];
      let m = 0;
      for (let k = 0; k < prev.length; k++) {
        const i = prev[k];
        if (first && !shown(i)) continue;
        const x0 = i16[i * S2], y0 = i16[i * S2 + 1], x1 = i16[(i + 1) * S2], y1 = i16[(i + 1) * S2 + 1];
        // The cells along the piece (steps of at most a cell in x and in y); within a cell, the
        // cell of its middle.
        path.length = 0;
        const steps = Math.ceil((Math.abs(x1 - x0) + Math.abs(y1 - y0)) / cell);
        if (steps <= 1) path.push(cellOf((x0 + x1) / 2, (y0 + y1) / 2));
        else {
          let last = -1;
          for (let q = 0; q <= steps; q++) {
            const c = cellOf(x0 + ((x1 - x0) * q) / steps, y0 + ((y1 - y0) * q) / steps);
            if (c !== last) path.push((last = c));
          }
        }
        let fresh = false;
        for (const c of path) if (!(seen[c >> 3] & (1 << (c & 7)))) fresh = true;
        if (!fresh) continue;
        for (const c of path) {
          const b = c >> 3;
          if (!seen[b]) touched.push(b);
          seen[b] |= 1 << (c & 7);
        }
        keep[m++] = i;
      }
      for (const b of touched) seen[b] = 0;
      // Not worth a level unless it leaves out a good share.
      if (m > prev.length * 0.8) continue;
      prev = keep.subarray(0, m);
      first = false;
      levels.push({ cell, off: 0, n: counts(prev, m) });
      lists.push(prev);
    }
  }
  const pieces = new Uint32Array(lists.reduce((a, l) => a + l.length, 0));
  let off = 0;
  lists.forEach((l, k) => {
    pieces.set(l, off);
    levels[k].off = off;
    off += l.length;
  });
  return { pieces, levels };
}
