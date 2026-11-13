// node web/test/depthgrid.test.ts (tools/dev/check.sh's web part): src/depthgrid.ts, the depth
// zooms the map asks for, and the values its relief and contours draw.
import assert from 'node:assert/strict';
import { DRY_MIN, REACH, SENTINEL, contourValues, depthZooms, reliefValues, type Grid, type Square } from '../src/depthgrid.ts';

const grid = (d: (x: number, y: number) => number, k: (x: number, y: number) => number): Grid => {
  const g = { d: new Float32Array(256 * 256), k: new Uint8Array(256 * 256) };
  for (let y = 0; y < 256; y++) for (let x = 0; x < 256; x++) {
    g.d[y * 256 + x] = d(x, y);
    g.k[y * 256 + x] = k(x, y);
  }
  return g;
};

// The zooms (#429: the map asked for z16 relief and contour tiles, made from the server's z17, past
// the layers' z16: 404s, no depth from map zoom 15.5).
{
  const l = (maxzoom: number) => ({ encoding: 'x', minzoom: 0, maxzoom });
  assert.deepEqual(depthZooms({ depth: l(16), 'depth-class': l(16), 'depth-shade': l(16) }), { tiles: 15, shade: 16 });
  // Each from the catalogue: the tightest of depth and its classes.
  assert.deepEqual(depthZooms({ depth: l(14), 'depth-class': l(15), 'depth-shade': l(13) }), { tiles: 13, shade: 13 });
  assert.equal(depthZooms({ depth: l(16) }), null);
  assert.equal(depthZooms(undefined), null);
}

// The relief (#963): a row of 64 px across. Water with data (x < 20, -6 m), the build's band on the
// land (class none, 0 m: x 20–22), a dike (land, x 20–29), then water without data (x ≥ 30):
// another water body. The land takes the water's own depth, never the band's; the water across the
// dike stays without; the ramp's drying colours only where marked drying.
{
  const N = 64;
  const sq = (f: (x: number) => [number, number, number]): Square => {
    const c = { d: new Float32Array(N * N), k: new Uint8Array(N * N), w: new Uint8Array(N * N) };
    for (let y = 0; y < N; y++) for (let x = 0; x < N; x++) [c.d[y * N + x], c.k[y * N + x], c.w[y * N + x]] = f(x);
    return c;
  };
  const row = (v: Float32Array) => Array.from(v.subarray(32 * N, 33 * N));
  const dike = sq((x) => (x < 20 ? [-6, 1, 1] : x < 23 ? [0, 0, 0] : x < 30 ? [SENTINEL, 0, 0] : [SENTINEL, 0, 1]));
  const r = row(reliefValues(dike, true));
  for (let x = 18; x < 30; x++) assert.equal(r[x], x <= 19 + REACH ? -6 : SENTINEL, `x ${x}`);
  assert.ok(r.slice(30).every((v) => v === SENTINEL), 'no depth across the dike');
  // Unmasked: only the basemap's water.
  assert.ok(row(reliefValues(dike, false)).slice(20).every((v) => v === SENTINEL));
  // A shore without a dike: the water without data beside the data takes it, within REACH.
  const shore = sq((x) => (x < 20 ? [-6, 1, 1] : [SENTINEL, 0, 1]));
  const s2 = row(reliefValues(shore, true));
  assert.equal(s2[20 + REACH - 1], -6);
  assert.equal(s2[20 + REACH], SENTINEL);
  // Drying: a value above 0 not marked drying is at most 0 (the shallowest blue); marked, at least DRY_MIN.
  const dry = sq((x) => (x < 10 ? [1.5, 4, 1] : x < 20 ? [0, 1 | 8, 1] : [SENTINEL, 0, 0]));
  const d = row(reliefValues(dry, true));
  assert.equal(d[5], 0);
  assert.equal(d[15], Math.fround(DRY_MIN));
  assert.equal(d[21], Math.fround(DRY_MIN), "the reach keeps its pixel's drying mark");
  // The contours: water without data takes the nearest in the water, never across the dike.
  const cv = row(contourValues(dike));
  assert.equal(cv[10], -6);
  assert.ok(cv.slice(20).every((v) => v === 0));
}
