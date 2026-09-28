import * as maplibregl from 'maplibre-gl';
import type { GeoJSONSource } from 'maplibre-gl';
import 'maplibre-gl/dist/maplibre-gl.css';
import { Protocol } from 'pmtiles';
// MapLibre resolves its worker at runtime, which bundlers can't see; bundle it explicitly.
import mlWorkerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url';
import './style.css';
import { getProfile, getWay, type Drive, type Meta, type Profile, type WayInfo } from './api';
import { baseStyle, LAYER_GROUPS } from './basemap';
import { AREA_LAYERS, Overlays, POINT_LAYERS } from './overlays';
import { paletteRgb } from './palettes';
import { RoadLayer, type HoverInfo } from './roads/layer';
import { distFromSamples, viewStats, type Dist, type Extreme, type ViewStats } from './roads/stats';
import { FLAG_LABELS, metricOf, modeDef, u8Area } from './scenic';
import * as prefs from './prefs';
import { Store, classMask, fromHash, fromSaved, groupMask, surfaceMask, toHash } from './state';
import * as cam3d from './camera3d';
import { applyLabelOpacity, applyTerrain, applyTint, tintCss, tintRange, type TintContext } from './terrain';
import { installTrackpad } from './trackpad';
import { Boot } from './ui/boot';
import { ClimbsPane, type Climb } from './ui/climbs';
import { ColourCard } from './ui/colour';
import { fmt, h, toast } from './ui/dom';
import { DrivesPane } from './ui/drives';
import { LayersCard } from './ui/layers';
import { NavControls } from './ui/nav';
import { ProfilePanel } from './ui/profile';
import { StatsCard } from './ui/stats';
import { Strip } from './ui/strip';
import { ViewshedTool } from './ui/viewshed';

// Debug: ?bgrender keeps the map rendering in a hidden/background tab (timer-driven frames),
// for automated checks. No effect otherwise.
if (new URLSearchParams(location.search).has('bgrender')) {
  window.requestAnimationFrame = (f) => window.setTimeout(() => f(performance.now()), 16);
  window.cancelAnimationFrame = (id) => window.clearTimeout(id);
}

const boot = new Boot(['Loading dataset metadata', 'Starting map engine', 'Loading basemap style', 'Loading road tiles in view']);

async function main() {
  boot.at(0);
  let meta: Meta;
  try {
    const r = await fetch('/api/meta');
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    meta = await r.json();
  } catch (e) {
    boot.fail(0, `backend not reachable (${(e as Error).message})`);
    return;
  }

  // A link (URL hash) wins; otherwise restore the last session from localStorage.
  const store = new Store(location.hash.length > 1 ? fromHash(location.hash) : fromSaved(prefs.load('state', null)));
  history.replaceState(null, '', toHash(store.s)); // so "Copy link" has the restored state
  boot.at(1);
  maplibregl.setWorkerUrl(mlWorkerUrl);
  maplibregl.addProtocol('pmtiles', new Protocol().tile);
  const v = store.s.view;
  const map = new maplibregl.Map({
    container: 'map',
    style: baseStyle(location.origin),
    center: v ? [v.lng, v.lat] : [-70, 46],
    zoom: v ? v.zoom : 5,
    bearing: v?.bearing ?? 0,
    pitch: v?.pitch ?? 0,
    minZoom: 1.2,
    maxZoom: 19.5,
    maxPitch: 80,
    dragRotate: true,
    pitchWithRotate: true,
    touchPitch: true,
    attributionControl: false,
    renderWorldCopies: false,
    fadeDuration: 120,
    // The camera does not ride up and down with the terrain under the view centre.
    centerClampedToGround: store.s.terrain.cameraFollow,
  });
  unlinkCameraFromTerrain(map, () => store.s.terrain.cameraFollow);
  if (v?.elev && !store.s.terrain.cameraFollow) map.jumpTo({ elevation: v.elev });
  // Once the terrain is ready, set the pivot for the view (camera fixed; see camera3d.relevel).
  const levelOnce = () => {
    if (map.queryTerrainElevation(map.getCenter()) === null) return;
    map.off('idle', levelOnce);
    cam3d.relevel(map);
  };
  if (!store.s.terrain.cameraFollow && store.s.terrain.on) map.on('idle', levelOnce);
  if (!v) {
    map.fitBounds([[-80.6, 40.4], [-52.6, 51.8]], { duration: 0, padding: { top: 20, bottom: 90, left: 340, right: 290 } });
    // 3D terrain is on by default: start with a gentle tilt so it shows.
    if (store.s.terrain.on) map.jumpTo({ pitch: 40 });
  }
  installTrackpad(map);

  // ---- road layer & range animation ----------------------------------------------
  const s0 = store.s;
  const roads = new RoadLayer({
    mode: s0.mode,
    palette: s0.palette,
    range: [...s0.range] as [number, number],
    classMask: classMask(s0),
    surfaceMask: surfaceMask(s0),
    weight: s0.weight,
    threshold: { ...s0.threshold },
    visible: s0.layers.roads,
    weights: [...s0.weights],
    equalize: s0.equalize,
    routeGlow: s0.routeGlow,
    terrain3d: s0.terrain.on,
    exaggeration: s0.terrain.exaggeration,
    lowFade: s0.lowFade,
    lowSpan: s0.lowSpan,
    perspective: s0.perspective,
    blendOverlaps: s0.blendOverlaps,
  });
  roads.bounds = meta.bounds;
  roads.version = String(meta.built);

  let stats: ViewStats | null = null;
  /** Distribution of the current colour metric over roads in view. */
  let mdist: Dist | null = null;
  let extraRows: [string, string, string?][] = [];
  let statsDirty = true;
  let lastStats = 0;
  let cur: [number, number] = [...s0.range] as [number, number];
  let snap = true;
  let cdf: Uint8Array | null = null;
  let cdfKey = '';

  const target = (): [number, number] => {
    const s = store.s;
    const d = modeDef(s.mode);
    if (s.mode === 'relief') {
      const e = stats?.elev;
      return e ? spread(e.quantile(0), e.quantile(1), 10) : cur;
    }
    if (!s.auto) return s.range;
    const [pl, ph] = s.fit;
    return mdist && mdist.total > 0 ? spread(mdist.quantile(pl / 100), mdist.quantile(ph / 100), d.step * 4) : cur;
  };
  const spread = (lo: number, hi: number, min: number): [number, number] => {
    if (hi - lo < min) {
      const m = (lo + hi) / 2;
      return [m - min / 2, m + min / 2];
    }
    return [lo, hi];
  };

  const computeStats = () => {
    const s = store.s;
    stats = viewStats(roads, groupMask(s), classMask(s), surfaceMask(s));
    const d = modeDef(s.mode);
    // Scenic summary for the stats card + the current metric's distribution, in one pass.
    const modes = ['score', 'openness', 'trees', 'vista'] as const;
    const scenicMode = d.id >= 3 && !(modes as readonly string[]).includes(s.mode) ? [s.mode] : [];
    const smp = roads.metricSamples([...modes, ...scenicMode], s.weights, 90_000);
    const dists = modes.map((m, i) => distFromSamples(smp.v[i], smp.w, ...modeDef(m).domain));
    if (s.mode === 'elev' || s.mode === 'relief') mdist = stats.elev;
    else if (s.mode === 'grade') mdist = stats.grade;
    else {
      const i = (modes as readonly string[]).indexOf(s.mode);
      mdist = i >= 0 ? dists[i] : distFromSamples(smp.v[modes.length], smp.w, ...d.domain);
    }
    const [sc, op, tr, vi] = dists;
    extraRows = sc && sc.total > 0
      ? [
          ['Scenic score', `${sc.quantile(0.5).toFixed(0)} median · ${sc.quantile(0.9).toFixed(0)} p90`, 'With the current weights (Colour → Scenic → Score)'],
          ['Open views', `${((op?.above(50) ?? 0) * 100).toFixed(0)} % of length`, 'Share of road length where at least half the directions are not blocked by trees or terrain within 300 m'],
          ['Roadside trees', `${tr?.quantile(0.5).toFixed(0)} m median`, 'Typical tree height within 30 m of the road (Meta/WRI canopy height)'],
          ['Vista distance', `${vi?.quantile(0.5).toFixed(1)} km median · ${vi?.quantile(0.9).toFixed(1)} p90`],
        ]
      : [];
  };

  const updateCdf = () => {
    const s = store.s;
    if (!s.equalize || !mdist || mdist.total <= 0) {
      if (cdf) {
        cdf = null;
        roads.setCdf(null);
      }
      return;
    }
    const key = `${s.mode}|${cur[0].toFixed(3)}|${cur[1].toFixed(3)}|${mdist.total.toFixed(0)}`;
    if (key === cdfKey) return;
    cdfKey = key;
    const out = new Uint8Array(256);
    const a = 1 - mdist.above(cur[0]), b = 1 - mdist.above(cur[1]);
    const span = Math.max(1e-9, b - a);
    for (let i = 0; i < 256; i++) {
      const v = cur[0] + (i / 255) * (cur[1] - cur[0]);
      out[i] = Math.round(Math.max(0, Math.min(1, (1 - mdist.above(v) - a) / span)) * 255);
    }
    cdf = out;
    roads.setCdf(out);
  };

  // Elevation tint: its range can follow the view or the road colours, so refresh it with the stats.
  const tintCtx = (): TintContext => ({
    roadPalette: store.s.palette,
    roadRange: cur,
    roadIsElevation: store.s.mode === 'elev' || store.s.mode === 'relief',
    roadIsGrade: store.s.mode === 'grade',
    elev: stats?.elev ?? null,
  });
  let lastTintLegend = '';
  let tintPreview: string | null = null;
  const refreshTint = () => {
    if (!styleReady) return;
    const t = tintPreview ? { ...store.s.terrain, tintPalette: tintPreview } : store.s.terrain;
    const ctx = tintCtx();
    applyTint(map, t, ctx);
    if (!t.tint) return;
    const r = tintRange(t, ctx);
    const css = tintCss(t, ctx);
    const key = `${css}|${r[0].toFixed(0)}|${r[1].toFixed(0)}`;
    if (key !== lastTintLegend) {
      lastTintLegend = key;
      layers.setTintLegend(css, r);
    }
  };

  const wireTintPreview = () => {
    layers.tintCssFor = (key) => tintCss({ ...store.s.terrain, tintPalette: key }, tintCtx());
    layers.onTintPreview = (key) => {
      tintPreview = key;
      refreshTint();
    };
    layers.sync(store.s);
  };

  // ---- UI ------------------------------------------------------------------------
  const km = Math.round(meta.elev_hist_10m_km.reduce((a, b) => a + b, 0));
  const pct = (n: number) => ((n / meta.dem.vertices) * 100).toFixed(0);
  const colour = new ColourCard(
    document.getElementById('colour')!,
    store,
    `Ontario · Québec · Atlantic Canada · New York · New England<br>` +
      `${fmt.n(km)} km of drivable public road · ${(meta.vertices / 1e6).toFixed(0)} M elevation samples ` +
      `(${pct(meta.dem.hrdem)} % lidar, ${pct(meta.dem.usgs3dep)} % 3DEP, ${pct(meta.dem.mrdem)} % MRDEM)`,
  );
  const layers = new LayersCard(document.getElementById('layers')!, store);
  wireTintPreview();
  const statsEl = document.getElementById('stats')!;
  const statsCard = new StatsCard(statsEl);
  // The layers card fills the height the stats card leaves free.
  new ResizeObserver(() => document.documentElement.style.setProperty('--stats-h', `${statsEl.offsetHeight + 12}px`)).observe(statsEl);
  const climbs = new ClimbsPane(statsCard.climbsRoot);
  const drives = new DrivesPane(statsCard.drivesRoot);
  const strip = new Strip(document.getElementById('strip')!, map);
  const profile = new ProfilePanel(document.getElementById('profile')!);
  new NavControls(document.getElementById('nav')!, map, store);
  const viewshed = new ViewshedTool(document.getElementById('viewshed')!, map);
  const overlays = new Overlays(map, layers);
  profile.colour = () => ({ palette: store.s.palette, mode: store.s.mode, range: cur, weights: store.s.weights, cdf: store.s.equalize ? cdf : null });
  const progressEl = document.getElementById('progress')!;
  const progressBar = progressEl.querySelector<HTMLDivElement>('.bar')!;
  let overlayBusy: string | null = null;
  overlays.onBusy = (label) => (overlayBusy = label);

  // Markers: profile cursor, highest / lowest road in view, viewshed eye.
  const marks: Record<string, GeoJSON.Feature | null> = { cursor: null, high: null, low: null, viewshed: null };
  const setMarks = () => {
    const src = map.getSource<GeoJSONSource>('marks');
    src?.setData({ type: 'FeatureCollection', features: Object.values(marks).filter(Boolean) as GeoJSON.Feature[] });
  };
  const point = (ll: [number, number], kind: string, label?: string): GeoJSON.Feature => ({
    type: 'Feature',
    properties: label ? { kind, label } : { kind },
    geometry: { type: 'Point', coordinates: ll },
  });
  const line = (coords: [number, number][] | null): GeoJSON.GeoJSON =>
    coords ? { type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates: coords } } : { type: 'FeatureCollection', features: [] };
  profile.onHover = (ll, label) => {
    marks.cursor = ll ? point(ll, 'cursor', label) : null;
    setMarks();
  };
  statsCard.onMark = (x, kind) => {
    marks[kind] = x ? point(x.lngLat, kind, fmt.m(x.elev)) : null;
    setMarks();
  };
  statsCard.onFly = (x: Extreme) => {
    map.flyTo({ center: x.lngLat, zoom: Math.max(map.getZoom(), 14), duration: 1200 });
    const kind = x === stats?.highest ? 'high' : 'low';
    marks[kind] = point(x.lngLat, kind, fmt.m(x.elev));
    setMarks();
    setTimeout(() => {
      marks[kind] = null;
      setMarks();
    }, 6000);
  };
  const fitPad = { top: 80, bottom: 320, left: 360, right: 320 };
  const bboxQuery = () => {
    const b = map.getBounds();
    return [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()].map((x) => x.toFixed(5)).join(',');
  };

  // Top climbs: live list for the view; hover highlights, click opens the profile.
  let pinnedClimb: Climb | null = null;
  const setClimb = (c: Climb | null) => map.getSource<GeoJSONSource>('climb')?.setData(line(c ? c.geom : null));
  climbs.query = () => ({ bbox: bboxQuery(), classes: classMask(store.s), surface: surfaceMask(store.s) });
  climbs.onHover = (c) => setClimb(c ?? pinnedClimb);
  climbs.onSelect = (c) => {
    pinnedClimb = c;
    setClimb(c);
    const g = c.geom;
    profile.highlight({ start: g[0], end: g[g.length - 1], label: `Climb +${fmt.n(c.gain_m)} m · ${fmt.dist(c.length_m)} · ${c.avg_grade.toFixed(1)} %` });
    store.set({ selected: c.way });
    const b = new maplibregl.LngLatBounds();
    for (const q of g) b.extend(q);
    map.fitBounds(b, { padding: fitPad, duration: 900, maxZoom: 15 });
  };

  // Scenic drives.
  let pinnedDrive: Drive | null = null;
  const setDriveHl = (d: Drive | null) => map.getSource<GeoJSONSource>('drive-hl')?.setData(line(d ? d.geom : null));
  drives.query = () => ({ bbox: bboxQuery(), classes: classMask(store.s), surface: surfaceMask(store.s), weights: store.s.weights });
  drives.onResults = (ds) =>
    map.getSource<GeoJSONSource>('drives')?.setData({
      type: 'FeatureCollection',
      features: ds.map((d) => ({ type: 'Feature', properties: { score: d.score }, geometry: { type: 'LineString', coordinates: d.geom } })),
    });
  drives.onHover = (d) => setDriveHl(d ?? pinnedDrive);
  drives.onSelect = (d) => {
    pinnedDrive = d;
    setDriveHl(d);
    const g = d.geom;
    profile.highlight({ start: g[0], end: g[g.length - 1], label: `Scenic ${d.score.toFixed(0)} · ${fmt.dist(d.length_m)}` });
    store.set({ selected: d.way });
    const b = new maplibregl.LngLatBounds();
    for (const q of g) b.extend(q);
    map.fitBounds(b, { padding: fitPad, duration: 900, maxZoom: 15 });
  };

  statsCard.onTab = (k) => {
    if (k === 'climbs') climbs.refresh(true);
    else if (!pinnedClimb) setClimb(null);
    if (k === 'drives') drives.refresh(true);
    else {
      drives.onResults([]);
      if (!pinnedDrive) setDriveHl(null);
    }
  };
  map.on('moveend', () => {
    climbs.refresh();
    drives.refresh();
  });

  statsCard.show(statsCard.tab); // restored tab: run its loaders now that callbacks exist

  statsCard.wayName = async (x) => {
    const info = await getWay(x.tile.data!.lineWay[x.line]);
    return info ? [info.ref, info.name].filter(Boolean).join(' ') : '';
  };

  // Hover readouts for the scenic channels.
  strip.extra = (hov) => {
    const c = hov.ch;
    const w = store.s.weights;
    const out: HTMLElement[] = [];
    const kv = (k: string, v: string, title?: string) => h('span', { class: 'kv', title }, `${k} `, h('b', {}, v));
    if (c.every((x) => x === 0)) return out;
    out.push(kv('scenic', metricOf('score', 0, 0, c, w).toFixed(0), 'Scenic score with the current weights'));
    out.push(kv('view', `${fmtArea(u8Area(c[0]))}`, 'Visible area within 15 km, trees and terrain blocking'));
    if (c[1] > 10) out.push(kv('water', fmtArea(u8Area(c[1])), 'Visible water area'));
    out.push(kv('vista', `${(c[8] / 17).toFixed(1)} km`, 'Mean farthest visible distance'));
    out.push(kv('open', `${Math.round(100 - c[5] / 2.55)} %`, 'Directions not blocked within 300 m'));
    out.push(kv('trees', `${Math.round(c[11] / 8)} m`, 'Roadside tree height (p95 within 30 m)'));
    const flags = FLAG_LABELS.filter(([m]) => c[7] & m).map(([, l]) => l);
    if (flags.length) out.push(h('span', { class: 'kv flags' }, flags.join(' · ')));
    return out;
  };

  let basemapLoading = false;
  map.on('dataloading', () => (basemapLoading = true));
  map.on('idle', () => (basemapLoading = false));

  const refresh = () => {
    const p = roads.progress();
    const busy = p.loaded < p.wanted;
    progressEl.classList.toggle('busy', busy || basemapLoading || !!overlayBusy);
    progressBar.style.width = `${p.wanted ? (p.loaded / p.wanted) * 100 : 100}%`;
    strip.setLoading(
      [busy ? `Loading roads ${p.loaded}/${p.wanted}` : '', basemapLoading ? 'basemap & terrain…' : '', overlayBusy ?? ''].filter(Boolean).join(' · '),
    );
    colour.update(mdist, cur, cdf);
    layers.update(stats);
    statsCard.update(stats, p, roads.zt, extraRows);
  };

  // Frame loop: statistics (throttled) and eased colour range.
  let last = performance.now();
  let lastPanel = 0;
  const tick = (now: number) => {
    const dt = now - last;
    last = now;
    let panels = false;
    if (statsDirty && now - lastStats > 150) {
      statsDirty = false;
      lastStats = now;
      computeStats();
      panels = true;
    }
    const t = target();
    const k = snap ? 1 : 1 - Math.exp(-dt / 140);
    snap = false;
    const next: [number, number] = [cur[0] + (t[0] - cur[0]) * k, cur[1] + (t[1] - cur[1]) * k];
    const eps = modeDef(store.s.mode).step * 0.01;
    if (Math.abs(next[0] - cur[0]) + Math.abs(next[1] - cur[1]) > eps) {
      cur = next;
      roads.style.range = cur;
      map.triggerRepaint();
      panels = true;
    }
    if (panels && now - lastPanel > 60) {
      lastPanel = now;
      updateCdf();
      refreshTint();
      refresh();
      profile.redraw();
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);

  roads.onChange = () => {
    statsDirty = true;
    const p = roads.progress();
    if (!bootDone) {
      boot.sub(3, p.wanted ? p.loaded / p.wanted : 0, `${p.loaded}/${p.wanted}`);
      if (p.wanted > 0 && p.loaded >= p.wanted) finishBoot();
    }
  };
  map.on('move', () => (statsDirty = true));

  // ---- hover & selection -----------------------------------------------------------
  const wayCache = new Map<number, WayInfo | null>();
  let hovered: HoverInfo | null = null;
  let pickAt: { x: number; y: number } | null = null;
  const colourOf = (hv: HoverInfo) => {
    const s = store.s;
    const val = metricOf(s.mode, hv.elev, hv.grade, hv.ch, s.weights);
    let u = Math.max(0, Math.min(1, (val - cur[0]) / (cur[1] - cur[0])));
    if (s.equalize && cdf) u = cdf[Math.min(255, Math.floor(u * 255 + 0.5))] / 255;
    return paletteRgb(s.palette, u);
  };
  const interactive = () => [...POINT_LAYERS].filter((id) => map.getLayer(id) && map.getLayoutProperty(id, 'visibility') !== 'none');
  const doPick = () => {
    const pt = pickAt;
    pickAt = null;
    if (!pt || driving) return;
    hovered = roads.pick(pt.x, pt.y);
    roads.setHover(hovered);
    const onPoi = !hovered && interactive().length > 0 && map.queryRenderedFeatures([[pt.x - 4, pt.y - 4], [pt.x + 4, pt.y + 4]], { layers: interactive() }).length > 0;
    map.getCanvas().style.cursor = viewshed.active ? 'crosshair' : hovered || onPoi ? 'pointer' : '';
    if (!hovered) return strip.show(null, null, '');
    const hv = hovered;
    const cached = wayCache.get(hv.way);
    strip.show(hv, cached === undefined ? 'loading' : cached, colourOf(hv));
    if (cached === undefined) {
      getWay(hv.way).then((info) => {
        wayCache.set(hv.way, info);
        if (hovered?.way === hv.way) strip.show(hovered, info, colourOf(hovered));
      });
    }
  };
  // Last cursor position on the map (terrain-aware), for the Street View shortcut.
  let cursorLL: maplibregl.LngLat | null = null;
  map.on('mousemove', (e) => {
    if (!pickAt) requestAnimationFrame(doPick);
    pickAt = { x: e.point.x, y: e.point.y };
    cursorLL = e.lngLat;
  });
  map.getCanvas().addEventListener('mouseleave', () => (cursorLL = null));
  map.getCanvas().addEventListener('mouseleave', () => {
    hovered = null;
    roads.setHover(null);
    strip.show(null, null, '');
  });

  let profileAbort: AbortController | null = null;
  const select = async (way: number | null) => {
    profileAbort?.abort();
    if (way === null) {
      profile.hide();
      map.getSource<GeoJSONSource>('selection')?.setData(line(null));
      pinnedClimb = null;
      setClimb(null);
      pinnedDrive = null;
      setDriveHl(null);
      return;
    }
    profileAbort = new AbortController();
    const info = wayCache.get(way) ?? (await getWay(way));
    profile.loading(info ? info.ref || info.name || 'this road' : 'this road');
    try {
      const p = await getProfile(way, profileAbort.signal);
      profile.show(p);
      map.getSource<GeoJSONSource>('selection')?.setData(line(p.coords));
    } catch (e) {
      if ((e as Error).name !== 'AbortError') profile.error((e as Error).message);
    }
  };
  profile.onClose = () => store.set({ selected: null });
  profile.onZoom = (p) => {
    const b = new maplibregl.LngLatBounds();
    for (const c of p.coords) b.extend(c);
    map.fitBounds(b, { padding: { top: 60, bottom: 300, left: 340, right: 300 }, duration: 900 });
  };

  // ---- fly-along "Drive" ------------------------------------------------------------
  let driving: { raf: number; stop: () => void } | null = null;
  const stopDrive = () => driving?.stop();
  profile.onDrive = (p: Profile) => {
    stopDrive();
    if (!store.s.terrain.on) store.terrain({ on: true });
    const n = p.coords.length;
    const L = p.dist[n - 1];
    // Whole road in 30 s – 3 min.
    const speed = L / Math.max(30, Math.min(180, (L / 1000) * 2.5));
    const at = (d: number): [number, number] => {
      d = Math.max(0, Math.min(L, d));
      let a = 0, b = n - 1;
      while (b - a > 1) {
        const m = (a + b) >> 1;
        if (p.dist[m] <= d) a = m;
        else b = m;
      }
      const t = (d - p.dist[a]) / Math.max(1e-9, p.dist[b] - p.dist[a]);
      return [p.coords[a][0] + (p.coords[b][0] - p.coords[a][0]) * t, p.coords[a][1] + (p.coords[b][1] - p.coords[a][1]) * t];
    };
    const brg = (a: [number, number], b: [number, number]) => {
      const k = Math.PI / 180;
      const y = Math.sin((b[0] - a[0]) * k) * Math.cos(b[1] * k);
      const x = Math.cos(a[1] * k) * Math.sin(b[1] * k) - Math.sin(a[1] * k) * Math.cos(b[1] * k) * Math.cos((b[0] - a[0]) * k);
      return Math.atan2(y, x) / k;
    };
    let d = 0;
    let bearing = brg(at(0), at(400));
    let elev: number | null = null;
    let t0 = performance.now();
    const H = map.getCanvas().clientHeight;
    map.setPadding({ top: H * 0.3, bottom: 0, left: 0, right: 0 });
    const frame = (now: number) => {
      const dt = Math.min(0.1, (now - t0) / 1000);
      t0 = now;
      d += speed * dt;
      const pos = at(d);
      const want = brg(at(d - 50), at(d + 450));
      let diff = ((want - bearing + 540) % 360) - 180;
      bearing += diff * Math.min(1, dt * 1.8);
      // The drive camera follows the road's height on purpose (smoothed).
      const ground = map.queryTerrainElevation(pos) ?? 0;
      elev = elev === null ? ground : elev + (ground - elev) * Math.min(1, dt * 2);
      map.jumpTo({ center: pos, bearing, pitch: 70, zoom: 14.6, elevation: elev });
      marks.cursor = point(pos, 'cursor');
      setMarks();
      if (d >= L) return driving?.stop();
      if (driving) driving.raf = requestAnimationFrame(frame);
    };
    const stop = () => {
      if (!driving) return;
      cancelAnimationFrame(driving.raf);
      driving = null;
      map.setPadding({ top: 0, bottom: 0, left: 0, right: 0 });
      marks.cursor = null;
      setMarks();
      canvas.removeEventListener('mousedown', stop);
      canvas.removeEventListener('wheel', stop);
    };
    const canvas = map.getCanvasContainer();
    canvas.addEventListener('mousedown', stop);
    canvas.addEventListener('wheel', stop);
    toast('Driving… click, scroll or Esc to stop');
    driving = { raf: requestAnimationFrame(frame), stop };
  };

  // ---- clicks ----------------------------------------------------------------------
  map.on('click', (e) => {
    if (viewshed.active) {
      viewshed.run([e.lngLat.lng, e.lngLat.lat]);
      return;
    }
    if (overlays.click(e.point, POINT_LAYERS)) return;
    const hv = roads.pick(e.point.x, e.point.y);
    if (hv) {
      overlays.closePopup();
      pinnedClimb = null;
      setClimb(null);
      pinnedDrive = null;
      setDriveHl(null);
      profile.highlight(null);
      store.set({ selected: hv.way });
      return;
    }
    overlays.click(e.point, AREA_LAYERS);
  });
  layers.onViewshed = () => (viewshed.active ? viewshed.cancel() : viewshed.start());
  viewshed.onActive = (on) => layers.setViewshedActive(on);
  viewshed.onMark = (ll) => {
    marks.viewshed = ll ? point(ll, 'viewshed') : null;
    setMarks();
  };
  window.addEventListener('keydown', (e) => {
    // G: Google Street View at the cursor (snapped to the hovered road), facing the map's bearing.
    if ((e.key === 'g' || e.key === 'G') && !e.metaKey && !e.ctrlKey && !e.altKey) {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'SELECT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (!cursorLL) return toast('Point at the map, then press G for Street View');
      const [lng, lat] = hovered ? hovered.lngLat : [cursorLL.lng, cursorLL.lat];
      const heading = ((map.getBearing() % 360) + 360) % 360;
      const q = new URLSearchParams({ api: '1', map_action: 'pano', viewpoint: `${lat.toFixed(6)},${lng.toFixed(6)}`, heading: heading.toFixed(0), pitch: '0', fov: '90' });
      window.open(`https://www.google.com/maps/@?${q.toString().replace(/%2C/g, ",")}`, '_blank', 'noopener');
      toast('Opening Street View (nearest panorama, if any)');
      return;
    }
    if (e.key !== 'Escape') return;
    if (driving) return stopDrive();
    if (viewshed.active) return viewshed.cancel();
    overlays.closePopup();
    store.set({ selected: null });
  });

  // ---- state → map ------------------------------------------------------------------
  // MapLibre's globe, which hands over to flat Web Mercator at zoom 11–12. That can't go deeper:
  // the globe camera measures its distance from sea level and must stay above the terrain, so
  // with exaggerated mountains (Mt Washington ≈ 5.7 km at ×3) a globe at zoom 13–14 could not
  // get close. At 11–12 the camera is 30–60 km out and the curvature across the screen is
  // under a pixel. The cursor-anchored camera handles both (camera3d.ts).
  const applyProjection = () => map.setProjection({ type: store.s.globe ? 'globe' : 'mercator' });
  const applyLayers = () => {
    for (const [k, ids] of Object.entries(LAYER_GROUPS)) {
      const on = store.s.layers[k as 'water' | 'boundaries' | 'places'];
      for (const id of ids) if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', on ? 'visible' : 'none');
    }
  };
  let hashTimer = 0;
  const writeHash = () => {
    clearTimeout(hashTimer);
    hashTimer = window.setTimeout(() => history.replaceState(null, '', toHash(store.s)), 150);
    prefs.saveSoon('state', () => ({ ...store.s, selected: null }));
  };
  let styleReady = false;
  let lastExaggeration = store.s.terrain.on ? store.s.terrain.exaggeration : 0;
  store.on((s, ch) => {
    const st = roads.style;
    st.mode = s.mode;
    st.palette = s.palette;
    st.classMask = classMask(s);
    st.surfaceMask = surfaceMask(s);
    st.weight = s.weight;
    st.threshold = { ...s.threshold };
    st.visible = s.layers.roads;
    st.weights = [...s.weights];
    st.equalize = s.equalize;
    st.routeGlow = s.routeGlow;
    st.terrain3d = s.terrain.on;
    st.exaggeration = s.terrain.exaggeration;
    st.lowFade = s.lowFade;
    st.lowSpan = s.lowSpan;
    st.perspective = s.perspective;
    st.blendOverlaps = s.blendOverlaps;
    if (ch.has('mode')) snap = true;
    if (ch.has('groups') || ch.has('surface') || ch.has('layers') || ch.has('mode') || ch.has('weights')) statsDirty = true;
    if (ch.has('equalize') || ch.has('mode')) cdfKey = '';
    if (styleReady) {
      if (ch.has('layers')) applyLayers();
      if (ch.has('terrain')) {
        map.setCenterClampedToGround(s.terrain.cameraFollow);
        // Terrain on/off or re-exaggerated: re-pivot for the new ground (camera stays put).
        if (!s.terrain.cameraFollow && (s.terrain.on ? s.terrain.exaggeration : 0) !== lastExaggeration) {
          if (!s.terrain.on) cam3d.repivot(map, 0); // flat map: pivot at sea level
          else map.on('idle', levelOnce);
        }
        lastExaggeration = s.terrain.on ? s.terrain.exaggeration : 0;
        applyTerrain(map, s.terrain, location.origin);
        applyLabelOpacity(map, s.labelOpacity);
      }
      if (ch.has('terrain') || ch.has('palette') || ch.has('mode')) refreshTint();
      if (ch.has('labelOpacity')) applyLabelOpacity(map, s.labelOpacity);
      if (ch.has('globe')) applyProjection();
      if (ch.has('overlays') || ch.has('heritageLevels')) overlays.apply(s);
    }
    if (ch.has('groups') || ch.has('surface')) {
      climbs.refresh();
      drives.refresh();
    }
    if (ch.has('weights')) drives.refresh();
    if (ch.has('selected')) select(s.selected);
    if (!ch.has('view')) {
      colour.sync();
      layers.sync(s);
      refresh();
      if (ch.has('weights') || ch.has('mode') || ch.has('palette')) profile.redraw();
    }
    map.triggerRepaint();
    writeHash();
  });
  // Keep the zoom level meaningful: after every move, put the camera pivot on the terrain at the
  // view centre *without moving the camera* (zoom and centre are re-solved from the camera
  // position). Nothing moves on screen; tile detail, line widths and labels follow the real
  // distance to the ground. The camera itself never follows the terrain.
  const relevel = () => {
    if (driving || store.s.terrain.cameraFollow || !store.s.terrain.on) return;
    cam3d.relevel(map);
  };
  // Only once a gesture has settled: mid-gesture the zoom number (and with it widths, labels and
  // tile detail) must change smoothly.
  let relevelTimer = 0;
  map.on('moveend', () => {
    clearTimeout(relevelTimer);
    relevelTimer = window.setTimeout(relevel, 300);
  });
  map.on('movestart', () => clearTimeout(relevelTimer));

  // Pasted / edited links: apply the new state without a reload.
  window.addEventListener('hashchange', () => {
    const next = fromHash(location.hash);
    const { view, ...rest } = next;
    store.set(rest);
    if (view) map.jumpTo({ center: [view.lng, view.lat], zoom: view.zoom, bearing: view.bearing, pitch: view.pitch });
  });
  map.on('moveend', () => {
    if (driving) return;
    const c = map.getCenter();
    const elev = store.s.terrain.cameraFollow ? 0 : map.getCenterElevation();
    store.set({ view: { zoom: map.getZoom(), lat: c.lat, lng: c.lng, bearing: map.getBearing(), pitch: map.getPitch(), elev } });
  });

  // ---- boot --------------------------------------------------------------------------
  let bootDone = false;
  const finishBoot = () => {
    if (bootDone) return;
    bootDone = true;
    boot.done();
  };
  boot.at(2);
  // Attach as soon as the style is parsed; basemap tiles keep streaming in behind.
  // (An inline style can finish parsing before listeners are registered.)
  const attach = () => {
    if (map.getLayer('roads')) return;
    styleReady = true;
    applyProjection();
    map.addLayer(roads, 'water-name-line');
    applyLayers();
    applyTerrain(map, store.s.terrain, location.origin);
    applyLabelOpacity(map, store.s.labelOpacity);
    refreshTint();
    overlays.apply(store.s);
    boot.at(3);
    if (store.s.selected !== null) select(store.s.selected);
    // Never block the UI for long on a slow first view.
    setTimeout(finishBoot, 12000);
  };
  if ((map as unknown as { style?: { _loaded?: boolean } }).style?._loaded) attach();
  else {
    map.once('style.load', attach);
    map.once('load', attach);
  }
  map.on('error', (e) => console.warn(e.error?.message ?? e));
  (window as any).__app = { map, roads, store, cam3d };
}

/**
 * MapLibre eases the camera pivot toward the terrain under the target on every animation
 * (easeTo, flyTo, fitBounds) even with centerClampedToGround off. When the camera shouldn't
 * follow the terrain, keep only the bookkeeping part (minimum elevation for clipping).
 */
function unlinkCameraFromTerrain(map: maplibregl.Map, follow: () => boolean) {
  type Cam = { terrain?: { getMinTileElevationForLngLatZoom: (c: unknown, z: number) => number }; _updateElevation?: (k: number, tr: any) => void };
  const cam = (map as unknown as { _camera?: Cam })._camera;
  const orig = cam?._updateElevation;
  if (!cam || !orig) return;
  cam._updateElevation = function (k: number, tr: any) {
    if (follow()) return orig.call(this, k, tr);
    const terrain = (this as Cam).terrain;
    if (terrain) tr.setMinElevationForCurrentTile(terrain.getMinTileElevationForLngLatZoom(tr.center, tr.tileZoom));
  };
  // MapLibre's "camera inside terrain" rescue throws on the globe when the camera is straight
  // above the centre (camera and target coincide); treat that as nothing to fix instead of
  // aborting the camera move.
  const c2 = cam as unknown as { _elevateCameraIfInsideTerrain?: (tr: unknown) => object };
  const origElevate = c2._elevateCameraIfInsideTerrain;
  if (origElevate) {
    c2._elevateCameraIfInsideTerrain = function (tr: unknown) {
      try {
        return origElevate.call(this, tr);
      } catch {
        return {};
      }
    };
  }
}

function fmtArea(km2: number) {
  return (km2 < 1 ? km2.toFixed(2) : km2 < 10 ? km2.toFixed(1) : Math.round(km2).toLocaleString('en-CA')) + ' km²';
}

main();
