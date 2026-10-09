// How rail lines and their stops are drawn, the pure parts: the cross-ties' size (the rail
// layer's shader, roads/layer.ts, takes these constants) and a stop dot's size by its line's stop
// spacing (stations.ts builds its MapLibre expression from the same numbers). Node loads this file
// for tools/check/raildraw.test.mjs, so it imports nothing.

/** A railway is a thin core line with cross-ties: the core's half-thickness is this share of the
 * line's half-width, at least CORE_MIN_CSS CSS px (so a line zoomed out stays a line). */
export const CORE_K = 0.42;
export const CORE_MIN_CSS = 0.6;
/** No ties below this zoom (zoomed out they only clutter). */
export const TIE_MIN_Z = 9;

/** The core's half-thickness and the ties' half-length (device px; 0: no ties) of a line of
 * half-width `halfw` (device px) at zoom `z`, ties `ties` × the core's thickness long. The core
 * keeps its CSS-pixel floor zoomed out, so the ties keep their length on screen there instead of
 * shrinking with the line's width (they spanned the line's full width, which fell below the
 * core's floor by zoom 6). */
export function railGeom(halfw: number, dpr: number, ties: number, z: number): { core: number; tie: number } {
  const core = Math.max(halfw * CORE_K, CORE_MIN_CSS * dpr);
  return { core, tie: z >= TIE_MIN_Z && ties > 0 ? core * ties : 0 };
}

/** A stop's size by its line's stop spacing (m), before the zoom and the line weight: ×1 at
 * STOP_REF_M (a commuter line; about a ferry terminal's dot), smaller for closer stops, larger for
 * wider ones. `contrast` (0..1) sets how much: 0 every stop alike; at STOP_CONTRAST (the default)
 * the factor is 1 + 0.15 × log2(spacing ÷ 4 km), between 0.5 and 1.4. */
export const STOP_REF_M = 4000;
export const STOP_CONTRAST = 0.5;
export const stopSlope = (contrast: number) => 0.3 * contrast;
export const stopBounds = (contrast: number): [number, number] => [Math.max(0.1, 1 - contrast), 1 + 0.8 * contrast];
export function stopFactor(spacingM: number, contrast: number): number {
  const [lo, hi] = stopBounds(contrast);
  return Math.min(hi, Math.max(lo, 1 + stopSlope(contrast) * Math.log2(spacingM / STOP_REF_M)));
}
/** A stop's radius (CSS px) at zoom stops, per unit of size factor × line weight × base size. */
export const STOP_R: [number, number][] = [[4, 0.5], [9, 0.9], [14, 1.75], [18, 2.5]];
