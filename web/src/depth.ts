// Depth under the water, seas and lakes (docs/plan.md §6, Depth), as two layers, each with its
// own switch (Layers → Depth):
//
// 1. Depth colour and floor shading: a colour-relief and a hillshade layer over raster-dem
//    sources that depth.worker.ts makes from the server's depth tiles (crates/server/src/depth.rs:
//    a tile the layer lacks comes from its nearest ancestor), masked by the basemap's own water at
//    the display's resolution.
// 2. Depth contours with labels: vector tiles the same worker makes (maplibre-contour's
//    isolines), clipped to the water.
// All go over the water fill and under the coast shading and everything else, draped on the 3D
// terrain like the water (which stays flat at the surface). Depth is below the local water
// surface: mean sea level at sea, the lake's surface on lakes; estimated depths look as measured
// ones do (the readout says which they are). While a depth layer shows, the map names the depth's
// sources in view and every notice their licences ask for (the catalog's `depth`).
import * as maplibregl from 'maplibre-gl';
import type { ExpressionSpecification, Map as MLMap } from 'maplibre-gl';
import { onVersions, ver } from './api';
import { hostFor } from './hosts';
import { attribution } from './depthattrib';
import { depthColour, hdepth, hexRgb, HBINS, rgba, scaleFns, DEPTH_DEFAULTS, type DepthState } from './depthlook';
import { h } from './ui/dom';
import type { DepthMsg, DepthReply, Probe } from './depth.worker';

type RGB = [number, number, number];

/** A depth source as the catalog names it (pipeline::depth::grids). */
export interface DepthSource {
  code: number;
  id: string;
  label: string;
  datum: string;
  water: 'sea' | 'lake';
  notice?: string | null;
}

const CLASSES = ['', 'measured', 'interpolated', 'estimated', 'filled to the shore'];

export class Depth {
  private s: DepthState = DEPTH_DEFAULTS;
  /** The catalog has depth (its layers) and its sources. */
  private has = false;
  private sources: DepthSource[] = [];
  private workers: Worker[] = [];
  private seq = 0;
  private waiting = new Map<number, { resolve: (r: DepthReply) => void; reject: (e: Error) => void }>();
  private added = false;
  /** The fitted domain (m) of the "fit to view" scale. */
  private fit: [number, number] = [1, 100];
  private viewTimer = 0;
  /** The sources with depth in view (their codes). */
  /** The sources with depth in view (their codes); null while that isn't known (moving, before
   * the first sample, too many tiles to sample): then every source counts, so no notice a
   * licence asks for is ever missing while its depths could show. */
  private inView: Set<number> | null = null;
  private hoverAt: maplibregl.LngLat | null = null;
  private hoverBusy = false;
  /** The cursor's readout and the on-map attribution while a depth layer is on. */
  readonly readout: HTMLElement;
  private attrib: HTMLElement;
  /** Told when the fitted domain changes (the panel's legend). */
  onFit: () => void = () => {};

  constructor(private map: MLMap) {
    this.readout = h('div', { class: 'depth-readout', hidden: true });
    this.attrib = h('div', { class: 'depth-attrib', hidden: true });
    map.getContainer().append(this.readout, this.attrib);
    map.on('moveend', () => this.scheduleView());
    map.on('mousemove', (e) => {
      this.hoverAt = e.lngLat;
      if (!this.hoverBusy) void this.hover();
    });
    map.getCanvas().addEventListener('mouseleave', () => {
      this.hoverAt = null;
      this.readout.hidden = true;
    });
    onVersions((f) => f.startsWith('depth'), () => this.initWorkers());
  }

  /** The catalog's depth: whether it has the layers, and its sources. */
  setCatalog(layers: Record<string, unknown> | undefined, depth: { sources?: DepthSource[] } | null | undefined) {
    const has = !!layers && 'depth' in layers && 'depth-class' in layers;
    this.sources = depth?.sources ?? [];
    this.inView = null;
    if (has !== this.has) {
      this.has = has;
      this.apply(this.s);
    }
    this.renderAttrib();
  }

  /** Whether the catalog has depth. */
  get available() {
    return this.has;
  }

  /** The domain (m of depth) the colour ramp spans now. */
  domain(s = this.s): [number, number] {
    return s.scale === 'fit' ? this.fit : [0, s.max];
  }

  /** The sources by code (the panel's notes). */
  source(code: number): DepthSource | undefined {
    return this.sources.find((s) => s.code === code);
  }

  // ---- the worker and its protocol --------------------------------------------------------------

  private initWorkers() {
    const msg: DepthMsg = {
      type: 'init',
      base: (this.map.getSource('base') as maplibregl.VectorTileSource | undefined)?.tiles?.[0] ?? `${hostFor('base')}/tiles/base/{z}/{x}/{y}`,
      depth: `${hostFor('layers')}/tiles/depth/{z}/{x}/{y}${ver('depth.tiles')}`,
      cls: `${hostFor('layers')}/tiles/depth-class/{z}/{x}/{y}${ver('depth-class.tiles')}`,
    };
    for (const w of this.workers) w.postMessage(msg);
  }

  private setupProtocol() {
    if (this.workers.length) return;
    for (let i = 0; i < 2; i++) {
      const w = new Worker(new URL('./depth.worker.ts', import.meta.url), { type: 'module' });
      w.onmessage = (ev: MessageEvent<DepthReply>) => {
        const p = this.waiting.get(ev.data.id);
        if (!p) return;
        this.waiting.delete(ev.data.id);
        if (ev.data.error) p.reject(new Error(ev.data.error));
        else p.resolve(ev.data);
      };
      this.workers.push(w);
    }
    this.initWorkers();
    maplibregl.addProtocol('depth', async (params, abort) => {
      const m = /^depth:\/\/(\w+)\/(\d+)\/(\d+)\/(\d+)\??(.*)$/.exec(params.url);
      if (!m) throw new Error(`depth: ${params.url}`);
      const [z, x, y] = [+m[2], +m[3], +m[4]];
      const q = Object.fromEntries(new URLSearchParams(m[5]));
      const r = await this.ask({ type: 'tile', id: 0, kind: m[1] as 'relief', z, x, y, q }, abort.signal, this.workerFor(z, x, y));
      return { data: r.data! };
    });
  }

  /** Each tile to one worker (its composite is reused by the tile's other variants there). */
  private workerFor(z: number, x: number, y: number) {
    return this.workers[(x + y + z) % this.workers.length];
  }

  private ask(msg: DepthMsg, signal?: AbortSignal, w = this.workers[0]): Promise<DepthReply> {
    const id = ++this.seq;
    (msg as { id: number }).id = id;
    return new Promise((resolve, reject) => {
      this.waiting.set(id, { resolve, reject });
      signal?.addEventListener('abort', () => {
        w.postMessage({ type: 'cancel', id } satisfies DepthMsg);
        this.waiting.delete(id);
        reject(new DOMException('aborted', 'AbortError'));
      });
      w.postMessage(msg);
    });
  }

  private urls(s: DepthState) {
    return {
      relief: 'depth://relief/{z}/{x}/{y}',
      shade: 'depth://shade/{z}/{x}/{y}',
      contour: `depth://contour/{z}/{x}/{y}?m=${s.interval}&f=${s.fixed}&k=${s.index}`,
    };
  }

  // ---- the layers -------------------------------------------------------------------------------

  /** Under the coast shading and the rivers drawn as lines: just over the water fill. */
  private before(): string | undefined {
    for (const id of ['coast-shade', 'waterway']) if (this.map.getLayer(id)) return id;
    return undefined;
  }

  private addLayers() {
    if (this.added) return;
    this.setupProtocol();
    const m = this.map, u = this.urls(this.s);
    const dem = { type: 'raster-dem' as const, tileSize: 512, maxzoom: 16, encoding: 'terrarium' as const };
    m.addSource('depth-relief', { ...dem, tiles: [u.relief] });
    m.addSource('depth-shade', { ...dem, tiles: [u.shade] });
    m.addSource('depth-contours', { type: 'vector', maxzoom: 16, tiles: [u.contour] });
    const b = this.before();
    m.addLayer({ id: 'depth-relief', type: 'color-relief', source: 'depth-relief', paint: { 'color-relief-opacity': this.s.opacity, resampling: 'linear' } as never }, b);
    m.addLayer({ id: 'depth-shade', type: 'hillshade', source: 'depth-shade', paint: { 'hillshade-illumination-anchor': 'map' } as never }, b);
    const lb = m.getLayer('boundary-county') ? 'boundary-county' : undefined;
    m.addLayer({ id: 'depth-contour', type: 'line', source: 'depth-contours', 'source-layer': 'c', filter: ['==', ['get', 'idx'], 0], layout: { 'line-join': 'round', 'line-cap': 'round' }, paint: {} }, lb);
    m.addLayer({ id: 'depth-contour-index', type: 'line', source: 'depth-contours', 'source-layer': 'c', filter: ['==', ['get', 'idx'], 1], layout: { 'line-join': 'round', 'line-cap': 'round' }, paint: {} }, lb);
    const sb = m.getLayer('place-minor') ? 'place-minor' : undefined;
    m.addLayer({
      id: 'depth-contour-label', type: 'symbol', source: 'depth-contours', 'source-layer': 'c', filter: ['==', ['get', 'idx'], 1],
      layout: {
        'symbol-placement': 'line', 'text-field': ['concat', ['number-format', ['get', 'd'], { 'max-fraction-digits': 1 }], ' m'],
        'text-font': ['Noto Sans Regular'], 'text-size': 10, 'text-padding': 16, 'symbol-spacing': 280,
        'text-pitch-alignment': 'viewport', 'text-rotation-alignment': 'map', 'text-max-angle': 40,
      },
      paint: { 'text-halo-color': '#0b0e13', 'text-halo-width': 1.3 },
    }, sb);
    this.added = true;
  }

  /** The settings `s` on the map (the style must be loaded). */
  apply(s: DepthState) {
    const before = this.s;
    this.s = s;
    const m = this.map;
    const on = this.has && (s.on || s.contours);
    if (!on && !this.added) return this.renderAttrib();
    if (on) this.addLayers();
    const u = this.urls(s), ou = this.urls(before);
    if (u.contour !== ou.contour) (m.getSource('depth-contours') as maplibregl.VectorTileSource | undefined)?.setTiles([u.contour]);
    const vis = (id: string, v: boolean) => m.getLayer(id) && m.setLayoutProperty(id, 'visibility', v ? 'visible' : 'none');
    vis('depth-relief', this.has && s.on);
    vis('depth-shade', this.has && s.on && s.shade > 0);
    vis('depth-contour', this.has && s.contours);
    vis('depth-contour-index', this.has && s.contours);
    vis('depth-contour-label', this.has && s.contours && s.labels);
    if (this.has && s.on) {
      m.setPaintProperty('depth-relief', 'color-relief-color', this.ramp(s), { validate: false });
      m.setPaintProperty('depth-relief', 'color-relief-opacity', s.opacity);
      const op = s.opacity;
      m.setPaintProperty('depth-shade', 'hillshade-method', 'combined' as never);
      m.setPaintProperty('depth-shade', 'hillshade-exaggeration', s.shade);
      m.setPaintProperty('depth-shade', 'hillshade-illumination-direction', s.light);
      m.setPaintProperty('depth-shade', 'hillshade-illumination-altitude', 40);
      m.setPaintProperty('depth-shade', 'hillshade-shadow-color', `rgba(2,6,14,${0.75 * op})`);
      m.setPaintProperty('depth-shade', 'hillshade-highlight-color', `rgba(190,220,245,${0.28 * op})`);
      m.setPaintProperty('depth-shade', 'hillshade-accent-color', `rgba(0,0,0,${0.3 * op})`);
    }
    if (this.has && s.contours) {
      const w = s.weight;
      for (const id of ['depth-contour', 'depth-contour-index']) m.setPaintProperty(id, 'line-color', s.colour);
      m.setPaintProperty('depth-contour', 'line-opacity', s.lineOpacity * 0.7);
      m.setPaintProperty('depth-contour-index', 'line-opacity', Math.min(1, s.lineOpacity * 1.25));
      m.setPaintProperty('depth-contour', 'line-width', ['interpolate', ['linear'], ['zoom'], 6, 0.5 * w, 12, 0.8 * w, 16, 1.1 * w] as ExpressionSpecification);
      m.setPaintProperty('depth-contour-index', 'line-width', ['interpolate', ['linear'], ['zoom'], 6, 1.0 * w, 12, 1.6 * w, 16, 2.2 * w] as ExpressionSpecification);
      m.setPaintProperty('depth-contour-label', 'text-color', s.colour);
      m.setPaintProperty('depth-contour-label', 'text-opacity', Math.min(1, s.lineOpacity * 1.5));
    }
    this.renderAttrib();
    this.scheduleView();
  }

  /** A palette shown on the map before it's chosen (the panel's ramp list); null: the chosen one. */
  preview(palette: string | null) {
    if (!this.added || !this.has || !this.s.on) return;
    this.map.setPaintProperty('depth-relief', 'color-relief-color', this.ramp({ ...this.s, palette: palette ?? this.s.palette }), { validate: false });
  }

  // ---- the colour ramp --------------------------------------------------------------------------

  private ramp(s: DepthState): ExpressionSpecification {
    const [lo, hi] = this.domain(s);
    const [, inv] = scaleFns(s.scale === 'linear' ? 'linear' : 'log', Math.max(0, lo), Math.max(lo + 0.5, hi));
    const stops: [number, string][] = [];
    // Deeper than the domain: its deepest colour; then the domain, 48 steps; then 0 m.
    stops.push([-11500, rgba(depthColour(s.palette, 1))]);
    const n = 48;
    for (let i = n; i >= 0; i--) {
      const t = i / n;
      stops.push([-inv(t), rgba(depthColour(s.palette, t))]);
    }
    if (lo > 0) stops.push([-0.001, rgba(depthColour(s.palette, 0))]);
    // Drying heights (between mean sea level and the shore), then nothing (land, no data).
    if (s.dry) {
      const dc = hexRgb(s.dryColour);
      stops.push([0.001, rgba(dc, 0.85)], [3, rgba(dc.map((v) => Math.min(1, v * 1.15)) as RGB, 0.85)], [20, rgba(dc.map((v) => Math.min(1, v * 1.3)) as RGB, 0.85)]);
    } else stops.push([0.001, rgba(depthColour(s.palette, 0))], [20, rgba(depthColour(s.palette, 0))]);
    stops.push([20.01, 'rgba(0,0,0,0)'], [13000, 'rgba(0,0,0,0)']);
    const seen = new Set<number>();
    const expr: unknown[] = ['interpolate', ['linear'], ['elevation']];
    for (const [v, c] of stops.sort((a, b) => a[0] - b[0])) {
      const k = Math.round(v * 1000) / 1000;
      if (seen.has(k)) continue;
      seen.add(k);
      expr.push(k, c);
    }
    return expr as ExpressionSpecification;
  }

  // ---- the view: the fitted scale and the sources in it ---------------------------------------------

  private scheduleView() {
    if (!this.has || !(this.s.on || this.s.contours)) return;
    this.inView = null;
    this.renderAttrib();
    clearTimeout(this.viewTimer);
    this.viewTimer = window.setTimeout(() => void this.view(), 250);
  }

  /** The tiles in view at the map's zoom (none past 48). */
  private viewTiles(): [number, number, number][] {
    const m = this.map, z = Math.max(0, Math.min(16, Math.floor(m.getZoom())));
    const b = m.getBounds();
    const n = 2 ** z;
    const tx = (lng: number) => Math.floor(((lng + 180) / 360) * n);
    const ty = (lat: number) => {
      const s = Math.sin((Math.max(-85, Math.min(85, lat)) * Math.PI) / 180);
      return Math.floor((0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * n);
    };
    const tiles: [number, number, number][] = [];
    for (let y = Math.max(0, ty(b.getNorth())); y <= Math.min(n - 1, ty(b.getSouth())); y++)
      for (let x = tx(b.getWest()); x <= tx(b.getEast()); x++) tiles.push([z, ((x % n) + n) % n, y]);
    return tiles.length > 48 ? [] : tiles;
  }

  /** The depths in view: their sources (the attribution), and with "fit to view" the scale's
   * domain, their 2nd to 98th percentile. */
  private async view() {
    if (!this.workers.length) return;
    const tiles = this.viewTiles();
    // (Too many to sample: every source counts, `inView` left unknown.)
    if (!tiles.length) return;
    const halves = this.workers.map((w) => tiles.filter((t) => this.workerFor(...t) === w));
    const rs = await Promise.all(halves.map((ts, i) => (ts.length ? this.ask({ type: 'hist', id: 0, tiles: ts }, undefined, this.workers[i]) : Promise.resolve({ id: 0, hist: new Array(HBINS).fill(0), codes: [] } as DepthReply))));
    this.inView = new Set(rs.flatMap((r) => r.codes ?? []));
    this.renderAttrib();
    if (this.s.scale !== 'fit') return;
    const hst = new Array(HBINS).fill(0);
    for (const r of rs) r.hist?.forEach((v, i) => (hst[i] += v));
    const tot = hst.reduce((a, b2) => a + b2, 0);
    if (tot < 50) return;
    const q = (p: number) => {
      let acc = 0;
      for (let i = 0; i < HBINS; i++) {
        acc += hst[i];
        if (acc >= p * tot) return hdepth(i + 0.5);
      }
      return hdepth(HBINS);
    };
    const lo = Math.max(0, q(0.02) * 0.9), hi = Math.max(lo + 1, q(0.98) * 1.05);
    if (Math.abs(Math.log(hi / this.fit[1])) < 0.05 && Math.abs(lo - this.fit[0]) < 0.3) return;
    this.fit = [lo, hi];
    if (this.map.getLayer('depth-relief')) this.map.setPaintProperty('depth-relief', 'color-relief-color', this.ramp(this.s), { validate: false });
    this.onFit();
  }

  /** The depth layers' on-map attribution: the sources in view, "not for navigation", and every
   * notice a licence asks to be shown with its data, whenever a depth layer is on. */
  private renderAttrib() {
    const on = this.has && (this.s.on || this.s.contours);
    this.attrib.hidden = !on;
    if (!on) return;
    const { here, notices } = attribution(this.sources, this.inView);
    this.attrib.replaceChildren(
      h('div', { class: 'depth-at-src' }, h('b', {}, 'Depth: '), here.map((s) => s.label).join(' · ') || '—', '. Not for navigation.'),
      ...notices.map((n) => h('div', { class: 'depth-at-notice' }, n)));
  }

  // ---- the cursor readout -----------------------------------------------------------------------

  private tileAt(ll: maplibregl.LngLat): { z: number; x: number; y: number; px: number; py: number } {
    const z = Math.max(0, Math.min(16, Math.floor(this.map.getZoom())));
    const n = 2 ** z;
    const fx = ((ll.lng + 180) / 360) * n;
    const s = Math.sin((ll.lat * Math.PI) / 180);
    const fy = (0.5 - Math.log((1 + s) / (1 - s)) / (4 * Math.PI)) * n;
    const x = Math.floor(fx), y = Math.floor(fy);
    return { z, x: ((x % n) + n) % n, y, px: Math.floor((fx - x) * 512), py: Math.floor((fy - y) * 512) };
  }

  /** Depth, class and source at a point (null: no depth there, or no depth layer on). */
  async at(ll: maplibregl.LngLat): Promise<Probe | null> {
    if (!this.workers.length || !this.has || !(this.s.on || this.s.contours)) return null;
    const t = this.tileAt(ll);
    const r = await this.ask({ type: 'probe', id: 0, ...t }, undefined, this.workerFor(t.z, t.x, t.y));
    return r.probe ?? null;
  }

  private async hover() {
    this.hoverBusy = true;
    try {
      while (this.hoverAt) {
        const ll = this.hoverAt;
        this.hoverAt = null;
        const p = await this.at(ll);
        const text = p ? this.describe(p) : '';
        this.readout.textContent = text;
        this.readout.hidden = !text;
      }
    } catch {
      // (a tile still loading)
    } finally {
      this.hoverBusy = false;
    }
  }

  /** A point's depth in words: "Depth 12 m · measured · BlueTopo, 4–16 m". */
  describe(p: Probe): string {
    if (!p.water || p.d === null) return '';
    const src = this.source(p.src);
    const v = p.d > 0 ? `Dries: ${p.d.toFixed(1)} m above mean sea level` : p.dries ? `Dries at low tide: ${(-p.d).toFixed(1)} m below mean sea level` : `Depth ${(-p.d).toFixed(p.d > -10 ? 1 : 0)} m`;
    return [v, CLASSES[p.cls] ?? '', src ? `${src.label} (below ${src.datum})` : ''].filter(Boolean).join(' · ');
  }
}
