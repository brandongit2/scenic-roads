// node web/test/catalogs.test.mjs (tools/dev/check.sh's web part): the Catalogues tab's sums
// (web/work/catalogs.js): the range's inputs (parsing, the sides' signs, one span at a time, the
// swap), the tiles' times, the colour map, the timeline.
import assert from "node:assert/strict";
import { axisTicks, commit, formatWhen, keyOf, labelled, mergeCatalogs, pairAt, paint, parseWhen, publishedAt, RAMP, rampColour, rangeOf, tileTimes, timelineSpan } from "../work/catalogs.js";

process.env.TZ = "UTC";
const NOW = Date.UTC(2026, 9, 24, 12, 0, 0) / 1000;
const H = 3600, D = 86400;

// A span: the left's always back, the right's always on, whatever sign is typed.
assert.deepEqual(parseWhen("48 hours", "left", NOW), { kind: "rel", s: -48 * H });
assert.deepEqual(parseWhen("+48 hours", "left", NOW), { kind: "rel", s: -48 * H });
assert.deepEqual(parseWhen("-48 hours", "right", NOW), { kind: "rel", s: 48 * H });
assert.deepEqual(parseWhen("7 days", "right", NOW), { kind: "rel", s: 7 * D });
assert.deepEqual(parseWhen("7d", "left", NOW), { kind: "rel", s: -7 * D });
assert.deepEqual(parseWhen("90 min", "left", NOW), { kind: "rel", s: -90 * 60 });
assert.deepEqual(parseWhen("2 weeks", "right", NOW), { kind: "rel", s: 14 * D });
assert.deepEqual(parseWhen("1.5 h", "left", NOW), { kind: "rel", s: -1.5 * H });
assert.equal(parseWhen("0 hours", "left", NOW), null);
assert.equal(parseWhen("48 parsecs", "left", NOW), null);
// A time (local; UTC here), or now.
assert.deepEqual(parseWhen("now", "right", NOW), { kind: "abs", t: NOW });
assert.deepEqual(parseWhen("2026-10-23 14:05", "left", NOW), { kind: "abs", t: Date.UTC(2026, 9, 23, 14, 5) / 1000 });
assert.deepEqual(parseWhen("2026-10-23T14:05:30", "left", NOW), { kind: "abs", t: Date.UTC(2026, 9, 23, 14, 5, 30) / 1000 });
assert.deepEqual(parseWhen("2026-10-23", "left", NOW), { kind: "abs", t: Date.UTC(2026, 9, 23) / 1000 });
assert.equal(parseWhen("2026-02-30", "left", NOW), null);
assert.equal(parseWhen("2026-10-23 25:00", "left", NOW), null);
assert.equal(parseWhen("", "left", NOW), null);
assert.equal(parseWhen("yesterday", "left", NOW), null);

// As shown.
assert.equal(formatWhen({ kind: "rel", s: -48 * H }), "-2 days");
assert.equal(formatWhen({ kind: "rel", s: -50 * H }), "-50 hours");
assert.equal(formatWhen({ kind: "rel", s: 7 * D }), "+1 week");
assert.equal(formatWhen({ kind: "rel", s: 90 * 60 }), "+90 minutes");
assert.equal(formatWhen({ kind: "abs", t: Date.UTC(2026, 9, 23, 14, 5) / 1000 }), "2026-10-23 14:05");
assert.equal(parseWhen(formatWhen({ kind: "rel", s: -48 * H }), "left", NOW).s, -48 * H);

// The defaults: "-48 hours" and now (at render); kept values back as they were.
const def = pairAt({}, NOW);
assert.deepEqual(def, { left: { kind: "rel", s: -48 * H }, right: { kind: "abs", t: NOW } });
assert.deepEqual(rangeOf(def.left, def.right), [NOW - 48 * H, NOW]);
assert.deepEqual(pairAt({ left: { kind: "abs", t: 5 }, right: null }, NOW).right, { kind: "abs", t: NOW });
assert.deepEqual(pairAt({ left: { kind: "rel", s: -H }, right: { kind: "rel", s: H } }, NOW).right, { kind: "abs", t: NOW });
// A span counts from the other input.
assert.deepEqual(rangeOf({ kind: "abs", t: 1000 }, { kind: "rel", s: 500 }), [1000, 1500]);
assert.equal(rangeOf({ kind: "rel", s: -1 }, { kind: "rel", s: 1 }), null);

// Committing: one span at a time; two times out of order swap.
const t1 = Date.UTC(2026, 9, 20) / 1000, t2 = Date.UTC(2026, 9, 22) / 1000;
assert.match(commit(def, "right", "+3 days", NOW).error, /Only one/);
assert.match(commit(def, "left", "soon", NOW).error, /isn't a time/);
let p = commit({ left: { kind: "abs", t: t1 }, right: { kind: "abs", t: t2 } }, "left", "2026-10-23", NOW);
assert.deepEqual(p, { left: { kind: "abs", t: t2 }, right: { kind: "abs", t: Date.UTC(2026, 9, 23) / 1000 } });
p = commit({ left: { kind: "abs", t: t1 }, right: { kind: "abs", t: t2 } }, "right", "2026-10-19", NOW);
assert.deepEqual(p, { left: { kind: "abs", t: Date.UTC(2026, 9, 19) / 1000 }, right: { kind: "abs", t: t1 } });
// (A span never needs it: the left's is back from the right.)
p = commit({ left: { kind: "abs", t: t1 }, right: { kind: "abs", t: t2 } }, "left", "7 days", NOW);
assert.deepEqual(p, { left: { kind: "rel", s: -7 * D }, right: { kind: "abs", t: t2 } });
p = commit(def, "left", "2026-10-25", NOW);
assert.deepEqual(p, { left: { kind: "abs", t: NOW }, right: { kind: "abs", t: Date.UTC(2026, 9, 25) / 1000 } });

// The tiles: the last catalogue publishing each; one dropped later gone.
assert.equal(keyOf(6, 50 * 64 + 30), "6/30/50");
const cats = mergeCatalogs(mergeCatalogs([], [{ n: 1, t: 100, first: true, z3: [0], z6: [65, 130] }]), [{ n: 1, t: 100, z6: [] }, { n: 2, t: 200, z3: [], z6: [65], z6_gone: [130] }]);
assert.deepEqual(cats.map((c) => c.n), [1, 2]);
assert.deepEqual([...tileTimes(cats, 6)], [["6/1/1", { t: 200, n: 2, first: false }]]);
assert.deepEqual([...tileTimes(cats, 3)], [["3/0/0", { t: 100, n: 1, first: true }]]);
// A tile's last publish (#685): its own at z3 or z6, a deeper one's z6 tile's, the root's newest z3.
const t3 = new Map([["3/0/0", { t: 100, n: 1 }], ["3/7/7", { t: 300, n: 3 }]]), t6 = new Map([["6/1/1", { t: 200, n: 2 }]]);
assert.equal(publishedAt(t3, t6, { z: 6, x: 1, y: 1 }).n, 2);
assert.equal(publishedAt(t3, t6, { z: 10, x: 16, y: 31 }).n, 2);
assert.equal(publishedAt(t3, t6, { z: 3, x: 0, y: 0 }).n, 1);
assert.equal(publishedAt(t3, t6, { z: 0, x: 0, y: 0 }).n, 3);
assert.equal(publishedAt(t3, t6, { z: 2, x: 0, y: 0 }).n, 1);
assert.equal(publishedAt(t3, t6, { z: 6, x: 9, y: 9 }), null);

// The colour map: its ends, clamped beyond; hidden outside with the box ticked.
assert.equal(rampColour(0, 0, 10), "rgb(63,79,168)");
assert.equal(rampColour(10, 0, 10), "rgb(246,233,160)");
assert.equal(rampColour(-5, 0, 10), rampColour(0, 0, 10));
assert.equal(rampColour(99, 0, 10), rampColour(10, 0, 10));
assert.equal(RAMP.length, 6);
const times = new Map([["6/0/0", { t: 5 }], ["6/1/0", { t: 50 }], ["6/2/0", { t: -50 }]]);
let r = paint(times, 0, 10, false);
assert.equal(r.inside, 1);
assert.equal(r.paint.size, 3);
assert.equal(r.paint.get("6/1/0"), rampColour(10, 0, 10));
r = paint(times, 0, 10, true);
assert.deepEqual([...r.paint.keys()], ["6/0/0"]);

// The timeline: the range its centre 80%.
const [a, b] = timelineSpan(1000, 1800);
assert.equal((1800 - 1000) / (b - a), 0.8);
assert.equal(1000 - a, b - 1800);
// Labels never overlap; the latest kept first.
assert.deepEqual(labelled([0, 10, 20, 100], 30), [2, 3]);
assert.deepEqual(labelled([0, 50, 100], 30), [0, 1, 2]);
assert.deepEqual(labelled([0, 40, 80], [30, 80, 30]), [0, 2]);
assert.deepEqual(labelled([0, 40, 80], 50, 6, [0]), [0, 2]);
// The axis: times of day up to two days, days past them.
assert.ok(axisTicks(NOW - 6 * H, NOW).every((t) => /^\d\d:\d\d$/.test(t.label)));
const days = axisTicks(NOW - 10 * D, NOW, 6);
assert.ok(days.length >= 2 && days.length <= 6 && days.every((t) => /^\d+ [A-Z][a-z]{2}$/.test(t.label)), JSON.stringify(days));

console.log("catalogs.test.mjs: ok");
