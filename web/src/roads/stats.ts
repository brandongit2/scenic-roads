// Aggregate per-cell quantile sketches of the tiles in view into view statistics.
import { CELLS, CLASS_GROUP, EQ, GQ, NCLASS, NSG } from '../config';
import { RoadLayer, type RoadTile } from './layer';

export interface Extreme {
  elev: number;
  lngLat: [number, number];
  tile: RoadTile;
  line: number;
}

export interface ViewStats {
  totalKm: number;
  /** Paved / unpaved km of the enabled classes, regardless of the surface filter. */
  surfaceKm: [number, number];
  classKm: number[];
  /** Merged elevation distribution: `bins` over [lo, hi]. */
  elev: Dist | null;
  grade: Dist | null;
  highest: Extreme | null;
  lowest: Extreme | null;
  complete: boolean;
  cells: number;
}

export class Dist {
  constructor(public lo: number, public hi: number, public bins: Float64Array, public total: number) {}

  /** Value at cumulative fraction p (linear within bins). */
  quantile(p: number): number {
    const target = p * this.total;
    const n = this.bins.length;
    const w = (this.hi - this.lo) / n;
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const b = this.bins[i];
      if (acc + b >= target && b > 0) return this.lo + (i + (target - acc) / b) * w;
      acc += b;
    }
    return this.hi;
  }

  /** Fraction of length at or above v. */
  above(v: number): number {
    const n = this.bins.length;
    const w = (this.hi - this.lo) / n;
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const b0 = this.lo + i * w;
      if (b0 >= v) acc += this.bins[i];
      else if (b0 + w > v) acc += (this.bins[i] * (b0 + w - v)) / w;
    }
    return this.total > 0 ? acc / this.total : 0;
  }

  /** Re-bin into `n` bins over [a, b] (for the legend histogram). */
  rebin(a: number, b: number, n: number): Float64Array {
    const out = new Float64Array(n);
    const m = this.bins.length;
    const w = (this.hi - this.lo) / m;
    for (let i = 0; i < m; i++) {
      if (!this.bins[i]) continue;
      const c = this.lo + (i + 0.5) * w;
      const k = Math.floor(((c - a) / (b - a)) * n);
      if (k >= 0 && k < n) out[k] += this.bins[i];
    }
    return out;
  }
}

const NB = 2048;

/** Spread each sketch's quantile intervals uniformly into a fine histogram. */
function merge(sketches: { q: Float32Array; off: number; w: number }[], nq: number, lo: number, hi: number): Dist | null {
  if (!sketches.length || !(hi > lo)) return null;
  const bins = new Float64Array(NB + 1);
  const diff = new Float64Array(NB + 2);
  const scale = NB / (hi - lo);
  let total = 0;
  for (const s of sketches) {
    const mass = s.w / (nq - 1);
    total += s.w;
    for (let k = 0; k < nq - 1; k++) {
      const a = (s.q[s.off + k] - lo) * scale;
      const b = (s.q[s.off + k + 1] - lo) * scale;
      const ia = Math.min(NB - 1, Math.max(0, Math.floor(a)));
      const ib = Math.min(NB - 1, Math.max(0, Math.floor(b)));
      if (ia === ib || b - a < 1e-9) {
        bins[ia] += mass;
        continue;
      }
      const dens = mass / (b - a);
      bins[ia] += dens * (ia + 1 - a);
      bins[ib] += dens * (b - ib);
      if (ib > ia + 1) {
        diff[ia + 1] += dens;
        diff[ib] -= dens;
      }
    }
  }
  let run = 0;
  const out = new Float64Array(NB);
  for (let i = 0; i < NB; i++) {
    run += diff[i];
    out[i] = bins[i] + run;
  }
  return new Dist(lo, hi, out, total);
}

export function viewStats(layer: RoadLayer, groupMask: number, classMask: number, surfaceMask = 3): ViewStats {
  const tiles = layer.viewTiles();
  const classKm = new Array(NCLASS).fill(0);
  const surfaceKm: [number, number] = [0, 0];
  const eS: { q: Float32Array; off: number; w: number }[] = [];
  const gS: { q: Float32Array; off: number; w: number }[] = [];
  let elo = Infinity, ehi = -Infinity;
  let highest: Extreme | null = null;
  let lowest: Extreme | null = null;
  let cells = 0;
  for (const t of tiles) {
    const d = t.data!;
    const [x0, y0, x1, y1] = layer.viewRectIn(t);
    const c = (v: number) => Math.max(0, Math.min(CELLS - 1, Math.floor((v / d.extent) * CELLS)));
    if (x1 < 0 || y1 < 0 || x0 > d.extent || y0 > d.extent) continue;
    for (let cy = c(y0); cy <= c(y1); cy++) {
      for (let cx = c(x0); cx <= c(x1); cx++) {
        const cell = cy * CELLS + cx;
        cells++;
        for (let k = 0; k < NCLASS; k++) {
          if (!((classMask >> k) & 1)) continue;
          for (let u = 0; u < 2; u++) {
            const km = d.clen[(cell * NCLASS + k) * 2 + u] / 1000;
            surfaceKm[u] += km;
            if ((surfaceMask >> u) & 1) classKm[k] += km;
          }
        }
        for (let sg = 0; sg < NSG; sg++) {
          if (!((groupMask >> (sg >> 1)) & 1) || !((surfaceMask >> (sg & 1)) & 1)) continue;
          const cg = cell * NSG + sg;
          const w = d.glen[cg];
          if (!(w > 0)) continue;
          const lo = d.eq[cg * EQ], hi = d.eq[cg * EQ + EQ - 1];
          if (!Number.isFinite(lo)) continue;
          eS.push({ q: d.eq, off: cg * EQ, w });
          gS.push({ q: d.gq, off: cg * GQ, w });
          if (lo < elo) elo = lo;
          if (hi > ehi) ehi = hi;
          const mx = d.ext[cg * 8], mn = d.ext[cg * 8 + 4];
          if (Number.isFinite(mx) && (!highest || mx > highest.elev)) {
            highest = { elev: mx, lngLat: RoadLayer.tileToLngLat(t, d.ext[cg * 8 + 1], d.ext[cg * 8 + 2]), tile: t, line: d.ext[cg * 8 + 3] };
          }
          if (Number.isFinite(mn) && (!lowest || mn < lowest.elev)) {
            lowest = { elev: mn, lngLat: RoadLayer.tileToLngLat(t, d.ext[cg * 8 + 5], d.ext[cg * 8 + 6]), tile: t, line: d.ext[cg * 8 + 7] };
          }
        }
      }
    }
  }
  const p = layer.progress();
  return {
    totalKm: classKm.reduce((a, b) => a + b, 0),
    surfaceKm,
    classKm,
    elev: merge(eS, EQ, elo, ehi === elo ? elo + 1 : ehi),
    grade: merge(gS, GQ, 0, 40),
    highest,
    lowest,
    complete: p.loaded >= p.wanted,
    cells,
  };
}

export function groupMaskFromClassMask(classMask: number): number {
  let g = 0;
  for (let c = 0; c < NCLASS; c++) if ((classMask >> c) & 1) g |= 1 << CLASS_GROUP[c];
  return g;
}

/** Length-weighted distribution of sampled values over [lo, hi]. */
export function distFromSamples(v: Float32Array, w: Float32Array, lo: number, hi: number, n = 512): Dist | null {
  if (!v.length || !(hi > lo)) return null;
  const bins = new Float64Array(n);
  let total = 0;
  const k = n / (hi - lo);
  for (let i = 0; i < v.length; i++) {
    const b = Math.max(0, Math.min(n - 1, Math.floor((v[i] - lo) * k)));
    bins[b] += w[i];
    total += w[i];
  }
  return new Dist(lo, hi, bins, total);
}
