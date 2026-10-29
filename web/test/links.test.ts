// node web/test/links.test.ts (tools/dev/check.sh's web part): links as a profile plus overrides
// (src/state.ts toHash/fromHash, task #819). A link names its profile and carries only the settings
// that differ from it; read back over the same profile it gives the same settings, every one a
// link can carry; older links (no profile) still read over the defaults.
import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';

// The app's modules import without extensions and import JSON as Vite does: resolved here.
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

const st = await import('../src/state.ts');
const { MAP_SCHEMES, presetLook, presetOfLook } = await import('../src/mapschemes.ts');
const preset = (k: string) => MAP_SCHEMES.find((m) => m.key === k)!;
const { defaults, fromHash, toHash, linkFields, fromProfile, profileSettings } = st;
type AppState = typeof defaults;

const same = (a: AppState, b: AppState, buildings = true) => assert.deepEqual(Object.fromEntries(linkFields(a, buildings)), Object.fromEntries(linkFields(b, buildings)));
const params = (h: string) => new URLSearchParams(h.slice(1));

/** A profile far from the defaults: most kinds of setting changed. */
function night(): AppState {
  const s = structuredClone(defaults);
  s.palette = 'magma';
  s.equalize = true;
  s.groups = s.groups.map((_, i) => i % 2 === 0);
  s.roadLenOn = false;
  s.routeGlow = true;
  s.globe = false;
  s.occlude = true;
  s.threshold = { on: true, dir: 'below', value: 0.4 };
  s.layers = { ...s.layers, boundaries: false };
  s.labelOpacity = 0.5;
  s.terrain = { ...s.terrain, exaggeration: 2.5, sky: !s.terrain.sky };
  s.rail = { ...s.rail, on: !s.rail.on, opacity: 0.6 };
  s.ferry = { ...s.ferry, dashed: !s.ferry.dashed };
  s.labelLooks = { city: { colour: '#ff8800', size: 1.4 }, river: { shown: false } };
  s.stopFilters = {};
  s.heritageOff = ['w.c'];
  return s;
}

const base = { id: 'k3f9', name: 'Scenic night', s: night() };

// The profile alone: its name in the link, no settings.
{
  const s = { ...structuredClone(base.s), view: { zoom: 7, lat: 46.1, lng: -71.2, bearing: 0, pitch: 0, elev: 0 } };
  const h = toHash(s, true, base);
  assert.deepEqual([...params(h).keys()], ['map', 'p']);
  assert.equal(params(h).get('p'), 'k3f9-scenic-night');
  same(fromHash(h, true, base.s), s);
}

// Overrides: only what differs from the profile, including a setting put back to the defaults'
// (the profile's on, the link's off) and a label type's look.
{
  const s = structuredClone(base.s);
  s.globe = true;
  s.routeGlow = false;
  s.labelOpacity = 0.9;
  s.labelLooks = { ...s.labelLooks, town: { font: 'Noto Sans Italic' } };
  const h = toHash(s, true, base);
  assert.deepEqual([...params(h).keys()].sort(), ['gb', 'lo', 'lx', 'p', 'rg']);
  const back = fromHash(h, true, base.s);
  same(back, s);
  assert.equal(back.globe, true);
  assert.equal(back.routeGlow, false);
  assert.deepEqual(back.labelLooks.town, { font: 'Noto Sans Italic' });
}

// Each kind of setting changed alone round-trips over the profile and over the defaults.
const changes: [string, (s: AppState) => void][] = [
  ['mode', (s) => st.Store.prototype.setMode.call({ s, set(p: Partial<AppState>) { Object.assign(s, p); } }, 'curvy')],
  ['palette', (s) => (s.palette = 'cividis')],
  ['range', (s) => ((s.auto = false), (s.range = [10, 90]))],
  ['equalize off', (s) => (s.equalize = !s.equalize)],
  ['weights', (s) => ((s.preset = ''), (s.weights = s.weights.map((w, i) => (i ? w : w + 0.5))))],
  ['groups', (s) => (s.groups = s.groups.map((g) => !g))],
  ['road length', (s) => ((s.roadLen = [2, 40]), (s.roadLenOn = !s.roadLenOn))],
  ['surface', (s) => (s.surface = { paved: true, unpaved: false })],
  ['threshold off', (s) => (s.threshold = { ...s.threshold, on: !s.threshold.on })],
  ['layers', (s) => (s.layers = { ...s.layers, water: !s.layers.water })],
  ['heritage', (s) => (s.heritageOff = s.heritageOff.length ? [] : ['n.top'])],
  ['stop filters', (s) => (s.stopUnknown = { heritage: false })],
  ['terrain', (s) => (s.terrain = { ...s.terrain, hillshade: !s.terrain.hillshade, light: 200 })],
  ['rail', (s) => (s.rail = { ...s.rail, on: !s.rail.on, ties: 1.5 })],
  ['ferry', (s) => (s.ferry = { ...s.ferry, opacity: 0.35 })],
  ['depth', (s) => (s.depth = { ...s.depth, on: !s.depth.on, max: 900 })],
  ['buildings', (s) => (s.buildings = { ...s.buildings, on: !s.buildings.on })],
  ['water', (s) => (s.water = { ...s.water, colour: '#123456' })],
  ['land', (s) => (s.land = '#202020')],
  ['globe', (s) => (s.globe = !s.globe)],
  ['occlude', (s) => (s.occlude = !s.occlude)],
  ['label looks', (s) => (s.labelLooks = Object.keys(s.labelLooks).length ? {} : { state: { case: 'uppercase' } })],
  ['label kinds', (s) => (s.labelKinds = { ...s.labelKinds, town: !s.labelKinds.town })],
  // Task #820: the Map look (a preset, then edited) and the road look every colour option shares.
  ['map preset', (s) => (s.roadMap = presetLook(preset(s.roadMap.kind === 'class' && presetOfLook(s.roadMap)?.key === 'carto' ? 'neon' : 'carto'), s.roadMap))],
  ['map look', (s) => (s.roadMap = {
    ...s.roadMap, kind: 'surface', fill: s.roadMap.fill.map((c, i) => (i === 8 ? '#ff000080' : c)), casing: s.roadMap.casing.map((c, i) => (i === 2 ? '#123456' : c)),
    cased: s.roadMap.cased.map((x, i) => (i < 3 ? !x : x)), cats: { ...s.roadMap.cats, surface: s.roadMap.cats.surface.map((c, i) => (i === 1 ? '#00ff0040' : c)) },
  })],
  ['road look', (s) => (s.roadLook = { ...s.roadLook, minZoom: s.roadLook.minZoom.map((z, i) => (i < 2 ? 11.5 : z)), casingFrom: 11, casingWidth: 1.75, tunnel: 0.7, bridgeCasing: !s.roadLook.bridgeCasing, dashUnpaved: !s.roadLook.dashUnpaved })],
  ['road widths', (s) => (s.roadLook = { ...s.roadLook, widths: s.roadLook.widths.map((r, c) => (c === 8 ? r.map((v) => v * 1.5) : r)) })],
];
for (const b of [base.s, defaults]) {
  for (const [what, f] of changes) {
    const s = structuredClone(b);
    f(s);
    const h = toHash(s, true, { id: 'k3f9', name: 'x', s: b });
    try {
      same(fromHash(h, true, b), s);
    } catch (e) {
      throw new Error(`${what} over ${b === defaults ? 'the defaults' : 'the profile'}: ${h}\n${(e as Error).message}`);
    }
    assert.ok([...params(h).keys()].length > 1, `${what}: nothing in ${h}`);
  }
}

// Older links: no profile, each setting where it differs from the defaults, the palette in p=.
{
  const s = fromHash('#map=5/46/-71&p=magma&eq=1&gb=0');
  assert.equal(s.palette, 'magma');
  assert.equal(s.equalize, true);
  assert.equal(s.globe, false);
  same(fromHash(toHash(s, true), true), s);
  // With no profile named, the link stays relative to the defaults (and its palette in pa=).
  assert.equal(params(toHash(s, true)).get('pa'), 'magma');
  assert.equal(params(toHash(s, true)).get('p'), null);
}

// The Map look and road look exactly back from a link (each colour, opacity, zoom), over a profile
// and over the defaults; older links' Map preset (ms=) sets the preset's look.
{
  const s = structuredClone(base.s);
  s.roadMap = { ...presetLook(preset('michelin')), kind: 'speed' };
  s.roadMap.fill[0] = '#abcdef12';
  s.roadMap.cats.speed[3] = '#fedcba';
  s.roadLook = { ...s.roadLook, minZoom: s.roadLook.minZoom.map((_, i) => i / 2), widths: s.roadLook.widths.map((r) => r.map((v) => v + 0.125)) };
  for (const b of [base.s, defaults]) {
    const back = fromHash(toHash(s, true, { id: 'k3f9', name: 'x', s: b }), true, b);
    assert.deepEqual(back.roadMap, s.roadMap);
    assert.deepEqual(back.roadLook, s.roadLook);
  }
  const old = fromHash('#map=5/46/-71&ms=os');
  assert.equal(presetOfLook(old.roadMap)?.key, 'os');
  // An unknown preset or a malformed look leaves the defaults'.
  assert.deepEqual(fromHash('#ms=nope&rm=class,zz,zz,1,a,b,c,d').roadMap.fill, defaults.roadMap.fill);
  assert.deepEqual(fromHash('#rx=1.2,3&wd=1,2').roadLook, defaults.roadLook);
  // Out-of-range values clamped.
  assert.equal(fromHash(`#rx=${new Array(10).fill(40).join('_')},99,-1,0,1,1`).roadLook.casingFrom, st.ROAD_LOOK_RANGE.casingFrom[1]);
  // Saved before the Map look: its preset (mapScheme); and in a profile, the look itself.
  assert.equal(presetOfLook(st.fromSaved({ mapScheme: 'neon' }).roadMap)?.key, 'neon');
  const kept = fromProfile(JSON.parse(JSON.stringify(profileSettings(s))));
  assert.deepEqual(kept.roadMap, s.roadMap);
  assert.deepEqual(kept.roadLook, s.roadLook);
}

// A profile's settings (what the server keeps): the state without the camera or the selection;
// read back, the same settings.
{
  const s = { ...night(), view: { zoom: 3, lat: 1, lng: 2, bearing: 0, pitch: 0, elev: 0 }, selected: { way: 5, at: [1, 2] as [number, number] } };
  const kept = JSON.parse(JSON.stringify(profileSettings(s)));
  assert.ok(!('view' in kept) && !('selected' in kept) && !('stretch' in kept));
  const back = fromProfile(kept);
  same(back, s);
  assert.equal(back.view, null);
  // The first profile's settings are {}: the defaults.
  same(fromProfile({}), defaults);
}

console.log('links: ok');
