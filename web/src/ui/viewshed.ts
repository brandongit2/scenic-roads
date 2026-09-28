// "What can I see from here?" — click a spot, get the area visible from eye height within a
// radius (terrain + tree canopy block the view), draped on the map, with a summary card.
import type { ImageSource, Map as MLMap } from 'maplibre-gl';
import { getViewshed, type Viewshed } from '../api';
import * as prefs from '../prefs';
import { fmt, h } from './dom';

export class ViewshedTool {
  active = false;
  private card: HTMLElement;
  private body: HTMLDivElement;
  private radius: HTMLSelectElement;
  private eye: HTMLSelectElement;
  private last: [number, number] | null = null;
  private abort: AbortController | null = null;
  onActive: (on: boolean) => void = () => {};
  onMark: (ll: [number, number] | null) => void = () => {};

  constructor(root: HTMLElement, private map: MLMap) {
    this.card = root;
    this.body = h('div', { class: 'vs-body' });
    this.radius = h('select', {}, ...[5, 10, 15, 25].map((r) => h('option', { value: r, selected: r === 15 }, `${r} km`)));
    this.eye = h('select', {},
      h('option', { value: 1.7, selected: true }, 'Standing (1.7 m)'),
      h('option', { value: 5 }, 'Roof / platform (5 m)'),
      h('option', { value: 20 }, 'Tower (20 m)'),
    );
    this.radius.value = String(prefs.load('viewshed.r', 15));
    this.eye.value = String(prefs.load('viewshed.eye', 1.7));
    if (!this.radius.value) this.radius.value = '15';
    if (!this.eye.value) this.eye.value = '1.7';
    const rerun = () => {
      prefs.save('viewshed.r', Number(this.radius.value));
      prefs.save('viewshed.eye', Number(this.eye.value));
      if (this.last) this.run(this.last);
    };
    this.radius.addEventListener('change', rerun);
    this.eye.addEventListener('change', rerun);
    root.append(
      h('div', { class: 'hd' }, h('h2', {}, 'Viewshed'), h('button', { class: 'collapse', title: 'Close', onclick: () => this.clear() }, '✕')),
      h('div', { class: 'bd' },
        h('div', { class: 'vs-ctl' }, this.radius, this.eye, h('button', { class: 'pill', title: 'Pick another spot', onclick: () => this.start() }, 'Move')),
        this.body,
        h('div', { class: 'vs-legend' },
          h('span', {}, h('i', { style: 'background:#ffc45c' }), 'visible land'),
          h('span', {}, h('i', { style: 'background:#60d6ff' }), 'visible water'),
          h('span', {}, h('i', { style: 'background:#06080c' }), 'hidden'),
        ),
      ),
    );
    root.hidden = true;
  }

  start() {
    this.active = true;
    this.map.getCanvas().style.cursor = 'crosshair';
    this.onActive(true);
  }

  cancel() {
    if (!this.active) return;
    this.active = false;
    this.map.getCanvas().style.cursor = '';
    this.onActive(false);
  }

  async run(ll: [number, number]) {
    this.cancel();
    this.last = ll;
    this.onMark(ll);
    this.card.hidden = false;
    this.body.replaceChildren(h('div', { class: 'muted' }, h('span', { class: 'spin' }), ' Tracing 2,048 sight lines over terrain and tree canopy…'));
    this.abort?.abort();
    this.abort = new AbortController();
    try {
      const v = await getViewshed(ll[0], ll[1], Number(this.radius.value), Number(this.eye.value), this.abort.signal);
      this.draw(v);
    } catch (e) {
      if ((e as Error).name !== 'AbortError') this.body.replaceChildren(h('div', { class: 'muted' }, `Could not compute: ${(e as Error).message}`));
    }
  }

  private draw(v: Viewshed) {
    const map = this.map;
    const src = map.getSource<ImageSource>('viewshed');
    if (src) src.updateImage({ url: v.image, coordinates: v.corners });
    else {
      map.addSource('viewshed', { type: 'image', url: v.image, coordinates: v.corners });
      map.addLayer(
        { id: 'viewshed', type: 'raster', source: 'viewshed', paint: { 'raster-opacity': 0.85, 'raster-fade-duration': 0, 'raster-resampling': 'nearest' } },
        'sel-halo',
      );
    }
    const kv = (k: string, val: string) => h('div', { class: 'kv' }, h('span', { class: 'muted' }, k), h('b', {}, val));
    this.body.replaceChildren(
      kv('Ground', fmt.m(v.ground_m)),
      kv('Visible area', `${v.visible_km2 < 10 ? v.visible_km2.toFixed(1) : fmt.n(v.visible_km2)} km²`),
      kv('Water in view', `${v.water_km2 < 10 ? v.water_km2.toFixed(1) : fmt.n(v.water_km2)} km²`),
      kv('Farthest visible', `${v.farthest_km.toFixed(1)} km`),
      kv('Share of circle', `${(v.visible_share * 100).toFixed(1)} %`),
    );
  }

  clear() {
    this.cancel();
    this.abort?.abort();
    this.last = null;
    this.card.hidden = true;
    this.onMark(null);
    if (this.map.getLayer('viewshed')) this.map.removeLayer('viewshed');
    if (this.map.getSource('viewshed')) this.map.removeSource('viewshed');
  }
}
