// The NAS on the build page's Machines tab (docs/pool.md §10, Live; docs/pipeline.md §5), as the
// owner's mockup lays it out (docs/mockups/build-live.html, "The NAS"): how fast it responds (the
// lead's listing time over the chart's range, then gauges: response time, requests waiting, those
// timed out over the last day, the disks' busy share; chips), its use against its capacity (what the
// pool moves now against the limits its probes give), who is using it (each Mac's running jobs'
// share of the budget, the rest free; what one job of each step running weighs), and the pool's NAS
// traffic, file work and disks over time. From the lead's status (`agent.nas`, `nas_share`,
// `nas_step`) and the Macs' /work/live answers, which live.js asks while the tab shows; each Mac's
// own NAS response stays in its Live block (live.js). A measure missing is said so, never zero.
import { draw } from "./chart.js";
import { commonGaps, fmt, footer, latest, recentMean, spanText } from "./chartmath.js";
import { liveOf, onLive } from "./live.js";
import { isSlotOf } from "./sums.js";

const MB = 1048576;
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const css = (k) => getComputedStyle(document.documentElement).getPropertyValue(k).trim();
const clamp01 = (x) => Math.max(0, Math.min(1, x));

// Built once and filled in place on every render and live answer: the section, its charts, gauges.
const root = el("section", "d-nas");
const parts = {};
// What the last render said: {agent, macs} (dash.js's model), for the repaints live answers bring.
let seen = null;

// A chart row (live.js's, the mockup's .rate): its name over its words, its chart beside.
function chartRow(name, tall) {
  const r = { row: el("div", `rate ${tall ? "" : "vrow"}`), c: el("canvas"), v: el("span", "v") };
  const lab = el("div", "lab"), cw = el("div", "cw");
  lab.append(el("span", "k", name), r.v);
  cw.append(r.c);
  r.row.append(lab, cw);
  return r;
}

// A gauge: its name, a bar over its scale's ticks, its words (dash.css .cap, .barcell).
function gauge(label, title) {
  const row = el("div", "cap");
  const bar = el("div", "cbar"), i = el("i"), scale = el("div", "scale"), v = el("span", "v");
  bar.append(i);
  const cell = el("div", "barcell");
  cell.append(bar, scale);
  row.append(el("span", "k", label), cell, v);
  if (title) row.title = title;
  return { row, bar, i, scale, v };
}
// Its fill (`used` of `whole`), class, words and ticks (re-set only when they change).
function set(g, used, whole, text, cls = "", ticks = null) {
  g.i.style.width = `${Math.round(clamp01(whole ? used / whole : 0) * 100)}%`;
  g.bar.className = `cbar ${cls}`;
  g.v.textContent = text;
  const t = (ticks || []).join("|");
  if (g.scale.dataset.t !== t) { g.scale.dataset.t = t; g.scale.replaceChildren(...(ticks || []).map((x) => el("span", null, x))); }
}

function build() {
  const box = el("div", "mc");
  const grid = el("div", "nasgrid");
  const left = el("div"), right = el("div");
  const h3 = (text, sub) => { const h = el("h3", null, text); if (sub) h.append(sub); return h; };
  parts.say = el("span");
  parts.lat = chartRow("Answer");
  parts.latHead = el("span", null, "the lead's listing of the project's folder");
  const gs = el("div", "caps");
  parts.resp = gauge("response time", "How long the lead's listing of the project's folder took (every 5 s while a build runs on it)");
  parts.wait = gauge("requests waiting", "Every Mac's NAS calls in flight, against their NAS call threads together");
  parts.missed = gauge("timed out", "NAS calls past their deadline or refused (every thread busy), every answering Mac's, over the last day");
  parts.disks = gauge("NAS disks busy", "The busiest disk's busy share, smoothed, against the owner's target (the gate shuts past it)");
  gs.append(parts.resp.row, parts.wait.row, parts.missed.row, parts.disks.row);
  parts.chips = el("div", "chips");
  const latBody = parts.latBody = el("div", "lbody");
  latBody.append(parts.lat.row);
  left.append(h3("How fast the NAS responds", parts.latHead), latBody, gs, parts.chips);
  parts.capSub = el("span");
  const lims = el("div", "caps");
  parts.lims = [["bytes", "data moved"], ["creates", "files created"], ["listed", "entries listed"], ["stats", "files looked up"]].map(([k, label]) => {
    const g = gauge(label); lims.append(g.row); return [k, g];
  });
  parts.shares = el("div", "caps shares");
  parts.weights = el("div", "small dim");
  right.append(h3("Use against its capacity", parts.capSub), lims,
    h3("Who is using the NAS", el("span", null, "the share each Mac's running jobs are allowed, set by the lead from each job's measured use")), parts.shares, parts.weights);
  grid.append(left, right);
  // Over time: the pool's traffic, its file work, the disks.
  const live = el("div", "live");
  parts.liveSaid = el("span", "dim");
  const head = el("div", "lhead");
  head.append(el("b", null, "The pool's NAS use over time"), parts.liveSaid);
  parts.body = el("div", "lbody");
  parts.io = chartRow("Traffic", true); parts.files = chartRow("Files"); parts.disk = chartRow("Disks");
  parts.body.append(parts.io.row, parts.files.row, parts.disk.row);
  live.append(head, parts.body);
  box.append(grid, live);
  const h2 = el("h2", null, "The NAS");
  h2.append(parts.say);
  box.prepend(h2);
  root.append(box);
}
build();

/** The NAS section for the Machines tab: `agent` the lead's status, `macs` the page's Macs (name,
 * fresh). The same element each call, filled in place. */
export function nasSection(agent, macs) {
  seen = { agent, macs };
  fill();
  return root;
}

const tb = (b) => (b >= 1e12 ? `${fmt(b / 1e12)} TB` : `${fmt(b / 1e9)} GB`);

// What the status says: the heading, the limits, the shares and the steps' weights.
function fill() {
  const { agent, macs } = seen;
  const v = agent?.nas;
  const l = v?.limits, t = v?.traffic;
  const lim = (g, used, limit, unit) => {
    const f = (x) => (unit === "MB/s" ? fmt(x, "MB/s") : `${fmt(x)}/s`);
    if (!l) set(g, 0, 0, "not probed");
    else if (!limit) set(g, 0, 0, "no limit weighed");
    else if (used == null) set(g, 0, limit, "not measured", "ok", ["0", `${f(limit)} limit`]);
    else set(g, used, limit, f(used), used / limit > 0.85 ? "warn" : "ok", ["0", `${f(limit)} limit`]);
  };
  const [[, gb], [, gc], [, gl], [, gs]] = parts.lims;
  // (The traffic in crate::agent::live::NAS_TRAFFIC's order: read MB/s, files, written MB/s, creates, listed, stats, deletes.)
  lim(gb, t ? t[0] + t[2] : null, l?.bytes_s / MB, "MB/s");
  lim(gc, t?.[3], l?.creates_s);
  lim(gl, t?.[4], l?.listed_s);
  lim(gs, t?.[5], l?.stats_s);
  const n = v?.probes?.length || 0;
  parts.capSub.textContent = l ? `the best of the probes kept: ${n} over the last day` : "the lead hasn't probed the NAS yet";
  // Who is using it: each Mac's jobs' weights (thousandths of the whole), then what's free.
  const shares = agent?.nas_share || {};
  const rows = [];
  if (Object.keys(shares).length) {
    let used = 0;
    for (const m of macs) {
      const w = Object.entries(shares).filter(([nm]) => isSlotOf(nm, m.name)).reduce((x, [, y]) => x + y, 0);
      used += w;
      rows.push(shareRow(m.name, w, w > 1000 ? "warn" : ""));
    }
    rows.push(shareRow("free", Math.max(0, 1000 - used), "free"));
  }
  parts.shares.replaceChildren(...rows);
  parts.shares.hidden = !rows.length;
  // What one job of each step running weighs.
  const st = Object.entries(agent?.nas_step || {}).sort((a, b) => b[1] - a[1]);
  parts.weights.textContent = !rows.length ? "No shares: the NAS isn't probed yet, or no job runs." : st.length
    ? `One job allowed: ${st.map(([s, w]) => `${s} ${pct(w)}`).join(", ")}.`
    : "No job is running.";
  paint();
}

const pct = (w) => `${fmt(w / 10)} %`;
function shareRow(name, w, cls) {
  const row = el("div", "cap");
  const bar = el("div", `cbar ${cls}`), i = el("i");
  i.style.width = `${Math.round(clamp01(w / 1000) * 100)}%`;
  bar.append(i);
  row.append(el("span", `k${cls === "free" ? " dim" : ""}`, name), bar, el("span", "v", pct(w)));
  return row;
}

// What the live answers say: the heading's words, the gauges, the chips, the charts.
function paint() {
  if (!seen || !root.isConnected) return;
  const { agent, macs } = seen;
  const v = agent?.nas;
  const lead = liveOf(agent?.host);
  const ms = lead?.series ? latest(lead.series.nas_ms, lead.now, 30) : null;
  const mt = v?.mount;
  parts.say.textContent = [mt && mt.name, mt && mt.proto, mt && `${tb(mt.free)} free`, ms != null && fmt(ms, "ms")].filter(Boolean).join(" · ")
    || "how fast it answers, how close to its limits, and who is using it";
  set(parts.resp, ms ?? 0, 400, ms == null ? "not measured" : fmt(ms, "ms"), ms != null && ms > 200 ? "warn" : "", ["0", "200 ms", "400 ms"]);
  let wait = null, threads = 0, day = null, answering = 0;
  for (const m of macs) {
    const st = liveOf(m.name);
    if (!st?.series) continue;
    answering++;
    threads += st.mac?.nas_threads || 0;
    const w = latest(st.series.nas_calls, st.now, 30);
    if (w != null) wait = (wait || 0) + w;
    if (st.day != null) day = (day || 0) + st.day;
  }
  set(parts.wait, wait ?? 0, threads, wait == null ? "not measured" : threads ? `${fmt(wait)} of ${threads}` : fmt(wait), wait && threads && wait >= threads ? "warn" : "",
    threads ? ["0", `${threads / 2}`, `${threads} threads`] : null);
  const dc = day == null ? null : Math.round(day);
  set(parts.missed, dc ?? 0, 10, dc == null ? "not measured" : `${dc} in 24 h`, "bad", ["0", "5", "10 a day"]);
  const d = v?.load_pct;
  set(parts.disks, d ?? 0, 100, d == null ? "not read" : `${Math.round(d)} %`, v?.held ? "bad" : d != null && d > v.target_pct ? "warn" : "", ["0", "50 %", "100 %"]);
  parts.disks.row.title = `The busiest disk's busy share, smoothed, against the owner's target${v ? ` (${v.target_pct} %)` : ""}: past it the gate shuts`;
  const probe = v?.probes?.length ? Math.max(...v.probes.map((p) => p.at)) : null;
  const nowS = Math.floor(Date.now() / 1000);
  const fresh = macs.filter((m) => m.fresh).length;
  const chips = [[probe ? `capacity measured ${spanText(Math.max(60, nowS - probe))} ago` : "capacity not measured", ""],
    [v?.held ? "sharing limits on: the gate is shut" : v?.limits ? "sharing limits on" : "no limits yet", v?.held ? "warn" : v?.limits ? "ok" : ""],
    [`${fresh} ${fresh === 1 ? "Mac" : "Macs"} connected`, ""],
    [dc == null ? "timeouts not measured" : dc ? `${dc} requests timed out in 24 h${answering < macs.length ? " (of the Macs answering)" : ""}` : "no requests timed out in 24 h", dc ? "warn" : dc === 0 ? "ok" : ""]];
  parts.chips.replaceChildren(...chips.map(([t, c]) => el("span", `chip ${c}`, t)));
  charts(lead);
}

function charts(st) {
  const ok = !!st?.series;
  parts.liveSaid.textContent = ok ? "" : st?.error ? `no live measures from the lead: ${st.error}` : "asking the lead…";
  parts.body.hidden = !ok;
  parts.latBody.hidden = !ok;
  if (!ok) return;
  const nr = css("--nas-r"), nw = css("--nas-w"), nd = css("--net-d"), warn = css("--warn");
  const s = st.series, now = st.now;
  const gl = commonGaps(["load1", "cores_busy", "mem_free_pct"].map((k) => ({ points: s[k], step: st.step })), now);
  parts.liveSaid.textContent = footer(st.range, st.step, gl);
  const base = { from: now - st.range, to: now, step: st.step, gaps: gl };
  const words = (r, text, cls = "", title = "") => { r.v.textContent = text; r.v.className = `v ${cls}`; r.v.title = title; };
  const lm = latest(s.nas_ms, now, 30);
  draw(parts.lat.c, { ...base, unit: "ms", series: [{ points: s.nas_ms, colour: lm != null && lm > 200 ? warn : css("--accent"), label: "answer", band: true }] });
  words(parts.lat, lm == null ? "no build on the lead now" : `${fmt(lm, "ms")} a listing`, lm != null && lm > 1000 ? "bad" : lm != null && lm > 200 ? "warn" : "");
  draw(parts.io.c, { ...base, axis: true, unit: "MB/s", series: [
    { points: s.pool_nas_read_mbs, colour: nr, label: "read" }, { points: s.pool_nas_write_mbs, colour: nw, label: "written" }] });
  parts.io.v.replaceChildren(el("div", "dim", "last min"), ...[["↓", "pool_nas_read_mbs", nr], ["↑", "pool_nas_write_mbs", nw]].map(([t, k, col]) => {
    const d = el("div");
    const i = el("i", "sw"); i.style.background = col;
    d.append(i, `${t} ${fmt(recentMean(s[k], now), "MB/s")}`);
    return d;
  }));
  parts.io.v.className = "v legend";
  // (Entries a second, per kind: opened to read, made, listed; a listing's entries dwarf the rest.)
  draw(parts.files.c, { ...base, unit: "/s", series: [
    { points: s.pool_nas_files_s, colour: nr, label: "opened" }, { points: s.pool_nas_creates_s, colour: nw, label: "made" }, { points: s.pool_nas_listed_s, colour: nd, label: "listed" }] });
  words(parts.files, `${fmt(recentMean(s.pool_nas_files_s, now))} opened · ${fmt(recentMean(s.pool_nas_creates_s, now))} made · ${fmt(recentMean(s.pool_nas_listed_s, now))} listed /s`, "", "The pool's jobs' files a second over the last minute: opened to read, made, entries listed");
  const target = seen.agent?.nas?.target_pct;
  draw(parts.disk.c, { ...base, unit: "%", max: 100, refs: target ? [{ v: target, label: `${target}% target` }] : [], series: [{ points: s.nas_disk_busy_pct, colour: warn, fill: true, label: "busiest disk", unit: "%" }] });
  const d = latest(s.nas_disk_busy_pct, now, 60);
  words(parts.disk, d == null ? "not read (no SNMP reading)" : `${fmt(d, "%")} busiest disk`, target && d != null && d > target ? "warn" : "");
}

// (Only the lead's answer and the Macs' calls feed this section: a frame's repaint for each.)
let queued = false;
onLive(() => { if (!queued) { queued = true; requestAnimationFrame(() => { queued = false; paint(); }); } });
