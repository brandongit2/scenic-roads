import type { Map as MLMap } from 'maplibre-gl';
import { anchorAt, centrePoint, dolly, ownPan, orbit, panTo, setLocationAt, type Anchor } from './camera3d';

/**
 * Trackpad-first navigation, anchored at the cursor:
 *   two-finger drag            → pan         (wheel events without modifiers; the ground at the
 *                                              view centre moves with the fingers)
 *   click-drag                 → pan         (the grabbed ground stays under the pointer)
 *   pinch                      → zoom        (wheel + ctrlKey in Chromium/Firefox, GestureEvent in Safari)
 *   ⌥ Option + two-finger drag → x rotates, y tilts
 *   right-drag / Ctrl-drag     → rotate + tilt
 * A mouse wheel zooms with short, snappy easing. Zoom, rotation and tilt keep the 3D point
 * under the cursor fixed on screen (see camera3d.ts).
 */
export interface CameraControls {
  /** Smooth zoom by dz levels about a screen point (default: the view centre). */
  zoomBy(dz: number, px?: number, py?: number): void;
  /** Animated rotate / tilt about the ground at the view centre. */
  orbitBy(dBearing: number, dPitch: number): void;
}

export function installTrackpad(map: MLMap): CameraControls {
  map.scrollZoom.disable();
  map.doubleClickZoom.disable(); // replaced by the cursor-anchored zoom below
  map.dragRotate.disable(); // replaced by the cursor-anchored orbit below
  map.dragPan.disable(); // replaced by the ground-anchored pan below
  const el = map.getCanvasContainer();
  let lastEvent = 0;
  let burstMouse = false;
  // Anchor for the current gesture burst, re-picked when the cursor moves.
  let anchor: { a: Anchor | null; x: number; y: number; t: number } | null = null;
  const anchorFor = (px: number, py: number): Anchor | null => {
    const now = performance.now();
    let stale = !anchor || now - anchor.t > 300 || Math.hypot(px - anchor.x, py - anchor.y) > 3;
    // Finer terrain tiles can change the ground under the cursor mid-gesture: re-pick when the
    // anchor no longer sits on it.
    if (!stale && anchor?.a?.ground) {
      const q = map.project(anchor.a.ll);
      const e = map.queryTerrainElevation(anchor.a.ll);
      if (Math.hypot(q.x - px, q.y - py) > 6 || (e !== null && Math.abs(e - anchor.a.elev) > 3)) stale = true;
    }
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
    wheel = { x: px, y: py, a: anchorFor(px, py) };
    zLeft += dz;
    if (!raf) {
      last = performance.now();
      raf = requestAnimationFrame(step);
    }
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
      const looksMouse = e.deltaMode === 1 || (e.deltaX === 0 && !!wdy && Math.abs(wdy) % 120 === 0 && wdy !== -3 * e.deltaY);
      if (now - lastEvent > 280) burstMouse = looksMouse;
      else if (e.deltaX !== 0 && !e.altKey) burstMouse = false;
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
        zLeft = 0;
        zoomAt(px, py, -dy * 0.012, anchorFor(px, py));
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

  // Left-drag: the ground under the pointer at the start stays under the pointer (MapLibre's own
  // grab where the ground is close to the pivot plane). Clicks still reach MapLibre: nothing here
  // stops the mouse events.
  let hold: { id: number; a: Anchor; own: boolean; moved: boolean; x0: number; y0: number } | null = null;
  el.addEventListener('pointerdown', (e: PointerEvent) => {
    if (e.button !== 0 || e.ctrlKey || e.altKey || e.shiftKey || e.metaKey) return;
    const rect = el.getBoundingClientRect();
    const a = anchorAt(map, e.clientX - rect.left, e.clientY - rect.top);
    if (!a) return; // sky
    hold = { id: e.pointerId, a, own: a.ground && ownPan(map, a), moved: false, x0: e.clientX, y0: e.clientY };
    zLeft = 0;
  });
  el.addEventListener('pointermove', (e: PointerEvent) => {
    if (!hold || e.pointerId !== hold.id) return;
    if (!hold.moved) {
      if (Math.hypot(e.clientX - hold.x0, e.clientY - hold.y0) < 3) return; // still a click
      hold.moved = true;
      try {
        el.setPointerCapture(e.pointerId);
      } catch {
        /* pointer already gone */
      }
      el.classList.add('grabbing');
    }
    const rect = el.getBoundingClientRect();
    const x = e.clientX - rect.left, y = e.clientY - rect.top;
    if (!hold.own || !panTo(map, hold.a, x, y)) setLocationAt(map, hold.a.ll, x, y);
  });
  const release = (e: PointerEvent) => {
    if (!hold || e.pointerId !== hold.id) return;
    if (hold.moved && el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
    el.classList.remove('grabbing');
    hold = null;
  };
  el.addEventListener('pointerup', release);
  el.addEventListener('pointercancel', release);

  // Safari pinch (relative to the previous event: the zoom number itself can be re-levelled).
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
    const dz = Math.log2(e.scale / lastScale);
    lastScale = e.scale;
    zoomAt(gx, gy, dz, ga);
  }, { passive: false });
  g.addEventListener('gestureend', (e: any) => e.preventDefault(), { passive: false });

  // Double-click zooms in about the point (Shift: out).
  el.addEventListener('dblclick', (e: MouseEvent) => {
    const rect = el.getBoundingClientRect();
    smoothZoom(e.clientX - rect.left, e.clientY - rect.top, e.shiftKey ? -1 : 1);
  });

  let orbRaf = 0;
  return {
    zoomBy(dz, px, py) {
      const c = centrePoint(map);
      smoothZoom(px ?? c.x, py ?? c.y, dz);
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
