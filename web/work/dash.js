// The whole build at a glance (docs/workers.md, The page): the page at /work/, for anyone it answers.
// From the coordinator: /work/steps once (the steps table: every step's title, noun, stage and
// place, docs/pipeline.md), /work/swarm every 10 s (the lead's heartbeat with its checklist,
// forecast and resources, its helpers', every worker and lease, the hours' work), and
// /work/history after the last event this page has (what happened).
//
// One shape for everything: a step is named by its title, its targets counted in its noun ("8 of
// 480 tiles"), coloured by its stage; a stage is a card of its steps' rows, each row the same
// cells; a machine is a card of its jobs, each job the same lines; an event is one templated
// sentence. The parts: the verdict (going or not, done when, next on the map, what needs a look,
// the pause), the overview (how far, how long), the stages (each step to the end), the machines
// (what each does now and next, why one waits, its power, disk, memory and pace), the road to
// done (each machine's schedule, publishing), the regions, and the activity (the hours, what
// happened).

import { poolSection, termText } from "./pool.js";

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
const STAGE_COLOURS = { pass: "#8fa2b5", sites: "#c47fb5", terrain: "#c98b4a", slope: "#d9c35a", trees: "#5fae6b", roads: "#5b8fd8", tiles: "#4fb3c6", finish: "#7fb3a8", trains: "#a9a0d6", landmarks: "#9b7be0", buildings: "#a39a8c", publish: "var(--mark)", upkeep: "#7b8590" };
const UPDATE = "publishing";
const table = { steps: {}, stages: [] };
const S = {
  row: (id) => table.steps[id],
  title: (id) => table.steps[id]?.title || (id === UPDATE ? "Publishing" : id || ""),
  noun: (id) => table.steps[id]?.noun || "target",
  world: (id) => table.steps[id]?.scope === "world",
  stage: (id) => table.steps[id]?.stage || (id === UPDATE ? "publish" : ""),
  stageTitle: (id) => table.stages.find((g) => g.id === id)?.title || id,
  colour: (id) => STAGE_COLOURS[S.stage(id)] || "#7b8590",
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
const finished = (x) => (x.left != null ? x.left === 0 : x.total != null && x.done >= x.total && x.total > 0);
// A stage's counts, a noun at a time in its items' order (build::Line::counts): "398 of 398 tiles ·
// 18 of 18 areas", "2 runs left"; "done" or "to come" with no item to count (an older lead's line:
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
  const fc = a.forecast && { ...a.forecast, updates: a.forecast.updates ?? [] };
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
// Whether worker `name` is one of `host`'s slots (crate::agent::is_slot_of: "m4", "m4 #2", …).
// (A slot's number from its name: 1 for the bare host's, N for "<host> #N"; null for none.)
const slotNo = (name, host) => (name === host ? 1 : name.startsWith(`${host} #`) && /^\d+$/.test(name.slice(host.length + 2)) && +name.slice(host.length + 2) >= 2 ? +name.slice(host.length + 2) : null);
const isSlotOf = (name, host) => slotNo(name, host) != null;
// Slot k's worker name (crate::agent::slot_worker).
const slotName = (host, k) => (k ? `${host} #${k + 1}` : host);
// What the lead's coordinator says of a Mac, by its slots' names: its capacity (coord::Capacity),
// why nothing more fits it (the packing's refusal) and its share of the NAS budget (thousandths).
function macSaid(a, host) {
  const cap = (a.capacity || []).find(([nm]) => isSlotOf(nm, host))?.[1];
  const why = Object.entries(a.packing || {}).filter(([nm]) => isSlotOf(nm, host)).map(([, w]) => w);
  const shares = Object.entries(a.nas_share || {}).filter(([nm]) => isSlotOf(nm, host));
  const nas = a.nas_share && Object.keys(a.nas_share).length ? shares.reduce((t, [, w]) => t + w, 0) : null;
  return { cap, why, nas };
}
// The checklist's line (a stage's) that counts `step`, and its item for it.
const lineOf = (m, step) => m.lines.find((l) => (l.steps || []).includes(step) || (l.items || []).some((i) => i.step === step));
const itemOf = (m, step) => lineOf(m, step)?.items?.find((i) => i.step === step);

// ---- The verdict --------------------------------------------------------------------------------
function machineState(st, fresh, paused) {
  if (!fresh) return ["out of touch", "bad"];
  // (Its jobs, every slot's: building while any is.)
  const jobs = jobsOf(st);
  if (jobs.some((j) => !j.paused && !j.pausing)) return ["building", "run"];
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
  if (a.pool?.lead?.alert) out.push({ cls: "bad", text: a.pool.lead.alert, to: "pool" });
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
    if (/held for review/.test(w.why)) out.push({ cls: "warn", text: "Publishing held for review", to: "road" });
    else if (/failed \d+ times?/.test(w.why)) out.push({ cls: "warn", text: `${w.what}: waiting out a failure`, to: "details" });
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

// ---- The gate (docs/inputs.md §4.7) -------------------------------------------------------------
// A banner per held unit: its held files, each finding, Accept per warning and Accept All; errors
// say what to fix. A quiet line per unit being checked.
function gateBanners(m, accept) {
  const units = (m.a.inputs || []).filter((v) => v.state !== "ok");
  return units.map((v) => {
    if (v.state === "checking") return h("div", "gate checking", `${v.unit}: checking what was dropped…`);
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
  });
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
    parts.push(h("b", null, working ? "Building" : "Idle"), working ? ` on ${plural(working, "machine")}` : "");
  }
  if (fc?.done_at) {
    const r = fc.range;
    parts.push(" · done ≈ ", h("b", null, clock(fc.done_at)), r ? ` (${clock(r[0])}–${clock(r[1])})` : "");
  } else if (fc?.why) parts.push(` · ${fc.why}`);
  const next = fc?.updates?.find((r) => r.regions.length);
  if (next) {
    const names = next.regions.slice(0, 2).map((id) => m.regionName[id] || id).join(", ");
    parts.push(" · next on the map ≈ ", h("b", null, clock(next.at)), `: ${names}${next.regions.length > 2 ? ` +${next.regions.length - 2}` : ""}`);
  }
  return parts;
}

// ---- The overview -------------------------------------------------------------------------------
// How far (the roads' tiles: the bulk of the work, a share that means the same at any moment), how
// long, and the numbers that aren't a step's: the regions, the map, the machines.
function overview(m) {
  const { a, fc, now, lines } = m;
  const roads = itemOf(m, "unit") || lineOf(m, "unit");
  const big = roads?.total ? `${Math.floor((roads.done / roads.total) * 100)}%` : "–";
  const workLeft = fc ? fc.steps.reduce((t, s) => t + s.work_s, 0) : 0;
  const left = h("div", null,
    h("div", "big", big, h("small", null, roads?.total ? ` of the roads' tiles built (${n(roads.done)} of ${n(roads.total)})` : "")),
    h("div", "small dim", !fc ? "No forecast yet (the lead makes one with each plan)" : fc.done_at ? `${dur(workLeft)} of work left at the lead's pace; with every machine, done ≈ ${clock(fc.done_at)}, if the Macs keep going (awake, on mains or above 30%, reaching the NAS)` : `No finish to forecast: ${fc.why || "unknown"}`),
    fc?.done_at ? h("div", "small dim", `${Math.round((fc.measured || 0) * 100)}% of that time measured, the rest estimated · forecast ${ago(now, fc.at)}`) : null,
  );
  // The stages as a strip, each its colour, the one under way outlined.
  const strip = h("div", "strip", lines.map((l) => {
    const done = finished(l), running = (l.steps || []).some((s) => m.running.has(s));
    const i = h("i", { class: `${done ? "done" : ""} ${running ? "now" : ""}`, title: `${l.what}: ${countsOf(l)}` });
    i.style.background = done ? S.colour(l.steps?.[0]) : "";
    if (!done && l.total) { const b = h("b"); b.style.width = `${Math.round((l.done / l.total) * 100)}%`; b.style.background = S.colour(l.steps?.[0]); i.append(b); }
    return i;
  }));
  const fin = lines.filter(finished).length;
  const nowLines = lines.filter((l) => !finished(l) && (l.steps || []).some((s) => m.running.has(s))).map((l) => l.what);
  const stagesLine = h("div", "small dim", lines.length ? `${fin} of ${lines.length} stages done${nowLines.length ? ` · now: ${nowLines.join(", ")}` : ""}` : "");
  const regs = fc?.regions || [];
  const onMap = regs.filter((r) => r.on_map === true && !Object.keys(r.left).length).length;
  const asWas = regs.filter((r) => r.on_map === false || (r.on_map === true && Object.keys(r.left).length)).length;
  const readyNow = regs.filter((r) => !Object.keys(r.left).length && r.on_map !== true).length;
  const lastAt = lastUpdate(m);
  const nextUpdate = fc?.updates?.[0];
  const busy = m.macs.filter((x) => x.fresh && jobsOf(x.status).length).length;
  const kpis = h("div", "kpis",
    kpi("Regions on the map", regs.length ? `${onMap} of ${regs.length}` : "–", [asWas ? `${asWas} more as they were` : "", readyNow ? `${readyNow} done, recorded built by the next catalog` : ""].filter(Boolean).join(" · ") || null),
    kpi("Map last updated", lastAt ? ago(now, lastAt) : "–", nextUpdate ? `next ≈ ${clock(nextUpdate.at)}` : null),
    kpi("Machines", `${busy} of ${m.macs.length} Macs working`, m.pages.length ? `and ${plural(m.pages.length, "page")}` : null),
  );
  return h("div", "overview", h("div", "box", left, strip, stagesLine), kpis);
}

// ---- The stages ---------------------------------------------------------------------------------
// A card per stage of the checklist, each the same: its mark, title, count and bar; under it a row
// per step, each the same cells: mark, swatch, title, where it runs, count, bar, when it's done,
// why it waits; then its jobs to come. (crates/pipeline/src/agent/build.rs Line and Item.)
function stageCard(m, l) {
  const { fc } = m;
  const done = finished(l);
  const running = (l.steps || []).some((s) => m.running.has(s));
  const fcOf = (step) => fc?.steps.find((s) => s.step === step);
  const colour = S.colour(l.steps?.[0]);
  const card = h("div", { class: `stage ${done ? "done" : ""} ${running ? "now" : ""}`, style: { borderLeft: `3px solid ${colour}` } });
  card.append(h("div", "row head", mark(done, running), h("span", "title", l.what), h("span", "n", countsOf(l))));
  if (!done && l.total) card.append(pbar(l.done / l.total, running ? "" : "dimbar", colour));
  for (const it of l.items || []) {
    const itDone = finished(it), itRun = m.running.has(it.step);
    const f = fcOf(it.step);
    const bits = [];
    if (!itDone && f?.work_s) bits.push(`${dur(f.work_s)} of work`);
    if (!itDone && f?.done_at) bits.push(`done ≈ ${clock(f.done_at)}`);
    card.append(h("div", { class: `row item ${itDone ? "done" : ""}`, title: S.row(it.step)?.does || "" },
      mark(itDone, itRun), h("span", "title", S.title(it.step), S.any(it.step) ? h("span", { class: "dim", title: "Any Mac of the pool may take it" }, " ⇄") : null),
      h("span", "n", countOf(it, `${S.noun(it.step)}s`))));
    if (!itDone && it.total > 0) card.append(pbar(it.done / it.total, itRun ? "" : "dimbar", colour));
    if (bits.length) card.append(h("div", "when", bits.join(" · ")));
  }
  const note = [];
  if (!done && l.note) note.push(l.note);
  if (!done && l.next?.length) note.push(`${running ? "now" : "next"}: ${l.next.slice(0, 3).join("; then ")}${l.next.length > 3 ? `; and ${l.next.length - 3} more` : ""}`);
  if (note.length) card.append(h("div", "note", note.join(" · ")));
  return card;
}

function stages(m) {
  if (!m.lines.length) return h("div", "small dim", "No word from the lead yet.");
  return h("div", "stages", m.lines.map((l) => stageCard(m, l)));
}

// ---- Machines -----------------------------------------------------------------------------------
// A job's progress in words: "40% · 2.4 of 6 tiles · about 12 min left" (one item alone, its share).
function progressText(p, j, now) {
  const pct = Math.floor((p.done / p.total) * 100);
  // (One item whose share is said: "the planet filtered"; else "0.4 of 1 tile", its noun singular.)
  const unit = p.total === 1 ? p.unit.replace(/^(\w+?)s\b/, "$1") : p.unit;
  const amount = p.total === 1 && /^the /.test(p.unit) ? p.unit : `${Number.isInteger(p.done) || p.total > 100 ? n(Math.floor(p.done)) : (Math.floor(p.done * 10) / 10).toFixed(1)} of ${n(p.total)} ${unit}`;
  const eta = p.eta_s != null && !j.paused ? ` · about ${dur(p.eta_s)} left (≈ ${clock(now + p.eta_s)})` : "";
  return `${pct}% · ${amount}${eta}`;
}

// A job, as every machine's is shown: its name (its step's title and targets), its parts, its
// progress, how long it's run and what it holds, and why it's stopped or stuck.
function jobBlock(j, now) {
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
  if (j.paused) box.append(h("div", "why", `Frozen: ${j.paused}`));
  else if (j.pausing) box.append(h("div", "why", "Stopping at its next safe point (what it's on is kept)"));
  return box;
}

function nextBlock(m, name) {
  const next = (m.fc?.next?.[name] || []).filter((x) => x.until > m.now);
  if (!next.length) return null;
  const say = (x) => `${S.said(x.step, x.targets.length)} (${clock(x.from)}–${clock(x.until)})`;
  return h("div", "next", h("b", null, "Next: "), next.slice(0, 3).map(say).join(" · then "));
}

function facts(st, x, m) {
  const c = st.conditions || {}, r = st.resources || {};
  const out = [];
  out.push(c.ac === false ? chip(`battery ${c.battery ?? "?"}%`, c.battery != null && c.battery < 40 ? "warn" : "") : chip(c.battery != null ? `mains · ${c.battery}%` : "mains"));
  out.push(c.nas === false ? chip("NAS unreachable", "bad") : chip(`NAS${r.nas_ms != null ? ` ${r.nas_ms} ms` : ""}${c.home === false ? " via Tailscale" : ""}${r.nas_free_tb != null && x.role === "lead" ? ` · ${r.nas_free_tb} TB free` : ""}`, c.home === false ? "warn" : "", "How long the NAS took to answer, and its free space"));
  if (r.disk_free_gb != null) out.push(chip(`${r.disk_free_gb} GB free${r.cache_gb != null ? ` (+${r.cache_gb} GB cache)` : ""}`, r.disk_free_gb < 10 ? "bad" : r.disk_free_gb < 20 ? "warn" : "", "Free on its disk; the caches the agent may drop to make room"));
  if (r.mem_gb) out.push(chip(`${r.mem_gb} GB memory${r.mem_free_pct != null ? `, ${r.mem_free_pct}% free` : ""}`, r.mem_free_pct != null && r.mem_free_pct < 10 ? "warn" : ""));
  if (r.load1 != null) out.push(chip(`load ${r.load1} on ${r.cores} cores`));
  out.push(chip(c.idle_s != null && c.idle_s < 300 ? "in use (half the cores)" : "not in use", "", "While someone uses the Mac, its jobs get half its cores"));
  if (st.app) out.push(chip(`app ${st.app}`, x.role !== "lead" && m.a.app && st.app !== m.a.app ? "warn" : ""));
  const sp = m.fc?.speed?.[x.name];
  if (sp != null && x.role !== "lead") out.push(chip(`${Math.round(sp * 100)}% of the lead's pace${(m.fc.guessed || []).includes(x.name) ? " (a guess until measured)" : ""}`));
  return h("div", "facts", out);
}

// The last day by the hour: how busy it was (a bar an hour; several, the pages, together, up to the
// hour), paused hours shaded; and what it finished, each step counted in its noun.
function spark(m, name) {
  const rows = m.sw.rates?.rows;
  if (!rows?.length) return null;
  const names = Array.isArray(name) ? name : [name];
  const bars = rows.map((r) => {
    const busy = Math.min(3600, names.reduce((t, nm) => t + (r.busy_s?.[nm] || 0), 0));
    const i = h("i", { class: r.paused_s > 1800 ? "p" : "", title: `${clock(r.t)}: busy ${Math.round(busy / 60)} min${r.paused_s ? `, paused ${Math.round(r.paused_s / 60)} min` : ""}` });
    i.style.height = `${Math.round((busy / 3600) * 100)}%`;
    return i;
  });
  const today = rows.reduce((t, r) => { for (const nm of names) for (const [s, k] of Object.entries(r.done?.[nm] || {})) t[s] = (t[s] || 0) + k; return t; }, {});
  const said = Object.entries(today).sort((x, y) => y[1] - x[1]).map(([s, k]) => S.said(s, k)).join(" · ");
  return [h("div", "spark", bars), h("div", "sub", `Last 24 h: ${said || "nothing finished"}`)];
}

function fitText(w) {
  if (!w?.fit?.length) return null;
  const parts = w.fit.filter((f) => f.offered).map((f) => {
    const bits = [`${S.title(f.step)}: ${n(f.fits)} of ${n(f.offered)} ${S.noun(f.step)}s it can take`];
    if (f.held) bits.push(`${n(f.held)} with others`);
    if (f.too_big) bits.push(`${n(f.too_big)} too big for its ${n(Math.round(w.mem_mb / 102.4) / 10)} GB`);
    if (f.kept_from) bits.push(`${n(f.kept_from)} kept from it after failing`);
    return bits.join(", ");
  });
  return parts.length ? h("div", "sub", `Offered: ${parts.join("; ")}`) : null;
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
// guard's limit, cores busy, disk reserved and free, its share of the NAS budget.
function capacityBlock(said) {
  const c = said.cap;
  if (!c && said.nas == null) return null;
  const rows = [];
  if (c) {
    rows.push(capBar("memory", c.held_mb, c.limit_mb, `${gb(c.held_mb)} of ${gb(c.limit_mb)} · ${gb(c.mem_mb)} free${c.guard ? "" : " · unguarded"}`, 0, c.limit_mb && c.held_mb / c.limit_mb > 0.9 ? "warn" : ""));
    const busy = Math.max(0, c.cores - c.cores_free);
    rows.push(capBar("cores", busy, c.cores, `${busy} of ${c.cores} busy${c.in_use && c.kept_cores ? ` · ${c.kept_cores} kept for its owner` : ""}`));
    const disk = c.disk_mb + c.reserved_mb;
    rows.push(capBar("disk", c.reserved_mb, disk, `${gb(c.reserved_mb)} reserved · ${gb(c.disk_mb)} free`, 0, c.disk_mb < 10240 ? "warn" : ""));
  }
  if (said.nas != null) rows.push(capBar("NAS", said.nas, 1000, `${Math.round(said.nas / 10)}% of the budget`, 0, said.nas > 1000 ? "warn" : ""));
  return h("div", { class: "caps", title: "As the lead's coordinator last heard it: memory its jobs hold of the guard's limit (and free for more, less what they're predicted to need); cores busy by the last minute's load; disk its jobs reserved, and free less the reserve and the owner's room target; its jobs' share of the NAS budget" }, rows);
}

// The lead's ramp (crate::agent::RampView): its slots now of the most; held (its slots kept at the
// start) or not clean (growth stopped), else clean since when and when it grows.
function rampText(r, now) {
  const bits = [`${r.slots}${r.most ? ` of ${r.most}` : ""} slots (ramp reached ${r.reached})`];
  if (r.held) bits.push(`held: ${r.held}`);
  else if (r.not_clean) bits.push(`not clean: ${r.not_clean}`);
  else if (r.clean_since) bits.push(`clean ${dur(now - r.clean_since)}${r.grows_at ? `, grows ≈ ${clock(r.grows_at)} if its slots keep busy` : ""}`);
  return bits.join(" · ");
}

// A machine's card, each the same: who and how it is (the lead marked coordinator, its ramp); its
// capacity; a block per running job, by slot, with what's next there; why nothing more fits (or
// why none runs); its facts; its day.
function machineCard(m, x) {
  const st = x.status, now = m.now;
  const [state, cls] = machineState(st, x.fresh, m.sw.pause);
  const jobs = jobsOf(st);
  const said = macSaid(m.a, x.name);
  const role = x.role === "lead" ? "lead · coordinator" : "member";
  const card = h("div", { class: `mc ${x.fresh ? "" : "off"}`, style: { borderLeft: `3px solid ${x.colour}` } },
    h("div", "top", h("span", "name", x.name), h("span", "role", role), chip(state, cls), jobs.length ? chip(plural(jobs.length, "job")) : null, h("span", "right", `heard from ${ago(now, st.beat)}`)));
  if (x.role === "lead" && st.ramp) card.append(h("div", `sub ramp ${st.ramp.held ? "held" : st.ramp.not_clean ? "notclean" : ""}`, rampText(st.ramp, now)));
  card.append(...[capacityBlock(said)].filter(Boolean));
  jobs.forEach((j) => {
    // (Its real slot, slots may be sparse: jobsOf.)
    const k = j.slot;
    const b = jobBlock(j, now);
    b.querySelector(".what").prepend(h("span", "slot", `${k + 1}`));
    card.append(b);
    card.append(...[nextBlock(m, slotName(x.name, k))].filter(Boolean));
  });
  if (!jobs.length) {
    // Why it isn't building.
    const why = (st.waiting || []).filter((w) => !w.step || x.role !== "lead").slice(0, 2);
    card.append(h("div", "job", h("div", "what dim", m.sw.pause ? "Paused with the build" : why.length ? "Waiting" : "Nothing to build now")));
    for (const w of why) card.append(h("div", "sub", x.role === "lead" ? `${w.what}: ${w.why}` : w.why));
    card.append(...[nextBlock(m, x.name)].filter(Boolean));
  }
  for (const w of said.why) card.append(h("div", "fits", h("b", null, "Nothing more fits: "), w));
  const names = Object.keys(m.fc?.lanes || {}).filter((nm) => isSlotOf(nm, x.name));
  const more = [x.role === "helper" ? fitText(x.worker) : null, facts(st, x, m), ...(spark(m, names.length ? names : [x.name]) || [])];
  card.append(...more.filter(Boolean));
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
  for (const e of spark(m, m.pages.map((p) => p.name)) || []) card.append(e);
  return card;
}

// ---- The road to done ---------------------------------------------------------------------------
let tip = null;
function showTip(ev, text) {
  if (!tip) { tip = h("div", "tip"); document.body.append(tip); }
  tip.textContent = text;
  tip.style.display = "block";
  const x = Math.min(window.innerWidth - 290, ev.clientX + 12), y = ev.clientY + 14;
  tip.style.left = `${x}px`;
  tip.style.top = `${y}px`;
}
const hideTip = () => tip && (tip.style.display = "none");

// Each machine's lane: a run of a step's targets is a segment in its stage's colour; publishing
// hatched; the regions out as marks across the lanes.
function schedule(m) {
  const fc = m.fc;
  if (!fc?.lanes || !Object.keys(fc.lanes).length) return h("div", "small dim", "No schedule yet (the forecast comes with the lead's next plan).");
  const t0 = m.now;
  const t1 = Math.max(fc.done_at || 0, ...Object.values(fc.lanes).flat().map((l) => l.until), t0 + 3600);
  const span = t1 - t0;
  const x = (t) => `${(((Math.max(t0, Math.min(t1, t)) - t0) / span) * 100).toFixed(3)}%`;
  // Each Mac's lanes, a slot each in order, the lead's first; then any other the forecast has.
  const ordered = m.macs.flatMap((mm) => Object.keys(fc.lanes).filter((nm) => isSlotOf(nm, mm.name)).sort((p, q) => slotNo(p, mm.name) - slotNo(q, mm.name)));
  const names = ordered.concat(Object.keys(fc.lanes).filter((nm) => !ordered.includes(nm)));
  const lanes = h("div", "lanes");
  const stagesSeen = new Set();
  const segment = (track, l, say) => {
    const seg = h("i", { class: l.step === UPDATE ? "update" : "", onmousemove: (e) => showTip(e, say), onmouseleave: hideTip, onclick: (e) => showTip(e, say) });
    Object.assign(seg.style, { left: x(l.from), width: `calc(${x(l.until)} - ${x(l.from)})`, background: S.colour(l.step) });
    track.append(seg);
    if (l.step !== UPDATE) stagesSeen.add(S.stage(l.step));
  };
  for (const nm of names) {
    const track = h("div", "track");
    for (const l of fc.lanes[nm]) {
      if (l.until < t0) continue;
      const say = l.step === UPDATE ? `${nm}: Publishing (${plural(l.n, "region")}) ${clock(l.from)}–${clock(l.until)}` : `${nm}: ${S.said(l.step, l.n)}, ${clock(l.from)}–${clock(l.until)}`;
      segment(track, l, say);
    }
    lanes.append(h("div", { class: "ln", title: nm }, nm), track);
  }
  // The pages: a job's tasks (crate::agent::steps::TASKS) as the lead runs the job, when a page is
  // measured faster than the lead at that kind (crate::offload) and spares what one takes.
  const own = Object.keys(fc.lanes).filter((nm) => isSlotOf(nm, m.a.host)).flatMap((nm) => fc.lanes[nm]).sort((p, q) => p.from - q.from);
  const pace = (p, kind) => (p.paces ? p.paces[kind] : kind === "tail" ? p.tail_pace : null);
  const fast = (kind) => m.pages.some((p) => (p.mem_mb || 0) >= (m.sw.task_mb?.[kind] || 0) && pace(p, kind) != null && pace(p, kind) * 1.25 < 1);
  const helps = Object.values(table.steps).filter((t) => t.of && fast(t.id) && own.some((l) => l.step === t.of && l.until > t0));
  if (helps.length) {
    const track = h("div", "track");
    const who = m.pages.map((p) => p.label).join(", ");
    for (const t of helps) {
      for (const l of own.filter((l) => l.step === t.of && l.until > t0)) segment(track, { ...l, step: t.id }, `${who}: ${t.title} as the lead runs ${S.said(t.of, l.n)}, ${clock(l.from)}–${clock(l.until)}`);
    }
    lanes.append(h("div", { class: "ln", title: who }, m.pages.length > 1 ? `Pages (${m.pages.length})` : m.pages[0].label), track);
  }
  // Ticks: every 1, 2, 3, 6, 12 or 24 hours, on the local hour (midnight a day's name).
  const step = [1, 2, 3, 6, 12, 24, 48].find((hh) => span / (hh * 3600) <= 8) || 48;
  const axis = h("div", "axis");
  const d = new Date(t0 * 1000);
  d.setMinutes(0, 0, 0);
  while (d.getTime() / 1000 < t0 || d.getHours() % Math.min(step, 24)) d.setHours(d.getHours() + 1);
  for (; d.getTime() / 1000 < t1; d.setHours(d.getHours() + step)) {
    const label = d.getHours() === 0 ? DAYS[d.getDay()] : `${String(d.getHours()).padStart(2, "0")}:00`;
    axis.append(h("span", { style: { left: x(d.getTime() / 1000) } }, label));
  }
  lanes.append(h("div"), axis);
  // Each publishing, a mark across the lanes, the regions it adds when pointed at.
  const marks = h("div", "updates");
  for (const r of fc.updates || []) {
    if (r.at < t0 || r.at > t1) continue;
    const say = `${r.last ? "Last on the map" : "On the map"} ≈ ${clock(r.at)}: ${r.regions.length ? r.regions.slice(0, 6).map((id) => m.regionName[id] || id).join(", ") + (r.regions.length > 6 ? ` and ${r.regions.length - 6} more` : "") : "the rest"}`;
    marks.append(h("i", { style: { left: x(r.at) }, onmousemove: (e) => showTip(e, say), onmouseleave: hideTip, onclick: (e) => showTip(e, say) }));
  }
  // The legend: the stages seen, in the build's order; publishing (hatched), out (a mark).
  const order = table.stages.map((g) => g.id);
  const legend = h("div", "legend", [...stagesSeen].sort((p, q) => order.indexOf(p) - order.indexOf(q)).map((g) => h("span", null, h("i", { style: { background: STAGE_COLOURS[g] || "#7b8590" } }), S.stageTitle(g))), h("span", null, h("i", { class: "rhatch" }), "publishing"), h("span", null, h("i", { class: "rmark" }), "regions out on the map"));
  return h("div", "sched", h("div", "lanewrap", lanes, marks), legend);
}

// The regions: their order to the map with their publishing between, or by name, or by what's left; each
// its state, what it has left, when it's done and when it's on the map.
const ui = { order: "map", filter: "", all: false, feed: "all" };
function regionsList(m, rerender) {
  // (Typing redraws the rows alone: the box keeps its keyboard.)
  const host = h("div");
  const draw = () => host.replaceChildren(...regionRows(m, rerender));
  const bar = h("div", "regbar",
    h("input", { placeholder: `Find one of ${(m.fc?.regions || []).length || Object.keys(m.a.built || {}).length} regions…`, value: ui.filter, oninput: (e) => { ui.filter = e.target.value; draw(); }, autocapitalize: "off", spellcheck: "false" }),
    h("div", "seg", [["map", "To the map"], ["name", "A–Z"], ["left", "Most left"]].map(([k, t]) => h("button", { class: ui.order === k ? "on" : "", onclick: () => { ui.order = k; rerender(); } }, t))));
  draw();
  return h("div", null, bar, host);
}

function regionRows(m, rerender) {
  const fc = m.fc, built = m.a.built || {};
  const regs = (fc?.regions || []).map((r) => ({ ...r, name: m.regionName[r.id] || r.id, built: built[r.id] }));
  if (!regs.length) {
    // (An older lead: its roads' tiles built alone.)
    const ids = Object.keys(built);
    if (!ids.length) return [h("div", "small dim", "No regions yet.")];
    return [h("div", "regs", ids.map((id) => h("div", "rg", h("span", "nm", m.regionName[id] || id), h("span", "when", `${built[id].built} of ${built[id].total} tiles built`))))];
  }
  const q = ui.filter.trim().toLowerCase();
  const leftOf = (r) => Object.values(r.left).reduce((t, k) => t + k, 0);
  let list = regs.filter((r) => !q || r.name.toLowerCase().includes(q) || r.id.includes(q));
  const mapOrder = (r) => (r.on_map === true && !leftOf(r) ? 2e12 : r.map_at || 1e12) + r.rank;
  if (ui.order === "map") list.sort((x, y) => mapOrder(x) - mapOrder(y));
  else if (ui.order === "name") list.sort((x, y) => x.name.localeCompare(y.name));
  else list.sort((x, y) => leftOf(y) - leftOf(x) || x.name.localeCompare(y.name));
  const total = list.length;
  if (!ui.all && !q) list = list.slice(0, 30);
  const out = h("div", "regs");
  let lastUpdate = null;
  for (const r of list) {
    const lft = leftOf(r);
    if (ui.order === "map" && r.map_at && r.map_at !== lastUpdate) {
      const update = fc.updates.find((x) => x.at === r.map_at);
      out.append(h("div", "update", `${update?.last ? "Last on the map" : "On the map"} ≈ ${clock(r.map_at)}${update ? ` · ${plural(update.regions.length, "region")}` : ""}`));
      lastUpdate = r.map_at;
    } else if (ui.order === "map" && !r.map_at && lastUpdate !== "on") {
      out.append(h("div", "update", "On the map"));
      lastUpdate = "on";
    }
    let state, cls = "";
    if (!lft && r.on_map === true) [state, cls] = ["on the map", "ok"];
    else if (!lft) [state, cls] = ["done; recorded built by the next catalog", "ok"];
    else if (r.rank === 0) [state, cls] = ["building now", "run"];
    else [state, cls] = [`#${r.rank + 1} in line`, ""];
    if (r.on_map === false) state += " · on the map as it was";
    const when = lft ? `done ≈ ${clock(r.ready_at)} · on the map ≈ ${clock(r.map_at)}` : r.map_at ? `on the map ≈ ${clock(r.map_at)}` : "";
    // What's left, a step at a time in the table's order, each counted in its noun.
    const order = Object.keys(table.steps);
    const leftWords = Object.entries(r.left).filter(([, k]) => k).sort(([p], [q]) => order.indexOf(p) - order.indexOf(q)).map(([s, k]) => S.said(s, k)).join(" · ");
    const tot = r.built?.total || 0;
    const stack = h("div", "stack");
    if (tot) {
      const seg = (k, c) => { const i = h("i"); Object.assign(i.style, { width: `${(k / tot) * 100}%`, background: c }); return i; };
      stack.append(seg(r.built.built, "var(--ok)"), seg(r.left.unit || 0, "var(--spark)"));
    }
    out.append(h("div", { class: `rg ${!lft && r.on_map === true ? "done" : ""}` },
      h("span", "nm", r.name, " ", chip(state, cls)), h("span", "when", when),
      h("div", "left", tot ? stack : null, [tot ? `${r.built.built} of ${tot} tiles built` : "", leftWords ? `left: ${leftWords}` : ""].filter(Boolean).join(" · "))));
  }
  const more = !q && total > list.length ? h("button", { class: "quiet more", onclick: () => { ui.all = true; rerender(); } }, `Show all ${total}`) : null;
  return [out, more].filter(Boolean);
}

// When the map last got new data: the last catalog the lead read (its number and time), else
// its last catalog job's end.
function lastUpdate(m) {
  return m.a.catalog?.at || (m.a.recent || []).find((d) => stepOf(d.id) === "catalog" && d.ok)?.ended || null;
}

function publishing(m) {
  const { a, fc, now } = m;
  const at = lastUpdate(m);
  const lines = [];
  lines.push(h("div", null, "Last update: ", at ? h("b", null, `${clock(at)} (${ago(now, at)})`) : "none yet", a.catalog ? h("span", "dim", ` · catalog ${a.catalog.n}`) : null));
  const next = fc?.updates?.[0];
  if (next) lines.push(h("div", null, "Next: ", h("b", null, `≈ ${clock(next.at)}`), next.regions.length ? ` with ${next.regions.slice(0, 4).map((id) => m.regionName[id] || id).join(", ")}${next.regions.length > 4 ? ` and ${next.regions.length - 4} more` : ""}` : ""));
  const waits = (a.waiting || []).filter((w) => w.step === "catalog" || /publish/i.test(w.what));
  for (const w of waits) lines.push(h("div", "small warn", w.why));
  // Publishing as the lead last planned it, and why (agent::build::PublishLine).
  const u = a.publishing;
  if (u) {
    lines.push(h("div", null, h("b", null, "Publishing"), `: ${u.why}`));
    if (!u.low) lines.push(h("div", "small dim", "The low zooms wait for their turn (state/pool/low-zoom-later)."));
    if (u.regions?.length) lines.push(h("div", "small", `Done, not on the map as they are: ${u.regions.slice(0, 6).map((r) => `${m.regionName[r.id] || r.id} (${r.kinds.join(", ")})`).join(", ")}${u.regions.length > 6 ? ` and ${u.regions.length - 6} more` : ""}`));
  } else {
    lines.push(h("div", "small dim", "A unit's map tiles are drawn as soon as it's built, and a catalog goes out at most each minute."));
  }
  if (fc?.updates?.length > 1) lines.push(h("div", "small dim", `Regions go on the map ${plural(fc.updates.length, "time")} more; the last ≈ ${clock(fc.updates[fc.updates.length - 1].at)}`));
  return h("div", null, lines);
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
    // (A Mac's build caches trimmed after the build, or cleared on its owner's ask: what was freed.)
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
    out.append(h("div", { class: seen != null && r.e.seq > seen ? "new" : "" }, h("span", "t", new Date(r.e.t * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })), h("span", cls, r.e.step ? [swatch(S.colour(r.e.step)), text] : text)));
  }
  if (!rows.length) out.append(h("div", "dim", events.length ? "Nothing of that kind lately." : "Nothing yet."));
  return out;
}

// What happened since this page last showed it: a sentence.
function sinceLine(m, events, seen) {
  if (seen == null) return null;
  const fresh = events.filter((e) => e.seq > seen);
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
  if (updates.length) bits.push(`${plural(updates.length, "catalog")}${updates.some((r) => r.targets?.length) ? `: ${updates.flatMap((r) => r.targets || []).slice(0, 4).map((id) => m.regionName[id] || id).join(", ")}` : ""}`);
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
  // The steps table itself: every step the same row (docs/pipeline.md).
  const rows = Object.values(table.steps);
  if (rows.length) box.append(h("h3", null, "The steps", h("span", null, "every step's row of the table: what it makes, of what, where it runs")), tbl(["Stage", "Step", "Does", "Kind", "Each target", "Runs on", "A job"], rows.map((s) => [S.stageTitle(s.stage), s.title, s.does, s.kind, s.of ? `a task of ${S.title(s.of)}` : s.scope === "world" ? "the world" : `${/^[aeiou]/.test(s.noun) ? "an" : "a"} ${s.noun}`, s.place === "any" ? "any Mac" : "the lead", s.of ? "" : s.place === "any" ? "a quarter hour's" : "all"])));
  box.append(h("div", "fresh", `Updated ${clock(now)} · the lead's heartbeat ${a.beat ? ago(now, a.beat) : "–"}${a.started ? `, its agent running since ${clock(a.started)} (app ${a.app})` : ""}${m.fc ? ` · forecast ${ago(now, m.fc.at)}` : ""} · history #${sw.seq ?? "–"}`));
  return h("details", { class: "more-box", open: ui.detailsOpen || null, ontoggle: (e) => (ui.detailsOpen = e.target.open) }, h("summary", null, "Details"), box);
}

// ---- The page -----------------------------------------------------------------------------------
let ctx = null;
let last = null;
let events = [];
let seenSeq = undefined;

function section(cls, id, title, sub, ...body) {
  return h("section", { class: cls, id }, title ? h("h2", null, title, sub ? h("span", null, sub) : null) : null, ...body);
}

function render() {
  if (!last || !ctx) return;
  const m = model(last);
  const root = ctx.root;
  const al = alerts(m);
  const pausing = !!last.pause;
  const ctl = h("div", "ctl",
    pausing ? h("button", { class: "primary", onclick: () => ctx.ask(null) }, "Resume the build")
      : [h("button", { onclick: () => ctx.ask({ mode: "drain" }), title: "Every Mac's job stops at its next safe point; nothing new starts" }, "Pause the build"),
        h("button", { class: "quiet", onclick: () => confirm("Freeze every Mac's job where it is now? (It goes on from there when resumed.)") && ctx.ask({ mode: "freeze" }), title: "Freeze every Mac's job where it is, at once" }, "Pause it now")]);
  const verdict = section("d-verdict", "verdict", null, null,
    h("div", "verdict", h("div", "say", verdictText(m)), ctl),
    h("div", "alerts", al.map((x) => h("span", { class: `alert ${x.cls}`, onclick: () => document.getElementById(x.to)?.scrollIntoView({ behavior: "smooth", block: "start" }) }, x.text))));
  const fin = m.lines.filter(finished).length;
  const stagesSec = section("d-stages", "stages", "Stages", m.lines.length ? `${fin} of ${m.lines.length} done · ⇄ any Mac may take it` : "", stages(m));
  const machines = section("d-machines", "machines", "Machines", `${m.macs.filter((x) => x.fresh).length} Macs${m.pages.length ? `, ${plural(m.pages.length, "page")}` : ""}`,
    h("div", "machines", m.macs.map((x) => machineCard(m, x)), m.gone.map(goneCard), pagesCard(m)));
  const road = section("d-road", "road", "Road to done", m.fc?.done_at ? `done ≈ ${clock(m.fc.done_at)}` : "",
    h("div", "road",
      h("div", "box", h("h3", null, "Schedule", h("span", null, "each machine's work from now to the end")), schedule(m)),
      h("div", "box", h("h3", null, "Publishing"), publishing(m))));
  // (Their own part: while their search has the keyboard, only it waits to be refreshed.)
  const regions = section("d-regions", "regions", null, null,
    h("div", "box", h("h3", null, "Regions", h("span", null, m.fc ? "built one at a time: those the map lacks first, then those with the fewest tiles left" : "")), regionsList(m, render)));
  const activity = section("d-activity", "activity", "Activity", "the last day",
    h("div", "box", hours(m),
      h("div", "feedbar", h("div", "seg", [["all", "All"], ["problems", "Problems"], ["publishing", "Publishing"], ["pauses", "Pauses & conditions"]].map(([k, t]) => h("button", { class: ui.feed === k ? "on" : "", onclick: () => { ui.feed = k; render(); } }, t)))),
      sinceLine(m, events, seenSeq), feed(m, events, seenSeq)));
  const scrollY = window.scrollY;
  const feedTop = root.querySelector(".feed")?.scrollTop || 0;
  const pool = poolSection(last.agent, m.now, (body) => ctx.call("/work/lead", body).then(poll, (e) => ctx.onError?.(`couldn't ask about the lead: ${e.message}`))) || section("d-pool", "pool", null, null);
  const banners = gateBanners(m, (body) => ctx.call("/work/inputs", body).then(poll, (e) => ctx.onError?.(`couldn't accept: ${e.message}`)));
  root.classList.toggle("gated", banners.length > 0);
  const gate = h("section", { class: "d-inputs", id: "inputs" }, banners);
  const next = [verdict, gate, section("d-overview", "overview", "Overview", null, overview(m)), stagesSec, machines, pool, road, regions, activity, section("d-details", "details", null, null, details(m))];
  // (A section where the user is typing or has text selected stays as it was until they're done:
  // a refresh would take their keyboard or selection away.)
  const sel = window.getSelection();
  const busy = (el) => el && (el.contains(document.activeElement) && document.activeElement.tagName === "INPUT" || (sel && !sel.isCollapsed && el.contains(sel.anchorNode)));
  if (root.children.length === next.length) {
    next.forEach((el, i) => { if (!busy(root.children[i])) root.children[i].replaceWith(el); });
  } else root.replaceChildren(...next);
  const feedEl = root.querySelector(".feed");
  if (feedEl) feedEl.scrollTop = feedTop;
  window.scrollTo(0, scrollY);
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

async function pollOnce() {
  if (!ctx) return;
  try {
    await loadTable();
    const [code, sw] = await ctx.call("/work/swarm", {});
    if (code !== 200 || !sw) return;
    last = sw;
    // What happened since the last event this page has (a few hundred at first).
    if (sw.seq != null) {
      const from = events.length ? events[events.length - 1].seq : Math.max(0, sw.seq - 600);
      if (sw.seq > from) {
        const [c2, hist] = await ctx.call("/work/history", { since: from, max: 1000 }).catch(() => [0, null]);
        if (c2 === 200 && hist?.events) {
          const last = events.length ? events[events.length - 1].seq : -1;
          events = events.concat(hist.events.filter((e) => e.seq > last)).slice(-2000);
        }
      }
      // (What this browser last showed: none on a first visit, then nothing's highlighted.)
      if (seenSeq === undefined) seenSeq = ctx.store.get("seenSeq", null);
    }
    render();
  } catch (e) {
    ctx.onState?.(`can't reach the lead (${e.message})`);
  }
}

/** Starts the dashboard in `root`: `call(path, body)` asks the coordinator ([status, json]); `ask(pause)`
 * pauses the build (null: lets it go on); `store` keeps this browser's own (what it last showed). */
export function dashboard(opts) {
  ctx = opts;
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
  const keep = () => { if (events.length) { left = events[events.length - 1].seq; ctx.store.set("seenSeq", left); } };
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") keep();
    else { if (left != null) seenSeq = left; poll(); }
  });
  window.addEventListener("pagehide", keep);
  setInterval(() => document.visibilityState === "visible" && poll(), POLL_MS);
  poll();
}
