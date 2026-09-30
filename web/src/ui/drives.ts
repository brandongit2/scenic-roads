// "Scenic drives" tab: the best-scoring stretch of each road in view, scored server-side with
// the current weights.
import { getDrives, type Drive } from '../api';
import { COMPONENTS } from '../scenic';
import * as prefs from '../prefs';
import { cap, fmt, h } from './dom';
import { withEnglish } from '../english';

const LENGTHS = [2, 5, 10, 25];

export class DrivesPane {
  private len = 5;
  private list: HTMLDivElement;
  private count: HTMLSpanElement;
  private spin: HTMLSpanElement;
  private seg: HTMLButtonElement[] = [];
  private abort: AbortController | null = null;
  private timer = 0;
  private lastKey = '';
  onResults: (d: Drive[]) => void = () => {};
  onHover: (d: Drive | null) => void = () => {};
  onSelect: (d: Drive) => void = () => {};
  /** The row under the pointer (G, M, O open it). */
  hovered: Drive | null = null;
  /** Listed stretches drawn on the map all the time (else only the hovered one). */
  showOnMap = prefs.load('drives.showOnMap', false);
  onShowChange: (on: boolean) => void = () => {};
  /** `len`: whole-road length filter [min, max], km (0 = no limit). */
  query: () => { bbox: string; poly: string; classes: number; surface: number; toll: number; unnamed: number; len: [number, number]; weights: number[] } = () => ({ bbox: '', poly: '', classes: 0, surface: 3, toll: 3, unnamed: 0, len: [0, 0], weights: [] });

  constructor(readonly root: HTMLElement) {
    this.list = h('div', { class: 'climbs' });
    this.count = h('span', { class: 'faint' });
    this.spin = h('span', { class: 'spin', hidden: true });
    const seg = h('div', { class: 'seg small' });
    for (const l of LENGTHS) {
      const b = h('button', { title: `Best ${l} km stretch of each road`, onclick: () => this.setLen(l) }, `${l} km`);
      this.seg.push(b);
      seg.append(b);
    }
    const show = h('input', { type: 'checkbox' });
    show.checked = this.showOnMap;
    show.addEventListener('change', () => {
      this.showOnMap = show.checked;
      prefs.save('drives.showOnMap', show.checked);
      this.onShowChange(show.checked);
    });
    root.append(
      seg,
      h('div', { class: 'climbs-meta' }, this.count, this.spin,
        h('label', { class: 'show-map', title: 'Highlight the listed stretches on the map all the time (hovering a row always highlights it)' }, show, 'On map')),
      this.list,
      h('div', { class: 'faint', style: 'font-size:10.5px;margin-top:6px;line-height:1.45' },
        'Best stretch of each continuous road, ranked by the mean scenic score with your weights (Colour → Scenic → Score). Hover to highlight (G: Street View, M: Google Maps, O: OpenStreetMap), click to select.'),
    );
    const l = prefs.load('drives.len', 5);
    this.setLen(LENGTHS.includes(l) ? l : 5);
  }

  private setLen(l: number) {
    this.len = l;
    prefs.save('drives.len', l);
    this.seg.forEach((b, i) => b.classList.toggle('on', LENGTHS[i] === l));
    this.lastKey = '';
    this.refresh(true);
  }

  refresh(now = false) {
    if (this.root.hidden) return;
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.load(), now ? 0 : 300);
  }

  private async load() {
    const q = this.query();
    const w = q.weights.map((v) => v.toFixed(2)).join(',');
    const key = `${q.poly || q.bbox}|${q.classes}|${q.surface}|${q.toll}|${q.unnamed}|${q.len}|${w}|${this.len}`;
    if (key === this.lastKey) return;
    this.lastKey = key;
    this.abort?.abort();
    this.abort = new AbortController();
    this.spin.hidden = false;
    try {
      const d = await getDrives({ bbox: q.bbox, poly: q.poly, w, len: String(this.len), limit: '30', classes: String(q.classes), surface: String(q.surface), toll: String(q.toll), unnamed: String(q.unnamed), lmin: String(q.len[0] * 1000), lmax: String(q.len[1] * 1000) }, this.abort.signal);
      this.render(d);
      this.onResults(d.drives);
    } catch (e) {
      if ((e as Error).name !== 'AbortError') {
        this.list.replaceChildren(h('div', { class: 'muted' }, 'Scenic analysis not available — run the scenic pipeline step.'));
        this.onResults([]);
      }
    } finally {
      this.spin.hidden = true;
    }
  }

  private render(d: { total: number; drives: Drive[] }) {
    this.hovered = null;
    this.count.textContent = d.total ? `${fmt.n(d.total)} roads of ${this.len} km or more in view · top ${d.drives.length}` : `No roads of ${this.len} km or more in view`;
    this.list.replaceChildren(
      ...d.drives.map((c, i) => {
        const title = h('span', { class: 'ct' });
        if (c.ref) title.append(h('span', { class: 'ref' }, c.ref));
        title.append(cap(c.name && withEnglish(c.name, c.geom[c.geom.length >> 1])) || c.route || (c.ref ? '' : `Unnamed ${c.class.replace('_', ' ')}`));
        // Top three contributing components (value × positive weight is done server-side via
        // the score; here show the strongest raw factors).
        const top = c.parts
          .map((v, k) => [v, k] as [number, number])
          .filter(([v, k]) => v > 0.05 && COMPONENTS[k].bar)
          .sort((a, b) => b[0] - a[0])
          .slice(0, 3)
          .map(([v, k]) => `${COMPONENTS[k].label} ${Math.round(v * 100)}`);
        const flags = [c.parts[9] > 0.3 ? 'scenic route' : '', c.parts[10] > 0.3 ? 'viewpoints' : '']
          .filter(Boolean);
        const bar = h('i', { class: 'sbar' });
        bar.style.width = `${Math.max(4, c.score)}%`;
        const row = h('a', { class: 'climb', onclick: () => this.onSelect(c) },
          h('span', { class: 'rank' }, String(i + 1)),
          h('div', { class: 'cbody' },
            h('div', { class: 'cl1' }, title, h('b', {}, c.score.toFixed(0))),
            h('div', { class: 'sbarw' }, bar),
            h('div', { class: 'cl2' }, `${fmt.dist(c.length_m)} · ${[...top, ...flags].join(' · ')}`),
          ),
        );
        row.addEventListener('mouseenter', () => this.onHover((this.hovered = c)));
        row.addEventListener('mouseleave', () => this.onHover((this.hovered = null)));
        return row;
      }),
    );
  }
}
