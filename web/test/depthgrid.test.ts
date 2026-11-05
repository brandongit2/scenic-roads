// node web/test/depthgrid.test.ts (tools/dev/check.sh's web part): src/depthgrid.ts, the depth
// zooms the map asks for and the worker's grids above the least zoom the server stores.
import assert from 'node:assert/strict';
import { SENTINEL, coarsen, depthZooms, type Grid } from '../src/depthgrid.ts';

const grid = (d: (x: number, y: number) => number, k: (x: number, y: number) => number): Grid => {
  const g = { d: new Float32Array(256 * 256), k: new Uint8Array(256 * 256) };
  for (let y = 0; y < 256; y++) for (let x = 0; x < 256; x++) {
    g.d[y * 256 + x] = d(x, y);
    g.k[y * 256 + x] = k(x, y);
  }
  return g;
};
const at = (g: Grid, x: number, y: number) => [g!.d[y * 256 + x], g!.k[y * 256 + x]];

// Above the least zoom: the tiles under it, averaged over the pixels with data.
{
  assert.equal(coarsen([null, null, null, null], 2), null);
  const g = grid((x) => (x % 2 ? SENTINEL : -10), () => 2);
  const c = coarsen([g, null, null, null], 2)!;
  assert.deepEqual(at(c, 0, 0), [-10, 2]);
  assert.deepEqual(at(c, 128, 0), [SENTINEL, 0]);
}

// The zooms (#429: the map asked for z16 relief and contour tiles, made from the server's z17, past
// the layers' z16: 404s, no depth from map zoom 15.5).
{
  const l = (minzoom: number, maxzoom: number) => ({ encoding: 'x', minzoom, maxzoom });
  assert.deepEqual(depthZooms({ depth: l(0, 16), 'depth-class': l(0, 16), 'depth-shade': l(0, 16) }), { least: 0, tiles: 15, shade: 16 });
  // Each from the catalogue: the tightest of depth and its classes.
  assert.deepEqual(depthZooms({ depth: l(0, 14), 'depth-class': l(3, 15), 'depth-shade': l(0, 13) }), { least: 3, tiles: 13, shade: 13 });
  assert.equal(depthZooms({ depth: l(0, 16) }), null);
  assert.equal(depthZooms(undefined), null);
}
