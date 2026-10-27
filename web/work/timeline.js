// The Jobs tab's schedule as a timeline one moves (task #787; docs/pipeline.md §5, the schedule):
// its lanes' tracks hold every segment over the whole domain (the history's seven days back to
// the forecast's end) as data, placed by dash.js; each track is a canvas, and the segments in view
// are drawn on it once a frame (with the axis, the now line and any arrows) whenever the view
// moves: no element a segment, so moving it lays nothing out. Zoomed far from where its segments
// were packed (sums.js packRuns: a pixel's gap), it asks for a render (`onRepack`).
//
// Moved: a trackpad's two-finger drag sideways (a wheel's deltaX; an up or down one is the
// panel's scroll), a mouse's drag, one finger's drag sideways on a touch screen (touch-action
// pan-y: up and down stays the page's). Zoomed: a trackpad's pinch (ctrl+wheel in Chrome, gesture
// events in Safari), Opt+wheel on a mouse, two fingers' pinch on a touch screen; about the point
// under the pointer, or between the fingers. Opens 12 h either side of now, now in the middle; the
// view moves with now.
//
// A segment ({key, from, until, colour, kind: past|running|next|mark, failed}) pointed at (or its
// card pinned: a tap's) is `active`: `relations(key)` (dash.js) names the segments it waits on and
// those waiting on it ({on, arrows, tiles}); red arrows, straight and square-cornered, run from
// each it waits on to it and from it to each waiting; everything else dims. `onLit(tiles)` hands
// its tiles to the map. A legend item's hover is a key like any other. The segment under the
// pointer is its canvas's `data-card` (dash.js's cards find it there).
import { timeTicks } from "./chartmath.js";
import { viewPan, viewRange, viewZoom } from "./sums.js";

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const SVG = "http://www.w3.org/2000/svg";
// The view at first: 12 h either side of now.
export const SPAN0 = 86400;
// A segment's look by kind (dash.css's before #787): its strength, its inset from the track's top
// and bottom (px).
const LOOK = { past: [0.8, 1], next: [0.4, 1], running: [1, 0], mark: [0.75, 1] };
const DIM = 0.14;

export function createTimeline({ relations = () => null, onLit = () => {}, onRepack = () => {} } = {}) {
  const el = document.createElement("div");
  el.className = "lanewrap";
  const axis = document.createElement("div");
  axis.className = "axis";
  const cap = document.createElement("div");
  cap.className = "nowcap";
  cap.innerHTML = "<span>now</span>";
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("class", "arrows");
  svg.innerHTML = `<defs><marker id="sched-ah" viewBox="0 0 6 6" refX="5.5" refY="3" markerWidth="6" markerHeight="6" orient="auto"><path d="M0 0L6 3L0 6z"/></marker></defs><g></g>`;
  const arrows = svg.lastChild;
  const archive = document.createElement("div");
  archive.className = "archived";
  // (An archive box: a lid over a box with a slot.)
  archive.innerHTML = `<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true"><path d="M1.5 2.5h9v2.5h-9zM2.5 5v4.5h7V5M5 7h2" fill="none" stroke="currentColor" stroke-width="1.1"/></svg>`;
  archive.setAttribute("role", "img");
  archive.setAttribute("aria-label", "Older history is in the archive, not shown here");
  archive.hidden = true;

  let view = { c: 0, span: SPAN0 };
  let dom = { now: 0, lo: 0, hi: 1 }; // the domain and now (unix s), as dash.js last rendered it
  let geo = { x0: 0, w: 0 }; // the tracks' column: its left in `el` and its width (px)
  let packed = 0; // the span its segments were packed at
  let tracks = []; // [{el, canvas, segs}]
  let hoverKey = null, pinKey = null, activeKey = null, rel = null, under = null;

  const range = () => viewRange(view, dom.now, dom.lo, dom.hi);
  // (A pointer's x as a share of the tracks' width.)
  const frac = (clientX) => {
    const r = el.getBoundingClientRect();
    return geo.w ? Math.max(0, Math.min(1, (clientX - r.left - geo.x0) / geo.w)) : 0.5;
  };
  // A segment's left and right in its track (px) in the view `r`: at least 2 px wide; a mark 2 px
  // about its time.
  const span = (s, r) => {
    const a = ((s.from - r.t0) / r.span) * geo.w;
    if (s.kind === "mark") return [a - 1, a + 1];
    return [a, Math.max(a + 2, ((s.until - r.t0) / r.span) * geo.w)];
  };

  // (The tokens' colours, "var(--st-…)" as the canvas can't read them: resolved once a render,
  // the theme's.)
  const probe = document.createElement("i");
  let colours = new Map();
  const colour = (c) => {
    if (!colours.has(c)) {
      probe.style.color = "";
      probe.style.color = c;
      el.append(probe);
      colours.set(c, getComputedStyle(probe).color);
      probe.remove();
    }
    return colours.get(c);
  };

  // ---- Drawing: once a frame, whatever moved it ----
  let queued = false, repackTimer = 0;
  const later = () => { if (!queued) { queued = true; requestAnimationFrame(frame); } };
  function frame() {
    queued = false;
    if (!geo.w) return;
    const r = range();
    const x = (t) => ((t - r.t0) / r.span) * geo.w;
    drawTracks(r);
    // The now line, where it's in view.
    const nx = x(dom.now);
    cap.hidden = nx < 0 || nx > geo.w;
    cap.style.left = `${(geo.x0 + nx).toFixed(1)}px`;
    // At the domain's left end: the archive's sign.
    archive.hidden = r.t0 > dom.lo + r.span / geo.w;
    archive.style.left = `${geo.x0}px`;
    drawAxis(r.t0, r.t1, x);
    drawArrows(r);
    // (Zoomed far from where it was packed: packed again once the zoom rests.)
    if (packed && (r.span / packed > 1.6 || packed / r.span > 1.6)) {
      clearTimeout(repackTimer);
      repackTimer = setTimeout(() => { if (!gesture()) onRepack(); }, 250);
    }
  }
  // Each track's segments in view on its canvas, in their stage's colour at their kind's strength
  // (dimmed but for those `rel` names while one's active; full under the pointer); a failed one
  // outlined red, a running one in the mark's colour.
  function drawTracks(r) {
    const dpr = window.devicePixelRatio || 1;
    const bad = colour("var(--bad)"), mark = colour("var(--mark)");
    for (const t of tracks) {
      const c = t.canvas, w = geo.w, hh = t.el.clientHeight || 13;
      const W = Math.round(w * dpr), H = Math.round(hh * dpr);
      if (c.width !== W || c.height !== H) { c.width = W; c.height = H; }
      const g = c.getContext("2d");
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      g.clearRect(0, 0, w, hh);
      for (const s of t.segs) {
        if (s.until < r.t0 - r.span / w * 2 || s.from > r.t1 + r.span / w * 2) continue;
        const [a, b] = span(s, r), [alpha, inset] = LOOK[s.kind] || LOOK.past;
        g.globalAlpha = s === under || rel?.on.has(s) ? 1 : rel ? DIM : alpha;
        g.fillStyle = colour(s.colour);
        g.fillRect(a, inset, b - a, hh - 2 * inset);
        if (s.failed || s.kind === "running") {
          g.strokeStyle = s.failed ? bad : mark;
          g.lineWidth = 1;
          g.strokeRect(a + 0.5, inset + 0.5, b - a - 1, hh - 2 * inset - 1);
        }
      }
      g.globalAlpha = 1;
    }
  }
  // A dense time axis: a tick at each round time (chartmath.js timeTicks, by local time, midnight
  // a day's name), smaller ones between, none crowding the now line (its cap says "now").
  function drawAxis(t0, t1, x) {
    const most = Math.max(4, Math.round(geo.w / 70));
    const kids = [];
    for (const t of timeTicks(t0, t1, most * 4)) { const b = document.createElement("b"); b.style.left = `${x(t.t).toFixed(1)}px`; kids.push(b); }
    // (Ticks a day or more apart are days, named, whatever hour local midnight falls at after a
    // clock change.)
    const days = (t1 - t0) / most >= 43200;
    for (const t of timeTicks(t0, t1, most)) {
      // (None crowding the now line, nor cut by either end.)
      if (Math.abs(x(t.t) - x(dom.now)) < 34 || x(t.t) < 16 || x(t.t) > geo.w - 16) continue;
      const s = document.createElement("span");
      const day = days || t.label === "00:00";
      s.textContent = day ? DAYS[new Date((t.t + 7200) * 1000).getDay()] : t.label;
      if (day) s.className = "day";
      s.style.left = `${x(t.t).toFixed(1)}px`;
      kids.push(s);
    }
    axis.replaceChildren(...kids);
  }
  // The active segment's arrows: from the end of each it waits on to its start, from its end to the
  // start of each waiting on it; across, down or up beside the arrow's end, across again (around
  // when the end comes first: down between the rows, back, then on).
  function drawArrows(r) {
    if (!rel?.arrows?.length) { if (arrows.firstChild) arrows.replaceChildren(); return; }
    svg.style.left = `${geo.x0}px`;
    svg.style.width = `${geo.w}px`;
    const top = svg.getBoundingClientRect().top;
    const rows = new Map(tracks.map((t) => { const b = t.el.getBoundingClientRect(); return [t, { y: (b.top + b.bottom) / 2 - top, h: b.height }]; }));
    const d = [];
    for (const [from, to] of rel.arrows) {
      const ya = rows.get(from.track), yb = rows.get(to.track);
      if (!ya || !yb) continue;
      const x1 = span(from, r)[1], y1 = ya.y, x2 = span(to, r)[0], y2 = yb.y;
      if (x2 - x1 >= 12) {
        const xm = Math.round(x2 - 6) + 0.5;
        d.push(`M${x1.toFixed(1)} ${y1.toFixed(1)}H${xm}V${y2.toFixed(1)}H${x2.toFixed(1)}`);
      } else {
        const ym = (y2 > y1 ? y2 - yb.h / 2 - 1.5 : y2 + yb.h / 2 + 1.5).toFixed(1);
        d.push(`M${x1.toFixed(1)} ${y1.toFixed(1)}H${(x1 + 6).toFixed(1)}V${ym}H${(x2 - 6).toFixed(1)}V${y2.toFixed(1)}H${x2.toFixed(1)}`);
      }
    }
    arrows.replaceChildren(...d.map((x) => { const p = document.createElementNS(SVG, "path"); p.setAttribute("d", x); return p; }));
  }

  // ---- What's under the pointer, and what's active: it, a legend item pointed at, a pinned card's ----
  // (The segment at a point: the last drawn there, within a pixel; its canvas's data-card.)
  function hit(e) {
    const c = e.target instanceof Element ? e.target.closest(".track") : null;
    const t = c && tracks.find((x) => x.el === c);
    let found = null;
    if (t) {
      const r = range(), px = e.clientX - t.el.getBoundingClientRect().left;
      for (let i = t.segs.length - 1; i >= 0 && !found; i--) {
        const [a, b] = span(t.segs[i], r);
        if (px >= a - 1 && px <= b + 1) found = t.segs[i];
      }
    }
    for (const x of tracks) if (x !== t || !found) { delete x.canvas.dataset.card; x.canvas.style.cursor = ""; }
    if (found) { t.canvas.dataset.card = found.key; t.canvas.style.cursor = "pointer"; }
    if (found !== under) { under = found; later(); }
    return found;
  }
  function activate(force = false) {
    const key = hoverKey ?? pinKey;
    if (key === activeKey && !force) return;
    activeKey = key;
    rel = key ? relations(key) : null;
    if (rel) rel.on = new Set(rel.on);
    later();
    onLit(rel?.tiles?.length ? rel.tiles : null);
  }
  el.addEventListener("pointerover", (e) => { if (e.pointerType !== "touch" && !drag?.moved && !pinch) segOver(e); });
  const segOver = (e) => {
    const s = hit(e);
    hoverKey = s?.key ?? null;
    activate();
    return s;
  };
  el.addEventListener("pointerleave", (e) => {
    if (e.pointerType === "touch") return;
    for (const x of tracks) delete x.canvas.dataset.card;
    if (under) { under = null; later(); }
    if (hoverKey) { hoverKey = null; activate(); }
  });

  // ---- Moving and zooming ----
  const setView = (v) => { view = v; later(); };
  const zoomAt = (k, clientX) => setView(viewZoom(view, k, frac(clientX), dom.now, dom.lo, dom.hi));
  const panPx = (dx) => setView(viewPan(view, (dx / geo.w) * range().span, dom.now, dom.lo, dom.hi));
  // (A wheel's run of events keeps the axis it began on: a trackpad's sideways drag wobbles.)
  let lock = { axis: "", t: -1e9 };
  el.addEventListener("wheel", (e) => {
    const f = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 400 : 1;
    if (e.ctrlKey || e.altKey) {
      e.preventDefault();
      // (Opt+wheel: a mouse's notches, about 100 a notch; ctrl+wheel: a pinch's small steps. A
      // wheel turned with Shift held too may say it in deltaX.)
      if (!gesturing) zoomAt(Math.exp((-(e.deltaY || e.deltaX) * f) / (e.ctrlKey ? 100 : 400)), e.clientX);
      return;
    }
    const dx = e.deltaX * f, dy = e.deltaY * f;
    if (e.timeStamp - lock.t > 180) lock.axis = Math.abs(dx) > Math.abs(dy) ? "x" : "y";
    lock.t = e.timeStamp;
    if (lock.axis !== "x") return;
    e.preventDefault();
    panPx(dx);
  }, { passive: false });
  // (Safari's trackpad pinch; its ctrl+wheel events, should it send them too, aren't counted twice.)
  let gesturing = false, gspan = 0;
  el.addEventListener("gesturestart", (e) => { e.preventDefault(); gesturing = true; gspan = range().span; });
  el.addEventListener("gesturechange", (e) => { e.preventDefault(); zoomAt(range().span / (gspan / Math.max(0.01, e.scale)), e.clientX); });
  el.addEventListener("gestureend", () => { gesturing = false; later(); });

  // A drag (a mouse's, a pen's, a finger's) moves it; two fingers pinch it.
  const ptrs = new Map();
  let drag = null, pinch = null, eatClick = false;
  const gesture = () => !!(drag?.moved || pinch || gesturing);
  el.addEventListener("pointerdown", (e) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    if (e.target instanceof Element && e.target.closest("button, a, input, select")) return;
    // (What a tap lands on: its card's key on the canvas before the page's tap reads it.)
    hit(e);
    if (e.isPrimary) { ptrs.clear(); pinch = null; }
    ptrs.set(e.pointerId, e.clientX);
    if (ptrs.size === 1) drag = { x: e.clientX, v: view, moved: false, id: e.pointerId, type: e.pointerType };
    else if (ptrs.size === 2) {
      const [a, b] = [...ptrs.values()], r = range(), f = frac((a + b) / 2);
      pinch = { d: Math.max(8, Math.abs(a - b)), span: r.span, t: r.t0 + f * r.span };
      drag = null;
    }
  });
  el.addEventListener("pointermove", (e) => {
    if (!ptrs.has(e.pointerId)) {
      if (e.pointerType !== "touch") segOver(e);
      return;
    }
    ptrs.set(e.pointerId, e.clientX);
    if (pinch && ptrs.size === 2) {
      const [a, b] = [...ptrs.values()];
      const span = (pinch.span * pinch.d) / Math.max(8, Math.abs(a - b)), f = frac((a + b) / 2);
      // (The time between the fingers at first stays between them.)
      setView(viewPan(viewZoom({ c: pinch.t - dom.now, span: pinch.span }, pinch.span / span, 0.5, dom.now, dom.lo, dom.hi), (0.5 - f) * span, dom.now, dom.lo, dom.hi));
      return;
    }
    if (!drag || drag.id !== e.pointerId) return;
    const dx = e.clientX - drag.x;
    if (!drag.moved) {
      if (Math.abs(dx) < 4) { if (e.pointerType !== "touch") segOver(e); return; }
      drag.moved = true;
      if (e.pointerType !== "touch") el.setPointerCapture?.(e.pointerId);
      el.classList.add("panning");
      hoverKey = null;
      under = null;
      activate();
    }
    setView(viewPan(drag.v, (-dx / geo.w) * range().span, dom.now, dom.lo, dom.hi));
  });
  const up = (e) => {
    if (drag?.id === e.pointerId && drag.moved && drag.type !== "touch") eatClick = true;
    else if (!drag?.moved) hit(e);
    ptrs.delete(e.pointerId);
    if (ptrs.size < 2) pinch = null;
    // (A pinch's one finger left: it drags on from where it is.)
    const [rest] = [...ptrs.entries()];
    drag = rest ? { x: rest[1], v: view, moved: true, id: rest[0], type: "touch" } : null;
    if (!ptrs.size) { el.classList.remove("panning"); later(); }
  };
  el.addEventListener("pointerup", up);
  el.addEventListener("pointercancel", up);
  // (A mouse's drag ends in a click on what it let go over: not a card's pin.)
  el.addEventListener("click", (e) => { if (eatClick) { eatClick = false; e.stopPropagation(); e.preventDefault(); } }, true);
  new ResizeObserver(() => measure()).observe(el);
  function measure() {
    if (!axis.isConnected) return;
    geo = { x0: axis.offsetLeft, w: axis.offsetWidth };
    later();
  }

  const api = {
    el, axis, archive,
    // The view's times now, before a render places its segments (dash.js).
    range: (now, lo, hi) => { dom = { now, lo, hi }; return range(); },
    // A track: its element (dash.js puts it in the grid) and its segments, added by `add`.
    track() {
      const t = { el: document.createElement("div"), canvas: document.createElement("canvas"), segs: [] };
      t.el.className = "track";
      t.el.append(t.canvas);
      t.add = (s) => { s.track = t; t.segs.push(s); return s; };
      return t;
    },
    // After a render: its lanes (the grid, `axis` its last row) in place, their tracks, packed at
    // the view's span; what was active found again.
    placed(lanes, trs, span) {
      el.replaceChildren(lanes, cap, svg, archive);
      tracks = trs;
      colours = new Map();
      under = null;
      packed = span;
      measure();
      frame();
      activate(true);
    },
    // The segments in view of the given kinds, at their middles (client px), for the tests
    // (tools/dev/touch-test.mjs): [{key, kind, x, y}].
    spots(kinds) {
      const r = range(), out = [];
      for (const t of tracks) {
        const b = t.el.getBoundingClientRect();
        for (const s of t.segs) {
          if (!kinds.includes(s.kind)) continue;
          const [a, z] = span(s, r), m = (Math.max(0, a) + Math.min(geo.w, z)) / 2;
          if (z > 2 && a < geo.w - 2) out.push({ key: s.key, kind: s.kind, x: b.left + m, y: (b.top + b.bottom) / 2 });
        }
      }
      return out;
    },
    // A card pinned (a segment's: a tap on a touch screen) or let go.
    pin(key) { pinKey = key; activate(); },
    // A legend item pointed at (its key; null: none).
    hover(key) { if (gesture()) return; hoverKey = key; activate(); },
  };
  el.spots = api.spots;
  return api;
}
