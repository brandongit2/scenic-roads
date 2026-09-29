// Colour card: mode, legend histogram with draggable range handles, palette, equalisation,
// threshold highlight and (for the scenic score) the component weights.
import type { Dist } from '../roads/stats';
import { COMPONENTS, MODES, isScenic, modeDef, type Mode } from '../scenic';
import { CLASS_LABELS } from '../config';
import { MAP_SCHEMES, mapScheme, type MapScheme } from '../mapschemes';
import type { Store } from '../state';
import { h } from './dom';
import { PresetBar } from './presets';
import { ScaleControls } from './scale';

const BASE: [Mode, string][] = [
  ['elev', 'Elevation'],
  ['grade', 'Grade'],
  ['relief', 'Relief'],
];
const SCENIC = MODES.filter(isScenic);

export class ColourCard {
  /** Live preview of a palette while hovering the ramp list (null: back to the chosen one). */
  onPalettePreview: (palette: string | null) => void = () => {};
  private scale: ScaleControls;
  private help: HTMLDivElement;
  private modeBtns: HTMLButtonElement[] = [];
  private scenicBtn: HTMLButtonElement;
  private scenicSel: HTMLSelectElement;
  private mapBtn: HTMLButtonElement;
  private metricBox: HTMLDivElement;
  private mapBox: HTMLDivElement;
  private schemeBtns: HTMLButtonElement[] = [];
  private schemeLegend: HTMLDivElement;
  private weightsBox: HTMLDivElement;
  private presetBar: PresetBar;
  private wInputs: HTMLInputElement[] = [];
  private wOuts: HTMLOutputElement[] = [];
  private lastScenic: Mode = 'score';

  constructor(root: HTMLElement, private store: Store, subtitle: string) {
    this.help = h('div', { class: 'mode-help' });
    this.scale = new ScaleControls({
      get: () => store.s,
      set: (patch) => store.set(patch),
      metric: () => {
        const d = modeDef(store.s.mode);
        return { domain: d.domain, step: d.step, fmt: d.fmt, hiPlus: store.s.mode === 'grade' };
      },
      noun: 'roads',
      measure: 'road length',
      fadeDefault: 0.7,
      spanDefault: 0.6,
      fixedCaption: () => (store.s.mode === 'relief' ? 'Lowest → highest road in view' : null),
      onPreview: (k) => this.onPalettePreview(k),
    });

    const seg = h('div', { class: 'seg' });
    for (const [m, label] of BASE) {
      const b = h('button', { onclick: () => store.setMode(m) }, label);
      this.modeBtns.push(b);
      seg.append(b);
    }
    this.scenicBtn = h('button', { title: 'Scenic metrics', onclick: () => store.setMode(this.lastScenic) }, 'Scenic');
    seg.append(this.scenicBtn);
    this.mapBtn = h('button', { title: 'Street-map colours', onclick: () => store.setMode('map') }, 'Map');
    seg.append(this.mapBtn);

    // Street-map schemes, grouped, each with a preview of its colours.
    this.schemeLegend = h('div', { class: 'scheme-legend' });
    this.mapBox = h('div', { class: 'schemes' });
    for (const group of [...new Set(MAP_SCHEMES.map((m) => m.group))]) {
      const row = h('div', { class: 'scheme-row' });
      for (const m of MAP_SCHEMES.filter((x) => x.group === group)) {
        const b = h('button', { class: 'scheme', title: m.help, onclick: () => store.set({ mapScheme: m.key }) }, preview(m), h('span', {}, m.label));
        this.schemeBtns.push(b);
        row.append(b);
      }
      this.mapBox.append(h('div', { class: 'scheme-group muted' }, group), row);
    }
    this.mapBox.append(this.schemeLegend);
    this.scenicSel = h('select', { class: 'scenic-sel' });
    for (const m of SCENIC) this.scenicSel.append(h('option', { value: m.key }, m.label));
    this.scenicSel.addEventListener('change', () => store.setMode(this.scenicSel.value as Mode));

    // Weights.
    this.presetBar = new PresetBar(store);
    const grid = h('div', { class: 'wgrid' });
    COMPONENTS.forEach((c, i) => {
      const inp = h('input', { type: 'range', min: -1.5, max: 2, step: 0.1, title: c.help });
      const out = h('output');
      inp.addEventListener('input', () => {
        const w = [...store.s.weights];
        w[i] = Number(inp.value);
        store.set({ weights: w });
      });
      inp.addEventListener('dblclick', () => {
        const w = [...store.s.weights];
        w[i] = 0;
        store.set({ weights: w });
      });
      this.wInputs.push(inp);
      this.wOuts.push(out);
      grid.append(h('label', { title: c.help }, c.label), inp, out);
    });
    this.weightsBox = h('div', { class: 'weights' },
      this.presetBar.el,
      grid,
      h('div', { class: 'faint note' },
        'Negative weights penalise. Views, vistas and water already account for trees (canopy heights block sight lines). Double-click a slider to zero it.'),
    );

    root.append(
      h('div', { class: 'title' }, h('h1', {}, 'Scenic roads'), h('p', { class: 'sub', html: subtitle })),
      h('div', { class: 'bd' },
        seg,
        this.scenicSel,
        this.help,
        this.mapBox,
        (this.metricBox = h('div', {},
          this.scale.legend,
          this.scale.palRow,
          this.scale.fadeRow,
          this.scale.thrRow,
          this.weightsBox,
        )),
      ),
    );

    this.sync();
  }

  /** Reflect store state in the controls. */
  sync() {
    const s = this.store.s;
    const d = modeDef(s.mode);
    const scenic = isScenic(d);
    const map = s.mode === 'map';
    if (scenic) this.lastScenic = s.mode;
    this.modeBtns.forEach((b, i) => b.classList.toggle('on', BASE[i][0] === s.mode));
    this.scenicBtn.classList.toggle('on', scenic);
    this.mapBtn.classList.toggle('on', map);
    this.scenicSel.hidden = !scenic;
    this.mapBox.hidden = !map;
    this.metricBox.hidden = map;
    const sch = mapScheme(s.mapScheme);
    this.schemeBtns.forEach((b, i) => b.classList.toggle('on', MAP_SCHEMES[i].key === sch.key));
    this.schemeLegend.replaceChildren(...legendOf(sch).map(([c, label]) => h('span', { class: 'lg' }, h('i', { style: `background:${c}` }), label)));
    this.scenicSel.value = this.lastScenic;
    this.help.textContent = d.help;
    this.weightsBox.hidden = s.mode !== 'score';
    this.presetBar.render();
    COMPONENTS.forEach((_, i) => {
      this.wInputs[i].value = String(s.weights[i]);
      const v = s.weights[i];
      this.wOuts[i].value = v === 0 ? '·' : (v > 0 ? '+' : '') + v.toFixed(1);
      this.wOuts[i].classList.toggle('neg', v < 0);
      this.wOuts[i].classList.toggle('zero', v === 0);
    });
    this.scale.sync();
  }

  update(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.scale.update(dist, range, cdf);
  }
}

/** A small stack of a scheme's colours for its button. */
function preview(m: MapScheme): HTMLElement {
  const cols = m.kind === 'class' ? [8, 7, 6, 5, 4, 2].map((c) => m.fill[c])
    : m.kind === 'network' ? ['#2e6fd8', '#1f8a4c', '#d8342c', '#f2c500', '#f2f2f2']
    : (m.cats ?? []).slice(1).map(([c]) => c);
  const el = h('span', { class: 'sw' });
  for (const c of cols) el.append(h('i', { style: `background:${c}` }));
  return el;
}

/** Legend entries of a scheme. */
function legendOf(m: MapScheme): [string, string][] {
  if (m.kind === 'class') return [8, 7, 6, 5, 4, 2, 0, 9].map((c) => [m.fill[c], CLASS_LABELS[c]]);
  if (m.kind === 'network') return m.legend ?? [];
  return m.cats ?? [];
}
