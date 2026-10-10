// How ferry lines are coloured (the settings panel's Ferries section, ui/layers.ts).
import { unitOf, withUnit } from '../autofit';
import { FERRY_GROUP_COLOURS, FERRY_GROUPS, FERRY_METRICS, HEADWAY_ONLY, SEASONS, UNKNOWN, ferryMetricDef, type FerryColour, type FerryMetric } from '../ferry';
import type { FerryCoverage } from '../ferries';
import type { Dist } from '../roads/stats';
import { freshLook, lookOfScale, scaleOfLook, type FerryState, type Store } from '../state';
import { h } from './dom';
import { ScaleControls } from './scale';

const COLOURS: [FerryColour, string, string][] = [
  ['service', 'Service', 'By service group: urban & commuter, short crossings, long-distance & overnight, cable & chain'],
  ['freq', 'Metric', 'By sailings a day or season length, through a colour map (grey: not known)'],
  ['season', 'Season', 'Year-round or seasonal, daily or some days only'],
  ['operator', 'Operator', 'Official line colours where published, else one colour per operator'],
  ['single', 'Single', 'One colour for every ferry'],
];

export class FerryCard {
  el: HTMLElement;
  private colBtns: HTMLButtonElement[] = [];
  private freqBox: HTMLDivElement;
  private metricSel: HTMLSelectElement;
  private metricHelp: HTMLDivElement;
  private scale: ScaleControls;
  private unknownLegend: HTMLDivElement;
  /** Live preview of a palette while hovering the ramp list (null: back to the chosen one). */
  onPalettePreview: (palette: string | null) => void = () => {};
  private cov: HTMLDivElement;
  private groupLegend: HTMLDivElement;
  private seasonLegend: HTMLDivElement;
  private note: HTMLDivElement;
  private single: HTMLInputElement;
  private singleBox: HTMLDivElement;
  private coverage: FerryCoverage | null = null;

  constructor(private store: Store) {
    const F = (patch: Partial<FerryState>) => store.set({ ferry: { ...store.s.ferry, ...patch } });
    const seg = h('div', { class: 'seg' });
    for (const [k, label, title] of COLOURS) {
      const b = h('button', { title, onclick: () => F({ colour: k }) }, label);
      this.colBtns.push(b);
      seg.append(b);
    }
    // Metric: each keeps its own colour settings.
    this.metricSel = h('select', { class: 'scenic-sel' });
    for (const m of FERRY_METRICS) this.metricSel.append(h('option', { value: m.key }, m.label));
    this.metricSel.addEventListener('change', () => {
      const f = store.s.ferry;
      const next = this.metricSel.value as FerryMetric;
      if (next === f.metric) return;
      const looks = { ...f.looks, [f.metric]: lookOfScale(f) };
      const d = ferryMetricDef(next);
      F({ metric: next, looks, ...scaleOfLook(looks[next] ?? { ...freshLook(d.range, 0), fit: [0, 100] }) });
    });
    this.metricHelp = h('div', { class: 'mode-help' });
    this.scale = new ScaleControls({
      get: () => store.s.ferry,
      set: (patch) => F(patch),
      metric: () => {
        const d = ferryMetricDef(store.s.ferry.metric);
        return { domain: d.domain, step: d.step, fmt: d.fmt };
      },
      noun: 'ferry lines',
      len: { active: () => !!ferryMetricDef(store.s.ferry.metric).byLen, get: () => store.s.ferry.fitLen, set: (fitLen) => F({ fitLen }),
        unit: () => unitOf(store.s.ferry.fitUnits, store.s.ferry.metric), setUnit: (u) => F({ fitUnits: withUnit(store.s.ferry.fitUnits, store.s.ferry.metric, u) }), best: () => 'busiest' },
      measure: 'ferry route length',
      fadeDefault: 0,
      spanDefault: 0.6,
      onPreview: (k) => this.onPalettePreview(k),
    });
    this.cov = h('div', { class: 'faint note' });
    this.unknownLegend = h('div', { class: 'scheme-legend' });
    this.freqBox = h('div', { class: 'rail-metric' },
      this.metricSel,
      this.metricHelp,
      this.scale.legend,
      this.unknownLegend,
      this.scale.palRow,
      this.scale.fadeRow,
      this.scale.thrRow,
      this.cov,
    );
    this.groupLegend = h('div', { class: 'scheme-legend' },
      ...FERRY_GROUPS.map((g, i) => h('span', { class: 'lg', title: g.help }, h('i', { style: `background:${FERRY_GROUP_COLOURS[i]}` }), g.label)));
    this.seasonLegend = h('div', { class: 'scheme-legend' },
      ...SEASONS.slice(1).concat([SEASONS[0]]).map(([c, label]) => h('span', { class: 'lg' }, h('i', { style: `background:${c}` }), label)));
    this.note = h('div', { class: 'faint note' }, 'Official line colours (OSM colour tags) where a line has one; otherwise each operator gets its own colour.');
    this.single = h('input', { type: 'color' });
    this.single.addEventListener('input', () => F({ single: this.single.value }));
    this.singleBox = h('div', { class: 'row2' }, h('span', { class: 'muted' }, 'Colour'), this.single);

    this.el = h('div', { class: 'rail-card ferry-card' },
      h('div', { class: 'rail-bd' },
        seg,
        this.freqBox,
        this.groupLegend,
        this.seasonLegend,
        this.note,
        this.singleBox,
      ),
    );
    this.sync();
  }

  sync() {
    const f = this.store.s.ferry;
    this.el.classList.toggle('off', !f.on);
    this.colBtns.forEach((b, i) => b.classList.toggle('on', COLOURS[i][0] === f.colour));
    this.freqBox.hidden = f.colour !== 'freq';
    this.groupLegend.hidden = f.colour !== 'service';
    this.seasonLegend.hidden = f.colour !== 'season';
    this.note.hidden = f.colour !== 'operator';
    this.singleBox.hidden = f.colour !== 'single';
    this.metricSel.value = f.metric;
    const d = ferryMetricDef(f.metric);
    this.metricHelp.textContent = d.help;
    this.unknownLegend.replaceChildren(
      ...(f.metric === 'freq'
        ? [h('span', { class: 'lg', title: 'A headway is published (e.g. every 20 min) but not the hours, so there is no daily count' }, h('i', { style: `background:${HEADWAY_ONLY}` }), 'Frequent, hours unknown'),
          h('span', { class: 'lg' }, h('i', { style: `background:${UNKNOWN}` }), 'No timetable found')]
        : [h('span', { class: 'lg' }, h('i', { style: `background:${UNKNOWN}` }), 'Season not known')]),
    );
    this.scale.sync();
    this.single.value = f.single;
    this.renderCov();
  }

  /** Lines in view and how many have a known frequency. */
  update(cov: FerryCoverage | null) {
    this.coverage = cov;
    this.renderCov();
  }

  /** The metric's distribution over the ferry lines in view, the range in use and the lookup. */
  updateScale(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.scale.update(dist, range, cdf);
  }

  private renderCov() {
    const c = this.coverage;
    this.cov.textContent = c && c.lines ? `Timetable found for ${c.known} of ${c.lines} lines in view.` : '';
  }
}
