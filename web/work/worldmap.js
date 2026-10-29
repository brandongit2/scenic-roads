// The build page's world map, in its left panel (docs/pipeline.md §5): one canvas in Web
// Mercator (the world 0–1000 each way, y down: countries.json's, lakes.json's and /work/regions' outlines), with
// the z3 grid (and z6's when zoomed in enough to see it), the countries, the regions coloured by
// how far they are, the queue (task #685: every tile to build, faint under a dotted outline), each
// running job's target tiles in its stage's colour, filled as far as they're done, and the
// finished tiles (each fading in as it comes; completed but not in a catalogue yet pale under a
// dashed outline); a soloed step's published tiles painted by age (`paint`; none drawn without a
// solo, #833), its queued ones published before the same but fainter under their dotted outline, and
// a catalogue's tiles lit (`lit`). Steps at one spot share its square as diagonal stripes, each
// step's colour in turn.
// Kept across the page's renders, drawn again when its data changes, as it's moved or zoomed, and
// while tiles fade in; nothing runs otherwise.
//
// Moved by a drag (mouse, pen or one finger), zoomed by a pinch (a trackpad's: ctrl+wheel in
// Chrome, gesture events in Safari; two fingers on a touch screen), a double click, the +/−
// buttons or the keyboard (+, −, arrows, 0); a two-finger scroll moves it once zoomed in, else
// scrolls the page. Opens on the whole world. What's under the pointer is said by the page's cards,
// the whole stack there (every tile at every zoom, then the innermost region): under a mouse the map names
// it in its `data-card` ("mstack") and `data-also` (the keys, "|" between: dash.js wireCards); a
// click or a tap the map takes itself, at the point lifted, and hands the stack to `pick` (a
// tap's click isn't left to the page: Mobile Safari, #615). It draws a tile's minimap for its card
// (`minimap`).
import { areaOf, inRings, ringsArea, tileBox, tileOf, topRegion } from "./sums.js";

const FADE_MS = 900;
const MAX_K = 48; // pixels a world unit, most zoomed in (a z6 tile ≈ 750 px)
const Z6 = 1000 / 64;
// A tile area's outline weight (#659: sums.js AREAS), times a state's own: the root's heaviest, a
// deep tile's a hairline; the dashes stay the states' (completed dashed, to come dotted).
const AREA_W = { root: 2.4, lo: 1.7, hi: 1, deep: 0.6 };
// The regions' paths, made once, by outline.
const regionPaths = new WeakMap();

/** A map: `el` to place, `highlight(id)` to light a region, `emphasize(e)` a legend item's kind,
 * `update(data)` with {countries (rings), lakes (rings), regions [{id, fill, edge, state}], running (Map key →
 * {step, frac, current, state}), finished (Map key → {t, step, state}), queued (Map key → {step,
 * at, mac, state "queued"}: sums.js mapTiles), colour(step) (a CSS colour, var() allowed),
 * paint (Map key → colour: a soloed step's published tiles, and its queued ones published before, by age), lit ("z/x/y"
 * keys in litColour: a catalogue's, its z6 tiles filled)}, `minimap(key)` a canvas of a tile and
 * around it; `pick(keys, x, y)` called on a click or tap (keys: the stack there, none off
 * everything). Every key but lit's "<step> z/x/y". */
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

  let data = { countries: [], lakes: [], regions: [], running: new Map(), finished: new Map(), queued: new Map(), colour: () => "#888" };
  let paths = { countries: null, lakes: null, regions: new Map() };
  const view = { cx: 500, cy: 420, k: 0, fitted: false };
  let size = { w: 0, h: 0 };
  let hover = []; // the stack under the mouse, every key outlined
  let emph = null; // a legend item hovered: {region | tile | stage} (emphasize)
  let held = null; // a region the page points at (the Regions tab's list under the pointer)
  const appeared = new Map(); // finished tile → when it began fading in (performance.now())
  let firstFinished = true;
  let raf = 0, litRaf = 0;

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
  // The lit tiles (a catalogue's, its timeline mark hovered or held; a schedule job's or legend
  // item's), over everything: z6 filled, z3 outlined. `c` in the map's units, `px` a pixel in them.
  function litTiles(c, px) {
    for (const key of data.lit || []) {
      const tl = tileOf(key);
      if (!tl) continue;
      const [bx, by, s] = tileBox(tl);
      c.fillStyle = c.strokeStyle = data.litColour;
      if (tl.z === 6) c.fillRect(bx, by, s, s);
      c.lineWidth = px * 1.6;
      c.strokeRect(bx, by, s, s);
    }
  }
  // (The map's scene but its lit tiles, kept: a change of lit tiles alone (`light`) copies it and
  // draws them over it, not the thousands of tiles under them again.)
  const base = document.createElement("canvas");
  function withBase() {
    const c = canvas.getContext("2d"), dpr = window.devicePixelRatio || 1;
    c.setTransform(1, 0, 0, 1, 0, 0);
    c.drawImage(base, 0, 0);
    const k = view.k;
    c.setTransform(k * dpr, 0, 0, k * dpr, (size.w / 2 - view.cx * k) * dpr, (size.h / 2 - view.cy * k) * dpr);
    litTiles(c, 1 / k);
    c.setTransform(1, 0, 0, 1, 0, 0);
  }
  // The scene into `c` (css pixels w × h, ratio `dpr`) at view `v`; `lw` a line's width in pixels.
  function scene(c, v, w, hgt, dpr, focus, withLit = true) {
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
    // The large lakes, as the sea (drawn only: a point's stack is tiles and regions, a lake that is a
    // region hit as one).
    if (paths.lakes) {
      c.fillStyle = tokens.bg;
      c.fill(paths.lakes);
      c.strokeStyle = tokens.line;
      c.lineWidth = px;
      c.stroke(paths.lakes);
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
    // (What's pointed at: every key in `focus`, tiles and regions. A legend item hovered
    // (`emph`): what it isn't drawn faint.)
    const lit = new Set(Array.isArray(focus) ? focus : focus ? [focus] : []);
    const faint = (yes) => (yes ? 1 : 0.15);
    const regionKind = (st) => (st === "done" ? "on" : st);
    const regionRel = (r) => !emph || emph.region === regionKind(r.state);
    const tileRel = (v, key) => !emph || emph.tile === v.state || (emph.area != null && areaOf(key) === emph.area);
    // Regions, by state.
    const regionColour = { on: tokens.ok, done: tokens.ok, building: tokens.accent, queued: tokens.dim };
    for (const r of data.regions) {
      const p = paths.regions.get(r.id);
      if (!p || !seen(p.bbox)) continue;
      const col = regionColour[r.state] || tokens.dim, f = faint(regionRel(r));
      c.globalAlpha = (r.state === "on" ? 0.16 : r.state === "building" ? 0.14 : 0.07) * f;
      c.fillStyle = col;
      c.fill(p.fill);
      c.globalAlpha = (r.state === "queued" ? 0.55 : 0.85) * f;
      c.strokeStyle = col;
      c.lineWidth = px * 0.8;
      c.stroke(p.edge);
    }
    c.globalAlpha = 1;
    // Tiles. Steps sharing a spot: each its diagonal stripes, in turn, across the whole square.
    const t = performance.now();
    const stripes = (bx, by, s, i, n) => {
      c.save();
      c.beginPath();
      c.rect(bx, by, s, s);
      c.clip();
      if (n > 1) {
        const w = s / Math.max(8, n * 2);
        c.beginPath();
        for (let j = i, d = i * w; d < 2 * s; j += n, d = j * w) { c.moveTo(bx + d, by); c.lineTo(bx + d + w, by); c.lineTo(bx + d + w - 2 * s, by + 2 * s); c.lineTo(bx + d - 2 * s, by + 2 * s); c.closePath(); }
        c.clip();
      }
    };
    // A soloed step's published tiles (dash.js), each its age's colour, solid; the countries'
    // edges over them, to tell where a tile is.
    if (data.paint?.size) {
      for (const [key, col] of data.paint) {
        if (data.queued.has(key)) continue; // (drawn with the queue, fainter)
        const [bx, by, s] = tileBox(tileOf(key));
        if (bx > x1 || by > y1 || bx + s < x0 || by + s < y0) continue;
        const [i, n] = stripe.get(key) || [0, 1];
        stripes(bx, by, s, i, n);
        c.globalAlpha = 0.85;
        c.fillStyle = col;
        c.fillRect(bx, by, s, s);
        c.restore();
      }
      c.globalAlpha = 1;
      if (paths.countries) {
        c.globalAlpha = 0.6;
        c.strokeStyle = tokens.bg;
        c.lineWidth = px;
        c.stroke(paths.countries);
        c.globalAlpha = 1;
      }
    }
    const outlines = [];
    for (const [key, v] of [...data.queued, ...data.finished, ...data.running]) {
      const run = data.running.has(key) && data.running.get(key) === v;
      const later = !run && v.state === "queued";
      const [bx, by, s] = tileBox(tileOf(key.slice(key.lastIndexOf(" ") + 1)));
      if (bx > x1 || by > y1 || bx + s < x0 || by + s < y0) continue;
      const [i, n] = stripe.get(key) || [0, 1];
      const col = resolve(data.colour(v.step)), f = faint(tileRel(v, key)), aw = AREA_W[areaOf(key)] || 1;
      // (A low zoom's tile, one of thousands of square kilometres, is drawn paler: it would hide the rest.)
      const low = s > Z6 * 1.5 ? 0.3 : 1;
      stripes(bx, by, s, i, n);
      c.fillStyle = col;
      if (later) {
        // Queued: barely filled under a dotted outline, behind what's running or done; one
        // published before (a solo's: `paint` has it), its age's colour, fainter than a published
        // tile's, under the same outline (#833).
        const aged = data.paint?.get(key);
        if (aged) c.fillStyle = aged;
        c.globalAlpha = (aged ? 0.4 : 0.07) * f * low;
        c.fillRect(bx, by, s, s);
        outlines.push([bx, by, s, col, 0.5 * f * low, px * 0.9 * aw, [1.5 * px, 2 * px]]);
      } else if (run) {
        // Running: faint, filled from the bottom as far as done, outlined.
        c.globalAlpha = 0.12 * f;
        c.fillRect(bx, by, s, s);
        c.globalAlpha = 0.75 * f;
        c.fillRect(bx, by + s * (1 - v.frac), s, s * v.frac);
        outlines.push([bx, by, s, col, f, px * (v.current ? 2 : 1.2) * aw, null]);
      } else {
        // Finished: faded in as it came; completed (not in a catalogue yet) half as strong as
        // published, under a dashed outline.
        const fade = Math.max(0, Math.min(1, (t - (appeared.get(key) ?? 0)) / FADE_MS));
        const pub = v.state === "published";
        c.globalAlpha = fade * f * low * 0.6 * (pub ? 1 : 0.5);
        c.fillRect(bx, by, s, s);
        outlines.push(pub ? [bx, by, s, col, 0.55 * f * fade * low, px * 0.8 * aw, null] : [bx, by, s, col, f * fade * low, px * 1.1 * aw, [4 * px, 3 * px]]);
      }
      c.restore();
    }
    for (const [bx, by, s, col, f, lw, dash] of outlines) {
      c.globalAlpha = f;
      c.strokeStyle = col;
      c.lineWidth = lw;
      c.setLineDash(dash || []);
      c.strokeRect(bx, by, s, s);
    }
    c.setLineDash([]);
    c.globalAlpha = 1;
    if (withLit) litTiles(c, px);
    // What's pointed at: each region by its outline, each tile's whole square (counted: the
    // scene's answer, the canvas's `data-outlined` for the touch test).
    let outlined = 0;
    for (const k of lit) {
      if (k.startsWith("mr ")) {
        const p = paths.regions.get(k.slice(3));
        if (!p) continue;
        c.globalAlpha = 0.18;
        c.fillStyle = tokens.accent;
        c.fill(p.fill);
        c.globalAlpha = 1;
        c.strokeStyle = tokens.accent;
        c.lineWidth = px * 1.8;
        c.stroke(p.edge);
        outlined++;
      } else if (k.startsWith("mt ")) {
        const tile = tileOf(k.slice(k.lastIndexOf(" ") + 1));
        if (!tile) continue;
        const [bx, by, s] = tileBox(tile);
        c.strokeStyle = tokens.text;
        c.lineWidth = px * 1.5;
        c.strokeRect(bx, by, s, s);
        outlined++;
      }
    }
    c.setTransform(1, 0, 0, 1, 0, 0);
    return outlined;
  }

  function draw() {
    // (Not shown, its tab another: drawn when it's shown again, as its size comes.)
    if (!size.w || !canvas.offsetParent) return;
    readTokens();
    const dpr = window.devicePixelRatio || 1;
    if (base.width !== canvas.width || base.height !== canvas.height) { base.width = canvas.width; base.height = canvas.height; }
    canvas.dataset.outlined = scene(base.getContext("2d"), view, size.w, size.h, dpr, held ? [...hover, held] : hover, false);
    withBase();
    // (For the touch test: the tiles painted by age, the queued ones published before, the region the page lights, whether any tile still fades in.)
    canvas.dataset.painted = data.paint?.size || 0;
    canvas.dataset.queuedWas = data.paint?.size ? [...data.queued.keys()].filter((k) => data.paint.has(k)).length : 0;
    canvas.dataset.held = held || "";
    // (Still fading in: drawn again next frame.)
    const t = performance.now();
    let fading = false;
    for (const k of data.finished.keys()) if (t - (appeared.get(k) ?? 0) < FADE_MS) { fading = true; break; }
    canvas.dataset.fading = fading ? "1" : "";
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
  // the one region there (sums.js topRegion: the innermost, an island over the waters round it);
  // none outside the canvas.
  let zooms = [], atTile = new Map(), stripe = new Map();
  function stackAt(px, py) {
    if (px < 0 || py < 0 || px > size.w || py > size.h) return [];
    const [wx, wy] = toWorld(px, py);
    const out = [];
    for (const z of zooms) {
      const n = 2 ** z, key = `${z}/${Math.floor((wx / 1000) * n)}/${Math.floor((wy / 1000) * n)}`;
      for (const k of atTile.get(key) || []) out.push(`mt ${k}`);
    }
    const hits = [];
    for (const r of data.regions) {
      const p = paths.regions.get(r.id);
      if (p && wx >= p.bbox[0] && wx <= p.bbox[2] && wy >= p.bbox[1] && wy <= p.bbox[3] && inRings(p.rings, wx, wy)) hits.push({ id: r.id, area: p.area });
    }
    const top = topRegion(hits);
    if (top) out.push(`mr ${top}`);
    return out;
  }
  const local = (e) => { const r = canvas.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };
  // (A mouse's or pen's hover: the stack named for the page's hover card, each of it drawn lit. A
  // touch names nothing: its tap is `pick`ed.)
  const point = (e) => {
    if (e.pointerType === "touch") return;
    const keys = stackAt(...local(e));
    canvas.dataset.card = keys.length ? "mstack" : "";
    canvas.dataset.also = keys.join("|");
    if (keys.join("|") !== hover.join("|")) { hover = keys; draw(); }
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
    if (hover.length) { hover = []; draw(); }
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

  // (What the map shows, as a string: a refresh that changes none of it draws nothing. The
  // thousands of finished, queued and painted tiles' parts kept by their Map: dash.js keeps those
  // Maps while they're the same.)
  let lastSig = "", lastRunKeys = null;
  const parts = new WeakMap();
  const part = (m, make) => { if (!m) return ""; let p = parts.get(m); if (p == null) parts.set(m, (p = make(m))); return p; };
  const signature = (d) => [
    d.regions.map((r) => `${r.id}:${r.state}`).join(","),
    [...d.running].map(([k, r]) => `${k}:${r.step}:${r.frac.toFixed(3)}:${r.current ? 1 : 0}`).join(","),
    part(d.finished, (m) => [...m].map(([k, f]) => `${k}:${f.state}`).join(",")),
    part(d.queued, (m) => [...m.keys()].join(",")),
    part(d.paint, (m) => [...m].join(",")),
    d.lit ? [...d.lit].join(",") : "",
  ].join("|");

  // ---- Data --------------------------------------------------------------------------------------
  function update(next) {
    let outlines = next.countries !== data.countries;
    if (outlines) paths.countries = ringsPath(next.countries, true);
    if (next.lakes !== data.lakes) { outlines = true; paths.lakes = ringsPath(next.lakes, true); }
    if (next.regions !== data.regions) {
      const had = paths.regions;
      paths.regions = new Map();
      for (const r of next.regions) {
        if (had.get(r.id)?.rings !== r.fill) outlines = true;
        let p = r.fill && regionPaths.get(r.fill);
        if (!p) {
          p = { rings: r.fill, fill: ringsPath(r.fill, true), edge: ringsPath(r.edge, false), bbox: bboxOf(r.fill), area: ringsArea(r.fill) };
          if (r.fill) regionPaths.set(r.fill, p);
        }
        paths.regions.set(r.id, p);
      }
    }
    // (Tiles new to the map fade in: at once as they come, staggered by their finish on the first
    // look, the window's history played back in a second; none new while they're the same Map.)
    const t = performance.now();
    const fresh = next.finished === data.finished ? [] : [...next.finished].filter(([k]) => !appeared.has(k)).sort((a, b) => a[1].t - b[1].t);
    fresh.forEach(([k], i) => appeared.set(k, firstFinished ? t + (i / Math.max(1, fresh.length)) * 1200 : t));
    if (fresh.length || next.finished.size) firstFinished = false;
    if (next.finished !== data.finished) for (const k of appeared.keys()) if (!next.finished.has(k)) appeared.delete(k);
    // (Tiles keyed by step and tile, "<step> z/x/y": two steps' at one spot are each their own,
    // drawn as diagonal stripes across the square in turn (`stripe`: [i, n]), each its own card.
    // Worked out again only when the tiles drawn are others.)
    const runKeys = [...next.running.keys()].join("\n");
    if (next.finished !== data.finished || next.queued !== data.queued || next.paint !== data.paint || runKeys !== lastRunKeys) {
      lastRunKeys = runKeys;
      atTile = new Map(); stripe = new Map();
      const keys = [...next.running.keys(), ...next.finished.keys(), ...next.queued.keys(), ...(next.paint?.keys() || [])];
      for (const k of keys) { const t = k.slice(k.lastIndexOf(" ") + 1); (atTile.get(t) || atTile.set(t, []).get(t)).push(k); }
      for (const ks of atTile.values()) { ks.sort(); ks.forEach((k, i) => stripe.set(k, [i, ks.length])); }
      zooms = [...new Set(keys.map((k) => tileOf(k)?.z).filter((z) => z != null))].sort((a, b) => b - a);
    }
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

  // A legend item hovered (#627, #659): {region: "on" | "building" | "queued"}, {tile: state} or
  // {area: name}, what it isn't drawn faint; null: all as they are. (A step's or stage's solos it:
  // dash.js.)
  function emphasize(e) {
    if (JSON.stringify(e) === JSON.stringify(emph)) return;
    emph = e;
    draw();
  }

  // The region `id` lit as if pointed at (null: none), the map's own pointer first.
  function highlight(id) {
    const key = id ? `mr ${id}` : null;
    if (key === held) return;
    held = key;
    draw();
  }

  // Only the lit tiles changed (the schedule's job or legend item hovered, task #787): drawn
  // again without working the rest out again as `update` does.
  function light(lit) {
    if (lit === data.lit) return;
    data = { ...data, lit };
    lastSig = "";
    // (Once a frame, however fast the pointer crosses jobs.)
    cancelAnimationFrame(litRaf);
    litRaf = requestAnimationFrame(() => { if (size.w && canvas.offsetParent && base.width) withBase(); });
  }

  return { el, update, light, minimap, highlight, emphasize };
}

// A world map for a Jobs card (#659, as the mockup's): the whole world square, the countries
// (`countries`, rings in the map's 0–1000) faintly, the tiles a job names lit in its colour
// (`colour`, var() allowed), each at least 5 px; no grid, no regions. Small until its card is
// pinned (dash.css .tc-map).
const countryLayers = new WeakMap();
export function tilesMap(countries, tiles, colour) {
  const size = 520, dpr = window.devicePixelRatio || 1;
  const cv = document.createElement("canvas");
  cv.className = "tc-map";
  cv.width = cv.height = size * dpr;
  cv.dataset.tiles = tiles.length;
  const g = cv.getContext("2d");
  g.scale(dpr, dpr);
  const cs = getComputedStyle(document.documentElement);
  const v = (k) => cs.getPropertyValue(k).trim() || "#888";
  // (The countries drawn once a theme and pixel ratio, kept: a card a hover, each its copy.)
  const sig = `${dpr} ${v("--bg")} ${v("--dim")}`;
  let under = Array.isArray(countries) && countryLayers.get(countries);
  if (!under || under.sig !== sig) {
    const c = document.createElement("canvas");
    c.width = c.height = size * dpr;
    const u = c.getContext("2d");
    u.scale(dpr, dpr);
    u.fillStyle = v("--bg");
    u.fillRect(0, 0, size, size);
    if (Array.isArray(countries)) {
      const p = new Path2D();
      for (const r of countries) {
        if (r.length < 4) continue;
        p.moveTo(r[0], r[1]);
        for (let k = 2; k + 1 < r.length; k += 2) p.lineTo(r[k], r[k + 1]);
        p.closePath();
      }
      u.scale(size / 1000, size / 1000);
      u.globalAlpha = 0.55;
      u.strokeStyle = v("--dim");
      u.lineWidth = 0.6 * (1000 / size);
      u.stroke(p);
    }
    under = { sig, c };
    if (Array.isArray(countries)) countryLayers.set(countries, under);
  }
  g.drawImage(under.c, 0, 0, size, size);
  const el = document.createElement("i");
  el.style.color = colour || "var(--accent)";
  document.body.append(el);
  const col = getComputedStyle(el).color;
  el.remove();
  g.lineWidth = 1.2;
  for (const t of tiles) {
    const tile = tileOf(t);
    if (!tile) continue;
    let [x, y, w] = tileBox(tile).map((q) => (q * size) / 1000), hh = w;
    if (w < 5) { x -= (5 - w) / 2; y -= (5 - w) / 2; w = hh = 5; }
    g.globalAlpha = tiles.length > 1 ? 0.45 : 0.6;
    g.fillStyle = col;
    g.fillRect(x, y, w, hh);
    g.globalAlpha = 1;
    g.strokeStyle = col;
    g.strokeRect(x + 0.5, y + 0.5, w - 1, hh - 1);
  }
  g.strokeStyle = v("--line");
  g.lineWidth = 1;
  g.strokeRect(0.5, 0.5, size - 1, size - 1);
  return cv;
}
