// The in-page self-benchmark: `#bench` in the address, in any browser (tools/bench/inpage.mjs drives
// Safari and Chrome with it).
//
//   …/#bench&views=tokyo,alps&runs=static,pan,orbit&secs=5&label=safari&sid=<run id>[&i=0][&cold=5301]
//
// One view a page load (web/src/data/benchviews.json, by name; all of them without `views=`): the map
// opens on it with the app's defaults (as a `#map=` link would), and once it has settled (idle, the
// road tiles in view loaded) flies a fixed camera path for each run, `secs` seconds each, the same
// path in every browser (positions from the time since the run began, not from the frames):
// - static: the view redrawn every frame (render cost alone);
// - pan: the centre carried 600 CSS px a second north-east, back again from halfway;
// - orbit: the bearing turned 60° a second and back, the pitch lowered by up to 6° (twice; never
//   raised: the camera stays over what it clears at the start).
// Recorded per view: the load (navigation to settled: frame intervals, the longest frames, tile
// loads and their timings by kind), and per run the frame intervals (requestAnimationFrame), the
// map's own render call on the main thread (CPU, every frame and frames that moved the camera), its
// GPU time where the browser has EXT_disjoint_timer_query_webgl2 (not Safari), long tasks where the
// browser reports them (not Safari: there the longest frames stand for them), tiles loaded meanwhile,
// and which layers were drawing. The view must lie in the map's coverage and the camera stay 200 m
// over the ground under it and over the highest ground in view, at the start and along each run
// (else the result is marked `invalid`, as tools/bench/bench.mjs refuses one).
// The result is posted to the map server (`POST /api/bench?name=…`, saved under <home>/bench/), then
// the page goes on to the next view: with `cold=<port>` on the next port (view i on port + i), a new
// origin whose HTTP cache is empty, so each view loads cold.
import type { Map as MLMap } from 'maplibre-gl';
import VIEWS from './data/benchviews.json';

const p = new URLSearchParams(location.hash.startsWith('#bench') ? location.hash.slice(6).replace(/^&/, '') : '');
const names = (p.get('views') ?? VIEWS.map((v) => v[0]).join(',')).split(',').filter((n) => VIEWS.some((v) => v[0] === n));
const index = Number(p.get('i') ?? 0);

/** The benchmark's settings, or null outside it. */
export const BENCH = location.hash.startsWith('#bench') && names[index]
  ? {
    names,
    index,
    view: VIEWS.find((v) => v[0] === names[index])!,
    runs: (p.get('runs') ?? 'static,pan,orbit').split(','),
    secs: Number(p.get('secs') ?? 5),
    label: (p.get('label') ?? 'browser').replace(/[^\w-]/g, ''),
    sid: (p.get('sid') ?? String(Date.now())).replace(/[^\w-]/g, ''),
    cold: Number(p.get('cold') ?? 0),
    /** `prof=1`: where the main thread's time goes: each WebGL call (count, ms) and each layer's draw. */
    prof: p.get('prof') === '1',
  }
  : null;

/**
 * `prof=1`: the main thread's other tasks timed too, from the page's start: worker and message-port
 * messages (by the worker's script), timers and animation-frame callbacks other than the map's
 * render (by function name). Installed as this module loads, before the map makes its workers.
 */
const tasks: Record<string, [number, number]> = {};
if (BENCH?.prof) {
  const add = (k: string, ms: number) => {
    const x = (tasks[k] ??= [0, 0]);
    x[0]++;
    x[1] += ms;
  };
  const timed = (k: string, f: (...a: any[]) => unknown) => function (this: unknown, ...a: unknown[]) {
    const t = performance.now();
    try {
      return f.apply(this, a);
    } finally {
      add(k, performance.now() - t);
    }
  };
  const label = (target: unknown, f: { name?: string }) =>
    target instanceof Worker ? `worker:${(target as Worker & { __src?: string }).__src ?? '?'}` : `${(target as object)?.constructor?.name ?? '?'}:${f.name || 'anon'}`;
  const W = window.Worker;
  window.Worker = class extends W {
    __src: string;
    constructor(u: string | URL, o?: WorkerOptions) {
      super(u, o);
      this.__src = String(u).split('/').pop()!.split('?')[0];
    }
  } as typeof Worker;
  const ael = EventTarget.prototype.addEventListener;
  EventTarget.prototype.addEventListener = function (this: EventTarget, type: string, f: any, o?: any) {
    if (type === 'message' && typeof f === 'function') {
      const w = timed(label(this, f), f);
      (f as { __w?: unknown }).__w = w;
      return ael.call(this, type, w, o);
    }
    return ael.call(this, type, f, o);
  };
  const rel = EventTarget.prototype.removeEventListener;
  EventTarget.prototype.removeEventListener = function (this: EventTarget, type: string, f: any, o?: any) {
    return rel.call(this, type, (f as { __w?: EventListener })?.__w ?? f, o);
  };
  for (const C of [W, MessagePort]) {
    const d = Object.getOwnPropertyDescriptor(C.prototype, 'onmessage')!;
    Object.defineProperty(C.prototype, 'onmessage', {
      configurable: true,
      get() { return d.get!.call(this); },
      set(f) { d.set!.call(this, typeof f === 'function' ? timed(label(this, f), f) : f); },
    });
  }
  const st = window.setTimeout.bind(window);
  window.setTimeout = ((f: unknown, ms?: number, ...a: unknown[]) => st(typeof f === 'function' ? timed(`timeout:${f.name || 'anon'}`, f as () => void) : (f as string), ms, ...a)) as typeof setTimeout;
  const raf = window.requestAnimationFrame.bind(window);
  window.requestAnimationFrame = (f: FrameRequestCallback) => raf(timed(`raf:${f.name || 'anon'}`, f) as FrameRequestCallback);
}
/** The tasks timed since the last call, the longest first (prof=1). */
const takeTasks = () => {
  const r = Object.entries(tasks).sort((a, b) => b[1][1] - a[1][1]).slice(0, 20).map(([k, [c, ms]]) => [k, c, Math.round(ms)] as const);
  for (const k in tasks) delete tasks[k];
  return r;
};

/** The hash the map opens on: the benchmark view's, as a `#map=` link. */
export const benchHash = (): string => `#map=${BENCH!.view[1]}`;

const q = (a: number[], f: number) => (a.length ? +a[Math.min(a.length - 1, Math.floor(f * a.length))].toFixed(1) : null);
const stats = (v: number[]) => {
  const s = [...v].sort((a, b) => a - b);
  return { p50: q(s, 0.5), p95: q(s, 0.95), max: q(s, 1), n: s.length };
};
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const CLEAR_M = 200;

type App = { map: MLMap; roads: { progress(): { loaded: number; wanted: number }; render: (...a: unknown[]) => unknown }; rails: { render: (...a: unknown[]) => unknown } };

/** Runs the benchmark on the open map (main.ts, once the app is up). */
export async function runBench(app: App): Promise<void> {
  const B = BENCH!;
  const map = app.map as any;
  const status = document.createElement('div');
  status.style.cssText = 'position:fixed;left:8px;bottom:8px;z-index:2000;font:12px ui-monospace,monospace;color:#cfd6e0;background:#0d1117cc;padding:4px 8px;border-radius:4px;pointer-events:none';
  document.body.append(status);
  const say = (t: string) => (status.textContent = `bench ${B.label} ${B.index + 1}/${B.names.length} ${B.view[0]}: ${t}`);
  say('loading');

  // ---- instruments: frames, the render call (CPU, GPU), long tasks ----
  const frames: number[] = [];
  const tick = (t: number) => {
    frames.push(t);
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  const longTasks: [number, number][] = [];
  try {
    new PerformanceObserver((l) => l.getEntries().forEach((e) => longTasks.push([e.startTime, e.duration]))).observe({ type: 'longtask', buffered: true });
  } catch {
    // (Safari has no long-task entries: the longest frames stand for them.)
  }
  const longTaskApi = PerformanceObserver.supportedEntryTypes?.includes('longtask') ?? false;
  const renders: [number, number, boolean][] = [];
  const gpu: [number, boolean, number][] = []; // ms, moved, when the frame began
  const gl = map.painter.context.gl as WebGL2RenderingContext;
  const tq = gl.getExtension('EXT_disjoint_timer_query_webgl2');
  const prof = B.prof ? profile(gl, map) : null;
  const queries: [WebGLQuery, boolean, number][] = [];
  let lastCam = '', qBusy = false;
  const orig = map._render.bind(map);
  map._render = function (t: number) {
    const a = performance.now();
    const c = map.getCenter(), cam = `${c.lng},${c.lat},${map.getZoom()},${map.getBearing()},${map.getPitch()}`;
    const moved = cam !== lastCam;
    lastCam = cam;
    let qq: WebGLQuery | null = null;
    if (tq && !qBusy) {
      qq = gl.createQuery();
      gl.beginQuery(tq.TIME_ELAPSED_EXT, qq);
      qBusy = true;
    }
    const r = orig(t);
    if (qq) {
      gl.endQuery(tq.TIME_ELAPSED_EXT);
      qBusy = false;
      queries.push([qq, moved, a]);
    }
    renders.push([a, performance.now() - a, moved]);
    for (let i = queries.length - 1; i >= 0; i--) {
      const [x, mv, at] = queries[i];
      if (gl.getQueryParameter(x, gl.QUERY_RESULT_AVAILABLE)) {
        if (!gl.getParameter(tq!.GPU_DISJOINT_EXT)) gpu.push([gl.getQueryParameter(x, gl.QUERY_RESULT) / 1e6, mv, at]);
        gl.deleteQuery(x);
        queries.splice(i, 1);
      }
    }
    return r;
  };
  const custom: Record<string, number[]> = { roads: [], rails: [] };
  for (const [k, layer] of [['roads', app.roads], ['rails', app.rails]] as const) {
    const f = layer.render.bind(layer);
    layer.render = (...a: unknown[]) => {
      const t = performance.now();
      const r = f(...a);
      custom[k].push(performance.now() - t);
      return r;
    };
  }
  const idle = () => new Promise<void>((r) => {
    map.once('idle', r);
    map.triggerRepaint();
    setTimeout(r, 30000);
  });
  /** Frame intervals between two times, and the longest. */
  const frameStats = (t0: number, t1: number) => {
    const fr = frames.filter((t) => t >= t0 && t <= t1);
    const dt = fr.slice(1).map((t, i) => t - fr[i]);
    const s = stats(dt);
    return {
      fps: +(fr.length / ((t1 - t0) / 1000)).toFixed(1), frameMs: { ...s, p90: q([...dt].sort((a, b) => a - b), 0.9), p99: q([...dt].sort((a, b) => a - b), 0.99) },
      over8ms: dt.filter((d) => d > 8.4).length, over17ms: dt.filter((d) => d > 16.8).length, over50ms: dt.filter((d) => d > 50).length,
      longestFrames: [...dt].sort((a, b) => b - a).slice(0, 6).map(Math.round),
    };
  };
  const ltStats = (t0: number, t1: number) => {
    if (!longTaskApi) return null;
    const d = longTasks.filter(([s]) => s >= t0 && s <= t1).map(([, x]) => x);
    return { n: d.length, ms: Math.round(d.reduce((a, b) => a + b, 0)), top: d.sort((a, b) => b - a).slice(0, 6).map(Math.round) };
  };
  /** Tiles and data loaded between two times, by kind: count, MB, request duration p50/p95. */
  const loads = (t0: number, t1: number) => {
    const by: Record<string, { d: number[]; b: number; wait: number[] }> = {};
    for (const e of performance.getEntriesByType('resource') as PerformanceResourceTiming[]) {
      if (e.startTime < t0 || e.startTime > t1) continue;
      const u = new URL(e.name);
      const k = u.pathname.startsWith('/tiles/') ? `tiles/${u.pathname.split('/')[2]}` : u.pathname.startsWith('/api/') ? u.pathname.split('/').slice(1, 3).join('/') : u.pathname.startsWith('/assets/') || u.pathname.startsWith('/src/') || u.pathname.startsWith('/node_modules/') ? 'app' : 'other';
      const x = (by[k] ??= { d: [], b: 0, wait: [] });
      x.d.push(e.duration);
      x.b += e.transferSize || e.encodedBodySize || 0;
      if (e.responseStart > 0) x.wait.push(e.responseStart - e.startTime);
    }
    return Object.fromEntries(Object.entries(by).sort((a, b) => b[1].d.length - a[1].d.length).map(([k, v]) => {
      const s = stats(v.d);
      return [k, { n: v.d.length, mb: +(v.b / 1e6).toFixed(2), ms: { p50: s.p50, p95: s.p95, max: s.max }, firstByteP50: stats(v.wait).p50 }];
    }));
  };

  // ---- coverage and clearance (as bench.mjs) ----
  const rings: { r: number[][]; box: number[] }[] = [];
  try {
    const fc = await (await fetch('/api/coverage')).json();
    for (const f of fc.features) for (const poly of f.geometry.type === 'Polygon' ? [f.geometry.coordinates] : f.geometry.coordinates) for (const r of poly) {
      let w = 180, e = -180, so = 90, n = -90;
      for (const [x, y] of r) {
        w = Math.min(w, x); e = Math.max(e, x); so = Math.min(so, y); n = Math.max(n, y);
      }
      rings.push({ r, box: [w, so, e, n] });
    }
  } catch {
    // (no coverage: reported as unknown)
  }
  const covered = (lng: number, lat: number) => {
    if (!rings.length) return null;
    let inside = false;
    for (const { r, box } of rings) {
      if (lng < box[0] || lng > box[2] || lat < box[1] || lat > box[3]) continue;
      for (let i = 0, j = r.length - 1; i < r.length; j = i++) {
        const [xi, yi] = r[i], [xj, yj] = r[j];
        if (yi > lat !== yj > lat && lng < ((xj - xi) * (lat - yi)) / (yj - yi) + xi) inside = !inside;
      }
    }
    return inside;
  };
  const clearance = () => {
    const tr = map._camera?.transform ?? map.transform;
    const m = map.getCenter();
    if (!map.getTerrain()) return { covered: covered(m.lng, m.lat) };
    const cam = tr.getCameraLngLat(), alt = tr.getCameraAltitude();
    const qe = (lng: number, lat: number) => map.queryTerrainElevation([lng, lat]) ?? 0;
    const mLat = 111320, mLng = 111320 * Math.cos((cam.lat * Math.PI) / 180);
    const dx = (m.lng - cam.lng) * mLng, dy = (m.lat - cam.lat) * mLat;
    const dist = Math.hypot(dx, dy), az = Math.atan2(dx, dy);
    const hfov = 2 * Math.atan((Math.tan(((tr.fov ?? 36.87) * Math.PI) / 360) * innerWidth) / innerHeight);
    let top = -Infinity;
    for (const f of [-0.45, -0.22, 0, 0.22, 0.45]) {
      const a = az + f * hfov, tx = cam.lng + (Math.sin(a) * dist) / mLng, ty = cam.lat + (Math.cos(a) * dist) / mLat;
      for (let i = 1; i <= 48; i++) top = Math.max(top, qe(cam.lng + (tx - cam.lng) * (i / 48), cam.lat + (ty - cam.lat) * (i / 48)));
    }
    return { under: Math.round(alt - qe(cam.lng, cam.lat)), over: Math.round(alt - top), covered: covered(m.lng, m.lat) };
  };
  const bad = (c: { under?: number; over?: number; covered: boolean | null }) =>
    c.covered === false ? 'outside coverage' : c.under !== undefined && (c.under < CLEAR_M || c.over! < CLEAR_M) ? `camera too low (${c.under} m over the ground, ${c.over} m over the highest in view)` : null;

  // ---- load ----
  await new Promise<void>((res) => {
    const check = () => {
      const pr = app.roads.progress();
      if (map.loaded() && pr.loaded >= pr.wanted) res();
      else setTimeout(check, 200);
    };
    check();
  });
  await idle();
  const tSettled = performance.now();
  const loadProf = prof?.take();
  const nav = performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming | undefined;
  const start = clearance();
  const out: Record<string, unknown> = {
    label: B.label, sid: B.sid, view: B.view[0], map: B.view[1], ua: navigator.userAgent, at: new Date().toISOString(),
    viewport: `${innerWidth}x${innerHeight}@${devicePixelRatio}`, canvas: `${map.getCanvas().width}x${map.getCanvas().height}`,
    origin: location.origin, cold: !!B.cold, gpuTimer: !!tq, longTaskApi,
    load: {
      navToSettledMs: Math.round(tSettled), domContentLoadedMs: Math.round(nav?.domContentLoadedEventEnd ?? 0),
      ...frameStats(nav?.domContentLoadedEventEnd ?? 0, tSettled), longTasks: ltStats(0, tSettled),
      renderCpuMs: stats(renders.filter(([s]) => s <= tSettled).map(([, d]) => d)), loads: loads(0, tSettled),
      clearance: start, prof: loadProf,
    },
    drawing: drawing(map),
    runs: {} as Record<string, unknown>,
  };
  let invalid = bad(start);

  // ---- runs ----
  const c0 = map.getCenter(), z0 = map.getZoom(), b0 = map.getBearing(), p0 = map.getPitch();
  const tri = (t: number, T: number) => (t < T / 2 ? t : T - t); // out and back
  const S = B.secs;
  const paths: Record<string, ((t: number) => void) | null> = {
    static: null,
    pan: (t) => {
      // 600 CSS px a second at the view's zoom (512 px tiles), north-east (9:5, as bench.mjs's pan).
      const px = 600 * tri(t, S), k = 360 / (512 * 2 ** z0);
      const mx = (c0.lng + 180) / 360 + (px * 0.874 * k) / 360;
      const y0 = 0.5 - Math.log(Math.tan(Math.PI / 4 + (c0.lat * Math.PI) / 360)) / (2 * Math.PI);
      const my = y0 - (px * 0.486 * k) / 360;
      map.jumpTo({ center: [mx * 360 - 180, (360 / Math.PI) * Math.atan(Math.exp((0.5 - my) * 2 * Math.PI)) - 90] });
    },
    orbit: (t) => map.jumpTo({ bearing: b0 + 60 * tri(t, S), pitch: Math.max(0, p0 - 6 * Math.abs(Math.sin((2 * Math.PI * t) / S))) }),
  };
  for (const name of B.runs) {
    const kind = name.replace(/\d+$/, '');
    if (!(kind in paths)) continue;
    map.jumpTo({ center: c0, zoom: z0, bearing: b0, pitch: p0 });
    await idle();
    await sleep(300);
    say(name);
    const clear: ReturnType<typeof clearance>[] = [];
    prof?.take();
    const t0 = performance.now();
    const before = { custom: Object.fromEntries(Object.entries(custom).map(([k, v]) => [k, v.length])) };
    await new Promise<void>((res) => {
      let n = 0;
      const f = () => {
        const t = (performance.now() - t0) / 1000;
        if (t > S) return res();
        const path = paths[kind];
        if (path) path(t);
        else map.triggerRepaint();
        if (++n % 15 === 0) clear.push(clearance());
        requestAnimationFrame(f);
      };
      f();
    });
    const t1 = performance.now();
    const runProf = prof?.take();
    await idle();
    const settleMs = Math.round(performance.now() - t1);
    const rd = renders.filter(([s]) => s >= t0 && s <= t1);
    const g = gpu.filter(([, , s]) => s >= t0 && s <= t1);
    const worst = clear.reduce<{ under: number; over: number; covered: boolean }>((w, c) => ({
      under: Math.min(w.under, c.under ?? Infinity), over: Math.min(w.over, c.over ?? Infinity), covered: w.covered && c.covered !== false,
    }), { under: Infinity, over: Infinity, covered: true });
    out.runs = {
      ...(out.runs as object),
      [name]: {
        ...frameStats(t0, t1), longTasks: ltStats(t0, t1),
        renderCpuMs: stats(rd.map(([, d]) => d)), movedCpuMs: stats(rd.filter(([, , m]) => m).map(([, d]) => d)),
        gpuMs: tq ? stats(g.map(([d]) => d)) : null, movedGpuMs: tq ? stats(g.filter(([, m]) => m).map(([d]) => d)) : null,
        customMs: Object.fromEntries(Object.entries(custom).map(([k, v]) => [k, stats(v.slice(before.custom[k]))])),
        loads: loads(t0, t1), settleMs, prof: runProf, clearance: clear.length ? worst : null,
      },
    };
    if (!invalid && clear.length && bad(worst)) invalid = `${bad(worst)} (along ${name})`;
  }
  out.invalid = invalid;
  out.glcheck = (window as { __glcheck?: unknown }).__glcheck ?? null; // (glstate.ts's ?glcheck=1)

  // ---- report, then the next view ----
  say('posting');
  let posted = false;
  try {
    posted = (await fetch(`/api/bench?name=${encodeURIComponent(`${B.sid}-${B.label}-${B.view[0]}`)}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(out) })).ok;
  } catch {
    // (shown below)
  }
  (window as any).__benchResult = out;
  const next = B.index + 1;
  if (next < B.names.length) {
    const h = new URLSearchParams(p);
    h.set('i', String(next));
    h.set('sid', B.sid);
    const origin = B.cold ? `${location.protocol}//${location.hostname}:${B.cold + next}` : location.origin;
    // (a search of its own, so the page loads again rather than only change its hash; the page's
    // other switches kept)
    const sq = new URLSearchParams(location.search);
    sq.set('bench', String(next));
    location.replace(`${origin}/?${sq}#bench&${h.toString()}`);
  } else {
    say(posted ? 'done, results posted' : 'done, POST /api/bench failed: results in window.__benchResult');
    // (nothing drawn after the last view: an open tab left behind costs nothing)
    setTimeout(() => map.remove(), 500);
  }
}

/** The style's layers drawing at this zoom (visible, in their zoom range), and the app's own. */
function drawing(map: any): string[] {
  const z = map.getZoom();
  const ids = (map.getStyle().layers as { id: string; minzoom?: number; maxzoom?: number; layout?: { visibility?: string } }[])
    .filter((l) => (l.layout?.visibility ?? 'visible') !== 'none' && (l.minzoom ?? 0) <= z && z < (l.maxzoom ?? 24))
    .map((l) => l.id);
  return [...ids, ...(map.getTerrain() ? [`terrain×${map.getTerrain().exaggeration}`] : [])];
}

/**
 * Where the main thread's time goes (`prof=1`): every WebGL call on the map's context (count and
 * summed ms, by method) and each style layer's draw (Painter#renderLayer, by layer), and the map's
 * source updates. `take()` returns what was gathered since the last call, the largest first. (Safari
 * rounds performance.now() to 1 ms, so its per-call figures are sums of rounded spans: the counts are
 * exact, the times good over many calls.)
 */
function profile(gl: WebGL2RenderingContext, map: any) {
  let calls: Record<string, [number, number]> = {}, layers: Record<string, [number, number]> = {}, callers: Record<string, [number, number]> = {};
  const SLOW = new Set(['texSubImage2D', 'texImage2D', 'getProgramParameter', 'getShaderParameter', 'getParameter', 'readPixels', 'bufferData']);
  const caller = () => (new Error().stack ?? '').split('\n').slice(3, 6).map((l) => l.replace(/^\s*(at )?/, '').replace(/\?[^:)]*/, '').replace(/https?:\/\/[^/]+\//, '').slice(0, 70)).join(' < ');
  const add = (o: Record<string, [number, number]>, k: string, ms: number) => {
    const x = (o[k] ??= [0, 0]);
    x[0]++;
    x[1] += ms;
  };
  const g = gl as unknown as Record<string, unknown>;
  for (let o = Object.getPrototypeOf(gl); o && o !== Object.prototype; o = Object.getPrototypeOf(o)) {
    for (const k of Object.getOwnPropertyNames(o)) {
      const d = Object.getOwnPropertyDescriptor(o, k);
      if (!d || typeof d.value !== 'function' || k === 'constructor' || Object.prototype.hasOwnProperty.call(g, k)) continue;
      const f = d.value as (...a: unknown[]) => unknown;
      g[k] = function (...a: unknown[]) {
        const t = performance.now();
        const r = f.apply(gl, a);
        const ms = performance.now() - t;
        add(calls, k, ms);
        // (the slow kinds by caller too: who uploads, who waits on a link)
        if (SLOW.has(k)) add(callers, `${k} <- ${caller()}`, ms);
        return r;
      };
    }
  }
  const painter = map.painter, rl = painter.renderLayer.bind(painter);
  painter.renderLayer = (p: unknown, tm: unknown, layer: { id: string; type: string }, ...rest: unknown[]) => {
    const t = performance.now();
    const r = rl(p, tm, layer, ...rest);
    add(layers, `${layer.type}:${layer.id}`, performance.now() - t);
    return r;
  };
  const st = map.style, us = st._updateSources.bind(st);
  st._updateSources = (...a: unknown[]) => {
    const t = performance.now();
    const r = us(...a);
    add(layers, '(style._updateSources)', performance.now() - t);
    return r;
  };
  const top = (o: Record<string, [number, number]>, n: number) =>
    Object.entries(o).sort((a, b) => b[1][1] - a[1][1]).slice(0, n).map(([k, [c, ms]]) => [k, c, Math.round(ms * 10) / 10] as const);
  return {
    take() {
      const r = { tasks: takeTasks(), gl: top(calls, 25), glCalls: Object.values(calls).reduce((a, [c]) => a + c, 0), glMs: Math.round(Object.values(calls).reduce((a, [, m]) => a + m, 0)), layers: top(layers, 25), callers: top(callers, 15) };
      calls = {};
      callers = {};
      layers = {};
      return r;
    },
  };
}
