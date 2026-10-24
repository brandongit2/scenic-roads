// The Activity tab's transfers (docs/pool.md §10, Live; docs/pipeline.md §5), as the owner's mockup
// lays them out (docs/mockups/build-live.html, "File stream" and "Internet by website"): what each
// Mac's jobs are moving now (the NAS files they opened or made and the internet transfers that
// ended, newest first) and the internet by website (each host's requests and MB a second over the
// last `RATE_S`, the connections open now, retries and 429 holds since each Mac's agent started,
// the budget it is metered by, its robots.txt). Each Mac a chip in its colour on the page. From each Mac's /work/live (`files` and `net`), asked every `EVERY_MS`
// while the tab shows: this page's own Mac through its own calls, the others at their addresses
// (live.js's `remote`). A host's robots.txt is as a job fetching its pages told its Mac's proxy
// (crate::budget::net::Robots), the newest told of any Mac; one never told is an API's or a data
// download's, which follow their provider's policy: no robots.txt applies (docs/inputs.md §6.9).
import { remote } from "./live.js";

const EVERY_MS = 2000;
// The span the websites' rates are taken over (s).
const RATE_S = 30;
// The stream's rows shown.
const ROWS = 60;
const MB = 1e6;

const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };

let cfg = null;
// Per host: {files, net, hist: [[t (ms), hosts]], error, asking, url}.
const kept = new Map();
let ownHost = null;

const root = el("div", "d-xfer");
const parts = {};
{
  const stream = el("div", "box");
  parts.streamSay = el("span");
  const sh = el("h3", null, "File stream ");
  sh.append(parts.streamSay);
  stream.append(sh, (parts.log = el("div", "xlog")));
  const hosts = el("div", "box");
  parts.hostsSay = el("span");
  const table = el("table", "t xhosts");
  const head = el("tr");
  for (const [t, c] of [["website"], ["requests/s", "num"], ["MB/s", "num"], ["open", "num"], ["retries", "num"], ["rate limit"], ["robots.txt"]]) head.append(el("th", c, t));
  table.append(el("thead", null), (parts.body = el("tbody")));
  table.tHead.append(head);
  const wrap = el("div", "tablewrap");
  wrap.append(table);
  const hh = el("h3", null, "Internet by website ");
  hh.append(parts.hostsSay);
  hosts.append(hh, wrap, (parts.none = el("div", "small dim")));
  root.append(stream, hosts);
}
parts.streamSay.textContent = "what each Mac is moving right now";
parts.hostsSay.textContent = "every job's downloads and requests, per website";

/** The transfers' part, filled in place as answers come (dash.js puts it in the Activity tab). */
export function transfersPart() {
  requestAnimationFrame(paint);
  return root;
}

/** Starts asking: `call(path, body)` this page's own Mac; `macs()` the swarm's members' addresses;
 * `machines()` the page's Macs (dash.js's model: name, colour, status); `active()` whether the
 * Activity tab is shown. */
export function startActivity(c) {
  cfg = c;
  setInterval(tick, EVERY_MS);
  document.addEventListener("visibilitychange", tick);
  window.addEventListener("hashchange", tick);
  tick();
}

function tick() {
  if (!cfg || document.visibilityState !== "visible" || !cfg.active()) return;
  ask(null);
  for (const m of cfg.macs()) if (m.host !== ownHost) ask(m);
}

async function ask(m) {
  const host = m ? m.host : ownHost || "";
  const st = kept.get(host) || { hist: [] };
  if (st.asking) return;
  st.asking = true;
  kept.set(host, st);
  // (No series: the stream and the counts alone.)
  const body = { range: 60, series: [] };
  try {
    let ans;
    if (!m) {
      const [code, v] = await cfg.call("/work/live", body);
      if (code !== 200 || !v) throw new Error(code === 503 ? "not measuring yet" : `answered ${code}`);
      ans = v;
      if (!ownHost) { ownHost = ans.host; kept.delete(""); kept.set(ownHost, st); }
    } else ans = await remote(m, st, body);
    const now = Date.now();
    st.files = ans.files || null;
    st.net = ans.net || null;
    if (st.net) {
      st.hist.push([now, st.net.hosts]);
      while (st.hist.length > 2 && now - st.hist[1][0] >= RATE_S * 1000) st.hist.shift();
    }
    st.error = null;
  } catch (e) {
    st.error = e.name === "TimeoutError" ? "no answer in 5 s" : e.message;
  } finally {
    st.asking = false;
  }
  if (!queued) { queued = true; requestAnimationFrame(() => { queued = false; paint(); }); }
}
let queued = false;

const bytes = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : b >= 1e6 ? `${(b / 1e6).toFixed(1)} MB` : b >= 1e3 ? `${Math.round(b / 1e3)} kB` : `${b} B`);
const clock = (ms) => new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
const short = (host) => host.replace(/\.local$/, "");
// A Mac's chip: in its colour on the build page (dash.js's MACHINE_COLOURS), its name its chip's
// (M4 Pro), else its host's.
function macChip(host) {
  const m = cfg?.machines?.().find((x) => x.name === host);
  const c = el("span", "mac", m?.status?.resources?.chip || short(host));
  if (m?.colour) c.style.color = m.colour;
  c.title = short(host);
  return c;
}
// A host's robots.txt cell: its state as told (module doc), when it was read on hover.
const ROBOTS = { unchecked: ["not checked yet", "warn"], allowed: ["allowed", "ok"], disallowed: ["disallowed", "warn"] };
function robotsCell(r) {
  if (!r) return Object.assign(el("td", "small dim", "n/a: API or data"), { title: "An API or a data download: it follows its provider's usage policy, no robots.txt read" });
  const [say, cls] = ROBOTS[r.state] || [r.state, "warn"];
  const hm = new Date(r.t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  return Object.assign(el("td", `small ${cls}`, r.state === "unchecked" ? say : `${say} (checked ${hm})`), { title: r.state === "unchecked" ? `Its pages are to be fetched; robots.txt not read yet (told ${clock(r.t)})` : `robots.txt read at ${clock(r.t)}` });
}
const num = (v, d = 1) => (v ? v.toFixed(d) : "0");

// The answers' rows: the NAS files and the internet transfers, each with its Mac, newest first.
function rows() {
  const out = [];
  for (const [host, st] of kept) {
    for (const f of st.files || []) out.push({ t: f.t, host, kind: f.kind === "wrote" ? "put" : "get", what: f.path, b: f.bytes, ms: f.ms });
    for (const e of st.net?.events || []) out.push({ t: e.t, host, kind: e.outcome === "ok" ? "fetch" : "bad", what: e.host, b: e.in + e.out, ms: e.ms, outcome: e.outcome });
  }
  return out.sort((a, b) => b.t - a.t).slice(0, ROWS);
}

const VERB = { put: "wrote to NAS", get: "read from NAS", fetch: "downloaded" };
const BROKE = { broken: "broken off", unreached: "unreachable", refused: "no connection to be had" };

function paint() {
  if (!root.isConnected) return;
  const asked = [...kept.values()];
  const answering = asked.filter((s) => !s.error && (s.files || s.net)).length;
  const errs = [...kept].filter(([, s]) => s.error).map(([h, s]) => `${short(h)}: ${s.error}`);
  const rs = rows();
  parts.log.replaceChildren(...rs.map((r) => {
    const d = el("div");
    d.append(el("span", "t", `${clock(r.t)} `), macChip(r.host), " ", el("span", r.kind, r.kind === "bad" ? `${BROKE[r.outcome] || r.outcome}: ` : `${VERB[r.kind]} `), el("b", null, r.what));
    // (A file's bytes are its length; an open's time is the open's, a copy's the copy's.)
    if (r.b || r.ms) d.append(` ${bytes(r.b)}${r.ms ? ` in ${r.ms} ms${r.kind !== "get" && r.b ? ` (${(r.b / MB / (r.ms / 1000)).toFixed(1)} MB/s)` : ""}` : ""}`);
    d.title = `${new Date(r.t).toLocaleString()} · ${short(r.host)} · ${r.what}`;
    return d;
  }));
  const streaming = asked.filter((s) => !s.error && s.files).length;
  if (!rs.length) parts.log.append(el("div", "dim", !kept.size ? "asking the Macs…" : streaming ? "nothing moved lately: no job opened or made a NAS file or ended a download"
    : answering ? "no file stream: the Macs' apps are older" : "no Mac answers"));
  parts.streamSay.textContent = `what each Mac is moving right now${errs.length ? ` · ${errs.join(" · ")}` : ""}`;
  // The websites: each Mac's counts now and RATE_S ago, summed by host.
  const by = new Map();
  for (const st of asked) {
    const h = st.hist;
    if (!h?.length) continue;
    const [t1, now] = h[h.length - 1], [t0, was] = h[0];
    const dt = (t1 - t0) / 1000;
    for (const [name, c] of Object.entries(now)) {
      const w = was[name] || { in: 0, out: 0, requests: 0 };
      const r = by.get(name) || { req: 0, mbs: 0, conns: 0, open: 0, retries: 0, holds: 0, limit: c.limit, rated: false };
      // (An agent restarted between: its counts began again, its rate from them alone.)
      const from = c.requests < w.requests || c.in + c.out < w.in + w.out ? { in: 0, out: 0, requests: 0 } : w;
      if (dt > 0) { r.req += (c.requests - from.requests) / dt; r.mbs += (c.in + c.out - from.in - from.out) / MB / dt; r.rated = true; }
      r.conns += c.conns; r.open += c.open ?? 0; r.retries += c.retries; r.holds += c.holds;
      if (c.robots && !(r.robots?.t > c.robots.t)) r.robots = c.robots;
      by.set(name, r);
    }
  }
  const list = [...by].sort((a, b) => b[1].mbs - a[1].mbs || b[1].req - a[1].req || b[1].open - a[1].open || b[1].conns - a[1].conns);
  parts.body.replaceChildren(...list.map(([name, r]) => {
    const tr = el("tr");
    tr.append(el("td", "mono", name), el("td", "num", r.rated ? num(r.req) : "…"), el("td", "num", r.rated ? num(r.mbs, 2) : "…"), Object.assign(el("td", "num", String(r.open)), { title: `${r.conns} opened since each Mac's agent started` }),
      el("td", `num ${r.retries ? "warn" : ""}`, String(r.retries)), el("td", `small ${r.holds ? "warn" : "dim"}`, [r.limit || "—", r.holds ? `${r.holds} held back (429)` : ""].filter(Boolean).join(" · ")), robotsCell(r.robots));
    return tr;
  }));
  parts.none.textContent = list.length ? `Rates over the last ${RATE_S} s; connections open now; retries and holds since each Mac's agent started.`
    : !kept.size ? "asking the Macs…" : answering ? "No internet traffic since the Macs' agents started." : "No Mac answers.";
}
