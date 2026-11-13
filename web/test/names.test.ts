// node web/test/names.test.ts (tools/dev/check.sh's web part): the names links, saved state,
// profiles and preset lists use (task #855): the colourings' keys, the score factors keyed as the
// metric of their channel, the preset lists' localStorage keys. A name the app doesn't have is
// ignored: the profile's colouring stays, a weight under an unknown key isn't read.
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';

registerHooks({
  resolve(spec, ctx, next) {
    if ((spec.startsWith('./') || spec.startsWith('../')) && !/\.[a-z]+$/.test(spec)) {
      for (const ext of ['.ts', '/index.ts']) {
        try {
          return next(spec + ext, ctx);
        } catch {
          /* the next */
        }
      }
    }
    const r = next(spec, ctx);
    return spec.endsWith('.json') ? { ...r, importAttributes: { type: 'json' } } : r;
  },
});

// The browser's storage, holding an edited rail list (presets.ts reads it as it loads) and lists
// under keys the app doesn't read.
const mem = new Map<string, string>();
Object.defineProperty(globalThis, 'localStorage', {
  configurable: true,
  value: { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => void mem.set(k, v), removeItem: (k: string) => void mem.delete(k) },
});
mem.set('scenic-roads:presets.rail', JSON.stringify([
  { id: 'balanced', name: 'Balanced', weights: { view: 1, water: 1.2, vista: 0.8, drama: 1, ridge: 0.8, elev: 0.4, viaduct: 0.8, tunnel: -1, grade: 0.4, curvy: 0.3, freq: 0.3 } },
  // No trains a day: a factor at 0 isn't listed. Keys the factors don't have aren't read.
  { id: 'zero-trains', name: 'Zero trains', weights: { view: 1.1, views: 9, water: 1.3, vista: 0.9, drama: 1.1, ridge: 0.9, ledge: 9, elev: 0.5, viaduct: 0.9, tunnel: -0.9, grade: 0.5, curvy: 0.4 } },
]));
mem.set('scenic-roads:weightPresets', JSON.stringify([{ id: 'mine', name: 'Mine', weights: { view: 3 } }]));
mem.set('scenic-roads:railWeightPresets', JSON.stringify([{ id: 'other', name: 'Other', weights: { view: 3 } }]));

const st = await import('../src/state.ts');
const { presets, railPresets, PresetList } = await import('../src/presets.ts');
const { RAIL_COMPONENTS } = await import('../src/rail.ts');
const { COMPONENTS, isScenic } = await import('../src/scenic.ts');
const { fromHash, fromSaved, fromProfile, toHash } = st;
/** The profile the tests read links and saves over: a fixed one (profile-fixture.json), not the
 * seed, which follows the owner's Default. */
const fx = fromProfile(JSON.parse((await import('node:fs')).readFileSync(new URL('./profile-fixture.json', import.meta.url), 'utf8')));
const field = (h: string, k: string, i: number) => new URLSearchParams(h.slice(1)).get(k)!.split(',')[i];
const at = '#map=9.00/46.00000/-71.00000';

// The colourings' names in links: written and read as they are; one the app doesn't have: the profile's.
{
  const rs = (colour: string) => `${at}&rs=1,11111,${colour},rscore,rocket,1,0,100,,,,ff0000,0.6,0,0,0,1,70,99.8,0,0.6,0,a,50,1,15,1`;
  const fy = (colour: string) => `${at}&fy=1,1111,${colour},oslo,,1,8fc8ff,0.4,0,0,0,1,months,1,-0.845,2,0,100,0,0.45,0.5,0,a,0.577,15,1`;
  assert.equal(fromHash(rs('service'), true, fx).rail.colour, 'service');
  const f = fromHash(fy('operator'), true, fx);
  assert.deepEqual([f.ferry.colour, f.ferry.metric, f.ferry.opacity], ['operator', 'months', 0.4]);
  const h = toHash({ ...structuredClone(fx), rail: { ...fx.rail, colour: 'service' }, ferry: { ...fx.ferry, opacity: 0.4 } }, true, { id: 'base', name: 'Default', s: fx });
  assert.equal(field(h, 'rs', 2), 'service');
  assert.equal(field(h, 'fy', 2), 'metric');
  for (const c of ['group', 'colourful']) assert.equal(fromHash(rs(c), true, fx).rail.colour, fx.rail.colour);
  assert.equal(fromHash(fy('freq'), true, fx).ferry.colour, fx.ferry.colour);
  assert.equal(fromHash(rs('group'), true, fx).rail.single, '#ff0000'); // (the rest of the value read)
}

// Rail weights in a link: every factor's, with their preset; any other length, a value not a
// number, or no preset beside them: not read.
{
  const rw = '0.7,0.4,1,1.3,2.4,0.5,2.6,-0.3,1.1,1.2,2';
  const s = fromHash(`${at}&rw=${rw}&rp=custom`, true, fx);
  assert.deepEqual(s.rail.weights, rw.split(',').map(Number));
  assert.equal(s.rail.preset, '');
  assert.equal(fromHash(`${at}&rw=${rw}&rp=balanced`, true, fx).rail.preset, 'balanced');
  assert.equal(fromHash(`${at}&rw=${rw}&rp=not-here`, true, fx).rail.preset, ''); // a preset this browser hasn't
  for (const q of [`rw=${rw.slice(0, -2)}&rp=custom`, 'rw=1,2,3&rp=custom', `rw=${rw.replace('0.4', 'x')}&rp=custom`, `rw=${rw.replace('0.4', '')}&rp=custom`, `rw=${rw}`, 'rp=balanced']) {
    assert.deepEqual(fromHash(`${at}&${q}`, true, fx).rail, fx.rail, q);
  }
}

// Saved state and profiles (the same reader).
{
  const now = fromSaved({ rail: { colour: 'service' }, ferry: { colour: 'operator', metric: 'months' } }, fx);
  assert.deepEqual([now.rail.colour, now.ferry.colour, now.ferry.metric], ['service', 'operator', 'months']);
  const odd = { rail: { colour: 'group', weights: [0.7, 0.4, 1, 1.3, 2.4, 0.5, 2.6, -0.3, 1.1, 1.2] }, ferry: { colour: 'freq' } };
  for (const [s, b] of [[fromSaved(structuredClone(odd), fx), fx], [fromProfile(structuredClone(odd)), st.seed]] as const) {
    assert.deepEqual([s.rail.colour, s.rail.weights, s.ferry.colour], [b.rail.colour, b.rail.weights, b.ferry.colour]);
  }
  assert.equal(fx.ferry.colour, 'metric');
}

// Preset lists: read from presets.roads and presets.rail by the factors' keys.
{
  const w = (id: string, list: typeof railPresets) => list.get(id)!.w;
  assert.equal(COMPONENTS.length, 12);
  assert.deepEqual(RAIL_COMPONENTS.map((c) => c.key), ['view', 'water', 'vista', 'drama', 'ridge', 'elev', 'viaduct', 'tunnel', 'grade', 'curvy', 'freq']);
  assert.ok(railPresets.custom);
  assert.deepEqual(w('balanced', railPresets), [1, 1.2, 0.8, 1, 0.8, 0.4, 0.8, -1, 0.4, 0.3, 0.3]);
  assert.deepEqual(w('zero-trains', railPresets), [1.1, 1.3, 0.9, 1.1, 0.9, 0.5, 0.9, -0.9, 0.5, 0.4, 0]);
  // Lists under other keys aren't read: the built-ins.
  assert.ok(!presets.custom);
  assert.deepEqual(presets.list, presets.builtin);
  assert.equal(railPresets.get('other'), undefined);
  // An edit saves the list by the factors' keys, those at 0 left out, and reads back the same.
  railPresets.rename('zero-trains', 'No trains');
  const saved = JSON.parse(mem.get('scenic-roads:presets.rail')!) as { id: string; name: string; weights: Record<string, number> }[];
  assert.deepEqual(Object.keys(saved[1].weights), RAIL_COMPONENTS.map((c) => c.key).filter((k) => k !== 'freq'));
  const rails = await import('../src/data/rail-presets.json', { with: { type: 'json' } });
  const again = new PresetList('presets.rail', RAIL_COMPONENTS, rails.default.presets);
  assert.deepEqual(again.list, railPresets.list);
  assert.equal(again.list[1].name, 'No trains');
  // "Built-ins" leaves no list.
  railPresets.restoreBuiltins();
  assert.equal(mem.get('scenic-roads:presets.rail'), 'null');
  const reset = new PresetList('presets.rail', RAIL_COMPONENTS, rails.default.presets);
  assert.equal(reset.custom, false);
  assert.deepEqual(reset.list, reset.builtin);
  // A stored value that isn't a list of presets: the built-ins.
  for (const bad of ['{"a":1}', '[{"id":"x"}]', '[7]', 'not json']) {
    mem.set('scenic-roads:presets.rail', bad);
    assert.deepEqual(new PresetList('presets.rail', RAIL_COMPONENTS, rails.default.presets).list, reset.builtin, bad);
  }
}

// The built-in presets name every factor by a key the components have.
{
  for (const [comps, file] of [[COMPONENTS, '../src/data/weight-presets.json'], [RAIL_COMPONENTS, '../src/data/rail-presets.json']] as const) {
    const keys = new Set<string>(comps.map((c) => c.key));
    const data = (await import(file, { with: { type: 'json' } })).default as { presets: { weights: Record<string, number> }[] };
    for (const p of data.presets) for (const k of Object.keys(p.weights)) assert.ok(keys.has(k), `${file}: ${k}`);
  }
}

// One "is scenic": every mode but elevation, grade, local relief and the street map.
assert.deepEqual(['elev', 'grade', 'relief', 'map', 'score', 'drama', 'bldg'].map((m) => isScenic(m as never)), [false, false, false, false, true, true, true]);
assert.equal(st.modeGroup('drama'), 'scenic');
assert.equal(st.modeGroup('relief'), 'relief');
