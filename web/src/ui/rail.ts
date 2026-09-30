// "Passenger rail" section of the top-left panel: how rail lines are coloured and drawn.
import { RAIL_GROUPS } from '../config';
import { railPresets } from '../presets';
import { RAIL_COMPONENTS, RAIL_GROUP_COLOURS, RAIL_METRICS, railMetricDef, type RailColour, type RailMetric } from '../rail';
import type { Dist } from '../roads/stats';
import { freshLook, lookOfScale, scaleOfLook, type RailState, type Store } from '../state';
import { h } from './dom';
import { PresetBar } from './presets';
import { ScaleControls } from './scale';

const COLOURS: [RailColour, string, string][] = [
  ['line', 'Line', 'Official line colours from OSM (the service type\'s colour where a line has none)'],
  ['group', 'Service', 'By service type: trams, metro, commuter, intercity, heritage & mountain'],
  ['metric', 'Metric', 'By scenic score or a ride factor, through a palette'],
  ['single', 'Single', 'One colour for every line'],
];

export class RailCard {
  el: HTMLElement;
  private on: HTMLInputElement;
  private colBtns: HTMLButtonElement[] = [];
  private metricSel: HTMLSelectElement;
  private metricHelp: HTMLDivElement;
  private scale: ScaleControls;
  private presetBar: PresetBar;
  /** Live preview of a palette while hovering the ramp list (null: back to the chosen one). */
  onPalettePreview: (palette: string | null) => void = () => {};
  private metricBox: HTMLDivElement;
  private groupLegend: HTMLDivElement;
  private single: HTMLInputElement;
  private singleBox: HTMLDivElement;
  private note: HTMLDivElement;
  private weightsBox: HTMLDivElement;
  private wInputs: HTMLInputElement[] = [];
  private wOuts: HTMLOutputElement[] = [];
  private dist: Dist | null = null;
  private range: [number, number] = [0, 1];

  constructor(private store: Store) {
    const R = (patch: Partial<RailState>) => store.set({ rail: { ...store.s.rail, ...patch } });
    this.on = h('input', { type: 'checkbox' });
    this.on.addEventListener('change', () => R({ on: this.on.checked }));

    const seg = h('div', { class: 'seg' });
    for (const [k, label, title] of COLOURS) {
      const b = h('button', { title, onclick: () => R({ colour: k }) }, label);
      this.colBtns.push(b);
      seg.append(b);
    }
    // Metric: each keeps its own colour settings (palette, range, fit, fades, highlight).
    this.metricSel = h('select', { class: 'scenic-sel' });
    for (const m of RAIL_METRICS) this.metricSel.append(h('option', { value: m.key }, m.label));
    this.metricSel.addEventListener('change', () => {
      const r = store.s.rail;
      const next = this.metricSel.value as RailMetric;
      if (next === r.metric) return;
      const looks = { ...r.looks, [r.metric]: lookOfScale(r) };
      R({ metric: next, looks, ...scaleOfLook(looks[next] ?? freshLook(railMetricDef(next).range, 0.4)) });
    });
    this.metricHelp = h('div', { class: 'mode-help' });
    this.scale = new ScaleControls({
      get: () => store.s.rail,
      set: (patch) => R(patch),
      metric: () => {
        const d = railMetricDef(store.s.rail.metric);
        return { domain: d.domain, step: d.step, fmt: d.fmt };
      },
      noun: 'rail',
      measure: 'rail length',
      fadeDefault: 0.4,
      spanDefault: 0.6,
      onPreview: (k) => this.onPalettePreview(k),
    });
    this.presetBar = new PresetBar(store, {
      list: railPresets,
      label: 'Ride factors',
      get: () => store.s.rail,
      set: (patch) => R(patch),
    });

    const grid = h('div', { class: 'wgrid' });
    RAIL_COMPONENTS.forEach((c, i) => {
      const inp = h('input', { type: 'range', min: -1.5, max: 2, step: 0.1, title: c.help });
      const out = h('output');
      inp.addEventListener('input', () => {
        const w = [...store.s.rail.weights];
        w[i] = Number(inp.value);
        R({ weights: w });
      });
      inp.addEventListener('dblclick', () => {
        const w = [...store.s.rail.weights];
        w[i] = 0;
        R({ weights: w });
      });
      this.wInputs.push(inp);
      this.wOuts.push(out);
      grid.append(h('label', { title: c.help }, c.label), inp, out);
    });
    this.weightsBox = h('div', { class: 'weights' },
      this.presetBar.el,
      grid,
      h('div', { class: 'faint note' }, 'Views and water are measured from a carriage window (2.8 m). Tunnels count against a ride; double-click a slider to zero it.'),
    );

    this.metricBox = h('div', { class: 'rail-metric' },
      this.metricSel,
      this.metricHelp,
      this.scale.legend,
      this.scale.palRow,
      this.scale.fadeRow,
      this.scale.thrRow,
      this.weightsBox,
    );
    this.groupLegend = h('div', { class: 'scheme-legend' },
      ...RAIL_GROUPS.map((g, i) => h('span', { class: 'lg' }, h('i', { style: `background:${RAIL_GROUP_COLOURS[i]}` }), g.label)));
    this.single = h('input', { type: 'color' });
    this.single.addEventListener('input', () => R({ single: this.single.value }));
    this.singleBox = h('div', { class: 'row2' }, h('span', { class: 'muted' }, 'Colour'), this.single);
    this.note = h('div', { class: 'faint note' });


    this.el = h('div', { class: 'rail-card' },
      h('label', { class: 'rail-hd' }, this.on, h('span', {}, 'Passenger rail'), h('span', { class: 'faint' }, 'Layers → groups, opacity')),
      h('div', { class: 'rail-bd' },
        seg,
        this.metricBox,
        this.groupLegend,
        this.singleBox,
        this.note,
      ),
    );
    this.sync();
  }

  sync() {
    const r = this.store.s.rail;
    this.on.checked = r.on;
    this.el.classList.toggle('off', !r.on);
    this.colBtns.forEach((b, i) => b.classList.toggle('on', COLOURS[i][0] === r.colour));
    this.metricBox.hidden = r.colour !== 'metric';
    this.groupLegend.hidden = r.colour !== 'group' && r.colour !== 'line';
    this.singleBox.hidden = r.colour !== 'single';
    this.note.hidden = r.colour !== 'line';
    this.note.textContent = 'Lines without an OSM colour use their service type\'s colour (above).';
    this.metricSel.value = r.metric;
    this.metricHelp.textContent = railMetricDef(r.metric).help;
    this.scale.sync();
    this.single.value = r.single;
    this.weightsBox.hidden = r.metric !== 'rscore';
    this.presetBar.render();
    RAIL_COMPONENTS.forEach((_, i) => {
      const v = r.weights[i] ?? 0;
      this.wInputs[i].value = String(v);
      this.wOuts[i].value = v === 0 ? '·' : (v > 0 ? '+' : '') + v.toFixed(1);
      this.wOuts[i].classList.toggle('neg', v < 0);
      this.wOuts[i].classList.toggle('zero', v === 0);
    });
  }

  /** The metric's distribution over rail in view, the range in use and the lookup (if equalising). */
  update(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.scale.update(dist, range, cdf);
  }
}
