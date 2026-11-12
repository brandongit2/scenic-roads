// A run's spans as a timeline's lanes (timeline.js): the dev job dashboard's run (tools/devtools'
// jobs.js, served at /lib/buildpage/) and a job's page (job.js, task #1105). Its spans come flat,
// each naming its `parent` ({id, parent, kind, name, t0, t1, status, attrs}: devjob.py's runs and
// the lead's `GET /api/job/<id>` alike); `spanTree` rebuilds the tree, `spanLanes` lays it out: a
// lane group a kind (or a thread: `group`) at each depth, its spans packed into rows that don't
// overlap at the view's zoom, those too short to see there sharing one row of their own.

/** Rows of items that don't overlap (by `gap` s), in time order: [[item…]…]. */
export function pack(items, gap, from = (x) => x.from, until = (x) => x.until) {
  const rows = [], ends = [];
  for (const it of [...items].sort((a, b) => from(a) - from(b))) {
    let i = ends.findIndex((e) => e + gap <= from(it));
    if (i < 0) { i = rows.length; rows.push([]); ends.push(-Infinity); }
    rows[i].push(it);
    ends[i] = Math.max(until(it), from(it) + gap);
  }
  return rows;
}

/** The tree of `spans` under the run `root` (its id): {byId, kids (parent id → spans), depth (id →
 * 1 for the run's own parts, deeper below; the run 0)}. A span whose parent isn't among them is the
 * run's own; a loop of parents is cut where it closes. */
export function spanTree(spans, root) {
  const byId = new Map(spans.map((s) => [s.id, s]));
  const kids = new Map();
  for (const s of spans) if (s.id !== root) (kids.get(s.parent) || kids.set(s.parent, []).get(s.parent)).push(s);
  const depth = new Map([[root, 0]]);
  for (const s of spans) {
    const chain = [];
    let x = s;
    while (x && !depth.has(x.id) && !chain.includes(x)) { chain.push(x); x = x.parent === root ? null : byId.get(x.parent); }
    let d = x && depth.has(x.id) ? depth.get(x.id) : 0;
    for (let i = chain.length - 1; i >= 0; i--) depth.set(chain[i].id, ++d);
  }
  return { byId, kids, depth };
}

/** The ids of the span `id`, what it's in and what's in it (a span pointed at lights them). */
export function lineage(tree, id) {
  const on = new Set([id]);
  for (let p = tree.byId.get(id)?.parent; p != null && tree.byId.has(p) && !on.has(p); p = tree.byId.get(p).parent) on.add(p);
  const down = (k) => { for (const c of tree.kids.get(k) || []) if (!on.has(c.id)) { on.add(c.id); down(c.id); } };
  down(id);
  return on;
}

/** The lanes of the spans of `tree` (the run's own left out) on timeline `tl`: rows [{label, title,
 * track}] in order, `outer` "depth" (each depth's groups) or "group" (each group's depths); groups
 * named by `group(span)`, ranked by `order` (others after, by name); each row's spans added by
 * `add(track, span, short)` (`short`: too short to see at `gap` s, or untimed, in its group's first
 * row); `label(group, depth, n)` names a group's first row; `sep` the room between two spans of a
 * row (s; `gap` by default: one ending as the next starts, a thread's phases, may share it at 0).
 * Spans of no start are left out. */
export function spanLanes(tl, tree, { gap, now, group = (s) => s.kind, order = [], outer = "depth", add, label, sep = gap }) {
  const at = new Map(); // "depth group" → spans
  for (const s of tree.byId.values()) {
    const d = tree.depth.get(s.id);
    if (!d || s.t0 == null) continue;
    const k = `${d}\n${group(s)}`;
    (at.get(k) || at.set(k, []).get(k)).push(s);
  }
  const rank = (g) => { const i = order.indexOf(g); return i < 0 ? order.length : i; };
  const keys = [...at.keys()].map((k) => { const [d, g] = k.split("\n"); return { k, d: +d, g }; });
  keys.sort((a, b) => (outer === "group" ? rank(a.g) - rank(b.g) || a.g.localeCompare(b.g) || a.d - b.d : a.d - b.d || rank(a.g) - rank(b.g) || a.g.localeCompare(b.g)));
  const rows = [];
  for (const { k, d, g } of keys) {
    const mine = at.get(k);
    const short = (s) => !!s.attrs?.untimed || (s.t1 != null && s.t1 - s.t0 < gap);
    const marks = mine.filter(short), timed = mine.filter((s) => !short(s));
    [...(marks.length ? [marks] : []), ...pack(timed, sep, (s) => s.t0, (s) => s.t1 ?? now)].forEach((row, i) => {
      const track = tl.track();
      for (const s of row) add(track, s, short(s));
      rows.push({ label: i ? "" : label(g, d, mine.length), title: g, track });
    });
  }
  return rows;
}
