import { GROUPS } from '../config';
import { HERITAGE_COLORS, HERITAGE_LEVELS, POI_STYLE } from '../basemap';
import type { ViewStats } from '../roads/stats';
import { OVERLAYS, WEIGHT_MAX, WEIGHT_MIN, defaults, type AppState, type HillshadeMethod, type OverlayKey, type Store, type TintRange, type TintVar } from '../state';
import { TINT_PALETTES, TINT_VARS } from '../terrain';
import * as prefs from '../prefs';
import { fmt, h } from './dom';
import { RampSelect } from './rampselect';

const SW = [2.6, 2, 1.4, 1, 1.4];
const METHODS: [HillshadeMethod, string][] = [
  ['combined', 'Combined'],
  ['standard', 'Standard'],
  ['igor', 'Igor (soft)'],
  ['multidirectional', 'Multi-directional'],
  ['basic', 'Basic'],
];
const OVERLAY_SWATCH: Partial<Record<OverlayKey, string>> = {
  parks: '#4f9a6b',
  heritage: HERITAGE_COLORS[1],
  heritageAreas: '#e7a0ff',
  special: '#6fe0cc',
  indigenous: '#d99a5e',
  ...Object.fromEntries(Object.entries(POI_STYLE).map(([k, [, c]]) => [k, c])),
};

function loadCollapsed(): Record<string, boolean> {
  return prefs.load<Record<string, boolean>>('layers.collapsed', {});
}

export class LayersCard {
  private roads: HTMLInputElement;
  private groupBoxes: HTMLInputElement[] = [];
  private groupKm: HTMLSpanElement[] = [];
  private other: Record<'water' | 'boundaries' | 'places', HTMLInputElement>;
  private surf: { paved: HTMLInputElement; unpaved: HTMLInputElement };
  private surfKm: [HTMLSpanElement, HTMLSpanElement];
  private weight: HTMLInputElement;
  private weightOut: HTMLOutputElement;
  private glow: HTMLInputElement;
  private persp: HTMLInputElement;
  private blend: HTMLInputElement;
  private t: {
    on: HTMLInputElement; ex: HTMLInputElement; exOut: HTMLOutputElement; hs: HTMLInputElement; method: HTMLSelectElement;
    light: HTMLInputElement; lightOut: HTMLOutputElement; shade: HTMLInputElement; shadeOut: HTMLOutputElement;
    tint: HTMLInputElement; contours: HTMLInputElement; sky: HTMLInputElement; follow: HTMLInputElement;
    tintBox: HTMLDivElement; tintVar: HTMLSelectElement; tintPal: RampSelect; tintRange: HTMLSelectElement; customBox: HTMLDivElement;
    tintMin: HTMLInputElement; tintMinOut: HTMLOutputElement; tintMax: HTMLInputElement; tintMaxOut: HTMLOutputElement;
    tintBands: HTMLSelectElement; tintCurve: HTMLInputElement; tintCurveOut: HTMLOutputElement;
    tintOp: HTMLInputElement; tintOpOut: HTMLOutputElement;
    tintFade: HTMLInputElement; tintFadeOut: HTMLOutputElement; tintSpan: HTMLInputElement; tintSpanOut: HTMLOutputElement; tintLegend: HTMLDivElement; tintLo: HTMLSpanElement; tintHi: HTMLSpanElement;
  };
  private globe: HTMLInputElement;
  private labelOp: HTMLInputElement;
  private labelOpOut: HTMLOutputElement;
  private ov: Partial<Record<OverlayKey, HTMLInputElement>> = {};
  private ovState: Partial<Record<OverlayKey, HTMLSpanElement>> = {};
  private levels: HTMLInputElement[] = [];
  private viewshedBtn: HTMLButtonElement;
  private collapsed = loadCollapsed();
  /** Elevation span the tint currently uses (for seeding a custom range). */
  private tintNow: [number, number] | null = null;
  onViewshed: () => void = () => {};
  /** Tint ramp gradient for a palette key under the current settings (set by the app). */
  tintCssFor: (key: string) => string = () => 'transparent';
  /** Live preview of a tint palette on the map (null: back to the chosen one). */
  onTintPreview: (key: string | null) => void = () => {};

  constructor(private root: HTMLElement, private store: Store) {
    const cb = (on: (v: boolean) => void) => {
      const e = h('input', { type: 'checkbox' });
      e.addEventListener('change', () => on(e.checked));
      return e;
    };
    const tog = (input: HTMLInputElement, label: string | Node, right: Node | string = '', cls = 'tog', title?: string) =>
      h('label', { class: cls, title }, input, typeof label === 'string' ? h('span', {}, label) : label, typeof right === 'string' ? h('span', { class: 'km' }, right) : right);
    const slider = (min: number, max: number, step: number, on: (v: number) => void, reset?: number) => {
      const e = h('input', { type: 'range', min, max, step });
      e.addEventListener('input', () => on(Number(e.value)));
      if (reset !== undefined) e.addEventListener('dblclick', () => on(reset));
      return e;
    };

    // ---- roads ----
    this.roads = cb((v) => this.setLayer('roads', v));
    const groups = h('div', { class: 'grp' });
    GROUPS.forEach((g, i) => {
      const c = cb((v) => {
        const next = [...this.store.s.groups];
        next[i] = v;
        this.store.set({ groups: next });
      });
      const sw = h('span', { class: 'swatch' });
      sw.style.borderTopWidth = `${SW[i]}px`;
      if (g.key === 'ferry') sw.style.borderTopStyle = 'dashed';
      const km = h('span', { class: 'km' });
      this.groupBoxes.push(c);
      this.groupKm.push(km);
      groups.append(tog(c, g.label, km, 'tog sub'));
    });
    const mk = (key: 'water' | 'boundaries' | 'places') => cb((v) => this.setLayer(key, v));
    this.other = { water: mk('water'), boundaries: mk('boundaries'), places: mk('places') };
    const sb = (k: 'paved' | 'unpaved') => cb((v) => this.store.set({ surface: { ...this.store.s.surface, [k]: v } }));
    this.surf = { paved: sb('paved'), unpaved: sb('unpaved') };
    this.surfKm = [h('span', { class: 'km' }), h('span', { class: 'km' })];
    this.weight = slider(WEIGHT_MIN, WEIGHT_MAX, 0.05, (v) => this.store.set({ weight: v }), defaults.weight);
    this.weightOut = h('output');
    this.glow = cb((v) => this.store.set({ routeGlow: v }));
    this.persp = cb((v) => this.store.set({ perspective: v }));
    this.blend = cb((v) => this.store.set({ blendOverlaps: v }));

    // ---- terrain ----
    const T = (patch: Partial<AppState['terrain']>) => this.store.terrain(patch);
    const method = h('select');
    for (const [k, l] of METHODS) method.append(h('option', { value: k }, l));
    method.addEventListener('change', () => T({ method: method.value as HillshadeMethod }));
    const sel = (opts: [string | number, string][], on: (v: string) => void) => {
      const e = h('select');
      for (const [k, l] of opts) e.append(h('option', { value: k }, l));
      e.addEventListener('change', () => on(e.value));
      return e;
    };
    const tintPal = new RampSelect(
      TINT_PALETTES.map((p) => ({ key: p.key, label: p.label })),
      (key) => this.tintCssFor(key),
      (key) => T({ tintPalette: key }),
      (key) => this.onTintPreview(key),
    );
    const tintVar = sel([['elev', 'Elevation'], ['slope', 'Terrain slope']], (v) => {
      const tv = v as TintVar;
      const t = this.store.s.terrain;
      const d = TINT_VARS[tv];
      // Switch to the variable's defaults where the old setting doesn't carry over.
      T({
        tintVar: tv,
        tintMin: d.custom[0],
        tintMax: d.custom[1],
        tintBands: d.bands.includes(t.tintBands) ? t.tintBands : 0,
        tintRange: tv === 'slope' && t.tintRange === 'view' ? 'region' : t.tintRange,
        tintPalette: tv === 'slope' && t.tintPalette === 'atlas' ? 'steep' : tv === 'elev' && t.tintPalette === 'steep' ? 'atlas' : t.tintPalette,
      });
    });
    const tintRange = sel([], (v) => {
      const patch: Partial<AppState['terrain']> = { tintRange: v as TintRange };
      // Start a custom range from what is on screen now.
      const step = TINT_VARS[this.store.s.terrain.tintVar].limits[2];
      if (v === 'custom' && this.tintNow) Object.assign(patch, { tintMin: Math.round(this.tintNow[0] / step) * step, tintMax: Math.round(this.tintNow[1] / step) * step });
      T(patch);
    });
    const tintBands = sel([], (v) => T({ tintBands: Number(v) }));
    // Emphasis slider in log space: −1 … 1 → curve 0.33 … 3.
    const tintCurve = slider(-1, 1, 0.05, (v) => T({ tintCurve: +(3 ** -v).toFixed(3) }), 0);
    const gap = () => TINT_VARS[this.store.s.terrain.tintVar].limits[2];
    const tintMin = slider(-50, 1950, 10, (v) => T({ tintMin: Math.min(v, this.store.s.terrain.tintMax - gap()) }));
    const tintMax = slider(-50, 1950, 10, (v) => T({ tintMax: Math.max(v, this.store.s.terrain.tintMin + gap()) }));
    this.t = {
      on: cb((v) => T({ on: v })),
      ex: slider(1, 6, 0.25, (v) => T({ exaggeration: v }), defaults.terrain.exaggeration),
      exOut: h('output'),
      hs: cb((v) => T({ hillshade: v })),
      method,
      light: slider(0, 359, 1, (v) => T({ light: v }), defaults.terrain.light),
      lightOut: h('output'),
      shade: slider(0, 1, 0.05, (v) => T({ shade: v }), defaults.terrain.shade),
      shadeOut: h('output'),
      tint: cb((v) => T({ tint: v })),
      contours: cb((v) => T({ contours: v })),
      sky: cb((v) => T({ sky: v })),
      follow: cb((v) => T({ cameraFollow: v })),
      tintBox: h('div'),
      tintVar,
      tintPal,
      tintRange,
      customBox: h('div'),
      tintMin,
      tintMinOut: h('output'),
      tintMax,
      tintMaxOut: h('output'),
      tintBands,
      tintCurve,
      tintCurveOut: h('output'),
      tintOp: slider(0, 1, 0.05, (v) => T({ tintOpacity: v }), defaults.terrain.tintOpacity),
      tintFade: slider(0, 1, 0.05, (v) => {
        const t = this.store.s.terrain;
        T({ tintFade: { ...t.tintFade, [t.tintVar]: v } });
      }),
      tintFadeOut: h('output'),
      tintSpan: slider(0.05, 1, 0.05, (v) => {
        const t = this.store.s.terrain;
        T({ tintFadeSpan: { ...t.tintFadeSpan, [t.tintVar]: v } });
      }),
      tintSpanOut: h('output'),
      tintOpOut: h('output'),
      tintLegend: h('div', { class: 'tint-bar' }),
      tintLo: h('span'),
      tintHi: h('span'),
    };
    this.labelOp = slider(0, 1, 0.05, (v) => this.store.set({ labelOpacity: v }), defaults.labelOpacity);
    this.globe = cb((v) => this.store.set({ globe: v }));
    this.labelOpOut = h('output');
    const row = (label: string, input: HTMLElement, out?: HTMLElement) => h('div', { class: 'row' }, h('span', { class: 'muted' }, label), input, out ?? h('span'));

    this.t.customBox.append(row('Min', this.t.tintMin, this.t.tintMinOut), row('Max', this.t.tintMax, this.t.tintMaxOut));
    this.t.tintBox.append(
      h('div', { class: 'tint-legend' }, this.t.tintLegend, h('div', { class: 'tint-ticks' }, this.t.tintLo, this.t.tintHi)),
      row('Colour by', this.t.tintVar),
      row('Colours', this.t.tintPal.el),
      row('Range', this.t.tintRange),
      this.t.customBox,
      row('Bands', this.t.tintBands),
      row('Emphasis', this.t.tintCurve, this.t.tintCurveOut),
      row('Opacity', this.t.tintOp, this.t.tintOpOut),
      row('Fade low end', this.t.tintFade, this.t.tintFadeOut),
      row('Fade span', this.t.tintSpan, this.t.tintSpanOut),
    );
    this.t.tintFade.title = 'Transparency at the bottom of the ramp: 100 % makes flat ground (or the lowest elevations) fully transparent';
    this.t.tintCurve.title = 'Left: more colour steps in the lowlands · right: more in the highlands (double-click resets)';

    // ---- overlays ----
    const ovToggle = (k: OverlayKey, label: string) => {
      const c = cb((v) => this.store.overlay(k, v));
      this.ov[k] = c;
      const sw = h('span', { class: 'dot' });
      sw.style.background = OVERLAY_SWATCH[k] ?? '#888';
      const state = h('span', { class: 'km' });
      this.ovState[k] = state;
      return tog(c, h('span', { class: 'lbl' }, sw, label), state);
    };
    const levels = h('div', { class: 'levels' });
    HERITAGE_LEVELS.forEach((l, i) => {
      const c = cb((v) => {
        const next = [...this.store.s.heritageLevels];
        next[i] = v;
        this.store.set({ heritageLevels: next });
      });
      this.levels.push(c);
      const dot = h('span', { class: 'dot' });
      dot.style.background = HERITAGE_COLORS[i];
      levels.append(h('label', { class: 'lv' }, c, dot, l));
    });
    const byGroup = (g: string) => OVERLAYS.filter((o) => o[2] === g).map(([k, l]) => ovToggle(k, l));

    this.viewshedBtn = h('button', { class: 'pill wide', title: 'Click a spot on the map to see everything visible from there (trees and terrain block the view)', onclick: () => this.onViewshed() }, 'What can I see from here?');

    const collapse = h('button', {
      class: 'collapse', title: 'Collapse',
      onclick: () => prefs.save('layers.min', root.classList.toggle('min')),
    }, '▾');
    root.classList.toggle('min', prefs.load('layers.min', false));
    root.append(
      h('div', { class: 'hd' }, h('h2', {}, 'Layers'), collapse),
      h('div', { class: 'bd scroll' },
        this.section('roads', 'Roads',
          tog(this.roads, 'Roads', h('span', { class: 'km faint' }, 'km in view')),
          groups,
          tog(this.surf.paved, 'Paved', this.surfKm[0], 'tog sub'),
          tog(this.surf.unpaved, 'Unpaved (dashed)', this.surfKm[1], 'tog sub'),
          h('div', { class: 'row sub' }, h('span', { class: 'muted' }, 'Line weight'), this.weight, this.weightOut),
          tog(this.glow, h('span', { class: 'lbl' }, h('span', { class: 'dot', style: 'background:#f5bd4d' }), 'Scenic-route glow'), '', 'tog', 'Gold halo on designated scenic byways and routes touristiques'),
          tog(this.persp, 'Perspective line widths', '', 'tog', 'In tilted views, distant roads get thinner like the ground they are on'),
          tog(this.blend, 'Blend overlapping roads', '', 'tog',
            'Off: each pixel shows one road, so junctions and dense areas are no brighter than a single road. On: overlapping translucent roads add up (density glow).'),
        ),
        this.section('terrain', 'Terrain',
          tog(this.t.on, '3D terrain', '', 'tog', 'Terrain mesh; tilt with ⌥ Option + two-finger drag, right-drag or the buttons'),
          row('Height ×', this.t.ex, this.t.exOut),
          tog(this.t.hs, 'Hill-shading'),
          row('Method', this.t.method),
          row('Light from', this.t.light, this.t.lightOut),
          row('Strength', this.t.shade, this.t.shadeOut),
          tog(this.t.tint, 'Elevation tint', '', 'tog', 'Hypsometric colour of the terrain surface'),
          this.t.tintBox,
          tog(this.t.contours, 'Contour lines', '', 'tog', 'Computed on the fly from the terrain tiles'),
          tog(this.t.sky, 'Sky & distance fog', '', 'tog', 'Visible when the map is tilted'),
          tog(this.t.follow, 'Camera follows terrain height', '', 'tog',
            'Off: the camera never rises or sinks with the ground under the view centre (⊥ levels it on demand). On: MapLibre default.'),
        ),
        this.section('map', 'Map',
          tog(this.globe, 'Globe', '', 'tog', 'Globe projection; flattens to Web Mercator as you zoom in'),
          tog(this.other.water, 'Water'),
          tog(this.other.boundaries, 'Boundaries'),
          tog(this.other.places, 'Place labels'),
          row('Label opacity', this.labelOp, this.labelOpOut),
          ...byGroup('map'),
        ),
        this.section('designations', 'Designations',
          ...byGroup('designations').slice(0, 1),
          levels,
          ...byGroup('designations').slice(1),
        ),
        this.section('stops', 'Stops & sights', ...byGroup('stops')),
        this.section('tools', 'Tools', this.viewshedBtn),
        h('div', { class: 'faint note' }, 'Tunnels faded · bridges cased · zoomed out, brightness = road density'),
      ),
    );
    this.sync(store.s);
  }

  private section(key: string, title: string, ...kids: Node[]) {
    const body = h('div', { class: 'grp' }, ...kids);
    const head = h('button', { class: 'sec' }, h('span', {}, title), h('i', {}, '▾'));
    const wrap = h('div', { class: 'section' }, head, body);
    wrap.classList.toggle('closed', !!this.collapsed[key]);
    head.addEventListener('click', () => {
      wrap.classList.toggle('closed');
      this.collapsed[key] = wrap.classList.contains('closed');
      prefs.save('layers.collapsed', this.collapsed);
    });
    return wrap;
  }

  private setLayer(k: keyof AppState['layers'], on: boolean) {
    this.store.set({ layers: { ...this.store.s.layers, [k]: on } });
  }

  /** Tint legend: gradient and the elevation span it covers. */
  setTintLegend(css: string, range: [number, number]) {
    this.tintNow = range;
    this.t.tintLegend.style.background = css;
    const u = (v: number) => (this.store.s.terrain.tintVar === 'slope' ? `${Math.round(v)} % (${Math.round((Math.atan(v / 100) * 180) / Math.PI)}°)` : fmt.m(v));
    this.t.tintLo.textContent = u(range[0]);
    this.t.tintHi.textContent = u(range[1]);
  }

  setViewshedActive(on: boolean) {
    this.viewshedBtn.classList.toggle('on', on);
    this.viewshedBtn.textContent = on ? 'Click the map… (Esc to cancel)' : 'What can I see from here?';
  }

  /** Loading / count status next to an overlay toggle. */
  setOverlayStatus(k: OverlayKey, text: string, loading = false) {
    const el = this.ovState[k];
    if (!el) return;
    el.replaceChildren(loading ? h('span', { class: 'spin' }) : text);
  }

  sync(s: AppState) {
    this.roads.checked = s.layers.roads;
    this.groupBoxes.forEach((c, i) => {
      c.checked = s.groups[i];
      c.disabled = !s.layers.roads;
    });
    this.surf.paved.checked = s.surface.paved;
    this.surf.unpaved.checked = s.surface.unpaved;
    this.surf.paved.disabled = this.surf.unpaved.disabled = this.weight.disabled = !s.layers.roads;
    this.weight.value = String(s.weight);
    this.weightOut.value = `${s.weight.toFixed(2)}×`;
    this.glow.checked = s.routeGlow;
    this.persp.checked = s.perspective;
    this.blend.checked = s.blendOverlaps;
    const t = s.terrain;
    this.t.on.checked = t.on;
    this.t.ex.value = String(t.exaggeration);
    this.t.exOut.value = `${t.exaggeration.toFixed(2).replace(/\.?0+$/, '')}×`;
    this.t.ex.disabled = !t.on;
    this.t.hs.checked = t.hillshade;
    this.t.method.value = t.method;
    this.t.light.value = String(t.light);
    this.t.lightOut.value = `${Math.round(t.light)}° ${compass(t.light)}`;
    this.t.shade.value = String(t.shade);
    this.t.shadeOut.value = t.shade.toFixed(2);
    this.t.method.disabled = this.t.light.disabled = this.t.shade.disabled = !t.hillshade;
    this.t.tint.checked = t.tint;
    this.t.tintBox.hidden = !t.tint;
    const slope = t.tintVar === 'slope';
    const tv = TINT_VARS[t.tintVar];
    this.t.tintVar.value = t.tintVar;
    this.t.tintPal.set(t.tintPalette);
    const opts = (el: HTMLSelectElement, list: [string | number, string][]) => {
      const sig = list.map((o) => o.join(':')).join('|');
      if (el.dataset.sig === sig) return;
      el.dataset.sig = sig;
      el.replaceChildren(...list.map(([k, l]) => h('option', { value: k }, l)));
    };
    opts(this.t.tintRange, slope
      ? [['region', 'Full scale (0–100 %, 45°)'], ['roads', 'Match road grade colours'], ['custom', 'Custom']]
      : [['region', 'Whole region (0–1,900 m)'], ['view', 'Fit to view'], ['roads', 'Match road colours'], ['custom', 'Custom']]);
    opts(this.t.tintBands, [[0, 'Smooth'], ...tv.bands.map((b): [number, string] => [b, `${b} ${tv.unit} bands`])]);
    this.t.tintRange.value = slope && t.tintRange === 'view' ? 'region' : t.tintRange;
    for (const el of [this.t.tintMin, this.t.tintMax]) {
      el.min = String(tv.limits[0]);
      el.max = String(tv.limits[1]);
      el.step = String(tv.limits[2]);
    }
    this.t.customBox.hidden = t.tintRange !== 'custom';
    this.t.tintMin.value = String(t.tintMin);
    this.t.tintMax.value = String(t.tintMax);
    this.t.tintMinOut.value = slope ? `${t.tintMin} %` : fmt.m(t.tintMin);
    this.t.tintMaxOut.value = slope ? `${t.tintMax} %` : fmt.m(t.tintMax);
    this.t.tintBands.value = String(t.tintBands);
    const lc = -Math.log(t.tintCurve) / Math.log(3);
    this.t.tintCurve.value = String(lc);
    this.t.tintCurveOut.value = Math.abs(lc) < 0.05 ? 'even' : lc > 0 ? 'low' : 'high';
    this.t.tintFade.value = String(t.tintFade[t.tintVar]);
    this.t.tintFadeOut.value = t.tintFade[t.tintVar] === 0 ? 'off' : `${Math.round(t.tintFade[t.tintVar] * 100)} %`;
    this.t.tintSpan.value = String(t.tintFadeSpan[t.tintVar]);
    this.t.tintSpanOut.value = `${Math.round(t.tintFadeSpan[t.tintVar] * 100)} %`;
    this.t.tintSpan.disabled = t.tintFade[t.tintVar] === 0;
    this.t.tintOp.value = String(t.tintOpacity);
    this.t.tintOpOut.value = `${Math.round(t.tintOpacity * 100)} %`;
    this.globe.checked = s.globe;
    this.labelOp.value = String(s.labelOpacity);
    this.labelOpOut.value = `${Math.round(s.labelOpacity * 100)} %`;
    this.t.contours.checked = t.contours;
    this.t.sky.checked = t.sky;
    this.t.follow.checked = t.cameraFollow;
    this.other.water.checked = s.layers.water;
    this.other.boundaries.checked = s.layers.boundaries;
    this.other.places.checked = s.layers.places;
    for (const [k] of OVERLAYS) if (this.ov[k]) this.ov[k]!.checked = s.overlays[k];
    this.levels.forEach((c, i) => {
      c.checked = s.heritageLevels[i];
      c.disabled = !s.overlays.heritage;
    });
  }

  update(stats: ViewStats | null) {
    GROUPS.forEach((g, i) => {
      const km = stats ? (g.classes as readonly number[]).reduce((a: number, c: number) => a + stats.classKm[c], 0) : 0;
      this.groupKm[i].textContent = stats && this.store.s.groups[i] ? fmt.km(km) : '';
    });
    this.surfKm.forEach((el, u) => (el.textContent = stats ? fmt.km(stats.surfaceKm[u]) : ''));
  }
}

function compass(deg: number) {
  return ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'][Math.round((((deg % 360) + 360) % 360) / 45) % 8];
}
