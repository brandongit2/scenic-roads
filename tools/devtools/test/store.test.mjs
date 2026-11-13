// The board's file: its form (PRAGMA user_version), and the one-time move of a form-0 file to form 1
// (#1229: status, assigned, meta/now, no extra columns, no item that isn't archived inside an
// archived folder). Run: node --test tools/devtools/test/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { openStore, FORM } from "../store.mjs";
import { itemErrors, eventErrors, metaErrors, errorText } from "../schema.mjs";

const tmp = (t) => { const dir = fs.mkdtempSync(path.join(os.tmpdir(), "taskboard-store-")); t.after(() => fs.rmSync(dir, { recursive: true, force: true })); return path.join(dir, "t.db"); };

// A file as the board wrote it before #1229 (form 0): its tables as they were.
function form0(file) {
  const db = new DatabaseSync(file);
  db.exec(`
    CREATE TABLE events (id TEXT PRIMARY KEY, item TEXT, kind TEXT, t TEXT, text TEXT, extra TEXT);
    CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE blobs (id TEXT PRIMARY KEY, type TEXT NOT NULL, data BLOB NOT NULL, sha256 TEXT, created TEXT);
    CREATE TABLE items (id TEXT PRIMARY KEY, kind TEXT, text TEXT, parent TEXT, "order" REAL, num INTEGER,
      stage TEXT CHECK (stage IN ('', 'working', 'backgrounded', 'integrating', 'review', 'waiting', 'done', 'abandoned')),
      "where" TEXT, note TEXT, reply TEXT, priority REAL, discuss INTEGER, archived INTEGER, archivedAt TEXT,
      added TEXT, started TEXT, finished TEXT, photos TEXT, extra TEXT);
    CREATE UNIQUE INDEX items_num_unique ON items(num) WHERE num IS NOT NULL;`);
  const item = db.prepare(`INSERT INTO items (id, kind, text, parent, "order", num, stage, "where", archived, archivedAt, added, photos) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '2026-10-26T08:00:00.000Z', ?)`);
  const T = "2026-11-01T00:00:00.000Z";
  // #1 archived, holding #2 archived and #3 archived holding #4, an open task: #1 and #3 must open.
  // #5 archived holding archived #6, #8 abandoned and #9, a folder holding #10 done: those three are
  // archived (closed, they keep no folder open), at the move's time.
  item.run("i1", "folder", "Top", "", 1, 1, null, null, 1, T, null);
  item.run("i2", "task", "Archived", "i1", 2, 2, "done", "You", 1, T, null);
  item.run("i3", "folder", "Inner", "i1", 3, 3, null, null, 1, T, null);
  item.run("i4", "task", "Left open", "i3", 4, 4, "waiting", "Subagent (Opus)", null, null, '["0123456789abcdef0123456789abcdef"]');
  item.run("i5", "folder", "All archived", "", 5, 5, null, null, 1, T, null);
  item.run("i6", "task", "Archived too", "i5", 6, 6, "", "Claude", 1, T, null);
  item.run("i7", "task", "Plain", "", 7, 7, "working", "Build Mac process (dem/bldfetch.py, started by hand)", 0, null, null);
  item.run("i8", "task", "Abandoned, left in", "i5", 8, 8, "abandoned", "You", null, null, null);
  item.run("i9", "folder", "Left in", "i5", 9, 9, null, null, 0, null, null);
  item.run("i10", "task", "Done, left in", "i9", 10, 10, "done", "You", null, null, null);
  const ev = db.prepare(`INSERT INTO events (id, item, kind, t, text) VALUES (?, ?, ?, ?, ?)`);
  ev.run("e1", "i7", "stage", T, "Stage Queued → Under way (by curl)");
  ev.run("e2", "i7", "stage", T, "agent started (Opus)");
  ev.run("e3", "i7", "note", T, "Stage corrected: under way");
  ev.run("e4", "i7", "stage", T, "Stage set by hand, see the note");
  db.prepare(`INSERT INTO meta (key, value) VALUES (?, ?)`).run("status", JSON.stringify({ now: "On #7", updated: T }));
  db.prepare(`INSERT INTO meta (key, value) VALUES (?, ?)`).run("counter", JSON.stringify({ next: 8 }));
  db.close();
}

test("a new file is made at this form", (t) => {
  const file = tmp(t);
  openStore(file).close();
  const db = new DatabaseSync(file);
  assert.equal(db.prepare("PRAGMA user_version").get().user_version, FORM);
  assert.deepEqual(db.prepare("SELECT name FROM pragma_table_info('items')").all().map((r) => r.name).filter((n) => ["stage", "where", "extra", "status", "assigned"].includes(n)), ["status", "assigned"]);
  assert.deepEqual(db.prepare("SELECT name FROM pragma_table_info('events')").all().map((r) => r.name), ["id", "item", "kind", "t", "text"]);
  db.close();
});

test("a form-0 file is moved to form 1 once: names, history, the now line, no extra columns, no open item in an archived folder", (t) => {
  const file = tmp(t);
  form0(file);
  const said = [];
  const s = openStore(file, { log: (m) => said.push(m) });
  assert.equal(said.length, 1);
  assert.equal(said[0], `${file}: moved to form 1 (status, assigned, meta/now); unarchived #1 (it held #4), #3 (it held #4); archived #8 (inside #5), #9 (inside #5), #10 (inside #5)`);
  assert.deepEqual(s.get("items", "i4"), { kind: "task", text: "Left open", parent: "i3", order: 4, num: 4, status: "waiting", assigned: "Subagent (Opus)", added: "2026-10-26T08:00:00.000Z", photos: ["0123456789abcdef0123456789abcdef"] });
  assert.equal(s.get("items", "i7").assigned, "Build Mac process (dem/bldfetch.py, started by hand)", "a value the old list had, kept");
  assert.deepEqual(["i1", "i2", "i3", "i5", "i6", "i8", "i9", "i10"].map((id) => s.get("items", id).archived), [false, true, false, true, true, true, true, true]);
  const at = s.get("items", "i8").archivedAt;
  assert.ok(at > "2026-11-01T00:00:00.000Z", "archived at the move's time, not the folder's");
  assert.deepEqual(["i9", "i10"].map((id) => s.get("items", id).archivedAt), [at, at]);
  assert.match(s.list("events").filter((e) => e.data.item === "i10" && e.data.kind === "archive")[0].data.text, /^Archived when the board moved to its new names \(#1229\): it was inside archived folder #5/);
  assert.equal("archivedAt" in s.get("items", "i1"), false);
  assert.deepEqual(s.get("events", "e1"), { item: "i7", kind: "status", t: "2026-11-01T00:00:00.000Z", text: "Status Queued → Under way (by curl)" });
  assert.deepEqual(s.get("events", "e2").kind, "status");
  assert.equal(s.get("events", "e2").text, "agent started (Opus)", "only the server's own words change");
  assert.equal(s.get("events", "e3").text, "Stage corrected: under way", "a note stays as written");
  assert.deepEqual(s.get("events", "e4"), { item: "i7", kind: "status", t: "2026-11-01T00:00:00.000Z", text: "Stage set by hand, see the note" }, "words of its own in a status event stay");
  assert.match(s.list("events").filter((e) => e.data.item === "i1" && e.data.kind === "archive")[0].data.text, /^Unarchived when the board moved to its new names \(#1229\): it held #4, open and not archived/);
  assert.deepEqual(s.get("meta", "now"), { text: "On #7", updated: "2026-11-01T00:00:00.000Z" });
  assert.equal(s.get("meta", "status"), null);
  assert.deepEqual(s.get("meta", "counter"), { next: 8 });
  // (Everything in it fits the schema.)
  for (const { id, data } of s.list("items")) assert.deepEqual(itemErrors(data), [], `items/${id}: ${errorText(itemErrors(data))}`);
  for (const { id, data } of s.list("events")) assert.deepEqual(eventErrors(data), [], `events/${id}`);
  assert.deepEqual(metaErrors("now", s.get("meta", "now")), []);
  const cols = (tbl) => s.db.prepare(`SELECT name FROM pragma_table_info('${tbl}')`).all().map((r) => r.name);
  assert.equal(cols("items").includes("extra") || cols("events").includes("extra"), false);
  assert.equal(s.db.prepare("PRAGMA user_version").get().user_version, 1);
  s.close();
  // (Opened again: nothing more to do.)
  const again = [];
  openStore(file, { log: (m) => again.push(m) }).close();
  assert.deepEqual(again, []);
});

test("a migration that fails leaves the file as it was", (t) => {
  const file = tmp(t);
  form0(file);
  // (A failure part way, stood in for by a table in the way of the move's events step: the items
  // have been moved by then.)
  const db = new DatabaseSync(file);
  db.exec(`CREATE TABLE events_new (x)`);
  db.close();
  assert.throws(() => openStore(file), /events_new already exists/);
  const after = new DatabaseSync(file);
  assert.equal(after.prepare("PRAGMA user_version").get().user_version, 0);
  assert.equal(after.prepare("SELECT name FROM pragma_table_info('items')").all().some((r) => r.name === "stage"), true);
  assert.equal(after.prepare("SELECT value FROM meta WHERE key = 'status'").get() !== undefined, true);
  assert.equal(after.prepare("SELECT archived FROM items WHERE id = 'i1'").get().archived, 1);
  after.close();
});

test("a file of a form this version doesn't read is refused, untouched", (t) => {
  const file = tmp(t);
  openStore(file).close();
  const db = new DatabaseSync(file);
  db.exec("PRAGMA user_version = 7");
  db.close();
  assert.throws(() => openStore(file), /is of form 7 \(PRAGMA user_version\); this version reads form 1/);
});
