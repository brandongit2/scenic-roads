// "Top climbs" tab: ranked sustained climbs in the current view (precomputed server-side).
import * as prefs from '../prefs';
import { fmt, h } from './dom';

export interface Climb {
  way: number;
  name: string;
  ref: string;
  class: string;
  gain_m: number;
  length_m: number;
  avg_grade: number;
  max_grade: number;
  start_elev: number;
  top_elev: number;
  unpaved: boolean;
  geom: [number, number][];
}

type Sort = 'gain' | 'grade' | 'score';
const SORTS: [Sort, string, string][] = [
  ['gain', 'Gain', 'Largest elevation gain'],
  ['score', 'Difficulty', 'Gain² ÷ length (FIETS-style)'],
  ['grade', 'Steepest', 'Highest average grade'],
];

export class ClimbsPane {
  private sort: Sort = 'gain';
  private list: HTMLDivElement;
  private count: HTMLSpanElement;
  private spin: HTMLSpanElement;
  private seg: HTMLButtonElement[] = [];
  private abort: AbortController | null = null;
  private timer = 0;
  private lastKey = '';
  onHover: (c: Climb | null) => void = () => {};
  onSelect: (c: Climb) => void = () => {};
  query: () => { bbox: string; classes: number; surface: number } = () => ({ bbox: '', classes: 0, surface: 3 });

  constructor(readonly root: HTMLElement) {
    this.list = h('div', { class: 'climbs' });
    this.count = h('span', { class: 'faint' });
    this.spin = h('span', { class: 'spin', hidden: true });
    const seg = h('div', { class: 'seg small' });
    for (const [k, label, title] of SORTS) {
      const b = h('button', { title, onclick: () => this.setSort(k) }, label);
      this.seg.push(b);
      seg.append(b);
    }
    root.append(
      seg,
      h('div', { class: 'climbs-meta' }, this.count, this.spin),
      this.list,
      h('div', { class: 'faint', style: 'font-size:10.5px;margin-top:6px;line-height:1.45' },
        '≥ 40 m gain, 3–25 % average, in the allowed direction of travel. Hover to highlight, click for the profile.'),
    );
    const k = prefs.load<Sort>('climbs.sort', 'gain');
    this.setSort(SORTS.some((x) => x[0] === k) ? k : 'gain');
  }

  private setSort(k: Sort) {
    this.sort = k;
    prefs.save('climbs.sort', k);
    this.seg.forEach((b, i) => b.classList.toggle('on', SORTS[i][0] === k));
    this.lastKey = '';
    this.refresh(true);
  }

  /** Re-query for the current view (debounced unless `now`). */
  refresh(now = false) {
    if (this.root.hidden) return;
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.load(), now ? 0 : 250);
  }

  private async load() {
    const q = this.query();
    const key = `${q.bbox}|${q.classes}|${q.surface}|${this.sort}`;
    if (key === this.lastKey) return;
    this.lastKey = key;
    this.abort?.abort();
    this.abort = new AbortController();
    this.spin.hidden = false;
    try {
      const r = await fetch(`/api/climbs?bbox=${q.bbox}&sort=${this.sort}&limit=25&classes=${q.classes}&surface=${q.surface}`, {
        signal: this.abort.signal,
      });
      const d: { total: number; climbs: Climb[] } = await r.json();
      this.render(d);
    } catch (e) {
      if ((e as Error).name !== 'AbortError') this.list.replaceChildren(h('div', { class: 'muted' }, 'Could not load climbs.'));
    } finally {
      this.spin.hidden = true;
    }
  }

  private render(d: { total: number; climbs: Climb[] }) {
    this.count.textContent = d.total ? `${fmt.n(d.total)} climbs in view · top ${d.climbs.length}` : 'No climbs in view';
    this.list.replaceChildren(
      ...d.climbs.map((c, i) => {
        const title = h('span', { class: 'ct' });
        if (c.ref) title.append(h('span', { class: 'ref' }, c.ref));
        title.append(c.name || (c.ref ? '' : `Unnamed ${c.class.replace('_', ' ')}`));
        const row = h('div', { class: 'climb', onclick: () => this.onSelect(c) },
          h('span', { class: 'rank' }, String(i + 1)),
          h('div', { class: 'cbody' },
            h('div', { class: 'cl1' }, title, h('b', {}, `+${fmt.n(c.gain_m)} m`)),
            h('div', { class: 'cl2' },
              `${fmt.dist(c.length_m)} · ${c.avg_grade.toFixed(1)} % avg · ${c.max_grade.toFixed(0)} % max · ${fmt.n(c.start_elev)}→${fmt.n(c.top_elev)} m${c.unpaved ? ' · unpaved' : ''}`),
          ),
        );
        row.addEventListener('mouseenter', () => this.onHover(c));
        row.addEventListener('mouseleave', () => this.onHover(null));
        return row;
      }),
    );
  }
}
