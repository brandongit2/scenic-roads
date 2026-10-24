// node --test tools/check/worksums.test.mjs: the build page's sums (web/work/sums.js): the
// headline's share of the whole build's work, the schedule's lanes worth a row, its past jobs,
// slots and Macs, and the wait while this Mac hasn't read the build's state.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { busyLanes, finishedTiles, inRings, mapTilesKept, regionWork, regionsShown, rowsWindow, stageSegments, stepTotals, isSlotOf, laneGroups, macSlot, notRead, packRuns, pastFrom, pastRuns, READ_RETRY_MS, READ_WAIT_MS, runningTiles, slotName, slotNo, targetsOf, tileBox, tileCovers, tileOf, wholeProgress } from '../../web/work/sums.js';

test('map: tiles from targets, their squares', () => {
  assert.deepEqual(tileOf('6/10/15'), { z: 6, x: 10, y: 15 });
  assert.equal(tileOf('catalog'), null);
  assert.equal(tileOf('3/8/0'), null);
  assert.deepEqual(tileBox({ z: 3, x: 1, y: 2 }), [125, 250, 125]);
});

test('map: a job\'s targets from its start or lease event, else its words', () => {
  const evs = [{ kind: 'start', step: 'pack', t: 100, targets: ['6/1/1', '6/1/2'] }, { kind: 'lease', lease: 7, targets: ['6/2/2'] }];
  assert.deepEqual(targetsOf({ step: 'pack', id: 'pack 6/1/1', started: 103 }, evs), ['6/1/1', '6/1/2']);
  assert.deepEqual(targetsOf({ step: 'pack', id: 'pack 6/1/1', started: 200 }, evs), ['6/1/1']);
  assert.deepEqual(targetsOf({ lease: 7, what: 'x' }, evs), ['6/2/2']);
  assert.deepEqual(targetsOf({ lease: 8, what: 'unit 6/3/3 6/3/4' }, evs), ['6/3/3', '6/3/4']);
});

test('map: running tiles filled from live, else the beat\'s frac', () => {
  const live = runningTiles([{ step: 'unit', targets: ['6/1/1', '6/1/2', '6/1/3'], live: { targets: 3, targets_done: 1, target: '6/1/2', target_since: 940 } }], { unit: 120 }, 1000);
  assert.deepEqual([...live].map(([k, v]) => [k, v.frac, v.current]), [['unit 6/1/1', 1, false], ['unit 6/1/2', 0.5, true], ['unit 6/1/3', 0, false]]);
  const capped = runningTiles([{ step: 'unit', targets: ['6/1/2'], live: { targets: 1, target: '6/1/2', target_since: 0 } }], { unit: 10 }, 1000);
  assert.equal(capped.get('unit 6/1/2').frac, 0.95);
  const beat = runningTiles([{ step: 'unit', targets: ['6/1/1', '6/1/2', 'x'], frac: 0.75 }], {}, 0);
  assert.deepEqual([...beat].map(([k, v]) => [k, v.frac]), [['unit 6/1/1', 1], ['unit 6/1/2', 0.5]]);
});

test('map: finished tiles in the window, the last end each', () => {
  const f = finishedTiles([
    { kind: 'end', step: 'pack', t: 50, targets: ['6/1/1'] },
    { kind: 'end', step: 'pack', t: 150, targets: ['6/1/1', 'catalog'] },
    { kind: 'done', step: 'unit', t: 160, ok: false, targets: ['6/2/2'] },
    { kind: 'start', step: 'unit', t: 170, targets: ['6/3/3'] },
  ], 100);
  assert.deepEqual([...f].map(([k, v]) => [k, v.t]), [['pack 6/1/1', 150]]);
});

test('map: the filters keep tiles by kind, published, member, stage and zoom (#615)', () => {
  const running = new Map([['unit 6/1/1', { step: 'unit', frac: 0, run: { mac: 'A' } }], ['unit 6/1/2', { step: 'unit', frac: 0.4, current: true, run: { mac: 'A' } }], ['slope 6/1/3', { step: 'slope', frac: 1, run: { mac: 'B #2' } }]]);
  const done = new Map([['pack 3/0/0', { t: 100, step: 'pack', worker: 'B' }], ['unit 6/2/2', { t: 300, step: 'unit', worker: 'A' }], ['unit 6/1/2', { t: 50, step: 'unit', worker: 'A' }]]);
  const all = { unstarted: true, progress: true, done: true };
  const keys = ([r, f]) => [[...r.keys()].sort(), [...f.keys()].sort()];
  assert.deepEqual(keys(mapTilesKept(running, done, all)), [['slope 6/1/3', 'unit 6/1/1', 'unit 6/1/2'], ['pack 3/0/0', 'unit 6/2/2']]);
  assert.deepEqual(keys(mapTilesKept(running, done, { ...all, unstarted: false, done: false })), [['unit 6/1/2'], []]);
  assert.deepEqual(keys(mapTilesKept(running, done, { ...all, published: true }, { pubAt: 200 })), [[], ['pack 3/0/0']]);
  const macOf = (w) => w.split(' #')[0];
  assert.deepEqual(keys(mapTilesKept(running, done, { ...all, member: 'B' }, { macOf })), [['slope 6/1/3'], ['pack 3/0/0']]);
  // (By stage: the unchecked stages' tiles go; none unchecked, all stay.)
  const stageOf = (s) => (s === 'unit' ? 'roads' : s === 'slope' ? 'terrain' : 'out');
  assert.deepEqual(keys(mapTilesKept(running, done, { ...all, offStages: ['terrain', 'out'] }, { stageOf })), [['unit 6/1/1', 'unit 6/1/2'], ['unit 6/2/2']]);
  assert.deepEqual(keys(mapTilesKept(running, done, { ...all, offStages: [] }, { stageOf })), keys(mapTilesKept(running, done, all)));
  // (Tile areas: z3 alone hides every z6 tile, and z6 every z3.)
  assert.deepEqual(keys(mapTilesKept(running, done, { ...all, zoom: '3' })), [[], ['pack 3/0/0']]);
  assert.deepEqual(keys(mapTilesKept(running, done, { ...all, zoom: '6' }))[1], ['unit 6/2/2']);
  assert.equal(tileCovers({ z: 1, x: 1, y: 0 }), '0.0°–85.1° N, 0.0°–180.0° E');
});

test('map: two steps on one tile are two entries, running and finished (#615)', () => {
  const runs = [{ step: 'marks', targets: ['3/1/2'], frac: 0.5 }, { step: 'bldg', targets: ['3/1/2'], frac: 0 }];
  assert.deepEqual([...runningTiles(runs, {}, 0).keys()].sort(), ['bldg 3/1/2', 'marks 3/1/2']);
  const ends = [{ kind: 'end', t: 10, step: 'marks', targets: ['3/1/2'] }, { kind: 'done', t: 20, step: 'bldg', targets: ['3/1/2'] }];
  const f = finishedTiles(ends, 0);
  assert.deepEqual([...f].map(([k, v]) => [k, v.step]).sort(), [['bldg 3/1/2', 'bldg'], ['marks 3/1/2', 'marks']]);
  assert.deepEqual(tileOf('marks 3/1/2'), { z: 3, x: 1, y: 2 });
});

test('map: a point in rings, nonzero', () => {
  const sq = [0, 0, 10, 0, 10, 10, 0, 10, 0, 0];
  assert.equal(inRings([sq], 5, 5), true);
  assert.equal(inRings([sq], 15, 5), false);
});

const line = (step, done, total) => ({ steps: [step], done, total, left: total - done, items: [{ step, done, total, left: total - done }] });

test('headline: the work done of the whole, each step weighed by its time a target', () => {
  // Terrain all done (10 × 60 s), roads 58 of 284 (× 300 s), the rest of roads left in the forecast.
  const lines = [line('terrain', 10, 10), line('unit', 58, 284)];
  const fc = { each_s: { terrain: 60, unit: 300 }, steps: [{ step: 'unit', work_s: 226 * 300 }] };
  const p = wholeProgress(lines, fc);
  assert.equal(p.done_s, 600 + 58 * 300);
  assert.equal(p.left_s, 226 * 300);
  assert.equal(p.pct, Math.floor(((600 + 17400) / (600 + 17400 + 67800)) * 100));
});

test('headline: never 100% while any stage has work left', () => {
  // The roads all built, a cheap stage still to come: 99%, not 100.
  const lines = [line('unit', 1000, 1000), line('marks', 0, 1)];
  const fc = { each_s: { unit: 1000, marks: 1 }, steps: [{ step: 'marks', work_s: 1 }] };
  assert.equal(wholeProgress(lines, fc).pct, 99);
  // A stage not finished though the forecast lists nothing left: still not 100.
  assert.equal(wholeProgress([line('unit', 5, 10)], { each_s: { unit: 1 }, steps: [] }).pct, 99);
});

test('headline: 100% when every stage is done and nothing is left', () => {
  const lines = [line('unit', 10, 10), line('marks', 1, 1)];
  assert.equal(wholeProgress(lines, { each_s: { unit: 5, marks: 1 }, steps: [] }).pct, 100);
});

test('headline: none without a forecast that weighs the steps', () => {
  assert.equal(wholeProgress([line('unit', 1, 2)], null), null);
  assert.equal(wholeProgress([line('unit', 1, 2)], { steps: [] }), null);
});

test('lanes: only those with a bar in the window, in their order', () => {
  const lanes = {
    'm4': [{ from: 100, until: 200 }],
    'm4 #2': [],
    'm4 #3': [{ from: 10, until: 50 }], // ended before the window
    'm1': [{ from: 900, until: 1200 }], // starts in it
    'm1 #2': [{ from: 5000, until: 6000 }], // after it
  };
  assert.deepEqual(busyLanes(['m4', 'm4 #2', 'm4 #3', 'm1', 'm1 #2', 'gone'], lanes, 60, 1000), ['m4', 'm1']);
});

test('not read yet: a quiet state and a retry every 2 s, an error after a minute', () => {
  const a = notRead(false, 1000, 3000);
  assert.equal(a.error, null);
  assert.equal(a.retryMs, READ_RETRY_MS);
  assert.equal(a.state, "reading the build's state…");
  assert.match(notRead(true, 0, READ_WAIT_MS - 1).state, /stale/);
  const z = notRead(true, 0, READ_WAIT_MS);
  assert.equal(z.retryMs, null);
  assert.equal(z.state, null);
  assert.match(z.error, /hasn't read the build's state in 60 s/);
  // (Counted from the first 503 in a row: still an error, no quick retry, minutes on.)
  const later = notRead(false, 0, 5 * READ_WAIT_MS);
  assert.equal(later.retryMs, null);
  assert.match(later.error, /in 300 s/);
});

test("slots: a worker's slot of a host, and a job's lane by its slot index", () => {
  assert.equal(slotNo('m4', 'm4'), 1);
  assert.equal(slotNo('m4 #3', 'm4'), 3);
  assert.equal(slotNo('m4 #1', 'm4'), null); // (#1 is the bare name)
  assert.equal(slotNo('m4x', 'm4'), null);
  assert.equal(slotNo('m4 #2x', 'm4'), null);
  assert.ok(isSlotOf('m4 #2', 'm4') && !isSlotOf('m1', 'm4'));
  assert.equal(slotName('m4', 0), 'm4');
  assert.equal(slotName('m4', 2), 'm4 #3');
});

test("past jobs: each job's end since `from`, by start, once each", () => {
  const ev = [
    { kind: 'end', seq: 5, t: 500, secs: 100, step: 'unit', worker: 'm4 #2', targets: ['a', 'b'], ok: true, what: 'Roads: 2 tiles' },
    { kind: 'start', seq: 6, t: 510, step: 'unit', worker: 'm4' },
    { kind: 'end', seq: 7, t: 300, secs: 250, step: 'depth', worker: 'm1', targets: ['c'], ok: false },
    { kind: 'end', seq: 5, t: 500, secs: 100, step: 'unit', worker: 'm4 #2', targets: ['a', 'b'] }, // the same again
    { kind: 'end', seq: 8, t: 90, secs: 10, step: 'unit', worker: 'm4' }, // before `from`
    { kind: 'end', seq: 9, t: 600, step: 'unit', worker: 'm4' }, // no time
  ];
  const r = pastRuns(ev, 100);
  assert.deepEqual(r.map((x) => [x.lane, x.step, x.from, x.until, x.n, x.ok]), [['m1', 'depth', 50, 300, 1, false], ['m4 #2', 'unit', 400, 500, 2, true]]);
});

test("past jobs: a worker's leased jobs too (done, fail), each on its Mac's first lane free", () => {
  const ev = [
    { kind: 'done', seq: 1, t: 200, secs: 100, step: 'pois', worker: 'm4', targets: ['a'], ok: true },
    { kind: 'fail', seq: 2, t: 250, secs: 100, step: 'unit', worker: 'm4', targets: ['b'], ok: false },
    { kind: 'done', seq: 3, t: 400, secs: 150, step: 'pois', worker: 'm4', targets: ['c'], ok: true },
    { kind: 'lapse', seq: 4, t: 400, step: 'pois', worker: 'm4', targets: ['d'] }, // no time: not a run
  ];
  const r = pastRuns(ev, 0, ['m4']);
  assert.deepEqual(r.map((x) => [x.lane, x.from, x.ok]), [['m4', 100, true], ['m4 #2', 150, false], ['m4', 250, true]]);
});

test("a Mac's lanes one group whatever names it went by", () => {
  const hosts = ['Mac-mini-de-Brandon', 'Brandons-MacBook-Pro'];
  assert.deepEqual(macSlot('Mac-mini #3', hosts), { host: 'Mac-mini-de-Brandon', slot: 3 });
  assert.deepEqual(macSlot('Mac-mini-de-Brandon', hosts), { host: 'Mac-mini-de-Brandon', slot: 1 });
  assert.deepEqual(macSlot('Brandons-MacBook-Pro (second job)', hosts), { host: 'Brandons-MacBook-Pro', slot: 2 });
  assert.deepEqual(macSlot('MacBook-Pro-de-Brandon #2', hosts), { host: 'MacBook-Pro-de-Brandon', slot: 2 }); // gone
  const ev = [
    { kind: 'end', seq: 1, t: 100, secs: 10, step: 'unit', worker: 'Mac-mini #2' },
    { kind: 'end', seq: 2, t: 200, secs: 10, step: 'unit', worker: 'Mac-mini-de-Brandon #2' },
  ];
  const names = [...new Set(pastRuns(ev, 0, hosts).map((x) => x.lane))];
  assert.deepEqual(laneGroups(names, hosts), [{ host: 'Mac-mini-de-Brandon', lanes: ['Mac-mini-de-Brandon #2'] }]);
});

test('past jobs: runs of one step closer than a pixel packed into one segment', () => {
  const run = (step, from, until, n = 1, ok = true) => ({ step, from, until, n, ok });
  const p = packRuns([run('unit', 0, 10), run('unit', 11, 20, 2), run('unit', 40, 50), run('depth', 50, 60, 1, false), run('depth', 60, 70)], 2);
  assert.deepEqual(p.map((x) => [x.step, x.from, x.until, x.n, x.ok, x.parts.length]), [['unit', 0, 20, 3, true, 2], ['unit', 40, 50, 1, true, 1], ['depth', 50, 70, 2, false, 2]]);
});

test("lanes grouped by Mac: the hosts in order, their slots by number, a gone Mac's after", () => {
  const g = laneGroups(['m1 #2', 'm4 #3', 'm4', 'old #2', 'm1', 'old'], ['m4', 'm1']);
  assert.deepEqual(g, [{ host: 'm4', lanes: ['m4', 'm4 #3'] }, { host: 'm1', lanes: ['m1', 'm1 #2'] }, { host: 'old', lanes: ['old', 'old #2'] }]);
  // (A host's name that's another's start isn't its.)
  assert.deepEqual(laneGroups(['m4x', 'm4'], ['m4']), [{ host: 'm4', lanes: ['m4'] }, { host: 'm4x', lanes: ['m4x'] }]);
});

test('past history: asked back a chunk at a time until it reaches the time', () => {
  assert.equal(pastFrom({ seq: 18000, t: 1000 }, 500), 12999);
  assert.equal(pastFrom({ seq: 300, t: 1000 }, 500), 0);
  assert.equal(pastFrom({ seq: 300, t: 400 }, 500), null);
  assert.equal(pastFrom({ seq: 1, t: 1000 }, 500), null);
  assert.equal(pastFrom(null, 500), null);
});

test('regions: work left by step in the table\'s order, timed, by stage', () => {
  const w = regionWork({ unit: 2, pack: 3, zz: 1, slope: 0 }, { unit: 100, pack: 2 }, ['pack', 'unit']);
  assert.deepEqual(w, [{ step: 'pack', k: 3, s: 6 }, { step: 'unit', k: 2, s: 200 }, { step: 'zz', k: 1, s: 0 }]);
  const stage = { pack: 'tiles', unit: 'roads', zz: 'roads' };
  assert.deepEqual(stageSegments(w, (s) => stage[s], ['roads', 'tiles']), [{ stage: 'roads', k: 3, s: 200 }, { stage: 'tiles', k: 3, s: 6 }]);
  assert.deepEqual(regionWork(null, {}, []), []);
});

test('regions: totals by step over regions', () => {
  assert.deepEqual(stepTotals([{ left: { pack: 2, unit: 1 } }, { left: { pack: 5, slope: 0 } }, {}]), { pack: 7, unit: 1 });
});

test('regions: found by folder, ordered to the map, by name, by time left', () => {
  const regs = [
    { id: 'b', name: 'Bravo', crumbs: ['Norway', 'Bravo'], rank: 1, map_at: 50, work: [{ step: 'pack', k: 1, s: 10 }] },
    { id: 'a', name: 'Alpha', crumbs: ['Canada', 'Alpha'], rank: 0, map_at: 90, work: [{ step: 'unit', k: 1, s: 500 }] },
    { id: 'c', name: 'Charlie', on_map: true, rank: 2, work: [] },
    { id: 'd', name: 'Delta', rank: 3, work: [{ step: 'pack', k: 2, s: 10 }] },
  ];
  const ids = (l) => l.map((r) => r.id);
  assert.deepEqual(ids(regionsShown([...regs], 'map', '')), ['b', 'a', 'd', 'c']);
  assert.deepEqual(ids(regionsShown([...regs], 'name', '')), ['a', 'b', 'c', 'd']);
  assert.deepEqual(ids(regionsShown([...regs], 'left', '')), ['a', 'd', 'b', 'c']);
  assert.deepEqual(ids(regionsShown([...regs], 'map', 'NORW')), ['b']);
  assert.deepEqual(ids(regionsShown([...regs], 'map', 'char')), ['c']);
});

test('regions: a virtual list\'s window', () => {
  assert.deepEqual(rowsWindow(0, 100, 20, 1000, 2), [0, 7]);
  assert.deepEqual(rowsWindow(400, 100, 20, 1000, 2), [18, 27]);
  assert.deepEqual(rowsWindow(400, 100, 20, 22, 2), [18, 22]);
  assert.deepEqual(rowsWindow(0, 100, 20, 0), [0, 0]);
});

test('settings: an answer lacking a setting shows "not set" for it alone, the others as said', async () => {
  const { macRows, nasRow } = await import('../../web/work/settings.js');
  const live = { mac: 'm1', in_use: { cores: 2, of: 10, set: false, default: 2 }, memory: { mb: 10240, total_mb: 16384, set: true, by: 'x' }, room: { bytes: null, free: null, disk: null } };
  assert.deepEqual(macRows(live).map((r) => r.now), ['2 of 10 cores', '10.0 GB', 'off']);
  assert.ok(macRows(live).every((r) => r.input));
  const rows = macRows({ mac: 'm1', memory: live.memory });
  assert.deepEqual(rows.map((r) => [r.key, r.now, !!r.input]), [['in_use', 'not set', false], ['memory', '10.0 GB', true], ['room', 'not set', false]]);
  assert.deepEqual(macRows({}).map((r) => r.now), ['not set', 'not set', 'not set']);
  assert.equal(nasRow({ bytes: 3 * 2 ** 30, free: null }).now, '3.0 GB');
});
