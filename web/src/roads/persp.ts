// Line widths with distance in tilted views (state.ts RoadPersp): the CPU side of the roads'
// vertex shader (layer.ts, "Width with distance"), for hit testing and the frame's width bounds.

export type Persp = { strength: number; cap: number; fade: number };
/** The style curve's widths alone. */
export const NO_PERSP: Persp = { strength: 0, cap: 1e6, fade: 0 };
/** A piece's width (as the vertex shader: w0 the style width at the view centre's zoom, wz at the
 * piece's own zoom, kr the centre's distance ÷ the piece's). */
export function perspWidth(w0: number, wz: number, kr: number, p: Persp = NO_PERSP): number {
  let w = wz * (w0 * kr / wz) ** p.strength;
  w = Math.min(w, Math.max(w0 * p.cap, wz));
  return w * Math.min(kr, 1) ** (p.fade * p.strength);
}
