// The field's roof ground read at coarser levels (task #102 B5), against web/src/bldfield.offsets.ts:
//  1. a small building's thin tail alone on a slope: its roof ground premultiplied by the share,
//     mipmapped as the GPU does (box means) and divided by the share read there, against filtered as
//     is: the roof's ground each level finds against its centroid's;
//  2. no run starts below a roof: a synthetic tile (random buildings, some across its edges, offsets
//     rounded and clamped as the builder's, shares rounded to R11F's 5-bit mantissa at every level),
//     reads at random points and along the tile's edges at levels 0–4, bilinear, clamped to the edge,
//     as the march makes them: each roof (its top plus its roof ground, clamped under the cell's
//     start) never above the start (its cell's tallest top plus largest roof ground). And, unclamped,
//     a level-0 read never above it either (the cells' bound is right where the march reads finest);
//     coarser reads need the clamp, counted.
// `--mutate nodilate` leaves the texel around a cell out of its largest roof ground: level-0 reads
// then come out above, and the check fails. Exits 1 on a failure.
//   node tools/bldcheck/offmip.mjs [--mutate nodilate]
import { roofGround, groundAt, shareOf, boxMip } from '../../web/src/bldfield.offsets.ts';

const mutate = process.argv.includes('--mutate') ? process.argv[process.argv.indexOf('--mutate') + 1] : '';
let failed = 0;
const roundAway = (d) => Math.sign(d) * Math.round(Math.abs(d));
const mant = (x) => (x > 0 ? (() => { const q = 2 ** (Math.floor(Math.log2(x)) - 5); return Math.round(x / q) * q; })() : 0);
const code = (f) => Math.round(Math.sqrt(f) * 255);

// 1. The tail.
{
  const T = 64, N = T * T, M = 8, C = T / 16, GE = C + 1 + 2 * M;
  // (ground rising 8 m a texel east: a cell of 16 texels 128 m)
  const ee = new Float32Array(GE * GE).map((_, k) => 128 * ((k % GE) - M));
  const g = (x, y) => groundAt(ee, GE, M, x, y);
  // (a thin tail in texel x 20 of row 20, a share of 0.1, its body in x 24–31, a share of 1)
  const texels = new Uint8Array(N * 3);
  const y0 = 20, parts = [[20, 0.1], ...Array.from({ length: 8 }, (_, j) => [24 + j, 1])];
  const cx = parts.reduce((s, [x, w]) => s + w * (x + 0.5), 0) / parts.reduce((s, [, w]) => s + w, 0);
  for (const [x, w] of parts) {
    const i = y0 * T + x;
    texels[i * 3] = code(w);
    texels[i * 3 + 1] = 128 + roundAway(cx - (x + 0.5));
    texels[i * 3 + 2] = 128;
  }
  const { dg } = roofGround(texels, 1, T, ee, GE, M);
  const f = new Float32Array(N), raw = new Float32Array(N);
  for (let i = 0; i < N; i++) {
    f[i] = shareOf(texels[i * 3]);
    raw[i] = f[i] ? dg[i] / f[i] : 0;
  }
  let fs = f, ps = dg, rs = raw, size = T;
  console.log('1. a tail on a slope (8 m a texel): level, the centroid\'s ground over the read\'s, premultiplied, filtered as is');
  for (let L = 0; L <= 4; L++) {
    const tx = 20 >> L, k = (y0 >> L) * size + tx, at = (tx + 0.5) * 2 ** L;
    const truth = g(cx, y0 + 0.5) - g(at, y0 + 0.5);
    const pre = fs[k] > 0 ? ps[k] / mant(fs[k]) : 0;
    // (premultiplied: within the slope over half the read's texel, plus the share's rounding)
    if (Math.abs(pre - truth) > 8 * Math.max(0.5, 2 ** (L - 1)) + 0.04 * Math.abs(truth) + 1e-6) failed++;
    console.log(`   ${L}  ${truth.toFixed(1)}  ${pre.toFixed(1)}  ${rs[k].toFixed(1)}`);
    fs = boxMip(fs, size, 1);
    ps = boxMip(ps, size, 1);
    rs = boxMip(rs, size, 1);
    size >>= 1;
  }
}

// 2. No run below a roof.
{
  let seed = 12345;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2 ** 31) / 2 ** 31);
  const T = 128, N = T * T, C = T / 16, M = 8, GE = C + 1 + 2 * M;
  // (a slope, 30 m a cell east and 20 north, with 40 m of noise)
  const ee = new Float32Array(GE * GE).map((_, k) => 30 * (k % GE) + 20 * Math.floor(k / GE) + rnd() * 40);
  // (buildings: rectangles 1–40 texels a side, some across the tile's edges, each texel's share
  // random, a height each; their centroids from their whole rectangles)
  const texels = new Uint8Array(N * 3), h = new Float32Array(N);
  for (let b = 0; b < 120; b++) {
    const w = 1 + Math.floor(rnd() * (rnd() < 0.2 ? 40 : 8)), hh = 1 + Math.floor(rnd() * (rnd() < 0.2 ? 40 : 8));
    const x0 = Math.floor(rnd() * (T + 20)) - 10, y0 = Math.floor(rnd() * (T + 20)) - 10, top = 3 + rnd() * 60;
    const sh = [];
    let sw = 0, sx = 0, sy = 0;
    for (let y = y0; y < y0 + hh; y++) for (let x = x0; x < x0 + w; x++) {
      const s = 0.05 + rnd() * 0.95;
      sh.push([x, y, s]);
      sw += s; sx += s * (x + 0.5); sy += s * (y + 0.5);
    }
    const cx = sx / sw, cy = sy / sw;
    for (const [x, y, s] of sh) {
      if (x < 0 || y < 0 || x >= T || y >= T) continue;
      const i = y * T + x;
      texels[i * 3] = code(s);
      texels[i * 3 + 1] = 128 + Math.max(-127, Math.min(127, roundAway(cx - (x + 0.5))));
      texels[i * 3 + 2] = 128 + Math.max(-127, Math.min(127, roundAway(cy - (y + 0.5))));
      h[i] = top;
    }
  }
  const { dg, dgc } = roofGround(texels, 1, T, ee, GE, M);
  if (mutate === 'nodilate') {
    // (each cell's largest roof ground over its own texels alone)
    dgc.fill(0);
    for (let i = 0; i < N; i++) {
      const f = shareOf(texels[i * 3]);
      if (!f) continue;
      const k = (((i / T) | 0) >> 4) * C + ((i % T) >> 4);
      dgc[k] = Math.max(dgc[k], dg[i] / f);
    }
  }
  // (each cell's tallest top, its texels' and the texel around them, as the worker's pool)
  const top = new Float32Array(C * C);
  for (let y = 0; y < T; y++) for (let x = 0; x < T; x++) {
    if (!shareOf(texels[(y * T + x) * 3])) continue;
    for (let cy = Math.max(0, (y - 1) >> 4); cy <= Math.min(C - 1, (y + 1) >> 4); cy++)
      for (let cx = Math.max(0, (x - 1) >> 4); cx <= Math.min(C - 1, (x + 1) >> 4); cx++) top[cy * C + cx] = Math.max(top[cy * C + cx], h[y * T + x]);
  }
  // (the levels as the GPU holds them: shares, share × top, share × roof ground, box means; the
  // shares rounded to R11F's mantissa at every level)
  const f0 = new Float32Array(N), v0 = new Float32Array(N);
  for (let i = 0; i < N; i++) {
    f0[i] = shareOf(texels[i * 3]);
    v0[i] = f0[i] * h[i];
  }
  const levels = [];
  let fs = f0, vs = v0, ps = dg, size = T;
  for (let L = 0; L <= 4; L++) {
    levels.push({ fs: fs.map(mant), vs, ps, size });
    fs = boxMip(fs, size, 1);
    vs = boxMip(vs, size, 1);
    ps = boxMip(ps, size, 1);
    size >>= 1;
  }
  const bilinear = (a, size, u, v) => {
    const x = u - 0.5, y = v - 0.5, i = Math.floor(x), j = Math.floor(y), fx = x - i, fy = y - j;
    const at = (ii, jj) => a[Math.min(size - 1, Math.max(0, jj)) * size + Math.min(size - 1, Math.max(0, ii))];
    return (at(i, j) * (1 - fx) + at(i + 1, j) * fx) * (1 - fy) + (at(i, j + 1) * (1 - fx) + at(i + 1, j + 1) * fx) * fy;
  };
  const points = [];
  for (let n = 0; n < 40000; n++) points.push([rnd() * T, rnd() * T]);
  for (let n = 0; n < 4000; n++) {
    const s = rnd() * T, e = rnd() * 0.5;
    points.push([e, s], [T - e, s], [s, e], [s, T - e]);
  }
  let reads = 0, above = 0;
  const unclamped = [0, 0, 0, 0, 0], perLevel = [0, 0, 0, 0, 0];
  for (let L = 0; L <= 4; L++) {
    const { fs, vs, ps, size } = levels[L], k2 = 2 ** L;
    for (const [px, py] of points) {
      const u = px / k2, v = py / k2, share = bilinear(fs, size, u, v);
      if (share < 1e-5) continue;
      const cell = Math.min(C - 1, Math.floor(py / 16)) * C + Math.min(C - 1, Math.floor(px / 16));
      const start = top[cell] + dgc[cell];
      const H0 = Math.max(bilinear(vs, size, u, v) / share, 0.5);
      const raw = H0 + bilinear(ps, size, u, v) / share;
      const roof = Math.min(Math.max(raw, 0.5), Math.max(start, 0.5));
      reads++;
      perLevel[L]++;
      if (roof > start + 1e-3 && start > 0.5) above++;
      if (raw > start + 1e-3) unclamped[L]++;
    }
  }
  console.log(`2. no run below a roof: ${reads} reads at levels 0–4 (${points.length} points, edges included): ${above} roofs above their cell's start`);
  console.log(`   unclamped reads above it, by level: ${unclamped.map((u, L) => `${L}: ${u} of ${perLevel[L]}`).join(', ')}`);
  if (above || unclamped[0]) failed++;
}
if (failed) {
  console.log(`${failed} failed`);
  process.exit(1);
}
console.log('ok');
