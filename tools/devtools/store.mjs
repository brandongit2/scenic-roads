// The task board's SQLite file: the tables, and documents <-> rows. No rules here (schema.mjs and
// board.mjs have them); a document is written as given, and a field no column holds isn't written.
import crypto from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { STATUSES } from "./schema.mjs";

// The file's form, its PRAGMA user_version (README.md: a change of a database's form sets it, so the
// M6's update can put the file back if the version that changed it doesn't start). A file of another
// form is refused at open.
export const FORM = 2;

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
// An open task in SQL (priority.mjs isOpen: neither done nor abandoned, not archived; no item that
// isn't archived is ever inside an archived folder, board.mjs). Its priority is indexed by this, and
// a query is answered from that index only when it says the same.
const OPEN_TASK = `kind = 'task' AND status NOT IN ('done', 'abandoned') AND archived IS NOT 1`;
// Every read the board makes often is answered from an index (#1344): a folder's items, a number's
// owner and the highest number, the last order, the open tasks a priority runs into; the events in
// time order, and one item's (its history, newest first, and its summary).
const INDEXES = `
  CREATE INDEX items_parent ON items(parent);
  CREATE UNIQUE INDEX items_num ON items(num);
  CREATE INDEX items_order ON items("order");
  CREATE INDEX items_open_priority ON items(priority) WHERE ${OPEN_TASK};
  CREATE INDEX events_t ON events(t, id);
  CREATE INDEX events_item ON events(item, t, id);`;

export const COLS = {
  items: ["kind", "text", "parent", "order", "num", "status", "assigned", "note", "reply", "priority", "discuss", "archived", "archivedAt", "added", "started", "finished", "photos"],
  events: ["item", "kind", "t", "text"],
};
// A list's order when it asks for none: the items by number, the events by time (oldest first). A
// field that isn't unique is followed by the id, in the same direction.
export const DEFAULT_ORDER = { items: "num", events: "t" };
const UNIQUE = new Set(["id", "num"]); // (no tie to break: an item's number is its own)
// How many changes the change log keeps (`since`, below): a reader further behind reads everything.
export const CHANGES_KEPT = 10000;
const BOOL = new Set(["discuss", "archived"]);
const JSONCOL = new Set(["photos"]);
const q = (c) => `"${c}"`;

function inTx(db, fn) {
  db.exec("BEGIN IMMEDIATE");
  try { const r = fn(); db.exec("COMMIT"); return r; } catch (e) { db.exec("ROLLBACK"); throw e; }
}

const toDoc = (col, row) => {
  const doc = {};
  for (const c of COLS[col]) {
    const v = row[c];
    if (v === null || v === undefined) continue;
    doc[c] = BOOL.has(c) ? !!v : JSONCOL.has(c) ? JSON.parse(v) : v;
  }
  return doc;
};

// A list read on connection `conn`: {total, rows}, `rows` an iterator of {id, data, at} in the order
// asked (`orderBy` a column or "id", else DEFAULT_ORDER; `dir`; then the id, in the same direction),
// `at` its place in that order ([its orderBy value, its id]: what `after` takes to go on from it).
// `limit` rows at most; `total`, how many there are in all (of one item's events, with `item`).
// `after` ([value, id]): only the rows after that place. (SQLite puts a null first going up, last
// going down.)
function listOn(conn, col, { orderBy, dir, limit, item, after } = {}) {
  const by = orderBy || DEFAULT_ORDER[col], desc = dir === "desc";
  const w = [], args = [];
  if (item !== undefined) { w.push("item = ?"); args.push(item); }
  const total = conn.prepare(`SELECT COUNT(*) AS n FROM ${col}${w.length ? ` WHERE ${w.join(" AND ")}` : ""}`).get(...args).n;
  if (after) {
    const [v, id] = after, c = q(by), gt = desc ? "<" : ">";
    if (by === "id") { w.push(`id ${gt} ?`); args.push(id); }
    else if (v === null) { w.push(desc ? `(${c} IS NULL AND id < ?)` : `((${c} IS NULL AND id > ?) OR ${c} IS NOT NULL)`); args.push(id); }
    else { w.push(`(${c} ${gt} ? OR (${c} = ? AND id ${gt} ?)${desc ? ` OR ${c} IS NULL` : ""})`); args.push(v, v, id); }
  }
  const d = desc ? "DESC" : "ASC";
  const sql = `SELECT * FROM ${col}${w.length ? ` WHERE ${w.join(" AND ")}` : ""} ORDER BY ${q(by)} ${d}${UNIQUE.has(by) ? "" : `, id ${d}`}${limit ? ` LIMIT ${limit}` : ""}`;
  const it = conn.prepare(sql).iterate(...args);
  const rows = (function* () { for (const r of it) yield { id: r.id, data: toDoc(col, r), at: [r[by], r.id] }; })();
  return { total, rows };
}

export function openStore(file) {
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
    } else if (form !== FORM) {
      throw new Error(`${file} is of form ${form} (PRAGMA user_version); this version reads form ${FORM}`);
    } else if (!cur.sql.includes(STATUS_CHECK)) {
      // (Rows with a status outside the list are refused by the copy: the rebuild fails, nothing changes.)
      inTx(db, () => db.exec(`${itemsTable("items_new")}; INSERT INTO items_new SELECT * FROM items; DROP TABLE items; ALTER TABLE items_new RENAME TO items;${INDEXES}`));
    }
  } catch (e) { db.close(); throw e; }

  const stmts = {};
  const stmt = (sql) => (stmts[sql] ||= db.prepare(sql));

  // ---- the change log: what each write changed, for the readers that hold a copy (`since`) -------
  // In memory: a mark is this process's (`<epoch>-<n>`), so one from before a restart, or older than
  // the last CHANGES_KEPT changes, is answered with everything. An event's change keeps its item (a
  // deleted one's too), for the history's summary.
  const epoch = crypto.randomBytes(4).toString("hex");
  let seq = 0;
  let changes = []; // [seq, col, id, item]
  const noted = (col, id, item) => {
    changes.push([++seq, col, id, item]);
    if (changes.length > 2 * CHANGES_KEPT) changes = changes.slice(-CHANGES_KEPT);
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
      noted(col, id, doc.item);
    },
    del(col, id) {
      const item = col === "events" ? stmt(`SELECT item FROM events WHERE id = ?`).get(id)?.item : undefined;
      const n = stmt(`DELETE FROM ${col} WHERE id = ?`).run(id).changes;
      if (n) noted(col, id, item);
      return n;
    },
    // A list read (listOn), whole, on this connection: [{id, data}].
    list(col, opts = {}) {
      if (col === "meta") return db.prepare(`SELECT key, value FROM meta ORDER BY key`).all().map((r) => ({ id: r.key, data: JSON.parse(r.value) }));
      return [...listOn(db, col, opts).rows].map(({ id, data }) => ({ id, data }));
    },
    // A read of its own, at one moment: a read-only connection in a transaction, so a list read as it
    // is sent (board.mjs) is the file as it was when it began, whatever is written meanwhile, and
    // `mark` the change log's then. {list(col, opts): listOn's, mark, close()}.
    snapshot() {
      const r = new DatabaseSync(file, { readOnly: true });
      r.exec("BEGIN");
      const mark = s.mark();
      return { list: (col, opts) => listOn(r, col, opts), mark, close() { try { r.exec("COMMIT"); } finally { r.close(); } } };
    },
    // The change log's mark now, and what changed in `col` after an earlier one: Map id → its item (an
    // event's), or null when the log can't say (another process's mark, or older than it keeps).
    mark: () => `${epoch}-${seq}`,
    changedSince(col, mark) {
      const m = /^([0-9a-f]+)-(\d+)$/.exec(mark || "");
      const n = m && m[1] === epoch ? Number(m[2]) : -1;
      if (n < 0 || n > seq || (changes.length && n < changes[0][0] - 1) || (!changes.length && n !== seq)) return null;
      const out = new Map();
      let i = changes.length;
      while (i > 0 && changes[i - 1][0] > n) i--;
      for (; i < changes.length; i++) if (changes[i][1] === col) out.set(changes[i][2], changes[i][3]);
      return out;
    },
    // Each item's events in short, for the lists that show a count and the latest change and nothing
    // of the history itself: {item ("" the board's own): [how many, the latest one's t, a mark]}. The
    // mark (the highest rowid: a write, an edit too, is a new row) changes with any write of the item's
    // events, and the count with a delete, so a reader holding a history knows when to read it again.
    // `items`: only those (one with no events left: null).
    history(items) {
      const out = {};
      if (items) {
        for (const it of items) { const r = stmt(`SELECT COUNT(*) AS n, MAX(t) AS last, MAX(rowid) AS v FROM events WHERE item = ?`).get(it); out[it] = r.n ? [r.n, r.last, r.v] : null; }
        return out;
      }
      for (const r of stmt(`SELECT item, COUNT(*) AS n, MAX(t) AS last, MAX(rowid) AS v FROM events GROUP BY item`).all()) out[r.item] = [r.n, r.last, r.v];
      return out;
    },
    // The open tasks (OPEN_TASK) whose priority is above `x`: [{id, priority}], from their index.
    openAbove: (x) => stmt(`SELECT id, priority FROM items WHERE ${OPEN_TASK} AND priority > ?`).all(x),
    count: (col) => stmt(`SELECT COUNT(*) AS n FROM ${col}`).get().n,
    childrenOf: (id) => stmt(`SELECT id FROM items WHERE parent = ?`).all(id).map((r) => r.id),
    eventsOf: (id) => stmt(`SELECT id FROM events WHERE item = ?`).all(id).map((r) => r.id),
    maxNum: () => stmt(`SELECT MAX(num) AS m FROM items`).get().m || 0,
    numOwner: (num) => stmt(`SELECT id FROM items WHERE num = ?`).get(num)?.id ?? null,
    maxOrder: () => stmt(`SELECT MAX("order") AS m FROM items`).get().m,
    close() { db.close(); },
  };
  return s;
}
