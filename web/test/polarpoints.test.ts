// node web/test/polarpoints.test.ts (tools/dev/check.sh's web part): the polar caps' names
// (src/polarlabels.ts: MapLibre's glyph layout and SDF edges on the CPU) and their thinning
// (a name shown once its reach spans the spacing on screen, as src/polarpoints.ts asks).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ONE_EM, charsOf, layout, nameShows, rasterize, wrap, type Glyph, type TextLook } from '../src/polarlabels.ts';

/** A glyph 10 em px wide advancing 12: its SDF 255 (inside) on its middle rows and columns, falling
 * to 0 at the bitmap's border, as a PBF glyph's (3 px border each side). */
function glyph(): Glyph {
  const w = 16, h = 22;
  const data = new Uint8Array(w * h);
  for (let j = 0; j < h; j++) {
    for (let i = 0; i < w; i++) {
      const edge = Math.min(i, j, w - 1 - i, h - 1 - j);
      data[j * w + i] = Math.min(255, edge * 40);
    }
  }
  return { bitmap: { width: w, height: h, data }, metrics: { width: 10, height: 16, left: 1, top: -4, advance: 12 } };
}

const look: TextLook = {
  fill: [1, 0.5, 0, 1],
  halo: [0, 0, 0, 1],
  haloWidth: 1.5,
  haloBlur: 0,
  size: 12,
  spacing: 0,
  maxWidth: 8,
  transform: 'none',
};
const glyphs = new Map([...'ABCD -'].map((c) => [c, c === ' ' ? { ...glyph(), metrics: { ...glyph().metrics, advance: 6 } } : glyph()] as const));

test('a long name wraps at its spaces into even lines no wider than the max width', () => {
  // (8 em = 192 em px: "AAAA BBBB CCCC DDDD" is 4 × 48 + 3 × 6 = 210 wide: two lines.)
  assert.deepEqual(wrap('AAAA BBBB CCCC DDDD', glyphs, look), ['AAAA BBBB', 'CCCC DDDD']);
  assert.deepEqual(wrap('AAAA', glyphs, look), ['AAAA']);
  assert.deepEqual(wrap('AAAAAAAAAAAAAAAAAAAA', glyphs, { ...look }), ['AAAAAAAAAAAAAAAAAAAA'], 'no space: one line');
  assert.deepEqual(charsOf([{ text: 'ab', scale: 1 }], { transform: 'uppercase' }), ['A', 'B']);
});

test('the glyphs placed as MapLibre shapes them: centred, its baseline offset', () => {
  const { placed, box } = layout([{ text: 'AB', scale: 1 }], glyphs, look, 'center');
  // (Two glyphs advancing 12: the line 24 wide, centred; each bitmap at its left less the border;
  // one line centred has its glyphs' origin at -17, their bitmaps' tops at -17 - top - 3.)
  assert.deepEqual(placed.map((p) => [p.x, p.y]), [[-12 + 1 - 3, -17 + 4 - 3], [1 - 3, -17 + 4 - 3]]);
  assert.deepEqual(box, [-12, -(1.2 * ONE_EM) / 2, 12, (1.2 * ONE_EM) / 2]);
  // (Hanging from its anchor: the block below it; a second line at 0.8 under the first.)
  const two = layout([{ text: 'A', scale: 1 }, { text: 'B', scale: 0.8 }], glyphs, look, 'top');
  assert.deepEqual(two.box, [-6, 0, 6, 1.2 * ONE_EM * 1.8]);
});

test('the SDF turned to the fill inside and the halo round it', () => {
  const img = rasterize([{ text: 'A', scale: 1 }], glyphs, look, 'center', 2)!;
  const px = (x: number, y: number) => [...img.data.slice((y * img.width + x) * 4, (y * img.width + x) * 4 + 4)];
  // (The glyph's middle in device px: its bitmap's centre, em px × size / 24 × dpr from the anchor.)
  const k = (look.size / ONE_EM) * 2;
  const cx = Math.round(img.ox + (-6 + 1 - 3 + 8) * k), cy = Math.round(img.oy + (-17 + 4 - 3 + 11) * k);
  assert.deepEqual(px(cx, cy), [255, 128, 0, 255], 'the fill');
  // (Toward the bitmap's edge: past the fill's edge, the halo, dark.)
  const edge = Math.round(img.ox + (-6 + 1 - 3 + 2.2) * k);
  const [r, g, b, a] = px(edge, cy);
  assert.ok(a > 0 && r < 60 && g < 40 && b === 0, `halo ${[r, g, b, a]}`);
  assert.equal(px(0, 0)[3], 0, 'nothing in the corner');
  assert.equal(rasterize([{ text: 'A', scale: 1 }], new Map(), look, 'center', 2), null, 'no glyph: nothing');
});

test('when a name shows', () => {
  // (Reach 9 km at 100 m a pixel: 90 px, the default spacing.)
  assert.ok(nameShows({ r: 9000 }, 100, 90, 90));
  assert.ok(!nameShows({ r: 9000 }, 101, 90, 90));
  // (An area's name also waits for its √area to span 20 px at the default spacing, sooner by half
  // as much as a closer spacing.)
  assert.ok(!nameShows({ r: 1e7, w: 1999 }, 100, 90, 90));
  assert.ok(nameShows({ r: 1e7, w: 2000 }, 100, 90, 90));
  assert.ok(nameShows({ r: 1e7, w: 1000 }, 100, 22.5, 90));
});
