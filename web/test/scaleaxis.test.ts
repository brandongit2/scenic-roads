// node web/test/scaleaxis.test.ts (tools/dev/check.sh's web part): src/scaleaxis.ts, the colour
// scales' spacing and their legends' ticks (ui/scale.ts ScaleControls).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { axisTicks, spaced, unspaced } from '../src/scaleaxis.ts';

/** Labels as the depth legend's (whole metres). */
const L = (v: number) => `${Math.round(v)} m`;
const near = (a: number, b: number, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} ≉ ${b}`);

test('spacing: log is log(1 + v), mirrored below 0, and unspaced undoes it', () => {
  near(spaced('log', 0), 0);
  near(spaced('log', 9), Math.log(10));
  near(spaced('log', -9), -Math.log(10));
  for (const v of [-50, -0.5, 0, 0.3, 1, 299.5, 6000]) {
    near(unspaced('log', spaced('log', v)), v, 1e-9 * Math.max(1, Math.abs(v)));
    assert.equal(unspaced('linear', spaced('linear', v)), v);
  }
});

test('log axis, 0–300 m over 400 px: 1, 2 and 5 per decade at log(1 + v), labelled as the depth legend was', () => {
  const W = 400, b = spaced('log', 300);
  const ticks = axisTicks('log', 0, b, W, 1, L, () => 24);
  assert.deepEqual(ticks.map((t) => t.v), [0, 1, 2, 5, 10, 20, 50, 100, 200]);
  for (const t of ticks) near(t.x, (Math.log1p(t.v) / Math.log1p(300)) * W);
  near(ticks[4].x, 168.0637, 1e-4); // 10 m: well left of the middle, where linear puts it at 13 px
  // 24 px labels all fit, 4 px apart. At 40 px, 1 m gives way to 0 m (placed first) and 5 m to
  // 10 m (a power of ten before a five); 2 m and 20 m, placed last, fit in the gaps left.
  assert.ok(ticks.every((t) => t.lx !== undefined));
  const wide = axisTicks('log', 0, b, W, 1, L, () => 40);
  assert.deepEqual(wide.filter((t) => t.lx !== undefined).map((t) => t.v), [0, 2, 10, 20, 50, 100, 200]);
  // Labels centred on their ticks, kept inside the axis.
  assert.equal(ticks[0].lx, 0);
  near(ticks[4].lx!, ticks[4].x - 12);
});

test('log axis: the handles\' labels keep their room; the step is the smallest tick', () => {
  const b = spaced('log', 300);
  const ticks = axisTicks('log', 0, b, 400, 1, L, () => 24, [[0, 20], [360, 400]]);
  const labelled = ticks.filter((t) => t.lx !== undefined).map((t) => t.v);
  assert.ok(!labelled.includes(0) && !labelled.includes(200), labelled.join());
  assert.ok(labelled.includes(100));
  assert.deepEqual(axisTicks('log', spaced('log', 10), spaced('log', 3000), 400, 1, L, () => 24).map((t) => t.v), [10, 20, 50, 100, 200, 500, 1000, 2000]);
  assert.deepEqual(axisTicks('log', 0, spaced('log', 1), 400, 0.1, L, () => 24).map((t) => t.v), [0, 0.1, 0.2, 0.5, 1]);
});

test('linear axis: a nice step, in proportion', () => {
  const ticks = axisTicks('linear', 0, 300, 400, 1, L, () => 24);
  assert.deepEqual(ticks.map((t) => t.v), [0, 50, 100, 150, 200, 250, 300]);
  for (const t of ticks) near(t.x, (t.v / 300) * 400);
  assert.ok(ticks.every((t) => t.lx !== undefined));
});

test('linear axis: no finer than the metric\'s step, and only labels that are exact and distinct', () => {
  // Grade, 0–3 % in 0.5 % steps, shown to the whole per cent: ticks at 0.5, labelled only at whole values.
  const pc = (v: number) => `${v.toFixed(0)} %`;
  const ticks = axisTicks('linear', 0, 3, 400, 0.5, pc, () => 20);
  assert.deepEqual(ticks.map((t) => t.v), [0, 0.5, 1, 1.5, 2, 2.5, 3]);
  assert.deepEqual(ticks.filter((t) => t.lx !== undefined).map((t) => t.v), [0, 1, 2, 3]);
  // A step coarser than the nice one wins: 0–2 in steps of 1, not 0.5.
  assert.deepEqual(axisTicks('linear', 0, 2, 400, 1, pc, () => 20).map((t) => t.v), [0, 1, 2]);
});
