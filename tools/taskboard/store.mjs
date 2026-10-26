// The task board's SQLite file: the tables, and documents <-> rows. No rules here (schema.mjs and
// server.mjs have them); a document is written as given, and one with a field no column holds is
// read back with it (a legacy row's `extra`, which repair.mjs clears) but never written with one.
import { DatabaseSync } from "node:sqlite";
import { STAGES } from "./schema.mjs";

// A task's stage is also enforced by the table itself (a CHECK constraint). A changed list rebuilds
// the table at open.
const STAGE_CHECK = `CHECK (stage IN (${STAGES.map((s) => `'${s}'`).join(", ")}))`;
const itemsTable = (name) => `CREATE TABLE ${name} (
    id TEXT PRIMARY KEY,
    kind TEXT,                 -- 'task' | 'folder'
    text TEXT,
    parent TEXT,
    "order" REAL,
    num INTEGER,               -- the item's number, #57
    stage TEXT ${STAGE_CHECK},  -- schema.mjs STAGES, '' queued
    "where" TEXT,              -- who or what it runs on
    note TEXT,
    reply TEXT,
    priority REAL,
    discuss INTEGER,           -- 0/1
    archived INTEGER,          -- 0/1
    archivedAt TEXT,
    added TEXT, started TEXT, finished TEXT,   -- ISO times
    photos TEXT,               -- JSON array of blob ids
    extra TEXT                 -- JSON: fields of old rows no column holds (never written now)
  )`;
const ITEMS_INDEXES = `
  CREATE INDEX IF NOT EXISTS items_parent ON items(parent);
  CREATE INDEX IF NOT EXISTS items_num ON items(num);
  CREATE UNIQUE INDEX IF NOT EXISTS items_num_unique ON items(num) WHERE num IS NOT NULL;`;

export const COLS = {
  items: ["kind", "text", "parent", "order", "num", "stage", "where", "note", "reply", "priority", "discuss", "archived", "archivedAt", "added", "started", "finished", "photos"],
  events: ["item", "kind", "t", "text"],
};
const BOOL = new Set(["discuss", "archived"]);
const JSONCOL = new Set(["photos"]);
const q = (c) => `"${c}"`;

export function openStore(file) {
  const db = new DatabaseSync(file);
  db.exec(`
    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;
    CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, item TEXT, kind TEXT, t TEXT, text TEXT, extra TEXT);
    CREATE INDEX IF NOT EXISTS events_item ON events(item);
    CREATE INDEX IF NOT EXISTS events_t ON events(t);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);   -- JSON
    CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, type TEXT NOT NULL, data BLOB NOT NULL, sha256 TEXT, created TEXT);
  `);
  const cur = db.prepare(`SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'items'`).get();
  if (!cur) db.exec(itemsTable("items") + ";" + ITEMS_INDEXES);
  else if (!cur.sql.includes(STAGE_CHECK)) {
    // (Rows with a stage outside the list are refused by the copy: the rebuild fails, nothing changes.)
    db.exec("BEGIN IMMEDIATE");
    try {
      db.exec(`${itemsTable("items_new")}; INSERT INTO items_new SELECT * FROM items; DROP TABLE items; ALTER TABLE items_new RENAME TO items;${ITEMS_INDEXES}`);
      db.exec("COMMIT");
    } catch (e) { db.exec("ROLLBACK"); throw e; }
  }

  const stmts = {};
  const stmt = (sql) => (stmts[sql] ||= db.prepare(sql));
  const toDoc = (col, row) => {
    const doc = {};
    for (const c of COLS[col]) {
      const v = row[c];
      if (v === null || v === undefined) continue;
      doc[c] = BOOL.has(c) ? !!v : JSONCOL.has(c) ? JSON.parse(v) : v;
    }
    if (row.extra) Object.assign(doc, JSON.parse(row.extra));
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
      const names = ["id", ...COLS[col], "extra"];
      stmt(`INSERT OR REPLACE INTO ${col} (${names.map(q).join(", ")}) VALUES (${names.map(() => "?").join(", ")})`).run(id, ...vals, null);
    },
    del(col, id) { return stmt(`DELETE FROM ${col} WHERE id = ?`).run(id).changes; },
    // [{id, data}], in the order asked (an `orderBy` that is a column; else by id).
    list(col, { orderBy, dir, limit } = {}) {
      if (col === "meta") return db.prepare(`SELECT key, value FROM meta ORDER BY key`).all().map((r) => ({ id: r.key, data: JSON.parse(r.value) }));
      const o = orderBy && (COLS[col].includes(orderBy) || orderBy === "id") ? ` ORDER BY ${q(orderBy)} ${dir === "desc" ? "DESC" : "ASC"}, id` : " ORDER BY id";
      const lim = Number.isFinite(limit) && limit > 0 ? ` LIMIT ${Math.min(limit, 10000)}` : "";
      return db.prepare(`${SELECT[col]}${o}${lim}`).all().map((r) => ({ id: r.id, data: toDoc(col, r) }));
    },
    count: (col) => (col === "meta" ? stmt(`SELECT COUNT(*) AS n FROM meta`) : stmt(`SELECT COUNT(*) AS n FROM ${col}`)).get().n,
    childrenOf: (id) => stmt(`SELECT id FROM items WHERE parent = ?`).all(id).map((r) => r.id),
    eventsOf: (id) => stmt(`SELECT id FROM events WHERE item = ?`).all(id).map((r) => r.id),
    maxNum: () => stmt(`SELECT MAX(num) AS m FROM items`).get().m || 0,
    numOwner: (num) => stmt(`SELECT id FROM items WHERE num = ?`).get(num)?.id ?? null,
    maxOrder: () => stmt(`SELECT MAX("order") AS m FROM items`).get().m,
    close() { db.close(); },
  };
  return s;
}
