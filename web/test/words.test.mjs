// A Mac's speed in words (web/buildpage/words.js speedWords, task #1418): the lead's measured
// instructions a second of each kind of core, over the steps it's measured on.
import { test } from "node:test";
import assert from "node:assert/strict";
import { speedWords } from "../buildpage/words.js";

test("both kinds of core, fitted with the other Macs' steps", () => {
  const s = { p: { ginsn_s: 8.123, steps: ["bldtiles", "catalog"], runs: 31, linked: true }, e: { ginsn_s: 2.91, steps: ["catalog"], runs: 4, linked: true } };
  assert.equal(speedWords(s), "8.1 G instructions/s a performance core, 2.9 an efficiency core · over 2 steps, fitted with the other Macs' (31 clean runs)");
});

test("one kind alone, over its own steps", () => {
  const s = { e: { ginsn_s: 3, steps: ["gc"], runs: 1, linked: false } };
  assert.equal(speedWords(s), "3.0 G instructions/s an efficiency core · over 1 step of its own, none shared with another Mac (1 clean run)");
});

test("none measured: nothing said", () => {
  assert.equal(speedWords(undefined), null);
  assert.equal(speedWords({}), null);
});
