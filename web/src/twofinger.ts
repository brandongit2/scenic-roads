/**
 * Two fingers on a touch screen, read as camera steps (trackpad.ts applies them about the terrain
 * point between the fingers, camera3d.ts). Each move of either finger gives the step since the
 * last one:
 *   · zoom: log2 of the spread's ratio, so the ground between the fingers keeps their spread (the
 *     camera's distance to that point is divided by the same ratio: the same at every zoom, over
 *     any terrain);
 *   · turn: the angle the line between the fingers turned (clockwise on screen: the bearing goes
 *     down, the map turning with the fingers);
 *   · pan: the ground point between the fingers follows their midpoint, once it has moved
 *     PAN_PX from where the gesture started (a pinch or a turn whose midpoint wavers by a few
 *     pixels doesn't drift the map); it then catches up and follows exactly;
 *   · tilt: both fingers dragged up or down together (0.5° a pixel, up steepens), side by side
 *     rather than one above the other. A gesture that starts so is a tilt only.
 * Zoom and turn start once past a threshold (MapLibre's own: a tenth of a level; 25 px of arc), so
 * a pinch doesn't turn the map nor a turn zoom it by accident; the zoom then catches up (the whole
 * spread counts), the turn doesn't (a jump of 10–20° would show).
 */
export type Pt = { x: number; y: number };
export interface TwoFingerStep {
  /** Where the ground point between the fingers is on screen before this step (zoom and turn
   * keep it there), and where it goes (the pan). */
  from: Pt;
  to: Pt;
  dz: number;
  dBearing: number;
  dPitch: number;
}

const ZOOM_THRESHOLD = 0.1;
const ROTATE_ARC_PX = 25;
const TILT_DEG_PER_PX = 0.5;
const PAN_PX = 6;
/** A finger has moved once this far (px); a gesture whose first moves are one finger's only for
 * this long is not a tilt (as MapLibre's). */
const MOVED_PX = 2;
const SINGLE_MS = 100;

const mid = (a: Pt, b: Pt): Pt => ({ x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 });
const angle = (a: Pt, b: Pt) => Math.atan2(b.y - a.y, b.x - a.x);
const wrap = (r: number) => ((((r + Math.PI) % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI)) - Math.PI;
const vertical = (dx: number, dy: number) => Math.abs(dy) > Math.abs(dx);

export class TwoFingers {
  private a: Pt;
  private b: Pt;
  private readonly a0: Pt;
  private readonly b0: Pt;
  private minSpread: number;
  private zooming = false;
  private turning = false;
  private panning = false;
  /** Where the ground point between the fingers is on screen. */
  private at: Pt;
  /** undefined: not decided yet; then whether this gesture is a tilt. */
  tilt: boolean | undefined;
  private firstMove: number | undefined;

  constructor(a: Pt, b: Pt) {
    this.a = this.a0 = { ...a };
    this.b = this.b0 = { ...b };
    this.minSpread = Math.hypot(b.x - a.x, b.y - a.y);
    this.at = mid(a, b);
    // Fingers one above the other: not a tilt (dragging them reads as a pan or a turn).
    if (vertical(b.x - a.x, b.y - a.y)) this.tilt = false;
  }

  /** The midpoint where the gesture started. */
  get start(): Pt {
    return mid(this.a0, this.b0);
  }

  /** The step for the fingers now at a and b (null: nothing to do yet). `t`: the event's time (ms). */
  move(a: Pt, b: Pt, t: number): TwoFingerStep | null {
    const pa = this.a, pb = this.b;
    const va = { x: a.x - pa.x, y: a.y - pa.y }, vb = { x: b.x - pb.x, y: b.y - pb.y };
    if (this.tilt === undefined) {
      const ma = Math.hypot(va.x, va.y) >= MOVED_PX, mb = Math.hypot(vb.x, vb.y) >= MOVED_PX;
      if (!ma && !mb) return null; // (too little yet: keep the last points to measure from)
      if (!ma || !mb) {
        this.firstMove ??= t;
        if (t - this.firstMove < SINGLE_MS) return null;
        this.tilt = false;
      } else this.tilt = vertical(va.x, va.y) && vertical(vb.x, vb.y) && va.y > 0 === vb.y > 0;
    }
    this.a = { ...a };
    this.b = { ...b };
    const from = this.at, m = mid(a, b);
    if (this.tilt) return { from, to: from, dz: 0, dBearing: 0, dPitch: (-(va.y + vb.y) / 2) * TILT_DEG_PER_PX };
    const s = this.start;
    if (!this.panning) this.panning = Math.hypot(m.x - s.x, m.y - s.y) >= PAN_PX;
    const to = this.panning ? m : from;
    this.at = to;
    const d0 = Math.hypot(pb.x - pa.x, pb.y - pa.y), d1 = Math.hypot(b.x - a.x, b.y - a.y);
    let dz = 0, dBearing = 0;
    if (d0 > 0 && d1 > 0) {
      if (this.zooming) dz = Math.log2(d1 / d0);
      else {
        // Past the threshold the zoom catches up with the spread since the start (a tenth of a
        // level at once), so the ground between the fingers keeps their spread exactly.
        const since = Math.log2(d1 / Math.hypot(this.b0.x - this.a0.x, this.b0.y - this.a0.y));
        this.zooming = Math.abs(since) >= ZOOM_THRESHOLD;
        if (this.zooming) dz = since;
      }
      if (!this.turning) {
        this.minSpread = Math.min(this.minSpread, d1);
        const since = Math.abs(wrap(angle(a, b) - angle(this.a0, this.b0)));
        this.turning = since * (this.minSpread / 2) >= ROTATE_ARC_PX;
      }
      if (this.turning) dBearing = (-wrap(angle(a, b) - angle(pa, pb)) * 180) / Math.PI;
    }
    return { from, to, dz, dBearing, dPitch: 0 };
  }
}
