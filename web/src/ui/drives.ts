// "Scenic drives" tab: the best-scoring stretch of each road in view, scored server-side with
// the current weights.
import { getDrives, type Drive } from '../api';
import { COMPONENTS } from '../scenic';
import * as prefs from '../prefs';
import { cap, h } from './dom';
import { fmt } from '../fmt';
import { LENGTHS, ListPane } from './listpane';
import { displayName } from '../names';

/** What "≈" by a list's count means. */
export const APPROX_TIP = 'Zoomed out: found in 500 m summaries of the roads (scores within a point or two of the exact ones); zoom in for exact lists.';

/** `len`: whole-road length filter [min, max], km (0 = no limit); `approx`: zoomed out (the
 * server's summaries). */
type Query = { bbox: string; poly: string; classes: number; surface: number; toll: number; unnamed: number; len: [number, number]; weights: number[]; approx: boolean };

export class DrivesPane extends ListPane<Drive, Query> {
  private len = 5;
  private lenSeg: { el: HTMLDivElement; on: (l: number) => void };
  onResults: (d: Drive[]) => void = () => {};
  /** Listed stretches drawn on the map all the time (else only the hovered one). */
  showOnMap = prefs.load('drives.showOnMap', false);
  onShowChange: (on: boolean) => void = () => {};

  constructor(root: HTMLElement) {
    super(root, 'Scenic drives not available.', () => ({ bbox: '', poly: '', classes: 0, surface: 3, toll: 3, unnamed: 0, len: [0, 0], weights: [], approx: false }));
    this.lenSeg = this.lengthSeg('road', (l) => this.setLen(l));
    const show = h('input', { type: 'checkbox' });
    show.checked = this.showOnMap;
    show.addEventListener('change', () => {
      this.showOnMap = show.checked;
      prefs.save('drives.showOnMap', show.checked);
      this.onShowChange(show.checked);
    });
    root.append(
      this.lenSeg.el,
      h('div', { class: 'picks-meta' }, this.count, this.spin,
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
    this.lenSeg.on(l);
    this.lastKey = '';
    this.refresh(true);
  }

  protected key(q: Query, w: string) {
    return `${q.poly || q.bbox}|${q.classes}|${q.surface}|${q.toll}|${q.unnamed}|${q.len}|${w}|${this.len}|${q.approx}`;
  }

  protected async fetch(q: Query, w: string, signal: AbortSignal) {
    const d = await getDrives({ bbox: q.bbox, poly: q.poly, w, len: String(this.len), limit: '30', classes: String(q.classes), surface: String(q.surface), toll: String(q.toll), unnamed: String(q.unnamed), lmin: String(q.len[0] * 1000), lmax: String(q.len[1] * 1000), ...(q.approx ? { approx: '1' } : {}) }, signal);
    this.render(d);
    this.onResults(d.drives);
  }

  protected failed() {
    this.onResults([]);
  }

  private render(d: { total: number; drives: Drive[]; approx?: boolean }) {
    this.hovered = null;
    this.count.textContent = d.total ? `${d.approx ? '≈ ' : ''}${fmt.n(d.total)} roads of ${this.len} km or more in view · top ${d.drives.length}` : `No roads of ${this.len} km or more in view`;
    this.count.title = d.approx ? APPROX_TIP : '';
    this.list.replaceChildren(
      ...d.drives.map((c, i) => {
        const title = h('span', { class: 'ct' });
        if (c.ref) title.append(h('span', { class: 'ref' }, c.ref));
        title.append(cap(displayName(c.main, c.name, c.sub)) || c.route || (c.ref ? '' : `Unnamed ${c.class.replace('_', ' ')}`));
        // Top three contributing components (value × positive weight is done server-side via
        // the score; here show the strongest raw factors).
        const top = c.parts
          .map((v, k) => [v, k] as [number, number])
          .filter(([v, k]) => v > 0.05 && COMPONENTS[k].bar)
          .sort((a, b) => b[0] - a[0])
          .slice(0, 3)
          .map(([v, k]) => `${COMPONENTS[k].label} ${Math.round(v * 100)}`);
        const flags = [c.parts[10] > 0.3 ? 'scenic route' : '', c.parts[11] > 0.3 ? 'viewpoints' : '']
          .filter(Boolean);
        return this.row(i, title, c.score, `${fmt.dist(c.length_m)} · ${[...top, ...flags].join(' · ')}`, c);
      }),
    );
  }
}
