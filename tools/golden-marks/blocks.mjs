// Golden, part 2: every z6 block of each kind decodes to exactly today's points (positions, lean
// properties but `i`, the filters' values), ids are unique, popups by id equal today's by index,
// and the worldwide counts equal the worker's.
import zlib from 'node:zlib';
const B = process.argv[2] ?? 'http://localhost:8092';
const KINDS = ['viewpoint', 'peak', 'waterfall', 'lighthouse', 'covered_bridge', 'rest', 'trailhead', 'heritage'];
const srcOf = (k) => (k === 'heritage' ? 'heritage' : `pois-${k}`);
const FIELDS = { peak: ['ele', 'pr', 'is'], waterfall: ['h'], lighthouse: ['h', 'fh', 'rg', 'y'], viewpoint: ['ele', 'pan', 'tw'], covered_bridge: ['len', 'y'], rest: ['fac'], trailhead: ['fac'], heritage: ['by', 'dy', 'wp'] };

export function decode(buf) {
  const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  if (String.fromCharCode(...buf.subarray(0, 4)) !== 'RDMT') throw new Error('not RDMT');
  const n = dv.getUint32(8, true), nf = dv.getUint32(12, true), nc = dv.getUint32(16, true), plen = dv.getUint32(20, true);
  let at = 32;
  const take = (len) => { const o = at; at = Math.ceil((at + len) / 8) * 8; return o; };
  const ab = buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength);
  const ids = new Float64Array(ab, take(n * 8), n);
  const fvals = Array.from({ length: nf }, () => new Float64Array(ab, take(n * 8), n));
  const lon = new Int32Array(ab, take(n * 4), n), lat = new Int32Array(ab, take(n * 4), n);
  const fa = new Float32Array(ab, take(n * 4), n), ia = new Float32Array(ab, take(n * 4), n), mz = new Float32Array(ab, take(n * 4), n);
  const rank = new Uint32Array(ab, take(n * 4), n);
  const kz = new Uint8Array(ab, take(n), n), cls = new Uint8Array(ab, take(n), n), tier = new Uint8Array(ab, take(n), n), flags = new Uint8Array(ab, take(n), n);
  const code = new Uint32Array(ab, take(nc * 4), nc), count = new Uint32Array(ab, take(nc * 4), nc), ctier = new Uint8Array(ab, take(nc), nc);
  const offs = new Uint32Array(ab, take((n + 1) * 4), n + 1);
  const pb = new Uint8Array(ab, take(plen), plen);
  const td = new TextDecoder();
  const props = (i) => JSON.parse(td.decode(pb.subarray(offs[i], offs[i + 1])));
  return { n, ids, fvals, lon, lat, fa, ia, mz, rank, kz, cls, tier, flags, cells: { code, count, tier: ctier }, props };
}

const get = async (u) => { const r = await fetch(u); if (r.status === 204) return null; if (!r.ok) throw new Error(`${u}: ${r.status}`); return new Uint8Array(await r.arrayBuffer()); };
const cat = await (await fetch(`${B}/api/catalog`)).json();
const tiles = cat.marks?.tiles ? Object.fromEntries(cat.marks.tiles.map((t) => [t, 1])) : null;
const units = tiles ? Object.keys(tiles) : null;
let bad = 0;
const fail = (m) => { bad++; if (bad < 20) console.log('FAIL', m); };
const key = (lon, lat, p) => `${lon},${lat},${JSON.stringify(Object.keys(p).sort().map((k) => [k, p[k]]))}`;
let ndet = 0;
for (const k of KINDS) {
  const fc = await (await fetch(`${B}/api/layer/${srcOf(k)}`)).json();
  const legacy = new Map();
  for (const f of fc.features) {
    const { i, ...rest } = f.properties;
    const kk = key(f.geometry.coordinates[0], f.geometry.coordinates[1], rest);
    if (!legacy.has(kk)) legacy.set(kk, []);
    legacy.get(kk).push(f);
  }
  const ids = new Set();
  let n = 0;
  const list = units ?? [];
  for (const t of list) {
    const [, x, y] = t.split('/');
    const b = await get(`${B}/api/marks/block/${k}/6/${x}/${y}`);
    if (!b) continue;
    const d = decode(b);
    for (let j = 0; j < d.n; j++) {
      n++;
      if (ids.has(d.ids[j])) fail(`${k}: id ${d.ids[j]} twice`);
      ids.add(d.ids[j]);
      const p = d.props(j);
      const lon = d.lon[j] / 1e7, lat = d.lat[j] / 1e7;
      const fs = legacy.get(key(lon, lat, p));
      if (!fs?.length) { fail(`${k} ${t}: no legacy point like ${JSON.stringify(p).slice(0, 120)} at ${lon},${lat}`); continue; }
      const f = fs.shift();
      FIELDS[k].forEach((fld, fi) => {
        const want = typeof f.properties[fld] === 'number' ? f.properties[fld] : NaN;
        if (!Object.is(d.fvals[fi][j], want)) fail(`${k}: ${fld} ${d.fvals[fi][j]} vs ${want}`);
      });
      if (Math.fround(f.properties.fa ?? 0) !== d.fa[j]) fail(`${k}: fa`);
      // Popups: every 40th point, by id against today's by index.
      if (j % 40 === 0) {
        ndet++;
        // (Points alike in place and properties are twins: the popup must be one of theirs.)
        const twins = [f, ...(legacy.get(key(lon, lat, p)) ?? [])];
        const a = await fetch(`${B}/api/marks/detail/${k}/${d.ids[j]}?at=${lon},${lat}`);
        const ta = a.status === 200 ? await a.text() : '';
        let ok = false;
        for (const tw of twins) {
          const c = await fetch(`${B}/api/detail/${k === 'heritage' ? 'heritage' : 'poi'}/${tw.properties.i}`);
          const tc = c.status === 200 ? await c.text() : '';
          if (ta === tc || JSON.stringify(JSON.parse(ta || 'null')) === JSON.stringify(JSON.parse(tc || 'null'))) ok = true;
          if (ok) break;
        }
        if (!ok) fail(`${k} detail ${d.ids[j]} / i ${f.properties.i}: ${ta.slice(0, 120)}`);
      }
    }
  }
  const left = [...legacy.values()].reduce((a, v) => a + v.length, 0);
  if (left) fail(`${k}: ${left} legacy points not in any block`);
  console.log(`${k}: ${n} points in blocks, ${fc.features.length} today, ids unique ${ids.size === n}`);
}
console.log(`${ndet} popups compared; ${bad} failures`);
process.exit(bad ? 1 : 0);
