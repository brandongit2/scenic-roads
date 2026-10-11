// Timing the field in a windowed Chrome (CDP 5293), the display pacing the frames:
//   node time.mjs <views.json> <out.json> [--frames 360]
// Each view: framed as the app frames it, then the camera turned slowly for N frames (one step a
// requestAnimationFrame) with the field on and off, twice: the field pass's GPU time (its timer
// query around the whole pass: both frame copies, the march, the last pass) and the frames' intervals.
// Every page tab is closed before and after (the clean-benchmark rule). The quiet-Mac gate (quiet.mjs)
// runs before and after each view: a view measured while builds ran, the load was high or the Mac
// was hot is measured again (up to 3 times, then the run stops); `--allow-busy` records it anyway,
// marked busy, and says so. The gate's findings go with every result.
import fs from 'node:fs';
import { waitQuiet, snapshot } from './quiet.mjs';

const argv = process.argv.slice(2);
const [viewsFile, outFile] = argv;
const EXTRA = argv.includes('--extra') ? argv[argv.indexOf('--extra') + 1] : '';
const DBG = Number(argv.includes('--dbg') ? argv[argv.indexOf('--dbg') + 1] : 0);
const N = Number(argv.includes('--frames') ? argv[argv.indexOf('--frames') + 1] : 360);
const CDP = 5293, APP = 'http://127.0.0.1:5291', FIELD = 'http://127.0.0.1:5290/field', BLD = 'http://127.0.0.1:5290/bld';
const views = JSON.parse(fs.readFileSync(viewsFile, 'utf8'));
const ALLOW_BUSY = argv.includes('--allow-busy');
const gate = async () => (ALLOW_BUSY ? snapshot() : waitQuiet());

const closeAll = async () => {
  for (const t of await (await fetch(`http://127.0.0.1:${CDP}/json/list`)).json()) if (t.type === 'page') await fetch(`http://127.0.0.1:${CDP}/json/close/${t.id}`);
};
async function tab(url) {
  const t = await (await fetch(`http://127.0.0.1:${CDP}/json/new?${encodeURIComponent(url)}`, { method: 'PUT' })).json();
  const ws = new WebSocket(t.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener('open', r));
  let seq = 0;
  const pend = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pend.has(m.id)) {
      pend.get(m.id)(m);
      pend.delete(m.id);
    }
  });
  const send = (method, params = {}) => new Promise((res) => {
    const id = ++seq;
    pend.set(id, res);
    ws.send(JSON.stringify({ id, method, params }));
  });
  await send('Page.bringToFront');
  const ev = async (expr) => {
    const r = await send('Runtime.evaluate', { expression: `(async () => { ${expr} })()`, awaitPromise: true, returnByValue: true });
    if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 400));
    return r.result?.result?.value;
  };
  return { ev, close: () => ws.close() };
}

const out = {};
await closeAll();
for (const v of views) {
  for (let attempt = 1; ; attempt++) {
  const before = await gate();
  const url = `${APP}/?bldcmp${EXTRA ? '&' + EXTRA : ''}&bldfield=${encodeURIComponent(FIELD)}&bldtiles=${encodeURIComponent(BLD)}#map=${v.z}/${v.lat}/${v.lon}/${v.bearing ?? 0}/${v.pitch ?? 0}`;
  const t = await tab(url);
  for (let i = 0; i < 300; i++) {
    if (await t.ev('return !!window.__bld').catch(() => false)) break;
    await new Promise((r) => setTimeout(r, 200));
  }
  await t.ev('await window.__bld.ready(); return 1');
  const place = await t.ev(`return await window.__bld.place(${JSON.stringify({ z: v.z + 0.03, lat: v.lat, lon: v.lon, pitch: v.pitch ?? 0, bearing: v.bearing ?? 0, margin: 300 })})`);
  const run = async (on) => t.ev(`
    const b = window.__bld, m = b.map, f = b.field;
    f.on = ${on}; f.gpuMs = []; f.timing = false; f.dbg = ${DBG};
    const b0 = m.getBearing();
    // Whole frames timed on the GPU too: a timer query around MapLibre's _render.
    const g = m.painter.context.gl, ext = g.getExtension('EXT_disjoint_timer_query_webgl2');
    if (!m.__timed) {
      const orig = m._render.bind(m);
      m.__fq = [];
      m.__fms = [];
      m._render = (t) => {
        const q = ext ? g.createQuery() : null;
        if (q && !f.timing) g.beginQuery(ext.TIME_ELAPSED_EXT, q);
        const r = orig(t);
        if (q && !f.timing) { g.endQuery(ext.TIME_ELAPSED_EXT); m.__fq.push(q); } else if (q) g.deleteQuery(q);
        while (m.__fq.length && g.getQueryParameter(m.__fq[0], g.QUERY_RESULT_AVAILABLE)) {
          const q0 = m.__fq.shift();
          if (!g.getParameter(ext.GPU_DISJOINT_EXT)) m.__fms.push(g.getQueryParameter(q0, g.QUERY_RESULT) / 1e6);
          g.deleteQuery(q0);
        }
        return r;
      };
      m.__timed = true;
    }
    m.__fms = [];
    // (settle: tiles for the turn loaded)
    await b.ready();
    const dts = [];
    let last = performance.now();
    await new Promise((res) => {
      let i = 0;
      const step = (now) => {
        if (i > 0) dts.push(now - last);
        last = now;
        m.jumpTo({ bearing: b0 + i * 0.02 });
        if (++i <= ${N}) requestAnimationFrame(step); else res();
      };
      requestAnimationFrame(step);
    });
    await new Promise((r) => setTimeout(r, 300));
    m.jumpTo({ bearing: b0 });
    f.timing = false;
    const q = (a, p) => (a.length ? [...a].sort((x, y) => x - y)[Math.min(a.length - 1, Math.floor(a.length * p))] : null);
    const fr = m.__fms.slice(10);
    return { frameMed: q(fr, 0.5), frameP90: q(fr, 0.9), frameN: fr.length, dtMed: q(dts, 0.5), dtP90: q(dts, 0.9), slow: dts.filter((d) => d > 12.5).length, n: dts.length,
      tiles: f.drawn.length, canvas: [m.painter.context.gl.drawingBufferWidth, m.painter.context.gl.drawingBufferHeight] };`);
  const rec = { place, runs: [] };
  for (let k = 0; k < 2; k++) rec.runs.push({ on: await run(true), off: await run(false) });
  const after = snapshot('during');
  rec.quiet = { before, after, busy: !(before.ok && after.ok) };
  if (rec.quiet.busy && !ALLOW_BUSY) {
    t.close();
    await closeAll();
    console.log(`${v.id}: the Mac got busy during the measurement (${after.reasons.join('; ')}): measured again`);
    if (attempt >= 3) throw new Error(`${v.id}: busy in 3 attempts, stopping`);
    continue;
  }
  if (rec.quiet.busy) console.log(`${v.id}: MEASURED ON A BUSY MAC (--allow-busy): ${[...before.reasons, ...after.reasons].join('; ')}`);
  out[v.id] = rec;
  console.log(v.id, JSON.stringify(rec.runs.map((r) => ({ frameOn: r.on.frameMed, frameOff: r.off.frameMed, p90On: r.on.frameP90, p90Off: r.off.frameP90, dtOn: r.on.dtMed, dtOff: r.off.dtMed, slowOn: r.on.slow, slowOff: r.off.slow, tiles: r.on.tiles })), null, 0), JSON.stringify(rec.runs[0].on.canvas));
  console.log(`  load ${before.load[0].toFixed(2)} → ${after.load[0].toFixed(2)}, thermal ${after.therm.join(' / ')}`);
  t.close();
  await closeAll();
  break;
  }
}
fs.writeFileSync(outFile, JSON.stringify(out, null, 1));
