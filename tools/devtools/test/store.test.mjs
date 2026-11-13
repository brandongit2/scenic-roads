// The board's file: its form (PRAGMA user_version). Run: node --test tools/devtools/test/*.test.mjs
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

test("a file of a form this version doesn't read is refused, untouched (form 0 too)", (t) => {
  for (const form of [0, 1, 7]) {
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
