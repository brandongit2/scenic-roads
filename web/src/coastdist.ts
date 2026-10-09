// The coastal shading's distance to the shore from the water's shares (coast.worker.ts), so that
// zoomed out it is the distance full detail would give: every pixel holding any land at all is
// land somewhere inside it (an island a hundredth of a pixel still has its shore and its glow), and
// a pixel partly land puts the shore inside it by its share.
//
// A pure module (no DOM): `node tools/check/coastdist.mjs` checks it.

const INF = 1e20;

/** For each cell of an n × n grid, the squared distance to the nearest seed and that seed's index
 * (-1 when there is none): Felzenszwalb & Huttenlocher's transform, the columns' nearest seed
 * first, then each row's lower envelope of parabolas. */
export function nearestSeed(seed: Uint8Array, n: number): { d2: Float64Array; at: Int32Array } {
  // Columns: each cell's nearest seed row in its column (two sweeps).
  const rowOf = new Int32Array(n * n).fill(-1);
  for (let c = 0; c < n; c++) {
    let last = -1;
    for (let r = 0; r < n; r++) {
      if (seed[r * n + c]) last = r;
      rowOf[r * n + c] = last;
    }
    last = -1;
    for (let r = n - 1; r >= 0; r--) {
      if (seed[r * n + c]) last = r;
      const up = rowOf[r * n + c];
      if (last >= 0 && (up < 0 || last - r < r - up)) rowOf[r * n + c] = last;
    }
  }
  // Rows: the lower envelope of the columns' parabolas.
  const d2 = new Float64Array(n * n), at = new Int32Array(n * n);
  const f = new Float64Array(n), v = new Int32Array(n), z = new Float64Array(n + 1);
  for (let r = 0; r < n; r++) {
    let any = false;
    for (let c = 0; c < n; c++) {
      const sr = rowOf[r * n + c];
      f[c] = sr < 0 ? INF : (sr - r) * (sr - r);
      if (sr >= 0) any = true;
    }
    if (!any) {
      for (let c = 0; c < n; c++) {
        d2[r * n + c] = INF;
        at[r * n + c] = -1;
      }
      continue;
    }
    let k = 0;
    // (The envelope only over columns that have a seed: INF parabolas never win.)
    let first = 0;
    while (f[first] >= INF) first++;
    v[0] = first;
    z[0] = -INF;
    z[1] = INF;
    for (let q = first + 1; q < n; q++) {
      if (f[q] >= INF) continue;
      let s = (f[q] + q * q - (f[v[k]] + v[k] * v[k])) / (2 * q - 2 * v[k]);
      while (s <= z[k]) {
        k--;
        s = (f[q] + q * q - (f[v[k]] + v[k] * v[k])) / (2 * q - 2 * v[k]);
      }
      k++;
      v[k] = q;
      z[k] = s;
      z[k + 1] = INF;
    }
    k = 0;
    for (let q = 0; q < n; q++) {
      while (z[k + 1] < q) k++;
      const c = v[k];
      d2[r * n + q] = (q - c) * (q - c) + f[c];
      at[r * n + q] = rowOf[r * n + c] * n + c;
    }
  }
  return { d2, at };
}

/**
 * The signed distance to the shore (pixels: + over water, − over land) of the middle `size` ×
 * `size` of an n × n grid of land shares (0 all water … 1 all land; the grid `margin` pixels wider
 * than the tile on each side, so a shore just across its edge counts). Exact as far as `margin`
 * (any nearer shore is inside the grid); beyond it, unknown: +Infinity over water (far, or for
 * `withFarField` to measure from a coarser level), −Infinity over land (deep: the shading's ramp
 * reaches less than a pixel into the land, coast.ts).
 *
 * Every pixel holding any land is a shore's seed, and any not wholly land a water's. A pixel's
 * distance is to its nearest seed's centre, plus where that seed's shore lies within it: a pixel
 * of share f along a shore has its edge 0.5 − f from its centre (half land: through it); a seed
 * with no land around it (an island smaller than a pixel) is a disc of its area somewhere inside,
 * on average about 0.2 of a pixel from the centre.
 */
export function signedDistance(frac: Float32Array, n: number, margin: number): Float32Array {
  const size = n - 2 * margin;
  const land = new Uint8Array(n * n), water = new Uint8Array(n * n);
  let nLand = 0, nWater = 0;
  for (let i = 0; i < n * n; i++) {
    if (frac[i] > 0) (land[i] = 1), nLand++;
    if (frac[i] < 1) (water[i] = 1), nWater++;
  }
  const sd = new Float32Array(size * size);
  if (nLand === 0) return sd.fill(Infinity);
  if (nWater === 0) return sd.fill(-Infinity);
  const toLand = nearestSeed(land, n), toWater = nearestSeed(water, n);
  /** Where a land seed's shore is past its centre, towards the water (pixels). */
  const landOffset = (j: number) => {
    const f = frac[j];
    const r = (j / n) | 0, c = j - r * n;
    let alone = true;
    for (let dy = -1; dy <= 1 && alone; dy++) {
      for (let dx = -1; dx <= 1; dx++) {
        if (!dx && !dy) continue;
        const rr = r + dy, cc = c + dx;
        if (rr >= 0 && cc >= 0 && rr < n && cc < n && land[rr * n + cc]) {
          alone = false;
          break;
        }
      }
    }
    return alone && f < 0.5 ? 0.2 - Math.sqrt(f / Math.PI) : 0.5 - f;
  };
  for (let r = 0; r < size; r++) {
    for (let c = 0; c < size; c++) {
      const i = (r + margin) * n + c + margin;
      let d: number;
      if (frac[i] < 0.5) {
        const j = toLand.at[i];
        d = j < 0 ? Infinity : Math.sqrt(toLand.d2[i]) + landOffset(j);
        if (d > margin) d = Infinity;
      } else {
        const j = toWater.at[i];
        d = j < 0 ? -Infinity : -(Math.sqrt(toWater.d2[i]) + frac[j] - 0.5);
        if (d < -margin) d = -Infinity;
      }
      sd[r * size + c] = d;
    }
  }
  return sd;
}


/**
 * The far field (pixels, in place): `sd`'s water beyond `margin` (+Infinity) or near it, from
 * `coarse`, the same tile's distances `k` levels coarser (its ancestor's `size` × `size`, coarse
 * pixels, ±Infinity beyond its own window). The tile is the ancestor's part from (`cx`, `cy`),
 * `size` / 2^k coarse pixels wide. Toward the poles a tile's pixels hold ever fewer metres, so its
 * window of `margin` pixels falls short of the band's width (coast.worker.ts farLevels); the
 * ancestor's reaches 2^k as far. Within 0.75 … 1 × `margin` the two are blended (the coarse one is
 * good to about a coarse pixel), so no edge shows where the window ends.
 */
export function withFarField(sd: Float32Array, size: number, margin: number, coarse: Float32Array, k: number, cx: number, cy: number) {
  const s = 2 ** k, from = 0.75 * margin;
  for (let r = 0; r < size; r++) {
    const v = cy + (r + 0.5) / s - 0.5;
    for (let c = 0; c < size; c++) {
      const i = r * size + c, d = sd[i];
      if (!(d >= from)) continue;
      const far = s * sample(coarse, size, cx + (c + 0.5) / s - 0.5, v);
      if (d === Infinity) sd[i] = Math.max(margin, far);
      else if (far < Infinity) {
        const t = (d - from) / (margin - from);
        sd[i] = (1 - t) * d + t * Math.max(0, far);
      }
    }
  }
}

/** `g` (n × n) at (x, y), bilinear over its finite values (+Infinity when the nearest is). */
function sample(g: Float32Array, n: number, x: number, y: number): number {
  x = Math.min(n - 1, Math.max(0, x));
  y = Math.min(n - 1, Math.max(0, y));
  const x0 = Math.min(n - 2, Math.floor(x)), y0 = Math.min(n - 2, Math.floor(y)), fx = x - x0, fy = y - y0;
  const near = g[Math.round(y) * n + Math.round(x)];
  if (!Number.isFinite(near)) return near;
  const i = y0 * n + x0, a = g[i], b = g[i + 1], c = g[i + n], d = g[i + n + 1];
  if (Number.isFinite(a) && Number.isFinite(b) && Number.isFinite(c) && Number.isFinite(d)) return (1 - fy) * ((1 - fx) * a + fx * b) + fy * ((1 - fx) * c + fx * d);
  let sum = 0, wsum = 0;
  const add = (v: number, w: number) => {
    if (Number.isFinite(v) && w > 0) (sum += w * v), (wsum += w);
  };
  add(a, (1 - fx) * (1 - fy));
  add(b, fx * (1 - fy));
  add(c, (1 - fx) * fy);
  add(d, fx * fy);
  return wsum > 0 ? sum / wsum : near;
}

/**
 * How many levels coarser the far field of tile z/·/y is measured (0: none): enough that its
 * window in metres is at least half what the same window holds at the equator. A tile's pixels
 * hold cos φ as many metres as the equator's, φ its edge nearer the pole; today's window, half the
 * equator's or more, reaches the band at the view's centre with room to spare (192 px against 28
 * CSS px of 0.35–0.71 px each), so tiles within 60° of the equator need none.
 */
export function farLevels(z: number, y: number): number {
  const n = 2 ** z, edge = y < n / 2 ? y : y + 1;
  const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * edge) / n)));
  const k = Math.ceil(Math.log2(0.5 / Math.cos(lat)) - 1e-9);
  return Math.max(0, Math.min(z, k));
}

// ---- the tiles' encoding ----------------------------------------------------------------------
//
// raster-dem, MapLibre's 'custom' encoding: elevation = R·6553.6 + G·25.6 + B·0.1 − 100 000, the
// signed distance in metres (0.1 m steps), squeezed past 400 km (1/64 as steep) so that the
// widest band zoomed all the way out still fits (a 80-CSS-px band at z1.2 on the equator is 2700
// km). Land floors at −100 km (the ramp needs at most 0.7 CSS px into it, 24 km at z1.2), water
// tops out at about 75 000 km: the land deeper than measured (−Infinity) and the water beyond
// any band (+Infinity) take those ends.

export const COAST_ENCODING = { encoding: 'custom' as const, redFactor: 6553.6, greenFactor: 25.6, blueFactor: 0.1, baseShift: 100000 };
/** Linear as far as this (m); beyond, 1/COAST_SQUEEZE as steep. */
export const COAST_LINEAR = 400000;
export const COAST_SQUEEZE = 64;
const MAX_CODE = 16777215;
const E_MIN = -COAST_ENCODING.baseShift, E_MAX = MAX_CODE / 10 - COAST_ENCODING.baseShift;

/** A signed distance (m) as the stored elevation (the colour ramp's stops likewise). */
export function coastElevation(d: number): number {
  const e = d <= COAST_LINEAR ? d : COAST_LINEAR + (d - COAST_LINEAR) / COAST_SQUEEZE;
  return Math.max(E_MIN, Math.min(E_MAX, e));
}

/** The 24-bit code (R·65536 + G·256 + B) for a signed distance (m). */
export function coastCode(d: number): number {
  return Math.max(0, Math.min(MAX_CODE, Math.round((coastElevation(d) + COAST_ENCODING.baseShift) * 10)));
}
