// The settings panel's Depth section (ui/layers.ts): the depth's two layers, each with its switch:
// the depth colour and the floor's shading (its colour scale the other layers' ScaleControls: the
// depths in view, the range, auto-fit and equalisation, the ramp), and the depth contours with
// their labels; and the 3D floor's switch (depth.ts).
import type { Dist } from '../roads/stats';
import { DEPTH_DEFAULTS, DEPTH_DOMAIN, DEPTH_PALETTES, depthColour, rgba, scaleFns, type DepthState } from '../depthlook';
import type { ScaleFields, Store } from '../state';
import { ColourPick, Slider, pct } from './controls';
import { h } from './dom';
import { ScaleControls, scaleU } from './scale';

const COMPASS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'];

export class DepthSection {
  readonly nodes: HTMLElement[];
  /** The colour layer's switch (the section's header). */
  readonly on: HTMLInputElement;
  /** Live preview of a palette while hovering the ramp list (null: back to the chosen one). */
  onPreview: (palette: string | null) => void = () => {};
  private body: HTMLDivElement;
  private scaleCtl: ScaleControls;
  /** The range in use and the equalisation lookup (update). */
  private range: [number, number] = [0, 300];
  private cdf: Uint8Array | null = null;
  private scaleBtns: HTMLButtonElement[] = [];
  private sliders: Slider[];
  private colours: ColourPick[];
  private dry: HTMLInputElement;
  private contours: HTMLInputElement;
  private contourBox: HTMLDivElement;
  private intervalBtns: HTMLButtonElement[] = [];
  private fixed: HTMLSelectElement;
  private index: HTMLSelectElement;
  private labels: HTMLInputElement;
  private floor: HTMLInputElement;
  private note: HTMLDivElement;
  /** The note's text before the estimates (setEstimated). */
  private static readonly NOTE = 'Below the water\'s surface: mean sea level at sea, the lake\'s surface on lakes. Each source at the detail of its grids. Not for navigation.';

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
    const C = () => S().colours;
    this.scaleCtl = new ScaleControls({
      get: C,
      set: (patch: Partial<ScaleFields>) => D({ colours: { ...C(), ...patch } }),
      metric: () => ({ domain: DEPTH_DOMAIN, step: 1, fmt: (v: number) => (v >= 1000 ? `${+(v / 1000).toFixed(2)} km` : `${Math.round(v)} m`), hiPlus: true }),
      noun: 'water',
      measure: 'water area',
      fadeDefault: 0,
      spanDefault: DEPTH_DEFAULTS.colours.lowSpan,
      palettes: { items: DEPTH_PALETTES, css: (k) => this.gradient(k) },
      colourAt: (v) => [rgba(depthColour(C().palette, this.position(v))), 1],
      onPreview: (k) => this.onPreview(k),
    });
    const scales = seg([['linear', 'Linear', 'Colour in proportion to depth'], ['log', 'Log', 'More colours in the shallows, where most of the detail is']],
      (k) => D({ scale: k as DepthState['scale'] }), this.scaleBtns);
    this.sliders = [
      new Slider({ label: 'Opacity', min: 0.1, max: 1, step: 0.05, reset: DEPTH_DEFAULTS.opacity, get: () => S().opacity, set: (opacity) => D({ opacity }), fmt: pct }),
      new Slider({ label: 'Shading', min: 0, max: 1, step: 0.05, reset: DEPTH_DEFAULTS.shade, get: () => S().shade, set: (shade) => D({ shade }), fmt: (v) => (v ? pct(v) : 'off'), title: 'How strongly the floor is shaded' }),
      new Slider({ label: 'Light from', min: 0, max: 359, step: 1, reset: DEPTH_DEFAULTS.light, get: () => S().light, set: (light) => D({ light }), fmt: (v) => `${Math.round(v)}° ${COMPASS[Math.round(v / 45) % 8]}`, title: 'Where the light comes from (map north)' }),
    ];
    this.dry = h('input', { type: 'checkbox' });
    this.dry.addEventListener('change', () => D({ dry: this.dry.checked }));
    const dryCol = new ColourPick({ label: 'Drying', get: () => S().dryColour, set: (dryColour) => D({ dryColour }), reset: DEPTH_DEFAULTS.dryColour, title: 'The colour of water that dries at low tide' });
    this.note = h('div', { class: 'faint note' }, DepthSection.NOTE);
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
    this.floor = h('input', { type: 'checkbox' });
    this.floor.addEventListener('change', () => D({ floor: this.floor.checked }));
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
      h('div', { class: 'tint-scale' }, this.scaleCtl.legend, this.scaleCtl.palRow),
      row('Spacing', scales),
      ...this.sliders.slice(0, 3).map((s) => s.el),
      tog(this.dry, 'Tidal flats in their own colour', 'Water that dries at low tide (above the chart\'s low water)'),
      dryCol.el,
      tog(this.contours, 'Depth contours', 'Lines of equal depth, labelled'),
      this.contourBox,
      tog(this.floor, '3D sea and lake floor', 'The 3D terrain shows the floor under the water, its surface drawn over it, see-through (with 3D terrain on)'),
      this.note,
    );
    this.nodes = [this.body];
    this.sync();
  }

  /** The sources whose depths are estimates (their labels, the catalogue's), named in the note. */
  setEstimated(labels: string[]) {
    this.note.textContent = labels.length
      ? `${DepthSection.NOTE} Estimated where nothing measured has the depth: ${labels.join(', ')} (the footer says which, on hover).`
      : DepthSection.NOTE;
  }

  private gradient(palette: string) {
    const stops = Array.from({ length: 9 }, (_, i) => `${rgba(depthColour(palette, i / 8))} ${(i / 8) * 100}%`);
    return `linear-gradient(90deg, ${stops.join(', ')})`;
  }

  sync() {
    const d = this.store.s.depth;
    this.on.checked = d.on;
    this.scaleBtns.forEach((b, i) => b.classList.toggle('on', ['linear', 'log'][i] === d.scale));
    this.scaleCtl.sync();
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
    this.floor.checked = d.floor;
  }

  /** The depths in view (null: not measured: neither auto-fit nor equalised), the range the ramp
   * spans now and the equalisation lookup (depth.ts). */
  update(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.range = range;
    this.cdf = cdf;
    this.scaleCtl.update(dist, range, cdf);
  }

  /** A depth's place on the ramp, as the map colours it (depth.ts position). */
  private position(v: number): number {
    if (this.cdf) return scaleU(v, this.range, this.cdf);
    return scaleFns(this.store.s.depth.scale, ...this.range)[0](Math.max(0, v));
  }
}
