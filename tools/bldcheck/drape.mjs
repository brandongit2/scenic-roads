// The terrain's render-to-texture tiles (the drapes: roads, water, the fills and lines under the
// labels) at upstream's zoom against the zoom over the terrain and by depth (task #193,
// TerrainTileManager.terrainAwareZoom), in the app as it is (?checks), in a windowed Chrome (CDP 5293,
// chrome-win.sh; nothing else open in it):
//   node drape.mjs <views.json> <out.json> [--reps 4] [--frames 300] [--shots <dir>] [--allow-busy]
//     [--app http://127.0.0.1:5291] [--maxz <deepest render-to-texture zoom>]
// Each view: the app opened at it, then for each repetition both settings in a rotated order (A B,
// B A, …) in the same tab: the cover switched, the map left to settle (idle), then the camera moved
// one step a requestAnimationFrame for `--frames` frames, turning (0.02° a frame) and then gliding
// ahead (a 4000th of the view's width a frame): every frame's GPU time (a timer query around
// MapLibre's _render), the render-to-texture tiles, the textures drawn that frame (Painter.acquireRTT
// calls), the draped layers; at rest before and after, the textures held (the tiles' and the pool's)
// and their memory (RGBA8 with mipmaps). `--shots`: each setting's view at rest saved as a PNG
// (<id>-on.png, <id>-off.png), a crop of the near drapes of each (the view's `crop`, CSS px)
// and the two crops side by side (<id>-crops-off-on.png, upstream's zoom on the left). The quiet-Mac gate (quiet.mjs) before each view and after each
// repetition; a busy repetition is measured again (3 tries), unless --allow-busy, which records it
// marked busy.
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { waitQuiet, snapshot } from './quiet.mjs';

const argv = process.argv.slice(2);
const opt = (k, d) => (argv.includes(`--${k}`) ? argv[argv.indexOf(`--${k}`) + 1] : d);
const [viewsFile, outFile] = argv;
const REPS = Number(opt('reps', 4)), MAXZ = opt('maxz', ''), N = Number(opt('frames', 300));
const SHOTS = opt('shots', '');
const CDP = Number(opt('cdp', 5293)), APP = opt('app', 'http://127.0.0.1:5291');
const ALLOW_BUSY = argv.includes('--allow-busy');
let views = JSON.parse(fs.readFileSync(viewsFile, 'utf8'));
if (opt('only')) views = views.filter((v) => opt('only').split(',').includes(v.id));
if (SHOTS) fs.mkdirSync(SHOTS, { recursive: true });

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
  return { ev, send, close: () => ws.close() };
}

// In the page: the hooks (once), then helpers on window.__drape.
const SETUP = `
  const m = window.__app.map, p = m.painter, g = p.context.gl, ext = g.getExtension('EXT_disjoint_timer_query_webgl2');
  if (!window.__drape) {
    const d = window.__drape = { acquired: 0, frames: null };
    const acq = p.acquireRTT.bind(p);
    p.acquireRTT = (size) => { d.acquired++; return acq(size); };
    const orig = m._render.bind(m);
    d.q = [];
    m._render = (t) => {
      const q = ext && d.frames ? g.createQuery() : null;
      const a0 = d.acquired;
      if (q) g.beginQuery(ext.TIME_ELAPSED_EXT, q);
      const r = orig(t);
      if (q) {
        g.endQuery(ext.TIME_ELAPSED_EXT);
        const tm = m.terrain.tileManager, rtt = m.painter.renderToTexture;
        const rec = { drawn: d.acquired - a0, tiles: tm._renderableTilesKeys.length, stacks: rtt ? rtt._stacks.length : 0, ms: null };
        d.frames.push(rec);
        d.q.push([q, rec]);
      }
      while (d.q.length && g.getQueryParameter(d.q[0][0], g.QUERY_RESULT_AVAILABLE)) {
        const [q0, rec] = d.q.shift();
        if (!g.getParameter(ext.GPU_DISJOINT_EXT)) rec.ms = g.getQueryParameter(q0, g.QUERY_RESULT) / 1e6;
        g.deleteQuery(q0);
      }
      return r;
    };
    d.idle = () => new Promise((res) => {
      const done = () => { clearTimeout(to); res(true); };
      const to = setTimeout(() => { m.off('idle', done); res(false); }, 60000);
      m.once('idle', done);
      m.triggerRepaint();
    });
    d.held = () => {
      const tm = m.terrain.tileManager;
      let n = 0, bytes = 0, zs = {};
      for (const k of tm._renderableTilesKeys) { const t = tm._tiles[k]; zs[t.tileID.canonical.z] = (zs[t.tileID.canonical.z] ?? 0) + 1; }
      for (const k in tm._tiles) for (const o of tm._tiles[k].rttObjects) if (o) { n++; bytes += o.size * o.size * 4 * 4 / 3; }
      let pool = 0;
      for (const o of p._rttObjectRecyclePool) { pool++; bytes += o.size * o.size * 4 * 4 / 3; }
      return { tiles: tm._renderableTilesKeys.length, zooms: zs, textures: n, pooled: pool, mb: bytes / 2 ** 20, size: p.renderToTexture?.rttSize };
    };
  }
  return 1;`;

const measure = (t, on) => t.ev(`
  const m = window.__app.map, d = window.__drape, tm = m.terrain.tileManager;
  const c0 = m.getCenter(), b0 = m.getBearing();
  tm.terrainAwareZoom = ${on};
  await d.idle(); await d.idle();
  const rest = d.held();
  const move = async (kind) => {
    d.frames = [];
    const dts = [];
    let last = performance.now();
    await new Promise((res) => {
      let i = 0;
      const w = m.getBounds().getEast() - m.getBounds().getWest();
      const step = (now) => {
        if (i > 0) dts.push(now - last);
        last = now;
        if (kind === 'turn') m.jumpTo({ bearing: b0 + i * 0.02 });
        else {
          const r = (b0 * Math.PI) / 180, s = (i * w) / 4000;
          m.jumpTo({ center: [c0.lng + s * Math.sin(r), c0.lat + s * Math.cos(r) * Math.cos((c0.lat * Math.PI) / 180)], bearing: b0 });
        }
        if (++i <= ${N}) requestAnimationFrame(step); else res();
      };
      requestAnimationFrame(step);
    });
    await new Promise((r) => setTimeout(r, 300));
    const fr = d.frames.slice(10);
    const held = d.held();
    d.frames = null;
    const q = (a, p) => (a.length ? [...a].sort((x, y) => x - y)[Math.min(a.length - 1, Math.floor(a.length * p))] : null);
    const ms = fr.map((f) => f.ms).filter((x) => x != null);
    const mean = (a) => a.reduce((s, x) => s + x, 0) / Math.max(1, a.length);
    return { frameMed: q(ms, 0.5), frameP90: q(ms, 0.9), frameMean: mean(ms), n: ms.length,
      tilesMean: mean(fr.map((f) => f.tiles)), drawnMean: mean(fr.map((f) => f.drawn)), drawnTotal: fr.reduce((s, f) => s + f.drawn, 0),
      framesDrawing: fr.filter((f) => f.drawn > 0).length, stacks: fr[0]?.stacks,
      drawnMs: mean(fr.filter((f) => f.drawn > 0 && f.ms != null).map((f) => f.ms)), quietMs: mean(fr.filter((f) => f.drawn === 0 && f.ms != null).map((f) => f.ms)),
      dtMed: q(dts, 0.5), slow: dts.filter((x) => x > 12.5).length, heldAfter: held };
  };
  const turn = await move('turn');
  m.jumpTo({ center: c0, bearing: b0 });
  await d.idle();
  const glide = await move('glide');
  m.jumpTo({ center: c0, bearing: b0 });
  return { rest, turn, glide };`);

const out = {};
await closeAll();
for (const v of views) {
  const before = ALLOW_BUSY ? snapshot() : await waitQuiet();
  const url = `${APP}/?checks#map=${v.z}/${v.lat}/${v.lon}/${v.bearing ?? 0}/${v.pitch ?? 0}`;
  const t = await tab(url);
  for (let i = 0; i < 300; i++) {
    if (await t.ev('return !!(window.__app && window.__app.map && window.__app.map.terrain)').catch(() => false)) break;
    await new Promise((r) => setTimeout(r, 200));
  }
  await t.ev(SETUP);
  // (--maxz: the render-to-texture tiles' deepest zoom, TerrainTileManager.maxzoom, 22 by default)
  if (MAXZ) await t.ev(`window.__app.map.terrain.tileManager.maxzoom = ${Number(MAXZ)}; return 1`);
  // (the view framed as the app frames a place, camera3d.frame, as bldcmp's place(): the target at the
  // centre over its ground, the camera kept 150 m or more above the terrain under it, farther out by
  // quarter zooms while it isn't; the app's own start from the hash can leave it elsewhere)
  const placed = await t.ev(`
    const m = window.__app.map, cam3d = window.__app.cam3d, tr = m._camera.transform, v = ${JSON.stringify(v)};
    let z = v.z, clear = 0;
    for (let i = 0; i < 24; i++) {
      const elev = m.queryTerrainElevation([v.lon, v.lat]) ?? 0;
      m.jumpTo(cam3d.frame(m, { lng: v.lon, lat: v.lat }, elev, z, v.pitch ?? 0, v.bearing ?? 0));
      await window.__drape.idle();
      clear = tr.getCameraAltitude() - (m.queryTerrainElevation(tr.getCameraLngLat()) ?? 0);
      if (clear >= 150 && Math.abs((m.queryTerrainElevation([v.lon, v.lat]) ?? 0) - elev) < 1) break;
      if (clear < 150) z -= 0.25;
    }
    const c = m.getCenter();
    return [c.lng, c.lat, m.getZoom(), z, clear];`);
  console.log(`${v.id} at ${placed.map((x) => x.toFixed(4)).join(' ')}`);
  const rec = { before, placed, shots: {}, reps: [] };
  if (SHOTS) {
    for (const on of [false, true]) {
      await t.ev(`const m = window.__app.map; m.terrain.tileManager.terrainAwareZoom = ${on}; await window.__drape.idle(); await window.__drape.idle(); await new Promise((r) => setTimeout(r, 500)); return 1`);
      const held = await t.ev('return window.__drape.held()');
      const shot = await t.send('Page.captureScreenshot', { format: 'png' });
      const file = path.join(SHOTS, `${v.id}-${on ? 'on' : 'off'}.png`);
      fs.writeFileSync(file, Buffer.from(shot.result.data, 'base64'));
      // (a crop of the near drapes at the screen's own pixels: the view's `crop`, CSS px)
      const [cx, cy, cw, ch] = v.crop ?? [300, 440, 560, 280];
      const cs = await t.send('Page.captureScreenshot', { format: 'png', clip: { x: cx, y: cy, width: cw, height: ch, scale: 1 } });
      fs.writeFileSync(file.replace(/\.png$/, '-crop.png'), Buffer.from(cs.result.data, 'base64'));
      rec.shots[on ? 'on' : 'off'] = { file, held };
      console.log(`${v.id} ${on ? 'on ' : 'off'} ${file} ${JSON.stringify(held)}`);
    }
  }
  if (SHOTS) {
    // the two crops side by side, upstream's zoom on the left (ffmpeg)
    const c = (k) => path.join(SHOTS, `${v.id}-${k}-crop.png`);
    execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', c('off'), '-i', c('on'), '-filter_complex', 'hstack', path.join(SHOTS, `${v.id}-crops-off-on.png`)]);
  }
  for (let r = 0; r < REPS; r++) {
    for (let attempt = 1; ; attempt++) {
      const order = r % 2 ? [true, false] : [false, true];
      const res = {};
      for (const on of order) res[on ? 'on' : 'off'] = await measure(t, on);
      const after = snapshot('during');
      res.quiet = { after, busy: !after.ok };
      if (res.quiet.busy && !ALLOW_BUSY) {
        console.log(`${v.id} rep ${r}: busy (${after.reasons.join('; ')}), again`);
        if (attempt >= 3) throw new Error(`${v.id}: busy in 3 attempts, stopping`);
        await waitQuiet();
        continue;
      }
      rec.reps.push(res);
      const f = (x) => (x == null ? '-' : x.toFixed(2));
      for (const k of ['turn', 'glide']) console.log(`${v.id} rep ${r} ${k}: off ${f(res.off[k].frameMed)} ms (rtt ${f(res.off[k].tilesMean)} tiles, ${f(res.off[k].drawnMean)} drawn/frame) | on ${f(res.on[k].frameMed)} ms (rtt ${f(res.on[k].tilesMean)} tiles, ${f(res.on[k].drawnMean)} drawn/frame)${res.quiet.busy ? ' BUSY' : ''}`);
      break;
    }
  }
  out[v.id] = rec;
  t.close();
  await closeAll();
  fs.writeFileSync(outFile, JSON.stringify(out, null, 1));
}
fs.writeFileSync(outFile, JSON.stringify(out, null, 1));
