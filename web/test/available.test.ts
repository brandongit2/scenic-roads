// node web/test/available.test.ts (tools/dev/check.sh's web part): the data available layers' model
// (src/available.ts): the themes the catalogue lists, the server's theme description read, each
// layer's axis in its tiles' values and its colour map, the ramp's stops on the map, the tiles'
// and the click's URLs, the click's rows and the piece a layer draws (the server's Theme::best),
// and the settings in a link and a save.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  NONE, atUrl, axisOf, cleanState, fromField, isBuilt, layerName, logTicks, parsePieces, parseTheme, pieceRows, polarTileUrl, rampStops, scaleOf, shown, themesOf, tileUrl, toField,
  type AvailableState,
} from '../src/available.ts';
import type { ScaleFields } from '../src/state.ts';

/** `/api/available/terrain` as crates/server/src/available.rs `described` writes it for the real
 * score.toml (the ranges of the real pieces of 13 Nov). */
const THEME = {
  theme: 'terrain', sets: ['best'], primary: 'spacing_m', within: 1.25, share: 0.02, availability: ['open', 'login', 'paid', 'closed'],
  metrics: [
    { id: 'spacing_m', label: 'Grid spacing', unit: 'm', weight: null, scale: 'log', worst: 1000, best: 0.1, values: null },
    { id: 'surface', label: 'Bare earth or surface', unit: null, weight: 0.4, scale: 'values', worst: null, best: null, values: { ground: 1, surface: 0 } },
    { id: 'vertical_rmse_m', label: 'Stated vertical accuracy (RMSE)', unit: 'm', weight: 0.4, scale: 'log', worst: 10, best: 0.05, values: null },
    { id: 'year', label: 'Year surveyed', unit: null, weight: 0.2, scale: 'linear', worst: 2000, best: 2026, values: null },
  ],
  layers: [
    { id: 'score', label: 'Best data', unit: 'm', scale: 'log', best: 'low', words: null, range: [0.1, 90] },
    { id: 'surface', label: 'Bare earth or surface', unit: null, scale: 'values', best: 'high', range: [0, 1],
      words: [{ value: 1, label: 'ground' }, { value: 0, label: 'surface' }] },
    { id: 'vertical_rmse_m', label: 'Stated vertical accuracy (RMSE)', unit: 'm', scale: 'log', best: 'low', words: null, range: [0.05, 16] },
    { id: 'year', label: 'Year surveyed', unit: null, scale: 'linear', best: 'high', words: null, range: [2000, 2024] },
    // (A scale the app doesn't know, and a server of the painted tiles' format: left out.)
    { id: 'odd', label: 'Odd', scale: 'pie' },
    { id: 'old', label: 'Old', legend: 'classes', stops: [] },
  ],
  sources: [],
  labels: {
    sets: { best: 'Best available anywhere' },
    availability: { open: 'downloads freely', login: 'free after a login or a request', paid: 'paid', closed: 'exists, not to be had' },
    extent: { index: 'from the publisher’s tile index', claimed: 'claimed area, not checked against the data' },
  },
};

const theme = parseTheme(THEME)!;

/** The default profile's first colour map of a layer (web/default-profile.json). */
const COLOURS: ScaleFields = {
  palette: 'viridis', auto: true, range: [0, 1], fit: [1, 99], equalize: false, lowFade: 0, lowSpan: 0.5,
  threshold: { on: false, dir: 'above', value: 0 },
};
const BASE: AvailableState = { on: false, theme: 'terrain', layer: 'score', set: 'best', avail: 'free', opacity: 0.85, colours: COLOURS, scales: {} };

test('the catalogue’s themes, with their poles', () => {
  const layers = {
    'available-terrain': { maxzoom: 12 }, 'available-terrain-n': { maxzoom: 3 }, 'available-terrain-s': { maxzoom: 3 },
    'polar-terrain-s': { maxzoom: 8 }, roads: {}, 'available-bathymetry': { maxzoom: 10 },
    // (The built set's layers aren't a theme of their own.)
    'available-built-terrain': { maxzoom: 12 }, 'available-built-terrain-s': { maxzoom: 3 },
  };
  assert.deepEqual(themesOf(layers), [{ theme: 'terrain', maxzoom: 12, poles: { n: 3, s: 3 } }, { theme: 'bathymetry', maxzoom: 10, poles: {} }]);
  assert.deepEqual(themesOf(undefined), []);
});

test('a theme read: its layers in the server’s order, one it can’t draw left out', () => {
  assert.deepEqual(theme.layers.map((l) => l.id), ['score', 'surface', 'vertical_rmse_m', 'year']);
  assert.deepEqual(theme.layers[0], { id: 'score', label: 'Best data', unit: 'm', scale: 'log', best: 'low', range: [0.1, 90], words: [] });
  assert.deepEqual(theme.layers[1].words, [{ value: 0, label: 'surface' }, { value: 1, label: 'ground' }], 'by value');
  assert.deepEqual(theme.sets, ['best']);
  assert.equal(parseTheme({ theme: 'terrain', layers: [], sources: [] }), null, 'no score: no layers');
  assert.equal(parseTheme('nope'), null);
  // The painted tiles' description (an older server): no layer the app can draw, so none shown.
  assert.equal(parseTheme({ ...THEME, layers: [THEME.layers[5]] }), null);
  // A metric the app has never seen: shown as any other; no values present: the layer kept, its
  // range none; a log range not above 0, or words missing: left out.
  const more = structuredClone(THEME) as Record<string, unknown> & typeof THEME;
  (more.layers as unknown[]).push({ id: 'density', label: 'Point density', unit: 'pt/m²', scale: 'log', best: 'high', range: null });
  (more.layers as unknown[]).push({ id: 'neg', label: 'Neg', unit: 'm', scale: 'log', best: 'low', range: [0, 3] });
  (more.layers as unknown[]).push({ id: 'kind', label: 'Kind', unit: null, scale: 'values', best: 'high', range: [0, 1], words: [{ value: 1, label: 'a' }] });
  delete (more as Record<string, unknown>).labels;
  const m = parseTheme(more)!;
  assert.deepEqual(m.layers.map((l) => l.id).slice(-1), ['density']);
  assert.equal(m.layers.at(-1)!.range, null);
  assert.deepEqual(m.labels, { sets: {}, availability: {}, extent: {} });
});

test('the layer and set shown: the state’s where the theme has them, else its first', () => {
  const s: AvailableState = { ...BASE, on: true, layer: 'year' };
  assert.equal(shown(s, theme).layer.id, 'year');
  assert.equal(shown({ ...s, layer: 'gone', set: 'built' }, theme).layer.id, 'score');
  // A save or link naming the grid spacing's own layer, which the score's took over: the score's.
  assert.equal(shown({ ...s, layer: 'spacing_m' }, theme).layer.id, 'score');
  assert.equal(shown({ ...s, set: 'built' }, theme).set, 'best');
});

test('a layer’s axis: log10 of its unit on a log layer, labelled in the unit, round ticks', () => {
  const [score, surface, acc, year] = theme.layers;
  const a = axisOf(score);
  assert.deepEqual(a.domain.map((v) => +v.toFixed(6)), [+(-1 - 1 / 256).toFixed(6), +(Math.log10(90) + 1 / 256).toFixed(6)], "the values present and a 1/256th");
  assert.equal(a.step, 0.01);
  assert.equal(a.fixed, false);
  assert.deepEqual([Math.log10(0.5), 0, Math.log10(90), Math.log10(1260), Math.log10(0.6096)].map(a.fmt), ['0.5 m', '1 m', '90 m', '1300 m', '0.61 m']);
  // 1, 5 and 2 times the powers of ten, in that rank, within the axis.
  const t = logTicks(Math.log10(0.4), Math.log10(30)).map(([v, r]) => [+(10 ** v).toPrecision(3), r]);
  assert.deepEqual(t.map(([v]) => v).sort((x, y) => x - y), [0.5, 1, 2, 5, 10, 20]);
  assert.deepEqual(new Map(t.map(([v, r]) => [v, r])).get(5), 1);
  // A narrow range: at least a factor of two wide, about its middle.
  const one = axisOf({ ...acc, range: [1, 1] });
  assert.ok(Math.abs(one.domain[1] - one.domain[0] - 0.3 - 2 / 256) < 1e-9 && Math.abs(one.domain[0] + 0.15 + 1 / 256) < 1e-9);
  // The year: as it is, whole years.
  const y = axisOf(year);
  assert.deepEqual(y.domain, [2000, 2024]);
  assert.equal(y.step, 1);
  assert.equal(y.fmt(2021), '2021');
  assert.equal(y.ticks, undefined);
  // Two words: the range theirs, fixed; each value its word.
  const w = axisOf(surface);
  assert.deepEqual([w.domain, w.fixed, w.fmt(0), w.fmt(1), w.fmt(0.5)], [[0, 1], true, 'surface', 'ground', '0.5']);
  assert.deepEqual(w.ticks!(0, 1), [[0, 0], [1, 0]]);
  // (The highlight's slider steps from word to word; words not evenly spaced, a hundredth.)
  assert.equal(w.step, 1);
  const three = axisOf({ ...surface, words: [{ value: 0, label: 'a' }, { value: 0.3, label: 'b' }, { value: 1, label: 'c' }] });
  assert.equal(three.step, 0.01);
});

test('a layer’s colour map: its own once changed, else the profile’s, reversed where low is better', () => {
  const [score, surface] = theme.layers;
  assert.equal(scaleOf(BASE, score).palette, 'viridis_r', 'the finest spacing at the ramp’s bright end');
  assert.equal(scaleOf(BASE, surface).palette, 'viridis');
  assert.equal(scaleOf({ ...BASE, colours: { ...COLOURS, palette: 'magma_r' } }, score).palette, 'magma');
  const own = { ...COLOURS, palette: 'turbo', equalize: true };
  assert.equal(scaleOf({ ...BASE, scales: { score: own } }, score), own);
  // The highlight's value on the layer's own axis: the profile's 0 is 1 m on a log layer, off the
  // year's axis (its middle instead).
  const year = theme.layers[3];
  assert.equal(scaleOf(BASE, score).threshold.value, 0);
  assert.equal(scaleOf(BASE, year).threshold.value, 2012);
  assert.equal(scaleOf({ ...BASE, scales: { year: { ...COLOURS, threshold: { on: true, dir: 'below', value: 2010 } } } }, year).threshold.value, 2010);
});

test('the ramp’s stops: none clear, the range’s colours clamped past it, the highlight’s edge sharp', () => {
  const at = (v: number) => `c${v.toFixed(4)}`;
  const off = { on: false, dir: 'above' as const, value: 0 };
  const s = rampStops([0, 1], false, at, off);
  assert.deepEqual(s.slice(0, 3), [[NONE, 'rgba(0,0,0,0)'], [NONE + 1, 'c0.0000'], [0, 'c0.0000']]);
  assert.equal(s.length, 2 + 33);
  assert.deepEqual(s.at(-1), [1, 'c1.0000']);
  // Every stop a whole 1/256th, ascending, none twice (the map packs them as the tiles).
  const narrow = rampStops([0.3, 0.34], true, at, off);
  for (let i = 1; i < narrow.length; i++) assert.ok(narrow[i][0] > narrow[i - 1][0]);
  assert.ok(narrow.every(([v]) => Number.isInteger(v * 256)));
  // Above 0.5: what's under it clear, the edge a 1/256th wide; below: the other way; above the
  // low end: from the range's start.
  const above = new Map(rampStops([0, 1], false, at, { on: true, dir: 'above', value: 0.5 }));
  assert.deepEqual([above.get(0.5 - 1 / 256), above.get(0.5), above.get(0.25), above.get(0.75)], ['rgba(0,0,0,0)', 'c0.5000', 'rgba(0,0,0,0)', 'c0.7500']);
  const below = new Map(rampStops([0, 1], false, at, { on: true, dir: 'below', value: 0.5 }));
  assert.deepEqual([below.get(0.5), below.get(0.5 + 1 / 256), below.get(0.25), below.get(1)], ['c0.5000', 'rgba(0,0,0,0)', 'c0.2500', 'rgba(0,0,0,0)']);
  const low = rampStops([0.25, 1], false, at, { on: true, dir: 'low', value: 9 });
  assert.equal(new Map(low).get(0.25 - 1 / 256), 'rgba(0,0,0,0)');
  assert.equal(low[1][1], 'rgba(0,0,0,0)', 'clamped below the range: clear too');
});

test('the URLs: tiles at the meta version, polar tiles, the click', () => {
  assert.equal(tileUrl('https://h', 'terrain', 'best', 'score', 'free', 'abc-123'), 'https://h/tiles/available/terrain/best/score/{z}/{x}/{y}?avail=free&v=abc-123');
  assert.equal(tileUrl('', 'terrain', 'best', 'year', 'all', ''), '/tiles/available/terrain/best/year/{z}/{x}/{y}?avail=all');
  assert.equal(polarTileUrl('', 's', 'terrain', 'best', 'score', 'free', 'v1', 2, 1, 3), '/tiles/polar-available/s/terrain/best/score/2/1/3?avail=free&v=v1');
  assert.equal(atUrl('terrain', 'best', 2.35, 48.85, 'all'), '/api/available/terrain/best?lng=2.350000&lat=48.850000&avail=all');
  // Ids from the server escaped.
  assert.equal(tileUrl('', 'terrain', 'best', 'a/b?c', 'free', 'v&1'), '/tiles/available/terrain/best/a%2Fb%3Fc/{z}/{x}/{y}?avail=free&v=v%261');
  // The layers meta versions, each set's (codecs::available::layer).
  assert.equal(layerName('terrain', 'best'), 'available-terrain');
  assert.equal(layerName('terrain', 'built'), 'available-built-terrain');
  assert.equal(layerName('terrain', 'built', 's'), 'available-built-terrain-s');
  assert.equal(layerName('terrain', 'best', 'n'), 'available-terrain-n');
  assert.ok(isBuilt('built') && !isBuilt('best'));
});

/** The click at Paris (shortened from the server's answer). */
const PARIS = {
  lng: 2.35, lat: 48.85, avail: 'all',
  pieces: [
    { id: 'ign-lidar-hd/mnt/year=2023', source: 'ign-lidar-hd', piece: 'mnt/year=2023', name: 'LiDAR HD MNT 0.5 m', source_name: 'IGN LiDAR HD', credit: 'IGN, LiDAR HD',
      licence: 'Licence Ouverte 2.0', licence_url: 'https://www.etalab.gouv.fr/licence-ouverte-open-licence/', link: 'https://www.data.gouv.fr/', availability: 'open', extent: 'index',
      read: { method: 'wfs', tolerance_m: 50, date: '2026-11-13' }, metrics: { spacing_m: 0.5, surface: 'ground', year: 2023 }, stated: null,
      values: { spacing_m: 0.825, surface: 1, vertical_rmse_m: null, year: 0.885 }, score: 0.828 },
    { id: 'cop-dem/glo-30', source: 'cop-dem', piece: 'glo-30', name: 'GLO-30', source_name: 'Copernicus DEM', licence: 'Copernicus DEM licence', licence_url: null, link: 'https://x/',
      availability: 'open', extent: 'index', read: { method: 'listing', grain_m: 111320, tolerance_m: 10, date: '2026-11-13' },
      metrics: { spacing_m: 30, surface: 'surface', vertical_rmse_m: 1.68, year: 2015 }, stated: null,
      values: { spacing_m: 0.381, surface: 0, vertical_rmse_m: 0.337, year: 0.577 }, score: 0.378 },
    { id: 'jaxa-aw3d30/aw3d30', source: 'jaxa-aw3d30', piece: 'aw3d30', name: 'AW3D30', source_name: 'ALOS World 3D', licence: 'JAXA terms', licence_url: null, link: null,
      availability: 'login', extent: 'claimed', read: null, metrics: { spacing_m: 30, year: 2011, stated_class: 'x' }, stated: { spacing_m: '1 arc-second' },
      values: { spacing_m: 0.381, year: 0.423 }, score: 0.376 },
  ],
  // (The piece each layer paints there: the server's pick.)
  painted: { score: 'ign-lidar-hd/mnt/year=2023', vertical_rmse_m: 'cop-dem/glo-30', year: 'ign-lidar-hd/mnt/year=2023' },
};

test('the click’s rows: best first, the painted one marked, metrics with units, plain words', () => {
  const ps = parsePieces(PARIS)!;
  const rows = pieceRows(ps, 'score', theme);
  assert.deepEqual(rows.map((r) => r.painted), [true, false, false]);
  assert.equal(rows[0].title, 'IGN LiDAR HD');
  assert.equal(rows[0].sub, 'LiDAR HD MNT 0.5 m');
  assert.deepEqual(rows[0].metrics, [['Grid spacing', '0.5 m'], ['Bare earth or surface', 'ground'], ['Year surveyed', '2023']]);
  assert.equal(rows[0].availability, 'downloads freely');
  assert.equal(rows[0].extent, 'from the publisher’s tile index');
  assert.equal(rows[0].read, 'wfs · to 50 m · 2026-11-13');
  assert.deepEqual(rows[0].licence, { text: 'Licence Ouverte 2.0', url: 'https://www.etalab.gouv.fr/licence-ouverte-open-licence/' });
  assert.equal(rows[0].score, '0.83');
  assert.deepEqual(rows[1].metrics.at(2), ['Stated vertical accuracy (RMSE)', '1.68 m']);
  assert.equal(rows[1].read, 'listing · cells of 111 km · 2026-11-13');
  // A class as published, the claimed area, a value no metric names left out.
  assert.deepEqual(rows[2].metrics[0], ['Grid spacing', '30 m (published as 1 arc-second)']);
  assert.equal(rows[2].extent, 'claimed area, not checked against the data');
  assert.equal(rows[2].availability, 'free after a login or a request');
  assert.equal(rows[2].metrics.length, 2);
  assert.deepEqual(parsePieces({ pieces: [] }), []);
  assert.equal(parsePieces({}), null);
});

test('the piece a layer paints: the server’s pick, none where it names none', () => {
  const ps = parsePieces(PARIS)!;
  assert.deepEqual(pieceRows(ps, 'vertical_rmse_m', theme).map((r) => r.painted), [false, true, false]);
  assert.deepEqual(pieceRows(ps, 'surface', theme).map((r) => r.painted), [false, false, false]);
  // A server not naming them (before `painted` was sent), or naming a piece not listed: none marked.
  assert.deepEqual(pieceRows(parsePieces({ ...PARIS, painted: undefined })!, 'score', theme).map((r) => r.painted), [false, false, false]);
  assert.deepEqual(pieceRows(parsePieces({ ...PARIS, painted: { score: 'gone', year: 3 } })!, 'score', theme).map((r) => r.painted), [false, false, false]);
  // Without the theme's words: the ids as they are.
  const bare = { ...theme, labels: { sets: {}, availability: {}, extent: {} } };
  assert.deepEqual(pieceRows(ps, 'score', bare).map((r) => [r.availability, r.extent]), [['open', 'index'], ['open', 'index'], ['login', 'claimed']]);
  // Malformed pieces: those without an id left out, fields of the wrong kind empty.
  const odd = parsePieces({ pieces: [3, null, { source: 'x' }, { id: 'a', source: 5, metrics: [], values: { year: 'x' }, score: 'high', read: 'no' }] })!;
  assert.equal(odd.length, 1);
  assert.deepEqual([odd[0].source, odd[0].metrics, odd[0].values, odd[0].score, odd[0].read], ['', {}, { year: null }, null, null]);
  assert.deepEqual(pieceRows(odd, 'score', theme)[0].metrics, []);
});

test('the settings in a link, and from a save', () => {
  const s: AvailableState = { ...BASE, on: true, layer: 'year', avail: 'all', opacity: 0.6 };
  assert.equal(toField(s), '1,terrain,year,best,all,0.6');
  assert.deepEqual(fromField(toField(s), BASE), s);
  assert.deepEqual(fromField('1,terrain,year', BASE), BASE, 'the wrong shape: ignored whole');
  assert.deepEqual(fromField('1,Terrain!,year,best,maybe,7', BASE), { ...BASE, on: true, layer: 'year', opacity: 1 });
  // (The link leaves the colour maps as they are.)
  const mine = { ...BASE, scales: { year: { ...COLOURS, palette: 'turbo' } } };
  assert.equal(fromField(toField(s), mine).scales, mine.scales);
  assert.deepEqual(cleanState({ ...s, avail: 'x' as 'free', opacity: 0, layer: 3 as unknown as string }, BASE), { ...s, avail: 'free', opacity: 0.1, layer: 'score' });
  // A save or a link from before free data took in the login pieces says `open`: free data, whatever
  // the base's view.
  const all = { ...BASE, avail: 'all' as const };
  assert.equal(cleanState({ ...s, avail: 'open' as 'free' }, all).avail, 'free');
  assert.equal(fromField('1,terrain,year,best,open,0.6', all).avail, 'free');
  assert.equal(fromField('1,terrain,year,best,free,0.6', all).avail, 'free');
  // Colour maps from a save: each field valid, else the base's (a layer's, the save's first one's).
  const odd = cleanState({ ...s, colours: { ...COLOURS, lowFade: 3, fit: [50, 10] } as ScaleFields,
    scales: { year: { palette: 'turbo', range: [5, 2], threshold: { on: true, dir: 'sideways', value: 'x' } }, 'Bad!': COLOURS } as unknown as AvailableState['scales'] }, BASE);
  assert.deepEqual(odd.colours, { ...COLOURS, lowFade: 1 });
  assert.deepEqual(odd.scales, { year: { ...COLOURS, lowFade: 1, palette: 'turbo', threshold: { on: true, dir: 'above', value: 0 } } });
  assert.deepEqual(cleanState({ ...s, scales: 3 } as unknown as AvailableState, BASE).scales, {});
});
