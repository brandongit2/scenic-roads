// The Scenic Roads task board: a standalone web app. One Node process (no dependencies; Node 22.5+
// for node:sqlite) serves the page in public/ and keeps its data in a SQLite file.
//
//   node server.mjs [--port 8077] [--host 127.0.0.1] [--db taskboard.db]
//
// The page is the Claude artifact's, unchanged in spirit; public/shim.js gives it the document-database
// API it was written against (collection/doc/get/add/update/delete/restore/onSnapshot, and photo
// uploads) over this server's HTTP API: a change is written here, and every open tab is told through
// an event stream to read the collection again.
//
// The server owns the data's shape (schema.mjs: every collection, on every write; README.md has the
// API). A client sends content only: the server gives a new item its num, id, order and added time,
// a new event its id and time, and keeps the counter. A write that doesn't fit is refused with 400 and
// every reason, and nothing is written.
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath, pathToFileURL } from "node:url";
import { openStore } from "./store.mjs";
import {
  ITEM_FIELDS, ITEM_CREATE, EVENT_CREATE, ITEM_EDIT, EVENT_EDIT,
  itemErrors, eventErrors, metaErrors, sentErrors, errorText, normTime, validId, describe,
} from "./schema.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const STAGE_LABEL = { "": "Queued", working: "Under way", backgrounded: "Backgrounded", integrating: "Integrating", review: "In review", waiting: "Waiting on you", done: "Done", abandoned: "Abandoned" };
const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript; charset=utf-8", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".ico": "image/x-icon", ".json": "application/json" };
const IMAGE = /^image\/(png|jpeg|gif|webp|avif|heic|heif)$/;
const MAX_BLOB = 20 * 1024 * 1024;
const UNDO_KEPT = 100; // deleted groups kept for a restore

// A request the server refuses: the HTTP status, a code for the shim, why (and each field's reason).
class Refusal extends Error {
  constructor(status, code, message, errors) { super(message); Object.assign(this, { status, code, errors }); }
}
const refuse = (status, code, message, errors) => { throw new Refusal(status, code, message, errors); };
const invalid = (errs) => refuse(400, "invalid_argument", errorText(errs), errs);
const refuseIf = (errs) => { if (errs.length) invalid(errs); };
const newId = () => { const a = "abcdefghijklmnopqrstuvwxyz0123456789"; return Array.from(crypto.randomBytes(20), (x) => a[x % a.length]).join(""); };
const nowIso = () => new Date().toISOString();

export function makeServer(store) {
  const { db } = store;

  // ---- change notices ----------------------------------------------------------------------------
  const streams = new Set();
  let pending = new Set(), notifyT = null;
  const changed = (...cols) => {
    for (const c of cols) pending.add(c);
    if (notifyT) return;
    notifyT = setTimeout(() => {
      for (const c of pending) for (const res of streams) res.write(`event: change\ndata: ${JSON.stringify({ col: c })}\n\n`);
      pending = new Set();
      notifyT = null;
    }, 25);
  };
  const ping = setInterval(() => { for (const res of streams) res.write(": ping\n\n"); }, 25000);
  ping.unref();

  // One write, all or nothing.
  function tx(fn) {
    db.exec("BEGIN IMMEDIATE");
    try { const r = fn(); db.exec("COMMIT"); return r; }
    catch (e) { db.exec("ROLLBACK"); throw e; }
  }

  // ---- numbers -----------------------------------------------------------------------------------
  // meta/counter is the server's: the larger of its `next` and one past the highest number any item
  // has. A deleted task's number stays retired.
  function takeNum() {
    const n = Math.max(store.get("meta", "counter")?.next || 1, store.maxNum() + 1);
    store.put("meta", "counter", { next: n + 1 });
    return n;
  }
  // The status line's `updated` is the server's too, so it can only be a valid time.
  // (Fresh files get both documents.)
  if (!store.get("meta", "counter")) store.put("meta", "counter", { next: store.maxNum() + 1 });
  if (!store.get("meta", "status")) store.put("meta", "status", { now: "", updated: nowIso() });

  // Every change of a task's stage leaves a history event, whoever writes it (the page, a script):
  // `kind: "stage"`, the old and new stages, and what wrote it (the page, or the client's User-Agent).
  function logStage(id, before, after, req) {
    const from = before?.stage ?? "", to = after?.stage ?? "";
    if (!after || after.kind !== "task" || (before && from === to) || (!before && to === "")) return;
    const ua = String(req.headers["user-agent"] || "unknown");
    const by = /Mozilla/.test(ua) ? "the page" : ua.split(/[ /]/)[0];
    const text = before ? `Stage ${STAGE_LABEL[from] ?? from} → ${STAGE_LABEL[to] ?? to} (by ${by})` : `Stage ${STAGE_LABEL[to] ?? to} (by ${by})`;
    store.put("events", crypto.randomUUID(), { item: id, t: nowIso(), kind: "stage", text });
  }

  // ---- items -------------------------------------------------------------------------------------
  const normTimes = (spec, fields) => {
    const out = {};
    for (const [k, v] of Object.entries(fields)) out[k] = spec[k]?.type === "time" ? normTime(v) ?? v : v;
    return out;
  };
  const dropNulls = (o) => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== null));
  // `parent` must be "" or an item that exists (and, for a move, not the item or inside it).
  function checkParent(id, parent) {
    if (parent === "") return;
    if (typeof parent !== "string") return; // (the schema says so)
    if (!store.get("items", parent)) invalid([{ field: "parent", message: `no item ${JSON.stringify(parent)} to be inside of` }]);
    for (let p = parent, n = 0; p && n < 10000; n++) {
      if (p === id) invalid([{ field: "parent", message: "an item can't be inside itself or what is inside it" }]);
      p = store.get("items", p)?.parent;
    }
  }
  function createItem(body, req) {
    refuseIf(sentErrors(body, ITEM_CREATE, ["id", "num", "order", "added"]));
    const given = dropNulls(body);
    const task = (given.kind ?? "task") === "task";
    const doc = { kind: "task", parent: "", ...(task ? { stage: "", where: "Subagent" } : {}), ...given };
    return tx(() => {
      // (The server's fields are filled in before the check, so it names what the client got wrong.)
      doc.added = nowIso();
      doc.order = (store.maxOrder(doc.parent) ?? 0) + 1;
      if (task) doc.num = takeNum();
      refuseIf(itemErrors(doc));
      checkParent("", doc.parent);
      let id = task ? `i${doc.num}` : newId();
      while (store.get("items", id)) id = newId();
      store.put("items", id, doc);
      logStage(id, null, doc, req);
      changed("items", "events");
      return { id, ...ownFields(doc) };
    });
  }
  const ownFields = (d) => ({ ...(d.num !== undefined ? { num: d.num } : {}), order: d.order, added: d.added });

  // PATCH (merge: null removes a field) or PUT (replace what a client may write; the server's num,
  // added and, unless given, order stay) of an item that exists.
  function updateItem(id, body, replace, req) {
    const cur = store.get("items", id);
    if (!cur) refuse(404, "not_found", `no item ${JSON.stringify(id)}`);
    refuseIf(sentErrors(body, ITEM_EDIT, ["id", "num", "added"]));
    return tx(() => {
      const sent = normTimes(ITEM_FIELDS, body);
      const doc = replace
        ? { ...Object.fromEntries(Object.entries(cur).filter(([k]) => ITEM_FIELDS[k]?.server || (k === "order" && !("order" in sent)))), ...dropNulls(sent) }
        : { ...cur, ...sent };
      if (!replace) for (const k of Object.keys(doc)) if (doc[k] === null) delete doc[k];
      if (doc.kind === "task" && doc.num === undefined) doc.num = takeNum(); // (a folder made a task)
      refuseIf(itemErrors(doc));
      checkParent(id, doc.parent);
      store.put("items", id, doc);
      logStage(id, cur, doc, req);
      changed("items", "events");
      return { id, ...ownFields(doc) };
    });
  }

  // ---- events ------------------------------------------------------------------------------------
  const checkItem = (item) => { if (item !== "" && typeof item === "string" && !store.get("items", item)) invalid([{ field: "item", message: `no item ${JSON.stringify(item)}; use its id (a task's is i<number>), or "" for the board itself` }]); };
  function createEvent(body) {
    refuseIf(sentErrors(body, EVENT_CREATE, ["id", "t"]));
    const doc = { ...dropNulls(body), t: nowIso() };
    refuseIf(eventErrors(doc));
    return tx(() => {
      checkItem(doc.item);
      const id = crypto.randomUUID();
      store.put("events", id, doc);
      changed("events");
      return { id, t: doc.t };
    });
  }
  function updateEvent(id, body, replace) {
    const cur = store.get("events", id);
    if (!cur) refuse(404, "not_found", `no event ${JSON.stringify(id)}`);
    refuseIf(sentErrors(body, EVENT_EDIT, ["id", "item", "t"]));
    return tx(() => {
      const doc = replace ? { item: cur.item, t: cur.t, ...dropNulls(body) } : { ...cur, ...body };
      for (const k of Object.keys(doc)) if (doc[k] === null) delete doc[k];
      refuseIf(eventErrors(doc));
      store.put("events", id, doc);
      changed("events");
      return { id, t: doc.t };
    });
  }

  // ---- meta --------------------------------------------------------------------------------------
  function updateMeta(key, body, replace) {
    if (key === "counter") refuse(403, "forbidden", "meta/counter is the server's: a new task's number comes from POST /api/c/items");
    if (key !== "status") invalid(metaErrors(key, {}));
    refuseIf(sentErrors(body, ["now"], ["updated"]));
    return tx(() => {
      const doc = { ...(replace ? {} : store.get("meta", "status")), ...body, updated: nowIso() };
      refuseIf(metaErrors("status", doc));
      store.put("meta", "status", doc);
      changed("meta");
      return { updated: doc.updated };
    });
  }

  // ---- delete, and restore -----------------------------------------------------------------------
  // Deleting an item deletes what is inside it and every event about any of them (a reference never
  // points at nothing). What goes is kept in memory, the last UNDO_KEPT deletions, for the page's
  // Undo: POST <col>/<id>/restore puts the group back, as it was (a restart forgets them).
  const trash = new Map(); // "col/id" → {docs: [{col, id, doc}]}
  const keep = (docs) => {
    const g = { docs };
    for (const d of docs) trash.set(`${d.col}/${d.id}`, g);
    while (new Set(trash.values()).size > UNDO_KEPT) for (const [k, v] of trash) { for (const d of v.docs) trash.delete(`${d.col}/${d.id}`); break; }
  };
  function remove(col, id) {
    if (col === "meta") refuse(405, "invalid_argument", "meta documents can't be deleted");
    return tx(() => {
      const docs = [];
      const take = (c, i) => { const doc = store.get(c, i); if (doc) docs.push({ col: c, id: i, doc }); };
      if (col === "events") take("events", id);
      else {
        const walk = (i) => { take("items", i); for (const k of store.childrenOf(i)) walk(k); };
        if (store.get("items", id)) walk(id);
        for (const d of [...docs]) for (const e of store.eventsOf(d.id)) take("events", e);
      }
      for (const d of docs) store.del(d.col, d.id);
      if (docs.length) { keep(docs); changed(...new Set(docs.map((d) => d.col))); }
      return { deleted: docs.map((d) => `${d.col}/${d.id}`) };
    });
  }
  function restore(col, id) {
    if (store.get(col, id)) return { restored: [] }; // (back already: a group's other members ask too)
    const g = trash.get(`${col}/${id}`);
    if (!g) refuse(404, "not_found", `nothing deleted to restore as ${col}/${id} (it may be older than the last ${UNDO_KEPT} deletions, or the server restarted)`);
    return tx(() => {
      const have = new Set(g.docs.filter((d) => d.col === "items").map((d) => d.id));
      for (const d of g.docs) {
        if (d.col === "items") {
          if (d.doc.num !== undefined && store.numOwner(d.doc.num)) refuse(409, "conflict", `#${d.doc.num} is another task's now`);
          if (d.doc.parent && !have.has(d.doc.parent) && !store.get("items", d.doc.parent)) refuse(409, "conflict", `its folder ${d.doc.parent} is gone`);
          if (store.get("items", d.id)) refuse(409, "conflict", `${d.id} exists again`);
        } else if (d.doc.item && !have.has(d.doc.item) && !store.get("items", d.doc.item)) refuse(409, "conflict", `its item ${d.doc.item} is gone`);
      }
      for (const d of g.docs) { store.put(d.col, d.id, d.doc); trash.delete(`${d.col}/${d.id}`); }
      changed(...new Set(g.docs.map((d) => d.col)));
      return { restored: g.docs.map((d) => `${d.col}/${d.id}`) };
    });
  }

  // ---- http --------------------------------------------------------------------------------------
  const send = (res, code, body, type = "application/json; charset=utf-8", headers = {}) => {
    res.writeHead(code, { "content-type": type, "cache-control": "no-store", ...headers });
    res.end(typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body));
  };
  const fail = (res, code, errCode, message, errors) => send(res, code, { error: { code: errCode, message, ...(errors ? { errors } : {}) } });
  const readBody = (req, max) => new Promise((resolve, reject) => {
    const chunks = []; let n = 0;
    req.on("data", (c) => { n += c.length; if (n > max) { reject({ code: "too_large" }); req.destroy(); } else chunks.push(c); });
    req.on("end", () => resolve(Buffer.concat(chunks)));
    req.on("error", reject);
  });
  const jsonBody = async (req) => {
    const text = (await readBody(req, 2e6)).toString("utf8");
    try { return JSON.parse(text || "{}"); } catch { return refuse(400, "invalid_argument", "the body is not valid JSON"); }
  };

  async function api(req, res, url) {
    const [what, col, id, action] = url.pathname.split("/").slice(2).map(decodeURIComponent); // after /api
    if (what === "stream" && req.method === "GET") {
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store", connection: "keep-alive" });
      res.write("retry: 1000\n\n");
      streams.add(res);
      req.on("close", () => streams.delete(res));
      return;
    }
    if (what === "schema" && req.method === "GET") return send(res, 200, describe());
    if (what === "blob" && req.method === "POST") {
      const type = String(req.headers["content-type"] || "").split(";")[0].trim().toLowerCase();
      if (!IMAGE.test(type)) return fail(res, 415, "unsupported_type", "not an image");
      let data;
      try { data = await readBody(req, MAX_BLOB); } catch { return fail(res, 413, "too_large", "over 20 MB"); }
      if (!data.length) return fail(res, 400, "invalid_argument", "empty");
      const sha = crypto.createHash("sha256").update(data).digest("hex");
      const bid = sha.slice(0, 32);
      store.stmt(`INSERT OR IGNORE INTO blobs (id, type, data, sha256, created) VALUES (?, ?, ?, ?, ?)`).run(bid, type, data, sha, nowIso());
      return send(res, 200, { id: bid, url: "_blob/" + bid });
    }
    if (what !== "c" || !["items", "events", "meta"].includes(col)) return fail(res, 404, "not_found", "no such route");
    const m = req.method;

    if (!id) {
      // (`total`: how many there are, so a reader that asked for fewer can say so.)
      if (m === "GET") return send(res, 200, { docs: store.list(col, { orderBy: url.searchParams.get("orderBy"), dir: url.searchParams.get("dir"), limit: Number(url.searchParams.get("limit")) }), total: store.count(col) });
      if (m === "POST" && col !== "meta") return send(res, 200, (col === "items" ? createItem : createEvent)(await jsonBody(req), req));
      return fail(res, 405, "invalid_argument", col === "meta" ? "meta documents are changed by PATCH meta/status" : "method not allowed: POST creates, PATCH/DELETE need an id");
    }
    if (!validId(id)) return fail(res, 400, "invalid_argument", "bad id");
    if (action === "restore" && m === "POST" && col !== "meta") return send(res, 200, restore(col, id));
    if (action) return fail(res, 404, "not_found", "no such route");
    if (m === "GET") { const d = store.get(col, id); return send(res, 200, { exists: d !== null, data: d }); }
    if (m === "DELETE") return send(res, 200, remove(col, id));
    if (m === "PUT" || m === "PATCH") {
      const body = await jsonBody(req);
      const replace = m === "PUT";
      if (col === "meta") return send(res, 200, updateMeta(id, body, replace));
      if (!store.get(col, id)) refuse(404, "not_found", `no ${col === "items" ? "item" : "event"} ${JSON.stringify(id)}; POST /api/c/${col} creates one (the server picks the id)`);
      return send(res, 200, col === "items" ? updateItem(id, body, replace, req) : updateEvent(id, body, replace));
    }
    return fail(res, 405, "invalid_argument", "method not allowed");
  }

  return http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, "http://x");
      if (url.pathname.startsWith("/api/")) return await api(req, res, url);
      if (url.pathname.startsWith("/_blob/") && req.method === "GET") {
        const r = store.stmt(`SELECT type, data FROM blobs WHERE id = ?`).get(decodeURIComponent(url.pathname.slice(7)));
        if (!r) return send(res, 404, "not found", "text/plain");
        return send(res, 200, Buffer.from(r.data), r.type, { "cache-control": "public, max-age=31536000, immutable", "x-content-type-options": "nosniff" });
      }
      if (req.method !== "GET" && req.method !== "HEAD") return send(res, 405, "method not allowed", "text/plain");
      const p = url.pathname === "/" ? "/index.html" : url.pathname;
      // (schema.mjs is the page's too: it checks what it shows by the same rules.)
      const file = p === "/schema.mjs" ? path.join(here, "schema.mjs") : path.join(here, "public", path.normalize(p));
      if ((p !== "/schema.mjs" && !file.startsWith(path.join(here, "public") + path.sep)) || !fs.existsSync(file) || !fs.statSync(file).isFile()) return send(res, 404, "not found", "text/plain");
      return send(res, 200, fs.readFileSync(file), TYPES[path.extname(file)] || "application/octet-stream");
    } catch (e) {
      if (e instanceof Refusal) return fail(res, e.status, e.code, e.message, e.errors);
      if (e?.code === "too_large") return fail(res, 413, "too_large", "the body is over 2 MB");
      console.error(e);
      if (!res.headersSent) fail(res, 500, "internal", String(e?.message || e));
      else res.end();
    }
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const arg = (name, d) => { const i = process.argv.indexOf("--" + name); return i > 0 ? process.argv[i + 1] : d; };
  const PORT = Number(arg("port", process.env.PORT || 8077));
  const HOST = arg("host", process.env.HOST || "127.0.0.1");
  const DB_PATH = path.resolve(arg("db", process.env.TASKBOARD_DB || path.join(here, "taskboard.db")));
  const server = makeServer(openStore(DB_PATH));
  server.listen(PORT, HOST, () => console.log(`Task board on http://${HOST}:${PORT}  (data: ${DB_PATH})`));
}
