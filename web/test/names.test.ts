// node web/test/names.test.ts (tools/dev/check.sh's web part): the names links, saved state,
// profiles and preset lists use (task #855), and the older names still read as the new ones. Each
// older link, save and list here was written by the app before the renames:
// - rail colouring 'group' → 'service', ferry colouring 'freq' → 'metric' (links' rs= and fy=,
//   saved state, profiles);
// - score factors keyed as the metric of their channel: roads views → view, relief → drama,
//   unblocked → openness; rail views → view, relief → drama, ledge → ridge, altitude → elev,
//   gradient → grade (preset lists);
// - edited preset lists kept under presets.roads and presets.rail (were weightPresets,
//   railWeightPresets), the rail ones' service frequency stored even at 0;
// - rail weights from before the service-frequency factor: its def (0.3), as every older weight list.
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

// The browser's storage, holding lists the older app saved (presets.ts reads it as it loads).
const mem = new Map<string, string>();
Object.defineProperty(globalThis, 'localStorage', {
  configurable: true,
  value: { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => void mem.set(k, v), removeItem: (k: string) => void mem.delete(k) },
});
mem.set('scenic-roads:railWeightPresets', JSON.stringify([
  { id: 'balanced', name: 'Balanced', weights: { views: 1, water: 1.2, vista: 0.8, relief: 1, ledge: 0.8, altitude: 0.4, viaduct: 0.8, tunnel: -1, gradient: 0.4, curvy: 0.3, freq: 0.3 } },
  // Saved at no trains a day: the older app left the factor out.
  { id: 'zero-trains', name: 'Zero trains', weights: { views: 1.1, water: 1.3, vista: 0.9, relief: 1.1, ledge: 0.9, altitude: 0.5, viaduct: 0.9, tunnel: -0.9, gradient: 0.5, curvy: 0.4 } },
]));
mem.set('scenic-roads:weightPresets', JSON.stringify([
  { id: 'mine', name: 'Mine', weights: { views: 1, water: 1.05, vista: 0.6, relief: 0.95, ridge: 0.5, curvy: 0.85, unblocked: 0.9, forest: 0.35, built: 0.4, bldg: -0.55, route: 1, viewpoint: 0.95 } },
  // From before roadside buildings: they weigh their def.
  { id: 'old', name: 'Old', weights: { views: 2 } },
]));

const st = await import('../src/state.ts');
const { presets, railPresets } = await import('../src/presets.ts');
const { RAIL_COMPONENTS } = await import('../src/rail.ts');
const { COMPONENTS, addedWeights, isScenic, migrateWeights } = await import('../src/scenic.ts');
const { defaults, fromHash, fromSaved, fromProfile, toHash } = st;
const field = (h: string, k: string, i: number) => new URLSearchParams(h.slice(1)).get(k)!.split(',')[i];
const at = '#map=9.00/46.00000/-71.00000';

// Older links (written by the app before the renames) read with the new names, the rest as they were.
{
  const r = fromHash(`${at}&rs=1,11111,group,rscore,rocket,1,0,100,,,,e8ecf2,0.6,0,0,0,1,70,99.8,0,0.6,0,a,50,1,15,1`);
  assert.equal(r.rail.colour, 'service');
  assert.deepEqual({ ...r.rail, colour: defaults.rail.colour }, defaults.rail);
  const f = fromHash(`${at}&fy=1,1111,freq,oslo,,1,8fc8ff,0.4,0,0,0,1,months,1,-0.845,2,0,100,0,0.45,0.5,0,a,0.577,15,1`);
  assert.equal(f.ferry.colour, 'metric');
  assert.equal(f.ferry.metric, 'months');
  assert.equal(f.ferry.opacity, 0.4);
  // A link of either name gives the same settings.
  const both = (rs: string, fy: string) => `${at}&rs=1,11111,${rs},rscore,rocket,1,0,100,,,,ff0000,0.6,0,0,0,1,70,99.8,0,0.6,0,a,50,1,15,1&fy=1,1111,${fy},oslo,,0,8fc8ff,0.9,0,0,0,1,freq,1,-0.845,2,0,100,0,0.45,0.5,0,a,0.577,15,1`;
  assert.deepEqual(fromHash(both('group', 'freq')), fromHash(both('service', 'metric')));
  // New links write the new names.
  const h = toHash({ ...structuredClone(defaults), rail: { ...defaults.rail, colour: 'service' }, ferry: { ...defaults.ferry, opacity: 0.4 } }, true);
  assert.equal(field(h, 'rs', 2), 'service');
  assert.equal(field(h, 'fy', 2), 'metric');
  // A colouring neither name has: the base's.
  assert.equal(fromHash(`${at}&rs=1,11111,colourful,rscore,rocket,1,0,100,,,,e8ecf2,0.6,0,0,0,1`).rail.colour, defaults.rail.colour);
}

// Rail weights in a link from before the service-frequency factor: one fewer, it at its def.
{
  const rw = '0.7,0.4,1,1.3,2.4,0.5,2.6,-0.3,1.1,1.2';
  const s = fromHash(`${at}&rw=${rw}&rp=custom`);
  assert.deepEqual(s.rail.weights, [...rw.split(',').map(Number), 0.3]);
  assert.equal(s.rail.preset, '');
  assert.deepEqual(fromHash(`${at}&rw=${rw},2&rp=custom`).rail.weights, [...rw.split(',').map(Number), 2]);
  // Any other length, or a value not a number: not weights.
  assert.deepEqual(fromHash(`${at}&rw=1,2,3`).rail.weights, defaults.rail.weights);
  assert.deepEqual(fromHash(`${at}&rw=${rw.replace('0.4', 'x')}`).rail.weights, defaults.rail.weights);
}

// Saved state and profiles (the same reader) from before the renames.
{
  const old = { rail: { colour: 'group', weights: [0.7, 0.4, 1, 1.3, 2.4, 0.5, 2.6, -0.3, 1.1, 1.2] }, ferry: { colour: 'freq', metric: 'months' } };
  for (const s of [fromSaved(structuredClone(old)), fromProfile(structuredClone(old))]) {
    assert.equal(s.rail.colour, 'service');
    assert.deepEqual(s.rail.weights, [...old.rail.weights, 0.3]);
    assert.equal(s.ferry.colour, 'metric');
    assert.equal(s.ferry.metric, 'months');
  }
  // The saved object itself is left as it was.
  assert.equal(old.rail.weights.length, 10);
  const now = fromSaved({ rail: { colour: 'service' }, ferry: { colour: 'metric' } });
  assert.deepEqual([now.rail.colour, now.ferry.colour], ['service', 'metric']);
  assert.equal(fromSaved({ rail: { colour: 'nope' }, ferry: { colour: 'nope' } }).ferry.colour, defaults.ferry.colour);
  assert.equal(defaults.ferry.colour, 'metric');
}

// Older weight lists: the road ones' 16 and 11 values, the rail ones' 10.
{
  assert.equal(COMPONENTS.length, 12);
  const w11 = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
  assert.deepEqual(migrateWeights(w11), [1, 2, 3, 4, 5, 6, 7, 8, 9, -1, 10, 11]);
  assert.deepEqual(migrateWeights([1, 2, 3, 4, 5, 6, 7, 8, 99, 9, 10, 11, 99, 99, 99, 99]), [1, 2, 3, 4, 5, 6, 7, 8, 9, -1, 10, 11]);
  assert.equal(migrateWeights([1, 2, 3, 4, 5, 6, 7, 8, NaN, 9, 10, 11, 0, 0, 0, 0]), null);
  assert.deepEqual(addedWeights([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], RAIL_COMPONENTS), [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0.3]);
  assert.equal(addedWeights([1, 2], RAIL_COMPONENTS), null);
}

// Preset lists saved before the renames: read from the older keys and names, each weight as the
// older app read it (no trains a day left out: 0; buildings left out: their def).
{
  const w = (id: string, list: typeof railPresets) => list.get(id)!.w;
  assert.ok(railPresets.custom && presets.custom);
  assert.deepEqual(w('balanced', railPresets), [1, 1.2, 0.8, 1, 0.8, 0.4, 0.8, -1, 0.4, 0.3, 0.3]);
  assert.deepEqual(w('zero-trains', railPresets), [1.1, 1.3, 0.9, 1.1, 0.9, 0.5, 0.9, -0.9, 0.5, 0.4, 0]);
  assert.deepEqual(w('mine', presets), [1, 1.05, 0.6, 0.95, 0.5, 0.85, 0.9, 0.35, 0.4, -0.55, 1, 0.95]);
  assert.deepEqual(w('old', presets), [2, 0, 0, 0, 0, 0, 0, 0, 0, -1, 0, 0]);
  // An edit saves under the new key, by the new names, the service frequency even at 0, and drops the older key.
  railPresets.rename('zero-trains', 'No trains');
  assert.equal(mem.get('scenic-roads:railWeightPresets'), undefined);
  const saved = JSON.parse(mem.get('scenic-roads:presets.rail')!) as { id: string; weights: Record<string, number> }[];
  assert.deepEqual(Object.keys(saved[1].weights), RAIL_COMPONENTS.map((c) => c.key));
  assert.equal(saved[1].weights.freq, 0);
  assert.deepEqual(RAIL_COMPONENTS.map((c) => c.key), ['view', 'water', 'vista', 'drama', 'ridge', 'elev', 'viaduct', 'tunnel', 'grade', 'curvy', 'freq']);
  // "Built-ins" leaves neither key with a list.
  presets.restoreBuiltins();
  assert.equal(mem.get('scenic-roads:weightPresets'), undefined);
  assert.equal(mem.get('scenic-roads:presets.roads'), 'null');
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
