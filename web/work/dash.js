// The whole build at a glance (docs/workers.md, The page): the page at /work/, for anyone it answers.
// From the coordinator: /work/steps once (the steps table: every step's id, noun, stage and
// place, docs/pipeline.md), /work/swarm every 10 s (the lead's heartbeat with its checklist,
// forecast and resources, its helpers', every worker and lease, the hours' work), and
// /work/history after the last event this page has (what happened; further back for the
// schedule's past jobs).
//
// One shape for everything: a stage, step, job, target, part or phase is named as the code names it
// (task #659: no display names: `terrain-hi`, `terrain-deep 10/259/338`, `6/17/24`), a step's targets
// counted in its noun ("8 of 480 z6 tiles"), coloured by its stage; a stage is a line over its steps' rows, each row the same
// cells; a machine is a card of its jobs, each job the same lines; an event is one templated
// sentence. Progress is the work left in the queue, never a share of a whole (task #685; the Jobs
// tab's stages alone count done and left). The parts: the header's verdict (paused, out of touch
// or idle; done when; the pause); the world map in the left panel (worldmap.js: the queue, a step
// soloed by its legend, the catalogues' age controls: catalogues.js); by tab in the right one, the
// overview (the alerts, the work left, the last and next catalogue, the Macs), the jobs (each
// Mac's schedule, past and to come; the stages, done and left), the machines (what each does now,
// inside each job, why one waits, and live on each: live.js), the regions (lit on the map), and
// the activity (the hours, what happened, what each Mac moves now and the internet by website:
// activity.js), and the details.

import { poolSection, termText } from "./pool.js";
import { AREAS, TILE_STATES, areaOf, areaTargets, busyLanes, finished, finishedTiles, isSlotOf, laneGroups, mapTiles, notRead, packRuns, pastFrom, pastRuns, queuedTiles, runLinks, runNumbers, regionWork, regionsShown, rowsWindow, runningTiles, stageFill, slotName, slotNo, stageSegments, stepTotals, targetsOf, macSlot, tileCovers, tileOf } from "./sums.js";
import { fmt } from "./chartmath.js";
import { liveBlock, remote, startLive } from "./live.js";
import { createMap, tilesMap } from "./worldmap.js";
import { SPAN0, createTimeline } from "./timeline.js";
import { HIGHLIGHT, createAges, paint as agePaint, whenText } from "./catalogs.js"; // the route keeps its old spelling (task #547); the file is catalogues.js
import { loadSettings, settingsSection } from "./settings.js";
import { nasSection } from "./nas.js";
import { startActivity, transfersPart } from "./activity.js";

const POLL_MS = 10000;
// A pressed Pause or Resume: the page asks again this often until the build says so, for this long at most.
const PENDING_POLL_MS = 1000;
const PENDING_MAX_MS = 60000;
const STALE_S = 360;
const STUCK_S = 900;

// ---- Words and numbers ------------------------------------------------------------------------
const n = (v) => Number(v).toLocaleString("en-US");
const dur = (s) => {
  s = Math.max(0, Math.round(s));
  if (s < 90) return `${s} s`;
  const m = Math.round(s / 60);
  if (m < 90) return `${m} min`;
  const h = s / 3600;
  if (h < 36) return `${h < 10 ? h.toFixed(1) : Math.round(h)} h`;
  return `${(h / 24).toFixed(1)} days`;
};
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
// A time: "16:40" today, "Wed 07:50" this week, else "8 Oct 07:50".
function clock(t) {
  if (!t) return "–";
  const d = new Date(t * 1000), now = new Date();
  const hm = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  if (d.toDateString() === now.toDateString()) return hm;
  if (Math.abs(d - now) < 6 * 86400000) return `${DAYS[d.getDay()]} ${hm}`;
  return `${d.getDate()} ${d.toLocaleString("en-GB", { month: "short" })} ${hm}`;
}
const ago = (now, t) => (t ? `${dur(now - t)} ago` : "never");
const plural = (k, one, many = `${one}s`) => `${n(k)} ${k === 1 ? one : many}`;

// ---- The steps table (crates/pipeline/src/agent/steps.rs, served at /work/steps) ---------------
// Every word about a step comes from its row: the noun its targets are counted in, its stage (its
// colour) and where it runs; it's named by its id, as are stages.
// A stage's colour: its token in index.html (--st-<stage>, a hue each in either theme).
const stageColour = (g) => `var(--st-${g || "upkeep"}, var(--st-upkeep))`;
const table = { steps: {}, stages: [] };
const S = {
  row: (id) => table.steps[id],
  noun: (id) => table.steps[id]?.noun || "target",
  world: (id) => table.steps[id]?.scope === "world",
  stage: (id) => table.steps[id]?.stage || "",
  colour: (id) => stageColour(S.stage(id)),
  any: (id) => table.steps[id]?.place === "any",
  // "8 tiles"; a worldwide step's runs: "1 run".
  counted: (id, k) => plural(k, S.noun(id)),
  // "terrain-hi: 8 z6 tiles", "outlines" (a worldwide step's one run).
  said: (id, k) => (S.world(id) || !k ? id || "" : `${id}: ${S.counted(id, k)}`),
  // A job as the code names it (crate::agent's job ids): its step and first target, "terrain-deep
  // 10/259/338"; a worldwide step's, its step.
  job: (step, targets) => (targets?.length && !S.world(step) ? `${step} ${targets[0]}` : step || ""),
};
// The one word a waiting line's or notice's `what` is said by: its step, else its own (a stage's id).
const waitName = (w) => w.step || w.what;
// A step's line of the checklist (build::Item) or a stage's (build::Line) in words, done and left
// (the Jobs tab's stages: the one place the page counts a whole, task #685): "12 tiles done · 468
// left", "3 runs left", "done", "to come".
function countOf(x, noun) {
  if (x.total != null && x.total > 0) return x.done >= x.total ? `${n(x.total)} ${noun} done` : `${n(x.done)} ${noun} done · ${n(x.total - x.done)} left`;
  if (x.left === 0) return "done";
  if (x.left != null) return `${plural(x.left, noun.replace(/s$/, ""))} left`;
  return "to come";
}
// A stage's counts, a noun at a time in its items' order (build::Line::counts): "398 tiles done ·
// 18 z3 tiles done · 2 left", "2 runs left"; "done" or "to come" with no item to count (an older
// lead's line: its own count).
function countsOf(l) {
  const out = [];
  const nouns = [];
  for (const i of l.items || []) {
    const noun = S.noun(i.step);
    if (nouns.includes(noun)) continue;
    nouns.push(noun);
    const of = l.items.filter((j) => S.noun(j.step) === noun);
    if (of.some((j) => j.total != null)) {
      if (of.every((j) => j.total != null)) out.push(countOf({ done: of.reduce((t, j) => t + j.done, 0), total: of.reduce((t, j) => t + j.total, 0) }, `${noun}s`));
      else out.push(`${noun}s to come`);
    } else if (of.every((j) => j.left != null)) {
      const left = of.reduce((t, j) => t + j.left, 0);
      if (left) out.push(`${plural(left, noun)} left`);
    } else out.push(`${noun}s to come`);
  }
  if (!out.length) out.push(l.items?.length ? (finished(l) ? "done" : "to come") : countOf(l, l.unit || "targets"));
  return out.join(" · ");
}
const fraction = (x) => (x.total ? x.done / x.total : x.left != null ? (x.left ? 0 : 1) : 0);
// Machines' colours, the lead first.
const MACHINE_COLOURS = ["#5b8fd8", "#b48ae8", "#e0a36a", "#e07a9a", "#6cc28a"];

// ---- Elements -----------------------------------------------------------------------------------
function h(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  if (typeof attrs === "string") e.className = attrs;
  else if (attrs) for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "style") for (const [p, x] of Object.entries(v)) p.startsWith("--") ? e.style.setProperty(p, x) : (e.style[p] = x);
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) e.append(k instanceof Node ? k : document.createTextNode(String(k)));
  return e;
}
const pbar = (frac, cls = "", colour) => { const i = h("i"); i.style.width = `${Math.round(Math.max(0, Math.min(1, frac)) * 100)}%`; if (colour) i.style.background = colour; return h("div", `pbar ${cls}`, i); };
const chip = (text, cls = "", title) => h("span", { class: `chip ${cls}`, title }, text);
const swatch = (colour) => h("i", { class: "sw", style: { background: colour } });
// A step's phase shares (crate::timings::PhaseShares) in a line: its three largest phases by
// their mean share of a run's time, each with its share of the bytes its phases read and wrote
// (local disk, NAS and internet alike); "" before any run.
const phaseShares = (ps) => {
  if (!ps?.phases?.length) return "";
  const pc = (f) => (f > 0 && f < 0.005 ? "<1%" : `${Math.round(f * 100)}%`);
  return `${ps.phases.slice(0, 3).map((p) => `${p.name} ${pc(p.wall)}${p.bytes ? ` (${pc(p.bytes)} of I/O)` : ""}`).join(" · ")} · ${ps.runs} run${ps.runs === 1 ? "" : "s"}`;
};
// A step's mark: done, under way, to come.
const mark = (done, running) => h("span", `mark ${done ? "ok" : running ? "run" : ""}`, done ? "✓" : running ? "▸" : "○");
// A key, a value and a line under it: the overview's numbers, each the same.
const kpi = (k, v, s) => h("div", "kpi", h("div", "k", k), h("div", "v", v), s ? h("div", "s", s) : null);

// ---- The data's shape, older coordinators' too --------------------------------------------------
// (The gate's units, `a.inputs`: docs/inputs.md §4.7; none from an agent before it.)
function model(sw) {
  const a = sw.agent || {};
  const now = sw.now;
  const fresh = (beat) => beat && now - beat < STALE_S;
  const fc = a.forecast && { ...a.forecast, publishes: a.forecast.publishes ?? [] };
  const regionName = Object.fromEntries((a.regions || []).map((r) => [r.id, r.name]));
  // (Not the lead's own helper file, from while it was a member: it leads now.)
  const helpers = (a.helpers || []).filter((x) => x.beat && now - x.beat < 600 && x.host !== a.host);
  const pages = (sw.workers || []).filter((w) => w.kind === "web" && w.seen_s < 900);
  const leasesOf = (name) => (sw.leases || []).filter((l) => l.worker === name);
  // Every machine: the lead, its helpers, then the pages (one entry).
  const macs = [];
  if (a.host) macs.push({ name: a.host, role: "lead", status: a, fresh: fresh(a.beat), colour: MACHINE_COLOURS[0] });
  helpers.forEach((x, i) => macs.push({ name: x.host, role: "helper", status: x, fresh: fresh(x.beat), worker: (sw.workers || []).find((w) => w.name === x.host), colour: MACHINE_COLOURS[(i + 1) % MACHINE_COLOURS.length] }));
  const colourOf = (name) => macs.find((m) => m.name === name)?.colour || (pages.some((p) => p.name === name) ? "#6cc28a" : "#7b8590");
  const tasks = typeof sw.tasks === "object" && sw.tasks ? sw.tasks : { all: sw.tasks || 0 };
  // (A helper Mac that's stopped reporting (its status ten minutes old): the coordinator's word of
  // it, when it last asked.)
  const gone = (sw.workers || []).filter((w) => w.kind === "native" && w.name !== a.host && !helpers.some((x) => x.host === w.name));
  // The steps under way, on any machine: the lead's jobs, its helpers', the leases.
  const running = new Set([...macs.flatMap((x) => jobsOf(x.status)).map((j) => stepOf(j.id)), ...(sw.leases || []).map((l) => l.step)]);
  return { sw, a, now, fresh, fc, regionName, helpers, pages, macs, gone, leasesOf, colourOf, tasks, running, lines: a.checklist || [] };
}

const stepOf = (id) => String(id || "").split(" ")[0];
// A Mac's jobs, every slot's in order (crate::agent::Status::jobs: its `jobs`, else an older
// agent's job, beside, more), each with its slot (an older agent's says none: its place).
const jobsOf = (st) => (st.jobs?.length ? st.jobs : [st.job, st.beside, ...(st.more || [])].filter(Boolean)).map((j, i) => ({ ...j, slot: j.slot ?? i }));
// What the lead's coordinator says of a Mac, by its slots' names: its capacity (coord::Capacity).

// ---- The verdict --------------------------------------------------------------------------------
function machineState(st, fresh, paused) {
  if (!fresh) return ["out of touch", "bad"];
  // (Its jobs, every slot's: building while any is.)
  const jobs = jobsOf(st);
  if (jobs.some((j) => !j.paused && !j.pausing)) return ["building", "run"];
  // (This Mac paused alone, task #224: its jobs stopping, then nothing new.)
  if (st.held) return [jobs.length ? "pausing" : "paused", "warn"];
  if (jobs.some((j) => j.pausing)) return ["stopping", "warn"];
  if (jobs.some((j) => j.paused)) return ["frozen", "warn"];
  if (paused) return ["paused", "warn"];
  return ["idle", ""];
}

function alerts(m) {
  const out = [];
  const { a, now } = m;
  if (!a.host) return [{ cls: "bad", text: "No word from the lead yet", to: "machines" }];
  if (!m.fresh(a.beat)) out.push({ cls: "bad", text: `Lead out of touch for ${dur(now - a.beat)}`, to: "machines" });
  if (a.pool?.lead?.alert) out.push({ cls: "bad", text: a.pool.lead.alert, to: "machines" });
  for (const w of m.gone) if (w.seen_s < 86400) out.push({ cls: "warn", text: w.seen_s < 360 ? `${w.name}'s status is old (can it reach the NAS?)` : `${w.name} out of touch for ${dur(w.seen_s)}`, to: "machines" });
  for (const x of m.macs) {
    const st = x.status, r = st.resources || {}, c = st.conditions || {};
    const short = x.role === "lead" ? "Lead" : x.name;
    if (x.role !== "lead" && !x.fresh) out.push({ cls: "warn", text: `${short} out of touch for ${dur(now - st.beat)}`, to: "machines" });
    for (const j of jobsOf(st)) {
      const k = j.slot, p = j.progress, which = k ? `job in slot ${k + 1}` : "job";
      if (p?.moved_at && !j.paused && now - p.moved_at >= STUCK_S) out.push({ cls: "warn", text: `${short}'s ${which} hasn't moved on for ${dur(now - p.moved_at)}`, to: "machines" });
    }
    if (c.ac === false) out.push({ cls: c.battery != null && c.battery < 40 ? "warn" : "info", text: `${short} on battery${c.battery != null ? ` (${c.battery}%)` : ""}`, to: "machines" });
    if (c.nas === false) out.push({ cls: "bad", text: `${short} can't reach the NAS`, to: "machines" });
    else if (c.home === false) out.push({ cls: "info", text: `${short} reaches the NAS through Tailscale (slowly)`, to: "machines" });
    if (r.disk_free_gb != null && r.disk_free_gb < 20) out.push({ cls: r.disk_free_gb < 10 ? "bad" : "warn", text: `${short}: ${r.disk_free_gb} GB free on its disk`, to: "machines" });
    if (x.role !== "lead" && st.app && a.app && st.app !== a.app) out.push({ cls: "warn", text: `${short} runs another app (${st.app})`, to: "machines" });
  }
  // Failures in the last six hours (the lead's own jobs; the workers' are in the activity).
  const failed = (a.recent || []).filter((d) => !d.ok && now - d.ended < 6 * 3600);
  if (failed.length) out.push({ cls: "bad", text: `${plural(failed.length, "job")} failed in the last 6 h`, to: "activity" });
  for (const w of a.waiting || []) {
    if (/held for review/.test(w.why)) out.push({ cls: "warn", text: "Publishing held for review", to: "overview" });
    else if (/failed \d+ times?/.test(w.why)) out.push({ cls: "warn", text: `${waitName(w)}: waiting out a failure`, to: "details" });
  }
  // Things to know that hold nothing back (agent::Notice): an info line each, apart from the waits.
  for (const x of a.notices || []) out.push({ cls: "info", text: `${x.what}: ${x.why}`, to: "details" });
  for (const p of m.pages) {
    if (p.bad) out.push({ cls: "bad", text: `${p.label}: a result differed; it gets no more work`, to: "machines" });
    else if (p.visible === false && p.seen_s < 300) out.push({ cls: "warn", text: `${p.label} is in the background (iPhones and iPads stop it)`, to: "machines" });
  }
  if ((a.bad_recipes || []).length) out.push({ cls: "warn", text: `${plural(a.bad_recipes.length, "region recipe")} doesn't read`, to: "details" });
  for (const v of (a.inputs || []).filter((v) => v.state === "held")) out.push({ cls: (v.findings || []).some((f) => f.level === "error") ? "bad" : "warn", text: `${v.unit}: a change held`, to: "inputs" });
  return out;
}

// ---- The inputs folders' gate (docs/inputs.md §4.7): the Inputs tab -----------------------------
// A banner per held unit: its held files, each finding, Accept per warning and Accept All; errors
// say what to fix. A quiet line per unit being checked; a line saying so when none waits.
function inputsTab(m) {
  const accept = (body) => ctx.call("/work/inputs", body).then(poll, (e) => ctx.onError?.(`couldn't accept: ${e.message}`));
  const units = (m.a.inputs || []).filter((v) => v.state !== "ok");
  const held = units.filter((v) => v.state === "held").length;
  return section("d-inputs", "Inputs", held ? `${plural(held, "change")} held for a look` : "",
    units.length ? units.map((v) => gateBanner(v, accept)) : h("div", "box dim small", `Nothing held: ${plural((m.a.inputs || []).length, "inputs folder", "inputs folders")} taken in${(m.a.inputs || []).length ? ` (${m.a.inputs.map((v) => v.unit).join(", ")})` : ""}.`));
}
function gateBanner(v, accept) {
  if (v.state === "checking") return h("div", "gate checking", `${v.unit}: checking the files put in its inputs folder…`);
  const fs = v.findings || [];
  const warns = fs.filter((f) => f.level === "warning");
  const errors = fs.filter((f) => f.level === "error");
  const all = warns.length > 1 && h("button", { onclick: () => confirm(`Accept all ${warns.length} warnings of ${v.unit}?\n\n${warns.map((f) => `• ${f.message}`).join("\n")}`) && accept({ unit: v.unit, all: true }), title: "Accepts every warning held here; the held changes are taken in at the next check (errors stay)" }, "Accept all");
  return h("div", `gate${errors.length ? " has-error" : ""}`,
    h("div", "gh", h("b", null, v.unit), h("span", "files", `held: ${(v.held || []).join(", ")}`), h("span", "dim small", v.checking ? "checking again…" : v.checked ? `checked ${clock(v.checked)}` : ""), all),
    v.together ? h("div", "together", v.together) : null,
    fs.map((f) => h("div", "gf",
      chip(f.level, f.level === "error" ? "bad" : "warn"),
      h("span", "msg", f.message),
      f.level === "warning"
        ? h("button", { onclick: () => confirm(`Accept this warning of ${v.unit}?\n\n${f.message}`) && accept({ unit: v.unit, accept: [f.id] }), title: `Takes the change in with it (${f.id})` }, "Accept")
        : h("span", "dim small", "fix or remove"),
      h("div", "sub", [f.files.join(", "), f.at ? ` · at ${f.at[1].toFixed(4)}, ${f.at[0].toFixed(4)}` : "", ` · ${f.id}`].join("")),
      (f.lines || []).length ? h("pre", null, f.lines.map(([n, l]) => `${n}: ${l}`).join("\n") + (f.more ? `\n… and ${f.more} more lines (scenic inputs)` : "")) : null)));
}

function verdictText(m) {
  const { a, sw, now, fc } = m;
  const parts = [];
  if (!a.host) return [h("b", null, "No word from the lead yet.")];
  if (sw.pause) {
    const stopping = m.macs.filter((x) => jobsOf(x.status).some((j) => j.pausing)).length;
    parts.push(h("b", null, "Paused"), ` by ${sw.pause.by}${sw.pause.at ? `, ${ago(now, sw.pause.at)}` : ""}`);
    parts.push(sw.pause.mode === "freeze" ? " · every job frozen where it was" : stopping ? ` · ${plural(stopping, "job")} finishing what ${stopping === 1 ? "it's" : "they're"} on` : " · nothing running");
  } else if (!m.fresh(a.beat)) {
    parts.push(h("b", null, "Out of touch"), ` · the lead last said something ${ago(now, a.beat)} (asleep, off, or its agent stopped)`);
  } else {
    const working = m.macs.filter((x) => x.fresh && jobsOf(x.status).length).length + m.pages.filter((p) => (sw.leases || []).some((l) => l.worker === p.name)).length;
    if (!working) parts.push(h("b", null, "Idle"));
  }
  // (The queue's finish, never a share done: a build has no definite start, task #685.)
  if (fc?.done_at) parts.push(parts.length ? " · " : "", "done ≈ ", h("b", null, clock(fc.done_at)));
  else if (fc?.why) parts.push(parts.length ? " · " : "", fc.why);
  return parts;
}

// The next region (or regions, at the same time) to reach the map as it's built now: the forecast's
// earliest `map_at` (its last z6 tile's end, its map tiles drawn, a catalogue within the minute).
function nextOnMap(m) {
  const regs = (m.fc?.regions || []).filter((r) => r.map_at);
  if (!regs.length) return null;
  const at = Math.min(...regs.map((r) => r.map_at));
  return { at, regions: regs.filter((r) => r.map_at === at) };
}

// The live error beside the finish (task #563): how the forecast of an hour or more ago fared
// against the work done since, and what each Mac was held to over the last day.
function liveError(fc) {
  const p = fc.pace;
  const err = p ? `live error (rough, ±50%): the forecast of ${clock(p.since)} did the ${p.real_n} targets done since in ${dur(p.run_s)}, not ${dur(p.real_s)} (${p.error > 0 ? "+" : ""}${Math.round(p.error * 100)}%)` : "live error (rough): not yet measured (a forecast an hour old is needed)";
  const held = Object.entries(fc.held || {}).map(([mac, x]) => `${mac} ${x.lanes} job${x.lanes > 1 ? "s" : ""} at ${Math.round(x.util * 100)}%`);
  return held.length ? `${err} · held to the last day's pace: ${held.join(", ")}` : err;
}

// ---- The overview ---------------------------------------------------------------------------------
// The work left in the queue (task #685: never a share of a whole; a build has no definite start,
// its keys invalidate and catalogues publish as it goes): the forecast's work left and its finish,
// the stages with work left as a strip (each its share of the queue, its card on hover), the tiles
// queued, under way and not yet published; then the numbers that aren't a step's: the last and next
// catalogue, the machines.
function overview(m, md) {
  const { fc, now, lines } = m;
  const work = (fc?.steps || []).reduce((t, x) => t + (x.work_s || 0), 0);
  const left = h("div", "big", !fc ? "–" : work ? `≈ ${dur(work)}` : "Nothing",
    h("small", null, !fc ? "no forecast yet" : `${work ? "of work in the queue" : "left in the queue"}${fc.done_at ? ` · done ≈ ${clock(fc.done_at)}` : fc.why ? ` · ${fc.why}` : ""}`));
  // (A stage's work left, its steps' in the forecast.)
  const byStage = new Map();
  for (const x of fc?.steps || []) if (x.work_s > 0) byStage.set(S.stage(x.step), (byStage.get(S.stage(x.step)) || 0) + x.work_s);
  const order = table.stages.map((g) => g.id);
  const queue = [...byStage].sort(([p], [q]) => (order.indexOf(p) + 1 || 99) - (order.indexOf(q) + 1 || 99));
  const strip = h("div", "strip", queue.map(([g, w]) => {
    const l = lines.find((x) => x.id === g);
    const running = (l?.steps || []).some((s) => m.running.has(s));
    const i = h("i", { class: running ? "now" : "", style: { flex: `${w} 1 0`, background: stageColour(g) }, title: `${g}: ≈ ${dur(w)} of work` });
    return l ? withCard(i, `s ${g}`, () => stageCard(m, l, false)) : i;
  }));
  const t = md?.whole?.tile;
  const nowLines = lines.filter((l) => !finished(l) && (l.steps || []).some((s) => m.running.has(s))).map((l) => l.id);
  const queueLine = h("div", "small dim", [t ? `${plural(t.queued, "tile")} queued · ${n(t.progress)} in progress · ${n(t.completed)} not yet published` : "", nowLines.length ? `now: ${nowLines.join(", ")}` : ""].filter(Boolean).join(" · "));
  const cat = lastCatalogue(m);
  const nextUpdate = nextOnMap(m);
  // Each Mac by name, a line each, with what it's doing.
  const doing = (x) => { const k = jobsOf(x.status).length; return !x.fresh ? "out of touch" : k ? plural(k, "job") : "idle"; };
  const kpis = h("div", "kpis",
    kpi("Last catalogue", cat ? `${clock(cat.at)} (${ago(now, cat.at)})` : "–", cat?.n != null ? `catalogue ${cat.n}${cat.kind ? `, ${cat.kind}` : ""}` : null),
    kpi("Next catalogue", nextUpdate ? `≈ ${clock(nextUpdate.at)}` : "–", m.a.publishing?.tiles ? `${plural(m.a.publishing.tiles, "map tile")} to draw` : null),
    kpi("Machines", h("div", "macs", m.macs.map((x) => h("div", null, x.name, h("span", "small dim", ` · ${doing(x)}`)))), m.pages.length ? `and ${plural(m.pages.length, "page")}` : null),
  );
  return h("div", "overview", h("div", "box", left, queue.length ? strip : null, queueLine), kpis);
}

// ---- The world map (worldmap.js), the left panel ---------------------------------------------------
// The countries' outlines (countries.json, one of the page's files), fetched once (again on a
// render after a failure).
let countries = [];
let countriesAsked = false;
function loadCountries() {
  if (countriesAsked) return;
  countriesAsked = true;
  fetch("countries.json").then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status))))).then((c) => { countries = c; render(); }, () => { countriesAsked = false; });
}
let worldMap = null, ages = null;
// A region's state on the map, as the Regions tab says it: on the map (nothing left), done (on
// the map with the next catalogue), building (first in line), to come; none without a forecast.
function regionState(r) {
  if (!r) return "";
  const left = Object.values(r.left || {}).some((k) => k);
  return !left ? (r.on_map === true ? "on" : "done") : r.rank === 0 ? "building" : "queued";
}
// Every running job, the lead's and its helpers' (their status's `jobs`) and the leased (the
// coordinator's leases), with its targets (the history's word: sums.js targetsOf), where it runs,
// its live data and, for where that's missing, its share done (a job's progress, a lease's beat).
function mapRuns(m) {
  const runs = [
    ...m.macs.flatMap((x) => jobsOf(x.status).map((j) => ({ step: stepOf(j.id), id: j.id, started: j.started, live: j.live, frac: j.progress?.total ? j.progress.done / j.progress.total : null, what: j.what, mac: x.name }))),
    ...(m.sw.leases || []).map((l) => ({ step: l.step, lease: l.id, what: l.what, live: l.live, frac: l.frac, mac: l.worker })),
  ];
  for (const r of runs) r.targets = areaTargets(r.step, targetsOf(r, events));
  return runs;
}
// The map's view (#615, #627, #685), this browser's own (ctx.store "mapView"): the tiles and the
// regions by state (`tiles`, `regions`: those toggled off in the legend, false; published tiles off
// at first), one pool member's, the tile areas toggled off in the legend (`offAreas`: sums.js
// AREAS' names), and the step or stage a click holds soloed (`solo`: {step} or {stage}); the one
// the pointer is on solos it meanwhile (`hoverSolo`).
const MAP_F = { tiles: { published: false }, regions: {}, member: "", offAreas: [], solo: null };
let mapF = null, hoverSolo = null;
const mapFilters = () => mapF || (mapF = { ...MAP_F, ...(ctx.store.get("mapView", {}) || {}) });
function setMapFilter(k, v) { mapFilters()[k] = v; ctx.store.set("mapView", mapF); render(); }
const soloKey = (x) => (!x ? "" : x.step != null ? `step ${x.step}` : `stage ${x.stage}`);
const soloOf = (key) => (!key ? null : key.startsWith("step ") ? { step: key.slice(5) } : { stage: key.slice(6) });
// The steps a solo shows: a step's own; a stage's, its steps the map has.
const soloSteps = (x, groups) => (!x ? null : x.step != null ? [x.step] : groups.find((g) => g.stage === x.stage)?.steps || []);

// The queue (#622, #685): every tile the lead's last forecast places, on any Mac (`/work/tocome`:
// forecast::ToCome), asked once a forecast (the status's `fc.at`). This Mac's agent answers when it
// made the forecast the status shows (the lead's: a Mac that led before keeps its last, older one);
// else the lead's own, at its addresses. A lead change drops the old lead's answer at once.
let toCome = { key: "", lead: "", ans: null, asking: false, st: {} };
function askToCome(m) {
  const key = `${m.a.host}@${m.fc?.at ?? ""}`;
  if (toCome.lead !== m.a.host) toCome = { ...toCome, lead: m.a.host, ans: null, st: {} };
  if (!m.fc || toCome.key === key || toCome.asking) return;
  toCome.asking = true;
  const lead = (last?.macs || []).find((x) => x.host === m.a.host);
  // (A member's agent answers 503: it made no forecast.)
  ctx.call("/work/tocome", {}).catch(() => [0, null]).then(([code, v]) => (code === 200 && v && v.at === m.fc?.at ? v : lead ? remote(lead, toCome.st, {}, "/work/tocome") : null))
    .catch(() => null)
    .then((v) => { toCome = { ...toCome, key, ans: toCome.lead === m.a.host ? v : null, asking: false }; render(); });
}
// Every tile the build's keys record as built, by step (`/work/built`: crate::agent::builtview,
// each Mac's agent reading the NAS's), asked each `BUILT_EVERY_MS` while the map is drawn.
const built = { steps: {}, at: 0, asked: 0, asking: false };
const BUILT_EVERY_MS = 60000;
function askBuilt() {
  if (built.asking || Date.now() - built.asked < BUILT_EVERY_MS) return;
  Object.assign(built, { asking: true, asked: Date.now() });
  ctx.call("/work/built", {}).catch(() => [0, null])
    .then(([code, v]) => { built.asking = false; if (code === 200 && v?.steps && v.at !== built.at) { Object.assign(built, { steps: v.steps, at: v.at }); render(); } });
}

// The world map's data for this render: the running jobs' tiles, the queue, the finished tiles
// (sums.js) and the regions; the tiles counted by state, step and area (`whole`: all of them, the
// Overview's; `counts`: the member chosen's, the legend's); the regions' cards. Drawn as the view
// says by `mapView`, again without a render as a solo or the age controls change (`paintMap`).
let lastMd = null;
// (The history's job ends the map takes a finished tile from, beside the keys' (`/work/built`): the
// last 6 h's.)
const MAP_ENDS_S = 6 * 3600;
function mapData(m) {
  loadCountries();
  askBuilt();
  askToCome(m);
  const f = mapFilters();
  const from = m.now - MAP_ENDS_S;
  const runs = mapRuns(m);
  // (Jobs with no tile to show, a worldwide step's: named under the map, as the mockup does.)
  // (A job and its lease are one run: named once.)
  const world = [...new Map(runs.filter((r) => !r.targets.some((t) => tileOf(t))).map((r) => [`${r.step} ${r.mac}`, r])).values()];
  const hosts = m.macs.map((x) => x.name);
  // (In the last catalogue: what finished by its job's start, when it took what was done; the
  // record's time is its end.)
  const catAt = lastCatalogue(m)?.at ?? null;
  const catJob = catAt == null ? null : (m.a.recent || []).find((d) => /^catalog/.test(stepOf(d.id)) && d.ok && Math.abs(d.ended - catAt) <= 5);
  const pubAt = catJob?.secs != null ? catAt - catJob.secs : catAt;
  const macOf = (w) => macSlot(w, hosts).host;
  const running = runningTiles(runs, m.fc?.each_s, m.now), queued = queuedTiles(toCome.ans), done = finishedTiles(endsHeld(), from, built.steps, heldFrom());
  // (A step counts its tiles to build or publish; every state and area its own.)
  const count = (t) => {
    const c = { tile: Object.fromEntries(TILE_STATES.map((st) => [st, 0])), step: {}, area: {} };
    for (const [k, v] of [...t.running, ...t.queued, ...t.finished]) {
      c.tile[v.state]++;
      c.area[areaOf(k)] = (c.area[areaOf(k)] || 0) + 1;
      if (v.state !== "published") c.step[v.step] = (c.step[v.step] || 0) + 1;
    }
    return c;
  };
  const whole = count(mapTiles(running, queued, done, {}, { pubAt, macOf }));
  const counts = f.member ? count(mapTiles(running, queued, done, { member: f.member }, { pubAt, macOf })) : whole;
  const fcRegs = Object.fromEntries((m.fc?.regions || []).map((r) => [r.id, r]));
  const regs = Object.values(drawn.byId).map((r) => ({ ...r, state: regionState(fcRegs[r.id]) }));
  const rows = regionRows(m);
  regionCards(rows);
  return (lastMd = { m, regs, rows, world, running, queued, done, pubAt, macOf, whole, counts, groups: mapSteps(counts.step) });
}

// What the map draws now (sums.js mapTiles): the view's tiles; a solo's published tiles painted
// by when a catalogue last published each (the age controls' range, catalogues.js); a catalogue's
// tiles lit; with each tile's card and the stack's.
function mapView(md) {
  const f = mapFilters(), m = md.m, age = ages.view();
  const t = mapTiles(md.running, md.queued, md.done, f, { pubAt: md.pubAt, macOf: md.macOf, solo: soloSteps(hoverSolo || f.solo, md.groups) });
  const when = new Map();
  for (const [k, v] of t.published) when.set(k, { t: age.at(tileOf(k))?.t ?? -Infinity, v });
  const painted = agePaint(when, age.from, age.to, age.hide).paint;
  tileCards(m, t, painted, age);
  const regionKind = (st) => (st === "done" ? "on" : st);
  return { countries, regions: md.regs.filter((r) => f.regions?.[regionKind(r.state)] !== false), running: t.running, finished: t.finished, queued: t.queued, paint: painted, lit: schedLit || age.lit, litColour: HIGHLIGHT, colour: S.colour };
}
// The map drawn again from the last render's data: a solo hovered or the age controls changed.
function paintMap() {
  if (!worldMap || !lastMd || !ages) return;
  worldMap.update(mapView(lastMd));
  // (The legend says what's soloed: the item, a stage's steps with it.)
  const x = hoverSolo || mapFilters().solo, steps = soloSteps(x, lastMd.groups) || [];
  for (const b of legendHost?.querySelectorAll("button[data-solo]") || []) b.classList.toggle("soloed", b.dataset.solo === soloKey(x) || (b.classList.contains("step") && steps.includes(b.dataset.name)));
  legendHost?.classList.toggle("soloing", !!x);
}

// Each tile's card, as the map shows it, and the stack's: everything under a point, one card after
// another.
function tileCards(m, t, painted, age) {
  const head = (key, step, state) => ({ kicker: `${step} · ${state}`, colour: S.colour(step), title: key.slice(key.lastIndexOf(" ") + 1) });
  for (const [key, r] of t.running) {
    const tl = tileOf(key), pct = `${Math.round(r.frac * 100)}%`;
    const state = r.frac >= 1 ? "completed, its job running" : r.current || r.frac > 0 ? "in progress" : "queued in its running job";
    cards.set(`mt ${key}`, () => cardBody({ ...head(key, r.step, state),
      stats: [[pct, "of the tile done"], [r.current ? r.run.live?.phase || "running" : r.frac >= 1 ? "done" : r.frac > 0 ? "part done" : "queued", "phase"]],
      bar: { frac: r.frac, label: r.frac >= 1 ? "done; its job runs on" : r.frac > 0 ? `≈ ${pct} of the tile` : "not begun" },
      foot: `${r.run.mac} · ${key}` }, [
      ["stage", S.stage(r.step)],
      ["job", S.job(r.step, r.run.targets)],
      ["on", r.run.mac],
      ["covers", tileCovers(tl)],
    ], worldMap.minimap(key)));
  }
  for (const [key, d] of t.finished) {
    const out = d.state === "published";
    cards.set(`mt ${key}`, () => cardBody({ ...head(key, d.step, out ? "published" : "completed, not yet published"),
      stats: [[d.t == null ? "earlier" : ago(m.now, d.t), "finished"], [out ? "yes" : "next catalogue", "in the app"]],
      bar: { frac: 1, label: d.t == null ? "done, before the history held" : `done ${clock(d.t)}` },
      foot: `${d.worker || "?"} · ${key}` }, [
      ["stage", S.stage(d.step)],
      ["on", d.worker],
      ["covers", tileCovers(tileOf(key))],
    ], worldMap.minimap(key)));
  }
  for (const [key, d] of t.queued) {
    cards.set(`mt ${key}`, () => cardBody({ ...head(key, d.step, "queued"),
      stats: [[`≈ ${clock(d.at)}`, "to begin"], [d.mac, "on"]],
      bar: { frac: 0, label: `to come at ≈ ${clock(d.at)} on ${d.mac}` },
      foot: `the forecast's · ${key}` }, [
      ["stage", S.stage(d.step)],
      ["on", d.mac],
      ["covers", tileCovers(tileOf(key))],
    ], worldMap.minimap(key)));
  }
  for (const [key, d] of t.published) {
    if (!painted.has(key)) continue;
    const p = age.at(tileOf(key));
    cards.set(`mt ${key}`, () => cardBody({ ...head(key, d.step, "published"),
      stats: [[p ? ago(m.now, p.t) : "–", "last published"], [p ? `${p.n}` : "–", "catalogue"]],
      bar: { frac: 1, colour: painted.get(key), label: p ? `catalogue ${p.n}, ${whenText(p.t)}${p.first ? " (the oldest kept: or before)" : ""}` : "in no catalogue kept" },
      foot: `${d.worker || "?"} · ${key}` }, [
      ["stage", S.stage(d.step)],
      ["on", d.worker],
      ["covers", tileCovers(tileOf(key))],
    ], worldMap.minimap(key)));
  }
  // (The stack: `also` its keys, "|" between, the deepest tile first; each card under a line.)
  cards.set("mstack", (also) => {
    const keys = (also || "").split("|").filter((k) => cards.has(k));
    const most = 8;
    return [...keys.slice(0, most).map((k) => h("div", "tc-sec", cards.get(k)())),
      keys.length > most ? h("div", "tc-more dim small", `and ${n(keys.length - most)} more here`) : null].filter(Boolean);
  });
}
// A map's click or tap: its stack pinned (nothing there: the pinned card let go).
const mapPick = (keys, x, y) => (keys.length ? pinCard("mstack", x, y, keys.join("|")) : unpin());
// The map's legend (#627, #659, #685): what's drawn, counted: the regions and tiles by state, the
// tile areas (sums.js AREAS: root, lo, hi, deep, each its outline's weight), each a toggle showing
// or hiding its kind (off: struck and dimmed), hovered: the rest of the map faint; and every step
// that draws tiles, grouped by its stage, each a solo: hovered, that step alone on the map while
// the pointer's on it (a stage: its steps), with its published tiles by age; a click holds it until
// clicked again. Then the member filter. Every step and stage by its id.
const TILE_WORDS = { queued: "queued", progress: "in progress", completed: "completed, not yet published", published: "published" };
// The steps the map draws tiles of, by stage in the build's order: a z3, z6 or z10 tile's, or a
// root's (sums.js areaTargets); then any other step it has tiles of. [{stage, steps}].
function mapSteps(stepCount) {
  const tiled = (r) => !r.of && (["area", "tile", "z10"].includes(r.scope) || areaTargets(r.id, [])[0]);
  const rows = Object.values(table.steps).filter(tiled);
  for (const id of Object.keys(stepCount)) if (!rows.some((r) => r.id === id)) rows.push({ id, stage: S.stage(id) });
  const order = table.stages.map((g) => g.id);
  const stages = [...new Set(rows.map((r) => r.stage || ""))].sort((p, q) => (order.indexOf(p) + 1 || 99) - (order.indexOf(q) + 1 || 99));
  return stages.map((g) => ({ stage: g, steps: rows.filter((r) => (r.stage || "") === g).map((r) => r.id) }));
}
function mapLegend(md) {
  const { regs, counts, world, groups } = md;
  const f = mapFilters(), map = worldMap;
  const count = (st) => regs.filter((r) => r.state === st || (st === "on" && r.state === "done")).length;
  // (`name`: a stage's, step's or area's code name, the item's `data-name`: touch-test.mjs checks each is one.)
  const toggle = (on, flip, emph, swatch, text, title, cls = "", name = null) => h("button", { type: "button", class: `lg${cls}${on ? "" : " off"}`, "aria-pressed": on ? "true" : "false", title, "data-name": name,
    onclick: () => { map.emphasize(null); flip(); }, onpointerenter: (e) => e.pointerType !== "touch" && map.emphasize(emph), onpointerleave: () => map.emphasize(null) }, swatch, text);
  const byState = (k, st, cls, text) => toggle(f[k][st] !== false, () => setMapFilter(k, { ...f[k], [st]: f[k][st] === false }), { [k === "tiles" ? "tile" : "region"]: st }, h("i", cls), text, "Show or hide these on the map");
  const offA = new Set(f.offAreas || []);
  const held = soloKey(f.solo);
  // (Hovered: soloed while the pointer's on it, `legendHost`'s handlers; a click holds it or lets it go.)
  const solo = (x, swatch, text, title, cls, name) => h("button", { type: "button", class: `lg ${cls}${soloKey(x) === held ? " held" : ""}`, "aria-pressed": soloKey(x) === held ? "true" : "false", title, "data-name": name, "data-solo": soloKey(x),
    onclick: () => setMapFilter("solo", soloKey(x) === soloKey(mapFilters().solo) ? null : x) }, swatch, text);
  const sw = (g) => h("i", { style: { background: stageColour(g), borderColor: "transparent" } });
  return [h("div", "wmap-legend small dim",
    regs.length ? [byState("regions", "on", "rg on", `${plural(count("on"), "region")} on the map`), byState("regions", "building", "rg building", `${plural(count("building"), "region")} building`), byState("regions", "queued", "rg queued", `${plural(count("queued"), "region")} to come`)] : h("span", null, "No regions' outlines from this Mac yet"),
    TILE_STATES.map((st) => byState("tiles", st, `tl ${st}`, `${plural(counts.tile[st], "tile")} ${TILE_WORDS[st]}`)),
    world.length ? h("span", null, `whole world: ${world.map((r) => `${r.step} (${r.mac})`).join(", ")}`) : null,
    h("span", "hint", "drag to move · pinch, double-click or +/− to zoom")),
  h("div", { class: "wmap-legend wmap-areas small dim", role: "group", "aria-label": "Tile areas" }, h("span", "lgh", "tile areas"),
    AREAS.map(([a, z]) => toggle(!offA.has(a), () => setMapFilter("offAreas", offA.has(a) ? [...offA].filter((x) => x !== a) : [...offA, a]), { area: a },
      h("i", `ar ${a}`), `${a} z${z} · ${n(counts.area[a] || 0)}`, `Show or hide the ${a} packs' tiles (z${z}) on the map`, " area", a))),
  h("div", { class: "wmap-legend wmap-steps small dim", role: "group", "aria-label": "Steps by stage: hover one to solo it, click to hold it" },
    groups.map(({ stage, steps }) => h("span", { class: "lgg", "data-stage": stage },
      solo({ stage }, sw(stage), h("b", null, stage || "–"), `${stage}'s steps alone on the map, their published tiles by when (a click holds it)`, "stage", stage),
      steps.map((id) => solo({ step: id }, sw(stage), `${id} ${n(counts.step[id] || 0)}`, `${id} alone on the map, its published tiles by when (a click holds it)`, "step", id))))),
  h("div", "wmap-legend small dim", h("span", null, `counts are tiles (a step's: to build or publish) · queued: faint, dotted · running: outlined, filled as far as done · completed, not yet published: dashed · published: solid · a step soloed: its published tiles by when, below · steps at one spot: stripes · outline weight: the tile area`)),
  mapFilterBar(md.m)];
}
// The member filter under the map: a member's tiles alone (those it ran, the history's ends; runs
// in its running jobs and leases; and the queue's it will run).
let mapFiltersN = 0;
function mapFilterBar(m) {
  const f = mapFilters(), id = ++mapFiltersN;
  const radio = (k, v, text) => h("label", "mf", h("input", { type: "radio", name: `mf-${k}-${id}`, value: v, checked: f[k] === v ? "" : null, onchange: () => setMapFilter(k, v) }), text);
  const members = [...new Set([...m.macs.map((x) => x.name), ...(f.member ? [f.member] : [])])];
  return h("div", "wmap-filters small",
    h("div", { class: "mfrow", role: "radiogroup", "aria-label": "Show only for" }, h("span", "dim", "Show only for:"), radio("member", "", "All pool members"), members.map((nm) => radio("member", nm, nm))));
}
// The left panel (#685): the map, its legend (made again each render), the age controls
// (catalogues.js); one element, kept across renders.
let mapRoot = null, legendHost = null;
function mapSection(md) {
  if (!mapRoot) {
    worldMap = createMap({ pick: mapPick });
    ages = createAges({ call: ctx.call, store: ctx.store, onChange: paintMap });
    legendHost = h("div", "wmap-lgs");
    // (A solo hovered: the item under the pointer; none once it leaves the legend. A touch solos
    // nothing: its tap holds one.)
    legendHost.addEventListener("pointerover", (e) => {
      if (e.pointerType === "touch") return;
      const x = soloOf(e.target.closest?.("button[data-solo]")?.dataset.solo);
      if (soloKey(x) !== soloKey(hoverSolo)) { hoverSolo = x; paintMap(); }
    });
    legendHost.addEventListener("pointerleave", () => { if (hoverSolo) { hoverSolo = null; paintMap(); } });
    mapRoot = h("section", { class: "d-map", "aria-label": "World map" }, worldMap.el, legendHost, ages.el);
  }
  legendHost.replaceChildren(...mapLegend(md));
  paintMap();
  return mapRoot;
}

// ---- Machines -----------------------------------------------------------------------------------
// A job's progress in words: "40% · 2.4 of 6 tiles · about 12 min left" (one item alone, its share).
function progressText(p, j, now) {
  const pct = Math.floor((p.done / p.total) * 100);
  // (One item whose share is said: "the planet filtered"; else "0.4 of 1 tile", its noun singular.)
  const unit = p.total === 1 ? p.unit.replace(/^((?:z\d+ )?\w+?)s\b/, "$1") : p.unit;
  const amount = p.total === 1 && /^the /.test(p.unit) ? p.unit : `${Number.isInteger(p.done) || p.total > 100 ? n(Math.floor(p.done)) : (Math.floor(p.done * 10) / 10).toFixed(1)} of ${n(p.total)} ${unit}`;
  const eta = p.eta_s != null && !j.paused ? ` · about ${dur(p.eta_s)} left (≈ ${clock(now + p.eta_s)})` : "";
  return `${pct}% · ${amount}${eta}`;
}

// A job, as every machine's is shown: its name (its step's title and targets), its parts, its
// progress, how long it's run and what it holds, and why it's stopped or stuck.
// A page on any device may cancel a job or try a failed target again, through its Mac's agent
// (/work/act).
async function act(body, what) {
  try {
    const [code, r] = await ctx.call("/work/act", body);
    if (code !== 200) throw new Error(r?.error || `answered ${code}`);
    ctx.onState?.(`asked to ${what}`);
    poll();
  } catch (e) {
    ctx.onError?.(`couldn't ask to ${what}: ${e.message}`);
  }
}
// A job's step and target, as its id says them (one target alone: "step target").
function jobTarget(j) {
  const w = (j.id || "").split(" ");
  return w.length === 2 ? { step: w[0], target: w[1] } : null;
}

// Inside a running job (its status's `live`, crate::agent::jobs::Live; docs/pool.md §10, Live): the
// phase its main thread is in and since when, the target under way against the step's time a
// target (the forecast's `each_s`), its targets ended, and the bytes it moved on the NAS and the
// internet since it began. Each part missing isn't said (a step that times no phases, an older agent).
function inside(j, now, fc) {
  const l = j.live;
  if (!l) return null;
  const rows = [];
  if (l.phase) rows.push(h("div", null, h("span", "k", "phase"), h("span", null, h("b", null, l.phase), l.class ? h("span", "dim", ` ${l.class}`) : null, l.phase_since ? h("span", "dim", ` · ${dur(now - l.phase_since)}`) : null)));
  if (l.target) {
    const each = fc?.each_s?.[stepOf(j.id)];
    const took = l.target_since ? now - l.target_since : null;
    const over = each && took != null ? took / each : null;
    rows.push(h("div", null, h("span", "k", "target"), h("span", null, h("b", null, l.target),
      h("span", over != null && over > 2 ? "warn" : "dim", took != null ? ` · ${dur(took)}${each ? ` of ≈ ${dur(each)}` : ""}` : ""),
      l.targets ? h("span", "dim", ` · ${n(l.targets_done)} of ${n(l.targets)} done`) : null)));
    if (over != null) rows.push(pbar(over, over > 2 ? "warn" : "", S.colour(stepOf(j.id))));
  } else if (l.targets) rows.push(h("div", null, h("span", "k", "targets"), h("span", null, `${n(l.targets_done)} of ${n(l.targets)} done`)));
  const bytes = [l.nas_read != null ? `NAS ↓ ${fmt(l.nas_read, "B")}` : null, l.nas_wrote != null ? `NAS ↑ ${fmt(l.nas_wrote, "B")}` : null,
    l.net_in != null ? `net ↓ ${fmt(l.net_in, "B")}` : null, l.net_out != null ? `net ↑ ${fmt(l.net_out, "B")}` : null].filter(Boolean);
  if (bytes.length) rows.push(h("div", null, h("span", "k", "moved"), h("span", null, bytes.join(" · "))));
  return rows.length ? h("div", { class: "inside", title: "Inside the job: its phase, its target under way (against the step's time a target), and the bytes it moved since it began" }, rows) : null;
}

function jobBlock(j, now) {
  const p = j.progress;
  const step = stepOf(j.id);
  const box = h("div", "job", h("div", "what", swatch(S.colour(step)), j.id));
  // (Only this job: no bars of its step's parts, the owner's 21 Oct 02:46 and 03:11; its words.)
  if (p && p.total > 0) box.append(h("div", "sub", progressText(p, j, now)));
  const bits = [`running ${dur(now - j.started)} (since ${clock(j.started)})`];
  if (j.threads) bits.push(`${j.threads} threads`);
  if (j.mem_mb) bits.push(`${n(Math.round(j.mem_mb / 102.4) / 10)} GB`);
  box.append(h("div", "sub", bits.join(" · ")));
  if (p?.moved_at && !j.paused && now - p.moved_at >= STUCK_S) box.append(h("div", "why", `No further for ${dur(now - p.moved_at)}: it may be stuck (its log's last lines are in the details)`));
  if (j.paused) box.append(h("div", "why", `Frozen: ${j.paused}`));
  else if (j.pausing) box.append(h("div", "why", "Stopping at its next safe point (what it's on is kept)"));
  const t = jobTarget(j);
  if (t) box.append(h("div", "ctl", h("button", { class: "quiet", title: "Stopped now and given back to the build (not a failure); it runs again from the start", onclick: () => confirm(`Cancel ${j.id}?`) && act({ cancel: t }, `cancel ${j.id}`) }, "Cancel")));
  return box;
}

// What the lead offered this Mac, per step, folded to a line (its steps and how many of their
// pieces it can take); the steps on a click, kept open across renders.
const fitOpen = new Set();
function fitText(w, host) {
  const offered = (w?.fit || []).filter((f) => f.offered);
  if (!offered.length) return null;
  const all = offered.reduce((s, f) => s + f.offered, 0), fits = offered.reduce((s, f) => s + f.fits, 0);
  const rows = offered.map((f) => {
    const bits = [`${f.step}: ${n(f.fits)} of ${n(f.offered)} ${S.noun(f.step)}s it can take`];
    if (f.held) bits.push(`${n(f.held)} with others`);
    if (f.too_big) bits.push(`${n(f.too_big)} too big for its ${n(Math.round(w.mem_mb / 102.4) / 10)} GB`);
    if (f.kept_from) bits.push(`${n(f.kept_from)} kept from it after failing`);
    return h("div", null, bits.join(", "));
  });
  const d = h("details", { class: "sub fit", open: fitOpen.has(host), ontoggle: () => (d.open ? fitOpen.add(host) : fitOpen.delete(host)) },
    h("summary", null, `Offered ${plural(offered.length, "step")}: it can take ${n(fits)} of ${n(all)} pieces`), ...rows);
  return d;
}

// A machine's card, each the same: who and how it is; its capacity; a block per running job, by
// slot, with what's inside it (or why none runs); live on it (live.js).
function machineCard(m, x) {
  const st = x.status, now = m.now;
  const [state, cls] = machineState(st, x.fresh, m.sw.pause);
  const jobs = jobsOf(st);
  const role = x.role === "lead" ? "lead · coordinator" : "member";
  const card = h("div", { class: `mc ${x.fresh ? "" : "off"}`, style: { borderLeft: `3px solid ${x.colour}` } },
    h("div", "top", h("span", "name", x.name), h("span", "role", role), st.app ? chip(`app ${st.app}`) : null, chip(state, cls), jobs.length ? chip(plural(jobs.length, "job")) : null, h("span", "right", `heard from ${ago(now, st.beat)}`)));
  jobs.forEach((j) => {
    // (Its real slot, slots may be sparse: jobsOf.)
    const b = jobBlock(j, now);
    b.querySelector(".what").prepend(h("span", "slot", `${j.slot + 1}`));
    card.append(b);
  });
  if (!jobs.length) {
    // Why it isn't building.
    // (Paused alone or with the build: its own waits say the same again, left out; the same words once.)
    const why = st.held || m.sw.pause ? [] : [...new Map((st.waiting || []).filter((w) => !w.step || x.role !== "lead").map((w) => [x.role === "lead" ? `${waitName(w)}: ${w.why}` : w.why, w])).values()].slice(0, 2);
    card.append(h("div", "job", h("div", "what dim", st.held ? `This Mac is paused (from ${st.held.by}); the rest of the pool goes on` : m.sw.pause ? "Paused with the build" : why.length ? "Waiting" : "Nothing to build now")));
    for (const w of why) card.append(h("div", "sub", x.role === "lead" ? `${waitName(w)}: ${w.why}` : w.why));
  }
  card.append(...[x.role === "helper" ? fitText(x.worker, x.name) : null, liveBlock(x.name, st.memory?.last)].filter(Boolean));
  return card;
}

function goneCard(w) {
  // (Still asking the coordinator, its own status not written lately: the NAS, likely.)
  const asking = w.seen_s < 360;
  return h("div", { class: "mc off", style: { borderLeft: "3px solid #7b8590" } },
    h("div", "top", h("span", "name", w.name), h("span", "role", "helper"), asking ? chip("no status lately", "warn") : chip("out of touch", "bad"), h("span", "right", `last heard ${dur(w.seen_s)} ago`)),
    h("div", "sub", `Its last word: ${w.what || "–"}`),
    h("div", "sub", asking ? "It asks the lead for work, but its status on the NAS hasn't been written for ten minutes (can it reach the NAS?)." : "Asleep, off, or its agent stopped; its work goes back to the others once its lease lapses."));
}

function pagesCard(m) {
  if (!m.pages.length) return null;
  const card = h("div", { class: "mc pages", style: { borderLeft: "3px solid #6cc28a" } },
    h("div", "top", h("span", "name", "Pages"), h("span", "role", "browsers doing tasks"), chip(`${m.pages.length}`, "ok")));
  for (const p of m.pages) {
    const leases = m.leasesOf(p.name);
    const st = p.bad ? chip("stopped", "bad") : p.seen_s > 120 ? chip("away", "") : leases.length ? chip(`${leases.length} running`, "run") : chip("waiting", "");
    const left = h("div", null, h("div", null, p.label, " ", st, p.visible === false ? chip("in the background", "warn") : null),
      h("div", "sub", leases.length ? leases.map((l) => `${l.progress || l.step}${l.frac != null ? ` (${Math.round(l.frac * 100)}%)` : ""}`).join(" · ") : p.what));
    const right = h("div", "sub", `${n(p.done)} done${p.checked ? ` (${n(p.checked)} checked against the lead's)` : ""}${p.failed ? ` · ${p.failed} failed` : ""}${p.mem_mb ? ` · ${n(p.mem_mb)} MB` : ""} · ${dur(p.seen_s)} ago`);
    card.append(h("div", "pg", left, right));
  }
  const t = m.tasks;
  if (t.offered != null) card.append(h("div", "sub", `Tasks now: ${t.offered || 0} offered, ${t.leased || 0} running, ${t.done || 0} done, ${t.failed || 0} failed${!t.offered && !t.leased ? " — a job offers them as it runs, a few at a time" : ""}`));
  return card;
}

// ---- Cards: on hover, pinned on a click ------------------------------------------------------------
// One card for everything the Jobs tab shows: under a mouse's or trackpad's pointer, a card follows
// it over anything that has one, never catching the pointer itself (pointer-events none), and goes
// as the pointer leaves what it's about (into anything else, off the map, out of the window); a
// click or a tap pins it where it is, with a close button, its words made again at each render
// while what it's about is still shown (by its key). A pinned card is modal (#627, owner 21 Oct):
// a clear veil under it takes every click and scroll behind it, a click on it only letting the
// card go; the card itself scrolls and stays live. Escape or the close button let it go too.
const cards = new Map();
const pin = { key: null, el: null, also: "", veil: null, x: 0, y: 0, back: null };
let hoverCard = null;
// `el`'s card: `build(also)` its rows (made when shown; `also`: what else is under the point, the
// element's `data-also`, as the world map says it), `key` naming it across renders.
function withCard(el, key, build) {
  cards.set(key, build);
  el.dataset.card = key;
  return el;
}
// A card's rows: its title, then [key, value] pairs (a value null: no row), then any lines. A
// title given as an object ({title, …}: the map's cards, as docs/mockups/build-live.html draws
// them) may have a head (`kicker` over it, a `colour` swatch), stat tiles (`stats`: [number,
// label] pairs, none null), a bar (`bar`: {frac, label, colour}), a bar in parts (`segs`: {parts
// [{w, colour, cls, f, title}], label: a part with `f` filled that far, the rest faint, or striped when
// it's `run`), chips (`chips` under `chipsLabel`) and a foot (`foot`); the Jobs tab's are `wide`,
// with a world map (`map`, a canvas) under the head.
const cardBody = (title, pairs, ...more) => {
  const o = title && typeof title === "object" && !(title instanceof Node) && !Array.isArray(title) ? title : { title };
  const stats = o.stats?.filter(([v]) => v != null);
  const head = o.kicker != null || o.colour
    ? h("div", o.wide ? "tc-head wide" : "tc-head", o.colour ? h("i", { class: "tc-sw", style: { background: o.colour } }) : null, h("div", null, o.kicker ? h("div", "tc-kick", o.kicker) : null, h("div", "tc-title", o.title)))
    : h("div", "ct", o.title);
  const bar = o.bar && h("div", "tc-bar", h("div", "track", h("i", { style: { width: `${Math.round(Math.max(0, Math.min(1, o.bar.frac)) * 100)}%`, background: o.bar.colour || o.colour || "var(--accent)" } })), o.bar.label ? h("div", "cap", o.bar.label) : null);
  const segs = o.segs?.parts?.length && h("div", "tc-bar", h("div", "track segs", o.segs.parts.map((p) => h("i", { class: [p.cls, p.f != null ? "fill" : null].filter(Boolean).join(" ") || null, title: p.title || null, style: { width: `${(p.w * 100).toFixed(2)}%`, ...(p.f != null ? { "--c": p.colour, "--f": `${(p.f * 100).toFixed(1)}%` } : { background: p.colour }) } }))), o.segs.label ? h("div", "cap", o.segs.label) : null);
  return [head, o.map || null,
    stats?.length ? h("div", "cstats", stats.map(([v, l]) => h("div", "cstat", h("div", "n", v), h("div", "l", l)))) : null,
    bar, segs,
    h("div", "ckv", pairs.filter(([, v]) => v != null && v !== "").flatMap(([k, v]) => [h("span", "k", k), h("span", null, v)])),
    o.chips?.length ? h("div", "tc-chipsbox", o.chipsLabel ? h("div", "tc-kick", o.chipsLabel) : null, h("div", "tc-chips", o.chips.map((c) => h("span", null, c)))) : null,
    ...more, o.foot ? h("div", { class: "tc-foot", title: o.foot }, o.foot) : null].filter(Boolean);
};
// (Beside the point, right else left where there's no room; wholly below it, else above where it
// fits there better: never across the pointer's line. No taller than 50svh (dash.css) or the room
// on its side; the rest scrolls inside it.)
function placeCard(el, x, y) {
  el.style.maxHeight = "";
  const r = el.getBoundingClientRect(), W = window.innerWidth, H = window.innerHeight;
  const left = x + 12 + r.width <= W - 8 ? x + 12 : x - 12 - r.width >= 8 ? x - 12 - r.width : Math.max(8, W - r.width - 8);
  const below = H - 8 - (y + 14), above = y - 10 - 8;
  const down = r.height <= below || below >= above;
  const h = Math.min(r.height, Math.max(0, down ? below : above));
  if (h < r.height) el.style.maxHeight = `${h}px`;
  el.style.left = `${left}px`;
  el.style.top = `${down ? y + 14 : y - 10 - h}px`;
}
function showHover(key, x, y, also = "") {
  const build = cards.get(key);
  if (!build || (pin.key === key && pin.also === also)) return hideHover();
  if (!hoverCard) { hoverCard = h("div", "card-tip"); document.body.append(hoverCard); }
  if (hoverCard.dataset.key !== `${key}|${also}`) { hoverCard.replaceChildren(...build(also)); hoverCard.dataset.key = `${key}|${also}`; }
  hoverCard.hidden = false;
  placeCard(hoverCard, x, y);
}
function hideHover() { if (hoverCard) { hoverCard.hidden = true; hoverCard.dataset.key = ""; } }
function pinCard(key, x, y, also = "") {
  unpin();
  hideHover();
  const close = h("button", { class: "quiet x", title: "Close (Esc)", "aria-label": "Close", onclick: unpin }, "×");
  pin.key = key;
  pin.also = also;
  pin.x = x;
  pin.y = y;
  pin.el = h("div", { class: "card-tip pinned", role: "dialog", "aria-modal": "true" }, close, h("div", "cb", cards.get(key)(also)));
  tl?.pin(key);
  // (The veil: a click on it lets the card go and nothing else; a wheel or a touch's drag there
  // moves nothing. The page under it can't scroll while the card is up.)
  pin.veil = h("div", { class: "card-veil" });
  const still = (e) => e.preventDefault();
  pin.veil.addEventListener("wheel", still, { passive: false });
  pin.veil.addEventListener("touchmove", still, { passive: false });
  pin.veil.addEventListener("pointerdown", (e) => e.stopPropagation());
  // (A tap pinned it at its lift, the page's or the map's: that tap's own click, coming after
  // onto the veil, is passed over.)
  const t0 = performance.now();
  pin.veil.addEventListener("click", (e) => { e.stopPropagation(); if (performance.now() - t0 > 800 || Math.hypot(e.clientX - x, e.clientY - y) > 12) unpin(); });
  document.body.append(pin.veil, pin.el);
  document.documentElement.classList.add("card-modal");
  placeCard(pin.el, x, y);
}
function unpin() {
  pin.el?.remove();
  pin.veil?.remove();
  document.documentElement.classList.remove("card-modal");
  // (A card opened from a button (the header's asks): the focus back on it, if it's still there.)
  if (pin.back?.isConnected && !pin.back.disabled) pin.back.focus();
  pin.key = pin.el = pin.veil = pin.back = null;
  tl?.pin(null);
}
// (After a render: the pinned card's words again, from what's shown now; kept as they were, said
// to be over, when it's gone.)
function refreshPin() {
  // (The confirmation of a press: words that don't change, and made again it'd lose its focus.)
  if (!pin.el || pin.key === "ask") return;
  const build = cards.get(pin.key);
  const body = pin.el.querySelector(".cb");
  if (build) body.replaceChildren(...build(pin.also));
  else if (!pin.el.classList.contains("gone")) { pin.el.classList.add("gone"); body.append(h("div", "dim small", "No longer shown (the schedule moved on).")); }
  placeCard(pin.el, pin.x, pin.y); // (new words, new height: still off the pointer's line)
}
let cardsWired = false;
function wireCards() {
  if (cardsWired) return;
  cardsWired = true;
  const at = (t) => (t instanceof Element && t.closest("[data-card]")?.dataset) || {};
  document.addEventListener("pointermove", (e) => {
    const d = e.pointerType === "touch" ? {} : at(e.target);
    if (d.card) showHover(d.card, e.clientX, e.clientY, d.also);
    else hideHover();
  });
  // (Leaving what has a card: into what has another or none, or out of the window (no element
  // to go to); the map's canvas says nothing once left (worldmap.js). A pinned card stays.)
  document.addEventListener("pointerout", (e) => { if (!e.relatedTarget || at(e.relatedTarget).card !== at(e.target).card) hideHover(); });
  document.documentElement.addEventListener("mouseleave", hideHover);
  window.addEventListener("blur", hideHover);
  const act = (e) => {
    if (pin.el?.contains(e.target) || pin.veil?.contains(e.target)) return;
    const d = at(e.target), key = d.card;
    if (key && cards.has(key)) pinCard(key, e.clientX, e.clientY, d.also);
    else unpin();
  };
  // (A tap is taken at its lift: Mobile Safari sends no click for a tap on what has no handler of
  // its own, plain text, so a tap there never let a card go (#615). Its click, if one comes, is
  // passed over. The world map takes its own.)
  let down = null, tapped = 0;
  document.addEventListener("pointerdown", (e) => { down = e.pointerType === "touch" && e.isPrimary ? { x: e.clientX, y: e.clientY, t: e.timeStamp } : null; });
  document.addEventListener("pointerup", (e) => {
    const d = down;
    down = null;
    if (!d || e.pointerType !== "touch" || Math.hypot(e.clientX - d.x, e.clientY - d.y) > 10 || e.timeStamp - d.t > 800) return;
    if (e.target instanceof Element && e.target.closest("canvas.wmap, a, button, input, select, label, summary, .card-veil")) return;
    tapped = performance.now();
    act(e);
  });
  document.addEventListener("pointercancel", () => { down = null; });
  document.addEventListener("click", (e) => { if (performance.now() - tapped > 1000) act(e); });
  document.addEventListener("keydown", (e) => e.key === "Escape" && unpin());
  window.addEventListener("scroll", hideHover, { passive: true });
}

// A Mac's short name for its lanes: its chip ("M4 Pro": cond::Resources::chip), else its name's
// first part; the full name when two Macs would read alike.
function shortNames(m) {
  const first = (nm) => nm.split(/[-.\s]/)[0] || nm;
  const short = Object.fromEntries(m.macs.map((x) => [x.name, x.status.resources?.chip || first(x.name)]));
  const seen = {};
  for (const v of Object.values(short)) seen[v] = (seen[v] || 0) + 1;
  for (const [k, v] of Object.entries(short)) if (seen[v] > 1) short[k] = k;
  return short;
}

// ---- Jobs: the schedule ------------------------------------------------------------------------------
// The history the schedule holds (task #787): the page view's seven days (coord::history::DAYS);
// older days are in the archive (agent::histarchive), not read here.
const HISTORY_S = 7 * 86400;
// The jobs' ends from the history further back than the page's own events (`/work/history`, a
// chunk at a time back to HISTORY_S ago: sums.js pastFrom), in the numbering they were read in.
const past = { ends: [], epoch: undefined, first: null, asking: false };
// Every job's end held (a lead's job's "end", a worker's "done" or "fail"): those read back, then
// the page's own events.
const isEnd = (e) => e.kind === "end" || e.kind === "done" || e.kind === "fail";
const endsHeld = () => past.ends.concat(events.filter(isEnd));
// The oldest event the page holds (its past window's or the live poll's), null: none.
const heldFrom = () => { let t = events[0]?.t ?? null; for (const e of past.ends) if (t == null || e.t < t) t = e.t; return t; };
async function loadPast(sw) {
  if (past.epoch !== sw.epoch) Object.assign(past, { ends: [], epoch: sw.epoch, first: null });
  if (past.asking || !events.length) return;
  past.asking = true;
  try {
    const from = sw.now - HISTORY_S;
    let first = past.first || { seq: events[0].seq, t: events[0].t };
    for (let since = pastFrom(first, from); since != null; since = pastFrom(first, from)) {
      const [code, hist] = await ctx.call("/work/history", { since, max: 5000 });
      if (code !== 200 || !hist?.events?.length || hist.epoch !== past.epoch) break;
      const got = hist.events.filter((e) => e.seq < first.seq);
      if (!got.length) break;
      past.ends = got.filter(isEnd).concat(past.ends);
      first = past.first = { seq: got[0].seq, t: got[0].t };
      render();
    }
  } catch { /* (Asked again with the next poll.) */ } finally {
    past.asking = false;
  }
}

// What a job running now is, as a schedule's run: on its slot's lane, from its start to now, its
// targets as the history names them (sums.js targetsOf).
const runningRuns = (m) => m.macs.flatMap((x) => jobsOf(x.status).map((j) => ({ lane: slotName(x.name, j.slot), step: stepOf(j.id), from: j.started, until: m.now, n: j.live?.targets || (jobTarget(j) ? 1 : 0), targets: targetsOf({ step: stepOf(j.id), id: j.id, started: j.started }, events), job: j, mac: x.name })));

// The Jobs tab's cards (#659), as the mockup's (look/finalmock: dash.js showTip): a head (the
// stage's swatch, a kicker "<step> · what it is", the code's name as title), a world map of the
// tiles it names, stat tiles, a bar, key–value rows, chips, its live part, a foot with its names.
// Twice the map cards' width (the owner's 22 Oct, BUILD-PAGE-OWNER.md 22); the map small until it's
// pinned.
const jobCard = (o, rows, ...more) => [h("div", "tc-sec", ...cardBody({ ...o, wide: true }, rows, ...more))];
// A list of targets as chips: the first 24, then how many more.
const targetChips = (t) => (t.length > 24 ? [...t.slice(0, 24), `and ${n(t.length - 24)} more`] : t);

// A run's card: a job that ran, runs or is to come (the forecast's), on which Mac, when, how long,
// how it went; its targets (on a world map where they're tiles), its part and phase under way.
function runCard(m, r, kind, laneName) {
  const several = r.parts?.length > 1;
  const colour = S.colour(r.step), targets = r.targets || [];
  const tiles = targets.filter((t) => tileOf(t));
  const map = tiles.length ? tilesMap(countries, tiles, colour) : null;
  if (kind === "running") {
    const j = r.job, p = j.progress;
    const frac = p?.total ? Math.min(1, p.done / p.total) : null;
    const parts = j.parts?.length || 0, at = Math.min(j.part ?? 0, Math.max(0, parts - 1));
    return jobCard({ kicker: `${r.step} · job under way${j.paused ? ", frozen" : ""}`, colour, title: j.id, map,
      stats: [[frac != null ? `${Math.floor(frac * 100)}%` : "–", "of the job"], [p?.eta_s != null ? dur(p.eta_s) : "–", "left"]],
      bar: frac != null ? { frac, label: progressText(p, j, m.now) } : null,
      chips: targets.length > 1 ? targetChips(targets) : [], chipsLabel: targets.length > 1 ? S.counted(r.step, targets.length) : "",
      foot: j.id }, [
      ["Mac", laneName(r.lane)],
      ["Running for", `${dur(m.now - j.started)} (since ${clock(j.started)})`],
      ["Memory", j.mem_mb ? `${(j.mem_mb / 1024).toFixed(1)} GB` : null],
      ["Threads", j.threads ?? null],
      ["Part", parts ? `${at + 1} of ${parts}: ${j.parts[at]}` : null],
    ], inside(j, m.now, m.fc));
  }
  if (kind === "past") {
    const state = r.ok ? "finished" : several ? "finished, some failed" : "failed";
    return jobCard({ kicker: `${r.step} · job ${state}`, colour: r.ok ? colour : "var(--bad)", title: several ? `${S.said(r.step, r.n)} (${r.parts.length} jobs)` : S.job(r.step, targets), map,
      stats: [[dur(r.until - r.from), "took"], [ago(m.now, r.until), "ended"]],
      bar: { frac: r.ok ? 1 : 0, label: state },
      chips: several ? [] : targetChips(targets), chipsLabel: !several && targets.length ? S.counted(r.step, targets.length) : "",
      foot: several ? r.step : `${r.step} ${targets.join(" ")}` }, [
      ["Mac", laneName(r.lane)],
      ["When", `${clock(r.from)}–${clock(r.until)}`],
    ]);
  }
  return jobCard({ kicker: `${r.step} · job to come`, colour, title: r.n ? S.counted(r.step, r.n) : r.step,
    stats: [[clock(r.from), "forecast start"], [clock(r.until), "forecast end"]],
    bar: { frac: 0, label: "not started" }, foot: r.step }, [
    ["Mac", laneName(r.lane)],
    ["Takes", `≈ ${dur(r.until - r.from)}`],
    ["While the lead runs", r.of ? S.said(r.of, r.n) : null],
    ["Waits on", r.waits != null ? plural(r.waits, "run") : null],
    ["Then", r.waited != null ? `${plural(r.waited, "run")} wait on it` : null],
  ]);
}

// What keeps a Mac's lanes from the work, beside its name over them (task #628), as the Machines
// tab's chips: paused alone (task #224; the forecast gives it no lanes, its past jobs still show),
// out of touch, on battery.
function laneChips(x) {
  const st = x.status, c = st.conditions || {};
  return [
    st.held && chip(jobsOf(st).length ? "pausing" : "paused", "warn", `Paused alone, from ${st.held.by}: it takes no new work; the other Macs go on`),
    !x.fresh && chip("out of touch", "bad", `Last heard from ${clock(st.beat)}`),
    c.ac === false && chip(`on battery${c.battery != null ? ` ${c.battery}%` : ""}`, c.battery != null && c.battery < 40 ? "warn" : ""),
  ].filter(Boolean);
}

// Each Mac's lanes, grouped by Mac (its short name over its slots' lanes, the lead first): the jobs
// it ran (the history's ends, as far back as it's read: seven days), those running now (to the now
// line) and the forecast's to come, each a segment in its stage's colour, packed where a pixel holds
// several at the view's zoom (sums.js packRuns); a now line; each region's mark where it's all on
// the map; a dense time axis. Moved and zoomed as timeline.js says, over the whole domain: the
// history's seven days back to the forecast's end (task #787). One element kept across renders
// (the timeline's view and gestures with it), its lanes made again each render.
let tl = null, schedRoot = null, schedBody = null;
// This render's segments by card key ({el, kind, step, r, run}: `run` a forecast run's number,
// sums.js runNumbers; a running job's is the run it goes on in), the runs' links (sums.js
// runLinks), each run's segments' keys.
let segs = new Map(), links = runLinks([]), runKeys = new Map();
// The tiles the schedule lights on the map (a job's or a legend item's), null: none.
let schedLit = null;
function scheduleSection(m) {
  if (!schedRoot) {
    tl = createTimeline({
      relations: schedRelations,
      onLit: (tiles) => { schedLit = tiles ? new Set(tiles) : null; paintMap(); },
      onRepack: () => render(),
    });
    schedBody = h("div", "sched");
    schedRoot = section("d-sched", "Schedule", "each Mac's jobs: those that ran, running, to come",
      h("div", "box", schedBody,
        // (What the coordinator's own scheduler decides, run ahead: docs/pool.md §7.2. What it can't see, said.)
        h("p", "note", "To come as the coordinator grants: by each Mac's room, cores and the NAS budget. Times and NAS weights are measured estimates, and no load from outside the build is assumed (optimistic while a Mac is in use); a pause, a repack, a Mac handing over on battery or a slow NAS moves the finish.")));
  }
  schedule(m);
  return schedRoot;
}

// A segment's tiles on the map ("z/x/y"): a run that ran, its jobs' targets; one running, its
// targets; one to come, its forecast run's (`/work/tocome`'s `runs`, when it's this forecast's).
function segTiles(s, m) {
  const r = s.r;
  let targets = [];
  if (s.kind === "past") targets = (r.parts || [r]).flatMap((p) => p.targets || []);
  else if (s.kind === "running") targets = r.targets || [];
  else if (s.run != null && toCome.ans?.at === m.fc?.at) targets = toCome.ans.runs?.[s.run] || [];
  return areaTargets(s.step, targets).filter((t) => tileOf(t)).map((t) => t.slice(t.lastIndexOf(" ") + 1));
}
// What a key points at in the schedule (timeline.js `relations`): a segment's run's own segments,
// those of the runs it waits on and of those waiting on it, and its arrows (from a run's end, its
// last segment, to a run's start, its first); a legend item's (`lg stage <id>`, `lg kind <kind>`)
// segments. Their tiles for the map: the segment's own, the legend item's all.
function schedRelations(key) {
  const m = lastSchedM;
  if (!m) return null;
  if (key.startsWith("lg ")) {
    const [, by, v] = key.split(" ");
    const on = [...segs.values()].filter((s) => (by === "stage" ? S.stage(s.step) === v : by === "failed" ? s.kind === "past" && !s.r.ok : by === "past" ? s.kind === "past" && s.r.ok : s.kind === v));
    return { on: on.map((s) => s.el), arrows: [], tiles: [...new Set(on.flatMap((s) => segTiles(s, m)))] };
  }
  const s = segs.get(key);
  if (!s) return null;
  const of = (run) => (runKeys.get(run) || []).map((k) => segs.get(k)).filter(Boolean);
  const self = s.run != null ? of(s.run) : [s];
  const head = (ss) => ss[0]?.el, tail = (ss) => ss[ss.length - 1]?.el;
  const on = [...self], arrows = [];
  for (const a of s.run != null ? links.on.get(s.run) || [] : []) { const x = of(a); on.push(...x); if (x.length) arrows.push([tail(x), head(self)]); }
  for (const b of s.run != null ? links.then.get(s.run) || [] : []) { const x = of(b); on.push(...x); if (x.length) arrows.push([tail(self), head(x)]); }
  return { on: on.map((x) => x.el), arrows, tiles: segTiles(s, m) };
}
let lastSchedM = null;

function schedule(m) {
  lastSchedM = m;
  const fc = m.fc, now = m.now;
  const fcLanes = fc?.lanes || {};
  const lo = now - HISTORY_S;
  const hi = Math.max(fc?.done_at || 0, ...Object.values(fcLanes).flat().map((l) => l.until), now + SPAN0 / 2);
  const full = hi - lo;
  const { span } = tl.range(now, lo, hi);
  // (A segment's place in its track: a share of the whole domain; the view is the timeline's.)
  const pct = (t) => `${(((Math.max(lo, Math.min(hi, t)) - lo) / full) * 100).toFixed(5)}%`;
  const width = Math.max(240, tl.axis.offsetWidth || ctx.tabsEl.clientWidth - 120);
  const gap = span / width;
  const pastAll = pastRuns(endsHeld(), lo, m.macs.map((mm) => mm.name));
  const running = runningRuns(m);
  const short = shortNames(m);
  const hosts = m.macs.map((mm) => mm.name);
  const names = [...new Set([...busyLanes(Object.keys(fcLanes), fcLanes, now, hi), ...pastAll.map((r) => r.lane), ...running.map((r) => r.lane)])];
  // (A Mac with no lanes in view still gets its row when something keeps it from the work: a Mac
  // paused alone has no forecast lanes, and no past ones once its last job is out of the past.)
  const chipsOf = new Map(m.macs.map((mm) => [mm.name, laneChips(mm)]));
  const grouped = laneGroups(names, hosts);
  const groups = [
    ...hosts.map((hh) => grouped.find((g) => g.host === hh) || (chipsOf.get(hh).length ? { host: hh, lanes: [] } : null)).filter(Boolean),
    ...grouped.filter((g) => !hosts.includes(g.host)),
  ];
  const laneName = (nm) => {
    const host = hosts.find((hh) => isSlotOf(nm, hh));
    return host ? `${short[host]} · slot ${slotNo(nm, host)}` : nm;
  };
  // The forecast's runs (numbered as `/work/tocome` numbers them) and what each waits on, when
  // its answer is this forecast's.
  const nums = runNumbers(fcLanes);
  const tc = toCome.ans?.at === fc?.at && toCome.ans?.runs?.length === nums.total ? toCome.ans : null;
  links = runLinks(tc?.deps);
  segs = new Map();
  runKeys = new Map();
  const stagesSeen = new Set();
  const kinds = new Set();
  const lanes = h("div", "lanes");
  const segment = (track, r, kind, key, run = null) => {
    const seg = withCard(h("i", { class: `${kind}${kind === "past" && !r.ok ? " failed" : ""}` }), key, () => runCard(m, r, kind, laneName));
    Object.assign(seg.style, { left: pct(r.from), width: `max(2px, calc(${pct(r.until)} - ${pct(r.from)}))`, background: S.colour(r.step) });
    track.append(seg);
    segs.set(key, { el: seg, kind, step: r.step, r, run });
    if (run != null) (runKeys.get(run) || runKeys.set(run, []).get(run)).push(key);
    if (S.stage(r.step)) stagesSeen.add(S.stage(r.step));
    kinds.add(kind === "past" && !r.ok ? "failed" : kind);
  };
  const trackOf = () => { const tv = h("div", "tv"); return [h("div", "track", tv), tv]; };
  for (const g of groups) {
    const mac = m.macs.find((mm) => mm.name === g.host);
    lanes.append(h("div", { class: "gh", title: g.host }, mac ? short[g.host] : g.host, mac ? null : h("span", "dim", " (gone)")), h("div", "ghr", ...(chipsOf.get(g.host) || [])));
    for (const nm of g.lanes) {
      const [track, tv] = trackOf();
      for (const r of packRuns(pastAll.filter((r) => r.lane === nm), gap)) segment(tv, r, "past", `p ${nm} ${r.seq}`);
      const ls = fcLanes[nm] || [];
      // (A job running now goes on as its lane's first run of its step in the forecast: one job.)
      for (const r of running.filter((r) => r.lane === nm)) {
        const k = ls.findIndex((l) => l.step === r.step && l.from <= fc.at + 1);
        segment(tv, r, "running", `r ${nm} ${r.job.id}`, k >= 0 ? nums.first[nm] + k : null);
      }
      ls.forEach((l, k) => {
        if (l.until <= now) return;
        const run = nums.first[nm] + k;
        segment(tv, { ...l, from: Math.max(l.from, now), lane: nm, waits: tc ? links.on.get(run)?.length || 0 : null, waited: tc ? links.then.get(run)?.length || 0 : null }, "next", `f ${nm} ${l.from} ${l.step}`, run);
      });
      lanes.append(h("div", { class: "ln", title: nm }, `${slotNo(nm, g.host) ?? nm}`), track);
    }
  }
  // The pages: a job's tasks (crate::agent::steps::TASKS) as the lead runs the job, when a page is
  // measured faster than the lead at that kind (crate::offload) and spares what one takes.
  const own = Object.keys(fcLanes).filter((nm) => isSlotOf(nm, m.a.host)).flatMap((nm) => fcLanes[nm]).sort((p, q) => p.from - q.from);
  const pace = (p, kind) => (p.paces ? p.paces[kind] : kind === "tail" ? p.tail_pace : null);
  const fast = (kind) => m.pages.some((p) => (p.mem_mb || 0) >= (m.sw.task_mb?.[kind] || 0) && pace(p, kind) != null && pace(p, kind) * 1.25 < 1);
  const helps = Object.values(table.steps).filter((t) => t.of && fast(t.id) && own.some((l) => l.step === t.of && l.until > now));
  if (helps.length) {
    const who = m.pages.length > 1 ? `Pages (${m.pages.length})` : m.pages[0].label;
    const [track, tv] = trackOf();
    for (const t of helps) for (const l of own.filter((l) => l.step === t.of && l.until > now)) segment(tv, { ...l, from: Math.max(l.from, now), step: t.id, lane: who, of: t.of }, "next", `g ${t.id} ${l.from}`);
    lanes.append(h("div", "gh", "Pages"), h("div", "ghr"), h("div", { class: "ln", title: m.pages.map((p) => p.label).join(", ") }, who), track);
  }
  // Each region all on the map as it's built now: a mark at its `map_at` (several at one time, one).
  const outs = {};
  for (const r of fc?.regions || []) if (r.map_at && r.map_at >= now && r.map_at <= hi) (outs[r.map_at] ||= []).push(m.regionName[r.id] || r.id);
  if (Object.keys(outs).length) {
    const [track, tv] = trackOf();
    track.classList.add("outs");
    for (const [at, nms] of Object.entries(outs)) {
      tv.append(withCard(h("b", { style: { left: pct(+at) } }), `o ${at}`, () => cardBody(`All on the map ≈ ${clock(+at)}`, [["regions", `${nms.slice(0, 12).join(", ")}${nms.length > 12 ? ` and ${nms.length - 12} more` : ""}`]])));
    }
    lanes.append(h("div", "ln", "On the map"), track);
  }
  lanes.append(h("div"), tl.axis);
  withCard(tl.archive, "sched-archive", () => cardBody("Older history: in the archive", [
    ["here", "the last 7 days (the history's days)"],
    ["older", "on the NAS, a tar a day (state/coord/history-archive)"],
    ["from this page", "not reachable"],
  ]));
  // The legend: the stages seen, in the build's order, each by its id; what a segment's look says;
  // the regions' mark. Each item hovered: its jobs' tiles lit on the map, the rest of the schedule
  // dimmed.
  const order = table.stages.map((g) => g.id);
  const stages = [...stagesSeen].sort((p, q) => order.indexOf(p) - order.indexOf(q));
  const lg = (key, ...kids) => h("span", { class: "lgi", "data-lg": key }, ...kids);
  const KEYS = [["past", "k-past", "ran"], ["failed", "k-failed", "failed"], ["running", "k-run", "running"], ["next", "k-next", "to come"]];
  const legend = h("div", "legend", stages.map((g) => lg(`lg stage ${g}`, h("i", { style: { background: stageColour(g) } }), g)),
    h("span", "keys", KEYS.filter(([k]) => kinds.has(k)).map(([k, cls, text]) => lg(`lg kind ${k}`, h("i", cls), text))),
    Object.keys(outs).length ? h("span", null, h("i", { class: "rmark" }), "a region all on the map") : null,
    h("span", "hint", tc ? "a job to come or running, hovered: red arrows from what it waits on and to what waits on it" : null),
    h("span", "hint", "drag to move · pinch or ⌥ scroll to zoom"));
  legend.addEventListener("pointerover", (e) => { if (e.pointerType !== "touch") tl.hover(e.target.closest?.("[data-lg]")?.dataset.lg || null); });
  legend.addEventListener("pointerleave", () => tl.hover(null));
  // (A forecast cut at its time budget (forecast::BUDGET): the lanes end where its run stopped.)
  const cut = fc?.partial && h("div", "why", "Partial: the forecast was cut at its time budget; each lane ends where its run stopped, the work after it not placed");
  const reaching = past.asking && past.first && past.first.t > lo ? h("span", "small dim", "reading the week's history…") : null;
  const top = h("div", "schedtop", reaching, h("span", "small dim", fc?.done_at ? `done ≈ ${clock(fc.done_at)}` : fc ? "" : "no forecast yet (it comes with the lead's next plan)"));
  if (!groups.length && !helps.length) { schedBody.replaceChildren(top, h("div", "small dim", "Nothing ran or is planned in the week.")); return; }
  schedBody.replaceChildren(top, cut || "", tl.el, legend);
  tl.placed(lanes, span);
}

// ---- Jobs: the stages ---------------------------------------------------------------------------------
// A list, a stage under another (the checklist's lines, build::Line): its mark, title, counts and
// bar; under it a row per step (build::Item): its mark, id (⇄ when any Mac may take it), count and
// bar. Done and left of all the enabled regions' work, in the steps' nouns (map tiles, z3 tiles,
// runs): the one place the page counts a whole (task #685); no past runs, no time.
function stagesList(m) {
  if (!m.lines.length) return h("div", "small dim", "No word from the lead yet.");
  const list = h("div", "stages");
  for (const l of m.lines) {
    const done = finished(l), run = (l.steps || []).some((s) => m.running.has(s));
    const colour = S.colour(l.steps?.[0]);
    const frac = l.total ? l.done / l.total : done ? 1 : 0;
    list.append(withCard(h("div", { class: `sh ${done ? "done" : ""} ${run ? "now" : ""}`, style: { borderLeftColor: colour } },
      mark(done, run), h("span", "title", l.id), h("span", "n", countsOf(l)), pbar(frac, done ? "ok" : run ? "" : "dimbar", done ? null : colour)), `s ${l.id}`, () => stageCard(m, l)));
    for (const it of l.items || []) {
      const step = it.step, itDone = finished(it), itRun = m.running.has(step);
      list.append(h("div", { class: `st ${itDone ? "done" : ""}` },
        mark(itDone, itRun),
        withCard(h("span", "title", step, S.any(step) ? h("span", { class: "dim" }, " ⇄") : null), `t ${step}`, () => stepCard(m, it)),
        h("span", "n", countOf(it, `${S.noun(step)}s`)),
        pbar(fraction(it), itDone ? "ok" : itRun ? "" : "dimbar", itDone ? null : S.colour(step))));
    }
  }
  return list;
}

// A stage's card: its work left and finish (stat tiles), in the Jobs tab's stages its counts done
// and left (a bar: `whole`; not on the Overview's strip, task #685), what it makes, what it's on or
// does next, its note; its steps as chips.
function stageCard(m, l, whole = true) {
  const g = table.stages.find((x) => x.id === l.id);
  const done = finished(l), running = (l.steps || []).some((s) => m.running.has(s));
  const fcs = (m.fc?.steps || []).filter((x) => (l.steps || []).includes(x.step));
  const work = fcs.reduce((t, x) => t + (x.work_s || 0), 0), at = Math.max(0, ...fcs.map((x) => x.done_at || 0));
  return jobCard({ kicker: "stage", colour: S.colour(l.steps?.[0]), title: l.id,
    stats: [[done ? "none" : work ? dur(work) : "–", "work left"], [!done && at ? clock(at) : "–", "done ≈"]],
    bar: !whole ? null : l.total ? { frac: l.done / l.total, label: countsOf(l) } : done ? { frac: 1, label: "done" } : { frac: 0, label: countsOf(l) },
    chips: l.steps || [], chipsLabel: "Steps" }, [
    ["Makes", g?.does],
    [running ? "Now" : "Next", !done && l.next?.length ? `${l.next.slice(0, 3).join("; then ")}${l.next.length > 3 ? `; and ${l.next.length - 3} more` : ""}` : null],
    ["Note", l.note],
  ]);
}

// A step's card: its targets done and left (stat tiles, a bar), what it makes, its kind and scope,
// where it runs, its first guess of memory, its forecast (work left, done ≈, time a target); its
// time by phase as chips.
function stepCard(m, it) {
  const row = S.row(it.step);
  const f = m.fc?.steps.find((s) => s.step === it.step);
  const each = m.fc?.each_s?.[it.step];
  const done = finished(it), noun = S.noun(it.step);
  const ps = m.a.phase_shares?.[it.step];
  return jobCard({ kicker: `${S.stage(it.step) || "step"} · step`, colour: S.colour(it.step), title: it.step,
    stats: it.total ? [[n(it.done), `${noun}s done`], [n(it.total - it.done), "left"]] : [[countOf(it, `${noun}s`), "count"]],
    bar: it.total ? { frac: it.done / it.total, label: countOf(it, `${noun}s`) } : done ? { frac: 1, label: "done" } : null,
    chips: (ps?.phases || []).map((p) => `${p.name} ${Math.round(p.wall * 100)}%`), chipsLabel: ps?.phases?.length ? `Time by phase (${plural(ps.runs, "run")})` : "",
    foot: it.step }, [
    ["Makes", row?.does],
    ["Kind", row ? `${row.kind} · ${row.of ? `tasks of ${row.of}` : row.scope}` : null],
    ["Runs on", row ? (row.place === "any" ? "any Mac of the pool with room for it" : "the lead only") : null],
    ["Memory", row?.mem_mb ? `about ${(row.mem_mb / 1024).toFixed(1)} GB a job until measured` : null],
    ["Work left", !done && f?.work_s ? dur(f.work_s) : null],
    ["Done ≈", !done && f?.done_at ? clock(f.done_at) : null],
    ["A target", each ? `≈ ${dur(each)}` : null],
  ]);
}

// ---- Regions --------------------------------------------------------------------------------------
// The Regions tab: the regions' list (virtual: only the rows in view are made), ordered as they
// reach the map, by name or by work left, found by any folder's name; each row its folders, state,
// when it's done and on the map, its tiles left and a bar of its stages (built, under way, to come,
// each with its targets left on hover); under the pointer, its region lit on the left panel's map
// (task #685) and its card. Kept across the page's renders (its scroll and search with it).
const ui = { order: "map", filter: "", feed: "all" };
// (Taller where the tabs' panel is under 720 px wide, a phone's or one dragged narrow: a name and
// the line under it wrap there, dash.css; 112 px holds the longest at 375 px.)
let ROW_H = 40;
const rp = { section: null, el: null, list: null, rowsEl: null, rows: [], shown: [], win: null, lit: null };
// (Across 720 px the rows change height: the same row stays at the top, the list drawn again.)
function rowHeight(w) {
  const h0 = ROW_H;
  ROW_H = w < 720 ? 112 : 40;
  if (h0 === ROW_H || !rp.list || !rp.rowsEl) return;
  const top = rp.list.scrollTop * (ROW_H / h0);
  relist();
  rp.list.scrollTop = top;
  paintRows(true);
}

// A download's size in words: "640 MB", "9.0 GB".
const size = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);

// Each region as the tab shows it: the forecast's (its `left` by step, rank, times) with its
// folders, size and outline (/work/regions), its tiles built (the lead's `built`), its targets built
// of all by stage (`fill`: the lead's steps, `steps`), its recipe
// (the lead's `regions`); those the forecast lacks (or every one without a forecast) without its words.
function regionRows(m) {
  const fc = m.fc, built = m.a.built || {};
  const order = Object.keys(table.steps), stages = table.stages.map((g) => g.id);
  const recipe = Object.fromEntries((m.a.regions || []).map((r) => [r.id, r]));
  const fcBy = Object.fromEntries((fc?.regions || []).map((r) => [r.id, r]));
  // (The forecast's first, in its order; then any it lacks that the lead, its built tiles or the
  // outlines name: the waters regions aren't in the plan, yet they're on the map.)
  const ids = [...new Set([...(fc?.regions || []).map((r) => r.id), ...(m.a.regions || []).map((r) => r.id), ...Object.keys(built), ...Object.keys(drawn.byId)])];
  return ids.map((id) => {
    const f = fcBy[id], d = drawn.byId[id];
    const work = regionWork(f?.left, fc?.each_s, order);
    return { ...f, id, f, name: m.regionName[id] || d?.name || id, crumbs: d?.crumbs, bytes: d?.bytes, built: built[id], outline: recipe[id]?.outline, work,
      fill: regionSteps.regions[id] ? stageFill(regionSteps.regions[id], S.stage) : null,
      segs: stageSegments(work, S.stage, stages), secs: work.reduce((t, w) => t + w.s, 0), count: work.reduce((t, w) => t + w.k, 0) };
  });
}

// The stages that build regions (a step of theirs not worldwide), in the build's order.
const regionStages = () => table.stages.map((g) => g.id).filter((g) => Object.values(table.steps).some((t) => t.stage === g && t.scope !== "world"));

// A region's state in words and its chip's class: on the map, done (with the next catalogue),
// building now, its place in line.
function rowState(r) {
  if (!r.f) return ["", ""];
  let [st, cls] = !r.count ? (r.on_map === true ? ["on the map", "ok"] : ["done; on the map with the next catalogue", "ok"]) : r.rank === 0 ? ["building now", "run"] : [`#${r.rank + 1} in line`, ""];
  return [st, cls];
}

// A region's stage bar, as its row and card draw it: every stage that builds regions, in order,
// built solid, the one under way striped, to come faint, each with its targets left (the lead's
// counts by region, `f`: all less built, task #636; else the forecast's); a stage the lead doesn't
// count by region, without a forecast, faint: not known. No share built (task #685).
function stageParts(r) {
  const left = Object.fromEntries(r.segs.map((g) => [g.stage, g.k])), now = r.rank === 0 ? r.segs[0]?.stage : null;
  return regionStages().map((g) => {
    const c = r.fill?.[g], k = c ? c[1] - c[0] : left[g];
    const todo = c ? k > 0 : g in left, known = !!(c || r.f);
    const state = !known ? "not counted by region" : todo ? `${n(k)} left${g === now ? ", building" : ""}` : "built";
    return { g, now: g === now, todo, cls: g === now ? "run" : todo || !known ? "next" : null, title: `${g}: ${state}` };
  });
}

// Each region's card (the list's rows and the map's regions), as the mockup draws it: its
// folders over its name, its download and tiles left (stat tiles), each stage that builds regions
// (a bar in parts: `stageParts`),
// state, when it's built and on the map, its place in the build's order, its work left by step
// (chips), its outline's recipe. (The other regions at a map's point have their own cards in its
// stack.)
const REGION_COLOUR = { on: "var(--ok)", done: "var(--ok)", building: "var(--accent)", queued: "var(--dim)" };
function regionCards(rows) {
  for (const r of rows) cards.set(`mr ${r.id}`, () => {
    const [st, cls] = rowState(r);
    const b = r.built;
    const sp = r.f || r.fill ? stageParts(r) : [];
    const parts = sp.map((p) => ({ w: 1 / sp.length, colour: stageColour(p.g), cls: p.cls, title: p.title }));
    const now = sp.find((p) => p.now)?.g, left = sp.filter((p) => p.todo).length;
    const builtN = sp.length - left;
    return cardBody({ kicker: r.crumbs?.slice(0, -1).join(" › ") || "Region", colour: REGION_COLOUR[regionState(r.f)] || "var(--dim)", title: r.name,
      stats: [[r.bytes != null ? size(r.bytes) : null, "to download"], [b?.total ? n(b.total - b.built) : null, "tiles left"]],
      segs: parts.length ? { parts, label: `stages: ${builtN} built${now ? `, ${now} building` : ""}, ${left - (now ? 1 : 0)} to come` } : null,
      chips: r.count ? r.work.map((w) => `${S.said(w.step, w.k)}${w.s ? ` · ${dur(w.s)}` : ""}`) : [], chipsLabel: "Work left",
      foot: r.outline?.length ? `outline: ${r.outline.join(" + ")}` : "" }, [
      ["state", st ? h("span", cls, st) : null],
      ["on the map", r.f ? (r.on_map === true && !r.count ? "yes" : r.map_at ? `all ≈ ${clock(r.map_at)}` : r.on_map === false ? "as it was" : "not yet") : null],
      ["all built", r.count && r.ready_at ? `≈ ${clock(r.ready_at)}` : null],
      ["build order", r.rank != null ? `#${n(r.rank + 1)} in line` : null],
    ]);
  });
}

// One row: its folders and name, state chip, when; its stage bar, tiles and work left, its work
// in time and download.
function regionRow(r, i) {
  const [st, cls] = rowState(r);
  const above = r.crumbs?.slice(0, -1).join(" › ");
  const when = r.count ? `done ≈ ${clock(r.ready_at)}${r.map_at ? ` · on the map ≈ ${clock(r.map_at)}` : ""}` : "";
  // (As the mockup's: a segment a stage with work left here, named in it; its tiles and the words left on hover.)
  const b = r.built;
  const tiles = b?.total ? (b.built < b.total ? `${plural(b.total - b.built, "tile")} left` : `${n(b.total)} tiles built`) : "";
  const left = r.work.map((w) => S.said(w.step, w.k)).join(" · ");
  // (`stageParts`, as the region's card's bar.)
  const bar = h("span", { class: "sbar", title: [tiles, left ? `left: ${left}` : ""].filter(Boolean).join(" · ") },
    stageParts(r).map((p) => h("i", { class: p.cls, title: p.title, style: { "--c": stageColour(p.g) } }, p.g)));
  const row = h("div", { class: `rrow${r.f && !r.count && r.on_map === true ? " done" : ""}${rp.lit === r.id ? " lit" : ""}`, role: "listitem", style: { top: `${i * ROW_H}px` } },
    h("span", "nm", above ? h("span", "dim", `${above} › `) : null, r.name, st ? [" ", chip(st, cls)] : null),
    h("span", "when", when),
    h("span", "lw", r.f || r.fill ? bar : h("span", "lt", tiles)),
    h("span", "when", [r.secs ? `≈ ${dur(r.secs)} of work` : "", r.bytes != null ? size(r.bytes) : ""].filter(Boolean).join(" · ")));
  row.dataset.id = r.id;
  row.dataset.card = `mr ${r.id}`;
  return row;
}

// The rows in view (again only when they change, unless `force`).
function paintRows(force) {
  const L = rp.list;
  const win = rowsWindow(L.scrollTop, L.clientHeight || 600, ROW_H, rp.shown.length);
  if (!force && rp.win && win[0] === rp.win[0] && win[1] === rp.win[1]) return;
  rp.win = win;
  const kids = [];
  for (let i = win[0]; i < win[1]; i++) kids.push(regionRow(rp.shown[i], i));
  rp.rowsEl.replaceChildren(...kids);
}

// The list again, as searched and ordered now.
function relist() {
  rp.shown = regionsShown([...rp.rows], ui.order, ui.filter);
  rp.rowsEl.style.height = `${rp.shown.length * ROW_H}px`;
  rp.count.textContent = ui.filter.trim() ? `${n(rp.shown.length)} of ${n(rp.rows.length)}` : "";
  for (const b of rp.seg.children) b.classList.toggle("on", b.dataset.order === ui.order);
  if (!rp.shown.length) rp.rowsEl.replaceChildren(h("div", "small dim rnone", rp.rows.length ? "None found." : "No regions yet."));
  else paintRows(true);
}

// The region under the pointer in the list, lit on the map.
function light(id) {
  if (rp.lit === id) return;
  rp.lit = id;
  worldMap?.highlight(id);
  for (const el of rp.rowsEl.children) el.classList.toggle("lit", el.dataset.id === id);
}

function regionsPane(m, md) {
  if (!rp.el) {
    const find = h("input", { type: "search", placeholder: "Find a region or a folder…", value: ui.filter, autocapitalize: "off", spellcheck: "false", "aria-label": "Find a region" });
    find.addEventListener("input", () => { ui.filter = find.value; rp.list.scrollTop = 0; relist(); });
    rp.seg = h("div", "seg", [["map", "To the map"], ["name", "A–Z"], ["left", "Most work left"]].map(([k, t]) => {
      const b = h("button", { onclick: () => { ui.order = k; rp.list.scrollTop = 0; relist(); } }, t);
      b.dataset.order = k;
      return b;
    }));
    rp.count = h("span", "small dim");
    rp.totals = h("div", "rtotals small dim");
    rp.rowsEl = h("div", "rrows");
    rp.list = h("div", { class: "rlist", role: "list", "aria-label": "Regions" }, rp.rowsEl);
    let ticking = false;
    const later = () => { if (!ticking) { ticking = true; requestAnimationFrame(() => { ticking = false; if (rp.shown.length) paintRows(false); }); } };
    rp.list.addEventListener("scroll", later, { passive: true });
    // (Its height changed (the window, the tab shown again): the rows that now fit.)
    new ResizeObserver(later).observe(rp.list);
    rp.list.addEventListener("pointerover", (e) => { if (e.pointerType !== "touch") light(e.target.closest?.(".rrow")?.dataset.id || null); });
    rp.list.addEventListener("pointerleave", () => light(null));
    rp.el = h("div", "rpane box", h("h3", null, "Regions"), h("div", "regbar", find, rp.seg, rp.count), rp.totals, rp.list);
    rp.section = section("d-regions", null, null, rp.el);
  }
  // (Asked only while the Regions tab shows: the lead counts them only while a page asks.)
  if (location.hash === "#regions") askSteps(m);
  rp.rows = md.rows;
  // What's left in all of them, a step at a time (the forecast's regions summed in the page).
  const order = Object.keys(table.steps);
  const words = Object.entries(stepTotals(m.fc?.regions)).sort(([p], [q]) => order.indexOf(p) - order.indexOf(q)).map(([s, k]) => S.said(s, k));
  const secs = rp.rows.reduce((t, r) => t + r.secs, 0);
  rp.totals.replaceChildren(!m.fc ? "No forecast yet (the lead makes one with each plan): the regions alone."
    : words.length ? `Left in ${plural(rp.rows.filter((r) => r.count).length, "region")}${secs ? ` (≈ ${dur(secs)} of work)` : ""}: ${words.join(" · ")}` : "Nothing left to build in any region.");
  relist();
  return rp.section;
}

// The map's last catalogue (agent::Published's `last`, else the last the lead read, else its last
// catalogue job's end): when, its number and its kind (with the low zooms or map tiles only).
function lastCatalogue(m) {
  const p = m.a.published?.last || m.a.catalog;
  if (p) return { at: p.at, n: p.n, kind: m.a.published?.last ? (m.a.published.low ? "with the low zooms" : "map tiles only") : null };
  const at = (m.a.recent || []).find((d) => stepOf(d.id) === "catalog" && d.ok)?.ended;
  return at ? { at } : null;
}

// ---- Activity -----------------------------------------------------------------------------------
function hours(m) {
  const rows = m.sw.rates?.rows;
  if (!rows?.length) return h("div", "small dim", "No history yet (the coordinator keeps it from this version on).");
  const who = [...new Set(rows.flatMap((r) => Object.keys(ui.metric === "busy" ? r.busy_s || {} : r.done || {})))];
  const val = (r, w) => (ui.metric === "busy" ? (r.busy_s?.[w] || 0) / 60 : r.done?.[w]?.unit || 0);
  const max = Math.max(1, ...rows.map((r) => who.reduce((t, w) => t + val(r, w), 0)));
  const chart = h("div", "hours", rows.map((r) => {
    const col = h("div", { class: r.paused_s > 1800 ? "paused" : "", title: `${clock(r.t)}–${clock(r.t + 3600)}: ${who.map((w) => `${w} ${ui.metric === "busy" ? `${Math.round(val(r, w))} min busy` : S.counted("unit", val(r, w))}`).join(", ")}` });
    for (const w of who) {
      const v = val(r, w);
      if (!v) continue;
      const i = h("i");
      Object.assign(i.style, { height: `${(v / max) * 100}%`, background: m.colourOf(w) });
      col.append(i);
    }
    return col;
  }));
  const axis = h("div", "hoursx", rows.map((r, i) => h("span", null, i % 6 === 0 ? String(new Date(r.t * 1000).getHours()).padStart(2, "0") : "")));
  // The day's totals by machine, each step counted in its noun.
  const sum = {};
  for (const r of rows) for (const [w, s] of Object.entries(r.done || {})) for (const [st, k] of Object.entries(s)) { sum[w] ??= {}; sum[w][st] = (sum[w][st] || 0) + k; }
  const totals = Object.entries(sum).map(([w, s]) => `${w}: ${Object.entries(s).sort((x, y) => y[1] - x[1]).map(([st, k]) => S.said(st, k)).join(", ")}`);
  const lastHour = rows.slice(-3).reduce((t, r) => t + Object.values(r.done || {}).reduce((u, s) => u + (s.unit || 0), 0), 0) / 3;
  return h("div", null,
    h("div", "seg", [["tiles", "unit: z6 tiles an hour"], ["busy", "Busy minutes"]].map(([k, t]) => h("button", { class: (ui.metric || "tiles") === k ? "on" : "", onclick: () => { ui.metric = k; render(); } }, t))),
    h("div", { style: { marginTop: "8px" } }, chart, axis),
    h("div", "small dim", `${lastHour.toFixed(1)} roads' tiles an hour lately · last 24 h: ${totals.join(" · ") || "nothing finished"}`),
    h("div", "legend", who.map((w) => h("span", null, h("i", { style: { background: m.colourOf(w) } }), w))));
}

// What happened, in words: an event (or a run of like ones, together), one sentence each, the
// step said as everywhere ("Terrain: 8 tiles").
function eventText(m, e, k = 1) {
  const who = e.worker || "";
  const nm = (id) => m.regionName[id] || id;
  const t = e.targets || [];
  // (Its job as the code names it, and its count: an older history's `what` held display names.)
  const what = e.step ? (t.length > 1 ? `${S.job(e.step, t)} (${S.counted(e.step, t.length)})` : S.job(e.step, t)) : "";
  switch (e.kind) {
    case "start": return [`${who} began ${what || e.note || ""}`, ""];
    case "end": {
      const job = what;
      return e.ok ? [`${who} finished ${job} in ${dur(e.secs || 0)}`, ""] : [`${who}'s ${job} ${e.note || "stopped"}${t.length ? ` (${S.counted(e.step, t.length)} done and kept)` : ""}`, /fail/.test(e.note) ? "bad" : "warn"];
    }
    case "lease": return [`${who} took ${what}`, "dim"];
    case "done": return [`${who} handed back ${what}, ${dur(e.secs || 0)} after taking it`, ""];
    case "fail": return [`${who}'s ${what} ${/^stopped/.test(e.note) ? e.note : `failed: ${e.note}`}`, /^stopped/.test(e.note) ? "warn" : "bad"];
    case "lapse": return [`${who} went quiet: its lease on ${e.note || what} lapsed and the work is offered again`, "warn"];
    case "task": return [k > 1 ? `${who} did ${plural(k, "task")}` : `${who} did a task${t[0] ? ` (${t[0]})` : ""} in ${dur(e.secs || 0)}`, "dim"];
    // (Given back, not failed: the page reloaded or closed, or emptied in the background.)
    case "task-back": return [`${who}: a task given back (${e.note})`, "warn"];
    case "task-fail": return /^stopped:/.test(e.note || "") ? [`${who}: a task given back (${e.note.replace(/^stopped:\s*/, "")})`, "warn"] : [`${who}: a task failed: ${e.note}`, "bad"];
    case "catalog": return [t.length ? `The map got ${t.length > 3 ? `${t.length} regions` : t.map(nm).join(", ")}` : "The map was updated", "ok"];
    case "pause": return [`The build was paused by ${e.note}`, "warn"];
    case "resume": return ["The build went on", "ok"];
    case "worker": return [`${who}${e.note && e.note !== who ? ` (${e.note})` : ""} joined`, "run"];
    case "agent": return [`${who}'s agent started (${e.note})`, "run"];
    case "conditions": return [`${who}: ${e.note}`, /doesn't answer|battery|away/.test(e.note) ? "warn" : ""];
    // (A Mac's build caches cleared on its owner's ask (older history: trimmed after the build): what was freed.)
    case "caches": return [`${who} ${e.note}`, /^didn't/.test(e.note || "") ? "warn" : ""];
    case "term": case "member": return termText(e);
    default: return [`${e.kind} ${who} ${what}`, ""];
  }
}

const FILTERS = { all: () => true, problems: (e) => (e.ok === false && !/^stopped:/.test(e.note || "")) || e.kind === "lapse" || (e.kind === "task-fail" && !/^stopped:/.test(e.note || "")), publishing: (e) => e.kind === "catalog" || (e.kind === "end" && e.step === "catalog"), pauses: (e) => ["pause", "resume", "conditions", "agent"].includes(e.kind) };

function feed(m, events, seen) {
  // (A job's start is shown only while it runs: once it's ended, its end says it all.)
  const lastEnd = {};
  for (const e of events) if (e.kind === "end") lastEnd[e.worker] = e.seq;
  const shown = events.filter(FILTERS[ui.feed] || FILTERS.all).filter((e) => !(e.kind === "start" && (lastEnd[e.worker] ?? -1) > e.seq));
  // Newest first; a run of one worker's tasks together.
  const rows = [];
  for (let i = shown.length - 1; i >= 0; i--) {
    const e = shown[i];
    const prev = rows[rows.length - 1];
    if (prev && e.kind === "task" && prev.e.kind === "task" && prev.e.worker === e.worker && prev.first - e.t < 1800) { prev.k++; prev.first = e.t; continue; }
    rows.push({ e, k: 1, first: e.t });
    if (rows.length >= 200) break;
  }
  const out = h("div", "feed");
  let day = null;
  for (const r of rows) {
    const d = new Date(r.e.t * 1000).toDateString();
    if (d !== day) { out.append(h("div", "day", d === new Date().toDateString() ? "Today" : d)); day = d; }
    const [text, cls] = eventText(m, r.e, r.k);
    // (A failure: tried again now, its back-off forgotten.)
    const again = r.e.kind === "fail" && r.e.step && r.e.targets?.length && !/^stopped/.test(r.e.note || "")
      ? h("button", { class: "quiet", title: "Forget its failures and back-off: it's offered and run again now", onclick: () => act({ retry: { step: r.e.step, target: r.e.targets[0] } }, `try ${r.e.targets[0]} again`) }, "Try again") : null;
    out.append(h("div", { class: isNew(r.e, seen) ? "new" : "" }, h("span", "t", new Date(r.e.t * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })), h("span", cls, r.e.step ? [swatch(S.colour(r.e.step)), text] : text), again));
  }
  if (!rows.length) out.append(h("div", "dim", events.length ? "Nothing of that kind lately." : "Nothing yet."));
  return out;
}

// What happened since this page last showed it: a sentence.
function sinceLine(m, events, seen) {
  if (seen == null) return null;
  const fresh = events.filter((e) => isNew(e, seen));
  if (!fresh.length) return h("div", "since dim", "Nothing new since you last looked.");
  const tiles = {};
  let failures = 0, updates = [];
  for (const e of fresh) {
    if ((e.kind === "end" || e.kind === "done") && e.step === "unit" && e.ok !== false) tiles[e.worker] = (tiles[e.worker] || 0) + (e.targets || []).length;
    // (Stops aren't failures: the build pausing, the Mac sleeping, a lease lapsing, an agent stopping.)
    if (e.ok === false && !/^(stopped|paused|its lease lapsed|the agent stopped|another Mac took)/.test(e.note || "")) failures++;
    if (e.kind === "catalog") updates.push(e);
  }
  const total = Object.values(tiles).reduce((t, k) => t + k, 0);
  const bits = [];
  if (total) bits.push(`${S.said("unit", total)} built (${Object.entries(tiles).map(([w, k]) => `${k} by ${w}`).join(", ")})`);
  if (updates.length) bits.push(`${plural(updates.length, "catalogue")}${updates.some((r) => r.targets?.length) ? `: ${updates.flatMap((r) => r.targets || []).slice(0, 4).map((id) => m.regionName[id] || id).join(", ")}` : ""}`);
  if (failures) bits.push(h("span", "bad", plural(failures, "failure")));
  const first = fresh[0];
  return h("div", "since", `Since ${clock(first.t)}: `, ...bits.flatMap((b, i) => (i ? [" · ", b] : [b])), bits.length ? "" : `${plural(fresh.length, "event")}`);
}

// ---- Details ------------------------------------------------------------------------------------
function details(m) {
  const { sw, a, now } = m;
  const box = h("div", "box");
  // (Each in a box that scrolls sideways on a phone.)
  const tbl = (head, rows) => h("div", "twrap", h("table", "t", h("tr", null, head.map((c) => h("th", null, c))), rows.map((r) => h("tr", null, r.map((c) => h("td", null, c))))));
  const fc = m.fc;
  if (fc?.done_at) {
    const workLeft = fc.steps.reduce((t, s) => t + s.work_s, 0);
    box.append(h("h3", null, "Forecast", h("span", null, `made ${ago(now, fc.at)}`)),
      h("div", "small", `${dur(workLeft)} of work left at the lead's pace; with every machine, done ≈ ${clock(fc.done_at)}, if the Macs keep going (awake, on mains or above 30%, reaching the NAS)`),
      h("div", "small dim", `${Math.round((fc.measured || 0) * 100)}% of that time measured, the rest estimated`),
      h("div", "small dim", liveError(fc)));
  }
  const leases = sw.leases || [];
  box.append(h("h3", null, "Leases", h("span", null, "who holds what; a worker that stops beating gives its work back")),
    leases.length ? tbl(["Worker", "Work", "For", "Last beat", "Lapses in", "Says"], leases.map((l) => [l.worker, S.said(l.step, l.n), dur(l.for_s), l.beat_s != null ? `${dur(l.beat_s)} ago` : "–", l.lapses_in_s != null ? dur(l.lapses_in_s) : "–", l.progress || ""])) : h("div", "small dim", "None."));
  const waits = a.waiting || [];
  box.append(h("h3", null, "Waiting", h("span", null, "what can't run yet, and why")), waits.length ? tbl(["What", "Why"], waits.map((w) => [waitName(w), w.why])) : h("div", "small dim", "Nothing."));
  // The timetables' to-do list and the routes matching nothing on the map (docs/inputs.md §6.7.3).
  const tt = a.timetables;
  if (tt) {
    box.append(h("h3", null, "Timetables", h("span", null, `${n(tt.to_do)} lines with no timetable (${n(tt.rail)} rail, ${n(tt.ferry)} ferry, ${n(Math.round(tt.to_do_km))} km): ${tt.file}`)),
      tt.unmatched_total ? tbl(["Mode", "Timetable", "Route", "OSM pin", "Matches nothing because"], tt.unmatched.map((u) => [u.mode, u.source, u.name ? `${u.route} (${u.name})` : u.route, u.osm || "", u.why])) : h("div", "small dim", "Every timetable route matches a line on the map."));
    if (tt.unmatched_total > tt.unmatched.length) box.append(h("div", "small dim", `… and ${n(tt.unmatched_total - tt.unmatched.length)} more (work/timetables/report)`));
  }
  const recent = a.recent || [];
  box.append(h("h3", null, "The lead's last jobs"), recent.length ? tbl(["", "Job", "Took", "Ended", "Note"], recent.slice(0, 20).map((d) => [d.ok ? "✓" : "✗", d.id, dur(d.secs), clock(d.ended), (d.note || "").split("\n").slice(-2).join(" ").slice(0, 160)])) : h("div", "small dim", "None yet."));
  if ((a.bad_recipes || []).length) box.append(h("h3", null, "Region recipes that don't read"), tbl(["File", "Problem"], a.bad_recipes));
  const lj = jobsOf(a)[0];
  if (lj?.tail) box.append(h("h3", null, "The lead's job's last lines"), h("pre", { class: "small dim", style: { whiteSpace: "pre-wrap", margin: 0 } }, lj.tail));
  // Each member's capacity as the lead's coordinator last heard it (coord::Capacity: the lead's own first).
  const caps = a.capacity || [];
  const gb = (mb) => `${n(Math.round(mb / 102.4) / 10)} GB`;
  if (caps.length) box.append(h("h3", null, "Capacity per Mac"), tbl(["Mac", "Memory free", "Cores free", "Disk free", "Power", "Use", "NAS", "Leases", "Said"], caps.map(([name, c]) => [name, `${gb(c.mem_mb)} of ${gb(c.limit_mb)} (${gb(c.held_mb)} held)`, `${n(c.cores_free)} of ${n(c.cores)}`, gb(c.disk_mb), c.ac ? "mains" : `battery${c.battery != null ? ` ${c.battery} %` : ""}`, c.in_use ? "in use" : "idle", !c.nas ? "not reachable" : c.home ? "reachable" : "Tailscale", (c.leases || []).join(", "), ago(now, c.at)])));
  const workers = (sw.workers || []).filter((w) => w.seen_s < 3600);
  if (workers.length) box.append(h("h3", null, "Workers heard from"), tbl(["Name", "Kind", "Doing", "Spares", "Done", "Failed", "Seen"], workers.map((w) => [w.label || w.name, w.kind, (w.what || "").slice(0, 80), w.mem_mb ? `${n(w.mem_mb)} MB` : "", n(w.done), n(w.failed), `${dur(w.seen_s)} ago`])));
  // The steps table itself: every step the same row (docs/pipeline.md), with where its runs' time
  // goes (the lead's measured phase shares, `a.phase_shares`: the mean over its last runs).
  const rows = Object.values(table.steps);
  if (rows.length) box.append(h("h3", null, "The steps", h("span", null, "every step's row of the table: what it makes, of what, where it runs; time by phase: each phase's mean share of a run's time over its last good runs, and of the bytes read and written (I/O: local disk, NAS and internet alike)")), tbl(["Stage", "Step", "Does", "Kind", "Each target", "Runs on", "A job", "Time by phase"], rows.map((s) => [s.stage, s.id, s.does, s.kind, s.of ? `a task of ${s.of}` : s.scope === "world" ? "the world" : `${/^[aeiou]/.test(s.noun) ? "an" : "a"} ${s.noun}`, s.place === "any" ? "any Mac" : "the lead", s.of ? "" : s.place === "any" ? "a quarter hour's" : "all", phaseShares(a.phase_shares?.[s.id])])));
  box.append(h("div", "fresh", `Updated ${clock(now)} · the lead's heartbeat ${a.beat ? ago(now, a.beat) : "–"}${a.started ? `, its agent running since ${clock(a.started)} (app ${a.app})` : ""}${m.fc ? ` · forecast ${ago(now, m.fc.at)}` : ""} · history #${sw.seq ?? "–"}`));
  return box;
}

// ---- The page -----------------------------------------------------------------------------------
let ctx = null;
let last = null;
// The last render's Macs (each with its colour), for activity.js's chips.
let shownMacs = [];
// The regions' outlines, folders and sizes (`/work/regions`: crate::agent::regionsview), by id; the
// catalogue they were sized by, and when they were asked.
const drawn = { byId: {}, catalogue: undefined, at: 0, asking: false };
const REGIONS_EVERY_MS = 600000;
// Each region's targets built of all by step (`/work/regions` with `steps`: the lead counts them
// while asked, crate::agent::build::region_steps), asked each `STEPS_EVERY_MS` while the Regions tab
// is drawn: this Mac's agent's when it counts them (the lead, or out of the pool), else the lead's.
const regionSteps = { regions: {}, at: 0, asked: 0, asking: false, st: {} };
const STEPS_EVERY_MS = 30000;
function askSteps(m) {
  if (regionSteps.asking || Date.now() - regionSteps.asked < STEPS_EVERY_MS) return;
  regionSteps.asking = true;
  regionSteps.asked = Date.now();
  const body = { steps: true };
  const lead = (last?.macs || []).find((x) => x.host === m.a.host);
  // (An agent before #636 answers the outlines, `regions` a list: none counted there.)
  const counted = (v) => v?.regions && !Array.isArray(v.regions);
  ctx.call("/work/regions", body).then(([code, v]) => (code === 200 && counted(v) ? v : lead ? remote(lead, regionSteps.st, body, "/work/regions") : null))
    .catch(() => null)
    .then((v) => { regionSteps.asking = false; if (counted(v) && v.at !== regionSteps.at) { regionSteps.regions = v.regions; regionSteps.at = v.at; render(); } });
}
let events = [];
// What this browser last showed: its last event's time (unix s; null: a first visit). By time, not
// by number: an event's number is the answering Mac's (`epoch`), new when its agent restarts.
let seenAt = undefined;
// The numbering the events are in (the answer's `epoch`: a Mac's agent restarted, another Mac
// answering).
let epoch = undefined;
const SEEN_KEY = "seenAt";
// An event shown since (later than the last second shown).
function isNew(e, seen) {
  return seen != null && e.t > seen;
}
// Whether the last answer was the build's (a 503 after it: what's shown is stale).
let answered = false;
let retry = null;
// When the 503s in a row began (ms), null while answered.
let readSince = null;
function waitRead() {
  readSince ??= Date.now();
  const w = notRead(answered, readSince, Date.now());
  if (w.state) ctx.onState?.(w.state);
  if (w.error) ctx.onState?.(`can't show the build: ${w.error}`);
  clearTimeout(retry);
  // (Like the regular poll, only while the page is seen: none from a closed popover.)
  if (w.retryMs) retry = setTimeout(() => document.visibilityState === "visible" && poll(), w.retryMs);
}

function section(cls, title, sub, ...body) {
  return h("section", { class: cls }, title ? h("h2", null, title, sub ? h("span", null, sub) : null) : null, ...body);
}

// The pause controls (task #673): each press opens a small confirmation (a pinned card, `pinCard("ask", …, kind)`);
// the confirmed one shows pending at once and the page asks again each second until the build's state is
// what was asked (`ctx.ask`). The header's sentence is always the state as the build reports it.
const ASKS = {
  drain: { title: "Pause at a safe point?", text: "Every Mac's job stops at its next safe point; nothing new starts. Resume lets it go on.", ok: "Pause", busy: "Pausing…", pause: { mode: "drain" } },
  freeze: { title: "Freeze every job now?", text: "Every Mac's job is frozen where it is, at once, and goes on from there when resumed.", ok: "Freeze", busy: "Freezing…", pause: { mode: "freeze" } },
  resume: { title: "Resume the build?", text: "The paused jobs go on and new work starts again.", ok: "Resume", busy: "Resuming…", pause: null },
};
const askCard = (kind) => {
  const a = ASKS[kind];
  return [h("div", "ct", a.title), h("div", "dim", a.text),
    h("div", "ask-btns", h("button", { class: "quiet", onclick: unpin }, "Cancel"), h("button", { class: "primary", onclick: () => { unpin(); ctx.ask(kind); } }, a.ok))];
};
let pending = null; // {kind, at}: a press waiting for the build to say so
const settled = (kind, p) => (kind === "resume" ? !p : kind === "freeze" ? p?.mode === "freeze" : !!p);
function head(m) {
  cards.set("ask", askCard); // (The cards are made again with each render.)
  const open = (kind) => (e) => {
    e.stopPropagation(); // (Not the page's own click: it lets a card go.)
    const r = e.currentTarget.getBoundingClientRect();
    pinCard("ask", r.left + r.width / 2, r.bottom - 6, kind);
    // (For the keyboard: on Cancel, so Enter or Space twice never acts; Escape lets it go.)
    pin.back = e.currentTarget;
    pin.el.querySelector(".ask-btns button")?.focus();
  };
  const btn = (kind, label, cls, title) => {
    const on = pending?.kind === kind;
    return h("button", { class: [cls, on ? "pending" : ""].filter(Boolean).join(" "), disabled: !!pending, "aria-busy": on ? "true" : null, onclick: open(kind), title }, on ? ASKS[kind].busy : label);
  };
  const ctl = h("div", "ctl", m.sw.pause
    ? btn("resume", "Resume", "primary", "Lets every job go on")
    : [btn("drain", "Pause at a safe point", "", "Every Mac's job stops at its next safe point; nothing new starts"),
      btn("freeze", "Freeze now", "quiet", "Freeze every Mac's job where it is, at once")]);
  const say = verdictText(m);
  if (pending) say.push(h("span", "dim", ` · ${ASKS[pending.kind].busy.replace("…", "")} asked, the build hasn't said so yet`));
  return [h("div", "verdict", h("div", "say", say), ctl)];
}

// The alerts, full width above the Overview's map (the owner, 21 Oct 04:00), not in the sticky
// header: each says what and links to the tab that says more.
function alertList(m) {
  const xs = alerts(m);
  return xs.length ? h("div", "alerts", xs.map((x) => h("a", { class: `alert ${x.cls}`, href: `#${x.to}` }, h("i", null, x.cls === "info" ? "i" : "!"), h("span", null, x.text), h("span", "go", `Open ${x.to}`)))) : null;
}

// One section, built alone: a failure (an answer missing a field, say) shows in that section, and
// the others and the tab still render. (Logged once a section and message, not on every render.)
const partErrors = new Set();
function part(cls, build) {
  try { return build(); } catch (e) {
    const key = `${cls}\n${e.message}`;
    if (!partErrors.has(key)) { partErrors.add(key); console.error(`the ${cls} section:`, e); }
    return section(cls, null, null, h("div", "box dim small", `This part couldn't be shown: ${e.message}`));
  }
}

// Each tab's sections, in order (index.html's panels, by the tab's name); `md` the map's data.
function tabs(m, md) {
  const fin = m.lines.filter(finished).length;
  return {
    overview: [
      part("d-alerts", () => alertList(m)),
      part("d-overview", () => section("d-overview", null, null, overview(m, md))),
    ],
    jobs: [
      part("d-sched", () => scheduleSection(m)),
      part("d-stages", () => section("d-stages", "Stages", m.lines.length ? `${fin} of ${m.lines.length} done · ⇄ any Mac may take it` : "", h("div", "box", stagesList(m)))),
    ],
    machines: [
      part("d-machines", () => section("d-machines", "Machines", `${m.macs.filter((x) => x.fresh).length} Macs${m.pages.length ? `, ${plural(m.pages.length, "page")}` : ""}`,
        h("div", "machines", m.macs.map((x) => machineCard(m, x)), m.gone.map(goneCard), pagesCard(m)))),
      part("d-pool", () => poolSection(last.agent, m.now, (body) => ctx.call("/work/lead", body).then(poll, (e) => ctx.onError?.(`couldn't ask about the lead: ${e.message}`))) || section("d-pool")),
      part("d-settings", () => settingsSection(ctx.call, last?.macs || [], render, ctx.onError)),
    ],
    regions: [part("d-regions", () => regionsPane(m, md))],
    nas: [part("d-nas", () => nasSection(last.agent))],
    inputs: [part("d-inputs", () => inputsTab(m))],
    activity: [part("d-activity", () => section("d-activity", "Activity", "the last day",
      h("div", "box", hours(m),
        h("div", "feedbar", h("div", "seg", [["all", "All"], ["problems", "Problems"], ["publishing", "Publishing"], ["pauses", "Pauses & conditions"]].map(([k, t]) => h("button", { class: ui.feed === k ? "on" : "", onclick: () => { ui.feed = k; render(); } }, t)))),
        sinceLine(m, events, seenAt), feed(m, events, seenAt)))), part("d-xfer", () => transfersPart())],
    details: [part("d-details", () => section("d-details", null, null, details(m)))],
  };
}

function render() {
  if (!last || !ctx) return;
  let m;
  try { m = model(last); } catch (e) {
    // (An answer the page can't read at all: said in the heading, the tabs left as they were.)
    ctx.head.replaceChildren(part("d-head", () => { throw e; }));
    return;
  }
  shownMacs = m.macs;
  // (A part where the user is typing, choosing in a list or has text selected stays as it was
  // until they're done: a refresh would take their keyboard or selection away. A checkbox or
  // radio focused by its click isn't that: kept, the map's part kept its old filters while the new
  // part had taken the map's element, and the map went (#627).)
  const sel = window.getSelection();
  const ae = document.activeElement;
  const typing = ae && (ae.tagName === "SELECT" || (ae.tagName === "INPUT" && !["checkbox", "radio", "button"].includes(ae.type)));
  const busy = (el) => el && ((typing && el.contains(ae)) || (sel && !sel.isCollapsed && el.contains(sel.anchorNode)));
  const put = (root, next) => {
    // (A part kept across renders, the Regions tab's, stays where it is.)
    if (root.children.length === next.length) next.forEach((el, i) => { if (root.children[i] !== el && !busy(root.children[i])) root.children[i].replaceWith(el); });
    else root.replaceChildren(...next);
  };
  // (The page's scroll on a phone, the tabs' panel's beside the map: kept.)
  const scrollY = window.scrollY, paneY = ctx.tabsEl.scrollTop;
  cards.clear();
  const feedTop = document.querySelector(".feed")?.scrollTop || 0;
  put(ctx.head, [].concat(part("d-head", () => head(m))));
  let md = null;
  try { md = mapData(m); } catch (e) { part("map data", () => { throw e; }); }
  put(ctx.mapPane, [part("d-map", () => mapSection(md))]);
  for (const [name, next] of Object.entries(tabs(m, md))) if (ctx.panels[name]) put(ctx.panels[name], next.filter(Boolean));
  const feedEl = document.querySelector(".feed");
  if (feedEl) feedEl.scrollTop = feedTop;
  window.scrollTo(0, scrollY);
  ctx.tabsEl.scrollTop = paneY;
  refreshPin();
  ctx.onState?.(m.sw.pause ? "paused" : !m.a.host ? "no word from the lead" : m.fresh(m.a.beat) ? (jobsOf(m.a).length ? "building" : "idle") : "lead out of touch");
}

let polling = null;
function poll() {
  // (One at a time: two would fetch the same events twice.)
  polling ??= pollOnce().finally(() => (polling = null));
  return polling;
}

// The steps table, once (again on a poll until a lead answers it; a lead without it: ids).
let tableAsked = false;
async function loadTable() {
  if (tableAsked) return;
  try {
    const [code, t] = await ctx.call("/work/steps", {});
    if (code === 200 && t?.steps) {
      table.steps = Object.fromEntries(t.steps.map((s) => [s.id, s]));
      table.stages = t.stages || [];
      tableAsked = true;
    } else if (code === 404) tableAsked = true;
  } catch { /* (Asked again with the next poll.) */ }
}

// The regions as `/work/regions` has them: again when the lead's last catalogue changes, else every
// ten minutes (a region's folder or outline changes with the regions' accepted version); not yet
// made (503): asked again next poll.
async function loadRegions(sw) {
  const n = sw.agent?.catalog?.n ?? null;
  if (drawn.asking || (drawn.at && drawn.catalogue === n && Date.now() - drawn.at < REGIONS_EVERY_MS)) return;
  drawn.asking = true;
  try {
    const [code, v] = await ctx.call("/work/regions", {});
    if (code !== 200 || !v?.regions) return;
    drawn.byId = Object.fromEntries(v.regions.map((r) => [r.id, r]));
    drawn.catalogue = v.catalog ?? null; // the catalogue it was made from: asked again until it catches up
    drawn.at = Date.now();
    render();
  } catch { /* (Asked again with the next poll.) */ } finally {
    drawn.asking = false;
  }
}

// (Told when the numbering changes: the page's own record of what it showed starts again too.)
let onEpoch = null;

async function pollOnce() {
  if (!ctx) return;
  try {
    await loadTable();
    const [code, sw] = await ctx.call("/work/swarm", {});
    // (This Mac's agent hasn't read the build's state yet: a quiet line, asked again in 2 s, an
    // error after a minute of it: sums.js notRead.)
    if (code === 503) return waitRead();
    if (code !== 200 || !sw) return;
    answered = true;
    readSince = null;
    clearTimeout(retry);
    last = sw;
    if (pending && settled(pending.kind, sw.pause)) pending = null;
    // (Not waited for: the rows show the regions' folders and sizes once they come.)
    loadRegions(sw);
    ctx.onAnswer?.();
    // (Another numbering, or this one gone back past the last event this page has: started again.)
    if (sw.epoch !== epoch || (sw.seq != null && events.length && sw.seq < events[events.length - 1].seq)) {
      epoch = sw.epoch;
      events = [];
      onEpoch?.();
    }
    // What happened since the last event this page has (a few hundred at first).
    if (sw.seq != null) {
      const from = events.length ? events[events.length - 1].seq : Math.max(0, sw.seq - 600);
      if (sw.seq > from) {
        const [c2, hist] = await ctx.call("/work/history", { since: from, max: 1000 }).catch(() => [0, null]);
        // (A 503 here, the swarm answered: left to the next regular poll.)
        if (c2 === 200 && hist?.events) {
          const last = events.length ? events[events.length - 1].seq : -1;
          events = events.concat(hist.events.filter((e) => e.seq > last)).slice(-2000);
        }
      }
      // (What this browser last showed: none on a first visit, then nothing's highlighted.)
      if (seenAt === undefined) seenAt = ctx.store.get(SEEN_KEY, null);
    }
    render();
    if (location.hash === "#machines") loadSettings(ctx.call, last?.macs || [], render);
    // (Not waited for: the schedule shows the jobs further back as they come.)
    loadPast(sw);
  } catch (e) {
    ctx.onState?.(`can't reach the lead (${e.message})`);
  }
}

/** Starts the dashboard: its header status in `head`, the world map in `mapPane`, each tab's
 * sections in `panels[tab]`, all of them inside `tabsEl` (the right panel, scrolled on its own);
 * `onAnswer()` on each answer of the build's (the footer's Live dot); `call(path, body)` asks the coordinator ([status, json]); `ask(kind)`
 * pauses the build ("drain", "freeze") or lets it go on ("resume"); `store` keeps this browser's own (what it last showed). */
export function dashboard(opts) {
  ctx = opts;
  wireCards();
  new ResizeObserver(() => rowHeight(ctx.tabsEl.clientWidth)).observe(ctx.tabsEl);
  let watching = false;
  ctx.ask = async (kind) => {
    const a = ASKS[kind];
    pending = { kind, at: Date.now() };
    render();
    try {
      // (No time of this browser's: the coordinator's clock orders the asks, and stamps it.)
      const [code] = await ctx.call("/work/pause", { pause: a.pause && { ...a.pause, by: ctx.label, at: 0 } });
      if (code !== 200) throw new Error(`answer ${code}`);
    } catch (e) {
      pending = null;
      render();
      return ctx.onError?.(`couldn't ask the build to ${a.pause ? "pause" : "go on"}: ${e.message}`);
    }
    // (The state follows the ask a few seconds later, through the agent and the NAS: asked again each second until it does.)
    if (watching) return;
    watching = true;
    while (pending) {
      await poll();
      if (!pending) break;
      if (Date.now() - pending.at > PENDING_MAX_MS) {
        const what = ASKS[pending.kind].busy.replace("…", "").toLowerCase();
        pending = null;
        render();
        ctx.onError?.(`the build hasn't said it's ${what} after ${PENDING_MAX_MS / 1000} s (the lead may be out of touch)`);
        break;
      }
      await new Promise((ok) => setTimeout(ok, PENDING_POLL_MS));
    }
    watching = false;
  };
  // What this page has shown, kept as it's left: when it's back (or the next visit), what came
  // since is highlighted ("since you last looked").
  let left = null;
  onEpoch = () => { left = null; };
  const keep = () => { if (events.length) { left = events[events.length - 1].t; ctx.store.set(SEEN_KEY, left); } };
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") keep();
    else { if (left != null) seenAt = left; poll(); }
  });
  window.addEventListener("pagehide", keep);
  setInterval(() => document.visibilityState === "visible" && poll(), POLL_MS);
  // (The Machines tab's settings asked as it opens, not at the next poll.)
  window.addEventListener("hashchange", () => last && location.hash === "#machines" && loadSettings(ctx.call, last.macs || [], render));
  // (The Regions tab's stage counts asked as it opens: asked only while it shows, askSteps.)
  window.addEventListener("hashchange", () => last && location.hash === "#regions" && render());
  poll();
  startLive({ call: ctx.call, store: ctx.store, macs: () => last?.macs || [], active: () => location.hash === "#machines" || location.hash === "#nas", only: () => (location.hash === "#nas" ? last?.agent?.host || null : null) });
  startActivity({ call: ctx.call, macs: () => last?.macs || [], machines: () => shownMacs, active: () => location.hash === "#activity" });
}
