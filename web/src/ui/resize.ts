// Drag handles on the inner edges of the left and right panels: their widths (the --left-w and
// --side-w variables the layout uses) follow the pointer, within limits that always leave room for
// the map, and are remembered. Double-click a handle for the default width.
import type { Map as MLMap } from 'maplibre-gl';
import * as prefs from '../prefs';

interface Side {
  key: 'left' | 'right';
  cssVar: string;
  min: number;
}

const SIDES: Side[] = [
  { key: 'left', cssVar: '--left-w', min: 260 },
  { key: 'right', cssVar: '--side-w', min: 240 },
];
/** At most this share of the window per panel, and at least this much map between them. */
const MAX_SHARE = 0.4;
const MIN_MAP = 320;

export function installPanelResize(map: MLMap) {
  const root = document.documentElement;
  const saved = prefs.load<Partial<Record<Side['key'], number>>>('ui.panels', {});
  const width = (s: Side) => parseFloat(getComputedStyle(root).getPropertyValue(s.cssVar)) || s.min;
  const clamp = (s: Side, w: number) => {
    const other = SIDES.find((o) => o !== s)!;
    const max = Math.min(window.innerWidth * MAX_SHARE, window.innerWidth - width(other) - MIN_MAP);
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
      const w = drag.w + (s.key === 'left' ? dx : -dx);
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
  // A smaller window: keep the panels within their limits.
  window.addEventListener('resize', () => {
    for (const s of SIDES) if (root.style.getPropertyValue(s.cssVar)) apply(s, width(s));
  });
}
