// A job's page's sums (job.js, task #1105; tested by web/test/jobview.test.mjs): its times as T+
// since the job's start or as local clock times, the axis's ticks in either, its record's figures
// against its step's medians, how it ended and why, and where each of its lanes' items goes.
import { pad } from "./words.js";

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const pad3 = (v) => String(v).padStart(3, "0");

/** A time `dt` s from the job's start: "T+05:01", "T+05:01.158" with `ms`, "T+1:05:01" past an hour
 * or with `hours` (a run an hour or longer: every label alike); before its start "T−00:03". */
export function tplus(dt, { hours = false, ms = false } = {}) {
  const neg = dt < 0;
  const q = Math.round(Math.abs(dt) * (ms ? 1000 : 1));
  const s = ms ? Math.floor(q / 1000) : q;
  const hh = Math.floor(s / 3600), mm = Math.floor((s % 3600) / 60), ss = s % 60;
  const head = hours || hh ? `${hh}:${pad(mm)}` : pad(mm);
  return `T${neg ? "−" : "+"}${head}:${pad(ss)}${ms ? `.${pad3(q % 1000)}` : ""}`;
}

/** A local clock time: "14:05:09", "14:05:09.158" with `ms`, its day first with `date`
 * ("Wed 12 Nov 23:59:58"). */
export function clockText(t, { ms = false, date = false } = {}) {
  const q = Math.round(t * (ms ? 1000 : 1)) * (ms ? 1 : 1000);
  const d = new Date(q);
  const day = date ? `${DAYS[d.getDay()]} ${d.getDate()} ${MONTHS[d.getMonth()]} ` : "";
  return `${day}${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}${ms ? `.${pad3(d.getMilliseconds())}` : ""}`;
}

/** Whether [t0, t1] crosses a local midnight. */
export const crossesMidnight = (t0, t1) => new Date(t0 * 1000).toDateString() !== new Date(t1 * 1000).toDateString();

/** A length in words: "158 ms", "4.2 s", "38 s", "5 min 01 s", "1 h 05 min". */
export function dur(s) {
  if (s == null || !Number.isFinite(s)) return "–";
  if (s < 1) return `${Math.round(s * 1000)} ms`;
  if (s < 10) return `${s.toFixed(1)} s`;
  if (s < 60) return `${Math.round(s)} s`;
  if (s < 3600) { const m = Math.floor(s / 60); return `${m} min ${pad(Math.round(s - m * 60))} s`; }
  const h = Math.floor(s / 3600); return `${h} h ${pad(Math.round((s - h * 3600) / 60))} min`;
}

// The axis's steps, s: from a millisecond (zoomed in) to a day.
const STEPS = [0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400];

/** How the page says a time (`mode` "rel": T+ from `origin`, the job's start; "clock": local times,
 * the day too when the run, [origin, end], crosses midnight): {at(t, ms), ticks(t0, t1, most)}, the
 * ticks the timeline's (timeline.js `ticks`: [{t, label, day}], at most `most`, on round steps: of
 * the job's time, or of the local day; milliseconds once a step is under a second; in clock times a
 * local midnight labelled with its day). Hours in every T+ label once the run is an hour long. */
export function timeWords(mode, origin, end) {
  const hours = end - origin >= 3600, date = crossesMidnight(origin, end);
  const at = (t, ms = true) => (mode === "clock" ? clockText(t, { ms, date }) : tplus(t - origin, { hours, ms }));
  const ticks = (t0, t1, most) => {
    const step = STEPS.find((s) => (t1 - t0) / s <= most) ?? 86400;
    const base = mode === "clock" ? -new Date(t0 * 1000).getTimezoneOffset() * -60 : origin;
    const out = [];
    for (let k = Math.ceil((t0 - base) / step - 1e-9), t = base + k * step; t <= t1 + 1e-9 && out.length < 2000; k++, t = base + k * step) {
      const ms = step < 1;
      if (mode !== "clock") { out.push({ t, label: tplus(t - origin, { hours, ms }), day: false }); continue; }
      const d = new Date(Math.round(t * 1000));
      const midnight = step >= 1 && !d.getHours() && !d.getMinutes() && !d.getSeconds();
      out.push({ t, label: midnight ? `${DAYS[d.getDay()]} ${d.getDate()} ${MONTHS[d.getMonth()]}` : clockText(t, { ms }).slice(0, step >= 60 && !ms ? 5 : undefined), day: midnight });
    }
    return out;
  };
  return { at, ticks, hours, date };
}

/** A record's figures (jobrt::timings::RunRec): wall and CPU (s), the cores busy (CPU over wall),
 * its peak memory (MB: the most of its phases', a sub-phase's too), its threads, its wait on the NAS
 * and the network (s, the main thread's top-level phases'); null without a record. */
export function recordFigures(rec) {
  if (!rec) return null;
  const all = (ps) => (ps || []).flatMap((p) => [p, ...all(p.sub)]);
  const peaks = all(rec.phases).map((p) => p.peak_mb || 0);
  const main = (rec.phases || []).filter((p) => !p.background);
  const wait = (k) => main.reduce((s, p) => s + (p.waits?.[k] || 0), 0);
  return {
    wall: rec.wall_s ?? null, cpu: rec.cpu_s ?? null, cores: rec.wall_s > 0 && rec.cpu_s != null ? rec.cpu_s / rec.wall_s : null,
    peak: peaks.length && Math.max(...peaks) > 0 ? Math.max(...peaks) : null, threads: rec.threads ?? null, nas: wait("nas_s"), net: wait("net_s"),
  };
}

/** The median of the numbers among `xs` (null: none). */
export function median(xs) {
  const v = xs.filter((x) => x != null && Number.isFinite(x)).sort((a, b) => a - b);
  if (!v.length) return null;
  const m = v.length >> 1;
  return v.length % 2 ? v[m] : (v[m - 1] + v[m]) / 2;
}

/** A step's medians over its records `runs` (`/api/timings`' `runs`, the kind's last), job `job`'s own
 * left out: {n, wall, cpu, cores, peak, threads, nas, net}. */
export function stepMedians(runs, job) {
  const fs = (runs || []).filter((r) => r.job !== job).map(recordFigures);
  const of = (k) => median(fs.map((f) => f[k]));
  return { n: fs.length, wall: of("wall"), cpu: of("cpu"), cores: of("cores"), peak: of("peak"), threads: of("threads"), nas: of("nas"), net: of("net") };
}

/** How job view `v` (`GET /api/job/<id>`) ended and why: {state: running|ok|failed|ended|unknown,
 * said, why: [words]}: its run's span's status, else its history's last end; why from its error,
 * its agent's exit, its history's notes, a pause. */
export function outcome(v) {
  const ev = v.events || [], hist = v.history || [];
  const end = [...hist].reverse().find((e) => ["end", "done", "fail", "cancel"].includes(e.kind));
  const root = (v.spans || []).find((s) => s.id === "job");
  const histOk = end ? end.ok !== false && end.kind !== "fail" && end.kind !== "cancel" : null;
  let state;
  if (!v.legacy && v.running) state = "running";
  else if (root) state = root.status === "fail" ? "failed" : "ok";
  else state = histOk == null ? "unknown" : histOk ? "ok" : "failed";
  const why = [];
  const add = (x) => { const t = (x || "").trim(); if (t && !why.includes(t)) why.push(t); };
  for (const e of ev) if (e.kind === "error") add(e.text);
  const exit = [...ev].reverse().find((e) => e.kind === "exit");
  if (exit) add(exit.text || (exit.ok === false ? "its agent says it failed" : ""));
  for (const e of hist) if (["end", "done", "fail", "cancel", "lapse"].includes(e.kind)) add(e.note || (e.kind === "cancel" ? "cancelled" : e.kind === "lapse" ? "its lease lapsed" : ""));
  if (ev.some((e) => e.kind === "paused")) add("paused at a safe point");
  if (root?.status === "cut" || (v.spans || []).some((s) => s.status === "cut")) add("spans were open as it ended (cut)");
  const said = { running: "running", ok: "finished", failed: "failed", unknown: "no end in the history" }[state];
  return { state, said, why };
}

// What each of a job's events is, as a marker lane's (job.js): its lane and words. Retries, safe
// points, pauses, take-overs and the end; the history's lease, start and end beside the job's own.
const MARKS = {
  retry: ["retries", "a retry"], safe: ["safe points", "safe points"], pausing: ["pauses", "asked to pause at its next safe point"], paused: ["pauses", "paused"],
  error: ["the end", "its error"], thinned: ["the end", "its spans thinned past their bound"], end: ["the end", "its end"], exit: ["the end", "its agent's word on its exit"],
};
/** A job's markers: [{lane, t, t1, kind, text, bad}] from its events (`v.events`) and its history
 * (its first lease the grant; a lease after it its run held again: a lapse, a lead's restart or its
 * agent's successor taking it over). */
export function markers(v) {
  const out = [];
  for (const e of v.events || []) {
    const m = MARKS[e.kind];
    if (!m || e.t == null) continue;
    const many = e.n > 1 ? ` (${e.n})` : "";
    out.push({ lane: m[0], t: e.t, t1: e.t1 ?? null, kind: e.kind, text: `${m[1]}${many}${e.text ? `: ${e.text}` : ""}`, bad: e.kind === "error" || e.ok === false || e.kind === "retry" });
  }
  let leases = 0;
  for (const e of v.history || []) {
    if (e.kind === "lease") out.push({ lane: "leases", t: e.t, t1: null, kind: leases++ ? "held again" : "lease", text: leases > 1 ? `held again under a new lease on ${e.worker || "?"} (a lapse, a restart or a take-over)` : `leased to ${e.worker || "?"}`, bad: false });
    else if (e.kind === "start") out.push({ lane: "leases", t: e.t, t1: null, kind: "start", text: `started on ${e.worker || "?"}`, bad: false });
    else if (["end", "done", "fail", "cancel", "lapse"].includes(e.kind)) out.push({ lane: e.kind === "lapse" ? "leases" : "the end", t: e.t, t1: null, kind: e.kind, text: `the history's ${e.kind}${e.note ? `: ${e.note}` : ""}`, bad: e.kind === "fail" || e.kind === "cancel" || e.kind === "lapse" || e.ok === false });
  }
  return out.sort((a, b) => a.t - b.t);
}
/** The marker lanes in order (those with any). */
export const MARK_LANES = ["leases", "retries", "safe points", "pauses", "the end"];

/** What a span waited on most, its colour's key: the NAS, the network, the disk, other work, or its
 * own CPU; "old" when it says neither. */
export function spanLook(s) {
  const a = s.attrs || {};
  const parts = [["nas", a.nas_s], ["net", a.net_s], ["disk", a.disk_s], ["other", a.other_s], ["cpu", a.cpu_s]].filter(([, x]) => x > 0);
  if (!parts.length) return "old";
  return parts.reduce((p, q) => (q[1] > p[1] ? q : p))[0];
}

/** The log's lines between t0 and t1 (a span's), by their times; lines without a time left out. */
export const linesIn = (lines, t0, t1) => (lines || []).filter((l) => l.t != null && l.t >= t0 - 1e-6 && l.t <= t1 + 1e-6);
