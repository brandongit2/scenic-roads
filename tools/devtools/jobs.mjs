// The dev job dashboard's data (README.md, "Dev jobs"): every run of the long dev jobs as a tree of
// spans, and each Mac's load and memory while one runs, in their own SQLite file (jobs.db).
//
// The Macs record them (tools/dev/devjob.zsh and devjob.py): events appended to a spool on the Mac,
// sent here by its sampler (POST /api/jobs/events, NDJSON). An event names its span by an id the Mac
// made, so a span's start, end and attributes may arrive in any order, and again (a resend after a
// failed post): each is merged into the span. Times are unix seconds.
//   {op: "start", id, parent?, kind, name, mac, t, attrs?}     a span begins (status "running")
//   {op: "end", id, t, status, attrs?}                          it ends
//   {op: "span", id, parent?, kind, name, mac, t0, t1, status, attrs?}   a whole span at once
//   {op: "attr", id, attrs}                                     more of its attributes
//   {op: "sample", mac, t, load, memfree, swap, ncpu, chip?, mem?, live: [ids]}   a Mac, every 10 s
// A running span its Mac's sampler hasn't named live for LOST_S is shown as lost (the Mac asleep,
// away or restarted); the sampler itself ends one whose process is gone (status "gone").
// A span's `mac` is the Mac it ran on: a test-Mac run (kind "run", attrs.ran_on) and its parts are
// the test Mac's. A tests job (remote-tests.sh's) is the Mac's it started on, its attrs.ran_on the
// test Mac its run went to (said as it starts there; attrs.rerun_here when it ran here after all):
// a span read is `on` the Mac it ran on, what the page groups by.
import { DatabaseSync } from "node:sqlite";
import { refuse, send, fail, readBody } from "./http.mjs";

export const OPS = ["start", "end", "span", "attr", "sample"];
export const STATUSES = ["running", "ok", "fail", "stopped", "gone", "skip", "reuse"];
// A running span not named live by its Mac's sampler for this long is lost (the sampler names each
// live one every 10 s: tools/dev/devjob.py SAMPLE_S).
export const LOST_S = 60;
const MAX_EVENTS_BODY = 32 * 1024 * 1024;
const MAX_ATTRS = 64 * 1024;
const ID = /^[A-Za-z0-9_.:@+\-]{1,200}$/;
const T_MIN = Date.UTC(2026, 0, 1) / 1000; // (nothing was recorded before this app)

export function openJobs(file) {
  const db = new DatabaseSync(file);
  db.exec(`
    PRAGMA journal_mode = WAL;
    CREATE TABLE IF NOT EXISTS spans (
      id TEXT PRIMARY KEY,
      parent TEXT,               -- the span it is part of ('' or null: a run of its own)
      root TEXT NOT NULL,        -- the run it is part of: the top of its parents (itself for a run)
      kind TEXT, name TEXT,
      mac TEXT,                  -- the Mac it ran on (LocalHostName)
      t0 REAL, t1 REAL,          -- unix s; t1 null while it runs
      status TEXT,               -- jobs.mjs STATUSES
      attrs TEXT NOT NULL DEFAULT '{}',   -- JSON object
      seen REAL                  -- the last time its Mac's sampler named it live
    );
    CREATE INDEX IF NOT EXISTS spans_root ON spans(root);
    CREATE INDEX IF NOT EXISTS spans_runs ON spans(t0) WHERE root = id;   -- (the runs, few beside their tests)
    CREATE INDEX IF NOT EXISTS spans_kind ON spans(kind, t0);
    CREATE TABLE IF NOT EXISTS samples (
      mac TEXT NOT NULL, t REAL NOT NULL,
      load REAL,                 -- the 1-minute load average
      memfree REAL,              -- the memory available, % (kern.memorystatus_level)
      swap REAL,                 -- swap used, MB
      PRIMARY KEY (mac, t)
    ) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS samples_t ON samples(t);
    CREATE TABLE IF NOT EXISTS macs (mac TEXT PRIMARY KEY, chip TEXT, mem REAL, ncpu INTEGER, seen REAL);
  `);
  // (Version 1, #1086: before it, a test-Mac run and its parts were the recording Mac's; moved to
  // the Mac their run names.)
  if (db.prepare("PRAGMA user_version").get().user_version < 1) {
    db.exec("BEGIN IMMEDIATE");
    try { onTheirMac(db); db.exec("PRAGMA user_version = 1; COMMIT"); } catch (e) { db.exec("ROLLBACK"); throw e; }
  }
  const stmts = {};
  const stmt = (sql) => (stmts[sql] ||= db.prepare(sql));
  return { db, stmt, close: () => db.close() };
}
// Every test-Mac run (a span naming `ran_on`, kind "run") and its parts put on that Mac.
function onTheirMac(db) {
  const remote = db.prepare(`SELECT id, root, json_extract(attrs, '$.ran_on') AS on_ FROM spans WHERE kind = 'run' AND json_extract(attrs, '$.ran_on') IS NOT NULL`).all();
  const inRun = db.prepare(`SELECT id, parent FROM spans WHERE root = ?`), set = db.prepare(`UPDATE spans SET mac = ? WHERE id = ?`);
  const kidsOf = new Map(); // a run's root → its spans' kids by parent
  for (const r of remote) {
    if (!kidsOf.has(r.root)) {
      const kids = new Map();
      for (const s of inRun.all(r.root)) (kids.get(s.parent) || kids.set(s.parent, []).get(s.parent)).push(s.id);
      kidsOf.set(r.root, kids);
    }
    const kids = kidsOf.get(r.root);
    for (const todo = [r.id]; todo.length;) { const id = todo.pop(); set.run(r.on_, id); todo.push(...(kids.get(id) || [])); }
  }
}

// Why event `e` can't be taken, or null.
export function eventError(e) {
  if (e === null || typeof e !== "object" || Array.isArray(e)) return "not an object";
  if (!OPS.includes(e.op)) return `op: one of ${OPS.join(", ")}`;
  const time = (k, need = true) => {
    const v = e[k];
    if (v === undefined || v === null) return need ? `${k}: is required (unix s)` : null;
    return typeof v === "number" && Number.isFinite(v) && v >= T_MIN && v <= Date.now() / 1000 + 86400 ? null : `${k}: unix seconds, from 2026 on and not a day ahead`;
  };
  const str = (k, max, need = true) => {
    const v = e[k];
    if (v === undefined || v === null || v === "") return need ? `${k}: is required` : null;
    return typeof v === "string" && v.length <= max ? null : `${k}: a string of at most ${max} characters`;
  };
  const num = (k) => (e[k] === undefined || e[k] === null || (typeof e[k] === "number" && Number.isFinite(e[k])) ? null : `${k}: a number`);
  if (e.op === "sample") {
    if (!Array.isArray(e.live ?? []) || !(e.live ?? []).every((x) => typeof x === "string" && ID.test(x))) return "live: a list of span ids";
    return str("mac", 200) || time("t") || num("load") || num("memfree") || num("swap") || num("ncpu") || num("mem") || str("chip", 200, false);
  }
  if (typeof e.id !== "string" || !ID.test(e.id)) return "id: 1-200 of A-Z a-z 0-9 _ . : @ + -";
  if (e.parent !== undefined && e.parent !== null && e.parent !== "" && (typeof e.parent !== "string" || !ID.test(e.parent))) return "parent: a span's id";
  if (e.parent === e.id && e.parent) return "parent: not itself";
  if (e.attrs !== undefined) {
    if (e.attrs === null || typeof e.attrs !== "object" || Array.isArray(e.attrs)) return "attrs: an object";
    if (JSON.stringify(e.attrs).length > MAX_ATTRS) return `attrs: at most ${MAX_ATTRS} bytes`;
  }
  if (e.status !== undefined && !STATUSES.includes(e.status)) return `status: one of ${STATUSES.join(", ")}`;
  if (e.op === "start") return str("kind", 200) || str("name", 2000) || str("mac", 200) || time("t");
  if (e.op === "end") return time("t") || (e.status ? null : "status: is required");
  if (e.op === "span") {
    const bad = str("kind", 200) || str("name", 2000) || str("mac", 200) || time("t0") || time("t1") || (e.status ? null : "status: is required");
    return bad || (e.t1 < e.t0 ? "t1: not before t0" : null);
  }
  if (e.op === "attr") return e.attrs ? null : "attrs: is required";
  return null;
}

export function makeJobs(jobs, changed) {
  const { db, stmt } = jobs;
  const rootOf = (id) => stmt(`SELECT root FROM spans WHERE id = ?`).get(id)?.root;
  // A span merged in: what it says over what was there (a status other than "running" is never
  // set back to it: an end may come before its start), its attributes added to.
  function upsert(e, f) {
    const parent = e.parent || null;
    const root = parent ? rootOf(parent) ?? parent : e.id;
    stmt(`INSERT INTO spans (id, parent, root, kind, name, mac, t0, t1, status, attrs) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(id) DO UPDATE SET
        parent = COALESCE(excluded.parent, parent),
        root = CASE WHEN excluded.parent IS NOT NULL THEN excluded.root ELSE root END,
        kind = COALESCE(excluded.kind, kind), name = COALESCE(excluded.name, name), mac = COALESCE(excluded.mac, mac),
        t0 = COALESCE(excluded.t0, t0), t1 = COALESCE(excluded.t1, t1),
        status = CASE WHEN excluded.status IS NULL OR (excluded.status = 'running' AND status IS NOT NULL) THEN status ELSE excluded.status END,
        attrs = json_patch(attrs, excluded.attrs)`)
      .run(e.id, parent, root, f.kind ?? null, f.name ?? null, f.mac ?? null, f.t0 ?? null, f.t1 ?? null, f.status ?? null, JSON.stringify(e.attrs || {}));
    // (Its parts that came first, taking it for their run, belong to its run.)
    const mine = rootOf(e.id);
    if (mine !== e.id) stmt(`UPDATE spans SET root = ? WHERE root = ? AND id <> ?`).run(mine, e.id, e.id);
  }
  function apply(e) {
    switch (e.op) {
      case "start": return upsert(e, { kind: e.kind, name: e.name, mac: e.mac, t0: e.t, status: "running" });
      case "end": return upsert(e, { t1: e.t, status: e.status });
      case "span": return upsert(e, { kind: e.kind, name: e.name, mac: e.mac, t0: e.t0, t1: e.t1, status: e.status });
      case "attr": return upsert(e, {});
      case "sample": {
        stmt(`INSERT OR REPLACE INTO samples (mac, t, load, memfree, swap) VALUES (?, ?, ?, ?, ?)`).run(e.mac, e.t, e.load ?? null, e.memfree ?? null, e.swap ?? null);
        stmt(`INSERT INTO macs (mac, chip, mem, ncpu, seen) VALUES (?, ?, ?, ?, ?) ON CONFLICT(mac) DO UPDATE SET
          chip = COALESCE(excluded.chip, chip), mem = COALESCE(excluded.mem, mem), ncpu = COALESCE(excluded.ncpu, ncpu), seen = MAX(COALESCE(seen, 0), excluded.seen)`)
          .run(e.mac, e.chip ?? null, e.mem ?? null, e.ncpu ?? null, e.t);
        for (const id of e.live || []) stmt(`UPDATE spans SET seen = MAX(COALESCE(seen, 0), ?) WHERE id = ?`).run(e.t, id);
        return;
      }
    }
  }
  // POST /api/jobs/events: NDJSON (or a JSON array). Every line that fits is taken, in one
  // transaction; one that doesn't is named with why (a spool must never stick on one bad line).
  async function post(req, res) {
    let raw;
    try { raw = (await readBody(req, MAX_EVENTS_BODY)).toString("utf8"); } catch (e) { if (e?.code === "too_large") return fail(res, 413, "too_large", `over ${MAX_EVENTS_BODY / 1048576} MB: send fewer events at once`); throw e; }
    let events;
    const t = raw.trimStart();
    if (t.startsWith("[")) {
      try { events = JSON.parse(t).map((e, i) => ({ n: i + 1, e })); } catch { refuse(400, "invalid_argument", "the body is not valid JSON"); }
    } else {
      events = raw.split("\n").map((l, i) => ({ n: i + 1, l })).filter((x) => x.l.trim()).map(({ n, l }) => {
        try { return { n, e: JSON.parse(l) }; } catch { return { n, why: "not valid JSON" }; }
      });
    }
    const refused = [];
    let accepted = 0;
    db.exec("BEGIN IMMEDIATE");
    try {
      for (const x of events) {
        const why = x.why || eventError(x.e);
        if (why) { refused.push({ line: x.n, why }); continue; }
        apply(x.e);
        accepted++;
      }
      db.exec("COMMIT");
    } catch (e) { db.exec("ROLLBACK"); throw e; }
    if (accepted) changed("jobs");
    return send(res, 200, { accepted, refused });
  }

  const now = () => Date.now() / 1000;
  const row = (r) => {
    const attrs = JSON.parse(r.attrs || "{}");
    const out = { id: r.id, parent: r.parent || null, kind: r.kind, name: r.name, mac: r.mac, on: (!attrs.rerun_here && attrs.ran_on) || r.mac, t0: r.t0, t1: r.t1, status: r.status, attrs };
    if (r.status === "running" && Math.max(r.seen || 0, r.t0 || 0) < now() - LOST_S) out.lost = Math.max(r.seen || 0, r.t0 || 0);
    return out;
  };
  const range = (url) => {
    const n = (k, d) => { const v = Number(url.searchParams.get(k)); return url.searchParams.has(k) && Number.isFinite(v) ? v : d; };
    const to = n("to", now()), from = n("from", to - 86400);
    if (!(from < to)) refuse(400, "invalid_argument", "from: before to (unix s)");
    return { from, to };
  };
  // The runs (spans of no other) overlapping [from, to], each with how many spans it holds, how
  // many of them failed and the Macs they ran on. (A span whose parent hasn't come is part of that
  // parent's run, shown once it comes: a Mac sends a run's start before its parts'.) And `away`: the
  // tests jobs inside other runs (check.sh's, publish's) whose run went to another Mac, each with
  // its run's id (`root`), shown on that Mac.
  function runs(url) {
    const { from, to } = range(url);
    const kind = url.searchParams.get("kind");
    const rs = stmt(`SELECT * FROM spans WHERE root = id AND t0 <= ? AND (t1 IS NULL OR t1 >= ?) ${kind ? "AND kind = ?" : ""} ORDER BY t0`)
      .all(...[to, from, ...(kind ? [kind] : [])]);
    const counts = new Map(stmt(`SELECT root, COUNT(*) AS n, SUM(status IN ('fail', 'gone')) AS bad, json_group_array(DISTINCT mac) AS ms FROM spans WHERE root IN (SELECT value FROM json_each(?)) GROUP BY root`)
      .all(JSON.stringify(rs.map((r) => r.root))).map((c) => [c.root, c]));
    const away = !kind || kind === "tests" ? stmt(`SELECT * FROM spans WHERE kind = 'tests' AND root <> id AND t0 <= ? AND (t1 IS NULL OR t1 >= ?)
      AND json_extract(attrs, '$.ran_on') <> mac AND json_extract(attrs, '$.rerun_here') IS NULL ORDER BY t0`).all(to, from) : [];
    return {
      now: now(), from, to,
      runs: rs.map((r) => { const c = counts.get(r.root); return { ...row(r), spans: (c?.n || 1) - 1, failed: c?.bad || 0, macs: c ? JSON.parse(c.ms).filter(Boolean).sort() : [r.mac] }; }),
      away: away.map((r) => ({ ...row(r), root: r.root })),
      macs: macs(),
    };
  }
  const macs = () => Object.fromEntries(stmt(`SELECT * FROM macs`).all().map((m) => [m.mac, { chip: m.chip, mem: m.mem, ncpu: m.ncpu, seen: m.seen }]));
  // The run a span is part of, whole: the run, its spans, and the load and memory of the Macs they
  // ran on (a tests job's test Mac too, before its run there has come) over its time.
  function run(id) {
    const r = stmt(`SELECT * FROM spans WHERE id = ?`).get(id);
    if (!r) refuse(404, "not_found", `no span ${JSON.stringify(id)}`);
    const top = stmt(`SELECT * FROM spans WHERE id = ?`).get(r.root) || r;
    const spans = stmt(`SELECT * FROM spans WHERE root = ? AND id <> ? ORDER BY t0`).all(top.root, top.id).map(row);
    const t0 = top.t0 ?? Math.min(...spans.map((s) => s.t0 ?? Infinity)), t1 = top.t1 ?? now();
    const ms = new Set([row(top), ...spans].flatMap((s) => [s.mac, s.on]).filter(Boolean));
    const samples = {};
    for (const m of ms) samples[m] = stmt(`SELECT t, load, memfree, swap FROM samples WHERE mac = ? AND t >= ? AND t <= ? ORDER BY t`).all(m, t0 - 60, t1 + 60).map((s) => [s.t, s.load, s.memfree, s.swap]);
    return { now: now(), run: row(top), spans, samples, macs: macs() };
  }
  // Each Mac's samples over [from, to], at most `n` a Mac (the page asks for about one a pixel):
  // each a bucket's [t, the most load, the least memory free, the most swap].
  function samples(url) {
    const { from, to } = range(url);
    const n = Math.max(10, Math.min(5000, Number(url.searchParams.get("n")) || 600));
    const w = (to - from) / n, out = {};
    for (const s of stmt(`SELECT mac, MIN(t) AS t, MAX(load) AS load, MIN(memfree) AS memfree, MAX(swap) AS swap FROM samples
        WHERE t >= ? AND t <= ? GROUP BY mac, CAST((t - ?) / ? AS INTEGER) ORDER BY mac, t`).all(from, to, from, w)) {
      (out[s.mac] ||= []).push([s.t, s.load, s.memfree, s.swap]);
    }
    return { from, to, step: w, samples: out, macs: macs() };
  }
  // Durations by name for one kind of span (a check part, a publish step, a test suite, a test, a
  // crate's compile…), all time or [from, to]: how often each ran, failed and was skipped or reused,
  // and its times (of those that ran and ended; not cargo test's tests, which have none).
  function stats(url) {
    const kind = url.searchParams.get("kind");
    if (!kind) refuse(400, "invalid_argument", "kind: is required");
    const has = url.searchParams.has("from") || url.searchParams.has("to");
    const { from, to } = has ? range(url) : { from: 0, to: now() + 86400 };
    const by = new Map();
    for (const r of stmt(`SELECT name, t0, t1, status, mac, json_extract(attrs, '$.untimed') AS untimed FROM spans WHERE kind = ? AND t0 >= ? AND t0 <= ? ORDER BY t0`).all(kind, from, to)) {
      let g = by.get(r.name);
      if (!g) by.set(r.name, (g = { name: r.name, n: 0, failed: 0, skipped: 0, d: [], last: null }));
      if (r.status === "skip" || r.status === "reuse") { g.skipped++; continue; }
      g.n++;
      if (r.status === "fail" || r.status === "gone") g.failed++;
      if (r.t1 != null && r.t0 != null && !r.untimed) g.d.push(r.t1 - r.t0);
      g.last = { t0: r.t0, t1: r.t1, status: r.status, mac: r.mac };
    }
    const q = (a, p) => (a.length ? a[Math.min(a.length - 1, Math.floor(p * a.length))] : null);
    const names = [...by.values()].map((g) => {
      const s = [...g.d].sort((a, b) => a - b);
      return { name: g.name, n: g.n, failed: g.failed, skipped: g.skipped, p50: q(s, 0.5), p90: q(s, 0.9), max: s.length ? s[s.length - 1] : null, total: s.reduce((a, b) => a + b, 0), recent: g.d.slice(-40), last: g.last };
    });
    return { kind, from: has ? from : null, to: has ? to : null, names };
  }
  // GET /api/jobs/kinds: each kind of span recorded and how many.
  const kinds = () => ({ kinds: stmt(`SELECT kind, COUNT(*) AS n FROM spans GROUP BY kind ORDER BY kind`).all() });

  async function api(req, res, url, parts) {
    const [what, id] = parts; // after /api/jobs
    const m = req.method;
    if (what === "events" && m === "POST") return post(req, res);
    if (m !== "GET") return fail(res, 405, "invalid_argument", "method not allowed: POST /api/jobs/events records; the rest is read");
    if (what === "runs" && !id) return send(res, 200, runs(url));
    if (what === "runs" && id) return send(res, 200, run(id));
    if (what === "samples") return send(res, 200, samples(url));
    if (what === "stats") return send(res, 200, stats(url));
    if (what === "kinds") return send(res, 200, kinds());
    return fail(res, 404, "not_found", "no such route");
  }
  return { api };
}
