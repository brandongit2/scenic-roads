// The world map's age controls (task #629; top-level since #685; docs/pipeline.md §5): a soloed
// step's published tiles coloured by when a catalogue last published each, over a range set by a
// pair of inputs, and a timeline of every catalogue (hovering one lights the tiles it published).
// Its data: /work/catalogs (crate::agent::cataloguesview), the catalogues after the last one this
// page has. The sums are apart from the DOM so Node tests them (web/test/catalogues.test.mjs).
import { hhmm, timeTicks } from "./chartmath.js";

// ---- The range's inputs ---------------------------------------------------------------------------
const UNITS = [["week", 604800], ["day", 86400], ["hour", 3600], ["minute", 60]];
const UNIT_RE = /^(w|wk|wks|weeks?|d|days?|h|hr|hrs|hours?|m|min|mins|minutes?)$/;
const unitSecs = (u) => (u.startsWith("w") ? 604800 : u.startsWith("d") ? 86400 : u.startsWith("h") ? 3600 : 60);

/** What's typed in one of the pair (`side` "left" or "right") at `now` (unix s): {kind: "rel", s}
 * (a span: the left's always back, negative; the right's always on, positive, whatever sign was
 * typed), {kind: "abs", t} (a local date and time: YYYY-MM-DD, with HH:MM[:SS] after a space or T),
 * {kind: "abs", t: now} for "now"; null when it isn't one. */
export function parseWhen(text, side, now) {
  const s = String(text ?? "").trim().toLowerCase();
  if (!s) return null;
  if (s === "now") return { kind: "abs", t: now };
  const rel = /^([+-]?)\s*(\d+(?:\.\d+)?)\s*([a-z]+)$/.exec(s);
  if (rel && UNIT_RE.test(rel[3])) {
    const v = Math.round(Number(rel[2]) * unitSecs(rel[3]));
    if (!(v > 0)) return null;
    return { kind: "rel", s: side === "left" ? -v : v };
  }
  const abs = /^(\d{4})-(\d{1,2})-(\d{1,2})(?:[ t](\d{1,2}):(\d{2})(?::(\d{2}))?)?$/.exec(s);
  if (abs) {
    const [y, mo, d, hh = 0, mi = 0, ss = 0] = abs.slice(1).map((x) => (x == null ? undefined : Number(x)));
    const dt = new Date(y, mo - 1, d, hh ?? 0, mi ?? 0, ss ?? 0);
    if (dt.getFullYear() !== y || dt.getMonth() !== mo - 1 || dt.getDate() !== d || hh > 23 || mi > 59 || ss > 59) return null;
    return { kind: "abs", t: Math.floor(dt.getTime() / 1000) };
  }
  return null;
}

const pad = (v) => String(v).padStart(2, "0");
/** A value as its input shows it: "-48 hours" / "+7 days" (the largest unit it's a whole number
 * of), or "2026-10-23 14:05" (local, to the minute). */
export function formatWhen(v) {
  if (!v) return "";
  if (v.kind === "rel") {
    const a = Math.abs(v.s);
    const [u, k] = UNITS.find(([, k]) => a % k === 0) || ["minute", 60];
    const n = a / k;
    return `${v.s < 0 ? "-" : "+"}${Number.isInteger(n) ? n : n.toFixed(1)} ${u}${n === 1 ? "" : "s"}`;
  }
  const d = new Date(v.t * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** The pair's range [from, to] (unix s): a relative value counts from the other input. */
export function rangeOf(left, right) {
  if (left.kind === "rel" && right.kind === "rel") return null;
  const to = right.kind === "abs" ? right.t : left.t + right.s;
  const from = left.kind === "abs" ? left.t : to + left.s;
  return [from, to];
}

/** The pair after `side`'s input is committed with `text`: {left, right} (swapped when the left
 * came out later than the right: the later always on the right), or {error} (the pair as it was
 * kept): not a time, or both relative. */
export function commit(pair, side, text, now) {
  const v = parseWhen(text, side, now);
  if (!v) return { error: `"${String(text).trim()}" isn't a time: a date and time (2026-10-23 14:05), "now", or a span (${side === "left" ? "-48 hours" : "+48 hours"}, 7 days)` };
  const next = { ...pair, [side]: v };
  if (next.left.kind === "rel" && next.right.kind === "rel") return { error: "Only one of the two may be a span: the other is the time it counts from" };
  // (Only two times can be out of order: a span is back from the right, or on from the left.)
  if (next.left.kind === "abs" && next.right.kind === "abs" && next.left.t > next.right.t) return { left: next.right, right: next.left };
  return { left: next.left, right: next.right };
}

/** The pair kept by this browser (`kept`: {left, right}, right null for "now at render"), as
 * values at `now`: the defaults "-48 hours" and now. */
export function pairAt(kept, now) {
  const ok = (v) => v && ((v.kind === "rel" && Number.isFinite(v.s)) || (v.kind === "abs" && Number.isFinite(v.t)));
  const left = ok(kept?.left) ? kept.left : { kind: "rel", s: -48 * 3600 };
  let right = ok(kept?.right) ? kept.right : { kind: "abs", t: now };
  if (left.kind === "rel" && right.kind === "rel") right = { kind: "abs", t: now };
  return { left, right };
}

// ---- The tiles ------------------------------------------------------------------------------------
/** The catalogues kept: `have` (sorted by number) with an answer's `catalogs` (those after the
 * last) added. */
export function mergeCatalogues(have, add) {
  const out = have.slice();
  const last = out.length ? out[out.length - 1].n : -Infinity;
  for (const c of add || []) if (c.n > last) out.push(c);
  out.sort((a, b) => a.n - b.n);
  return out;
}

/** A tile's key ("z/x/y") from its index (`y * 2^z + x`). */
export const keyOf = (z, i) => `${z}/${i % 2 ** z}/${Math.floor(i / 2 ** z)}`;

/** The published tiles at zoom `z` (3, 6 or 10: a deep tile's, its own packs'): key → {t, n} of the
 * catalogue that last published it, those a later catalogue dropped left out. */
export function tileTimes(cats, z) {
  const out = new Map();
  for (const c of cats) {
    for (const i of c[`z${z}`] || []) out.set(keyOf(z, i), { t: c.t, n: c.n, first: !!c.first });
    for (const i of c[`z${z}_gone`] || []) out.delete(keyOf(z, i));
  }
  return out;
}

/** When a tile ({z, x, y}) was last published, from the z3, z6 and z10 tiles' times (`tileTimes`):
 * its own at z3 or z6; a deep one's (z10 and past) its z10 tile's own (the catalogue that last named
 * a new file of its packs: #833 review), its z6 tile's when the agent sends no z10 tiles (one before
 * them); z7–9, its z6 tile's; a shallower one's (the root), the latest of the z3 tiles under it.
 * {t, n, first} or null (no catalogue kept names it). */
export function publishedAt(t3, t6, { z, x, y }, t10 = null) {
  if (z === 3) return t3.get(`3/${x}/${y}`) || null;
  if (z >= 10 && t10?.size) return t10.get(`10/${x >> (z - 10)}/${y >> (z - 10)}`) || null;
  if (z >= 6) return t6.get(`6/${x >> (z - 6)}/${y >> (z - 6)}`) || null;
  let best = null;
  for (const [k, v] of t3) {
    const [, kx, ky] = k.split("/").map(Number);
    if (kx >> (3 - z) === x && ky >> (3 - z) === y && !(best?.t >= v.t)) best = v;
  }
  return best;
}

// The colour map, oldest to newest: one hue path from a deep blue to a light yellow, which reads on
// either theme's ground; the highlight is apart from every colour of it (magenta).
export const RAMP = ["#3f4fa8", "#2f86b8", "#2fae9a", "#7fc35c", "#d8cf4a", "#f6e9a0"];
export const HIGHLIGHT = "#ff3ad0";
const hex = (c) => [1, 3, 5].map((i) => parseInt(c.slice(i, i + 2), 16));
/** The colour of a time `t` in [from, to] (clamped outside it). */
export function rampColour(t, from, to) {
  const f = to > from ? Math.max(0, Math.min(1, (t - from) / (to - from))) : 1;
  const x = f * (RAMP.length - 1), i = Math.min(RAMP.length - 2, Math.floor(x)), u = x - i;
  const [a, b] = [hex(RAMP[i]), hex(RAMP[i + 1])];
  return `rgb(${a.map((v, k) => Math.round(v + (b[k] - v) * u)).join(",")})`;
}

/** What the map paints: key → colour, the tiles outside [from, to] clamped to the nearer end, or
 * left out with `hide`; and how many are in the range. */
export function paint(times, from, to, hide) {
  const out = new Map();
  let inside = 0;
  for (const [k, v] of times) {
    const inR = v.t >= from && v.t <= to;
    if (inR) inside++;
    else if (hide) continue;
    out.set(k, rampColour(v.t, from, to));
  }
  return { paint: out, inside };
}

// ---- The timeline ---------------------------------------------------------------------------------
/** The timeline's span: the range in its centre 80%, a tenth of the whole beyond it each side. */
export function timelineSpan(from, to) {
  const pad = Math.max(1, to - from) * 0.125;
  return [from - pad, to + pad];
}

/** Which marks (their x, sorted) get their label of width `w` (px; one width, or each label's)
 * without two overlapping (`gap` px between): the `pin` indexes first (the latest first), then
 * the rest the latest first; with `width`, none past [0, width] (#836 review: passed over before the
 * choice, so the next mark in takes its place; filtered after, marks bunched at an edge, as while
 * the catalogues are read oldest first, had left none labelled). The indexes labelled, in order. */
export function labelled(xs, w, gap = 6, pin = [], width = null) {
  const wd = (i) => (Array.isArray(w) ? w[i] : w) / 2;
  const pinned = new Set(pin);
  const fits = (i) => width == null || (xs[i] - wd(i) >= 0 && xs[i] + wd(i) <= width);
  const order = [...[...pinned].sort((a, b) => b - a), ...xs.map((_, i) => i).reverse().filter((i) => !pinned.has(i))].filter(fits);
  const out = [];
  for (const i of order) if (out.every((k) => xs[i] + wd(i) + gap <= xs[k] - wd(k) || xs[k] + wd(k) + gap <= xs[i] - wd(i))) out.push(i);
  return out.sort((a, b) => a - b);
}

const MON = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WD = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
/** A catalogue's time as named: "Thu 23 Oct 21:32". */
export const whenText = (t) => { const d = new Date(t * 1000); return `${WD[d.getDay()]} ${d.getDate()} ${MON[d.getMonth()]} ${hhmm(t)}`; };

/** The timeline's mark labels: of catalogues `cats` ({n, t}) at `xs` (sorted) on a timeline `W`
 * px wide, {lab: the indexes labelled (a Set), text: each one's label}. A label is "386 · 22:17";
 * the first labelled of a day has its date too, "386 · 23 Oct 22:17": the marks thinned on the
 * plain widths, those day-firsts then widened and kept, the rest around them; none past the
 * timeline's edge, the next in labelled instead. A day-first whose dated label would pass the
 * edge keeps its plain one (#839 review: dropped, the one label of marks bunched at an edge, as
 * while the catalogues are read oldest first, left none: touch-test's intermittent 'mark labels'
 * FAIL, 0 labels). */
export function markLabels(cats, xs, W) {
  const day = (c) => new Date(c.t * 1000).toDateString();
  const plain = cats.map((c) => `${c.n} · ${hhmm(c.t)}`);
  const base = labelled(xs, plain.map((t) => t.length * 6.4), 6, [], W);
  const datedText = (c) => { const d = new Date(c.t * 1000); return `${c.n} · ${d.getDate()} ${MON[d.getMonth()]} ${hhmm(c.t)}`; };
  const fits = (i, t) => xs[i] - t.length * 3.2 >= 0 && xs[i] + t.length * 3.2 <= W;
  const firsts = base.filter((v, k) => k === 0 || day(cats[base[k - 1]]) !== day(cats[v]));
  const dated = new Set(firsts.filter((i) => fits(i, datedText(cats[i]))));
  const text = plain.map((t, i) => (dated.has(i) ? datedText(cats[i]) : t));
  const lab = new Set(labelled(base.map((i) => xs[i]), base.map((i) => text[i].length * 6.4), 6, firsts.map((i) => base.indexOf(i)), W).map((k) => base[k]));
  return { lab, text };
}

/** The time axis's ticks over [from, to]: times of day (chartmath.timeTicks) for two days or
 * less, else days ("23 Oct"), at most `most`. */
export function axisTicks(from, to, most = 6) {
  if (to - from <= 2 * 86400) return timeTicks(from, to, most);
  const day = [1, 2, 7, 14, 28].find((k) => (to - from) / (k * 86400) <= most) ?? 28;
  const out = [];
  const d = new Date(from * 1000);
  d.setHours(0, 0, 0, 0);
  for (let t = d.getTime() / 1000; t <= to; ) {
    if (t >= from) out.push({ t, label: `${new Date(t * 1000).getDate()} ${MON[new Date(t * 1000).getMonth()]}` });
    const n = new Date(t * 1000);
    n.setDate(n.getDate() + day);
    t = n.getTime() / 1000;
  }
  return out;
}

// ---- The tab ------------------------------------------------------------------------------------------
const SVGNS = "http://www.w3.org/2000/svg";
function el(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) e.append(k instanceof Node ? k : document.createTextNode(String(k)));
  return e;
}
const svg = (tag, attrs) => { const e = document.createElementNS(SVGNS, tag); for (const [k, v] of Object.entries(attrs || {})) e.setAttribute(k, v); return e; };
const plural = (k, one) => `${Number(k).toLocaleString("en-US")} ${one}${k === 1 ? "" : "s"}`;

/** The age controls: `el` (kept across the page's renders), `view()` what they say now ({from,
 * to, hide, lit (the "z/x/y" keys a hovered or held catalogue published), at(tile) its last
 * publish, publishedAt}); `onChange()` called as any of it changes. `call(path, body)` asks the
 * lead; `store` this browser's own. Reads the catalogues while the page is seen. */
export function createAges({ call, store, onChange }) {
  const now = () => Math.floor(Date.now() / 1000);
  const kept = store.get("catalogues", {}) || {};
  const st = { hide: !!kept.hide, pair: pairAt(kept, now()), keepRight: kept.right ? kept.right : null };
  let cats = [], t3 = new Map(), t6 = new Map(), t10 = new Map(), latest = null, reading = false, error = "", timer = 0, hovered = null, pinnedN = null;
  const save = () => store.set("catalogues", { hide: st.hide, left: st.pair.left, right: st.keepRight });

  const input = (side) => {
    const i = el("input", { type: "text", class: "cat-when", spellcheck: "false", autocomplete: "off", "aria-label": side === "left" ? "From (a time, or a span back from the other)" : "To (a time, or a span on from the other)" });
    const go = () => {
      const r = commit(st.pair, side, i.value, now());
      if (r.error) { msg.textContent = r.error; msg.hidden = false; fill(); return; }
      msg.hidden = true;
      // (The right left as the time it was shown at, now's at render: kept as "now", not that time.)
      const sameNow = (v) => v.kind === "abs" && st.keepRight == null && st.pair.right.kind === "abs" && Math.abs(v.t - st.pair.right.t) < 60;
      st.keepRight = sameNow(r.right) ? null : r.right;
      st.pair = { left: r.left, right: r.right };
      save();
      fill();
      changed();
    };
    i.addEventListener("change", go);
    i.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); i.blur(); } else if (e.key === "Escape") { fill(); i.blur(); } });
    return i;
  };
  const leftIn = input("left"), rightIn = input("right");
  const fill = () => { leftIn.value = formatWhen(st.pair.left); rightIn.value = formatWhen(st.pair.right); };
  fill();
  const msg = el("div", { class: "cat-msg small", role: "alert", hidden: true });
  const hide = el("input", { type: "checkbox", checked: st.hide, onchange: (e) => { st.hide = e.target.checked; save(); changed(); } });
  const controls = el("div", { class: "wmap-filters cat-ctl small" },
    el("div", { class: "mfrow" },
      el("span", { class: "cat-pair" }, el("span", { class: "dim" }, "Published from"), leftIn, el("span", { class: "dim" }, "to"), rightIn),
      el("label", { class: "mf" }, hide, "Hide tiles outside the date range")),
    msg);
  const legend = el("div", { class: "cat-legend small dim" });
  const line = svg("svg", { class: "cat-tl", role: "img", "aria-label": "Timeline of the catalogues" });
  const tip = el("div", { class: "cat-tip small", hidden: true });
  const status = el("div", { class: "small dim cat-status" });
  const tlBox = el("div", { class: "cat-tlbox" }, line, tip);
  const root = el("div", { class: "cat" }, controls, legend, tlBox, status);
  new ResizeObserver(() => drawLine()).observe(tlBox);

  const range = () => rangeOf(st.pair.left, st.pair.right) || [now() - 48 * 3600, now()];
  const changed = () => { draw(); onChange(); };

  function view() {
    const [from, to] = range();
    const lit = new Set();
    const hc = cats.find((c) => c.n === (hovered ?? pinnedN));
    if (hc) for (const z of [3, 6, 10]) for (const i of hc[`z${z}`] || []) lit.add(keyOf(z, i));
    return { from, to, hide: st.hide, lit, at: (tile) => publishedAt(t3, t6, tile, t10) };
  }

  function draw() {
    const [from, to] = range();
    const grad = `linear-gradient(to right, ${RAMP.join(", ")})`;
    legend.replaceChildren(
      el("span", null, "A soloed step's published tiles, by when:"),
      el("span", { class: "cat-scale" }, el("span", null, whenText(from)), el("i", { style: `background:${grad}` }), el("span", null, whenText(to))),
      el("span", null, st.hide ? "(the rest hidden)" : "(the rest at the nearer end's colour)"),
      el("span", { class: "lg" }, el("i", { style: `background:${HIGHLIGHT};border-color:${HIGHLIGHT}` }), "a catalogue's tiles, its mark hovered"));
    status.textContent = error ? `Can't read the catalogues: ${error}` : reading && !cats.length ? "Reading the catalogues on the NAS…" : `${plural(cats.length, "catalogue")} kept on the NAS${latest != null ? `, the last ${latest}` : ""}${reading ? " · reading" : ""} · times local`;
    drawLine();
  }
  // The timeline: the range in its centre 80%, the ends dimmed; a mark a catalogue (tall for one
  // written whole), labelled where its label fits; the time axis under it, its unit (local time)
  // said once in the status line.
  function drawLine() {
    const W = tlBox.clientWidth;
    if (!W) return;
    const H = 66, axisY = 44;
    const [from, to] = range();
    const [a, b] = timelineSpan(from, to);
    const x = (t) => ((t - a) / (b - a)) * W;
    line.setAttribute("viewBox", `0 0 ${W} ${H}`);
    line.setAttribute("width", W);
    line.setAttribute("height", H);
    const kids = [svg("rect", { x: 0, y: 0, width: x(from), height: axisY, class: "dimmed" }), svg("rect", { x: x(to), y: 0, width: W - x(to), height: axisY, class: "dimmed" }),
      svg("line", { x1: 0, x2: W, y1: axisY, y2: axisY, class: "axis" })];
    for (const tk of axisTicks(a, b, Math.max(2, Math.floor(W / 90)))) {
      kids.push(svg("line", { x1: x(tk.t), x2: x(tk.t), y1: axisY, y2: axisY + 4, class: "axis" }));
      const tx = svg("text", { x: x(tk.t), y: axisY + 16, class: "tick" });
      tx.textContent = tk.label;
      kids.push(tx);
    }
    const shownCats = cats.filter((c) => c.t >= a && c.t <= b);
    const xs = shownCats.map((c) => x(c.t));
    const { lab, text } = markLabels(shownCats, xs, W);
    // (Empty catalogues' marks go under the rest, so their hit boxes never cover a real one's.)
    const empties = [], under = kids.length;
    shownCats.forEach((c, i) => {
      const k = (c.z3 || []).length + (c.z6 || []).length;
      const out = c.t < from || c.t > to;
      const on = c.n === (hovered ?? pinnedN);
      const g = svg("g", { class: `mk${out ? " out" : ""}${on ? " on" : ""}`, "data-n": c.n, "data-k": k });
      g.append(svg("line", { x1: xs[i], x2: xs[i], y1: c.whole ? 10 : 18, y2: axisY }));
      if (lab.has(i)) { const t = svg("text", { x: xs[i], y: 8, class: "lbl" }); t.textContent = text[i]; g.append(t); }
      g.append(svg("rect", { x: xs[i] - 5, y: 0, width: 10, height: axisY, class: "hit" }));
      (k ? kids : empties).push(g);
    });
    kids.splice(under, 0, ...empties);
    line.replaceChildren(...kids);
  }
  const markOf = (e) => e.target.closest?.("g.mk");
  const say = (n, e) => {
    const c = cats.find((k) => k.n === n);
    if (!c) { tip.hidden = true; return; }
    tip.textContent = `Catalogue ${c.n} · ${whenText(c.t)} · ${c.whole ? "whole" : "a delta"} · ${plural((c.z6 || []).length, "z6 tile")}, ${plural((c.z3 || []).length, "z3 tile")} published${c.first ? " (the oldest kept: everything it names)" : ""}`;
    tip.hidden = false;
    const r = tlBox.getBoundingClientRect();
    tip.style.left = `${Math.max(0, Math.min(r.width - tip.offsetWidth, e.clientX - r.left - tip.offsetWidth / 2))}px`;
  };
  line.addEventListener("pointermove", (e) => {
    if (e.pointerType === "touch") return;
    const g = markOf(e), n = g ? +g.dataset.n : null;
    if (n !== hovered) { hovered = n; changed(); }
    if (n != null) say(n, e); else if (pinnedN == null) tip.hidden = true;
  });
  line.addEventListener("pointerleave", () => { if (hovered == null) return; hovered = null; if (pinnedN == null) tip.hidden = true; changed(); });
  // (A tap or click holds a catalogue lit; again, or off the marks, lets it go.)
  line.addEventListener("click", (e) => {
    e.stopPropagation();
    const g = markOf(e), n = g ? +g.dataset.n : null;
    pinnedN = n === pinnedN ? null : n;
    if (pinnedN != null) say(pinnedN, e); else tip.hidden = true;
    changed();
  });

  async function load() {
    clearTimeout(timer);
    if (document.visibilityState !== "visible") return;
    try {
      const [code, a] = await call("/work/catalogs", latest == null ? {} : { after: latest });
      if (code === 200 && a) {
        const had = cats.length;
        cats = mergeCatalogues(cats, a.catalogs);
        if (cats.length !== had) { t3 = tileTimes(cats, 3); t6 = tileTimes(cats, 6); t10 = tileTimes(cats, 10); }
        latest = cats.length ? cats[cats.length - 1].n : null;
        reading = !!a.reading;
        error = a.error || "";
      } else error = code === 503 ? "this Mac hasn't made the view yet" : `answer ${code}`;
    } catch (e) { error = e.message; }
    // ("now" at render: the right input moves on with the clock while kept as now.)
    if (st.keepRight == null) { st.pair = pairAt({ left: st.pair.left, right: null }, now()); if (document.activeElement !== rightIn && document.activeElement !== leftIn) fill(); }
    changed();
    timer = setTimeout(load, reading ? 2000 : 30000);
  }
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && load());
  draw();
  load();
  return { el: root, view };
}
