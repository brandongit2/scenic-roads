// The quiet-Mac re-test of the field's timings (task #102), in one session:
//   node retest.mjs <views-T.json> <out.json> [--reps 6] [--frames 300] [--phases H,B,S]
// with the app on 5291 (?bldcmp), the field and meshes on 5290, and the windowed Chrome on CDP 5293
// (chrome-win.sh; nothing else open in it). Phases:
//   H  the six views' headline numbers: the field's whole-frame cost (frames on − off, medians of
//      `--frames` frames turning slowly) and its pass timer, `--reps` times each;
//   B  the hull's columns: 8 × 8, 4 × 4, 2 × 2 cells and one a cell (blockCells 8, 4, 2, 1);
//   S  stopping early: the light left where the march stops, 0.004, 0.1, 0.3 (field.tmin);
//   X  the steps' least length: 1 and 2 texels (field.stepTexel);
//   Q  the step ordering and the hits summed in linear light, each off and both (field.dbg 26, field.linAcc);
//   P  what the march's later pieces cost: the sub-cell walk off, and with it the walls' open-share
//      rate or the roofs' correlation off (field.subWalk, field.dbg 23 / 24).
// B and S on Manhattan and Kantō at 70° and Tokyo straight down. Variants are switched in the same
// tab, interleaved, each repetition in a rotated order (A B C, B C A, C A B, …), with the frames
// without the field measured in every repetition too, so drift and the variants' order cancel; each
// difference is taken within its repetition (paired).
// The quiet-Mac gate (quiet.mjs) runs before each view (with the load average) and before and after
// each repetition (the processes and the thermal state): a repetition
// with the Mac busy at either end is measured again (3 tries), and the gate's findings are kept with
// every repetition. No result is recorded from a busy Mac.
import fs from 'node:fs';
import { waitQuiet, snapshot } from './quiet.mjs';

const argv = process.argv.slice(2);
const [viewsFile, outFile] = argv;
const opt = (k, d) => (argv.includes(`--${k}`) ? argv[argv.indexOf(`--${k}`) + 1] : d);
const REPS = Number(opt('reps', 6)), N = Number(opt('frames', 300));
const PHASES = opt('phases', 'H,B,S').split(',');
const CDP = 5293, APP = 'http://127.0.0.1:5291', FIELD = 'http://127.0.0.1:5290/field', BLD = 'http://127.0.0.1:5290/bld';
const views = JSON.parse(fs.readFileSync(viewsFile, 'utf8'));
const VAR_VIEWS = opt('var-views', 't-manhattan-p70,t-kanto-p70,t-tokyo-z11').split(',');
const BASE = { blockCells: 8, tmin: 0.004, stepTexel: 1 };

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

// In the page: whole frames timed on the GPU (a timer query around MapLibre's _render), or the field
// pass alone (its own query), over N frames turning the bearing slowly.
const RUN = (mode, n) => `
  const b = window.__bld, m = b.map, f = b.field;
  const g = m.painter.context.gl, ext = g.getExtension('EXT_disjoint_timer_query_webgl2');
  if (!m.__timed) {
    const orig = m._render.bind(m);
    m.__fq = []; m.__fms = [];
    m._render = (t) => {
      const q = ext && !f.timing ? g.createQuery() : null;
      if (q) g.beginQuery(ext.TIME_ELAPSED_EXT, q);
      const r = orig(t);
      if (q) { g.endQuery(ext.TIME_ELAPSED_EXT); m.__fq.push(q); }
      while (m.__fq.length && g.getQueryParameter(m.__fq[0], g.QUERY_RESULT_AVAILABLE)) {
        const q0 = m.__fq.shift();
        if (!g.getParameter(ext.GPU_DISJOINT_EXT)) m.__fms.push(g.getQueryParameter(q0, g.QUERY_RESULT) / 1e6);
        g.deleteQuery(q0);
      }
      return r;
    };
    m.__timed = true;
  }
  f.on = ${mode !== 'off'};
  f.timing = ${mode === 'pass'};
  f.gpuMs = []; m.__fms = [];
  await b.ready();
  const b0 = m.getBearing();
  const dts = [];
  let last = performance.now();
  await new Promise((res) => {
    let i = 0;
    const step = (now) => { if (i > 0) dts.push(now - last); last = now; m.jumpTo({ bearing: b0 + i * 0.02 }); if (++i <= ${n}) requestAnimationFrame(step); else res(); };
    requestAnimationFrame(step);
  });
  await new Promise((r) => setTimeout(r, 300));
  m.jumpTo({ bearing: b0 });
  f.timing = false;
  const a = (${mode === 'pass'} ? f.gpuMs : m.__fms).slice(10).sort((x, y) => x - y);
  const q = (p) => (a.length ? a[Math.min(a.length - 1, Math.floor(a.length * p))] : null);
  dts.sort((x, y) => x - y);
  return { med: q(0.5), p90: q(0.9), n: a.length, dtMed: dts[Math.floor(dts.length / 2)], slow: dts.filter((d) => d > 12.5).length };`;
const SET = (v) => `
  const b = window.__bld, f = b.field;
  f.blockCells = ${v.blockCells}; f.tmin = ${v.tmin}; f.stepTexel = ${v.stepTexel}; f.subWalk = ${v.subWalk ?? false}; f.dbg = ${v.dbg ?? 0}; f.linAcc = ${v.linAcc ?? true};
  b.map.triggerRepaint();
  await b.ready();
  await new Promise((r) => { let i = 0; const s = () => (++i < 30 ? requestAnimationFrame(s) : r()); requestAnimationFrame(s); });
  return 1;`;

async function openView(v) {
  const url = `${APP}/?bldcmp&bldfield=${encodeURIComponent(FIELD)}&bldtiles=${encodeURIComponent(BLD)}#map=${v.z}/${v.lat}/${v.lon}/${v.bearing ?? 0}/${v.pitch ?? 0}`;
  const t = await tab(url);
  for (let i = 0; i < 300; i++) {
    if (await t.ev('return !!window.__bld').catch(() => false)) break;
    await new Promise((r) => setTimeout(r, 200));
  }
  await t.ev('await window.__bld.ready(); return 1');
  const place = await t.ev(`return await window.__bld.place(${JSON.stringify({ z: v.z + 0.03, lat: v.lat, lon: v.lon, pitch: v.pitch ?? 0, bearing: v.bearing ?? 0, margin: 300 })})`);
  return { t, place };
}

/** One repetition of the variants in the given order: frames without the field, then each variant
 * with it (whole frames, then the pass); measured again while the Mac was busy at either end. */
async function repetition(t, variants, label) {
  for (let attempt = 1; ; attempt++) {
    const before = snapshot('during');
    const rec = { off: await t.ev(RUN('off', N)), variants: {} };
    for (const v of variants) {
      await t.ev(SET(v));
      rec.variants[v.name] = { on: await t.ev(RUN('on', N)), pass: await t.ev(RUN('pass', Math.round(N * 2 / 3))) };
    }
    const after = snapshot('during');
    rec.quiet = { before, after };
    if (before.ok && after.ok) return rec;
    console.log(`${label}: busy (${[...before.reasons, ...after.reasons].join('; ')}): measured again`);
    if (attempt >= 3) throw new Error(`${label}: the Mac busy in 3 tries, stopping`);
    await waitQuiet({ phase: 'during' });
  }
}

const med = (a) => { const s = [...a].sort((x, y) => x - y); return s.length ? (s[(s.length - 1) >> 1] + s[s.length >> 1]) / 2 : null; };
const sd = (a) => { const m = a.reduce((x, y) => x + y, 0) / a.length; return Math.sqrt(a.reduce((x, y) => x + (y - m) ** 2, 0) / Math.max(1, a.length - 1)); };
function summarize(reps, names) {
  const out = {};
  for (const nm of names) {
    const cost = reps.map((r) => r.variants[nm].on.med - r.off.med), pass = reps.map((r) => r.variants[nm].pass.med);
    out[nm] = { cost: med(cost), costSd: sd(cost), costRange: [Math.min(...cost), Math.max(...cost)], pass: med(pass), passSd: sd(pass) };
    if (nm !== names[0]) {
      const dc = reps.map((r) => r.variants[nm].on.med - r.variants[names[0]].on.med), dp = reps.map((r) => r.variants[nm].pass.med - r.variants[names[0]].pass.med);
      out[nm].vsFirst = { cost: med(dc), costSd: sd(dc), pass: med(dp), passSd: sd(dp) };
    }
  }
  return out;
}

const out = { started: new Date().toISOString(), reps: REPS, frames: N, phases: {} };
const save = () => fs.writeFileSync(outFile, JSON.stringify(out, null, 1));
await closeAll();
let warmed = false;
async function phase(key, viewIds, variants) {
  out.phases[key] = {};
  for (const id of viewIds) {
    const v = views.find((x) => x.id === id);
    const gateBefore = await waitQuiet();
    const { t, place } = await openView(v);
    if (!warmed) { await t.ev(RUN('on', 600)); warmed = true; }   // (the GPU's clocks up, discarded)
    const reps = [];
    for (let r = 0; r < REPS; r++) {
      const order = variants.map((_, i) => variants[(i + r) % variants.length]);
      reps.push(await repetition(t, order, `${key} ${id} rep ${r + 1}`));
    }
    t.close();
    await closeAll();
    const sum = summarize(reps, variants.map((x) => x.name));
    out.phases[key][id] = { place, gateBefore, reps, summary: sum };
    save();
    console.log(`${key} ${id}: ` + Object.entries(sum).map(([nm, s]) => `${nm} +${s.cost.toFixed(2)}±${s.costSd.toFixed(2)} (pass ${s.pass.toFixed(2)}±${s.passSd.toFixed(2)})${s.vsFirst ? ` Δ ${s.vsFirst.cost >= 0 ? '+' : ''}${s.vsFirst.cost.toFixed(2)}±${s.vsFirst.costSd.toFixed(2)}` : ''}`).join('  ') + `  [load ${gateBefore.load[0].toFixed(2)}]`);
  }
}
if (PHASES.includes('H')) await phase('H', views.map((v) => v.id), [{ name: 'default', ...BASE }]);
if (PHASES.includes('B')) await phase('B', VAR_VIEWS, [8, 4, 2, 1].map((c) => ({ name: `blocks${c}`, ...BASE, blockCells: c })));
if (PHASES.includes('X')) await phase('X', VAR_VIEWS, [1, 2].map((x) => ({ name: `steps${x}`, ...BASE, stepTexel: x })));
if (PHASES.includes('P')) await phase('P', VAR_VIEWS, [
  { name: 'now', ...BASE },
  { name: 'noSub', ...BASE, subWalk: false },
  { name: 'noSubNoOpen', ...BASE, subWalk: false, dbg: 23 },
  { name: 'noSubNoCov', ...BASE, subWalk: false, dbg: 24 },
]);
if (PHASES.includes('Q')) await phase('Q', VAR_VIEWS, [
  { name: 'now', ...BASE },
  { name: 'noLin', ...BASE, linAcc: false },
  { name: 'oldOrder', ...BASE, dbg: 26 },
  { name: 'neither', ...BASE, linAcc: false, dbg: 26 },
]);
if (PHASES.includes('S')) await phase('S', VAR_VIEWS, [0.004, 0.1, 0.3].map((x) => ({ name: `tmin${x}`, ...BASE, tmin: x })));
out.finished = new Date().toISOString();
save();
console.log(`done: ${out.started} → ${out.finished}`);
