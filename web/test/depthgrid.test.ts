// node web/test/depthgrid.test.ts (tools/dev/check.sh's web part): src/depthgrid.ts, the depth
// worker's grids for zooms the server doesn't store.
import assert from 'node:assert/strict';
import { NODATA_MIN, SENTINEL, coarsen, refine, type Grid } from '../src/depthgrid.ts';

const grid = (d: (x: number, y: number) => number, k: (x: number, y: number) => number): Grid => {
  const g = { d: new Float32Array(256 * 256), k: new Uint8Array(256 * 256) };
  for (let y = 0; y < 256; y++) for (let x = 0; x < 256; x++) {
    g.d[y * 256 + x] = d(x, y);
    g.k[y * 256 + x] = k(x, y);
  }
  return g;
};
const at = (g: Grid, x: number, y: number) => [g!.d[y * 256 + x], g!.k[y * 256 + x]];

// Past the greatest zoom (#429: the map's z16 tiles and the footer at z16 and over had no depth, the
// server's z17 being a 404): the ancestor's quarter, scaled up.
{
  // A small lake (class 3, source 15: the small-lake estimate) in the ancestor's bottom-right
  // quarter, x and y 200–209, its depth −x/10 m; land (none) around it.
  const inLake = (x: number, y: number) => x >= 200 && x < 210 && y >= 200 && y < 210;
  const g = grid((x, y) => (inLake(x, y) ? -x / 10 : SENTINEL), (x, y) => (inLake(x, y) ? (15 << 4) | 3 : 0));
  assert.equal(refine(null, 2, 0, 0), null);
  const r = refine(g, 2, 1, 1)!;
  // Output pixel (2·(x−128), 2·(y−128)) and the next lie over ancestor pixel (x, y).
  const ox = (x: number) => 2 * (x - 128);
  // Inside: bilinear between the ancestor's centres (a quarter of the way from 204 to 205, then three quarters).
  assert.deepEqual(at(r, ox(204) + 1, ox(205)).map((v) => +v.toFixed(3)), [-20.425, (15 << 4) | 3]);
  assert.deepEqual(at(r, ox(205), ox(205)).map((v) => +v.toFixed(3)), [-20.475, (15 << 4) | 3]);
  // At the shore: the nearest pixel's own depth, not blended into none; the class the nearest's.
  assert.deepEqual(at(r, ox(209) + 1, ox(205)).map((v) => +v.toFixed(3)), [-20.9, (15 << 4) | 3]);
  assert.deepEqual(at(r, ox(210), ox(205)), [SENTINEL, 0]);
  assert.deepEqual(at(r, ox(199) + 1, ox(205)), [SENTINEL, 0]);
  // Every pixel with depth is the lake's, and the lake's area is 4× (each ancestor pixel 2 × 2).
  let n = 0;
  for (let i = 0; i < 256 * 256; i++) if (r.d[i] < NODATA_MIN) {
    n++;
    assert.equal(r.k[i], (15 << 4) | 3);
  }
  assert.equal(n, 4 * 100);
  // Another quarter of the same ancestor: none of the lake.
  assert.ok(refine(g, 2, 0, 0)!.d.every((v) => v >= NODATA_MIN));
  // Two zooms past (f = 4), its edge clamped: the ancestor's last row and column hold.
  const e = refine(grid((x, y) => -(x + y), () => 1), 4, 3, 3)!;
  assert.equal(at(e, 255, 255)[0], -510);
  assert.equal(at(e, 0, 0)[0], -2 * 191.625); // (its centre 0.375 of a pixel before the quarter's first)
}

// Above the least zoom: the tiles under it, averaged over the pixels with data.
{
  assert.equal(coarsen([null, null, null, null], 2), null);
  const g = grid((x) => (x % 2 ? SENTINEL : -10), () => 2);
  const c = coarsen([g, null, null, null], 2)!;
  assert.deepEqual(at(c, 0, 0), [-10, 2]);
  assert.deepEqual(at(c, 128, 0), [SENTINEL, 0]);
}
