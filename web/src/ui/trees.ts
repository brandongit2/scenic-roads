// "Trees" section of the Layers panel: the tree cover layer and its styling.
import { LEAF_CLASSES, TREE_PALETTES, TREE_VARS, treeColour, treeVarDef, type TreeState, type TreeStyle, type TreeVar } from '../trees';
import type { Store } from '../state';
import { h } from './dom';
import { RampSelect } from './rampselect';

export class TreeSection {
  readonly nodes: HTMLElement[];
  /** Live preview of a palette while hovering the ramp list (null: back to the chosen one). */
  onPreview: (palette: string | null) => void = () => {};
  private on: HTMLInputElement;
  private varBtns: HTMLButtonElement[] = [];
  private styleBtns: HTMLButtonElement[] = [];
  private styleRow: HTMLElement;
  private pal: RampSelect;
  private palRow: HTMLElement;
  private cut: HTMLInputElement;
  private cutOut: HTMLOutputElement;
  private cutLabel: HTMLSpanElement;
  private maskCol: HTMLInputElement;
  private maskRow: HTMLElement;
  private op: HTMLInputElement;
  private opOut: HTMLOutputElement;
  private legend: HTMLDivElement;
  private ticks: HTMLDivElement;
  private note: HTMLDivElement;
  private body: HTMLDivElement;

  constructor(private store: Store) {
    const T = (patch: Partial<TreeState>) => store.set({ trees: { ...store.s.trees, ...patch } });
    this.on = h('input', { type: 'checkbox' });
    this.on.addEventListener('change', () => T({ on: this.on.checked }));
    const seg = (items: [string, string, string][], pick: (k: string) => void, into: HTMLButtonElement[]) => {
      const el = h('div', { class: 'seg small' });
      for (const [k, label, title] of items) {
        const b = h('button', { title, onclick: () => pick(k) }, label);
        into.push(b);
        el.append(b);
      }
      return el;
    };
    const vars = seg(TREE_VARS.map((v) => [v.key, v.label, v.help]), (k) => T({ variable: k as TreeVar }), this.varBtns);
    const styles = seg([['ramp', 'Shaded', 'Colour ramp by value, hiding values below a cutoff'], ['mask', 'Mask', 'One flat colour wherever the value reaches a threshold, like forest on a paper map']],
      (k) => T({ style: k as TreeStyle }), this.styleBtns);
    this.pal = new RampSelect(TREE_PALETTES, (k) => this.gradient(k), (k) => T({ palette: k }), (k) => this.onPreview(k));
    this.cut = h('input', { type: 'range', min: 0, max: 95, step: 1 });
    this.cutOut = h('output');
    this.cutLabel = h('span', { class: 'muted' });
    this.cut.addEventListener('input', () => {
      const t = store.s.trees, v = Number(this.cut.value);
      const key = t.style === 'mask' ? (t.variable === 'cover' ? 'maskCover' : 'maskHeight') : t.variable === 'cover' ? 'cutCover' : 'cutHeight';
      T({ [key]: v } as Partial<TreeState>);
    });
    this.maskCol = h('input', { type: 'color' });
    this.maskCol.addEventListener('input', () => T({ maskColour: this.maskCol.value }));
    this.op = h('input', { type: 'range', min: 0.05, max: 1, step: 0.05, title: 'Opacity (double-click: default)' });
    this.opOut = h('output');
    this.op.addEventListener('input', () => T({ opacity: Number(this.op.value) }));
    this.op.addEventListener('dblclick', () => T({ opacity: 0.55 }));
    this.legend = h('div', { class: 'tint-bar' });
    this.ticks = h('div', { class: 'tint-ticks' });
    this.note = h('div', { class: 'faint note tree-note' });
    const row = (label: HTMLElement | string, input: HTMLElement, out?: HTMLElement) =>
      h('div', { class: 'row' }, typeof label === 'string' ? h('span', { class: 'muted' }, label) : label, input, out ?? h('span'));
    this.styleRow = row('Style', styles);
    this.palRow = row('Colours', this.pal.el);
    this.maskRow = row('Colour', this.maskCol);
    this.body = h('div', { class: 'tree-body' },
      row('Show', vars),
      this.styleRow,
      h('div', { class: 'tint-legend' }, this.legend, this.ticks),
      this.palRow,
      row(this.cutLabel, this.cut, this.cutOut),
      this.maskRow,
      row('Opacity', this.op, this.opOut),
      this.note,
    );
    this.nodes = [
      h('label', { class: 'tog', title: 'Tree cover, canopy height or forest leaf type, draped on the terrain' }, this.on, h('span', {}, 'Tree cover'), h('span', { class: 'km faint' }, '~25 m')),
      this.body,
    ];
    this.sync();
  }

  private gradient(palette: string) {
    const stops = Array.from({ length: 9 }, (_, i) => `${treeColour(palette, i / 8)} ${(i / 8) * 100}%`);
    return `linear-gradient(90deg, ${stops.join(', ')})`;
  }

  sync() {
    const t = this.store.s.trees;
    const d = treeVarDef(t.variable);
    this.on.checked = t.on;
    this.body.classList.toggle('off', !t.on);
    this.varBtns.forEach((b, i) => b.classList.toggle('on', TREE_VARS[i].key === t.variable));
    this.styleBtns.forEach((b, i) => b.classList.toggle('on', ['ramp', 'mask'][i] === t.style));
    const leaf = t.variable === 'leaf';
    const mask = !leaf && t.style === 'mask';
    this.styleRow.hidden = leaf;
    this.palRow.hidden = leaf || mask;
    this.maskRow.hidden = !mask;
    this.cut.parentElement!.hidden = leaf;
    this.pal.set(t.palette);
    this.maskCol.value = t.maskColour;
    this.op.value = String(t.opacity);
    this.opOut.value = `${Math.round(t.opacity * 100)} %`;
    if (!leaf) {
      const v = mask ? (t.variable === 'cover' ? t.maskCover : t.maskHeight) : t.variable === 'cover' ? t.cutCover : t.cutHeight;
      this.cut.min = String(mask ? 1 : 0);
      this.cut.max = String(t.variable === 'cover' ? (mask ? 100 : 95) : mask ? 40 : 39);
      this.cut.value = String(v);
      this.cutLabel.textContent = mask ? 'Forest at' : 'Hide below';
      this.cutOut.value = `${mask ? '≥ ' : ''}${v}${d.unit === '%' ? ' %' : ' m'}`;
      this.cut.title = mask ? `Shown where ${d.label.toLowerCase()} is at least this` : `Values below this are transparent`;
    }
    // Legend.
    if (leaf) {
      this.legend.style.background = 'none';
      this.legend.className = 'tree-chips';
      this.legend.replaceChildren(...LEAF_CLASSES.map(([, label, c, help]) => h('span', { class: 'lg', title: help }, h('i', { style: `background:${c}` }), label)));
      this.ticks.replaceChildren();
    } else {
      this.legend.className = 'tint-bar';
      this.legend.replaceChildren();
      const lo = mask ? (t.variable === 'cover' ? t.maskCover : t.maskHeight) : t.variable === 'cover' ? t.cutCover : t.cutHeight;
      this.legend.style.background = mask ? t.maskColour : this.gradient(t.palette);
      this.ticks.replaceChildren(h('span', {}, `${lo}${d.unit === '%' ? ' %' : ' m'}`), h('span', {}, mask ? '' : `${d.max}${d.unit === '%' ? ' %' : '+ m'}`));
    }
    this.note.textContent = d.help;
  }
}
