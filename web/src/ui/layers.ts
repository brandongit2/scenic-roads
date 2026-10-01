import { GROUPS, RAIL0, RAIL_GROUPS } from '../config';
import { RAIL_GROUP_COLOURS } from '../rail';
import { FERRY_GROUPS, FERRY_GROUP_COLOURS } from '../ferry';
import { HERITAGE_GROUPS, HERITAGE_TIERS, POI_STYLE } from '../basemap';
import { Dist, type ViewStats } from '../roads/stats';
import { STOP_FILTERS, axisPos, filtersOf, type StopFilter } from '../stopfilters';
import { DEFAULT_DENSITY, DENSITY_KINDS, LABEL_KINDS, LINE_KINDS, OVERLAYS, SPACING_RANGE, WEIGHT_RANGE, defaults, type AppState, type LabelDensity, type HillshadeMethod, type LineKind, type OverlayKey, type Store, type TintVar } from '../state';
import { TINT_PALETTES, TINT_VARS } from '../terrain';
import * as prefs from '../prefs';
import { fmt, h } from './dom';
import { ScaleControls } from './scale';
import { RangeFilter, Slider, pct, type SliderOpts } from './controls';
import { TreeSection } from './trees';
import { toggleAllStops } from './stops';

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
  heritage: HERITAGE_GROUPS[1].colour,
  heritageAreas: '#e7a0ff',
  special: '#6fe0cc',
  indigenous: '#d99a5e',
  ...Object.fromEntries(Object.entries(POI_STYLE).map(([k, [, c]]) => [k, c])),
};

function loadCollapsed(): Record<string, boolean> {
  return prefs.load<Record<string, boolean>>('layers.collapsed', {});
}

/** A section's switch dims its body while off (synced on change and by the panel's sync). */
function wrapSwitch(toggle: HTMLInputElement, body: HTMLElement) {
  const apply = () => body.classList.toggle('off', !toggle.checked && !toggle.indeterminate);
  toggle.addEventListener('change', apply);
  (toggle as HTMLInputElement & { syncOff?: () => void }).syncOff = apply;
}

/** Label density changes while a slider is dragged: at most this often (ms). */
const DENSITY_MS = 120;

export class LayersCard {
  private roads: HTMLInputElement;
  private groupBoxes: HTMLInputElement[] = [];
  private groupKm: HTMLSpanElement[] = [];
  private unnamedBoxes: HTMLInputElement[] = [];
  private unnamedKm: HTMLSpanElement[] = [];
  private roadLen: RangeFilter;
  /** Tree cover layer controls. */
  readonly trees: TreeSection;
  private railSection: Node[];
  private rail!: HTMLInputElement;
  private railBoxes: HTMLInputElement[] = [];
  private railKm: HTMLSpanElement[] = [];
  private ferrySection: Node[];
  private railFreq!: FreqFilterRow;
  private ferryFreq!: FreqFilterRow;
  private ferry!: HTMLInputElement;
  private ferryBoxes: HTMLInputElement[] = [];
  private ferryKm: HTMLSpanElement[] = [];
  private other: Record<'water' | 'boundaries' | 'places', HTMLInputElement>;
  private labelBoxes: HTMLInputElement[] = [];
  /** Stops & sights filters: must-haves (a checkbox each) and ranges (a histogram each). */
  private sfFlags: { key: string; on: HTMLInputElement }[] = [];
  private sfRange = new Map<string, RangeFilter>();
  private sfKeep: Partial<Record<OverlayKey, HTMLInputElement>> = {};
  private sfBlocks: Partial<Record<OverlayKey, { hd: HTMLButtonElement; body: HTMLElement; open: boolean }>> = {};
  private surf: { paved: HTMLInputElement; unpaved: HTMLInputElement };
  private surfKm: [HTMLSpanElement, HTMLSpanElement];
  private tollBox: { free: HTMLInputElement; toll: HTMLInputElement };
  private tollKm: [HTMLSpanElement, HTMLSpanElement];
  /** Every slider row (synced from the state). */
  private sliders: Slider[] = [];
  private ferryDashed: HTMLInputElement;
  private glow: HTMLInputElement;
  private boundaryBoxes: HTMLInputElement[] = [];
  private occlude: HTMLInputElement;
  private t: {
    on: HTMLInputElement; hs: HTMLInputElement; method: HTMLSelectElement;
    tint: HTMLInputElement; contours: HTMLInputElement; sky: HTMLInputElement;
    tintBox: HTMLDivElement; tintVar: HTMLSelectElement; tintBands: HTMLSelectElement;
  };
  private globe: HTMLInputElement;
  private ov: Partial<Record<OverlayKey, HTMLInputElement>> = {};
  /** Every stop & sight at once (the kinds that were on come back when it's ticked again). */
  private stopsAll!: HTMLInputElement;
  private ovState: Partial<Record<OverlayKey, HTMLSpanElement>> = {};
  /** Heritage groups and their kinds of designation (checkbox, site count). */
  private hGroups: { key: string; c: HTMLInputElement; n: HTMLSpanElement }[] = [];
  private hTiers: { key: string; c: HTMLInputElement; n: HTMLSpanElement }[] = [];
  private viewshedBtn: HTMLButtonElement;
  private collapsed = loadCollapsed();
  /** The terrain tint's colour scale (the shared histogram component). */
  private tintScale!: ScaleControls;
  /** Set by the app: the tint's colour and opacity at a value on the range in use; whether the
   * road colours can be matched (roads coloured by elevation, or grade for slope). */
  tintColourAt: (v: number) => [string, number] = () => ['rgb(0,0,0)', 0];
  tintMatchAvailable: () => boolean = () => false;
  onViewshed: () => void = () => {};
  /** Tint ramp gradient for a palette key under the current settings (set by the app). */
  tintCssFor: (key: string) => string = () => 'transparent';
  /** Live preview of a tint palette on the map (null: back to the chosen one). */
  onTintPreview: (key: string | null) => void = () => {};
  /** A stop's filters were opened: their histograms are wanted (filtersOpen). */
  onFiltersOpen: () => void = () => {};

  /** `cards`: how each layer is coloured (colour.ts, rail.ts, ferry.ts, stops.ts), shown in its
   * section. */
  constructor(private root: HTMLElement, private store: Store, cards: { roads: HTMLElement; rail: HTMLElement; ferry: HTMLElement; stops: HTMLElement }) {
    const cb = (on: (v: boolean) => void) => {
      const e = h('input', { type: 'checkbox' });
      e.addEventListener('change', () => on(e.checked));
      return e;
    };
    const tog = (input: HTMLInputElement, label: string | Node, right: Node | string = '', cls = 'tog', title?: string) =>
      h('label', { class: cls, title }, input, typeof label === 'string' ? h('span', {}, label) : label, typeof right === 'string' ? h('span', { class: 'km' }, right) : right);
    // A slider row, synced with the rest.
    const sl = (o: SliderOpts) => {
      const x = new Slider(o);
      this.sliders.push(x);
      return x.el;
    };
    const S = () => this.store.s;

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
      // Unnamed roads of this type (no name, no route number), toggled on their own.
      const u = cb((v) => {
        const next = [...this.store.s.unnamed];
        next[i] = v;
        this.store.set({ unnamed: next });
      });
      const ukm = h('span', { class: 'km' });
      this.unnamedBoxes.push(u);
      this.unnamedKm.push(ukm);
      groups.append(tog(u, 'Unnamed', ukm, 'tog sub unnamed', `${g.label} with neither a name nor a route number`));
    });
    const mk = (key: 'water' | 'boundaries' | 'places') => cb((v) => this.setLayer(key, v));
    this.other = { water: mk('water'), boundaries: mk('boundaries'), places: mk('places') };
    const sb = (k: 'paved' | 'unpaved') => cb((v) => this.store.set({ surface: { ...this.store.s.surface, [k]: v } }));
    this.surf = { paved: sb('paved'), unpaved: sb('unpaved') };
    this.surfKm = [h('span', { class: 'km' }), h('span', { class: 'km' })];
    const tb = (k: 'free' | 'toll') => cb((v) => this.store.set({ toll: { ...this.store.s.toll, [k]: v } }));
    this.tollBox = { free: tb('free'), toll: tb('toll') };
    this.tollKm = [h('span', { class: 'km' }), h('span', { class: 'km' })];
    // Whole-road length filter, km (0 = no limit), over the roads in view by length.
    this.roadLen = new RangeFilter({
      label: 'Road length', unit: 'km', domain: [0.05, 5000], axis: 'log',
      title: 'Length of the whole road (every way with the same name or route number, joined end to end). Drag the handles; to an end for no limit.',
      get: () => ({ on: this.store.s.roadLenOn, min: this.store.s.roadLen[0], max: this.store.s.roadLen[1] }),
      set: (p) => this.store.set({
        ...(p.on !== undefined ? { roadLenOn: p.on } : {}),
        ...(p.min !== undefined || p.max !== undefined ? { roadLen: [p.min ?? this.store.s.roadLen[0], p.max ?? this.store.s.roadLen[1]] as [number, number] } : {}),
      }),
      fmt: (v) => (v >= 10 ? fmt.n(Math.round(v)) : String(+v.toPrecision(2))),
    });
    this.trees = new TreeSection(this.store);
    // Passenger rail: the layer and its service groups (styling: top-left panel).
    this.rail = cb((v) => this.store.set({ rail: { ...this.store.s.rail, on: v } }));
    const railGroups = h('div', { class: 'grp' });
    RAIL_GROUPS.forEach((g, i) => {
      const c = cb((v) => {
        const groups = [...this.store.s.rail.groups];
        groups[i] = v;
        this.store.set({ rail: { ...this.store.s.rail, groups } });
      });
      const km = h('span', { class: 'km' });
      this.railBoxes.push(c);
      this.railKm.push(km);
      const sw = h('span', { class: 'dot' });
      sw.style.background = RAIL_GROUP_COLOURS[i];
      railGroups.append(tog(c, h('span', { class: 'lbl' }, sw, g.label), km, 'tog sub'));
    });
    this.railFreq = new FreqFilterRow('Trains a day', 'Trains a day each way on a typical weekday (all services on the track), from operators\u2019 timetables. Drag the handles; to an end for no limit.',
      'Tracks without a timetable', () => this.store.s.rail, (p) => this.store.set({ rail: { ...this.store.s.rail, ...p } }), [0.5, 3000]);
    this.rail.title = 'Tracks used by passenger services (OSM route relations), plus trams, metros, funiculars and heritage lines';
    this.railSection = [railGroups, ...this.railFreq.nodes];
    const railOpRow = sl({
      label: 'Opacity', min: 0.1, max: 1, step: 0.05, reset: defaults.rail.opacity, title: 'Opacity of the rail lines and their stop dots',
      get: () => S().rail.opacity, set: (opacity) => this.store.set({ rail: { ...S().rail, opacity } }), fmt: pct, disabled: () => !S().rail.on,
    });
    // Passenger ferries: the layer and its service groups (styling: top-left panel).
    this.ferry = cb((v) => this.store.set({ ferry: { ...this.store.s.ferry, on: v } }));
    const ferryGroups = h('div', { class: 'grp' });
    FERRY_GROUPS.forEach((g, i) => {
      const c = cb((v) => {
        const groups = [...this.store.s.ferry.groups];
        groups[i] = v;
        this.store.set({ ferry: { ...this.store.s.ferry, groups } });
      });
      const km = h('span', { class: 'km' });
      this.ferryBoxes.push(c);
      this.ferryKm.push(km);
      const sw = h('span', { class: 'dot' });
      sw.style.background = FERRY_GROUP_COLOURS[i];
      ferryGroups.append(tog(c, h('span', { class: 'lbl' }, sw, g.label), km, 'tog sub', g.help));
    });
    this.ferryDashed = cb((v) => this.store.set({ ferry: { ...this.store.s.ferry, dashed: v } }));
    this.ferry.title = 'Passenger ferries, car ferries included (OSM ferry routes). Car ferries are also part of the road network (Roads → Car ferries).';
    const ferryOpRow = sl({
      label: 'Opacity', min: 0.05, max: 1, step: 0.05, reset: defaults.ferry.opacity, title: 'Opacity of the ferry lines and their terminal dots',
      get: () => S().ferry.opacity, set: (opacity) => this.store.set({ ferry: { ...S().ferry, opacity } }), fmt: pct, disabled: () => !S().ferry.on,
    });
    this.ferrySection = [
      ferryGroups,
      ...(this.ferryFreq = new FreqFilterRow('Sailings a day', 'Sailings a day each way, from operators\u2019 timetables (on a stretch used by several lines, added up). Drag the handles; to an end for no limit.',
        'Lines without a timetable', () => this.store.s.ferry, (p) => this.store.set({ ferry: { ...this.store.s.ferry, ...p } }), [0.1, 500])).nodes,
    ];
    const ferryDashedRow = tog(this.ferryDashed, 'Dashed lines', '', 'tog', 'Ferry lines dashed, as on paper maps');
    // Line weights: the global one scales every line on the map; each kind's is relative to it.
    const lwRow = (key: 'global' | LineKind, label: string, help: string) => sl({
      label, min: WEIGHT_RANGE[0], max: WEIGHT_RANGE[1], step: 0.05, reset: 1, title: help, cls: key === 'global' ? 'lw top' : 'lw',
      get: () => S().lineWeights[key], set: (v) => this.store.set({ lineWeights: { ...S().lineWeights, [key]: v } }), fmt: (v) => `${v.toFixed(2)}×`,
    });
    // Each kind's weight in its layer's section; the global one and the map's own lines in Map.
    const lwOf = Object.fromEntries(LINE_KINDS.filter(([k]) => k === 'roads' || k === 'rail' || k === 'ferries').map(([k, , help]) => [k, lwRow(k, 'Line weight', help)])) as unknown as Record<'roads' | 'rail' | 'ferries', HTMLElement>;
    const lineWeights = h('div', { class: 'lw-block' },
      lwRow('global', 'Global line weight', 'Width of every line on the map (contour lines too); each layer\'s weight is relative to it'),
      ...LINE_KINDS.filter(([k]) => k === 'borders' || k === 'rivers' || k === 'outlines').map(([k, label, help]) => lwRow(k, label, help)));
    this.glow = cb((v) => this.store.set({ routeGlow: v }));
    this.occlude = cb((v) => this.store.set({ occlude: v }));

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
    const tintVar = sel([['elev', 'Elevation'], ['slope', 'Terrain slope']], (v) => {
      const tv = v as TintVar;
      const t = this.store.s.terrain;
      // Each variable keeps its own scale; bands carry over where the new variable offers them.
      T({ tintVar: tv, tintBands: TINT_VARS[tv].bands.includes(t.tintBands) ? t.tintBands : 0 });
    });
    // The tint's colour scale: the shared histogram component over the terrain in view.
    const TS = () => this.store.s.terrain;
    this.tintScale = new ScaleControls({
      get: () => TS().tintScales[TS().tintVar],
      set: (patch) => {
        const t = TS();
        // Choosing a range of its own stops following the roads'.
        T({ tintScales: { ...t.tintScales, [t.tintVar]: { ...t.tintScales[t.tintVar], ...patch } }, ...('auto' in patch || 'range' in patch ? { tintMatch: false } : {}) });
      },
      metric: () => {
        const tv = TINT_VARS[TS().tintVar];
        const slope = TS().tintVar === 'slope';
        return { domain: tv.domain, step: tv.step, fmt: slope ? (x: number) => `${Math.round(x)} %` : (x: number) => fmt.m(x), hiPlus: slope };
      },
      noun: 'terrain',
      measure: 'land area',
      fadeDefault: 0.6,
      spanDefault: 0.3,
      follow: {
        label: 'Match roads', title: 'Follow the road colours\' range (roads coloured by elevation, or by grade for slope)', caption: 'Following the road colours\' range',
        available: () => this.tintMatchAvailable(), on: () => TS().tintMatch, set: (on) => T({ tintMatch: on }),
      },
      palettes: { items: TINT_PALETTES.map((p) => ({ key: p.key, label: p.label, group: p.group })), css: (key) => this.tintCssFor(key) },
      colourAt: (v) => this.tintColourAt(v),
      onPreview: (key) => this.onTintPreview(key),
    });
    const tintBands = sel([], (v) => T({ tintBands: Number(v) }));
    this.t = {
      on: cb((v) => T({ on: v })),
      hs: cb((v) => T({ hillshade: v })),
      method,
      tint: cb((v) => T({ tint: v })),
      contours: cb((v) => T({ contours: v })),
      sky: cb((v) => T({ sky: v })),
      tintBox: h('div'),
      tintVar,
      tintBands,
    };
    const noShade = () => !TS().hillshade;
    const exRow = sl({
      label: 'Height ×', min: 1, max: 6, step: 0.25, reset: defaults.terrain.exaggeration, title: 'Vertical exaggeration of the 3D terrain',
      get: () => TS().exaggeration, set: (exaggeration) => T({ exaggeration }), fmt: (v) => `${v.toFixed(2).replace(/\.?0+$/, '')}×`, disabled: () => !TS().on,
    });
    const lightRow = sl({
      label: 'Light from', min: 0, max: 359, step: 1, reset: defaults.terrain.light, title: 'Direction the light comes from',
      get: () => TS().light, set: (light) => T({ light }), fmt: (v) => `${Math.round(v)}° ${compass(v)}`, disabled: noShade,
    });
    const shadeRow = sl({
      label: 'Strength', min: 0, max: 1, step: 0.05, reset: defaults.terrain.shade, title: 'How dark the shading gets',
      get: () => TS().shade, set: (shade) => T({ shade }), fmt: (v) => v.toFixed(2), disabled: noShade,
    });
    // Emphasis in log space: −1 … 1 → curve 3 … 0.33.
    const curveRow = sl({
      label: 'Emphasis', min: -1, max: 1, step: 0.05, reset: 1, title: 'Left: more colour steps in the lowlands · right: more in the highlands',
      scale: { to: (c) => -Math.log(c) / Math.log(3), from: (p) => +(3 ** -p).toFixed(3) },
      get: () => TS().tintCurve, set: (tintCurve) => T({ tintCurve }),
      fmt: (c) => {
        const lc = -Math.log(c) / Math.log(3);
        return Math.abs(lc) < 0.05 ? 'even' : lc > 0 ? 'low' : 'high';
      },
    });
    const tintOpRow = sl({
      label: 'Opacity', min: 0, max: 1, step: 0.05, reset: defaults.terrain.tintOpacity, title: 'Opacity of the tint',
      get: () => TS().tintOpacity, set: (tintOpacity) => T({ tintOpacity }), fmt: pct,
    });
    const labelOpRow = sl({
      label: 'Label opacity', min: 0, max: 1, step: 0.05, reset: defaults.labelOpacity, cls: 'lw', title: 'Opacity of every label on the map',
      get: () => S().labelOpacity, set: (labelOpacity) => this.store.set({ labelOpacity }), fmt: pct,
    });
    const roadOpRow = sl({
      label: 'Opacity', min: 0.1, max: 1, step: 0.05, reset: defaults.roadOpacity, title: 'Opacity of the roads, in every display type',
      get: () => S().roadOpacity, set: (roadOpacity) => this.store.set({ roadOpacity }), fmt: pct,
    });
    const boundOpRow = sl({
      label: 'Opacity', min: 0, max: 1, step: 0.05, reset: defaults.boundaryOpacity, cls: 'sub', title: 'Opacity of the boundary lines',
      get: () => S().boundaryOpacity, set: (boundaryOpacity) => this.store.set({ boundaryOpacity }), fmt: pct, disabled: () => !S().layers.boundaries,
    });
    const poiOpRow = sl({
      label: 'Opacity', min: 0.1, max: 1, step: 0.05, reset: defaults.poiOpacity,
      title: 'Dots, areas and their labels (labels also follow Label opacity). Sizes and fades along the landmark score (Prominence)',
      get: () => S().poiOpacity, set: (poiOpacity) => this.store.set({ poiOpacity }), fmt: pct,
    });
    this.globe = cb((v) => this.store.set({ globe: v }));
    // A new density lays the label tiles (and landmarks' point tiles) out again: while dragging, at
    // most every DENSITY_MS.
    let pending: Partial<LabelDensity> | null = null;
    let timer = 0;
    const flush = () => {
      timer = 0;
      if (pending) this.store.set({ labelDensity: { ...this.store.s.labelDensity, ...pending } });
      pending = null;
    };
    const setDensity = (d: Partial<LabelDensity>) => {
      pending = { ...pending, ...d };
      if (!timer) timer = window.setTimeout(flush, DENSITY_MS);
    };
    const noLabels = () => !S().layers.places;
    const spacingRow = sl({
      label: 'Label spacing', min: Math.log2(SPACING_RANGE[0]), max: Math.log2(SPACING_RANGE[1]), step: 0.01, reset: DEFAULT_DENSITY.px, cls: 'lw',
      title: 'A label shows once the nearest label of its kind that matters more is this far away on screen: wider, fewer labels',
      scale: { to: Math.log2, from: (p) => Math.round(2 ** p) },
      get: () => S().labelDensity.px, set: (px) => setDensity({ px }), fmt: (px) => `${px} px`, disabled: noLabels,
    });
    const densityRows = DENSITY_KINDS.map(([k, label, help]) => sl({
      label, min: -2, max: 2, step: 0.25, reset: 1, cls: 'lw dens', title: `${help}: more or fewer than the spacing gives`,
      scale: { to: Math.log2, from: (p) => 2 ** p },
      get: () => S().labelDensity.kinds[k], set: (f) => setDensity({ kinds: { ...S().labelDensity.kinds, ...pending?.kinds, [k]: f } }),
      fmt: (f) => `×${+f.toFixed(f < 1 ? 2 : 1)}`, disabled: noLabels,
    }));
    const horizonRow = sl({
      label: 'Toward horizon', min: 0, max: 1, step: 0.05, reset: DEFAULT_DENSITY.horizon, cls: 'lw',
      title: 'In a tilted view, labels thin out with distance beyond the centre, where the ground is foreshortened',
      get: () => S().labelDensity.horizon, set: (horizon) => setDensity({ horizon }), fmt: pct, disabled: noLabels,
    });
    const row = (label: string, input: HTMLElement, out?: HTMLElement) => h('div', { class: 'row' }, h('span', { class: 'muted' }, label), input, out ?? h('span'));

    this.t.tintBox.append(
      row('Colour by', this.t.tintVar),
      h('div', { class: 'tint-scale' }, this.tintScale.legend, this.tintScale.palRow, this.tintScale.fadeRow, this.tintScale.thrRow),
      row('Bands', this.t.tintBands),
      curveRow,
      tintOpRow,
    );

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
    // Heritage groups, each with its kinds of designation folded beneath (open while any is off).
    const levels = h('div', { class: 'levels' });
    const setOff = (keys: string[], on: boolean) => {
      const off = this.store.s.heritageOff.filter((k) => !keys.includes(k));
      this.store.set({ heritageOff: on ? off : [...off, ...keys] });
    };
    for (const g of HERITAGE_GROUPS) {
      const keys = HERITAGE_TIERS.filter((t) => t.key[0] === g.key).map((t) => t.key);
      const gc = cb((v) => setOff(keys, v));
      const gn = h('span', { class: 'km' });
      this.hGroups.push({ key: g.key, c: gc, n: gn });
      const dot = h('span', { class: 'dot' });
      dot.style.background = g.colour;
      const kids = h('div', { class: 'lv-kids' });
      kids.hidden = !keys.some((k) => this.store.s.heritageOff.includes(k));
      const caret = h('button', { class: 'lv-caret', title: 'Kinds of designation' }, '▸');
      caret.classList.toggle('open', !kids.hidden);
      caret.addEventListener('click', () => {
        kids.hidden = !kids.hidden;
        caret.classList.toggle('open', !kids.hidden);
      });
      for (const t of HERITAGE_TIERS.filter((x) => x.key[0] === g.key)) {
        const c = cb((v) => setOff([t.key], v));
        const n = h('span', { class: 'km' });
        this.hTiers.push({ key: t.key, c, n });
        kids.append(h('label', { class: 'lv kid', title: t.help }, c, h('span', {}, t.label), n));
      }
      levels.append(h('div', { class: 'lv-row' }, caret, h('label', { class: 'lv' }, gc, dot, h('span', {}, g.label), gn)), kids);
    }
    // Filters under a stop or designation: min–max ranges, must-haves, and whether ones without
    // the data stay; folded away until opened (open while any is on).
    const filterBlock = (k: OverlayKey): HTMLElement[] => {
      const defs = filtersOf(k);
      if (!defs.length) return [];
      const cur = (key: string): StopFilter => S().stopFilters[key] ?? { on: false, min: 0, max: 0 };
      const setF = (key: string, patch: Partial<StopFilter>) => this.store.set({ stopFilters: { ...S().stopFilters, [key]: { ...cur(key), ...patch } } });
      const rows: HTMLElement[] = [];
      for (const d of defs) {
        if (d.type === 'flag') {
          const on = cb((v) => setF(d.key, { on: v }));
          rows.push(tog(on, d.label, '', 'tog sub2', d.help ?? `Only ones with ${d.label.toLowerCase()}`));
          this.sfFlags.push({ key: d.key, on });
          continue;
        }
        // A histogram of the ones in view along the filter's axis, the limits as its handles.
        const year = d.unit === 'year';
        const rf = new RangeFilter({
          label: d.label, unit: year ? '' : d.unit ?? '', domain: d.domain!, axis: d.axis,
          title: `${d.help ? d.help + '. ' : ''}The bars: the ones in view. Drag the handles; to an end for no limit.`,
          get: () => cur(d.key), set: (patch) => setF(d.key, patch),
          fmt: year ? (y) => (y < 0 ? `${-y} BC` : String(y)) : (v) => (v >= 10 ? fmt.n(Math.round(v)) : String(+v.toPrecision(2))),
        });
        this.sfRange.set(d.key, rf);
        rows.push(rf.el);
      }
      if (defs.some((d) => d.type === 'range')) {
        const keep = cb((v) => this.store.set({ stopUnknown: { ...S().stopUnknown, [k]: v } }));
        this.sfKeep[k] = keep;
        rows.push(tog(keep, 'Keep ones without data', '', 'tog sub2', 'With a range filter on: keep the ones that have no value for it (off: hide them)'));
      }
      const body = h('div', { class: 'stop-filters' }, ...rows);
      const blk = { hd: h('button', { class: 'filt-hd', type: 'button' }), body, open: false };
      blk.hd.addEventListener('click', () => {
        blk.open = !blk.open;
        this.syncFilters(this.store.s);
        if (blk.open) this.onFiltersOpen();
      });
      this.sfBlocks[k] = blk;
      return [blk.hd, body];
    };
    const byGroup = (g: string) => OVERLAYS.filter((o) => o[2] === g).flatMap(([k, l]) => [ovToggle(k, l), ...filterBlock(k)]);
    // Built once: the toggles register themselves (this.ov) for syncing.
    const designations = byGroup('designations');

    // All stops & sights at once (as the top-left panel's Stops & sights toggle).
    this.stopsAll = cb(() => toggleAllStops(this.store));

    this.viewshedBtn = h('button', { class: 'pill wide', title: 'Click a spot on the map to see everything visible from there (trees and terrain block the view)', onclick: () => this.onViewshed() }, 'What can I see from here?');

    const sub = (label: string) => h('div', { class: 'subhd' }, label);
    const boundaryLevels = BOUNDARY_LEVELS.map(([label, help], i) => {
      const c = cb((v) => {
        const next = [...this.store.s.boundaryLevels] as [boolean, boolean, boolean];
        next[i] = v;
        this.store.set({ boundaryLevels: next });
      });
      this.boundaryBoxes.push(c);
      return tog(c, label, '', 'tog sub', help);
    });
    const labelKinds = LABEL_KINDS.map(([k, label, help]) => {
      const c = cb((v) => this.store.set({ labelKinds: { ...this.store.s.labelKinds, [k]: v } }));
      this.labelBoxes.push(c);
      return tog(c, label, '', 'tog sub', help);
    });
    this.roads.title = 'Roads';
    this.stopsAll.title = 'Every kind at once. Off hides them all; on brings back the kinds you had on';
    this.other.places.title = 'Names of places, water, and of the parks, sites and stops shown';
    root.append(
      // One section per layer: its switch in the header, then how it is coloured, what it shows
      // and how it is drawn.
      this.section('roads', 'Roads', this.roads,
        sub('Colour'), cards.roads,
        sub('Show'),
        h('div', { class: 'cols-hd faint' }, h('span', {}, 'Road types'), h('span', {}, 'km in view')),
        groups,
        tog(this.surf.paved, 'Paved', this.surfKm[0], 'tog sub'),
        tog(this.surf.unpaved, 'Unpaved (dashed)', this.surfKm[1], 'tog sub'),
        tog(this.tollBox.free, 'Toll-free', this.tollKm[0], 'tog sub', 'Roads without a toll'),
        tog(this.tollBox.toll, 'Toll roads', this.tollKm[1], 'tog sub', 'Roads tagged as tolled in OpenStreetMap (toll=yes), including toll bridges and tunnels'),
        this.roadLen.el,
        sub('Style'),
        roadOpRow,
        lwOf.roads,
        tog(this.glow, h('span', { class: 'lbl' }, h('span', { class: 'dot', style: 'background:#f5bd4d' }), 'Scenic-route glow'), '', 'tog', 'Gold halo on designated scenic byways and routes touristiques'),
        tog(this.occlude, 'Hide roads behind terrain', '', 'tog',
          'With 3D terrain. Off: roads behind hills are drawn faint, as if seen through them. On: they are hidden.'),
      ),
      this.section('rail', 'Passenger rail', this.rail,
        sub('Colour'), cards.rail,
        sub('Show'), ...this.railSection,
        sub('Style'), railOpRow, lwOf.rail,
      ),
      this.section('ferry', 'Ferries', this.ferry,
        sub('Colour'), cards.ferry,
        sub('Show'), ...this.ferrySection,
        sub('Style'), ferryOpRow, lwOf.ferries, ferryDashedRow,
      ),
      this.section('stops', 'Stops & sights', this.stopsAll,
        sub('Prominence'), cards.stops,
        sub('Show'),
        ...byGroup('map'),
        ...designations.slice(0, 1),
        levels,
        ...designations.slice(1),
        ...byGroup('stops'),
        sub('Style'),
        poiOpRow,
      ),
      this.section('terrain', 'Terrain', null,
        tog(this.t.on, '3D terrain', '', 'tog', 'Terrain mesh; tilt with ⌥ Option + two-finger drag, right-drag or the buttons'),
        exRow,
        tog(this.t.hs, 'Hill-shading'),
        row('Method', this.t.method),
        lightRow,
        shadeRow,
        tog(this.t.tint, 'Elevation tint', '', 'tog', 'Colour of the terrain surface by elevation or slope'),
        this.t.tintBox,
        tog(this.t.contours, 'Contour lines', '', 'tog', 'Computed on the fly from the terrain tiles'),
        tog(this.t.sky, 'Sky & distance fog', '', 'tog', 'Visible when the map is tilted'),
      ),
      this.section('trees', 'Trees', this.trees.on, ...this.trees.nodes),
      this.section('map', 'Map', null,
        tog(this.globe, 'Globe', '', 'tog', 'Globe projection; flattens to Web Mercator as you zoom in'),
        tog(this.other.water, 'Water'),
        tog(this.other.boundaries, 'Boundaries'),
        ...boundaryLevels,
        boundOpRow,
        sub('Line weights'),
        lineWeights,
      ),
      this.section('labels', 'Labels', this.other.places,
        ...labelKinds,
        labelOpRow,
        sub('Density'),
        spacingRow,
        ...densityRows,
        horizonRow,
      ),
      this.section('tools', 'Tools', null, this.viewshedBtn),
      h('div', { class: 'faint note' }, 'Tunnels faded · bridges cased · zoomed out, brightness = road density'),
    );
    this.sync(store.s);
  }

  /** A collapsible section; `toggle`: its layer's switch, in the header. */
  private section(key: string, title: string, toggle: HTMLInputElement | null, ...kids: Node[]) {
    const body = h('div', { class: 'grp' }, ...kids);
    const name = h('button', { class: 'sec-t', type: 'button' }, h('i', {}, '▾'), h('span', {}, title));
    const head = h('div', { class: 'sec' }, name);
    if (toggle) {
      toggle.classList.add('switch');
      head.append(toggle);
      wrapSwitch(toggle, body);
    }
    const wrap = h('div', { class: 'section' }, head, body);
    wrap.classList.toggle('closed', !!this.collapsed[key]);
    name.addEventListener('click', () => {
      wrap.classList.toggle('closed');
      this.collapsed[key] = wrap.classList.contains('closed');
      prefs.save('layers.collapsed', this.collapsed);
    });
    return wrap;
  }

  private setLayer(k: keyof AppState['layers'], on: boolean) {
    this.store.set({ layers: { ...this.store.s.layers, [k]: on } });
  }

  /** What is in view along the range filters' axes: roads by whole-road length (log10 km), rail
   * and ferry lines by trains and sailings a day (log10). */
  updateFilters(x: { roadLen?: Dist | null; railFreq?: Dist | null; ferryFreq?: Dist | null }) {
    if (x.roadLen !== undefined) this.roadLen.update(x.roadLen);
    if (x.railFreq !== undefined) this.railFreq.update(x.railFreq);
    if (x.ferryFreq !== undefined) this.ferryFreq.update(x.ferryFreq);
  }

  /** Whether an overlay's filters are open (their histograms wanted: onFiltersOpen). */
  filtersOpen(k: OverlayKey): boolean {
    return !!this.sfBlocks[k]?.open && this.store.s.overlays[k];
  }

  /** The stop filters' histograms in view (stopfilters.ts filterHists: bins over axis positions). */
  updateStopFilters(hists: Record<string, { bins: Float64Array; n: number }>) {
    for (const [key, x] of Object.entries(hists)) {
      const d = STOP_FILTERS.find((f) => f.key === key);
      const rf = this.sfRange.get(key);
      if (!d?.domain || !rf) continue;
      const ax = d.axis ?? 'lin';
      rf.update(x.n > 0 ? new Dist(axisPos(ax, d.domain[0]), axisPos(ax, d.domain[1]), x.bins, x.n) : null);
    }
  }

  /** The terrain in view, the tint's range in use and its equalisation lookup (if on). */
  updateTint(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.tintScale.update(dist, range, cdf);
  }

  setViewshedActive(on: boolean) {
    this.viewshedBtn.classList.toggle('on', on);
    this.viewshedBtn.textContent = on ? 'Click the map… (Esc to cancel)' : 'What can I see from here?';
  }

  /** Stops & sights filter controls from the state. */
  private syncFilters(s: AppState) {
    for (const u of this.sfFlags) u.on.checked = !!s.stopFilters[u.key]?.on;
    for (const rf of this.sfRange.values()) rf.sync();
    for (const [k, c] of Object.entries(this.sfKeep)) c!.checked = s.stopUnknown[k as OverlayKey] !== false;
    for (const [k, b] of Object.entries(this.sfBlocks)) {
      const ov = k as OverlayKey;
      const n = filtersOf(ov).filter((d) => s.stopFilters[d.key]?.on).length;
      if (n) b!.open = b!.open || n > 0;
      b!.hd.hidden = !s.overlays[ov];
      b!.hd.textContent = `${b!.open ? '▾' : '▸'} Filters${n ? ` · ${n} on` : ''}`;
      b!.hd.classList.toggle('on', n > 0);
      b!.body.hidden = !s.overlays[ov] || !b!.open;
    }
  }

  /** Sites per kind of heritage designation (all loaded), and per group. */
  setHeritageCounts(counts: Record<string, number>) {
    for (const t of this.hTiers) t.n.textContent = counts[t.key] ? fmt.n(counts[t.key]) : '';
    for (const g of this.hGroups) {
      const n = this.hTiers.filter((t) => t.key[0] === g.key).reduce((a, t) => a + (counts[t.key] ?? 0), 0);
      g.n.textContent = n ? fmt.n(n) : '';
    }
  }

  setOverlayStatus(k: OverlayKey, text: string, loading = false) {
    const el = this.ovState[k];
    if (!el) return;
    el.replaceChildren(loading ? h('span', { class: 'spin' }) : text);
  }

  sync(s: AppState) {
    this.trees.sync();
    queueMicrotask(() => {
      for (const t of this.root.querySelectorAll<HTMLInputElement & { syncOff?: () => void }>('input.switch')) t.syncOff?.();
    });
    this.roads.checked = s.layers.roads;
    this.groupBoxes.forEach((c, i) => {
      c.checked = s.groups[i];
      this.unnamedBoxes[i].checked = s.unnamed[i];
      this.unnamedBoxes[i].disabled = !s.groups[i];
      c.disabled = !s.layers.roads;
    });
    this.surf.paved.checked = s.surface.paved;
    this.surf.unpaved.checked = s.surface.unpaved;
    this.surf.paved.disabled = this.surf.unpaved.disabled = !s.layers.roads;
    this.tollBox.free.checked = s.toll.free;
    this.tollBox.toll.checked = s.toll.toll;
    this.tollBox.free.disabled = this.tollBox.toll.disabled = !s.layers.roads;
    this.roadLen.sync();
    for (const x of this.sliders) x.sync();
    this.ferryDashed.checked = s.ferry.dashed;
    this.ferryDashed.disabled = !s.ferry.on;
    this.glow.checked = s.routeGlow;
    this.occlude.checked = s.occlude;
    this.rail.checked = s.rail.on;
    this.railBoxes.forEach((c, i) => {
      c.checked = s.rail.groups[i];
      c.disabled = !s.rail.on;
    });
    this.railFreq.sync(s.rail.on);
    this.ferryFreq.sync(s.ferry.on);
    this.ferry.checked = s.ferry.on;
    this.ferryBoxes.forEach((c, i) => {
      c.checked = s.ferry.groups[i];
      c.disabled = !s.ferry.on;
    });
    const t = s.terrain;
    this.t.on.checked = t.on;
    this.t.hs.checked = t.hillshade;
    this.t.method.value = t.method;
    this.t.method.disabled = !t.hillshade;
    this.t.tint.checked = t.tint;
    this.t.tintBox.hidden = !t.tint;
    const tv = TINT_VARS[t.tintVar];
    this.t.tintVar.value = t.tintVar;
    const opts = (el: HTMLSelectElement, list: [string | number, string][]) => {
      const sig = list.map((o) => o.join(':')).join('|');
      if (el.dataset.sig === sig) return;
      el.dataset.sig = sig;
      el.replaceChildren(...list.map(([k, l]) => h('option', { value: k }, l)));
    };
    opts(this.t.tintBands, [[0, 'Smooth'], ...tv.bands.map((b): [number, string] => [b, `${b} ${tv.unit} bands`])]);
    this.t.tintBands.value = String(t.tintBands);
    this.tintScale.sync();
    this.globe.checked = s.globe;
    this.t.contours.checked = t.contours;
    this.t.sky.checked = t.sky;
    this.other.water.checked = s.layers.water;
    this.other.boundaries.checked = s.layers.boundaries;
    this.boundaryBoxes.forEach((c, i) => {
      c.checked = s.boundaryLevels[i];
      c.disabled = !s.layers.boundaries;
    });
    this.other.places.checked = s.layers.places;
    this.syncFilters(s);
    this.labelBoxes.forEach((c, i) => {
      c.checked = s.labelKinds[LABEL_KINDS[i][0]] !== false;
      c.disabled = !s.layers.places;
    });
    for (const [k] of OVERLAYS) if (this.ov[k]) this.ov[k]!.checked = s.overlays[k];
    const nOn = OVERLAYS.filter(([k]) => s.overlays[k]).length;
    this.stopsAll.checked = nOn > 0;
    this.stopsAll.indeterminate = nOn > 0 && nOn < OVERLAYS.length;
    for (const t of this.hTiers) {
      t.c.checked = !s.heritageOff.includes(t.key);
      t.c.disabled = !s.overlays.heritage;
    }
    for (const g of this.hGroups) {
      const kids = this.hTiers.filter((t) => t.key[0] === g.key);
      const on = kids.filter((t) => t.c.checked).length;
      g.c.checked = on === kids.length;
      g.c.indeterminate = on > 0 && on < kids.length;
      g.c.disabled = !s.overlays.heritage;
    }
  }

  /** Rail km in view per service group (primary group of each track). */
  updateRail(stats: ViewStats | null) {
    RAIL_GROUPS.forEach((_, i) => {
      this.railKm[i].textContent = stats && this.store.s.rail.on && this.store.s.rail.groups[i] ? fmt.km(stats.classKm[RAIL0 + i]) : '';
    });
  }

  /** Ferry km in view per service group. */
  updateFerry(km: number[] | null) {
    FERRY_GROUPS.forEach((_, i) => {
      this.ferryKm[i].textContent = km && this.store.s.ferry.on && this.store.s.ferry.groups[i] ? fmt.km(km[i]) : '';
    });
  }

  update(stats: ViewStats | null) {
    GROUPS.forEach((g, i) => {
      const km = stats ? (g.classes as readonly number[]).reduce((a: number, c: number) => a + stats.classKm[c], 0) : 0;
      this.groupKm[i].textContent = stats && this.store.s.groups[i] ? fmt.km(km) : '';
      this.unnamedKm[i].textContent = stats && this.store.s.groups[i] ? fmt.km(stats.unnamedKm[i]) : '';
    });
    this.surfKm.forEach((el, u) => (el.textContent = stats ? fmt.km(stats.surfaceKm[u]) : ''));
    this.tollKm.forEach((el, u) => (el.textContent = stats ? fmt.km(stats.tollKm[u]) : ''));
  }
}

function compass(deg: number) {
  return ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'][Math.round((((deg % 360) + 360) % 360) / 45) % 8];
}

/** Boundary levels (AppState.boundaryLevels order): label, help. */
const BOUNDARY_LEVELS: [string, string][] = [
  ['Countries', 'National borders'],
  ['Provinces & states', 'Provinces, states, and the regions / nations of France, Spain, Portugal and the UK'],
  ['Counties & regions', 'Counties, départements, provincias, distritos (zoom 7 and closer)'],
];

/** A frequency filter row (Layers): on/off, min–max a day, and whether lines without a timetable stay. */
interface FreqState {
  freqOn: boolean;
  freqMin: number;
  freqMax: number;
  freqUnknown: boolean;
}

/** A trains or sailings a day filter: the range filter over a log axis (ui/controls.ts), with
 * whether lines no timetable was found for stay. */
class FreqFilterRow {
  readonly nodes: HTMLElement[];
  private rf: RangeFilter;

  constructor(label: string, title: string, unknownLabel: string, private get: () => FreqState, private set: (p: Partial<FreqState>) => void, domain: [number, number]) {
    this.rf = new RangeFilter({
      label, unit: 'a day', title, domain, axis: 'log',
      get: () => ({ on: this.get().freqOn, min: this.get().freqMin, max: this.get().freqMax }),
      set: (p) => this.set({ ...(p.on !== undefined ? { freqOn: p.on } : {}), ...(p.min !== undefined ? { freqMin: p.min } : {}), ...(p.max !== undefined ? { freqMax: p.max } : {}) }),
      fmt: (v) => (v >= 10 ? fmt.n(Math.round(v)) : String(+v.toPrecision(2))),
      unknown: { label: unknownLabel, title: 'Keep showing lines no timetable was found for', get: () => this.get().freqUnknown, set: (v) => this.set({ freqUnknown: v }) },
    });
    this.nodes = [this.rf.el];
  }

  sync(layerOn: boolean) {
    this.rf.sync();
    this.rf.el.classList.toggle('dim', !layerOn);
  }

  /** The lines in view by frequency (log10 of a day's count). */
  update(dist: Dist | null) {
    this.rf.update(dist);
  }
}
