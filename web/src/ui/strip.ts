// Bottom strip: hover inspector, cursor position, scale, links, attribution.
import type { Map as MLMap } from 'maplibre-gl';
import { CLASS_LABELS, ST_BRIDGE, ST_LINK, ST_TUNNEL, ST_UNPAVED } from '../config';
import type { HoverInfo } from '../roads/layer';
import type { WayInfo } from '../api';
import { fmt, h, toast } from './dom';

export class Strip {
  private r1: HTMLDivElement;
  private coord: HTMLSpanElement;
  private zoom: HTMLSpanElement;
  private scale: HTMLSpanElement;
  private gmaps: HTMLAnchorElement;
  private loading: HTMLSpanElement;
  private idle: HTMLElement;
  /** Extra hover readouts (scenic metrics), supplied by the app. */
  extra: (hov: HoverInfo) => HTMLElement[] = () => [];

  constructor(root: HTMLElement, private map: MLMap) {
    this.idle = h('span', { class: 'hint' }, 'Hover a road for elevation & scenic metrics · click for its profile · two-finger drag pans, pinch zooms, ⌥ + two-finger drag or right-drag tilts & rotates around the cursor · G: Street View at the cursor');
    this.r1 = h('div', { class: 'r1' }, this.idle);
    this.coord = h('span', { class: 'num' });
    this.zoom = h('span', { class: 'num' });
    this.scale = h('span', { class: 'scale' });
    this.loading = h('span');
    this.gmaps = h('a', { target: '_blank', rel: 'noopener', title: 'Open this exact view in Google Maps' }, 'Google Maps ↗');
    const copy = h('button', {
      title: 'Copy a link to this view',
      onclick: async () => {
        await navigator.clipboard.writeText(location.href);
        toast('Link copied');
      },
    }, 'Copy link');
    root.append(
      this.r1,
      h('div', { class: 'r2' }, this.coord, this.zoom, this.scale, this.loading, h('span', { class: 'grow' }),
        this.gmaps, copy,
        h('span', { title: 'OpenStreetMap contributors · NRCan HRDEM/MRDEM · USGS 3DEP · Meta/WRI canopy height · ESA WorldCover · Parks Canada · NPS · MCC Québec · Ontario Heritage Trust · NS · CRHP · UNESCO' }, '© OpenStreetMap · NRCan · USGS · Meta · ESA · UNESCO'),
      ),
    );
    map.on('move', () => this.view());
    map.on('mousemove', (e) => (this.coord.textContent = fmt.coord(e.lngLat.lat, e.lngLat.lng)));
    this.view();
  }

  private view() {
    const z = this.map.getZoom();
    const c = this.map.getCenter();
    this.zoom.textContent = `z ${z.toFixed(1)}`;
    // Google Maps zoom levels are one higher than MapLibre's (256 vs 512 px tiles).
    this.gmaps.href = `https://www.google.com/maps/@${c.lat.toFixed(6)},${c.lng.toFixed(6)},${(z + 1).toFixed(2)}z`;
    // Metric scale bar (~100 px).
    const mpp = (40075016.686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** z);
    const target = 100 * mpp;
    const p = 10 ** Math.floor(Math.log10(target));
    const nice = [1, 2, 5, 10].map((k) => k * p).filter((v) => v <= target).pop() ?? p;
    const bar = h('i');
    bar.style.width = `${nice / mpp}px`;
    this.scale.replaceChildren(bar, fmt.dist(nice));
  }

  setLoading(text: string) {
    this.loading.textContent = text;
  }

  show(hov: HoverInfo | null, info: WayInfo | null | 'loading', colour: string) {
    if (!hov) {
      this.r1.replaceChildren(this.idle);
      return;
    }
    const st = hov.style;
    const cls = CLASS_LABELS[st & 15] + (st & ST_LINK ? ' link' : '');
    const tags = [cls];
    if (st & ST_UNPAVED) tags.push('unpaved');
    if (st & ST_BRIDGE) tags.push('bridge');
    if (st & ST_TUNNEL) tags.push('tunnel');
    const road = h('span', { class: 'road' });
    if (info === 'loading') road.append(h('span', { class: 'spin' }));
    else if (info) {
      if (info.ref) road.append(h('span', { class: 'ref' }, info.ref));
      road.append(info.name || (info.ref ? '' : 'Unnamed road'));
      if (info.surface && !(st & ST_UNPAVED)) tags.push(info.surface);
      if (info.maxspeed) tags.push(`${info.maxspeed} km/h`);
      if (info.lanes) tags.push(`${info.lanes} lanes`);
      if (info.oneway) tags.push('one-way');
      if (info.toll) tags.push('toll');
      if (info.covered) tags.push('covered bridge');
      if (info.route) tags.push(`★ ${info.route}`);
    }
    const sw = h('span', { class: 'sw' });
    sw.style.background = colour;
    const src = info && info !== 'loading' && info.sources.length ? info.sources.map(([s]) => s).join(' + ') : '';
    this.r1.replaceChildren(
      road,
      h('span', { class: 'kv' }, sw, h('b', {}, fmt.m1(hov.elev))),
      h('span', { class: 'kv' }, 'grade ', h('b', {}, fmt.pct(hov.grade))),
      ...this.extra(hov),
      h('span', { class: 'kv' }, tags.join(' · ')),
      ...(src ? [h('span', { class: 'kv faint' }, src)] : []),
    );
  }
}
