// node web/test/links.test.ts (tools/dev/check.sh's web part): links as a profile plus overrides
// (src/state.ts toHash/fromHash, task #819). A link names its profile and carries only the settings
// that differ from it; read back over the same profile it gives the same settings, every one a
// link can carry; a link naming no profile reads over the base it is given (the default profile).
// A param or saved value not in the shape the app writes is ignored, whole: the profile's stays.
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

// The whitewater's full steepness at or under its start is moved past it.
{
  const w = fromHash('#map=5/46/-71&wa=123456,1,28,0.22,1.8,4f7fa8,0.25,0,0.35,0,0.8,30,10,0.75,cee0ec,1', true, seed).water;
  assert.equal(w.colour, '#123456');
  assert.deepEqual([w.foamFrom, w.foamTo], [30, 31]);
}

// A link over a profile carrying dp= alone keeps the profile's colour scale.
{
  const p = structuredClone(seed);
  p.depth.colours = { ...p.depth.colours, auto: true, palette: 'quiet' };
  const s = structuredClone(p);
  s.depth = { ...s.depth, opacity: 0.5 };
  const h = toHash(s, true, { id: 'k3f9', name: 'x', s: p });
  assert.equal(params(h).get('ds'), null);
  assert.deepEqual(fromHash(h, true, p).depth.colours, p.depth.colours);
}

// Data available's colour maps in a link: the profile's first one and each layer's own (#1373: a
// reload, which reads the link, kept neither); a layer's map in a link over a profile that has
// others replaces them; none changed, none in the link.
{
  const p = structuredClone(seed);
  const s = structuredClone(p);
  const year = { ...s.available.colours, palette: 'magma', auto: false, range: [2010, 2020] as [number, number] };
  s.available = { ...s.available, colours: { ...s.available.colours, palette: 'cividis' }, scales: { year, score: { ...year, palette: 'turbo_r', auto: true, threshold: { on: true, dir: 'below' as const, value: 0.5 } } } };
  const h = toHash(s, true, { id: 'k3f9', name: 'x', s: p });
  assert.ok(params(h).get('am')!.startsWith('score~turbo_r,1,'), h);
  const back = fromHash(h, true, p).available;
  assert.deepEqual(back.colours, s.available.colours);
  assert.deepEqual(back.scales, s.available.scales);
  const q = structuredClone(p);
  q.available.scales = { year };
  assert.deepEqual(fromHash(toHash(p, true, { id: 'k3f9', name: 'x', s: q }), true, q).available.scales, {});
  assert.equal(params(toHash(p, true, { id: 'k3f9', name: 'x', s: p })).get('am'), null);
  assert.deepEqual(fromHash('#map=5/46/-71&am=Bad~x*year~magma,0', true, p).available.scales, {});
}

// A link naming no profile: each setting where it differs from the default profile (given as the base).
{
  const s = fromHash('#map=5/46/-71&pa=cividis&eq=1&gb=0', true, base.s);
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
// and over the defaults.
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
  // A preset cases the road classes, not ferries (as the Map styles did before #820: 0x1ff).
  const { lookUniforms } = await import('../src/mapschemes.ts');
  for (const m of MAP_SCHEMES) assert.equal(lookUniforms(presetLook(m)).casedMask, 0x1ff, m.key);
  // A malformed look leaves the defaults'.
  assert.deepEqual(fromHash('#rm=class,zz,zz,1,a,b,c,d', true, seed).roadMap.fill, seed.roadMap.fill);
  // A look colouring by what the app doesn't offer, linked or saved: the profile's look, whole.
  const signed = toHash({ ...structuredClone(s), roadMap: { ...s.roadMap, kind: 'network' as never } }, true, { id: 'base', name: 'Default', s: seed });
  assert.match(signed, /rm=network,/);
  assert.deepEqual(fromHash(signed, true, seed).roadMap, seed.roadMap);
  assert.deepEqual(st.fromSaved({ roadMap: { ...s.roadMap, kind: 'network' } }, seed).roadMap, seed.roadMap);
  assert.deepEqual(fromHash('#rx=1.2,3&wd=1,2', true, seed).roadLook, seed.roadLook);
  // Out-of-range values clamped.
  assert.equal(fromHash(`#rx=${new Array(10).fill(40).join('_')},99,-1,0,1,1`, true, seed).roadLook.casingFrom, st.ROAD_LOOK_RANGE.casingFrom[1]);
  // In a profile, the look itself.
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

// Links and saves in shapes the app doesn't write (a param it doesn't have, a value with too few
// fields, a list of another length, a key it doesn't have): each is ignored, whole, and the
// profile's setting stays; nothing throws and nothing else changes. Over a profile far from the
// seed and over the seed.
for (const b of [base.s, seed]) {
  const at = '#map=5/46/-71';
  const unchanged: string[] = [
    // params the app doesn't have
    'p=cividis', 'ms=os', 'hl=10101', 'w=1.5', 'tn=terrain,custom,0,900,0,1,0.6,elev', 'tf=0.5,0.6,0.5,0.6', 'k=abc',
    // too few fields
    'wa=123456,1,28,0.22,1.8,4f7fa8,0.25,0,0.35,0', 'rs=1,11111,group,rscore,rocket,1,0,100,,,,e8ecf2,0.6,0,0,0,1',
    'fy=0,1111,freq,oslo,1.5,1,8fc8ff', 'dp=1,quiet,fit,900,0.5,0.45,315,1,8fae7d,0,zoom,10,5,1,9ec3dc,1,0.55', 'bd=1,0,height,0.9,1',
    'cl=1,0.4,0.8,ffffff,0,0.5,1,6', 'lm=0.5,1,0,1,0.5,0,100,0,0.6,0,a,0.5', 'rk=2', 'pl=ff0000', 'te=viridis,1,0,900', 'ts=viridis', 'ds=quiet,1', 'bh=viridis,1',
    'lk=1010', 't3=1,2,1', 'tv=elev,0', 'lw=1,1,1', 'o=101',
    // weight lists of another length, a preset named without its weights, weights without their preset
    'pr=custom&wt=1,2,3,4,5,6,7,8,9,10,11', 'pr=custom&wt=1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16', 'pr=vistas', `wt=${b.weights.map((w) => w + 1).join(',')}`,
    'rp=custom&rw=1,2,3,4,5,6,7,8,9,10', 'rw=1,2,3,4,5,6,7,8,9,10,11', 'rp=balanced',
    // values of another kind
    'wt=a,b&pr=custom', 'lx=%7Bnot-json', 'rm=nope', 'rx=1.2,3', 'wd=1,2', 'ld=1,2', 'lf=x,y', 'r=5', 'm=nope', 'fg=1,2', 'tc=1', 's=12',
  ];
  for (const q of unchanged) {
    const got = fromHash(`${at}&${q}`, true, b);
    assert.deepEqual({ ...got, view: null }, { ...b, view: null }, q);
  }
  // All of them at once, with one current param: only that one applies.
  const all = fromHash(`${at}&${unchanged.filter((q) => !/^(pr|wt|rp|rw|r|m)=/.test(q)).join('&')}&gb=${b.globe ? 0 : 1}`, true, b);
  assert.deepEqual({ ...all, view: null }, { ...b, view: null, globe: !b.globe });
  // A dp= with something in its unused fields 2 and 4: they are ignored, the rest read.
  const dp = fromHash(`${at}&dp=1,quiet,linear,900,0.5,0.45,315,1,8fae7d,0,zoom,10,5,1,9ec3dc,1,0.55,0`, true, b).depth;
  assert.deepEqual(dp.colours, b.depth.colours);
  assert.deepEqual([dp.on, dp.scale, dp.opacity], [true, 'linear', 0.5]);
  // Saved state and profiles' settings: keys the app doesn't have, values of another type or length.
  const saves: unknown[] = [
    { mapScheme: 'neon', heritageLevels: [true, false, true, false, true], weight: 1.5, fitUnit: 'pct', fitLen: [10, 1], roadWidths: [[1]] },
    { weights: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], rail: { weights: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10], weight: 2, stopOutline: '#aabbcc', stopOutlineOpacity: 0.3, fitUnit: 'pct', fitLen: [20, 2], colour: 'group' } },
    { ferry: { weight: 2, colour: 'freq', fitUnit: 'pct', fitLen: [12, 1] }, depth: { palette: 'quiet', scale: 'fit', max: 120 } },
    { terrain: { tintPalette: 'terrain', tintRange: 'custom', tintMin: 0, tintMax: 900, tintFade: { elev: 0.5 } }, buildings: { height: 'x', heritage: 1 } },
    { mode: 'nope', palette: 7, range: [1], groups: [true], labelKinds: 'all', lineWeights: null, water: [], landmarks: { top: 'x' } },
    'text', 7, [], null,
  ];
  for (const o of saves) assert.deepEqual(st.fromSaved(structuredClone(o), b), b, JSON.stringify(o));
  // … and beside a current value, that one applies.
  assert.deepEqual(st.fromSaved({ mapScheme: 'neon', fitUnit: 'pct', globe: !b.globe }, b), { ...b, globe: !b.globe });
  // A number too large to be one (1e309, Infinity once read) is unreadable: the profile's.
  // (Over the same link with that field as written: a link's numbers are rounded.)
  const fy = linkFields(b, true).get('fy')!.split(',');
  for (const i of [9, 10, 14, 15]) {
    const f = [...fy];
    f[i] = i === 14 ? '-1e309' : '1e309';
    const want = fromHash(`${at}&fy=${fy.join(',')}`, true, b).ferry;
    assert.deepEqual(fromHash(`${at}&fy=${f.join(',')}`, true, b).ferry, i < 14 ? { ...want, [i === 9 ? 'freqMin' : 'freqMax']: b.ferry[i === 9 ? 'freqMin' : 'freqMax'] } : { ...want, range: b.ferry.range }, `fy[${i}]`);
  }
  assert.deepEqual(fromHash(`${at}&rl=1e309,5`, true, b).roadLen, [b.roadLen[0], 5]);
  const huge = st.fromSaved(JSON.parse('{"range":[0,1e309],"view":{"zoom":5,"lat":46,"lng":-71,"bearing":1e309,"pitch":-1e309}}'), b);
  assert.deepEqual([huge.range, huge.view], [b.range, { zoom: 5, lat: 46, lng: -71, bearing: 0, pitch: 0, elev: 0 }]);
}
// A profile's settings in an older shape: each such value is the seed's.
assert.deepEqual(fromProfile({ mapScheme: 'neon', weights: [1, 2, 3], rail: { colour: 'group' }, terrain: { tintPalette: 'terrain' } }), seed);

console.log('links: ok');
