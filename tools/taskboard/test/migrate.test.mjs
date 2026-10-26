// migrate.mjs (#709): tasks holding items become folders, every item gets a number, `order` becomes
// the tree's order, slot goes; it keeps a copy, checks, and refuses a second run.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { openStore } from "../store.mjs";
import { main, problems } from "../migrate.mjs";

// A board as it was before #709: folders without numbers, a task holding tasks, per-folder orders, slots.
function before() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-migrate-"));
  const file = path.join(dir, "t.db");
  const store = openStore(file);
  store.db.exec(`ALTER TABLE items ADD COLUMN slot INTEGER`);
  const ins = (id, row) => {
    const names = ["id", ...Object.keys(row)];
    store.db.prepare(`INSERT INTO items (${names.map((n) => `"${n}"`).join(",")}) VALUES (${names.map(() => "?").join(",")})`).run(id, ...Object.values(row));
  };
  const t = "2026-10-01T00:00:00.000Z";
  ins("f-b", { kind: "folder", text: "Second folder", parent: "", order: 2, added: t });
  ins("f-a", { kind: "folder", text: "First folder", parent: "", order: 1, added: t });
  ins("i1", { kind: "task", text: "Holds two", parent: "f-a", order: 1, num: 1, stage: "done", where: "You", priority: 3, added: t });
  ins("i2", { kind: "task", text: "Inside a task", parent: "i1", order: 1, num: 2, stage: "", where: "You", added: t, slot: 1 });
  ins("i3", { kind: "task", text: "Inside a task too", parent: "i1", order: 2, num: 3, stage: "", where: "You", added: t });
  ins("i4", { kind: "task", text: "In the second", parent: "f-b", order: 1, num: 4, stage: "", where: "You", added: t });
  ins("i5", { kind: "task", text: "Top", parent: "", order: 3, num: 5, stage: "", where: "You", added: t });
  store.put("events", "e1", { item: "i1", kind: "note", t, text: "about #1" });
  store.put("events", "e2", { item: "f-b", kind: "note", t, text: "about the folder" });
  store.put("meta", "counter", { next: 9 });
  store.put("meta", "status", { now: "", updated: t });
  store.close();
  return { dir, file };
}
const quiet = (fn) => { const log = console.log, err = console.error; console.log = console.error = () => {}; try { return fn(); } finally { console.log = log; console.error = err; } };

test("a dry run changes nothing", () => {
  const { dir, file } = before();
  const dump = () => { const s = openStore(file); const d = JSON.stringify([s.list("items"), s.list("events"), s.list("meta")]); s.close(); return d; };
  const was = dump();
  assert.equal(quiet(() => main([file])), 0);
  assert.equal(dump(), was);
  assert.ok(!fs.readdirSync(dir).some((f) => f.includes(".before-709-")));
  fs.rmSync(dir, { recursive: true, force: true });
});

test("--run migrates, keeps a copy, and refuses a second run", () => {
  const { dir, file } = before();
  assert.equal(quiet(() => main([file, "--run"])), 0);
  assert.ok(fs.readdirSync(dir).some((f) => f.includes(".before-709-")));
  const s = openStore(file);
  assert.deepEqual(problems(s), []);
  const g = (id) => s.get("items", id);
  // The task holding items is a folder, its number kept, its task fields in an event.
  assert.equal(g("i1").kind, "folder");
  assert.equal(g("i1").num, 1);
  assert.equal(g("i1").stage, undefined);
  assert.ok(s.list("events").some((e) => e.data.item === "i1" && /stage: "done"/.test(e.data.text) && /priority: 3/.test(e.data.text)));
  // Numbers from the counter, in tree order; the counter past them.
  assert.equal(g("f-a").num, 9);
  assert.equal(g("f-b").num, 10);
  assert.equal(s.get("meta", "counter").next, 11);
  // Order: the tree, depth first.
  assert.deepEqual(["f-a", "i1", "i2", "i3", "f-b", "i4", "i5"].map((id) => g(id).order), [1, 2, 3, 4, 5, 6, 7]);
  // Parents and events as they were.
  assert.deepEqual(["i2", "i3", "i4", "i5"].map((id) => g(id).parent), ["i1", "i1", "f-b", ""]);
  assert.equal(s.list("events").length, 3);
  assert.equal(s.db.prepare(`SELECT 1 FROM pragma_table_info('items') WHERE name = 'slot'`).get(), undefined);
  s.close();
  assert.equal(quiet(() => main([file, "--run"])), 3, "already migrated");
  fs.rmSync(dir, { recursive: true, force: true });
});
