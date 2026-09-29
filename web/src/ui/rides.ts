// "Scenic rides" and "Rail lines" tabs: passenger lines in view scored server-side with the
// current ride-factor weights (Passenger rail → Metric → Ride score).
import { getRailLines, getRides, type RailLine, type Ride } from '../api';
import { RAIL_COMPONENTS } from '../rail';
import * as prefs from '../prefs';
import { cap, fmt, h } from './dom';

const LENGTHS = [2, 5, 10, 25];
type Query = { bbox: string; poly: string; weights: number[]; groups: number };

const swatch = (colour: number) => {
  const i = h('i', { class: 'dot' });
  i.style.background = colour ? `#${(colour & 0xffffff).toString(16).padStart(6, '0')}` : 'var(--line-strong)';
  return i;
};
const trains = (t: number) => (t >= 10 ? fmt.n(Math.round(t)) : t > 0 ? t.toFixed(1) : '');

/** Shared list machinery: debounced loads keyed on the query, a spinner and a count line. */
abstract class RailPane<T> {
  protected list = h('div', { class: 'climbs' });
  protected count = h('span', { class: 'faint' });
  protected spin = h('span', { class: 'spin', hidden: true });
  private abort: AbortController | null = null;
  private timer = 0;
  protected lastKey = '';
  query: () => Query = () => ({ bbox: '', poly: '', weights: [], groups: 31 });
  onHover: (x: T | null) => void = () => {};
  onSelect: (x: T) => void = () => {};

  constructor(readonly root: HTMLElement) {}

  refresh(now = false) {
    if (this.root.hidden) return;
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.load(), now ? 0 : 300);
  }

  protected abstract extraKey(): string;
  protected abstract fetch(q: Query, w: string, signal: AbortSignal): Promise<void>;

  private async load() {
    const q = this.query();
    const w = q.weights.map((v) => v.toFixed(2)).join(',');
    const key = `${q.poly || q.bbox}|${q.groups}|${w}|${this.extraKey()}`;
    if (key === this.lastKey) return;
    this.lastKey = key;
    this.abort?.abort();
    this.abort = new AbortController();
    this.spin.hidden = false;
    try {
      await this.fetch(q, w, this.abort.signal);
    } catch (e) {
      if ((e as Error).name !== 'AbortError') this.list.replaceChildren(h('div', { class: 'muted' }, 'Rail lines not available.'));
    } finally {
      this.spin.hidden = true;
    }
  }

  protected row(i: number, title: Node, score: number, sub: string, x: T) {
    const bar = h('i', { class: 'sbar' });
    bar.style.width = `${Math.max(4, score)}%`;
    const r = h('a', { class: 'climb', onclick: () => this.onSelect(x) },
      h('span', { class: 'rank' }, String(i + 1)),
      h('div', { class: 'cbody' },
        h('div', { class: 'cl1' }, title, h('b', {}, score.toFixed(0))),
        h('div', { class: 'sbarw' }, bar),
        h('div', { class: 'cl2' }, sub),
      ),
    );
    r.addEventListener('mouseenter', () => this.onHover(x));
    r.addEventListener('mouseleave', () => this.onHover(null));
    return r;
  }
}

export class RidesPane extends RailPane<Ride> {
  private len = prefs.load('rides.len', 10);
  private seg: HTMLButtonElement[] = [];
  onResults: (r: Ride[]) => void = () => {};

  constructor(root: HTMLElement) {
    super(root);
    const seg = h('div', { class: 'seg small' });
    for (const l of LENGTHS) {
      const b = h('button', { title: `Best ${l} km stretch of each line`, onclick: () => this.setLen(l) }, `${l} km`);
      this.seg.push(b);
      seg.append(b);
    }
    root.append(seg, h('div', { class: 'climbs-meta' }, this.count, this.spin), this.list,
      h('div', { class: 'faint', style: 'font-size:10.5px;margin-top:6px;line-height:1.45' },
        'Best stretch of each passenger line in view, ranked by the mean ride score with your ride-factor weights (Passenger rail → Metric). Hover to highlight, click for the profile.'));
    this.setLen(LENGTHS.includes(this.len) ? this.len : 10);
  }

  private setLen(l: number) {
    this.len = l;
    prefs.save('rides.len', l);
    this.seg.forEach((b, i) => b.classList.toggle('on', LENGTHS[i] === l));
    this.lastKey = '';
    this.refresh(true);
  }

  protected extraKey() {
    return String(this.len);
  }

  protected async fetch(q: Query, w: string, signal: AbortSignal) {
    const d = await getRides({ bbox: q.bbox, poly: q.poly, w, len: String(this.len), limit: '30', groups: String(q.groups) }, signal);
    this.count.textContent = d.total ? `${fmt.n(d.total)} lines of ${this.len} km or more in view · top ${d.rides.length}` : `No lines of ${this.len} km or more in view`;
    this.list.replaceChildren(...d.rides.map((r, i) => {
      const top = r.parts
        .map((v, k) => [v, k] as [number, number])
        .filter(([v, k]) => v > 0.05 && k !== 7)
        .sort((a, b) => b[0] - a[0])
        .slice(0, 3)
        .map(([v, k]) => `${RAIL_COMPONENTS[k].short} ${Math.round(v * 100)}`);
      const title = h('span', { class: 'ct' }, swatch(r.colour), cap(r.name) || 'Rail line');
      return this.row(i, title, r.score, [fmt.dist(r.length_m), r.trains ? `${trains(r.trains)} trains a day` : '', ...top].filter(Boolean).join(' · '), r);
    }));
    this.onResults(d.rides);
  }
}

const SORTS = [['score', 'Ride score'], ['trains', 'Trains a day'], ['length', 'Length']] as const;

export class LinesPane extends RailPane<RailLine> {
  private sort: string = prefs.load('lines.sort', 'score');
  private sortBtns: HTMLButtonElement[] = [];

  constructor(root: HTMLElement) {
    super(root);
    const seg = h('div', { class: 'seg small' });
    for (const [k, label] of SORTS) {
      const b = h('button', { title: `Sort by ${label.toLowerCase()}`, onclick: () => this.setSort(k) }, label);
      this.sortBtns.push(b);
      seg.append(b);
    }
    root.append(seg, h('div', { class: 'climbs-meta' }, this.count, this.spin), this.list,
      h('div', { class: 'faint', style: 'font-size:10.5px;margin-top:6px;line-height:1.45' },
        'Passenger lines in view: length in view, mean ride score with your ride-factor weights, and trains a day each way on the busiest part (published timetables). Hover to highlight, click to fit.'));
    this.setSort(this.sort);
  }

  private setSort(k: string) {
    this.sort = SORTS.some(([s]) => s === k) ? k : 'score';
    prefs.save('lines.sort', this.sort);
    this.sortBtns.forEach((b, i) => b.classList.toggle('on', SORTS[i][0] === this.sort));
    this.lastKey = '';
    this.refresh(true);
  }

  protected extraKey() {
    return this.sort;
  }

  protected async fetch(q: Query, w: string, signal: AbortSignal) {
    const d = await getRailLines({ bbox: q.bbox, poly: q.poly, w, limit: '40', groups: String(q.groups), sort: this.sort }, signal);
    this.count.textContent = d.total ? `${fmt.n(d.total)} lines in view · top ${d.lines.length}` : 'No passenger lines in view';
    this.list.replaceChildren(...d.lines.map((l, i) => {
      const title = h('span', { class: 'ct' }, swatch(l.colour), cap(l.name));
      const svc = l.services.split(' · ').map((x) => x.split(':')[0].trim()).filter((x, k, a) => x && x !== l.name && a.indexOf(x) === k).slice(0, 3).join(', ');
      return this.row(i, title, l.score, [`${fmt.dist(l.length_m)} in view`, l.trains ? `${trains(l.trains)} trains a day` : '', svc].filter(Boolean).join(' · '), l);
    }));
  }
}
