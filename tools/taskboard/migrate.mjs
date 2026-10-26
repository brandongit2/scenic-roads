// The one-time migration to one list (#709): only folders hold items, every item has a number, and
// `order` is one order over all items.
//
//   node migrate.mjs [db]          dry run: does it all in a transaction, checks, prints, rolls back
//   node migrate.mjs [db] --run    the same for real: first copies the file (<db>.before-709-<time>)
//
// Stop the board's server before --run. It refuses a file it has already migrated (one whose items
// table has no `slot` column), and commits only when the check passes:
//   1. A task that holds items becomes a folder (its task fields go; an event on it keeps them).
//   2. Every item without a number takes the next from meta/counter, in tree order.
//   3. `order` becomes each item's place in the tree, walked depth first (siblings by their old order),
//      so the tree shows as before and equal priorities follow it. `slot` (the old order within a
//      priority) is dropped, column and all.
// The check: the same items, with the same ids and parents; the same events, on the same items (plus
// the one per converted task); every number unique and the counter past them; no task holding items;
// everything fitting schema.mjs.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath, pathToFileURL } from "node:url";
import { openStore } from "./store.mjs";
import { itemErrors, eventErrors, metaErrors, errorText } from "./schema.mjs";

const TASK_ONLY = ["stage", "where", "priority", "discuss", "started", "finished", "reply"];

const hasSlot = (store) => store.db.prepare(`SELECT 1 FROM pragma_table_info('items') WHERE name = 'slot'`).get() !== undefined;

// The items in tree order: depth first from the top level, siblings by order then id; an item whose
// parent is missing (or loops) counts as top level, as the page shows it.
function treeOrder(items) {
  const kids = new Map();
  const top = (id, d) => !d.parent || d.parent === id || !items.has(d.parent);
  for (const [id, d] of items) { const p = top(id, d) ? "" : d.parent; (kids.get(p) || kids.set(p, []).get(p)).push(id); }
  const ord = (id) => (Number.isFinite(items.get(id).order) ? items.get(id).order : 0);
  for (const l of kids.values()) l.sort((a, b) => ord(a) - ord(b) || (a < b ? -1 : a > b ? 1 : 0));
  const out = [], seen = new Set();
  const walk = (id) => { if (seen.has(id)) return; seen.add(id); out.push(id); for (const k of kids.get(id) || []) walk(k); };
  for (const id of kids.get("") || []) walk(id);
  for (const id of items.keys()) walk(id); // (parents that loop)
  return out;
}

// Changes the open store (inside the caller's transaction). Returns what it did.
export function migrate(store) {
  const items = new Map(store.list("items").map((d) => [d.id, d.data]));
  const parents = new Set([...items.values()].map((d) => d.parent).filter(Boolean));
  const seq = treeOrder(items);
  const now = new Date().toISOString();
  const folded = [], numbered = [], events = [];
  for (const id of seq) {
    const d = items.get(id);
    if (d.kind === "task" && parents.has(id)) {
      const kept = TASK_ONLY.filter((k) => d[k] !== undefined).map((k) => `${k}: ${JSON.stringify(d[k])}`);
      for (const k of TASK_ONLY) delete d[k];
      d.kind = "folder";
      folded.push({ id, num: d.num, text: d.text, inside: [...items.values()].filter((c) => c.parent === id).length });
      const e = { item: id, kind: "note", t: now, text: `Made a folder by #709's migration (only folders hold items now). As a task it had ${kept.join(", ") || "no task fields"}.` };
      events.push(e);
      store.put("events", crypto.randomUUID(), e);
    }
  }
  let next = Math.max(store.get("meta", "counter")?.next || 1, store.maxNum() + 1);
  for (const id of seq) { const d = items.get(id); if (!Number.isInteger(d.num)) { d.num = next++; numbered.push({ id, num: d.num, kind: d.kind, text: d.text }); } }
  store.put("meta", "counter", { next });
  let reordered = 0;
  seq.forEach((id, i) => { const d = items.get(id); if (d.order !== i + 1) reordered++; d.order = i + 1; });
  const slots = store.db.prepare(`SELECT id, slot FROM items WHERE slot IS NOT NULL`).all();
  for (const id of seq) store.put("items", id, items.get(id));
  store.db.exec(`ALTER TABLE items DROP COLUMN slot`);
  return { folded, numbered, reordered, slots, events: events.length };
}

// Everything that doesn't fit: schema, references, numbers, cycles, tasks holding items.
export function problems(store) {
  const out = [];
  const items = new Map(store.list("items").map((d) => [d.id, d.data]));
  const nums = new Map();
  for (const [id, d] of items) {
    for (const e of itemErrors(d)) out.push(`items/${id}: ${errorText([e])}`);
    if (d.parent && !items.has(d.parent)) out.push(`items/${id}: parent ${d.parent} doesn't exist`);
    else if (d.parent && items.get(d.parent).kind !== "folder") out.push(`items/${id}: its parent ${d.parent} is a task`);
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

// What must survive: every item (id → parent) and event (id → item, kind, text, t).
const snapshot = (store) => ({
  items: new Map(store.list("items").map((d) => [d.id, d.data.parent])),
  events: new Map(store.list("events").map((d) => [d.id, JSON.stringify(d.data)])),
});
function lost(a, b) {
  const out = [];
  for (const [id, p] of a.items) if (!b.items.has(id)) out.push(`items/${id} is gone`); else if (b.items.get(id) !== p) out.push(`items/${id}: parent ${p} → ${b.items.get(id)}`);
  for (const id of b.items.keys()) if (!a.items.has(id)) out.push(`items/${id} is new`);
  for (const [id, e] of a.events) if (b.events.get(id) !== e) out.push(`events/${id} ${b.events.has(id) ? "changed" : "is gone"}`);
  return out;
}

export function main(argv) {
  const run = argv.includes("--run");
  const file = path.resolve(argv.find((a) => !a.startsWith("--")) ?? path.join(path.dirname(fileURLToPath(import.meta.url)), "taskboard.db"));
  if (!fs.existsSync(file)) { console.error(`no database at ${file}`); return 2; }
  const store = openStore(file);
  try {
    if (!hasSlot(store)) { console.error(`${file} is migrated already (its items have no slot column): nothing done`); return 3; }
    const before = snapshot(store);
    console.log(`${file}\n${before.items.size} items, ${before.events.size} events, counter at ${store.get("meta", "counter")?.next}`);
    if (run) { const copy = `${file}.before-709-${new Date().toISOString().replace(/[:.]/g, "-")}`; store.db.exec(`VACUUM INTO '${copy.replace(/'/g, "''")}'`); console.log(`copied to ${copy}`); }
    store.db.exec("BEGIN IMMEDIATE");
    let r, bad;
    try {
      r = migrate(store);
      const after = snapshot(store);
      bad = [...lost(before, after), ...problems(store)];
      if (after.events.size !== before.events.size + r.events) bad.push(`events: ${before.events.size} + ${r.events} expected, ${after.events.size} found`);
    } catch (e) { store.db.exec("ROLLBACK"); throw e; }
    console.log(`\ntasks holding items, made folders: ${r.folded.length}`);
    for (const f of r.folded) console.log(`  ${f.id}  #${f.num} ${JSON.stringify(f.text)} (${f.inside} inside)`);
    console.log(`\nitems numbered: ${r.numbered.length}${r.numbered.length ? ` (#${r.numbered[0].num}–#${r.numbered.at(-1).num}); the counter is at ${store.get("meta", "counter").next} now` : ""}`);
    for (const n of r.numbered) console.log(`  #${n.num}  ${n.id}  ${n.kind} ${JSON.stringify(n.text)}`);
    console.log(`\norder: ${r.reordered} of ${before.items.size} items given their place in the tree; slot dropped (${r.slots.length} had one: ${r.slots.map((s) => `${s.id}=${s.slot}`).join(", ") || "none"})`);
    console.log(`\nchecked: ${bad.length ? `${bad.length} problems` : "every item with its id and parent, every event, the numbers, no task holding items, the schema: all fine"}`);
    for (const p of bad) console.log(`  ${p}`);
    if (bad.length || !run) {
      store.db.exec("ROLLBACK");
      console.log(bad.length ? "rolled back: nothing was changed" : "dry run: nothing was changed (--run does the above)");
      return bad.length ? 1 : 0;
    }
    store.db.exec("COMMIT");
    console.log("migrated");
    return 0;
  } finally { store.close(); }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exit(main(process.argv.slice(2)));
