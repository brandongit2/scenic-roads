// Designation and stop overlays: GeoJSON fetched on first use, visibility, the heritage level
// filter, and click popups.
import * as maplibregl from 'maplibre-gl';
import { cdfOf } from './ui/scale';
import { distFromSamples, type Dist } from './roads/stats';
import type { ExpressionSpecification, GeoJSONSource, Map as MLMap, MapGeoJSONFeature } from 'maplibre-gl';
import { HERITAGE_GROUPS, HERITAGE_TIER, HERITAGE_TIERS, LABEL_SPACING_PX, LANDMARK_LABELS, OVERLAY_LAYERS, POI_STYLE, SIG_LAYERS, landmarkScoreOf, prominencePaint, spacingFilter, heritageGroupOf, heritageTierOf, OVERLAY_SOURCE, baseId, labelKindOf, partIds } from './basemap';
import { OVERLAYS, labelShown, type AppState, type LabelKind, type OverlayKey } from './state';
import { ver } from './api';
import { enrichArea, enrichHeritage, enrichPoi, loadDetail, type Detail, type DetailRef, type Enriched } from './details';
import { stopFilterExpr, stopFilterPass } from './stopfilters';
import type { KindQuery, LandmarkItem, LandmarkRequest, LandmarkResponse } from './landmarks.worker';
import { cap, fmt, h } from './ui/dom';
import type { LayersCard } from './ui/layers';

const POI_LABEL: Record<string, string> = {
  viewpoint: 'Viewpoint', peak: 'Peak', waterfall: 'Waterfall', lighthouse: 'Lighthouse', covered_bridge: 'Covered bridge',
  rest_area: 'Rest area', picnic_site: 'Picnic site', trailhead: 'Trailhead',
};
const SPECIAL_LABEL: Record<string, string> = { biosphere: 'UNESCO Biosphere Reserve', geopark: 'UNESCO Global Geopark', dark_sky: 'Dark-sky place' };

/** A hovered map feature, summarised for the bottom bar. */
export interface FeatureSummary {
  title: string;
  kind: string;
  colour: string;
  facts: string[];
  source: string;
  /** Second-row description (replaces the source line), truncated to fit. */
  desc?: string;
  area: boolean;
  /** Where its details come from (/api/detail, /api/park), and what kind of thing it is. */
  ref?: DetailRef;
  what?: string;
  /** Other places at the same spot. */
  also?: string[];
}

/** A summary with its loaded details: their facts after the layer's own, their description. */
export function withDetails(f: FeatureSummary, d: Detail | null): FeatureSummary {
  const e = d ? enrichOf(f, d) : null;
  if (!e) return f;
  const facts = [...f.facts];
  for (const x of e.facts) if (!facts.includes(x)) facts.push(x);
  return { ...f, facts, desc: e.desc ?? f.desc, source: f.source || d?.props?.source || '' };
}

function enrichOf(f: FeatureSummary, d: Detail): Enriched | null {
  if (!f.ref) return null;
  if ('park' in f.ref) return enrichArea(d);
  switch (f.ref.layer) {
    case 'poi': return enrichPoi(f.what ?? '', {}, d);
    case 'heritage': return enrichHeritage(d);
    default: return enrichArea(d);
  }
}

const PARK_COLOUR = '#5fb37a';
const SPECIAL_COLOUR = '#b48cff';
const INDIGENOUS_COLOUR = '#e0a060';
const HERITAGE_AREA_COLOUR = '#e7a0ff';

/** Most significant first: heritage level, then how widely it is covered (Wikipedia sitelinks), as
 * the heritage labels are placed (basemap.ts). */
const significance = (a: MapGeoJSONFeature, b: MapGeoJSONFeature) =>
  (a.properties?.level ?? 9) - (b.properties?.level ?? 9) || (b.properties?.sl ?? 0) - (a.properties?.sl ?? 0);

/** Point layers are tested before roads on click; area layers after. */
export const POINT_LAYERS = ['heritage-pt', ...Object.keys(OVERLAY_LAYERS).filter((k) => OVERLAY_SOURCE[k] === 'pois').map((k) => `poi-${k}`)];
/** Area layers (parks: the basemap's, and its parts' clones). */
export const areaLayers = () => ['heritage-area-fill', 'special-fill', 'indigenous-fill', ...partIds('park-fill')];

/** An overlay's layer file, as the map draws it (dem/layers.py), versioned for the browser cache. */
const layerUrl = (src: string) => `${location.origin}/api/layer/${src}${ver(`layer-${src}.json`) || ver(`${src}.json`)}`;
type PointSource = 'pois' | 'heritage';
const isPoints = (src: string): src is PointSource => src === 'pois' || src === 'heritage';

/**
 * The overlays' data lives off the main thread: MapLibre's worker fetches and tiles each layer file
 * (the sources get its URL), and the landmarks worker (landmarks.worker.ts) indexes the stops &
 * sights and heritage sites for everything "in view" (prominence, counts, Sights, summit). The page
 * never parses or holds the features.
 */
export class Overlays {
  /** Sources whose map data is set. */
  private sourced = new Set<string>();
  /** Point sources in the landmarks worker. */
  private indexed = new Map<PointSource, 'loading' | 'ready'>();
  private worker = new Worker(new URL('./landmarks.worker.ts', import.meta.url), { type: 'module' });
  private queryId = 0;
  private countId = 0;
  private countFor = new Map<number, OverlayKey>();
  /** Feature counts and areas of the polygon overlays (layer-summary.json), for their counts. */
  private summary: Record<string, { n: number; a: (number | null)[] }> | null = null;
  private summaryLoading = false;
  /** Paint last set per layer and property (unchanged values aren't set again: each set
   * re-evaluates every loaded feature). */
  private painted = new Map<string, string>();
  private summit: { name: string; ele: number; lngLat: [number, number] } | null = null;
  private top: LandmarkItem[] = [];
  private topByKind: Record<string, LandmarkItem[]> = {};
  private popup: maplibregl.Popup | null = null;
  /** Layers' own filters (kinds, names), which the Stops & sights filters are added to. */
  private baseFilters = new Map<string, unknown>();
  private state: AppState | null = null;
  onBusy: (label: string | null) => void = () => {};

  constructor(private map: MLMap, private layers: LayersCard) {
    this.worker.onmessage = (ev: MessageEvent<LandmarkResponse>) => this.onWorker(ev.data);
  }

  private onWorker(m: LandmarkResponse) {
    if (m.type === 'loaded') {
      const src = m.src as PointSource;
      this.indexed.set(src, 'ready');
      this.onBusy(null);
      if (src === 'heritage') this.layers.setHeritageCounts(m.counts);
      for (const [key] of OVERLAYS) if (OVERLAY_SOURCE[key] === src && this.state?.overlays[key]) this.refreshStatus(key, src);
      this.prominence();
    } else if (m.type === 'count') {
      const k = this.countFor.get(m.id);
      this.countFor.delete(m.id);
      if (k && this.state?.overlays[k]) this.layers.setOverlayStatus(k, m.n === m.of ? fmt.n(m.of) : `${fmt.n(m.n)} of ${fmt.n(m.of)}`);
    } else if (m.type === 'result' && m.id === this.queryId) {
      this.applyResult(m);
    }
  }

  apply(s: AppState) {
    const map = this.map;
    this.state = s;
    for (const [k] of OVERLAYS) {
      const on = s.overlays[k];
      const src = OVERLAY_SOURCE[k];
      if (on && src) this.ensure(src, k);
      // Labels follow "Place labels" and their kind's toggle under it.
      for (const id of (OVERLAY_LAYERS[k] ?? []).flatMap(partIds)) {
        const lk = labelKindOf(id) as LabelKind | undefined;
        const show = on && (!lk || labelShown(s, lk));
        if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', show ? 'visible' : 'none');
      }
      // Stops & sights filters, with each layer's own filter.
      // Names appear by interest isolation (LABEL_SPACING_PX); dots always (sized and faded by
      // significance).
      if (k !== 'heritage') {
        const extra = stopFilterExpr(k, s.stopFilters, s.stopUnknown[k] !== false);
        for (const id of (OVERLAY_LAYERS[k] ?? []).flatMap(partIds)) {
          if (!map.getLayer(id)) continue;
          if (!this.baseFilters.has(id)) this.baseFilters.set(id, map.getFilter(id) ?? null);
          const base = this.baseFilters.get(id) as ExpressionSpecification | null;
          const thin = id.startsWith('poi-') && id.endsWith('-label') ? spacingFilter(LABEL_SPACING_PX) : null;
          const parts = [base, extra, thin].filter((x): x is ExpressionSpecification => !!x);
          map.setFilter(id, parts.length > 1 ? ['all', ...parts] : parts[0] ?? null);
        }
      }
      if (!on) this.layers.setOverlayStatus(k, '');
      else if (src) this.refreshStatus(k, src);
    }
    const shown = HERITAGE_TIERS.map((t) => t.key).filter((k) => !s.heritageOff.includes(k));
    if (map.getLayer('heritage-pt')) {
      const extra = stopFilterExpr('heritage', s.stopFilters, s.stopUnknown.heritage !== false);
      const levels: ExpressionSpecification = ['in', HERITAGE_TIER, ['literal', shown]];
      map.setFilter('heritage-pt', extra ? ['all', levels, extra] : levels);
      map.setFilter('heritage-label', [
        'all',
        levels,
        // Names once a site's interest isolation spans LABEL_SPACING_PX (older data: by level).
        ['case', ['has', 'mz'], spacingFilter(LABEL_SPACING_PX)!,
          ['any', ['<=', ['get', 'level'], 1], ['all', ['<=', ['get', 'level'], 2], ['>=', ['zoom'], 9]], ['>=', ['zoom'], 13]]],
        ...(extra ? [extra] : []),
      ]);
    }
    this.prominence(s);
  }

  /** The landmark scale's distribution, range and lookup, for the Layers panel's legend. */
  onScale: ((dist: Dist | null, range: [number, number], cdf: Uint8Array | null) => void) | null = null;
  /** The landmarks in view changed (In view summary, Sights list). */
  onView: (() => void) | null = null;
  /** Outline of the ground in view (lng, lat), as the "in view" lists use; else the bounds. */
  viewOutline: (() => [number, number][]) | null = null;

  /** Whether a point is in view: within the ground outline (and its bounding box). */
  private inViewTest(): (x: number, y: number) => boolean {
    const poly = this.viewOutline?.() ?? [];
    if (poly.length < 3) {
      const b = this.map.getBounds();
      const [w, s, e, n] = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
      return (x, y) => y >= s && y <= n && (w <= e ? x >= w && x <= e : x >= w || x <= e);
    }
    return inPolygon(poly);
  }
  /** In view (after the filters), per visible landmark kind: how many, and the best-known (fame). */
  viewLandmarks: { key: OverlayKey; label: string; colour: string; n: number; best: { name: string; lngLat: [number, number]; layer: string } | null }[] = [];

  /** The most prominent landmarks in view (optionally of one kind), for the Sights list. */
  topInView(kind: OverlayKey | null, limit = 60): LandmarkItem[] {
    return (kind ? this.topByKind[kind] ?? [] : this.top).slice(0, limit);
  }

  /** The highest named peak in view, as of the last prominence pass (indexes the stops & sights if
   * not yet). */
  summitInView(): { name: string; ele: number; lngLat: [number, number] } | null {
    this.ensureIndex('pois');
    return this.summit;
  }

  /** A landmark kind for the worker: its source and layer, and its filters. */
  private kindQuery(k: OverlayKey, src: PointSource, layer: string): KindQuery {
    const s = this.state!;
    return {
      k, src, layer,
      off: k === 'heritage' ? [...s.heritageOff] : undefined,
      filters: Object.fromEntries(Object.entries(s.stopFilters).filter(([key]) => key.startsWith(`${k}.`))),
      keepUnknown: s.stopUnknown[k] !== false,
    };
  }

  /** Size and fade the landmark dots by prominence: the scores of the visible landmarks in view (all
   * kinds, after their filters) give the histogram and the auto-fitted range; each dot is placed on
   * that scale (landmarks: range or lock, equalisation, fade, highlight). */
  prominence(s: AppState = this.state!) {
    if (!s) return;
    this.state = s;
    const map = this.map;
    const kinds: KindQuery[] = [];
    for (const id of SIG_LAYERS) {
      const k = (id === 'heritage-pt' ? 'heritage' : id.slice(4)) as OverlayKey;
      const src = OVERLAY_SOURCE[k];
      if (!map.getLayer(id) || !s.overlays[k] || !src || !isPoints(src) || this.indexed.get(src) !== 'ready') continue;
      kinds.push(this.kindQuery(k, src, id));
    }
    const b = map.getBounds();
    this.worker.postMessage({
      type: 'query', id: ++this.queryId, outline: this.viewOutline?.() ?? [], bounds: [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()],
      balance: s.landmarks.balance, kinds, top: 60,
    } satisfies LandmarkRequest);
  }

  /** The worker's answer for the view: the summaries, and the dots sized and faded on the scale. */
  private applyResult(r: Extract<LandmarkResponse, { type: 'result' }>) {
    const s = this.state!;
    const map = this.map;
    const lm = s.landmarks;
    this.viewLandmarks = r.byKind.map((x) => ({
      key: x.key, label: OVERLAYS.find((o) => o[0] === x.key)?.[1] ?? x.key,
      colour: x.key === 'heritage' ? HERITAGE_GROUPS[1].colour : POI_STYLE[x.key]?.[1] ?? '#888',
      n: x.n, best: x.best ? { ...x.best, name: cap(x.best.name) } : null,
    }));
    this.top = r.top;
    this.topByKind = r.topByKind;
    this.summit = r.summit ? { ...r.summit, name: cap(r.summit.name) } : null;
    const dist = distFromSamples(r.scores, new Float32Array(r.scores.length).fill(1), 0, 1);
    const range: [number, number] = !lm.auto ? lm.range
      : dist && dist.total > 0 ? spreadRange(dist.quantile(lm.fit[0] / 100), dist.quantile(lm.fit[1] / 100)) : lm.range;
    const cdf = lm.equalize ? cdfOf(dist, range) : null;
    const eqStops: [number, number][] | null = cdf
      ? Array.from({ length: 33 }, (_, i) => [range[0] + (i / 32) * (range[1] - range[0]), cdf[Math.round((i / 32) * 255)] / 255] as [number, number])
      : null;
    // The range to 0.005 (finer is invisible): a changed paint re-evaluates every loaded dot.
    const q = (v: number) => Math.round(v * 200) / 200;
    const rq: [number, number] = [q(range[0]), q(range[1])];
    const eq = eqStops?.map(([a, b]) => [q(a), q(b)] as [number, number]) ?? null;
    const paint = (id: string, prop: 'circle-radius' | 'circle-opacity' | 'circle-stroke-opacity' | 'text-opacity', v: unknown) => {
      const key = `${id}|${prop}`, j = JSON.stringify(v);
      if (this.painted.get(key) === j) return;
      this.painted.set(key, j);
      map.setPaintProperty(id, prop, v as ExpressionSpecification);
    };
    for (const id of SIG_LAYERS) {
      if (!map.getLayer(id)) continue;
      const k = (id === 'heritage-pt' ? 'heritage' : id.slice(4)) as OverlayKey;
      if (!s.overlays[k]) continue; // hidden: styled when shown (apply → prominence)
      const p = prominencePaint(id, { range: rq, eqStops: eq, lowFade: lm.lowFade, lowSpan: lm.lowSpan, threshold: lm.threshold, balance: lm.balance }, s.poiEmphasis, s.poiOpacity);
      paint(id, 'circle-radius', p.radius);
      paint(id, 'circle-opacity', p.opacity);
      paint(id, 'circle-stroke-opacity', p.opacity);
      const lid = LANDMARK_LABELS[id];
      if (map.getLayer(lid)) paint(lid, 'text-opacity', p.label(s.labelOpacity));
    }
    this.onScale?.(dist, range, cdf);
    this.onView?.();
  }

  /** Fly to a landmark and open its popup. */
  openAt(lngLat: [number, number], layer: string) {
    const map = this.map;
    map.flyTo({ center: lngLat, zoom: Math.max(map.getZoom(), 12), duration: 900 });
    map.once('idle', () => this.click(map.project(lngLat), [layer]));
  }

  /** The Layers panel's count for an overlay: "n", or "n of N" when filtered. */
  private refreshStatus(k: OverlayKey, src: string) {
    const s = this.state;
    if (!s) return;
    if (isPoints(src)) {
      if (this.indexed.get(src) !== 'ready') return; // shown as loading until indexed
      const id = ++this.countId;
      this.countFor.set(id, k);
      this.worker.postMessage({ type: 'count', id, kind: this.kindQuery(k, src, '') } satisfies LandmarkRequest);
      return;
    }
    const sm = this.summary?.[k];
    if (!sm) return this.ensureSummary();
    const pass = stopFilterPass(k, s.stopFilters, s.stopUnknown[k] !== false);
    this.layers.setOverlayStatus(k, pass ? `${fmt.n(sm.a.filter((a) => pass({ a })).length)} of ${fmt.n(sm.n)}` : fmt.n(sm.n));
  }

  /** An overlay's source gets its layer file (MapLibre's worker fetches and tiles it); points are
   * also indexed in the landmarks worker. */
  private ensure(src: string, k: OverlayKey) {
    if (isPoints(src)) this.ensureIndex(src);
    if (this.sourced.has(src)) return;
    this.sourced.add(src);
    this.map.getSource<GeoJSONSource>(src)?.setData(layerUrl(src));
    if (!isPoints(src)) this.refreshStatus(k, src);
  }

  private ensureIndex(src: PointSource) {
    if (this.indexed.has(src)) return;
    this.indexed.set(src, 'loading');
    for (const [key] of OVERLAYS) if (OVERLAY_SOURCE[key] === src && this.state?.overlays[key]) this.layers.setOverlayStatus(key, '', true);
    this.onBusy(`Loading ${src === 'pois' ? 'stops & sights' : 'heritage sites'}…`);
    this.worker.postMessage({ type: 'load', src, url: layerUrl(src) } satisfies LandmarkRequest);
  }

  private ensureSummary() {
    if (this.summary || this.summaryLoading) return;
    this.summaryLoading = true;
    fetch(`/api/layer/summary${ver('layer-summary.json')}`)
      .then((r) => (r.ok ? r.json() : null))
      .catch(() => null)
      .then((sm) => {
        this.summary = sm ?? {};
        for (const [key] of OVERLAYS) {
          const src = OVERLAY_SOURCE[key];
          if (src && !isPoints(src) && this.state?.overlays[key]) this.refreshStatus(key, src);
        }
      });
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
    // Every feature at the spot (sites often share one: a national historic site and a heritage
    // building in it), most significant heritage level first.
    fs.sort(significance);
    const seen = new Set<string>();
    const uniq = fs.filter((f) => {
      const k = `${f.layer.id}|${f.properties?.i ?? ''}|${f.properties?.name ?? ''}`;
      return !seen.has(k) && !!seen.add(k);
    });
    this.show(uniq.slice(0, 6), map.unproject(point), uniq.length);
    return true;
  }

  /** Features under a screen point for the hover readout: markers (within a few px) and the
   * highlighted areas containing the point, most significant first. */
  hoverAt(pt: { x: number; y: number }): { point: FeatureSummary | null; areas: FeatureSummary[] } {
    const map = this.map;
    const vis = (ids: string[]) => ids.filter((id) => map.getLayer(id) && map.getLayoutProperty(id, 'visibility') !== 'none');
    const pts = vis(POINT_LAYERS);
    let point: FeatureSummary | null = null;
    if (pts.length) {
      const fs = map.queryRenderedFeatures([[pt.x - 5, pt.y - 5], [pt.x + 5, pt.y + 5]], { layers: pts });
      fs.sort(significance);
      point = fs.length ? summarise(fs[0], map.unproject([pt.x, pt.y])) : null;
      // Others at the same spot (click lists them all).
      const names = new Set(fs.slice(1).map((f) => cap(f.properties?.name ?? '')).filter((n) => n && n !== cap(point?.title)));
      if (point && names.size) point.also = [...names];
    }
    const ars = vis(areaLayers());
    const seen = new Set<string>();
    const at = map.unproject([pt.x, pt.y]);
    const areas = (ars.length ? map.queryRenderedFeatures([pt.x, pt.y], { layers: ars }) : [])
      .map((f) => summarise(f, at))
      .filter((f): f is FeatureSummary => !!f && !seen.has(f.title + f.kind) && !!seen.add(f.title + f.kind));
    return { point, areas };
  }

  closePopup() {
    this.popup?.remove();
    this.popup = null;
  }

  /** A popup with a section per feature (at most a few), details filled in as they load. */
  private show(fs: MapGeoJSONFeature[], at: maplibregl.LngLat, total: number) {
    const parts = fs.map((f) => this.section(f, at)).filter((b): b is HTMLElement => !!b);
    if (!parts.length) return;
    const box = h('div', { class: 'pop-multi' });
    if (total > 1) box.append(h('div', { class: 'pop-count faint' }, `${total} places here`));
    parts.forEach((b, i) => {
      if (i) box.append(h('div', { class: 'pop-sep' }));
      box.append(b);
    });
    this.closePopup();
    this.popup = new maplibregl.Popup({ closeButton: true, maxWidth: '320px', className: 'dark-pop', offset: 8 })
      .setLngLat(at)
      .setDOMContent(box)
      .addTo(this.map);
  }

  private section(f: MapGeoJSONFeature, at: maplibregl.LngLat): HTMLElement | null {
    const p = f.properties ?? {};
    const lid = f.layer.id;
    const body = h('div', { class: 'pop' });
    const put = (...xs: (Node | string | null)[]) => body.append(...(xs.filter((x) => x !== null) as (Node | string)[]));
    const link = (url: string | undefined, label: string) =>
      url ? h('a', { href: url, target: '_blank', rel: 'noopener' }, `${label} ↗`) : null;
    const kv = (k: string, v: unknown) => (v === undefined || v === null || v === '' ? null : h('div', { class: 'kv' }, h('span', {}, k), h('b', {}, String(v))));
    if (lid === 'heritage-pt') {
      const dot = h('span', { class: 'dot' });
      dot.style.background = heritageGroupOf(heritageTierOf(p)).colour;
      // The site's record (dates, authority, links, source) comes with its details (props).
      const rows = h('div'), links = h('div', { class: 'links' }), src = h('div', { class: 'src' }), notice = h('div', { class: 'src' });
      put(h('div', { class: 'ttl' }, cap(p.name) || 'Designated place'),
        h('div', { class: 'sub' }, dot, `${p.designation ?? ''}`, h('span', { class: 'faint' }, ` · ${heritageKindLabel(p)}`)),
        rows, links, src, notice);
      const fill = (q: Record<string, any>) => {
        rows.replaceChildren(...[
          kv('Designated', q.date), kv('Authority', q.authority), kv('Municipality', q.municipality), kv('Category', q.category ?? q.type),
          kv('Built', q.built), kv('Criteria', q.criteria),
          q.in_danger ? h('div', { class: 'warn' }, 'On the List of World Heritage in Danger') : null,
          q.approx ? h('div', { class: 'warn' }, `Location matched by name (${q.location}); may be approximate`) : null,
        ].filter((x) => x !== null) as HTMLElement[]);
        const l = link(q.url, Number(q.level) === 1 ? 'UNESCO page' : 'Official record');
        links.replaceChildren(...(l ? [l] : []));
        src.textContent = q.source ? `Source: ${q.source}` : '';
        notice.textContent = q.notice ?? '';
      };
      fill(p);
      const ref = { layer: 'heritage' as const, i: Number(p.i) };
      if (Number.isFinite(ref.i)) loadDetail(ref).then((d) => d?.props && fill({ ...p, ...d.props }));
    } else if (lid.startsWith('poi-')) {
      put(
        h('div', { class: 'ttl' }, cap(p.name) || POI_LABEL[p.kind] || 'Point of interest'),
        h('div', { class: 'sub' }, POI_LABEL[p.kind] ?? p.kind),
        kv('Elevation', p.ele ? fmt.m(Number(p.ele)) : null),
        h('div', { class: 'src' }, 'Source: OpenStreetMap'),
      );
    } else if (lid === 'special-fill') {
      put(
        h('div', { class: 'ttl' }, cap(p.name)),
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
        h('div', { class: 'ttl' }, cap(p.name)),
        h('div', { class: 'sub' }, p.designation ?? 'Heritage district'),
        kv('Designated', p.date),
        kv('Municipality', p.municipality),
        kv('By-law', p.bylaw),
        kv('Properties', p.properties_count),
        h('div', { class: 'links' }, link(p.url, 'Official record')),
        h('div', { class: 'src' }, `Source: ${p.source ?? ''}`),
      );
    } else if (lid === 'indigenous-fill') {
      put(h('div', { class: 'ttl' }, cap(p.name) || 'Indigenous land'), h('div', { class: 'sub' }, 'Indigenous land / reserve'), h('div', { class: 'src' }, 'Boundary: OpenStreetMap'));
    } else if (baseId(lid) === 'park-fill') {
      put(
        h('div', { class: 'ttl' }, cap(p.name || p['name:latin']) || 'Protected area'),
        h('div', { class: 'sub' }, String(p.class ?? 'protected area').replace(/_/g, ' ')),
        h('div', { class: 'src' }, 'Boundary: OpenStreetMap'),
      );
    } else return null;
    // Details (description, facts, links), filled in when loaded.
    const sum = summarise(f, at);
    if (!sum?.ref) return body;
    const slot = h('div', { class: 'pop-details' }, h('div', { class: 'faint' }, 'Loading details…'));
    const src = body.querySelector('.src');
    body.insertBefore(slot, src);
    loadDetail(sum.ref).then((d) => {
      const e = d ? enrichOf(sum, d) : null;
      if (!e || (!e.desc && !e.rows.length && !e.links.length)) {
        slot.remove();
        return;
      }
      slot.replaceChildren(
        ...(e.desc ? [h('div', { class: 'pdesc' }, cap(e.desc))] : []),
        ...(e.descSource ? [h('div', { class: 'src' }, e.descSource)] : []),
        ...e.rows.map(([k, v]) => kv(k, v)!),
        ...(e.links.length ? [h('div', { class: 'links' }, ...e.links.map(([l, u]) => link(u, l)!))] : []),
      );
    });
    return body;
  }
}

/** The same facts as the click popups, condensed for the bottom bar. */
/** "National · top grade": a heritage site's group and kind. */
function heritageKindLabel(p: Record<string, unknown>): string {
  const t = heritageTierOf(p);
  const tier = HERITAGE_TIERS.find((x) => x.key === t);
  return `${heritageGroupOf(t).label}${tier ? ` · ${tier.label.toLowerCase()}` : ''}`;
}

function summarise(f: MapGeoJSONFeature, at: maplibregl.LngLat): FeatureSummary | null {
  const p = f.properties ?? {};
  const idx = Number.isFinite(Number(p.i)) && p.i !== undefined ? Number(p.i) : null;
  const lid = f.layer.id;
  const facts = (...xs: (string | null | undefined | false)[]) => xs.filter((x): x is string => !!x);
  if (lid === 'heritage-pt') {
    const kind = heritageKindLabel(p);
    return {
      title: p.name || 'Designated place', kind: p.designation ?? kind, colour: heritageGroupOf(heritageTierOf(p)).colour, area: false,
      facts: facts(kind, p.date && `designated ${p.date}`, p.municipality, p.category ?? p.type, p.in_danger && 'in danger'),
      source: p.source ?? '',
      ref: idx !== null ? { layer: 'heritage', i: idx } : undefined,
    };
  }
  if (lid.startsWith('poi-')) {
    return {
      title: p.name || POI_LABEL[p.kind] || 'Point of interest', kind: POI_LABEL[p.kind] ?? p.kind, colour: '#e79a6b', area: false,
      facts: facts(p.ele && fmt.m(Number(p.ele))), source: 'OpenStreetMap',
      ref: idx !== null ? { layer: 'poi', i: idx } : undefined, what: p.kind,
    };
  }
  if (lid === 'special-fill') {
    return {
      title: p.name, kind: SPECIAL_LABEL[p.kind] ?? p.kind, colour: SPECIAL_COLOUR, area: true,
      facts: facts(p.category, p.year && `since ${p.year}`, p.area_km2 && `${fmt.n(Number(p.area_km2))} km²`, p.certifier), source: p.source ?? '',
      ref: idx !== null ? { layer: 'special', i: idx } : undefined,
    };
  }
  if (lid === 'heritage-area-fill') {
    return {
      title: p.name, kind: p.designation ?? 'Heritage district', colour: HERITAGE_AREA_COLOUR, area: true,
      facts: facts(p.date && `designated ${p.date}`, p.municipality), source: p.source ?? '',
      ref: idx !== null ? { layer: 'harea', i: idx } : undefined,
    };
  }
  if (lid === 'indigenous-fill') {
    return {
      title: p.name || 'Indigenous land', kind: 'Indigenous land / reserve', colour: INDIGENOUS_COLOUR, area: true, facts: [], source: 'OpenStreetMap',
      ref: idx !== null ? { layer: 'indigenous', i: idx } : undefined,
    };
  }
  if (baseId(lid) === 'park-fill') {
    return {
      title: p.name || p['name:latin'] || 'Protected area', kind: String(p.class ?? 'protected area').replace(/_/g, ' '), colour: PARK_COLOUR, area: true,
      facts: [], source: 'OpenStreetMap',
      ref: p.name ? { park: { name: String(p.name), lon: at.lng, lat: at.lat } } : undefined,
    };
  }
  return null;
}

/** A range at least 0.02 wide around the auto-fit percentiles (scores bunch up in sparse views). */
function spreadRange(lo: number, hi: number): [number, number] {
  if (hi - lo >= 0.02) return [lo, hi];
  const m = (lo + hi) / 2;
  return [Math.max(0, m - 0.01), Math.min(1, m + 0.01)];
}

/** Point-in-polygon test (lng, lat ring), with a bounding-box pre-check. */
export function inPolygon(poly: [number, number][]): (x: number, y: number) => boolean {
  let w = Infinity, s = Infinity, e = -Infinity, n = -Infinity;
  for (const [x, y] of poly) {
    w = Math.min(w, x); e = Math.max(e, x); s = Math.min(s, y); n = Math.max(n, y);
  }
  return (x, y) => {
    if (x < w || x > e || y < s || y > n) return false;
    let inside = false;
    for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
      const [xi, yi] = poly[i], [xj, yj] = poly[j];
      if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
    }
    return inside;
  };
}
