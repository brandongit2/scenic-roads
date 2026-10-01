// How the landmark dots (heritage sites, stops and sights) are sized and faded along their score
// (the settings panel's Stops & sights section, ui/layers.ts, which has the kinds shown, their
// filters and the opacity).
import type { Dist } from '../roads/stats';
import { OVERLAYS, defaults, type AppState, type Store } from '../state';
import * as prefs from '../prefs';
import { Slider } from './controls';
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
  private scale: ScaleControls;
  private sliders: Slider[];

  constructor(private store: Store) {
    const L = (patch: Partial<AppState['landmarks']>) => store.set({ landmarks: { ...store.s.landmarks, ...patch } });
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
    this.sliders = [
      new Slider({
        label: 'Fame ↔ rarity', min: 0, max: 1, step: 0.05, reset: defaults.landmarks.balance,
        title: 'What makes a landmark prominent: how well known it is (Wikipedia pageviews) or how rare it is nearby (distance to a better-known one of its kind)',
        get: () => store.s.landmarks.balance, set: (balance) => L({ balance }),
        fmt: (b) => (b <= 0 ? 'fame' : b >= 1 ? 'rarity' : `${Math.round((1 - b) * 100)}:${Math.round(b * 100)}`),
      }),
      new Slider({
        label: 'Size contrast', min: 0, max: 1, step: 0.05, reset: defaults.poiEmphasis,
        title: 'How much dot size varies along the scale. 0: all dots the same size',
        get: () => store.s.poiEmphasis, set: (poiEmphasis) => store.set({ poiEmphasis }), fmt: (v) => `${Math.round(v * 100)} %`,
      }),
    ];

    this.el = h('div', { class: 'rail-card stops-card' },
      h('div', { class: 'rail-bd' },
        h('div', { class: 'lm-scale', title: 'Landmark score in view (0–100): how well known (Wikipedia pageviews) and how rare nearby (distance to a better-known one of its kind). Dots are sized and faded along this scale.' },
          this.scale.legend, this.scale.fadeRow, this.scale.thrRow),
        ...this.sliders.map((x) => x.el),
      ),
    );
    this.sync();
  }

  sync() {
    const s = this.store.s;
    const nOn = OVERLAYS.filter(([k]) => s.overlays[k]).length;
    this.el.classList.toggle('off', nOn === 0);
    for (const x of this.sliders) x.sync();
    this.scale.sync();
  }

  /** The landmark scores in view, the range in use and the lookup (if equalising). */
  update(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.scale.update(dist, range, cdf);
  }
}
