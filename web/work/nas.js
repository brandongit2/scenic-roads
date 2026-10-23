// The NAS on the build page's Machines tab (docs/pool.md §10, Live; docs/pipeline.md §5): a card
// for the NAS itself (what the lead's status says of it, `agent.nas`: the load of its disks against
// the owner's target, the gate, the limits its probes give; and over time the pool's traffic, its
// file work and the disks' busy share, from the lead's /work/live) and a card a Mac (its share of
// the NAS budget, its jobs' reads and writes, how long its listings take, its calls in flight and
// those that missed their deadline, from its own /work/live). The answers are live.js's, asked
// while the tab shows; every chart is chart.js's. A measure missing is said so, never zero.
import { draw } from "./chart.js";
import { commonGaps, fmt, footer, latest, recentMean } from "./chartmath.js";
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
  const cards = [nasCard(view, lead)];
  for (const m of macs) cards.push(macCard(m, haveShares ? Object.entries(shares).filter(([nm]) => isSlotOf(nm, m.name)).reduce((t, [, w]) => t + w, 0) : null));
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

function nasCard(v, lead) {
  const card = el("div", "mc");
  const top = el("div", "top");
  const chip = el("span", `chip ${v?.held ? "warn" : "ok"}`, v ? (v.held ? "gate shut" : "gate open") : "not known");
  top.append(el("span", "name", "The NAS"), el("span", "role", lead ? `measured by ${lead}` : ""), chip);
  card.append(top);
  const caps = el("div", "caps");
  if (v?.load_pct != null) caps.append(bar("disks", v.load_pct, 100, `${Math.round(v.load_pct)}% busy (smoothed) · target ${v.target_pct}%`, v.held ? "warn" : "", v.target_pct));
  if (v?.traffic) caps.append(bar("pool now", v.traffic[0] + v.traffic[2], Math.max(1, (v.limits?.bytes_s || 0) / MB), `${fmt(v.traffic[0], "MB/s")} read · ${fmt(v.traffic[2], "MB/s")} written`, "", null));
  if (caps.children.length) card.append(caps);
  const live = el("div", "live");
  const head = el("div", "lhead");
  const said = el("span", "dim");
  head.append(el("b", null, "Pool"), said);
  const body = el("div", "lbody");
  body.append(...["io", "files", "disks"].map((k) => row(`pool.${k}`, { io: "Traffic", files: "Files", disks: "Disks" }[k], k === "io").row));
  live.append(head, body);
  live.dataset.lead = lead || "";
  live.dataset.kind = "pool";
  card.append(live);
  return card;
}

function macCard(m, share) {
  const card = el("div", `mc ${m.fresh ? "" : "off"}`);
  card.style.borderLeft = `3px solid ${m.colour}`;
  const top = el("div", "top");
  top.append(el("span", "name", m.name), share != null ? el("span", `chip ${share > 1000 ? "warn" : ""}`, `${Math.round(share / 10)}% of the budget`) : el("span", "role", "no share known"));
  card.append(top);
  if (share != null) {
    const caps = el("div", "caps");
    caps.append(bar("share", share, 1000, `its jobs' weights, of the whole ${share > 1000 ? "(over it)" : ""}`.trim(), share > 1000 ? "warn" : ""));
    card.append(caps);
  }
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

function paint() {
  if (!seen || !root.isConnected) return;
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
