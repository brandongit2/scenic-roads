// node --test tools/check/autofit.test.mjs: auto-fitting colour ranges by screen widths or by
// percentiles of line length (web/src/autofit.ts, which Node loads with its types stripped), and
// each metric's unit through a link and saved settings, old ones too (web/src/state.ts, loaded through Vite for its imports).
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { allPct, bestOfPct, fitByPct, fitByWidths, screenWidthKm, setBestPct, spread, unitOf, unitsField, unitsOfField, withUnit } from '../../web/src/autofit.ts';
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

test('older links, without the unit, read as screen widths', () => {
  const s = structuredClone(st.defaults);
  s.rail.fitLen = [20, 2];
  s.ferry.fitLen = [12, 1];
  s.fitLen = [10, 1];
  const h = st.toHash(s, true);
  // Screen widths add nothing to a link: the rail and ferry fields end at fitLen, as before the unit.
  assert.match(h, /rs=[^&]*,20,2(&|$)/);
  assert.match(h, /fy=[^&]*,12,1(&|$)/);
  const b = st.fromHash(h);
  assert.deepEqual(pcts(b), [[], [], []]);
  assert.deepEqual(b.rail.fitLen, [20, 2]);
  assert.deepEqual(b.ferry.fitLen, [12, 1]);
  assert.deepEqual(b.fitLen, [10, 1]);
  // Shorter, older rail and ferry fields too.
  const o = st.fromHash(h.replace(/(rs=[^&]*),20,2/, '$1').replace(/(fy=[^&]*),12,1/, '$1'));
  assert.deepEqual(pcts(o), [[], [], []]);
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
