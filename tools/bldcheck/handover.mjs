// The hand-over at zoom 14 (task #102): each view framed as shoot.mjs frames it, then the map's zoom
// set exactly (a view's `mapZoom`: just below 14 and just above, the same centre, bearing and
// pitch), drawn by the app in its comparison mode (web/src/bldcmp.ts): `app` as the app draws it (the
// field below zoom 14, the z14 meshes from 14), `fieldonly` the field everywhere (the meshes
// hidden), `ref` full detail; saved as PNGs (stored-value and linear-light shrinks).
//   node handover.mjs <views.json> <outdir> [--only id,id] [--modes app,fieldonly] [--cdp 5293]
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { waitQuiet, snapshot } from './quiet.mjs';

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
  const url = `${APP}/?bldcmp&dpr=2${v.bcol ? `&bcol=${v.bcol}` : ""}&bldfield=${encodeURIComponent(FIELD)}&bldtiles=${encodeURIComponent(BLD)}${v.flat ? '' : ''}#map=${v.z}/${v.lat}/${v.lon}/${v.bearing ?? 0}/${v.pitch ?? 0}${v.flat ? '&gb=0' : ''}`;
  await tab.send('Page.navigate', { url });
  for (let i = 0; i < 600; i++) {
    if (await tab.evaluate('return !!window.__bld', 10).catch(() => false)) break;
    await new Promise((r) => setTimeout(r, 200));
  }
  const rec = { url, ...(summary[v.id] ?? {}) };
  try {
    await tab.evaluate('await window.__bld.ready(); return 1');
    // Framed as the app frames a place, the camera kept 300 m above the terrain under it.
    rec.place = await tab.evaluate(`return await window.__bld.place(${JSON.stringify({ z: v.z + 0.03, lat: v.lat, lon: v.lon, pitch: v.pitch ?? 0, bearing: v.bearing ?? 0, margin: 300 })})`);
    if (v.light !== undefined) await tab.evaluate(`window.__app.map.setLight({ anchor: 'map', position: [1.5, ${v.light}, 40], intensity: 0.35, color: '#ffffff' }); await window.__bld.ready(); return 1`);
    if (v.mapZoom !== undefined) await tab.evaluate(`window.__bld.map.jumpTo({ zoom: ${v.mapZoom} }); await window.__bld.ready(); return 1`);
    rec.camera = await tab.evaluate('return window.__bld.camera()');
    const z = rec.place.z, zf = Math.floor(rec.place.mapZoom + 1e-6);
    // The comparison's box: the buildings within it alone, in every mode (a tilted view's full
    // detail to the horizon is more tiles than a GPU holds).
    let box = v.box ?? null;
    if (!box && v.boxKm) {
      const dlat = v.boxKm / 111.32, dlon = dlat / Math.cos((v.lat * Math.PI) / 180);
      box = [v.lon - dlon, v.lat - dlat, v.lon + dlon, v.lat + dlat];
      // Snapped out to whole z14 tiles: the meshes' source takes whole tiles, the field clips there too.
      // (to whole z12 tiles: the coarsest meshes' tiles, which MapLibre draws whole)
      const n = 2 ** 12;
      const X = (lon) => ((lon + 180) / 360) * n, Y = (lat) => ((1 - Math.log(Math.tan((lat * Math.PI) / 180) + 1 / Math.cos((lat * Math.PI) / 180)) / Math.PI) / 2) * n;
      const lonOf = (x) => (x / n) * 360 - 180, latOf = (y) => (Math.atan(Math.sinh(Math.PI * (1 - (2 * y) / n))) * 180) / Math.PI;
      const e = 1e-7;
      box = [lonOf(Math.floor(X(box[0]))) + e, latOf(Math.floor(Y(box[1])) + 1) + e, lonOf(Math.floor(X(box[2])) + 1) - e, latOf(Math.floor(Y(box[3]))) - e];
    }
    const common = { ...(box ? { box } : {}), ...(v.opacity !== undefined ? { opacity: v.opacity } : {}) };
    const tilesIn = (k) => {
      const n = 2 ** 14;
      if (box) {
        const X = (lon) => ((lon + 180) / 360) * n, Y = (lat) => ((1 - Math.log(Math.tan((lat * Math.PI) / 180) + 1 / Math.cos((lat * Math.PI) / 180)) / Math.PI) / 2) * n;
        return (Math.floor(X(box[2])) - Math.floor(X(box[0])) + 1) * (Math.floor(Y(box[1])) - Math.floor(Y(box[3])) + 1);
      }
      return ((W * 2 ** k) / 512 + 1) * ((H * 2 ** k) / 512 + 1);
    };
    for (const mode of MODES) {
      const t1 = Date.now();
      let o = null;
      if (mode === 'app') o = { field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14, ...common };
      else if (mode === 'step0' || mode === 'sub' || mode === 'nearest') {
        // (diagnosis: the march's steps a pixel's footprint, the sub-cell walk, the textures unfiltered)
        await tab.evaluate(`const f = window.__bld.field; f.stepTexel = ${mode === 'step0' ? 0 : 2}; f.subWalk = ${mode === 'sub'}; f.nearest = ${mode === 'nearest'}; return 1`);
        o = { field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14, ...common };
      }
      else if (mode.startsWith('dbg')) {
        // (diagnosis: a debug view of the field, `fdbg` the number after "dbg")
        await tab.evaluate(`window.__bld.field.dbg = ${Number(mode.slice(3))}; return 1`);
        o = { field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14, ...common };
      }
      else if (mode === 'flatroof') {
        // (before the centroids' offsets: each roof over the ground under its own texel, `fdbg` 32)
        await tab.evaluate('window.__bld.field.dbg = 32; return 1');
        o = { field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14, ...common };
      }
      else if (mode === 'fieldonly') o = { field: true, buildings: false, k: 0, fbias: 1, ss: 1, fieldMax: 14, ...common };
      else if (mode === 'appneg') o = { field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14, wallSign: -1, ...common };
      else if (mode === 'flat') o = { field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14, linear: false, ...common };
      else if (mode === 'cur') o = { field: false, k: 0, ss: 1, ...common };
      else if (mode === 'ref') {
        // Full detail: the z14 meshes, at 4 x the pixel ratio, point-sampled, shrunk.
        const k = Math.max(0, 14 - zf);
        if (k > (v.maxRefK ?? 7)) continue;
        const strips = v.strips ?? Math.max(1, Math.ceil(tilesIn(k) / 120));
        o = { field: false, k, ss: 4, strips, ...common };
      } else if (mode === 'chain') {
        // The app as at 3 zooms closer (its field and meshes), shrunk: what the closer zooms show.
        if (zf >= 11) continue;
        o = { field: true, k: 3, fbias: 4, ss: 4, fieldMax: 11, ...common };
      }
      if (!o) continue;
      const r = await tab.fetchBig(`return await window.__bld.shot(${JSON.stringify(o)})`);
      save(`${v.id}.${mode}`, r);
      rec[mode] = { ...o, secs: (Date.now() - t1) / 1000, tiles: r.tiles };
      await tab.evaluate('await window.__bld.reset(); return 1');
      console.log(`${v.id} ${mode} ${JSON.stringify({ ...o, box: undefined })} ${((Date.now() - t1) / 1000).toFixed(1)} s`);
    }
    if (MODES.includes('cost')) {
      // The meshes by their tiles' distance (the layers with no least zoom) against from map zoom 14
      // alone (the layers' least zoom 14), and the roofs at the ground under each texel (`fdbg` 32), interleaved `reps` times, each behind the quiet-Mac gate
      // (quiet.mjs): the z14 mesh tiles loaded, the source's buffers, and whole frames (`__bld.frames`).
      rec.cost = [];
      for (let r = 0; r < (v.reps ?? 3); r++) {
        // (`--rules`: also readonly, the offsets read but no ground taken
        // from them nor runs started higher, `fdbg` 36; startonly, the runs started higher but no
        // offsets read, `fdbg` 37)
        const rules = opt('rules', 'distance,zoom14,flatroof').split(',');
        const n = rules.length;
        for (const rule of rules.slice(r % n).concat(rules.slice(0, r % n))) {
          // (the quiet-Mac gate before each, its findings kept with it; and the processes after)
          const gate = await waitQuiet({ log: () => {} });
          const c = await tab.evaluate(`const b = window.__bld; const m = b.map; for (const id of ['buildings', 'buildings-flat', 'buildings-pick']) if (m.getLayer(id)) m.setLayerZoomRange(id, ${rule === 'zoom14' ? 14 : 0}, 24); b.field.on = true; b.field.dbg = ${({ flatroof: 32, readonly: 36, startonly: 37 })[rule] ?? 0}; await b.ready(); const tm = m.style.tileManagers.bld; const ts = tm?._inViewTiles?.getAllTiles?.() ?? []; let bytes = 0; for (const t of ts) for (const bk of Object.values(t.buckets ?? {})) for (const k of ['layoutVertexBuffer', 'indexBuffer']) bytes += bk[k]?.byteLength ?? (bk[k]?.length ?? 0) * 0; const f = await b.frames(${v.frames ?? 120}); return { rule: '${rule}', zoom: m.getZoom(), meshTiles: ts.filter((t) => t.state === 'loaded').length, fieldTiles: new Set(b.field.drawn).size, frame: f }`, 600);
          rec.cost.push({ ...c, gate, after: snapshot('post') });
        }
      }
      console.log(`${v.id} cost ${JSON.stringify(rec.cost.map((c) => [c.rule, c.meshTiles, c.fieldTiles, c.frame?.frameMs]))}`);
    }
    if (MODES.includes('sweep') && v.sweep) {
      // A slow zoom through the hand-over (v.sweep: from, to, step), the app as it draws, a frame a
      // step (the stored values' PNG: what the screen shows).
      const { from, to, step } = v.sweep;
      rec.sweep = [];
      for (let i = 0, z = from; z <= to + 1e-9; i++, z += step) {
        await tab.evaluate(`window.__bld.map.jumpTo({ zoom: ${z} }); await window.__bld.ready(); return 1`);
        const r = await tab.fetchBig(`return await window.__bld.shot(${JSON.stringify({ field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14, ...common })})`);
        fs.writeFileSync(path.join(OUT, `${v.id}.sweep${String(i).padStart(3, '0')}.png`), png(r.w, r.h, Buffer.from(r.srgb, 'base64')));
        rec.sweep.push(await tab.evaluate('return window.__bld.camera()'));
        await tab.evaluate('await window.__bld.reset(); return 1');
      }
      console.log(`${v.id} sweep ${rec.sweep.length} frames`);
    }
    if (MODES.includes('seq')) {
      // A slow pan (v.seq.dx CSS px a frame) or a zoom sweep (v.seq.dz a frame), frame by frame: the
      // prototype and full detail at each, for flicker (tools/bldcheck: seq.py).
      const sq = v.seq ?? { n: 12, dx: 0.125, dz: 0 };
      const k = Math.max(0, 14 - zf);
      for (let i = 0; i < sq.n; i++) {
        if (i > 0) await tab.evaluate(`const m = window.__bld.map; ${sq.dx ? `m.panBy([${sq.dx}, 0], { duration: 0 });` : ''} ${sq.dz ? `m.jumpTo({ zoom: m.getZoom() + ${sq.dz} });` : ''} await window.__bld.ready(); return 1`);
        const ra = await tab.fetchBig(`return await window.__bld.shot(${JSON.stringify({ field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14, ...common })})`);
        save(`${v.id}.seq${String(i).padStart(2, '0')}.app`, ra);
        await tab.evaluate('await window.__bld.reset(); return 1');
        if (k <= (v.maxRefK ?? 7)) {
          const strips = v.strips ?? Math.max(1, Math.ceil(tilesIn(k) / 120));
          const rr = await tab.fetchBig(`return await window.__bld.shot(${JSON.stringify({ field: false, k, ss: 4, strips, ...common })})`);
          save(`${v.id}.seq${String(i).padStart(2, '0')}.ref`, rr);
          await tab.evaluate('await window.__bld.reset(); return 1');
        }
      }
      console.log(`${v.id} seq ${sq.n} frames`);
    }
    if (MODES.includes('frames')) {
      rec.frames = [];
      for (let i = 0; i < 3; i++) {
        const on = await tab.evaluate(`await window.__bld.shot(${JSON.stringify({ field: true, k: 0, fbias: 1, ss: 1, fieldMax: 14 })}); return await window.__bld.frames(60)`);
        const off = await tab.evaluate('window.__bld.field.on = false; return await window.__bld.frames(60)');
        rec.frames.push({ on, off });
      }
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
