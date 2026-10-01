// A tile's piece lists for the sprite draw (layer.ts), built in the tile workers and rebuilt on the
// main thread when the road filters change.
//
// Level 0 is every piece (its first vertex), in draw order, without the line ends, which the
// vertex-pair layout would otherwise run the shaders for. Coarser levels (for tiles drawn small:
// zoomed out, far off in tilted views, or zoomed out beyond the coarsest tiles) keep, in draw
// order, only the shown pieces that reach a cell of `cell` tile units no piece kept before them
// reaches (the cells along each piece): in a dense network (a city's streets, zoomed out) most
// pieces, long ones too, and the many tiny roads the tile's grid put on the very same point. Zoomed
// out a road adds its area to a pixel (layer.ts: length × width, under a pixel wide), so a piece
// left out passes its area to the one kept that first reached its cell (`mult`: per vertex, the
// factor on the kept piece's own, log2 × 32), and a view is as bright at every level of detail and
// tile zoom: without it, a city centre drew one street per cell of the dozens there. Each road
// class is thinned apart (the minor ones together), so the area keeps its colour (a cell's dim
// residential streets taken in by the motorway kept there drew it bright). Where in its cell the
// area is drawn doesn't show: the renderer sums what falls in each pixel. Pieces the filters hide
// are left out of the coarser levels: kept, one could stand in for the shown roads of its cells
// and take them out of view. Level 0 is always the whole tile, so it stays right whatever the filters; the
// coarser ones are for the filters in `sig`.

import { GPU_EOL, LF_RAIL_SHIFT, LF_TOLL, LF_UNNAMED, LOD_CELLS, MINOR_MAX_CLASS, NCLASS, SPRITE_MAXZ, TILE_MINZOOM, WIDTHS, WIDTH_Z, interp } from '../config';
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
  // Drawn at one to two times its size, a tile's 8-unit cells are a CSS pixel or less, the coarsest
  // the renderer takes (LOD_CELL_PX): finer levels would never be drawn.
  return z === TILE_MINZOOM ? LOD_CELLS : LOD_CELLS.slice(0, 1);
}

/** Level 0: every piece's first vertex, without the line ends. */
export function levelZero(verts: ArrayBuffer, nverts: number): Uint32Array {
  const u8 = new Uint8Array(verts);
  const all = new Uint32Array(Math.max(0, nverts - 1));
  let n = 0;
  for (let i = 0; i + 1 < nverts; i++) if (!(u8[i * STRIDE + 9] & GPU_EOL)) all[n++] = i;
  return all.subarray(0, n);
}

/** Cell → the kept piece (its place in the level's list) that first reached it: open addressing
 * over typed arrays (a JS Map took ten times as long on the densest tiles). */
class Owners {
  private keys = new Int32Array(1 << 16).fill(-1);
  private vals = new Int32Array(1 << 16);
  private used: number[] = [];
  private mask = (1 << 16) - 1;
  get(c: number): number {
    for (let h = Math.imul(c, 0x9e3779b1) & this.mask; ; h = (h + 1) & this.mask) {
      const k = this.keys[h];
      if (k === c) return this.vals[h];
      if (k < 0) return -1;
    }
  }
  /** Sets the owner of a cell not seen yet; false if it had one. */
  claim(c: number, v: number): boolean {
    if (this.used.length * 2 >= this.mask) this.grow();
    for (let h = Math.imul(c, 0x9e3779b1) & this.mask; ; h = (h + 1) & this.mask) {
      const k = this.keys[h];
      if (k === c) return false;
      if (k < 0) {
        this.keys[h] = c;
        this.vals[h] = v;
        this.used.push(h);
        return true;
      }
    }
  }
  has(c: number): boolean {
    return this.get(c) >= 0;
  }
  private grow() {
    const ks = this.used.map((h) => this.keys[h]), vs = this.used.map((h) => this.vals[h]);
    const size = (this.mask + 1) * 4;
    this.keys = new Int32Array(size).fill(-1);
    this.vals = new Int32Array(size);
    this.mask = size - 1;
    this.used = [];
    for (let i = 0; i < ks.length; i++) this.claim(ks[i], vs[i]);
  }
  clear() {
    for (const h of this.used) this.keys[h] = -1;
    this.used.length = 0;
  }
}
const owners = new Owners();

/**
 * The lists: level 0 (`all`) and the coarser levels for `cells` and `filter`, in one index array.
 * `bounds`: the first vertex of the roads, of the minor classes and of the tunnels & ferries (the
 * draw groups, see DecodedTile). `lineRoadLen`: per line, the whole road's length (m).
 */
export function pieceLists(verts: ArrayBuffer, all: Uint32Array, bounds: number[], extent: number, cells: number[], filter: LodFilter | null, lineRoadLen: Float32Array, z = 8): { pieces: Uint32Array<ArrayBuffer>; levels: PieceLevel[] } {
  const u8 = new Uint8Array(verts), i16 = new Int16Array(verts), u32 = new Uint32Array(verts), f32 = new Float32Array(verts);
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
    // A piece's area in tile units × px of width at about the zoom the tile is drawn at (only the
    // ratios between pieces matter): a dot's merged road length (worker.ts), else its length.
    const wcls = Array.from({ length: NCLASS }, (_, c) => interp(WIDTH_Z, WIDTHS[c], z - 0.5));
    const own = new Float32Array(u8.length / STRIDE);
    for (const i of all) {
      const x0 = i16[i * S2], y0 = i16[i * S2 + 1], x1 = i16[(i + 1) * S2], y1 = i16[(i + 1) * S2 + 1];
      const len = x0 === x1 && y0 === y1 ? f32[i * S4 + 3] : Math.hypot(x1 - x0, y1 - y0);
      own[i] = Math.max(1e-6, len) * wcls[u8[i * STRIDE + 9] & 15];
    }
    // The area each piece of the previous level stands for (its own and what it took in).
    let prev = all, first = true;
    let prevArea = new Float32Array(all.length);
    for (let k = 0; k < all.length; k++) prevArea[k] = own[all[k]];
    for (const cell of cells) {
      const side = Math.ceil(extent / cell) + 1;
      const keep = new Uint32Array(prev.length);
      const keepArea = new Float32Array(prev.length);
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
        // Each class (the minor ones together: their colours are alike in every scheme, and they
        // are most of the pieces) has its own cells (key: cell × 16 + class).
        const cls = u8[i * STRIDE + 9] & 15;
        const sub = cls <= MINOR_MAX_CLASS ? 0 : cls;
        for (let q = 0; q < path.length; q++) path[q] = path[q] * 16 + sub;
        let fresh = false;
        for (const c of path) if (!owners.has(c)) fresh = true;
        if (!fresh) {
          // Its area to the piece kept that first reached its first cell.
          keepArea[owners.get(path[0])] += prevArea[k];
          continue;
        }
        for (const c of path) owners.claim(c, m);
        keepArea[m] = prevArea[k];
        keep[m++] = i;
      }
      owners.clear();
      // Not worth a level unless it leaves out a good share.
      if (m > prev.length * 0.8) continue;
      prev = keep.subarray(0, m);
      prevArea = keepArea.subarray(0, m);
      first = false;
      const mult = new Uint8Array(u8.length / STRIDE);
      for (let q = 0; q < m; q++) mult[prev[q]] = Math.max(0, Math.min(255, Math.round(Math.log2(prevArea[q] / own[prev[q]]) * 32)));
      levels.push({ cell, off: 0, n: counts(prev, m), mult });
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
