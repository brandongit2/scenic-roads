// The settings panel's Buildings section (ui/layers.ts): the 3D buildings and how they're drawn
// (buildings.ts): 3D or flat, colour (plain, by height on the shared colour scale, by where the
// height comes from), the heritage tint, opacity, height scale.
import { HEIGHT_DOMAIN, SOURCES, heightColour, type BuildingColour, type BuildingState, type HeightLook } from '../buildings';
import type { Dist } from '../roads/stats';
import type { ScaleFields, Store } from '../state';
import { Slider, pct } from './controls';
import { h } from './dom';
import { ScaleControls } from './scale';

export class BuildingSection {
  readonly nodes: HTMLElement[];
  /** The layer's switch (the section's header). */
  readonly on: HTMLInputElement;
  private showBtns: HTMLButtonElement[] = [];
  private colourBtns: HTMLButtonElement[] = [];
  private op: Slider;
  private scale: Slider;
  private withTerrain: HTMLInputElement;
  private legend: HTMLDivElement;
  private ticks: HTMLDivElement;
  private body: HTMLDivElement;
  private heritage: HTMLInputElement;
  /** Colour by height: the shared colour scale over the buildings in view. */
  private scaleCtl: ScaleControls;
  private scaleBox: HTMLDivElement;
  private look: HeightLook = { range: [0, 100], cdf: null };
  /** Live preview of a colour map while hovering the list (null: back to the chosen one). */
  onPalettePreview: (palette: string | null) => void = () => {};

  constructor(private store: Store) {
    const B = (patch: Partial<BuildingState>) => store.set({ buildings: { ...store.s.buildings, ...patch } });
    this.on = h('input', { type: 'checkbox', title: 'Buildings (B)' });
    this.on.addEventListener('change', () => B({ on: this.on.checked }));
    const seg = (items: [string, string, string][], pick: (k: string) => void, into: HTMLButtonElement[]) => {
      const el = h('div', { class: 'seg small' });
      for (const [k, label, title] of items) {
        const b = h('button', { title, onclick: () => pick(k) }, label);
        into.push(b);
        el.append(b);
      }
      return el;
    };
    const show = seg([['3d', '3D', 'Extruded to their heights on the 3D terrain'], ['flat', 'Flat', 'Footprints only (as a map without 3D terrain shows them)']], (k) => B({ flat: k === 'flat' }), this.showBtns);
    const colour = seg([
      ['plain', 'Plain', 'One colour, lit from the hill-shading’s light'],
      ['height', 'Height', 'Coloured by height'],
      ['source', 'Source', 'Coloured by where the height comes from: measured, from floors, or estimated (and how)'],
    ], (k) => B({ colour: k as BuildingColour }), this.colourBtns);
    this.op = new Slider({ label: 'Opacity', min: 0.1, max: 1, step: 0.05, reset: 0.85, get: () => store.s.buildings.opacity, set: (opacity) => B({ opacity }), fmt: pct, title: 'Under 100 %, roads behind a building show through it faintly (and the buildings take two passes to draw)' });
    this.scale = new Slider({
      label: 'Height', min: 1, max: 3, step: 0.1, reset: 1, get: () => store.s.buildings.scale || 1, set: (scale) => B({ scale }),
      fmt: (v) => `${v.toFixed(1)}×`, title: 'Heights × this (true heights at 1×)', disabled: () => store.s.buildings.scale === 0,
    });
    this.withTerrain = h('input', { type: 'checkbox' });
    this.withTerrain.addEventListener('change', () => B({ scale: this.withTerrain.checked ? 0 : 1 }));
    this.legend = h('div', { class: 'tint-bar' });
    this.ticks = h('div', { class: 'tint-ticks' });
    this.heritage = h('input', { type: 'checkbox' });
    this.heritage.addEventListener('change', () => B({ heritage: this.heritage.checked }));
    const H = () => store.s.buildings.height;
    this.scaleCtl = new ScaleControls({
      get: H,
      set: (patch: Partial<ScaleFields>) => B({ height: { ...H(), ...patch } }),
      metric: () => ({ domain: HEIGHT_DOMAIN, step: 1, fmt: (v: number) => `${Math.round(v)} m`, hiPlus: true }),
      noun: 'buildings',
      measure: 'buildings',
      fadeDefault: 0.6,
      spanDefault: 0.4,
      colourAt: (v) => [heightColour(v, H(), this.look), 1],
      onPreview: (k) => this.onPalettePreview(k),
    });
    this.scaleBox = h('div', { class: 'tint-scale' }, this.scaleCtl.legend, this.scaleCtl.palRow, this.scaleCtl.fadeRow, this.scaleCtl.thrRow);
    const row = (label: string, input: HTMLElement) => h('div', { class: 'row' }, h('span', { class: 'muted' }, label), input, h('span'));
    this.body = h('div', { class: 'tree-body' },
      row('Show', show),
      row('Colour', colour),
      h('div', { class: 'tint-legend' }, this.legend, this.ticks),
      this.scaleBox,
      h('label', { class: 'tog sub', title: 'Buildings holding a heritage site drawn in its colour (World Heritage, national, provincial, municipal) while Heritage sites show' }, this.heritage, h('span', {}, 'Heritage sites in their colour')),
      this.op.el,
      this.scale.el,
      h('label', { class: 'tog sub', title: 'Heights × the terrain’s exaggeration, so buildings keep their proportion to the hills' }, this.withTerrain, h('span', {}, 'With the terrain’s exaggeration')),
      h('div', { class: 'faint note tree-note' }, 'Overture Maps’ buildings (OpenStreetMap, Microsoft, Esri, USGS and others). Where no height is known it is estimated: colour by source to see how. B: on and off.'),
    );
    this.nodes = [this.body];
    this.sync();
  }

  sync() {
    const b = this.store.s.buildings;
    this.on.checked = b.on;
    this.body.classList.toggle('off', !b.on);
    this.showBtns.forEach((x, i) => x.classList.toggle('on', i === (b.flat ? 1 : 0)));
    this.colourBtns.forEach((x, i) => x.classList.toggle('on', ['plain', 'height', 'source'][i] === b.colour));
    this.op.sync();
    this.scale.sync();
    this.withTerrain.checked = b.scale === 0;
    this.heritage.checked = b.heritage;
    const legendBox = this.legend.parentElement as HTMLElement | null;
    if (legendBox) legendBox.hidden = b.colour !== 'source';
    this.scaleBox.hidden = b.colour !== 'height';
    if (b.colour === 'source') {
      this.legend.className = 'tree-chips bld-chips';
      this.legend.style.background = 'none';
      this.legend.replaceChildren(...SOURCES.map(([label, c, help], i) => h('span', { class: 'lg', title: help }, h('i', { style: `background:${c}` }), ['measured', 'floors', 'Microsoft', 'neighbours', 'GHSL', 'size'][i] ?? label)));
      this.ticks.replaceChildren();
    }
    this.scaleCtl.sync();
  }

  /** Colour by height: the buildings' heights in view, the scale's range now and its lookup. */
  updateHeights(dist: Dist | null, look: HeightLook) {
    this.look = look;
    this.scaleCtl.update(dist, look.range, look.cdf);
  }
}
