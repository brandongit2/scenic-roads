// Live on each Mac (the build page's Machines tab; docs/pipeline.md §5): each Mac's own measures
// over time from its own port's /work/live (docs/pool.md §10, Live), asked every 1.5 s while the
// Machines tab is on screen: the window the footer's range picks at first, then the points since
// the newest it has (the same ring, so one resolution). This page's own Mac through its own calls;
// the others at their addresses in /work/swarm's `macs` (by the scheme the page came over).
// Drawn per Mac as docs/mockups/build-live.html draws it (a row a measure: its name over its words, its chart beside): each core's share busy now, its transfers
// (the jobs' NAS reads and writes, the internet in and out, each with a second line on its own scale:
// files opened, files made, the proxy's requests a second), then a chart a measure: CPU use (the
// build's and the rest's, stacked, against its cores and the build's limit), the load, memory (the
// build's and the rest's against the build's limit), its pressure, the swap, the disk's free space
// (against the room target) and writes, the battery (shaded on mains), the NAS's answer time and
// calls, its heat (macOS's thermal state), and whether the owner is using it; and a footer saying the range, its resolution and how
// long the Mac was silent in it (the charts' gaps). A measure missing is said so, never zero.
import { draw } from "./chart.js";
import { commonGaps, fmt, footer, hhmm, latest, merge, pressureClass, pressureWord, recentMean } from "./chartmath.js";
import { putSetting } from "./settings.js";

const EVERY_MS = 1500;
// How old a reading of the agent's loop (power, idleness, limits) may be and still be said.
const LOOP_FRESH_S = 900;
const SERIES = ["nas_read_mbs", "nas_write_mbs", "net_in_mbs", "net_out_mbs", "nas_files_s", "nas_creates_s", "net_req_s", "build_cores", "others_cores", "cores_busy", "cores_busy_e", "cores_busy_p", "load1",
  "mem_free_pct", "mem_pressure", "mem_compressed_gb", "thermal", "swap_mb", "build_mem_gb", "others_mem_gb", "disk_free_gb", "disk_size_gb", "disk_write_mbs", "battery_pct", "ac", "idle_s",
  "build_cores_max", "build_mem_max_gb", "disk_room_gb", "disk_reserved_gb", "nas_ms", "nas_calls", "nas_missed", "jobs",
  // (The lead's alone, summed over the pool; missing from the others' answers: nas.js.)
  "pool_nas_read_mbs", "pool_nas_write_mbs", "pool_nas_files_s", "pool_nas_creates_s", "pool_nas_listed_s", "pool_nas_stats_s",
  "pool_nas_calls", "pool_nas_missed", "pool_nas_threads", "nas_disk_busy_pct"];

let cfg = null;
// Per host: {series, step, now, mac, at (ms), error, asking, url (the address that answered)}.
const kept = new Map();
// Per host: its block's elements, kept across the page's renders (moved into each new card).
const blocks = new Map();
let ownHost = null;
// Limits set from a chart, by "host|key" ("|key" the pool's): {v, at (ms)}, drawn till the loop reads them.
const pending = new Map();

const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const css = (k) => getComputedStyle(document.documentElement).getPropertyValue(k).trim();

/** Starts asking: `call(path, body)` this page's own Mac ([status, json]); `store` the browser's
 * own (the footer's `range`); `macs()` the swarm's members' addresses; `active()` whether the
 * Machines or NAS tab is shown; `only()`, the one Mac to ask (the lead, on the NAS tab), or null. */
export function startLive(c) {
  cfg = c;
  setInterval(tick, EVERY_MS);
  document.addEventListener("visibilitychange", tick);
  window.addEventListener("hashchange", tick);
  tick();
}

/** A Mac's kept answer ({series, now, step, range, mac, error}; null before the first) for the
 * NAS section (nas.js), and `onLive(fn)`: `fn(host)` after each answer or failure. */
export const liveOf = (host) => kept.get(host) || null;
const listeners = [];
export const onLive = (fn) => listeners.push(fn);

const range = () => cfg.store.get("range", 3600);

function tick() {
  if (!cfg || document.visibilityState !== "visible" || !cfg.active()) return;
  // (The NAS tab reads the lead's answer alone: only it is asked there, this Mac too till it knows
  // its own name.)
  const only = cfg.only?.();
  if (!only || !ownHost || only === ownHost) ask(null);
  for (const m of cfg.macs()) if (m.host !== ownHost && (!only || m.host === only)) ask(m);
}

// The newest point's time among the series (asked from on).
const newest = (series) => Math.max(0, ...Object.values(series || {}).map((p) => p[p.length - 1][0]));

async function ask(m) {
  const host = m ? m.host : ownHost || "";
  const st = kept.get(host) || {};
  if (st.asking) return;
  const r = range();
  const fresh = st.series && st.range === r;
  const body = { range: r, series: SERIES, net: false, ...(fresh ? { since: newest(st.series) } : {}) };
  st.asking = true;
  kept.set(host, st);
  try {
    let ans;
    if (!m) {
      const [code, v] = await cfg.call("/work/live", body);
      if (code !== 200 || !v) throw new Error(code === 503 ? "not measuring yet" : `answered ${code}`);
      ans = v;
      if (!ownHost) { ownHost = ans.host; kept.delete(""); kept.set(ownHost, st); }
    } else ans = await remote(m, st, body);
    st.series = merge(fresh ? st.series : null, ans, ans.now - r);
    Object.assign(st, { range: r, step: ans.step, now: ans.now, mac: ans.mac, cores_now: ans.cores_now, at: Date.now(), error: null });
  } catch (e) {
    st.error = e.name === "TimeoutError" ? "no answer in 5 s" : e.message;
  } finally {
    st.asking = false;
  }
  paint(m ? m.host : ownHost);
}

// Another Mac's answer, at its addresses (HTTPS over an HTTPS page: a browser refuses plain HTTP
// there), the last one that answered first; `path` the call (activity.js asks with it too, and the
// world map its member's tiles to come: dash.js).
export async function remote(m, st, body, path = "/work/live") {
  const https = location.protocol === "https:";
  const urls = https ? [m.https].filter(Boolean) : [...(m.urls || [])];
  if (!urls.length) throw new Error(https ? "no HTTPS address (this page came over HTTPS)" : "no address");
  if (st.url && urls.includes(st.url)) urls.sort((a, b) => (a === st.url ? -1 : b === st.url ? 1 : 0));
  let last;
  for (const u of urls) {
    try {
      const r = await fetch(`${u.replace(/\/$/, "")}${path}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal: AbortSignal.timeout ? AbortSignal.timeout(5000) : undefined });
      if (!r.ok) throw new Error(r.status === 503 ? "not measuring yet" : r.status === 404 ? "its app is older (no live measures)" : `answered ${r.status}`);
      st.url = u;
      return await r.json();
    } catch (e) { last = e; }
  }
  throw last.message === "Failed to fetch" || last.message === "Load failed" ? new Error("doesn't answer") : last;
}

/** A Mac's live block (kept: moved into each new card), drawn as its answers come; `guard`, what
 * its memory guard last did ([unix s, words]: its status's `memory.last`), said on the pressure row. */
export function liveBlock(host, guard) {
  let b = blocks.get(host);
  if (!b) {
    b = build();
    blocks.set(host, b);
  }
  b.guard = guard || null;
  queueMicrotask(() => paint(host));
  return b.root;
}

// A row (the mockup's .rate): its name over its words (its tip on hover) on the left, its chart on the right; `col`, a swatch before the name.
export function row(root, name, tip, col, cls = "vrow") {
  const r = el("div", `rate ${cls}`);
  const lab = el("div", "lab");
  lab.title = tip;
  const k = el("span", "k");
  if (col) { const sw = el("i"); sw.style.background = `var(${col})`; k.append(sw); }
  k.append(name);
  const v = el("span", "v");
  lab.append(k, v);
  const cw = el("div", "cw");
  const c = el("canvas");
  cw.append(c);
  r.append(lab, cw);
  root.append(r);
  return { c, v };
}

function build() {
  const root = el("div", "live");
  const head = el("div", "lhead");
  const said = el("span", "dim");
  head.append(el("b", null, "Live on this Mac"), said);
  root.append(head);
  const body = el("div", "lbody");
  root.append(body);
  const io = el("div", "mac");
  const xfer = (name, col, tip) => row(io, name, tip, col, "");
  const ioRows = {
    nr: xfer("NAS read", "--nas-r", "Bytes this Mac's jobs read from the NAS each second. Dashed, on the right's scale: files they opened each second."),
    nw: xfer("NAS write", "--nas-w", "Bytes this Mac's jobs wrote to the NAS each second. Dashed, on the right's scale: files they made each second."),
    nd: xfer("From internet", "--net-d", "Bytes this Mac's jobs downloaded from the internet each second. Dashed, on the right's scale: requests they made each second."),
    nu: xfer("To internet", "--net-u", "Bytes this Mac's jobs sent to the internet each second: requests and queries. Dashed, on the right's scale: requests they made each second."),
  };
  const vit = el("div", "mac");
  const legend = el("div", "phlegend");
  for (const [c, t] of [["var(--accent)", "busy"], ["var(--line)", "idle"]]) { const sp = el("span"); const sw = el("i"); sw.style.background = c; sp.append(sw, t); legend.append(sp); }
  const bars = el("div", "cores");
  bars.title = "Each core's share busy over the last second: the efficiency cores first, then the performance ones.";
  const nowV = el("div", "dim small");
  vit.append(legend, bars, nowV);
  body.append(io, vit);
  return {
    root, said, body, bars, nowV, ioRows,
    cpu: row(vit, "cpu use", "Cores busy, summed over every core, stacked: the build's jobs (blue) and everything else (grey). Dashed: the Mac's cores, and the cores the build may use now (fewer while the owner uses the Mac)."),
    load: row(vit, "CPU load", "Threads ready to run, averaged over a minute. Above the dashed line (the core count), work waits for a core."),
    mem: row(vit, "memory", "RAM in use, stacked: the build's jobs (blue) and every other process (grey). Dashed: the memory the build's jobs may hold together."),
    press: row(vit, "memory pressure", "100 % less macOS's memory-free percentage (kern.memorystatus_level); the line's colour is macOS's pressure level. Dashed: 50 % and 80 %."),
    swap: row(vit, "swap used", "Memory macOS has written out to disk. Rising swap means jobs slow down."),
    disk: row(vit, "disk free", "Free space on the Mac's disk. Dashed: the owner's room target; below it the disk guard freezes the fastest-growing job."),
    dw: row(vit, "disk write", "Bytes written to this Mac's disks each second: the jobs' scratch and caches, and every other app's."),
    power: row(vit, "battery", "Battery charge; shaded while on mains. Dashed: on battery, a Mac below it can't lead."),
    nas: row(vit, "NAS response", "How long this Mac's listing of the NAS took (measured while a build runs here); its NAS calls in flight; those past their deadline or refused (every thread busy)"),
    use: row(vit, "someone using this Mac?", "Whether the owner is using this Mac (a keyboard or trackpad touch lately), shaded while so. While in use, the build leaves cores free for them."),
    heat: row(vit, "heat", "macOS's own thermal state (ProcessInfo.thermalState's four levels): nominal, fair (slightly raised), serious (macOS slows the Mac), critical (it slows it hard)."),
  };
}

// A value's words: its lines (the first the figure, the rest dim; [colour, text] keyed), coloured when it's a worry.
export function words(v, lines, cls = "", title = "") {
  const shown = lines.filter((t) => t);
  v.replaceChildren(...shown.map((t, i) => {
    if (!i && !Array.isArray(t)) return document.createTextNode(t);
    if (!Array.isArray(t)) return el("small", null, t);
    const d = el("small", "memline");
    const sw = el("i");
    sw.style.background = t[0];
    d.append(sw, t[1]);
    return d;
  }));
  v.className = `v ${cls}`;
  v.title = title;
}

// A duration in words (s): "40 s", "12 min", "3 h".
const dur = (s) => (s < 90 ? `${Math.round(s)} s` : s < 5400 ? `${Math.round(s / 60)} min` : `${fmt(s / 3600)} h`);

function paint(host) {
  for (const f of listeners) f(host);
  const b = blocks.get(host);
  const st = kept.get(host);
  if (!b || !b.root.isConnected) return;
  if (!st?.series) {
    b.body.hidden = true;
    b.said.textContent = st?.error ? `no live measures: ${st.error}` : "asking…";
    return;
  }
  b.body.hidden = false;
  const s = st.series, now = st.now, step = st.step, r = st.range;
  const from = now - r;
  const gapList = commonGaps(["load1", "cores_busy", "mem_free_pct"].map((k) => ({ points: s[k], step })), now);
  const late = Date.now() - st.at > 10000;
  b.said.textContent = `${footer(r, step, gapList)}${late || st.error ? ` · stale: ${st.error || "no answer"} since ${new Date(st.at).toLocaleTimeString([], { hour12: false })}` : ""}`;
  b.said.className = late ? "warn" : "dim";
  const at = (k) => latest(s[k], now);
  // (Read by the agent's loop, not the sampler: a point a loop, minutes apart when a step's long or
  // the Mac's starved; their lines joined over that.)
  const loop = (k) => latest(s[k], now, LOOP_FRESH_S);
  // (Every chart says the time under it, as the mockup's.)
  // (A settable limit (#612): its line dragged, or its ✎'s editor, sets it on this Mac through
  // /work/settings as Settings' rows do; set, the line holds the
  // new value till the agent's loop reads it (LOOP_FRESH_S at most).)
  const id = (key) => `${host}|${key}`;
  // (Dropped once the loop says it, or says otherwise twice since: set elsewhere since, or refused.)
  const held = (key, v, points) => {
    const p = pending.get(id(key));
    if (!p) return v;
    let since = 0;
    for (let i = (points?.length || 0) - 1; i >= 0 && since < 2 && points[i][0] * 1000 > p.at; i--) since++;
    if (Date.now() - p.at > LOOP_FRESH_S * 1000 || (v != null && Math.abs(v - p.v) < 1e-6) || since >= 2) { pending.delete(id(key)); return v; }
    return p.v;
  };
  const edit = (key, e) => ({ key, ...e, set: async (v) => {
    const ok = await putSetting(host, { [key]: e.body ? e.body(v) : v }, e.title.toLowerCase());
    if (ok && v !== "off") pending.set(id(key), { v, at: Date.now() });
    else if (ok) pending.delete(id(key));
    if (ok) paint(host);
    return ok;
  } });
  const base = { from, to: now, step, gaps: gapList, axis: true, times: true };
  const loopBase = { ...base, step: Math.max(step, LOOP_FRESH_S / 2) };
  const nr = css("--nas-r"), nw = css("--nas-w"), nd = css("--net-d"), nu = css("--net-u"), acc = css("--accent"), dim = css("--dim");
  const ok = css("--ok"), warn = css("--warn"), bad = css("--bad");
  const mac = st.mac || {};

  // Cores now: each core's share busy over the last second, efficiency cores first.
  const each = st.cores_now || [];
  const ne = mac.cores_e || 0;
  if (b.bars.querySelectorAll("i:not(.rule)").length !== each.length) {
    const groups = (ne ? [["efficiency", 0, ne], ["performance", ne, each.length]] : [["", 0, each.length]]).filter(([, a0, a1]) => a1 > a0);
    b.bars.replaceChildren(...groups.map(([kind, a0, a1]) => {
      const g = el("div", "cgrp");
      g.style.flex = String(a1 - a0);
      const cb = el("div", "cbars");
      for (let i = a0; i < a1; i++) { const c = el("i"); c.append(el("b")); c.title = `${kind || "core"} ${kind ? `core ${i - a0 + 1}` : i + 1}`; cb.append(c); }
      const cn = el("div", "cname");
      cn.append(el("span", null, `${a1 - a0} ${kind ? `${kind} cores` : "cores"}`), el("i", "rule"));
      g.append(cb, cn);
      return g;
    }));
  }
  const coreBars = b.bars.querySelectorAll(".cbars > i");
  each.forEach((f, i) => { coreBars[i].firstChild.style.height = `${Math.round(f * 100)}%`; });
  b.bars.hidden = !each.length;
  b.nowV.textContent = each.length ? "" : "cores now: not measured";

  // Transfers: the jobs' NAS reads and writes, the internet in and out (MB/s), each with its count
  // a second on the right's scale (files opened, files made, requests).
  for (const [key, k, col, k2, u2, w2] of [["nr", "nas_read_mbs", nr, "nas_files_s", "files/s", "opened"], ["nw", "nas_write_mbs", nw, "nas_creates_s", "files/s", "made"],
    ["nd", "net_in_mbs", nd, "net_req_s", "req/s", "requests"], ["nu", "net_out_mbs", nu, "net_req_s", "req/s", "requests"]]) {
    const r = b.ioRows[key];
    draw(r.c, { ...base, unit: "MB/s", rightUnit: u2, series: [{ points: s[k], colour: col, fill: true, label: "" }, { points: s[k2], colour: col, right: true, label: w2 }] });
    const now1 = at(k), now2 = at(k2);
    words(r.v, [now1 == null ? "not measured" : fmt(now1, "MB/s"), `${w2} ${now2 == null ? "not measured" : fmt(now2, u2)}`, `last min ${fmt(recentMean(s[k], now), "MB/s")}`]);
  }

  // CPU use: the build's and the rest's, stacked, against the Mac's cores and the build's limit.
  const n = mac.cores;
  const lim = held("in_use", loop("build_cores_max"), s.build_cores_max);
  draw(b.cpu.c, { ...base, unit: "", stacked: true, least: n || 0,
    // (The Mac's cores are the limit's top, so its line is always the limit's, at the cores when the build may use every one.)
    refs: n && lim != null ? [{ v: lim, label: `build's limit: ${fmt(lim)} cores${lim === n ? " (all)" : ""}`, edit: edit("in_use", { title: "Cores for the build", note: `Most cores the build's jobs may use while the owner is using ${host}; the rest are kept for the owner's apps. Off: the default share.`, unit: "cores", min: 1, max: n, step: 1, label: (v) => `build's limit: ${fmt(v)} cores${v === n ? " (all)" : ""}`, body: (v) => (v === "off" ? v : n - v) }) }] : n ? [{ v: n, label: `${n} cores` }] : [],
    series: [{ points: s.build_cores, colour: acc, label: "build", unit: "cores" }, { points: s.others_cores, colour: dim, label: "other", unit: "cores" }] });
  const busy = at("cores_busy"), ce = at("cores_busy_e"), cp = at("cores_busy_p");
  words(b.cpu.v, busy == null ? ["not measured"] : [`${fmt(busy)}${n ? ` of ${n}` : ""} cores busy`, [acc, `build's jobs ${fmt(at("build_cores"))}`], [dim, `other apps ${fmt(at("others_cores"))}`],
    ne && ce != null ? `P ${fmt(cp)} · E ${fmt(ce)}` : "", lim != null ? `build may use ${fmt(lim)}` : ""]);

  // Load: threads running or waiting, against the cores (past them, work waits for a core).
  const load = at("load1");
  draw(b.load.c, { ...base, unit: "", least: n ? 2 * n : 0, refs: n ? [{ v: n, label: `${n} cores` }] : [], series: [{ points: s.load1, colour: n && load > n ? warn : acc, label: "load" }] });
  words(b.load.v, load == null ? ["not measured"] : [`load ${fmt(load)}`, n ? (load > n ? `${fmt(load - n)} waiting for a core` : "no queue") : ""], n && load > 2 * n ? "warn" : "");

  // Memory: the build's and the rest's (GB), stacked, against the Mac's and the build's limit.
  const memGb = mac.mem_gb, memLim = held("memory", loop("build_mem_max_gb"), s.build_mem_max_gb);
  draw(b.mem.c, { ...base, unit: "GB", stacked: true, max: memGb || undefined, refs: memLim != null ? [{ v: memLim, label: `build's memory: ${fmt(memLim, "GB")}`, edit: edit("memory", { title: "Build memory", note: `The memory ${host} gives the build's jobs together. Off: the memory guard's limit alone.`, unit: "GB", min: 1, max: Math.floor(memGb || 1), step: 1, label: (v) => `build's memory: ${fmt(v, "GB")}` }) }] : [],
    series: [{ points: s.build_mem_gb, colour: acc, label: "build", unit: "GB" }, { points: s.others_mem_gb, colour: dim, label: "other", unit: "GB" }] });
  const free = at("mem_free_pct");
  words(b.mem.v, free == null ? ["not measured"] : [`${memGb ? fmt(memGb * (100 - free) / 100, "GB") : fmt(100 - free, "%")}${memGb ? ` of ${fmt(memGb, "GB")}` : ""} used`,
    [acc, `build's jobs ${fmt(at("build_mem_gb"), "GB")}`], [dim, `other apps ${fmt(at("others_mem_gb"), "GB")}`], memGb ? `free ${fmt(memGb * free / 100, "GB")}` : ""]);

  // Pressure: 100 % less memory free, coloured by macOS's level then.
  const used = (s.mem_free_pct || []).map((p) => [p[0], 100 - p[3], 100 - p[2], 100 - p[1]]);
  const level = new Map((s.mem_pressure || []).map((p) => [p[0], p[3]]));
  const colourOf = (p) => { const c = pressureClass(level.get(p[0])); return c === "bad" ? bad : c === "warn" ? warn : null; };
  // (What the memory guard last did, when in the range: a mark on the chart, and its words.)
  const g = b.guard && b.guard[0] >= from ? b.guard : null;
  draw(b.press.c, { ...base, unit: "%", max: 100, colourOf, marks: g ? [g[0]] : [], refs: [{ v: 50, label: "warn" }, { v: 80, label: "critical", colour: bad }], series: [{ points: used, colour: ok, fill: true, label: "pressure", unit: "%" }] });
  const pr = at("mem_pressure");
  const comp = at("mem_compressed_gb");
  words(b.press.v, free == null ? ["not measured"] : [`${fmt(100 - free, "%")} · ${pressureWord(pr).replace("pressure ", "")}`, `memory free ${fmt(free, "%")}`,
    comp != null ? `${fmt(comp, "GB")} compressed` : "", g ? `the guard at ${hhmm(g[0])}: ${g[1]}` : ""], pressureClass(pr), pressureWord(pr));

  // Swap (GB).
  const swapGb = (s.swap_mb || []).map((p) => [p[0], p[1] / 1024, p[2] / 1024, p[3] / 1024]);
  const sw = at("swap_mb");
  draw(b.swap.c, { ...base, unit: "GB", least: 1, series: [{ points: swapGb, colour: warn, fill: true, label: "swap", unit: "GB" }] });
  words(b.swap.v, sw == null ? ["not measured"] : [fmt(sw / 1024, "GB"), "written out to disk"]);

  // Disk free, against the owner's room target; the jobs' reservations as words.
  const df = latest(s.disk_free_gb, now, 60), size = latest(s.disk_size_gb, now, 60), room = held("room", loop("disk_room_gb"), s.disk_room_gb), resv = loop("disk_reserved_gb");
  draw(b.disk.c, { ...base, unit: "GB", refs: room ? [{ v: room, label: `room target: ${fmt(room, "GB")}`, edit: edit("room", { title: "Disk room target", note: `Free space the build keeps on ${host}'s disk. Off: no target.`, unit: "GB", min: 1, max: size ? Math.ceil(size) - 1 : 100000, step: 5, label: (v) => `room target: ${fmt(v, "GB")}` }) }] : [], series: [{ points: s.disk_free_gb, colour: acc, fill: true, label: "free", unit: "GB" }] });
  words(b.disk.v, df == null ? ["not measured"] : [`${fmt(df, "GB")} free`, size ? `of ${fmt(size, "GB")}` : "", resv != null ? `${fmt(resv, "GB")} reserved by jobs` : ""], room && df != null && df < room ? "bad" : "");

  // Disk write (MB/s).
  draw(b.dw.c, { ...base, unit: "MB/s", series: [{ points: s.disk_write_mbs, colour: nw, fill: true, label: "write" }] });
  const dwNow = at("disk_write_mbs");
  words(b.dw.v, dwNow == null ? ["not measured"] : [fmt(dwNow, "MB/s"), `last min ${fmt(recentMean(s.disk_write_mbs, now), "MB/s")}`]);

  // Battery: its charge, shaded while on mains, against the lead's floor on battery.
  const bat = loop("battery_pct"), ac = loop("ac"), floor = held("battery_floor", mac.battery_min);
  draw(b.power.c, { ...loopBase, unit: "%", max: 100, shade: { points: s.ac, colour: ok }, refs: floor != null ? [{ v: floor, label: `can't lead below ${floor} % on battery`, edit: edit("battery_floor", { title: "Battery floor", note: `${host}'s own: on battery, it can't lead below it, and as the lead hands over. Off: the default.`, unit: "%", min: 0, max: 100, step: 5, label: (v) => `can't lead below ${fmt(v)} % on battery` }) }] : [],
    series: [{ points: s.battery_pct, colour: ac === 0 && floor && bat < floor ? bad : ok, label: "battery", unit: "%" }] });
  words(b.power.v, bat == null ? [ac === 1 ? "on mains, no battery" : "not measured"] : [fmt(bat, "%"), ac === 0 ? "on battery" : "on mains"], ac === 0 && floor && bat < floor ? "warn" : "");

  // NAS: how long a listing took (measured only while a build runs here); the calls in flight.
  const ms = latest(s.nas_ms, now, 30);
  draw(b.nas.c, { ...base, unit: "ms", series: [{ points: s.nas_ms, colour: nr, label: "answer", band: true }] });
  // (Missed: a rate a second; the last minute's mean times 60 is its count.)
  const missed = Math.round((recentMean(s.nas_missed, now) || 0) * 60);
  words(b.nas.v, [...(ms == null ? ["not measured", "no build here now"] : [`${fmt(ms, "ms")} to answer`]), `${fmt(at("nas_calls"))} calls in flight`, missed ? `${missed} missed in the last min` : ""],
    missed || (ms != null && ms > 1000) ? "bad" : ms != null && ms > 200 ? "warn" : "");

  // Heat: macOS's thermal state, 0 nominal to 3 critical.
  const HEAT = ["nominal", "fair", "serious", "critical"];
  const th = at("thermal");
  draw(b.heat.c, { ...base, unit: "", max: 3, series: [{ points: s.thermal, colour: th > 1 ? bad : th ? warn : ok, label: "heat", unit: "" }], ticks: HEAT.map((w, i) => [i, w]) });
  words(b.heat.v, th == null ? ["not measured"] : [HEAT[Math.round(th)], ["within normal limits", "slightly raised", "macOS is slowing the Mac", "macOS is slowing the Mac hard"][Math.round(th)]], th >= 2 ? "bad" : th >= 1 ? "warn" : "");

  // In use: the owner's last touch within `away_s`, as the agent's loop read it.
  const away = mac.away_s || 300;
  const inUse = (s.idle_s || []).map((p) => { const u = +(p[1] < away); return [p[0], u, u, u]; });
  const idle = loop("idle_s");
  draw(b.use.c, { ...loopBase, unit: "", max: 1, shade: { points: inUse, colour: acc }, series: [{ points: inUse, colour: acc, label: "in use", unit: "" }], ticks: [[0, "away"], [1, "in use"]] });
  words(b.use.v, idle == null ? ["not measured"] : idle < away ? ["in use", `touched ${dur(idle)} ago`] : [`away for ${dur(idle)}`, "the build may use every core"]);
}
