/**
 * Mouse wheel or trackpad? A pure reading of a stream of wheel events (no DOM, no clock: the
 * events' own timeStamp), so that recordings can be replayed as test fixtures
 * (web/test/wheelkind.test.ts, from the owner's mouse and trackpad in Chrome and Safari).
 *
 * A device is judged once per burst (events more than BURST_GAP_MS apart open a new one), on the
 * burst's first event, and a mouse reading is taken back within the burst by a step under NOTCH_MIN
 * (a trackpad's; no notch is smaller than the one that opened it). What separates them in the recordings:
 *   mouse: the opening event is a whole-pixel step of NOTCH_MIN px or more along one axis only (a
 *          sideways one with Shift): a notch, 12-13 on the owner's mouse in both browsers; then a spin
 *          scales it up (103, 205, 308 ...) at 5-9 ms, or a slow wheel gives single events 40 ms
 *          or more apart. Elsewhere: deltaMode lines, Chromium's wheelDeltaY multiples of 120,
 *          Firefox's notch.
 *   trackpad: opens at 1-7 px (often on both axes), then follows the display at 8 ms with
 *          steps that grow into the hundreds in a flick; a pinch is ctrlKey (left out of the
 *          reading, the burst's kind stands).
 * Misreadings lean to the trackpad, the less harmful here: a mouse read as a trackpad pans a
 * little per notch, a trackpad read as a mouse zooms a level per event. So a mouse whose notch is
 * under NOTCH_MIN px with no wheelDeltaY in 120s, a smooth-scrolling mouse and a Magic Mouse (small
 * opening steps) read as trackpads; Windows-style notches (100, 120 px) as mice; a trackpad opening
 * at NOTCH_MIN or more reads as a mouse only until its first smaller step (a few ms in a flick).
 */
export interface WheelSample {
  deltaX: number;
  deltaY: number;
  /** 0 pixels, 1 lines, 2 pages. */
  deltaMode: number;
  /** The legacy wheelDeltaY (Chromium, Safari); absent elsewhere. */
  wheelDeltaY?: number;
  ctrlKey: boolean;
  /** Milliseconds, any origin. */
  timeStamp: number;
}
export type WheelKind = 'mouse' | 'trackpad';

/** Firefox on macOS reports a mouse wheel notch as this many pixels (as MapLibre's scroll zoom). */
export const FIREFOX_NOTCH = 4.000244140625;
/** A pause this long (ms) between events starts a new burst. */
export const BURST_GAP_MS = 280;

/** A whole-pixel opening step of at least this (px, one axis) is a mouse wheel's: a notch (12-13 on the owner's
 * mouse), or a spin's first step; a trackpad opens at 1-7. */
export const NOTCH_MIN = 12;

/** Whether one event, taken alone (and `opening` a burst or not), looks like a mouse wheel's. */
export function looksMouse(e: WheelSample, opening: boolean): boolean {
  if (e.deltaMode === 1) return true;
  const one = (e.deltaX === 0) !== (e.deltaY === 0); // along one axis only
  if (!one) return false;
  const d = Math.abs(e.deltaX || e.deltaY);
  const wdy = e.wheelDeltaY;
  return (
    (opening && Number.isInteger(d) && d >= NOTCH_MIN) ||
    (!!wdy && Math.abs(wdy) % 120 === 0 && wdy !== -3 * e.deltaY) ||
    d % FIREFOX_NOTCH === 0
  );
}

export class WheelClassifier {
  private last = -Infinity;
  private kind: WheelKind = 'trackpad';
  /** Reads the next event: the device the burst it belongs to comes from, and whether it opens the burst.
   * A pinch (ctrlKey) is no reading of the device: the burst's kind stands. */
  push(e: WheelSample): { kind: WheelKind; opening: boolean; mouseLook: boolean } {
    if (e.ctrlKey) return { kind: this.kind, opening: false, mouseLook: false };
    const gap = e.timeStamp - this.last;
    const opening = gap > BURST_GAP_MS;
    const mouseLook = looksMouse(e, opening);
    if (opening) this.kind = mouseLook ? 'mouse' : 'trackpad';
    else if (this.kind === 'mouse' && e.deltaMode === 0 && Math.max(Math.abs(e.deltaX), Math.abs(e.deltaY)) < NOTCH_MIN && !mouseLook) this.kind = 'trackpad';
    this.last = e.timeStamp;
    return { kind: this.kind, opening, mouseLook };
  }
}

/** The device behind a recorded stream of wheel events: that of its last burst. */
export function classifyWheel(events: readonly WheelSample[]): WheelKind {
  const c = new WheelClassifier();
  let kind: WheelKind = 'trackpad';
  for (const e of events) kind = c.push(e).kind;
  return kind;
}
