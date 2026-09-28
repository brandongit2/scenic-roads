// Designation and stop overlays: GeoJSON fetched on first use, visibility, the heritage level
// filter, and click popups.
import * as maplibregl from 'maplibre-gl';
import type { GeoJSONSource, Map as MLMap, MapGeoJSONFeature } from 'maplibre-gl';
import { HERITAGE_COLORS, HERITAGE_LEVELS, OVERLAY_LAYERS, OVERLAY_SOURCE } from './basemap';
import { OVERLAYS, type AppState, type OverlayKey } from './state';
import { fmt, h } from './ui/dom';
import type { LayersCard } from './ui/layers';

const POI_LABEL: Record<string, string> = {
  viewpoint: 'Viewpoint', peak: 'Peak', waterfall: 'Waterfall', lighthouse: 'Lighthouse', covered_bridge: 'Covered bridge',
  rest_area: 'Rest area', picnic_site: 'Picnic site', trailhead: 'Trailhead',
};
const SPECIAL_LABEL: Record<string, string> = { biosphere: 'UNESCO Biosphere Reserve', geopark: 'UNESCO Global Geopark', dark_sky: 'Dark-sky place' };

/** Point layers are tested before roads on click; area layers after. */
export const POINT_LAYERS = ['heritage-pt', ...Object.keys(OVERLAY_LAYERS).filter((k) => OVERLAY_SOURCE[k] === 'pois').map((k) => `poi-${k}`)];
export const AREA_LAYERS = ['heritage-area-fill', 'special-fill', 'indigenous-fill', 'park-fill'];

export class Overlays {
  private loads = new Map<string, Promise<GeoJSON.FeatureCollection | null>>();
  private data = new Map<string, GeoJSON.FeatureCollection>();
  private popup: maplibregl.Popup | null = null;
  onBusy: (label: string | null) => void = () => {};

  constructor(private map: MLMap, private layers: LayersCard) {}

  apply(s: AppState) {
    const map = this.map;
    for (const [k] of OVERLAYS) {
      const on = s.overlays[k];
      const src = OVERLAY_SOURCE[k];
      if (on && src) this.ensure(src, k);
      for (const id of OVERLAY_LAYERS[k] ?? []) if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', on ? 'visible' : 'none');
      if (!on) this.layers.setOverlayStatus(k, '');
      else if (src && this.data.has(src)) this.layers.setOverlayStatus(k, this.count(k, src));
    }
    const lv = s.heritageLevels.flatMap((on, i) => (on ? [i + 1] : []));
    if (map.getLayer('heritage-pt')) {
      map.setFilter('heritage-pt', ['in', ['get', 'level'], ['literal', lv]]);
      map.setFilter('heritage-label', [
        'all',
        ['in', ['get', 'level'], ['literal', lv]],
        ['any', ['<=', ['get', 'level'], 1], ['all', ['<=', ['get', 'level'], 2], ['>=', ['zoom'], 9]], ['>=', ['zoom'], 13]],
      ]);
    }
  }

  private count(k: OverlayKey, src: string): string {
    const fc = this.data.get(src)!;
    if (src !== 'pois') return fmt.n(fc.features.length);
    const kinds = k === 'rest' ? ['rest_area', 'picnic_site'] : [k];
    return fmt.n(fc.features.filter((f) => kinds.includes(f.properties?.kind)).length);
  }

  private ensure(src: string, k: OverlayKey) {
    if (this.loads.has(src)) {
      if (!this.data.has(src)) this.layers.setOverlayStatus(k, '', true);
      return;
    }
    this.layers.setOverlayStatus(k, '', true);
    this.onBusy(`Loading ${src.replace('-', ' ')}…`);
    const p = fetch(`/api/layer/${src}`)
      .then((r) => (r.ok ? r.json() : null))
      .catch(() => null)
      .then((fc: GeoJSON.FeatureCollection | null) => {
        this.onBusy(null);
        if (!fc) return null;
        this.data.set(src, fc);
        this.map.getSource<GeoJSONSource>(src)?.setData(fc);
        for (const [key] of OVERLAYS) if (OVERLAY_SOURCE[key] === src) this.layers.setOverlayStatus(key, this.count(key, src));
        return fc;
      });
    this.loads.set(src, p);
  }

  /** Show a popup for the overlay feature at a point, if any. Returns true if handled. */
  click(point: maplibregl.PointLike, layers: string[]): boolean {
    const map = this.map;
    const ids = layers.filter((id) => map.getLayer(id) && map.getLayoutProperty(id, 'visibility') !== 'none');
    if (!ids.length) return false;
    const p = map.project(map.unproject(point));
    const pad = 5;
    const fs = map.queryRenderedFeatures([[p.x - pad, p.y - pad], [p.x + pad, p.y + pad]], { layers: ids });
    if (!fs.length) return false;
    // Prefer the most significant heritage level, then points over areas.
    fs.sort((a, b) => (a.properties?.level ?? 9) - (b.properties?.level ?? 9));
    this.show(fs[0], map.unproject(point));
    return true;
  }

  closePopup() {
    this.popup?.remove();
    this.popup = null;
  }

  private show(f: MapGeoJSONFeature, at: maplibregl.LngLat) {
    const p = f.properties ?? {};
    const lid = f.layer.id;
    const body = h('div', { class: 'pop' });
    const put = (...xs: (Node | string | null)[]) => body.append(...(xs.filter((x) => x !== null) as (Node | string)[]));
    const link = (url: string | undefined, label: string) =>
      url ? h('a', { href: url, target: '_blank', rel: 'noopener' }, `${label} ↗`) : null;
    const kv = (k: string, v: unknown) => (v === undefined || v === null || v === '' ? null : h('div', { class: 'kv' }, h('span', {}, k), h('b', {}, String(v))));
    if (lid === 'heritage-pt') {
      const lv = Number(p.level) || 5;
      const dot = h('span', { class: 'dot' });
      dot.style.background = HERITAGE_COLORS[lv - 1];
      put(
        h('div', { class: 'ttl' }, p.name || 'Designated place'),
        h('div', { class: 'sub' }, dot, `${p.designation ?? ''}`, h('span', { class: 'faint' }, ` · ${HERITAGE_LEVELS[lv - 1]}`)),
        kv('Designated', p.date),
        kv('Authority', p.authority),
        kv('Municipality', p.municipality),
        kv('Category', p.category ?? p.type),
        kv('Built', p.built),
        kv('Criteria', p.criteria),
        p.in_danger ? h('div', { class: 'warn' }, 'On the List of World Heritage in Danger') : null,
        p.approx ? h('div', { class: 'warn' }, `Location matched by name (${p.location}); may be approximate`) : null,
        h('div', { class: 'links' }, link(p.url, lv === 1 ? 'UNESCO page' : 'Official record')),
        h('div', { class: 'src' }, `Source: ${p.source ?? ''}`),
        p.notice ? h('div', { class: 'src' }, p.notice) : null,
      );
    } else if (lid.startsWith('poi-')) {
      put(
        h('div', { class: 'ttl' }, p.name || POI_LABEL[p.kind] || 'Point of interest'),
        h('div', { class: 'sub' }, POI_LABEL[p.kind] ?? p.kind),
        kv('Elevation', p.ele ? fmt.m(Number(p.ele)) : null),
        h('div', { class: 'src' }, 'Source: OpenStreetMap'),
      );
    } else if (lid === 'special-fill') {
      put(
        h('div', { class: 'ttl' }, p.name),
        h('div', { class: 'sub' }, SPECIAL_LABEL[p.kind] ?? p.kind, p.category ? ` · ${p.category}` : ''),
        kv('Certified by', p.certifier),
        kv('Since', p.year),
        kv('Area', p.area_km2 ? `${fmt.n(Number(p.area_km2))} km²` : null),
        p.approx ? h('div', { class: 'warn' }, 'Boundary approximated by a circle of the designated area') : null,
        h('div', { class: 'links' }, link(p.url, 'Official page')),
        h('div', { class: 'src' }, `Source: ${p.source ?? ''}`),
      );
    } else if (lid === 'heritage-area-fill') {
      put(
        h('div', { class: 'ttl' }, p.name),
        h('div', { class: 'sub' }, p.designation ?? 'Heritage district'),
        kv('Designated', p.date),
        kv('Municipality', p.municipality),
        kv('By-law', p.bylaw),
        kv('Properties', p.properties_count),
        h('div', { class: 'links' }, link(p.url, 'Official record')),
        h('div', { class: 'src' }, `Source: ${p.source ?? ''}`),
      );
    } else if (lid === 'indigenous-fill') {
      put(h('div', { class: 'ttl' }, p.name || 'Indigenous land'), h('div', { class: 'sub' }, 'Indigenous land / reserve'), h('div', { class: 'src' }, 'Boundary: OpenStreetMap'));
    } else if (lid === 'park-fill') {
      put(
        h('div', { class: 'ttl' }, p.name || p['name:latin'] || 'Protected area'),
        h('div', { class: 'sub' }, String(p.class ?? 'protected area').replace(/_/g, ' ')),
        h('div', { class: 'src' }, 'Boundary: OpenStreetMap'),
      );
    } else return;
    this.closePopup();
    this.popup = new maplibregl.Popup({ closeButton: true, maxWidth: '300px', className: 'dark-pop', offset: 8 })
      .setLngLat(at)
      .setDOMContent(body)
      .addTo(this.map);
  }
}
