// Elevation profile panel for a selected road.
import type { Profile } from '../api';
import { paletteRgb } from '../palettes';
import { metricOf, modeDef, type Mode } from '../scenic';
import * as prefs from '../prefs';
import { fmt, h, niceStep, setupCanvas } from './dom';

export interface ProfileColour {
  palette: string;
  mode: Mode;
  range: [number, number];
  weights: number[];
  cdf: Uint8Array | null;
}

/** Secondary series drawn over the elevation profile. */
const SERIES: [Mode | 'none', string][] = [
  ['none', 'No overlay'],
  ['score', 'Scenic score'],
  ['view', 'Views'],
  ['water', 'Water views'],
  ['vista', 'Vista distance'],
  ['openness', 'Unblocked views'],
  ['trees', 'Roadside trees'],
  ['curvy', 'Curviness'],
];

const ZERO = new Array(12).fill(0);

export class ProfilePanel {
  private p: Profile | null = null;
  private canvas: HTMLCanvasElement = h('canvas');
  private hoverX: number | null = null;
  /** A stretch to emphasise (e.g. a climb picked from the list), matched to the profile by position. */
  private hl: { start: [number, number]; end: [number, number]; label: string } | null = null;
  private hlIdx: [number, number] | null = null;
  onHover: (lngLat: [number, number] | null, label?: string) => void = () => {};
  onClose: () => void = () => {};
  onZoom: (p: Profile) => void = () => {};
  onDrive: (p: Profile) => void = () => {};
  colour: () => ProfileColour = () => ({ palette: 'viridis', mode: 'elev', range: [0, 1], weights: [], cdf: null });
  private series: Mode | 'none' = (() => {
    const v = prefs.load<string>('profile.series', 'none');
    return (SERIES.some(([k]) => k === v) ? v : 'none') as Mode | 'none';
  })();

  constructor(private root: HTMLElement) {
    this.canvas.addEventListener('pointermove', (e) => {
      this.hoverX = e.offsetX;
      this.draw();
    });
    this.canvas.addEventListener('pointerleave', () => {
      this.hoverX = null;
      this.draw();
      this.onHover(null);
    });
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  loading(label: string) {
    this.p = null;
    this.root.hidden = false;
    this.root.replaceChildren(
      h('div', { class: 'loading' }, h('span', { class: 'spin' }), `Tracing ${label} and loading its elevation profile…`),
    );
  }

  error(msg: string) {
    this.root.replaceChildren(h('div', { class: 'loading' }, `Could not load profile: ${msg}`), this.buttons(false));
  }

  highlight(hl: { start: [number, number]; end: [number, number]; label: string } | null) {
    this.hl = hl;
    this.matchHighlight();
    if (this.p) this.show(this.p);
  }

  private matchHighlight() {
    const p = this.p;
    this.hlIdx = null;
    if (!p || !this.hl) return;
    const near = (q: [number, number]) => {
      const k = Math.cos((q[1] * Math.PI) / 180);
      let best = 0, bd = Infinity;
      for (let i = 0; i < p.coords.length; i++) {
        const dx = (p.coords[i][0] - q[0]) * k, dy = p.coords[i][1] - q[1];
        const d = dx * dx + dy * dy;
        if (d < bd) { bd = d; best = i; }
      }
      return best;
    };
    const a = near(this.hl.start), b = near(this.hl.end);
    this.hlIdx = a < b ? [a, b] : [b, a];
  }

  hide() {
    this.p = null;
    this.hl = null;
    this.hlIdx = null;
    this.root.hidden = true;
    this.onHover(null);
  }

  show(p: Profile) {
    this.p = p;
    this.root.hidden = false;
    this.matchHighlight();
    const w = p.way;
    const title = h('div', { class: 'name' });
    if (w.ref) title.append(h('span', { class: 'pill', style: 'color:var(--text)' }, w.ref));
    title.append(h('span', {}, w.name || (w.ref ? `Route ${w.ref}` : `Unnamed ${w.class.replace('_', ' ')}`)));
    const chip = (k: string, v: string) => h('span', {}, `${k} `, h('b', {}, v));
    const src = p.sources.map(([s, f]) => `${s.replace(/ \(.*\)/, '')} ${(f * 100).toFixed(0)} %`).join(' · ');
    this.root.replaceChildren(
      h('div', { class: 'hd' }, title, this.buttons(true)),
      h('div', { class: 'chips' },
        chip('Length', fmt.dist(p.length_m)),
        chip('Min', fmt.m(p.elev_min)),
        chip('Max', fmt.m(p.elev_max)),
        chip('Climb ↑', fmt.m(p.climb_m)),
        chip('Descent ↓', fmt.m(p.descent_m)),
        chip('Max grade', fmt.pct(p.max_grade)),
        chip('Avg |grade|', fmt.pct(p.avg_grade)),
        chip('Segments', String(p.ways.length)),
        ...this.scenicChips(p, chip),
        ...(this.hl ? [h('span', { class: 'hl-chip' }, this.hl.label)] : []),
      ),
      this.canvas,
      h('div', { class: 'foot' },
        h('span', {}, `${w.class.replace('_', ' ')}${w.surface ? ' · ' + w.surface : ''} · DEM: ${src}${p.truncated ? ' · truncated at 400 km each way' : ''}`),
        h('span', {}, 'Smoothed ~15 m · bridges & tunnels interpolated'),
      ),
    );
    requestAnimationFrame(() => this.draw());
  }

  private scenicChips(p: Profile, chip: (k: string, v: string) => HTMLElement): HTMLElement[] {
    const n = p.dist.length;
    if (!p.ch?.length || p.ch.length !== n) return [];
    const w = this.colour().weights;
    let L = 0, sc = 0, best = 0, open = 0, water = 0, trees = 0;
    for (let i = 0; i + 1 < n; i++) {
      const dl = p.dist[i + 1] - p.dist[i];
      const c = p.ch[i];
      const s = metricOf('score', 0, 0, c, w);
      L += dl;
      sc += s * dl;
      best = Math.max(best, s);
      if (metricOf('openness', 0, 0, c, w) >= 50) open += dl;
      if (c[1] > 60) water += dl;
      trees += (c[11] / 8) * dl;
    }
    if (L <= 0) return [];
    return [
      chip('Scenic', `${(sc / L).toFixed(0)} avg · ${best.toFixed(0)} best`),
      chip('Open views', `${((open / L) * 100).toFixed(0)} %`),
      chip('Water in view', `${((water / L) * 100).toFixed(0)} %`),
      chip('Roadside trees', `${(trees / L).toFixed(0)} m`),
    ];
  }

  private buttons(full: boolean) {
    const sel = h('select', { class: 'series', title: 'Overlay a scenic metric on the profile' }, ...SERIES.map(([k, l]) => h('option', { value: k, selected: k === this.series }, l)));
    sel.addEventListener('change', () => {
      this.series = sel.value as Mode | 'none';
      prefs.save('profile.series', this.series);
      this.draw();
    });
    const scenic = full && !!this.p?.ch?.length;
    return h('div', { class: 'btns' },
      scenic ? sel : null,
      full ? h('button', { class: 'pill', title: 'Fly along the road in 3D (Esc stops)', onclick: () => this.p && this.onDrive(this.p) }, '▶ Drive') : null,
      full ? h('button', { class: 'pill', title: 'Reverse direction', onclick: () => this.reverse() }, '⇄ Reverse') : null,
      full ? h('button', { class: 'pill', title: 'Zoom to the whole road', onclick: () => this.p && this.onZoom(this.p) }, 'Zoom to') : null,
      h('button', { class: 'pill', title: 'Close (Esc)', onclick: () => this.onClose() }, '✕'),
    );
  }

  private reverse() {
    const p = this.p;
    if (!p) return;
    const L = p.dist[p.dist.length - 1];
    p.coords.reverse();
    p.elev.reverse();
    p.grade.reverse();
    p.ch?.reverse();
    p.dist = p.dist.map((d) => L - d).reverse();
    [p.climb_m, p.descent_m] = [p.descent_m, p.climb_m];
    this.matchHighlight();
    this.show(p);
  }

  redraw() {
    this.draw();
  }

  private draw() {
    const p = this.p;
    if (!p || this.root.hidden) return;
    const c = this.canvas;
    const ctx = setupCanvas(c);
    const W = c.clientWidth, H = c.clientHeight;
    ctx.clearRect(0, 0, W, H);
    const L = { l: 54, r: 12, t: 10, b: 20 };
    const pw = W - L.l - L.r, ph = H - L.t - L.b;
    const n = p.dist.length;
    const total = p.dist[n - 1] || 1;
    let lo = p.elev_min, hi = p.elev_max;
    const pad = Math.max(5, (hi - lo) * 0.08);
    lo -= pad;
    hi += pad;
    const X = (d: number) => L.l + (d / total) * pw;
    const Y = (e: number) => L.t + ph - ((e - lo) / (hi - lo)) * ph;
    const col = this.colour();

    // Grid.
    ctx.font = '10px -apple-system, system-ui, sans-serif';
    ctx.fillStyle = 'rgba(255,255,255,0.35)';
    ctx.strokeStyle = 'rgba(255,255,255,0.06)';
    ctx.lineWidth = 1;
    const ys = niceStep(hi - lo, 4);
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    for (let v = Math.ceil(lo / ys) * ys; v <= hi; v += ys) {
      const y = Math.round(Y(v)) + 0.5;
      ctx.beginPath();
      ctx.moveTo(L.l, y);
      ctx.lineTo(W - L.r, y);
      ctx.stroke();
      ctx.fillText(`${Math.round(v).toLocaleString('en-CA')} m`, L.l - 6, y);
    }
    const xs = niceStep(total / 1000, 8) * 1000;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'top';
    for (let d = 0; d <= total; d += xs) ctx.fillText(fmt.dist(d), X(d), H - L.b + 5);

    // Highlighted stretch.
    if (this.hlIdx) {
      const x0 = X(p.dist[this.hlIdx[0]]), x1 = X(p.dist[this.hlIdx[1]]);
      ctx.fillStyle = 'rgba(255,255,255,0.07)';
      ctx.fillRect(x0, L.t, Math.max(1, x1 - x0), ph);
      ctx.fillStyle = 'rgba(255,255,255,0.45)';
      ctx.fillRect(x0, L.t, 1, ph);
      ctx.fillRect(x1, L.t, 1, ph);
    }

    // Area, coloured per pixel column like the map.
    let j = 0;
    for (let px = 0; px < pw; px++) {
      const d = (px / pw) * total;
      while (j < n - 2 && p.dist[j + 1] < d) j++;
      const t = (d - p.dist[j]) / Math.max(1e-9, p.dist[j + 1] - p.dist[j]);
      const e = p.elev[j] + (p.elev[j + 1] - p.elev[j]) * t;
      const g = p.grade[j] + (p.grade[j + 1] - p.grade[j]) * t;
      const chs = p.ch?.length ? p.ch[t < 0.5 ? j : j + 1] : null;
      const v = chs || col.mode === 'elev' || col.mode === 'grade' || col.mode === 'relief' ? metricOf(col.mode, e, g, chs ?? ZERO, col.weights) : e;
      let u = Math.max(0, Math.min(1, (v - col.range[0]) / (col.range[1] - col.range[0])));
      if (col.cdf) u = col.cdf[Math.min(255, Math.floor(u * 255 + 0.5))] / 255;
      ctx.fillStyle = paletteRgb(col.palette, u);
      ctx.globalAlpha = 0.55;
      const y = Y(e);
      ctx.fillRect(L.l + px, y, 1.2, L.t + ph - y);
    }
    ctx.globalAlpha = 1;
    ctx.strokeStyle = '#f2f5f9';
    ctx.lineWidth = 1.25;
    ctx.beginPath();
    const step = Math.max(1, Math.floor(n / (pw * 2)));
    for (let i = 0; i < n; i += step) {
      const x = X(p.dist[i]), y = Y(p.elev[i]);
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.lineTo(X(p.dist[n - 1]), Y(p.elev[n - 1]));
    ctx.stroke();

    // Scenic overlay series on its own 0–max axis (right edge).
    let sv: ((i: number) => number) | null = null;
    let sdef = null as ReturnType<typeof modeDef> | null;
    if (this.series !== 'none' && p.ch?.length === n) {
      const m = this.series;
      sdef = modeDef(m);
      const [a, b] = sdef.domain[0] < 0 ? sdef.domain : [0, m === 'score' ? 100 : sdef.range[1]];
      sv = (i: number) => (metricOf(m, p.elev[i], p.grade[i], p.ch[i], col.weights) - a) / (b - a);
      ctx.strokeStyle = 'rgba(255,196,92,0.9)';
      ctx.lineWidth = 1.2;
      ctx.beginPath();
      for (let i = 0; i < n; i += step) {
        const x = X(p.dist[i]), y = L.t + ph - Math.max(0, Math.min(1, sv(i))) * ph;
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.stroke();
      ctx.fillStyle = 'rgba(255,196,92,0.75)';
      ctx.textAlign = 'right';
      ctx.textBaseline = 'top';
      ctx.font = '10px -apple-system, system-ui, sans-serif';
      ctx.fillText(`${sdef.label} (${sdef.fmt(b)} top)`, W - L.r - 2, L.t + 2);
    }

    // Hover readout.
    if (this.hoverX !== null && this.hoverX >= L.l && this.hoverX <= W - L.r) {
      const d = ((this.hoverX - L.l) / pw) * total;
      let k = 0;
      let a = 0, b = n - 1;
      while (b - a > 1) {
        const m = (a + b) >> 1;
        if (p.dist[m] <= d) a = m;
        else b = m;
      }
      k = a;
      const t = (d - p.dist[k]) / Math.max(1e-9, p.dist[k + 1] - p.dist[k]);
      const e = p.elev[k] + (p.elev[k + 1] - p.elev[k]) * t;
      const g = p.grade[k] + (p.grade[k + 1] - p.grade[k]) * t;
      const x = X(d), y = Y(e);
      ctx.strokeStyle = 'rgba(255,255,255,0.5)';
      ctx.beginPath();
      ctx.moveTo(x + 0.5, L.t);
      ctx.lineTo(x + 0.5, L.t + ph);
      ctx.stroke();
      ctx.fillStyle = '#fff';
      ctx.beginPath();
      ctx.arc(x, y, 3.5, 0, Math.PI * 2);
      ctx.fill();
      let label = `${fmt.dist(d)} · ${fmt.m1(e)} · ${fmt.pct(g)}`;
      if (sdef && this.series !== 'none' && p.ch?.length === n)
        label += ` · ${sdef.short} ${sdef.fmt(metricOf(this.series, e, g, p.ch[t < 0.5 ? k : k + 1], col.weights))}`;
      ctx.font = '600 11px -apple-system, system-ui, sans-serif';
      const tw = ctx.measureText(label).width + 12;
      const bx = Math.min(W - L.r - tw, Math.max(L.l, x + 8));
      ctx.fillStyle = 'rgba(11,14,19,0.9)';
      ctx.fillRect(bx, L.t, tw, 18);
      ctx.fillStyle = '#fff';
      ctx.textAlign = 'left';
      ctx.textBaseline = 'middle';
      ctx.fillText(label, bx + 6, L.t + 9);
      const lng = p.coords[k][0] + (p.coords[k + 1][0] - p.coords[k][0]) * t;
      const lat = p.coords[k][1] + (p.coords[k + 1][1] - p.coords[k][1]) * t;
      this.onHover([lng, lat], fmt.m(e));
    }
  }
}
