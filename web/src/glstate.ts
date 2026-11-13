// The framebuffer and viewport bound as a custom layer starts drawing, from MapLibre's own record of
// its GL state (Context#bindFramebuffer, Context#viewport) instead of gl.getParameter. In Safari every
// getParameter waits for the GPU process to run all the GL calls queued before it (~0.5 ms each on an
// M1 Pro with a dense city drawing, five a frame: a third of the frame's budget at 120 Hz); Chrome
// answers it from its own copy. The record is right when a layer starts: MapLibre binds both through
// it, and every custom layer puts back what it changed.
import type { Map as MLMap } from 'maplibre-gl';

type Ctx = { bindFramebuffer: { get(): WebGLFramebuffer | null }; viewport: { get(): [number, number, number, number] } };

/** The framebuffer bound now, and the viewport ([x, y, width, height]). */
export function boundTarget(map: MLMap): { fb: WebGLFramebuffer | null; vp: readonly [number, number, number, number] } {
  const ctx = (map as unknown as { painter: { context: Ctx } }).painter.context;
  return { fb: ctx.bindFramebuffer.get(), vp: ctx.viewport.get() };
}
