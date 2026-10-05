// Controls the settings panel shares: a labelled slider row (its value written beside it, a
// double-click back to the default), a grid of a score's component weights, and a range filter
// over a histogram of what is in view.
import type { Dist } from '../roads/stats';
import { axisPos, axisValue, type Axis } from '../stopfilters';
import { h, setupCanvas } from './dom';
import { onDouble } from './touch';

export interface SliderOpts {
  label: string;
  /** The slider's ends and step (on its scale, where it has one). */
  min: number;
  max: number;
  step: number;
  get: () => number;
  set: (v: number) => void;
  /** The value as written beside the slider. */
  fmt: (v: number) => string;
  /** Double-click: back to this. */
  reset?: number;
  title?: string;
  /** Row class (layout variants: 'lw', 'lw dens', 'sub'…). */
  cls?: string;
  /** A scale of its own (e.g. log2): the slider's position for a value, and back. */
  scale?: { to: (v: number) => number; from: (p: number) => number };
  /** Disabled while this says so. */
  disabled?: () => boolean;
}

/** A labelled slider row. */
export class Slider {
  readonly el: HTMLDivElement;
  readonly input: HTMLInputElement;
  private out: HTMLOutputElement;

  constructor(private o: SliderOpts) {
    const from = o.scale?.from ?? ((p: number) => p);
    this.input = h('input', { type: 'range', min: o.min, max: o.max, step: o.step, title: `${o.title ?? o.label}${o.reset !== undefined ? ' (double-click: default)' : ''}` });
    this.out = h('output');
    this.input.addEventListener('input', () => o.set(from(Number(this.input.value))));
    if (o.reset !== undefined) onDouble(this.input, () => o.set(o.reset!));
    this.el = h('div', { class: `row ${o.cls ?? ''}`.trim(), title: o.title }, h('span', { class: 'muted' }, o.label), this.input, this.out);
  }

  sync() {
    const v = this.o.get();
    this.input.value = String(this.o.scale ? this.o.scale.to(v) : v);
    this.out.value = this.o.fmt(v);
    this.input.disabled = !!this.o.disabled?.();
  }
}

/** Percent, for opacity sliders. */
export const pct = (v: number) => `${Math.round(v * 100)} %`;

/** The components of a weighted score, a slider each (−1.5 … 2; double-click: 0). */
export class WeightGrid {
  readonly el: HTMLDivElement;
  private ins: HTMLInputElement[] = [];
  private outs: HTMLOutputElement[] = [];

  constructor(comps: readonly { label: string; help: string }[], private get: () => number[], set: (w: number[]) => void) {
    this.el = h('div', { class: 'wgrid' });
    comps.forEach((c, i) => {
      const put = (v: number) => {
        const w = [...this.get()];
        w[i] = v;
        set(w);
      };
      const inp = h('input', { type: 'range', min: -1.5, max: 2, step: 0.1, title: c.help });
      inp.addEventListener('input', () => put(Number(inp.value)));
      onDouble(inp, () => put(0));
      const out = h('output');
      this.ins.push(inp);
      this.outs.push(out);
      this.el.append(h('label', { title: c.help }, c.label), inp, out);
    });
  }

  sync() {
    const w = this.get();
    this.ins.forEach((inp, i) => {
      const v = w[i];
      inp.value = String(v);
      this.outs[i].value = v === 0 ? '·' : (v > 0 ? '+' : '') + v.toFixed(1);
      this.outs[i].classList.toggle('neg', v < 0);
      this.outs[i].classList.toggle('zero', v === 0);
    });
  }
}

export interface RangeFilterOpts {
  label: string;
  unit: string;
  title?: string;
  /** The axis: its value range and scale (log: lengths, counts; age: years far back; else even). */
  domain: [number, number];
  axis?: Axis;
  /** The filter's setting: on, and its limits (0: none on that side). */
  get: () => { on: boolean; min: number; max: number };
  set: (patch: { on?: boolean; min?: number; max?: number }) => void;
  fmt: (v: number) => string;
  /** What has no value for the filter: kept or not. */
  unknown?: { label: string; title?: string; get: () => boolean; set: (v: boolean) => void };
}

/**
 * A range filter: the histogram of what is in view along its axis, with handles for the limits
 * (dragged to an end: no limit there), and a switch. Under the limits' span the bars are bright;
 * what the filter leaves out, dim.
 */
export class RangeFilter {
  readonly el: HTMLDivElement;
  private canvas: HTMLCanvasElement;
  private on: HTMLInputElement;
  private text: HTMLSpanElement;
  private unk: HTMLInputElement | null = null;
  private dist: Dist | null = null;
  private drag: 0 | 1 | null = null;

  constructor(private o: RangeFilterOpts) {
    this.on = h('input', { type: 'checkbox', title: 'On: only what the limits allow' });
    this.on.addEventListener('change', () => o.set({ on: this.on.checked }));
    this.text = h('span', { class: 'rf-text' });
    this.canvas = h('canvas', { class: 'rf-hist' });
    const unk = o.unknown;
    if (unk) {
      this.unk = h('input', { type: 'checkbox' });
      this.unk.addEventListener('change', () => unk.set(this.unk!.checked));
    }
    this.el = h('div', { class: 'rf', title: o.title },
      h('label', { class: 'rf-hd' }, this.on, h('span', { class: 'muted' }, o.label), this.text),
      this.canvas,
      ...(unk && this.unk ? [h('label', { class: 'rf-unk faint', title: unk.title }, this.unk, h('span', {}, unk.label))] : []),
    );
    this.bindDrag();
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  private get ax(): Axis {
    return this.o.axis ?? 'lin';
  }
  /** The domain's ends on the axis. */
  private span(): [number, number] {
    return [axisPos(this.ax, this.o.domain[0]), axisPos(this.ax, this.o.domain[1])];
  }
  /** Position 0..1 of a value along the histogram, and back. */
  private u(v: number): number {
    const [a, b] = this.span();
    return Math.max(0, Math.min(1, (axisPos(this.ax, v) - a) / (b - a)));
  }
  private v(u: number): number {
    const [a, b] = this.span();
    return axisValue(this.ax, a + Math.max(0, Math.min(1, u)) * (b - a));
  }
  /** A handle's value as set: two significant digits on a log axis; years to the year (centuries
   * back: to the decade; millennia: to the century); else to a hundredth of the domain or finer. */
  private nice(v: number): number {
    const r = (x: number, p: number) => +(Math.round(x / p) * p).toPrecision(12);
    if (this.ax === 'log') return r(v, 10 ** (Math.floor(Math.log10(v)) - 1));
    if (this.ax === 'age') return r(v, 10 ** Math.max(0, Math.floor(Math.log10(Math.max(1, 2030 - v))) - 1));
    const [a, b] = this.o.domain;
    return r(v, 10 ** Math.floor(Math.log10((b - a) / 100)));
  }

  private bindDrag() {
    const c = this.canvas;
    const at = (e: PointerEvent) => Math.max(0, Math.min(1, e.offsetX / Math.max(1, c.clientWidth)));
    c.addEventListener('pointerdown', (e) => {
      const f = this.o.get();
      const x = at(e);
      const lo = f.min ? this.u(f.min) : 0, hi = f.max ? this.u(f.max) : 1;
      this.drag = Math.abs(x - lo) <= Math.abs(x - hi) ? 0 : 1;
      c.setPointerCapture(e.pointerId);
      this.move(x);
    });
    c.addEventListener('pointermove', (e) => {
      if (this.drag !== null) this.move(at(e));
    });
    const end = () => (this.drag = null);
    c.addEventListener('pointerup', end);
    c.addEventListener('pointercancel', end);
  }

  private move(x: number) {
    const f = this.o.get();
    const end = this.drag === 0 ? x <= 0.01 : x >= 0.99;
    const v = end ? 0 : this.nice(this.v(x));
    // (0: no limit; limits crossing: the other one goes)
    if (this.drag === 0) this.o.set({ on: true, min: v, max: f.max && v && v >= f.max ? 0 : f.max });
    else this.o.set({ on: true, max: v, min: f.min && v && v <= f.min ? 0 : f.min });
  }

  /** The distribution in view along the axis (values on it: logged by the owner where log). */
  update(dist: Dist | null) {
    this.dist = dist;
    this.draw();
  }

  sync() {
    const f = this.o.get();
    this.on.checked = f.on;
    const lim = (v: number, side: string) => (v ? this.o.fmt(v) : side);
    this.text.textContent = f.on || f.min || f.max ? `${lim(f.min, 'any')} – ${lim(f.max, 'any')} ${this.o.unit}`.trim() : 'no limits';
    this.el.classList.toggle('off', !f.on);
    if (this.unk && this.o.unknown) this.unk.checked = this.o.unknown.get();
    this.draw();
  }

  private draw() {
    const c = this.canvas;
    if (!c.isConnected || c.clientWidth === 0) return;
    const ctx = setupCanvas(c);
    const W = c.clientWidth, H = c.clientHeight;
    ctx.clearRect(0, 0, W, H);
    const f = this.o.get();
    const lo = f.min ? this.u(f.min) : 0, hi = f.max ? this.u(f.max) : 1;
    const N = Math.max(16, Math.floor(W / 4));
    const d = this.dist;
    const HB = H - 6;
    if (d && d.total > 0) {
      // The owner's distribution is over axis positions (stopfilters.ts axisPos).
      const [a, b] = this.span();
      // (no finer than the owner's bins: coarser ones would leave gaps)
      const n = Math.min(N, Math.round(d.bins.length * ((b - a) / (d.hi - d.lo))));
      const bins = d.rebin(a, b, n);
      let max = 0;
      for (const x of bins) max = Math.max(max, x);
      for (let i = 0; i < n; i++) {
        if (!bins[i]) continue;
        const uc = (i + 0.5) / n;
        const hgt = Math.max(1, Math.sqrt(bins[i] / max) * HB);
        const inside = !f.on || (uc >= lo && uc <= hi);
        ctx.fillStyle = inside ? 'rgba(111, 211, 166, 0.85)' : 'rgba(255, 255, 255, 0.18)';
        ctx.fillRect((i / n) * W + 0.5, HB - hgt, Math.max(1, W / n - 1), hgt);
      }
    } else {
      ctx.fillStyle = 'rgba(255, 255, 255, 0.06)';
      ctx.fillRect(0, HB - 1, W, 1);
    }
    // The limits' span along the bottom, and its handles.
    ctx.fillStyle = f.on ? 'rgba(111, 211, 166, 0.5)' : 'rgba(255, 255, 255, 0.15)';
    ctx.fillRect(lo * W, HB + 2, Math.max(1, (hi - lo) * W), 3);
    ctx.fillStyle = f.on ? '#fff' : 'rgba(255, 255, 255, 0.4)';
    for (const u of [lo, hi]) ctx.fillRect(Math.max(0, Math.min(W - 2, u * W - 1)), 0, 2, H);
  }
}
