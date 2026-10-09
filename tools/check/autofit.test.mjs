// node --test tools/check/autofit.test.mjs: auto-fitting colour ranges by screen widths or by
// percentiles of line length (web/src/autofit.ts, which Node loads with its types stripped), and
// the unit's round trip through a link (web/src/state.ts, loaded through Vite for its imports).
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { bestOfPct, fitByPct, fitByWidths, screenWidthKm, setBestPct, spread } from '../../web/src/autofit.ts';
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

test('screen widths are the default, and a default link says nothing of the unit', () => {
  const s = structuredClone(st.defaults);
  assert.equal(s.fitUnit, 'widths');
  assert.equal(s.rail.fitUnit, 'widths');
  assert.equal(s.ferry.fitUnit, 'widths');
  const h = st.toHash(s, true);
  assert.ok(!/(^|[#&])(fu|rs|fy)=/.test(h), h);
});

test('percentiles, for roads, rail and ferries, survive a link', () => {
  const s = structuredClone(st.defaults);
  s.fitUnit = 'pct';
  s.fit = [85, 99];
  s.rail.fitUnit = 'pct';
  s.rail.fit = [60, 99.5];
  s.ferry.fitUnit = 'pct';
  const back = st.fromHash(st.toHash(s, true));
  assert.equal(back.fitUnit, 'pct');
  assert.deepEqual(back.fit, [85, 99]);
  assert.equal(back.rail.fitUnit, 'pct');
  assert.deepEqual(back.rail.fit, [60, 99.5]);
  assert.equal(back.ferry.fitUnit, 'pct');
  // Each on its own.
  for (const k of ['road', 'rail', 'ferry']) {
    const t = structuredClone(st.defaults);
    (k === 'road' ? t : t[k]).fitUnit = 'pct';
    const b = st.fromHash(st.toHash(t, true));
    assert.deepEqual([b.fitUnit, b.rail.fitUnit, b.ferry.fitUnit], ['road', 'rail', 'ferry'].map((x) => (x === k ? 'pct' : 'widths')));
  }
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
  assert.deepEqual([b.fitUnit, b.rail.fitUnit, b.ferry.fitUnit], ['widths', 'widths', 'widths']);
  assert.deepEqual(b.rail.fitLen, [20, 2]);
  assert.deepEqual(b.ferry.fitLen, [12, 1]);
  assert.deepEqual(b.fitLen, [10, 1]);
  // Shorter, older rail and ferry fields too.
  const o = st.fromHash(h.replace(/(rs=[^&]*),20,2/, '$1').replace(/(fy=[^&]*),12,1/, '$1'));
  assert.deepEqual([o.fitUnit, o.rail.fitUnit, o.ferry.fitUnit], ['widths', 'widths', 'widths']);
});

test('saved settings: a bad unit reads as screen widths', () => {
  const b = st.fromSaved({ fitUnit: 'furlongs', rail: { fitUnit: 'pct' }, ferry: { fitUnit: 3 } });
  assert.deepEqual([b.fitUnit, b.rail.fitUnit, b.ferry.fitUnit], ['widths', 'pct', 'widths']);
});
