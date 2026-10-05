// The whole build at a glance (docs/workers.md, The page): both pages' (/work/ and /work/watch/).
// From the coordinator: /work/swarm every 10 s (the build Mac's heartbeat with its checklist,
// forecast and resources, its helpers', every worker and lease, the hours' work), and
// /work/history after the last event this page has (what happened). Five parts, each answering
// many questions at once: the verdict (going or not, done when, next on the map, what needs a look,
// the pause), the overview (how far, how long), the machines (what each does now and next, why
// one waits, its power, disk, memory and pace), the road to done (each machine's schedule, the
// steps, the regions and when each reaches the map) and the activity (the hours, what happened).

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

// A step's name, short (lanes, legends) and its colour.
const STEP = {
  "osm-pass": ["OpenStreetMap pass", "#8fa2b5"], "pass-sets": ["The pass's sets", "#8fa2b5"], trailends: ["Route ends", "#8fa2b5"], reach: ["Roads' reach", "#8fa2b5"],
  "terrain-z8": ["World terrain", "#c98b4a"], buildings: ["Roadside buildings", "#8fa2b5"], summits: ["Summits", "#8fa2b5"], labels: ["Place labels", "#8fa2b5"],
  "heritage-sites": ["Heritage sites", "#c47fb5"], terrain: ["Terrain", "#c98b4a"], slope: ["Slope", "#d9c35a"], trees: ["Tree cover", "#5fae6b"],
  unit: ["Areas", "#5b8fd8"], tail: ["Areas' last steps", "#6cc28a"], pack: ["Map tiles", "#4fb3c6"], lo: ["Zoomed-out tiles", "#4fb3c6"], prune: ["Pruning", "#8fa2b5"],
  roadunits: ["Road index", "#4fb3c6"], stations: ["Rail stops", "#4fb3c6"], ferries: ["Ferries", "#4fb3c6"], "terrain-root": ["World terrain", "#c98b4a"], "slope-root": ["World slope", "#d9c35a"],
  "rail-feeds": ["Rail timetables", "#8fa2b5"], rail: ["Trains a day", "#8fa2b5"], pois: ["Landmark candidates", "#9b7be0"], peaks: ["Peaks", "#b48ae8"],
  items: ["Wikidata facts", "#c47fb5"], heritage: ["Heritage details", "#c47fb5"], marks: ["Landmarks", "#c47fb5"], overlays: ["Area overlays", "#c47fb5"],
  catalog: ["Publishing", "#e6e6e6"], "catalog-held": ["Publishing (held)", "#e6e6e6"], round: ["Publishing round", "#e6e6e6"], gc: ["Clean-up", "#8fa2b5"], backup: ["Backup", "#8fa2b5"],
};
const stepName = (s) => (STEP[s] || [s])[0];
const stepColour = (s) => (STEP[s] || [0, "#7b8590"])[1];
// The noun a step's targets are counted in.
const NOUN = { unit: "area", terrain: "area", slope: "area", trees: "large tile", pois: "area", peaks: "area", pack: "tile", lo: "tile", tail: "task" };
const targets = (step, k) => plural(k, NOUN[step] || "job");
// What was finished, in words: "3 areas", "1 terrain area", "5 map tiles".
const DID = { unit: ["area", "areas"], terrain: ["terrain area", "terrain areas"], slope: ["slope area", "slope areas"], trees: ["tree-cover tile", "tree-cover tiles"], pack: ["map tile", "map tiles"], lo: ["zoomed-out tile", "zoomed-out tiles"], pois: ["area's candidates", "areas' candidates"], peaks: ["area's peaks", "areas' peaks"], tail: ["area's last steps", "areas' last steps"], catalog: ["map update", "map updates"] };
const did = (step, k) => (DID[step] ? plural(k, ...DID[step]) : `${stepName(step)}${k > 1 ? ` ×${k}` : ""}`);
// Machines' colours, the build Mac first.
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
const pbar = (frac, cls = "") => { const i = h("i"); i.style.width = `${Math.round(Math.max(0, Math.min(1, frac)) * 100)}%`; return h("div", `pbar ${cls}`, i); };
const chip = (text, cls = "", title) => h("span", { class: `chip ${cls}`, title }, text);

// ---- The data's shape, older coordinators' too --------------------------------------------------
function model(sw) {
  const a = sw.agent || {};
  const now = sw.now;
  const fresh = (beat) => beat && now - beat < STALE_S;
  const fc = a.forecast || null;
  const regionName = Object.fromEntries((a.regions || []).map((r) => [r.id, r.name]));
  const helpers = (a.helpers || []).filter((x) => x.beat && now - x.beat < 600);
  const pages = (sw.workers || []).filter((w) => w.kind === "web" && w.seen_s < 900);
  const leasesOf = (name) => (sw.leases || []).filter((l) => l.worker === name);
  // Every machine: the build Mac, its helpers, then the pages (one entry).
  const macs = [];
  if (a.host) macs.push({ name: a.host, role: "build Mac", status: a, fresh: fresh(a.beat), colour: MACHINE_COLOURS[0] });
  helpers.forEach((x, i) => macs.push({ name: x.host, role: "helper", status: x, fresh: fresh(x.beat), worker: (sw.workers || []).find((w) => w.name === x.host), colour: MACHINE_COLOURS[(i + 1) % MACHINE_COLOURS.length] }));
  const colourOf = (name) => macs.find((m) => m.name === name)?.colour || (pages.some((p) => p.name === name) ? "#6cc28a" : "#7b8590");
  const tasks = typeof sw.tasks === "object" && sw.tasks ? sw.tasks : { all: sw.tasks || 0 };
  return { sw, a, now, fresh, fc, regionName, helpers, pages, macs, leasesOf, colourOf, tasks, steps: a.checklist || [] };
}

// A checklist line's state.
const finished = (st) => (st.left != null ? st.left === 0 : st.total != null && st.done >= st.total && st.total > 0);
const stepOf = (id) => String(id || "").split(" ")[0];

// The share of the build's work done: each checklist line's targets done, at its step's time a target
// (the forecast's, else a first guess), against what's left (the forecast's work).
const GUESS = { terrain: 900, slope: 400, trees: 600, unit: 400, pack: 35, lo: 25 };
function shareDone(m) {
  if (!m.fc) return null;
  const per = (step) => {
    const f = m.fc.steps.find((s) => s.step === step);
    return f && f.left ? f.work_s / f.left : GUESS[step] || 60;
  };
  let doneS = 0;
  for (const st of m.steps) {
    if (st.total == null || !st.done) continue;
    const ss = st.steps || [];
    doneS += st.done * (ss.reduce((t, s) => t + per(s), 0) / Math.max(1, ss.length));
  }
  const leftS = m.fc.steps.reduce((t, s) => t + s.work_s, 0);
  return doneS + leftS > 0 ? doneS / (doneS + leftS) : 1;
}

// ---- The verdict --------------------------------------------------------------------------------
function machineState(st, fresh, paused) {
  if (!fresh) return ["out of touch", "bad"];
  const j = st.job;
  if (j?.paused) return ["frozen", "warn"];
  if (j?.pausing) return ["stopping", "warn"];
  if (j) return ["building", "run"];
  if (paused) return ["paused", "warn"];
  return ["idle", ""];
}

function alerts(m) {
  const out = [];
  const { a, now } = m;
  if (!a.host) return [{ cls: "bad", text: "No word from the build Mac yet", to: "machines" }];
  if (!m.fresh(a.beat)) out.push({ cls: "bad", text: `Build Mac out of touch for ${dur(now - a.beat)}`, to: "machines" });
  for (const x of m.macs) {
    const st = x.status, r = st.resources || {}, c = st.conditions || {};
    const short = x.role === "build Mac" ? "Build Mac" : x.name;
    if (x.role !== "build Mac" && !x.fresh) out.push({ cls: "warn", text: `${short} out of touch for ${dur(now - st.beat)}`, to: "machines" });
    const p = st.job?.progress;
    if (p?.moved_at && !st.job.paused && now - p.moved_at >= STUCK_S) out.push({ cls: "warn", text: `${short}'s job hasn't moved on for ${dur(now - p.moved_at)}`, to: "machines" });
    if (c.ac === false) out.push({ cls: c.battery != null && c.battery < 40 ? "warn" : "info", text: `${short} on battery${c.battery != null ? ` (${c.battery}%; work stops at 30%)` : ""}`, to: "machines" });
    if (c.nas === false) out.push({ cls: "bad", text: `${short} can't reach the NAS`, to: "machines" });
    else if (c.home === false) out.push({ cls: "info", text: `${short} reaches the NAS through Tailscale (slowly)`, to: "machines" });
    if (r.disk_free_gb != null && r.disk_free_gb < 20) out.push({ cls: r.disk_free_gb < 10 ? "bad" : "warn", text: `${short}: ${r.disk_free_gb} GB free on its disk`, to: "machines" });
    if (x.role !== "build Mac" && st.app && a.app && st.app !== a.app) out.push({ cls: "warn", text: `${short} runs another app (${st.app})`, to: "machines" });
  }
  // Failures in the last six hours (the build Mac's own jobs; the workers' are in the activity).
  const failed = (a.recent || []).filter((d) => !d.ok && now - d.ended < 6 * 3600);
  if (failed.length) out.push({ cls: "bad", text: `${plural(failed.length, "job")} failed in the last 6 h`, to: "activity" });
  for (const w of a.waiting || []) {
    if (/held for review/.test(w.why)) out.push({ cls: "warn", text: "Publishing held for review", to: "road" });
    else if (/failed \d+ times?/.test(w.why)) out.push({ cls: "warn", text: `${w.what}: waiting out a failure`, to: "details" });
  }
  for (const p of m.pages) {
    if (p.bad) out.push({ cls: "bad", text: `${p.label}: a result differed; it gets no more work`, to: "machines" });
    else if (p.visible === false && p.seen_s < 300) out.push({ cls: "warn", text: `${p.label} is in the background (iPhones and iPads stop it)`, to: "machines" });
  }
  if ((a.bad_recipes || []).length) out.push({ cls: "warn", text: `${plural(a.bad_recipes.length, "region recipe")} doesn't read`, to: "details" });
  return out;
}

function verdictText(m) {
  const { a, sw, now, fc } = m;
  const parts = [];
  if (!a.host) return [h("b", null, "No word from the build Mac yet.")];
  if (sw.pause) {
    const stopping = m.macs.filter((x) => x.status.job?.pausing).length;
    parts.push(h("b", null, "Paused"), ` by ${sw.pause.by}, ${ago(now, sw.pause.at)}`);
    parts.push(sw.pause.mode === "freeze" ? " · every job frozen where it was" : stopping ? ` · ${plural(stopping, "job")} finishing what ${stopping === 1 ? "it's" : "they're"} on` : " · nothing running");
  } else if (!m.fresh(a.beat)) {
    parts.push(h("b", null, "Out of touch"), ` · the build Mac last said something ${ago(now, a.beat)} (asleep, off, or its agent stopped)`);
  } else {
    const working = m.macs.filter((x) => x.fresh && x.status.job).length + m.pages.filter((p) => (sw.leases || []).some((l) => l.worker === p.name)).length;
    parts.push(h("b", null, working ? "Building" : "Idle"), working ? ` on ${plural(working, "machine")}` : "");
  }
  if (fc?.done_at) {
    const r = fc.range;
    parts.push(" · done ≈ ", h("b", null, clock(fc.done_at)), r ? ` (${clock(r[0])}–${clock(r[1])})` : "");
  } else if (fc && !fc.done_at) parts.push(" · no finish in sight: there's work no machine can do");
  const next = fc?.rounds?.find((r) => r.regions.length);
  if (next) {
    const names = next.regions.slice(0, 2).map((id) => m.regionName[id] || id).join(", ");
    parts.push(" · next on the map ≈ ", h("b", null, clock(next.at)), `: ${names}${next.regions.length > 2 ? ` +${next.regions.length - 2}` : ""}`);
  }
  return parts;
}

// ---- The overview -------------------------------------------------------------------------------
function overview(m) {
  const { a, fc, now, steps } = m;
  const share = shareDone(m);
  const unitsLine = steps.find((st) => (st.steps || []).includes("unit"));
  const big = share != null ? `${Math.floor(share * 100)}%` : unitsLine?.total ? `${Math.floor((unitsLine.done / unitsLine.total) * 100)}%` : "–";
  const workLeft = fc ? fc.steps.reduce((t, s) => t + s.work_s, 0) : 0;
  const left = h("div", null,
    h("div", "big", big, h("small", null, share != null ? " of the work done" : unitsLine?.total ? " of the areas built" : "")),
    h("div", "small dim", fc ? `${dur(workLeft)} of work left at the build Mac's pace; ${fc.done_at ? `with every machine, done ≈ ${clock(fc.done_at)}, if the Macs keep going (awake, on mains or above 30%, reaching the NAS)` : "no finish in sight"}` : "No forecast yet (the build Mac makes one with each plan)"),
    fc ? h("div", "small dim", `${Math.round((fc.measured || 0) * 100)}% of that time measured, the rest estimated · forecast ${ago(now, fc.at)}`) : null,
  );
  // The steps as a strip, the one under way outlined.
  const nowStep = stepOf(a.job?.id);
  const strip = h("div", "strip", steps.map((st) => {
    const done = finished(st);
    const i = h("i", { class: `${done ? "done" : ""} ${(st.steps || []).includes(nowStep) ? "now" : ""}`, title: `${st.what}: ${st.total != null ? `${n(st.done)} of ${n(st.total)}` : st.left != null ? (st.left ? `${st.left} left` : "done") : "to come"}` });
    if (!done && st.total) { const b = h("b"); b.style.width = `${Math.round((st.done / st.total) * 100)}%`; i.append(b); }
    return i;
  }));
  const fin = steps.filter(finished).length;
  const stepsLine = h("div", "small dim", steps.length ? `${fin} of ${steps.length} steps done${nowStep ? ` · now: ${steps.find((st) => (st.steps || []).includes(nowStep))?.what || stepName(nowStep)}` : ""}` : "");
  // The numbers that matter most.
  const kpi = (k, v, s) => h("div", "kpi", h("div", "k", k), h("div", "v", v), s ? h("div", "s", s) : null);
  const line = (step) => steps.find((st) => (st.steps || []).includes(step));
  const frac = (st) => (st && st.total != null ? `${n(st.done)} of ${n(st.total)}` : st && st.left != null ? (st.left ? `${st.left} left` : "done") : "–");
  const regs = fc?.regions || [];
  const onMap = regs.filter((r) => r.on_map === true && !Object.keys(r.left).length).length;
  const asWas = regs.filter((r) => r.on_map === false || (r.on_map === true && Object.keys(r.left).length)).length;
  const readyNow = regs.filter((r) => !Object.keys(r.left).length && r.on_map !== true).length;
  const lastCat = (a.recent || []).find((d) => stepOf(d.id) === "catalog" && d.ok);
  const nextRound = fc?.rounds?.[0];
  const busy = m.macs.filter((x) => x.fresh && x.status.job).length;
  const kpis = h("div", "kpis",
    kpi("Areas built", frac(line("unit")), fc ? `${n(fc.steps.find((s) => s.step === "unit")?.left || 0)} to build` : null),
    kpi("Regions on the map", regs.length ? `${onMap} of ${regs.length}` : "–", [asWas ? `${asWas} more as they were` : "", readyNow ? `${readyNow} done, out with the next round` : ""].filter(Boolean).join(" · ") || null),
    kpi("Terrain · slope", `${frac(line("terrain"))} · ${frac(line("slope"))}`),
    kpi("Tree cover", frac(line("trees"))),
    kpi("Map tiles", frac(line("pack"))),
    kpi("Map last updated", lastCat ? ago(now, lastCat.ended) : "–", nextRound ? `next ≈ ${clock(nextRound.at)}` : null),
    kpi("Machines", `${busy} of ${m.macs.length} Macs working`, m.pages.length ? `and ${plural(m.pages.length, "page")}` : null),
  );
  return h("div", "overview", h("div", "box", left, strip, stepsLine), kpis);
}

// ---- Machines -----------------------------------------------------------------------------------
// A job's progress in words: "40% · 2.4 of 6 areas · about 12 min left" (one item alone, its share).
function progressText(p, j, now) {
  const pct = Math.floor((p.done / p.total) * 100);
  // (One item whose share is said: "the planet filtered"; else "0.4 of 1 area", its noun singular.)
  const unit = p.total === 1 ? p.unit.replace(/^(\w+?)s\b/, "$1") : p.unit;
  const amount = p.total === 1 && /^the /.test(p.unit) ? p.unit : `${Number.isInteger(p.done) || p.total > 100 ? n(Math.floor(p.done)) : (Math.floor(p.done * 10) / 10).toFixed(1)} of ${n(p.total)} ${unit}`;
  const eta = p.eta_s != null && !j.paused ? ` · about ${dur(p.eta_s)} left (≈ ${clock(now + p.eta_s)})` : "";
  return `${pct}% · ${amount}${eta}`;
}

function jobBlock(j, now) {
  const p = j.progress;
  const box = h("div", "job", h("div", "what", j.what));
  if (j.parts?.length && j.part != null) {
    box.append(h("div", "parts", j.parts.map((_, i) => h("i", { class: i < j.part ? "done" : i === j.part ? "now" : "", title: j.parts[i] }))));
    box.append(h("div", "sub", `Part ${j.part + 1} of ${j.parts.length}: ${j.parts[j.part]}`));
  }
  if (p && p.total > 0) {
    box.append(pbar(p.done / p.total, j.paused ? "warn" : ""));
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
  const next = (m.fc?.next?.[name] || []).filter((x) => x.from > m.now - 60);
  if (!next.length) return null;
  const say = (x) => `${stepName(x.step)}: ${targets(x.step, x.targets.length)} (${clock(x.from)}–${clock(x.until)})`;
  return h("div", "next", h("b", null, "Next: "), next.slice(0, 3).map(say).join(" · then "));
}

function facts(st, x, m) {
  const c = st.conditions || {}, r = st.resources || {};
  const out = [];
  out.push(c.ac === false ? chip(`battery ${c.battery ?? "?"}%`, c.battery != null && c.battery < 40 ? "warn" : "") : chip(c.battery != null ? `mains · ${c.battery}%` : "mains"));
  out.push(c.nas === false ? chip("NAS unreachable", "bad") : chip(`NAS${r.nas_ms != null ? ` ${r.nas_ms} ms` : ""}${c.home === false ? " via Tailscale" : ""}${r.nas_free_tb != null && x.role === "build Mac" ? ` · ${r.nas_free_tb} TB free` : ""}`, c.home === false ? "warn" : "", "How long the NAS took to answer, and its free space"));
  if (r.disk_free_gb != null) out.push(chip(`${r.disk_free_gb} GB free${r.cache_gb != null ? ` (+${r.cache_gb} GB cache)` : ""}`, r.disk_free_gb < 10 ? "bad" : r.disk_free_gb < 20 ? "warn" : "", "Free on its disk; the caches the agent may drop to make room"));
  if (r.mem_gb) out.push(chip(`${r.mem_gb} GB memory${r.mem_free_pct != null ? `, ${r.mem_free_pct}% free` : ""}`, r.mem_free_pct != null && r.mem_free_pct < 10 ? "warn" : ""));
  if (r.load1 != null) out.push(chip(`load ${r.load1} on ${r.cores} cores`));
  out.push(chip(c.idle_s != null && c.idle_s < 300 ? "in use (half the cores)" : "not in use", "", "While someone uses the Mac, its jobs get half its cores"));
  if (st.app) out.push(chip(`app ${st.app}`, x.role !== "build Mac" && m.a.app && st.app !== m.a.app ? "warn" : ""));
  const sp = m.fc?.speed?.[x.name];
  if (sp != null && x.role !== "build Mac") out.push(chip(`${Math.round(sp * 100)}% of the build Mac's pace`));
  return h("div", "facts", out);
}

// The last day by the hour: how busy it was (a bar an hour; several, the pages, together, up to the
// hour), paused hours shaded.
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
  const said = Object.entries(today).sort((x, y) => y[1] - x[1]).map(([s, k]) => did(s, k)).join(", ");
  return [h("div", "spark", bars), h("div", "sub", `Last 24 h: ${said || "nothing finished"}`)];
}

function fitText(w) {
  if (!w?.fit?.length) return null;
  const parts = w.fit.filter((f) => f.offered).map((f) => {
    const bits = [`${n(f.fits)} of ${n(f.offered)} ${stepName(f.step).toLowerCase()} it can take`];
    if (f.held) bits.push(`${n(f.held)} with others`);
    if (f.too_big) bits.push(`${n(f.too_big)} too big for its ${n(Math.round(w.mem_mb / 102.4) / 10)} GB`);
    if (f.kept_from) bits.push(`${n(f.kept_from)} kept from it after failing`);
    return bits.join(", ");
  });
  return parts.length ? h("div", "sub", `Offered: ${parts.join("; ")}`) : null;
}

function machineCard(m, x) {
  const st = x.status, now = m.now;
  const [state, cls] = machineState(st, x.fresh, m.sw.pause);
  const card = h("div", { class: `mc ${x.fresh ? "" : "off"}`, style: { borderLeft: `3px solid ${x.colour}` } },
    h("div", "top", h("span", "name", x.name), h("span", "role", x.role), chip(state, cls), h("span", "right", `heard from ${ago(now, st.beat)}`)));
  if (st.job) card.append(jobBlock(st.job, now));
  else {
    // Why it isn't building.
    const why = (st.waiting || []).filter((w) => !w.step || x.role !== "build Mac").slice(0, 2);
    card.append(h("div", "job", h("div", "what dim", m.sw.pause ? "Paused with the build" : why.length ? "Waiting" : "Nothing to build now")));
    for (const w of why) card.append(h("div", "sub", x.role === "build Mac" ? `${w.what}: ${w.why}` : w.why));
  }
  const more = [nextBlock(m, x.name), x.role === "helper" ? fitText(x.worker) : null, facts(st, x, m), ...(spark(m, x.name) || [])];
  card.append(...more.filter(Boolean));
  return card;
}

function pagesCard(m) {
  if (!m.pages.length) return null;
  const card = h("div", { class: "mc pages", style: { borderLeft: "3px solid #6cc28a" } },
    h("div", "top", h("span", "name", "Pages"), h("span", "role", "browsers doing areas' last steps"), chip(`${m.pages.length}`, "ok")));
  for (const p of m.pages) {
    const leases = m.leasesOf(p.name);
    const st = p.bad ? chip("stopped", "bad") : p.seen_s > 120 ? chip("away", "") : leases.length ? chip(`${leases.length} running`, "run") : chip("waiting", "");
    const left = h("div", null, h("div", null, p.label, " ", st, p.visible === false ? chip("in the background", "warn") : null),
      h("div", "sub", leases.length ? leases.map((l) => `${l.progress || "task"}${l.frac != null ? ` (${Math.round(l.frac * 100)}%)` : ""}`).join(" · ") : p.what));
    const right = h("div", "sub", `${n(p.done)} done${p.checked ? ` (${n(p.checked)} checked against the build Mac's)` : ""}${p.failed ? ` · ${p.failed} failed` : ""}${p.mem_mb ? ` · ${n(p.mem_mb)} MB` : ""} · ${dur(p.seen_s)} ago`);
    card.append(h("div", "pg", left, right));
  }
  const t = m.tasks;
  if (t.offered != null) card.append(h("div", "sub", `Tasks now: ${t.offered || 0} offered, ${t.leased || 0} running, ${t.done || 0} done, ${t.failed || 0} failed${!t.offered && !t.leased ? " — the build Mac offers them only while it builds areas, a few at a time" : ""}`));
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

function schedule(m) {
  const fc = m.fc;
  if (!fc?.lanes || !Object.keys(fc.lanes).length) return h("div", "small dim", "No schedule yet (the forecast comes with the build Mac's next plan).");
  const t0 = m.now;
  const t1 = Math.max(fc.done_at || 0, ...Object.values(fc.lanes).flat().map((l) => l.until), t0 + 3600);
  const span = t1 - t0;
  const x = (t) => `${(((Math.max(t0, Math.min(t1, t)) - t0) / span) * 100).toFixed(3)}%`;
  const names = m.macs.map((mm) => mm.name).filter((nm) => fc.lanes[nm]).concat(Object.keys(fc.lanes).filter((nm) => !m.macs.some((mm) => mm.name === nm)));
  const lanes = h("div", "lanes");
  const steps = new Set();
  for (const nm of names) {
    const track = h("div", "track");
    for (const l of fc.lanes[nm]) {
      if (l.until < t0) continue;
      steps.add(l.step);
      const say = l.step === "round" ? `Publishing round (${plural(l.n, "region")}) ${clock(l.from)}–${clock(l.until)}` : `${stepName(l.step)}: ${targets(l.step, l.n)}, ${clock(l.from)}–${clock(l.until)}`;
      const seg = h("i", { class: l.step === "round" ? "round" : "", onmousemove: (e) => showTip(e, `${nm}: ${say}`), onmouseleave: hideTip, onclick: (e) => showTip(e, `${nm}: ${say}`) });
      Object.assign(seg.style, { left: x(l.from), width: `calc(${x(l.until)} - ${x(l.from)})`, background: stepColour(l.step) });
      track.append(seg);
    }
    lanes.append(h("div", { class: "ln", title: nm }, nm), track);
  }
  // Ticks: every 1, 2, 3, 6, 12 or 24 hours, on the hour.
  const step = [1, 2, 3, 6, 12, 24, 48].find((hh) => span / (hh * 3600) <= 8) || 48;
  const axis = h("div", "axis");
  for (let t = Math.ceil(t0 / (step * 3600)) * step * 3600; t < t1; t += step * 3600) {
    const d = new Date(t * 1000);
    const label = d.getHours() === 0 || step >= 24 ? `${DAYS[d.getDay()]}` : `${String(d.getHours()).padStart(2, "0")}:00`;
    axis.append(h("span", { style: { left: x(t) } }, label));
  }
  lanes.append(h("div"), axis);
  // Each round of publishing, a mark across the lanes, the regions it adds when pointed at.
  const marks = h("div", "rounds");
  for (const r of fc.rounds || []) {
    if (r.at < t0 || r.at > t1) continue;
    const say = `${r.last ? "Last round" : "Round"} ≈ ${clock(r.at)}: ${r.regions.length ? r.regions.slice(0, 6).map((id) => m.regionName[id] || id).join(", ") + (r.regions.length > 6 ? ` and ${r.regions.length - 6} more` : "") : "the rest"}`;
    marks.append(h("i", { style: { left: x(r.at) }, onmousemove: (e) => showTip(e, say), onmouseleave: hideTip, onclick: (e) => showTip(e, say) }));
  }
  const legend = h("div", "legend", [...steps].filter((s) => s !== "round").map((s) => h("span", null, h("i", { style: { background: stepColour(s) } }), stepName(s))), h("span", null, h("i", { class: "rmark" }), "a round of publishing"));
  return h("div", "sched", h("div", "lanewrap", lanes, marks), legend);
}

function stepsTable(m) {
  const { steps, fc, a } = m;
  if (!steps.length) return h("div", "small dim", "No word from the build Mac yet.");
  const nowStep = stepOf(a.job?.id);
  const grid = h("div", "steps2");
  for (const st of steps) {
    const done = finished(st);
    const here = !done && (st.steps || []).includes(nowStep);
    const busy = !done && m.helpers.some((x) => (st.steps || []).includes(stepOf(x.job?.id)));
    const f = fc ? fc.steps.filter((s) => (st.steps || []).includes(s.step)) : [];
    const doneAt = f.length && f.every((s) => s.done_at) ? Math.max(...f.map((s) => s.done_at)) : null;
    const work = f.reduce((t, s) => t + s.work_s, 0);
    const count = st.total != null ? `${n(st.done)} of ${n(st.total)} ${st.unit || ""}` : st.left != null ? (st.left ? `${st.left} left` : "done") : "to come";
    grid.append(h("span", `mark ${done ? "ok" : here || busy ? "run" : ""}`, done ? "✓" : here || busy ? "▸" : "○"),
      h("span", null, st.what, st.shared ? h("span", { class: "dim", title: `Helpers may take part: ${st.shared}` }, " ⇄") : null),
      h("span", "n", count));
    if (!done && st.total) grid.append(pbar(st.done / st.total, here || busy ? "" : "dimbar"));
    const bits = [];
    if (!done && work) bits.push(`${dur(work)} of work`);
    if (!done && doneAt) bits.push(`done ≈ ${clock(doneAt)}`);
    if (!done && st.note) bits.push(st.note);
    if (!done && st.next?.length > 1) bits.push(`then ${st.next.slice(1, 3).join("; ")}`);
    if (bits.length) grid.append(h("span", "note", bits.join(" · ")));
  }
  return grid;
}

// The regions: their order to the map with the rounds between, or by name, or by what's left; each
// its state, what it has left, when it's done and when it's on the map.
const ui = { order: "map", filter: "", all: false, feed: "all" };
function regionsList(m, rerender) {
  const fc = m.fc, built = m.a.built || {};
  const regs = (fc?.regions || []).map((r) => ({ ...r, name: m.regionName[r.id] || r.id, built: built[r.id] }));
  if (!regs.length) {
    // (An older build Mac: its areas built alone.)
    const ids = Object.keys(built);
    if (!ids.length) return h("div", "small dim", "No regions yet.");
    return h("div", "regs", ids.map((id) => h("div", "rg", h("span", "nm", m.regionName[id] || id), h("span", "when", `${built[id].built}/${built[id].total} areas`))));
  }
  const bar = h("div", "regbar",
    h("input", { placeholder: `Find one of ${regs.length} regions…`, value: ui.filter, oninput: (e) => { ui.filter = e.target.value; rerender(); }, autocapitalize: "off", spellcheck: "false" }),
    h("div", "seg", [["map", "To the map"], ["name", "A–Z"], ["left", "Most left"]].map(([k, t]) => h("button", { class: ui.order === k ? "on" : "", onclick: () => { ui.order = k; rerender(); } }, t))));
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
  let lastRound = null;
  for (const r of list) {
    const lft = leftOf(r);
    if (ui.order === "map" && r.map_at && r.map_at !== lastRound) {
      const round = fc.rounds.find((x) => x.at === r.map_at);
      out.append(h("div", "round", `${round?.last ? "Last round" : "Round"} ≈ ${clock(r.map_at)}${round ? ` · ${plural(round.regions.length, "region")}` : ""}`));
      lastRound = r.map_at;
    } else if (ui.order === "map" && !r.map_at && lastRound !== "on") {
      out.append(h("div", "round", "On the map"));
      lastRound = "on";
    }
    let state, cls = "";
    if (!lft && r.on_map === true) [state, cls] = ["on the map", "ok"];
    else if (!lft) [state, cls] = ["done; out with the next round", "ok"];
    else if (r.rank === 0) [state, cls] = ["building now", "run"];
    else [state, cls] = [`#${r.rank + 1} in line`, ""];
    if (r.on_map === false) state += " · on the map as it was";
    const when = lft ? `done ≈ ${clock(r.ready_at)} · on the map ≈ ${clock(r.map_at)}` : r.map_at ? `on the map ≈ ${clock(r.map_at)}` : "";
    const order = ["unit", "terrain", "slope", "trees"];
    const words = { unit: ["area", "areas"], terrain: ["terrain area", "terrain areas"], slope: ["slope area", "slope areas"], trees: ["tree-cover tile", "tree-cover tiles"] };
    const leftWords = order.filter((s) => r.left[s]).map((s) => plural(r.left[s], ...words[s])).join(", ");
    const tot = r.built?.total || 0;
    const stack = h("div", "stack");
    if (tot) {
      const seg = (k, c) => { const i = h("i"); Object.assign(i.style, { width: `${(k / tot) * 100}%`, background: c }); return i; };
      stack.append(seg(r.built.built, "var(--ok)"), seg(r.left.unit || 0, "#2c4566"));
    }
    out.append(h("div", { class: `rg ${!lft && r.on_map === true ? "done" : ""}` },
      h("span", "nm", r.name, " ", chip(state, cls)), h("span", "when", when),
      h("div", "left", tot ? stack : null, [tot ? `${r.built.built} of ${tot} areas built` : "", leftWords ? `left: ${leftWords}` : ""].filter(Boolean).join(" · "))));
  }
  const more = !q && total > list.length ? h("button", { class: "quiet more", onclick: () => { ui.all = true; rerender(); } }, `Show all ${total}`) : null;
  return h("div", null, bar, out, more);
}

function publishing(m) {
  const { a, fc, now } = m;
  const lastCat = (a.recent || []).find((d) => stepOf(d.id) === "catalog" && d.ok);
  const lines = [];
  lines.push(h("div", null, "Last update: ", lastCat ? h("b", null, `${clock(lastCat.ended)} (${ago(now, lastCat.ended)})`) : "none yet"));
  const next = fc?.rounds?.[0];
  if (next) lines.push(h("div", null, "Next: ", h("b", null, `≈ ${clock(next.at)}`), next.regions.length ? ` with ${next.regions.slice(0, 4).map((id) => m.regionName[id] || id).join(", ")}${next.regions.length > 4 ? ` and ${next.regions.length - 4} more` : ""}` : ""));
  const waits = (a.waiting || []).filter((w) => w.step === "catalog" || /publish/i.test(w.what));
  for (const w of waits) lines.push(h("div", "small warn", w.why));
  lines.push(h("div", "small dim", "A round goes out when a region is done, at most hourly while areas are left; after the last, at once."));
  if (fc?.rounds?.length > 1) lines.push(h("div", "small dim", `${plural(fc.rounds.length, "round")} to come; the last ≈ ${clock(fc.rounds[fc.rounds.length - 1].at)}`));
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
    const col = h("div", { class: r.paused_s > 1800 ? "paused" : "", title: `${clock(r.t)}–${clock(r.t + 3600)}: ${who.map((w) => `${w} ${ui.metric === "busy" ? `${Math.round(val(r, w))} min busy` : `${val(r, w)} areas`}`).join(", ")}` });
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
  // The day's totals by machine.
  const sum = {};
  for (const r of rows) for (const [w, s] of Object.entries(r.done || {})) for (const [st, k] of Object.entries(s)) { sum[w] ??= {}; sum[w][st] = (sum[w][st] || 0) + k; }
  const totals = Object.entries(sum).map(([w, s]) => `${w}: ${Object.entries(s).sort((x, y) => y[1] - x[1]).map(([st, k]) => did(st, k)).join(", ")}`);
  const lastHour = rows.slice(-3).reduce((t, r) => t + Object.values(r.done || {}).reduce((u, s) => u + (s.unit || 0), 0), 0) / 3;
  return h("div", null,
    h("div", "seg", [["areas", "Areas an hour"], ["busy", "Busy minutes"]].map(([k, t]) => h("button", { class: (ui.metric || "areas") === k ? "on" : "", onclick: () => { ui.metric = k; render(); } }, t))),
    h("div", { style: { marginTop: "8px" } }, chart, axis),
    h("div", "small dim", `${lastHour.toFixed(1)} areas an hour lately · last 24 h: ${totals.join(" · ") || "nothing finished"}`),
    h("div", "legend", who.map((w) => h("span", null, h("i", { style: { background: m.colourOf(w) } }), w))));
}

// What happened, in words: an event (or a run of like ones, together).
function eventText(m, e, k = 1) {
  const who = e.worker || "";
  const nm = (id) => m.regionName[id] || id;
  const t = e.targets || [];
  const what = e.step ? `${stepName(e.step)}${t.length ? ` (${t.length === 1 ? t[0] : targets(e.step, t.length)})` : ""}` : "";
  switch (e.kind) {
    case "start": return [`${who} began ${e.note || what}`, ""];
    case "end": return e.ok ? [`${who} finished ${what} in ${dur(e.secs || 0)}`, ""] : [`${who}'s ${what} ${e.note || "stopped"}${t.length ? ` (${t.length} done and kept)` : ""}`, /fail/.test(e.note) ? "bad" : "warn"];
    case "lease": return [`${who} took ${what}`, "dim"];
    case "done": return [`${who} handed back ${what}, ${dur(e.secs || 0)} after taking it`, ""];
    case "fail": return [`${who}'s ${what} ${/^stopped/.test(e.note) ? e.note : `failed: ${e.note}`}`, /^stopped/.test(e.note) ? "warn" : "bad"];
    case "lapse": return [`${who} went quiet: its lease on ${e.note || what} lapsed and the work is offered again`, "warn"];
    case "task": return [k > 1 ? `${who} did ${k} areas' last steps` : `${who} did an area's last steps (${t[0] || ""}) in ${dur(e.secs || 0)}`, "dim"];
    case "task-fail": return [`${who}: a task failed: ${e.note}`, "bad"];
    case "catalog": return [t.length ? `The map got ${t.length > 3 ? `${t.length} regions` : t.map(nm).join(", ")}` : "The map was updated", "ok"];
    case "pause": return [`The build was paused by ${e.note}`, "warn"];
    case "resume": return ["The build went on", "ok"];
    case "worker": return [`${who}${e.note && e.note !== who ? ` (${e.note})` : ""} joined`, "run"];
    case "agent": return [`${who}'s agent started (${e.note})`, "run"];
    case "conditions": return [`${who}: ${e.note}`, /doesn't answer|battery|away/.test(e.note) ? "warn" : ""];
    default: return [`${e.kind} ${who} ${what}`, ""];
  }
}

const FILTERS = { all: () => true, problems: (e) => e.ok === false || e.kind === "lapse" || e.kind === "task-fail", publishing: (e) => e.kind === "catalog" || (e.kind === "end" && e.step === "catalog"), pauses: (e) => ["pause", "resume", "conditions", "agent"].includes(e.kind) };

function feed(m, events, seen) {
  const shown = events.filter(FILTERS[ui.feed] || FILTERS.all).filter((e) => !(e.kind === "start" && ui.feed === "all" && events.some((x) => x.kind === "end" && x.worker === e.worker && x.seq > e.seq)));
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
    out.append(h("div", { class: seen != null && r.e.seq > seen ? "new" : "" }, h("span", "t", new Date(r.e.t * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })), h("span", cls, text)));
  }
  if (!rows.length) out.append(h("div", "dim", events.length ? "Nothing of that kind lately." : "Nothing yet."));
  return out;
}

// What happened since this page last showed it: a sentence.
function sinceLine(m, events, seen) {
  if (seen == null) return null;
  const fresh = events.filter((e) => e.seq > seen);
  if (!fresh.length) return h("div", "since dim", "Nothing new since you last looked.");
  const areas = {};
  let failures = 0, rounds = [];
  for (const e of fresh) {
    if ((e.kind === "end" || e.kind === "done") && e.step === "unit" && e.ok !== false) areas[e.worker] = (areas[e.worker] || 0) + (e.targets || []).length;
    if (e.ok === false && !/^stopped|paused/.test(e.note || "")) failures++;
    if (e.kind === "catalog") rounds.push(e);
  }
  const total = Object.values(areas).reduce((t, k) => t + k, 0);
  const bits = [];
  if (total) bits.push(`${plural(total, "area")} built (${Object.entries(areas).map(([w, k]) => `${k} by ${w}`).join(", ")})`);
  if (rounds.length) bits.push(`${plural(rounds.length, "map update")}${rounds.some((r) => r.targets?.length) ? `: ${rounds.flatMap((r) => r.targets || []).slice(0, 4).map((id) => m.regionName[id] || id).join(", ")}` : ""}`);
  if (failures) bits.push(h("span", "bad", plural(failures, "failure")));
  const first = fresh[0];
  return h("div", "since", `Since ${clock(first.t)}: `, ...bits.flatMap((b, i) => (i ? [" · ", b] : [b])), bits.length ? "" : `${plural(fresh.length, "event")}`);
}

// ---- Details ------------------------------------------------------------------------------------
function details(m) {
  const { sw, a, now } = m;
  const box = h("div", "box");
  const table = (head, rows) => h("table", "t", h("tr", null, head.map((c) => h("th", null, c))), rows.map((r) => h("tr", null, r.map((c) => h("td", null, c)))));
  const leases = sw.leases || [];
  box.append(h("h3", null, "Leases", h("span", null, "who holds what; a worker that stops beating gives its work back")),
    leases.length ? table(["Worker", "Work", "For", "Last beat", "Lapses in", "Says"], leases.map((l) => [l.worker, l.what.length > 60 ? `${l.what.slice(0, 60)}…` : l.what, dur(l.for_s), l.beat_s != null ? `${dur(l.beat_s)} ago` : "–", l.lapses_in_s != null ? dur(l.lapses_in_s) : "–", l.progress || ""])) : h("div", "small dim", "None."));
  const waits = a.waiting || [];
  box.append(h("h3", null, "Waiting", h("span", null, "what can't run yet, and why")), waits.length ? table(["What", "Why"], waits.map((w) => [w.what, w.why])) : h("div", "small dim", "Nothing."));
  const recent = a.recent || [];
  box.append(h("h3", null, "The build Mac's last jobs"), recent.length ? table(["", "Job", "Took", "Ended", "Note"], recent.slice(0, 20).map((d) => [d.ok ? "✓" : "✗", d.what, dur(d.secs), clock(d.ended), (d.note || "").split("\n").slice(-2).join(" ").slice(0, 160)])) : h("div", "small dim", "None yet."));
  if ((a.bad_recipes || []).length) box.append(h("h3", null, "Region recipes that don't read"), table(["File", "Problem"], a.bad_recipes));
  if (a.job?.tail) box.append(h("h3", null, "The build Mac's job's last lines"), h("pre", { class: "small dim", style: { whiteSpace: "pre-wrap", margin: 0 } }, a.job.tail));
  const workers = (sw.workers || []).filter((w) => w.seen_s < 3600);
  if (workers.length) box.append(h("h3", null, "Workers heard from"), table(["Name", "Kind", "Doing", "Spares", "Done", "Failed", "Seen"], workers.map((w) => [w.label || w.name, w.kind, (w.what || "").slice(0, 80), w.mem_mb ? `${n(w.mem_mb)} MB` : "", n(w.done), n(w.failed), `${dur(w.seen_s)} ago`])));
  box.append(h("div", "fresh", `Updated ${clock(now)} · the build Mac's heartbeat ${a.beat ? ago(now, a.beat) : "–"}${a.started ? `, its agent running since ${clock(a.started)} (app ${a.app})` : ""}${m.fc ? ` · forecast ${ago(now, m.fc.at)}` : ""} · history #${sw.seq ?? "–"}`));
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
    pausing ? h("button", { class: "primary", onclick: () => ctx.ask(null) }, "Resume")
      : [h("button", { onclick: () => ctx.ask({ mode: "drain" }), title: "Every Mac's job stops at its next safe point; nothing new starts" }, "Pause"),
        h("button", { class: "quiet", onclick: () => confirm("Freeze every Mac's job where it is now? (It goes on from there when resumed.)") && ctx.ask({ mode: "freeze" }), title: "Freeze every job where it is, at once" }, "Pause now")]);
  if (!ctx.token) ctl.replaceChildren();
  const verdict = section("d-verdict", "verdict", null, null,
    h("div", "verdict", h("div", "say", verdictText(m)), ctl),
    h("div", "alerts", al.map((x) => h("span", { class: `alert ${x.cls}`, onclick: () => document.getElementById(x.to)?.scrollIntoView({ behavior: "smooth", block: "start" }) }, x.text))));
  const machines = section("d-machines", "machines", "Machines", `${m.macs.filter((x) => x.fresh).length} Macs${m.pages.length ? `, ${plural(m.pages.length, "page")}` : ""}`,
    h("div", "machines", m.macs.map((x) => machineCard(m, x)), pagesCard(m)));
  const road = section("d-road", "road", "Road to done", m.fc?.done_at ? `done ≈ ${clock(m.fc.done_at)}` : "",
    h("div", "road",
      h("div", "box", h("h3", null, "Schedule", h("span", null, "each machine's work from now to the end")), schedule(m)),
      h("div", "box", h("h3", null, "Map updates"), publishing(m)),
      h("div", "box", h("h3", null, "Steps", h("span", null, "⇄ helpers may take part")), stepsTable(m)),
      h("div", "box", h("h3", null, "Regions", h("span", null, m.fc ? "built one at a time: those the map lacks first, then those with the fewest areas left" : "")), regionsList(m, render))));
  const activity = section("d-activity", "activity", "Activity", "the last day",
    h("div", "box", hours(m),
      h("div", "feedbar", h("div", "seg", [["all", "All"], ["problems", "Problems"], ["publishing", "Map updates"], ["pauses", "Pauses & conditions"]].map(([k, t]) => h("button", { class: ui.feed === k ? "on" : "", onclick: () => { ui.feed = k; render(); } }, t)))),
      sinceLine(m, events, seenSeq), feed(m, events, seenSeq)));
  const scrollY = window.scrollY;
  const focused = document.activeElement?.tagName === "INPUT" && root.contains(document.activeElement);
  const pos = focused ? document.activeElement.selectionStart : null;
  root.replaceChildren(verdict, section("d-overview", "overview", "Overview", null, overview(m)), machines, road, activity, section("d-details", "details", null, null, details(m)));
  if (focused) { const i = root.querySelector(".regbar input"); if (i) { i.focus(); i.setSelectionRange(pos, pos); } }
  window.scrollTo(0, scrollY);
  ctx.onState?.(m.sw.pause ? "paused" : !m.a.host ? "no word from the build Mac" : m.fresh(m.a.beat) ? (m.a.job ? "building" : "idle") : "build Mac out of touch");
}

async function poll() {
  if (!ctx?.token) return;
  try {
    const [code, sw] = await ctx.call("/work/swarm", {});
    if (code !== 200 || !sw) return;
    last = sw;
    // What happened since the last event this page has (a few hundred at first).
    if (sw.seq != null) {
      const from = events.length ? events[events.length - 1].seq : Math.max(0, sw.seq - 600);
      if (sw.seq > from) {
        const [c2, hist] = await ctx.call("/work/history", { since: from, max: 1000 }).catch(() => [0, null]);
        if (c2 === 200 && hist?.events) {
          events = events.concat(hist.events).slice(-2000);
        }
      }
      // (What this browser last showed: none on a first visit, then nothing's highlighted.)
      if (seenSeq === undefined) seenSeq = ctx.store.get("seenSeq", null);
    }
    render();
  } catch (e) {
    ctx.onState?.(`can't reach the build Mac (${e.message})`);
  }
}

/** Starts the dashboard in `root`: `call(path, body)` asks the coordinator ([status, json]); `ask(pause)`
 * pauses the build (null: lets it go on); `store` keeps this browser's own (what it last showed). */
export function dashboard(opts) {
  ctx = opts;
  ctx.ask = async (pause) => {
    try {
      await ctx.call("/work/pause", { pause: pause && { ...pause, by: ctx.label, at: Math.floor(Date.now() / 1000) }, at: Math.floor(Date.now() / 1000) });
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
