// The whole build at a glance (docs/workers.md, The page): the page at /work/, for anyone it answers.
// From the coordinator: /work/steps once (the steps table: every step's title, noun, stage and
// place, docs/pipeline.md), /work/swarm every 10 s (the lead's heartbeat with its checklist,
// forecast and resources, its helpers', every worker and lease, the hours' work), and
// /work/history after the last event this page has (what happened; further back for the
// schedule's past jobs).
//
// One shape for everything: a step is named by its title, its targets counted in its noun ("8 of
// 480 tiles"), coloured by its stage; a stage is a line over its steps' rows, each row the same
// cells; a machine is a card of its jobs, each job the same lines; an event is one templated
// sentence. The parts: the header's verdict (paused, out of touch or idle; percent done, done when; what needs
// a look, the pause); by tab, the overview (how far, how long, the map's last catalogue, the world map: worldmap.js), the
// jobs (each Mac's schedule, past and to come; the stages, each step's jobs), the machines (what each does now, inside
// each job, why one waits, and live on each: live.js), the regions, and the activity (the hours,
// what happened, what each Mac moves now and the internet by website: activity.js), and the details.

import { poolSection, termText } from "./pool.js";
import { TILE_STATES, busyLanes, finished, finishedTiles, isSlotOf, laneGroups, notRead, packRuns, pastFrom, pastRuns, regionWork, regionsShown, rowsWindow, runningTiles, slotName, slotNo, stageSegments, stepTotals, targetsOf, mapTilesKept, macSlot, tileCovers, tileOf, wholeProgress } from "./sums.js";
import { fmt, timeTicks } from "./chartmath.js";
import { liveBlock, startLive } from "./live.js";
import { createMap } from "./worldmap.js";
import { loadSettings, settingsSection } from "./settings.js";
import { nasSection } from "./nas.js";
import { startActivity, transfersPart } from "./activity.js";

const POLL_MS = 10000;
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
// Every word about a step comes from its row: its title, the noun its targets are counted in, its
// stage (its colour) and where it runs. Before the table has loaded (or from a lead without it) a
// step goes by its id.
// A stage's colour: its token in index.html (--st-<stage>, a hue each in either theme).
const stageColour = (g) => `var(--st-${g || "upkeep"}, var(--st-upkeep))`;
const table = { steps: {}, stages: [] };
const S = {
  row: (id) => table.steps[id],
  title: (id) => table.steps[id]?.title || id || "",
  noun: (id) => table.steps[id]?.noun || "target",
  world: (id) => table.steps[id]?.scope === "world",
  stage: (id) => table.steps[id]?.stage || "",
  stageTitle: (id) => table.stages.find((g) => g.id === id)?.title || id,
  colour: (id) => stageColour(S.stage(id)),
  any: (id) => table.steps[id]?.place === "any",
  // "8 tiles"; a worldwide step's runs: "1 run".
  counted: (id, k) => plural(k, S.noun(id)),
  // "Terrain: 8 tiles", "Terrain: 8 of 480 tiles", "Outlines" (a worldwide step's one run).
  said: (id, k, of) => (S.world(id) || !k ? S.title(id) : `${S.title(id)}: ${of != null && of !== k ? `${n(k)} of ${n(of)} ${S.noun(id)}s` : S.counted(id, k)}`),
};
// A step's line of the checklist (build::Item) or a stage's (build::Line) in words: "12 of 480
// tiles", "3 runs left", "done", "to come".
function countOf(x, noun) {
  if (x.total != null && x.total > 0) return `${n(x.done)} of ${n(x.total)} ${noun}`;
  if (x.left === 0) return "done";
  if (x.left != null) return `${plural(x.left, noun.replace(/s$/, ""))} left`;
  return "to come";
}
// A stage's counts, a noun at a time in its items' order (build::Line::counts): "398 of 398 tiles ·
// 18 of 18 z3 tiles", "2 runs left"; "done" or "to come" with no item to count (an older lead's line:
// its own count).
function countsOf(l) {
  const out = [];
  const nouns = [];
  for (const i of l.items || []) {
    const noun = S.noun(i.step);
    if (nouns.includes(noun)) continue;
    nouns.push(noun);
    const of = l.items.filter((j) => S.noun(j.step) === noun);
    if (of.some((j) => j.total != null)) {
      if (of.every((j) => j.total != null)) out.push(`${n(of.reduce((t, j) => t + j.done, 0))} of ${n(of.reduce((t, j) => t + j.total, 0))} ${noun}s`);
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
    else if (k === "style") Object.assign(e.style, v);
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
const capOf = (a, host) => (a.capacity || []).find(([nm]) => isSlotOf(nm, host))?.[1];

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
    else if (/failed \d+ times?/.test(w.why)) out.push({ cls: "warn", text: `${w.what}: waiting out a failure`, to: "details" });
  }
  // Things to know that hold nothing back (agent::Notice): an info line each, apart from the waits.
  for (const x of a.notices || []) out.push({ cls: "info", text: `${x.what}: ${x.why}`, to: "details" });
  for (const p of m.pages) {
    if (p.bad) out.push({ cls: "bad", text: `${p.label}: a result differed; it gets no more work`, to: "machines" });
    else if (p.visible === false && p.seen_s < 300) out.push({ cls: "warn", text: `${p.label} is in the background (iPhones and iPads stop it)`, to: "machines" });
  }
  if ((a.bad_recipes || []).length) out.push({ cls: "warn", text: `${plural(a.bad_recipes.length, "region recipe")} doesn't read`, to: "details" });
  for (const v of (a.inputs || []).filter((v) => v.state === "held")) out.push({ cls: (v.findings || []).some((f) => f.level === "error") ? "bad" : "warn", text: `${v.unit}: a change held`, to: "dropboxes" });
  return out;
}

// ---- The drop boxes' gate (docs/inputs.md §4.7): the Drop boxes tab -----------------------------
// A banner per held unit: its held files, each finding, Accept per warning and Accept All; errors
// say what to fix. A quiet line per unit being checked; a line saying so when none waits.
function dropBoxes(m) {
  const accept = (body) => ctx.call("/work/inputs", body).then(poll, (e) => ctx.onError?.(`couldn't accept: ${e.message}`));
  const units = (m.a.inputs || []).filter((v) => v.state !== "ok");
  const held = units.filter((v) => v.state === "held").length;
  return section("d-inputs", "Drop boxes", held ? `${plural(held, "change")} held for a look` : "",
    units.length ? units.map((v) => gateBanner(v, accept)) : h("div", "box dim small", `Nothing held: ${plural((m.a.inputs || []).length, "drop box", "drop boxes")} taken in${(m.a.inputs || []).length ? ` (${m.a.inputs.map((v) => v.unit).join(", ")})` : ""}.`));
}
function gateBanner(v, accept) {
  if (v.state === "checking") return h("div", "gate checking", `${v.unit}: checking the files put in its drop box…`);
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
  // (The owner's line: the build's progress in percent and its finish, nothing more.)
  const whole = wholeProgress(m.lines, fc);
  if (whole) parts.push(parts.length ? " · " : "", h("b", null, `${whole.pct}%`));
  if (fc?.done_at) parts.push(" · done ≈ ", h("b", null, clock(fc.done_at)));
  else if (fc?.why) parts.push(` · ${fc.why}`);
  return parts;
}

// The next region (or regions, at the same time) to reach the map as it's built now: the forecast's
// earliest `map_at` (its last z6 tile's end, its map tiles drawn, a catalog within the minute).
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

// ---- The overview -------------------------------------------------------------------------------
// How far (the whole build's work, every stage's: sums.js wholeProgress), how long (the same work
// left), and the numbers that aren't a step's: the map, the machines (the regions: the world map's legend).
function overview(m) {
  const { a, fc, now, lines } = m;
  const whole = wholeProgress(lines, fc);
  const workLeft = fc ? fc.steps.reduce((t, s) => t + s.work_s, 0) : 0;
  const left = h("div", null,
    h("div", "big", whole ? `${whole.pct}%` : "–", h("small", null, whole ? ` of the whole build's work done (${dur(whole.done_s)} of ${dur(whole.done_s + whole.left_s)}, every stage's)` : "")),
    h("div", "small dim", !fc ? "No forecast yet (the lead makes one with each plan)" : fc.done_at ? `${dur(workLeft)} of work left at the lead's pace; with every machine, done ≈ ${clock(fc.done_at)}, if the Macs keep going (awake, on mains or above 30%, reaching the NAS)` : `No finish to forecast: ${fc.why || "unknown"}`),
    fc?.done_at ? h("div", "small dim", `${Math.round((fc.measured || 0) * 100)}% of that time measured, the rest estimated · forecast ${ago(now, fc.at)}`) : null,
    fc?.done_at ? h("div", "small dim", liveError(fc)) : null,
  );
  // The stages as a strip, each its colour, the one under way outlined; each with the Jobs tab's stage card (hover, pin).
  const strip = h("div", "strip", lines.map((l) => {
    const done = finished(l), running = (l.steps || []).some((s) => m.running.has(s));
    const i = withCard(h("i", { class: `${done ? "done" : ""} ${running ? "now" : ""}` }), `s ${l.id || l.what}`, () => stageCard(m, l));
    i.style.background = done ? S.colour(l.steps?.[0]) : "";
    if (!done && l.total) { const b = h("b"); b.style.width = `${Math.round((l.done / l.total) * 100)}%`; b.style.background = S.colour(l.steps?.[0]); i.append(b); }
    return i;
  }));
  const fin = lines.filter(finished).length;
  const nowLines = lines.filter((l) => !finished(l) && (l.steps || []).some((s) => m.running.has(s))).map((l) => l.what);
  const stagesLine = h("div", "small dim", lines.length ? `${fin} of ${lines.length} stages done${nowLines.length ? ` · now: ${nowLines.join(", ")}` : ""}` : "");
  const cat = lastCatalogue(m);
  const nextUpdate = nextOnMap(m);
  // Each Mac by name, a line each, with what it's doing.
  const doing = (x) => { const k = jobsOf(x.status).length; return !x.fresh ? "out of touch" : k ? plural(k, "job") : "idle"; };
  const kpis = h("div", "kpis",
    kpi("Last catalogue", cat ? `${clock(cat.at)} (${ago(now, cat.at)})` : "–", cat?.n != null ? `catalogue ${cat.n}${cat.kind ? `, ${cat.kind}` : ""}` : null),
    kpi("Next catalogue", nextUpdate ? `≈ ${clock(nextUpdate.at)}` : "–", m.a.publishing?.tiles ? `${plural(m.a.publishing.tiles, "map tile")} to draw` : null),
    kpi("Machines", h("div", "macs", m.macs.map((x) => h("div", null, x.name, h("span", "small dim", ` · ${doing(x)}`)))), m.pages.length ? `and ${plural(m.pages.length, "page")}` : null),
  );
  return h("div", "overview", h("div", "box", left, strip, stagesLine), kpis);
}

// ---- The world map (worldmap.js) -----------------------------------------------------------------
// The countries' outlines (countries.json, one of the page's files), fetched once (again on a
// render after a failure).
let countries = [];
let countriesAsked = false;
function loadCountries() {
  if (countriesAsked) return;
  countriesAsked = true;
  fetch("countries.json").then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status))))).then((c) => { countries = c; render(); }, () => { countriesAsked = false; });
}
let worldMap = null;
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
  for (const r of runs) r.targets = targetsOf(r, events);
  return runs;
}
// The world map's filters (#615, #627), this browser's own (ctx.store): the tiles by state and
// the regions by state (`tiles`, `regions`: those toggled off in the legend, false), finished
// tiles faded by age or not, one pool member's, the tile areas (all, z3, z6), the stages shown
// (`offStages`: those toggled off in the legend).
const MAP_F = { tiles: {}, regions: {}, fade: true, member: "", zoom: "", offStages: [] };
let mapF = null;
const mapFilters = () => (mapF ||= { ...MAP_F, ...(ctx.store.get("mapFilters", {}) || {}) });
function setMapFilter(k, v) { mapFilters()[k] = v; ctx.store.set("mapFilters", mapF); render(); }

// The world map's data for this render, the Overview's and the Regions tab's (one each:
// worldmap.js), as its filters keep it (sums.js mapTilesKept), with its tiles' cards, the
// regions' (regionCards) and the stack's: everything under a point, one card after another.
function mapData(m) {
  loadCountries();
  const f = mapFilters();
  const from = m.now - (ui.past || 6 * 3600);
  const runs = mapRuns(m);
  // (Jobs with no tile to show, a worldwide step's: named under the map, as the mockup does.)
  const world = runs.filter((r) => !r.targets.some((t) => tileOf(t)));
  const hosts = m.macs.map((x) => x.name);
  // (In the last catalogue: what finished by its job's start, when it took what was done; the
  // record's time is its end.)
  const catAt = lastCatalogue(m)?.at ?? null;
  const catJob = catAt == null ? null : (m.a.recent || []).find((d) => /^catalog/.test(stepOf(d.id)) && d.ok && Math.abs(d.ended - catAt) <= 5);
  const pubAt = catJob?.secs != null ? catAt - catJob.secs : catAt;
  // (The legend counts every state, those toggled off too; the map gets the kept.)
  const [allRun, allDone] = mapTilesKept(runningTiles(runs, m.fc?.each_s, m.now), finishedTiles(endsHeld(), from), { ...f, tiles: {} },
    { pubAt, macOf: (w) => macSlot(w, hosts).host, stageOf: S.stage });
  const tileCount = Object.fromEntries(TILE_STATES.map((st) => [st, 0]));
  for (const v of [...allRun.values(), ...allDone.values()]) tileCount[v.state]++;
  const kept = (mp) => new Map([...mp].filter(([, v]) => f.tiles?.[v.state] !== false));
  const running = kept(allRun), done = kept(allDone);
  const fcRegs = Object.fromEntries((m.fc?.regions || []).map((r) => [r.id, r]));
  const regs = Object.values(drawn.byId).map((r) => ({ ...r, state: regionState(fcRegs[r.id]) }));
  const head = (key, step, state) => ({ kicker: `${S.stageTitle(S.stage(step)) || S.title(step)} · ${state}`, colour: S.colour(step), title: `Tile ${key.slice(key.lastIndexOf(" ") + 1)}` });
  for (const [key, r] of running) {
    const t = tileOf(key), pct = `${Math.round(r.frac * 100)}%`;
    const state = r.frac >= 1 ? "completed, its job running" : r.current || r.frac > 0 ? "in progress" : "unstarted, to come in its job";
    cards.set(`mt ${key}`, () => cardBody({ ...head(key, r.step, state),
      stats: [[pct, "of the tile done"], [r.current ? r.run.live?.phase || "running" : r.frac >= 1 ? "done" : r.frac > 0 ? "part done" : "queued", "phase"]],
      bar: { frac: r.frac, label: r.frac >= 1 ? "done; its job runs on" : r.frac > 0 ? `≈ ${pct} of the tile` : "not begun" },
      foot: `${r.run.mac} · ${key}` }, [
      ["step", S.title(r.step)],
      ["job", r.run.what],
      ["on", r.run.mac],
      ["covers", tileCovers(t)],
    ], mapsShown().minimap(key)));
  }
  for (const [key, d] of done) {
    const out = d.state === "published";
    cards.set(`mt ${key}`, () => cardBody({ ...head(key, d.step, out ? "published" : "completed, not yet published"),
      stats: [[ago(m.now, d.t), "finished"], [out ? "yes" : "next catalogue", "in the app"]],
      bar: { frac: 1, label: `done ${clock(d.t)}` },
      foot: `${d.worker || "?"} · ${key}` }, [
      ["step", S.title(d.step)],
      ["on", d.worker],
      ["covers", tileCovers(tileOf(key))],
    ], mapsShown().minimap(key)));
  }
  const rows = regionRows(m);
  regionCards(rows);
  // (The stack: `also` its keys, "|" between, the deepest tile first; each card under a line.)
  cards.set("mstack", (also) => {
    const keys = (also || "").split("|").filter((k) => cards.has(k));
    const most = 8;
    return [...keys.slice(0, most).map((k) => h("div", "tc-sec", cards.get(k)())),
      keys.length > most ? h("div", "tc-more dim small", `and ${n(keys.length - most)} more here`) : null].filter(Boolean);
  });
  const regionKind = (st) => (st === "done" ? "on" : st);
  return { regs, rows, tileCount, world, data: { countries, regions: regs.filter((r) => f.regions?.[regionKind(r.state)] !== false), running, finished: done, now: m.now, from, colour: S.colour, stageOf: S.stage, fade: f.fade } };
}
// (A tile's minimap is drawn by either map: the Overview's unless only the Regions tab's exists.)
const mapsShown = () => worldMap || rp.map;
// A map's click or tap: its stack pinned (nothing there: the pinned card let go).
const mapPick = (keys, x, y) => (keys.length ? pinCard("mstack", x, y, keys.join("|")) : unpin());
// The map's legend (#627): what's drawn, counted, and every stage's colour, each item a toggle
// showing or hiding its kind on the map (off: struck and dimmed; kept as the filters are),
// hovered: the rest of the map faint; then the other filters.
const TILE_WORDS = { unstarted: "unstarted", progress: "in progress", completed: "completed, not yet published", published: "published" };
function mapLegend(m, md, map) {
  const { regs, tileCount, world } = md;
  const f = mapFilters();
  const count = (st) => regs.filter((r) => r.state === st || (st === "on" && r.state === "done")).length;
  const toggle = (on, flip, emph, swatch, text, title) => h("button", { type: "button", class: `lg${on ? "" : " off"}`, "aria-pressed": on ? "true" : "false", title,
    onclick: () => { map.emphasize(null); flip(); }, onpointerenter: (e) => e.pointerType !== "touch" && map.emphasize(emph), onpointerleave: () => map.emphasize(null) }, swatch, text);
  const byState = (k, st, cls, text) => toggle(f[k][st] !== false, () => setMapFilter(k, { ...f[k], [st]: f[k][st] === false }), { [k === "tiles" ? "tile" : "region"]: st }, h("i", cls), text, "Show or hide these on the map");
  const span = (PAST.find(([s]) => s === ui.past)?.[1] || "Past 6 hours").toLowerCase();
  const off = new Set(f.offStages || []);
  const btn = (text, v) => h("button", { type: "button", class: "mfbtn", onclick: () => setMapFilter("offStages", v) }, text);
  return [h("div", "wmap-legend small dim",
    regs.length ? [byState("regions", "on", "rg on", `${plural(count("on"), "region")} on the map`), byState("regions", "building", "rg building", `${plural(count("building"), "region")} building`), byState("regions", "queued", "rg queued", `${plural(count("queued"), "region")} to come`)] : h("span", null, "No regions' outlines from this Mac yet"),
    TILE_STATES.map((st) => byState("tiles", st, `tl ${st}`, `${plural(tileCount[st], "tile")} ${TILE_WORDS[st]}`)),
    world.length ? h("span", null, `whole world: ${world.map((r) => `${S.title(r.step)} (${r.mac})`).join(", ")}`) : null,
    h("span", "hint", "drag to move · pinch, double-click or +/− to zoom")),
  h("div", "wmap-legend wmap-stages small dim", table.stages.map((g) => toggle(!off.has(g.id), () => setMapFilter("offStages", off.has(g.id) ? [...off].filter((x) => x !== g.id) : [...off, g.id]), { stage: g.id },
    h("i", { style: { background: stageColour(g.id), borderColor: "transparent" } }), g.title, "Show or hide this stage's tiles on the map")),
    btn("Check all", []), btn("Uncheck all", table.stages.map((g) => g.id)),
    h("span", null, `finished in the ${span}: published solid, completed paler under a dashed outline · running: outlined, filled as far as done · steps at one spot: stripes`)),
  mapFilterBar(m)];
}
// The filters under a map (each map its own controls over one state). A member: the tiles it
// ran (the history's ends), runs or will run in its running jobs (theirs and its leases'); the
// forecast's lanes name no tiles, so its later jobs' aren't known.
let mapFiltersN = 0;
function mapFilterBar(m) {
  const f = mapFilters(), id = ++mapFiltersN;
  const radio = (k, v, text) => h("label", "mf", h("input", { type: "radio", name: `mf-${k}-${id}`, value: v, checked: f[k] === v ? "" : null, onchange: () => setMapFilter(k, v) }), text);
  const members = [...new Set([...m.macs.map((x) => x.name), ...(f.member ? [f.member] : [])])];
  return h("div", "wmap-filters small",
    h("div", "mfrow", h("label", "mf", h("input", { type: "checkbox", checked: f.fade ? "" : null, onchange: (e) => setMapFilter("fade", e.target.checked) }), "Fade completed tiles progressively")),
    h("div", { class: "mfrow", role: "radiogroup", "aria-label": "Show only for" }, h("span", "dim", "Show only for:"), radio("member", "", "All pool members"), members.map((nm) => radio("member", nm, nm))),
    h("div", { class: "mfrow", role: "radiogroup", "aria-label": "Show tile areas" }, h("span", "dim", "Show tile areas:"), radio("zoom", "", "All"), radio("zoom", "3", "z3"), radio("zoom", "6", "z6")));
}
function mapSection(m, md) {
  worldMap ||= createMap({ pick: mapPick });
  worldMap.update(md.data);
  const k = md.regs.length;
  return section("d-map", "Map", `${k ? plural(k, "region") : "no regions"} · the z3 grid · tiles in their stage's colour`, h("div", "box wmap-box", worldMap.el, ...mapLegend(m, md, worldMap)));
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
  if (l.phase) rows.push(h("div", null, h("span", "k", "phase"), h("b", null, l.phase), l.class ? h("span", "dim", ` ${l.class}`) : null, l.phase_since ? h("span", "dim", ` · ${dur(now - l.phase_since)}`) : null));
  if (l.target) {
    const each = fc?.each_s?.[stepOf(j.id)];
    const took = l.target_since ? now - l.target_since : null;
    const over = each && took != null ? took / each : null;
    rows.push(h("div", null, h("span", "k", "target"), h("b", null, l.target),
      h("span", over != null && over > 2 ? "warn" : "dim", took != null ? ` · ${dur(took)}${each ? ` of ≈ ${dur(each)}` : ""}` : ""),
      l.targets ? h("span", "dim", ` · ${n(l.targets_done)} of ${n(l.targets)} done`) : null));
    if (over != null) rows.push(pbar(over, over > 2 ? "warn" : "", S.colour(stepOf(j.id))));
  } else if (l.targets) rows.push(h("div", null, h("span", "k", "targets"), `${n(l.targets_done)} of ${n(l.targets)} done`));
  const bytes = [l.nas_read != null ? `NAS ↓ ${fmt(l.nas_read, "B")}` : null, l.nas_wrote != null ? `NAS ↑ ${fmt(l.nas_wrote, "B")}` : null,
    l.net_in != null ? `net ↓ ${fmt(l.net_in, "B")}` : null, l.net_out != null ? `net ↑ ${fmt(l.net_out, "B")}` : null].filter(Boolean);
  if (bytes.length) rows.push(h("div", null, h("span", "k", "moved"), bytes.join(" · ")));
  return rows.length ? h("div", { class: "inside", title: "Inside the job: its phase, its target under way (against the step's time a target), and the bytes it moved since it began" }, rows) : null;
}

function jobBlock(j, now, fc) {
  const p = j.progress;
  const step = stepOf(j.id);
  const box = h("div", "job", h("div", "what", swatch(S.colour(step)), j.what));
  if (j.parts?.length && j.part != null) {
    box.append(h("div", "parts", j.parts.map((_, i) => h("i", { class: i < j.part ? "done" : i === j.part ? "now" : "", title: j.parts[i] }))));
    box.append(h("div", "sub", `Part ${j.part + 1} of ${j.parts.length}: ${j.parts[j.part]}`));
  }
  if (p && p.total > 0) {
    box.append(pbar(p.done / p.total, j.paused ? "warn" : "", S.colour(step)));
    box.append(h("div", "sub", progressText(p, j, now)));
  }
  const bits = [`running ${dur(now - j.started)} (since ${clock(j.started)})`];
  if (j.threads) bits.push(`${j.threads} threads`);
  if (j.mem_mb) bits.push(`${n(Math.round(j.mem_mb / 102.4) / 10)} GB`);
  box.append(h("div", "sub", bits.join(" · ")));
  if (p?.moved_at && !j.paused && now - p.moved_at >= STUCK_S) box.append(h("div", "why", `No further for ${dur(now - p.moved_at)}: it may be stuck (its log's last lines are in the details)`));
  box.append(...[inside(j, now, fc)].filter(Boolean));
  if (j.paused) box.append(h("div", "why", `Frozen: ${j.paused}`));
  else if (j.pausing) box.append(h("div", "why", "Stopping at its next safe point (what it's on is kept)"));
  const t = jobTarget(j);
  if (t) box.append(h("div", "ctl", h("button", { class: "quiet", title: "Stopped now and given back to the build (not a failure); it runs again from the start", onclick: () => confirm(`Cancel ${j.what}?`) && act({ cancel: t }, `cancel ${j.what}`) }, "Cancel")));
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
    const bits = [`${S.title(f.step)}: ${n(f.fits)} of ${n(f.offered)} ${S.noun(f.step)}s it can take`];
    if (f.held) bits.push(`${n(f.held)} with others`);
    if (f.too_big) bits.push(`${n(f.too_big)} too big for its ${n(Math.round(w.mem_mb / 102.4) / 10)} GB`);
    if (f.kept_from) bits.push(`${n(f.kept_from)} kept from it after failing`);
    return h("div", null, bits.join(", "));
  });
  const d = h("details", { class: "sub fit", open: fitOpen.has(host), ontoggle: () => (d.open ? fitOpen.add(host) : fitOpen.delete(host)) },
    h("summary", null, `Offered ${plural(offered.length, "step")}: it can take ${n(fits)} of ${n(all)} pieces`), ...rows);
  return d;
}

// A capacity bar: its label, the bar (a part `used` of `whole`, and a second part `also` after
// it, lighter), its words.
function capBar(label, used, whole, words, also = 0, cls = "") {
  const pc = (v) => `${Math.round(Math.max(0, Math.min(1, whole ? v / whole : 0)) * 100)}%`;
  const bar = h("div", `cbar ${cls}`, h("i", { style: { width: pc(used) } }), also ? h("b", { style: { width: pc(also) } }) : null);
  return h("div", "cap", h("span", "k", label), bar, h("span", "v", words));
}
const gb = (mb) => `${(mb / 1024).toFixed(1)} GB`;

// A Mac's capacity as the lead's coordinator last heard it (coord::Capacity): memory held of the
// build's (scenic memory, within the guard's limit), cores busy, the disk free and what's left of it
// for jobs (diskWords).
function capacityBlock(c) {
  if (!c) return null;
  const rows = [];
  const build = c.build_mb || c.limit_mb;
  const limit = c.build_mb && c.build_mb < c.limit_mb ? ` (limit ${gb(c.limit_mb)})` : "";
  rows.push(capBar("memory", c.held_mb, build, `${gb(c.held_mb)} of the build's ${gb(build)}${limit} · ${gb(c.mem_mb)} free${c.guard ? "" : " · unguarded"}`, 0, build && c.held_mb / build > 0.9 ? "warn" : ""));
  const busy = Math.max(0, c.cores - c.cores_free);
  rows.push(capBar("cores", busy, c.cores, `${busy} of ${c.cores} busy${c.in_use && c.kept_cores ? ` · ${c.kept_cores} kept for its owner` : ""}`));
  const disk = c.disk_mb + c.reserved_mb;
  rows.push(capBar("disk", c.reserved_mb, disk, c.free_mb ? diskWords(c) : `${gb(c.reserved_mb)} reserved · ${gb(c.disk_mb)} free`, 0, c.disk_mb < 10240 ? "warn" : ""));
  return h("div", { class: "caps", title: "As the lead's coordinator last heard it: memory its jobs hold of what the build gets on this Mac (scenic memory, within the memory guard's limit; and free for more, less what they're predicted to need); cores busy by the last minute's load; the disk's free space, and what's left of it for jobs past the owner's room target and its jobs' reservations" }, rows);
}

// A Mac's disk in words (coord::disk_words, task #391): "38 GB free · 18 GB for jobs (20 GB room
// target)"; nothing kept, no parentheses. (No fixed reserve, task #421: an older agent's
// `reserve_mb` is passed over, its `disk_mb` having it taken off already.)
function diskWords(c) {
  const g = (mb) => `${Math.round(mb / 1024)} GB`;
  const kept = [];
  if (c.target_mb) kept.push(`${g(c.target_mb)} room target`);
  if (c.reserved_mb) kept.push(`${g(c.reserved_mb)} reserved by its jobs`);
  return `${g(c.free_mb)} free · ${g(c.disk_mb)} for jobs${kept.length ? ` (${kept.join(", ")})` : ""}`;
}

// A machine's card, each the same: who and how it is; its capacity; a block per running job, by
// slot, with what's inside it (or why none runs); live on it (live.js).
function machineCard(m, x) {
  const st = x.status, now = m.now;
  const [state, cls] = machineState(st, x.fresh, m.sw.pause);
  const jobs = jobsOf(st);
  const role = x.role === "lead" ? "lead · coordinator" : "member";
  const card = h("div", { class: `mc ${x.fresh ? "" : "off"}`, style: { borderLeft: `3px solid ${x.colour}` } },
    h("div", "top", h("span", "name", x.name), h("span", "role", role), chip(state, cls), jobs.length ? chip(plural(jobs.length, "job")) : null, h("span", "right", `heard from ${ago(now, st.beat)}`)));
  card.append(...[capacityBlock(capOf(m.a, x.name))].filter(Boolean));
  jobs.forEach((j) => {
    // (Its real slot, slots may be sparse: jobsOf.)
    const b = jobBlock(j, now, m.fc);
    b.querySelector(".what").prepend(h("span", "slot", `${j.slot + 1}`));
    card.append(b);
  });
  if (!jobs.length) {
    // Why it isn't building.
    const why = (st.waiting || []).filter((w) => !w.step || x.role !== "lead").slice(0, 2);
    card.append(h("div", "job", h("div", "what dim", st.held ? `This Mac is paused (from ${st.held.by}); the rest of the pool goes on` : m.sw.pause ? "Paused with the build" : why.length ? "Waiting" : "Nothing to build now")));
    for (const w of why) card.append(h("div", "sub", x.role === "lead" ? `${w.what}: ${w.why}` : w.why));
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
      h("div", "sub", leases.length ? leases.map((l) => `${l.progress || S.title(l.step)}${l.frac != null ? ` (${Math.round(l.frac * 100)}%)` : ""}`).join(" · ") : p.what));
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
const pin = { key: null, el: null, also: "", veil: null };
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
// [{w, colour, cls, title}], label}), chips (`chips` under `chipsLabel`) and a foot (`foot`).
const cardBody = (title, pairs, ...more) => {
  const o = title && typeof title === "object" && !(title instanceof Node) && !Array.isArray(title) ? title : { title };
  const stats = o.stats?.filter(([v]) => v != null);
  const head = o.kicker != null || o.colour
    ? h("div", "tc-head", o.colour ? h("i", { class: "tc-sw", style: { background: o.colour } }) : null, h("div", null, o.kicker ? h("div", "tc-kick", o.kicker) : null, h("div", "tc-title", o.title)))
    : h("div", "ct", o.title);
  const bar = o.bar && h("div", "tc-bar", h("div", "track", h("i", { style: { width: `${Math.round(Math.max(0, Math.min(1, o.bar.frac)) * 100)}%`, background: o.bar.colour || o.colour || "var(--accent)" } })), o.bar.label ? h("div", "cap", o.bar.label) : null);
  const segs = o.segs?.parts?.length && h("div", "tc-bar", h("div", "track segs", o.segs.parts.map((p) => h("i", { class: p.cls || null, title: p.title || null, style: { width: `${(p.w * 100).toFixed(2)}%`, background: p.colour } }))), o.segs.label ? h("div", "cap", o.segs.label) : null);
  return [head,
    stats?.length ? h("div", "cstats", stats.map(([v, l]) => h("div", "cstat", h("div", "n", v), h("div", "l", l)))) : null,
    bar, segs,
    h("div", "ckv", pairs.filter(([, v]) => v != null && v !== "").flatMap(([k, v]) => [h("span", "k", k), h("span", null, v)])),
    o.chips?.length ? h("div", "tc-chipsbox", o.chipsLabel ? h("div", "tc-kick", o.chipsLabel) : null, h("div", "tc-chips", o.chips.map((c) => h("span", null, c)))) : null,
    ...more, o.foot ? h("div", { class: "tc-foot", title: o.foot }, o.foot) : null].filter(Boolean);
};
// (Beside the point, right and below, else left or above where there's no room: never over the
// pointer, as long as there's room on either side.)
function placeCard(el, x, y) {
  const r = el.getBoundingClientRect(), W = window.innerWidth, H = window.innerHeight;
  const left = x + 12 + r.width <= W - 8 ? x + 12 : x - 12 - r.width >= 8 ? x - 12 - r.width : Math.max(8, W - r.width - 8);
  const top = y + 14 + r.height <= H - 8 ? y + 14 : y - 10 - r.height >= 8 ? y - 10 - r.height : Math.max(8, H - r.height - 8);
  el.style.left = `${left}px`;
  el.style.top = `${top}px`;
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
  pin.el = h("div", { class: "card-tip pinned", role: "dialog", "aria-modal": "true" }, close, h("div", "cb", cards.get(key)(also)));
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
  pin.key = pin.el = pin.veil = null;
}
// (After a render: the pinned card's words again, from what's shown now; kept as they were, said
// to be over, when it's gone.)
function refreshPin() {
  if (!pin.el) return;
  const build = cards.get(pin.key);
  const body = pin.el.querySelector(".cb");
  if (build) body.replaceChildren(...build(pin.also));
  else if (!pin.el.classList.contains("gone")) { pin.el.classList.add("gone"); body.append(h("div", "dim small", "No longer shown (the schedule moved on).")); }
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
// The past jobs the schedule may show: how far back (this browser's own, kept).
const PAST = [[0, "None"], [3600, "Past hour"], [2 * 3600, "Past 2 hours"], [6 * 3600, "Past 6 hours"], [86400, "Past 24 hours"], [2 * 86400, "Past 48 hours"], [3 * 86400, "Past 72 hours"], [7 * 86400, "Past 7 days"]];
// The jobs' ends from the history further back than the page's own events (`/work/history`, a
// chunk at a time back to the past chosen: sums.js pastFrom), in the numbering they were read in.
const past = { ends: [], epoch: undefined, first: null, asking: false };
// Every job's end held (a lead's job's "end", a worker's "done" or "fail"): those read back, then
// the page's own events.
const isEnd = (e) => e.kind === "end" || e.kind === "done" || e.kind === "fail";
const endsHeld = () => past.ends.concat(events.filter(isEnd));
async function loadPast(sw) {
  if (past.epoch !== sw.epoch) Object.assign(past, { ends: [], epoch: sw.epoch, first: null });
  if (!ui.past || past.asking || !events.length) return;
  past.asking = true;
  try {
    const from = sw.now - ui.past;
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

// What a job running now is, as a schedule's run: on its slot's lane, from its start to now.
const runningRuns = (m) => m.macs.flatMap((x) => jobsOf(x.status).map((j) => ({ lane: slotName(x.name, j.slot), step: stepOf(j.id), from: j.started, until: m.now, n: jobTarget(j) ? 1 : 0, what: j.what, job: j, mac: x.name })));

// A run's card: what, where, when, how long, how it went; its targets (a past job's); the step's
// phases once the lead measures them (phaseRows).
function runCard(m, r, kind, laneName) {
  const several = r.parts?.length > 1;
  const said = kind === "running" ? r.what : several ? `${S.said(r.step, r.n)} (${r.parts.length} jobs)` : r.what || S.said(r.step, r.n);
  const state = kind === "running" ? (r.job.paused ? "running, frozen" : "running") : kind === "past" ? (r.ok ? "finished" : several ? "finished, some failed" : "failed") : "to come (forecast)";
  const p = r.job?.progress;
  return cardBody([swatch(S.colour(r.step)), said], [
    ["step", h("span", "mono", r.step)],
    ["on", laneName(r.lane)],
    ["when", `${clock(r.from)}–${kind === "running" ? "now" : clock(r.until)}`],
    ["took", kind === "next" ? `≈ ${dur(r.until - r.from)}` : dur(r.until - r.from)],
    ["state", h("span", kind === "past" && !r.ok ? "bad" : kind === "running" ? "run" : null, state)],
    ["progress", p?.total > 0 ? progressText(p, r.job, m.now) : null],
    ["targets", kind === "past" && !several && r.targets.length ? `${r.targets.slice(0, 8).join(", ")}${r.targets.length > 8 ? ` and ${r.targets.length - 8} more` : ""}` : null],
  ], ...phaseRows(m, r.step));
}

// The step's phases, each a share of a job's time: none yet. (Seam for the lead's measured phase
// shares, task #511, once status.json carries them: the rows go here and in stepCard, drawn from
// what the lead publishes, never typed in.)
const phaseRows = () => [];

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
// it ran (the history's ends, as far back as the past chosen), those running now (to the now line)
// and the forecast's to come, each a segment in its stage's colour, packed where a pixel holds
// several (sums.js packRuns); a now line; each region's mark where it's all on the map; a dense
// time axis.
function schedule(m) {
  const fc = m.fc, now = m.now;
  const fcLanes = fc?.lanes || {};
  const t0 = now - ui.past;
  const t1 = Math.max(fc?.done_at || 0, ...Object.values(fcLanes).flat().map((l) => l.until), now + 3600);
  const span = t1 - t0;
  const x = (t) => `${(((Math.max(t0, Math.min(t1, t)) - t0) / span) * 100).toFixed(3)}%`;
  const gap = span / 1200;
  const pastAll = ui.past ? pastRuns(endsHeld(), t0, m.macs.map((mm) => mm.name)) : [];
  const running = runningRuns(m);
  const short = shortNames(m);
  const hosts = m.macs.map((mm) => mm.name);
  const names = [...new Set([...busyLanes(Object.keys(fcLanes), fcLanes, now, t1), ...pastAll.map((r) => r.lane), ...running.map((r) => r.lane)])];
  const groups = laneGroups(names, hosts);
  const laneName = (nm) => {
    const host = hosts.find((hh) => isSlotOf(nm, hh));
    return host ? `${short[host]} · slot ${slotNo(nm, host)}` : nm;
  };
  const stagesSeen = new Set();
  const lanes = h("div", "lanes");
  const segment = (track, r, kind, key) => {
    const seg = withCard(h("i", { class: `${kind}${kind === "past" && !r.ok ? " failed" : ""}` }), key, () => runCard(m, r, kind, laneName));
    Object.assign(seg.style, { left: x(r.from), width: `max(2px, calc(${x(r.until)} - ${x(r.from)}))`, background: S.colour(r.step) });
    track.append(seg);
    if (S.stage(r.step)) stagesSeen.add(S.stage(r.step));
  };
  for (const g of groups) {
    const mac = m.macs.find((mm) => mm.name === g.host);
    lanes.append(h("div", { class: "gh", title: g.host }, mac ? short[g.host] : g.host, mac ? null : h("span", "dim", " (gone)")), h("div", "ghr", ...(mac ? laneChips(mac) : [])));
    for (const nm of g.lanes) {
      const track = h("div", "track");
      for (const r of packRuns(pastAll.filter((r) => r.lane === nm), gap)) segment(track, r, "past", `p ${nm} ${r.seq}`);
      for (const r of running.filter((r) => r.lane === nm)) segment(track, r, "running", `r ${nm} ${r.job.id}`);
      for (const l of fcLanes[nm] || []) if (l.until > now && l.from < t1) segment(track, { ...l, from: Math.max(l.from, now), lane: nm }, "next", `f ${nm} ${l.from} ${l.step}`);
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
    const track = h("div", "track");
    for (const t of helps) for (const l of own.filter((l) => l.step === t.of && l.until > now)) segment(track, { ...l, from: Math.max(l.from, now), step: t.id, lane: who, what: `${t.title} as the lead runs ${S.said(t.of, l.n)}` }, "next", `g ${t.id} ${l.from}`);
    lanes.append(h("div", "gh", "Pages"), h("div", "ghr"), h("div", { class: "ln", title: m.pages.map((p) => p.label).join(", ") }, who), track);
  }
  // Each region all on the map as it's built now: a mark at its `map_at` (several at one time, one).
  const outs = {};
  for (const r of fc?.regions || []) if (r.map_at && r.map_at >= now && r.map_at <= t1) (outs[r.map_at] ||= []).push(m.regionName[r.id] || r.id);
  if (Object.keys(outs).length) {
    const track = h("div", "track outs");
    for (const [at, nms] of Object.entries(outs)) {
      track.append(withCard(h("b", { style: { left: x(+at) } }), `o ${at}`, () => cardBody(`All on the map ≈ ${clock(+at)}`, [["regions", `${nms.slice(0, 12).join(", ")}${nms.length > 12 ? ` and ${nms.length - 12} more` : ""}`]])));
    }
    lanes.append(h("div", "ln", "On the map"), track);
  }
  lanes.append(h("div"), timeAxis(t0, t1, now, x));
  // The legend: the stages seen, in the build's order, each once (by its title); what a segment's
  // look says; the regions' mark.
  const order = table.stages.map((g) => g.id);
  const titles = new Set();
  const stages = [...stagesSeen].sort((p, q) => order.indexOf(p) - order.indexOf(q)).filter((g) => !titles.has(S.stageTitle(g)) && titles.add(S.stageTitle(g)));
  const legend = h("div", "legend", stages.map((g) => h("span", null, h("i", { style: { background: stageColour(g) } }), S.stageTitle(g))),
    h("span", "keys", h("i", "k-past"), "ran", h("i", "k-failed"), "failed", h("i", "k-run"), "running", h("i", "k-next"), "to come"),
    Object.keys(outs).length ? h("span", null, h("i", { class: "rmark" }), "a region all on the map") : null);
  // (A forecast cut at its time budget (forecast::BUDGET): the lanes end where its run stopped.)
  const cut = fc?.partial && h("div", "why", "Partial: the forecast was cut at its time budget; each lane ends where its run stopped, the work after it not placed");
  const reaching = ui.past && past.first && past.first.t > t0 && past.asking ? h("span", "dim", " · reading further back…") : null;
  const pick = h("select", { class: "past", title: "How far back the schedule shows the jobs that ran (the history keeps 7 days)", onchange: (e) => { e.target.blur(); ui.past = +e.target.value; ctx.store.set("past", ui.past); render(); loadPast(last); } },
    PAST.map(([s, t]) => h("option", { value: s, selected: s === ui.past }, t)));
  const top = h("div", "schedtop", h("label", "small dim", "Show past jobs ", pick), reaching, h("span", "small dim", fc?.done_at ? `done ≈ ${clock(fc.done_at)}` : fc ? "" : "no forecast yet (it comes with the lead's next plan)"));
  if (!groups.length && !helps.length) return h("div", "sched", top, h("div", "small dim", "Nothing ran or is planned in this window."));
  const cap = h("div", "nowcap", h("span", null, "now"));
  cap.style.setProperty("--f", ((Math.max(t0, Math.min(t1, now)) - t0) / span).toFixed(5));
  return h("div", "sched", top, cut || null, h("div", "lanewrap", lanes, cap), legend);
}

// A dense time axis under the lanes: a tick at each round time (chartmath.js timeTicks, by local
// time, midnight a day's name), smaller ones between, none crowding the now line (its cap says "now").
function timeAxis(t0, t1, now, x) {
  const most = Math.max(4, Math.round((Math.min(window.innerWidth, 1400) - 160) / 70));
  const major = timeTicks(t0, t1, most);
  const axis = h("div", "axis");
  for (const t of timeTicks(t0, t1, most * 4)) axis.append(h("b", { style: { left: x(t.t) } }));
  for (const t of major) {
    const d = new Date(t.t * 1000);
    const label = t.label === "00:00" ? DAYS[d.getDay()] : t.label;
    if (Math.abs(t.t - now) > (t1 - t0) / most / 2) axis.append(h("span", { class: label === t.label ? "" : "day", style: { left: x(t.t) } }, label));
  }
  return axis;
}

// ---- Jobs: the stages ---------------------------------------------------------------------------------
// A list, a stage under another (the checklist's lines, build::Line): its mark, title and counts, its
// bar; under it a row per step (build::Item): its mark, id, title (⇄ when any Mac may take it),
// count, and a cell per job: those finished (the history held: the past chosen, at least the
// page's own events), running and to come (the forecast's), each with its card.
const CELLS_DONE = 24, CELLS_NEXT = 40;
function stagesList(m) {
  if (!m.lines.length) return h("div", "small dim", "No word from the lead yet.");
  const ended = pastRuns(endsHeld(), 0, m.macs.map((mm) => mm.name));
  const since = ended.length ? ended[0].until : null;
  const running = runningRuns(m);
  const fcLanes = m.fc?.lanes || {};
  const toCome = Object.entries(fcLanes).flatMap(([lane, ls]) => ls.filter((l) => l.until > m.now && l.from >= m.now - 1).map((l) => ({ ...l, lane }))).sort((p, q) => p.from - q.from);
  const short = shortNames(m);
  const laneName = (nm) => {
    const host = m.macs.find((mm) => isSlotOf(nm, mm.name))?.name;
    return host ? `${short[host]} · slot ${slotNo(nm, host)}` : nm;
  };
  const list = h("div", "stages");
  for (const l of m.lines) {
    const done = finished(l), run = (l.steps || []).some((s) => m.running.has(s));
    const colour = S.colour(l.steps?.[0]);
    const frac = l.total ? l.done / l.total : done ? 1 : 0;
    list.append(withCard(h("div", { class: `sh ${done ? "done" : ""} ${run ? "now" : ""}`, style: { borderLeftColor: colour } },
      mark(done, run), h("span", "title", l.what), h("span", "n", countsOf(l)), pbar(frac, done ? "ok" : run ? "" : "dimbar", done ? null : colour)), `s ${l.id || l.what}`, () => stageCard(m, l)));
    for (const it of l.items || []) {
      const step = it.step, itDone = finished(it), itRun = m.running.has(step);
      const mine = (xs) => xs.filter((r) => r.step === step);
      const fin = mine(ended), now = mine(running), next = mine(toCome);
      const cells = h("div", "cells");
      if (fin.length > CELLS_DONE) cells.append(h("span", "more", `+${n(fin.length - CELLS_DONE)}`));
      for (const r of fin.slice(-CELLS_DONE)) cells.append(withCard(h("i", { class: r.ok ? "c-done" : "c-failed", style: { background: r.ok ? S.colour(step) : null } }), `c ${r.seq}`, () => runCard(m, r, "past", laneName)));
      for (const r of now) cells.append(withCard(h("i", { class: "c-run", style: { borderColor: S.colour(step) } }), `cr ${r.job.id} ${r.lane}`, () => runCard(m, r, "running", laneName)));
      for (const r of next.slice(0, CELLS_NEXT)) cells.append(withCard(h("i", { class: "c-next", style: { borderColor: S.colour(step) } }), `cn ${r.lane} ${r.from} ${step}`, () => runCard(m, { ...r, from: Math.max(r.from, m.now) }, "next", laneName)));
      if (next.length > CELLS_NEXT) cells.append(h("span", "more", `+${n(next.length - CELLS_NEXT)}`));
      list.append(h("div", { class: `st ${itDone ? "done" : ""}` },
        mark(itDone, itRun), h("span", "mono id", step),
        withCard(h("span", "title", S.title(step), S.any(step) ? h("span", { class: "dim" }, " ⇄") : null), `t ${step}`, () => stepCard(m, it, { fin, now, next, since })),
        h("span", "n", countOf(it, `${S.noun(step)}s`)), cells));
    }
  }
  return list;
}

// A stage's card: its counts, what it does, its note and its next jobs.
function stageCard(m, l) {
  const g = table.stages.find((x) => x.id === S.stage(l.steps?.[0]));
  return cardBody([swatch(S.colour(l.steps?.[0])), l.what], [
    ["counts", countsOf(l)],
    ["does", g?.does],
    ["note", l.note],
    [finished(l) ? null : (l.steps || []).some((s) => m.running.has(s)) ? "now" : "next", l.next?.length ? `${l.next.slice(0, 3).join("; then ")}${l.next.length > 3 ? `; and ${l.next.length - 3} more` : ""}` : null],
  ]);
}

// A step's card: what it does, where it runs, its count, its jobs (finished in the history held,
// running, to come), its forecast (work left, done ≈, time a target); its phases (phaseRows).
function stepCard(m, it, { fin, now, next, since }) {
  const row = S.row(it.step);
  const f = m.fc?.steps.find((s) => s.step === it.step);
  const each = m.fc?.each_s?.[it.step];
  const failed = fin.filter((r) => !r.ok).length;
  return cardBody([swatch(S.colour(it.step)), S.title(it.step)], [
    ["step", h("span", "mono", it.step)],
    ["does", row?.does],
    ["runs on", row ? (row.place === "any" ? "any Mac" : "the lead") : null],
    ["count", countOf(it, `${S.noun(it.step)}s`)],
    ["jobs", `${n(fin.length)} finished${failed ? ` (${failed} failed)` : ""}${since ? ` since ${clock(since)}` : ""} · ${n(now.length)} running · ${n(next.length)} to come`],
    ["work left", f?.work_s ? dur(f.work_s) : null],
    ["done ≈", f?.done_at && !finished(it) ? clock(f.done_at) : null],
    ["a target", each ? `≈ ${dur(each)}` : null],
  ], ...phaseRows(m, it.step));
}

// ---- Regions --------------------------------------------------------------------------------------
// The Regions tab: the world map over the regions' list, split by a bar to drag. The list
// (virtual: only the rows in view are made) orders them as they reach the map, by name or by
// work left, found by any folder's name; each row its folders, state, when it's done and on the
// map, its tiles built and a bar of its work left by stage (its targets left at each step's
// measured seconds a target, the longest region's full width); under the pointer, its region lit
// on the map and its card. Kept across the page's renders (its scroll, search and split with it).
const ui = { order: "map", filter: "", feed: "all", past: 6 * 3600 };
const ROW_H = 40;
const rp = { section: null, el: null, map: null, list: null, rowsEl: null, rows: [], shown: [], maxS: 0, win: null, lit: null, split: 0.65 };

// A download's size in words: "640 MB", "9.0 GB".
const size = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);

// Each region as the tab shows it: the forecast's (its `left` by step, rank, times) with its
// folders, size and outline (/work/regions), its tiles built (the lead's `built`), its recipe
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
      segs: stageSegments(work, S.stage, stages), secs: work.reduce((t, w) => t + w.s, 0), count: work.reduce((t, w) => t + w.k, 0) };
  });
}

// A region's state in words and its chip's class: on the map, done (with the next catalogue),
// building now, its place in line; as the map had it before, said.
function rowState(r) {
  if (!r.f) return ["", ""];
  let [st, cls] = !r.count ? (r.on_map === true ? ["on the map", "ok"] : ["done; on the map with the next catalogue", "ok"]) : r.rank === 0 ? ["building now", "run"] : [`#${r.rank + 1} in line`, ""];
  if (r.on_map === false && r.count) st += " · on the map as it was";
  return [st, cls];
}

// Each region's card (the list's rows and both maps' regions), as the mockup draws it: its
// folders over its name, its download and tiles built (stat tiles), its tiles built (a bar), each
// stage of the build (a bar in parts: built solid, the one under way striped, to come faint),
// state, when it's built and on the map, its place in the build's order, its work left by step
// (chips), its outline's recipe. (The other regions at a map's point have their own cards in its
// stack.)
const REGION_COLOUR = { on: "var(--ok)", done: "var(--ok)", building: "var(--accent)", queued: "var(--dim)" };
function regionCards(rows) {
  const ranked = rows.filter((r) => r.f).length;
  for (const r of rows) cards.set(`mr ${r.id}`, () => {
    const [st, cls] = rowState(r);
    const b = r.built;
    const left = new Set(r.segs.map((g) => g.stage));
    const now = r.rank === 0 ? r.segs[0]?.stage : null;
    const stages = r.f ? table.stages.map((g) => g.id) : [];
    const parts = stages.map((g) => ({ w: 1 / stages.length, colour: stageColour(g), cls: g === now ? "run" : left.has(g) ? "next" : null, title: `${S.stageTitle(g)}: ${g === now ? "building" : left.has(g) ? "to come" : "built"}` }));
    const builtN = stages.filter((g) => !left.has(g)).length;
    return cardBody({ kicker: r.crumbs?.slice(0, -1).join(" › ") || "Region", colour: REGION_COLOUR[regionState(r.f)] || "var(--dim)", title: r.name,
      stats: [[r.bytes != null ? size(r.bytes) : null, "to download"], [b?.total ? `${n(b.built)} / ${n(b.total)}` : null, "tiles built"]],
      bar: b?.total ? { frac: b.built / b.total, label: `${Math.round((b.built / b.total) * 100)}% of its tiles built`, colour: "var(--ok)" } : null,
      segs: parts.length ? { parts, label: `stages: ${builtN} built${now ? `, ${S.stageTitle(now)} building` : ""}, ${left.size - (now ? 1 : 0)} to come` } : null,
      chips: r.count ? r.work.map((w) => `${S.said(w.step, w.k)}${w.s ? ` · ${dur(w.s)}` : ""}`) : [], chipsLabel: "Work left",
      foot: r.outline?.length ? `outline: ${r.outline.join(" + ")}` : "" }, [
      ["state", st ? h("span", cls, st) : null],
      ["on the map", r.f ? (r.on_map === true && !r.count ? "yes" : r.map_at ? `all ≈ ${clock(r.map_at)}` : r.on_map === false ? "as it was" : "not yet") : null],
      ["all built", r.count && r.ready_at ? `≈ ${clock(r.ready_at)}` : null],
      ["build order", r.rank != null ? `${n(r.rank + 1)} of ${n(ranked)}` : null],
    ]);
  });
}

// One row: its folders and name, state chip, when; its stage bar, tiles and work left, its work
// in time and download.
function regionRow(r, i) {
  const [st, cls] = rowState(r);
  const above = r.crumbs?.slice(0, -1).join(" › ");
  const when = r.count ? `done ≈ ${clock(r.ready_at)}${r.map_at ? ` · on the map ≈ ${clock(r.map_at)}` : ""}` : "";
  const bar = h("span", "sbar");
  for (const g of r.segs) bar.append(h("i", { style: { width: `${Math.max(rp.maxS ? (g.s / rp.maxS) * 100 : 0, 2)}%`, background: stageColour(g.stage) } }));
  const b = r.built;
  const tiles = b?.total ? (b.built < b.total ? `${n(b.built)} of ${n(b.total)} tiles` : `${n(b.total)} tiles built`) : "";
  const left = r.work.map((w) => S.said(w.step, w.k)).join(" · ");
  const row = h("div", { class: `rrow${r.f && !r.count && r.on_map === true ? " done" : ""}${rp.lit === r.id ? " lit" : ""}`, role: "listitem", style: { top: `${i * ROW_H}px` } },
    h("span", "nm", above ? h("span", "dim", `${above} › `) : null, r.name, st ? [" ", chip(st, cls)] : null),
    h("span", "when", when),
    h("span", "lw", bar, h("span", "lt", [tiles, left ? `left: ${left}` : ""].filter(Boolean).join(" · "))),
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
  rp.maxS = Math.max(0, ...rp.rows.map((r) => r.secs));
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
  rp.map.highlight(id);
  for (const el of rp.rowsEl.children) el.classList.toggle("lit", el.dataset.id === id);
}

// The split: the map's share of the height, dragged (pointer), stepped (arrow keys) or reset
// (double click); kept in this browser.
const SPLIT = [0.25, 0.85];
function setSplit(f, keep) {
  rp.split = Math.max(SPLIT[0], Math.min(SPLIT[1], f));
  rp.el.style.setProperty("--split", `${(rp.split * 100).toFixed(1)}%`);
  if (keep) ctx.store.set("regionsMapSplit", rp.split);
}
function splitter() {
  const bar = h("div", { class: "rsplit", role: "separator", "aria-orientation": "horizontal", "aria-label": "The map's share of the height", tabindex: "0", title: "Drag to share the height · double-click to reset" });
  let drag = false;
  bar.addEventListener("pointerdown", (e) => { drag = true; bar.setPointerCapture(e.pointerId); bar.classList.add("on"); e.preventDefault(); });
  bar.addEventListener("pointermove", (e) => { if (!drag) return; const r = rp.el.getBoundingClientRect(); setSplit((e.clientY - r.top) / r.height, false); });
  const end = () => { if (!drag) return; drag = false; bar.classList.remove("on"); setSplit(rp.split, true); };
  bar.addEventListener("pointerup", end);
  bar.addEventListener("pointercancel", end);
  bar.addEventListener("dblclick", () => setSplit(0.65, true));
  bar.addEventListener("keydown", (e) => { const d = { ArrowUp: -0.02, ArrowDown: 0.02 }[e.key]; if (d) { e.preventDefault(); setSplit(rp.split + d, true); } });
  return bar;
}

function regionsPane(m, md) {
  if (!rp.el) {
    rp.map = createMap({ pick: mapPick });
    const find = h("input", { type: "search", placeholder: "Find a region or a folder…", value: ui.filter, autocapitalize: "off", spellcheck: "false", "aria-label": "Find a region" });
    find.addEventListener("input", () => { ui.filter = find.value; rp.list.scrollTop = 0; relist(); });
    rp.seg = h("div", "seg", [["map", "To the map"], ["name", "A–Z"], ["left", "Most work left"]].map(([k, t]) => {
      const b = h("button", { onclick: () => { ui.order = k; rp.list.scrollTop = 0; relist(); } }, t);
      b.dataset.order = k;
      return b;
    }));
    rp.count = h("span", "small dim");
    rp.totals = h("div", "rtotals small dim");
    rp.stages = h("div", "rstages small dim");
    rp.rowsEl = h("div", "rrows");
    rp.list = h("div", { class: "rlist", role: "list", "aria-label": "Regions" }, rp.rowsEl);
    let ticking = false;
    const later = () => { if (!ticking) { ticking = true; requestAnimationFrame(() => { ticking = false; if (rp.shown.length) paintRows(false); }); } };
    rp.list.addEventListener("scroll", later, { passive: true });
    // (Its height changed (the window, the tab shown again): the rows that now fit.)
    new ResizeObserver(later).observe(rp.list);
    rp.list.addEventListener("pointerover", (e) => { if (e.pointerType !== "touch") light(e.target.closest?.(".rrow")?.dataset.id || null); });
    rp.list.addEventListener("pointerleave", () => light(null));
    rp.legend = h("div");
    rp.el = h("div", "rpane",
      h("div", "rmapp", rp.map.el, rp.legend),
      splitter(),
      h("div", "rlistp", h("div", "regbar", find, rp.seg, rp.count), rp.totals, rp.list, rp.stages));
    setSplit(Number(ctx.store.get("regionsMapSplit", 0.65)) || 0.65, false);
    rp.section = section("d-regions", null, null, h("div", "box", rp.el));
  }
  rp.rows = md.rows;
  rp.map.update(md.data);
  rp.legend.replaceChildren(...mapLegend(m, md, rp.map));
  // What's left in all of them, a step at a time (the forecast's regions summed in the page).
  const order = Object.keys(table.steps);
  const words = Object.entries(stepTotals(m.fc?.regions)).sort(([p], [q]) => order.indexOf(p) - order.indexOf(q)).map(([s, k]) => S.said(s, k));
  const secs = rp.rows.reduce((t, r) => t + r.secs, 0);
  rp.totals.replaceChildren(!m.fc ? "No forecast yet (the lead makes one with each plan): the regions alone."
    : words.length ? `Left in ${plural(rp.rows.filter((r) => r.count).length, "region")}${secs ? ` (≈ ${dur(secs)} of work)` : ""}: ${words.join(" · ")}` : "Nothing left to build in any region.");
  const seen = [...new Set(rp.rows.flatMap((r) => r.segs.map((g) => g.stage)))];
  rp.stages.replaceChildren(...(seen.length ? [h("span", null, "Bars: work left by stage, the most a region has full width"), ...seen.map((g) => h("span", null, swatch(stageColour(g)), S.stageTitle(g)))] : []));
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
    h("div", "seg", [["tiles", `${S.title("unit")}: tiles an hour`], ["busy", "Busy minutes"]].map(([k, t]) => h("button", { class: (ui.metric || "tiles") === k ? "on" : "", onclick: () => { ui.metric = k; render(); } }, t))),
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
  const what = e.step ? (t.length === 1 && !S.world(e.step) ? `${S.title(e.step)}: ${t[0]}` : S.said(e.step, t.length)) : "";
  switch (e.kind) {
    case "start": return [`${who} began ${e.what || e.note || what}`, ""];
    case "end": {
      const job = e.what || what;
      return e.ok ? [`${who} finished ${job} in ${dur(e.secs || 0)}`, ""] : [`${who}'s ${job} ${e.note || "stopped"}${t.length ? ` (${S.counted(e.step, t.length)} done and kept)` : ""}`, /fail/.test(e.note) ? "bad" : "warn"];
    }
    case "lease": return [`${who} took ${what}`, "dim"];
    case "done": return [`${who} handed back ${what}, ${dur(e.secs || 0)} after taking it`, ""];
    case "fail": return [`${who}'s ${what} ${/^stopped/.test(e.note) ? e.note : `failed: ${e.note}`}`, /^stopped/.test(e.note) ? "warn" : "bad"];
    case "lapse": return [`${who} went quiet: its lease on ${e.note || what} lapsed and the work is offered again`, "warn"];
    case "task": return [k > 1 ? `${who} did ${plural(k, "task")}` : `${who} did a task${t[0] ? ` (${t[0]})` : ""} in ${dur(e.secs || 0)}`, "dim"];
    // (Given back, not failed: the page reloaded or closed, or emptied in the background.)
    case "task-fail": return /^stopped:/.test(e.note || "") ? [`${who}: a task given back (${e.note.replace(/^stopped:\s*/, "")})`, "warn"] : [`${who}: a task failed: ${e.note}`, "bad"];
    case "catalog": return [t.length ? `The map got ${t.length > 3 ? `${t.length} regions` : t.map(nm).join(", ")}` : "The map was updated", "ok"];
    case "pause": return [`The build was paused by ${e.note}`, "warn"];
    case "resume": return ["The build went on", "ok"];
    case "worker": return [`${who}${e.note && e.note !== who ? ` (${e.note})` : ""} joined`, "run"];
    case "agent": return [`${who}'s agent started (${e.note})`, "run"];
    case "conditions": return [`${who}: ${e.note}`, /doesn't answer|battery|away/.test(e.note) ? "warn" : ""];
    // (A Mac's build caches cleared on its owner's ask (older history: trimmed after the build): what was freed.)
    case "caches": return [`${who} ${e.note}`, /^didn't/.test(e.note || "") ? "warn" : ""];
    case "term": return termText(e);
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
  const leases = sw.leases || [];
  box.append(h("h3", null, "Leases", h("span", null, "who holds what; a worker that stops beating gives its work back")),
    leases.length ? tbl(["Worker", "Work", "For", "Last beat", "Lapses in", "Says"], leases.map((l) => [l.worker, S.said(l.step, l.n), dur(l.for_s), l.beat_s != null ? `${dur(l.beat_s)} ago` : "–", l.lapses_in_s != null ? dur(l.lapses_in_s) : "–", l.progress || ""])) : h("div", "small dim", "None."));
  const waits = a.waiting || [];
  box.append(h("h3", null, "Waiting", h("span", null, "what can't run yet, and why")), waits.length ? tbl(["What", "Why"], waits.map((w) => [w.what, w.why])) : h("div", "small dim", "Nothing."));
  // The timetables' to-do list and the routes matching nothing on the map (docs/inputs.md §6.7.3).
  const tt = a.timetables;
  if (tt) {
    box.append(h("h3", null, "Timetables", h("span", null, `${n(tt.to_do)} lines with no timetable (${n(tt.rail)} rail, ${n(tt.ferry)} ferry, ${n(Math.round(tt.to_do_km))} km): ${tt.file}`)),
      tt.unmatched_total ? tbl(["Mode", "Timetable", "Route", "OSM pin", "Matches nothing because"], tt.unmatched.map((u) => [u.mode, u.source, u.name ? `${u.route} (${u.name})` : u.route, u.osm || "", u.why])) : h("div", "small dim", "Every timetable route matches a line on the map."));
    if (tt.unmatched_total > tt.unmatched.length) box.append(h("div", "small dim", `… and ${n(tt.unmatched_total - tt.unmatched.length)} more (work/timetables/report)`));
  }
  const recent = a.recent || [];
  box.append(h("h3", null, "The lead's last jobs"), recent.length ? tbl(["", "Job", "Took", "Ended", "Note"], recent.slice(0, 20).map((d) => [d.ok ? "✓" : "✗", d.what, dur(d.secs), clock(d.ended), (d.note || "").split("\n").slice(-2).join(" ").slice(0, 160)])) : h("div", "small dim", "None yet."));
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
  if (rows.length) box.append(h("h3", null, "The steps", h("span", null, "every step's row of the table: what it makes, of what, where it runs; time by phase: each phase's mean share of a run's time over its last good runs, and of the bytes read and written (I/O: local disk, NAS and internet alike)")), tbl(["Stage", "Step", "Does", "Kind", "Each target", "Runs on", "A job", "Time by phase"], rows.map((s) => [S.stageTitle(s.stage), s.title, s.does, s.kind, s.of ? `a task of ${S.title(s.of)}` : s.scope === "world" ? "the world" : `${/^[aeiou]/.test(s.noun) ? "an" : "a"} ${s.noun}`, s.place === "any" ? "any Mac" : "the lead", s.of ? "" : s.place === "any" ? "a quarter hour's" : "all", phaseShares(a.phase_shares?.[s.id])])));
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
const drawn = { byId: {}, catalog: undefined, at: 0, asking: false };
const REGIONS_EVERY_MS = 600000;
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

// The header's status: the verdict's sentence, the pause controls, the alerts (each a link to the
// tab that says more).
function head(m) {
  const pausing = !!m.sw.pause;
  const ctl = h("div", "ctl",
    pausing ? h("button", { class: "primary", onclick: () => ctx.ask(null) }, "Resume")
      : [h("button", { onclick: () => ctx.ask({ mode: "drain" }), title: "Every Mac's job stops at its next safe point; nothing new starts" }, "Pause"),
        h("button", { class: "quiet", onclick: () => confirm("Freeze every Mac's job where it is now? (It goes on from there when resumed.)") && ctx.ask({ mode: "freeze" }), title: "Freeze every Mac's job where it is, at once" }, "Freeze")]);
  return [h("div", "verdict", h("div", "say", verdictText(m)), ctl),
    h("div", "alerts", alerts(m).map((x) => h("a", { class: `alert ${x.cls}`, href: `#${x.to}` }, x.text)))];
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

// Each tab's sections, in order (index.html's panels, by the tab's name).
function tabs(m) {
  const fin = m.lines.filter(finished).length;
  let md = null;
  try { md = mapData(m); } catch (e) { part("map data", () => { throw e; }); }
  return {
    overview: [
      part("d-overview", () => section("d-overview", null, null, overview(m))),
      part("d-map", () => mapSection(m, md)),
    ],
    jobs: [
      part("d-sched", () => section("d-sched", "Schedule", "each Mac's jobs: those that ran, running, to come",
        h("div", "box", schedule(m),
          // (What the coordinator's own scheduler decides, run ahead: docs/pool.md §7.2. What it can't see, said.)
          h("p", "note", "To come as the coordinator grants: by each Mac's room, cores and the NAS budget. Times and NAS weights are measured estimates, and no load from outside the build is assumed (optimistic while a Mac is in use); a pause, a repack, a Mac handing over on battery or a slow NAS moves the finish.")))),
      part("d-stages", () => section("d-stages", "Stages", m.lines.length ? `${fin} of ${m.lines.length} done · ⇄ any Mac may take it` : "", h("div", "box", stagesList(m)))),
    ],
    machines: [
      part("d-machines", () => section("d-machines", "Machines", `${m.macs.filter((x) => x.fresh).length} Macs${m.pages.length ? `, ${plural(m.pages.length, "page")}` : ""}`,
        h("div", "machines", m.macs.map((x) => machineCard(m, x)), m.gone.map(goneCard), pagesCard(m)))),
      part("d-pool", () => poolSection(last.agent, m.now, (body) => ctx.call("/work/lead", body).then(poll, (e) => ctx.onError?.(`couldn't ask about the lead: ${e.message}`))) || section("d-pool")),
      part("d-settings", () => settingsSection(ctx.call, last?.macs || [], render, ctx.onError)),
    ],
    regions: [part("d-regions", () => regionsPane(m, md))],
    nas: [part("d-nas", () => nasSection(last.agent, m.macs))],
    dropboxes: [part("d-inputs", () => dropBoxes(m))],
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
  const scrollY = window.scrollY;
  cards.clear();
  const feedTop = document.querySelector(".feed")?.scrollTop || 0;
  put(ctx.head, [].concat(part("d-head", () => head(m))));
  for (const [name, next] of Object.entries(tabs(m))) if (ctx.panels[name]) put(ctx.panels[name], next);
  const feedEl = document.querySelector(".feed");
  if (feedEl) feedEl.scrollTop = feedTop;
  window.scrollTo(0, scrollY);
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
  if (drawn.asking || (drawn.at && drawn.catalog === n && Date.now() - drawn.at < REGIONS_EVERY_MS)) return;
  drawn.asking = true;
  try {
    const [code, v] = await ctx.call("/work/regions", {});
    if (code !== 200 || !v?.regions) return;
    drawn.byId = Object.fromEntries(v.regions.map((r) => [r.id, r]));
    drawn.catalog = v.catalog ?? null; // the catalogue it was made from: asked again until it catches up
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

/** Starts the dashboard: its header status in `head`, each tab's sections in `panels[tab]`;
 * `onAnswer()` on each answer of the build's (the footer's Live dot); `call(path, body)` asks the coordinator ([status, json]); `ask(pause)`
 * pauses the build (null: lets it go on); `store` keeps this browser's own (what it last showed). */
export function dashboard(opts) {
  ctx = opts;
  ui.past = PAST.some(([s]) => s === ctx.store.get("past", ui.past)) ? ctx.store.get("past", ui.past) : ui.past;
  wireCards();
  ctx.ask = async (pause) => {
    try {
      // (No time of this browser's: the coordinator's clock orders the asks, and stamps it.)
      await ctx.call("/work/pause", { pause: pause && { ...pause, by: ctx.label, at: 0 } });
      poll();
    } catch (e) {
      ctx.onError?.(`couldn't ask the build to ${pause ? "pause" : "go on"}: ${e.message}`);
    }
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
  poll();
  startLive({ call: ctx.call, store: ctx.store, macs: () => last?.macs || [], active: () => location.hash === "#machines" || location.hash === "#nas" });
  startActivity({ call: ctx.call, macs: () => last?.macs || [], machines: () => shownMacs, active: () => location.hash === "#activity" });
}
