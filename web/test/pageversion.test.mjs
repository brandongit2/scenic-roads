// An open build page reloads into a newly published version, and only then (web/buildpage/version.js,
// docs/coordinator.md §7, The page's version).
import { test } from "node:test";
import assert from "node:assert/strict";
import { versionWatch, HEADER } from "../buildpage/version.js";

function watch(served) {
  const w = { reloads: 0, kept: null };
  const seen = versionWatch(served, () => w.reloads++, { get: () => w.kept, set: (v) => (w.kept = v) });
  w.answer = (v) => seen(new Headers(v == null ? {} : { [HEADER]: v }));
  return w;
}

test("the same version, or none, doesn't reload", () => {
  const w = watch("aaa");
  w.answer("aaa");
  w.answer(null); // (the lead away: a proxy's 502, an error without the coordinator's headers)
  w.answer("");
  assert.equal(w.reloads, 0);
});

test("a different version reloads, once per version", () => {
  const w = watch("aaa");
  w.answer("bbb");
  assert.equal(w.reloads, 1);
  w.answer("bbb"); // (A reload that served the same old page: not again for it.)
  assert.equal(w.reloads, 1);
  w.answer("ccc");
  assert.equal(w.reloads, 2);
});

test("a page served without a version never reloads", () => {
  const w = watch(undefined);
  w.answer("bbb");
  assert.equal(watch("").answer("bbb"), undefined);
  assert.equal(w.reloads, 0);
});

test("not while the page says it isn't ready (a field being typed in): the next answer asks again", () => {
  const w = { reloads: 0, kept: null, ready: false };
  const seen = versionWatch("aaa", () => w.reloads++, { get: () => w.kept, set: (v) => (w.kept = v) }, () => w.ready);
  seen(new Headers({ [HEADER]: "bbb" }));
  assert.equal(w.reloads, 0);
  assert.equal(w.kept, null);
  w.ready = true;
  seen(new Headers({ [HEADER]: "bbb" }));
  assert.equal(w.reloads, 1);
});
