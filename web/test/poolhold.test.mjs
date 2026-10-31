// A pool member's Pause/Resume (web/work/pool.js holdControl, task #873): from its status's hold
// stage (control::HoldView), in touch or not, and a press under way.
import { test } from "node:test";
import assert from "node:assert/strict";
import { holdControl } from "../work/pool.js";

const m4 = { member: "id-m4", host: "m4" }, me = { member: "id-m6", host: "m6", me: true };

test("building: Pause, asking a hold", () => {
  const c = holdControl(m4, null, true, undefined);
  assert.deepEqual([c.label, c.held, c.disabled], ["Pause", true, false]);
  assert.match(c.title, /^Pause m4 alone/);
  assert.match(holdControl(me, null, true).title, /this Mac/);
});

test("pausing and paused: Resume, letting it go on", () => {
  for (const stage of ["pausing", "paused"]) {
    const c = holdControl(m4, { stage, pause: true, said: "…" }, true);
    assert.deepEqual([c.label, c.held, c.disabled], ["Resume", false, false]);
  }
});

test("asked and resuming: disabled, the stage's words as its tip", () => {
  const a = holdControl(me, { stage: "asked", pause: true, said: "Pause asked" }, true);
  assert.deepEqual([a.label, a.disabled, a.title], ["Pausing…", true, "Pause asked"]);
  const r = holdControl(m4, { stage: "resuming", pause: false, said: "Resuming: …" }, true);
  assert.deepEqual([r.label, r.disabled], ["Resuming…", true]);
});

test("a press under way: disabled; its answer's words don't hold the button", () => {
  assert.equal(holdControl(m4, null, true, true).disabled, true);
  assert.equal(holdControl(m4, null, true, "m4 is paused").disabled, false);
});

test("out of touch: the hold is queued on the NAS, said in its tip", () => {
  const c = holdControl(m4, null, false);
  assert.equal(c.disabled, false);
  assert.match(c.title, /out of touch: this is kept on the NAS/);
  assert.doesNotMatch(holdControl(me, null, false).title, /out of touch/);
});
