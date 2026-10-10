// Prototype harness (task #102): each view of a set drawn by the app in its comparison mode
// (web/src/bldcmp.ts) in several ways, saved as PNGs (stored-value and linear-light shrinks).
//   node shoot.mjs <views.json> <outdir> [--only id,id] [--modes app,cur,ref,chain] [--cdp 5292]
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const argv = process.argv.slice(2);
const opt = (k, d) => (argv.includes(`--${k}`) ? argv[argv.indexOf(`--${k}`) + 1] : d);
const [viewsFile, OUT] = argv;
const CDP = Number(opt('cdp', 5292));
const APP = opt('app', 'http://127.0.0.1:5291');
const FIELD = opt('field', 'http://127.0.0.1:5290/field');
const BLD = opt('bld', 'http://127.0.0.1:5290/bld');
const MODES = opt('modes', 'app,cur,ref,chain').split(',');
const W = 500, H = 360; // CSS px (the window's)
let views = JSON.parse(fs.readFileSync(viewsFile, 'utf8'));
if (opt('only')) views = views.filter((v) => opt('only').split(',').includes(v.id));
fs.mkdirSync(OUT, { recursive: true });

async function openTab() {
  const target = await (await fetch(`http://127.0.0.1:${CDP}/json/new?about:blank`, { method: 'PUT' })).json();
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r, j) => {
    ws.addEventListener('open', r);
    ws.addEventListener('error', j);
  });
  let seq = 0;
  const pending = new Map();
  const errors = [];
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    } else if (m.method === 'Runtime.exceptionThrown') errors.push(String(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text).split('\n')[0]);
    else if (m.method === 'Runtime.consoleAPICalled' && (m.params.type === 'error' || m.params.type === 'warning')) errors.push(m.params.args.map((a) => a.value ?? a.description).join(' ').slice(0, 300));
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const id = ++seq;
    pending.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async (expr, timeout = 1800) => {
    const r = await Promise.race([
      send('Runtime.evaluate', { expression: `(async () => { ${expr} })()`, awaitPromise: true, returnByValue: true }),
      new Promise((_, j) => setTimeout(() => j(new Error(`no answer in ${timeout} s`)), timeout * 1000).unref()),
    ]);
    if (r.exceptionDetails) throw new Error(`page: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
    return r.result.value;
  };
  const fetchBig = async (expr, timeout = 1800) => {
    const shape = await evaluate(`window.__o = await (async () => { ${expr} })(); const o = window.__o; return Object.fromEntries(Object.entries(o).map(([k, v]) => [k, typeof v === 'string' ? { len: v.length } : v]))`, timeout);
    const out = {};
    for (const [k, v] of Object.entries(shape)) {
      if (!(v && typeof v === 'object' && 'len' in v)) {
        out[k] = v;
        continue;
      }
      let s = '';
      for (let o = 0; o < v.len; o += 1 << 20) s += await evaluate(`return window.__o[${JSON.stringify(k)}].slice(${o}, ${o + (1 << 20)})`, 60);
      out[k] = s;
    }
    await evaluate('delete window.__o', 10);
    return out;
  };
  await send('Runtime.enable');
  await send('Page.enable');
  await send('Page.bringToFront');
  const close = async () => {
    ws.close();
    await fetch(`http://127.0.0.1:${CDP}/json/close/${target.id}`).catch(() => {});
  };
  return { send, evaluate, fetchBig, close, errors };
}

function png(w, h, data) {
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) Buffer.from(data.buffer, data.byteOffset + y * w * 3, w * 3).copy(raw, y * (w * 3 + 1) + 1);
  const chunk = (t, d) => {
    const b = Buffer.alloc(12 + d.length);
    b.writeUInt32BE(d.length, 0);
    b.write(t, 4, 'ascii');
    d.copy(b, 8);
    b.writeUInt32BE(zlib.crc32(b.subarray(4, 8 + d.length)) >>> 0, 8 + d.length);
    return b;
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw, { level: 6 })), chunk('IEND', Buffer.alloc(0))]);
}
const save = (file, r) => {
  fs.writeFileSync(path.join(OUT, `${file}.png`), png(r.w, r.h, Buffer.from(r.srgb, 'base64')));
  fs.writeFileSync(path.join(OUT, `${file}.lin.png`), png(r.w, r.h, Buffer.from(r.linear, 'base64')));
};

const summary = fs.existsSync(path.join(OUT, 'shots.json')) ? JSON.parse(fs.readFileSync(path.join(OUT, 'shots.json'), 'utf8')) : {};
for (const v of views) {
  const t0 = Date.now();
  const tab = await openTab();
  const url = `${APP}/?bldcmp&dpr=2&bldfield=${encodeURIComponent(FIELD)}&bldtiles=${encodeURIComponent(BLD)}${v.flat ? '' : ''}#map=${v.z}/${v.lat}/${v.lon}/${v.bearing ?? 0}/${v.pitch ?? 0}${v.elev ? `/${v.elev}` : ''}${v.flat ? '&gb=0' : ''}`;
  await tab.send('Page.navigate', { url });
  for (let i = 0; i < 600; i++) {
    if (await tab.evaluate('return !!window.__bld', 10).catch(() => false)) break;
    await new Promise((r) => setTimeout(r, 200));
  }
  const rec = { url, ...(summary[v.id] ?? {}) };
  try {
    await tab.evaluate('await window.__bld.ready(); return 1');
    // The view's pivot on the ground (the camera above the terrain, as the app levels it).
    rec.elev = await tab.evaluate(`const m = window.__bld.map; const e = m.queryTerrainElevation(m.getCenter()) ?? 0; m.jumpTo({ elevation: e }); m.jumpTo({ zoom: ${v.z} }); await window.__bld.ready(); return e`);
    rec.camera = await tab.evaluate('return window.__bld.camera()');
    const zf = Math.floor(v.z);
    for (const mode of MODES) {
      const t1 = Date.now();
      let o = null;
      if (mode === 'app') o = { field: true, k: 1, ss: 1, fieldMax: 13 };
      else if (mode === 'cur') o = { field: false, k: 0, ss: 1 };
      else if (mode === 'ref') {
        // Full detail: the z14 meshes, at 4 x the pixel ratio, point-sampled, shrunk.
        const k = Math.max(0, 14 - zf);
        if (v.maxRefK !== undefined && k > v.maxRefK) continue;
        const tiles = ((W * 2 ** k) / 512 + 1) * ((H * 2 ** k) / 512 + 1);
        const strips = v.strips ?? Math.max(1, Math.ceil(tiles / 120));
        o = { field: false, k, ss: 4, strips };
      } else if (mode === 'chain') {
        // The app as at 3 zooms closer (its field and meshes), shrunk: what the closer zooms show.
        if (zf >= 10) continue;
        o = { field: true, k: 4, ss: 4, fieldMax: 10 };
      }
      if (!o) continue;
      const r = await tab.fetchBig(`return await window.__bld.shot(${JSON.stringify(o)})`);
      save(`${v.id}.${mode}`, r);
      rec[mode] = { ...o, secs: (Date.now() - t1) / 1000, tiles: r.tiles };
      await tab.evaluate('await window.__bld.reset(); return 1');
      console.log(`${v.id} ${mode} ${JSON.stringify(o)} ${((Date.now() - t1) / 1000).toFixed(1)} s`);
    }
    if (MODES.includes('frames')) {
      // Frames drawn back to back (each waited for: gl.finish), the field on and off, three rounds.
      rec.frames = [];
      for (let i = 0; i < 3; i++) {
        const on = await tab.evaluate('await window.__bld.shot({ field: true, k: 1, ss: 1, fieldMax: 13 }); return await window.__bld.frames(60)');
        const off = await tab.evaluate('window.__bld.field.on = false; return await window.__bld.frames(60)');
        rec.frames.push({ on, off });
      }
      console.log(v.id, 'frames', JSON.stringify(rec.frames.map((r) => [r.on.frameMs, r.off.frameMs])), JSON.stringify(rec.frames[0].on));
    }
  } catch (e) {
    rec.error = String(e.message ?? e);
    console.log(`${v.id}: ${rec.error}`);
  }
  rec.errors = tab.errors.slice(0, 10);
  if (tab.errors.length) console.log(`${v.id} page errors: ${tab.errors.slice(0, 3).join(' | ')}`);
  summary[v.id] = rec;
  fs.writeFileSync(path.join(OUT, 'shots.json'), JSON.stringify(summary, null, 1));
  await tab.close();
  console.log(`${v.id} done in ${((Date.now() - t0) / 1000).toFixed(0)} s`);
}
