// node web/test/regionlist.test.ts (tools/dev/check.sh's web part): src/regionlist.ts, the Regions
// list's sorting, filters, folders' countries and areas.
import assert from 'node:assert/strict';
import { areaText, countryCode, entries, filter, folderCountry, km2, norm } from '../src/regionlist.ts';

const reg = (id: string, name: string) => ({ id, name, outline: [] });
const regions = [reg('azores', 'Azores'), reg('madeira', 'Madeira'), reg('alabama', 'Alabama'), reg('alaska', 'Alaska'), reg('nuble', 'Ñuble'), reg('bermuda', 'Bermuda'), reg('baker-island', 'Baker Island')];
const byId = new Map(regions.map((r) => [r.id, r]));
const f = (path: string, name: string, ids: string[], folders: ReturnType<typeof f>[] = [], order?: number) => ({ path, name, regions: ids, folders, ...(order === undefined ? {} : { order }) });
const umi = f('united-states/umi', 'U.S. Minor Outlying Islands', ['baker-island']);
const us = f('united-states', 'United States', ['alabama', 'alaska'], [umi]);
const pt = f('portugal', 'Portugal', ['azores', 'madeira']);
const root = f('', '', ['nuble', 'bermuda'], [us, pt]);

// Folders sorted among the regions by name; an ordered folder first.
assert.deepEqual(entries(root, byId).map((e) => (e.kind === 'region' ? e.r.name : `[${e.f.name}]`)), ['Bermuda', 'Ñuble', '[Portugal]', '[United States]']);
assert.deepEqual(entries(f('', '', ['nuble'], [us, f('x', 'Zed', [], [], 1)]), byId).map((e) => (e.kind === 'region' ? e.r.name : e.f.name)), ['Zed', 'Ñuble', 'United States']);
assert.deepEqual(entries(f('', '', ['gone', 'azores'], []), byId).map((e) => e.kind), ['region'], 'ids the list lacks left out');

const all = () => true;
// No query: everything, nothing forced open.
let s = filter(root, byId, '', all);
assert.equal(s.regions.size, regions.length);
assert.equal(s.open.size, 0);
// A region's name: it, its folders shown and opened.
s = filter(root, byId, 'baker', all);
assert.deepEqual([...s.regions], ['baker-island']);
assert.deepEqual([...s.folders].sort(), ['', 'united-states', 'united-states/umi']);
assert.deepEqual([...s.open].sort(), ['', 'united-states', 'united-states/umi']);
// Accents off.
assert.deepEqual([...filter(root, byId, 'nub', all).regions], ['nuble']);
assert.equal(norm('Ñuble'), 'nuble');
// A folder's name: all its contents kept, subfolders' too; the folder itself not forced open.
s = filter(root, byId, 'united states', all);
assert.deepEqual([...s.regions].sort(), ['alabama', 'alaska', 'baker-island']);
assert.ok(!s.open.has('united-states') && !s.open.has('united-states/umi'));
// Downloaded only: through a folder's name match too.
s = filter(root, byId, 'portugal', (id) => id === 'madeira');
assert.deepEqual([...s.regions], ['madeira']);
s = filter(root, byId, '', (id) => id === 'alaska');
assert.deepEqual([...s.regions], ['alaska']);
assert.deepEqual([...s.folders].sort(), ['', 'united-states']);
assert.equal(s.open.size, 0, 'the downloaded filter alone opens nothing');
assert.equal(filter(root, byId, 'zzz', all).folders.size, 0);

// A region's country (its folder's flag): a country's or territory's own code; else the country
// its area lies in; else its geometry's; none.
assert.equal(countryCode({ iso: 'PT', country: true }), 'PT');
assert.equal(countryCode({ iso: 'GB-SCT', in: 'GB' }), 'GB');
assert.equal(countryCode({ iso: 'US-AL', in: 'us' }), 'US');
assert.equal(countryCode({ geo: 'GL' }), 'GL');
assert.equal(countryCode({}), null);
// A folder's country: its own regions' by land; all its regions' without any of its own; none.
const realm = f('dk', 'Danish Realm', ['denmark', 'faroe'], [f('dk/gl', 'Greenland', ['kujalleq'])]);
const cc: Record<string, string> = { denmark: 'DK', faroe: 'FO', kujalleq: 'GL', 'gl-waters': 'GL' };
const land: Record<string, number> = { denmark: 43_000, faroe: 1_400, kujalleq: 2e6 };
assert.equal(folderCountry(realm, (id) => cc[id] ?? null, (id) => land[id] ?? 0), 'DK');
assert.equal(folderCountry(realm.folders[0], (id) => cc[id] ?? null, () => 0), 'GL');
assert.equal(folderCountry(f('x', 'X', ['q']), () => null, () => 1), null);
// (A region's own flag, and the placeholder for one with none of its own, are flags.ts's: nothing
// is passed down a folder.)

// Areas in words, compact (metric).
assert.equal(km2(0.4), '<1 km²');
assert.equal(km2(5), '5 km²');
assert.equal(km2(9212), '9,212 km²');
assert.equal(km2(92211.6), '92.2k km²');
assert.equal(km2(667_676), '668k km²');
assert.equal(km2(999_700), '1M km²');
assert.equal(km2(1_234_000), '1.23M km²');
assert.equal(km2(10_130_000), '10.1M km²');
assert.equal(km2(17_098_246), '17.1M km²');
assert.equal(areaText(667_676, 10_130_000), '668k km² land · 10.1M km² water');
assert.equal(areaText(5, 0), '5 km² land');
assert.equal(areaText(0, 12), '12 km² water');
assert.equal(areaText(undefined, undefined), '');
console.log('regionlist: ok');
