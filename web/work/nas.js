// The build page's NAS tab (docs/pool.md §10, Live; docs/pipeline.md §5), as the owner's final
// mockup lays it out (its "pool" tab; the owner's 04:33 ask: every stat a chart over time, as the
// Machines tab's): a row a measure, its name over its words, its chart beside, the same rows,
// widths and range (the footer's) as each Mac's live block (live.js): how long the lead's listing
// takes, the pool's NAS calls waiting (against their threads) and timed out, the NAS's disks busy
// (against the owner's target), then the pool's use against the limits the lead's probes give:
// data moved, files created, entries listed, files looked up, and the files opened. All from the
// lead's /work/live answer (live.js asks it while the tab shows; the pool's series are the lead's
// alone, task #517 and #634) and its status's `agent.nas` (the limits, the target, the mount). A
// measure missing is said so, never zero.
import { draw } from "./chart.js";
import { commonGaps, fmt, footer, latest, short, spanText } from "./chartmath.js";
import { liveOf, onLive, row, words } from "./live.js";

const MB = 1048576;
// The pool's series are the lead's coordinator's, every 10 s: a point stands for that much.
const POOL_EVERY = 10;
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const css = (k) => getComputedStyle(document.documentElement).getPropertyValue(k).trim();

// Built once and filled in place on every render and live answer.
const root = el("section", "d-nas");
const parts = {};
// The lead's status (dash.js's `agent`), for the repaints live answers bring.
let seen = null;

function build() {
  const box = el("div", "mc");
  const live = el("div", "live");
  const head = el("div", "lhead");
  parts.said = el("span", "dim");
  head.append(parts.said);
  parts.body = el("div", "lbody");
  const rows = el("div", "mac");
  const r = (name, tip, col) => row(rows, name, tip, col);
  parts.resp = r("response time", "How long the lead's listing of the project's folder on the NAS takes (every 5 s while a build runs on the lead). Over a few hundred ms, every job reading the NAS slows down.");
  parts.wait = r("requests waiting", "Every Mac's NAS calls in flight, summed. Dashed: their NAS call threads together; past it, calls queue.");
  parts.missed = r("timed out", "Every Mac's NAS calls past their deadline or refused (every thread busy), a minute.");
  parts.disks = r("NAS disks busy", "The busiest disk's busy share, as the lead's load gate reads it over SNMP. Dashed: the owner's target; past it the gate shuts and no new NAS work starts.");
  parts.bytes = r("data moved", "Bytes a second the pool's jobs read from and wrote to the NAS, stacked. Dashed: the most the lead's probes measured it can take (the best of the last day).");
  parts.creates = r("files created", "New files a second the pool's jobs make on the NAS. Dashed: its measured limit.");
  parts.listed = r("entries listed", "Folder entries a second the pool's jobs list on the NAS. Dashed: its measured limit.");
  parts.stats = r("files looked up", "Files a second the pool's jobs look up (stat) on the NAS. Dashed: its measured limit.");
  parts.opened = r("files opened", "Files a second the pool's jobs open to read from the NAS.");
  parts.body.append(rows);
  live.append(head, parts.body);
  box.append(live);
  root.append(box);
}
build();

/** The NAS tab's section: `agent` the lead's status. The same element each call, filled in place. */
export function nasSection(agent) {
  seen = agent;
  // (Drawn once it's on the page: a canvas sizes to its box.)
  queueMicrotask(paint);
  return root;
}

const tb = (b) => (b >= 1e12 ? `${fmt(b / 1e12)} TB` : `${fmt(b / 1e9)} GB`);

function paint() {
  if (!seen || !root.isConnected) return;
  const v = seen?.nas;
  const st = liveOf(seen?.host);
  if (!st?.series) {
    parts.body.hidden = true;
    parts.said.textContent = st?.error ? `no live measures from the lead: ${st.error}` : "asking the lead…";
    return;
  }
  parts.body.hidden = false;
  const s = st.series, now = st.now;
  const gl = commonGaps(["load1", "cores_busy", "mem_free_pct"].map((k) => ({ points: s[k], step: st.step })), now);
  const probe = v?.probes?.length ? Math.max(...v.probes.map((p) => p.at)) : null;
  const nowS = Math.floor(Date.now() / 1000);
  // (The capacity's age a tip, the mockup's: the owner deleted its line, 04:26.)
  parts.said.textContent = footer(st.range, st.step, gl);
  parts.said.title = probe ? `The limits: capacity measured ${spanText(Math.max(60, nowS - probe))} ago, the best of ${v.probes.length} probes over the last day` : "The lead hasn't probed the NAS yet: no limits";
  const base = { from: now - st.range, to: now, step: st.step, gaps: gl, axis: true, times: true };
  // (The pool's series a point every 10 s: their lines joined over that.)
  const pool = { ...base, step: Math.max(st.step, POOL_EVERY) };
  const acc = css("--accent"), nr = css("--nas-r"), nw = css("--nas-w"), warn = css("--warn"), bad = css("--bad");
  const at = (k, fresh = 30) => latest(s[k], now, fresh);

  // Response time: the lead's listing.
  const ms = at("nas_ms");
  const mt = v?.mount;
  draw(parts.resp.c, { ...base, unit: "ms", series: [{ points: s.nas_ms, colour: ms != null && ms > 200 ? warn : acc, label: "answer", band: true }] });
  words(parts.resp.v, [ms == null ? "no build on the lead now" : fmt(ms, "ms"), [mt?.name, mt?.proto, mt && `${tb(mt.free)} free`].filter(Boolean).join(" · ")],
    ms != null && ms > 1000 ? "bad" : ms != null && ms > 200 ? "warn" : "");

  // Requests waiting: the pool's calls in flight, against their threads.
  const calls = at("pool_nas_calls"), threads = at("pool_nas_threads");
  draw(parts.wait.c, { ...pool, unit: "", refs: threads ? [{ v: threads, label: `${fmt(threads)} threads` }] : [], series: [{ points: s.pool_nas_calls, colour: acc, label: "waiting", unit: "" }] });
  words(parts.wait.v, [calls == null ? "not measured" : threads ? `${fmt(calls)} of ${fmt(threads)}` : fmt(calls), "calls in flight, every Mac's"], calls != null && threads && calls >= threads ? "warn" : "");

  // Timed out: a minute's, and the range's count (a point's mean a second times what it stands for).
  const perMin = (s.pool_nas_missed || []).map((p) => [p[0], p[1] * 60, p[2] * 60, p[3] * 60]);
  const span = Math.max(st.step, POOL_EVERY);
  const count = s.pool_nas_missed ? Math.round(s.pool_nas_missed.reduce((t, p) => t + p[2] * span, 0)) : null;
  draw(parts.missed.c, { ...pool, unit: "/min", series: [{ points: perMin, colour: bad, fill: true, label: "timed out", unit: "/min" }] });
  words(parts.missed.v, [count == null ? "not measured" : `${count} in ${spanText(st.range)}`, count == null ? "" : `${fmt((at("pool_nas_missed") || 0) * 60)} a minute now`], count ? "bad" : "");

  // The disks: the busiest one's share against the owner's target.
  const target = v?.target_pct, d = at("nas_disk_busy_pct", 60);
  draw(parts.disks.c, { ...pool, unit: "%", max: 100, refs: target ? [{ v: target, label: `${target}% target` }] : [], series: [{ points: s.nas_disk_busy_pct, colour: warn, fill: true, label: "busiest disk", unit: "%" }] });
  words(parts.disks.v, [d == null ? "not read" : fmt(d, "%"), d == null ? "no SNMP reading" : v?.held ? "the gate is shut" : "busiest disk"], v?.held ? "bad" : target && d != null && d > target ? "warn" : "");

  // Use against the limits: each with its limit dashed and its share of it.
  const l = v?.limits;
  const limited = (r, series, limit, unit, extra = []) => {
    const f = (x) => (unit === "MB/s" ? fmt(x, "MB/s") : `${x >= 1000 ? short(x) : fmt(x)}/s`);
    const used = series.reduce((t, k) => { const x = at(k); return x == null ? t : (t || 0) + x; }, null);
    const stacked = series.length > 1;
    draw(r.c, { ...pool, unit: unit === "MB/s" ? "MB/s" : "/s", stacked, refs: limit ? [{ v: limit, label: `limit ${f(limit)}` }] : [],
      series: series.map((k, i) => ({ points: s[k], colour: stacked ? [nr, nw][i] : acc, fill: !stacked, label: extra[i]?.[1] || "", unit: unit === "MB/s" ? "MB/s" : "/s" })) });
    words(r.v, [used == null ? "not measured" : f(used), ...extra.map(([col, w, k]) => [col, `${w} ${f(at(k))}`]),
      !l ? "not probed" : !limit ? "no limit weighed" : used == null ? `limit ${f(limit)}` : `limit ${f(limit)} · ${Math.round((used / limit) * 100)} % of it`],
      limit && used != null && used / limit > 0.85 ? "warn" : "");
  };
  limited(parts.bytes, ["pool_nas_read_mbs", "pool_nas_write_mbs"], l?.bytes_s / MB, "MB/s", [[nr, "read", "pool_nas_read_mbs"], [nw, "written", "pool_nas_write_mbs"]]);
  limited(parts.creates, ["pool_nas_creates_s"], l?.creates_s, "/s");
  limited(parts.listed, ["pool_nas_listed_s"], l?.listed_s, "/s");
  limited(parts.stats, ["pool_nas_stats_s"], l?.stats_s, "/s");
  const op = at("pool_nas_files_s");
  draw(parts.opened.c, { ...pool, unit: "/s", series: [{ points: s.pool_nas_files_s, colour: nr, fill: true, label: "opened", unit: "/s" }] });
  words(parts.opened.v, [op == null ? "not measured" : `${fmt(op)}/s`]);
}

// (Only the lead's answers feed this section: a frame's repaint for each.)
let queued = false;
onLive((host) => { if (host !== seen?.host || queued) return; queued = true; requestAnimationFrame(() => { queued = false; paint(); }); });
