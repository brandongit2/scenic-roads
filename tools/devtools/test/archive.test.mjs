// Archived items are read-only (#1229): the API refuses a change or a new event on one (unarchiving
// is the one change), nothing goes into an archived folder, and no item that isn't archived is ever
// inside one: archiving a folder holding such items is refused unless `cascade` (what the page sends
// once you've confirmed), which archives them with it. Run: node --test tools/devtools/test/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { openStore } from "../store.mjs";
import { openJobs } from "../jobs.mjs";
import { makeServer } from "../server.mjs";

// A board of its own on a free port, in a temporary folder; `t.after` closes and removes it.
async function board(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-arch-"));
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
  const dump = () => JSON.stringify([store.list("items"), store.list("events")]);
  // A refusal: 400 (or `status`), its message matching `says`, and nothing written.
  const refused = async (p, says, status = 400) => {
    const before = dump();
    const r = await p();
    assert.equal(r.status, status, JSON.stringify(r.body));
    assert.match(r.body.error.message, says);
    assert.equal(dump(), before, "nothing written");
  };
  const arch = (id) => !!store.get("items", id).archived;
  const evs = (id) => store.list("events").filter((e) => e.data.item === id && e.data.kind === "archive").map((e) => e.data.text);
  return { store, call, add, refused, arch, evs, patch: (id, b) => call("PATCH", `/api/c/items/${id}`, b) };
}

// A folder (#1) holding a task (#2) and a folder (#3) holding a task (#4).
async function tree(b) {
  const f = await b.add({ kind: "folder", text: "Outer" });
  const t = await b.add({ text: "Task", parent: f });
  const g = await b.add({ kind: "folder", text: "Inner", parent: f });
  const u = await b.add({ text: "Deep", parent: g });
  return { f, t, g, u };
}

test("archiving a folder holding items that aren't archived: refused, naming them, unless cascade", async (t) => {
  const b = await board(t);
  const { f, t: t2, g, u } = await tree(b);
  await b.refused(() => b.patch(f, { archived: true }), /archived: #1 "Outer" holds 3 item\(s\) that aren't archived \(#2, #3, #4\): archive those first/);
  await b.refused(() => b.patch(f, { cascade: true }), /cascade: goes with archived/);
  await b.refused(() => b.patch(f, { archived: true, cascade: "yes" }), /cascade: must be true or false/);
  await b.refused(() => b.call("PUT", `/api/c/items/${f}`, { kind: "folder", text: "Outer", parent: "", archived: true, cascade: true }), /cascade: is not a field you can send here/);
  const r = await b.patch(f, { archived: true, archivedAt: "2026-11-12T10:00:00Z", cascade: true });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  for (const id of [f, t2, g, u]) {
    assert.equal(b.arch(id), true, id);
    assert.equal(b.store.get("items", id).archivedAt, "2026-11-12T10:00:00.000Z", "archived at the same time");
  }
  assert.deepEqual(b.evs(f), ["Archived, with the 3 inside (by curl)"]);
  // (Without cascade, once what's inside is archived, the folder goes too; archivedAt is the server's then.)
  const h = await b.add({ kind: "folder", text: "Second" });
  const x = await b.add({ text: "x", parent: h });
  assert.equal((await b.patch(x, { archived: true })).status, 200);
  assert.equal((await b.patch(h, { archived: true })).status, 200);
  assert.match(b.store.get("items", h).archivedAt, /^\d{4}-\d\d-\d\dT/);
  assert.deepEqual(b.evs(h), ["Archived (by curl)"]);
});

test("an archived item is read-only: no PATCH, PUT or event, but unarchiving", async (t) => {
  const b = await board(t);
  const { f, t: t2, u } = await tree(b);
  await b.add({ text: "other" });
  assert.equal((await b.patch(f, { archived: true, cascade: true })).status, 200);
  const ev = (await b.call("POST", "/api/c/events", { item: f, kind: "note", text: "before" })); // refused: archived
  assert.equal(ev.status, 400);
  for (const body of [{ text: "new" }, { status: "done" }, { priority: 3 }, { archived: false, text: "x" }, { parent: "" }, { archivedAt: "2026-11-12T10:00:00Z" }, { archived: true }]) {
    await b.refused(() => b.patch(t2, body), /#2 "Task" is archived: it can't be changed \(archived items are read-only\); the one change taken is unarchiving it, PATCH \{"archived": false\}/);
  }
  await b.refused(() => b.call("PUT", `/api/c/items/${t2}`, { kind: "task", text: "x", parent: f, status: "", assigned: "" }), /is archived: it can't be changed/);
  await b.refused(() => b.call("POST", "/api/c/events", { item: u, kind: "note", text: "x" }), /item: #4 "Deep" is archived: its history is read-only \(unarchive it first\)/);
  // (An event already in its history can't be edited either; the board's own events can.)
  const old = b.store.list("events").find((e) => e.data.item === f);
  await b.refused(() => b.call("PATCH", `/api/c/events/${old.id}`, { text: "edited" }), /item: #1 "Outer" is archived: its history is read-only/);
  assert.equal((await b.call("POST", "/api/c/events", { item: "", kind: "note", text: "about the board" })).status, 200);
  // (Deleting is no edit: an archived item can still be deleted.)
  const lone = await b.add({ text: "lone" });
  assert.equal((await b.patch(lone, { archived: true })).status, 200);
  assert.equal((await b.call("DELETE", `/api/c/items/${lone}`)).status, 200);
});

test("nothing goes into an archived folder, at any depth: new or moved, named", async (t) => {
  const b = await board(t);
  const { f, g } = await tree(b);
  const out = await b.add({ text: "Outside" });
  assert.equal((await b.patch(f, { archived: true, cascade: true })).status, 200);
  await b.refused(() => b.call("POST", "/api/c/items", { text: "new", parent: f }), /parent: folder #1 "Outer" is archived: nothing goes into an archived folder/);
  await b.refused(() => b.call("POST", "/api/c/items", { text: "new", parent: g }), /parent: folder #3 "Inner" is archived: nothing goes into an archived folder/);
  await b.refused(() => b.patch(out, { parent: g }), /parent: folder #3 "Inner" is archived/);
  // (A folder that isn't archived itself but sits in an archived one: only reachable by a file from
  // before these rules, which the migration mends; named by the archived one around it.)
  b.store.put("items", g, { ...b.store.get("items", g), archived: false });
  await b.refused(() => b.patch(out, { parent: g }), /parent: folder #3 "Inner" is inside archived folder #1 "Outer": nothing goes into an archived folder/);
});

test("unarchiving inside an archived folder: refused, naming it, unless cascade unarchives the folders around", async (t) => {
  const b = await board(t);
  const { f, t: t2, g, u } = await tree(b);
  assert.equal((await b.patch(f, { archived: true, cascade: true })).status, 200);
  await b.refused(() => b.patch(u, { archived: false }), /archived: #4 "Deep" is inside archived folder #3 "Inner": unarchive that first, or send "cascade": true/);
  const r = await b.patch(u, { archived: false, archivedAt: null, cascade: true });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.deepEqual([f, t2, g, u].map(b.arch), [false, true, false, false], "the folders around it, and it: what else was inside stays archived");
  assert.equal("archivedAt" in b.store.get("items", g), false);
  assert.deepEqual(b.evs(g).at(-1), "Unarchived, with #4 inside (by curl)");
  assert.deepEqual(b.evs(u), ["Unarchived (by curl)"]);
  // (Unarchived from the top down, one at a time, no cascade is needed; null, which removes the
  // field, unarchives as false does.)
  assert.equal((await b.patch(t2, { archived: null, archivedAt: null })).status, 200);
  assert.equal("archived" in b.store.get("items", t2), false);
  // (And the board is never left with an item that isn't archived inside an archived folder.)
  const items = new Map(b.store.list("items").map((d) => [d.id, d.data]));
  for (const [id, d] of items) if (!d.archived) for (let p = d.parent; p; p = items.get(p).parent) assert.equal(!!items.get(p).archived, false, `${id} in ${p}`);
});

test("a deleted item isn't put back into a folder archived since", async (t) => {
  const b = await board(t);
  const f = await b.add({ kind: "folder", text: "F" });
  const x = await b.add({ text: "x", parent: f });
  assert.equal((await b.call("DELETE", `/api/c/items/${x}`)).status, 200);
  assert.equal((await b.patch(f, { archived: true })).status, 200);
  await b.refused(() => b.call("POST", `/api/c/items/${x}/restore`), /its folder #1 "F" is archived now/, 409);
});
