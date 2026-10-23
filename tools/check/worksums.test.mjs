// node --test tools/check/worksums.test.mjs: the build page's sums (web/work/sums.js): the
// headline's share of the whole build's work, the schedule's lanes worth a row, its past jobs,
// slots and Macs, and the wait while this Mac hasn't read the build's state.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { busyLanes, isSlotOf, laneGroups, notRead, packRuns, pastFrom, pastRuns, READ_RETRY_MS, READ_WAIT_MS, slotName, slotNo, wholeProgress } from '../../web/work/sums.js';

const line = (step, done, total) => ({ steps: [step], done, total, left: total - done, items: [{ step, done, total, left: total - done }] });

test('headline: the work done of the whole, each step weighed by its time a target', () => {
  // Terrain all done (10 × 60 s), roads 58 of 284 (× 300 s), the rest of roads left in the forecast.
  const lines = [line('terrain', 10, 10), line('unit', 58, 284)];
  const fc = { each_s: { terrain: 60, unit: 300 }, steps: [{ step: 'unit', work_s: 226 * 300 }] };
  const p = wholeProgress(lines, fc);
  assert.equal(p.done_s, 600 + 58 * 300);
  assert.equal(p.left_s, 226 * 300);
  assert.equal(p.pct, Math.floor(((600 + 17400) / (600 + 17400 + 67800)) * 100));
});

test('headline: never 100% while any stage has work left', () => {
  // The roads all built, a cheap stage still to come: 99%, not 100.
  const lines = [line('unit', 1000, 1000), line('marks', 0, 1)];
  const fc = { each_s: { unit: 1000, marks: 1 }, steps: [{ step: 'marks', work_s: 1 }] };
  assert.equal(wholeProgress(lines, fc).pct, 99);
  // A stage not finished though the forecast lists nothing left: still not 100.
  assert.equal(wholeProgress([line('unit', 5, 10)], { each_s: { unit: 1 }, steps: [] }).pct, 99);
});

test('headline: 100% when every stage is done and nothing is left', () => {
  const lines = [line('unit', 10, 10), line('marks', 1, 1)];
  assert.equal(wholeProgress(lines, { each_s: { unit: 5, marks: 1 }, steps: [] }).pct, 100);
});

test('headline: none without a forecast that weighs the steps', () => {
  assert.equal(wholeProgress([line('unit', 1, 2)], null), null);
  assert.equal(wholeProgress([line('unit', 1, 2)], { steps: [] }), null);
});

test('lanes: only those with a bar in the window, in their order', () => {
  const lanes = {
    'm4': [{ from: 100, until: 200 }],
    'm4 #2': [],
    'm4 #3': [{ from: 10, until: 50 }], // ended before the window
    'm1': [{ from: 900, until: 1200 }], // starts in it
    'm1 #2': [{ from: 5000, until: 6000 }], // after it
  };
  assert.deepEqual(busyLanes(['m4', 'm4 #2', 'm4 #3', 'm1', 'm1 #2', 'gone'], lanes, 60, 1000), ['m4', 'm1']);
});

test('not read yet: a quiet state and a retry every 2 s, an error after a minute', () => {
  const a = notRead(false, 1000, 3000);
  assert.equal(a.error, null);
  assert.equal(a.retryMs, READ_RETRY_MS);
  assert.equal(a.state, "reading the build's state…");
  assert.match(notRead(true, 0, READ_WAIT_MS - 1).state, /stale/);
  const z = notRead(true, 0, READ_WAIT_MS);
  assert.equal(z.retryMs, null);
  assert.equal(z.state, null);
  assert.match(z.error, /hasn't read the build's state in 60 s/);
  // (Counted from the first 503 in a row: still an error, no quick retry, minutes on.)
  const later = notRead(false, 0, 5 * READ_WAIT_MS);
  assert.equal(later.retryMs, null);
  assert.match(later.error, /in 300 s/);
});

test("slots: a worker's slot of a host, and a job's lane by its slot index", () => {
  assert.equal(slotNo('m4', 'm4'), 1);
  assert.equal(slotNo('m4 #3', 'm4'), 3);
  assert.equal(slotNo('m4 #1', 'm4'), null); // (#1 is the bare name)
  assert.equal(slotNo('m4x', 'm4'), null);
  assert.equal(slotNo('m4 #2x', 'm4'), null);
  assert.ok(isSlotOf('m4 #2', 'm4') && !isSlotOf('m1', 'm4'));
  assert.equal(slotName('m4', 0), 'm4');
  assert.equal(slotName('m4', 2), 'm4 #3');
});

test("past jobs: each job's end since `from`, by start, once each", () => {
  const ev = [
    { kind: 'end', seq: 5, t: 500, secs: 100, step: 'unit', worker: 'm4 #2', targets: ['a', 'b'], ok: true, what: 'Roads: 2 tiles' },
    { kind: 'start', seq: 6, t: 510, step: 'unit', worker: 'm4' },
    { kind: 'end', seq: 7, t: 300, secs: 250, step: 'depth', worker: 'm1', targets: ['c'], ok: false },
    { kind: 'end', seq: 5, t: 500, secs: 100, step: 'unit', worker: 'm4 #2', targets: ['a', 'b'] }, // the same again
    { kind: 'end', seq: 8, t: 90, secs: 10, step: 'unit', worker: 'm4' }, // before `from`
    { kind: 'end', seq: 9, t: 600, step: 'unit', worker: 'm4' }, // no time
  ];
  const r = pastRuns(ev, 100);
  assert.deepEqual(r.map((x) => [x.lane, x.step, x.from, x.until, x.n, x.ok]), [['m1', 'depth', 50, 300, 1, false], ['m4 #2', 'unit', 400, 500, 2, true]]);
});

test('past jobs: runs of one step closer than a pixel packed into one segment', () => {
  const run = (step, from, until, n = 1, ok = true) => ({ step, from, until, n, ok });
  const p = packRuns([run('unit', 0, 10), run('unit', 11, 20, 2), run('unit', 40, 50), run('depth', 50, 60, 1, false), run('depth', 60, 70)], 2);
  assert.deepEqual(p.map((x) => [x.step, x.from, x.until, x.n, x.ok, x.parts.length]), [['unit', 0, 20, 3, true, 2], ['unit', 40, 50, 1, true, 1], ['depth', 50, 70, 2, false, 2]]);
});

test("lanes grouped by Mac: the hosts in order, their slots by number, a gone Mac's after", () => {
  const g = laneGroups(['m1 #2', 'm4 #3', 'm4', 'old #2', 'm1', 'old'], ['m4', 'm1']);
  assert.deepEqual(g, [{ host: 'm4', lanes: ['m4', 'm4 #3'] }, { host: 'm1', lanes: ['m1', 'm1 #2'] }, { host: 'old', lanes: ['old', 'old #2'] }]);
  // (A host's name that's another's start isn't its.)
  assert.deepEqual(laneGroups(['m4x', 'm4'], ['m4']), [{ host: 'm4', lanes: ['m4'] }, { host: 'm4x', lanes: ['m4x'] }]);
});

test('past history: asked back a chunk at a time until it reaches the time', () => {
  assert.equal(pastFrom({ seq: 18000, t: 1000 }, 500), 12999);
  assert.equal(pastFrom({ seq: 300, t: 1000 }, 500), 0);
  assert.equal(pastFrom({ seq: 300, t: 400 }, 500), null);
  assert.equal(pastFrom({ seq: 1, t: 1000 }, 500), null);
  assert.equal(pastFrom(null, 500), null);
});
