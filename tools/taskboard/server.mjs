// The Scenic Roads task board: a standalone web app. One Node process (no dependencies; Node 22.5+
// for node:sqlite) serves the page in public/ and keeps its data in a SQLite file.
//
//   node server.mjs [--port 8090] [--db taskboard.db]
//
// The page is the Claude artifact's, unchanged; public/shim.js gives it the document-database API it
// was written against (collection/doc/get/set/update/delete/onSnapshot/acquire, and photo uploads)
// over this server's HTTP API: a change is written here, and every open tab is told through an
// event stream to read the collection again.
import { DatabaseSync } from "node:sqlite";
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const arg = (name, d) => { const i = process.argv.indexOf("--" + name); return i > 0 ? process.argv[i + 1] : d; };
const PORT = Number(arg("port", process.env.PORT || 8090));
const HOST = arg("host", process.env.HOST || "127.0.0.1");
const DB_PATH = path.resolve(arg("db", process.env.TASKBOARD_DB || path.join(here, "taskboard.db")));

const db = new DatabaseSync(DB_PATH);
db.exec(`
  PRAGMA journal_mode = WAL;
  PRAGMA foreign_keys = ON;
  CREATE TABLE IF NOT EXISTS items (
    id TEXT PRIMARY KEY,
    kind TEXT,                 -- 'task' | 'folder'
    text TEXT,
    parent TEXT,
    "order" REAL,
    num INTEGER,               -- a task's number, #57
    stage TEXT,                -- '' (queued) | working | integrating | review | waiting | done
    "where" TEXT,              -- who or what it runs on
    note TEXT,
    reply TEXT,
    priority REAL,
    slot INTEGER,
    discuss INTEGER,           -- 0/1
    archived INTEGER,          -- 0/1
    archivedAt TEXT,
    added TEXT, started TEXT, finished TEXT,   -- ISO times
    photos TEXT,               -- JSON array of blob ids
    extra TEXT                 -- JSON: any other field
  );
  CREATE INDEX IF NOT EXISTS items_parent ON items(parent);
  CREATE INDEX IF NOT EXISTS items_num ON items(num);
  CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    item TEXT,
    kind TEXT,
    t TEXT,
    text TEXT,
    extra TEXT
  );
  CREATE INDEX IF NOT EXISTS events_item ON events(item);
  CREATE INDEX IF NOT EXISTS events_t ON events(t);
  CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);   -- JSON
  CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, type TEXT NOT NULL, data BLOB NOT NULL, sha256 TEXT, created TEXT);
`);

// ---- documents <-> rows -------------------------------------------------------------------------
const BOOL = new Set(["discuss", "archived"]);
const JSONCOL = new Set(["photos"]);
const COLS = {
  items: ["kind", "text", "parent", "order", "num", "stage", "where", "note", "reply", "priority", "slot", "discuss", "archived", "archivedAt", "added", "started", "finished", "photos"],
  events: ["item", "kind", "t", "text"],
};
const q = (c) => `"${c}"`;

function toRow(col, doc) {
  const cols = COLS[col], row = {}, extra = {};
  for (const [k, v] of Object.entries(doc)) {
    if (v === null || v === undefined) continue; // (a field that goes is null: not stored)
    if (cols.includes(k)) row[k] = BOOL.has(k) ? (v ? 1 : 0) : JSONCOL.has(k) ? JSON.stringify(v) : v;
    else extra[k] = v;
  }
  row.extra = Object.keys(extra).length ? JSON.stringify(extra) : null;
  return row;
}
function toDoc(col, row) {
  const doc = {};
  for (const c of COLS[col]) {
    const v = row[c];
    if (v === null || v === undefined) continue;
    doc[c] = BOOL.has(c) ? !!v : JSONCOL.has(c) ? JSON.parse(v) : v;
  }
  if (row.extra) Object.assign(doc, JSON.parse(row.extra));
  return doc;
}

const stmts = {};
const stmt = (sql) => (stmts[sql] ||= db.prepare(sql));
const SELECT = {
  items: `SELECT * FROM items`,
  events: `SELECT * FROM events`,
};

function getDoc(col, id) {
  if (col === "meta") {
    const r = stmt(`SELECT value FROM meta WHERE key = ?`).get(id);
    return r ? JSON.parse(r.value) : null;
  }
  const r = stmt(`${SELECT[col]} WHERE id = ?`).get(id);
  return r ? toDoc(col, r) : null;
}
function putDoc(col, id, doc) {
  if (col === "meta") {
    stmt(`INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value`).run(id, JSON.stringify(doc));
    return;
  }
  const row = toRow(col, doc);
  const names = ["id", ...COLS[col], "extra"];
  const vals = names.map((n) => (n === "id" ? id : row[n] ?? null));
  stmt(`INSERT OR REPLACE INTO ${col} (${names.map(q).join(", ")}) VALUES (${names.map(() => "?").join(", ")})`).run(...vals);
}
function delDoc(col, id) {
  if (col === "meta") return stmt(`DELETE FROM meta WHERE key = ?`).run(id).changes;
  return stmt(`DELETE FROM ${col} WHERE id = ?`).run(id).changes;
}
function listDocs(col, { orderBy, dir, limit }) {
  if (col === "meta") return db.prepare(`SELECT key, value FROM meta`).all().map((r) => ({ id: r.key, data: JSON.parse(r.value) }));
  const o = orderBy && (COLS[col].includes(orderBy) || orderBy === "id") ? ` ORDER BY ${q(orderBy)} ${dir === "desc" ? "DESC" : "ASC"}, id` : " ORDER BY id";
  const lim = Number.isFinite(limit) && limit > 0 ? ` LIMIT ${Math.min(limit, 10000)}` : "";
  return db.prepare(`${SELECT[col]}${o}${lim}`).all().map((r) => ({ id: r.id, data: toDoc(col, r) }));
}

// ---- change notices ------------------------------------------------------------------------------
const streams = new Set();
let pending = new Set(), notifyT = null;
function changed(col) {
  pending.add(col);
  if (notifyT) return;
  notifyT = setTimeout(() => {
    for (const c of pending) for (const res of streams) res.write(`event: change\ndata: ${JSON.stringify({ col: c })}\n\n`);
    pending = new Set();
    notifyT = null;
  }, 25);
}
setInterval(() => { for (const res of streams) res.write(": ping\n\n"); }, 25000).unref();

// ---- leases (the numbering counter's) ------------------------------------------------------------
const leases = new Map(); // name → {holder, expires}
function acquire(name, holder, ttlMs) {
  const now = Date.now(), l = leases.get(name);
  if (l && l.expires > now && l.holder !== holder) return { acquired: false, expiresAt: new Date(l.expires).toISOString() };
  const expires = now + Math.min(Math.max(ttlMs || 2000, 200), 60000);
  leases.set(name, { holder, expires });
  return { acquired: true, expiresAt: new Date(expires).toISOString() };
}

// ---- http ----------------------------------------------------------------------------------------
const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".ico": "image/x-icon", ".json": "application/json" };
const IMAGE = /^image\/(png|jpeg|gif|webp|avif|heic|heif)$/;
const MAX_BLOB = 20 * 1024 * 1024;

const send = (res, code, body, type = "application/json; charset=utf-8", headers = {}) => {
  res.writeHead(code, { "content-type": type, "cache-control": "no-store", ...headers });
  res.end(typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body));
};
const fail = (res, code, errCode, message) => send(res, code, { error: { code: errCode, message } });
const readBody = (req, max) => new Promise((resolve, reject) => {
  const chunks = []; let n = 0;
  req.on("data", (c) => { n += c.length; if (n > max) { reject({ code: "too_large" }); req.destroy(); } else chunks.push(c); });
  req.on("end", () => resolve(Buffer.concat(chunks)));
  req.on("error", reject);
});
const jsonBody = async (req) => { try { return JSON.parse((await readBody(req, 2e6)).toString("utf8") || "{}"); } catch { return null; } };
const validId = (s) => typeof s === "string" && /^[A-Za-z0-9_\-.~:@+]{1,200}$/.test(s) && s !== "." && s !== "..";
const newId = () => { const a = "abcdefghijklmnopqrstuvwxyz0123456789"; const b = crypto.randomBytes(20); return Array.from(b, (x) => a[x % a.length]).join(""); };

async function api(req, res, url) {
  const parts = url.pathname.split("/").slice(2).map(decodeURIComponent); // after /api
  const [what, col, id] = parts;

  if (what === "stream" && req.method === "GET") {
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store", connection: "keep-alive" });
    res.write("retry: 1000\n\n");
    streams.add(res);
    req.on("close", () => streams.delete(res));
    return;
  }
  if (what === "lease" && req.method === "POST") {
    const b = await jsonBody(req);
    if (!b || !validId(b.name) || typeof b.holder !== "string") return fail(res, 400, "invalid_argument", "bad lease");
    return send(res, 200, acquire(b.name, b.holder, Number(b.ttlMs)));
  }
  if (what === "blob" && req.method === "POST") {
    const type = String(req.headers["content-type"] || "").split(";")[0].trim().toLowerCase();
    if (!IMAGE.test(type)) return fail(res, 415, "unsupported_type", "not an image");
    let data;
    try { data = await readBody(req, MAX_BLOB); } catch { return fail(res, 413, "too_large", "over 20 MB"); }
    if (!data.length) return fail(res, 400, "invalid_argument", "empty");
    const sha = crypto.createHash("sha256").update(data).digest("hex");
    const bid = sha.slice(0, 32);
    stmt(`INSERT OR IGNORE INTO blobs (id, type, data, sha256, created) VALUES (?, ?, ?, ?, ?)`).run(bid, type, data, sha, new Date().toISOString());
    return send(res, 200, { id: bid, url: "/_blob/" + bid });
  }
  if (what !== "c" || !(col in COLS || col === "meta")) return fail(res, 404, "not_found", "no such route");

  if (!id) {
    if (req.method !== "GET") return fail(res, 405, "invalid_argument", "method not allowed");
    const docs = listDocs(col, { orderBy: url.searchParams.get("orderBy"), dir: url.searchParams.get("dir"), limit: Number(url.searchParams.get("limit")) });
    return send(res, 200, { docs });
  }
  if (!validId(id)) return fail(res, 400, "invalid_argument", "bad id");

  if (req.method === "GET") {
    const d = getDoc(col, id);
    return send(res, 200, { exists: d !== null, data: d });
  }
  if (req.method === "DELETE") {
    delDoc(col, id);
    changed(col);
    return send(res, 200, {});
  }
  if (req.method === "PUT" || req.method === "PATCH") {
    const b = await jsonBody(req);
    if (!b || typeof b !== "object" || Array.isArray(b)) return fail(res, 400, "invalid_argument", "body must be an object");
    db.exec("BEGIN IMMEDIATE");
    try {
      if (req.method === "PATCH") {
        const cur = getDoc(col, id);
        if (cur === null) { db.exec("ROLLBACK"); return fail(res, 404, "invalid_argument", "no such document"); }
        const merged = { ...cur };
        for (const [k, v] of Object.entries(b)) { if (v === null) delete merged[k]; else merged[k] = v; }
        putDoc(col, id, merged);
      } else putDoc(col, id, b);
      db.exec("COMMIT");
    } catch (e) {
      db.exec("ROLLBACK");
      return fail(res, 500, "internal", String(e.message));
    }
    changed(col);
    return send(res, 200, {});
  }
  return fail(res, 405, "invalid_argument", "method not allowed");
}

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://x");
    if (url.pathname.startsWith("/api/")) return await api(req, res, url);
    if (url.pathname.startsWith("/_blob/") && req.method === "GET") {
      const r = stmt(`SELECT type, data FROM blobs WHERE id = ?`).get(decodeURIComponent(url.pathname.slice(7)));
      if (!r) return send(res, 404, "not found", "text/plain");
      return send(res, 200, Buffer.from(r.data), r.type, { "cache-control": "public, max-age=31536000, immutable", "x-content-type-options": "nosniff" });
    }
    if (req.method !== "GET" && req.method !== "HEAD") return send(res, 405, "method not allowed", "text/plain");
    let p = url.pathname === "/" ? "/index.html" : url.pathname;
    const file = path.join(here, "public", path.normalize(p));
    if (!file.startsWith(path.join(here, "public") + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) return send(res, 404, "not found", "text/plain");
    return send(res, 200, fs.readFileSync(file), TYPES[path.extname(file)] || "application/octet-stream");
  } catch (e) {
    console.error(e);
    if (!res.headersSent) fail(res, 500, "internal", String(e?.message || e));
    else res.end();
  }
});
server.listen(PORT, HOST, () => console.log(`Task board on http://${HOST}:${PORT}  (data: ${DB_PATH})`));
