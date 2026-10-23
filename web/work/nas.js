// The NAS on the build page's Machines tab (docs/pool.md §10, Live; docs/pipeline.md §5): a card
// for the NAS itself (what the lead's status says of it, `agent.nas`: the load of its disks against
// the owner's target, the gate, the limits its probes give; and over time the pool's traffic, its
// file work and the disks' busy share, from the lead's /work/live) and a card a Mac (its share of
// the NAS budget, its jobs' reads and writes, how long its listings take, its calls in flight and
// those that missed their deadline, from its own /work/live). The answers are live.js's, asked
// while the tab shows; every chart is chart.js's. A measure missing is said so, never zero.
import { draw } from "./chart.js";
import { commonGaps, fmt, footer, latest, recentMean, spanText } from "./chartmath.js";
import { liveOf, onLive } from "./live.js";
import { isSlotOf } from "./sums.js";

const MB = 1048576;
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const css = (k) => getComputedStyle(document.documentElement).getPropertyValue(k).trim();

// Kept across the page's renders (moved into each new section): a chart row's canvas and words.
const rows = new Map();
const root = el("section", "d-nas");
// What the last render said: {agent, macs} (dash.js's model), for the repaints live answers bring.
let seen = null;

function row(key, name, tall) {
  let r = rows.get(key);
  if (!r) {
    r = { row: el("div", `lrow ${tall ? "tall" : ""}`), c: el("canvas"), v: el("div", "v") };
    r.row.append(el("span", "k", name), r.c, r.v);
    rows.set(key, r);
  }
  return r;
}

// A bar and its words, as a Mac's card says its capacity (dash.css .cap).
function bar(label, used, whole, words, cls = "", mark = null) {
  const pc = (v) => `${Math.round(Math.max(0, Math.min(1, whole ? v / whole : 0)) * 100)}%`;
  const b = el("div", `cbar ${cls}`);
  const i = el("i"); i.style.width = pc(used); b.append(i);
  if (mark != null) { const m = el("b"); m.style.cssText = `width:2px;left:${pc(mark)};background:var(--mark);position:absolute;top:0;bottom:0`; b.append(m); b.style.position = "relative"; }
  const d = el("div", "cap");
  d.append(el("span", "k", label), b, el("span", "v", words));
  return d;
}

const words = (v, text, cls = "", title = "") => { v.textContent = text; v.className = `v ${cls}`; v.title = title; };
const legend = (v, items) => {
  v.replaceChildren(el("div", "dim", "last min"), ...items.map(([t, text, col]) => {
    const d = el("div");
    const i = el("i", "sw"); i.style.background = col;
    d.append(i, `${t} ${text}`);
    return d;
  }));
  v.className = "v legend";
};

/** The NAS section for the Machines tab: `agent` the lead's status, `macs` the page's Macs (name,
 * colour, fresh). The same element each call, filled in place. */
export function nasSection(agent, macs) {
  seen = { agent, macs };
  build();
  return root;
}

function build() {
  const { agent, macs } = seen;
  const view = agent?.nas;
  const shares = agent?.nas_share || {};
  const haveShares = Object.keys(shares).length > 0;
  const lead = agent?.host;
  const cards = [nasCard(view, lead, macs, haveShares ? shares : null)];
  for (const m of macs) cards.push(macCard(m));
  const head = el("h2", null, "NAS");
  head.append(el("span", null, view ? nasSub(view) : "the lead hasn't measured it yet"));
  const grid = el("div", "machines");
  grid.append(...cards);
  root.replaceChildren(head, grid);
  paint();
}

function nasSub(v) {
  const l = v.limits;
  const bits = [];
  if (l && l.bytes_s) bits.push(`${fmt(l.bytes_s / MB, "MB/s")} and ${Math.round(l.creates_s)} new files/s by the probes`);
  if (v.load_pct != null) bits.push(`disks ${Math.round(v.load_pct)}% busy of a ${v.target_pct}% target`);
  return bits.join(" · ");
}

// A gauge row kept across renders: its bar and words (set by `gauge`).
function gauge(key, label, title) {
  let g = rows.get(key);
  if (!g) {
    const b = el("div", "cbar"); const i = el("i"); b.append(i);
    const d = el("div", "cap");
    const v = el("span", "v");
    d.append(el("span", "k", label), b, v);
    if (title) d.title = title;
    g = { row: d, bar: b, i, v };
    rows.set(key, g);
  }
  return g;
}
const set = (g, used, whole, text, cls = "") => {
  g.i.style.width = `${Math.round(Math.max(0, Math.min(1, whole ? used / whole : 0)) * 100)}%`;
  g.bar.className = `cbar ${cls}`;
  g.v.textContent = text;
};

function nasCard(v, lead, macs, shares) {
  const card = el("div", "mc");
  const top = el("div", "top");
  const chip = el("span", `chip ${v?.held ? "warn" : "ok"}`, v ? (v.held ? "gate shut" : "gate open") : "not known");
  top.append(el("span", "name", "The NAS"), el("span", "role", lead ? `measured by ${lead}` : ""), chip, el("span", "right", ""));
  card.append(top);
  // Use against its capacity: what the pool moves now against the limits its probes give.
  const l = v?.limits, t = v?.traffic;
  const caps = el("div", "caps");
  const lim = (key, label, used, limit, fmtUsed, fmtLimit) => {
    const g = gauge(key, label);
    if (!l || limit == null) set(g, 0, 0, "not probed");
    else if (!limit) set(g, 0, 0, "not probed (no limit weighed)");
    else if (used == null) set(g, 0, limit, `– of ${fmtLimit(limit)}`);
    else set(g, used, limit, `${fmtUsed(used)} of ${fmtLimit(limit)}`, used / limit > 0.85 ? "warn" : "");
    return g.row;
  };
  const num = (x) => `${fmt(x, "")}/s`;
  caps.append(
    lim("lim.bytes", "data moved", t ? t[0] + t[2] : null, l?.bytes_s / MB, (x) => fmt(x, "MB/s"), (x) => fmt(x, "MB/s")),
    lim("lim.creates", "files created", t ? t[3] : null, l?.creates_s, num, num),
    lim("lim.listed", "entries listed", t ? t[4] : null, l?.listed_s, num, num),
    lim("lim.stats", "files looked up", t ? t[5] : null, l?.stats_s, num, num));
  const g = (k, label, title) => caps.append(gauge(k, label, title).row);
  g("resp", "response time", "How long the lead's listing of the project's folder took (every 5 s while a build runs)");
  g("wait", "requests waiting", "The agents' NAS calls in flight, every Mac's together");
  g("missed", "timed out", "NAS calls past their deadline or refused (every thread busy), every Mac's, over the chart's range");
  g("disks", "NAS disks busy", "The busiest disk's busy share, smoothed, against the owner's target");
  card.append(caps);
  // Who is using the NAS: the share each Mac's running jobs are allowed, set by the lead from each job's measured use.
  const h3 = el("div", "small dim", "Who is using the NAS: each Mac's running jobs' share of the budget");
  h3.style.marginTop = "6px";
  const sc = el("div", "caps");
  let used = 0;
  for (const m of macs) {
    const w = shares ? Object.entries(shares).filter(([nm]) => isSlotOf(nm, m.name)).reduce((x, [, y]) => x + y, 0) : null;
    if (w == null) continue;
    used += w;
    const d = bar(m.name, w, 1000, `${Math.round(w / 10)}%`, w > 1000 ? "warn" : "");
    sc.append(d);
  }
  if (sc.children.length) { sc.append(bar("free", Math.max(0, 1000 - used), 1000, `${Math.round(Math.max(0, 1000 - used) / 10)}%`, "")); card.append(h3, sc); }
  card.append(el("div", "chips", ""));
  card.lastChild.id = "nas-chips";
  card.lastChild.style.cssText = "display:flex;flex-wrap:wrap;gap:4px;margin-top:6px";
  const live = el("div", "live");
  const head = el("div", "lhead");
  const said = el("span", "dim");
  head.append(el("b", null, "NAS over time"), said);
  const body = el("div", "lbody");
  body.append(...["lat", "io", "files", "disks"].map((k) => row(`pool.${k}`, { lat: "Answer", io: "Traffic", files: "Files", disks: "Disks" }[k], k === "io").row));
  live.append(head, body);
  live.dataset.lead = lead || "";
  live.dataset.kind = "pool";
  card.append(live);
  return card;
}

function macCard(m) {
  const card = el("div", `mc ${m.fresh ? "" : "off"}`);
  card.style.borderLeft = `3px solid ${m.colour}`;
  const top = el("div", "top");
  top.append(el("span", "name", m.name));
  card.append(top);
  const live = el("div", "live");
  const head = el("div", "lhead");
  head.append(el("b", null, "Live"), el("span", "dim"));
  const body = el("div", "lbody");
  body.append(...[["io", "Transfers", true], ["ms", "Answer"], ["calls", "Calls"]].map(([k, t, tall]) => row(`${m.name}.${k}`, t, tall).row));
  live.append(head, body);
  live.dataset.host = m.name;
  card.append(live);
  return card;
}

// The card's footer words: why there's no chart, or the range and how long the Mac was silent.
function note(head, st, gapList) {
  const said = head.querySelector("span.dim") || head.lastChild;
  if (!st?.series) { said.textContent = st?.error ? `no live measures: ${st.error}` : "asking…"; return false; }
  said.textContent = footer(st.range, st.step, gapList);
  return true;
}

// The live-driven gauges and the chips under them: the lead's answer time, every Mac's calls in flight
// and those missed over the chart's range, the disks' busy share.
function gauges() {
  const { agent, macs } = seen;
  const v = agent?.nas;
  const lead = liveOf(agent?.host);
  const now = lead?.now;
  const ms = lead?.series ? latest(lead.series.nas_ms, now, 30) : null;
  set(gauge("resp"), ms ?? 0, 400, ms == null ? "not measured (no build on the lead now)" : fmt(ms, "ms"), ms != null && ms > 200 ? "warn" : "");
  let wait = null, missed = null, range = 0;
  for (const m of macs) {
    const st = liveOf(m.name);
    if (!st?.series) continue;
    const w = latest(st.series.nas_calls, st.now, 30);
    if (w != null) wait = (wait || 0) + w;
    if (st.series.nas_missed) { missed = (missed || 0) + st.series.nas_missed.reduce((t, p) => t + p[2] * st.step, 0); range = Math.max(range, st.range); }
  }
  set(gauge("wait"), wait ?? 0, Math.max(12, wait ?? 0), wait == null ? "not measured" : `${fmt(wait)} in flight`);
  const mc = missed == null ? null : Math.round(missed);
  set(gauge("missed"), mc ?? 0, 10, mc == null ? "not measured" : `${mc} in the last ${spanText(range)}`, mc ? "warn" : "");
  const d = v?.load_pct;
  set(gauge("disks"), d ?? 0, 100, d == null ? "not read" : `${Math.round(d)}% (target ${v.target_pct}%)`, v?.held ? "warn" : "");
  const chips = root.querySelector("#nas-chips");
  if (!chips) return;
  const probe = v?.probes?.length ? Math.max(...v.probes.map((p) => p.at)) : null;
  const nowS = Math.floor(Date.now() / 1000);
  const fresh = macs.filter((m) => m.fresh).length;
  const list = [[probe ? `capacity measured ${spanText(Math.max(60, nowS - probe))} ago` : "capacity not measured", ""],
    [v?.held ? "sharing limits on: the gate is shut" : v?.limits ? "sharing limits on" : "no limits yet", v?.held ? "warn" : v?.limits ? "ok" : ""],
    [`${fresh} ${fresh === 1 ? "Mac" : "Macs"} connected`, ""],
    [mc == null ? "timeouts not measured" : mc ? `${mc} requests timed out` : "no requests timed out", mc ? "warn" : mc === 0 ? "ok" : ""]];
  chips.replaceChildren(...list.map(([t, c]) => el("span", `chip ${c}`, t)));
}

function paint() {
  if (!seen || !root.isConnected) return;
  gauges();
  const nr = css("--nas-r"), nw = css("--nas-w"), nd = css("--net-d"), warn = css("--warn"), acc = css("--accent");
  const gapsOf = (st) => commonGaps(["load1", "cores_busy", "mem_free_pct"].map((k) => ({ points: st.series[k], step: st.step })), st.now);
  for (const live of root.querySelectorAll(".live")) {
    const pool = live.dataset.kind === "pool";
    const host = pool ? live.dataset.lead : live.dataset.host;
    const st = liveOf(host);
    const body = live.querySelector(".lbody");
    const head = live.querySelector(".lhead");
    const ok = !!st?.series;
    const gl = ok ? gapsOf(st) : [];
    note(head, st, gl);
    body.hidden = !ok;
    if (!ok) continue;
    const s = st.series, now = st.now;
    const base = { from: now - st.range, to: now, step: st.step, gaps: gl };
    const at = (k, fresh) => latest(s[k], now, fresh);
    if (pool) {
      const r = (k) => rows.get(`pool.${k}`);
      draw(r("lat").c, { ...base, unit: "ms", series: [{ points: s.nas_ms, colour: nr, label: "answer", band: true }] });
      const lm = at("nas_ms", 30);
      words(r("lat").v, lm == null ? "not measured (no build here now)" : `${fmt(lm, "ms")} a listing`, lm != null && lm > 1000 ? "bad" : lm != null && lm > 200 ? "warn" : "");
      draw(r("io").c, { ...base, axis: true, unit: "MB/s", series: [
        { points: s.pool_nas_read_mbs, colour: nr, label: "read" }, { points: s.pool_nas_write_mbs, colour: nw, label: "written" }] });
      legend(r("io").v, [["↓", fmt(recentMean(s.pool_nas_read_mbs, now), "MB/s"), nr], ["↑", fmt(recentMean(s.pool_nas_write_mbs, now), "MB/s"), nw]]);
      // (Entries a second, per kind: opened to read, made, listed; a listing's entries dwarf the rest.)
      draw(r("files").c, { ...base, unit: "/s", series: [
        { points: s.pool_nas_files_s, colour: nr, label: "opened" }, { points: s.pool_nas_creates_s, colour: nw, label: "made" }, { points: s.pool_nas_listed_s, colour: nd, label: "listed" }] });
      words(r("files").v, `${fmt(recentMean(s.pool_nas_files_s, now), "")} opened · ${fmt(recentMean(s.pool_nas_creates_s, now), "")} made · ${fmt(recentMean(s.pool_nas_listed_s, now), "")} listed /s`, "", "The pool's jobs' files a second over the last minute: opened to read, made, entries listed");
      const target = seen.agent?.nas?.target_pct;
      draw(r("disks").c, { ...base, unit: "%", max: 100, refs: target ? [{ v: target, label: `${target}% target` }] : [], series: [{ points: s.nas_disk_busy_pct, colour: warn, fill: true, label: "busiest disk", unit: "%" }] });
      const d = at("nas_disk_busy_pct", 60);
      words(r("disks").v, d == null ? "not read (no SNMP reading)" : `${fmt(d, "%")} busiest disk`, target && d != null && d > target ? "warn" : "");
    } else {
      const r = (k) => rows.get(`${host}.${k}`);
      draw(r("io").c, { ...base, axis: true, unit: "MB/s", series: [
        { points: s.nas_read_mbs, colour: nr, label: "read" }, { points: s.nas_write_mbs, colour: nw, label: "written" }] });
      legend(r("io").v, [["↓", fmt(recentMean(s.nas_read_mbs, now), "MB/s"), nr], ["↑", fmt(recentMean(s.nas_write_mbs, now), "MB/s"), nw]]);
      const ms = at("nas_ms", 30);
      draw(r("ms").c, { ...base, unit: "ms", series: [{ points: s.nas_ms, colour: nr, label: "answer", band: true }] });
      words(r("ms").v, ms == null ? "not measured (no build here now)" : `${fmt(ms, "ms")} a listing`, ms != null && ms > 1000 ? "bad" : ms != null && ms > 200 ? "warn" : "", "How long the agent's NAS listing took (measured while a build runs here)");
      draw(r("calls").c, { ...base, unit: "", series: [{ points: s.nas_calls, colour: acc, label: "in flight", band: true }] });
      // (A rate a second: the last minute's mean times 60 is its count.)
      const missed = Math.round((recentMean(s.nas_missed, now) || 0) * 60);
      words(r("calls").v, `${fmt(at("nas_calls", 30))} in flight${missed ? ` · ${missed} missed in the last min` : ""}`, missed ? "warn" : "", "The agent's NAS calls under their deadlines: in flight now; missed: past the deadline or refused (every thread busy)");
    }
  }
}

onLive(() => requestAnimationFrame(paint));
