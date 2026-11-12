// node web/test/links.test.ts (tools/dev/check.sh's web part): links as a profile plus overrides
// (src/state.ts toHash/fromHash, task #819). A link names its profile and carries only the settings
// that differ from it; read back over the same profile it gives the same settings, every one a
// link can carry; a link naming no profile reads over the base it is given (the default profile).
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
const { seed, fromHash, toHash, linkFields, fromProfile, profileSettings } = st;
type AppState = typeof defaults;

const same = (a: AppState, b: AppState, buildings = true) => assert.deepEqual(Object.fromEntries(linkFields(a, buildings)), Object.fromEntries(linkFields(b, buildings)));
const params = (h: string) => new URLSearchParams(h.slice(1));

/** A profile far from the defaults: most kinds of setting changed. */
function night(): AppState {
  const s = structuredClone(seed);
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
  ['depth', (s) => (s.depth = { ...s.depth, on: !s.depth.on, scale: 'linear' })],
  ['depth colours', (s) => (s.depth = { ...s.depth, colours: { ...s.depth.colours, palette: 'quiet', auto: false, range: [3, 900], fit: [5, 95], equalize: true } })],
  ['buildings', (s) => (s.buildings = { ...s.buildings, on: !s.buildings.on })],
  ['water', (s) => (s.water = { ...s.water, colour: '#123456' })],
  ['whitewater', (s) => (s.water = { ...s.water, foam: 0.5, foamFrom: 12, foamTo: 120, foamCover: 0.6, foamColour: '#e0f0ff' })],
  ['river lines off', (s) => (s.water = { ...s.water, rivers: false })],
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
for (const b of [base.s, seed]) {
  for (const [what, f] of changes) {
    const s = structuredClone(b);
    f(s);
    const h = toHash(s, true, { id: 'k3f9', name: 'x', s: b });
    try {
      same(fromHash(h, true, b), s);
    } catch (e) {
      throw new Error(`${what} over ${b === seed ? 'the defaults' : 'the profile'}: ${h}\n${(e as Error).message}`);
    }
    assert.ok([...params(h).keys()].length > 1, `${what}: nothing in ${h}`);
  }
}

// A link's water from before the whitewater (ten fields) keeps the whitewater's defaults; a full
// steepness at or under the start is moved past it.
{
  const old = fromHash('#map=5/46/-71&wa=123456,1,28,0.22,1.8,4f7fa8,0.25,0,0.35,0', true, seed);
  assert.equal(old.water.colour, '#123456');
  for (const k of ['foam', 'foamFrom', 'foamTo', 'foamCover', 'foamColour'] as const) assert.equal(old.water[k], seed.water[k]);
  const w = fromHash('#map=5/46/-71&wa=123456,1,28,0.22,1.8,4f7fa8,0.25,0,0.35,0,0.8,30,10,0.75,cee0ec', true, seed).water;
  assert.deepEqual([w.foamFrom, w.foamTo], [30, 31]);
}

// Older links' depth (#854): its palette, 'fit' (now Auto, the ramp log-spaced) and the ramp's end
// (now the range from 0) in dp=; saved settings alike.
{
  const tail = '1,0.45,315,1,8fae7d,0,zoom,10,5,1,9ec3dc,1,0.55,0';
  const fit = fromHash(`#map=5/46/-71&dp=1,quiet,fit,900,${tail}`, true, seed).depth;
  assert.deepEqual([fit.scale, fit.colours.palette, fit.colours.auto, fit.colours.range], ['log', 'quiet', true, [0, 900]]);
  const lin = fromHash(`#map=5/46/-71&dp=1,nautical,linear,40,${tail}`, true, seed).depth;
  assert.deepEqual([lin.scale, lin.colours.auto, lin.colours.range], ['linear', false, [0, 40]]);
  const saved = st.fromSaved({ depth: { ...seed.depth, colours: undefined, palette: 'quiet', scale: 'fit', max: 120 } }, seed).depth;
  assert.deepEqual([saved.scale, saved.colours.palette, saved.colours.auto, saved.colours.range], ['log', 'quiet', true, [0, 120]]);
  // A link over a profile carrying dp= alone keeps the profile's colour scale.
  const p = structuredClone(seed);
  p.depth.colours = { ...p.depth.colours, auto: true, palette: 'quiet' };
  const s = structuredClone(p);
  s.depth = { ...s.depth, opacity: 0.5 };
  const h = toHash(s, true, { id: 'k3f9', name: 'x', s: p });
  assert.equal(params(h).get('ds'), null);
  assert.deepEqual(fromHash(h, true, p).depth.colours, p.depth.colours);
}

// Older links: no profile named, each setting where it differs from the default profile (given as
// the base), the palette in p=.
{
  const s = fromHash('#map=5/46/-71&p=cividis&eq=1&gb=0', true, base.s);
  assert.equal(s.palette, 'cividis');
  assert.equal(s.equalize, true);
  assert.equal(s.globe, false);
  assert.equal(s.terrain.exaggeration, base.s.terrain.exaggeration);
  assert.equal(s.rail.on, base.s.rail.on);
  // A bare link is the base, whole.
  same(fromHash('#map=5/46/-71', true, base.s), base.s);
  assert.equal(params(toHash(s, true, base)).get('pa'), 'cividis');
  assert.equal(params(toHash(s, true, base)).get('p'), 'k3f9-scenic-night');
}

// The Map look and road look exactly back from a link (each colour, opacity, zoom), over a profile
// and over the defaults; older links' Map preset (ms=) sets the preset's look.
{
  const s = structuredClone(base.s);
  s.roadMap = { ...presetLook(preset('michelin')), kind: 'speed' };
  s.roadMap.fill[0] = '#abcdef12';
  s.roadMap.cats.speed[3] = '#fedcba';
  s.roadLook = { ...s.roadLook, minZoom: s.roadLook.minZoom.map((_, i) => i / 2), widths: s.roadLook.widths.map((r) => r.map((v) => v + 0.125)) };
  for (const b of [base.s, seed]) {
    const back = fromHash(toHash(s, true, { id: 'k3f9', name: 'x', s: b }), true, b);
    assert.deepEqual(back.roadMap, s.roadMap);
    assert.deepEqual(back.roadLook, s.roadLook);
  }
  const old = fromHash('#map=5/46/-71&ms=os', true, seed);
  assert.equal(presetOfLook(old.roadMap)?.key, 'os');
  // A preset cases the road classes, not ferries (as the Map styles did before #820: 0x1ff).
  const { lookUniforms } = await import('../src/mapschemes.ts');
  for (const m of MAP_SCHEMES) assert.equal(lookUniforms(presetLook(m)).casedMask, 0x1ff, m.key);
  // An unknown preset or a malformed look leaves the defaults'.
  assert.deepEqual(fromHash('#ms=nope&rm=class,zz,zz,1,a,b,c,d', true, seed).roadMap.fill, seed.roadMap.fill);
  // A look colouring by what the app no longer has (the signed routes, removed by #853), linked,
  // saved or as a preset's name: the default look, whole.
  const signed = toHash({ ...structuredClone(s), roadMap: { ...s.roadMap, kind: 'network' as never } }, true, { id: 'base', name: 'Default', s: seed });
  assert.match(signed, /rm=network,/);
  assert.deepEqual(fromHash(signed, true, seed).roadMap, seed.roadMap);
  assert.deepEqual(fromHash('#map=5/46/-71&ms=network', true, seed).roadMap, seed.roadMap);
  assert.deepEqual(st.fromSaved({ roadMap: { ...s.roadMap, kind: 'network' } }, seed).roadMap, seed.roadMap);
  assert.deepEqual(st.fromSaved({ mapScheme: 'network' }, seed).roadMap, seed.roadMap);
  assert.deepEqual(fromHash('#rx=1.2,3&wd=1,2', true, seed).roadLook, seed.roadLook);
  // Out-of-range values clamped.
  assert.equal(fromHash(`#rx=${new Array(10).fill(40).join('_')},99,-1,0,1,1`, true, seed).roadLook.casingFrom, st.ROAD_LOOK_RANGE.casingFrom[1]);
  // Saved before the Map look: its preset (mapScheme); and in a profile, the look itself.
  assert.equal(presetOfLook(st.fromSaved({ mapScheme: 'neon' }, seed).roadMap)?.key, 'neon');
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
  // A profile holding nothing: the seed.
  same(fromProfile({}), seed);
}

// The seed is the one definition of a Default profile's settings (default-profile.json, which the
// server seeds a new Default from too): whole, so that a profile holding nothing is the seed; the
// settings it gives back are the file's, and a save or a link missing a setting takes the profile's
// (not the seed's).
{
  const file = JSON.parse((await import('node:fs')).readFileSync(new URL('../default-profile.json', import.meta.url), 'utf8'));
  assert.deepEqual(profileSettings(seed), file);
  same(fromProfile(file), seed);
  assert.deepEqual(st.fromSaved(null, base.s), base.s);
  assert.deepEqual(st.fromSaved({ palette: 'cividis' }, base.s), { ...base.s, palette: 'cividis' });
  const partial = profileSettings(base.s);
  delete partial.terrain;
  assert.deepEqual(st.fromSaved(partial, base.s).terrain, base.s.terrain);
  assert.notDeepEqual(base.s.terrain, seed.terrain);
  // A broken value in a save is the profile's, not the seed's.
  assert.deepEqual(st.fromSaved({ landmarks: { top: 'x' } }, base.s).landmarks.top, base.s.landmarks.top);
  // So is a malformed width table or depth palette.
  const p = structuredClone(base.s);
  p.roadLook.widths = p.roadLook.widths.map((r) => r.map((v) => v * 2));
  p.depth.colours.palette = 'quiet';
  const bad = profileSettings(p) as { roadLook: { widths: unknown[] }; depth: { colours: { palette: string } } };
  bad.roadLook.widths = bad.roadLook.widths.map(() => ['x']);
  bad.depth.colours.palette = 'nope';
  const got = st.fromSaved(bad, p);
  assert.deepEqual([got.roadLook.widths, got.depth.colours.palette], [p.roadLook.widths, 'quiet']);
}

console.log('links: ok');
