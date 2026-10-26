// One-time (#672): validate the task board's existing data against schema.mjs and repair what
// doesn't fit. Prints what it would change, and changes nothing, unless run with --run.
//
//   node repair.mjs [db] [--run] [--all]
//
// A dry run does the whole repair inside a transaction, checks the result against the schema and
// the references, and rolls it back, so what it prints is what --run does. --run first copies the
// file (`<db>.before-repair-<time>`, VACUUM INTO), then commits only if nothing is left that doesn't
// fit; events it can't tie to an item are written to `<db>.quarantine-<time>.json` before they go.
// --all lists every change (else the first few of each kind). Stop the board's server before --run:
// the server only reads what a request asks for, but a write in the middle would be lost to the
// rollback or fail on the lock.
import path from "node:path";
import fs from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { openStore } from "./store.mjs";
import { ITEM_FIELDS, EVENT_KINDS, STAGES, WHERES, itemErrors, eventErrors, metaErrors, normTime, errorText } from "./schema.mjs";

// Time from what an old writer may have written: ISO text, or epoch milliseconds or seconds as a
// number or digits (with a fraction: "1792…000.0").
function toTime(v) {
  const n = typeof v === "number" ? v : typeof v === "string" && /^\d{9,}(\.\d+)?$/.test(v) ? Number(v) : null;
  if (n !== null) return new Date(n >= 1e11 ? n : n * 1000).toISOString();
  return normTime(v);
}

export function repair(store) {
  const notes = []; // {kind, id, what}
  const note = (kind, id, what = "") => notes.push({ kind, id, what });
  const quarantine = [];
  const items = new Map(store.list("items").map((d) => [d.id, d.data]));
  const byNum = new Map();
  for (const [id, d] of items) if (Number.isInteger(d.num) && !byNum.has(d.num)) byNum.set(d.num, id);
  const itemByDigits = (s) => { const m = /\d+/.exec(String(s ?? "")); return m ? byNum.get(Number(m[0])) : undefined; };

  // ---- events: the legacy shapes, then the one shape ------------------------------------------
  const out = []; // [{id, doc, from}] the events to write; `from`: the doc ids they replace
  const kindOfId = (id) => { const k = id.split("-")[2]; return EVENT_KINDS.includes(k) ? k : "note"; };
  for (const { id, data: d } of store.list("events")) {
    if (Array.isArray(d.events)) {
      // A bundle keyed by its item's id: {events: [{t, text}]}.
      const item = items.has(id) ? id : itemByDigits(id);
      d.events.forEach((e, i) => out.push({ id: i ? `${id}-${i + 1}` : id, from: id, doc: { item, kind: e.kind, t: e.t, text: e.text }, shape: "a bundle of events under an item's id" }));
      continue;
    }
    const doc = { item: d.item, kind: d.kind, t: d.t ?? d.at, text: d.text };
    let shape = null;
    if (d.task !== undefined || d.at !== undefined) shape = "task/at fields (item, t)";
    if (d.task !== undefined) doc.item = items.has(d.task) ? d.task : itemByDigits(d.num);
    if (doc.kind === undefined && d.task !== undefined) doc.kind = kindOfId(id);
    if (typeof doc.item !== "string" && d.task === undefined) {
      // No item: the event's own id names it (a number alone is a task's; else an item's id, or
      // "e<number>…"), else it is about the board itself.
      const via = /^\d+$/.test(id) ? itemByDigits(id) ?? (items.has(id) ? id : undefined) : items.has(id) ? id : /^e\d/.test(id) ? itemByDigits(id) : undefined;
      doc.item = via ?? "";
      shape = via ? "no item (named by the event's id)" : "no item (about the board itself)";
    } else if (typeof doc.item === "string" && doc.item !== "" && !items.has(doc.item)) {
      const via = itemByDigits(doc.item);
      if (via) { doc.item = via; shape = "item that isn't an id (a task's number)"; }
    }
    out.push({ id, from: id, doc, shape });
  }
  // The time an item's first event happened, to date what has none.
  const firstT = new Map();
  for (const e of out) { const t = toTime(e.doc.t); if (t && typeof e.doc.item === "string" && (!firstT.has(e.doc.item) || t < firstT.get(e.doc.item))) firstT.set(e.doc.item, t); }
  const earliest = [...firstT.values()].sort()[0] ?? new Date(0).toISOString();
  for (const e of out) {
    const { doc } = e;
    const bad = [];
    if (typeof doc.text !== "string" || !doc.text.trim()) bad.push("no text");
    if (typeof doc.item !== "string" || (doc.item !== "" && !items.has(doc.item))) bad.push(`no item (${JSON.stringify(doc.item ?? null)})`);
    if (bad.length) { quarantine.push({ id: e.from, why: bad.join(", "), doc: store.get("events", e.from) }); e.drop = true; continue; }
    if (e.shape) note("events: legacy shape rewritten", e.id, `${e.shape} → item ${doc.item}`);
    if (!EVENT_KINDS.includes(doc.kind)) { note("events: kind missing, set to note", e.id, `was ${JSON.stringify(doc.kind ?? null)}`); doc.kind = "note"; }
    const t = toTime(doc.t);
    if (t === null) {
      const it = items.get(doc.item);
      doc.t = firstT.get(doc.item) ?? toTime(it?.added) ?? toTime(it?.started) ?? earliest;
      note("events: time missing, dated by the item's other times", e.id, `${doc.t} (item ${doc.item || "(board)"})`);
    } else {
      if (t !== doc.t) note(typeof doc.t === "string" && normTime(doc.t) ? "events: time written in the one format" : "events: time converted", e.id, `${JSON.stringify(doc.t)} → ${t}`);
      doc.t = t;
    }
  }
  // ---- items --------------------------------------------------------------------------------------
  const siblingsMax = new Map();
  for (const d of items.values()) if (Number.isFinite(d.order)) siblingsMax.set(d.parent ?? "", Math.max(siblingsMax.get(d.parent ?? "") ?? 0, d.order));
  const fixedItems = new Map();
  for (const [id, d0] of items) {
    const d = { ...d0 };
    const was = (what, k, v) => note(`items: ${what}`, id, `${k}: ${JSON.stringify(d0[k] ?? null)} → ${JSON.stringify(v)}`);
    for (const k of Object.keys(d)) {
      if (!ITEM_FIELDS[k] || (ITEM_FIELDS[k].task && d.kind === "folder")) { note(`items: ${ITEM_FIELDS[k] ? "task-only field on a folder" : "unknown field"} removed`, id, `${k}: ${JSON.stringify(d[k])}`); delete d[k]; }
    }
    if (d.kind === "task") {
      if (!STAGES.includes(d.stage)) { d.stage = ""; was("stage missing, set to queued", "stage", d.stage); }
      if (!WHERES.includes(d.where)) { d.where = ""; was("where missing, set to none", "where", d.where); }
    }
    if (typeof d.parent !== "string") { d.parent = ""; was("parent missing, set to the top level", "parent", ""); }
    else if (d.parent !== "" && !items.has(d.parent)) { was("parent gone, set to the top level", "parent", ""); d.parent = ""; }
    if (!Number.isFinite(d.order)) { d.order = (siblingsMax.get(d.parent) ?? 0) + 1; siblingsMax.set(d.parent, d.order); was("order missing, put last among its siblings", "order", d.order); }
    for (const k of ["added", "archivedAt", "started", "finished"]) {
      if (d[k] === undefined) continue;
      const t = toTime(d[k]);
      if (t === null) { delete d[k]; was("time unreadable, removed", k, null); }
      else if (t !== d[k]) { d[k] = t; was(typeof d0[k] === "string" && normTime(d0[k]) ? "time written in the one format" : "time converted", k, t); }
    }
    if (d.archivedAt !== undefined && d.archived !== true) { delete d.archivedAt; was("archivedAt without archived, removed", "archivedAt", null); }
    fixedItems.set(id, d);
  }
  // An item with no added time takes its first event's, else its start, else the nearest lower
  // numbered task's (tasks are numbered as they are made), else its folder's, else the earliest time.
  const withAdded = [...fixedItems.values()].filter((d) => d.added && Number.isInteger(d.num)).sort((a, b) => a.num - b.num);
  for (const [id, d] of fixedItems) {
    if (d.added) continue;
    const lower = [...withAdded].reverse().find((x) => x.num < (d.num ?? Infinity));
    const how = firstT.has(id) ? ["its first event", firstT.get(id)] : d.started ? ["its start", d.started] : lower ? [`#${lower.num}'s`, lower.added] : fixedItems.get(d.parent)?.added ? ["its folder's", fixedItems.get(d.parent).added] : ["the earliest time on the board", earliest];
    d.added = how[1];
    note("items: added missing, dated", id, `${how[1]} (from ${how[0]})`);
  }
  // Numbers: a task has one, and no two share one.
  const taken = new Set();
  for (const [id, d] of fixedItems) if (d.num !== undefined) { if (taken.has(d.num)) { note("items: duplicate number removed", id, `#${d.num}`); delete d.num; } else taken.add(d.num); }
  let next = Math.max(store.get("meta", "counter")?.next ?? 1, 1 + Math.max(0, ...taken));
  for (const [id, d] of [...fixedItems].filter(([, d]) => d.kind === "task" && d.num === undefined).sort((a, b) => a[1].added < b[1].added ? -1 : 1)) { d.num = next++; note("items: task without a number, numbered", id, `#${d.num}`); }

  // ---- meta -----------------------------------------------------------------------------------------
  const counter = { next: Math.max(next, 1 + Math.max(0, ...taken)) };
  const status0 = store.get("meta", "status") ?? { now: "" };
  const status = { now: typeof status0.now === "string" ? status0.now : "", updated: toTime(status0.updated) ?? new Date().toISOString() };
  if (JSON.stringify(status) !== JSON.stringify(status0)) note("meta: status written in the one shape", "status", `updated: ${JSON.stringify(status0.updated ?? null)} → ${status.updated}`);
  if (JSON.stringify(counter) !== JSON.stringify(store.get("meta", "counter"))) note("meta: counter raised past the highest number", "counter", JSON.stringify(counter));

  // ---- write ------------------------------------------------------------------------------------------
  for (const [id, d] of fixedItems) if (JSON.stringify(d) !== JSON.stringify(items.get(id)) || store.get("items", id) === null) store.put("items", id, d);
  for (const id of new Set([...out.map((e) => e.from), ...quarantine.map((q) => q.id)])) store.del("events", id);
  const used = new Set();
  for (const e of out) {
    if (e.drop) continue;
    let id = e.id;
    while (used.has(id) || store.get("events", id)) id += "x";
    used.add(id);
    store.put("events", id, e.doc);
  }
  store.put("meta", "counter", counter);
  store.put("meta", "status", status);
  for (const q of quarantine) note("events: dropped (can't be tied to an item)", q.id, `${q.why}: ${JSON.stringify(q.doc).slice(0, 160)}`);
  return { notes, quarantine };
}

// Everything left that doesn't fit: schema, references, numbers, cycles.
export function problems(store) {
  const out = [];
  const items = new Map(store.list("items").map((d) => [d.id, d.data]));
  const nums = new Map();
  for (const [id, d] of items) {
    for (const e of itemErrors(d)) out.push(`items/${id}: ${errorText([e])}`);
    if (d.parent && !items.has(d.parent)) out.push(`items/${id}: parent ${d.parent} doesn't exist`);
    if (d.num !== undefined) { if (nums.has(d.num)) out.push(`items/${id}: #${d.num} is also ${nums.get(d.num)}'s`); nums.set(d.num, id); }
    const seen = new Set([id]);
    for (let p = d.parent; p; p = items.get(p)?.parent) { if (seen.has(p)) { out.push(`items/${id}: its parents loop`); break; } seen.add(p); }
  }
  for (const { id, data: d } of store.list("events")) {
    for (const e of eventErrors(d)) out.push(`events/${id}: ${errorText([e])}`);
    if (d.item && !items.has(d.item)) out.push(`events/${id}: item ${d.item} doesn't exist`);
  }
  for (const { id, data: d } of store.list("meta")) for (const e of metaErrors(id, d)) out.push(`meta/${id}: ${errorText([e])}`);
  if ((store.get("meta", "counter")?.next ?? 0) <= store.maxNum()) out.push("meta/counter: next isn't past the highest number");
  return out;
}

export function main(argv) {
  const run = argv.includes("--run"), all = argv.includes("--all");
  const file = path.resolve(argv.find((a) => !a.startsWith("--")) ?? path.join(path.dirname(fileURLToPath(import.meta.url)), "taskboard.db"));
  if (!fs.existsSync(file)) { console.error(`no database at ${file}`); return 2; }
  const store = openStore(file);
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const before = problems(store);
  console.log(`${file}\n${store.list("items").length} items, ${store.list("events").length} events; ${before.length} things that don't fit the schema now`);
  if (run) { const copy = `${file}.before-repair-${stamp}`; store.db.exec(`VACUUM INTO '${copy.replace(/'/g, "''")}'`); console.log(`copied to ${copy}`); }
  store.db.exec("BEGIN IMMEDIATE");
  let after, result;
  try {
    result = repair(store);
    after = problems(store);
  } catch (e) { store.db.exec("ROLLBACK"); throw e; }
  const groups = new Map();
  for (const n of result.notes) (groups.get(n.kind) || groups.set(n.kind, []).get(n.kind)).push(n);
  for (const [kind, ns] of groups) {
    console.log(`\n${kind}: ${ns.length}`);
    for (const n of all ? ns : ns.slice(0, 4)) console.log(`  ${n.id}  ${n.what}`);
    if (!all && ns.length > 4) console.log(`  … ${ns.length - 4} more (--all)`);
  }
  console.log(`\n${result.notes.length} changes in all; ${after.length} things still don't fit afterwards`);
  for (const p of after) console.log(`  ${p}`);
  if (after.length || !run) {
    store.db.exec("ROLLBACK");
    console.log(after.length ? "rolled back: nothing was changed" : "dry run: nothing was changed (--run does the above)");
    return after.length ? 1 : 0;
  }
  if (result.quarantine.length) { const q = `${file}.quarantine-${stamp}.json`; fs.writeFileSync(q, JSON.stringify(result.quarantine, null, 1)); console.log(`events dropped: kept in ${q}`); }
  store.db.exec("COMMIT");
  console.log("repaired");
  return 0;
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exit(main(process.argv.slice(2)));
