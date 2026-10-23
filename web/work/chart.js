// The build page's charts over time, on a canvas (its sums in chartmath.js, tested): series of
// /work/live's points ([t, least, mean, most]) over [from, to], a line a series (its least-to-most
// band faint when asked, filled or stacked when asked), the scale's lines and its top in the unit,
// reference lines (a Mac's cores), the gaps where its Mac was silent shaded, the time below when
// asked; under the pointer, the time and each series' value there.
import { fmt, gapOver, hhmm, runs, topOf } from "./chartmath.js";

const opts = new WeakMap();
// The scale's gutter (px): the same in every chart, so their plots start and end at the same x.
const AXIS_W = 58;
// The time's tick steps (s), and a time ago in words: "now", "-45 s", "-10 min", "-6 h".
const AGO_STEPS = [15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400];
const ago = (a) => (a === 0 ? "now" : a < 60 ? `−${a} s` : a < 3600 ? `−${a / 60} min` : `−${+(a / 3600).toFixed(1)} h`);
const hover = new WeakMap();

// The page's colours (index.html's tokens), read once a draw.
function tokens(el) {
  const cs = getComputedStyle(el);
  const v = (k, d) => cs.getPropertyValue(k).trim() || d;
  return { line: v("--line", "#262c33"), grid: v("--grid", "#1c2127"), warn: v("--warn", "#e0b45a"), dim: v("--dim", "#8a939c"), text: v("--text", "#d7dce2"), panel: v("--panel", "#161a1f"), bad: v("--bad", "#e5534b") };
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
 *   refs: [{v, label}], gaps: [{from, to}], axis, colourOf(point), shade: {points, colour}, marks: [t]}` —
 * `colourOf` colours a line by its points (memory pressure by its level); `shade` shades the
 * spans a state was on (mains power, the owner present); `marks`, events' times (a vertical line each); `axis` draws the time and the scale's labels (`times: false`: the scale's alone). */
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
  // (The mockup's frame: the scale's labels in a gutter at the left, every chart's the same width so
  // their plots line up; the time under it.)
  const L = o.axis ? AXIS_W : 0, R = o.axis ? 4 : 0;
  const bottom = o.axis && o.times !== false ? 14 : 0;
  const top = o.axis ? 6 : 1;
  const ph = Math.max(1, H - bottom - top);
  const pw = Math.max(1, W - L - R);
  const span = Math.max(1, o.to - o.from);
  const X = (t) => L + ((t - o.from) / span) * pw;
  const lists = o.series.map((s) => s.points || []);
  const max = o.max ?? topOf(lists, { band: o.series.some((s) => s.band), least: Math.max(o.least || 0, ...(o.refs || []).map((r) => r.v)), stacked: o.stacked });
  const Y = (v) => top + ph - (Math.max(0, Math.min(max, v)) / max) * ph;
  g.font = "10px -apple-system, BlinkMacSystemFont, system-ui, sans-serif";
  g.textBaseline = "top";

  // Silent stretches: shaded, hatched lightly.
  g.fillStyle = k.line;
  for (const gp of o.gaps || []) {
    const x0 = Math.max(L, X(gp.from)), x1 = Math.min(W - R, X(gp.to));
    if (x1 - x0 < 0.5) continue;
    g.globalAlpha = 0.35;
    g.fillRect(x0, top, x1 - x0, ph);
    g.globalAlpha = 1;
  }
  // A state's spans shaded behind (`shade`: a point's mean at least half on; on till the next
  // point, a gap's width at most).
  if (o.shade) {
    const p = o.shade.points || [];
    g.fillStyle = o.shade.colour; g.globalAlpha = 0.16;
    p.forEach((q, i) => {
      if (q[2] < 0.5) return;
      const end = Math.min(p[i + 1] ? p[i + 1][0] : o.to, q[0] + gapOver(o.step));
      g.fillRect(X(q[0]), top, Math.max(1, X(end) - X(q[0])), ph);
    });
    g.globalAlpha = 1;
  }
  // Events' marks (`marks`: what the memory guard last did).
  if (o.marks?.length) {
    g.fillStyle = k.bad;
    for (const t of o.marks) if (t >= o.from && t <= o.to) g.fillRect(Math.round(X(t)), top, 1, ph);
  }
  // The scale: a line at 0, half and the top (0's the stronger), each labelled in the gutter; the
  // time's lines at its ticks, labelled under (seconds ago, "now" at the right).
  if (o.axis) {
    g.lineWidth = 1;
    const ys = [0, max / 2, max];
    ys.forEach((v, i) => {
      const y = Math.round(Y(v)) + 0.5;
      g.strokeStyle = i ? k.grid : k.line;
      g.beginPath(); g.moveTo(L, y); g.lineTo(W - R, y); g.stroke();
    });
    g.fillStyle = k.dim; g.textAlign = "right"; g.textBaseline = "middle";
    const drawn = [];
    for (const i of [0, 2, 1]) {
      const y = Y(ys[i]);
      if (drawn.some((d) => Math.abs(d - y) < 12)) continue;
      drawn.push(y);
      g.fillText(fmt(ys[i], o.unit), L - 4, y);
    }
    if (o.times !== false) {
      const step = AGO_STEPS.find((x) => span / x <= Math.max(2, Math.floor(pw / 80))) ?? 86400;
      g.textBaseline = "top";
      for (let a = 0; a <= span + 1e-6; a += step) {
        const x = L + (1 - a / span) * pw;
        g.strokeStyle = k.grid; g.beginPath(); g.moveTo(Math.round(x) + 0.5, top); g.lineTo(Math.round(x) + 0.5, top + ph); g.stroke();
        g.fillStyle = k.dim; g.textAlign = a === 0 ? "right" : Math.abs(a - span) < 1e-6 ? "left" : "center";
        g.fillText(ago(a), x, top + ph + 3);
      }
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
        g.fillStyle = s.colour; g.globalAlpha = o.stacked ? 0.35 : 0.18;
        g.beginPath();
        run.forEach((p, i) => (i ? g.lineTo(X(p[0]), Y(hi[i])) : g.moveTo(X(p[0]), Y(hi[i]))));
        for (let i = run.length - 1; i >= 0; i--) g.lineTo(X(run[i][0]), Y(lo[i]));
        g.closePath(); g.fill(); g.globalAlpha = 1;
      }
      g.lineWidth = 1.5;
      g.lineJoin = "round";
      if (run.length === 1) { g.fillStyle = s.colour; g.fillRect(X(run[0][0]) - 1, Y(hi[0]) - 1, 2, 2); }
      for (let i = 1; i < run.length; i++) {
        g.strokeStyle = o.colourOf?.(run[i]) || s.colour;
        g.beginPath(); g.moveTo(X(run[i - 1][0]), Y(hi[i - 1])); g.lineTo(X(run[i][0]), Y(hi[i])); g.stroke();
      }
      if (o.stacked) run.forEach((p, i) => base.set(p[0], hi[i]));
    }
    // (The newest point, a dot.)
    const ps = s.points || [], last = ps[ps.length - 1];
    if (last && o.to - last[0] <= gapOver(o.step)) {
      g.fillStyle = o.colourOf?.(last) || s.colour;
      g.beginPath(); g.arc(X(last[0]), Y(o.stacked ? base.get(last[0]) || 0 : last[2]), 2.5, 0, Math.PI * 2); g.fill();
    }
  });
  // Reference lines, dashed, in the warning's colour (or their own), labelled over their right end.
  for (const r of o.refs || []) {
    if (r.v > max) continue;
    const y = Math.round(Y(r.v)) + 0.5;
    const col = r.colour || k.warn;
    g.strokeStyle = col; g.setLineDash([4, 3]); g.lineWidth = 1;
    g.beginPath(); g.moveTo(L, y); g.lineTo(W - R, y); g.stroke(); g.setLineDash([]);
    if (r.label && o.axis) { g.fillStyle = col; g.textAlign = "right"; g.textBaseline = "bottom"; g.fillText(r.label, W - R - 4, Math.max(top + 10, y - 2)); }
  }
  // Under the pointer: its time and each series' mean there.
  const hx = hover.get(c);
  if (hx != null) {
    const x = Math.max(L, Math.min(W - R, hx * W));
    const t = o.from + ((x - L) / pw) * span;
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
