// Live on each Mac (the build page's Machines tab; docs/pipeline.md §5): each Mac's own measures
// over time from its own port's /work/live (docs/pool.md §10, Live), asked every 1.5 s while the
// Machines tab is on screen: the window the footer's range picks at first, then the points since
// the newest it has (the same ring, so one resolution). This page's own Mac through its own calls;
// the others at their addresses in /work/swarm's `macs` (by the scheme the page came over).
// Drawn per Mac (docs/mockups/build-live.html's vitals): each core's share busy now, its transfers
// (the jobs' NAS reads and writes, the internet in and out), then a chart a measure: CPU use (the
// build's and the rest's, stacked, against its cores and the build's limit), the load, memory (the
// build's and the rest's against the build's limit), its pressure, the swap, the disk's free space
// (against the room target) and writes, the battery (shaded on mains), the NAS's answer time and
// calls, its heat (macOS's thermal state), and whether the owner is using it; and a footer saying the range, its resolution and how
// long the Mac was silent in it (the charts' gaps). A measure missing is said so, never zero.
import { draw } from "./chart.js";
import { commonGaps, fmt, footer, hhmm, latest, merge, pressureClass, pressureWord, recentMean } from "./chartmath.js";

const EVERY_MS = 1500;
// How often a Mac's day of missed NAS calls is asked (nas.js's "timed out").
const DAY_EVERY_MS = 60000;
// How old a reading of the agent's loop (power, idleness, limits) may be and still be said.
const LOOP_FRESH_S = 900;
const SERIES = ["nas_read_mbs", "nas_write_mbs", "net_in_mbs", "net_out_mbs", "build_cores", "others_cores", "cores_busy", "cores_busy_e", "cores_busy_p", "load1",
  "mem_free_pct", "mem_pressure", "mem_compressed_gb", "thermal", "swap_mb", "build_mem_gb", "others_mem_gb", "disk_free_gb", "disk_size_gb", "disk_write_mbs", "battery_pct", "ac", "idle_s",
  "build_cores_max", "build_mem_max_gb", "disk_room_gb", "disk_reserved_gb", "nas_ms", "nas_calls", "nas_missed", "jobs",
  // (The lead's alone, summed over the pool; missing from the others' answers: nas.js.)
  "pool_nas_read_mbs", "pool_nas_write_mbs", "pool_nas_files_s", "pool_nas_creates_s", "pool_nas_listed_s", "nas_disk_busy_pct"];

let cfg = null;
// Per host: {series, step, now, mac, at (ms), error, asking, url (the address that answered)}.
const kept = new Map();
// Per host: its block's elements, kept across the page's renders (moved into each new card).
const blocks = new Map();
let ownHost = null;

const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const css = (k) => getComputedStyle(document.documentElement).getPropertyValue(k).trim();

/** Starts asking: `call(path, body)` this page's own Mac ([status, json]); `store` the browser's
 * own (the footer's `range`); `macs()` the swarm's members' addresses; `active()` whether the
 * Machines tab is shown. */
export function startLive(c) {
  cfg = c;
  setInterval(tick, EVERY_MS);
  document.addEventListener("visibilitychange", tick);
  window.addEventListener("hashchange", tick);
  tick();
}

/** A Mac's kept answer ({series, now, step, range, mac, error, day}; null before the first) for the
 * NAS section (nas.js), and `onLive(fn)`: `fn(host)` after each answer or failure. `day`: its NAS
 * calls missed over the last day (the live store's minutes, asked once a minute; null till then). */
export const liveOf = (host) => kept.get(host) || null;
const listeners = [];
export const onLive = (fn) => listeners.push(fn);

const range = () => cfg.store.get("range", 3600);

function tick() {
  if (!cfg || document.visibilityState !== "visible" || !cfg.active()) return;
  ask(null);
  for (const m of cfg.macs()) if (m.host !== ownHost) ask(m);
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
    if (!(Date.now() - st.dayAt < DAY_EVERY_MS)) {
      st.dayAt = Date.now();
      const dayBody = { range: 86400, series: ["nas_missed"], net: false };
      // (Its own failure kept apart: the minute's answer stands; the day said unknown till the next.)
      const d = await (m ? remote(m, st, dayBody) : cfg.call("/work/live", dayBody).then(([code, v]) => (code === 200 ? v : null))).catch(() => null);
      // (A point's mean a second times its span: the calls missed in it.)
      st.day = d?.series ? (d.series.nas_missed || []).reduce((t, p) => t + p[2] * d.step, 0) : null;
    }
  } catch (e) {
    st.error = e.name === "TimeoutError" ? "no answer in 5 s" : e.message;
  } finally {
    st.asking = false;
  }
  paint(m ? m.host : ownHost);
}

// Another Mac's answer, at its addresses (HTTPS over an HTTPS page: a browser refuses plain HTTP
// there), the last one that answered first. (activity.js asks with it too.)
export async function remote(m, st, body) {
  const https = location.protocol === "https:";
  const urls = https ? [m.https].filter(Boolean) : [...(m.urls || [])];
  if (!urls.length) throw new Error(https ? "no HTTPS address (this page came over HTTPS)" : "no address");
  if (st.url && urls.includes(st.url)) urls.sort((a, b) => (a === st.url ? -1 : b === st.url ? 1 : 0));
  let last;
  for (const u of urls) {
    try {
      const r = await fetch(`${u.replace(/\/$/, "")}/work/live`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal: AbortSignal.timeout ? AbortSignal.timeout(5000) : undefined });
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

// A row: its name (its tip on hover), a canvas (a chart with its axis; tall for the transfers), its words.
function row(root, name, tip, tall) {
  const r = el("div", `lrow ${tall ? "tall" : ""}`);
  const c = el("canvas");
  const v = el("div", "v");
  const k = el("span", "k", name);
  k.title = tip;
  r.append(k, c, v);
  root.append(r);
  return { c, v };
}

function build() {
  const root = el("div", "live");
  const head = el("div", "lhead");
  const said = el("span", "dim");
  head.append(el("b", null, "Live"), said);
  root.append(head);
  const body = el("div", "lbody");
  root.append(body);
  const nowRow = el("div", "lrow");
  const bars = el("div", "cnow");
  const nowV = el("div", "v");
  const nowK = el("span", "k", "Cores now");
  nowK.title = "Each core's share busy over the last second: the efficiency cores (dimmer) first, then the performance ones.";
  nowRow.append(nowK, bars, nowV);
  body.append(nowRow);
  return {
    root, said, body, bars, nowV,
    io: row(body, "Transfers", "The jobs' NAS reads and writes, the internet in and out", true),
    cpu: row(body, "CPU use", "Cores busy, summed over every core, stacked: the build's jobs (blue) and everything else (grey). Dashed: the Mac's cores, and the cores the build may use now (fewer while the owner uses the Mac)."),
    load: row(body, "Load", "Threads ready to run, averaged over a minute. Above the dashed line (the core count), work waits for a core."),
    mem: row(body, "Memory", "RAM in use, stacked: the build's jobs (blue) and every other process (grey). Dashed: the memory the build's jobs may hold together."),
    press: row(body, "Pressure", "100 % less macOS's memory-free percentage (kern.memorystatus_level); the line's colour is macOS's pressure level. Dashed: 50 % and 80 %."),
    swap: row(body, "Swap", "Memory macOS has written out to disk. Rising swap means jobs slow down."),
    disk: row(body, "Disk free", "Free space on the Mac's disk. Dashed: the owner's room target; below it the disk guard freezes the fastest-growing job."),
    dw: row(body, "Disk write", "Bytes written to this Mac's disks each second: the jobs' scratch and caches, and every other app's."),
    power: row(body, "Battery", "Battery charge; shaded while on mains. Dashed: on battery, a Mac below it can't lead."),
    nas: row(body, "NAS", "How long this Mac's listing of the NAS took (measured while a build runs here); its NAS calls in flight; those past their deadline or refused (every thread busy)"),
    heat: row(body, "Heat", "macOS's own thermal state (ProcessInfo.thermalState's four levels): nominal, fair (slightly raised), serious (macOS slows the Mac), critical (it slows it hard)."),
    use: row(body, "In use", "Whether the owner is using this Mac (a keyboard or trackpad touch lately), shaded while so. While in use, the build leaves cores free for them."),
  };
}

// A value's words: its lines (the first the figure, the rest dim; [colour, text] keyed), coloured when it's a worry.
function words(v, lines, cls = "", title = "") {
  v.replaceChildren(...lines.filter((t) => t).map((t, i) => {
    if (!Array.isArray(t)) return el("div", i ? "dim" : null, t);
    const d = el("div", "dim");
    const sw = el("i", "sw");
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
  // (The transfers' chart says the time; the rest share its range, their scales alone.)
  const base = { from, to: now, step, gaps: gapList, axis: true, times: false };
  const loopBase = { ...base, step: Math.max(step, LOOP_FRESH_S / 2) };
  const nr = css("--nas-r"), nw = css("--nas-w"), nd = css("--net-d"), nu = css("--net-u"), acc = css("--accent"), dim = css("--dim");
  const ok = css("--ok"), warn = css("--warn"), bad = css("--bad");
  const mac = st.mac || {};

  // Cores now: each core's share busy over the last second, efficiency cores first.
  const each = st.cores_now || [];
  const ne = mac.cores_e || 0;
  if (b.bars.childElementCount !== each.length) {
    b.bars.replaceChildren(...each.map((_, i) => {
      const c = el("i", i < ne ? "e" : "");
      c.append(el("b"));
      c.title = `${i < ne ? "efficiency" : "performance"} core ${i < ne ? i + 1 : i - ne + 1}`;
      return c;
    }));
  }
  each.forEach((f, i) => { b.bars.children[i].firstChild.style.height = `${Math.round(f * 100)}%`; });
  words(b.nowV, each.length ? [ne ? `${ne} E · ${each.length - ne} P` : `${each.length} cores`] : ["not measured"]);

  // Transfers: the jobs' NAS reads and writes, the internet in and out (MB/s).
  draw(b.io.c, { ...base, times: true, unit: "MB/s", series: [
    { points: s.nas_read_mbs, colour: nr, label: "NAS ↓" }, { points: s.nas_write_mbs, colour: nw, label: "NAS ↑" },
    { points: s.net_in_mbs, colour: nd, label: "net ↓" }, { points: s.net_out_mbs, colour: nu, label: "net ↑" }] });
  // (Their last minute's mean: a transfer comes in bursts, its newest second mostly none.)
  words(b.io.v, ["last min", ...[["NAS ↓", "nas_read_mbs", nr], ["NAS ↑", "nas_write_mbs", nw], ["net ↓", "net_in_mbs", nd], ["net ↑", "net_out_mbs", nu]].map(([t, k, col]) => [col, `${t} ${fmt(recentMean(s[k], now), "MB/s")}`])]);

  // CPU use: the build's and the rest's, stacked, against the Mac's cores and the build's limit.
  const n = mac.cores;
  const lim = loop("build_cores_max");
  draw(b.cpu.c, { ...base, unit: "", stacked: true, least: n || 0,
    // (The cores' line unlabelled when the build's limit is drawn: the two labels would meet.)
    refs: [...(n ? [{ v: n, label: lim != null && lim !== n ? "" : `${n} cores` }] : []), ...(lim != null && lim !== n ? [{ v: lim, label: `build's limit ${fmt(lim)}` }] : [])],
    series: [{ points: s.build_cores, colour: acc, label: "build", unit: "cores" }, { points: s.others_cores, colour: dim, label: "other", unit: "cores" }] });
  const busy = at("cores_busy"), ce = at("cores_busy_e"), cp = at("cores_busy_p");
  words(b.cpu.v, busy == null ? ["not measured"] : [`${fmt(busy)}${n ? ` of ${n}` : ""} cores busy`, [acc, `build's jobs ${fmt(at("build_cores"))}`], [dim, `other apps ${fmt(at("others_cores"))}`],
    ne && ce != null ? `P ${fmt(cp)} · E ${fmt(ce)}` : "", lim != null ? `build may use ${fmt(lim)}` : ""]);

  // Load: threads running or waiting, against the cores (past them, work waits for a core).
  const load = at("load1");
  draw(b.load.c, { ...base, unit: "", least: n ? 2 * n : 0, refs: n ? [{ v: n, label: `${n} cores` }] : [], series: [{ points: s.load1, colour: n && load > n ? warn : acc, label: "load" }] });
  words(b.load.v, load == null ? ["not measured"] : [`load ${fmt(load)}`, n ? (load > n ? `${fmt(load - n)} waiting for a core` : "no queue") : ""], n && load > 2 * n ? "warn" : "");

  // Memory: the build's and the rest's (GB), stacked, against the Mac's and the build's limit.
  const memGb = mac.mem_gb, memLim = loop("build_mem_max_gb");
  draw(b.mem.c, { ...base, unit: "GB", stacked: true, max: memGb || undefined, refs: memLim != null ? [{ v: memLim, label: `build's memory ${fmt(memLim, "GB")}` }] : [],
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
  draw(b.press.c, { ...base, unit: "%", max: 100, colourOf, marks: g ? [g[0]] : [], refs: [{ v: 50, label: "warn" }, { v: 80, label: "critical" }], series: [{ points: used, colour: ok, fill: true, label: "pressure", unit: "%" }] });
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
  const df = latest(s.disk_free_gb, now, 60), size = latest(s.disk_size_gb, now, 60), room = loop("disk_room_gb"), resv = loop("disk_reserved_gb");
  draw(b.disk.c, { ...base, unit: "GB", refs: room ? [{ v: room, label: `room target ${fmt(room, "GB")}` }] : [], series: [{ points: s.disk_free_gb, colour: acc, fill: true, label: "free", unit: "GB" }] });
  words(b.disk.v, df == null ? ["not measured"] : [`${fmt(df, "GB")} free`, size ? `of ${fmt(size, "GB")}` : "", resv != null ? `${fmt(resv, "GB")} reserved by jobs` : ""], room && df != null && df < room ? "bad" : "");

  // Disk write (MB/s).
  draw(b.dw.c, { ...base, unit: "MB/s", series: [{ points: s.disk_write_mbs, colour: nw, fill: true, label: "write" }] });
  const dwNow = at("disk_write_mbs");
  words(b.dw.v, dwNow == null ? ["not measured"] : [fmt(dwNow, "MB/s"), `last min ${fmt(recentMean(s.disk_write_mbs, now), "MB/s")}`]);

  // Battery: its charge, shaded while on mains, against the lead's floor on battery.
  const bat = loop("battery_pct"), ac = loop("ac"), floor = mac.battery_min;
  draw(b.power.c, { ...loopBase, unit: "%", max: 100, shade: { points: s.ac, colour: ok }, refs: floor ? [{ v: floor, label: `lead floor ${floor} %` }] : [],
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
  draw(b.heat.c, { ...base, unit: "", max: 3, series: [{ points: s.thermal, colour: th > 1 ? bad : th ? warn : ok, label: "heat", unit: "" }], refs: [{ v: 2, label: "macOS slows the Mac" }] });
  words(b.heat.v, th == null ? ["not measured"] : [HEAT[Math.round(th)], ["within normal limits", "slightly raised", "macOS is slowing the Mac", "macOS is slowing the Mac hard"][Math.round(th)]], th >= 2 ? "bad" : th >= 1 ? "warn" : "");

  // In use: the owner's last touch within `away_s`, as the agent's loop read it.
  const away = mac.away_s || 300;
  const inUse = (s.idle_s || []).map((p) => { const u = +(p[1] < away); return [p[0], u, u, u]; });
  const idle = loop("idle_s");
  draw(b.use.c, { ...loopBase, unit: "", max: 1, shade: { points: inUse, colour: acc }, series: [{ points: inUse, colour: acc, label: "in use", unit: "" }] });
  words(b.use.v, idle == null ? ["not measured"] : idle < away ? ["in use", `touched ${dur(idle)} ago`] : [`away for ${dur(idle)}`, "the build may use every core"]);
}
