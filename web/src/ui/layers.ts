import { GROUPS, RAIL0, RAIL_GROUPS } from '../config';
import { RAIL_GROUP_COLOURS } from '../rail';
import { FERRY_GROUPS, FERRY_GROUP_COLOURS } from '../ferry';
import { HERITAGE_GROUPS, HERITAGE_TIERS, POI_STYLE } from '../basemap';
import type { ViewStats } from '../roads/stats';
import { filtersOf, type StopFilter } from '../stopfilters';
import { LABEL_KINDS, OVERLAYS, WEIGHT_MAX, WEIGHT_MIN, defaults, type AppState, type HillshadeMethod, type OverlayKey, type Store, type TintRange, type TintVar } from '../state';
import { baseKey, isRev, withRev } from '../palettes';
import { TINT_PALETTES, TINT_VARS } from '../terrain';
import * as prefs from '../prefs';
import { fmt, h } from './dom';
import { ScaleControls } from './scale';
import { RampSelect } from './rampselect';
import { TreeSection } from './trees';

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
  private weight: HTMLInputElement;
  private weightOut: HTMLOutputElement;
  private glow: HTMLInputElement;
  private boundaryBoxes: HTMLInputElement[] = [];
  private occlude: HTMLInputElement;
  private t: {
    on: HTMLInputElement; ex: HTMLInputElement; exOut: HTMLOutputElement; hs: HTMLInputElement; method: HTMLSelectElement;
    light: HTMLInputElement; lightOut: HTMLOutputElement; shade: HTMLInputElement; shadeOut: HTMLOutputElement;
    tint: HTMLInputElement; contours: HTMLInputElement; sky: HTMLInputElement;
    tintBox: HTMLDivElement; tintVar: HTMLSelectElement; tintPal: RampSelect; tintRange: HTMLSelectElement; customBox: HTMLDivElement;
    tintMin: HTMLInputElement; tintMinOut: HTMLOutputElement; tintMax: HTMLInputElement; tintMaxOut: HTMLOutputElement;
    tintBands: HTMLSelectElement; tintCurve: HTMLInputElement; tintCurveOut: HTMLOutputElement;
    tintOp: HTMLInputElement; tintOpOut: HTMLOutputElement;
    tintFade: HTMLInputElement; tintFadeOut: HTMLOutputElement; tintSpan: HTMLInputElement; tintSpanOut: HTMLOutputElement; tintLegend: HTMLDivElement; tintLo: HTMLSpanElement; tintHi: HTMLSpanElement;
  };
  private globe: HTMLInputElement;
  private labelOp: HTMLInputElement;
  private labelOpOut: HTMLOutputElement;
  private poiOp: HTMLInputElement;
  private poiOpOut: HTMLOutputElement;
  private poiEm: HTMLInputElement;
  private poiEmOut: HTMLOutputElement;
  /** Landmark prominence: the shared scale controls (histogram, fit, fade, highlight) over the
   * landmark score, and the fame ↔ rarity balance. */
  readonly lmScale: ScaleControls;
  private lmBal: HTMLInputElement;
  private lmBalOut: HTMLOutputElement;
  private ov: Partial<Record<OverlayKey, HTMLInputElement>> = {};
  private ovState: Partial<Record<OverlayKey, HTMLSpanElement>> = {};
  /** Heritage groups and their kinds of designation (checkbox, site count). */
  private hGroups: { key: string; c: HTMLInputElement; n: HTMLSpanElement }[] = [];
  private hTiers: { key: string; c: HTMLInputElement; n: HTMLSpanElement }[] = [];
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
    this.railSection = [
      tog(this.rail, 'Passenger rail', h('span', { class: 'km faint' }, 'km in view'), 'tog', 'Tracks used by passenger services (OSM route relations), plus trams, metros, funiculars and heritage lines'),
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
    this.ferrySection = [
      tog(this.ferry, 'Ferries', h('span', { class: 'km faint' }, 'km in view'), 'tog',
        'Passenger ferries, car ferries included (OSM ferry routes). Car ferries are also part of the road network (Roads → Car ferries).'),
      ferryGroups,
      ...(this.ferryFreq = new FreqFilterRow('Sailings a day', 'Sailings a day each way, from operators\u2019 timetables (on a stretch used by several lines, added up). Empty = no limit.',
        'Lines without a timetable', () => this.store.s.ferry, (p) => this.store.set({ ferry: { ...this.store.s.ferry, ...p } }))).nodes,
    ];
    this.weight = slider(WEIGHT_MIN, WEIGHT_MAX, 0.05, (v) => this.store.set({ weight: v }), defaults.weight);
    this.weight.title = 'Width of roads, passenger rail and ferries (double-click: default)';
    this.weightOut = h('output');
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
    const tintPal = new RampSelect(
      TINT_PALETTES.map((p) => ({ key: p.key, label: p.label, group: p.group })),
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
        tintPalette: tv === 'slope' && baseKey(t.tintPalette) === 'atlas' ? withRev('steep', isRev(t.tintPalette))
          : tv === 'elev' && baseKey(t.tintPalette) === 'steep' ? withRev('atlas', isRev(t.tintPalette)) : t.tintPalette,
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
    this.poiOp = slider(0.1, 1, 0.05, (v) => this.store.set({ poiOpacity: v }), defaults.poiOpacity);
    this.poiOpOut = h('output');
    this.poiEm = slider(0, 1, 0.05, (v) => this.store.set({ poiEmphasis: v }), defaults.poiEmphasis);
    this.poiEmOut = h('output');
    const lmSet = (patch: Partial<AppState['landmarks']>) => this.store.set({ landmarks: { ...this.store.s.landmarks, ...patch } });
    this.lmScale = new ScaleControls({
      get: () => this.store.s.landmarks,
      set: lmSet,
      metric: () => ({ domain: [0, 1], step: 0.01, fmt: (v) => String(Math.round(v * 100)) }),
      noun: 'landmarks',
      measure: 'landmarks',
      fadeDefault: defaults.landmarks.lowFade,
      spanDefault: defaults.landmarks.lowSpan,
      onPreview: () => {},
    });
    this.lmBal = slider(0, 1, 0.05, (v) => lmSet({ balance: v }), defaults.landmarks.balance);
    this.lmBalOut = h('output');
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

    this.viewshedBtn = h('button', { class: 'pill wide', title: 'Click a spot on the map to see everything visible from there (trees and terrain block the view)', onclick: () => this.onViewshed() }, 'What can I see from here?');

    root.append(
      h('div', { class: 'hd' }, h('h2', {}, 'Layers')),
      h('div', { class: 'bd scroll' },
        this.section('map', 'Map',
          tog(this.globe, 'Globe', '', 'tog', 'Globe projection; flattens to Web Mercator as you zoom in'),
          h('div', { class: 'row', title: 'Width of roads, passenger rail and ferries' }, h('span', { class: 'muted' }, 'Line weight'), this.weight, this.weightOut),
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
          tog(this.other.places, 'Place labels', '', 'tog', 'Names of places, water, and of the parks, sites and stops shown'),
          ...LABEL_KINDS.map(([k, label, help]) => {
            const c = cb((v) => this.store.set({ labelKinds: { ...this.store.s.labelKinds, [k]: v } }));
            this.labelBoxes.push(c);
            return tog(c, label, '', 'tog sub', help);
          }),
          row('Label opacity', this.labelOp, this.labelOpOut),
        ),
        this.section('roads', 'Roads',
          tog(this.roads, 'Roads', h('span', { class: 'km faint' }, 'km in view')),
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
          h('div', { class: 'row', title: 'Dots, areas and their labels (labels also follow Label opacity)' }, h('span', { class: 'muted' }, 'Opacity'), this.poiOp, this.poiOpOut),
          h('div', { class: 'lm-scale', title: 'Landmark score in view (0–100): how well known (Wikipedia pageviews) and how rare nearby (distance to a better-known one of its kind). Dots are sized and faded along this scale.' },
            this.lmScale.legend, this.lmScale.fadeRow, this.lmScale.thrRow),
          h('div', { class: 'row', title: 'What makes a landmark prominent: how well known it is (Wikipedia pageviews) or how rare it is nearby (distance to a better-known one of its kind)' },
            h('span', { class: 'muted' }, 'Fame ↔ rarity'), this.lmBal, this.lmBalOut),
          h('div', { class: 'row', title: 'How much dot size varies along the scale. 0: all dots the same size.' },
            h('span', { class: 'muted' }, 'Size contrast'), this.poiEm, this.poiEmOut),
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
    this.weight.value = String(s.weight);
    this.weightOut.value = `${s.weight.toFixed(2)}×`;
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
    this.poiOp.value = String(s.poiOpacity);
    this.poiOpOut.value = `${Math.round(s.poiOpacity * 100)} %`;
    this.poiEm.value = String(s.poiEmphasis);
    this.poiEmOut.value = `${Math.round(s.poiEmphasis * 100)} %`;
    const lm = s.landmarks;
    this.lmBal.value = String(lm.balance);
    this.lmBalOut.value = lm.balance <= 0 ? 'fame' : lm.balance >= 1 ? 'rarity' : `${Math.round((1 - lm.balance) * 100)}:${Math.round(lm.balance * 100)}`;
    this.lmScale.sync();
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
