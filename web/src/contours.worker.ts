// Contour tiles decoded off the main thread, into what the contour layer draws (contours.ts): the
// tile's lines as runs of points ("slots", four int16 each: x and y in units of 8192 a tile,
// elevation in metres, flags | level << 8), clipped to the tile, without the 0 m line, and without
// the closed rings smaller than asked (specks of flat land a hair above an interval).
//
// Each slot's flags say what its neighbours are, for the draw's instanced segments (one per slot,
// reading it and the three after): SEG, a segment from this point to the next slot's; PREV and
// NEXT, the slot before or after is this point's neighbour on the line (the mitres). A closed ring
// comes with its last point before the first and its second after the last, so it is mitred all
// the way round.
import { readLines } from './mvt';

export interface ContourRequest {
  id: number;
  buf: ArrayBuffer;
  /** Closed rings smaller than this across are left out: CSS px at the tile's own zoom (512 a tile). */
  ring: number;
}
export interface ContourResponse {
  id: number;
  /** The slots: one before the first point and two after the last, so that each of the `n`
   * segments' instances reads four. */
  slots: Int16Array;
  n: number;
  /** Rings left out. */
  dropped: number;
}

const SEG = 1, PREV = 2, NEXT = 4;
const UNITS = 8192;

self.onmessage = (ev: MessageEvent<ContourRequest>) => {
  const { id, buf, ring } = ev.data;
  const r = build(buf, ring);
  (self as unknown as Worker).postMessage({ id, ...r } satisfies ContourResponse, [r.slots.buffer]);
};

function build(buf: ArrayBuffer, ringPx: number): Omit<ContourResponse, 'id'> {
  let w = new Int16Array(4 * 4096);
  let k = 1; // slot 0 stands before the first point
  const put = (x: number, y: number, e: number, f: number) => {
    if (4 * (k + 3) > w.length) {
      const g = new Int16Array(w.length * 2);
      g.set(w);
      w = g;
    }
    const o = 4 * k++;
    w[o] = x;
    w[o + 1] = y;
    w[o + 2] = e;
    w[o + 3] = f;
  };
  let dropped = 0;
  const layer = readLines(buf, 'contours');
  if (layer) {
    const E = layer.extent, s = UNITS / E;
    const ringMin = (ringPx * E) / 512;
    // A run in the slots' units, without the points rounding made repeats.
    const units = (run: number[]) => {
      const out: number[] = [];
      for (let i = 0; i + 1 < run.length; i += 2) {
        const x = Math.round(run[i] * s), y = Math.round(run[i + 1] * s), n = out.length;
        if (n && out[n - 2] === x && out[n - 1] === y) continue;
        out.push(x, y);
      }
      return out;
    };
    for (const f of layer.lines) {
      const ele = Math.round(Number(f.props.ele));
      // No 0 m line: the sea and the land below it are 0 (terrain.rs), so it only traced every
      // shore and pier, and wandered round each rise in a plain below sea level (Nagoya).
      if (!(ele > 0) || ele > 32767) continue;
      const lv = (Number(f.props.level) > 0 ? 1 : 0) << 8;
      const open = (run: number[]) => {
        const p = units(run), n = p.length / 2;
        if (n < 2) return;
        for (let i = 0; i < n; i++) put(p[2 * i], p[2 * i + 1], ele, (i < n - 1 ? SEG | NEXT : 0) | (i > 0 ? PREV : 0) | lv);
      };
      for (const run of f.runs) {
        const n = run.length / 2;
        if (n >= 4 && run[0] === run[2 * n - 2] && run[1] === run[2 * n - 1]) {
          let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
          for (let i = 0; i < run.length; i += 2) {
            x0 = Math.min(x0, run[i]);
            x1 = Math.max(x1, run[i]);
            y0 = Math.min(y0, run[i + 1]);
            y1 = Math.max(y1, run[i + 1]);
          }
          if (Math.max(x1 - x0, y1 - y0) < ringMin) {
            dropped++;
            continue;
          }
          if (x0 >= 0 && y0 >= 0 && x1 <= E && y1 <= E) {
            // Wholly in the tile: mitred all the way round.
            const p = units(run), m = p.length / 2 - 1; // its points, the first not repeated
            if (m >= 3 && p[0] === p[2 * m] && p[1] === p[2 * m + 1]) {
              put(p[2 * m - 2], p[2 * m - 1], ele, NEXT | lv);
              for (let i = 0; i < m; i++) put(p[2 * i], p[2 * i + 1], ele, SEG | PREV | NEXT | lv);
              put(p[0], p[1], ele, PREV | NEXT | lv);
              put(p[2], p[3], ele, PREV | lv);
              continue;
            }
          }
        }
        clip(run, E, open);
      }
    }
  }
  put(0, 0, 0, 0);
  put(0, 0, 0, 0);
  return { slots: w.slice(0, 4 * k), n: k - 3, dropped };
}

/** The parts of a run inside the tile (0 … E), each to `out`: Liang–Barsky per segment. The
 * lines reach a little beyond the tile (the tile's buffer); cut at its edge, they meet the
 * neighbouring tile's end to end. */
function clip(run: number[], E: number, out: (part: number[]) => void) {
  let part: number[] = [];
  for (let i = 0; i + 3 < run.length; i += 2) {
    const x0 = run[i], y0 = run[i + 1], dx = run[i + 2] - x0, dy = run[i + 3] - y0;
    let t0 = 0, t1 = 1, ok = true;
    for (const [p, q] of [[-dx, x0], [dx, E - x0], [-dy, y0], [dy, E - y0]]) {
      if (p === 0) {
        if (q < 0) ok = false;
      } else {
        const r = q / p;
        if (p < 0) {
          if (r > t1) ok = false;
          else if (r > t0) t0 = r;
        } else if (r < t0) ok = false;
        else if (r < t1) t1 = r;
      }
    }
    if (!ok) {
      if (part.length >= 4) out(part);
      part = [];
      continue;
    }
    if (t0 > 0 || !part.length) {
      if (part.length >= 4) out(part);
      part = [x0 + t0 * dx, y0 + t0 * dy];
    }
    part.push(x0 + t1 * dx, y0 + t1 * dy);
    if (t1 < 1) {
      out(part);
      part = [];
    }
  }
  if (part.length >= 4) out(part);
}
