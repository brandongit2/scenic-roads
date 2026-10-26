// What the task board's data may look like: one schema per collection (items, events, meta),
// enforced by server.mjs on every create and update, and by repair.mjs on the data already there.
// Pure functions, no I/O. A document that doesn't fit gets a list of {field, message}: every reason
// at once, in words the caller can act on.

// The fixed lists are the values in use on 26 Oct 2026, kept in one place; the page reads them
// from GET /api/schema. To allow a new one, add it here.
export const STAGES = ["", "working", "backgrounded", "integrating", "review", "waiting", "done", "abandoned"]; // "" = queued
export const ITEM_KINDS = ["task", "folder"];
export const WHERES = [
  "", "Subagent", "Subagent (Opus)", "Subagent (Sonnet)", "Subagent (Fable)", "Fable subagent (high effort)", "Sonnet subagent",
  "worktree agent (Opus)", "worktree agent on the M1 (Opus)", "agent, this Mac", "PM (M4)", "Claude", "You", "Pipeline (lead)",
  "Build Mac process (dem/bldfetch.py, started by hand)",
];
export const EVENT_KINDS = ["note", "owner", "stage", "added", "start", "delegate", "progress", "review", "merge", "publish", "done", "wait", "stop", "bug", "decision"];

// Times are one format everywhere: ISO 8601, UTC, to the millisecond (what Date#toISOString gives),
// which also sorts as text. Accepted on the way in: the same with or without the fraction
// ("2026-10-26T08:35:48Z"); stored as the full form. Epoch numbers, offsets and dates alone are not.
const TIME_IN = /^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)(?:\.(\d{1,3}))?Z$/;
export const TIME_FORMAT = "YYYY-MM-DDTHH:MM:SS.sssZ in UTC (e.g. 2026-10-26T08:35:48.000Z; new Date().toISOString())";
export function normTime(v) {
  const m = typeof v === "string" && TIME_IN.exec(v);
  if (!m) return null;
  const t = new Date(`${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:${m[6]}.${(m[7] || "").padEnd(3, "0")}Z`);
  return Number.isNaN(t.getTime()) || t.toISOString().slice(0, 10) !== `${m[1]}-${m[2]}-${m[3]}` ? null : t.toISOString();
}
export const isCanonicalTime = (v) => normTime(v) === v;

const ID = /^[A-Za-z0-9_\-.~:@+]{1,200}$/;
export const validId = (s) => typeof s === "string" && ID.test(s) && s !== "." && s !== "..";

// A field: type, and for a few, limits. `server`: only the server writes it. `task`: tasks only.
const F = {
  string: (max) => ({ type: "string", max }),
  int: (min) => ({ type: "int", min }),
};
export const ITEM_FIELDS = {
  kind: { type: "enum", values: ITEM_KINDS },
  text: F.string(2000),
  parent: { type: "string", max: 200 }, // "" = top level; else an item's id (checked against the items)
  order: { type: "number" }, // its place among all items: siblings in the tree, and ties in a sort; changed by a drag
  num: { ...F.int(1), server: true }, // every item's number (#57), tasks and folders from one counter
  added: { type: "time", server: true },
  note: F.string(100000),
  archived: { type: "bool" },
  archivedAt: { type: "time" },
  photos: { type: "photos" }, // blob ids (POST /api/blob)
  stage: { type: "enum", values: STAGES, task: true },
  where: { type: "enum", values: WHERES, task: true },
  priority: { type: "number", min: 0, task: true },
  discuss: { type: "bool", task: true },
  started: { type: "time", task: true },
  finished: { type: "time", task: true },
  reply: { ...F.string(100000), task: true },
};
export const EVENT_FIELDS = {
  item: { type: "string", max: 200 }, // "" = the board itself; else an item's id
  kind: { type: "enum", values: EVENT_KINDS },
  text: F.string(100000),
  t: { type: "time", server: true },
};
export const META_FIELDS = {
  status: { now: F.string(5000), updated: { type: "time", server: true } },
  counter: { next: { ...F.int(1), server: true } },
};
// What a client may send when it creates one (the server fills the rest: ids, num, order, added, t,
// and the defaults).
export const ITEM_CREATE = ["kind", "text", "parent", "note", "where", "priority", "stage", "discuss"];
export const EVENT_CREATE = ["item", "kind", "text"];
// What a client may change in one that exists (null removes an optional field).
export const ITEM_EDIT = Object.keys(ITEM_FIELDS).filter((k) => !ITEM_FIELDS[k].server);
export const EVENT_EDIT = ["kind", "text"];

const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const list = (a) => a.map((x) => JSON.stringify(x)).join(", ");

// Why `v` isn't a valid value of field spec `s`, or null. Times are checked in their canonical form
// (writes normalise first).
function badValue(s, v) {
  switch (s.type) {
    case "string":
      if (typeof v !== "string") return "must be a string";
      if (s.max && v.length > s.max) return `is too long (${v.length} characters; at most ${s.max})`;
      return null;
    case "enum": return s.values.includes(v) ? null : `must be one of ${list(s.values)}`;
    case "int": return Number.isInteger(v) && v >= (s.min ?? -Infinity) ? null : `must be a whole number from ${s.min}`;
    case "number": return typeof v === "number" && Number.isFinite(v) && v >= (s.min ?? -Infinity) ? null : `must be a number${s.min !== undefined ? ` from ${s.min}` : ""}`;
    case "bool": return typeof v === "boolean" ? null : "must be true or false";
    case "time": return isCanonicalTime(v) ? null : `must be a time as ${TIME_FORMAT}`;
    case "photos": return Array.isArray(v) && v.length <= 50 && v.every((x) => typeof x === "string" && /^[0-9a-f]{32}$/.test(x)) ? null : "must be a list of up to 50 photo ids (32 hex digits, from POST /api/blob)";
    default: return "has an unknown type";
  }
}

// Every way `doc` (a whole item) breaks the item schema.
export function itemErrors(doc) {
  const errs = [];
  const bad = (field, message) => errs.push({ field, message });
  if (!isObj(doc)) return [{ field: "", message: "must be an object" }];
  const task = doc.kind === "task";
  for (const k of Object.keys(doc)) {
    const s = ITEM_FIELDS[k];
    if (!s) bad(k, "is not a field of an item");
    else if (s.task && doc.kind === "folder") bad(k, "is only for tasks (a folder has none)");
    else { const w = badValue(s, doc[k]); if (w) bad(k, w); }
  }
  for (const k of ["kind", "text", "parent", "order", "num", "added"]) if (doc[k] === undefined) bad(k, "is required");
  if (task) for (const k of ["stage", "where"]) if (doc[k] === undefined) bad(k, "is required for a task");
  if (typeof doc.text === "string" && !doc.text.trim()) bad("text", "must not be empty");
  if (doc.archivedAt !== undefined && doc.archived !== true) bad("archivedAt", "is only for an archived item (archived: true)");
  return errs;
}
// Every way `doc` (a whole event) breaks the event schema.
export function eventErrors(doc) {
  const errs = [];
  if (!isObj(doc)) return [{ field: "", message: "must be an object" }];
  for (const k of Object.keys(doc)) {
    const s = EVENT_FIELDS[k];
    const w = s ? badValue(s, doc[k]) : "is not a field of an event";
    if (w) errs.push({ field: k, message: w });
  }
  for (const k of ["item", "kind", "text", "t"]) if (doc[k] === undefined) errs.push({ field: k, message: "is required" });
  if (typeof doc.text === "string" && !doc.text.trim()) errs.push({ field: "text", message: "must not be empty" });
  return errs;
}
// Every way `doc` breaks meta/<key>'s schema (an unknown key is no document at all).
export function metaErrors(key, doc) {
  const fields = META_FIELDS[key];
  if (!fields) return [{ field: "", message: `there is no meta/${key}; the documents are ${list(Object.keys(META_FIELDS))}` }];
  if (!isObj(doc)) return [{ field: "", message: "must be an object" }];
  const errs = [];
  for (const k of Object.keys(doc)) {
    const w = fields[k] ? badValue(fields[k], doc[k]) : "is not a field of this document";
    if (w) errs.push({ field: k, message: w });
  }
  for (const k of Object.keys(fields)) if (doc[k] === undefined) errs.push({ field: k, message: "is required" });
  return errs;
}
export const docErrors = (col, id, doc) => (col === "items" ? itemErrors(doc) : col === "events" ? eventErrors(doc) : metaErrors(id, doc));

// A request body's keys against what a client may send: the server's own fields are named as such.
export function sentErrors(sent, allowed, owned) {
  const errs = [];
  if (!isObj(sent)) return [{ field: "", message: "the body must be a JSON object" }];
  for (const k of Object.keys(sent)) {
    if (allowed.includes(k)) continue;
    errs.push({ field: k, message: owned.includes(k) ? "is set by the server; don't send it" : `is not a field you can send here (these are: ${allowed.join(", ")})` });
  }
  return errs;
}
export const errorText = (errs) => errs.map((e) => (e.field ? `${e.field}: ${e.message}` : e.message)).join("; ");

// The browser page reads this (GET /api/schema): the lists, and which fields are tasks'.
export function describe() {
  const f = (fields) => Object.fromEntries(Object.entries(fields).map(([k, s]) => [k, { type: s.type, ...(s.values ? { values: s.values } : {}), ...(s.max ? { max: s.max } : {}), ...(s.min !== undefined ? { min: s.min } : {}), ...(s.server ? { server: true } : {}), ...(s.task ? { taskOnly: true } : {}) }]));
  return {
    stages: STAGES, itemKinds: ITEM_KINDS, wheres: WHERES, eventKinds: EVENT_KINDS, timeFormat: TIME_FORMAT,
    items: { fields: f(ITEM_FIELDS), create: ITEM_CREATE, edit: ITEM_EDIT },
    events: { fields: f(EVENT_FIELDS), create: EVENT_CREATE, edit: EVENT_EDIT },
    meta: Object.fromEntries(Object.entries(META_FIELDS).map(([k, v]) => [k, f(v)])),
  };
}
