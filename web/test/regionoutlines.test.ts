// node web/test/regionoutlines.test.ts (tools/dev/check.sh's web part): src/regions.ts, the
// regions' outlines on the map: the region under the cursor (the footer's), the zoom asked, and the
// lines drawn from outlines clipped to a box or reaching past the Mercator tiles' latitudes.
import assert from 'node:assert/strict';
import { MERCATOR_LAT } from '../src/merc.ts';
import { inBand, inside, outlineLines, outlineZoom, regionAt, type RegionOutline } from '../src/regions.ts';

const sq = (w: number, s: number, e: number, n: number) => [[w, s], [e, s], [e, n], [w, n], [w, s]];
const region = (id: string, rings: number[][][], bbox: [number, number, number, number]): RegionOutline => ({
  type: 'Feature', geometry: { type: 'MultiPolygon', coordinates: [rings] },
  properties: { region: id, name: id, disabled: false, bbox, label: null, area: null },
});

// A country's waters around it, a province inside it with a lake (a hole).
const waters = region('waters', [sq(-80, 40, -50, 60)], [-80, 40, -50, 60]);
const quebec = region('quebec', [sq(-79, 45, -57, 62), sq(-75, 50, -73, 52)], [-79, 45, -57, 62]);
assert.equal(inside(quebec.geometry.coordinates, [-72, 53]), true);
assert.equal(inside(quebec.geometry.coordinates, [-74, 51]), false, 'in its lake');
// The smallest region holding the point wins; none outside them all.
assert.equal(regionAt([waters, quebec], [-72, 53])?.properties.region, 'quebec');
assert.equal(regionAt([waters, quebec], [-74, 51])?.properties.region, 'waters');
assert.equal(regionAt([waters, quebec], [-72, 61])?.properties.region, 'quebec');
assert.equal(regionAt([waters, quebec], [0, 0]), null);

// The zoom asked: floored, 0–12.
assert.deepEqual([-1, 0, 3.9, 12, 17.5].map(outlineZoom), [0, 0, 3, 12, 12]);

// Clipped to a box: the stretches along its edges aren't drawn, the rest is, in pieces.
const clipped = region('c', [[[0, 0], [2, 0], [2, 1], [0, 1], [0, 0]]], [0, 0, 2, 1]);
const unclipped = outlineLines(clipped, null).geometry.coordinates;
assert.equal(unclipped.length, 1);
assert.equal(unclipped[0].length, 5);
// (The box's right edge, x = 2, is a cut: the other three sides stay, in two pieces.)
assert.deepEqual(outlineLines(clipped, [-1, -1, 2, 2]).geometry.coordinates, [[[0, 0], [2, 0]], [[2, 1], [0, 1], [0, 0]]]);
// Split at the antimeridian: the stretch along 180° is a cut too, with or without a box.
const east = region('e', [[[170, 0], [180, 0], [180, 5], [170, 5], [170, 0]]], [170, 0, 180, 5]);
assert.deepEqual(outlineLines(east, null).geometry.coordinates, [[[170, 0], [180, 0]], [[180, 5], [170, 5], [170, 0]]]);
// Reaching a pole (Antarctica's drawn shape, split at the antimeridian, closed along 90° S): the
// stretch along the pole's latitude isn't drawn (MapLibre held it to the tiles' edge, a ring at
// 85.05° S), nor what lies past ±MERCATOR_LAT; the coast stays whole. The North Pole's alike.
const south = region('s', [[[-180, -70], [0, -65], [180, -70], [180, -90], [-180, -90], [-180, -70]]], [-180, -90, 180, -65]);
assert.deepEqual(outlineLines(south, null).geometry.coordinates, [[[-180, -70], [0, -65], [180, -70]]]);
const north = region('n', [[[-180, 80], [180, 80], [180, 90], [-180, 90], [-180, 80]]], [-180, 80, 180, 90]);
assert.deepEqual(outlineLines(north, null).geometry.coordinates, [[[-180, 80], [180, 80]]]);
// A border crossing the edge is cut where it does, on the edge; one along a meridian to the pole
// stops at the edge on that meridian.
const lats = (ls: number[][][]) => ls.flat().map((p) => p[1]);
const across = inBand([[10, 80], [20, 88], [30, 80]]);
assert.equal(across.length, 2);
assert.deepEqual([across[0][0], across[1][1]], [[10, 80], [30, 80]]);
assert.ok(Math.abs(across[0][1][1] - MERCATOR_LAT) < 1e-9 && Math.abs(across[1][0][1] - MERCATOR_LAT) < 1e-9);
assert.ok(across[0][1][0] > 10 && across[0][1][0] < 20 && across[1][0][0] > 20 && across[1][0][0] < 30);
const meridian = inBand([[45, -80], [45, -90]]);
assert.equal(meridian.length, 1);
assert.equal(meridian[0][1][0], 45);
assert.ok(Math.abs(meridian[0][1][1] + MERCATOR_LAT) < 1e-9);
// Wholly past the edge: nothing; on it: drawn.
assert.deepEqual(inBand([[0, 86], [10, 89], [20, 86]]), []);
assert.ok(lats(inBand([[0, MERCATOR_LAT], [10, MERCATOR_LAT]])).every((l) => l === MERCATOR_LAT));
console.log('regionoutlines: ok');
