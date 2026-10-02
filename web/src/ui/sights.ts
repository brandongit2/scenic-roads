// "Sights" tab: the most prominent landmarks in view (the score that sizes their dots: fame from
// Wikipedia pageviews and rarity nearby), narrowed to a kind with chips.
import type { OverlayKey } from '../state';
import * as prefs from '../prefs';
import { cap, fmt, h } from './dom';
import { displayOf } from '../names';

export interface Sight {
  k: OverlayKey;
  layer: string;
  score: number;
  props: Record<string, any>;
  lngLat: [number, number];
}

export class SightsPane {
  private kind: OverlayKey | null = prefs.load<OverlayKey | null>('sights.kind', null);
  private chips: HTMLDivElement;
  private list: HTMLDivElement;
  private count: HTMLSpanElement;
  /** Visible landmark kinds in view: key, label, colour, count. */
  kinds: () => { key: OverlayKey; label: string; colour: string; n: number }[] = () => [];
  query: (kind: OverlayKey | null) => Sight[] = () => [];
  onHover: (s: Sight | null) => void = () => {};
  onSelect: (s: Sight) => void = () => {};
  /** The row under the pointer (M, O open it). */
  hovered: Sight | null = null;

  constructor(readonly root: HTMLElement) {
    this.chips = h('div', { class: 'sight-chips' });
    this.count = h('span', { class: 'faint' });
    this.list = h('div', { class: 'climbs' });
    root.append(
      this.chips,
      h('div', { class: 'climbs-meta' }, this.count),
      this.list,
      h('div', { class: 'faint', style: 'font-size:10.5px;margin-top:6px;line-height:1.45' },
        'Landmarks in view by prominence: how well known they are (Wikipedia pageviews) and how rare nearby, as set in Layers → Stops & sights. Hover to mark (M: Google Maps, O: OpenStreetMap), click for details.'),
    );
  }

  refresh() {
    if (this.root.hidden) return;
    const kinds = this.kinds();
    if (this.kind && !kinds.some((k) => k.key === this.kind)) this.kind = null;
    const chip = (key: OverlayKey | null, label: string, colour?: string) => {
      const b = h('button', { class: 'pill' + (this.kind === key ? ' on' : ''), onclick: () => {
        this.kind = key;
        prefs.save('sights.kind', key);
        this.refresh();
      } });
      if (colour) {
        const dot = h('i', { class: 'dot' });
        dot.style.background = colour;
        b.append(dot);
      }
      b.append(label);
      return b;
    };
    this.chips.replaceChildren(chip(null, 'All'), ...kinds.filter((k) => k.n > 0).map((k) => chip(k.key, k.label, k.colour)));
    if (!kinds.length) {
      this.count.textContent = '';
      this.list.replaceChildren(h('div', { class: 'muted', style: 'padding:8px 6px' }, 'Turn on stops & sights or heritage sites in Layers to list them here.'));
      return;
    }
    const items = this.query(this.kind);
    this.hovered = null;
    const total = kinds.filter((k) => !this.kind || k.key === this.kind).reduce((a, k) => a + k.n, 0);
    this.count.textContent = `${fmt.n(total)} in view · top ${items.length}`;
    const colour = new Map(kinds.map((k) => [k.key, k.colour]));
    const label = new Map(kinds.map((k) => [k.key, k.label]));
    this.list.replaceChildren(...items.map((s, i) => {
      const p = s.props;
      const facts: string[] = [];
      if (p.pv) facts.push(`${fmt.n(p.pv)} views a month`);
      if (s.k === 'peak' && p.ele) facts.push(`${fmt.n(Math.round(p.ele))} m`);
      if (s.k === 'peak' && p.pr) facts.push(`prominence ${fmt.n(Math.round(p.pr))} m`);
      if (s.k === 'waterfall' && p.h) facts.push(`${fmt.n(Math.round(p.h))} m high`);
      if (s.k === 'heritage' && p.designation) facts.push(String(p.designation));
      if (p.ia >= 1 && p.ia < 20000) facts.push(`best-known for ${p.ia >= 10 ? fmt.n(Math.round(p.ia)) : p.ia.toFixed(1)} km`);
      const dot = h('i', { class: 'dot' });
      dot.style.background = colour.get(s.k) ?? '#888';
      const bar = h('i', { class: 'sbar' });
      bar.style.width = `${Math.max(4, s.score * 100)}%`;
      const row = h('a', { class: 'climb', onclick: () => this.onSelect(s) },
        h('span', { class: 'rank' }, String(i + 1)),
        h('div', { class: 'cbody' },
          h('div', { class: 'cl1' }, h('span', { class: 'ct' }, dot, cap(displayOf(p)) || `Unnamed ${String(label.get(s.k) ?? '').toLowerCase()}`), h('b', {}, String(Math.round(s.score * 100)))),
          h('div', { class: 'sbarw' }, bar),
          h('div', { class: 'cl2' }, [label.get(s.k), ...facts].filter(Boolean).join(' · ')),
        ),
      );
      row.addEventListener('mouseenter', () => this.onHover((this.hovered = s)));
      row.addEventListener('mouseleave', () => this.onHover((this.hovered = null)));
      return row;
    }));
  }
}
