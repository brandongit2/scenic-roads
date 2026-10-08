// The coastal shading's distance to the shore (web/src/coastdist.ts) against brute force, and its
// shores where full detail puts them.
//   node tools/check/coastdist.mjs
import assert from 'node:assert/strict';
import { nearestSeed, signedDistance } from '../../web/src/coastdist.ts';

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
  const sd = signedDistance(frac, n, M, 2);
  for (let c = 16; c < 27; c++) {
    const want = Math.max(-2, c + 0.5 - 20.3);
    assert.ok(Math.abs(at(sd, 20, c) - want) < 0.1, `x ${c}: ${at(sd, 20, c)} against ${want}`);
  }
}

// An island a hundredth of a pixel: still land, its glow measured from it.
{
  const frac = new Float32Array(n * n);
  frac[20 * n + 20] = 0.01;
  const sd = signedDistance(frac, n, M, 2);
  assert.ok(at(sd, 20, 20) < 0.3 && at(sd, 20, 20) > -0.1, `the island's own pixel: ${at(sd, 20, 20)}`);
  for (const [r, c] of [[20, 25], [24, 20], [17, 17]]) {
    const want = Math.hypot(r - 20, c - 20);
    assert.ok(Math.abs(at(sd, r, c) - want) < 0.5, `${r},${c}: ${at(sd, r, c)} against ${want}`);
  }
  // No land at all: as far as measured.
  assert.ok(signedDistance(new Float32Array(n * n), n, M, 2).every((v) => v === M));
}
console.log('coastdist: ok');
