// node web/test/worldmap.test.mjs (tools/dev/check.sh's web part): the build page's world map's
// view rule (web/work/worldmap.js constrain, #865).
import assert from "node:assert/strict";
import { constrain, minZoom, pannable, wheelSplit } from "../work/worldmap.js";

const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-6, `${a} ≉ ${b}`);
const wide = { w: 1600, h: 800 }, narrow = { w: 400, h: 900 }, square = { w: 600, h: 600 };

// The least zoom: the whole square, side = the shorter axis; never further out.
near(minZoom(wide), 0.8); near(minZoom(narrow), 0.4); near(minZoom(square), 0.6);
for (const s of [wide, narrow, square]) {
  const v = constrain({ cx: 0, cy: 0, k: 0.01 }, s);
  near(v.k, minZoom(s)); near(v.cx, 500); near(v.cy, 500); // centred both ways (whole map shown)
  assert.equal(pannable(v, s), false);
}
assert.equal(constrain({ cx: 500, cy: 500, k: 1e9 }, wide).k, 48);

// Wide box at min zoom: the map's height = the box's; a pan either way goes nowhere.
{ const v = constrain({ cx: 900, cy: 50, k: 0.8 }, wide); near(v.cx, 500); near(v.cy, 500); }
// Wide, k between the axes' thresholds (map taller than the box, narrower than it): x locked
// centred, y pans with edges clamped (top at most at the top, bottom at most at the bottom).
{ const k = 1.2; // map 1200 px: > h 800, < w 1600
  near(constrain({ cx: 10, cy: 500, k }, wide).cx, 500);
  near(constrain({ cx: 500, cy: 0, k }, wide).cy, 400 / k); // top edge at the box's top
  near(constrain({ cx: 500, cy: 1000, k }, wide).cy, 1000 - 400 / k); // bottom at the bottom
  near(constrain({ cx: 500, cy: 480, k }, wide).cy, 480); // inside: free
  assert.equal(pannable({ k }, wide), true); }
// Just under / at / over the x threshold (k = 1.6: map = box width).
near(constrain({ cx: 100, cy: 500, k: 1.599 }, wide).cx, 500);
near(constrain({ cx: 100, cy: 500, k: 1.6 }, wide).cx, 500);
near(constrain({ cx: 100, cy: 500, k: 1.7 }, wide).cx, 800 / 1.7);
// Narrow box: at min zoom locked both ways (centred vertically, not clinging to the bottom).
{ const v = constrain({ cx: 0, cy: 1000, k: 0.4 }, narrow); near(v.cy, 500); near(v.cx, 500); }
{ const k = 0.6; // map 600 px: > w 400, < h 900 → x pans clamped, y locked centred
  near(constrain({ cx: 0, cy: 0, k }, narrow).cx, 200 / k);
  near(constrain({ cx: 1000, cy: 0, k }, narrow).cx, 1000 - 200 / k);
  near(constrain({ cx: 300, cy: 0, k }, narrow).cy, 500); }
near(constrain({ cx: 500, cy: 0, k: 0.9 }, narrow).cy, 500); // y threshold: equal → locked
near(constrain({ cx: 500, cy: 0, k: 0.91 }, narrow).cy, 450 / 0.91);
// Square box: both thresholds the same.
{ const v = constrain({ cx: 0, cy: 1000, k: 0.6 }, square); near(v.cx, 500); near(v.cy, 500); }
{ const v = constrain({ cx: 0, cy: 1000, k: 1 }, square); near(v.cx, 300); near(v.cy, 700); }
// A box not laid out yet (0×0): a finite view.
assert.ok(Number.isFinite(constrain({ cx: 500, cy: 500, k: 0 }, { w: 0, h: 0 }).k));
// A two-finger scroll: the locked axis's part to the page, the map only the axes it moves on.
{ const v = { cx: 500, cy: 500, k: 0.6 }; // narrow: x pans, y locked
  assert.deepEqual(wheelSplit(v, narrow, 30, 5), { native: false, map: [30, 0], page: [0, 5] });
  assert.equal(wheelSplit(v, narrow, 3, 40).native, true); // mostly vertical: the page's
  assert.deepEqual(wheelSplit(v, narrow, 3, 40).map, [0, 0]); }
assert.deepEqual(wheelSplit({ k: 2 }, narrow, 3, 40), { native: false, map: [3, 40], page: [0, 0] });
assert.equal(wheelSplit({ k: 0.4 }, narrow, 30, 40).native, true); // not pannable: all the page's
console.log("worldmap ok");
