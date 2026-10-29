// Layers → Labels → Style: one label type's look at a time (labellook.ts), picked from a list;
// each control writes its change into AppState.labelLooks (only what differs from the style's).
import { LABEL_FONTS, LABEL_TYPES, type LabelLook, type TextCase } from '../labellook';
import type { Store } from '../state';
import * as prefs from '../prefs';
import { h } from './dom';
import { Slider, pct } from './controls';
import { onDouble } from './touch';

const CASES: [TextCase, string][] = [['none', 'As written'], ['uppercase', 'UPPER CASE'], ['lowercase', 'lower case']];

export class LabelStyleSection {
  readonly nodes: HTMLElement[];
  private defaults: Record<string, LabelLook> = {};
  private type: string;
  private pick: HTMLSelectElement;
  private body: HTMLDivElement;
  private syncs: (() => void)[] = [];
  private pickSig = '';

  constructor(private store: Store) {
    this.type = prefs.load('labels.styleType', 'state');
    if (!LABEL_TYPES.some((t) => t.key === this.type)) this.type = LABEL_TYPES[0].key;
    this.pick = h('select', { class: 'lk-pick', title: 'The type of label to style (• changed from the default)' });
    this.pick.addEventListener('change', () => {
      this.type = this.pick.value;
      prefs.save('labels.styleType', this.type);
      this.sync();
    });
    const resetType = h('button', { class: 'pill', title: 'This type’s look back to the default', onclick: () => this.patch(null) }, 'Reset type');
    const resetAll = h('button', { class: 'pill', title: 'Every label type’s look back to the default', onclick: () => this.store.set({ labelLooks: {} }) }, 'Reset all');
    this.body = h('div', { class: 'lk-body' },
      this.check('Shown', 'shown', 'Show this type of label (with its kind’s switch above)'),
      this.colour('Fill', 'colour', 'opacity', 'Text colour and its opacity (× Label opacity)'),
      this.colour('Halo', 'halo', 'haloOpacity', 'Halo (outline) colour and its opacity'),
      this.slider('Halo width', 'haloWidth', 0, 4, 0.1, (v) => `${v.toFixed(1)} px`, 'Halo width (px)'),
      this.slider('Halo blur', 'haloBlur', 0, 3, 0.1, (v) => `${v.toFixed(1)} px`, 'Halo blur (px): a soft edge'),
      this.select('Font', 'font', LABEL_FONTS.map((f) => [f, f.replace('Noto Sans ', '')]), 'Font (the weights and styles the app ships: Regular, Medium, Italic)'),
      this.slider('Size', 'size', 0.5, 2.5, 0.05, (v) => `${v.toFixed(2)}×`, 'Size × the type’s own (Label size applies on top)'),
      this.slider('Zoom growth', 'growth', -1, 1, 0.05, (v) => (v === 0 ? 'as is' : `${(2 ** v).toFixed(2)}×`),
        'Growth with zoom on top of the style\u2019s: the size × this per 6 zoom levels in, ÷ it per 6 out (from zoom 10)'),
      this.slider('Spacing', 'spacing', -0.1, 0.5, 0.01, (v) => `${v.toFixed(2)} em`, 'Letter spacing (em)'),
      this.select('Case', 'case', CASES, 'Text case'),
    );
    this.nodes = [
      h('div', { class: 'row lk-head' }, h('span', { class: 'muted' }, 'Type'), this.pick),
      this.body,
      h('div', { class: 'pills lk-reset' }, resetType, resetAll),
    ];
  }

  /** The style's own looks (once the map has its style): the controls' defaults. */
  setDefaults(d: Record<string, LabelLook>) {
    this.defaults = d;
    this.sync();
  }

  private look(): LabelLook | null {
    const d = this.defaults[this.type];
    return d ? { ...d, ...this.store.s.labelLooks[this.type] } : null;
  }
  /** A change to the type's look (null: back to the default); values equal to the default dropped. */
  private patch(p: Partial<LabelLook> | null) {
    const looks = { ...this.store.s.labelLooks };
    const d = this.defaults[this.type];
    if (p && d) {
      const o: Partial<LabelLook> = { ...looks[this.type], ...p };
      for (const k of Object.keys(o) as (keyof LabelLook)[]) if (o[k] === d[k]) delete o[k];
      if (Object.keys(o).length) looks[this.type] = o;
      else delete looks[this.type];
    } else delete looks[this.type];
    this.store.set({ labelLooks: looks });
  }

  private row(label: string, title: string, ...kids: Node[]) {
    return h('div', { class: 'row', title }, h('span', { class: 'muted' }, label), ...kids);
  }
  private check(label: string, key: 'shown', title: string) {
    const c = h('input', { type: 'checkbox', title });
    c.addEventListener('change', () => this.patch({ [key]: c.checked }));
    this.syncs.push(() => (c.checked = this.look()?.[key] ?? true));
    return h('label', { class: 'tog', title }, c, h('span', {}, label));
  }
  private colour(label: string, key: 'colour' | 'halo', op: 'opacity' | 'haloOpacity', title: string) {
    const c = h('input', { type: 'color', title });
    c.addEventListener('input', () => this.patch({ [key]: c.value }));
    const r = h('input', { type: 'range', min: 0, max: 1, step: 0.05, title: `${title} (double-click: default)` });
    r.addEventListener('input', () => this.patch({ [op]: Number(r.value) }));
    onDouble(r, () => this.patch({ [op]: this.defaults[this.type]?.[op] }));
    const out = h('output');
    this.syncs.push(() => {
      const l = this.look();
      if (!l) return;
      c.value = l[key];
      r.value = String(l[op]);
      out.value = pct(l[op]);
    });
    return this.row(label, title, h('span', { class: 'lk-colour' }, c, r), out);
  }
  private slider(label: string, key: 'haloWidth' | 'haloBlur' | 'size' | 'growth' | 'spacing', min: number, max: number, step: number, fmt: (v: number) => string, title: string) {
    const sl = new Slider({
      label, min, max, step, fmt, title,
      get: () => this.look()?.[key] ?? 0,
      set: (v) => this.patch({ [key]: v }),
    });
    // Double-click: this type's default (it changes with the type picked, so not Slider's fixed reset).
    onDouble(sl.input, () => this.patch({ [key]: this.defaults[this.type]?.[key] }));
    sl.input.title += ' (double-click: default)';
    this.syncs.push(() => sl.sync());
    return sl.el;
  }
  private select<K extends 'font' | 'case'>(label: string, key: K, opts: [string, string][], title: string) {
    const s = h('select', { title }, ...opts.map(([v, t]) => h('option', { value: v }, t)));
    s.addEventListener('change', () => this.patch({ [key]: s.value } as Partial<LabelLook>));
    this.syncs.push(() => {
      const l = this.look();
      if (l) s.value = l[key];
    });
    return this.row(label, title, s);
  }

  sync() {
    const looks = this.store.s.labelLooks;
    this.pick.replaceChildren(...LABEL_TYPES.filter((t) => this.defaults[t.key] || !Object.keys(this.defaults).length).map((t) =>
      h('option', { value: t.key }, `${looks[t.key] ? '• ' : ''}${t.label}`)));
    this.pick.value = this.type;
    this.body.classList.toggle('off', !this.defaults[this.type]);
    for (const f of this.syncs) f();
  }
}
