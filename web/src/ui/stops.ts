// "Stops & sights" section of the top-left panel: how the landmark dots (heritage sites, stops and
// sights) are sized and faded along their score. Which kinds show, their filters and opacity: the
// Layers panel.
import type { Dist } from '../roads/stats';
import { OVERLAYS, defaults, type AppState, type Store } from '../state';
import * as prefs from '../prefs';
import { h } from './dom';
import { ScaleControls } from './scale';

/** Every stop & sight kind at once: off remembers the kinds that were on; on brings them back (all
 * of them if none were remembered). */
export function toggleAllStops(store: Store) {
  const S = store.s;
  const on = OVERLAYS.filter(([k]) => S.overlays[k]).map(([k]) => k);
  if (on.length) prefs.save('stops.lastOn', on);
  const keep = prefs.load<string[]>('stops.lastOn', []).filter((k) => OVERLAYS.some(([o]) => o === k));
  const next = on.length ? [] : keep.length ? keep : OVERLAYS.map(([k]) => k);
  store.set({ overlays: Object.fromEntries(OVERLAYS.map(([k]) => [k, next.includes(k)])) as AppState['overlays'] });
}

export class StopsCard {
  el: HTMLElement;
  private on: HTMLInputElement;
  private scale: ScaleControls;
  private bal: HTMLInputElement;
  private balOut: HTMLOutputElement;
  private em: HTMLInputElement;
  private emOut: HTMLOutputElement;

  constructor(private store: Store) {
    const L = (patch: Partial<AppState['landmarks']>) => store.set({ landmarks: { ...store.s.landmarks, ...patch } });
    this.on = h('input', { type: 'checkbox', title: 'Every kind at once. Off hides them all; on brings back the kinds you had on' });
    this.on.addEventListener('change', () => toggleAllStops(store));
    this.scale = new ScaleControls({
      get: () => store.s.landmarks,
      set: L,
      metric: () => ({ domain: [0, 1], step: 0.01, fmt: (v) => String(Math.round(v * 100)) }),
      noun: 'landmarks',
      measure: 'landmarks',
      rank: { get: () => store.s.landmarks.top, set: (top) => L({ top }) },
      fadeDefault: defaults.landmarks.lowFade,
      spanDefault: defaults.landmarks.lowSpan,
      onPreview: () => {},
    });
    const slider = (on: (v: number) => void, reset: number, title: string) => {
      const e = h('input', { type: 'range', min: 0, max: 1, step: 0.05, title: `${title} (double-click: default)` });
      e.addEventListener('input', () => on(Number(e.value)));
      e.addEventListener('dblclick', () => on(reset));
      return e;
    };
    this.bal = slider((v) => L({ balance: v }), defaults.landmarks.balance,
      'What makes a landmark prominent: how well known it is (Wikipedia pageviews) or how rare it is nearby (distance to a better-known one of its kind)');
    this.balOut = h('output');
    this.em = slider((v) => store.set({ poiEmphasis: v }), defaults.poiEmphasis, 'How much dot size varies along the scale. 0: all dots the same size');
    this.emOut = h('output');

    this.el = h('div', { class: 'rail-card stops-card' },
      h('label', { class: 'rail-hd' }, this.on, h('span', {}, 'Stops & sights'), h('span', { class: 'faint' }, 'Layers → kinds, opacity')),
      h('div', { class: 'rail-bd' },
        h('div', { class: 'lm-scale', title: 'Landmark score in view (0–100): how well known (Wikipedia pageviews) and how rare nearby (distance to a better-known one of its kind). Dots are sized and faded along this scale.' },
          this.scale.legend, this.scale.fadeRow, this.scale.thrRow),
        h('div', { class: 'fade lm-more' },
          h('span', { class: 'muted' }, 'Fame ↔ rarity'), this.bal, this.balOut,
          h('span', { class: 'muted' }, 'Size contrast'), this.em, this.emOut),
      ),
    );
    this.sync();
  }

  sync() {
    const s = this.store.s;
    const nOn = OVERLAYS.filter(([k]) => s.overlays[k]).length;
    this.on.checked = nOn > 0;
    this.on.indeterminate = nOn > 0 && nOn < OVERLAYS.length;
    this.el.classList.toggle('off', nOn === 0);
    const lm = s.landmarks;
    this.bal.value = String(lm.balance);
    this.balOut.value = lm.balance <= 0 ? 'fame' : lm.balance >= 1 ? 'rarity' : `${Math.round((1 - lm.balance) * 100)}:${Math.round(lm.balance * 100)}`;
    this.em.value = String(s.poiEmphasis);
    this.emOut.value = `${Math.round(s.poiEmphasis * 100)} %`;
    this.scale.sync();
  }

  /** The landmark scores in view, the range in use and the lookup (if equalising). */
  update(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.scale.update(dist, range, cdf);
  }
}
