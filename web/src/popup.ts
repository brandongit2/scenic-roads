// The map's click popups (landmarks, ferries): one at a time per owner, dark, kept inside the map,
// and the rows they are made of.
//
// Kept inside the map: above its point if it fits there, else below, else on the side with more
// room, shifted right or left near an edge (or, wider than either side, moved inside); its height
// capped to that room with the rest scrolling inside. Again whenever its content changes size
// (details arrive after it opens) and after the map moves (MapLibre only picks a side when the
// popup opens, for its size then).
import * as maplibregl from 'maplibre-gl';
import type { LngLatLike, Map as MLMap, Popup } from 'maplibre-gl';
import { h } from './ui/dom';

/** Kept clear of the map's edges (px). */
const MARGIN = 10;
/** The popup's offset from its point (its `offset` option, as made), and that with its tip. */
const OFFSET = 8;
const GAP = 18;
/** Its padding and border (.dark-pop). */
const CHROME = 24;

/** `content` in a scroller (`el`, the popup's content) and `fit`, which places the popup (call once
 * it is on the map); it places itself again until it closes. */
function fitPopup(map: MLMap, popup: Popup, content: HTMLElement): { el: HTMLElement; fit: () => void } {
  const el = document.createElement('div');
  el.className = 'pop-scroll';
  el.append(content);
  let frame = 0;
  const fit = () => {
    frame = 0;
    if (!popup.isOpen()) return;
    const c = map.getContainer();
    const p = map.project(popup.getLngLat());
    const above = p.y - GAP - MARGIN, below = c.clientHeight - p.y - GAP - MARGIN;
    const natural = content.offsetHeight + CHROME;
    const up = natural <= above || (natural > below && above >= below);
    el.style.maxHeight = `${Math.max(60, (up ? above : below) - CHROME)}px`;
    const w = popup.getElement()?.offsetWidth ?? 320, cw = c.clientWidth;
    // Centred on its point; or from it rightwards or leftwards near an edge; or, too wide for any of
    // those (a phone), centred and then shifted inside, its tip (no longer at the point) hidden.
    let side = '', dx = 0;
    if (p.x - w / 2 < MARGIN || p.x + w / 2 > cw - MARGIN) {
      if (p.x < cw / 2 && p.x + w <= cw - MARGIN) side = '-left';
      else if (p.x >= cw / 2 && p.x - w >= MARGIN) side = '-right';
      else dx = Math.round(Math.max(MARGIN, Math.min(cw - w - MARGIN, p.x - w / 2)) - (p.x - w / 2));
    }
    const anchor = `${up ? 'bottom' : 'top'}${side}`;
    // (no setter for the anchor: MapLibre reads its option on every update)
    const opts = (popup as unknown as { options: { anchor?: string; offset?: unknown } }).options;
    const offset = dx ? [dx, up ? -OFFSET : OFFSET] : OFFSET;
    if (opts.anchor !== anchor || JSON.stringify(opts.offset) !== JSON.stringify(offset)) {
      opts.anchor = anchor;
      popup.getElement()?.classList.toggle('pop-shifted', dx !== 0);
      popup.setOffset(offset as number);
    }
  };
  const soon = () => {
    if (!frame) frame = requestAnimationFrame(fit);
  };
  const ro = new ResizeObserver(soon);
  ro.observe(content);
  map.on('moveend', soon);
  const stop = () => {
    ro.disconnect();
    map.off('moveend', soon);
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
  };
  popup.on('close', stop);
  return { el, fit };
}

/** One popup on the map at a time: showing another closes the one open. */
export class MapPopup {
  private popup: Popup | null = null;

  constructor(private map: MLMap) {}

  show(at: LngLatLike, content: HTMLElement) {
    this.close();
    const popup = new maplibregl.Popup({ closeButton: true, maxWidth: '320px', className: 'dark-pop', offset: OFFSET });
    const fit = fitPopup(this.map, popup, content);
    this.popup = popup.setLngLat(at).setDOMContent(fit.el).addTo(this.map);
    fit.fit();
  }

  close() {
    this.popup?.remove();
    this.popup = null;
  }
}

/** A popup's "key: value" row; none for a value missing or empty. */
export const kv = (k: string, v: unknown): HTMLElement | null =>
  v === undefined || v === null || v === '' ? null : h('div', { class: 'kv' }, h('span', {}, k), h('b', {}, String(v)));

/** A popup's link out ("label ↗", a new tab); none without a URL. */
export const link = (url: string | undefined, label: string): HTMLElement | null =>
  url ? h('a', { href: url, target: '_blank', rel: 'noopener' }, `${label} ↗`) : null;
