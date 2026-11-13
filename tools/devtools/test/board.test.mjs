// The task board's API: what it assigns, and that every invalid shape is refused with a 400 that
// says why and writes nothing (#672). Run: node --test tools/devtools/test/*.test.mjs
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { openStore } from "../store.mjs";
import { openJobs } from "../jobs.mjs";
import { makeServer } from "../server.mjs";
import { STATUSES, ASSIGNED_SUGGESTIONS, EVENT_KINDS, itemErrors, eventErrors, metaErrors, errorText } from "../schema.mjs";

const dir = fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-test-"));
const store = openStore(path.join(dir, "t.db"));
const jobs = openJobs(path.join(dir, "jobs.db"));
const server = makeServer({ board: store, jobs });
let base;
before(async () => { await new Promise((r) => server.listen(0, "127.0.0.1", r)); base = `http://127.0.0.1:${server.address().port}`; });
after(() => { server.close(); server.closeAllConnections?.(); store.close(); jobs.close(); fs.rmSync(dir, { recursive: true, force: true }); });

// Every way the stored data could break the schema or its references (none expected, ever).
function problems(store) {
  const out = [];
  const items = new Map(store.list("items").map((d) => [d.id, d.data]));
  const nums = new Map();
  for (const [id, d] of items) {
    for (const e of itemErrors(d)) out.push(`items/${id}: ${errorText([e])}`);
    if (d.parent && !items.has(d.parent)) out.push(`items/${id}: parent ${d.parent} doesn't exist`);
    else if (d.parent && items.get(d.parent).kind !== "folder") out.push(`items/${id}: its parent ${d.parent} is a task`);
    if (d.num !== undefined) { if (nums.has(d.num)) out.push(`items/${id}: #${d.num} is also ${nums.get(d.num)}'s`); nums.set(d.num, id); }
    const seen = new Set([id]);
    for (let p = d.parent; p; p = items.get(p)?.parent) { if (seen.has(p)) { out.push(`items/${id}: its parents loop`); break; } seen.add(p); }
  }
  for (const { id, data: d } of store.list("events")) {
    for (const e of eventErrors(d)) out.push(`events/${id}: ${errorText([e])}`);
    if (d.item && !items.has(d.item)) out.push(`events/${id}: item ${d.item} doesn't exist`);
  }
  for (const { id, data: d } of store.list("meta")) for (const e of metaErrors(id, d)) out.push(`meta/${id}: ${errorText([e])}`);
  if ((store.get("meta", "counter")?.next ?? 0) <= store.maxNum()) out.push("meta/counter: next isn't past the highest number");
  return out;
}

async function call(method, url, body, headers = {}) {
  const r = await fetch(base + url, { method, headers: { "content-type": "application/json", "user-agent": "curl/8", ...headers }, body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body) });
  return { status: r.status, body: await r.json() };
}
const post = (col, b) => call("POST", `/api/c/${col}`, b);
const patch = (col, id, b) => call("PATCH", `/api/c/${col}/${id}`, b);
const get = async (col, id) => (await call("GET", `/api/c/${col}/${id}`)).body.data;
const count = () => ({ items: store.list("items").length, events: store.list("events").length, counter: store.get("meta", "counter").next, now: JSON.stringify(store.get("meta", "now")) });
const ISO = /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/;
const refused = async (p, what) => {
  const before = count(), dump = JSON.stringify([store.list("items"), store.list("events")]);
  const r = await p();
  assert.equal(r.status, what.status ?? 400, JSON.stringify(r.body));
  assert.equal(r.body.error.code, what.code ?? "invalid_argument");
  if (what.says) assert.match(r.body.error.message, what.says);
  assert.deepEqual(count(), before, "nothing counted changed");
  assert.equal(JSON.stringify([store.list("items"), store.list("events")]), dump, "nothing written");
  return r.body.error;
};

let folder, t1;
test("POST items: the server assigns num, id, order and added, and keeps the counter", async () => {
  const f = await post("items", { kind: "folder", text: "A folder" });
  assert.equal(f.status, 200);
  folder = f.body.id;
  assert.equal(f.body.num, 1, "a folder is numbered too");
  assert.equal(f.body.id, "i1");
  assert.equal(f.body.order, 1);
  assert.match(f.body.added, ISO);
  const a = await post("items", { text: "First", parent: folder, note: "n", priority: 2 });
  const b = await post("items", { text: "Second", parent: folder, status: "working", assigned: "You" });
  const g = await post("items", { kind: "folder", text: "Another folder" });
  const c = await post("items", { text: "Third" });
  t1 = a.body.id;
  assert.equal(a.body.id, `i${a.body.num}`);
  assert.deepEqual([a.body.num, b.body.num, g.body.num, c.body.num], [2, 3, 4, 5], "tasks and folders from one counter");
  assert.deepEqual([a.body.order, b.body.order, g.body.order, c.body.order], [2, 3, 4, 5], "last in the one order over all items");
  assert.match(a.body.added, ISO);
  assert.deepEqual(await get("items", a.body.id), { kind: "task", text: "First", parent: folder, note: "n", priority: 2, status: "", assigned: "Subagent", num: a.body.num, order: 2, added: a.body.added });
  assert.equal((await get("items", g.body.id)).num, 4);
  assert.equal((await get("meta", "counter")).next, c.body.num + 1);
  // working at birth leaves a status event, dated by the server
  const ev = store.list("events").filter((e) => e.data.item === b.body.id);
  assert.equal(ev.length, 1);
  assert.equal(ev[0].data.kind, "status");
  assert.match(ev[0].data.t, ISO);
});

test("POST items at once: no two share a number or an id", async () => {
  const rs = await Promise.all(Array.from({ length: 30 }, (_, i) => post("items", { text: `Burst ${i}` })));
  assert.ok(rs.every((r) => r.status === 200));
  assert.equal(new Set(rs.map((r) => r.body.num)).size, 30);
  assert.equal(new Set(rs.map((r) => r.body.id)).size, 30);
});

test("a deleted task's number stays retired", async () => {
  const x = await post("items", { text: "Short-lived" });
  await call("DELETE", `/api/c/items/${x.body.id}`);
  const y = await post("items", { text: "Next" });
  assert.equal(y.body.num, x.body.num + 1);
});

const BAD_POSTS = {
  "no text": [{}, /text: is required/],
  "empty text": [{ text: "  " }, /text: must not be empty/],
  "text a number": [{ text: 5 }, /text: must be a string/],
  "text too long": [{ text: "x".repeat(2001) }, /text: is too long/],
  "unknown field": [{ text: "x", colour: "red" }, /colour: is not a field you can send here/],
  "num sent": [{ text: "x", num: 900 }, /num: is set by the server/],
  "id sent": [{ text: "x", id: "mine" }, /id: is set by the server/],
  "order sent": [{ text: "x", order: 3 }, /order: is set by the server/],
  "added sent": [{ text: "x", added: "2026-10-26T08:35:48Z" }, /added: is set by the server/],
  "added as epoch ms": [{ text: "x", added: "1792000000000.0" }, /added: is set by the server/],
  "bad status": [{ text: "x", status: "finished" }, /status: must be one of/],
  "assigned too long": [{ text: "x", assigned: "x".repeat(201) }, /assigned: is too long/],
  "assigned a number": [{ text: "x", assigned: 5 }, /assigned: must be a string/],
  "stage (renamed)": [{ text: "x", stage: "working" }, /stage: is renamed: send "status", not "stage"/],
  "where (renamed)": [{ text: "x", where: "You" }, /where: is renamed: send "assigned", not "where"/],
  "bad kind": [{ text: "x", kind: "epic" }, /kind: must be one of/],
  "parent that isn't there": [{ text: "x", parent: "nope" }, /parent: no item "nope"/],
  "parent not a string": [{ text: "x", parent: 4 }, /parent: must be a string/],
  "priority a string": [{ text: "x", priority: "1" }, /priority: must be a number/],
  "priority negative": [{ text: "x", priority: -1 }, /priority: must be a number from 0/],
  "discuss a string": [{ text: "x", discuss: "yes" }, /discuss: must be true or false/],
  "slot (gone: order is the order)": [{ text: "x", slot: 1 }, /slot: is not a field you can send here/],
  "inside a task": [{ text: "x", parent: "T1" }, /parent: i\d+ is a task; only folders hold items/],
  "note a number": [{ text: "x", note: 1 }, /note: must be a string/],
  "folder with a status": [{ text: "x", kind: "folder", status: "done" }, /status: is only for tasks/],
  "folder with an assigned": [{ text: "x", kind: "folder", assigned: "You" }, /assigned: is only for tasks/],
  "several at once": [{ status: "no", assigned: 1 }, /(?=.*text: is required)(?=.*status: must be one of)(?=.*assigned: must be a string)/],
  "an array": [[], /the body must be a JSON object/],
  "a string": ['"x"', /the body must be a JSON object/],
  "not JSON": ["{", /not valid JSON/],
};
for (const [name, [body, says]] of Object.entries(BAD_POSTS)) test(`POST items refused: ${name}`, () => refused(() => post("items", JSON.parse(JSON.stringify(body).replace("T1", t1))), { says }));

test("PATCH items: edits an existing item only", async () => {
  await refused(() => patch("items", "nope", { text: "x" }), { status: 404, code: "not_found", says: /no item "nope"/ });
  await refused(() => call("PUT", "/api/c/items/nope", { kind: "task", text: "x", parent: "", status: "", assigned: "" }), { status: 404, code: "not_found" });
  const r = await patch("items", t1, { status: "review", priority: 3 });
  assert.equal(r.status, 200);
  assert.equal((await get("items", t1)).status, "review");
  const ev = store.list("events").filter((e) => e.data.item === t1);
  assert.match(ev.at(-1).data.text, /Status Queued → In review \(by curl\)/);
  assert.match(ev.at(-1).data.t, ISO);
});
test("assigned is free text: anything up to 200 characters, the suggestions or not", async () => {
  for (const a of ["Subagent (Fable, low effort)", "", "someone new", "x".repeat(200)]) {
    assert.equal((await patch("items", t1, { assigned: a })).status, 200, a);
    assert.equal((await get("items", t1)).assigned, a);
  }
  assert.equal((await post("items", { text: "Freeform", assigned: "Subagent (Fable, low effort)" })).status, 200);
  // (One line, and not only spaces.)
  await refused(() => patch("items", t1, { assigned: "two\nlines" }), { says: /assigned: must be one line/ });
  await refused(() => patch("items", t1, { assigned: "   " }), { says: /assigned: is only spaces; "" is nobody/ });
});
test("PATCH: null removes an optional field, and never a required one", async () => {
  await patch("items", t1, { note: null, priority: null });
  const d = await get("items", t1);
  assert.equal("note" in d, false);
  assert.equal("priority" in d, false);
  await refused(() => patch("items", t1, { text: null }), { says: /text: is required/ });
  await refused(() => patch("items", t1, { status: null }), { says: /status: is required for a task/ });
});
const BAD_PATCHES = {
  "num": [{ num: 1 }, /num: is set by the server/],
  "id": [{ id: "x" }, /id: is set by the server/],
  "added": [{ added: "2026-01-01T00:00:00Z" }, /added: is set by the server/],
  "unknown field": [{ priorty: 1 }, /priorty: is not a field you can send here/],
  "epoch-ms started": [{ started: "1792000000000.0" }, /started: must be a time as YYYY-MM-DDTHH:MM:SS/],
  "epoch number finished": [{ finished: 1792000000000 }, /finished: must be a time/],
  "date only": [{ started: "2026-10-26" }, /started: must be a time/],
  "an offset": [{ started: "2026-10-26T08:00:00+02:00" }, /started: must be a time/],
  "impossible date": [{ started: "2026-02-30T00:00:00Z" }, /started: must be a time/],
  "bad status": [{ status: "nope" }, /status: must be one of/],
  "assigned too long": [{ assigned: "x".repeat(201) }, /assigned: is too long/],
  "stage (renamed)": [{ stage: "done" }, /stage: is renamed: send "status"/],
  "where (renamed)": [{ where: "You", priority: 2 }, /where: is renamed: send "assigned"/],
  "parent missing": [{ parent: "gone" }, /parent: no item "gone"/],
  "parent itself": [{ parent: "SELF" }, /inside itself/],
  "order a string": [{ order: "1" }, /order: must be a number/],
  "archivedAt alone": [{ archivedAt: "2026-10-26T08:00:00Z" }, /archivedAt: is only for an archived item/],
  "photos": [{ photos: ["x"] }, /photos: must be a list/],
};
for (const [name, [body, says]] of Object.entries(BAD_PATCHES)) test(`PATCH items refused: ${name}`, () => refused(() => patch("items", t1, JSON.parse(JSON.stringify(body).replace("SELF", t1))), { says }));

test("PATCH: times are stored in one format; reorder, archive and the rest go through", async () => {
  const r = await patch("items", t1, { started: "2026-10-26T08:35:48Z", finished: "2026-10-26T09:00:00.5Z", archived: true, archivedAt: "2026-10-26T10:00:00Z", order: 7.5, discuss: true, reply: "ok", photos: ["0123456789abcdef0123456789abcdef"] });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  const d = await get("items", t1);
  assert.equal(d.started, "2026-10-26T08:35:48.000Z");
  assert.equal(d.finished, "2026-10-26T09:00:00.500Z");
  assert.equal(d.archivedAt, "2026-10-26T10:00:00.000Z");
  assert.equal(d.order, 7.5);
  // (Unarchived alone: its time goes with it.)
  assert.equal((await patch("items", t1, { archived: false })).status, 200);
  const u = await get("items", t1);
  assert.equal(u.archived, false);
  assert.equal("archivedAt" in u, false);
});
test("PATCH: a cycle of parents is refused; a move into a folder is not, into a task is", async () => {
  const inner = await post("items", { kind: "folder", text: "Inner", parent: folder });
  await refused(() => patch("items", folder, { parent: inner.body.id }), { says: /inside itself or what is inside it/ });
  assert.equal((await patch("items", t1, { parent: inner.body.id })).status, 200);
  const other = await post("items", { text: "Not a folder" });
  await refused(() => patch("items", t1, { parent: other.body.id }), { says: /is a task; only folders hold items/ });
});
test("a folder and a task change kind keeping their number; a folder holding items stays one", async () => {
  const f = await post("items", { kind: "folder", text: "Becomes a task" });
  const n = f.body.num;
  const r = await patch("items", f.body.id, { kind: "task", status: "", assigned: "You" });
  assert.equal(r.status, 200);
  assert.equal(r.body.num, n);
  await patch("items", f.body.id, { kind: "folder", status: null, assigned: null });
  assert.equal((await get("items", f.body.id)).num, n);
  assert.equal((await patch("items", f.body.id, { kind: "task", status: "", assigned: "You" })).body.num, n, "the same number again");
  await refused(() => patch("items", f.body.id, { kind: "folder" }), { says: /status: is only for tasks.*assigned: is only for tasks/ });
  await patch("items", f.body.id, { kind: "folder", status: null, assigned: null });
  await post("items", { text: "Inside", parent: f.body.id });
  await refused(() => patch("items", f.body.id, { kind: "task", status: "", assigned: "You" }), { says: /holds 1 item\(s\); only folders hold items/ });
});
test("PUT items: replaces what a client writes, and keeps num, added and order", async () => {
  const a = await post("items", { text: "Put me", note: "gone soon", priority: 4 });
  const before = await get("items", a.body.id);
  const r = await call("PUT", `/api/c/items/${a.body.id}`, { kind: "task", text: "Put", parent: "", status: "done", assigned: "You" });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.deepEqual(await get("items", a.body.id), { kind: "task", text: "Put", parent: "", status: "done", assigned: "You", num: before.num, added: before.added, order: before.order });
  await refused(() => call("PUT", `/api/c/items/${a.body.id}`, { text: "only text" }), { says: /kind: is required/ });
});

let ev1;
test("POST events: the server assigns id and time; the item must exist", async () => {
  const r = await post("events", { item: t1, kind: "progress", text: "Under way" });
  assert.equal(r.status, 200);
  ev1 = r.body.id;
  assert.match(r.body.t, ISO);
  assert.deepEqual(await get("events", ev1), { item: t1, kind: "progress", text: "Under way", t: r.body.t });
  assert.equal((await post("events", { item: "", kind: "note", text: "About the board" })).status, 200);
});
const BAD_EVENTS = {
  "item that isn't there": [{ item: "i99999", kind: "note", text: "x" }, /item: no item "i99999"/],
  "no item": [{ kind: "note", text: "x" }, /item: is required/],
  "no kind": [{ item: "T1", text: "x" }, /kind: is required/],
  "bad kind": [{ item: "T1", kind: "shout", text: "x" }, /kind: must be one of/],
  "kind stage (renamed)": [{ item: "T1", kind: "stage", text: "x" }, /kind: is renamed: send "status", not "stage"/],
  "no text": [{ item: "T1", kind: "note" }, /text: is required/],
  "empty text": [{ item: "T1", kind: "note", text: " " }, /text: must not be empty/],
  "t sent as epoch ms": [{ item: "T1", kind: "note", text: "x", t: "1792000000000.0" }, /t: is set by the server/],
  "t sent as ISO": [{ item: "T1", kind: "note", text: "x", t: "2026-10-26T08:35:48Z" }, /t: is set by the server/],
  "id sent": [{ item: "T1", kind: "note", text: "x", id: "mine" }, /id: is set by the server/],
  "unknown field": [{ item: "T1", kind: "note", text: "x", extra: 1 }, /extra: is not a field you can send here/],
  "item a number": [{ item: 582, kind: "note", text: "x" }, /item: must be a string/],
};
for (const [name, [body, says]] of Object.entries(BAD_EVENTS)) test(`POST events refused: ${name}`, () => refused(() => post("events", JSON.parse(JSON.stringify(body).replace("T1", t1))), { says }));

test("events: PATCH changes text and kind only; PUT and PATCH need an existing id; DELETE removes", async () => {
  assert.equal((await patch("events", ev1, { text: "Edited", kind: "note" })).status, 200);
  await refused(() => patch("events", ev1, { item: "" }), { says: /item: is set by the server|item: is not a field/ });
  await refused(() => patch("events", ev1, { t: "2026-10-26T08:35:48Z" }), { says: /t: is set by the server/ });
  await refused(() => patch("events", "nope", { text: "x" }), { status: 404, code: "not_found" });
  await refused(() => call("PUT", "/api/c/events/mine", { item: "", kind: "note", text: "x" }), { status: 404, code: "not_found", says: /POST \/api\/c\/events creates one/ });
  assert.equal((await call("DELETE", `/api/c/events/${ev1}`)).status, 200);
  assert.equal(await get("events", ev1), null);
});

test("meta: the counter is the server's; the now line's time is too", async () => {
  await refused(() => patch("meta", "counter", { next: 1 }), { status: 403, code: "forbidden", says: /server's/ });
  await refused(() => call("PUT", "/api/c/meta/counter", { next: 1 }), { status: 403, code: "forbidden" });
  await refused(() => patch("meta", "now", { text: "x", updated: 1793004672000 }), { says: /updated: is set by the server/ });
  await refused(() => patch("meta", "now", { text: 5 }), { says: /text: must be a string/ });
  await refused(() => patch("meta", "now", { other: 1 }), { says: /other: is not a field you can send here/ });
  await refused(() => patch("meta", "now", { now: "x" }), { says: /now: is renamed: send "text", not "now"/ });
  await refused(() => patch("meta", "mystery", { text: "x" }), { says: /there is no meta\/mystery/ });
  await refused(() => call("DELETE", "/api/c/meta/now"), { status: 405 });
  await refused(() => call("POST", "/api/c/meta", { text: "x" }), { status: 405 });
  // (The status line's old name, refused whatever is asked of it.)
  for (const [m, b] of [["GET"], ["PATCH", { now: "x" }], ["PUT", { now: "x" }]]) await refused(() => call(m, "/api/c/meta/status", b), { says: /meta\/status is renamed meta\/now \(its fields: text, updated; "now" is "text"; a task board page open since before the rename: reload it\)/ });
  const r = await patch("meta", "now", { text: "Working on it" });
  assert.equal(r.status, 200);
  const s = await get("meta", "now");
  assert.equal(s.text, "Working on it");
  assert.match(s.updated, ISO);
});

test("delete takes what is inside and their history; restore puts it all back", async () => {
  const f = await post("items", { kind: "folder", text: "Doomed" });
  const c = await post("items", { text: "Inside", parent: f.body.id });
  await post("events", { item: c.body.id, kind: "note", text: "About inside" });
  await post("events", { item: f.body.id, kind: "note", text: "About the folder" });
  const d = await call("DELETE", `/api/c/items/${f.body.id}`);
  assert.equal(d.body.deleted.length, 4);
  assert.equal(await get("items", c.body.id), null);
  assert.equal(store.list("events").filter((e) => [f.body.id, c.body.id].includes(e.data.item)).length, 0);
  assert.deepEqual(problems(store), []);
  assert.equal((await call("DELETE", `/api/c/items/${f.body.id}`)).status, 200, "deleting what is gone is no error");
  const back = await call("POST", `/api/c/items/${c.body.id}/restore`);
  assert.equal(back.body.restored.length, 4);
  assert.equal((await get("items", c.body.id)).num, c.body.num, "the same number");
  assert.equal(store.list("events").filter((e) => e.data.item === c.body.id).length, 1);
  assert.deepEqual((await call("POST", `/api/c/items/${f.body.id}/restore`)).body.restored, [], "already back: nothing to do");
  await refused(() => call("POST", "/api/c/items/never-was/restore"), { status: 404, code: "not_found" });
  assert.deepEqual(problems(store), []);
});

test("routes: nothing else is open", async () => {
  await refused(() => call("POST", "/api/c/items/someid", {}), { status: 405 });
  await refused(() => call("DELETE", "/api/c/items"), { status: 405 });
  await refused(() => call("GET", "/api/c/things"), { status: 404, code: "not_found" });
  await refused(() => call("POST", "/api/lease", { name: "counter", holder: "x" }), { status: 404, code: "not_found" });
  const s = await call("GET", "/api/schema");
  assert.deepEqual(s.body.statuses, STATUSES);
  assert.deepEqual(s.body.assignedSuggestions, ASSIGNED_SUGGESTIONS);
  assert.deepEqual(s.body.eventKinds, EVENT_KINDS);
  assert.deepEqual(s.body.renamed.itemFields, { stage: "status", where: "assigned" });
  // (A sort by a field's old name, or by no field, is refused.)
  await refused(() => call("GET", "/api/c/items?orderBy=stage"), { says: /orderBy: "stage" is renamed "status"/ });
  await refused(() => call("GET", "/api/c/items?orderBy=colour"), { says: /orderBy: "colour" isn't a field of items/ });
  assert.equal((await call("GET", "/api/c/events?orderBy=t&dir=desc&limit=2")).status, 200);
  // (A query it doesn't take is refused, not ignored.)
  await refused(() => call("GET", "/api/c/items?stage=done"), { says: /stage: isn't a query this takes \(these are: orderBy, dir, limit\)/ });
  await refused(() => call("GET", "/api/c/items?orderBy=num&dir=up"), { says: /dir: must be asc or desc/ });
  await refused(() => call("GET", "/api/c/items?limit=-1"), { says: /limit: must be a whole number from 1/ });
  // (An old name's refusal says what a page opened before the rename should do.)
  assert.match((await call("PATCH", `/api/c/items/${t1}`, { stage: "done" })).body.error.message, /reload it/);
});

test("history is read by item and in pages, and in short for the lists; no cap hides any of it", async () => {
  const a = (await post("items", { text: "History A" })).body.id, b = (await post("items", { text: "History B" })).body.id;
  const made = [];
  for (let i = 0; i < 7; i++) made.push((await post("events", { item: a, kind: "note", text: `a${i}` })).body.id);
  await post("events", { item: b, kind: "note", text: "b0" });
  const board = (await post("events", { item: "", kind: "note", text: "top" })).body;
  const one = async (qs) => (await call("GET", `/api/c/events?${qs}`)).body;
  // One item's events, newest first (ties by id), and its own total.
  const all = await one(`item=${a}&orderBy=t&dir=desc`);
  assert.equal(all.docs.length, 7);
  assert.equal(all.total, 7, "the total is the item's, not the board's");
  assert.ok(all.docs.every((d) => d.data.item === a));
  assert.deepEqual(all.docs.map((d) => d.id).sort(), [...made].sort());
  assert.deepEqual(all.docs.map((d) => [d.data.t, d.id]), all.docs.map((d) => [d.data.t, d.id]).sort((x, y) => (x[0] < y[0] ? 1 : x[0] > y[0] ? -1 : x[1] < y[1] ? -1 : 1)));
  // Pages of 3 from where the last ended give exactly the same, none twice, none missed (ties in t too).
  const pages = [];
  let cur = await one(`item=${a}&orderBy=t&dir=desc&limit=3`);
  while (cur.docs.length) {
    pages.push(...cur.docs);
    const e = cur.docs.at(-1);
    cur = await one(`item=${a}&orderBy=t&dir=desc&limit=3&before=${encodeURIComponent(e.data.t)}&beforeId=${e.id}`);
  }
  assert.deepEqual(pages.map((d) => d.id), all.docs.map((d) => d.id));
  // The board's own events are item "".
  assert.deepEqual((await one("item=&orderBy=t&dir=desc")).docs.map((d) => d.id).includes(board.id), true);
  assert.equal((await one(`item=${b}`)).docs.length, 1);
  assert.equal((await one("item=nothing")).docs.length, 0);
  // In short: each item's count and its latest time, for the rows and the recent view.
  const h = (await call("GET", "/api/history")).body;
  assert.equal(h[a][0], 7);
  assert.equal(h[a][1], all.docs[0].data.t);
  assert.deepEqual([h[b][0], h[""][0] >= 1], [1, true]);
  assert.equal(Object.values(h).reduce((n, v) => n + v[0], 0), store.count("events"), "every event is counted once");
  // A write moves it at once.
  await post("events", { item: b, kind: "note", text: "b1" });
  assert.equal((await call("GET", "/api/history")).body[b][0], 2);
  // Bad queries are refused.
  await refused(() => call("GET", `/api/c/events?item=${a}&before=2026-01-01T00:00:00.000Z&beforeId=x`), { says: /before: needs orderBy=t&dir=desc/ });
  await refused(() => call("GET", `/api/c/events?orderBy=t&dir=desc&before=2026-01-01T00:00:00.000Z`), { says: /beforeId: go together/ });
  await refused(() => call("GET", `/api/c/events?orderBy=t&dir=desc&before=yesterday&beforeId=x`), { says: /before: must be a time/ });
  await refused(() => call("GET", `/api/c/items?item=${a}`), { says: /item: isn't a query this takes/ });
});

test("the page reads no whole list of events and caps none (history is read by item, in short for the lists)", () => {
  const page = fs.readFileSync(new URL("../public/task-board/index.html", import.meta.url), "utf8");
  assert.doesNotMatch(page, /collection\("events"\)/, "no read of the events collection as a whole");
  assert.doesNotMatch(page, /limit\(10000\)|oldest weren't loaded/);
  assert.match(page, /get\("history"\)/);
});

test("the app's pages, the board's schema and the dashboard's timeline are served", async () => {
  for (const [url, type] of [["/", /text\/html/], ["/task-board/", /text\/html/], ["/task-board/709", /text\/html/], ["/task-board/shim.js", /javascript/],
    ["/task-board/schema.mjs", /javascript/], ["/task-board/priority.mjs", /javascript/], ["/jobs/", /text\/html/], ["/jobs/jobs.js", /javascript/], ["/lib/buildpage/timeline.js", /javascript/], ["/lib/buildpage/dom.js", /javascript/], ["/lib/buildpage/words.js", /javascript/], ["/favicon.svg", /svg/]]) {
    const r = await fetch(base + url);
    assert.equal(r.status, 200, url);
    assert.match(r.headers.get("content-type"), type, url);
  }
  // (Without its slash, a page's address goes to it, relative: a proxy's path stays.)
  const r = await fetch(base + "/task-board", { redirect: "manual" });
  assert.equal(r.status, 301);
  assert.equal(r.headers.get("location"), "task-board/");
  // (A task's address from before the dev tools app, /709, goes to the board's.)
  const old = await fetch(base + "/709?x=1", { redirect: "manual" });
  assert.equal(old.status, 301);
  assert.equal(old.headers.get("location"), "task-board/709?x=1");
  // (And its page's API, at the old proxy path's.)
  assert.deepEqual(await (await fetch(base + "/task-board/api/c/meta/now")).json(), await (await fetch(base + "/api/c/meta/now")).json());
  // (Only the build page's modules the dashboard imports, nothing else of the checkout.)
  for (const url of ["/lib/buildpage/dash.js", "/lib/buildpage/../../../README.md", "/../server.mjs", "/task-board/../../jobs.mjs"]) assert.equal((await fetch(base + url)).status, 404, url);
});

test("after all of that, every item, event and meta document fits the schema", () => {
  assert.deepEqual(problems(store), []);
  for (const { data } of store.list("items")) assert.match(data.added, ISO);
  for (const { data } of store.list("events")) assert.match(data.t, ISO);
});

test("a path that isn't valid percent-encoding is a 400, not a 500", async () => {
  const r = await call("GET", "/api/c/items/%E0%A4%A");
  assert.equal(r.status, 400);
  assert.equal(r.body.error.code, "invalid_argument");
});

test("the page's shim never shows a read that began before a write was answered (the flicker on archive)", async () => {
  const vm = await import("node:vm");
  const ctl = []; // fetches held until the test answers them
  const fetchStub = (url, o = {}) => new Promise((res) => ctl.push((state) => res({ ok: true, json: async () => state, method: o.method || "GET" })));
  class ES { constructor() { ES.me = this; this.l = {}; } addEventListener(n, f) { this.l[n] = f; } }
  const window = {};
  vm.runInNewContext(fs.readFileSync(new URL("../public/task-board/shim.js", import.meta.url), "utf8"), { window, fetch: fetchStub, EventSource: ES, URLSearchParams, encodeURIComponent, JSON, Promise, Object, Error });
  const db = (await window.claude.use("db"));
  const shown = [];
  db.collection("items").onSnapshot((s) => shown.push(s.size));
  const tick = () => new Promise((r) => setImmediate(r));
  const docs = (n) => ({ docs: Array.from({ length: n }, (_, i) => ({ id: "i" + i, data: {} })), total: n });
  await tick();
  ctl.shift()(docs(2)); await tick(); // the first read
  assert.deepEqual(shown, [2]);
  ES.me.l.change({ data: '{"col":"items"}' }); await tick(); // a read begins (it will answer with the old state)
  const stale = ctl.shift();
  const w = db.doc("items/i0").update({ archived: true }); await tick();
  ctl.shift()({}); await w; // the write is answered
  stale(docs(2)); await tick(); // the old read arrives after it
  assert.deepEqual(shown, [2], "the stale read isn't shown");
  ctl.shift()(docs(1)); await tick(); // the read made again
  assert.deepEqual(shown, [2, 1]);
});
