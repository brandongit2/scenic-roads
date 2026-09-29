// The colour-scale controls shared by roads, rail and ferries: a histogram of what's in view with
// draggable range handles, auto-fit percentiles, Auto / Lock / Full / Equalise, the colour map,
// the low-end fade and a threshold highlight. Each owner adapts it to its own state.
import { PALETTE_ITEMS, paletteCss, paletteRgb } from '../palettes';
import type { Dist } from '../roads/stats';
import type { ThresholdDir } from '../state';
import { h, niceStep, setupCanvas } from './dom';
import { RampSelect } from './rampselect';

/** The unit-free and in-units settings of one colour scale. */
export interface ScaleValue {
  palette: string;
  auto: boolean;
  range: [number, number];
  /** Auto-fit percentiles (low, high), 0–100. */
  fit: [number, number];
  equalize: boolean;
  lowFade: number;
  lowSpan: number;
  threshold: { on: boolean; dir: ThresholdDir; value: number };
}

/** What the scale measures. */
export interface ScaleMetric {
  domain: [number, number];
  step: number;
  fmt: (v: number) => string;
  /** The top label reads "N+" (open-ended scales, like grade). */
  hiPlus?: boolean;
}

export interface ScaleOpts {
  get: () => ScaleValue;
  set: (patch: Partial<ScaleValue>) => void;
  metric: () => ScaleMetric;
  /** "roads", "rail", "ferry lines": for captions. */
  noun: string;
  /** "road length", "rail length"…: what the histogram and shares measure. */
  measure: string;
  fadeDefault: number;
  spanDefault: number;
  /** A fixed caption instead of the auto-fit controls, no dragging (e.g. relief). */
  fixedCaption?: () => string | null;
  onPreview: (palette: string | null) => void;
}

/** Road opacity at scale position u (same curve as the shaders and ferry expressions). */
export const fadeAlpha = (u: number, lowFade: number, lowSpan: number) =>
  1 - lowFade * Math.pow(1 - Math.max(0, Math.min(1, u / Math.max(lowSpan, 1e-3))), 1.5);

/** Histogram-equalisation lookup (256 steps over the range) from a distribution, or null. */
export function cdfOf(dist: Dist | null, range: [number, number]): Uint8Array | null {
  if (!dist || dist.total <= 0) return null;
  const out = new Uint8Array(256);
  const a = 1 - dist.above(range[0]), b = 1 - dist.above(range[1]);
  const span = Math.max(1e-9, b - a);
  for (let i = 0; i < 256; i++) {
    const v = range[0] + (i / 255) * (range[1] - range[0]);
    out[i] = Math.round(Math.max(0, Math.min(1, (1 - dist.above(v) - a) / span)) * 255);
  }
  return out;
}

/** Scale position 0..1 of a value (equalised when a lookup is given). */
export function scaleU(v: number, range: [number, number], cdf: Uint8Array | null): number {
  let u = Math.max(0, Math.min(1, (v - range[0]) / (range[1] - range[0] || 1e-9)));
  if (cdf) u = cdf[Math.min(255, Math.floor(u * 255 + 0.5))] / 255;
  return u;
}

/** Whether a value passes the threshold highlight ('low' follows the range's low end). */
export function passes(v: number, thr: ScaleValue['threshold'], range: [number, number]): boolean {
  if (!thr.on) return true;
  if (thr.dir === 'low') return v >= range[0];
  return thr.dir === 'below' ? v <= thr.value : v >= thr.value;
}

export class ScaleControls {
  readonly legend: HTMLDivElement;
  readonly palRow: HTMLDivElement;
  readonly fadeRow: HTMLDivElement;
  readonly thrRow: HTMLDivElement;
  private canvas: HTMLCanvasElement;
  private caption: HTMLSpanElement;
  private pills: HTMLDivElement;
  private pal: RampSelect;
  private fitLo: HTMLInputElement;
  private fitHi: HTMLInputElement;
  private fade: HTMLInputElement;
  private fadeOut: HTMLOutputElement;
  private span: HTMLInputElement;
  private spanOut: HTMLOutputElement;
  private thrOn: HTMLInputElement;
  private thrDir: HTMLSelectElement;
  private thrVal: HTMLInputElement;
  private thrOut: HTMLOutputElement;
  private thrShare: HTMLDivElement;
  private dist: Dist | null = null;
  private cdf: Uint8Array | null = null;
  private range: [number, number] = [0, 1];
  private axis: [number, number] = [0, 1];
  private drag: { which: 0 | 1; axis: [number, number] } | null = null;

  constructor(private o: ScaleOpts) {
    this.canvas = h('canvas');
    this.caption = h('span');
    this.pills = h('div', { class: 'pills' });
    // Auto-fit percentiles (low, high): anywhere in 0–100, at least 0.5 apart; the one being
    // edited keeps its value and pushes the other along.
    const pct = (i: 0 | 1) => {
      const e = h('input', {
        type: 'number', class: 'pct', min: i ? 0.5 : 0, max: i ? 100 : 99.5, step: 0.5,
        title: i ? `Upper percentile of ${o.measure} in view (100 = maximum)` : `Lower percentile of ${o.measure} in view (0 = minimum)`,
      });
      e.addEventListener('change', () => {
        const v = Math.max(i ? 0.5 : 0, Math.min(i ? 100 : 99.5, Number(e.value) || 0));
        const f: [number, number] = [...o.get().fit];
        f[i] = v;
        if (f[1] - f[0] < 0.5) f[1 - i] = i ? v - 0.5 : v + 0.5;
        o.set({ fit: f });
      });
      return e;
    };
    this.fitLo = pct(0);
    this.fitHi = pct(1);
    this.legend = h('div', { class: 'legend' }, this.canvas, h('div', { class: 'caption' }, this.caption, this.pills));

    this.pal = new RampSelect(PALETTE_ITEMS, (k) => paletteCss(k), (k) => o.set({ palette: k }), (k) => o.onPreview(k));
    this.palRow = h('div', { class: 'pal-row' }, h('span', { class: 'muted' }, 'Colours'), this.pal.el);

    this.fade = h('input', { type: 'range', min: 0, max: 1, step: 0.05, title: `How transparent ${o.noun} at the low end of the colour scale become (double-click: default)` });
    this.fadeOut = h('output');
    this.span = h('input', { type: 'range', min: 0.1, max: 1, step: 0.05, title: 'How far up the scale the fade reaches (double-click: default)' });
    this.spanOut = h('output');
    this.fade.addEventListener('input', () => o.set({ lowFade: Number(this.fade.value) }));
    this.fade.addEventListener('dblclick', () => o.set({ lowFade: o.fadeDefault }));
    this.span.addEventListener('input', () => o.set({ lowSpan: Number(this.span.value) }));
    this.span.addEventListener('dblclick', () => o.set({ lowSpan: o.spanDefault }));
    this.fadeRow = h('div', { class: 'fade' },
      h('span', { class: 'muted' }, 'Fade low end'), this.fade, this.fadeOut,
      h('span', { class: 'muted' }, 'Fade span'), this.span, this.spanOut,
    );

    this.thrOn = h('input', { type: 'checkbox' });
    this.thrDir = h('select', { title: '"above low end" follows the scale\'s left handle (auto-fit or dragged)' },
      h('option', { value: 'above' }, 'above'),
      h('option', { value: 'below' }, 'below'),
      h('option', { value: 'low' }, 'above low end'),
    );
    this.thrVal = h('input', { type: 'range' });
    this.thrOut = h('output');
    this.thrShare = h('div', { class: 'share' });
    this.thrRow = h('div', { class: 'thr' }, h('label', {}, this.thrOn, 'Highlight'), this.thrDir, this.thrVal, this.thrOut, this.thrShare);
    const thr = () => o.set({ threshold: { on: this.thrOn.checked, dir: this.thrDir.value as ThresholdDir, value: Number(this.thrVal.value) } });
    this.thrOn.addEventListener('change', thr);
    this.thrDir.addEventListener('change', () => {
      this.thrOn.checked = true;
      thr();
    });
    this.thrVal.addEventListener('input', () => {
      this.thrOn.checked = true;
      thr();
    });
    this.bindDrag();
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  /** Reflect the owner's state in the controls. */
  sync() {
    const s = this.o.get();
    const d = this.o.metric();
    this.pal.set(s.palette);
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
    this.thrVal.hidden = s.threshold.dir === 'low';
    this.thrOut.value = d.fmt(this.thrValue());
    this.pills.replaceChildren();
    const fixed = this.o.fixedCaption?.();
    if (fixed) {
      this.caption.textContent = fixed;
    } else {
      if (s.auto) this.caption.replaceChildren('Auto-fit to percentiles ', this.fitLo, '–', this.fitHi, ` of ${this.o.noun} in view`);
      else this.caption.textContent = 'Fixed range · drag the handles';
      this.fitLo.value = String(s.fit[0]);
      this.fitHi.value = String(s.fit[1]);
      this.pills.append(
        h('button', { class: 'pill' + (s.auto ? ' on' : ''), title: `Follow the ${this.o.noun} in view`, onclick: () => this.o.set({ auto: true }) }, 'Auto'),
        h('button', { class: 'pill' + (!s.auto ? ' on' : ''), title: 'Freeze the current range', onclick: () => this.o.set({ auto: false, range: [...this.range] as [number, number] }) }, 'Lock'),
        h('button', { class: 'pill', title: `Whole scale: ${d.fmt(d.domain[0])} – ${d.fmt(d.domain[1])}`, onclick: () => this.o.set({ auto: false, range: [...d.domain] as [number, number] }) }, 'Full'),
      );
    }
    this.pills.append(
      h('button', {
        class: 'pill' + (s.equalize ? ' on' : ''),
        title: `Histogram equalisation: spread colours evenly over the ${this.o.measure} in view`,
        onclick: () => this.o.set({ equalize: !s.equalize }),
      }, 'Equalise'),
    );
    this.draw();
  }

  /** The distribution in view, the range in use and the equalisation lookup (if on). */
  update(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.dist = dist;
    this.range = range;
    this.cdf = cdf;
    this.draw();
    const s = this.o.get();
    const d = this.o.metric();
    const tv = this.thrValue();
    if (s.threshold.dir === 'low') this.thrOut.value = d.fmt(tv);
    if (s.threshold.on && dist && dist.total > 0) {
      const f = dist.above(tv);
      const below = s.threshold.dir === 'below';
      this.thrShare.textContent = `${((below ? 1 - f : f) * 100).toFixed(1)} % of ${this.o.measure} in view is ${below ? 'below' : 'above'} ${d.fmt(tv)}`;
    } else {
      this.thrShare.textContent = '';
    }
  }

  /** Threshold in display units ('low' follows the scale's low end as currently drawn). */
  private thrValue(): number {
    const t = this.o.get().threshold;
    return t.dir === 'low' ? this.range[0] : t.value;
  }

  private uAt(v: number): number {
    return scaleU(v, this.range, this.o.get().equalize ? this.cdf : null);
  }

  private colourAt(v: number): string {
    return paletteRgb(this.o.get().palette, this.uAt(v));
  }

  private alphaAt(v: number): number {
    const s = this.o.get();
    return fadeAlpha(this.uAt(v), s.lowFade, s.lowSpan);
  }

  private bindDrag() {
    const c = this.canvas;
    const toVal = (x: number, axis: [number, number]) => axis[0] + (x / c.clientWidth) * (axis[1] - axis[0]);
    const toX = (v: number) => ((v - this.axis[0]) / (this.axis[1] - this.axis[0])) * c.clientWidth;
    const draggable = () => !this.o.fixedCaption?.();
    c.addEventListener('pointermove', (e) => {
      if (this.drag) {
        const v = toVal(e.offsetX, this.drag.axis);
        const r: [number, number] = [...this.range];
        r[this.drag.which] = v;
        const span = this.o.metric().step;
        if (r[1] - r[0] < span) return;
        const q = (x: number) => Math.round(x / span) * span;
        this.o.set({ auto: false, range: [q(r[0]), q(r[1])] });
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

  draw() {
    const c = this.canvas;
    if (!c.isConnected || c.clientWidth === 0) return;
    const ctx = setupCanvas(c);
    const W = c.clientWidth, H = c.clientHeight;
    ctx.clearRect(0, 0, W, H);
    const s = this.o.get();
    const d = this.o.metric();
    const st = this.dist;
    const r = this.range;
    // Axis: union of the colour range and the distribution in view (dragging keeps it fixed).
    let axis: [number, number];
    if (this.drag) axis = this.drag.axis;
    else if (this.o.fixedCaption?.() || !st || st.total <= 0) axis = [r[0], r[1]];
    else {
      const lo = Math.max(d.domain[0], Math.min(r[0], st.quantile(0.001))), hi = Math.min(d.domain[1], Math.max(r[1], st.quantile(0.999)));
      const pad = (hi - lo) * 0.04;
      axis = [lo - pad, hi + pad];
    }
    if (!(axis[1] > axis[0])) axis = [axis[0], axis[0] + 1];
    this.axis = axis;
    const X = (v: number) => ((v - axis[0]) / (axis[1] - axis[0])) * W;
    const HB = 34, SY = 38, SH = 9;

    // Histogram of length in view.
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
      const x = X(this.thrValue());
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
      const txt = lab(r[k]) + (k === 1 && d.hiPlus ? '+' : '');
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
