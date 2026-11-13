// The layers panel's Data available section (ui/layers.ts) and the click's panel: the themes'
// layers from the server's description (available.ts), one shown at a time, the set, the
// availability switch, the opacity, and the legend of the layer shown.
import { legendModel, setLabel, shown, type AvailableState, type AvLayer, type AvTheme, type PieceRow, OPACITY_RANGE } from '../available';
import type { Store } from '../state';
import { Slider, pct } from './controls';
import { h } from './dom';
import { inUse } from './settingprofiles';

/** A theme as the section has it: its description, or where that stands. */
export type ThemeEntry = { theme: string; desc: AvTheme | 'loading' | 'failed' };

export class AvailableSection {
  readonly nodes: HTMLElement[];
  /** The group's switch (the section's header). */
  readonly on: HTMLInputElement;
  private list = h('div', { class: 'av-layers' });
  private setRow: HTMLElement;
  private setSeg = h('div', { class: 'seg small' });
  private availBtns: HTMLButtonElement[] = [];
  private op: Slider;
  private legend = h('div', { class: 'av-legend' });
  private body: HTMLDivElement;
  private themes: ThemeEntry[] = [];

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
    const row = (label: string, input: HTMLElement) => h('div', { class: 'row' }, h('span', { class: 'muted' }, label), input, h('span'));
    this.setRow = row('Data', this.setSeg);
    this.body = h('div', { class: 'av-body' },
      this.list,
      this.setRow,
      row('Show', avail),
      this.op.el,
      this.legend,
      h('div', { class: 'faint note' }, 'Zoomed out, a pixel shows the best data anywhere under it, so small areas of fine data stay in sight. Click the map for every source there. Places nobody has looked into yet show nothing.'),
    );
    this.nodes = [this.body];
  }

  /** The themes the catalogue lists, each with its description or where that stands. */
  setThemes(themes: ThemeEntry[]) {
    this.themes = themes;
    this.sync();
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
      else if (t.desc === 'failed') rows.push(h('div', { class: 'muted av-wait' }, 'The layers could not be read: the server is away.'));
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
    this.setSeg.replaceChildren(...sets.map((k) => h('button', { class: k === set ? 'on' : '', onclick: () => A({ set: k }) }, setLabel(k))));
    this.legend.replaceChildren(...(desc ? legendNodes(shown(s, desc).layer, desc) : []));
  }
}

/** A layer's legend: a swatch and label per class or word (the score's classes split from their
 * best to their worst shade), or a gradient with its stops' labels. */
function legendNodes(l: AvLayer, t: AvTheme): HTMLElement[] {
  const m = legendModel(l, t);
  if (m.kind === 'ramp') {
    return [h('div', { class: 'tint-bar', style: `background:${m.gradient}` }), h('div', { class: 'tint-ticks' }, ...m.ticks.map((x) => h('span', {}, x)))];
  }
  const sw = (colour: string, to?: string) => h('i', { style: `background:${to ? `linear-gradient(90deg, ${colour} 50%, ${to} 50%)` : colour}` });
  return [
    h('div', { class: 'av-swatches' }, ...m.rows.map((r) => h('span', { class: 'lg' }, sw(r.colour, r.to), r.label))),
    ...(m.caption ? [h('div', { class: 'faint av-caption' }, m.caption)] : []),
  ];
}

/** The click's panel: the pieces at the point, best first, the one the layer paints marked. */
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
    box.append(h('div', { class: 'muted' }, all ? 'No source is known to have data here.' : 'No source known here downloads freely (All data shows the others).'));
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
  return box;
}
