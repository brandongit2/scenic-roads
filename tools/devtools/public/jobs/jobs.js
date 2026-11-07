// The dev job dashboard (tools/devtools/README.md, "Dev jobs"): every run of the long dev jobs the
// Macs record (tools/dev/devjob.zsh, devjob.py), from the app's /api/jobs (jobs.mjs).
//   Runs     each Mac's runs on a timeline, a lane a kind (check, publish, tests, cargo, a
//            measurement, a backtest), with the Mac's load and memory under them; the build page's
//            timeline (web/work/timeline.js), moved and zoomed the same way.
//   Run      one run opened: what it is (who, task, commit, its Mac's load), its spans on a timeline
//            of their own (a lane a kind, deepest last: parts or steps, compiles, the test Mac's
//            run, suites, tests, crates), where its time went, its longest and failed spans.
//   By name  one kind of span over all time: each name's runs, failures and times.
// The page's address keeps what's open (?range=24h&run=<id>&kind=test). Live: the app's event
// stream says when a Mac sent more.
import { createTimeline } from "../lib/work/timeline.js";
import { h, token } from "../lib/work/dom.js";
import { pad } from "../lib/work/words.js";

const API = "../api/jobs/";
const RANGES = [["6h", 6 * 3600], ["24h", 86400], ["7d", 7 * 86400], ["30d", 30 * 86400], ["all", null]];
// A run's class (its lane): a cargo run's purpose when it has one, else its kind.
const CLASSES = ["check", "publish", "tests", "cargo", "measurement", "backtest"];
const CLASS_LABEL = { check: "check.sh", publish: "publish.sh", tests: "test runs", cargo: "cargo", measurement: "measurements", backtest: "backtests" };
// A span's kinds in a run, in the order their lanes come at one depth.
const KIND_ORDER = ["step", "part", "wait", "reset", "copy", "build", "compile", "run", "tests", "suite", "program", "crate", "test", "cargo"];
const MIN_SPAN = 20; // the timelines' least width, s (the axis ticks every 10 s at the finest)
const ROWS_SHOWN = 150; // a table's rows before "show all"

// ---- small things ---------------------------------------------------------------------------------
const $ = (id) => document.getElementById(id);
const get = async (path) => {
  const r = await fetch(API + path, { cache: "no-store" });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j?.error?.message || `${r.status}`);
  return j;
};
export function dur(s) {
  if (s == null || !Number.isFinite(s)) return "–";
  if (s < 1) return `${Math.round(s * 1000)} ms`;
  if (s < 10) return `${s.toFixed(1)} s`;
  if (s < 60) return `${Math.round(s)} s`;
  if (s < 3600) { const m = Math.floor(s / 60); return `${m} min ${String(Math.round(s - m * 60)).padStart(2, "0")} s`; }
  const hr = Math.floor(s / 3600); return `${hr} h ${String(Math.round((s - hr * 3600) / 60)).padStart(2, "0")} min`;
}
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
function clock(t, secs = true) {
  const d = new Date(t * 1000), today = new Date();
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}${secs ? `:${pad(d.getSeconds())}` : ""}`;
  if (d.toDateString() === today.toDateString()) return hm;
  return `${DAYS[d.getDay()]} ${d.getDate()}/${d.getMonth() + 1} ${hm}`;
}
const classOf = (r) => (r.kind === "cargo" && (r.attrs.purpose === "measurement" || r.attrs.purpose === "backtest") ? r.attrs.purpose : CLASSES.includes(r.kind) ? r.kind : "other");
const colourOf = (k) => `var(--k-${k}, var(--k-other))`;
const bad = (s) => s.status === "fail" || s.status === "gone" || s.lost != null;
const statusText = (s) => (s.lost != null ? `lost (no word since ${clock(s.lost)})` : s.status === "running" ? "running" : s.status === "ok" ? "ok" : s.status);
const statusClass = (s) => (bad(s) ? "bad" : s.status === "running" ? "run" : s.status === "stopped" ? "warn" : s.status === "ok" ? "ok" : "dim");
const took = (s, now) => (s.t1 ?? now) - s.t0;
// A Mac's short name: its chip ("M1 Pro"), then its host name.
const macName = (mac, macs) => { const c = macs?.[mac]?.chip?.replace(/^Apple /, ""); return c ? `${c} · ${mac}` : mac; };
// A colour from green to red by a share (0 fine, 1 bad), in ten steps, as rgb() from the theme's
// --ok, --warn and --bad (#rrggbb): what any canvas takes, Safari's too.
const rgbOf = (name) => { const v = token(name); return [1, 3, 5].map((i) => parseInt(v.slice(i, i + 2), 16)); };
const heats = new Map(); // (each step's, for the theme now)
const heat = (x) => {
  const v = Math.round(Math.max(0, Math.min(1, x)) * 10) / 10;
  if (!heats.has(v)) {
    const [a, b, f] = v <= 0.5 ? [rgbOf("--ok"), rgbOf("--warn"), v * 2] : [rgbOf("--warn"), rgbOf("--bad"), (v - 0.5) * 2];
    heats.set(v, `rgb(${a.map((c, i) => Math.round(c + (b[i] - c) * f)).join(", ")})`);
  }
  return heats.get(v);
};
matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => { heats.clear(); renderMain(); renderRun(); });
// The load's share of trouble: a load at the cores' count is half, at twice it all; memory free (macOS's
// memorystatus_level) at 50 % none, at 10 % all.
const loadHeat = (load, ncpu) => (load == null ? null : heat(load / (2 * (ncpu || 8))));
const memHeat = (free) => (free == null ? null : heat((50 - free) / 40));

// Rows of [from, until] items that don't overlap (by `gap` s), in time order: [[item…]…].
function pack(items, gap, from = (x) => x.from, until = (x) => x.until) {
  const rows = [], ends = [];
  for (const it of [...items].sort((a, b) => from(a) - from(b))) {
    let i = ends.findIndex((e) => e + gap <= from(it));
    if (i < 0) { i = rows.length; rows.push([]); ends.push(-Infinity); }
    rows[i].push(it);
    ends[i] = Math.max(until(it), from(it) + gap);
  }
  return rows;
}

// ---- state, kept in the address ---------------------------------------------------------------------
const q = new URLSearchParams(location.search);
const st = {
  range: RANGES.some(([k]) => k === q.get("range")) ? q.get("range") : "24h",
  hide: new Set((q.get("hide") || "").split(",").filter(Boolean)),
  run: q.get("run"),
  kind: q.get("kind") || "",
  sort: q.get("sort") || "p90",
  filter: "",
};
function saveState() {
  const p = new URLSearchParams();
  if (st.range !== "24h") p.set("range", st.range);
  if (st.hide.size) p.set("hide", [...st.hide].join(","));
  if (st.run) p.set("run", st.run);
  if (st.kind) p.set("kind", st.kind);
  if (st.sort !== "p90") p.set("sort", st.sort);
  history.replaceState(null, "", `${location.pathname}${p.size ? "?" + p : ""}`);
}

// ---- the card of what's pointed at ---------------------------------------------------------------------
const tip = $("tip");
let pinned = null;
function showTip(content, x, y, pin = false) {
  tip.replaceChildren(...content);
  tip.hidden = false;
  tip.classList.toggle("pinned", pin);
  const r = tip.getBoundingClientRect(), W = innerWidth, H = innerHeight;
  tip.style.left = `${Math.max(8, Math.min(W - r.width - 8, x + 14))}px`;
  tip.style.top = `${y + 16 + r.height > H ? Math.max(8, y - r.height - 10) : y + 16}px`;
}
const hideTip = () => { tip.hidden = true; pinned = null; };
document.addEventListener("pointerdown", (e) => { if (pinned && !tip.contains(e.target) && !e.target.closest?.(".lanewrap")) hideTip(); });
const facts = (rows) => h("dl", null, rows.filter((r) => r && r[1] != null && r[1] !== "").map(([k, v]) => [h("dt", null, k), h("dd", null, v)]));
// Who and what a run is, for its card and its header.
function runFacts(r, now, macs) {
  const a = r.attrs || {};
  return [
    ["status", h("span", statusClass(r), statusText(r))],
    ["began", clock(r.t0)],
    ["took", r.t1 != null ? dur(r.t1 - r.t0) : `${dur(now - r.t0)} so far`],
    ["Mac", macName(r.mac, macs)],
    a.ran_on && ["tests ran on", macName(a.ran_on, macs)],
    ["checkout", [a.checkout, a.by && `(${a.by === "agent" ? `an agent${a.session ? ` ${a.session}` : ""}` : "a terminal"})`].filter(Boolean).join(" ")],
    a.task && ["task", h("a", { href: `../task-board/${a.task}`, target: "_blank" }, `#${a.task}`)],
    a.branch && ["branch", a.branch],
    (a.commit || a.head) && ["commit", a.commit || a.head],
    a.purpose && ["for", a.purpose],
    a.args && ["command", `cargo ${a.args}`],
    a.command && ["command", a.command],
    a.version && ["published", a.version],
    a.failed_in && ["failed in", a.failed_in],
    a.why && ["why", a.why],
    a.exit && a.exit !== "0" && ["exit", a.exit],
    a.cpu && ["CPU", `${a.cpu} s`],
    a.peak_mb && ["largest process", `${a.peak_mb} MB`],
    a.log && ["log", a.log],
    a.tests_log && ["tests' log", a.tests_log],
  ];
}

// ---- the runs' timeline ---------------------------------------------------------------------------------
let main = null; // {from, to, now, runs, samples, macs}
let tl = null, tlRange = null;
const cards = new Map(); // a segment's key → its card's content
// (Each render's own dropped first: the runs' timeline's keys "r", "l", "m", a run's "s", "x".)
const dropCards = (...ps) => { for (const k of [...cards.keys()]) if (ps.includes(k[0])) cards.delete(k); };
let lastWidth = 0;
function rangeFrom(now) {
  const span = RANGES.find(([k]) => k === st.range)[1];
  return span ? now - span : null;
}
async function loadMain() {
  const now = Date.now() / 1000;
  let from = rangeFrom(now);
  const runs = await get(`runs?from=${from ?? 0}&to=${now + 60}`);
  if (from == null) from = Math.min(now - 3600, ...runs.runs.map((r) => r.t0));
  const w = Math.max(200, Math.round($("runs").clientWidth || 800));
  const samples = await get(`samples?from=${from}&to=${now + 60}&n=${w}`);
  main = { from, to: now, now: runs.now, runs: runs.runs, samples: samples.samples, step: samples.step, macs: { ...samples.macs, ...runs.macs } };
  renderMain();
}
function mainRelations(key) {
  const r = segRuns.get(key);
  if (key.startsWith("lg ")) {
    const c = key.slice(3);
    return { on: [...segs.values()].filter((s) => s.cls === c), arrows: [] };
  }
  // (A run pointed at: the checkout's other runs lit, an agent's jobs across the lanes.)
  if (!r?.attrs?.checkout) return null;
  return { on: [...segs.values()].filter((s) => s.run?.attrs?.checkout === r.attrs.checkout), arrows: [] };
}
const segs = new Map(), segRuns = new Map();
function renderMain() {
  if (!main) return;
  const { now, from, macs } = main;
  if (!tl || tlRange !== st.range) {
    const span = RANGES.find(([k]) => k === st.range)[1] ?? now - from;
    tl = createTimeline({ relations: mainRelations, onRepack: renderMain, minSpan: MIN_SPAN, span0: span });
    tlRange = st.range;
    hookCards(tl, (key) => segRuns.get(key));
  }
  // (A little past now, so the now line and its word sit inside.)
  const lo = from, hi = now + Math.max(60, (now - from) * 0.03);
  const { span } = tl.range(now, lo, hi);
  const width = Math.max(240, tl.axis.offsetWidth || $("runs").clientWidth - 140);
  const gap = (span / width) * 3;
  segs.clear(); segRuns.clear(); dropCards("r", "l", "m");
  const shown = main.runs.filter((r) => !st.hide.has(classOf(r)));
  const byMac = new Map();
  for (const r of shown) (byMac.get(r.mac) || byMac.set(r.mac, []).get(r.mac)).push(r);
  for (const m of Object.keys(main.samples)) if (!byMac.has(m)) byMac.set(m, []);
  const lanes = h("div", "lanes"), tracks = [];
  const track = (cls) => { const t = tl.track(); if (cls) t.el.classList.add(cls); tracks.push(t); return t; };
  let running = 0, failed = 0;
  for (const mac of [...byMac.keys()].sort()) {
    const runs = byMac.get(mac), ss = main.samples[mac] || [], last = ss[ss.length - 1], info = macs[mac] || {};
    // (The Mac by its chip, its name, and its load and memory now, or when it was last sampled.)
    lanes.append(h("div", { class: "gh", title: mac }, info.chip?.replace(/^Apple /, "") || mac),
      h("div", "ghr", h("span", null, `${mac}${info.mem ? ` · ${info.mem} GB` : ""}`), last && now - last[0] < 120
        ? [h("span", null, `load ${last[1]?.toFixed(1)}/${info.ncpu ?? "?"}`), h("span", null, `${last[2]}% memory free`), last[3] ? h("span", null, `swap ${(last[3] / 1024).toFixed(1)} GB`) : null]
        : h("span", null, info.seen ? `last sampled ${clock(info.seen, false)}` : "")));
    for (const cls of [...CLASSES, "other"]) {
      const mine = runs.filter((r) => classOf(r) === cls);
      if (!mine.length) continue;
      pack(mine, gap, (r) => r.t0, (r) => r.t1 ?? now).forEach((row, i) => {
        const t = track();
        for (const r of row) {
          const key = `r ${r.id}`;
          if (r.status === "running" && r.lost == null) running++;
          if (bad(r)) failed++;
          const s = t.add({ key, from: r.t0, until: r.t1 ?? now, colour: colourOf(cls), kind: r.status === "running" && r.lost == null ? "running" : "past", failed: bad(r), cls, run: r });
          segs.set(key, s); segRuns.set(key, r);
          cards.set(key, () => [h("b", null, r.name), facts([...runFacts(r, now, macs), r.spans ? ["spans", `${r.spans}${r.failed ? `, ${r.failed} failed` : ""}`] : null, ["Mac's load then", sampleAt(mac, r.t0)]]), h("div", "go", "click to open it")]);
        }
        lanes.append(h("div", { class: "ln", title: CLASS_LABEL[cls] || cls }, i ? "" : `${CLASS_LABEL[cls] || cls} (${mine.length})`), t.el);
      });
    }
    // The Mac's load and memory, a sample bucket a segment.
    if (ss.length) {
      const lt = track("thin"), mt = track("thin");
      ss.forEach((p, i) => {
        const until = ss[i + 1] && ss[i + 1][0] - p[0] < main.step * 3 + 30 ? ss[i + 1][0] : p[0] + Math.max(10, main.step);
        const lc = loadHeat(p[1], info.ncpu), mc = memHeat(p[2]);
        if (lc) lt.add({ key: `l ${mac} ${i}`, from: p[0], until, colour: lc, kind: "past" });
        if (mc) mt.add({ key: `m ${mac} ${i}`, from: p[0], until, colour: mc, kind: "past" });
        cards.set(`l ${mac} ${i}`, () => [h("b", null, `${macName(mac, macs)} at ${clock(p[0])}`), facts([["load", `${p[1]} (${info.ncpu ?? "?"} cores; the most in ${dur(main.step)})`], ["memory free", `${p[2]} %`], ["swap", p[3] != null ? `${(p[3] / 1024).toFixed(2)} GB` : null]])]);
        cards.set(`m ${mac} ${i}`, cards.get(`l ${mac} ${i}`));
      });
      lanes.append(h("div", "ln", "load"), lt.el, h("div", "ln", "memory"), mt.el);
    }
  }
  lanes.append(h("div"), tl.axis);
  $("runs-sum").textContent = `${shown.length} in ${st.range === "all" ? "all time" : `the last ${st.range}`}${running ? ` · ${running} running` : ""}${failed ? ` · ${failed} failed` : ""}`;
  $("state").textContent = running ? `${running} running` : "";
  const legend = h("div", "legend",
    CLASSES.filter((c) => shown.some((r) => classOf(r) === c)).map((c) => h("span", { class: "lgi", "data-lg": `lg ${c}` }, h("i", { style: { background: colourOf(c) } }), CLASS_LABEL[c])),
    h("span", "lgi", h("i", "k-fail"), "failed, gone or lost"), h("span", "lgi", h("i", "k-run"), "running"),
    h("span", "lgi", h("i", "heat"), "load (to twice the cores), memory (50 % free to 10 %)"),
    h("span", "hint", "drag to move · pinch or ⌥ scroll to zoom · a run pointed at lights its checkout's"));
  legend.addEventListener("pointerover", (e) => { if (e.pointerType !== "touch") tl.hover(e.target.closest?.("[data-lg]")?.dataset.lg || null); });
  legend.addEventListener("pointerleave", () => tl.hover(null));
  if (!byMac.size) { $("runs").replaceChildren(h("div", "empty", "Nothing recorded in this time: a run of check.sh, publish.sh, remote-tests.sh or cargo in a worktree records itself.")); return; }
  $("runs").replaceChildren(tl.el, legend);
  tl.placed(lanes, tracks, span);
  lastWidth = width;
}
function sampleAt(mac, t) {
  const ss = main?.samples?.[mac] || [];
  let best = null;
  for (const p of ss) { if (p[0] <= t + 30) best = p; else break; }
  if (!best || t - best[0] > 120) return null;
  return `${best[1]} load, ${best[2]} % memory free`;
}
// A timeline's cards: the segment under the pointer (timeline.js's data-card) shown; a click (a
// tap pins the card) opens a run.
function hookCards(t, runOf) {
  const el = t.el;
  el.addEventListener("pointermove", (e) => {
    if (pinned) return;
    const key = e.target?.dataset?.card;
    if (!key || !cards.has(key)) { if (!tip.hidden) hideTip(); return; }
    showTip(cards.get(key)(), e.clientX, e.clientY);
  });
  el.addEventListener("pointerleave", () => { if (!pinned) hideTip(); });
  el.addEventListener("click", (e) => {
    const key = e.target?.dataset?.card;
    if (!key || !cards.has(key)) { hideTip(); return; }
    const r = runOf(key);
    if (e.pointerType === "touch" || (e.pointerType === "" && matchMedia("(pointer: coarse)").matches)) {
      pinned = key;
      t.pin(key);
      const c = cards.get(key)();
      if (r) c.push(h("button", { type: "button", onclick: () => { hideTip(); t.pin(null); openRun(r.id); } }, "Open the run"));
      showTip(c, e.clientX, e.clientY, true);
      return;
    }
    if (r) openRun(r.id);
  });
}

// ---- one run ---------------------------------------------------------------------------------------------
let runTl = null, runData = null, runSegs = new Map(), runSpans = new Map();
async function openRun(id) {
  st.run = id; saveState();
  if (!runData || runData.run.id !== id) { $("run-sec").hidden = false; $("run").replaceChildren(h("div", "empty", "reading the run…")); }
  try { runData = await get(`runs/${encodeURIComponent(id)}`); } catch (e) { runData = null; $("run-sec").hidden = false; $("run").replaceChildren(h("div", "empty", `Can't read run ${id}: ${e.message}`)); return; }
  runTl = null;
  // (Shown first: a timeline measures its width as it's placed.)
  $("run-sec").hidden = false;
  renderRun();
  if (!$("run-sec").dataset.seen) { $("run-sec").scrollIntoView({ behavior: "smooth", block: "start" }); $("run-sec").dataset.seen = id; }
}
function runRelations(key) {
  const s = runSpans.get(key);
  if (!s) return null;
  // (A span pointed at: it, what it's part of and what's part of it.)
  const on = new Set([key]);
  for (let p = s.parent; p && runSpans.has(`s ${p}`); p = runSpans.get(`s ${p}`).parent) on.add(`s ${p}`);
  const down = (id) => { for (const k of kids.get(id) || []) { on.add(`s ${k.id}`); down(k.id); } };
  down(s.id);
  return { on: [...on].map((k) => runSegs.get(k)).filter(Boolean), arrows: [] };
}
let kids = new Map();
function renderRun() {
  if (!runData) return;
  const { run, spans, samples, macs, now } = runData;
  const t0 = run.t0 ?? Math.min(...spans.map((s) => s.t0)), t1 = run.t1 ?? now;
  kids = new Map();
  for (const s of spans) (kids.get(s.parent) || kids.set(s.parent, []).get(s.parent)).push(s);
  const depth = new Map([[run.id, 0]]);
  const depthOf = (s) => { if (depth.has(s.id)) return depth.get(s.id); const d = (s.parent && s.parent !== run.id && runSpansById.get(s.parent) ? depthOf(runSpansById.get(s.parent)) : 0) + 1; depth.set(s.id, d); return d; };
  const runSpansById = new Map(spans.map((s) => [s.id, s])), byId = runSpansById;
  spans.forEach(depthOf);
  $("run-title").textContent = run.name || run.id;
  const pad = Math.max(2, (t1 - t0) * 0.02);
  if (!runTl) {
    runTl = createTimeline({ relations: runRelations, onRepack: renderRun, minSpan: MIN_SPAN, span0: t1 - t0 + 2 * pad });
    hookCards(runTl, () => null);
  }
  const lo = t0 - pad, hi = Math.max(t1 + pad, lo + MIN_SPAN);
  const { span } = runTl.range(now, lo, hi);
  const width = Math.max(240, runTl.axis.offsetWidth || $("run").clientWidth - 140);
  const gap = (span / width) * 2;
  runSegs = new Map(); runSpans = new Map(); dropCards("s", "x");
  const lanes = h("div", "lanes"), tracks = [];
  const track = (cls) => { const t = runTl.track(); if (cls) t.el.classList.add(cls); tracks.push(t); return t; };
  // The run itself, then a lane a kind at each depth.
  const top = track();
  top.add({ key: `s ${run.id}`, from: t0, until: t1, colour: colourOf(classOf(run)), kind: run.status === "running" && run.lost == null ? "running" : "past", failed: bad(run) });
  cards.set(`s ${run.id}`, () => [h("b", null, run.name), facts(runFacts(run, now, macs))]);
  lanes.append(h("div", "ln", "the run"), top.el);
  const maxDepth = Math.max(0, ...depth.values());
  for (let d = 1; d <= maxDepth; d++) {
    const at = spans.filter((s) => depth.get(s.id) === d && s.t0 != null);
    const ks = [...new Set(at.map((s) => s.kind))].sort((a, b) => (KIND_ORDER.indexOf(a) + 1 || 99) - (KIND_ORDER.indexOf(b) + 1 || 99));
    for (const k of ks) {
      const mine = at.filter((s) => s.kind === k);
      // (Those too short to see at this zoom, and cargo test's tests, marks at their ends, share one row:
      // they may touch. The rest packed, a row each at once.)
      const short = (s) => s.attrs?.untimed || (s.t1 != null && s.t1 - s.t0 < gap);
      const marks = mine.filter(short), timed = mine.filter((s) => !short(s));
      [...(marks.length ? [marks] : []), ...pack(timed, gap, (s) => s.t0, (s) => s.t1 ?? now)].forEach((row, i) => {
        const t = track();
        for (const s of row) {
          const key = `s ${s.id}`, untimed = s.attrs?.untimed;
          const seg = t.add({ key, from: s.t0, until: s.t1 ?? now, colour: colourOf(k), kind: untimed ? "mark" : s.status === "running" && s.lost == null ? "running" : "past", failed: bad(s) });
          runSegs.set(key, seg); runSpans.set(key, s);
          cards.set(key, () => [h("b", null, s.name), facts([["kind", s.kind], s.parent !== run.id && byId.get(s.parent) ? ["in", byId.get(s.parent).name] : null, ["status", h("span", statusClass(s), statusText(s))], untimed ? ["ended", clock(s.t1)] : ["began", clock(s.t0)], ["took", untimed ? "– (cargo test says when a test ended, not when it began)" : s.t1 != null ? dur(s.t1 - s.t0) : `${dur(now - s.t0)} so far`],
            s.mac !== run.mac ? ["Mac", macName(s.mac, macs)] : null, ...Object.entries(s.attrs || {}).filter(([a]) => a !== "untimed").map(([a, v]) => [a, Array.isArray(v) ? `${v.length}: ${v.slice(0, 40).join(", ")}${v.length > 40 ? "…" : ""}` : v])])]);
        }
        lanes.append(h("div", { class: "ln", title: k }, i ? "" : `${"· ".repeat(d - 1)}${k} (${mine.length})`), t.el);
      });
    }
  }
  // The Macs' load and memory over it: the run's Mac and the test Mac.
  for (const [mac, ss] of Object.entries(samples)) {
    if (!ss.length) continue;
    const info = macs[mac] || {}, lt = track("thin"), mt = track("thin");
    ss.forEach((p, i) => {
      const until = ss[i + 1] && ss[i + 1][0] - p[0] < 40 ? ss[i + 1][0] : p[0] + 10;
      const lc = loadHeat(p[1], info.ncpu), mc = memHeat(p[2]);
      const key = `x ${mac} ${i}`;
      if (lc) lt.add({ key, from: p[0], until, colour: lc, kind: "past" });
      if (mc) mt.add({ key, from: p[0], until, colour: mc, kind: "past" });
      cards.set(key, () => [h("b", null, `${macName(mac, macs)} at ${clock(p[0])}`), facts([["load", `${p[1]} (${info.ncpu ?? "?"} cores)`], ["memory free", `${p[2]} %`], ["swap", p[3] != null ? `${(p[3] / 1024).toFixed(2)} GB` : null]])]);
    });
    const short = macs[mac]?.chip?.replace(/^Apple /, "") || mac;
    lanes.append(h("div", { class: "ln", title: mac }, `${short} load`), lt.el, h("div", { class: "ln", title: mac }, `${short} memory`), mt.el);
  }
  lanes.append(h("div"), runTl.axis);

  // Where its time went (its own parts, in order), its longest spans, what failed; the time by kind.
  const timed = spans.filter((s) => s.t0 != null && !s.attrs?.untimed);
  const first = (kids.get(run.id) || []).filter((s) => s.t0 != null).sort((a, b) => a.t0 - b.t0);
  const whole = Math.max(1e-3, t1 - t0);
  // (A test named with its suite.)
  const label = (s) => (s.kind === "test" && byId.get(s.parent) ? `${byId.get(s.parent).name} › ${s.name}` : s.name);
  const row = (s, d, scale) => h("tr", { class: "pick", onpointerenter: () => runTl.hover(`s ${s.id}`), onpointerleave: () => runTl.hover(null), onclick: () => { runTl.pin(`s ${s.id}`); } },
    h("td", { class: "nm", title: label(s) }, h("span", statusClass(s), bad(s) ? "✗ " : ""), label(s)), h("td", "dim", s.kind), h("td", "num", s.attrs?.untimed ? "–" : dur(d)),
    scale ? h("td", "barcell", h("div", { class: "bar", style: { width: `${Math.max(0.5, (100 * d) / scale)}%`, background: colourOf(s.kind) } })) : null);
  const byKind = new Map();
  for (const s of timed) byKind.set(s.kind, (byKind.get(s.kind) || 0) + took(s, now));
  const longest = [...timed].filter((s) => s.parent !== run.id || first.length < 2).sort((a, b) => took(b, now) - took(a, now)).slice(0, 25);
  const fails = spans.filter(bad).sort((a, b) => a.t0 - b.t0);
  const table = (head, rows) => h("table", null, h("thead", null, h("tr", null, head.map(([t, c]) => h("th", c, t)))), h("tbody", null, rows));
  const cols = h("div", "cols",
    h("div", null, h("h3", null, "Where its time went"),
      first.length ? table([["", null], ["kind", null], ["took", "num"], ["", null]], first.map((s) => row(s, took(s, now), whole))) : h("div", "empty", "No parts recorded."),
      byKind.size ? [h("h3", { style: { marginTop: "10px" } }, "Time by kind (summed; parallel spans count each)"), table([["kind", null], ["total", "num"]], [...byKind].sort((a, b) => b[1] - a[1]).map(([k, v]) => h("tr", null, h("td", "nm", h("i", { class: "sw", style: { background: colourOf(k), marginRight: "5px" } }), k), h("td", "num", dur(v)))))] : null),
    h("div", null, h("h3", null, "Longest"), longest.length ? table([["", null], ["kind", null], ["took", "num"]], longest.map((s) => row(s, took(s, now)))) : h("div", "empty", "–")),
    h("div", null, h("h3", null, `Failed (${fails.length})`), fails.length ? table([["", null], ["kind", null], ["took", "num"]], fails.slice(0, ROWS_SHOWN).map((s) => row(s, took(s, now)))) : h("div", "empty", "Nothing failed.")));
  const f = runFacts(run, now, macs).filter((r) => r && r[1] != null && r[1] !== "");
  $("run").replaceChildren(h("div", "facts", f.map(([k, v]) => h("span", null, h("b", null, k), v))), h("div", "sched", runTl.el), cols);
  runTl.placed(lanes, tracks, span);
}
$("run-close").addEventListener("click", () => { st.run = null; saveState(); runData = null; $("run-sec").hidden = true; delete $("run-sec").dataset.seen; });

// ---- by name -----------------------------------------------------------------------------------------------
let kinds = [], stats = null, statsAll = false;
async function loadStats() {
  kinds = (await get("kinds")).kinds;
  if (!st.kind || !kinds.some((k) => k.kind === st.kind)) st.kind = (kinds.find((k) => k.kind === "part") || kinds.find((k) => k.kind === "suite") || kinds[0])?.kind || "";
  stats = st.kind ? await get(`stats?kind=${encodeURIComponent(st.kind)}`) : null;
  renderStats();
}
const SORTS = [["p90", "p90", (g) => g.p90 ?? -1], ["p50", "median", (g) => g.p50 ?? -1], ["max", "longest", (g) => g.max ?? -1], ["total", "total", (g) => g.total], ["n", "runs", (g) => g.n], ["failed", "failed", (g) => g.failed]];
function spark(d) {
  if (d.length < 2) return null;
  const W = 80, H = 16, top = Math.max(...d) || 1;
  const pts = d.map((v, i) => `${((i / (d.length - 1)) * W).toFixed(1)},${(H - 1 - (v / top) * (H - 2)).toFixed(1)}`).join(" ");
  const s = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  s.setAttribute("class", "spark"); s.setAttribute("width", W); s.setAttribute("height", H); s.setAttribute("viewBox", `0 0 ${W} ${H}`);
  s.innerHTML = `<polyline points="${pts}"/>`;
  s.append(Object.assign(document.createElementNS("http://www.w3.org/2000/svg", "title"), { textContent: `its last ${d.length} times, the highest ${dur(top)}` }));
  return s;
}
function renderStats() {
  const box = $("stats");
  if (!kinds.length) { box.replaceChildren(h("div", "empty", "Nothing recorded yet.")); return; }
  const sel = h("select", { "aria-label": "Which kind of span", onchange: (e) => { st.kind = e.target.value; statsAll = false; saveState(); loadStats(); } },
    kinds.map((k) => h("option", { value: k.kind, selected: k.kind === st.kind }, `${k.kind} (${k.n})`)));
  const filter = h("input", { type: "search", placeholder: "filter names", value: st.filter, oninput: (e) => { st.filter = e.target.value; renderTable(); } });
  const tableBox = h("div");
  box.replaceChildren(h("div", "statbar", h("span", "dim", "kind"), sel, filter, h("span", "dim", "all time; times of the runs that ended; cargo test's tests have no times (it says when one ended, not when it began)")), tableBox);
  function renderTable() {
    const by = SORTS.find(([k]) => k === st.sort) || SORTS[0];
    const f = st.filter.toLowerCase();
    const names = (stats?.names || []).filter((g) => !f || g.name.toLowerCase().includes(f)).sort((a, b) => by[2](b) - by[2](a));
    $("stats-sum").textContent = `${names.length} ${st.kind} name${names.length === 1 ? "" : "s"}`;
    const th = (k, label, opt = "") => h("th", { class: `num sort${st.sort === k ? " on" : ""}${opt}`, onclick: () => { st.sort = k; saveState(); renderTable(); } }, label);
    const shown = statsAll ? names : names.slice(0, ROWS_SHOWN);
    tableBox.replaceChildren(h("table", null,
      h("thead", null, h("tr", null, h("th", null, "name"), th("n", "runs"), th("failed", "failed"), th("p50", "median"), th("p90", "p90"), th("max", "longest", " opt"), th("total", "total", " opt"), h("th", "opt", "recent"), h("th", "opt", "last"))),
      h("tbody", null, shown.map((g) => h("tr", null,
        h("td", { class: "nm", title: g.name }, g.name), h("td", { class: "num", title: g.skipped ? `and ${g.skipped} skipped or reused` : null }, g.n), h("td", `num${g.failed ? " bad" : ""}`, g.failed || ""),
        h("td", "num", dur(g.p50)), h("td", "num", dur(g.p90)), h("td", "num opt", dur(g.max)), h("td", "num opt", dur(g.total)),
        h("td", "opt", spark(g.recent)), h("td", { class: "num dim opt", title: g.last?.mac || "" }, g.last ? clock(g.last.t0, false) : ""))))),
      ...(names.length > shown.length ? [h("div", "more", h("button", { type: "button", onclick: () => { statsAll = true; renderTable(); } }, `show all ${names.length}`))] : []));
  }
  renderTable();
}

// ---- the header's controls ------------------------------------------------------------------------------
function renderControls() {
  $("ranges").replaceChildren(...RANGES.map(([k]) => h("button", { type: "button", class: st.range === k ? "on" : null, onclick: () => { st.range = k; saveState(); renderControls(); loadMain(); } }, k)));
  $("kinds").replaceChildren(...CLASSES.map((c) => h("label", { class: st.hide.has(c) ? null : "on", title: `${st.hide.has(c) ? "Show" : "Hide"} ${CLASS_LABEL[c]}` },
    h("input", { type: "checkbox", checked: !st.hide.has(c), onchange: () => { st.hide.has(c) ? st.hide.delete(c) : st.hide.add(c); saveState(); renderControls(); renderMain(); } }),
    h("i", { style: { background: colourOf(c) } }), CLASS_LABEL[c])));
}

// ---- live --------------------------------------------------------------------------------------------------
let again = null, lastLoad = 0;
function reload() {
  // (At most once in 3 s, however many Macs send.)
  if (again) return;
  again = setTimeout(async () => {
    again = null; lastLoad = Date.now();
    await loadMain().catch(showErr);
    if (st.run && (!runData || runData.run.status === "running" || runData.run.t1 == null || Date.now() / 1000 - runData.run.t1 < 120)) {
      const keep = runTl;
      runData = await get(`runs/${encodeURIComponent(st.run)}`).catch(() => runData);
      runTl = keep; renderRun();
    }
  }, Math.max(0, 3000 - (Date.now() - lastLoad)));
}
function showErr(e) { $("state").textContent = `can't reach the app: ${e.message}`; }
const es = new EventSource("../api/stream");
es.addEventListener("change", (e) => { if (JSON.parse(e.data).col === "jobs") reload(); });
// (Now moves on: the timelines with it, and a run not heard of goes lost.)
setInterval(reload, 30000);
new ResizeObserver(() => { const w = $("runs").clientWidth; if (main && Math.abs(w - lastWidth) > 40) renderMain(); }).observe($("runs"));

renderControls();
loadMain().catch(showErr);
loadStats().catch(showErr);
if (st.run) openRun(st.run);
