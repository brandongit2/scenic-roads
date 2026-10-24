// The build page's charts over time, on a canvas (its sums in chartmath.js, tested): series of
// /work/live's points ([t, least, mean, most]) over [from, to], a line a series (its least-to-most
// band faint when asked, filled or stacked when asked), the scale's lines and its top in the unit,
// reference lines (a Mac's cores), the gaps where its Mac was silent shaded, the time below when
// asked; under the pointer, the time and each series' value there.
import { fmt, gapOver, hhmm, runs, topOf } from "./chartmath.js";

const opts = new WeakMap();
// The scales' gutters (px), left and right: the same in every chart, so their plots start and end at
// the same x (the right's holds a second scale's words when a chart has one).
const AXIS_W = 58;
const AXIS_R = 40;
// The time's tick steps (s), and a time ago in words: "now", "-45 s", "-10 min", "-6 h", "-2 d"
// (whole days from two on: seven days' ticks every two).
const AGO_STEPS = [15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400, 172800];
const ago = (a) =>
  a === 0 ? "now" : a < 60 ? `−${a} s` : a < 3600 ? `−${a / 60} min` : a >= 172800 && a % 86400 === 0 ? `−${a / 86400} d` : `−${+(a / 3600).toFixed(1)} h`;
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
 * `{from, to, step, series: [{points, colour, label, fill, band, unit, right}], rightUnit, stacked, unit, least, max,
 *   refs: [{v, label, colour, edit}], ticks: [[v, label]], gaps: [{from, to}], axis, colourOf(point), shade: {points, colour}, marks: [t]}` —
 * a series `right` is drawn thin and dashed on a second scale of its own (`rightUnit`), its words in the right gutter;
 * `colourOf` colours a line by its points (memory pressure by its level); `shade` shades the
 * spans a state was on (mains power, the owner present); `marks`, events' times (a vertical line each); `axis` draws the time and the scale's labels (`times: false`: the scale's alone);
 * `ticks`, the scale's lines and words in place of 0, half and the top; a ref's label is a boxed tag
 * over the canvas (the canvas's parent positioned). A ref with `edit` is a setting the owner sets
 * here: `{key, title, note, unit, min, max, step, label(v), set(v | "off") → Promise<bool>}`; its line
 * or tag dragged up or down (a mouse at once, a touch after a long press on the tag) previews `v`
 * snapped to `step` within [min, max] and sets it on release (Escape cancels); its ✎ opens a small
 * editor on the chart (a number, Set, Off, Cancel). */
export function draw(c, o) {
  opts.set(c, o);
  if (!c.dataset.hover) {
    c.dataset.hover = "1";
    const move = (e) => { const r = c.getBoundingClientRect(); hover.set(c, (e.clientX - r.left) / r.width); paint(c); };
    c.addEventListener("pointermove", (e) => { move(e); c.style.cursor = e.pointerType === "mouse" && lineAt(c, e) ? "ns-resize" : ""; });
    c.addEventListener("pointerdown", (e) => {
      // (A mouse on a settable line takes it; a touch scrolls, its tag's long press drags.)
      const r = e.pointerType !== "touch" && e.button === 0 && lineAt(c, e);
      if (r) { e.preventDefault(); startDrag(c, r.edit, r.v, e.clientY, "pointer"); } else move(e);
    });
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
  const L = o.axis ? AXIS_W : 0, R = o.axis ? AXIS_R : 0;
  const bottom = o.axis && o.times !== false ? 14 : 0;
  const top = o.axis ? 6 : 1;
  const ph = Math.max(1, H - bottom - top);
  const pw = Math.max(1, W - L - R);
  const span = Math.max(1, o.to - o.from);
  const X = (t) => L + ((t - o.from) / span) * pw;
  const left = o.series.filter((s) => !s.right), right = o.series.filter((s) => s.right);
  const max = o.max ?? topOf(left.map((s) => s.points || []), { band: left.some((s) => s.band), least: Math.max(o.least || 0, ...(o.refs || []).map((r) => r.v)), stacked: o.stacked });
  const rmax = right.length ? topOf(right.map((s) => s.points || []), {}) : 1;
  const Y = (v) => top + ph - (Math.max(0, Math.min(max, v)) / max) * ph;
  const YR = (v) => top + ph - (Math.max(0, Math.min(rmax, v)) / rmax) * ph;
  g.font = "10px -apple-system, BlinkMacSystemFont, system-ui, sans-serif";
  g.textBaseline = "top";

  // Silent stretches: shaded faintly (the mockup draws none; a gap's missing line says it).
  g.fillStyle = k.grid;
  for (const gp of o.gaps || []) {
    const x0 = Math.max(L, X(gp.from)), x1 = Math.min(W - R, X(gp.to));
    if (x1 - x0 < 0.5) continue;
    g.globalAlpha = 0.6;
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
    const ys = o.ticks ? o.ticks.map((t) => t[0]) : [0, max / 2, max];
    // (The unit once for the axis, the owner's: "0, 100, 200" and "MB/s" beside, in the top's unit.)
    const [said, unit] = o.ticks ? [o.ticks.map((t) => t[1]), ""] : axisWords(ys, max, o.unit);
    ys.forEach((v, i) => {
      const y = Math.round(Y(v)) + 0.5;
      g.strokeStyle = i ? k.grid : k.line;
      g.beginPath(); g.moveTo(L, y); g.lineTo(W - R, y); g.stroke();
    });
    g.fillStyle = k.dim; g.textAlign = "right"; g.textBaseline = "middle";
    const drawn = [];
    for (const i of ys.map((_, i) => i).sort((a, b) => +(a > 0 && a < ys.length - 1) - +(b > 0 && b < ys.length - 1))) {
      const y = Y(ys[i]);
      if (drawn.some((d) => Math.abs(d - y) < 12)) continue;
      drawn.push(y);
      g.fillText(said[i], L - 4, y);
    }
    if (unit) { g.textAlign = "left"; g.fillText(unit, 0, top + ph / 2); }
    // (The second scale on the right, in its series' colour, on the same lines: its 0 and top, its
    // unit once where its half would be, so its words never meet.)
    if (right.length && !o.ticks) {
      const [rs, ru] = axisWords([0, rmax], rmax, o.rightUnit);
      g.fillStyle = right[0].colour; g.textAlign = "left";
      g.fillText(rs[0], W - R + 4, Y(0));
      g.fillText(rs[1], W - R + 4, Y(max));
      if (ru) g.fillText(ru, W - R + 4, top + ph / 2);
    }
    if (o.times !== false) {
      // (At most four steps, the mockup's: "−1 min, −45 s, −30 s, −15 s, now" at every range.)
      const step = AGO_STEPS.find((x) => span / x <= Math.max(2, Math.min(4, Math.floor(pw / 80)))) ?? AGO_STEPS[AGO_STEPS.length - 1];
      g.textBaseline = "top";
      // (Right to left; a word that would meet the one beside it left out, the owner's: labels never overlap.)
      let edge = Infinity;
      for (let a = 0; a <= span + 1e-6; a += step) {
        const x = L + (1 - a / span) * pw;
        g.strokeStyle = k.grid; g.beginPath(); g.moveTo(Math.round(x) + 0.5, top); g.lineTo(Math.round(x) + 0.5, top + ph); g.stroke();
        g.fillStyle = k.dim; g.textAlign = a === 0 ? "right" : Math.abs(a - span) < 1e-6 ? "left" : "center";
        const tw = g.measureText(ago(a)).width, x0 = g.textAlign === "right" ? x - tw : g.textAlign === "left" ? x : x - tw / 2;
        if (x0 + tw > edge - 6) continue;
        edge = x0;
        g.fillText(ago(a), x, top + ph + 3);
      }
    }
  }
  // The series: stacked fills in order, or each its line (and band, fill).
  const base = new Map(), Y0 = Y;
  o.series.forEach((s) => {
    const Y = s.right ? YR : Y0;
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
      if ((s.fill || o.stacked) && !s.right) {
        g.fillStyle = s.colour; g.globalAlpha = o.stacked ? 0.35 : 0.18;
        g.beginPath();
        run.forEach((p, i) => (i ? g.lineTo(X(p[0]), Y(hi[i])) : g.moveTo(X(p[0]), Y(hi[i]))));
        for (let i = run.length - 1; i >= 0; i--) g.lineTo(X(run[i][0]), Y(lo[i]));
        g.closePath(); g.fill(); g.globalAlpha = 1;
      }
      g.lineWidth = s.right ? 1.25 : 1.5;
      g.lineJoin = "round";
      g.setLineDash(s.right ? [3, 2] : []);
      if (run.length === 1) { g.fillStyle = s.colour; g.fillRect(X(run[0][0]) - 1, Y(hi[0]) - 1, 2, 2); }
      for (let i = 1; i < run.length; i++) {
        g.strokeStyle = o.colourOf?.(run[i]) || s.colour;
        g.beginPath(); g.moveTo(X(run[i - 1][0]), Y(hi[i - 1])); g.lineTo(X(run[i][0]), Y(hi[i])); g.stroke();
      }
      g.setLineDash([]);
      if (o.stacked) run.forEach((p, i) => base.set(p[0], hi[i]));
    }
    // (The newest point, a dot.)
    const ps = s.points || [], last = ps[ps.length - 1];
    if (last && o.to - last[0] <= gapOver(o.step)) {
      g.fillStyle = o.colourOf?.(last) || s.colour;
      g.beginPath(); g.arc(X(last[0]), Y(o.stacked ? base.get(last[0]) || 0 : last[2]), 2.5, 0, Math.PI * 2); g.fill();
    }
  });
  // Reference lines, dashed, in the warning's colour (or their own), each labelled by a boxed tag
  // over its right end, the highest first (under its line when over it would cover one already placed).
  const tags = [];
  // (A ref being dragged drawn at the drag's value, its tag saying it.)
  const d = dragging.get(c);
  const refs = (o.refs || []).map((r) => (d && r.edit?.key === d.edit.key ? { ...r, v: d.v, label: r.edit.label(d.v), live: true } : r));
  geom.set(c, { top, ph, max, refs: [] });
  for (const r of refs.sort((a, b) => b.v - a.v)) {
    if (r.v > max) continue;
    const y = Math.round(Y(r.v)) + 0.5;
    const col = r.colour || k.warn;
    g.strokeStyle = col; g.setLineDash([4, 3]); g.lineWidth = 1;
    g.beginPath(); g.moveTo(L, y); g.lineTo(W - R, y); g.stroke(); g.setLineDash([]);
    if (r.edit) geom.get(c).refs.push({ y, r });
    if (r.label && o.axis) {
      const free = (ty) => !tags.some((t) => Math.abs(t.y - ty) < 16);
      const ty = [Math.max(0, y - 17), Math.min(H - 16, y + 2)].find(free);
      if (ty != null) tags.push({ r, y: ty, col });
    }
  }
  placeTags(c, o, tags, R);
  // Under the pointer: its time and each series' mean there.
  const hx = hover.get(c);
  if (hx != null) {
    const x = Math.max(L, Math.min(W - R, hx * W));
    const t = o.from + ((x - L) / pw) * span;
    g.strokeStyle = k.dim; g.lineWidth = 1;
    g.beginPath(); g.moveTo(Math.round(x) + 0.5, top); g.lineTo(Math.round(x) + 0.5, top + ph); g.stroke();
    const said = o.series.map((s) => {
      const p = nearest(s.points || [], t, o.step);
      return `${s.label ? `${s.label} ` : ""}${p ? fmt(p[2], s.unit || (s.right ? o.rightUnit : o.unit)) : "–"}`;
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

// The scale's numbers in the top's unit (`fmt`'s, kB/s to TB), and that unit: [labels, unit].
function axisWords(ys, max, unit) {
  const m = /^(-?[\d.,]+)\s*(.*)$/.exec(fmt(max, unit));
  if (!m) return [ys.map((v) => fmt(v, unit)), ""];
  const k = parseFloat(m[1].replace(/,/g, "")) / max;
  return [ys.map((v) => (v === 0 ? "0" : String(+(v * k).toPrecision(3)))), m[2]];
}

// The refs' tags, kept over the canvas between draws (one element a ref, reused): a settable one's
// words drag it (cursor ns-resize; a touch after a long press), its ✎ a button opening the editor.
function placeTags(c, o, tags, R) {
  const host = c.parentNode;
  if (!host) return;
  const old = [...host.querySelectorAll(":scope > .reftag")];
  tags.forEach((t, i) => {
    const settable = !!t.r.edit;
    let e = old[i];
    if (!e || e.classList.contains("set") !== settable) {
      const n = document.createElement("span");
      n.className = settable ? "reftag set" : "reftag";
      if (settable) {
        n.append(document.createElement("span"));
        const pen = document.createElement("button");
        pen.type = "button"; pen.className = "pen"; pen.textContent = "✎";
        pen.addEventListener("click", () => n._edit && openEditor(c, n._edit, n._v));
        n.append(pen);
        tagDrag(c, n);
      }
      if (e) e.replaceWith(n); else host.append(n);
      old[i] = e = n;
    }
    const words = settable ? e.firstChild : e;
    if (words.textContent !== t.r.label) words.textContent = t.r.label;
    if (settable) {
      e._edit = t.r.edit; e._v = t.r.v;
      e.title = `Drag to set ${t.r.edit.title.toLowerCase()} (Escape cancels); ✎ to type it`;
      e.lastChild.title = `Set ${t.r.edit.title.toLowerCase()}`;
      e.classList.toggle("live", !!t.r.live);
    }
    e.style.right = `${R + 2}px`; e.style.top = `${Math.round(t.y)}px`;
    e.style.color = t.col; e.style.borderColor = `color-mix(in srgb, ${t.col} 45%, transparent)`;
  });
  old.slice(tags.length).forEach((e) => e.remove());
}

// ---- Setting a ref's value: dragged, or typed in the editor ----------------------------------------
// Per canvas: its plot's top, height and top value and its settable refs' lines as last drawn; the
// drag under way ({edit, v, from}).
const geom = new WeakMap();
const dragging = new WeakMap();
const LONG_PRESS_MS = 450;

// The settable ref whose line is within 4 px of the pointer.
function lineAt(c, e) {
  const gm = geom.get(c);
  if (!gm || dragging.has(c)) return null;
  const y = e.clientY - c.getBoundingClientRect().top;
  return gm.refs.find((x) => Math.abs(x.y - y) <= 4)?.r || null;
}

/** The value at `y` (px in the canvas) snapped to `edit.step` within [min, max] (the chart's top
 * too: past it the line can't be drawn). */
export function valueAt(gm, edit, y) {
  const raw = ((gm.top + gm.ph - y) / gm.ph) * gm.max;
  const top = Math.min(edit.max, Math.floor(gm.max / edit.step) * edit.step);
  return Math.max(edit.min, Math.min(Math.max(edit.min, top), Math.round(raw / edit.step) * edit.step));
}

// A drag of `edit`'s line from `v`, its moves from the pointer (`by` "pointer") or a long-pressed
// touch ("touch"): previewed on each move, set on release when it moved, given up on Escape.
function startDrag(c, edit, v, clientY, by) {
  const d = { edit, v, from: v };
  dragging.set(c, d);
  c.closest(".mc, .box")?.querySelector(".cfg")?.remove();
  const at = (y) => { const nv = valueAt(geom.get(c), edit, y - c.getBoundingClientRect().top); if (nv !== d.v) { d.v = nv; paint(c); } };
  const end = (keep) => {
    removeEventListener("pointermove", pm); removeEventListener("pointerup", pu); removeEventListener("pointercancel", pc);
    removeEventListener("touchmove", tm); removeEventListener("touchend", tu); removeEventListener("touchcancel", pc);
    removeEventListener("keydown", kd, true); removeEventListener("blur", pc);
    document.body.classList.remove("dragging-ref");
    if (!keep || d.v === d.from) { dragging.delete(c); paint(c); return; }
    // (The preview stays till the set's answer: its caller draws the new value then.)
    Promise.resolve(edit.set(d.v)).catch(() => false).finally(() => { if (dragging.get(c) === d) { dragging.delete(c); paint(c); } });
  };
  const pm = (e) => at(e.clientY), pu = () => end(true), pc = () => end(false);
  const tm = (e) => { e.preventDefault(); at(e.touches[0].clientY); }, tu = () => end(true);
  const kd = (e) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); end(false); } };
  if (by === "touch") { addEventListener("touchmove", tm, { passive: false }); addEventListener("touchend", tu); addEventListener("touchcancel", pc); }
  else { addEventListener("pointermove", pm); addEventListener("pointerup", pu); addEventListener("pointercancel", pc); }
  addEventListener("keydown", kd, true);
  // (The window losing focus mid-drag gives it up: its release may never come.)
  addEventListener("blur", pc);
  document.body.classList.add("dragging-ref");
  at(clientY);
  paint(c);
}

// A settable tag's drag: a mouse or pen pressing its words at once; a touch held still for
// LONG_PRESS_MS (moved first, it's the page's scroll).
function tagDrag(c, n) {
  n.addEventListener("pointerdown", (e) => {
    if (e.pointerType === "touch" || e.button !== 0 || e.target.closest(".pen") || !n._edit) return;
    e.preventDefault();
    startDrag(c, n._edit, n._v, e.clientY, "pointer");
  });
  let timer = null, y0 = 0;
  const stop = () => { clearTimeout(timer); timer = null; };
  n.addEventListener("touchstart", (e) => {
    if (e.target.closest(".pen") || !n._edit || e.touches.length !== 1) return;
    y0 = e.touches[0].clientY;
    timer = setTimeout(() => { timer = null; startDrag(c, n._edit, n._v, y0, "touch"); }, LONG_PRESS_MS);
  }, { passive: true });
  n.addEventListener("touchmove", (e) => { if (timer && Math.abs(e.touches[0].clientY - y0) > 8) stop(); else if (timer) y0 = e.touches[0].clientY; }, { passive: true });
  n.addEventListener("touchend", stop);
  n.addEventListener("touchcancel", stop);
  n.addEventListener("contextmenu", (e) => e.preventDefault());
}

// The editor on the chart (the mockup's): the setting's name, a number in its unit, Set, Off,
// Cancel; Enter sets, Escape cancels; a refused set says why in it.
function openEditor(c, edit, v) {
  const host = c.parentNode;
  host.querySelector(":scope > .cfg")?.remove();
  const el = (tag, cls, text) => { const x = document.createElement(tag); if (cls) x.className = cls; if (text != null) x.textContent = text; return x; };
  const pnl = el("div", "cfg");
  const inp = el("input");
  Object.assign(inp, { type: "number", min: edit.min, max: edit.max, step: 1, value: v });
  inp.setAttribute("aria-label", edit.title);
  const row = el("div", "cfgrow");
  const set = el("button", "primary", "Set"), off = el("button", null, "Off"), cancel = el("button", null, "Cancel");
  for (const b of [set, off, cancel]) b.type = "button";
  const said = el("p", "dim");
  row.append(inp, el("span", "dim", edit.unit), set, off, cancel);
  pnl.append(el("b", null, edit.title), row, said);
  if (edit.note) said.textContent = edit.note;
  const close = () => pnl.remove();
  let busy = false;
  const go = async (what) => {
    if (busy) return;
    if (what !== "off") {
      const n = +inp.value;
      if (inp.value === "" || !Number.isFinite(n)) { said.textContent = `a number, ${edit.min} to ${edit.max} ${edit.unit}`; return; }
      what = Math.max(edit.min, Math.min(edit.max, n));
    }
    busy = set.disabled = off.disabled = true;
    said.textContent = "setting…";
    const ok = await Promise.resolve(edit.set(what)).catch(() => false);
    if (ok) close();
    else { busy = set.disabled = off.disabled = false; said.textContent = "not set: the page's banner says why"; }
  };
  set.addEventListener("click", () => go(null));
  off.addEventListener("click", () => go("off"));
  cancel.addEventListener("click", close);
  pnl.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.stopPropagation(); close(); } else if (e.key === "Enter") go(null); });
  host.append(pnl);
  inp.focus({ preventScroll: true }); inp.select();
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
