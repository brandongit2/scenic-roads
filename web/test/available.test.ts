// node web/test/available.test.ts (tools/dev/check.sh's web part): the data available layers' model
// (src/available.ts): the themes the catalogue lists, the server's theme description read, the
// legends drawn from it, the tiles' and the click's URLs, the click's rows and the piece a layer
// paints (the server's Theme::best), and the settings in a link.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  atUrl, cleanState, fromField, legendModel, parsePieces, parseTheme, pieceRows, polarTileUrl, shown, themesOf, tileUrl, toField,
  type AvailableState,
} from '../src/available.ts';

/** `/api/available/terrain` as crates/server/src/available.rs `described` writes it for the real
 * score.toml (shortened: three of eight classes). */
const THEME = {
  theme: 'terrain', sets: ['best'], primary: 'spacing_m', within: 1.25, share: 0.02,
  metrics: [
    { id: 'spacing_m', label: 'Grid spacing', unit: 'm', weight: null, scale: 'log', worst: 1000, best: 0.1, values: null, primary: true },
    { id: 'surface', label: 'Bare earth or surface', unit: null, weight: 0.4, scale: 'values', worst: null, best: null, values: { ground: 1, surface: 0 }, primary: false },
    { id: 'vertical_rmse_m', label: 'Stated vertical accuracy (RMSE)', unit: 'm', weight: 0.4, scale: 'log', worst: 10, best: 0.05, values: null, primary: false },
    { id: 'year', label: 'Year surveyed', unit: null, weight: 0.2, scale: 'linear', worst: 2000, best: 2026, values: null, primary: false },
  ],
  layers: [
    { id: 'score', label: 'Best data', unit: 'm', legend: 'classes', classes_of: 'spacing_m', shade: 0.4, shaded_by: ['surface', 'vertical_rmse_m', 'year'],
      stops: [{ label: '0.5 m', upto: 0.625, colour: '#e7fda7', shaded: '#c8f6b3' }, { label: '1 m', upto: 1.25, colour: '#9aefba', shaded: '#7fe3c4' }, { label: 'coarser', upto: null, colour: '#803041', shaded: '#7a2040' }] },
    { id: 'spacing_m', label: 'Grid spacing', unit: 'm', legend: 'classes', stops: [{ label: '0.5 m', upto: 0.625, colour: '#e7fda7' }, { label: 'coarser', upto: null, colour: '#803041' }] },
    { id: 'surface', label: 'Bare earth or surface', unit: null, legend: 'values', stops: [{ value: 0, label: 'surface', colour: '#e69f00' }, { value: 1, label: 'ground', colour: '#56b4e9' }] },
    { id: 'year', label: 'Year surveyed', unit: null, legend: 'ramp', ramp: [[0, '#803041'], [0.5, '#4fb6df'], [1, '#e7fda7']],
      stops: [{ value: 0, label: '2000', colour: '#803041' }, { value: 1, label: '2026', colour: '#e7fda7' }] },
    { id: 'vertical_rmse_m', label: 'Stated vertical accuracy (RMSE)', unit: 'm', legend: 'ramp', ramp: [[0, '#803041'], [1, '#e7fda7']],
      stops: [{ value: 0, label: '10', colour: '#803041' }, { value: 1, label: '0.05', colour: '#e7fda7' }] },
    // (A legend the app doesn't know: left out.)
    { id: 'odd', label: 'Odd', legend: 'pie', stops: [] },
  ],
  sources: [],
  labels: {
    sets: { best: 'Best available anywhere' },
    availability: { open: 'downloads freely', login: 'free after a login or a request', paid: 'paid', closed: 'exists, not to be had' },
    extent: { index: 'from the publisher’s tile index', claimed: 'claimed area, not checked against the data' },
  },
};

const theme = parseTheme(THEME)!;

test('the catalogue’s themes, with their poles', () => {
  const layers = {
    'available-terrain': { maxzoom: 12 }, 'available-terrain-n': { maxzoom: 3 }, 'available-terrain-s': { maxzoom: 3 },
    'polar-terrain-s': { maxzoom: 8 }, roads: {}, 'available-bathymetry': { maxzoom: 10 },
  };
  assert.deepEqual(themesOf(layers), [{ theme: 'terrain', maxzoom: 12, poles: { n: 3, s: 3 } }, { theme: 'bathymetry', maxzoom: 10, poles: {} }]);
  assert.deepEqual(themesOf(undefined), []);
});

test('a theme read: its layers in the server’s order, one it can’t draw left out', () => {
  assert.deepEqual(theme.layers.map((l) => l.id), ['score', 'spacing_m', 'surface', 'year', 'vertical_rmse_m']);
  assert.equal(theme.layers[0].label, 'Best data');
  assert.deepEqual(theme.sets, ['best']);
  assert.equal(parseTheme({ theme: 'terrain', layers: [], sources: [] }), null, 'no score: no layers');
  assert.equal(parseTheme('nope'), null);
  assert.equal(theme.labels.sets.best, 'Best available anywhere');
  // A metric the app has never seen, drawn by a legend it knows: shown as any other; a server
  // without `labels` (before they were sent): no words, the ids shown as they are.
  const more = structuredClone(THEME) as Record<string, unknown> & typeof THEME;
  more.metrics.push({ id: 'density', label: 'Point density', unit: 'pt/m²', weight: 0.1, scale: 'log', worst: 0.1, best: 20, values: null, primary: false });
  (more.layers as unknown[]).push({ id: 'density', label: 'Point density', unit: 'pt/m²', legend: 'ramp', ramp: [[0, '#000000'], [1, '#ffffff']], stops: [{ value: 0, label: '0.1', colour: '#000000' }, { value: 1, label: '20', colour: '#ffffff' }] });
  delete more.labels;
  const m = parseTheme(more)!;
  assert.deepEqual(m.layers.at(-1)!.id, 'density');
  assert.deepEqual(legendModel(m.layers.at(-1)!, m), { kind: 'ramp', gradient: 'linear-gradient(90deg, #000000 0%, #ffffff 100%)', ticks: ['0.1 pt/m²', '20 pt/m²'] });
  assert.deepEqual(m.labels, { sets: {}, availability: {}, extent: {} });
  // A colour not #rrggbb: the layer is left out, not drawn wrong.
  const bad = structuredClone(THEME);
  bad.layers[1].stops[0].colour = 'red';
  assert.deepEqual(parseTheme(bad)!.layers.map((l) => l.id), ['score', 'surface', 'year', 'vertical_rmse_m']);
});

test('the layer and set shown: the state’s where the theme has them, else its first', () => {
  const s: AvailableState = { on: true, theme: 'terrain', layer: 'year', set: 'best', avail: 'open', opacity: 0.85 };
  assert.equal(shown(s, theme).layer.id, 'year');
  assert.equal(shown({ ...s, layer: 'gone', set: 'built' }, theme).layer.id, 'score');
  assert.equal(shown({ ...s, set: 'built' }, theme).set, 'best');
});

test('legends: classes finest first with the score’s shading, words, a ramp', () => {
  const score = legendModel(theme.layers[0], theme);
  assert.equal(score.kind, 'swatches');
  if (score.kind !== 'swatches') return;
  assert.deepEqual(score.rows[0], { colour: '#e7fda7', to: '#c8f6b3', label: '0.5 m' });
  assert.equal(score.rows.at(-1)!.label, 'coarser');
  assert.equal(score.caption, 'Within a class, left its best and right its worst by Bare earth or surface · Stated vertical accuracy (RMSE) · Year surveyed');
  const spacing = legendModel(theme.layers[1], theme);
  assert.deepEqual(spacing, { kind: 'swatches', rows: [{ colour: '#e7fda7', label: '0.5 m' }, { colour: '#803041', label: 'coarser' }], caption: '' });
  const words = legendModel(theme.layers[2], theme);
  assert.deepEqual(words, { kind: 'swatches', rows: [{ colour: '#e69f00', label: 'surface' }, { colour: '#56b4e9', label: 'ground' }], caption: '' });
  assert.deepEqual(legendModel(theme.layers[3], theme), { kind: 'ramp', gradient: 'linear-gradient(90deg, #803041 0%, #4fb6df 50%, #e7fda7 100%)', ticks: ['2000', '2026'] });
  // The ramp's labels in the metric's unit.
  const acc = legendModel(theme.layers[4], theme);
  assert.deepEqual(acc.kind === 'ramp' && acc.ticks, ['10 m', '0.05 m']);
});

test('the URLs: tiles at the meta version, polar tiles, the click', () => {
  assert.equal(tileUrl('https://h', 'terrain', 'best', 'score', 'open', 'abc-123'), 'https://h/tiles/available/terrain/best/score/{z}/{x}/{y}?avail=open&v=abc-123');
  assert.equal(tileUrl('', 'terrain', 'best', 'year', 'all', ''), '/tiles/available/terrain/best/year/{z}/{x}/{y}?avail=all');
  assert.equal(polarTileUrl('', 's', 'terrain', 'best', 'score', 'open', 'v1', 2, 1, 3), '/tiles/polar-available/s/terrain/best/score/2/1/3?avail=open&v=v1');
  assert.equal(atUrl('terrain', 'best', 2.35, 48.85, 'all'), '/api/available/terrain/best?lng=2.350000&lat=48.850000&avail=all');
  // Ids from the server escaped.
  assert.equal(tileUrl('', 'terrain', 'best', 'a/b?c', 'open', 'v&1'), '/tiles/available/terrain/best/a%2Fb%3Fc/{z}/{x}/{y}?avail=open&v=v%261');
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
  painted: { score: 'ign-lidar-hd/mnt/year=2023', spacing_m: 'ign-lidar-hd/mnt/year=2023', vertical_rmse_m: 'cop-dem/glo-30', year: 'ign-lidar-hd/mnt/year=2023' },
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
  const base: AvailableState = { on: false, theme: 'terrain', layer: 'score', set: 'best', avail: 'open', opacity: 0.85 };
  const s: AvailableState = { on: true, theme: 'terrain', layer: 'year', set: 'best', avail: 'all', opacity: 0.6 };
  assert.equal(toField(s), '1,terrain,year,best,all,0.6');
  assert.deepEqual(fromField(toField(s), base), s);
  assert.deepEqual(fromField('1,terrain,year', base), base, 'the wrong shape: ignored whole');
  assert.deepEqual(fromField('1,Terrain!,year,best,maybe,7', base), { ...base, on: true, layer: 'year', opacity: 1 });
  assert.deepEqual(cleanState({ ...s, avail: 'x' as 'open', opacity: 0, layer: 3 as unknown as string }, base), { ...s, avail: 'open', opacity: 0.1, layer: 'score' });
});
