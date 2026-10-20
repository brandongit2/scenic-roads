// node --test tools/check/persp.test.mjs: road widths with distance (web/src/roads/persp.ts, the
// CPU copy of the roads' vertex shader in web/src/roads/layer.ts, for hit testing and the frame's
// near/far width bounds).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { NO_PERSP, perspWidth } from '../../web/src/roads/persp.ts';

const near = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) <= eps * Math.max(1, Math.abs(b)), `${a} ≉ ${b}`);
// A style curve (WIDTHS' primary row), as the shader's widthAt.
const Z = [4, 7, 10, 13, 16, 19], W = [0.8, 1.2, 2.0, 3.2, 6.5, 13];
const at = (z) => {
  if (z <= Z[0]) return W[0];
  for (let i = 1; i < Z.length; i++) if (z <= Z[i]) return W[i - 1] + ((W[i] - W[i - 1]) * (z - Z[i - 1])) / (Z[i] - Z[i - 1]);
  return W[W.length - 1];
};
const KR = [1 / 256, 1 / 64, 1 / 8, 0.3, 1, 1.7, 4, 16];
const width = (zoom, kr, p) => perspWidth(at(zoom), at(zoom + Math.log2(kr)), kr, p);

test('strength 0 is the style curve exactly, whatever the cap and fade', () => {
  for (const zoom of [5, 9, 13, 17]) for (const kr of KR) for (const p of [NO_PERSP, { strength: 0, cap: 1, fade: 1 }, { strength: 0, cap: 8, fade: 0.3 }]) {
    assert.equal(width(zoom, kr, p), at(zoom + Math.log2(kr)));
  }
});

test('strength 1, no cap, no fade: true perspective', () => {
  for (const kr of KR) near(width(13, kr, { strength: 1, cap: 1e6, fade: 0 }), at(13) * kr);
  // (a primary at z13 with kr 1/8 draws ~0.21 px at the defaults, not the style curve's 2)
  assert.ok(width(13, 1 / 8, { strength: 1, cap: 3, fade: 0.3 }) < 0.4);
});

test('the cap is a ceiling near the camera, never below the style curve', () => {
  for (const kr of KR) for (const cap of [1, 1.5, 3, 8]) {
    const w = width(13, kr, { strength: 1, cap, fade: 0 });
    assert.ok(w <= Math.max(at(13) * cap, at(13 + Math.log2(kr))) + 1e-12);
  }
});

test('width grows with the distance ratio (the near edge an upper bound, the far a lower)', () => {
  for (const p of [NO_PERSP, { strength: 1, cap: 3, fade: 0.3 }, { strength: 0.5, cap: 1.5, fade: 1 }, { strength: 1, cap: 1, fade: 1 }]) {
    for (const zoom of [5, 9, 13]) for (let i = 1; i < KR.length; i++) assert.ok(width(zoom, KR[i], p) >= width(zoom, KR[i - 1], p) - 1e-12);
  }
});

test('the shader computes the same as perspWidth', () => {
  const src = readFileSync(new URL('../../web/src/roads/layer.ts', import.meta.url), 'utf8');
  for (const line of [
    'float w0 = widthAt(cls, u_zoom), wsz = widthAt(cls, ze);',
    'float w = wsz * pow(w0 * kr / wsz, u_persp.x);',
    'w = min(w, max(w0 * u_persp.y, wsz));',
    'w *= pow(min(kr, 1.0), u_persp.z * u_persp.x);',
    'float cw = casingAt(ze) * g, gw = route ? glowAt(ze) * g : 0.0;',
    'gl.uniform3f(u.u_persp, pp.strength, pp.cap, pp.fade);',
  ]) assert.ok(src.includes(line), `layer.ts lacks: ${line}`);
});
