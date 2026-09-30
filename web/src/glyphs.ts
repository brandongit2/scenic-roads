// Glyphs drawn on the page (Chinese, Japanese and Korean characters, which the served fonts don't
// have: MapLibre draws them with TinySDF on a canvas) a few at a time, in the time between frames.
// MapLibre draws every glyph a tile asks for at once: a tile of new Japanese names was a 50–70 ms
// task, a few frames dropped mid-gesture. The tile's labels wait for their glyphs as before.
import type { Map as MLMap } from 'maplibre-gl';

/** A slice when no idle time came within OVERDUE_MS (ms). */
const SLICE_MS = 2;
const OVERDUE_MS = 100;

type Draw = (...args: unknown[]) => Promise<unknown>;
type GlyphManager = { _drawGlyph: Draw; __sliced?: boolean };

export function slicedGlyphs(map: MLMap) {
  const gm = (map as unknown as { style?: { glyphManager?: GlyphManager } }).style?.glyphManager;
  if (!gm) return;
  const proto = Object.getPrototypeOf(gm) as GlyphManager;
  if (Object.prototype.hasOwnProperty.call(proto, '__sliced') || typeof proto._drawGlyph !== 'function') return;
  proto.__sliced = true;
  const draw = proto._drawGlyph;
  const queue: (() => Promise<unknown>)[] = [];
  let scheduled = false;
  const run = async (left: () => number) => {
    // (Each draw is awaited: its work happens in the promise's continuation.)
    while (queue.length && left() > 0) await queue.shift()!();
    scheduled = false;
    if (queue.length) schedule();
  };
  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    if (typeof requestIdleCallback === 'function') {
      requestIdleCallback((d) => {
        const t0 = performance.now();
        void run(() => (d.didTimeout ? SLICE_MS - (performance.now() - t0) : d.timeRemaining() - 1));
      }, { timeout: OVERDUE_MS });
    } else {
      setTimeout(() => {
        const t0 = performance.now();
        void run(() => SLICE_MS - (performance.now() - t0));
      }, 0);
    }
  };
  proto._drawGlyph = function (this: GlyphManager, ...args: unknown[]) {
    return new Promise((resolve, reject) => {
      queue.push(() => draw.apply(this, args).then(resolve, reject));
      schedule();
    });
  };
}
