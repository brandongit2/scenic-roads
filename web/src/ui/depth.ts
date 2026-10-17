// The settings panel's Depth section (ui/layers.ts): the depth's two layers, each with its switch:
// the depth colour and the floor's shading (its colour ramp chosen as the panel's other ramps are),
// and the depth contours with their labels (depth.ts).
import { DEPTH_DEFAULTS, DEPTH_PALETTES, depthColour, rgba, scaleFns, type DepthState } from '../depthlook';
import type { Store } from '../state';
import { ColourPick, Slider, pct } from './controls';
import { h } from './dom';
import { RampSelect } from './rampselect';

const COMPASS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'];

export class DepthSection {
  readonly nodes: HTMLElement[];
  /** The colour layer's switch (the section's header). */
  readonly on: HTMLInputElement;
  /** Live preview of a palette while hovering the ramp list (null: back to the chosen one). */
  onPreview: (palette: string | null) => void = () => {};
  /** The domain the colour ramp spans now (the fitted one for "fit to view"). */
  domain: () => [number, number] = () => [0, this.store.s.depth.max];
  private body: HTMLDivElement;
  private pal: RampSelect;
  private scaleBtns: HTMLButtonElement[] = [];
  private maxRow: Slider;
  private sliders: Slider[];
  private colours: ColourPick[];
  private dry: HTMLInputElement;
  private contours: HTMLInputElement;
  private contourBox: HTMLDivElement;
  private intervalBtns: HTMLButtonElement[] = [];
  private fixed: HTMLSelectElement;
  private index: HTMLSelectElement;
  private labels: HTMLInputElement;
  private legend: HTMLDivElement;
  private ticks: HTMLDivElement;
  private note: HTMLDivElement;

  constructor(private store: Store) {
    const D = (patch: Partial<DepthState>) => store.set({ depth: { ...store.s.depth, ...patch } });
    const S = () => store.s.depth;
    this.on = h('input', { type: 'checkbox' });
    this.on.addEventListener('change', () => D({ on: this.on.checked }));
    const seg = (items: [string, string, string][], pick: (k: string) => void, into: HTMLButtonElement[]) => {
      const el = h('div', { class: 'seg small' });
      for (const [k, label, title] of items) {
        const b = h('button', { title, onclick: () => pick(k) }, label);
        into.push(b);
        el.append(b);
      }
      return el;
    };
    const row = (label: string, input: HTMLElement, out?: HTMLElement, title?: string) => h('div', { class: 'row', title }, h('span', { class: 'muted' }, label), input, out ?? h('span'));
    this.pal = new RampSelect(DEPTH_PALETTES, (k) => this.gradient(k), (k) => D({ palette: k }), (k) => this.onPreview(k));
    const scales = seg([['linear', 'Linear', 'Colour in proportion to depth'], ['log', 'Log', 'More colours in the shallows, where most of the detail is'], ['fit', 'Fit view', 'The ramp spans the depths in view (2nd to 98th percentile)']],
      (k) => D({ scale: k as DepthState['scale'] }), this.scaleBtns);
    this.maxRow = new Slider({ label: 'Down to', min: 5, max: 6000, step: 1, reset: DEPTH_DEFAULTS.max, get: () => S().max, set: (max) => D({ max: Math.round(max) }), fmt: (v) => `${Math.round(v)} m`,
      scale: { to: (v) => Math.log(v), from: (p) => Math.exp(p) }, title: 'The depth at the ramp\'s end' });
    this.maxRow.input.min = String(Math.log(5));
    this.maxRow.input.max = String(Math.log(6000));
    this.maxRow.input.step = '0.01';
    this.sliders = [
      this.maxRow,
      new Slider({ label: 'Opacity', min: 0.1, max: 1, step: 0.05, reset: DEPTH_DEFAULTS.opacity, get: () => S().opacity, set: (opacity) => D({ opacity }), fmt: pct }),
      new Slider({ label: 'Shading', min: 0, max: 1, step: 0.05, reset: DEPTH_DEFAULTS.shade, get: () => S().shade, set: (shade) => D({ shade }), fmt: (v) => (v ? pct(v) : 'off'), title: 'How strongly the floor is shaded' }),
      new Slider({ label: 'Light from', min: 0, max: 359, step: 1, reset: DEPTH_DEFAULTS.light, get: () => S().light, set: (light) => D({ light }), fmt: (v) => `${Math.round(v)}° ${COMPASS[Math.round(v / 45) % 8]}`, title: 'Where the light comes from (map north)' }),
    ];
    this.dry = h('input', { type: 'checkbox' });
    this.dry.addEventListener('change', () => D({ dry: this.dry.checked }));
    const dryCol = new ColourPick({ label: 'Drying', get: () => S().dryColour, set: (dryColour) => D({ dryColour }), reset: DEPTH_DEFAULTS.dryColour, title: 'The colour of water that dries at low tide' });
    this.legend = h('div', { class: 'tint-bar' });
    this.ticks = h('div', { class: 'tint-ticks depth-ticks' });
    this.note = h('div', { class: 'faint note' }, 'Below the water\'s surface: mean sea level at sea, the lake\'s surface on lakes. Each source at its own detail where it measured, coarser where it interpolated or estimated. Not for navigation.');
    // Contours.
    this.contours = h('input', { type: 'checkbox' });
    this.contours.addEventListener('change', () => D({ contours: this.contours.checked }));
    const intervals = seg([['zoom', 'By zoom', 'Closer lines as you zoom in'], ['fixed', 'Fixed', 'The same interval at every zoom']], (k) => D({ interval: k as DepthState['interval'] }), this.intervalBtns);
    this.fixed = h('select', {}, ...[1, 2, 5, 10, 20, 50, 100, 200, 500].map((v) => h('option', { value: String(v) }, `${v} m`)));
    this.fixed.addEventListener('change', () => D({ fixed: Number(this.fixed.value) }));
    this.index = h('select', { title: 'Bolder lines, labelled' }, ...[2, 4, 5, 10].map((v) => h('option', { value: String(v) }, `every ${v}th`)));
    this.index.addEventListener('change', () => D({ index: Number(this.index.value) }));
    this.labels = h('input', { type: 'checkbox' });
    this.labels.addEventListener('change', () => D({ labels: this.labels.checked }));
    const lineCol = new ColourPick({ label: 'Colour', get: () => S().colour, set: (colour) => D({ colour }), reset: DEPTH_DEFAULTS.colour });
    this.colours = [dryCol, lineCol];
    const lineSliders = [
      new Slider({ label: 'Weight', min: 0.4, max: 3, step: 0.1, reset: DEPTH_DEFAULTS.weight, get: () => S().weight, set: (weight) => D({ weight }), fmt: (v) => `${v.toFixed(1)}×` }),
      new Slider({ label: 'Opacity', min: 0.1, max: 1, step: 0.05, reset: DEPTH_DEFAULTS.lineOpacity, get: () => S().lineOpacity, set: (lineOpacity) => D({ lineOpacity }), fmt: pct }),
    ];
    this.sliders.push(...lineSliders);
    const tog = (input: HTMLInputElement, label: string, title = '') => h('label', { class: 'tog', title }, input, h('span', {}, label));
    this.contourBox = h('div', { class: 'grp sub' },
      row('Interval', intervals, this.fixed),
      row('Index lines', this.index),
      tog(this.labels, 'Labels (m) on the index lines'),
      lineCol.el,
      ...lineSliders.map((s) => s.el));
    this.body = h('div', { class: 'depth-body' },
      h('div', { class: 'tint-legend' }, this.legend, this.ticks),
      row('Colours', this.pal.el),
      row('Scale', scales),
      ...this.sliders.slice(0, 4).map((s) => s.el),
      tog(this.dry, 'Tidal flats in their own colour', 'Water that dries at low tide (above the chart\'s low water)'),
      dryCol.el,
      tog(this.contours, 'Depth contours', 'Lines of equal depth, labelled'),
      this.contourBox,
      this.note,
    );
    this.nodes = [this.body];
    this.sync();
  }

  private gradient(palette: string) {
    const stops = Array.from({ length: 9 }, (_, i) => `${rgba(depthColour(palette, i / 8))} ${(i / 8) * 100}%`);
    return `linear-gradient(90deg, ${stops.join(', ')})`;
  }

  sync() {
    const d = this.store.s.depth;
    this.on.checked = d.on;
    this.pal.set(d.palette);
    this.scaleBtns.forEach((b, i) => b.classList.toggle('on', ['linear', 'log', 'fit'][i] === d.scale));
    this.maxRow.el.hidden = d.scale === 'fit';
    for (const s of this.sliders) s.sync();
    for (const c of this.colours) c.sync();
    this.dry.checked = d.dry;
    this.colours[0].el.hidden = !d.dry;
    this.contours.checked = d.contours;
    this.contourBox.hidden = !d.contours;
    this.intervalBtns.forEach((b, i) => b.classList.toggle('on', ['zoom', 'fixed'][i] === d.interval));
    this.fixed.hidden = d.interval !== 'fixed';
    this.fixed.value = String(d.fixed);
    this.index.value = String(d.index);
    this.labels.checked = d.labels;
    // The legend: the ramp over its domain, with round depths marked.
    const [lo, hi] = this.domain();
    const [fwd] = scaleFns(d.scale === 'linear' ? 'linear' : 'log', Math.max(0, lo), Math.max(lo + 0.5, hi));
    this.legend.style.background = this.gradient(d.palette);
    const nice = [0, 1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000];
    const ticks = nice.filter((v) => v >= lo && v <= hi).filter((v, i, a) => i === 0 || fwd(v) - fwd(a[i - 1]) > 0.09);
    this.ticks.replaceChildren(...ticks.map((v) => h('span', { style: `left:${(fwd(v) * 100).toFixed(1)}%` }, v >= 1000 ? `${v / 1000} km` : `${v} m`)));
  }
}
