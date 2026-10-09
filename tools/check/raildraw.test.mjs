// node --test tools/check/raildraw.test.mjs: rail lines' cross-ties and stop dots' size
// (web/src/raildraw.ts, which Node loads with its types stripped), and their settings' round trip
// through a link and saved settings (web/src/state.ts, loaded through Vite for its imports).
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { CORE_K, CORE_MIN_CSS, STOP_CONTRAST, TIE_MIN_Z, railGeom, stopBounds, stopFactor } from '../../web/src/raildraw.ts';
import { createServer } from '../../web/node_modules/vite/dist/node/index.js';

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps, `${a} ≉ ${b}`);

test('ties: a fixed length on screen while the line is at its thinnest', () => {
  // Zoomed out, the line's half-width shrinks; the core keeps its floor and the ties their length.
  for (const dpr of [1, 2]) {
    const lens = [0.2, 0.5, 1, 1.4].map((h) => railGeom(h * dpr, dpr, 2.5, 10).tie);
    for (const l of lens) near(l, 2.5 * CORE_MIN_CSS * dpr);
  }
});

test('ties: never shorter than the line is thick, at any width', () => {
  for (let h = 0.1; h < 12; h *= 1.3) {
    const g = railGeom(h, 2, 1, 12);
    assert.ok(g.tie >= g.core, `half-width ${h}: tie ${g.tie} < core ${g.core}`);
    // ...and the core is at least its floor.
    assert.ok(g.core >= CORE_MIN_CSS * 2 - 1e-9);
  }
});

test('ties: the setting is a multiple of the core line’s thickness', () => {
  const wide = 8; // device px: past the floor, the core follows the line's width
  for (const k of [1, 2.5, 4]) near(railGeom(wide, 2, k, 14).tie, k * wide * CORE_K);
  // Today's look at zoom 12-14 (ties spanning the line's width, the core 0.42 of it): about ×2.4.
  near(railGeom(wide, 2, 1 / CORE_K, 13).tie, wide);
});

test('ties: none below zoom 9', () => {
  assert.equal(railGeom(4, 2, 2.5, TIE_MIN_Z - 0.01).tie, 0);
  assert.equal(railGeom(4, 2, 2.5, 6).tie, 0);
  assert.ok(railGeom(4, 2, 2.5, TIE_MIN_Z).tie > 0);
  assert.equal(railGeom(4, 2, 0, 12).tie, 0);
});

test('stop dots: the default contrast is the size by spacing the map always had', () => {
  assert.equal(STOP_CONTRAST, 0.5);
  for (const sp of [200, 800, 4000, 12000, 60000, 400000]) near(stopFactor(sp, STOP_CONTRAST), Math.min(1.4, Math.max(0.5, 1 + 0.15 * Math.log2(sp / 4000))));
});

test('stop dots: contrast 0 sizes every stop alike; more contrast spreads them', () => {
  for (const sp of [100, 4000, 1e6]) assert.equal(stopFactor(sp, 0), 1);
  const spread = (c) => stopFactor(100000, c) / stopFactor(500, c);
  assert.ok(spread(0) < spread(0.5) && spread(0.5) < spread(1));
  // Always a positive size, within the bounds.
  for (const c of [0, 0.25, 0.5, 0.75, 1]) {
    const [lo, hi] = stopBounds(c);
    assert.ok(lo > 0 && lo <= 1 && hi >= 1);
    for (const sp of [1, 100, 4000, 1e7]) { const f = stopFactor(sp, c); assert.ok(f >= lo && f <= hi); }
  }
});

// The link round trip, through Vite's module loader (state.ts imports JSON and extensionless paths).
const vite = await createServer({
  root: fileURLToPath(new URL('../../web', import.meta.url)), configFile: false, logLevel: 'error', appType: 'custom',
  server: { middlewareMode: true, hmr: false, ws: false }, optimizeDeps: { noDiscovery: true },
});
after(() => vite.close());
const st = await vite.ssrLoadModule('/src/state.ts');

const look = (r) => ({ ties: r.ties, stopSize: r.stopSize, stopContrast: r.stopContrast, stopOutline: r.stopOutline, stopOutlineOpacity: r.stopOutlineOpacity });

test('the defaults add nothing to a link', () => {
  const h = st.toHash(structuredClone(st.defaults), true);
  assert.ok(!/(^|[#&])rk=/.test(h), h);
});

test('ties and stop dots survive a link', () => {
  const s = structuredClone(st.defaults);
  Object.assign(s.rail, { ties: 3.5, stopSize: 1.75, stopContrast: 0.2, stopOutline: '#ffcc00', stopOutlineOpacity: 0.4 });
  const h = st.toHash(s, true);
  assert.match(h, /rk=3\.5,1\.75,0\.2,ffcc00,0\.4/);
  assert.deepEqual(look(st.fromHash(h).rail), look(s.rail));
});

test('older links, without them, read as the defaults; bad values are clamped or dropped', () => {
  const s = structuredClone(st.defaults);
  s.rail.single = '#123456'; // some rail field in the link, so it has an rs= but no rk=
  const b = st.fromHash(st.toHash(s, true));
  assert.deepEqual(look(b.rail), look(st.defaults.rail));
  const c = st.fromHash('#rk=99,0.01,7,nothex,-1');
  assert.deepEqual(look(c.rail), { ties: 5, stopSize: 0.25, stopContrast: 1, stopOutline: st.defaults.rail.stopOutline, stopOutlineOpacity: 0 });
  const d = st.fromHash('#rk=2');
  assert.deepEqual(look(d.rail), { ...look(st.defaults.rail), ties: 2 });
});

test('saved settings keep them, and settings saved before them take the defaults', () => {
  const r = { ties: 1.5, stopSize: 3, stopContrast: 0.9, stopOutline: '#AABBCC', stopOutlineOpacity: 0.3 };
  assert.deepEqual(look(st.fromSaved({ rail: r }).rail), { ...r, stopOutline: '#aabbcc' });
  assert.deepEqual(look(st.fromSaved({ rail: { on: true } }).rail), look(st.defaults.rail));
  assert.deepEqual(look(st.fromSaved({ rail: { ties: 'x', stopOutline: 'red' } }).rail), look(st.defaults.rail));
});
