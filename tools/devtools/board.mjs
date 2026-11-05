// The task board's API (README.md): items, events and meta in the board's SQLite file (store.mjs),
// every write checked against schema.mjs. A client sends content only: the server gives a new item
// its num, id, order and added time, a new event its id and time, and keeps the counter. A write that
// doesn't fit is refused with 400 and every reason, and nothing is written.
import crypto from "node:crypto";
import {
  ITEM_FIELDS, ITEM_CREATE, EVENT_CREATE, ITEM_EDIT, EVENT_EDIT,
  itemErrors, eventErrors, metaErrors, sentErrors, errorText, normTime, validId, describe,
} from "./schema.mjs";
import { refuse, send, fail, readBody, jsonBody } from "./http.mjs";

const STAGE_LABEL = { "": "Queued", working: "Under way", backgrounded: "Backgrounded", integrating: "Integrating", review: "In review", waiting: "Waiting on you", done: "Done", abandoned: "Abandoned" };
const IMAGE = /^image\/(png|jpeg|gif|webp|avif|heic|heif)$/;
const MAX_BLOB = 20 * 1024 * 1024;
const UNDO_KEPT = 100; // deleted groups kept for a restore

const invalid = (errs) => refuse(400, "invalid_argument", errorText(errs), errs);
const refuseIf = (errs) => { if (errs.length) invalid(errs); };
const newId = () => { const a = "abcdefghijklmnopqrstuvwxyz0123456789"; return Array.from(crypto.randomBytes(20), (x) => a[x % a.length]).join(""); };
const nowIso = () => new Date().toISOString();

// `changed(...cols)`: tells the open pages which collections changed (server.mjs's event stream).
// Returns {api(req, res, url, parts): a call under /api (parts: the path after it), blob(res, id)}.
export function makeBoard(store, changed) {
  const { db } = store;

  // One write, all or nothing.
  function tx(fn) {
    db.exec("BEGIN IMMEDIATE");
    try { const r = fn(); db.exec("COMMIT"); return r; }
    catch (e) { db.exec("ROLLBACK"); throw e; }
  }

  // ---- numbers -----------------------------------------------------------------------------------
  // meta/counter is the server's: the larger of its `next` and one past the highest number any item
  // has. Tasks and folders take their numbers from it alike; a deleted item's number stays retired.
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
  // `parent` must be "" or a folder that exists (only folders hold items; and, for a move, not the
  // item or inside it).
  function checkParent(id, parent) {
    if (parent === "") return;
    if (typeof parent !== "string") return; // (the schema says so)
    const p = store.get("items", parent);
    if (!p) invalid([{ field: "parent", message: `no item ${JSON.stringify(parent)} to be inside of` }]);
    for (let q = parent, n = 0; q && n < 10000; n++) {
      if (q === id) invalid([{ field: "parent", message: "an item can't be inside itself or what is inside it" }]);
      q = store.get("items", q)?.parent;
    }
    if (p.kind !== "folder") invalid([{ field: "parent", message: `${parent} is a task; only folders hold items` }]);
  }
  function createItem(body, req) {
    refuseIf(sentErrors(body, ITEM_CREATE, ["id", "num", "order", "added"]));
    const given = dropNulls(body);
    const task = (given.kind ?? "task") === "task";
    const doc = { kind: "task", parent: "", ...(task ? { stage: "", where: "Subagent" } : {}), ...given };
    return tx(() => {
      // (The server's fields are filled in before the check, so it names what the client got wrong.)
      doc.added = nowIso();
      doc.order = (store.maxOrder() ?? 0) + 1;
      doc.num = takeNum();
      refuseIf(itemErrors(doc));
      checkParent("", doc.parent);
      let id = `i${doc.num}`;
      while (store.get("items", id)) id = newId();
      store.put("items", id, doc);
      logStage(id, null, doc, req);
      changed("items", "events");
      return { id, ...ownFields(doc) };
    });
  }
  const ownFields = (d) => ({ num: d.num, order: d.order, added: d.added });

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
      refuseIf(itemErrors(doc));
      checkParent(id, doc.parent);
      if (doc.kind !== "folder" && store.childrenOf(id).length) invalid([{ field: "kind", message: `${id} holds ${store.childrenOf(id).length} item(s); only folders hold items, so it stays a folder` }]);
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
          if (d.doc.num !== undefined && store.numOwner(d.doc.num)) refuse(409, "conflict", `#${d.doc.num} is another item's now`);
          if (d.doc.parent && !have.has(d.doc.parent) && !store.get("items", d.doc.parent)) refuse(409, "conflict", `its folder ${d.doc.parent} is gone`);
          if (store.get("items", d.id)) refuse(409, "conflict", `${d.id} exists again`);
        } else if (d.doc.item && !have.has(d.doc.item) && !store.get("items", d.doc.item)) refuse(409, "conflict", `its item ${d.doc.item} is gone`);
      }
      for (const d of g.docs) { store.put(d.col, d.id, d.doc); trash.delete(`${d.col}/${d.id}`); }
      changed(...new Set(g.docs.map((d) => d.col)));
      return { restored: g.docs.map((d) => `${d.col}/${d.id}`) };
    });
  }

  async function api(req, res, url, parts) {
    const [what, col, id, action] = parts; // after /api
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
      return send(res, 200, { id: bid });
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

  function blob(res, id) {
    const r = store.stmt(`SELECT type, data FROM blobs WHERE id = ?`).get(id);
    if (!r) return send(res, 404, "not found", "text/plain");
    return send(res, 200, Buffer.from(r.data), r.type, { "cache-control": "public, max-age=31536000, immutable", "x-content-type-options": "nosniff" });
  }
  return { api, blob };
}
