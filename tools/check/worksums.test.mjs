// node --test tools/check/worksums.test.mjs: the build page's sums (web/work/sums.js): the
// headline's share of the whole build's work, the schedule's lanes worth a row, and the wait while
// this Mac hasn't read the build's state.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { busyLanes, notRead, READ_RETRY_MS, READ_WAIT_MS, wholeProgress } from '../../web/work/sums.js';

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
});
