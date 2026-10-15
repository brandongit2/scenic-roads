// The field's offsets read at coarser levels (task #102 B5's review), against web/src/bldfield.offsets.ts:
//  1. a small building's thin tail alone on a slope, its offsets as field5-webp stores them, mipmapped
//     as the GPU does (box means) and read at each level as the march does (premultiplied by the
//     share, divided by the share read there), against the offsets filtered as stored: the ground at
//     the centroid each read finds;
//  2. no run starts below a roof: a synthetic tile (random buildings, some across its edges, offsets
//     rounded and clamped as the builder's, shares rounded to R11F's 5-bit mantissa at every level),
//     reads at random points (and at the tile's edges, clamped as the GPU clamps) at levels 0–4,
//     bilinear: the ground at each read's centroid never above the read's cell's box's highest
//     ground (the near box to level 2, the far one to 4), which is what the march adds to the start
//     of the cell's runs.
// Exits 1 on a failure.
//   node tools/bldcheck/offmip.mjs
import { premultiplied, centroidBoxes, boxGroundMax, boxMip, NEAR_LEVEL, FAR_LEVEL } from '../../web/src/bldfield.offsets.ts';

let failed = 0;
const toMantissa = (x, bits) => {
  if (!(x > 0)) return 0;
  const q = 2 ** (Math.floor(Math.log2(x)) - bits);
  return Math.round(x / q) * q;
};
const roundAway = (d) => Math.sign(d) * Math.round(Math.abs(d));

// 1. The tail.
{
  const T = 64, N = T * T, slope = 0.5;
  const ground = (x) => slope * x;
  // (a thin tail in texel x 20 of row 20, a share of 0.1, its body in x 24–31, a share of 1: the
  // centroid at x ≈ 27.9, the tail's offset 7 texels)
  const f = new Float32Array(N), raw = new Uint8Array(N * 2).fill(128);
  const y0 = 20, parts = [[20, 0.1], ...Array.from({ length: 8 }, (_, j) => [24 + j, 1])];
  const cx = parts.reduce((s, [x, w]) => s + w * (x + 0.5), 0) / parts.reduce((s, [, w]) => s + w, 0);
  for (const [x, w] of parts) {
    const i = y0 * T + x;
    f[i] = w;
    raw[i * 2] = 128 + roundAway(cx - (x + 0.5));
  }
  const pre = premultiplied(raw, [f], T);
  const old = new Float32Array(N * 2);
  for (let i = 0; i < N * 2; i++) old[i] = raw[i] - 128;
  let fs = f, ps = pre, os = old, size = T;
  console.log('1. a tail on a slope: level, true ground, premultiplied (ground, error), as stored (ground, error)');
  for (let L = 0; L <= FAR_LEVEL; L++) {
    const tx = 20 >> L, k = (y0 >> L) * size + tx, at = (tx + 0.5) * 2 ** L;
    const gNew = ground(at + (fs[k] > 0 ? ps[k * 2] / fs[k] : 0)), gOld = ground(at + os[k * 2]);
    const tol = slope * Math.max(0.5, 2 ** (L - 1)) + 1e-6;
    if (Math.abs(gNew - ground(cx)) > tol) failed++;
    console.log(`   ${L}  ${ground(cx).toFixed(2)}  ${gNew.toFixed(2)} ${(gNew - ground(cx)).toFixed(2)}  ${gOld.toFixed(2)} ${(gOld - ground(cx)).toFixed(2)}`);
    fs = boxMip(fs, size, 1);
    ps = boxMip(ps, size, 2);
    os = boxMip(os, size, 2);
    size >>= 1;
  }
}

// 2. No run starts below a roof.
{
  let seed = 12345;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2 ** 31) / 2 ** 31);
  const T = 128, N = T * T, C = T / 16, M = 8, GE = C + 1 + 2 * M;
  // (a slope, 30 m a cell east and 20 north (a cell 16 texels: ~1 in 3 at z14), and 5 m of noise:
  // steep enough that a box a few texels short shows, smooth enough that the boxes' ground is tight)
  const ee = new Float32Array(GE * GE).map((_, k) => 30 * (k % GE) + 20 * Math.floor(k / GE) + rnd() * 5);
  const ground = (px, py) => {
    const gx = Math.min(Math.max(px / 16 + M, 0), GE - 1 - 0.001), gy = Math.min(Math.max(py / 16 + M, 0), GE - 1 - 0.001);
    const i = Math.floor(gx), j = Math.floor(gy), fx = gx - i, fy = gy - j;
    const a = ee[j * GE + i], b = ee[j * GE + i + 1], c = ee[(j + 1) * GE + i], d = ee[(j + 1) * GE + i + 1];
    return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy;
  };
  // (buildings: rectangles 1–40 texels a side, some across the tile's edges, each texel's share
  // random; their centroids from their whole rectangles, so an edge's offsets point out of the tile)
  const f = new Float32Array(N), raw = new Uint8Array(N * 2).fill(128);
  for (let b = 0; b < 120; b++) {
    const w = 1 + Math.floor(rnd() * (rnd() < 0.2 ? 40 : 8)), h = 1 + Math.floor(rnd() * (rnd() < 0.2 ? 40 : 8));
    const x0 = Math.floor(rnd() * (T + 20)) - 10, y0 = Math.floor(rnd() * (T + 20)) - 10;
    const sh = [];
    let sw = 0, sx = 0, sy = 0;
    for (let y = y0; y < y0 + h; y++) for (let x = x0; x < x0 + w; x++) {
      const s = 0.05 + rnd() * 0.95;
      sh.push([x, y, s]);
      sw += s; sx += s * (x + 0.5); sy += s * (y + 0.5);
    }
    const cx = sx / sw, cy = sy / sw;
    for (const [x, y, s] of sh) {
      if (x < 0 || y < 0 || x >= T || y >= T) continue;
      const i = y * T + x;
      f[i] = toMantissa(s, 5);
      raw[i * 2] = 128 + Math.max(-127, Math.min(127, roundAway(cx - (x + 0.5))));
      raw[i * 2 + 1] = 128 + Math.max(-127, Math.min(127, roundAway(cy - (y + 0.5))));
    }
  }
  const { near, far } = centroidBoxes(raw, [f], T);
  const own = (x, y) => Math.max(ee[(y + M) * GE + x + M], ee[(y + M) * GE + x + M + 1], ee[(y + M + 1) * GE + x + M], ee[(y + M + 1) * GE + x + M + 1]);
  const hiNear = [], hiFar = [];
  for (let y = 0; y < C; y++) for (let x = 0; x < C; x++) {
    const k = y * C + x;
    hiNear[k] = boxGroundMax(near, k, ee, GE, M, own(x, y));
    hiFar[k] = Math.max(hiNear[k], boxGroundMax(far, k, ee, GE, M, own(x, y)));
  }
  // (the levels as the GPU holds them: share × offset box means; the shares' means rounded to
  // R11F's mantissa at every level, as the class texture's mipmaps are)
  const levels = [];
  let fs = f, ps = premultiplied(raw, [f], T), size = T;
  for (let L = 0; L <= FAR_LEVEL; L++) {
    levels.push({ fs: fs.map((v) => toMantissa(v, 5)), ps, size });
    fs = boxMip(fs, size, 1);
    ps = boxMip(ps, size, 2);
    size >>= 1;
  }
  const bilinear = (a, comps, size, u, v, c) => {
    // (clamped to the edge, as CLAMP_TO_EDGE)
    const x = u - 0.5, y = v - 0.5, i = Math.floor(x), j = Math.floor(y), fx = x - i, fy = y - j;
    const at = (ii, jj) => a[(Math.min(size - 1, Math.max(0, jj)) * size + Math.min(size - 1, Math.max(0, ii))) * comps + c];
    return (at(i, j) * (1 - fx) + at(i + 1, j) * fx) * (1 - fy) + (at(i, j + 1) * (1 - fx) + at(i + 1, j + 1) * fx) * fy;
  };
  let reads = 0, bad = 0;
  const worst = [-Infinity, -Infinity];
  const points = [];
  for (let n = 0; n < 40000; n++) points.push([rnd() * T, rnd() * T]);
  // (and along the tile's edges)
  for (let n = 0; n < 4000; n++) {
    const s = rnd() * T, e = rnd() * 0.5;
    points.push([e, s], [T - e, s], [s, e], [s, T - e]);
  }
  for (let L = 0; L <= FAR_LEVEL; L++) {
    const { fs, ps, size } = levels[L], k2 = 2 ** L;
    for (const [px, py] of points) {
      const u = px / k2, v = py / k2;
      const share = bilinear(fs, 1, size, u, v, 0);
      if (share < 1e-5) continue;
      const ex = px + bilinear(ps, 2, size, u, v, 0) / share, ey = py + bilinear(ps, 2, size, u, v, 1) / share;
      const cell = Math.min(C - 1, Math.floor(py / 16)) * C + Math.min(C - 1, Math.floor(px / 16));
      const hi = L <= NEAR_LEVEL ? hiNear[cell] : hiFar[cell];
      const over = ground(ex, ey) - hi;
      reads++;
      worst[L <= NEAR_LEVEL ? 0 : 1] = Math.max(worst[L <= NEAR_LEVEL ? 0 : 1], over);
      if (over > 1e-3) bad++;
    }
  }
  console.log(`2. no run below a roof: ${reads} reads at levels 0–${FAR_LEVEL} (${points.length} points, edges included): a roof's ground at least ${(-worst[0]).toFixed(1)} m (levels 0–${NEAR_LEVEL}) and ${(-worst[1]).toFixed(1)} m (to ${FAR_LEVEL}) under the cell's start, ${bad} above it`);
  if (bad) failed++;
}
if (failed) {
  console.log(`${failed} failed`);
  process.exit(1);
}
console.log('ok');
