// node --test tools/check/autofit.test.mjs: auto-fitting colour ranges by screen widths or by
// percentiles of line length (web/src/autofit.ts, which Node loads with its types stripped), and
// each metric's unit and numbers through a link and saved settings, old ones too (web/src/state.ts, loaded
// through Vite for its imports).
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import {
  allPct, bestOfPct, fitByPct, fitByWidths, pairOf, pairsField, pairsOfField, screenWidthKm, setBestPct, spread, unitOf, unitsField, unitsOfField, validFit, validFitLen,
  withPair, withUnit,
} from '../../web/src/autofit.ts';
import { createServer } from '../../web/node_modules/vite/dist/node/index.js';

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} ≉ ${b}`);

/** A length distribution over values 0..100 in unit bins, `len(v)` km in the bin starting at v,
 * with roads/stats.ts Dist's quantile (linear within bins). */
function dist(len) {
  const bins = Array.from({ length: 100 }, (_, i) => len(i));
  const total = bins.reduce((a, b) => a + b, 0);
  return {
    total,
    quantile(p) {
      const target = p * total;
      let acc = 0;
      for (let i = 0; i < 100; i++) {
        if (acc + bins[i] >= target && bins[i] > 0) return i + (target - acc) / bins[i];
        acc += bins[i];
      }
      return 100;
    },
  };
}

test('percentiles are by length: a long bland road outweighs many short good ones', () => {
  // 90 km at 0–10, 10 km at 90–100: the 50th percentile of length is bland.
  const d = dist((v) => (v < 10 ? 9 : v >= 90 ? 1 : 0));
  near(fitByPct(d, [50, 95], 1)[0], 50 / 9);
  near(fitByPct(d, [50, 95], 1)[1], 95);
});

test('percentiles: the low end at the low percentile, full colour from the high one', () => {
  const d = dist(() => 1); // uniform: the p-th percentile is p
  assert.deepEqual(fitByPct(d, [80, 99.9], 1), [80, 99.9]);
  assert.deepEqual(fitByPct(d, [0, 100], 1), [0, 100]);
});

test('percentiles are a share: twice the length in view, the same range', () => {
  const a = dist(() => 1), b = dist(() => 2);
  assert.deepEqual(fitByPct(a, [70, 99], 1), fitByPct(b, [70, 99], 1));
});

test('screen widths are an amount: twice the length in view, a higher low end', () => {
  const a = dist(() => 1), b = dist(() => 2); // 100 km and 200 km
  const lo = (d) => fitByWidths(d, d.total, 1, [15, 1], 1)[0];
  near(lo(a), 85); // the best 15 km of 100
  near(lo(b), 92.5); // the best 15 km of 200
});

test('a range narrower than four steps widens about its middle', () => {
  const d = dist((v) => (v === 50 ? 1 : 0));
  const [lo, hi] = fitByPct(d, [10, 90], 1);
  near(hi - lo, 4);
  near((lo + hi) / 2, 50.5);
  assert.deepEqual(spread(1, 9, 4), [1, 9]);
});

test('a screen width at the equator, zoom 0, 512 px: the whole earth', () => {
  near(screenWidthKm(0, 0, 512), 40075.016686, 1e-6);
  near(screenWidthKm(60, 1, 512), 40075.016686 / 4, 1e-6);
});

test('"the best n %": percentiles shown from the top', () => {
  assert.equal(bestOfPct(80), 20);
  assert.equal(bestOfPct(99.9), 0.1);
  assert.deepEqual(setBestPct([80, 99.9], 0, 15), [85, 99.9]);
  assert.deepEqual(setBestPct([80, 99.9], 1, 1), [80, 99]);
});

test('"the best n %": limits, and the edited one pushes the other along', () => {
  assert.deepEqual(setBestPct([80, 99], 0, 0), [99.5, 100]); // low end at least the best 0.5 %
  assert.deepEqual(setBestPct([80, 99], 0, 500), [0, 99]);
  assert.deepEqual(setBestPct([80, 99], 1, 30), [69.5, 70]); // top above the low end: pushes it
  assert.deepEqual(setBestPct([80, 99], 0, 0.8), [99.2, 99.7]);
  assert.deepEqual(setBestPct([80, 99], 1, -3), [80, 100]);
  assert.deepEqual(setBestPct([80, 99], 1, 99.9), [0, 0.5]);
  assert.deepEqual(setBestPct([80, 99], 0, Number.NaN), [99.5, 100]);
});

// The link round trip, through Vite's module loader (state.ts imports JSON and extensionless paths).
const vite = await createServer({
  root: fileURLToPath(new URL('../../web', import.meta.url)), configFile: false, logLevel: 'error', appType: 'custom',
  server: { middlewareMode: true, hmr: false, ws: false }, optimizeDeps: { noDiscovery: true },
});
after(() => vite.close());
const st = await vite.ssrLoadModule('/src/state.ts');

const ROAD = st.ROAD_FIT_KEYS, RAIL = st.RAIL_FIT_KEYS, FERRY = st.FERRY_FIT_KEYS;
/** The three layers' units, each as a sorted list of its metrics in percentiles. */
const pcts = (s) => [s.fitUnits, s.rail.fitUnits, s.ferry.fitUnits].map((u) => Object.keys(u).filter((k) => u[k] === 'pct').sort());

test('the metrics with a unit: the scenic modes, rail\'s and ferries\' ranked metrics', () => {
  assert.ok(ROAD.includes('score') && ROAD.includes('view') && !ROAD.includes('elev') && !ROAD.includes('map'));
  assert.ok(RAIL.includes('rscore') && RAIL.includes('freq') && !RAIL.includes('elev') && !RAIL.includes('ridge'));
  assert.deepEqual(FERRY, ['freq']);
});

test('each metric its own unit: setting one leaves the others', () => {
  let u = {};
  u = withUnit(u, 'score', 'pct');
  assert.equal(unitOf(u, 'score'), 'pct');
  assert.equal(unitOf(u, 'view'), 'widths');
  u = withUnit(u, 'view', 'pct');
  u = withUnit(u, 'score', 'widths');
  assert.deepEqual(u, { view: 'pct' }); // screen widths aren't kept: absent is screen widths
  assert.equal(unitOf(u, 'score'), 'widths');
});

test('a layer\'s units in a link: nothing, p for all, else the keys in percentiles', () => {
  const keys = ['a', 'b', 'c'];
  assert.equal(unitsField({}, keys), '');
  assert.equal(unitsField({ a: 'pct', b: 'pct', c: 'pct' }, keys), 'p');
  assert.equal(unitsField({ c: 'pct' }, keys), 'c');
  assert.equal(unitsField({ c: 'pct', a: 'pct' }, keys), '-b'); // shorter than a.c
  assert.equal(unitsField({ z: 'pct' }, keys), ''); // not one of the layer's metrics
  for (const u of [{}, { b: 'pct' }, { a: 'pct', c: 'pct' }, allPct(keys)]) assert.deepEqual(unitsOfField(unitsField(u, keys), keys), u);
  assert.deepEqual(unitsOfField('p', keys), allPct(keys));
  assert.deepEqual(unitsOfField('b.zz', keys), { b: 'pct' });
  // All but a few: '-' and those in screen widths, when shorter.
  const many = ['score', 'view', 'water', 'vista', 'drama'];
  const most = withUnit(allPct(many), 'drama', 'widths');
  assert.equal(unitsField(most, many), '-drama');
  assert.deepEqual(unitsOfField('-drama', many), most);
  assert.deepEqual(unitsOfField('-drama.zz', many), most);
  assert.equal(unitsField({ score: 'pct', view: 'pct' }, many), 'score.view');
  for (let m = 0; m < 1 << many.length; m++) {
    const u = Object.fromEntries(many.filter((_, i) => m & (1 << i)).map((k) => [k, 'pct']));
    assert.deepEqual(unitsOfField(unitsField(u, many), many), u);
  }
  assert.deepEqual(unitsOfField('', keys), {});
  assert.deepEqual(unitsOfField(null, keys), {});
  assert.deepEqual(unitsOfField(undefined, keys), {});
});

test('screen widths are the default, and a default link says nothing of the unit', () => {
  const s = structuredClone(st.defaults);
  assert.deepEqual(pcts(s), [[], [], []]);
  const h = st.toHash(s, true);
  assert.ok(!/(^|[#&])(fu|rs|fy)=/.test(h), h);
});

test('one metric in percentiles, another in screen widths, through a link (roads, rail, ferries)', () => {
  const s = structuredClone(st.defaults);
  s.fitUnits = { view: 'pct' }; // the active mode (score) stays in screen widths
  s.fit = [85, 99];
  s.rail.fitUnits = { freq: 'pct', curvy: 'pct' }; // the ride score stays in screen widths
  s.rail.fit = [60, 99.5];
  s.ferry.fitUnits = { freq: 'pct' };
  const h = st.toHash(s, true);
  assert.match(h, /(^|[#&])fu=view(&|$)/);
  assert.match(h, /rs=[^&]*,15,1,freq\.curvy(&|$)/);
  assert.match(h, /fy=[^&]*,15,1,p(&|$)/); // ferries' only ranked metric: all of them, p
  const back = st.fromHash(h);
  assert.deepEqual(pcts(back), [['view'], ['curvy', 'freq'], ['freq']]);
  assert.deepEqual(back.fit, [85, 99]);
  assert.deepEqual(back.rail.fit, [60, 99.5]);
  // Every scenic metric in percentiles: a link's p, as before the unit per metric.
  const all = structuredClone(st.defaults);
  all.fitUnits = allPct(ROAD);
  all.rail.fitUnits = allPct(RAIL);
  const ha = st.toHash(all, true);
  assert.match(ha, /(^|[#&])fu=p(&|$)/);
  assert.match(ha, /rs=[^&]*,15,1,p(&|$)/);
  assert.deepEqual(pcts(st.fromHash(ha)), [[...ROAD].sort(), [...RAIL].sort(), []]);
  // All but one: the one named.
  all.fitUnits = { ...allPct(ROAD) };
  delete all.fitUnits.drama;
  const hb = st.toHash(all, true);
  assert.match(hb, /(^|[#&])fu=-drama(&|$)/);
  assert.deepEqual(pcts(st.fromHash(hb))[0], ROAD.filter((k) => k !== 'drama').sort());
  // Each layer on its own.
  for (const k of [0, 1, 2]) {
    const t = structuredClone(st.defaults);
    const x = [t, t.rail, t.ferry][k];
    x.fitUnits = { [[ROAD, RAIL, FERRY][k][0]]: 'pct' };
    const b = st.fromHash(st.toHash(t, true));
    assert.deepEqual(pcts(b).map((l) => l.length), [0, 1, 2].map((i) => (i === k ? 1 : 0)));
  }
});

test('per-layer links (fu=p, a last p in rs and fy) open with every metric of that layer in percentiles', () => {
  // A link the app with the unit per layer wrote (711d7ff's toHash): views, rail by frequency,
  // every layer in percentiles.
  const h = '#m=view&fp=85,99&fu=p&un=11101&rl=0.5,'
    + '&rs=1,11111,metric,freq,rocket,1,0,100,,,,e8ecf2,0.6,0,0,0,1,70,99.8,0,0.6,0,a,50,1,15,1,p'
    + '&fy=1,1111,freq,oslo,,1,8fc8ff,0.9,0,0,0,1,freq,1,-0.845,2,0,100,0,0.45,0.5,0,a,0.577,15,1,p&sqx=waterfall&oc=1';
  const b = st.fromHash(h);
  assert.equal(b.mode, 'view');
  assert.deepEqual(pcts(b), [[...ROAD].sort(), [...RAIL].sort(), ['freq']]);
  assert.deepEqual(b.fit, [85, 99]);
  assert.equal(b.rail.metric, 'freq');
  // Written again, the same link: p.
  const again = st.toHash(b, true);
  assert.equal(again, h);
  assert.match(again, /(^|[#&])fu=p(&|$)/);
  assert.match(again, /rs=[^&]*,15,1,p(&|$)/);
  assert.match(again, /fy=[^&]*,15,1,p(&|$)/);
});

/** Every one of `keys` at `v`. */
const allOf = (keys, v) => Object.fromEntries(keys.map((k) => [k, [...v]]));

test('older links, without the unit, read as screen widths, their one screen-widths fit every metric\'s', () => {
  const s = structuredClone(st.defaults);
  s.rail.fitLens = allOf(RAIL, [20, 2]);
  s.ferry.fitLens = { freq: [12, 1] };
  s.fitLens = allOf(ROAD, [10, 1]);
  const h = st.toHash(s, true);
  // One fit for every metric, all in screen widths: the link as before either was per metric.
  assert.match(h, /(^|[#&])fl=10,1(&|$)/);
  assert.ok(!/flm=|fpm=/.test(h), h);
  assert.match(h, /rs=[^&]*,20,2(&|$)/);
  assert.match(h, /fy=[^&]*,12,1(&|$)/);
  const b = st.fromHash(h);
  assert.deepEqual(pcts(b), [[], [], []]);
  assert.deepEqual(b.rail.fitLens, allOf(RAIL, [20, 2]));
  assert.deepEqual(b.ferry.fitLens, { freq: [12, 1] });
  assert.deepEqual(b.fitLens, allOf(ROAD, [10, 1]));
  // Shorter, older rail and ferry fields too.
  const o = st.fromHash(h.replace(/(rs=[^&]*),20,2/, '$1').replace(/(fy=[^&]*),12,1/, '$1'));
  assert.deepEqual(pcts(o), [[], [], []]);
  assert.deepEqual([o.rail.fitLens, o.ferry.fitLens], [{}, {}]);
});

test('per-metric pairs in a link: the most common as the base, the others listed', () => {
  const keys = ['a', 'b', 'c', 'd'], d = [15, 1];
  const at = (r) => (k) => r[k] ?? d;
  assert.deepEqual(pairsField(at({}), keys, d), { base: [15, 1], rest: '' });
  assert.deepEqual(pairsField(at({ b: [10, 1] }), keys, d), { base: [15, 1], rest: 'b_10_1' });
  assert.deepEqual(pairsField(at({ a: [10, 1], b: [10, 1], c: [10, 1] }), keys, d), { base: [10, 1], rest: 'd_15_1' });
  assert.deepEqual(pairsField(at({ a: [10, 1], b: [10, 1] }), keys, d).base, [15, 1]); // a tie with the default: the default
  assert.deepEqual(pairsField(at({ a: [10, 1], b: [10, 1], c: [8, 0.5], d: [8, 0.5] }), keys, d), { base: [10, 1], rest: 'c_8_0.5/d_8_0.5' });
  assert.deepEqual(pairsField(at({ a: [10, 1] }), keys, d, [10, 1]), { base: [10, 1], rest: 'b_15_1/c_15_1/d_15_1' }); // a base given
  assert.deepEqual(pairsOfField('b_10_1/c_8_0.5', keys, validFitLen), { b: [10, 1], c: [8, 0.5] });
  // Unknown metrics, bad and malformed pairs dropped.
  assert.deepEqual(pairsOfField('zz_10_1/b_1_10/c_8/d__1/a_x_1', keys, validFitLen), {});
  assert.deepEqual(pairsOfField('a_80_99.9/b_99_80', keys, validFit), { a: [80, 99.9] });
  assert.deepEqual(pairsOfField(null, keys, validFit), {});
  // Any mix round-trips (base and the listed ones).
  const vals = [[15, 1], [10, 1], [20, 2.5]];
  for (let m = 0; m < 3 ** keys.length; m++) {
    const r = Object.fromEntries(keys.map((k, i) => [k, vals[Math.floor(m / 3 ** i) % 3]]));
    const f = pairsField(at(r), keys, d);
    const own = pairsOfField(f.rest, keys, validFitLen);
    assert.deepEqual(Object.fromEntries(keys.map((k) => [k, own[k] ?? f.base])), r);
  }
  assert.deepEqual(withPair({ a: [10, 1] }, 'a', [15, 1], d), {});
  assert.deepEqual(withPair({}, 'b', [8, 1], d), { b: [8, 1] });
  assert.deepEqual(pairOf({ b: [8, 1] }, 'a', d), [15, 1]);
});

test('two scenic metrics with their own numbers and units through a link, rail and ferries too', () => {
  const s = structuredClone(st.defaults); // the scenic score shown
  s.fitLens = { score: [10, 1], drama: [20, 2] };
  s.fitUnits = { view: 'pct' };
  s.fit = [75, 99]; // the score's percentiles
  s.scenicFits = { view: [70, 99.5] };
  s.rail.fitLens = { rscore: [20, 2], freq: [8, 1] };
  s.rail.fitUnits = { freq: 'pct' };
  s.ferry.fitLens = { freq: [12, 1] };
  const h = st.toHash(s, true);
  assert.ok(!/(^|[#&])fl=/.test(h), h); // most scenic metrics at the default: no base
  assert.match(h, /(^|[#&])flm=score_10_1\/drama_20_2(&|$)/);
  assert.ok(!/(^|[#&])fp=/.test(h), h);
  assert.match(h, /(^|[#&])fpm=score_75_99\/view_70_99\.5(&|$)/);
  assert.match(h, /rs=[^&]*,15,1,freq,rscore_20_2\/freq_8_1(&|$)/);
  assert.match(h, /fy=[^&]*,12,1(&|$)/);
  const b = st.fromHash(h);
  assert.deepEqual(b.fitLens, s.fitLens);
  assert.deepEqual(b.fit, [75, 99]);
  assert.deepEqual(b.scenicFits, { view: [70, 99.5] });
  assert.deepEqual(pcts(b), [['view'], ['freq'], []]);
  assert.deepEqual(b.rail.fitLens, s.rail.fitLens);
  assert.deepEqual(b.ferry.fitLens, { freq: [12, 1] });
  assert.equal(st.toHash(b, true), h);
  // Units without other numbers keep the rail field's shape; numbers without units leave it empty.
  const t = structuredClone(st.defaults);
  t.rail.fitLens = { viaduct: [6, 1] };
  assert.match(st.toHash(t, true), /rs=[^&]*,15,1,,viaduct_6_1(&|$)/);
  assert.deepEqual(st.fromHash(st.toHash(t, true)).rail.fitLens, { viaduct: [6, 1] });
  // Another display type shown (elevation): fp is its own, the scenic metrics' all in fpm.
  const e = new st.Store(structuredClone(s));
  e.setMode('elev');
  const he = st.toHash(e.s, true);
  assert.match(he, /(^|[#&])fpm=score_75_99\/view_70_99\.5(&|$)/);
  const be = st.fromHash(he);
  assert.deepEqual(be.scenicFits, { score: [75, 99], view: [70, 99.5] });
  assert.deepEqual(be.fit, e.s.fit);
});

test('a scenic metric keeps its own percentiles when another is picked, and gets them back', () => {
  const store = new st.Store(structuredClone(st.defaults));
  store.set({ fit: [70, 99] }); // the score's
  store.setMode('view');
  assert.deepEqual(store.s.fit, st.defaults.fit); // the views': the default, 20 % … 0.1 %
  store.set({ fit: [90, 99.5] });
  store.setMode('elev');
  assert.deepEqual(store.s.scenicFits, { score: [70, 99], view: [90, 99.5] });
  store.setMode('score');
  assert.deepEqual(store.s.fit, [70, 99]);
  assert.deepEqual(store.s.scenicFits, { view: [90, 99.5] });
  store.setMode('view');
  assert.deepEqual(store.s.fit, [90, 99.5]);
});

test('per-layer links\' numbers (fl, fp, rail\'s and ferries\' fit) open as every metric\'s', () => {
  // A link the app with the numbers per layer wrote (e6d1a8f's toHash): views shown in %, 10 screen
  // widths to 1 for roads, rail by frequency at 20 to 2, ferries 12 to 1.
  const h = '#m=view&fp=85,99&fl=10,1&fu=p&un=11101&rl=0.5,'
    + '&rs=1,11111,metric,freq,rocket,1,0,100,,,,e8ecf2,0.6,0,0,0,1,70,99.8,0,0.6,0,a,50,1,20,2'
    + '&fy=1,1111,freq,oslo,,1,8fc8ff,0.9,0,0,0,1,freq,1,-0.845,2,0,100,0,0.45,0.5,0,a,0.577,12,1&sqx=waterfall&oc=1';
  const b = st.fromHash(h);
  assert.deepEqual(b.fitLens, allOf(ROAD, [10, 1]));
  assert.deepEqual(b.fit, [85, 99]);
  assert.deepEqual(b.scenicFits, allOf(ROAD.filter((k) => k !== 'view'), [85, 99]));
  assert.deepEqual(b.rail.fitLens, allOf(RAIL, [20, 2]));
  assert.deepEqual(b.ferry.fitLens, { freq: [12, 1] });
  assert.equal(st.toHash(b, true), h);
  // Another scenic metric picked: the link's one set.
  const store = new st.Store(b);
  store.setMode('drama');
  assert.deepEqual(store.s.fit, [85, 99]);
  // Shown with elevation, fp was elevation's: the scenic metrics keep the default.
  const be = st.fromHash('#m=elev&p=viridis&fp=1,99&fl=10,1&un=11101&rl=0.5,&sqx=waterfall&lf=0.7,0.6&oc=1');
  assert.deepEqual(be.fit, [1, 99]);
  assert.deepEqual(be.scenicFits, {});
  assert.deepEqual(be.fitLens, allOf(ROAD, [10, 1]));
});

test('saved settings: each metric\'s numbers kept; the per-layer ones saved before apply to every metric', () => {
  const s = structuredClone(st.defaults);
  s.fitLens = { score: [10, 1] };
  s.scenicFits = { view: [70, 99] };
  s.rail.fitLens = { freq: [8, 1] };
  s.ferry.fitLens = { freq: [12, 1] };
  const b = st.fromSaved(JSON.parse(JSON.stringify(s)));
  assert.deepEqual([b.fitLens, b.scenicFits, b.rail.fitLens, b.ferry.fitLens], [{ score: [10, 1] }, { view: [70, 99] }, { freq: [8, 1] }, { freq: [12, 1] }]);
  // Saved with the numbers per layer, a scenic metric shown: its fit every scenic metric's.
  const old = st.fromSaved({ mode: 'view', fit: [85, 99], fitLen: [10, 1], rail: { fitLen: [20, 2] }, ferry: { fitLen: [12, 1] } });
  assert.deepEqual(old.fit, [85, 99]);
  assert.deepEqual(old.scenicFits, allOf(ROAD.filter((k) => k !== 'view'), [85, 99]));
  assert.deepEqual(old.fitLens, allOf(ROAD, [10, 1]));
  assert.deepEqual(old.rail.fitLens, allOf(RAIL, [20, 2]));
  assert.deepEqual(old.ferry.fitLens, { freq: [12, 1] });
  // … another display type shown: the scenic look's.
  const lk = { palette: 'pubugn', fit: [70, 99], equalize: false, lowFade: 0.8, lowSpan: 0.6, thrOn: false, thrDir: 'above' };
  const oe = st.fromSaved({ mode: 'elev', fit: [1, 99], looks: { scenic: lk } });
  assert.deepEqual(oe.scenicFits, allOf(ROAD, [70, 99]));
  assert.deepEqual(oe.fit, [1, 99]);
  // The defaults saved before: nothing kept.
  const od = st.fromSaved({ mode: 'score', fit: [80, 99.9], fitLen: [15, 1] });
  assert.deepEqual([od.fitLens, od.scenicFits], [{}, {}]);
  // Bad values dropped.
  const bad = st.fromSaved({ fitLens: { score: [1, 10], view: [10, 1], elev: [10, 1], zz: [9, 1] }, scenicFits: { view: [99, 80], water: [60, 99] }, rail: { fitLen: 'x' } });
  assert.deepEqual([bad.fitLens, bad.scenicFits, bad.rail.fitLens], [{ view: [10, 1] }, { water: [60, 99] }, {}]);
});

test('saved settings: each metric\'s unit kept; the per-layer unit saved before it applies to every metric', () => {
  // As saved now.
  const s = structuredClone(st.defaults);
  s.fitUnits = { score: 'pct' };
  s.rail.fitUnits = { viaduct: 'pct' };
  s.ferry.fitUnits = { freq: 'pct' };
  assert.deepEqual(pcts(st.fromSaved(JSON.parse(JSON.stringify(s)))), [['score'], ['viaduct'], ['freq']]);
  // As saved with the unit per layer.
  const old = st.fromSaved({ fitUnit: 'pct', rail: { fitUnit: 'widths' }, ferry: { fitUnit: 'pct' } });
  assert.deepEqual(pcts(old), [[...ROAD].sort(), [], ['freq']]);
  // Bad values: screen widths; unknown metrics and values dropped.
  const bad = st.fromSaved({ fitUnit: 'furlongs', rail: { fitUnits: { rscore: 'pct', elev: 'pct', freq: 'widths', curvy: 1 } }, ferry: { fitUnit: 3 } });
  assert.deepEqual(pcts(bad), [[], ['rscore'], []]);
  assert.deepEqual(pcts(st.fromSaved({ fitUnits: ['score'] })), [[], [], []]);
});

test('malformed link fields: a bare -, pairs with more parts', () => {
  const keys = ['a', 'b'];
  assert.deepEqual(unitsOfField('-', keys), {}); // not every metric in %
  assert.deepEqual(st.fromHash('#fu=-').fitUnits, {});
  assert.deepEqual(pairsOfField('a_10_1_3/b_8_1', keys, validFitLen), { b: [8, 1] });
  assert.deepEqual(st.fromHash('#flm=view_10_1_3/water_8_1').fitLens, { water: [8, 1] });
});

test('the scenic metrics\' percentiles while another display type is shown: a base of their own (fpb)', () => {
  const s = structuredClone(st.defaults);
  s.mode = 'elev';
  s.fit = [1, 99];
  s.scenicFits = { ...allOf(ROAD, [70, 99]), view: [60, 99.5] };
  const h = st.toHash(s, true);
  assert.match(h, /(^|[#&])fp=1,99(&|$)/);
  assert.match(h, /(^|[#&])fpb=70,99(&|$)/);
  assert.match(h, /(^|[#&])fpm=view_60_99\.5(&|$)/);
  assert.ok(h.length < 200, h);
  const b = st.fromHash(h);
  assert.deepEqual(b.fit, [1, 99]);
  assert.deepEqual(b.scenicFits, s.scenicFits);
  assert.equal(st.toHash(b, true), h);
  // While a scenic metric is shown, fp is that base and fpb isn't written (nor read).
  const t = structuredClone(st.defaults);
  t.fit = [70, 99];
  t.scenicFits = allOf(ROAD.filter((k) => k !== 'score'), [70, 99]);
  const ht = st.toHash(t, true);
  assert.ok(!/fpb=/.test(ht), ht);
  assert.match(ht, /(^|[#&])fp=70,99(&|$)/);
  assert.deepEqual(st.fromHash(ht + '&fpb=50,99').scenicFits, t.scenicFits);
});

test('a link read as the page reads a pasted one (hashchange: the state replaced in place)', () => {
  const store = new st.Store(structuredClone(st.defaults));
  store.set({ fit: [70, 99], fitLens: { score: [10, 1] } });
  store.setMode('view');
  const s = structuredClone(st.defaults);
  s.mode = 'drama';
  s.fitLens = { drama: [20, 2] };
  s.fitUnits = { drama: 'pct', view: 'pct' };
  s.fit = [75, 99];
  s.scenicFits = { score: [60, 99] };
  s.rail.fitLens = { freq: [8, 1] };
  const h = st.toHash(s, true);
  // As main.ts's hashchange listener: everything but the view set on the store.
  const { view: _v, ...rest } = st.fromHash(h);
  store.set(rest);
  assert.equal(st.toHash(store.s, true), h);
  assert.deepEqual([store.s.fitLens, store.s.scenicFits, store.s.fit], [{ drama: [20, 2] }, { score: [60, 99] }, [75, 99]]);
  // … and the metrics switched to afterwards bring their own.
  store.setMode('score');
  assert.deepEqual(store.s.fit, [60, 99]);
  assert.deepEqual(store.s.scenicFits, { drama: [75, 99] });
});

test('the scenic metric shown is never kept in scenicFits', () => {
  // A link listing it: its percentiles are fit.
  const b = st.fromHash('#m=view&fpm=view_60_99/water_70_99');
  assert.deepEqual(b.fit, [60, 99]);
  assert.deepEqual(b.scenicFits, { water: [70, 99] });
  // Saved settings listing it.
  const sv = st.fromSaved({ mode: 'view', fit: [65, 99], scenicFits: { view: [10, 90], water: [70, 99] } });
  assert.deepEqual(sv.fit, [65, 99]);
  assert.deepEqual(sv.scenicFits, { water: [70, 99] });
  for (const x of [b, sv]) assert.ok(!(x.mode in x.scenicFits));
});

test('old saved settings with a mode that no longer exists: the default mode, its fit every scenic metric\'s', () => {
  const o = st.fromSaved({ mode: 'bogus', fit: [75, 99], fitLen: [8, 1] });
  assert.equal(o.mode, st.defaults.mode);
  assert.deepEqual(o.fit, [75, 99]);
  assert.deepEqual(o.scenicFits, allOf(ROAD.filter((k) => k !== st.defaults.mode), [75, 99]));
  assert.ok(!(o.mode in o.scenicFits));
  assert.deepEqual(o.fitLens, allOf(ROAD, [8, 1]));
});

test('every rail and ferry metric\'s palette and percentiles through a link, those as first picked left out', () => {
  const s = structuredClone(st.defaults);
  s.rail.metric = 'freq'; // the shown one's in the fields it always had
  s.rail.palette = 'magma';
  s.rail.looks = {
    rscore: { ...st.railFreshLook('rscore'), palette: 'magma', fit: [60, 99] }, // the ride score changed from the defaults'
    curvy: { ...st.railFreshLook('curvy'), fit: [5, 95] }, // the palette as first picked
    view: { ...st.railFreshLook('view'), palette: 'plasma_r' }, // a reversed ramp
    elev: st.railFreshLook('elev'), // as first picked: not listed
    freq: { ...st.railFreshLook('freq'), palette: 'turbo' }, // the shown one's old look: not listed
  };
  s.ferry.looks = { months: { ...st.ferryFreshLook('months'), palette: 'greens-cb', fit: [10, 90] } };
  const h = st.toHash(s, true);
  assert.match(h, /rs=[^&]*,15,1,,,rscore_magma_60_99\/view_plasma_r\/curvy__5_95(&|$)/);
  assert.match(h, /fy=[^&]*,15,1,,,months_greens-cb_10_90(&|$)/);
  const b = st.fromHash(h);
  assert.equal(b.rail.palette, 'magma');
  assert.deepEqual(Object.keys(b.rail.looks).sort(), ['curvy', 'rscore', 'view']);
  for (const k of ['rscore', 'curvy', 'view']) assert.deepEqual(b.rail.looks[k], s.rail.looks[k]);
  assert.deepEqual(b.ferry.looks, s.ferry.looks);
  assert.equal(st.toHash(b, true), h);
  // Unknown metrics, the shown one and malformed entries ignored.
  const odd = h.replace(/(rs=[^&]*,15,1,,,)[^&]*/, '$1zz_magma/freq_turbo/curvy_/view_bad!name/elev__1_200/drama_inferno_5_95');
  assert.deepEqual(Object.keys(st.fromHash(odd).rail.looks), ['drama']);
  assert.deepEqual(st.fromHash(odd).rail.looks.drama, { ...st.railFreshLook('drama'), palette: 'inferno', fit: [5, 95] });
});

test('links from before every metric\'s look travelled: the shown one\'s as before, the others as first picked', () => {
  // A link in the older form (nothing after the screen widths): rail by frequency (magma, 10–90),
  // ferries by season length.
  const h = '#un=11101&rl=0.5,&rs=1,11111,metric,freq,magma,1,0,2.5,,,,e8ecf2,0.4,0,0,0,1,10,90,0,0.6,0,a,1.25,1,15,1'
    + '&fy=1,1111,freq,oslo,,1,8fc8ff,0.9,0,0,0,1,months,1,0,12,0,100,0,0,0.6,0,a,6,15,1&sqx=waterfall';
  const b = st.fromHash(h);
  assert.equal(b.rail.metric, 'freq');
  assert.equal(b.rail.palette, 'magma');
  assert.deepEqual(b.rail.fit, [10, 90]);
  assert.deepEqual([b.rail.looks, b.ferry.looks], [{}, {}]);
  assert.equal(b.ferry.metric, 'months');
  assert.equal(st.toHash(b, true), h);
});

test('each layer\'s default metric: its first look is the defaults\'', () => {
  assert.deepEqual(st.railFreshLook('rscore'), st.lookOfScale(st.defaults.rail));
  assert.deepEqual(st.ferryFreshLook('freq'), st.lookOfScale(st.defaults.ferry));
  // Untouched and not shown: not in the link.
  const s = structuredClone(st.defaults);
  s.rail.looks = { rscore: st.lookOfScale(st.defaults.rail) };
  s.rail.metric = 'freq';
  Object.assign(s.rail, st.scaleOfLook(st.railFreshLook('freq')));
  s.ferry.looks = { freq: st.lookOfScale(st.defaults.ferry) };
  s.ferry.metric = 'months';
  Object.assign(s.ferry, st.scaleOfLook(st.ferryFreshLook('months')));
  const h = st.toHash(s, true);
  assert.match(h, /rs=[^&]*,freq,[^&]*,15,1(&|$)/);
  assert.match(h, /fy=[^&]*,months,[^&]*,15,1(&|$)/);
  // Changed: its palette and percentiles listed, its fades the defaults' after the round trip.
  s.ferry.looks = { freq: { ...st.lookOfScale(st.defaults.ferry), palette: 'magma', fit: [5, 95] } };
  s.rail.looks = { rscore: { ...st.lookOfScale(st.defaults.rail), palette: 'turbo' } };
  const b = st.fromHash(st.toHash(s, true));
  assert.deepEqual(b.ferry.looks.freq, s.ferry.looks.freq);
  assert.deepEqual([b.ferry.looks.freq.lowFade, b.ferry.looks.freq.lowSpan], [0.45, 0.5]);
  assert.deepEqual(b.rail.looks.rscore, s.rail.looks.rscore);
  assert.equal(b.rail.looks.rscore.lowFade, 0.6);
});

test('an older link showing rail frequency, then the ride score picked: the defaults\' ride score', () => {
  const h = '#un=11101&rl=0.5,&rs=1,11111,metric,freq,magma,1,0,2.5,,,,e8ecf2,0.4,0,0,0,1,10,90,0,0.6,0,a,1.25,1,15,1&sqx=waterfall';
  const b = st.fromHash(h);
  assert.deepEqual(b.rail.looks, {});
  // As the rail card's metric menu does.
  const r = b.rail, looks = { ...r.looks, [r.metric]: st.lookOfScale(r) };
  const next = { ...r, metric: 'rscore', looks, ...st.scaleOfLook(looks.rscore ?? st.railFreshLook('rscore')) };
  assert.equal(next.palette, 'rocket');
  assert.deepEqual(next.fit, [70, 99.8]);
  assert.equal(next.lowFade, 0.6);
});
