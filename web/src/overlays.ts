// Designation and stop overlays: GeoJSON fetched on first use, visibility, the heritage level
// filter, and click popups.
import * as maplibregl from 'maplibre-gl';
import { cdfOf } from './ui/scale';
import { Dist } from './roads/stats';
import type { ExpressionSpecification, GeoJSONSource, Map as MLMap, MapGeoJSONFeature } from 'maplibre-gl';
import { HERITAGE_GROUPS, HERITAGE_TIER, HERITAGE_TIERS, LANDMARK_LABELS, OVERLAY_LAYERS, POINT_TILES, POI_STYLE, SIG_LAYERS, landmarkScoreOf, nameOpacityPaint, spacingFilter, heritageGroupOf, heritageTierOf, OVERLAY_SOURCE, baseId, labelKindOf, partIds, type NameScale } from './basemap';
import { OVERLAYS, kindSpacing, labelShown, type AppState, type LabelKind, type OverlayKey } from './state';
import { ver } from './api';
import { hostFor } from './hosts';
import { tasks } from './tasks';
import { withEnglish } from './english';
import { enrichArea, enrichHeritage, enrichPoi, loadDetail, type Detail, type DetailRef, type Enriched } from './details';
import { stopFilterExpr, stopFilterPass } from './stopfilters';
import type { KindQuery, LandmarkItem, LandmarkRequest, LandmarkResponse, TileNames } from './landmarks.worker';
import { NameFader } from './namefade';
import type { LandmarkDots } from './dots';
import { fitPopup } from './popupfit';
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
  // Not what the kind already says: the Spanish registers' designations carry their category
  // ("Bé cultural d'interès nacional (Monument Històric)"), which the details repeat.
  const said = f.kind.toLowerCase();
  for (const x of e.facts) if (!facts.includes(x) && !said.includes(x.toLowerCase())) facts.push(x);
  return { ...f, facts, desc: e.desc ?? f.desc, source: f.source || d?.props?.source || '' };
}

function enrichOf(f: FeatureSummary, d: Detail): Enriched | null {
  if (!f.ref) return null;
  if ('park' in f.ref) return enrichArea(d);
  switch (f.ref.layer) {
    case 'poi': return enrichPoi(f.what ?? '', {}, d);
    case 'heritage': return enrichHeritage(d);
    // The register's area and year (in the summary) rather than the mapped outline's and
    // Wikidata's, which can disagree with them.
    case 'special': return enrichArea(d, { area: f.facts.some((x) => / (km²|ha)$/.test(x)), since: f.facts.some((x) => x.startsWith('since ')) });
    default: return enrichArea(d);
  }
}

const PARK_COLOUR = '#5fb37a';
const SPECIAL_COLOUR = '#b48cff';
const INDIGENOUS_COLOUR = '#e0a060';
const HERITAGE_AREA_COLOUR = '#e7a0ff';

/** Most significant first: heritage level, then how widely it is covered (Wikipedia sitelinks), as
 * the heritage labels are placed (basemap.ts). */
const levelOf = (f: MapGeoJSONFeature) => (f.layer.id === 'whs-fill' || f.layer.id === 'whs-line' ? 1 : f.properties?.level ?? 9);
const significance = (a: MapGeoJSONFeature, b: MapGeoJSONFeature) =>
  levelOf(a) - levelOf(b) || (a.properties?.pt ? 1 : 0) - (b.properties?.pt ? 1 : 0)
  || (b.properties?.sl ?? 0) - (a.properties?.sl ?? 0);

/** Point layers are tested before roads on click; area layers after. */
export const POINT_LAYERS = ['heritage-pt', 'heritage-part', ...Object.keys(OVERLAY_LAYERS).filter((k) => OVERLAY_SOURCE[k]?.startsWith('pois-')).map((k) => `poi-${k}`)];
/** Area layers (parks: the basemap's, and its parts' clones). */
export const areaLayers = () => ['whs-fill', 'whs-line', 'heritage-area-fill', 'special-fill', 'indigenous-fill', ...partIds('park-fill')];
/** World Heritage outlines (layer-whs-shapes.json: n name, c category, i the site's record) as the
 * site's properties, so they show like its dot. */
const whsAsSite = (p: Record<string, any>): Record<string, any> =>
  ({ name: p.n, designation: 'UNESCO World Heritage Site', level: 1, t: p.c === 'Cultural' ? 'w.c' : 'w.n', category: p.c, i: p.i });
const isWhs = (id: string) => id === 'whs-fill' || id === 'whs-line';

/** An overlay's layer file, as the map draws it (dem/layers.py), versioned for the browser cache. */
const layerUrl = (src: string) => `${hostFor('layers')}/api/layer/${src}${ver(`layer-${src}.json`) || ver(`${src}.json`)}`;
/** Point sources: heritage, and the stops & sights per kind (pois-<kind>). */
type PointSource = string;
const isPoints = (src: string): src is PointSource => src === 'heritage' || src.startsWith('pois-');

/** The point sources' map tiles (basemap.ts POINT_TILES, `lmk://<source>/{z}/{x}/{y}`): made by the
 * landmarks worker from its index (landmarks.worker.ts tile), so no other copy of the files is
 * held. Registered before any map asks; the requests wait for the Overlays' worker. */
let tileWorker: Worker | null = null;
let tileSeq = 0;
const tileReplies = new Map<number, (m: Extract<LandmarkResponse, { type: 'tile' }>) => void>();
const tileQueue: LandmarkRequest[] = [];
/** The dots' scale as it stands, for the names' opacity in a tile, and where the tiles' named
 * points go (namefade.ts): set by the Overlays. */
let tileScale: () => NameScale | null = () => null;
let tileNames: (src: string, z: number, x: number, y: number, names: TileNames) => void = () => {};
maplibregl.addProtocol(POINT_TILES, (params) => new Promise((resolve) => {
  const [src, z, x, y] = params.url.replace(`${POINT_TILES}://`, '').split('/').map((v, i) => (i ? parseInt(v, 10) : v)) as [string, number, number, number];
  const id = ++tileSeq;
  tileReplies.set(id, (m) => {
    tileNames(src, z, x, y, m.names);
    resolve({ data: m.data });
  });
  const req: LandmarkRequest = { type: 'tile', id, src, z, x, y, scale: tileScale() };
  if (tileWorker) tileWorker.postMessage(req);
  else tileQueue.push(req);
}));

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
  private whsRequested = false;
  private indexed = new Map<PointSource, 'loading' | 'ready'>();
  private worker = new Worker(new URL('./landmarks.worker.ts', import.meta.url), { type: 'module' });
  private queryId = 0;
  /** The in-view query whose answer is awaited (the ids are shared with the mask requests). */
  private lastQuery = 0;
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

  /** The landmark names, easing with the dots. */
  private names: NameFader;

  constructor(private map: MLMap, private layers: LayersCard, private dots: LandmarkDots) {
    this.names = new NameFader(map, dots);
    tileScale = () => dots.nameScale();
    tileNames = (src, z, x, y, names) => this.names.tile(src, z, x, y, names);
    this.worker.onmessage = (ev: MessageEvent<LandmarkResponse>) => this.onWorker(ev.data);
    tileWorker = this.worker;
    for (const r of tileQueue.splice(0)) this.worker.postMessage(r);
  }

  /** The latest mask request per source (the dots' filters), and the filters it was for. */
  private maskIds = new Map<string, number>();
  private maskKey = '';
  /** Asks the worker which points of these sources pass their filters, for the dots. */
  private requestMasks(srcs: string[]) {
    const s = this.state;
    if (!s) return;
    const kinds: KindQuery[] = [];
    for (const src of srcs) {
      if (this.indexed.get(src) !== 'ready') continue;
      const k = (src === 'heritage' ? 'heritage' : OVERLAYS.find(([o]) => OVERLAY_SOURCE[o] === src)?.[0]) as OverlayKey | undefined;
      if (k) kinds.push(this.kindQuery(k, src, ''));
    }
    if (!kinds.length) return;
    const id = ++this.queryId;
    for (const q of kinds) this.maskIds.set(q.src, id);
    this.worker.postMessage({ type: 'mask', id, kinds } satisfies LandmarkRequest);
  }

  private onWorker(m: LandmarkResponse) {
    if (m.type === 'tile') {
      tileReplies.get(m.id)?.(m);
      tileReplies.delete(m.id);
    } else if (m.type === 'loaded') {
      const src = m.src as PointSource;
      this.indexed.set(src, 'ready');
      tasks.end(`index:${src}`);
      if (m.dots) {
        this.dots.setSource(src, m.dots);
        this.requestMasks([src]);
      }
      if (src === 'heritage') this.layers.setHeritageCounts(m.counts);
      for (const [key] of OVERLAYS) if (OVERLAY_SOURCE[key] === src && this.state?.overlays[key]) this.refreshStatus(key, src);
      this.prominence();
    } else if (m.type === 'count') {
      const k = this.countFor.get(m.id);
      this.countFor.delete(m.id);
      if (k && this.state?.overlays[k]) this.layers.setOverlayStatus(k, m.n === m.of ? fmt.n(m.of) : `${fmt.n(m.n)} of ${fmt.n(m.of)}`);
    } else if (m.type === 'mask') {
      if (this.maskIds.get(m.src) === m.id) this.dots.setMask(m.src, m.vis);
    } else if (m.type === 'result') {
      if (m.id === this.lastQuery) this.applyResult(m);
    }
  }

  apply(s: AppState) {
    const map = this.map;
    this.state = s;
    const spacing = kindSpacing(s.labelDensity, 'landmarks');
    this.names.spacingPx = spacing;
    for (const [k] of OVERLAYS) {
      const on = s.overlays[k];
      const src = OVERLAY_SOURCE[k];
      if (on && src) this.ensure(src, k);
      if (src && isPoints(src)) this.dots.setShown(src, on);
      // Labels follow "Place labels" and their kind's toggle under it.
      for (const id of (OVERLAY_LAYERS[k] ?? []).flatMap(partIds)) {
        const lk = labelKindOf(id) as LabelKind | undefined;
        const show = on && (!lk || labelShown(s, lk));
        if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', show ? 'visible' : 'none');
      }
      // Stops & sights filters, with each layer's own filter.
      // Names appear by interest isolation (the landmarks' label spacing); dots always (sized and
      // faded by significance).
      if (k !== 'heritage') {
        const extra = stopFilterExpr(k, s.stopFilters, s.stopUnknown[k] !== false);
        for (const id of (OVERLAY_LAYERS[k] ?? []).flatMap(partIds)) {
          if (!map.getLayer(id)) continue;
          if (!this.baseFilters.has(id)) this.baseFilters.set(id, map.getFilter(id) ?? null);
          const base = this.baseFilters.get(id) as ExpressionSpecification | null;
          const thin = id.startsWith('poi-') && id.endsWith('-label') ? spacingFilter(spacing) : null;
          const parts = [base, extra, thin].filter((x): x is ExpressionSpecification => !!x);
          map.setFilter(id, parts.length > 1 ? ['all', ...parts] : parts[0] ?? null);
        }
      }
      if (!on) this.layers.setOverlayStatus(k, '');
      else if (src) this.refreshStatus(k, src);
    }
    const shown = HERITAGE_TIERS.map((t) => t.key).filter((k) => !s.heritageOff.includes(k));
    // World Heritage outlines: loaded with the sites, shown with their kind (cultural, natural).
    if (map.getLayer('whs-line')) {
      if (s.overlays.heritage && !this.whsRequested) {
        this.whsRequested = true;
        map.getSource<GeoJSONSource>('whs')?.setData(layerUrl('whs-shapes'));
      }
      const byKind: ExpressionSpecification = ['case', ['in', ['get', 'c'], ['literal', ['Natural', 'Mixed']]], shown.includes('w.n'), shown.includes('w.c')];
      map.setFilter('whs-line', byKind);
      map.setFilter('whs-fill', ['all', ['==', ['get', 'a'], 1], byKind]);
    }
    if (map.getLayer('heritage-pt')) {
      const extra = stopFilterExpr('heritage', s.stopFilters, s.stopUnknown.heritage !== false);
      const levels: ExpressionSpecification = ['in', HERITAGE_TIER, ['literal', shown]];
      // A World Heritage Site in several components: one dot (heritage-pt), and its components
      // small close in (heritage-part, pt).
      const part: ExpressionSpecification = ['has', 'pt'];
      map.setFilter('heritage-pt', ['all', ['!', part], levels, ...(extra ? [extra] : [])]);
      if (map.getLayer('heritage-part')) map.setFilter('heritage-part', ['all', part, levels, ...(extra ? [extra] : [])]);
      map.setFilter('heritage-label', [
        'all',
        ['!', part],
        levels,
        // Names once a site's interest isolation spans the label spacing (older data: by level).
        ['case', ['has', 'mz'], spacingFilter(spacing)!,
          ['any', ['<=', ['get', 'level'], 1], ['all', ['<=', ['get', 'level'], 2], ['>=', ['zoom'], 9]], ['>=', ['zoom'], 13]]],
        ...(extra ? [extra] : []),
      ]);
    }
    // The dots' filters (the worker's masks), again when the filters change.
    const mk = JSON.stringify([s.heritageOff, s.stopFilters, s.stopUnknown]);
    if (mk !== this.maskKey) {
      this.maskKey = mk;
      this.requestMasks([...this.indexed.keys()]);
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
  viewLandmarks: { key: OverlayKey; label: string; colour: string; n: number; best: { name: string; lngLat: [number, number]; layer: string; props: Record<string, any> } | null }[] = [];

  /** The most prominent landmarks in view (optionally of one kind), for the Sights list. */
  topInView(kind: OverlayKey | null, limit = 60): LandmarkItem[] {
    return (kind ? this.topByKind[kind] ?? [] : this.top).slice(0, limit);
  }

  /** The highest named peak in view, as of the last prominence pass (indexes the stops & sights if
   * not yet). */
  summitInView(): { name: string; ele: number; lngLat: [number, number] } | null {
    if (!this.summitsRequested && !this.held) {
      this.summitsRequested = true;
      this.worker.postMessage({ type: 'summits', url: layerUrl('summits') } satisfies LandmarkRequest);
    }
    return this.summit;
  }
  private summitsRequested = false;
  private held = true;

  /** Start loading the overlays shown (see ensure). */
  release() {
    if (!this.held) return;
    this.held = false;
    if (this.state) this.apply(this.state);
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
      type: 'query', id: (this.lastQuery = ++this.queryId), outline: this.viewOutline?.() ?? [], bounds: [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()],
      balance: s.landmarks.balance, kinds, top: 60, ranks: [s.landmarks.top[0], s.landmarks.top[1]],
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
    const dist = r.n ? new Dist(0, 1, r.hist, r.n) : null;
    // Auto: from the top[0]-th best landmark in view to the top[1]-th (fewer in view: the least
    // prominent of them), one bar for the whole view (the worker's ranks).
    const range: [number, number] = !lm.auto || !r.atRanks ? lm.range : spreadRange(r.atRanks[0], r.atRanks[1]);
    const cdf = lm.equalize ? cdfOf(dist, range) : null;
    const eqStops: [number, number][] | null = cdf
      ? Array.from({ length: 33 }, (_, i) => [range[0] + (i / 32) * (range[1] - range[0]), cdf[Math.round((i / 32) * 255)] / 255] as [number, number])
      : null;
    // The dots ease to the new scale (dots.ts, on the GPU).
    this.dots.setScale({
      range, eq: eqStops ? eqStops.map(([, u]) => u) : null, lowFade: lm.lowFade, lowSpan: lm.lowSpan,
      threshold: lm.threshold, balance: lm.balance, emphasis: s.poiEmphasis, opacity: s.poiOpacity,
    });
    // Their names ease with them (namefade.ts: feature states, not a paint expression of the scale,
    // which MapLibre snaps and lays the whole source out again for); Label opacity is the layers'.
    this.names.kick();
    for (const id of SIG_LAYERS) {
      const lid = LANDMARK_LABELS[id];
      const k = (id === 'heritage-pt' ? 'heritage' : id.slice(4)) as OverlayKey;
      if (!map.getLayer(lid) || !s.overlays[k]) continue; // hidden: styled when shown (apply → prominence)
      const v = nameOpacityPaint(s.labelOpacity);
      const key = `${lid}|text-opacity`, j = JSON.stringify(v);
      if (this.painted.get(key) === j) continue;
      this.painted.set(key, j);
      map.setPaintProperty(lid, 'text-opacity', v);
    }
    this.onScale?.(dist, range, cdf);
    this.onView?.();
  }

  /** Fly to a landmark and open its popup. */
  /** A landmark picked from a list (Sights, In view): its popup, the map staying where it is. */
  select(x: { lngLat: [number, number]; layer: string; props?: Record<string, any> }) {
    const at = maplibregl.LngLat.convert(x.lngLat);
    if (x.props) this.show([{ layer: { id: x.layer }, properties: x.props } as unknown as MapGeoJSONFeature], at, 1);
    else this.click(this.map.project(at), [x.layer]);
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
   * indexed in the landmarks worker instead, which makes their tiles. */
  private ensure(src: string, k: OverlayKey) {
    // At start-up, the overlay files wait for the roads in view (release()): parsing them competes
    // with the road tiles for the CPU, and the roads are what the map is for.
    if (this.held) return;
    if (isPoints(src)) return this.ensureIndex(src);
    if (this.sourced.has(src)) return;
    this.sourced.add(src);
    this.map.getSource<GeoJSONSource>(src)?.setData(layerUrl(src));
    this.refreshStatus(k, src);
  }

  private ensureIndex(src: PointSource) {
    if (this.indexed.has(src)) return;
    this.indexed.set(src, 'loading');
    for (const [key] of OVERLAYS) if (OVERLAY_SOURCE[key] === src && this.state?.overlays[key]) this.layers.setOverlayStatus(key, '', true);
    tasks.begin(`index:${src}`, `${src === 'heritage' ? 'Heritage sites' : OVERLAYS.find(([k]) => OVERLAY_SOURCE[k] === src)?.[1] ?? 'Stops'} list`, 'downloading and indexing for the in-view lists and counts');
    this.worker.postMessage({ type: 'load', src, url: layerUrl(src) } satisfies LandmarkRequest);
  }

  private ensureSummary() {
    if (this.summary || this.summaryLoading) return;
    this.summaryLoading = true;
    fetch(layerUrl('summary'))
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
    let fs = this.onDots(map.queryRenderedFeatures([[p.x - pad, p.y - pad], [p.x + pad, p.y + pad]], { layers: ids }), p);
    if (!fs.length) return false;
    // Every feature at the spot (sites often share one: a national historic site and a heritage
    // building in it): markers in the hover's order (rank), areas by heritage level.
    fs = ids.every((id) => POINT_LAYERS.includes(id)) ? this.rank(fs, p) : fs.sort(significance);
    const seen = new Set<string>();
    const uniq = fs.filter((f) => {
      const k = isWhs(f.layer.id) ? `whs|${f.properties?.id}` : `${f.layer.id}|${f.properties?.i ?? ''}|${f.properties?.name ?? ''}`;
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
      const fs = this.rank(this.onDots(map.queryRenderedFeatures([[pt.x - 5, pt.y - 5], [pt.x + 5, pt.y + 5]], { layers: pts }), pt), pt);
      point = fs.length ? summarise(fs[0], map.unproject([pt.x, pt.y])) : null;
      // Others at the same spot (click lists them all).
      const names = new Set(fs.slice(1).map((f) => cap(f.properties?.name ?? '')).filter((n) => n && n !== cap(point?.title)));
      if (point && names.size) point.also = [...names];
    }
    const ars = vis(areaLayers());
    const seen = new Set<string>();
    const at = map.unproject([pt.x, pt.y]);
    // World Heritage lines and areas: a few pixels' slack, so a canal or wall is easy to find.
    const whs = vis(['whs-line', 'whs-fill']);
    const whsHit = whs.length ? map.queryRenderedFeatures([[pt.x - 4, pt.y - 4], [pt.x + 4, pt.y + 4]], { layers: whs }) : [];
    const areas = [...whsHit, ...(ars.length ? map.queryRenderedFeatures([pt.x, pt.y], { layers: ars }) : [])]
      .map((f) => summarise(f, at))
      .filter((f): f is FeatureSummary => !!f && !seen.has(f.title + f.kind) && !!seen.add(f.title + f.kind));
    return { point, areas };
  }

  /** The World Heritage line (a canal, a wall; not an area's outline) nearest a screen point
   * within `tol` px, and how near: a click on it opens the site unless a road is nearer. */
  whsLineAt(pt: { x: number; y: number }, tol = 4): { px: number } | null {
    const map = this.map;
    if (!map.getLayer('whs-line') || map.getLayoutProperty('whs-line', 'visibility') === 'none') return null;
    let best: number | null = null;
    for (const f of map.queryRenderedFeatures([[pt.x - tol, pt.y - tol], [pt.x + tol, pt.y + tol]], { layers: ['whs-line'] })) {
      const g = f.geometry;
      if (f.properties?.a === 1 || (g.type !== 'LineString' && g.type !== 'MultiLineString')) continue;
      for (const line of (g.type === 'LineString' ? [g.coordinates] : g.coordinates) as [number, number][][]) {
        let a = map.project(line[0]);
        for (let i = 1; i < line.length; i++) {
          const b = map.project(line[i]);
          const dx = b.x - a.x, dy = b.y - a.y, l2 = dx * dx + dy * dy;
          const t = l2 ? Math.max(0, Math.min(1, ((pt.x - a.x) * dx + (pt.y - a.y) * dy) / l2)) : 0;
          const d = Math.hypot(pt.x - (a.x + t * dx), pt.y - (a.y + t * dy));
          if (best === null || d < best) best = d;
          a = b;
        }
      }
    }
    return best !== null && best <= tol ? { px: best } : null;
  }

  /** Markers under a screen point, the one it points at first: a dot the point is on before one
   * it is only near (within the slack), then the most prominent (the dots' score, fame and rarity,
   * as they are sized: the biggest dot there), then the designation level and Wikipedia coverage;
   * the parts of a World Heritage Site after sites. */
  private rank(fs: MapGeoJSONFeature[], pt: { x: number; y: number }): MapGeoJSONFeature[] {
    const balance = this.state?.landmarks.balance ?? 0.5;
    return fs.map((f) => {
      const p = f.properties ?? {};
      const q = this.map.project((f.geometry as unknown as { coordinates: [number, number] }).coordinates);
      const r = SIG_LAYERS.includes(f.layer.id) ? this.dots.radiusOf(f.layer.id, p) : 3;
      return {
        f, on: Math.hypot(q.x - pt.x, q.y - pt.y) <= r + 1, part: !!p.pt,
        score: landmarkScoreOf(Number(p.fa) || 0, p.ia == null ? 20000 : Number(p.ia), balance), level: levelOf(f), sl: Number(p.sl) || 0,
      };
    }).sort((a, b) => Number(b.on) - Number(a.on) || Number(a.part) - Number(b.part) || b.score - a.score || a.level - b.level || b.sl - a.sl)
      .map((x) => x.f);
  }

  /** Hits on the landmark dots' invisible circles (drawn at the largest radius) that fall on a
   * dot as drawn now (dots.ts radiusOf), with a few pixels' slack; other layers' as they are. */
  private onDots(fs: MapGeoJSONFeature[], pt: { x: number; y: number }): MapGeoJSONFeature[] {
    return fs.filter((f) => {
      if (!SIG_LAYERS.includes(f.layer.id)) return true;
      const q = this.map.project((f.geometry as unknown as { coordinates: [number, number] }).coordinates);
      return Math.hypot(q.x - pt.x, q.y - pt.y) <= this.dots.radiusOf(f.layer.id, f.properties ?? {}) + 4;
    });
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
    // Kept inside the map: the side of the point with room, capped and scrolling (popupfit.ts).
    const popup = new maplibregl.Popup({ closeButton: true, maxWidth: '320px', className: 'dark-pop', offset: 8 });
    const fit = fitPopup(this.map, popup, box);
    this.popup = popup.setLngLat(at).setDOMContent(fit.el).addTo(this.map);
    fit.fit();
  }

  private section(f: MapGeoJSONFeature, at: maplibregl.LngLat): HTMLElement | null {
    // A World Heritage outline shows its site, as the site's dot does.
    const p = isWhs(f.layer.id) ? whsAsSite(f.properties ?? {}) : f.properties ?? {};
    const lid = isWhs(f.layer.id) ? 'heritage-pt' : f.layer.id;
    const body = h('div', { class: 'pop' });
    const put = (...xs: (Node | string | null)[]) => body.append(...(xs.filter((x) => x !== null) as (Node | string)[]));
    const link = (url: string | undefined, label: string) =>
      url ? h('a', { href: url, target: '_blank', rel: 'noopener' }, `${label} ↗`) : null;
    const kv = (k: string, v: unknown) => (v === undefined || v === null || v === '' ? null : h('div', { class: 'kv' }, h('span', {}, k), h('b', {}, String(v))));
    if (lid === 'heritage-pt' || lid === 'heritage-part') {
      const dot = h('span', { class: 'dot' });
      dot.style.background = heritageGroupOf(heritageTierOf(p)).colour;
      // The site's record (dates, authority, links, source) comes with its details (props).
      const rows = h('div'), links = h('div', { class: 'links' }), src = h('div', { class: 'src' }), notice = h('div', { class: 'src' });
      const part = lid === 'heritage-part';
      put(h('div', { class: 'ttl' }, named(part ? p.cn : p.name, part ? {} : p, at) || 'Designated place'),
        part ? h('div', { class: 'sub' }, dot, partOf(p)) : null,
        h('div', { class: 'sub' }, part ? '' : dot, `${p.designation ?? ''}`, h('span', { class: 'faint' }, ` · ${heritageKindLabel(p)}`)),
        rows, links, src, notice);
      const fill = (q: Record<string, any>) => {
        rows.replaceChildren(...[
          part ? null : kv('Components', p.np ? `${fmt.n(Number(p.np))}, shown close in` : null),
          kv('Designated', q.date), kv('Authority', q.authority), kv('Municipality', q.municipality),
          kv('Category', String(p.designation ?? '').includes(q.category ?? q.type ?? '\u0000') ? null : q.category ?? q.type),
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
        h('div', { class: 'ttl' }, named(p.name, p, at) || POI_LABEL[p.kind] || 'Point of interest'),
        h('div', { class: 'sub' }, POI_LABEL[p.kind] ?? p.kind),
        kv('Elevation', p.ele ? fmt.m(Number(p.ele)) : null),
        h('div', { class: 'src' }, 'Source: OpenStreetMap'),
      );
    } else if (lid === 'special-fill') {
      put(
        h('div', { class: 'ttl' }, named(p.name, p, at)),
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
        h('div', { class: 'ttl' }, named(p.name, p, at)),
        h('div', { class: 'sub' }, p.designation ?? 'Heritage district'),
        kv('Designated', p.date),
        kv('Municipality', p.municipality),
        kv('By-law', p.bylaw),
        kv('Properties', p.properties_count),
        h('div', { class: 'links' }, link(p.url, 'Official record')),
        h('div', { class: 'src' }, `Source: ${p.source ?? ''}`),
      );
    } else if (lid === 'indigenous-fill') {
      put(h('div', { class: 'ttl' }, named(p.name, p, at) || 'Indigenous land'), h('div', { class: 'sub' }, 'Indigenous land / reserve'), h('div', { class: 'src' }, 'Boundary: OpenStreetMap'));
    } else if (baseId(lid) === 'park-fill') {
      put(
        h('div', { class: 'ttl' }, named(p.name || p['name:latin'], p, at) || 'Protected area'),
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

/** A World Heritage Site's component: "Part of <site>" (the canal of the Rideau Canal: a component). */
const partOf = (p: Record<string, any>): string => (p.cn && p.cn !== p.name ? `Part of ${cap(p.name)}` : 'World Heritage component');

/** The same facts as the click popups, condensed for the bottom bar. */
/** "National · top grade": a heritage site's group and kind. */
function heritageKindLabel(p: Record<string, unknown>): string {
  const t = heritageTierOf(p);
  const tier = HERITAGE_TIERS.find((x) => x.key === t);
  return `${heritageGroupOf(t).label}${tier ? ` · ${tier.label.toLowerCase()}` : ''}`;
}

/** A landmark's details record (its OSM object among them), from its layer and properties. */
export function landmarkRef(layer: string, p: Record<string, any>): DetailRef | null {
  const i = Number(p.i);
  if (p.i === undefined || !Number.isFinite(i)) return null;
  return layer === 'heritage-pt' || layer === 'heritage-part' ? { layer: 'heritage', i } : layer.startsWith('poi-') ? { layer: 'poi', i } : null;
}

/** A feature's name with its English in parentheses (english.ts): its own (`en`; heritage sites'
 * `name_en`; a basemap feature's `name:en`), else the translation for where it is. */
function named(name: unknown, p: Record<string, any>, at: maplibregl.LngLat | [number, number]): string {
  return name ? cap(withEnglish(String(name), at, p.en ?? p.name_en ?? p['name:en'])) : '';
}

function summarise(f: MapGeoJSONFeature, at: maplibregl.LngLat): FeatureSummary | null {
  const p = f.properties ?? {};
  const idx = Number.isFinite(Number(p.i)) && p.i !== undefined ? Number(p.i) : null;
  const lid = f.layer.id;
  const facts = (...xs: (string | null | undefined | false)[]) => xs.filter((x): x is string => !!x);
  if (lid === 'heritage-pt' || lid === 'heritage-part') {
    const kind = heritageKindLabel(p);
    const part = lid === 'heritage-part';
    return {
      title: named(part ? p.cn : p.name, part ? {} : p, at) || 'Designated place', kind: part ? partOf(p) : p.designation ?? kind,
      colour: heritageGroupOf(heritageTierOf(p)).colour, area: false,
      facts: facts(p.category ?? p.type, p.in_danger && 'in danger'),
      source: p.source ?? '',
      ref: idx !== null ? { layer: 'heritage', i: idx } : undefined,
    };
  }
  if (lid.startsWith('poi-')) {
    return {
      title: named(p.name, p, at) || POI_LABEL[p.kind] || 'Point of interest', kind: POI_LABEL[p.kind] ?? p.kind, colour: '#e79a6b', area: false,
      facts: facts(p.ele && fmt.m(Number(p.ele))), source: 'OpenStreetMap',
      ref: idx !== null ? { layer: 'poi', i: idx } : undefined, what: p.kind,
    };
  }
  if (lid === 'special-fill') {
    return {
      title: named(p.name, p, at), kind: SPECIAL_LABEL[p.kind] ?? p.kind, colour: SPECIAL_COLOUR, area: true,
      // (the certifier only where the kind doesn't name it: not "UNESCO" after "UNESCO Biosphere Reserve")
      facts: facts(p.category, p.year && `since ${p.year}`, p.area_km2 && `${fmt.n(Number(p.area_km2))} km²`,
        p.certifier && !(SPECIAL_LABEL[p.kind] ?? '').includes(p.certifier) && p.certifier), source: p.source ?? '',
      ref: idx !== null ? { layer: 'special', i: idx } : undefined,
    };
  }
  if (lid === 'whs-line' || lid === 'whs-fill') {
    return {
      title: named(p.n, p, at), kind: 'UNESCO World Heritage Site', colour: HERITAGE_GROUPS[0].colour, area: true,
      facts: facts(p.c), source: 'Outline: OpenStreetMap; site: UNESCO World Heritage Centre',
      ref: idx !== null ? { layer: 'heritage', i: idx } : undefined,
    };
  }
  if (lid === 'heritage-area-fill') {
    return {
      title: named(p.name, p, at), kind: p.designation ?? 'Heritage district', colour: HERITAGE_AREA_COLOUR, area: true,
      facts: facts(p.municipality), source: p.source ?? '',
      ref: idx !== null ? { layer: 'harea', i: idx } : undefined,
    };
  }
  if (lid === 'indigenous-fill') {
    return {
      title: named(p.name, p, at) || 'Indigenous land', kind: 'Indigenous land / reserve', colour: INDIGENOUS_COLOUR, area: true, facts: [], source: 'OpenStreetMap',
      ref: idx !== null ? { layer: 'indigenous', i: idx } : undefined,
    };
  }
  if (baseId(lid) === 'park-fill') {
    return {
      title: named(p.name || p['name:latin'], p, at) || 'Protected area', kind: String(p.class ?? 'protected area').replace(/_/g, ' '), colour: PARK_COLOUR, area: true,
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
