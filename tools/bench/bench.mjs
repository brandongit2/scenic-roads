// Frame-budget benchmark for the map app, driven through the Chrome DevTools Protocol.
//
// Start a Chrome with the real GPU (Metal) and remote debugging, then run scenarios against the app
// served by the backend (the launch config's "backend", or the installed app):
//
//   "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --remote-debugging-port=9333 \
//     --user-data-dir=/tmp/bench-chrome --use-angle=metal --enable-gpu --ignore-gpu-blocklist \
//     --disable-gpu-vsync --disable-frame-rate-limit about:blank
//
// The last two flags uncap the frame rate (headless Chrome otherwise ticks at 60 Hz), so fps is
// throughput and the frame intervals can be read against a 120 Hz display's 8.3 ms budget.
//   node tools/bench/bench.mjs [--url URL] [--runs pan,pinch,orbit,hover] [--secs 5] [--profile [--profile-out f]] [--trace file.json]
//                              [--set "js run in the page before the runs, e.g. __app.store.set({...})"] [--label name]
//
// Input is real: trackpad two-finger pans, pinches and ⌥-orbits are wheel events dispatched to the
// map canvas at 120 Hz (the app's own trackpad handling runs), hovering is mouse moves. Measured per
// run, on the page's main thread: frame intervals (requestAnimationFrame), long tasks, MapLibre's
// render call (CPU), the GPU time of each frame (EXT_disjoint_timer_query_webgl2), and after the
// gesture how long until the map is idle again. --profile adds a CPU profile (self time by
// function); --trace records a timeline of every thread (workers included) and sums busy time per
// thread.
import fs from 'node:fs';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
    return acc;
  }, []),
);
const PORT = Number(args.port ?? 9333);
const W = Number(args.w ?? 1512), H = Number(args.h ?? 900), DPR = Number(args.dpr ?? 2);
const SECS = Number(args.secs ?? 5);
const RUNS = String(args.runs ?? 'pan,pinch,orbit,hover').split(',');
/** The camera's least height over the ground under it and over the highest ground in view (m, as drawn). */
const CLEAR_M = Number(args['clear-m'] ?? 200);
const URL = String(args.url ?? 'http://localhost:7311/');

// ---- CDP plumbing --------------------------------------------------------------------------
const target = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: 'PUT' })).json();
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let seq = 0;
const pending = new Map();
const listeners = new Map();
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    pending.get(m.id)(m);
    pending.delete(m.id);
  } else if (m.method) (listeners.get(m.method) ?? []).forEach((f) => f(m.params));
});
const send = (method, params = {}) =>
  new Promise((res, rej) => {
    const id = ++seq;
    pending.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
const on = (method, f) => listeners.set(method, [...(listeners.get(method) ?? []), f]);
const evaluate = async (expr, timeout = 120) => {
  const r = await send('Runtime.evaluate', { expression: `(async () => { ${expr} })()`, awaitPromise: true, returnByValue: true, timeout: timeout * 1000 });
  if (r.exceptionDetails) throw new Error(`page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
  return r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// (Node's WebSocket doesn't hold the process open: a long call, e.g. a screenshot of a busy page,
// otherwise ended it mid-await.)
const keepAlive = setInterval(() => {}, 1000);
const close = async () => {
  clearInterval(keepAlive);
  ws.close();
  await fetch(`http://127.0.0.1:${PORT}/json/close/${target.id}`).catch(() => {});
};

// Uncaught exceptions in the page (reported with the results).
const exceptions = [];
on('Runtime.exceptionThrown', (p) => exceptions.push(`${p.exceptionDetails.exception?.description ?? p.exceptionDetails.text}`.split('\n').slice(0, 3).join(' | ')));
// A crashed page never answers: report it and stop rather than wait forever.
on('Inspector.targetCrashed', () => {
  console.error('page crashed');
  console.log(JSON.stringify({ crashed: true, exceptions }));
  process.exit(3);
});

try {
  await send('Runtime.enable');
  await send('Page.enable');
  await send('Inspector.enable').catch(() => {});
  // --native: a real (headed) window at its own size and pixel ratio, paced by the display.
  if (!args.native) await send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: DPR, mobile: false });
  await send('Emulation.setFocusEmulationEnabled', { enabled: true });
  await send('Page.bringToFront');

  // ---- load --------------------------------------------------------------------------------
  // --profile-load: a CPU profile from navigation to settled.
  if (args['profile-load']) {
    await send('Profiler.enable');
    await send('Profiler.setSamplingInterval', { interval: 500 });
    await send('Profiler.start');
  }
  // --cold: the browser's HTTP cache emptied first, so every tile comes from the server (a
  // first visit to the view: what the owner sees while tiles load).
  if (args.cold) {
    await send('Network.enable');
    await send('Network.clearBrowserCache');
  }
  // Long tasks and long animation frames (over 50 ms, with the scripts that ran in them: what the
  // long tasks were) observed from the page's first script on, so the load's own count too.
  const EARLY = `window.__early = { lt: [], loaf: [], frames: [] };
    { const tick = (now) => { __early.frames.push(now); requestAnimationFrame(tick); }; requestAnimationFrame(tick); }
    new PerformanceObserver((l) => { for (const e of l.getEntries()) __early.lt.push([e.startTime, e.duration]); }).observe({ type: 'longtask', buffered: true });
    try {
      new PerformanceObserver((l) => { for (const e of l.getEntries()) __early.loaf.push([e.startTime, e.duration, e.blockingDuration,
        (e.scripts ?? []).filter((s) => s.duration >= 5).sort((a, b) => b.duration - a.duration).slice(0, 4)
          .map((s) => \`\${Math.round(s.duration)} \${s.invokerType}:\${s.invoker} \${s.sourceFunctionName || ''}@\${(s.sourceURL || '').split('/').pop()}:\${s.sourceCharPosition}\`)]); })
        .observe({ type: 'long-animation-frame', buffered: true });
    } catch {}`;
  await send('Page.addScriptToEvaluateOnNewDocument', { source: EARLY });
  const tLoad = Date.now();
  await send('Page.navigate', { url: URL });
  for (let i = 0; i < 240; i++) {
    const ok = await evaluate('return !!(window.__app && window.__app.map && window.__app.map.loaded())').catch(() => false);
    if (ok) break;
    await sleep(250);
  }
  if (args.set) await evaluate(String(args.set));
  // The camera over the landscape (the terrain exaggerated as drawn, queryTerrainElevation's): its
  // height over the ground under it (under), and over the highest ground in front of it (over: five
  // lines across the view out to the centre's distance, 48 points each: a valley's walls, a ridge).
  // A view under the ground or walled in measures nothing useful (CLEAR_M below).
  await evaluate(`window.__clearance = () => {
    const map = __app.map, tr = map._camera?.transform ?? map.transform;
    if (!map.getTerrain()) return null;
    const cam = tr.getCameraLngLat(), alt = tr.getCameraAltitude();
    const q = (lng, lat) => map.queryTerrainElevation([lng, lat]) ?? 0;
    const under = alt - q(cam.lng, cam.lat);
    const c = map.getCenter();
    const mPerDegLat = 111320, mPerDegLng = 111320 * Math.cos(cam.lat * Math.PI / 180);
    const dx = (c.lng - cam.lng) * mPerDegLng, dy = (c.lat - cam.lat) * mPerDegLat;
    const dist = Math.hypot(dx, dy), az = Math.atan2(dx, dy);
    const hfov = 2 * Math.atan(Math.tan((tr.fov ?? 36.87) * Math.PI / 360) * innerWidth / innerHeight);
    let top = -Infinity, at = null;
    for (const f of [-0.45, -0.22, 0, 0.22, 0.45]) {
      const a = az + f * hfov, tx = cam.lng + Math.sin(a) * dist / mPerDegLng, ty = cam.lat + Math.cos(a) * dist / mPerDegLat;
      for (let i = 1; i <= 48; i++) {
        const s = i / 48, lng = cam.lng + (tx - cam.lng) * s, lat = cam.lat + (ty - cam.lat) * s;
        const g = q(lng, lat);
        if (g > top) { top = g; at = [+lng.toFixed(4), +lat.toFixed(4)]; }
      }
    }
    return { under: Math.round(under), over: Math.round(alt - top), highest: at, camAlt: Math.round(alt), dist: Math.round(dist) };
  };`);
  // Instrument: frames, long tasks, MapLibre's render (CPU), GPU time per frame.
  const load = await evaluate(`
    const app = window.__app, map = app.map;
    const B = window.__bench = { lt: [], renders: [], gpu: [], gpuMoved: [], frames: [], custom: {}, lastCam: '' };
    // (the observers installed before the page's own scripts ran: EARLY below)
    B.lt = window.__early.lt;
    B.loaf = window.__early.loaf;
    const gl = map.painter.context.gl;
    const tq = gl.getExtension('EXT_disjoint_timer_query_webgl2');
    const queries = [];
    const orig = map._render.bind(map);
    // Frames that moved the camera are kept apart: uncapped, many frames render between two input
    // events with the camera unchanged, and those can reuse work (e.g. cached projections), so the
    // moving frames' cost is what a display gets on every frame of a gesture.
    map._render = function (t) {
      const a = performance.now();
      const c = map.getCenter(), cam = [c.lng, c.lat, map.getZoom(), map.getBearing(), map.getPitch()].join(',');
      const moved = cam !== B.lastCam;
      B.lastCam = cam;
      let q = null;
      // (--set instrumentation running timer queries of its own sets window.__noFrameQuery: one query at a time)
      if (tq && !B.qBusy && !window.__noFrameQuery) { q = gl.createQuery(); gl.beginQuery(tq.TIME_ELAPSED_EXT, q); B.qBusy = true; }
      const r = orig(t);
      if (q) { gl.endQuery(tq.TIME_ELAPSED_EXT); B.qBusy = false; queries.push([q, moved]); }
      B.renders.push([a, performance.now() - a, moved]);
      for (let i = queries.length - 1; i >= 0; i--) {
        const [qq, mv] = queries[i];
        if (gl.getQueryParameter(qq, gl.QUERY_RESULT_AVAILABLE)) {
          if (!gl.getParameter(tq.GPU_DISJOINT_EXT)) {
            const ms = gl.getQueryParameter(qq, gl.QUERY_RESULT) / 1e6;
            B.gpu.push(ms);
            if (mv) B.gpuMoved.push(ms);
          }
          gl.deleteQuery(qq);
          queries.splice(i, 1);
        }
      }
      return r;
    };
    for (const [k, layer] of [['roads', app.roads], ['rails', app.rails]]) {
      const f = layer.render.bind(layer);
      B.custom[k] = [];
      layer.render = function (...a) { const t = performance.now(); const r = f(...a); B.custom[k].push(performance.now() - t); return r; };
    }
    B.frames = window.__early.frames;
    // Settled: idle, and the road tiles in view loaded.
    const t0 = performance.now();
    await new Promise((res) => {
      const check = () => { const p = app.roads.progress(); if (map.loaded() && p.loaded >= p.wanted) res(); else setTimeout(check, 200); };
      check();
    });
    await new Promise((r) => { map.once('idle', r); map.triggerRepaint(); setTimeout(r, 30000); });
    const res = performance.getEntriesByType('resource');
    const byType = {};
    for (const e of res) { const k = e.name.includes('/api/layer/') ? 'layer:' + e.name.split('/api/layer/')[1].split('?')[0] : e.name.match(/\\/(tiles|api\\/[a-z]+|assets)/)?.[1] ?? 'other'; byType[k] = (byType[k] ?? 0) + (e.transferSize || e.encodedBodySize || 0); }
    const lt = B.lt.filter(([s]) => s < performance.now());
    // Frame intervals from the page's scripts run (DOMContentLoaded) to idle: the view loading.
    const f0 = performance.getEntriesByType('navigation')[0]?.domContentLoadedEventEnd ?? 0, fl = B.frames.filter((t) => t >= f0);
    const fdt = fl.slice(1).map((t, i) => t - fl[i]).sort((a, b) => a - b);
    const qq = (a, p) => (a.length ? +a[Math.min(a.length - 1, Math.floor(p * a.length))].toFixed(1) : null);
    return {
      settleMs: Math.round(performance.now() - t0), navToIdleMs: Math.round(performance.now()),
      frameMs: { p50: qq(fdt, 0.5), p95: qq(fdt, 0.95), max: qq(fdt, 1), n: fdt.length },
      longTasks: lt.length, longTaskMs: Math.round(lt.reduce((a, [, d]) => a + d, 0)), longestTaskMs: Math.round(Math.max(0, ...lt.map(([, d]) => d))),
      topTasks: lt.map(([, d]) => Math.round(d)).sort((a, b) => b - a).slice(0, 6),
      topFrames: [...B.loaf].sort((a, b) => b[1] - a[1]).slice(0, 4).map(([, d, , sc]) => [Math.round(d), ...sc]),
      heapMB: performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1e6) : null,
      transferMB: Object.fromEntries(Object.entries(byType).map(([k, v]) => [k, +(v / 1e6).toFixed(2)]).sort((a, b) => b[1] - a[1]).slice(0, 12)),
      layers: map.getStyle().layers.length, visibleLayers: map.getStyle().layers.filter((l) => (l.layout?.visibility ?? 'visible') !== 'none').length,
      sources: Object.keys(map.getStyle().sources).length,
      viewport: \`\${innerWidth}x\${innerHeight}@\${devicePixelRatio}\`,
    };
  `, 300);
  if (args['profile-load']) {
    const { profile } = await send('Profiler.stop');
    load.profile = topFunctions(profile, 25);
    if (args['profile-out']) fs.writeFileSync(String(args['profile-out']).replace(/(\.cpuprofile)?$/, '-load.cpuprofile'), JSON.stringify(profile));
  }
  load.clearance = await evaluate('return window.__clearance()');
  // The map's covered regions (the server's /api/coverage): outside them it draws little, so a view
  // whose centre is outside, at the start or along a run, measures nothing (--anywhere: measured
  // anyway). Even-odd over every ring of every region.
  await evaluate(`const fc = await (await fetch('/api/coverage')).json();
    const rings = [];
    for (const f of fc.features) for (const poly of (f.geometry.type === 'Polygon' ? [f.geometry.coordinates] : f.geometry.coordinates)) for (const r of poly) {
      let w = 180, e = -180, so = 90, n = -90;
      for (const [x, y] of r) { w = Math.min(w, x); e = Math.max(e, x); so = Math.min(so, y); n = Math.max(n, y); }
      rings.push({ r, box: [w, so, e, n] });
    }
    window.__covered = (lng, lat) => {
      let inside = false;
      for (const { r, box } of rings) {
        if (lng < box[0] || lng > box[2] || lat < box[1] || lat > box[3]) continue;
        for (let i = 0, j = r.length - 1; i < r.length; j = i++) {
          const [xi, yi] = r[i], [xj, yj] = r[j];
          if ((yi > lat) !== (yj > lat) && lng < ((xj - xi) * (lat - yi)) / (yj - yi) + xi) inside = !inside;
        }
      }
      return inside;
    };
    const c0 = __clearance; window.__clearance = () => { const c = c0() ?? {}; const m = __app.map.getCenter(); c.covered = __covered(m.lng, m.lat); c.center = [+m.lng.toFixed(3), +m.lat.toFixed(3)]; return c; };`, 120);
  load.clearance = await evaluate('return window.__clearance()');
  const outside = (c, where) => {
    if (!c || args.anywhere || c.covered !== false) return;
    console.error(`view's centre outside the map's coverage ${where}: move the view into a covered region`);
    console.log(JSON.stringify({ outsideCoverage: where }));
    process.exitCode = 5;
    throw new Error('outside coverage');
  };
  outside(load.clearance, 'at the start');
  // (--any-camera: measured wherever the camera is)
  const lowCamera = (c, where) => {
    if (!c || args['any-camera'] || c.under === undefined || (c.under >= CLEAR_M && c.over >= CLEAR_M)) return;
    console.error(`camera too low ${where}: ${c.under} m over the ground under it, ${c.over} m over the highest ground in view (at least ${CLEAR_M} m each): raise the view`);
    console.log(JSON.stringify({ lowCamera: where, clearance: c }));
    process.exitCode = 4;
    throw new Error('camera too low');
  };
  lowCamera(load.clearance, 'at the start');
  const heap = await send('Runtime.getHeapUsage').catch(() => null);
  load.wallMs = Date.now() - tLoad;
  if (heap) load.heapMB = Math.round(heap.usedSize / 1e6);
  const out = { label: args.label ?? '', url: URL.slice(0, 120), viewport: load.viewport, load, runs: {} };
  // --shot file.png: the map's canvas (device pixels) once the loaded view is idle (after --set).
  if (args.shot) {
    await evaluate(`const map = window.__app.map; await new Promise((r) => { map.once('idle', r); map.triggerRepaint(); setTimeout(r, 20000); }); await new Promise((r) => setTimeout(r, 500));`);
    // (the map's canvas read in its own render event, while its drawing buffer holds the frame:
    // Page.captureScreenshot never answers in a headless Chrome with the frame rate uncapped)
    const data = await evaluate(`const map = window.__app.map, c = map.getCanvas();
      return await new Promise((r) => { map.once('render', () => r(c.toDataURL('image/png'))); map.triggerRepaint(); });`);
    fs.writeFileSync(String(args.shot), Buffer.from(data.split(',')[1], 'base64'));
  }

  // ---- gesture runs --------------------------------------------------------------------------
  const cx = W * 0.55, cy = H * 0.55;
  const wheel = (dx, dy, modifiers = 0, x = cx, y = cy) =>
    send('Input.dispatchMouseEvent', { type: 'mouseWheel', x, y, deltaX: dx, deltaY: dy, modifiers, pointerType: 'mouse' });
  // The pan's speed below zoom 10: halved a zoom level out (a flick that crosses a city at 14
  // crossed a continent at 6, and the centre left the map's coverage).
  const zoom0 = await evaluate('return window.__app.map.getZoom()');
  const panK = Math.min(1, 2 ** (zoom0 - 10));
  // The view's centre on screen (the map's padding, e.g. a side panel, moves it off the middle):
  // the pinch and the orbit act there, so the centre stays put and the view stays on the map.
  const [mx, my] = await evaluate('const m = window.__app.map, p = m.project(m.getCenter()), r = m.getCanvas().getBoundingClientRect(); return [r.left + p.x, r.top + p.y]');
  const gestures = {
    // Two-finger pan: a steady drag, reversing every 1.25 s (so the view stays within ~60 km of its
    // start at zoom 11 and comes back; an island or a coast keeps the centre on the map).
    pan: (t) => { const s = Math.floor(t / 1.25) % 2 ? -1 : 1; return wheel(9 * s * panK, 5 * s * panK); },
    // Pinch (ctrl + wheel) at the view's centre (so the centre stays put): zoom out 2.5 levels, then
    // in again (out first: the camera only rises, so a tilted city view stays over CLEAR_M; in
    // first, Tokyo's at 16.5 was 156 m over the ground).
    pinch: (t) => wheel(0, t < SECS / 2 ? 3.4 : -3.4, 2, mx, my),
    // ⌥ + two-finger drag at the centre: rotate, and tilt down then back (never further up: the
    // camera stays over what it clears at the start; tilting up first put the Alps' peaks over it).
    orbit: (t) => wheel(t < SECS / 2 ? 6 : -6, t < SECS / 4 || t > (3 * SECS) / 4 ? 2 : -2, 1, mx, my),
    // Hovering over roads and places: mouse moves sweeping the view.
    hover: (t) => send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: W * (0.3 + 0.4 * ((t * 0.37) % 1)), y: H * (0.3 + 0.4 * ((t * 0.23) % 1)) }),
    // No input: the same view redrawn every frame (render cost alone: CPU and GPU).
    static: null,
  };
  // --variants: the static render with parts of the state switched off, one at a time (GPU and CPU
  // cost of each feature), as [label, JS expression patching the state `s` in place].
  if (args.variants) {
    const variants = JSON.parse(fs.readFileSync(String(args.variants), 'utf8'));
    out.variants = {};
    await evaluate(`window.__S0 = structuredClone(window.__app.store.s);`);
    for (const [label, patch] of variants) {
      out.variants[label] = await evaluate(`
        const app = window.__app, map = app.map, B = window.__bench;
        const s = structuredClone(window.__S0);
        ${patch};
        app.store.set(s);
        await new Promise((r) => { map.once('idle', r); map.triggerRepaint(); setTimeout(r, 30000); });
        await new Promise((r) => setTimeout(r, 300));
        B.renders.length = 0; B.gpu.length = 0; B.gpuMoved.length = 0;
        const t0 = performance.now();
        await new Promise((res) => { const f = () => { if (performance.now() - t0 > ${SECS * 1000}) return res(); map.triggerRepaint(); requestAnimationFrame(f); }; f(); });
        await new Promise((r) => setTimeout(r, 200));
        const q = (a, p) => (a.length ? +a[Math.min(a.length - 1, Math.floor(p * a.length))].toFixed(1) : null);
        const rd = B.renders.map(([, d]) => d).sort((a, b) => a - b), gpu = [...B.gpu].sort((a, b) => a - b);
        app.store.set(structuredClone(window.__S0));
        return { gpuP50: q(gpu, 0.5), gpuP95: q(gpu, 0.95), cpuP50: q(rd, 0.5), cpuP95: q(rd, 0.95), frames: rd.length };
      `, 120);
      console.error(label, JSON.stringify(out.variants[label]));
    }
  }
  for (const name of RUNS) {
    // A digit suffix repeats a run (pan2: the pan again, e.g. with its tiles and shaders warm).
    const g = gestures[name.replace(/\d+$/, '')];
    if (!(name.replace(/\d+$/, '') in gestures)) continue;
    await evaluate(`const B = window.__bench; B.lt.length = 0; B.loaf.length = 0; B.clear = []; B.nr = 0; if (!B.clearOn) { B.clearOn = true; window.__app.map.on('render', () => { if (B.clear && ++B.nr % 15 === 0) { const c = window.__clearance(); if (c) B.clear.push(c); } }); } B.renders.length = 0; B.gpu.length = 0; B.gpuMoved.length = 0; B.frames.length = 0; for (const k in B.custom) B.custom[k].length = 0; B.t0 = performance.now();`);
    if (args.profile) {
      await send('Profiler.enable');
      await send('Profiler.setSamplingInterval', { interval: 200 });
      await send('Profiler.start');
    }
    let traceDone = null;
    if (args.trace) {
      const chunks = [];
      on('Tracing.dataCollected', (p) => chunks.push(...p.value));
      traceDone = new Promise((r) => on('Tracing.tracingComplete', () => r(chunks)));
      await send('Tracing.start', { traceConfig: { includedCategories: ['devtools.timeline', 'disabled-by-default-devtools.timeline', 'v8.execute', 'blink', 'gpu', 'toplevel', 'disabled-by-default-devtools.timeline.frame'] }, transferMode: 'ReportEvents' });
    }
    const kind = name.replace(/\d+$/, '');
    if (kind === 'static') await evaluate(`const map = window.__app.map, B = window.__bench; B.repaint = true; const f = () => { if (!B.repaint) return; map.triggerRepaint(); requestAnimationFrame(f); }; f();`);
    const t0 = Date.now();
    while (kind === 'static' && Date.now() - t0 < SECS * 1000) await sleep(50);
    if (kind === 'static') await evaluate(`window.__bench.repaint = false;`);
    while (g && Date.now() - t0 < SECS * 1000) {
      const t = (Date.now() - t0) / 1000;
      const a = Date.now();
      await g(t);
      const wait = (kind === 'hover' ? 16 : 8) - (Date.now() - a);
      if (wait > 0) await sleep(wait);
    }
    const r = await evaluate(`
      const B = window.__bench, map = window.__app.map;
      const tEnd = performance.now();
      await new Promise((r) => { map.once('idle', r); map.triggerRepaint(); setTimeout(r, 30000); });
      const settleMs = performance.now() - tEnd;
      const fr = B.frames.filter((t) => t >= B.t0 && t <= tEnd);
      const dts = fr.slice(1).map((t, i) => t - fr[i]).sort((a, b) => a - b);
      const q = (a, p) => (a.length ? +a[Math.min(a.length - 1, Math.floor(p * a.length))].toFixed(1) : null);
      const during = B.lt.filter(([s]) => s >= B.t0 && s <= tEnd), after = B.lt.filter(([s]) => s > tEnd);
      const rd = B.renders.filter(([s]) => s >= B.t0 && s <= tEnd).map(([, d]) => d).sort((a, b) => a - b);
      const rdMoved = B.renders.filter(([s, , m]) => m && s >= B.t0 && s <= tEnd).map(([, d]) => d).sort((a, b) => a - b);
      const gpu = [...B.gpu].sort((a, b) => a - b), gpuMoved = [...B.gpuMoved].sort((a, b) => a - b);
      const custom = Object.fromEntries(Object.entries(B.custom).map(([k, v]) => { const s = [...v].sort((a, b) => a - b); return [k, { p50: q(s, 0.5), p95: q(s, 0.95) }]; }));
      return {
        fps: +(fr.length / ((tEnd - B.t0) / 1000)).toFixed(1),
        frameMs: { p50: q(dts, 0.5), p90: q(dts, 0.9), p95: q(dts, 0.95), p99: q(dts, 0.99), max: q(dts, 1) },
        over8ms: dts.filter((d) => d > 8.4).length, over17ms: dts.filter((d) => d > 16.8).length, over50ms: dts.filter((d) => d > 50).length, frames: dts.length,
        renderCpuMs: { p50: q(rd, 0.5), p95: q(rd, 0.95), max: q(rd, 1), n: rd.length },
        gpuMs: { p50: q(gpu, 0.5), p95: q(gpu, 0.95), n: gpu.length },
        // Frames that moved the camera: what every frame of a gesture costs on a display.
        moved: { cpu: { p50: q(rdMoved, 0.5), p95: q(rdMoved, 0.95), n: rdMoved.length }, gpu: { p50: q(gpuMoved, 0.5), p95: q(gpuMoved, 0.95), n: gpuMoved.length } },
        customMs: custom,
        longTasks: { during: during.length, duringMs: Math.round(during.reduce((a, [, d]) => a + d, 0)), after: after.length, afterMs: Math.round(after.reduce((a, [, d]) => a + d, 0)), longest: Math.round(Math.max(0, ...during.map(([, d]) => d))),
          top: during.map(([, d]) => Math.round(d)).sort((a, b) => b - a).slice(0, 6) },
        // The longest animation frames during the run, each with its longest scripts.
        topFrames: B.loaf.filter(([s]) => s >= B.t0 && s <= tEnd).sort((a, b) => b[1] - a[1]).slice(0, 4).map(([, d, , sc]) => [Math.round(d), ...sc]),
        settleMs: Math.round(settleMs),
        // The least clearance along the run (sampled every 15th frame): under the camera, sight lines.
        clearance: B.clear.length ? { under: Math.min(...B.clear.map((c) => c.under ?? Infinity)), over: Math.min(...B.clear.map((c) => c.over ?? Infinity)), covered: B.clear.every((c) => c.covered !== false), n: B.clear.length,
          // (where the run took the view, and where it first left the map: to place a view that strays)
          end: (() => { const c = map.getCenter(); return [+c.lng.toFixed(3), +c.lat.toFixed(3), +map.getZoom().toFixed(2)]; })(),
          outsideAt: B.clear.find((c) => c.covered === false)?.center ?? null } : null,
      };
    `, 120);
    if (args.profile) {
      const { profile } = await send('Profiler.stop');
      r.profile = topFunctions(profile, 25);
      // --profile-out file: the raw CPU profile of each run (file-<run>.cpuprofile, opens in DevTools).
      if (args['profile-out']) fs.writeFileSync(String(args['profile-out']).replace(/(\.cpuprofile)?$/, `-${name}.cpuprofile`), JSON.stringify(profile));
    }
    if (traceDone) {
      await send('Tracing.end');
      const events = await traceDone;
      r.threads = threadBusy(events);
      // Written in slices: a busy trace is larger than the longest string V8 can build.
      const file = String(args.trace).replace(/(\.json)?$/, `-${name}.json`);
      fs.writeFileSync(file, '{"traceEvents":[');
      for (let i = 0; i < events.length; i += 20000) fs.appendFileSync(file, (i ? ',' : '') + events.slice(i, i + 20000).map((e) => '\n' + JSON.stringify(e)).join(','));
      fs.appendFileSync(file, ']}');
    }
    out.runs[name] = r;
    lowCamera(r.clearance, `along ${name}`);
    outside(r.clearance, `along ${name}`);
    // Let the view settle between runs.
    await sleep(500);
  }
  // --after "js returning a value": evaluated after the runs, reported as `after` (instrumentation
  // installed with --set can report here).
  if (args.after) out.after = await evaluate(String(args.after));
  out.exceptions = { count: exceptions.length, first: [...new Set(exceptions)].slice(0, 5) };
  console.log(JSON.stringify(out, null, 1));
} finally {
  await close();
}

/** Self time by function (ms), from a V8 CPU profile. */
function topFunctions(profile, n) {
  const self = new Map();
  const byId = new Map(profile.nodes.map((nd) => [nd.id, nd]));
  const dt = profile.timeDeltas;
  const counts = new Map();
  profile.samples.forEach((id, i) => counts.set(id, (counts.get(id) ?? 0) + (dt[i] ?? 0)));
  let total = 0;
  for (const [id, us] of counts) {
    const nd = byId.get(id);
    const cf = nd.callFrame;
    const file = (cf.url || '').split('/').pop().replace(/-[A-Za-z0-9_]{8}\.js$/, '.js');
    const key = `${cf.functionName || '(anon)'} ${file}:${cf.lineNumber + 1}`;
    self.set(key, (self.get(key) ?? 0) + us);
    if (cf.functionName !== '(idle)' && cf.functionName !== '(program)') total += us;
  }
  const rows = [...self.entries()].filter(([k]) => !k.startsWith('(idle)')).sort((a, b) => b[1] - a[1]).slice(0, n);
  // Inclusive time: each sample counts once for every distinct function on its stack.
  const parent = new Map();
  for (const nd of profile.nodes) for (const c of nd.children ?? []) parent.set(c, nd.id);
  const label = (nd) => {
    const cf = nd.callFrame;
    return `${cf.functionName || '(anon)'} ${(cf.url || '').split('/').pop().replace(/-[A-Za-z0-9_]{8}\.js$/, '.js')}:${cf.lineNumber + 1}`;
  };
  const incl = new Map();
  for (const [id, us] of counts) {
    const seen = new Set();
    for (let x = id; x !== undefined; x = parent.get(x)) {
      const k = label(byId.get(x));
      if (!seen.has(k)) {
        seen.add(k);
        incl.set(k, (incl.get(k) ?? 0) + us);
      }
    }
  }
  const skip = /^\((root|program|idle|garbage collector)\)/;
  const inc = [...incl.entries()].filter(([k]) => !skip.test(k)).sort((a, b) => b[1] - a[1]).slice(0, n * 2);
  return {
    totalBusyMs: Math.round(total / 1000),
    top: rows.map(([k, us]) => `${(us / 1000).toFixed(0).padStart(5)} ms  ${k}`),
    inclusive: inc.map(([k, us]) => `${(us / 1000).toFixed(0).padStart(5)} ms  ${k}`),
  };
}

/** Busy time per thread (ms) from trace events: top-level tasks. */
function threadBusy(events) {
  const names = new Map();
  for (const e of events) if (e.ph === 'M' && e.name === 'thread_name') names.set(`${e.pid}:${e.tid}`, e.args.name);
  const busy = new Map();
  for (const e of events) {
    if (e.ph !== 'X' || !(e.name === 'RunTask' || e.name === 'ThreadControllerImpl::RunTask')) continue;
    const k = `${e.pid}:${e.tid}`;
    busy.set(k, (busy.get(k) ?? 0) + (e.dur ?? 0) / 1000);
  }
  return Object.fromEntries([...busy.entries()].map(([k, v]) => [`${names.get(k) ?? k}`, Math.round(v)]).filter(([, v]) => v > 20).sort((a, b) => b[1] - a[1]).slice(0, 14));
}
