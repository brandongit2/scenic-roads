// "Scenic drives" tab: the best-scoring stretch of each road in view, scored server-side with
// the current weights.
import { getDrives, type Drive } from '../api';
import { COMPONENTS } from '../scenic';
import * as prefs from '../prefs';
import { fmt, h } from './dom';

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
  query: () => { bbox: string; classes: number; surface: number; weights: number[] } = () => ({ bbox: '', classes: 0, surface: 3, weights: [] });

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
    root.append(
      seg,
      h('div', { class: 'climbs-meta' }, this.count, this.spin),
      this.list,
      h('div', { class: 'faint', style: 'font-size:10.5px;margin-top:6px;line-height:1.45' },
        'Best stretch of each continuous road, ranked by the mean scenic score with your weights (Colour → Scenic → Score). Hover to highlight, click for the profile.'),
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
    const key = `${q.bbox}|${q.classes}|${q.surface}|${w}|${this.len}`;
    if (key === this.lastKey) return;
    this.lastKey = key;
    this.abort?.abort();
    this.abort = new AbortController();
    this.spin.hidden = false;
    try {
      const d = await getDrives({ bbox: q.bbox, w, len: String(this.len), limit: '30', classes: String(q.classes), surface: String(q.surface) }, this.abort.signal);
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
    this.count.textContent = d.total ? `${fmt.n(d.total)} roads in view · top ${d.drives.length}` : 'No roads long enough in view';
    this.list.replaceChildren(
      ...d.drives.map((c, i) => {
        const title = h('span', { class: 'ct' });
        if (c.ref) title.append(h('span', { class: 'ref' }, c.ref));
        title.append(c.name || c.route || (c.ref ? '' : `Unnamed ${c.class.replace('_', ' ')}`));
        // Top three contributing components (value × positive weight is done server-side via
        // the score; here show the strongest raw factors).
        const top = c.parts
          .map((v, k) => [v, k] as [number, number])
          .filter(([v, k]) => v > 0.05 && k < 10)
          .sort((a, b) => b[0] - a[0])
          .slice(0, 3)
          .map(([v, k]) => `${COMPONENTS[k].label} ${Math.round(v * 100)}`);
        const flags = [c.parts[10] > 0.3 ? 'scenic route' : '', c.parts[12] > 0.3 ? 'waterfront' : '', c.parts[11] > 0.3 ? 'viewpoints' : '']
          .filter(Boolean);
        const bar = h('i', { class: 'sbar' });
        bar.style.width = `${Math.max(4, c.score)}%`;
        const row = h('div', { class: 'climb', onclick: () => this.onSelect(c) },
          h('span', { class: 'rank' }, String(i + 1)),
          h('div', { class: 'cbody' },
            h('div', { class: 'cl1' }, title, h('b', {}, c.score.toFixed(0))),
            h('div', { class: 'sbarw' }, bar),
            h('div', { class: 'cl2' }, `${fmt.dist(c.length_m)} · ${[...top, ...flags].join(' · ')}`),
          ),
        );
        row.addEventListener('mouseenter', () => this.onHover(c));
        row.addEventListener('mouseleave', () => this.onHover(null));
        return row;
      }),
    );
  }
}
