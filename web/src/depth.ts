// Depth under the water, seas and lakes (docs/steps.md §3), as two layers, each with its
// own switch (Layers → Depth):
//
// 1. Depth colour and floor shading: a colour-relief layer over a raster-dem source that
//    depth.worker.ts makes from the server's depth tiles (crates/server/src/depth.rs: a tile the
//    layer lacks comes from its nearest ancestor), and a hillshade layer over the floor's shading
//    as the build stores it (the `depth-shade` layer: each pixel's steepness and mean direction,
//    pipeline::depth's shading; served by crates/server/src/terrain.rs), shaded as the terrain's
//    is. Drawn as the water itself where the map has the water tiles: our MapLibre draws them over
//    the water layer's own tiles, as those tiles' alpha (`scenic:mask`, maplibre/src/webgl/draw/
//    water_mask.ts), the depth interpolated bilinearly and reaching a few pixels past its data so no
//    water drawn lacks it; water with no depth near shows the water's own colour. Without the water
//    tiles, the colour masked by the basemap's own water at the display's resolution, and no floor
//    shading (its tiles reach onto the shore).
// 2. Depth contours with labels: vector tiles the same worker makes (maplibre-contour's, contour/:
//    isolines), clipped to the water.
// All go over the water fill and under the coast shading and everything else, draped on the 3D
// terrain like the water (which stays flat at the surface).
// 3. The 3D floor (setFloor, its switch in the same section): the 3D terrain from the terrain less
//    the depth (the server's `?floor=1`, basemap.ts floorTiles), the terrain itself its surface
//    (terrain.ts applyTerrain; our MapLibre's Map#setTerrainSurface). The water stays draped, now on
//    the floor (with the depth's colour over it), and a see-through copy of it is drawn at the
//    surface (`water-surface`, the elevation reference `surface`); the ferries' lines are lifted to
//    the surface, and the water's and ferries' names stand on it (basemap.ts: the anchor `surface`,
//    the terrain itself without a floor). Roads and bridges keep their own heights (roads/). Depth is below the local water
// surface: mean sea level at sea, the lake's surface on lakes; estimated depths look as measured
// ones do (the footer says which they are, on hover: ui/strip.ts showWater). The depth's sources and the notices their licences ask
// for are in the credits (ui/strip.ts), not on the map.
import * as maplibregl from 'maplibre-gl';
import type { ExpressionSpecification, Map as MLMap } from 'maplibre-gl';
import { onVersions, ver } from './api';
import { SHADE_SHIFT, floorTiles, waterTilesOn } from './basemap';
import { hostFor } from './hosts';
import { depthColour, hdepth, hexRgb, HBINS, rgba, scaleFns, DEPTH_DEFAULTS, type DepthState } from './depthlook';
import type { DepthMsg, DepthReply, Probe } from './depth.worker';

type RGB = [number, number, number];

/** A depth source as the catalogue names it (pipeline::depth::grids). */
export interface DepthSource {
  code: number;
  id: string;
  label: string;
  datum: string;
  water: 'sea' | 'lake';
  notice?: string | null;
}

const CLASSES = ['', 'full detail', 'coarser, scaled up', 'estimated', 'filled to the shore'];
/** The water's surface over the floor (setFloor): see-through, the floor's colours showing. */
const SURFACE_OPACITY = 0.5;
/** What's drawn at the surface stands this high over it (m), clear of the floor where the two meet
 * (the shore, water without depth). */
const SURFACE_LIFT_M = 1;

export class Depth {
  private s: DepthState = DEPTH_DEFAULTS;
  /** The catalogue has depth (its layers) and its sources. */
  private has = false;
  private sources: DepthSource[] = [];
  /** The depth layers' least zoom stored (the catalogue's: the build makes z3 up). */
  private minzoom = 0;
  private workers: Worker[] = [];
  private seq = 0;
  private waiting = new Map<number, { resolve: (r: DepthReply) => void; reject: (e: Error) => void }>();
  private added = false;
  /** The 3D floor shown (setFloor). */
  private floorOn = false;
  /** The fitted domain (m) of the "fit to view" scale. */
  private fit: [number, number] = [1, 100];
  private viewTimer = 0;
  /** Told when the fitted domain changes (the panel's legend). */
  onFit: () => void = () => {};

  constructor(private map: MLMap) {
    map.on('moveend', () => this.scheduleView());
    onVersions((f) => f.startsWith('depth'), () => this.initWorkers());
  }

  /** The catalogue's depth: whether it has the layers, and its sources. */
  setCatalogue(layers: Record<string, unknown> | undefined, depth: { sources?: DepthSource[] } | null | undefined) {
    const has = !!layers && 'depth' in layers && 'depth-class' in layers;
    this.sources = depth?.sources ?? [];
    const mz = has ? Math.max(...(['depth', 'depth-class'] as const).map((k) => (layers[k] as { minzoom?: number }).minzoom ?? 0)) : 0;
    if (mz !== this.minzoom) {
      this.minzoom = mz;
      if (this.workers.length) this.initWorkers();
    }
    if (has !== this.has) {
      this.has = has;
      this.apply(this.s);
    }
  }

  /** Whether the catalogue has depth. */
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
      minzoom: this.minzoom,
    };
    for (const w of this.workers) w.postMessage(msg);
    (this.map.getSource('depth-shade') as maplibregl.RasterDEMTileSource | undefined)?.setTiles([this.shadeUrl()]);
    (this.map.getSource('dem-floor') as maplibregl.RasterDEMTileSource | undefined)?.setTiles([floorTiles()]);
  }

  /** The floor's shading tiles (the server's `depth-shade`). */
  private shadeUrl() {
    return `${hostFor('layers')}/tiles/depth-shade/{z}/{x}/{y}${ver('depth-shade.tiles')}`;
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

  /** Drawn as the water tiles' water (depth.ts's head). */
  private masked() {
    return waterTilesOn() && this.map.getSource('water')?.type === 'raster';
  }

  private urls(s: DepthState) {
    const q = this.masked() ? '?mask=1' : '';
    return {
      relief: `depth://relief/{z}/{x}/{y}${q}`,
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
    // (The shading's encoding: its base shift marks it for our MapLibre's shaders, basemap.ts's slope.)
    m.addSource('depth-shade', {
      type: 'raster-dem', tileSize: 256, maxzoom: 16, encoding: 'custom', redFactor: 256, greenFactor: 1, blueFactor: 1 / 256, baseShift: SHADE_SHIFT,
      tiles: [this.shadeUrl()],
    });
    m.addSource('depth-contours', { type: 'vector', maxzoom: 16, tiles: [u.contour] });
    const b = this.before();
    const metadata = this.masked() ? { 'scenic:mask': 'water' } : undefined;
    m.addLayer({ id: 'depth-relief', type: 'color-relief', source: 'depth-relief', metadata, paint: { 'color-relief-opacity': this.s.opacity, resampling: 'linear' } as never }, b);
    m.addLayer({ id: 'depth-shade', type: 'hillshade', source: 'depth-shade', metadata, paint: { 'hillshade-illumination-anchor': 'map', resampling: 'linear' } as never }, b);
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
    if (!on && !this.added) return;
    if (on) this.addLayers();
    const u = this.urls(s), ou = this.urls(before);
    if (u.contour !== ou.contour) (m.getSource('depth-contours') as maplibregl.VectorTileSource | undefined)?.setTiles([u.contour]);
    const vis = (id: string, v: boolean) => m.getLayer(id) && m.setLayoutProperty(id, 'visibility', v ? 'visible' : 'none');
    vis('depth-relief', this.has && s.on);
    vis('depth-shade', this.has && s.on && s.shade > 0 && this.masked());
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
    this.scheduleView();
  }

  /**
   * The 3D floor on the map or off (the depth's head, 3): with it, the water's see-through surface
   * over the floor and the ferries' lines lifted to it; main.ts swaps the terrain's source with it
   * (terrain.ts applyTerrain). Lifted layers aren't draped, so both go after the draped ones, under
   * the roads (one among them would split the terrain's draping in two).
   */
  setFloor(on: boolean) {
    if (on === this.floorOn) return;
    this.floorOn = on;
    const m = this.map;
    const above = m.getLayer('roads') ? 'roads' : undefined;
    const water = m.getLayer('water');
    if (on && water && !m.getLayer('water-surface')) {
      const layout = { visibility: m.getLayoutProperty('water', 'visibility') ?? 'visible' };
      m.addLayer(water.type === 'raster'
        ? {
          id: 'water-surface', type: 'raster', source: 'water', layout,
          paint: { 'raster-opacity': SURFACE_OPACITY, 'raster-resampling': 'linear', 'raster-fade-duration': 0, 'raster-elevation-reference': 'surface', 'raster-z-offset': SURFACE_LIFT_M } as never,
        }
        : {
          id: 'water-surface', type: 'fill', source: 'base', 'source-layer': 'water', layout, filter: m.getFilter('water') ?? undefined,
          paint: { 'fill-color': m.getPaintProperty('water', 'fill-color'), 'fill-opacity': SURFACE_OPACITY, 'fill-elevation-reference': 'surface', 'fill-z-offset': SURFACE_LIFT_M } as never,
        }, above);
    }
    if (!on && m.getLayer('water-surface')) m.removeLayer('water-surface');
    if (m.getLayer('ferry-line')) {
      // (Back under the selection's halo when off: its place in the basemap's style.)
      m.moveLayer('ferry-line', on ? above : m.getLayer('sel-halo') ? 'sel-halo' : undefined);
      m.setPaintProperty('ferry-line', 'line-elevation-reference' as never, (on ? 'surface' : 'ground') as never);
      m.setPaintProperty('ferry-line', 'line-z-offset' as never, (on ? SURFACE_LIFT_M : 0) as never);
    }
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

  /** With "fit to view", the scale's domain: the depths in view's 2nd to 98th percentile. */
  private async view() {
    if (!this.workers.length || this.s.scale !== 'fit') return;
    const tiles = this.viewTiles();
    // (Too many to sample.)
    if (!tiles.length) return;
    const halves = this.workers.map((w) => tiles.filter((t) => this.workerFor(...t) === w));
    const rs = await Promise.all(halves.map((ts, i) => (ts.length ? this.ask({ type: 'hist', id: 0, tiles: ts }, undefined, this.workers[i]) : Promise.resolve({ id: 0, hist: new Array(HBINS).fill(0) } as DepthReply))));
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

  // ---- the depth under the cursor (the footer) --------------------------------------------------

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
  private async at(ll: maplibregl.LngLat): Promise<Probe | null> {
    if (!this.workers.length || !this.has || !(this.s.on || this.s.contours)) return null;
    const t = this.tileAt(ll);
    const r = await this.ask({ type: 'probe', id: 0, ...t }, undefined, this.workerFor(t.z, t.x, t.y));
    return r.probe ?? null;
  }

  /** What the depth says of a point, for the footer (ui/strip.ts showWater): null where there's no
   * water with depth, no depth layer on, or its tile still loading. */
  async water(ll: maplibregl.LngLat): Promise<WaterDepth | null> {
    const p = await this.at(ll).catch(() => null);
    if (!p || !p.water || p.d === null) return null;
    const src = this.source(p.src);
    // A label's tail after its last comma is the source's resolution where it names one ("BlueTopo, 4–16 m").
    const m = src?.label.match(/^(.*),\s*([^,]*\d[^,]*)$/);
    return {
      depth: -p.d,
      dries: p.d > 0 || p.dries,
      colour: this.colourOf(p.d, p.dries),
      cls: CLASSES[p.cls] ?? '',
      source: m ? m[1] : src?.label ?? '',
      resolution: m ? m[2] : '',
      datum: src?.datum ?? '',
      water: src?.water ?? null,
    };
  }

  /** The colour the depth layer gives depth `d` (m, negative below the surface): its ramp's, or the drying colour. */
  private colourOf(d: number, dries: boolean): string {
    const s = this.s;
    if ((d > 0 || dries) && s.dry) return s.dryColour;
    const [lo, hi] = this.domain(s);
    const [fwd] = scaleFns(s.scale === 'linear' ? 'linear' : 'log', Math.max(0, lo), Math.max(lo + 0.5, hi));
    return rgba(depthColour(s.palette, fwd(Math.max(0, -d))));
  }
}

/** A point's depth as the footer shows it (Depth.water). */
export interface WaterDepth {
  /** Metres below the water's surface (negative: above it, drying). */
  depth: number;
  /** Water that dries at low tide. */
  dries: boolean;
  /** Its swatch: the depth layer's colour there. */
  colour: string;
  /** Full detail, coarser, estimated … (CLASSES). */
  cls: string;
  source: string;
  /** As the source's label names it ('' when it doesn't). */
  resolution: string;
  datum: string;
  water: 'sea' | 'lake' | null;
}
