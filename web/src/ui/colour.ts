// How the roads are coloured (the settings panel's Roads section, ui/layers.ts): mode, legend
// histogram with draggable range handles, palette, equalisation, threshold highlight and (for the
// scenic score) the component weights; the panel's title above it all.
import type { Dist } from '../roads/stats';
import { COMPONENTS, MODES, isScenic, modeDef, type Mode } from '../scenic';
import { CLASS_LABELS } from '../config';
import { MAP_SCHEMES, mapScheme, type MapScheme } from '../mapschemes';
import { FIT_LEN_DEFAULT, pairOf, unitOf, withPair, withUnit } from '../autofit';
import { modeGroup, type Store } from '../state';
import { h } from './dom';
import { WeightGrid } from './controls';
import { PresetBar } from './presets';
import { ScaleControls } from './scale';

const BASE: [Mode, string][] = [
  ['elev', 'Elevation'],
  ['grade', 'Grade'],
  ['relief', 'Relief'],
];
const SCENIC = MODES.filter(isScenic);

export class ColourCard {
  /** How the roads are coloured (the settings panel's Roads section). */
  readonly el: HTMLDivElement;
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
  private weights: WeightGrid;
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
      len: { active: () => modeGroup(store.s.mode) === 'scenic', get: () => pairOf(store.s.fitLens, store.s.mode, FIT_LEN_DEFAULT),
        set: (v) => store.set({ fitLens: withPair(store.s.fitLens, store.s.mode, v, FIT_LEN_DEFAULT) }),
        unit: () => unitOf(store.s.fitUnits, store.s.mode), setUnit: (u) => store.set({ fitUnits: withUnit(store.s.fitUnits, store.s.mode, u) }) },
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
    this.weights = new WeightGrid(COMPONENTS, () => store.s.weights, (weights) => store.set({ weights }));
    this.weightsBox = h('div', { class: 'weights' },
      this.presetBar.el,
      this.weights.el,
      h('div', { class: 'faint note' },
        'Negative weights penalise. Views, vistas and water already account for trees (canopy heights block sight lines). Double-click (on a touch screen, double-tap) a slider to zero it.'),
    );

    root.append(h('div', { class: 'title' }, h('h1', {}, 'Scenic roads'), h('p', { class: 'sub', html: subtitle })));
    this.el = h('div', { class: 'colour-body' },
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
    this.weights.sync();
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
