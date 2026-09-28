import { CLASS_LABELS, NCLASS } from '../config';
import type { LoadProgress } from '../roads/layer';
import type { Extreme, ViewStats } from '../roads/stats';
import * as prefs from '../prefs';
import { fmt, h } from './dom';

// Neutral class colours (elevation owns the hue elsewhere).
const CLASS_COLORS = ['#3a4250', '#4b5566', '#5a6577', '#6c778a', '#8791a3', '#a3adbd', '#c2cad6', '#dde3ea', '#ffffff', '#4f6f8f'];

type Tab = 'stats' | 'drives' | 'climbs';
const TABS: Tab[] = ['stats', 'drives', 'climbs'];

export class StatsCard {
  private spin: HTMLSpanElement;
  private body: HTMLDivElement;
  private render: HTMLDivElement;
  private statsPane: HTMLDivElement;
  readonly climbsRoot: HTMLDivElement;
  readonly drivesRoot: HTMLDivElement;
  private tabs: HTMLButtonElement[] = [];
  tab: Tab = 'stats';
  onTab: (tab: Tab) => void = () => {};
  onFly: (e: Extreme) => void = () => {};
  onMark: (e: Extreme | null, kind: 'high' | 'low') => void = () => {};
  wayName: (tileLine: Extreme) => Promise<string> = async () => '';

  constructor(root: HTMLElement) {
    this.spin = h('span', { class: 'spin', title: 'Loading tiles in view…' });
    this.body = h('div');
    this.render = h('div', { class: 'render' });
    this.statsPane = h('div', {}, this.body, h('div', { class: 'sep' }), this.render);
    this.climbsRoot = h('div', { hidden: true });
    this.drivesRoot = h('div', { hidden: true });
    const tab = (k: Tab, label: string) => {
      const b = h('button', { class: 'tab', onclick: () => this.show(k) }, label);
      this.tabs.push(b);
      return b;
    };
    root.append(
      h('div', { class: 'hd' }, h('div', { class: 'tabs' }, tab('stats', 'In view'), tab('drives', 'Scenic drives'), tab('climbs', 'Climbs')), this.spin),
      h('div', { class: 'bd' }, this.statsPane, this.drivesRoot, this.climbsRoot),
    );
    const saved = prefs.load<Tab>('stats.tab', 'stats');
    this.show(TABS.includes(saved) ? saved : 'stats');
  }

  show(k: Tab) {
    this.tab = k;
    prefs.save('stats.tab', k);
    this.tabs.forEach((b, i) => b.classList.toggle('on', TABS[i] === k));
    this.statsPane.hidden = k !== 'stats';
    this.climbsRoot.hidden = k !== 'climbs';
    this.drivesRoot.hidden = k !== 'drives';
    this.onTab(k);
  }

  update(st: ViewStats | null, p: LoadProgress, zt: number, extra: [string, string, string?][] = []) {
    this.spin.hidden = p.loaded >= p.wanted;
    this.render.textContent =
      `Tiles z${zt}: ${p.loaded}/${p.wanted}` + (p.inflight ? ` (${p.inflight} loading)` : '') +
      ` · ${p.tilesDrawn} drawn · ${fmt.big(p.vertices)} vertices · ${fmt.mb(p.gpuBytes)} GPU`;
    if (!st || st.totalKm <= 0 || !st.elev) {
      this.body.replaceChildren(h('div', { class: 'muted' }, p.loaded < p.wanted ? 'Loading roads…' : 'No roads in view.'));
      return;
    }
    const e = st.elev;
    const bar = h('div', { class: 'classbar' });
    const list = h('div', { class: 'classlist' });
    for (let c = NCLASS - 1; c >= 0; c--) {
      const km = st.classKm[c];
      if (km <= 0) continue;
      const seg = h('i', { title: `${CLASS_LABELS[c]}: ${fmt.km(km)}` });
      seg.style.width = `${(km / st.totalKm) * 100}%`;
      seg.style.background = CLASS_COLORS[c];
      bar.append(seg);
      const sw = h('i');
      sw.style.background = CLASS_COLORS[c];
      list.append(h('div', {}, h('span', {}, sw, CLASS_LABELS[c]), h('span', { class: 'num' }, km < 10 ? km.toFixed(1) : fmt.n(km))));
    }
    const ext = (x: Extreme | null, kind: 'high' | 'low') => {
      if (!x) return h('dd', {}, '—');
      const a = h('a', { title: 'Show on map', onclick: () => this.onFly(x) }, fmt.m(x.elev));
      a.addEventListener('mouseenter', () => this.onMark(x, kind));
      a.addEventListener('mouseleave', () => this.onMark(null, kind));
      const name = h('span', { class: 'xname' });
      this.wayName(x).then((n) => {
        name.textContent = n;
        name.title = n;
      });
      return h('dd', { class: 'ext' }, name, a);
    };
    const g = st.grade;
    this.body.replaceChildren(
      h('dl', {},
        h('dt', {}, 'Road length'), h('dd', {}, h('b', {}, fmt.km(st.totalKm))),
      ),
      h('div', { class: 'faint', style: 'font-size:10.5px;margin-top:6px' }, 'km by class'),
      bar,
      list,
      h('div', { class: 'sep' }),
      h('dl', {},
        h('dt', {}, 'Elevation'), h('dd', {}, `${fmt.m(e.quantile(0))} – ${fmt.m(e.quantile(1))}`),
        h('dt', {}, 'Median · IQR'), h('dd', {}, `${fmt.m(e.quantile(0.5))} · ${fmt.m(e.quantile(0.25))}–${fmt.m(e.quantile(0.75))}`),
        h('dt', {}, 'Highest road'), ext(st.highest, 'high'),
        h('dt', {}, 'Lowest road'), ext(st.lowest, 'low'),
        h('dt', {}, 'Grade > 6 % · > 10 %'), h('dd', {}, g ? `${(g.above(6) * 100).toFixed(1)} % · ${(g.above(10) * 100).toFixed(1)} %` : '—'),
        h('dt', {}, 'Median grade'), h('dd', {}, g ? fmt.pct(g.quantile(0.5)) : '—'),
      ),
      ...(extra.length
        ? [
            h('div', { class: 'sep' }),
            h('dl', {}, ...extra.flatMap(([k, v, title]) => [h('dt', { title }, k), h('dd', { title }, v)])),
          ]
        : []),
    );
  }
}
