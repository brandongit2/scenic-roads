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
 * The signed distance to the shore (pixels: + over water, − over land, at most `landPx` deep) of
 * the middle `size` × `size` of an n × n grid of land shares (0 all water … 1 all land; the grid
 * `margin` pixels wider than the tile on each side, so a shore just across its edge counts; the
 * distance at most `margin`).
 *
 * Every pixel holding any land is a shore's seed, and any not wholly land a water's. A pixel's
 * distance is to its nearest seed's centre, plus where that seed's shore lies within it: a pixel
 * of share f along a shore has its edge 0.5 − f from its centre (half land: through it); a seed
 * with no land around it (an island smaller than a pixel) is a disc of its area somewhere inside,
 * on average about 0.2 of a pixel from the centre.
 */
export function signedDistance(frac: Float32Array, n: number, margin: number, landPx: number): Float32Array {
  const size = n - 2 * margin;
  const land = new Uint8Array(n * n), water = new Uint8Array(n * n);
  let nLand = 0, nWater = 0;
  for (let i = 0; i < n * n; i++) {
    if (frac[i] > 0) (land[i] = 1), nLand++;
    if (frac[i] < 1) (water[i] = 1), nWater++;
  }
  const sd = new Float32Array(size * size);
  if (nLand === 0) return sd.fill(margin);
  if (nWater === 0) return sd.fill(-landPx);
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
        d = j < 0 ? margin : Math.min(margin, Math.sqrt(toLand.d2[i]) + landOffset(j));
      } else {
        const j = toWater.at[i];
        d = j < 0 ? -landPx : Math.max(-landPx, -(Math.sqrt(toWater.d2[i]) + frac[j] - 0.5));
      }
      sd[r * size + c] = d;
    }
  }
  return sd;
}
