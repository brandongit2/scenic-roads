// A job's page (task #1105; docs/pipeline.md §5, A job's page), at the build page's job/<id>: one
// job run as the lead keeps it (`GET /api/job/<id>`, docs/formats.md, Job spans).
//   Left, fixed: what it is (step, targets, Mac, job id and lease, app), what it held and used
//   (threads granted, cores busy, peak memory, its NAS weight), when and how it ended and why, its
//   figures beside its step's medians (`/api/timings`), the other runs of its targets and the jobs
//   beside it on its Mac (the history's: `runs`, `beside`).
//   Main: its spans on a timeline (timeline.js, the schedule's; laid out by runlanes.js, the dev
//   job dashboard's): the run, its main thread's phases by depth, each other thread's work, its
//   waits as intervals, its markers (leases and take-overs, retries, safe points, pauses, the end),
//   its log's lines; under them its Mac's CPU, memory, NAS and network (that Mac's `/api/live`)
//   and the other jobs on it meanwhile. A span clicked: its log lines under the timeline.
//   Times as T+ since its start, or the device's clock (the footer's button, kept in this browser).
//   A job still running: asked again every 5 s, its spans coming with its Mac's beats.
//   A job from before the spans (`legacy`): said, the details as far as the history has them.
import { createTimeline } from "./timeline.js";
import { h, token } from "./dom.js";
import { n, plural } from "./words.js";
import { lineage, pack, spanLanes, spanTree } from "./runlanes.js";
import { versionWatch } from "./version.js";
import { remote } from "./live.js";
import { MARK_LANES, dur, linesIn, markers, outcome, recordFigures, spanLook, stepMedians, timeWords, clockText } from "./jobview.js";

const $ = (id) => document.getElementById(id);
// (The build page's folder: "" at the coordinator's root, "/build" through tools/caddy.)
const ROOT = location.pathname.replace(/\/job\/[^/]*$/, "");
const ID = decodeURIComponent(location.pathname.split("/").pop() || "");
const POLL_MS = 5000;
const LIVE_MS = 15000;
const LOG_SHOWN = 4000;
// The Mac's measures (its /api/live, docs/pool.md §10, Live) drawn under the job, and their colours.
const SERIES = ["build_cores", "others_cores", "build_mem_gb", "others_mem_gb", "nas_read_mbs", "nas_write_mbs", "net_in_mbs", "net_out_mbs"];
const LIVE_KEPT_S = 7 * 86400;
// What a span or a wait waited on (sums.js WAIT_PARTS' colours, dash.js WAIT_COLOURS'), and the CPU.
const LOOK = { nas: "#3d6fa8", net: "#7a5fb0", disk: "#2f8a8a", other: "#6b6f76", cpu: "#a8823d", old: "#4a5260" };
const LOOK_SAID = { nas: "the NAS", net: "the network", disk: "the local disk", other: "other work", cpu: "computing", old: "unsaid" };
const MARK_COLOUR = { lease: "var(--accent)", "held again": "var(--warn)", start: "var(--accent)", retry: "var(--warn)", safe: "var(--ok)", pausing: "var(--warn)", paused: "var(--warn)", error: "var(--bad)", thinned: "var(--dim)" };

const store = {
  get(k, d) { try { const x = localStorage.getItem(k); return x === null ? d : JSON.parse(x); } catch { return d; } },
  set(k, x) { try { localStorage.setItem(k, JSON.stringify(x)); } catch { /* (a private window: not kept) */ } },
};
const watch = versionWatch(document.querySelector('meta[name="page-version"]')?.content, () => location.reload(), {
  get: () => { try { return sessionStorage.getItem("page-version-tried"); } catch { return null; } },
  set: (x) => { try { sessionStorage.setItem("page-version-tried", x); } catch { /* (not kept) */ } },
});
async function ask(path, body) {
  const r = await fetch(ROOT + path, body === undefined ? { cache: "no-store" } : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  watch(r.headers);
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw Object.assign(new Error(j?.error || `answered ${r.status}`), { status: r.status });
  return j;
}

// ---- state ------------------------------------------------------------------------------------------
let v = null; // the job's view
let steps = null; // /api/steps: the stages' colours
let meds = null; // its step's medians (stepMedians), or {error}
let mac = null; // {host, ans} | {host, error}
let swarm = null;
let mode = store.get("job-times", "rel") === "clock" ? "clock" : "rel";
let tw = null; // timeWords for the view now
let tl = null, tlDomain = null;
let sel = null; // the span clicked: its key
let logAll = false;
let heard = 0, failing = null;

const busy = (on) => { $("busy").hidden = !on; };
const hostOf = (w) => (w || "").replace(/ #\d+$/, "");
const stageOf = (step) => steps?.steps?.find((s) => s.id === step)?.stage;
const stepColour = (step) => `var(--st-${stageOf(step) || "upkeep"}, var(--st-upkeep))`;
const mb = (x) => (x == null ? "–" : x >= 1024 ? `${(x / 1024).toFixed(x >= 10240 ? 0 : 1)} GB` : `${Math.round(x)} MB`);
const bytes = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : b >= 1e6 ? `${(b / 1e6).toFixed(1)} MB` : b >= 1e3 ? `${Math.round(b / 1e3)} kB` : `${b} B`);
const fx = (x, d = 1) => (x == null ? "–" : x.toFixed(d));

// The job's own facts, as far as its view or its history has them (a legacy job's: its events').
function facts() {
  const hist = v.history || [];
  const first = (k) => hist.find((e) => e.kind === k);
  const lease = first("lease"), start = first("start");
  const end = [...hist].reverse().find((e) => ["end", "done", "fail", "cancel"].includes(e.kind));
  const step = v.step || end?.step || start?.step || lease?.step || v.record?.kind || "";
  const targets = v.targets?.length ? v.targets : (end || start || lease)?.targets || [];
  const worker = v.worker || start?.worker || lease?.worker || end?.worker || "";
  const t0 = v.start ?? (end?.secs != null ? end.t - end.secs : start?.t ?? lease?.t ?? v.record?.start ?? null);
  const t1 = v.legacy ? end?.t ?? null : v.end;
  return { step, targets, worker, host: v.host || hostOf(worker), t0, t1, lease, end, leases: hist.filter((e) => e.kind === "lease").length };
}

// ---- the sidebar -------------------------------------------------------------------------------------
const kv = (rows) => h("div", "kv", rows.filter((r) => r && r[1] != null && r[1] !== "").flatMap(([k, x, title]) => [h("span", { class: "k", title: title || null }, k), h("span", { title: title || null }, x)]));
const sec = (title, ...kids) => h("section", "ss", h("h3", null, title), ...kids);
function side(f, out) {
  const [epoch, num] = ID.split("-");
  const fig = recordFigures(v.record);
  const end = f.t1 ?? (v.running ? v.now : null);
  const chips = (t) => h("div", "chips", (t.length > 30 ? [...t.slice(0, 30), `and ${n(t.length - 30)} more`] : t).map((x) => h("span", null, x)));
  const vs = (x, m, fmt, title) => [h("span", null, fmt(x)), h("span", { class: "dim", title }, m == null ? "–" : fmt(m)), h("span", "dim", x != null && m ? `×${(x / m).toFixed(x / m >= 10 ? 0 : 1)}` : "")];
  const medRows = meds && !meds.error && meds.n ? [
    ["wall", fig?.wall ?? (f.t1 != null && f.t0 != null ? f.t1 - f.t0 : null), meds.wall, dur],
    ["CPU", fig?.cpu, meds.cpu, dur],
    ["cores busy", fig?.cores, meds.cores, (x) => fx(x)],
    ["peak memory", fig?.peak, meds.peak, mb],
    ["threads", fig?.threads, meds.threads, (x) => (x == null ? "–" : String(Math.round(x)))],
    ["NAS wait", fig?.nas, meds.nas, dur],
    ["network wait", fig?.net, meds.net, dur],
  ] : null;
  const runs = v.runs || [], beside = v.beside || [];
  const runRow = (r) => h("li", null, h("a", { href: encodeURIComponent(r.job), title: `job ${r.job} on ${r.worker}` }, clockText(r.from, { date: true }).replace(/:\d\d$/, "")),
    h("span", r.until == null ? "run" : r.ok === false ? "bad" : "dim", r.until == null ? "running" : `${dur(r.until - r.from)}${r.ok === false ? " failed" : ""}`), h("span", "dim", hostOf(r.worker)));
  $("side").replaceChildren(
    sec("Job", kv([
      ["step", f.step ? h("span", null, h("i", { class: "sw", style: { background: stepColour(f.step) } }), f.step, stageOf(f.step) ? h("span", "dim", ` · ${stageOf(f.step)}`) : null) : "–"],
      ["name", v.name || null],
      ["targets", f.targets.length ? `${n(f.targets.length)}` : "none"],
    ]), f.targets.length ? chips(f.targets) : null,
    kv([
      ["Mac", f.host ? `${f.host}${f.worker && f.worker !== f.host ? ` (${f.worker.slice(f.host.length + 1)})` : ""}` : "–"],
      ["job id", ID],
      ["lease", num ? `${num} of epoch ${epoch}${f.leases > 1 ? `; held again ${plural(f.leases - 1, "time")}` : ""}` : null, "The run's id is the lease it was first granted under (docs/pool.md §7.5): a lapse, a lead's restart or a take-over hold it again under a new one"],
      ["app", v.app || (v.legacy ? "not recorded" : "not recorded (its agent's app is older)")],
    ])),
    sec("What it held and used", kv([
      ["threads", fig?.threads ?? f.end?.threads ?? null, "Granted: RAYON_NUM_THREADS, the cores its grant held"],
      ["cores busy", fig?.cores != null ? `${fx(fig.cores)} (${dur(fig.cpu)} CPU)` : v.running ? "when it ends (its record)" : "no record", "Its CPU over its wall time, its children's too"],
      ["peak memory", fig?.peak != null ? mb(fig.peak) : v.running ? "when it ends" : "no record", "The most its process group held in any phase (physical footprints, every 250 ms)"],
      ["NAS weight", "not recorded", "The lead weighs a running job's NAS share from its beats (coord::sched job_weight); nothing keeps the weight it was granted under"],
    ])),
    sec("When and how", kv([
      ["started", f.t0 != null ? clockText(f.t0, { date: true }) : "–"],
      ["took", f.t0 != null && end != null ? `${dur(end - f.t0)}${v.running ? " so far" : ""}` : "–"],
      ["outcome", h("span", out.state === "failed" ? "bad" : out.state === "running" ? "run" : out.state === "ok" ? "ok" : "dim", out.said)],
    ]), out.why.length ? h("ul", out.state === "failed" ? "why" : "why said", out.why.map((w) => h("li", null, w))) : null),
    sec(`Beside its step's medians${meds?.n ? ` (${plural(meds.n, "run")})` : ""}`, medRows
      ? h("div", "med", h("span", "dim", ""), h("span", "dim", "this"), h("span", "dim", "median"), h("span", "dim", ""), medRows.flatMap(([k, x, m, fmt]) => [h("span", "k", k), ...vs(x, m, fmt)]))
      : h("div", "dim small", meds?.error ? `Its step's records can't be read: ${meds.error}` : meds ? "No other run of its step in the lead's records." : "Reading its step's records…"),
      h("div", "dim small", "Its step's last 20 records on the lead (agent/coord/timings.jsonl), this one left out.")),
    sec(`Other runs of its targets${v.runs_total > runs.length ? ` (the newest ${runs.length} of ${n(v.runs_total)})` : runs.length ? ` (${runs.length})` : ""}`,
      runs.length ? h("ul", "runs", runs.map(runRow)) : h("div", "dim small", "None in the history's seven days.")),
    sec(`Beside it on ${f.host || "its Mac"}${beside.length ? ` (${beside.length})` : ""}`,
      beside.length ? h("ul", "runs", beside.slice(0, 40).map((r) => h("li", null, h("a", { href: encodeURIComponent(r.job) }, r.step), h("span", "dim", r.targets.join(" ") + (r.n > r.targets.length ? ` +${r.n - r.targets.length}` : "")), h("span", "dim", r.until == null ? "running" : dur(r.until - r.from))))) : h("div", "dim small", "No other job on its Mac while it ran, as the history has it.")),
  );
}

// ---- the card under the pointer ------------------------------------------------------------------------
const tip = h("div", "card-tip");
tip.hidden = true;
document.body.append(tip);
const cards = new Map();
function showTip(rows, x, y) {
  tip.replaceChildren(...rows);
  tip.hidden = false;
  const r = tip.getBoundingClientRect(), W = innerWidth, H = innerHeight;
  tip.style.left = `${x + 14 + r.width <= W - 8 ? x + 14 : Math.max(8, x - 14 - r.width)}px`;
  tip.style.top = `${y + 16 + r.height <= H - 8 ? y + 16 : Math.max(8, y - 10 - r.height)}px`;
}
const hideTip = () => { tip.hidden = true; };
const card = (title, rows) => [h("div", "ct", title), h("div", "ckv", rows.filter((r) => r && r[1] != null && r[1] !== "").flatMap(([k, x]) => [h("span", "k", k), h("span", null, x)]))];
const when = (t0, t1) => (t1 == null || Math.abs(t1 - t0) < 5e-4 ? tw.at(t0) : `${tw.at(t0)} – ${tw.at(t1)} (${dur(t1 - t0)})`);

// ---- the timeline ------------------------------------------------------------------------------------
let segs = new Map(), tree = null, charts = [];
function relations(key) {
  const s = segs.get(key);
  if (!s) return null;
  if (key.startsWith("s ") && tree) return { on: [...lineage(tree, key.slice(2))].map((id) => segs.get(`s ${id}`)).filter(Boolean), arrows: [] };
  return { on: [s], arrows: [] };
}
function timeline(f) {
  const now = v.now, t0 = f.t0, t1 = f.t1 ?? now;
  const marks = markers(v);
  const lo0 = Math.min(t0, ...marks.map((m) => m.t)), len = Math.max(0.01, t1 - lo0);
  const pad = Math.max(0.005, len * 0.015);
  const lo = lo0 - pad, hi = t1 + pad;
  if (!tl) {
    tl = createTimeline({ relations, minSpan: 0.002, span0: hi - lo, ticks: (a, b, m) => tw.ticks(a, b, m), archived: false, onRepack: () => render() });
    tl.el.addEventListener("pointermove", (e) => pointAt(e));
    tl.el.addEventListener("pointerleave", hideTip);
    tl.el.addEventListener("click", (e) => clickAt(e));
  }
  // (Showing the whole run as it grows: kept whole while it's whole; a view zoomed in stays.)
  const was = tlDomain && tl.range(tlDomain.now, tlDomain.lo, tlDomain.hi);
  const whole = !was || (was.t0 <= tlDomain.lo + 1e-6 && was.t1 >= tlDomain.hi - 1e-6);
  tlDomain = { now, lo, hi };
  if (whole) tl.show(lo, hi, now);
  const { span } = tl.range(now, lo, hi);
  const width = Math.max(240, tl.axis.offsetWidth || $("jmain").clientWidth - 140);
  const gap = (span / width) * 2;
  segs = new Map(); cards.clear(); charts = [];
  const lanes = h("div", "lanes"), tracks = [];
  const head = (title, ...right) => lanes.append(h("div", "gh", title), h("div", "ghr", ...right));
  const row = (label, t, title = label) => { tracks.push(t); lanes.append(h("div", { class: "ln", title }, label), t.el); return t; };
  const seg = (t, key, o, build) => { const s = t.add({ key, ...o }); segs.set(key, s); cards.set(key, build); return s; };

  // The run and its spans: its main thread's phases by depth, then each other thread's.
  tree = spanTree(v.spans || [], "job");
  const root = (v.spans || []).find((s) => s.id === "job");
  head("Job", h("span", "small dim", `${plural((v.spans || []).length - 1, "span")}${v.running ? " · running" : ""}`));
  const top = row("the run", tl.track());
  if (root) seg(top, "s job", { from: root.t0, until: root.t1 ?? now, colour: stepColour(f.step), kind: root.status === "running" ? "running" : "past", failed: root.status === "fail" }, () => card(v.name || f.step, [["status", root.status], ["when", when(root.t0, root.t1 ?? now)], ["spans", n((v.spans || []).length - 1)]]));
  const spanCard = (s) => () => {
    const a = s.attrs || {}, p = tree.byId.get(s.parent);
    return card(s.name, [
      ["in", p && p.id !== "job" ? p.name : null], ["on", a.th ? `thread ${a.th}` : "the main thread"], ["status", s.status],
      ["when", when(s.t0, s.t1 ?? now)], ["merged", a.n > 1 ? `${n(a.n)} spans, ${dur(a.busy_s)} busy` : null],
      ["CPU", a.cpu_s != null ? `${dur(a.cpu_s)}${s.t1 > s.t0 ? ` (${fx(a.cpu_s / (s.t1 - s.t0))} cores)` : ""}` : null],
      ["NAS", a.nas_s ? dur(a.nas_s) : null], ["network", a.net_s ? dur(a.net_s) : null], ["disk", a.disk_s ? dur(a.disk_s) : null], ["other work", a.other_s ? dur(a.other_s) : null],
      ["bytes", a.bytes ? bytes(a.bytes) : null], ["files", a.files ? n(a.files) : null], ["waited most on", LOOK_SAID[spanLook(s)]],
      ["log", `${plural(linesIn(v.log?.lines, s.t0, s.t1 ?? now).length, "line")} (click)`],
    ]);
  };
  const thread = (s) => (s.kind === "beside" ? `thread ${s.attrs?.th ?? "?"}` : "main");
  for (const r of spanLanes(tl, tree, {
    gap, now, sep: 0, outer: "group", order: ["main"], group: thread,
    label: (g, d, k) => (d === 1 ? `${g} (${k})` : `${"· ".repeat(d - 1)}inside (${k})`),
    add: (t, s, short) => seg(t, `s ${s.id}`, { from: s.t0, until: s.t1 ?? now, colour: LOOK[spanLook(s)], kind: s.status === "running" ? "running" : short ? "mark" : "past", failed: s.status === "cut" }, spanCard(s)),
  })) row(r.label, r.track, r.title);

  // Its waits, a lane a kind.
  const waits = v.waits || [];
  if (waits.length) {
    head("Waits", h("span", "small dim", "intervals any thread waited (merged within a gap)"));
    for (const on of ["nas", "net", "disk", "other"]) {
      const mine = waits.filter((w) => w.on === on);
      if (!mine.length) continue;
      const t = row(LOOK_SAID[on], tl.track());
      mine.forEach((w, i) => seg(t, `w ${on} ${i}`, { from: w.t0, until: w.t1, colour: LOOK[on], kind: "past" }, () => card(`Waiting on ${LOOK_SAID[on]}`, [["when", when(w.t0, w.t1)], ["waits", n(w.n)], ["waited", dur(w.busy_s)]])));
    }
  }

  // Its markers: leases and take-overs, retries, safe points, pauses, the end.
  if (marks.length) {
    head("Events", h("span", "small dim", "the job's and the history's"));
    for (const lane of MARK_LANES) {
      const mine = marks.filter((m) => m.lane === lane);
      if (!mine.length) continue;
      const t = row(lane, tl.track());
      mine.forEach((m, i) => seg(t, `m ${lane} ${i}`, { from: m.t, until: m.t1 ?? m.t, colour: m.bad ? "var(--bad)" : MARK_COLOUR[m.kind] || (lane === "the end" ? "var(--ok)" : "var(--mark)"), kind: m.t1 != null && m.t1 - m.t > gap ? "past" : "mark" }, () => card(m.text, [["when", when(m.t, m.t1)]])));
    }
  }

  // Its log's lines, a mark each.
  const lines = v.log?.lines || [];
  const timed = lines.filter((l) => l.t != null);
  if (lines.length) {
    head("Log", h("span", "small dim", `${plural(lines.length, "line")}${timed.length < lines.length ? `, ${n(lines.length - timed.length)} without a time` : ""}`));
    const t = row("lines", tl.track());
    lines.forEach((l, i) => { if (l.t != null) seg(t, `l ${i}`, { from: l.t, until: l.t, colour: "var(--dim)", kind: "mark" }, () => card(tw.at(l.t), [["line", l.text.slice(0, 200)]])); });
  }

  // Its Mac: CPU, memory, NAS and network over it; the other jobs on it meanwhile.
  const host = f.host || "its Mac";
  head(host, h("span", "small dim", macSaid()));
  const ans = mac?.ans;
  const chart = (label, series, top, unit, title) => {
    const t = tl.track(drawChart(series, top));
    t.el.classList.add("chart");
    t.el.dataset.chart = String(charts.length);
    charts.push({ label, series, unit });
    return row(label, t, title);
  };
  if (ans) {
    const cores = ans.mac?.cores, mem = ans.mac?.mem_gb;
    chart("CPU", [["build_cores", "the build's", "--accent"], ["others_cores", "the rest of the Mac's", "--dim"]], cores || null, "cores", `Cores busy, the build's (its jobs') and the rest's, of ${cores ?? "?"}`);
    chart("memory", [["build_mem_gb", "the build's", "--accent"], ["others_mem_gb", "the rest of the Mac's", "--dim"]], mem || null, "GB", `Memory in use, the build's and the rest's, of ${mem ?? "?"} GB`);
    chart("NAS", [["nas_read_mbs", "read", "--nas-r"], ["nas_write_mbs", "written", "--nas-w"]], null, "MB/s", "The Mac's jobs' NAS traffic, all of them (MB/s)");
    chart("network", [["net_in_mbs", "in", "--net-d"], ["net_out_mbs", "out", "--net-u"]], null, "MB/s", "The jobs' internet traffic through the Mac's proxy (MB/s)");
  }
  const beside = v.beside || [];
  if (beside.length) {
    pack(beside, gap, (r) => r.from, (r) => r.until ?? now).forEach((rw, i) => {
      const t = row(i ? "" : `other jobs (${beside.length})`, tl.track(), "Other jobs on its Mac while it ran: the history's");
      for (const r of rw) seg(t, `b ${r.job}`, { from: r.from, until: r.until ?? now, colour: stepColour(r.step), kind: r.until == null ? "running" : "past", failed: r.ok === false, href: encodeURIComponent(r.job) },
        () => card(`${r.step}${r.targets.length ? ` ${r.targets.join(" ")}` : ""}${r.n > r.targets.length ? ` and ${r.n - r.targets.length} more` : ""}`, [["on", r.worker], ["when", when(r.from, r.until)], ["ended", r.until == null ? "running (no end in the history)" : r.ok === false ? "failed" : "ok"], ["job", `${r.job} (click: its page)`]]));
    });
  }
  lanes.append(h("div"), tl.axis);
  const legend = h("div", "legend", Object.entries(LOOK).filter(([k]) => k !== "old").map(([k, c]) => h("span", "lgi", h("i", { style: { background: c } }), k === "cpu" ? "computing" : `waiting on ${LOOK_SAID[k]}`)),
    h("span", "lgi", h("i", { class: "k-run" }), "running"), h("span", "lgi", h("i", { class: "k-failed" }), "cut (open as it ended)"),
    h("span", "hint", "a span clicked: its log lines · drag to move · pinch or ⌥ scroll to zoom"));
  return { lanes, tracks, span, legend };
}
function macSaid() {
  if (!mac) return "reading its measures…";
  if (mac.error) return `its measures: ${mac.error}`;
  const st = mac.ans?.step;
  return st ? `its measures a point every ${dur(st)} (the Mac's live store)` : "";
}
// A chart drawn on a track: the series' means stacked, a bar a point, to `top` (null: the most in
// the job's time).
function drawChart(series, top) {
  return (g, r, w, hh) => {
    const ans = mac?.ans;
    if (!ans) return;
    const step = ans.step || 1;
    const sums = new Map();
    for (const [name] of series) for (const p of ans.series?.[name] || []) sums.set(p[0], (sums.get(p[0]) || 0) + p[2]);
    const most = top || Math.max(1e-3, ...[...sums].filter(([t]) => t >= tlDomain.lo - step && t <= tlDomain.hi).map(([, x]) => x));
    const x = (t) => ((t - r.t0) / r.span) * w;
    const base = new Map();
    g.globalAlpha = 0.85;
    for (const [name, , col] of series) {
      g.fillStyle = token(col) || "#888";
      for (const p of ans.series?.[name] || []) {
        if (p[0] + step < r.t0 || p[0] > r.t1) continue;
        const b = base.get(p[0]) || 0, hgt = (p[2] / most) * (hh - 1);
        const a = x(p[0]), z = Math.max(a + 1, x(p[0] + step));
        g.fillRect(a, hh - b - hgt, z - a, hgt);
        base.set(p[0], b + hgt);
      }
    }
    g.globalAlpha = 1;
  };
}
function pointAt(e) {
  const key = e.target?.dataset?.card;
  if (key && cards.has(key)) { showTip(cards.get(key)(), e.clientX, e.clientY); return; }
  const tr = e.target?.closest?.(".track.chart");
  const c = tr && charts[+tr.dataset.chart];
  if (c && mac?.ans) {
    const t = tl.at(e.clientX), step = mac.ans.step || 1;
    const rows = c.series.map(([name, said]) => {
      const p = (mac.ans.series?.[name] || []).find((q) => q[0] <= t && t < q[0] + step);
      return [said, p ? `${fx(p[2], 2)} ${c.unit} (${fx(p[1], 2)}–${fx(p[3], 2)})` : "not measured"];
    });
    showTip(card(`${c.label} at ${tw.at(t, false)}`, [...rows, ["point", `mean, least–most over ${dur(step)}`]]), e.clientX, e.clientY);
    return;
  }
  hideTip();
}
function clickAt(e) {
  const key = e.target?.dataset?.card;
  if (key?.startsWith("s ")) { sel = sel === key ? null : key; tl.select(sel); logAll = false; renderLog(); document.getElementById("logsec")?.scrollIntoView({ behavior: "smooth", block: "nearest" }); }
  else if (key?.startsWith("l ")) { const i = +key.slice(2); sel = null; tl.select(null); logAll = true; renderLog(i); }
  else if (!key && !e.target?.closest?.(".track.chart")) { if (sel) { sel = null; tl.select(null); renderLog(); } }
}

// ---- the log under it --------------------------------------------------------------------------------
function renderLog(focus = null) {
  const box = document.getElementById("logsec");
  if (!box) return;
  const lines = v.log?.lines || [];
  const s = sel && segs.get(sel);
  const span = sel && tree?.byId.get(sel.slice(2));
  const t0 = span?.t0 ?? s?.from, t1 = span ? span.t1 ?? v.now : s?.until;
  const idx = lines.map((l, i) => i);
  const shown = span ? idx.filter((i) => lines[i].t != null && lines[i].t >= t0 - 1e-6 && lines[i].t <= t1 + 1e-6) : idx;
  const cut = !logAll && shown.length > LOG_SHOWN ? shown.slice(0, LOG_SHOWN) : shown;
  const head = span
    ? [h("span", null, `${plural(shown.length, "line")} of `, h("b", null, span.name), ` (${when(t0, t1)})`), h("button", { type: "button", class: "quiet", onclick: () => { sel = null; tl.select(null); renderLog(); } }, "all lines")]
    : [h("span", null, `${plural(lines.length, "line")}${v.log?.bytes ? ` · ${bytes(v.log.bytes)}` : ""}`), h("span", "dim", "a span clicked shows its own")];
  const gaps = (v.gaps || []).filter((g) => g.of === "log").map((g) => h("div", "why", `The log's middle is left out: ${bytes(g.to - g.from)} (the lead keeps a run's first 1 MB and its last 256 KB past 1.25 MB).`));
  const list = h("div", "loglines", cut.map((i) => h("div", { class: `ll${i === focus ? " on" : ""}`, "data-i": i }, h("span", "lt", lines[i].t != null ? tw.at(lines[i].t) : "–"), h("span", "lx", lines[i].text))));
  const more = cut.length < shown.length ? h("button", { type: "button", onclick: () => { logAll = true; renderLog(); } }, `show all ${n(shown.length)}`) : null;
  box.replaceChildren(h("h2", null, "Log", h("span", null, ...head)), ...gaps, lines.length ? list : h("div", "dim small", v.running ? "No line yet." : "No log kept for it."), more || "");
  if (focus != null) list.querySelector(`[data-i="${focus}"]`)?.scrollIntoView({ block: "center" });
}

// ---- the page --------------------------------------------------------------------------------------------
function render() {
  if (!v) return;
  const f = facts(), out = outcome(v);
  tw = timeWords(mode, f.t0 ?? v.now, f.t1 ?? v.now);
  document.title = `${f.step || "job"} ${f.targets.slice(0, 2).join(" ")} · job ${ID}`;
  $("title").textContent = v.name || `${f.step} ${f.targets.slice(0, 3).join(" ")}`.trim() || `job ${ID}`;
  $("state").textContent = out.said;
  $("state").className = `chip ${out.state === "failed" ? "bad" : out.state === "running" ? "run" : out.state === "ok" ? "ok" : ""}`;
  $("sub").textContent = `job ${ID}${f.host ? ` on ${f.host}` : ""}`;
  $("mode").textContent = mode === "clock" ? "Clock times" : "T+ times";
  $("mode").setAttribute("aria-pressed", String(mode === "clock"));
  $("mode-said").textContent = mode === "clock" ? `local times${tw.date ? ", the date as it crosses midnight" : ""}` : `since its start, ${f.t0 != null ? clockText(f.t0, { date: true }) : "–"}`;
  side(f, out);
  const main = $("jmain");
  if (v.legacy || f.t0 == null) {
    tl = null; tlDomain = null;
    main.replaceChildren(h("section", "box legacy", h("h2", null, "A legacy job"),
      h("p", null, "No timeline is kept for this job: it ran before the spans (task #1104), on an app before them, or was kept by an earlier lead (the spans stay on the Mac that led). The details on the left are the history's, as far as it has them", v.record ? ", and its timings' record." : ".")));
    return;
  }
  const { lanes, tracks, span, legend } = timeline(f);
  let tlsec = document.getElementById("tlsec"), logsec = document.getElementById("logsec");
  if (!tlsec) {
    tlsec = h("section", { id: "tlsec", class: "box sched" });
    logsec = h("section", { id: "logsec", class: "box" });
    main.replaceChildren(tlsec, logsec);
  }
  tlsec.replaceChildren(tl.el, legend);
  tl.placed(lanes, tracks, span);
  if (sel && !segs.has(sel)) sel = null;
  tl.select(sel);
  renderLog();
}

// The Mac's measures over the job: its own port's /api/live when it's the lead (this page's), else
// at its addresses (live.js remote); none past the store's seven days.
async function readMac(f) {
  if (!f.host) return;
  if (f.t0 != null && v.now - f.t0 > LIVE_KEPT_S) { mac = { host: f.host, error: "not kept (a Mac keeps its measures seven days)" }; return; }
  const body = { range: Math.ceil(v.now - (f.t0 ?? v.now) + 120), series: SERIES, net: false };
  try {
    if (!swarm) swarm = await ask("/api/swarm", {});
    const here = swarm?.agent?.host;
    const m = (swarm?.macs || []).find((x) => x.host === f.host);
    const ans = here === f.host || !m ? await ask("/api/live", body) : await remote(m, {}, body);
    mac = ans.host && ans.host !== f.host ? { host: f.host, error: "its Mac isn't in the pool's list" } : { host: f.host, ans };
  } catch (e) {
    mac = { host: f.host, error: e.message === "doesn't answer" ? "its Mac doesn't answer" : e.message };
  }
}

function liveDot() {
  const el = $("live");
  const on = v?.running && heard && Date.now() - heard < 3 * POLL_MS;
  el.className = on ? "on" : failing ? "stale" : "";
  el.querySelector("b").textContent = !v ? "" : v.running ? (failing ? "Stale" : "Live") : "Ended";
  $("live-at").textContent = failing ? ` ${failing}` : heard ? ` updated ${clockText(heard / 1000)}` : "";
}

let lastMac = 0;
async function load() {
  busy(true);
  try {
    v = await ask(`/api/job/${encodeURIComponent(ID)}`);
    heard = Date.now(); failing = null;
  } catch (e) {
    failing = e.status === 503 ? "this Mac isn't the lead: the jobs' spans are the lead's" : e.status === 400 ? `not a job id: ${ID}` : `can't read the job: ${e.message}`;
    if (!v) $("jmain").replaceChildren(h("div", "empty bad", failing));
  } finally {
    busy(false);
  }
  liveDot();
  if (!v) { if (failing && !/not a job id/.test(failing)) setTimeout(load, POLL_MS * 2); return; }
  render();
  const f = facts();
  const firstTime = !steps;
  if (firstTime) {
    ask("/api/steps", {}).then((s) => { steps = s; render(); }, () => {});
    ask("/api/timings", { kind: v.record?.kind || v.kind || f.step, last: 20 }).then((t) => { meds = stepMedians(t.runs, ID); render(); }, (e) => { meds = { error: e.message }; render(); });
  }
  if (firstTime || (v.running && Date.now() - lastMac > LIVE_MS)) {
    lastMac = Date.now();
    readMac(f).then(render);
  }
  if (v.running) setTimeout(load, POLL_MS);
}

$("mode").addEventListener("click", () => { mode = mode === "clock" ? "rel" : "clock"; store.set("job-times", mode); render(); });
addEventListener("storage", (e) => { if (e.key === "job-times") { mode = store.get("job-times", "rel") === "clock" ? "clock" : "rel"; render(); } });
setInterval(liveDot, 2000);
load();
