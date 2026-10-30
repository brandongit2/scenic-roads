// node web/test/regionoutlines.test.ts (tools/dev/check.sh's web part): src/regions.ts, the
// regions' outlines on the map: the region under the cursor (the footer's), the zoom asked, and the
// lines drawn from outlines clipped to a box.
import assert from 'node:assert/strict';
import { inside, outlineLines, outlineZoom, regionAt, type RegionOutline } from '../src/regions.ts';

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
console.log('regionoutlines: ok');
