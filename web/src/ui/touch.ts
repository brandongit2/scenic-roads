// Phones and tablets (docs/plan.md §4, Devices): the settings panel folding away, and a double tap
// where a mouse would double-click. Nothing here asks what the device is: a narrow window folds the
// panel, a coarse pointer gets the touch hints, any touch screen the double tap.
import type { Map as MLMap } from 'maplibre-gl';
import * as prefs from '../prefs';

/** A finger rather than a pointer (the hints, what a tap does). */
export const coarse = (): boolean => matchMedia('(pointer: coarse)').matches;

/** The settings panel: on a narrow or short window (a phone) always folded away, brought over the map by the button at
 * the top left (#menu; a tap on the scrim, the panel's ‹ or Escape puts it away); on a wider one
 * docked, or folded by its ‹ (remembered) and docked again by the button. */
export function installPanel(map: MLMap): void {
  const body = document.body;
  const menu = document.getElementById('menu') as HTMLButtonElement;
  const scrim = document.getElementById('scrim')!;
  const panel = document.getElementById('colour')!;
  // (A phone either way up: narrow, or short.)
  const narrow = matchMedia('(max-width: 700px), (max-height: 500px)');
  let folded = prefs.load<boolean>('ui.panelOff', false);
  const apply = (open = body.classList.contains('panel-open')) => {
    const off = narrow.matches || folded;
    const was = body.classList.contains('panel-off');
    body.classList.toggle('panel-off', off);
    body.classList.toggle('panel-open', off && open);
    scrim.hidden = !(off && open);
    menu.setAttribute('aria-expanded', String(!off || open));
    // (The map's room changes with the panel docked or not: MapLibre follows its container, at
    // once rather than a frame late.)
    if (was !== off) map.resize();
  };
  menu.onclick = () => {
    if (narrow.matches) return apply(!body.classList.contains('panel-open'));
    folded = false;
    prefs.save('ui.panelOff', false);
    apply(false);
  };
  scrim.onclick = () => apply(false);
  const fold = document.createElement('button');
  fold.className = 'fold';
  fold.type = 'button';
  fold.title = 'Fold the settings away';
  fold.setAttribute('aria-label', fold.title);
  fold.textContent = '‹';
  fold.onclick = () => {
    if (narrow.matches) return apply(false);
    folded = true;
    prefs.save('ui.panelOff', true);
    apply(false);
  };
  // (On the panel's title, once the settings card has made it.)
  const place = () => {
    const t = panel.querySelector('.title');
    if (t && !t.contains(fold)) t.append(fold);
    return !!t;
  };
  if (!place()) {
    const watch = new MutationObserver(() => place() && watch.disconnect());
    watch.observe(panel, { childList: true, subtree: true });
  }
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && body.classList.contains('panel-open')) apply(false);
  });
  narrow.addEventListener('change', () => apply(false));
  apply(false);
}

/** `f` on a double click, and on a double tap (two taps within 350 ms and 24 px: a touch screen
 * sends no double click). */
export function onDouble(el: HTMLElement, f: () => void): void {
  el.addEventListener('dblclick', f);
  let last: { t: number; x: number; y: number } | null = null;
  el.addEventListener('pointerup', (e) => {
    if (e.pointerType !== 'touch') return;
    const now = performance.now();
    if (last && now - last.t < 350 && Math.hypot(e.clientX - last.x, e.clientY - last.y) < 24) {
      last = null;
      f();
    } else {
      last = { t: now, x: e.clientX, y: e.clientY };
    }
  });
}
