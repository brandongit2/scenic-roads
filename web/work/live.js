// Live on each Mac (the build page's Machines tab; docs/pipeline.md §5): each Mac's own measures
// over time from its own port's /work/live (docs/pool.md §10, Live), asked every 1.5 s while the
// Machines tab is on screen: the window the footer's range picks at first, then the points since
// the newest it has (the same ring, so one resolution). This page's own Mac through its own calls;
// the others at their addresses in /work/swarm's `macs` (by the scheme the page came over).
// Drawn per Mac: its transfers (the jobs' NAS reads and writes, the internet in and out), its
// cores (the build's and the rest's, stacked, against its cores), and its vitals in compact rows
// (CPU and load, memory coloured by its pressure with the swap, the disk's free space and writes,
// the battery, the NAS's answer time), and a footer saying the range, its resolution and how long
// the Mac was silent in it (the charts' gaps). A measure missing is said so, never zero.
import { draw } from "./chart.js";
import { commonGaps, fmt, footer, latest, merge, pressureClass, pressureWord } from "./chartmath.js";

const EVERY_MS = 1500;
const SERIES = ["nas_read_mbs", "nas_write_mbs", "net_in_mbs", "net_out_mbs", "build_cores", "others_cores", "cores_busy", "cores_busy_e", "cores_busy_p", "load1",
  "mem_free_pct", "mem_pressure", "swap_mb", "build_mem_gb", "disk_free_gb", "disk_write_mbs", "battery_pct", "ac", "nas_ms", "nas_calls", "jobs"];

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
    Object.assign(st, { range: r, step: ans.step, now: ans.now, mac: ans.mac, at: Date.now(), error: null });
  } catch (e) {
    st.error = e.name === "TimeoutError" ? "no answer in 5 s" : e.message;
  } finally {
    st.asking = false;
  }
  paint(m ? m.host : ownHost);
}

// Another Mac's answer, at its addresses (HTTPS over an HTTPS page: a browser refuses plain HTTP
// there), the last one that answered first.
async function remote(m, st, body) {
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

/** A Mac's live block (kept: moved into each new card), drawn as its answers come. */
export function liveBlock(host) {
  let b = blocks.get(host);
  if (!b) {
    b = build();
    blocks.set(host, b);
  }
  queueMicrotask(() => paint(host));
  return b.root;
}

// A row: its name, a canvas (tall: a chart with its axis; else a strip), its words.
function row(root, name, tall) {
  const r = el("div", `lrow ${tall ? "tall" : ""}`);
  const c = el("canvas");
  const v = el("div", "v");
  r.append(el("span", "k", name), c, v);
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
  return {
    root, said, body,
    io: row(body, "Transfers", true),
    cores: row(body, "Cores", true),
    cpu: row(body, "CPU"),
    mem: row(body, "Memory"),
    disk: row(body, "Disk"),
    power: row(body, "Battery"),
    nas: row(body, "NAS"),
  };
}

// A value's words, coloured when it's a worry.
const words = (v, text, cls = "", title = "") => { v.textContent = text; v.className = `v ${cls}`; v.title = title; };

function paint(host) {
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
  const base = { from, to: now, step, gaps: gapList };
  const nr = css("--nas-r"), nw = css("--nas-w"), nd = css("--net-d"), nu = css("--net-u"), acc = css("--accent"), dim = css("--dim");

  // Transfers: the jobs' NAS reads and writes, the internet in and out (MB/s).
  draw(b.io.c, { ...base, axis: true, unit: "MB/s", series: [
    { points: s.nas_read_mbs, colour: nr, label: "NAS ↓" }, { points: s.nas_write_mbs, colour: nw, label: "NAS ↑" },
    { points: s.net_in_mbs, colour: nd, label: "net ↓" }, { points: s.net_out_mbs, colour: nu, label: "net ↑" }] });
  b.io.v.replaceChildren(...[["NAS ↓", "nas_read_mbs", nr], ["NAS ↑", "nas_write_mbs", nw], ["net ↓", "net_in_mbs", nd], ["net ↑", "net_out_mbs", nu]].map(([t, k, col]) => {
    const d = el("div", null);
    const i = el("i", "sw"); i.style.background = col;
    d.append(i, `${t} ${fmt(at(k), "MB/s")}`);
    return d;
  }));
  b.io.v.className = "v legend";

  // Cores: the build's and the rest's, stacked, against the Mac's cores and its performance ones.
  const n = st.mac?.cores, e = st.mac?.cores_e || 0;
  draw(b.cores.c, { ...base, axis: true, unit: "", stacked: true, least: n || 0, refs: n ? [{ v: n, label: `${n} cores` }, ...(e ? [{ v: n - e, label: `${n - e} P` }] : [])] : [], series: [
    { points: s.build_cores, colour: acc, label: "build", unit: "cores" }, { points: s.others_cores, colour: dim, label: "other", unit: "cores" }] });
  const ce = at("cores_busy_e"), cp = at("cores_busy_p");
  b.cores.v.replaceChildren(el("div", null, `build ${fmt(at("build_cores"))} · other ${fmt(at("others_cores"))}${n ? ` of ${n}` : ""}`),
    el("div", "dim", e && ce != null ? `P ${fmt(cp)} of ${n - e} · E ${fmt(ce)} of ${e}` : ""));
  b.cores.v.className = "v";

  // CPU: the cores busy (of all), and the load.
  const busy = at("cores_busy");
  draw(b.cpu.c, { ...base, unit: "cores", max: n || undefined, series: [{ points: s.cores_busy, colour: acc, fill: true, label: "busy" }] });
  words(b.cpu.v, busy == null ? "not measured" : `${n ? fmt((busy / n) * 100, "%") : fmt(busy, "cores")} · load ${fmt(at("load1"))}`);

  // Memory: in use (of all), the line coloured by its pressure then; the build's and the swap as words.
  const used = (s.mem_free_pct || []).map((p) => [p[0], 100 - p[3], 100 - p[2], 100 - p[1]]);
  const pressure = new Map((s.mem_pressure || []).map((p) => [p[0], p[3]]));
  const colourOf = (p) => { const c = pressureClass(pressure.get(p[0])); return c === "bad" ? css("--bad") : c === "warn" ? css("--warn") : null; };
  draw(b.mem.c, { ...base, unit: "%", max: 100, colourOf, series: [{ points: used, colour: css("--ok"), fill: true, label: "in use", unit: "%" }] });
  const free = at("mem_free_pct"), pr = at("mem_pressure");
  const mem = st.mac?.mem_gb;
  words(b.mem.v, free == null ? "not measured" : `${fmt(100 - free, "%")}${mem ? ` of ${fmt(mem, "GB")}` : ""} · build ${fmt(at("build_mem_gb"), "GB")} · swap ${fmt(at("swap_mb"), "MB")}`, pressureClass(pr), `Memory in use; its colour is the memory pressure (${pressureWord(pr)})`);

  // Disk: its writes over time; free space and the write rate as words.
  const df = latest(s.disk_free_gb, now, 60);
  draw(b.disk.c, { ...base, unit: "MB/s", series: [{ points: s.disk_write_mbs, colour: nw, fill: true, label: "write" }] });
  words(b.disk.v, `${fmt(df, "GB")} free · writing ${fmt(at("disk_write_mbs"), "MB/s")}`, df != null && df < 10 ? "bad" : df != null && df < 20 ? "warn" : "");

  // Battery: its charge; on mains or not.
  const bat = latest(s.battery_pct, now, 120), ac = latest(s.ac, now, 120);
  draw(b.power.c, { ...base, unit: "%", max: 100, series: [{ points: s.battery_pct, colour: ac === 0 ? css("--warn") : css("--ok"), fill: true, label: "battery", unit: "%" }] });
  words(b.power.v, bat == null ? (ac === 1 ? "on mains, no battery" : "not measured") : `${ac === 0 ? "on battery" : "on mains"} · ${fmt(bat, "%")}`, ac === 0 && bat != null && bat < 40 ? "warn" : "");

  // NAS: how long a listing took (measured only while a build runs here); the calls in flight.
  const ms = latest(s.nas_ms, now, 30);
  draw(b.nas.c, { ...base, unit: "ms", series: [{ points: s.nas_ms, colour: nr, label: "answer", band: true }] });
  words(b.nas.v, ms == null ? "not measured (no build here now)" : `${fmt(ms, "ms")} · ${fmt(at("nas_calls"))} calls`, ms != null && ms > 1000 ? "bad" : ms != null && ms > 200 ? "warn" : "");
}
