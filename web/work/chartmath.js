// The charts' sums, apart from the canvas so Node tests them (web/test/chartmath.test.mjs): scales,
// units, time ticks, the gaps where a Mac slept, a chart's footer, and merging /work/live's
// answers (docs/pool.md §10, Live: a point is [t, least, mean, most]).

// A scale's top: the next of 1, 2, 2.5, 5 times a power of ten at or above `v` (1 for none).
export function niceMax(v) {
  if (!(v > 0) || !Number.isFinite(v)) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  const f = v / p;
  return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 2.5 ? 2.5 : f <= 5 ? 5 : 10) * p;
}

// The lines across a scale up to `max` (a niceMax): 0 and two to four between, ending at max.
export function ticks(max) {
  const p = 10 ** Math.floor(Math.log10(max));
  const f = Math.round((max / p) * 100) / 100;
  const step = (f === 1 ? 0.5 : f === 2 ? 1 : f === 2.5 ? 0.5 : f === 5 ? 2.5 : 5) * p;
  const out = [];
  for (let v = 0; v <= max + step / 1000; v += step) out.push(Math.round(v / step) * step);
  return out;
}

const fix = (v, d) => {
  const s = v.toFixed(d);
  return d && s.includes(".") ? s.replace(/\.?0+$/, "") : s;
};
const digits = (v) => (Math.abs(v) < 10 ? 1 : 0);

// A value in its unit, as short as it reads (metric; the store's MB are 2^20 bytes, as it counts):
// "MB/s" (kB/s below 1, GB/s from 1024), "GB" (TB from 1024), "MB" (GB from 1024), "B" (a count of
// bytes: kB, MB, GB, TB), "ms" (s from 1000), "%", "cores", "" (a plain number). Null: "–".
export function fmt(v, unit = "") {
  if (v == null || !Number.isFinite(v)) return "–";
  const neg = v < 0 ? "-" : "";
  const a = Math.abs(v);
  const say = (x, u) => `${neg}${fix(x, digits(x))}${u ? ` ${u}` : ""}`;
  switch (unit) {
    case "MB/s":
      if (a === 0) return "0 MB/s";
      if (a < 1) return say(a * 1024, "kB/s");
      return a >= 1024 ? say(a / 1024, "GB/s") : say(a, "MB/s");
    case "GB":
      return a >= 1024 ? say(a / 1024, "TB") : say(a, "GB");
    case "MB":
      return a >= 1024 ? say(a / 1024, "GB") : say(a, "MB");
    case "B": {
      const u = ["B", "kB", "MB", "GB", "TB"];
      let x = a, k = 0;
      while (x >= 1024 && k < u.length - 1) { x /= 1024; k++; }
      return k ? say(x, u[k]) : `${neg}${Math.round(x)} B`;
    }
    case "ms":
      return a >= 1000 ? say(a / 1000, "s") : `${neg}${Math.round(a)} ms`;
    case "%":
      return `${neg}${Math.round(a)}%`;
    case "cores":
      return say(a, a === 1 ? "core" : "cores");
    default:
      return say(a, unit);
  }
}

// A span of seconds as the footer's range says it: "1 min", "15 min", "1 h", "72 h".
export function spanText(s) {
  if (s < 60) return `${Math.round(s)} s`;
  if (s < 3600) return `${Math.round(s / 60)} min`;
  return `${fix(s / 3600, s % 3600 ? 1 : 0)} h`;
}

// A point's resolution in words: "a point a second", "a point every 10 s", "a point a minute".
export const stepText = (step) => (step <= 1 ? "a point a second" : step === 60 ? "a point a minute" : `a point every ${spanText(step)}`);

// Where a series has no points for longer than its points' spacing allows (the Mac asleep, or its
// agent stopped): [{from, to}], between points and from the last to `now`. A gap is more than two
// steps and 5 s without a point (a sampler's second skipped is no gap).
export const gapOver = (step) => Math.max(2 * step, step + 5);
export function gaps(points, step, now) {
  const out = [];
  const over = gapOver(step);
  for (let i = 1; i < points.length; i++) if (points[i][0] - points[i - 1][0] > over) out.push({ from: points[i - 1][0] + step, to: points[i][0] });
  const last = points[points.length - 1];
  if (last && now != null && now - last[0] > over) out.push({ from: last[0] + step, to: now });
  return out;
}

// The gaps of several series together: only where every one with points is silent (one series
// sampled every 10 s doesn't make the others' seconds a gap). `list` is [{points, step}].
export function commonGaps(list, now) {
  const withPts = list.filter((x) => x.points?.length);
  if (!withPts.length) return [];
  let out = gaps(withPts[0].points, withPts[0].step, now);
  for (const x of withPts.slice(1)) {
    const g = gaps(x.points, x.step, now);
    out = out.flatMap((a) => g.map((b) => ({ from: Math.max(a.from, b.from), to: Math.min(a.to, b.to) })).filter((c) => c.to > c.from));
  }
  return out;
}

// A series split into runs at its gaps (each run drawn as one line).
export function runs(points, step) {
  const out = [];
  let run = [];
  const over = gapOver(step);
  for (const p of points) {
    if (run.length && p[0] - run[run.length - 1][0] > over) { out.push(run); run = []; }
    run.push(p);
  }
  if (run.length) out.push(run);
  return out;
}

const pad = (v) => String(v).padStart(2, "0");
// A time of day for an axis: "14:05", or "14:05:30" when the ticks are under a minute apart
// (local time: `tz` minutes east of UTC, the browser's when left out).
export function hhmm(t, withS = false, tz = null) {
  const off = tz ?? -new Date(t * 1000).getTimezoneOffset();
  const s = ((Math.floor(t) + off * 60) % 86400 + 86400) % 86400;
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}${withS ? `:${pad(s % 60)}` : ""}`;
}

const TICK_STEPS = [10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400];
// The time axis's ticks over [from, to]: the first of TICK_STEPS giving at most `most` ticks, on
// round times (by local time: `tz` as hhmm's), each {t, label}.
export function timeTicks(from, to, most = 5, tz = null) {
  const span = Math.max(1, to - from);
  const step = TICK_STEPS.find((s) => span / s <= most) ?? 86400;
  const off = (tz ?? -new Date(from * 1000).getTimezoneOffset()) * 60;
  const out = [];
  for (let t = Math.ceil((from + off) / step) * step - off; t <= to; t += step) out.push({ t, label: hhmm(t, step < 60, tz) });
  return out;
}

// A chart's footer: its range, its points' resolution and how long its Mac was silent in it:
// "last 1 h · a point a second · asleep or away 12 min".
export function footer(range, step, gapList = []) {
  const silent = gapList.reduce((s, g) => s + (g.to - g.from), 0);
  return [`last ${spanText(range)}`, stepText(step), silent >= 60 ? `asleep or away ${spanText(silent)}` : null].filter(Boolean).join(" · ");
}

// What a chart shows of its series in [from, to]: the scale's top (the most of the means, or of the
// most when `band`; at least `least`), from the points' means or most.
export function topOf(seriesList, { band = false, least = 0, stacked = false } = {}) {
  let top = least;
  if (stacked) {
    const sums = new Map();
    for (const s of seriesList) for (const p of s || []) sums.set(p[0], (sums.get(p[0]) || 0) + p[2]);
    for (const v of sums.values()) top = Math.max(top, v);
  } else for (const s of seriesList) for (const p of s || []) top = Math.max(top, band ? p[3] : p[2]);
  return niceMax(top);
}

// /work/live's answers kept: `have` (an earlier answer's `series`, or null) and `ans` (an answer to
// `{since}` or `{range}`), the points before `from` dropped; each series' points from the first new
// one's time on replaced (the last one asked again may have grown). Returns a new series map.
export function merge(have, ans, from) {
  const out = {};
  const names = new Set([...Object.keys(have || {}), ...Object.keys(ans?.series || {})]);
  for (const n of names) {
    const old = have?.[n] || [];
    const add = ans?.series?.[n] || [];
    const cut = add.length ? add[0][0] : Infinity;
    let kept = old.filter((p) => p[0] >= from && p[0] < cut);
    kept = kept.concat(add);
    if (kept.length) out[n] = kept;
  }
  return out;
}

// The newest mean of a series within `fresh` seconds of `now` (null: none: never zero).
export function latest(points, now, fresh = 30) {
  const p = points?.[points.length - 1];
  return p && now - p[0] <= fresh ? p[2] : null;
}

// The memory pressure's level (kern.memorystatus: 1 normal, 2 warning, 4 critical) as a class.
export const pressureClass = (v) => (v == null ? "" : v >= 4 ? "bad" : v >= 2 ? "warn" : "ok");
export const pressureWord = (v) => (v == null ? "pressure not measured" : v >= 4 ? "pressure critical" : v >= 2 ? "pressure raised" : "pressure normal");
