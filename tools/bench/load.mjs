// Load-time benchmark: every request of a page load, through the Chrome DevTools Protocol.
//
// Against a Chrome started as in bench.mjs (port 9333), load the app and record each request (the
// page's and its workers'): when it was issued, when the browser sent it (the gap: queued behind
// the connection limit or the cache), when the headers and the last byte arrived, and its size.
// Reports per kind of data (road tiles, terrain, basemap, layers…): count, bytes, time span, how
// many were in flight at once; the load's milestones (boot overlay gone, roads in view loaded,
// the map done: its tiles and overlays; the network quiet for 2 s); and the slowest requests.
//
//   node tools/bench/load.mjs [--url URL] [--warm] [--profile] [--out file.json]
//
// Cold by default (the browser cache cleared first); --warm loads the page once, then measures a
// reload with the cache as the first load left it. --profile adds a CPU profile of the page and
// every worker during the load: busy time per thread and the functions that took it.
import fs from 'node:fs';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith('--')) acc.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith('--') ? all[i + 1] : true]);
    return acc;
  }, []),
);
const PORT = Number(args.port ?? 9333);
const URL = String(args.url ?? 'http://localhost:8080/');
const W = Number(args.w ?? 1512), H = Number(args.h ?? 900), DPR = Number(args.dpr ?? 2);

const target = await (await fetch(`http://127.0.0.1:${PORT}/json/new?about:blank`, { method: 'PUT' })).json();
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
let seq = 0;
const pending = new Map();
const listeners = [];
ws.addEventListener('message', (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) {
    pending.get(m.id)(m);
    pending.delete(m.id);
  } else if (m.method) for (const f of listeners) f(m.method, m.params, m.sessionId);
});
const send = (method, params = {}, sessionId) =>
  new Promise((res, rej) => {
    const id = ++seq;
    pending.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result)));
    ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
  });
const evaluate = async (expr) => {
  const r = await send('Runtime.evaluate', { expression: `(async () => { ${expr} })()`, awaitPromise: true, returnByValue: true });
  return r.exceptionDetails ? null : r.result.value;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Requests by id (page and worker sessions share Chrome's request ids).
const reqs = new Map();
let recording = false;
// --profile: the sessions profiled (page = undefined), and their worker URLs.
const profiled = new Map();
listeners.push(async (method, p, sessionId) => {
  if (method === 'Target.attachedToTarget') {
    // Workers: their own requests, reported on their session.
    await send('Network.enable', {}, p.sessionId).catch(() => {});
    if (args.profile && recording) {
      await send('Profiler.enable', {}, p.sessionId).catch(() => {});
      await send('Profiler.setSamplingInterval', { interval: 200 }, p.sessionId).catch(() => {});
      await send('Profiler.start', {}, p.sessionId).catch(() => {});
      profiled.set(p.sessionId, p.targetInfo.url.replace(/^.*\//, '').replace(/-[\w-]{8}\.js$/, ''));
    }
    await send('Runtime.runIfWaitingForDebugger', {}, p.sessionId).catch(() => {});
    return;
  }
  if (!recording) return;
  if (method === 'Network.requestWillBeSent') {
    reqs.set(p.requestId, { url: p.request.url, issued: p.timestamp, type: p.type, worker: !!sessionId });
  } else if (method === 'Network.responseReceived') {
    const r = reqs.get(p.requestId);
    if (!r) return;
    const t = p.response.timing;
    r.status = p.response.status;
    r.fromCache = p.response.fromDiskCache || p.response.fromMemoryCache || p.response.fromServiceWorker || p.response.fromPrefetchCache;
    if (t) {
      r.sent = t.requestTime + Math.max(0, t.sendStart) / 1000;
      r.headers = t.requestTime + t.receiveHeadersEnd / 1000;
    }
  } else if (method === 'Network.loadingFinished') {
    const r = reqs.get(p.requestId);
    if (r) (r.end = p.timestamp), (r.bytes = p.encodedDataLength);
  } else if (method === 'Network.loadingFailed') {
    const r = reqs.get(p.requestId);
    if (r) (r.end = p.timestamp), (r.failed = p.errorText || 'failed'), (r.canceled = p.canceled);
  }
});

const kind = (u) => {
  const { pathname } = new globalThis.URL(u);
  const m = pathname.match(/^\/tiles\/(roads|rails|terrain|slope)\//) || pathname.match(/^\/tiles\/(trees)\//);
  if (m) return `tiles/${m[1]}`;
  if (pathname.startsWith('/tiles/base')) return 'basemap';
  if (pathname.startsWith('/fonts/')) return 'fonts';
  const l = pathname.match(/^\/api\/layer\/([\w-]+)/);
  if (l) return `layer/${l[1]}`;
  const a = pathname.match(/^\/api\/(\w+)/);
  if (a) return `api/${a[1]}`;
  return 'app';
};

try {
  await send('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true });
  await send('Runtime.enable');
  await send('Page.enable');
  await send('Network.enable', { maxTotalBufferSize: 200e6 });
  await send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: DPR, mobile: false });
  if (args.warm) {
    await send('Page.navigate', { url: URL });
    for (let i = 0; i < 600 && !(await evaluate('return !!(window.__app && window.__app.map && window.__app.map.loaded() && window.__app.map.areTilesLoaded())')); i++) await sleep(250);
    await sleep(8000);
    await send('Page.navigate', { url: 'about:blank' });
    await sleep(500);
  } else {
    await send('Network.clearBrowserCache');
  }
  recording = true;
  if (args.profile) {
    await send('Profiler.enable');
    await send('Profiler.setSamplingInterval', { interval: 200 });
    await send('Profiler.start');
    profiled.set(undefined, 'page');
  }
  const t0 = Date.now();
  await send('Page.navigate', { url: URL });
  // Milestones, from the page (ms since navigation).
  const marks = {};
  let quietSince = 0;
  for (let i = 0; i < 2400; i++) {
    await sleep(100);
    const el = Date.now() - t0;
    const s = await evaluate(`const a = window.__app; if (!a) return null;
      const p = a.roads.progress();
      return { boot: document.getElementById('boot')?.classList.contains('done') ?? false, loaded: a.map.loaded(), tiles: a.map.areTilesLoaded(), roads: p.loaded >= p.wanted && p.wanted > 0 };`).catch(() => null);
    if (s) {
      if (s.boot && marks.boot === undefined) marks.boot = el;
      if (s.roads && marks.roadsInView === undefined) marks.roadsInView = el;
      if (s.tiles && s.loaded && s.roads && marks.mapTiles === undefined) marks.mapTiles = el;
    }
    const inflight = [...reqs.values()].filter((r) => r.end === undefined).length;
    if (inflight === 0 && reqs.size > 0) {
      quietSince ||= el;
      // Done: the network quiet for 2 s and the map finished (its tiles, the overlays tiled).
      if (el - quietSince > 2000 && (marks.mapTiles !== undefined || el > 60000)) {
        marks.quiet = quietSince;
        break;
      }
    } else quietSince = 0;
  }
  recording = false;
  // CPU per thread: busy time, and the functions with the most self time.
  const cpu = [];
  for (const [sid, name] of profiled) {
    const r = await Promise.race([send('Profiler.stop', {}, sid).catch(() => null), sleep(5000).then(() => null)]);
    if (!r) continue;
    const nodes = new Map(r.profile.nodes.map((n) => [n.id, n]));
    const self = new Map();
    const dt = r.profile.timeDeltas;
    r.profile.samples.forEach((id, i) => {
      const n = nodes.get(id);
      const f = n.callFrame.functionName || '(anonymous)';
      if (['(idle)', '(program)', '(garbage collector)'].includes(f) && f !== '(garbage collector)') return;
      const key = `${f} ${(n.callFrame.url || '').replace(/^.*\//, '').replace(/-[\w-]{8}\.js$/, '')}:${n.callFrame.lineNumber}`;
      self.set(key, (self.get(key) ?? 0) + (dt[i] ?? 0) / 1000);
    });
    const busy = [...self.values()].reduce((a, b) => a + b, 0);
    cpu.push({ thread: name, busyMs: Math.round(busy), top: [...self.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8).map(([k, v]) => `${Math.round(v)}ms ${k}`) });
  }

  // ---- summary --------------------------------------------------------------------------------
  const all = [...reqs.values()].filter((r) => r.end !== undefined && !r.url.startsWith('data:') && !r.url.startsWith('blob:'));
  const base = Math.min(...all.map((r) => r.issued));
  const ms = (t) => Math.round((t - base) * 1000);
  const byKind = {};
  for (const r of all) {
    const k = kind(r.url);
    const b = (byKind[k] ??= { n: 0, kb: 0, first: Infinity, last: 0, queuedMs: 0, serverMs: 0, downloadMs: 0, cached: 0, failed: 0, maxInflight: 0 });
    b.n++;
    b.kb += (r.bytes ?? 0) / 1024;
    b.first = Math.min(b.first, ms(r.issued));
    b.last = Math.max(b.last, ms(r.end));
    if (r.fromCache) b.cached++;
    if (r.failed && !r.canceled) b.failed++;
    if (r.sent) {
      b.queuedMs += Math.max(0, (r.sent - r.issued) * 1000);
      b.serverMs += Math.max(0, (r.headers - r.sent) * 1000);
      b.downloadMs += Math.max(0, (r.end - r.headers) * 1000);
    }
  }
  // In flight at once (sent, not finished), overall and per kind: sampled every 10 ms.
  const end = Math.max(...all.map((r) => ms(r.end)));
  let maxAll = 0;
  const timeline = [];
  for (let t = 0; t <= end; t += 10) {
    let n = 0;
    const per = {};
    for (const r of all) {
      const a = ms(r.sent ?? r.issued), e = ms(r.end);
      if (a <= t && t < e) {
        n++;
        const k = kind(r.url);
        per[k] = (per[k] ?? 0) + 1;
      }
    }
    maxAll = Math.max(maxAll, n);
    for (const [k, v] of Object.entries(per)) byKind[k].maxInflight = Math.max(byKind[k].maxInflight, v);
    if (t % 250 === 0) timeline.push([t, n, Object.entries(per).sort((a, b) => b[1] - a[1]).slice(0, 4).map(([k, v]) => `${k}:${v}`).join(' ')]);
  }
  for (const b of Object.values(byKind)) {
    b.kb = Math.round(b.kb);
    b.avgQueuedMs = Math.round(b.queuedMs / b.n);
    b.avgServerMs = Math.round(b.serverMs / b.n);
    b.avgDownloadMs = Math.round(b.downloadMs / b.n);
    delete b.queuedMs, delete b.serverMs, delete b.downloadMs;
  }
  const slow = all
    .map((r) => ({ kind: kind(r.url), url: r.url.replace(/^https?:\/\/[^/]+/, '').slice(0, 90), start: ms(r.issued), queued: r.sent ? Math.round((r.sent - r.issued) * 1000) : null,
      server: r.sent ? Math.round((r.headers - r.sent) * 1000) : null, download: r.headers ? Math.round((r.end - r.headers) * 1000) : null, total: Math.round((r.end - r.issued) * 1000), kb: Math.round((r.bytes ?? 0) / 1024) }))
    .sort((a, b) => b.total - a.total)
    .slice(0, 20);
  const out = { url: URL, warm: !!args.warm, marks, cpu: cpu.sort((a, b) => b.busyMs - a.busyMs), requests: all.length, maxInflight: maxAll, mb: Math.round(all.reduce((a, r) => a + (r.bytes ?? 0), 0) / 1e5) / 10, byKind, slow, timeline };
  if (args.out) fs.writeFileSync(String(args.out), JSON.stringify(out, null, 1));
  console.log(JSON.stringify(out, null, 1));
} finally {
  ws.close();
  await fetch(`http://127.0.0.1:${PORT}/json/close/${target.id}`).catch(() => {});
}
