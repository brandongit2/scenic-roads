// node web/test/merc.test.ts (tools/dev/check.sh's web part): src/merc.ts, the app's one Web
// Mercator: its square's edges, the inverse, and the clamped forms the tile choices use.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MERCATOR_LAT, clampLat, mercLat, mercLon, mercPhi, mercX, mercXIn, mercY, mercYClamped, mercYIn, tileX, tileY } from '../src/merc.ts';

const near = (a: number, b: number, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} ≉ ${b}`);

test('the square: MERCATOR_LAT is its top, the equator its middle', () => {
  near(mercY(MERCATOR_LAT), 0, 1e-12);
  near(mercY(-MERCATOR_LAT), 1, 1e-12);
  assert.equal(mercY(0), 0.5);
  assert.equal(mercX(-180), 0);
  assert.equal(mercX(180), 1);
});

test('the inverse', () => {
  for (const lat of [-85, -60.5, -1e-6, 0, 12.25, 47.3, 84.9]) near(mercLat(mercY(lat)), lat, 1e-9);
  for (const lon of [-180, -73.5, 0, 151.2]) near(mercLon(mercX(lon)), lon, 1e-12);
  near(mercPhi(0.25), (mercLat(0.25) * Math.PI) / 180, 1e-15);
});

test('past the tiles: mercY runs on, the clamped and In forms hold', () => {
  assert.ok(mercY(89) < 0 && mercY(-89) > 1);
  assert.equal(mercY(90), -Infinity);
  assert.equal(clampLat(89), MERCATOR_LAT);
  assert.equal(mercYClamped(89), mercY(MERCATOR_LAT));
  assert.equal(mercYIn(90), 0);
  assert.ok(mercYIn(-90) < 1 && mercYIn(-90) > 1 - 1e-8);
  assert.ok(mercXIn(180) < 1);
});

test('tile rows and columns, held to the world', () => {
  assert.equal(tileX(-180, 3), 0);
  assert.equal(tileX(180, 3), 7);
  assert.equal(tileY(90, 10), 0);
  assert.equal(tileY(-90, 10), 1023);
  // Within 0.05° of the top, row 0 at z10 (85° would have been row 1).
  assert.equal(tileY(85.03, 10), 0);
  assert.equal(tileY(85, 10), 1);
  assert.equal(tileY(0, 1), 1);
});
