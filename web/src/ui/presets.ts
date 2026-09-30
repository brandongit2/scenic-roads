// Weight preset picker and editor (road score weights, rail ride factors): choose a preset; Save
// overwrites it with the current weights, Save as… adds a new one, and presets can be renamed and
// deleted (inline prompts).
import { presets as roadPresets, sameWeights, type PresetList } from '../presets';
import type { Store } from '../state';
import { h } from './dom';

/** Where the bar reads and writes the selected preset and the weights. */
export interface PresetTarget {
  list: PresetList;
  label: string;
  get: () => { preset: string; weights: number[] };
  set: (patch: { preset?: string; weights?: number[] }) => void;
}

type Prompt = { kind: 'saveas' | 'rename' } | { kind: 'delete' | 'restore' };

export class PresetBar {
  readonly el: HTMLDivElement;
  private sel: HTMLSelectElement;
  private row: HTMLDivElement;
  private prompt: Prompt | null = null;

  private t: PresetTarget;

  constructor(store: Store, target?: PresetTarget) {
    this.t = target ?? { list: roadPresets, label: 'Score weights', get: () => store.s, set: (patch) => store.set(patch) };
    const presets = this.t.list;
    this.sel = h('select', { title: 'Weight preset' });
    this.sel.addEventListener('change', () => {
      const p = presets.get(this.sel.value);
      if (p) this.t.set({ preset: p.id, weights: [...p.w] });
      this.prompt = null;
      this.render();
    });
    this.row = h('div', { class: 'pactions' });
    this.el = h('div', { class: 'presets' }, h('div', { class: 'whd' }, h('span', { class: 'muted' }, this.t.label), this.sel), this.row);
    presets.on(() => this.render());
    this.render();
  }

  render() {
    const presets = this.t.list;
    const s = this.t.get();
    const cur = presets.get(s.preset);
    const dirty = !cur || !sameWeights(s.weights, cur.w);
    // Options.
    const opts = presets.list.map((p) => h('option', { value: p.id }, p.id === cur?.id && dirty ? `${p.name} (modified)` : p.name));
    if (!cur) opts.push(h('option', { value: '' }, 'Custom'));
    this.sel.replaceChildren(...opts);
    this.sel.value = cur ? cur.id : '';

    // Actions, or the open prompt.
    const btn = (label: string, title: string, on: () => void, disabled = false) =>
      h('button', { class: 'pill', title, disabled, onclick: on }, label);
    const open = (p: Prompt) => {
      this.prompt = p;
      this.render();
    };
    const close = () => {
      this.prompt = null;
      this.render();
    };
    const pr = this.prompt;
    if (!pr) {
      this.row.replaceChildren(
        btn('Save', cur ? `Overwrite “${cur.name}” with the current weights` : 'Save the current weights as a new preset',
          () => (cur ? presets.save(cur.id, s.weights) : open({ kind: 'saveas' })), !!cur && !dirty),
        btn('Save as…', 'Save the current weights as a new preset', () => open({ kind: 'saveas' })),
        btn('Rename…', 'Rename this preset', () => open({ kind: 'rename' }), !cur),
        btn('Delete', 'Delete this preset', () => open({ kind: 'delete' }), !cur || presets.list.length <= 1),
      );
      if (presets.custom) this.row.append(btn('Built-ins', 'Replace your presets with the built-in ones', () => open({ kind: 'restore' })));
      return;
    }
    if (pr.kind === 'delete' || pr.kind === 'restore') {
      const msg = pr.kind === 'delete' ? `Delete “${cur?.name}”?` : 'Replace all presets with the built-ins?';
      this.row.replaceChildren(
        h('span', { class: 'ask' }, msg),
        h('span', { class: 'grow' }),
        btn(pr.kind === 'delete' ? 'Delete' : 'Replace', '', () => {
          this.prompt = null;
          if (pr.kind === 'delete' && cur) {
            const i = presets.list.indexOf(cur);
            presets.remove(cur.id);
            const next = presets.list[Math.min(i, presets.list.length - 1)];
            if (next) this.t.set({ preset: next.id, weights: [...next.w] });
          } else if (pr.kind === 'restore') {
            presets.restoreBuiltins();
            const p = presets.get(s.preset);
            if (!p) this.t.set({ preset: '' });
          }
          this.render();
        }),
        btn('Cancel', '', close),
      );
      return;
    }
    // Name prompt (save as / rename).
    const name = h('input', {
      type: 'text', class: 'pname', maxlength: 40,
      value: pr.kind === 'rename' ? (cur?.name ?? '') : cur ? `${cur.name} (copy)` : 'My preset',
    });
    const ok = () => {
      const v = name.value.trim();
      if (!v) return name.focus();
      this.prompt = null;
      if (pr.kind === 'rename' && cur) presets.rename(cur.id, v);
      else this.t.set({ preset: presets.add(v, s.weights, cur?.id) });
      this.render();
    };
    name.addEventListener('keydown', (e) => {
      e.stopPropagation();
      if (e.key === 'Enter') ok();
      else if (e.key === 'Escape') close();
    });
    this.row.replaceChildren(name, btn(pr.kind === 'rename' ? 'Rename' : 'Save', '', ok), btn('Cancel', '', close));
    requestAnimationFrame(() => {
      name.focus();
      name.select();
    });
  }
}
