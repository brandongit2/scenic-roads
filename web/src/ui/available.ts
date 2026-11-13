// The layers panel's Data available section (ui/layers.ts) and the click's panel: the themes'
// layers from the server's description (available.ts), one shown at a time, the set, the
// availability switch, the opacity, and the layer's colour map: the sidebar's own control
// (ui/scale.ts ScaleControls) over the values in view, on the layer's own axis (available.ts axisOf).
import { axisOf, scaleOf, shown, word, type AvailableState, type AvLayer, type AvTheme, type PieceRow, OPACITY_RANGE } from '../available';
import type { Dist } from '../roads/stats';
import type { ScaleFields, Store } from '../state';
import { Slider, pct } from './controls';
import { h } from './dom';
import { ScaleControls } from './scale';
import { inUse } from './settingprofiles';

/** A theme as the section has it: its description, or where that stands. */
export type ThemeEntry = { theme: string; desc: AvTheme | 'loading' | 'failed' };

export class AvailableSection {
  readonly nodes: HTMLElement[];
  /** The group's switch (the section's header). */
  readonly on: HTMLInputElement;
  /** A palette shown on the map while hovering the ramp list (null: back to the chosen one). */
  onPreview: (palette: string | null) => void = () => {};
  private list = h('div', { class: 'av-layers' });
  private setRow: HTMLElement;
  private setSeg = h('div', { class: 'seg small' });
  private availBtns: HTMLButtonElement[] = [];
  private op: Slider;
  private scale: ScaleControls;
  private scaleBox: HTMLElement;
  private body: HTMLDivElement;
  private themes: ThemeEntry[] = [];
  /** The layer shown (its axis is the colour map's), null before a theme is read. */
  private layer: AvLayer | null = null;

  constructor(private store: Store) {
    const A = (patch: Partial<AvailableState>) => store.set({ available: { ...store.s.available, ...patch } });
    this.on = h('input', { type: 'checkbox', title: 'Where the best terrain data anyone publishes is, and how good it is' });
    this.on.addEventListener('change', () => A({ on: this.on.checked }));
    const avail = h('div', { class: 'seg small' });
    for (const [k, label, title] of [
      ['open', 'Open data', 'Only data that downloads freely, with no login, request or payment'],
      ['all', 'All data', 'Every source, also those behind a login or a request, paid or not to be had'],
    ] as const) {
      const b = h('button', { title, onclick: () => A({ avail: k }) }, label);
      this.availBtns.push(b);
      avail.append(b);
    }
    this.op = new Slider({
      label: 'Opacity', min: OPACITY_RANGE[0], max: OPACITY_RANGE[1], step: 0.05, reset: () => inUse().available.opacity, title: 'Opacity of the layer',
      get: () => store.s.available.opacity, set: (opacity) => A({ opacity }), fmt: pct,
    });
    // The colour map of the layer shown: each layer keeps its own.
    const S = (): ScaleFields => (this.layer ? scaleOf(store.s.available, this.layer) : store.s.available.colours);
    const axis = () => (this.layer ? axisOf(this.layer) : null);
    this.scale = new ScaleControls({
      get: S,
      set: (patch) => {
        const l = this.layer;
        if (l) A({ scales: { ...store.s.available.scales, [l.id]: { ...S(), ...patch } } });
      },
      metric: () => {
        const a = axis();
        return a ? { domain: a.domain, step: a.step, fmt: a.fmt, ticks: a.ticks } : { domain: [0, 1], step: 0.01, fmt: (v: number) => v.toFixed(2) };
      },
      noun: 'data',
      measure: 'area with data',
      fadeDefault: () => inUse().available.colours.lowFade,
      spanDefault: () => inUse().available.colours.lowSpan,
      // (A layer of words: its range their values, the ramp's ends theirs.)
      fixedCaption: () => (this.layer && axis()?.fixed ? `${this.layer.words.map((w) => w.label).join(' and ')}: the ramp’s ends` : null),
      // (Its words are the range's ends: equalized, each still takes its end's colour.)
      equalizable: () => !axis()?.fixed,
      onPreview: (k) => this.onPreview(k),
    });
    const row = (label: string, input: HTMLElement) => h('div', { class: 'row' }, h('span', { class: 'muted' }, label), input, h('span'));
    this.setRow = row('Data', this.setSeg);
    this.scaleBox = h('div', { class: 'tint-scale' }, this.scale.legend, this.scale.palRow, this.scale.fadeRow, this.scale.thrRow);
    this.body = h('div', { class: 'av-body' },
      this.list,
      this.setRow,
      row('Show', avail),
      this.op.el,
      this.scaleBox,
      h('div', { class: 'faint note' }, 'Zoomed out, a pixel shows the best data anywhere under it, so small areas of fine data stay in sight. Click the map for every source there. Places nobody has looked into yet show nothing.'),
    );
    this.nodes = [this.body];
  }

  /** The themes the catalogue lists, each with its description or where that stands. */
  setThemes(themes: ThemeEntry[]) {
    this.themes = themes;
    this.sync();
  }

  /** The values in view, the range in use and the equalization lookup (ctl/available.ts). */
  update(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.scale.update(dist, range, cdf);
  }

  sync() {
    const s = this.store.s.available;
    this.on.checked = s.on;
    this.body.classList.toggle('off', !s.on);
    this.availBtns.forEach((b, i) => b.classList.toggle('on', ['open', 'all'][i] === s.avail));
    this.op.sync();
    const A = (patch: Partial<AvailableState>) => this.store.set({ available: { ...this.store.s.available, ...patch } });
    // The layers, a theme's under its name when there are several; the one shown ticked.
    const cur = this.themes.find((t) => t.theme === s.theme) ?? this.themes[0];
    const rows: HTMLElement[] = [];
    for (const t of this.themes) {
      if (this.themes.length > 1) rows.push(h('div', { class: 'av-theme muted' }, t.theme));
      if (t.desc === 'loading') rows.push(h('div', { class: 'muted av-wait' }, h('span', { class: 'spin' }), ' Reading the layers…'));
      else if (t.desc === 'failed') rows.push(h('div', { class: 'muted av-wait' }, 'The layers could not be read; asked again in a minute.'));
      else {
        const sel = t === cur ? shown(s, t.desc).layer.id : null;
        for (const l of t.desc.layers) {
          const r = h('input', { type: 'radio', name: 'av-layer' });
          r.checked = l.id === sel;
          r.addEventListener('change', () => A({ theme: t.theme, layer: l.id }));
          rows.push(h('label', { class: `tog${l === t.desc.layers[0] ? ' av-primary' : ''}` }, r, h('span', {}, l.label)));
        }
      }
    }
    this.list.replaceChildren(...rows);
    // The set: shown once a theme has more than one.
    const desc = cur && typeof cur.desc === 'object' ? cur.desc : null;
    const sets = desc?.sets ?? [];
    this.setRow.hidden = sets.length < 2;
    const set = desc ? shown(s, desc).set : '';
    this.setSeg.replaceChildren(...sets.map((k) => h('button', { class: k === set ? 'on' : '', onclick: () => A({ set: k }) }, word(desc ? desc.labels.sets : {}, k))));
    this.layer = desc ? shown(s, desc).layer : null;
    this.scaleBox.hidden = !this.layer;
    this.scale.sync();
  }
}

/** Why a click picks no road or area while the group is on. */
const note = () => h('div', { class: 'faint av-note' }, 'Roads and areas aren’t picked while Data available is on.');

/** The click's panel: the pieces at the point, best first, the one the layer draws marked. */
export function piecesView(rows: PieceRow[] | 'loading' | 'failed', layer: AvLayer, all: boolean): HTMLElement {
  const box = h('div', { class: 'pop av-pop' });
  if (rows === 'loading') {
    box.append(h('div', { class: 'muted' }, h('span', { class: 'spin' }), ' Reading the data here…'));
    return box;
  }
  if (rows === 'failed') {
    box.append(h('div', { class: 'muted' }, 'The data here could not be read: the server is away.'));
    return box;
  }
  box.append(h('div', { class: 'ttl' }, 'Data available here'));
  if (!rows.length) {
    box.append(h('div', { class: 'muted' }, all ? 'No source is known to have data here.' : 'No source known here downloads freely (All data shows the others).'), note());
    return box;
  }
  box.append(h('div', { class: 'faint av-count' }, `${rows.length} source${rows.length > 1 ? 's' : ''}, best first${all ? '' : ', those that download freely'}`));
  for (const r of rows) {
    const kv = (k: string, v: string) => h('div', { class: 'kv' }, h('span', {}, k), h('b', {}, v));
    box.append(h('div', { class: `av-piece${r.painted ? ' painted' : ''}` },
      h('div', { class: 'av-head' }, h('b', {}, r.title), r.score ? h('span', { class: 'faint', title: 'Its score, 0 to 1' }, r.score) : null),
      r.sub ? h('div', { class: 'sub' }, r.sub) : null,
      r.painted ? h('div', { class: 'av-mark' }, `Drawn by ${layer.label}`) : null,
      ...r.metrics.map(([k, v]) => kv(k, v)),
      r.availability ? kv('Availability', r.availability) : null,
      r.extent ? h('div', { class: 'kv', title: r.read ? `Read: ${r.read}` : '' }, h('span', {}, 'Extent'), h('b', {}, r.extent)) : null,
      h('div', { class: 'links' },
        r.licence ? (r.licence.url ? h('a', { href: r.licence.url, target: '_blank', rel: 'noopener' }, r.licence.text) : h('span', { class: 'faint' }, r.licence.text)) : null,
        r.link ? h('a', { href: r.link, target: '_blank', rel: 'noopener' }, 'Source ↗') : null),
    ));
  }
  box.append(note());
  return box;
}
