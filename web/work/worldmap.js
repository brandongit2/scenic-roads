// The overview's world map (the build page's Overview tab; docs/pipeline.md §5): one canvas in Web
// Mercator (the world 0–1000 each way, y down: countries.json's and /work/regions' outlines), with
// the z3 grid (and z6's when zoomed in enough to see it), the countries, the regions coloured by
// how far they are, the tiles finished in the past window (each fading in as it comes, paler as it
// ages), and each running job's target tiles in its stage's colour, filled as far as they're done.
// Kept across the page's renders (its element moved into each new section), drawn again when its
// data changes, as it's moved or zoomed, and while tiles fade in; nothing runs otherwise.
//
// Moved by a drag (mouse, pen or one finger), zoomed by a pinch (a trackpad's: ctrl+wheel in
// Chrome, gesture events in Safari; two fingers on a touch screen), a double click, the +/−
// buttons or the keyboard (+, −, arrows, 0); a two-finger scroll moves it once zoomed in, else
// scrolls the page. Opens on the whole world. What's under the pointer is said by the page's cards,
// the whole stack there (every tile at every zoom, then every region): under a mouse the map names
// it in its `data-card` ("mstack") and `data-also` (the keys, "|" between: dash.js wireCards); a
// click or a tap the map takes itself, at the point lifted, and hands the stack to `pick` (a
// tap's click isn't left to the page: Mobile Safari, #615). It draws a tile's minimap for its card
// (`minimap`).
import { inRings, tileBox, tileOf } from "./sums.js";

const FADE_MS = 900;
const MAX_K = 48; // pixels a world unit, most zoomed in (a z6 tile ≈ 750 px)
const Z6 = 1000 / 64;
// The regions' paths, made once for every map (the Overview's and the Regions tab's), by outline.
const regionPaths = new WeakMap();

/** A map: `el` to place, `highlight(id)` to light a region, `update(data)` with {countries (rings), regions [{id, fill, edge, state}],
 * running (Map key → {step, frac, current}), finished (Map key → {t}), from, now, colour(step)
 * (a CSS colour, var() allowed), fade (finished tiles paler as they age)}, `minimap(key)` a canvas
 * of a tile and around it; `pick(keys, x, y)` called on a click or tap (keys: the stack there,
 * none off everything). */
export function createMap({ pick = () => {} } = {}) {
  const canvas = document.createElement("canvas");
  canvas.className = "wmap";
  canvas.tabIndex = 0;
  canvas.setAttribute("role", "img");
  canvas.setAttribute("aria-label", "World map of the build: regions, running and finished tiles");
  const btn = (text, title, fn) => { const b = document.createElement("button"); b.className = "quiet"; b.textContent = text; b.title = title; b.onclick = (e) => { e.stopPropagation(); fn(); }; return b; };
  const ctl = document.createElement("div");
  ctl.className = "wmap-ctl";
  ctl.append(btn("+", "Zoom in (+)", () => zoomBy(2)), btn("−", "Zoom out (−)", () => zoomBy(0.5)), btn("⤢", "Whole map (0)", () => { fit(); draw(); }));
  const el = document.createElement("div");
  el.className = "wmap-wrap";
  el.append(canvas, ctl);

  let data = { countries: [], regions: [], running: new Map(), finished: new Map(), now: 0, from: 0, colour: () => "#888" };
  let paths = { countries: null, regions: new Map() };
  const view = { cx: 500, cy: 420, k: 0, fitted: false };
  let size = { w: 0, h: 0 };
  let hover = null;
  let held = null; // a region the page points at (the Regions tab's list under the pointer)
  const appeared = new Map(); // finished tile → when it began fading in (performance.now())
  let firstFinished = true;
  let raf = 0;

  // ---- Colours: the page's tokens (index.html), read at each draw (light or dark as it is now) --
  const tokens = {};
  const readTokens = () => {
    const cs = getComputedStyle(document.documentElement);
    for (const k of ["bg", "panel", "line", "rule", "dim", "text", "ok", "accent", "spark", "warn"]) tokens[k] = cs.getPropertyValue(`--${k}`).trim() || "#888";
    tokens.cs = cs;
  };
  const resolve = (c) => {
    const m = /^var\((--[\w-]+)(?:,\s*var\((--[\w-]+)\))?\)$/.exec(c || "");
    return m ? tokens.cs.getPropertyValue(m[1]).trim() || (m[2] && tokens.cs.getPropertyValue(m[2]).trim()) || "#888" : c;
  };

  // ---- Paths, once per outline set ---------------------------------------------------------------
  const ringsPath = (rings, close) => {
    const p = new Path2D();
    for (const r of rings || []) {
      if (r.length < 4) continue;
      p.moveTo(r[0], r[1]);
      for (let i = 2; i + 1 < r.length; i += 2) p.lineTo(r[i], r[i + 1]);
      if (close) p.closePath();
    }
    return p;
  };
  const bboxOf = (rings) => {
    const b = [Infinity, Infinity, -Infinity, -Infinity];
    for (const r of rings || []) for (let i = 0; i + 1 < r.length; i += 2) {
      b[0] = Math.min(b[0], r[i]); b[1] = Math.min(b[1], r[i + 1]); b[2] = Math.max(b[2], r[i]); b[3] = Math.max(b[3], r[i + 1]);
    }
    return b;
  };

  // ---- The view ----------------------------------------------------------------------------------
  // (Zoomed out at most to the whole world (82° N to 66° S) fitting the box, both ways: in a box
  // wider or taller than that, bands beside or under it, the world centred.)
  const minK = () => Math.min(size.w / 1000, size.h / 730);
  function fit() {
    // (The whole world, but not the poles' stretched rows: from about 82° N to 66° S.)
    const b = [0, 60, 1000, 790];
    view.cx = (b[0] + b[2]) / 2;
    view.cy = (b[1] + b[3]) / 2;
    view.k = Math.max(minK(), Math.min(MAX_K, size.w / (b[2] - b[0]), size.h / (b[3] - b[1])));
    view.fitted = true;
    clampView();
  }
  const clampView = () => {
    view.k = Math.max(minK(), Math.min(MAX_K, view.k));
    const hw = size.w / 2 / view.k, hh = size.h / 2 / view.k;
    view.cx = hw >= 500 ? 500 : Math.max(hw, Math.min(1000 - hw, view.cx));
    view.cy = hh >= 500 ? 425 : Math.max(hh, Math.min(1000 - hh, view.cy));
  };
  const toWorld = (px, py) => [view.cx + (px - size.w / 2) / view.k, view.cy + (py - size.h / 2) / view.k];
  function zoomAt(f, px = size.w / 2, py = size.h / 2) {
    const [wx, wy] = toWorld(px, py);
    view.k *= f;
    clampView();
    view.cx = wx - (px - size.w / 2) / view.k;
    view.cy = wy - (py - size.h / 2) / view.k;
    clampView();
    touchAction();
    draw();
  }
  const zoomBy = (f) => zoomAt(f);
  const zoomed = () => view.k > minK() * 1.02 && view.k > fitK() * 1.02;
  const fitK = () => { const s = { ...view }; fit(); const k = view.k; Object.assign(view, s); return k; };
  // (Zoomed out, a vertical swipe on a touch screen scrolls the page; zoomed in, it moves the map.)
  const touchAction = () => { canvas.style.touchAction = zoomed() ? "none" : "pan-y"; };

  // ---- Drawing -----------------------------------------------------------------------------------
  // The scene into `c` (css pixels w × h, ratio `dpr`) at view `v`; `lw` a line's width in pixels.
  function scene(c, v, w, hgt, dpr, focus) {
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.fillStyle = tokens.bg;
    c.fillRect(0, 0, w, hgt);
    const k = v.k;
    c.setTransform(k * dpr, 0, 0, k * dpr, (w / 2 - v.cx * k) * dpr, (hgt / 2 - v.cy * k) * dpr);
    const px = 1 / k;
    const [x0, y0] = [v.cx - w / 2 / k, v.cy - hgt / 2 / k];
    const [x1, y1] = [v.cx + w / 2 / k, v.cy + hgt / 2 / k];
    const seen = (b) => b[2] >= x0 && b[0] <= x1 && b[3] >= y0 && b[1] <= y1;
    // Countries.
    if (paths.countries) {
      c.fillStyle = tokens.panel;
      c.fill(paths.countries);
      c.strokeStyle = tokens.line;
      c.lineWidth = px;
      c.stroke(paths.countries);
    }
    // The grid: z3, and z6 once its cells are 24 px or more (zoomed in from the whole world).
    const grid = (n, colour, alpha) => {
      const s = 1000 / n;
      c.globalAlpha = alpha;
      c.strokeStyle = colour;
      c.lineWidth = px;
      c.beginPath();
      for (let i = Math.max(0, Math.ceil(x0 / s)); i * s <= Math.min(1000, x1); i++) { c.moveTo(i * s, Math.max(0, y0)); c.lineTo(i * s, Math.min(1000, y1)); }
      for (let i = Math.max(0, Math.ceil(y0 / s)); i * s <= Math.min(1000, y1); i++) { c.moveTo(Math.max(0, x0), i * s); c.lineTo(Math.min(1000, x1), i * s); }
      c.stroke();
      c.globalAlpha = 1;
    };
    if (Z6 * k >= 24) grid(64, tokens.rule, 0.35);
    grid(8, tokens.rule, 0.7);
    // Regions, by state.
    const regionColour = { on: tokens.ok, done: tokens.ok, building: tokens.accent, queued: tokens.dim };
    for (const r of data.regions) {
      const p = paths.regions.get(r.id);
      if (!p || !seen(p.bbox)) continue;
      const col = regionColour[r.state] || tokens.dim;
      c.globalAlpha = r.state === "on" ? 0.16 : r.state === "building" ? 0.14 : 0.07;
      c.fillStyle = col;
      c.fill(p.fill);
      c.globalAlpha = r.state === "queued" ? 0.55 : 0.85;
      c.strokeStyle = col;
      c.lineWidth = px * (focus === `mr ${r.id}` ? 2.2 : 0.8);
      c.stroke(p.edge);
      c.globalAlpha = 1;
    }
    // (The region the page points at: filled and outlined in the accent, over the rest.)
    const lit = focus?.startsWith("mr ") && paths.regions.get(focus.slice(3));
    if (lit) {
      c.globalAlpha = 0.3;
      c.fillStyle = tokens.accent;
      c.fill(lit.fill);
      c.globalAlpha = 1;
      c.strokeStyle = tokens.accent;
      c.lineWidth = px * 1.8;
      c.stroke(lit.edge);
    }
    // Finished tiles: faded in as they came, paler as they age over the window.
    const t = performance.now();
    const span = Math.max(1, data.now - data.from);
    for (const [key, f] of data.finished) {
      if (data.running.has(key)) continue;
      const [bx, by, s, w] = boxOf(key, bandF);
      if (bx > x1 || by > y1 || bx + s < x0 || by + s < y0) continue;
      const fade = Math.max(0, Math.min(1, (t - (appeared.get(key) ?? 0)) / FADE_MS));
      const age = Math.max(0, Math.min(1, (data.now - f.t) / span));
      // (A low zoom's tile, one of thousands of square kilometres, is drawn paler: it would hide the rest.)
      c.globalAlpha = fade * (data.fade === false ? 0.6 : 0.18 + 0.5 * (1 - age)) * (s > Z6 * 1.5 ? 0.3 : 1);
      c.fillStyle = resolve(data.colour(f.step));
      c.fillRect(bx, by, w, s);
    }
    c.globalAlpha = 1;
    // Running tiles: outlined in their stage's colour, filled from the bottom as far as they're done.
    for (const [key, r] of data.running) {
      const [bx, by, s, w] = boxOf(key, bandR);
      if (bx > x1 || by > y1 || bx + s < x0 || by + s < y0) continue;
      const col = resolve(data.colour(r.step));
      c.globalAlpha = 0.12;
      c.fillStyle = col;
      c.fillRect(bx, by, w, s);
      c.globalAlpha = 0.75;
      c.fillRect(bx, by + s * (1 - r.frac), w, s * r.frac);
      c.globalAlpha = 1;
      c.strokeStyle = col;
      c.lineWidth = px * (r.current ? 2 : 1.2);
      c.strokeRect(bx, by, w, s);
    }
    // What's pointed at.
    if (focus?.startsWith("mt ")) {
      const [bx, by, s, w] = boxOf(focus.slice(3), data.running.has(focus.slice(3)) ? bandR : bandF);
      c.strokeStyle = tokens.text;
      c.lineWidth = px * 1.5;
      c.strokeRect(bx, by, w, s);
    }
    c.setTransform(1, 0, 0, 1, 0, 0);
  }

  function draw() {
    // (Not shown, its tab another: drawn when it's shown again, as its size comes.)
    if (!size.w || !canvas.offsetParent) return;
    readTokens();
    const dpr = window.devicePixelRatio || 1;
    const c = canvas.getContext("2d");
    scene(c, view, size.w, size.h, dpr, hover || held);
    // (Still fading in: drawn again next frame.)
    const t = performance.now();
    let fading = false;
    for (const k of data.finished.keys()) if (t - (appeared.get(k) ?? 0) < FADE_MS) { fading = true; break; }
    cancelAnimationFrame(raf);
    if (fading) raf = requestAnimationFrame(draw);
  }

  // ---- Size ---------------------------------------------------------------------------------------
  new ResizeObserver(() => {
    const r = canvas.getBoundingClientRect();
    if (!r.width) return;
    const dpr = window.devicePixelRatio || 1;
    // (Not zoomed in: the whole world again at the new size, a pane dragged or the window resized.)
    const whole = !view.fitted || !zoomed();
    size = { w: r.width, h: r.height };
    canvas.width = Math.round(r.width * dpr);
    canvas.height = Math.round(r.height * dpr);
    if (whole) fit();
    clampView();
    touchAction();
    draw();
  }).observe(canvas);
  // (A theme change: the map and the minimaps shown in cards drawn again in its colours.)
  matchMedia("(prefers-color-scheme: dark)").addEventListener?.("change", () => {
    draw();
    for (const cv of document.querySelectorAll("canvas.wmap-mini")) if (cv.dataset.key) paintMini(cv, cv.dataset.key);
  });

  // ---- What's at a point -----------------------------------------------------------------------
  // The stack there, as the cards' keys: every tile shown at every zoom (the deepest first), then
  // every region (regions overlap); none outside the canvas.
  let zooms = [], atTile = new Map(), bandR = new Map(), bandF = new Map();
  // A tile entry's box: [x, y, side, width], its band of the square when steps share it.
  const boxOf = (key, bands) => { const [bx, by, s] = tileBox(tileOf(key)); const [i, n] = bands.get(key) || [0, 1]; return [bx + (s * i) / n, by, s, s / n]; };
  function stackAt(px, py) {
    if (px < 0 || py < 0 || px > size.w || py > size.h) return [];
    const [wx, wy] = toWorld(px, py);
    const out = [];
    for (const z of zooms) {
      const n = 2 ** z, key = `${z}/${Math.floor((wx / 1000) * n)}/${Math.floor((wy / 1000) * n)}`;
      for (const k of atTile.get(key) || []) out.push(`mt ${k}`);
    }
    for (const r of data.regions) {
      const p = paths.regions.get(r.id);
      if (p && wx >= p.bbox[0] && wx <= p.bbox[2] && wy >= p.bbox[1] && wy <= p.bbox[3] && inRings(p.rings, wx, wy)) out.push(`mr ${r.id}`);
    }
    return out;
  }
  const local = (e) => { const r = canvas.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };
  // (A mouse's or pen's hover: the stack named for the page's hover card, its top drawn lit. A
  // touch names nothing: its tap is `pick`ed.)
  const point = (e) => {
    if (e.pointerType === "touch") return;
    const keys = stackAt(...local(e));
    canvas.dataset.card = keys.length ? "mstack" : "";
    canvas.dataset.also = keys.join("|");
    const top = keys[0] || null;
    if (top !== hover) { hover = top; draw(); }
  };

  // ---- Moving and zooming ----------------------------------------------------------------------
  const ptrs = new Map();
  let drag = null, moved = false, pinch = null, tap = false;
  canvas.addEventListener("pointerdown", (e) => {
    // (A first finger down: no other is. A finger whose end never came, a touch the browser
    // took over to scroll the page (pan-y) without a pointercancel, would stay held and the next
    // tap be taken for a pinch's second finger: no card, #615.)
    if (e.isPrimary) { ptrs.clear(); pinch = null; }
    point(e);
    ptrs.set(e.pointerId, local(e));
    moved = false;
    tap = ptrs.size === 1;
    // (A finger on the map zoomed out (touch-action pan-y) moves it sideways only: an up or down
    // swipe is the page's to scroll, never the map's, #615.)
    if (ptrs.size === 1) drag = { x: ptrs.get(e.pointerId)[0], y: ptrs.get(e.pointerId)[1], cx: view.cx, cy: view.cy, ys: e.pointerType !== "touch" || zoomed() };
    else if (ptrs.size === 2) {
      const [a, b] = [...ptrs.values()];
      pinch = { d: Math.hypot(a[0] - b[0], a[1] - b[1]), k: view.k };
      drag = null;
      tap = false;
    }
  });
  canvas.addEventListener("pointermove", (e) => {
    if (ptrs.has(e.pointerId)) ptrs.set(e.pointerId, local(e));
    if (pinch && ptrs.size === 2) {
      const [a, b] = [...ptrs.values()];
      moved = true;
      zoomAt((pinch.k * Math.hypot(a[0] - b[0], a[1] - b[1])) / Math.max(1, pinch.d) / view.k, (a[0] + b[0]) / 2, (a[1] + b[1]) / 2);
    } else if (drag) {
      const [lx, ly] = local(e), dx = lx - drag.x, dy = ly - drag.y;
      if (!moved && Math.hypot(dx, dy) < 4) return point(e);
      // (A mouse's drag held past the canvas's edge; a touch is held by the browser itself.)
      if (!moved && e.pointerType !== "touch") canvas.setPointerCapture?.(e.pointerId);
      moved = true;
      tap = false;
      view.cx = drag.cx - dx / view.k;
      if (drag.ys) view.cy = drag.cy - dy / view.k;
      clampView();
      canvas.dataset.card = "";
      draw();
    } else point(e);
  });
  const up = (e) => {
    // (A tap or click: the stack where it lifted, to the page.)
    if (e.type === "pointerup" && tap && !moved && ptrs.size === 1) pick(stackAt(...local(e)), e.clientX, e.clientY);
    tap = false;
    ptrs.delete(e.pointerId);
    if (ptrs.size < 2) pinch = null;
    // (A pinch's one finger left: it drags on from where it is.)
    const [rest] = [...ptrs.values()];
    drag = rest && ptrs.size === 1 ? { x: rest[0], y: rest[1], cx: view.cx, cy: view.cy, ys: e.pointerType !== "touch" || zoomed() } : null;
  };
  canvas.addEventListener("pointerup", up);
  canvas.addEventListener("pointercancel", up);
  canvas.addEventListener("pointerleave", (e) => {
    if (e.pointerType === "touch") return;
    canvas.dataset.card = canvas.dataset.also = "";
    if (hover) { hover = null; draw(); }
  });
  // (The map's clicks are its own, picked at their pointerup: the page's own click handler
  // never sees one.)
  canvas.addEventListener("click", (e) => { e.stopPropagation(); moved = false; });
  canvas.addEventListener("dblclick", (e) => { e.preventDefault(); zoomAt(2, ...local(e)); });
  canvas.addEventListener("wheel", (e) => {
    const f = e.deltaMode === 1 ? 16 : 1;
    if (e.ctrlKey) { e.preventDefault(); if (!gesturing) zoomAt(Math.exp((-e.deltaY * f) / 200), ...local(e)); return; }
    if (!zoomed()) return; // (Zoomed out: the page scrolls.)
    e.preventDefault();
    view.cx += (e.deltaX * f) / view.k;
    view.cy += (e.deltaY * f) / view.k;
    clampView();
    draw();
  }, { passive: false });
  // (Safari's trackpad pinch.)
  // (Its ctrl+wheel events, should it send them too, aren't counted twice.)
  let gk = 0, gesturing = false;
  canvas.addEventListener("gesturestart", (e) => { e.preventDefault(); gk = view.k; gesturing = true; });
  canvas.addEventListener("gestureend", () => { gesturing = false; });
  canvas.addEventListener("gesturechange", (e) => { e.preventDefault(); const r = canvas.getBoundingClientRect(); zoomAt((gk * e.scale) / view.k, e.clientX - r.left, e.clientY - r.top); });
  canvas.addEventListener("keydown", (e) => {
    const step = 60 / view.k;
    const act = { "+": () => zoomBy(1.5), "=": () => zoomBy(1.5), "-": () => zoomBy(1 / 1.5), "0": () => { fit(); touchAction(); draw(); },
      ArrowLeft: () => { view.cx -= step; }, ArrowRight: () => { view.cx += step; }, ArrowUp: () => { view.cy -= step; }, ArrowDown: () => { view.cy += step; } }[e.key];
    if (!act) return;
    e.preventDefault();
    act();
    clampView();
    draw();
  });

  // (What the map shows, as a string: a refresh that changes none of it draws nothing.)
  let lastSig = "";
  const signature = (d) => [
    d.regions.map((r) => `${r.id}:${r.state}`).join(","),
    [...d.running].map(([k, r]) => `${k}:${r.step}:${r.frac.toFixed(3)}:${r.current ? 1 : 0}`).join(","),
    [...d.finished].map(([k, f]) => `${k}:${f.step}:${Math.round((d.now - f.t) / Math.max(1, d.now - d.from) * 50)}`).join(","),
    d.fade,
  ].join("|");

  // ---- Data --------------------------------------------------------------------------------------
  function update(next) {
    let outlines = next.countries !== data.countries;
    if (outlines) paths.countries = ringsPath(next.countries, true);
    if (next.regions !== data.regions) {
      const had = paths.regions;
      paths.regions = new Map();
      for (const r of next.regions) {
        if (had.get(r.id)?.rings !== r.fill) outlines = true;
        let p = r.fill && regionPaths.get(r.fill);
        if (!p) {
          p = { rings: r.fill, fill: ringsPath(r.fill, true), edge: ringsPath(r.edge, false), bbox: bboxOf(r.fill) };
          if (r.fill) regionPaths.set(r.fill, p);
        }
        paths.regions.set(r.id, p);
      }
    }
    // (Tiles new to the map fade in: at once as they come, staggered by their finish on the first
    // look, the window's history played back in a second.)
    const t = performance.now();
    const fresh = [...next.finished].filter(([k]) => !appeared.has(k)).sort((a, b) => a[1].t - b[1].t);
    fresh.forEach(([k], i) => appeared.set(k, firstFinished ? t + (i / Math.max(1, fresh.length)) * 1200 : t));
    if (fresh.length || next.finished.size) firstFinished = false;
    for (const k of appeared.keys()) if (!next.finished.has(k)) appeared.delete(k);
    // (Tiles keyed by step and tile, "<step> z/x/y": two steps' at one spot are each their own, drawn
    // side by side in the square, each a band of it, and each its own card in the stack.)
    atTile = new Map(); bandR = new Map(); bandF = new Map();
    const bands = (m, into) => {
      const by = new Map();
      for (const k of m.keys()) { const t = k.slice(k.lastIndexOf(" ") + 1); (by.get(t) || by.set(t, []).get(t)).push(k); }
      for (const ks of by.values()) ks.sort().forEach((k, i) => into.set(k, [i, ks.length]));
      for (const [t, ks] of by) atTile.set(t, [...new Set([...(atTile.get(t) || []), ...ks])]);
    };
    bands(next.running, bandR);
    bands(new Map([...next.finished].filter(([k]) => !next.running.has(k))), bandF);
    zooms = [...new Set([...next.running.keys(), ...next.finished.keys()].map((k) => tileOf(k)?.z).filter((z) => z != null))].sort((a, b) => b - a);
    const sig = signature(next);
    const same = sig === lastSig && !fresh.length && !outlines;
    lastSig = sig;
    data = next;
    if (!same) draw();
  }

  // A tile's minimap for its card: the tile and half a tile around it, as the map draws them.
  function minimap(key) {
    const cv = document.createElement("canvas");
    cv.className = "wmap-mini";
    if (tileOf(key)) paintMini(cv, key);
    return cv;
  }
  function paintMini(cv, key) {
    const [bx, by, s] = tileBox(tileOf(key));
    const w = 132, hgt = 132, dpr = window.devicePixelRatio || 1;
    cv.dataset.key = key;
    cv.width = w * dpr;
    cv.height = hgt * dpr;
    cv.style.width = `${w}px`;
    cv.style.height = `${hgt}px`;
    readTokens();
    scene(cv.getContext("2d"), { cx: bx + s / 2, cy: by + s / 2, k: w / (s * 2) }, w, hgt, dpr, `mt ${key}`);
  }

  // The region `id` lit as if pointed at (null: none), the map's own pointer first.
  function highlight(id) {
    const key = id ? `mr ${id}` : null;
    if (key === held) return;
    held = key;
    draw();
  }

  return { el, update, minimap, highlight };
}
