// node --test tools/check/twofinger.test.mjs: two fingers on a touch screen read as camera steps
// (web/src/twofinger.ts, which Node loads with its types stripped).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { TwoFingers } from '../../web/src/twofinger.ts';

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} ≉ ${b}`);
/** Runs a gesture of `n` steps, finger positions given for s in [0, 1]; returns the summed steps. */
function run(pos, n = 40, dt = 16) {
  const [a0, b0] = pos(0);
  const g = new TwoFingers({ x: a0[0], y: a0[1] }, { x: b0[0], y: b0[1] });
  const sum = { dz: 0, dBearing: 0, dPitch: 0, pan: { x: 0, y: 0 } };
  const steps = [];
  for (let i = 1; i <= n; i++) {
    const [a, b] = pos(i / n);
    const st = g.move({ x: a[0], y: a[1] }, { x: b[0], y: b[1] }, i * dt);
    if (!st) continue;
    steps.push(st);
    sum.dz += st.dz;
    sum.dBearing += st.dBearing;
    sum.dPitch += st.dPitch;
    sum.pan.x += st.to.x - st.from.x;
    sum.pan.y += st.to.y - st.from.y;
  }
  return { g, sum, steps };
}

test('a pinch zooms by log2 of the spread, about the midpoint', () => {
  const { sum, steps, g } = run((s) => { const d = 100 + 100 * s; return [[500 - d / 2, 400], [500 + d / 2, 400]]; });
  assert.equal(g.tilt, false);
  // Nothing below the threshold (a tenth of a level); past it, the whole spread counts.
  assert.equal(steps[0].dz, 0);
  near(sum.dz, 1, 1e-9);
  near(sum.dBearing, 0);
  near(sum.pan.x, 0);
  near(sum.pan.y, 0);
  for (const st of steps) assert.deepEqual(st.from, { x: 500, y: 400 });
});

test('the zoom is the same whatever the finger distance: a spread ratio, not pixels', () => {
  const a = run((s) => { const d = 60 * (1 + s); return [[500 - d / 2, 400], [500 + d / 2, 400]]; }).sum.dz;
  const b = run((s) => { const d = 300 * (1 + s); return [[500 - d / 2, 400], [500 + d / 2, 400]]; }).sum.dz;
  near(a, b, 1e-6);
});

test('pinching in zooms out', () => {
  const { sum } = run((s) => { const d = 240 - 120 * s; return [[500 - d / 2, 400], [500 + d / 2, 400]]; });
  near(sum.dz, -1, 1e-9);
});

test('turning the fingers clockwise lowers the bearing by the angle turned past the threshold', () => {
  const r = 90, th = Math.PI / 4;
  const { sum, g } = run((s) => { const a = th * s; return [[500 - r * Math.cos(a), 400 - r * Math.sin(a)], [500 + r * Math.cos(a), 400 + r * Math.sin(a)]]; });
  assert.equal(g.tilt, false);
  // The threshold is 25 px of arc on the circle of the fingers' spread: 25 / 90 rad.
  const thresholdDeg = (25 / r) * (180 / Math.PI);
  assert.ok(sum.dBearing < -(45 - thresholdDeg - 2) && sum.dBearing >= -45, `${sum.dBearing}`);
  near(sum.dz, 0, 1e-9);
});

test('a small turn below the threshold does not rotate', () => {
  const r = 150, th = (10 / r);
  const { sum } = run((s) => { const a = th * s; return [[500 - r * Math.cos(a), 400 - r * Math.sin(a)], [500 + r * Math.cos(a), 400 + r * Math.sin(a)]]; });
  near(sum.dBearing, 0);
});

test('both fingers dragged up together tilt (0.5° a pixel) and nothing else', () => {
  const { sum, g, steps } = run((s) => [[410, 400 - 60 * s], [590, 400 - 60 * s]]);
  assert.equal(g.tilt, true);
  assert.ok(sum.dPitch > 29 && sum.dPitch <= 30, `${sum.dPitch}`);
  near(sum.dz, 0);
  near(sum.dBearing, 0);
  for (const st of steps) assert.deepEqual(st.to, st.from);
});

test('dragged down they flatten', () => {
  assert.ok(run((s) => [[410, 400 + 60 * s], [590, 400 + 60 * s]]).sum.dPitch < -29);
});

test('fingers one above the other dragged up pan, not tilt', () => {
  const { sum, g } = run((s) => [[500, 300 - 60 * s], [500, 500 - 60 * s]]);
  assert.equal(g.tilt, false);
  near(sum.dPitch, 0);
  near(sum.pan.y, -60, 1e-9);
});

test('a two-finger drag sideways pans by the midpoint', () => {
  const { sum, g } = run((s) => [[410 + 80 * s, 400], [590 + 80 * s, 400]]);
  assert.equal(g.tilt, false);
  near(sum.pan.x, 80);
  near(sum.dz, 0);
  near(sum.dBearing, 0);
});

test('a pinch whose midpoint wavers a few pixels does not pan', () => {
  const { sum, steps } = run((s) => { const d = 100 + 100 * s; return [[500 - d / 2, 400 + 4 * Math.sin(9 * s)], [500 + d / 2 + 6 * s, 400]]; });
  assert.ok(sum.dz > 0.85);
  near(sum.pan.x, 0);
  near(sum.pan.y, 0);
  for (const st of steps) assert.deepEqual(st.from, { x: 500, y: 400 });
});

test('a pan catches up with the midpoint once past the dead zone, then follows it', () => {
  const { steps } = run((s) => [[410 + 40 * s, 400], [590 + 40 * s, 400]]);
  const last = steps[steps.length - 1];
  near(last.to.x, 540);
  const first = steps.findIndex((st) => st.to.x !== st.from.x);
  assert.ok(steps[first].to.x - 500 >= 6);
});

test('one finger moving alone for long is not a tilt', () => {
  const g = new TwoFingers({ x: 400, y: 400 }, { x: 600, y: 400 });
  assert.equal(g.move({ x: 400, y: 380 }, { x: 600, y: 400 }, 0), null);
  assert.equal(g.move({ x: 400, y: 360 }, { x: 600, y: 400 }, 50), null);
  const st = g.move({ x: 400, y: 340 }, { x: 600, y: 400 }, 120);
  assert.equal(g.tilt, false);
  assert.ok(st && st.dPitch === 0);
});
