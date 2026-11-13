// The task board's API (README.md): items, events and meta in the board's SQLite file (store.mjs),
// every write checked against schema.mjs. A client sends content only: the server gives a new item
// its num, id, order and added time, a new event its id and time, and keeps the counter. A write that
// doesn't fit is refused with 400 and every reason, and nothing is written. A task's priority with a
// fraction is placed by priority.mjs, the page's rule too: rounded up, the open tasks it runs into
// moved down one. Archived items are read-only (unarchiving is the one change), nothing goes inside
// an archived folder, and no item that isn't archived is ever inside one.
import crypto from "node:crypto";
import {
  ITEM_FIELDS, ITEM_CREATE, EVENT_CREATE, ITEM_EDIT, EVENT_EDIT, RENAMED,
  itemErrors, eventErrors, metaErrors, sentErrors, errorText, normTime, TIME_FORMAT, validId, describe, renamedMeta,
} from "./schema.mjs";
import { COLS } from "./store.mjs";
import { refuse, send, fail, readBody, jsonBody } from "./http.mjs";
import { isOpen, placePriority } from "./priority.mjs";

const STATUS_LABEL = { "": "Queued", working: "Under way", backgrounded: "Backgrounded", integrating: "Integrating", review: "In review", waiting: "Waiting on you", done: "Done", abandoned: "Abandoned" };
const IMAGE = /^image\/(png|jpeg|gif|webp|avif|heic|heif)$/;
const MAX_BLOB = 20 * 1024 * 1024;
const UNDO_KEPT = 100; // deleted groups kept for a restore

const invalid = (errs) => refuse(400, "invalid_argument", errorText(errs), errs);
const refuseIf = (errs) => { if (errs.length) invalid(errs); };
const newId = () => { const a = "abcdefghijklmnopqrstuvwxyz0123456789"; return Array.from(crypto.randomBytes(20), (x) => a[x % a.length]).join(""); };
const nowIso = () => new Date().toISOString();
// An item as a message names it: #57 "Its title".
const named = (d) => { const t = String(d.text ?? "").trim(); return `#${d.num} ${JSON.stringify(t.length > 60 ? t.slice(0, 59) + "…" : t)}`; };
const nums = (ds) => ds.slice(0, 10).map((d) => `#${d.num}`).join(", ") + (ds.length > 10 ? ` and ${ds.length - 10} more` : "");

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
  // The now line's `updated` is the server's too, so it can only be a valid time.
  // (Fresh files get both documents.)
  if (!store.get("meta", "counter")) store.put("meta", "counter", { next: store.maxNum() + 1 });
  if (!store.get("meta", "now")) store.put("meta", "now", { text: "", updated: nowIso() });

  // What wrote a change, for its history event: the page, or the client's User-Agent.
  const byWho = (req) => {
    const ua = String(req.headers["user-agent"] || "unknown");
    return /Mozilla/.test(ua) ? "the page" : ua.split(/[ /]/)[0];
  };
  const logEvent = (item, kind, text) => store.put("events", crypto.randomUUID(), { item, t: nowIso(), kind, text });
  // Every change of a task's status leaves a history event, whoever writes it (the page, a script):
  // `kind: "status"`, the old and new statuses, and what wrote it.
  function logStatus(id, before, after, req) {
    const from = before?.status ?? "", to = after?.status ?? "";
    if (!after || after.kind !== "task" || (before && from === to) || (!before && to === "")) return;
    const by = byWho(req);
    logEvent(id, "status", before ? `Status ${STATUS_LABEL[from] ?? from} → ${STATUS_LABEL[to] ?? to} (by ${by})` : `Status ${STATUS_LABEL[to] ?? to} (by ${by})`);
  }

  // ---- archived ----------------------------------------------------------------------------------
  // The archived folders around an item whose folder is `parent`, nearest first: [{id, doc}].
  function archivedAround(parent) {
    const out = [];
    for (let p = parent, n = 0; p && n < 10000; n++) {
      const d = store.get("items", p);
      if (!d) break;
      if (d.archived) out.push({ id: p, doc: d });
      p = d.parent;
    }
    return out;
  }
  // Everything inside `id`, at any depth: [{id, doc}].
  function inside(id) {
    const out = [];
    const walk = (p) => { for (const k of store.childrenOf(p)) { out.push({ id: k, doc: store.get("items", k) }); walk(k); } };
    walk(id);
    return out;
  }
  // (Read-only: an archived item, or one inside an archived folder. Its history too.)
  function readOnlyWhy(id, doc) {
    if (doc.archived) return `${named(doc)} is archived`;
    const a = archivedAround(doc.parent)[0];
    return a ? `${named(doc)} is inside archived folder ${named(a.doc)}` : null;
  }

  // A task's priority sent with a fraction (`doc`, about to be written as `id`; `before`, what it
  // was): placed by priority.mjs, in the caller's transaction. The task's own number is set in `doc`;
  // each open task it runs into is written moved down one. The whole move leaves one `kind: "priority"`
  // event, on the task placed, saying how many moved and from which numbers. Returns {priority, moved: [{id, num, from, to}]}, or null for a whole number (stored as sent).
  function placeIfFraction(id, doc, before, req) {
    const x = doc.priority;
    if (doc.kind !== "task" || typeof x !== "number" || Number.isInteger(x)) return null;
    const items = new Map(store.list("items").map((d) => [d.id, d.data]));
    const inArchived = (d) => {
      for (let p = d.parent, n = 0; p && n < 10000; p = items.get(p)?.parent, n++) if (items.get(p)?.archived) return true;
      return false;
    };
    const others = [];
    for (const [oid, o] of items) if (oid !== id && o.kind === "task" && isOpen(o, inArchived(o))) others.push({ id: oid, priority: o.priority });
    const { to, moves } = placePriority(x, others);
    doc.priority = to;
    const by = byWho(req), old = before?.priority;
    const moved = moves.map((m) => {
      const o = items.get(m.id);
      store.put("items", m.id, { ...o, priority: m.to });
      return { id: m.id, num: o.num, from: m.from, to: m.to };
    });
    // (One event for the whole cascade, on the task placed. The ones it pushed get none: their number
    // shows, and this event names them.)
    const lo = moved.length ? Math.min(...moved.map((m) => m.from)) : 0, hi = moved.length ? Math.max(...moved.map((m) => m.from)) : 0;
    const cascade = moved.length ? `; ${moved.length} other${moved.length === 1 ? "" : "s"} moved down one, from priority ${lo === hi ? lo : `${lo}–${hi}`} (${nums(moved)})` : "";
    logEvent(id, "priority", `Priority ${typeof old === "number" && old !== to ? `${old} → ` : ""}${to}: asked for ${x}${cascade} (by ${by})`);
    return { priority: to, moved };
  }

  // ---- items -------------------------------------------------------------------------------------
  const normTimes = (spec, fields) => {
    const out = {};
    for (const [k, v] of Object.entries(fields)) out[k] = spec[k]?.type === "time" ? normTime(v) ?? v : v;
    return out;
  };
  const dropNulls = (o) => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== null));
  // `parent` must be "" or a folder that exists (only folders hold items; and, for a move, not the
  // item or inside it). Moved there (`into`: a new item, or a new parent), it must not be archived nor
  // inside an archived folder.
  function checkParent(id, parent, into) {
    if (parent === "") return;
    if (typeof parent !== "string") return; // (the schema says so)
    const p = store.get("items", parent);
    if (!p) invalid([{ field: "parent", message: `no item ${JSON.stringify(parent)} to be inside of` }]);
    for (let q = parent, n = 0; q && n < 10000; n++) {
      if (q === id) invalid([{ field: "parent", message: "an item can't be inside itself or what is inside it" }]);
      q = store.get("items", q)?.parent;
    }
    if (p.kind !== "folder") invalid([{ field: "parent", message: `${parent} is a task; only folders hold items` }]);
    const a = into ? archivedAround(parent)[0] : null;
    if (a) invalid([{ field: "parent", message: `${a.id === parent ? `folder ${named(p)} is archived` : `folder ${named(p)} is inside archived folder ${named(a.doc)}`}: nothing goes into an archived folder` }]);
  }
  function createItem(body, req) {
    refuseIf(sentErrors(body, ITEM_CREATE, ["id", "num", "order", "added"], RENAMED.itemFields));
    const given = dropNulls(body);
    const task = (given.kind ?? "task") === "task";
    const doc = { kind: "task", parent: "", ...(task ? { status: "", assigned: "Subagent" } : {}), ...given };
    return tx(() => {
      // (The server's fields are filled in before the check, so it names what the client got wrong.)
      doc.added = nowIso();
      doc.order = (store.maxOrder() ?? 0) + 1;
      doc.num = takeNum();
      refuseIf(itemErrors(doc));
      checkParent("", doc.parent, true);
      let id = `i${doc.num}`;
      while (store.get("items", id)) id = newId();
      const placed = placeIfFraction(id, doc, null, req);
      store.put("items", id, doc);
      logStatus(id, null, doc, req);
      changed("items", "events");
      return { id, ...ownFields(doc), ...placed };
    });
  }
  const ownFields = (d) => ({ num: d.num, order: d.order, added: d.added });

  // PATCH (merge: null removes a field) or PUT (replace what a client may write; the server's num,
  // added and, unless given, order stay) of an item that exists.
  //
  // Archiving and unarchiving (`archived`; `archivedAt`, when, is the server's time unless sent, and
  // goes with an unarchive), with `cascade: true` (sent, never stored), what the page sends once you
  // have confirmed it:
  // - archiving a folder holding items that aren't archived is refused, unless `cascade`: then they
  //   are archived with it, at the same time;
  // - an archived item, or one inside an archived folder, can't be changed: the one PATCH taken is
  //   `{"archived": false}` (and `"archivedAt": null`). Inside an archived folder that is refused too,
  //   unless `cascade`: then the archived folders around it are unarchived with it.
  // Each archive and unarchive leaves a `kind: "archive"` event.
  function updateItem(id, body, replace, req) {
    const cur = store.get("items", id);
    if (!cur) refuse(404, "not_found", `no item ${JSON.stringify(id)}`);
    refuseIf(sentErrors(body, [...ITEM_EDIT, ...(replace ? [] : ["cascade"])], ["id", "num", "added"], RENAMED.itemFields));
    const { cascade = false, ...fields } = body;
    if (typeof cascade !== "boolean") invalid([{ field: "cascade", message: "must be true or false" }]);
    if (cascade && !("archived" in fields)) invalid([{ field: "cascade", message: "goes with archived: it archives what is inside too, or unarchives the folders around" }]);
    const ro = readOnlyWhy(id, cur);
    // (Unarchiving: archived false, or null, which removes the field; its time may go with it.)
    if (ro && !(!replace && (fields.archived === false || fields.archived === null) && Object.entries(fields).every(([k, v]) => k === "archived" || (k === "archivedAt" && v === null)))) {
      invalid([{ field: "", message: `${ro}: it can't be changed (archived items are read-only); the one change taken is unarchiving it, PATCH {"archived": false}` }]);
    }
    return tx(() => {
      const sent = normTimes(ITEM_FIELDS, fields);
      const doc = replace
        ? { ...Object.fromEntries(Object.entries(cur).filter(([k]) => ITEM_FIELDS[k]?.server || (k === "order" && !("order" in sent)))), ...dropNulls(sent) }
        : { ...cur, ...sent };
      if (!replace) for (const k of Object.keys(doc)) if (doc[k] === null) delete doc[k];
      const archiving = doc.archived === true && !cur.archived, unarchiving = !!cur.archived && doc.archived !== true;
      if (archiving && doc.archivedAt === undefined) doc.archivedAt = nowIso();
      if (unarchiving) delete doc.archivedAt;
      refuseIf(itemErrors(doc));
      checkParent(id, doc.parent, doc.parent !== cur.parent);
      if (doc.kind !== "folder" && store.childrenOf(id).length) invalid([{ field: "kind", message: `${id} holds ${store.childrenOf(id).length} item(s); only folders hold items, so it stays a folder` }]);
      const by = byWho(req);
      if (doc.archived !== true) {
        const around = archivedAround(doc.parent);
        if (around.length && !cascade) invalid([{ field: "archived", message: `${named(doc)} is inside archived folder ${named(around[0].doc)}: unarchive that first, or send "cascade": true to unarchive the folders around it too` }]);
        for (const a of around) {
          const { archivedAt, ...rest } = a.doc;
          store.put("items", a.id, { ...rest, archived: false });
          logEvent(a.id, "archive", `Unarchived, with #${doc.num} inside (by ${by})`);
        }
      }
      if (archiving) {
        const open = inside(id).filter((d) => !d.doc.archived);
        if (open.length && !cascade) invalid([{ field: "archived", message: `${named(doc)} holds ${open.length} item(s) that aren't archived (${nums(open.map((d) => d.doc))}): archive those first` }]);
        for (const d of open) store.put("items", d.id, { ...d.doc, archived: true, archivedAt: doc.archivedAt });
        logEvent(id, "archive", `Archived${open.length ? `, with the ${open.length} inside` : ""} (by ${by})`);
      }
      if (unarchiving) logEvent(id, "archive", `Unarchived (by ${by})`);
      const placed = "priority" in sent ? placeIfFraction(id, doc, cur, req) : null;
      store.put("items", id, doc);
      logStatus(id, cur, doc, req);
      changed("items", "events");
      return { id, ...ownFields(doc), ...placed };
    });
  }

  // ---- events ------------------------------------------------------------------------------------
  // (An event's item must exist; an archived item's history is read-only, as the item is.)
  const checkItem = (item) => {
    if (item === "" || typeof item !== "string") return;
    const d = store.get("items", item);
    if (!d) invalid([{ field: "item", message: `no item ${JSON.stringify(item)}; use its id (a task's is i<number>), or "" for the board itself` }]);
    const ro = readOnlyWhy(item, d);
    if (ro) invalid([{ field: "item", message: `${ro}: its history is read-only (unarchive it first)` }]);
  };
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
      checkItem(cur.item);
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
    if (key !== "now") invalid(metaErrors(key, {}));
    refuseIf(sentErrors(body, ["text"], ["updated"], RENAMED.metaFields));
    return tx(() => {
      const doc = { ...(replace ? {} : store.get("meta", "now")), ...body, updated: nowIso() };
      refuseIf(metaErrors("now", doc));
      store.put("meta", "now", doc);
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
  // (Read-only, archived: an archived item, one inside an archived folder and their history can't be
  // deleted on their own; a folder that isn't archived goes with everything inside it.)
  function remove(col, id) {
    if (col === "meta") refuse(405, "invalid_argument", "meta documents can't be deleted");
    const ro = col === "items" ? (store.get("items", id) ? readOnlyWhy(id, store.get("items", id)) : null) : null;
    if (ro) invalid([{ field: "", message: `${ro}: it can't be deleted (archived items are read-only); unarchive it first, PATCH {"archived": false}` }]);
    if (col === "events") { const e = store.get("events", id); if (e) checkItem(e.item); }
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
          const a = d.doc.parent && !have.has(d.doc.parent) && !d.doc.archived ? archivedAround(d.doc.parent)[0] : null;
          if (a) refuse(409, "conflict", `its folder ${named(a.doc)} is archived now`);
        } else if (d.doc.item && !have.has(d.doc.item)) {
          const it = store.get("items", d.doc.item);
          if (!it) refuse(409, "conflict", `its item ${d.doc.item} is gone`);
          const ro = readOnlyWhy(d.doc.item, it);
          if (ro) refuse(409, "conflict", `${ro} now: its history is read-only`);
        }
      }
      for (const d of g.docs) { store.put(d.col, d.id, d.doc); trash.delete(`${d.col}/${d.id}`); }
      changed(...new Set(g.docs.map((d) => d.col)));
      return { restored: g.docs.map((d) => `${d.col}/${d.id}`) };
    });
  }

  // A list's query: `orderBy` a field of the collection, or "id" (an old name refused with its new
  // one), `dir` asc or desc, `limit` a whole number from 1; anything else is refused, not ignored.
  // Events also take `item` (one item's, "" the board's own) and, to read a history in pages,
  // `before` (a time) with `beforeId`: the events after that one in the order t newest first, then id
  // (the last one a page ended with); they need `orderBy=t&dir=desc`.
  function listQuery(col, params) {
    const errs = [];
    const allowed = ["orderBy", "dir", "limit", ...(col === "events" ? ["item", "before", "beforeId"] : [])];
    for (const k of params.keys()) if (!allowed.includes(k)) errs.push({ field: k, message: `isn't a query this takes (these are: ${allowed.join(", ")})` });
    const orderBy = params.get("orderBy"), dir = params.get("dir"), limit = params.get("limit");
    if (orderBy && col !== "meta") {
      if (col === "items" && RENAMED.itemFields[orderBy]) errs.push({ field: "orderBy", message: `${JSON.stringify(orderBy)} is renamed ${JSON.stringify(RENAMED.itemFields[orderBy])}` });
      else if (orderBy !== "id" && !COLS[col].includes(orderBy)) errs.push({ field: "orderBy", message: `${JSON.stringify(orderBy)} isn't a field of ${col} (these are: id, ${COLS[col].join(", ")})` });
    }
    if (dir !== null && dir !== "asc" && dir !== "desc") errs.push({ field: "dir", message: "must be asc or desc" });
    if (limit !== null && !/^[1-9]\d{0,8}$/.test(limit)) errs.push({ field: "limit", message: "must be a whole number from 1" });
    const item = params.get("item"), t = params.get("before"), beforeId = params.get("beforeId");
    if (item !== null && item !== "" && !validId(item)) errs.push({ field: "item", message: "isn't an item's id (or \"\" for the board itself)" });
    if (t !== null && normTime(t) !== t) errs.push({ field: "before", message: `must be a time, ${TIME_FORMAT}` });
    if ((t === null) !== (beforeId === null)) errs.push({ field: t === null ? "before" : "beforeId", message: "go together: the time and the id of the event a page ended with" });
    if (t !== null && !(orderBy === "t" && dir === "desc")) errs.push({ field: "before", message: "needs orderBy=t&dir=desc: it is the place in that order a page ended" });
    refuseIf(errs);
    return { orderBy, dir, limit: Number(limit), ...(item !== null ? { item } : {}), ...(t !== null ? { before: [t, beforeId] } : {}) };
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
    if (what === "history" && req.method === "GET") return send(res, 200, store.history());
    if (what !== "c" || !["items", "events", "meta"].includes(col)) return fail(res, 404, "not_found", "no such route");
    const m = req.method;

    if (!id) {
      // (`total`: how many there are, so a reader that asked for fewer can say so.)
      if (m === "GET") { const q = listQuery(col, url.searchParams); return send(res, 200, { docs: store.list(col, q), total: store.count(col, q.item) }); }
      if (m === "POST" && col !== "meta") return send(res, 200, (col === "items" ? createItem : createEvent)(await jsonBody(req), req));
      return fail(res, 405, "invalid_argument", col === "meta" ? "meta documents are changed by PATCH meta/now" : "method not allowed: POST creates, PATCH/DELETE need an id");
    }
    if (!validId(id)) return fail(res, 400, "invalid_argument", "bad id");
    if (col === "meta" && RENAMED.meta[id]) return fail(res, 400, "invalid_argument", renamedMeta(id));
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
