// The board's file: its form (PRAGMA user_version), and the one-time move of a form-1 file to form 2
// (#1344: the indexes the hot reads are answered from). Run: node --test tools/devtools/test/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { openStore, FORM } from "../store.mjs";

const tmp = (t) => { const dir = fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-store-")); t.after(() => fs.rmSync(dir, { recursive: true, force: true })); return path.join(dir, "t.db"); };
const indexes = (db) => db.prepare("SELECT name, sql FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL ORDER BY name").all().map((r) => `${r.name}: ${r.sql.replace(/\s+/g, " ")}`);
const NOW = [
  `events_item: CREATE INDEX events_item ON events(item, t, id)`,
  `events_t: CREATE INDEX events_t ON events(t, id)`,
  `items_num: CREATE UNIQUE INDEX items_num ON items(num)`,
  `items_open_priority: CREATE INDEX items_open_priority ON items(priority) WHERE kind = 'task' AND status NOT IN ('done', 'abandoned') AND archived IS NOT 1`,
  `items_order: CREATE INDEX items_order ON items("order")`,
  `items_parent: CREATE INDEX items_parent ON items(parent)`,
];

// A file as the board wrote it at form 1 (#1229 to #1344): its tables, and its indexes as they were.
function form1(file) {
  openStore(file).close();
  const db = new DatabaseSync(file);
  db.exec(`${NOW.map((l) => `DROP INDEX ${l.split(":")[0]};`).join(" ")}
    CREATE INDEX items_parent ON items(parent);
    CREATE INDEX items_num ON items(num);
    CREATE UNIQUE INDEX items_num_unique ON items(num) WHERE num IS NOT NULL;
    CREATE INDEX events_item ON events(item);
    CREATE INDEX events_t ON events(t);
    PRAGMA user_version = 1;`);
  db.prepare(`INSERT INTO items (id, kind, text, parent, "order", num, status, assigned, priority, added) VALUES ('i1', 'task', 'One', '', 1, 1, '', 'You', 3, '2026-11-01T00:00:00.000Z')`).run();
  db.prepare(`INSERT INTO events (id, item, kind, t, text) VALUES ('e1', 'i1', 'note', '2026-11-01T00:00:00.000Z', 'hello')`).run();
  db.close();
}

test("a new file is made at this form, with its indexes", (t) => {
  const file = tmp(t);
  openStore(file).close();
  const db = new DatabaseSync(file);
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, FORM);
  assert.equal(FORM, 2);
  assert.deepEqual(indexes(db), NOW);
  assert.deepEqual(db.prepare("SELECT name FROM pragma_table_info('events')").all().map((r) => r.name), ["id", "item", "kind", "t", "text"]);
  db.close();
});

test("a form-1 file is moved to form 2 once: its indexes, its data as it was (ONE-TIME, #1344: delete with migrateTo2)", (t) => {
  const file = tmp(t);
  form1(file);
  const said = [];
  const s = openStore(file, { log: (m) => said.push(m) });
  assert.deepEqual(said, [`${file}: moved to form 2 (#1344's indexes)`]);
  assert.equal(s.db.prepare("PRAGMA user_version").get().user_version, 2);
  assert.deepEqual(indexes(s.db), NOW);
  assert.deepEqual(s.get("items", "i1"), { kind: "task", text: "One", parent: "", order: 1, num: 1, status: "", assigned: "You", priority: 3, added: "2026-11-01T00:00:00.000Z" });
  assert.deepEqual(s.get("events", "e1"), { item: "i1", kind: "note", t: "2026-11-01T00:00:00.000Z", text: "hello" });
  s.close();
  const again = [];
  openStore(file, { log: (m) => again.push(m) }).close();
  assert.deepEqual(again, [], "opened again: nothing more to do");
});

test("a move that fails leaves the file as it was", (t) => {
  const file = tmp(t);
  form1(file);
  // (A failure part way, stood in for by a table in the way of an index it makes: form 1's are
  // dropped by then.)
  const db = new DatabaseSync(file);
  db.exec(`CREATE TABLE items_order (x)`);
  const before = indexes(db);
  db.close();
  assert.throws(() => openStore(file), /items_order/);
  const after = new DatabaseSync(file);
  assert.equal(after.prepare("PRAGMA user_version").get().user_version, 1);
  assert.deepEqual(indexes(after), before);
  after.close();
});

test("a file of a form this version doesn't read is refused, untouched (form 0 too)", (t) => {
  for (const form of [0, 7]) {
    const file = tmp(t);
    openStore(file).close();
    const db = new DatabaseSync(file);
    db.exec(`PRAGMA user_version = ${form}`);
    db.close();
    assert.throws(() => openStore(file), new RegExp(`is of form ${form} \\(PRAGMA user_version\\); this version reads form 2`));
    const after = new DatabaseSync(file);
    assert.equal(after.prepare("PRAGMA user_version").get().user_version, form);
    after.close();
  }
});
