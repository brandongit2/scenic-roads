// The coastal shading's distance to the shore (web/src/coastdist.ts) against brute force, and its
// shores where full detail puts them.
//   node tools/check/coastdist.mjs
import assert from 'node:assert/strict';
import { COAST_ENCODING, COAST_LINEAR, coastCode, coastElevation, farLevels, nearestSeed, signedDistance, withFarField } from '../../web/src/coastdist.ts';

// The nearest seed: against brute force on random grids.
for (let t = 0; t < 20; t++) {
  const n = 23;
  const seed = new Uint8Array(n * n);
  for (let i = 0; i < n * n; i++) seed[i] = Math.random() < (t % 4) * 0.02 ? 1 : 0;
  const { d2, at } = nearestSeed(seed, n);
  for (let i = 0; i < n * n; i++) {
    let best = Infinity;
    for (let j = 0; j < n * n; j++) if (seed[j]) best = Math.min(best, ((i % n) - (j % n)) ** 2 + (((i / n) | 0) - ((j / n) | 0)) ** 2);
    if (best === Infinity) assert.equal(at[i], -1);
    else {
      assert.equal(d2[i], best);
      assert.equal(((i % n) - (at[i] % n)) ** 2 + (((i / n) | 0) - ((at[i] / n) | 0)) ** 2, best);
      assert.ok(seed[at[i]]);
    }
  }
}

const M = 8, n = 40;
const at = (sd, r, c) => sd[(r - M) * (n - 2 * M) + c - M];

// A straight shore at x = 20.3 (land to its left): each pixel's distance to it within a tenth.
{
  const frac = new Float32Array(n * n);
  for (let r = 0; r < n; r++) for (let c = 0; c < n; c++) frac[r * n + c] = Math.min(1, Math.max(0, 20.3 - c));
  const sd = signedDistance(frac, n, M);
  for (let c = 13; c < 28; c++) {
    const want = c + 0.5 - 20.3;
    assert.ok(Math.abs(at(sd, 20, c) - want) < 0.1, `x ${c}: ${at(sd, 20, c)} against ${want}`);
  }
}

// An island a hundredth of a pixel: still land, its glow measured from it.
{
  const frac = new Float32Array(n * n);
  frac[20 * n + 20] = 0.01;
  const sd = signedDistance(frac, n, M);
  assert.ok(at(sd, 20, 20) < 0.3 && at(sd, 20, 20) > -0.1, `the island's own pixel: ${at(sd, 20, 20)}`);
  for (const [r, c] of [[20, 25], [24, 20], [17, 17]]) {
    const want = Math.hypot(r - 20, c - 20);
    assert.ok(Math.abs(at(sd, r, c) - want) < 0.5, `${r},${c}: ${at(sd, r, c)} against ${want}`);
  }
  // No land at all: far (beyond the window); all land: deep.
  assert.ok(signedDistance(new Float32Array(n * n), n, M).every((v) => v === Infinity));
  assert.ok(signedDistance(new Float32Array(n * n).fill(1), n, M).every((v) => v === -Infinity));
}

// The window: exact as far as the margin, then far over water and deep over land (a shore at x =
// 20.3 again, on a grid wide enough to see past it).
{
  const n2 = 60, M2 = 8;
  const frac = new Float32Array(n2 * n2);
  for (let r = 0; r < n2; r++) for (let c = 0; c < n2; c++) frac[r * n2 + c] = Math.min(1, Math.max(0, 20.3 - c));
  const sd = signedDistance(frac, n2, M2), size = n2 - 2 * M2;
  for (let c = M2; c < n2 - M2; c++) {
    const v = sd[(30 - M2) * size + c - M2], want = c + 0.5 - 20.3;
    if (Math.abs(want) <= M2 - 0.1) assert.ok(Math.abs(v - want) < 0.1, `x ${c}: ${v} against ${want}`);
    else if (Math.abs(want) > M2 + 0.1) assert.equal(v, want > 0 ? Infinity : -Infinity, `x ${c}`);
  }
}

// The far field: a fine tile whose window ends short of the band takes the water beyond from its
// ancestor; continuous where the two meet, and equal to the true distance within a coarse pixel.
{
  // A shore along y = 0 of the ancestor's grid: true distance (fine px) for fine row r is r + 0.5
  // + the fine tile's offset. Coarse (k = 2, size 64): its row v at distance v + 0.5 coarse px.
  const size = 64, k = 2, s = 4, Mf = 16;
  const coarse = new Float32Array(size * size);
  for (let v = 0; v < size; v++) for (let u = 0; u < size; u++) coarse[v * size + u] = v + 0.5 > 40 ? Infinity : v + 0.5;
  // The fine tile is the ancestor's part from (0, 16): its row r lies (16·4 + r + 0.5) fine px out.
  const fine = new Float32Array(size * size);
  for (let r = 0; r < size; r++) for (let c = 0; c < size; c++) {
    const d = 64 + r + 0.5;
    fine[r * size + c] = d > Mf ? Infinity : d;
  }
  withFarField(fine, size, Mf, coarse, k, 0, 16);
  for (let r = 0; r < size; r++) {
    const want = 64 + r + 0.5, got = fine[r * size + 5];
    if (want / s + 0.5 < 40) assert.ok(Math.abs(got - want) <= s, `row ${r}: ${got} against ${want}`);
  }
  // Blended near the window's end: a fine value of 0.9 M meets a coarse estimate 2 px off.
  const one = new Float32Array(size * size).fill(0.9 * Mf);
  const flat = new Float32Array(size * size).fill((0.9 * Mf + 2) / s);
  withFarField(one, size, Mf, flat, k, 0, 0);
  assert.ok(Math.abs(one[0] - (0.9 * Mf + 1.2)) < 1e-4, `blend: ${one[0]}`);
  // Never nearer than the window for water beyond it.
  const beyond = new Float32Array(size * size).fill(Infinity), near = new Float32Array(size * size).fill(1);
  withFarField(beyond, size, Mf, near, k, 0, 0);
  assert.ok(beyond.every((v) => v === Mf));
}

// The levels: none within 60°, then enough that cos φ · 2^k ≥ 0.5 at the tile's poleward edge.
{
  for (let z = 0; z <= 14; z++) {
    const n3 = 2 ** z;
    for (const y of [0, 1, (n3 / 3) | 0, (n3 / 2) | 0, n3 - 2, n3 - 1].filter((v) => v >= 0 && v < n3)) {
      const k = farLevels(z, y);
      const edge = y < n3 / 2 ? y : y + 1;
      const lat = Math.atan(Math.sinh(Math.PI * (1 - (2 * edge) / n3)));
      assert.ok(k >= 0 && k <= z);
      if (k < z) assert.ok(Math.cos(lat) * 2 ** k >= 0.5 - 1e-9, `z${z} y${y}: k ${k}`);
      if (k > 0) assert.ok(Math.cos(lat) * 2 ** (k - 1) < 0.5, `z${z} y${y}: k ${k} more than needed`);
      if (Math.abs(lat) <= Math.PI / 3 - 1e-9) assert.equal(k, 0);
    }
  }
  assert.equal(farLevels(4, 15), 3);
  assert.equal(farLevels(8, 0), 3);
}

// The encoding: MapLibre's custom unpacking of the code gives the elevation back; linear (0.1 m)
// as far as COAST_LINEAR, monotonic, and land deeper than measured / water beyond reach beyond any
// ramp's ends (0.7 CSS px of land, an 80-px band, at the app's least zoom, 1.2, on the equator).
{
  const { redFactor, greenFactor, blueFactor, baseShift } = COAST_ENCODING;
  const unpack = (code) => (code >> 16) * redFactor + ((code >> 8) & 255) * greenFactor + (code & 255) * blueFactor - baseShift;
  let last = -Infinity;
  for (const d of [-1e9, -100000, -24000, -5, -0.05, 0, 0.05, 1.23, 1000, 399999.9, 400000, 400064, 1e6, 3e6, 7e7, 1e9]) {
    const e = coastElevation(d), code = coastCode(d);
    assert.ok(Math.abs(unpack(code) - e) < 0.051, `${d}: ${unpack(code)} against ${e}`);
    assert.ok(e >= last);
    last = e;
  }
  for (const d of [-99999, -3.21, 0, 7.77, 123456.7, COAST_LINEAR]) assert.ok(Math.abs(coastElevation(d) - d) < 1e-9);
  const px = 40075016.686 / (512 * 2 ** 1.2);
  assert.ok(unpack(coastCode(-Infinity)) < -0.7 * px, 'deep land past the ramp');
  assert.ok(unpack(coastCode(Infinity)) > coastElevation(81 * px), 'far water past the widest band');
  assert.ok(coastElevation(81 * px) < unpack(coastCode(Infinity)) && coastElevation(81 * px) > coastElevation(80 * px));
}
console.log('coastdist: ok');
