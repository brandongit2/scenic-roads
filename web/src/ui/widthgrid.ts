// Layers → Roads → Style → Widths by zoom: the line width table (config.ts WIDTHS, CSS px at
// weight 1 at the zooms WIDTH_Z, per class) as a grid of numbers. Typing sets a cell (Enter or
// leaving it); scrolling over the one being edited (trackpad or wheel) nudges it by 5 % a step. Edits go into
// WIDTHS in place (setWidths, saved in localStorage), so the next frame draws them; no tile reload.
import { CLASS_LABELS, DEFAULT_WIDTHS, NCLASS, WIDTHS, WIDTH_RANGE, WIDTH_Z, setWidths } from '../config';
import { h } from './dom';

const fmt = (v: number) => (v >= 10 ? v.toFixed(1) : v >= 1 ? v.toFixed(2) : v.toFixed(2).replace(/^0/, ''));

export class WidthGrid {
  readonly el: HTMLDetailsElement;
  private cells: HTMLInputElement[][] = [];

  /** `changed`: after an edit (the map to redraw). */
  constructor(private changed: () => void) {
    const put = (c: number, i: number, v: number) => {
      const t = WIDTHS.map((r) => [...r]);
      t[c][i] = v;
      this.apply(t);
    };
    const grid = h('div', { class: 'wtab' });
    grid.append(h('span', { class: 'wtab-h' }, 'CSS px'), ...WIDTH_Z.map((z) => h('span', { class: 'wtab-h' }, `z${z}`)), h('span'));
    for (let c = 0; c < NCLASS; c++) {
      const row: HTMLInputElement[] = [];
      grid.append(h('label', { title: CLASS_LABELS[c] }, CLASS_LABELS[c]));
      WIDTH_Z.forEach((z, i) => {
        const inp = h('input', { type: 'text', 'aria-label': `${CLASS_LABELS[c]} z${z}`, inputmode: 'decimal', spellcheck: 'false', title: `${CLASS_LABELS[c]} at zoom ${z}: width in CSS px (${WIDTH_RANGE[0]}–${WIDTH_RANGE[1]}; default ${DEFAULT_WIDTHS[c][i]}). Scroll over it while editing to nudge.` });
        const commit = () => {
          const v = Number(inp.value.replace(',', '.'));
          if (inp.value.trim() !== '' && Number.isFinite(v)) put(c, i, v);
          else this.sync();
        };
        inp.addEventListener('change', commit);
        inp.addEventListener('keydown', (e) => {
          if (e.key === 'Enter') inp.blur();
          else if (e.key === 'Escape') { this.sync(); inp.blur(); }
        });
        // Scrolling nudges only the cell being edited (focused); elsewhere it scrolls the panel.
        inp.addEventListener('wheel', (e) => {
          if (document.activeElement !== inp || Math.abs(e.deltaY) < 1) return;
          e.preventDefault();
          put(c, i, +(WIDTHS[c][i] * (e.deltaY < 0 ? 1.05 : 1 / 1.05)).toPrecision(3));
        }, { passive: false });
        row.push(inp);
        grid.append(inp);
      });
      const rs = h('button', { class: 'wtab-rs', title: `${CLASS_LABELS[c]}: back to the default widths` }, '↺');
      rs.addEventListener('click', () => {
        const t = WIDTHS.map((r) => [...r]);
        t[c] = [...DEFAULT_WIDTHS[c]];
        this.apply(t);
      });
      grid.append(rs);
      this.cells.push(row);
    }
    const all = h('button', { class: 'wtab-all', title: 'Every class back to the default widths' }, 'Reset to defaults');
    all.addEventListener('click', () => this.apply(null));
    this.el = h('details', { class: 'wtab-box' },
      h('summary', { title: 'Line width (CSS px, before the line weights) of each road class and rail line at zooms 4–19; between them interpolated. Edits are kept in this browser.' }, 'Widths by zoom'),
      grid, all);
    this.sync();
  }

  private apply(t: number[][] | null) {
    setWidths(t);
    this.sync();
    this.changed();
  }

  sync() {
    this.cells.forEach((row, c) => row.forEach((inp, i) => {
      if (document.activeElement !== inp) inp.value = fmt(WIDTHS[c][i]);
      inp.classList.toggle('edited', WIDTHS[c][i] !== DEFAULT_WIDTHS[c][i]);
    }));
  }
}
