import type { Map as MLMap } from 'maplibre-gl';
import { anchorAt, centrePoint, dolly, ownPan, orbit, panTo, setLocationAt, type Anchor } from './camera3d';
import { TwoFingers, type Pt } from './twofinger';

/**
 * Trackpad-first navigation, anchored at the cursor:
 *   two-finger drag            → pan         (wheel events without modifiers; the ground at the
 *                                              view centre moves with the fingers)
 *   click-drag                 → pan         (the grabbed ground stays under the pointer)
 *   pinch                      → zoom        (wheel + ctrlKey in Chromium/Firefox, GestureEvent in Safari),
 *                                              carrying on briefly after the fingers stop (PINCH_TAU_MS)
 *   ⌘ Cmd + two-finger drag    → zoom        (vertical)
 *   ⌥ Option + two-finger drag → x rotates, y tilts
 *   right-drag / Ctrl-drag     → rotate + tilt
 * And on a touch screen: one finger pans (as a left-drag); two pinch to zoom, turn to rotate,
 * move together to pan and drag up or down side by side to tilt, all about the terrain point
 * between them (twofinger.ts reads the fingers; the one-finger pan is let go of when the second
 * lands; the zoom glides on as a trackpad pinch's, nothing else does); a double tap zooms in
 * there, and a tap then a press dragged down or up zooms in or out about the tapped point; a long
 * press calls `onLongPress` (main.ts: a menu of links there); a tap's click waits out the double
 * tap's time (`single`).
 * A mouse wheel zooms with short, snappy easing. Zoom, rotation and tilt keep the 3D point
 * under the cursor fixed on screen (see camera3d.ts).
 */
/** Pinch inertia: the zoom speed of the last PINCH_SAMPLE_MS carries on, decaying with this time
 * constant (short: it settles in about a third of a second), once no pinch event has come for
 * PINCH_END_MS (Chromium sends no gesture end); at most PINCH_MAX_DZ more levels. */
const PINCH_TAU_MS = 110;
const PINCH_SAMPLE_MS = 70;
const PINCH_END_MS = 45;
const PINCH_MAX_DZ = 0.9;
/** Zoom levels per pixel of a ⌘ two-finger scroll (the trackpad's own momentum carries it on). */
const CMD_ZOOM_PER_PX = 0.0045;
/** Firefox on macOS reports a mouse wheel notch as this many pixels (as MapLibre's scroll zoom). */
const FIREFOX_NOTCH = 4.000244140625;
export interface CameraControls {
  /** Smooth zoom by dz levels about a screen point (default: the view centre). */
  zoomBy(dz: number, px?: number, py?: number): void;
  /** Animated rotate / tilt about the ground at the view centre. */
  orbitBy(dBearing: number, dPitch: number): void;
  /** A click's action (the map's click handler): at once for a mouse's; for a finger's tap once
   * the double tap's time has passed with no finger down again, so that a double tap only zooms
   * and a tap followed at once by a pan or a pinch does nothing else. */
  single(f: () => void): void;
}

export function installTrackpad(map: MLMap, opts: { onLongPress?: (px: number, py: number) => void } = {}): CameraControls {
  map.scrollZoom.disable();
  map.doubleClickZoom.disable(); // replaced by the cursor-anchored zoom below
  map.dragRotate.disable(); // replaced by the cursor-anchored orbit below
  map.dragPan.disable(); // replaced by the ground-anchored pan below
  // Replaced by the terrain-anchored two-finger gesture and tap-drag zoom below. MapLibre's turn
  // and tilt pivot about the view centre's point on the pivot plane (sea level on the globe), and
  // its zoom scales the distance to that plane: over mountains seen from close by they pivoted
  // kilometres from the fingers, and a pinch overshot several times (tools/check/touch-gestures.mjs).
  map.touchZoomRotate.disable();
  map.touchPitch.disable();
  const el = map.getCanvasContainer();
  let lastEvent = 0;
  let burstMouse = false;
  let burstCmd = false;
  // Anchor for the current gesture burst, re-picked when the cursor moves.
  let anchor: { a: Anchor | null; x: number; y: number; t: number } | null = null;
  // Finer terrain tiles can change the ground under the cursor mid-gesture: an anchor that no
  // longer sits on it at (px, py) is picked again.
  const offGround = (a: Anchor | null, px: number, py: number) => {
    if (!a?.ground) return false;
    const q = map.project(a.ll);
    const e = map.queryTerrainElevation(a.ll);
    return Math.hypot(q.x - px, q.y - py) > 6 || (e !== null && Math.abs(e - a.elev) > 3);
  };
  const anchorFor = (px: number, py: number): Anchor | null => {
    const now = performance.now();
    const stale = !anchor || now - anchor.t > 300 || Math.hypot(px - anchor.x, py - anchor.y) > 3 || offGround(anchor.a, px, py);
    if (stale) anchor = { a: anchorAt(map, px, py), x: px, y: py, t: now };
    anchor!.t = now;
    return anchor!.a;
  };

  /** Zoom by dz around a screen point. Returns false at a zoom limit. */
  const zoomAt = (px: number, py: number, dz: number, a: Anchor | null) => {
    if (!dz) return true;
    // 3D dolly toward the anchor; at a limit it stops rather than switching anchoring mid-gesture.
    if (a) return dolly(map, a, dz, px, py);
    // No anchor (cursor off the planet): zoom about the view centre.
    const z = Math.max(map.getMinZoom(), Math.min(map.getMaxZoom(), map.getZoom() + dz));
    if (z === map.getZoom()) return false;
    map.zoomTo(z, { duration: 0 });
    return true;
  };
  /** Rotate / tilt around a screen point (around the view centre if the cursor is on sky). */
  const orbitAt = (dBearing: number, dPitch: number, a: Anchor | null, px: number, py: number) => {
    if (a?.ground) {
      // Around the cursor. If a step is refused (camera would get too close to the ground),
      // keep what is possible rather than switching to a different pivot mid-gesture.
      if (orbit(map, a, dBearing, dPitch, px, py)) return;
      if (dBearing && orbit(map, a, dBearing, 0, px, py)) return;
      return;
    }
    map.jumpTo({
      bearing: map.getBearing() + dBearing,
      pitch: Math.max(0, Math.min(map.getMaxPitch(), map.getPitch() + dPitch)),
      elevation: map.getCenterElevation(), // jumpTo would otherwise move the pivot to the terrain
    });
  };

  // ---- pan ----
  // A two-finger pan grabs the ground at the view centre and moves it with the fingers, the
  // camera keeping its height (MapLibre's pan moves the pivot plane, sea level on the globe, so
  // over high ground seen from close by it would be several times too fast). Where the ground
  // is close to that plane, MapLibre's pan is used as is (a = null). The grabbed point is
  // re-picked after a pause or once it has travelled far from the centre.
  let grab: { a: Anchor | null; x: number; y: number; t: number } | null = null;
  const panStep = (dx: number, dy: number) => {
    const now = performance.now();
    const c = centrePoint(map);
    const cv = map.getCanvas(); // (the canvas container itself has no height)
    const far = grab && Math.hypot(grab.x - c.x, grab.y - c.y) > 0.25 * Math.min(cv.clientWidth, cv.clientHeight);
    if (!grab || now - grab.t > 250 || far) {
      const a = anchorAt(map, c.x, c.y);
      grab = { a: a?.ground && ownPan(map, a) ? a : null, x: c.x, y: c.y, t: now };
    }
    grab.t = now;
    if (grab.a) {
      if (panTo(map, grab.a, grab.x - dx, grab.y - dy)) {
        grab.x -= dx;
        grab.y -= dy;
        return;
      }
      grab = null; // re-pick on the next event
    }
    map.panBy([dx, dy], { animate: false });
  };

  // ---- smooth wheel zoom ----
  let zLeft = 0; // zoom levels still to apply
  let wheel: { x: number; y: number; a: Anchor | null } | null = null;
  let raf = 0;
  let last = 0;
  const step = (now: number) => {
    const dt = Math.min(64, now - last);
    last = now;
    if (!wheel || Math.abs(zLeft) < 0.002) {
      zLeft = 0;
      raf = 0;
      return;
    }
    const k = 1 - Math.exp(-dt / 55);
    const dz = Math.abs(zLeft) < 0.004 ? zLeft : zLeft * k;
    if (!zoomAt(wheel.x, wheel.y, dz, wheel.a)) zLeft = 0; // hit a zoom limit
    else zLeft -= dz;
    raf = requestAnimationFrame(step);
  };
  const smoothZoom = (px: number, py: number, dz: number) => {
    stopInertia();
    wheel = { x: px, y: py, a: anchorFor(px, py) };
    zLeft += dz;
    if (!raf) {
      last = performance.now();
      raf = requestAnimationFrame(step);
    }
  };

  // ---- pinch inertia ----
  // Recent pinch steps (time, levels) and where they were; when they stop, the speed carries on.
  const pinchSteps: [number, number][] = [];
  let pinchAt: { x: number; y: number; a: Anchor | null } | null = null;
  let pinchEnd = 0;
  let inertia = 0;
  const stopInertia = () => {
    cancelAnimationFrame(inertia);
    inertia = 0;
  };
  /** A pinch's zoom step (a trackpad's: released when no step comes for PINCH_END_MS; a touch
   * screen's, `timed` false: when the fingers lift). */
  const pinchStep = (px: number, py: number, dz: number, a: Anchor | null, timed = true) => {
    stopInertia();
    zLeft = 0;
    const now = performance.now();
    pinchSteps.push([now, dz]);
    while (pinchSteps.length && now - pinchSteps[0][0] > PINCH_SAMPLE_MS) pinchSteps.shift();
    pinchAt = { x: px, y: py, a };
    zoomAt(px, py, dz, a);
    clearTimeout(pinchEnd);
    if (timed) pinchEnd = window.setTimeout(pinchRelease, PINCH_END_MS);
  };
  // The fingers have stopped: carry on at the zoom speed of the last steps, decaying.
  const pinchRelease = () => {
    // (Fingers held still before lifting: their last steps are old, and carry nothing.)
    const now = performance.now();
    while (pinchSteps.length && now - pinchSteps[0][0] > PINCH_SAMPLE_MS + PINCH_END_MS) pinchSteps.shift();
    const at = pinchAt;
    if (!at || pinchSteps.length < 2) return void (pinchSteps.length = 0);
    const span = Math.max(16, pinchSteps[pinchSteps.length - 1][0] - pinchSteps[0][0]);
    let v = pinchSteps.reduce((sum, [, dz]) => sum + dz, 0) / span; // levels per ms
    pinchSteps.length = 0;
    // A pause or a slow finish carries nothing; the carry is capped (v·τ levels in all).
    if (Math.abs(v) < 0.0006) return;
    v = Math.sign(v) * Math.min(Math.abs(v), PINCH_MAX_DZ / PINCH_TAU_MS);
    let t0 = performance.now();
    const tick = (now: number) => {
      const dt = Math.min(48, now - t0);
      t0 = now;
      const dz = v * PINCH_TAU_MS * (1 - Math.exp(-dt / PINCH_TAU_MS));
      v *= Math.exp(-dt / PINCH_TAU_MS);
      if (Math.abs(v) < 0.00008 || !zoomAt(at.x, at.y, dz, at.a)) return void (inertia = 0);
      inertia = requestAnimationFrame(tick);
    };
    inertia = requestAnimationFrame(tick);
  };

  el.addEventListener(
    'wheel',
    (e: WheelEvent) => {
      e.preventDefault();
      const now = performance.now();
      const rect = el.getBoundingClientRect();
      const px = e.clientX - rect.left, py = e.clientY - rect.top;
      let dx = e.deltaX, dy = e.deltaY;
      // Classify the burst: a mouse wheel reports whole notches (deltaMode 1, or legacy
      // wheelDelta in multiples of 120 that are not the trackpad's −3 × deltaY).
      const wdy = (e as unknown as { wheelDeltaY?: number }).wheelDeltaY;
      // A mouse: lines; Chromium's notches (wheelDelta in 120s, not the trackpad's −3 × deltaY);
      // Firefox's (multiples of FIREFOX_NOTCH); or, opening a burst, a large whole-pixel vertical
      // step (a trackpad starts small and fractional, with some sideways motion).
      const looksMouse = e.deltaMode === 1 || (e.deltaX === 0 && e.deltaY !== 0 && (
        (!!wdy && Math.abs(wdy) % 120 === 0 && wdy !== -3 * e.deltaY) ||
        Math.abs(e.deltaY) % FIREFOX_NOTCH === 0 ||
        (now - lastEvent > 280 && Number.isInteger(e.deltaY) && Math.abs(e.deltaY) >= 50 && wdy !== -3 * e.deltaY)));
      if (now - lastEvent > 280) {
        burstMouse = looksMouse;
        burstCmd = e.metaKey && !e.ctrlKey && !looksMouse;
      } else if (e.deltaX !== 0 && !e.altKey) burstMouse = false;
      lastEvent = now;
      if (e.deltaMode === 1) {
        dx *= 16;
        dy *= 16;
      } else if (e.deltaMode === 2) {
        dx *= map.getCanvas().clientHeight;
        dy *= map.getCanvas().clientHeight;
      }
      if (e.ctrlKey) {
        // Pinch (or Ctrl + wheel).
        pinchStep(px, py, -dy * 0.012, anchorFor(px, py));
      } else if (burstCmd || (e.metaKey && !burstMouse)) {
        // ⌘ + two-finger scroll: zoom, the whole burst (its momentum too, Cmd released or not).
        stopInertia();
        zLeft = 0;
        zoomAt(px, py, -dy * CMD_ZOOM_PER_PX, anchorFor(px, py));
      } else if (burstMouse && !e.altKey) {
        // ~0.55 zoom levels per notch.
        const notches = e.deltaMode === 1 ? e.deltaY / 3 : dy / 100;
        smoothZoom(px, py, -Math.sign(notches) * Math.min(2, Math.abs(notches)) * 0.55);
      } else if (e.altKey) {
        orbitAt(-dx * 0.35, dy * 0.3, anchorFor(px, py), px, py);
      } else {
        panStep(dx, dy);
      }
    },
    { passive: false },
  );

  // A drag isn't a click. MapLibre drops a click whose pointer moved from where it went down, but
  // moving the camera (jumpTo) resets its handlers and with them where the pointer went down, so
  // after a drag here its click would still fire: the click that ends a drag is dropped before
  // MapLibre sees it (capture on the container, which holds the canvas).
  let dragged = false;
  el.addEventListener('pointerdown', () => (dragged = false), true);
  el.addEventListener('click', (e) => {
    if (!dragged) return;
    dragged = false;
    e.stopImmediatePropagation();
    e.preventDefault();
  }, true);

  // Right-drag or Ctrl + left-drag: rotate (x) and tilt (y) around the point under the cursor.
  // The anchor stays pinned where the drag started (ax, ay).
  let drag: { id: number; x: number; y: number; ax: number; ay: number; a: Anchor | null } | null = null;
  el.addEventListener('pointerdown', (e: PointerEvent) => {
    if (!(e.button === 2 || (e.button === 0 && e.ctrlKey))) return;
    const rect = el.getBoundingClientRect();
    const px = e.clientX - rect.left, py = e.clientY - rect.top;
    drag = { id: e.pointerId, x: e.clientX, y: e.clientY, ax: px, ay: py, a: anchorAt(map, px, py) };
    el.setPointerCapture(e.pointerId);
    e.preventDefault();
    e.stopPropagation();
  }, true);
  el.addEventListener('pointermove', (e: PointerEvent) => {
    if (!drag || e.pointerId !== drag.id) return;
    const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
    drag.x = e.clientX;
    drag.y = e.clientY;
    if (dx || dy) dragged = true;
    orbitAt(-dx * 0.5, -dy * 0.5, drag.a, drag.ax, drag.ay);
    e.stopPropagation();
  }, true);
  const end = (e: PointerEvent) => {
    if (!drag || e.pointerId !== drag.id) return;
    drag = null;
    el.releasePointerCapture(e.pointerId);
  };
  el.addEventListener('pointerup', end, true);
  el.addEventListener('pointercancel', end, true);
  el.addEventListener('contextmenu', (e) => e.preventDefault());

  // Fingers down now (a touch screen's): a second one ends the one-finger pan, its pinch MapLibre's.
  const touches = new Set<number>();
  // A long press: a finger still for LONG_MS (moving under 8 px) is a press, not a pan.
  const LONG_MS = 550;
  let press: { id: number; timer: number } | null = null;
  const unpress = () => {
    if (press) clearTimeout(press.timer);
    press = null;
  };
  // A tap's place and time, for a double tap; and when a double tap last zoomed (the double click
  // a browser may send for it as well isn't another zoom).
  let lastTap: { t: number; x: number; y: number } | null = null;
  let tapZoomed = -Infinity;
  // For `single`: when a finger's tap last ended, and the fingers down so far.
  const DOUBLE_MS = 300;
  let tapUp = -Infinity;
  let downs = 0;
  const letGo = () => {
    if (!hold) return;
    if (hold.moved && el.hasPointerCapture(hold.id)) el.releasePointerCapture(hold.id);
    el.classList.remove('grabbing');
    hold = null;
  };

  // Left-drag: the ground under the pointer at the start stays under the pointer (MapLibre's own
  // grab where the ground is close to the pivot plane). Clicks still reach MapLibre: nothing here
  // stops the mouse events.
  // A press soon after a tap, near it (tap then drag): dragging zooms about the tapped point.
  type TapDrag = { a: Anchor | null; x: number; y: number; lastY: number };
  let hold: { id: number; a: Anchor; own: boolean; moved: boolean; x0: number; y0: number; tapDrag?: TapDrag } | null = null;
  el.addEventListener('pointerdown', (e: PointerEvent) => {
    if (e.pointerType === 'touch') {
      downs++;
      // (A gesture's first finger: any left from one before, whose end wasn't seen, are gone.)
      if (e.isPrimary) touches.clear();
      touches.add(e.pointerId);
      if (touches.size > 1) {
        // Two fingers: pinch, turn, pan and tilt (below), the one-finger pan let go of.
        unpress();
        lastTap = null;
        letGo();
        if (touches.size === 2) startTwo();
        return;
      }
    }
    if (e.button !== 0 || e.ctrlKey || e.altKey || e.shiftKey || e.metaKey) return;
    const rect = el.getBoundingClientRect();
    const a = anchorAt(map, e.clientX - rect.left, e.clientY - rect.top);
    if (!a) return; // sky
    hold = { id: e.pointerId, a, own: a.ground && ownPan(map, a), moved: false, x0: e.clientX, y0: e.clientY };
    if (e.pointerType === 'touch' && lastTap && performance.now() - lastTap.t < DOUBLE_MS) {
      const px = e.clientX - rect.left, py = e.clientY - rect.top;
      if (Math.hypot(px - lastTap.x, py - lastTap.y) < 30) hold.tapDrag = { a: anchorAt(map, lastTap.x, lastTap.y), x: lastTap.x, y: lastTap.y, lastY: py };
    }
    zLeft = 0;
    stopInertia();
    if (e.pointerType === 'touch' && opts.onLongPress) {
      unpress();
      const px = e.clientX - rect.left, py = e.clientY - rect.top;
      press = {
        id: e.pointerId,
        timer: window.setTimeout(() => {
          press = null;
          // (Not a pan, and not a tap: the click it would end in is dropped, as a drag's.)
          dragged = true;
          letGo();
          opts.onLongPress?.(px, py);
        }, LONG_MS),
      };
    }
  });
  el.addEventListener('pointermove', (e: PointerEvent) => {
    if (!hold || e.pointerId !== hold.id) return;
    if (!hold.moved) {
      const d = Math.hypot(e.clientX - hold.x0, e.clientY - hold.y0);
      if (d < (e.pointerType === 'touch' ? 8 : 3)) return; // still a click (a finger wavers more)
      unpress();
      hold.moved = true;
      dragged = true;
      try {
        el.setPointerCapture(e.pointerId);
      } catch {
        /* pointer already gone */
      }
      el.classList.add('grabbing');
    }
    const rect = el.getBoundingClientRect();
    const x = e.clientX - rect.left, y = e.clientY - rect.top;
    const td = hold.tapDrag;
    if (td) {
      // Tap then drag: down zooms in, up out (MapLibre's rate, a level per 128 px), about the
      // tapped point; not a double tap.
      lastTap = null;
      zoomAt(td.x, td.y, (y - td.lastY) / 128, td.a);
      td.lastY = y;
      return;
    }
    if (!hold.own || !panTo(map, hold.a, x, y)) setLocationAt(map, hold.a.ll, x, y);
  });
  const release = (e: PointerEvent) => {
    touches.delete(e.pointerId);
    touchAt.delete(e.pointerId);
    if (two && (e.pointerId === two.ids[0] || e.pointerId === two.ids[1])) endTwo();
    if (press?.id === e.pointerId) unpress();
    if (!hold || e.pointerId !== hold.id) return;
    // A finger's tap, the second within DOUBLE_MS and 30 px of the first: zoom in there (its
    // click dropped, as a drag's).
    if (e.type === 'pointerup' && e.pointerType === 'touch' && !hold.moved && !dragged) {
      const rect = el.getBoundingClientRect();
      const x = e.clientX - rect.left, y = e.clientY - rect.top, now = performance.now();
      if (lastTap && now - lastTap.t < DOUBLE_MS && Math.hypot(x - lastTap.x, y - lastTap.y) < 30) {
        lastTap = null;
        tapZoomed = now;
        dragged = true;
        smoothZoom(x, y, 1);
      } else {
        lastTap = { t: now, x, y };
        tapUp = now;
      }
    }
    letGo();
  };
  el.addEventListener('pointerup', release);
  el.addEventListener('pointercancel', release);

  // ---- two fingers on a touch screen ----
  // The terrain point between the fingers (anchorAt at their midpoint) stays between them: a pinch
  // dollies the camera toward it (its distance divided by the spread's ratio, the same at every
  // zoom and over any terrain), a turn orbits it about the vertical, moving the fingers together
  // carries it with them (panTo), and dragging them up or down side by side tilts about it where
  // the tilt began. Steps are taken once a frame, from where the fingers are then.
  const touchAt = new Map<number, Pt>();
  let two: { ids: [number, number]; g: TwoFingers; a: Anchor | null; raf: number } | null = null;
  const where = (e: PointerEvent): Pt => {
    const rect = el.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  };
  el.addEventListener('pointerdown', (e: PointerEvent) => {
    if (e.pointerType === 'touch') touchAt.set(e.pointerId, where(e));
  }, true);
  el.addEventListener('pointermove', (e: PointerEvent) => {
    if (e.pointerType !== 'touch' || !touchAt.has(e.pointerId)) return;
    touchAt.set(e.pointerId, where(e));
    if (two && !two.raf && (e.pointerId === two.ids[0] || e.pointerId === two.ids[1])) two.raf = requestAnimationFrame(twoStep);
  }, true);
  const startTwo = () => {
    const ids = [...touches].filter((id) => touchAt.has(id)).slice(0, 2) as [number, number];
    if (ids.length < 2) return;
    endTwo();
    stopInertia();
    zLeft = 0;
    pinchSteps.length = 0;
    const g = new TwoFingers(touchAt.get(ids[0])!, touchAt.get(ids[1])!);
    two = { ids, g, a: anchorAt(map, g.start.x, g.start.y), raf: 0 };
  };
  const endTwo = () => {
    if (!two) return;
    cancelAnimationFrame(two.raf);
    two = null;
    pinchRelease(); // the zoom glides on, if the fingers were still spreading or closing
  };
  const twoStep = () => {
    if (!two) return;
    two.raf = 0;
    const pa = touchAt.get(two.ids[0]), pb = touchAt.get(two.ids[1]);
    if (!pa || !pb) return;
    const st = two.g.move(pa, pb, performance.now());
    if (!st) return;
    const { from, to } = st;
    if (st.dz || st.dBearing || st.dPitch || to.x !== from.x || to.y !== from.y) dragged = true;
    if (offGround(two.a, from.x, from.y)) two.a = anchorAt(map, from.x, from.y);
    const a = two.a;
    if (st.dz) pinchStep(from.x, from.y, st.dz, a, false);
    if (st.dBearing || st.dPitch) orbitAt(st.dBearing, st.dPitch, a, from.x, from.y);
    if (to.x !== from.x || to.y !== from.y) {
      // The ground between the fingers follows them (off the ground: MapLibre's pan).
      if (!a?.ground || !panTo(map, a, to.x, to.y)) map.panBy([from.x - to.x, from.y - to.y], { animate: false });
      if (pinchAt) pinchAt = { x: to.x, y: to.y, a };
    }
  };

  // Safari pinch (relative to the previous event: the zoom number itself can be re-levelled): a
  // trackpad's. On a touch screen Safari sends these gesture events for two fingers as well; that
  // pinch, turn and tilt are the two-finger gesture's (above), so these only keep Safari from
  // zooming the page.
  let lastScale = 1;
  let gx = 0, gy = 0;
  let ga: Anchor | null = null;
  const g = el as unknown as { addEventListener: (t: string, f: (e: any) => void, o?: any) => void };
  g.addEventListener('gesturestart', (e: any) => {
    e.preventDefault();
    lastScale = 1;
    const rect = el.getBoundingClientRect();
    gx = e.clientX - rect.left;
    gy = e.clientY - rect.top;
    ga = anchorAt(map, gx, gy);
  }, { passive: false });
  g.addEventListener('gesturechange', (e: any) => {
    e.preventDefault();
    if (touches.size > 0) return;
    const dz = Math.log2(e.scale / lastScale);
    lastScale = e.scale;
    pinchStep(gx, gy, dz, ga);
  }, { passive: false });
  g.addEventListener('gestureend', (e: any) => {
    e.preventDefault();
    if (touches.size > 0) return;
    clearTimeout(pinchEnd);
    pinchRelease();
  }, { passive: false });

  // Double-click zooms in about the point (Shift: out).
  el.addEventListener('dblclick', (e: MouseEvent) => {
    if (performance.now() - tapZoomed < 600) return;
    const rect = el.getBoundingClientRect();
    smoothZoom(e.clientX - rect.left, e.clientY - rect.top, e.shiftKey ? -1 : 1);
  });

  let orbRaf = 0;
  return {
    zoomBy(dz, px, py) {
      const c = centrePoint(map);
      smoothZoom(px ?? c.x, py ?? c.y, dz);
    },
    single(f) {
      // (A click comes straight after the pointerup that ends its tap.)
      if (performance.now() - tapUp > 150) return f();
      const d = downs;
      window.setTimeout(() => downs === d && f(), DOUBLE_MS);
    },
    orbitBy(dBearing, dPitch) {
      cancelAnimationFrame(orbRaf);
      const c = centrePoint(map);
      const a = anchorAt(map, c.x, c.y);
      const t0 = performance.now();
      let done = 0;
      const tick = (now: number) => {
        const k = Math.min(1, (now - t0) / 350);
        const e = 1 - (1 - k) ** 3;
        orbitAt(dBearing * (e - done), dPitch * (e - done), a, c.x, c.y);
        done = e;
        if (k < 1) orbRaf = requestAnimationFrame(tick);
      };
      orbRaf = requestAnimationFrame(tick);
    },
  };
}
