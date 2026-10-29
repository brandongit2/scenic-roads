// node web/test/inview.test.ts (tools/dev/check.sh's web part): src/inview.ts, the ground in view as
// the "in view" lists, landmarks and ferries test it (#826): an ordinary view, a view round each
// pole and one across the antimeridian, and the bounds without an outline.
import assert from 'node:assert/strict';
import { boxKm, boxesMeet, inBox, viewArea } from '../src/inview.ts';

const D = Math.PI / 180;
const wrap = (x: number) => ((((x + 180) % 360) + 360) % 360) - 180;
/** A ring of `k` points at `r` degrees (great-circle) round (lng, lat). */
function ring(lng: number, lat: number, r: number, k = 40): [number, number][] {
  const l0 = lng * D, p0 = lat * D, d = r * D;
  return Array.from({ length: k }, (_, i) => {
    const b = (i / k) * 2 * Math.PI;
    const p = Math.asin(Math.sin(p0) * Math.cos(d) + Math.cos(p0) * Math.sin(d) * Math.cos(b));
    const l = l0 + Math.atan2(Math.sin(b) * Math.sin(d) * Math.cos(p0), Math.cos(d) - Math.sin(p0) * Math.sin(p));
    return [wrap(l / D), p / D];
  });
}
const none: [number, number, number, number] = [0, 0, 0, 0];

// Ordinary: Québec.
{
  const a = viewArea([[-75, 45], [-70, 45], [-70, 48], [-75, 48]], none);
  assert.ok(a.test(-72, 46));
  assert.ok(!a.test(-76, 46) && !a.test(-72, 49) && !a.test(108, -46)); // and not its antipode
  assert.deepEqual(a.box, [-75, 45, -70, 48]);
  assert.equal(a.pole, null);
  assert.ok(Math.abs(boxKm(a.box) - 5 * 111.32 * Math.cos(46.5 * D)) < 1e-6);
}
// Round the South Pole (centred off it): the pole and the far side of it in, the coast out.
{
  const a = viewArea(ring(30, -88, 5), none);
  assert.equal(a.pole, 's');
  assert.ok(a.test(0, -90) && a.test(-150, -88) && a.test(30, -84) && a.test(120, -86));
  assert.ok(!a.test(30, -82) && !a.test(-150, -84) && !a.test(0, 90) && !a.test(30, -70));
  assert.equal(a.box[0], -180); assert.equal(a.box[1], -90); assert.equal(a.box[2], 180);
  assert.ok(Math.abs(a.box[3] + 83) < 0.01);
  assert.ok(Math.abs(boxKm(a.box) - 2 * 7 * 111.32) < 2);
}
// Round the North Pole.
{
  const a = viewArea(ring(0, 89, 3), none);
  assert.equal(a.pole, 'n');
  assert.ok(a.test(120, 89.5) && a.test(180, 88.5) && !a.test(180, 85) && !a.test(0, -90));
  assert.equal(a.box[3], 90);
}
// Across the antimeridian: Fiji's side and Samoa's side in, the rest of the band out.
{
  const a = viewArea(ring(180, -17, 3), none);
  assert.equal(a.pole, null);
  assert.ok(a.test(179, -17) && a.test(-179, -17) && a.test(180, -18));
  assert.ok(!a.test(0, -17) && !a.test(170, -17) && !a.test(-170, -17));
  assert.ok(a.box[0] > 176 && a.box[2] < -176, `west > east: ${a.box}`);
  assert.ok(inBox(a.box, 179.5, -17) && !inBox(a.box, 0, -17));
  assert.ok(boxKm(a.box) < 700, `its width is ~6°, not ~354°: ${boxKm(a.box)}`);
  assert.ok(boxesMeet(a.box, [-179.5, -18, -179, -16]) && boxesMeet(a.box, [170, -20, 179.5, -10]) && !boxesMeet(a.box, [0, -20, 10, -10]));
}
// No outline: the bounds, across the antimeridian too.
{
  const a = viewArea([], [170, -10, 190, 10]);
  assert.deepEqual(a.box, [170, -10, -170, 10]);
  assert.ok(a.test(175, 0) && a.test(-175, 0) && !a.test(0, 0));
  assert.deepEqual(viewArea([], [-200, -10, 200, 10]).box, [-180, -10, 180, 10]);
}
console.log('inview: ok');
