/**
 * Mouse wheel or trackpad? A pure reading of a stream of wheel events (no DOM, no clock: the
 * events' own timeStamp), so that recordings can be replayed as test fixtures
 * (web/test/wheelkind.test.ts).
 *
 * A device is judged once per burst (events less than BURST_GAP_MS apart), from the burst's first
 * event, and a later event that moves sideways turns the burst to a trackpad. The signals:
 *   mouse: deltaMode lines; or a purely vertical step that is Chromium's notch (wheelDeltaY a
 *          multiple of 120 that is not the trackpad's −3 × deltaY), Firefox's notch (a multiple of
 *          FIREFOX_NOTCH pixels), or, opening a burst, a whole-pixel step of 50 px or more.
 *   trackpad: everything else (small, fractional, sideways steps).
 * Known to fail where the signals don't hold: a mouse with smooth scrolling or a free-spinning
 * wheel (small fractional steps, then read as a trackpad), a high-resolution mouse on Safari, a
 * trackpad's first step of a flick that is whole and 50 px or more.
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

/** Whether one event, taken alone (and `opening` a burst or not), looks like a mouse wheel's. */
export function looksMouse(e: WheelSample, opening: boolean): boolean {
  if (e.deltaMode === 1) return true;
  if (e.deltaX !== 0 || e.deltaY === 0) return false;
  const wdy = e.wheelDeltaY;
  return (
    (!!wdy && Math.abs(wdy) % 120 === 0 && wdy !== -3 * e.deltaY) ||
    Math.abs(e.deltaY) % FIREFOX_NOTCH === 0 ||
    (opening && Number.isInteger(e.deltaY) && Math.abs(e.deltaY) >= 50 && wdy !== -3 * e.deltaY)
  );
}

export class WheelClassifier {
  private last = -Infinity;
  private kind: WheelKind = 'trackpad';
  /** Reads the next event: the device the burst it belongs to comes from, and whether it opens the burst. */
  push(e: WheelSample, keepMouse = false): { kind: WheelKind; opening: boolean; mouseLook: boolean } {
    const opening = e.timeStamp - this.last > BURST_GAP_MS;
    const mouseLook = looksMouse(e, opening);
    if (opening) this.kind = mouseLook ? 'mouse' : 'trackpad';
    else if (e.deltaX !== 0 && !keepMouse) this.kind = 'trackpad';
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
