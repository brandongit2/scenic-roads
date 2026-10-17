// The framebuffer and viewport bound as a custom layer starts drawing, from MapLibre's own record of
// its GL state (Context#bindFramebuffer, Context#viewport) instead of gl.getParameter. In Safari every
// getParameter waits for the GPU process to run all the GL calls queued before it (~0.5 ms each on an
// M1 Pro with a dense city drawing, five a frame: a third of the frame's budget at 120 Hz); Chrome
// answers it from its own copy. The record is right when a layer starts: MapLibre binds both through
// it, and every custom layer puts back what it changed. (`?glcheck=1`: each read checked against
// getParameter, mismatches counted in `window.__glcheck`.)
import type { Map as MLMap } from 'maplibre-gl';

type Ctx = { bindFramebuffer: { get(): WebGLFramebuffer | null }; viewport: { get(): [number, number, number, number] } };
const CHECK = new URLSearchParams(location.search).get('glcheck') === '1';

/** The framebuffer bound now, and the viewport ([x, y, width, height]). */
export function boundTarget(map: MLMap, gl: WebGL2RenderingContext): { fb: WebGLFramebuffer | null; vp: readonly [number, number, number, number] } {
  const ctx = (map as unknown as { painter: { context: Ctx } }).painter.context;
  const fb = ctx.bindFramebuffer.get(), vp = ctx.viewport.get();
  if (CHECK) {
    const w = window as unknown as { __glcheck?: { reads: number; bad: number } };
    const c = (w.__glcheck ??= { reads: 0, bad: 0 });
    c.reads++;
    const v = gl.getParameter(gl.VIEWPORT) as Int32Array;
    if (gl.getParameter(gl.FRAMEBUFFER_BINDING) !== fb || v.some((x, i) => x !== vp[i])) c.bad++;
  }
  return { fb, vp };
}
