import type { Map as MLMap } from 'maplibre-gl';
import { anchorAt, dolly, orbit, type Anchor } from './camera3d';

/**
 * Trackpad-first navigation, anchored at the cursor:
 *   two-finger drag            → pan         (wheel events without modifiers)
 *   pinch                      → zoom        (wheel + ctrlKey in Chromium/Firefox, GestureEvent in Safari)
 *   ⌥ Option + two-finger drag → x rotates, y tilts
 *   right-drag / Ctrl-drag     → rotate + tilt
 * A mouse wheel zooms with short, snappy easing. Zoom, rotation and tilt keep the 3D point
 * under the cursor fixed on screen (see camera3d.ts).
 */
export function installTrackpad(map: MLMap) {
  map.scrollZoom.disable();
  map.dragRotate.disable(); // replaced by the cursor-anchored orbit below
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
    map.jumpTo({ bearing: map.getBearing() + dBearing, pitch: Math.max(0, Math.min(map.getMaxPitch(), map.getPitch() + dPitch)) });
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
  map.on('movestart', (e) => {
    // A user drag cancels a running wheel zoom.
    if ((e as { originalEvent?: Event }).originalEvent?.type === 'mousedown') zLeft = 0;
  });

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
        dx *= rect.height;
        dy *= rect.height;
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
        map.panBy([dx, dy], { animate: false });
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
}
