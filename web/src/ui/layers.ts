import { GROUPS, RAIL0, RAIL_GROUPS } from '../config';
import { RAIL_GROUP_COLOURS } from '../rail';
import { FERRY_GROUPS, FERRY_GROUP_COLOURS } from '../ferry';
import { HERITAGE_GROUPS, HERITAGE_TIERS, POI_STYLE } from '../basemap';
import type { ViewStats } from '../roads/stats';
import { filtersOf, type StopFilter } from '../stopfilters';
import { DEFAULT_DENSITY, DENSITY_KINDS, LABEL_KINDS, LINE_KINDS, OVERLAYS, SPACING_RANGE, WEIGHT_RANGE, defaults, type AppState, type DensityKind, type LabelDensity, type HillshadeMethod, type LineKind, type OverlayKey, type Store, type TintVar } from '../state';
import { baseKey, isRev, withRev } from '../palettes';
import { TINT_PALETTES, TINT_VARS } from '../terrain';
import * as prefs from '../prefs';
import { fmt, h } from './dom';
import { RampSelect } from './rampselect';
import { ScaleControls } from './scale';
import type { Dist } from '../roads/stats';
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

/** Label density changes while a slider is dragged: at most this often (ms). */
const DENSITY_MS = 120;

export class LayersCard {
  private roads: HTMLInputElement;
  private groupBoxes: HTMLInputElement[] = [];
  private groupKm: HTMLSpanElement[] = [];
  private unnamedBoxes: HTMLInputElement[] = [];
  private unnamedKm: HTMLSpanElement[] = [];
  private lenIn: [HTMLInputElement, HTMLInputElement];
  private lenOn: HTMLInputElement;
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
  /** Stops & sights filter controls. */
  private sfUi: { key: string; flag: boolean; on: HTMLInputElement; lo?: HTMLInputElement; hi?: HTMLInputElement }[] = [];
  private sfKeep: Partial<Record<OverlayKey, HTMLInputElement>> = {};
  private sfBlocks: Partial<Record<OverlayKey, { hd: HTMLButtonElement; body: HTMLElement; open: boolean }>> = {};
  private surf: { paved: HTMLInputElement; unpaved: HTMLInputElement };
  private surfKm: [HTMLSpanElement, HTMLSpanElement];
  private tollBox: { free: HTMLInputElement; toll: HTMLInputElement };
  private tollKm: [HTMLSpanElement, HTMLSpanElement];
  /** Line weights: the global one, then each kind's. */
  private lw: { key: 'global' | LineKind; input: HTMLInputElement; out: HTMLOutputElement }[] = [];
  private railOp: HTMLInputElement;
  private railOpOut: HTMLOutputElement;
  private ferryOp: HTMLInputElement;
  private ferryOpOut: HTMLOutputElement;
  private ferryDashed: HTMLInputElement;
  private glow: HTMLInputElement;
  private boundaryBoxes: HTMLInputElement[] = [];
  private occlude: HTMLInputElement;
  private t: {
    on: HTMLInputElement; ex: HTMLInputElement; exOut: HTMLOutputElement; hs: HTMLInputElement; method: HTMLSelectElement;
    light: HTMLInputElement; lightOut: HTMLOutputElement; shade: HTMLInputElement; shadeOut: HTMLOutputElement;
    tint: HTMLInputElement; contours: HTMLInputElement; sky: HTMLInputElement;
    tintBox: HTMLDivElement; tintVar: HTMLSelectElement;
    tintBands: HTMLSelectElement; tintCurve: HTMLInputElement; tintCurveOut: HTMLOutputElement;
    tintOp: HTMLInputElement; tintOpOut: HTMLOutputElement;
  };
  private globe: HTMLInputElement;
  private labelOp: HTMLInputElement;
  private labelOpOut: HTMLOutputElement;
  private roadOp: HTMLInputElement;
  private roadOpOut: HTMLOutputElement;
  private boundOp: HTMLInputElement;
  private boundOpOut: HTMLOutputElement;
  private poiOp: HTMLInputElement;
  private poiOpOut: HTMLOutputElement;
  /** Label density: the spacing (log2 px), a factor per kind (log2) and the horizon thinning. */
  private spacing: HTMLInputElement;
  private spacingOut: HTMLOutputElement;
  private densities: { k: DensityKind; input: HTMLInputElement; out: HTMLOutputElement }[] = [];
  private horizon: HTMLInputElement;
  private horizonOut: HTMLOutputElement;
  /** Landmark prominence: the shared scale controls (histogram, fit, fade, highlight) over the
   * landmark score, and the fame ↔ rarity balance. */
  private ov: Partial<Record<OverlayKey, HTMLInputElement>> = {};
  /** Every stop & sight at once (the kinds that were on come back when it's ticked again). */
  private stopsAll!: HTMLInputElement;
  private ovState: Partial<Record<OverlayKey, HTMLSpanElement>> = {};
  /** Heritage groups and their kinds of designation (checkbox, site count). */
  private hGroups: { key: string; c: HTMLInputElement; n: HTMLSpanElement }[] = [];
  private hTiers: { key: string; c: HTMLInputElement; n: HTMLSpanElement }[] = [];
  private viewshedBtn: HTMLButtonElement;
  private collapsed = loadCollapsed();
  /** Elevation span the tint currently uses (for seeding a custom range). */
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
    // Whole-road length filter, km (empty = no limit).
    const lenInput = (i: 0 | 1, placeholder: string) => {
      const e = h('input', { type: 'number', min: 0, step: 'any', placeholder, class: 'num' });
      e.addEventListener('change', () => {
        const v = Math.max(0, Number(e.value) || 0);
        const next: [number, number] = [...this.store.s.roadLen];
        next[i] = v;
        this.store.set({ roadLen: next });
      });
      return e;
    };
    this.lenIn = [lenInput(0, 'min'), lenInput(1, 'max')];
    this.lenOn = cb((v) => this.store.set({ roadLenOn: v }));
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
    this.railFreq = new FreqFilterRow('Trains a day', 'Trains a day each way on a typical weekday (all services on the track), from operators\u2019 timetables. Empty = no limit.',
      'Tracks without a timetable', () => this.store.s.rail, (p) => this.store.set({ rail: { ...this.store.s.rail, ...p } }));
    this.railOp = slider(0.1, 1, 0.05, (v) => this.store.set({ rail: { ...this.store.s.rail, opacity: v } }), defaults.rail.opacity);
    this.railOp.title = 'Opacity of the rail lines and their stop dots (double-click: default)';
    this.railOpOut = h('output');
    this.railSection = [
      tog(this.rail, 'Passenger rail', h('span', { class: 'km faint' }, 'km in view'), 'tog', 'Tracks used by passenger services (OSM route relations), plus trams, metros, funiculars and heritage lines'),
      h('div', { class: 'row', title: 'Opacity of the rail lines and their stop dots' }, h('span', { class: 'muted' }, 'Opacity'), this.railOp, this.railOpOut),
      railGroups,
      ...this.railFreq.nodes,
    ];
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
    this.ferryOp = slider(0.05, 1, 0.05, (v) => this.store.set({ ferry: { ...this.store.s.ferry, opacity: v } }), defaults.ferry.opacity);
    this.ferryOp.title = 'Opacity of the ferry lines and their terminal dots (double-click: default)';
    this.ferryOpOut = h('output');
    this.ferryDashed = cb((v) => this.store.set({ ferry: { ...this.store.s.ferry, dashed: v } }));
    this.ferrySection = [
      tog(this.ferry, 'Ferries', h('span', { class: 'km faint' }, 'km in view'), 'tog',
        'Passenger ferries, car ferries included (OSM ferry routes). Car ferries are also part of the road network (Roads → Car ferries).'),
      h('div', { class: 'row', title: 'Opacity of the ferry lines and their terminal dots' }, h('span', { class: 'muted' }, 'Opacity'), this.ferryOp, this.ferryOpOut),
      ferryGroups,
      ...(this.ferryFreq = new FreqFilterRow('Sailings a day', 'Sailings a day each way, from operators\u2019 timetables (on a stretch used by several lines, added up). Empty = no limit.',
        'Lines without a timetable', () => this.store.s.ferry, (p) => this.store.set({ ferry: { ...this.store.s.ferry, ...p } }))).nodes,
      tog(this.ferryDashed, 'Dashed lines', '', 'tog', 'Ferry lines dashed, as on paper maps'),
    ];
    // Line weights: the global one scales every line on the map; each kind's is relative to it.
    const lwRow = (key: 'global' | LineKind, label: string, help: string) => {
      const input = slider(WEIGHT_RANGE[0], WEIGHT_RANGE[1], 0.05, (v) => this.store.set({ lineWeights: { ...this.store.s.lineWeights, [key]: v } }), 1);
      input.title = 'Double-click: default';
      const out = h('output');
      this.lw.push({ key, input, out });
      return h('div', { class: key === 'global' ? 'row lw top' : 'row lw', title: help }, h('span', key === 'global' ? {} : { class: 'muted' }, label), input, out);
    };
    const lineWeights = h('div', { class: 'lw-block' },
      lwRow('global', 'Global line weight', 'Width of every line on the map (contour lines too); the weights below are relative to it'),
      ...LINE_KINDS.map(([k, label, help]) => lwRow(k, label, help)));
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
    // Emphasis slider in log space: −1 … 1 → curve 0.33 … 3.
    const tintCurve = slider(-1, 1, 0.05, (v) => T({ tintCurve: +(3 ** -v).toFixed(3) }), 0);
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
      tintBox: h('div'),
      tintVar,
      tintBands,
      tintCurve,
      tintCurveOut: h('output'),
      tintOp: slider(0, 1, 0.05, (v) => T({ tintOpacity: v }), defaults.terrain.tintOpacity),
      tintOpOut: h('output'),
    };
    this.labelOp = slider(0, 1, 0.05, (v) => this.store.set({ labelOpacity: v }), defaults.labelOpacity);
    this.roadOp = slider(0.1, 1, 0.05, (v) => this.store.set({ roadOpacity: v }), defaults.roadOpacity);
    this.roadOp.title = 'Opacity of the roads, in every display type (double-click: default)';
    this.roadOpOut = h('output');
    this.boundOp = slider(0, 1, 0.05, (v) => this.store.set({ boundaryOpacity: v }), defaults.boundaryOpacity);
    this.boundOp.title = 'Opacity of the boundary lines (double-click: default)';
    this.boundOpOut = h('output');
    this.globe = cb((v) => this.store.set({ globe: v }));
    this.labelOpOut = h('output');
    this.poiOp = slider(0.1, 1, 0.05, (v) => this.store.set({ poiOpacity: v }), defaults.poiOpacity);
    this.poiOpOut = h('output');
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
    this.spacing = slider(Math.log2(SPACING_RANGE[0]), Math.log2(SPACING_RANGE[1]), 0.01, (v) => setDensity({ px: Math.round(2 ** v) }), Math.log2(DEFAULT_DENSITY.px));
    this.spacing.title = 'A label shows once the nearest label of its kind that matters more is this far away on screen: wider, fewer labels (double-click: default)';
    this.spacingOut = h('output');
    const densityRows = DENSITY_KINDS.map(([k, label, help]) => {
      const input = slider(-2, 2, 0.25, (v) => setDensity({ kinds: { ...this.store.s.labelDensity.kinds, ...pending?.kinds, [k]: 2 ** v } }), 0);
      const out = h('output');
      this.densities.push({ k, input, out });
      return h('div', { class: 'row lw dens', title: `${help}: more or fewer than the spacing gives (double-click: ×1)` }, h('span', { class: 'muted' }, label), input, out);
    });
    this.horizon = slider(0, 1, 0.05, (v) => setDensity({ horizon: v }), DEFAULT_DENSITY.horizon);
    this.horizonOut = h('output');
    const row = (label: string, input: HTMLElement, out?: HTMLElement) => h('div', { class: 'row' }, h('span', { class: 'muted' }, label), input, out ?? h('span'));

    this.t.tintBox.append(
      row('Colour by', this.t.tintVar),
      h('div', { class: 'tint-scale' }, this.tintScale.legend, this.tintScale.palRow, this.tintScale.fadeRow, this.tintScale.thrRow),
      row('Bands', this.t.tintBands),
      row('Emphasis', this.t.tintCurve, this.t.tintCurveOut),
      row('Opacity', this.t.tintOp, this.t.tintOpOut),
    );
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
      const S = () => this.store.s;
      const setF = (key: string, patch: Partial<StopFilter>) => {
        const cur = S().stopFilters[key] ?? { on: false, min: 0, max: 0 };
        this.store.set({ stopFilters: { ...S().stopFilters, [key]: { ...cur, ...patch } } });
      };
      const rows: HTMLElement[] = [];
      for (const d of defs) {
        const on = cb((v) => setF(d.key, { on: v }));
        if (d.type === 'flag') {
          rows.push(tog(on, d.label, '', 'tog sub2', d.help ?? `Only ones with ${d.label.toLowerCase()}`));
          this.sfUi.push({ key: d.key, flag: true, on });
          continue;
        }
        const inp = (side: 'min' | 'max') => {
          const e = h('input', { type: 'number', class: 'num', placeholder: side, step: d.step ?? 'any' });
          e.addEventListener('change', () => setF(d.key, { [side]: Number(e.value) || 0, on: true }));
          return e;
        };
        const lo = inp('min'), hi = inp('max');
        rows.push(h('div', { class: 'row sub len sf', title: `${d.help ? d.help + '. ' : ''}Empty = no limit; untick to switch it off and keep the limits.` },
          h('label', { class: 'lenon' }, on, h('span', { class: 'muted' }, d.label)),
          h('span', { class: 'pair' }, lo, h('span', { class: 'faint' }, '–'), hi), h('span', { class: 'muted' }, d.unit === 'year' ? '' : d.unit ?? '')));
        this.sfUi.push({ key: d.key, flag: false, on, lo, hi });
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

    root.append(
      h('div', { class: 'hd' }, h('h2', {}, 'Layers')),
      h('div', { class: 'bd scroll' },
        this.section('map', 'Map',
          tog(this.globe, 'Globe', '', 'tog', 'Globe projection; flattens to Web Mercator as you zoom in'),
          lineWeights,
          tog(this.other.water, 'Water'),
          tog(this.other.boundaries, 'Boundaries'),
          ...BOUNDARY_LEVELS.map(([label, help], i) => {
            const c = cb((v) => {
              const next = [...this.store.s.boundaryLevels] as [boolean, boolean, boolean];
              next[i] = v;
              this.store.set({ boundaryLevels: next });
            });
            this.boundaryBoxes.push(c);
            return tog(c, label, '', 'tog sub', help);
          }),
          h('div', { class: 'row sub', title: 'Opacity of the boundary lines' }, h('span', { class: 'muted' }, 'Opacity'), this.boundOp, this.boundOpOut),
          tog(this.other.places, 'Place labels', '', 'tog', 'Names of places, water, and of the parks, sites and stops shown'),
          ...LABEL_KINDS.map(([k, label, help]) => {
            const c = cb((v) => this.store.set({ labelKinds: { ...this.store.s.labelKinds, [k]: v } }));
            this.labelBoxes.push(c);
            return tog(c, label, '', 'tog sub', help);
          }),
          h('div', { class: 'row lw' }, h('span', { class: 'muted' }, 'Label opacity'), this.labelOp, this.labelOpOut),
          h('div', { class: 'row lw', title: this.spacing.title }, h('span', { class: 'muted' }, 'Label spacing'), this.spacing, this.spacingOut),
          ...densityRows,
          h('div', { class: 'row lw', title: 'In a tilted view, labels thin out with distance beyond the centre, where the ground is foreshortened (double-click: default)' },
            h('span', { class: 'muted' }, 'Toward horizon'), this.horizon, this.horizonOut),
        ),
        this.section('roads', 'Roads',
          tog(this.roads, 'Roads', h('span', { class: 'km faint' }, 'km in view')),
          h('div', { class: 'row', title: 'Opacity of the roads, in every display type' }, h('span', { class: 'muted' }, 'Opacity'), this.roadOp, this.roadOpOut),
          groups,
          tog(this.surf.paved, 'Paved', this.surfKm[0], 'tog sub'),
          tog(this.surf.unpaved, 'Unpaved (dashed)', this.surfKm[1], 'tog sub'),
          tog(this.tollBox.free, 'Toll-free', this.tollKm[0], 'tog sub', 'Roads without a toll'),
          tog(this.tollBox.toll, 'Toll roads', this.tollKm[1], 'tog sub', 'Roads tagged as tolled in OpenStreetMap (toll=yes), including toll bridges and tunnels'),
          h('div', { class: 'row sub len', title: 'Length of the whole road (every way with the same name or route number, joined end to end). Empty = no limit; untick to switch the filter off and keep the limits.' },
            h('label', { class: 'lenon' }, this.lenOn, h('span', { class: 'muted' }, 'Road length')),
            h('span', { class: 'pair' }, this.lenIn[0], h('span', { class: 'faint' }, '–'), this.lenIn[1]), h('span', { class: 'muted' }, 'km')),
          tog(this.glow, h('span', { class: 'lbl' }, h('span', { class: 'dot', style: 'background:#f5bd4d' }), 'Scenic-route glow'), '', 'tog', 'Gold halo on designated scenic byways and routes touristiques'),
          tog(this.occlude, 'Hide roads behind terrain', '', 'tog',
            'With 3D terrain. Off: roads behind hills are drawn faint, as if seen through them. On: they are hidden.'),
        ),
        this.section('rail', 'Passenger rail lines', ...this.railSection),
        this.section('ferry', 'Ferries', ...this.ferrySection),
        this.section('trees', 'Trees', ...this.trees.nodes),
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
        ),
        this.section('stops', 'Stops & sights',
          tog(this.stopsAll, 'Show stops & sights', '', 'tog', 'Every kind below at once. Off hides them all; on brings back the kinds you had on'),
          h('div', { class: 'row', title: 'Dots, areas and their labels (labels also follow Label opacity). Sizes and fades along the landmark score: the top-left panel' }, h('span', { class: 'muted' }, 'Opacity'), this.poiOp, this.poiOpOut),
          ...byGroup('map'),
          ...designations.slice(0, 1),
          levels,
          ...designations.slice(1),
          ...byGroup('stops'),
        ),
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

  /** The terrain in view, the tint's range in use and its equalisation lookup (if on). */
  updateTint(dist: Dist | null, range: [number, number], cdf: Uint8Array | null) {
    this.tintScale.update(dist, range, cdf);
  }

  setViewshedActive(on: boolean) {
    this.viewshedBtn.classList.toggle('on', on);
    this.viewshedBtn.textContent = on ? 'Click the map… (Esc to cancel)' : 'What can I see from here?';
  }

  /** Loading / count status next to an overlay toggle. */
  /** Stops & sights filter controls from the state. */
  private syncFilters(s: AppState) {
    for (const u of this.sfUi) {
      const f = s.stopFilters[u.key];
      u.on.checked = !!f?.on;
      if (u.lo && document.activeElement !== u.lo) u.lo.value = f?.min ? String(f.min) : '';
      if (u.hi && document.activeElement !== u.hi) u.hi.value = f?.max ? String(f.max) : '';
    }
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
    this.lenOn.checked = s.roadLenOn;
    this.lenOn.disabled = !s.layers.roads;
    this.lenIn.forEach((e, i) => {
      if (document.activeElement !== e) e.value = s.roadLen[i] ? String(s.roadLen[i]) : '';
      e.disabled = !s.layers.roads || !s.roadLenOn;
    });
    for (const { key, input, out } of this.lw) {
      input.value = String(s.lineWeights[key]);
      out.value = `${s.lineWeights[key].toFixed(2)}×`;
    }
    this.railOp.value = String(s.rail.opacity);
    this.railOpOut.value = `${Math.round(s.rail.opacity * 100)} %`;
    this.ferryOp.value = String(s.ferry.opacity);
    this.ferryOpOut.value = `${Math.round(s.ferry.opacity * 100)} %`;
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
    const opts = (el: HTMLSelectElement, list: [string | number, string][]) => {
      const sig = list.map((o) => o.join(':')).join('|');
      if (el.dataset.sig === sig) return;
      el.dataset.sig = sig;
      el.replaceChildren(...list.map(([k, l]) => h('option', { value: k }, l)));
    };
    opts(this.t.tintBands, [[0, 'Smooth'], ...tv.bands.map((b): [number, string] => [b, `${b} ${tv.unit} bands`])]);
    this.t.tintBands.value = String(t.tintBands);
    const lc = -Math.log(t.tintCurve) / Math.log(3);
    this.t.tintCurve.value = String(lc);
    this.t.tintCurveOut.value = Math.abs(lc) < 0.05 ? 'even' : lc > 0 ? 'low' : 'high';
    this.tintScale.sync();
    this.t.tintOp.value = String(t.tintOpacity);
    this.t.tintOpOut.value = `${Math.round(t.tintOpacity * 100)} %`;
    this.globe.checked = s.globe;
    this.labelOp.value = String(s.labelOpacity);
    this.labelOpOut.value = `${Math.round(s.labelOpacity * 100)} %`;
    this.roadOp.value = String(s.roadOpacity);
    this.roadOpOut.value = `${Math.round(s.roadOpacity * 100)} %`;
    this.boundOp.value = String(s.boundaryOpacity);
    this.boundOpOut.value = `${Math.round(s.boundaryOpacity * 100)} %`;
    this.boundOp.disabled = !s.layers.boundaries;
    this.poiOp.value = String(s.poiOpacity);
    this.poiOpOut.value = `${Math.round(s.poiOpacity * 100)} %`;
    const d = s.labelDensity;
    this.spacing.value = String(Math.log2(d.px));
    this.spacingOut.value = `${d.px} px`;
    this.spacing.disabled = !s.layers.places;
    for (const x of this.densities) {
      const f = d.kinds[x.k];
      x.input.value = String(Math.log2(f));
      x.out.value = `×${+f.toFixed(f < 1 ? 2 : 1)}`;
      x.input.disabled = !s.layers.places;
    }
    this.horizon.value = String(d.horizon);
    this.horizonOut.value = `${Math.round(d.horizon * 100)} %`;
    this.horizon.disabled = !s.layers.places;
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

class FreqFilterRow {
  readonly nodes: HTMLElement[];
  private on: HTMLInputElement;
  private min: HTMLInputElement;
  private max: HTMLInputElement;
  private unknown: HTMLInputElement;
  private unknownRow: HTMLElement;

  constructor(label: string, title: string, unknownLabel: string, private get: () => FreqState, private set: (p: Partial<FreqState>) => void) {
    this.on = h('input', { type: 'checkbox' });
    this.on.addEventListener('change', () => this.set({ freqOn: this.on.checked }));
    const num = (key: 'freqMin' | 'freqMax', placeholder: string) => {
      const e = h('input', { type: 'number', min: 0, step: 'any', placeholder, class: 'num' });
      e.addEventListener('change', () => this.set({ [key]: Math.max(0, Number(e.value) || 0) } as Partial<FreqState>));
      return e;
    };
    this.min = num('freqMin', 'min');
    this.max = num('freqMax', 'max');
    this.unknown = h('input', { type: 'checkbox' });
    this.unknown.addEventListener('change', () => this.set({ freqUnknown: this.unknown.checked }));
    this.unknownRow = h('label', { class: 'tog sub unnamed', title: 'Keep showing lines no timetable was found for' }, this.unknown, h('span', {}, unknownLabel), h('span', { class: 'km' }));
    this.nodes = [
      h('div', { class: 'row sub len', title },
        h('label', { class: 'lenon' }, this.on, h('span', { class: 'muted' }, label)),
        h('span', { class: 'pair' }, this.min, h('span', { class: 'faint' }, '–'), this.max), h('span', { class: 'muted' }, '')),
      this.unknownRow,
    ];
  }

  sync(layerOn: boolean) {
    const f = this.get();
    this.on.checked = f.freqOn;
    this.on.disabled = !layerOn;
    for (const [e, v] of [[this.min, f.freqMin], [this.max, f.freqMax]] as const) {
      if (document.activeElement !== e) e.value = v ? String(v) : '';
      e.disabled = !layerOn || !f.freqOn;
    }
    this.unknown.checked = f.freqUnknown;
    this.unknown.disabled = !layerOn || !f.freqOn;
    this.unknownRow.classList.toggle('dim', !f.freqOn);
  }
}
