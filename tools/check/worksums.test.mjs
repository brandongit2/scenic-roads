// node --test tools/check/worksums.test.mjs: the build page's sums (web/work/sums.js): the
// headline's share of the whole build's work, the schedule's lanes worth a row, its past jobs,
// slots and Macs, and the wait while this Mac hasn't read the build's state.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { areaOf, areaTargets, busyLanes, finishedTiles, inRings, mapTiles, queuedTiles, regionWork, regionsShown, rowsWindow, stageFill, stageSegments, stepTotals, isSlotOf, laneGroups, macSlot, notRead, packRuns, publishedBefore, pastFrom, pastRuns, runLinks, runNumbers, SCHED_MIN_SPAN, viewPan, viewRange, viewZoom, READ_RETRY_MS, READ_WAIT_MS, runningTiles, slotName, slotNo, startOf, targetsOf, tileBox, tileCovers, tileOf, topRegion, ringsArea, phaseBreakdown } from '../../web/work/sums.js';

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

test('map: every tile the keys record stays finished, whatever the window and events held (#674)', () => {
  // (Live, 26 Oct: terrain-deep's northeastern Ontario tiles ended 7–12 h before; in a 6 h window
  // they were gone, the newer central ones alone drawn.)
  const ends = [
    { kind: 'end', step: 'terrain-deep', t: 1000, worker: 'mini #3', targets: ['10/257/342', '10/257/343'] },
    { kind: 'done', step: 'terrain-deep', t: 9000, worker: 'm4', targets: ['10/264/360'] },
  ];
  const built = { 'terrain-deep': ['10/257/342', '10/257/343', '10/264/360', '10/100/100'] };
  assert.deepEqual([...finishedTiles(ends, 5000).keys()], ['terrain-deep 10/264/360'], 'the window alone');
  const f = finishedTiles(ends, 5000, built);
  assert.deepEqual([...f.keys()].sort(), ['terrain-deep 10/100/100', 'terrain-deep 10/257/342', 'terrain-deep 10/257/343', 'terrain-deep 10/264/360']);
  assert.deepEqual(f.get('terrain-deep 10/257/342'), { t: 1000, step: 'terrain-deep', worker: 'mini #3' }, 'its end held: its time, before the window too');
  assert.deepEqual(f.get('terrain-deep 10/100/100'), { t: null, before: null, step: 'terrain-deep', worker: null }, 'none held');
  // (The page's events gone, a new numbering: the recorded tiles stay.)
  assert.equal(finishedTiles([], 5000, built).size, 4);
  // (Finished, not merged into the keys yet: from its end.)
  assert.ok(finishedTiles([{ kind: 'done', step: 'terrain-deep', t: 9500, targets: ['10/1/1'] }], 5000, built).has('terrain-deep 10/1/1'));
  // (One with no end held finished before the oldest event held: published when that's by the
  // catalogue's time, else it can't be told and isn't claimed.)
  const held = finishedTiles(ends, 5000, built, 900);
  assert.equal(held.get('terrain-deep 10/100/100').before, 900);
  const both = (v) => new Map([...v.finished, ...v.published]);
  const fin = both(mapTiles(new Map(), new Map(), held, {}, { pubAt: 8000 }));
  assert.deepEqual([...fin].map(([k, v]) => [k.split(' ')[1], v.state]).sort(), [['10/100/100', 'published'], ['10/257/342', 'published'], ['10/257/343', 'published'], ['10/264/360', 'completed']]);
  assert.equal(mapTiles(new Map(), new Map(), finishedTiles(ends, 5000, built, 8500), {}, { pubAt: 8000 }).finished.get('terrain-deep 10/100/100').state, 'completed', 'held from after the catalogue');
  assert.equal(mapTiles(new Map(), new Map(), f, {}, { pubAt: 8000 }).finished.get('terrain-deep 10/100/100').state, 'completed', 'nothing held');
  assert.equal(mapTiles(new Map(), new Map(), f, {}, {}).finished.get('terrain-deep 10/100/100').state, 'completed', 'no catalogue: none published');
  assert.equal(mapTiles(new Map(), new Map(), f, { member: 'm4' }, {}).finished.size, 1, 'a member: the tiles its ends name');
});

test('map: a tile built with nothing to publish is its own state, hidden until shown (#839)', () => {
  // (Live, 29 Oct: 10/266/334 ran, found no fine data, recorded done, no pack: it had shown as
  // published with no catalogue.)
  const ends = [{ kind: 'end', step: 'terrain-deep', t: 1000, worker: 'mini #3', targets: ['10/266/334'] }];
  const built = { 'terrain-deep': ['10/266/334', '10/266/335', '10/266/336'] };
  const f = finishedTiles(ends, 5000, built, 900, { 'terrain-deep': [0, 2] });
  assert.deepEqual(f.get('terrain-deep 10/266/334'), { t: 1000, step: 'terrain-deep', worker: 'mini #3', none: true }, 'its end held, still none');
  assert.equal(f.get('terrain-deep 10/266/336').none, true);
  assert.equal(f.get('terrain-deep 10/266/335').none, undefined);
  const hidden = mapTiles(new Map(), new Map(), f, {}, { pubAt: 8000 });
  assert.deepEqual([...hidden.finished.keys()], [], 'empty: hidden by default');
  assert.deepEqual([...hidden.published.keys()], ['terrain-deep 10/266/335'], 'the one with a pack: published');
  const shown = mapTiles(new Map(), new Map(), f, { tiles: { empty: true } }, { pubAt: 8000 });
  assert.deepEqual([...shown.finished].map(([k, v]) => [k.split(' ')[1], v.state]).sort(), [['10/266/334', 'empty'], ['10/266/336', 'empty']], 'before the catalogue check');
  assert.equal(mapTiles(new Map(), new Map(), f, { tiles: { empty: true } }, { solo: ['slope-deep'] }).finished.size, 0, 'another step soloed');
});

test('map: the queue from the forecast, the earliest where two Macs name a tile (#685)', () => {
  const a = { at: 1000, macs: { m4: { unit: [['6/1/1', 60], ['norway', 5]], pack: [['3/0/0', 10]] }, mini: { unit: [['6/1/1', 30], ['6/1/2', 90]] } } };
  const q = queuedTiles(a);
  assert.deepEqual([...q.keys()].sort(), ['pack 3/0/0', 'unit 6/1/1', 'unit 6/1/2']);
  assert.deepEqual(q.get('unit 6/1/1'), { step: 'unit', at: 1030, mac: 'mini' });
  assert.equal(queuedTiles(null).size, 0);
});

test('map: the view keeps tiles by state, member, tile area and solo (#615, #627, #685)', () => {
  const running = new Map([['unit 6/1/1', { step: 'unit', frac: 0, run: { mac: 'A' } }], ['unit 6/1/2', { step: 'unit', frac: 0.4, current: true, run: { mac: 'A' } }], ['slope 6/1/3', { step: 'slope', frac: 1, run: { mac: 'B #2' } }]]);
  const queued = new Map([['unit 6/3/3', { step: 'unit', at: 5, mac: 'B' }], ['unit 6/2/2', { step: 'unit', at: 9, mac: 'A' }], ['unit 6/1/1', { step: 'unit', at: 1, mac: 'A' }]]);
  const done = new Map([['pack 3/0/0', { t: 100, step: 'pack', worker: 'B' }], ['unit 6/2/2', { t: 300, step: 'unit', worker: 'A' }], ['unit 6/1/2', { t: 50, step: 'unit', worker: 'A' }], ['unit 6/4/4', { t: 150, step: 'unit', worker: 'A' }]]);
  const keys = (v) => Object.fromEntries(Object.entries(v).map(([k, m]) => [k, [...m.keys()].sort()]));
  const states = (v) => [...v.running, ...v.queued, ...v.finished, ...v.published].map(([k, x]) => `${k}:${x.state}`).sort();
  // (Running first, then the queue (built before, to be built again: its finished tile not drawn), then finished.)
  assert.deepEqual(states(mapTiles(running, queued, done, {}, { pubAt: 200 })), ['pack 3/0/0:published', 'slope 6/1/3:completed', 'unit 6/1/1:queued', 'unit 6/1/2:progress', 'unit 6/2/2:queued', 'unit 6/3/3:queued', 'unit 6/4/4:published']);
  // (Published tiles always apart, drawn only for a solo (#833); queued off.)
  assert.deepEqual(keys(mapTiles(running, queued, done, { tiles: { queued: false } }, { pubAt: 200 })), { running: ['slope 6/1/3', 'unit 6/1/2'], queued: [], finished: [], published: ['pack 3/0/0', 'unit 6/4/4'] });
  // (A queued tile built before, whenever, published or not: `built` (#833, the page then asks the catalogues when it was last published); one never built, not.)
  for (const pubAt of [400, 200, null]) assert.deepEqual([...mapTiles(running, queued, done, {}, { pubAt }).queued].map(([k, v]) => `${k}:${!!v.built}`).sort(), ['unit 6/2/2:true', 'unit 6/3/3:false']);
  // (Published before (#833 review): a deep tile when a catalogue kept published its own packs,
  // record or not; a shallower one only when built too (its z6 tile's catalogue is any step's).)
  const at = (t) => (t.z === 10 && t.x === 259 ? { t: 7, n: 734 } : t.z === 6 ? { t: 5, n: 514 } : null);
  assert.deepEqual(publishedBefore('terrain-deep 10/259/349', {}, at), { t: 7, n: 734 });
  assert.equal(publishedBefore('terrain-deep 10/271/349', { built: true }, at), null);
  assert.deepEqual(publishedBefore('unit 6/2/2', { built: true }, at), { t: 5, n: 514 });
  assert.equal(publishedBefore('unit 6/3/3', {}, at), null);
  assert.equal(publishedBefore('worldwide', { built: true }, at), null);
  // (No catalogue yet: nothing published, every finish completed.)
  assert.deepEqual(states(mapTiles(new Map(), new Map(), done, {})), ['pack 3/0/0:completed', 'unit 6/1/2:completed', 'unit 6/2/2:completed', 'unit 6/4/4:completed']);
  const macOf = (w) => w.split(' #')[0];
  assert.deepEqual(keys(mapTiles(running, queued, done, { member: 'B' }, { macOf, pubAt: 200 })), { running: ['slope 6/1/3'], queued: ['unit 6/3/3'], finished: [], published: ['pack 3/0/0'] });
  // (A solo: its steps alone, its published tiles apart whatever the states shown.)
  const solo = mapTiles(running, queued, done, { tiles: { published: false } }, { pubAt: 200, solo: ['unit'] });
  assert.deepEqual(keys(solo), { running: ['unit 6/1/1', 'unit 6/1/2'], queued: ['unit 6/2/2', 'unit 6/3/3'], finished: [], published: ['unit 6/4/4'] });
  assert.deepEqual(keys(mapTiles(running, queued, done, {}, { pubAt: 200, solo: ['pack'] })), { running: [], queued: [], finished: [], published: ['pack 3/0/0'] });
  // (Tile areas, by the packs' zooms: hi off hides every z6 tile, lo off every z3.)
  assert.deepEqual(keys(mapTiles(running, queued, done, { offAreas: ['hi'] }, { pubAt: 200 })), { running: [], queued: [], finished: [], published: ['pack 3/0/0'] });
  assert.deepEqual(['0/0/0', '3/1/1', '6/1/1', '10/1/1', 'terrain-deep 10/5/5', '8/1/1'].map(areaOf), ['root', 'lo', 'hi', 'deep', 'deep', 'z8']);
  // (A root step's worldwide target is the root tile; others' as they are.)
  assert.deepEqual([areaTargets('terrain-root', ['terrain-root']), areaTargets('slope-root', []), areaTargets('unit', ['6/1/1'])], [['0/0/0'], ['0/0/0'], ['6/1/1']]);
  assert.deepEqual([...finishedTiles([{ kind: 'end', t: 5, step: 'slope-root', targets: ['slope-root'] }], 0).keys()], ['slope-root 0/0/0']);
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

test('map: the one region a point picks (#831): holes not hit, the innermost of nested ones', () => {
  const sq = (x, y, d, cw = false) => (cw ? [x, y, x, y + d, x + d, y + d, x + d, y, x, y] : [x, y, x + d, y, x + d, y + d, x, y + d, x, y]);
  const at = (regions, x, y) => topRegion(regions.filter((r) => inRings(r.rings, x, y)).map((r) => ({ id: r.id, area: ringsArea(r.rings) })));
  const island = { id: 'newfoundland', rings: [sq(40, 40, 20)] };
  // Waters with the island as a hole (wound the other way): the island alone is hit there.
  const holed = { id: 'canada-waters', rings: [sq(0, 0, 100), sq(40, 40, 20, true)] };
  assert.equal(inRings(holed.rings, 50, 50), false);
  assert.equal(at([holed, island], 50, 50), 'newfoundland');
  assert.equal(at([holed, island], 10, 10), 'canada-waters');
  // Waters drawn under it, no hole: the smaller wins, in either order.
  const plain = { id: 'canada-waters', rings: [sq(0, 0, 100)] };
  assert.equal(at([plain, island], 50, 50), 'newfoundland');
  assert.equal(at([island, plain], 50, 50), 'newfoundland');
  // An enclave (Lesotho in South Africa, holed), the Vatican in Italy.
  const za = { id: 'za', rings: [sq(0, 0, 100), sq(30, 30, 10, true)] }, ls = { id: 'ls', rings: [sq(30, 30, 10)] };
  assert.equal(at([za, ls], 35, 35), 'ls');
  const it = { id: 'it', rings: [sq(0, 0, 100)] }, va = { id: 'va', rings: [sq(50, 50, 0.1)] };
  assert.equal(at([it, va], 50.05, 50.05), 'va');
  assert.equal(at([it, va], 20, 20), 'it');
  assert.equal(at([za, ls], 200, 200), null);
  // Each ring counts whole whatever its winding: shapes wound opposite ways don't cancel.
  assert.equal(ringsArea([sq(0, 0, 10), sq(20, 0, 10, true)]), 200);
  const mixed = { id: 'waters', rings: [sq(0, 0, 100), sq(200, 0, 100, true)] };
  assert.equal(at([mixed, island], 50, 50), 'newfoundland');
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
    { kind: 'end', seq: 5, t: 500, secs: 100, step: 'unit', worker: 'm4 #2', targets: ['a', 'b'], ok: true },
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

test('past jobs: what each waited on, as its start or lease names it (none before #791)', () => {
  const ev = [
    { kind: 'lease', seq: 11, t: 90, lease: 4, step: 'terrain-hi', worker: 'm1', targets: ['6/1/1'] },
    { kind: 'done', seq: 12, t: 100, secs: 10, lease: 4, step: 'terrain-hi', worker: 'm1', targets: ['6/1/1'] },
    { kind: 'start', seq: 13, t: 110, step: 'unit', worker: 'm4', targets: ['6/1/1'], after: [12] },
    { kind: 'start', seq: 14, t: 111, step: 'depth', worker: 'm4 #2' },
    { kind: 'end', seq: 15, t: 200, secs: 90, step: 'unit', worker: 'm4', targets: ['6/1/1'] },
    { kind: 'end', seq: 16, t: 210, secs: 99, step: 'depth', worker: 'm4 #2', targets: ['6/1/1'] },
  ];
  // (Given out of order: the page's past ends before its live events.)
  const r = pastRuns([...ev].reverse(), 0, ['m1', 'm4']);
  assert.deepEqual(r.map((x) => [x.seq, x.after]), [[12, []], [15, [12]], [16, []]]);
  // An end whose start isn't held names none, not an earlier start's of its slot and step.
  const again = [...ev, { kind: 'end', seq: 17, t: 300, secs: 50, step: 'unit', worker: 'm4', targets: ['6/1/2'] }];
  assert.deepEqual(pastRuns(again, 0, ['m1', 'm4']).find((x) => x.seq === 17).after, []);
  // A running job's: the lead's own by its start (its slot's first), a member's by its lease.
  assert.deepEqual(startOf({ step: 'unit', id: 'unit 6/1/1', started: 112 }, ev)?.after, [12]);
  const two = [...ev, { kind: 'start', seq: 18, t: 112, step: 'unit', worker: 'm4 #2', targets: ['6/2/2'] }];
  assert.deepEqual(startOf({ step: 'unit', id: 'unit 6/1/1', started: 112, worker: 'm4' }, two)?.after, [12]);
  assert.equal(startOf({ lease: 4 }, ev)?.seq, 11);
  assert.equal(startOf({ step: 'terrain-hi', id: 'x', started: 95 }, ev), null);
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

test('regions: targets built of all by stage', () => {
  const stage = { terrain: 'terrain', 'terrain-lo': 'terrain', unit: 'roads' };
  assert.deepEqual(stageFill({ terrain: [3, 10], 'terrain-lo': [1, 2], unit: [0, 4] }, (s) => stage[s]), { terrain: [4, 12], roads: [0, 4] });
  assert.deepEqual(stageFill(undefined, (s) => stage[s]), {});
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
  const { macRows, poolRows } = await import('../../web/work/settings.js');
  const live = { mac: 'm1', in_use: { cores: 2, of: 10, set: false, default: 2 }, memory: { mb: 10240, total_mb: 16384, set: true, by: 'x' }, room: { bytes: null, free: null, disk: null }, battery_floor: { pct: 20, set: true, default: 30, own: true, by: 'x' } };
  assert.deepEqual(macRows(live).map((r) => r.now), ['2 of 10 cores', '10.0 GB', 'off', '20 %']);
  assert.ok(macRows(live).every((r) => r.input));
  const rows = macRows({ mac: 'm1', memory: live.memory });
  assert.deepEqual(rows.map((r) => [r.key, r.now, !!r.input]), [['in_use', 'not set', false], ['memory', '10.0 GB', true], ['room', 'not set', false], ['battery_floor', 'not set', false]]);
  assert.deepEqual(macRows({}).map((r) => r.now), ['not set', 'not set', 'not set', 'not set']);
  const older = macRows({ ...live, battery_floor: { pct: 20, set: true, default: 30, by: 'x' } }).find((r) => r.key === 'battery_floor');
  assert.deepEqual([older.now, older.input], ['20 %', null], "an older agent's floor, the pool's: shown, not set from here");
  const pool = poolRows({ nas_room: { bytes: 3 * 2 ** 30, free: null } });
  assert.deepEqual(pool.map((r) => [r.key, r.now, !!r.input]), [['nas_room', '3.0 GB', true]]);
  assert.deepEqual(poolRows({ nas_room: { bytes: null } }).map((r) => r.now), ['off']);
});

test("schedule's view: 12 h each side of now at first, held in the domain, zoomed about the pointer (#787)", () => {
  const now = 1000000, lo = now - 7 * 86400, hi = now + 2 * 86400;
  const v = { c: 0, span: 86400 };
  assert.deepEqual(viewRange(v, now, lo, hi), { t0: now - 43200, t1: now + 43200, span: 86400 });
  // Zoomed in twice about a quarter of the way: the time there stays there.
  const z = viewZoom(v, 2, 0.25, now, lo, hi), r = viewRange(z, now, lo, hi);
  assert.equal(r.span, 43200);
  assert.equal(r.t0 + 0.25 * r.span, now - 43200 + 0.25 * 86400);
  // Never narrower than the least span, never wider than the domain.
  assert.equal(viewRange(viewZoom(v, 1e9, 0.5, now, lo, hi), now, lo, hi).span, SCHED_MIN_SPAN);
  assert.equal(viewRange(viewZoom(v, 1e-9, 0.5, now, lo, hi), now, lo, hi).span, hi - lo);
  // Panned past either end: held at it.
  assert.equal(viewRange(viewPan(v, -30 * 86400, now, lo, hi), now, lo, hi).t0, lo);
  assert.equal(viewRange(viewPan(v, 30 * 86400, now, lo, hi), now, lo, hi).t1, hi);
  // The view moves with now: the same view a minute later is a minute later.
  assert.equal(viewRange(v, now + 60, lo + 60, hi).t0, now + 60 - 43200);
});

test("forecast runs: numbered by lane name in byte order, linked both ways (#787)", () => {
  const lanes = { m4: [{}, {}], "Mac-mini": [{}], "m4 #2": [{}] };
  assert.deepEqual(runNumbers(lanes), { first: { "Mac-mini": 0, m4: 1, "m4 #2": 3 }, total: 4 });
  const l = runLinks([[0, 2], [1, 2], [2, 3]]);
  assert.deepEqual(l.on.get(2), [0, 1]);
  assert.deepEqual(l.then.get(2), [3]);
  assert.equal(l.on.get(0), undefined);
});

test("a step's time breakdown over the footer's range (#844)", () => {
  // (crate::timings::PhaseTimes: runs summed by the first range edge their end is within.)
  const pt = { at: 0, edges: [60, 300, 600, 1800, 3600, 7200, 21600, 86400, 259200, 604800], steps: {
    depth: { phases: [["composing", "compute"], ["reading the depth sources' windows", "nas-read"]],
      b: [[0, 1, 0, 100, 40, [[0, 30, 30, 0, 0], [1, 60, 6, 3.5, 0]]], [6, 2, 1, 200, 50, [[0, 150, 140, 0, 0]]], [8, 5, 0, 900, 0, [[1, 900, 0, 10, 0]]]] },
  } };
  assert.equal(phaseBreakdown(pt, "slope", 21600), null, "a step with no runs");
  // The last minute: the first edge's alone.
  const m = phaseBreakdown(pt, "depth", 60);
  assert.deepEqual([m.runs, m.failed, m.wall, m.untimed], [1, 0, 100, 10]);
  assert.deepEqual(m.parts.map((p) => [p.name, p.wall, p.nas]), [["reading the depth sources' windows", 60, 3.5], ["composing", 30, 0]]);
  // Six hours: the edges up to it; largest first.
  const six = phaseBreakdown(pt, "depth", 21600);
  assert.deepEqual([six.runs, six.failed, six.wall, six.cpu], [3, 1, 300, 90]);
  assert.deepEqual(six.parts.map((p) => [p.name, p.wall, p.cpu]), [["composing", 180, 170], ["reading the depth sources' windows", 60, 6]]);
  // A week: everything; a range between edges takes the edges below it.
  assert.equal(phaseBreakdown(pt, "depth", 604800).runs, 8);
  assert.equal(phaseBreakdown(pt, "depth", 50000).runs, 3);
  assert.equal(phaseBreakdown(pt, "depth", 30), null, "under the first edge: none");
});
