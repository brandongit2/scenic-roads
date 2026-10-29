// Layers → Roads → Colour → Map: the presets (mapschemes.ts MAP_SCHEMES), each setting the Map look
// (AppState.roadMap) when clicked, and the look itself, edited freely: what colours the roads
// (road class, signed route, or an attribute), per road class its fill colour and opacity, its
// casing's colour and whether it is cased, and the attribute's category colours. Shown and applied
// only while the roads are coloured by Map; the drawing settings every colour option shares are under
// Style (ui/layers.ts).
import { CLASS_LABELS, NROAD } from '../config';
import { CAT_LABELS, MAP_KINDS, MAP_SCHEMES, lookLegend, presetLook, presetOfLook, type CatKind, type MapScheme, type RoadMapLook } from '../mapschemes';
import type { Store } from '../state';
import { h } from './dom';

const alphaOf = (c: string) => (c.length >= 9 ? parseInt(c.slice(7, 9), 16) / 255 : 1);
const withAlpha = (rgb: string, a: number) => {
  const q = Math.round(Math.min(1, Math.max(0, a)) * 255);
  return q >= 255 ? rgb.slice(0, 7) : `${rgb.slice(0, 7)}${q.toString(16).padStart(2, '0')}`;
};
/** Rows top-down: motorway first, ferry last. */
const ROWS = [8, 7, 6, 5, 4, 3, 2, 1, 0, 9].filter((c) => c < NROAD);

export class RoadMapEditor {
  readonly el: HTMLDivElement;
  private presetBtns: HTMLButtonElement[] = [];
  private kindSel: HTMLSelectElement;
  private classBox: HTMLDivElement;
  private catBox: HTMLDivElement;
  private legend: HTMLDivElement;
  private rows: { fill: HTMLInputElement; op: HTMLInputElement; out: HTMLOutputElement; cas: HTMLInputElement; cased: HTMLInputElement }[] = [];
  private catRows: { col: HTMLInputElement; op: HTMLInputElement; label: HTMLSpanElement }[] = [];

  constructor(private store: Store) {
    const L = () => store.s.roadMap;
    const set = (patch: Partial<RoadMapLook>) => store.set({ roadMap: { ...L(), ...patch } });
    const put = (key: 'fill' | 'casing', c: number, v: string) => set({ [key]: L()[key].map((x, i) => (i === c ? v : x)) });

    const presets = h('div', { class: 'schemes' });
    for (const group of [...new Set(MAP_SCHEMES.map((m) => m.group))]) {
      const row = h('div', { class: 'scheme-row' });
      for (const m of MAP_SCHEMES.filter((x) => x.group === group)) {
        const b = h('button', { class: 'scheme', title: `${m.help} (sets the settings below)`, onclick: () => store.set({ roadMap: presetLook(m, L()) }) }, preview(m), h('span', {}, m.label));
        this.presetBtns.push(b);
        row.append(b);
      }
      presets.append(h('div', { class: 'scheme-group muted' }, group), row);
    }

    this.kindSel = h('select', { title: 'What colours the roads: their class, the signed route they carry, or one of their attributes (the classes’ colours stay for the roads it doesn’t cover)' });
    for (const [k, label] of MAP_KINDS) this.kindSel.append(h('option', { value: k }, label));
    this.kindSel.addEventListener('change', () => set({ kind: this.kindSel.value as RoadMapLook['kind'] }));

    // Per class: fill colour, its opacity, casing colour, cased.
    this.classBox = h('div', { class: 'rm-tab' },
      h('span', { class: 'rm-h' }, 'Class'), h('span', { class: 'rm-h' }, 'Fill'), h('span', { class: 'rm-h' }, 'Opacity'), h('span'),
      h('span', { class: 'rm-h', title: 'Casing: the edge drawn around the road when zoomed in (its width and start zoom: Style)' }, 'Casing'), h('span'));
    for (const c of ROWS) {
      const name = CLASS_LABELS[c];
      const fill = h('input', { type: 'color', title: `${name}: fill colour` });
      fill.addEventListener('input', () => put('fill', c, withAlpha(fill.value, alphaOf(L().fill[c]))));
      const op = h('input', { type: 'range', min: 0, max: 1, step: 0.01, title: `${name}: fill opacity (see-through minor roads let the terrain show)` });
      op.addEventListener('input', () => put('fill', c, withAlpha(L().fill[c], Number(op.value))));
      const out = h('output');
      const cas = h('input', { type: 'color', title: `${name}: casing colour` });
      cas.addEventListener('input', () => put('casing', c, cas.value));
      const cased = h('input', { type: 'checkbox', title: `${name}: cased when zoomed in (bridges: Style)` });
      cased.addEventListener('change', () => set({ cased: L().cased.map((x, i) => (i === c ? cased.checked : x)) }));
      this.classBox.append(h('label', { title: name }, name), fill, op, out, cas, cased);
      this.rows[c] = { fill, op, out, cas, cased };
    }

    // An attribute's categories: colour and opacity each.
    this.catBox = h('div', { class: 'rm-cats' });
    for (let i = 0; i < 8; i++) {
      const col = h('input', { type: 'color' });
      const op = h('input', { type: 'range', min: 0, max: 1, step: 0.01 });
      const label = h('span', { class: 'muted' });
      const putCat = (v: string) => {
        const l = L();
        if (l.kind === 'class' || l.kind === 'network') return;
        const k = l.kind as CatKind;
        set({ cats: { ...l.cats, [k]: l.cats[k].map((x, j) => (j === i ? v : x)) } });
      };
      col.addEventListener('input', () => putCat(withAlpha(col.value, Number(op.value))));
      op.addEventListener('input', () => putCat(withAlpha(col.value, Number(op.value))));
      this.catBox.append(h('div', { class: 'rm-cat' }, col, label, op));
      this.catRows.push({ col, op, label });
    }
    this.legend = h('div', { class: 'scheme-legend' });

    this.el = h('div', { class: 'roadmap' },
      presets,
      h('div', { class: 'row rm-kind' }, h('span', { class: 'muted' }, 'Colour by'), this.kindSel),
      this.catBox,
      this.legend,
      this.classBox,
    );
  }

  sync() {
    const l = this.store.s.roadMap;
    const on = presetOfLook(l);
    this.presetBtns.forEach((b, i) => b.classList.toggle('on', MAP_SCHEMES[i] === on));
    this.kindSel.value = l.kind;
    for (const c of ROWS) {
      const r = this.rows[c], a = alphaOf(l.fill[c]);
      r.fill.value = l.fill[c].slice(0, 7);
      r.op.value = String(a);
      r.out.value = `${Math.round(a * 100)}`;
      r.cas.value = l.casing[c];
      r.cased.checked = l.cased[c];
    }
    const cat = l.kind !== 'class' && l.kind !== 'network' ? (l.kind as CatKind) : null;
    this.catBox.hidden = !cat;
    this.catRows.forEach((r, i) => {
      const c = cat ? l.cats[cat][i] : undefined;
      r.col.parentElement!.hidden = c === undefined;
      if (!cat || c === undefined) return;
      r.col.value = c.slice(0, 7);
      r.op.value = String(alphaOf(c));
      r.label.textContent = CAT_LABELS[cat][i];
      r.col.title = r.op.title = `${CAT_LABELS[cat][i]}: colour and opacity`;
    });
    // Signed routes: their colours by group (fixed); the other roads take their class's.
    const lg = l.kind === 'network' ? lookLegend(l) : [];
    this.legend.hidden = !lg.length;
    this.legend.replaceChildren(...lg.map(([c, label]) => h('span', { class: 'lg' }, h('i', { style: `background:${c}` }), label)));
  }
}

/** A small stack of a preset's colours for its button. */
function preview(m: MapScheme): HTMLElement {
  const cols = m.kind === 'class' ? [8, 7, 6, 5, 4, 2].map((c) => m.fill[c])
    : m.kind === 'network' ? ['#2e6fd8', '#1f8a4c', '#d8342c', '#f2c500', '#f2f2f2']
    : (m.cats ?? []).slice(1).map(([c]) => c);
  const el = h('span', { class: 'sw' });
  for (const c of cols) el.append(h('i', { style: `background:${c}` }));
  return el;
}
