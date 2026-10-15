// The field's offsets read at coarser levels (task #102 B5's review): a small building alone on a
// slope, its offsets to its centroid as field5-webp stores them, mipmapped as the GPU does (box
// means) and read at each level as the march does: premultiplied by the share and divided by the
// share read there (web/src/bldfield.offsets.ts), against the offsets filtered as stored. Prints the
// ground at the centroid each read finds against the ground at the true one, and fails (exit 1) when
// the premultiplied read errs by more than the slope over half the read's texel.
//   node tools/bldcheck/offmip.mjs
import { premultiplied, reachOf, boxMip, REACH_PAD } from '../../web/src/bldfield.offsets.ts';

const T = 64, N = T * T;
const slope = 0.5; // m of ground a level-0 texel, rising with x
const ground = (x) => slope * x;
// A building on row 20: a thin tail in texel x 20 (a share of 0.1) and its body in x 24–31 (a share
// of 1 each), so its centroid is at x ≈ 27.9 and the tail's texel stores an offset of 7 texels
// (round(centroid − (x + 0.5)), half away from zero as the builder rounds). The tail's texel has
// only empty texels around it until level 3, where the body joins it.
const f = new Float32Array(N), raw = new Uint8Array(N * 2).fill(128);
const y0 = 20, parts = [[20, 0.1], ...Array.from({ length: 8 }, (_, j) => [24 + j, 1])];
const cx = parts.reduce((s, [x, w]) => s + w * (x + 0.5), 0) / parts.reduce((s, [, w]) => s + w, 0);
for (const [x, w] of parts) {
  const i = y0 * T + x, d = cx - (x + 0.5);
  f[i] = w;
  raw[i * 2] = 128 + Math.sign(d) * Math.round(Math.abs(d));
}
const pre = premultiplied(raw, [f], T);
const old = new Float32Array(N * 2);
for (let i = 0; i < N * 2; i++) old[i] = raw[i] - 128;
let fs = f, ps = pre, os = old, size = T, bad = 0;
console.log('level  texel  true ground  premultiplied: ground, error   as stored: ground, error');
for (let L = 0; size >= 1; L++) {
  // (the level's texel holding the tail, read at its centre)
  const tx = 20 >> L, ty = y0 >> L, k = ty * size + tx;
  const at = (tx + 0.5) * 2 ** L; // its centre in level-0 texels (x)
  const share = fs[k];
  const odNew = share > 0 ? ps[k * 2] / share : 0;
  const odOld = os[k * 2];
  const gTrue = ground(cx), gNew = ground(at + odNew), gOld = ground(at + odOld);
  const tol = slope * Math.max(0.5, 2 ** (L - 1)) + 1e-6;
  if (Math.abs(gNew - gTrue) > tol) bad++;
  console.log(`${String(L).padStart(5)}  ${String(tx).padStart(5)}  ${gTrue.toFixed(2).padStart(11)}  ${gNew.toFixed(2).padStart(13)}, ${(gNew - gTrue).toFixed(2).padStart(5)}   ${gOld.toFixed(2).padStart(9)}, ${(gOld - gTrue).toFixed(2).padStart(5)}`);
  if (size === 1) break;
  fs = boxMip(fs, size, 1);
  ps = boxMip(ps, size, 2);
  os = boxMip(os, size, 2);
  size >>= 1;
}
const reach = reachOf(raw, 1, T);
console.log(`reach of the tail's cell: ${reach[(y0 >> 4) * (T / 16) + (20 >> 4)]} cells (the largest offset there and in its neighbours, rounded up to cells, + ${REACH_PAD})`);
if (bad) {
  console.log(`${bad} levels err by more than the slope over half their texel`);
  process.exit(1);
}
console.log('ok');
