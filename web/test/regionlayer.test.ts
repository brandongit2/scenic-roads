// node web/test/regionlayer.test.ts (tools/dev/check.sh's web part): src/regionlayer.ts, the region
// layer's borders as the app styles them: which borders a region's or a folder's hover draws, their
// colour, the region the footer tells of those at a point, and the names' points.
import assert from 'node:assert/strict';
import { GREEN, GREY, borderOf, colourOf, labelPoints, pick, sideHas } from '../src/regionlayer.ts';

/** The expressions' subset these build, evaluated as MapLibre does on a feature's properties. */
type Props = Record<string, string>;
function evaluate(e: unknown, p: Props): unknown {
  if (!Array.isArray(e)) return e;
  const [op, ...a] = e as [string, ...unknown[]];
  switch (op) {
    case 'get': return p[a[0] as string] ?? null;
    case 'concat': return a.map((x) => String(evaluate(x, p))).join('');
    case 'in': return String(evaluate(a[1], p)).includes(String(evaluate(a[0], p)));
    case 'any': return a.some((x) => evaluate(x, p) === true);
    case '!=': return evaluate(a[0], p) !== evaluate(a[1], p);
    case 'case': return evaluate(a[0], p) === true ? evaluate(a[1], p) : evaluate(a[2], p);
    case 'match': return (a[1] as string[]).includes(String(evaluate(a[0], p))) ? evaluate(a[2], p) : evaluate(a[3], p);
    default: throw new Error(`no ${op}`);
  }
}

// A coast (France on its left, no region on its right), a land border (France, Spain), the border
// of Corsica's region inside France's waters ("fr-cor" isn't "fr"), and a border with two regions on
// one side (overlapping regions: both on its left).
const coast = { l: 'fr', r: '' };
const land = { l: 'fr', r: 'es' };
const corsica = { l: 'fr-cor', r: 'fr-waters' };
const shared = { l: 'es,es-cat', r: 'fr' };

assert.equal(evaluate(sideHas('l', ['fr']), corsica), false, 'a prefix is not the region');
assert.equal(evaluate(sideHas('l', ['es-cat']), shared), true);
assert.equal(evaluate(sideHas('l', ['es']), shared), true, 'one of several');
assert.equal(evaluate(sideHas('l', ['fr', 'es']), land), true);
assert.equal(evaluate(sideHas('r', ['fr', 'es']), coast), false, 'none');
assert.equal(sideHas('l', []), false);

// A region's border: where it is on one side only.
const fr = borderOf(['fr']);
assert.deepEqual([coast, land, corsica, shared].map((f) => evaluate(fr, f)), [true, true, false, true]);
// A folder's regions as one: the border between two of them isn't drawn.
const iberia = borderOf(['es', 'fr']);
assert.deepEqual([coast, land, shared].map((f) => evaluate(iberia, f)), [true, false, false]);
// None: nothing drawn.
assert.deepEqual([coast, land].map((f) => evaluate(borderOf([]), f)), [false, false]);

// The colour: green with an enabled region either side, grey between disabled regions (or none).
const colour = colourOf(['es']);
assert.deepEqual([coast, land, shared].map((f) => evaluate(colour, f)), [GREY, GREEN, GREEN]);
assert.equal(colourOf([]), GREY);

// The region the footer tells: the smallest by its box; one with no box yet after any with one.
const byId = new Map([
  ['fr-waters', { id: 'fr-waters', name: 'French waters', bbox: [-10, 40, 12, 52] as [number, number, number, number] }],
  ['fr-cor', { id: 'fr-cor', name: 'Corsica', bbox: [8.5, 41.3, 9.6, 43.1] as [number, number, number, number] }],
  ['new', { id: 'new', name: 'Unsized' }],
]);
assert.equal(pick(['fr-waters', 'fr-cor'], byId), 'fr-cor');
assert.equal(pick(['new', 'fr-waters'], byId), 'fr-waters');
assert.equal(pick(['unknown'], byId), 'unknown');
assert.equal(pick([], byId), null);

// The names: where the server put each, none for a region without a point yet.
const pts = labelPoints([
  { id: 'a', name: 'A', label: [1, 2] },
  { id: 'b', name: 'B', disabled: true, label: [3, 4] },
  { id: 'c', name: 'C' },
]);
assert.deepEqual(pts.features.map((f) => [f.properties.name, f.properties.disabled, f.geometry.coordinates]), [['A', false, [1, 2]], ['B', true, [3, 4]]]);

console.log('regionlayer: ok');
