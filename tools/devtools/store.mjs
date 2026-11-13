// The task board's SQLite file: the tables, and documents <-> rows. No rules here (schema.mjs and
// board.mjs have them); a document is written as given, and a field no column holds isn't written.
import { DatabaseSync } from "node:sqlite";
import { STATUSES } from "./schema.mjs";

// The file's form, its PRAGMA user_version (README.md: a change of a database's form sets it, so the
// M6's update can put the file back if the version that changed it doesn't start). 1: #1229's names
// (status, assigned, meta/now). A file of another form is refused at open, except form 0, the one
// before it, which is moved to this form once (migrateTo1).
export const FORM = 1;

// A task's status is also enforced by the table itself (a CHECK constraint). A changed list rebuilds
// the table at open.
const STATUS_CHECK = `CHECK (status IN (${STATUSES.map((s) => `'${s}'`).join(", ")}))`;
const itemsTable = (name) => `CREATE TABLE ${name} (
    id TEXT PRIMARY KEY,
    kind TEXT,                 -- 'task' | 'folder'
    text TEXT,
    parent TEXT,
    "order" REAL,
    num INTEGER,               -- the item's number, #57
    status TEXT ${STATUS_CHECK},  -- schema.mjs STATUSES, '' queued
    assigned TEXT,             -- who or what it's assigned to (free text)
    note TEXT,
    reply TEXT,
    priority REAL,
    discuss INTEGER,           -- 0/1
    archived INTEGER,          -- 0/1
    archivedAt TEXT,
    added TEXT, started TEXT, finished TEXT,   -- ISO times
    photos TEXT                -- JSON array of blob ids
  )`;
const eventsTable = (name) => `CREATE TABLE ${name} (id TEXT PRIMARY KEY, item TEXT, kind TEXT, t TEXT, text TEXT)`;
const INDEXES = `
  CREATE INDEX IF NOT EXISTS items_parent ON items(parent);
  CREATE INDEX IF NOT EXISTS items_num ON items(num);
  CREATE UNIQUE INDEX IF NOT EXISTS items_num_unique ON items(num) WHERE num IS NOT NULL;
  CREATE INDEX IF NOT EXISTS events_item ON events(item);
  CREATE INDEX IF NOT EXISTS events_t ON events(t);`;

export const COLS = {
  items: ["kind", "text", "parent", "order", "num", "status", "assigned", "note", "reply", "priority", "discuss", "archived", "archivedAt", "added", "started", "finished", "photos"],
  events: ["item", "kind", "t", "text"],
};
// (The events a list keeps: one item's, and those after a given event.)
const eventsWhere = (item, before) => {
  const w = [], args = [];
  if (item !== undefined) { w.push("item = ?"); args.push(item); }
  if (before) { w.push("(t < ? OR (t = ? AND id > ?))"); args.push(before[0], before[0], before[1]); }
  return [w.length ? ` WHERE ${w.join(" AND ")}` : "", args];
};
const BOOL = new Set(["discuss", "archived"]);
const JSONCOL = new Set(["photos"]);
const q = (c) => `"${c}"`;

function inTx(db, fn) {
  db.exec("BEGIN IMMEDIATE");
  try { const r = fn(); db.exec("COMMIT"); return r; } catch (e) { db.exec("ROLLBACK"); throw e; }
}

// ONE-TIME (#1229). DELETE this function, its call in openStore and its tests (test/store.test.mjs)
// once the M6's board is at form 1: board task #1229's rollout says so.
// Form 0 → 1, in one transaction (a failure leaves the file as it was). Items' stage and where become
// status and assigned; the unused extra columns of items and events go; status events (kind "stage")
// become kind "status", and the server's own words in them ("Stage Queued → Under way (by curl)")
// "Status …", anything else written in them left as it is; meta/status {now} becomes meta/now {text}.
// And no item that isn't archived may be inside an archived folder (board.mjs keeps it so from now
// on): an archived folder holding an open task (one neither done nor abandoned, not archived), at any
// depth, is unarchived; what is left not archived inside an archived folder after that (done and
// abandoned tasks, folders with no open task) is archived, now, as archiving the folder now would. Each leaves an `archive` event saying why. Returns
// {unarchived: [{num, for: [num]}], archived: [{num, in: num}]}.
function migrateTo1(db) {
  return inTx(db, () => {
    db.exec(`${itemsTable("items_new")};
      INSERT INTO items_new (id, kind, text, parent, "order", num, status, assigned, note, reply, priority, discuss, archived, archivedAt, added, started, finished, photos)
        SELECT id, kind, text, parent, "order", num, stage, "where", note, reply, priority, discuss, archived, archivedAt, added, started, finished, photos FROM items;
      DROP TABLE items; ALTER TABLE items_new RENAME TO items;
      ${eventsTable("events_new")};
      INSERT INTO events_new (id, item, kind, t, text)
        SELECT id, item, CASE kind WHEN 'stage' THEN 'status' ELSE kind END, t,
          CASE WHEN kind = 'stage' AND text GLOB 'Stage * (by *)' THEN 'Status ' || substr(text, 7) ELSE text END FROM events;
      DROP TABLE events; ALTER TABLE events_new RENAME TO events;
      INSERT INTO meta (key, value) SELECT 'now', json_object('text', coalesce(json_extract(value, '$.now'), ''), 'updated', json_extract(value, '$.updated')) FROM meta WHERE key = 'status';
      DELETE FROM meta WHERE key = 'status';
      ${INDEXES}`);
    const rows = db.prepare(`SELECT id, kind, parent, num, status, archived, archivedAt FROM items`).all();
    const byId = new Map(rows.map((r) => [r.id, r]));
    const up = (r, f) => { for (let p = byId.get(r.parent), n = 0; p && n < 10000; p = byId.get(p.parent), n++) if (f(p)) return p; return null; };
    const t = new Date().toISOString();
    const note = db.prepare(`INSERT INTO events (id, item, kind, t, text) VALUES (?, ?, 'archive', ?, ?)`);
    const why = new Map(); // archived folder id → the numbers of the open tasks inside it
    for (const r of rows) {
      if (r.archived || r.kind !== "task" || r.status === "done" || r.status === "abandoned") continue;
      up(r, (p) => { if (p.archived) (why.get(p.id) || why.set(p.id, []).get(p.id)).push(r.num); return false; });
    }
    const unarchived = [];
    for (const [id, nums] of [...why].sort((a, b) => byId.get(a[0]).num - byId.get(b[0]).num)) {
      const r = byId.get(id);
      r.archived = 0;
      db.prepare(`UPDATE items SET archived = 0, archivedAt = NULL WHERE id = ?`).run(id);
      note.run(crypto.randomUUID(), id, t, `Unarchived when the board moved to its new names (#1229): it held ${nums.map((n) => `#${n}`).join(", ")}, open and not archived, and nothing that isn't archived may be inside an archived folder`);
      unarchived.push({ num: r.num, for: nums });
    }
    const archived = [];
    for (const r of [...rows].sort((a, b) => a.num - b.num)) {
      if (r.archived) continue;
      const a = up(r, (p) => p.archived);
      if (!a) continue;
      db.prepare(`UPDATE items SET archived = 1, archivedAt = ? WHERE id = ?`).run(t, r.id);
      note.run(crypto.randomUUID(), r.id, t, `Archived when the board moved to its new names (#1229): it was inside archived folder #${a.num}, and nothing that isn't archived may be inside an archived folder`);
      archived.push({ num: r.num, in: a.num });
    }
    db.exec(`PRAGMA user_version = ${FORM}`);
    return { unarchived, archived };
  });
}

// `log`: where the open says what it changed in the file (a migration), e.g. console.log.
export function openStore(file, { log = () => {} } = {}) {
  const db = new DatabaseSync(file);
  db.exec(`
    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);   -- JSON
    CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, type TEXT NOT NULL, data BLOB NOT NULL, sha256 TEXT, created TEXT);
  `);
  try {
    const form = db.prepare(`PRAGMA user_version`).get().user_version;
    const cur = db.prepare(`SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'items'`).get();
    if (!cur) {
      inTx(db, () => db.exec(`${itemsTable("items")}; ${eventsTable("events")}; ${INDEXES} PRAGMA user_version = ${FORM};`));
    } else if (form === 0) {
      const { unarchived: un, archived: ar } = migrateTo1(db);
      log(`${file}: moved to form ${FORM} (status, assigned, meta/now)${un.length ? `; unarchived ${un.map((u) => `#${u.num} (it held ${u.for.map((n) => `#${n}`).join(", ")})`).join(", ")}` : ""}${ar.length ? `; archived ${ar.map((a) => `#${a.num} (inside #${a.in})`).join(", ")}` : ""}`);
    } else if (form !== FORM) {
      throw new Error(`${file} is of form ${form} (PRAGMA user_version); this version reads form ${FORM}`);
    } else if (!cur.sql.includes(STATUS_CHECK)) {
      // (Rows with a status outside the list are refused by the copy: the rebuild fails, nothing changes.)
      inTx(db, () => db.exec(`${itemsTable("items_new")}; INSERT INTO items_new SELECT * FROM items; DROP TABLE items; ALTER TABLE items_new RENAME TO items;${INDEXES}`));
    }
  } catch (e) { db.close(); throw e; }

  const stmts = {};
  const stmt = (sql) => (stmts[sql] ||= db.prepare(sql));
  const toDoc = (col, row) => {
    const doc = {};
    for (const c of COLS[col]) {
      const v = row[c];
      if (v === null || v === undefined) continue;
      doc[c] = BOOL.has(c) ? !!v : JSONCOL.has(c) ? JSON.parse(v) : v;
    }
    return doc;
  };
  const SELECT = { items: `SELECT * FROM items`, events: `SELECT * FROM events` };
  const s = {
    db,
    stmt,
    get(col, id) {
      if (col === "meta") { const r = stmt(`SELECT value FROM meta WHERE key = ?`).get(id); return r ? JSON.parse(r.value) : null; }
      const r = stmt(`${SELECT[col]} WHERE id = ?`).get(id);
      return r ? toDoc(col, r) : null;
    },
    // Writes the document whole (a field it lacks is gone). Fields no column holds aren't written.
    put(col, id, doc) {
      if (col === "meta") { stmt(`INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value`).run(id, JSON.stringify(doc)); return; }
      const vals = COLS[col].map((c) => { const v = doc[c]; return v == null ? null : BOOL.has(c) ? (v ? 1 : 0) : JSONCOL.has(c) ? JSON.stringify(v) : v; });
      const names = ["id", ...COLS[col]];
      stmt(`INSERT OR REPLACE INTO ${col} (${names.map(q).join(", ")}) VALUES (${names.map(() => "?").join(", ")})`).run(id, ...vals);
    },
    del(col, id) { return stmt(`DELETE FROM ${col} WHERE id = ?`).run(id).changes; },
    // [{id, data}], in the order asked (an `orderBy` that is a column, or "id"; else by id). Events
    // only: `item` keeps one item's events ("" the board's own); `before` ([t, id]) the ones after that
    // event in the order t newest first, then id (the next page of a history, read from where the last
    // one ended).
    list(col, { orderBy, dir, limit, item, before } = {}) {
      if (col === "meta") return db.prepare(`SELECT key, value FROM meta ORDER BY key`).all().map((r) => ({ id: r.key, data: JSON.parse(r.value) }));
      const o = orderBy && (COLS[col].includes(orderBy) || orderBy === "id") ? ` ORDER BY ${q(orderBy)} ${dir === "desc" ? "DESC" : "ASC"}, id` : " ORDER BY id";
      const lim = Number.isFinite(limit) && limit > 0 ? ` LIMIT ${Math.min(limit, 10000)}` : "";
      const [where, args] = eventsWhere(item, before);
      return db.prepare(`${SELECT[col]}${where}${o}${lim}`).all(...args).map((r) => ({ id: r.id, data: toDoc(col, r) }));
    },
    // How many there are (of one item's events, with `item`).
    count: (col, item) => (item === undefined ? stmt(`SELECT COUNT(*) AS n FROM ${col}`) : stmt(`SELECT COUNT(*) AS n FROM events WHERE item = ?`)).get(...(item === undefined ? [] : [item])).n,
    // Each item's events in short, for the lists that show a count and the latest change and nothing
    // of the history itself: {item ("" the board's own): [how many, the latest one's t, a mark]}. The
    // mark (the highest rowid: a write, an edit too, is a new row) changes with any write of the item's
    // events, and the count with a delete, so a reader holding a history knows when to read it again.
    history() {
      const out = {};
      for (const r of stmt(`SELECT item, COUNT(*) AS n, MAX(t) AS last, MAX(rowid) AS v FROM events GROUP BY item`).all()) out[r.item] = [r.n, r.last, r.v];
      return out;
    },
    childrenOf: (id) => stmt(`SELECT id FROM items WHERE parent = ?`).all(id).map((r) => r.id),
    eventsOf: (id) => stmt(`SELECT id FROM events WHERE item = ?`).all(id).map((r) => r.id),
    maxNum: () => stmt(`SELECT MAX(num) AS m FROM items`).get().m || 0,
    numOwner: (num) => stmt(`SELECT id FROM items WHERE num = ?`).get(num)?.id ?? null,
    maxOrder: () => stmt(`SELECT MAX("order") AS m FROM items`).get().m,
    close() { db.close(); },
  };
  return s;
}
