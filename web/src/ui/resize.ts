// Drag handles: the settings panel's inner edge (its width, --left-w) and the floating lists' top
// edge (their height, --lists-h) follow the pointer, within limits that leave room for the map,
// and are remembered. Double-click a handle for the default.
import type { Map as MLMap } from 'maplibre-gl';
import * as prefs from '../prefs';

interface Side {
  key: 'left' | 'right';
  cssVar: string;
  min: number;
}

const SIDES: Side[] = [{ key: 'left', cssVar: '--left-w', min: 260 }];
/** At most this share of the window, and at least this much map beside it. */
const MAX_SHARE = 0.45;
const MIN_MAP = 320;

export function installPanelResize(map: MLMap) {
  const root = document.documentElement;
  const saved = prefs.load<Partial<Record<Side['key'], number>>>('ui.panels', {});
  const width = (s: Side) => parseFloat(getComputedStyle(root).getPropertyValue(s.cssVar)) || s.min;
  const clamp = (s: Side, w: number) => {
    const max = Math.min(window.innerWidth * MAX_SHARE, window.innerWidth - MIN_MAP);
    return Math.round(Math.max(s.min, Math.min(max, w)));
  };
  const apply = (s: Side, w: number | null) => {
    if (w === null) root.style.removeProperty(s.cssVar);
    else root.style.setProperty(s.cssVar, `${clamp(s, w)}px`);
    map.resize();
  };
  for (const s of SIDES) if (saved[s.key]) apply(s, saved[s.key]!);

  for (const s of SIDES) {
    const handle = document.createElement('div');
    handle.className = `resizer ${s.key}`;
    handle.title = 'Drag to resize (double-click: default width)';
    document.body.append(handle);
    let drag: { x: number; w: number } | null = null;
    let raf = 0;
    handle.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      drag = { x: e.clientX, w: width(s) };
      handle.setPointerCapture(e.pointerId);
      handle.classList.add('on');
      document.body.classList.add('resizing');
      e.preventDefault();
    });
    handle.addEventListener('pointermove', (e) => {
      if (!drag) return;
      const dx = e.clientX - drag.x;
      const w = drag.w + dx;
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => apply(s, w));
    });
    const end = (e: PointerEvent) => {
      if (!drag) return;
      drag = null;
      handle.releasePointerCapture(e.pointerId);
      handle.classList.remove('on');
      document.body.classList.remove('resizing');
      saved[s.key] = width(s);
      prefs.save('ui.panels', saved);
    };
    handle.addEventListener('pointerup', end);
    handle.addEventListener('pointercancel', end);
    handle.addEventListener('dblclick', () => {
      delete saved[s.key];
      prefs.save('ui.panels', saved);
      apply(s, null);
    });
  }
  // A smaller window: keep the panel within its limits.
  window.addEventListener('resize', () => {
    for (const s of SIDES) if (root.style.getPropertyValue(s.cssVar)) apply(s, width(s));
  });
}

/** The floating lists' height (--lists-h): their top edge dragged, remembered; double-click for
 * the default. */
export function installListsResize(panel: HTMLElement) {
  const root = document.documentElement;
  const MIN = 140;
  const clamp = (h: number) => Math.round(Math.max(MIN, Math.min(window.innerHeight - 140, h)));
  const saved = prefs.load<number | null>('ui.listsH', null);
  if (saved) root.style.setProperty('--lists-h', `${clamp(saved)}px`);
  const grip = document.createElement('div');
  grip.className = 'lists-grip';
  grip.title = 'Drag to resize (double-click: default height)';
  panel.append(grip);
  let drag: { y: number; h: number } | null = null;
  let raf = 0;
  grip.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    drag = { y: e.clientY, h: panel.getBoundingClientRect().height };
    grip.setPointerCapture(e.pointerId);
    grip.classList.add('on');
    document.body.classList.add('resizing-v');
    e.preventDefault();
  });
  grip.addEventListener('pointermove', (e) => {
    if (!drag) return;
    const h = drag.h - (e.clientY - drag.y);
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(() => root.style.setProperty('--lists-h', `${clamp(h)}px`));
  });
  const end = (e: PointerEvent) => {
    if (!drag) return;
    drag = null;
    grip.releasePointerCapture(e.pointerId);
    grip.classList.remove('on');
    document.body.classList.remove('resizing-v');
    prefs.save('ui.listsH', panel.getBoundingClientRect().height);
  };
  grip.addEventListener('pointerup', end);
  grip.addEventListener('pointercancel', end);
  grip.addEventListener('dblclick', () => {
    prefs.save('ui.listsH', null);
    root.style.removeProperty('--lists-h');
  });
}
