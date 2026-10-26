// The one-time repair (#672): it fits legacy data to the schema, a dry run changes nothing, and a
// second run finds nothing to do.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { openStore } from "../store.mjs";
import { repair, problems, main } from "../repair.mjs";

function legacy() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-repair-"));
  const file = path.join(dir, "t.db");
  const store = openStore(file);
  const item = (id, row) => {
    const { extra, ...cols } = row;
    const names = ["id", ...Object.keys(cols), "extra"];
    store.db.prepare(`INSERT INTO items (${names.map((n) => `"${n}"`).join(",")}) VALUES (${names.map(() => "?").join(",")})`).run(id, ...Object.values(cols), extra ? JSON.stringify(extra) : null);
  };
  const event = (id, row) => {
    const { extra, ...cols } = row;
    const names = ["id", ...Object.keys(cols), "extra"];
    store.db.prepare(`INSERT INTO events (${names.map((n) => `"${n}"`).join(",")}) VALUES (${names.map(() => "?").join(",")})`).run(id, ...Object.values(cols), extra ? JSON.stringify(extra) : null);
  };
  item("f1", { kind: "folder", text: "Folder", parent: "", order: 1, added: "2026-10-01T00:00:00Z", stage: "done", extra: { note_append: "" } });
  item("i1", { kind: "task", text: "Fine", parent: "f1", order: 1, num: 1, stage: "done", where: "You", added: "2026-10-02T10:00:00.250Z" });
  item("i2", { kind: "task", text: "Epoch added", parent: "f1", order: 2, num: 2, stage: "", where: "You", added: "1792000000000.0", started: "2026-10-03T10:00:00Z" });
  item("i3", { kind: "task", text: "No added, no parent, no order", num: 3, stage: "working" });
  item("i4", { kind: "task", text: "No number", parent: "ghost", order: 4, stage: "", where: "Subagent", added: "2026-10-05T00:00:00Z" });
  event("e-good", { item: "i1", kind: "done", t: "2026-10-02T11:00:00Z", text: "ok" });
  event("e-epoch", { item: "i1", kind: "note", t: "1792000000000.0", text: "epoch t" });
  event("i3", { kind: "note", text: "keyed by its item, no item/t", extra: {} });
  event("3", { text: "keyed by a number" });
  event("e-task", { text: "task/at", extra: { task: "i1", num: 1, at: "2026-10-02T12:00:00Z" } });
  event("ev-1-done-17", { text: "from the id", extra: { task: "nope", num: 1, at: "2026-10-02T13:00:00Z" } });
  event("i2", { text: "bundle", extra: { events: [{ t: "2026-10-03T11:00:00Z", text: "one" }, { t: "2026-10-03T12:00:00Z", text: "two" }] } });
  event("e-board", { text: "about the board", extra: { at: "2026-10-04T00:00:00Z" } });
  event("e-num-item", { item: "2", kind: "note", t: "2026-10-04T01:00:00Z", text: "item is a number" });
  event("e-textless", { kind: "note", t: "2026-10-04T01:00:00Z", item: "i1" });
  store.put("meta", "counter", { next: 2 });
  store.db.prepare("INSERT OR REPLACE INTO meta (key, value) VALUES ('status', ?)").run(JSON.stringify({ now: "hello", updated: 1793004672000 }));
  return { dir, file, store };
}
const dump = (s) => JSON.stringify([s.list("items"), s.list("events"), s.list("meta")]);

test("a dry run changes nothing and says what it would change", () => {
  const { dir, file, store } = legacy();
  const before = dump(store);
  assert.ok(problems(store).length > 10);
  store.close();
  const logs = [];
  const log = console.log;
  console.log = (...a) => logs.push(a.join(" "));
  let code;
  try { code = main([file]); } finally { console.log = log; }
  assert.equal(code, 0);
  const s2 = openStore(file);
  assert.equal(dump(s2), before, "rolled back");
  assert.ok(logs.some((l) => /dry run: nothing was changed/.test(l)));
  assert.ok(logs.some((l) => /events: legacy shape rewritten/.test(l)));
  s2.close();
  fs.rmSync(dir, { recursive: true, force: true });
});

test("--run repairs it, keeps a copy, and a second run does nothing", () => {
  const { dir, file, store } = legacy();
  store.close();
  const log = console.log;
  console.log = () => {};
  try { assert.equal(main([file, "--run"]), 0); } finally { console.log = log; }
  assert.ok(fs.readdirSync(dir).some((f) => f.includes(".before-repair-")));
  const s = openStore(file);
  assert.deepEqual(problems(s), []);
  const g = (id) => s.get("items", id);
  assert.equal(g("f1").stage, undefined);
  assert.match(g("i2").added, /^2026-10-14T17:46:40\.000Z$/);
  assert.equal(g("i1").added, "2026-10-02T10:00:00.250Z");
  assert.equal(g("i3").parent, "");
  assert.equal(typeof g("i3").order, "number");
  assert.equal(g("i3").where, "");
  assert.ok(g("i3").added, "dated");
  assert.equal(g("i4").parent, "");
  assert.ok(g("i4").num > 3);
  assert.ok(s.get("meta", "counter").next > g("i4").num);
  assert.match(s.get("meta", "status").updated, /^2026-10-26T08:51:12\.000Z$/);
  const evs = s.list("events").map((e) => e.data);
  assert.ok(evs.some((e) => e.item === "i3" && e.text.startsWith("keyed by its item")));
  assert.ok(evs.some((e) => e.item === "i3" && e.text === "keyed by a number"), "a number names the task with it");
  assert.ok(evs.some((e) => e.item === "i1" && e.t === "2026-10-02T12:00:00.000Z" && e.text === "task/at"));
  assert.ok(evs.some((e) => e.item === "i1" && e.kind === "done" && e.text === "from the id"));
  assert.equal(evs.filter((e) => e.item === "i2" && ["one", "two"].includes(e.text)).length, 2);
  assert.ok(evs.some((e) => e.item === "" && e.text === "about the board"));
  assert.ok(evs.some((e) => e.item === "i2" && e.text === "item is a number"));
  assert.equal(evs.some((e) => e.item === "i1" && e.text === "epoch t"), true);
  assert.equal(evs.some((e) => e.text === undefined), false);
  assert.ok(fs.readdirSync(dir).some((f) => f.includes(".quarantine-")), "the textless one is kept aside");
  assert.equal(repair(s).notes.length, 0, "nothing left to do");
  s.close();
  fs.rmSync(dir, { recursive: true, force: true });
});
