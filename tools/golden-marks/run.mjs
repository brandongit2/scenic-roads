// Golden test (docs/phase5.md "Exactness"): today's landmarks worker, run here on today's files as the
// server serves them (/api/layer/…), against the server's /api/marks/view, request by request. They
// must be exactly equal, but for the legacy `i` in properties.
//   node run.mjs [base url] [--kinds peak,viewpoint,…]
const B = process.argv[2]?.startsWith('http') ? process.argv[2] : 'http://localhost:8092';
const arg = (n) => { const i = process.argv.indexOf(n); return i > 0 ? process.argv[i + 1] : null; };
const KINDS = (arg('--kinds') ?? 'viewpoint,peak,waterfall,lighthouse,covered_bridge,rest,trailhead').split(',');

const waiting = [];
globalThis.window = globalThis;
globalThis.localStorage = { getItem: () => null, setItem() {}, removeItem() {} };
globalThis.self = { postMessage: (m) => { const w = waiting.findIndex((x) => x.match(m)); if (w >= 0) waiting.splice(w, 1)[0].resolve(m); } };
await import('./worker.mjs');
const ask = (msg, match) => new Promise((resolve) => { waiting.push({ match, resolve }); self.onmessage({ data: msg }); });

const t0 = Date.now();
for (const k of KINDS) {
  const m = await ask({ type: 'load', src: `pois-${k}`, url: `${B}/api/layer/pois-${k}` }, (r) => r.type === 'loaded' && r.src === `pois-${k}`);
  if (!m.ok) throw new Error(`load ${k} failed`);
}
await new Promise((resolve) => { self.onmessage({ data: { type: 'summits', url: `${B}/api/layer/summits` } }); setTimeout(resolve, 3000); });
console.error(`loaded ${KINDS.length} kinds and the summits in ${Date.now() - t0} ms`);

const PLACES = {
  lakes: [-3.1, 54.45], alps: [6.87, 45.92], london: [-0.12, 51.507], tokyo: [139.76, 35.68], quebec: [-72, 47],
  bc: [-123, 49.6], hongkong: [114.17, 22.3], pyrenees: [0.6, 42.6], highlands: [-5, 57], kyoto: [135.77, 35.0],
};
const FILTERS = [
  {},
  { peak: { 'peak.pr': { on: true, min: 100, max: 0 } } },
  { peak: { 'peak.ele': { on: true, min: 1000, max: 3000 }, 'peak.is': { on: true, min: 0.45, max: 0 } }, viewpoint: { 'viewpoint.pan': { on: true, min: 0, max: 0 } } },
  { rest: { 'rest.toilets': { on: true, min: 0, max: 0 } }, waterfall: { 'waterfall.h': { on: true, min: 10, max: 0 } }, lighthouse: { 'lighthouse.y': { on: true, min: 1800, max: 1900 } } },
];
const kindsFor = (fi, keepUnknown, hists) => KINDS.map((k) => ({ k, src: `pois-${k}`, layer: `poi-${k}`, filters: FILTERS[fi][k] ?? {}, keepUnknown, hists }));

function* cases() {
  for (const [name, [lon, lat]] of Object.entries(PLACES)) {
    for (const z of [3, 4, 5, 6, 8, 10, 12, 14]) {
      const w = (360 * 5) / 2 ** z, h = w * 0.6 * Math.cos((lat * Math.PI) / 180);
      const b = [lon - w / 2, lat - h / 2, lon + w / 2, lat + h / 2];
      const i = z % 4;
      yield { name: `${name} z${z}`, bounds: b, outline: [], balance: [0, 0.3, 0.5, 1][i], ranks: [[250, 10], [1000, 10], [50, 3], [250, 10]][i], fi: (z / 2) % 4 | 0, keepUnknown: z % 3 !== 0, hists: z % 2 === 0 };
      // A pitched view's outline (a trapezoid) over the same place.
      const out = [[b[0], b[1]], [b[2], b[1]], [lon + w, b[3] + h], [lon - w, b[3] + h], [b[0], b[1]]];
      yield { name: `${name} z${z} outline`, bounds: [lon - w, b[1], lon + w, b[3] + h], outline: out, balance: 0.3, ranks: [250, 10], fi: 0, keepUnknown: true, hists: true };
    }
  }
  yield { name: 'world', bounds: [-180, -85, 180, 85], outline: [], balance: 0.3, ranks: [250, 10], fi: 0, keepUnknown: true, hists: true };
  yield { name: 'world filtered', bounds: [-180, -85, 180, 85], outline: [], balance: 0.7, ranks: [1000, 10], fi: 2, keepUnknown: false, hists: true };
  yield { name: 'antimeridian', bounds: [100, 10, -170, 70], outline: [], balance: 0.3, ranks: [250, 10], fi: 1, keepUnknown: true, hists: false };
}

const strip = (p) => { const { i, ...rest } = p ?? {}; return rest; };
const norm = (r) => ({
  hist: Array.from(r.hist), n: r.n, atRanks: r.atRanks,
  byKind: r.byKind.map((b) => ({ key: b.key, n: b.n, best: b.best && { name: b.best.name, lngLat: b.best.lngLat, layer: b.best.layer, props: strip(b.best.props) } })),
  top: r.top.map((t) => ({ k: t.k, layer: t.layer, score: t.score, props: strip(t.props), lngLat: t.lngLat })),
  topByKind: Object.fromEntries(Object.entries(r.topByKind).map(([k, l]) => [k, l.map((t) => ({ k: t.k, layer: t.layer, score: t.score, props: strip(t.props), lngLat: t.lngLat }))])),
  fhist: Object.fromEntries(Object.entries(r.fhist).map(([k, v]) => [k, { bins: Array.from(v.bins), n: v.n }])),
  summit: r.summit,
});
// Deep equality with sorted keys (props' key order differs between the two).
const canon = (v) => Array.isArray(v) ? v.map(canon) : v && typeof v === 'object' ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, canon(v[k])])) : v;
function diff(a, b, path = '') {
  if (typeof a !== typeof b || Array.isArray(a) !== Array.isArray(b) || (a === null) !== (b === null)) return `${path}: ${JSON.stringify(a)?.slice(0, 200)} vs ${JSON.stringify(b)?.slice(0, 200)}`;
  if (Array.isArray(a)) {
    if (a.length !== b.length) return `${path}: length ${a.length} vs ${b.length}`;
    for (let i = 0; i < a.length; i++) { const d = diff(a[i], b[i], `${path}[${i}]`); if (d) return d; }
    return null;
  }
  if (a && typeof a === 'object') {
    const ka = Object.keys(a).sort(), kb = Object.keys(b).sort();
    if (ka.join() !== kb.join()) return `${path}: keys ${ka} vs ${kb}`;
    for (const k of ka) { const d = diff(a[k], b[k], `${path}.${k}`); if (d) return d; }
    return null;
  }
  return Object.is(a, b) ? null : `${path}: ${a} vs ${b}`;
}

let n = 0, bad = 0, id = 0, tw = 0, ts = 0;
for (const c of cases()) {
  const kinds = kindsFor(c.fi, c.keepUnknown, c.hists);
  let t = performance.now();
  const w = await ask({ type: 'query', id: ++id, outline: c.outline, bounds: c.bounds, balance: c.balance, kinds, top: 60, ranks: c.ranks }, (r) => r.type === 'result' && r.id === id);
  tw += performance.now() - t;
  t = performance.now();
  const res = await fetch(`${B}/api/marks/view`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ outline: c.outline, bounds: c.bounds, balance: c.balance, kinds, top: 60, ranks: c.ranks }) });
  if (!res.ok) throw new Error(`${c.name}: HTTP ${res.status}`);
  const s = await res.json();
  ts += performance.now() - t;
  const d = diff(canon(norm(w)), canon(norm(s)));
  n++;
  if (d) { bad++; console.log(`DIFF ${c.name}: ${d}`); }
}
// Worldwide counts (the worker's `count`), per kind and filter setting.
let nc = 0;
for (const fi of [0, 1, 2, 3]) {
  for (const keepUnknown of [true, false]) {
    for (const kq of kindsFor(fi, keepUnknown, false)) {
      const w = await ask({ type: 'count', id: ++id, kind: kq }, (r) => r.type === 'count' && r.id === id);
      const q = JSON.stringify({ filters: kq.filters, keepUnknown: kq.keepUnknown, off: [] });
      const s = await (await fetch(`${B}/api/marks/count?kind=${kq.k}&q=${encodeURIComponent(q)}`)).json();
      nc++;
      if (w.n !== s.n || w.of !== s.of) { bad++; console.log(`DIFF count ${kq.k} ${q}: ${w.n}/${w.of} vs ${s.n}/${s.of}`); }
    }
  }
}
console.log(`${nc} counts compared`);
console.log(`${n} cases, ${bad} different; worker ${(tw / n).toFixed(1)} ms, server ${(ts / n).toFixed(1)} ms a query`);
process.exit(bad ? 1 : 0);
