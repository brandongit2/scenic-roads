// Colour card: mode, legend histogram with draggable range handles, palette, equalisation,
// threshold highlight and (for the scenic score) the component weights.
import { PALETTES, paletteCss, paletteRgb } from '../palettes';
import type { Dist } from '../roads/stats';
import { COMPONENTS, MODES, PRESETS, modeDef, type Mode } from '../scenic';
import type { Store } from '../state';
import { h, niceStep, setupCanvas } from './dom';

const BASE: [Mode, string][] = [
  ['elev', 'Elevation'],
  ['grade', 'Grade'],
  ['relief', 'Relief'],
];
const SCENIC = MODES.filter((m) => m.id >= 3);

export class ColourCard {
  private canvas: HTMLCanvasElement;
  private caption: HTMLSpanElement;
  private pills: HTMLDivElement;
  private help: HTMLDivElement;
  private modeBtns: HTMLButtonElement[] = [];
  private scenicBtn: HTMLButtonElement;
  private scenicSel: HTMLSelectElement;
  private palBtns: HTMLButtonElement[] = [];
  private thrOn: HTMLInputElement;
  private thrDir: HTMLSelectElement;
  private thrVal: HTMLInputElement;
  private thrOut: HTMLOutputElement;
  private thrShare: HTMLDivElement;
  private fitLo: HTMLInputElement;
  private fitHi: HTMLInputElement;
  private fade: HTMLInputElement;
  private fadeOut: HTMLOutputElement;
  private span: HTMLInputElement;
  private spanOut: HTMLOutputElement;
  private weightsBox: HTMLDivElement;
  private presetSel: HTMLSelectElement;
  private wInputs: HTMLInputElement[] = [];
  private wOuts: HTMLOutputElement[] = [];
  private dist: Dist | null = null;
  private cdf: Uint8Array | null = null;
  private range: [number, number] = [0, 1];
  private axis: [number, number] = [0, 1];
  private drag: { which: 0 | 1; axis: [number, number] } | null = null;
  private lastScenic: Mode = 'score';

  constructor(root: HTMLElement, private store: Store, subtitle: string) {
    this.canvas = h('canvas');
    this.caption = h('span');
    this.pills = h('div', { class: 'pills' });
    this.help = h('div', { class: 'mode-help' });
    this.thrOn = h('input', { type: 'checkbox' });
    this.thrDir = h('select', {}, h('option', { value: 'above' }, 'above'), h('option', { value: 'below' }, 'below'));
    this.thrVal = h('input', { type: 'range' });
    this.thrOut = h('output');
    this.thrShare = h('div', { class: 'share' });
    // Auto-fit percentiles (low, high).
    const pct = (i: 0 | 1) => {
      const e = h('input', {
        type: 'number', class: 'pct', min: i ? 50 : 0, max: i ? 100 : 50, step: 0.5,
        title: i ? 'Upper percentile of road length in view (100 = maximum)' : 'Lower percentile of road length in view (0 = minimum)',
      });
      e.addEventListener('change', () => {
        const v = Math.max(i ? 50 : 0, Math.min(i ? 100 : 50, Number(e.value) || 0));
        const f: [number, number] = [...this.store.s.fit];
        f[i] = v;
        if (f[1] <= f[0]) f[i] = i ? f[0] + 0.5 : f[1] - 0.5;
        this.store.set({ fit: f });
      });
      return e;
    };
    this.fitLo = pct(0);
    this.fitHi = pct(1);
    this.fade = h('input', { type: 'range', min: 0, max: 1, step: 0.05, title: 'How transparent roads at the low end of the colour scale become (double-click: default)' });
    this.fadeOut = h('output');
    this.span = h('input', { type: 'range', min: 0.1, max: 1, step: 0.05, title: 'How far up the scale the fade reaches' });
    this.spanOut = h('output');
    this.fade.addEventListener('input', () => store.set({ lowFade: Number(this.fade.value) }));
    this.fade.addEventListener('dblclick', () => store.set({ lowFade: 0.7 }));
    this.span.addEventListener('input', () => store.set({ lowSpan: Number(this.span.value) }));
    this.span.addEventListener('dblclick', () => store.set({ lowSpan: 0.6 }));

    const seg = h('div', { class: 'seg' });
    for (const [m, label] of BASE) {
      const b = h('button', { onclick: () => store.setMode(m) }, label);
      this.modeBtns.push(b);
      seg.append(b);
    }
    this.scenicBtn = h('button', { title: 'Scenic metrics', onclick: () => store.setMode(this.lastScenic) }, 'Scenic');
    seg.append(this.scenicBtn);
    this.scenicSel = h('select', { class: 'scenic-sel' });
    for (const m of SCENIC) this.scenicSel.append(h('option', { value: m.key }, m.label));
    this.scenicSel.addEventListener('change', () => store.setMode(this.scenicSel.value as Mode));

    const pals = h('div', { class: 'palettes' });
    for (const p of PALETTES) {
      const b = h('button', { title: p.label, onclick: () => store.set({ palette: p.key }) }, h('span', {}, p.label));
      b.style.background = paletteCss(p.key);
      this.palBtns.push(b);
      pals.append(b);
    }

    // Weights.
    this.presetSel = h('select');
    for (const [k, p] of Object.entries(PRESETS)) this.presetSel.append(h('option', { value: k }, p.label));
    this.presetSel.append(h('option', { value: 'custom' }, 'Custom'));
    this.presetSel.addEventListener('change', () => {
      const k = this.presetSel.value;
      if (PRESETS[k]) store.set({ preset: k, weights: [...PRESETS[k].w] });
    });
    const grid = h('div', { class: 'wgrid' });
    COMPONENTS.forEach((c, i) => {
      const inp = h('input', { type: 'range', min: -1.5, max: 2, step: 0.1, title: c.help });
      const out = h('output');
      inp.addEventListener('input', () => {
        const w = [...store.s.weights];
        w[i] = Number(inp.value);
        store.set({ weights: w, preset: 'custom' });
      });
      inp.addEventListener('dblclick', () => {
        const w = [...store.s.weights];
        w[i] = 0;
        store.set({ weights: w, preset: 'custom' });
      });
      this.wInputs.push(inp);
      this.wOuts.push(out);
      grid.append(h('label', { title: c.help }, c.label), inp, out);
    });
    this.weightsBox = h('div', { class: 'weights' },
      h('div', { class: 'whd' },
        h('span', { class: 'muted' }, 'Score weights'),
        this.presetSel,
      ),
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
        h('div', { class: 'legend' }, this.canvas, h('div', { class: 'caption' }, this.caption, this.pills)),
        pals,
        h('div', { class: 'fade' },
          h('span', { class: 'muted' }, 'Fade low end'), this.fade, this.fadeOut,
          h('span', { class: 'muted' }, 'Fade span'), this.span, this.spanOut,
        ),
        h('div', { class: 'thr' },
          h('label', {}, this.thrOn, 'Highlight'),
          this.thrDir,
          this.thrVal,
          this.thrOut,
          this.thrShare,
        ),
        this.weightsBox,
      ),
    );

    const thr = () => store.set({ threshold: { on: this.thrOn.checked, dir: this.thrDir.value as 'above' | 'below', value: Number(this.thrVal.value) } });
    this.thrOn.addEventListener('change', thr);
    this.thrDir.addEventListener('change', thr);
    this.thrVal.addEventListener('input', () => {
      if (!this.thrOn.checked) this.thrOn.checked = true;
      thr();
    });
    this.bindDrag();
    this.sync();
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  /** Reflect store state in the controls. */
  sync() {
    const s = this.store.s;
    const d = modeDef(s.mode);
    const scenic = d.id >= 3;
    if (scenic) this.lastScenic = s.mode;
    this.modeBtns.forEach((b, i) => b.classList.toggle('on', BASE[i][0] === s.mode));
    this.scenicBtn.classList.toggle('on', scenic);
    this.scenicSel.hidden = !scenic;
    this.scenicSel.value = this.lastScenic;
    this.help.textContent = d.help;
    this.palBtns.forEach((b, i) => b.classList.toggle('on', PALETTES[i].key === s.palette));
    this.fade.value = String(s.lowFade);
    this.fadeOut.value = s.lowFade === 0 ? 'off' : `${Math.round(s.lowFade * 100)} %`;
    this.span.value = String(s.lowSpan);
    this.spanOut.value = `${Math.round(s.lowSpan * 100)} %`;
    this.span.disabled = s.lowFade === 0;
    this.thrOn.checked = s.threshold.on;
    this.thrDir.value = s.threshold.dir;
    this.thrVal.min = String(d.domain[0]);
    this.thrVal.max = String(d.domain[1]);
    this.thrVal.step = String(d.step);
    this.thrVal.value = String(s.threshold.value);
    this.thrOut.value = d.fmt(s.threshold.value);
    this.pills.replaceChildren();
    if (s.mode === 'relief') {
      this.caption.textContent = 'Lowest → highest road in view';
    } else {
      if (s.auto) this.caption.replaceChildren('Auto-fit to percentiles ', this.fitLo, '–', this.fitHi, ' of roads in view');
      else this.caption.textContent = 'Fixed range · drag the handles';
      this.fitLo.value = String(s.fit[0]);
      this.fitHi.value = String(s.fit[1]);
      this.pills.append(
        h('button', { class: 'pill' + (s.auto ? ' on' : ''), title: 'Follow the roads in view', onclick: () => this.store.set({ auto: true }) }, 'Auto'),
        h('button', { class: 'pill' + (!s.auto ? ' on' : ''), title: 'Freeze the current range', onclick: () => this.store.set({ auto: false, range: [...this.range] as [number, number] }) }, 'Lock'),
        h('button', { class: 'pill', title: `Whole scale: ${d.fmt(d.domain[0])} – ${d.fmt(d.domain[1])}`, onclick: () => this.store.set({ auto: false, range: [...d.domain] as [number, number] }) }, 'Full'),
      );
    }
    this.pills.append(
      h('button', {
        class: 'pill' + (s.equalize ? ' on' : ''),
        title: 'Histogram equalisation: spread colours evenly over the road length in view',
        onclick: () => this.store.set({ equalize: !s.equalize }),
      }, 'Equalise'),
    );
    this.weightsBox.hidden = s.mode !== 'score';
    this.presetSel.value = s.preset;
    COMPONENTS.forEach((_, i) => {
      this.wInputs[i].value = String(s.weights[i]);
      const v = s.weights[i];
      this.wOuts[i].value = v === 0 ? '·' : (v > 0 ? '+' : '') + v.toFixed(1);
      this.wOuts[i].classList.toggle('neg', v < 0);
      this.wOuts[i].classList.toggle('zero', v === 0);
    });
    this.draw();
  }

  update(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.dist = dist;
    this.range = range;
    this.cdf = cdf;
    this.draw();
    const s = this.store.s;
    if (s.threshold.on && dist && dist.total > 0) {
      const f = dist.above(s.threshold.value);
      const share = s.threshold.dir === 'above' ? f : 1 - f;
      this.thrShare.textContent = `${(share * 100).toFixed(1)} % of road length in view is ${s.threshold.dir} ${modeDef(s.mode).fmt(s.threshold.value)}`;
    } else {
      this.thrShare.textContent = '';
    }
  }

  private uAt(v: number): number {
    const r = this.range;
    let u = Math.max(0, Math.min(1, (v - r[0]) / (r[1] - r[0])));
    if (this.store.s.equalize && this.cdf) u = this.cdf[Math.min(255, Math.floor(u * 255 + 0.5))] / 255;
    return u;
  }

  private colourAt(v: number): string {
    return paletteRgb(this.store.s.palette, this.uAt(v));
  }

  /** Road opacity at a value (low-end fade, same curve as the shader). */
  private alphaAt(v: number): number {
    const s = this.store.s;
    return 1 - s.lowFade * Math.pow(1 - Math.max(0, Math.min(1, this.uAt(v) / Math.max(s.lowSpan, 1e-3))), 1.5);
  }

  private bindDrag() {
    const c = this.canvas;
    const toVal = (x: number, axis: [number, number]) => axis[0] + (x / c.clientWidth) * (axis[1] - axis[0]);
    const toX = (v: number) => ((v - this.axis[0]) / (this.axis[1] - this.axis[0])) * c.clientWidth;
    const draggable = () => this.store.s.mode !== 'relief';
    c.addEventListener('pointermove', (e) => {
      if (this.drag) {
        const v = toVal(e.offsetX, this.drag.axis);
        const r: [number, number] = [...this.range];
        r[this.drag.which] = v;
        const span = modeDef(this.store.s.mode).step;
        if (r[1] - r[0] < span) return;
        const q = (x: number) => Math.round(x / span) * span;
        this.store.set({ auto: false, range: [q(r[0]), q(r[1])] });
        return;
      }
      const near = draggable() && Math.min(Math.abs(e.offsetX - toX(this.range[0])), Math.abs(e.offsetX - toX(this.range[1]))) < 10;
      c.classList.toggle('drag', near);
    });
    c.addEventListener('pointerdown', (e) => {
      if (!draggable()) return;
      const d0 = Math.abs(e.offsetX - toX(this.range[0])), d1 = Math.abs(e.offsetX - toX(this.range[1]));
      if (Math.min(d0, d1) > 14) return;
      this.drag = { which: d0 <= d1 ? 0 : 1, axis: [...this.axis] as [number, number] };
      c.setPointerCapture(e.pointerId);
    });
    const end = () => (this.drag = null);
    c.addEventListener('pointerup', end);
    c.addEventListener('pointercancel', end);
  }

  private draw() {
    const c = this.canvas;
    const ctx = setupCanvas(c);
    const W = c.clientWidth, H = c.clientHeight;
    ctx.clearRect(0, 0, W, H);
    const s = this.store.s;
    const d = modeDef(s.mode);
    const st = this.dist;
    const r = this.range;
    // Axis: union of the colour range and the distribution in view (dragging keeps it fixed).
    let axis: [number, number];
    if (this.drag) axis = this.drag.axis;
    else if (s.mode === 'relief' || !st || st.total <= 0) axis = [r[0], r[1]];
    else {
      const lo = Math.max(d.domain[0], Math.min(r[0], st.quantile(0.001))), hi = Math.min(d.domain[1], Math.max(r[1], st.quantile(0.999)));
      const pad = (hi - lo) * 0.04;
      axis = [lo - pad, hi + pad];
    }
    if (!(axis[1] > axis[0])) axis = [axis[0], axis[0] + 1];
    this.axis = axis;
    const X = (v: number) => ((v - axis[0]) / (axis[1] - axis[0])) * W;
    const HB = 34, SY = 38, SH = 9;

    // Histogram of road length.
    const NBIN = Math.max(20, Math.floor(W / 4));
    if (st && st.total > 0) {
      const bins = st.rebin(axis[0], axis[1], NBIN);
      let max = 0;
      for (const b of bins) max = Math.max(max, b);
      const bw = W / NBIN;
      for (let i = 0; i < NBIN; i++) {
        if (!bins[i]) continue;
        const v = axis[0] + ((i + 0.5) / NBIN) * (axis[1] - axis[0]);
        const hgt = Math.max(1, Math.sqrt(bins[i] / max) * HB);
        const inRange = v >= r[0] && v <= r[1];
        ctx.fillStyle = this.colourAt(v);
        ctx.globalAlpha = (inRange ? 0.95 : 0.35) * Math.max(0.15, this.alphaAt(v));
        ctx.fillRect(i * bw + 0.5, HB - hgt, Math.max(1, bw - 1), hgt);
      }
      ctx.globalAlpha = 1;
    } else {
      ctx.fillStyle = 'rgba(255,255,255,0.05)';
      ctx.fillRect(0, HB - 1, W, 1);
    }

    // Colour strip: clamped outside the range.
    const grd = ctx.createLinearGradient(0, 0, W, 0);
    const NS = s.equalize ? 64 : 24;
    for (let i = 0; i <= NS; i++) {
      const x = i / NS;
      const v = axis[0] + x * (axis[1] - axis[0]);
      grd.addColorStop(x, this.colourAt(v).replace('rgb(', 'rgba(').replace(')', `,${Math.max(0.12, this.alphaAt(v)).toFixed(3)})`));
    }
    ctx.fillStyle = grd;
    roundRect(ctx, 0, SY, W, SH, 3);
    ctx.fill();

    // Threshold marker.
    if (s.threshold.on) {
      const x = X(s.threshold.value);
      if (x >= 0 && x <= W) {
        ctx.strokeStyle = '#fff';
        ctx.setLineDash([2, 2]);
        ctx.beginPath();
        ctx.moveTo(x + 0.5, 0);
        ctx.lineTo(x + 0.5, SY + SH);
        ctx.stroke();
        ctx.setLineDash([]);
      }
    }

    // Range handles + labels.
    ctx.font = '600 11px -apple-system, system-ui, sans-serif';
    ctx.textBaseline = 'top';
    const lab = (v: number) => d.fmt(v);
    const xs = [X(r[0]), X(r[1])];
    for (let k = 0; k < 2; k++) {
      const x = Math.max(0, Math.min(W, xs[k]));
      ctx.fillStyle = '#fff';
      ctx.fillRect(x - 1, SY - 3, 2, SH + 6);
      const txt = lab(r[k]) + (k === 1 && s.mode === 'grade' ? '+' : '');
      const tw = ctx.measureText(txt).width;
      const tx = k === 0 ? Math.max(0, Math.min(x - tw / 2, xs[1] - tw - 8)) : Math.min(W - tw, Math.max(x - tw / 2, xs[0] + ctx.measureText(lab(r[0])).width + 8));
      ctx.fillStyle = '#e8ecf2';
      ctx.fillText(txt, tx, SY + SH + 5);
    }
    // Faint axis ticks.
    ctx.fillStyle = 'rgba(255,255,255,0.28)';
    const step = niceStep(axis[1] - axis[0], 5);
    for (let v = Math.ceil(axis[0] / step) * step; v <= axis[1]; v += step) ctx.fillRect(X(v), HB + 1, 1, 2);
  }
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, hh: number, r: number) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + hh, r);
  ctx.arcTo(x + w, y + hh, x, y + hh, r);
  ctx.arcTo(x, y + hh, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}
