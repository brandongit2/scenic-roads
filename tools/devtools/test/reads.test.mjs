// The board's list reads at any size (#1344): no cap, a stable order, `next` when a read stops
// short, an answer read from one moment and sent as it is read; `since` a mark (what changed); the
// hot reads answered from an index. Run: node --test tools/devtools/test/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { openStore, CHANGES_KEPT } from "../store.mjs";
import { openJobs } from "../jobs.mjs";
import { makeServer } from "../server.mjs";

// A board of its own on a free port, in a temporary folder; `t.after` closes and removes it.
async function scratchBoard(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-reads-"));
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
  // A refusal: 400, its message matching `says`.
  const refused = async (p, says) => {
    const r = await p();
    assert.equal(r.status, 400, JSON.stringify(r.body));
    assert.match(r.body.error.message, says);
  };
  return { store, base, call, refused };
}

test("a list read has no cap: every document asked for, in a stable order, `next` when it stops short, read as it is sent", async (t) => {
  const b = await scratchBoard(t);
  // 12,345 events (past the 10,000 a read was once cut at), some at the same time, and 30 items.
  b.store.db.exec("BEGIN");
  for (let i = 0; i < 30; i++) b.store.put("items", `i${i + 1}`, { kind: "task", text: `t${i}`, parent: "", order: i, num: i + 1, status: "", assigned: "", added: "2026-11-01T00:00:00.000Z", ...(i % 3 ? { priority: i % 7 } : {}) });
  for (let i = 0; i < 12345; i++) b.store.put("events", `e${String(i).padStart(5, "0")}`, { item: `i${(i % 30) + 1}`, kind: "note", t: `2026-11-${String(1 + (i % 9)).padStart(2, "0")}T00:00:00.000Z`, text: `n${i}` });
  b.store.db.exec("COMMIT");
  const get = async (qs) => (await b.call("GET", `/api/c/${qs}`)).body;
  // Asked for more than there are: all of them, the total, no next.
  const all = await get("events?limit=20000");
  assert.equal(all.docs.length, 12345);
  assert.equal(all.total, 12345);
  assert.equal(all.next, undefined);
  // The default order: an event's time, oldest first, then its id.
  const order = (ds) => ds.map((d) => `${d.data.t} ${d.id}`);
  assert.deepEqual(order(all.docs), [...order(all.docs)].sort());
  assert.deepEqual((await get("events")).docs.map((d) => d.id), all.docs.map((d) => d.id), "no limit: the same");
  assert.deepEqual((await get("items")).docs.map((d) => d.data.num), Array.from({ length: 30 }, (_, i) => i + 1), "items by number");
  // Fewer asked for: those, and `next`, from which the rest come, each once, in the same order.
  const pages = [];
  let cur = await get("events?limit=5000");
  for (;;) {
    pages.push(...cur.docs);
    assert.equal(cur.total, 12345);
    if (!cur.next) break;
    assert.equal(cur.docs.length, 5000);
    cur = await get(`events?limit=5000&after=${cur.next}`);
  }
  assert.deepEqual(pages.map((d) => d.id), all.docs.map((d) => d.id));
  // In any order, nulls too (a priority some tasks lack), up and down.
  for (const dir of ["asc", "desc"]) {
    const whole = (await get(`items?orderBy=priority&dir=${dir}`)).docs.map((d) => d.id);
    const got = [];
    for (let c = await get(`items?orderBy=priority&dir=${dir}&limit=4`); ; c = await get(`items?orderBy=priority&dir=${dir}&limit=4&after=${c.next}`)) { got.push(...c.docs.map((d) => d.id)); if (!c.next) break; }
    assert.deepEqual(got, whole, dir);
    const p = whole.map((id) => b.store.get("items", id).priority ?? null);
    assert.deepEqual(p, [...p].sort((x, y) => (x === y ? 0 : x === null ? -1 : y === null ? 1 : x - y)).slice()[dir === "asc" ? "slice" : "reverse"](), `${dir}: nulls ${dir === "asc" ? "first" : "last"}`);
  }
  // A place from another order is refused, as is one that isn't a place.
  const up = (await get("events?limit=5000")).next;
  await b.refused(() => b.call("GET", `/api/c/events?orderBy=t&dir=desc&limit=5000&after=${up}`), /after: is a place in another order \(orderBy=t&dir=asc\)/);
  await b.refused(() => b.call("GET", "/api/c/events?after=nonsense"), /after: isn't a place in a list/);
  // (A forged place, its value no column's, is refused too, never a 500.)
  const forged = (v) => Buffer.from(JSON.stringify(["t", "asc", v, "e00001"])).toString("base64url");
  for (const v of [{}, [1], true]) await b.refused(() => b.call("GET", `/api/c/events?after=${forged(v)}`), /after: isn't a place in a list/);
  // A slow reader: the read waits for it each time, leaving no listener behind.
  let warned = null;
  const onWarn = (w) => { warned = w; };
  process.on("warning", onWarn);
  t.after(() => process.off("warning", onWarn));
  const slow = await new Promise((done, failed) => (async () => {
    const r = await fetch(`${b.base}/api/c/events`), rd = r.body.getReader(), chunks = [];
    for (;;) { const { done: end, value } = await rd.read(); if (end) break; chunks.push(value); await new Promise((go) => setTimeout(go, 2)); }
    done(JSON.parse(Buffer.concat(chunks).toString("utf8")));
  })().catch(failed));
  assert.equal(slow.docs.length, 12345);
  assert.equal(warned, null, String(warned));
  // Read as it is sent, from the file at one moment: a write made while the answer is on its way isn't in it.
  const http = await import("node:http");
  const body = await new Promise((done, failed) => http.get(`${b.base}/api/c/events`, (res) => {
    const chunks = [];
    res.once("data", (c) => {
      chunks.push(c);
      res.pause();
      b.store.put("events", "zz-late", { item: "", kind: "note", t: "2026-11-30T00:00:00.000Z", text: "late" });
      setTimeout(() => res.resume(), 50);
      res.on("data", (c2) => chunks.push(c2));
    });
    res.on("end", () => done(JSON.parse(Buffer.concat(chunks).toString("utf8"))));
    res.on("error", failed);
  }));
  assert.equal(body.total, 12345);
  assert.equal(body.docs.length, 12345);
  assert.equal(body.docs.some((d) => d.id === "zz-late"), false);
  assert.equal((await get("events?limit=1&orderBy=t&dir=desc")).docs[0].id, "zz-late", "written all the same");
  // A read that fails part way ends the connection: never an answer that parses.
  b.store.db.exec(`INSERT INTO items (id, kind, text, parent, "order", num, status, assigned, added, photos) VALUES ('ibad', 'task', 'x', '', 99, 5000, '', '', '2026-11-01T00:00:00.000Z', '{not json')`);
  b.store.db.exec("BEGIN");
  for (let i = 0; i < 2000; i++) b.store.put("items", `ifill${i}`, { kind: "task", text: "x".repeat(100), parent: "", order: 100 + i, num: 100 + i, status: "", assigned: "", added: "2026-11-01T00:00:00.000Z" });
  b.store.db.exec("COMMIT");
  const quiet = console.error;
  console.error = () => {};
  t.after(() => { console.error = quiet; });
  await assert.rejects(async () => (await fetch(`${b.base}/api/c/items`)).json());
  console.error = quiet;
});

test("since a mark: what changed in a collection (a deleted one by its id), everything when the mark is too old or another process's", async (t) => {
  const b = await scratchBoard(t);
  const one = (await b.call("POST", "/api/c/items", { text: "one" })).body.id;
  const first = (await b.call("GET", "/api/c/items")).body;
  assert.match(first.mark, /^[0-9a-f]+-\d+$/);
  const two = (await b.call("POST", "/api/c/items", { text: "two" })).body.id;
  await b.call("PATCH", `/api/c/items/${one}`, { text: "one, edited" });
  const d = (await b.call("GET", `/api/c/items?since=${first.mark}`)).body;
  assert.deepEqual(d.docs.map((x) => [x.id, x.data.text]).sort(), [[one, "one, edited"], [two, "two"]]);
  assert.deepEqual([d.deleted, d.total, d.full], [[], 2, undefined]);
  await b.call("DELETE", `/api/c/items/${two}`);
  const d2 = (await b.call("GET", `/api/c/items?since=${d.mark}`)).body;
  assert.deepEqual([d2.docs, d2.deleted, d2.total], [[], [two], 1]);
  assert.deepEqual((await b.call("GET", `/api/c/items?since=${d2.mark}`)).body.docs, []);
  // (Everything, said so: none held, another process's mark, one older than the log keeps.)
  for (const m of ["0", "00000000-3"]) {
    const f = (await b.call("GET", `/api/c/items?since=${m}`)).body;
    assert.deepEqual([f.full, f.docs.map((x) => x.id), f.total], [true, [one], 1], m);
  }
  b.store.db.exec("BEGIN");
  for (let i = 0; i <= CHANGES_KEPT * 2; i++) b.store.put("events", `e${i}`, { item: "", kind: "note", t: "2026-11-01T00:00:00.000Z", text: "x" });
  b.store.db.exec("COMMIT");
  assert.equal((await b.call("GET", `/api/c/items?since=${d2.mark}`)).body.full, true);
  await b.refused(() => b.call("GET", `/api/c/items?since=${d2.mark}&limit=3`), /since: goes alone/);
  await b.refused(() => b.call("GET", "/api/c/items?since=today"), /since: must be the `mark` a read gave, or 0/);
  await b.refused(() => b.call("GET", "/api/c/meta?limit=1"), /limit: isn't a query this takes \(it takes none\)/);
});

test("the board's hot reads are answered from an index: no scan of a table, no sort", (t) => {
  const s = openStore(path.join(fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-plan-")), "t.db"));
  t.after(() => s.close());
  const plan = (sql) => s.db.prepare(`EXPLAIN QUERY PLAN ${sql}`).all().map((r) => r.detail).join("; ");
  for (const [what, sql] of [
    ["the items in order", `SELECT * FROM items ORDER BY "num" ASC`],
    ["the events in order", `SELECT * FROM events ORDER BY "t" ASC, id ASC`],
    ["the events, newest first", `SELECT * FROM events ORDER BY "t" DESC, id DESC`],
    ["a history's next page", `SELECT * FROM events WHERE item = ? AND ("t" < ? OR ("t" = ? AND id < ?) OR "t" IS NULL) ORDER BY "t" DESC, id DESC LIMIT 101`],
    ["a history's summary", `SELECT COUNT(*) AS n, MAX(t) AS last, MAX(rowid) AS v FROM events WHERE item = ?`],
    ["the open tasks a priority runs into", `SELECT id, priority FROM items WHERE kind = 'task' AND status NOT IN ('done', 'abandoned') AND archived IS NOT 1 AND priority > ?`],
    ["the last order", `SELECT MAX("order") AS m FROM items`],
    ["the highest number", `SELECT MAX(num) AS m FROM items`],
    ["a number's owner", `SELECT id FROM items WHERE num = ?`],
    ["a folder's items", `SELECT id FROM items WHERE parent = ?`],
    ["an item's events", `SELECT id FROM events WHERE item = ?`],
  ]) {
    const p = plan(sql);
    assert.match(p, /USING (COVERING )?INDEX/, `${what}: ${p}`);
    assert.doesNotMatch(p, /TEMP B-TREE|^SCAN (items|events)$/, `${what}: ${p}`);
  }
});
