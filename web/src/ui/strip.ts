// Bottom strip. Row 1: the hovered road — its name, then the scenic score and each measured score
// factor as a bar (full = the factor at its cap), then roadside tree height. A rail
// line shows its services and the ride factors in the same slots; a marker or highlighted area
// (with no road under the cursor) gets one descriptive line.
// Row 2: yes/no flags as chips, road tags and elevation sources on the left; loading status,
// cursor position, zoom, scale, the build Mac and the NAS (buildstatus.ts), links and the credits
// dialog on the right. Every slot has a fixed width and its elements persist, so nothing moves as
// the values change.
import type { Map as MLMap } from 'maplibre-gl';
import type { Credit } from '../catalog';
import { CLASS_LABELS, RAIL0, RAIL_GROUPS, ST_BRIDGE, ST_LINK, ST_TUNNEL, ST_UNPAVED } from '../config';
import type { FeatureSummary } from '../overlays';
import { legibleCss } from '../linecolour';
import { displayName } from '../names';
import { RAIL_COMPONENTS, fmtTrains, railComponents, railScore, type RailSample } from '../rail';
import type { HoverInfo } from '../roads/layer';
import type { WayInfo } from '../api';
import { COMPONENTS, FLAG_LABELS, components, scoreOf } from '../scenic';
import { cap, fmt, h } from './dom';

const VALUE_FONT = '600 10px -apple-system, system-ui, sans-serif';
let measureCtx: CanvasRenderingContext2D | null = null;
const textWidth = (t: string) => {
  measureCtx ??= document.createElement('canvas').getContext('2d')!;
  measureCtx.font = VALUE_FONT;
  return measureCtx.measureText(t).width;
};

/** A labelled bar whose value sits at the tip of the fill: inside it if it fits, else just past it. */
class Bar {
  readonly el: HTMLDivElement;
  private track: HTMLDivElement;
  private fill: HTMLElement;
  private val: HTMLSpanElement;

  private lbl: HTMLSpanElement;

  constructor(label: string, private unit: string, private title: string, cls = '') {
    this.fill = h('i');
    this.val = h('span', { class: 'bv' });
    this.track = h('div', { class: 'track' }, this.fill, this.val);
    this.lbl = h('span', { class: 'lbl' });
    this.el = h('div', { class: `bar ${cls}`, title }, this.lbl, this.track);
    this.label(label, unit, title);
  }

  /** Relabel (the same slots show road or rail factors). */
  label(label: string, unit: string, title: string) {
    this.unit = unit;
    this.title = title;
    this.lbl.replaceChildren(label, unit ? h('span', { class: 'u' }, ` ${unit}`) : '');
  }

  set(frac: number, text: string, trackW: number, colour: string) {
    const f = Math.max(0, Math.min(1, frac));
    const fillPx = f * trackW;
    const tw = textWidth(text);
    this.fill.style.width = `${f * 100}%`;
    this.fill.style.background = colour;
    this.val.textContent = text;
    this.el.title = `${text}${this.unit ? ` ${this.unit}` : ''} · ${this.title}`;
    // Inside the fill, ending at its tip; else just past the tip; else (a narrow bar) not at all.
    const inside = fillPx >= tw + 6;
    const outside = !inside && fillPx + 3 + tw <= trackW - 2;
    this.val.classList.toggle('in', inside);
    this.val.classList.toggle('none', !inside && !outside);
    this.val.style.left = `${inside ? fillPx - tw - 3 : fillPx + 3}px`;
  }

  /** Width of the track (all bars share it). */
  get width() {
    return this.track.clientWidth;
  }
}

export class Strip {
  private r1: HTMLDivElement;
  private road: HTMLSpanElement;
  private trees: HTMLElement;
  private score: Bar;
  private bars: { i: number; bar: Bar }[];
  private info: HTMLSpanElement;
  private coord: HTMLSpanElement;
  private zoom: HTMLSpanElement;
  private scale: HTMLSpanElement;
  private loading: HTMLSpanElement;
  private build: HTMLSpanElement;
  private credits: { d: HTMLDialogElement; set: (data: Credit[]) => void };
  /** The data's credits shown, as a key (they come again with every catalog poll). */
  private creditsKey = '';
  private last: { hov: HoverInfo; info: WayInfo | null | 'loading'; areas: FeatureSummary[] } | null = null;
  private featLine: HTMLSpanElement;
  /** Which factors the bars currently show. */
  private barsFor: 'road' | 'rail' = 'road';
  /** Rail: the ride factors shown in the bar slots (tunnels go to row 2). */
  private readonly railBars = RAIL_COMPONENTS.map((_, i) => i).filter((i) => RAIL_COMPONENTS[i].key !== 'tunnel');
  railWeights: () => number[] = () => [];

  constructor(root: HTMLElement, private map: MLMap, private weights: () => number[]) {
    const idle = h('span', { class: 'hint' },
      'Two-finger drag pans, pinch zooms, ⌥ + two-finger drag or right-drag tilts & rotates around the cursor · G: Street View, M: Google Maps, O: OpenStreetMap at the cursor');
    const cell = (label: string, title: string) => {
      const b = h('b');
      return { el: h('div', { class: 'cell', title }, h('span', { class: 'lbl' }, label), b), b };
    };
    this.road = h('span', { class: 'road' });
    const t = cell('Trees', 'Roadside tree height (p95 within 30 m)');
    this.trees = t.b;
    this.score = new Bar('Scenic', '', 'Scenic score with the current weights (0–100)', 'score');
    this.bars = COMPONENTS.flatMap((c, i) => (c.bar ? [{ i, bar: new Bar(c.bar.short, c.bar.unit, `${c.label}: ${c.help}`) }] : []));
    this.featLine = h('span', { class: 'featline' });
    this.r1 = h('div', { class: 'r1 idle' },
      idle, this.featLine, this.road, this.score.el, ...this.bars.map((b) => b.bar.el), t.el);
    this.r1.style.setProperty('--bars', String(this.bars.length)); // a column each (style.css)

    this.info = h('span', { class: 'info' });
    this.coord = h('span', { class: 'coord num' });
    this.zoom = h('span', { class: 'zoom num' });
    this.scale = h('span', { class: 'scale' });
    this.loading = h('span', { class: 'status' });
    this.build = h('span');
    this.credits = creditsDialog();
    const credits = h('button', { title: 'Data sources, credits and licences', onclick: () => this.credits.d.showModal() }, '© Credits');
    root.append(
      this.r1,
      h('div', { class: 'r2' },
        this.info,
        h('span', { class: 'right' }, this.loading, this.coord, this.zoom, this.scale, this.build, credits),
      ),
      this.credits.d,
    );
    map.on('move', () => this.view());
    map.on('mousemove', (ev) => (this.coord.textContent = fmt.coord(ev.lngLat.lat, ev.lngLat.lng)));
    // Bar widths change with the window: re-place the values.
    new ResizeObserver(() => this.last && this.show(this.last.hov, this.last.info, this.last.areas)).observe(this.r1);
    this.view();
  }

  /** What the zoom and scale readouts show (they change only when it does: every camera move
   * calls view). */
  private shownView = '';

  private view() {
    const z = this.map.getZoom();
    const c = this.map.getCenter();
    // Metric scale bar (~100 px).
    const mpp = (40075016.686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** z);
    const target = 100 * mpp;
    const p = 10 ** Math.floor(Math.log10(target));
    const nice = [1, 2, 5, 10].map((k) => k * p).filter((v) => v <= target).pop() ?? p;
    const zs = `z ${z.toFixed(1)}`, px = Math.round(nice / mpp);
    const key = `${zs}|${nice}|${px}`;
    if (key === this.shownView) return;
    this.shownView = key;
    this.zoom.textContent = zs;
    const bar = h('i');
    bar.style.width = `${px}px`;
    this.scale.replaceChildren(bar, fmt.dist(nice));
  }

  /** The status element (tasks.ts keeps it up to date). */
  get status(): HTMLElement {
    return this.loading;
  }

  /** The build Mac's and the NAS's element (ui/buildstatus.ts keeps it up to date). */
  get buildStatus(): HTMLElement {
    return this.build;
  }

  /** The credits of the sources the map's data comes from, as its catalog lists them (catalog.ts;
   * none before its first answer). */
  setCredits(data: Credit[] | undefined) {
    const key = JSON.stringify(data ?? []);
    if (key === this.creditsKey) return;
    this.creditsKey = key;
    this.credits.set(data ?? []);
  }

  /** A marker or highlighted area (no road under the cursor), with the other areas it lies in. */
  showFeature(f: FeatureSummary, areas: FeatureSummary[]) {
    this.last = null;
    this.r1.classList.remove('idle');
    this.r1.classList.add('feat');
    const dot = h('i', { class: f.area ? 'fa' : 'fp' });
    dot.style.background = f.colour;
    // The name keeps its room; the designation and then the facts give way (ending in …).
    this.featLine.replaceChildren(dot, h('b', {}, cap(f.title)), h('span', { class: 'kind' }, f.kind),
      h('span', { class: 'facts' }, ...f.facts.map((x) => h('span', { class: 'fact' }, x))));
    this.featLine.title = [f.title, f.kind, ...f.facts].join(' · ');
    const others = areas.filter((a) => a.title !== f.title);
    if (f.also?.length) {
      // A pill of its own, apart from the facts before it.
      const c = h('span', { class: 'also', title: `Also here: ${f.also.join(' · ')} (click for all)` },
        h('span', { class: 'n' }, `+${f.also.length} here`), `${f.also[0]}${f.also.length > 1 ? ' …' : ''}`);
      this.featLine.append(c);
    }
    const second = f.desc ? h('span', { class: 'src desc', title: cap(f.desc) }, cap(f.desc)) : f.source ? h('span', { class: 'src' }, `Source: ${f.source}`) : null;
    this.info.replaceChildren(...this.areaChips(others), ...(second ? [second] : []));
  }

  private areaChips(areas: FeatureSummary[]) {
    return areas.slice(0, 3).map((a) => {
      const c = h('span', { class: 'chip area', title: `${a.kind}${a.facts.length ? ' · ' + a.facts.join(' · ') : ''}` }, `in ${a.title}`);
      c.style.setProperty('--c', a.colour);
      return c;
    });
  }

  private relabel(to: 'road' | 'rail') {
    if (this.barsFor === to) return;
    this.barsFor = to;
    this.bars.forEach(({ i, bar }, k) => {
      if (to === 'road') {
        const c = COMPONENTS[i];
        bar.label(c.bar!.short, c.bar!.unit, `${c.label}: ${c.help}`);
        bar.el.style.visibility = '';
      } else {
        const c = RAIL_COMPONENTS[this.railBars[k]];
        if (c) bar.label(c.short, c.unit, `${c.label}: ${c.help}`);
        bar.el.style.visibility = c ? '' : 'hidden'; // rail has fewer factors than roads
      }
    });
    this.score.label('Scenic', '', to === 'road' ? 'Scenic score with the current weights (0–100)' : 'Ride score with the rail weights (0–100)');
  }

  show(hov: HoverInfo | null, info: WayInfo | null | 'loading', areas: FeatureSummary[] = []) {
    this.r1.classList.remove('feat');
    if (!hov) {
      this.last = null;
      this.r1.classList.add('idle');
      this.info.replaceChildren();
      return;
    }
    this.last = { hov, info, areas };
    this.r1.classList.remove('idle');
    const st = hov.style;
    if ((st & 15) >= RAIL0) return this.showRail(hov, info, areas);
    this.relabel('road');

    // Row 1.
    this.road.replaceChildren();
    if (info === 'loading') this.road.append(h('span', { class: 'spin' }));
    else if (info) {
      if (info.ref) this.road.append(h('span', { class: 'ref' }, info.ref));
      this.road.append(cap(displayName(info.main, info.name, info.sub)) || (info.ref ? '' : 'Unnamed road'));
    }
    this.road.title = this.road.textContent ?? '';
    const c = hov.ch;
    const none = c.every((x) => x === 0); // no scenic analysis here
    const w = this.weights();
    const k = components(c);
    const tw = this.bars[0].bar.width, ts = this.score.width; // factor bars share one width
    const score = scoreOf(c, w);
    this.score.set(none ? 0 : score / 100, none ? '–' : score.toFixed(0), ts, '#f3c77a');
    for (const { i, bar } of this.bars) {
      bar.set(none ? 0 : k[i], none ? '–' : COMPONENTS[i].bar!.text(c), tw, w[i] < 0 ? '#ff8a7a' : 'var(--accent)');
    }
    this.trees.textContent = none ? '–' : `${Math.round(c[11] / 8)} m`;

    // Row 2: flags, tags, sources.
    const chips = FLAG_LABELS.filter(([m]) => c[7] & m).map(([m, l]) =>
      h('span', { class: 'chip' }, m === 1 && info && info !== 'loading' && info.route ? `★ ${info.route}` : l));
    const tags = [CLASS_LABELS[st & 15] + (st & ST_LINK ? ' link' : '')];
    if (st & ST_UNPAVED) tags.push('unpaved');
    if (st & ST_BRIDGE) tags.push('bridge');
    if (st & ST_TUNNEL) tags.push('tunnel');
    if (info && info !== 'loading') {
      if (info.surface && !(st & ST_UNPAVED)) tags.push(info.surface);
      if (info.maxspeed) tags.push(`${info.maxspeed} km/h`);
      if (info.lanes) tags.push(`${info.lanes} lanes`);
      if (info.oneway) tags.push('one-way');
      if (info.toll) tags.push('toll');
      if (info.covered) tags.push('covered bridge');
      if (info.route && !(c[7] & 1)) tags.push(`★ ${info.route}`);
    }
    const src = info && info !== 'loading' && info.sources.length ? info.sources.map(([s]) => s).join(' + ') : '';
    this.info.replaceChildren(...chips, ...this.areaChips(areas), h('span', { class: 'tags' }, tags.join(' · ')), ...(src ? [h('span', { class: 'src' }, src)] : []));
  }

  private showRail(hov: HoverInfo, info: WayInfo | null | 'loading', areas: FeatureSummary[]) {
    this.relabel('rail');
    const st = hov.style;
    this.road.replaceChildren();
    if (info === 'loading') this.road.append(h('span', { class: 'spin' }));
    else if (info) {
      if (info.colour) {
        const sw = h('i', { class: 'line-sw' });
        sw.style.background = legibleCss(info.colour) ?? info.colour;
        this.road.append(sw);
      }
      this.road.append(cap(displayName(info.main, info.name, info.sub)) || info.route || CLASS_LABELS[st & 15]);
    }
    this.road.title = [info && info !== 'loading' ? info.route : '', this.road.textContent].filter(Boolean).join(' · ');
    const smp: RailSample = { elev: hov.elev, grade: hov.grade, ground: hov.ground ?? hov.elev, bridge: !!(st & ST_BRIDGE), tunnel: !!(st & ST_TUNNEL), ch: hov.ch, freq: hov.fq ?? -1 };
    const none = hov.ch.every((x) => x === 0);
    const w = this.railWeights();
    const k = railComponents(smp);
    const tw = this.bars[0].bar.width, ts = this.score.width;
    const score = railScore(smp, w);
    this.score.set(none ? 0 : score / 100, none ? '–' : score.toFixed(0), ts, '#f3c77a');
    this.bars.forEach(({ bar }, j) => {
      const i = this.railBars[j];
      const c = RAIL_COMPONENTS[i];
      if (!c) return;
      const measured = !none || ['altitude', 'viaduct', 'gradient'].includes(c.key);
      bar.set(measured ? k[i] : 0, measured ? c.text(smp) : '–', tw, (w[i] ?? 0) < 0 ? '#ff8a7a' : 'var(--accent)');
    });
    this.trees.textContent = none ? '–' : `${Math.round(hov.ch[11] / 8)} m`;
    const tags = [CLASS_LABELS[st & 15]];
    if (st & ST_BRIDGE) tags.push('bridge');
    if (st & ST_TUNNEL) tags.push('tunnel');
    tags.push(smp.freq > 0 ? `${hov.fqMin ? 'at least ' : ''}${fmtTrains(smp.freq)} trains a day each way` : 'no timetable');
    const chips: HTMLElement[] = [];
    if (info && info !== 'loading') {
      for (const g of info.rail ?? []) {
        const gi = RAIL_GROUPS.findIndex((x) => x.key === g);
        if (gi >= 0 && RAIL0 + gi !== (st & 15)) tags.push(RAIL_GROUPS[gi].label.toLowerCase());
      }
      if (info.route) chips.push(h('span', { class: 'chip rail', title: 'Services using this track (OSM route relations)' }, info.route));
      if (info.maxspeed) tags.push(`${info.maxspeed} km/h`);
    }
    const src = info && info !== 'loading' && info.sources.length ? info.sources.map(([s]) => s).join(' + ') : '';
    this.info.replaceChildren(...chips, ...this.areaChips(areas), h('span', { class: 'tags' }, tags.join(' · ')), ...(src ? [h('span', { class: 'src' }, src)] : []));
  }
}

/** The app's own credits, whatever data it shows: the colour ramps it ships. The data's come with
 * its catalog (pipeline::rules::CREDITS, those of the sources its data comes from). */
const APP_CREDITS: Credit[] = [
  {
    what: 'Colour ramps',
    source: 'matplotlib (viridis, magma, plasma, inferno, cividis, cubehelix), seaborn (mako, rocket), Google (turbo), ColorBrewer (Cynthia Brewer), Fabio Crameri’s Scientific colour maps (batlow, hawaii, La Jolla, Oslo, Bamako, Tokyo, Vik, Berlin), cmocean (ice), colorcet (fire, Peter Kovesi)',
    terms: 'ColorBrewer: Apache 2.0; Crameri, cmocean: MIT; colorcet: CC BY 4.0',
  },
];

/** Data sources and licences in a modal dialog: the catalog's credits (`set`), then the app's. */
function creditsDialog(): { d: HTMLDialogElement; set: (data: Credit[]) => void } {
  const body = h('tbody');
  const set = (data: Credit[]) =>
    body.replaceChildren(...[...data, ...APP_CREDITS].map((c) => h('tr', {}, h('td', {}, c.what), h('td', {}, c.source), h('td', {}, c.terms))));
  set([]);
  const d = h('dialog', { class: 'credits' },
    h('div', { class: 'hd' }, h('b', {}, 'Data sources & licences'), h('button', { class: 'x', title: 'Close', onclick: () => d.close() }, '×')),
    h('table', {},
      h('thead', {}, h('tr', {}, h('th', {}, 'What'), h('th', {}, 'Source'), h('th', {}, 'Licence / terms'))),
      body,
    ),
    h('p', { class: 'faint' }, 'For personal use: do not publish or redistribute the built data or a hosted copy of this map.'),
  );
  // Click on the backdrop closes it.
  d.addEventListener('click', (e) => {
    if (e.target === d) d.close();
  });
  return { d, set };
}
