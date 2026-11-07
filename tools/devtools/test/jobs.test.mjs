// The dev job dashboard's API (jobs.mjs): events merged into spans whatever their order, a bad line
// refused without the rest, and what the page reads. Run: node --test tools/devtools/test/
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { openStore } from "../store.mjs";
import { openJobs, LOST_S } from "../jobs.mjs";
import { makeServer } from "../server.mjs";

const dir = fs.mkdtempSync(path.join(os.tmpdir(), "devjobs-test-"));
const store = openStore(path.join(dir, "t.db"));
const jobs = openJobs(path.join(dir, "jobs.db"));
const server = makeServer({ board: store, jobs });
let base;
before(async () => { await new Promise((r) => server.listen(0, "127.0.0.1", r)); base = `http://127.0.0.1:${server.address().port}`; });
after(() => { server.close(); server.closeAllConnections?.(); store.close(); jobs.close(); fs.rmSync(dir, { recursive: true, force: true }); });

const send = async (events, raw) => {
  const r = await fetch(base + "/api/jobs/events", { method: "POST", headers: { "content-type": "application/x-ndjson" }, body: raw ?? events.map((e) => JSON.stringify(e)).join("\n") + "\n" });
  return { status: r.status, body: await r.json() };
};
const get = async (url) => { const r = await fetch(base + url); return { status: r.status, body: await r.json() }; };
const now = Math.floor(Date.now() / 1000);
const T = now - 3600;

test("a run's events merge whatever their order: an end before its start, parts before their run", async () => {
  const r = await send([
    { op: "span", id: "m-1-part", parent: "m-1", kind: "part", name: "fmt", mac: "m", t0: T + 1, t1: T + 3, status: "ok" },
    { op: "span", id: "m-1-crate", parent: "m-1-part", kind: "crate", name: "serde", mac: "m", t0: T + 1, t1: T + 2, status: "ok" },
    { op: "end", id: "m-1", t: T + 10, status: "fail", attrs: { why: "clippy" } },
    { op: "start", id: "m-1", kind: "check", name: "check agent-x", mac: "m", t: T, attrs: { task: "946", who: "agent-x" } },
  ]);
  assert.deepEqual(r.body, { accepted: 4, refused: [] });
  const d = await get("/api/jobs/runs/m-1-crate");
  assert.equal(d.body.run.id, "m-1", "a span's run is its parents' top");
  assert.deepEqual([d.body.run.t0, d.body.run.t1, d.body.run.status], [T, T + 10, "fail"], "the end's status kept over the start's running");
  assert.deepEqual(d.body.run.attrs, { why: "clippy", task: "946", who: "agent-x" });
  assert.deepEqual(d.body.spans.map((s) => s.id), ["m-1-part", "m-1-crate"]);
  const runs = await get(`/api/jobs/runs?from=${T - 60}&to=${T + 60}`);
  assert.deepEqual(runs.body.runs.map((x) => [x.id, x.spans, x.failed]), [["m-1", 2, 1]], "one run, its two spans, one of them (itself) failed");
});

test("resent events change nothing (a spool sent again after a lost answer)", async () => {
  const ev = [{ op: "span", id: "m-2", kind: "cargo", name: "cargo build", mac: "m", t0: T + 20, t1: T + 30, status: "ok", attrs: { args: "build" } }];
  await send(ev);
  await send(ev);
  const d = await get("/api/jobs/runs/m-2");
  assert.deepEqual([d.body.run.status, d.body.run.attrs, d.body.spans.length], ["ok", { args: "build" }, 0]);
});

test("a bad line is refused with why, and the others are taken", async () => {
  const lines = [
    JSON.stringify({ op: "start", id: "m-3", kind: "publish", name: "publish", mac: "m", t: T + 40 }),
    "{not json",
    JSON.stringify({ op: "start", id: "bad id!", kind: "x", name: "x", mac: "m", t: T }),
    JSON.stringify({ op: "end", id: "m-3", t: 1000, status: "ok" }),
    JSON.stringify({ op: "end", id: "m-3", t: T + 41, status: "maybe" }),
    JSON.stringify({ op: "span", id: "m-4", kind: "x", name: "x", mac: "m", t0: T + 5, t1: T + 4, status: "ok" }),
    JSON.stringify({ op: "explode", id: "m-5" }),
  ];
  const r = await send(null, lines.join("\n"));
  assert.equal(r.status, 200);
  assert.equal(r.body.accepted, 1);
  assert.deepEqual(r.body.refused.map((x) => x.line), [2, 3, 4, 5, 6, 7]);
  assert.match(r.body.refused[0].why, /JSON/);
  assert.match(r.body.refused[1].why, /^id:/);
  assert.match(r.body.refused[2].why, /^t:/);
  assert.match(r.body.refused[3].why, /^status:/);
  assert.match(r.body.refused[4].why, /^t1: not before t0/);
  assert.match(r.body.refused[5].why, /^op:/);
});

test("a running span its Mac's sampler stops naming is lost; one it names is not", async () => {
  await send([
    { op: "start", id: "m-6", kind: "tests", name: "tests", mac: "m", t: now - 600 },
    { op: "start", id: "m-7", kind: "tests", name: "tests", mac: "m", t: now - 600 },
    { op: "sample", mac: "m", t: now - 5, load: 3.5, memfree: 61, swap: 0, ncpu: 10, chip: "Apple M1 Pro", mem: 16, live: ["m-7"] },
  ]);
  const runs = (await get(`/api/jobs/runs?from=${now - 700}&to=${now}`)).body;
  const by = Object.fromEntries(runs.runs.map((x) => [x.id, x]));
  assert.ok(by["m-6"].lost >= now - 600 && by["m-6"].lost <= now - LOST_S, "lost since its start");
  assert.equal(by["m-7"].lost, undefined);
  assert.deepEqual(runs.macs.m, { chip: "Apple M1 Pro", mem: 16, ncpu: 10, seen: now - 5 });
});

test("samples come in at most n buckets a Mac, each its most load and least memory free", async () => {
  const ev = [];
  for (let i = 0; i < 100; i++) ev.push({ op: "sample", mac: "s", t: T + i * 10, load: i, memfree: 100 - i, swap: i * 2, ncpu: 8, live: [] });
  await send(ev);
  const s = (await get(`/api/jobs/samples?from=${T}&to=${T + 1000}&n=10`)).body;
  assert.equal(s.samples.s.length, 10);
  assert.deepEqual(s.samples.s[0], [T, 9, 91, 18]);
});

test("stats: each name's runs, failures and times", async () => {
  await send([
    { op: "span", id: "t-1", kind: "test", name: "a::b", mac: "m", t0: T, t1: T + 2, status: "ok" },
    { op: "span", id: "t-2", kind: "test", name: "a::b", mac: "m", t0: T + 10, t1: T + 14, status: "fail" },
    { op: "span", id: "t-3", kind: "test", name: "a::c", mac: "m", t0: T, t1: T + 1, status: "ok" },
    { op: "span", id: "t-4", kind: "test", name: "a::b", mac: "m", t0: T + 20, t1: T + 20, status: "ok", attrs: { untimed: "1" } },
    { op: "span", id: "t-5", kind: "test", name: "a::b", mac: "m", t0: T + 30, t1: T + 30, status: "skip" },
  ]);
  const s = (await get("/api/jobs/stats?kind=test")).body;
  const b = s.names.find((x) => x.name === "a::b");
  // (A run with no time of its own, cargo test's, counts but has no time; a skipped one only as skipped.)
  assert.deepEqual([b.n, b.failed, b.skipped, b.p50, b.max, b.total, b.recent], [3, 1, 1, 4, 4, 6, [2, 4]]);
  assert.equal((await get("/api/jobs/stats")).status, 400);
  assert.ok((await get("/api/jobs/kinds")).body.kinds.some((k) => k.kind === "test" && k.n === 5));
});

test("only POST events writes; a span that isn't there is a 404", async () => {
  assert.equal((await fetch(base + "/api/jobs/runs", { method: "POST" })).status, 405);
  assert.equal((await get("/api/jobs/runs/nope")).status, 404);
  assert.equal((await get("/api/jobs/runs?from=10&to=5")).status, 400);
});

test("a test run is on the Mac it ran on: the tests job while its run is remote, the test Mac's run and its parts", async () => {
  const U = now - 1800;
  await send([
    // (remote-tests.sh on its own, its run on m4; then check.sh with its tests part's run on m4; then one run here, one rerun here.)
    { op: "start", id: "r-1", kind: "tests", name: "tests k", mac: "m1", t: U, attrs: { checkout: "k" } },
    { op: "attr", id: "r-1", attrs: { ran_on: "m4" } },
    { op: "sample", mac: "m4", t: U + 5, load: 9, memfree: 40, swap: 0, ncpu: 14, live: [] },
    { op: "start", id: "c-1", kind: "check", name: "check k", mac: "m1", t: U, attrs: { checkout: "k" } },
    { op: "start", id: "c-1-tests", parent: "c-1", kind: "tests", name: "tests k", mac: "m1", t: U + 1, attrs: { ran_on: "m4" } },
    { op: "span", id: "c-1-run", parent: "c-1-tests", kind: "run", name: "run on the test Mac", mac: "m4", t0: U + 2, t1: U + 8, status: "ok", attrs: { ran_on: "m4" } },
    { op: "span", id: "c-1-suite", parent: "c-1-run", kind: "suite", name: "store", mac: "m4", t0: U + 3, t1: U + 7, status: "ok" },
    { op: "start", id: "h-1", kind: "tests", name: "tests h", mac: "m1", t: U, attrs: { ran_here: "SCENIC_CHECK_LOCAL set" } },
    { op: "start", id: "b-1", kind: "tests", name: "tests b", mac: "m1", t: U, attrs: { ran_on: "m4", rerun_here: "the test Mac's run ended without a result" } },
  ]);
  const d = (await get(`/api/jobs/runs?from=${U - 60}&to=${U + 60}`)).body;
  const by = Object.fromEntries(d.runs.map((x) => [x.id, x]));
  assert.deepEqual([by["r-1"].mac, by["r-1"].on], ["m1", "m4"], "started on m1, shown on m4 while its run is there");
  assert.deepEqual([by["h-1"].on, by["b-1"].on], ["m1", "m1"], "a run here, and one rerun here, stay on their Mac");
  assert.deepEqual([by["c-1"].on, by["c-1"].macs], ["m1", ["m1", "m4"]], "check.sh on its Mac, naming the Macs its parts ran on");
  assert.deepEqual(d.away.map((x) => [x.id, x.root, x.on]), [["c-1-tests", "c-1", "m4"]], "check's tests job on the test Mac, with its run");
  assert.deepEqual((await get(`/api/jobs/runs?from=${U - 60}&to=${U + 60}&kind=check`)).body.away, []);
  const r = (await get("/api/jobs/runs/r-1")).body;
  assert.deepEqual(Object.keys(r.samples).sort(), ["m1", "m4"], "the test Mac's load beside its own, before the run's spans come");
  const c = (await get("/api/jobs/runs/c-1-suite")).body;
  assert.deepEqual(c.spans.map((s) => [s.id, s.mac, s.on]), [["c-1-tests", "m1", "m4"], ["c-1-run", "m4", "m4"], ["c-1-suite", "m4", "m4"]]);
});

test("a stored history's test-Mac runs move to the Mac they name, once (jobs.db before #1086)", () => {
  const file = path.join(dir, "old.db");
  const old = openJobs(file);
  const ins = old.db.prepare(`INSERT INTO spans (id, parent, root, kind, name, mac, t0, t1, status, attrs) VALUES (?, ?, 'o', ?, ?, 'm1', 1, 2, 'ok', ?)`);
  ins.run("o", null, "check", "check", "{}");
  ins.run("o-t", "o", "tests", "tests", JSON.stringify({ ran_on: "m4" }));
  ins.run("o-copy", "o-t", "copy", "copy to the test Mac", "{}");
  ins.run("o-run", "o-t", "run", "run on the test Mac", JSON.stringify({ ran_on: "m4" }));
  ins.run("o-suite", "o-run", "suite", "store", "{}");
  ins.run("o-test", "o-suite", "test", "a::b", "{}");
  old.db.exec("PRAGMA user_version = 0");
  old.close();
  const again = openJobs(file);
  const macs = Object.fromEntries(again.db.prepare(`SELECT id, mac FROM spans`).all().map((r) => [r.id, r.mac]));
  assert.deepEqual(macs, { o: "m1", "o-t": "m1", "o-copy": "m1", "o-run": "m4", "o-suite": "m4", "o-test": "m4" });
  assert.equal(again.db.prepare("PRAGMA user_version").get().user_version, 1);
  again.close();
});
