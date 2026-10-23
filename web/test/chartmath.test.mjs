// node web/test/chartmath.test.mjs (tools/dev/check.sh's web part): the build page's charts' sums
// (web/work/chartmath.js).
import assert from "node:assert/strict";
import { commonGaps, fmt, footer, gaps, hhmm, latest, merge, niceMax, pressureClass, recentMean, runs, spanText, ticks, timeTicks, topOf } from "../work/chartmath.js";

// Scales.
assert.equal(niceMax(0), 1);
assert.equal(niceMax(NaN), 1);
assert.equal(niceMax(0.7), 1);
assert.equal(niceMax(1.3), 2);
assert.equal(niceMax(2.2), 2.5);
assert.equal(niceMax(37), 50);
assert.equal(niceMax(8), 10);
assert.deepEqual(ticks(10), [0, 5, 10]);
assert.deepEqual(ticks(50), [0, 25, 50]);
assert.deepEqual(ticks(2), [0, 1, 2]);
assert.deepEqual(ticks(1), [0, 0.5, 1]);

// Units, metric, as short as they read.
assert.equal(fmt(null, "MB/s"), "–");
assert.equal(fmt(0, "MB/s"), "0 MB/s");
assert.equal(fmt(0.5, "MB/s"), "512 kB/s");
assert.equal(fmt(3.25, "MB/s"), "3.3 MB/s");
assert.equal(fmt(2048, "MB/s"), "2 GB/s");
assert.equal(fmt(212.4, "GB"), "212 GB");
assert.equal(fmt(1536, "GB"), "1.5 TB");
assert.equal(fmt(1.5 * 2 ** 30, "B"), "1.5 GB");
assert.equal(fmt(500, "B"), "500 B");
assert.equal(fmt(42.4, "ms"), "42 ms");
assert.equal(fmt(2500, "ms"), "2.5 s");
assert.equal(fmt(61.6, "%"), "62%");
assert.equal(fmt(1, "cores"), "1 core");
assert.equal(fmt(5.25, "cores"), "5.3 cores");
assert.equal(spanText(60), "1 min");
assert.equal(spanText(900), "15 min");
assert.equal(spanText(259200), "72 h");

// Gaps where a Mac slept: more than two steps (and 5 s) without a point; and up to now.
const pts = [[100, 0, 1, 1], [101, 0, 1, 1], [103, 0, 1, 1], [200, 0, 2, 2], [201, 0, 2, 2]];
assert.deepEqual(gaps(pts, 1, 202), [{ from: 104, to: 200 }], "a second skipped is no gap; 97 s is");
assert.deepEqual(gaps(pts, 1, 300), [{ from: 104, to: 200 }, { from: 202, to: 300 }], "silent since the last point");
assert.deepEqual(runs(pts, 1).map((r) => r.length), [3, 2]);
assert.deepEqual(gaps([[0, 0, 0, 0], [10, 0, 0, 0], [20, 0, 0, 0]], 10, 25), [], "a 10 s ring's spacing is no gap");
// (Several series: only where all are silent; one sampled every 10 s doesn't make the seconds a gap.)
const slow = [[100, 0, 1, 1], [110, 0, 1, 1], [200, 0, 1, 1]];
assert.deepEqual(commonGaps([{ points: pts, step: 1 }, { points: slow, step: 10 }], 202), [{ from: 120, to: 200 }]);
assert.deepEqual(commonGaps([{ points: [], step: 1 }], 10), []);

// Time ticks on round local times.
assert.equal(hhmm(3600 * 14 + 300, false, 0), "14:05");
assert.equal(hhmm(3600 * 14 + 330, true, 0), "14:05:30");
assert.equal(hhmm(0, false, 120), "02:00");
const tt = timeTicks(3600 * 10 + 7, 3600 * 11 + 7, 5, 0);
assert.deepEqual(tt.map((x) => x.label), ["10:15", "10:30", "10:45", "11:00"]);
assert.deepEqual(timeTicks(1000, 1060, 5, 0).map((x) => x.label), ["00:16:45", "00:17:00", "00:17:15", "00:17:30"]);
assert.ok(timeTicks(0, 259200, 5, 0).length <= 5);

// The footer: range, resolution, silence of a minute or more.
assert.equal(footer(3600, 1, []), "last 1 h · a point a second");
assert.equal(footer(86400, 10, [{ from: 0, to: 720 }]), "last 24 h · a point every 10 s · asleep or away 12 min");
assert.equal(footer(259200, 60, [{ from: 0, to: 30 }]), "last 72 h · a point a minute");

// Scales from the points: means, the most (a band), stacked.
assert.equal(topOf([[[0, 1, 3, 9]]]), 5);
assert.equal(topOf([[[0, 1, 3, 9]]], { band: true }), 10);
assert.equal(topOf([[[0, 0, 3, 3]], [[0, 0, 4, 4]]], { stacked: true }), 10);
assert.equal(topOf([[]], { least: 8 }), 10);

// Merging answers: points before the window dropped, the last one again replaced.
const a = merge(null, { series: { x: [[10, 0, 1, 1], [11, 0, 1, 1]] } }, 0);
const b = merge(a, { series: { x: [[11, 0, 5, 5], [12, 0, 2, 2]], y: [[12, 0, 1, 1]] } }, 11);
assert.deepEqual(b.x, [[11, 0, 5, 5], [12, 0, 2, 2]]);
assert.deepEqual(b.y, [[12, 0, 1, 1]]);
assert.deepEqual(merge(b, { series: {} }, 13), {}, "a window past every point: none (never zero)");
assert.equal(latest(b.x, 20), 2);
assert.equal(latest(b.x, 100), null, "an old point isn't now's");
assert.equal(recentMean([[0, 0, 4, 4], [50, 0, 0, 0], [60, 0, 2, 2]], 60), 1, "the last minute's means");
assert.equal(recentMean([[0, 0, 4, 4]], 100), null, "none in the last minute: none");
assert.equal(pressureClass(1), "ok");
assert.equal(pressureClass(2), "warn");
assert.equal(pressureClass(4), "bad");
assert.equal(pressureClass(null), "");
console.log("chartmath: ok");
