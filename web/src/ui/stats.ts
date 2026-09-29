import { CLASS_LABELS, NROAD } from '../config';
import type { LoadProgress } from '../roads/layer';
import type { Extreme, ViewStats } from '../roads/stats';
import * as prefs from '../prefs';
import { fmt, h } from './dom';

// Neutral class colours (elevation owns the hue elsewhere).
const CLASS_COLORS = ['#3a4250', '#4b5566', '#5a6577', '#6c778a', '#8791a3', '#a3adbd', '#c2cad6', '#dde3ea', '#ffffff', '#4f6f8f'];

type Tab = 'stats' | 'drives' | 'rides' | 'lines' | 'sights';
const TABS: Tab[] = ['stats', 'drives', 'rides', 'lines', 'sights'];

/** A place the In view summary links to (fly there; landmarks open their popup). */
export interface ViewPlace {
  name: string;
  lngLat: [number, number];
  /** Map layer of a landmark (for its popup). */
  layer?: string;
}

/** The In view summary beyond the road statistics (each part only while its layer is on). */
export interface InViewExtra {
  /** Scenic score median · p90 (current weights); vista distance median · p90, km. */
  scenic?: [number, number] | null;
  vista?: [number, number] | null;
  rail?: { total: number; groups: { label: string; colour: string; km: number }[]; busiest: (ViewPlace & { perDay: number }) | null; highest: Extreme | null } | null;
  ferry?: { routes: number; seasonal: number; busiest: (ViewPlace & { perDay: number }) | null } | null;
  landmarks?: { label: string; colour: string; n: number; best: ViewPlace | null }[] | null;
  terrain?: { summit: (ViewPlace & { ele: number }) | null; relief: number | null; above1000: number | null } | null;
}

export class StatsCard {
  private spin: HTMLSpanElement;
  private body: HTMLDivElement;
  private render: HTMLDivElement;
  private statsPane: HTMLDivElement;
  readonly drivesRoot: HTMLDivElement;
  readonly ridesRoot: HTMLDivElement;
  readonly linesRoot: HTMLDivElement;
  readonly sightsRoot: HTMLDivElement;
  private tabs: HTMLButtonElement[] = [];
  tab: Tab = 'stats';
  onTab: (tab: Tab) => void = () => {};
  onFly: (e: Extreme) => void = () => {};
  onMark: (e: Extreme | null, kind: 'high' | 'low') => void = () => {};
  /** A linked place: fly there (and open a landmark's popup); hover marks it. */
  onPlace: (p: ViewPlace) => void = () => {};
  onPlaceHover: (p: ViewPlace | null) => void = () => {};
  wayName: (tileLine: Extreme) => Promise<string> = async () => '';

  constructor(root: HTMLElement) {
    this.spin = h('span', { class: 'spin', title: 'Loading tiles in view…' });
    this.body = h('div');
    this.render = h('div', { class: 'render' });
    this.statsPane = h('div', { class: 'pane-stats' }, this.body, h('div', { class: 'sep' }), this.render);
    this.drivesRoot = h('div', { hidden: true });
    this.ridesRoot = h('div', { hidden: true });
    this.linesRoot = h('div', { hidden: true });
    this.sightsRoot = h('div', { hidden: true });
    const tab = (k: Tab, label: string, title?: string) => {
      const b = h('button', { class: 'tab', title: title ?? label, onclick: () => this.show(k) }, label);
      this.tabs.push(b);
      return b;
    };
    root.append(
      h('div', { class: 'hd' }, h('div', { class: 'tabs' }, tab('stats', 'In view'), tab('drives', 'Drives', 'Scenic drives'), tab('rides', 'Rides', 'Scenic rides'), tab('lines', 'Rail lines'), tab('sights', 'Sights')), this.spin),
      h('div', { class: 'bd' }, this.statsPane, this.drivesRoot, this.ridesRoot, this.linesRoot, this.sightsRoot),
    );
    const saved = prefs.load<Tab>('stats.tab', 'stats');
    this.show(TABS.includes(saved) ? saved : 'stats');
  }

  show(k: Tab) {
    this.tab = k;
    prefs.save('stats.tab', k);
    this.tabs.forEach((b, i) => b.classList.toggle('on', TABS[i] === k));
    this.statsPane.hidden = k !== 'stats';
    this.drivesRoot.hidden = k !== 'drives';
    this.ridesRoot.hidden = k !== 'rides';
    this.linesRoot.hidden = k !== 'lines';
    this.sightsRoot.hidden = k !== 'sights';
    this.onTab(k);
  }

  update(st: ViewStats | null, p: LoadProgress, zt: number, x: InViewExtra = {}) {
    this.spin.hidden = p.loaded >= p.wanted;
    this.render.textContent =
      `Tiles z${zt}: ${p.loaded}/${p.wanted}` + (p.inflight ? ` (${p.inflight} loading)` : '') +
      ` · ${p.tilesDrawn} drawn · ${fmt.big(p.vertices)} vertices · ${fmt.mb(p.gpuBytes)} GPU`;
    const parts: Node[] = [];
    const head = (t: string) => h('div', { class: 'iv-head' }, t);
    const place = (pl: ViewPlace | null, text: string) => {
      if (!pl) return h('dd', {}, '—');
      const a = h('a', { title: 'Show on map', onclick: () => this.onPlace(pl) }, text);
      a.addEventListener('mouseenter', () => this.onPlaceHover(pl));
      a.addEventListener('mouseleave', () => this.onPlaceHover(null));
      return h('dd', { class: 'ext' }, h('span', { class: 'xname', title: pl.name }, pl.name), a);
    };
    const ext = (e: Extreme | null, kind: 'high' | 'low') => {
      if (!e) return h('dd', {}, '—');
      const a = h('a', { title: 'Show on map', onclick: () => this.onFly(e) }, fmt.m(e.elev));
      a.addEventListener('mouseenter', () => this.onMark(e, kind));
      a.addEventListener('mouseleave', () => this.onMark(null, kind));
      const name = h('span', { class: 'xname' });
      this.wayName(e).then((n) => {
        name.textContent = n;
        name.title = n;
      });
      return h('dd', { class: 'ext' }, name, a);
    };
    const bar = (items: { label: string; colour: string; km: number }[], total: number) => {
      const b = h('div', { class: 'classbar' });
      const list = h('div', { class: 'classlist' });
      for (const it of items) {
        if (it.km <= 0) continue;
        const seg = h('i', { title: `${it.label}: ${fmt.km(it.km)}` });
        seg.style.width = `${(it.km / total) * 100}%`;
        seg.style.background = it.colour;
        b.append(seg);
        const sw = h('i');
        sw.style.background = it.colour;
        list.append(h('div', {}, h('span', {}, sw, it.label), h('span', { class: 'num' }, it.km < 10 ? it.km.toFixed(1) : fmt.n(it.km))));
      }
      return [b, list];
    };

    // Roads.
    if (st && st.totalKm > 0) {
      const classes: { label: string; colour: string; km: number }[] = [];
      for (let c = NROAD - 1; c >= 0; c--) classes.push({ label: CLASS_LABELS[c], colour: CLASS_COLORS[c], km: st.classKm[c] });
      const g = st.grade;
      parts.push(
        head('Roads'),
        h('dl', {}, h('dt', {}, 'Road length'), h('dd', {}, h('b', {}, fmt.km(st.totalKm)))),
        ...bar(classes, st.totalKm),
        h('dl', {},
          ...(x.scenic ? [h('dt', { title: 'With the current weights (Scenic → Score)' }, 'Scenic score'), h('dd', {}, `${x.scenic[0].toFixed(0)} median · ${x.scenic[1].toFixed(0)} p90`)] : []),
          ...(x.vista ? [h('dt', { title: 'How far the road sees, typically' }, 'Vista distance'), h('dd', {}, `${x.vista[0].toFixed(1)} km median · ${x.vista[1].toFixed(1)} p90`)] : []),
          h('dt', {}, 'Steeper than 10 %'), h('dd', {}, g ? `${(g.above(10) * 100).toFixed(1)} % of length` : '—'),
          h('dt', {}, 'Highest road'), ext(st.highest, 'high'),
          h('dt', {}, 'Lowest road'), ext(st.lowest, 'low'),
        ),
      );
    } else {
      parts.push(h('div', { class: 'muted' }, p.loaded < p.wanted ? 'Loading roads…' : 'No roads in view.'));
    }
    // Rail and ferries.
    if ((x.rail && x.rail.total > 0) || (x.ferry && x.ferry.routes > 0)) {
      parts.push(h('div', { class: 'sep' }), head('Rail & ferries'));
      if (x.rail && x.rail.total > 0) {
        parts.push(
          h('dl', {}, h('dt', {}, 'Passenger rail'), h('dd', {}, h('b', {}, fmt.km(x.rail.total)))),
          ...bar(x.rail.groups, x.rail.total),
          h('dl', {},
            h('dt', { title: 'Trains a day each way, all services on the track (published timetables)' }, 'Busiest line'),
            place(x.rail.busiest, x.rail.busiest ? `${fmt.n(x.rail.busiest.perDay)} a day` : ''),
            h('dt', {}, 'Highest on rail'), ext(x.rail.highest, 'high'),
          ),
        );
      }
      if (x.ferry && x.ferry.routes > 0) {
        parts.push(h('dl', {},
          h('dt', {}, 'Ferry routes'), h('dd', {}, `${x.ferry.routes}${x.ferry.seasonal ? ` · ${x.ferry.seasonal} seasonal` : ''}`),
          h('dt', { title: 'Sailings a day each way' }, 'Busiest crossing'),
          place(x.ferry.busiest, x.ferry.busiest ? `${x.ferry.busiest.perDay >= 10 ? fmt.n(x.ferry.busiest.perDay) : x.ferry.busiest.perDay.toFixed(1)} a day` : ''),
        ));
      }
    }
    // Landmarks.
    if (x.landmarks && x.landmarks.some((l) => l.n > 0)) {
      const dl = h('dl', { class: 'iv-lm' });
      for (const l of x.landmarks) {
        if (!l.n) continue;
        const sw = h('i', { class: 'dot' });
        sw.style.background = l.colour;
        dl.append(h('dt', {}, sw, `${l.label} · ${fmt.n(l.n)}`), place(l.best, l.best ? 'best-known' : ''));
      }
      parts.push(h('div', { class: 'sep' }), head('Landmarks'), dl);
    }
    // Terrain.
    if (x.terrain && (x.terrain.summit || x.terrain.relief != null)) {
      const t = x.terrain;
      parts.push(h('div', { class: 'sep' }), head('Terrain'), h('dl', {},
        h('dt', {}, 'Highest summit'), place(t.summit, t.summit ? fmt.m(t.summit.ele) : ''),
        ...(t.relief != null ? [h('dt', { title: 'Highest summit or road minus lowest road in view' }, 'Relief'), h('dd', {}, fmt.m(t.relief))] : []),
        ...(t.above1000 != null ? [h('dt', {}, 'Road above 1,000 m'), h('dd', {}, `${(t.above1000 * 100).toFixed(1)} % of length`)] : []),
      ));
    }
    this.body.replaceChildren(...parts);
  }
}
