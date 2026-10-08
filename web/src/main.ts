import * as maplibregl from 'maplibre-gl';
import type { GeoJSONSource } from 'maplibre-gl';
import 'maplibre-gl/dist/maplibre-gl.css';
// MapLibre resolves its worker at runtime, which bundlers can't see; bundle it explicitly.
import mlWorkerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url';
import './style.css';
import { getProfile, getRoadWays, getWay, keepable, onVersions, peekWay, roadWays, setVersions, ver, version, type Drive, type Meta, type Profile, type Ride } from './api';
import { displayName, displayOf, lineName } from './names';
import { applyBoundaryOpacity, applyLabelDensity, applyLineWidths, applyOverlayOpacity, baseStyle, HER_R, LABEL_LAYERS, SLOPE4_MAX, LAYER_GROUPS, overlayLabelScale, POI_STYLE, coastInput, lakeColour, setWaterColours, labelTilesOn, ovTilesOn, waterTilesOn, stationTilesOn, versionedTiles } from './basemap';
import { setHorizonThinning } from './horizon';
import { LandmarkDots } from './dots';
import { AREA_LAYERS, landmarkRef, Overlays, POINT_LAYERS, summariseFeature, withDetails } from './overlays';
import { loadDetail, osmPath, peekDetail, refKey } from './details';
import { paletteRgb } from './palettes';
import { RoadLayer, type HoverInfo, type RoadStyle, type SchemeUniforms } from './roads/layer';
import { CASING_CLASSES_MASK, RAIL0, RAIL_GROUPS } from './config';
import { mapScheme, rgb, schemeUniforms } from './mapschemes';
import { RAIL_GROUP_COLOURS, railMetricDef, railMetricOf } from './rail';
import type { FeatureSummary } from './overlays';
import { RailCard } from './ui/rail';
import { FerryCard } from './ui/ferry';
import { StopsCard } from './ui/stops';
import { Ferries } from './ferries';
import { Stations } from './stations';
import { tasks, type Task } from './tasks';
import { idle } from './idle';
import { mapTasks } from './maptasks';
import { initHosts } from './hosts';
import { ferryMetricDef } from './ferry';
import { cdfOf, passes, scaleU } from './ui/scale';
import { applyTrees } from './trees';
import { addBuildings, applyBuildings, buildingAt, heritageChanged, heritagePoints, hiddenByBuilding, onHeights, roofAt, setHovered as setBuildingHover, summarise as buildingSummary, switchBuildings, type HeightLook, type Ray } from './buildings';
import { distFromSamples, viewStatsGen, type Dist, type Extreme, type ViewStats } from './roads/stats';
import { metricOf, modeDef } from './scenic';
import * as prefs from './prefs';
import { ROAD_WEIGHT, Store, classMask, defaults, labelShown, modeGroup, fromHash, fromSaved, groupMask, lineWeight, railMask, roadLenKm, roadLenM, surfaceMask, toHash, tollMask, unnamedHideClasses, unnamedHideGroups, type AppState, type Selection, type Stretch } from './state';
import * as cam3d from './camera3d';
import { applyLabelOpacity, applyLabelSize, applyTerrain, applyTint, cacheTerrainRays, switchContours, TINT_VARS, tintColourAt, tintCss } from './terrain';
import { applyWater, switchCoast, updateCoastRamp } from './coast';
import { CatalogWatch } from './catalog';
import { TileRetry } from './retry';
import { RegionLayers } from './regions';
import { ContourLayer, type ContourDraw } from './contours';
import { terrainDist } from './terrainstats';
import { cheaperCovers } from './covers';
import { steadierPlacement } from './placement';
import { slicedGlyphs } from './glyphs';
import { pacedDrapes } from './drape';
import { installTrackpad } from './trackpad';
import { Boot } from './ui/boot';
import { BuildStatus } from './ui/buildstatus';
import { ColourCard } from './ui/colour';
import { cap, fmt, h, toast } from './ui/dom';
import { DrivesPane } from './ui/drives';
import { LayersCard } from './ui/layers';
import { NavControls } from './ui/nav';
import { PlaceSearch } from './ui/search';
import { installListsResize, installPanelResize } from './ui/resize';
import { ProfilePanel } from './ui/profile';
import { RegionsPanel } from './ui/regions';
import { StatsCard, type InViewExtra, type ViewPlace } from './ui/stats';
import { SightsPane, type Sight } from './ui/sights';
import { LinesPane, RidesPane } from './ui/rides';
import { Strip } from './ui/strip';
import { ViewshedTool } from './ui/viewshed';
import { installPanel } from './ui/touch';
import { EVAL, evalState, installEval } from './evalmode';

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
    // The data hosts are probed while the metadata loads (hosts.ts).
    const [r] = await Promise.all([fetch('/api/meta'), initHosts()]);
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    meta = await r.json();
    setVersions(meta.versions);
  } catch (e) {
    boot.fail(0, `backend not reachable (${(e as Error).message})`);
    return;
  }

  /** Whether the catalog has the 3D buildings: their settings section, the B key and `bd=` in
   * links only then (a catalog can gain them while the map is open: newCatalog). */
  let hasBuildings = !!meta.layers?.buildings;
  // A link (URL hash) wins; otherwise restore the last session from localStorage. (A link's `bd=`
  // only with the buildings: without, the saved settings stay.)
  // (An address from when the map had a key, `#k=…`: the key dropped, the view kept.)
  if (/(^#|&)k=/.test(location.hash)) {
    const rest = location.hash.slice(1).split('&').filter((p) => p && !p.startsWith('k=')).join('&');
    history.replaceState(null, '', location.pathname + location.search + (rest ? `#${rest}` : ''));
  }
  const saved = fromSaved(prefs.load('state', null));
  const linked = location.hash.length > 1 ? { ...fromHash(location.hash, hasBuildings), ...(hasBuildings ? {} : { buildings: saved.buildings }) } : saved;
  // The shoreline check's eval mode (evalmode.ts): the link's view, nothing but land and water.
  const store = new Store(EVAL ? evalState(fromHash(location.hash, hasBuildings)) : linked);
  history.replaceState(null, '', toHash(store.s, hasBuildings)); // the address bar holds the restored state
  boot.at(1);
  maplibregl.setWorkerUrl(mlWorkerUrl);
  // MapLibre parses every tile and overlay file on a single worker by default (outside Safari):
  // the basemap and terrain tiles then wait behind a 50 MB overlay file. Several, leaving cores for
  // the road decoders and the landmarks worker.
  maplibregl.setWorkerCount(Math.max(2, Math.min(6, (navigator.hardwareConcurrency || 4) - 3)));
  // Terrain, slope and tree tiles are images: more of them at once than MapLibre's default 16, now
  // that each kind has connections of its own (hosts.ts).
  maplibregl.setMaxParallelImageRequests(32);
  const v = store.s.view;
  // (Eval mode: the map fills the window, the panels under it.)
  if (EVAL) document.getElementById('map')!.style.cssText = 'position:fixed;inset:0;z-index:1000';
  // (The water's tiles carry its colours: the user's from the start, so they're fetched once.)
  setWaterColours(store.s.water.colour, lakeColour(store.s.water.colour));
  const map = new maplibregl.Map({
    container: 'map',
    style: baseStyle(!!meta.labelTiles, store.s.labelDensity, !!meta.ovTiles, !!meta.stationTiles, !!meta.water),
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
    centerClampedToGround: false,
    // (Eval mode reads the canvas back, at its own pixel ratio, its reference finer: evalmode.ts.)
    ...(EVAL ? { canvasContextAttributes: { preserveDrawingBuffer: true }, maxCanvasSize: [16384, 16384] as [number, number], ...(EVAL.dpr ? { pixelRatio: EVAL.dpr } : {}) } : {}),
  });
  if (EVAL) installEval(map);
  // Debug: ?checks hands the map to automated checks (window.__map: its camera, after gestures).
  if (new URLSearchParams(location.search).has('checks')) (window as unknown as { __map: maplibregl.Map }).__map = map;
  // Tiles the NAS couldn't answer are asked for again (retry.ts).
  const tileRetry = new TileRetry(map);
  unlinkCameraFromTerrain(map);
  cacheTerrainRays(map);
  cheaperCovers(map);
  slicedGlyphs(map);
  pacedDrapes(map);
  if (v?.elev) map.jumpTo({ elevation: v.elev });
  // Once the terrain is ready, set the pivot for the view (camera fixed; see camera3d.relevel).
  const levelOnce = () => {
    if (map.queryTerrainElevation(map.getCenter()) === null) return;
    map.off('idle', levelOnce);
    cam3d.relevel(map);
  };
  if (store.s.terrain.on) map.on('idle', levelOnce);
  if (!v) {
    map.fitBounds([[-80.6, 40.4], [-52.6, 51.8]], { duration: 0, padding: 20 });
    // 3D terrain is on by default: start with a gentle tilt so it shows.
    if (store.s.terrain.on) map.jumpTo({ pitch: 40 });
  }
  // (A long press's menu needs the links, defined further on.)
  let onLongPress = (_px: number, _py: number) => {};
  const cameraControls = installTrackpad(map, { onLongPress: (px, py) => onLongPress(px, py) });
  installPanel(map);
  installPanelResize(map);
  // The view has settled: no camera change for SETTLE_MS. The trackpad camera moves by jumpTo, so
  // MapLibre fires movestart / moveend around every wheel event (60–120 a second in a gesture):
  // work that follows the view (in-view summaries, lists, the link) waits for this instead.
  const SETTLE_MS = 160;
  const settledFns: (() => void)[] = [];
  const onSettled = (f: () => void) => settledFns.push(f);
  let settleTimer = 0;
  let moving = false;
  let quiet = false; // camera changes that don't move the view (re-levelling the pivot)
  map.on('move', () => {
    if (quiet) return;
    if (!moving) idle.setMoving(true);
    moving = true;
    clearTimeout(settleTimer);
    settleTimer = window.setTimeout(() => {
      moving = false;
      idle.setMoving(false);
      for (const f of settledFns) f();
    }, SETTLE_MS);
  });
  steadierPlacement(map, () => moving);
  /** Work that follows the view while the camera moves too, at most every `ms` (the last move gets
   * its turn after the interval); with onSettled for when it settles. */
  const duringMoves = (f: () => void, ms: number) => {
    let at = 0, timer = 0;
    map.on('move', () => {
      if (quiet) return;
      const wait = ms - (performance.now() - at);
      if (wait <= 0) {
        at = performance.now();
        f();
      } else if (!timer) {
        timer = window.setTimeout(() => {
          timer = 0;
          at = performance.now();
          f();
        }, wait);
      }
    });
  };
  // Depth precision on the globe (see cam3d.tuneDepth): before every frame the camera moved for.
  map.on('move', () => cam3d.tuneDepth(map));
  map.on('load', () => cam3d.tuneDepth(map));
  // The camera stays a few metres above the 3D buildings' roofs, as above the ground.
  cam3d.setRoofs((ll) => roofAt(map, ll));
  map.on('terrain', () => cam3d.tuneDepth(map));

  // ---- road layer & range animation ----------------------------------------------
  const s0 = store.s;
  const roads = new RoadLayer({
    freqFilter: { on: false, min: 0, max: 0, unknown: true },
    mode: s0.mode,
    palette: s0.palette,
    range: [...s0.range] as [number, number],
    classMask: classMask(s0),
    surfaceMask: surfaceMask(s0),
    tollMask: tollMask(s0),
    unnamedHide: unnamedHideClasses(s0),
    weight: ROAD_WEIGHT * lineWeight(s0, 'roads'),
    threshold: { ...s0.threshold },
    visible: s0.layers.roads,
    weights: [...s0.weights],
    equalize: s0.equalize,
    routeGlow: s0.routeGlow,
    terrain3d: s0.terrain.on,
    exaggeration: s0.terrain.exaggeration,
    lowFade: s0.lowFade,
    lowSpan: s0.lowSpan,
    occlude: s0.occlude,
    opacity: s0.roadOpacity,
    lenMin: roadLenM(s0)[0],
    lenMax: roadLenM(s0)[1],
    direct: 0,
    scheme: null,
    single: [1, 1, 1],
    modeId: null,
    railMask: 0,
    railWeights: [],
    casingMask: CASING_CLASSES_MASK,
  });
  // The tiles' version in their URLs (cached for good): the catalog's for the layer, else none. (Not
  // the build time in meta: it stayed when the tiles' way column changed from indices to OSM ids,
  // and a browser holding the older tiles under the same URLs would have kept them.)
  roads.setSource(version('roads.tiles'), meta.bounds);
  // Street-map colours (Map display type), rebuilt when the scheme changes.
  let schemeKey = '';
  let schemeU: SchemeUniforms | null = null;
  const applyMapMode = (s: AppState) => {
    const st = roads.style;
    const on = s.mode === 'map';
    if (on && schemeKey !== s.mapScheme) {
      schemeKey = s.mapScheme;
      schemeU = schemeUniforms(mapScheme(s.mapScheme));
    }
    st.direct = on ? 1 : 0;
    st.scheme = on ? schemeU : null;
    // Street maps case every road when zoomed in.
    st.casingMask = on ? 0x1ff : CASING_CLASSES_MASK;
  };
  applyMapMode(s0);

  // ---- passenger rail -------------------------------------------------------------
  const railScheme: SchemeUniforms = (() => {
    const u = schemeUniforms(mapScheme('mono'));
    RAIL_GROUP_COLOURS.forEach((c, k) => {
      u.classCol.set([...rgb(c), 1], (RAIL0 + k) * 4);
      u.classCas.set(rgb('#07090c'), (RAIL0 + k) * 3);
    });
    return u;
  })();
  const rails = new RoadLayer(
    // Drawn as railways (a thin line with cross-ties: the layer's rail pattern), cased when zoomed in.
    { ...roads.style, classMask: 0x7c00, casingMask: 0x7c00, surfaceMask: 3, tollMask: 3, unnamedHide: 0, threshold: { on: false, dir: 'above', value: 0 }, routeGlow: false, lenMin: 0, lenMax: Infinity, equalize: false, opacity: 1 },
    {
      id: 'rails',
      rail: true,
      metric: (_st, e, g, ch, ground, style, fq) =>
        railMetricOf(store.s.rail.metric, { elev: e, grade: g, ground, bridge: (style & 96) === 32, tunnel: (style & 64) !== 0, ch, freq: fq }, store.s.rail.weights),
    },
  );
  rails.setSource(version('rails.tiles'), meta.bounds);
  // The tilted tile cover unprojects onto the terrain (not the camera pivot's level).
  roads.groundSamples = rails.groundSamples = (pts) => cam3d.coverSamples(map, pts);
  let railCur: [number, number] = [...s0.rail.range] as [number, number];
  // Rail equalisation lookup (from the rail distribution in view), when on.
  let railCdf: Uint8Array | null = null;
  let railCdfKey = '';
  const applyRailStyle = (s: AppState) => {
    const r = s.rail, st: RoadStyle = rails.style;
    st.visible = r.on;
    st.railMask = railMask(s);
    st.direct = r.colour === 'line' ? 2 : r.colour === 'group' ? 3 : r.colour === 'single' ? 4 : 0;
    st.scheme = railScheme;
    st.single = rgb(r.single);
    st.modeId = railMetricDef(r.metric).id;
    st.palette = r.palette;
    st.railWeights = [...r.weights];
    st.freqFilter = { on: r.freqOn, min: r.freqMin, max: r.freqMax, unknown: r.freqUnknown };
    st.weight = lineWeight(s, 'rail');
    st.opacity = r.opacity;
    const metric = r.colour === 'metric';
    st.lowFade = metric ? r.lowFade : 0;
    st.lowSpan = r.lowSpan;
    st.equalize = metric && r.equalize;
    st.threshold = metric ? { ...r.threshold } : { on: false, dir: 'above', value: 0 };
    st.terrain3d = s.terrain.on;
    st.exaggeration = s.terrain.exaggeration;
    st.occlude = s.occlude;
    st.range = railCur;
  };
  applyRailStyle(s0);
  // Rail service frequency per way (railfreq): sorted OSM way ids (as the rail tiles' way column)
  // and trains a day each way. Loaded for a version of the file (again for a new catalog's); a
  // failed request isn't kept (asked again the next time rail changes).
  let railFreqFor: string | null = null;
  const loadRailFreq = () => {
    const v = version('rail-freq.bin');
    if (railFreqFor === v) return;
    railFreqFor = v;
    const req = fetch(`/api/railfreq${ver('rail-freq.bin')}`).then(keepable).then((r) => (r.ok ? r.arrayBuffer() : null));
    tasks.track('railfreq', 'Rail frequencies', req, 'trains a day per line')
      .then((b) => {
        if (railFreqFor !== v || !b || b.byteLength < 8) return;
        const n = b.byteLength / 8;
        const dv = new DataView(b);
        // Negative: a lower bound (MTR lines with only published headways).
        const ways = new Uint32Array(n), vals = new Float32Array(n);
        for (let i = 0; i < n; i++) {
          ways[i] = dv.getUint32(i * 8, true);
          vals[i] = dv.getFloat32(i * 8 + 4, true);
        }
        const find = (w: number) => {
          let lo = 0, hi = n - 1;
          while (lo <= hi) {
            const m = (lo + hi) >> 1;
            if (ways[m] < w) lo = m + 1;
            else if (ways[m] > w) hi = m - 1;
            else return m;
          }
          return -1;
        };
        rails.setLineValues((w) => {
          const i = find(w);
          return i < 0 ? -1 : Math.abs(vals[i]);
        }, (w) => {
          const i = find(w);
          return i >= 0 && vals[i] < 0;
        });
      })
      .catch(() => {
        if (railFreqFor === v) railFreqFor = null;
      });
  };
  if (s0.rail.on) loadRailFreq();

  let stats: ViewStats | null = null;
  let railStats: ViewStats | null = null;
  /** Distribution of the rail colour metric over rail in view. */
  let railDist: Dist | null = null;
  /** Rail in view by trains a day (log10), for the frequency filter's histogram. */
  let railFreqDist: Dist | null = null;
  /** Distribution of the current colour metric over roads in view. */
  let mdist: Dist | null = null;
  let viewExtra: InViewExtra = {};
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
    if (modeGroup(s.mode) === 'scenic') return mdist && mdist.total > 0 && stats && stats.totalKm > 0 ? byLen(mdist, stats.totalKm, s.fitLen, d.step) : cur;
    const [pl, ph] = s.fit;
    return mdist && mdist.total > 0 ? spread(mdist.quantile(pl / 100), mdist.quantile(ph / 100), d.step * 4) : cur;
  };
  /** The best fitLen[0] screen widths of line in view to the best fitLen[1] (the roads' scenic
   * metrics, rail's and ferries' ranked ones): a fixed amount of line at the view centre's scale,
   * whatever share of the view it is. `dist`: the metric over the lines in view; `km`: their length. */
  const byLen = (dist: Dist, km: number, fitLen: [number, number], step: number): [number, number] => {
    const c = map.getCenter();
    const widthKm = ((40075.016686 * Math.cos((c.lat * Math.PI) / 180)) / (512 * 2 ** map.getZoom())) * map.getCanvas().clientWidth;
    const at = (widths: number) => dist.quantile(Math.max(0, 1 - (widths * widthKm) / km));
    return spread(at(fitLen[0]), at(fitLen[1]), step * 4);
  };
  const spread = (lo: number, hi: number, min: number): [number, number] => {
    if (hi - lo < min) {
      const m = (lo + hi) / 2;
      return [m - min / 2, m + min / 2];
    }
    return [lo, hi];
  };

  /**
   * The In view statistics; `full`: everything, else (while the camera moves) only what the colour
   * ranges follow: the current metric's distribution, from fewer samples. The rest (km, the other
   * metrics, landmarks, summit, ferries, busiest line) stays from the last settled view. A tile at a
   * time (a generator): the frame loop runs it a few milliseconds a frame (at 120 Hz a frame has
   * 8 ms, and a full pass over a dense view took 40 ms), and the results take effect once it ends.
   */
  function* computeStats(full = true): Generator<void, void> {
    const s = store.s;
    const nSamples = full ? 90_000 : 15_000;
    const d = modeDef(s.mode);
    const fromSketches = s.mode === 'elev' || s.mode === 'relief' || s.mode === 'grade';
    let st = stats, md = mdist, extra = viewExtra, rst = railStats, rdist = railDist, rfd = railFreqDist;
    if (full || fromSketches || !st) {
      const lr = roadLenM(s);
      st = yield* viewStatsGen(roads, groupMask(s), classMask(s), surfaceMask(s), unnamedHideGroups(s), lr[0] > 0 || lr[1] < Infinity ? lr : null, tollMask(s));
    }
    if (fromSketches) {
      md = s.mode === 'grade' ? st!.grade : st!.elev;
    } else if (!full) {
      const smp = yield* roads.metricSamplesGen([s.mode], s.weights, nSamples);
      md = distFromSamples(smp.v[0], smp.w, ...d.domain);
    }
    if (full) {
      // Scenic summary for the stats card + the current metric's distribution, in one pass.
      const modes = ['score', 'openness', 'trees', 'vista', 'bldg'] as const;
      const scenicMode = d.id >= 3 && !(modes as readonly string[]).includes(s.mode) ? [s.mode] : [];
      const smp = yield* roads.metricSamplesGen([...modes, ...scenicMode], s.weights, nSamples);
      const dists = modes.map((m, i) => distFromSamples(smp.v[i], smp.w, ...modeDef(m).domain));
      if (!fromSketches) {
        const i = (modes as readonly string[]).indexOf(s.mode);
        md = i >= 0 ? dists[i] : distFromSamples(smp.v[modes.length], smp.w, ...d.domain);
      }
      const [sc, , , vi] = dists;
      extra = inViewExtra(sc, vi);
    }
    // Rail: km per service group, and the colour metric's distribution.
    const r = s.rail;
    if (full || !rst) rst = r.on ? yield* viewStatsGen(rails, 31, 0x7c00, 3, 0, null) : null;
    if (r.on && r.colour === 'metric') {
      const rd = railMetricDef(r.metric);
      const smpR = yield* rails.sampleWithGen([(e, g, ch, ground, style, fq) =>
        railMetricOf(r.metric, { elev: e, grade: g, ground, bridge: (style & 96) === 32, tunnel: (style & 64) !== 0, ch, freq: fq }, r.weights)], nSamples);
      // Lines without a timetable have no frequency (NaN): left out of the distribution.
      const keep = smpR.v[0].map((v) => (Number.isNaN(v) ? 0 : 1));
      const vv = smpR.v[0].filter((_, i) => keep[i]), ww = smpR.w.filter((_, i) => keep[i]);
      rdist = distFromSamples(vv, ww, ...rd.domain);
    } else rdist = null;
    if (r.on && (full || !rfd)) {
      // Lines without a timetable (NaN or none) are left out.
      const smpF = yield* rails.sampleWithGen([(_e, _g, _ch, _ground, _style, fq) => (fq > 0 ? Math.log10(fq) : NaN)], nSamples);
      const keep = smpF.v[0].map((v) => (Number.isNaN(v) ? 0 : 1));
      rfd = distFromSamples(smpF.v[0].filter((_, i) => keep[i]), smpF.w.filter((_, i) => keep[i]), Math.log10(0.5), Math.log10(3000), 256);
    } else if (!r.on) rfd = null;
    stats = st;
    mdist = md;
    viewExtra = extra;
    railStats = rst;
    railDist = rdist;
    railFreqDist = rfd;
  }
  /** The statistics pass under way (computeStats), run a few ms a frame by the frame loop, and
   * whether it is a full one (dropped when the camera moves: the view it was for is gone). */
  let statsJob: Generator<void, void> | null = null;
  let statsJobFull = false;
  let lastFullStats = 0;
  /** Time a pass may take a frame (ms): at rest, and while the camera moves (the frame's own work
   * then fills most of its 8 ms). */
  const STATS_MS = 3;
  const STATS_MOVING_MS = 1;

  const updateRailCdf = () => {
    const r = store.s.rail;
    if (!r.equalize || r.colour !== 'metric' || !railDist || railDist.total <= 0) {
      if (railCdf) {
        railCdf = null;
        railCdfKey = '';
        rails.setCdf(null);
      }
      return;
    }
    const key = `${r.metric}|${railCur[0].toFixed(3)}|${railCur[1].toFixed(3)}|${railDist.total.toFixed(0)}`;
    if (key === railCdfKey) return;
    railCdfKey = key;
    railCdf = cdfOf(railDist, railCur);
    rails.setCdf(railCdf);
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

  // Terrain tint: a colour scale like the roads' (ui/scale.ts), over the terrain in view (its
  // histogram from the DEM tiles drawn, terrainstats.ts). Its range eases to the target as the
  // road colours' does (frame loop), and the terrain is measured again every few hundred
  // milliseconds while the view changes, mid-gesture too.
  let terrainD: Dist | null = null;
  let terrainDirty = true;
  let lastTerrain = 0;
  let tintCur: [number, number] = [...s0.terrain.tintScales[s0.terrain.tintVar].range] as [number, number];
  let tintSnap = true;
  let tintCdf: Uint8Array | null = null;
  let tintCdfKey = '';
  let tintPreview: string | null = null;
  let tintKey = `${s0.terrain.tint}|${s0.terrain.tintVar}`;
  /** Roads coloured so that the tint can follow their range: by elevation (or grade, for slope). */
  const tintMatchable = () => (store.s.terrain.tintVar === 'slope' ? store.s.mode === 'grade' : store.s.mode === 'elev' || store.s.mode === 'relief');
  const tintScale = () => {
    const t = store.s.terrain;
    const sc = t.tintScales[t.tintVar];
    return tintPreview ? { ...sc, palette: tintPreview } : sc;
  };
  const tintTarget = (): [number, number] => {
    const t = store.s.terrain;
    const sc = t.tintScales[t.tintVar];
    if (t.tintMatch && tintMatchable()) return cur;
    if (!sc.auto) return sc.range;
    if (!terrainD || terrainD.total <= 0) return tintCur;
    return spread(terrainD.quantile(sc.fit[0] / 100), terrainD.quantile(sc.fit[1] / 100), TINT_VARS[t.tintVar].step * 4);
  };
  const measureTerrain = () => {
    const t = store.s.terrain;
    lastTerrain = performance.now();
    terrainDirty = false;
    if (!t.tint || !styleReady) return;
    const slope = t.tintVar === 'slope';
    const d = terrainDist(map, slope ? 'slope' : 'dem-hs', TINT_VARS[t.tintVar].domain, groundOutline().map((ll) => [ll.lng, ll.lat] as [number, number]), slope ? { max: SLOPE4_MAX } : null);
    if (d) terrainD = d;
  };
  const updateTintCdf = () => {
    const sc = store.s.terrain.tintScales[store.s.terrain.tintVar];
    if (!sc.equalize || !terrainD || terrainD.total <= 0) {
      tintCdf = null;
      tintCdfKey = '';
      return;
    }
    const key = `${tintCur[0].toPrecision(4)}|${tintCur[1].toPrecision(4)}|${terrainD.total.toPrecision(6)}`;
    if (key === tintCdfKey) return;
    tintCdfKey = key;
    tintCdf = cdfOf(terrainD, tintCur);
  };
  let tintAt: ((v: number) => [[number, number, number], number]) | null = null;
  const refreshTint = () => {
    if (!styleReady) return;
    updateTintCdf();
    const t = store.s.terrain;
    applyTint(map, t, tintScale(), tintCur, tintCdf, store.s.palette, tintCdfKey);
    tintAt = null;
  };

  const wireTintPreview = () => {
    layers.tintCssFor = (key) => tintCss(store.s.terrain, { ...tintScale(), palette: key }, tintCur, store.s.palette);
    layers.tintColourAt = (v) => {
      tintAt ??= tintColourAt(store.s.terrain, tintScale(), tintCur, tintCdf, store.s.palette);
      const [c, a] = tintAt(v);
      return [`rgb(${c.map((x) => Math.round(Math.max(0, Math.min(1, x)) * 255)).join(',')})`, a];
    };
    layers.tintMatchAvailable = tintMatchable;
    layers.onTintPreview = (key) => {
      tintPreview = key;
      refreshTint();
      layers.updateTint(terrainD, tintCur, tintCdf);
    };
    layers.sync(store.s);
  };

  // ---- UI ------------------------------------------------------------------------
  const km = Math.round(meta.elev_hist_10m_km.reduce((a, b) => a + b, 0));
  const colour = new ColourCard(document.getElementById('colour')!, store, `${fmt.n(km)} km of drivable public road`);
  const railCard = new RailCard(store);
  const ferryCard = new FerryCard(store);
  const stopsCard = new StopsCard(store);
  // Palette previews while hovering a ramp list (null: back to the chosen palette).
  colour.onPalettePreview = (k) => {
    roads.style.palette = k ?? store.s.palette;
    map.triggerRepaint();
  };
  railCard.onPalettePreview = (k) => {
    rails.style.palette = k ?? store.s.rail.palette;
    map.triggerRepaint();
  };
  ferryCard.onPalettePreview = (k) => ferries.apply(k ? { ...store.s, ferry: { ...store.s.ferry, palette: k } } : store.s);
  // Every setting in the left panel, a section per layer, each with how the layer is coloured.
  const layersRoot = document.createElement('div');
  layersRoot.id = 'layers';
  document.getElementById('colour')!.append(layersRoot);
  const layers = new LayersCard(layersRoot, store, { roads: colour.el, rail: railCard.el, ferry: ferryCard.el, stops: stopsCard.el });
  layers.showBuildings(hasBuildings);
  wireTintPreview();
  const statsEl = document.getElementById('stats')!;
  const statsCard = new StatsCard(statsEl);
  installListsResize(statsEl);
  const drives = new DrivesPane(statsCard.drivesRoot);
  const sights = new SightsPane(statsCard.sightsRoot);
  const rides = new RidesPane(statsCard.ridesRoot);
  const lines = new LinesPane(statsCard.linesRoot);
  const strip = new Strip(document.getElementById('strip')!, map, () => store.s.weights);
  strip.railWeights = () => store.s.rail.weights;
  const profile = new ProfilePanel(document.getElementById('profile')!);
  new NavControls(document.getElementById('nav')!, map, cameraControls);
  // Place search, beside the view controls (ui/search.ts): where it goes is marked below.
  const search = new PlaceSearch(document.getElementById('search')!, () => {
    const c = map.getCenter();
    return [c.lng, c.lat];
  });
  const viewshed = new ViewshedTool(document.getElementById('viewshed')!, map);
  // Regions (Layers → Regions): the regions the map is built for, their coverage on the map, and new
  // ones made of administrative areas.
  const regions = new RegionsPanel(new RegionLayers(map));
  layers.addSection('regions', 'Regions', regions.nodes, (open) => regions.setOpen(open));
  // The landmark dots, drawn on the GPU (dots.ts); the overlays feed them.
  const dots = new LandmarkDots();
  // The 3D buildings' heritage tint takes the heritage sites' points from the dots, as shown.
  heritagePoints(map, (box) => dots.points('heritage', box));
  dots.onChange = (id) => {
    if (id === 'heritage') heritageChanged(map, store.s.overlays.heritage);
  };
  dots.setTerrain({ on: store.s.terrain.on, exaggeration: store.s.terrain.exaggeration, occlude: store.s.occlude });
  // Contour lines (contours.ts), from the terrain settings and the global line weight.
  const contours = new ContourLayer();
  const contourStyle = (s: AppState): Partial<ContourDraw> => {
    const t = s.terrain, c = t.contour, k = s.lineWeights.global * c.weight;
    return {
      on: t.contours, terrain3d: t.on, exaggeration: t.exaggeration, colour: c.colour, opacity: [c.minor, c.major],
      width: [0.5 * k, 0.9 * k], perspective: c.perspective, ring: c.ring,
    };
  };
  contours.set(contourStyle(store.s));
  const overlays = new Overlays(map, layers, dots);
  overlays.onScale = (dist, range, cdf) => stopsCard.update(dist, range, cdf);
  overlays.onView = () => {
    markDirty();
    sights.refresh();
  };
  sights.kinds = () => overlays.viewLandmarks.filter((l) => store.s.overlays[l.key]);
  sights.query = (k) => overlays.topInView(k);
  sights.onSelect = (x) => overlays.select(x);
  sights.onHover = (x) => {
    marks.high = x ? point(x.lngLat, 'high') : null;
    setMarks();
    if (x) sightOsm(x); // ready for O
    listHover(x ? { feature: { layer: x.layer, props: x.props, lngLat: x.lngLat } } : null);
  };
  /** The In view summary beyond roads: rail and ferries, landmarks and terrain (each while shown). */
  const scenicExtra = (sc: Dist | null | undefined, vi: Dist | null | undefined): InViewExtra => ({
    scenic: sc && sc.total > 0 ? [sc.quantile(0.5), sc.quantile(0.9)] : null,
    vista: vi && vi.total > 0 ? [vi.quantile(0.5), vi.quantile(0.9)] : null,
  });
  const inViewExtra = (sc: Dist | null | undefined, vi: Dist | null | undefined): InViewExtra => {
    const s = store.s;
    const x: InViewExtra = scenicExtra(sc, vi);
    if (s.rail.on && railStats) {
      const groups = RAIL_GROUPS.map((g, k) => ({ label: g.label, colour: RAIL_GROUP_COLOURS[k], km: railStats!.classKm[RAIL0 + k] }));
      const total = groups.reduce((a, g) => a + g.km, 0);
      const b = rails.busiestInView();
      x.rail = { total, groups, busiest: b ? { name: railName.get(b.way) ?? 'Rail line', perDay: b.perDay, lngLat: b.lngLat } : null, highest: railStats.highest };
      if (b && !railName.has(b.way)) getWay(b.way, b.lngLat).then((info) => {
        // The line's name without a route's direction or service codes ("Highland Sleeper").
        railName.set(b.way, (info && lineName(info)) || 'Rail line');
        markDirty();
      });
    }
    if (s.ferry.on) x.ferry = ferries.viewSummary();
    const lms = overlays.viewLandmarks.filter((l) => s.overlays[l.key]);
    if (lms.length) x.landmarks = lms;
    const summit = overlays.summitInView();
    const hi = Math.max(summit?.ele ?? -Infinity, stats?.highest?.elev ?? -Infinity);
    const lo = stats?.lowest?.elev;
    x.terrain = {
      summit,
      relief: Number.isFinite(hi) && lo !== undefined ? hi - lo : null,
      above1000: stats?.elev ? stats.elev.above(1000) : null,
    };
    return x;
  };
  /** The busiest lines' names as shown (by way). */
  const railName = new Map<number, string>();
  statsCard.onPlace = (pl: ViewPlace) => {
    if (pl.layer) overlays.select({ lngLat: pl.lngLat, layer: pl.layer, props: pl.props });
    else map.flyTo({ center: pl.lngLat, zoom: Math.max(map.getZoom(), 11), duration: 900 });
  };
  statsCard.onPlaceHover = (pl: ViewPlace | null) => {
    marks.high = pl ? point(pl.lngLat, 'high') : null;
    setMarks();
    if (!pl) return listHover(null);
    if (pl.layer) return listHover({ feature: { layer: pl.layer, props: pl.props ?? {}, lngLat: pl.lngLat } });
    if (pl === viewExtra.rail?.busiest) return listHover({ layer: rails, at: pl.lngLat });
    if (pl === viewExtra.ferry?.busiest) return listHover({ ferryAt: pl.lngLat });
    listHover(null);
  };
  profile.colour = () => ({ palette: store.s.palette, mode: store.s.mode, range: cur, weights: store.s.weights, cdf: store.s.equalize ? cdf : null });
  const ferries = new Ferries(map);
  ferries.byBlocks = !!meta.ferryBlocks;
  const stations = new Stations(map);
  const updateFerries = () => {
    if (!ferries.loaded || !store.s.ferry.on) {
      layers.updateFerry(null);
      ferryCard.update(null);
      return;
    }
    const v = ferries.inView();
    layers.updateFerry(v.km);
    ferryCard.update(v.cov);
    // The metric scale: distribution of the ferry lines in view and the auto-fitted range, which
    // the frame loop eases to (ferryEase).
    const f = store.s.ferry;
    const d = ferryMetricDef(f.metric);
    ferryDist = ferries.metricDist();
    // (its weights are the lines' km in view)
    ferryTarget = !f.auto || !ferryDist || ferryDist.total <= 0 ? f.range
      : d.byLen ? byLen(ferryDist, ferryDist.total, f.fitLen, d.step)
      : spread(ferryDist.quantile(f.fit[0] / 100), ferryDist.quantile(f.fit[1] / 100), d.step * 4);
    if (!ferryCur || ferryKey !== f.metric) {
      ferryKey = f.metric;
      ferryCur = ferryTarget;
      ferryEase(true);
    }
    wake();
  };
  let ferryDist: Dist | null = null;
  let ferryTarget: [number, number] | null = null;
  let ferryCur: [number, number] | null = null;
  let ferryKey = '';
  let ferryPainted = 0;
  /** The ferry scale's eased range applied: the lines' colours are a data-driven expression (each
   * change lays the ferry source out again), so at most every 100 ms while it eases. */
  const ferryEase = (force = false) => {
    if (!ferryCur) return;
    const now = performance.now();
    if (!force && now - ferryPainted < 100) return;
    ferryPainted = now;
    const fcdf = store.s.ferry.equalize ? cdfOf(ferryDist, ferryCur) : null;
    ferries.setScale(ferryCur, fcdf);
    ferryCard.updateScale(ferryDist, ferryCur, fcdf);
  };
  ferries.onLoaded = updateFerries;
  onSettled(updateFerries);
  duringMoves(updateFerries, 300);
  // Landmark prominence: the histogram (and an auto-fitted range) follow the landmarks in view, while
  // the camera moves too (a query at a time; the dots ease to each new scale).
  onSettled(() => overlays.prominence(store.s));
  duringMoves(() => overlays.prominenceSoon(store.s), 300);

  // Markers: profile cursor, highest / lowest road in view, viewshed eye, a place searched for.
  const marks: Record<string, GeoJSON.Feature | null> = { cursor: null, high: null, low: null, viewshed: null, ring: null, place: null };
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
  // A place found: the map goes there, at the zoom that shows it, and marks it (named by the line
  // under its label when it has one, its English mostly), until the search's words go.
  search.onGo = (p) => {
    const ll = new maplibregl.LngLat(p.lon, p.lat);
    marks.place = point([p.lon, p.lat], 'place', p.sub || p.main);
    setMarks();
    map.flyTo({ ...cam3d.frame(map, ll, map.queryTerrainElevation(ll) ?? 0, p.zoom), duration: 1500 });
  };
  search.onClear = () => {
    marks.place = null;
    setMarks();
  };
  statsCard.onMark = (x, kind) => {
    marks[kind] = x ? point(x.lngLat, kind, fmt.m(x.elev)) : null;
    setMarks();
    listHover(x ? { layer: ((x.tile.data?.lineStyle[x.line] ?? 0) & 15) >= RAIL0 ? rails : roads, at: x.lngLat, way: x.tile.data?.lineWay[x.line] } : null);
  };
  statsCard.onFly = (x: Extreme) => {
    const ll = maplibregl.LngLat.convert(x.lngLat);
    map.flyTo({ ...cam3d.frame(map, ll, map.queryTerrainElevation(ll) ?? 0, Math.max(map.getZoom(), 14)), duration: 1200 });
    const kind = x === stats?.highest ? 'high' : 'low';
    marks[kind] = point(x.lngLat, kind, fmt.m(x.elev));
    setMarks();
    setTimeout(() => {
      marks[kind] = null;
      setMarks();
    }, 6000);
  };
  // The profile panel covers the map's bottom.
  const fitPad = { top: 60, bottom: 270, left: 60, right: 60 };
  /** fitBounds, but framed from the ground at the centre rather than from sea level (cam3d.frame). */
  const fitGround = (b: maplibregl.LngLatBounds, padding: maplibregl.PaddingOptions, maxZoom?: number) => {
    // (Keeping the bearing; maxZoom only when given: MapLibre takes an undefined one for a limit,
    // and the camera comes out NaN.)
    const cam = map.cameraForBounds(b, { padding, bearing: map.getBearing(), ...(maxZoom !== undefined ? { maxZoom } : {}) });
    if (!cam?.center || cam.zoom === undefined) return;
    const ll = maplibregl.LngLat.convert(cam.center);
    map.flyTo({ ...cam3d.frame(map, ll, map.queryTerrainElevation(ll) ?? 0, cam.zoom), duration: 900 });
  };
  const bboxQuery = () => {
    const b = map.getBounds();
    return [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()].map((x) => x.toFixed(5)).join(',');
  };
  /** Outline of the ground in view, for the lists "in view" (scenic drives, climbs): the screen
   * unprojected, up to where the ground gets too foreshortened to see (a pixel covering more than
   * 4× the ground it covers at the centre). A tilted view is a trapezoid whose bounding box takes in
   * far more (Portugal from Québec), and near the horizon a sliver of another continent can show. */
  // Computed once per camera (several "in view" summaries ask for it), on the terrain-free globe or
  // plane: map.unproject ray-marches the 3D terrain on the CPU, and a thousand of those per outline
  // took most of the frame budget.
  let outlineKey = '';
  let outline: maplibregl.LngLat[] = [];
  const groundOutline = (): maplibregl.LngLat[] => {
    const c = map.getCanvas();
    const W = c.clientWidth, H = c.clientHeight, N = 10;
    const ctr = map.getCenter();
    const key = `${W}x${H}|${ctr.lng.toFixed(6)},${ctr.lat.toFixed(6)}|${map.getZoom().toFixed(4)}|${map.getBearing().toFixed(2)}|${map.getPitch().toFixed(2)}|${map.getCenterElevation().toFixed(0)}`;
    if (key === outlineKey) return outline;
    type Tr = { screenPointToLocation?: (p: maplibregl.Point) => maplibregl.LngLat };
    const m = map as unknown as { _camera?: { transform?: Tr }; transform?: Tr };
    const tr = m._camera?.transform ?? m.transform;
    const ground = (x: number, y: number) => {
      const ll = tr?.screenPointToLocation ? tr.screenPointToLocation(new maplibregl.Point(x, y)) : map.unproject([x, y]);
      return ll && Number.isFinite(ll.lng) && Number.isFinite(ll.lat) ? maplibregl.LngLat.convert(ll) : null;
    };
    const mpp = (x: number, y: number) => {
      const a = ground(x, y), b = ground(x, y - 4);
      return a && b ? a.distanceTo(b) / 4 : Infinity;
    };
    const limit = 4 * Math.max(1e-3, mpp(W / 2, H / 2));
    // Highest usable point in each column (ground resolution only coarsens toward the horizon):
    // a binary search up from the bottom edge.
    const top = (x: number) => {
      if (mpp(x, 4) <= limit) return 0;
      let lo = 4, hi = H; // mpp(lo) > limit, mpp(hi) ≤ limit (or the bottom edge)
      while (hi - lo > H / 64) {
        const m = (lo + hi) / 2;
        if (mpp(x, m) > limit) lo = m;
        else hi = m;
      }
      return hi;
    };
    const xs = Array.from({ length: N + 1 }, (_, i) => (W * i) / N);
    const tops = xs.map(top);
    const pts: [number, number][] = [];
    xs.forEach((x) => pts.push([x, H])); // bottom, left → right
    for (let i = 1; i < N; i++) pts.push([W, H + ((tops[N] - H) * i) / N]); // right edge, up
    for (let i = N; i >= 0; i--) pts.push([xs[i], tops[i]]); // top, right → left
    for (let i = N - 1; i > 0; i--) pts.push([0, H + ((tops[0] - H) * i) / N]); // left edge, down
    outlineKey = key;
    outline = pts.map(([x, y]) => ground(x, y)).filter((ll): ll is maplibregl.LngLat => !!ll);
    return outline;
  };
  const polyQuery = () => groundOutline().map((ll) => `${ll.lng.toFixed(4)},${ll.lat.toFixed(4)}`).join(',');
  // Zoomed-out lists (docs/phase5.md "Zoomed-out queries"): from the server's summaries when the
  // outline's box is wider than 1,200 km, exact again below 900 km (between, as it was: panning or a
  // resize doesn't flip it), one mode for drives, rides and rail lines.
  let approxLists: boolean | null = null;
  const approxQuery = (): boolean => {
    const o = groundOutline();
    if (o.length < 3) return approxLists ?? false;
    let [w, s, e, n] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const p of o) [w, s, e, n] = [Math.min(w, p.lng), Math.min(s, p.lat), Math.max(e, p.lng), Math.max(n, p.lat)];
    const km = Math.max((e - w) * 111.32 * Math.cos((((s + n) / 2) * Math.PI) / 180), (n - s) * 111.32);
    approxLists = approxLists === null ? km > 1050 : approxLists ? km > 900 : km > 1200;
    return approxLists;
  };
  // Landmarks and ferries "in view" use the same outline (the bounding box of a globe or tilted
  // view takes in far more: Toronto from northern British Columbia).
  overlays.viewOutline = () => groundOutline().map((ll) => [ll.lng, ll.lat] as [number, number]);
  ferries.viewOutline = overlays.viewOutline;

  // A climb or scenic drive picked from a list is a stretch of the selected road (store.s.stretch,
  // so links carry it). Its line on the map: the list's geometry, or, when it came from a link,
  // the stretch cut out of the road's profile once loaded.
  let pinned: [number, number][] | null = null;
  let profileCoords: [number, number][] | null = null;
  const cut = (coords: [number, number][], a: [number, number], b: [number, number]) => {
    const near = (q: [number, number]) => {
      const k = Math.cos((q[1] * Math.PI) / 180);
      let best = 0, bd = Infinity;
      coords.forEach((c, i) => {
        const d = ((c[0] - q[0]) * k) ** 2 + (c[1] - q[1]) ** 2;
        if (d < bd) [bd, best] = [d, i];
      });
      return best;
    };
    const [i, j] = [near(a), near(b)].sort((x, y) => x - y);
    return coords.slice(i, j + 1);
  };
  const setClimb = (g: [number, number][] | null) => map.getSource<GeoJSONSource>('climb')?.setData(line(g));
  const setDriveHl = (g: [number, number][] | null) => map.getSource<GeoJSONSource>('drive-hl')?.setData(line(g));
  const drawStretch = () => {
    const st = store.s.selected !== null ? store.s.stretch : null;
    if (st && !pinned && profileCoords) pinned = cut(profileCoords, st.a, st.b);
    setClimb(st?.kind === 'climb' ? pinned : null);
    setDriveHl(st?.kind === 'drive' ? pinned : null);
  };
  const applyStretch = () => {
    const st = store.s.selected !== null ? store.s.stretch : null;
    if (!st) pinned = null;
    profile.highlight(st ? { start: st.a, end: st.b, label: st.label } : null);
    drawStretch();
  };
  const driveStretch = (d: Drive): Stretch => ({
    kind: 'drive', a: d.geom[0], b: d.geom[d.geom.length - 1], label: `Scenic ${d.score.toFixed(0)} · ${fmt.dist(d.length_m)}`,
  });
  const boundsOf = (g: [number, number][]) => {
    const b = new maplibregl.LngLatBounds();
    for (const q of g) b.extend(q);
    return b;
  };
  /** A drive or ride picked from a list: its road or line selected (by its first way and the point
   * the list gives on it) and the stretch marked, the map staying where it is. A zoomed-out list's
   * geometry (its summaries' samples, 500 m apart) isn't kept: the stretch is cut from the road's
   * profile once loaded, as a link's is. */
  const pickStretch = (sel: Selection, st: Stretch, geom: [number, number][] | null) => {
    store.set({ selected: sel, stretch: st });
    pinned = geom;
    drawStretch();
  };

  // Scenic drives.
  drives.query = () => ({ bbox: bboxQuery(), poly: polyQuery(), classes: classMask(store.s), surface: surfaceMask(store.s), toll: tollMask(store.s), unnamed: unnamedHideClasses(store.s), len: roadLenKm(store.s), weights: store.s.weights, approx: approxQuery() });
  drives.onResults = (ds) =>
    map.getSource<GeoJSONSource>('drives')?.setData({
      type: 'FeatureCollection',
      features: ds.map((d) => ({ type: 'Feature', properties: { score: d.score }, geometry: { type: 'LineString', coordinates: d.geom } })),
    });
  drives.onHover = (d) => {
    listHover(d ? { layer: roads, at: midpoint(d.geom), way: d.way, geom: [d.geom] } : null);
    return d ? setDriveHl(d.geom) : drawStretch();
  };
  drives.onSelect = (d) => pickStretch({ way: d.way, at: d.at }, driveStretch(d), d.approx ? null : d.geom);

  // Scenic rides and rail lines: the rail weights and service groups shown.
  const railQuery = () => ({ bbox: bboxQuery(), poly: polyQuery(), weights: store.s.rail.weights, groups: railMask(store.s), approx: approxQuery() });
  rides.query = railQuery;
  lines.query = railQuery;
  const rideStretch = (r: Ride): Stretch => ({
    kind: 'drive', a: r.geom[0], b: r.geom[r.geom.length - 1], label: `Scenic ride ${r.score.toFixed(0)} · ${fmt.dist(r.length_m)}`,
  });
  rides.onResults = (rs) => drives.onResults(rs.map((r) => ({ score: r.score, geom: r.geom }) as unknown as Drive));
  rides.onHover = (r) => {
    listHover(r ? { layer: rails, at: midpoint(r.geom), geom: [r.geom] } : null);
    return r ? setDriveHl(r.geom) : drawStretch();
  };
  rides.onSelect = (r) => pickStretch({ way: r.way, at: r.at }, rideStretch(r), r.approx ? null : r.geom);
  lines.onHover = (l) => {
    listHover(l ? { layer: rails, at: midpoint(l.geom.reduce((a, b) => (lineLen(b) > lineLen(a) ? b : a))), geom: l.geom } : null);
    map.getSource<GeoJSONSource>('drive-hl')?.setData(l ? { type: 'Feature', properties: {}, geometry: { type: 'MultiLineString', coordinates: l.geom } } : line(null));
    if (!l) drawStretch();
  };
  // A line picked from the list: selected, as a click on it on the map does.
  lines.onSelect = (l) => store.set({ selected: { way: l.way, at: l.at }, stretch: null });

  // A list item hovered (Drives, Rides, Rail lines, Sights, In view): the bottom bar shows it as a
  // hover on the map would, and a road or line too small on screen to see at a glance gets a ring.
  /** Length of a line (degrees, scaled by latitude: only for comparing lines). */
  const lineLen = (g: [number, number][]) => {
    let l = 0;
    for (let i = 1; i < g.length; i++) l += Math.hypot((g[i][0] - g[i - 1][0]) * Math.cos((g[i][1] * Math.PI) / 180), g[i][1] - g[i - 1][1]);
    return l;
  };
  /** The point halfway along a line. */
  const midpoint = (g: [number, number][]): [number, number] => {
    const half = lineLen(g) / 2;
    let l = 0;
    for (let i = 1; i < g.length; i++) {
      const d = Math.hypot((g[i][0] - g[i - 1][0]) * Math.cos((g[i][1] * Math.PI) / 180), g[i][1] - g[i - 1][1]);
      if (l + d >= half && d > 0) {
        const t = (half - l) / d;
        return [g[i - 1][0] + (g[i][0] - g[i - 1][0]) * t, g[i - 1][1] + (g[i][1] - g[i - 1][1]) * t];
      }
      l += d;
    }
    return g[0];
  };
  /** The ring around lines (lng, lat) too small on screen to see at a glance: whose smallest
   * enclosing circle (with the line's width) is under two of the largest landmark dots across. The
   * ring clears them by a comfortable margin. */
  const ringAround = (parts: [number, number][][] | null) => {
    marks.ring = null;
    const all = parts?.flat() ?? [];
    if (all.length) {
      const step = Math.max(1, Math.floor(all.length / 400));
      const pts = all.filter((_, i) => i % step === 0 || i === all.length - 1).map((q) => map.project(q));
      // Ritter's bounding circle: from the point farthest from the first to the one farthest from
      // that, grown to take in any point outside.
      const far = (a: { x: number; y: number }) => pts.reduce((b, q) => (Math.hypot(q.x - a.x, q.y - a.y) > Math.hypot(b.x - a.x, b.y - a.y) ? q : b), a);
      const p1 = far(pts[0]), p2 = far(p1);
      let cx = (p1.x + p2.x) / 2, cy = (p1.y + p2.y) / 2, r = Math.hypot(p2.x - p1.x, p2.y - p1.y) / 2;
      for (const q of pts) {
        const d = Math.hypot(q.x - cx, q.y - cy);
        if (d > r) {
          const nr = (r + d) / 2, k = (nr - r) / d;
          cx += (q.x - cx) * k;
          cy += (q.y - cy) * k;
          r = nr;
        }
      }
      const z = map.getZoom();
      // The largest landmark dot: a World Heritage site at the top of the scale (dots.ts).
      const stops = HER_R[0], i = stops.findIndex(([sz]) => sz >= z);
      const base = i < 0 ? stops[stops.length - 1][1] : i === 0 ? stops[0][1] : stops[i - 1][1] + ((stops[i][1] - stops[i - 1][1]) * (z - stops[i - 1][0])) / (stops[i][0] - stops[i - 1][0]);
      const maxDot = 2 * base * 1.25;
      const lineW = 3;
      if (2 * (r + lineW / 2) < 2 * maxDot) {
        const c = map.unproject([cx, cy]);
        marks.ring = { type: 'Feature', properties: { kind: 'ring', r: Math.max(10, r + lineW / 2 + Math.max(6, r * 0.6)) }, geometry: { type: 'Point', coordinates: [c.lng, c.lat] } };
      }
    }
    setMarks();
  };
  type ListItem =
    | { layer: RoadLayer; at: [number, number]; way?: number; geom?: [number, number][][] }
    | { feature: { layer: string; props: Record<string, any>; lngLat: [number, number] } }
    | { ferryAt: [number, number] };
  let listTok = 0;
  const listHover = (x: ListItem | null) => {
    const tok = ++listTok;
    if (!x) {
      ringAround(null);
      strip.show(null, null);
      return;
    }
    if ('feature' in x) {
      ringAround(null);
      const f = summariseFeature(x.feature.layer, x.feature.props, x.feature.lngLat);
      if (f) showFeat(f, []);
      return;
    }
    if ('ferryAt' in x) {
      ringAround(null);
      const p = map.project(x.ferryAt);
      const f = ferries.hoverAt({ x: p.x, y: p.y });
      return f ? strip.showFeature(f, []) : strip.show(null, null);
    }
    ringAround(x.geom ?? null);
    // The road's ways where known (a drive spans several), so a road crossing there isn't taken.
    const set = x.way !== undefined ? roadWays(x.way) : undefined;
    const ok = set ? (w: number) => set.has(w) : x.way !== undefined ? (w: number) => w === x.way : undefined;
    const hv = x.layer.pickNear(x.at[0], x.at[1], 4, ok) ?? x.layer.pickNear(x.at[0], x.at[1], 4);
    if (!hv) return strip.show(null, null);
    const known = peekWay(hv.way);
    strip.show(hv, known === undefined ? 'loading' : known, []);
    if (known === undefined) {
      getWay(hv.way, hv.lngLat).then((info) => {
        if (listTok === tok) strip.show(hv, info, []);
      });
    }
  };

  statsCard.onTab = (k) => {
    if (k === 'drives') drives.refresh(true);
    else if (k === 'rides') rides.refresh(true);
    else drives.onResults([]);
    if (k === 'lines') lines.refresh(true);
    if (k === 'sights') sights.refresh();
    drawStretch(); // drop hover highlights of the list left behind
  };
  onSettled(() => {
    drives.refresh();
    rides.refresh();
    lines.refresh();
  });
  // The lists follow the view while it moves too, every 1.5 s (a request at a time: a newer one
  // replaces the one under way).
  duringMoves(() => {
    drives.refresh();
    rides.refresh();
    lines.refresh();
  }, 1500);
  const applyDrivesShown = () => map.getLayer('drives-line') && map.setLayoutProperty('drives-line', 'visibility', drives.showOnMap ? 'visible' : 'none');
  drives.onShowChange = applyDrivesShown;

  statsCard.show(statsCard.tab); // restored tab: run its loaders now that callbacks exist

  statsCard.wayName = async (x) => {
    const info = await getWay(x.tile.data!.lineWay[x.line], x.lngLat);
    // Rail: the line's name without a route's direction or service codes.
    if (info && ['tram', 'metro', 'commuter', 'intercity', 'heritage'].includes(info.class)) return lineName(info);
    return info ? [info.ref, displayName(info.main, info.name, info.sub)].filter(Boolean).join(' ') : '';
  };

  // Hover readouts for the scenic channels.

  // The status line: every task under way (tasks.ts), from the map's tiles, the road and rail
  // tiles, and the app's own downloads and searches. Map events start its polling; it stops by
  // itself once nothing is busy.
  tasks.mount(strip.status);
  tasks.poll(() => {
    const out: Task[] = [];
    for (const [layer, label] of [[roads, 'Roads'], [rails, 'Rail']] as const) {
      if (layer === rails && !store.s.rail.on) continue;
      const p = layer.progress();
      if (p.loaded < p.wanted) out.push({ label, done: p.loaded, total: p.wanted, detail: 'tiles (downloaded, decoded and uploaded)' });
    }
    return out;
  });
  tasks.poll(() => mapTasks(map));
  for (const ev of ['dataloading', 'sourcedataloading', 'render', 'moveend'] as const) map.on(ev, () => tasks.kick());

  const refresh = () => {
    const p = roads.progress();
    colour.update(mdist, cur, cdf);
    railCard.update(railDist, railCur, railCdf);
    layers.updateTint(terrainD, tintCur, tintCdf);
    layers.updateFilters({ roadLen: stats?.roadLen ?? null, railFreq: railFreqDist, ferryFreq: store.s.ferry.on ? ferries.freqDist() : null });
    layers.update(stats);
    layers.updateRail(railStats);
    statsCard.update(stats, p, roads.zt, viewExtra);
  };

  // Frame loop: statistics (throttled) and eased colour range. It runs while there is something to
  // do (stats to compute, a range still easing, panels to refresh) and sleeps otherwise.
  let last = performance.now();
  let lastPanel = 0;
  let ticking = false;
  let panelsDue = false;
  const tick = (now: number) => {
    const dt = now - last;
    last = now;
    let panels = panelsDue;
    let easing = false;
    // While the camera moves, the cheap statistics only, and less often; everything once settled.
    // A pass runs STATS_MS a frame (STATS_MOVING_MS while moving) until it ends.
    if (!statsJob && statsDirty && now - lastStats > (moving ? 400 : 150)) {
      statsDirty = false;
      // While moving, the cheap pass (what the colour ranges follow), and every 2.5 s a full one:
      // the In view summary follows the view too, a little behind.
      statsJobFull = !moving || now - lastFullStats > 2500;
      if (statsJobFull) lastFullStats = now;
      statsJob = computeStats(statsJobFull);
    }
    if (statsJob) {
      const t0 = performance.now();
      const budget = moving ? STATS_MOVING_MS : STATS_MS;
      while (performance.now() - t0 < budget) {
        if (statsJob.next().done) {
          statsJob = null;
          lastStats = now;
          panels = true;
          break;
        }
      }
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
      easing = true;
    }
    {
      const r = store.s.rail;
      const rd = railMetricDef(r.metric);
      const tt: [number, number] = !r.auto ? r.range
        : !railDist || railDist.total <= 0 ? railCur
        : rd.byLen ? (railStats && railStats.totalKm > 0 ? byLen(railDist, railStats.totalKm, r.fitLen, rd.step) : railCur)
        : spread(railDist.quantile(r.fit[0] / 100), railDist.quantile(r.fit[1] / 100), rd.step * 4);
      const nr: [number, number] = [railCur[0] + (tt[0] - railCur[0]) * k, railCur[1] + (tt[1] - railCur[1]) * k];
      if (Math.abs(nr[0] - railCur[0]) + Math.abs(nr[1] - railCur[1]) > rd.step * 0.01) {
        railCur = nr;
        rails.style.range = railCur;
        map.triggerRepaint();
        panels = true;
        easing = true;
      }
    }
    if (ferryCur && ferryTarget && store.s.ferry.on) {
      const ft = ferryTarget, span = Math.max(1e-9, ft[1] - ft[0]);
      const nf: [number, number] = [ferryCur[0] + (ft[0] - ferryCur[0]) * k, ferryCur[1] + (ft[1] - ferryCur[1]) * k];
      if (Math.abs(nf[0] - ferryCur[0]) + Math.abs(nf[1] - ferryCur[1]) > span * 0.002) {
        ferryCur = nf;
        ferryEase();
        easing = true;
      } else if (ferryCur !== ft && (Math.abs(ft[0] - ferryCur[0]) + Math.abs(ft[1] - ferryCur[1]) > 0)) {
        ferryCur = ft;
        ferryEase(true);
      }
    }
    {
      // Terrain tint: measured again every 300 ms while the view changes (150 ms at rest), the
      // range easing to its target as the roads' does.
      const t = store.s.terrain;
      if (t.tint && terrainDirty && now - lastTerrain > (moving ? 300 : 150)) {
        measureTerrain();
        panels = true;
      }
      if (t.tint) {
        const tt = tintTarget();
        const kt = tintSnap ? 1 : k;
        tintSnap = false;
        const nt: [number, number] = [tintCur[0] + (tt[0] - tintCur[0]) * kt, tintCur[1] + (tt[1] - tintCur[1]) * kt];
        if (Math.abs(nt[0] - tintCur[0]) + Math.abs(nt[1] - tintCur[1]) > TINT_VARS[t.tintVar].step * 0.01) {
          tintCur = nt;
          refreshTint();
          panels = true;
          easing = true;
        }
      }
    }
    panelsDue = panels && now - lastPanel <= 60;
    if (panels && !panelsDue) {
      lastPanel = now;
      updateCdf();
      updateRailCdf();
      refreshTint();
      // The panels' DOM in idle time (idle.ts), during gestures too (the legends follow the view).
      idle.run('panels', () => {
        refresh();
        profile.redraw();
      }, { moving: true });
    }
    if (statsDirty || statsJob || easing || panelsDue || (store.s.terrain.tint && terrainDirty)) requestAnimationFrame(tick);
    else ticking = false;
  };
  function wake() {
    if (ticking) return;
    ticking = true;
    last = performance.now();
    requestAnimationFrame(tick);
  }
  /** The statistics need recomputing (and the loop wakes to do it). */
  function markDirty() {
    statsDirty = true;
    wake();
  }
  wake();

  rails.onChange = () => markDirty();
  roads.onChange = () => {
    markDirty();
    const p = roads.progress();
    if (!bootDone) {
      boot.sub(3, p.wanted ? p.loaded / p.wanted : 0, `${p.loaded}/${p.wanted}`);
      // On a timer, out of MapLibre's render pass (this runs in the road layer's prerender): the
      // pass sets up its draped textures (3D terrain) first, and threw on overlays released in it.
      if (p.wanted > 0 && p.loaded >= p.wanted) setTimeout(finishBoot, 0);
    }
  };
  map.on('move', () => {
    terrainDirty = true;
    markDirty();
    if (styleReady) updateCoastRamp(map, store.s.water);
  });
  let labelSizeTimer = 0, labelSizeAt = 0;
  const labelSizeSoon = () => {
    const run = () => {
      labelSizeTimer = 0;
      labelSizeAt = performance.now();
      applyLabelSize(map, store.s.labelSize, store.s.terrain.contour.labelSize);
    };
    if (labelSizeTimer) return;
    const wait = Math.max(0, 150 - (performance.now() - labelSizeAt));
    if (wait === 0) run();
    else labelSizeTimer = window.setTimeout(run, wait);
  };
  // New elevation or slope tiles: the terrain in view again.
  map.on('sourcedata', (e) => {
    if ((e.sourceId === 'dem-hs' || e.sourceId === 'slope') && (e as { tile?: unknown }).tile && store.s.terrain.tint) {
      terrainDirty = true;
      wake();
    }
  });
  onSettled(() => markDirty());

  // ---- hover & selection -----------------------------------------------------------
  let hovered: HoverInfo | null = null;
  // With the HUD off (I) the map is only for looking: nothing is hovered or selected.
  const hudOff = () => document.body.classList.contains('hud-off');
  let pickAt: { x: number; y: number } | null = null;
  const hex = (c: number) => `#${c.toString(16).padStart(6, '0')}`;
  const colourOf = (hv: HoverInfo) => {
    const s = store.s;
    const cls = hv.style & 15;
    if (cls >= RAIL0) {
      const r = s.rail;
      const lc = hv.tile.data!.lineColour[hv.hit.line];
      if (r.colour === 'line') return lc ? hex(lc - 1) : RAIL_GROUP_COLOURS[cls - RAIL0];
      if (r.colour === 'group') return RAIL_GROUP_COLOURS[cls - RAIL0];
      if (r.colour === 'single') return r.single;
      const v = railMetricOf(r.metric, { elev: hv.elev, grade: hv.grade, ground: hv.ground, bridge: (hv.style & 96) === 32, tunnel: (hv.style & 64) !== 0, ch: hv.ch, freq: hv.fq }, r.weights);
      if (Number.isNaN(v)) return '#5c6673';
      if (!passes(v, r.threshold, railCur)) return '#3a414c';
      return paletteRgb(r.palette, scaleU(v, railCur, r.equalize ? railCdf : null));
    }
    if (s.mode === 'map') return mapScheme(s.mapScheme).fill[cls] ?? '#888';
    const val = metricOf(s.mode, hv.elev, hv.grade, hv.ch, s.weights);
    let u = Math.max(0, Math.min(1, (val - cur[0]) / (cur[1] - cur[0])));
    if (s.equalize && cdf) u = cdf[Math.min(255, Math.floor(u * 255 + 0.5))] / 255;
    return paletteRgb(s.palette, u);
  };
  // Rail stop and ferry terminal dots in the colour of their line at that point; each shows only
  // once coloured (stations.ts, ferries.ts). In idle time (idle.ts), after a render, at most every
  // 100 ms (300 ms while the camera moves): the dots shown and not yet coloured, and all of them
  // again when the lines' colours or the rail tiles drawn change; a few milliseconds at a time.
  let dotsKey = '';
  let dotsAt = 0;
  function* colourDots(): Generator<void, void> {
    dotsAt = performance.now();
    const key = [rails.drawnCount, JSON.stringify({ ...store.s.rail, opacity: 1 }), railCur.join(), JSON.stringify(store.s.ferry), JSON.stringify(store.s.lineWeights)].join('|');
    const all = key !== dotsKey;
    dotsKey = key;
    const p = rails.progress();
    const colourAt = (lng: number, lat: number) => {
      const hv = rails.pickNear(lng, lat, 4);
      return hv === undefined ? undefined : hv ? colourOf(hv) : null;
    };
    yield* stations.recolour(colourAt, all, p.loaded >= p.wanted);
    yield* ferries.recolourTerminals(all);
  }
  const colourDotsSoon = () => {
    if (idle.has('dots')) return;
    // While the camera moves too (every 300 ms): the lines' colours ease as their range follows
    // the view, and the stops follow them.
    const wait = (moving ? 300 : 100) - (performance.now() - dotsAt);
    if (wait <= 0) idle.run('dots', colourDots, { moving: true });
    else window.setTimeout(colourDotsSoon, wait);
  };
  // Again when the view settles, stop or terminal tiles arrive, the rail drawn changes or its
  // colouring does; not on every frame drawn (a hover's highlight): the stops' query took 5–10 ms.
  let stopTiles = 0, dotsSig = '';
  map.on('sourcedata', (e) => {
    if ((e.sourceId === 'stations' || e.sourceId === 'ferries') && (e as { tile?: unknown }).tile) stopTiles++;
  });
  map.on('render', () => {
    const sig = [rails.drawnCount, stopTiles, JSON.stringify({ ...store.s.rail, opacity: 1 }), railCur.join(), JSON.stringify(store.s.ferry), JSON.stringify(store.s.lineWeights)].join('|');
    if (sig === dotsSig) return;
    dotsSig = sig;
    colourDotsSoon();
  });
  onSettled(colourDotsSoon);
  // The first Chinese, Japanese or Korean text the page lays out makes the browser find and load a
  // font for its script, 40 ms of text shaping (the first hovered road with a Japanese name, the
  // first such name in a list). Done beforehand in idle time: a few hidden words per script and
  // weight, one layout at a time.
  idle.run('fonts', function* () {
    const samples: [string, string][] = [['ja', '東京駅 とうきょう トウキョウ'], ['zh-Hans', '北京市 广州'], ['zh-Hant', '臺北市 高雄'], ['ko', '서울특별시']];
    for (const [lang, text] of samples) {
      for (const weight of ['400', '600']) {
        const el = document.createElement('span');
        el.lang = lang;
        el.setAttribute('aria-hidden', 'true');
        el.style.cssText = `position:fixed;left:-10000px;top:0;visibility:hidden;white-space:nowrap;font-weight:${weight}`;
        el.textContent = text;
        document.body.append(el);
        void el.offsetWidth;
        el.remove();
        yield;
      }
    }
  });
  const interactive = () => [...POINT_LAYERS].filter((id) => map.getLayer(id) && map.getLayoutProperty(id, 'visibility') !== 'none');
  // A hovered stop, site or area: shown at once, then again with its details when they arrive.
  let featKey = '';
  const showFeat = (f: FeatureSummary, areas: FeatureSummary[]) => {
    const key = f.ref ? refKey(f.ref) : '';
    featKey = key;
    const d = f.ref ? peekDetail(f.ref) : undefined;
    strip.showFeature(d !== undefined ? withDetails(f, d) : f, areas);
    if (f.ref && d === undefined) {
      loadDetail(f.ref).then((dd) => {
        if (featKey === key) strip.showFeature(withDetails(f, dd), areas);
      });
    }
  };
  const doPick = () => {
    const pt = pickAt;
    pickAt = null;
    // Not while the camera moves: the map's queries project through the 3D terrain, and a
    // hover readout mid-gesture is of no use.
    if (!pt || driving || moving || hudOff()) return;
    featKey = '';
    // Markers first (small targets), then the nearest road or rail line, then the highlighted
    // areas under the cursor.
    const feats = overlays.hoverAt(pt);
    // Ferries next (car ferries are in the road layer too; their ferry details say more).
    const ferry = feats.point ? null : ferries.hoverAt(pt);
    if (ferry) {
      hoverBuilding(null);
      hovered = null;
      roads.setHover(null);
      rails.setHover(null);
      map.getCanvas().style.cursor = viewshed.active || regions.picking ? 'crosshair' : 'pointer';
      hoverAreas = feats.areas;
      return strip.showFeature(ferry, feats.areas);
    }
    const hr = feats.point ? null : roads.pick(pt.x, pt.y);
    const hl = feats.point || !store.s.rail.on ? null : rails.pick(pt.x, pt.y);
    hovered = hr && hl ? (hl.px <= hr.px ? hl : hr) : hr ?? hl;
    // A road or rail line behind a building gives way to it (the lines are picked within a few
    // pixels of the cursor, hidden or not).
    const exag = store.s.terrain.on ? store.s.terrain.exaggeration : 0;
    const ray: Ray = { at: (x, y, e) => cam3d.rayAt(map, x, y, e), camera: cam3d.cameraAltitude(map) };
    if (hovered && store.s.buildings.on && hiddenByBuilding(map, hovered.lngLat, store.s.buildings, exag, ray)) hovered = null;
    const layer = hovered && hovered === hl ? rails : roads;
    (layer === rails ? roads : rails).setHover(null);
    // Highlight the whole road (or line), not just the way under the cursor (fetched once per road).
    const road = hovered ? roadWays(hovered.way) : undefined;
    layer.setHover(hovered, road ?? null);
    if (hovered && !road) {
      getRoadWays(hovered.way, hovered.lngLat).then((set) => {
        if (set && hovered && set.has(hovered.way)) layer.setHover(hovered, set);
      });
    }
    map.getCanvas().style.cursor = viewshed.active || regions.picking ? 'crosshair' : hovered || feats.point ? 'pointer' : '';
    hoverAreas = feats.areas;
    if (!hovered) {
      // A building, when no marker, road or rail line answers: the areas it's in as chips (a whole
      // old town is a heritage area, which would otherwise hide every building in it).
      const bf = feats.point ? null : buildingAt(map, pt, store.s.buildings, exag, ray)?.f ?? null;
      hoverBuilding(bf);
      if (bf) {
        roads.setHover(null);
        rails.setHover(null);
        return strip.showFeature(buildingSummary(bf), feats.areas);
      }
      const f = feats.point ?? feats.areas[0];
      return f ? showFeat(f, feats.areas) : strip.show(null, null);
    }
    hoverBuilding(null);
    const hv = hovered;
    const known = peekWay(hv.way);
    strip.show(hv, known === undefined ? 'loading' : known, hoverAreas);
    if (known === undefined) {
      getWay(hv.way, hv.lngLat).then((info) => {
        if (hovered?.way === hv.way) strip.show(hovered, info, hoverAreas);
      });
    }
  };
  let hoverAreas: FeatureSummary[] = [];
  /** The hovered building highlighted (null: none), when it changed. */
  let hoveredBuilding: unknown = null;
  const hoverBuilding = (f: maplibregl.MapGeoJSONFeature | null) => {
    const key = f ? JSON.stringify(f.geometry) : null;
    if (key === hoveredBuilding) return;
    hoveredBuilding = key;
    setBuildingHover(map, f, store.s.buildings, store.s.terrain.on ? store.s.terrain.exaggeration : 0);
  };
  // Last cursor position on the map (terrain-aware), for the Street View shortcut.
  let cursorLL: maplibregl.LngLat | null = null;
  map.on('mousemove', (e) => {
    if (!pickAt) requestAnimationFrame(doPick);
    pickAt = { x: e.point.x, y: e.point.y };
    cursorLL = e.lngLat;
  });
  map.getCanvas().addEventListener('mouseleave', () => (cursorLL = null));
  // Back on the map, a slider, dropdown or button used earlier lets go of the keyboard, so keys
  // (G, Esc, arrows) reach the map; a text field keeps it (you may still be typing).
  map.getCanvas().addEventListener('mouseenter', () => {
    const a = document.activeElement;
    // Number fields too: blurring commits them, and keys then reach the map (G, M, O).
    const num = a instanceof HTMLInputElement && a.type === 'number';
    if (a instanceof HTMLElement && a !== document.body && (num || !isTyping(a)) && !a.closest('.maplibregl-canvas-container')) a.blur();
  });
  /** No hover: the cursor left the map, or the HUD went off. */
  const unhover = () => {
    pickAt = null;
    hovered = null;
    roads.setHover(null);
    rails.setHover(null);
    hoverBuilding(null);
    strip.show(null, null);
  };
  map.getCanvas().addEventListener('mouseleave', unhover);

  let profileAbort: AbortController | null = null;
  const select = async (sel: Selection | null) => {
    profileAbort?.abort();
    profileCoords = null;
    if (sel === null) {
      profile.hide();
      map.getSource<GeoJSONSource>('selection')?.setData(line(null));
      pinned = null;
      drawStretch();
      return;
    }
    profileAbort = new AbortController();
    const info = peekWay(sel.way) ?? (await getWay(sel.way, sel.at));
    profile.loading((info && (info.ref || displayName(info.main, info.name, info.sub))) || 'this road');
    try {
      const p = await getProfile(sel.way, sel.at, profileAbort.signal);
      profile.show(p);
      map.getSource<GeoJSONSource>('selection')?.setData(line(p.coords));
      profileCoords = p.coords;
      drawStretch();
    } catch (e) {
      if ((e as Error).name !== 'AbortError') profile.error((e as Error).message);
    }
  };
  profile.onClose = () => store.set({ selected: null, stretch: null });
  profile.onZoom = (p) => {
    const b = new maplibregl.LngLatBounds();
    for (const c of p.coords) b.extend(c);
    fitGround(b, fitPad);
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
      // The drive camera follows the road's height on purpose (smoothed): zoom 14.6 from the
      // road, not from sea level (on the globe that would put it inside the mountains).
      const ground = map.queryTerrainElevation(pos) ?? 0;
      elev = elev === null ? ground : elev + (ground - elev) * Math.min(1, dt * 2);
      map.jumpTo(cam3d.frame(map, new maplibregl.LngLat(pos[0], pos[1]), elev, 14.6, 70, bearing));
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
  // (A finger's tap once it's not the first of a double tap: trackpad.ts, single.)
  map.on('click', (e) => cameraControls.single(() => click(e)));
  const click = (e: maplibregl.MapMouseEvent) => {
    if (hudOff()) return;
    if (viewshed.active) {
      viewshed.run([e.lngLat.lng, e.lngLat.lat]);
      return;
    }
    if (regions.picking) {
      void regions.pickAt([e.lngLat.lng, e.lngLat.lat]);
      return;
    }
    if (overlays.click(e.point, POINT_LAYERS)) return;
    if (ferries.click(e.point, e.lngLat)) {
      overlays.closePopup();
      return;
    }
    ferries.closePopup();
    const hr = roads.pick(e.point.x, e.point.y);
    const hl = store.s.rail.on ? rails.pick(e.point.x, e.point.y) : null;
    const hv = hr && hl ? (hl.px <= hr.px ? hl : hr) : hr ?? hl;
    // A World Heritage line (a canal, a wall) against the roads and rail: the nearer wins.
    const wl = overlays.whsLineAt(e.point);
    if (wl && (!hv || wl.px <= hv.px) && overlays.click(e.point, ['whs-line'])) return;
    if (hv) {
      overlays.closePopup();
      store.set({ selected: { way: hv.way, at: hv.lngLat }, stretch: null });
      return;
    }
    overlays.click(e.point, AREA_LAYERS);
  };
  layers.onViewshed = () => (viewshed.active ? viewshed.cancel() : viewshed.start());
  layers.trees.onPreview = (palette) => applyTrees(map, palette ? { ...store.s.trees, palette } : store.s.trees);
  viewshed.onActive = (on) => layers.setViewshedActive(on);
  viewshed.onMark = (ll) => {
    marks.viewshed = ll ? point(ll, 'viewshed') : null;
    setMarks();
  };
  // ---- G / M / O: Street View, Google Maps, OpenStreetMap --------------------------------------
  // For what is under the pointer: a list entry (drives, rides, rail lines, sights), a point on
  // the profile, else the map at the cursor (snapped to the hovered road). Google Maps shows a
  // road or place by searching its name there, else a pin; OpenStreetMap opens its object, shown
  // selected (a road's way, a rail line's route relation, a place's node or way), else a marker. Street View: roads, facing along the road
  // (at the cursor: the map's bearing).
  interface LinkTarget {
    /** A point on it: the pin or marker there, and Street View. */
    lngLat: [number, number];
    /** MapLibre zoom framing it (else the map's). */
    zoom?: number;
    search?: string;
    /** Its OSM object ('way/123'); a promise while it loads; null: none. */
    osm?: string | null | Promise<string | null>;
    streetView: boolean;
    heading?: number;
    /** For the message: "at the cursor", "for “Mount Washington”". */
    what: string;
  }
  /** A way's OSM object (its id is the OSM way id). */
  const wayOsm = (way: number): string => `way/${way}`;
  /** A landmark's OSM object, from its details record. */
  const sightOsm = (x: Sight): string | null | Promise<string | null> => {
    const ref = landmarkRef(x.layer, x.props, x.lngLat);
    if (!ref) return null;
    const d = peekDetail(ref);
    return d !== undefined ? osmPath(d?.osm) : loadDetail(ref).then((dd) => osmPath(dd?.osm), () => null);
  };
  /** Halfway along a line, and the heading there (degrees from north). */
  const halfway = (g: [number, number][]): { at: [number, number]; heading: number } => {
    const k = Math.cos((g[0][1] * Math.PI) / 180);
    const seg = g.slice(1).map((q, i) => Math.hypot((q[0] - g[i][0]) * k, q[1] - g[i][1]));
    let rest = seg.reduce((a, b) => a + b, 0) / 2;
    let i = 0;
    while (i < seg.length - 1 && rest > seg[i]) rest -= seg[i++];
    const [a, b] = [g[i], g[i + 1] ?? g[i]];
    const t = seg[i] ? Math.min(1, rest / seg[i]) : 0;
    return { at: [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], heading: ((Math.atan2((b[0] - a[0]) * k, b[1] - a[1]) * 180) / Math.PI + 360) % 360 };
  };
  const frameZoom = (g: [number, number][]) => map.cameraForBounds(boundsOf(g), { padding: 40 })?.zoom;
  const named = (name: string) => (name ? `for “${name}”` : '');
  // (Google Maps searches the name itself; the message gives it as the list shows it.)
  const linkTarget = (): LinkTarget | null => {
    const tab = statsCard.tab;
    if (tab === 'drives' && drives.hovered) {
      const d = drives.hovered, name = cap(d.name) || d.ref, mid = halfway(d.geom);
      return { lngLat: mid.at, heading: mid.heading, zoom: frameZoom(d.geom), search: name || undefined, osm: wayOsm(d.way), streetView: true, what: named(cap(displayName(d.main, d.name, d.sub)) || d.ref) || 'for this drive' };
    }
    if (tab === 'rides' && rides.hovered) {
      const r = rides.hovered;
      return { lngLat: halfway(r.geom).at, zoom: frameZoom(r.geom), osm: r.rel ? `relation/${r.rel}` : wayOsm(r.way), streetView: false, what: named(cap(displayName(r.main, r.name, r.sub))) || 'for this ride' };
    }
    if (tab === 'lines' && lines.hovered) {
      const l = lines.hovered;
      const longest = l.geom.reduce((a, b) => (b.length > a.length ? b : a), l.geom[0]);
      return { lngLat: halfway(longest).at, zoom: frameZoom(l.geom.flat()), osm: l.rel ? `relation/${l.rel}` : wayOsm(l.way), streetView: false, what: named(cap(displayName(l.main, l.name, l.sub))) || 'for this line' };
    }
    if (tab === 'sights' && sights.hovered) {
      const x = sights.hovered, name = cap(x.props.name);
      return { lngLat: x.lngLat, zoom: Math.max(map.getZoom(), 15), search: name || undefined, osm: sightOsm(x), streetView: false, what: named(cap(displayOf(x.props))) || 'for this place' };
    }
    const hp = profile.hoverPoint();
    if (hp) return { lngLat: hp.lngLat, heading: hp.heading, streetView: !hp.rail, what: 'at this point' };
    if (cursorLL) return { lngLat: hovered ? hovered.lngLat : [cursorLL.lng, cursorLL.lat], streetView: true, what: 'at the cursor' };
    return null;
  };
  const openLink = (k: string, t: LinkTarget) => {
    const [lng, lat] = t.lngLat;
    const la = lat.toFixed(6), lo = lng.toFixed(6), ll = `${la},${lo}`;
    // Google Maps and OSM zoom levels are one higher than MapLibre's (256 vs 512 px tiles).
    const z = (t.zoom ?? map.getZoom()) + 1;
    if (k === 'g') {
      if (!t.streetView) return toast('Street View is for roads');
      const heading = (((t.heading ?? map.getBearing()) % 360) + 360) % 360;
      const q = new URLSearchParams({ api: '1', map_action: 'pano', viewpoint: ll, heading: heading.toFixed(0), pitch: '0', fov: '90' });
      window.open(`https://www.google.com/maps/@?${q.toString().replace(/%2C/g, ',')}`, '_blank', 'noopener');
      return toast(`Opening Street View ${t.what} (nearest panorama, if any)`);
    }
    if (k === 'm') {
      const gz = Math.min(21, Math.max(3, z)).toFixed(1);
      window.open(t.search ? `https://www.google.com/maps/search/${encodeURIComponent(t.search)}/@${ll},${gz}z` : `https://www.google.com/maps/place/${ll}/@${ll},${gz}z`, '_blank', 'noopener');
      return toast(`Opening Google Maps ${t.what}`);
    }
    const oz = Math.round(Math.min(19, Math.max(3, z)));
    const url = (osm: string | null | undefined) => (osm ? `https://www.openstreetmap.org/${osm}` : `https://www.openstreetmap.org/?mlat=${la}&mlon=${lo}#map=${oz}/${la}/${lo}`);
    if (t.osm instanceof Promise) {
      // Still loading: the tab opens now, while the key press allows one, and goes there once known.
      const w = window.open('', '_blank');
      if (w) {
        w.opener = null;
        t.osm.then((o) => (w.location.href = url(o)));
      }
    } else window.open(url(t.osm), '_blank', 'noopener');
    toast(`Opening OpenStreetMap ${t.what}`);
  };
  // A long press on the map (a touch screen's G, M and O): a menu of them for the place pressed.
  let pressMenu: HTMLElement | null = null;
  const closePress = () => {
    pressMenu?.remove();
    pressMenu = null;
  };
  onLongPress = (px, py) => {
    closePress();
    const ll = map.unproject([px, py]);
    const t: LinkTarget = { lngLat: [ll.lng, ll.lat], streetView: true, what: 'here' };
    const box = map.getContainer();
    const menu = document.createElement('div');
    menu.className = 'press-menu';
    menu.append(Object.assign(document.createElement('div'), { className: 'pm-hd', textContent: `${ll.lat.toFixed(5)}, ${ll.lng.toFixed(5)}` }));
    for (const [k, label] of [['g', 'Street View here'], ['m', 'Google Maps here'], ['o', 'OpenStreetMap here']] as const) {
      const b = Object.assign(document.createElement('button'), { type: 'button', textContent: label });
      b.onclick = () => {
        closePress();
        openLink(k, t);
      };
      menu.append(b);
    }
    box.append(menu);
    // (Inside the map, beside the finger, not under it.)
    const w = menu.offsetWidth, hgt = menu.offsetHeight;
    menu.style.left = `${Math.max(8, Math.min(px + 12, box.clientWidth - w - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(py - hgt - 12 < 8 ? py + 12 : py - hgt - 12, box.clientHeight - hgt - 8))}px`;
    pressMenu = menu;
  };
  document.addEventListener('pointerdown', (e) => {
    if (pressMenu && !pressMenu.contains(e.target as Node)) closePress();
  }, true);
  map.on('movestart', closePress);

  window.addEventListener('keydown', (e) => {
    const k = e.key.toLowerCase();
    const SITES: Record<string, string> = { g: 'Street View', m: 'Google Maps', o: 'OpenStreetMap' };
    if (SITES[k] && !e.metaKey && !e.ctrlKey && !e.altKey) {
      if (isTyping(e.target)) return;
      const t = linkTarget();
      if (!t) return toast(`Point at the map, a list entry or the profile, then press ${k.toUpperCase()} for ${SITES[k]}`);
      // Not for a focused control (a dropdown would jump to an option starting with the letter).
      e.preventDefault();
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
      openLink(k, t);
      return;
    }
    // /: the place search (when it's there: not with the HUD off).
    if (e.key === '/' && !e.metaKey && !e.ctrlKey && !e.altKey && !isTyping(e.target) && search.visible) {
      e.preventDefault();
      search.focus();
      return;
    }
    // B: the 3D buildings on and off (when the catalog has them).
    if (k === 'b' && hasBuildings && !e.metaKey && !e.ctrlKey && !e.altKey && !isTyping(e.target)) {
      e.preventDefault();
      const on = !store.s.buildings.on;
      store.set({ buildings: { ...store.s.buildings, on } });
      toast(on ? 'Buildings on (B)' : 'Buildings off (B)');
      return;
    }
    // I: the HUD (panels, bottom bar, controls) off and on, the map filling the window.
    if (k === 'i' && !e.metaKey && !e.ctrlKey && !e.altKey && !isTyping(e.target)) {
      e.preventDefault();
      const off = document.body.classList.toggle('hud-off');
      map.resize();
      if (off) {
        // Nothing stays hovered or selected (no popup either) while it's off.
        unhover();
        map.getCanvas().style.cursor = '';
        overlays.closePopup();
        ferries.closePopup();
        store.set({ selected: null, stretch: null });
        toast('Press I to bring the panels back');
      }
      return;
    }
    if (e.key !== 'Escape') return;
    if (document.querySelector('dialog[open]')) return; // the dialog closes itself
    if (driving) return stopDrive();
    if (viewshed.active) return viewshed.cancel();
    if (regions.picking) return regions.stopPicking();
    overlays.closePopup();
    store.set({ selected: null, stretch: null });
  });

  // ---- state → map ------------------------------------------------------------------
  // A true globe for all but street level, so tilted views show the real horizon distance and dip
  // and far ranges sinking below it; it hands over to flat Web Mercator at zoom 15.5–16.5, where
  // the horizon is 30–80 km out in the fog (and globe rendering, in float32, would wobble).
  // The cursor-anchored camera handles both (camera3d.ts).
  const applyProjection = () =>
    map.setProjection({
      type: store.s.globe
        ? (['interpolate', ['linear'], ['zoom'], 15.5, 'vertical-perspective', 16.5, 'mercator'] as unknown as 'globe')
        : 'mercator',
    });
  const applyLayers = () => {
    const vis = (id: string, show: boolean) => {
      if (map.getLayer(id)) map.setLayoutProperty(id, 'visibility', show ? 'visible' : 'none');
    };
    for (const [k, ids] of Object.entries(LAYER_GROUPS)) {
      const on = store.s.layers[k as 'water' | 'boundaries'];
      ids.forEach((id, i) => vis(id, on && (k !== 'boundaries' || store.s.boundaryLevels[i])));
    }
    vis('boundary-country-disputed', store.s.layers.boundaries && store.s.boundaryLevels[0]);
    // Place and water labels (overlay and ferry labels follow their layers: overlays.ts, ferries.ts).
    for (const k of ['city', 'town', 'village', 'minor', 'state'] as const) for (const id of LABEL_LAYERS[k]) vis(id, labelShown(store.s, k));
    for (const id of LABEL_LAYERS.water) vis(id, store.s.layers.water && labelShown(store.s, 'water'));
  };
  let hashTimer = 0;
  const writeHash = () => {
    clearTimeout(hashTimer);
    hashTimer = window.setTimeout(() => history.replaceState(null, '', toHash(store.s, hasBuildings)), 150);
    prefs.saveSoon('state', () => ({ ...store.s, selected: null }));
  };
  let styleReady = false;
  let lastExaggeration = store.s.terrain.on ? store.s.terrain.exaggeration : 0;
  // Colour by height: the shared colour scale over the buildings in view (their tiles' heights,
  // read when the map is idle: buildings.onHeights), auto-fit to its percentiles as the roads'.
  let bldDist: Dist | null = null;
  let bldLook: HeightLook = { range: [...store.s.buildings.height.range], cdf: null };
  let bldPreview: string | null = null;
  const bldLookNow = (): HeightLook => {
    const sc = store.s.buildings.height;
    const range: [number, number] = !sc.auto ? [...sc.range] : bldDist && bldDist.total > 0 ? spread(bldDist.quantile(sc.fit[0] / 100), bldDist.quantile(sc.fit[1] / 100), 4) : bldLook.range;
    return { range, cdf: sc.equalize ? cdfOf(bldDist, range) : null };
  };
  /** The buildings' settings applied (the terrain's exaggeration and light are theirs too). */
  const applyBuildingsNow = (s: AppState) => {
    // Bridges and elevated rail drawn after the buildings while they stand in 3D (roads/layer.ts
    // bridgeLayer): placed after them, before the first symbol layer.
    const apart = !!map.getLayer('buildings') && s.buildings.on && !s.buildings.flat && s.terrain.on;
    for (const [l, id] of [[roads, 'roads-bridges'], [rails, 'rails-bridges']] as const) {
      if (!map.getLayer(id) && map.getLayer('buildings')) map.addLayer(l.bridgeLayer(id), 'water-name-line');
      if (l.bridgesApart !== apart) {
        l.bridgesApart = apart;
        map.triggerRepaint();
      }
    }
    bldLook = bldLookNow();
    const b = bldPreview ? { ...s.buildings, height: { ...s.buildings.height, palette: bldPreview } } : s.buildings;
    applyBuildings(map, b, s.terrain.on ? s.terrain.exaggeration : 0, s.terrain.light, bldLook);
    layers.buildings.updateHeights(bldDist, bldLook);
  };
  onHeights(map, (d) => {
    bldDist = d;
    if (styleReady && store.s.buildings.colour === 'height') applyBuildingsNow(store.s);
  });
  layers.buildings.onPalettePreview = (k) => {
    bldPreview = k;
    if (styleReady) applyBuildingsNow(store.s);
  };
  store.on((s, ch) => {
    const st = roads.style;
    st.mode = s.mode;
    st.palette = s.palette;
    st.classMask = classMask(s);
    st.surfaceMask = surfaceMask(s);
    st.tollMask = tollMask(s);
    st.unnamedHide = unnamedHideClasses(s);
    st.weight = ROAD_WEIGHT * lineWeight(s, 'roads');
    st.threshold = { ...s.threshold };
    st.visible = s.layers.roads;
    st.weights = [...s.weights];
    st.equalize = s.equalize;
    st.routeGlow = s.routeGlow;
    st.terrain3d = s.terrain.on;
    st.exaggeration = s.terrain.exaggeration;
    st.lowFade = s.lowFade;
    st.lowSpan = s.lowSpan;
    st.occlude = s.occlude;
    st.opacity = s.roadOpacity;
    [st.lenMin, st.lenMax] = roadLenM(s);
    dots.setTerrain({ on: s.terrain.on, exaggeration: s.terrain.exaggeration, occlude: s.occlude });
    contours.set(contourStyle(s));
    applyMapMode(s);
    applyRailStyle(s);
    if (ch.has('rail')) {
      markDirty();
      railCard.sync();
      if (s.rail.on) loadRailFreq();
      map.triggerRepaint();
    }
    if ((ch.has('layers') || ch.has('labelKinds') || ch.has('labelDensity')) && styleReady) overlays.apply(s);
    if ((ch.has('rail') || ch.has('layers') || ch.has('lineWeights') || ch.has('labelKinds') || ch.has('labelDensity')) && styleReady) stations.apply(s);
    if ((ch.has('ferry') || ch.has('layers') || ch.has('lineWeights') || ch.has('labelKinds')) && styleReady) {
      ferries.apply(s);
      updateFerries();
    }
    if (ch.has('ferry')) ferryCard.sync();
    if (ch.has('trees') && styleReady) applyTrees(map, s.trees);
    if ((ch.has('buildings') || ch.has('terrain')) && styleReady) {
      applyBuildingsNow(s);
      if (!s.buildings.on) hoverBuilding(null);
    }
    if (ch.has('groups') || ch.has('unnamed') || ch.has('roadLen') || ch.has('roadLenOn') || ch.has('surface') || ch.has('toll') || ch.has('layers') || ch.has('mode') || ch.has('weights')) markDirty();
    if (ch.has('equalize') || ch.has('mode')) cdfKey = '';
    if (styleReady) {
      if (ch.has('layers') || ch.has('boundaryLevels') || ch.has('labelKinds')) applyLayers();
      if (ch.has('terrain')) {
        // Terrain on/off or re-exaggerated: re-pivot for the new ground (camera stays put).
        if ((s.terrain.on ? s.terrain.exaggeration : 0) !== lastExaggeration) {
          if (!s.terrain.on) cam3d.repivot(map, 0); // flat map: pivot at sea level
          else map.on('idle', levelOnce);
        }
        lastExaggeration = s.terrain.on ? s.terrain.exaggeration : 0;
        applyTerrain(map, s.terrain);
        applyLabelOpacity(map, s.labelOpacity, overlayLabelScale(s.poiOpacity));
      }
      // Label sizes lay the labels out again: at most every 150 ms while a slider is dragged.
      if (ch.has('labelSize') || ch.has('terrain')) labelSizeSoon();
      if (ch.has('water') || ch.has('layers')) applyWater(map, s.water, coastInput, s.layers.water);
      // (after the terrain: contour lines are added when first shown)
      if (ch.has('lineWeights') || ch.has('terrain')) applyLineWidths(map, s.lineWeights);
      if (ch.has('terrain') || ch.has('palette') || ch.has('mode')) {
        // Another variable (metres ↔ percent) or the tint just shown: measure and snap to it.
        const tk = `${s.terrain.tint}|${s.terrain.tintVar}`;
        if (tk !== tintKey) {
          tintKey = tk;
          tintSnap = true;
          terrainD = null;
          terrainDirty = true;
        }
        refreshTint();
        wake();
      }
      if (ch.has('labelOpacity') || ch.has('poiOpacity')) applyLabelOpacity(map, s.labelOpacity, overlayLabelScale(s.poiOpacity));
      if (ch.has('poiOpacity')) applyOverlayOpacity(map, s.poiOpacity);
      if (ch.has('boundaryOpacity')) applyBoundaryOpacity(map, s.boundaryOpacity);
      if (ch.has('labelDensity')) {
        applyLabelDensity(map, s.labelDensity);
        setHorizonThinning(map, s.labelDensity.horizon);
      }
      if (ch.has('poiOpacity') || ch.has('poiEmphasis') || ch.has('landmarks') || ch.has('labelOpacity')) overlays.prominence(s);
      if (ch.has('globe')) applyProjection();
      if (ch.has('overlays') || ch.has('heritageOff') || ch.has('stopFilters') || ch.has('stopUnknown')) overlays.apply(s);
      // (The buildings' heritage tint follows the sites shown.)
      if (ch.has('overlays') || ch.has('heritageOff') || ch.has('stopFilters') || ch.has('stopUnknown')) heritageChanged(map, s.overlays.heritage);
    }
    if (ch.has('groups') || ch.has('unnamed') || ch.has('roadLen') || ch.has('roadLenOn') || ch.has('surface') || ch.has('toll')) drives.refresh();
    if (ch.has('weights')) drives.refresh();
    if (ch.has('rail')) {
      rides.refresh();
      lines.refresh();
    }
    if (ch.has('selected')) select(s.selected);
    if (ch.has('selected') || ch.has('stretch')) applyStretch();
    if (!ch.has('view')) {
      colour.sync();
      stopsCard.sync();
      layers.sync(s);
      refresh();
      if (ch.has('weights') || ch.has('mode') || ch.has('palette')) profile.redraw();
    }
    map.triggerRepaint();
    writeHash();
    wake(); // the colour range may have a new target
  });
  // Keep the zoom level meaningful: after every move, put the camera pivot on the terrain at the
  // view centre *without moving the camera* (zoom and centre are re-solved from the camera
  // position). Nothing moves on screen; tile detail, line widths and labels follow the real
  // distance to the ground. The camera itself never follows the terrain.
  const relevel = () => {
    if (driving || !store.s.terrain.on) return;
    cam3d.relevel(map);
  };
  // Only once a gesture has settled: mid-gesture the zoom number (and with it widths, labels and
  // tile detail) must change smoothly. First of the settled steps (the rest see the new zoom); the
  // camera doesn't move, so it doesn't start another settle.
  settledFns.unshift(() => {
    quiet = true;
    try {
      relevel();
    } finally {
      quiet = false;
    }
  });

  // Reset to defaults: every saved preference and preset list cleared, the state the defaults with
  // the map where it is (the address bar and the saved state too, so that nothing pending brings
  // the old back), and the page loaded again for the panels' own preferences.
  layers.onReset = () => {
    prefs.clearAll();
    store.set({ ...structuredClone(defaults), view: store.s.view });
    history.replaceState(null, '', toHash(store.s, hasBuildings));
    prefs.save('state', { ...store.s, selected: null });
    location.reload();
  };

  // Pasted / edited links: apply the new state without a reload.
  window.addEventListener('hashchange', () => {
    const next = fromHash(location.hash, hasBuildings);
    const { view, buildings, ...rest } = next;
    store.set(hasBuildings ? { ...rest, buildings } : rest);
    if (view) map.jumpTo({ center: [view.lng, view.lat], zoom: view.zoom, bearing: view.bearing, pitch: view.pitch });
  });
  onSettled(() => {
    if (driving) return;
    const c = map.getCenter();
    const elev = map.getCenterElevation();
    store.set({ view: { zoom: map.getZoom(), lat: c.lat, lng: c.lng, bearing: map.getBearing(), pitch: map.getPitch(), elev } });
  });

  // ---- new data, the build Mac, regions -------------------------------------------------
  // A new catalog (catalog.ts) switches the map to its data in place: the modules that fetch data
  // follow its versions by themselves (api.ts onVersions), the map's tile sources here, and what
  // can't follow asks for a reload (the status bar offers it). The status bar shows the build Mac
  // and the NAS from the same answers.
  let watch: CatalogWatch | null = null;
  onVersions(['roads.tiles'], () => roads.setSource(version('roads.tiles'), roads.bounds));
  onVersions(['rails.tiles'], () => rails.setSource(version('rails.tiles'), rails.bounds));
  onVersions(['buildings.tiles'], () => switchBuildings(map));
  onVersions(['rail-freq.bin'], () => {
    if (railFreqFor !== null && store.s.rail.on) loadRailFreq();
  });
  onVersions(versionedTiles().map((t) => t.file), (files) => {
    for (const t of versionedTiles()) {
      if (files.includes(t.file)) (map.getSource(t.source) as { setTiles?: (tiles: string[]) => void } | undefined)?.setTiles?.([t.url]);
    }
    if (files.includes('base.pmtiles') || files.includes('water')) switchCoast(map, coastInput());
    if (files.includes('terrain.tiles')) switchContours(map);
  });
  const newCatalog = (m: Meta) => {
    // What the versions don't say: the data's bounds (where tiles are asked for), and whether the
    // labels come from our tiles (the style is made for one or the other).
    roads.setSource(version('roads.tiles'), m.bounds);
    rails.setSource(version('rails.tiles'), m.bounds);
    // The buildings, once a catalog has them: the layers, their settings, the B key and `bd=`.
    if (m.layers?.buildings && !hasBuildings) {
      hasBuildings = true;
      layers.showBuildings(true);
      writeHash();
    }
    if (m.layers?.buildings && styleReady && !map.getSource('bld')) {
      addBuildings(map, 'water-name-line', 'boundary-county');
      applyBuildingsNow(store.s);
      heritageChanged(map, store.s.overlays.heritage);
    }
    if (!!m.labelTiles !== labelTilesOn() || !!m.ovTiles !== ovTilesOn() || !!m.stationTiles !== stationTilesOn() || !!m.water !== waterTilesOn() || !!m.ferryBlocks !== ferries.byBlocks) watch?.wantReload('New map data');
    markDirty();
  };
  regions.onFit = (b) => fitGround(new maplibregl.LngLatBounds([b[0], b[1]], [b[2], b[3]]), { top: 60, bottom: 60, left: 60, right: 340 });
  // Keep this view: the ground on screen, as the lists "in view" take it.
  regions.viewOutline = () => groundOutline().map((ll) => [ll.lng, ll.lat] as [number, number]);
  regions.onPicking = (on) => (map.getCanvas().style.cursor = on ? 'crosshair' : '');
  regions.onChanged = () => void watch?.poll();

  // ---- boot --------------------------------------------------------------------------
  let bootDone = false;
  const finishBoot = () => {
    if (bootDone) return;
    bootDone = true;
    boot.done();
    releaseOverlays();
  };
  const releaseOverlays = () => {
    overlays.release();
    stations.release();
    ferries.release();
  };
  boot.at(2);
  // Attach as soon as the style is parsed; basemap tiles keep streaming in behind.
  // (An inline style can finish parsing before listeners are registered.)
  const attach = () => {
    if (map.getLayer('roads')) return;
    styleReady = true;
    applyProjection();
    map.addLayer(roads, 'water-name-line');
    map.addLayer(rails, 'water-name-line');
    // The 3D buildings after the road and rail layers (a road in front stays in front: it lies on
    // the terrain, whose depth they're tested against), their footprints among the draped layers
    // before the boundaries (docs/buildings3d.md §4.2).
    if (meta.layers?.buildings) {
      addBuildings(map, 'water-name-line', 'boundary-county');
      heritageChanged(map, store.s.overlays.heritage);
    }
    // Under the roads, above every layer draped on the terrain (one between them would split the
    // draping in two: the terrain drawn twice).
    map.addLayer(contours, 'roads');
    // Under every landmark name (and above the parts of World Heritage Sites).
    map.addLayer(dots, `poi-${Object.keys(POI_STYLE)[0]}`);
    applyLayers();
    applyTerrain(map, store.s.terrain);
    applyLabelSize(map, store.s.labelSize, store.s.terrain.contour.labelSize);
    applyWater(map, store.s.water, coastInput, store.s.layers.water);
    applyLineWidths(map, store.s.lineWeights);
    applyTrees(map, store.s.trees);
    applyBuildingsNow(store.s);
    applyLabelOpacity(map, store.s.labelOpacity, overlayLabelScale(store.s.poiOpacity));
    applyOverlayOpacity(map, store.s.poiOpacity);
    applyBoundaryOpacity(map, store.s.boundaryOpacity);
    setHorizonThinning(map, store.s.labelDensity.horizon);
    refreshTint();
    overlays.apply(store.s);
    ferries.apply(store.s);
    stations.apply(store.s);
    applyDrivesShown();
    regions.start();
    // The catalog and the build Mac from now on (a new catalog finds the style's sources there).
    watch = new CatalogWatch(meta.catalog);
    watch.onSwitch = newCatalog;
    // Newer points on the server than the page has (a 409): the catalog now, not in a minute.
    overlays.onStale = () => void watch?.poll();
    new BuildStatus(strip.buildStatus, watch);
    // How far each region is built, from the build Mac's heartbeat; the coverage drawn is the
    // catalog's, so it follows a new one.
    const w = watch;
    w.on(() => regions.setProgress(w.status?.agent?.built));
    w.on(() => regions.setCatalog(w.status?.n));
    // The credits of the sources the catalog's data comes from.
    w.on(() => strip.setCredits(w.status?.credits));
    // Landmarks by view when the catalog has them (else the whole files).
    w.on(() => {
      if (w.status) overlays.setMarks(w.status.marks ?? null);
      else if (w.unreachable) overlays.setMarks(null);
    });
    // The NAS back (or the server): what failed meanwhile is asked for now.
    let reachable = true;
    w.on(() => {
      const now = !w.unreachable && w.status?.online !== false;
      if (now && !reachable) {
        tileRetry.now();
        roads.retryNow();
        rails.retryNow();
      }
      reachable = now;
    });
    boot.at(3);
    if (store.s.selected !== null) {
      select(store.s.selected);
      applyStretch();
    }
    // Never block the UI for long on a slow first view; overlays start within 2 s either way.
    setTimeout(finishBoot, 12000);
    // On another device (an iPhone, an iPad), the app kept for when the Mac can't be reached, and
    // the map looked at with it (public/sw.js). Not on the Macs: they have the data themselves.
    const local = /^(localhost|127\.0\.0\.1|\[::1\])$|\.localhost$/.test(location.hostname);
    // (Checked for a new one whenever the app comes back to the front: an installed app may stay
    // open for days.)
    if (!local && window.isSecureContext && 'serviceWorker' in navigator) {
      navigator.serviceWorker.register('/sw.js').then((reg) => {
        document.addEventListener('visibilitychange', () => document.visibilityState === 'visible' && reg.update().catch(() => undefined));
      }, () => undefined);
    }
    setTimeout(releaseOverlays, 2000);
  };
  if ((map as unknown as { style?: { _loaded?: boolean } }).style?._loaded) attach();
  else {
    map.once('style.load', attach);
    map.once('load', attach);
  }
  map.on('error', (e) => console.warn(e.error?.message ?? e));
  (window as any).__app = { map, roads, rails, store, cam3d, ferries, dots, overlays, idle, contours, regions, catalog: () => watch };
}

/**
 * MapLibre eases the camera pivot toward the terrain under the target on every animation
 * (easeTo, flyTo, fitBounds) even with centerClampedToGround off. The camera never follows the
 * terrain here: keep only the bookkeeping part (minimum elevation for clipping).
 */
function unlinkCameraFromTerrain(map: maplibregl.Map) {
  type Cam = { terrain?: { getMinTileElevationForLngLatZoom: (c: unknown, z: number) => number }; _updateElevation?: (k: number, tr: any) => void };
  const cam = (map as unknown as { _camera?: Cam })._camera;
  const orig = cam?._updateElevation;
  if (!cam || !orig) return;
  cam._updateElevation = function (_k: number, tr: any) {
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

/** Whether key presses on this element are text being typed (not a slider, dropdown or button). */
function isTyping(el: EventTarget | null): boolean {
  if (el instanceof HTMLTextAreaElement) return true;
  if (el instanceof HTMLInputElement) return !['range', 'checkbox', 'radio', 'button', 'submit', 'reset', 'color', 'file'].includes(el.type);
  return el instanceof HTMLElement && el.isContentEditable;
}

main();
