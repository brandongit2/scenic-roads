// node web/test/rings.test.ts (tools/dev/check.sh's web part): src/rings.ts's nesting, against
// comparing every pair, and its time on an Arctic-sized outline (#564: 111 531 rings froze the page).
import assert from 'node:assert/strict';
import { nested } from '../src/rings.ts';

type Ring = number[][];
const sq = (x: number, y: number, d: number): Ring => [[x, y], [x + d, y], [x + d, y + d], [x, y + d], [x, y]];
const feat = (rings: Ring[]) => ({ type: 'Feature' as const, properties: {}, geometry: { type: 'MultiPolygon' as const, coordinates: rings.map((r) => [r]) } });

/** The nesting by comparing every pair (the code before #564). */
function brute(rings: Ring[]): Ring[][] {
  const box = rings.map((r) => [Math.min(...r.map((p) => p[0])), Math.min(...r.map((p) => p[1])), Math.max(...r.map((p) => p[0])), Math.max(...r.map((p) => p[1]))]);
  const inRing = (r: Ring, [x, y]: number[]) => {
    let inside = false;
    for (let i = 0, j = r.length - 1; i < r.length; j = i++) if (r[i][1] > y !== r[j][1] > y && x < ((r[j][0] - r[i][0]) * (y - r[i][1])) / (r[j][1] - r[i][1]) + r[i][0]) inside = !inside;
    return inside;
  };
  const around = rings.map((r, i) => rings.flatMap((_, j) => (j !== i && box[j][0] <= box[i][0] && box[j][1] <= box[i][1] && box[j][2] >= box[i][2] && box[j][3] >= box[i][3] && inRing(rings[j], r[0]) ? [j] : [])));
  const depth = around.map((a) => a.length);
  const size = box.map((b) => (b[2] - b[0]) * (b[3] - b[1]));
  const polys = new Map<number, Ring[]>();
  rings.forEach((r, i) => depth[i] % 2 === 0 && polys.set(i, [r]));
  rings.forEach((r, i) => {
    if (depth[i] % 2 === 0) return;
    const outer = around[i].filter((j) => depth[j] === depth[i] - 1).sort((a, b) => size[a] - size[b])[0];
    if (outer === undefined) polys.set(i, [r]);
    else polys.get(outer)!.push(r);
  });
  return [...polys.values()];
}

// An outer ring, a hole in it, an island in the hole, a lake on the island; a separate square.
const simple = [sq(0, 0, 10), sq(2, 2, 6), sq(3, 3, 4), sq(4, 4, 1), sq(20, 0, 2)];
assert.deepEqual(nested(feat(simple)).geometry.coordinates, [[simple[0], simple[1]], [simple[2], simple[3]], [simple[4]]]);

// Random squares nested by chance, at the grid's edges too: the same as comparing every pair.
let seed = 1;
const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
for (let t = 0; t < 20; t++) {
  const rings = Array.from({ length: 300 }, () => sq(rnd() * 100, rnd() * 100, rnd() ** 4 * 60));
  assert.deepEqual(nested(feat(rings)).geometry.coordinates, brute(rings), `random set ${t}`);
}

// Nunavut-sized: 110 000 islands, a few lakes on them, in a large outer ring. Every pair took minutes.
const many: Ring[] = [sq(-1, -1, 1002)];
for (let i = 0; i < 110_000; i++) many.push(sq(rnd() * 999, rnd() * 999, 0.5));
for (let i = 1; i < 2000; i += 2) many.push(sq(many[i][0][0] + 0.1, many[i][0][1] + 0.1, 0.1));
const t0 = performance.now();
const out = nested(feat(many)).geometry.coordinates;
const ms = performance.now() - t0;
assert.equal(out.reduce((n, p) => n + p.length, 0), many.length);
assert.ok(ms < 3000, `110 000 rings nested in ${ms.toFixed(0)} ms (over 3 s)`);
console.log(`rings: ok (110 000 rings in ${ms.toFixed(0)} ms)`);
