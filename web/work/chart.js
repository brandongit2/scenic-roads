// The build page's charts over time, on a canvas (its sums in chartmath.js, tested): series of
// /work/live's points ([t, least, mean, most]) over [from, to], a line a series (its least-to-most
// band faint when asked, filled or stacked when asked), the scale's lines and its top in the unit,
// reference lines (a Mac's cores), the gaps where its Mac was silent shaded, the time below when
// asked; under the pointer, the time and each series' value there.
import { fmt, hhmm, runs, ticks, timeTicks, topOf } from "./chartmath.js";

const opts = new WeakMap();
const hover = new WeakMap();

// The page's colours (index.html's tokens), read once a draw.
function tokens(el) {
  const cs = getComputedStyle(el);
  const v = (k, d) => cs.getPropertyValue(k).trim() || d;
  return { line: v("--line", "#262c33"), dim: v("--dim", "#8a939c"), text: v("--text", "#d7dce2"), panel: v("--panel", "#161a1f") };
}

// The canvas sized to its box at the screen's pixels: [context, width, height] in CSS pixels.
function fit(c) {
  const r = c.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(r.width * dpr)), hh = Math.max(1, Math.round(r.height * dpr));
  if (c.width !== w || c.height !== hh) { c.width = w; c.height = hh; }
  const g = c.getContext("2d");
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  return [g, r.width, r.height];
}

/** Draws chart `o` on canvas `c` (again on each call, and as the pointer moves over it):
 * `{from, to, step, series: [{points, colour, label, fill, band, unit}], stacked, unit, least, max,
 *   refs: [{v, label}], gaps: [{from, to}], axis, colourOf(point)}` — `colourOf` colours a line by
 * its points (memory by its pressure); `axis` draws the time and the scale's labels. */
export function draw(c, o) {
  opts.set(c, o);
  if (!c.dataset.hover) {
    c.dataset.hover = "1";
    const move = (e) => { const r = c.getBoundingClientRect(); hover.set(c, (e.clientX - r.left) / r.width); paint(c); };
    c.addEventListener("pointermove", move);
    c.addEventListener("pointerdown", move);
    c.addEventListener("pointerleave", () => { hover.delete(c); paint(c); });
  }
  paint(c);
}

function paint(c) {
  const o = opts.get(c);
  if (!o || !c.isConnected) return;
  const [g, W, H] = fit(c);
  const k = tokens(c);
  g.clearRect(0, 0, W, H);
  const bottom = o.axis ? 13 : 0;
  const top = o.axis ? 2 : 1;
  const ph = Math.max(1, H - bottom - top);
  const span = Math.max(1, o.to - o.from);
  const X = (t) => ((t - o.from) / span) * W;
  const lists = o.series.map((s) => s.points || []);
  const max = o.max ?? topOf(lists, { band: o.series.some((s) => s.band), least: Math.max(o.least || 0, ...(o.refs || []).map((r) => r.v)), stacked: o.stacked });
  const Y = (v) => top + ph - (Math.max(0, Math.min(max, v)) / max) * ph;
  g.font = "10px -apple-system, BlinkMacSystemFont, system-ui, sans-serif";
  g.textBaseline = "top";

  // Silent stretches: shaded, hatched lightly.
  g.fillStyle = k.line;
  for (const gp of o.gaps || []) {
    const x0 = Math.max(0, X(gp.from)), x1 = Math.min(W, X(gp.to));
    if (x1 - x0 < 0.5) continue;
    g.globalAlpha = 0.35;
    g.fillRect(x0, top, x1 - x0, ph);
    g.globalAlpha = 1;
  }
  // The scale's lines.
  if (o.axis) {
    g.strokeStyle = k.line;
    g.lineWidth = 1;
    for (const v of ticks(max)) {
      const y = Math.round(Y(v)) + 0.5;
      g.beginPath(); g.moveTo(0, y); g.lineTo(W, y); g.stroke();
    }
  }
  // The series: stacked fills in order, or each its line (and band, fill).
  const base = new Map();
  o.series.forEach((s) => {
    for (const run of runs(s.points || [], o.step)) {
      const lo = run.map((p) => (o.stacked ? base.get(p[0]) || 0 : 0));
      const hi = run.map((p, i) => lo[i] + p[2]);
      if (s.band && !o.stacked) {
        g.fillStyle = s.colour; g.globalAlpha = 0.18;
        g.beginPath();
        run.forEach((p, i) => (i ? g.lineTo(X(p[0]), Y(p[3])) : g.moveTo(X(p[0]), Y(p[3]))));
        for (let i = run.length - 1; i >= 0; i--) g.lineTo(X(run[i][0]), Y(run[i][1]));
        g.closePath(); g.fill(); g.globalAlpha = 1;
      }
      if (s.fill || o.stacked) {
        g.fillStyle = s.colour; g.globalAlpha = o.stacked ? 0.55 : 0.22;
        g.beginPath();
        run.forEach((p, i) => (i ? g.lineTo(X(p[0]), Y(hi[i])) : g.moveTo(X(p[0]), Y(hi[i]))));
        for (let i = run.length - 1; i >= 0; i--) g.lineTo(X(run[i][0]), Y(lo[i]));
        g.closePath(); g.fill(); g.globalAlpha = 1;
      }
      g.lineWidth = 1.25;
      g.lineJoin = "round";
      if (run.length === 1) { g.fillStyle = s.colour; g.fillRect(X(run[0][0]) - 1, Y(hi[0]) - 1, 2, 2); }
      for (let i = 1; i < run.length; i++) {
        g.strokeStyle = o.colourOf?.(run[i]) || s.colour;
        g.beginPath(); g.moveTo(X(run[i - 1][0]), Y(hi[i - 1])); g.lineTo(X(run[i][0]), Y(hi[i])); g.stroke();
      }
      if (o.stacked) run.forEach((p, i) => base.set(p[0], hi[i]));
    }
  });
  // Reference lines, dashed, labelled at their right.
  for (const r of o.refs || []) {
    if (r.v > max) continue;
    const y = Math.round(Y(r.v)) + 0.5;
    g.strokeStyle = k.dim; g.setLineDash([3, 3]); g.lineWidth = 1;
    g.beginPath(); g.moveTo(0, y); g.lineTo(W, y); g.stroke(); g.setLineDash([]);
    if (r.label && o.axis) { g.fillStyle = k.dim; g.textAlign = "right"; g.fillText(r.label, W - 2, Math.min(H - bottom - 11, y + 2)); }
  }
  // The scale's top, and the time.
  if (o.axis) {
    g.fillStyle = k.dim;
    g.textAlign = "left";
    g.fillText(fmt(max, o.unit), 2, top + 1);
    g.textBaseline = "bottom";
    for (const tk of timeTicks(o.from, o.to, Math.max(2, Math.floor(W / 70)))) {
      const x = X(tk.t);
      g.textAlign = x < 20 ? "left" : x > W - 20 ? "right" : "center";
      g.fillText(tk.label, x, H);
    }
  }
  // Under the pointer: its time and each series' mean there.
  const hx = hover.get(c);
  if (hx != null) {
    const t = o.from + hx * span;
    const x = hx * W;
    g.strokeStyle = k.dim; g.lineWidth = 1;
    g.beginPath(); g.moveTo(Math.round(x) + 0.5, top); g.lineTo(Math.round(x) + 0.5, top + ph); g.stroke();
    const said = o.series.map((s) => {
      const p = nearest(s.points || [], t, o.step);
      return `${s.label ? `${s.label} ` : ""}${p ? fmt(p[2], s.unit || o.unit) : "–"}`;
    });
    const text = `${hhmm(t, span <= 600)}  ${said.join(" · ")}`;
    g.font = "10.5px -apple-system, BlinkMacSystemFont, system-ui, sans-serif";
    const w = g.measureText(text).width + 8;
    const bx = Math.max(0, Math.min(W - w, x + 6 + w > W ? x - 6 - w : x + 6));
    g.fillStyle = k.panel; g.globalAlpha = 0.92; g.fillRect(bx, top, w, 14); g.globalAlpha = 1;
    g.fillStyle = k.text; g.textAlign = "left"; g.textBaseline = "top";
    g.fillText(text, bx + 4, top + 2);
  }
}

// The point at `t`, within a step of it (none in a gap).
function nearest(points, t, step) {
  let lo = 0, hi = points.length - 1;
  if (hi < 0) return null;
  while (lo < hi) { const mid = (lo + hi) >> 1; if (points[mid][0] < t) lo = mid + 1; else hi = mid; }
  const cands = [points[lo], points[lo - 1]].filter(Boolean);
  const p = cands.sort((a, b) => Math.abs(a[0] - t) - Math.abs(b[0] - t))[0];
  return Math.abs(p[0] - t) <= step * 1.5 ? p : null;
}
