// A task's priority with a fraction (#1100): placed by priority.mjs, by the server on a POST or PATCH
// as by the page. Run: node --test tools/devtools/test/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { openStore } from "../store.mjs";
import { openJobs } from "../jobs.mjs";
import { makeServer } from "../server.mjs";
import { placePriority, isOpen } from "../priority.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));

// A board of its own on a free port, in a temporary folder; `t.after` closes and removes it.
async function board(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-prio-"));
  const store = openStore(path.join(dir, "t.db"));
  const jobs = openJobs(path.join(dir, "jobs.db"));
  const server = makeServer({ board: store, jobs });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const base = `http://127.0.0.1:${server.address().port}`;
  t.after(() => { server.close(); server.closeAllConnections?.(); store.close(); jobs.close(); fs.rmSync(dir, { recursive: true, force: true }); });
  const call = async (method, url, body) => {
    const r = await fetch(base + url, { method, headers: { "content-type": "application/json", "user-agent": "curl/8" }, body: body === undefined ? undefined : JSON.stringify(body) });
    return { status: r.status, body: await r.json() };
  };
  const add = async (fields) => { const r = await call("POST", "/api/c/items", { text: "t", ...fields }); assert.equal(r.status, 200, JSON.stringify(r.body)); return r.body.id; };
  const prio = (id) => store.get("items", id).priority;
  const events = (id) => store.list("events").filter((e) => e.data.item === id && e.data.kind === "priority").map((e) => e.data.text);
  return { store, call, add, prio, events, patch: (id, b) => call("PATCH", `/api/c/items/${id}`, b) };
}

test("placePriority: a whole number as it is; a fraction rounded up, running into the next ones as far as a gap", () => {
  const others = [{ id: "a", priority: 5 }, { id: "b", priority: 6 }, { id: "c", priority: 7 }, { id: "d", priority: 9 }, { id: "e" }];
  assert.deepEqual(placePriority(6, others), { to: 6, moves: [] });
  assert.deepEqual(placePriority(5.5, others), { to: 6, moves: [{ id: "b", from: 6, to: 7 }, { id: "c", from: 7, to: 8 }] });
  assert.deepEqual(placePriority(7.5, others), { to: 8, moves: [] }, "8 is free: nothing moves");
  assert.deepEqual(placePriority(0.5, others), { to: 1, moves: [] });
  assert.deepEqual(placePriority(4.5, [{ id: "x", priority: 5 }, { id: "y", priority: 5 }, { id: "z", priority: 6 }]).moves,
    [{ id: "x", from: 5, to: 6 }, { id: "y", from: 5, to: 6 }, { id: "z", from: 6, to: 7 }], "tasks sharing a number move together");
  assert.equal(isOpen({ status: "" }), true);
  assert.equal(isOpen({ status: "working" }), true);
  for (const t of [{ status: "done" }, { status: "abandoned" }, { status: "", archived: true }]) assert.equal(isOpen(t), false, JSON.stringify(t));
  assert.equal(isOpen({ status: "" }, true), false, "inside an archived folder");
});

test("PATCH with a fraction: the task takes it rounded up, the open tasks it runs into move down one, one event for the cascade", async (t) => {
  const b = await board(t);
  const [p5, p6, p7, p9, w8] = [await b.add({ priority: 5 }), await b.add({ priority: 6 }), await b.add({ priority: 7, status: "working" }), await b.add({ priority: 9 }), await b.add({ priority: 12 })];
  const r = await b.patch(w8, { priority: 5.5, note: "moved up" });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  const num = (id) => b.store.get("items", id).num;
  assert.deepEqual(r.body.moved, [{ id: p6, num: num(p6), from: 6, to: 7 }, { id: p7, num: num(p7), from: 7, to: 8 }]);
  assert.equal(r.body.priority, 6);
  assert.deepEqual([b.prio(p5), b.prio(p6), b.prio(p7), b.prio(p9), b.prio(w8)], [5, 7, 8, 9, 6]);
  assert.equal(b.store.get("items", w8).note, "moved up", "the rest of the PATCH is written too");
  // One event for the whole cascade, on the task placed; the ones it pushed get none.
  assert.deepEqual(b.events(w8), [`Priority 12 → 6: asked for 5.5; 2 others moved down one, from priority 6–7 (#${num(p6)}, #${num(p7)}) (by curl)`]);
  assert.deepEqual([b.events(p5), b.events(p6), b.events(p7), b.events(p9)], [[], [], [], []]);
  assert.equal(b.store.list("events").filter((e) => e.data.kind === "priority").length, 1);
});

test("a whole number is stored as given: nothing moves, nothing more in the answer", async (t) => {
  const b = await board(t);
  const [a, c] = [await b.add({ priority: 6 }), await b.add({ priority: 3 })];
  const r = await b.patch(c, { priority: 6 });
  assert.equal(r.status, 200);
  assert.equal("moved" in r.body, false);
  assert.deepEqual([b.prio(a), b.prio(c)], [6, 6]);
  assert.deepEqual(b.events(c), []);
  assert.equal((await b.patch(c, { note: "only a note" })).body.moved, undefined, "a PATCH without priority places nothing");
});

test("ties: tasks sharing a number all move down, and so do those they run into", async (t) => {
  const b = await board(t);
  const [x, y, z, w] = [await b.add({ priority: 6 }), await b.add({ priority: 6 }), await b.add({ priority: 7 }), await b.add({})];
  const r = await b.patch(w, { priority: 5.5 });
  assert.deepEqual(r.body.moved.map((m) => [m.id, m.from, m.to]), [[x, 6, 7], [y, 6, 7], [z, 7, 8]]);
  assert.deepEqual([b.prio(x), b.prio(y), b.prio(z), b.prio(w)], [7, 7, 8, 6]);
  assert.match(b.events(w)[0], /^Priority 6: asked for 5\.5; 3 others moved down one, from priority 6–7 \(#\d+, #\d+, #\d+\) \(by curl\)$/, "it had none before");
  assert.deepEqual([b.events(x), b.events(y), b.events(z)], [[], [], []]);
});

test("done, abandoned and archived tasks, and tasks inside an archived folder, keep their numbers and aren't run into", async (t) => {
  const b = await board(t);
  const folder = await b.add({ kind: "folder" });
  const inner = await b.add({ kind: "folder", parent: folder });
  const done = await b.add({ priority: 6, status: "done" });
  const gone = await b.add({ priority: 6, status: "abandoned" });
  const arch = await b.add({ priority: 6 });
  const deep = await b.add({ priority: 6, parent: inner });
  const open6 = await b.add({ priority: 6, status: "review" });
  const done7 = await b.add({ priority: 7, status: "done" });
  const open8 = await b.add({ priority: 8 });
  for (const id of [folder, arch]) assert.equal((await b.patch(id, { archived: true, cascade: true })).status, 200);
  const r = await b.patch(await b.add({}), { priority: 5.5 });
  assert.deepEqual(r.body.moved.map((m) => m.id), [open6], "the open 6 moves to 7, a closed task's number; 8 is past it");
  assert.deepEqual([done, gone, arch, deep, open6, done7, open8].map(b.prio), [6, 6, 6, 6, 7, 7, 8]);
  // (A closed task given a fraction is placed too: it runs into the open ones.)
  const r2 = await b.patch(done, { priority: 6.5 });
  assert.deepEqual([r2.body.priority, r2.body.moved.map((m) => [m.id, m.to])], [7, [[open6, 8], [open8, 9]]]);
});

test("POST with a fraction is placed the same way, in its answer too", async (t) => {
  const b = await board(t);
  const [a, c] = [await b.add({ priority: 1 }), await b.add({ priority: 2 })];
  const r = await b.call("POST", "/api/c/items", { text: "New", priority: 0.5 });
  assert.equal(r.status, 200);
  assert.equal(r.body.priority, 1);
  assert.deepEqual(r.body.moved.map((m) => [m.id, m.from, m.to]), [[a, 1, 2], [c, 2, 3]]);
  assert.deepEqual([b.prio(r.body.id), b.prio(a), b.prio(c)], [1, 2, 3]);
  assert.deepEqual(b.events(r.body.id), [`Priority 1: asked for 0.5; 2 others moved down one, from priority 1–2 (#${b.store.get("items", a).num}, #${b.store.get("items", c).num}) (by curl)`]);
  assert.equal("moved" in (await b.call("POST", "/api/c/items", { text: "Whole", priority: 2 })).body, false);
});

test("a refused write moves nothing", async (t) => {
  const b = await board(t);
  const [a, c] = [await b.add({ priority: 6 }), await b.add({})];
  const before = JSON.stringify([b.store.list("items"), b.store.list("events")]);
  const r = await b.patch(c, { priority: 5.5, parent: "nowhere" });
  assert.equal(r.status, 400);
  assert.equal(JSON.stringify([b.store.list("items"), b.store.list("events")]), before);
  assert.equal(b.prio(a), 6);
});

test("a priority that isn't a number from 0, or on a folder, is refused, nothing moved; one placed on its own number says so", async (t) => {
  const b = await board(t);
  const [a, c] = [await b.add({ priority: 3 }), await b.add({ priority: 4 })];
  const before = JSON.stringify([b.store.list("items"), b.store.list("events")]);
  for (const bad of [{ priority: "2.5" }, { priority: -0.5 }, { priority: true }, { priority: 2.5, status: "nope" }]) {
    assert.equal((await b.patch(a, bad)).status, 400, JSON.stringify(bad));
  }
  assert.equal((await b.call("POST", "/api/c/items", { kind: "folder", text: "f", priority: 1.5 })).status, 400);
  assert.equal(JSON.stringify([b.store.list("items"), b.store.list("events")]), before);
  const r = await b.patch(a, { priority: 2.5 });
  assert.deepEqual([r.body.priority, r.body.moved, b.prio(c)], [3, [], 4], "4 is past 3: nothing moves");
  assert.deepEqual(b.events(a), ["Priority 3: asked for 2.5 (by curl)"]);
});

test("PUT with a fraction is placed too", async (t) => {
  const b = await board(t);
  const [a, c] = [await b.add({ priority: 2 }), await b.add({ priority: 9 })];
  const r = await b.call("PUT", `/api/c/items/${c}`, { kind: "task", text: "t", parent: "", status: "", assigned: "Subagent", priority: 1.5 });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.deepEqual([r.body.priority, r.body.moved.map((m) => [m.id, m.from, m.to])], [2, [[a, 2, 3]]]);
  assert.deepEqual([b.prio(a), b.prio(c)], [3, 2]);
});

test("the page places priorities with the same module, not a copy", () => {
  const page = fs.readFileSync(path.join(here, "../public/task-board/index.html"), "utf8");
  assert.match(page, /import \{[^}]*placePriority[^}]*\} from "\.\/priority\.mjs"/);
  assert.doesNotMatch(page, /priority \+ 1/, "no cascade of its own");
});
